//! Borrow the same desktop services for platform-originated task commands.
//! This does not borrow/replace the root VM or synthesize script instructions.
use anyhow::Result;

use super::{HeadlessPlayer, LoggingHost};

impl HeadlessPlayer {
    /// The callback remains unacknowledged while its task awaits audio.
    /// Callers may advance unrelated tasks without advancing this cursor.
    pub(super) fn root_dispatch_waits_for_audio(&mut self) -> Result<bool> {
        let target = self.data_services.tasks.pending_root_dispatch()
            .map_or(self.current_task_id, |(target, _)| target);
        self.with_host(|host| Ok(host.task_waits_for_audio(target)))
    }

    pub(super) fn service_pending_root_dispatch(&mut self, budget: usize) -> Result<bool> {
        let Some((target, mut message)) = self.data_services.tasks.pending_root_dispatch() else {
            return Ok(true);
        };
        if target != self.current_task_id {
            anyhow::bail!("pending platform dispatch targets another external task");
        }
        let original = message;
        let mut vm = self.vm.take()
            .ok_or_else(|| anyhow::anyhow!("pending root task dispatch has no loaded VM"))?;
        let result = self.with_host(|host| {
            Ok(host.dispatch_external_task_message(&mut vm, &mut message, budget)?)
        });
        self.vm = Some(vm);
        match result? {
            Some(cs2_script::ResumeStep::Yield) => Ok(false),
            step => {
                self.data_services.tasks.complete_root_dispatch_step(target, original, message, step)?;
                Ok(true)
            }
        }
    }

    pub(super) fn with_host<T>(
        &mut self, call: impl FnOnce(&mut LoggingHost<'_>) -> Result<T>,
    ) -> Result<T> {
        let mut host = LoggingHost {
            events: &mut self.events,
            frame: self.state.clock.frame,
            resources: &self.scan.resources,
            xml_handles: &mut self.xml_handles,
            next_xml_handle: &mut self.next_xml_handle,
            configured_shared_slots: self.configured_shared_slots,
            text_encoding: self.script_encoding,
            shared_values: &mut self.shared_values,
            current_parameter: &mut self.current_parameter,
            current_task_id: self.current_task_id,
            pointer_position: [self.state.input.cursor_x, self.state.input.cursor_y],
            task_time: self.state.clock.elapsed_ticks as u32,
            task_origin: 0,
            auto_select: self.auto_select,
            scene_animation_enabled: 1,
            fullscreen: 0,
            fullscreen_mw_pos: 0,
            string_values: &mut self.string_values,
            next_task_id: &mut self.next_task_id,
            task_parents: &mut self.task_parents,
            scene_nodes: &mut self.scene_nodes,
            image_handles: &mut self.image_handles,
            image_resources: &mut self.image_resources,
            backlogs: &mut self.backlogs,
            canvas: Some(&mut self.graphics_canvas),
            back_buffer: Some(&self.canvas),
            particles: &mut self.particles,
            record_stacks: &mut self.record_stacks,
            input_task: &mut self.input_task,
            task_vms: &mut self.task_vms,
            script_replacements: &self.script_replacements,
            system_config_blocks: &mut self.system_config_blocks,
            css_handles: &mut self.css_handles,
            integer_maps: &mut self.integer_maps,
            data_services: &mut self.data_services,
            cst_contexts: &mut self.cst_contexts,
            motion_controllers: &mut self.motion_controllers,
            motion_snapshots: &mut self.motion_snapshots,
            save_root: &self.save_root,
            message_wait_address: &mut self.message_wait_address,
            audio_host: &mut self.audio_host,
            movie_host: Some(&mut self.movie_host),
            movie_playbacks: &mut self.movie_playbacks,
            movie_resources: &mut self.movie_resources,
            sound_channels: &mut self.sound_channels,
            sound_groups: &self.sound_groups,
            sound_config: &self.sound_config,
            script_random: &mut self.script_random,
            menu_item_requests: &mut self.menu_item_requests,
            debug_mode: self.debug_mode,
            sound_config_enabled: self.sound_config_enabled,
        };
        call(&mut host)
    }
}

#[cfg(test)]
mod async_callback_tests {
    use super::*;
    use crate::{PlayerFramePhase, SoundChannelState, sound_async};
    use cs2_runtime::{AudioChannel, AudioRequest, AudioStart};
    use cs2_script::{KcsProgram, Operand, Vm};

    fn callback_player() -> (std::path::PathBuf, HeadlessPlayer, u32,
        std::sync::mpsc::Sender<cs2_runtime::RuntimeResult<()>>)
    {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let directory = std::env::temp_dir().join(format!("cs2-audio-callback-{}-{stamp}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let mut player = HeadlessPlayer::open_with_save_root(&directory, directory.join("profile")).unwrap();
        let mut code = Vec::new();
        for (opcode, values) in [(0x25, vec![0]), (0x2b, vec![71, 4]),
            (0x25, vec![6]), (0x25, vec![1]), (0x2b, vec![157, 8])]
        {
            code.push(opcode);
            for value in values {
                let operand = Operand::immediate(value);
                code.push(operand.flags.bits());
                code.extend_from_slice(&operand.payload.to_le_bytes());
            }
        }
        code.push(0x3b);
        let vm = Vm::from_program(KcsProgram::from_code(&code, vec![0; 256], 64).unwrap()).unwrap();
        player.data_services.tasks.set_factory_external(vm.external_binding());
        player.data_services.tasks.register_external_task(player.current_task_id, 0,
            &player.data_services.language.languages).unwrap();
        player.vm = Some(vm);
        assert!(player.advance_host_frame_with_retries(1000).unwrap());
        assert!(player.vm.as_ref().unwrap().waiting_for_message());
        let root = player.current_task_id;
        let sibling = player.with_host(|host| Ok(host.create_task_object(0, root, b"sibling", b"")?)).unwrap();
        let (completion, gate) = std::sync::mpsc::channel();
        let AudioStart::Pending(ticket) = player.audio_host.begin_test_pending_start(AudioRequest {
            channel: AudioChannel::SoundEffect, resource: "callback.ogg".into(),
            volume: 1.0, pan: 0.0, looped: false,
        }, gate).unwrap() else { panic!("test driver must remain pending"); };
        player.sound_channels.insert(6, SoundChannelState { resource: Some("callback.ogg".into()),
            pending_start: Some(sound_async::PendingSoundStart { started: std::time::Instant::now(), owner: root, index: 157,
                value: 1, configured: Some(AudioChannel::SoundEffect), transition: None,
                operation: sound_async::Operation::Start(ticket) }), ..Default::default() });
        assert!(!player.with_host(|host| Ok(host.send_fes_task_message(root, [2, 0, 0, 0])?)).unwrap());
        (directory, player, sibling, completion)
    }

    fn exercise_callback_wait(mut player: HeadlessPlayer, sibling: u32,
        completion: std::sync::mpsc::Sender<cs2_runtime::RuntimeResult<()>>, configuration: bool)
    {
        if configuration { player.frame_phase = PlayerFramePhase::Configuration; }
        let before = player.state.clock.frame;
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
        let mut completed = 0;
        let mut input_queued = false;
        while std::time::Instant::now() < deadline {
            assert!(player.advance_host_frame_with_retries(1000).unwrap(),
                "audio callback must leave room for complete animation frames");
            assert!(player.data_services.tasks.pending_root_dispatch().is_some());
            assert!(player.sound_channels[&6].handle.is_none());
            if player.audio_input_deferred {
                if !input_queued {
                    let sampled = player.data_services.messages.physical_buttons.clone();
                    player.apply_host_input(cs2_runtime::InputEvent::KeyDown(cs2_runtime::InputKey::Keyboard(13))).unwrap();
                    player.apply_host_input(cs2_runtime::InputEvent::KeyUp(cs2_runtime::InputKey::Keyboard(13))).unwrap();
                    assert_eq!(player.data_services.messages.physical_buttons, sampled);
                    input_queued = true;
                }
                assert_eq!(player.data_services.pending_host_input.len(), 2);
                assert!(!player.state.input.key_pressed(13));
            }
            completed += 1;
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(completed > 5);
        assert!(player.state.clock.frame >= before + 5);
        let sibling_now = player.with_host(|host| Ok(host.data_services.tasks.task_clock(sibling).unwrap().current)).unwrap();
        assert_eq!(u64::from(sibling_now), player.state.clock.elapsed_ticks);
        completion.send(Ok(())).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        while (player.data_services.tasks.pending_root_dispatch().is_some()
            || player.audio_input_deferred || player.audio_configuration_deferred)
            && std::time::Instant::now() < deadline
        {
            player.advance_host_frame_with_retries(1000).unwrap();
            std::thread::yield_now();
        }
        assert!(player.data_services.tasks.pending_root_dispatch().is_none());
        assert!(player.sound_channels[&6].handle.is_some());
        assert!(!player.audio_input_deferred);
        assert!(!player.audio_configuration_deferred);
        assert!(input_queued);
        assert_eq!(player.data_services.pending_host_input.len(), 2,
            "the completing callback must finish before later input is sampled");
        player.advance_host_frame_with_retries(1000).unwrap();
        assert!(player.data_services.pending_host_input.is_empty());
    }

    #[test]
    fn async_input_root_callback_keeps_sibling_frames_advancing_for_500ms() {
        let (directory, player, sibling, completion) = callback_player();
        exercise_callback_wait(player, sibling, completion, false);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn async_configuration_root_callback_keeps_sibling_frames_advancing_for_500ms() {
        let (directory, player, sibling, completion) = callback_player();
        exercise_callback_wait(player, sibling, completion, true);
        std::fs::remove_dir_all(directory).unwrap();
    }
}

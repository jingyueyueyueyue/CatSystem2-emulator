//! Borrow the same desktop services for platform-originated task commands.
//! This does not borrow/replace the root VM or synthesize script instructions.
use anyhow::Result;

use super::{HeadlessPlayer, LoggingHost};

impl HeadlessPlayer {
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

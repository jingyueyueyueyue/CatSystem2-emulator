//! Kind 13 commands over the same physical channels as ordinary/FES audio.
//! Implemented from docs/audio.md contracts; no engine code or host pointers.
use std::collections::{BTreeMap, VecDeque};

use cs2_runtime::{TaskCommand, TaskCommandBuffers, TaskMessage};
use cs2_script::{NativeReturn, Result, Vm};

use super::{LoggingHost, SoundGroup, audio_script_error, read_u32, trim_nul};

#[path = "sound_context_profile.rs"]
mod creation_context_profile;
#[path = "sound_task_state.rs"]
pub(crate) mod checkpoint;

const MAX_PENDING: usize = 1024;
const MAX_LOOPS: usize = 65_536;

#[derive(Default)]
pub(super) struct SoundTaskStore {
    tasks: BTreeMap<u32, SoundTask>,
    ever_created: bool,
    // The Top constructor owns this number independently of subsequent
    // script-created sound tasks. Keep it after release; queries never create
    // a replacement service or silently discard its loop table.
    root_task: Option<u32>,
    // Immutable source-derived ABI identity, retained when restoring task state.
    root_query_code: Option<u32>,
    pub(super) speech: super::sound_speech::SpeechConfigurationService,
}

#[derive(cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct SoundTask {
    enabled: bool,
    stereo: bool,
    groups: Vec<SoundGroup>,
    pending: VecDeque<PendingMessage>,
    loops: BTreeMap<Vec<u8>, [i64; 2]>,
    group_reserved: BTreeMap<u32, u32>,
    update_serial: u64,
}

#[derive(Clone, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct PendingMessage {
    packet: TaskMessage,
    input: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
enum DelayedControl {
    Release,
    Pause,
    Stop,
}

#[derive(Debug, Clone, PartialEq, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct ChannelTransition {
    logical: u32,
    old: u32,
    clock: u32,
    remaining: i64,
    completed_update: Option<u64>,
    new_volume: f32,
    old_handle: Option<u64>,
    old_resource_id: u64,
    new_start: f32,
}

#[derive(Debug, Clone, PartialEq, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(super) struct SoundTaskChannel {
    pub(super) owner: Option<u32>,
    pub(super) requested_volume: f32,
    pub(super) started_tick: u32,
    pub(super) grouped: bool,
    pub(super) release_priority: bool,
    pub(super) control_priority: bool,
    pub(super) last_tick: u32,
    pub(super) elapsed: u32,
    pub(super) distance_bits: [u32; 2],
    pub(super) position_bits: [u32; 3],
    pub(super) velocity_bits: [u32; 3],
    pub(super) doppler_bits: u32,
    pub(super) signal_enabled: bool,
    #[serde(with = "crate::audio_control::checkpoint::signal")]
    pub(super) signal: Option<std::sync::Arc<crate::audio_control::AudioSignal>>,
    pub(super) loop_range: [i64; 2],
    pub(super) whole_buffer: bool,
    pub(super) linked: Option<u32>,
    pub(super) load_mode: i32,
    pub(super) load_duration: u32,
    pub(super) alternate_origin: Option<u32>,
    pub(super) resource_id: u64,
    pub(super) sidecars: super::sound_sidecar::SoundSidecars,
    pub(super) source_queries: super::sound_queries::SourceQueries,
    pub(super) is_speech: bool,
    release_when_finished: bool,
    transition: Option<ChannelTransition>,
    delayed: Option<(DelayedControl, u32)>,
    #[serde(skip)]
    pub(super) delayed_finish: Option<bool>,
}

impl Default for SoundTaskChannel {
    fn default() -> Self {
        Self {
            owner: None,
            requested_volume: 100.,
            started_tick: u32::MAX,
            grouped: false,
            release_priority: false,
            control_priority: false,
            last_tick: 0,
            elapsed: 0,
            distance_bits: [100_f32.to_bits(); 2],
            position_bits: [0; 3],
            velocity_bits: [0; 3],
            doppler_bits: 1_f32.to_bits(),
            signal_enabled: false,
            signal: None,
            loop_range: [0, -1],
            whole_buffer: false,
            linked: None,
            load_mode: 0,
            load_duration: 0,
            alternate_origin: None,
            resource_id: 0,
            sidecars: super::sound_sidecar::SoundSidecars::default(),
            source_queries: super::sound_queries::SourceQueries::default(),
            is_speech: false,
            release_when_finished: false,
            transition: None,
            delayed: None,
            delayed_finish: None,
        }
    }
}

impl SoundTaskChannel {
    pub(super) fn cancel_pending_control(&mut self) {
        self.delayed_finish = None;
        self.delayed = None;
        self.release_priority = false;
        self.control_priority = false;
        self.transition = None;
    }
}

impl LoggingHost<'_> {
    /// Raw Top query numbers differ from our canonical typed command35.
    /// Only independently identified images may bootstrap this service.
    fn discover_root_sound_query_code(&self) -> Option<u32> {
        let sha = self.data_services.native_profile.map(|profile| profile.pe_sha256)
            .or(self.data_services.css_task_pe_sha256)
            .or(self.data_services.particle_message_pe_sha256);
        super::sound_loop_table::legacy_sound_loop_root_query(sha).or_else(|| match sha {
            Some("659d5f069f3c97f69a61e12449cf1510b45e48a10649b9ac6977c2b93d6d28ad") => Some(31),
            Some("5869b7d03a636c200f5042f0c3fc37878a362615bf724f58d47aec5eb8b2d44c") => Some(35),
            _ if self.data_services.native_profile.is_some() => Some(35),
            _ => None,
        })
    }

    fn root_sound_query_code(&self) -> Option<u32> {
        self.data_services.sounds.root_query_code
    }

    pub(super) fn initialize_root_sound_service(&mut self) -> Result<()> {
        if self.data_services.sounds.root_task.is_some() {
            return Ok(());
        }
        let mut query = self.discover_root_sound_query_code();
        if query.is_none() && self.verified_utate_sound_loop_source()? {
            query = Some(32);
        }
        if query.is_none() {
            query = self.rebellions_sound_loop_query()?;
        }
        self.data_services.sounds.root_query_code = query;
        if query.is_none()
            && !self.verified_kanmi_sound_loop_source()?
            && !self.verified_sudden_sound_loop_source()?
        {
            return Ok(());
        }
        if self.data_services.sounds.ever_created {
            return Err(audio_script_error("root sound service must be initialized before script sound tasks"));
        }
        self.ensure_task_record(0)?;
        // PE Top constructors create type13 "SOUND TASK" before entering KCS:
        // V401 b9e4c..b9eb3, Newton a7157..a71b3, NEKO 1ddbfd..1ddc59,
        // Campanella 17f98..17ff2, Utate 1765b..176b5, Kamikaze 1893e,
        // Djibril5/Sudden's companion 189ee, Grisaia 1884e, and Tsunaban 17d58
        // (all RVAs). Packed sources have a portable SOUND owner but do not
        // publish a raw Top query number for an unverified image.
        // The normal factory inserts parent/index/clock/store ownership, so
        // its existing tree Body and Release phases also own this service.
        let id = self.create_task_object(13, 0, b"SOUND TASK", &[])?;
        self.data_services.sounds.root_task = Some(id);
        Ok(())
    }

    /// Internal wrappers use canonical35, just as loop records use canonical199.
    pub(super) fn receive_root_sound_query(
        &mut self, command: TaskCommand, output: &mut u32,
    ) -> Result<Option<cs2_runtime::TaskCommandOutcome>> {
        if command.target != 0 || command.code != 35 {
            return Ok(None);
        }
        let Some(id) = self.data_services.sounds.root_task else { return Ok(None); };
        if !self.data_services.tasks.message_task_state(0).is_some_and(|(_, deletion, _)| deletion != 0) {
            *output = id;
        }
        Ok(Some(cs2_runtime::TaskCommandOutcome { done: true, propagate: false }))
    }

    pub(super) fn receive_root_sound_message_query(
        &mut self, target: u32, packet: TaskMessage, output: &mut u32,
    ) -> Result<bool> {
        if target != 0 || self.root_sound_query_code() != Some(packet[1]) {
            return Ok(false);
        }
        Ok(self.receive_root_sound_query(TaskCommand { target: 0,
            sender: packet[0] as i32, code: 35, data: [packet[2], packet[3], 0] }, output)?.is_some())
    }

    pub(super) fn query_root_sound_task(&mut self) -> Result<Option<u32>> {
        let mut output = 0;
        let outcome = self.route_task_command_buffers(TaskCommand {
            target: 0, sender: self.current_task_id as i32, code: 35, data: [0; 3],
        }, &mut super::native_tasks::OwnedTaskBuffers::new(None, None)?, &mut output, 4096)?;
        match outcome {
            Some(result) if result.done && !result.propagate => {
                if output == 0 { return Ok(None); }
                if !self.task_is_reachable(output) || !self.data_services.tasks.message_task_state(output)
                    .is_some_and(|(kind, _, _)| kind == 13) {
                    return Err(audio_script_error("root35 returned a task without a reachable kind13 sound receiver"));
                }
                Ok(Some(output))
            }
            Some(result) if !result.done => Err(audio_script_error("root sound query unexpectedly suspended")),
            _ => self.default_sound_task(),
        }
    }

    pub(super) fn register_sound_task(&mut self, id: u32, context: &[u8]) -> Result<()> {
        // Native66 submits inherited caller bytes through command26 after
        // construction. In the identified sound classes this reaches both
        // default receivers without reading the context, so it is not an
        // audio configuration packet. The generic task record owns its bytes.
        let sha = self.data_services.native_profile.map_or("", |profile| profile.pe_sha256);
        if !context.is_empty() && !creation_context_profile::ignores_creation_context(sha) {
            return Err(audio_script_error(
                "sound creation context requires an identified receiver contract",
            ));
        }
        self.ensure_legacy_sound_settings()?;
        let stereo = self.data_services.legacy_sound.stereo;
        self.data_services.sounds.ever_created = true;
        self.data_services
            .sounds
            .tasks
            .entry(id)
            .or_insert_with(|| SoundTask {
                enabled: true,
                stereo,
                groups: self.sound_groups.to_vec(),
                pending: VecDeque::new(),
                loops: BTreeMap::new(),
                group_reserved: BTreeMap::new(),
                update_serial: 0,
            });
        Ok(())
    }

    /// Configuration bridge for the task factory/platform. A missing output
    /// device is still an error from AudioHost::play, never a disabled success.
    pub(super) fn set_sound_task_options(
        &mut self,
        id: u32,
        enabled: bool,
        stereo: bool,
        jtalk: bool,
    ) -> Result<()> {
        self.register_sound_task(id, &[])?;
        let task = self.data_services.sounds.tasks.get_mut(&id).unwrap();
        task.enabled = enabled;
        task.stereo = stereo;
        // This is an engine-wide switch in the PE, not a frozen per-task
        // flag. FES use_jtalk writes this same live field.
        self.data_services.legacy_sound.jtalk = jtalk;
        if jtalk {
            self.initialize_sound_speech()?;
        }
        Ok(())
    }

    pub(super) fn refresh_sound_task_options(&mut self) {
        let stereo = self.data_services.legacy_sound.stereo;
        for task in self.data_services.sounds.tasks.values_mut() {
            task.stereo = stereo;
        }
    }

    pub(super) fn single_sound_task_owner(&self) -> Option<u32> {
        if let Some(id) = self.data_services.sounds.root_task {
            return self.data_services.sounds.tasks.contains_key(&id).then_some(id);
        }
        let mut ids = self.data_services.sounds.tasks.keys().copied();
        let first = ids.next()?;
        ids.next().is_none().then_some(first)
    }

    pub(super) fn default_sound_task(&self) -> Result<Option<u32>> {
        if let Some(id) = self.data_services.sounds.root_task {
            return if self.data_services.sounds.tasks.contains_key(&id) {
                Ok(Some(id))
            } else {
                Err(audio_script_error("the desktop root sound service has been released"))
            };
        }
        let mut ids = self.data_services.sounds.tasks.keys().copied();
        let first = ids.next();
        if ids.next().is_some() {
            return Err(audio_script_error(
                "multiple sound tasks require an explicitly addressed sound command",
            ));
        }
        if first.is_none() && self.data_services.sounds.ever_created {
            return Err(audio_script_error(
                "the desktop sound task has been released",
            ));
        }
        Ok(first)
    }

    /// C's explicit temporary-buffer entry, also usable without a caller VM.
    /// true consumes the sound-specific command; false leaves base commands
    /// to the task dispatcher. The ordinary output and data1 stay independent.
    pub(super) fn dispatch_sound_task_command(
        &mut self,
        id: u32,
        command: TaskCommand,
        buffers: &mut TaskCommandBuffers<'_>,
        output: &mut u32,
    ) -> Result<bool> {
        let Some((kind, deletion, _)) = self.data_services.tasks.message_task_state(id) else {
            return Ok(false);
        };
        if kind != 13 {
            return Ok(false);
        }
        if command.target as u32 != id {
            return Err(audio_script_error(
                "sound command target differs from receiver",
            ));
        }
        if deletion != 0 {
            return Ok(true);
        }
        self.register_sound_task(id, &[])?;
        let mut task = self.data_services.sounds.tasks.remove(&id).unwrap();
        let result = if !task.enabled {
            *output = 0;
            Ok(true)
        } else if sound_specific(command.code) {
            self.receive_sound_command(id, &mut task, command, buffers.data1.as_deref_mut(), output)
                .map(|()| true)
        } else {
            Ok(false)
        };
        self.data_services.sounds.tasks.insert(id, task);
        if let Some(cancelled) = self.data_services.sound_async.queued_cancellations.remove(&id) {
            self.discard_cancelled_queued_sound_load(&cancelled);
        }
        result
    }

    /// The temporary-buffer counterpart of native 70. Input is copied now,
    /// before the caller's stack buffer expires; no temporary output survives.
    pub(super) fn queue_sound_task_command(
        &mut self,
        id: u32,
        command: TaskCommand,
        buffers: &TaskCommandBuffers<'_>,
        output: &mut u32,
    ) -> Result<bool> {
        let Some((kind, deletion, _)) = self.data_services.tasks.message_task_state(id) else {
            return Ok(false);
        };
        if kind != 13 {
            return Ok(false);
        }
        if deletion != 0 {
            return Ok(true);
        }
        if command.target as u32 != id {
            return Err(audio_script_error(
                "sound queue target differs from receiver",
            ));
        }
        self.register_sound_task(id, &[])?;
        let task = self.data_services.sounds.tasks.get_mut(&id).unwrap();
        if !task.enabled {
            *output = 0;
            return Ok(true);
        }
        if !sound_specific(command.code) {
            return Ok(false);
        }
        if matches!(command.code, 198 | 214) {
            return Err(audio_script_error(
                "queued sound198/214 requires synchronous temporary output",
            ));
        }
        if task.pending.len() >= MAX_PENDING {
            return Err(audio_script_error("kind 13 sound message queue is full"));
        }
        let input = if let Some(bytes) = buffers.data1.as_deref() {
            let limit = match command.code {
                228 => super::sound_speech::MAX_RECORD,
                225..=227 => super::sound_speech_load::MAX_RECORD,
                _ => 292,
            };
            if bytes.len() > limit {
                return Err(audio_script_error(
                    "queued sound temporary record exceeds its command limit",
                ));
            }
            Some(bytes.to_vec())
        } else {
            None
        };
        task.pending.push_back(PendingMessage {
            packet: [
                command.sender as u32,
                command.code,
                command.data[0],
                command.data[1],
            ],
            input,
        });
        *output = 1;
        Ok(true)
    }

    /// Native 69 borrows its caller VM only during dispatch. Native 70 copies
    /// input bytes into a bounded owned queue; no VM address survives a frame.
    pub(super) fn dispatch_sound_task_message(
        &mut self,
        id: u32,
        packet: TaskMessage,
        queued: bool,
        output: &mut u32,
        mut vm: Option<&mut Vm>,
    ) -> Result<bool> {
        let Some((kind, deletion, _)) = self.data_services.tasks.message_task_state(id) else {
            return Ok(false);
        };
        if kind != 13 {
            return Ok(false);
        }
        if deletion != 0 {
            return Ok(true);
        }
        self.register_sound_task(id, &[])?;
        if !self.data_services.sounds.tasks[&id].enabled {
            *output = 0;
            return Ok(true);
        }
        if !sound_specific(packet[1]) {
            return Ok(false);
        }
        if queued && matches!(packet[1], 198 | 214) {
            return Err(audio_script_error(
                "queued sound198/214 cannot retain caller VM output; use synchronous69",
            ));
        }
        let mut input = self.snapshot_sound_input(id, packet, vm.as_deref())?;
        if queued {
            let task = self.data_services.sounds.tasks.get_mut(&id).unwrap();
            if task.pending.len() >= MAX_PENDING {
                return Err(audio_script_error("kind 13 sound message queue is full"));
            }
            task.pending.push_back(PendingMessage { packet, input });
            *output = 1;
            return Ok(true);
        }
        let command = message_command(id, packet);
        let mut buffers = TaskCommandBuffers {
            data0: None,
            data1: input.as_deref_mut(),
        };
        let handled = self.dispatch_sound_task_command(id, command, &mut buffers, output)?;
        if packet[1] == 198 {
            // Bounds were checked before dispatch. Copy only the DWORD result.
            let bytes = input.as_deref().ok_or_else(|| {
                audio_script_error("sound 198 requires a writable VM output buffer")
            })?;
            vm.as_deref_mut()
                .ok_or_else(|| audio_script_error("sound 198 requires a caller VM"))?
                .write(packet[3], &bytes[..4])?;
        }
        if packet[1] == 214
            && let Some(bytes) = input.as_deref()
        {
            let destination = read_u32(bytes, 0)?;
            if destination != 0 && bytes.len() > 8 {
                vm.as_deref_mut()
                    .ok_or_else(|| audio_script_error("sound214 requires caller VM output"))?
                    .write(destination, &bytes[8..])?;
            }
        }
        Ok(handled)
    }

    fn snapshot_sound_input(
        &self,
        id: u32,
        packet: TaskMessage,
        vm: Option<&Vm>,
    ) -> Result<Option<Vec<u8>>> {
        let [_, code, channel, address] = packet;
        if matches!(code, 225..=227) {
            if !self.data_services.legacy_sound.jtalk || (code == 225 && channel >= 64) {
                return Ok(None);
            }
            if address == 0 {
                return Err(audio_script_error("speech load requires a record address"));
            }
            if code != 225 {
                let vm = vm.ok_or_else(|| audio_script_error("speech group load requires a VM"))?;
                let prefix = vm.read(address, if code == 226 { 8 } else { 12 })?;
                let key = read_u32(&prefix, if code == 226 { 4 } else { 8 })?;
                if sound_group(&self.data_services.sounds.tasks[&id], code == 226, key).is_none() {
                    return Ok(Some(prefix));
                }
            }
            return super::sound_speech_load::snapshot_speech_record(
                vm.ok_or_else(|| audio_script_error("speech load requires its caller VM"))?,
                address,
            ).map(Some);
        }
        if code == 228 {
            if channel == 0 || address == 0 {
                return Ok(None);
            }
            return super::sound_speech::snapshot_configuration(
                vm.ok_or_else(|| audio_script_error("sound228 requires caller-VM strings"))?,
                channel,
                address,
            ).map(Some);
        }
        if code == 180 && channel >= 64 {
            return Ok(None);
        }
        if code == 215 && channel != u32::MAX && channel >= 64 {
            return Ok(None);
        }
        if code == 214 {
            if channel >= 64 || address == 0 {
                return Ok(None);
            }
            let vm =
                vm.ok_or_else(|| audio_script_error("sound214 record requires a caller VM"))?;
            let mut record = vm.read(address, 8)?;
            let destination = read_u32(&record, 0)?;
            if destination != 0 {
                let capacity = self.sound_signal_capacity(channel, read_u32(&record, 4)?)?;
                let length = capacity * 4;
                // Validate the destination before querying/publishing results.
                if length != 0 {
                    vm.read(destination, length)?;
                }
                record.resize(8 + length, 0);
            }
            return Ok(Some(record));
        }
        if code == 204
            && self.data_services.sounds.tasks[&id]
                .groups
                .iter()
                .any(|group| group.key == channel)
        {
            return Ok(None);
        }
        let length = match code {
            180 | 181 | 206..=209 => 288,
            193 => 8,
            194 | 196 => 12,
            197 | 198 => 4,
            199 => 48,
            204 => 16,
            210 | 215 | 216 => {
                if address == 0 {
                    return Err(audio_script_error(
                        "sound text query requires a name buffer",
                    ));
                }
                let vm = vm.ok_or_else(|| {
                    audio_script_error(
                        "sound input VM is absent; dispatch with explicit TaskCommandBuffers",
                    )
                })?;
                let mut bytes = Vec::new();
                for offset in 0..260_u32 {
                    let byte = vm.read(
                        address.checked_add(offset).ok_or_else(|| {
                            audio_script_error("sound text-query address overflow")
                        })?,
                        1,
                    )?[0];
                    bytes.push(byte);
                    if byte == 0 {
                        return Ok(Some(bytes));
                    }
                }
                return Err(audio_script_error(
                    "sound text query name lacks a bounded NUL terminator",
                ));
            }
            _ => return Ok(None),
        };
        if address == 0 {
            return Ok(None);
        }
        if matches!(code, 193 | 194 | 196 | 197) && channel >= 64 {
            return Ok(None);
        }
        let vm = vm.ok_or_else(|| {
            audio_script_error(
                "sound input VM is absent; dispatch with explicit TaskCommandBuffers",
            )
        })?;
        if code == 181 {
            let prefix = vm.read(address, 4)?;
            if read_u32(&prefix, 0)? as i32 >= 64 {
                return Ok(Some(prefix));
            }
        } else if matches!(code, 206..=209) {
            let prefix = vm.read(address, if code <= 207 { 8 } else { 12 })?;
            let key = read_u32(&prefix, if code <= 207 { 4 } else { 8 })?;
            if sound_group(&self.data_services.sounds.tasks[&id], code <= 207, key).is_none() {
                return Ok(Some(prefix));
            }
        }
        Ok(Some(vm.read(address, length)?))
    }

    fn receive_sound_command(
        &mut self,
        id: u32,
        task: &mut SoundTask,
        command: TaskCommand,
        mut data1: Option<&mut [u8]>,
        output: &mut u32,
    ) -> Result<()> {
        let code = command.code;
        let [logical, value, _] = command.data;
        let channel = if matches!(code, 183 | 187..=194 | 196..=198 | 201..=202 | 211..=214 | 219) {
            self.sound_selected_channel(logical)
        } else {
            logical
        };
        if channel < 64 && matches!(code, 182..=194 | 196..=198 | 201 | 211..=213 | 218 | 219) {
            self.sound_channels
                .entry(channel)
                .or_default()
                .task
                .owner
                .get_or_insert(id);
        }
        match code {
            19 => {
                if !self.data_services.legacy_sound.jtalk {
                    // PE task+fc is set by speech225..227, not ordinary grouped
                    // loads. Documented query-index behavior: speech active
                    // first members affect physical zero, not their own index.
                    let speech: Vec<_> = self
                        .sound_channels
                        .iter()
                        .filter_map(|(&channel, state)| {
                            (state.task.owner == Some(id) && state.task.is_speech).then_some(channel)
                        })
                        .collect();
                    for channel in speech {
                        let linked = self
                            .sound_channels
                            .get(&channel)
                            .and_then(|state| state.task.linked);
                        for (index, physical) in std::iter::once(channel).chain(linked).enumerate()
                        {
                            if self.sound_playing(physical)? != 0 {
                                let state = self.sound_channels.entry(index as u32).or_default();
                                state.task.release_priority = true;
                                if let Some(fade) = state.volume_fade.as_mut() {
                                    fade.remaining = 0;
                                }
                            }
                        }
                    }
                }
                *output = 1;
            }
            180 | 181 | 206..=209 => {
                // A superseded invocation owns only its cancellation receipt.
                // Consume it before group selection can reclaim another
                // caller's newly published device or enqueue another stop.
                if self.data_services.sound_async.cancelled.remove(&self.current_task_id) {
                    *output = if code >= 206 { u32::MAX } else { 0 };
                    return Ok(());
                }
                if code == 180 && channel >= 64 {
                    return Ok(());
                }
                let saved_load = self.data_services.sound_async.task_loads.get(&self.current_task_id)
                    .filter(|saved| saved.task == id && saved.code == code).cloned();
                let saved_record = saved_load.as_ref().map(|saved| saved.record.clone());
                let bytes = saved_record.as_deref().or(data1.as_deref()).ok_or_else(|| {
                    audio_script_error("sound load requires an explicit record buffer")
                })?;
                let grouped = code >= 206;
                if saved_load.is_none() && self.data_services.fast_forward
                    && let Some(field) = bytes.get(12..272)
                    && let Some(end) = field.iter().position(|&byte| byte == 0)
                    && !self.sound_is_bgm(&super::sound_native::decode_name(self.text_encoding, &field[..end])?)
                {
                    *output = if grouped { u32::MAX } else { 0 };
                    return Ok(());
                }
                let logical = if let Some(saved) = &saved_load { saved.logical } else if grouped {
                    let key = read_u32(bytes, if code <= 207 { 4 } else { 8 })?;
                    let Some(group) = sound_group(task, code <= 207, key) else {
                        *output = u32::MAX;
                        return Ok(());
                    };
                    let Some(selected) = self.select_task_sound_group_channel(group)? else {
                        *output = u32::MAX;
                        return Ok(());
                    };
                    selected
                } else {
                    let target = read_u32(bytes, 0)?;
                    if code == 181 && target as i32 >= 64 {
                        return Ok(());
                    }
                    target
                };
                if bytes.len() < 288 {
                    return Err(audio_script_error(
                        "sound load record is shorter than 288 bytes",
                    ));
                }
                let mode = read_u32(bytes, 280)?;
                let target = if let Some(saved) = &saved_load { saved.target } else if mode == 0 {
                    logical
                } else {
                    logical.wrapping_add(32)
                };
                if target >= 64 || (!grouped && code == 180 && logical >= 64) {
                    return Err(audio_script_error(
                        "sound load primary/alternate physical channel is outside 0..63",
                    ));
                }
                let name_field = &bytes[12..272];
                let end = name_field
                    .iter()
                    .position(|&byte| byte == 0)
                    .ok_or_else(|| {
                        audio_script_error("sound load filename lacks a NUL terminator")
                    })?;
                let name = &name_field[..end];
                let whole = matches!(code, 181 | 207 | 209);
                let previous_range = self
                    .sound_channels
                    .get(&target)
                    .map_or([0, -1], |state| state.task.loop_range);
                let loop_range = if let Some(saved) = &saved_load { saved.loop_range } else if code == 209 {
                    previous_range
                } else if !whole && read_u32(bytes, 272)? != 0 {
                    task.loops
                        .get(&folded_name(name))
                        .copied()
                        .unwrap_or([0, -1])
                } else {
                    [0, -1]
                };
                // Busy grouped allocation can first reclaim the logical
                // slot, even when this record loads its alternate physical slot.
                // Keep that exact allocation and record until its stop completes.
                let reclaiming = self.sound_channels.values().any(|state| state.pending_start.as_ref().is_some_and(|pending|
                    pending.owner == self.current_task_id && matches!(pending.operation, crate::sound_async::Operation::Release(_))));
                if reclaiming {
                    let reclaim_state = saved_load.as_ref().and_then(|saved| saved.reclaim_state)
                        .or_else(|| self.sound_channels.get(&logical).map(|state|
                            (state.task.last_tick, state.task.loop_range[1], state.task.linked)));
                    self.data_services.sound_async.task_loads.entry(self.current_task_id).or_insert_with(|| crate::sound_native::PendingTaskSoundLoad {
                        task: id, code, record: bytes.to_vec(), logical, target, loop_range,
                        queued: self.data_services.sound_async.queued_owner == Some(id),
                        reclaim_state,
                    });
                    if self.poll_task_audio_start(self.current_task_id)? == Some(NativeReturn::Retry) { return Ok(()); }
                }
                if let Some((last_tick, loop_end, linked)) = self.data_services.sound_async.task_loads
                    .get(&self.current_task_id).and_then(|saved| saved.reclaim_state)
                    && let Some(state) = self.sound_channels.get_mut(&logical)
                    && state.pending_start.is_none()
                {
                    // The synchronous selector restored these after release.
                    // Alternate loads retain the old logical slot's metadata.
                    state.task.last_tick = last_tick;
                    state.task.loop_range[1] = loop_end;
                    state.task.linked = linked;
                }
                let result = self.load_sound_resolved_name(
                    target,
                    name,
                    if whole { 2 } else { 1 },
                    read_u32(bytes, 276)?,
                );
                // Ancillary errors may retain already prepared PCM. Keep
                // that partial state owned so task deletion still frees it.
                if let Some(state) = self.sound_channels.get_mut(&target) {
                    state.task.owner = Some(id);
                }
                let result = result?;
                if result == NativeReturn::Retry {
                    self.data_services.sound_async.task_loads.entry(self.current_task_id).or_insert_with(|| crate::sound_native::PendingTaskSoundLoad {
                        task: id, code, record: bytes.to_vec(), logical, target, loop_range,
                        queued: self.data_services.sound_async.queued_owner == Some(id),
                        reclaim_state: None,
                    });
                    return Ok(());
                }
                self.data_services.sound_async.task_loads.remove(&self.current_task_id);
                let loaded = matches!(result, NativeReturn::Value(1));
                if let Some(state) = self.sound_channels.get_mut(&target) {
                    state.task.owner = Some(id);
                    state.task.grouped = grouped && loaded;
                    state.task.started_tick = u32::MAX;
                    state.task.elapsed = 0;
                    state.task.loop_range = loop_range;
                    state.task.whole_buffer = whole
                        || state.prepared_audio.as_ref().is_some_and(|audio| {
                            audio.probe.frames
                                + u64::from(audio.probe.sample_rate)
                                    * u64::from(state.duration_frames)
                                    / 1000
                                <= u64::from(audio.probe.sample_rate) * 4
                        });
                    state.task.load_mode = mode as i32;
                    state.task.load_duration = read_u32(bytes, 284)?;
                    state.task.alternate_origin = (mode != 0).then_some(logical);
                    if mode != 0 {
                        state.task.linked = Some(target);
                    }
                }
                if mode != 0 && !grouped && code == 180 {
                    let state = self.sound_channels.entry(logical).or_default();
                    state.task.owner = Some(id);
                    state.task.linked = Some(target);
                }
                *output = if grouped {
                    if loaded { target } else { u32::MAX }
                } else {
                    u32::from(loaded)
                };
            }
            182 | 184 | 185 if channel < 64 => {
                let linked = self
                    .sound_channels
                    .get(&channel)
                    .and_then(|state| state.task.linked);
                self.delay_or_control_sound(
                    channel,
                    match code {
                        182 => DelayedControl::Release,
                        184 => DelayedControl::Pause,
                        _ => DelayedControl::Stop,
                    },
                )?;
                if let Some(linked) = linked.filter(|linked| *linked != channel) {
                    self.delay_or_control_sound(
                        linked,
                        match code {
                            182 => DelayedControl::Release,
                            184 => DelayedControl::Pause,
                            _ => DelayedControl::Stop,
                        },
                    )?;
                }
            }
            183 if channel < 64 => {
                self.stamp_sound_start(channel);
                if let Some(state) = self.sound_channels.get_mut(&channel) {
                    state.task.delayed = None;
                    state.task.release_priority = false;
                    state.task.control_priority = false;
                    // The original target is f32, with x86 truncation at start.
                    state.volume = x86_i32(state.task.requested_volume) as u32;
                }
                if self
                    .sound_channels
                    .get(&channel)
                    .is_some_and(|state| state.resource.is_some())
                {
                    let result = self.control_sound_channel_values(157, channel, value, 0)?;
                    if result == NativeReturn::Retry {
                        if let Some(pending) = self.sound_channels.get_mut(&channel).and_then(|state| state.pending_start.as_mut()) {
                            pending.transition = Some([id, logical, channel]);
                        }
                    } else { self.begin_sound_transition(id, logical, channel)?; }
                }
            }
            186 => {
                let linked = self
                    .sound_channels
                    .get(&channel)
                    .and_then(|state| state.task.linked);
                let playing = self.sound_playing(channel)?;
                *output = if let Some(linked) = linked.filter(|linked| *linked != channel) {
                    u32::from(playing != 0 || self.sound_playing(linked)? != 0)
                } else {
                    playing
                };
            }
            187 => {
                self.control_sound_channel_values(161, channel, value, 0)?;
                if let Some(transition) = self
                    .sound_channels
                    .get_mut(&channel)
                    .and_then(|state| state.task.transition.as_mut())
                {
                    transition.new_volume = value as i32 as f32;
                }
            }
            188 => *output = returned_value(self.control_sound_channel_values(162, channel, 0, 0)?),
            189 | 192 => {
                let target = value as u16;
                let target = if code == 192 {
                    i32::from(target as i16) as u32
                } else {
                    u32::from(target)
                };
                if code == 189
                    && self.retarget_sound_transition(channel, target as f32, value >> 16)?
                {
                    return Ok(());
                }
                self.control_sound_channel_values(
                    if code == 189 { 163 } else { 166 },
                    channel,
                    target,
                    value >> 16,
                )?;
                if let Some(state) = self.sound_channels.get_mut(&channel) {
                    state.fade_clock = self
                        .data_services
                        .tasks
                        .task_clock(id)
                        .map_or(0, |clock| clock.elapsed());
                }
            }
            190 => {
                if task.stereo {
                    self.control_sound_channel_values(164, channel, value, 0)?;
                } else if let Some(state) = self.sound_channels.get_mut(&channel) {
                    state.pan_fade = None;
                }
            }
            191 => *output = returned_value(self.control_sound_channel_values(165, channel, 0, 0)?),
            193 | 194 | 196 | 197 if channel < 64 => {
                if let Some(bytes) = data1.as_deref() {
                    let length = match code {
                        193 => 8,
                        197 => 4,
                        _ => 12,
                    };
                    if bytes.len() < length {
                        return Err(audio_script_error("sound spatial record is truncated"));
                    }
                    if let Some(state) = self.sound_channels.get_mut(&channel)
                        && state.resource.is_some()
                    {
                        match code {
                            193 => state.task.distance_bits[1] = read_u32(bytes, 4)?,
                            194 if task.stereo => {
                                state.task.position_bits = [
                                    read_u32(bytes, 0)?,
                                    read_u32(bytes, 4)?,
                                    read_u32(bytes, 8)?,
                                ];
                                let [x, _, z] = state.task.position_bits.map(f32::from_bits);
                                let angle = f64::from(x).atan2(f64::from(z)) as f32;
                                let pan = x86_i32((f64::from(angle).sin() as f32) * 100.)
                                    .clamp(-100, 100);
                                state.pan = pan;
                                crate::sound_control::apply_live(state)
                                    .map_err(|cause| self.sound_host_error(cause))?;
                            }
                            196 => {
                                state.task.velocity_bits = [
                                    read_u32(bytes, 0)?,
                                    read_u32(bytes, 4)?,
                                    read_u32(bytes, 8)?,
                                ]
                            }
                            197 => state.task.doppler_bits = read_u32(bytes, 0)?,
                            _ => {}
                        }
                    }
                } else if value != 0 {
                    return Err(audio_script_error(
                        "sound spatial pointer requires an explicit buffer",
                    ));
                }
            }
            198 => {
                let result = data1
                    .as_deref_mut()
                    .and_then(|bytes| bytes.get_mut(..4))
                    .ok_or_else(|| {
                        audio_script_error("sound 198 requires four writable output bytes")
                    })?;
                let active = self
                    .sound_channels
                    .get(&channel)
                    .filter(|_| channel < 64)
                    .is_some_and(|state| {
                        state.volume_fade.is_some()
                            || state
                                .task
                                .transition
                                .as_ref()
                                .is_some_and(|transition| transition.completed_update.is_none())
                    });
                result.copy_from_slice(&u32::from(active).to_le_bytes());
            }
            195 => {
                // PE RVA 0x144351 visits all 64 physical channels, stops and
                // releases live resources. The other original helpers clear
                // sidecar signal/tag arrays;
                // they must not discard the real PCM monitor of idle sources.
                let channels: Vec<_> = self
                    .sound_channels
                    .keys()
                    .copied()
                    .filter(|&channel| channel < 64)
                    .collect();
                for channel in channels {
                    if self.sound_playing(channel)? != 0 {
                        self.control_sound_channel_values(156, channel, 0, 0)?;
                    } else if let Some(state) = self.sound_channels.get_mut(&channel) {
                        state.task.sidecars.clear_auxiliary();
                    }
                }
            }
            199 => {
                let Some(bytes) = data1.as_deref() else {
                    return Ok(());
                };
                if bytes.len() < 48 {
                    return Err(audio_script_error(
                        "sound loop record is shorter than 48 bytes",
                    ));
                }
                let range = [
                    i64::from_le_bytes(bytes[32..40].try_into().unwrap()),
                    i64::from_le_bytes(bytes[40..48].try_into().unwrap()),
                ];
                if range[1] != -1 && range[0] >= range[1] {
                    return Ok(());
                }
                let name = trim_nul(&bytes[..32]);
                if name.len() >= 32 {
                    return Err(audio_script_error("sound loop name exceeds 31 bytes"));
                }
                let key = folded_name(name);
                if let Some(previous) = task.loops.get_mut(&key) {
                    previous[0] = range[0];
                } else {
                    if task.loops.len() >= MAX_LOOPS {
                        return Err(audio_script_error("sound loop registration limit exceeded"));
                    }
                    task.loops.insert(key, range);
                }
            }
            201 => *output = self.sound_duration_millis(channel),
            202 if logical < 64 => *output = self.sound_source_completed_passes(logical)?,
            202 => {}
            203 => {
                let _ = self.sound_source_loop_start(logical);
            }
            204 => {
                // Group map insertion is first-wins. Raw record is
                // [category id, first channel, last channel, reserved].
                // PE 0x144760; grouped loads use node+0x18/+0x1c as bounds.
                if task.groups.iter().any(|group| group.key == logical) {
                    return Ok(());
                }
                let bytes = data1.as_deref().ok_or_else(|| {
                    audio_script_error("sound 204 requires a 16-byte group record")
                })?;
                if bytes.len() < 16 {
                    return Err(audio_script_error("sound group record is truncated"));
                }
                if task.groups.len() >= MAX_LOOPS {
                    return Err(audio_script_error(
                        "sound group registration limit exceeded",
                    ));
                }
                task.groups.push(SoundGroup {
                    key: logical,
                    id: read_u32(bytes, 0)?,
                    label: Vec::new(),
                    range: [read_u32(bytes, 4)? as i32, read_u32(bytes, 8)? as i32],
                });
                task.group_reserved.insert(logical, read_u32(bytes, 12)?);
            }
            205 => {
                task.groups.retain(|group| group.key != logical);
                task.group_reserved.remove(&logical);
            }
            210 => {
                let bytes = data1.as_deref().ok_or_else(|| {
                    audio_script_error("sound 210 requires an explicit group-name buffer")
                })?;
                if !bytes.contains(&0) {
                    return Err(audio_script_error(
                        "sound group name lacks a NUL terminator",
                    ));
                }
                let name = trim_nul(bytes);
                *output = if self.sound_config_enabled != 0 {
                    self.sound_groups
                        .iter()
                        .find(|group| group.label == name)
                        .map_or(0, |group| group.id)
                } else {
                    [b"bgm".as_slice(), b"se", b"voice", b"sysvoice"]
                        .iter()
                        .position(|label| name.eq_ignore_ascii_case(label))
                        .map_or(5, |index| index as u32)
                };
            }
            211 if channel < 64 => {
                // Release once device consumption has really finished. This
                // is independent of grouped ten-tick control fades.
                if self.sound_playing(channel)? != 0 {
                    self.sound_channels
                        .entry(channel)
                        .or_default()
                        .task
                        .release_when_finished = true;
                } else {
                    self.control_sound_channel_values(156, channel, 0, 0)?;
                }
            }
            212 if channel < 64 => {
                let state = self.sound_channels.entry(channel).or_default();
                state.task.signal_enabled = value != 0;
                if let Some(signal) = &state.task.signal {
                    signal.enable(value != 0);
                } else if state.handle.is_some() {
                    return Err(audio_script_error(
                        "this audio host has no actual PCM signal monitor",
                    ));
                }
            }
            213 => {
                let now = self.host_millis();
                *output = self
                    .sound_channels
                    .get(&channel)
                    .filter(|_| channel < 64)
                    .map_or(0, |state| {
                        state.task.sidecars.signal.as_ref().map_or_else(
                            || {
                                state
                                    .task
                                    .signal
                                    .as_ref()
                                    .map_or(0, |signal| signal.level())
                            },
                            |signal| signal.level(now, state.task.started_tick),
                        )
                    });
            }
            214 => {
                *output = 0;
                if logical >= 64 {
                    return Ok(());
                }
                let Some(bytes) = data1.as_deref_mut() else {
                    if value != 0 {
                        return Err(audio_script_error(
                            "sound214 requires an explicit8-byte record",
                        ));
                    }
                    return Ok(());
                };
                let destination_present = read_u32(bytes, 0)? != 0;
                let requested = read_u32(bytes, 4)?;
                if !destination_present {
                    *output = self.sound_signal_window(logical, None)?;
                    return Ok(());
                }
                let capacity = self.sound_signal_capacity(logical, requested)?;
                let target = bytes.get_mut(8..8 + capacity * 4).ok_or_else(|| {
                    audio_script_error("sound214 output buffer is shorter than its capacity")
                })?;
                let mut values = vec![0; capacity];
                *output = self.sound_signal_window(logical, Some(&mut values))?;
                for (target, value) in target.chunks_exact_mut(4).zip(values) {
                    target.copy_from_slice(&value.to_le_bytes());
                }
            }
            215 if logical != u32::MAX && logical >= 64 => *output = 0,
            215 | 216 => {
                let Some(bytes) = data1.as_deref() else {
                    if value != 0 {
                        return Err(audio_script_error(
                            "sound tag query requires an explicit name buffer",
                        ));
                    }
                    *output = if code == 216 { u32::MAX } else { 0 };
                    return Ok(());
                };
                if !bytes.contains(&0) {
                    return Err(audio_script_error(
                        "sound tag query name lacks a NUL terminator",
                    ));
                }
                *output = if code == 216 {
                    self.sound_tag_channel(bytes)
                } else {
                    self.sound_tag_value(logical, bytes)
                };
            }
            223 if logical < 64 => {
                *output = u32::from(
                    self.sound_lip_bytes(logical, false)
                        .or_else(|| self.sound_lip_bytes(logical, true))
                        .is_some(),
                );
            }
            223 => {}
            225..=227 => {
                if self.data_services.fast_forward {
                    *output = if code == 225 { 0 } else { u32::MAX };
                    return Ok(());
                }
                if !self.data_services.legacy_sound.jtalk {
                    if code != 225 { *output = u32::MAX; }
                    return Ok(());
                }
                if code == 225 && logical >= 64 { return Ok(()); }
                let bytes = data1.as_deref()
                    .ok_or_else(|| audio_script_error("speech load requires an owned record"))?;
                let grouped = code != 225;
                let origin = if grouped {
                    let key = read_u32(bytes, if code == 226 { 4 } else { 8 })?;
                    let Some(group) = sound_group(task, code == 226, key) else {
                        *output = u32::MAX;
                        return Ok(());
                    };
                    let Some(selected) = self.select_task_sound_group_channel(group)? else {
                        *output = u32::MAX;
                        return Ok(());
                    };
                    selected
                } else { read_u32(bytes, 0)? };
                let mode = read_u32(bytes, 280)?;
                let physical = if mode == 0 { origin } else { origin.wrapping_add(32) };
                if origin >= 64 || physical >= 64 {
                    return Err(audio_script_error("speech primary/alternate channel exceeds0..63"));
                }
                let text = super::sound_speech_load::speech_record_text(bytes)?;
                let loaded = matches!(self.load_sound_speech_pcm(
                    physical, text, read_u32(bytes, 276)?,
                )?, NativeReturn::Value(1));
                if let Some(state) = self.sound_channels.get_mut(&physical) {
                    state.task.owner = Some(id);
                    state.task.grouped = grouped && loaded;
                    state.task.load_mode = mode as i32;
                    state.task.load_duration = read_u32(bytes, 284)?;
                    state.task.alternate_origin = (mode != 0).then_some(origin);
                    if grouped && mode != 0 { state.task.linked = Some(physical); }
                }
                let name = &bytes[12..272];
                let name_end = name.iter().position(|&byte| byte == 0)
                    .ok_or_else(|| audio_script_error("speech metadata name lacks boundedNUL"))?;
                if name_end != 0 {
                    let name = super::sound_native::decode_name(self.text_encoding, &name[..name_end])?;
                    let sidecars = self.load_sound_sidecars(&name)?;
                    if let Some(state) = self.sound_channels.get_mut(&physical) {
                        state.task.sidecars = sidecars;
                    }
                }
                if mode != 0 && !grouped {
                    let state = self.sound_channels.entry(origin).or_default();
                    state.task.owner = Some(id);
                    state.task.linked = Some(physical);
                }
                *output = if grouped { if loaded { physical } else { u32::MAX } }
                    else { u32::from(loaded) };
            }
            228 if logical != 0 && value != 0 => {
                let record = data1.as_deref().ok_or_else(|| {
                    audio_script_error("sound228 requires an owned two-string configuration record")
                })?;
                self.receive_sound_speech_configuration(record)?;
            }
            228 => {}
            218 if channel < 64 => {
                self.stamp_sound_start(channel);
                if let Some(linked) = self
                    .sound_channels
                    .get(&channel)
                    .and_then(|state| state.task.linked)
                {
                    self.stamp_sound_start(linked);
                }
            }
            219 => self.control_sound_play_count(channel, value)?,
            220 => *output = self.sound_elapsed_ticks(logical),
            182..=185 | 193 | 194 | 196 | 197 | 211 | 212 | 218 if channel >= 64 => {}
            _ => return Err(audio_script_error("unsupported kind 13 sound command")),
        }
        Ok(())
    }

    fn sound_playing(&mut self, channel: u32) -> Result<u32> {
        Ok(returned_value(
            self.control_sound_channel_values(160, channel, 0, 0)?,
        ))
    }

    pub(super) fn sound_selected_channel(&self, channel: u32) -> u32 {
        self.sound_channels
            .get(&channel)
            .and_then(|state| state.task.linked)
            .filter(|linked| *linked < 64)
            .unwrap_or(channel)
    }

    /// Command220 reads the task's accumulated ticks, not the PCM position.
    /// PE RVA 0x1455fe returns MAX during release/pause/stop requests.
    pub(super) fn sound_elapsed_ticks(&self, logical: u32) -> u32 {
        if logical >= 64 {
            return u32::MAX;
        }
        let channel = self.sound_selected_channel(logical);
        self.sound_channels.get(&channel).map_or(0, |state| {
            if state.task.delayed.is_some() {
                u32::MAX
            } else {
                state.task.elapsed
            }
        })
    }

    pub(super) fn begin_sound_transition(&mut self, owner: u32, logical: u32, physical: u32) -> Result<()> {
        let Some(state) = self.sound_channels.get(&physical) else {
            return Ok(());
        };
        if state.task.load_mode == 0 {
            return Ok(());
        }
        let span = state.task.load_duration;
        let target_volume = state.task.requested_volume;
        let old = if logical != physical {
            logical
        } else {
            state.task.alternate_origin.unwrap_or(u32::MAX)
        };
        let old_handle = self.sound_channels.get(&old).and_then(|state| state.handle);
        let old_resource_id = self
            .sound_channels
            .get(&old)
            .map_or(0, |state| state.task.resource_id);
        let clock = self
            .data_services
            .tasks
            .task_clock(owner)
            .map_or(0, |clock| clock.elapsed());
        if old != physical
            && let Some(state) = self.sound_channels.get_mut(&old)
        {
            // Original old/new slots have independent fade clocks. A pause,
            // stop or retarget on the new source must not strand the old fade.
            state.volume_fade = Some(crate::audio_control::AudioFade {
                current: state.task.requested_volume,
                target: 0.,
                remaining: i64::from(span),
            });
            state.fade_clock = clock;
            state.task.owner.get_or_insert(owner);
        }
        let state = self.sound_channels.get_mut(&physical).unwrap();
        state.volume = 0;
        state.task.transition = Some(ChannelTransition {
            logical,
            old,
            clock,
            remaining: i64::from(span),
            completed_update: None,
            new_volume: target_volume,
            old_handle,
            old_resource_id,
            new_start: 0.,
        });
        crate::sound_control::apply_live(state).map_err(|cause| self.sound_host_error(cause))
    }

    fn retarget_sound_transition(&mut self, channel: u32, target: f32, span: u32) -> Result<bool> {
        let active = self
            .sound_channels
            .get(&channel)
            .and_then(|state| state.task.transition.as_ref())
            .is_some_and(|transition| transition.completed_update.is_none());
        if !active {
            return Ok(false);
        }
        let state = self.sound_channels.get_mut(&channel).unwrap();
        let current = state.volume as f32;
        let clock = state
            .task
            .owner
            .and_then(|id| self.data_services.tasks.task_clock(id))
            .map_or(self.task_time.wrapping_sub(self.task_origin), |clock| {
                clock.elapsed()
            });
        let transition = state.task.transition.as_mut().unwrap();
        transition.new_start = current;
        transition.new_volume = target;
        // 189 retargets only the associated new channel. Its old source
        // keeps the fade and clock started by 183.
        transition.remaining = i64::from(span);
        transition.clock = clock;
        Ok(true)
    }

    fn advance_sound_transition(
        &mut self,
        physical: u32,
        now: u32,
        update: u64,
    ) -> Result<Option<u32>> {
        let Some(mut transition) = self
            .sound_channels
            .get_mut(&physical)
            .and_then(|state| state.task.transition.take())
        else {
            return Ok(None);
        };
        if transition.completed_update.is_none() {
            transition.remaining -= i64::from(now.wrapping_sub(transition.clock));
            transition.clock = now;
            // Original task keeps intermediate float values. The task tick
            // delta reduces remaining time, but is not a numerator multiplier.
            if transition.remaining <= 0 {
                transition.new_start = transition.new_volume;
            } else {
                transition.new_start +=
                    (transition.new_volume - transition.new_start) / transition.remaining as f32;
            }
        }
        self.sound_channels
            .get_mut(&physical)
            .unwrap()
            .task
            .transition = Some(transition.clone());
        if transition.completed_update.is_none()
            && let Some(state) = self.sound_channels.get_mut(&physical)
        {
            state.volume = x86_i32(transition.new_start) as u32;
            crate::sound_control::apply_live(state)
                .map_err(|cause| self.sound_host_error(cause))?;
        }
        let old_matches = self
            .sound_channels
            .get(&transition.old)
            .is_some_and(|state| {
                state.handle == transition.old_handle
                    && state.task.resource_id == transition.old_resource_id
            });
        let mut retained_channel = physical;
        if transition
            .completed_update
            .is_some_and(|serial| serial != update)
        {
            if self.sound_channels.get(&transition.old).is_some_and(|state| state.pending_start.is_some()) {
                return Ok(Some(physical));
            }
            if old_matches {
                if self.control_sound_channel_values(156, transition.old, 0, 0)? == NativeReturn::Retry {
                    // The original source still owns its stop ticket. Keep
                    // the new physical slot and transition until it releases;
                    // replacing the logical slot here would orphan that ticket.
                    return Ok(Some(physical));
                }
            }
            let can_move = self
                .sound_channels
                .get(&transition.logical)
                .is_none_or(|state| {
                    state.task.resource_id == transition.old_resource_id || state.resource.is_none()
                });
            if transition.logical != physical && transition.logical < 64 && can_move {
                // Move ownership, never stop/reopen the new device. Subsequent
                // loads can crossfade into the now-free alternate slot again.
                if let Some(mut state) = self.sound_channels.remove(&physical) {
                    state.task.linked = None;
                    state.task.load_mode = 0;
                    state.task.transition = None;
                    state.task.alternate_origin = None;
                    self.sound_channels.insert(transition.logical, state);
                    retained_channel = transition.logical;
                }
            } else if let Some(state) = self.sound_channels.get_mut(&physical) {
                state.task.load_mode = 0;
                state.task.linked = None;
                state.task.transition = None;
                state.task.alternate_origin = None;
            }
        } else {
            if transition.remaining <= 0 && transition.completed_update.is_none() {
                // New physical slot follows the old slot in the 64-channel
                // scan. Mark old release now; its next task update performs
                // release/swap. Never stop and reopen the new PCM device.
                transition.completed_update = Some(update);
                if old_matches && let Some(old) = self.sound_channels.get_mut(&transition.old) {
                    old.task.delayed = Some((DelayedControl::Release, 0));
                    old.task.release_priority = true;
                }
            }
            if let Some(state) = self.sound_channels.get_mut(&physical) {
                state.task.transition = Some(transition);
            }
        }
        Ok(Some(retained_channel))
    }

    pub(super) fn stamp_sound_start(&mut self, channel: u32) {
        let now = self.host_millis();
        let state = self.sound_channels.entry(channel).or_default();
        if state.task.started_tick == u32::MAX {
            state.task.started_tick = now;
        }
    }

    fn select_task_sound_group_channel(&mut self, group: &SoundGroup) -> Result<Option<u32>> {
        let first = group.range[0].clamp(0, 64) as u32;
        let last = group.range[1].clamp(0, 64) as u32;
        if first == 64 {
            return Err(audio_script_error("sound group physical endpoint 64 is out of bounds"));
        }
        let mut available = Vec::new();
        for channel in first..=last {
            if channel == 64 {
                return Err(audio_script_error("sound group physical endpoint 64 is out of bounds"));
            }
            // A suspended load reserves the logical group slot even when its
            // actual material is preparing in logical+32's alternate buffer.
            let reserved = self.sound_channels.get(&channel).is_some_and(|state| state.pending_start.is_some())
                || self.data_services.sound_async.task_loads.values().any(|load| load.logical == channel);
            if reserved { continue; }
            available.push(channel);
            if self.sound_playing(channel)? == 0 { return Ok(Some(channel)); }
        }
        if first <= last && available.is_empty() { return Ok(None); }
        let selected = available.iter().copied().find(|channel| self.sound_channels.get(channel)
                .is_some_and(|state| state.task.release_priority))
            .or_else(|| available.iter().copied().find(|channel| self.sound_channels.get(channel)
                .is_some_and(|state| state.task.control_priority)))
            .unwrap_or_else(|| available.first().copied().unwrap_or(first));
        // All busy (or reversed range) is an actual reclamation, even when a
        // subsequent alternate-mode load targets selected+32. Stop the old
        // device now; it must not survive merely because another slot loads.
        let remembered = self.sound_channels.get(&selected).map(|state| {
            (
                state.task.last_tick,
                state.task.loop_range[1],
                state.task.linked,
            )
        });
        self.control_sound_channel_values(156, selected, 0, 0)?;
        if let Some((last_tick, loop_end, linked)) = remembered
            && let Some(state) = self.sound_channels.get_mut(&selected)
        {
            state.task.last_tick = last_tick;
            state.task.loop_range[1] = loop_end;
            state.task.linked = linked;
        }
        Ok(Some(selected))
    }

    fn delay_or_control_sound(&mut self, channel: u32, action: DelayedControl) -> Result<()> {
        let playing = self.sound_playing(channel)? != 0;
        let now = self
            .sound_channels
            .get(&channel)
            .and_then(|state| state.task.owner)
            .and_then(|id| self.data_services.tasks.task_clock(id))
            .map_or(self.task_time.wrapping_sub(self.task_origin), |clock| {
                clock.elapsed()
            });
        let state = self.sound_channels.entry(channel).or_default();
        if state.task.grouped && playing {
            if action == DelayedControl::Release
                && state
                    .task
                    .delayed
                    .is_some_and(|(pending, _)| pending == DelayedControl::Release)
            {
                return Ok(());
            }
            state.task.release_priority = action == DelayedControl::Release;
            state.task.control_priority = action != DelayedControl::Release;
            state.task.transition = None;
            state.task.delayed = Some((action, 10));
            state.fade_clock = now;
            state.volume_fade = Some(crate::audio_control::AudioFade {
                current: state.volume as f32,
                target: 0.,
                remaining: 10,
            });
            state.pan_fade = None;
        } else if action == DelayedControl::Release && state.task.grouped {
            state.task.started_tick = u32::MAX;
            state.task.elapsed = 0;
            state.task.last_tick = 0;
        } else {
            self.control_sound_channel_values(control_index(action), channel, 0, 0)?;
        }
        Ok(())
    }

    pub(super) fn discard_cancelled_queued_sound_load(&mut self, load: &crate::sound_native::PendingTaskSoundLoad) {
        if !load.queued { return; }
        if let Some(task) = self.data_services.sounds.tasks.get_mut(&load.task) {
            let owns_front = task.pending.front().is_some_and(|next|
                next.packet[1] == load.code && next.input.as_deref() == Some(load.record.as_slice()));
            if owns_front { task.pending.pop_front(); }
        } else {
            // The receiver temporarily owns its task outside the service map.
            // Apply this exact receipt immediately after it is reinserted.
            self.data_services.sound_async.queued_cancellations.insert(load.task, load.clone());
        }
    }

    /// Called from kind13's real task body after TaskClock::update. Queue
    /// inputs are retained on failure; successful commands are not repeated.
    pub(super) fn update_sound_task(&mut self, id: u32) -> Result<()> {
        self.register_sound_task(id, &[])?;
        // Queued commands are executed by the sound service, which owns
        // their completion even though no script VM remains on the stack.
        let caller = self.current_task_id;
        self.current_task_id = id;
        let result = (|| {
        if self.poll_task_audio_start(id)? == Some(NativeReturn::Retry) {
            return Ok(());
        }
        let finished: Vec<_> = self.sound_channels.iter().filter_map(|(&channel, state)|
            (state.task.owner == Some(id) && state.pending_start.is_none())
                .then_some(channel).filter(|_| state.task.delayed_finish.is_some())).collect();
        for channel in finished {
            let state = self.sound_channels.get_mut(&channel).unwrap();
            let grouped = state.task.delayed_finish.take().unwrap();
            state.task.owner = Some(id);
            state.task.grouped = grouped;
            state.task.started_tick = u32::MAX;
            state.task.elapsed = 0;
            state.task.last_tick = 0;
            state.volume = x86_i32(state.task.requested_volume) as u32;
            crate::sound_control::apply_live(state).map_err(|cause| self.sound_host_error(cause))?;
        }
        if !self.data_services.sounds.tasks[&id].enabled {
            return Ok(());
        }
        loop {
            let next = self.data_services.sounds.tasks[&id]
                .pending
                .front()
                .cloned();
            let Some(mut next) = next else {
                break;
            };
            let mut output = 0;
            let previous_queue_owner = self.data_services.sound_async.queued_owner.replace(id);
            let dispatch = self.dispatch_sound_task_command(
                id,
                message_command(id, next.packet),
                &mut TaskCommandBuffers {
                    data0: None,
                    data1: next.input.as_deref_mut(),
                },
                &mut output,
            );
            self.data_services.sound_async.queued_owner = previous_queue_owner;
            dispatch?;
            if self.data_services.sound_async.loads.contains_key(&id)
                || self.data_services.sound_async.task_loads.contains_key(&id) { return Ok(()); }
            self.data_services
                .sounds
                .tasks
                .get_mut(&id)
                .unwrap()
                .pending
                .pop_front();
            if self.task_waits_for_audio(id) {
                return Ok(());
            }
        }
        let clock = self.data_services.tasks.task_clock(id).unwrap_or_default();
        let stereo = self.data_services.sounds.tasks[&id].stereo;
        let task = self.data_services.sounds.tasks.get_mut(&id).unwrap();
        task.update_serial = task.update_serial.wrapping_add(1);
        let update = task.update_serial;
        let channels: Vec<_> = self
            .sound_channels
            .iter()
            .filter_map(|(&channel, state)| (state.task.owner == Some(id)).then_some(channel))
            .collect();
        for channel in channels {
            // Its script/FES caller may own the outstanding operation.
            // Preserve its handle, timing and fades until that caller polls.
            if self.sound_channels[&channel].pending_start.is_some() {
                continue;
            }
            let playing = self.sound_playing(channel)? != 0;
            if !playing
                && self
                    .sound_channels
                    .get(&channel)
                    .is_some_and(|state| state.task.release_when_finished)
            {
                self.control_sound_channel_values(156, channel, 0, 0)?;
                continue;
            }
            if let Some(retained) =
                self.advance_sound_transition(channel, clock.elapsed(), update)?
            {
                if let Some(state) = self.sound_channels.get_mut(&retained) {
                    // Crossfade and handoff retain the source's task clock;
                    // command220 must not freeze for the fade's duration.
                    let delta = if state.task.last_tick == u32::MAX {
                        1
                    } else {
                        clock.current.wrapping_sub(state.task.last_tick)
                    };
                    state.task.elapsed = state.task.elapsed.wrapping_add(delta);
                    state.task.last_tick = clock.current;
                    crate::sound_control::advance_fades_at(state, clock.elapsed(), stereo)
                        .map_err(|cause| self.sound_host_error(cause))?;
                }
                continue;
            }
            let state = self.sound_channels.get_mut(&channel).unwrap();
            if let Some((action, ticks)) = state.task.delayed {
                if ticks == 0 {
                    let grouped = state.task.grouped;
                    if self.control_sound_channel_values(control_index(action), channel, 0, 0)? == NativeReturn::Retry {
                        if let Some(state) = self.sound_channels.get_mut(&channel) {
                            state.task.delayed_finish = Some(grouped);
                        }
                        continue;
                    }
                    if let Some(state) = self.sound_channels.get_mut(&channel) {
                        state.task.owner = Some(id);
                        state.task.grouped = grouped;
                        state.task.delayed = None;
                        state.task.release_priority = false;
                        state.task.control_priority = false;
                        state.task.started_tick = u32::MAX;
                        state.task.elapsed = 0;
                        state.task.last_tick = 0;
                        state.volume = x86_i32(state.task.requested_volume) as u32;
                        crate::sound_control::apply_live(state)
                            .map_err(|cause| self.sound_host_error(cause))?;
                    }
                    continue;
                }
                let delta = clock.elapsed().wrapping_sub(state.fade_clock);
                state.task.delayed = Some((action, ticks.saturating_sub(delta)));
                if let Some(fade) = state.volume_fade.as_mut() {
                    let active = fade.advance(delta);
                    state.volume = fade.raw_value();
                    if active {
                        state.fade_clock = clock.elapsed();
                    }
                }
                crate::sound_control::apply_live(state)
                    .map_err(|cause| self.sound_host_error(cause))?;
                continue;
            }
            let delta = if state.task.last_tick == u32::MAX {
                1
            } else {
                clock.current.wrapping_sub(state.task.last_tick)
            };
            state.task.elapsed = state.task.elapsed.wrapping_add(delta);
            state.task.last_tick = clock.current;
            crate::sound_control::advance_fades_at(state, clock.elapsed(), stereo)
                .map_err(|cause| self.sound_host_error(cause))?;
        }
        Ok(())
        })();
        self.current_task_id = caller;
        result
    }

    /// Retires device workers before destroying the task and its owned queue.
    /// FES object release does not call this: the sound service owns playback.
    pub(super) fn release_sound_task(&mut self, id: u32) -> Result<()> {
        let channels: Vec<_> = self
            .sound_channels
            .iter()
            .filter_map(|(&channel, state)| (state.task.owner == Some(id)).then_some(channel))
            .collect();
        for channel in channels {
            // No task invocation survives release to consume a completion.
            // Cancel its ticket, then give any published playback to the
            // backend's retirement queue without joining the device thread.
            self.cancel_audio_channel_start(channel)?;
            if let Some(handle) = self.sound_channels.get(&channel).and_then(|state| state.handle) {
                if let cs2_runtime::AudioControl::Pending(ticket) = self.audio_host.begin_stop(handle)
                    .map_err(|cause| self.sound_host_error(cause))?
                {
                    self.audio_host.cancel_control(ticket)
                        .map_err(|cause| self.sound_host_error(cause))?;
                }
            }
            self.sound_channels.remove(&channel);
        }
        self.data_services.sound_async.cancelled.remove(&id);
        self.data_services.sound_async.loads.remove(&id);
        self.data_services.sound_async.task_loads.remove(&id);
        self.data_services.sound_async.queued_cancellations.remove(&id);
        self.data_services.sounds.tasks.remove(&id);
        Ok(())
    }
}

fn sound_specific(code: u32) -> bool {
    code == 19 || (180..=229).contains(&code)
}

#[cfg(test)]
mod async_transition_tests {
    use super::*;
    use crate::{HeadlessPlayer, SoundChannelState};
    use cs2_runtime::{AudioChannel, AudioHost, AudioRequest, AudioStart};
    use std::time::{Duration, Instant};

    #[test]
    fn async_crossfade_handoff_preserves_the_old_owners_stop_ticket() {
        let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let directory = std::env::temp_dir().join(format!("cs2-crossfade-{}-{nonce}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let mut player = HeadlessPlayer::open_with_save_root(&directory, directory.join("profile")).unwrap();
        let (started, startup) = std::sync::mpsc::channel();
        let (cleanup, controls) = std::sync::mpsc::channel();
        let AudioStart::Pending(ticket) = player.audio_host.begin_test_pending_start_with_controls(AudioRequest {
            channel: AudioChannel::Bgm, resource: "old.ogg".into(), volume: 1., pan: 0., looped: false,
        }, startup, Some(controls)).unwrap() else { panic!("pending startup required"); };
        started.send(Ok(())).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        let playback = loop {
            if let Some(playback) = player.audio_host.poll_play(ticket).unwrap() { break playback; }
            assert!(Instant::now() < deadline); std::thread::yield_now();
        };
        let owner = player.current_task_id;
        let other = player.with_host(|host| Ok(host.create_task_object(0, owner, b"other owner", b"")?)).unwrap();
        player.sound_channels.insert(6, SoundChannelState { resource: Some("old.ogg".into()),
            handle: Some(playback.handle), task: SoundTaskChannel { owner: Some(owner), resource_id: 1,
                ..Default::default() }, ..Default::default() });
        player.sound_channels.insert(38, SoundChannelState { resource: Some("new.ogg".into()),
            task: SoundTaskChannel { owner: Some(owner), resource_id: 2,
                transition: Some(ChannelTransition { logical: 6, old: 6, clock: 1, remaining: 0,
                    completed_update: Some(1), new_volume: 100., old_handle: Some(playback.handle),
                    old_resource_id: 1, new_start: 100. }), ..Default::default() }, ..Default::default() });
        assert_eq!(player.with_host(|host| Ok(host.advance_sound_transition(38, 2, 2)?)).unwrap(), Some(38));
        assert_eq!(player.sound_channels[&6].pending_start.as_ref().unwrap().owner, owner);
        player.with_host(|host| {
            host.current_task_id = other;
            assert_eq!(host.advance_sound_transition(38, 3, 3)?, Some(38));
            Ok(())
        }).unwrap();
        assert_eq!(player.sound_channels[&6].pending_start.as_ref().unwrap().owner, owner);
        assert_eq!(player.sound_channels[&38].resource.as_deref(), Some("new.ogg"));
        cleanup.send(Ok(())).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while player.sound_channels[&6].pending_start.is_some() {
            player.with_host(|host| Ok(host.poll_task_audio_start(owner)?)).unwrap();
            assert!(Instant::now() < deadline); std::thread::yield_now();
        }
        assert_eq!(player.with_host(|host| Ok(host.advance_sound_transition(38, 4, 4)?)).unwrap(), Some(6));
        assert!(!player.sound_channels.contains_key(&38));
        assert_eq!(player.sound_channels[&6].resource.as_deref(), Some("new.ogg"));
        assert!(player.sound_channels[&6].task.transition.is_none());
        drop(player);
        std::fs::remove_dir_all(directory).unwrap();
    }
}

fn message_command(id: u32, packet: TaskMessage) -> TaskCommand {
    TaskCommand {
        target: id as i32,
        sender: packet[0] as i32,
        code: packet[1],
        data: [packet[2], packet[3], 0],
    }
}

fn sound_group(task: &SoundTask, by_key: bool, key: u32) -> Option<&SoundGroup> {
    if by_key {
        task.groups.iter().find(|group| group.key == key)
    } else {
        task.groups
            .iter()
            .filter(|group| group.id == key)
            .min_by_key(|group| group.key)
    }
}

fn folded_name(name: &[u8]) -> Vec<u8> {
    name.iter().map(u8::to_ascii_lowercase).collect()
}

fn returned_value(value: NativeReturn) -> u32 {
    if let NativeReturn::Value(value) = value {
        value
    } else {
        0
    }
}

fn control_index(action: DelayedControl) -> u32 {
    match action {
        DelayedControl::Release => 156,
        DelayedControl::Pause => 158,
        DelayedControl::Stop => 159,
    }
}

pub(super) fn x86_i32(value: f32) -> i32 {
    if !value.is_finite() || value < -2_147_483_648. || value >= 2_147_483_648. {
        i32::MIN
    } else {
        value.trunc() as i32
    }
}

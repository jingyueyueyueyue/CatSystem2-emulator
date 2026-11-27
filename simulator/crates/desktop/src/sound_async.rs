//! Device completions suspend one logical task, never the scene frame.
//! Only owned tickets cross threads; VM/channel mutations remain ordered here.
use std::collections::{BTreeMap, BTreeSet};
use cs2_runtime::{AudioChannel, AudioControl, AudioRequest, AudioStart, PlaybackState, RuntimeError};
use cs2_script::NativeReturn;
use crate::{LoggingHost, SoundChannelState, native_play_looped, should_resume_playback};

#[derive(Default)]
pub(super) struct SoundAsyncStore {
    pub(super) resources: Option<std::sync::Arc<cs2_runtime::ResourceIndex>>,
    pub(super) cancelled: BTreeSet<u32>,
    pub(super) loads: BTreeMap<u32, crate::sound_native::PendingSoundLoad>,
    pub(super) task_loads: BTreeMap<u32, crate::sound_native::PendingTaskSoundLoad>,
    pub(super) queued_owner: Option<u32>,
    pub(super) queued_cancellations: BTreeMap<u32, crate::sound_native::PendingTaskSoundLoad>,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct PendingSoundStart {
    pub(super) started: std::time::Instant,
    pub(super) owner: u32,
    pub(super) index: u32,
    pub(super) value: u32,
    pub(super) configured: Option<AudioChannel>,
    pub(super) transition: Option<[u32; 3]>,
    pub(super) operation: Operation,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum Operation {
    Start(u64),
    StopForStart(u64),
    StopForLoad(u64),
    PrepareLoad,
    Resume(u64, u32),
    Pause(u64),
    Stop(u64),
    Release(u64),
}

impl LoggingHost<'_> {
    /// Device and model waits can resume on a later logical frame.
    pub(super) fn task_waits_for_audio(&self, owner: u32) -> bool {
        // A synchronous sender still needs the receiver's scheduled body
        // after its media ticket is consumed, even if that body has yet to run.
        if self.data_services.tasks.waits_for_message_update(owner) {
            return true;
        }
        // Image preparation can interrupt a compound scene update after it has
        // registered a layer but before it has set its position and opacity.
        // Retry the same logical frame so those intermediate layers stay private.
        if self.data_services.image_manager.pending_image_owners().any(|caller|
            self.data_services.tasks.audio_wait_reaches(caller, owner)) {
            return false;
        }
        self.task_has_pending_audio(owner)
            || self.data_services.emote.pending_load_owners().any(|caller|
                self.data_services.tasks.audio_wait_reaches(caller, owner))
    }

    fn task_has_pending_audio(&self, owner: u32) -> bool {
        self.sound_channels.values().filter_map(|state| state.pending_start.as_ref())
            .any(|pending| self.data_services.tasks.audio_wait_reaches(pending.owner, owner))
            || self.data_services.sound_async.task_loads.keys().any(|&caller|
                self.data_services.tasks.audio_wait_reaches(caller, owner))
            || self.data_services.sound_async.cancelled.iter().any(|&cancelled|
                self.data_services.tasks.audio_wait_reaches(cancelled, owner))
    }

    pub(super) fn retain_scene_during_list_setup(&mut self) -> bool {
        // A newly bound source may still need its motion or alpha setup when
        // the sound call returns. Keep the canvas while the sound task advances.
        let mut owners = std::mem::take(&mut self.data_services.tasks.new_visible_list_sources);
        owners.retain(|&owner| self.task_has_pending_audio(owner));
        let retain_scene = !owners.is_empty();
        self.data_services.tasks.new_visible_list_sources = owners;
        retain_scene
    }

    pub(super) fn cancel_audio_channel_start(&mut self, channel: u32) -> cs2_script::Result<()> {
        let pending = self.sound_channels.get_mut(&channel).and_then(|state| state.pending_start.take());
        let Some(pending) = pending else { return Ok(()); };
        self.data_services.sound_async.cancelled.insert(pending.owner);
        if matches!(pending.operation, Operation::StopForStart(_) | Operation::StopForLoad(_) | Operation::Stop(_) | Operation::Release(_)) {
            if let Some(state) = self.sound_channels.get_mut(&channel) { state.handle = None; }
        }
        self.data_services.sound_async.loads.remove(&pending.owner);
        if matches!(pending.operation, Operation::PrepareLoad) {
            crate::perf_detail::audio_async_note(&format!("event=material_cancel owner={} channel={channel}", pending.owner));
        }
        if let Some(load) = self.data_services.sound_async.task_loads.remove(&pending.owner) {
            self.discard_cancelled_queued_sound_load(&load);
        }
        match pending.operation {
            Operation::PrepareLoad => Ok(()),
            Operation::Start(ticket) => self.audio_host.cancel_play(ticket),
            Operation::StopForStart(ticket) | Operation::StopForLoad(ticket) | Operation::Resume(ticket, _) | Operation::Pause(ticket)
                | Operation::Stop(ticket) | Operation::Release(ticket) => self.audio_host.cancel_control(ticket),
        }.map_err(|cause| self.sound_host_error(cause))
    }

    /// Called before repeating a native dispatch or sending the FES PLAY
    /// callback, so its receiver/prelude is never executed twice.
    pub(super) fn poll_task_audio_start(&mut self, owner: u32) -> cs2_script::Result<Option<NativeReturn>> {
        let channels = self.sound_channels.iter().filter_map(|(&channel, state)|
            state.pending_start.as_ref().filter(|pending| pending.owner == owner).map(|_| channel)).collect::<Vec<_>>();
        if channels.is_empty() {
            return Ok(self.data_services.sound_async.cancelled.remove(&owner).then_some(NativeReturn::Continue));
        }
        // Loads must return through their original receiver so grouped
        // allocation, metadata and the native return value commit together.
        if channels.iter().any(|channel| self.sound_channels[channel].pending_start.as_ref()
            .is_some_and(|pending| matches!(pending.operation, Operation::StopForLoad(_) | Operation::PrepareLoad))) {
            return Ok(None);
        }
        // Linked original/alternate channels belong to one script command.
        // All must finish before its VM or FES callback is acknowledged.
        for channel in channels { self.poll_audio_channel_start(channel)?; }
        if self.sound_channels.values().any(|state| state.pending_start.as_ref().is_some_and(|pending| pending.owner == owner)) {
            Ok(Some(NativeReturn::Retry))
        } else {
            self.data_services.sound_async.cancelled.remove(&owner);
            Ok(Some(NativeReturn::Continue))
        }
    }

    fn poll_audio_channel_start(&mut self, channel: u32) -> cs2_script::Result<NativeReturn> {
        let mut state = self.sound_channels.remove(&channel).expect("pending channel exists");
        let mut pending = state.pending_start.take().expect("pending operation exists");
        let result = self.advance_audio_operation(&mut state, &mut pending);
        let result = match result {
            Ok(false) => { state.pending_start = Some(pending); Ok(NativeReturn::Retry) }
            Ok(true) => {
                crate::perf_detail::audio_async_note(&format!("event=completed owner={} channel={channel} native={} pending_wall_us={}",
                    pending.owner, pending.index, pending.started.elapsed().as_micros()));
                let transition = pending.transition;
                self.sound_channels.insert(channel, state);
                if let Some([owner, logical, physical]) = transition {
                    self.begin_sound_transition(owner, logical, physical)?;
                }
                return Ok(NativeReturn::Continue);
            }
            Err(RuntimeError::AudioStartTimeout(cause)) if pending.configured == Some(AudioChannel::SoundEffect) && pending.index == 157 => {
                crate::perf_detail::audio_async_note(&format!("event=timeout_skipped owner={} channel={channel} native={} pending_wall_us={}",
                    pending.owner, pending.index, pending.started.elapsed().as_micros()));
                state.paused = false;
                state.repeat_remaining = 0;
                state.task.signal = None;
                state.source_query_stopped();
                if let Some(event) = self.events.last_mut() {
                    event.detail.push_str(&format!(" sound_effect_start_skipped channel={channel}: {cause}"));
                }
                Ok(NativeReturn::Continue)
            }
            Err(cause) => {
                crate::perf_detail::audio_async_note(&format!("event=failed owner={} channel={channel} native={} pending_wall_us={}",
                    pending.owner, pending.index, pending.started.elapsed().as_micros()));
                Err(self.sound_host_error(cause))
            }
        };
        self.sound_channels.insert(channel, state);
        result
    }

    pub(super) fn begin_async_sound_control(&mut self, index: u32, channel: u32, value: u32)
        -> cs2_script::Result<Option<NativeReturn>>
    {
        if !matches!(index, 156..=159) || channel >= 64 { return Ok(None); }
        if let Some(pending) = self.sound_channels.get(&channel).and_then(|state| state.pending_start.as_ref()) {
            if pending.owner == self.current_task_id && pending.index == index {
                return self.poll_task_audio_start(self.current_task_id);
            }
            self.cancel_audio_channel_start(channel)?;
        }
        let mut state = self.sound_channels.remove(&channel).unwrap_or_default();
        let configured = state.resource.as_ref().and_then(|resource| self.configured_sound_channel(resource, state.task.is_speech));
        let _profile = crate::perf_detail::audio::Profile::new(self.current_task_id, channel, "control_enqueue", Some(index), state.resource.as_deref().unwrap_or(""));
        let mut pending = PendingSoundStart { started: std::time::Instant::now(), owner: self.current_task_id, index, value, configured,
            transition: None, operation: Operation::Start(0) };
        let result = (|| -> cs2_runtime::RuntimeResult<bool> {
            if matches!(index, 157..=159) && let Some(handle) = state.handle
                && !self.audio_host.is_active(handle)?
            {
                state.handle = None;
                state.paused = false;
            }
            state.task.cancel_pending_control();
            if index == 157 {
                if state.task.started_tick == u32::MAX { state.task.started_tick = self.host_millis(); }
                state.volume_fade = None;
                state.pan_fade = None;
                state.volume = if state.task.load_mode != 0 { 0 } else { crate::sound_task::x86_i32(state.task.requested_volume) as u32 };
                if should_resume_playback(&state) {
                    let origin = state.task.signal.as_ref().map_or(0, |signal| signal.completed_passes());
                    crate::sound_control::apply_live(&state)?;
                    // A short clip can finish before the next engine tick.
                    // Publish its new repeat policy before unpausing the worker.
                    crate::audio_control::set_playback_repeat_count(state.handle.unwrap(), value)?;
                    return match self.audio_host.begin_pause(state.handle.unwrap(), false)? {
                        AudioControl::Pending(ticket) => { pending.operation = Operation::Resume(ticket, origin); Ok(false) }
                        AudioControl::Ready => self.finish_audio_resume(&mut state, value, origin).map(|_| true),
                    };
                }
            } else {
                state.volume_fade = None;
                state.pan_fade = None;
            }
            if let Some(handle) = state.handle {
                let control = if index == 158 { self.audio_host.begin_pause(handle, true)? }
                    else { self.audio_host.begin_stop(handle)? };
                if let AudioControl::Pending(ticket) = control {
                    pending.operation = match index { 156 => Operation::Release(ticket), 157 => Operation::StopForStart(ticket),
                        158 => Operation::Pause(ticket), _ => Operation::Stop(ticket) };
                    return Ok(false);
                }
            }
            if index == 157 { self.queue_audio_start(&mut state, &mut pending) }
            else { self.finish_audio_control(&mut state, index); Ok(true) }
        })();
        let result = match result {
            Ok(false) => {
                crate::perf_detail::audio_async_note(&format!("event=queued owner={} channel={channel} native={index} operation={:?}", pending.owner, pending.operation));
                state.pending_start = Some(pending); Ok(NativeReturn::Retry)
            }
            Ok(true) => Ok(NativeReturn::Continue),
            Err(RuntimeError::AudioStartTimeout(cause)) if configured == Some(AudioChannel::SoundEffect) && index == 157 => {
                state.paused = false;
                state.repeat_remaining = 0;
                state.task.signal = None;
                state.source_query_stopped();
                if let Some(event) = self.events.last_mut() { event.detail.push_str(&format!(" sound_effect_start_skipped channel={channel}: {cause}")); }
                Ok(NativeReturn::Continue)
            }
            Err(cause) => Err(self.sound_host_error(cause)),
        };
        self.sound_channels.insert(channel, state);
        result.map(Some)
    }

    fn queue_audio_start(&mut self, state: &mut SoundChannelState, pending: &mut PendingSoundStart) -> cs2_runtime::RuntimeResult<bool> {
        state.handle = None;
        let Some(resource) = state.resource.clone() else { return Ok(true); };
        let request = AudioRequest { channel: pending.configured.unwrap_or_else(|| crate::sound_audio_channel(&resource)),
            resource, volume: state.request_volume(), pan: state.request_pan(), looped: native_play_looped(pending.value) };
        match crate::audio::begin_loaded_sound_range(self.audio_host, request, state.prepared_audio.clone(),
            state.duration_frames, pending.value, state.task.signal_enabled,
            crate::audio_control::AudioLoop { range: state.task.loop_range, whole: state.task.whole_buffer })?
        {
            AudioStart::Pending(ticket) => { pending.operation = Operation::Start(ticket); Ok(false) }
            AudioStart::Ready(playback) => self.finish_audio_start(state, pending.value, playback).map(|_| true),
        }
    }

    fn advance_audio_operation(&mut self, state: &mut SoundChannelState, pending: &mut PendingSoundStart)
        -> cs2_runtime::RuntimeResult<bool>
    {
        match pending.operation {
            Operation::StopForLoad(_) | Operation::PrepareLoad => Ok(false),
            Operation::Start(ticket) => match self.audio_host.poll_play(ticket)? {
                None => Ok(false), Some(playback) => self.finish_audio_start(state, pending.value, playback).map(|_| true),
            },
            Operation::StopForStart(ticket) => {
                if self.audio_host.poll_control(ticket)?.is_none() { return Ok(false); }
                self.queue_audio_start(state, pending)
            }
            Operation::Resume(ticket, origin) => {
                if self.audio_host.poll_control(ticket)?.is_none() { return Ok(false); }
                self.finish_audio_resume(state, pending.value, origin).map(|_| true)
            }
            Operation::Pause(ticket) | Operation::Stop(ticket) | Operation::Release(ticket) => {
                if self.audio_host.poll_control(ticket)?.is_none() { return Ok(false); }
                self.finish_audio_control(state, pending.index); Ok(true)
            }
        }
    }

    fn finish_audio_start(&mut self, state: &mut SoundChannelState, count: u32, playback: PlaybackState) -> cs2_runtime::RuntimeResult<()> {
        let controls = crate::audio_control::has_desktop_controls(playback.handle);
        if !controls && (state.task.is_speech || state.task.load_mode != 0 || (!state.task.whole_buffer && state.task.loop_range != [0, -1])) {
            self.audio_host.stop(playback.handle)?;
            return Err(RuntimeError::Host("this audio host cannot consume generated PCM, loop ranges or alternate fades".into()));
        }
        state.handle = Some(playback.handle);
        state.paused = false;
        state.looped = playback.looped;
        state.task.signal = if controls { Some(crate::audio_control::signal_monitor(playback.handle)?) } else { None };
        state.source_query_started(0, count);
        state.repeat_remaining = if controls || playback.looped { 0 } else { count.saturating_sub(1) };
        Ok(())
    }

    fn finish_audio_resume(&mut self, state: &mut SoundChannelState, count: u32, origin: u32) -> cs2_runtime::RuntimeResult<()> {
        let active = match state.handle { Some(handle) => self.audio_host.is_active(handle)?, None => false };
        if !active {
            state.handle = None;
            state.paused = false;
            state.repeat_remaining = 0;
            state.task.signal = None;
            state.source_query_stopped();
            return Ok(());
        }
        state.paused = false;
        state.looped = count == 0;
        state.repeat_remaining = 0;
        state.source_query_started(origin, count);
        Ok(())
    }

    fn finish_audio_control(&self, state: &mut SoundChannelState, index: u32) {
        match index {
            156 => { let owner = state.task.owner; let delayed_finish = state.task.delayed_finish;
                let mut sidecars = std::mem::take(&mut state.task.sidecars);
                sidecars.clear_auxiliary(); state.release(); state.task.owner = owner;
                state.task.delayed_finish = delayed_finish; state.task.sidecars = sidecars; }
            158 => { if state.handle.is_some() { state.paused = true; } }
            159 => { state.handle = None; state.paused = false; state.repeat_remaining = 0; state.source_query_stopped(); }
            _ => {}
        }
    }
}

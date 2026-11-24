//! Device-backed sound controls; property changes never restart sample playback.
use crate::audio_control::{AudioFade, set_playback_properties};
use crate::{
    HeadlessPlayer, LoggingHost, SoundChannelState, audio_script_error, native_play_looped,
    read_u32, should_resume_playback, sound_audio_channel,
};
use cs2_runtime::{AudioChannel, AudioRequest, RuntimeError, TaskCommand, TaskCommandBuffers};
use cs2_script::{NativeReturn, Vm};

/// Platform lifecycle is independent of the game's saved clock/pause state.
/// The explicit audio-only API is another suspension reason, not an alias
/// that can accidentally resume an Activity which is still backgrounded.
#[derive(Debug, Default)]
pub(super) struct HostPauseState {
    pub(super) paused: bool,
    audio_suspended: bool,
    window_audio_suspended: bool,
}

pub(super) fn audio_channel_for_type(kind: &[u8]) -> Option<AudioChannel> {
    if kind.eq_ignore_ascii_case(b"voice") || kind.eq_ignore_ascii_case(b"sysvoice") {
        Some(AudioChannel::Voice)
    } else if kind.eq_ignore_ascii_case(b"bgm") {
        Some(AudioChannel::Bgm)
    } else if kind.eq_ignore_ascii_case(b"se") || kind.eq_ignore_ascii_case(b"sysse") {
        Some(AudioChannel::SoundEffect)
    } else {
        None
    }
}

impl LoggingHost<'_> {
    pub(super) fn sound_is_bgm(&self, resource: &str) -> bool {
        self.configured_sound_channel(resource, false)
            .unwrap_or_else(|| sound_audio_channel(resource)) == AudioChannel::Bgm
    }

    pub(super) fn release_fast_forward_sounds(&mut self) -> cs2_script::Result<()> {
        let channels: Vec<_> = self.sound_channels.iter().filter_map(|(&channel, state)| {
            if state.pending_start.as_ref().is_some_and(|pending| matches!(pending.operation,
                crate::sound_async::Operation::StopForLoad(_) | crate::sound_async::Operation::PrepareLoad))
            {
                return None;
            }
            let resource = state.resource.as_deref()?;
            (state.task.is_speech || !self.sound_is_bgm(resource)).then_some(channel)
        }).collect();
        for channel in channels {
            self.cancel_audio_channel_start(channel)?;
            if let Some(handle) = self.sound_channels[&channel].handle
                && let cs2_runtime::AudioControl::Pending(ticket) = self.audio_host.begin_stop(handle)
                    .map_err(|cause| self.sound_host_error(cause))?
            {
                self.audio_host.cancel_control(ticket).map_err(|cause| self.sound_host_error(cause))?;
            }
            let state = self.sound_channels.get_mut(&channel).unwrap();
            let owner = state.task.owner;
            state.release();
            state.task.owner = owner;
        }
        Ok(())
    }

    pub(super) fn configured_sound_channel(&self, resource: &str, speech: bool) -> Option<AudioChannel> {
        if speech {
            return Some(AudioChannel::Voice);
        }
        let (directory, name) = resource.rsplit_once(['/', '\\']).unwrap_or(("", resource));
        if self.sound_config_enabled == 0 {
            let path = super::sound_native::encode_name(self.text_encoding, resource).ok()?;
            return self.legacy_file_audio_channel(&path, !directory.is_empty());
        }
        let name = super::sound_native::encode_name(self.text_encoding, name).ok()?;
        let path = self.sound_config.find_path(&name, super::SoundMatch::Prefix);
        if !directory.is_empty() {
            let directory = directory.rsplit_once('.')
                .filter(|(_, ext)| ext.eq_ignore_ascii_case("int"))
                .map_or(directory, |(stem, _)| stem);
            let directory = super::sound_native::encode_name(self.text_encoding, directory).ok()?;
            if !path.last()?.archive.eq_ignore_ascii_case(&directory) {
                return None;
            }
        }
        path.iter().rev().find_map(|part| audio_channel_for_type(&part.type_name))
    }

    /// Direct FES command 219 adapter. Updates the existing PCM worker without
    /// starting/restarting playback or changing its paused state.
    pub(super) fn control_sound_play_count(
        &mut self,
        channel: u32,
        count: u32,
    ) -> cs2_script::Result<()> {
        let channel = self.sound_selected_channel(channel);
        if channel >= 64 {
            return Ok(());
        }
        let Some(state) = self.sound_channels.get(&channel) else {
            return Ok(());
        };
        let handle = state.handle;
        if let Some(handle) = handle
            && self
                .audio_host
                .is_active(handle)
                .map_err(|cause| self.sound_host_error(cause))?
        {
            crate::audio_control::set_playback_repeat_count(handle, count)
                .map_err(|cause| self.sound_host_error(cause))?;
        }
        // Commit script bookkeeping only after an active device accepted the
        // policy. The handle, paused bit, source and source cursor are kept.
        if let Some(state) = self.sound_channels.get_mut(&channel) {
            state.looped = count == 0;
            state.repeat_remaining = 0;
            if state.resource.is_some() {
                state.source_query_repeat_changed(count);
            }
        }
        Ok(())
    }

    /// Compatibility name retained for callers from the earlier recovery batch.
    pub(super) fn update_sound_repeat_count(
        &mut self,
        channel: u32,
        count: u32,
    ) -> cs2_script::Result<()> {
        self.control_sound_play_count(channel, count)
    }

    pub(super) fn control_sound_native(
        &mut self,
        index: u32,
        args: &[u8],
        _vm: &mut Vm,
    ) -> cs2_script::Result<NativeReturn> {
        let required = match index {
            157 | 161 | 164 => 8,
            163 | 166 => 10,
            167 => 12,
            168 | 347 => 16,
            348 => 8,
            156 | 158..=160 | 162 | 165 | 349 => 4,
            _ => return Err(audio_script_error("unsupported sound control")),
        };
        if args.len() < required {
            return Err(audio_script_error("sound control arguments are truncated"));
        }
        if matches!(index, 156..=159)
            && let Some(result) = self.poll_task_audio_start(self.current_task_id)?
        { return Ok(result); }
        let channel = read_u32(args, 0)?;
        let (value, duration) = if matches!(index, 163 | 166) {
            let target = u16::from_le_bytes(args[4..6].try_into().unwrap());
            let target = if index == 166 {
                i32::from(target as i16) as u32
            } else {
                u32::from(target)
            };
            (
                target,
                u32::from(u16::from_le_bytes(args[8..10].try_into().unwrap())),
            )
        } else if matches!(index, 157 | 161 | 164) {
            (read_u32(args, 4)?, 0)
        } else {
            (0, 0)
        };
        if let Some(task) = self.default_sound_task()? {
            let code = match index {
                156..=168 => index + 26,
                347..=349 => index - 151,
                _ => return Err(audio_script_error("unsupported sound control")),
            };
            let query = matches!(index, 160 | 162 | 165 | 349);
            if self
                .data_services
                .tasks
                .message_task_state(task)
                .is_none_or(|(kind, deletion, _)| kind != 13 || deletion != 0)
            {
                return if query {
                    Err(audio_script_error(
                        "sound query target is deleted or unavailable",
                    ))
                } else {
                    Ok(NativeReturn::Continue)
                };
            }
            let mut temporary = match index {
                167 | 168 | 347 | 348 => args[4..required].to_vec(),
                349 => u32::MAX.to_le_bytes().to_vec(),
                _ => Vec::new(),
            };
            let packed = if matches!(index, 163 | 166) {
                (value & 0xffff) | (duration << 16)
            } else {
                value
            };
            let mut output = 0;
            let consumed = self.dispatch_sound_task_command(
                task,
                TaskCommand {
                    target: task as i32,
                    sender: self.current_task_id as i32,
                    code,
                    data: [channel, packed, 0],
                },
                &mut TaskCommandBuffers {
                    data0: None,
                    data1: (!temporary.is_empty()).then_some(temporary.as_mut_slice()),
                },
                &mut output,
            )?;
            if !consumed {
                return Err(audio_script_error(
                    "sound task did not consume its control command",
                ));
            }
            if matches!(index, 156..=159) && self.task_waits_for_audio(self.current_task_id) {
                return Ok(NativeReturn::Retry);
            }
            if index == 349 {
                let value = read_u32(&temporary, 0)?;
                if value == u32::MAX {
                    return Err(audio_script_error(
                        "sound 198 did not write its temporary output",
                    ));
                }
                return Ok(NativeReturn::Value(value));
            }
            return Ok(if query {
                NativeReturn::Value(output)
            } else {
                NativeReturn::Continue
            });
        }
        if matches!(index, 167 | 168 | 347 | 348) {
            return Err(audio_script_error(
                "spatial sound control requires a kind 13 task",
            ));
        }
        self.control_sound_values(index, channel, value, duration)
    }

    /// FES and ordinary wrappers share this entry. duration is a task-clock
    /// span for 163/166; value is the target DWORD, not a VM address.
    pub(super) fn control_sound_values(
        &mut self,
        index: u32,
        channel: u32,
        value: u32,
        duration: u32,
    ) -> cs2_script::Result<NativeReturn> {
        if matches!(index, 156..=159)
            && let Some(result) = self.poll_task_audio_start(self.current_task_id)?
        { return Ok(result); }
        if index == 219 {
            self.control_sound_play_count(channel, value)?;
            return Ok(NativeReturn::Continue);
        }
        if let Some(task) = self.default_sound_task()? {
            let code = match index {
                156..=166 => index + 26,
                349 => 198,
                _ => return Err(audio_script_error("unsupported direct sound control")),
            };
            let query = matches!(index, 160 | 162 | 165 | 349);
            let mut output = 0;
            let mut temporary = u32::MAX.to_le_bytes();
            let packed = if matches!(index, 163 | 166) {
                (value & 0xffff) | ((duration & 0xffff) << 16)
            } else {
                value
            };
            let consumed = self.dispatch_sound_task_command(
                task,
                TaskCommand {
                    target: task as i32,
                    sender: self.current_task_id as i32,
                    code,
                    data: [channel, packed, 0],
                },
                &mut TaskCommandBuffers {
                    data0: None,
                    data1: (index == 349).then_some(temporary.as_mut_slice()),
                },
                &mut output,
            )?;
            if !consumed {
                return Err(audio_script_error(
                    "sound task did not consume its direct control",
                ));
            }
            if matches!(index, 156..=159) && self.task_waits_for_audio(self.current_task_id) {
                return Ok(NativeReturn::Retry);
            }
            if index == 349 {
                let value = u32::from_le_bytes(temporary);
                if value == u32::MAX {
                    return Err(audio_script_error(
                        "sound 198 did not write its temporary output",
                    ));
                }
                return Ok(NativeReturn::Value(value));
            }
            return Ok(if query {
                NativeReturn::Value(output)
            } else {
                NativeReturn::Continue
            });
        }
        self.control_sound_channel_values(index, channel, value, duration)
    }

    /// Physical device implementation. Kind13 dispatch/update/release call
    /// this directly, avoiding recursive service lookup or a second fade.
    pub(super) fn control_sound_channel_values(
        &mut self,
        index: u32,
        channel: u32,
        value: u32,
        duration: u32,
    ) -> cs2_script::Result<NativeReturn> {
        if index == 157 && self.data_services.fast_forward
            && self.sound_channels.get(&channel).is_some_and(|state|
                state.task.is_speech || state.resource.as_deref().is_some_and(|name| !self.sound_is_bgm(name)))
        {
            return Ok(NativeReturn::Continue);
        }
        if let Some(result) = self.begin_async_sound_control(index, channel, value)? {
            return Ok(result);
        }
        if index == 219 {
            self.control_sound_play_count(channel, value)?;
            return Ok(NativeReturn::Continue);
        }
        if channel >= 64 {
            return Ok(if matches!(index, 160 | 162 | 165 | 349) {
                NativeReturn::Value(0)
            } else {
                NativeReturn::Continue
            });
        }
        let now_millis = self.host_millis();
        let mut state = self.sound_channels.remove(&channel).unwrap_or_default();
        let _audio = match index {
            156..=159 => Some(crate::perf_detail::audio::Profile::new(
                self.current_task_id,
                channel,
                "control",
                Some(index),
                state.resource.as_deref().unwrap_or(""),
            )),
            _ => None,
        };
        let result = (|| {
            if matches!(index, 157..=166)
                && let Some(handle) = state.handle
                && !self
                    .audio_host
                    .is_active(handle)
                    .map_err(|cause| self.sound_host_error(cause))?
            {
                state.handle = None;
                state.paused = false;
            }
            let mut output = None;
            match index {
                156 => {
                    if let Some(handle) = state.handle {
                        self.audio_host
                            .stop(handle)
                            .map_err(|cause| self.sound_host_error(cause))?;
                    }
                    let owner = state.task.owner;
                    let mut sidecars = std::mem::take(&mut state.task.sidecars);
                    sidecars.clear_auxiliary();
                    state.release();
                    state.task.owner = owner;
                    state.task.sidecars = sidecars;
                }
                157 => {
                    state.task.cancel_pending_control();
                    if state.task.started_tick == u32::MAX {
                        state.task.started_tick = now_millis;
                    }
                    state.volume_fade = None;
                    state.pan_fade = None;
                    state.volume = if state.task.load_mode != 0 {
                        0
                    } else {
                        crate::sound_task::x86_i32(state.task.requested_volume) as u32
                    };
                    if should_resume_playback(&state) {
                        let counter_origin = state.task.signal.as_ref()
                            .map_or(0, |signal| signal.completed_passes());
                        apply_live(&state).map_err(|cause| self.sound_host_error(cause))?;
                        self.audio_host
                            .pause(state.handle.unwrap(), false)
                            .map_err(|cause| self.sound_host_error(cause))?;
                        state.paused = false;
                        crate::audio_control::set_playback_repeat_count(
                            state.handle.unwrap(),
                            value,
                        )
                        .map_err(|cause| self.sound_host_error(cause))?;
                        state.looped = value == 0;
                        state.repeat_remaining = 0;
                        state.source_query_started(counter_origin, value);
                    } else if let Some(resource) = state.resource.clone() {
                        if let Some(handle) = state.handle {
                            self.audio_host
                                .stop(handle)
                                .map_err(|cause| self.sound_host_error(cause))?;
                            state.handle = None;
                        }
                        let configured = self.configured_sound_channel(&resource, state.task.is_speech);
                        let request = AudioRequest {
                            channel: configured.unwrap_or_else(|| sound_audio_channel(&resource)),
                            resource,
                            volume: state.request_volume(),
                            pan: state.request_pan(),
                            looped: native_play_looped(value),
                        };
                        let playback = match crate::audio::play_loaded_sound_range(
                            self.audio_host,
                            request,
                            state.prepared_audio.clone(),
                            state.duration_frames,
                            value,
                            state.task.signal_enabled,
                            crate::audio_control::AudioLoop {
                                range: state.task.loop_range,
                                whole: state.task.whole_buffer,
                            },
                        ) {
                            Ok(playback) => playback,
                            Err(RuntimeError::AudioStartTimeout(cause)) if configured == Some(AudioChannel::SoundEffect) => {
                                state.paused = false;
                                state.repeat_remaining = 0;
                                state.task.signal = None;
                                state.source_query_stopped();
                                if let Some(event) = self.events.last_mut() {
                                    event.detail.push_str(&format!(" sound_effect_start_skipped channel={channel}: {cause}"));
                                }
                                return Ok(NativeReturn::Continue);
                            }
                            Err(RuntimeError::AudioStartTimeout(cause)) => {
                                return Err(self.sound_host_error(format!(
                                    "start timeout channel={channel} resource={:?}: {cause}", state.resource
                                )));
                            }
                            Err(cause) => return Err(self.sound_host_error(cause)),
                        };
                        state.handle = Some(playback.handle);
                        state.paused = false;
                        state.looped = playback.looped;
                        let desktop_controls =
                            crate::audio_control::has_desktop_controls(playback.handle);
                        if !desktop_controls
                            && (state.task.is_speech
                                || state.task.load_mode != 0
                                || (!state.task.whole_buffer && state.task.loop_range != [0, -1]))
                        {
                            self.audio_host
                                .stop(playback.handle)
                                .map_err(|cause| self.sound_host_error(cause))?;
                            state.handle = None;
                            return Err(audio_script_error(
                                "this audio host cannot consume generated PCM, loop ranges or alternate fades",
                            ));
                        }
                        state.task.signal = if desktop_controls {
                            Some(
                                crate::audio_control::signal_monitor(playback.handle)
                                    .map_err(|cause| self.sound_host_error(cause))?,
                            )
                        } else {
                            None
                        };
                        state.source_query_started(0, value);
                        state.repeat_remaining = if desktop_controls || playback.looped {
                            0
                        } else {
                            value.saturating_sub(1)
                        };
                    } else if let Some(event) = self.events.last_mut() {
                        // A documented empty channel does not start a device.
                        // Keep status false and allow the FES PLAY callback.
                        event.detail.push_str(" sound_start=no_loaded_resource");
                    }
                }
                158 => {
                    state.task.cancel_pending_control();
                    state.volume_fade = None;
                    state.pan_fade = None;
                    if let Some(handle) = state.handle {
                        self.audio_host
                            .pause(handle, true)
                            .map_err(|cause| self.sound_host_error(cause))?;
                        state.paused = true;
                    }
                }
                159 => {
                    state.task.cancel_pending_control();
                    state.volume_fade = None;
                    state.pan_fade = None;
                    if let Some(handle) = state.handle {
                        self.audio_host
                            .stop(handle)
                            .map_err(|cause| self.sound_host_error(cause))?;
                        state.handle = None;
                    }
                    state.paused = false;
                    state.repeat_remaining = 0;
                    state.source_query_stopped();
                }
                160 => {
                    if let Some(handle) = state.handle
                        && !self
                            .audio_host
                            .is_active(handle)
                            .map_err(|cause| self.sound_host_error(cause))?
                    {
                        state.handle = None;
                        state.paused = false;
                    }
                    output = Some(u32::from(state.is_playing()));
                }
                161 => {
                    state.volume = value;
                    state.task.requested_volume = value as i32 as f32;
                    state.volume_fade = None;
                    apply_live(&state).map_err(|cause| self.sound_host_error(cause))?;
                }
                162 => {
                    output = Some(if state.resource.is_some() {
                        state.volume
                    } else {
                        0
                    })
                }
                163 => {
                    state.volume_fade = Some(AudioFade {
                        current: if state.resource.is_some() {
                            state.volume as f32
                        } else {
                            0.
                        },
                        target: value as f32,
                        remaining: i64::from(duration),
                    });
                    state.fade_clock = self.sound_fade_clock(&state);
                }
                164 => {
                    state.pan = (value as i32).clamp(-100, 100);
                    state.pan_fade = None;
                    apply_live(&state).map_err(|cause| self.sound_host_error(cause))?;
                }
                165 => {
                    output = Some(if state.resource.is_some() {
                        state.pan as u32
                    } else {
                        0
                    })
                }
                166 => {
                    state.pan_fade = Some(AudioFade {
                        current: if state.resource.is_some() {
                            state.pan as f32
                        } else {
                            0.
                        },
                        target: value as i32 as f32,
                        remaining: i64::from(duration),
                    });
                    state.fade_clock = self.sound_fade_clock(&state);
                }
                349 => output = Some(u32::from(state.volume_fade.is_some())),
                _ => return Err(audio_script_error("unsupported direct sound control")),
            }
            Ok(output.map_or(NativeReturn::Continue, NativeReturn::Value))
        })();
        self.sound_channels.insert(channel, state);
        result
    }

    fn sound_fade_clock(&self, state: &SoundChannelState) -> u32 {
        state
            .task
            .owner
            .and_then(|id| self.data_services.tasks.task_clock(id))
            // PE 189 stores SoundTask::elapsed, not timeGetTime. An explicit
            // no-task host uses the absolute engine tick timeline instead;
            // the same timeline is GameClock.elapsed_ticks in its poll pass.
            .map_or(self.task_time, |clock| clock.elapsed())
    }
}

impl HeadlessPlayer {
    /// Freeze host-driven frames and suspend physical PCM for app lifecycle.
    /// Repeated calls preserve GameClock.paused and each script channel's
    /// paused bit/cursor. Resume never clears an existing game pause.
    ///
    /// Call on the engine thread from Android onPause/onResume (and audio
    /// focus handling). A failed pause still gates frames; a failed resume
    /// retains the host pause so the caller can report/retry the device error.
    pub fn set_host_paused(&mut self, paused: bool) -> anyhow::Result<()> {
        if paused {
            self.data_services.host_pause.paused = true;
        }
        let suspend_audio = paused || self.data_services.host_pause.audio_suspended;
        let ordinary = self.audio_host.set_suspended(suspend_audio || self.data_services.host_pause.window_audio_suspended);
        let movies = self.movie_host.set_host_paused(suspend_audio);
        ordinary.and(movies)?;
        self.data_services.host_pause.paused = paused;
        Ok(())
    }

    /// The Android/desktop application shell can call this on lifecycle
    /// transitions. It does not alter script channel flags or restart PCM.
    /// Clearing this audio-only reason never overrides set_host_paused(true).
    pub fn set_audio_suspended(&mut self, suspended: bool) -> anyhow::Result<()> {
        self.data_services.host_pause.audio_suspended = suspended;
        let suspended = suspended || self.data_services.host_pause.paused;
        let ordinary = self.audio_host.set_suspended(suspended || self.data_services.host_pause.window_audio_suspended);
        let movies = self.movie_host.set_host_paused(suspended);
        ordinary.and(movies)?;
        Ok(())
    }

    /// Window activation is an independent ordinary-sound suspension reason.
    /// Movies keep their existing lifecycle policy. Clearing this reason
    /// never resumes an app/OS suspension or a script-paused PCM source.
    pub(super) fn set_window_audio_suspended(&mut self, suspended: bool) -> anyhow::Result<()> {
        let reasons = &mut self.data_services.host_pause;
        if reasons.window_audio_suspended == suspended { return Ok(()); }
        self.audio_host.set_suspended(suspended || reasons.paused || reasons.audio_suspended)?;
        reasons.window_audio_suspended = suspended;
        Ok(())
    }

    pub(super) fn advance_sound_fades(&mut self) -> anyhow::Result<()> {
        // Unowned physical channels have no pausable TaskClock. Preserve
        // engine tick units, budget boundaries and game pause, not a presumed
        // 60Hz-to-milliseconds ratio or the separate physical host clock.
        let now = self.state.clock.elapsed_ticks as u32;
        for state in self.sound_channels.values_mut() {
            if state.pending_start.is_some() { continue; }
            // Kind13's body uses its real TaskClock and owns queued control.
            if state.task.owner.is_none() {
                if let Some(handle) = state.handle
                    && !self.audio_host.is_active(handle)?
                {
                    state.handle = None;
                    state.paused = false;
                }
                advance_fades_at(state, now, true)?;
            }
        }
        Ok(())
    }
}

pub(super) fn advance_fades_at(
    state: &mut SoundChannelState,
    now: u32,
    stereo: bool,
) -> cs2_runtime::RuntimeResult<()> {
    let mut changed = false;
    if let Some(fade) = state.volume_fade.as_mut() {
        let active = fade.advance(now.wrapping_sub(state.fade_clock));
        state.volume = fade.raw_value();
        if active {
            state.fade_clock = now;
        } else {
            state.volume_fade = None;
        }
        changed = true;
    }
    if let Some(fade) = state.pan_fade.as_mut() {
        let active = fade.advance(now.wrapping_sub(state.fade_clock));
        if stereo {
            state.pan = (fade.raw_value() as i32).clamp(-100, 100);
            changed = true;
        }
        if active {
            state.fade_clock = now;
        } else {
            state.pan_fade = None;
        }
    }
    if changed {
        apply_live(state)?;
    }
    Ok(())
}

pub(super) fn apply_live(state: &SoundChannelState) -> cs2_runtime::RuntimeResult<()> {
    if let Some(handle) = state.handle {
        set_playback_properties(handle, state.request_volume(), state.request_pan())?;
    }
    Ok(())
}

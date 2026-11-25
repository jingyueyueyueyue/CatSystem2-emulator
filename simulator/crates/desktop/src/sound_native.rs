//! Ordinary 935/936 and direct FES sound preparation, without synthetic VM pointers.
#[cfg(test)]
use crate::audio_decode::prepare_named_audio;
use crate::{
    LoggingHost, ScriptTextEncoding, audio_script_error, expand_kcs_format, read_u32, trim_nul,
};
use cs2_runtime::{AudioControl, TaskCommand, TaskCommandBuffers};
use cs2_script::{NativeReturn, Vm};
use std::borrow::Cow;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

#[path = "sound_material.rs"]
mod material;

/// Owned while a real previous device is stopping. Reentry uses these exact
/// names/options even if another task changes the caller's source strings.
#[derive(Clone)]
pub(super) struct PendingSoundLoad {
    pub(super) channel: u32,
    names: Vec<String>,
    selector: u32,
    duration: u32,
    group: Option<u32>,
    source: Option<Arc<material::Source>>,
    material: Option<Arc<material::Ticket>>,
}

#[derive(Clone)]
pub(super) struct PendingTaskSoundLoad {
    pub(super) task: u32,
    pub(super) code: u32,
    pub(super) record: Vec<u8>,
    pub(super) logical: u32,
    pub(super) target: u32,
    pub(super) loop_range: [i64; 2],
    pub(super) queued: bool,
    pub(super) reclaim_state: Option<(u32, i64, Option<u32>)>,
}

static SOUND_RESOURCE_ID: AtomicU64 = AtomicU64::new(1);

pub(crate) fn reserve_checkpoint_resource_identity(next: u64) {
    SOUND_RESOURCE_ID.fetch_max(next, Ordering::AcqRel);
}

pub(super) fn next_sound_resource_identity() -> cs2_script::Result<u64> {
    SOUND_RESOURCE_ID
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
        .map_err(|_| audio_script_error("sound resource identity exhausted"))
}

impl LoggingHost<'_> {
    pub(super) fn load_sound_native(
        &mut self,
        args: &[u8],
        vm: &mut Vm,
    ) -> cs2_script::Result<NativeReturn> {
        if args.len() < 20 {
            return Err(audio_script_error(
                "native 935 requires at least 20 argument bytes",
            ));
        }
        if let Some(result) = self.resume_task_sound_load()? { return Ok(result); }
        if let Some(result) = self.resume_plain_sound_load()? { return Ok(result); }
        let name = expand_kcs_format(vm, &vm.read_string(read_u32(args, 8)?)?, 0)?;
        let duration = ((u64::from(read_u32(args, 16)?) * 1000) / 60) as u32;
        if let Some(task) = self.default_sound_task()? {
            return self.load_sound_task_record(
                task,
                read_u32(args, 0)?,
                false,
                false,
                &name,
                read_u32(args, 12)?,
                duration,
            );
        }
        self.load_sound_name(read_u32(args, 0)?, &name, read_u32(args, 12)?, duration)
    }

    pub(super) fn load_sound_group_native(
        &mut self,
        args: &[u8],
        vm: &mut Vm,
    ) -> cs2_script::Result<NativeReturn> {
        if args.len() < 24 {
            return Err(audio_script_error(
                "native 936 requires at least 24 argument bytes",
            ));
        }
        if let Some(result) = self.resume_task_sound_load()? { return Ok(result); }
        if let Some(result) = self.resume_plain_sound_load()? { return Ok(result); }
        let name = expand_kcs_format(vm, &vm.read_string(read_u32(args, 8)?)?, 0)?;
        let duration = ((u64::from(read_u32(args, 20)?) * 1000) / 60) as u32;
        if let Some(task) = self.default_sound_task()? {
            return self.load_sound_task_record(
                task,
                read_u32(args, 0)?,
                true,
                read_u32(args, 12)? != 0,
                &name,
                read_u32(args, 16)?,
                duration,
            );
        }
        self.load_sound_group_name(
            read_u32(args, 0)?,
            read_u32(args, 12)? != 0,
            &name,
            read_u32(args, 16)?,
            duration,
        )
    }

    /// Ordinary wrappers build the documented load record, using encoded
    /// completed names and the same explicit buffer contract as native69.
    #[allow(clippy::too_many_arguments)]
    fn load_sound_task_record(
        &mut self,
        task: u32,
        key: u32,
        grouped: bool,
        by_key: bool,
        name: &[u8],
        selector: u32,
        duration: u32,
    ) -> cs2_script::Result<NativeReturn> {
        self.load_sound_task_record_mode(task, key, grouped, by_key, name, selector, duration, 0, 0)
    }

    /// FES load duration is a fade span at offset284. It is distinct from
    /// the ordinary935 front-silence option at offset276.
    pub(super) fn load_sound_name_mode(
        &mut self,
        channel: u32,
        name: &[u8],
        selector: u32,
        mode: i32,
        duration: u32,
    ) -> cs2_script::Result<NativeReturn> {
        if let Some(result) = self.resume_task_sound_load()? { return Ok(result); }
        if channel >= 64 {
            return Ok(NativeReturn::Value(0));
        }
        let task = self.required_sound_task()?;
        self.load_sound_task_record_mode(
            task, channel, false, false, name, selector, 0, mode, duration,
        )
    }

    pub(super) fn load_sound_group_name_mode(
        &mut self,
        group: u32,
        by_key: bool,
        name: &[u8],
        selector: u32,
        mode: i32,
        duration: u32,
    ) -> cs2_script::Result<NativeReturn> {
        if let Some(result) = self.resume_task_sound_load()? { return Ok(result); }
        let task = self.required_sound_task()?;
        self.load_sound_task_record_mode(
            task, group, true, by_key, name, selector, 0, mode, duration,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn load_sound_task_record_mode(
        &mut self,
        task: u32,
        key: u32,
        grouped: bool,
        by_key: bool,
        name: &[u8],
        selector: u32,
        options: u32,
        mode: i32,
        duration: u32,
    ) -> cs2_script::Result<NativeReturn> {
        if let Some(result) = self.resume_task_sound_load()? { return Ok(result); }
        let names = self.sound_resource_names(name)?;
        if !matches!(selector, 1 | 2) {
            return Err(audio_script_error("sound load selector must be 1 or 2"));
        }
        let Some(completed) = names.first() else {
            return Ok(NativeReturn::Value(if grouped { u32::MAX } else { 0 }));
        };
        let name = encode_name(self.text_encoding, completed)?;
        if name.len() >= 260 {
            return Err(audio_script_error(
                "completed sound name exceeds 259 encoded bytes",
            ));
        }
        let mut record = [0_u8; 292];
        let offset = if grouped { 4 } else { 0 };
        record[offset..offset + 4].copy_from_slice(&key.to_le_bytes());
        record[12..12 + name.len()].copy_from_slice(&name);
        record[272..276].copy_from_slice(&u32::from(key >> 16 == 0).to_le_bytes());
        record[276..280].copy_from_slice(&options.to_le_bytes());
        record[280..284].copy_from_slice(&mode.to_le_bytes());
        record[284..288].copy_from_slice(&duration.to_le_bytes());
        let code = if grouped {
            if by_key {
                205 + selector
            } else {
                207 + selector
            }
        } else {
            179 + selector
        };
        self.dispatch_sound_load_record(task, code, &mut record)
    }

    fn resume_plain_sound_load(&mut self) -> cs2_script::Result<Option<NativeReturn>> {
        let Some(saved) = self.data_services.sound_async.loads.get(&self.current_task_id).cloned()
            else { return Ok(None); };
        let result = self.load_sound_candidates(saved.channel, Vec::new(), saved.selector, saved.duration)?;
        let Some(group) = saved.group else { return Ok(Some(result)); };
        Ok(Some(match result {
            NativeReturn::Value(0) => NativeReturn::Value(u32::MAX),
            NativeReturn::Value(_) => {
                let state = self.sound_channels.get_mut(&saved.channel).expect("loaded grouped channel");
                state.looped = group & 0xffff_0000 == 0;
                state.repeat_remaining = 0;
                state.task.grouped = true;
                NativeReturn::Value(saved.channel)
            }
            other => other,
        }))
    }

    pub(super) fn resume_task_sound_load(&mut self) -> cs2_script::Result<Option<NativeReturn>> {
        let Some(saved) = self.data_services.sound_async.task_loads.get(&self.current_task_id).cloned()
            else { return Ok(None); };
        let mut record = saved.record;
        self.dispatch_sound_load_record(saved.task, saved.code, &mut record).map(Some)
    }

    pub(super) fn dispatch_sound_load_record(
        &mut self, task: u32, code: u32, record: &mut [u8],
    ) -> cs2_script::Result<NativeReturn> {
        let mut output = 0;
        if !self.dispatch_sound_task_command(task, TaskCommand {
                target: task as i32, sender: self.current_task_id as i32, code, data: [0; 3],
            }, &mut TaskCommandBuffers { data0: None, data1: Some(record) }, &mut output)? {
            return Err(audio_script_error("sound task did not consume its load record"));
        }
        if self.data_services.sound_async.loads.contains_key(&self.current_task_id)
            || self.data_services.sound_async.task_loads.contains_key(&self.current_task_id) {
            Ok(NativeReturn::Retry)
        } else { Ok(NativeReturn::Value(output)) }
    }

    /// Name is already format-expanded bytes in this host's script encoding.
    /// duration is the load option in milliseconds, not a number of frames.
    /// Successful ordinary channel loading returns Value(1); it does not play.
    pub(super) fn load_sound_name(
        &mut self,
        channel: u32,
        name: &[u8],
        selector: u32,
        duration: u32,
    ) -> cs2_script::Result<NativeReturn> {
        if let Some(result) = self.resume_task_sound_load()? { return Ok(result); }
        if let Some(result) = self.resume_plain_sound_load()? { return Ok(result); }
        let names = self.sound_resource_names(name)?;
        if !matches!(selector, 1 | 2) {
            return Err(audio_script_error("sound load selector must be 1 or 2"));
        }
        if channel >= 64 {
            return Ok(NativeReturn::Value(0));
        }
        if let Some(task) = self.default_sound_task()? {
            return self
                .load_sound_task_record(task, channel, false, false, name, selector, duration);
        }
        self.load_sound_candidates(channel, names, selector, duration)
    }

    /// The JavaScript bridge has no resumable host-call continuation yet.
    /// Preserve its synchronous CsSound.load contract explicitly; VM/FES paths
    /// use the ordinary Retry-returning entry above.
    pub(super) fn load_sound_name_compat(&mut self, channel: u32, name: &[u8], selector: u32, duration: u32)
        -> cs2_script::Result<NativeReturn>
    {
        loop {
            let result = self.load_sound_name(channel, name, selector, duration)?;
            if result != NativeReturn::Retry { return Ok(result); }
            self.complete_sound_material_compat();
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    pub(super) fn load_sound_group_name_compat(&mut self, group: u32, by_key: bool, name: &[u8], selector: u32, duration: u32)
        -> cs2_script::Result<NativeReturn>
    {
        loop {
            let result = self.load_sound_group_name(group, by_key, name, selector, duration)?;
            if result != NativeReturn::Retry { return Ok(result); }
            self.complete_sound_material_compat();
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    fn complete_sound_material_compat(&mut self) {
        let owner = self.current_task_id;
        let Some(mut saved) = self.data_services.sound_async.loads.get(&owner).cloned() else { return; };
        if saved.material.is_some() { return; }
        let Some(source) = saved.source.as_ref() else { return; };
        let cached = self.data_services.ordinary.cached_resource_with_source(&source.resource)
            .filter(|cached| cached.source == source.candidate).map(|cached| cached.bytes.as_slice());
        saved.material = Some(material::prepare_compat(self.resources, source, cached));
        self.data_services.sound_async.loads.insert(owner, saved);
    }

    /// Kind13 records already carry category-completed virtual names. Do not
    /// prepend another category or choose a different archive a second time.
    pub(super) fn load_sound_resolved_name(
        &mut self,
        channel: u32,
        name: &[u8],
        selector: u32,
        duration: u32,
    ) -> cs2_script::Result<NativeReturn> {
        let name = trim_nul(name);
        if name.len() >= 260 {
            return Err(audio_script_error(
                "sound record name exceeds 259 encoded bytes",
            ));
        }
        let name = decode_name(self.text_encoding, name)?;
        self.load_sound_candidates(channel, vec![name], selector, duration)
    }

    fn load_sound_candidates(
        &mut self,
        channel: u32,
        names: Vec<String>,
        selector: u32,
        duration: u32,
    ) -> cs2_script::Result<NativeReturn> {
        if !matches!(selector, 1 | 2) {
            return Err(audio_script_error("native 935 selector must be 1 or 2"));
        }
        if channel >= 64 {
            return Ok(NativeReturn::Value(0));
        }
        let owner = self.current_task_id;
        let saved = self.data_services.sound_async.loads.get(&owner).cloned();
        let mut saved = if let Some(saved) = saved {
            saved
        } else {
            if self.data_services.sound_async.cancelled.remove(&owner) { return Ok(NativeReturn::Value(0)); }
            if self.data_services.fast_forward && names.iter().all(|name| !self.sound_is_bgm(name)) {
                crate::perf_detail::audio_async_note(&format!("event=fast_forward_skip owner={owner} channel={channel} candidates={names:?}"));
                return Ok(NativeReturn::Value(0));
            }
            self.cancel_audio_channel_start(channel)?;
            let mut state = self.sound_channels.remove(&channel).unwrap_or_default();
            let saved = PendingSoundLoad { channel, names, selector, duration, group: None, source: None, material: None };
            if let Some(handle) = state.handle {
                match self.audio_host.begin_stop(handle) {
                    Ok(AudioControl::Ready) => { state.handle = None; }
                    Ok(AudioControl::Pending(ticket)) => {
                        self.data_services.sound_async.loads.insert(owner, saved);
                        state.pending_start = Some(crate::sound_async::PendingSoundStart {
                            started: std::time::Instant::now(), owner, index: 935, value: 0,
                            configured: None, transition: None, operation: crate::sound_async::Operation::StopForLoad(ticket),
                        });
                        self.sound_channels.insert(channel, state);
                        return Ok(NativeReturn::Retry);
                    }
                    Err(cause) => { self.sound_channels.insert(channel, state); return Err(self.sound_host_error(cause)); }
                }
            }
            self.sound_channels.insert(channel, state);
            saved
        };
        let channel = saved.channel;
        if let Some(crate::sound_async::Operation::StopForLoad(ticket)) = self.sound_channels.get(&channel)
            .and_then(|state| state.pending_start.as_ref()).map(|pending| pending.operation.clone())
        {
            match self.audio_host.poll_control(ticket) {
                Ok(None) => return Ok(NativeReturn::Retry),
                Err(cause) => {
                    self.data_services.sound_async.loads.remove(&owner);
                    self.data_services.sound_async.task_loads.remove(&owner);
                    if let Some(state) = self.sound_channels.get_mut(&channel) { state.pending_start = None; state.handle = None; }
                    return Err(self.sound_host_error(cause));
                }
                Ok(Some(())) => {}
            }
            if let Some(state) = self.sound_channels.get_mut(&channel) { state.pending_start = None; state.handle = None; }
        }
        if saved.source.is_none() {
            let mut state = self.sound_channels.remove(&channel).unwrap_or_default();
            state.release();
            self.sound_channels.insert(channel, state);
            let mut source = None;
            for name in &saved.names {
                match self.resources.resolve_mode_utf16(&name.encode_utf16().collect::<Vec<_>>(),
                    cs2_runtime::resource_router::ResourceMode::LooseFirst)
                {
                    Ok(Some(candidate)) => { source = Some((name.clone(), candidate.value.clone())); break; }
                    Ok(None) => {}
                    Err(cause) => {
                        self.data_services.sound_async.loads.remove(&owner);
                        self.note_sound_resource_failure("resolve", name, cause);
                        return Ok(NativeReturn::Value(0));
                    }
                }
            }
            let Some((resource, candidate)) = source else {
                self.data_services.sound_async.loads.remove(&owner);
                return Ok(NativeReturn::Value(0));
            };
            if self.data_services.fast_forward && !self.sound_is_bgm(&resource) {
                self.data_services.sound_async.loads.remove(&owner);
                crate::perf_detail::audio_async_note(&format!("event=fast_forward_skip owner={owner} channel={channel} resource={resource:?}"));
                return Ok(NativeReturn::Value(0));
            }
            let Some(version) = cs2_runtime::emote::EmoteArchiveVersion::observe(&candidate) else {
                self.data_services.sound_async.loads.remove(&owner);
                self.note_sound_resource_failure("load", &resource, "sound material source is unavailable");
                return Ok(NativeReturn::Value(0));
            };
            let sidecars = self.sound_sidecar_request(&resource).map_err(|cause| cause.to_string());
            saved.source = Some(Arc::new(material::Source { resource: resource.clone(), candidate, sidecars, version }));
            let state = self.sound_channels.get_mut(&channel).unwrap();
            state.pending_start = Some(crate::sound_async::PendingSoundStart {
                started: std::time::Instant::now(), owner, index: 935, value: 0,
                configured: None, transition: None, operation: crate::sound_async::Operation::PrepareLoad,
            });
            crate::perf_detail::audio_async_note(&format!("event=material_queued owner={owner} channel={channel} resource={resource:?}"));
        }
        let source = saved.source.as_ref().unwrap().clone();
        if self.data_services.fast_forward && !self.sound_is_bgm(&source.resource) {
            self.data_services.sound_async.loads.remove(&owner);
            self.sound_channels.get_mut(&channel).unwrap().pending_start = None;
            return Ok(NativeReturn::Value(0));
        }
        let started = self.sound_channels[&channel].pending_start.as_ref()
            .filter(|pending| pending.owner == owner).map(|pending| pending.started)
            .ok_or_else(|| audio_script_error("pending sound material lost its channel reservation"))?;
        let ready = saved.material.as_ref().and_then(|ticket| ticket.poll());
        if ready.is_none() && started.elapsed() > material::TIMEOUT {
            self.data_services.sound_async.loads.remove(&owner);
            self.sound_channels.get_mut(&channel).unwrap().pending_start = None;
            crate::perf_detail::audio_async_note(&format!("event=material_timeout owner={owner} channel={channel} pending_wall_us={}", started.elapsed().as_micros()));
            self.note_sound_resource_failure("load", &source.resource, "sound material preparation timed out");
            return Ok(NativeReturn::Value(0));
        }
        if saved.material.is_none() {
            let resources = self.data_services.sound_async.resources.get_or_insert_with(|| Arc::new(self.resources.clone())).clone();
            let cached = self.data_services.ordinary.cached_resource_with_source(&source.resource)
                .filter(|cached| cached.source == source.candidate).map(|cached| cached.bytes.as_slice());
            match material::begin(resources, source.clone(), cached) {
                Ok(ticket) => saved.material = ticket,
                Err(cause) => {
                    self.data_services.sound_async.loads.remove(&owner);
                    self.sound_channels.get_mut(&channel).unwrap().pending_start = None;
                    self.note_sound_resource_failure("load", &source.resource, cause);
                    return Ok(NativeReturn::Value(0));
                }
            }
            // Even a tiny cached clip yields the native once at submission.
            // Its next entry owns completion and the following VM instruction.
            self.data_services.sound_async.loads.insert(owner, saved);
            return Ok(NativeReturn::Retry);
        }
        let Some(ready) = ready else {
            self.data_services.sound_async.loads.insert(owner, saved);
            return Ok(NativeReturn::Retry);
        };
        self.data_services.sound_async.loads.remove(&owner);
        let mut state = self.sound_channels.remove(&channel).unwrap_or_default();
        state.pending_start = None;
        let ready = match ready {
            Ok(ready) => ready,
            Err(cause) => {
                self.sound_channels.insert(channel, state);
                self.note_sound_resource_failure("load", &source.resource, cause);
                crate::perf_detail::audio_async_note(&format!("event=material_failed owner={owner} channel={channel} pending_wall_us={}", started.elapsed().as_micros()));
                return Ok(NativeReturn::Value(0));
            }
        };
        let committing = std::time::Instant::now();
        let resource = source.resource.clone();
        if cs2_runtime::emote::EmoteArchiveVersion::observe(&source.candidate).as_ref() != Some(&source.version) {
            self.sound_channels.insert(channel, state);
            self.note_sound_resource_failure("load", &resource, "sound material source changed before installation");
            return Ok(NativeReturn::Value(0));
        }
        let prepared = ready.prepared;
        let selector = saved.selector;
        let duration = saved.duration;
        state.resource = Some(resource.clone());
        state.prepared_audio = Some(prepared.clone());
        state.task.resource_id = next_sound_resource_identity()?;
        state.task.owner = self.single_sound_task_owner();
        state.source_query_loaded();
        state.volume = 100;
        state.pan = 0;
        state.looped = channel & 0xffff_0000 == 0;
        state.selector = selector;
        state.duration_frames = duration;
        state.task.whole_buffer = selector == 2
            || prepared.probe.frames
                + u64::from(prepared.probe.sample_rate) * u64::from(duration) / 1000
                <= u64::from(prepared.probe.sample_rate) * 4;
        state.volume_fade = None;
        match ready.sidecars {
            Ok(sidecars) => match self.validate_sound_sidecar_install(&sidecars) {
                Ok(()) => state.task.sidecars = sidecars,
                Err(cause) => self.note_sound_resource_failure("sidecar", &resource, cause),
            },
            Err(cause) => self.note_sound_resource_failure("sidecar", &resource, cause),
        }
        self.sound_channels.insert(channel, state);
        if let Some(event) = self.events.last_mut() {
            event.detail.push_str(&format!(
                " sound_load channel={channel} selector={selector} resource={resource:?} frames={} rate={} channels={} codec={:?}",
                prepared.probe.frames, prepared.probe.sample_rate, prepared.probe.channels, prepared.probe.codec));
        }
        crate::perf_detail::audio_async_note(&format!(
            "event=material_ready owner={owner} channel={channel} material_read_us={} material_prepare_us={} sidecar_us={} background_wall_us={} framecommit_us={} pending_wall_us={}",
            ready.read_us, ready.prepare_us, ready.sidecar_us, ready.background_wall_us,
            committing.elapsed().as_micros(), started.elapsed().as_micros()));
        Ok(NativeReturn::Value(1))
    }

    /// Successful grouped loading returns the actual channel number, or MAX on
    /// a missing group/source. It uses the same PCM preparation as native 935.
    pub(super) fn load_sound_group_name(
        &mut self,
        group: u32,
        by_key: bool,
        name: &[u8],
        selector: u32,
        duration: u32,
    ) -> cs2_script::Result<NativeReturn> {
        if let Some(result) = self.resume_task_sound_load()? { return Ok(result); }
        if let Some(result) = self.resume_plain_sound_load()? { return Ok(result); }
        // Name resolution occurs before selector validation and group lookup.
        let names = self.sound_resource_names(name)?;
        if !matches!(selector, 1 | 2) {
            return Err(audio_script_error(
                "native 936 selector has no defined return value",
            ));
        }
        if self.data_services.fast_forward && names.iter().all(|name| !self.sound_is_bgm(name)) {
            return Ok(NativeReturn::Value(u32::MAX));
        }
        if let Some(task) = self.default_sound_task()? {
            return self
                .load_sound_task_record(task, group, true, by_key, name, selector, duration);
        }
        let channel = if let Some(saved) = self.data_services.sound_async.loads.get(&self.current_task_id) {
            saved.channel
        } else {
            let Some(channel) = self.select_sound_group_channel(group, by_key)? else {
                return Ok(NativeReturn::Value(u32::MAX));
            };
            channel
        };
        match self.load_sound_name(channel, name, selector, duration)? {
            NativeReturn::Retry => {
                if let Some(saved) = self.data_services.sound_async.loads.get_mut(&self.current_task_id) { saved.group = Some(group); }
                Ok(NativeReturn::Retry)
            }
            NativeReturn::Value(0) => Ok(NativeReturn::Value(u32::MAX)),
            NativeReturn::Value(_) => {
                let state = self
                    .sound_channels
                    .get_mut(&channel)
                    .expect("loaded sound channel");
                state.looped = group & 0xffff_0000 == 0;
                state.repeat_remaining = 0;
                state.task.grouped = true;
                Ok(NativeReturn::Value(channel))
            }
            _ => Err(audio_script_error(
                "sound preparation did not return a channel result",
            )),
        }
    }

    /// Explicit allocation helper, never implicitly used for CHANNEL=-1.
    /// A playing or paused device owns its physical channel.
    pub(super) fn select_free_sound_channel(&mut self) -> cs2_script::Result<u32> {
        for channel in 0..64 {
            if self.sound_channels.get(&channel).is_some_and(|state| state.pending_start.is_some())
                || self.data_services.sound_async.task_loads.values().any(|load| load.logical == channel) { continue; }
            let handle = self
                .sound_channels
                .get(&channel)
                .and_then(|state| state.handle);
            let active = match handle {
                Some(handle) => self
                    .audio_host
                    .is_active(handle)
                    .map_err(|cause| self.sound_host_error(cause))?,
                None => false,
            };
            if !active {
                if let Some(state) = self.sound_channels.get_mut(&channel) {
                    state.handle = None;
                    state.paused = false;
                }
                return Ok(channel);
            }
        }
        Err(audio_script_error(
            "all 64 physical sound channels are active",
        ))
    }

    pub(super) fn sound_duration_millis(&self, channel: u32) -> u32 {
        let channel = self.sound_selected_channel(channel);
        if channel >= 64 {
            return 0;
        }
        self.sound_channels
            .get(&channel)
            .and_then(|state| state.prepared_audio.as_ref())
            .map(|audio| {
                ((audio.probe.frames as f64 / f64::from(audio.probe.sample_rate) * 1000.).trunc()
                    as u64) as u32
            })
            .unwrap_or(0)
    }

    fn sound_resource_names(&mut self, bytes: &[u8]) -> cs2_script::Result<Vec<String>> {
        let bytes = trim_nul(bytes);
        if bytes.len() >= 260 {
            return Err(audio_script_error(
                "expanded sound name exceeds 259 encoded bytes",
            ));
        }
        let (name, encoded) = match decode_name(self.text_encoding, bytes) {
            Ok(name) => (name, Cow::Borrowed(bytes)),
            Err(cause) => {
                // Unicode CST readers can pass a UTF-8 command argument to
                // a narrow sound wrapper. Its resource name still has to fit
                // the narrow loader's filename record and category matcher.
                let Ok(name) = std::str::from_utf8(bytes) else {
                    return Err(cause);
                };
                let Ok(encoded) = encode_name(self.text_encoding, name) else {
                    return Ok(Vec::new());
                };
                (name.to_owned(), Cow::Owned(encoded))
            }
        };
        let names = if name.rfind(['/', '\\']).is_some_and(|offset| offset > 0) {
            vec![name.clone()]
        } else {
            let category = if self.sound_config_enabled != 0 {
                self.sound_config.file_part(&encoded).map(|part| part.archive.clone())
            } else {
                self.legacy_sound_archive_name(&encoded)?
            };
            let Some(category) = category else { return Ok(Vec::new()); };
            self.media_category_candidates(&category, &name)?
                .unwrap_or_default()
        };
        for name in &names {
            if encoded_name_length(self.text_encoding, name)? >= 260 {
                return Err(audio_script_error(
                    "completed sound name exceeds 259 encoded bytes",
                ));
            }
        }
        Ok(names)
    }

    fn note_sound_resource_failure(
        &mut self,
        operation: &str,
        resource: &str,
        cause: impl std::fmt::Display,
    ) {
        let detail = format!("sound_{operation}_failed resource={resource:?} error={cause:#}");
        if let Some(event) = self.events.last_mut() {
            event.detail.push(' ');
            event.detail.push_str(&detail);
        }
        #[cfg(target_os = "android")]
        crate::audio_android::report_audio(&detail);
    }

    pub(super) fn sound_host_error(
        &mut self,
        cause: impl std::fmt::Display,
    ) -> cs2_script::ScriptError {
        let cause = format!("{cause:#}");
        if let Some(event) = self.events.last_mut() {
            event.detail.push_str(&format!(" sound_host_error={cause}"));
        }
        crate::native_state_error_owned(format!("sound host failed: {cause}"))
    }
}

pub(super) fn decode_name(
    encoding: ScriptTextEncoding,
    bytes: &[u8],
) -> cs2_script::Result<String> {
    if encoding == ScriptTextEncoding::Utf8 {
        return std::str::from_utf8(bytes)
            .map(str::to_owned)
            .map_err(|_| audio_script_error("sound name contains invalid UTF-8"));
    }
    let codec = if encoding == ScriptTextEncoding::Gbk {
        encoding_rs::GBK
    } else {
        encoding_rs::SHIFT_JIS
    };
    let (name, _, invalid) = codec.decode(bytes);
    if invalid {
        return Err(audio_script_error(
            "sound name is invalid in the declared script encoding",
        ));
    }
    Ok(name.into_owned())
}

fn encoded_name_length(encoding: ScriptTextEncoding, name: &str) -> cs2_script::Result<usize> {
    Ok(encode_name(encoding, name)?.len())
}

pub(super) fn encode_name(encoding: ScriptTextEncoding, name: &str) -> cs2_script::Result<Vec<u8>> {
    if encoding == ScriptTextEncoding::Utf8 {
        return Ok(name.as_bytes().to_vec());
    }
    let codec = if encoding == ScriptTextEncoding::Gbk {
        encoding_rs::GBK
    } else {
        encoding_rs::SHIFT_JIS
    };
    let (bytes, _, invalid) = codec.encode(name);
    if invalid {
        return Err(audio_script_error(
            "completed sound name cannot be represented in the script encoding",
        ));
    }
    Ok(bytes.into_owned())
}

#[cfg(test)]
pub(super) fn gate_sound_material_for_test(resource: &str)
    -> (std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>)
{
    async_load_tests::install_material_gate(resource)
}

#[cfg(test)]
mod async_load_tests {
    use super::*;
    use crate::{HeadlessPlayer, SoundChannelState};
    use cs2_runtime::{AudioChannel, AudioHost, AudioRequest, AudioStart};
    use std::{path::PathBuf, sync::mpsc, time::{Duration, Instant}};

    struct MaterialGate {
        release: std::sync::Mutex<mpsc::Receiver<()>>,
        entered: Option<mpsc::Sender<()>>,
        timeout: Duration,
    }
    fn material_gate(release: mpsc::Receiver<()>) -> Arc<MaterialGate> {
        Arc::new(MaterialGate { release: std::sync::Mutex::new(release), entered: None, timeout: Duration::from_secs(2) })
    }
    type MaterialGates = std::collections::BTreeMap<String, Arc<MaterialGate>>;
    static MATERIAL_DELAYS: std::sync::LazyLock<std::sync::Mutex<MaterialGates>> =
        std::sync::LazyLock::new(Default::default);
    pub(super) fn install_material_gate(resource: &str) -> (mpsc::Sender<()>, mpsc::Receiver<()>) {
        let (release, gate) = mpsc::channel();
        let (entered, notification) = mpsc::channel();
        MATERIAL_DELAYS.lock().unwrap().insert(resource.to_owned(), Arc::new(MaterialGate {
            release: std::sync::Mutex::new(gate), entered: Some(entered), timeout: Duration::from_secs(10),
        }));
        (release, notification)
    }
    pub(super) fn delay_material(resource: &str) {
        let resource = resource.replace('\\', "/");
        let gate = MATERIAL_DELAYS.lock().unwrap().iter().find_map(|(path, gate)|
            path.replace('\\', "/").eq_ignore_ascii_case(&resource).then(|| gate.clone()));
        if let Some(gate) = gate {
            if let Some(entered) = &gate.entered { let _ = entered.send(()); }
            let _ = gate.release.lock().unwrap().recv_timeout(gate.timeout);
        }
    }

    #[test]
    fn native935_cold_material_wait_keeps_frames_siblings_and_instruction_order() {
        exercise_cold_material_wait(false);
    }

    #[test]
    fn native935_preserves_the_last_canvas_during_list_setup() {
        exercise_cold_material_wait(true);
    }

    fn exercise_cold_material_wait(register_image: bool) {
        use cs2_script::{KcsProgram, Operand};
        let (_scratch, mut player, cleanup) = player();
        // Cold, unique name; cache warmup for replacement.wav must not satisfy it.
        let name = format!("sound/cold-material-{}.wav", _scratch.0.file_name().unwrap().to_string_lossy());
        std::fs::write(_scratch.0.join(&name), wave()).unwrap();
        // Reindex the new real source, retaining this test's routing.
        let router = player.scan.resources.resource_router().cloned();
        player.scan.resources = cs2_runtime::ResourceIndex::scan(&_scratch.0).unwrap();
        if let Some(router) = router { player.scan.resources.set_resource_router(router).unwrap(); }
        player.data_services.sound_async.resources = Some(Arc::new(player.scan.resources.clone()));
        let (material_ready, gate) = mpsc::channel();
        let (entered, material_entered) = mpsc::channel();
        MATERIAL_DELAYS.lock().unwrap().insert(_scratch.0.join(&name).to_string_lossy().into_owned(),
            Arc::new(MaterialGate { release: std::sync::Mutex::new(gate), entered: Some(entered), timeout: Duration::from_secs(2) }));
        let mut code = Vec::new();
        player.render_scene().unwrap();
        let old_pixel = player.canvas.pixel(0, 0).unwrap();
        let new_pixel = cs2_renderer::Color::rgb(25, 95, 175);
        player.with_host(|host| {
            let plane = host.create_task_object(2, 0, b"presentation probe", b"")?;
            let node = host.scene_nodes.get_mut(&plane).unwrap();
            node.size = [8., 8.];
            node.position = [0., 0., 5000.];
            node.color = 0xff19_5faf;
            node.visible = true;
            node.initialized = true;
            Ok(())
        }).unwrap();
        let push = |code: &mut Vec<u8>, value| {
            let operand = Operand::immediate(value);
            code.push(operand.flags.bits()); code.extend_from_slice(&operand.payload.to_le_bytes());
        };
        if register_image {
            let (list, wrapper) = player.with_host(|host| {
                let list = host.create_task_object(15, host.current_task_id, b"image list", b"")?;
                host.initialize_fes_image_list(list, 16, 16)?;
                let image = cs2_renderer::RgbaFrame::from_rgba8(1, 1, vec![240, 80, 32, 255])?;
                Ok((list, host.register_image_wrapper(image, false)?))
            }).unwrap();
            for value in [list, 70, wrapper, 4f32.to_bits(), 4f32.to_bits(), 0, 0, 0,
                1f32.to_bits(), 1f32.to_bits(), 255, 1, 0] {
                code.push(0x25);
                push(&mut code, value);
            }
            code.push(0x2b);
            push(&mut code, 257);
            push(&mut code, 52);
        }
        for value in [8, 0, 64, 1, 0] {
            code.push(0x25);
            push(&mut code, value);
        }
        let sound_pc = code.len();
        code.push(0x2b);
        push(&mut code, 935);
        push(&mut code, 20);
        code.push(0x3b);
        let mut vm = Vm::from_program(KcsProgram::from_code(&code, vec![0; 512], 64).unwrap()).unwrap();
        vm.write(64, format!("{name}\0").as_bytes()).unwrap();
        player.data_services.tasks.set_factory_external(vm.external_binding());
        player.data_services.tasks.register_external_task(player.current_task_id, 0,
            &player.data_services.language.languages).unwrap();
        player.vm = Some(vm);
        let sibling = player.with_host(|host| Ok(host.create_unloaded_ordinary_task(0, b"")?)).unwrap();
        assert!(player.advance_host_frame_with_retries(1000).unwrap());
        let entered_deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match material_entered.try_recv() {
                Ok(()) => break,
                Err(mpsc::TryRecvError::Disconnected) => panic!("the actual material worker never entered its gate"),
                Err(mpsc::TryRecvError::Empty) => {}
            }
            assert!(Instant::now() < entered_deadline, "the actual material worker must acknowledge entry");
            assert!(player.advance_host_frame_with_retries(1000).unwrap());
            assert_eq!(player.vm.as_ref().unwrap().instruction_pointer, sound_pc);
            std::thread::yield_now();
        }
        let frame = player.state.clock.frame;
        let start = Instant::now();
        let mut max_frame = Duration::ZERO;
        for _ in 0..4 {
            let frame_started = Instant::now();
            assert!(player.advance_host_frame_with_retries(1000).unwrap());
            max_frame = max_frame.max(frame_started.elapsed());
            assert_eq!(player.vm.as_ref().unwrap().instruction_pointer, sound_pc,
                "native935 must keep the following opcode ordered behind material completion");
            assert!(player.sound_channels.get(&8).is_none_or(|state| state.prepared_audio.is_none()));
            assert_eq!(player.canvas.pixel(0, 0).unwrap(), if register_image { old_pixel } else { new_pixel });
        }
        assert_eq!(player.state.clock.frame, frame + 4);
        assert!(player.data_services.tasks.task_clock(sibling).unwrap().current > 0);
        eprintln!("async_sound_material gated_frames=4 wall_us={} max_frame_us={}", start.elapsed().as_micros(), max_frame.as_micros());
        material_ready.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while player.vm.as_ref().unwrap().instruction_pointer == sound_pc {
            assert!(Instant::now() < deadline);
            player.advance_host_frame_with_retries(1000).unwrap();
            std::thread::yield_now();
        }
        assert_eq!(player.sound_channels[&8].resource.as_deref(), Some(name.as_str()));
        assert!(player.sound_channels[&8].prepared_audio.is_some());
        assert_eq!(player.canvas.pixel(0, 0).unwrap(), new_pixel);
        MATERIAL_DELAYS.lock().unwrap().remove(&_scratch.0.join(&name).to_string_lossy().into_owned());
        let _ = cleanup.send(Ok(()));
    }

    #[test]
    fn async_material_cancellation_never_installs_a_late_result() {
        let (_scratch, mut player, cleanup) = player();
        let (material_ready, gate) = mpsc::channel();
        MATERIAL_DELAYS.lock().unwrap().insert(_scratch.0.join("sound/replacement.wav").to_string_lossy().into_owned(), material_gate(gate));
        let result = player.with_host(|host| Ok(host.load_sound_resolved_name(8, b"sound/replacement.wav", 1, 0)?)).unwrap();
        assert_eq!(result, NativeReturn::Retry);
        player.with_host(|host| Ok(host.cancel_audio_channel_start(8)?)).unwrap();
        assert!(player.data_services.sound_async.loads.is_empty());
        let _ = material_ready.send(());
        assert_eq!(player.with_host(|host| Ok(host.load_sound_resolved_name(8, b"sound/replacement.wav", 1, 0)?)).unwrap(), NativeReturn::Value(0));
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let result = player.with_host(|host| Ok(host.load_sound_resolved_name(8, b"sound/second.wav", 2, 13)?)).unwrap();
            if result != NativeReturn::Retry { assert_eq!(result, NativeReturn::Value(1)); break; }
            assert!(Instant::now() < deadline); std::thread::yield_now();
        }
        assert_eq!(player.sound_channels[&8].resource.as_deref(), Some("sound/second.wav"));
        assert_eq!(player.sound_channels[&8].selector, 2);
        let _ = cleanup.send(Ok(()));
    }

    #[test]
    fn async_material_timeout_and_decode_failure_leave_no_pending_load() {
        let (_scratch, mut player, cleanup) = player();
        let (material_ready, gate) = mpsc::channel();
        MATERIAL_DELAYS.lock().unwrap().insert(_scratch.0.join("sound/replacement.wav").to_string_lossy().into_owned(), material_gate(gate));
        assert_eq!(player.with_host(|host| Ok(host.load_sound_resolved_name(8, b"sound/replacement.wav", 1, 0)?)).unwrap(), NativeReturn::Retry);
        player.sound_channels.get_mut(&8).unwrap().pending_start.as_mut().unwrap().started =
            Instant::now() - Duration::from_secs(31);
        assert_eq!(player.with_host(|host| Ok(host.load_sound_resolved_name(8, b"sound/replacement.wav", 1, 0)?)).unwrap(), NativeReturn::Value(0));
        assert!(player.data_services.sound_async.loads.is_empty());
        assert!(player.sound_channels[&8].pending_start.is_none());
        let _ = material_ready.send(());
        std::fs::write(_scratch.0.join("sound/second.wav"), b"malformed wave").unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let result = player.with_host(|host| Ok(host.load_sound_resolved_name(8, b"sound/second.wav", 1, 0)?)).unwrap();
            if result != NativeReturn::Retry { assert_eq!(result, NativeReturn::Value(0)); break; }
            assert!(Instant::now() < deadline); std::thread::yield_now();
        }
        assert!(player.sound_channels[&8].prepared_audio.is_none());
        assert!(player.sound_channels[&8].pending_start.is_none());
        let _ = cleanup.send(Ok(()));
    }

    #[test]
    fn async_material_rejects_a_source_replaced_while_preparation_is_pending() {
        let (_scratch, mut player, cleanup) = player();
        let (material_ready, gate) = mpsc::channel();
        MATERIAL_DELAYS.lock().unwrap().insert(_scratch.0.join("sound/replacement.wav").to_string_lossy().into_owned(), material_gate(gate));
        assert_eq!(player.with_host(|host| Ok(host.load_sound_resolved_name(8, b"sound/replacement.wav", 1, 0)?)).unwrap(), NativeReturn::Retry);
        std::fs::write(_scratch.0.join("sound/replacement.wav"), b"a replaced and invalid source of different length").unwrap();
        let _ = material_ready.send(());
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let result = player.with_host(|host| Ok(host.load_sound_resolved_name(8, b"sound/replacement.wav", 1, 0)?)).unwrap();
            if result != NativeReturn::Retry { assert_eq!(result, NativeReturn::Value(0)); break; }
            assert!(Instant::now() < deadline); std::thread::yield_now();
        }
        assert!(player.sound_channels[&8].prepared_audio.is_none());
        assert!(player.sound_channels[&8].pending_start.is_none());
        let _ = cleanup.send(Ok(()));
    }

    #[test]
    fn javascript_compat_load_finishes_while_other_material_owners_are_suspended() {
        let (_scratch, mut player, cleanup) = player();
        let root = player.current_task_id;
        let caller = player.with_host(|host| Ok(host.create_unloaded_ordinary_task(root, b"")?)).unwrap();
        let javascript = player.with_host(|host| Ok(host.create_unloaded_ordinary_task(root, b"")?)).unwrap();
        let (first_ready, first_gate) = mpsc::channel();
        let (second_ready, second_gate) = mpsc::channel();
        MATERIAL_DELAYS.lock().unwrap().insert(_scratch.0.join("sound/replacement.wav").to_string_lossy().into_owned(), material_gate(first_gate));
        MATERIAL_DELAYS.lock().unwrap().insert(_scratch.0.join("sound/second.wav").to_string_lossy().into_owned(), material_gate(second_gate));
        assert_eq!(player.with_host(|host| Ok(host.load_sound_resolved_name(8, b"sound/replacement.wav", 1, 0)?)).unwrap(), NativeReturn::Retry);
        player.with_host(|host| { host.current_task_id = caller;
            assert_eq!(host.load_sound_resolved_name(9, b"sound/second.wav", 1, 0)?, NativeReturn::Retry);
            Ok(())
        }).unwrap();
        let start = Instant::now();
        player.with_host(|host| { host.current_task_id = javascript;
            assert_eq!(host.load_sound_name_compat(10, b"sound/compat.wav", 1, 0)?, NativeReturn::Value(1));
            Ok(())
        }).unwrap();
        assert_eq!(player.sound_channels[&10].resource.as_deref(), Some("sound/compat.wav"));
        assert!(player.data_services.sound_async.loads.contains_key(&root));
        assert!(player.data_services.sound_async.loads.contains_key(&caller));
        eprintln!("javascript_material_compat other_owners_pending=2 wall_us={}", start.elapsed().as_micros());
        player.with_host(|host| { host.cancel_audio_channel_start(8)?; host.cancel_audio_channel_start(9)?; Ok(()) }).unwrap();
        let _ = first_ready.send(()); let _ = second_ready.send(());
        let _ = cleanup.send(Ok(()));
    }

    #[test]
    fn grouped_load_allocation_reserves_channels_owned_by_pending_commands() {
        let (_scratch, mut player, cleanup) = player();
        player.sound_groups.push(crate::SoundGroup { key: 17, id: 0, label: b"effects".to_vec(), range: [8, 9] });
        for channel in [8, 9] {
            if channel == 9 {
                assert_eq!(player.with_host(|host| Ok(host.select_sound_group_channel(17, true)?)).unwrap(), Some(9));
            }
            player.sound_channels.insert(channel, SoundChannelState {
                pending_start: Some(crate::sound_async::PendingSoundStart {
                    started: Instant::now(), owner: channel, index: 157, value: 1,
                    configured: None, transition: None, operation: crate::sound_async::Operation::Start(0),
                }), ..Default::default()
            });
        }
        assert_eq!(player.with_host(|host| Ok(host.select_sound_group_channel(17, true)?)).unwrap(), None);
        // Dummy tickets represent reservation only; do not send them to a real backend.
        for channel in [8, 9] { player.sound_channels.get_mut(&channel).unwrap().pending_start = None; }
        let _ = cleanup.send(Ok(()));
    }

    #[test]
    fn async_group_material_commands_keep_separate_physical_owners() {
        let (_scratch, mut player, cleanup) = player();
        player.sound_groups.push(crate::SoundGroup { key: 17, id: 0, label: b"effects".to_vec(), range: [8, 9] });
        let root = player.current_task_id;
        let task = player.with_host(|host| Ok(host.create_task_object(13, root, b"sound", b"")?)).unwrap();
        let caller = player.with_host(|host| Ok(host.create_unloaded_ordinary_task(root, b"")?)).unwrap();
        let mut record = [0_u8; 292];
        record[4..8].copy_from_slice(&17_u32.to_le_bytes());
        record[12..33].copy_from_slice(b"sound/replacement.wav");
        assert_eq!(player.with_host(|host| Ok(host.dispatch_sound_load_record(task, 206, &mut record)?)).unwrap(), NativeReturn::Retry);
        player.with_host(|host| { host.current_task_id = caller;
            assert_eq!(host.dispatch_sound_load_record(task, 206, &mut record)?, NativeReturn::Retry);
            Ok(())
        }).unwrap();
        assert_eq!(player.data_services.sound_async.task_loads[&root].target, 8);
        assert_eq!(player.data_services.sound_async.task_loads[&caller].target, 9);
        let deadline = Instant::now() + Duration::from_secs(2);
        for owner in [root, caller] {
            loop {
                let result = player.with_host(|host| { host.current_task_id = owner;
                    Ok(host.dispatch_sound_load_record(task, 206, &mut record)?) }).unwrap();
                if result != NativeReturn::Retry { assert_eq!(result, NativeReturn::Value(if owner == root { 8 } else { 9 })); break; }
                assert!(Instant::now() < deadline); std::thread::yield_now();
            }
        }
        assert!(player.data_services.sound_async.cancelled.is_empty());
        assert_eq!(player.sound_channels[&8].task.owner, Some(task));
        assert_eq!(player.sound_channels[&9].task.owner, Some(task));
        let _ = cleanup.send(Ok(()));
    }

    #[test]
    fn legacy_sound_prepare_waits_for_material_before_returning() {
        use cs2_script::{Host, KcsProgram};

        let (_scratch, mut player, cleanup) = player();
        let root = player.current_task_id;
        player.sound_groups.push(crate::SoundGroup { key: 17, id: 0, label: b"effects".to_vec(), range: [9, 9] });
        let sound = player.with_host(|host| Ok(host.create_task_object(13, root, b"sound", b"")?)).unwrap();
        let mut vm = Vm::from_program(KcsProgram::from_code(&[0x3b], vec![0; 512], 64).unwrap()).unwrap();
        vm.write(64, b"sound/replacement.wav\0").unwrap();

        for (index, words, channel, expected) in [
            (155, &[8_u32, 0, 64, 1][..], 8, 1),
            (604, &[17, 0, 64, 1, 1][..], 9, 9),
        ] {
            let args = words.iter().copied().flat_map(u32::to_le_bytes).collect::<Vec<_>>();
            let result = player.with_host(|host| Ok(host.call(index, &args, &mut vm)?)).unwrap();
            assert_eq!(result, NativeReturn::Retry, "native {index} returned before loading");
            assert_eq!(player.data_services.sound_async.task_loads[&root].task, sound);

            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                let result = player.with_host(|host| Ok(host.call(index, &args, &mut vm)?)).unwrap();
                if result != NativeReturn::Retry {
                    assert_eq!(result, NativeReturn::Value(expected));
                    break;
                }
                assert!(Instant::now() < deadline, "native {index} did not finish loading");
                std::thread::yield_now();
            }
            assert_eq!(player.sound_channels[&channel].resource.as_deref(), Some("sound/replacement.wav"));
            assert!(player.data_services.sound_async.loads.is_empty());
            assert!(player.data_services.sound_async.task_loads.is_empty());
        }
        let _ = cleanup.send(Ok(()));
    }

    #[test]
    fn native936_cancelled_retry_never_reclaims_a_new_owners_saturated_channel() {
        use cs2_script::{Host, KcsProgram};
        let (_scratch, mut player, old_cleanup) = player();
        player.sound_groups.push(crate::SoundGroup { key: 17, id: 0, label: b"effects".to_vec(), range: [6, 6] });
        let root = player.current_task_id;
        player.with_host(|host| Ok(host.create_task_object(13, root, b"sound", b"")?)).unwrap();
        let other = player.with_host(|host| Ok(host.create_unloaded_ordinary_task(root, b"")?)).unwrap();
        let mut vm = Vm::from_program(KcsProgram::from_code(&[0x3b], vec![0; 512], 64).unwrap()).unwrap();
        vm.write(64, b"sound/replacement.wav\0").unwrap();
        let args = [17_u32, 0, 64, 1, 1, 0].into_iter().flat_map(u32::to_le_bytes).collect::<Vec<_>>();
        assert_eq!(player.with_host(|host| Ok(host.call(936, &args, &mut vm)?)).unwrap(), NativeReturn::Retry);
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let result = player.with_host(|host| { host.current_task_id = other;
                Ok(host.load_sound_resolved_name(6, b"sound/second.wav", 1, 0)?) }).unwrap();
            if result != NativeReturn::Retry { assert_eq!(result, NativeReturn::Value(1)); break; }
            assert!(Instant::now() < deadline); std::thread::yield_now();
        }
        assert!(player.data_services.sound_async.cancelled.contains(&root));
        let (started, startup) = mpsc::channel();
        let (new_cleanup, controls) = mpsc::channel();
        let AudioStart::Pending(ticket) = player.audio_host.begin_test_pending_start_with_controls(AudioRequest {
            channel: AudioChannel::SoundEffect, resource: "new-owner.wav".into(), volume: 1., pan: 0., looped: false,
        }, startup, Some(controls)).unwrap() else { panic!("test startup must be pending"); };
        started.send(Ok(())).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        let playback = loop {
            if let Some(playback) = player.audio_host.poll_play(ticket).unwrap() { break playback; }
            assert!(Instant::now() < deadline); std::thread::yield_now();
        };
        player.sound_channels.get_mut(&6).unwrap().handle = Some(playback.handle);
        // The only group member is now owned by an unrelated active device.
        // Its cleanup gate stays closed: a wrong retry would enqueue stop and
        // suspend again instead of acknowledging the cancelled native.
        assert_eq!(player.with_host(|host| Ok(host.call(936, &args, &mut vm)?)).unwrap(), NativeReturn::Value(u32::MAX));
        assert_eq!(player.sound_channels[&6].handle, Some(playback.handle));
        assert!(player.sound_channels[&6].pending_start.is_none());
        assert!(player.audio_host.is_active(playback.handle).unwrap());
        assert!(!player.data_services.sound_async.cancelled.contains(&root));
        let _ = old_cleanup.send(Ok(())); let _ = new_cleanup.send(Ok(()));
    }

    struct Scratch(PathBuf);
    impl Drop for Scratch {
        fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
    }
    fn wave() -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF"); bytes.extend_from_slice(&38_u32.to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt "); bytes.extend_from_slice(&16_u32.to_le_bytes());
        for value in [1_u16, 1] { bytes.extend_from_slice(&value.to_le_bytes()); }
        bytes.extend_from_slice(&44_100_u32.to_le_bytes()); bytes.extend_from_slice(&88_200_u32.to_le_bytes());
        for value in [2_u16, 16] { bytes.extend_from_slice(&value.to_le_bytes()); }
        bytes.extend_from_slice(b"data"); bytes.extend_from_slice(&2_u32.to_le_bytes()); bytes.extend_from_slice(&0_i16.to_le_bytes());
        bytes
    }

    #[test]
    fn native_sound_load_accepts_unicode_cst_input() -> anyhow::Result<()> {
        use cs2_script::{Host, KcsProgram};

        let (_scratch, mut player, cleanup) = player();
        let root = player.current_task_id;
        player.sound_groups.push(crate::SoundGroup {
            key: 1, id: 0, label: b"se".to_vec(), range: [8, 9],
        });
        player.with_host(|host| Ok(host.create_task_object(13, root, b"sound", b"")?))?;
        let mut vm = Vm::from_program(KcsProgram::from_code(&[0x3b], vec![0; 512], 64)?)?;
        let group_args = [1_u32, 0, 64, 1, 1, 0].map(u32::to_le_bytes).concat();
        let single_args = [8_u32, 0, 64, 1, 0].map(u32::to_le_bytes).concat();
        // The GBK scene contains the base archive's CP932 command bytes.
        // The Unicode scene reader passes this spelling to native 936.
        let raw = b"se[\x82\xc7\x82\xb3\x82\xc1]";
        let unicode = ScriptTextEncoding::Gbk.decode(raw);
        assert!(decode_name(ScriptTextEncoding::Cp932, unicode.as_bytes()).is_err());
        assert!(encode_name(ScriptTextEncoding::Cp932, &unicode).is_err());
        vm.write(64, &[unicode.as_bytes(), b"\0"].concat())?;
        for fast_forward in [false, true] {
            player.data_services.fast_forward = fast_forward;
            player.with_host(|host| {
                assert_eq!(host.call(936, &group_args, &mut vm)?, NativeReturn::Value(u32::MAX));
                assert_eq!(host.call(935, &single_args, &mut vm)?, NativeReturn::Value(0));
                assert!(host.load_sound_group_name_mode(1, true, unicode.as_bytes(), 1, 0, 0)?
                    == NativeReturn::Value(u32::MAX));
                Ok(())
            })?;
            assert!(!player.sound_channels.contains_key(&8));
            assert!(player.data_services.sound_async.task_loads.is_empty());
        }
        player.with_host(|host| {
            let name = "sound/\u{6708}.wav";
            assert!(decode_name(ScriptTextEncoding::Cp932, name.as_bytes()).is_err());
            let narrow = encode_name(ScriptTextEncoding::Cp932, name)?;
            assert_eq!(host.sound_resource_names(&narrow)?, [name]);
            assert_eq!(host.sound_resource_names(name.as_bytes())?, [name]);
            assert_eq!(host.sound_resource_names(b"sound/replacement.wav")?, ["sound/replacement.wav"]);
            assert!(host.sound_resource_names(b"sound/\x81").is_err());
            Ok(())
        })?;
        cleanup.send(Ok(()))?;
        Ok(())
    }

    #[test]
    fn fast_forward_twenty_sound_cycles() -> anyhow::Result<()> {
        let scratch = crate::tests::TempDir::new();
        std::fs::create_dir(scratch.0.join("sound"))?;
        std::fs::write(scratch.0.join("sound/voice.wav"), wave())?;
        std::fs::write(scratch.0.join("sound/bgm.wav"), wave())?;
        let mut player = HeadlessPlayer::open_with_save_root(&scratch.0, scratch.0.join("profile"))?;
        let mut router = cs2_runtime::resource_router::ResourceRouter::default();
        router.slots[0] = cs2_runtime::resource_router::ResourceSlot {
            prefix: "sound".encode_utf16().collect(), loose_directory: "sound".encode_utf16().collect(),
            loose_enabled: true, archive: None,
        };
        player.scan.resources.set_resource_router(router)?;
        player.enable_host_fast_forward(0);
        let ctrl = cs2_runtime::InputKey::Keyboard(17);
        let load = |player: &mut HeadlessPlayer, channel, name: &[u8]| -> anyhow::Result<NativeReturn> {
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                let result = player.with_host(|host| Ok(host.load_sound_resolved_name(channel, name, 1, 0)?))?;
                if result != NativeReturn::Retry { return Ok(result); }
                anyhow::ensure!(Instant::now() < deadline, "audio preparation timed out");
                std::thread::sleep(Duration::from_millis(1));
            }
        };
        for _ in 0..20 {
            assert_eq!(player.with_host(|host| Ok(host.load_sound_resolved_name(8, b"sound/voice.wav", 1, 0)?))?, NativeReturn::Retry);
            player.apply_host_input(cs2_runtime::InputEvent::KeyDown(ctrl))?;
            assert!(player.advance_host_frame(1000)?);
            assert!(player.sound_channels.get(&6).is_none_or(|state| state.prepared_audio.is_none()));
            assert_eq!(load(&mut player, 8, b"sound/voice.wav")?, NativeReturn::Value(0));
            assert!(player.sound_channels[&8].pending_start.is_none());
            assert_eq!(load(&mut player, 6, b"sound/voice.wav")?, NativeReturn::Value(0));
            assert_eq!(load(&mut player, 7, b"sound/bgm.wav")?, NativeReturn::Value(1));
            assert_eq!(load(&mut player, 7, b"sound/voice.wav")?, NativeReturn::Value(0));
            assert_eq!(player.sound_channels[&7].resource.as_deref(), Some("sound/bgm.wav"));
            player.apply_host_input(cs2_runtime::InputEvent::KeyUp(ctrl))?;
            assert!(player.advance_host_frame(1000)?);
            assert_eq!(load(&mut player, 6, b"sound/voice.wav")?, NativeReturn::Value(1));
        }
        println!("fast_forward cycles=20 new_voice_skipped=40 pending_voice_skipped=20 bgm_prepared=20 bgm_preserved=20 voice_resumed=20");
        Ok(())
    }

    fn player() -> (Scratch, HeadlessPlayer, mpsc::Sender<cs2_runtime::RuntimeResult<()>>) {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let directory = std::env::temp_dir().join(format!("cs2-async-load-{}-{stamp}", std::process::id()));
        std::fs::create_dir_all(directory.join("sound")).unwrap();
        std::fs::write(directory.join("sound/replacement.wav"), wave()).unwrap();
        std::fs::write(directory.join("sound/second.wav"), wave()).unwrap();
        std::fs::write(directory.join("sound/compat.wav"), wave()).unwrap();
        let mut player = HeadlessPlayer::open_with_save_root(&directory, directory.join("profile")).unwrap();
        let mut router = cs2_runtime::resource_router::ResourceRouter::default();
        router.slots[0] = cs2_runtime::resource_router::ResourceSlot {
            prefix: "sound".encode_utf16().collect(), loose_directory: "sound".encode_utf16().collect(),
            loose_enabled: true, archive: None,
        };
        player.scan.resources.set_resource_router(router).unwrap();
        assert!(player.resources().resolve_mode_utf16(&"sound/replacement.wav".encode_utf16().collect::<Vec<_>>(),
            cs2_runtime::resource_router::ResourceMode::LooseFirst).unwrap().is_some());
        prepare_named_audio(wave(), "sound/replacement.wav").unwrap();
        let (started, startup) = mpsc::channel();
        let (cleanup, control) = mpsc::channel();
        let AudioStart::Pending(ticket) = player.audio_host.begin_test_pending_start_with_controls(AudioRequest {
            channel: AudioChannel::SoundEffect, resource: "old.wav".into(), volume: 1.0, pan: 0.0, looped: false,
        }, startup, Some(control)).unwrap() else { panic!("pending startup required"); };
        started.send(Ok(())).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        let playback = loop {
            if let Some(playback) = player.audio_host.poll_play(ticket).unwrap() { break playback; }
            assert!(Instant::now() < deadline); std::thread::yield_now();
        };
        player.sound_channels.insert(6, SoundChannelState { resource: Some("old.wav".into()), handle: Some(playback.handle), ..Default::default() });
        (Scratch(directory), player, cleanup)
    }

    #[test]
    fn async_sound_reload_waits_for_stop_and_uses_the_original_names_and_options() {
        let (_scratch, mut player, cleanup) = player();
        let result = player.with_host(|host| Ok(host.load_sound_resolved_name(6, b"sound/replacement.wav", 1, 25)?)).unwrap();
        assert_eq!(result, NativeReturn::Retry);
        assert_eq!(player.sound_channels[&6].resource.as_deref(), Some("old.wav"));
        assert!(player.with_host(|host| Ok(host.poll_task_audio_start(host.current_task_id)?)).unwrap().is_none());
        cleanup.send(Ok(())).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let result = player.with_host(|host| Ok(host.load_sound_resolved_name(9, b"missing.wav", 2, 999)?)).unwrap();
            if result != NativeReturn::Retry { assert_eq!(result, NativeReturn::Value(1)); break; }
            assert!(Instant::now() < deadline); std::thread::yield_now();
        }
        assert_eq!(player.sound_channels[&6].resource.as_deref(), Some("sound/replacement.wav"));
        assert_eq!(player.sound_channels[&6].selector, 1);
        assert_eq!(player.sound_channels[&6].duration_frames, 25);
        assert!(!player.sound_channels.contains_key(&9));
    }

    #[test]
    fn async_group_reload_retains_its_record_and_physical_allocation_through_reclaim() {
        let (_scratch, mut player, cleanup) = player();
        player.sound_groups.push(crate::SoundGroup { key: 17, id: 0, label: b"effects".to_vec(), range: [6, 6] });
        let root = player.current_task_id;
        let task = player.with_host(|host| Ok(host.create_task_object(13, root, b"sound", b"")?)).unwrap();
        let mut record = [0_u8; 292];
        record[4..8].copy_from_slice(&17_u32.to_le_bytes());
        record[12..33].copy_from_slice(b"sound/replacement.wav");
        record[280..284].copy_from_slice(&1_u32.to_le_bytes());
        record[284..288].copy_from_slice(&37_u32.to_le_bytes());
        let result = player.with_host(|host| Ok(host.dispatch_sound_load_record(task, 206, &mut record)?)).unwrap();
        assert_eq!(result, NativeReturn::Retry);
        assert!(player.sound_channels.get(&38).is_none_or(|state| state.resource.is_none()));
        cleanup.send(Ok(())).unwrap();
        record[4..8].copy_from_slice(&99_u32.to_le_bytes());
        record[280..284].copy_from_slice(&0_u32.to_le_bytes());
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let result = player.with_host(|host| Ok(host.dispatch_sound_load_record(task, 206, &mut record)?)).unwrap();
            if result != NativeReturn::Retry { assert_eq!(result, NativeReturn::Value(38)); break; }
            assert!(Instant::now() < deadline); std::thread::yield_now();
        }
        assert_eq!(player.sound_channels[&38].resource.as_deref(), Some("sound/replacement.wav"));
        assert_eq!(player.sound_channels[&38].task.load_mode, 1);
        assert_eq!(player.sound_channels[&38].task.load_duration, 37);
        assert!(player.sound_channels[&6].resource.is_none());
    }

    #[test]
    fn async_saturated_group_reuse_waits_for_cleanup_then_queues_the_next_play() {
        for mode in [0_u32, 1] {
            let (_scratch, mut player, cleanup) = player();
            let (startup, gate) = mpsc::channel();
            let AudioStart::Pending(ticket) = player.audio_host.begin_test_pending_start(AudioRequest {
                channel: AudioChannel::SoundEffect, resource: "other.wav".into(), volume: 1.0, pan: 0.0, looped: false,
            }, gate).unwrap() else { panic!("pending startup required"); };
            startup.send(Ok(())).unwrap();
            let deadline = Instant::now() + Duration::from_secs(2);
            let other = loop {
                if let Some(playback) = player.audio_host.poll_play(ticket).unwrap() { break playback; }
                assert!(Instant::now() < deadline); std::thread::yield_now();
            };
            player.sound_channels.insert(7, SoundChannelState { resource: Some("other.wav".into()), handle: Some(other.handle), ..Default::default() });
            {
                let state = player.sound_channels.get_mut(&6).unwrap();
                state.task.last_tick = 77;
                state.task.loop_range[1] = 50;
                state.task.linked = Some(31);
            }
            player.sound_groups.push(crate::SoundGroup { key: 17, id: 0, label: b"effects".to_vec(), range: [6, 7] });
            let owner = player.current_task_id;
            let task = player.with_host(|host| Ok(host.create_task_object(13, owner, b"sound", b"")?)).unwrap();
            let mut record = [0_u8; 292];
            record[4..8].copy_from_slice(&17_u32.to_le_bytes());
            record[12..33].copy_from_slice(b"sound/replacement.wav");
            record[280..284].copy_from_slice(&mode.to_le_bytes());
            for _ in 0..2 {
                let result = player.with_host(|host| Ok(host.dispatch_sound_load_record(task, 206, &mut record)?)).unwrap();
                assert_eq!(result, NativeReturn::Retry);
                assert_eq!(player.sound_channels[&6].resource.as_deref(), Some("old.wav"));
                assert!(!player.data_services.sound_async.cancelled.contains(&owner));
            }
            cleanup.send(Ok(())).unwrap();
            let channel = 6 + mode * 32;
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                let result = player.with_host(|host| Ok(host.dispatch_sound_load_record(task, 206, &mut record)?)).unwrap();
                if result != NativeReturn::Retry { assert_eq!(result, NativeReturn::Value(channel)); break; }
                assert!(Instant::now() < deadline); std::thread::yield_now();
            }
            assert!(!player.data_services.sound_async.cancelled.contains(&owner));
            assert_eq!(player.sound_channels[&channel].resource.as_deref(), Some("sound/replacement.wav"));
            if mode == 1 {
                let logical = &player.sound_channels[&6];
                assert!(logical.resource.is_none());
                assert_eq!(logical.task.last_tick, 77);
                assert_eq!(logical.task.loop_range[1], 50);
                assert_eq!(logical.task.linked, Some(31));
            }
            assert!(player.audio_host.is_active(other.handle).unwrap());
            let result = player.with_host(|host| Ok(host.control_sound_values(157, channel, 1, 0)?)).unwrap();
            assert_eq!(result, NativeReturn::Retry, "the next native157 must enqueue playback, never consume a stale cancellation");
            assert!(matches!(player.sound_channels[&channel].pending_start.as_ref().map(|pending| &pending.operation),
                Some(crate::sound_async::Operation::Start(_))));
            player.with_host(|host| Ok(host.cancel_audio_channel_start(channel)?)).unwrap();
        }
    }

    #[test]
    fn async_queued_reload_cancellation_removes_only_its_retained_packet() {
        let (_scratch, mut player, cleanup) = player();
        let root = player.current_task_id;
        let task = player.with_host(|host| Ok(host.create_task_object(13, root, b"sound", b"")?)).unwrap();
        let mut record = [0_u8; 292];
        record[..4].copy_from_slice(&6_u32.to_le_bytes());
        record[12..33].copy_from_slice(b"sound/replacement.wav");
        player.with_host(|host| {
            let mut output = 0;
            host.queue_sound_task_command(task, TaskCommand { target: task as i32, sender: root as i32, code: 180, data: [0; 3] },
                &TaskCommandBuffers { data0: None, data1: Some(&mut record) }, &mut output)?;
            host.queue_sound_task_command(task, TaskCommand { target: task as i32, sender: root as i32, code: 187, data: [6, 42, 0] },
                &TaskCommandBuffers { data0: None, data1: None }, &mut output)?;
            host.update_sound_task(task)?;
            Ok(())
        }).unwrap();
        assert!(player.data_services.sound_async.task_loads[&task].queued);
        player.with_host(|host| Ok(host.cancel_audio_channel_start(6)?)).unwrap();
        cleanup.send(Ok(())).unwrap();
        player.with_host(|host| Ok(host.update_sound_task(task)?)).unwrap();
        assert_eq!(player.sound_channels[&6].resource.as_deref(), Some("old.wav"));
        assert_eq!(player.sound_channels[&6].volume, 42);
        assert!(player.data_services.sound_async.task_loads.is_empty());
    }

    #[test]
    fn async_queued_reload_superseded_inside_receiver_cannot_replay_after_reinsertion() {
        let (_scratch, mut player, cleanup) = player();
        let root = player.current_task_id;
        let task = player.with_host(|host| Ok(host.create_task_object(13, root, b"sound", b"")?)).unwrap();
        let mut record = [0_u8; 292];
        record[..4].copy_from_slice(&6_u32.to_le_bytes());
        record[12..33].copy_from_slice(b"sound/replacement.wav");
        player.with_host(|host| {
            let mut output = 0;
            host.queue_sound_task_command(task, TaskCommand { target: task as i32, sender: root as i32, code: 180, data: [0; 3] },
                &TaskCommandBuffers { data0: None, data1: Some(&mut record) }, &mut output)?;
            host.update_sound_task(task)?;
            Ok(())
        }).unwrap();
        record[12..272].fill(0);
        record[12..28].copy_from_slice(b"sound/second.wav");
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let result = player.with_host(|host| Ok(host.dispatch_sound_load_record(task, 180, &mut record)?)).unwrap();
            if result != NativeReturn::Retry { assert_eq!(result, NativeReturn::Value(1)); break; }
            assert!(Instant::now() < deadline); std::thread::yield_now();
        }
        cleanup.send(Ok(())).unwrap();
        player.with_host(|host| Ok(host.update_sound_task(task)?)).unwrap();
        assert_eq!(player.sound_channels[&6].resource.as_deref(), Some("sound/second.wav"));
        assert!(player.data_services.sound_async.queued_cancellations.is_empty());
    }

    #[test]
    fn async_sound_reload_cancellation_drops_the_owned_load_without_joining() {
        let (_scratch, mut player, cleanup) = player();
        let result = player.with_host(|host| Ok(host.load_sound_resolved_name(6, b"sound/replacement.wav", 1, 0)?)).unwrap();
        assert_eq!(result, NativeReturn::Retry);
        player.with_host(|host| Ok(host.cancel_audio_channel_start(6)?)).unwrap();
        assert!(player.data_services.sound_async.loads.is_empty());
        assert!(player.sound_channels[&6].handle.is_none());
        cleanup.send(Ok(())).unwrap();
        let result = player.with_host(|host| Ok(host.load_sound_resolved_name(6, b"missing.wav", 1, 0)?)).unwrap();
        assert_eq!(result, NativeReturn::Value(0));
        assert_eq!(player.sound_channels[&6].resource.as_deref(), Some("old.wav"));
    }
}

//! Ordinary 935/936 and direct FES sound preparation, without synthetic VM pointers.
use crate::audio_decode::{MAX_ENCODED, prepare_named_audio};
use crate::perf_detail::audio::{Profile as AudioProfile, Stage as AudioStage};
use crate::perf_detail::SlowTask;
use crate::{
    LoggingHost, ScriptTextEncoding, audio_script_error, expand_kcs_format, read_u32, trim_nul,
};
use cs2_runtime::{TaskCommand, TaskCommandBuffers};
use cs2_script::{NativeReturn, Vm};
use std::sync::atomic::{AtomicU64, Ordering};

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
        if channel >= 64 {
            return Ok(NativeReturn::Value(0));
        }
        let task = self.default_sound_task()?.ok_or_else(|| {
            audio_script_error("FES sound mode loading requires a kind 13 sound task")
        })?;
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
        let task = self.default_sound_task()?.ok_or_else(|| {
            audio_script_error("FES sound mode loading requires a kind 13 sound task")
        })?;
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
        let mut output = 0;
        if !self.dispatch_sound_task_command(
            task,
            TaskCommand {
                target: task as i32,
                sender: self.current_task_id as i32,
                code,
                data: [0; 3],
            },
            &mut TaskCommandBuffers {
                data0: None,
                data1: Some(&mut record),
            },
            &mut output,
        )? {
            return Err(audio_script_error(
                "sound task did not consume its load record",
            ));
        }
        Ok(NativeReturn::Value(output))
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
        let mut state = self.sound_channels.remove(&channel).unwrap_or_default();
        if let Some(handle) = state.handle
            && let Err(cause) = self.audio_host.stop(handle)
        {
            self.sound_channels.insert(channel, state);
            return Err(self.sound_host_error(cause));
        }
        state.release();
        let resources = self.resources;
        let mut source = None;
        for name in names {
            match resources.resolve_mode_utf16(
                &name.encode_utf16().collect::<Vec<_>>(),
                cs2_runtime::resource_router::ResourceMode::LooseFirst,
            ) {
                Ok(Some(candidate)) => {
                    source = Some((name, candidate.value));
                    break;
                }
                Ok(None) => {}
                Err(cause) => {
                    self.sound_channels.insert(channel, state);
                    self.note_sound_resource_failure("resolve", &name, cause);
                    return Ok(NativeReturn::Value(0));
                }
            }
        }
        let Some((resource, candidate)) = source else {
            self.sound_channels.insert(channel, state);
            return Ok(NativeReturn::Value(0));
        };
        let _audio = AudioProfile::new(self.current_task_id, channel, "load", None, &resource);
        let kind = || self.scene_nodes.get(&self.current_task_id).map_or(-1, |node| node.kind as i32);
        let encoded = {
            let _detail = SlowTask::new(self.current_task_id, kind, "sound_load_read")
                .named(resource.as_bytes(), None);
            let _timing = AudioStage::Read.begin();
            resources.read_candidate_bounded(candidate, MAX_ENCODED)
        };
        let prepared = match encoded {
            Ok(Some(encoded)) => {
                let _detail = SlowTask::new(self.current_task_id, kind, "sound_load_decode")
                    .named(resource.as_bytes(), None);
                let _timing = AudioStage::Prepare.begin();
                prepare_named_audio(encoded, &resource)
            }
            Ok(None) => Err(crate::media_process::error("encoded sound exceeds 64 MiB")),
            Err(cause) => Err(cause),
        };
        let prepared = match prepared {
            Ok(prepared) => prepared,
            Err(cause) => {
                self.sound_channels.insert(channel, state);
                self.note_sound_resource_failure("load", &resource, cause);
                return Ok(NativeReturn::Value(0));
            }
        };
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
        let sidecars = {
            let _timing = AudioStage::Sidecars.begin();
            self.load_sound_sidecars(&resource)
        };
        match sidecars {
            Ok(sidecars) => state.task.sidecars = sidecars,
            Err(cause) => {
                self.note_sound_resource_failure("sidecar", &resource, cause);
            }
        }
        self.sound_channels.insert(channel, state);
        if let Some(event) = self.events.last_mut() {
            event.detail.push_str(&format!(
                " sound_load channel={channel} selector={selector} resource={resource:?} frames={} rate={} channels={} codec={:?}",
                prepared.probe.frames, prepared.probe.sample_rate, prepared.probe.channels, prepared.probe.codec));
        }
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
        // Name resolution occurs before selector validation and group lookup.
        let _names = self.sound_resource_names(name)?;
        if !matches!(selector, 1 | 2) {
            return Err(audio_script_error(
                "native 936 selector has no defined return value",
            ));
        }
        if let Some(task) = self.default_sound_task()? {
            return self
                .load_sound_task_record(task, group, true, by_key, name, selector, duration);
        }
        let Some(channel) = self.select_sound_group_channel(group, by_key)? else {
            return Ok(NativeReturn::Value(u32::MAX));
        };
        match self.load_sound_name(channel, name, selector, duration)? {
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
        let name = decode_name(self.text_encoding, bytes)?;
        let names = if name.rfind(['/', '\\']).is_some_and(|offset| offset > 0) {
            vec![name.clone()]
        } else {
            let category = if self.sound_config_enabled != 0 {
                self.sound_config.file_part(bytes).map(|part| part.archive.clone())
            } else {
                self.legacy_sound_archive_name(bytes)?
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

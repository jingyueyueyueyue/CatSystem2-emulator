//! Typed backlog natives and ordered CST history import. This module owns no
//! CST registrations or shared strings; it borrows those existing services.
use cs2_runtime::{resource_router::ResourceMode, backlog::{Backlog, SegmentEncoding, Settings}};
use cs2_script::{CstDocument, NativeReturn, Result, Vm};
use super::{LoggingHost, LogicEvent, ScriptTextEncoding, cp932_to_utf8,
    expand_kcs_format, native_state_error, read_u32, trim_nul};
use super::cst_context_native::{decode_cst_dialogue, repair_routed_scene_text};
use super::string_native::expand_shared_strings;

const MAX_BYTES: usize = 64 * 1024 * 1024;
const MAX_ITEMS: usize = 1_000_000;

impl LoggingHost<'_> {
    pub(super) fn backlog_native(&mut self, index: u32, args: &[u8], vm: &mut Vm) -> Result<NativeReturn> {
        if index == 696 {
            // Its original wrapper does not read the argument block.
            let settings = match self.read_legacy_sound_xml("startup.xml")
                .map_err(|_| native_state_error("backlog startup XML is unavailable/malformed"))? {
                Some(xml) => Settings::from_xml(&xml)
                    .map_err(|_| native_state_error("backlog startup settings are invalid"))?,
                None => Settings::default(),
            };
            let backlog = Backlog::new(settings)
                .map_err(|_| native_state_error("backlog settings cannot advance layout"))?;
            let handle = self.allocate_handle();
            self.backlogs.insert(handle, backlog);
            return Ok(NativeReturn::Value(handle));
        }
        let handle = read_u32(args, 0)?;
        if handle == 0 {
            return Ok(if matches!(index, 850 | 851) { NativeReturn::Value(0) } else { NativeReturn::Continue });
        }
        if matches!(index, 850 | 851) {
            let record = read_u32(args, 4)?;
            let log = read_u32(args, 8)?;
            if log == 0 { return Ok(NativeReturn::Value(0)); }
            if index == 850 {
                let bytes = self.backlogs.get(&log)
                    .ok_or_else(|| native_state_error("native 850 invalid/wrong-type backlog"))?
                    .serialize().map_err(|_| native_state_error("backlog serialization failed"))?;
                self.css_handles.get_mut(&handle)
                    .ok_or_else(|| native_state_error("native 850 invalid/wrong-type CSS container"))?
                    .append(record, &bytes).map_err(|_| native_state_error("backlog CSS append failed"))?;
            } else {
                let Some(bytes) = self.css_handles.get(&handle)
                    .ok_or_else(|| native_state_error("native 851 invalid/wrong-type CSS container"))?
                    .try_record(record).map_err(|_| native_state_error("backlog CSS record is malformed"))?
                    .map(ToOwned::to_owned) else { return Ok(NativeReturn::Value(0)); };
                self.backlogs.get_mut(&log)
                    .ok_or_else(|| native_state_error("native 851 invalid/wrong-type backlog"))?
                    .restore(&bytes).map_err(|_| native_state_error("backlog restore failed"))?;
            }
            return Ok(NativeReturn::Value(1));
        }
        if index == 697 {
            self.backlogs.remove(&handle)
                .ok_or_else(|| native_state_error("native 697 invalid/wrong-type backlog"))?;
            return Ok(NativeReturn::Continue);
        }
        if matches!(index, 699 | 1034) {
            let context = read_u32(args, 4)?;
            if context == 0 { return Ok(NativeReturn::Continue); }
            self.backlogs.get_mut(&handle)
                .ok_or_else(|| native_state_error("history import invalid/wrong-type backlog"))?
                .clear();
            // Clear precedes formatting and all resource accesses. Errors
            // preserve the completed clear and any earlier imported entries.
            let sound = formatted(vm, read_u32(args, 8)?, 64)?;
            let continuous = formatted(vm, read_u32(args, 12)?, 64)?;
            let language = if index == 1034 {
                Some(formatted(vm, read_u32(args, 16)?, 64)?)
            } else { None };
            return self.import_backlog(handle, context, &sound, &continuous, language.as_deref());
        }
        if !self.backlogs.contains_key(&handle) {
            return Err(native_state_error("invalid/wrong-type backlog handle"));
        }
        match index {
            698 | 977 => {
                let source = formatted(vm, read_u32(args, 4)?, 64)?;
                let input = read_u32(args, 8)?;
                let (mut name, name_codec) = self.formatted_history_text(vm, read_u32(args, 12)?, 256)?;
                let (text, mut body_codec) = self.formatted_history_text(vm, read_u32(args, 16)?, 4096)?;
                let scene_codec = self.cst_contexts.values().find_map(|context|
                    context.story.scene_name().as_bytes().eq_ignore_ascii_case(&source).then_some(context.encoding));
                body_codec = body_codec.or(scene_codec);
                strip_name_prefix(&mut name, b"\xef\xbc\xa0");
                self.backlogs.get_mut(&handle).unwrap().append_sourced(
                    &source, input, &name, &text,
                    read_u32(args, 20)?, read_u32(args, 24)?,
                    if index == 977 { read_u32(args, 28)? } else { u32::MAX },
                    SegmentEncoding { name: name_codec, body: body_codec },
                ).map_err(|_| native_state_error("backlog byte layout or storage is invalid"))?;
            }
            700 => {
                let target = read_u32(args, 4)?;
                let row = read_u32(args, 8)?;
                let task = self.current_task_id;
                let origins = &mut self.data_services.cst_text_origins;
                origins.invalidate_returned(task, target);
                vm.write(target, &[0])?;
                let log = &self.backlogs[&handle];
                if let Some(name) = log.name_for_row(row) {
                    let source_codec = log.segment_for_row(row).and_then(|(segment, encoding)| {
                        encoding.name.or_else(|| self.saved_history_name_codec(log, segment, name))
                    });
                    let codec = source_codec.unwrap_or(if self.data_services.has_loose_gbk_text() {
                        ScriptTextEncoding::Gbk
                    } else { self.text_encoding });
                    let (display, bad_at) = log.decode_name(name, codec);
                    let mut bytes = name.to_vec();
                    bytes.push(0);
                    vm.write(target, &bytes)?;
                    self.data_services.cst_text_origins.record_display(task, target, name, codec, display);
                    if let Some(bad_at) = bad_at {
                        self.data_services.note_text_decode(self.events, self.frame, format!(
                            "backlog speaker row={row} codec={codec:?} bad_at={bad_at}",
                        ));
                    }
                    if self.data_services.story_trace {
                        let (source, input) = log.segment_for_row(row).map_or((None, None), |(segment, _)|
                            (log.sources().get(segment.source as usize), Some(segment.input)));
                        self.events.push(LogicEvent { frame: self.frame, kind: "backlog-speaker".into(),
                            detail: format!("row={row} source={:?} input={input:?} codec={codec:?} matched={source_codec:?} bytes={:02x?}",
                                source.map(|source| String::from_utf8_lossy(source)), name),
                        });
                    }
                }
            }
            785 => self.backlogs.get_mut(&handle).unwrap().set_limit(read_u32(args, 4)?),
            876 => self.backlogs.get_mut(&handle).unwrap().clear(),
            _ => return Err(cs2_script::ScriptError::UnimplementedNativeCall { index }),
        }
        Ok(NativeReturn::Continue)
    }

    fn formatted_history_text(
        &self, vm: &Vm, address: u32, capacity: usize,
    ) -> Result<(Vec<u8>, Option<ScriptTextEncoding>)> {
        let mut codec = None;
        let mut mixed = false;
        let origins = &self.data_services.cst_text_origins;
        let input = vm.read_string(address)?;
        let bytes = super::expand_kcs_format_observed(vm, &input, 0, &mut |address, bytes| {
            if bytes.is_ascii() { return; }
            match origins.returned(self.current_task_id, address, bytes) {
                Some(candidate) if codec.is_none_or(|previous| previous == candidate) => codec = Some(candidate),
                _ => mixed = true,
            }
        })?;
        let bytes = trim_nul(&bytes);
        if bytes.len() >= capacity {
            return Err(native_state_error("backlog formatted text exceeds buffer"));
        }
        let direct = origins.returned(self.current_task_id, address, bytes);
        let expanded = if mixed || !input.is_ascii() { None } else { codec };
        Ok((bytes.to_vec(), direct.or(expanded)))
    }

    fn saved_history_name_codec(
        &self, log: &Backlog, segment: &cs2_runtime::backlog::Segment, name: &[u8],
    ) -> Option<ScriptTextEncoding> {
        if name.is_ascii() { return None; }
        let source = log.sources().get(segment.source as usize)?;
        let (_, resource) = self.cst_resource_with_source(b"scene", source).ok()??;
        let mut document = CstDocument::parse(&resource.bytes).ok()?;
        let codec = self.cst_scene_encoding_override_with_document(&resource, Some(&document))
            .unwrap_or(self.text_encoding);
        repair_routed_scene_text(self.resources, &resource, &mut document);
        let mut matches = None;
        for record in document.input_records(segment.input as usize)?.iter().filter(|row| row.kind == 0x21) {
            let mut raw = trim_nul(&record.text).to_vec();
            let delimiter = if codec == ScriptTextEncoding::Utf8 { b"\xef\xbc\xa0".as_slice() } else { b"\x81\x97" };
            strip_name_prefix(&mut raw, delimiter);
            if log.matches_name(name, &raw, codec) {
                if matches.is_some_and(|other| other != codec) { return None; }
                matches = Some(codec);
            }
            if let Ok(decoded) = decode_cst_dialogue(&record.text, codec) {
                let mut utf8 = decoded.into_owned().into_bytes();
                strip_name_prefix(&mut utf8, b"\xef\xbc\xa0");
                if log.matches_name(name, &utf8, ScriptTextEncoding::Utf8) {
                    if matches.is_some_and(|other| other != ScriptTextEncoding::Utf8) { return None; }
                    matches = Some(ScriptTextEncoding::Utf8);
                }
            }
        }
        matches
    }

    fn import_backlog(
        &mut self, handle: u32, context: u32, sound: &[u8], continuous: &[u8], language: Option<&[u8]>,
    ) -> Result<NativeReturn> {
        let history = self.cst_contexts.get(&context)
            .ok_or_else(|| native_state_error("history import invalid/wrong-type CST context"))?
            .history.iter().map(|entry| (trim_nul(&entry.name).to_vec(), entry.value))
            .collect::<Vec<_>>();
        if history.len() > MAX_ITEMS { return Err(native_state_error("CST history exceeds item limit")); }
        let limit = self.backlogs[&handle].limit();
        let mut first = 0usize;
        if limit != u32::MAX && history.len() > 1 {
            let mut index = history.len() - 1;
            let mut count = 0u32;
            while index > 0 && (count as i32) < limit as i32 {
                first = index;
                count = count.wrapping_add(history[index].1.wrapping_sub(1));
                index -= 1;
            }
            if index == 0 && (count as i32) < limit as i32 { first = 0; }
        }
        let mut is_continuous = 0i32;
        let mut imported = 0usize;
        for (history_index, (name, count)) in history.iter().enumerate().skip(first) {
            let Some((resolved, resource)) = self.cst_resource_with_source(b"scene", name)? else { continue; };
            let bytes = &resource.bytes;
            if bytes.is_empty() { continue; }
            let source_codec = self.cst_scene_encoding_override(&resource).unwrap_or(self.text_encoding);
            // Both wrappers use the same temporary scene loader. After any
            // nonempty CST read it also attempts the adjacent language file,
            // even though no languages are registered into this scene.
            let mut cstl = resolved.clone();
            if let Some(dot) = cstl.rfind('.').filter(|&dot| cstl[..dot]
                .rfind(['/', '\\']).is_none_or(|slash| slash < dot)) {
                cstl.truncate(dot);
            }
            cstl.push_str(".cstl");
            let _ = self.backlog_read(&cstl, false)?;
            if bytes.get(..8) != Some(b"CatScene") {
                self.backlog_skipped(history_index, "non-CST resource"); continue;
            }
            if bytes.len() >= 16 {
                let packed = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
                let expanded = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
                let expected = 16usize.checked_add(if packed == 0 { expanded } else { packed });
                if expected != Some(bytes.len()) {
                    // 0x689daf..0x689dd0 returns load failure before any
                    // record query. Make that skip visible, then continue
                    // with later history entries instead of clearing-only.
                    self.backlog_skipped(history_index, "CST outer length rejected by original loader");
                    continue;
                }
            }
            let mut document = CstDocument::parse(bytes)
                .map_err(|_| native_state_error("backlog CST resource is malformed"))?;
            repair_routed_scene_text(self.resources, &resource, &mut document);
            if *count as i32 <= 0 { continue; }
            let count = *count as usize;
            imported = imported.checked_add(count).filter(|&count| count <= MAX_ITEMS)
                .ok_or_else(|| native_state_error("backlog import exceeds one million input segments"))?;
            for input in 0..count {
                let mut text = Vec::new();
                let mut speaker = Vec::new();
                let mut mark = 0u32;
                let mut space = u32::from(is_continuous == 0);
                let mut scratch = Vec::new();
                if let Some(range) = document.inputs.get(input) {
                    for record in &document.records[range.clone()] {
                        match record.kind {
                            0x20 | 0x21 => {
                                scratch = if language.is_some() {
                                    let (text, bad_at) = source_codec.decode_message(trim_nul(&record.text));
                                    if let Some(bad_at) = bad_at {
                                        self.data_services.note_text_decode(self.events, self.frame, format!(
                                            "native=1034 source={resolved:?} input={input} kind={:#x} codec={source_codec:?} bad_at={bad_at}",
                                            record.kind,
                                        ));
                                    }
                                    text.into_bytes()
                                } else { trim_nul(&record.text).to_vec() };
                                if scratch.len() >= 4096 { return Err(native_state_error("backlog CST text exceeds read buffer")); }
                                if record.kind == 0x20 {
                                    append_bounded(&mut text, &scratch, 4096)?;
                                } else {
                                    let delimiter = if language.is_some() { b"\xef\xbc\xa0".as_slice() } else { b"\x81\x97" };
                                    let mut name = scratch.clone();
                                    strip_name_prefix(&mut name, delimiter);
                                    append_bounded(&mut speaker, &name, 256)?;
                                }
                            }
                            0x30 => {
                                // Once a sound has succeeded the original
                                // wrapper reuses the last text buffer for all
                                // remaining commands in this input segment.
                                if mark == 0 {
                                    scratch = if language.is_some() { cp932_to_utf8(&record.text) } else { record.text.clone() };
                                    scratch.truncate(scratch.iter().position(|&byte| byte == 0).unwrap_or(scratch.len()));
                                    if scratch.len() >= 4096 { return Err(native_state_error("backlog command exceeds read buffer")); }
                                    if self.backlog_sound_exists(sound, &scratch)? {
                                        mark = 1;
                                        continue;
                                    }
                                }
                                is_continuous = self.backlog_continuous(continuous, &scratch, is_continuous)?;
                            }
                            2 => space = u32::from(is_continuous == 0),
                            3 if is_continuous != 0 => space = 1,
                            _ => {}
                        }
                    }
                }
                let text = expand_shared_strings(&text, self.string_values)?;
                let speaker = expand_shared_strings(&speaker, self.string_values)?;
                if text.len() >= 4096 || speaker.len() >= 256 {
                    return Err(native_state_error("backlog substituted text exceeds import buffers"));
                }
                let backlog = self.backlogs.get_mut(&handle).unwrap();
                let result = if language.is_some() {
                    backlog.append_utf8(name, input as u32, &speaker, &text, mark, space, history_index as u32)
                } else {
                    backlog.append_sourced(name, input as u32, &speaker, &text, mark, space, history_index as u32,
                        SegmentEncoding { name: Some(source_codec), body: Some(source_codec) })
                };
                result.map_err(|cause| super::native_state_error_owned(format!(
                    "backlog history={history_index} input={input} byte layout: {cause}"
                )))?;
            }
        }
        Ok(NativeReturn::Continue)
    }

    fn backlog_continuous(&mut self, configured: &[u8], command: &[u8], old: i32) -> Result<i32> {
        let length = command.iter().position(|&byte| byte == b' ').unwrap_or(command.len());
        if configured.len() < length || !configured[..length].eq_ignore_ascii_case(&command[..length])
            || length == command.len() { return Ok(old); }
        let tail = &command[length + 1..];
        let value = &tail[..tail.iter().position(|&byte| byte == b' ').unwrap_or(tail.len())];
        if value.len() >= 256 { return Err(native_state_error("backlog continuous parameter exceeds 255 bytes")); }
        self.eval_script_expression(value, false).map(|value| i32::from(value != 0))
    }

    fn backlog_sound_exists(&mut self, configured: &[u8], command: &[u8]) -> Result<bool> {
        let mut words = command.split(|&byte| byte == b' ').filter(|word| !word.is_empty());
        if words.next() != Some(configured) { return Ok(false); }
        for argument in words {
            let source = String::from_utf8(cp932_to_utf8(argument)).expect("CP932 conversion emits UTF-8");
            let names = self.media_category_candidates(b"sound", &source)?.unwrap_or_default();
            for name in names {
                if self.backlog_read(&name, true)?.is_some() { return Ok(true); }
            }
        }
        Ok(false)
    }

    pub(super) fn backlog_read(&self, name: &str, existence: bool) -> Result<Option<Vec<u8>>> {
        if let Some(bytes) = self.data_services.ordinary.cached_resource(name) {
            if !existence && bytes.len() > MAX_BYTES {
                return Err(native_state_error("backlog cached resource exceeds 64 MiB"));
            }
            return Ok(Some(if existence { Vec::new() } else { bytes.to_vec() }));
        }
        let Some(found) = self.resources.resolve_mode_utf16(
            &name.encode_utf16().collect::<Vec<_>>(), ResourceMode::LooseFirst,
        ).map_err(|_| native_state_error("backlog resource routing failed"))? else { return Ok(None); };
        let candidate = found.value;
        if !existence && candidate.size > MAX_BYTES as u64 {
            return Err(native_state_error("backlog resource exceeds 64 MiB"));
        }
        // Zero-byte sounds are openable; no decoder or object is required.
        let bytes = if existence {
            self.resources.open_media_candidate(candidate, u64::MAX).map(|_| Vec::new())
        } else { self.resources.read_candidate(candidate) }
            .map_err(|_| native_state_error("backlog resource open/read failed"))?;
        if !existence && bytes.len() > MAX_BYTES { return Err(native_state_error("backlog resource exceeds 64 MiB")); }
        Ok(Some(bytes))
    }

    fn backlog_skipped(&mut self, history: usize, reason: &str) {
        self.events.push(LogicEvent { frame: self.frame, kind: "backlog-import-skip".into(),
            detail: format!("history={history} reason={reason}") });
    }
}

fn formatted(vm: &Vm, address: u32, capacity: usize) -> Result<Vec<u8>> {
    let bytes = expand_kcs_format(vm, &vm.read_string(address)?, 0)?;
    let bytes = trim_nul(&bytes);
    if bytes.len() >= capacity { return Err(native_state_error("backlog formatted text exceeds buffer")); }
    Ok(bytes.to_vec())
}

fn strip_name_prefix(name: &mut Vec<u8>, delimiter: &[u8]) {
    if let Some(index) = name.windows(delimiter.len()).position(|window| window == delimiter) {
        name.drain(..index + delimiter.len());
    }
}

fn append_bounded(target: &mut Vec<u8>, source: &[u8], capacity: usize) -> Result<()> {
    if target.len().saturating_add(source.len()) >= capacity {
        return Err(native_state_error("backlog concatenated text exceeds fixed buffer"));
    }
    target.extend_from_slice(source);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cs2_script::Host;

    #[test]
    fn backlog_speaker_codec_survives_native_publication_to_fes() -> anyhow::Result<()> {
        let serial = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos();
        let root = std::env::temp_dir().join(format!("cs2-backlog-speaker-{}-{serial}", std::process::id()));
        std::fs::create_dir(&root)?;
        let result = (|| {
            let mut player = crate::HeadlessPlayer::open_with_save_root(&root, root.join("profile"))?;
            let mut vm = Vm::new();
            vm.bind_external(vec![0; 4096])?;
            let args = |words: &[u32]| words.iter().flat_map(|word| word.to_le_bytes()).collect::<Vec<_>>();
            let output = 0x8000_0400;
            let raw = b"\xe3\x80\x90\xc2\xa9\xe3\x80\x91";
            player.with_host(|host| {
                let NativeReturn::Value(handle) = host.call(696, &[], &mut vm)? else {
                    anyhow::bail!("missing backlog handle");
                };
                let mut backlog = Backlog::new(Settings {
                    name_first: "\u{3010}".as_bytes().to_vec(), name_last: "\u{3011}".as_bytes().to_vec(),
                    ..Settings::default()
                })?;
                backlog.append_sourced(
                    b"scene", 0, b"\xc2\xa9", b"\xc4\xe3", 0, 0, 0,
                    SegmentEncoding { name: Some(ScriptTextEncoding::Cp932), body: Some(ScriptTextEncoding::Gbk) },
                )?;
                host.backlogs.insert(handle, backlog);
                host.call(700, &args(&[handle, output, 0]), &mut vm)?;
                host.call(276, &args(&[60, output]), &mut vm)?;
                let stored = vm.read_string(output)?;
                assert_eq!(stored, raw);
                assert_eq!(host.data_services.cst_text_origins.shared(60, &stored), Some(ScriptTextEncoding::Cp932));
                assert_eq!(host.data_services.cst_text_origins.shared_display(60, &stored), Some("\u{3010}\u{ff82}\u{ff69}\u{3011}"));
                host.call(700, &args(&[handle, output, u32::MAX]), &mut vm)?;
                host.call(276, &args(&[60, output]), &mut vm)?;
                assert!(vm.read_string(output)?.is_empty());
                assert_eq!(host.data_services.cst_text_origins.shared(60, raw), None);
                assert_eq!(host.data_services.cst_text_origins.shared_display(60, raw), None);
                Ok(())
            })
        })();
        std::fs::remove_dir_all(root)?;
        result
    }

    #[test]
    #[ignore = "requires the installed Daydream translation and an isolated profile"]
    fn daydream_body_speaker_and_imported_history_use_the_winning_cst_codec() -> anyhow::Result<()> {
        let root = std::env::var_os("CS2_DAYDREAM_TEXT_TEST_ROOT").expect("installed game root required");
        let profile = std::env::var_os("CS2_DAYDREAM_TEXT_TEST_PROFILE").expect("isolated profile required");
        let mut player = crate::HeadlessPlayer::open_with_save_root(root, profile)?;
        let mut vm = Vm::new();
        vm.bind_external(vec![0; 4096])?;
        let args = |words: &[u32]| words.iter().flat_map(|word| word.to_le_bytes()).collect::<Vec<_>>();
        let scene = 0x8000_0100;
        let result = 0x8000_0400;
        let empty = 0x8000_0800;
        vm.write(scene, b"prologue.cst\0")?;
        let speaker = "\u{6d77}\u{6597}";
        let body = "\u{300c}\u{2014}\u{2014}\u{80fd}\u{6539}\u{53d8}\u{4eba}\u{7684}\u{ff0c}\u{53ea}\u{6709}\u{4eba}\u{300d}";
        player.with_host(|host| {
            let (_, resource) = host.cst_resource_with_source(b"scene", b"prologue.cst")?.unwrap();
            assert_eq!(host.cst_scene_encoding_override(&resource), Some(crate::ScriptTextEncoding::Gbk));
            let NativeReturn::Value(context) = host.call(323, &[], &mut vm)? else {
                anyhow::bail!("missing CST context handle");
            };
            assert_eq!(host.call(325, &args(&[context, scene]), &mut vm)?, NativeReturn::Value(1));
            for (ordinal, expected) in [(16, speaker), (17, body)] {
                host.call(1037, &args(&[context, result, 0, ordinal]), &mut vm)?;
                assert_eq!(vm.read_string(result)?, expected.as_bytes());
            }
            assert_eq!(host.call(329, &args(&[context, scene, 1]), &mut vm)?, NativeReturn::Value(0));
            let NativeReturn::Value(backlog) = host.call(696, &[], &mut vm)? else {
                anyhow::bail!("missing backlog handle");
            };
            host.call(1034, &args(&[backlog, context, empty, empty, empty]), &mut vm)?;
            let history = &host.backlogs[&backlog];
            let name = std::str::from_utf8(history.name_for_row(0).expect("missing history speaker"))?;
            assert!(name.contains(speaker));
            let rows = history.lines().iter().map(|line| std::str::from_utf8(&line.text))
                .collect::<std::result::Result<Vec<_>, _>>()?;
            assert!(rows.concat().contains(body));
            Ok(())
        })
    }

    #[test]
    fn backlog_resolves_voice_by_its_configured_archive() -> anyhow::Result<()> {
        let serial = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos();
        let root = std::env::temp_dir().join(format!("cs2-backlog-{}-{serial}", std::process::id()));
        std::fs::create_dir(&root)?;
        let result = (|| {
            std::fs::create_dir(root.join("config"))?;
            std::fs::create_dir(root.join("voice"))?;
            std::fs::write(root.join("config/startup.xml"), concat!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?><document>",
                "<VOICE><v00><head>voice_</head><int>pcm_a</int></v00></VOICE>",
                "<INT><int01><name>pcm_a</name><int>pcm_a.int</int><ext>ogg</ext>",
                "<dir>voice</dir></int01></INT></document>"
            ))?;
            std::fs::write(root.join("voice/voice_001.ogg"), [])?;
            let mut player = crate::HeadlessPlayer::open_with_save_root(&root, root.join("profile"))?;
            player.with_host(|host| {
                assert!(host.backlog_sound_exists(b"pcm", b"pcm voice_001")?);
                assert!(!host.backlog_sound_exists(b"pcm", b"pcm voice_002")?);
                assert!(!host.backlog_sound_exists(b"pcm", b"pcm no_such_category")?);
                Ok(())
            })
        })();
        std::fs::remove_dir_all(root)?;
        result
    }
}

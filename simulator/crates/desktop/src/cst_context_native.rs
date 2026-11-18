//! CST scene loading/localization and history variable snapshots.
//! Original binaries are used only as static byte evidence, never executed.
use std::collections::BTreeMap;

use cs2_formats::{ConfigXml, CstlTable};
use cs2_runtime::resource_router::ResourceMode;
use cs2_script::{CstDocument, NativeReturn, Result, ScriptError, Vm};
use cs2_script::cst_history::{HistorySnapshot, history_blocks, prepend_history_snapshot};

use super::{LoggingHost, ScriptTextEncoding, native_state_error, read_u32, trim_nul,
    expand_kcs_format, signed_cst_index, write_kcs_terminated};

const MAX_BYTES: usize = 64 * 1024 * 1024;
const MAX_ITEMS: usize = 1_000_000;

impl LoggingHost<'_> {
    pub(super) fn copy_cst_attached_data_native981(&mut self, args: &[u8]) -> Result<NativeReturn> {
        let destination = read_u32(args, 0)?;
        if destination == 0 { return Ok(NativeReturn::Continue); }
        let source = read_u32(args, 4)?;
        if source == 0 { return Ok(NativeReturn::Continue); }
        let destination_context = self.cst_contexts.get(&destination)
            .ok_or_else(|| native_state_error("native 981 invalid/wrong-type destination context"))?;
        let source_context = self.cst_contexts.get(&source)
            .ok_or_else(|| native_state_error("native 981 invalid/wrong-type source context"))?;
        if source == destination {
            // The original frees the same buffer it subsequently reads.
            if destination_context.registrations.iter().chain(&destination_context.history)
                .any(|entry| !entry.data.is_empty())
            {
                return Err(native_state_error("native 981 self-copy would read freed attached data"));
            }
            return Ok(NativeReturn::Continue);
        }
        let mut total = 0usize;
        let mut collect = |source: &[super::CstRegistration], count: usize| -> Result<Vec<Vec<u8>>> {
            source.iter().take(count).map(|entry| {
                total = total.checked_add(entry.data.len()).filter(|&total| total <= 256 * 1024 * 1024)
                    .ok_or_else(|| native_state_error("native 981 attached data exceeds 256 MiB"))?;
                Ok(entry.data.clone())
            }).collect()
        };
        let registrations = collect(&source_context.registrations, destination_context.registrations.len())?;
        let history = collect(&source_context.history, destination_context.history.len())?;
        let destination = self.cst_contexts.get_mut(&destination).unwrap();
        // Positional zip over BOTH tables. Names, values and string records
        // are untouched, and unmatched destination suffixes survive.
        for (entry, data) in destination.registrations.iter_mut().zip(registrations) { entry.data = data; }
        for (entry, data) in destination.history.iter_mut().zip(history) { entry.data = data; }
        Ok(NativeReturn::Continue)
    }

    pub(super) fn cst_option_native1064(&self, args: &[u8], vm: &mut Vm) -> Result<NativeReturn> {
        let handle = read_u32(args, 0)?;
        if handle == 0 { return Ok(NativeReturn::Value(0)); }
        let context = self.cst_contexts.get(&handle)
            .ok_or_else(|| native_state_error("native 1064 invalid/wrong-type CST context"))?;
        let target = read_u32(args, 4)?;
        match vm.read(target, 1) {
            Err(ScriptError::MemoryOutOfBounds { .. }) => return Ok(NativeReturn::Value(0)),
            other => { other?; }
        }
        let ordinal = read_u32(args, 8)?;
        let language = formatted(vm, read_u32(args, 12)?, 64)?;
        vm.write(target, &[0])?;
        let Some(table) = context.localization.as_ref().filter(|_|
            context.languages.binary_search_by(|name| name.as_slice().cmp(language.as_slice())).is_ok())
        else { return Ok(NativeReturn::Value(0)); };
        let Some(column) = table.languages.iter().position(|name| name.as_bytes() == language)
        else { return Ok(NativeReturn::Value(0)); };
        let count = context.document.as_ref().map_or(0, |document| document.inputs.len() as u32);
        let input = count.wrapping_sub(1).wrapping_add(ordinal);
        let mut text = Vec::new();
        if let Some(message) = table.messages.get(input as usize).and_then(|row| row.get(column)) {
            // Stop at the first empty fragment, including an initial U+001F.
            for fragment in message.content.split('\u{1f}').take_while(|fragment| !fragment.is_empty()) {
                let fragment = trim_nul(fragment.as_bytes());
                if text.len().checked_add(fragment.len()).is_none_or(|length| length >= MAX_BYTES) {
                    return Err(native_state_error("native 1064 localized option exceeds 64 MiB"));
                }
                text.extend_from_slice(fragment);
            }
        }
        write_kcs_terminated(vm, target, &text, 1)?;
        Ok(NativeReturn::Value(text.len() as u32))
    }

    pub(super) fn cst_container_native(&mut self, index: u32, args: &[u8]) -> Result<NativeReturn> {
        let handle = read_u32(args, 0)?;
        if handle == 0 {
            return Ok(if matches!(index, 979 | 980) {
                NativeReturn::Value(0)
            } else { NativeReturn::Continue });
        }
        if index == 993 {
            let source = read_u32(args, 4)?;
            if source == 0 { return Ok(NativeReturn::Continue); }
            if args.len() != 8 { return Err(native_state_error("native 993 requires two CST contexts")); }
            if !self.cst_contexts.contains_key(&handle) || !self.cst_contexts.contains_key(&source) {
                return Err(native_state_error("native 993 invalid/wrong-type CST context"));
            }
            // 0x68b877 clears the destination BEFORE enumerating the source.
            // Self-copy therefore clears the history rather than cloning it.
            let history = if source == handle { Vec::new() } else {
                self.cst_contexts[&source].history.iter().map(|entry| {
                    let mut name = [0; 32];
                    let length = entry.name.iter().position(|&byte| byte == 0)
                        .ok_or_else(|| native_state_error("native 993 source name has no terminator"))?;
                    let text = &entry.name[..length];
                    name[..text.len()].copy_from_slice(text);
                    Ok(super::CstRegistration { name, value: entry.value,
                        attachments: Vec::new(), data: Vec::new() })
                }).collect::<Result<Vec<_>>>()?
            };
            self.cst_contexts.get_mut(&handle).unwrap().history = history;
            return Ok(NativeReturn::Continue);
        }
        let context = self.cst_contexts.get(&handle)
            .ok_or_else(|| native_state_error("CST container operation has an invalid/wrong-type context"))?;
        if index == 980 {
            if args.len() != 4 { return Err(native_state_error("native 980 requires one context")); }
            let count = context.document.as_ref().map_or(0, |document| {
                document.records.iter().filter(|record| record.kind == 0x20).count() as u32
            });
            return Ok(NativeReturn::Value(count));
        }
        let value = read_u32(args, 4)?;
        if args.len() != 8 { return Err(native_state_error("native 978/979 requires two DWORDs")); }
        if index == 978 {
            if value == 0 { return Ok(NativeReturn::Continue); }
            let bytes = self.css_handles.get(&value)
                .ok_or_else(|| native_state_error("native 978 invalid/wrong-type CSS container"))?
                .snapshot().map_err(|_| native_state_error("native 978 CSS snapshot is malformed"))?;
            if let Some(last) = self.cst_contexts.get_mut(&handle).unwrap().history.last_mut() {
                // Same actual storage as 991's compressed variable chain.
                // Do not tag it or create an independent shadow attachment.
                last.data = bytes;
            }
            return Ok(NativeReturn::Continue);
        }
        if index == 979 {
            let Some(entry) = context.history.get(value as usize).filter(|entry| !entry.data.is_empty())
            else { return Ok(NativeReturn::Value(0)); };
            let file = cs2_formats::CssFile::from_snapshot(&entry.data)
                .map_err(|_| native_state_error("native 979 history attachment is not a valid CSS snapshot"))?;
            let handle = self.allocate_handle();
            self.css_handles.insert(handle, file);
            return Ok(NativeReturn::Value(handle));
        }
        Err(cs2_script::ScriptError::UnimplementedNativeCall { index })
    }

    pub(super) fn load_cst_context(&mut self, handle: u32, address: u32, vm: &Vm) -> Result<NativeReturn> {
        let name = formatted(vm, address, 260)?;
        let Some((resolved, bytes)) = self.cst_resource(b"scene", &name)? else {
            return Ok(NativeReturn::Value(0));
        };
        if bytes.is_empty() { return Ok(NativeReturn::Value(0)); }
        let signature = bytes.get(..8) == Some(b"CatScene");
        let valid_length = if signature && bytes.len() >= 16 {
            let packed = read_u32(&bytes, 8)? as usize;
            let plain = read_u32(&bytes, 12)? as usize;
            16usize.checked_add(if packed == 0 { plain } else { packed }) == Some(bytes.len())
        } else { false };
        // A wrong signature keeps the old scene; a recognized container with
        // rejected outer length clears only its scene, not its history/position.
        let loaded = if signature && valid_length {
            Some(CstDocument::parse(&bytes)?)
        } else { None };
        let success = loaded.is_some();
        {
            let context = self.cst_contexts.get_mut(&handle)
                .ok_or_else(|| native_state_error("CST context handle is invalid"))?;
            if signature {
                context.document = loaded;
                if success {
                    context.position = [0, 0];
                    context.encoding = self.text_encoding;
                }
            }
            context.localization = None;
        }
        let mut adjacent = resolved;
        if let Some(dot) = adjacent.rfind('.').filter(|&dot| adjacent[..dot]
            .rfind(['/', '\\']).is_none_or(|slash| slash < dot))
        {
            adjacent.truncate(dot);
        }
        adjacent.push_str(".cstl");
        if let Some(bytes) = self.cst_read_resource(&adjacent)? {
            if !bytes.is_empty() {
                let (table, _) = CstlTable::parse_prefix(&bytes)
                    .map_err(|_| native_state_error("CST adjacent localization table is malformed"))?;
                self.cst_contexts.get_mut(&handle).unwrap().localization = Some(table);
            }
        }
        Ok(NativeReturn::Value(u32::from(success)))
    }

    pub(super) fn read_cst_record(
        &self, index: u32, args: &[u8], vm: &mut Vm,
    ) -> Result<NativeReturn> {
        let context = self.cst_contexts.get(&read_u32(args, 0)?)
            .ok_or_else(|| native_state_error("CST context handle is invalid"))?;
        let target = read_u32(args, 4)?;
        // Ordinary start rejection returns zero without touching the buffer;
        // shared storage lock failures remain real diagnostics.
        match vm.read(target, 1) {
            Err(ScriptError::MemoryOutOfBounds { .. }) => return Ok(NativeReturn::Value(0)),
            other => { other?; }
        }
        let input = signed_cst_index(read_u32(args, 8)?, context.position[0]);
        let ordinal = signed_cst_index(read_u32(args, 12)?, context.position[1]);
        let Some(document) = &context.document else { return Ok(NativeReturn::Value(0)); };
        let Some(records) = usize::try_from(input).ok().and_then(|input| document.input_records(input))
        else { return Ok(NativeReturn::Value(0)); };
        let Some(record) = usize::try_from(ordinal).ok().and_then(|ordinal| records.get(ordinal))
        else { return Ok(NativeReturn::Value(0)); };
        let language = if matches!(index, 1033 | 1038) {
            Some(formatted(vm, read_u32(args, 16)?, 64)?)
        } else { None };
        let bound = language.as_deref().and_then(|language| {
            if context.languages.binary_search_by(|name| name.as_slice().cmp(language)).is_err() {
                return None;
            }
            let table = context.localization.as_ref()?;
            let column = table.languages.iter().position(|name| name.as_bytes() == language)?;
            Some((table, column))
        });
        let text = if let Some((table, column)) = bound.filter(|_| matches!(record.kind, 0x20 | 0x21)) {
            let message = table.messages.get(input as usize).and_then(|row| row.get(column));
            if record.kind == 0x21 {
                message.map_or(&b""[..], |message| message.name.as_bytes()).to_vec()
            } else {
                let fragment = records[..ordinal as usize].iter().filter(|r| r.kind == 0x20).count();
                message.and_then(|message| message.content.split('\u{1f}').nth(fragment))
                    .unwrap_or("").as_bytes().to_vec()
            }
        } else if matches!(index, 1037 | 1038) {
            context.encoding.decode(&record.text).into_bytes()
        } else { record.text.clone() };
        let text = trim_nul(&text);
        write_kcs_terminated(vm, target, text, 1)?;
        Ok(NativeReturn::Value(text.len() as u32))
    }

    pub(super) fn cst_snapshot_native(
        &mut self, index: u32, args: &[u8], vm: &mut Vm,
    ) -> Result<NativeReturn> {
        let handle = read_u32(args, 0)?;
        if handle == 0 {
            return Ok(if index == 992 { NativeReturn::Value(0) } else { NativeReturn::Continue });
        }
        if !self.cst_contexts.contains_key(&handle) {
            return Err(native_state_error("CST snapshot context handle is invalid"));
        }
        let expected = if index == 992 { 16 } else { 12 };
        if args.len() != expected { return Err(native_state_error("CST snapshot argument count is invalid")); }
        if index == 994 {
            return self.trim_cst_snapshots(handle, read_u32(args, 4)?, read_u32(args, 8)?);
        }
        if index == 991 && self.cst_contexts[&handle].history.is_empty() {
            return Ok(NativeReturn::Continue);
        }
        let (requested, position, outputs) = if index == 992 {
            let history_address = read_u32(args, 4)?;
            let position_address = read_u32(args, 8)?;
            (vm.read_u32(history_address)?, vm.read_u32(position_address)?,
                Some((history_address, position_address)))
        } else { (0, read_u32(args, 4)?, None) };
        let config = formatted(vm, read_u32(args, if index == 992 { 12 } else { 8 })?, 1024)?;
        let strings = self.cst_snapshot_string_ids(&config)?;
        if index == 991 {
            self.ensure_fes_settings()
                .map_err(|_| native_state_error("CST snapshot flag settings are unavailable"))?;
            let mut numbers = Vec::new();
            for [start, count] in self.data_services.fes_settings.cst_local_flag_ranges() {
                let mut id = start;
                let end = start.wrapping_add(count) as i32;
                while (id as i32) < end {
                    if numbers.len() >= MAX_ITEMS {
                        return Err(native_state_error("CST snapshot numeric ranges exceed one million values"));
                    }
                    let value = self.shared_values_for(self.current_task_id)
                        .and_then(|values| values.get(id as usize)).copied().unwrap_or(0);
                    numbers.push((id, value));
                    id = id.wrapping_add(1);
                }
            }
            let strings = strings.into_iter().map(|id| {
                (id, self.string_values.get(&id).cloned().unwrap_or_default())
            }).collect::<BTreeMap<_, _>>();
            let context = self.cst_contexts.get_mut(&handle).unwrap();
            let last = context.history.last_mut().unwrap();
            let data = prepend_history_snapshot(&HistorySnapshot { position, numbers, strings }, &last.data)?;
            last.data = data;
            return Ok(NativeReturn::Continue);
        }
        if (requested as i32) < 0 { return Ok(NativeReturn::Value(0)); }
        let history = &self.cst_contexts[&handle].history;
        let mut found = None;
        if !history.is_empty() {
            for actual in (0..=usize::min(requested as usize, history.len() - 1)).rev() {
                let blocks = history_blocks(&history[actual].data)?;
                if let Some(block) = blocks.into_iter().find(|block| {
                    actual as u32 != requested || block.position as i32 <= position as i32
                }) {
                    found = Some((actual as u32, block));
                    break;
                }
            }
        }
        let Some((actual, block)) = found else { return Ok(NativeReturn::Value(0)); };
        let (history_address, position_address) = outputs.unwrap();
        // Preserve the original output order, including overlapping outputs,
        // before applying saved numeric and configured string variables.
        vm.write(history_address, &actual.to_le_bytes())?;
        vm.write(position_address, &block.position.to_le_bytes())?;
        let snapshot = block.decode()?;
        if let Some(values) = self.shared_values_for_mut(self.current_task_id) {
            for (id, value) in snapshot.numbers {
                if let Some(target) = values.get_mut(id as usize) { *target = value; }
            }
        }
        for id in strings {
            self.string_values.insert(id, snapshot.strings.get(&id).cloned().unwrap_or_default());
        }
        Ok(NativeReturn::Value(1))
    }

    fn trim_cst_snapshots(&mut self, handle: u32, requested: u32, position: u32) -> Result<NativeReturn> {
        let context = self.cst_contexts.get_mut(&handle).unwrap();
        if !context.history.is_empty() {
            let selected = usize::min(requested as usize, context.history.len() - 1);
            let data = &context.history[selected].data;
            if let Some(block) = history_blocks(data)?.into_iter()
                .find(|block| (block.position as i32) < position as i32)
            {
                if block.offset != 0 {
                    let remaining = data[block.offset..].to_vec();
                    // 0x509260 updates the LAST history item before truncation.
                    context.history.last_mut().unwrap().data = remaining;
                }
            }
            context.history[selected].value = position;
        }
        let delete = requested.wrapping_add(1);
        if delete != u32::MAX {
            let start = if (delete as i32) < 0 { 0 } else { delete as usize };
            context.history.truncate(start);
        }
        Ok(NativeReturn::Continue)
    }

    fn cst_snapshot_string_ids(&self, name: &[u8]) -> Result<Vec<u32>> {
        let Some((_, bytes)) = self.cst_resource(b"config", name)? else { return Ok(Vec::new()); };
        if bytes.is_empty() { return Ok(Vec::new()); }
        let xml = ConfigXml::parse_runtime(&bytes)
            .map_err(|_| native_state_error("CST snapshot configuration XML is malformed"))?;
        let text = trim_nul(xml.text(&[xml.root.as_slice(), b"localstr"]).unwrap_or_default());
        if text.len() >= 2048 { return Err(native_state_error("CST snapshot localstr exceeds 2047 bytes")); }
        Ok(text.split(|&byte| byte == b',').filter(|part| !part.is_empty())
            .map(|part| integer_prefix(part).map_or(0, |(_, value)| value)).collect())
    }

    pub(super) fn cst_resource(&self, category: &[u8], name: &[u8]) -> Result<Option<(String, Vec<u8>)>> {
        let name = ScriptTextEncoding::Cp932.decode(name);
        let candidates = self.resources.category_names(category, &name)
            .map_err(|_| native_state_error("CST resource category resolution failed"))?;
        for name in candidates {
            if let Some(bytes) = self.cst_read_resource(&name)? { return Ok(Some((name, bytes))); }
        }
        Ok(None)
    }

    fn cst_read_resource(&self, name: &str) -> Result<Option<Vec<u8>>> {
        if let Some(bytes) = self.data_services.ordinary.cached_resource(name) {
            if bytes.len() > MAX_BYTES { return Err(native_state_error("CST cached resource exceeds 64 MiB")); }
            return Ok(Some(bytes.to_vec()));
        }
        let Some(found) = self.resources.resolve_mode_utf16(
            &name.encode_utf16().collect::<Vec<_>>(), ResourceMode::LooseFirst,
        ).map_err(|_| native_state_error("CST resource routing failed"))? else { return Ok(None); };
        if found.value.size > MAX_BYTES as u64 { return Err(native_state_error("CST resource exceeds 64 MiB")); }
        let bytes = self.resources.read_candidate(found.value)
            .map_err(|_| native_state_error("CST resource read failed"))?;
        if bytes.len() > MAX_BYTES { return Err(native_state_error("CST resource exceeds 64 MiB")); }
        Ok(Some(bytes))
    }
}

fn formatted(vm: &Vm, address: u32, capacity: usize) -> Result<Vec<u8>> {
    let bytes = expand_kcs_format(vm, &vm.read_string(address)?, 0)?;
    let text = trim_nul(&bytes);
    if text.len() >= capacity { return Err(native_state_error("CST formatted name exceeds fixed buffer")); }
    Ok(text.to_vec())
}

/// Untrimmed, signed decimal/hex prefix with DWORD wrapping. The caller
/// decides whether an unparseable value means a missing marker or numeric zero.
pub(super) fn integer_prefix(bytes: &[u8]) -> Option<(usize, u32)> {
    let negative = bytes.first() == Some(&b'-');
    let mut cursor = usize::from(matches!(bytes.first(), Some(b'+' | b'-')));
    let hex = bytes.get(cursor..)?.starts_with(b"0x") || bytes.get(cursor..)?.starts_with(b"0X");
    if hex { cursor += 2; }
    let start = cursor;
    let mut value = 0u32;
    while let Some(digit) = bytes.get(cursor).and_then(|byte| match *byte {
        b'0'..=b'9' => Some(u32::from(*byte - b'0')),
        b'a'..=b'f' if hex => Some(u32::from(*byte - b'a' + 10)),
        b'A'..=b'F' if hex => Some(u32::from(*byte - b'A' + 10)),
        _ => None,
    }) {
        value = value.wrapping_mul(if hex { 16 } else { 10 }).wrapping_add(digit);
        cursor += 1;
    }
    if cursor == start || (!hex && bytes.get(cursor) == Some(&b'.')) { return None; }
    Some((cursor, if negative { value.wrapping_neg() } else { value }))
}

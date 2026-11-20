//! CST scene loading/localization and history variable snapshots.
//! Original binaries are used only as static byte evidence, never executed.
use std::borrow::Cow;
use std::collections::{BTreeMap, VecDeque};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use cs2_formats::{ConfigXml, CstlTable};
use cs2_runtime::{ResourceIndex, ResourceLocation};
use cs2_runtime::resource_router::ResourceMode;
use cs2_script::{CstDocument, NativeReturn, Result, ScriptError, Vm};
use cs2_script::cst_history::{HistorySnapshot, history_blocks, prepend_history_snapshot};

use super::{LogicEvent, LoggingHost, ScriptTextEncoding, native_state_error, native_state_error_owned, read_u32, trim_nul,
    expand_kcs_format, signed_cst_index, write_kcs_terminated};
use super::resource_preload::SourcedResource;
use super::particle_plugin_digest::sha256;
use super::{HostPhase, measure_host_operation, measure_host_phase};

const MAX_BYTES: usize = 64 * 1024 * 1024;
const MAX_ITEMS: usize = 1_000_000;
const NEKOPARA_PE_SHA256: &str = "c71c5b85e6eca660474f156d96b0021a53a5f05690dd077d7861d72ff5b11adc";
const MAX_BODY_SOURCES: usize = 64;
static NEXT_BODY_LOAD: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct CstBodySource {
    pub(super) context_handle: u32,
    pub(super) load_id: u64,
    pub(super) group: i32,
    pub(super) absolute_record: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CstSourceProbe {
    Eden,
    Kanmi,
    August,
}

impl CstSourceProbe {
    fn accepts(self, native: u32, group: i32, record: usize, kind: u8) -> bool {
        matches!((self, group, record, kind),
            (Self::Eden, 4, 101, 0x21) | (Self::Eden, 4, 102, 0x20)
            | (Self::Kanmi, 0, 40, 0x20) | (Self::Kanmi, 1, 42, 0x20))
            || self == Self::August && native == 1037
                && matches!((group, record, kind), (0, 78, 0x21) | (0, 79 | 80, 0x20))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct CstSourceRecordObservation {
    pub(super) source: CstBodySource,
    pub(super) frame: u64,
    pub(super) probe: CstSourceProbe,
    pub(super) native: u32,
    pub(super) kind: u8,
    pub(super) stored_codec: ScriptTextEncoding,
    pub(super) output_codec: ScriptTextEncoding,
    pub(super) raw_bytes: usize,
    pub(super) output_bytes: usize,
    pub(super) body_tagged: bool,
    pub(super) localized: bool,
}

struct CstSourceProbeLoad {
    probe: CstSourceProbe,
    load_id: u64,
    records: Vec<CstSourceRecordObservation>,
}

pub(super) fn join_body_sources(prefix: &[CstBodySource], suffix: &[CstBodySource]) -> Option<Vec<CstBodySource>> {
    if prefix.len() + suffix.len() > MAX_BODY_SOURCES { return None; }
    if let (Some(previous), Some(next)) = (prefix.last(), suffix.first())
        && (previous.context_handle != next.context_handle || previous.load_id != next.load_id
            || previous.group != next.group || previous.absolute_record >= next.absolute_record)
    {
        return None;
    }
    Some(prefix.iter().chain(suffix).copied().collect())
}

pub(super) fn body_control_only(mut bytes: &[u8]) -> bool {
    while !bytes.is_empty() {
        if let Some(rest) = bytes.strip_prefix(b"\\n").or_else(|| bytes.strip_prefix(b"\\r")) {
            bytes = rest;
        } else if let Some(rest) = bytes.strip_prefix(b"\\p") {
            bytes = if matches!(rest.first(), Some(b'c' | b'l' | b'r')) { &rest[1..] } else { rest };
        } else {
            return false;
        }
    }
    true
}

struct CstByteOrigin {
    bytes: Vec<u8>,
    codec: ScriptTextEncoding,
    body: Vec<CstBodySource>,
}

#[derive(Default)]
pub(super) struct CstTextOrigins {
    returned: VecDeque<((u32, u32), CstByteOrigin)>,
    shared: BTreeMap<u32, CstByteOrigin>,
    body_loads: BTreeMap<u32, u64>,
    source_probes: BTreeMap<u32, CstSourceProbeLoad>,
}

impl CstTextOrigins {
    pub(super) fn source_record_observations(&self, handle: u32) -> &[CstSourceRecordObservation] {
        self.source_probes.get(&handle)
            .filter(|load| self.body_loads.get(&handle) == Some(&load.load_id))
            .map_or(&[], |load| load.records.as_slice())
    }

    fn note_source_record(&mut self, observation: CstSourceRecordObservation) -> bool {
        let source = observation.source;
        let Some(load) = self.source_probes.get_mut(&source.context_handle) else { return false; };
        if load.load_id != source.load_id || load.probe != observation.probe
            || self.body_loads.get(&source.context_handle) != Some(&source.load_id)
            || !load.probe.accepts(observation.native, source.group, source.absolute_record, observation.kind)
        {
            return false;
        }
        if let Some(current) = load.records.iter_mut().find(|current| current.source == source) {
            *current = observation;
            return false;
        }
        load.records.push(observation);
        true
    }

    /// Refresh native 278's encoding. Only a proven no-op preserves the input body tag.
    pub(super) fn record(&mut self, task: u32, address: u32, bytes: &[u8], codec: ScriptTextEncoding) {
        let body = self.returned_origin(task, address, bytes)
            .filter(|origin| origin.codec == codec && self.body_is_current(&origin.body)
                && !bytes.windows(4).any(|marker| marker == b"$str"))
            .map_or_else(Vec::new, |origin| origin.body.clone());
        self.record_with_body(task, address, bytes, codec, body);
    }

    fn record_with_body(&mut self, task: u32, address: u32, bytes: &[u8], codec: ScriptTextEncoding,
        body: Vec<CstBodySource>,
    ) {
        let key = (task, address);
        self.returned.retain(|(old, ..)| *old != key);
        if self.returned.len() == 64 {
            self.returned.pop_front();
        }
        self.returned.push_back((key, CstByteOrigin { bytes: bytes.to_vec(), codec, body }));
    }

    pub(super) fn invalidate_returned(&mut self, task: u32, address: u32) {
        self.returned.retain(|(key, ..)| *key != (task, address));
    }

    fn returned_origin(&self, task: u32, address: u32, bytes: &[u8]) -> Option<&CstByteOrigin> {
        self.returned.iter().find_map(|(key, origin)| {
            (*key == (task, address) && origin.bytes == bytes).then_some(origin)
        })
    }

    pub(super) fn returned(&self, task: u32, address: u32, bytes: &[u8]) -> Option<ScriptTextEncoding> {
        self.returned_origin(task, address, bytes).map(|origin| origin.codec)
    }

    pub(super) fn publish(&mut self, task: u32, address: u32, id: u32, bytes: &[u8]) {
        if let Some(origin) = self.returned_origin(task, address, bytes)
            .filter(|origin| !bytes.is_ascii() || !origin.body.is_empty())
        {
            let origin = CstByteOrigin { bytes: bytes.to_vec(), codec: origin.codec, body: origin.body.clone() };
            self.shared.insert(id, origin);
        } else {
            self.shared.remove(&id);
        }
    }

    pub(super) fn shared(&self, id: u32, bytes: &[u8]) -> Option<ScriptTextEncoding> {
        self.shared.get(&id).and_then(|origin| (origin.bytes == bytes && !bytes.is_ascii()).then_some(origin.codec))
    }

    pub(super) fn shared_body(&self, id: u32, bytes: &[u8]) -> Option<&[CstBodySource]> {
        let origin = self.shared.get(&id)?;
        (origin.bytes == bytes && self.body_is_current(&origin.body)).then_some(origin.body.as_slice())
    }

    pub(super) fn body_is_current(&self, sources: &[CstBodySource]) -> bool {
        !sources.is_empty() && sources.iter().all(|source|
            self.body_loads.get(&source.context_handle) == Some(&source.load_id))
    }

    pub(super) fn invalidate_shared(&mut self, id: u32) {
        self.shared.remove(&id);
    }

    fn capture_raw_copy(&self, task: u32, target: u32, source: u32, append: bool, vm: &Vm)
        -> Option<CstRawCopy>
    {
        if !self.returned.iter().any(|((owner, address), _)|
            *owner == task && (*address == source || append && *address == target))
        {
            return None;
        }
        let suffix = vm.read_string(source).ok()?;
        let prefix = if append { vm.read_string(target).ok()? } else { Vec::new() };
        let prefix_codec = self.returned(task, target, &prefix);
        let suffix_codec = self.returned(task, source, &suffix);
        let body_part = |address, bytes: &[u8]| {
            if let Some(origin) = self.returned_origin(task, address, bytes)
                && self.body_is_current(&origin.body)
            {
                return Some(origin.body.as_slice());
            }
            body_control_only(bytes).then_some(&[][..])
        };
        let body = body_part(target, &prefix).zip(body_part(source, &suffix))
            .and_then(|(prefix, suffix)| join_body_sources(prefix, suffix)).unwrap_or_default();
        let codec = match (prefix.is_ascii(), suffix.is_ascii()) {
            (true, false) => suffix_codec?,
            (false, true) => prefix_codec?,
            (false, false) if prefix_codec == suffix_codec => prefix_codec?,
            (true, true) if !body.is_empty() => suffix_codec.or(prefix_codec)?,
            _ => return None,
        };
        Some(CstRawCopy { prefix, suffix, codec, body })
    }

    fn finish_copy(&mut self, task: u32, target: u32, copy: Option<CstRawCopy>, vm: &Vm) {
        self.invalidate_returned(task, target);
        if let Some(copy) = copy
            && let Ok(bytes) = vm.read_string(target)
            && copy.prefix.iter().chain(&copy.suffix).eq(bytes.iter())
        {
            self.record_with_body(task, target, &bytes, copy.codec, copy.body);
        }
    }
}

struct CstRawCopy {
    prefix: Vec<u8>,
    suffix: Vec<u8>,
    codec: ScriptTextEncoding,
    body: Vec<CstBodySource>,
}

const SSCRIPT_SPEAKER_COPY_NEXT_PC: usize = 0x1476;
const SSCRIPT_SPEAKER_SCRATCH: u32 = 0x8000_176c;

fn capture_sscript_speaker_copy(source: u32, vm: &Vm) -> Option<CstRawCopy> {
    const SSCRIPT_CODE_SHA256: &str = "3024880f72c31051d071f6cfb6c1b4125086de3b355691a903293e4dfb826fa6";
    if vm.instruction_pointer != SSCRIPT_SPEAKER_COPY_NEXT_PC
        || !matches!(source.checked_sub(SSCRIPT_SPEAKER_SCRATCH), Some(0 | 3))
    {
        return None;
    }
    let suffix = vm.read_string(source).ok()?;
    if suffix.is_ascii() || std::str::from_utf8(&suffix).is_err()
        || vm.program_code_sha256()? != SSCRIPT_CODE_SHA256
    {
        return None;
    }
    Some(CstRawCopy { prefix: Vec::new(), suffix, codec: ScriptTextEncoding::Utf8, body: Vec::new() })
}

pub(super) fn verified_newton_text_profile(resources: &ResourceIndex) -> bool {
    let Some(config) = resources.find("config/startup.xml").filter(|candidate| candidate.size == 39_216)
    else { return false; };
    let Ok(config) = resources.read_candidate(config) else { return false; };
    if sha256(&config) != "8019386a793415a0520a7182fa8c9b4e4e4bec20e9e4caaf6e509f503ce64f3f" {
        return false;
    }
    let Ok(names) = resources.category_names(b"kcs", "sscript") else { return false; };
    let Some(script) = names.iter().find_map(|name| resources.find(name))
        .filter(|candidate| candidate.size == 145_552) else { return false; };
    resources.read_candidate(script).is_ok_and(|bytes|
        sha256(&bytes) == "1f581def5d97d2f8a5ecb465f4ee5803fa30788d7c8fba0852b4b26ce769c5c4")
}

fn newton_loose_scene(root: &Path, source: &SourcedResource) -> bool {
    let ResourceLocation::File { path } = &source.source.location else { return false; };
    let Ok(relative) = path.strip_prefix(root) else { return false; };
    relative.parent().is_some_and(|parent| parent.eq(Path::new("scene")))
        && relative.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("cst"))
}

fn verified_gbk_scene(root: &Path, resource: &SourcedResource) -> bool {
    let relative_matches = |path: &Path, expected: &str| {
        path.strip_prefix(root).ok().is_some_and(|relative|
            relative.to_string_lossy().replace('\\', "/").eq_ignore_ascii_case(expected))
    };
    let digest = match &resource.source.location {
        ResourceLocation::File { path } if relative_matches(path, "scene/com_001A.cst") =>
            "91e7241ee4dc6b0c36d697913cf22bb9649d7ea5b924b88e199a3bcb6e8f4e5b",
        ResourceLocation::Archive { path, .. } if relative_matches(path, "update03.int")
            && resource.source.name.eq_ignore_ascii_case("prologue.cst") =>
            "4f713b09acbe41fd0d19e072eb3d5f89d3ee34854904aa8300640f0ba00e05e8",
        ResourceLocation::Archive { path, .. } if relative_matches(path, "scene.int")
            && resource.source.name.eq_ignore_ascii_case("aft_002.cst") =>
            "0d5670492bf927f91d358bcc4b0d857bac7ebd786c3dafa7b5c56fd638cb098a",
        ResourceLocation::Archive { path, .. } if relative_matches(path, "scene.int")
            && resource.source.name.eq_ignore_ascii_case("BLAOP1_END.cst") =>
            "c09bcb90152977cd9d21b68ec019d627ccc80136036503a5ce376a2ae6020561",
        ResourceLocation::Archive { path, .. } if relative_matches(path, "scene.int")
            && resource.source.name.eq_ignore_ascii_case("A_01_01.cst") =>
            "92de81ed6532c41611106715b4d8185cd5210ff0b9a177586e1d4d9f1b839176",
        ResourceLocation::Archive { path, .. } if relative_matches(path, "update01.int")
            && resource.source.name.eq_ignore_ascii_case("op.cst") =>
            "2596c7c430dabed8e1e3144c50c4b8672b59b1d452e4b92b7e18cf0c9f409817",
        ResourceLocation::Archive { path, .. } if relative_matches(path, "update05.int")
            && resource.source.name.eq_ignore_ascii_case("common00.cst") =>
            "dea050b2a240e487c5ceed1aca21cfa6e1fb985b71a3f856b1a5aea0f92a51c3",
        ResourceLocation::Archive { path, .. } if relative_matches(path, "update05.int")
            && resource.source.name.eq_ignore_ascii_case("common01.cst") =>
            "80bf853d65d55a8683c3860ab4453973404da781bcca092589a8283d22fa4ac1",
        _ => return false,
    };
    sha256(&resource.bytes) == digest
}

fn require_gbk_records(document: &CstDocument) -> Result<()> {
    for record in &document.records {
        if encoding_rs::GBK.decode_without_bom_handling_and_without_replacement(&record.text).is_none() {
            return Err(native_state_error("verified GBK scene contains a malformed record"));
        }
    }
    Ok(())
}

pub(super) fn decode_cst_dialogue(bytes: &[u8], codec: ScriptTextEncoding) -> Result<Cow<'_, str>> {
    let bytes = trim_nul(bytes);
    let decoded = match codec {
        ScriptTextEncoding::Cp932 => encoding_rs::SHIFT_JIS.decode_without_bom_handling_and_without_replacement(bytes),
        ScriptTextEncoding::Gbk => encoding_rs::GBK.decode_without_bom_handling_and_without_replacement(bytes),
        ScriptTextEncoding::Utf8 => std::str::from_utf8(bytes).ok().map(Cow::Borrowed),
    };
    decoded.ok_or_else(|| native_state_error_owned(format!("CST dialogue has invalid {codec:?} source bytes")))
}

impl LoggingHost<'_> {
    fn cst_source_probe(&self, resource: &SourcedResource) -> Option<CstSourceProbe> {
        let ResourceLocation::Archive { path, .. } = &resource.source.location else { return None; };
        let archive = path.strip_prefix(self.resources.root()).ok()?.to_str()?;
        if !archive.eq_ignore_ascii_case("scene.int") {
            return None;
        }
        if self.css_task_sha() == Some("c2af2ee9baf4b26a3c2cf8a1c867f15f92b54ad1ac5fac381858046a47818f6c")
            && resource.source.name.eq_ignore_ascii_case("BLAOP1_END.cst")
            && sha256(&resource.bytes) == "c09bcb90152977cd9d21b68ec019d627ccc80136036503a5ce376a2ae6020561"
        {
            Some(CstSourceProbe::Eden)
        } else if self.data_services.particle_message_pe_sha256
            == Some("c264b2465889c78723079a694d2654ef1091253db657a735e92f58b75031aa26")
            && resource.source.name.eq_ignore_ascii_case("sc01.cst")
            && sha256(&resource.bytes) == "7c0d796d069db416291effc217aa3664ba449f23ce43b88a11a5d13cfff1a55e"
        {
            Some(CstSourceProbe::Kanmi)
        } else if self.css_task_sha() == Some("4ccb48ad4bb6bfb5a15a45b8e1f2c2e62072cfad9f1487f869561a473770ed94")
            && resource.source.name.eq_ignore_ascii_case("00_001.cst")
            && sha256(&resource.bytes) == "9e4efc626b072ff3de336665e3f23bb993e9da4f79f9a59e5bee5ba183b8fe78"
        {
            Some(CstSourceProbe::August)
        } else {
            None
        }
    }

    pub(super) fn cst_string_copy_native(&mut self, index: u32, args: &[u8], vm: &mut Vm) -> Result<NativeReturn> {
        let packet = if args.len() >= 8 { args } else { vm.native_argument_storage(8)? };
        let target = read_u32(packet, 0)?;
        let source = read_u32(packet, 4)?;
        let task = self.current_task_id;
        let copy = if matches!(index, 13 | 15) {
            self.data_services.cst_text_origins.capture_raw_copy(task, target, source, index == 15, vm)
        } else { None };
        let speaker_copy = if copy.is_none() && index == 13 {
            capture_sscript_speaker_copy(source, vm)
        } else { None };
        let trace_len = speaker_copy.as_ref().map(|source| source.suffix.len());
        let result = self.string_copy_native(index, args, vm);
        let origins = &mut self.data_services.cst_text_origins;
        origins.finish_copy(task, target, if result.is_ok() { copy.or(speaker_copy) } else { None }, vm);
        if self.data_services.story_trace && let Some(length) = trace_len
            && let Ok(bytes) = vm.read_string(target)
            && origins.returned(task, target, &bytes) == Some(ScriptTextEncoding::Utf8)
        {
            self.events.push(super::LogicEvent {
                frame: self.frame,
                kind: "cst-text-sourced-copy".into(),
                detail: format!(
                    "task={task} native=13 next_pc={:#x} scratch_offset={} bytes={length} source=sscript-speaker/Utf8",
                    vm.instruction_pointer, source - SSCRIPT_SPEAKER_SCRATCH,
                ),
            });
        }
        result
    }

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
        let Some((resolved, resource)) = self.cst_resource_with_source(b"scene", &name)? else {
            return Ok(NativeReturn::Value(0));
        };
        let bytes = &resource.bytes;
        if bytes.is_empty() { return Ok(NativeReturn::Value(0)); }
        let signature = bytes.get(..8) == Some(b"CatScene");
        let valid_length = if signature && bytes.len() >= 16 {
            let packed = read_u32(bytes, 8)? as usize;
            let plain = read_u32(bytes, 12)? as usize;
            16usize.checked_add(if packed == 0 { plain } else { packed }) == Some(bytes.len())
        } else { false };
        // A wrong signature keeps the old scene; a recognized container with
        // rejected outer length clears only its scene, not its history/position.
        let mut encoding = self.text_encoding;
        let loaded = if signature && valid_length {
            let document = measure_host_operation(HostPhase::ScriptParse, || CstDocument::parse(bytes))?;
            if let Some(source_encoding) = self.cst_scene_encoding_override(&resource) {
                require_gbk_records(&document)?;
                encoding = source_encoding;
            }
            Some(document)
        } else { None };
        let success = loaded.is_some();
        if signature {
            self.data_services.cst_text_origins.body_loads.remove(&handle);
            self.data_services.cst_text_origins.source_probes.remove(&handle);
        }
        {
            let context = self.cst_contexts.get_mut(&handle)
                .ok_or_else(|| native_state_error("CST context handle is invalid"))?;
            if signature {
                context.document = loaded;
                if success {
                    context.position = [0, 0];
                    context.encoding = encoding;
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
        if let Some(SourcedResource { bytes, source }) = self.cst_read_resource(&adjacent)? {
            if !bytes.is_empty() {
                let (table, _) = measure_host_operation(HostPhase::ScriptParse, || CstlTable::parse_prefix(&bytes))
                    .map_err(|_| native_state_error("CST adjacent localization table is malformed"))?;
                if success && self.trace_neko_cstl() {
                    let origin = match &source.location {
                        ResourceLocation::File { .. } => source.name.replace('\\', "/"),
                        ResourceLocation::Archive { path, .. } => format!("{}::{}",
                            path.file_name().unwrap_or(path.as_os_str()).to_string_lossy(), source.name),
                    };
                    self.events.push(LogicEvent {
                        frame: self.frame,
                        kind: "neko-cstl-load".into(),
                        detail: format!("handle={handle} requested={adjacent:?} source={origin:?} sha256={} columns={:?}",
                            sha256(&bytes), table.languages),
                    });
                }
                self.cst_contexts.get_mut(&handle).unwrap().localization = Some(table);
            }
        }
        if self.data_services.story_trace && signature {
            if success {
                let probe = self.cst_source_probe(&resource);
                let origins = &mut self.data_services.cst_text_origins;
                origins.body_loads.retain(|handle, _| self.cst_contexts.contains_key(handle));
                origins.source_probes.retain(|handle, _| self.cst_contexts.contains_key(handle));
                let load_id = NEXT_BODY_LOAD.fetch_add(1, Ordering::Relaxed);
                origins.body_loads.insert(handle, load_id);
                if let Some(probe) = probe {
                    origins.source_probes.insert(handle, CstSourceProbeLoad {
                        probe, load_id, records: Vec::new(),
                    });
                }
            }
            let context = self.cst_contexts.get_mut(&handle).unwrap();
            context.story = if success {
                super::story_trace::CstStoryPosition::loaded(&name, self.frame)
            } else {
                Default::default()
            };
        }
        Ok(NativeReturn::Value(u32::from(success)))
    }

    pub(super) fn read_cst_record(
        &mut self, index: u32, args: &[u8], vm: &mut Vm,
    ) -> Result<NativeReturn> {
        let handle = read_u32(args, 0)?;
        let context = self.cst_contexts.get(&handle)
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
        let localized = bound.filter(|_| matches!(record.kind, 0x20 | 0x21));
        let codec = if localized.is_some() || matches!(index, 1037 | 1038) {
            ScriptTextEncoding::Utf8
        } else {
            context.encoding
        };
        let text = if let Some((table, column)) = localized {
            let message = table.messages.get(input as usize).and_then(|row| row.get(column));
            if record.kind == 0x21 {
                message.map_or(&b""[..], |message| message.name.as_bytes()).to_vec()
            } else {
                let fragment = records[..ordinal as usize].iter().filter(|r| r.kind == 0x20).count();
                message.and_then(|message| message.content.split('\u{1f}').nth(fragment))
                    .unwrap_or("").as_bytes().to_vec()
            }
        } else if matches!(index, 1037 | 1038) {
            if matches!(record.kind, 0x20 | 0x21) {
                decode_cst_dialogue(&record.text, context.encoding).map_err(|error|
                    native_state_error_owned(format!("native {index} input={input} record={ordinal}: {error}")))?
                    .into_owned().into_bytes()
            } else {
                context.encoding.decode(&record.text).into_bytes()
            }
        } else { record.text.clone() };
        let text = trim_nul(&text);
        write_kcs_terminated(vm, target, text, 1)?;
        if index == 1038 && (0..=2).contains(&input) && matches!(record.kind, 0x20 | 0x21)
            && self.trace_neko_cstl()
        {
            let languages = &self.data_services.language.languages;
            let ui = languages.prefix(0).map(String::from_utf8_lossy);
            let main = languages.prefix(1).map(String::from_utf8_lossy);
            let requested = language.as_deref().map(String::from_utf8_lossy);
            let column = localized.map(|(_, column)| column);
            let source = if column.is_some() { "cstl" } else { "raw-cst" };
            self.events.push(LogicEvent {
                frame: self.frame,
                kind: "neko-cstl-read".into(),
                detail: format!("handle={handle} input={input} ordinal={ordinal} kind={:#04x} destination={target:#010x} ui={ui:?} main={main:?} requested={requested:?} source={source} column={column:?} bytes={} sha256={}",
                    record.kind, text.len(), sha256(text)),
            });
        }
        if matches!(record.kind, 0x20 | 0x21) {
            let main_prefix = self.data_services.language.languages.prefix(1);
            let origins = &mut self.data_services.cst_text_origins;
            let load = origins.body_loads.get(&handle).copied().filter(|_| {
                record.kind == 0x20 && !body_control_only(text)
                    && language.as_deref().is_none_or(|requested|
                        main_prefix == Some(requested)
                            || (main_prefix.is_none() && requested.is_empty() && localized.is_none()))
            });
            let absolute_record = document.inputs[input as usize].start + ordinal as usize;
            if let Some(load_id) = load {
                origins.record_with_body(self.current_task_id, target, text, codec, vec![CstBodySource {
                    context_handle: handle, load_id, group: input, absolute_record,
                }]);
            } else {
                origins.record_with_body(self.current_task_id, target, text, codec, Vec::new());
            }
            if let Some(&(probe, load_id)) = origins.source_probes.get(&handle)
                .map(|load| (load.probe, load.load_id)).as_ref()
            {
                let observation = CstSourceRecordObservation {
                    source: CstBodySource { context_handle: handle, load_id, group: input, absolute_record },
                    frame: self.frame, probe, native: index, kind: record.kind,
                    stored_codec: context.encoding, output_codec: codec,
                    raw_bytes: record.text.len(), output_bytes: text.len(),
                    body_tagged: load == Some(load_id), localized: localized.is_some(),
                };
                if origins.note_source_record(observation) {
                    self.events.push(LogicEvent {
                        frame: self.frame, kind: "cst-source-record".into(), detail: format!("{observation:?}"),
                    });
                }
            }
        } else {
            self.data_services.cst_text_origins.invalidate_returned(self.current_task_id, target);
        }
        Ok(NativeReturn::Value(text.len() as u32))
    }

    fn trace_neko_cstl(&self) -> bool {
        self.data_services.story_trace
            && self.data_services.native_profile.is_some_and(|profile| profile.pe_sha256 == NEKOPARA_PE_SHA256)
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
            self.data_services.cst_text_origins.invalidate_shared(id);
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
        Ok(self.cst_resource_with_source(category, name)?
            .map(|(name, resource)| (name, resource.bytes)))
    }

    pub(super) fn cst_scene_encoding_override(&self, resource: &SourcedResource) -> Option<ScriptTextEncoding> {
        let island = matches!(&resource.source.location, ResourceLocation::Archive { path, .. }
            if self.resources.resource_router().is_some_and(|router|
                router.is_island_chinese_scene(self.resources, path)));
        (island || verified_gbk_scene(self.resources.root(), resource)
            || self.data_services.newton_legacy_gbk && newton_loose_scene(self.resources.root(), resource))
            .then_some(ScriptTextEncoding::Gbk)
    }

    pub(super) fn cst_resource_with_source(&self, category: &[u8], name: &[u8]) -> Result<Option<(String, SourcedResource)>> {
        let name = ScriptTextEncoding::Cp932.decode(name);
        let candidates = self.resources.category_names(category, &name)
            .map_err(|_| native_state_error("CST resource category resolution failed"))?;
        for name in candidates {
            if let Some(resource) = self.cst_read_resource(&name)? { return Ok(Some((name, resource))); }
        }
        Ok(None)
    }

    fn cst_read_resource(&self, name: &str) -> Result<Option<SourcedResource>> {
        let _timing = measure_host_phase(HostPhase::ScriptRead);
        if let Some(resource) = self.data_services.ordinary.cached_resource_with_source(name) {
            if resource.bytes.len() > MAX_BYTES { return Err(native_state_error("CST cached resource exceeds 64 MiB")); }
            return Ok(Some(resource.clone()));
        }
        let Some(found) = self.resources.resolve_mode_utf16(
            &name.encode_utf16().collect::<Vec<_>>(), ResourceMode::LooseFirst,
        ).map_err(|_| native_state_error("CST resource routing failed"))? else { return Ok(None); };
        if found.value.size > MAX_BYTES as u64 { return Err(native_state_error("CST resource exceeds 64 MiB")); }
        let bytes = self.resources.read_candidate(found.value)
            .map_err(|_| native_state_error("CST resource read failed"))?;
        if bytes.len() > MAX_BYTES { return Err(native_state_error("CST resource exceeds 64 MiB")); }
        Ok(Some(SourcedResource { source: found.value.clone(), bytes }))
    }
}

#[cfg(test)]
mod source_encoding_tests {
    use super::*;
    use cs2_script::{CstRecord, Host};

    #[test]
    fn august_source_observation_keeps_only_first_page_legacy_reads_from_the_live_load() {
        let mut origins = CstTextOrigins::default();
        let context_handle = 139;
        let load_id = 3;
        let probe = CstSourceProbe::August;
        origins.body_loads.insert(context_handle, load_id);
        origins.source_probes.insert(context_handle, CstSourceProbeLoad {
            probe, load_id, records: Vec::new(),
        });
        let speaker = CstSourceRecordObservation {
            source: CstBodySource { context_handle, load_id, group: 0, absolute_record: 78 },
            frame: 1309, probe, native: 1037, kind: 0x21,
            stored_codec: ScriptTextEncoding::Cp932, output_codec: ScriptTextEncoding::Utf8,
            raw_bytes: 12, output_bytes: 18, body_tagged: false, localized: false,
        };
        let first_body = CstSourceRecordObservation {
            source: CstBodySource { absolute_record: 79, ..speaker.source },
            kind: 0x20, raw_bytes: 58, output_bytes: 87, body_tagged: true, ..speaker
        };
        let second_body = CstSourceRecordObservation {
            source: CstBodySource { absolute_record: 80, ..first_body.source },
            output_bytes: 86, ..first_body
        };
        assert!(!origins.note_source_record(CstSourceRecordObservation { native: 327, ..first_body }));
        assert!(!origins.note_source_record(CstSourceRecordObservation {
            source: CstBodySource { group: 1, absolute_record: 84, ..first_body.source }, ..first_body
        }));
        for observation in [speaker, first_body, second_body] {
            assert!(origins.note_source_record(observation));
        }
        let repeat = CstSourceRecordObservation { frame: 1310, ..first_body };
        assert!(!origins.note_source_record(repeat));
        assert_eq!(origins.source_record_observations(context_handle), &[speaker, repeat, second_body]);
        origins.body_loads.insert(context_handle, load_id + 1);
        assert!(origins.source_record_observations(context_handle).is_empty());
        assert!(!origins.note_source_record(second_body));
    }

    #[test]
    fn cst_text_origin_is_scoped_to_the_producing_task_and_current_bytes() {
        let mut origins = CstTextOrigins::default();
        let gbk = b"\xb7\xc7";
        let utf8 = b"\xe9\x9d\x9e";
        origins.record(4, 0x100, gbk, ScriptTextEncoding::Gbk);
        origins.publish(4, 0x100, 1000, gbk);
        assert_eq!(origins.shared(1000, gbk), Some(ScriptTextEncoding::Gbk));
        assert_eq!(origins.shared(1000, utf8), None);
        origins.publish(5, 0x100, 1000, gbk);
        assert_eq!(origins.shared(1000, gbk), None);
        origins.record(4, 0x100, utf8, ScriptTextEncoding::Utf8);
        origins.publish(4, 0x100, 1000, utf8);
        assert_eq!(origins.shared(1000, utf8), Some(ScriptTextEncoding::Utf8));
        origins.invalidate_returned(4, 0x100);
        origins.publish(4, 0x100, 1000, utf8);
        assert_eq!(origins.shared(1000, utf8), None);
    }

    #[test]
    fn cst_dialogue_decodes_ambiguous_bytes_only_with_declared_codec() {
        let bytes = b"\xc2\xa9\0\x81";
        assert_eq!(decode_cst_dialogue(bytes, ScriptTextEncoding::Cp932).unwrap(), "\u{ff82}\u{ff69}");
        assert_eq!(decode_cst_dialogue(bytes, ScriptTextEncoding::Gbk).unwrap(), "\u{6f0f}");
        assert_eq!(decode_cst_dialogue(bytes, ScriptTextEncoding::Utf8).unwrap(), "\u{a9}");
        for codec in [ScriptTextEncoding::Cp932, ScriptTextEncoding::Gbk, ScriptTextEncoding::Utf8] {
            assert!(decode_cst_dialogue(b"\x81", codec).is_err());
        }
    }

    #[test]
    fn cst_text_origins_survive_message_window_cache_republication() -> Result<()> {
        let task = 8;
        let input = 0x8000_0100;
        let cache = 0x8000_0200;
        let output = 0x8000_0300;
        let mut vm = Vm::new();
        vm.bind_external(vec![0; 4096])?;
        for codec in [ScriptTextEncoding::Utf8, ScriptTextEncoding::Gbk] {
            let mut origins = CstTextOrigins::default();
            let body = b"\xc2\xa9";
            vm.write(input, b"\xc2\xa9\0")?;
            vm.write(cache, b"\\n\\pc\0")?;
            origins.record(task, input, body, codec);
            let append = origins.capture_raw_copy(task, cache, input, true, &vm);
            let combined = b"\\n\\pc\xc2\xa9";
            vm.write(cache, b"\\n\\pc\xc2\xa9\0")?;
            origins.finish_copy(task, cache, append, &vm);

            origins.publish(task, input, 1000, b"");
            let copy = origins.capture_raw_copy(task, output, cache, false, &vm);
            vm.write(output, b"\\n\\pc\xc2\xa9\0")?;
            origins.finish_copy(task, output, copy, &vm);
            origins.publish(task, output, 1000, combined);
            assert_eq!(origins.shared(1000, combined), Some(codec));
            let decoded = crate::fes_text::decode_sourced_text(b"$str1000", combined,
                ScriptTextEncoding::Cp932, ScriptTextEncoding::Cp932, false,
                |id| (combined, origins.shared(id, combined))).unwrap().unwrap();
            let expected = if codec == ScriptTextEncoding::Utf8 { "\u{a9}" } else { "\u{6f0f}" };
            assert_eq!(decoded, format!("\\n\\pc{expected}"));

            assert!(origins.capture_raw_copy(task + 1, output, cache, false, &vm).is_none());
            let copy = origins.capture_raw_copy(task, output, cache, false, &vm);
            vm.write(output, b"changed\0")?;
            origins.finish_copy(task, output, copy, &vm);
            assert_eq!(origins.returned(task, output, combined), None);
            origins.record(task, input, body, ScriptTextEncoding::Cp932);
            assert!(origins.capture_raw_copy(task, cache, input, true, &vm).is_none());
        }
        Ok(())
    }

    #[test]
    fn cst_body_origins_bind_ordered_publish_to_the_current_load() -> Result<()> {
        let task = 8;
        let first = 0x8000_0100;
        let second = 0x8000_0200;
        let cache = 0x8000_0300;
        let mut vm = Vm::new();
        vm.bind_external(vec![0; 4096])?;
        let mut origins = CstTextOrigins::default();
        let first_tag = CstBodySource { context_handle: 9, load_id: 42, group: 1, absolute_record: 20 };
        let second_tag = CstBodySource { absolute_record: 21, ..first_tag };
        origins.body_loads.insert(first_tag.context_handle, first_tag.load_id);
        for (address, text, written, tag) in [
            (first, b"ABC".as_slice(), b"ABC\0".as_slice(), first_tag),
            (second, b"DEF", b"DEF\0", second_tag),
        ] {
            vm.write(address, written)?;
            origins.record_with_body(task, address, text, ScriptTextEncoding::Utf8, vec![tag]);
            origins.record(task, address, text, ScriptTextEncoding::Utf8);
        }
        vm.write(cache, b"\\r\\pc\0")?;
        let copy = origins.capture_raw_copy(task, cache, first, true, &vm);
        vm.write(cache, b"\\r\\pcABC\0")?;
        origins.finish_copy(task, cache, copy, &vm);
        let copy = origins.capture_raw_copy(task, cache, second, true, &vm);
        vm.write(cache, b"\\r\\pcABCDEF\0")?;
        origins.finish_copy(task, cache, copy, &vm);
        let combined = b"\\r\\pcABCDEF";
        origins.publish(task, cache, 1000, combined);
        assert_eq!(origins.shared_body(1000, combined), Some([first_tag, second_tag].as_slice()));
        assert_eq!(origins.shared(1000, combined), None);
        origins.publish(task + 1, cache, 1000, combined);
        assert!(origins.shared_body(1000, combined).is_none());
        origins.publish(task, cache, 1000, combined);
        origins.body_loads.insert(first_tag.context_handle, 43);
        assert!(origins.shared_body(1000, combined).is_none());
        Ok(())
    }

    #[test]
    fn cst_body_origins_drop_replaced_expansions_and_codec_changes() {
        let mut origins = CstTextOrigins::default();
        let tag = CstBodySource { context_handle: 9, load_id: 42, group: 0, absolute_record: 17 };
        origins.body_loads.insert(tag.context_handle, tag.load_id);
        for (input, output, codec) in [
            (b"$str1000".as_slice(), b"$str1000".as_slice(), ScriptTextEncoding::Utf8),
            (b"A$str1000", b"ABC", ScriptTextEncoding::Utf8),
            (b"ABC", b"ABC", ScriptTextEncoding::Cp932),
        ] {
            origins.record_with_body(8, 0x100, input, ScriptTextEncoding::Utf8, vec![tag]);
            origins.record(8, 0x100, output, codec);
            origins.publish(8, 0x100, 1000, output);
            assert!(origins.shared_body(1000, output).is_none());
        }
    }

    #[test]
    fn sidecar9_verified_codec_validation_is_strict() {
        let mut document = CstDocument {
            inputs: vec![0..1],
            records: vec![CstRecord { kind: 0x20, text: vec![0xb7, 0xc7] }],
        };
        assert!(require_gbk_records(&document).is_ok());
        assert_eq!(ScriptTextEncoding::Gbk.decode(&document.records[0].text), "\u{975e}");
        let cp932 = encoding_rs::SHIFT_JIS.decode_without_bom_handling_and_without_replacement(&document.records[0].text).unwrap();
        assert_ne!(cp932, "\u{975e}");
        document.records[0].text.pop();
        assert!(require_gbk_records(&document).is_err());
    }

    fn args(words: &[u32]) -> Vec<u8> {
        words.iter().flat_map(|word| word.to_le_bytes()).collect()
    }

    #[test]
    #[ignore = "requires the installed Grisaia Eden V1 patch and an isolated profile path"]
    fn grisaia_eden_blaop1_end_publishes_gbk_name_and_body() -> anyhow::Result<()> {
        let game = std::env::var_os("CS2_EDEN_TEXT_TEST_ROOT").expect("installed game root required");
        let profile = std::env::var_os("CS2_EDEN_TEXT_TEST_PROFILE").expect("isolated profile required");
        let mut player = crate::HeadlessPlayer::open_with_save_root(
            game, Path::new(&profile).join("eden-blaop1-end"),
        )?;
        let mut vm = Vm::new();
        vm.bind_external(vec![0; 4096])?;
        let scene = b"BLAOP1_END.cst";
        let scene_name = 0x8000_0100;
        let destination = 0x8000_0200;
        vm.write(scene_name, b"BLAOP1_END.cst\0")?;
        player.with_host(|host| {
            let (_, winner) = host.cst_resource_with_source(b"scene", scene)?.unwrap();
            assert!(verified_gbk_scene(host.resources.root(), &winner));
            let mut changed = winner.clone();
            changed.bytes[0] ^= 1;
            assert!(!verified_gbk_scene(host.resources.root(), &changed));
            changed.source.location = ResourceLocation::File {
                path: host.resources.root().join("scene/BLAOP1_END.cst"),
            };
            changed.bytes = winner.bytes;
            assert!(!verified_gbk_scene(host.resources.root(), &changed));

            let NativeReturn::Value(handle) = host.call(323, &[], &mut vm)? else {
                anyhow::bail!("no CST context handle");
            };
            assert_eq!(host.call(325, &args(&[handle, scene_name]), &mut vm)?, NativeReturn::Value(1));
            assert_eq!(host.cst_contexts[&handle].encoding, ScriptTextEncoding::Gbk);
            for (ordinal, id, kind, length) in [(10, 1001, 0x21, 4), (11, 1000, 0x20, 97)] {
                let packet = args(&[handle, destination, 4, ordinal]);
                let record = &host.cst_contexts[&handle].document.as_ref().unwrap()
                    .input_records(4).unwrap()[ordinal as usize];
                assert_eq!(record.kind, kind);
                assert_eq!(record.text.len(), length);
                let expected = decode_cst_dialogue(&record.text, ScriptTextEncoding::Gbk)?.into_owned();
                if kind == 0x20 {
                    assert!(decode_cst_dialogue(&record.text, ScriptTextEncoding::Cp932).is_err());
                }
                host.read_cst_record(327, &packet, &mut vm)?;
                host.call(276, &args(&[id, destination]), &mut vm)?;
                let raw = vm.read_string(destination)?;
                assert_eq!(host.data_services.cst_text_origins.shared(id, &raw), Some(ScriptTextEncoding::Gbk));
                host.read_cst_record(1037, &packet, &mut vm)?;
                assert_eq!(vm.read_string(destination)?, expected.as_bytes());
            }
            Ok(())
        })
    }

    #[test]
    #[ignore = "requires the installed Rebellions A-line patch and an isolated profile path"]
    fn rebellions_a01_gbk_uses_exact_winning_member() -> anyhow::Result<()> {
        let game = std::env::var_os("CS2_REBELLIONS_TEST_GAME").expect("installed game root required");
        let profile = std::env::var_os("CS2_SIDECAR9_PROFILE").expect("isolated profile required");
        let mut player = crate::HeadlessPlayer::open_with_save_root(
            game, Path::new(&profile).join("rebellions-a01-encoding"),
        )?;
        let mut vm = Vm::new();
        vm.bind_external(vec![0; 4096])?;
        let scene_name = 0x8000_0100;
        let destination = 0x8000_0200;
        player.with_host(|host| {
            let NativeReturn::Value(handle) = host.call(323, &[], &mut vm)? else {
                anyhow::bail!("no CST context handle");
            };
            let (_, patched) = host.cst_resource_with_source(b"scene", b"A_01_01.cst")?.unwrap();
            let (_, japanese) = host.cst_resource_with_source(b"scene", b"C_01_01.cst")?.unwrap();
            assert_eq!(host.cst_scene_encoding_override(&patched), Some(ScriptTextEncoding::Gbk));
            assert_eq!(host.cst_scene_encoding_override(&japanese), None);

            let mut same_name = patched.clone();
            same_name.bytes = japanese.bytes;
            assert_eq!(host.cst_scene_encoding_override(&same_name), None);
            let mut loose = patched;
            loose.source.location = ResourceLocation::File {
                path: host.resources.root().join("scene/A_01_01.cst"),
            };
            assert_eq!(host.cst_scene_encoding_override(&loose), None);

            for (name, ordinal, codec, expected) in [
                (b"A_01_01.cst\0", 12, ScriptTextEncoding::Gbk,
                    "\u{2015}\u{2015}\u{4ed6}\u{505a}\u{4e86}\u{4e00}\u{4e2a}\u{68a6}\u{3002}"),
                (b"C_01_01.cst\0", 16, ScriptTextEncoding::Cp932,
                    "\u{2015}\u{2015}\u{5f7c}\u{306f}\u{3001}\u{5922}\u{3092}\u{898b}\u{3066}\u{3044}\u{305f}\u{3002}"),
            ] {
                vm.write(scene_name, name)?;
                assert_eq!(host.call(325, &args(&[handle, scene_name]), &mut vm)?, NativeReturn::Value(1));
                assert_eq!(host.cst_contexts[&handle].encoding, codec);
                let read_args = args(&[handle, destination, 0, ordinal]);
                host.read_cst_record(327, &read_args, &mut vm)?;
                let raw = vm.read_string(destination)?;
                host.call(278, &args(&[destination]), &mut vm)?;
                host.call(276, &args(&[1000, destination]), &mut vm)?;
                assert_eq!(host.data_services.cst_text_origins.shared(1000, &raw), Some(codec));
                host.read_cst_record(1037, &read_args, &mut vm)?;
                assert_eq!(vm.read_string(destination)?, expected.as_bytes());
            }
            Ok(())
        })
    }

    #[test]
    #[ignore = "requires the pinned installed Cyanotype game and an isolated profile path"]
    fn cyanotype_speaker_copy_publishes_utf8_to_fes() -> anyhow::Result<()> {
        let game = std::env::var_os("CS2_SIDECAR9_CYANOTYPE").expect("installed game root required");
        let profile = std::env::var_os("CS2_SIDECAR9_PROFILE").expect("isolated profile required");
        let run_id = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos();
        let save_root = Path::new(&profile).join(format!("cyanotype-f711-{}-{run_id}", std::process::id()));
        let mut player = crate::HeadlessPlayer::open_with_save_root(game, save_root)?;
        player.enable_host_window_requests();
        player.initialize_general_window_host(1)?;
        player.set_host_input_update_rate(60);
        player.enable_host_system_open_queue();
        player.enable_host_tweet_queue();
        player.enable_host_dialog_queue();
        player.enable_host_story_trace();
        player.load_startup()?;
        player.apply_host_input(cs2_runtime::InputEvent::FocusChanged(true))?;
        for frame in 1..=711 {
            let mut advanced = false;
            for _ in 0..128 {
                advanced = anyhow::Context::with_context(player.advance_host_frame(20_000), || {
                    let error = player.events().iter().rev().find(|event| event.kind == "task-error");
                    format!("Cyanotype f{frame}: {}", error.map_or("no task error event", |event| event.detail.as_str()))
                })?;
                if advanced { break; }
            }
            anyhow::ensure!(advanced, "Cyanotype stalled at f{frame}");
            player.limit_host_diagnostics(10_000, 1_000);
        }
        assert_eq!(player.host_story_trace().2, "prologue");
        let mut events = player.events().iter().filter(|event| event.frame == 711);
        assert!(events.any(|event| event.kind == "cst-text-sourced-copy"
            && event.detail.ends_with("source=sscript-speaker/Utf8")));
        assert!(events.any(|event| event.kind == "fes-text-sourced-queued"
            && event.detail.contains("cst=[Utf8]")));
        player.with_host(|host| {
            assert!(host.data_services.language.languages.prefix(1).is_none());
            let (owner, _, _, _) = host.data_services.tasks.fes.story_runtimes()
                .find(|(_, name, _, _)| *name == b"cinema.fes")
                .ok_or_else(|| anyhow::anyhow!("Cyanotype f711 cinema receiver missing"))?;
            let text = &host.data_services.fes_text;
            let generation = text.story_body_generation(owner)
                .ok_or_else(|| anyhow::anyhow!("Cyanotype f711 cinema body generation missing"))?;
            assert_eq!(generation.source_frame, 711);
            let sources = text.story_body_sources(owner, generation.source_epoch, &host.data_services.cst_text_origins)
                .ok_or_else(|| anyhow::anyhow!("Cyanotype f711 cinema body source missing"))?;
            let [source] = sources else { anyhow::bail!("Cyanotype f711 expected one cinema body source"); };
            assert_eq!((source.group, source.absolute_record), (0, 17));
            let (_, bytes) = host.cst_resource(b"scene", b"prologue")?
                .ok_or_else(|| anyhow::anyhow!("Cyanotype prologue resource missing"))?;
            let prologue = CstDocument::parse(&bytes)?;
            assert_eq!(host.cst_contexts[&source.context_handle].document.as_ref(), Some(&prologue));
            Ok(())
        })
    }

    fn check_scene(game: &str, filename: &[u8], records: &[usize], fallback: &str) -> anyhow::Result<()> {
        let root = std::env::var_os(game).expect("installed game root required");
        let profile = std::env::var_os("CS2_SIDECAR9_PROFILE").expect("isolated profile required");
        let mut player = crate::HeadlessPlayer::open_with_save_root(root, Path::new(&profile).join(game))?;
        let mut vm = Vm::new();
        vm.bind_external(vec![0; 4096])?;
        let scene_name = 0x8000_0100;
        let category = 0x8000_0200;
        let unbound_language = 0x8000_0300;
        let destination = 0x8000_0400;
        vm.write(scene_name, filename)?;
        vm.write(category, b"scene\0")?;
        vm.write(unbound_language, b"sidecar9-unbound\0")?;
        let (handle, virtual_name) = player.with_host(|host| {
            let NativeReturn::Value(handle) = host.call(323, &[], &mut vm)? else {
                anyhow::bail!("no CST context handle");
            };
            let (virtual_name, winner) = host.cst_resource_with_source(b"scene", trim_nul(filename))?.unwrap();
            assert!(verified_gbk_scene(host.resources.root(), &winner));
            let mut changed = winner.clone();
            changed.bytes[0] ^= 1;
            assert!(!verified_gbk_scene(host.resources.root(), &changed));
            let mut moved = winner.clone();
            match &mut moved.source.location {
                ResourceLocation::File { path } => *path = host.resources.root().join("other/com_001A.cst"),
                ResourceLocation::Archive { path, .. } => *path = host.resources.root().join("update02.int"),
            }
            assert!(!verified_gbk_scene(host.resources.root(), &moved));
            if matches!(&winner.source.location, ResourceLocation::Archive { .. }) {
                moved.source.location = winner.source.location.clone();
                moved.source.name = "other.cst".into();
                assert!(!verified_gbk_scene(host.resources.root(), &moved));
            }

            assert_eq!(host.call(325, &args(&[handle, scene_name]), &mut vm)?, NativeReturn::Value(1));
            let context = &host.cst_contexts[&handle];
            assert_eq!(context.encoding, ScriptTextEncoding::Gbk);
            let document = context.document.as_ref().unwrap().clone();
            for &record in records {
                let input = document.inputs.iter().position(|range| range.contains(&record)).unwrap();
                let ordinal = record - document.inputs[input].start;
                let read_args = args(&[handle, destination, input as u32, ordinal as u32]);
                let raw = &document.records[record].text;
                let expected = encoding_rs::GBK.decode_without_bom_handling_and_without_replacement(raw).unwrap();
                assert_ne!(expected.as_ref(), ScriptTextEncoding::Cp932.decode(raw));
                host.read_cst_record(327, &read_args, &mut vm)?;
                assert_eq!(&vm.read_string(destination)?, raw);
                host.call(278, &args(&[destination]), &mut vm)?;
                host.call(276, &args(&[1000, destination]), &mut vm)?;
                assert_eq!(host.data_services.cst_text_origins.shared(1000, raw), Some(ScriptTextEncoding::Gbk));
                host.read_cst_record(1037, &read_args, &mut vm)?;
                assert_eq!(vm.read_string(destination)?, expected.as_bytes());
                host.call(276, &args(&[1000, destination]), &mut vm)?;
                assert_eq!(host.data_services.cst_text_origins.shared(1000, expected.as_bytes()), Some(ScriptTextEncoding::Utf8));
                let localized_args = args(&[handle, destination, input as u32, ordinal as u32, unbound_language]);
                assert_eq!(host.read_cst_record(1038, &localized_args, &mut vm)?,
                    NativeReturn::Value(expected.len() as u32));
                assert_eq!(vm.read_string(destination)?, expected.as_bytes());
            }
            host.call(435, &args(&[scene_name, category, 1]), &mut vm)?;
            host.call(436, &[], &mut vm)?;
            let cached = host.data_services.ordinary.cached_resource_with_source(&virtual_name).unwrap();
            assert_eq!(cached.source, winner.source);
            assert_eq!(cached.bytes, winner.bytes);
            Ok((handle, virtual_name))
        })?;

        let resources = &mut player.scan.resources;
        let mut router = resources.resource_router().unwrap().clone();
        if fallback == "scene.int" {
            let prefix: Vec<_> = "scene.int".encode_utf16().collect();
            for slot in router.slots.iter_mut().filter(|slot| slot.prefix == prefix) {
                slot.loose_enabled = false;
            }
        } else {
            router.patches.retain(|mount| resources.archives()[mount.archive].path != Path::new("update03.int"));
        }
        resources.set_resource_router(router)?;

        player.with_host(|host| {
            let candidate = host.resources.resolve(&virtual_name)?.unwrap();
            let ResourceLocation::Archive { path, .. } = &candidate.location else {
                anyhow::bail!("expected archive fallback");
            };
            assert_eq!(path.strip_prefix(host.resources.root())?, Path::new(fallback));
            assert_eq!(host.call(325, &args(&[handle, scene_name]), &mut vm)?, NativeReturn::Value(1));
            assert_eq!(host.cst_contexts[&handle].encoding, ScriptTextEncoding::Gbk);
            host.call(434, &[], &mut vm)?;
            assert_eq!(host.call(325, &args(&[handle, scene_name]), &mut vm)?, NativeReturn::Value(1));
            let context = &host.cst_contexts[&handle];
            assert_eq!(context.encoding, ScriptTextEncoding::Cp932);
            let document = context.document.as_ref().unwrap();
            let record = &document.records[records[0]];
            assert!(record.text.iter().any(|byte| !byte.is_ascii()));
            let expected = encoding_rs::SHIFT_JIS.decode_without_bom_handling_and_without_replacement(&record.text).unwrap().into_owned();
            let input = document.inputs.iter().position(|range| range.contains(&records[0])).unwrap();
            let ordinal = records[0] - document.inputs[input].start;
            let localized_args = args(&[handle, destination, input as u32, ordinal as u32, unbound_language]);
            assert_eq!(host.read_cst_record(1038, &localized_args, &mut vm)?,
                NativeReturn::Value(expected.len() as u32));
            assert_eq!(vm.read_string(destination)?, expected.as_bytes());
            Ok(())
        })
    }

    #[test]
    #[ignore = "requires the pinned installed Newton game and an isolated profile path"]
    fn sidecar9_newton_origin_cache_and_fallback() -> anyhow::Result<()> {
        let root = std::env::var_os("CS2_SIDECAR9_NEWTON").expect("installed game root required");
        let scan = cs2_runtime::GameRoot::open(root)?.scan()?;
        assert!(verified_newton_text_profile(&scan.resources));
        check_scene("CS2_SIDECAR9_NEWTON", b"com_001A.cst\0", &[15, 23], "scene.int")
    }

    #[test]
    #[ignore = "requires the installed ISLAND game and an isolated profile path"]
    fn island_chinese_choices_and_ending_use_resolved_scene_codec() -> anyhow::Result<()> {
        let root = std::env::var_os("CS2_ISLAND_TEXT_TEST_ROOT").expect("installed game root required");
        let profile = std::env::var_os("CS2_SIDECAR9_PROFILE").expect("isolated profile required");
        let mut player = crate::HeadlessPlayer::open_with_save_root(root, Path::new(&profile).join("island-text"))?;
        let mut vm = Vm::new();
        vm.bind_external(vec![0; 16384])?;
        let scene_name = 0x8000_0100;
        let destination = 0x8000_0400;
        player.with_host(|host| {
            let NativeReturn::Value(handle) = host.call(323, &[], &mut vm)? else {
                anyhow::bail!("no CST context handle");
            };
            vm.write(scene_name, b"com01.cst\0")?;
            assert_eq!(host.call(325, &args(&[handle, scene_name]), &mut vm)?, NativeReturn::Value(1));
            let raw = b"\\n\\pc\xc6\xf0\xb3\xf5\xa3\xac\xcc\xfd\xb5\xbd\xc1\xcb\xd2\xbb\xcb\xbf\xc9\xf9\xcf\xec\xa1\xa3";
            let expected = "\\n\\pc\u{8d77}\u{521d}\u{ff0c}\u{542c}\u{5230}\u{4e86}\u{4e00}\u{4e1d}\u{58f0}\u{54cd}\u{3002}";
            let read_args = args(&[handle, destination, 0, 53]);
            host.read_cst_record(327, &read_args, &mut vm)?;
            host.call(278, &args(&[destination]), &mut vm)?;
            host.call(276, &args(&[1000, destination]), &mut vm)?;
            let published = &host.string_values[&1000];
            assert_eq!(published.as_slice(), raw);
            let origin = host.data_services.cst_text_origins.shared(1000, published);
            assert_eq!(origin, Some(ScriptTextEncoding::Gbk));
            assert_eq!(super::super::fes_text::decode_sourced_text(
                b"$str1000", published, ScriptTextEncoding::Cp932, ScriptTextEncoding::Cp932, false, |id| {
                    assert_eq!(id, 1000);
                    (published.as_slice(), origin)
                },
            )?.as_deref(), Some(expected));
            host.read_cst_record(1037, &read_args, &mut vm)?;
            assert_eq!(vm.read_string(destination)?, expected.as_bytes());
            for (filename, kind) in [(b"com01_selC.cst\0".as_slice(), 0x30), (b"BAD_02.cst\0", 0x20)] {
                vm.write(scene_name, filename)?;
                assert_eq!(host.call(325, &args(&[handle, scene_name]), &mut vm)?, NativeReturn::Value(1));
                let context = &host.cst_contexts[&handle];
                assert_eq!(context.encoding, ScriptTextEncoding::Gbk);
                let document = context.document.as_ref().unwrap();
                let (input, ordinal, expected) = document.inputs.iter().enumerate().find_map(|(input, range)| {
                    document.records[range.clone()].iter().enumerate().find_map(|(ordinal, record)| {
                        if record.kind == kind && !record.text.is_ascii() {
                            let text = encoding_rs::GBK.decode_without_bom_handling_and_without_replacement(&record.text)?;
                            Some((input, ordinal, text.into_owned()))
                        } else { None }
                    })
                }).expect("expected translated record");
                host.read_cst_record(1037, &args(&[handle, destination, input as u32, ordinal as u32]), &mut vm)?;
                assert_eq!(vm.read_string(destination)?, expected.as_bytes());
            }
            Ok(())
        })
    }

    #[test]
    #[ignore = "requires the pinned installed Cyanotype game and an isolated profile path"]
    fn sidecar9_cyanotype_origin_cache_and_fallback() -> anyhow::Result<()> {
        check_scene("CS2_SIDECAR9_CYANOTYPE", b"prologue.cst\0", &[17], "update02.int")
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

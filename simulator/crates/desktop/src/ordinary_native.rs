//! Persistent ordinary-table objects. All allocations use LoggingHost's
//! existing shared counter; these handles are never host or VM pointers.
use std::collections::BTreeMap;

use cs2_formats::CssFile;
use cs2_runtime::{NativeSequence, ResourceIndex, RuntimeError, native_image_metadata};
use cs2_script::{NativeReturn, Result, ScriptError, Vm};

use super::{expand_kcs_format, native_state_error, read_u32};
use super::ordinary_pair_table::GroupPairs;

const MAX_BYTES: usize = 64 * 1024 * 1024;
const MAX_LIVE_IMAGE_BYTES: usize = 256 * 1024 * 1024;
const TSUNABAN_PE_SHA256: &str = "72c9c1e7335a92bc2f1c731275a8da34d638d1730b9d5003fb6a8e85fe5b15c8";

#[derive(Debug, Clone, Default)]
pub(super) struct OrdinaryNativeServices {
    sequences: BTreeMap<u32, NativeSequence>,
    image_data: BTreeMap<u32, Vec<u8>>,
    image_bytes: usize,
    pair_tables: BTreeMap<u32, GroupPairs>,
    preloads: super::resource_preload::ResourcePreloads,
    sequence_file_uses_image_category: bool,
    /// The documented baseline default. A configuration host may replace it;
    /// no game/version inference is made from a native number.
    pub(super) sequence_name_mode: u32,
}

pub(super) struct PreparedOrdinaryData {
    sequences: BTreeMap<u32, NativeSequence>,
    image_data: BTreeMap<u32, Vec<u8>>,
    image_bytes: usize,
    pair_tables: BTreeMap<u32, GroupPairs>,
}

impl OrdinaryNativeServices {
    pub(super) fn identify(&mut self, pe_sha256: &str) {
        self.sequence_file_uses_image_category = pe_sha256 == TSUNABAN_PE_SHA256;
    }

    pub(crate) fn graph_unsupported_object_count(&self) -> usize {
        self.preloads.graph_request_count()
    }

    pub(super) fn sequence_snapshots(&self) -> Result<BTreeMap<u32, cs2_script::data_snapshot::SequenceSnapshot>> {
        self.sequences.iter().map(|(&handle, sequence)| Ok((handle, sequence.snapshot()?))).collect()
    }

    pub(super) fn raw_image_snapshots(&self) -> BTreeMap<u32, Vec<u8>> {
        self.image_data.clone()
    }

    pub(super) fn pair_snapshots(&self) -> BTreeMap<u32, cs2_script::data_snapshot::PairTableSnapshot> {
        self.pair_tables.iter().map(|(&handle, table)| (handle, table.snapshot())).collect()
    }

    pub(super) fn prepare_data_restore(saved: &cs2_script::data_snapshot::ScriptDataObjectsSnapshot) -> Result<PreparedOrdinaryData> {
        let sequences = saved.sequences.iter().map(|(&handle, sequence)|
            Ok((handle, NativeSequence::from_snapshot(sequence)?))).collect::<Result<_>>()?;
        Ok(PreparedOrdinaryData {
            sequences, image_data: saved.raw_images.clone(),
            image_bytes: saved.raw_images.values().map(Vec::len).sum(),
            pair_tables: saved.pair_tables.iter().map(|(&handle, table)|
                (handle, GroupPairs::from_snapshot(table))).collect(),
        })
    }

    pub(super) fn commit_data_restore(&mut self, prepared: PreparedOrdinaryData) {
        self.sequences = prepared.sequences;
        self.image_data = prepared.image_data;
        self.image_bytes = prepared.image_bytes;
        self.pair_tables = prepared.pair_tables;
    }
    pub(super) fn raw_image(&self, handle: u32) -> Result<Vec<u8>> {
        self.image_data
            .get(&handle)
            .cloned()
            .ok_or_else(|| native_state_error("invalid/wrong-type raw image data handle"))
    }

    pub(super) fn supports(index: u32) -> bool {
        matches!(index, 365 | 366 | 368 | 419..=425 | 431 | 434..=438 | 458 | 459 | 683..=685)
    }

    pub(super) fn call(
        &mut self,
        index: u32,
        args: &[u8],
        vm: &mut Vm,
        resources: &ResourceIndex,
        containers: &mut BTreeMap<u32, CssFile>,
        next_handle: &mut u32,
    ) -> Result<NativeReturn> {
        match index {
            431 => {
                let handle = read_u32(args, 0)?;
                let output = read_u32(args, 4)?;
                if handle == 0 { return Ok(NativeReturn::Value(0)); }
                match vm.read(output, 1) {
                    Ok(_) => {}
                    Err(ScriptError::MemoryOutOfBounds { .. }) => return Ok(NativeReturn::Value(0)),
                    Err(error) => return Err(error),
                }
                vm.write(output, &[0; 36])?;
                let bytes = self.raw_image(handle)?;
                if !bytes.starts_with(b"HG-3") && !bytes.starts_with(b"HG-2") {
                    return Err(native_state_error("native 431 aggregate metadata requires verified HG input"));
                }
                let file = cs2_formats::HgFile::parse(&bytes)
                    .map_err(|_| native_state_error("native 431 malformed HG structure"))?;
                let metadata = file.native_aggregate_metadata()
                    .map_err(|_| native_state_error("native 431 HG aggregate metadata failed"))?;
                let packed = metadata.into_iter().flat_map(u32::to_le_bytes).collect::<Vec<_>>();
                vm.write(output, &packed)?;
                Ok(NativeReturn::Value(1))
            }
            434..=438 => self.preloads.call(index, args, vm, resources),
            683 => {
                let name = resource_name(vm, read_u32(args, 0)?)?;
                // No category resolution or FES decompression at this entry.
                // A successfully opened empty file creates a valid empty table.
                let bytes = if let Some(bytes) = self.cached_resource(&name) {
                    if bytes.len() > MAX_BYTES { return Err(native_state_error("native 683 cached resource exceeds 64 MiB")); }
                    bytes.to_vec()
                } else {
                    let Some(candidate) = resources.resolve_mode_utf16(
                        &name.encode_utf16().collect::<Vec<_>>(), cs2_runtime::resource_router::ResourceMode::LooseFirst,
                    ).map_err(|_| native_state_error("native 683 resource routing failed"))? else {
                        return Ok(NativeReturn::Value(0));
                    };
                    let candidate = candidate.value;
                    if candidate.size > MAX_BYTES as u64 {
                        return Err(native_state_error("native 683 resource exceeds 64 MiB"));
                    }
                    match resources.read_candidate(candidate) {
                    Ok(bytes) if bytes.len() <= MAX_BYTES => bytes,
                    Ok(_) => return Err(native_state_error("native 683 resource exceeds 64 MiB")),
                    Err(RuntimeError::ResourceNotFound(_)) => return Ok(NativeReturn::Value(0)),
                    Err(RuntimeError::Io { source, .. }) if matches!(source.kind(),
                        std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied
                    ) => return Ok(NativeReturn::Value(0)),
                    Err(_) => return Err(native_state_error("native 683 resource read failed")),
                    }
                };
                let table = GroupPairs::parse(&bytes)?;
                let handle = self.allocate(next_handle)?;
                self.pair_tables.insert(handle, table);
                Ok(NativeReturn::Value(handle))
            }
            684 => {
                let handle = read_u32(args, 0)?;
                if handle != 0 && self.pair_tables.remove(&handle).is_none() {
                    return Err(native_state_error("native 684 invalid/wrong-type pair-table handle"));
                }
                Ok(NativeReturn::Continue)
            }
            685 => {
                let handle = read_u32(args, 0)?;
                if handle == 0 {
                    return Ok(NativeReturn::Value(0));
                }
                let group = formatted(vm, read_u32(args, 4)?)?;
                if group.len() >= 256 {
                    return Err(native_state_error("native 685 group query exceeds 255 bytes"));
                }
                let key = formatted(vm, read_u32(args, 8)?)?;
                if key.len() >= 256 {
                    return Err(native_state_error("native 685 key query exceeds 255 bytes"));
                }
                let output = read_u32(args, 16)?;
                match vm.read(output, 1) {
                    Ok(_) => {}
                    Err(ScriptError::MemoryOutOfBounds { .. }) => return Ok(NativeReturn::Value(0)),
                    Err(error) => return Err(error),
                }
                let occurrence = read_u32(args, 12)?;
                let table = self.pair_tables.get(&handle).ok_or_else(|| native_state_error(
                    "native 685 invalid/wrong-type pair-table handle",
                ))?;
                let Some(value) = table.query(&group, &key, occurrence) else {
                    // A miss does not clear or otherwise touch the output.
                    return Ok(NativeReturn::Value(0));
                };
                let mut terminated = value.to_vec();
                terminated.push(0);
                vm.write(output, &terminated)?;
                Ok(NativeReturn::Value(1))
            }
            365 => {
                let name = resource_name(vm, read_u32(args, 0)?)?;
                let Some(bytes) = self.read_resource(resources, b"image", &name)? else {
                    return Ok(NativeReturn::Value(0));
                };
                let total = self
                    .image_bytes
                    .checked_add(bytes.len())
                    .filter(|&total| total <= MAX_LIVE_IMAGE_BYTES)
                    .ok_or_else(|| {
                        native_state_error("native 365 live image data exceeds 256 MiB")
                    })?;
                let handle = self.allocate(next_handle)?;
                self.image_data.insert(handle, bytes);
                self.image_bytes = total;
                Ok(NativeReturn::Value(handle))
            }
            366 => {
                let handle = read_u32(args, 0)?;
                if handle != 0 {
                    let bytes = self.image_data.remove(&handle).ok_or_else(|| {
                        native_state_error("native 366 invalid/wrong-type image data handle")
                    })?;
                    self.image_bytes -= bytes.len();
                }
                Ok(NativeReturn::Continue)
            }
            368 => {
                let handle = read_u32(args, 0)?;
                if handle == 0 {
                    return Ok(NativeReturn::Value(0));
                }
                let id = read_u32(args, 4)?;
                let target = read_u32(args, 8)?;
                if vm.read(target, 1).is_err() {
                    return Ok(NativeReturn::Value(0));
                }
                let bytes = self.image_data.get(&handle).ok_or_else(|| {
                    native_state_error("native 368 invalid/wrong-type image data handle")
                })?;
                vm.write(target, &[0; 36])?;
                let Some(fields) = native_image_metadata(bytes, id)? else {
                    return Ok(NativeReturn::Value(0));
                };
                let bytes: Vec<_> = fields.into_iter().flat_map(u32::to_le_bytes).collect();
                vm.write(target, &bytes)?;
                Ok(NativeReturn::Value(1))
            }
            419 => {
                let handle = self.allocate(next_handle)?;
                self.sequences.insert(handle, NativeSequence::default());
                Ok(NativeReturn::Value(handle))
            }
            420 => {
                let handle = read_u32(args, 0)?;
                if handle != 0 && self.sequences.remove(&handle).is_none() {
                    return Err(native_state_error(
                        "native 420 invalid/wrong-type sequence handle",
                    ));
                }
                Ok(NativeReturn::Continue)
            }
            421 => {
                let handle = read_u32(args, 0)?;
                if handle != 0 {
                    self.sequence(handle)?.clear();
                }
                Ok(NativeReturn::Continue)
            }
            422 | 423 => {
                let handle = read_u32(args, 0)?;
                if handle == 0 {
                    return Ok(if index == 423 {
                        NativeReturn::Value(0)
                    } else {
                        NativeReturn::Continue
                    });
                }
                // Validate the object before evaluating a text argument.
                self.sequence(handle)?;
                let text = formatted(vm, read_u32(args, 4)?)?;
                if index == 422 {
                    let mode = self.sequence_name_mode;
                    self.sequence(handle)?.register(&text, mode)?;
                    Ok(NativeReturn::Continue)
                } else {
                    Ok(NativeReturn::Value(u32::from(
                        self.sequence(handle)?.contains(&text)?,
                    )))
                }
            }
            424 | 425 => {
                let container = read_u32(args, 0)?;
                if container == 0 {
                    return Ok(NativeReturn::Value(0));
                }
                let record_id = read_u32(args, 4)?;
                if index == 424 {
                    let handle = read_u32(args, 8)?;
                    if handle == 0 {
                        return Ok(NativeReturn::Value(0));
                    }
                    let bytes = self.sequence(handle)?.save_tree()?;
                    containers
                        .get_mut(&container)
                        .ok_or_else(|| {
                            native_state_error("native 424 invalid/wrong-type CSS container")
                        })?
                        .append(record_id, &bytes)
                        .map_err(|_| native_state_error("native 424 CSS append failed"))?;
                } else {
                    let container = containers.get(&container).ok_or_else(|| {
                        native_state_error("native 425 invalid/wrong-type CSS container")
                    })?;
                    let Some(bytes) = container.try_record(record_id)
                        .map_err(|_| native_state_error("native 425 malformed CSS record storage"))? else {
                        return Ok(NativeReturn::Value(0));
                    };
                    let handle = read_u32(args, 8)?;
                    if handle == 0 {
                        return Ok(NativeReturn::Value(0));
                    }
                    self.sequence(handle)?.restore_tree(bytes)?;
                }
                Ok(NativeReturn::Value(1))
            }
            458 => {
                let handle = read_u32(args, 0)?;
                if handle == 0 {
                    return Ok(NativeReturn::Continue);
                }
                self.sequence(handle)?;
                let name = resource_name(vm, read_u32(args, 4)?)?;
                let category: &[u8] = if self.sequence_file_uses_image_category {
                    b"image"
                } else {
                    b"config"
                };
                if let Some(bytes) = self.read_resource(resources, category, &name)? {
                    self.sequence(handle)?.load_file(bytes)?;
                }
                Ok(NativeReturn::Continue)
            }
            459 => {
                let handle = read_u32(args, 0)?;
                if handle == 0 {
                    return Ok(NativeReturn::Continue);
                }
                self.sequence(handle)?;
                let group = read_u32(args, 4)?;
                let name = formatted(vm, read_u32(args, 8)?)?;
                self.sequence(handle)?.import_group(group, &name)?;
                Ok(NativeReturn::Continue)
            }
            _ => Err(ScriptError::UnimplementedNativeCall { index }),
        }
    }

    fn sequence(&mut self, handle: u32) -> Result<&mut NativeSequence> {
        self.sequences
            .get_mut(&handle)
            .ok_or_else(|| native_state_error("ordinary native invalid/wrong-type sequence handle"))
    }

    fn read_resource(&self, resources: &ResourceIndex, category: &[u8], name: &str) -> Result<Option<Vec<u8>>> {
        let names = resources.category_names(category, name)
            .map_err(|_| native_state_error("ordinary native resource category resolution failed"))?;
        for name in &names {
            if let Some(bytes) = self.cached_resource(name) {
                return Ok((!bytes.is_empty()).then(|| bytes.to_vec()));
            }
        }
        read_resource(resources, category, name)
    }

    /// Common resource readers may consume the completed explicit-preload
    /// buffer. A cache miss keeps their normal ResourceIndex fallback.
    pub(super) fn cached_resource(&self, name: &str) -> Option<&[u8]> {
        self.preloads.cached(name)
    }

    pub(super) fn cached_resource_with_source(&self, name: &str) -> Option<&super::resource_preload::SourcedResource> {
        self.preloads.cached_with_source(name)
    }

    pub(super) fn take_preloaded_model(
        &mut self, version: &cs2_runtime::emote::EmoteArchiveVersion,
    ) -> Option<std::sync::Arc<cs2_runtime::emote::CpuMotionModel>> {
        self.preloads.take_model(version)
    }

    #[cfg(all(feature = "gpu-raster", any(windows, target_os = "android")))]
    pub(super) fn stage_preloaded_model(&mut self) {
        if let Some(model) = self.preloads.ready_model() {
            super::gpu_raster::stage_emote_model(model);
        }
    }

    pub(super) fn drain_prefetch_notes(&mut self) -> Vec<String> { self.preloads.drain_notes() }

    pub(super) fn update_prefetch(&mut self, resources: &ResourceIndex,
        requests: &[super::resource_preload::AheadRequest], boundaries: &[&'static str]) {
        self.preloads.update_prefetch(resources, requests, boundaries);
    }

    pub(super) fn predecode_image(&mut self, bytes: &[u8]) { self.preloads.predecode_image(bytes); }

    pub(super) fn take_predecoded_image(&mut self, bytes: &[u8], id: u32) -> Option<cs2_renderer::RgbaFrame> {
        self.preloads.take_predecoded_image(bytes, id)
    }

    fn allocate(&self, next_handle: &mut u32) -> Result<u32> {
        if self.sequences.len().saturating_add(self.image_data.len())
            .saturating_add(self.pair_tables.len()) >= 1_000_000 {
            return Err(native_state_error("ordinary native object limit exceeded"));
        }
        let handle = (*next_handle).max(1);
        let next = handle
            .checked_add(1)
            .ok_or_else(|| native_state_error("ordinary native handle space exhausted"))?;
        if self.sequences.contains_key(&handle) || self.image_data.contains_key(&handle)
            || self.pair_tables.contains_key(&handle) {
            return Err(native_state_error(
                "ordinary native counter overlaps a live object",
            ));
        }
        *next_handle = next;
        Ok(handle)
    }
}

fn formatted(vm: &Vm, address: u32) -> Result<Vec<u8>> {
    let text = expand_kcs_format(vm, &vm.read_string(address)?, 0)?;
    Ok(text[..text
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(text.len())]
        .to_vec())
}

impl super::LoggingHost<'_> {
    /// Source-first alias, native status two. Copy only the value and clear
    /// supplemental state instead of cloning attachments or ordered history.
    pub(super) fn cst_alias_native(&mut self, args: &[u8], vm: &Vm) -> Result<NativeReturn> {
        let handle = read_u32(args, 0)?;
        if handle == 0 {
            return Ok(NativeReturn::Continue);
        }
        if !self.cst_contexts.contains_key(&handle) {
            return Err(native_state_error(
                "native 934 invalid/wrong-type CST context",
            ));
        }
        let mut source = formatted(vm, read_u32(args, 4)?)?;
        let mut destination = formatted(vm, read_u32(args, 8)?)?;
        if source.len() >= 128 || destination.len() >= 128 {
            return Err(native_state_error(
                "native 934 formatted name exceeds 127 bytes",
            ));
        }
        super::normalize_cst_name(&mut source);
        super::normalize_cst_name(&mut destination);
        let context = self.cst_contexts.get_mut(&handle).unwrap();
        let Some(value) = context
            .registrations
            .iter()
            .find(|entry| super::cst_name_matches(&entry.name, &source))
            .map(|entry| entry.value)
        else {
            return Ok(NativeReturn::Continue);
        };
        if context
            .registrations
            .iter()
            .any(|entry| super::cst_name_matches(&entry.name, &destination))
        {
            return Ok(NativeReturn::Continue);
        }
        if destination.len() >= 32 {
            return Err(native_state_error("native 934 alias name exceeds 31 bytes"));
        }
        let mut name = [0; 32];
        name[..destination.len()].copy_from_slice(&destination);
        context.registrations.push(super::CstRegistration {
            name,
            value,
            attachments: Vec::new(),
            data: Vec::new(),
        });
        Ok(NativeReturn::Continue)
    }
}

fn resource_name(vm: &Vm, address: u32) -> Result<String> {
    // Resource names retain CP932 independently of the selected text encoding.
    Ok(
        String::from_utf8(super::cp932_to_utf8(&formatted(vm, address)?))
            .expect("CP932 conversion produces UTF-8"),
    )
}

fn read_resource(
    resources: &ResourceIndex,
    category: &[u8],
    name: &str,
) -> Result<Option<Vec<u8>>> {
    let names = resources
        .category_names(category, name)
        .map_err(|_| native_state_error("ordinary native resource category resolution failed"))?;
    for name in names {
        let Some(candidate) = resources.resolve_mode_utf16(
            &name.encode_utf16().collect::<Vec<_>>(), cs2_runtime::resource_router::ResourceMode::LooseFirst,
        ).map_err(|_| native_state_error("ordinary native resource routing failed"))? else {
            continue;
        };
        let candidate = candidate.value;
        if candidate.size > MAX_BYTES as u64 {
            return Err(native_state_error(
                "ordinary native resource exceeds 64 MiB",
            ));
        }
        return match resources.read_candidate(candidate) {
            Ok(bytes) if bytes.is_empty() => Ok(None),
            Ok(bytes) if bytes.len() <= MAX_BYTES => Ok(Some(bytes)),
            Ok(_) => Err(native_state_error(
                "ordinary native resource exceeds 64 MiB",
            )),
            Err(RuntimeError::ResourceNotFound(_)) => Ok(None),
            Err(RuntimeError::Io { source, .. }) if matches!(source.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied) => Ok(None),
            Err(_) => Err(native_state_error("ordinary native resource read failed")),
        };
    }
    Ok(None)
}

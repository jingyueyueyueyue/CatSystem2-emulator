//! Direct resource wrappers, independently recovered from fixed PE helpers.
use cs2_runtime::native_table::{ImageProducer703, ImageRecordLayers, ImageRecordLayout};
use cs2_runtime::resource_router::ResourceMode;
use cs2_script::{NativeReturn, Result, Vm};
use super::{HeadlessPlayer, LoggingHost, expand_kcs_format, native_state_error, read_u32, trim_nul};

#[path = "image_record_fill.rs"]
mod fill;

/// A host must explicitly opt in to repaired layer grouping. The default
/// follows a fixed PE contract, including its defects and diagnostics.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ImageRecordPolicy {
    #[default]
    PeCompatibility,
    PortableStaticRuns,
}

impl HeadlessPlayer {
    pub fn set_image_record_policy(&mut self, policy: ImageRecordPolicy) {
        self.data_services.image_record_policy = policy;
    }
    pub fn image_record_policy(&self) -> ImageRecordPolicy {
        self.data_services.image_record_policy
    }
}

struct ImageRecordPlan {
    name: String,
    animated: bool,
    // A single-file call preserves its old name tail, like 5ca0e1..5ca192.
    name_tail: Option<Vec<u8>>,
}

impl LoggingHost<'_> {
    pub(super) fn image_records_native(&mut self, index: u32, args: &[u8], vm: &mut Vm) -> Result<NativeReturn> {
        let table = self.data_services.ordinary_native_table.as_ref();
        if index == 704 {
            if args.len() != 8 { return Err(native_state_error("native 704 requires exactly 8 argument bytes")); }
            let base = read_u32(args, 0)?;
            match vm.read(base, 1) {
                Err(cs2_script::ScriptError::MemoryOutOfBounds { .. }) => return Ok(NativeReturn::Continue),
                other => { other?; }
            }
            let count = read_u32(args, 4)? as usize;
            if count == 0 { return Ok(NativeReturn::Continue); }
            let stride = table.and_then(|table| table.image_release_704())
                .ok_or_else(|| native_state_error("native 704 image record release wrapper was not recognized"))?
                .stride();
            if count > (64 * 1024 * 1024) / stride { return Err(native_state_error("native 704 image record count exceeds capacity")); }
            for i in 0..count {
                let address = record_address(base, i, stride)?;
                let record = vm.read(address, 8)?;
                let tag = read_u32(&record, 0)?;
                let handle = read_u32(&record, 4)?;
                drop(record);
                match tag {
                    1 if handle != 0 => { self.release_image_wrapper(handle)?; vm.write(address + 4, &[0; 4])?; }
                    2 if handle != 0 => {
                        self.release_list_entry_source(handle)
                            .map_err(|_| native_state_error("native 704 invalid/wrong-type retained image object"))?;
                        vm.write(address + 4, &[0; 4])?;
                    }
                    _ => {}
                }
                vm.write(address, &[0; 4])?;
            }
            return Ok(NativeReturn::Continue);
        }
        let (layout, layers) = match table.and_then(|table| table.image_producer_703()) {
            Some(ImageProducer703::SelectionPointer) => return self.move_selection_pointer_native703(args),
            Some(ImageProducer703::Records { layout, layers }) => (layout, layers),
            None => return Err(native_state_error("native 703 image record or selection pointer wrapper was not recognized")),
        };
        if !matches!(args.len(), 12 | 16) { return Err(native_state_error("native 703 requires three DWORD arguments; an unused trailing DWORD is accepted")); }
        let expanded = expand_kcs_format(vm, &vm.read_string(read_u32(args, 0)?)?, 0)?;
        let raw = trim_nul(&expanded);
        if raw.len() >= 260 { return Err(native_state_error("native 703 image name exceeds its original buffer")); }
        let base = read_u32(args, 4)?;
        match vm.read(base, 1) {
            Err(cs2_script::ScriptError::MemoryOutOfBounds { .. }) => return Ok(NativeReturn::Value(0)),
            other => { other?; }
        }
        let name = super::ScriptTextEncoding::Cp932.decode(raw);
        let names = self.resources.category_names(b"image", &name)
            .map_err(|_| native_state_error("native 703 image category resolution failed"))?;
        let name = names.first().ok_or_else(|| native_state_error("native 703 image category resolved no name"))?;
        let count = read_u32(args, 8)? as i32;
        if count <= 0 { return Ok(NativeReturn::Value(0)); }
        let count = count as usize;
        let stride = layout.stride();
        if count > (64 * 1024 * 1024) / stride { return Err(native_state_error("native 703 image record count exceeds capacity")); }
        record_address(base, count - 1, stride)?;
        // Validate the complete records, including narrow/wide name tails,
        // before creating an object. Regions and the high external domain
        // must not be crossed by a record or by the complete array.
        vm.read(base, count * stride)?;
        // Clear only type and object. The narrow/wide name tail is retained.
        for i in 0..count { vm.write(record_address(base, i, stride)?, &[0; 8])?; }
        let policy = self.data_services.image_record_policy;
        let mut created = Vec::<(u32, u32)>::new();
        let result = (|| {
            self.visit_image_records(name, layout, layers, policy, count, |host, plan| {
                let tag = if plan.animated { 2u32 } else { 1u32 };
                let handle = if plan.animated {
                    // The ANM test chooses the object kind, but attach reads
                    // the HG body. Timeline loading/playback remains 624/625
                    // -> list 299/301 on the SAME shared entry object.
                    let Some((path, bytes)) = host.read_managed_image_asset(&plan.name, false)? else { return Ok(()); };
                    check_record_capacity(created.len(), count)?;
                    host.create_unowned_list_entry_source(&bytes, Some(&path))
                        .map_err(|_| native_state_error("native 703 multi-frame image object construction failed"))?
                } else {
                    // Both single images and runs of static selectors use
                    // the existing cache, metadata pass and integer-alpha
                    // layer compositor. The 703 wrapper passes cache=0.
                    let Some(frame) = host.load_managed_image_plan(&plan.name, false)? else { return Ok(()); };
                    check_record_capacity(created.len(), count)?;
                    host.register_image_wrapper(frame, false)?
                };
                let address = record_address(base, created.len(), stride)?;
                created.push((tag, handle));
                if let Some(tail) = plan.name_tail { vm.write(address + 8, &tail)?; }
                let output = [tag, handle].into_iter().flat_map(u32::to_le_bytes).collect::<Vec<_>>();
                vm.write(address, &output)?;
                Ok(())
            })?;
            Ok(NativeReturn::Value(created.len() as u32))
        })();
        if result.is_err() && policy == ImageRecordPolicy::PortableStaticRuns {
            // Never strand unowned wrappers or producer references after a
            // failed later decode. Names remain diagnostic data, as in 704.
            for (i, (tag, handle)) in created.into_iter().enumerate() {
                if tag == 2 {
                    self.release_list_entry_source(handle)
                        .map_err(|_| native_state_error("native 703 retained object cleanup failed"))?;
                } else { self.release_image_wrapper(handle)?; }
                vm.write(record_address(base, i, stride)?, &[0; 8])?;
            }
        }
        result
    }

    fn visit_image_records(&mut self, name: &str, layout: ImageRecordLayout,
        layers: Option<ImageRecordLayers>, policy: ImageRecordPolicy, capacity: usize,
        mut visit: impl FnMut(&mut Self, ImageRecordPlan) -> Result<()>) -> Result<()> {
        let dot = name.rfind('.').filter(|&dot| name[..dot].rfind(['/', '\\']).is_none_or(|slash| dot > slash));
        let (stem, extension) = dot.map_or((name, ""), |dot| (&name[..dot], &name[dot + 1..]));
        let (encoded, _, invalid) = encoding_rs::SHIFT_JIS.encode(stem);
        if invalid || encoded.len() >= 256 {
            return Err(native_state_error("native 703 selector table exceeds its CP932 buffer or is unencodable"));
        }
        let parts = stem.split(',').filter(|part| !part.is_empty()).collect::<Vec<_>>();
        let Some(prefix) = parts.first() else { return Ok(()); };
        if parts.len() == 1 {
            let animated = self.image_is_animation_resource(name)?;
            return visit(self, ImageRecordPlan { name: name.to_owned(), animated, name_tail: None });
        }
        if parts.len() > 65 {
            return Err(native_state_error("native 703 image plan exceeds 64 layer selectors"));
        }
        if policy == ImageRecordPolicy::PeCompatibility {
            return match layers {
                Some(ImageRecordLayers::NarrowStaticRuns) => {
                    self.visit_narrow_compatible_records(prefix, extension, &parts[1..], layout, &mut visit)
                }
                Some(ImageRecordLayers::NarrowPrefixReadAsUtf16) => {
                    self.events.push(super::LogicEvent { frame: self.frame, kind: "image-record-compatibility".into(),
                        detail: format!("policy=pe-compatibility fault=narrow-buffer-passed-to-wsprintfW name={name}") });
                    Err(native_state_error("native 703 verified wide layer consumer reads a narrow selector prefix without an initialized UTF16 terminator; deterministic compatibility is undefined"))
                }
                None => Err(native_state_error("native 703 layered behavior was not recognized in the registered producer")),
            };
        }
        // Portable, explicit repair of the original split-record branch:
        // preserve the evidenced physical names and real source kinds, but
        // use typed strings, flush static runs before ANM and at end, and
        // append the extension exactly once. Fixed PE code instead contains
        // narrow-as-wide calls, a duplicate extension and lost static runs.
        // See image_records_B_20261117.md; this is not PE-bitwise equivalence.
        let mut plans = Vec::new();
        let mut selectors = vec!['0'; parts.len() - 1];
        let mut static_end = None;
        for (i, part) in parts[1..].iter().enumerate() {
            let (encoded, _, invalid) = encoding_rs::SHIFT_JIS.encode(part);
            let first = encoded[0];
            if invalid || !first.is_ascii() {
                return Err(native_state_error("native 703 non-ASCII first-byte layer selector is unsupported"));
            }
            if first == b'0' { continue; }
            let physical = format!("{prefix}_{}{selector}.{extension}", "0".repeat(i), selector = char::from(first));
            check_record_resource_name(&physical)?;
            if self.image_is_animation_resource(&physical)? {
                append_static_record(&mut plans, prefix, extension, &selectors, static_end.take(), layout)?;
                selectors.fill('0');
                let tail = image_record_name_tail(&physical, layout)?;
                plans.push(ImageRecordPlan { name: physical, animated: true, name_tail: Some(tail) });
            } else {
                selectors[i] = char::from(first);
                static_end = Some(i + 1);
            }
        }
        append_static_record(&mut plans, prefix, extension, &selectors, static_end, layout)?;
        if plans.len() > capacity {
            return Err(native_state_error("native 703 output record capacity is smaller than its layer plan"));
        }
        self.events.push(super::LogicEvent {
            frame: self.frame,
            kind: "image-record-layer-plan".into(),
            detail: format!("policy=portable-static-runs count={} name={name}", plans.len()),
        });
        for plan in plans { visit(self, plan)?; }
        Ok(())
    }

    fn visit_narrow_compatible_records(&mut self, prefix: &str, extension: &str,
        parts: &[&str], layout: ImageRecordLayout,
        visit: &mut impl FnMut(&mut Self, ImageRecordPlan) -> Result<()>) -> Result<()> {
        let mut candidates = 0;
        let mut pending = prefix.to_owned();
        let mut previous_animation = false;
        for (index, part) in parts.iter().enumerate() {
            let (encoded, _, invalid) = encoding_rs::SHIFT_JIS.encode(part);
            let first = encoded[0];
            if invalid || !first.is_ascii() {
                return Err(native_state_error("native 703 narrow PE non-ASCII signed selector requires its original byte-string representation"));
            }
            if first == b'0' { continue; }
            let physical = format!("{prefix}_{}{selector}.{extension}", "0".repeat(index), selector = char::from(first));
            check_record_resource_name(&physical)?;
            if self.image_is_animation_resource(&physical)? {
                // 54c3da..54c41c appends '.' and the extension AGAIN before
                // loading HG. A missing doubled name produces no record, but
                // still resets pending and leaves the animation flag set.
                let actual = format!("{physical}.{extension}");
                check_record_resource_name(&actual)?;
                let tail = image_record_name_tail(&actual, layout)?;
                visit(self, ImageRecordPlan { name: actual, animated: true, name_tail: Some(tail) })?;
                candidates += 1;
                pending = format!("{prefix}{}", ",0".repeat(index));
                previous_animation = true;
            } else if previous_animation {
                // Flush BEFORE including this static selector. The name tail
                // copies physical (13c), while loading uses pending (23c).
                // They intentionally differ. No current-selector append here.
                let actual = format!("{pending}.{extension}");
                check_record_resource_name(&actual)?;
                let tail = image_record_name_tail(&physical, layout)?;
                visit(self, ImageRecordPlan { name: actual, animated: false, name_tail: Some(tail) })?;
                candidates += 1;
                pending = format!("{prefix}{}", ",0".repeat(index));
                previous_animation = false;
            } else {
                pending.push(','); pending.push(char::from(first));
                check_record_resource_name(&pending)?;
            }
        }
        // 54c6b5 returns immediately: no final static flush.
        self.events.push(super::LogicEvent { frame: self.frame, kind: "image-record-layer-plan".into(),
            detail: format!("policy=pe-compatibility candidates={candidates} no-final-flush=true") });
        Ok(())
    }

    pub(super) fn sound_exists_native284(&mut self, args: &[u8], vm: &Vm) -> Result<NativeReturn> {
        if args.len() != 8 { return Err(native_state_error("native 284 requires exactly 8 argument bytes")); }
        // arg0 is reserved. No image/sound object is created or decoded.
        let name = expand_kcs_format(vm, &vm.read_string(read_u32(args, 4)?)?, 0)?;
        let name = trim_nul(&name);
        if name.len() >= 260 { return Err(native_state_error("native 284 sound filename exceeds its original buffer")); }
        let decoded = super::sound_native::decode_name(self.text_encoding, name)?;
        let name = if decoded.rfind(['/', '\\']).is_some_and(|at| at > 0) {
            decoded
        } else {
            let slot = self.resource_category_slot_name(b"sound", name)?;
            if slot == u32::MAX { return Ok(NativeReturn::Value(0)); }
            self.resources.category_slot_name(slot, &decoded)
                .map_err(|_| native_state_error("native 284 sound category resolution failed"))?
        };
        let found = self.resources.resolve_mode_utf16(&name.encode_utf16().collect::<Vec<_>>(), ResourceMode::LooseFirst)
            .map_err(|_| native_state_error("native 284 sound routing failed"))?;
        if let Some(found) = &found {
            self.resources.open_media_candidate(found.value, 4 * 1024 * 1024 * 1024)
                .map_err(|_| native_state_error("native 284 sound source open failed"))?;
        }
        let found = found.is_some();
        Ok(NativeReturn::Value(u32::from(found)))
    }

    pub(super) fn motion_contains_native514(&mut self, args: &[u8], vm: &Vm) -> Result<NativeReturn> {
        let handle = read_u32(args, 0)?;
        if handle == 0 { return Ok(NativeReturn::Value(0)); }
        if args.len() != 8 { return Err(native_state_error("native 514 requires exactly 8 argument bytes")); }
        if !self.motion_controllers.contains_key(&handle) {
            return Err(native_state_error("native 514 invalid/wrong-type motion controller"));
        }
        let name = expand_kcs_format(vm, &vm.read_string(read_u32(args, 4)?)?, 0)?;
        let name = trim_nul(&name);
        if name.len() >= 260 { return Err(native_state_error("native 514 motion filename exceeds its original buffer")); }
        let name = super::ScriptTextEncoding::Cp932.decode(name);
        let names = self.resources.category_names(b"mot", &name)
            .map_err(|_| native_state_error("native 514 motion category resolution failed"))?;
        let name = names.first().ok_or_else(|| native_state_error("native 514 motion category resolved no name"))?;
        let controller = self.motion_controllers.get_mut(&handle).unwrap();
        if controller.pending.is_some() {
            return Err(native_state_error("native 514 cannot change motion properties during a pending update"));
        }
        // 6e93dc..6e93ea copies +4..+93 to +94..+123 before searching,
        // even on a miss. This query has a real controller-state side effect.
        controller.properties = controller.base_properties;
        let found = controller.tasks.values().any(|task| {
            let task = super::ScriptTextEncoding::Cp932.decode(trim_nul(&task.name));
            task.eq_ignore_ascii_case(name)
        });
        Ok(NativeReturn::Value(u32::from(found)))
    }
}

fn record_address(base: u32, index: usize, stride: usize) -> Result<u32> {
    let address = index.checked_mul(stride).and_then(|offset| u32::try_from(offset).ok())
        .and_then(|offset| base.checked_add(offset))
        .filter(|&address| address.checked_add(stride as u32 - 1).is_some_and(|end|
            end & 0x8000_0000 == base & 0x8000_0000) && address & 0x8000_0000 == base & 0x8000_0000)
        .ok_or_else(|| native_state_error("image record address overflows its VM address domain"))?;
    Ok(address)
}

fn check_record_capacity(written: usize, count: usize) -> Result<()> {
    if written >= count {
        return Err(native_state_error("native 703 PE would write beyond its declared record array; prior records retained"));
    }
    Ok(())
}

fn append_static_record(
    output: &mut Vec<ImageRecordPlan>, prefix: &str, extension: &str,
    selectors: &[char], end: Option<usize>, layout: ImageRecordLayout,
) -> Result<()> {
    let Some(end) = end else { return Ok(()); };
    let selectors = selectors[..end].iter().map(char::to_string).collect::<Vec<_>>().join(",");
    let name = format!("{prefix},{selectors}.{extension}");
    check_record_resource_name(&name)?;
    let tail = image_record_name_tail(&name, layout)?;
    output.push(ImageRecordPlan { name, animated: false, name_tail: Some(tail) });
    Ok(())
}

fn check_record_resource_name(name: &str) -> Result<()> {
    let (bytes, _, invalid) = encoding_rs::SHIFT_JIS.encode(name);
    if invalid || bytes.len() >= 260 || name.encode_utf16().count() >= 260 {
        return Err(native_state_error("native 703 generated resource name exceeds its original buffer"));
    }
    Ok(())
}

fn image_record_name_tail(name: &str, layout: ImageRecordLayout) -> Result<Vec<u8>> {
    let mut bytes = if layout == ImageRecordLayout::Cp932 {
        let (bytes, _, invalid) = encoding_rs::SHIFT_JIS.encode(name);
        if invalid { return Err(native_state_error("native 703 record name is not CP932 encodable")); }
        let mut bytes = bytes.into_owned();
        bytes.push(0);
        bytes
    } else {
        name.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect()
    };
    let tail_size = layout.stride() - 8;
    if bytes.len() > tail_size {
        return Err(native_state_error("native 703 name exceeds its fixed 64-character record tail"));
    }
    bytes.resize(tail_size, 0);
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cs2_runtime::native_table::OrdinaryNativeTable;
    use cs2_script::{Host, KcsProgram};
    use std::path::Path;

    fn game(path: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../games").join(path)
    }

    #[test]
    fn native_image_records_uses_registered_layout_for_adjacent_records() -> anyhow::Result<()> {
        for (path, stride) in [
            ("cs2_full_v401/system/cs2_open.exe", 0x48usize),
            ("Yukikoi Melt/YukikoiMelt.exe", 0x88),
        ] {
            let sample = game(path);
            if !sample.is_file() {
                continue;
            }
            let temp = crate::tests::TempDir::new();
            std::fs::write(temp.0.join("startup.xml"), concat!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?><document><INT><int00>",
                "<name>image</name><int>image.int</int><ext>hg3</ext><dir>image</dir>",
                "</int00></INT></document>",
            ))?;
            let mut player = HeadlessPlayer::open_with_save_root(&temp.0, temp.0.join("profile"))?;
            let table = OrdinaryNativeTable::from_pe(&std::fs::read(sample)?).expect(path);
            player.with_host(|host| {
                assert!(host.data_services.native_profile.is_none());
                host.data_services.ordinary_native_table = Some(table);
                let mut vm = Vm::from_program(KcsProgram::from_code(&[0x3b], vec![0; 64], 0)?)?;
                let marker = vec![0x7d; 2 * stride + 16];
                let base = vm.allocate_region(&marker)?;
                let missing = vm.allocate_region(b"image/missing.hg3\0")?;
                let args = [missing, base, 2].map(u32::to_le_bytes).concat();
                assert_eq!(host.call(703, &args, &mut vm)?, NativeReturn::Value(0));
                let mut cleared = marker;
                cleared[..8].fill(0);
                cleared[stride..stride + 8].fill(0);
                assert_eq!(vm.read(base, cleared.len())?, cleared);

                let mut handles = Vec::new();
                for record in [base, base + stride as u32] {
                    let frame = cs2_renderer::RgbaFrame::from_rgba8(1, 1, vec![0; 4])?;
                    let handle = host.register_image_wrapper(frame, false)?;
                    handles.push(handle);
                    let header = [1, handle].map(u32::to_le_bytes).concat();
                    vm.write(record, &header)?;
                }
                let args = [base, 2].map(u32::to_le_bytes).concat();
                assert_eq!(host.call(704, &args, &mut vm)?, NativeReturn::Continue);
                assert!(handles.into_iter().all(|handle| !host.image_handles.contains_key(&handle)));
                assert_eq!(vm.read(base, cleared.len())?, cleared);

                let layered = vm.allocate_region(b"image/missing,1.hg3\0")?;
                let args = [layered, base, 2].map(u32::to_le_bytes).concat();
                let result = host.call(703, &args, &mut vm);
                if stride == 0x88 {
                    assert!(result.unwrap_err().to_string().contains("narrow selector prefix"));
                } else {
                    assert_eq!(result?, NativeReturn::Value(0));
                }
                Ok::<_, anyhow::Error>(())
            })?;
        }
        Ok(())
    }

    #[test]
    fn native_image_records_routes_legacy_selection_pointer_without_product_digest() -> anyhow::Result<()> {
        let sample = game("\u{3046}\u{305f}\u{3066}\u{3081}\u{3050}\u{308a}/UTATEMEGURI.exe");
        if !sample.is_file() {
            return Ok(());
        }
        let temp = crate::tests::TempDir::new();
        let mut player = HeadlessPlayer::open_with_save_root(&temp.0, temp.0.join("profile"))?;
        let table = OrdinaryNativeTable::from_pe(&std::fs::read(sample)?).expect("legacy native table");
        player.with_host(|host| {
            assert!(host.data_services.css_task_pe_sha256.is_none());
            host.data_services.ordinary_native_table = Some(table);
            let mut vm = Vm::new();
            for force in [0, 1] {
                let args = [16_000u32, 13_000, 10, force].map(u32::to_le_bytes).concat();
                assert_eq!(host.call(703, &args, &mut vm)?, NativeReturn::Continue);
            }
            Ok(())
        })
    }
}

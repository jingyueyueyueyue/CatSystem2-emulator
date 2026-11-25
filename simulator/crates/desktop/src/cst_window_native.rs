//! Ordinary379..384: independent history windows over a live CST context.
//! Cursor rotation and endpoint rules follow the fixed PE's 0x5a2dd0.
use std::collections::BTreeMap;

use cs2_script::{CstDocument, NativeReturn, Result, ScriptError, Vm};
use cs2_script::cst_snapshot::CstDocumentSnapshot;
use cs2_script::data_snapshot::{CstWindowSnapshot, CstWindowSlotSnapshot};
use super::{LoggingHost, native_state_error, read_u32, trim_nul, write_kcs_terminated};

const MAX_SLOTS: usize = 4096;
const MAX_SCAN: usize = 1_000_000;
const MAX_STORAGE: usize = 256 * 1024 * 1024;
const MAX_TEXT: usize = 64 * 1024 * 1024;

#[derive(Default)]
pub(super) struct CstWindowStore {
    objects: BTreeMap<u32, HistoryWindow>,
}

struct HistoryWindow {
    context: u32,
    last_history: i32,
    slots: Vec<HistorySlot>,
}

#[derive(Clone, Copy)]
struct SlotCursor {
    history: i32,
    input: i32,
    saved_count: u32,
    valid: bool,
}

struct HistorySlot {
    cursor: SlotCursor,
    document: Option<CstDocument>,
}

impl HistorySlot {
    fn empty() -> Self {
        Self { cursor: SlotCursor { history: -1, input: -1, saved_count: u32::MAX, valid: false },
            document: None }
    }
}

impl CstWindowStore {
    pub(super) fn snapshots(&self) -> Result<Vec<CstWindowSnapshot>> {
        self.objects.iter().map(|(&handle, window)| {
            let saved = CstWindowSnapshot {
                handle, context: window.context, last_history: window.last_history,
                slots: window.slots.iter().map(|slot| Ok(CstWindowSlotSnapshot {
                    history: slot.cursor.history, input: slot.cursor.input,
                    saved_count: slot.cursor.saved_count, valid: slot.cursor.valid,
                    document: slot.document.as_ref().map(CstDocumentSnapshot::capture).transpose()?,
                })).collect::<Result<_>>()?,
            };
            saved.storage_bytes()?;
            Ok(saved)
        }).collect()
    }

    pub(super) fn from_snapshots(snapshots: &[CstWindowSnapshot]) -> Result<Self> {
        let mut objects = BTreeMap::new();
        for saved in snapshots {
            saved.storage_bytes()?;
            let window = HistoryWindow {
                context: saved.context, last_history: saved.last_history,
                slots: saved.slots.iter().map(|slot| Ok(HistorySlot {
                    cursor: SlotCursor { history: slot.history, input: slot.input,
                        saved_count: slot.saved_count, valid: slot.valid },
                    document: slot.document.as_ref().map(CstDocumentSnapshot::restore).transpose()?,
                })).collect::<Result<_>>()?,
            };
            if objects.insert(saved.handle, window).is_some() {
                return Err(native_state_error("CST window snapshot repeats an object identity"));
            }
        }
        Ok(Self { objects })
    }
}

impl LoggingHost<'_> {
    pub(super) fn cst_window_native(
        &mut self, index: u32, args: &[u8], vm: &mut Vm,
    ) -> Result<NativeReturn> {
        let handle = read_u32(args, 0)?;
        if index == 379 {
            if handle == 0 { return Ok(NativeReturn::Value(0)); }
            let count = read_u32(args, 4)? as i32;
            if count <= 0 || count as usize > MAX_SLOTS {
                return Err(native_state_error("CST history window requires 1..4096 slots"));
            }
            let context = self.cst_contexts.get(&handle)
                .ok_or_else(|| native_state_error("native 379 invalid/wrong-type CST context"))?;
            let last = context.history.len().checked_sub(1).map_or(-1, |index| index as i32);
            let mut window = HistoryWindow { context: handle, last_history: last,
                slots: (0..count).map(|_| HistorySlot::empty()).collect() };
            if last >= 0 {
                let saved = context.history[last as usize].value;
                window.slots[0].cursor = SlotCursor {
                    history: last, input: saved as i32, saved_count: saved, valid: false,
                };
                let mut scans = 0;
                loop {
                    scan_limit(&mut scans)?;
                    let valid = read_slot(self, &mut window, 0)?;
                    window.slots[0].cursor.valid = valid;
                    if valid || !previous_input(self, &mut window, 0)? { break; }
                }
                if count > 1 {
                    for _ in 1..count { shift(self, &mut window, false)?; }
                } else if window.slots[0].cursor.input as u32 == window.slots[0].cursor.saved_count {
                    shift(self, &mut window, false)?;
                }
            }
            let handle = self.allocate_handle();
            self.data_services.cst_windows.objects.insert(handle, window);
            return Ok(NativeReturn::Value(handle));
        }
        // These wrappers resolve their output starts before testing the window.
        // Address zero is a valid VM address, never a native null pointer.
        let outputs = if matches!(index, 382 | 384) {
            Some([(read_u32(args, 8)?, output_start(vm, read_u32(args, 8)?)?),
                (read_u32(args, 12)?, output_start(vm, read_u32(args, 12)?)?)])
        } else if index == 383 {
            Some([(read_u32(args, 8)?, output_start(vm, read_u32(args, 8)?)?), (0, false)])
        } else { None };
        if handle == 0 {
            return Ok(if index == 380 { NativeReturn::Continue } else { NativeReturn::Value(0) });
        }
        let mut window = self.data_services.cst_windows.objects.remove(&handle)
            .ok_or_else(|| native_state_error("CST window operation has an invalid/wrong-type object"))?;
        if index == 380 { return Ok(NativeReturn::Continue); }
        let result = (|| {
            if index == 381 {
                return Ok(NativeReturn::Value(u32::from(shift(self, &mut window, read_u32(args, 4)? != 0)?)));
            }
            let slot = read_u32(args, 4)? as usize;
            let Some(cursor) = window.slots.get(slot).map(|slot| slot.cursor).filter(|cursor| cursor.valid)
            else { return Ok(NativeReturn::Value(0)); };
            if index == 384 {
                let outputs = outputs.unwrap();
                if !outputs[0].1 || !outputs[1].1 {
                    return Err(native_state_error("native 384 valid window has an invalid output start"));
                }
                vm.write(outputs[0].0, &cursor.history.to_le_bytes())?;
                vm.write(outputs[1].0, &cursor.input.to_le_bytes())?;
                return Ok(NativeReturn::Value(1));
            }
            if !matches!(index, 382 | 383) {
                return Err(ScriptError::UnimplementedNativeCall { index });
            }
            let ordinal = if index == 383 { read_u32(args, 12)? } else { 0 };
            if !load_slot(self, &mut window, slot)? { return Ok(NativeReturn::Value(0)); }
            let outputs = outputs.unwrap();
            // A successful load clears output even if no matching record exists.
            for &(address, valid) in &outputs {
                if valid { vm.write(address, &[0])?; }
            }
            let current = &window.slots[slot];
            let records = usize::try_from(current.cursor.input).ok()
                .and_then(|input| current.document.as_ref()?.input_records(input)).unwrap_or(&[]);
            if index == 383 {
                let Some(record) = records.iter().filter(|record| record.kind == 0x30)
                    .nth(ordinal as usize) else { return Ok(NativeReturn::Value(0)); };
                let text = trim_nul(&record.text);
                if text.len() >= 4096 {
                    return Err(native_state_error("CST window command exceeds the original 4096-byte scratch"));
                }
                if outputs[0].1 { write_kcs_terminated(vm, outputs[0].0, text, 1)?; }
                return Ok(NativeReturn::Value(1));
            }
            let mut found = false;
            // Argument +8 is Message (0x20); +12 is Name (0x21).
            // Raw records are concatenated, without localization or $str expansion.
            for record in records {
                let target = match record.kind { 0x20 => 0, 0x21 => 1, _ => continue };
                found = true;
                let (address, valid) = outputs[target];
                if !valid { continue; }
                let text = trim_nul(&record.text);
                if text.len() >= 4096 {
                    return Err(native_state_error("CST window record exceeds the original 4096-byte scratch"));
                }
                // Re-read after every write: aliased/overlapping VM outputs
                // observe the same append order as the original wrapper.
                let mut value = vm.read_string(address)?;
                let length = value.len().checked_add(text.len()).filter(|&length| length < MAX_TEXT)
                    .ok_or_else(|| native_state_error("CST window text exceeds 64 MiB"))?;
                value.reserve(length - value.len());
                value.extend_from_slice(text);
                write_kcs_terminated(vm, address, &value, 1)?;
            }
            Ok(NativeReturn::Value(u32::from(found)))
        })();
        self.data_services.cst_windows.objects.insert(handle, window);
        result
    }
}

fn output_start(vm: &Vm, address: u32) -> Result<bool> {
    match vm.read(address, 1) {
        Ok(_) => Ok(true),
        Err(ScriptError::MemoryOutOfBounds { .. }) => Ok(false),
        Err(error) => Err(error),
    }
}

fn history_position(host: &LoggingHost<'_>, window: &HistoryWindow, requested: i32)
    -> Result<Option<(i32, u32, Vec<u8>)>>
{
    let context = host.cst_contexts.get(&window.context)
        .ok_or_else(|| native_state_error("CST window source context was released"))?;
    let Some(last) = context.history.len().checked_sub(1) else { return Ok(None); };
    let actual = usize::min(requested as u32 as usize, last);
    let entry = &context.history[actual];
    let end = entry.name.iter().position(|&byte| byte == 0)
        .ok_or_else(|| native_state_error("CST history window source name has no terminator"))?;
    Ok(Some((actual as i32, entry.value, entry.name[..end].to_vec())))
}

fn load_slot(host: &LoggingHost<'_>, window: &mut HistoryWindow, slot: usize) -> Result<bool> {
    let Some((history, count, name)) = history_position(host, window, window.slots[slot].cursor.history)?
    else { window.slots[slot].cursor.history = -1; return Ok(false); };
    window.slots[slot].cursor.history = history;
    window.slots[slot].cursor.saved_count = count;
    // The internal scene starts with an empty source debug name. 325/689090
    // does not set it to the resource filename. Do not invent a cache hit.
    if name.is_empty() { return Ok(true); }
    let Some((_, resource)) = host.cst_resource_with_source(b"scene", &name)? else { return Ok(false); };
    let bytes = &resource.bytes;
    if bytes.is_empty() { return Ok(false); }
    if bytes.get(..8) != Some(b"CatScene") { return Ok(false); }
    let packed = read_u32(bytes, 8)? as usize;
    let plain = read_u32(bytes, 12)? as usize;
    if 16usize.checked_add(if packed == 0 { plain } else { packed }) != Some(bytes.len()) {
        window.slots[slot].document = None;
        return Ok(false);
    }
    let mut document = CstDocument::parse(bytes)?;
    super::cst_context_native::repair_routed_scene_text(host.resources, &resource, &mut document);
    let size = document_storage(&document)?;
    let retained = window.slots.iter().enumerate().filter(|(index, _)| *index != slot)
        .filter_map(|(_, slot)| slot.document.as_ref()).try_fold(0usize, |total, document| {
            total.checked_add(document_storage(document)?).filter(|&total| total <= MAX_STORAGE)
                .ok_or_else(|| native_state_error("CST window scene storage exceeds 256 MiB"))
        })?;
    if retained.checked_add(size).is_none_or(|total| total > MAX_STORAGE) {
        return Err(native_state_error("CST window scene storage exceeds 256 MiB"));
    }
    window.slots[slot].document = Some(document);
    Ok(true)
}

fn document_storage(document: &CstDocument) -> Result<usize> {
    document.records.iter().try_fold(document.inputs.len().saturating_mul(16), |total, record| {
        total.checked_add(record.text.len()).and_then(|size| size.checked_add(32))
            .filter(|&size| size <= MAX_STORAGE)
            .ok_or_else(|| native_state_error("CST window scene storage exceeds 256 MiB"))
    })
}

fn read_slot(host: &LoggingHost<'_>, window: &mut HistoryWindow, slot: usize) -> Result<bool> {
    if !load_slot(host, window, slot)? { return Ok(false); }
    let slot = &window.slots[slot];
    Ok(usize::try_from(slot.cursor.input).ok()
        .and_then(|input| slot.document.as_ref()?.input_records(input))
        .is_some_and(|records| records.iter().any(|record| matches!(record.kind, 0x20 | 0x21))))
}

fn previous_input(host: &LoggingHost<'_>, window: &mut HistoryWindow, slot: usize) -> Result<bool> {
    let cursor = &mut window.slots[slot].cursor;
    cursor.input = cursor.input.wrapping_sub(1);
    if cursor.input >= 0 { return Ok(true); }
    let previous = cursor.history.wrapping_sub(1);
    cursor.history = previous;
    if previous < 0 { return Ok(false); }
    let Some((history, count, _)) = history_position(host, window, previous)? else { return Ok(false); };
    window.slots[slot].cursor.history = history;
    window.slots[slot].cursor.saved_count = count;
    window.slots[slot].cursor.input = count as i32;
    Ok(true)
}

fn next_input(host: &LoggingHost<'_>, window: &mut HistoryWindow, slot: usize) -> Result<bool> {
    let cursor = &mut window.slots[slot].cursor;
    cursor.input = cursor.input.wrapping_add(1);
    let crossed = if cursor.history == window.last_history {
        cursor.input > cursor.saved_count as i32
    } else { cursor.input >= cursor.saved_count as i32 };
    if !crossed { return Ok(true); }
    let next = cursor.history.wrapping_add(1);
    cursor.history = next;
    if next > window.last_history { return Ok(false); }
    let Some((history, count, _)) = history_position(host, window, next)? else { return Ok(false); };
    window.slots[slot].cursor.history = history;
    window.slots[slot].cursor.saved_count = count;
    window.slots[slot].cursor.input = 0;
    Ok(true)
}

fn shift(host: &LoggingHost<'_>, window: &mut HistoryWindow, forward: bool) -> Result<bool> {
    if !forward {
        window.slots.rotate_right(1);
        let saved = window.slots[0].cursor;
        if window.slots.len() >= 2 { window.slots[0].cursor = window.slots[1].cursor; }
        window.slots[0].cursor.valid = false;
        let mut scans = 0;
        while previous_input(host, window, 0)? {
            scan_limit(&mut scans)?;
            if read_slot(host, window, 0)? {
                window.slots[0].cursor.valid = true;
                return Ok(true);
            }
        }
        window.slots[0].cursor = saved;
        window.slots.rotate_left(1);
        return Ok(false);
    }
    window.slots.rotate_left(1);
    let single = window.slots.len() == 1;
    let target = if single { 0 } else {
        let Some(previous) = (0..window.slots.len() - 1).rev()
            .find(|&index| window.slots[index].cursor.valid) else {
                window.slots.rotate_right(1);
                return Ok(false);
            };
        previous + 1
    };
    let saved = window.slots[target].cursor;
    if !single { window.slots[target].cursor = window.slots[target - 1].cursor; }
    window.slots[target].cursor.valid = false;
    let mut scans = 0;
    while next_input(host, window, target)? {
        scan_limit(&mut scans)?;
        if read_slot(host, window, target)? {
            window.slots[target].cursor.valid = true;
            let cursor = window.slots[target].cursor;
            if single && cursor.history == window.last_history && cursor.input >= cursor.saved_count as i32 {
                window.slots[target].cursor = saved;
                return Ok(false);
            }
            return Ok(true);
        }
    }
    window.slots[target].cursor = saved;
    window.slots.rotate_right(1);
    Ok(false)
}

fn scan_limit(scans: &mut usize) -> Result<()> {
    *scans += 1;
    if *scans > MAX_SCAN { return Err(native_state_error("CST history window exceeds one million search steps")); }
    Ok(())
}

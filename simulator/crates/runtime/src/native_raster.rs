//! Raster18 messages recovered from the ordinary VM wrappers.

use super::root_clock::primary_command_receiver;
use crate::pe_resources::Section;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RasterMessages {
    pub start: u32,
    pub stop: u32,
    pub query: u32,
    pub configure: u32,
    pub parameters: u32,
}

const KIND_QUERY: &[u8] = b"\x55\x8b\xec\x83\xe4\xf8\x83\xec\x08\x56\x8b\x75\x0c\x8b\x06\x57\
    \x8b\x7d\x10\x8b\x17\x8d\x4c\x24\x0c\x51\x8b\x4e\x04\x6a\x00\x6a\x00\x6a\x00\x6a\x0c\
    \x52\x8b\x50\x10\x51\x8b\xce\xff\xd2\x8b\x44\x24\x0c";
const START_STOP: &[u8] = b"\x8b\x4c\x24\x08\x8b\x01\x8b\x40\x10\x8d\x54\x24\x08\x52\
    \x8b\x54\x24\x10\x8b\x12\x6a\x00\x6a\x00\x6a\x00\x68";
const CALL_VOID: &[u8] = b"\x52\x8b\x51\x04\x52\xff\xd0\xb8\x02\x00\x00\x00\xc2\x10\x00";
const LEGACY_FRAMELESS_QUERY_RETURN: &[u8] = b"\x52\x8b\x51\x04\x52\xff\xd0\
    \x8b\x4c\x24\x10\x8b\x54\x24\x08\x89\x11\xb8\x01\x00\x00\x00\xc2\x10\x00";
const LEGACY_FRAMELESS_KIND_QUERY: &[u8] = b"\x56\x8b\x74\x24\x0c\x8b\x06\x57\
    \x8b\x7c\x24\x14\x8b\x17\x8d\x4c\x24";
const LEGACY_FRAMELESS_KIND_DISPATCH: &[u8] = b"\x51\x8b\x4e\x04\x6a\x00\x6a\x00\
    \x6a\x00\x6a\x0c\x52\x8b\x50\x10\x51\x8b\xce\xff\xd2\x8b\x44\x24";
const LEGACY_FRAMELESS_CONFIGURE_ARGUMENT: &[u8] = b"\x8b\x47\x04\x8b\x4c\x24\x0c\x50";
const LEGACY_FRAMELESS_PARAMETERS_ARGUMENT: &[u8] = b"\x8b\x47\x08\x8b\x4c\x24\x0c\x50";
const LEGACY_FRAMELESS_RECORD_ARGUMENTS: &[u8] = b"\x8d\x4c\x24\x10\x51\x6a\x00\x50\x6a\x00\x68";
const LEGACY_FRAMELESS_RECORD_DISPATCH: &[u8] = b"\x8b\x07\x8b\x4e\x04\x8b\x16\
    \x8b\x52\x10\x50\x51\x8b\xce\xff\xd2";
const LEGACY_FRAMELESS_RECORD_RETURN: &[u8] = b"\x5f\xb8\x02\x00\x00\x00\x5e\xc2\x10\x00";
const QUERY: &[u8] = b"\x55\x8b\xec\x83\xe4\xf8\x8b\x4d\x0c\x8b\x01\x8b\x40\x10\x83\xec\x08\
    \x8d\x54\x24\x04\x52\x8b\x55\x10\x8b\x12\x6a\x00\x6a\x00\x6a\x00\x68";
const QUERY_RETURN: &[u8] = b"\x52\x8b\x51\x04\x52\xff\xd0\x8b\x4d\x14\x8b\x54\x24\x04\x89\x11\
    \xb8\x01\x00\x00\x00\x8b\xe5\x5d\xc2\x10\x00";
const CALL_RECORD: &[u8] = b"\x8b\x07\x8b\x4e\x04\x8b\x16\x8b\x52\x10\x50\x51\x8b\xce\xff\xd2\
    \x5f\xb8\x02\x00\x00\x00\x5e\x8b\xe5\x5d\xc2\x10\x00";

const FRAMELESS_SEND: &[u8] = b"\x8b\x4c\x24\x08\x8d\x44\x24\x08\x50\x8b\x44\x24\x10\
    \x6a\x00\x8b\x11\x6a\x00\x6a\x00\x68";
const FRAMELESS_DISPATCH: &[u8] = b"\xff\x30\xff\x71\x04\xff\x52\x10";
const CONTINUE: &[u8] = b"\xb8\x02\x00\x00\x00\xc2\x10\x00";
const FRAMELESS_QUERY_RETURN: &[u8] = b"\x8b\x4c\x24\x10\x8b\x44\x24\x08\x89\x01\
    \xb8\x01\x00\x00\x00\xc2\x10\x00";
const FRAMELESS_CONFIGURE_SWITCH: &[u8] = b"\x51\x56\x8b\x74\x24\x10\x8d\x4c\x24\x04\x57\
    \x8b\x7c\x24\x18\x51\x8b\x06\x8b\xce\x6a\x00\x6a\x00\x6a\x00\x6a\x0d\
    \xff\x37\xff\x76\x04\xff\x50\x10\x8b\x44\x24\x08\x83\xc0\xef\x83\xf8\x0e\x0f\x87";
const FRAMELESS_CONFIGURE_ARGUMENT: &[u8] = b"\xff\x77\x04\x8b\x4c\x24\x14";
const FRAMELESS_CONFIGURE_DISPATCH: &[u8] = b"\xff\x37\x8b\x16\x8b\xce\xff\x76\x04\xff\x52\x10";
const FRAMELESS_CONFIGURE_RETURN: &[u8] = b"\x5f\xb8\x02\x00\x00\x00\x5e\x59\xc2\x10\x00";
const FRAMELESS_PARAMETERS_SWITCH: &[u8] = b"\x56\x8b\x74\x24\x10\x8d\x4c\x24\x0c\x57\
    \x8b\x7c\x24\x10\x51\x6a\x00\x6a\x00\x8b\x07\x8b\xcf\x6a\x00\x6a\x0d\
    \xff\x36\xff\x77\x04\xff\x50\x10\x8b\x44\x24\x10\x83\xf8\x12\x74";
const FRAMELESS_PARAMETERS_ARGUMENT: &[u8] = b"\xff\x76\x08\x8b\x4c\x24\x10";
const FRAMELESS_PARAMETERS_DISPATCH: &[u8] = b"\xff\x36\x8b\x17\x8b\xcf\xff\x77\x04\xff\x52\x10";
const FRAMELESS_PARAMETERS_RETURN: &[u8] = b"\x5f\xb8\x02\x00\x00\x00\x5e\xc2\x10\x00";

// Raster18's early receiver body checks its owner, then indexes an in-code
// message jump table. The selector at start + 4 may point to the default.
const RECIPIENT_SWITCH: &[u8] = b"\x55\x8b\xec\x83\xe4\xf8\x51\x53\x56\x8b\xf1\x8b\x4e\x6c\x57\x33\xff\
    \x3b\xcf\x0f\x84\x01\x01\x00\x00\x8b\x45\x0c\x05";
const RECIPIENT_DEFAULT: &[u8] = b"\x5f\x5e\xb8\x01\x00\x00\x00\x5b\x8b\xe5\x5d\xc2\x18\x00";
const FRAMELESS_RECIPIENT_SWITCH: &[u8] = b"\x56\x8b\xf1\x8b\x4e";
const FRAMELESS_RECIPIENT_RANGE: &[u8] = b"\x83\xf8\x07\x0f\x87";
const FRAMELESS_RECIPIENT_DEFAULT: &[u8] = b"\xb8\x01\x00\x00\x00\x5e\xc2\x18\x00";
const RASTER_TYPE: &[u8] = b".?AVTask_Eff_Raster@@\0";

pub(super) fn messages<'a>(
    entries: &[(u32, &'a [u8])],
    image_base: u32,
    code: impl Fn(u32, bool) -> Option<&'a [u8]>,
) -> Option<RasterMessages> {
    let layout = if entries.get(196)?.1.starts_with(LEGACY_FRAMELESS_KIND_QUERY) {
        legacy_frameless_messages(entries, image_base, &code)?
    } else if entries.get(197)?.1.starts_with(FRAMELESS_SEND) {
        frameless_messages(entries, image_base, &code)?
    } else {
        early_messages(entries, image_base, &code)?
    };
    // The receiver families assign the triplet and the two record messages
    // consecutively, with the two groups six commands apart.
    let RasterMessages { start, stop, query, configure, parameters } = layout;
    (start != 0 && stop.checked_sub(start) == Some(1) && query.checked_sub(start) == Some(2)
        && configure.checked_sub(start) == Some(6) && parameters.checked_sub(start) == Some(7))
        .then_some(layout)
}

fn early_messages<'a>(
    entries: &[(u32, &'a [u8])],
    image_base: u32,
    code: &impl Fn(u32, bool) -> Option<&'a [u8]>,
) -> Option<RasterMessages> {
    let start = command(entries.get(197)?.1.strip_prefix(START_STOP)?, CALL_VOID)?;
    let stop = command(entries.get(198)?.1.strip_prefix(START_STOP)?, CALL_VOID)?;
    let query = command(entries.get(200)?.1.strip_prefix(QUERY)?, QUERY_RETURN)?;
    let (primary_rva, primary) = *entries.get(196)?;
    // Dispatch on receiver kind minus 17. Entry 1 is the Raster18 branch.
    let switch = primary
        .strip_prefix(KIND_QUERY)?
        .strip_prefix(b"\x83\xc0\xef\x83\xf8\x0e\x0f\x87")?;
    let table = switch.get(4..)?.strip_prefix(b"\xff\x24\x85")?;
    let table_rva = dword(table)?.checked_sub(image_base)?;
    let configure_rva = dword(code(table_rva, true)?.get(4..)?)?.checked_sub(image_base)?;
    let configure_branch = code(configure_rva, true)?;
    let record_decoder = relative_call(
        configure_rva.checked_add(8)?,
        configure_branch.strip_prefix(b"\x8b\x47\x04\x8b\x4d\x08\x50\x51")?,
    )?;
    let configure_command = configure_branch
        .get(13..)?
        .strip_prefix(b"\x8d\x4c\x24\x08\x51\x6a\x00\x50\x6a\x00\x68")?;
    let configure = dword(configure_command)?;
    let tail = configure_command.get(4..)?.strip_prefix(b"\xe9")?;
    let tail_rva = configure_rva
        .checked_add(33)?
        .checked_add_signed(dword(tail)? as i32)?;
    if tail_rva < primary_rva || !code(tail_rva, true)?.starts_with(CALL_RECORD) {
        return None;
    }

    let (secondary_rva, secondary) = *entries.get(201)?;
    let jump = secondary
        .strip_prefix(KIND_QUERY)?
        .strip_prefix(b"\x83\xf8\x12\x74")?;
    let displacement = *jump.first()? as i8;
    let parameters_offset = (KIND_QUERY.len() + 5).checked_add_signed(displacement as isize)?;
    let parameters_branch = secondary.get(parameters_offset..)?;
    let mode_check = parameters_branch.strip_prefix(b"\x83\x7f\x04\x00\x75")?;
    let record_call = mode_check.get(1..)?.strip_prefix(b"\x8b\x47\x08\x8b\x4d\x08\x50\x51")?;
    let parameters_rva = secondary_rva.checked_add(parameters_offset as u32)?;
    if relative_call(parameters_rva.checked_add(14)?, record_call)? != record_decoder
        || code(record_decoder, true).is_none()
    {
        return None;
    }
    let parameters = command(
        record_call.get(5..)?.strip_prefix(b"\x8d\x4c\x24\x0c\x51\x6a\x00\x50\x6a\x00\x68")?,
        CALL_RECORD,
    )?;
    Some(RasterMessages { start, stop, query, configure, parameters })
}

fn frameless_send(body: &[u8], return_body: &[u8]) -> Option<u32> {
    let immediate = body.strip_prefix(FRAMELESS_SEND)?;
    let tail = immediate.get(4..)?.strip_prefix(FRAMELESS_DISPATCH)?;
    tail.starts_with(return_body).then_some(())?;
    dword(immediate)
}

fn legacy_frameless_kind_query(body: &[u8], slot: u8) -> Option<&[u8]> {
    body.strip_prefix(LEGACY_FRAMELESS_KIND_QUERY)?
        .strip_prefix(&[slot][..])?
        .strip_prefix(LEGACY_FRAMELESS_KIND_DISPATCH)?
        .strip_prefix(&[slot][..])
}

fn legacy_frameless_messages<'a>(
    entries: &[(u32, &'a [u8])],
    image_base: u32,
    code: &impl Fn(u32, bool) -> Option<&'a [u8]>,
) -> Option<RasterMessages> {
    let start = command(entries.get(197)?.1.strip_prefix(START_STOP)?, CALL_VOID)?;
    let stop = command(entries.get(198)?.1.strip_prefix(START_STOP)?, CALL_VOID)?;
    let query = command(
        entries.get(200)?.1.strip_prefix(START_STOP)?,
        LEGACY_FRAMELESS_QUERY_RETURN,
    )?;

    let (primary_rva, primary) = *entries.get(196)?;
    let switch = legacy_frameless_kind_query(primary, 0x14)?
        .strip_prefix(b"\x83\xc0\xef\x83\xf8\x0e\x0f\x87")?;
    let fallback_rva = primary_rva.checked_add((primary.len() - switch.len() + 4) as u32)?
        .checked_add_signed(dword(switch)? as i32)?;
    let table_rva = dword(switch.get(4..)?.strip_prefix(b"\xff\x24\x85")?)?
        .checked_sub(image_base)?;
    // Slot 1 of the receiver-kind switch corresponds to kind 18.
    let configure_rva = dword(code(table_rva, true)?.get(4..)?)?.checked_sub(image_base)?;
    let record_call = code(configure_rva, true)?
        .strip_prefix(LEGACY_FRAMELESS_CONFIGURE_ARGUMENT)?;
    let decoder = relative_call(
        configure_rva.checked_add(LEGACY_FRAMELESS_CONFIGURE_ARGUMENT.len() as u32)?,
        record_call,
    )?;
    code(decoder, true)?;
    let immediate = record_call.get(5..)?.strip_prefix(LEGACY_FRAMELESS_RECORD_ARGUMENTS)?;
    let configure = dword(immediate)?;
    let tail = immediate.get(4..)?.strip_prefix(b"\xe9")?;
    let jump_end = LEGACY_FRAMELESS_CONFIGURE_ARGUMENT.len() + 5
        + LEGACY_FRAMELESS_RECORD_ARGUMENTS.len() + 4 + 5;
    let tail_rva = configure_rva.checked_add(jump_end as u32)?
        .checked_add_signed(dword(tail)? as i32)?;
    if tail_rva < primary_rva
        || tail_rva.checked_add(LEGACY_FRAMELESS_RECORD_DISPATCH.len() as u32)? != fallback_rva
        || !code(tail_rva, true)?.starts_with(LEGACY_FRAMELESS_RECORD_DISPATCH)
        || !code(fallback_rva, true)?.starts_with(LEGACY_FRAMELESS_RECORD_RETURN)
    {
        return None;
    }

    let (secondary_rva, secondary) = *entries.get(201)?;
    let jump = legacy_frameless_kind_query(secondary, 0x10)?.strip_prefix(b"\x83\xf8\x12\x74")?;
    let branch_offset = (secondary.len() - jump.len() + 1)
        .checked_add_signed(*jump.first()? as i8 as isize)?;
    let mode = secondary.get(branch_offset..)?.strip_prefix(b"\x83\x7f\x04\x00\x75")?;
    let record_call = mode.get(1..)?.strip_prefix(LEGACY_FRAMELESS_PARAMETERS_ARGUMENT)?;
    let call_offset = secondary.len() - record_call.len();
    if relative_call(secondary_rva.checked_add(call_offset as u32)?, record_call)? != decoder {
        return None;
    }
    let immediate = record_call.get(5..)?.strip_prefix(LEGACY_FRAMELESS_RECORD_ARGUMENTS)?;
    let return_body = immediate.get(4..)?.strip_prefix(LEGACY_FRAMELESS_RECORD_DISPATCH)?;
    let mode_jump = mode.get(1..)?.len() - return_body.len();
    if !return_body.starts_with(LEGACY_FRAMELESS_RECORD_RETURN)
        || usize::try_from(*mode.first()? as i8).ok()? != mode_jump
    {
        return None;
    }
    let parameters = dword(immediate)?;
    Some(RasterMessages { start, stop, query, configure, parameters })
}

fn frameless_messages<'a>(
    entries: &[(u32, &'a [u8])],
    image_base: u32,
    code: &impl Fn(u32, bool) -> Option<&'a [u8]>,
) -> Option<RasterMessages> {
    let start = frameless_send(entries.get(197)?.1, CONTINUE)?;
    let stop = frameless_send(entries.get(198)?.1, CONTINUE)?;
    let query = frameless_send(entries.get(200)?.1, FRAMELESS_QUERY_RETURN)?;

    let (primary_rva, primary) = *entries.get(196)?;
    let switch = primary.strip_prefix(FRAMELESS_CONFIGURE_SWITCH)?;
    let fallback_rva = primary_rva.checked_add((FRAMELESS_CONFIGURE_SWITCH.len() + 4) as u32)?
        .checked_add_signed(dword(switch)? as i32)?;
    let table = switch.get(4..)?.strip_prefix(b"\xff\x24\x85")?;
    let table_rva = dword(table)?.checked_sub(image_base)?;
    // The switch indexes receiver kind minus 17. Slot 1 is Raster18.
    let configure_rva = dword(code(table_rva, true)?.get(4..)?)?.checked_sub(image_base)?;
    let record_call = code(configure_rva, true)?.strip_prefix(FRAMELESS_CONFIGURE_ARGUMENT)?;
    let decoder = relative_call(configure_rva.checked_add(FRAMELESS_CONFIGURE_ARGUMENT.len() as u32)?, record_call)?;
    code(decoder, true)?;
    let immediate = record_call.get(5..)?.strip_prefix(b"\x8d\x4c\x24\x14\x51\x6a\x00\x50\x6a\x00\x68")?;
    let configure = dword(immediate)?;
    let tail_rva = configure_rva.checked_add((FRAMELESS_CONFIGURE_ARGUMENT.len() + 25) as u32)?
        .checked_add_signed(dword(immediate.get(4..)?.strip_prefix(b"\xe9")?)? as i32)?;
    if tail_rva < primary_rva || tail_rva.checked_add(FRAMELESS_CONFIGURE_DISPATCH.len() as u32)? != fallback_rva
        || !code(tail_rva, true)?.starts_with(FRAMELESS_CONFIGURE_DISPATCH)
        || !code(fallback_rva, true)?.starts_with(FRAMELESS_CONFIGURE_RETURN)
    {
        return None;
    }

    let (secondary_rva, secondary) = *entries.get(201)?;
    let jump = secondary.strip_prefix(FRAMELESS_PARAMETERS_SWITCH)?;
    let branch_offset = (FRAMELESS_PARAMETERS_SWITCH.len() + 1).checked_add_signed(*jump.first()? as i8 as isize)?;
    let mode = secondary.get(branch_offset..)?.strip_prefix(b"\x83\x7e\x04\x00\x75")?;
    let record_call = mode.get(1..)?.strip_prefix(FRAMELESS_PARAMETERS_ARGUMENT)?;
    let call_offset = branch_offset + 6 + FRAMELESS_PARAMETERS_ARGUMENT.len();
    if relative_call(secondary_rva.checked_add(call_offset as u32)?, record_call)? != decoder {
        return None;
    }
    let immediate = record_call.get(5..)?.strip_prefix(b"\x8d\x4c\x24\x10\x51\x6a\x00\x50\x6a\x00\x68")?;
    let tail = immediate.get(4..)?.strip_prefix(FRAMELESS_PARAMETERS_DISPATCH)?;
    if !tail.starts_with(FRAMELESS_PARAMETERS_RETURN)
        || usize::try_from(*mode.first()? as i8).ok()? != 5 + 11 + FRAMELESS_PARAMETERS_ARGUMENT.len()
            + 4 + FRAMELESS_PARAMETERS_DISPATCH.len()
    {
        return None;
    }
    let parameters = dword(immediate)?;
    Some(RasterMessages { start, stop, query, configure, parameters })
}

pub(super) fn finalize_passthrough<'a>(
    entries: &[(u32, &'a [u8])],
    messages: RasterMessages,
    image: &'a [u8],
    sections: &[Section],
    image_base: u32,
    file_bytes: impl Fn(u32, bool) -> Option<&'a [u8]>,
) -> Option<()> {
    let wrapper = entries.get(203)?.1;
    let finalize = if wrapper.starts_with(FRAMELESS_SEND) {
        frameless_send(wrapper, CONTINUE)?
    } else {
        command(wrapper.strip_prefix(START_STOP)?, CALL_VOID)?
    };
    let index = finalize.checked_sub(messages.start)?;
    if index != 4 {
        return None;
    }
    let code = |rva| file_bytes(rva, true);
    let receiver_rva = primary_command_receiver(image, sections, image_base, RASTER_TYPE, &code)?;
    let receiver = code(receiver_rva)?;
    if receiver.starts_with(FRAMELESS_RECIPIENT_SWITCH) {
        return frameless_finalize_passthrough(receiver_rva, receiver, messages.start, index, image_base, &code);
    }
    let selector = receiver.strip_prefix(RECIPIENT_SWITCH)?;
    if (dword(selector)? as i32).checked_neg()? as u32 != messages.start {
        return None;
    }
    let branch = selector.get(4..)?.strip_prefix(b"\x83\xf8\x07\x0f\x87")?;
    let default_rva = receiver_rva.checked_add((RECIPIENT_SWITCH.len() + 13) as u32)?
        .checked_add_signed(dword(branch)? as i32)?;
    let table_rva = dword(branch.get(4..)?.strip_prefix(b"\xff\x24\x85")?)?
        .checked_sub(image_base)?;
    let dispatch_rva = dword(code(table_rva)?.get(index as usize * 4..)?)?
        .checked_sub(image_base)?;
    (dispatch_rva == default_rva
        && code(default_rva)?.starts_with(RECIPIENT_DEFAULT)).then_some(())
}

fn frameless_finalize_passthrough<'a>(
    receiver_rva: u32,
    receiver: &[u8],
    start: u32,
    index: u32,
    image_base: u32,
    code: &impl Fn(u32) -> Option<&'a [u8]>,
) -> Option<()> {
    let prologue = receiver.strip_prefix(FRAMELESS_RECIPIENT_SWITCH)?;
    let (&owner_offset, prologue) = prologue.split_first()?;
    if !matches!(owner_offset, 0x6c | 0x70) { return None; }
    let owner_missing = prologue.strip_prefix(b"\x85\xc9\x0f\x84")?;
    let owner_missing_rva = receiver_rva.checked_add(14)?
        .checked_add_signed(dword(owner_missing)? as i32)?;
    let selector = owner_missing.get(4..)?.strip_prefix(b"\x8b\x44\x24\x0c\x05")?;
    if (dword(selector)? as i32).checked_neg()? as u32 != start { return None; }
    let bound = selector.get(4..)?.strip_prefix(FRAMELESS_RECIPIENT_RANGE)?;
    let default_rva = receiver_rva.checked_add(32)?.checked_add_signed(dword(bound)? as i32)?;
    if default_rva != owner_missing_rva || !code(default_rva)?.starts_with(FRAMELESS_RECIPIENT_DEFAULT) {
        return None;
    }
    let table_rva = dword(bound.get(4..)?.strip_prefix(b"\xff\x24\x85")?)?.checked_sub(image_base)?;
    let table = code(table_rva)?.get(..8 * 4)?;
    let target = dword(table.get(index as usize * 4..)?)?.checked_sub(image_base)?;
    (target == default_rva).then_some(())
}

fn dword(bytes: &[u8]) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(..4)?.try_into().ok()?))
}

fn relative_call(rva: u32, bytes: &[u8]) -> Option<u32> {
    let displacement = dword(bytes.strip_prefix(b"\xe8")?)? as i32;
    rva.checked_add(5)?.checked_add_signed(displacement)
}

fn command(bytes: &[u8], following: &[u8]) -> Option<u32> {
    if !bytes.get(4..)?.starts_with(following) {
        return None;
    }
    dword(bytes)
}

#[cfg(test)]
mod tests {
    use super::RasterMessages;
    use crate::native_table::OrdinaryNativeTable;

    #[test]
    fn native_raster_recovers_legacy_frameless_wrappers() -> std::io::Result<()> {
        let games = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../games");
        let expected = Some(RasterMessages {
            start: 240, stop: 241, query: 242, configure: 246, parameters: 247,
        });
        for name in ["Grisaia3.exe", "Grisaia3_cn.exe"] {
            let path = games.join("\u{7070}\u{8272}\u{7684}\u{4e50}\u{56ed}v1").join(name);
            if !path.is_file() {
                continue;
            }
            let pe = std::fs::read(&path)?;
            let table = OrdinaryNativeTable::from_pe(&pe).unwrap();
            assert_eq!(table.raster_messages(), expected, "{name}");
            assert!(table.raster_finalize_passes_through(), "{name}");

            for offset in [0xd1b57, 0xd1d0c, 0xd1e37, 0xd1ea9, 0xd1ec7] {
                let mut changed = pe.clone();
                changed[offset] ^= 1;
                let table = OrdinaryNativeTable::from_pe(&changed).unwrap();
                assert_eq!(table.raster_messages(), None, "{name} wrapper at {offset:#x}");
            }
            for offset in [0xd1f7a, 0xa4f90, 0xa4f75] {
                let mut changed = pe.clone();
                changed[offset] ^= 1;
                let table = OrdinaryNativeTable::from_pe(&changed).unwrap();
                assert_eq!(table.raster_messages(), expected, "{name} at {offset:#x}");
                assert!(!table.raster_finalize_passes_through(), "{name} finalize at {offset:#x}");
            }
        }
        Ok(())
    }

    #[test]
    fn native_raster_recovers_registered_early_wrapper_family() -> std::io::Result<()> {
        let games = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../games");
        let samples = [
            (
                "\u{7070}\u{8272}\u{7684}\u{679c}\u{5b9e}/Grisaia.exe",
                RasterMessages { start: 231, stop: 232, query: 233, configure: 237, parameters: 238 },
                [0x52835, 0x85da4, 0x85f6e],
                &[][..],
            ),
            (
                "\u{7a81}\u{7136}\u{4e4b}\u{95f4}\u{53d1}\u{73b0}\u{6211}\u{5df2}\u{604b}\u{4e0a}\u{4f60}/cs2_cn.exe",
                RasterMessages { start: 233, stop: 234, query: 235, configure: 239, parameters: 240 },
                [0x8747b, 0x87424, 0x875df],
                &[0x876ab, 0x690f8, 0x690dc][..],
            ),
        ];
        let messages = |data: &[u8]| {
            OrdinaryNativeTable::from_pe(data).and_then(|table| table.raster_messages())
        };
        for (relative, expected, offsets, finalize_offsets) in samples {
            let game = games.join(relative);
            if !game.is_file() {
                continue;
            }
            let pe = std::fs::read(game)?;
            assert_eq!(messages(&pe), Some(expected), "{relative}");
            assert!(OrdinaryNativeTable::from_pe(&pe).unwrap().raster_finalize_passes_through(), "{relative}");
            for offset in offsets {
                let mut changed = pe.clone();
                changed[offset] ^= 1;
                assert_eq!(messages(&changed), None, "{relative} mutated at {offset:#x}");
            }
            for &offset in finalize_offsets {
                let mut changed = pe.clone();
                changed[offset] ^= 1;
                let table = OrdinaryNativeTable::from_pe(&changed).unwrap();
                assert_eq!(table.raster_messages(), Some(expected), "{offset:#x}");
                assert!(!table.raster_finalize_passes_through(), "{offset:#x}");
            }
        }
        Ok(())
    }

    #[test]
    fn native_raster_recovers_early_finalize_at_relocated_receiver() -> std::io::Result<()> {
        let games = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../games");
        let path = games.join("\u{30ab}\u{30df}\u{30ab}\u{30bc}\u{2606}\u{30a8}\u{30af}\u{30b9}\u{30d7}\u{30ed}\u{30fc}\u{30e9}\u{30fc}!/cs2.exe");
        if !path.is_file() { return Ok(()); }
        let pe = std::fs::read(path)?;
        let table = OrdinaryNativeTable::from_pe(&pe).unwrap();
        let expected = Some(RasterMessages { start: 231, stop: 232, query: 233, configure: 237, parameters: 238 });
        assert_eq!(table.raster_messages(), expected);
        assert!(table.raster_finalize_passes_through());

        // The wrapper's command byte, the jump-table entry, and its default
        // destination are all independently required for slot 203.
        for offset in [0x86039, 0x67c78, 0x67c5c] {
            let mut changed = pe.clone();
            changed[offset] ^= 1;
            let table = OrdinaryNativeTable::from_pe(&changed).unwrap();
            assert_eq!(table.raster_messages(), expected, "{offset:#x}");
            assert!(!table.raster_finalize_passes_through(), "{offset:#x}");
        }
        Ok(())
    }

    #[test]
    fn native_raster_recovers_registered_frameless_wrapper_family() -> std::io::Result<()> {
        let games = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../games");
        let samples = [
            (
                "\u{679c}\u{3064}\u{308b}\u{3053}\u{3068}\u{306a}\u{304d}\u{672a}\u{6765}\u{3088}\u{308a}/Hatsumira.exe",
                247,
                [0x108418, 0x10854a, 0x1085d7],
                [0xb4f48, 0xb4f2d],
            ),
            (
                "Yukikoi Melt/YukikoiMelt.exe", 263,
                [0x19a118, 0x19a23a, 0x19a2c7],
                [0x14617c, 0x146163],
            ),
            (
                "Happiness\u{ff01}2 \u{6a31}\u{82b1}\u{76db}\u{5178}/cs2_crack.exe", 263,
                [0x1996c2, 0x1999a9, 0x1999c1],
                [0x1457a0, 0x1455e3],
            ),
            (
                "cs2_full_v401/system/cs2_open.exe", 252,
                [0x128d78, 0x128eaa, 0x128f37],
                [0xc8198, 0xc817d],
            ),
        ];
        for (relative, start, wrapper_offsets, receiver_offsets) in samples {
            let game = games.join(relative);
            if !game.is_file() {
                continue;
            }
            let pe = std::fs::read(game)?;
            let layout = RasterMessages {
                start, stop: start + 1, query: start + 2,
                configure: start + 6, parameters: start + 7,
            };
            let table = OrdinaryNativeTable::from_pe(&pe).unwrap();
            assert_eq!(table.raster_messages(), Some(layout), "{relative}");
            assert!(table.raster_finalize_passes_through(), "{relative}");
            for offset in wrapper_offsets {
                let mut changed = pe.clone();
                changed[offset] ^= 1;
                assert_eq!(
                    OrdinaryNativeTable::from_pe(&changed).unwrap().raster_messages(),
                    None,
                    "{relative} wrapper mutation at {offset:#x}",
                );
            }
            for offset in receiver_offsets {
                let mut changed = pe.clone();
                changed[offset] ^= 1;
                let table = OrdinaryNativeTable::from_pe(&changed).unwrap();
                assert_eq!(table.raster_messages(), Some(layout));
                assert!(!table.raster_finalize_passes_through(), "{relative} receiver mutation at {offset:#x}");
            }
        }
        Ok(())
    }
}

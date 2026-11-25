//! CSS record lookup and dispatch from registered ordinary slot 576.
use super::root_clock::{primary_command_receiver, primary_task_vtable, read_word, relative_target};
use crate::pe_resources::Section;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CssRestoreCommand {
    pub code: u32,
    pub common_receivers_preserve_output: bool,
    pub view_rejects_restore: bool,
}

pub(super) fn legacy_view_lifecycle(wrapper: &[u8]) -> Option<u32> {
    let tail = wrapper.strip_prefix(b"\x8b\x44\x24\x0c\x8b\x00\x85\xc0\x74\x1f\x8b\x4c\x24\x08\x8b\x11\x8b\x52\x0c\x56\x8d\x74\x24\x10\x56\x6a\0\x6a\0\x6a\0\x6a")?;
    let (&command, tail) = tail.split_first()?;
    (matches!(command, 60 | 61)
        && tail.starts_with(b"\x50\x8b\x41\x04\x50\xff\xd2\x5e\xb8\x02\0\0\0\xc2\x10\0"))
        .then_some(u32::from(command))
}

pub(super) fn kc_view_binding<'a>(
    load: &[u8],
    bind: &[u8],
    image: &[u8],
    sections: &[Section],
    image_base: u32,
    code: impl Fn(u32) -> Option<&'a [u8]>,
) -> Option<()> {
    // The loader fills a texture configuration and then sends the model name.
    if !load.starts_with(b"\x81\xec\x18\x03\0\0")
        || !at(load, 0x9a, b"\x6a\0\x68\x4c\x01\0\0\x50\x51\x8b\xce\xff\xd2")
        || !at(load, 0xef, b"\x68\x4d\x01\0\0\x51\x50\x8b\xce\xff\xd2")
        || !bind.starts_with(b"\x83\xec\x10\x8b\x44\x24\x1c\xd9\x40\x04\x8b\x4c\x24\x18\x8b\x11\xd9\x1c\x24\xd9\x40\x08\x8b\0\x8b\x52\x0c\xd9\x5c\x24\x04\x56\x8d\x74\x24\x20\x56\x6a\0\x8d\x74\x24\x0c\x56\x6a\0\x68\x70\x01\0\0\x50\x8b\x41\x04\x50\xff\xd2\xb8\x02\0\0\0\x5e\x83\xc4\x10\xc2\x10\0")
    {
        return None;
    }
    let (_, vtable) = super::root_clock::primary_task_vtable_location::<7>(
        image, sections, image_base, b".?AVkcTask3DView@@\0", &code, false,
    )?;
    let receiver = code(vtable[6])?;
    let texture = view_leaf(receiver, 332, image_base, &code)?;
    let model = view_leaf(receiver, 333, image_base, &code)?;
    let scale = view_leaf(receiver, 368, image_base, &code)?;
    (at(texture, 0x16, b"\x8d\x86\x04\x01\0\0")
        && at(texture, 0x25, b"\xd9\x86\x08\x02\0\0\xd9\x9b\x6c\x02\0\0")
        && model.starts_with(b"\x8b\x7d\x14\x8d\x43\x6c\x50\x57\xe8")
        && scale.starts_with(b"\x8b\x45\x14\xd9\x40\x04\x8b\x73\x64\x8b\x53\x5c\x83\xec\x08"))
        .then_some(())
}

pub(super) fn registered<'a>(
    (rva, wrapper): (u32, &[u8]),
    image: &[u8],
    sections: &[Section],
    image_base: u32,
    code: impl Fn(u32) -> Option<&'a [u8]>,
) -> Option<CssRestoreCommand> {
    let (command, lookup) = wrapper_command(wrapper)?;
    let lookup = code(relative_target(rva, lookup, read_word(wrapper, lookup)?)?)?;
    if ![
        b"\x55\x8b\xec\x8b\x09\x56\x33\xf6\x8b\x11\x85\xd2\x75\x07\x33\xc0\x5e\x5d\xc2\x08\0\x8b\x49\x04\x57\x85\xc9\x74\x18\x8b\x7d\x08\x39\x3a\x74\x19\x8b\x42\x04\x83\xc6\x08\x83\xc2\x08\x03\xf0\x03\xd0\x3b\xf1\x72\xeb\x5f\x33\xc0\x5e\x5d\xc2\x08\0\x8b\x45\x0c\x8b\x4a\x04\x5f\x5e\x89\x08\x8d\x42\x08\x5d\xc2\x08\0".as_slice(),
        b"\x55\x8b\xec\x56\x8b\x31\x33\xc0\x57\x8b\x0e\x85\xc9\x74\x26\x8b\x76\x04\x85\xf6\x74\x1f\x8b\x7d\x08\x0f\x1f\x80\0\0\0\0\x8b\x51\x04\x39\x39\x74\x16\x83\xc0\x08\x83\xc1\x08\x03\xc2\x03\xca\x3b\xc6\x72\xeb\x5f\x33\xc0\x5e\x5d\xc2\x08\0\x8b\x45\x0c\x5f\x5e\x89\x10\x8d\x41\x08\x5d\xc2\x08\0",
        b"\x8b\x09\x8b\x01\x33\xd2\x85\xc0\x75\x03\xc2\x08\0\x56\x8b\x71\x04\x57\x85\xf6\x76\x1d\x8b\x7c\x24\x0c\x8d\x9b\0\0\0\0\x8b\x48\x04\x39\x38\x74\x13\x8d\x54\x0a\x08\x8d\x44\x08\x08\x3b\xd6\x72\xed\x5f\x33\xc0\x5e\xc2\x08\0\x8b\x54\x24\x10\x5f\x89\x0a\x83\xc0\x08\x5e\xc2\x08\0",
    ].iter().any(|layout| lookup.starts_with(layout)) {
        return None;
    }
    let receiver = |name: &[u8]| primary_command_receiver(image, sections, image_base, name, &code);
    let common = || {
        let top = receiver(b".?AVkcTaskTop@@\0")?;
        let script = receiver(b".?AVkcTaskScript@@\0")?;
        let base = primary_task_vtable(image, sections, image_base, b".?AVkcTaskBase@@\0", &code)?[6];
        top_default(top, command, &code)?;
        script_default(script, command, &code)?;
        base_default(base, command, &code)
    };
    let rejects_restore = |receiver| {
        let leaf = view_leaf(code(receiver)?, command, image_base, &code)?;
        leaf.starts_with(b"\x8b\x45\x1c\xc7\0\0\0\0\0\x33\xc0\x8b\x8c\x24\x70\x01\0\0\x64\x89\x0d\0\0\0\0\x59\x5f\x5e\x5b\x8b\xe5\x5d\xc2\x18\0")
            .then_some(())
    };
    let view_type = b".?AVkcTask3DView@@\0";
    let modern_view = primary_command_receiver(image, sections, image_base, view_type, &code);
    let view_rejects_restore = modern_view.and_then(rejects_restore).or_else(|| {
        let (_, vtable) = super::root_clock::primary_task_vtable_location::<7>(
            image, sections, image_base, view_type, &code, false,
        )?;
        rejects_restore(vtable[6])
    }).is_some();
    Some(CssRestoreCommand {
        code: command,
        common_receivers_preserve_output: common().is_some(),
        view_rejects_restore,
    })
}

fn view_leaf<'a>(
    receiver: &[u8],
    command: u32,
    image_base: u32,
    code: &impl Fn(u32) -> Option<&'a [u8]>,
) -> Option<&'a [u8]> {
    let dispatch = receiver.get(0x45..)?;
    if !dispatch.starts_with(b"\x8b\x45\x0c\x05")
        || !at(dispatch, 8, b"\x83\xf8\x2c\x0f\x87")
        || !at(dispatch, 17, b"\xff\x24\x85")
    {
        return None;
    }
    let selector = command.wrapping_add(read_word(dispatch, 4)?);
    if selector > 0x2c {
        return None;
    }
    let table = code(read_word(dispatch, 20)?.checked_sub(image_base)?)?;
    code(read_word(table, selector as usize * 4)?.checked_sub(image_base)?)
}

fn wrapper_command(body: &[u8]) -> Option<(u32, usize)> {
    if !body.starts_with(b"\x51\x56\x8b\x74\x24\x14\x8b\x0e\x85\xc9") {
        return None;
    }
    let (command, lookup) = if at(body, 0x0a, b"\x74\x4c\x8d\x44\x24\x04\x50\xff\x76\x04\xe8") {
        if !at(body, 0x19, b"\x85\xc0\x74\x3b\x8b\x4c\x24\x10\x57\x8d\x7c\x24\x18\x57\x8b\x11\x6a\0\x50\x6a\0\x68")
            || !at(body, 0x33, b"\xff\x76\x08\xff\x71\x04\xff\x52\x10\x83\x7c\x24\x18\0\x5f\x74\x14\x8b\x44\x24\x18\x5e\xc7\0\x01\0\0\0\xb8\x01\0\0\0\x59\xc2\x10\0\x8b\x44\x24\x18\x5e\xc7\0\0\0\0\0\xb8\x01\0\0\0\x59\xc2\x10\0") {
            return None;
        }
        (0x2f, 0x15)
    } else {
        if !at(body, 0x0a, b"\x75\x14\x8b\x44\x24\x18\xc7\0\0\0\0\0\xb8\x01\0\0\0\x5e\x59\xc2\x10\0") {
            return None;
        }
        if at(body, 0x20, b"\x8d\x44\x24\x04\x50\xff\x76\x04\xe8") {
            if !at(body, 0x2d, b"\x8b\xd0\x85\xd2\x74\xd9\x8b\x4c\x24\x10\x57\x8d\x7c\x24\x18\x57\x8b\x01\x6a\0\x52\x6a\0\x68")
                || !at(body, 0x49, b"\xff\x76\x08\xff\x71\x04\xff\x50\x10\x83\x7c\x24\x18\0\x8b\x44\x24\x1c\x5f\x74\xb2\xc7\0\x01\0\0\0\xb8\x01\0\0\0\x5e\x59\xc2\x10\0") {
                return None;
            }
            (0x45, 0x29)
        } else {
            if !at(body, 0x20, b"\x8b\x46\x04\x8d\x54\x24\x04\x52\x50\xe8")
                || !at(body, 0x2e, b"\x85\xc0\x75\x10\x8b\x4c\x24\x18\x89\x01\xb8\x01\0\0\0\x5e\x59\xc2\x10\0\x8b\x4c\x24\x10\x8b\x11\x8b\x52")
                || !matches!(body.get(0x4a), Some(0x0c | 0x10))
                || !at(body, 0x4b, b"\x57\x8d\x7c\x24\x18\x57\x6a\0\x50\x8b\x46\x08\x6a\0\x68")
                || !at(body, 0x5e, b"\x50\x8b\x41\x04\x50\xff\xd2\x83\x7c\x24\x18\0\x5f\x74\x9f\x8b\x4c\x24\x18\xc7\x01\x01\0\0\0\xb8\x01\0\0\0\x5e\x59\xc2\x10\0") {
                return None;
            }
            (0x5a, 0x2a)
        }
    };
    let command = read_word(body, command)?;
    (command >= 256 && command < 512).then_some((command, lookup))
}

fn top_default<'a>(rva: u32, command: u32, code: &impl Fn(u32) -> Option<&'a [u8]>) -> Option<()> {
    let body = code(rva)?;
    if at(body, 0, b"\x8b\x44\x24\x08\x48\x83\xf8") {
        return (command.checked_sub(1)? > u32::from(*body.get(7)?)
            && at(body, 8, b"\x0f\x87")
            && code(jump(body, rva, 10)?)?.starts_with(b"\x33\xc0\xc2\x18\0")).then_some(());
    }
    let (compare, unwind, tail_offset, tail): (_, &[u8], _, &[u8]) = [
        (b"\x8b\x84\x24\x58\x02\0\0\x8b\x9c\x24\x68\x02\0\0\x48\x8b\xbc\x24\x60\x02\0\0\x89\x5c\x24\x20\x83\xf8".as_slice(), b"\x44\x02\0\0".as_slice(), 35, b"\x81\xc4\x3c\x02\0\0\xc2\x18\0".as_slice()),
        (b"\x8b\x84\x24\x6c\x02\0\0\x8b\xac\x24\x7c\x02\0\0\x8b\x9c\x24\x74\x02\0\0\x48\x8b\xf1\x89\x6c\x24\x1c\x83\xf8", b"\x58\x02\0\0", 35, b"\x81\xc4\x50\x02\0\0\xc2\x18\0"),
        (b"\x8b\x7d\x14\x8b\x45\x1c\x8b\xd9\x8b\x4d\x0c\x49\x89\x7c\x24\x1c\x89\x44\x24\x20\x83\xf9", b"\xb0\x02\0\0", 34, b"\x8b\xe5\x5d\xc2\x18\0"),
    ].into_iter().find_map(|(prefix, unwind, tail_offset, tail)| Some((after(body, prefix)?, unwind, tail_offset, tail)))?;
    if command.checked_sub(1)? <= u32::from(*body.get(compare)?) || !at(body, compare + 1, b"\x0f\x87") { return None; }
    let leaf = code(jump(body, rva, compare + 3)?)?;
    (at(leaf, 0, b"\x33\xc0\x8b\x8c\x24") && at(leaf, 5, unwind)
        && at(leaf, 9, b"\x64\x89\x0d\0\0\0\0\x59\x5f\x5e")
        && at(leaf, tail_offset, tail)).then_some(())
}

fn script_default<'a>(rva: u32, command: u32, code: &impl Fn(u32) -> Option<&'a [u8]>) -> Option<()> {
    let body = code(rva)?;
    let compare = [
        b"\x8b\x84\x24\x88\0\0\0\x8b\xb4\x24\x8c\0\0\0\x8b\xac\x24\x98\0\0\0\x8b\x9c\x24\x90\0\0\0\x89\x74\x24\x24\x89\x6c\x24\x18\x3d".as_slice(),
        b"\x8b\x45\x0c\x8b\x4d\x10\x8b\x75\x1c\x8b\x5d\x14\x89\x4d\xa4\x89\x75\x9c\x3d",
        b"\x64\xa3\0\0\0\0\x8b\xf1\x8b\x45\x0c\x3d",
        b"\x64\xa3\0\0\0\0\x8b\xd9\x8b\x45\x0c\x3d",
    ].into_iter().find_map(|prefix| after(body, prefix))?;
    if command <= read_word(body, compare)? || !at(body, compare + 4, b"\x0f\x8f") { return None; }
    let high_rva = jump(body, rva, compare + 6)?;
    let high = code(high_rva)?;
    if high.first() != Some(&0x3d) || read_word(high, 1)? == command || !at(high, 5, b"\x0f\x85") { return None; }
    let leaf_rva = jump(high, high_rva, 7)?;
    let leaf = code(leaf_rva)?.strip_prefix(b"\xb8\x01\0\0\0")?;
    if leaf.first() == Some(&0xe9) {
        let tail = code(jump(code(leaf_rva)?, leaf_rva, 6)?)?;
        return ((tail.starts_with(b"\x8b\x4c\x24\x74\x64\x89\x0d\0\0\0\0\x59\x5f\x5e\x5d\x5b\x8b\x4c\x24\x5c\x33\xcc\xe8")
            && at(tail, 27, b"\x83\xc4\x6c\xc2\x18\0"))
            || (tail.starts_with(b"\x8b\x4d\xf4\x64\x89\x0d\0\0\0\0\x59\x5f\x5e\x5b\x8b\x4d\xf0\x33\xcd\xe8")
            && at(tail, 24, b"\x8b\xe5\x5d\xc2\x18\0"))).then_some(());
    }
    (leaf.starts_with(b"\x8b\x4c\x24") && matches!(leaf.get(3), Some(0x68 | 0x70))
        && at(leaf, 4, b"\x64\x89\x0d\0\0\0\0\x59\x5f\x5e\x5b\x8b\xe5\x5d\xc2\x18\0")).then_some(())
}

fn base_default<'a>(rva: u32, command: u32, code: &impl Fn(u32) -> Option<&'a [u8]>) -> Option<()> {
    let body = code(rva)?;
    let call = [
        b"\x56\x8b\xf1\x83\x7e\x54\0\x74\x06\x33\xc0\x5e\xc2\x08\0\x8b\x06\x57\x8b\x7c\x24\x0c\xff\x74\x24\x10\xff\x77\x14\xff\x77\x10\xff\x77\x0c\xff\x77\x08\xff\x77\x04\xff\x50\x1c\x85\xc0\x74\x11\xff\x74\x24\x10\x8b\xce\x57\xe8".as_slice(),
        b"\x56\x8b\xf1\x57\x83\x7e\x54\0\x75\x31\x8b\x7c\x24\x0c\xff\x74\x24\x10\x8b\x06\xff\x77\x14\xff\x77\x10\xff\x77\x0c\xff\x77\x08\xff\x77\x04\xff\x50\x1c\x85\xc0\x74\x11\xff\x74\x24\x10\x8b\xce\x57\xe8",
        b"\x57\x8b\xf9\x83\x7f\x54\0\x74\x06\x33\xc0\x5f\xc2\x08\0\x8b\x07\x53\x8b\x5c\x24\x10\x56\x8b\x74\x24\x10\x8b\x4e\x14\x8b\x56\x10\x53\x51\x8b\x4e\x0c\x52\x8b\x56\x08\x51\x8b\x4e\x04\x52\x8b\x50\x1c\x51\x8b\xcf\xff\xd2\x85\xc0\x74\x0f\x53\x56\x8b\xcf\xe8",
        b"\x57\x8b\xf9\x83\x7f\x54\0\x74\x06\x33\xc0\x5f\xc2\x08\0\x8b\x07\x53\x8b\x5c\x24\x10\x56\x8b\x74\x24\x10\x8b\x4e\x14\x8b\x56\x10\x53\x51\x8b\x4e\x0c\x52\x8b\x56\x08\x51\x8b\x4e\x04\x52\x8b\x50\x1c\x51\x8b\xcf\xff\xd2\x85\xc0\x74\x0e\x53\x56\x57\xe8",
    ].into_iter().find_map(|prefix| after(body, prefix))?;
    let generic_rva = jump(body, rva, call)?;
    let generic = code(generic_rva)?;
    let (compare, leaf) = [
        (b"\x83\xec\x18\x53\x55\x56\x57\x8b\x7c\x24\x2c\x8b\xf1\x8b\x47\x08\x83\xc0\xfe\x83\xf8".as_slice(), b"\x5f\x5e\x5d\xb8\x01\0\0\0\x5b\x83\xc4\x18\xc2\x08\0".as_slice()),
        (b"\x83\xec\x10\x53\x55\x56\x57\x8b\x7c\x24\x24\x8b\xf1\x8b\x47\x08\x83\xc0\xfe\x83\xf8", b"\x5f\x5e\x5d\xb8\x01\0\0\0\x5b\x83\xc4\x10\xc2\x08\0"),
        (b"\x83\xec\x28\x53\x55\x8b\x6c\x24\x34\x8b\x45\x08\x56\x83\xc0\xfe\x57\x8b\xf1\x83\xf8", b"\x5f\x5e\x5d\xb8\x01\0\0\0\x5b\x83\xc4\x28\xc2\x08\0"),
        (b"\x55\x8b\xec\x83\xe4\xf8\x83\xec\x34\x53\x56\x8b\x75\x0c\x8b\x46\x08\x83\xc0\xfe\x57\x83\xf8", b"\x5f\x5e\xb8\x01\0\0\0\x5b\x8b\xe5\x5d\xc2\x0c\0"),
    ].into_iter().find_map(|(prefix, leaf)| generic.starts_with(prefix).then_some((prefix.len(), leaf)))?;
    (command.checked_sub(2)? > u32::from(*generic.get(compare)?)
        && at(generic, compare + 1, b"\x0f\x87")
        && code(jump(generic, generic_rva, compare + 3)?)?.starts_with(leaf)).then_some(())
}

fn after(body: &[u8], prefix: &[u8]) -> Option<usize> {
    body.get(..128.min(body.len()))?.windows(prefix.len()).position(|bytes| bytes == prefix)
        .map(|offset| offset + prefix.len())
}

fn jump(body: &[u8], rva: u32, operand: usize) -> Option<u32> {
    relative_target(rva, operand, read_word(body, operand)?)
}

fn at(body: &[u8], offset: usize, pattern: &[u8]) -> bool {
    body.get(offset..).is_some_and(|bytes| bytes.starts_with(pattern))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_table::OrdinaryNativeTable;
    use crate::pe_resources::parse_sections;
    use std::{fs, io, path::Path};

    #[test]
    #[ignore = "requires the installed game executables"]
    fn native_css_restore_uses_registered_wrappers_and_receivers() -> io::Result<()> {
        let games = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../games");
        let samples = [
            ("cs2_full_v401/system/cs2_open.exe", 426, true),
            ("[170526][NEKO WORKs] ネコぱら vol.3 steam版 (files)/nekopara_vol3/NEKOPARAvol3.exe", 431, true),
            ("ISLAND/ISLAND.exe", 421, true),
            ("ISLAND/ISLAND_CHS.exe", 421, true),
            ("Yukikoi Melt/YukikoiMelt.exe", 437, true),
            ("ここから夏のイノセンス！/cs2.exe", 423, true),
            ("ここから夏のイノセンス！/cs2_cn.exe", 423, true),
            ("未来ラジオと人工鳩（AUGUST）/cs2.exe", 431, true),
            ("未来ラジオと人工鳩（AUGUST）/cs2_cn.exe", 431, true),
            ("果つることなき未来より/Hatsumira.exe", 421, true),
            ("灰色的乐园v1/Grisaia3.exe", 407, true),
            ("灰色的乐园v1/Grisaia3_cn.exe", 407, true),
            ("灰色的迷宫/Grisaia2.exe", 400, true),
            ("灰色的迷宫/Grisaia2_cn.exe", 400, true),
            ("牛顿与苹果树/cs2.exe", 428, true),
            ("牛顿与苹果树/牛顿与苹果树.exe", 428, true),
            ("白昼夢の青写真/cs2.exe", 437, true),
            ("カミカゼ☆エクスプローラー!/cs2.exe", 393, true),
            ("戦国天使ジブリール/Djibril5.exe", 395, true),
            ("灰色的果实/Grisaia.exe", 389, true),
            ("ツナバン らぶみくす/cs2.exe", 341, false),
            ("ツナバン らぶみくす/cs2_gemini2.5pro.exe", 341, false),
            ("祝福のカンパネラ/cs2.exe", 366, false),
            ("うたてめぐり/UTATEMEGURI.exe", 372, true),
            ("秘密游戏 反叛/REBELLIONS.exe", 412, true),
            ("Happiness！2 樱花盛典/cs2_crack.exe", 437, true),
        ];
        for (relative, code, common) in samples {
            let mut image = fs::read(games.join(relative))?;
            let expected = Some(CssRestoreCommand {
                code,
                common_receivers_preserve_output: common,
                view_rejects_restore: matches!(code, 366 | 372),
            });
            for overlay in [b"".as_slice(), b"unrelated PE overlay"] {
                image.extend_from_slice(overlay);
                let table = OrdinaryNativeTable::from_pe(&image)
                    .unwrap_or_else(|| panic!("missing ordinary table for {relative}"));
                assert_eq!(table.css_restore_command(), expected, "{relative}");
                if let Some(command) = match code {
                    341 => Some(60),
                    366 => Some(61),
                    _ => None,
                } {
                    assert_eq!(table.legacy_view_lifecycle(), Some(command), "{relative}");
                }
            }
        }
        Ok(())
    }

    #[test]
    #[ignore = "requires the installed legacy 3D View engine samples"]
    fn kc_view_binding_follows_the_registered_load_and_scale_wrappers() -> io::Result<()> {
        let games = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../games");
        let mut image = fs::read(games.join("祝福のカンパネラ/cs2.exe"))?;
        let supports = |image: &[u8]| OrdinaryNativeTable::from_pe(image).unwrap().kc_view_binding();
        assert!(supports(&image));
        image.extend_from_slice(b"unrelated PE overlay");
        assert!(supports(&image));
        let sections = parse_sections(&image).unwrap();
        let operand = sections.iter().find_map(|section| section.file_bytes_from(&image, 0x492cf)).unwrap();
        let offset = operand.as_ptr() as usize - image.as_ptr() as usize;
        image[offset] ^= 1;
        assert!(!supports(&image));
        assert!(!supports(&fs::read(games.join("うたてめぐり/UTATEMEGURI.exe"))?));
        assert!(!supports(&fs::read(games.join("ツナバン らぶみくす/cs2.exe"))?));
        Ok(())
    }

    #[test]
    #[ignore = "requires the installed V401 engine"]
    fn native_css_restore_requires_lookup_and_receiver_evidence() -> io::Result<()> {
        let original = fs::read(Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../games/cs2_full_v401/system/cs2_open.exe"))?;
        let sections = parse_sections(&original).unwrap();
        let offset = |rva| sections.iter().find_map(|section| section.file_bytes_from(&original, rva))
            .unwrap().as_ptr() as usize - original.as_ptr() as usize;
        let inspect = |image: &[u8]| OrdinaryNativeTable::from_pe(image).unwrap().css_restore_command();
        let expected = CssRestoreCommand { code: 426, common_receivers_preserve_output: true, view_rejects_restore: false };
        assert_eq!(inspect(&original), Some(expected));
        for rva in [0x115a70 + 0x31, 0x115a70 + 0x54, 0x224260 + 0x23] {
            let mut changed = original.clone();
            changed[offset(rva)] ^= 1;
            assert_eq!(inspect(&changed), None, "wrapper/lookup changed at {rva:#x}");
        }
        for rva in [0x0baf36, 0x0c5172, 0x0b95bc] {
            let mut changed = original.clone();
            changed[offset(rva)] ^= 1;
            assert_eq!(inspect(&changed), Some(CssRestoreCommand {
                common_receivers_preserve_output: false, ..expected
            }), "receiver changed at {rva:#x}");
        }
        Ok(())
    }
}

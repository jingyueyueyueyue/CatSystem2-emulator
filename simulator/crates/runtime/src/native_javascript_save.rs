//! Recognize the five registered JavaScript CSS and save-notification callbacks.

use super::root_clock::{primary_task_vtable, read_word, relative_target};
use crate::pe_resources::Section;

const REGISTRATION: &[u8] = b"\x6a\0\x6a\0\x6a\0\x68????\xff\x73\x68\x8d\x44\x24\x38\x50\xe8????\x6a\xff\x6a\0\x68????\
\x8b\x30\x8d\x44\x24\x44\xff\x73\x68\x50\xe8????\x83\xc4\x2c\x8b\0\x6a\0\x51\x8b\xcc\x50\x89\x31\x8b\xcf\xe8????";
const NAMES: [&[u8]; 5] = [
    b"cfIsExistSave\0",
    b"cfSaveData\0",
    b"cfUpdateSaveData\0",
    b"cfLoadSaveData\0",
    b"cfLoadSaveDataOnlyFlag\0",
];

const OWNER_GETTER: &[u8] = b"\x8b\x44\x24\x04\x8b\0\x8b\x40\x04\x8b\x40\x04\xc3";
const PARENT_GETTER: &[u8] = b"\x8b\x41\x0c\xc3";
const TASK_HANDLE_GETTER: &[u8] = b"\x8b\x41\x04\xc3";
const REGISTER_PARENT: &[u8] = b"\x8b\x44\x24\x04\x89\x41\x0c\xc2\x04\0";
const NOTIFY: &[u8] = b"\x8b\x54\x24\x04\x83\xfa\xff\x75\x03\x8b\x51\x04\xff\x74\x24\x14\x8b\x01\x6a\0\xff\x74\x24\x18\xff\x74\x24\x18\
\xff\x74\x24\x18\x52\xff\x71\x04\xff\x50\x10\xc2\x14\0";
const TAG_GETTER: &[u8] = b"\x8b\x01\x85\xc0\x74\x04\x8b\x40\x0c\xc3\x33\xc0\xc3";

pub(super) fn callbacks<'a>(
    data: &'a [u8],
    sections: &[Section],
    image_base: u32,
    file: impl Fn(u32, bool) -> Option<&'a [u8]>,
) -> Option<()> {
    let code = |rva| file(rva, true);
    let mut registrations = None;
    for section in sections.iter().filter(|section| section.executable()) {
        let Some(bytes) = code(section.virtual_address) else { continue };
        for (offset, window) in bytes.windows(REGISTRATION.len() * NAMES.len()).enumerate() {
            if !window.starts_with(&REGISTRATION[..7]) {
                continue;
            }
            let rva = section.virtual_address.checked_add(u32::try_from(offset).ok()?)?;
            let Some(entries) = registered_callbacks(rva, window, image_base, &file) else {
                continue;
            };
            if registrations.replace(entries).is_some() {
                return None;
            }
        }
    }
    let [query, save, update, load, flags] = registrations?;
    let query_code = code(query)?;
    let save_code = code(save)?;
    let update_code = code(update)?;
    let load_code = code(load)?;
    let flags_code = code(flags)?;

    let owner = call_target(save, save_code, 0x1f)?;
    if !code(owner)?.starts_with(OWNER_GETTER)
        || [(query, query_code, 0x43), (update, update_code, 0x14),
            (load, load_code, 0x15), (flags, flags_code, 0x15)]
            .iter().any(|&(rva, body, offset)| call_target(rva, body, offset) != Some(owner))
    {
        return None;
    }
    css_callbacks((query, query_code), (save, save_code), image_base, &file)?;

    let [_, register_parent, ..] = primary_task_vtable(data, sections, image_base,
        b".?AVkcTaskJavaScript@@\0", &code)?;
    if !code(register_parent)?.starts_with(REGISTER_PARENT) {
        return None;
    }

    let receivers = load_notification(load, load_code, b"\x09\x04\0\0")?;
    if receivers != load_notification(flags, flags_code, b"\x01\x10\0\0")?
        || receivers != update_notification(update, update_code)?
    {
        return None;
    }
    for (rva, body) in receivers.into_iter().zip([PARENT_GETTER, TASK_HANDLE_GETTER, NOTIFY]) {
        if !code(rva)?.starts_with(body) {
            return None;
        }
    }
    Some(())
}

fn registered_callbacks<'a>(
    rva: u32,
    window: &[u8],
    image_base: u32,
    file: &impl Fn(u32, bool) -> Option<&'a [u8]>,
) -> Option<[u32; 5]> {
    let mut callbacks = [0; 5];
    let registrar = |rva, body| {
        Some([call_target(rva, body, 0x13)?, call_target(rva, body, 0x2b)?,
            call_target(rva, body, 0x3f)?])
    };
    let targets = registrar(rva, window)?;
    for (index, name) in NAMES.into_iter().enumerate() {
        let offset = index * REGISTRATION.len();
        let body = window.get(offset..offset + REGISTRATION.len())?;
        let rva = rva.checked_add(u32::try_from(offset).ok()?)?;
        if !matches_code(body, REGISTRATION)
            || registrar(rva, body) != Some(targets)
            || !file(read_word(body, 0x1d)?.checked_sub(image_base)?, false)?.starts_with(name)
        {
            return None;
        }
        callbacks[index] = read_word(body, 7)?.checked_sub(image_base)?;
        file(callbacks[index], true)?;
    }
    for target in targets {
        file(target, true)?;
    }
    Some(callbacks)
}

fn css_callbacks<'a>(
    (query, query_code): (u32, &[u8]),
    (save, save_code): (u32, &[u8]),
    image_base: u32,
    file: &impl Fn(u32, bool) -> Option<&'a [u8]>,
) -> Option<()> {
    // Both callbacks format the same slot path. The writer receives the bound
    // CSS and the keep-new-mtime flag. The query constructs its own CSS, loads
    // it through the private directory, and uses unsigned tag comparisons.
    let path_template = read_word(save_code, 0x58)?;
    let formatter = call_target(save, save_code, 0x5d)?;
    if !file(path_template.checked_sub(image_base)?, false)?.starts_with(b"save%04d.dat\0")
        || path_template != read_word(query_code, 0x79)?
        || formatter != call_target(query, query_code, 0x7e)?
        || !matches_code(save_code.get(0x24..)?, b"\x8b\x4e\x08\x83\xc4\x04\x32\xdb\x8b\xf8\x83\xf9\x01\x7c\x58\x83\xbf\x60\x02\0\0\0\x74\x4f")
        || !matches_code(save_code.get(0x62..)?, b"\x8b\x8f\x60\x02\0\0\x8d\x44\x24\x1c\x83\xc4\x0c\x6a\x01\x50\x8d\x87\x5c\x01\0\0\x50\xe8????\x85\xc0")
        || !matches_code(query_code.get(0xd9..)?, b"\x6a\0\x8d\x4c\x24\x18\xe8????\x8d\x44\x24\x1c\xc7\x84\x24\x28\x0a\0\0\0\0\0\0\
\x50\x8b\x44\x24\x1c\x8d\x4c\x24\x18\x05\x5c\x01\0\0\x50\xe8????\x85\xc0\x74\x1e\xbd\x01\0\0\0\x83\xfb\xff\x74\x14\x8d\x4c\x24\x14\xe8????\
\x3b\xc3\x72\x04\x3b\xc7\x76\x03\x83\xcd\xff")
    {
        return None;
    }
    file(formatter, true)?;
    let writer = file(call_target(save, save_code, 0x79)?, true)?;
    let reader = file(call_target(query, query_code, 0x102)?, true)?;
    let tag = file(call_target(query, query_code, 0x119)?, true)?;
    (writer.get(0x80..0x86)? == b"\xc7\x07CSS\0"
        && reader.get(0x129..0x12f)? == b"\x81\x3bCSS\0"
        && tag.starts_with(TAG_GETTER)).then_some(())
}

fn load_notification(rva: u32, body: &[u8], command: &[u8; 4]) -> Option<[u32; 3]> {
    let packet = body.get(0x1a..)?;
    if !matches_code(packet, b"\x8b\x56\x08\x83\xc4\x04\x8b\xf8\x83\xfa\x01\x7c\x58\x8b\x4f\x04\x89\x4c\x24\x0c\xc7\x44\x24\x10????\
\xc7\x44\x24\x14\0\0\0\0\x85\xd2\x7f\x0a\x8b\x0e\x8b\x49\x04\x83\xc1\x28\xeb\x03\x8b\x4e\x04\xe8????\x89\x44\x24\x18\x8b\xcf\
\x8d\x44\x24\x08\x50\x8d\x44\x24\x10\x50\x6a\0\x68\xaa\0\0\0\xe8????\x8b\xc8\xe8????\x50\x8b\xcf\xe8????")
        || body.get(0x32..0x36)? != command
    {
        return None;
    }
    Some([call_target(rva, body, 0x6b)?, call_target(rva, body, 0x72)?, call_target(rva, body, 0x7a)?])
}

fn update_notification(rva: u32, body: &[u8]) -> Option<[u32; 3]> {
    if !matches_code(body.get(0x19..)?, b"\x83\xc4\x04\x8b\xf0\x8d\x44\x24\x04\x8b\x4e\x04\x50\x8d\x44\x24\x0c\x89\x4c\x24\x0c\x50\x6a\0\
\x68\xaa\0\0\0\x8b\xce\xc7\x44\x24\x1c\0\x10\0\0\xc7\x44\x24\x20\0\0\0\0\xc7\x44\x24\x24\0\0\0\0\
\xe8????\x8b\xc8\xe8????\x50\x8b\xce\xe8????")
    {
        return None;
    }
    Some([call_target(rva, body, 0x50)?, call_target(rva, body, 0x57)?, call_target(rva, body, 0x5f)?])
}

fn call_target(rva: u32, body: &[u8], opcode: usize) -> Option<u32> {
    (*body.get(opcode)? == 0xe8).then_some(())?;
    let operand = opcode + 1;
    relative_target(rva, operand, read_word(body, operand)?)
}

fn matches_code(body: &[u8], pattern: &[u8]) -> bool {
    body.get(..pattern.len()).is_some_and(|body| {
        body.iter().zip(pattern).all(|(&byte, &expected)| expected == b'?' || byte == expected)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{native_table::OrdinaryNativeTable, pe_resources::parse_sections};
    use std::{fs, io, path::Path};

    #[test]
    fn javascript_save_uses_registered_css_and_notification_callbacks() -> io::Result<()> {
        let source = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../games/cs2_full_v401/system/cs2_open.exe");
        if !source.is_file() {
            return Ok(());
        }
        let mut pe = fs::read(source)?;
        let recognizes = |pe: &[u8]| {
            OrdinaryNativeTable::from_pe(pe).expect("ordinary native table")
                .javascript_save_callbacks_registered()
        };
        assert!(recognizes(&pe));
        pe.extend_from_slice(b"unrelated overlay");
        assert!(recognizes(&pe));

        let sections = parse_sections(&pe).unwrap();
        let offset_at = |rva| {
            let bytes = sections.iter().find_map(|section| section.file_bytes_from(&pe, rva)).unwrap();
            bytes.as_ptr() as usize - pe.as_ptr() as usize
        };
        for rva in [0x1111d4, 0x111202, 0x13596f + 1, 0x13552d, 0x135550,
            0x1355f2, 0x50c572, 0x4a7df6, 0x2243e2, 0x22473b] {
            let mut changed = pe.clone();
            changed[offset_at(rva)] ^= 1;
            assert!(!recognizes(&changed), "modified RVA {rva:#x}");
        }
        Ok(())
    }
}

//! The format-4 blend used by the registered nine-piece HG constructor.
use super::root_clock::{read_word, relative_target};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HgWindowBlend {
    SkipTransparent,
    SeedAdjacentRgb,
}

pub(super) fn registered<'a>(
    (wrapper_rva, wrapper): (u32, &[u8]),
    image_base: u32,
    file_bytes: impl Fn(u32, bool) -> Option<&'a [u8]>,
) -> Option<HgWindowBlend> {
    let (constructor_call, legacy) = if at(wrapper, 0, b"\x55\x8b\xec\x83\xe4\xf8\x83\xec\x0c\x53\x56\x57\x68\x14\x01\0\0\xe8")
        && at(wrapper, 0x33, b"\x8b\x45\x10\x8b\x48\x08\x8b\x50\x04\x8b\0\x51\x52\x50\x8b\xc3\x89\x5c\x24\x1c")
        && at(wrapper, 0x4c, b"\x8b\x4d\x0c\x8b\x81\x88\0\0\0\x8b\x70\x14")
        && at(wrapper, 0x76, b"\x8b\x55\x14\x89\x7e\x04\x8b\x4f\x04\x89\x39\x5f\x5e\x89\x1a\xb8\x01\0\0\0\x5b\x8b\xe5\x5d\xc2\x10\0")
    {
        (0x47, true)
    } else if at(wrapper, 0x26, b"\x68\x18\x02\0\0\xe8")
        && at(wrapper, 0x4e, b"\x8b\x45\x10\x6a\x04\xc7\x45\xfc\xff\xff\xff\xff\x89\x4d\xf0\xff\x70\x08\xff\x70\x04\xff\x30")
        && at(wrapper, 0x6a, b"\x8b\x45\x0c\x8b\xb0\xdc\0\0\0\x8d\x45\xf0\x50")
        && at(wrapper, 0x96, b"\x8b\x4d\x14\x40\x89\x46\x04\x8b\x45\xf0\x89\x57\x04\x89\x13\x89\x01\xb8\x01\0\0\0")
        && at(wrapper, 0xbd, b"\xc2\x10\0")
    {
        (0x65, false)
    } else {
        return None;
    };

    let constructor_rva = call_target(wrapper_rva, wrapper, constructor_call)?;
    let constructor = file_bytes(constructor_rva, true)?;
    // Verify the format-4 allocation and all nine tile blend calls.
    let (blit_calls, table_offsets): (&[usize], _) = if legacy {
        if !at(constructor, 0xbf, b"\x6a\x04\x56\x51\x89\x03\xe8")
            || !at(constructor, 0x113, b"\xc7\x44\x24\x64\xff\0\0\0\x8b\x17\x8b\x42\x04\x55\x53\x8b\xcf\xff\xd0")
            || !at(constructor, 0x67d, b"\xc2\x0c\0")
        {
            return None;
        }
        (&[0x177, 0x1e2, 0x248, 0x2b5, 0x34a, 0x3fe, 0x49a, 0x54e, 0x637], [0x35, 0x3e])
    } else {
        if !at(constructor, 0xf7, b"\xff\x75\x1c\x8b\x45\0\x57\x53\xc7\x45\xfc\xff\xff\xff\xff\x89\x08\xe8")
            || !at(constructor, 0x16f, b"\xc7\x45\xc0\xff\0\0\0\x8b\x06\x6a\0\xff\x75\x10\xff\x50\x04")
            || !at(constructor, 0x810, b"\xc2\x10\0")
        {
            return None;
        }
        (&[0x1fb, 0x28a, 0x319, 0x3a8, 0x45d, 0x52e, 0x5f7, 0x6bb, 0x7b6], [0x77, 0x83])
    };
    let blit_rva = call_target(constructor_rva, constructor, blit_calls[0])?;
    if !blit_calls[1..].iter().all(|&offset| call_target(constructor_rva, constructor, offset) == Some(blit_rva)) {
        return None;
    }
    let blit = file_bytes(blit_rva, true)?;
    if legacy {
        if !at(blit, 0x15, b"\x83\x7e\x70\0\x8b\x4e\x28\x8b\x16\x8d\x04\x8a\x75\x18\x83\x7e\x5c\0\x74\x09\x8b\x3c\x85")
            || !at(blit, 0x30, b"\xeb\x10\x8b\x3c\x85")
            || !at(blit, 0x39, b"\xeb\x07\x8b\x3c\x85")
        {
            return None;
        }
    } else if !at(blit, 0x57, b"\x83\x7e\x70\0\x8b\x56\x28\x8b\x0e\x75\x1b\x83\x7e\x5c\0\x8d\x3c\x91\x74\x09\x8b\x3c\xbd")
        || !at(blit, 0x72, b"\xeb\x13\x8b\x3c\xbd")
        || !at(blit, 0x7b, b"\xeb\x0a\x8d\x04\x91\x8b\x3c\x85")
    {
        return None;
    }

    // The stock 4-to-4 alpha path is shared by these two selector tables.
    let [format_table, alpha_table] = table_offsets.map(|offset| read_word(blit, offset).and_then(|va| va.checked_sub(image_base)));
    let cell = |table: u32, index: u32| read_word(file_bytes(table.checked_add(index * 4)?, false)?, 0);
    let kernel_va = cell(format_table?, 4 + 4 * 4)?;
    if cell(alpha_table?, 4)? != kernel_va {
        return None;
    }
    let kernel = file_bytes(kernel_va.checked_sub(image_base)?, true)?;
    if legacy && at(kernel, 0xe0, b"\x8b\x4c\x1e\xff\x0f\xb6\x54\x1e\x02\x89\x4c\x24\x10\x8a\x4c\x14\x38\x88\x4c\x24\x13\x84\xc9\x0f\x84\x24\x01\0\0")
        && at(kernel, 0x221, b"\x8b\x4c\x24\x24\x41\x83\xc6\x04")
    {
        Some(HgWindowBlend::SkipTransparent)
    } else if !legacy && at(kernel, 0x213, b"\x83\x7a\x10\0\x8b\xc6\x89\x85\xf4\xfe\xff\xff\x8b\xcb\x7e\x24\x83\x7b\xfc\0\x8b\x06")
        && at(kernel, 0x236, b"\x75\x09\x8b\x85\xf8\xfe\xff\xff\x89\x43\xfc")
        && at(kernel, 0x410, b"\x75\x08\x8b\x85\xf0\xfe\xff\xff\x89\x01")
    {
        Some(HgWindowBlend::SeedAdjacentRgb)
    } else {
        None
    }
}

fn call_target(rva: u32, bytes: &[u8], opcode: usize) -> Option<u32> {
    if bytes.get(opcode) != Some(&0xe8) {
        return None;
    }
    relative_target(rva, opcode + 1, read_word(bytes, opcode + 1)?)
}

fn at(bytes: &[u8], offset: usize, pattern: &[u8]) -> bool {
    bytes.get(offset..offset + pattern.len()) == Some(pattern)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{native_table::OrdinaryNativeTable, pe_resources::parse_sections};
    use std::{fs, io, path::Path};

    #[test]
    #[ignore = "requires both installed HG window engines"]
    fn native_hg_window_uses_registered_format_four_blend() -> io::Result<()> {
        let games = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../games");
        let cases = [
            ("Yukikoi Melt/YukikoiMelt.exe", HgWindowBlend::SeedAdjacentRgb,
                [0x196900 + 0x51, 0x1e14b0 + 0x1fc, 0x62866c, 0x1df410 + 0x23e]),
            ("祝福のカンパネラ/cs2.exe", HgWindowBlend::SkipTransparent,
                [0x59e00 + 0x36, 0xa3800 + 0x178, 0x25174c, 0xb5c50 + 0xf7]),
        ];
        let blend = |image: &[u8]| OrdinaryNativeTable::from_pe(image).unwrap().hg_window_blend();
        for (path, expected, mutations) in cases {
            let image = fs::read(games.join(path))?;
            assert_eq!(blend(&image), Some(expected), "{path}");
            let mut with_overlay = image.clone();
            with_overlay.extend_from_slice(b"nonloaded data after the PE");
            assert_eq!(blend(&with_overlay), Some(expected), "{path} with overlay");
            let sections = parse_sections(&image).unwrap();
            for rva in mutations {
                let bytes = sections.iter().find_map(|section| section.file_bytes_from(&image, rva)).unwrap();
                let offset = bytes.as_ptr() as usize - image.as_ptr() as usize;
                let mut changed = image.clone();
                changed[offset] ^= 1;
                assert_eq!(blend(&changed), None, "{path} at RVA {rva:#x}");
            }
        }
        Ok(())
    }
}

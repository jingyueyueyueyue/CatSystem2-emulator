//! Configuration block ABI from the registered ordinary 359 and 360 wrappers.

/// Recover the default for GAME/flag/max from the configuration field read
/// by native 789. The key lookup and the field initializer must agree.
pub(super) fn shared_slots_default<'a>(
    getter: &[u8],
    image_base: u32,
    read_data: impl Fn(u32) -> Option<&'a [u8]>,
    code: impl Iterator<Item = &'a [u8]> + Clone,
) -> Option<usize> {
    let [0xa1, g0, g1, g2, g3, 0x8b, 0x88, a, b, c, d, 0x8b, 0x54, 0x24, 0x10,
        0x89, 0x0a, 0xb8, 1, 0, 0, 0, 0xc2, 0x10, 0] = *getter.get(..25)?
    else {
        return None;
    };
    let read_word = |bytes: &[u8]| Some(u32::from_le_bytes(bytes.try_into().ok()?));
    let data_at_va = |va: &[u8]| read_data(read_word(va)?.checked_sub(image_base)?);
    let field_offset = u32::from_le_bytes([a, b, c, d]);
    let global = [g0, g1, g2, g3];
    read_word(&global)?.checked_sub(image_base)?;

    // Initialization projects a subobject of the configuration owner into
    // the global pointer the getter reads.
    let owner_bias = unique_value(code.clone().flat_map(|section| {
        section.windows(32).filter_map(move |window| {
            let [0xa1, _, _, _, _, 0x8d, 0x88, x0, x1, x2, x3, 0x05,
                _, _, _, _, 0x68, _, _, _, _, 0x68, _, _, _, _, 0x89, 0x0d,
                g0, g1, g2, g3] = *window else { return None };
            ([g0, g1, g2, g3] == global)
                .then_some(u32::from_le_bytes([x0, x1, x2, x3]))
        })
    }))?;
    let field = field_offset.checked_add(owner_bias)?.to_le_bytes();

    let field_has_flag_key = code.clone().any(|section| {
        section.windows(44).any(|window| {
            let [0x68, _, _, _, _, 0x68, _, _, _, _, 0x68, _, _, _, _,
                0x89, 0x87, _, _, _, _, 0x6a, 3, 0x8b, 0x8f, _, _, _, _,
                0x56, 0xe8, _, _, _, _, 0x83, 0xc4, _, 0x89, 0x87, _, _, _, _] = *window
            else { return false };
            window[25..29] == field && window[40..44] == field
                && data_at_va(&window[1..5]).is_some_and(|key| key.starts_with(b"max\0"))
                && data_at_va(&window[6..10]).is_some_and(|key| key.starts_with(b"flag\0"))
                && data_at_va(&window[11..15]).is_some_and(|key| key.starts_with(b"GAME\0"))
        })
    });
    if !field_has_flag_key { return None; }

    let slots = unique_value(code.flat_map(|section| {
        section.windows(10).filter_map(move |window| {
            (window.starts_with(b"\xc7\x85") && window[2..6] == field)
                .then(|| read_word(&window[6..10])).flatten()
        })
    }))?;
    (1..=1_000_000).contains(&slots).then_some(slots as usize)
}

fn unique_value<T: Copy + Eq>(mut values: impl Iterator<Item = T>) -> Option<T> {
    let first = values.next()?;
    values.all(|value| value == first).then_some(first)
}

struct Wrapper<const N: usize> {
    bytes: &'static [u8],
    lengths: [usize; N],
    offsets: [usize; N],
    copies: [usize; N],
    resolutions: [usize; N],
}

// Each full callback maps consecutive DWORD script addresses, checks the
// settings owner and destinations, and calls the same copy for each block.
// The three compilation forms share this sequence and differ in registers
// and stack layout. '?' marks relocations, native owner offsets and lengths.
const EXPORT: [Wrapper<4>; 3] = [
    Wrapper {
        bytes: b"\x55\x8b\xec\x83\xe4\xf8\x83\xec\x0c\x53\x56\x8b\x75\x10\x8b\x06\x57\x8b\x7d\x08\x50\x57\xe8????\x8b\x4e\x04\x51\x57\
            \x8b\xd8\xe8????\x8b\x56\x08\x52\x57\x89\x44\x24\x18\xe8????\x89\x44\x24\x14\x8b\x46\x0c\x50\x57\xe8?\
            ???\x83\x3d????\x00\x8b\xf0\x74\x7b\x85\xdb\x74\x1b\x8b\x0d????\x68????\x81\xc1?\
            ???\x51\x53\xe8????\x83\xc4\x0c\x8b\x44\x24\x10\x85\xc0\x74\x18\x8b\x15????\x6a?\x81\xc2?\
            ???\x52\x50\xe8????\x83\xc4\x0c\x8b\x44\x24\x14\x85\xc0\x74\x18\x8b\x0d????\x6a?\x81\xc1?\
            ???\x51\x50\xe8????\x83\xc4\x0c\x85\xf6\x74\x18\x8b\x15????\x6a?\x81\xc2????\x52\
            \x56\xe8????\x83\xc4\x0c\x5f\x5e\xb8\x02\x00\x00\x00\x5b\x8b\xe5\x5d\xc2\x10\x00",
        lengths: [0x59, 0x7c, 0x9c, 0xb8],
        offsets: [0x5f, 0x7f, 0x9f, 0xbb],
        copies: [0x65, 0x85, 0xa5, 0xc1],
        resolutions: [0x16, 0x22, 0x30, 0x3e],
    },
    Wrapper {
        bytes: b"\x53\x55\x56\x8b\x74\x24\x10\x8b\xce\x57\x8b\x7c\x24\x1c\xff\x37\xe8????\xff\x77\x04\x8b\xce\x89\x44\x24\x20\xe8?\
            ???\xff\x77\x08\x8b\xce\x8b\xe8\xe8????\xff\x77\x0c\x8b\xce\x8b\xd8\xe8????\x83\x3d???\
            ?\x00\x8b\xf0\x74\x71\x8b\x44\x24\x1c\x85\xc0\x74\x1b\x8b\x0d????\x68????\x81\xc1????\x51\
            \x50\xe8????\x83\xc4\x0c\x85\xed\x74\x16\xa1????\x6a?\x05????\x50\x55\xe8????\
            \x83\xc4\x0c\x85\xdb\x74\x16\xa1????\x6a?\x05????\x50\x53\xe8????\x83\xc4\x0c\x85\xf6\x74\
            \x16\xa1????\x6a?\x05????\x50\x56\xe8????\x83\xc4\x0c\x5f\x5e\x5d\xb8\x02\x00\x00\x00\x5b\
            \xc2\x10\x00",
        lengths: [0x55, 0x73, 0x8d, 0xa7],
        offsets: [0x5b, 0x75, 0x8f, 0xa9],
        copies: [0x61, 0x7b, 0x95, 0xaf],
        resolutions: [0x10, 0x1e, 0x2a, 0x36],
    },
    Wrapper {
        bytes: b"\x53\x55\x56\x8b\x74\x24\x18\x8b\x06\x57\x8b\x7c\x24\x14\x50\x8b\xcf\xe8????\x8b\x4e\x04\x51\x8b\xcf\x8b\xd8\xe8?\
            ???\x8b\x56\x08\x52\x8b\xcf\x8b\xe8\xe8????\x89\x44\x24\x1c\x8b\x46\x0c\x50\x8b\xcf\xe8????\x83\
            \x3d????\x00\x8b\xf0\x74\x77\x85\xdb\x74\x1b\x8b\x0d????\x68????\x81\xc1????\x51\
            \x53\xe8????\x83\xc4\x0c\x85\xed\x74\x18\x8b\x15????\x6a?\x81\xc2????\x52\x55\xe8??\
            ??\x83\xc4\x0c\x8b\x44\x24\x1c\x85\xc0\x74\x18\x8b\x0d????\x6a?\x81\xc1????\x51\x50\xe8??\
            ??\x83\xc4\x0c\x85\xf6\x74\x18\x8b\x15????\x6a?\x81\xc2????\x52\x56\xe8????\x83\xc4\
            \x0c\x5f\x5e\x5d\xb8\x02\x00\x00\x00\x5b\xc2\x10\x00",
        lengths: [0x55, 0x74, 0x94, 0xb0],
        offsets: [0x5b, 0x77, 0x97, 0xb3],
        copies: [0x61, 0x7d, 0x9d, 0xb9],
        resolutions: [0x11, 0x1e, 0x2b, 0x3a],
    },
];

const IMPORT: [Wrapper<3>; 3] = [
    Wrapper {
        bytes: b"\x55\x8b\xec\x83\xe4\xf8\x51\x53\x56\x8b\x75\x10\x8b\x06\x57\x8b\x7d\x08\x50\x57\xe8????\x8b\x4e\x04\x51\x57\x8b\xd8\
            \xe8????\x8b\x56\x08\x52\x57\x89\x44\x24\x14\xe8????\x83\x3d????\x00\x8b\xf0\x74\x59\x85\xdb\
            \x74\x19\xa1????\x68????\x05????\x53\x50\xe8????\x83\xc4\x0c\x8b\x44\x24\x0c\x85\
            \xc0\x74\x18\x8b\x0d????\x6a?\x50\x81\xc1????\x51\xe8????\x83\xc4\x0c\x85\xf6\x74\x18\x8b\
            \x15????\x6a?\x81\xc2????\x56\x52\xe8????\x83\xc4\x0c\x5f\x5e\xb8\x02\x00\x00\x00\x5b\x8b\
            \xe5\x5d\xc2\x10\x00",
        lengths: [0x48, 0x6a, 0x86],
        offsets: [0x4d, 0x6e, 0x89],
        copies: [0x53, 0x73, 0x8f],
        resolutions: [0x14, 0x20, 0x2e],
    },
    Wrapper {
        bytes: b"\x53\x55\x56\x8b\x74\x24\x10\x8b\xce\x57\x8b\x7c\x24\x1c\xff\x37\xe8????\xff\x77\x04\x8b\xce\x8b\xe8\xe8???\
            ?\xff\x77\x08\x8b\xce\x8b\xd8\xe8????\x83\x3d????\x00\x8b\xf0\x74\x53\x85\xed\x74\x1b\x8b\x0d??\
            ??\x68????\x81\xc1????\x55\x51\xe8????\x83\xc4\x0c\x85\xdb\x74\x16\xa1????\
            \x6a?\x05????\x53\x50\xe8????\x83\xc4\x0c\x85\xf6\x74\x16\xa1????\x6a?\x05???\
            ?\x56\x50\xe8????\x83\xc4\x0c\x5f\x5e\x5d\xb8\x02\x00\x00\x00\x5b\xc2\x10\x00",
        lengths: [0x43, 0x61, 0x7b],
        offsets: [0x49, 0x63, 0x7d],
        copies: [0x4f, 0x69, 0x83],
        resolutions: [0x10, 0x1c, 0x28],
    },
    Wrapper {
        bytes: b"\x53\x55\x56\x8b\x74\x24\x18\x8b\x06\x57\x8b\x7c\x24\x14\x50\x8b\xcf\xe8????\x8b\x4e\x04\x51\x8b\xcf\x8b\xd8\xe8?\
            ???\x8b\x56\x08\x52\x8b\xcf\x8b\xe8\xe8????\x83\x3d????\x00\x8b\xf0\x74\x55\x85\xdb\x74\x19\xa1\
            ????\x68????\x05????\x53\x50\xe8????\x83\xc4\x0c\x85\xed\x74\x18\x8b\x0d??\
            ??\x6a?\x81\xc1????\x55\x51\xe8????\x83\xc4\x0c\x85\xf6\x74\x18\x8b\x15????\x6a?\
            \x81\xc2????\x56\x52\xe8????\x83\xc4\x0c\x5f\x5e\x5d\xb8\x02\x00\x00\x00\x5b\xc2\x10\x00",
        lengths: [0x45, 0x63, 0x7f],
        offsets: [0x4a, 0x66, 0x82],
        copies: [0x50, 0x6c, 0x88],
        resolutions: [0x11, 0x1e, 0x2b],
    },
];

#[derive(Debug)]
struct Copies<const N: usize> {
    lengths: [usize; N],
    offsets: [u32; N],
    copy_target: u32,
    resolution_target: u32,
}

fn matching_wrapper<const N: usize>(
    wrappers: &[Wrapper<N>], rva: u32, body: &[u8],
) -> Option<Copies<N>> {
    let wrapper = wrappers.iter().find(|wrapper| {
        body.get(..wrapper.bytes.len()).is_some_and(|prefix| {
            prefix.iter().zip(wrapper.bytes).all(|(&found, &expected)| expected == b'?' || expected == found)
        })
    })?;
    let relative_target = |offset| {
        let displacement = i32::from_le_bytes(body.get(offset + 1..offset + 5)?.try_into().ok()?);
        rva.checked_add(u32::try_from(offset + 5).ok()?)?.checked_add_signed(displacement)
    };
    let shared_target = |offsets: &[usize; N]| {
        let target = relative_target(offsets[0])?;
        offsets.iter().all(|&offset| relative_target(offset) == Some(target)).then_some(target)
    };
    let copy_target = shared_target(&wrapper.copies)?;
    let resolution_target = shared_target(&wrapper.resolutions)?;
    let length = |index| {
        let offset: usize = wrapper.lengths[index];
        if index == 0 {
            u32::from_le_bytes(body[offset..offset + 4].try_into().unwrap()) as usize
        } else {
            body[offset] as usize
        }
    };
    let lengths = std::array::from_fn(length);
    let offsets = wrapper.offsets.map(|offset| {
        u32::from_le_bytes(body[offset..offset + 4].try_into().unwrap())
    });
    if offsets.windows(2).zip(lengths).any(|(pair, length)| {
        pair[0].checked_add(length as u32) != Some(pair[1])
    }) {
        return None;
    }
    Some(Copies { lengths, offsets, copy_target, resolution_target })
}

pub(super) fn configuration_lengths<'a>(
    entries: &[(u32, &'a [u8])],
    file_bytes_at: impl Fn(u32, bool) -> Option<&'a [u8]>,
) -> Option<[usize; 4]> {
    let &(export_rva, export_body) = entries.get(359)?;
    let &(import_rva, import_body) = entries.get(360)?;
    let export = matching_wrapper(&EXPORT, export_rva, export_body)?;
    let import = matching_wrapper(&IMPORT, import_rva, import_body)?;
    let [sound, message, effects, window] = export.lengths;
    if !matches!(sound, 0xb28 | 0x15fc)
        || !(0x20..=0x48).contains(&message) || message % 4 != 0
        || !(0x18..=0x20).contains(&effects) || effects % 4 != 0
        || window != 0x0c
        || export.lengths[..3] != import.lengths
        || export.offsets[..3] != import.offsets
        || export.copy_target != import.copy_target
        || export.resolution_target != import.resolution_target
    {
        return None;
    }
    file_bytes_at(export.copy_target, true)?;
    file_bytes_at(export.resolution_target, true)?;
    Some(export.lengths)
}

#[cfg(test)]
mod tests {
    use crate::native_table::OrdinaryNativeTable;
    use crate::pe_resources::{parse_sections, x86_image_layout};
    use std::path::Path;

    #[test]
    fn native_configuration_resolves_registered_copy_layouts() -> std::io::Result<()> {
        let games = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../games");
        if !games.is_dir() {
            return Ok(());
        }
        let samples = [
            ("\u{795d}\u{798f}\u{306e}\u{30ab}\u{30f3}\u{30d1}\u{30cd}\u{30e9}/cs2.exe", [0xb28, 0x20, 0x1c, 0x0c]),
            ("\u{30c4}\u{30ca}\u{30d0}\u{30f3} \u{3089}\u{3076}\u{307f}\u{304f}\u{3059}/cs2.exe", [0xb28, 0x20, 0x18, 0x0c]),
            ("\u{3046}\u{305f}\u{3066}\u{3081}\u{3050}\u{308a}/UTATEMEGURI.exe", [0xb28, 0x34, 0x1c, 0x0c]),
            ("\u{7070}\u{8272}\u{7684}\u{8ff7}\u{5bab}/Grisaia2.exe", [0xb28, 0x38, 0x1c, 0x0c]),
            ("\u{7070}\u{8272}\u{7684}\u{4e50}\u{56ed}v1/Grisaia3.exe", [0x15fc, 0x38, 0x1c, 0x0c]),
            ("ISLAND/ISLAND.exe", [0x15fc, 0x3c, 0x1c, 0x0c]),
            ("Yukikoi Melt/YukikoiMelt.exe", [0x15fc, 0x48, 0x20, 0x0c]),
            ("\u{7a81}\u{7136}\u{4e4b}\u{95f4}\u{53d1}\u{73b0}\u{6211}\u{5df2}\u{604b}\u{4e0a}\u{4f60}/cs2_cn.exe", [0xb28, 0x38, 0x1c, 0x0c]),
        ];
        for (path, expected) in samples {
            let pe = std::fs::read(games.join(path))?;
            assert_eq!(
                OrdinaryNativeTable::from_pe(&pe).and_then(|table| table.configuration_lengths()),
                Some(expected),
                "{path}"
            );
        }
        Ok(())
    }

    #[test]
    fn native_configuration_requires_pair_agreement() -> std::io::Result<()> {
        let game = Path::new(env!("CARGO_MANIFEST_DIR")).join(concat!(
            "../../../games/",
            "\u{795d}\u{798f}\u{306e}\u{30ab}\u{30f3}\u{30d1}\u{30cd}\u{30e9}/cs2.exe"
        ));
        if !game.is_file() {
            return Ok(());
        }
        let pe = std::fs::read(game)?;
        let (base, _) = x86_image_layout(&pe).unwrap();
        let sections = parse_sections(&pe).unwrap();
        for address in [0x44d2a0 + 0x7c, 0x44d380 + 0x54, 0x44d380 + 0xa2] {
            let at = sections.iter().find_map(|section| {
                section.file_bytes_from(&pe, address - base)
            }).unwrap();
            let file_offset = at.as_ptr() as usize - pe.as_ptr() as usize;
            let mut changed = pe.clone();
            changed[file_offset] ^= 1;
            let table = OrdinaryNativeTable::from_pe(&changed).unwrap();
            assert_eq!(table.configuration_lengths(), None, "{address:#x}");
        }
        Ok(())
    }
}

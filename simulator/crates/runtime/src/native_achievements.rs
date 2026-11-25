//! Recognize the ordinary Steam achievement wrappers and their local-cache receivers.

use super::root_clock::{read_word, relative_target};
use crate::pe_resources::x86_data_directory;

const INITIALIZE: &[u8] = b"\xe8????\xb8\x02\0\0\0\xc2\x10\0";
const UNLOCK: &[u8] = b"\x8b\x44\x24\x0c\x8b\x4c\x24\x04\x68????\xff\x30\xe8????\x8b\x0d????\x85\xc9\x74\x0a\x68????\xe8????\xb8\x02\0\0\0\xc2\x10\0";
const QUERY: &[u8] = b"\x8b\x44\x24\x0c\x8b\x4c\x24\x04\x56\x68????\xff\x30\xe8????\x8b\x74\x24\x14\xc7\x06\0\0\0\0\x8b\x0d????\x85\xc9\x74\x19\x68????\xe8????\x84\xc0\xb8\x01\0\0\0\x74\x0b\x89\x06\x5e\xc2\x10\0\xb8\x01\0\0\0\x5e\xc2\x10\0";
const RESET: &[u8] = b"\x8b\x0d????\x85\xc9\x74\x05\xe8????\xb8\x01\0\0\0\xc2\x10\0";

const SPLIT_UNLOCK_GATE: &[u8] =
    b"\x55\x8b\xec\x83\x39\0\x74\x10\x8b\x89\x04\x20\0\0\x85\xc9\x74\x06\x5d\xe9????\x5d\xc2\x04\0";
const SPLIT_QUERY_GATE: &[u8] =
    b"\x55\x8b\xec\x32\xc0\x83\x39\0\x74\x10\x8b\x89\x04\x20\0\0\x85\xc9\x74\x06\x5d\xe9????\x5d\xc2\x04\0";
const SPLIT_RESET_GATE: &[u8] = b"\x83\x39\0\x74\x0e\x8b\x89\x04\x20\0\0\x85\xc9\x0f\x85????\xc3";
const UNLOCK_CACHE: &[u8] =
    b"\x80\x78\x40\0\x75\x1e\x8d\x50\x10\xc6\x40\x40\x01\x83\x7a\x14\x10\x72\x02\x8b\x12\x8b\x0e\x52\x8b\x01\xff\x50\x1c\x8b\x0e\x8b\x01\xff\x50\x28";
const QUERY_CACHE: &[u8] = b"\x3b\x46\x04\x74\x14\x8a\x40\x40\x5e";

type PeLookup<'a, 'f> = dyn Fn(u32, bool) -> Option<&'a [u8]> + 'f;

pub(super) fn registered_family<'a>(
    entries: &[(u32, &'a [u8])],
    data: &[u8],
    image_base: u32,
    file: &PeLookup<'a, '_>,
) -> Option<()> {
    let &[init, unlock, query, reset] = entries.get(1041..=1044)? else {
        return None;
    };
    if ![
        (init.1, INITIALIZE),
        (unlock.1, UNLOCK),
        (query.1, QUERY),
        (reset.1, RESET),
    ]
    .into_iter()
    .all(|(body, pattern)| matches_at(body, 0, pattern))
    {
        return None;
    }

    let format_buffer = read_word(unlock.1, 9)?;
    let platform = read_word(unlock.1, 22)?;
    let formatter = branch_target(unlock, 16)?;
    if read_word(unlock.1, 31)? != format_buffer
        || read_word(query.1, 10)? != format_buffer
        || read_word(query.1, 42)? != format_buffer
        || read_word(query.1, 33)? != platform
        || read_word(reset.1, 2)? != platform
        || branch_target(query, 17)? != formatter
    {
        return None;
    }
    file(formatter, true)?;
    let receiver = |entry, operand| {
        let rva = branch_target(entry, operand)?;
        Some((rva, file(rva, true)?))
    };
    let init = receiver(init, 1)?;
    let unlock = receiver(unlock, 36)?;
    let query = receiver(query, 47)?;
    let reset = receiver(reset, 11)?;

    let init_iat = if matches_at(unlock.1, 0, SPLIT_UNLOCK_GATE) {
        if !matches_at(query.1, 0, SPLIT_QUERY_GATE) || !matches_at(reset.1, 0, SPLIT_RESET_GATE) {
            return None;
        }
        let unlock = receiver(unlock, 20)?.1;
        let query = receiver(query, 22)?.1;
        let reset = receiver(reset, 15)?.1;
        if !matches_at(unlock, 0x84, UNLOCK_CACHE)
            || !matches_at(query, 0x7b, QUERY_CACHE)
            || !matches_at(reset, 0x10, b"\xc6\x40\x40\0")
            || !matches_at(
                reset,
                0x5c,
                b"\x8b\x0e\x5e\x85\xc9\x74\x07\x8b\x01\x6a\x01\xff\x50\x54\xc3",
            )
            || !matches_at(init.1, 0x24, b"\xa1????\x85\xc0\x75\x51\x68\x08\x20\0\0")
            || !matches_at(init.1, 0x5a, b"\x8b\xce\xe8????\xeb\x02\x33\xc0\xa3????")
            || read_word(init.1, 0x25)? != platform
            || read_word(init.1, 0x66)? != platform
        {
            return None;
        }
        let constructor = receiver(init, 0x5d)?.1;
        if !matches_at(
            constructor,
            0x23,
            b"\xc7\x86\x04\x20\0\0\0\0\0\0\x57\xff\x15",
        ) {
            return None;
        }
        read_word(constructor, 0x36)?
    } else {
        const PLATFORM_GATE: &[u8] = b"\x8b\xb1\x04\x20\0\0\x85\xf6";
        if !matches_at(unlock.1, 0x1d, PLATFORM_GATE)
            || !matches_at(query.1, 0x1f, PLATFORM_GATE)
            || !matches_at(reset.1, 0x06, PLATFORM_GATE)
            || !matches_at(unlock.1, 0xa0, UNLOCK_CACHE)
            || !matches_at(query.1, 0x96, QUERY_CACHE)
            || !matches_at(reset.1, 0x20, b"\xc6\x40\x40\0")
            || !matches_at(
                reset.1,
                0x65,
                b"\x8b\x0e\x85\xc9\x74\x07\x8b\x01\x6a\x01\xff\x50\x54\x5e\xc3",
            )
            || !matches_at(
                init.1,
                0x31,
                b"\xa1????\x85\xc0\x0f\x85\x9f\0\0\0\x68\x08\x20\0\0",
            )
            || !matches_at(
                init.1,
                0x67,
                b"\xc7\x86\x04\x20\0\0\0\0\0\0\x8d\x7e\x04\x68????\x57\xff\x15",
            )
            || read_word(init.1, 0x32)? != platform
        {
            return None;
        }
        read_word(init.1, 0x82)?
    };
    let init_iat = init_iat.checked_sub(image_base)?;
    steam_init_import(data, init_iat, file)
}

fn steam_init_import(data: &[u8], slot: u32, file: &PeLookup<'_, '_>) -> Option<()> {
    // The observed PE32 images use a delay import. Its in-file IAT points to
    // the loader stub, while the corresponding name-table entry names the API.
    let (directory, size) = x86_data_directory(data, 13)?;
    if directory == 0 {
        return None;
    }
    let directory = file(directory, false)?.get(..usize::try_from(size).ok()?)?;
    for descriptor in directory
        .chunks_exact(32)
        .take_while(|row| row.iter().any(|&byte| byte != 0))
    {
        if read_word(descriptor, 0)? != 1
            || !file(read_word(descriptor, 4)?, false)?
                .get(..14)?
                .eq_ignore_ascii_case(b"steam_api.dll\0")
        {
            continue;
        }
        let iat_offset = slot.checked_sub(read_word(descriptor, 12)?)?;
        if iat_offset % 4 != 0 {
            return None;
        }
        let names = file(read_word(descriptor, 16)?, false)?;
        for (offset, name) in names.chunks_exact(4).enumerate() {
            let name_rva = read_word(name, 0)?;
            if name_rva == 0 {
                break;
            }
            if u32::try_from(offset).ok()? == iat_offset / 4 {
                return file(name_rva, false)?
                    .get(2..)?
                    .starts_with(b"SteamAPI_Init\0")
                    .then_some(());
            }
        }
    }
    None
}

fn branch_target((rva, body): (u32, &[u8]), displacement: usize) -> Option<u32> {
    relative_target(rva, displacement, read_word(body, displacement)?)
}

fn matches_at(body: &[u8], offset: usize, pattern: &[u8]) -> bool {
    body.get(offset..offset + pattern.len())
        .is_some_and(|bytes| {
            bytes
                .iter()
                .zip(pattern)
                .all(|(&byte, &expected)| expected == b'?' || byte == expected)
        })
}

#[cfg(test)]
mod tests {
    use crate::{native_table::OrdinaryNativeTable, pe_resources::parse_sections};
    use std::{fs, io, path::Path};

    #[test]
    #[ignore = "requires the four installed original achievement-enabled executables"]
    fn achievement_family_uses_registered_platform_and_cache_receivers() -> io::Result<()> {
        let games = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../games");
        let samples = [
            (
                "[170526][NEKO WORKs] \u{30cd}\u{30b3}\u{3071}\u{3089} vol.3 steam\u{7248} (files)/nekopara_vol3/NEKOPARAvol3.exe",
                [
                    0x62ed80,
                    0x2549b0 + 31,
                    0x1aed20 + 14,
                    0x1af320 + 0xa0,
                    0x1af400 + 0x82,
                    0x1af4b0 + 0x69,
                    0x610b94,
                ],
            ),
            (
                "Yukikoi Melt/YukikoiMelt.exe",
                [
                    0x628130,
                    0x1acc10 + 31,
                    0xd0e00 + 0x1f,
                    0xd0e00 + 0xbc,
                    0xd0ee0 + 0x9d,
                    0xd0fb0 + 0x71,
                    0x622e12,
                ],
            ),
            (
                "\u{672a}\u{6765}\u{30e9}\u{30b8}\u{30aa}\u{3068}\u{4eba}\u{5de5}\u{9ce9}\u{ff08}AUGUST\u{ff09}/cs2.exe",
                [0; 7],
            ),
            (
                "\u{767d}\u{663c}\u{5922}\u{306e}\u{9752}\u{5199}\u{771f}/cs2.exe",
                [0; 7],
            ),
        ];
        let recognized = |pe: &[u8]| {
            OrdinaryNativeTable::from_pe(pe).is_some_and(|table| table.has_achievement_family())
        };
        for (relative, mutations) in samples {
            let mut pe = fs::read(games.join(relative))?;
            assert!(recognized(&pe), "{relative}");
            pe.extend_from_slice(b"unrelated overlay");
            assert!(recognized(&pe), "{relative} with an overlay");

            let sections = parse_sections(&pe).unwrap();
            for rva in mutations.into_iter().filter(|&rva| rva != 0) {
                let bytes = sections
                    .iter()
                    .find_map(|section| section.file_bytes_from(&pe, rva))
                    .unwrap();
                let offset = bytes.as_ptr() as usize - pe.as_ptr() as usize;
                let mut modified = pe.clone();
                modified[offset] ^= 1;
                assert!(
                    !recognized(&modified),
                    "{relative} with modified RVA {rva:#x}"
                );
            }
        }
        let pe = fs::read(games.join("cs2_full_v401/system/cs2_open.exe"))?;
        assert!(
            !recognized(&pe),
            "V401 has no registered Steam achievement family"
        );
        Ok(())
    }
}

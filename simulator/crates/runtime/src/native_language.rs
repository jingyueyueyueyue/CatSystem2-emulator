//! Recognize ordinary slot 1036's main-text and subtitle prefix outputs.

use super::root_clock::{read_word, relative_target};

const WRAPPER: &[u8] = b"\x8b\x4c\x24\x04\x53\x8b\x5c\x24\x10\x56\x57\xff\x33\xe8????\xff\x73\x04\x8b\x4c\x24\x14\x8b\xf8\xe8????\
\x8b\x0d????\x8b\xf0\x57\xe8????\x8b\x0d????\x56\xe8????\x5f\x5e\xb8\x02\0\0\0\x5b\xc2\x10\0";

// Both writers bound their index by the prefix vector, fall back to the
// same default index, and copy the selected prefix to their output buffer.
const PREFIX_WRITER: &[u8] = b"\x55\x8b\xec\x56\x57\x8b\xf9\x68????\xff\x75\x08\x8b\x87\xe4\xe4\0\0\x8b\x70\x04\x2b\x30\xb8\x93\x24\x49\x92\
\xf7\xee\x03\xd6\xc1\xfa\x05\x8b\xf2\xc1\xee\x1f\x03\xf2\xe8????\x8b\x87????\x83\xc4\x08\x3b\xf0\x77\x0a\x8b\x87\xe0\xe4\0\0\x3b\xf0\x76\x29\
\x8d\x0c\xc5\0\0\0\0\x2b\xc8\x8b\x87\xe4\xe4\0\0\x8b\0\x83\x7c\xc8\x14\x08\x8d\x04\xc8\x72\x02\x8b\0\x50\xff\x75\x08\xe8????\x83\xc4\x08\x5f\x5e\x5d\xc2\x04\0";

// The same setup reader binds XML language/id/main and language/id/sub to
// the two fields. The intervening load commits the previous UI selection.
const SETUP_READER: &[u8] = b"\x68????\x68????\x68????\x6a\x03\xff\xb6\0\xfc\0\0\x89\x86\xfc\xfb\0\0\x57\xe8????\x83\xc4\x44\x89\x86\0\xfc\0\0\
\x68????\x68????\x68????\x6a\x03\xff\xb6\x04\xfc\0\0\x57\xe8????\x8b\xbe\xe4\xe4\0\0\x8b\xd8\x89\x9e\x04\xfc\0\0";

pub(super) fn story_prefixes<'a>(
    (rva, wrapper): (u32, &[u8]),
    image_base: u32,
    file_bytes_at: impl Fn(u32, bool) -> Option<&'a [u8]>,
    code_sections: impl Iterator<Item = (u32, &'a [u8])>,
) -> Option<()> {
    if !matches_code(wrapper, WRAPPER) || read_word(wrapper, 0x22)? != read_word(wrapper, 0x30)? {
        return None;
    }
    let resolver = call_target(rva, wrapper, 0x0e)?;
    if resolver != call_target(rva, wrapper, 0x1c)? {
        return None;
    }
    file_bytes_at(resolver, true)?;

    let main = call_target(rva, wrapper, 0x2a)?;
    let subtitle = call_target(rva, wrapper, 0x36)?;
    let main_writer = file_bytes_at(main, true)?;
    let subtitle_writer = file_bytes_at(subtitle, true)?;
    if !matches_code(main_writer, PREFIX_WRITER)
        || !matches_code(subtitle_writer, PREFIX_WRITER)
        || read_word(main_writer, 0x34)? != 0xfc00
        || read_word(subtitle_writer, 0x34)? != 0xfc04
    {
        return None;
    }
    for operand in [0x2e, 0x6b] {
        let target = call_target(main, main_writer, operand)?;
        if target != call_target(subtitle, subtitle_writer, operand)? {
            return None;
        }
        file_bytes_at(target, true)?;
    }

    let mut setup_readers = code_sections.flat_map(|(rva, code)| {
        code.windows(SETUP_READER.len()).enumerate().filter_map(move |(offset, body)| {
            if !matches_code(body, SETUP_READER) {
                return None;
            }
            Some((rva.checked_add(u32::try_from(offset).ok()?)?, body))
        })
    }).filter(|&(rva, body)| {
        let target = call_target(rva, body, 0x1f);
        if target != call_target(rva, body, 0x45)
            || target.and_then(|rva| file_bytes_at(rva, true)).is_none()
        {
            return false;
        }
        [(1, &b"main\0"[..]), (6, b"id\0"), (11, b"language\0"),
            (0x2d, b"sub\0"), (0x32, b"id\0"), (0x37, b"language\0")].iter()
            .all(|&(offset, name)| {
                read_word(body, offset).and_then(|va| va.checked_sub(image_base))
                    .and_then(|rva| file_bytes_at(rva, false))
                    .is_some_and(|bytes| bytes.starts_with(name))
            })
    });
    setup_readers.next()?;
    setup_readers.next().is_none().then_some(())
}

fn call_target(rva: u32, body: &[u8], operand: usize) -> Option<u32> {
    relative_target(rva, operand, read_word(body, operand)?)
}

fn matches_code(body: &[u8], pattern: &[u8]) -> bool {
    body.get(..pattern.len()).is_some_and(|body| {
        body.iter().zip(pattern).all(|(&byte, &expected)| expected == b'?' || byte == expected)
    })
}

#[cfg(test)]
mod tests {
    use crate::{native_table::OrdinaryNativeTable, pe_resources::parse_sections};
    use std::path::Path;

    #[test]
    fn native1036_uses_registered_prefix_writers_and_setup_fields() -> std::io::Result<()> {
        let games = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../games");
        let neko = games.join(concat!(
            "[170526][NEKO WORKs] \u{30cd}\u{30b3}\u{3071}\u{3089} vol.3 steam\u{7248} (files)",
            "/nekopara_vol3/NEKOPARAvol3.exe",
        ));
        if neko.is_file() {
            let mut pe = std::fs::read(neko)?;
            pe.extend_from_slice(b"unrelated overlay");
            let recognizes = |pe: &[u8]| {
                OrdinaryNativeTable::from_pe(pe).expect("ordinary table")
                    .language_query_uses_story_prefixes()
            };
            assert!(recognizes(&pe));

            let sections = parse_sections(&pe).unwrap();
            let offset_at = |rva| {
                let bytes = sections.iter().find_map(|section| section.file_bytes_from(&pe, rva)).unwrap();
                bytes.as_ptr() as usize - pe.as_ptr() as usize
            };
            for rva in [0x247790 + 0x22, 0x313cd0 + 0x34, 0x320549 + 1] {
                let offset = offset_at(rva);
                let mut changed = pe.clone();
                changed[offset] ^= 1;
                assert!(!recognizes(&changed), "modified RVA {rva:#x}");
            }
            let table = offset_at(0x62dd38);
            let main = table + 1036 * 4;
            let prior = main - 4;
            pe.copy_within(prior..main, main);
            assert!(!recognizes(&pe));
        }

        let daydream = games.join("\u{767d}\u{663c}\u{5922}\u{306e}\u{9752}\u{5199}\u{771f}/cs2.exe");
        if daydream.is_file() {
            let pe = std::fs::read(daydream)?;
            let table = OrdinaryNativeTable::from_pe(&pe).expect("Daydream ordinary table");
            assert!(!table.language_query_uses_story_prefixes());
        }
        Ok(())
    }
}

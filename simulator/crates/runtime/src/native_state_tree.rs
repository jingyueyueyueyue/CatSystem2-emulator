//! Command packets emitted by the registered state-tree capture and restore.

use super::root_clock::read_word;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StateTreeCommands {
    pub capture: u32,
    pub restore: u32,
    pub release: u32,
}

pub(super) fn commands(capture_body: &[u8], restore_body: &[u8]) -> Option<StateTreeCommands> {
    let (capture, releases) = capture_commands(capture_body)?;
    let restore = restore_command(restore_body)?;
    // Both capture exits that obtained a tree send the same release message.
    let release = capture.checked_add(2)?;
    ((390..=460).contains(&capture)
        && restore == capture + 1
        && releases == [release; 2])
        .then_some(StateTreeCommands { capture, restore, release })
}

fn capture_commands(body: &[u8]) -> Option<(u32, [u32; 2])> {
    // Direct dispatch: null CSS can return in the entry block or branch
    // to the common zero-result tail, depending on the stack-frame layout.
    let direct_call = b"\x8b\x7c\x24\x18\x8d\x4c\x24\x1c\x51\x6a\0\x6a\0\x8b\x07\x8b\xcf\x6a\0\x68";
    let direct_zero = b"\x8b\x44\x24\x20\x5f\x5d\x5b\xc7\0\0\0\0\0\xb8\x01\0\0\0\x59\xc2\x10\0";
    if body.starts_with(b"\x51\x53\x8b\x5c\x24\x14\x55\x57\x8b\x03\x89\x44\x24\x0c\x85\xc0\x74\x58")
        || body.starts_with(b"\x51\x53\x8b\x5c\x24\x14\x8b\x03\x89\x44\x24\x04\x85\xc0\x75\x14\x8b\x44\x24\x18\x5b\xc7\0\0\0\0\0\xb8\x01\0\0\0\x59\xc2\x10\0\x55\x57")
    {
        let start = if body[6] == 0x55 { 0x12 } else { 0x26 };
        let capture = packet(body, start, direct_call,
            b"\xff\x73\x08\xff\x77\x04\xff\x50\x10\x8b\x6c\x24\x1c\x85\xed\x74\x2f")?;
        let release_packet = b"\x51\x6a\0\x55\x6a\0\x68";
        let release_call = b"\xff\x73\x08\x8b\xcf\xff\x77\x04\xff\x50\x10";
        let first = packet(body, start + 0x42, release_packet, release_call)?;
        let second = packet(body, start + 0xa6, release_packet, release_call)?;
        check(body, start + 0x58, direct_zero)?;
        check(body, start + 0x86, b"\xff\x74\x24\x1c\x8b\x4c\x24\x14\x56\xff\x73\x04\xe8")?;
        check(body, start + 0xbc,
            b"\x8b\x44\x24\x24\x5e\x5f\x5d\xc7\0\x01\0\0\0\xb8\x01\0\0\0\x5b\x59\xc2\x10\0")?;
        return Some((capture, [first, second]));
    }
    check(body, 0, b"\x51\x53\x8b\x5c\x24\x14\x8b\x03\x89\x44\x24\x04\x85\xc0\x75\x14\x8b\x44\x24\x18\xc7\0\0\0\0\0\xb8\x01\0\0\0\x5b\x59\xc2\x10\0")?;
    let capture = packet(body, 0x24,
        b"\x8b\x4b\x08\x56\x8b\x74\x24\x14\x8b\x16\x8b\x52\x10\x8d\x44\x24\x18\x50\x8b\x46\x04\x6a\0\x6a\0\x6a\0\x68",
        b"\x51\x50\x8b\xce\xff\xd2\x8b\x4c\x24\x18\x85\xc9\x75\x0f\x8b\x44\x24\x1c\x5e\x89\x08\x8d\x41\x01\x5b\x59\xc2\x10\0")?;
    let first = packet(body, 0x75,
        b"\x8b\x4b\x08\x8b\x16\x8b\x52\x10\x8d\x44\x24\x1c\x50\x8b\x46\x04\x6a\0\x55\x6a\0\x68",
        b"\x51\x50\x8b\xce\xff\xd2\x8b\x44\x24\x20\x5d\x5e\xc7\0\0\0\0\0\xb8\x01\0\0\0\x5b\x59\xc2\x10\0")?;
    check(body, 0xc3, b"\x8b\x4c\x24\x1c\x8b\x53\x04\x51\x8b\x4c\x24\x14\x57\x52\xe8")?;
    let second = packet(body, 0xe8,
        b"\x51\x8b\x4e\x04\x6a\0\x55\x6a\0\x68",
        b"\x52\x8b\x50\x10\x51\x8b\xce\xff\xd2\x8b\x44\x24\x24\x5f\x5d\x5e\xc7\0\x01\0\0\0\xb8\x01\0\0\0\x5b\x59\xc2\x10\0")?;
    Some((capture, [first, second]))
}

fn restore_command(body: &[u8]) -> Option<u32> {
    // Each layout reads CSS and its record before decoding or dispatching.
    if body.starts_with(b"\x55\x8b\xec\x6a\xff\x68") {
        check(body, 0x27, b"\x8b\x75\x10\x8b\x0e\x85\xc9\x75\x0b\x8b\x45\x14\xc7\0\0\0\0\0\xeb\x5c\x8d\x45\xf0\x50\xff\x76\x04\xe8")?;
        check(body, 0x47, b"\x8b\xf8\x85\xff\x74\xe3\x8d\x4d\x10\xe8")?;
        return packet(body, 0x67, b"\x8b\x4d\x0c\x8d\x55\xec\x52\x6a\0\x8d\x55\x10\x8b\x01\x52\x6a\0\x68",
            b"\xff\x76\x08\xff\x71\x04\xff\x50\x10\x8b\x45\x14\x8d\x4d\x10\xc7\0\x01\0\0\0\xe8");
    }
    check(body, 0, b"\x6a\xff\x68")?;
    if check(body, 0x25, b"\x8b\x74\x24\x2c\x8b\x0e\x85\xc9\x75\x0c\x8b\x44\x24\x30\xc7\0\0\0\0\0\xeb\x6d\x8d\x44\x24\x0c\x50\xff\x76\x04\xe8").is_some() {
        check(body, 0x48, b"\x8b\xf8\x85\xff\x74\xe1\x8d\x4c\x24\x2c\xe8")?;
        return packet(body, 0x6b, b"\x8b\x4c\x24\x28\x8d\x54\x24\x10\x52\x6a\0\x8d\x54\x24\x34\x8b\x01\x52\x6a\0\x68",
            b"\xff\x76\x08\xff\x71\x04\xff\x50\x10\x8b\x44\x24\x30\x8d\x4c\x24\x2c\xc7\x44\x24\x1c\xff\xff\xff\xff\xc7\0\x01\0\0\0\xe8");
    }
    if check(body, 0x25, b"\x8b\x7c\x24\x2c\x8b\x0f\x85\xc9\x75\x08\x8b\x44\x24\x30\x89\x08\xeb\x7a\x8b\x47\x04\x8d\x54\x24\x0c\x52\x50\xe8").is_some() {
        check(body, 0x45, b"\x8b\xf0\x85\xf6\x75\x08\x8b\x4c\x24\x30\x89\x01\xeb\x5e\x8d\x4c\x24\x2c\xe8")?;
        return packet(body, 0x70, b"\x8b\x4c\x24\x28\x8b\x11\x8b\x52\x10\x8d\x44\x24\x10\x50\x6a\0\x8d\x44\x24\x34\x50\x8b\x47\x08\x6a\0\x68",
            b"\x50\x8b\x41\x04\x50\xff\xd2\x8b\x44\x24\x30\x8d\x4c\x24\x2c\xc7\0\x01\0\0\0\xc7\x44\x24\x1c\xff\xff\xff\xff\xe8");
    }
    check(body, 0x26, b"\x8b\x5c\x24\x30\x8b\x0b\x33\xf6\x3b\xce\x75\x0b\x8b\x44\x24\x34\x89\x30\xe9\xa9\0\0\0\x8b\x43\x04\x8d\x54\x24\x10\x52\x50\xe8")?;
    check(body, 0x4b, b"\x8b\xf8\x3b\xfe\x75\x0b\x8b\x4c\x24\x34\x89\x31\xe9\x8a\0\0\0")?;
    packet(body, 0x9e, b"\x8b\x4c\x24\x2c\x8b\x11\x8b\x52\x10\x8d\x44\x24\x14\x50\x56\x8d\x44\x24\x38\x50\x8b\x43\x08\x56\x68",
        b"\x50\x8b\x41\x04\x50\xff\xd2\x8b\x44\x24\x34\xc7\0\x01\0\0\0\xc7\x44\x24\x20\xff\xff\xff\xff")
}

fn packet(body: &[u8], offset: usize, prefix: &[u8], suffix: &[u8]) -> Option<u32> {
    let operand = body.get(offset..)?.strip_prefix(prefix)?;
    check(operand, 4, suffix)?;
    read_word(operand, 0)
}

fn check(body: &[u8], offset: usize, expected: &[u8]) -> Option<()> {
    body.get(offset..)?.starts_with(expected).then_some(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_table::OrdinaryNativeTable;
    use crate::pe_resources::parse_sections;
    use std::{fs, io, path::Path};

    #[test]
    #[ignore = "requires installed games with all four state-tree wrapper layouts"]
    fn native_state_tree_commands_follow_installed_registration() -> io::Result<()> {
        let games = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../games");
        let samples = [
            ("cs2_full_v401/system/cs2_open.exe", 434),
            ("[170526][NEKO WORKs] ネコぱら vol.3 steam版 (files)/nekopara_vol3/NEKOPARAvol3.exe", 439),
            ("ISLAND/ISLAND.exe", 429),
            ("ISLAND/ISLAND_CHS.exe", 429),
            ("Yukikoi Melt/YukikoiMelt.exe", 445),
            ("ここから夏のイノセンス！/cs2.exe", 431),
            ("ここから夏のイノセンス！/cs2_cn.exe", 431),
            ("未来ラジオと人工鳩（AUGUST）/cs2.exe", 439),
            ("未来ラジオと人工鳩（AUGUST）/cs2_cn.exe", 439),
            ("果つることなき未来より/Hatsumira.exe", 429),
            ("灰色的乐园v1/Grisaia3.exe", 415),
            ("灰色的乐园v1/Grisaia3_cn.exe", 415),
            ("灰色的迷宫/Grisaia2.exe", 408),
            ("灰色的迷宫/Grisaia2_cn.exe", 408),
            ("牛顿与苹果树/cs2.exe", 436),
            ("牛顿与苹果树/牛顿与苹果树.exe", 436),
            ("白昼夢の青写真/cs2.exe", 445),
            ("カミカゼ☆エクスプローラー!/cs2.exe", 401),
            ("戦国天使ジブリール/Djibril5.exe", 403),
            ("灰色的果实/Grisaia.exe", 397),
            ("秘密游戏 反叛/REBELLIONS.exe", 420),
            ("Happiness！2 樱花盛典/cs2_crack.exe", 445),
        ];
        for (relative, capture) in samples {
            let mut image = fs::read(games.join(relative))?;
            let expected = StateTreeCommands { capture, restore: capture + 1, release: capture + 2 };
            for overlay in [b"".as_slice(), b"unrelated PE overlay"] {
                image.extend_from_slice(overlay);
                let table = OrdinaryNativeTable::from_pe(&image)
                    .unwrap_or_else(|| panic!("ordinary table missing for {relative}"));
                assert_eq!(table.state_tree_commands(), Some(expected), "{relative}");
            }
        }
        Ok(())
    }

    #[test]
    #[ignore = "requires the installed V401 engine"]
    fn native_state_tree_rejects_changed_packets_and_branches() -> io::Result<()> {
        let games = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../games");
        let original = fs::read(games.join("cs2_full_v401/system/cs2_open.exe"))?;
        let sections = parse_sections(&original).unwrap();
        let offset = |rva| sections.iter().find_map(|section| section.file_bytes_from(&original, rva))
            .unwrap().as_ptr() as usize - original.as_ptr() as usize;
        let registered = |image: &[u8]| OrdinaryNativeTable::from_pe(image).unwrap().state_tree_commands();
        assert!(registered(&original).is_some());
        for rva in [
            0x116500 + 0x0f, // Null-CSS branch.
            0x116500 + 0x6f, // Zero-length release packet command.
            0x116500 + 0xd3, // Success release packet command.
            0x115ae0 + 0x4d, // Missing-record branch.
            0x115ae0 + 0x80, // Restore packet command.
        ] {
            let mut changed = original.clone();
            changed[offset(rva)] ^= 1;
            assert!(registered(&changed).is_none(), "changed RVA {rva:#x}");
        }
        Ok(())
    }
}

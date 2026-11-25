//! Ordinary particle messages recovered from registered wrappers and the task receiver.

use super::root_clock::{
    operands, primary_task_vtable, primary_task_vtable_location, read_word, relative_target,
};
use crate::pe_resources::Section;

const CONTROL_PREFIX: &[u8] =
    b"\x8b\x4c\x24\x08\x8d\x44\x24\x08\x50\x8b\x44\x24\x10\x6a\x00\x8b\x11";
const CONTROL_TAIL: &[u8] =
    b"\xff\x30\xff\x71\x04\xff\x52\x10\xb8\x02\x00\x00\x00\xc2\x10\x00";
const LEGACY_CONTROL: [[&[u8]; 2]; 2] = [
    [
        b"\x8b\x4c\x24\x08\x8b\x01\x8b\x40\x10\x8d\x54\x24\x08\x52\x8b\x54\x24\x10\x8b\x12\x6a\x00\x6a\x00\x6a\x00\x68",
        b"\x52\x8b\x51\x04\x52\xff\xd0\xb8\x02\x00\x00\x00\xc2\x10\x00",
    ],
    [
        b"\x8b\x4c\x24\x08\x8b\x11\x8b\x52\x10\x56\x8d\x44\x24\x0c\x50\x8b\x44\x24\x14\x8b\x70\x04\x8b\x00\x6a\x00\x56\x6a\x00\x68",
        b"\x50\x8b\x41\x04\x50\xff\xd2\xb8\x02\x00\x00\x00\x5e\xc2\x10\x00",
    ],
];
const EARLY_CONTROL_PREFIX: [&[u8]; 2] = [
    b"\x8b\x4c\x24\x08\x8b\x01\x8b\x40\x0c\x8d\x54\x24\x08\x52\x8b\x54\x24\x10\x8b\x12\x6a\x00\x6a\x00\x6a\x00\x68",
    b"\x8b\x4c\x24\x08\x8b\x11\x8b\x52\x0c\x56\x8d\x44\x24\x0c\x50\x8b\x44\x24\x14\x8b\x70\x04\x8b\x00\x6a\x00\x56\x6a\x00\x68",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParticleMessages {
    pub base: u32,
    pub camera: bool,
}

pub(super) fn registered<'a>(
    entries: &[(u32, &'a [u8])],
    image: &[u8],
    sections: &[Section],
    image_base: u32,
    code: impl Fn(u32) -> Option<&'a [u8]>,
) -> Option<ParticleMessages> {
    let control = |slot: usize, takes_value| control_message(entries.get(slot)?.1, takes_value);
    let base = control(272, false)?.checked_sub(1)?;
    for (slot, offset, takes_value) in [(273, 2, false), (426, 6, true), (427, 7, true)] {
        if control(slot, takes_value)?.checked_sub(base) != Some(offset) {
            return None;
        }
    }

    let task = b".?AVkcTaskParticle@@\0";
    let receiver = primary_task_vtable(image, sections, image_base, task, &code)
        .map(|methods| methods[7])
        .or_else(|| primary_task_vtable_location::<7>(image, sections, image_base, task, &code, false)
            .map(|(_, methods)| methods[6]))?;
    let body = code(receiver)?;
    let variants: &[(&[u8], &[u8], &[u8], &[u8])] = &[
        (
            b"\x53\x8b\xd9\x83\x7b\x6c\x00\x75\x09\xb8\x01\x00\x00\x00\x5b\xc2\x18\x00\x8b\x44\x24\x0c\x05",
            b"\x56",
            b"\x8b\x74\x24\x18\x85\xf6\x0f\x84",
            b"\x57\x8d\xbb\x80\x00\x00\x00\xb9\xa4\x00\x00\x00\xf3\xa5",
        ),
        (
            b"\x53\x55\x8b\xd9\x33\xed\x39\x6b\x6c\x75\x08\x8d\x45\x01\x5d\x5b\xc2\x18\x00\x8b\x44\x24\x10\x05",
            b"\x56",
            b"\x8b\x74\x24\x1c\x3b\xf5\x0f\x84",
            b"\x57\x8d\xbb\x80\x00\x00\x00\xb9\xa4\x00\x00\x00\xf3\xa5",
        ),
        (
            b"\x55\x8b\xe9\x8b\x4d\x6c\x33\xd2\x3b\xca\x75\x07\x8d\x42\x01\x5d\xc2\x18\x00\x8b\x44\x24\x0c\x53\x05",
            b"\x56\x57",
            b"\x8b\x74\x24\x20\x3b\xf2\x0f\x84",
            b"\x8d\xbd\x80\x00\x00\x00\xb9\xa4\x00\x00\x00\x8b\xdd\xf3\xa5",
        ),
    ];
    if let Some((prefix, saved, load, copy)) = variants.iter().find(|variant| body.starts_with(variant.0)) {
        validate_record_switch(body.strip_prefix(*prefix)?, base, saved, load, copy, image_base, &code)?;
        return Some(ParticleMessages { base, camera: false });
    }

    if let Some([skip]) = operands(body,
        b"\x55\x8b\xec\x83\xe4\xf8\x83\xec\x0c\x53\x8b\xd9\x8b\x4b\x6c\x33\xd2\x56\x57\x3b\xca\x0f\x84",
        [b"\x8b\x45\x0c\x05"],
    ) {
        validate_record_switch(body.get(skip.0 + 8..)?, base, b"",
            b"\x8b\x75\x14\x3b\xf2\x0f\x84", b"\x8d\xbb\x80\x00\x00\x00\xb9\xa4\x00\x00\x00\x53\xf3\xa5",
            image_base, &code)?;
        return Some(ParticleMessages { base, camera: false });
    }

    if let Some([skip]) = operands(body,
        b"\x55\x8b\xec\x83\xe4\xf8\x51\x53\x8b\xd9\x8b\x4b\x60\x33\xd2\x56\x57\x3b\xca\x0f\x84",
        [b"\x8b\x45\x0c\x05"],
    ) {
        let dispatch = body.get(skip.0 + 8..)?;
        let load = b"\x8b\x75\x14\x3b\xf2\x0f\x84";
        [
            b"\x8d\x7b\x74\xb9\x62\x00\x00\x00\x53\xf3\xa5",
            b"\x8d\x7b\x74\xb9\xa3\x00\x00\x00\x53\xf3\xa5",
        ].iter().find_map(|copy| validate_record_switch(dispatch, base, b"", load, *copy, image_base, &code))?;
        return Some(ParticleMessages { base, camera: false });
    }

    let [_, clear, set] = operands(body,
        b"\x8b\x54\x24\x10\x53\x8b\xd9\x56\x83\x7b\x70\x00\x0f\x84",
        [b"\x8b\x44\x24\x10\x3d\x4f\x02\x00\x00\x0f\x8f", b"\x0f\x84", b"\x05"],
    )?;
    validate_record_switch(body.get(set.0 + 5..)?, base, b"",
        b"\x85\xd2\x0f\x84", b"\x57\x8b\xf2\x8d\xbb\x84\x00\x00\x00\xb9\xa4\x00\x00\x00\xf3\xa5",
        image_base, &code)?;
    let camera = control(1068, true) == Some(591)
        && control(1069, false) == Some(592)
        && relative_target(receiver, set.0, set.1).and_then(&code)
            .is_some_and(|body| body.starts_with(b"\x8b\x03\x8d\x4c\x24\x18\x51\x6a\x00\x6a\x00\x6a\x00\x68\x4d\x02\x00\x00"))
        && relative_target(receiver, clear.0, clear.1).and_then(&code)
            .is_some_and(|body| body.starts_with(b"\x3d\x50\x02\x00\x00\x75"));
    Some(ParticleMessages { base, camera })
}

fn control_message(body: &[u8], takes_value: bool) -> Option<u32> {
    if let Some(body) = body.strip_prefix(CONTROL_PREFIX) {
        let arguments = if takes_value { b"\xff\x70\x04\x6a\x00\x68".as_slice() }
            else { b"\x6a\x00\x6a\x00\x68" };
        let [(_, command)] = operands(body, arguments, [CONTROL_TAIL])?;
        return Some(command);
    }
    let argument = usize::from(takes_value);
    let [prefix, tail] = LEGACY_CONTROL[argument];
    let [(_, command)] = operands(body, prefix, [tail])
        .or_else(|| operands(body, EARLY_CONTROL_PREFIX[argument], [tail]))?;
    Some(command)
}

fn validate_record_switch<'a>(
    body: &[u8], base: u32, saved: &[u8], load_prefix: &[u8], load_copy: &[u8],
    image_base: u32, code: &impl Fn(u32) -> Option<&'a [u8]>,
) -> Option<()> {
    let selector = read_word(body, 0)?;
    let body = body.get(4..)?.strip_prefix(saved)?;
    let [_, (_, table)] = operands(body, b"\x83\xf8\x07\x0f\x87", [b"\xff\x24\x85", b""])?;
    if base == 0 || selector != base.wrapping_neg() {
        return None;
    }
    let load = code(table.checked_sub(image_base)?)?;
    let load = code(read_word(load, 0)?.checked_sub(image_base)?)?;
    operands(load, load_prefix, [load_copy])?;
    Some(())
}

#[cfg(test)]
mod tests {
    use crate::native_table::OrdinaryNativeTable;

    #[test]
    #[ignore = "requires installed engines with registered particle receivers"]
    fn particle_messages_follow_the_registered_wrappers() {
        let games = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../games");
        let cases = [
            ("cs2_full_v401/system/cs2_open.exe", 309, false),
            ("ISLAND/ISLAND.exe", 304, false),
            ("ISLAND/ISLAND_CHS.exe", 304, false),
            ("Yukikoi Melt/YukikoiMelt.exe", 320, true),
            ("白昼夢の青写真/cs2.exe", 320, true),
            ("Happiness！2 樱花盛典/cs2_crack.exe", 320, true),
            ("未来ラジオと人工鳩（AUGUST）/cs2.exe", 314, false),
            ("未来ラジオと人工鳩（AUGUST）/cs2_cn.exe", 314, false),
            ("果つることなき未来より/Hatsumira.exe", 304, false),
            ("ここから夏のイノセンス！/cs2.exe", 306, false),
            ("ここから夏のイノセンス！/cs2_cn.exe", 306, false),
            ("牛顿与苹果树/cs2.exe", 311, false),
            ("牛顿与苹果树/牛顿与苹果树.exe", 311, false),
            ("カミカゼ☆エクスプローラー!/cs2.exe", 285, false),
            ("戦国天使ジブリール/Djibril5.exe", 287, false),
            ("灰色的果实/Grisaia.exe", 285, false),
            ("灰色的迷宫/Grisaia2.exe", 292, false),
            ("灰色的迷宫/Grisaia2_cn.exe", 292, false),
            ("灰色的乐园v1/Grisaia3.exe", 297, false),
            ("灰色的乐园v1/Grisaia3_cn.exe", 297, false),
            ("秘密游戏 反叛/REBELLIONS.exe", 302, false),
            ("うたてめぐり/UTATEMEGURI.exe", 277, false),
            ("ツナバン らぶみくす/cs2.exe", 265, false),
            ("ツナバン らぶみくす/cs2_gemini2.5pro.exe", 265, false),
            ("祝福のカンパネラ/cs2.exe", 271, false),
            ("[170526][NEKO WORKs] ネコぱら vol.3 steam版 (files)/nekopara_vol3/NEKOPARAvol3.exe", 314, false),
        ];
        let mut failures = Vec::new();
        for (name, base, camera) in cases {
            let mut bytes = std::fs::read(games.join(name)).expect("read particle PE");
            bytes.extend_from_slice(b"test overlay");
            let expected = Some(super::ParticleMessages { base, camera });
            let actual = OrdinaryNativeTable::from_pe(&bytes).and_then(|table| table.particle_messages());
            if actual != expected {
                failures.push(format!("{name}: expected {expected:?}, got {actual:?}"));
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
        let scan = crate::GameRoot::open(games.join("cs2_full_v401/system")).unwrap().scan().unwrap();
        let table = OrdinaryNativeTable::from_game_scan(&scan, Some("kcs.int/main.kcs"))
            .expect("registered ordinary table from the actual game scan");
        assert_eq!(table.particle_messages().map(|messages| messages.base), Some(309));
    }
}

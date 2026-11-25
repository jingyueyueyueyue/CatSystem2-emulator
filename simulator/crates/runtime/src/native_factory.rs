//! Null task factory leaves reached by the ordinary slot 66 dispatch.

use super::root_clock::{primary_task_vtable, read_word, relative_target};
use crate::pe_resources::Section;

#[derive(Debug, Default, Clone, Copy)]
pub struct NullTaskFactory([bool; 2]);

impl NullTaskFactory {
    pub fn returns_null(self, kind: u32) -> bool {
        match kind {
            14 => self.0[0],
            43 => self.0[1],
            _ => false,
        }
    }
}

pub(super) fn registered_factory<'a>(
    wrapper: &[u8],
    emote_load_wrapper: Option<&[u8]>,
    image: &[u8],
    sections: &[Section],
    image_base: u32,
    code: impl Fn(u32) -> Option<&'a [u8]>,
) -> Option<(NullTaskFactory, bool)> {
    // The wrapper sends message 1 through the task vtable, passing the kind,
    // parent, formatted name, and an output pointer. There are three compiled
    // argument orders for the same seven-push ABI.
    if !wrapper_dispatch(wrapper) {
        return None;
    }
    let methods = primary_task_vtable(image, sections, image_base, b".?AVkcTaskTop@@\0", &code)?;
    let route_rva = methods[5];
    let route = code(route_rva)?;
    let call = [
        b"\x53\x8b\x5c\x24\x0c\x56\x8b\x74\x24\x0c\x57\x8b\xf9\x8b\x46\x08\x48\x75\x23\x8d\x44\x24\x10\x50\xff\x76\x10\xff\x76\x0c\xe8".as_slice(),
        b"\x53\x8b\x5c\x24\x0c\x56\x8b\x74\x24\x0c\x57\x8b\xf9\x8b\x46\x08\x83\xe8\x01\x75\x23\x8d\x44\x24\x10\x50\xff\x76\x10\xff\x76\x0c\xe8",
        b"\x53\x8b\x5c\x24\x0c\x56\x8b\x74\x24\x0c\x8b\x46\x08\x83\xe8\x01\x57\x8b\xf9\x75\x27\x8b\x4e\x10\x8b\x56\x0c\x8d\x44\x24\x10\x50\x51\x52\x8b\xcf\xe8",
        b"\x53\x8b\x5c\x24\x0c\x56\x8b\x74\x24\x0c\x8b\x46\x08\x83\xe8\x01\x57\x8b\xf9\x75\x25\x8b\x4e\x0c\x8d\x44\x24\x10\x50\x8b\x46\x10\x51\x57\xe8",
    ]
    .into_iter()
    .find_map(|prefix| route.starts_with(prefix).then_some(prefix.len()))?;
    let factory_rva = relative_target(route_rva, call, read_word(route, call)?)?;
    let factory = code(factory_rva)?;
    let dispatch = factory_dispatch(factory)?;
    let find_target = |operand| relative_target(factory_rva, operand, read_word(factory, operand)?);
    let search = find_target(dispatch.first_search)?;
    if find_target(dispatch.second_search)? != search || code(search)?.is_empty() {
        return None;
    }

    let switch = factory.get(dispatch.switch..)?;
    let [0x83, compare, last, 0x0f, 0x87, ..] = *switch.get(..16)? else {
        return None;
    };
    if !(43..=127).contains(&last)
        || compare != dispatch.compare
        || switch.get(9..12)? != [0xff, 0x24, dispatch.index]
    {
        return None;
    }
    let table_rva = read_word(switch, 12)?.checked_sub(image_base)?;
    let default = find_target(dispatch.switch + 5)?;
    code(default)?;
    let table = code(table_rva)?.get(..(usize::from(last) + 1) * 4)?;
    let target = |kind: usize| {
        (kind <= usize::from(last)).then_some(())?;
        read_word(table, kind * 4)?.checked_sub(image_base)
    };
    // Slots 0, 1, and 13 are real constructors in these factory layouts.
    // This corroborates the table and rejects a table of default targets.
    for kind in [0, 1, 13] {
        let entry = target(kind)?;
        if entry < factory_rva || entry >= table_rva || entry == default
            || code(entry)?.starts_with(dispatch.null_epilogue)
        {
            return None;
        }
    }

    let null = NullTaskFactory([14, 43].map(|kind| {
        target(kind)
            .filter(|&entry| entry > factory_rva && entry < table_rva)
            .and_then(&code)
            .is_some_and(|body| body.starts_with(dispatch.null_epilogue))
    }));
    let emote = emote_load_wrapper
        .and_then(|wrapper| emote_factory(target(54)?, wrapper, image, sections, image_base, &code))
        .is_some();
    Some((null, emote))
}

fn emote_factory<'a>(
    leaf_rva: u32,
    wrapper: &[u8],
    image: &[u8],
    sections: &[Section],
    image_base: u32,
    code: &impl Fn(u32) -> Option<&'a [u8]>,
) -> Option<()> {
    // Ordinary 906 formats the resource name and sends it with mode in the
    // 524 load packet. Verify that both operands use the same scratch buffer.
    let tail = wrapper.get(22..)?;
    if !wrapper.starts_with(b"\x8b\x4c\x24\x04\x56\x8b\x74\x24\x10\x68")
        || wrapper.get(14..18)? != b"\xff\x76\x04\xe8"
        || !tail.starts_with(b"\x8b\x4c\x24\x0c\x8d\x54\x24\x10\x52\x6a\0\x68")
        || read_word(wrapper, 10)? != read_word(tail, 12)?
        || !tail.get(16..)?.starts_with(b"\xff\x76\x08\x8b\x01\x68\x0c\x02\0\0\xff\x36\xff\x71\x04\xff\x50\x10\xb8\x02\0\0\0\x5e\xc2\x10\0")
    {
        return None;
    }
    let leaf = code(leaf_rva)?;
    if !leaf.starts_with(b"\x68\xb8\x04\0\0\xe8")
        || leaf.get(10..23)? != b"\x83\xc4\x04\x89\x44\x24\x28\xc7\x44\x24\x20\x2e\0"
        || leaf.get(33..47)? != b"\xff\x77\x68\x8b\xc8\xff\x77\x48\xff\x74\x24\x34\x53\xe8"
    {
        return None;
    }
    let constructor = code(relative_target(leaf_rva, 47, read_word(leaf, 47)?)?)?;
    let (vtable, methods) = super::root_clock::primary_task_vtable_location::<8>(
        image, sections, image_base, b".?AVkcTaskM2Plane@@\0", code, false,
    )?;
    if constructor.get(83..85)? != b"\xc7\x07"
        || read_word(constructor, 85)? != vtable
        || constructor.get(127..133)? != b"\x89\x87\x80\x03\0\0"
    {
        return None;
    }
    let receiver_rva = methods[7];
    let receiver = code(receiver_rva)?;
    if receiver.get(93..101)? != b"\x81\xf9\x0c\x02\0\0\x0f\x8f"
        || receiver.get(105..107)? != b"\x0f\x84"
    {
        return None;
    }
    let load = code(relative_target(receiver_rva, 107, read_word(receiver, 107)?)?)?;
    load.starts_with(b"\x8b\x8e\x80\x03\0\0").then_some(())
}

fn wrapper_dispatch(body: &[u8]) -> bool {
    let (formatter_name, dispatch_name, push_message): (_, _, &[u8]) = if body.starts_with(
        b"\x8b\x4c\x24\x04\x56\x8b\x74\x24\x10\x57\x68",
    ) && body.get(23..33) == Some(b"\x8b\x7c\x24\x10\x8d\x4c\x24\x14\x51\x6a")
        && body.get(33..35) == Some(b"\0\x68")
    {
        (11, 35, b"\xff\x36\x8b\x07\x8b\xcf\x6a\x01\xff\x76\x04\xff\x77\x04\xff\x50\x10")
    } else if body.starts_with(b"\x8b\x4c\x24\x04\x56\x57\x8b\x7c\x24\x14\x8b\x47\x08\x68")
        && body.get(24..46) == Some(b"\x8b\x0f\x8b\x74\x24\x10\x8b\x16\x8b\x52\x10\x8d\x44\x24\x14\x50\x8b\x47\x04\x6a\0\x68")
    {
        (14, 46, b"\x51\x8b\x4e\x04\x6a\x01\x50\x51\x8b\xce\xff\xd2")
    } else if body.starts_with(b"\x56\x8b\x74\x24\x08\x57\x8b\x7c\x24\x14\x8b\x47\x08\x50\x56\xe8")
        && body.get(33..55) == Some(b"\x8b\x17\x8b\x74\x24\x10\x8b\x06\x8b\x40\x10\x8d\x4c\x24\x14\x51\x8b\x4f\x04\x6a\0\x68")
    {
        (21, 55, b"\x52\x8b\x56\x04\x6a\x01\x51\x52\x8b\xce\xff\xd0")
    } else {
        return false;
    };
    read_word(body, formatter_name).is_some_and(|name| read_word(body, dispatch_name) == Some(name))
        && body.get(dispatch_name + 4..).is_some_and(|tail| tail.starts_with(push_message))
}

struct FactoryDispatch {
    first_search: usize,
    second_search: usize,
    switch: usize,
    compare: u8,
    index: u8,
    null_epilogue: &'static [u8],
}

fn factory_dispatch(body: &[u8]) -> Option<FactoryDispatch> {
    let (first, loop_start, middle, loop_end, compare, index, null_epilogue): (_, _, &[u8], &[u8], _, _, &[u8]) =
        if body.starts_with(b"\x51\x53\x55\x56\x57\x8d\x44\x24\x10\x33\xdb\x50\x53\x8b\xf9")
        {
            (0x0f, 0x14, b"\x85\xc0\x74\x12\x8d\x44\x24\x10\x43\x50\x53\x8b\xcf\xe8",
                b"\x85\xc0\x75\xee\x8b\x6c\x24\x18", 0xfd, 0xad, b"\x5f\x5e\x5d\x33\xc0\x5b\x59\xc2\x0c\0")
        } else if body.starts_with(b"\x51\x53\x55\x56\x57\x8d\x44\x24\x10\x50\x33\xdb\x53\x8b\xf1") {
            (0x0f, 0x14, b"\x85\xc0\x74\x12\x8d\x4c\x24\x10\x51\x43\x53\x8b\xce\xe8",
                b"\x85\xc0\x75\xee\x8b\x6c\x24\x18", 0xfd, 0xad, b"\x5f\x5e\x5d\x33\xc0\x5b\x59\xc2\x0c\0")
        } else if body.starts_with(b"\x6a\xff\x68")
            && body.get(0x18..0x2f)?.starts_with(b"\x33\xc4\x50\x8d\x44\x24\x18\x64\xa3\0\0\0\0\x8b\xf9\x8d\x44\x24\x14\x33\xdb\x50\x53")
        {
            (0x2f, 0x34, b"\x85\xc0\x74\x12\x8d\x44\x24\x14\x43\x50\x53\x8b\xcf\xe8",
                b"\x85\xc0\x75\xee\x8b\x6c\x24\x28", 0xfd, 0xad,
                b"\x33\xc0\x8b\x4c\x24\x18\x64\x89\x0d\0\0\0\0\x59\x5f\x5e\x5d\x5b\x83\xc4\x10\xc2\x0c\0")
        } else if body.starts_with(b"\x55\x8b\xec\x6a\xff\x68")
            && body.get(0x1c..0x31)?.starts_with(b"\x33\xc5\x50\x8d\x45\xf4\x64\xa3\0\0\0\0\x8b\xf9\x8d\x45\xf0\x33\xdb\x50\x53")
        {
            (0x31, 0x36, b"\x85\xc0\x74\x17\x66\x0f\x1f\x44\0\0\x8d\x45\xf0\x43\x50\x53\x8b\xcf\xe8",
                b"\x85\xc0\x75\xef\x8b\x45\x08", 0xf8, 0x85,
                b"\x33\xc0\x8b\x4d\xf4\x64\x89\x0d\0\0\0\0\x59\x5f\x5e\x5b\x8b\xe5\x5d\xc2\x0c\0")
        } else if body.starts_with(b"\x55\x8b\xec\x83\xe4\xf8\x83\xec\x0c\x53\x8b\x5d\x08\x56\x57\x8b\xf0\x8d\x44\x24\x14\x50\x33\xff\x57\x8b\xc3\x89\x7c\x24\x18") {
            (0x1f, 0x24, b"\x85\xc0\x74\x16\x8d\x4c\x24\x14\x51\x47\x57\x8b\xc3\xe8",
                b"\x85\xc0\x75\xee\x89\x7c\x24\x10\x8b\x45\x0c", 0xf8, 0x85,
                b"\x5f\x5e\x33\xc0\x5b\x8b\xe5\x5d\xc2\x0c\0")
        } else {
            return None;
        };
    let second_search = loop_start + middle.len();
    let after_second = second_search + 4;
    if body.get(first) != Some(&0xe8)
        || !body.get(loop_start..)?.starts_with(middle)
        || !body.get(after_second..)?.starts_with(loop_end)
    {
        return None;
    }
    Some(FactoryDispatch {
        first_search: first + 1,
        second_search,
        switch: after_second + loop_end.len(),
        compare,
        index,
        null_epilogue,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_table::OrdinaryNativeTable;
    use crate::pe_resources::{parse_sections, x86_image_layout};
    use std::{fs, io, path::Path};

    #[test]
    #[ignore = "requires the installed M2 and V401 engine samples"]
    fn emote_factory_and_load_protocol_follow_registered_slots() -> io::Result<()> {
        let games = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../games");
        let path = games.join("[170526][NEKO WORKs] ネコぱら vol.3 steam版 (files)/nekopara_vol3/NEKOPARAvol3.exe");
        let mut image = fs::read(path)?;
        assert!(OrdinaryNativeTable::from_pe(&image).unwrap().emote_task_family());
        image.extend_from_slice(b"unrelated PE overlay");
        assert!(OrdinaryNativeTable::from_pe(&image).unwrap().emote_task_family());
        let sections = parse_sections(&image).unwrap();
        let load = sections.iter().find_map(|section| section.file_bytes_from(&image, 0x24d2db)).unwrap();
        let message = load.as_ptr() as usize - image.as_ptr() as usize + 1;
        image[message] = 0x0d;
        assert!(!OrdinaryNativeTable::from_pe(&image).unwrap().emote_task_family());
        let other = fs::read(games.join("cs2_full_v401/system/cs2_open.exe"))?;
        assert!(!OrdinaryNativeTable::from_pe(&other).unwrap().emote_task_family());
        Ok(())
    }

    #[test]
    #[ignore = "requires installed games carrying the known task factory layouts"]
    fn native_factory_follows_registered_slots_for_installed_games() -> io::Result<()> {
        let games = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../games");
        let samples = [
            "cs2_full_v401/system/cs2_open.exe",
            "[170526][NEKO WORKs] ネコぱら vol.3 steam版 (files)/nekopara_vol3/NEKOPARAvol3.exe",
            "ISLAND/ISLAND.exe",
            "Yukikoi Melt/YukikoiMelt.exe",
            "ここから夏のイノセンス！/cs2.exe",
            "未来ラジオと人工鳩（AUGUST）/cs2.exe",
            "果つることなき未来より/Hatsumira.exe",
            "灰色的乐园v1/Grisaia3.exe",
            "灰色的迷宫/Grisaia2.exe",
            "牛顿与苹果树/cs2.exe",
            "牛顿与苹果树/牛顿与苹果树.exe",
            "白昼夢の青写真/cs2.exe",
            "カミカゼ☆エクスプローラー!/cs2.exe",
            "戦国天使ジブリール/Djibril5.exe",
            "灰色的果实/Grisaia.exe",
        ];
        for relative in samples {
            let mut image = fs::read(games.join(relative))?;
            for overlay in [b"".as_slice(), b"unrelated PE overlay"] {
                image.extend_from_slice(overlay);
                let table = OrdinaryNativeTable::from_pe(&image)
                    .unwrap_or_else(|| panic!("ordinary table missing for {relative}"));
                let factory = table.null_task_factory();
                assert!(factory.returns_null(14), "{relative}, kind 14");
                assert!(factory.returns_null(43), "{relative}, kind 43");
                for kind in [0, 1, 13, 42, 44, u32::MAX] {
                    assert!(!factory.returns_null(kind), "{relative}, kind {kind}");
                }
            }
        }
        Ok(())
    }

    #[test]
    #[ignore = "requires the installed V401 task factory"]
    fn native_factory_rejects_changed_wrapper_callback_and_leaf() -> io::Result<()> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../games/cs2_full_v401/system/cs2_open.exe");
        let original = fs::read(path)?;
        let sections = parse_sections(&original).unwrap();
        let (base, _) = x86_image_layout(&original).unwrap();
        let index = |rva| {
            sections.iter().find_map(|section| section.file_bytes_from(&original, rva))
                .unwrap().as_ptr() as usize - original.as_ptr() as usize
        };
        let table = |image: &[u8]| OrdinaryNativeTable::from_pe(image).unwrap().null_task_factory();
        assert!(table(&original).returns_null(14) && table(&original).returns_null(43));

        // Each mutation breaks one edge of the registered wrapper -> Top
        // callback -> direct factory table -> side-effect-free return chain.
        let mut wrong_message = original.clone();
        let wrapper_start = index(0x128e20);
        let message = original[wrapper_start..wrapper_start + 60].windows(4)
            .position(|bytes| bytes == b"\x6a\x01\xff\x76").unwrap() + 1;
        wrong_message[wrapper_start + message] = 2;
        assert!(!table(&wrong_message).returns_null(43));

        let mut wrong_callback = original.clone();
        let top_route_slot = index(0x592cbc + 5 * 4);
        let other_callback = read_word(&original, top_route_slot + 4).unwrap();
        wrong_callback[top_route_slot..top_route_slot + 4].copy_from_slice(&other_callback.to_le_bytes());
        assert!(!table(&wrong_callback).returns_null(43));

        let mut construction = original.clone();
        let slot = index(0x0baa90 + 43 * 4);
        let target = base + 0x0b9fad;
        construction[slot..slot + 4].copy_from_slice(&target.to_le_bytes());
        let changed = table(&construction);
        assert!(!changed.returns_null(43) && changed.returns_null(14));

        let mut wrong_epilogue = original.clone();
        wrong_epilogue[index(0x0baa86) + 3] = 0x8b;
        assert!(!table(&wrong_epilogue).returns_null(14));
        assert!(!table(&wrong_epilogue).returns_null(43));
        Ok(())
    }
}

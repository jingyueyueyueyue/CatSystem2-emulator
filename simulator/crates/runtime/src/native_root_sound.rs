//! The Top message that returns its constructor-owned kind 13 sound task.
use super::root_clock::{operands, primary_task_vtable_location, read_word, relative_target};
use crate::pe_resources::Section;

pub(super) fn registered<'a>(
    image: &[u8],
    sections: &[Section],
    image_base: u32,
    file: impl Fn(u32, bool) -> Option<&'a [u8]>,
) -> Option<u32> {
    let code = |rva| file(rva, true);
    let top = b".?AVkcTaskTop@@\0";
    let (table, receiver) = primary_task_vtable_location::<8>(image, sections, image_base, top, &code, false)
        .map(|(table, methods)| (table, methods[7]))
        .or_else(|| primary_task_vtable_location::<7>(image, sections, image_base, top, &code, false)
            .map(|(table, methods)| (table, methods[6])))?;
    let body = code(receiver)?;
    let (switch, index_register, output_register) = if body.starts_with(b"\x8b\x44\x24\x08\x48") {
        (5, 0, 0)
    } else if at(body, 0x40, b"\x8b\x7d\x14\x8b\x45\x1c\x8b\xd9\x8b\x4d\x0c\x49") {
        (0x54, 1, 1)
    } else if at(body, 0x40, b"\x8b\x75\x14\x8b\x45\x1c\x8b\xd9\x8b\x4d\x0c\x49\x89\x74\x24\x28\x89\x44\x24\x20") {
        (0x54, 1, 1)
    } else if at(body, 0x3b, b"\x8b\xf1\x8b\x84\x24\x58\x02\0\0\x8b\x9c\x24\x68\x02\0\0\x48") {
        (0x57, 0, 2)
    } else if at(body, 0x3b, b"\x8b\x84\x24\x6c\x02\0\0\x8b\xac\x24\x7c\x02\0\0\x8b\x9c\x24\x74\x02\0\0\x48\x8b\xf1") {
        (0x57, 0, 3)
    } else {
        return None;
    };
    let (default, selectors, jumps) = switch_tables(receiver, body, switch, index_register, image_base, &code)?;
    let typed = |va: u32| file(va.checked_sub(image_base)?, false);
    let field = constructor_sound_field(receiver, table, &code, &typed)?;
    let mut commands = selectors.iter().enumerate().filter_map(|(index, &selector)| {
        let target = read_word(jumps, usize::from(selector) * 4)?.checked_sub(image_base)?;
        if target == default || target < receiver { return None; }
        let leaf = code(target)?;
        if output_register == 0 {
            let tail = leaf.strip_prefix(b"\x8b\x44\x24\x18")?;
            let (&displacement, tail) = match tail {
                [0x8b, 0x49, field, tail @ ..] => (field, tail),
                [0x8b, 0x89, field, 0, 0, 0, tail @ ..] => (field, tail),
                _ => return None,
            };
            if displacement != field || !tail.starts_with(b"\x89\x08\x33\xc0\xc2\x18\0") { return None; }
        } else {
            let prefix = match output_register {
                1 => [0x8b, 0x53, field, 0x89, 0x10, 0xe9, 0, 0],
                2 => [0x8b, 0x46, field, 0x89, 0x03, 0xe9, 0, 0],
                _ => [0x8b, 0x46, field, 0x89, 0x45, 0, 0xe9, 0],
            };
            let operand = if output_register == 3 { 7 } else { 6 };
            if !leaf.starts_with(&prefix[..operand])
                || relative_target(target, operand, read_word(leaf, operand)?) != Some(default) {
                return None;
            }
        }
        Some(index as u32 + 1)
    });
    let command = commands.next()?;
    commands.next().is_none().then_some(command)
}

fn switch_tables<'a>(
    rva: u32, body: &[u8], switch: usize, register: u8, base: u32,
    code: &impl Fn(u32) -> Option<&'a [u8]>,
) -> Option<(u32, &'a [u8], &'a [u8])> {
    let [0x83, compare, limit @ 0x1d..=0x3f, ..] = *body.get(switch..switch + 3)? else { return None; };
    if compare != 0xf8 + register { return None; }
    let tail = body.get(switch + 3..)?;
    let [(branch, default), (_, selectors), (_, jumps)] = operands(tail, b"\x0f\x87", [
        &[0x0f, 0xb6, 0x80 + register * 9], &[0xff, 0x24, 0x85 + register * 8], b"",
    ])?;
    let default = relative_target(rva, switch + 3 + branch, default)?;
    if !code(default)?.starts_with(b"\x33\xc0") { return None; }
    let selectors = code(selectors.checked_sub(base)?)?.get(..=usize::from(limit))?;
    let jumps = code(jumps.checked_sub(base)?)?;
    Some((default, selectors, jumps))
}

fn constructor_sound_field<'a>(
    receiver: u32, table: u32, code: &impl Fn(u32) -> Option<&'a [u8]>,
    data: &impl Fn(u32) -> Option<&'a [u8]>,
) -> Option<u8> {
    // Top constructors appear near the receiver in all observed layouts. Bind
    // each candidate receiver to its own vtable store and its named kind 13.
    let start = receiver.checked_sub(0x2000)?;
    let region = code(start)?.get(..0x2000)?;
    let mut fields = region.windows(6).enumerate().filter_map(|(at, store)| {
        if !matches!(store[..2], [0xc7, 0x06 | 0x07]) || read_word(store, 2) != Some(table) {
            return None;
        }
        let following = region.get(at + 6..)?;
        let name_search = following.get(..following.len().min(240))?;
        name_search.windows(5).enumerate().find_map(|(at, operand)| {
            if !matches!(operand[0], 0x68 | 0xb8 | 0xb9) { return None; }
            let field = read_word(operand, 1)?;
            if !data(field).is_some_and(|literal| literal.starts_with(b"SOUND TASK\0")) { return None; }
            let body = following.get(at..)?;
            sound_field_for_constructor(body, field, store[1])
        })
    });
    let field = fields.next()?;
    fields.next().is_none().then_some(field)
}

fn sound_field_for_constructor(body: &[u8], name: u32, owner: u8) -> Option<u8> {
    // The inline packet tags the factory input as a kind 13 child. The
    // handle is read back after registration and committed to that same Top.
    let (kind, field) = match (owner, body.first().copied()?) {
        (7, 0x68) if at(body, 5, b"\x6a\x0d\x89\x74\x24\x24\xc7\x44\x24\x2c\x01\0\0\0")
            && at(body, 0x1b, b"\xc7\x44\x24\x34") && read_word(body, 0x1f) == Some(name)
            && at(body, 0x53, b"\x8b\x4c\x24\x0c\xe8")
            && at(body, 0x5c, b"\x89\x47") => (0x17, 0x5e),
        (7, 0x68) if at(body, 5, b"\x6a\x0d\x89\x75\xd8\xc7\x45\xe0\x01\0\0\0")
            && at(body, 0x18, b"\xc7\x45\xe8") && read_word(body, 0x1b) == Some(name)
            && at(body, 0x48, b"\x8b\x4d\xd4\xe8") && at(body, 0x50, b"\x89\x87") => (0x14, 0x52),
        (6, 0x68) if at(body, 5, b"\x6a\x0d\x8b\xce\x89\x46\x78")
            && at(body, 0x24, b"\xc7\x44\x24\x2c") && read_word(body, 0x28) == Some(name)
            && at(body, 0x5a, b"\x8b\x4c\x24\x40\xe8") && at(body, 0x63, b"\x89\x46") => (0x20, 0x65),
        (6, 0xb8) if at(body, 0x1e, b"\x89\x44\x24\x2c\xe8")
            && at(body, 0x50, b"\x8b\x54\x24\x3c\x8b\x42\x04\x89\x46") => (0x1a, 0x59),
        (6, 0xb9) if at(body, 0x21, b"\x89\x4c\x24\x2c\xe8")
            && at(body, 0x53, b"\x8b\x4c\x24\x3c\x8b\x51\x04\x89\x56") => (0x1d, 0x5c),
        _ => return None,
    };
    if read_word(body, kind)? != 13 { return None; }
    let field = *body.get(field)?;
    matches!(field, 0x70 | 0x7c | 0x80).then_some(field)
}

fn at(body: &[u8], offset: usize, pattern: &[u8]) -> bool {
    body.get(offset..offset + pattern.len()) == Some(pattern)
}

#[cfg(test)]
mod tests {
    use crate::native_table::OrdinaryNativeTable;
    use std::{fs, io, path::Path};

    #[test]
    #[ignore = "requires the installed legacy and modern sound engines"]
    fn native_root_sound_uses_owning_top_and_message_receiver() -> io::Result<()> {
        let games = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../games");
        let mut failures = Vec::new();
        for (name, expected) in [
            ("cs2_full_v401/system/cs2_open.exe", 33),
            ("Yukikoi Melt/YukikoiMelt.exe", 35),
            ("Happiness！2 樱花盛典/cs2_crack.exe", 35),
            ("ISLAND/ISLAND.exe", 33),
            ("果つることなき未来より/Hatsumira.exe", 33),
            ("ここから夏のイノセンス！/cs2.exe", 33),
            ("白昼夢の青写真/cs2.exe", 35),
            ("牛顿与苹果树/cs2.exe", 35),
            ("未来ラジオと人工鳩（AUGUST）/cs2.exe", 35),
            ("うたてめぐり/UTATEMEGURI.exe", 32),
            ("カミカゼ☆エクスプローラー!/cs2.exe", 32),
            ("戦国天使ジブリール/Djibril5.exe", 32),
            ("灰色的果实/Grisaia.exe", 32),
            ("灰色的迷宫/Grisaia2.exe", 32),
            ("灰色的乐园v1/Grisaia3.exe", 32),
            ("秘密游戏 反叛/REBELLIONS.exe", 33),
            ("突然之间发现我已恋上你/cs2_cn.exe", 32),
            ("祝福のカンパネラ/cs2.exe", 31),
            ("ツナバン らぶみくす/cs2.exe", 30),
        ] {
            let bytes = fs::read(games.join(name))?;
            let table = OrdinaryNativeTable::from_pe(&bytes);
            let query = table.as_ref().and_then(OrdinaryNativeTable::root_sound_query);
            if query != Some(expected) {
                failures.push(format!("{name}: table={}, expected {expected}, got {query:?}", table.is_some()));
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
        Ok(())
    }
}

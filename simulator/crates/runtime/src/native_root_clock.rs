use crate::pe_resources::Section;

const ROOT_TYPE: &[u8] = b".?AVkcTaskTop@@\0";
const SAMPLE_GETTER: &[u8] = b"\x8b\x41\x24\xc3";
// Both calls in the timer use this unsigned 64-bit divide helper, which pops
// its four DWORD arguments. Its internal branches have fixed displacements.
const DIVIDE_HELPER: &[u8] = b"\x53\x56\x8b\x44\x24\x18\x0b\xc0\x75\x18\x8b\x4c\x24\x14\x8b\x44\x24\x10\x33\xd2\xf7\xf1\x8b\xd8\x8b\x44\x24\x0c\xf7\xf1\x8b\xd3\xeb\x41\x8b\xc8\x8b\x5c\x24\x14\x8b\x54\x24\x10\x8b\x44\x24\x0c\xd1\xe9\xd1\xdb\xd1\xea\xd1\xd8\x0b\xc9\x75\xf4\xf7\xf3\x8b\xf0\xf7\x64\x24\x18\x8b\xc8\x8b\x44\x24\x14\xf7\xe6\x03\xd1\x72\x0e\x3b\x54\x24\x10\x77\x08\x72\x07\x3b\x44\x24\x0c\x76\x01\x4e\x33\xd2\x8b\xc6\x5e\x5b\xc2\x10\x00";

pub(super) fn command<'a>(
    wrapper: &[u8],
    data: &'a [u8],
    sections: &[Section],
    image_base: u32,
    file: impl Fn(u32, bool) -> Option<&'a [u8]>,
) -> Option<u32> {
    let command = wrapper_command(wrapper)?;
    let code = |rva| file(rva, true);
    let handler_rva = primary_command_receiver(data, sections, image_base, ROOT_TYPE, &code)?;
    let field = request_field(handler_rva, command, image_base, &code)?;

    let mut samples = 0;
    let mut consumers = 0;
    for section in sections.iter().filter(|section| section.executable()) {
        let Some(bytes) = section.file_bytes_from(data, section.virtual_address) else {
            continue;
        };
        for (offset, &byte) in bytes.iter().enumerate() {
            if byte != 0xe8 {
                continue;
            }
            let rva = section
                .virtual_address
                .checked_add(u32::try_from(offset).ok()?)?;
            let body = bytes.get(offset..)?;
            if is_sample(bytes, offset, body, rva, field, &code) {
                samples += 1;
            }
            if let Some(timer) = consumer_timer(body, rva, &code) {
                if !timer_holds_elapsed(timer, image_base, &file) {
                    return None;
                }
                consumers += 1;
            }
            if samples > 1 || consumers > 1 {
                return None;
            }
        }
    }
    (samples == 1 && consumers == 1).then_some(u32::from(command))
}

fn wrapper_command(body: &[u8]) -> Option<u8> {
    // The wrapper sends the DWORD as data1 to the root and returns Continue.
    let (prefix, suffix): (&[u8], &[u8]) = if body.starts_with(b"\x8b\x4c\x24\x08\x8d") {
        (
            b"\x8b\x4c\x24\x08\x8d\x44\x24\x08\x50\x8b\x44\x24\x10\x6a\x00\x8b\x11\xff\x30\x6a\x00\x6a",
            b"\x6a\x00\xff\x71\x04\xff\x52\x10\xb8\x02\x00\x00\x00\xc2\x10\x00",
        )
    } else {
        (
            b"\x8b\x4c\x24\x08\x8b\x01\x8b\x40\x10\x8d\x54\x24\x08\x52\x8b\x54\x24\x10\x8b\x12\x6a\x00\x52\x8b\x51\x04\x6a\x00\x6a",
            b"\x6a\x00\x52\xff\xd0\xb8\x02\x00\x00\x00\xc2\x10\x00",
        )
    };
    let (&command, tail) = body.strip_prefix(prefix)?.split_first()?;
    (tail.starts_with(suffix) && (1..=0x7f).contains(&command)).then_some(command)
}

pub(super) fn primary_command_receiver<'a>(
    image: &[u8],
    sections: &[Section],
    image_base: u32,
    type_name: &[u8],
    code: &impl Fn(u32) -> Option<&'a [u8]>,
) -> Option<u32> {
    Some(primary_task_vtable(image, sections, image_base, type_name, code)?[7])
}

pub(super) fn primary_task_vtable<'a>(
    image: &[u8],
    sections: &[Section],
    image_base: u32,
    type_name: &[u8],
    code: &impl Fn(u32) -> Option<&'a [u8]>,
) -> Option<[u32; 8]> {
    primary_task_vtable_location(image, sections, image_base, type_name, code, false)
        .map(|(_, methods)| methods)
}

pub(super) fn primary_task_vtable_location<'a, const N: usize>(
    image: &[u8],
    sections: &[Section],
    image_base: u32,
    type_name: &[u8],
    code: &impl Fn(u32) -> Option<&'a [u8]>,
    trailing_locator: bool,
) -> Option<(u32, [u32; N])> {
    let data_sections = || {
        sections
            .iter()
            .filter(|section| section.is_readable_data())
            .filter_map(|section| {
                let rva = section.virtual_address;
                Some((
                    image_base.checked_add(rva)?,
                    section.file_bytes_from(image, rva)?,
                ))
            })
    };
    let descriptor = unique(data_sections().flat_map(|(va, body)| {
        body.windows(type_name.len())
            .enumerate()
            .filter_map(move |(offset, name)| {
                (name == type_name).then_some(())?;
                va.checked_add(u32::try_from(offset.checked_sub(8)?).ok()?)
            })
    }))?;
    let locator = unique(data_sections().flat_map(|(va, body)| {
        body.chunks_exact(4)
            .enumerate()
            .filter_map(move |(index, word)| {
                let field = index * 4;
                let offset = field.checked_sub(12)?;
                (word == descriptor.to_le_bytes() && body.get(offset..field)? == [0; 12])
                    .then_some(())?;
                va.checked_add(u32::try_from(offset).ok()?)
            })
    }))?;
    // The ordinary MSVC layout puts the locator before the vtable. Some
    // earlier engines put the locator after their seven-method vtable.
    unique(data_sections().flat_map(|(start, body)| {
        body.chunks_exact(4)
            .enumerate()
            .filter_map(move |(index, word)| {
                (word == locator.to_le_bytes()).then_some(())?;
                let first = if trailing_locator { index.checked_sub(N)? } else { index + 1 };
                let table = body.get(first * 4..(first + N) * 4)?;
                let mut methods = [0; N];
                for (body, pointer) in methods.iter_mut().zip(table.chunks_exact(4)) {
                    *body = read_word(pointer, 0)?.checked_sub(image_base)?;
                    code(*body)?;
                }
                Some((start.checked_add(u32::try_from(first * 4).ok()?)?, methods))
            })
    }))
}

fn request_field<'a>(
    rva: u32,
    command: u8,
    image_base: u32,
    code: &impl Fn(u32) -> Option<&'a [u8]>,
) -> Option<u32> {
    let body = code(rva)?;
    // The SEH prologues preserve ECX as ESI and load data1 into EDI or EBX.
    let (dispatch, writer_prefix, writer_suffix): (_, &[u8], &[u8]) =
        if let Some(dispatch) = body.strip_prefix(b"\x8b\x44\x24\x08\x48") {
            (dispatch, b"\x8b\x44\x24\x10\x89\x81", b"\x33\xc0\xc2\x18\x00")
        } else if let Some([_, (_, cookie), (_, cookie_again)]) = operands(body, b"\x6a\xff\x68", [
            b"\x64\xa1\x00\x00\x00\x00\x50\x81\xec\x30\x02\x00\x00\xa1",
            b"\x33\xc4\x89\x84\x24\x2c\x02\x00\x00\x53\x55\x56\x57\xa1",
            b"\x33\xc4\x50\x8d\x84\x24\x44\x02\x00\x00\x64\xa3\x00\x00\x00\x00\x8b\xf1\x8b\x84\x24\x58\x02\x00\x00\x8b\x9c\x24\x68\x02\x00\x00\x48\x8b\xbc\x24\x60\x02\x00\x00\x89\x5c\x24\x20",
        ]) {
            (cookie == cookie_again).then_some(())?;
            (body.get(87..)?, b"\x89\xbe", b"\x33\xc0\x8b\x8c\x24\x44\x02\x00\x00")
        } else {
            let [_, (_, cookie), (_, cookie_again)] = operands(body, b"\x6a\xff\x68", [
                b"\x64\xa1\x00\x00\x00\x00\x50\x81\xec\x44\x02\x00\x00\xa1",
                b"\x33\xc4\x89\x84\x24\x40\x02\x00\x00\x53\x55\x56\x57\xa1",
                b"\x33\xc4\x50\x8d\x84\x24\x58\x02\x00\x00\x64\xa3\x00\x00\x00\x00\x8b\x84\x24\x6c\x02\x00\x00\x8b\xac\x24\x7c\x02\x00\x00\x8b\x9c\x24\x74\x02\x00\x00\x48\x8b\xf1\x89\x6c\x24\x1c",
            ])?;
            (cookie == cookie_again).then_some(())?;
            (body.get(87..)?, b"\x89\x9e", b"\x33\xc0\x8b\x8c\x24\x58\x02\x00\x00")
        };
    let index = command.checked_sub(1)?;
    let after_bound = dispatch.strip_prefix(&[0x83, 0xf8, index][..])?;
    let [(branch, default), (_, map), (_, jump_table)] = operands(
        after_bound,
        b"\x0f\x87",
        [b"\x0f\xb6\x80", b"\xff\x24\x85", b""],
    )?;
    let default = relative_target(rva, body.len() - after_bound.len() + branch, default)?;
    code(default)?;
    let selector = usize::from(*code(map.checked_sub(image_base)?)?.get(usize::from(index))?);
    let writer_rva = read_word(code(jump_table.checked_sub(image_base)?)?, selector * 4)?
        .checked_sub(image_base)?;
    (writer_rva != default).then_some(())?;
    let [(_, field)] = operands(code(writer_rva)?, writer_prefix, [writer_suffix])?;
    (field != 0 && field.is_multiple_of(4)).then_some(field)
}

fn is_sample<'a>(
    bytes: &[u8],
    offset: usize,
    body: &[u8],
    rva: u32,
    field: u32,
    code: &impl Fn(u32) -> Option<&'a [u8]>,
) -> bool {
    // The platform samples the root field after calling the tree updater.
    let before = &bytes[..offset];
    if !before.ends_with(b"\x8b\x01\xff\x74\x24\x0c\xff\x50\x08\x8b\x4e\x08")
        && !before.ends_with(b"\x08\x52\x8b\x54\x24\x0c\x52\xff\xd0\x8b\x4e\x08")
    {
        return false;
    }
    operands(body, b"\xe8", [b"\x89\x46\x24"])
        .and_then(|[(offset, displacement)]| code(relative_target(rva, offset, displacement)?))
        .is_some_and(|getter| {
            getter.starts_with(b"\x8b\x81")
                && read_word(getter, 2) == Some(field)
                && getter.get(6) == Some(&0xc3)
        })
}

fn consumer_timer<'a>(
    body: &[u8],
    rva: u32,
    code: &impl Fn(u32) -> Option<&'a [u8]>,
) -> Option<u32> {
    const STEP_ELAPSED: &[u8] =
        b"\x75\x17\x33\xd2\xb8\x3c\x00\x00\x00\xf7\x76\x04\x01\x46\x28\xeb\x08\xe8";
    // The result of the sample getter is ORed with the platform flag. Both
    // policy paths pass that value to the same timer, which supplies elapsed.
    let (after_getter, before_flag_push, after_flag_check, hold_check): (
        &[u8],
        &[u8],
        &[u8],
        &[u8],
    ) = match body.get(5..7)? {
        b"\x8b\xd8" if body.get(15) == Some(&0x83) => (
            b"\x8b\xd8\x0b\x5e\x50\xeb\x03\x8b\x5e\x50\x83\x7e\x10\x00\x0f\x85",
            b"\x8d\x4c\x24\x1c\x53\x85\xc0\x74\x21\x83\xb8",
            b"\x00\x74\x18\xe8",
            b"\x85\xdb",
        ),
        b"\x8b\x56" => (
            b"\x8b\x56\x10\x8b\xd8\x0b\x5e\x50\xeb\x03\x8b\x5e\x50\x85\xd2\x0f\x85",
            b"\x8d\x4d\xb0\x53\x85\xc0\x74\x20\x39\x90",
            b"\x74\x18\xe8",
            b"\x85\xdb",
        ),
        b"\x8b\xe8" => (
            b"\x8b\xe8\x0b\x6e\x50\xeb\x03\x8b\x6e\x50\x39\x5e\x10\x0f\x85",
            b"\x55\x8d\x4c\x24\x24\x3b\xc3\x74\x20\x39\x98",
            b"\x74\x18\xe8",
            b"\x3b\xeb",
        ),
        b"\x8b\xd8" => (
            b"\x8b\xd8\x0b\x5e\x50\xeb\x03\x8b\x5e\x50\x39\x6e\x10\x0f\x85",
            b"\x53\x8d\x4c\x24\x24\x3b\xc5\x74\x20\x39\xa8",
            b"\x74\x18\xe8",
            b"\x3b\xdd",
        ),
        _ => return None,
    };
    let [(getter_offset, getter), _, _, _, (first_offset, first)] = operands(
        body,
        b"\xe8",
        [
            after_getter,
            b"\xa1",
            before_flag_push,
            after_flag_check,
            b"",
        ],
    )?;
    let after_first = body
        .get(first_offset + 4..)?
        .strip_prefix(hold_check)?
        .strip_prefix(STEP_ELAPSED)?;
    let [(second_offset, second)] = operands(after_first, b"", [b"\x89\x46\x28"])?;
    code(relative_target(rva, getter_offset, getter)?)?
        .starts_with(SAMPLE_GETTER)
        .then_some(())?;
    let timer = relative_target(rva, first_offset, first)?;
    let other = relative_target(rva, body.len() - after_first.len() + second_offset, second)?;
    (timer == other).then_some(timer)
}

fn timer_holds_elapsed<'a>(
    rva: u32,
    image_base: u32,
    file: &impl Fn(u32, bool) -> Option<&'a [u8]>,
) -> bool {
    let Some(body) = file(rva, true) else {
        return false;
    };
    if let Some([first, (_, sleep), (_, time_get_time), (_, get_tick_count), second]) = operands(
        body, b"\x83\xec\x08\x53\x55\x56\x8b\xf1\x57\xff\x76\x2c\xff\x76\x28\xff\x76\x24\xff\x76\x20\xe8", [
            b"\x8b\x1d", b"\x8b\x2d",
            b"\x89\x44\x24\x10\x89\x54\x24\x14\x8b\xff\x6a\x01\xff\xd3\x83\x7e\x04\x00\x74\x04\xff\xd5\xeb\x06\xff\x15",
            b"\xff\x76\x2c\x8b\xf8\xff\x76\x28\x57\x6a\x00\xe8",
            b"\x8b\xc8\x8b\x44\x24\x10\x3b\xc1\x75\x10\x39\x54\x24\x14\x75\x0a\x83\x46\x30\x01\x83\x56\x34\x00\xeb\xc2\x83\x7c\x24\x1c\x00\x75\x14\x2b\xc8\x33\xd2\x8b\xc1\xc1\xe0\x04\x2b\xc1\xc1\xe0\x02\xf7\x76\x08\x01\x46\x14\x8b\x46\x14",
        ],
    ).or_else(|| operands(
        body, b"\x83\xec\x08\x53\x55\x56\x8b\xf1\x57\xff\x76\x2c\xff\x76\x28\xff\x76\x24\xff\x76\x20\xe8", [
            b"\x8b\x2d", b"\x89\x44\x24\x10\x89\x54\x24\x14\x6a\x01\xff\xd5\x83\x7e\x04\x00\x74\x08\xff\x15", b"\xeb\x06\xff\x15",
            b"\xff\x76\x2c\x8b\xf8\xff\x76\x28\x57\x6a\x00\xe8",
            b"\x8b\xd8\x8b\x44\x24\x10\x3b\xc3\x75\x10\x39\x54\x24\x14\x75\x0a\x83\x46\x30\x01\x83\x56\x34\x00\xeb\xbe\x83\x7c\x24\x1c\x00\x8b\x4e\x14\x75\x16\x2b\xd8\x33\xd2\x8b\xc3\xc1\xe0\x04\x2b\xc3\xc1\xe0\x02\xf7\x76\x08\x03\xc8\x89\x4e\x14\x89\x7e\x24",
        ],
    )).or_else(|| operands(
        body, b"\x83\xec\x08\x53\x55\x56\x8b\xf1\x8b\x46\x2c\x8b\x4e\x28\x8b\x56\x24\x57\x50\x8b\x46\x20\x51\x52\x50\xe8", [
            b"\x8b\xe8\x89\x54\x24\x14\x6a\x01\xff\x15", b"\x83\x7e\x04\x00\x74\x08\xff\x15", b"\xeb\x06\xff\x15",
            b"\x8b\x4e\x2c\x8b\x56\x28\x51\x52\x8b\xd8\x53\x33\xff\x57\xe8",
            b"\x3b\xe8\x75\x0f\x39\x54\x24\x14\x75\x09\x83\x46\x30\x01\x11\x7e\x34\xeb\xbe\x83\x7c\x24\x1c\x00\x75\x15\x2b\xc5\x8b\xc8\xc1\xe0\x04\x2b\xc1\x03\xc0\x03\xc0\x33\xd2\xf7\x76\x08\x01\x46\x14\x8b\x46\x14",
        ],
    )) {
        // The fixed short branches skip the elapsed write when entry ESP+4
        // is nonzero. Validate all stack cleanups before using that evidence.
        let helper = relative_target(rva, first.0, first.1);
        return helper == relative_target(rva, second.0, second.1)
            && helper.and_then(|rva| file(rva, true)).is_some_and(|body| body.starts_with(DIVIDE_HELPER))
            && [(sleep, b"Sleep\0".as_slice()), (time_get_time, b"timeGetTime\0"), (get_tick_count, b"GetTickCount\0")]
                .into_iter().all(|(iat, name)| {
                    iat.checked_sub(image_base).and_then(|rva| file(rva, false))
                        .and_then(|slot| file(read_word(slot, 0)?, false))
                        .and_then(|symbol| symbol.get(2..))
                        .is_some_and(|symbol| symbol.starts_with(name))
                });
    }
    false
}

// Each fragment following the prefix comes immediately after one DWORD
// operand. All other bytes are fixed, including intra-function branches.
pub(super) fn operands<const N: usize>(
    body: &[u8],
    prefix: &[u8],
    fragments: [&[u8]; N],
) -> Option<[(usize, u32); N]> {
    let mut tail = body.strip_prefix(prefix)?;
    let mut found = [(0, 0); N];
    for (operand, fragment) in found.iter_mut().zip(fragments) {
        *operand = (body.len() - tail.len(), read_word(tail, 0)?);
        tail = tail.get(4..)?.strip_prefix(fragment)?;
    }
    Some(found)
}

pub(super) fn read_word(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        bytes.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

pub(super) fn relative_target(rva: u32, operand_offset: usize, displacement: u32) -> Option<u32> {
    rva.checked_add(u32::try_from(operand_offset + 4).ok()?)?
        .checked_add_signed(displacement as i32)
}

fn unique<T>(mut values: impl Iterator<Item = T>) -> Option<T> {
    let result = values.next()?;
    values.next().is_none().then_some(result)
}

#[cfg(test)]
mod tests {
    use crate::native_table::OrdinaryNativeTable;
    use crate::pe_resources::parse_sections;
    use std::{fs, io, path::Path};

    fn games() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../games")
    }

    #[test]
    fn native_root_clock_recognizes_registered_clock_families() -> io::Result<()> {
        let games = games();
        let samples = [
            ("cs2_full_v401/system/cs2_open.exe", 38),
            (
                "[170526][NEKO WORKs] ネコぱら vol.3 steam版 (files)/nekopara_vol3/NEKOPARAvol3.exe",
                40,
            ),
            ("ISLAND/ISLAND.exe", 38),
            ("Yukikoi Melt/YukikoiMelt.exe", 40),
            ("ここから夏のイノセンス！/cs2.exe", 38),
            ("未来ラジオと人工鳩（AUGUST）/cs2.exe", 40),
            ("果つることなき未来より/Hatsumira.exe", 38),
            ("灰色的乐园v1/Grisaia3.exe", 37),
            ("灰色的迷宫/Grisaia2.exe", 37),
            ("牛顿与苹果树/cs2.exe", 40),
            ("牛顿与苹果树/牛顿与苹果树.exe", 40),
            ("白昼夢の青写真/cs2.exe", 40),
            // This release was outside the original SHA allowlist.
            ("秘密游戏 反叛/REBELLIONS.exe", 38),
        ];
        for (relative, expected) in samples {
            let path = games.join(relative);
            if !path.is_file() {
                continue;
            }
            let table = OrdinaryNativeTable::from_pe(&fs::read(path)?)
                .unwrap_or_else(|| panic!("missing ordinary table in {relative}"));
            assert_eq!(table.root_clock_command(), Some(expected), "{relative}");
        }
        Ok(())
    }

    #[test]
    fn native_root_clock_rejects_changed_instruction_chain() -> io::Result<()> {
        let game = games().join("ISLAND/ISLAND.exe");
        if !game.is_file() {
            return Ok(());
        }
        let pe = fs::read(game)?;
        let sections = parse_sections(&pe).unwrap();
        let command = |image: &[u8]| {
            OrdinaryNativeTable::from_pe(image)
                .unwrap()
                .root_clock_command()
        };
        assert_eq!(command(&pe), Some(38));
        for (rva, edge) in [
            (1053904 + 22, "wrapper command"),
            (688512 + 2, "receiver store"),
            (679257 + 1, "sample getter call"),
            (677531 + 1, "platform sample getter call"),
            (677603 + 1, "timer call"),
            (678654 + 6, "elapsed gate branch"),
        ] {
            let bytes = sections
                .iter()
                .find_map(|section| section.file_bytes_from(&pe, rva))
                .unwrap();
            let offset = bytes.as_ptr() as usize - pe.as_ptr() as usize;
            let mut changed = pe.clone();
            changed[offset] ^= 1;
            assert_eq!(command(&changed), None, "changed {edge} at {rva:#x}");
        }
        let mut changed = pe;
        changed.push(0x42);
        assert_eq!(command(&changed), Some(38), "appended PE overlay");
        Ok(())
    }
}

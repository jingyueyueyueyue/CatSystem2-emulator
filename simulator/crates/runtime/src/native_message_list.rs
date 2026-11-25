//! Type-47 inspector ABI recovered from registered wrappers and kcTaskMesList.

use super::root_clock::{operands, primary_task_vtable, primary_task_vtable_location, read_word, relative_target};
use crate::pe_resources::Section;

const TYPE_NAME: &[u8] = b".?AVkcTaskMesList@@\0";
const SEND: &[u8] =
    b"\x8b\x4c\x24\x08\x8d\x44\x24\x08\x50\x8b\x44\x24\x10\x6a\x00\x8b\x11\x6a\x00\x6a\x00\x68";
const SEND_RETURN: &[u8] = b"\xff\x30\xff\x71\x04\xff\x52\x10\xb8\x02\x00\x00\x00\xc2\x10\x00";
const EARLY_SEND: &[u8] = b"\x8b\x4c\x24\x08\x8b\x01\x8b\x40\x10\x8d\x54\x24\x08\x52\x8b\x54\x24\x10\x8b\x12\x6a\x00\x6a\x00\x6a\x00\x68";
const EARLY_SEND_RETURN: &[u8] = b"\x52\x8b\x51\x04\x52\xff\xd0\xb8\x02\x00\x00\x00\xc2\x10\x00";
const HANDLED: &[u8] = b"\x33\xc0\x5e\xc2\x18\x00";

pub(super) fn command_base<'a>(
    entries: &[(u32, &'a [u8])],
    image: &[u8],
    sections: &[Section],
    image_base: u32,
    file_bytes: impl Fn(u32, bool) -> Option<&'a [u8]>,
) -> Option<u32> {
    let create = entries.get(570)?.1;
    let mut legacy_send = EARLY_SEND.to_vec();
    legacy_send[8] = 0x0c;
    let legacy = create.starts_with(&legacy_send);
    let early = legacy || create.starts_with(EARLY_SEND);
    let (send, send_return) = if legacy {
        (legacy_send.as_slice(), EARLY_SEND_RETURN)
    } else if early {
        (EARLY_SEND, EARLY_SEND_RETURN)
    } else {
        (SEND, SEND_RETURN)
    };
    let [(_, base)] = operands(create, send, [send_return])?;
    let [(_, refresh)] = operands(entries.get(572)?.1, send, [send_return])?;
    if base == 0
        || refresh.checked_sub(base) != Some(2)
        || bind_command(entries.get(571)?.1, early, legacy)?.checked_sub(base) != Some(3)
    {
        return None;
    }

    let code = |rva| file_bytes(rva, true);
    let (update, receiver) = if legacy {
        let (_, [_, update, _, _, _, _, receiver]) =
            primary_task_vtable_location(image, sections, image_base, TYPE_NAME, &code, false)?;
        (update, receiver)
    } else {
        let [_, _, update, _, _, _, _, receiver] =
            primary_task_vtable(image, sections, image_base, TYPE_NAME, &code)?;
        (update, receiver)
    };
    let [(_, selector), (disp_offset, disp), (_, table)] = operands(
        code(receiver)?,
        b"\x8b\x44\x24\x08\x05",
        [b"\x56\x8b\xf1\x83\xf8\x03\x0f\x87", b"\xff\x24\x85", b""],
    )?;
    if (selector as i32).checked_neg()? as u32 != base {
        return None;
    }
    let default = relative_target(receiver, disp_offset, disp)?;
    code(default)?.strip_prefix(b"\xb8\x01\x00\x00\x00\x5e\xc2\x18\x00")?;
    let table = code(table.checked_sub(image_base)?)?;
    let branch = |offset| code(read_word(table, offset)?.checked_sub(image_base)?);
    let branches = [branch(0)?, branch(4)?, branch(8)?, branch(12)?];
    let (worker, imports, selection) = if early {
        let (worker, imports) = early_receiver(branches, legacy)?;
        let body = code(update)?;
        let selection = if legacy {
            early_selection_command(body, true)?
        } else {
            early_selection_command(body, false)
                .or_else(|| early_selection_with_parent_getters(update, body, &code))?
        };
        (worker, imports, selection)
    } else {
        let [_, _, _, bind] = branches;
        let modern = bind.starts_with(b"\x8b\x44\x24\x14\x89\x46\x78");
        let (worker, imports) = frameless_receiver(branches, modern)?;
        (worker, imports, selection_command(code(update)?, modern)?)
    };
    code(worker.checked_sub(image_base)?)?;
    for import in imports {
        file_bytes(import.checked_sub(image_base)?, false)?;
    }
    (selection.checked_sub(base) == Some(4)).then_some(base)
}

fn early_receiver([open, visible, refresh, bind]: [&[u8]; 4], legacy: bool) -> Option<(u32, [u32; 4])> {
    let (worker_store, wait, window, source) = if legacy {
        (&b"\x83\xc4\x0c\x89\x46\x64\x8b\x46\x74\x6a\xff\x50\xff\x15"[..],
         &b"\x8b\x4e\x74\x51\xff\x15"[..], 0x60, 0x68)
    } else {
        (&b"\x83\xc4\x0c\x89\x46\x70\x8b\x86\x80\x00\x00\x00\x6a\xff\x50\xff\x15"[..],
         &b"\x8b\x8e\x80\x00\x00\x00\x51\xff\x15"[..], 0x6c, 0x74)
    };
    let [(_, worker), _, (_, wait_handle), (_, close_handle)] = operands(
        open,
        b"\x56\x6a\x00\x68",
        [
            b"\xe8",
            worker_store,
            wait,
            HANDLED,
        ],
    )?;
    let [(_, show), (_, hide)] = operands(
        visible,
        &[0x83, 0x7c, 0x24, 0x14, 0, 0x74, 0x12, 0x8b, 0x56, window, 0x6a, 5, 0x52, 0xff, 0x15],
        [
            &[0x33, 0xc0, 0x5e, 0xc2, 0x18, 0, 0x8b, 0x46, window, 0x6a, 0, 0x50, 0xff, 0x15],
            HANDLED,
        ],
    )?;
    let [(_, post)] = operands(
        refresh,
        &[0x8b, 0x4e, window, 0x6a, 0, 0x6a, 0, 0x68, 0x0a, 4, 0, 0, 0x51, 0xff, 0x15],
        [HANDLED],
    )?;
    bind.strip_prefix(&[0x8b, 0x54, 0x24, 0x14, 0x89, 0x56, source][..])?
        .strip_prefix(HANDLED)?;
    (show == hide).then_some((worker, [wait_handle, close_handle, show, post]))
}

fn frameless_receiver(
    [open, visible, refresh, bind]: [&[u8]; 4],
    modern: bool,
) -> Option<(u32, [u32; 4])> {
    // The following four branches create the worker, call ShowWindow with
    // 5 or 0, post its 0x40a refresh, and store data1 as the bound CST object.
    // Older builds place the corresponding receiver fields four bytes earlier.
    let (open_tail, hwnd, show_head, show_hidden, send_refresh, bind_head): (
        _,
        &[u8],
        &[u8],
        &[u8],
        &[u8],
        &[u8],
    ) = if modern {
        (
            b"\x83\xc4\x0c\x89\x46\x74\x6a\xff\xff\xb6\x84\x00\x00\x00\xff\x15".as_slice(),
            b"\xff\xb6\x84\x00\x00\x00\xff\x15",
            b"\x83\x7c\x24\x14\x00\x8b\x46\x70\x74\x0f\x6a\x05\x50\xff\x15",
            b"\x6a\x00\x50\xff\x15",
            b"\xff\x76\x70\xff\x15",
            b"\x8b\x44\x24\x14\x89\x46\x78",
        )
    } else {
        (
            b"\x83\xc4\x0c\x89\x46\x70\x6a\xff\xff\xb6\x80\x00\x00\x00\xff\x15".as_slice(),
            b"\xff\xb6\x80\x00\x00\x00\xff\x15",
            b"\x83\x7c\x24\x14\x00\x74\x11\x6a\x05\xff\x76\x6c\xff\x15",
            b"\x6a\x00\xff\x76\x6c\xff\x15",
            b"\xff\x76\x6c\xff\x15",
            b"\x8b\x44\x24\x14\x89\x46\x74",
        )
    };
    let [(_, worker), (_, _), (_, wait_handle), (_, close_handle)] = operands(
        open,
        b"\x56\x6a\x00\x68",
        [b"\xe8", open_tail, hwnd, HANDLED],
    )?;
    let [(_, show)] = operands(visible, show_head, [HANDLED])?;
    let hidden = visible.get(show_head.len() + 4 + HANDLED.len()..)?;
    let [(_, hide)] = operands(hidden, show_hidden, [HANDLED])?;
    let refresh = refresh.strip_prefix(b"\x6a\x00\x6a\x00\x68\x0a\x04\x00\x00")?;
    let [(_, post)] = operands(refresh, send_refresh, [HANDLED])?;
    bind.strip_prefix(bind_head)?.strip_prefix(HANDLED)?;
    (show == hide).then_some((worker, [wait_handle, close_handle, show, post]))
}

fn bind_command(body: &[u8], early: bool, legacy: bool) -> Option<u32> {
    // Both the CST unwrap path and the null path must send the same command.
    if early {
        let dispatch = if legacy { 0x0c } else { 0x10 };
        let mut binding = *b"\x50\x8b\x46\x10\x52\xff\xd0\x5f\xb8\x02\x00\x00\x00\x5e\xc2\x10\x00\x8b\x11\x8b\x52\x10\x8d\x74\x24\x0c\x56\x6a\x00\x6a\x00\x6a\x00\x68";
        binding[3] = dispatch;
        binding[21] = dispatch;
        let [(_, bind), (_, clear)] = operands(body,
            b"\x8b\x44\x24\x0c\x8b\x50\x04\x8b\x4c\x24\x08\x8b\x00\x56\x85\xd2\x74\x28\x8b\x12\x8b\x31\x57\x8d\x7c\x24\x10\x57\x6a\x00\x52\x8b\x51\x04\x6a\x00\x68",
            [&binding,
             b"\x50\x8b\x41\x04\x50\xff\xd2\xb8\x02\x00\x00\x00\x5e\xc2\x10\x00"],
        )?;
        return (bind == clear).then_some(bind);
    }
    let [(_, bind), (_, clear)] = operands(body,
        b"\x8b\x4c\x24\x08\x8b\x54\x24\x0c\x56\x8b\x01\x8b\x72\x04\x8b\x40\x10\x85\xf6\x74\x22\x57\x8d\x7c\x24\x14\x57\x6a\x00\xff\x36\x6a\x00\x68",
        [b"\xff\x32\xff\x71\x04\xff\xd0\x5f\xb8\x02\x00\x00\x00\x5e\xc2\x10\x00\x8d\x74\x24\x10\x56\x6a\x00\x6a\x00\x6a\x00\x68",
         b"\xff\x32\xff\x71\x04\xff\xd0\xb8\x02\x00\x00\x00\x5e\xc2\x10\x00"],
    ).or_else(|| operands(body,
        b"\x8b\x54\x24\x0c\x8b\x4c\x24\x08\x56\x8b\x72\x04\x8b\x01\x85\xf6\x74\x23\x57\x8d\x7c\x24\x10\x57\x6a\x00\xff\x36\x6a\x00\x68",
        [b"\xff\x32\xff\x71\x04\xff\x50\x10\x5f\xb8\x02\x00\x00\x00\x5e\xc2\x10\x00\x8d\x74\x24\x0c\x56\x6a\x00\x6a\x00\x6a\x00\x68",
         b"\xff\x32\xff\x71\x04\xff\x50\x10\xb8\x02\x00\x00\x00\x5e\xc2\x10\x00"],
    ))?;
    (bind == clear).then_some(bind)
}

fn early_selection_command(body: &[u8], legacy: bool) -> Option<u32> {
    let (field, dispatch) = if legacy { (0x70, 0x0c) } else { (0x7c, 0x10) };
    let mut send = *b"\x85\xc0\x75\x04\x5f\xc2\x08\x00\x8b\x47\x7c\x83\xf8\xff\x74\x2a\x8b\x17\x8b\x52\x10\x8d\x4c\x24\x0c\x51\x6a\x00\x50\x8b\x47\x0c\x8b\x48\x04\x8b\x47\x04\x6a\x00\x68";
    send[10] = field;
    send[20] = dispatch;
    let mut reset = *b"\x51\x50\x8b\xcf\xff\xd2\xc7\x47\x7c\xff\xff\xff\xff\xb8\x01\x00\x00\x00\x5f\xc2\x08\x00";
    reset[8] = field;
    let [_, (_, command)] = operands(body,
        b"\x8b\x44\x24\x08\x57\x50\x8b\x44\x24\x0c\x8b\xf9\xe8",
        [&send, &reset],
    )?;
    Some(command)
}

fn early_selection_with_parent_getters<'a>(
    rva: u32,
    body: &[u8],
    code: &impl Fn(u32) -> Option<&'a [u8]>,
) -> Option<u32> {
    // This form calls the parent and handle getters instead of inlining them.
    let [(gate_offset, gate), (_, command), (parent_offset, parent),
        (target_offset, target), (sender_offset, sender)] = operands(body,
        b"\x8b\x44\x24\x08\x56\x8b\xf1\x8b\x4c\x24\x08\x50\x51\x8b\xce\xe8",
        [b"\x85\xc0\x75\x04\x5e\xc2\x08\x00\x8b\x46\x7c\x83\xf8\xff\x74\x38\x57\x8b\x3e\x8d\x54\x24\x10\x52\x6a\x00\x50\x6a\x00\x68",
         b"\x8b\xce\xe8", b"\x8b\xc8\xe8", b"\x50\x8b\xce\xe8",
         b"\x50\x8b\x47\x10\x8b\xce\xff\xd0\xc7\x46\x7c\xff\xff\xff\xff\x5f\xb8\x01\x00\x00\x00\x5e\xc2\x08\x00"],
    )?;
    code(relative_target(rva, gate_offset, gate)?)?
        .strip_prefix(b"\x55\x8b\xec\x83\xe4\xf8\x8b\x45\x08\x83\xec\x1c\x53\x56\x8b")?;
    code(relative_target(rva, parent_offset, parent)?)?.strip_prefix(b"\x8b\x41\x0c\xc3")?;
    let target_getter = relative_target(rva, target_offset, target)?;
    code(target_getter)?.strip_prefix(b"\x8b\x41\x04\xc3")?;
    (relative_target(rva, sender_offset, sender)? == target_getter).then_some(command)
}

fn selection_command(body: &[u8], modern: bool) -> Option<u32> {
    let (read_index, reset_index): (&[u8], &[u8]) = if modern {
        (
            b"\x8b\x87\x80\x00\x00\x00\x83\xf8\xff\x74\x39",
            b"\x50\x8b\xcf\xff\x56\x10\xc7\x87\x80\x00\x00\x00",
        )
    } else {
        (
            b"\x8b\x47\x7c\x83\xf8\xff\x74\x36",
            b"\x50\x8b\xcf\xff\x56\x10\xc7\x47\x7c",
        )
    };
    let tail = body
        .strip_prefix(b"\x57\xff\x74\x24\x0c\x8b\xf9\xff\x74\x24\x0c\xe8")?
        .get(4..)?
        .strip_prefix(b"\x85\xc0\x75\x04\x5f\xc2\x08\x00")?
        .strip_prefix(read_index)?;
    let [(_, command), _, _, (call_operand, _)] = operands(
        tail,
        b"\x56\x8b\x37\x8d\x4c\x24\x10\x51\x6a\x00\x50\x6a\x00\x68",
        [
            b"\x8b\xcf\xe8",
            b"\x8b\xc8\xe8",
            b"\x50\x8b\xcf\xe8",
            reset_index,
        ],
    )?;
    // The reset stores -1 immediately before returning to the task pump.
    let reset_offset = call_operand + 4 + reset_index.len();
    tail.get(reset_offset..)?
        .strip_prefix(b"\xff\xff\xff\xff\x5e\xb8\x01\x00\x00\x00\x5f\xc2\x08\x00")?;
    Some(command)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_table::OrdinaryNativeTable;
    use std::{fs, io, path::Path};

    const KAMIKAZE_PE: &str = "\u{30ab}\u{30df}\u{30ab}\u{30bc}\u{2606}\u{30a8}\u{30af}\u{30b9}\u{30d7}\u{30ed}\u{30fc}\u{30e9}\u{30fc}!/cs2.exe";

    #[test]
    fn registered_early_message_list_uses_receiver_and_update() -> io::Result<()> {
        let games = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../games");
        let samples = [
            (KAMIKAZE_PE, 404),
            ("\u{7070}\u{8272}\u{7684}\u{679c}\u{5b9e}/Grisaia.exe", 400),
            ("\u{7070}\u{8272}\u{7684}\u{8ff7}\u{5bab}/Grisaia2.exe", 411),
            (
                "\u{7070}\u{8272}\u{7684}\u{8ff7}\u{5bab}/Grisaia2_cn.exe",
                411,
            ),
            (
                "\u{7070}\u{8272}\u{7684}\u{4e50}\u{56ed}v1/Grisaia3.exe",
                418,
            ),
            (
                "\u{7070}\u{8272}\u{7684}\u{4e50}\u{56ed}v1/Grisaia3_cn.exe",
                418,
            ),
            (
                "\u{79d8}\u{5bc6}\u{6e38}\u{620f} \u{53cd}\u{53db}/REBELLIONS.exe",
                423,
            ),
            (
                "\u{6226}\u{56fd}\u{5929}\u{4f7f}\u{30b8}\u{30d6}\u{30ea}\u{30fc}\u{30eb}/Djibril5.exe",
                406,
            ),
            (
                "\u{3046}\u{305f}\u{3066}\u{3081}\u{3050}\u{308a}/UTATEMEGURI.exe",
                383,
            ),
            (
                "\u{7a81}\u{7136}\u{4e4b}\u{95f4}\u{53d1}\u{73b0}\u{6211}\u{5df2}\u{604b}\u{4e0a}\u{4f60}/cs2_cn.exe",
                406,
            ),
        ];
        for (path, base) in samples {
            let file = games.join(path);
            if !file.is_file() {
                continue;
            }
            let mut pe = fs::read(file)?;
            pe.extend_from_slice(b"unrelated PE overlay");
            assert_eq!(
                OrdinaryNativeTable::from_pe(&pe)
                    .unwrap()
                    .message_list_base(),
                Some(base),
                "{path}"
            );
        }
        Ok(())
    }

    #[test]
    fn registered_early_selection_requires_original_parent_and_handle_fields() -> io::Result<()> {
        let game = Path::new(env!("CARGO_MANIFEST_DIR")).join(
            "../../../games/\u{79d8}\u{5bc6}\u{6e38}\u{620f} \u{53cd}\u{53db}/REBELLIONS.exe",
        );
        if !game.is_file() {
            return Ok(());
        }
        let original = fs::read(game)?;
        let recover = |pe: &[u8]| {
            OrdinaryNativeTable::from_pe(pe)
                .unwrap()
                .message_list_base()
        };
        assert_eq!(recover(&original), Some(423));
        for (offset, old, new) in [
            (0x41bb2, 0xab, 0xaa),
            (0xbaed2, 0x0c, 0x04),
            (0x38222, 0x04, 0x0c),
        ] {
            let mut changed = original.clone();
            assert_eq!(changed[offset], old, "{offset:#x}");
            changed[offset] = new;
            assert_eq!(recover(&changed), None, "{offset:#x}");
        }

        // Redirect the sender to the real parent getter while leaving the
        // target getter and the complete dispatch instruction chain intact.
        let mut changed = original;
        let sender_displacement = 0xbbad0_i32 - (0x42780 + 0x48 + 4);
        changed[0x41bc8..0x41bcc].copy_from_slice(&sender_displacement.to_le_bytes());
        assert_eq!(recover(&changed), None);
        Ok(())
    }

    #[test]
    fn registered_early_message_list_rejects_dialogue_and_changed_receiver() -> io::Result<()> {
        let game = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../games")
            .join(KAMIKAZE_PE);
        if !game.is_file() {
            return Ok(());
        }
        let original = fs::read(game)?;
        let recover = |pe: &[u8]| {
            OrdinaryNativeTable::from_pe(pe)
                .unwrap()
                .message_list_base()
        };
        assert_eq!(recover(&original), Some(404));

        // Change the verified CST store and selection command independently.
        for offset in [0x53a7b, 0x539ba] {
            let mut changed = original.clone();
            changed[offset] ^= 1;
            assert_eq!(recover(&changed), None, "{offset:#x}");
        }

        const TABLE: usize = 0x35a0f0;
        let mut changed = original.clone();
        // Kamikaze's dialogue slots have the same send code and +2 spacing.
        for (inspector, dialogue, command) in [(570, 615, 415), (572, 617, 417)] {
            let source = TABLE + 4 * dialogue;
            let rva = (read_word(&original, source).unwrap() - 0x400c00) as usize;
            assert_eq!(
                operands(&original[rva..], EARLY_SEND, [EARLY_SEND_RETURN]).unwrap()[0].1,
                command
            );
            changed.copy_within(source..source + 4, TABLE + 4 * inspector);
        }
        let bind = &mut changed[0x82290..0x822f0];
        let command = [0x68, 0x97, 0x01, 0, 0];
        let offsets: Vec<_> = bind
            .windows(command.len())
            .enumerate()
            .filter_map(|(offset, bytes)| (bytes == command).then_some(offset + 1))
            .collect();
        assert_eq!(offsets.len(), 2);
        for offset in offsets {
            bind[offset] = 0xa2;
        }
        assert_eq!(bind_command(bind, true, false), Some(418));
        assert_eq!(recover(&changed), None);
        Ok(())
    }

    #[test]
    fn registered_message_list_uses_wrapper_receiver_and_update() -> io::Result<()> {
        let games = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../games");
        let samples = [
            ("Happiness！2 樱花盛典/cs2_crack.exe", 448),
            ("Yukikoi Melt/YukikoiMelt.exe", 448),
            ("cs2_full_v401/system/cs2_open.exe", 437),
            ("灰色的迷宫/补丁/原版备份/Grisaia2.exe", 411),
            ("灰色的乐园v1/补丁/原版备份/Grisaia3.exe", 418),
            ("白昼夢の青写真/cs2.exe", 448),
            ("牛顿与苹果树/cs2.exe", 439),
            ("牛顿与苹果树/牛顿与苹果树.exe", 439),
            ("ISLAND/ISLAND.exe", 432),
            ("ISLAND/ISLAND_CHS.exe", 432),
            ("未来ラジオと人工鳩（AUGUST）/cs2.exe", 442),
            ("未来ラジオと人工鳩（AUGUST）/cs2_cn.exe", 442),
            ("祝福のカンパネラ/cs2.exe", 377),
            ("ツナバン らぶみくす/cs2.exe", 343),
            ("ツナバン らぶみくす/cs2_gemini2.5pro.exe", 343),
            ("ここから夏のイノセンス！/cs2.exe", 434),
            ("ここから夏のイノセンス！/cs2_cn.exe", 434),
            ("果つることなき未来より/Hatsumira.exe", 432),
            ("[170526][NEKO WORKs] ネコぱら vol.3 steam版 (files)/nekopara_vol3/NEKOPARAvol3.exe", 442),
            ("突然之间发现我已恋上你/cs2_cn.exe", 406),
        ];
        let mut missing = Vec::new();
        for (path, base) in samples {
            let file = games.join(path);
            if !file.is_file() {
                continue;
            }
            let mut pe = fs::read(file)?;
            pe.extend_from_slice(b"unrelated PE overlay");
            let table = OrdinaryNativeTable::from_pe(&pe).unwrap();
            if table.message_list_base() != Some(base) {
                missing.push(format!("{path}: {:?}, expected {base}", table.message_list_base()));
            }
            if path.starts_with("Happiness") {
                // Separate mutations of the bind wrapper, CST writer, and selection sender.
                for offset in [0x19e520, 0x1791f5, 0x178dc1] {
                    let mut changed = pe.clone();
                    changed[offset] ^= 1;
                    assert_eq!(
                        OrdinaryNativeTable::from_pe(&changed)
                            .unwrap()
                            .message_list_base(),
                        None,
                        "{offset:#x}"
                    );
                }
            }
        }
        assert!(missing.is_empty(), "{}", missing.join("\n"));
        Ok(())
    }

    #[test]
    fn registered_message_list_rejects_identical_dialogue_wrappers() -> io::Result<()> {
        let games = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../games");
        let v401 = games.join("cs2_full_v401/system/cs2_open.exe");
        let happiness =
            games.join("Happiness\u{ff01}2 \u{6a31}\u{82b1}\u{76db}\u{5178}/cs2_crack.exe");
        if !v401.is_file() || !happiness.is_file() {
            return Ok(());
        }
        let mut v401 = fs::read(v401)?;
        let happiness = fs::read(happiness)?;
        const V401_TABLE: usize = 0x73b100;
        const HAPPINESS_TABLE: usize = 0x5e26e8;
        const WRAPPER_LEN: usize = SEND.len() + 4 + SEND_RETURN.len();
        let body_offset = |pe: &[u8], table: usize, slot: usize| {
            (read_word(pe, table + 4 * slot).unwrap() - 0x400c00) as usize
        };
        for (inspector, dialogue) in [(570, 615), (572, 617)] {
            let plain = body_offset(&happiness, HAPPINESS_TABLE, inspector);
            let collision = body_offset(&v401, V401_TABLE, dialogue);
            assert_eq!(
                happiness[plain..plain + WRAPPER_LEN],
                v401[collision..collision + WRAPPER_LEN]
            );
            let source = V401_TABLE + 4 * dialogue;
            v401.copy_within(source..source + 4, V401_TABLE + 4 * inspector);
        }

        // Give the redirected slots an internally consistent tuple. It
        // still differs from the executable's actual kind-47 receiver base.
        let start = body_offset(&v401, V401_TABLE, 571);
        let bind = &mut v401[start..start + 86];
        let original = [0x68, 0xb8, 0x01, 0, 0];
        let offsets: Vec<_> = bind
            .windows(original.len())
            .enumerate()
            .filter_map(|(offset, bytes)| (bytes == original).then_some(offset))
            .collect();
        assert_eq!(offsets.len(), 2);
        for offset in offsets {
            bind[offset + 1] = 0xc3;
        }
        assert_eq!(bind_command(bind, false, false), Some(451));
        assert_eq!(
            OrdinaryNativeTable::from_pe(&v401)
                .unwrap()
                .message_list_base(),
            None
        );
        Ok(())
    }
}

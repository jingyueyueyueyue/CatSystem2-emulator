//! Scene-tool and export commands emitted by the registered ordinary natives.

use super::root_clock::{primary_task_vtable_location, read_word, relative_target};
use crate::pe_resources::Section;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SceneToolCommands {
    pub debug: Option<[u32; 6]>,
    pub list: Option<[u32; 3]>,
    pub export_base: Option<u32>,
}

impl SceneToolCommands {
    pub fn operation(self, kind: u32, command: u32) -> Option<usize> {
        let commands: &[u32] = match kind {
            32 => self.debug.as_ref()?,
            33 => &self.list.as_ref()?[..2],
            _ => return None,
        };
        commands.iter().position(|&candidate| candidate == command)
    }
}

const DEBUG_TYPE: &[u8] = b".?AVkcTaskSceneDebug@@\0";
const LIST_TYPE: &[u8] = b".?AVkcTaskSceneList@@\0";
const EXPORT_TYPE: &[u8] = b".?AVkcTaskZTExport@@\0";
const SOUND_EXPORT_TYPE: &[u8] = b".?AVkcTaskZTExportWinSnd@@\0";
const RETURN_CONTINUE: &[u8] = b"\xb8\x02\0\0\0\xc2\x10\0";

pub(super) fn registered<'a>(
    entries: &[(u32, &'a [u8])],
    image: &[u8],
    sections: &[Section],
    image_base: u32,
    file: impl Fn(u32, bool) -> Option<&'a [u8]>,
) -> SceneToolCommands {
    let code = |rva| file(rva, true);
    let slot = |index: usize| entries.get(index).map(|&(_, body)| body);
    let methods = |name: &[u8], early: bool| {
        if early {
            let (_, methods) = primary_task_vtable_location::<7>(image, sections, image_base, name, &code, false)?;
            Some((methods[1], methods[6]))
        } else {
            let (_, methods) = primary_task_vtable_location::<8>(image, sections, image_base, name, &code, false)?;
            Some((methods[2], methods[7]))
        }
    };

    let debug = (|| {
        let (open, layout) = send_no_value(slot(279)?)?;
        let visible = send_visibility(slot(280)?, layout)?;
        let [record, refresh] = find_send(slot(281)?, 0xb0, RECORD_SENDS)?;
        let [log] = find_send(slot(282)?, 0x88, LOG_SENDS)?;
        let (clear, clear_layout) = send_no_value(slot(283)?)?;
        let commands = [open, visible, record, log, clear, refresh];
        if clear_layout != layout || !consecutive(commands) {
            return None;
        }
        let (_, receiver) = methods(DEBUG_TYPE, layout == SenderLayout::Early)?;
        validate_debug_receiver(receiver, open, image_base, &code)?;
        Some(commands)
    })();

    let list = (|| {
        let (open, layout) = send_no_value(slot(287)?)?;
        let visible = send_visibility(slot(288)?, layout)?;
        let (update, receiver) = methods(LIST_TYPE, layout == SenderLayout::Early)?;
        let [selection] = find_send(code(update)?, 0x52, LIST_SELECTION_SENDS)?;
        if !consecutive([open, visible, selection]) {
            return None;
        }
        let (begin, toggle) = two_way_receiver(receiver, code(receiver)?, open, LIST_RECEIVERS)?;
        if !code(begin)?.starts_with(b"\x56\x6a\0\x68") || !code(toggle)?.starts_with(b"\x83\x7c\x24\x14\0") {
            return None;
        }
        Some([open, visible, selection])
    })();

    let export_base = (|| {
        let [begin] = find_send(slot(690)?, 0x88, EXPORT_SOURCE_SENDS)?;
        let [query] = match_start(slot(691)?, EXPORT_QUERY_SENDS)?;
        let [begin_sound] = find_send(slot(692)?, 0xc0, EXPORT_SOUND_SENDS)?;
        if !consecutive([begin, query]) || begin_sound != begin {
            return None;
        }
        for name in [EXPORT_TYPE, SOUND_EXPORT_TYPE] {
            let (_, receiver) = methods(name, false)?;
            let (start, status) = two_way_receiver(receiver, code(receiver)?, begin, EXPORT_RECEIVERS)?;
            validate_export_receiver(code(start)?, code(status)?)?;
        }
        Some(begin)
    })();

    SceneToolCommands { debug, list, export_base }
}

fn consecutive<const N: usize>(commands: [u32; N]) -> bool {
    commands[0] != 0 && commands.windows(2).all(|pair| pair[0].checked_add(1) == Some(pair[1]))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SenderLayout {
    Modern,
    Older,
    Early,
}

const NONE_SENDS: &[&[u8]] = &[
    b"\x8b\x4c\x24\x08\x8d\x44\x24\x08\x50\x8b\x44\x24\x10\x6a\0\x8b\x11\x6a\0\x6a\0\x68####\xff\x30\xff\x71\x04\xff\x52\x10",
    b"\x8b\x4c\x24\x08\x8b\x01\x8b\x40\x10\x8d\x54\x24\x08\x52\x8b\x54\x24\x10\x8b\x12\x6a\0\x6a\0\x6a\0\x68####\x52\x8b\x51\x04\x52\xff\xd0",
    b"\x8b\x4c\x24\x08\x8b\x01\x8b\x40\x0c\x8d\x54\x24\x08\x52\x8b\x54\x24\x10\x8b\x12\x6a\0\x6a\0\x6a\0\x68####\x52\x8b\x51\x04\x52\xff\xd0",
];
const VISIBLE_SENDS: &[&[u8]] = &[
    b"\x8b\x4c\x24\x08\x8d\x44\x24\x08\x50\x8b\x44\x24\x10\x6a\0\x8b\x11\xff\x70\x04\x6a\0\x68####\xff\x30\xff\x71\x04\xff\x52\x10",
    b"\x8b\x4c\x24\x08\x8b\x11\x8b\x52\x10\x56\x8d\x44\x24\x0c\x50\x8b\x44\x24\x14\x8b\x70\x04\x8b\x00\x6a\0\x56\x6a\0\x68####\x50\x8b\x41\x04\x50\xff\xd2",
    b"\x8b\x4c\x24\x08\x8b\x11\x8b\x52\x0c\x56\x8d\x44\x24\x0c\x50\x8b\x44\x24\x14\x8b\x70\x04\x8b\x00\x6a\0\x56\x6a\0\x68####\x50\x8b\x41\x04\x50\xff\xd2",
];

fn send_no_value(body: &[u8]) -> Option<(u32, SenderLayout)> {
    [SenderLayout::Modern, SenderLayout::Older, SenderLayout::Early]
        .into_iter()
        .zip(NONE_SENDS)
        .find_map(|(layout, pattern)| {
            let [message] = match_operands(body, pattern)?;
            body.get(pattern.len()..)?.strip_prefix(RETURN_CONTINUE)?;
            Some((message, layout))
        })
}

fn send_visibility(body: &[u8], layout: SenderLayout) -> Option<u32> {
    let index = match layout {
        SenderLayout::Modern => 0,
        SenderLayout::Older => 1,
        SenderLayout::Early => 2,
    };
    let pattern = VISIBLE_SENDS[index];
    let [message] = match_operands(body, pattern)?;
    let tail = body.get(pattern.len()..)?;
    if index == 0 {
        tail.strip_prefix(RETURN_CONTINUE)?;
    } else {
        tail.strip_prefix(b"\xb8\x02\0\0\0\x5e\xc2\x10\0")?;
    }
    Some(message)
}

const RECORD_SENDS: &[&[u8]] = &[
    b"\xb9\x58\x00\x00\x00\xf3\xa5\x8b\x7c\x24?\x8d\x4c\x24?\x51\x6a\x00\x8d\x4c\x24\x20\x8b\x07\x51\x6a\x00\x68####\xff\x33\x8b\xcf\xff\x77\x04\xff\x50\x10\x8b\x37\x8d\x44\x24?\x50\x6a\x00\x8b\xcd\xe8????\x50\x6a\x00\x68####\xff\x33\x8b\xcf\xff\x77\x04\xff\x56\x10",
    b"\xb9\x58\x00\x00\x00\x8d\x7c\x24\x20\xf3\xa5\x8b\x74\x24\x1c\x8b\x06\x8d\x4c\x24\x20\x51\x8b\x4b\x04\x6a\x00\x68####\x50\x51\x8b\xcb\xff\xd2\x8b\x13\x8b\x52?\x8d\x44\x24\x0c\x50\x8b\x44\x24\x14\x8b\x48\x08\x8b\x40\x18\x2b\x41\x20\x8b\x0e\x6a\x00\x50\x8b\x43\x04\x6a\x00\x68####\x51\x50\x8b\xcb\xff\xd2",
    b"\xb9\x58\x00\x00\x00\x8d\x7c\x24\x20\xf3\xa5\x8b\x7c\x24\x1c\x8b\x07\x8d\x4c\x24\x20\x51\x8b\x4b\x04\x6a\x00\x68####\x50\x51\x8b\xcb\xff\xd2\x8b\x33\x8b\x4c\x24\x10\x8d\x44\x24\x0c\x50\x6a\x00\x83\xc6\x10\xe8????\x8b\x0f\x8b\x53\x04\x50\x8b\x06\x6a\x00\x68####\x51\x52\x8b\xcb\xff\xd0",
];
const LOG_SENDS: &[&[u8]] = &[
    b"\x8b\x07\x8d\x4c\x24\x14\x83\xc4\x0c\x51\x6a\x00\x8d\x8c\x24\x94\x00\x00\x00\x51\x6a\x00\x68####\xff\x36\x8b\xcf\xff\x77\x04\xff\x50\x10",
    b"\x8b\x07\x8d\x4c\x24\x08\x51\x6a\x00\x8d\x4c\x24\x14\x51\x6a\x00\x68####\xff\x36\x8b\xcf\xff\x77\x04\xff\x50\x10",
    b"\x8b\x16\x8b\x52?\x8d\x44\x24\x0c\x50\x8b\x03\x6a\x00\x8d\x4c\x24\x18\x51\x8b\x4e\x04\x6a\x00\x68####\x50\x51\x8b\xce\xff\xd2",
    b"\x8b\x06\x8b\x40\x10\x8d\x4c\x24\x08\x51\x8b\x0f\x6a\x00\x8d\x54\x24\x14\x52\x8b\x56\x04\x6a\x00\x68####\x51\x52\x8b\xce\xff\xd0",
];
const EXPORT_SOURCE_SENDS: &[&[u8]] = &[
    b"\x8b\x07\x8d\x4c\x24\x08\x51\x6a\x00\x8d\x4c\x24\x14\x51\x6a\x00\x68####\xff\x36\x8b\xcf\xff\x77\x04\xff\x50\x10",
    b"\x8b\x16\x8b\x52\x10\x8d\x44\x24\x0c\x50\x8b\x03\x6a\x00\x8d\x4c\x24\x18\x51\x8b\x4e\x04\x6a\x00\x68####\x50\x51\x8b\xce\xff\xd2",
    b"\x8b\x16\x8b\x52\x10\x8d\x44\x24\x08\x50\x8b\x07\x6a\x00\x8d\x4c\x24\x14\x51\x8b\x4e\x04\x6a\x00\x68####\x50\x51\x8b\xce\xff\xd2",
];
const EXPORT_QUERY_SENDS: &[&[u8]] = &[
    b"\x8b\x4c\x24\x08\x8d\x44\x24\x08\x50\x8b\x44\x24\x10\x6a\x00\x8b\x11\x68????\x6a\x00\x68####\xff\x30\xff\x71\x04\xff\x52\x10\x8b\x4c\x24\x10\x8b\x44\x24\x08\x89\x01\xb8\x01\x00\x00\x00\xc2\x10\x00",
    b"\x8b\x4c\x24\x08\x8b\x01\x8b\x40\x10\x8d\x54\x24\x08\x52\x8b\x54\x24\x10\x8b\x12\x6a\x00\x68????\x6a\x00\x68####\x52\x8b\x51\x04\x52\xff\xd0\x8b\x4c\x24\x10\x8b\x54\x24\x08\x89\x11\xb8\x01\x00\x00\x00\xc2\x10\x00",
];
const EXPORT_SOUND_SENDS: &[&[u8]] = &[
    b"\x8b\x03\x8d\x4c\x24\x0c\x51\x6a\x00\x8d\x4c\x24\x18\x51\x8d\x8c\x24\x20\x01\x00\x00\x51\x68####\xff\x36\x8b\xcb\xff\x73\x04\xff\x50\x10",
    b"\x8b\x17\x8b\x52\x10\x8d\x44\x24\x0c\x50\x6a\x00\x8d\x4c\x24\x18\x51\x8b\x0b\x8d\x84\x24\x20\x01\x00\x00\x50\x8b\x47\x04\x68####\x51\x50\x8b\xcf\xff\xd2",
    b"\x8b\x03\x8d\x4c\x24\x0c\x51\x6a\x00\x8d\x54\x24\x18\x52\x8b\x16\x8d\x8c\x24\x20\x01\x00\x00\x51\x8b\x4b\x04\x68####\x52\x8b\x50\x10\x51\x8b\xcb\xff\xd2",
];

const LIST_SELECTION_SENDS: &[&[u8]] = &[
    b"\x83\xbf?\0\0\0\0\x74\x34\x8b\x4f\x0c\x8d\x44\x24\x0c\x56\x8b\x37\x50\x6a\0\x8d\x87?\0\0\0\x50\x6a\0\x68####\xe8????\x50\xff\x77\x04\x8b\xcf\xff\x56\x10\xc7\x87?\0\0\0\0\0\0\0\x5e\xb8\x01\0\0\0\x5f\xc2\x08\0",
    b"\x83\xbf\x84\0\0\0\0\x74\x33\x8b\x17\x8b\x52\x10\x8d\x44\x24\x0c\x50\x8b\x47\x0c\x6a\0\x8d\x8f\x88\0\0\0\x51\x8b\x48\x04\x8b\x47\x04\x6a\0\x68####\x51\x50\x8b\xcf\xff\xd2\xc7\x87\x84\0\0\0\0\0\0\0\xb8\x01\0\0\0\x5f\xc2\x08\0",
    b"\x83\x7f\x78\0\x74\x2d\x8b\x17\x8b\x52\x0c\x8d\x44\x24\x0c\x50\x8b\x47\x0c\x6a\0\x8d\x4f\x7c\x51\x8b\x48\x04\x8b\x47\x04\x6a\0\x68####\x51\x50\x8b\xcf\xff\xd2\xc7\x47\x78\0\0\0\0\xb8\x01\0\0\0\x5f\xc2\x08\0",
    b"\x83\xbe\x84\0\0\0\0\x74\x37\x8b\x4e\x0c\x57\x8b\x3e\x8d\x54\x24\x10\x52\x6a\0\x8d\x86\x88\0\0\0\x50\x6a\0\x68####\xe8????\x8b\x4e\x04\x8b\x57\x10\x50\x51\x8b\xce\xff\xd2\xc7\x86\x84\0\0\0\0\0\0\0\x5f\xb8\x01\0\0\0\x5e\xc2\x08\0",
];

fn match_start<const N: usize>(body: &[u8], patterns: &[&[u8]]) -> Option<[u32; N]> {
    patterns.iter().find_map(|pattern| match_operands(body, pattern))
}

fn find_send<const N: usize>(body: &[u8], limit: usize, patterns: &[&[u8]]) -> Option<[u32; N]> {
    let mut found = (0..limit.min(body.len())).filter_map(|offset| match_start(&body[offset..], patterns));
    let command = found.next()?;
    found.next().is_none().then_some(command)
}

// The patterns include argument reads and the send call. A question mark
// matches one address/displacement byte and four hashes capture a DWORD.
fn match_operands<const N: usize>(body: &[u8], pattern: &[u8]) -> Option<[u32; N]> {
    let mut values = [0; N];
    let mut count = 0;
    let mut offset = 0;
    let body = body.get(..pattern.len())?;
    while let Some(&byte) = pattern.get(offset) {
        match byte {
            b'#' => {
                pattern.get(offset..offset + 4)?.strip_prefix(b"####")?;
                *values.get_mut(count)? = read_word(body, offset)?;
                count += 1;
                offset += 4;
            }
            b'?' => offset += 1,
            expected if body[offset] == expected => offset += 1,
            _ => return None,
        }
    }
    (count == N).then_some(values)
}

const DEBUG_SWITCHES: &[&[u8]] = &[
    b"\x8b\x44\x24\x08\x53\x05####\x8b\xd9\x56\x83\xf8\x05\x0f\x87####\xff\x24\x85####",
    b"\x8b\x44\x24\x08\x53\x55\x05####\x56\x57\x8b\xe9\x83\xf8\x05\x0f\x87####\xff\x24\x85####",
    b"\x8b\x44\x24\x08\x55\x56\x05####\x57\x8b\xf1\x83\xf8\x05\x0f\x87####\xff\x24\x85####",
];

fn validate_debug_receiver<'a>(
    receiver: u32, base: u32, image_base: u32, code: &impl Fn(u32) -> Option<&'a [u8]>,
) -> Option<()> {
    let body = code(receiver)?;
    let (pattern, [index, default, jumps]) = DEBUG_SWITCHES.iter()
        .find_map(|pattern| Some((*pattern, match_operands(body, pattern)?)))?;
    if index.wrapping_neg() != base {
        return None;
    }
    // The six switch destinations must be inside this receiver. The last four
    // forward the record, log, clear, and VM stack offset to the window.
    let default = relative_target(receiver, pattern.len() - 11, default)?;
    let table = code(jumps.checked_sub(image_base)?)?;
    let mut branches = [0; 6];
    for (index, branch) in branches.iter_mut().enumerate() {
        *branch = read_word(table, index * 4)?.checked_sub(image_base)?;
        if !(receiver + pattern.len() as u32..default).contains(branch) {
            return None;
        }
    }
    let [open, visible, record, log, clear, refresh] = branches;
    let open = code(open)?;
    if ![&b"\x8b\x33"[..], b"\x57\x8b\x3b", b"\x8b\x55\x0c", b"\x8b\x56\x0c", b"\x53\x8b\x1e"]
        .iter().any(|pattern| open.starts_with(pattern))
        || !code(visible)?.starts_with(b"\x83\x7c\x24")
    {
        return None;
    }
    for (branch, operation) in [(record, 0x0d), (log, 0x0e), (clear, 0x0f), (refresh, 0x10)] {
        let body = code(branch)?;
        let prefix = body.get(..body.len().min(21))?;
        let message = [0x6a, 0, 0x68, operation, 4, 0, 0];
        if !prefix.windows(message.len()).any(|bytes| bytes == message) {
            return None;
        }
    }
    Some(())
}

// Receivers normalize [esp+8] with SUB, then dispatch the first two codes
// through two short conditional branches. SEH builds save the first argument
// in a different stack slot before the same dispatch.
const LIST_RECEIVERS: &[&[u8]] = &[
    b"\x8b\x44\x24\x08\x56\x8b\xf1\x2d####\x74?\x48\x74?\xb8\x01\0\0\0\x5e\xc2\x18\0",
    b"\x8b\x44\x24\x08\x56\x8b\xf1\x2d####\x74?\x83\xe8\x01\x74?\xb8\x01\0\0\0\x5e\xc2\x18\0",
    b"\x8b\x44\x24\x08\x2d####\x56\x8b\xf1\x74?\x83\xe8\x01\x74?\xb8\x01\0\0\0\x5e\xc2\x18\0",
];
const EXPORT_RECEIVERS: &[&[u8]] = &[
    b"\x6a\xff\x68????\x64\xa1\0\0\0\0\x50\x56\x57\xa1????\x33\xc4\x50\x8d\x44\x24\x0c\x64\xa3\0\0\0\0\x8b\xf1\x8b\x44\x24\x20\x2d####\x74?\x48\x74?\xb8\x01\0\0\0",
    b"\x55\x8b\xec\x6a\xff\x68????\x64\xa1\0\0\0\0\x50\x56\x57\xa1????\x33\xc5\x50\x8d\x45\xf4\x64\xa3\0\0\0\0\x8b\xf1\x8b\x45\x0c\x2d####\x74?\x83\xe8\x01\x74?\xb8\x01\0\0\0",
    b"\x8b\x44\x24\x08\x56\x8b\xf1\x57\x2d####\x74?\x48\x74?\x5f\xb8\x01\0\0\0\x5e\xc2\x18\0",
    b"\x8b\x44\x24\x08\x2d####\x53\x56\x57\x8b\xf9\x74?\x83\xe8\x01\x74?\x5f\x5e\xb8\x01\0\0\0\x5b\xc2\x18\0",
    b"\x8b\x44\x24\x08\x2d####\x56\x57\x8b?\x74?\x83\xe8\x01\x74?\x5f\xb8\x01\0\0\0\x5e\xc2\x18\0",
];

fn two_way_receiver(
    rva: u32, body: &[u8], base: u32, patterns: &[&[u8]],
) -> Option<(u32, u32)> {
    let pattern = patterns.iter().find(|pattern| match_operands(body, pattern) == Some([base]))?;
    let mut offsets = pattern.windows(2).enumerate()
        .filter_map(|(offset, bytes)| (bytes == b"\x74?").then_some(offset + 1));
    let target = |offset: usize| {
        rva.checked_add((offset + 1) as u32)?.checked_add_signed(i32::from(body[offset] as i8))
    };
    Some((target(offsets.next()?)?, target(offsets.next()?)?))
}

fn validate_export_receiver(start: &[u8], status: &[u8]) -> Option<()> {
    // Begin reads the source pointer (data1). Status writes the boolean
    // result through the command's final argument before it returns.
    let source_loads: &[&[u8]] = &[
        b"\x8b\x4c\x24\x1c", b"\x8b\x54\x24\x1c", b"\x8b\x44\x24\x1c",
        b"\x8b\x4c\x24\x24", b"\x8b\x54\x24\x24", b"\x8b\x44\x24\x24",
        b"\x8b\x45\x14", b"\x8b\x4d\x14",
        b"\x8b\x44\x24\x18", b"\x8b\x4c\x24\x18",
        b"\x8b\x4c\x24\x20", b"\x8b\x54\x24\x20", b"\x8b\x44\x24\x20",
        b"\x8b\x4c\x24\x28", b"\x8b\x54\x24\x28", b"\x8b\x44\x24\x28",
    ];
    let source = source_loads.iter().any(|bytes| start.starts_with(bytes));
    let output = [&b"\xc7\x07\x01\0\0\0"[..], b"\xc7\x06\x01\0\0\0"]
        .iter()
        .any(|bytes| status.get(..status.len().min(20))
            .is_some_and(|head| head.windows(bytes.len()).any(|window| window == *bytes)));
    (source && output).then_some(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_table::OrdinaryNativeTable;
    use std::{fs, io, path::Path};

    #[test]
    fn recognized_scene_and_export_commands_require_the_registered_receiver() -> io::Result<()> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../games/ISLAND/ISLAND.exe");
        if !path.is_file() {
            return Ok(());
        }
        let mut bytes = fs::read(path)?;
        let (base, _) = crate::pe_resources::x86_image_layout(&bytes).unwrap();
        let sections = crate::pe_resources::parse_sections(&bytes).unwrap();
        let code = |rva| sections.iter().find_map(|section| {
            section.executable().then(|| section.file_bytes_from(&bytes, rva))?
        });
        let offset = |name| {
            let (_, methods) = primary_task_vtable_location::<8>(&bytes, &sections, base, name, &code, false).unwrap();
            let receiver = code(methods[7]).unwrap();
            receiver.as_ptr() as usize - bytes.as_ptr() as usize
        };
        let debug = offset(DEBUG_TYPE);
        let list = offset(LIST_TYPE);
        let sound_export = offset(SOUND_EXPORT_TYPE);
        let table = OrdinaryNativeTable::from_pe(&bytes).unwrap();
        assert_eq!(table.scene_tool_commands().debug.unwrap()[0], 312);
        assert_eq!(table.scene_tool_commands().list.unwrap()[0], 318);
        assert_eq!(table.scene_tool_commands().export_base, Some(476));

        bytes[debug + 6] ^= 1;
        bytes[list + 8] ^= 1;
        bytes[sound_export + 9] ^= 1;
        let table = OrdinaryNativeTable::from_pe(&bytes).unwrap();
        assert_eq!(table.scene_tool_commands(), SceneToolCommands::default());
        Ok(())
    }

    #[test]
    fn recognized_scene_and_export_commands_match_installed_wrappers() -> io::Result<()> {
        let games = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../games");
        let mut failures = Vec::new();
        for (file, debug, list, export) in [
            ("[170526][NEKO WORKs] \u{30cd}\u{30b3}\u{3071}\u{3089} vol.3 steam\u{7248} (files)/nekopara_vol3/NEKOPARAvol3.exe", 322, 328, 507),
            ("cs2_full_v401/system/cs2_open.exe", 317, 323, 481),
            ("Happiness\u{ff01}2 \u{6a31}\u{82b1}\u{76db}\u{5178}/cs2_crack.exe", 328, 334, 516),
            ("ISLAND/ISLAND.exe", 312, 318, 476),
            ("ISLAND/ISLAND_CHS.exe", 312, 318, 476),
            ("Yukikoi Melt/YukikoiMelt.exe", 328, 334, 516),
            ("\u{3046}\u{305f}\u{3066}\u{3081}\u{3050}\u{308a}/UTATEMEGURI.exe", 285, 291, 403),
            ("\u{3053}\u{3053}\u{304b}\u{3089}\u{590f}\u{306e}\u{30a4}\u{30ce}\u{30bb}\u{30f3}\u{30b9}\u{ff01}/cs2.exe", 314, 320, 478),
            ("\u{3053}\u{3053}\u{304b}\u{3089}\u{590f}\u{306e}\u{30a4}\u{30ce}\u{30bb}\u{30f3}\u{30b9}\u{ff01}/cs2_cn.exe", 314, 320, 478),
            ("\u{30ab}\u{30df}\u{30ab}\u{30bc}\u{2606}\u{30a8}\u{30af}\u{30b9}\u{30d7}\u{30ed}\u{30fc}\u{30e9}\u{30fc}!/cs2.exe", 293, 299, 446),
            ("\u{30c4}\u{30ca}\u{30d0}\u{30f3} \u{3089}\u{3076}\u{307f}\u{304f}\u{3059}/cs2.exe", 273, 279, 0),
            ("\u{30c4}\u{30ca}\u{30d0}\u{30f3} \u{3089}\u{3076}\u{307f}\u{304f}\u{3059}/cs2_gemini2.5pro.exe", 273, 279, 0),
            ("\u{6226}\u{56fd}\u{5929}\u{4f7f}\u{30b8}\u{30d6}\u{30ea}\u{30fc}\u{30eb}/Djibril5.exe", 295, 301, 448),
            ("\u{672a}\u{6765}\u{30e9}\u{30b8}\u{30aa}\u{3068}\u{4eba}\u{5de5}\u{9ce9}\u{ff08}AUGUST\u{ff09}/cs2.exe", 322, 328, 507),
            ("\u{672a}\u{6765}\u{30e9}\u{30b8}\u{30aa}\u{3068}\u{4eba}\u{5de5}\u{9ce9}\u{ff08}AUGUST\u{ff09}/cs2_cn.exe", 322, 328, 507),
            ("\u{679c}\u{3064}\u{308b}\u{3053}\u{3068}\u{306a}\u{304d}\u{672a}\u{6765}\u{3088}\u{308a}/Hatsumira.exe", 312, 318, 476),
            ("\u{7070}\u{8272}\u{7684}\u{4e50}\u{56ed}v1/Grisaia3.exe", 305, 311, 461),
            ("\u{7070}\u{8272}\u{7684}\u{4e50}\u{56ed}v1/Grisaia3_cn.exe", 305, 311, 461),
            ("\u{7070}\u{8272}\u{7684}\u{4e50}\u{56ed}v1/\u{8865}\u{4e01}/\u{539f}\u{7248}\u{5907}\u{4efd}/Grisaia3.exe", 305, 311, 461),
            ("\u{7070}\u{8272}\u{7684}\u{679c}\u{5b9e}/Grisaia.exe", 293, 299, 438),
            ("\u{7070}\u{8272}\u{7684}\u{8ff7}\u{5bab}/Grisaia2.exe", 300, 306, 453),
            ("\u{7070}\u{8272}\u{7684}\u{8ff7}\u{5bab}/Grisaia2_cn.exe", 300, 306, 453),
            ("\u{7070}\u{8272}\u{7684}\u{8ff7}\u{5bab}/\u{8865}\u{4e01}/\u{539f}\u{7248}\u{5907}\u{4efd}/Grisaia2.exe", 300, 306, 453),
            ("\u{725b}\u{987f}\u{4e0e}\u{82f9}\u{679c}\u{6811}/cs2.exe", 319, 325, 483),
            ("\u{725b}\u{987f}\u{4e0e}\u{82f9}\u{679c}\u{6811}/\u{725b}\u{987f}\u{4e0e}\u{82f9}\u{679c}\u{6811}.exe", 319, 325, 483),
            ("\u{767d}\u{663c}\u{5922}\u{306e}\u{9752}\u{5199}\u{771f}/cs2.exe", 328, 334, 516),
            ("\u{795d}\u{798f}\u{306e}\u{30ab}\u{30f3}\u{30d1}\u{30cd}\u{30e9}/cs2.exe", 279, 285, 0),
            ("\u{79d8}\u{5bc6}\u{6e38}\u{620f} \u{53cd}\u{53db}/REBELLIONS.exe", 310, 316, 466),
            ("\u{7a81}\u{7136}\u{4e4b}\u{95f4}\u{53d1}\u{73b0}\u{6211}\u{5df2}\u{604b}\u{4e0a}\u{4f60}/cs2_cn.exe", 295, 301, 448),
        ] {
            let path = games.join(file);
            if !path.is_file() {
                continue;
            }
            let mut bytes = fs::read(path)?;
            bytes.extend_from_slice(b"unrelated overlay");
            let commands = OrdinaryNativeTable::from_pe(&bytes).map(|table| table.scene_tool_commands());
            let expected = SceneToolCommands {
                debug: Some(std::array::from_fn(|index| debug + index as u32)),
                list: Some([list, list + 1, list + 2]),
                export_base: (export != 0).then_some(export),
            };
            if commands != Some(expected) {
                failures.push(format!("{file}: {commands:?} != {expected:?}"));
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
        Ok(())
    }
}


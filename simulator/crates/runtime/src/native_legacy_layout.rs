//! Argument layouts used by the older list and motion wrappers.

use super::root_clock::{read_word, relative_target};

// The wrapper zeroes a 40-byte record, resolves the nullable image wrapper,
// copies the affine fields, clamps opacity, and forwards the record by message.
const AFFINE_WRAPPER: &[u8] = b"\x55\x8b\xec\x83\xe4\xf8\x83\xec\x38\xd9\xee\x56\x8b\x75\x10\
    \xd9\x54\x24\x1c\x57\xd9\x54\x24\x1c\x8b\x7e\x08\xd9\x54\x24\x28\xd9\x54\x24\x24\
    \x8d\x44\x24\x18\x6a\x28\xd9\x54\x24\x38\x50\xd9\x5c\x24\x38\xe8????\x83\xc4\x08\
    \x83\x7e\x08\x00\x74\x08\x8b\x0f\x89\x4c\x24\x18\xeb\x08\xc7\x44\x24\x18\x00\x00\x00\x00\
    \xd9\x46\x0c\x8b\x46\x28\xd9\x5c\x24\x10\xc7\x44\x24\x38\xff\x00\x00\x00\xd9\x46\x10\
    \xd9\x5c\x24\x14\xd9\x44\x24\x10\xd9\x5c\x24\x1c\xd9\x44\x24\x14\xd9\x5c\x24\x20\
    \xd9\x46\x14\xd9\x5c\x24\x10\xd9\x46\x18\xd9\x5c\x24\x14\xd9\x44\x24\x10\xd9\x5c\x24\x24\
    \xd9\x44\x24\x14\xd9\x5c\x24\x28\xd9\x46\x1c\xd9\x5c\x24\x2c\xd9\x46\x20\xd9\x5c\x24\x10\
    \xd9\x46\x24\xd9\x5c\x24\x14\xd9\x44\x24\x10\xd9\x5c\x24\x30\xd9\x44\x24\x14\xd9\x5c\x24\x34\
    \x3d\xff\x00\x00\x00\x73\x04\x89\x44\x24\x38\x8b\x56\x2c\x8b\x4d\x0c\x8b\x01\x8b\x40?\
    \x89\x54\x24\x3c\x8d\x54\x24\x0c\x52\x6a\x00\x8d\x54\x24\x20\x52\x8b\x56\x04\x52\x8b\x16\
    \x68????\x52\x8b\x51\x04\x52\xff\xd0\x5f\xb8\x02\x00\x00\x00\x5e\x8b\xe5\x5d\xc2\x10\x00";

pub(super) fn list_affine(
    index: usize,
    rva: u32,
    body: &[u8],
    in_code: impl Fn(u32) -> bool,
) -> Option<()> {
    if !matches_at(body, 0, AFFINE_WRAPPER) || !in_code(call_target(rva, body, 0x32)?) {
        return None;
    }
    let command = read_word(body, 0xe9)?;
    matches!(
        (index, body[0xd1], command),
        (626, 0x0c, 0x107 | 0x10b) | (627, 0x0c, 0x108 | 0x10c)
            | (626, 0x10, 0x111) | (627, 0x10, 0x112)
    ).then_some(())
}

const SETTER_START: &[u8] = b"\x55\x8b\xec\x83\xe4\xf8\x81\xec\xac\x00\x00\x00\x53\x56\x57\x8b\x7d\x10\x8b\x07\x89\x44\x24\x20\x85\xc0";
const GETTER_START: &[u8] = b"\x55\x8b\xec\x83\xe4\xf8\x81\xec\xb4\x00\x00\x00\x53\x56\x8b\x75\x10\x8b\x06\x57\x89\x44\x24\x14\x85\xc0";
const SETTER_LAST_GROUP: &[u8] = b"\x8b\x47\x1c\x50\x56\x89\x54\x24\x18\xe8????\x8b\x4f\x20\x51\x56\x89\x44\x24\x20\xe8";
const GETTER_LAST_GROUP: &[u8] = b"\x8b\x46\x1c\x50\x57\xe8????\x8b\x4e\x20\x51\x57\x89\x44\x24\x28\xe8";
const GETTER_LAST_WRITE: &[u8] = b"\x8b\x94\x24\xb0\x00\x00\x00\x8b\x44\x24\x28\x89\x10\x8b\x8c\x24\xb4\x00\x00\x00\x89\x48\x04\x8b\x94\x24\xb8\x00\x00\x00\x89\x50\x08";
const RETURN: &[u8] = b"\x5f\x5e\xb8\x02\x00\x00\x00\x5b\x8b\xe5\x5d\xc2\x10\x00";

pub(super) fn short_motion(
    index: usize,
    rva: u32,
    body: &[u8],
    in_code: impl Fn(u32) -> bool,
) -> Option<()> {
    // The setters and getters copy 0x8c bytes, leaving the ninth group out.
    // The field offsets (4 or 0x90) distinguish base from current properties.
    let (start, last_offset, last_group, resolver_call, copy_offset, copy, end) = match index {
        299 => (SETTER_START, 0x6a, SETTER_LAST_GROUP, 0x81, 0x13c,
            &b"\x8b\x4c\x24\x20\x68\x8c\x00\x00\x00\x8d\x44\x24\x2c\x50\x83\xc1\x04\x51\x89\x94\x24\xbc\x00\x00\x00"[..], 0x15d),
        300 => (GETTER_START, 0x75, GETTER_LAST_GROUP, 0x88, 0x8d,
            &b"\x8b\x54\x24\x14\x68\x8c\x00\x00\x00\x83\xc2\x04\x89\x44\x24\x2c\x52\x8d\x44\x24\x38\x50"[..], 0x16e),
        301 => (SETTER_START, 0x6a, SETTER_LAST_GROUP, 0x81, 0x13c,
            &b"\x8b\x4c\x24\x20\x68\x8c\x00\x00\x00\x8d\x44\x24\x2c\x50\x81\xc1\x90\x00\x00\x00\x51\x89\x94\x24\xbc\x00\x00\x00"[..], 0x160),
        302 => (GETTER_START, 0x75, GETTER_LAST_GROUP, 0x88, 0x8d,
            &b"\x8b\x54\x24\x14\x68\x8c\x00\x00\x00\x81\xc2\x90\x00\x00\x00\x89\x44\x24\x2c\x52\x8d\x44\x24\x38\x50"[..], 0x171),
        _ => return None,
    };
    if !matches_at(body, 0, start) || !matches_at(body, last_offset, last_group)
        || !matches_at(body, copy_offset, copy) || !matches_at(body, end, RETURN)
        || (matches!(index, 300 | 302) && !matches_at(body, end - GETTER_LAST_WRITE.len(), GETTER_LAST_WRITE))
    {
        return None;
    }
    let resolver = call_target(rva, body, resolver_call)?;
    let copier = call_target(rva, body, copy_offset + copy.len())?;
    (resolver != copier && in_code(resolver) && in_code(copier)
        && call_target(rva, body, 0x28) == Some(resolver)).then_some(())
}

fn call_target(rva: u32, body: &[u8], opcode: usize) -> Option<u32> {
    if body.get(opcode) != Some(&0xe8) {
        return None;
    }
    let operand = opcode + 1;
    relative_target(rva, operand, read_word(body, operand)?)
}

fn matches_at(body: &[u8], offset: usize, pattern: &[u8]) -> bool {
    body.get(offset..offset + pattern.len()).is_some_and(|bytes| {
        bytes.iter().zip(pattern).all(|(&byte, &expected)| expected == b'?' || byte == expected)
    })
}

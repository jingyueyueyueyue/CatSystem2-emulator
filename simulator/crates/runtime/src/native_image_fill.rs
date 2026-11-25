//! Recognize registered ordinary slot 736 and its image and canvas callees.

struct ImageFillWrapper {
    wrapper: &'static [u8],
    fill: &'static [u8],
    canvas_gate: &'static [u8],
    tile_prefix: &'static [u8],
    fill_call: usize,
    gate_call: usize,
    clip_call: usize,
    tile_call: usize,
    kernel_table: usize,
    kernel_call: usize,
    bgr24_opaque: (usize, &'static [u8]),
    bgr24_partial: (usize, &'static [u8]),
    bgra32: (usize, &'static [u8]),
}

// The displacement inside a relative call or absolute table operand varies
// by link layout. Each ? matches one such byte. The long canvas field
// encoding also carries its independently observed displacement.
const MODERN_WRAPPER: &[u8] =
    b"\x56\x8b\x74\x24\x10\x8b\x0e\x85\xc9\x74\x2b\xe8????\x8b\xc8\xe8???\
          ?\xff\x76\x14\x8b\xc8\xff\x76\x10\xff\x76\x0c\xff\x76\x08\xff\x76\x04\xe8???\
          ?\xb8\x02\x00\x00\x00\x5e\xc2\x10\x00\xff\x76\x10\x8b\x4c\x24\x10\xff\x76\x0c\xff\x76\
          \x08\x8b\x49\x74\xff\x76\x04\xe8????\xb8\x02\x00\x00\x00\x5e\xc2\x10\x00";
const MODERN_FILL: &[u8] =
    b"\x83\xec\x28\x56\x8d\x44\x24\x04\x8b\xf1\x6a\x28\x50\xe8????\x8b\x46\x04\x89\
          \x44\x24\x18\x8b\x46\x08\x89\x44\x24\x1c\x8b\x44\x24\x38\x89\x44\x24\x20\x8b\x44\x24\x3c\
          \x89\x44\x24\x24\x8b\x46\x0c\x89\x44\x24\x10\x8b\x44\x24\x40\x89\x44\x24\x28\x8b\x44\x24\
          \x44\x89\x44\x24\x2c\x8b\x44\x24\x48\x89\x44\x24\x30\x8b\x46\x18\x89\x44\x24\x0c\x8d\x44\
          \x24\x0c\x50\xe8????\x83\xc4\x0c\xb8\x01\x00\x00\x00\x5e\x83\xc4\x28\xc2\x14\
          \x00";
const MODERN_CANVAS_GATE: &[u8] =
    b"\x56\x33\xf6\x39\x71\x34\x75\x06\x33\xc0\x5e\xc2\x10\x00\x39\x31\x75\x2a\x39\x71\x04\x74\
          \x25\x8b\x49\x08\xe8????\x85\xc0\x74\x19\xff\x74\x24\x14\x8b\xc8\xff\x74\x24\
          \x14\xff\x74\x24\x14\xff\x74\x24\x14\xe8????\x8b\xf0\x8b\xc6\x5e\xc2\x10\x00";
const MODERN_TILE_PREFIX: &[u8] =
    b"\x83\xec\x7c\x53\x56\x57\x8b\x3d????\x8d\x44\x24\x14\x50\x8b\xf1\xff\xd7\x8d\
          \x44\x24\x24\x50\xff\xd7\x8d\x44\x24\x34\x50\xff\xd7";
const MODERN_LONG_FIELD_WRAPPER: &[u8] =
    b"\x56\x8b\x74\x24\x10\x8b\x0e\x85\xc9\x74\x2b\xe8????\x8b\xc8\xe8???\
          ?\xff\x76\x14\x8b\xc8\xff\x76\x10\xff\x76\x0c\xff\x76\x08\xff\x76\x04\xe8???\
          ?\xb8\x02\x00\x00\x00\x5e\xc2\x10\x00\xff\x76\x10\x8b\x4c\x24\x10\xff\x76\x0c\xff\x76\
          \x08\x8b\x89?\x00\x00\x00\xff\x76\x04\xe8????\xb8\x02\x00\x00\x00\x5e\xc2\
          \x10\x00";
const LEGACY_WRAPPER: &[u8] =
    b"\x8b\x44\x24\x0c\x8b\x08\x85\xc9\x74\x26\x8b\x50\x14\x8b\x49\x0c\x52\x8b\x50\x10\x52\x8b\
          \x50\x0c\x52\x8b\x50\x08\x8b\x40\x04\x52\x50\x8b\x01\xe8????\xb8\x02\x00\x00\
          \x00\xc2\x10\x00\x8b\x50\x10\x8b\x48\x0c\x52\x8b\x50\x08\x51\x52\x8b\x50\x04\x8b\x44\x24\
          \x14\x8b\x40\x74\xe8????\xb8\x02\x00\x00\x00\xc2\x10\x00";
const LEGACY_FILL: &[u8] =
    b"\x55\x8b\xec\x83\xe4\xf8\x83\xec\x2c\x56\x8b\xf0\x8d\x44\x24\x08\x6a\x28\x50\xe8??\
          ??\x8b\x4e\x04\x8b\x56\x08\x8b\x45\x08\x89\x4c\x24\x1c\x8b\x4d\x0c\x89\x54\x24\x20\
          \x8b\x56\x0c\x89\x44\x24\x24\x8b\x45\x10\x89\x4c\x24\x28\x8b\x4d\x14\x89\x54\x24\x14\x8b\
          \x55\x18\x89\x44\x24\x2c\x8b\x46\x18\x83\xc4\x08\x8d\x74\x24\x08\x89\x4c\x24\x28\x89\x54\
          \x24\x2c\x89\x44\x24\x08\xe8????\xb8\x01\x00\x00\x00\x5e\x8b\xe5\x5d\xc2\x14\
          \x00";
const LEGACY_CANVAS_GATE: &[u8] =
    b"\x33\xc9\x57\x39\x48\x34\x75\x06\x33\xc0\x5f\xc2\x0c\x00\x39\x08\x75\x29\x39\x48\x04\x74\
          \x24\x8b\x40\x08\xe8????\x8b\xf8\x85\xff\x74\x16\x8b\x44\x24\x10\x8b\x4c\x24\
          \x0c\x50\x8b\x44\x24\x0c\x51\x52\xe8????\x8b\xc8\x8b\xc1\x5f\xc2\x0c\x00";
const LEGACY_TILE_PREFIX: &[u8] =
    b"\x55\x8b\xec\x83\xe4\xf8\x81\xec\x88\x00\x00\x00\x53\x56\x8b\x35????\x8b\xd8\
          \x8d\x44\x24\x10\x50\xff\xd6\x8d\x4c\x24\x2c\x51\xff\xd6\x8d\x54\x24\x3c\x52\xff\xd6";
const INTERMEDIATE_WRAPPER: &[u8] =
    b"\x56\x8b\x74\x24\x10\x8b\x0e\x85\xc9\x74\x30\xe8????\x8b\xc8\xe8???\
          ?\x8b\x4e\x14\x8b\x56\x10\x51\x8b\x4e\x0c\x52\x8b\x56\x08\x51\x8b\x4e\x04\x52\x51\x8b\
          \xc8\xe8????\xb8\x02\x00\x00\x00\x5e\xc2\x10\x00\x8b\x56\x10\x8b\x46\x0c\x8b\
          \x4e\x08\x52\x8b\x56\x04\x50\x8b\x44\x24\x14\x51\x8b\x48\x74\x52\xe8????\xb8\
          \x02\x00\x00\x00\x5e\xc2\x10\x00";
const INTERMEDIATE_FILL: &[u8] =
    b"\x83\xec\x28\x56\x8d\x44\x24\x04\x6a\x28\x50\x8b\xf1\xe8????\x8b\x4e\x04\x8b\
          \x56\x08\x8b\x44\x24\x38\x89\x4c\x24\x18\x8b\x4c\x24\x3c\x89\x4c\x24\x24\x8b\x4c\x24\x44\
          \x89\x54\x24\x1c\x8b\x56\x0c\x89\x44\x24\x20\x8b\x44\x24\x40\x89\x4c\x24\x2c\x89\x54\x24\
          \x10\x8b\x54\x24\x48\x89\x44\x24\x28\x8b\x46\x18\x8d\x4c\x24\x0c\x51\x89\x54\x24\x34\x89\
          \x44\x24\x10\xe8????\x83\xc4\x0c\xb8\x01\x00\x00\x00\x5e\x83\xc4\x28\xc2\x14\
          \x00";
const INTERMEDIATE_CANVAS_GATE: &[u8] =
    b"\x56\x33\xf6\x39\x71\x34\x75\x06\x33\xc0\x5e\xc2\x10\x00\x39\x31\x75\x2e\x39\x71\x04\x74\
          \x29\x8b\x49\x08\xe8????\x85\xc0\x74\x1d\x8b\x4c\x24\x14\x8b\x54\x24\x10\x51\
          \x8b\x4c\x24\x10\x52\x8b\x54\x24\x10\x51\x52\x8b\xc8\xe8????\x8b\xf0\x8b\xc6\
          \x5e\xc2\x10\x00";
const INTERMEDIATE_TILE_PREFIX: &[u8] =
    b"\x81\xec\x80\x00\x00\x00\x56\x57\x8b\x3d????\x8d\x44\x24\x14\x50\x8b\xf1\xff\
          \xd7\x8d\x4c\x24\x24\x51\xff\xd7\x8d\x54\x24\x34\x52\xff\xd7";

// The modern and older kernels use different x86 encodings. Both implement
// the same BGR24 channel arithmetic and BGRA32 whole-DWORD assignment.
const MODERN_BGR24_OPAQUE: (usize, &[u8]) = (
    0xa0,
    b"\x8b\x55?\x8d\x40\x03\x88\x50\xfc\x41\x8b\x55\xe8\x88\x50\xfd\x88\x58\xfe",
);
const MODERN_BGR24_PARTIAL: (usize, &[u8]) = (
    0x10a,
    b"\x65\x8b\x7d\xf4\x0f\xb6\xd3\x8b\x5d\xe0\x0f\xb6\xc9\x0f\xb6\xc0\x89\x55\x08\x89\x4d\xf0\
          \x89\x45\xec\x0f\xb6\x47\xff\x8d\x7f\x03\x8b\x4d\x08\x43\x2b\xc8\x0f\xb6\x47\xfd\x0f\xaf\
          \x4d\x08\xc1\xf9\x08\x00\x4f\xfc\x8b\x4d\xf0\x2b\xc8\x0f\xb6\x47\xfe\x0f\xaf\x4d\xf0\xc1\
          \xf9\x08\x00\x4f\xfd\x8b\x4d\xec\x2b\xc8\x0f\xaf\x4d\xec\xc1\xf9\x08\x00\x4f\xfe",
);
const MODERN_BGRA32: (usize, &[u8]) = (
    0x90,
    b"\x8b\x46\x24\x8d\x49\x04\x89\x41\xfc\x42\x8b\x46\x1c\x3b\xd0\x7c\xef",
);
const LEGACY_BGR24_OPAQUE: (usize, &[u8]) = (
    0xa0,
    b"\x0f\xb6\x4c\x24\x13\x88\x48\xff\x0f\xb6\x4c\x24\x30\x88\x08\x88\x58\x01",
);
const LEGACY_BGR24_PARTIAL: (usize, &[u8]) = (
    0x10c,
    b"\x0f\xb6\x6c\x24\x30\x0f\xb6\xd2\x0f\xb6\xfb\x89\x54\x24\x18\xeb\x03\x8d\x49\x00\x8a\x48\
          \xff\x0f\xb6\xd1\x8b\xdf\x2b\xda\x0f\xaf\xdf\xc1\xfb\x08\x02\xd9\x8a\x08\x88\x58\xff\x0f\
          \xb6\xd1\x8b\xdd\x2b\xda\x0f\xaf\xdd\xc1\xfb\x08\x02\xd9\x8a\x48\x01\x88\x18\x8b\x5c\x24\
          \x18\x0f\xb6\xd1\x2b\xda\x0f\xaf\x5c\x24\x18\x8b\x54\x24\x1c\xc1\xfb\x08\x02\xd9\x88\x58\
          \x01",
);
const LEGACY_BGRA32: (usize, &[u8]) = (
    0x78,
    b"\x8b\x56\x24\x89\x11\x8b\x56\x1c\x40\x83\xc1\x04\x3b\xc2\x7c\xf0",
);

const WRAPPERS: [ImageFillWrapper; 4] = [
    ImageFillWrapper {
        wrapper: MODERN_WRAPPER,
        fill: MODERN_FILL,
        canvas_gate: MODERN_CANVAS_GATE,
        tile_prefix: MODERN_TILE_PREFIX,
        fill_call: 0x28,
        gate_call: 0x49,
        clip_call: 0x5b,
        tile_call: 0x35,
        kernel_table: 0x3e,
        kernel_call: 0xe2,
        bgr24_opaque: MODERN_BGR24_OPAQUE,
        bgr24_partial: MODERN_BGR24_PARTIAL,
        bgra32: MODERN_BGRA32,
    },
    ImageFillWrapper {
        wrapper: MODERN_LONG_FIELD_WRAPPER,
        fill: MODERN_FILL,
        canvas_gate: MODERN_CANVAS_GATE,
        tile_prefix: MODERN_TILE_PREFIX,
        fill_call: 0x28,
        gate_call: 0x4c,
        clip_call: 0x5b,
        tile_call: 0x35,
        kernel_table: 0x3e,
        kernel_call: 0xe2,
        bgr24_opaque: MODERN_BGR24_OPAQUE,
        bgr24_partial: MODERN_BGR24_PARTIAL,
        bgra32: MODERN_BGRA32,
    },
    ImageFillWrapper {
        wrapper: LEGACY_WRAPPER,
        fill: LEGACY_FILL,
        canvas_gate: LEGACY_CANVAS_GATE,
        tile_prefix: LEGACY_TILE_PREFIX,
        fill_call: 0x23,
        gate_call: 0x46,
        clip_call: 0x5e,
        tile_call: 0x34,
        kernel_table: 0x39,
        kernel_call: 0xf9,
        bgr24_opaque: LEGACY_BGR24_OPAQUE,
        bgr24_partial: LEGACY_BGR24_PARTIAL,
        bgra32: LEGACY_BGRA32,
    },
    ImageFillWrapper {
        wrapper: INTERMEDIATE_WRAPPER,
        fill: INTERMEDIATE_FILL,
        canvas_gate: INTERMEDIATE_CANVAS_GATE,
        tile_prefix: INTERMEDIATE_TILE_PREFIX,
        fill_call: 0x2d,
        gate_call: 0x52,
        clip_call: 0x5b,
        tile_call: 0x39,
        kernel_table: 0x3e,
        kernel_call: 0xff,
        bgr24_opaque: LEGACY_BGR24_OPAQUE,
        bgr24_partial: LEGACY_BGR24_PARTIAL,
        bgra32: LEGACY_BGRA32,
    },
];

pub(super) fn is_current_pixels_fill<'a>(
    entry_rva: u32,
    entry: &[u8],
    image_base: u32,
    file_bytes_at: impl Fn(u32, bool) -> Option<&'a [u8]>,
) -> bool {
    let Some(wrapper) = WRAPPERS
        .iter()
        .find(|variant| matches_at(entry, 0, variant.wrapper))
    else {
        return false;
    };
    if wrapper.wrapper == MODERN_LONG_FIELD_WRAPPER && !matches!(entry[0x45], 0xb4 | 0xb8) {
        return false;
    }
    // The two accessor calls in some versions must read the current wrapper
    // and its pixel pointer. The older wrapper performs both loads inline.
    if wrapper.fill_call != 0x23
        && !(direct_call(entry_rva, entry, 0x0b)
            .and_then(|rva| file_bytes_at(rva, true))
            .is_some_and(|body| body.starts_with(b"\x8b\x41\x0c\xc3"))
            && direct_call(entry_rva, entry, 0x12)
                .and_then(|rva| file_bytes_at(rva, true))
                .is_some_and(|body| body.starts_with(b"\x8b\x01\xc3")))
    {
        return false;
    }
    let at_call = |rva, bytes, offset| {
        let target = direct_call(rva, bytes, offset)?;
        Some((target, file_bytes_at(target, true)?))
    };
    let Some((fill_rva, fill)) = at_call(entry_rva, entry, wrapper.fill_call) else {
        return false;
    };
    let Some((gate_rva, gate)) = at_call(entry_rva, entry, wrapper.gate_call) else {
        return false;
    };
    if !matches_at(fill, 0, wrapper.fill) || !matches_at(gate, 0, wrapper.canvas_gate) {
        return false;
    }
    let Some((_, tile)) = at_call(gate_rva, gate, wrapper.tile_call) else {
        return false;
    };
    if !matches_at(tile, 0, wrapper.tile_prefix) {
        return false;
    }
    let Some((_, clip)) = at_call(fill_rva, fill, wrapper.clip_call) else {
        return false;
    };
    // mov edi,[eax*4+table]; the record selects the kernel by pixel format.
    if !matches_at(clip, wrapper.kernel_table, b"\x8b\x3c\x85")
        || !matches_at(clip, wrapper.kernel_call, b"\xff\xd7")
    {
        return false;
    }
    let Some(table) = dword_at(clip, wrapper.kernel_table + 3)
        .and_then(|va| va.checked_sub(image_base))
        .and_then(|rva| file_bytes_at(rva, false))
    else {
        return false;
    };
    let kernel_at = |offset| {
        let rva = dword_at(table, offset)?.checked_sub(image_base)?;
        file_bytes_at(rva, true)
    };
    kernel_at(12).is_some_and(|bgr24| {
        matches_at(bgr24, wrapper.bgr24_opaque.0, wrapper.bgr24_opaque.1)
            && matches_at(bgr24, wrapper.bgr24_partial.0, wrapper.bgr24_partial.1)
    }) && kernel_at(16).is_some_and(|bgra32| matches_at(bgra32, wrapper.bgra32.0, wrapper.bgra32.1))
}

fn dword_at(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        bytes.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

fn direct_call(rva: u32, code: &[u8], offset: usize) -> Option<u32> {
    if *code.get(offset)? != 0xe8 {
        return None;
    }
    let next_rva = rva.checked_add(u32::try_from(offset).ok()?.checked_add(5)?)?;
    next_rva.checked_add_signed(dword_at(code, offset + 1)? as i32)
}

fn matches_at(code: &[u8], offset: usize, pattern: &[u8]) -> bool {
    code.get(offset..offset + pattern.len())
        .is_some_and(|bytes| {
            pattern
                .iter()
                .zip(bytes)
                .all(|(&expected, &actual)| expected == b'?' || expected == actual)
        })
}

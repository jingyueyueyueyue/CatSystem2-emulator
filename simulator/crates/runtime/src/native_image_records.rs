//! Image record and legacy pointer contracts in the registered 703/704 slots.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageRecordLayout {
    Cp932,
    Utf16,
}

impl ImageRecordLayout {
    pub const fn stride(self) -> usize {
        match self {
            Self::Cp932 => 0x48,
            Self::Utf16 => 0x88,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageRecordLayers {
    NarrowStaticRuns,
    NarrowPrefixReadAsUtf16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageProducer703 {
    SelectionPointer,
    Records {
        layout: ImageRecordLayout,
        layers: Option<ImageRecordLayers>,
    },
}

#[derive(Debug, Default)]
pub(super) struct ImageRecords {
    pub(super) producer: Option<ImageProducer703>,
    pub(super) release: Option<ImageRecordLayout>,
}

// Both arms pack coordinates and send a message through vtable slot +0x10.
const SELECTION_POINTER: &[u8] =
    b"\x51\x56\x8b\x74\x24\x10\x8b\x06\x8b\x56\x04\x8b\x40\x10\x57\x8d\x4c\x24\x14\x51\x6a\
            \x00\x6a\x00\x6a\x00\x6a\x1f\x6a\x00\x52\x8b\xce\xff\xd0\x8b\x4c\x24\x18\x0f\xb7\x41\
            \x04\x0f\xb7\x11\xc1\xe0\x10\x0b\xc2\x8b\x16\x83\x79\x0c\x00\x8b\x52\x10\x8d\x7c\x24\
            \x08\x57\x6a\x00\x50\x8b\x41\x08\x8b\x4c\x24\x20\x50\x8b\x46\x04\x74\x16\x68\x81\x00\
            \x00\x00\x51\x50\x8b\xce\xff\xd2\x5f\xb8\x02\x00\x00\x00\x5e\x59\xc2\x10\x00\x68\x80\
            \x00\x00\x00\x51\x50\x8b\xce\xff\xd2\x5f\xb8\x02\x00\x00\x00\x5e\x59\xc2\x10\x00";

struct Release {
    layout: ImageRecordLayout,
    header: &'static [u8],
    tail: &'static [u8],
    resolve: usize,
}

// The complete loop reads tags 1 and 2, calls distinct releases, clears
// the live handle and tag, and advances to the next record by the stride.
const RELEASE_OBJECTS: &[u8] =
    b"\x75\x1e\x8b\x0e\x85\xc9\x74\x18\xe8????\xeb\x0b\x8b\x0e\x85\xc9\x74\x0b\xe8????\xc7\
            \x06\x00\x00\x00\x00\xc7\x46\xfc\x00\x00\x00\x00\x47";
const RELEASE: [Release; 4] = [
    Release {
        layout: ImageRecordLayout::Cp932,
        header: b"\x8b\x4c\x24\x04\x53\x8b\x5c\x24\x10\xff\x33\xe8????\x85\xc0\x74\x45\x57\x33\xff\x39\
            \x7b\x04\x76\x3c\x56\x8d\x70\x04\x8b\x46\xfc\x48\x74\x10\x48",
        tail: b"\x83\xc6\x48\x3b\x7b\x04\x72\xc9\x5e\x5f\xb8\x02\x00\x00\x00\x5b\xc2\x10\x00",
        resolve: 0x0b,
    },
    Release {
        layout: ImageRecordLayout::Utf16,
        header: b"\x8b\x4c\x24\x04\x53\x8b\x5c\x24\x10\xff\x33\xe8????\x85\xc0\x74\x48\x57\x33\xff\x39\
            \x7b\x04\x76\x3f\x56\x8d\x70\x04\x8b\x46\xfc\x48\x74\x10\x48",
        tail: b"\x81\xc6\x88\x00\x00\x00\x3b\x7b\x04\x72\xc6\x5e\x5f\xb8\x02\x00\x00\x00\x5b\xc2\x10\x00",
        resolve: 0x0b,
    },
    Release {
        layout: ImageRecordLayout::Cp932,
        header: b"\x8b\x4c\x24\x04\x53\x8b\x5c\x24\x10\x8b\x03\x50\xe8????\x85\xc0\x74\x49\x57\x33\xff\
            \x39\x7b\x04\x76\x40\x56\x8d\x70\x04\x8b\x46\xfc\x83\xe8\x01\x74\x12\x83\xe8\x01",
        tail: b"\x83\xc6\x48\x3b\x7b\x04\x72\xc5\x5e\x5f\xb8\x02\x00\x00\x00\x5b\xc2\x10\x00",
        resolve: 0x0c,
    },
    Release {
        layout: ImageRecordLayout::Utf16,
        header: b"\x8b\x4c\x24\x04\x53\x8b\x5c\x24\x10\xff\x33\xe8????\x85\xc0\x74\x5f\x57\x33\xff\x39\
            \x7b\x04\x76\x4d\x56\x8d\x70\x04\x8b\x46\xfc\x83\xe8\x01\x74\x12\x83\xe8\x01",
        tail: b"\x81\xc6\x88\x00\x00\x00\x3b\x7b\x04\x72\xc2\x5e\x5f\xb8\x02\x00\x00\x00\x5b\xc2\x10\
            \x00\x5f\xb8\x02\x00\x00\x00\x5b\xc2\x10\x00\xb8\x02\x00\x00\x00\x5b\xc2\x10\x00",
        resolve: 0x0b,
    },
];

struct Producer {
    wrapper: &'static [u8],
    resolve: usize,
    produce: usize,
    consumers: &'static [ProducerBody],
}

struct ProducerBody {
    layout: ImageRecordLayout,
    clear_loop: (usize, &'static [u8]),
    layers: for<'a> fn(&[u8], u32, u32, &PeLookup<'a, '_>) -> Option<ImageRecordLayers>,
}

type PeLookup<'a, 'f> = dyn Fn(u32, bool) -> Option<&'a [u8]> + 'f;

const PRODUCERS: [Producer; 4] = [
    Producer {
        wrapper:
            b"\x81\xec\x08\x01\x00\x00\xa1????\x33\xc4\x89\x84\x24\x04\x01\x00\x00\x53\x8b\x9c\x24\
            \x1c\x01\x00\x00\x8d\x44\x24\x04\x55\x8b\xac\x24\x18\x01\x00\x00\x56\x8b\xb4\x24\x20\
            \x01\x00\x00\x57\x8b\xbc\x24\x1c\x01\x00\x00\x8b\xcf\x50\xc7\x03\x00\x00\x00\x00\xff\
            \x36\xe8????\x85\xc0\x74\x3f\xff\x76\x04\x8b\xcf\xe8????\x8b\xf8\x85\xff\x74\x2f\x8b\
            \x0d????\x8d\x44\x24\x10\x68????\x50\x68????\xe8????\x8b\x4d\x74\x6a\x00\xff\x76\x08\
            \x57\x68????\xe8????\x89\x03\x8b\x8c\x24\x14\x01\x00\x00\xb8\x01\x00\x00\x00\x5f\x5e\
            \x5d\x5b\x33\xcc\xe8????\x81\xc4\x08\x01\x00\x00\xc2\x10\x00",
        resolve: 0x51,
        produce: 0x84,
        consumers: &NARROW,
    },
    Producer {
        wrapper:
            b"\x81\xec\x08\x01\x00\x00\xa1????\x33\xc4\x89\x84\x24\x04\x01\x00\x00\x53\x8b\x9c\x24\
            \x1c\x01\x00\x00\x55\x8b\xac\x24\x18\x01\x00\x00\x56\x8b\xb4\x24\x20\x01\x00\x00\x57\
            \x8b\xbc\x24\x1c\x01\x00\x00\x8d\x44\x24\x10\xc7\x03\x00\x00\x00\x00\x8b\x0e\x50\x51\
            \x8b\xcf\xe8????\x85\xc0\x74\x41\x8b\x56\x04\x52\x8b\xcf\xe8????\x8b\xf8\x85\xff\x74\
            \x30\x8b\x0d????\x68????\x8d\x44\x24\x14\x50\x68????\xe8????\x8b\x4e\x08\x6a\x00\x51\
            \x8b\x4d\x74\x57\x68????\xe8????\x89\x03\x8b\x8c\x24\x14\x01\x00\x00\x5f\x5e\x5d\x5b\
            \x33\xcc\xb8\x01\x00\x00\x00\xe8????\x81\xc4\x08\x01\x00\x00\xc2\x10\x00",
        resolve: 0x53,
        produce: 0x87,
        consumers: &OLDER_NARROW,
    },
    Producer {
        wrapper:
            b"\x81\xec\x08\x09\x00\x00\xa1????\x33\xc4\x89\x84\x24\x04\x09\x00\x00\x53\x8b\x9c\x24\
            \x1c\x09\x00\x00\x8d\x44\x24\x04\x55\x8b\xac\x24\x18\x09\x00\x00\x56\x8b\xb4\x24\x20\
            \x09\x00\x00\x57\x8b\xbc\x24\x1c\x09\x00\x00\x8b\xcf\x50\xc7\x03\x00\x00\x00\x00\xff\
            \x36\xe8????\x85\xc0\x74\x57\xff\x76\x04\x8b\xcf\xe8????\x8b\xf8\x85\xff\x74\x47\x8b\
            \x0d????\x8d\x44\x24\x10\x68????\x50\x68????\xe8????\x8d\x84\x24\x14\x01\x00\x00\x68\
            ????\x50\xe8????\x8b\x4d\x74\x8d\x84\x24\x1c\x01\x00\x00\x83\xc4\x08\x6a\x00\xff\x76\
            \x08\x57\x50\xe8????\x89\x03\x8b\x8c\x24\x14\x09\x00\x00\xb8\x01\x00\x00\x00\x5f\x5e\
            \x5d\x5b\x33\xcc\xe8????\x81\xc4\x08\x09\x00\x00\xc2\x10\x00",
        resolve: 0x51,
        produce: 0x9c,
        consumers: &WIDE,
    },
    Producer {
        wrapper:
            b"\x81\xec\x08\x09\x00\x00\xa1????\x33\xc4\x89\x84\x24\x04\x09\x00\x00\x53\x8b\x9c\x24\
            \x1c\x09\x00\x00\x8d\x44\x24\x04\x55\x8b\xac\x24\x18\x09\x00\x00\x56\x8b\xb4\x24\x20\
            \x09\x00\x00\x57\x8b\xbc\x24\x1c\x09\x00\x00\x8b\xcf\x50\xc7\x03\x00\x00\x00\x00\xff\
            \x36\xe8????\x85\xc0\x74\x5a\xff\x76\x04\x8b\xcf\xe8????\x8b\xf8\x85\xff\x74\x4a\x8b\
            \x0d????\x8d\x44\x24\x10\x68????\x50\x68????\xe8????\x8d\x84\x24\x14\x01\x00\x00\x68\
            ????\x50\xe8????\x8b\x8d?\x00\x00\x00\x8d\x84\x24\x1c\x01\x00\x00\x83\xc4\x08\x6a\x00\
            \xff\x76\x08\x57\x50\xe8????\x89\x03\x8b\x8c\x24\x14\x09\x00\x00\xb8\x01\x00\x00\x00\
            \x5f\x5e\x5d\x5b\x33\xcc\xe8????\x81\xc4\x08\x09\x00\x00\xc2\x10\x00",
        resolve: 0x51,
        produce: 0x9f,
        consumers: &WIDE,
    },
];

const NARROW: [ProducerBody; 1] = [ProducerBody {
    layout: ImageRecordLayout::Cp932,
    clear_loop: (
        0x50,
        b"\xc7\x00\x00\x00\x00\x00\x8d\x40\x48\xc7\x40\xbc\x00\x00\x00\x00\x49\x75\xed",
    ),
    layers: narrow_layers,
}];
const OLDER_NARROW: [ProducerBody; 1] = [ProducerBody {
    layout: ImageRecordLayout::Cp932,
    clear_loop: (
        0x42,
        b"\x89\x18\x89\x58\x04\x83\xc0\x48\x83\xe9\x01\x75\xf3",
    ),
    layers: older_narrow_layers,
}];
const WIDE: [ProducerBody; 2] = [
    ProducerBody {
        layout: ImageRecordLayout::Utf16,
        clear_loop: (0x63, b"\xc7\x00\x00\x00\x00\x00\x8d\x80\x88\x00\x00\x00\xc7\x80\x7c\xff\xff\xff\x00\x00\x00\x00\x49\x75\xe7"),
        layers: wide_layers,
    },
    ProducerBody {
        layout: ImageRecordLayout::Utf16,
        clear_loop: (0x60, b"\xc7\x00\x00\x00\x00\x00\x8d\x80\x88\x00\x00\x00\xc7\x80\x7c\xff\xff\xff\x00\x00\x00\x00\x83\xe9\x01\x75\xe5"),
        layers: older_wide_layers,
    },
];

fn matches_at(body: &[u8], at: usize, pattern: &[u8]) -> bool {
    body.get(at..at + pattern.len()).is_some_and(|found| {
        found
            .iter()
            .zip(pattern)
            .all(|(&byte, &expected)| expected == b'?' || byte == expected)
    })
}

fn word(body: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(body.get(at..at + 4)?.try_into().ok()?))
}

fn target(body: &[u8], rva: u32, call: usize) -> Option<u32> {
    if body.get(call) != Some(&0xe8) {
        return None;
    }
    rva.checked_add(u32::try_from(call + 5).ok()?)?
        .checked_add_signed(word(body, call + 1)? as i32)
}

fn data_at<'a>(
    body: &[u8],
    operand: usize,
    image_base: u32,
    file: &PeLookup<'a, '_>,
) -> Option<&'a [u8]> {
    file(word(body, operand)?.checked_sub(image_base)?, false)
}

fn import_name_is(
    body: &[u8],
    operand: usize,
    image_base: u32,
    file: &PeLookup<'_, '_>,
    name: &[u8],
) -> bool {
    // In the file the IAT slot still contains the RVA of the hint/name record.
    data_at(body, operand, image_base, file)
        .and_then(|thunk| file(word(thunk, 0)?, false))
        .and_then(|symbol| symbol.get(2..))
        .is_some_and(|symbol| symbol.starts_with(name))
}

fn narrow_selector(body: &[u8], legacy: bool) -> bool {
    // The first token is copied as bytes through the first single-byte NUL.
    // Each later token contributes only its sign-extended first byte.
    if legacy {
        matches_at(body, 0x03d, b"\xc6\x06\x00\x50\xc7\x03\x00\x00\x00\x00")
            && matches_at(
                body,
                0x077,
                b"\x8b\xd6\x2b\xd0\xeb\x03\x8d\x49\x00\x8a\x08\x88\x0c\x02\x40\x84\xc9\x75\xf6",
            )
            && matches_at(
                body,
                0x0a1,
                b"\x0f\xbe\x00\x46\x8d\x4c\x24\x0c\x56\x89\x07\x51\x83\xc7\x04",
            )
    } else {
        matches_at(body, 0x02e, b"\xc6\x06\x00\x50\xc7\x03\x00\x00\x00\x00")
            && matches_at(
                body,
                0x065,
                b"\x2b\xf1\x8a\x01\x8d\x49\x01\x88\x44\x0e\xff\x84\xc0\x75\xf3",
            )
            && matches_at(body, 0x090, b"\x0f\xbe\x00\x8d\x7f\x04\x89\x47\xfc\x46")
    }
}

fn narrow_layers(
    body: &[u8],
    rva: u32,
    image_base: u32,
    file: &PeLookup<'_, '_>,
) -> Option<ImageRecordLayers> {
    let selector = file(target(body, rva, 0x0cc)?, true)?;
    // Verify the physical prefix, the animation arm's extra dot/extension,
    // the earlier-animation gate and name-tail copy, and the direct exit.
    (narrow_selector(selector, false)
        && data_at(body, 0x203, image_base, file)?.starts_with(b"%s_\0")
        && data_at(body, 0x2ec, image_base, file)?.starts_with(b".\0")
        && matches_at(
            body,
            0x2ce,
            b"\x0f\x84\x82\x01\x00\x00\x8d\xbc\x24\x3c\x01\x00\x00",
        )
        && matches_at(
            body,
            0x2ea,
            b"\x66\xa1????\x8d\x94\x24\x2c\x01\x00\x00\x66\x89\x07\x8b\xf2",
        )
        && matches_at(
            body,
            0x457,
            b"\xbc\x24\x3c\x02\x00\x00\x4f\x85\xdb\x0f\x84\xfa\x00\x00\x00",
        )
        && matches_at(
            body,
            0x4c5,
            b"\x89\x45\x04\x85\xc0\x74\x33\x8b\x7c\x24\x1c\x8d\x8c\x24\x3c\x01\x00\x00\xc7\x45\x00\
            \x01\x00\x00\x00\x8b\xff\x8a\x01\x8d\x49\x01\x88\x44\x0f\xff\x84\xc0\x75\xf3",
        )
        && matches_at(
            body,
            0x5bb,
            b"\x3b\x74\x24\x24\x0f\x8c\x1b\xfc\xff\xff\x8b\x44\x24\x14\x8b\x8c\x24\x40\x05\x00\x00\
            \x5f\x5e\x5d\x5b\x33\xcc\xe8????\x81\xc4\x34\x05\x00\x00\xc2\x10\x00",
        ))
    .then_some(ImageRecordLayers::NarrowStaticRuns)
}

fn older_narrow_layers(
    body: &[u8],
    rva: u32,
    image_base: u32,
    file: &PeLookup<'_, '_>,
) -> Option<ImageRecordLayers> {
    let selector = file(target(body, rva, 0x0b3)?, true)?;
    (narrow_selector(selector, true)
        && data_at(body, 0x1dc, image_base, file)?.starts_with(b"%s_\0")
        && data_at(body, 0x2cb, image_base, file)?.starts_with(b".\0")
        && matches_at(
            body,
            0x2c8,
            b"\x66\x8b\x0d????\x8d\x84\x24\x30\x01\x00\x00\x66\x89\x0f\x8b\xd0",
        )
        && matches_at(body, 0x434, b"\x83\x7c\x24\x28\x00\x0f\x84\x01\x01\x00\x00")
        && matches_at(
            body,
            0x4a2,
            b"\x89\x46\x04\x85\xc0\x74\x33\x8b\x54\x24\x20\xc7\x06\x01\x00\x00\x00\x8d\x84\x24\x40\
            \x01\x00\x00\x8d\x9b\x00\x00\x00\x00\x8a\x08\x88\x0c\x02\x40\x84\xc9\x75\xf6",
        )
        && matches_at(
            body,
            0x58f,
            b"\x40\x3b\x44\x24\x24\x89\x44\x24\x10\x0f\x8c\x17\xfc\xff\xff\x8b\x44\x24\x14\x8b\x8c\
            \x24\x44\x05\x00\x00\x5f\x5e\x5d\x5b\x33\xcc\xe8????\x81\xc4\x38\x05\x00\x00\xc2\x10\
            \x00",
        ))
    .then_some(ImageRecordLayers::NarrowStaticRuns)
}

fn wide_layers(
    body: &[u8],
    rva: u32,
    image_base: u32,
    file: &PeLookup<'_, '_>,
) -> Option<ImageRecordLayers> {
    let selector = file(target(body, rva, 0x0ce)?, true)?;
    // The second selector argument points to the same stack local that
    // wsprintfW later consumes, after accounting for the pushes and cleanup.
    (narrow_selector(selector, false)
        && matches_at(
            body,
            0x0be,
            b"\x8d\x84\x24\x78\x03\x00\x00\x50\x8d\x84\x24\x7c\x06\x00\x00\x50\xe8????",
        )
        && matches_at(
            body,
            0x19d,
            b"\x8d\x84\x24\x54\x03\x00\x00\x33\xff\x50\x8d\x84\x24\x58\x04\x00\x00\x89\x7c\x24\x1c\
            \x68????\x50\xff\x15????",
        )
        && matches_at(body, 0x1d3, b"\x8b\x35????")
        && data_at(body, 0x1b3, image_base, file)?.starts_with(b"%\0s\0\0\0")
        && import_name_is(body, 0x1ba, image_base, file, b"wsprintfW\0")
        && import_name_is(body, 0x1d5, image_base, file, b"lstrcatW\0"))
    .then_some(ImageRecordLayers::NarrowPrefixReadAsUtf16)
}

fn older_wide_layers(
    body: &[u8],
    rva: u32,
    image_base: u32,
    file: &PeLookup<'_, '_>,
) -> Option<ImageRecordLayers> {
    let selector = file(target(body, rva, 0x0c5)?, true)?;
    (narrow_selector(selector, false)
        && matches_at(
            body,
            0x0b7,
            b"\x8d\x85\x04\x04\x00\x00\x50\x8d\x85\x00\x01\x00\x00\x50\xe8????",
        )
        && matches_at(
            body,
            0x187,
            b"\x8d\x85\x04\x04\x00\x00\xc7\x45\xe4\x00\x00\x00\x00\x50\x8d\x85\x04\x02\x00\x00\x68\
            ????\x50\xff\x15????",
        )
        && matches_at(body, 0x1ba, b"\x8b\x35????")
        && data_at(body, 0x19c, image_base, file)?.starts_with(b"%\0s\0\0\0")
        && import_name_is(body, 0x1a3, image_base, file, b"wsprintfW\0")
        && import_name_is(body, 0x1bc, image_base, file, b"lstrcatW\0"))
    .then_some(ImageRecordLayers::NarrowPrefixReadAsUtf16)
}

pub(super) fn image_records<'a>(
    entries: &[(u32, &'a [u8])],
    image_base: u32,
    file_bytes_at: impl Fn(u32, bool) -> Option<&'a [u8]>,
) -> ImageRecords {
    let release = entries.get(704).and_then(|&(rva, body)| {
        let wrapper = RELEASE.iter().find(|wrapper| {
            let tail = wrapper.header.len() + RELEASE_OBJECTS.len();
            matches_at(body, 0, wrapper.header)
                && matches_at(body, wrapper.header.len(), RELEASE_OBJECTS)
                && matches_at(body, tail, wrapper.tail)
        })?;
        let resolve = target(body, rva, wrapper.resolve)?;
        let release_first = target(body, rva, wrapper.header.len() + 8)?;
        let release_second = target(body, rva, wrapper.header.len() + 21)?;
        if release_first == release_second || [release_first, release_second].contains(&resolve) {
            return None;
        }
        for destination in [resolve, release_first, release_second] {
            file_bytes_at(destination, true)?;
        }
        Some((wrapper.layout, resolve))
    });
    let producer = entries.get(703).and_then(|&(rva, body)| {
        if matches_at(body, 0, SELECTION_POINTER) {
            return Some(ImageProducer703::SelectionPointer);
        }
        let (layout, resolve) = release?;
        let wrapper = PRODUCERS
            .iter()
            .find(|wrapper| matches_at(body, 0, wrapper.wrapper))?;
        if target(body, rva, wrapper.resolve)? != resolve {
            return None;
        }
        let producer_rva = target(body, rva, wrapper.produce)?;
        let producer_body = file_bytes_at(producer_rva, true)?;
        let consumer = wrapper.consumers.iter().find(|consumer| {
            consumer.layout == layout
                && matches_at(producer_body, consumer.clear_loop.0, consumer.clear_loop.1)
        })?;
        Some(ImageProducer703::Records {
            layout,
            layers: (consumer.layers)(producer_body, producer_rva, image_base, &file_bytes_at),
        })
    });
    ImageRecords {
        producer,
        release: release.map(|(layout, _)| layout),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_table::OrdinaryNativeTable;
    use crate::pe_resources::{parse_sections, x86_image_layout};
    use std::path::Path;

    fn games() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../games")
    }

    #[test]
    fn native_image_records_recognizes_registered_contracts() -> std::io::Result<()> {
        let games = games();
        if !games.is_dir() {
            return Ok(());
        }
        let narrow = ImageRecordLayout::Cp932;
        let wide = ImageRecordLayout::Utf16;
        let samples = [
            ("cs2_full_v401/system/cs2_open.exe", narrow),
            ("ISLAND/ISLAND.exe", narrow),
            ("ISLAND/ISLAND_CHS.exe", narrow),
            (
                "\u{7070}\u{8272}\u{7684}\u{8ff7}\u{5bab}/Grisaia2.exe",
                narrow,
            ),
            (
                "\u{7070}\u{8272}\u{7684}\u{4e50}\u{56ed}v1/Grisaia3.exe",
                narrow,
            ),
            (
                "\u{79d8}\u{5bc6}\u{6e38}\u{620f} \u{53cd}\u{53db}/REBELLIONS.exe",
                narrow,
            ),
            (
                "[170526][NEKO WORKs] \u{30cd}\u{30b3}\u{3071}\u{3089} vol.3 steam\u{7248} (files)/nekopara_vol3/NEKOPARAvol3.exe",
                wide,
            ),
            ("Yukikoi Melt/YukikoiMelt.exe", wide),
            (
                "\u{672a}\u{6765}\u{30e9}\u{30b8}\u{30aa}\u{3068}\u{4eba}\u{5de5}\u{9ce9}\u{ff08}AUGUST\u{ff09}/cs2.exe",
                wide,
            ),
            (
                "\u{767d}\u{663c}\u{5922}\u{306e}\u{9752}\u{5199}\u{771f}/cs2.exe",
                wide,
            ),
        ];
        for (path, layout) in samples {
            let pe = std::fs::read(games.join(path))?;
            let table = OrdinaryNativeTable::from_pe(&pe).expect(path);
            assert_eq!(table.image_release_704(), Some(layout), "{path}");
            let layers = match layout {
                ImageRecordLayout::Cp932 => ImageRecordLayers::NarrowStaticRuns,
                ImageRecordLayout::Utf16 => ImageRecordLayers::NarrowPrefixReadAsUtf16,
            };
            assert_eq!(
                table.image_producer_703(),
                Some(ImageProducer703::Records {
                    layout,
                    layers: Some(layers)
                }),
                "{path}"
            );
        }
        for (path, expected) in [
            (
                "\u{3046}\u{305f}\u{3066}\u{3081}\u{3050}\u{308a}/UTATEMEGURI.exe",
                Some(ImageProducer703::SelectionPointer),
            ),
            (
                "\u{30ab}\u{30df}\u{30ab}\u{30bc}\u{2606}\u{30a8}\u{30af}\u{30b9}\u{30d7}\u{30ed}\u{30fc}\u{30e9}\u{30fc}!/cs2.exe",
                None,
            ),
        ] {
            let pe = std::fs::read(games.join(path))?;
            let table = OrdinaryNativeTable::from_pe(&pe).expect(path);
            assert_eq!(table.image_release_704(), None, "{path}");
            assert_eq!(table.image_producer_703(), expected, "{path}");
        }
        Ok(())
    }

    #[test]
    fn native_image_records_requires_release_producer_stride_agreement() -> std::io::Result<()> {
        let path = games().join("cs2_full_v401/system/cs2_open.exe");
        if !path.is_file() {
            return Ok(());
        }
        let pe = std::fs::read(path)?;
        let (base, _) = x86_image_layout(&pe).unwrap();
        let sections = parse_sections(&pe).unwrap();
        let edit = |va| {
            let at = sections
                .iter()
                .find_map(|section| section.file_bytes_from(&pe, va - base))
                .unwrap();
            let offset = at.as_ptr() as usize - pe.as_ptr() as usize;
            let mut changed = pe.clone();
            changed[offset] ^= 1;
            changed
        };
        let layout = ImageRecordLayout::Cp932;
        let mut unrelated_edit = pe.clone();
        unrelated_edit[0x20] ^= 1;
        let unchanged = OrdinaryNativeTable::from_pe(&unrelated_edit).unwrap();
        assert_eq!(unchanged.image_release_704(), Some(layout));
        assert_eq!(
            unchanged.image_producer_703(),
            Some(ImageProducer703::Records {
                layout,
                layers: Some(ImageRecordLayers::NarrowStaticRuns),
            })
        );
        let bad_release = OrdinaryNativeTable::from_pe(&edit(0x518420 + 0x51)).unwrap();
        assert_eq!(bad_release.image_release_704(), None);
        assert_eq!(bad_release.image_producer_703(), None);
        let bad_producer = OrdinaryNativeTable::from_pe(&edit(0x54c0f0 + 0x58)).unwrap();
        assert_eq!(bad_producer.image_release_704(), Some(layout));
        assert_eq!(bad_producer.image_producer_703(), None);
        let unknown_layers = OrdinaryNativeTable::from_pe(&edit(0x54c0f0 + 0x45e)).unwrap();
        assert_eq!(unknown_layers.image_release_704(), Some(layout));
        assert_eq!(
            unknown_layers.image_producer_703(),
            Some(ImageProducer703::Records {
                layout,
                layers: None
            })
        );
        Ok(())
    }
}

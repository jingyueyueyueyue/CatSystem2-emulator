//! Independent PSB2/3/4 object reader from documented format facts.
//! FreeMote is research-only (CC-BY-NC-SA); no FreeMote code is incorporated.
//! Format cross-check: number201724/psbfile MIT snapshot and original PSB bytes.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use super::{EmoteError, EmoteResource, MAX_PSB_BYTES, Result};

const MAX_VALUES: usize = 2_000_000;
const MAX_ARRAY: usize = 4_000_000;
const MAX_STRING: usize = 1_048_576;

fn invalid(why: &'static str) -> EmoteError {
    EmoteError::InvalidResource(why)
}

#[derive(Debug, Clone, PartialEq)]
pub enum PsbValue {
    Null,
    Bool(bool),
    Integer(i64),
    Number(f64),
    String(String),
    Integers(Vec<u32>),
    Resource { extra: bool, index: usize },
    List(Vec<PsbValue>),
    Object(BTreeMap<String, PsbValue>),
}

impl PsbValue {
    pub fn get(&self, name: &str) -> Option<&Self> {
        match self {
            Self::Object(value) => value.get(name),
            _ => None,
        }
    }
    pub fn list(&self) -> Option<&[Self]> {
        match self {
            Self::List(value) => Some(value),
            _ => None,
        }
    }
    pub fn object(&self) -> Option<&BTreeMap<String, Self>> {
        match self {
            Self::Object(value) => Some(value),
            _ => None,
        }
    }
    pub fn text(&self) -> Option<&str> {
        match self {
            Self::String(value) => Some(value),
            _ => None,
        }
    }
    pub fn number(&self) -> Option<f64> {
        match self {
            Self::Integer(value) => Some(*value as f64),
            Self::Number(value) => Some(*value),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PsbDocument {
    pub version: u16,
    pub recovered_header_key: Option<u32>,
    pub value_count: usize,
    pub root: PsbValue,
    bytes: Arc<Vec<u8>>,
    encrypted_header: Option<([u8; 48], usize)>,
    chunks: Vec<(usize, usize)>,
    extra_chunks: Vec<(usize, usize)>,
}

impl PsbDocument {
    pub(super) fn estimated_storage_bytes(&self) -> usize {
        fn value_heap_bytes(value: &PsbValue) -> usize {
            match value {
                PsbValue::String(value) => value.capacity(),
                PsbValue::Integers(values) => values.capacity() * size_of::<u32>(),
                PsbValue::List(values) => values.capacity() * size_of::<PsbValue>()
                    + values.iter().map(value_heap_bytes).sum::<usize>(),
                PsbValue::Object(values) => values.iter().map(|(key, value)|
                    key.capacity() + size_of::<(String, PsbValue)>() * 2 + value_heap_bytes(value)
                ).sum(),
                _ => 0,
            }
        }
        self.bytes.capacity() + value_heap_bytes(&self.root)
            + (self.chunks.capacity() + self.extra_chunks.capacity()) * size_of::<(usize, usize)>()
    }

    #[cfg(test)]
    pub(super) fn from_test_root(root: PsbValue) -> Self {
        Self { version: 2, recovered_header_key: None, value_count: 0, root,
            bytes: Arc::new(Vec::new()), encrypted_header: None,
            chunks: Vec::new(), extra_chunks: Vec::new() }
    }

    pub fn from_resource(resource: &EmoteResource) -> Result<Self> {
        Self::parse_shared(resource.bytes.clone())
    }

    pub(super) fn matches_resource(&self, resource: &EmoteResource) -> bool {
        if Arc::ptr_eq(&self.bytes, &resource.bytes) {
            return true;
        }
        let source = resource.bytes();
        if source.len() != self.bytes.len() {
            return false;
        }
        if let Some((original, end)) = &self.encrypted_header {
            source[..8] == self.bytes[..8]
                && source[8..*end] == original[..*end - 8]
                && source[*end..] == self.bytes[*end..]
        } else {
            source == self.bytes.as_slice()
        }
    }

    pub fn parse(input: &[u8]) -> Result<Self> {
        if input.len() > MAX_PSB_BYTES {
            return Err(invalid("PSB signature/size"));
        }
        Self::parse_shared(Arc::new(input.to_vec()))
    }

    fn parse_shared(mut bytes: Arc<Vec<u8>>) -> Result<Self> {
        let input = bytes.as_slice();
        if input.len() < 40 || input.len() > MAX_PSB_BYTES || !input.starts_with(b"PSB\0") {
            return Err(invalid("PSB signature/size"));
        }
        let version = u16::from_le_bytes([input[4], input[5]]);
        let header_size = match version {
            2 => 40,
            3 => 44,
            4 => 56,
            _ => return Err(invalid("PSB version")),
        };
        if input.len() < header_size {
            return Err(invalid("PSB header truncated"));
        }
        let original_length = u32_at(input, 8)?;
        let mut header = [0; 56];
        header[..header_size].copy_from_slice(&input[..header_size]);
        let key = if original_length != header_size as u32 {
            // First encrypted DWORD is the known canonical header length.
            // Invert y = seed XOR seed>>19, then validate every structural
            // offset and the actual PSB3/4 Adler header checksum.
            let a = 123456789u32 ^ 123456789u32.wrapping_shl(11);
            let y = original_length ^ header_size as u32 ^ a ^ (a >> 8);
            let seed = y ^ (y >> 19);
            let mut cipher = HeaderCipher::new(seed);
            for byte in &mut header[8..header_size] {
                *byte ^= cipher.byte();
            }
            Some(seed)
        } else {
            None
        };
        let mut h = [0u32; 12];
        for (index, word) in h.iter_mut().enumerate().take((header_size - 8) / 4) {
            *word = u32_at(&header, 8 + 4 * index)?;
        }
        if h[0] != header_size as u32 || h[1] != header_size as u32 {
            return Err(invalid("PSB decrypted header length/names offset"));
        }
        if h[1..8].iter().any(|&p| p as usize >= bytes.len()) {
            return Err(invalid("PSB section offset"));
        }
        if version >= 3 {
            let mut check = header[8..40].to_vec();
            if version == 4 {
                check.extend_from_slice(&header[44..56]);
            }
            if adler32(&check) != h[8] {
                return Err(invalid("PSB header Adler32"));
            }
        }
        // Usual sections refer only to bytes after the header. Retain the
        // previous decrypted-byte semantics for offsets into the header itself.
        let encrypted_header = if key.is_some() && h[1..8].iter()
            .chain(h[9..12].iter().take(if version == 4 { 3 } else { 0 }))
            .any(|&offset| (offset as usize) < header_size)
        {
            let mut original = [0; 48];
            original[..header_size - 8].copy_from_slice(&bytes[8..header_size]);
            Arc::make_mut(&mut bytes)[8..header_size].copy_from_slice(&header[8..header_size]);
            Some((original, header_size))
        } else {
            None
        };
        let (bases, next) = packed_array(&bytes, h[1] as usize)?;
        let (parents, next) = packed_array(&bytes, next)?;
        let (indices, _) = packed_array(&bytes, next)?;
        let mut names = Vec::with_capacity(indices.len());
        for index in indices {
            let mut cursor = *parents
                .get(index as usize)
                .ok_or_else(|| invalid("name index"))?;
            let mut seen = BTreeSet::new();
            let mut name = Vec::new();
            while cursor != 0 {
                if !seen.insert(cursor) || name.len() >= MAX_STRING {
                    return Err(invalid("name trie cycle/limit"));
                }
                let parent = *parents
                    .get(cursor as usize)
                    .ok_or_else(|| invalid("name parent"))?;
                let base = *bases
                    .get(parent as usize)
                    .ok_or_else(|| invalid("name base"))?;
                let character = cursor
                    .checked_sub(base)
                    .filter(|&v| v <= 255)
                    .ok_or_else(|| invalid("name character"))?;
                name.push(character as u8);
                cursor = parent;
            }
            name.reverse();
            names.push(String::from_utf8(name).map_err(|_| invalid("name UTF8"))?);
        }
        let (string_offsets, _) = packed_array(&bytes, h[2] as usize)?;
        let mut strings = Vec::with_capacity(string_offsets.len());
        for offset in string_offsets {
            let pos = (h[3] as usize)
                .checked_add(offset as usize)
                .ok_or_else(|| invalid("string offset"))?;
            let tail = bytes.get(pos..).ok_or_else(|| invalid("string bounds"))?;
            let end = tail
                .iter()
                .take(MAX_STRING + 1)
                .position(|&b| b == 0)
                .ok_or_else(|| invalid("string terminator/limit"))?;
            strings.push(
                std::str::from_utf8(&tail[..end])
                    .map_err(|_| invalid("string UTF8"))?
                    .to_owned(),
            );
        }
        let chunks = resource_ranges(&bytes, h[4], h[5], h[6])?;
        let extra_chunks = if version == 4 {
            resource_ranges(&bytes, h[9], h[10], h[11])?
        } else {
            Vec::new()
        };
        let mut reader = Reader {
            bytes: &bytes,
            names: &names,
            strings: &strings,
            count: 0,
            active: Vec::with_capacity(16),
        };
        let root = reader.value(h[7] as usize, 0)?;
        let value_count = reader.count;
        Ok(Self {
            version,
            recovered_header_key: key,
            value_count,
            root,
            bytes,
            encrypted_header,
            chunks,
            extra_chunks,
        })
    }

    pub fn resource(&self, value: &PsbValue) -> Result<&[u8]> {
        Ok(&self.bytes[self.resource_range(value)?])
    }

    pub(super) fn resource_backing(&self) -> Arc<Vec<u8>> {
        self.bytes.clone()
    }

    pub(super) fn resource_range(&self, value: &PsbValue) -> Result<std::ops::Range<usize>> {
        let PsbValue::Resource { extra, index } = value else {
            return Err(invalid("expected PSB chunk reference"));
        };
        let table = if *extra {
            &self.extra_chunks
        } else {
            &self.chunks
        };
        let &(start, len) = table
            .get(*index)
            .ok_or_else(|| invalid("PSB chunk index"))?;
        let end = start.checked_add(len)
            .filter(|&end| end <= self.bytes.len())
            .ok_or_else(|| invalid("PSB chunk bounds"))?;
        Ok(start..end)
    }
}

struct HeaderCipher {
    words: [u32; 4],
    remaining: u32,
}
impl HeaderCipher {
    fn new(seed: u32) -> Self {
        Self {
            words: [123456789, 362436069, 521288629, seed],
            remaining: 0,
        }
    }
    fn byte(&mut self) -> u8 {
        if self.remaining == 0 {
            let a = self.words[0] ^ self.words[0].wrapping_shl(11);
            let b = self.words[3];
            let c = a ^ b ^ ((a ^ (b >> 11)) >> 8);
            self.words = [self.words[1], self.words[2], b, c];
            self.remaining = c;
        }
        let byte = self.remaining as u8;
        self.remaining >>= 8;
        byte
    }
}

fn adler32(bytes: &[u8]) -> u32 {
    let (mut low, mut high) = (1u32, 0u32);
    for &byte in bytes {
        low = (low + u32::from(byte)) % 65521;
        high = (high + low) % 65521;
    }
    high << 16 | low
}

fn uint(bytes: &[u8], pos: usize, width: usize) -> Result<u64> {
    let end = pos
        .checked_add(width)
        .ok_or_else(|| invalid("PSB integer overflow"))?;
    if !(1..=8).contains(&width) {
        return Err(invalid("PSB integer width"));
    }
    let input = bytes
        .get(pos..end)
        .ok_or_else(|| invalid("PSB integer bounds"))?;
    let mut value = [0u8; 8];
    value[..width].copy_from_slice(input);
    Ok(u64::from_le_bytes(value))
}
fn u32_at(bytes: &[u8], pos: usize) -> Result<u32> {
    Ok(uint(bytes, pos, 4)? as u32)
}

fn packed_array(bytes: &[u8], pos: usize) -> Result<(Vec<u32>, usize)> {
    let count_width = usize::from(*bytes.get(pos).ok_or_else(|| invalid("PSB array tag"))?)
        .checked_sub(12)
        .filter(|&w| (1..=4).contains(&w))
        .ok_or_else(|| invalid("PSB array tag"))?;
    let count = uint(bytes, pos + 1, count_width)? as usize;
    let width_pos = pos + 1 + count_width;
    let width = usize::from(*bytes.get(width_pos).ok_or_else(|| invalid("array width"))?)
        .checked_sub(12)
        .filter(|&w| (1..=4).contains(&w))
        .ok_or_else(|| invalid("array width"))?;
    let start = width_pos + 1;
    let size = count
        .checked_mul(width)
        .ok_or_else(|| invalid("array size"))?;
    let end = start
        .checked_add(size)
        .filter(|&n| n <= bytes.len())
        .ok_or_else(|| invalid("array bounds"))?;
    if count > MAX_ARRAY {
        return Err(invalid("array limit"));
    }
    let entries = &bytes[start..end];
    let values = match width {
        1 => entries.iter().copied().map(u32::from).collect(),
        2 => entries.as_chunks::<2>().0.iter().map(|&entry| u32::from(u16::from_le_bytes(entry))).collect(),
        3 => entries.as_chunks::<3>().0.iter().map(|&[a, b, c]| u32::from_le_bytes([a, b, c, 0])).collect(),
        4 => entries.as_chunks::<4>().0.iter().copied().map(u32::from_le_bytes).collect(),
        _ => unreachable!("packed array width was validated"),
    };
    Ok((values, end))
}

fn resource_ranges(
    bytes: &[u8],
    offsets: u32,
    lengths: u32,
    base: u32,
) -> Result<Vec<(usize, usize)>> {
    let (offsets, _) = packed_array(bytes, offsets as usize)?;
    let (lengths, _) = packed_array(bytes, lengths as usize)?;
    if offsets.len() != lengths.len() {
        return Err(invalid("chunk table lengths"));
    }
    offsets
        .into_iter()
        .zip(lengths)
        .map(|(off, len)| {
            let start = (base as usize)
                .checked_add(off as usize)
                .ok_or_else(|| invalid("chunk overflow"))?;
            let size = len as usize;
            if start
                .checked_add(size)
                .filter(|&end| end <= bytes.len())
                .is_none()
            {
                return Err(invalid("chunk bounds"));
            }
            Ok((start, size))
        })
        .collect()
}

struct Reader<'a> {
    bytes: &'a [u8],
    names: &'a [String],
    strings: &'a [String],
    count: usize,
    active: Vec<usize>,
}

impl Reader<'_> {
    fn value(&mut self, pos: usize, depth: usize) -> Result<PsbValue> {
        self.count += 1;
        if self.count > MAX_VALUES || depth > 128 {
            return Err(invalid("PSB value budget/cycle"));
        }
        let tag = *self
            .bytes
            .get(pos)
            .ok_or_else(|| invalid("PSB value offset"))?;
        let recursive = matches!(tag, 32 | 33);
        if recursive {
            if self.active.contains(&pos) {
                return Err(invalid("PSB value budget/cycle"));
            }
            self.active.push(pos);
        }
        let p = pos + 1;
        let result = match tag {
            0 | 1 => PsbValue::Null,
            2 | 3 => PsbValue::Bool(tag == 3),
            4 => PsbValue::Integer(0),
            5..=12 => {
                let width = usize::from(tag - 4);
                let value = uint(self.bytes, p, width)?;
                let shift = (8 - width) * 8;
                PsbValue::Integer(((value << shift) as i64) >> shift)
            }
            13..=20 => PsbValue::Integers(packed_array(self.bytes, pos)?.0),
            21..=24 => {
                let index = uint(self.bytes, p, usize::from(tag - 20))? as usize;
                PsbValue::String(
                    self.strings
                        .get(index)
                        .ok_or_else(|| invalid("PSB string index"))?
                        .clone(),
                )
            }
            25..=28 | 34..=37 => PsbValue::Resource {
                extra: tag >= 34,
                index: uint(
                    self.bytes,
                    p,
                    usize::from(tag - if tag >= 34 { 33 } else { 24 }),
                )? as usize,
            },
            29 => PsbValue::Number(0.0),
            30 => PsbValue::Number(f32::from_bits(uint(self.bytes, p, 4)? as u32) as f64),
            31 => PsbValue::Number(f64::from_bits(uint(self.bytes, p, 8)?)),
            32 => {
                let (offsets, base) = packed_array(self.bytes, p)?;
                let mut values = Vec::with_capacity(offsets.len());
                for off in offsets {
                    let loc = base
                        .checked_add(off as usize)
                        .ok_or_else(|| invalid("list offset"))?;
                    values.push(self.value(loc, depth + 1)?);
                }
                PsbValue::List(values)
            }
            33 => {
                let (names, next) = packed_array(self.bytes, p)?;
                let (offsets, base) = packed_array(self.bytes, next)?;
                if names.len() != offsets.len() {
                    return Err(invalid("dictionary arrays"));
                }
                let mut values = BTreeMap::new();
                for (name, off) in names.into_iter().zip(offsets) {
                    let key = self
                        .names
                        .get(name as usize)
                        .ok_or_else(|| invalid("dictionary name"))?
                        .clone();
                    let loc = base
                        .checked_add(off as usize)
                        .ok_or_else(|| invalid("dictionary offset"))?;
                    if values.insert(key, self.value(loc, depth + 1)?).is_some() {
                        return Err(invalid("duplicate dictionary key"));
                    }
                }
                PsbValue::Object(values)
            }
            _ => return Err(invalid("unknown PSB value tag")),
        };
        if recursive { self.active.pop(); }
        Ok(result)
    }
}

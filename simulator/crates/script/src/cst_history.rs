//! CST history attachment snapshots (991/992/994), independently implemented
//! from the five-DWORD block and serialized variable contracts.
use std::collections::BTreeMap;
use std::io::Write;

use flate2::{Compression, Decompress, FlushDecompress, Status, write::ZlibEncoder};

use crate::{Result, ScriptError};

const MAX_BYTES: usize = 64 * 1024 * 1024;
const MAX_ITEMS: usize = 1_000_000;

fn invalid(message: &'static str) -> ScriptError {
    ScriptError::InvalidStructuredText { offset: 0, message }
}

fn word(bytes: &[u8], offset: usize) -> Result<u32> {
    let end = offset.checked_add(4).ok_or_else(|| invalid("CST snapshot offset overflows"))?;
    bytes.get(offset..end).map(|raw| u32::from_le_bytes(raw.try_into().unwrap()))
        .ok_or_else(|| invalid("CST snapshot DWORD is truncated"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistorySnapshot {
    pub position: u32,
    /// Order and duplicate numeric ids are significant.
    pub numbers: Vec<(u32, u32)>,
    pub strings: BTreeMap<u32, Vec<u8>>,
}

#[derive(Debug, Clone, Copy)]
pub struct HistoryBlock<'a> {
    /// Offset from the start of the complete attachment, for raw-chain trimming.
    pub offset: usize,
    pub position: u32,
    numeric_length: usize,
    string_length: usize,
    compressed: &'a [u8],
}

/// Inspect the relative-pointer chain without decompressing unselected blocks.
/// Each pointer advances beyond its own compressed payload.
pub fn history_blocks(bytes: &[u8]) -> Result<Vec<HistoryBlock<'_>>> {
    if bytes.len() > MAX_BYTES { return Err(invalid("CST snapshot chain exceeds 64 MiB")); }
    let mut blocks = Vec::new();
    if bytes.is_empty() { return Ok(blocks); }
    let mut offset = 0usize;
    loop {
        if blocks.len() >= MAX_ITEMS { return Err(invalid("CST snapshot chain has too many blocks")); }
        let data = bytes.get(offset..).ok_or_else(|| invalid("CST snapshot old pointer is invalid"))?;
        let old = word(data, 0)? as usize;
        let end = word(data, 4)? as usize;
        let numeric_length = word(data, 8)? as usize;
        let string_length = word(data, 12)? as usize;
        if end < 20 || numeric_length % 8 != 0 || numeric_length / 8 > MAX_ITEMS
            || numeric_length.checked_add(string_length).is_none_or(|length| length > MAX_BYTES)
        {
            return Err(invalid("CST snapshot lengths are invalid"));
        }
        let compressed = data.get(20..end)
            .ok_or_else(|| invalid("CST snapshot compressed body is truncated"))?;
        blocks.push(HistoryBlock { offset, position: word(data, 16)?,
            numeric_length, string_length, compressed });
        if old == 0 { break; }
        if old < end { return Err(invalid("CST snapshot old pointer overlaps its payload")); }
        offset = offset.checked_add(old).filter(|&next| next < bytes.len())
            .ok_or_else(|| invalid("CST snapshot old pointer is outside the chain"))?;
    }
    Ok(blocks)
}

impl HistoryBlock<'_> {
    pub fn decode(self) -> Result<HistorySnapshot> {
        let length = self.numeric_length.checked_add(self.string_length)
            .filter(|&length| length <= MAX_BYTES)
            .ok_or_else(|| invalid("CST snapshot expanded length exceeds 64 MiB"))?;
        let mut bytes = vec![0; length + 1];
        let mut decoder = Decompress::new(true);
        let status = decoder.decompress(self.compressed, &mut bytes, FlushDecompress::Finish)
            .map_err(|_| invalid("CST snapshot Zlib stream is malformed"))?;
        if status != Status::StreamEnd || decoder.total_out() != length as u64
            || decoder.total_in() != self.compressed.len() as u64
        {
            return Err(invalid("CST snapshot compressed/expanded length does not match"));
        }
        bytes.truncate(length);
        let numbers = bytes[..self.numeric_length].chunks_exact(8)
            .map(|pair| (u32::from_le_bytes(pair[..4].try_into().unwrap()),
                u32::from_le_bytes(pair[4..].try_into().unwrap()))).collect();
        let body = &bytes[self.numeric_length..];
        let count = word(body, 0)? as usize;
        if count > MAX_ITEMS { return Err(invalid("CST snapshot string count exceeds the limit")); }
        let mut cursor = 4usize;
        let mut strings = BTreeMap::new();
        for _ in 0..count {
            let id = word(body, cursor)?;
            cursor += 4;
            let tail = body.get(cursor..).ok_or_else(|| invalid("CST snapshot string is truncated"))?;
            let length = tail.iter().position(|&byte| byte == 0)
                .ok_or_else(|| invalid("CST snapshot string has no terminator"))?;
            strings.entry(id).or_insert_with(|| tail[..length].to_vec());
            cursor += length + 1;
        }
        // The original producer includes eight unwritten bytes at the very
        // end. Their values are not part of the string contents.
        if body.len().saturating_sub(cursor) < 8 {
            return Err(invalid("CST snapshot string trailer is truncated"));
        }
        Ok(HistorySnapshot { position: self.position, numbers, strings })
    }
}

/// Prepend one compressed block and retain the old serialized chain verbatim.
pub fn prepend_history_snapshot(snapshot: &HistorySnapshot, old: &[u8]) -> Result<Vec<u8>> {
    history_blocks(old)?;
    if snapshot.numbers.len() > MAX_ITEMS || snapshot.strings.len() > MAX_ITEMS {
        return Err(invalid("CST snapshot variable count exceeds the limit"));
    }
    let numeric_length = snapshot.numbers.len().checked_mul(8)
        .ok_or_else(|| invalid("CST snapshot numeric length overflows"))?;
    let length = snapshot.strings.values().try_fold(numeric_length + 12, |length, value| {
        length.checked_add(5)?.checked_add(value.len()).filter(|&length| length <= MAX_BYTES)
    }).filter(|&length| length <= MAX_BYTES)
        .ok_or_else(|| invalid("CST snapshot body exceeds 64 MiB"))?;
    let mut body = Vec::with_capacity(length);
    for &(id, value) in &snapshot.numbers {
        body.extend_from_slice(&id.to_le_bytes());
        body.extend_from_slice(&value.to_le_bytes());
    }
    body.extend_from_slice(&(snapshot.strings.len() as u32).to_le_bytes());
    for (&id, value) in &snapshot.strings {
        body.extend_from_slice(&id.to_le_bytes());
        let start = body.len();
        body.resize(start + value.len() + 1, 0);
        let end = value.iter().position(|&byte| byte == 0).unwrap_or(value.len());
        body[start..start + end].copy_from_slice(&value[..end]);
    }
    body.extend_from_slice(&[0; 8]);
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&body).map_err(|_| invalid("CST snapshot compression failed"))?;
    let compressed = encoder.finish().map_err(|_| invalid("CST snapshot compression failed"))?;
    let end = compressed.len().checked_add(20).ok_or_else(|| invalid("CST snapshot length overflows"))?;
    let total = end.checked_add(old.len()).filter(|&length| length <= MAX_BYTES)
        .ok_or_else(|| invalid("CST snapshot chain exceeds 64 MiB"))?;
    let mut output = Vec::with_capacity(total);
    for value in [if old.is_empty() { 0 } else { end as u32 }, end as u32,
        numeric_length as u32, (body.len() - numeric_length) as u32, snapshot.position]
    {
        output.extend_from_slice(&value.to_le_bytes());
    }
    output.extend_from_slice(&compressed);
    output.extend_from_slice(old);
    Ok(output)
}

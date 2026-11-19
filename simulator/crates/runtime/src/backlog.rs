//! Byte-oriented backlog tables, independently recovered from the public
//! format contract and baseline PE instructions. No font/layout dependency.
use std::collections::VecDeque;
use cs2_formats::ConfigXml;
use serde::{Deserialize, Serialize};
use thiserror::Error;

const MAX_BYTES: usize = 64 * 1024 * 1024;
const MAX_ITEMS: usize = 1_000_000;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum BacklogError {
    #[error("backlog data exceeds its storage limit")]
    TooLarge,
    #[error("backlog settings cannot advance byte layout")]
    InvalidSettings,
    #[error("backlog name prefix/suffix exceeds 31 bytes")]
    NameFormatTooLong,
    #[error("backlog data is truncated or has an unterminated string")]
    Truncated,
    #[error("backlog table indices or offsets are invalid")]
    InvalidTables,
    #[error("backlog text has an incomplete two-byte character")]
    TruncatedCharacter,
}

pub type Result<T> = std::result::Result<T, BacklogError>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    pub columns: u32,
    pub extra_columns: u32,
    pub name_line: u32,
    pub name_first: Vec<u8>,
    pub name_last: Vec<u8>,
    pub indent: u32,
    pub name_indent: u32,
    pub display_message: u32,
    pub display_name: u32,
    pub space: u32,
    pub escape: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self { columns: 40, extra_columns: 4, name_line: 1,
            name_first: Vec::new(), name_last: Vec::new(), indent: 1, name_indent: 0,
            display_message: 1, display_name: 1, space: 0, escape: 1 }
    }
}

impl Settings {
    pub fn from_xml(xml: &ConfigXml) -> Result<Self> {
        let mut result = Self::default();
        for (suffix, field) in [
            (&[b"col".as_slice()][..], &mut result.columns),
            (&[b"prof".as_slice()][..], &mut result.extra_columns),
            (&[b"nameline".as_slice()][..], &mut result.name_line),
            (&[b"indent".as_slice()][..], &mut result.indent),
            (&[b"nameindent".as_slice()][..], &mut result.name_indent),
            (&[b"disp".as_slice(), b"mes"][..], &mut result.display_message),
            (&[b"disp".as_slice(), b"name"][..], &mut result.display_name),
            (&[b"space".as_slice()][..], &mut result.space),
            (&[b"escape".as_slice()][..], &mut result.escape),
        ] {
            *field = xml.integer(&settings_path(suffix), *field as i32) as u32;
        }
        result.name_first = xml.text(&settings_path(&[b"nameformat", b"first"])).unwrap_or_default().to_vec();
        result.name_last = xml.text(&settings_path(&[b"nameformat", b"last"])).unwrap_or_default().to_vec();
        result.validate()?;
        Ok(result)
    }

    fn validate(&self) -> Result<()> {
        if nul(&self.name_first).len() >= 32 || nul(&self.name_last).len() >= 32 {
            return Err(BacklogError::NameFormatTooLong);
        }
        if self.columns as i32 <= 0 || self.columns > 1024
            || self.extra_columns > 1024 || (self.name_indent as i32) < 0
            || self.name_indent >= self.columns.saturating_sub(1) {
            return Err(BacklogError::InvalidSettings);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub flags: u32,
    pub source: u32,
    pub input: u32,
    pub first_line: u32,
    pub last_line: u32,
    pub name: u32,
    pub extra: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub segment: u32,
    pub text: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Backlog {
    settings: Settings,
    limit: u32,
    sources: Vec<Vec<u8>>,
    segments: VecDeque<Segment>,
    lines: VecDeque<Line>,
    names: Vec<Vec<u8>>,
    segment_start: u32,
    line_start: u32,
    serialized_size: usize,
}

/// Complete host object state. Ordinary850's body intentionally omits the
/// constructor settings and retention limit; a full save must retain both.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BacklogSnapshot {
    pub version: u32,
    pub settings: Settings,
    pub limit: u32,
    pub body: Vec<u8>,
}

impl Backlog {
    pub fn snapshot(&self) -> Result<BacklogSnapshot> {
        Ok(BacklogSnapshot { version: 1, settings: self.settings.clone(),
            limit: self.limit, body: self.serialize()? })
    }

    pub fn from_snapshot(snapshot: &BacklogSnapshot) -> Result<Self> {
        if snapshot.version != 1 { return Err(BacklogError::InvalidTables); }
        let mut result = Self::new(snapshot.settings.clone())?;
        result.limit = snapshot.limit;
        result.restore(&snapshot.body)?;
        Ok(result)
    }

    pub fn new(settings: Settings) -> Result<Self> {
        settings.validate()?;
        Ok(Self { settings, limit: 100, sources: Vec::new(), segments: VecDeque::new(),
            lines: VecDeque::new(), names: Vec::new(), segment_start: 0, line_start: 0,
            serialized_size: 40 })
    }

    pub fn settings(&self) -> &Settings { &self.settings }
    pub fn limit(&self) -> u32 { self.limit }
    pub fn sources(&self) -> &[Vec<u8>] { &self.sources }
    pub fn segments(&self) -> &VecDeque<Segment> { &self.segments }
    pub fn lines(&self) -> &VecDeque<Line> { &self.lines }
    pub fn names(&self) -> &[Vec<u8>] { &self.names }
    pub fn segment_start(&self) -> u32 { self.segment_start }
    pub fn line_start(&self) -> u32 { self.line_start }

    /// Replace display bytes in the retained rows, without laying out or
    /// appending history again. None leaves a spacer/unresolved row intact.
    /// Names are indexed by the existing segment, never by a new history id.
    /// All validation/allocation finishes before any retained row is changed.
    pub fn replace_remade_rows(
        &mut self, rows: Vec<Option<Vec<u8>>>, names: Vec<(usize, Vec<u8>)>,
    ) -> Result<()> {
        if rows.len() != self.lines.len() || names.len() > self.segments.len() {
            return Err(BacklogError::InvalidTables);
        }
        // The original remake regenerates only the presentation-name table.
        // History segments/rows themselves are neither cleared nor appended.
        let mut next_names = Vec::new();
        let mut bindings = vec![None; self.segments.len()];
        for (segment, name) in names {
            if segment >= self.segments.len() || name.contains(&0)
                || bindings[segment].is_some()
            { return Err(BacklogError::InvalidTables); }
            let length = self.settings.name_first.len().checked_add(name.len())
                .and_then(|length| length.checked_add(self.settings.name_last.len()))
                .filter(|&length| length <= MAX_BYTES).ok_or(BacklogError::TooLarge)?;
            let mut formatted = Vec::with_capacity(length);
            formatted.extend_from_slice(nul(&self.settings.name_first));
            formatted.extend_from_slice(&name);
            formatted.extend_from_slice(nul(&self.settings.name_last));
            // A present empty localized Name is still formatted by the native
            // name helper (28f0b0); absence of a Name record is a separate case.
            let name = intern(&mut next_names, &formatted)?;
            bindings[segment] = Some(name);
        }
        let mut size = 40usize.checked_add(self.segments.len() * 28)
            .ok_or(BacklogError::TooLarge)?;
        for name in self.sources.iter().chain(&next_names) {
            size = size.checked_add(name.len()).and_then(|n| n.checked_add(1))
                .filter(|&n| n <= MAX_BYTES).ok_or(BacklogError::TooLarge)?;
        }
        for (old, replacement) in self.lines.iter().zip(&rows) {
            let bytes = replacement.as_deref().unwrap_or(&old.text);
            if replacement.is_some() && (old.segment == u32::MAX || bytes.contains(&0)) {
                return Err(BacklogError::InvalidTables);
            }
            size = size.checked_add(bytes.len()).and_then(|n| n.checked_add(5))
                .filter(|&n| n <= MAX_BYTES).ok_or(BacklogError::TooLarge)?;
        }
        // No clear/restore/trim: absolute line numbers, limits, flags, input,
        // source, voice/path extra and every existing segment stay in place.
        for (line, text) in self.lines.iter_mut().zip(rows) {
            if let Some(text) = text { line.text = text; }
        }
        for (segment, name) in self.segments.iter_mut().zip(bindings) {
            segment.name = name.unwrap_or(u32::MAX);
        }
        self.names = next_names;
        self.serialized_size = size;
        Ok(())
    }

    pub fn clear(&mut self) {
        self.sources.clear(); self.segments.clear(); self.lines.clear(); self.names.clear();
        self.serialized_size = 40;
        // 0x68cb40 leaves both cumulative starts and the limit unchanged.
    }

    pub fn set_limit(&mut self, limit: u32) {
        self.limit = limit;
        self.trim();
    }

    pub fn name_for_row(&self, row: u32) -> Option<&[u8]> {
        let segment = self.lines.get(row as usize)?.segment;
        if segment == u32::MAX { return None; }
        let segment = self.segments.get(segment.wrapping_sub(self.segment_start) as usize)?;
        self.names.get(segment.name as usize).map(Vec::as_slice)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn append(
        &mut self, source: &[u8], input: u32, name: &[u8], text: &[u8],
        mark: u32, space: u32, extra: u32,
    ) -> Result<bool> {
        let source = nul(source);
        if self.segments.back().is_some_and(|last| last.input == input
            && self.sources.get(last.source as usize).is_some_and(|last| last == source)) {
            return Ok(false);
        }
        let name = preprocess(nul(name), self.settings.escape != 0)?;
        let text = preprocess(nul(text), self.settings.escape != 0)?;
        let formatted_name = if name.is_empty() { Vec::new() } else {
            [nul(&self.settings.name_first), &name, nul(&self.settings.name_last)].concat()
        };
        let mut rows = Vec::new();
        if self.settings.space != 0 && space != 0 && !self.lines.is_empty() {
            rows.push(Line { segment: u32::MAX, text: Vec::new() });
        }
        let absolute_segment = self.segment_start.wrapping_add(self.segments.len() as u32);
        let mut first = 0u32;
        let mut last = 0u32;
        let mut separate_name = None;
        if !name.is_empty() && self.settings.name_line != 0 {
            let index = self.line_start.wrapping_add((self.lines.len() + rows.len()) as u32);
            separate_name = Some(index);
            rows.push(Line { segment: absolute_segment, text: formatted_name.clone() });
        }
        if !text.is_empty() {
            let prefix = if self.settings.name_line == 0 && self.settings.display_name != 0 {
                formatted_name.as_slice()
            } else { &[] };
            let wrapped = layout(&text, prefix, &self.settings)?;
            if !wrapped.is_empty() {
                first = self.line_start.wrapping_add((self.lines.len() + rows.len()) as u32);
                last = first.wrapping_add(wrapped.len() as u32 - 1);
                rows.extend(wrapped.into_iter().map(|text| Line { segment: absolute_segment, text }));
            }
        }
        if let Some(index) = separate_name { first = index; }
        // A separate name without body leaves last at its native zero value.
        let mut addition = 28usize;
        for row in &rows { addition = addition.saturating_add(5).saturating_add(row.text.len()); }
        if !self.sources.iter().any(|value| value == source) {
            addition = addition.saturating_add(source.len()).saturating_add(1);
        }
        if !name.is_empty() && !self.names.contains(&formatted_name) {
            addition = addition.saturating_add(formatted_name.len()).saturating_add(1);
        }
        if self.serialized_size.saturating_add(addition) > MAX_BYTES {
            return Err(BacklogError::TooLarge);
        }
        let source_id = intern(&mut self.sources, source)?;
        let name_id = if name.is_empty() { u32::MAX } else {
            intern(&mut self.names, &formatted_name)?
        };
        if self.segments.len() >= MAX_ITEMS || self.lines.len().saturating_add(rows.len()) > MAX_ITEMS {
            return Err(BacklogError::TooLarge);
        }
        let flags = u32::from(!text.is_empty()) | (u32::from(!name.is_empty()) << 1)
            | (u32::from(mark != 0) << 2);
        self.segments.push_back(Segment {
            flags, source: source_id, input, first_line: first, last_line: last,
            name: name_id, extra,
        });
        self.lines.extend(rows);
        self.serialized_size += addition;
        self.trim();
        Ok(true)
    }

    fn trim(&mut self) {
        if self.limit == u32::MAX { return; }
        while self.segments.len() > self.limit as usize {
            let Some(segment) = self.segments.pop_front() else { break; };
            self.serialized_size -= 28;
            // Empty segment endpoints remain zero. The PE still advances
            // line_start for each nonnegative absolute range, even if the
            // current deque contains no corresponding display line.
            if segment.first_line as i32 <= segment.last_line as i32 {
                let count = segment.last_line.wrapping_sub(self.line_start) as i32;
                if count >= 0 {
                    let count = count as usize + 1;
                    let existing = count.min(self.lines.len());
                    for line in self.lines.drain(..existing) {
                        self.serialized_size -= 5 + line.text.len();
                    }
                    self.line_start = self.line_start.wrapping_add(count as u32);
                }
            }
            while self.lines.front().is_some_and(|line| line.segment == u32::MAX) {
                if let Some(line) = self.lines.pop_front() {
                    self.serialized_size -= 5 + line.text.len();
                }
                self.line_start = self.line_start.wrapping_add(1);
            }
            self.segment_start = self.segment_start.wrapping_add(1);
        }
    }

    pub fn serialize(&self) -> Result<Vec<u8>> {
        let mut output = vec![0; 24];
        output[16..20].copy_from_slice(&self.segment_start.to_le_bytes());
        output[20..24].copy_from_slice(&self.line_start.to_le_bytes());
        for table in 0..4 {
            let offset = u32::try_from(output.len()).map_err(|_| BacklogError::TooLarge)?;
            output[table * 4..table * 4 + 4].copy_from_slice(&offset.to_le_bytes());
            let count = match table { 0 => self.sources.len(), 1 => self.segments.len(),
                2 => self.lines.len(), _ => self.names.len() };
            put(&mut output, count as u32)?;
            match table {
                0 | 3 => for text in if table == 0 { &self.sources } else { &self.names } {
                    put_string(&mut output, text)?;
                },
                1 => for item in &self.segments {
                    for value in [item.flags, item.source, item.input, item.first_line,
                        item.last_line, item.name, item.extra] { put(&mut output, value)?; }
                },
                2 => for item in &self.lines {
                    put(&mut output, item.segment)?; put_string(&mut output, &item.text)?;
                },
                _ => unreachable!(),
            }
        }
        Ok(output)
    }

    pub fn restore(&mut self, data: &[u8]) -> Result<()> {
        if data.len() > MAX_BYTES { return Err(BacklogError::TooLarge); }
        let mut restored = Self::new(self.settings.clone())?;
        restored.limit = self.limit;
        restored.segment_start = word(data, 16)?;
        restored.line_start = word(data, 20)?;
        for table in 0..4 {
            let mut cursor = word(data, table * 4)? as usize;
            if cursor < 24 { return Err(BacklogError::InvalidTables); }
            let count = word(data, cursor)? as usize;
            cursor += 4;
            if count > MAX_ITEMS { return Err(BacklogError::TooLarge); }
            match table {
                0 | 3 => for _ in 0..count {
                    let text = string(data, &mut cursor)?.to_vec();
                    if table == 0 { restored.sources.push(text); } else { restored.names.push(text); }
                },
                1 => for _ in 0..count {
                    let mut fields = [0; 7];
                    for field in &mut fields { *field = word(data, cursor)?; cursor += 4; }
                    restored.segments.push_back(Segment {
                        flags: fields[0], source: fields[1], input: fields[2], first_line: fields[3],
                        last_line: fields[4], name: fields[5], extra: fields[6],
                    });
                },
                2 => for _ in 0..count {
                    let segment = word(data, cursor)?; cursor += 4;
                    let text = string(data, &mut cursor)?.to_vec();
                    restored.lines.push_back(Line { segment, text });
                },
                _ => unreachable!(),
            }
        }
        if restored.segments.iter().any(|segment| segment.source as usize >= restored.sources.len()
            || (segment.name != u32::MAX && segment.name as usize >= restored.names.len()))
            || restored.lines.iter().any(|line| line.segment != u32::MAX
                && line.segment.wrapping_sub(restored.segment_start) as usize >= restored.segments.len()) {
            return Err(BacklogError::InvalidTables);
        }
        restored.serialized_size = 40;
        for text in restored.sources.iter().chain(&restored.names) {
            restored.serialized_size = restored.serialized_size.saturating_add(text.len()).saturating_add(1);
        }
        restored.serialized_size = restored.serialized_size.saturating_add(restored.segments.len() * 28);
        for line in &restored.lines {
            restored.serialized_size = restored.serialized_size.saturating_add(5).saturating_add(line.text.len());
        }
        if restored.serialized_size > MAX_BYTES { return Err(BacklogError::TooLarge); }
        restored.trim();
        *self = restored;
        Ok(())
    }
}

fn settings_path<'a>(suffix: &[&'a [u8]]) -> Vec<&'a [u8]> {
    let mut path = vec![b"document".as_slice(), b"GAME", b"log"];
    path.extend_from_slice(suffix);
    path
}

fn intern(table: &mut Vec<Vec<u8>>, value: &[u8]) -> Result<u32> {
    if let Some(index) = table.iter().position(|text| text == value) { return Ok(index as u32); }
    if table.len() >= MAX_ITEMS || value.len() > MAX_BYTES { return Err(BacklogError::TooLarge); }
    table.push(value.to_vec());
    Ok(table.len() as u32 - 1)
}

fn nul(data: &[u8]) -> &[u8] { data.split(|&byte| byte == 0).next().unwrap_or_default() }
fn word(data: &[u8], offset: usize) -> Result<u32> {
    let end = offset.checked_add(4).ok_or(BacklogError::TooLarge)?;
    Ok(u32::from_le_bytes(data.get(offset..end).ok_or(BacklogError::Truncated)?
        .try_into().unwrap()))
}
fn string<'a>(data: &'a [u8], cursor: &mut usize) -> Result<&'a [u8]> {
    let tail = data.get(*cursor..).ok_or(BacklogError::Truncated)?;
    let length = tail.iter().position(|&byte| byte == 0).ok_or(BacklogError::Truncated)?;
    *cursor += length + 1;
    Ok(&tail[..length])
}
fn put(output: &mut Vec<u8>, value: u32) -> Result<()> {
    if output.len().saturating_add(4) > MAX_BYTES { return Err(BacklogError::TooLarge); }
    output.extend_from_slice(&value.to_le_bytes()); Ok(())
}
fn put_string(output: &mut Vec<u8>, value: &[u8]) -> Result<()> {
    let value = nul(value);
    if output.len().saturating_add(value.len()).saturating_add(1) > MAX_BYTES {
        return Err(BacklogError::TooLarge);
    }
    output.extend_from_slice(value); output.push(0); Ok(())
}

fn unit(data: &[u8], offset: usize) -> Result<usize> {
    let byte = *data.get(offset).ok_or(BacklogError::TruncatedCharacter)?;
    let next = data.get(offset + 1);
    let double_byte = matches!(byte, 0x81..=0x9f | 0xe0..=0xef)
        && matches!(next, Some(0x40..=0x7e | 0x80..=0xfc));
    let escape = byte == b'\\' && matches!(next, Some(b'[' | b'\\' | b'n'));
    if double_byte || escape {
        Ok(2)
    } else { Ok(1) }
}

fn numeric_span(data: &[u8]) -> usize {
    let mut offset = usize::from(matches!(data.first(), Some(b'+' | b'-')));
    let hexadecimal = data.get(offset..).is_some_and(|tail| tail.starts_with(b"0x") || tail.starts_with(b"0X"));
    if hexadecimal { offset += 2; }
    let start = offset;
    while data.get(offset).is_some_and(|byte| if hexadecimal { byte.is_ascii_hexdigit() } else { byte.is_ascii_digit() }) {
        offset += 1;
    }
    if offset == start || (!hexadecimal && data.get(offset) == Some(&b'.')) { return 0; }
    offset + usize::from(data.get(offset) == Some(&b';'))
}

fn preprocess(data: &[u8], escape: bool) -> Result<Vec<u8>> {
    if data.len() > MAX_BYTES { return Err(BacklogError::TooLarge); }
    let mut output = Vec::with_capacity(data.len());
    let mut offset = 0;
    while offset < data.len() {
        if data[offset] == b'\\' {
            let tail = &data[offset..];
            let next = tail.get(1).copied();
            if escape {
                if next == Some(b'@') { offset += 2; continue; }
                if matches!(next, Some(b'r' | b'R')) {
                    offset += if matches!(tail.get(2..4), Some(b"\\n" | b"\\N")) { 4 } else { 2 };
                    continue;
                }
            } else {
                let consumed = match next {
                    Some(b'@' | b'n' | b'N') => 2,
                    Some(b'c' | b'C') => {
                        let prefix = if matches!(tail.get(2), Some(b'r' | b'R' | b's' | b'S')) { 3 } else { 2 };
                        prefix + numeric_span(tail.get(prefix..).unwrap_or_default())
                    }
                    Some(b'w' | b'W' | b'x' | b'X' | b'y' | b'Y') =>
                        2 + numeric_span(tail.get(2..).unwrap_or_default()),
                    Some(b'f' | b'F') => {
                        match tail.get(2) {
                            Some(b'+' | b'-' | b'r' | b'R') =>
                                3 + numeric_span(tail.get(3..).unwrap_or_default()),
                            Some(b's' | b'S' | b'l' | b'L') => {
                                let second = match tail[2].to_ascii_lowercase() { b's' => b's', _ => b'l' };
                                let count = if tail.get(3).is_some_and(|byte| byte.to_ascii_lowercase() == second) { 4 } else { 3 };
                                count + usize::from(tail.get(count) == Some(&b';'))
                            }
                            Some(b'n' | b'N') => 3 + usize::from(tail.get(4) == Some(&b';')),
                            _ => 2 + numeric_span(tail.get(2..).unwrap_or_default()),
                        }
                    }
                    Some(b'p' | b'P') => if matches!(tail.get(2),
                        Some(b'c' | b'C' | b'l' | b'L' | b'r' | b'R')) { 3 } else { 1 },
                    Some(b'[' | b'\\') => {
                        output.extend_from_slice(&tail[..2]); offset += 2; continue;
                    }
                    _ => 1, // Unsupported control removes only its slash.
                };
                offset += consumed.min(tail.len());
                continue;
            }
        }
        let width = unit(data, offset)?;
        output.extend_from_slice(&data[offset..offset + width]);
        offset += width;
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backlog_keeps_unpaired_bytes_and_still_consumes_valid_pairs() -> Result<()> {
        assert_eq!(unit(b"\xe3", 0)?, 1);
        assert_eq!(unit(b"\x81\x7f", 0)?, 1);
        assert_eq!(unit(b"\x81\x40", 0)?, 2);
        let mut backlog = Backlog::new(Settings::default())?;
        let name = b"Sample";
        let text = b"\xe3\x80\x8cA sample line for history. \xe2\x99\xaa\xe3\x80\x8d";
        assert!(backlog.append(b"01", 0, name, text, 1, 0, 0)?);
        assert_eq!(backlog.names()[0], name);
        let restored_text: Vec<u8> = backlog.lines().iter().skip(1)
            .flat_map(|line| line.text.iter().copied()).collect();
        assert_eq!(restored_text, text);
        Ok(())
    }
}

fn layout(text: &[u8], prefix: &[u8], settings: &Settings) -> Result<Vec<Vec<u8>>> {
    let opening_indent = if text.len() >= 2 && matches!(&text[..2],
        b"\x81\x69" | b"\x81\x6d" | b"\x81\x6f" | b"\x81\x75" | b"\x81\x77") { 2 } else { 0 };
    let mut output = Vec::new();
    let mut offset = 0usize;
    while offset < text.len() {
        let start = offset;
        let first = output.is_empty();
        let indent = settings.name_indent as usize + if first { 0 } else { opening_indent };
        let mut row = if first { prefix.to_vec() } else { Vec::new() };
        let spaces = if first { indent.saturating_sub(prefix.len()) } else { indent };
        for _ in 0..spaces / 2 { row.extend_from_slice(b"\x81\x40"); }
        if spaces % 2 != 0 { row.push(b' '); }
        let mut width = row.len();
        while offset < text.len() {
            if text[offset] == b'[' {
                let mut end = offset + 1;
                let mut body_end = None;
                while end < text.len() {
                    if text[end] == b'/' && body_end.is_none() { body_end = Some(end); }
                    if text[end] == b']' { end += 1; break; }
                    end += unit(text, end)?;
                }
                let body = body_end.unwrap_or_else(|| end - usize::from(text.get(end.wrapping_sub(1)) == Some(&b']')));
                let visible = body.saturating_sub(offset + 1);
                if width.saturating_add(visible) > (settings.columns + settings.extra_columns) as usize { break; }
                width += visible;
                offset = end;
                if width >= settings.columns as usize { break; }
            } else {
                let count = unit(text, offset)?;
                if width.saturating_add(count) >= settings.columns as usize { break; }
                width += count;
                offset += count;
            }
        }
        if offset == start { return Err(BacklogError::InvalidSettings); }
        // Native kinsoku lookahead adjusts local cursors, but serialization
        // copies exactly this forward-consumed source range.
        row.extend_from_slice(&text[start..offset]);
        if row.len() >= 1024 { return Err(BacklogError::TooLarge); }
        output.push(row);
        if output.len() > MAX_ITEMS { return Err(BacklogError::TooLarge); }
    }
    Ok(output)
}

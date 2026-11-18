//! `anmtable.csv`: native 684010 uses the delimited-text reader in TAB mode.
//! Stored keys remain literal; lookup uppercases single-byte input characters.
use std::collections::BTreeMap;
use thiserror::Error;
use crate::util::{MAX_BUFFER, MAX_ITEMS};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AnimationAliases {
    entries: BTreeMap<Vec<u8>, Vec<u8>>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum AnimationAliasError {
    #[error("animation alias table exceeds its storage/count limit")]
    TooLarge,
    #[error("animation alias field exceeds the original fixed scratch buffer")]
    FieldTooLong,
    #[error("animation alias table contains a zero byte")]
    EmbeddedZero,
    #[error("animation alias table contains an empty stored key")]
    EmptyKey,
}

impl AnimationAliases {
    /// Stored keys/values, without lookup case folding or a text round trip.
    pub fn entries(&self) -> &BTreeMap<Vec<u8>, Vec<u8>> { &self.entries }

    /// Restore an already parsed table while retaining its literal byte keys.
    pub fn from_entries(entries: BTreeMap<Vec<u8>, Vec<u8>>) -> Result<Self, AnimationAliasError> {
        if entries.len() > MAX_ITEMS { return Err(AnimationAliasError::TooLarge); }
        let mut bytes = 0usize;
        for (key, value) in &entries {
            if key.is_empty() { return Err(AnimationAliasError::EmptyKey); }
            if key.contains(&0) || value.contains(&0) {
                return Err(AnimationAliasError::EmbeddedZero);
            }
            if key.len() >= 256 || value.len() >= 256 { return Err(AnimationAliasError::FieldTooLong); }
            bytes = bytes.checked_add(key.len()).and_then(|n| n.checked_add(value.len()))
                .filter(|&n| n <= MAX_BUFFER).ok_or(AnimationAliasError::TooLarge)?;
        }
        Ok(Self { entries })
    }

    pub fn parse(data: &[u8]) -> Result<Self, AnimationAliasError> {
        if data.len() > MAX_BUFFER { return Err(AnimationAliasError::TooLarge); }
        if data.contains(&0) { return Err(AnimationAliasError::EmbeddedZero); }
        let mut entries = BTreeMap::new();
        let mut cursor = 0;
        let mut rows = 0;
        let mut cells = 0;
        while cursor < data.len() {
            rows += 1;
            if rows > MAX_ITEMS { return Err(AnimationAliasError::TooLarge); }
            let mut fields = Vec::new();
            while cursor < data.len() && !matches!(data[cursor], b'\r' | b'\n') {
                cells += 1;
                if cells > MAX_ITEMS { return Err(AnimationAliasError::TooLarge); }
                let value = field(data, &mut cursor)?;
                if fields.len() < 2 { fields.push(value); }
            }
            if cursor < data.len() {
                let newline = data[cursor];
                cursor += 1;
                if newline == b'\r' && data.get(cursor) == Some(&b'\n') { cursor += 1; }
            }
            if fields.len() == 2 && !fields[0].is_empty() {
                if fields[0].len() >= 256 || fields[1].len() >= 256 {
                    return Err(AnimationAliasError::FieldTooLong);
                }
                let value = fields.pop().unwrap();
                // std::map insertion keeps the first duplicate stored key.
                entries.entry(fields.pop().unwrap()).or_insert(value);
            }
        }
        Ok(Self { entries })
    }

    pub fn resolve(&self, name: &[u8]) -> Option<&[u8]> {
        let mut key = Vec::with_capacity(name.len());
        let mut cursor = 0;
        while cursor < name.len() {
            let width = character_width(&name[cursor..]);
            if width == 2 { key.extend_from_slice(&name[cursor..cursor + 2]); }
            else { key.push(name[cursor].to_ascii_uppercase()); }
            cursor += width;
        }
        self.entries.get(&key).map(Vec::as_slice)
    }
}

fn character_width(bytes: &[u8]) -> usize {
    if bytes.len() >= 2 && matches!(bytes[0], 0x81..=0x9f | 0xe0..=0xef)
        && matches!(bytes[1], 0x40..=0x7e | 0x80..=0xfc) { 2 } else { 1 }
}

fn field(data: &[u8], cursor: &mut usize) -> Result<Vec<u8>, AnimationAliasError> {
    let quoted = data[*cursor] == b'"';
    if quoted { *cursor += 1; }
    let mut value = Vec::new();
    while *cursor < data.len() {
        let byte = data[*cursor];
        match byte {
            b'"' => {
                *cursor += 1;
                if data.get(*cursor) == Some(&b'"') {
                    value.push(b'"');
                    *cursor += 1;
                } else { break; }
            }
            b'\t' if !quoted => break,
            b'\r' | b'\n' if !quoted => break,
            b'\r' | b'\n' => {
                // The parser's constructor leaves newline substitution off:
                // quoted newlines disappear; CRLF/LFCR count as one break.
                *cursor += 1;
                if data.get(*cursor).is_some_and(|&next|
                    (byte == b'\r' && next == b'\n') || (byte == b'\n' && next == b'\r'))
                { *cursor += 1; }
            }
            _ => {
                let width = character_width(&data[*cursor..]);
                value.extend_from_slice(&data[*cursor..*cursor + width]);
                *cursor += width;
            }
        }
        if value.len() >= 1024 { return Err(AnimationAliasError::FieldTooLong); }
    }
    // After a closing quote the native skips the remainder of this field;
    // it neither joins that suffix nor requires it to be whitespace.
    while *cursor < data.len() && !matches!(data[*cursor], b'\t' | b'\r' | b'\n') {
        *cursor += character_width(&data[*cursor..]);
    }
    if data.get(*cursor) == Some(&b'\t') { *cursor += 1; }
    Ok(value)
}

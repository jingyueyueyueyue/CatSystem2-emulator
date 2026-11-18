//! Ordinary 683/685's byte-oriented group/key/value table.
//! Recovered independently from 2.6.1.90 PE 0x6e7970/0x6e8040/0x6e7dd0.
//! Group names include '#'; matching is case-sensitive and duplicates append.
use std::collections::BTreeMap;

use cs2_script::Result;

use super::native_state_error;

const MAX_BYTES: usize = 64 * 1024 * 1024;
const MAX_ITEMS: usize = 1_000_000;
const MAX_FIELD: usize = 1023;

type Rows = Vec<(Vec<u8>, Vec<u8>)>;

#[derive(Debug, Clone, Default)]
pub(super) struct GroupPairs {
    groups: BTreeMap<Vec<u8>, Rows>,
}

impl GroupPairs {
    pub(super) fn snapshot(&self) -> cs2_script::data_snapshot::PairTableSnapshot {
        cs2_script::data_snapshot::PairTableSnapshot { groups: self.groups.clone() }
    }

    /// Validated together with the enclosing object group before commit.
    pub(super) fn from_snapshot(saved: &cs2_script::data_snapshot::PairTableSnapshot) -> Self {
        Self { groups: saved.groups.clone() }
    }

    pub(super) fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MAX_BYTES {
            return Err(native_state_error("native 683 pair table exceeds 64 MiB"));
        }
        let bytes = &bytes[..bytes.iter().position(|&byte| byte == 0).unwrap_or(bytes.len())];
        let mut result = Self::default();
        let mut current = None;
        let mut block_comment = false;
        let mut cursor = 0;
        let mut line = Vec::new();
        let mut items = 0usize;
        let mut stored = 0usize;
        // The original first splits physical lines, then strips comments
        // while carrying block state. Keep physical line boundaries even
        // inside comments so prefixes/suffixes never join across lines.
        while cursor <= bytes.len() {
            if cursor == bytes.len() || matches!(bytes[cursor], b'\r' | b'\n') {
                let normalized = normalize_spaces(&line);
                line.clear();
                if !normalized.is_empty() {
                    items += 1;
                    stored = stored.checked_add(normalized.len())
                        .filter(|&length| length <= MAX_BYTES)
                        .ok_or_else(|| native_state_error("native 683 table storage exceeds 64 MiB"))?;
                    if items > MAX_ITEMS {
                        return Err(native_state_error("native 683 table exceeds one million rows"));
                    }
                    if normalized[0] == b'#' {
                        result.groups.entry(normalized.clone()).or_default();
                        current = Some(normalized);
                    } else if let Some(group) = current.as_ref() {
                        let separator = normalized.iter().position(|&byte| byte == b' ');
                        let (key, value) = match separator {
                            Some(at) => (&normalized[..at], &normalized[at + 1..]),
                            None => (normalized.as_slice(), &[][..]),
                        };
                        if key.len() > MAX_FIELD || value.len() > MAX_FIELD {
                            return Err(native_state_error("native 683 table field exceeds 1023 bytes"));
                        }
                        result.groups.get_mut(group).expect("current group is registered")
                            .push((key.to_vec(), value.to_vec()));
                    }
                }
                if cursor == bytes.len() { break; }
                if bytes[cursor] == b'\r' && bytes.get(cursor + 1) == Some(&b'\n') {
                    cursor += 1;
                }
                cursor += 1;
            } else if block_comment {
                if bytes[cursor..].starts_with(b"*/") {
                    block_comment = false;
                    cursor += 2;
                } else {
                    cursor += 1;
                }
            } else if bytes[cursor..].starts_with(b"/*") {
                block_comment = true;
                cursor += 2;
            } else if bytes[cursor..].starts_with(b"//") {
                while cursor < bytes.len() && !matches!(bytes[cursor], b'\r' | b'\n') {
                    cursor += 1;
                }
            } else {
                line.push(bytes[cursor]);
                cursor += 1;
            }
        }
        Ok(result)
    }

    pub(super) fn query(&self, group: &[u8], key: &[u8], occurrence: u32) -> Option<&[u8]> {
        self.groups.get(group)?.iter()
            .filter(|(candidate, _)| candidate.as_slice() == key)
            .nth(occurrence as usize)
            .map(|(_, value)| value.as_slice())
    }
}

/// Tabs become spaces, then runs collapse and both ends are trimmed. Quotes
/// have no special role in either normalization or the key/value split.
fn normalize_spaces(bytes: &[u8]) -> Vec<u8> {
    let mut line = Vec::with_capacity(bytes.len());
    let mut pending_space = false;
    for &byte in bytes {
        if matches!(byte, b' ' | b'\t') {
            pending_space = !line.is_empty();
        } else {
            if pending_space { line.push(b' '); }
            pending_space = false;
            line.push(byte);
        }
    }
    line
}

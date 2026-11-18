//! Runtime debugger records and scenelist.csv rows. Original byte fields are
//! retained; this parser neither expands dialogue nor scans resource names.
use crate::{RuntimeError, RuntimeResult};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const MAX_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SceneDebugRecord {
    pub label: Vec<u8>,
    pub name: Vec<u8>,
    pub raw: Vec<u8>,
}

impl SceneDebugRecord {
    pub fn parse(bytes: &[u8]) -> RuntimeResult<Self> {
        let bytes = bytes.get(..0x160)
            .ok_or_else(|| invalid("scene-debug record needs 0x160 bytes"))?;
        Ok(Self {
            label: field(&bytes[..8])?,
            name: field(&bytes[8..0x108])?,
            raw: bytes.to_vec(),
        })
    }

    pub fn word(&self, offset: usize) -> Option<u32> {
        self.raw.get(offset..offset.checked_add(4)?)
            .map(|bytes| u32::from_le_bytes(bytes.try_into().unwrap()))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SceneListEntry {
    /// Original physical CSV line; filtering never renumbers this identity.
    pub line: u32,
    pub title: Vec<u8>,       // record+00, capacity64
    pub group: Vec<u8>,       // record+40, capacity32
    pub scene: Vec<u8>,       // record+60, capacity32
    pub value: u32,           // record+80
}

/// Type33 updates the list at the interpreter's prefix, not the CST table.
/// Original record: key DWORD followed by a 64-byte terminated name. The
/// list keeps insertion order and replaces the first equal key in place.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SceneSourceRecord {
    pub key: u32,
    pub name: Vec<u8>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SceneSourceTable {
    records: Vec<SceneSourceRecord>,
}

impl SceneSourceTable {
    pub fn records(&self) -> &[SceneSourceRecord] { &self.records }

    pub fn validate(&self) -> RuntimeResult<()> {
        if self.records.len() > MAX_BYTES / 68 {
            return Err(invalid("VM scene-source table exceeds 64 MiB"));
        }
        let mut keys = std::collections::BTreeSet::new();
        for record in &self.records {
            if record.name.len() > 63 || record.name.contains(&0) || !keys.insert(record.key) {
                return Err(invalid("VM scene-source table has an invalid record or duplicate key"));
            }
        }
        Ok(())
    }

    pub fn insert(&mut self, key: u32, name: &[u8]) -> RuntimeResult<bool> {
        if name.len() > 63 || name.contains(&0) {
            return Err(invalid("VM scene-source name exceeds its 64-byte terminated field"));
        }
        if let Some(record) = self.records.iter_mut().find(|record| record.key == key) {
            let changed = record.name != name;
            record.name = name.to_vec();
            return Ok(changed);
        }
        if self.records.len() >= MAX_BYTES / 68 {
            return Err(invalid("VM scene-source table exceeds 64 MiB"));
        }
        self.records.push(SceneSourceRecord { key, name: name.to_vec() });
        Ok(true)
    }
}

/// Addresses are local to `owner`'s VM. The VM snapshot owns the actual bytes;
/// this ledger preserves why the allocation must remain mapped after a
/// four-DWORD message has been consumed or copied by script code.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SceneStringLease {
    pub owner: u32,
    pub generation: u64,
    pub base: u32,
    pub length: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingSceneString {
    pub before: usize,
    pub packet: [u32; 4],
    pub bytes: Vec<u8>,
    pub address: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SceneSelectionContinuation {
    pub sender: u32,
    pub data0: u32,
    pub name: Vec<u8>,
    pub nodes: Vec<u32>,
    pub cursor: usize,
    pub enqueued: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SceneMessageMemorySnapshot {
    pub version: u32,
    pub next_generation: u64,
    pub generations: BTreeMap<u32, u64>,
    pub leases: BTreeMap<u32, BTreeMap<u32, SceneStringLease>>,
    pub pending: BTreeMap<u32, Vec<PendingSceneString>>,
    pub sources: BTreeMap<u32, SceneSourceTable>,
    pub selections: BTreeMap<u32, SceneSelectionContinuation>,
}

impl SceneMessageMemorySnapshot {
    /// VM existence and exact allocation boundaries are checked separately
    /// against the prepared VM group, before any host state is committed.
    pub fn validate(&self) -> RuntimeResult<()> {
        if self.version != 1 || self.generations.len() > 1_000_000
            || self.leases.len() > 1_000_000 || self.pending.len() > 1_000_000
            || self.sources.len() > 1_000_000
            || self.selections.len() > 1_000_000
        {
            return Err(invalid("scene-message memory version or owner count is invalid"));
        }
        let mut unique = std::collections::BTreeSet::new();
        if self.generations.values().any(|&generation|
            generation == 0 || generation > self.next_generation || !unique.insert(generation))
        {
            return Err(invalid("scene-message VM generation is invalid or duplicated"));
        }
        let mut count = 0usize;
        let mut bytes = 0usize;
        for (&owner, leases) in &self.leases {
            let generation = self.generations.get(&owner)
                .ok_or_else(|| invalid("scene-message lease has no VM generation"))?;
            for (&base, lease) in leases {
                if lease.owner != owner || lease.base != base || lease.generation != *generation
                    || base == 0 || base & 0x8000_0000 != 0 || !(2..=33).contains(&lease.length)
                    || base.checked_add(lease.length).is_none_or(|end| end > 0x8000_0000)
                {
                    return Err(invalid("scene-message allocation lease is invalid"));
                }
                count += 1;
                bytes = bytes.saturating_add(lease.length as usize);
            }
        }
        for (&owner, pending) in &self.pending {
            if pending.len() > 4096 {
                return Err(invalid("pending scene-message queue exceeds 4096 entries"));
            }
            let mut previous = 0usize;
            let mut addresses = std::collections::BTreeSet::new();
            for string in pending {
                if string.before < previous || string.before > 1_000_000
                    || string.bytes.len() > 31 || string.bytes.contains(&0)
                    || string.packet[1] != 0xffff_0010 || string.packet[3] != 0
                {
                    return Err(invalid("pending scene-message bytes or queue position is invalid"));
                }
                previous = string.before;
                if let Some(base) = string.address {
                    if !addresses.insert(base) {
                        return Err(invalid("two pending scene strings share one allocation lease"));
                    }
                    let lease = self.leases.get(&owner).and_then(|leases| leases.get(&base))
                        .ok_or_else(|| invalid("pending scene string lost its owning VM lease"))?;
                    if lease.length as usize != string.bytes.len() + 2 {
                        return Err(invalid("pending scene string differs from its allocation length"));
                    }
                } else {
                    count += 1;
                    bytes = bytes.saturating_add(string.bytes.len() + 2);
                }
            }
        }
        for table in self.sources.values() {
            table.validate()?;
            count = count.saturating_add(table.records().len());
            bytes = bytes.saturating_add(table.records().len().saturating_mul(68));
        }
        for selection in self.selections.values() {
            if selection.name.len() > 31 || selection.name.contains(&0)
                || selection.nodes.is_empty() || selection.cursor > selection.nodes.len()
                || selection.nodes.len() > 1_000_000
                || selection.nodes.iter().copied().collect::<std::collections::BTreeSet<_>>()
                    .len() != selection.nodes.len()
            {
                return Err(invalid("scene-selection continuation is invalid"));
            }
            count = count.saturating_add(selection.nodes.len());
            bytes = bytes.saturating_add(selection.nodes.len().saturating_mul(4))
                .saturating_add(selection.name.len());
        }
        if count > 1_000_000 || bytes > MAX_BYTES {
            return Err(invalid("scene-message/source memory exceeds its group storage limits"));
        }
        Ok(())
    }
}

pub fn parse_scene_list(bytes: &[u8]) -> RuntimeResult<Vec<SceneListEntry>> {
    if bytes.len() > MAX_BYTES { return Err(invalid("scene list exceeds 64 MiB")); }
    let bytes = bytes.split(|&byte| byte == 0).next().unwrap_or_default();
    let mut entries = Vec::new();
    for (line, source) in bytes.split(|&byte| byte == b'\n').enumerate() {
        if line >= 1_000_000 { return Err(invalid("scene list exceeds one million lines")); }
        let source = source.strip_suffix(b"\r").unwrap_or(source);
        if source.is_empty() || source[0] == b',' || source.starts_with(b"//") { continue; }
        // Original reader is delimiter based. Quotes are ordinary bytes;
        // replacing this with RFC CSV parsing would change the record ABI.
        let mut fields = source.split(|&byte| byte == b',');
        let title = fields.next().unwrap_or_default();
        let group = fields.next().unwrap_or_default();
        let scene = fields.next().unwrap_or_default();
        let value = fields.next().unwrap_or_default();
        if title.is_empty() || group.is_empty() { continue; }
        if title.len() > 62 || group.len() > 30 || scene.len() > 30 || value.len() > 30 {
            return Err(invalid("scene-list field exceeds its original fixed record"));
        }
        entries.push(SceneListEntry {
            line: line as u32, title: title.to_vec(), group: group.to_vec(), scene: scene.to_vec(),
            value: integer(value),
        });
    }
    Ok(entries)
}

fn field(bytes: &[u8]) -> RuntimeResult<Vec<u8>> {
    let end = bytes.iter().position(|&byte| byte == 0)
        .ok_or_else(|| invalid("scene-debug text field is not terminated"))?;
    Ok(bytes[..end].to_vec())
}

fn integer(bytes: &[u8]) -> u32 {
    let mut bytes = bytes.trim_ascii_start();
    let negative = bytes.first() == Some(&b'-');
    if matches!(bytes.first(), Some(b'+' | b'-')) { bytes = &bytes[1..]; }
    let (radix, bytes) = if bytes.starts_with(b"0x") || bytes.starts_with(b"0X") {
        (16u32, &bytes[2..])
    } else { (10u32, bytes) };
    let mut value = 0u32;
    for &byte in bytes {
        let digit = match byte {
            b'0'..=b'9' => u32::from(byte-b'0'),
            b'a'..=b'f' => u32::from(byte-b'a')+10,
            b'A'..=b'F' => u32::from(byte-b'A')+10,
            _ => break,
        };
        if digit >= radix { break; }
        value = value.wrapping_mul(radix).wrapping_add(digit);
    }
    if negative { value.wrapping_neg() } else { value }
}

fn invalid(message: &str) -> RuntimeError { RuntimeError::Host(message.into()) }

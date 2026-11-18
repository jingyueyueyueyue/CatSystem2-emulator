//! Task-state tree wire format used by ordinary 688/689. Native linked-list
//! pointers are replaced by relative byte offsets in each 12-byte record.
//! Node payloads remain opaque until the owning object decodes them.
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

const MAX_BYTES: usize = 64 * 1024 * 1024;
const MAX_NODES: usize = 1_000_000;
const MAX_DEPTH: usize = 256;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StateTree {
    pub roots: Vec<StateNode>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StateNode {
    pub payload: Vec<u8>,
    pub children: Vec<StateNode>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum StateTreeError {
    #[error("task-state tree is truncated at byte {0}")]
    Truncated(usize),
    #[error("task-state tree has an invalid relative link at byte {0}")]
    InvalidLink(usize),
    #[error("task-state tree contains overlapping records at byte {0}")]
    Overlap(usize),
    #[error("task-state tree exceeds its size, node or depth bound")]
    Limit,
}

impl StateTree {
    pub fn decode(bytes: &[u8]) -> Result<Self, StateTreeError> {
        if bytes.len() > MAX_BYTES { return Err(StateTreeError::Limit); }
        if bytes.is_empty() { return Ok(Self::default()); }
        let mut reader = Reader {
            bytes, visited: BTreeSet::new(), spans: BTreeMap::new(),
        };
        let roots = reader.siblings(0, 0)?;
        // The serializer emits a packed tree. Gaps/extra bytes are not an
        // alternative payload for a later node and must not hide corrupt data.
        let mut cursor = 0;
        for (begin, end) in reader.spans {
            if begin != cursor {
                return Err(if begin < cursor { StateTreeError::Overlap(begin) }
                           else { StateTreeError::InvalidLink(begin) });
            }
            cursor = end;
        }
        if cursor != bytes.len() { return Err(StateTreeError::InvalidLink(cursor)); }
        Ok(Self { roots })
    }

    pub fn encode(&self) -> Result<Vec<u8>, StateTreeError> {
        let mut bytes = Vec::new();
        let mut count = 0;
        emit_siblings(&self.roots, 0, &mut count, &mut bytes)?;
        Ok(bytes)
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    visited: BTreeSet<usize>,
    spans: BTreeMap<usize, usize>,
}

impl Reader<'_> {
    fn siblings(&mut self, mut at: usize, depth: usize) -> Result<Vec<StateNode>, StateTreeError> {
        if depth >= MAX_DEPTH { return Err(StateTreeError::Limit); }
        let mut nodes = Vec::new();
        loop {
            if self.visited.len() >= MAX_NODES { return Err(StateTreeError::Limit); }
            if !self.visited.insert(at) { return Err(StateTreeError::InvalidLink(at)); }
            let next = self.word(at)? as usize;
            let child = self.word(at.checked_add(4).ok_or(StateTreeError::Limit)?)? as usize;
            let length = self.word(at.checked_add(8).ok_or(StateTreeError::Limit)?)? as usize;
            let begin = at.checked_add(12).ok_or(StateTreeError::Limit)?;
            let end = begin.checked_add(length).ok_or(StateTreeError::Limit)?;
            if self.spans.range(..=at).next_back().is_some_and(|(_, &previous_end)| previous_end > at)
                || self.spans.range(at..).next().is_some_and(|(&next_begin, _)| next_begin < end)
            {
                return Err(StateTreeError::Overlap(at));
            }
            self.spans.insert(at, end);
            let payload = self.bytes.get(begin..end).ok_or(StateTreeError::Truncated(at))?.to_vec();
            let children = if child == 0 { Vec::new() } else {
                if child < 12 + length { return Err(StateTreeError::InvalidLink(at)); }
                self.siblings(at.checked_add(child).ok_or(StateTreeError::Limit)?, depth + 1)?
            };
            nodes.push(StateNode { payload, children });
            if next == 0 { break; }
            if next < 12 + length { return Err(StateTreeError::InvalidLink(at)); }
            at = at.checked_add(next).ok_or(StateTreeError::Limit)?;
        }
        Ok(nodes)
    }

    fn word(&self, at: usize) -> Result<u32, StateTreeError> {
        let end = at.checked_add(4).ok_or(StateTreeError::Limit)?;
        Ok(u32::from_le_bytes(self.bytes.get(at..end)
            .ok_or(StateTreeError::Truncated(at))?.try_into().unwrap()))
    }
}

fn emit_siblings(
    nodes: &[StateNode], depth: usize, count: &mut usize, bytes: &mut Vec<u8>,
) -> Result<(), StateTreeError> {
    if nodes.is_empty() { return Ok(()); }
    if depth >= MAX_DEPTH { return Err(StateTreeError::Limit); }
    for (index, node) in nodes.iter().enumerate() {
        *count = count.checked_add(1).ok_or(StateTreeError::Limit)?;
        if *count > MAX_NODES { return Err(StateTreeError::Limit); }
        let header = bytes.len();
        let local = 12_usize.checked_add(node.payload.len()).ok_or(StateTreeError::Limit)?;
        let end = header.checked_add(local).filter(|&end| end <= MAX_BYTES).ok_or(StateTreeError::Limit)?;
        bytes.try_reserve(local).map_err(|_| StateTreeError::Limit)?;
        bytes.resize(header + 12, 0);
        bytes.extend_from_slice(&node.payload);
        debug_assert_eq!(bytes.len(), end);
        let child = if node.children.is_empty() { 0 } else { local as u32 };
        bytes[header + 4..header + 8].copy_from_slice(&child.to_le_bytes());
        bytes[header + 8..header + 12].copy_from_slice(&(node.payload.len() as u32).to_le_bytes());
        emit_siblings(&node.children, depth + 1, count, bytes)?;
        if index + 1 != nodes.len() {
            let next = u32::try_from(bytes.len() - header).map_err(|_| StateTreeError::Limit)?;
            bytes[header..header + 4].copy_from_slice(&next.to_le_bytes());
        }
    }
    Ok(())
}

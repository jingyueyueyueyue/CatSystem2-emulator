//! Runtime ZT export source. This is the type45 file tree, not a KIF archive.
//! Node links use the shared StateTree wire format; the typed payload is
//! kind:u32, name:[u8;260], compressed_size:u32, plain_size:u32, zlib bytes.
use std::collections::VecDeque;

use cs2_formats::state_tree::{StateNode, StateTree};
use flate2::{Decompress, FlushDecompress, Status};

use crate::{RuntimeError, RuntimeResult};

const MAX_BYTES: usize = 64 * 1024 * 1024;
const MAX_ITEMS: usize = 1_000_000;
const HEADER: usize = 0x110;

#[derive(Clone, Debug)]
pub enum ExportEntry {
    Directory { path: Vec<u8> },
    File { path: Vec<u8>, bytes: Vec<u8> },
}

struct PendingEntry {
    path: Vec<u8>,
    compressed: Vec<u8>,
    plain_size: usize,
    directory: bool,
}

/// Successful writes are acknowledged individually. An I/O failure can retry
/// the same entry without repeating already committed output files.
pub struct ExportWork {
    pending: VecDeque<PendingEntry>,
    ready: Option<ExportEntry>,
    completed: usize,
}

impl ExportWork {
    pub fn decode(bytes: &[u8]) -> RuntimeResult<Self> {
        if bytes.is_empty() {
            return Err(error_text("export source has no tree record".into()));
        }
        let tree = StateTree::decode(bytes).map_err(|error| error_text(error.to_string()))?;
        let mut pending = VecDeque::new();
        let mut plain_total = 0usize;
        collect(&tree.roots, b"", &mut pending, &mut plain_total, 0)?;
        Ok(Self {
            pending,
            ready: None,
            completed: 0,
        })
    }

    /// Decode only the next file. The original receiver writes one complete
    /// inflated file at a time; no pointer to compressed input escapes.
    pub fn next_entry(&mut self) -> RuntimeResult<Option<&ExportEntry>> {
        if self.ready.is_none() {
            let Some(next) = self.pending.front() else {
                return Ok(None);
            };
            self.ready = Some(if next.directory {
                ExportEntry::Directory {
                    path: next.path.clone(),
                }
            } else {
                ExportEntry::File {
                    path: next.path.clone(),
                    bytes: inflate(&next.compressed, next.plain_size)?,
                }
            });
        }
        Ok(self.ready.as_ref())
    }

    pub fn acknowledge(&mut self) -> RuntimeResult<()> {
        if self.ready.take().is_none() {
            return Err(error_text(
                "export acknowledgement has no completed file".into(),
            ));
        }
        self.pending.pop_front();
        self.completed += 1;
        Ok(())
    }

    pub fn completed_entries(&self) -> usize {
        self.completed
    }
    pub fn finished(&self) -> bool {
        self.pending.is_empty() && self.ready.is_none()
    }
}

fn collect(
    nodes: &[StateNode],
    prefix: &[u8],
    output: &mut VecDeque<PendingEntry>,
    plain_total: &mut usize,
    depth: usize,
) -> RuntimeResult<()> {
    if depth >= 256 {
        return Err(error_text("export directory depth exceeds 256".into()));
    }
    // The reader inserts each decoded sibling at the head, then enumeration
    // follows +0xc. Thus its visible order reverses the wire sibling order.
    for node in nodes.iter().rev() {
        if output.len() >= MAX_ITEMS {
            return Err(error_text("export item limit exceeded".into()));
        }
        let bytes = &node.payload;
        if bytes.len() < HEADER {
            return Err(error_text("export node payload is truncated".into()));
        }
        let kind = word(bytes, 0);
        // The concrete traversal ignores types other than file0/directory1.
        if kind > 1 {
            continue;
        }
        let name = &bytes[4..0x108];
        let end = name
            .iter()
            .position(|&byte| byte == 0)
            .ok_or_else(|| error_text("export node name lacks its 260-byte terminator".into()))?;
        let name = &name[..end];
        let mut path = prefix.to_vec();
        if !path.is_empty() {
            path.push(b'/');
        }
        path.extend_from_slice(name);
        if path.len() > 4095 {
            return Err(error_text("export path exceeds its storage limit".into()));
        }
        validate_path(&path)?;
        let compressed_size = word(bytes, 0x108) as usize;
        let plain_size = word(bytes, 0x10c) as usize;
        if kind == 1 {
            output.push_back(PendingEntry {
                path: path.clone(),
                compressed: Vec::new(),
                plain_size: 0,
                directory: true,
            });
            collect(&node.children, &path, output, plain_total, depth + 1)?;
        } else {
            let end = HEADER
                .checked_add(compressed_size)
                .filter(|&end| end <= bytes.len())
                .ok_or_else(|| {
                    error_text("export compressed member exceeds its node payload".into())
                })?;
            *plain_total = plain_total
                .checked_add(plain_size)
                .filter(|&total| total <= MAX_BYTES)
                .ok_or_else(|| error_text("export inflated members exceed 64 MiB".into()))?;
            output.push_back(PendingEntry {
                path,
                compressed: bytes[HEADER..end].to_vec(),
                plain_size,
                directory: false,
            });
        }
    }
    Ok(())
}

fn inflate(bytes: &[u8], length: usize) -> RuntimeResult<Vec<u8>> {
    if length == 0 {
        return Ok(Vec::new());
    }
    let mut result = vec![
        0;
        length
            .checked_add(1)
            .ok_or_else(|| error_text("export file size overflow".into()))?
    ];
    let mut decoder = Decompress::new(true);
    let status = decoder
        .decompress(bytes, &mut result, FlushDecompress::Finish)
        .map_err(|error| error_text(format!("export zlib member: {error}")))?;
    if status != Status::StreamEnd
        || decoder.total_out() != length as u64
        || decoder.total_in() != bytes.len() as u64
    {
        return Err(error_text(
            "export zlib member length or end marker is invalid".into(),
        ));
    }
    result.truncate(length);
    Ok(result)
}

fn validate_path(path: &[u8]) -> RuntimeResult<()> {
    // Walk CP932 byte pairs so a trail byte 0x5c remains part of its name.
    let mut component = Vec::new();
    let mut at = 0;
    while at < path.len() {
        let byte = path[at];
        if matches!(byte, 0x81..=0x9f | 0xe0..=0xfc) && at + 1 < path.len() {
            component.extend_from_slice(&path[at..at + 2]);
            at += 2;
            continue;
        }
        if matches!(byte, b'/' | b'\\') {
            check_component(&component)?;
            component.clear();
        } else {
            if byte == b':' || byte == 0 {
                return Err(error_text("export path is not relative".into()));
            }
            component.push(byte);
        }
        at += 1;
    }
    check_component(&component)
}

fn check_component(component: &[u8]) -> RuntimeResult<()> {
    if component.is_empty() || matches!(component, b"." | b"..") {
        return Err(error_text(
            "export path has an empty or parent component".into(),
        ));
    }
    Ok(())
}
fn word(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}
fn error_text(message: String) -> RuntimeError {
    RuntimeError::Host(message)
}

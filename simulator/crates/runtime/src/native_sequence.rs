//! Ordinary-native sequence trees. Independently implemented from the fixed
//! reference documentation; these objects contain no process pointers.
use std::collections::BTreeMap;

use cs2_script::{Result, ScriptError};
use cs2_script::data_snapshot::{SequenceSnapshot, SequenceNodeSnapshot};

const MAX_ITEMS: usize = 1_000_000;
const MAX_BYTES: usize = 64 * 1024 * 1024;
const MAX_DEPTH: usize = 256;

#[derive(Debug, Clone, Default)]
struct Edge {
    children: BTreeMap<i32, Edge>,
}

#[derive(Debug, Clone)]
pub struct NativeSequence {
    names: BTreeMap<Vec<u8>, Edge>,
    file: Vec<u8>,
    record_span: usize,
    nodes: usize,
}

impl Default for NativeSequence {
    fn default() -> Self {
        Self {
            names: BTreeMap::new(),
            file: Vec::new(),
            record_span: 256,
            nodes: 0,
        }
    }
}

impl NativeSequence {
    pub fn snapshot(&self) -> Result<SequenceSnapshot> {
        fn flatten(edge: &Edge, parent: Option<u32>, key: i32, nodes: &mut Vec<SequenceNodeSnapshot>) {
            let index = nodes.len() as u32;
            nodes.push(SequenceNodeSnapshot { parent, key });
            for (&key, child) in &edge.children {
                flatten(child, Some(index), key, nodes);
            }
        }
        let mut nodes = Vec::with_capacity(self.nodes);
        let mut roots = BTreeMap::new();
        for (name, edge) in &self.names {
            roots.insert(name.clone(), nodes.len() as u32);
            flatten(edge, None, 0, &mut nodes);
        }
        if nodes.len() != self.nodes {
            return Err(error("sequence node counter differs from its live tree"));
        }
        let snapshot = SequenceSnapshot { roots, nodes, file: self.file.clone(),
            record_span: self.record_span as u32 };
        snapshot.storage_bytes()?;
        Ok(snapshot)
    }

    pub fn from_snapshot(snapshot: &SequenceSnapshot) -> Result<Self> {
        snapshot.storage_bytes()?;
        let mut edges = vec![Edge::default(); snapshot.nodes.len()];
        for (index, node) in snapshot.nodes.iter().enumerate().rev() {
            if let Some(parent) = node.parent {
                let child = std::mem::take(&mut edges[index]);
                edges[parent as usize].children.insert(node.key, child);
            }
        }
        let names = snapshot.roots.iter().map(|(name, &index)|
            (name.clone(), std::mem::take(&mut edges[index as usize]))).collect();
        Ok(Self { names, file: snapshot.file.clone(), record_span: snapshot.record_span as usize,
            nodes: snapshot.nodes.len() })
    }

    /// Clearing the tree leaves the separately loaded file intact.
    pub fn clear(&mut self) {
        self.names.clear();
        self.nodes = 0;
    }

    pub fn register(&mut self, text: &[u8], name_mode: u32) -> Result<()> {
        let Some((name, path)) = sequence_parts(text)? else {
            return Ok(());
        };
        self.insert(&name, &path)?;
        if let Some(index) = (2..name.len())
            .find(|&index| name[index] == b'_' && matches!(name[index - 1], b'l' | b'L'))
        {
            let mut alias = name;
            match name_mode {
                0 => {
                    alias.remove(index - 1);
                }
                1 => alias[index - 1] = b'm',
                _ => return Ok(()),
            }
            self.insert(&alias, &path)?;
        }
        Ok(())
    }

    pub fn contains(&self, text: &[u8]) -> Result<bool> {
        let Some((name, path)) = sequence_parts(text)? else {
            return Ok(false);
        };
        let Some(mut edge) = self.names.get(&name) else {
            return Ok(false);
        };
        for (index, &key) in path.iter().enumerate() {
            if key == -1 && index + 1 == path.len() {
                // The query sentinel and a literal 0xff share an edge.
                // A query ending in 0xff must not require a second sentinel.
                return Ok(edge.children.contains_key(&-1));
            }
            let Some(next) = edge.children.get(&key) else {
                return Ok(false);
            };
            edge = next;
        }
        // -1 is a real tree edge, shared with the signed byte 0xff.
        Ok(edge.children.contains_key(&-1))
    }

    fn insert(&mut self, name: &[u8], path: &[i32]) -> Result<()> {
        if !self.names.contains_key(name) {
            reserve_node(&mut self.nodes)?;
        }
        let mut edge = self.names.entry(name.to_vec()).or_default();
        for key in path.iter().copied().chain(std::iter::once(-1)) {
            if !edge.children.contains_key(&key) {
                reserve_node(&mut self.nodes)?;
            }
            edge = edge.children.entry(key).or_default();
        }
        Ok(())
    }

    /// Empty/missing resource reads are handled by the caller and do not
    /// discard the previous file. The four-byte marker alone is legacy data.
    pub fn load_file(&mut self, bytes: Vec<u8>) -> Result<()> {
        if bytes.len() > MAX_BYTES {
            return Err(error("sequence file exceeds 64 MiB"));
        }
        if bytes.is_empty() {
            return Ok(());
        }
        if bytes.len() > 4 && bytes.starts_with(b"CGL\0") {
            self.file = bytes[4..].to_vec();
            self.record_span = 256;
        } else {
            self.file = bytes;
            self.record_span = 64;
        }
        Ok(())
    }

    /// Import only the first group/name pair. Relative links are measured
    /// from the header holding that link, and may not run backwards.
    pub fn import_group(&mut self, group: u32, name: &[u8]) -> Result<()> {
        if name.len() >= 260 {
            return Err(error("sequence group name exceeds 259 bytes"));
        }
        if self.file.is_empty() {
            return Ok(());
        }
        let mut group_offset = 0usize;
        let mut visited = 0usize;
        loop {
            visited += 1;
            if visited > MAX_ITEMS {
                return Err(error("sequence file has too many linked groups"));
            }
            let header = range(&self.file, group_offset, 12)?;
            let next_group = word(header, 0)?;
            let group_id = word(header, 4)?;
            let item_count = word(header, 8)? as i32;
            if group_id == group {
                let mut item_offset = group_offset + 12;
                for _ in 0..item_count.max(0) as usize {
                    visited += 1;
                    if visited > MAX_ITEMS {
                        return Err(error("sequence file has too many linked items"));
                    }
                    let header = range(&self.file, item_offset, 72)?;
                    let next_item = word(header, 0)?;
                    let item_name = zstring(&header[4..68])?;
                    let record_count = word(header, 68)? as i32;
                    if item_name.eq_ignore_ascii_case(name) {
                        let mut record_offset = item_offset + 72;
                        for _ in 0..record_count.max(0) as usize {
                            if visited >= MAX_ITEMS {
                                return Err(error("sequence file has too many records"));
                            }
                            visited += 1;
                            let text =
                                zstring(range(&self.file, record_offset, self.record_span)?)?;
                            let parts = sequence_parts(text)?;
                            if let Some((key, path)) = parts {
                                // File import never adds the L-name variant.
                                self.insert(&key, &path)?;
                            }
                            record_offset = record_offset
                                .checked_add(self.record_span)
                                .ok_or_else(|| error("sequence record offset overflows"))?;
                        }
                        return Ok(());
                    }
                    if next_item == 0 {
                        break;
                    }
                    item_offset = linked_offset(item_offset, next_item, 72)?;
                }
            }
            if next_group == 0 {
                return Ok(());
            }
            group_offset = linked_offset(group_offset, next_group, 12)?;
        }
    }

    /// Flatten the gallery table in file order, retaining the category IDs
    /// used by named lookups. The final item in each category has a zero link.
    pub fn gallery_entries(&self) -> Result<Vec<GalleryEntry>> {
        let mut entries = Vec::new();
        let mut category = 0;
        let mut visited = 0;
        while category < self.file.len() {
            let header = range(&self.file, category, 12)?;
            let next_category = word(header, 0)?;
            let id = word(header, 4)?;
            let mut item = category + 12;
            for _ in 0..(word(header, 8)? as i32).max(0) {
                let header = range(&self.file, item, 72)?;
                let next_item = word(header, 0)?;
                let name = zstring(&header[4..68])?.to_vec();
                let count = (word(header, 68)? as i32).max(0) as usize;
                visited += 1 + count;
                if visited > MAX_ITEMS {
                    return Err(error("gallery file has too many items and variants"));
                }
                let records = range(&self.file, item + 72, count * self.record_span)?;
                let variants = records.chunks_exact(self.record_span).map(|record| {
                    let name = zstring(record)?;
                    Ok(GalleryVariant { name: name.to_vec(), registered: self.contains(name)? })
                }).collect::<Result<_>>()?;
                entries.push(GalleryEntry { category: id, name, variants });
                if next_item == 0 { break; }
                item = linked_offset(item, next_item, 72)?;
            }
            if next_category == 0 { break; }
            category = linked_offset(category, next_category, 12)?;
            range(&self.file, category, 12)?;
        }
        Ok(entries)
    }

    pub fn save_tree(&self) -> Result<Vec<u8>> {
        let mut bytes = Vec::new();
        for (name, root) in &self.names {
            if name.len() >= 64 {
                return Err(error("sequence save name exceeds 63 bytes"));
            }
            let start = grow(&mut bytes, 68)?;
            bytes[start..start + 4].copy_from_slice(&(root.children.len() as u32).to_le_bytes());
            bytes[start + 4..start + 4 + name.len()].copy_from_slice(name);
            save_edges(&root.children, &mut bytes, 0)?;
        }
        Ok(bytes)
    }

    /// Restore clears before parsing and keeps completed prefixes on failure.
    /// Duplicate names stop importing successfully; duplicate edges diagnose.
    pub fn restore_tree(&mut self, bytes: &[u8]) -> Result<()> {
        self.clear();
        if bytes.len() > MAX_BYTES {
            return Err(error("sequence save record exceeds 64 MiB"));
        }
        let mut cursor = 0usize;
        while cursor < bytes.len() {
            let header = range(bytes, cursor, 68)?;
            let count = word(header, 0)? as i32;
            let name = zstring(&header[4..68])?.to_vec();
            if self.names.contains_key(&name) {
                return Ok(());
            }
            reserve_node(&mut self.nodes)?;
            let root = self.names.entry(name).or_default();
            cursor += 68;
            read_edges(bytes, &mut cursor, count, root, &mut self.nodes, 0)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct GalleryEntry {
    pub category: u32,
    pub name: Vec<u8>,
    pub variants: Vec<GalleryVariant>,
}

#[derive(Debug, Clone)]
pub struct GalleryVariant {
    pub name: Vec<u8>,
    pub registered: bool,
}

fn sequence_parts(text: &[u8]) -> Result<Option<(Vec<u8>, Vec<i32>)>> {
    let text = &text[..text
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(text.len())];
    if text.len() > 254 {
        return Err(error("sequence text exceeds 254 bytes"));
    }
    // A legal baseline double-byte trail cannot be a comma, so splitting
    // commas directly also preserves the documented double-byte path.
    let parts: Vec<_> = text
        .split(|&byte| byte == b',')
        .filter(|part| !part.is_empty())
        .collect();
    if parts.len() > 64 {
        return Err(error("sequence text exceeds 64 components"));
    }
    let Some(first) = parts.first() else {
        return Ok(None);
    };
    let mut name = first.to_vec();
    if let Some(second) = parts.get(1) {
        name.push(b'_');
        name.extend_from_slice(second);
    }
    let path = parts
        .iter()
        .skip(2)
        .map(|part| i32::from(part[0] as i8))
        .collect();
    Ok(Some((name, path)))
}

fn save_edges(children: &BTreeMap<i32, Edge>, bytes: &mut Vec<u8>, depth: usize) -> Result<()> {
    if depth >= MAX_DEPTH && !children.is_empty() {
        return Err(error("sequence save tree exceeds 256 levels"));
    }
    for (&key, edge) in children {
        let start = grow(bytes, 8)?;
        bytes[start..start + 4].copy_from_slice(&(edge.children.len() as u32).to_le_bytes());
        bytes[start + 4..start + 8].copy_from_slice(&key.to_le_bytes());
        save_edges(&edge.children, bytes, depth + 1)?;
    }
    Ok(())
}

fn read_edges(
    bytes: &[u8],
    cursor: &mut usize,
    count: i32,
    parent: &mut Edge,
    nodes: &mut usize,
    depth: usize,
) -> Result<()> {
    if depth >= MAX_DEPTH && count > 0 {
        return Err(error("sequence save tree exceeds 256 levels"));
    }
    for _ in 0..count.max(0) as usize {
        let header = range(bytes, *cursor, 8)?;
        let children = word(header, 0)? as i32;
        let key = word(header, 4)? as i32;
        if parent.children.contains_key(&key) {
            return Err(error("sequence save contains a duplicate edge"));
        }
        reserve_node(nodes)?;
        let edge = parent.children.entry(key).or_default();
        *cursor += 8;
        read_edges(bytes, cursor, children, edge, nodes, depth + 1)?;
    }
    Ok(())
}

fn reserve_node(nodes: &mut usize) -> Result<()> {
    if *nodes >= MAX_ITEMS {
        return Err(error("sequence tree exceeds one million nodes"));
    }
    *nodes += 1;
    Ok(())
}

fn grow(bytes: &mut Vec<u8>, length: usize) -> Result<usize> {
    let start = bytes.len();
    let end = start
        .checked_add(length)
        .filter(|&end| end <= MAX_BYTES)
        .ok_or_else(|| error("sequence save exceeds 64 MiB"))?;
    bytes.resize(end, 0);
    Ok(start)
}

fn linked_offset(offset: usize, relative: u32, minimum: u32) -> Result<usize> {
    if relative < minimum {
        return Err(error("sequence file contains an invalid relative link"));
    }
    offset
        .checked_add(relative as usize)
        .ok_or_else(|| error("sequence file link overflows"))
}

fn zstring(bytes: &[u8]) -> Result<&[u8]> {
    let end = bytes
        .iter()
        .position(|&byte| byte == 0)
        .ok_or_else(|| error("sequence file string is not terminated"))?;
    Ok(&bytes[..end])
}

fn range(bytes: &[u8], offset: usize, length: usize) -> Result<&[u8]> {
    let end = offset
        .checked_add(length)
        .ok_or_else(|| error("sequence offset overflows"))?;
    bytes
        .get(offset..end)
        .ok_or_else(|| error("sequence data is truncated"))
}

fn word(bytes: &[u8], offset: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(
        range(bytes, offset, 4)?.try_into().unwrap(),
    ))
}

fn error(message: &'static str) -> ScriptError {
    ScriptError::InvalidStructuredText { offset: 0, message }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signed_terminal_edges_and_name_aliases_are_not_vector_equality() {
        let mut sequence = NativeSequence::default();
        sequence.register(b"gal,01,10", 0).unwrap();
        assert!(sequence.contains(b"gal,01,1").unwrap());
        assert!(sequence.contains(b"ga,01,1").unwrap());
        assert!(sequence.contains(b"gal,01,1,\xff").unwrap());
        assert!(!sequence.contains(b"gal,01").unwrap());
        sequence.register(b"prefix,key,\xff", 2).unwrap();
        assert!(sequence.contains(b"prefix,key").unwrap());
        assert!(!sequence.contains(b"GA,01,1").unwrap());
    }

    #[test]
    fn restore_preserves_full_i32_edges_and_completed_prefix_on_failure() {
        let mut bytes = vec![0; 68];
        bytes[..4].copy_from_slice(&2u32.to_le_bytes());
        bytes[4..7].copy_from_slice(b"key");
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&(-1i32).to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&0x1234_5678u32.to_le_bytes());
        let mut sequence = NativeSequence::default();
        sequence.restore_tree(&bytes).unwrap();
        assert!(sequence.contains(b"key").unwrap());
        assert_eq!(sequence.save_tree().unwrap(), bytes);
        bytes.pop();
        assert!(sequence.restore_tree(&bytes).is_err());
        assert!(sequence.contains(b"key").unwrap());
    }

    #[test]
    fn file_data_survives_clear_and_import_uses_relative_links() {
        let mut bytes = vec![0; 12 + 72 + 64];
        bytes[8..12].copy_from_slice(&1u32.to_le_bytes());
        bytes[16..19].copy_from_slice(b"BG1");
        bytes[80..84].copy_from_slice(&1u32.to_le_bytes());
        bytes[84..90].copy_from_slice(b"a,b,10");
        let mut sequence = NativeSequence::default();
        sequence.load_file(bytes).unwrap();
        sequence.import_group(0, b"bg1").unwrap();
        assert!(sequence.contains(b"a,b,1").unwrap());
        sequence.clear();
        assert!(!sequence.contains(b"a,b,1").unwrap());
        sequence.import_group(0, b"Bg1").unwrap();
        assert!(sequence.contains(b"a,b,1").unwrap());
    }
}

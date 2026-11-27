//! Complete persistent script-owned data objects in an application's save
//! graph. Native CSS snapshots remain their original used-only wire format.
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use cs2_formats::CssFile;
use crate::{Result, ScriptError, cst_snapshot::{CstContextStoreSnapshot, CstDocumentSnapshot}};

const MAX_BYTES: usize = 256 * 1024 * 1024;
const MAX_ITEMS: usize = 1_000_000;

fn invalid(message: &'static str) -> ScriptError {
    ScriptError::InvalidStructuredText { offset: 0, message }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CssObjectSnapshot {
    pub handle: u32,
    pub tag: u32,
    pub logical_capacity: u32,
    pub used_length: u32,
    #[serde(with = "wire_bytes")]
    pub allocation: Vec<u8>,
}

/// A byte sequence encoded as its length and bytes by the host graph codec.
pub mod wire_bytes {
    use serde::{Deserializer, Serializer, de};

    pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bytes(bytes)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        struct BytesVisitor;

        impl<'de> de::Visitor<'de> for BytesVisitor {
            type Value = Vec<u8>;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a byte sequence")
            }

            fn visit_bytes<E: de::Error>(self, bytes: &[u8]) -> Result<Vec<u8>, E> {
                let mut output = Vec::new();
                output.try_reserve_exact(bytes.len()).map_err(E::custom)?;
                output.extend_from_slice(bytes);
                Ok(output)
            }

            fn visit_byte_buf<E: de::Error>(self, bytes: Vec<u8>) -> Result<Vec<u8>, E> {
                Ok(bytes)
            }

            fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<u8>, A::Error> {
                let mut output = Vec::new();
                while let Some(byte) = seq.next_element()? {
                    output.push(byte);
                }
                Ok(output)
            }
        }

        deserializer.deserialize_byte_buf(BytesVisitor)
    }
}

impl CssObjectSnapshot {
    pub fn capture(handle: u32, file: &CssFile) -> Self {
        Self { handle, tag: file.tag(), logical_capacity: file.capacity(),
            used_length: file.used_len(), allocation: file.storage().to_vec() }
    }

    pub fn restore(&self) -> Result<CssFile> {
        if self.handle == 0 { return Err(invalid("CSS object snapshot has a zero handle")); }
        if self.allocation.len() > cs2_formats::save::MAX_HOST_GRAPH_CSS_BYTES
            || self.logical_capacity as usize > self.allocation.len()
        {
            return Err(invalid("CSS object snapshot allocation/capacity is invalid"));
        }
        CssFile::from_storage_state(self.tag, self.logical_capacity, self.used_length,
            self.allocation.clone())
            .map_err(|_| invalid("CSS object snapshot has invalid capacity or record storage"))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptDataObjectsSnapshot {
    pub version: u32,
    pub contexts: CstContextStoreSnapshot,
    pub containers: Vec<CssObjectSnapshot>,
    /// Keep stored spans, including any bytes beyond an embedded NUL. Native
    /// 243 uses those lengths even though consumers stop at the first NUL.
    pub shared_strings: BTreeMap<u32, Vec<u8>>,
    #[serde(default)]
    pub databases: Vec<DatabaseObjectSnapshot>,
    #[serde(default)]
    pub cst_windows: Vec<CstWindowSnapshot>,
    /// Ordered string maps store both integers and float IEEE bits as DWORDs.
    #[serde(default)]
    pub ordered_maps: BTreeMap<u32, BTreeMap<Vec<u8>, u32>>,
    #[serde(default)]
    pub raw_blocks: Vec<RawBlockSnapshot>,
    pub triple_tables: BTreeMap<u32, TripleTableSnapshot>,
    pub name_tables: BTreeMap<u32, Vec<NameRowSnapshot>>,
    pub sequences: BTreeMap<u32, SequenceSnapshot>,
    /// Original resource bytes, including resources not decoded yet.
    pub raw_images: BTreeMap<u32, Vec<u8>>,
    pub pair_tables: BTreeMap<u32, PairTableSnapshot>,
    pub record_stacks: BTreeMap<u32, RecordStackSnapshot>,
    pub xml_documents: BTreeMap<u32, XmlDocumentSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordStackSnapshot {
    pub stride: u32,
    pub count: u32,
    pub growth: u32,
    pub capacity: u32,
    /// Entire allocation includes popped rows and retained spare capacity.
    pub bytes: Vec<u8>,
}

impl RecordStackSnapshot {
    pub fn storage_bytes(&self) -> Result<usize> {
        let length = (self.stride as usize).checked_mul(self.capacity as usize);
        if length != Some(self.bytes.len()) || self.bytes.len() > 64 * 1024 * 1024
            || self.stride != 0 && self.count > self.capacity
        { return Err(invalid("record stack snapshot allocation or count is invalid")); }
        Ok(self.bytes.len())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct XmlDocumentSnapshot {
    pub root: Vec<u8>,
    pub fields: Vec<XmlFieldSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct XmlFieldSnapshot {
    pub path: Vec<Vec<u8>>,
    pub text: Vec<u8>,
}

impl XmlDocumentSnapshot {
    pub fn storage_bytes(&self) -> Result<usize> {
        if self.root.is_empty() || self.root.len() > 511 || self.fields.len() > MAX_ITEMS {
            return Err(invalid("XML snapshot root or field count is invalid"));
        }
        let mut paths = std::collections::BTreeSet::new();
        let mut total = self.root.len();
        let mut path_bytes = 0usize;
        for field in &self.fields {
            if field.path.is_empty() || field.path.len() > 256 || field.path.first() != Some(&self.root)
                || field.path.iter().any(|name| name.is_empty() || name.len() > 511)
                || !paths.insert(&field.path)
            { return Err(invalid("XML snapshot has an invalid or duplicate rooted path")); }
            for name in &field.path {
                path_bytes = bounded_storage(path_bytes, name.len(), 64 * 1024 * 1024)?;
            }
            total = bounded_storage(total, field.text.len(), 64 * 1024 * 1024)?;
        }
        bounded_storage(total, path_bytes, MAX_BYTES)
    }

    pub fn capture(document: &cs2_formats::ConfigXml) -> Self {
        Self { root: document.root.clone(), fields: document.fields.iter().map(|field|
            XmlFieldSnapshot { path: field.path.clone(), text: field.text.clone() }).collect() }
    }

    pub fn restore(&self) -> Result<cs2_formats::ConfigXml> {
        self.storage_bytes()?;
        Ok(cs2_formats::ConfigXml { root: self.root.clone(), fields: self.fields.iter().map(|field|
            cs2_formats::XmlField { path: field.path.clone(), text: field.text.clone() }).collect() })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TripleTableSnapshot {
    pub groups: BTreeMap<Vec<u8>, [u32; 3]>,
    pub entries: BTreeMap<Vec<u8>, [u32; 3]>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NameRowSnapshot {
    pub id: u32,
    pub name: Vec<u8>,
    pub additional: [Vec<u8>; 2],
    pub aliases: Vec<Vec<u8>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairTableSnapshot {
    pub groups: BTreeMap<Vec<u8>, Vec<(Vec<u8>, Vec<u8>)>>,
}

/// Parent-first flat tree keeps 256-level native trees below the wire
/// codec's nesting limit. Partial, nonterminal prefixes are real state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SequenceSnapshot {
    pub roots: BTreeMap<Vec<u8>, u32>,
    pub nodes: Vec<SequenceNodeSnapshot>,
    pub file: Vec<u8>,
    pub record_span: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SequenceNodeSnapshot {
    pub parent: Option<u32>,
    pub key: i32,
}

impl SequenceSnapshot {
    pub fn storage_bytes(&self) -> Result<usize> {
        if self.nodes.len() > MAX_ITEMS || self.roots.len() > MAX_ITEMS
            || !matches!(self.record_span, 64 | 256) || self.file.len() > 64 * 1024 * 1024
        { return Err(invalid("sequence snapshot count, file or record span is invalid")); }
        let mut roots = std::collections::BTreeSet::new();
        let mut total = self.file.len() + self.nodes.len() * 12;
        for (name, &index) in &self.roots {
            if name.len() > 254 || name.contains(&0)
                || self.nodes.get(index as usize).is_none_or(|node| node.parent.is_some() || node.key != 0)
                || !roots.insert(index as usize)
            { return Err(invalid("sequence snapshot root name or identity is invalid")); }
            total = total.checked_add(name.len()).filter(|&size| size <= MAX_BYTES)
                .ok_or_else(|| invalid("sequence snapshot storage exceeds 256 MiB"))?;
        }
        let mut depths = Vec::with_capacity(self.nodes.len());
        let mut siblings = std::collections::BTreeSet::new();
        for (index, node) in self.nodes.iter().enumerate() {
            let depth = if let Some(parent) = node.parent {
                if parent as usize >= index || !siblings.insert((parent, node.key)) {
                    return Err(invalid("sequence snapshot has a forward parent or duplicate child"));
                }
                depths[parent as usize] + 1usize
            } else {
                if !roots.contains(&index) {
                    return Err(invalid("sequence snapshot contains an unnamed root"));
                }
                0
            };
            if depth > 256 { return Err(invalid("sequence snapshot exceeds 256 levels")); }
            depths.push(depth);
        }
        Ok(total)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawBlockSnapshot {
    pub base: u32,
    pub reserved_end: u32,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatabaseObjectSnapshot {
    pub handle: u32,
    pub separators: [Vec<u8>; 3],
    pub dirty: bool,
    pub rows: Vec<DatabaseRowSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatabaseRowSnapshot {
    pub index: Vec<u8>,
    pub id: Vec<u8>,
    pub short_title: Vec<u8>,
    pub title: Vec<u8>,
    pub indications: Vec<Vec<u8>>,
    pub state: [u32; 3],
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CstWindowSnapshot {
    pub handle: u32,
    pub context: u32,
    pub last_history: i32,
    pub slots: Vec<CstWindowSlotSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CstWindowSlotSnapshot {
    pub history: i32,
    pub input: i32,
    pub saved_count: u32,
    pub valid: bool,
    pub document: Option<CstDocumentSnapshot>,
}

impl DatabaseObjectSnapshot {
    pub fn storage_bytes(&self) -> Result<usize> {
        if self.handle == 0 || self.rows.len() > MAX_ITEMS
            || self.separators.iter().any(|text| text.len() >= 32 || text.contains(&0))
        {
            return Err(invalid("database snapshot has an invalid identity, row count or separator"));
        }
        let mut total = self.separators.iter().map(Vec::len).sum::<usize>();
        let mut indices = std::collections::BTreeSet::new();
        for row in &self.rows {
            if row.index.len() >= 256 || row.index.contains(&0) || !indices.insert(row.index.as_slice())
                || row.id.len() > 120 || row.short_title.len() > 2048 || row.title.len() > 2048
                || row.short_title.contains(&0) || row.title.contains(&0)
                || row.indications.len() != 24 || row.indications.iter().any(|text| text.contains(&0))
            {
                return Err(invalid("database snapshot row metadata or index is invalid"));
            }
            for bytes in [&row.index, &row.id, &row.short_title, &row.title].into_iter()
                .chain(row.indications.iter())
            {
                total = total.checked_add(bytes.len()).filter(|&total| total <= MAX_BYTES)
                    .ok_or_else(|| invalid("database snapshot exceeds 256 MiB"))?;
            }
            // Include bounded row/container bookkeeping even for empty
            // metadata strings, rather than budgeting only text payload.
            total = total.checked_add(768).filter(|&total| total <= MAX_BYTES)
                .ok_or_else(|| invalid("database snapshot exceeds 256 MiB"))?;
        }
        Ok(total)
    }
}

impl CstWindowSnapshot {
    pub fn storage_bytes(&self) -> Result<usize> {
        if self.handle == 0 || self.context == 0 || self.last_history < -1
            || self.slots.is_empty() || self.slots.len() > 4096
        {
            return Err(invalid("CST window snapshot identity or slot count is invalid"));
        }
        self.slots.iter().try_fold(self.slots.len() * 20, |total, slot| {
            // The live context may have been shortened after this cursor was
            // created. Keep its captured signed positions and saved count.
            let bytes = slot.document.as_ref().map_or(Ok(0), CstDocumentSnapshot::storage_bytes)?;
            total.checked_add(bytes).filter(|&total| total <= MAX_BYTES)
                .ok_or_else(|| invalid("CST window snapshot exceeds 256 MiB"))
        })
    }
}

impl ScriptDataObjectsSnapshot {
    pub fn standalone_handles(&self) -> impl Iterator<Item = u32> + '_ {
        self.triple_tables.keys().chain(self.name_tables.keys())
            .chain(self.sequences.keys()).chain(self.raw_images.keys())
            .chain(self.pair_tables.keys()).chain(self.record_stacks.keys())
            .chain(self.xml_documents.keys()).copied()
    }

    /// Cross-type uniqueness inside this group. The complete host graph must
    /// also validate these identities against its images/dictionaries/buffers.
    pub fn validate(&self) -> Result<()> {
        if self.version != 5 || self.containers.len() > MAX_ITEMS
            || self.shared_strings.len() > MAX_ITEMS || self.databases.len() > MAX_ITEMS
            || self.cst_windows.len() > MAX_ITEMS
            || self.ordered_maps.len() > MAX_ITEMS || self.raw_blocks.len() > MAX_ITEMS
            || self.standalone_handles().take(MAX_ITEMS + 1).count() > MAX_ITEMS
        {
            return Err(invalid("script data object snapshot version or count is invalid"));
        }
        let mut total = self.contexts.storage_bytes()?;
        let mut handles = self.contexts.contexts.iter().map(|context| context.handle)
            .collect::<std::collections::BTreeSet<_>>();
        for container in &self.containers {
            if container.handle == 0 || !handles.insert(container.handle) {
                return Err(invalid("script data snapshot repeats an object identity"));
            }
            if container.logical_capacity as usize > container.allocation.len()
                || container.allocation.len() > cs2_formats::save::MAX_HOST_GRAPH_CSS_BYTES
            {
                return Err(invalid("CSS object snapshot allocation/capacity is invalid"));
            }
            // Extended portable CSS is bounded separately by the enclosing graph
            // and revalidated as a tagged container before it becomes live.
            let ordinary_bytes = container.allocation.len().min(64 * 1024 * 1024);
            total = total.checked_add(ordinary_bytes)
                .filter(|&total| total <= MAX_BYTES)
                .ok_or_else(|| invalid("script data object storage exceeds 256 MiB"))?;
        }
        let mut string_bytes = 0usize;
        for (&handle, bytes) in &self.shared_strings {
            if handle == 0 || !handles.insert(handle) {
                return Err(invalid("shared string snapshot repeats an object identity"));
            }
            string_bytes = string_bytes.checked_add(bytes.len())
                .filter(|&total| total <= 64 * 1024 * 1024)
                .ok_or_else(|| invalid("shared string snapshot exceeds 64 MiB"))?;
        }
        total = total.checked_add(string_bytes).filter(|&total| total <= MAX_BYTES)
            .ok_or_else(|| invalid("script data object storage exceeds 256 MiB"))?;
        for database in &self.databases {
            if !handles.insert(database.handle) {
                return Err(invalid("script data snapshot repeats an object identity"));
            }
            total = total.checked_add(database.storage_bytes()?).filter(|&total| total <= MAX_BYTES)
                .ok_or_else(|| invalid("script data object storage exceeds 256 MiB"))?;
        }
        let contexts = self.contexts.contexts.iter().map(|context| context.handle)
            .collect::<std::collections::BTreeSet<_>>();
        for window in &self.cst_windows {
            if !handles.insert(window.handle) || !contexts.contains(&window.context) {
                return Err(invalid("CST window snapshot repeats an identity or references an absent context"));
            }
            total = total.checked_add(window.storage_bytes()?).filter(|&total| total <= MAX_BYTES)
                .ok_or_else(|| invalid("script data object storage exceeds 256 MiB"))?;
        }
        for (&handle, map) in &self.ordered_maps {
            if handle == 0 || !handles.insert(handle) || map.len() > MAX_ITEMS {
                return Err(invalid("ordered map snapshot has an invalid/repeated identity or count"));
            }
            let mut length = 4usize;
            for key in map.keys() {
                if key.contains(&0) { return Err(invalid("ordered map snapshot key contains a NUL")); }
                let span = key.len().checked_add(4).map(|length| length & !3)
                    .and_then(|length| length.checked_add(4)).ok_or_else(|| invalid("ordered map key overflows"))?;
                length = length.checked_add(span).filter(|&length| length <= 64 * 1024 * 1024)
                    .ok_or_else(|| invalid("ordered map snapshot exceeds 64 MiB"))?;
            }
            total = total.checked_add(length).filter(|&total| total <= MAX_BYTES)
                .ok_or_else(|| invalid("script data object storage exceeds 256 MiB"))?;
        }
        for handle in self.standalone_handles() {
            if handle == 0 || !handles.insert(handle) {
                return Err(invalid("standalone data snapshot repeats an object identity"));
            }
        }
        for table in self.triple_tables.values() {
            if table.groups.len() > MAX_ITEMS || table.entries.len() > MAX_ITEMS {
                return Err(invalid("triple table snapshot exceeds its item limit"));
            }
            let mut bytes = 0usize;
            for name in table.groups.keys().chain(table.entries.keys()) {
                if name.len() > 511 { return Err(invalid("triple table snapshot name exceeds 511 bytes")); }
                bytes = bounded_storage(bytes, name.len() + 12, 64 * 1024 * 1024)?;
            }
            total = bounded_storage(total, bytes, MAX_BYTES)?;
        }
        for rows in self.name_tables.values() {
            if rows.len() > MAX_ITEMS { return Err(invalid("name table snapshot exceeds its row limit")); }
            let mut bytes = 0usize;
            let mut aliases = 0usize;
            for row in rows {
                if row.name.len() > 255 || row.additional.iter().any(|text| text.len() > 255)
                    || row.aliases.first() != Some(&row.name)
                    || row.aliases.iter().any(|text| text.len() > 1023)
                { return Err(invalid("name table snapshot row metadata is invalid")); }
                aliases = aliases.checked_add(row.aliases.len()).filter(|&count| count <= MAX_ITEMS)
                    .ok_or_else(|| invalid("name table snapshot exceeds its alias limit"))?;
                for text in std::iter::once(&row.name).chain(row.additional.iter()).chain(row.aliases.iter()) {
                    bytes = bounded_storage(bytes, text.len(), 64 * 1024 * 1024)?;
                }
                // Empty rows still require bounded bookkeeping.
                total = bounded_storage(total, 64, MAX_BYTES)?;
            }
            total = bounded_storage(total, bytes, MAX_BYTES)?;
        }
        for sequence in self.sequences.values() {
            total = bounded_storage(total, sequence.storage_bytes()?, MAX_BYTES)?;
        }
        let mut image_bytes = 0usize;
        for bytes in self.raw_images.values() {
            if bytes.len() > 64 * 1024 * 1024 { return Err(invalid("raw image snapshot exceeds 64 MiB")); }
            image_bytes = bounded_storage(image_bytes, bytes.len(), MAX_BYTES)?;
        }
        total = bounded_storage(total, image_bytes, MAX_BYTES)?;
        for table in self.pair_tables.values() {
            let mut items = table.groups.len();
            if items > MAX_ITEMS { return Err(invalid("pair table snapshot exceeds its group limit")); }
            let mut bytes = 0usize;
            for (group, rows) in &table.groups {
                items = items.checked_add(rows.len()).filter(|&count| count <= MAX_ITEMS)
                    .ok_or_else(|| invalid("pair table snapshot exceeds its row limit"))?;
                bytes = bounded_storage(bytes, group.len(), 64 * 1024 * 1024)?;
                for (key, value) in rows {
                    if key.len() > 1023 || value.len() > 1023 {
                        return Err(invalid("pair table snapshot field exceeds 1023 bytes"));
                    }
                    bytes = bounded_storage(bytes, key.len() + value.len(), 64 * 1024 * 1024)?;
                }
            }
            total = bounded_storage(total, bytes, MAX_BYTES)?;
        }
        for stack in self.record_stacks.values() {
            total = bounded_storage(total, stack.storage_bytes()?, MAX_BYTES)?;
        }
        for document in self.xml_documents.values() {
            total = bounded_storage(total, document.storage_bytes()?, MAX_BYTES)?;
        }
        let mut blocks = self.raw_blocks.iter().collect::<Vec<_>>();
        blocks.sort_unstable_by_key(|block| block.base);
        let mut end = 0;
        for block in blocks {
            let span = u32::try_from(block.bytes.len()).ok()
                .and_then(|length| length.checked_add(4)).map(|length| length & !3);
            if block.base == 0 || block.base & 3 != 0 || block.base < end
                || block.bytes.len() > 64 * 1024 * 1024
                || span.and_then(|span| block.base.checked_add(span)) != Some(block.reserved_end)
                || handles.range(block.base..block.reserved_end).next().is_some()
            {
                return Err(invalid("RAW snapshot has an invalid or overlapping reserved address range"));
            }
            end = block.reserved_end;
            total = total.checked_add(block.bytes.len()).filter(|&total| total <= MAX_BYTES)
                .ok_or_else(|| invalid("script data object storage exceeds 256 MiB"))?;
        }
        Ok(())
    }
}

fn bounded_storage(current: usize, added: usize, limit: usize) -> Result<usize> {
    current.checked_add(added).filter(|&bytes| bytes <= limit)
        .ok_or_else(|| invalid("standalone data snapshot storage exceeds its limit"))
}

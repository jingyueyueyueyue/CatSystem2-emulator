//! A sparse SFNT metadata view for system-font discovery. It never reads glyf,
//! CFF, outline or bitmap tables and never constructs a fontdue rasterizer.
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use super::font_cache::FontVersion;

const MAX_CACHE_BYTES: usize = 16 * 1024 * 1024;
const MAX_VIEW_BYTES: usize = 4 * 1024 * 1024;
const MAX_TABLE_BYTES: usize = 1024 * 1024;

#[derive(Default)]
struct MetadataCache {
    entries: BTreeMap<FontVersion, Arc<Vec<u8>>>,
    bytes: usize,
}

pub(crate) fn system_font_metadata(path: &Path) -> io::Result<Arc<Vec<u8>>> {
    static CACHE: OnceLock<Mutex<MetadataCache>> = OnceLock::new();
    let version = FontVersion::file(path)?;
    let mut cache = CACHE.get_or_init(Mutex::default).lock().unwrap_or_else(|error| error.into_inner());
    if let Some(view) = cache.entries.get(&version) { return Ok(Arc::clone(view)); }
    let view = Arc::new(read_metadata_view(path)?);
    version.unchanged(path)?;
    while cache.bytes.saturating_add(view.len()) > MAX_CACHE_BYTES || cache.entries.len() >= 512 {
        let Some((_, removed)) = cache.entries.pop_first() else { break };
        cache.bytes -= removed.len();
    }
    cache.bytes += view.len();
    cache.entries.insert(version, Arc::clone(&view));
    Ok(view)
}

fn invalid() -> io::Error { io::Error::new(io::ErrorKind::InvalidData, "invalid SFNT metadata") }
fn u16be(bytes: &[u8], offset: usize) -> io::Result<u16> {
    Ok(u16::from_be_bytes(bytes.get(offset..offset + 2).ok_or_else(invalid)?.try_into().map_err(|_| invalid())?))
}
fn u32be(bytes: &[u8], offset: usize) -> io::Result<u32> {
    Ok(u32::from_be_bytes(bytes.get(offset..offset + 4).ok_or_else(invalid)?.try_into().map_err(|_| invalid())?))
}
fn read_at(file: &mut File, size: u64, offset: u64, length: usize) -> io::Result<Vec<u8>> {
    if offset.checked_add(length as u64).is_none_or(|end| end > size) { return Err(invalid()); }
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = vec![0; length];
    file.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn read_metadata_view(path: &Path) -> io::Result<Vec<u8>> {
    let mut file = File::open(path)?;
    let size = file.metadata()?.len();
    let header = read_at(&mut file, size, 0, 12)?;
    let collection = header.starts_with(b"ttcf");
    let count = if collection { u32be(&header, 8)?.min(64) as usize } else { 1 };
    let offsets = if collection { read_at(&mut file, size, 12, count * 4)? } else { Vec::new() };
    let mut output = if collection {
        let mut bytes = header;
        bytes[8..12].copy_from_slice(&(count as u32).to_be_bytes());
        bytes.resize(12 + count * 4, 0);
        bytes
    } else { Vec::new() };
    let mut payloads = BTreeMap::<(u32, usize), usize>::new();
    for index in 0..count {
        let source_offset = if collection { u32be(&offsets, index * 4)? as u64 } else { 0 };
        let face_header = read_at(&mut file, size, source_offset, 12)?;
        let table_count = usize::from(u16be(&face_header, 4)?);
        if table_count > 4096 { return Err(invalid()); }
        let directory = read_at(&mut file, size, source_offset + 12, table_count * 16)?;
        let mut tables = Vec::new();
        for record in directory.chunks_exact(16) {
            let tag: [u8; 4] = record[0..4].try_into().map_err(|_| invalid())?;
            let length = u32be(record, 12)? as usize;
            let wanted = match &tag {
                b"name" | b"cmap" => (length <= MAX_TABLE_BYTES).then_some(length),
                b"OS/2" => Some(length.min(96)),
                b"head" => Some(length.min(54)),
                b"post" => Some(length.min(32)),
                _ => None,
            };
            if let Some(length) = wanted { tables.push((tag, u32be(record, 8)?, length)); }
        }
        while output.len() % 4 != 0 { output.push(0); }
        let base = output.len();
        if collection { output[12 + index * 4..16 + index * 4].copy_from_slice(&(base as u32).to_be_bytes()); }
        output.extend_from_slice(&face_header);
        output[base + 4..base + 6].copy_from_slice(&(tables.len() as u16).to_be_bytes());
        output.resize(output.len() + tables.len() * 16, 0);
        for (number, (tag, offset, length)) in tables.into_iter().enumerate() {
            let key = (offset, length);
            let target = if let Some(&target) = payloads.get(&key) { target } else {
                while output.len() % 4 != 0 { output.push(0); }
                if output.len().saturating_add(length) > MAX_VIEW_BYTES { return Err(invalid()); }
                let target = output.len();
                output.extend_from_slice(&read_at(&mut file, size, u64::from(offset), length)?);
                payloads.insert(key, target);
                target
            };
            let record = base + 12 + number * 16;
            output[record..record + 4].copy_from_slice(&tag);
            output[record + 8..record + 12].copy_from_slice(&(target as u32).to_be_bytes());
            output[record + 12..record + 16].copy_from_slice(&(length as u32).to_be_bytes());
        }
    }
    Ok(output)
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn sparse_metadata_keeps_names_styles_and_cmap_without_font_outlines() {
        let windows = std::path::PathBuf::from(std::env::var_os("WINDIR").unwrap());
        let path = windows.join("Fonts/arialbi.ttf");
        let view = system_font_metadata(&path).unwrap();
        let face = ttf_parser::RawFace::parse(&view, 0).unwrap();
        for tag in [b"name", b"OS/2", b"head", b"post", b"cmap"] {
            assert!(face.table(ttf_parser::Tag::from_bytes(tag)).is_some());
        }
        assert!(face.table(ttf_parser::Tag::from_bytes(b"glyf")).is_none());
        assert!(face.table(ttf_parser::Tag::from_bytes(b"CFF ")).is_none());
        assert!(view.len() < path.metadata().unwrap().len() as usize);
        assert!(Arc::ptr_eq(&view, &system_font_metadata(&path).unwrap()));
    }
}

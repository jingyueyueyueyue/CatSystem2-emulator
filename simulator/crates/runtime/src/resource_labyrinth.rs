use super::{ArchiveIndex, ArchiveResourceEntry, ArchiveStatus};
use crate::{RuntimeError, RuntimeResult};
use std::collections::BTreeSet;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

pub(crate) const LABYRINTH_PATCH: &str = "Grisaia2_cn.cpk";
const HEADER_SIZE: u64 = 0x40;
const SCENE_FOLDER: usize = 0x9f;
const SCENE_DIRECTORY_START: usize = 0xd5;
const MAX_DIRECTORY_SIZE: u64 = 64 * 1024 * 1024;

pub(super) fn scan(path: &Path, relative_path: PathBuf, size: u64) -> RuntimeResult<Option<ArchiveIndex>> {
    if size < SCENE_DIRECTORY_START as u64 {
        return Ok(None);
    }
    let io_error = |source| RuntimeError::Io { path: path.to_path_buf(), source };
    let mut file = File::open(path).map_err(io_error)?;
    let mut prefix = [0; SCENE_DIRECTORY_START];
    file.read_exact(&mut prefix).map_err(io_error)?;
    let Some((data_start, data_end, count)) = scene_layout(&prefix, size) else {
        return Ok(None);
    };
    let mut directory = vec![0; data_start as usize];
    directory[..prefix.len()].copy_from_slice(&prefix);
    file.read_exact(&mut directory[prefix.len()..]).map_err(io_error)?;
    let Some(entries) = scene_entries(&directory, data_start, data_end, count) else {
        return Ok(None);
    };
    // EVB entries contain the complete, stored CST container. Validate its
    // inexpensive outer header so an unsupported layout cannot mount as CST.
    let mut cst = [0; 16];
    for entry in &entries {
        file.seek(SeekFrom::Start(entry.offset)).map_err(io_error)?;
        file.read_exact(&mut cst).map_err(io_error)?;
        let packed = u32::from_le_bytes(cst[8..12].try_into().unwrap());
        let plain = u32::from_le_bytes(cst[12..].try_into().unwrap());
        let length = if packed == 0 { plain } else { packed };
        if &cst[..8] != b"CatScene" || u64::from(length) + 16 != entry.size {
            return Ok(None);
        }
    }
    Ok(Some(ArchiveIndex {
        path: relative_path, entry_count: count, entries,
        status: ArchiveStatus::Indexed, layout: None, payload_key: None,
        name_seed: None, name_seed_score: None,
    }))
}

fn scene_layout(prefix: &[u8; SCENE_DIRECTORY_START], size: u64) -> Option<(u64, u64, u32)> {
    let field = |at: usize| u64::from_le_bytes(prefix[at..at + 8].try_into().unwrap());
    // This EVB layout starts with an uncompressed directory and a `scene`
    // folder under its default root. Other EVB variants need their own parser.
    if &prefix[..8] != b"EVB\0\x40\0\0\0"
        || field(0x18) != HEADER_SIZE || field(0x28) != 1
        || &prefix[SCENE_FOLDER..SCENE_FOLDER + 8] != b"\x02\0\0\0\x02\0\0\0"
        || &prefix[SCENE_FOLDER + 12..SCENE_FOLDER + 24] != b"s\0c\0e\0n\0e\0\0\0"
    {
        return None;
    }
    let count = u32::from_le_bytes(prefix[SCENE_FOLDER + 8..SCENE_FOLDER + 12].try_into().unwrap());
    let data_start = HEADER_SIZE.checked_add(field(0x40))?.checked_add(4)?;
    let data_end = field(0x30);
    if count == 0 || u64::from(count) + 2 > field(0x38)
        || data_start > MAX_DIRECTORY_SIZE
        || data_start < SCENE_DIRECTORY_START as u64 + u64::from(count) * 72
        || data_start >= data_end || data_end > size
        || HEADER_SIZE.checked_add(field(0x20))? != data_end
    {
        return None;
    }
    Some((data_start, data_end, count))
}

fn scene_entries(directory: &[u8], data_start: u64, data_end: u64, count: u32) -> Option<Vec<ArchiveResourceEntry>> {
    let mut cursor = SCENE_DIRECTORY_START;
    let mut names = BTreeSet::new();
    let mut entries = Vec::with_capacity(count as usize);
    for id in 3..3 + count {
        let node = directory.get(cursor..cursor + 12)?;
        if u32::from_le_bytes(node[..4].try_into().ok()?) != id || node[4..] != [3, 0, 0, 0, 0, 0, 0, 0] {
            return None;
        }
        cursor += node.len();
        let mut name = Vec::new();
        loop {
            let unit = directory.get(cursor..cursor + 2)?;
            cursor += unit.len();
            if unit == [0, 0] { break; }
            if name.len() == 255 || unit[1] != 0 || !(unit[0].is_ascii_alphanumeric() || b"._-".contains(&unit[0])) {
                return None;
            }
            name.push(unit[0]);
        }
        // File metadata has a three-byte kind, length, timestamps, flags,
        // and the offset and stored length in the EVB data region.
        let metadata = directory.get(cursor..cursor + 58)?;
        if metadata[..3] != [2, 0, 0] || !name.ends_with(b".cst")
            || !names.insert(name.to_ascii_lowercase()) {
            return None;
        }
        let size = u64::from_le_bytes(metadata[3..11].try_into().ok()?);
        let offset = data_start.checked_add(u64::from_le_bytes(metadata[42..50].try_into().ok()?))?;
        let stored = u64::from_le_bytes(metadata[50..58].try_into().ok()?);
        if stored != size || size < 16 || offset.checked_add(size)? > data_end {
            return None;
        }
        entries.push(ArchiveResourceEntry {
            name: String::from_utf8(name.clone()).ok()?, raw_name: name, offset, size,
        });
        cursor += metadata.len();
    }
    Some(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resource_router::{GameLanguage, LocalizedSceneEncoding, ResourceMode};
    use crate::{ResourceIndex, ResourceLocation};

    #[test]
    fn routes_installed_evb_scenes_only_in_localized_mode() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../games/灰色的迷宫");
        if !root.join(LABYRINTH_PATCH).is_file() { return; }
        for language in [GameLanguage::Localized, GameLanguage::Japanese] {
            let index = ResourceIndex::scan_with_language(&root, language).unwrap();
            let evb = index.archive_index(LABYRINTH_PATCH).unwrap();
            assert_eq!(evb.entries.len(), 216);
            let candidate = index.resolve_mode_candidate("scene.int/amass_1.cst", ResourceMode::LooseFirst)
                .unwrap().unwrap();
            let ResourceLocation::Archive { path, .. } = &candidate.location else {
                panic!("expected an archive for amass_1.cst");
            };
            let localized = language == GameLanguage::Localized;
            assert_eq!(path == &root.join(LABYRINTH_PATCH), localized);
            assert_eq!(index.resource_router().unwrap().localized_scene_encoding(&index, path),
                localized.then_some(LocalizedSceneEncoding::Gbk));
            let bytes = index.read_candidate(candidate).unwrap();
            assert!(!cs2_script::CstDocument::parse(&bytes).unwrap().records.is_empty());
        }
    }

    #[test]
    fn evb_scene_directory_rejects_out_of_range_members() {
        let name = b"s\0c\0e\0n\0e\0.\0c\0s\0t\0\0\0";
        let metadata_start = SCENE_DIRECTORY_START + 12 + name.len();
        let data_start = (metadata_start + 58) as u64;
        let data_end = data_start + 16;
        let mut directory = vec![0; data_start as usize];
        directory[..8].copy_from_slice(b"EVB\0\x40\0\0\0");
        for (offset, value) in [(0x18, HEADER_SIZE), (0x20, data_end - HEADER_SIZE),
            (0x28, 1), (0x30, data_end), (0x38, 3), (0x40, data_start - HEADER_SIZE - 4)]
        {
            directory[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
        }
        directory[SCENE_FOLDER..SCENE_FOLDER + 24]
            .copy_from_slice(b"\x02\0\0\0\x02\0\0\0\x01\0\0\0s\0c\0e\0n\0e\0\0\0");
        directory[SCENE_DIRECTORY_START..SCENE_DIRECTORY_START + 12]
            .copy_from_slice(b"\x03\0\0\0\x03\0\0\0\0\0\0\0");
        directory[SCENE_DIRECTORY_START + 12..metadata_start].copy_from_slice(name);
        directory[metadata_start] = 2;
        directory[metadata_start + 3..metadata_start + 11].copy_from_slice(&16u64.to_le_bytes());
        directory[metadata_start + 50..metadata_start + 58].copy_from_slice(&16u64.to_le_bytes());

        let prefix = directory[..SCENE_DIRECTORY_START].try_into().unwrap();
        assert_eq!(scene_layout(prefix, data_end), Some((data_start, data_end, 1)));
        assert_eq!(scene_layout(prefix, data_end - 1), None);
        assert_eq!(scene_entries(&directory, data_start, data_end, 1).unwrap()[0].offset, data_start);
        directory[metadata_start + 42..metadata_start + 50].copy_from_slice(&1u64.to_le_bytes());
        assert!(scene_entries(&directory, data_start, data_end, 1).is_none());
    }
}

use super::{ArchiveIndex, ArchiveResourceEntry, ArchiveStatus};
use crate::{RuntimeError, RuntimeResult, sha256::Sha256};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

pub(crate) const LABYRINTH_PATCH: &str = "Grisaia2_cn.cpk";
const PATCH_SIZE: u64 = 31_855_419;
const PATCH_SHA256: &str = "65fc3667d54292ae49e70440e55db52e7b9356f7d5b98f1b5ba1df1a3f94cb1a";
const DATA_START: usize = 0x1b229;
const DATA_END: u64 = 0x1e61255;
const SCENE_DIRECTORY_START: usize = 0xd5;
const SCENE_COUNT: u32 = 216;

pub(super) fn scan(path: &Path, relative_path: PathBuf, size: u64) -> RuntimeResult<Option<ArchiveIndex>> {
    if size != PATCH_SIZE {
        return Ok(None);
    }
    let io_error = |source| RuntimeError::Io { path: path.to_path_buf(), source };
    let mut file = File::open(path).map_err(io_error)?;
    let mut directory = vec![0; DATA_START];
    file.read_exact(&mut directory).map_err(io_error)?;
    let mut digest = Sha256::default();
    digest.update(&directory);
    let mut remaining = file.take(size - DATA_START as u64 + 1);
    let mut buffer = [0; 32 * 1024];
    loop {
        let count = remaining.read(&mut buffer).map_err(io_error)?;
        if count == 0 { break; }
        digest.update(&buffer[..count]);
    }
    if digest.finish() != PATCH_SHA256 {
        return Ok(None);
    }
    let entries = scene_entries(&directory).ok_or_else(|| RuntimeError::Archive(
        "invalid scene directory in the verified Labyrinth EVB patch".into(),
    ))?;
    Ok(Some(ArchiveIndex {
        path: relative_path, entry_count: SCENE_COUNT, entries,
        status: ArchiveStatus::Indexed, layout: None, payload_key: None,
        name_seed: None, name_seed_score: None,
    }))
}

fn scene_entries(directory: &[u8]) -> Option<Vec<ArchiveResourceEntry>> {
    let mut cursor = SCENE_DIRECTORY_START;
    let mut entries = Vec::with_capacity(SCENE_COUNT as usize);
    for id in 3..3 + SCENE_COUNT {
        let node = directory.get(cursor..cursor + 12)?;
        if u32::from_le_bytes(node[..4].try_into().ok()?) != id || node[4..] != [3, 0, 0, 0, 0, 0, 0, 0] {
            return None;
        }
        cursor += node.len();
        let mut name = Vec::new();
        for unit in directory.get(cursor..)?.chunks_exact(2) {
            cursor += 2;
            if unit == [0, 0] { break; }
            if unit[1] != 0 || !(unit[0].is_ascii_alphanumeric() || b"._-".contains(&unit[0])) {
                return None;
            }
            name.push(unit[0]);
        }
        // File metadata has a three-byte kind, length, timestamps, flags,
        // and the offset and stored length in the EVB data region.
        let metadata = directory.get(cursor..cursor + 58)?;
        if metadata[..3] != [2, 0, 0] || !name.ends_with(b".cst") {
            return None;
        }
        let size = u64::from_le_bytes(metadata[3..11].try_into().ok()?);
        let offset = (DATA_START as u64).checked_add(u64::from_le_bytes(metadata[42..50].try_into().ok()?))?;
        let stored = u64::from_le_bytes(metadata[50..58].try_into().ok()?);
        if stored != size || size < 16 || offset.checked_add(size)? > DATA_END {
            return None;
        }
        entries.push(ArchiveResourceEntry {
            name: String::from_utf8(name.clone()).ok()?, raw_name: name, offset, size,
        });
        cursor += metadata.len();
    }
    Some(entries)
}

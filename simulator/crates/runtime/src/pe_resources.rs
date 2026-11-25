use blowfish::{
    BlowfishLE,
    cipher::{BlockDecrypt, KeyInit},
};
use cs2_formats::discover_archive_name_seed;
use std::path::Path;

use crate::{RuntimeError, RuntimeResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Section {
    name: [u8; 8],
    pub(crate) virtual_address: u32,
    virtual_size: u32,
    raw_address: u32,
    raw_size: u32,
    characteristics: u32,
}

impl Section {
    pub(crate) fn executable(self) -> bool {
        self.characteristics & 0x2000_0000 != 0
    }

    pub(crate) fn is_readable_data(self) -> bool {
        // IMAGE_SCN_MEM_READ | IMAGE_SCN_CNT_INITIALIZED_DATA
        self.characteristics & 0x6000_00e0 == 0x4000_0040
    }

    pub(crate) fn is_flat_text_code(self) -> bool {
        // IMAGE_SCN_MEM_READ | IMAGE_SCN_CNT_CODE, without MEM_EXECUTE.
        self.characteristics & 0x6000_00e0 == 0x4000_0020
            && self.name == *b".text\0\0\0"
            && self.virtual_address != 0
            && self.raw_address == self.virtual_address
            && self.raw_size != 0
            && self.raw_size == self.virtual_size
    }

    pub(crate) fn file_bytes_from(self, data: &[u8], rva: u32) -> Option<&[u8]> {
        let start = rva.checked_sub(self.virtual_address)?;
        let size = if self.virtual_size == 0 {
            self.raw_size
        } else {
            self.raw_size.min(self.virtual_size)
        };
        if start >= size {
            return None;
        }
        let start = usize::try_from(self.raw_address.checked_add(start)?).ok()?;
        let end = usize::try_from(self.raw_address.checked_add(size)?).ok()?;
        data.get(start..end)
    }
}

#[derive(Debug, Clone, Copy)]
struct ResourceDirectoryEntry {
    name: u32,
    offset: u32,
}

pub fn archive_name_seed(path: &Path) -> RuntimeResult<Option<u32>> {
    let bytes = std::fs::read(path).map_err(|source| RuntimeError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    archive_name_seed_from_pe(&bytes)
}

pub(crate) fn archive_name_seed_from_pe(bytes: &[u8]) -> RuntimeResult<Option<u32>> {
    let key = resource_bytes(bytes, b"KEY_CODE", b"KEY")?
        .map(|bytes| bytes.into_iter().map(|byte| byte ^ 0xcd).collect())
        .unwrap_or_else(|| b"windmill".to_vec());
    let (encrypted_vcode, legacy) = match resource_bytes(bytes, b"V_CODE2", b"DATA")? {
        Some(code) => (code, false),
        None => match resource_bytes(bytes, b"V_CODE", b"DATA")? {
            Some(code) => (code, true),
            None => return Ok(None),
        },
    };
    let vcode = decrypt_vcode(&encrypted_vcode, &key)?;
    let seed = discover_archive_name_seed(
        (!legacy).then_some(vcode.as_slice()),
        legacy.then_some(vcode.as_slice()),
        None,
    )
    .map_err(|error| RuntimeError::Archive(format!("invalid PE version resource: {error}")))?;
    Ok(Some(seed.value))
}

fn decrypt_vcode(bytes: &[u8], key: &[u8]) -> RuntimeResult<Vec<u8>> {
    let cipher = BlowfishLE::new_from_slice(key)
        .map_err(|error| RuntimeError::Archive(format!("invalid PE resource key: {error}")))?;
    let mut output = bytes.to_vec();
    if output.len() % 8 != 0 {
        return Err(RuntimeError::Archive(
            "V_CODE resource is not block aligned".to_string(),
        ));
    }
    for block in output.chunks_exact_mut(8) {
        cipher.decrypt_block(block.into());
    }
    while output.last() == Some(&0) {
        output.pop();
    }
    Ok(output)
}

fn resource_bytes(
    data: &[u8],
    type_name: &[u8],
    item_name: &[u8],
) -> RuntimeResult<Option<Vec<u8>>> {
    let sections = parse_sections(data)?;
    let resource_rva = resource_directory_rva(data)?;
    let Some(resource_offset) = rva_to_offset(&sections, resource_rva) else {
        return Ok(None);
    };
    let Some(type_directory) =
        find_named_directory(data, resource_offset, resource_offset, type_name)?
    else {
        return Ok(None);
    };
    let Some(item_directory) =
        find_named_directory(data, type_directory, resource_offset, item_name)?
    else {
        return Ok(None);
    };
    let Some(language_entry) = first_entry(data, item_directory, resource_offset)? else {
        return Ok(None);
    };
    let data_entry = data
        .get(language_entry..language_entry + 16)
        .ok_or_else(|| RuntimeError::Archive("PE resource data entry is truncated".to_string()))?;
    let data_rva = u32_at(data_entry, 0)?;
    let length = usize::try_from(u32_at(data_entry, 4)?)
        .map_err(|_| RuntimeError::Archive("PE resource length is too large".to_string()))?;
    let Some(offset) = rva_to_offset(&sections, data_rva) else {
        return Err(RuntimeError::Archive(
            "PE resource points outside file sections".to_string(),
        ));
    };
    let bytes = data
        .get(
            offset
                ..offset.checked_add(length).ok_or_else(|| {
                    RuntimeError::Archive("PE resource range overflows".to_string())
                })?,
        )
        .ok_or_else(|| RuntimeError::Archive("PE resource payload is truncated".to_string()))?;
    Ok(Some(bytes.to_vec()))
}

fn coff_header_offset(data: &[u8]) -> RuntimeResult<usize> {
    if data.get(..2) != Some(b"MZ") {
        return Err(RuntimeError::Archive(
            "PE DOS signature is invalid".to_string(),
        ));
    }
    let pe_offset = usize::try_from(u32_at(
        data.get(0x3c..0x40)
            .ok_or_else(|| RuntimeError::Archive("PE header offset is truncated".to_string()))?,
        0,
    )?)
    .map_err(|_| RuntimeError::Archive("PE header offset is too large".to_string()))?;
    let coff = pe_offset
        .checked_add(4)
        .ok_or_else(|| RuntimeError::Archive("PE header offset is too large".to_string()))?;
    if data.get(pe_offset..coff) != Some(b"PE\0\0") {
        return Err(RuntimeError::Archive("PE signature is invalid".to_string()));
    }
    Ok(coff)
}

pub(crate) fn x86_image_layout(data: &[u8]) -> Option<(u32, u32)> {
    let coff = coff_header_offset(data).ok()?;
    let optional_offset = coff.checked_add(20)?;
    let header = data.get(coff..optional_offset)?;
    let optional_length = usize::from(u16_at(header, 16).ok()?);
    let optional = data.get(optional_offset..optional_offset.checked_add(optional_length)?)?;
    if u16_at(header, 0).ok()? != 0x014c || u16_at(optional, 0).ok()? != 0x010b {
        return None;
    }
    Some((u32_at(optional, 28).ok()?, u32_at(optional, 16).ok()?))
}

pub(crate) fn x86_data_directory(data: &[u8], index: usize) -> Option<(u32, u32)> {
    let coff = coff_header_offset(data).ok()?;
    let optional_offset = coff.checked_add(20)?;
    let header = data.get(coff..optional_offset)?;
    let optional_length = usize::from(u16_at(header, 16).ok()?);
    let optional = data.get(optional_offset..optional_offset.checked_add(optional_length)?)?;
    if u16_at(optional, 0).ok()? != 0x010b
        || index >= usize::try_from(u32_at(optional, 92).ok()?).ok()?
    {
        return None;
    }
    let entry = index.checked_mul(8)?.checked_add(96)?;
    Some((u32_at(optional, entry).ok()?, u32_at(optional, entry + 4).ok()?))
}

pub(crate) fn parse_sections(data: &[u8]) -> RuntimeResult<Vec<Section>> {
    let coff = coff_header_offset(data)?;
    let optional = coff
        .checked_add(20)
        .ok_or_else(|| RuntimeError::Archive("PE COFF header offset overflows".to_string()))?;
    let header = data
        .get(coff..optional)
        .ok_or_else(|| RuntimeError::Archive("PE COFF header is truncated".to_string()))?;
    let section_count = usize::from(u16_at(header, 2)?);
    let optional_length = usize::from(u16_at(header, 16)?);
    let section_table = optional
        .checked_add(optional_length)
        .and_then(|start| data.get(start..start.checked_add(section_count * 40)?))
        .ok_or_else(|| RuntimeError::Archive("PE section table is truncated".to_string()))?;
    section_table.chunks_exact(40).map(|section| {
        Ok(Section {
            name: section[..8].try_into().unwrap(),
            virtual_size: u32_at(section, 8)?,
            virtual_address: u32_at(section, 12)?,
            raw_size: u32_at(section, 16)?,
            raw_address: u32_at(section, 20)?,
            characteristics: u32_at(section, 36)?,
        })
    }).collect()
}

fn resource_directory_rva(data: &[u8]) -> RuntimeResult<u32> {
    let pe_offset = usize::try_from(u32_at(
        data.get(0x3c..0x40)
            .ok_or_else(|| RuntimeError::Archive("PE header offset is truncated".to_string()))?,
        0,
    )?)
    .map_err(|_| RuntimeError::Archive("PE header offset is too large".to_string()))?;
    let optional = pe_offset + 4 + 20;
    let magic = u16_at(data, optional)?;
    let directory_offset = match magic {
        0x10b => 96,
        0x20b => 112,
        _ => {
            return Err(RuntimeError::Archive(
                "unsupported PE optional header".to_string(),
            ));
        }
    };
    u32_at(data, optional + directory_offset + 8 * 2)
}

fn rva_to_offset(sections: &[Section], rva: u32) -> Option<usize> {
    sections.iter().find_map(|section| {
        let length = section.virtual_size.max(section.raw_size);
        if rva < section.virtual_address
            || rva - section.virtual_address >= length
            || rva - section.virtual_address >= section.raw_size
        {
            return None;
        }
        usize::try_from(section.raw_address.checked_add(rva - section.virtual_address)?).ok()
    })
}

fn find_named_directory(
    data: &[u8],
    directory: usize,
    resource_base: usize,
    wanted: &[u8],
) -> RuntimeResult<Option<usize>> {
    for entry in directory_entries(data, directory)? {
        if entry.name & 0x8000_0000 == 0 {
            continue;
        }
        let name_offset = entry.name & 0x7fff_ffff;
        let name = resource_base
            .checked_add(name_offset as usize)
            .ok_or_else(|| RuntimeError::Archive("PE resource name overflows".to_string()))?;
        let length = usize::from(u16_at(data, name)?);
        let bytes = data
            .get(name + 2..name + 2 + length * 2)
            .ok_or_else(|| RuntimeError::Archive("PE resource name is truncated".to_string()))?;
        let mut decoded = Vec::with_capacity(length);
        for pair in bytes.chunks_exact(2) {
            let unit = u16::from_le_bytes([pair[0], pair[1]]);
            if unit > 0x7f {
                decoded.clear();
                break;
            }
            decoded.push(unit as u8);
        }
        if decoded == wanted {
            if entry.offset & 0x8000_0000 == 0 {
                return Err(RuntimeError::Archive(
                    "PE named resource does not reference a directory".to_string(),
                ));
            }
            let child = resource_base
                .checked_add((entry.offset & 0x7fff_ffff) as usize)
                .ok_or_else(|| {
                    RuntimeError::Archive("PE resource directory overflows".to_string())
                })?;
            return Ok(Some(child));
        }
    }
    Ok(None)
}

fn first_entry(
    data: &[u8],
    directory: usize,
    resource_base: usize,
) -> RuntimeResult<Option<usize>> {
    let Some(entry) = directory_entries(data, directory)?.into_iter().next() else {
        return Ok(None);
    };
    if entry.offset & 0x8000_0000 != 0 {
        return Err(RuntimeError::Archive(
            "PE language resource unexpectedly references a directory".to_string(),
        ));
    }
    Ok(Some(resource_base + (entry.offset & 0x7fff_ffff) as usize))
}

fn directory_entries(data: &[u8], directory: usize) -> RuntimeResult<Vec<ResourceDirectoryEntry>> {
    let header = data
        .get(directory..directory + 16)
        .ok_or_else(|| RuntimeError::Archive("PE resource directory is truncated".to_string()))?;
    let named = usize::from(u16_at(header, 12)?);
    let ids = usize::from(u16_at(header, 14)?);
    let count = named
        .checked_add(ids)
        .ok_or_else(|| RuntimeError::Archive("PE resource entry count overflows".to_string()))?;
    let entries = data
        .get(directory + 16..directory + 16 + count * 8)
        .ok_or_else(|| RuntimeError::Archive("PE resource entries are truncated".to_string()))?;
    entries
        .chunks_exact(8)
        .map(|entry| {
            Ok(ResourceDirectoryEntry {
                name: u32::from_le_bytes(entry[..4].try_into().map_err(|_| {
                    RuntimeError::Archive("PE resource name field is invalid".to_string())
                })?),
                offset: u32::from_le_bytes(entry[4..8].try_into().map_err(|_| {
                    RuntimeError::Archive("PE resource offset field is invalid".to_string())
                })?),
            })
        })
        .collect()
}

fn u16_at(data: &[u8], offset: usize) -> RuntimeResult<u16> {
    let bytes = data
        .get(offset..offset + 2)
        .ok_or_else(|| RuntimeError::Archive("PE field is truncated".to_string()))?;
    Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn u32_at(data: &[u8], offset: usize) -> RuntimeResult<u32> {
    let bytes = data
        .get(offset..offset + 4)
        .ok_or_else(|| RuntimeError::Archive("PE field is truncated".to_string()))?;
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decrypts_archive_name_seeds_from_local_runtime_resources() {
        let Some(root) = std::env::var_os("CS2_SAMPLE_ROOT").map(std::path::PathBuf::from) else {
            return;
        };
        let catalog = root.join("games");
        let root = if catalog.is_dir() { catalog } else { root };
        for (relative, expected) in [
            ("cs2_full_v401/system/cs2_open.exe", 0x1658_0a97),
            (
                "\u{725b}\u{987f}\u{4e0e}\u{82f9}\u{679c}\u{6811}/cs2.exe",
                0x1cf6_43b4,
            ),
            (
                "[170526][NEKO WORKs] \u{30cd}\u{30b3}\u{3071}\u{3089} vol.3 steam\u{7248} (files)/nekopara_vol3/NEKOPARAvol3.exe",
                0x0f6b_b0ae,
            ),
            (
                "ISLAND/ISLAND.exe",
                0x0878_7c5d,
            ),
            ("Yukikoi Melt/YukikoiMelt.exe", 0x23a8_93a2),
            (
                "\u{679c}\u{3064}\u{308b}\u{3053}\u{3068}\u{306a}\u{304d}\u{672a}\u{6765}\u{3088}\u{308a}/Hatsumira.exe",
                0xe113_742d,
            ),
        ] {
            let path = root.join(relative);
            if path.is_file() {
                assert_eq!(archive_name_seed(&path).unwrap(), Some(expected));
            }
        }
    }
}

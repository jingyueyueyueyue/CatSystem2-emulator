use blowfish::{
    BlowfishLE,
    cipher::{BlockDecrypt, KeyInit},
};
use thiserror::Error;

use crate::util::MAX_BUFFER;

// ISLAND's translation image pack is a distinct container, not encrypted KIF.
#[path = "fuyin.rs"]
pub mod fuyin;

const KIF_HEADER_SIZE: usize = 8;
const KIF_LEGACY_NAME_SIZE: usize = 32;
const KIF_NAME_SIZE: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveLayout {
    Name32,
    Name64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveSeedVersion {
    VCode2,
    VCode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiscoveredArchiveSeed {
    pub value: u32,
    pub version: ArchiveSeedVersion,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegacySeedVariant {
    VCodeOnly,
    VCodeOnlyWithCs2GkDat,
    LegacyKifWithoutVersionCode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveEntry {
    pub name: Vec<u8>,
    pub offset: u64,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Archive {
    pub entries: Vec<ArchiveEntry>,
    layout: ArchiveLayout,
    encrypted: bool,
    file_key: Option<u32>,
}

#[derive(Debug, Error)]
pub enum ArchiveError {
    #[error("invalid KIF signature")]
    InvalidSignature,
    #[error("KIF directory exceeds the format reader limit")]
    TooLarge,
    #[error("archive entry exceeds the format reader limit")]
    EntryTooLarge,
    #[error("encrypted KIF archive requires a name seed")]
    EncryptedArchiveRequiresSeed,
    #[error("legacy KIF name seed is unavailable ({variant:?}): {evidence}")]
    LegacySeedUnavailable {
        variant: LegacySeedVariant,
        evidence: &'static str,
    },
    #[error("archive name seed is missing: no V_CODE2 or legacy V_CODE was supplied")]
    MissingNameSeed,
    #[error("version-code resource is empty")]
    InvalidVersionCode,
    #[error("archive is truncated")]
    Truncated,
    #[error("KIF entry name is not NUL terminated")]
    InvalidEntryName,
    #[error("archive entry is outside the input")]
    OutOfBounds,
    #[error("KIF entry table is invalid")]
    InvalidTable,
}

impl Archive {
    /// Parses an unencrypted KIF archive.
    ///
    /// Encrypted CS2 archives must be opened with [`Self::parse_with_name_seed`]
    /// because the executable supplies the archive-specific name seed.
    pub fn parse(data: &[u8]) -> Result<Self, ArchiveError> {
        Self::parse_directory(data, data.len() as u64, None)
    }

    /// Parses a KIF archive using the seed derived from the executable's
    /// `V_CODE2` or legacy `V_CODE` value.
    pub fn parse_with_name_seed(data: &[u8], name_seed: u32) -> Result<Self, ArchiveError> {
        Self::parse_directory(data, data.len() as u64, Some(name_seed))
    }

    /// Parses a KIF archive using an explicitly selected directory layout.
    ///
    /// This is needed for old unencrypted archives whose entries do not carry
    /// the encrypted `__key__.dat` marker that otherwise identifies 32-byte
    /// names.
    pub fn parse_with_layout(data: &[u8], layout: ArchiveLayout) -> Result<Self, ArchiveError> {
        Self::parse_directory_with_layout(data, data.len() as u64, None, layout)
    }

    /// Parses a KIF header and directory table without requiring archive payload bytes.
    ///
    /// `data` must contain the complete header and directory table. `archive_length`
    /// is the length of the full archive and is used to validate decoded resource ranges.
    pub fn parse_directory(
        data: &[u8],
        archive_length: u64,
        name_seed: Option<u32>,
    ) -> Result<Self, ArchiveError> {
        Self::parse_directory_internal(data, archive_length, name_seed, None)
    }

    pub fn parse_directory_with_layout(
        data: &[u8],
        archive_length: u64,
        name_seed: Option<u32>,
        layout: ArchiveLayout,
    ) -> Result<Self, ArchiveError> {
        Self::parse_directory_internal(data, archive_length, name_seed, Some(layout))
    }

    fn parse_directory_internal(
        data: &[u8],
        archive_length: u64,
        name_seed: Option<u32>,
        requested_layout: Option<ArchiveLayout>,
    ) -> Result<Self, ArchiveError> {
        let (mut entries, layout, encrypted, file_key) =
            parse_raw_entries_with_layout(data, archive_length, true, requested_layout)?;
        if encrypted {
            let name_seed = match name_seed {
                Some(seed) => seed,
                None if layout == ArchiveLayout::Name32 => {
                    return Err(ArchiveError::LegacySeedUnavailable {
                        variant: LegacySeedVariant::LegacyKifWithoutVersionCode,
                        evidence: LEGACY_KIF_SEED_EVIDENCE,
                    });
                }
                None => return Err(ArchiveError::EncryptedArchiveRequiresSeed),
            };
            let key = file_key.ok_or(ArchiveError::InvalidTable)?;
            let cipher = BlowfishLE::new_from_slice(&key.to_le_bytes())
                .map_err(|_| ArchiveError::InvalidTable)?;

            decrypt_entry_metadata(&mut entries, &cipher);
            for (index, entry) in entries.iter_mut().enumerate().skip(1) {
                let index = index as u32;
                unobfuscate_name(&mut entry.name, name_seed.wrapping_add(index));
            }
            entries.remove(0);
        }

        validate_ranges_u64(&entries, archive_length)?;
        Ok(Self {
            entries,
            layout,
            encrypted,
            file_key,
        })
    }

    /// Parses the fixed KIF header and raw 32- or 64-byte-name entries.
    ///
    /// This is useful for encrypted archives when the executable seed is not
    /// available yet. Raw encrypted offsets and names are not readable until
    /// [`Self::parse_with_name_seed`] is used.
    pub fn parse_raw(data: &[u8]) -> Result<Self, ArchiveError> {
        let (entries, layout, encrypted, file_key) =
            parse_raw_entries_with_layout(data, data.len() as u64, false, None)?;
        Ok(Self {
            entries,
            layout,
            encrypted,
            file_key,
        })
    }

    pub fn parse_raw_with_layout(data: &[u8], layout: ArchiveLayout) -> Result<Self, ArchiveError> {
        let (entries, layout, encrypted, file_key) =
            parse_raw_entries_with_layout(data, data.len() as u64, false, Some(layout))?;
        Ok(Self {
            entries,
            layout,
            encrypted,
            file_key,
        })
    }

    pub fn layout(&self) -> ArchiveLayout {
        self.layout
    }

    pub fn is_encrypted(&self) -> bool {
        self.encrypted
    }

    pub fn payload_key(&self) -> Option<u32> {
        self.file_key.filter(|_| self.encrypted)
    }

    /// Decrypts complete Blowfish blocks in place. A trailing partial block is unchanged.
    pub fn decrypt_payload(&self, bytes: &mut [u8]) {
        let Some(key) = self.payload_key() else {
            return;
        };
        let cipher = BlowfishLE::new_from_slice(&key.to_le_bytes())
            .expect("a four-byte Blowfish key is valid");
        for block in bytes.chunks_exact_mut(8) {
            cipher.decrypt_block(block.into());
        }
    }

    pub fn find(&self, name: &[u8]) -> Option<&ArchiveEntry> {
        self.entries
            .iter()
            .find(|entry| names_equal(&entry.name, name))
    }

    pub fn read(&self, data: &[u8], name: &[u8]) -> Result<Option<Vec<u8>>, ArchiveError> {
        let Some(entry) = self.find(name) else {
            return Ok(None);
        };
        self.read_entry(data, entry).map(Some)
    }

    pub fn read_entry(&self, data: &[u8], entry: &ArchiveEntry) -> Result<Vec<u8>, ArchiveError> {
        let offset = usize::try_from(entry.offset).map_err(|_| ArchiveError::OutOfBounds)?;
        let size = usize::try_from(entry.size).map_err(|_| ArchiveError::OutOfBounds)?;
        if size > MAX_BUFFER {
            return Err(ArchiveError::EntryTooLarge);
        }
        let mut bytes = data
            .get(offset..offset.checked_add(size).ok_or(ArchiveError::OutOfBounds)?)
            .ok_or(ArchiveError::OutOfBounds)?
            .to_vec();

        self.decrypt_payload(&mut bytes);
        Ok(bytes)
    }
}

pub fn verification_seed(code: &[u8]) -> u32 {
    let mut seed = u32::MAX;
    for &byte in code.split(|&byte| byte == 0).next().unwrap_or_default() {
        seed ^= (byte as u32) << 24;
        for _ in 0..8 {
            seed = (seed << 1)
                ^ if seed & 0x8000_0000 != 0 {
                    0x04c1_1db7
                } else {
                    0
                };
        }
        seed = !seed;
    }
    seed
}

const LEGACY_KIF_SEED_EVIDENCE: &str =
    "encrypted 32-byte-name KIF requires an executable V_CODE/V_CODE2 name seed; cs2_gk.dat alone is not a verified seed source";

/// Discovers a KIF name seed from already-extracted, decrypted PE resource bytes.
///
/// `V_CODE2` takes precedence. The same CRC operation on legacy `V_CODE` is
/// independently verified by the UTATEMEGURI, TSUNABAN-LM (32-byte names), and
/// CAMPANELLA archives, including named, checksum-valid `main.kcs` payloads.
/// `cs2_gk.dat` is not a name-seed source and is not needed for these archives.
///
/// This derives a candidate for the caller's selected executable; it cannot
/// select between original and translated executables. Range-valid metadata
/// alone does not verify the name seed: metadata uses the archive's file key.
pub fn discover_archive_name_seed(
    v_code2: Option<&[u8]>,
    v_code: Option<&[u8]>,
    _cs2_gk_dat: Option<&[u8]>,
) -> Result<DiscoveredArchiveSeed, ArchiveError> {
    if let Some(code) = v_code2 {
        if code.first().is_none_or(|&byte| byte == 0) {
            return Err(ArchiveError::InvalidVersionCode);
        }
        return Ok(DiscoveredArchiveSeed {
            value: verification_seed(code),
            version: ArchiveSeedVersion::VCode2,
        });
    }

    if let Some(code) = v_code {
        if code.first().is_none_or(|&byte| byte == 0) {
            return Err(ArchiveError::InvalidVersionCode);
        }
        return Ok(DiscoveredArchiveSeed {
            value: verification_seed(code),
            version: ArchiveSeedVersion::VCode,
        });
    }

    Err(ArchiveError::MissingNameSeed)
}

fn parse_raw_entries_with_layout(
    data: &[u8],
    archive_length: u64,
    validate_encrypted_ranges: bool,
    requested_layout: Option<ArchiveLayout>,
) -> Result<(Vec<ArchiveEntry>, ArchiveLayout, bool, Option<u32>), ArchiveError> {
    if data.get(..4) != Some(b"KIF\0") {
        return Err(ArchiveError::InvalidSignature);
    }
    let count = u32_at(data, 4).ok_or(ArchiveError::Truncated)? as usize;
    if count == 0 {
        return Ok((
            Vec::new(),
            requested_layout.unwrap_or(ArchiveLayout::Name64),
            false,
            None,
        ));
    }
    let mut candidates = Vec::new();
    let mut errors = Vec::new();

    for name_size in [KIF_NAME_SIZE, KIF_LEGACY_NAME_SIZE] {
        match parse_entries_with_name_size(
            data,
            count,
            name_size,
            archive_length,
            validate_encrypted_ranges,
        ) {
            Ok(candidate) => candidates.push((name_size, candidate)),
            Err(error) => errors.push(error),
        }
    }

    // In encrypted archives the first entry is the key marker. In the old
    // layout bytes 40..72 contain its metadata and the next name, while the
    // standard layout contains zero name padding there.
    let legacy_layout_hint = data
        .get(KIF_HEADER_SIZE..KIF_HEADER_SIZE + KIF_LEGACY_NAME_SIZE)
        .and_then(|name| {
            let end = name.iter().position(|&byte| byte == 0)?;
            Some(name[..end].eq_ignore_ascii_case(b"__key__.dat"))
        })
        .is_some_and(|is_key| {
            is_key
                && data
                    .get(40..data.len().min(72))
                    .is_some_and(|bytes| bytes.iter().any(|&byte| byte != 0))
        });
    let layout_hint = requested_layout
        .map(|layout| match layout {
            ArchiveLayout::Name32 => KIF_LEGACY_NAME_SIZE,
            ArchiveLayout::Name64 => KIF_NAME_SIZE,
        })
        .or_else(|| {
            Some(if legacy_layout_hint {
                KIF_LEGACY_NAME_SIZE
            } else {
                KIF_NAME_SIZE
            })
        });
    candidates.retain(|(name_size, _)| Some(*name_size) == layout_hint);

    match candidates.len() {
        0 => Err(preferred_layout_error(errors)),
        1 => {
            let (name_size, candidate) = candidates.pop().unwrap();
            let layout = if name_size == KIF_LEGACY_NAME_SIZE {
                ArchiveLayout::Name32
            } else {
                ArchiveLayout::Name64
            };
            Ok((candidate.0, layout, candidate.1, candidate.2))
        }
        _ => Err(ArchiveError::InvalidTable),
    }
}

fn parse_entries_with_name_size(
    data: &[u8],
    count: usize,
    name_size: usize,
    archive_length: u64,
    validate_encrypted_ranges: bool,
) -> Result<(Vec<ArchiveEntry>, bool, Option<u32>), ArchiveError> {
    let entry_size = name_size + 8;
    let table_size = count
        .checked_mul(entry_size)
        .and_then(|size| size.checked_add(KIF_HEADER_SIZE))
        .ok_or(ArchiveError::InvalidTable)?;
    if table_size > MAX_BUFFER {
        return Err(ArchiveError::TooLarge);
    }
    if data.len() < table_size {
        return Err(ArchiveError::Truncated);
    }

    let mut entries = Vec::with_capacity(count);
    for index in 0..count {
        let offset = KIF_HEADER_SIZE + index * entry_size;
        let name_bytes = data
            .get(offset..offset + name_size)
            .ok_or(ArchiveError::Truncated)?;
        let name_end = name_bytes
            .iter()
            .position(|&byte| byte == 0)
            .ok_or(ArchiveError::InvalidEntryName)?;
        entries.push(ArchiveEntry {
            name: name_bytes[..name_end].to_vec(),
            offset: u32_at(data, offset + name_size).ok_or(ArchiveError::Truncated)? as u64,
            size: u32_at(data, offset + name_size + 4).ok_or(ArchiveError::Truncated)? as u64,
        });
    }

    let encrypted = entries
        .first()
        .is_some_and(|entry| entry.name.eq_ignore_ascii_case(b"__key__.dat"));

    let file_key = encrypted.then(|| {
        let seed = entries[0].size as u32;
        let mut random = LegacyRandom::seeded(seed);
        random.next()
    });
    if encrypted && validate_encrypted_ranges {
        validate_candidate_ranges(&entries, encrypted, file_key, archive_length)?;
    } else if !encrypted {
        validate_ranges_u64(&entries, archive_length)?;
    }

    Ok((entries, encrypted, file_key))
}

fn validate_candidate_ranges(
    entries: &[ArchiveEntry],
    encrypted: bool,
    file_key: Option<u32>,
    archive_length: u64,
) -> Result<(), ArchiveError> {
    if !encrypted {
        return validate_ranges_u64(entries, archive_length);
    }

    let key = file_key.ok_or(ArchiveError::InvalidTable)?;
    let cipher =
        BlowfishLE::new_from_slice(&key.to_le_bytes()).map_err(|_| ArchiveError::InvalidTable)?;
    for (index, entry) in entries.iter().enumerate().skip(1) {
        let mut metadata = [0; 8];
        metadata[..4].copy_from_slice(
            &(entry.offset as u32)
                .wrapping_add(index as u32)
                .to_le_bytes(),
        );
        metadata[4..].copy_from_slice(&(entry.size as u32).to_le_bytes());
        cipher.decrypt_block((&mut metadata).into());
        let offset = u32::from_le_bytes(metadata[..4].try_into().unwrap()) as u64;
        let size = u32::from_le_bytes(metadata[4..].try_into().unwrap()) as u64;
        validate_range(offset, size, archive_length)?;
    }
    Ok(())
}

fn preferred_layout_error(errors: Vec<ArchiveError>) -> ArchiveError {
    errors
        .into_iter()
        .max_by_key(|error| match error {
            ArchiveError::OutOfBounds => 4,
            ArchiveError::InvalidEntryName => 3,
            ArchiveError::TooLarge => 2,
            ArchiveError::Truncated => 1,
            _ => 0,
        })
        .unwrap_or(ArchiveError::InvalidTable)
}

fn decrypt_entry_metadata(entries: &mut [ArchiveEntry], cipher: &BlowfishLE) {
    for (index, entry) in entries.iter_mut().enumerate().skip(1) {
        let mut metadata = [0; 8];
        metadata[..4].copy_from_slice(
            &(entry.offset as u32)
                .wrapping_add(index as u32)
                .to_le_bytes(),
        );
        metadata[4..].copy_from_slice(&(entry.size as u32).to_le_bytes());
        cipher.decrypt_block((&mut metadata).into());
        entry.offset = u32::from_le_bytes(metadata[..4].try_into().unwrap()) as u64;
        entry.size = u32::from_le_bytes(metadata[4..].try_into().unwrap()) as u64;
    }
}

fn validate_ranges_u64(entries: &[ArchiveEntry], archive_length: u64) -> Result<(), ArchiveError> {
    for entry in entries {
        validate_range(entry.offset, entry.size, archive_length)?;
    }
    Ok(())
}

fn validate_range(offset: u64, size: u64, archive_length: u64) -> Result<(), ArchiveError> {
    if offset
        .checked_add(size)
        .is_none_or(|end| end > archive_length)
    {
        return Err(ArchiveError::OutOfBounds);
    }
    Ok(())
}

fn names_equal(left: &[u8], right: &[u8]) -> bool {
    let mut left = left;
    let mut right = right;
    while !left.is_empty() && !right.is_empty() {
        let left_lead = is_cp932_lead(left);
        let right_lead = is_cp932_lead(right);
        if left_lead || right_lead {
            if left.len() < 2 || right.len() < 2 || left[..2] != right[..2] {
                return false;
            }
            left = &left[2..];
            right = &right[2..];
        } else {
            if !left[0].eq_ignore_ascii_case(&right[0]) {
                return false;
            }
            left = &left[1..];
            right = &right[1..];
        }
    }
    left.is_empty() && right.is_empty()
}

fn is_cp932_lead(bytes: &[u8]) -> bool {
    matches!(bytes.first(), Some(0x81..=0x9f | 0xe0..=0xef))
        && bytes
            .get(1)
            .is_some_and(|byte| matches!(*byte, 0x40..=0x7e | 0x80..=0xfc))
}

fn unobfuscate_name(name: &mut [u8], seed: u32) {
    const FORWARD: &[u8; 52] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    const REVERSE: &[u8; 52] = b"zyxwvutsrqponmlkjihgfedcbaZYXWVUTSRQPONMLKJIHGFEDCBA";

    let mut random = LegacyRandom::seeded(seed);
    let key = random.next();
    let mut shift = (key >> 24)
        .wrapping_add(key >> 16)
        .wrapping_add(key >> 8)
        .wrapping_add(key) as u8 as usize;

    for byte in name {
        if !byte.is_ascii_alphabetic() {
            shift += 1;
            continue;
        }

        let mut index = 0;
        let mut cursor = shift;
        loop {
            if REVERSE[cursor % 52] == *byte {
                break;
            }
            if REVERSE[(shift + index + 1) % 52] == *byte {
                index += 1;
                break;
            }
            if REVERSE[(shift + index + 2) % 52] == *byte {
                index += 2;
                break;
            }
            if REVERSE[(shift + index + 3) % 52] == *byte {
                index += 3;
                break;
            }
            index += 4;
            cursor += 4;
            if index >= 52 {
                break;
            }
        }
        if index < 52 {
            *byte = FORWARD[index];
        }
        shift += 1;
    }
}

pub(crate) struct LegacyRandom {
    values: [u32; 624],
    index: usize,
}

impl LegacyRandom {
    pub(crate) fn seeded(mut seed: u32) -> Self {
        let mut values = [0; 624];
        for value in &mut values {
            let high = seed & 0xffff_0000;
            seed = seed.wrapping_mul(69069).wrapping_add(1);
            *value = high | (seed >> 16);
            seed = seed.wrapping_mul(69069).wrapping_add(1);
        }
        Self { values, index: 624 }
    }

    pub(crate) fn next(&mut self) -> u32 {
        if self.index == 624 {
            for index in 0..624 {
                let y = (self.values[index] & 0x8000_0000)
                    | (self.values[(index + 1) % 624] & 0x7fff_ffff);
                self.values[index] = self.values[(index + 397) % 624]
                    ^ (y >> 1)
                    ^ if y & 1 != 0 { 0x9908_b0df } else { 0 };
            }
            self.index = 0;
        }
        let mut value = self.values[self.index];
        self.index += 1;
        value ^= value >> 11;
        value ^= (value << 7) & 0x9d2c_5680;
        value ^= (value << 15) & 0xefc6_0000;
        value ^ (value >> 18)
    }
}

fn u32_at(data: &[u8], offset: usize) -> Option<u32> {
    let bytes = data.get(offset..offset.checked_add(4)?)?;
    Some(u32::from_le_bytes(bytes.try_into().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use blowfish::cipher::BlockEncrypt;

    fn plain_directory(name: &[u8], offset: u32, size: u32) -> Vec<u8> {
        let mut data = b"KIF\0".to_vec();
        data.extend_from_slice(&1u32.to_le_bytes());
        let mut name_field = [0; 64];
        name_field[..name.len()].copy_from_slice(name);
        data.extend_from_slice(&name_field);
        data.extend_from_slice(&offset.to_le_bytes());
        data.extend_from_slice(&size.to_le_bytes());
        data
    }

    fn encrypted_directory(offset: u32, size: u32) -> Vec<u8> {
        let key_seed = 0x1234_5678;
        let mut random = LegacyRandom::seeded(key_seed);
        let key = random.next();
        let cipher = BlowfishLE::new_from_slice(&key.to_le_bytes()).unwrap();
        let mut data = b"KIF\0".to_vec();
        data.extend_from_slice(&2u32.to_le_bytes());

        let mut key_name = [0; 64];
        key_name[..11].copy_from_slice(b"__key__.dat");
        data.extend_from_slice(&key_name);
        data.extend_from_slice(&0u32.to_le_bytes());
        data.extend_from_slice(&key_seed.to_le_bytes());

        let mut name = [0; 64];
        name[..3].copy_from_slice(b"1.2");
        data.extend_from_slice(&name);
        let mut metadata = [0; 8];
        metadata[..4].copy_from_slice(&offset.to_le_bytes());
        metadata[4..].copy_from_slice(&size.to_le_bytes());
        cipher.encrypt_block((&mut metadata).into());
        let stored_offset = u32::from_le_bytes(metadata[..4].try_into().unwrap()).wrapping_sub(1);
        data.extend_from_slice(&stored_offset.to_le_bytes());
        data.extend_from_slice(&metadata[4..]);
        data
    }

    fn legacy_encrypted_directory(offset: u32, size: u32) -> Vec<u8> {
        let key_seed = 0x1234_5678;
        let mut random = LegacyRandom::seeded(key_seed);
        let key = random.next();
        let cipher = BlowfishLE::new_from_slice(&key.to_le_bytes()).unwrap();
        let mut data = b"KIF\0".to_vec();
        data.extend_from_slice(&2u32.to_le_bytes());

        let mut key_name = [0; KIF_LEGACY_NAME_SIZE];
        key_name[..11].copy_from_slice(b"__key__.dat");
        data.extend_from_slice(&key_name);
        data.extend_from_slice(&0u32.to_le_bytes());
        data.extend_from_slice(&key_seed.to_le_bytes());

        let mut name = [0; KIF_LEGACY_NAME_SIZE];
        name[..9].copy_from_slice(b"scene.kcs");
        data.extend_from_slice(&name);
        let mut metadata = [0; 8];
        metadata[..4].copy_from_slice(&offset.to_le_bytes());
        metadata[4..].copy_from_slice(&size.to_le_bytes());
        cipher.encrypt_block((&mut metadata).into());
        let stored_offset = u32::from_le_bytes(metadata[..4].try_into().unwrap()).wrapping_sub(1);
        data.extend_from_slice(&stored_offset.to_le_bytes());
        data.extend_from_slice(&metadata[4..]);
        data
    }

    #[test]
    fn parses_directory_without_payload_using_u64_archive_length() {
        let data = plain_directory(b"large.bin", u32::MAX - 7, 7);
        let archive = Archive::parse_directory(&data, u32::MAX as u64 + 1, None).unwrap();
        assert_eq!(archive.entries[0].offset, u32::MAX as u64 - 7);
        assert_eq!(archive.entries[0].size, 7);

        assert_eq!(
            Archive::parse_directory(&data, u32::MAX as u64 - 1, None)
                .unwrap_err()
                .to_string(),
            "archive entry is outside the input"
        );
    }

    #[test]
    fn encrypted_directory_requires_seed_and_checks_decoded_ranges() {
        let data = encrypted_directory(100, 16);
        assert!(matches!(
            Archive::parse_directory(&data, 116, None),
            Err(ArchiveError::EncryptedArchiveRequiresSeed)
        ));
        let archive = Archive::parse_directory(&data, 116, Some(0)).unwrap();
        assert!(archive.is_encrypted());
        assert_eq!(archive.entries[0].name, b"1.2");
        assert_eq!(archive.entries[0].offset, 100);
        assert_eq!(archive.payload_key().is_some(), true);

        assert!(matches!(
            Archive::parse_directory(&data, 115, Some(0)),
            Err(ArchiveError::OutOfBounds)
        ));
    }

    #[test]
    fn seed_discovery_prefers_vcode2_and_supports_verified_legacy_vcode() {
        let discovered = discover_archive_name_seed(Some(b"cs2 V_CODE"), None, None).unwrap();
        assert_eq!(discovered.version, ArchiveSeedVersion::VCode2);
        assert_eq!(discovered.value, 0x1658_0a97);
        assert_eq!(discovered.value, verification_seed(b"cs2 V_CODE"));

        let gk_data = [0u8; 96];
        for (code, expected) in [
            (b"UTATEMEGURI".as_slice(), 0x72ff_c86f),
            (b"TSUNABAN-LM".as_slice(), 0x803d_26a6),
            (b"CAMPANELLA".as_slice(), 0x4260_e643),
        ] {
            for gk in [None, Some(gk_data.as_slice())] {
                let discovered = discover_archive_name_seed(None, Some(code), gk).unwrap();
                assert_eq!(discovered.version, ArchiveSeedVersion::VCode);
                assert_eq!(discovered.value, expected);
            }
        }
        assert_eq!(
            discover_archive_name_seed(Some(b"cs2 V_CODE"), Some(b"TSUNABAN-LM"), None)
                .unwrap(),
            discovered
        );
        assert!(matches!(
            discover_archive_name_seed(None, None, None),
            Err(ArchiveError::MissingNameSeed)
        ));
    }

    #[test]
    fn parses_legacy_kif_with_32_byte_names_and_40_byte_entries() {
        let directory = legacy_encrypted_directory(88, 4);
        let mut archive_bytes = directory.clone();
        archive_bytes.extend_from_slice(b"data");

        assert!(matches!(
            Archive::parse(&archive_bytes),
            Err(ArchiveError::LegacySeedUnavailable {
                variant: LegacySeedVariant::LegacyKifWithoutVersionCode,
                ..
            })
        ));
        let archive =
            Archive::parse_directory(&directory, archive_bytes.len() as u64, Some(0)).unwrap();
        assert!(archive.is_encrypted());
        assert_eq!(archive.layout(), ArchiveLayout::Name32);
        assert_eq!(archive.entries.len(), 1);
        assert_eq!(archive.entries[0].offset, 88);
        assert_eq!(archive.entries[0].size, 4);
        assert_eq!(
            archive
                .read(&archive_bytes, &archive.entries[0].name)
                .unwrap(),
            Some(b"data".to_vec())
        );

        let raw = Archive::parse_raw(&archive_bytes).unwrap();
        assert!(raw.is_encrypted());
        assert_eq!(raw.entries.len(), 2);
        assert_eq!(raw.entries[0].name, b"__key__.dat");
    }

    #[test]
    fn decrypts_complete_payload_blocks_and_preserves_partial_tail() {
        let directory = encrypted_directory(0, 11);
        let archive = Archive::parse_directory(&directory, 11, Some(0)).unwrap();
        let key = archive.payload_key().unwrap();
        let cipher = BlowfishLE::new_from_slice(&key.to_le_bytes()).unwrap();
        let mut encrypted: Vec<u8> = (0..8).collect();
        cipher.encrypt_block((&mut encrypted[..8]).into());
        encrypted.extend_from_slice(b"xyz");
        archive.decrypt_payload(&mut encrypted);
        assert_eq!(&encrypted[..8], &(0..8).collect::<Vec<_>>());
        assert_eq!(&encrypted[8..], b"xyz");

        let payload_offset = directory.len() as u32;
        let directory = encrypted_directory(payload_offset, 11);
        let mut file = directory.clone();
        let mut encrypted_payload = b"12345678xyz".to_vec();
        cipher.encrypt_block((&mut encrypted_payload[..8]).into());
        file.extend_from_slice(&encrypted_payload);
        let archive = Archive::parse_with_name_seed(&file, 0).unwrap();
        assert_eq!(
            archive.read(&file, b"1.2").unwrap(),
            Some(b"12345678xyz".to_vec())
        );
    }

    #[test]
    fn complete_parse_api_uses_directory_parser_and_shared_payload_decryption() {
        let mut data = plain_directory(b"payload", 80, 4);
        data.extend_from_slice(b"data");
        let archive = Archive::parse(&data).unwrap();
        assert_eq!(
            archive.read(&data, b"payload").unwrap(),
            Some(b"data".to_vec())
        );
        assert_eq!(archive.payload_key(), None);
    }
}

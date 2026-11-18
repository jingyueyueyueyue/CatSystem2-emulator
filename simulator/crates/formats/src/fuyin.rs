//! Independently recovered from ISLAND cnimage.int and Accelerator.dll's
//! directory loader at RVA 0x93d0 (SHA-256 recorded in the recovery report).
//! No executable code or third-party implementation is loaded or reused.
use std::io::{Cursor, Read, Seek, SeekFrom};

use thiserror::Error;

use super::ArchiveEntry;
use crate::util::{MAX_BUFFER, MAX_ITEMS, u32_at};

pub const SIGNATURE: &[u8; 8] = b"FUYINPAK";
const HEADER_SIZE: u64 = 24;
const ENTRY_SIZE: usize = 20;
const NAME_CAPACITY: usize = 260;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FuyinArchive {
    /// Raw NUL-truncated names and absolute payload ranges, in on-disk order.
    /// Existing archive readers can read these ranges without decryption.
    pub entries: Vec<ArchiveEntry>,
}

#[derive(Debug, Error)]
pub enum FuyinError {
    #[error("invalid FUYINPAK signature")]
    InvalidSignature,
    #[error("FUYINPAK header or directory is truncated")]
    Truncated,
    #[error("FUYINPAK directory exceeds the format reader limit")]
    TooLarge,
    #[error("FUYINPAK directory length is not a multiple of 20")]
    InvalidDirectory,
    #[error("FUYINPAK entry {0} has an invalid name range or terminator")]
    InvalidName(usize),
    #[error("FUYINPAK entry {0} points outside the payload area")]
    OutOfBounds(usize),
    #[error("FUYINPAK entry {entry} has unverified size fields {stored}/{auxiliary}")]
    UnsupportedStorage { entry: usize, stored: u32, auxiliary: u32 },
    #[error("read FUYINPAK: {0}")]
    Io(#[from] std::io::Error),
}

impl FuyinArchive {
    pub fn parse(data: &[u8]) -> Result<Self, FuyinError> {
        Self::read_directory(&mut Cursor::new(data), data.len() as u64)
    }

    /// Reads only the fixed header, trailing directory and bounded name spans.
    /// Payload bytes stay in the caller's file and use the normal range reader.
    pub fn read_directory<R: Read + Seek>(
        reader: &mut R,
        archive_length: u64,
    ) -> Result<Self, FuyinError> {
        if archive_length < HEADER_SIZE { return Err(FuyinError::Truncated); }
        reader.seek(SeekFrom::Start(0))?;
        let mut header = [0u8; HEADER_SIZE as usize];
        reader.read_exact(&mut header)?;
        if header.get(..8) != Some(SIGNATURE.as_slice()) {
            return Err(FuyinError::InvalidSignature);
        }
        // The loader reads these two DWORDs at +16/+20. The observed +8/+12
        // words are 24/0; it does not use them to relocate directory entries.
        let table_start = u32_at(&header, 16).ok_or(FuyinError::Truncated)? as u64;
        let table_size = u32_at(&header, 20).ok_or(FuyinError::Truncated)? as usize;
        if table_size % ENTRY_SIZE != 0 { return Err(FuyinError::InvalidDirectory); }
        let count = table_size / ENTRY_SIZE;
        if count > MAX_ITEMS || table_size > MAX_BUFFER { return Err(FuyinError::TooLarge); }
        if table_start < HEADER_SIZE
            || table_start.checked_add(table_size as u64).is_none_or(|end| end > archive_length)
        { return Err(FuyinError::Truncated); }
        reader.seek(SeekFrom::Start(table_start))?;
        let mut table = vec![0u8; table_size];
        reader.read_exact(&mut table)?;
        let mut entries = Vec::with_capacity(count);
        let mut name_bytes = 0usize;
        let mut names_start = table_start;
        for (index, row) in table.chunks_exact(ENTRY_SIZE).enumerate() {
            let name_start = u32_at(row, 0).ok_or(FuyinError::InvalidDirectory)? as u64;
            let name_size = u32_at(row, 4).ok_or(FuyinError::InvalidDirectory)? as usize;
            let offset = u32_at(row, 8).ok_or(FuyinError::InvalidDirectory)? as u64;
            let stored = u32_at(row, 12).ok_or(FuyinError::InvalidDirectory)?;
            let auxiliary = u32_at(row, 16).ok_or(FuyinError::InvalidDirectory)?;
            if stored != auxiliary {
                // All 458 observed entries are verbatim PNGs. Do not guess a
                // codec from unequal fields: the PNG receiver only uses the
                // payload offset, so it does not establish a decoding rule.
                return Err(FuyinError::UnsupportedStorage { entry: index, stored, auxiliary });
            }
            if name_start < HEADER_SIZE || name_size == 0 || name_size > NAME_CAPACITY
                || name_start.checked_add(name_size as u64).is_none_or(|end| end > table_start)
            { return Err(FuyinError::InvalidName(index)); }
            name_bytes = name_bytes.checked_add(name_size)
                .filter(|&size| size <= MAX_BUFFER).ok_or(FuyinError::TooLarge)?;
            reader.seek(SeekFrom::Start(name_start))?;
            let mut name = vec![0u8; name_size];
            reader.read_exact(&mut name)?;
            // The original loader zeroes its 260-byte name field first. A
            // shorter field may therefore omit the on-disk terminating NUL.
            let end = name.iter().position(|&b| b == 0).unwrap_or(name.len());
            if end == 0 || end == NAME_CAPACITY { return Err(FuyinError::InvalidName(index)); }
            name.truncate(end);
            if offset < HEADER_SIZE
                || offset.checked_add(stored as u64).is_none_or(|end| end > table_start)
            { return Err(FuyinError::OutOfBounds(index)); }
            names_start = names_start.min(name_start);
            entries.push(ArchiveEntry { name, offset, size: stored as u64 });
        }
        // Real data is followed by the names, then the directory. Permit
        // shared/overlapping payload ranges, but never read names as pixels.
        for (index, entry) in entries.iter().enumerate() {
            if entry.offset.checked_add(entry.size).is_none_or(|end| end > names_start) {
                return Err(FuyinError::OutOfBounds(index));
            }
        }
        Ok(Self { entries })
    }
}

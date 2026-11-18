use crate::util::{MAX_BUFFER, inflate_exact, range, u32_at};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FesFile {
    pub compressed_length: u32,
    pub text: Vec<u8>,
    pub reserved: u32,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum FesError {
    #[error("invalid FES signature")]
    InvalidSignature,
    #[error("FES container exceeds the format reader limit")]
    TooLarge,
    #[error("FES container is truncated")]
    Truncated,
    #[error("FES length or declared text size is invalid")]
    InvalidLength,
    #[error("FES reserved header field is invalid")]
    InvalidReserved,
    #[error("FES zlib stream is invalid")]
    InvalidCompression,
}

impl FesFile {
    pub fn parse(data: &[u8]) -> Result<Self, FesError> {
        if data.len() > MAX_BUFFER {
            return Err(FesError::TooLarge);
        }
        if data.get(..4) != Some(b"FES\0") {
            return Err(FesError::InvalidSignature);
        }
        let compressed_length = u32_at(data, 4).ok_or(FesError::Truncated)?;
        let text_length = u32_at(data, 8).ok_or(FesError::Truncated)? as usize;
        let reserved = u32_at(data, 12).ok_or(FesError::Truncated)?;
        let compressed_length =
            usize::try_from(compressed_length).map_err(|_| FesError::InvalidLength)?;
        if text_length > MAX_BUFFER {
            return Err(FesError::InvalidLength);
        }
        if reserved != 0 {
            return Err(FesError::InvalidReserved);
        }
        if compressed_length == 0 {
            return Err(FesError::InvalidLength);
        }
        let expected_file_length = 16usize
            .checked_add(compressed_length)
            .ok_or(FesError::InvalidLength)?;
        if data.len() < expected_file_length {
            return Err(FesError::Truncated);
        }
        if data.len() != expected_file_length {
            return Err(FesError::InvalidLength);
        }
        let compressed = range(data, 16, compressed_length).ok_or(FesError::Truncated)?;
        let text = inflate_exact(compressed, text_length).ok_or(FesError::InvalidCompression)?;
        Ok(Self {
            compressed_length: compressed_length as u32,
            text,
            reserved,
        })
    }
}

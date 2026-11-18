//! Legacy 72c9 model envelope (49118d..4911e6), independently decoded.
//! The payload is COLLADA XML; this is not the later MDL graph format.
use thiserror::Error;
use crate::util::{MAX_BUFFER, inflate_exact, u32_at};

#[derive(Debug, Error)]
pub enum KxsError {
    #[error("KXS header/payload length is invalid")]
    Length,
    #[error("KXS zlib payload is invalid")]
    Compression,
}

/// Accept raw DAE bytes or the exact lowercase kxs\0 envelope. Never infer
/// compression from arbitrary XML contents or accept a truncated stream.
pub fn decode_model_xml(bytes: &[u8]) -> Result<Vec<u8>, KxsError> {
    if bytes.len() > MAX_BUFFER { return Err(KxsError::Length); }
    if bytes.get(..4) != Some(b"kxs\0") { return Ok(bytes.to_vec()); }
    let expanded = u32_at(bytes, 4).ok_or(KxsError::Length)? as usize;
    let compressed = u32_at(bytes, 8).ok_or(KxsError::Length)? as usize;
    if expanded > MAX_BUFFER || compressed == 0
        || compressed.checked_add(12) != Some(bytes.len()) {
        return Err(KxsError::Length);
    }
    inflate_exact(&bytes[12..], expanded).ok_or(KxsError::Compression)
}

use std::ops::Range;
use thiserror::Error;

use crate::util::{
    MAX_BUFFER, MAX_ITEMS, c_string, inflate_at_most, inflate_exact,
    inflate_missing_adler32_zero, range, u32_at,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CstVariant {
    Uncompressed,
    Zlib,
    ZlibDeclaredLengthMismatch,
    /// A complete DEFLATE body with three Adler32 bytes; the last byte is zero.
    ZlibMissingAdler32Zero,
    XorObfuscatedZlib,
    /// The XOR encoder overwrote a key word; a unique verified seed was recovered.
    XorObfuscatedZlibRecoveredKey,
    LegacyUnknownCompression,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CstLengthInfo {
    pub declared: u32,
    pub actual: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CstXorInfo {
    /// Index in the complete CatScene container's little-endian DWORD array.
    pub key_word_index: u32,
    pub initial_key: u32,
    pub increment: u32,
    pub recovered_key_word: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CstDiagnostic {
    DeclaredLengthMismatch(CstLengthInfo),
    ScriptLengthMismatch(CstLengthInfo),
    MissingAdler32Zero,
    XorObfuscation(CstXorInfo),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CstRecord {
    pub kind: u8,
    pub text: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CstFile {
    pub variant: CstVariant,
    pub length: CstLengthInfo,
    pub script_length: CstLengthInfo,
    pub diagnostics: Vec<CstDiagnostic>,
    pub records: Vec<CstRecord>,
    pub inputs: Vec<Range<usize>>,
}

#[derive(Debug, Error)]
pub enum CstError {
    #[error("CST file is truncated")]
    Truncated,
    #[error("CST file exceeds the format reader limit")]
    TooLarge,
    #[error("invalid CST signature")]
    InvalidSignature,
    #[error("CST zlib stream is invalid")]
    InvalidCompression,
    #[error("CST uses an unsupported compression variant: {0:?}")]
    UnsupportedVariant(CstVariant),
    #[error("CST table points outside the file")]
    OutOfBounds,
    #[error("CST record has an invalid prefix")]
    InvalidRecord,
    #[error("CST input ranges are not contiguous")]
    InvalidInputs,
    #[error("CST XOR key recovery has multiple checksum-valid candidates")]
    AmbiguousXorKey,
}

impl CstFile {
    pub fn parse(data: &[u8]) -> Result<Self, CstError> {
        if data.len() > MAX_BUFFER {
            return Err(CstError::TooLarge);
        }
        if data.get(..8) != Some(b"CatScene") {
            return Err(CstError::InvalidSignature);
        }
        let compressed_size = u32_at(data, 8).ok_or(CstError::Truncated)? as usize;
        let decompressed_size = u32_at(data, 12).ok_or(CstError::Truncated)? as usize;
        if decompressed_size > MAX_BUFFER {
            return Err(CstError::OutOfBounds);
        }
        let stored_size = if compressed_size == 0 {
            decompressed_size
        } else {
            compressed_size
        };
        let compressed = range(data, 16, stored_size).ok_or(CstError::Truncated)?;
        if data.len() != 16 + stored_size {
            return Err(CstError::OutOfBounds);
        }
        let declared_length =
            u32::try_from(decompressed_size).map_err(|_| CstError::OutOfBounds)?;
        let (body, mut variant, mut diagnostics) = if compressed_size == 0 {
            (compressed.to_vec(), CstVariant::Uncompressed, Vec::new())
        } else {
            let (body, base_variant, mut diagnostics) =
                if let Some(body) = inflate_at_most(compressed, decompressed_size) {
                    (body, CstVariant::Zlib, Vec::new())
                } else if let Some(body) = if is_zlib_header(compressed) {
                    inflate_missing_adler32_zero(compressed, decompressed_size)
                } else {
                    None
                } {
                    (
                        body,
                        CstVariant::ZlibMissingAdler32Zero,
                        vec![CstDiagnostic::MissingAdler32Zero],
                    )
                } else if let Some((body, info)) = inflate_xor_cst(data, decompressed_size)? {
                    (
                        body,
                        if info.recovered_key_word {
                            CstVariant::XorObfuscatedZlibRecoveredKey
                        } else {
                            CstVariant::XorObfuscatedZlib
                        },
                        vec![CstDiagnostic::XorObfuscation(info)],
                    )
                } else if is_zlib_header(compressed) {
                    return Err(CstError::InvalidCompression);
                } else {
                    return Err(CstError::UnsupportedVariant(
                        CstVariant::LegacyUnknownCompression,
                    ));
                };
            let actual = u32::try_from(body.len()).map_err(|_| CstError::OutOfBounds)?;
            if actual == declared_length {
                (body, base_variant, diagnostics)
            } else {
                diagnostics.push(CstDiagnostic::DeclaredLengthMismatch(CstLengthInfo {
                    declared: declared_length,
                    actual,
                }));
                (
                    body,
                    CstVariant::ZlibDeclaredLengthMismatch,
                    diagnostics,
                )
            }
        };

        let script_length = u32_at(&body, 0).ok_or(CstError::Truncated)? as usize;
        let actual_script_length = body.len().checked_sub(16).ok_or(CstError::Truncated)?;
        let declared_script_length =
            u32::try_from(script_length).map_err(|_| CstError::OutOfBounds)?;
        let actual_script_length_u32 =
            u32::try_from(actual_script_length).map_err(|_| CstError::OutOfBounds)?;
        if script_length != actual_script_length {
            // Translation patches retain the original script-length field
            // while replacing the string table. UPDATE05.int contains both
            // larger and smaller declarations. All accesses below are bounded
            // by the actual, checksum-verified body instead of this stale field.
            variant = match variant {
                CstVariant::Zlib => CstVariant::ZlibDeclaredLengthMismatch,
                other => other,
            };
            diagnostics.push(CstDiagnostic::ScriptLengthMismatch(CstLengthInfo {
                declared: declared_script_length,
                actual: actual_script_length_u32,
            }));
        }
        let input_count = u32_at(&body, 4).ok_or(CstError::Truncated)? as usize;
        let offset_table = u32_at(&body, 8).ok_or(CstError::Truncated)? as usize;
        let string_table = u32_at(&body, 12).ok_or(CstError::Truncated)? as usize;
        let input_table_length = input_count.checked_mul(8).ok_or(CstError::OutOfBounds)?;
        if input_count > MAX_ITEMS
            || input_count > actual_script_length / 8
            || offset_table < input_table_length
            || offset_table > string_table
            || string_table > actual_script_length
            || (string_table - offset_table) % 4 != 0
        {
            return Err(CstError::OutOfBounds);
        }

        let record_count = (string_table - offset_table) / 4;
        if record_count > MAX_ITEMS {
            return Err(CstError::OutOfBounds);
        }
        let mut records = Vec::with_capacity(record_count);
        let mut text_bytes = 0usize;
        for index in 0..record_count {
            let relative =
                u32_at(&body, 16 + offset_table + index * 4).ok_or(CstError::Truncated)? as usize;
            let record_offset = string_table
                .checked_add(relative)
                .and_then(|offset| offset.checked_add(16))
                .ok_or(CstError::OutOfBounds)?;
            let raw = c_string(&body, record_offset).ok_or(CstError::OutOfBounds)?;
            if raw.len() < 2 || raw[0] != 1 {
                return Err(CstError::InvalidRecord);
            }
            // Offsets can alias or be out of order in real translation
            // patches. Bound the owned copies instead of rejecting that
            // layout or allowing a small string table to expand without limit.
            text_bytes = text_bytes
                .checked_add(raw.len() - 2)
                .filter(|&length| length <= MAX_BUFFER)
                .ok_or(CstError::TooLarge)?;
            records.push(CstRecord {
                kind: raw[1],
                text: raw[2..].to_vec(),
            });
        }

        let mut inputs = Vec::with_capacity(input_count);
        let mut previous = 0;
        for index in 0..input_count {
            let length = u32_at(&body, 16 + index * 8).ok_or(CstError::Truncated)? as usize;
            let start = u32_at(&body, 20 + index * 8).ok_or(CstError::Truncated)? as usize;
            let end = start.checked_add(length).ok_or(CstError::OutOfBounds)?;
            if start != previous || end > record_count {
                return Err(CstError::InvalidInputs);
            }
            inputs.push(start..end);
            previous = end;
        }
        if previous != record_count {
            return Err(CstError::InvalidInputs);
        }

        Ok(Self {
            variant,
            length: CstLengthInfo {
                declared: declared_length,
                actual: u32::try_from(body.len()).map_err(|_| CstError::OutOfBounds)?,
            },
            script_length: CstLengthInfo {
                declared: declared_script_length,
                actual: actual_script_length_u32,
            },
            diagnostics,
            records,
            inputs,
        })
    }
}

fn is_zlib_header(data: &[u8]) -> bool {
    let Some(&cmf) = data.first() else {
        return false;
    };
    let Some(&flg) = data.get(1) else {
        return false;
    };
    cmf & 0x0f == 8
        && cmf >> 4 <= 7
        && ((u16::from(cmf) << 8 | u16::from(flg)) % 31 == 0)
}

/// Restores the DWORD mask in the local Grisaia UPDATE01.INT patch.
///
/// The native hook at RVA 0x3c96e5 derives `bound` from the complete resource
/// length, then uses it both as a byte offset for the two key words and as a
/// DWORD-index bound. Keeping those two units distinct is essential.
fn inflate_xor_cst(
    data: &[u8],
    expected: usize,
) -> Result<Option<(Vec<u8>, CstXorInfo)>, CstError> {
    if data.len() < 48 || expected < 16 {
        return Ok(None);
    }
    let bound = ((data.len() & !15) / 4) - 4;
    let key_index = bound / 4;
    if bound < 8 || bound.checked_mul(4).and_then(|n| n.checked_add(4)) > Some(data.len()) {
        return Ok(None);
    }
    let Some(left) = u32_at(data, bound) else {
        return Ok(None);
    };
    let Some(step) = u32_at(data, bound + 4) else {
        return Ok(None);
    };
    if key_index != 3 {
        return Ok(unmask_and_inflate(data, expected, bound, key_index, left, step).map(
            |body| {
                (
                    body,
                    CstXorInfo {
                        key_word_index: key_index as u32,
                        initial_key: left ^ step,
                        increment: step,
                        recovered_key_word: false,
                    },
                )
            },
        ));
    }

    // For a 48..63-byte stored payload, the encoder started past key word 3
    // and overwrote key word 4 with word 3. The two observed files use a
    // fixed-Huffman block beginning with the inner script-length field.
    // Enumerate only legal no-dictionary zlib headers and the two possible
    // BFINAL values. Nothing is accepted without a unique complete stream,
    // exact expanded length, Adler32, and the expected inner length.
    let declared = expected as u32;
    if u32_at(data, 16) != Some(declared) {
        return Ok(None);
    }
    let script_length = declared - 16;
    let mut recovered = None;
    for final_block in [false, true] {
        let prefix = fixed_length_prefix(script_length, final_block).to_le_bytes();
        for window in 0u8..8 {
            let cmf = (window << 4) | 8;
            for level in 0u8..4 {
                let flags = level << 6;
                let header = (u16::from(cmf) << 8) | u16::from(flags);
                let flags = flags + ((31 - header % 31) % 31) as u8;
                let seed = u32::from_le_bytes([cmf, flags, prefix[0], prefix[1]]);
                let Some(body) =
                    unmask_and_inflate(data, expected, bound, key_index, declared, seed)
                else {
                    continue;
                };
                if u32_at(&body, 0) != Some(script_length) {
                    continue;
                }
                if recovered.is_some() {
                    return Err(CstError::AmbiguousXorKey);
                }
                recovered = Some((
                    body,
                    CstXorInfo {
                        key_word_index: key_index as u32,
                        initial_key: declared ^ seed,
                        increment: seed,
                        recovered_key_word: true,
                    },
                ));
            }
        }
    }
    Ok(recovered)
}

fn unmask_and_inflate(
    data: &[u8],
    expected: usize,
    bound: usize,
    key_index: usize,
    left: u32,
    step: u32,
) -> Option<Vec<u8>> {
    let mut decoded = data.to_vec();
    let mut key = left ^ step;
    let mut index = 4;
    while index <= bound {
        // The encoded stream retains both words, including when they occur
        // at the start of the payload. The original hook's 0xda78 shortcut
        // skips thirteen such streams even though their later bytes are masked.
        if index == key_index {
            index += 2;
            continue;
        }
        let offset = index.checked_mul(4)?;
        let value = u32_at(&decoded, offset)? ^ key;
        decoded
            .get_mut(offset..offset.checked_add(4)?)?
            .copy_from_slice(&value.to_le_bytes());
        key = key.wrapping_add(step);
        index += 1;
    }
    let payload = decoded.get(16..)?;
    if !is_zlib_header(payload) {
        return None;
    }
    inflate_exact(payload, expected)
}

/// Low 16 bits of a fixed-Huffman DEFLATE block encoding the first two
/// little-endian bytes of the known inner length; no text bytes are inferred.
fn fixed_length_prefix(script_length: u32, final_block: bool) -> u16 {
    let mut bits = 2 | u32::from(final_block);
    let mut shift = 3;
    for value in script_length.to_le_bytes().into_iter().take(2) {
        let (code, width) = if value < 144 {
            (0x30 + u32::from(value), 8)
        } else {
            (0x190 + u32::from(value) - 144, 9)
        };
        let reversed = code.reverse_bits() >> (32 - width);
        bits |= reversed << shift;
        shift += width;
    }
    bits as u16
}

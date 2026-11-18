//! Native933:17eea0 produces a CP932 bank/driver-value text dump.24cbb0
//! sends that text to the host clipboard (CF_TEXT), not to a save-state file.
//! This module owns only the snapshot and its concrete byte serialization.

use std::collections::BTreeMap;

use super::super::{EmoteError, Result};

const MAX_BYTES: usize = 16 * 1024 * 1024;
const FILE: &[u8] = b"\x83\x74\x83\x40\x83\x43\x83\x8b,";
const CHARACTER: &[u8] = b"\x83\x4c\x83\x83\x83\x89\x83\x4e\x83\x5e,";
const MOTION: &[u8] = b"\x83\x82\x81\x5b\x83\x56\x83\x87\x83\x93,";

#[derive(Debug, Clone)]
struct Bank {
    resource_cp932: Vec<u8>,
    character_utf8: String,
    motion_utf8: String,
    values: BTreeMap<String, f32>,
}

/// An owned, momentary view of selector0 banks. Capture registered names from
/// the live value table, including names retained across profile changes.
/// Values are actual driver virtual94 samples when a driver exists; only a
/// missing driver uses value+30. Do not substitute config defaults or export
/// all PSB variables indiscriminately. No timer/force/blink owner is mutated.
#[derive(Debug, Clone, Default)]
pub(in crate::emote) struct CpuM2DebugText {
    //16b787/16b78a compare task ids as signed; header %d does the same.
    banks: BTreeMap<i32, Bank>,
}

impl CpuM2DebugText {
    ///17ef6a..17efd8 filters on nonempty source vector/character/motion.
    ///Only the first source string is printed, even for a multi-source bank.
    ///character/motion/value names are internal UTF-8, not CP932 aliases.
    pub(in crate::emote) fn insert_bank(
        &mut self,
        bank_id: u32,
        resources_cp932: &[Vec<u8>],
        character_utf8: &str,
        motion_utf8: &str,
        registered_values: impl IntoIterator<Item = (String, f32)>,
    ) -> Result<()> {
        let Some(resource) = resources_cp932.first() else { return Ok(()); };
        if character_utf8.is_empty() || motion_utf8.is_empty() {
            return Ok(());
        }
        if self.banks.len() >= 100_000 {
            return Err(EmoteError::Capacity);
        }
        let mut values = BTreeMap::new();
        for (name, value) in registered_values {
            if values.len() >= 100_000 {
                return Err(EmoteError::Capacity);
            }
            if values.insert(name, value).is_some() {
                return Err(EmoteError::InvalidResource("duplicate M2 debug registered name"));
            }
        }
        let bank = Bank {
            resource_cp932: resource.clone(),
            character_utf8: character_utf8.to_owned(),
            motion_utf8: motion_utf8.to_owned(),
            values,
        };
        if self.banks.insert(bank_id as i32, bank).is_some() {
            return Err(EmoteError::InvalidResource("duplicate M2 debug bank id"));
        }
        Ok(())
    }

    ///Bytes exclude NUL. Empty output means native933 must skip clipboard
    ///publication entirely, retaining its previous contents (24cbc1/24cbc3).
    ///The host adds one NUL only when publishing a nonempty CF_TEXT payload.
    pub(in crate::emote) fn into_cp932(self) -> Result<Vec<u8>> {
        let mut output = Vec::new();
        for (id, bank) in self.banks {
            append(&mut output, format!("[BANK ID : {id}]\r\n").as_bytes())?;
            line(&mut output, FILE, c_prefix(&bank.resource_cp932))?;
            line(&mut output, CHARACTER, &driver_name_cp932(&bank.character_utf8)?)?;
            line(&mut output, MOTION, &driver_name_cp932(&bank.motion_utf8)?)?;
            //16e3f0 visits registered UTF-8 byte keys in tree order, before
            //17f1a5 converts each name. Sorting the encoded keys is wrong.
            for (name, value) in bank.values {
                if !value.is_finite() {
                    return Err(EmoteError::Unsupported {
                        native: 933,
                        reason: "M2 debug legacy CRT nonfinite scalar spelling is not recovered",
                    });
                }
                append(&mut output, &driver_name_cp932(&name)?)?;
                //17f1ba..17f1d8 rounds to f32 then promotes to double for %f.
                append(&mut output, format!(",{:.6}\r\n", f64::from(value)).as_bytes())?;
            }
            append(&mut output, b"\r\n")?;
        }
        Ok(output)
    }
}

fn c_prefix(bytes: &[u8]) -> &[u8] {
    &bytes[..bytes.iter().position(|&byte| byte == 0).unwrap_or(bytes.len())]
}

fn driver_name_cp932(name: &str) -> Result<Vec<u8>> {
    //2a54e0 clears a string longer than4000 UTF-8 bytes before conversion.
    if name.len() > 4000 {
        return Ok(Vec::new());
    }
    let prefix = std::str::from_utf8(c_prefix(name.as_bytes()))
        .map_err(|_| EmoteError::InvalidResource("M2 debug UTF-8 name"))?;
    //2a55ba uses CP_UTF8(65001);2a5607/2a5633 use CP932(932).
    //There is no CSV quoting, escaping or substitution of punctuation here.
    let (bytes, _, invalid) = encoding_rs::SHIFT_JIS.encode(prefix);
    if invalid {
        return Err(EmoteError::Unsupported {
            native: 933,
            reason: "M2 debug Windows CP932 default/best-fit conversion is not recovered",
        });
    }
    Ok(bytes.into_owned())
}

fn append(output: &mut Vec<u8>, bytes: &[u8]) -> Result<()> {
    if output.len().checked_add(bytes.len()).is_none_or(|size| size > MAX_BYTES) {
        return Err(EmoteError::Capacity);
    }
    output.extend_from_slice(bytes);
    Ok(())
}

fn line(output: &mut Vec<u8>, label: &[u8], text: &[u8]) -> Result<()> {
    append(output, label)?;
    append(output, text)?;
    append(output, b"\r\n")
}

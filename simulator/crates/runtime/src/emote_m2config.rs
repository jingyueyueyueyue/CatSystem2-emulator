//! Bounded byte-name M2 configuration reader from actual m2config.int records.
//!
//! Text stays CP932 bytes for the host's resource namespace. This reads real
//! #profile/#charaname/#motname/#position/#scale records; it does not guess the
//! native906 compound grammar or silently implement physics/action sections.

use super::{EmoteError, Result};
use std::collections::BTreeMap;

const MAX_CONFIG_BYTES: usize = 4 * 1024 * 1024;
const MAX_RECORDS: usize = 100_000;

#[derive(Debug, Clone, Default)]
pub struct CpuM2Config {
    pub sections: BTreeMap<Vec<u8>, Vec<Vec<Vec<u8>>>>,
}

#[derive(Debug, Clone)]
pub struct CpuM2ProfileSelection {
    pub chara_id: i32,
    pub motion_id: i32,
    pub player_name_cp932: Vec<u8>,
    pub resource_stem_cp932: Vec<u8>,
    /// A label in metadata.timelineControl, separate from metadata.base.motion.
    pub timeline_label_cp932: Vec<u8>,
    pub difference_files_cp932: Vec<Vec<u8>>,
}

#[derive(Debug, Clone)]
pub struct CpuM2LoadSpec {
    pub profile_alias_cp932: Vec<u8>,
    pub chara_id: i32,
    /// Colon suffix; original absent suffix defaults to byte string"0".
    pub motion_id: i32,
    /// Comma tokens from the third onward are a separate pattern vector.
    pub pattern_selectors_cp932: Vec<Vec<u8>>,
}

///17e950 separates the secondary dimension vector from a colon player key.
///The suffix chooses an existing manager player, never a PSB timeline.
pub(super) struct CpuM2PlaySpec {
    pub patterns: Vec<Vec<u8>>,
    pub player: Vec<u8>,
}

impl CpuM2PlaySpec {
    pub fn parse_unquoted(bytes: &[u8]) -> Result<Self> {
        let bytes = bytes.strip_suffix(&[0]).unwrap_or(bytes);
        if bytes.len() > 4095 || bytes.contains(&0)
            || bytes.iter().any(|&b| b == b'"' || b == b'\'' || b.is_ascii_whitespace())
        {
            return Err(invalid("unsupported quoted/whitespace M2 play specification"));
        }
        let mut parts = bytes.split(|&b| b == b':');
        let dimensions = parts.next().unwrap_or_default();
        let player = parts.next().unwrap_or(b"0").to_vec();
        if parts.next().is_some() {
            return Err(invalid("multiple M2 play player suffixes"));
        }
        let patterns = if dimensions.is_empty() { Vec::new() } else {
            dimensions.split(|&b| b == b',').map(<[u8]>::to_vec).collect()
        };
        Ok(Self { patterns, player })
    }
}

impl CpuM2LoadSpec {
    /// Verified unquoted grammar from17e530,restricted before the original
    /// general tokenizer's escaping/quoting behavior has been recovered.
    pub fn parse_unquoted(bytes: &[u8]) -> Result<Self> {
        let bytes = bytes.strip_suffix(&[0]).unwrap_or(bytes);
        if bytes.len() > 4095
            || bytes.contains(&0)
            || bytes
                .iter()
                .any(|&b| b == b'"' || b == b'\'' || b.is_ascii_whitespace())
        {
            return Err(invalid(
                "unsupported quoted/whitespace M2 load specification",
            ));
        }
        let colon = bytes.split(|&b| b == b':').collect::<Vec<_>>();
        if colon.len() > 2 {
            return Err(invalid("multiple M2 colon suffixes"));
        }
        let mut main = colon[0]
            .split(|&b| b == b',')
            .map(<[u8]>::to_vec)
            .collect::<Vec<_>>();
        if main.first().is_none_or(Vec::is_empty) {
            return Err(invalid("empty M2 profile alias"));
        }
        let extra = if main.len() > 2 {
            main.split_off(2)
        } else {
            Vec::new()
        };
        while main.len() < 3 {
            main.push(b"0".to_vec());
        }
        Ok(Self {
            profile_alias_cp932: main[0].clone(),
            chara_id: integer(&main[1])?,
            motion_id: integer(colon.get(1).copied().unwrap_or(b"0"))?,
            pattern_selectors_cp932: extra,
        })
    }
}

#[derive(Debug, Clone, Default)]
pub struct CpuM2DifferenceTransform {
    pub include_files_cp932: Vec<Vec<u8>>,
    pub position: Option<[i32; 2]>,
    pub scale: Option<[f32; 2]>,
    pub scale_offset: Option<[i32; 2]>,
}

fn invalid(why: &'static str) -> EmoteError {
    EmoteError::InvalidResource(why)
}

fn integer(bytes: &[u8]) -> Result<i32> {
    std::str::from_utf8(bytes)
        .map_err(|_| invalid("M2 integer bytes"))?
        .parse()
        .map_err(|_| invalid("M2 integer"))
}

/// Shared comment/CP932 pass for difference records and spline directives.
pub(super) fn clean_cp932(bytes: &[u8]) -> Result<Vec<u8>> {
        if bytes.len() > MAX_CONFIG_BYTES {
            return Err(EmoteError::Capacity);
        }
        let mut cleaned = Vec::with_capacity(bytes.len());
        let mut index = 0usize;
        let mut comment = false;
        while index < bytes.len() {
            if comment {
                if bytes.get(index..index + 2) == Some(b"*/") {
                    comment = false;
                    index += 2;
                } else {
                    if matches!(bytes[index], b'\r' | b'\n') {
                        cleaned.push(bytes[index]);
                    }
                    index += 1;
                }
            } else if bytes.get(index..index + 2) == Some(b"/*") {
                comment = true;
                cleaned.push(b' ');
                index += 2;
            } else if bytes.get(index..index + 2) == Some(b"//") {
                while index < bytes.len() && bytes[index] != b'\n' {
                    index += 1;
                }
            } else {
                // Skip a complete CP932 double-byte character so a trail
                // backslash is never mistaken for syntax by future extensions.
                let lead = bytes[index];
                cleaned.push(lead);
                index += 1;
                if (0x81..=0x9f).contains(&lead) || (0xe0..=0xfc).contains(&lead) {
                    let trail = *bytes
                        .get(index)
                        .ok_or_else(|| invalid("truncated CP932 M2 text"))?;
                    if trail < 0x40 || trail == 0x7f || trail > 0xfc {
                        return Err(invalid("invalid CP932 M2 trail byte"));
                    }
                    cleaned.push(trail);
                    index += 1;
                }
            }
        }
        if comment {
            return Err(invalid("unterminated M2 block comment"));
        }
        Ok(cleaned)
}

impl CpuM2Config {
    pub fn parse_cp932(bytes: &[u8]) -> Result<Self> {
        let cleaned = clean_cp932(bytes)?;
        let mut result = Self::default();
        let mut section = Vec::new();
        let mut records = 0usize;
        for line in cleaned.split(|&b| b == b'\n') {
            let words: Vec<Vec<u8>> = line
                .split(|b| b.is_ascii_whitespace())
                .filter(|word| !word.is_empty())
                .map(<[u8]>::to_vec)
                .collect();
            if words.is_empty() {
                continue;
            }
            if words[0].first() == Some(&b'#') {
                if words.len() != 1 || words[0].len() < 2 {
                    return Err(invalid("M2 section directive"));
                }
                section = words[0][1..].to_ascii_lowercase();
                result.sections.entry(section.clone()).or_default();
                continue;
            }
            if section.is_empty() {
                return Err(invalid("M2 record outside a section"));
            }
            records += 1;
            if records > MAX_RECORDS || words.iter().any(|w| w.len() > 4095 || w.contains(&0)) {
                return Err(EmoteError::Capacity);
            }
            result
                .sections
                .entry(section.clone())
                .or_default()
                .push(words);
        }
        Ok(result)
    }

    pub fn records(&self, section: &[u8]) -> &[Vec<Vec<u8>>] {
        self.sections
            .get(&section.to_ascii_lowercase())
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub fn profile_file(&self, alias: &[u8]) -> Result<&[u8]> {
        self.records(b"profile")
            .iter()
            .find(|r| r.first().is_some_and(|key| key.eq_ignore_ascii_case(alias)))
            .and_then(|r| r.get(1))
            .map(Vec::as_slice)
            .ok_or_else(|| invalid("M2 profile alias is absent"))
    }

    pub fn select_profile(&self, chara_id: i32, motion_id: i32) -> Result<CpuM2ProfileSelection> {
        let mut chara = None;
        for row in self.records(b"charaname") {
            if row.len() != 3 {
                return Err(invalid("M2 charaname record"));
            }
            if integer(&row[0])? == chara_id {
                chara = Some(row);
                break;
            }
        }
        let chara = chara.ok_or_else(|| invalid("M2 chara selector is absent"))?;
        let mut motion = None;
        for row in self.records(b"motname") {
            if row.len() < 3 {
                return Err(invalid("M2 motname record"));
            }
            if integer(&row[0])? == chara_id && integer(&row[1])? == motion_id {
                motion = Some(row);
                break;
            }
        }
        let motion = motion.ok_or_else(|| invalid("M2 motion selector is absent"))?;
        Ok(CpuM2ProfileSelection {
            chara_id,
            motion_id,
            player_name_cp932: chara[1].clone(),
            resource_stem_cp932: chara[2].clone(),
            timeline_label_cp932: motion[2].clone(),
            difference_files_cp932: motion[3..].to_vec(),
        })
    }

    pub fn difference_transform(&self) -> Result<CpuM2DifferenceTransform> {
        let pair = |section: &[u8]| -> Result<Option<&[Vec<u8>]>> {
            let rows = self.records(section);
            if rows.is_empty() {
                return Ok(None);
            }
            if rows.len() != 1 || rows[0].len() != 2 {
                return Err(invalid("M2 transform pair"));
            }
            Ok(Some(&rows[0]))
        };
        let position = pair(b"position")?
            .map(|r| -> Result<[i32; 2]> { Ok([integer(&r[0])?, integer(&r[1])?]) })
            .transpose()?;
        let scale_offset = pair(b"scaleoffset")?
            .map(|r| -> Result<[i32; 2]> { Ok([integer(&r[0])?, integer(&r[1])?]) })
            .transpose()?;
        let scale = pair(b"scale")?
            .map(|r| -> Result<[f32; 2]> {
                let parse = |b: &[u8]| -> Result<f32> {
                    let n: f32 = std::str::from_utf8(b)
                        .map_err(|_| invalid("M2 scale bytes"))?
                        .parse()
                        .map_err(|_| invalid("M2 scale"))?;
                    if !n.is_finite() {
                        return Err(invalid("non-finite M2 scale"));
                    }
                    Ok(n)
                };
                Ok([parse(&r[0])?, parse(&r[1])?])
            })
            .transpose()?;
        let include_files_cp932 = self
            .records(b"include")
            .iter()
            .map(|r| {
                if r.len() != 1 {
                    return Err(invalid("M2 include filename"));
                }
                Ok(r[0].clone())
            })
            .collect::<Result<_>>()?;
        Ok(CpuM2DifferenceTransform {
            include_files_cp932,
            position,
            scale,
            scale_offset,
        })
    }
}

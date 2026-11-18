//! Sound sidecars recovered from PE RVAs 0x142f80..0x14333f/0x146160.
//! Owned bytes and names only; no original engine or VM-shaped host pointer.
use std::collections::BTreeMap;
use std::sync::Arc;

use cs2_runtime::resource_router::ResourceMode;
use cs2_script::Result;

use super::sound_native::{decode_name, encode_name};
use super::{LoggingHost, ScriptTextEncoding, audio_script_error, trim_nul};

const MAX_FILE: u64 = 4 * 1024 * 1024;
const MAX_TOTAL: usize = 64 * 1024 * 1024;
const MAX_TAGS: usize = 65_536;

#[derive(Debug, Clone, Default, PartialEq, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(super) struct SoundSidecars {
    pub(super) tags: BTreeMap<Vec<u8>, u32>,
    pub(super) signal: Option<PrecomputedSignal>,
    #[serde(with = "crate::sound_task::checkpoint::arc_bytes")]
    pub(super) lip: Option<Arc<[u8]>>,
    #[serde(with = "crate::sound_task::checkpoint::arc_bytes")]
    pub(super) legacy_lip: Option<Arc<[u8]>>,
    encoded_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(super) struct PrecomputedSignal {
    pub(super) raw_kind: u32,
    pub(super) period_millis: u32,
    #[serde(with = "crate::sound_task::checkpoint::arc_u16")]
    samples: Arc<[u16]>,
}

impl SoundSidecars {
    pub(crate) fn validate_checkpoint(&self) -> anyhow::Result<()> {
        if self.tags.len() > MAX_TAGS || self.tags.keys().any(|name| name.len() >= 20 || name.contains(&0))
            || self.encoded_bytes > MAX_TOTAL || self.lip.as_ref().is_some_and(|v| v.len() > MAX_FILE as usize)
            || self.legacy_lip.as_ref().is_some_and(|v| v.len() > MAX_FILE as usize)
            || self.signal.as_ref().is_some_and(|s| s.samples.len() > MAX_FILE as usize / 2
                || !s.samples.is_empty() && s.period_millis == 0) {
            anyhow::bail!("SOUND checkpoint sidecar storage is invalid");
        }
        Ok(())
    }

    /// The original release helpers clear PLV/LPB/LV while the tag map lives
    /// until a subsequent load or destruction of its owning sound task.
    pub(super) fn clear_auxiliary(&mut self) {
        self.signal = None;
        self.lip = None;
        self.legacy_lip = None;
        self.encoded_bytes = self.tags.len() * 24;
    }
}

impl PrecomputedSignal {
    pub(super) fn level(&self, now: u32, started: u32) -> u32 {
        if self.period_millis == 0 {
            return 0;
        }
        let index = now.wrapping_sub(started) / self.period_millis;
        self.samples
            .get(index as usize)
            .copied()
            .map_or(0, u32::from)
    }

    pub(super) fn window(&self, now: u32, started: u32, output: &mut [u32]) -> usize {
        output.fill(0);
        if self.period_millis == 0 {
            return 0;
        }
        let index = (now.wrapping_sub(started) / self.period_millis) as usize;
        let remaining = self.samples.len().saturating_sub(index);
        let count = remaining.min(output.len());
        for (target, sample) in output[..count]
            .iter_mut()
            .zip(self.samples.iter().skip(index))
        {
            *target = u32::from(*sample);
        }
        count
    }
}

impl LoggingHost<'_> {
    pub(super) fn load_sound_sidecars(&mut self, resource: &str) -> Result<SoundSidecars> {
        // These helpers scan raw narrow bytes, including a CP932 trail 0x5c;
        // they do not share the category resolver's double-byte-aware scan.
        let encoded = encode_name(self.text_encoding, resource)?;
        let encoded = trim_nul(&encoded);
        let start = encoded.iter().rposition(|&byte| matches!(byte, b'/' | b'\\'))
            .filter(|&at| at > 0).map_or(0, |at| at + 1);
        let stem = decode_name(self.text_encoding, without_extension(&encoded[start..]))?;
        let mut result = SoundSidecars::default();
        if let Some(bytes) = self.read_sound_sidecar(b"tag", &stem, None)? {
            if bytes.len() % 24 != 0 {
                return Err(audio_script_error(
                    "sound tag table is not a sequence of 24-byte records",
                ));
            }
            if bytes.len() / 24 > MAX_TAGS {
                return Err(audio_script_error("sound tag table exceeds 65536 records"));
            }
            result.encoded_bytes += bytes.len();
            for record in bytes.chunks_exact(24) {
                let name = trim_nul(&record[..20]);
                if name.len() == 20 {
                    return Err(audio_script_error(
                        "sound tag name lacks a bounded NUL terminator",
                    ));
                }
                let value = u32::from_le_bytes(record[20..24].try_into().unwrap());
                result.tags.entry(name.to_vec()).or_insert(value);
            }
        }
        if let Some(bytes) = self.read_sound_sidecar(b"sound", &stem, Some("plv"))? {
            result.encoded_bytes += bytes.len();
            if bytes.len() < 16 || &bytes[..4] != b"PVL\0" {
                return Err(audio_script_error(
                    "sound PLV file has an invalid PVL header",
                ));
            }
            let word = |offset| u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
            let count = word(8) as usize;
            let period = word(12);
            if count > (bytes.len() - 16) / 2 || (count != 0 && period == 0) {
                return Err(audio_script_error(
                    "sound PLV sample count/period is invalid",
                ));
            }
            result.signal = Some(PrecomputedSignal {
                raw_kind: word(4),
                period_millis: period,
                samples: bytes[16..16 + count * 2]
                    .chunks_exact(2)
                    .map(|sample| u16::from_le_bytes([sample[0], sample[1]]))
                    .collect::<Vec<_>>()
                    .into(),
            });
        }
        // The original .lpb success suppresses the independent .lv fallback.
        if let Some(bytes) = self.read_sound_sidecar(b"sound", &stem, Some("lpb"))? {
            result.encoded_bytes += bytes.len();
            result.lip = Some(bytes.into());
        } else if let Some(bytes) = self.read_sound_sidecar(b"sound", &stem, Some("lv"))? {
            result.encoded_bytes += bytes.len();
            result.legacy_lip = Some(bytes.into());
        }
        let used = self
            .sound_channels
            .values()
            .map(|state| state.task.sidecars.encoded_bytes)
            .sum::<usize>();
        if used
            .checked_add(result.encoded_bytes)
            .is_none_or(|total| total > MAX_TOTAL)
        {
            return Err(audio_script_error("owned sound sidecars exceed 64 MiB"));
        }
        Ok(result)
    }

    fn read_sound_sidecar(
        &mut self,
        category: &[u8],
        stem: &str,
        extension: Option<&str>,
    ) -> Result<Option<Vec<u8>>> {
        let mut names = match self.media_category_candidates(category, stem)? {
            Some(names) => names,
            None => return Ok(None),
        };
        if let Some(extension) = extension {
            for name in &mut names {
                *name = replace_extension(self.text_encoding, name, extension)?;
            }
        }
        let resources = self.resources;
        for name in names {
            let Some(resource) = resources
                .resolve_mode_utf16(&name.encode_utf16().collect::<Vec<_>>(), ResourceMode::LooseFirst)
                .map_err(|cause| self.sound_host_error(cause))? else {
                continue;
            };
            let candidate = resource.value;
            if candidate.size > MAX_FILE {
                return Err(audio_script_error("sound sidecar exceeds 4 MiB"));
            }
            let bytes = resources.read_candidate(candidate)
                .map_err(|cause| self.sound_host_error(cause))?;
            return Ok((!bytes.is_empty()).then_some(bytes));
        }
        Ok(None)
    }

    /// 215 exact encoded tag lookup; logical=-1 searches physical 0..63.
    pub(super) fn sound_tag_value(&self, logical: u32, name: &[u8]) -> u32 {
        let name = trim_nul(name);
        if logical == u32::MAX {
            (0..64)
                .find_map(|channel| {
                    self.sound_channels
                        .get(&channel)
                        .and_then(|state| state.task.sidecars.tags.get(name))
                        .copied()
                })
                .unwrap_or(0)
        } else if logical < 64 {
            self.sound_channels
                .get(&self.sound_selected_channel(logical))
                .and_then(|state| state.task.sidecars.tags.get(name))
                .copied()
                .unwrap_or(0)
        } else {
            0
        }
    }

    /// 216 returns the first physical slot containing the exact tag.
    pub(super) fn sound_tag_channel(&self, name: &[u8]) -> u32 {
        let name = trim_nul(name);
        (0..64)
            .find(|channel| {
                self.sound_channels
                    .get(channel)
                    .is_some_and(|state| state.task.sidecars.tags.contains_key(name))
            })
            .unwrap_or(u32::MAX)
    }

    /// Safe equivalent for native consumers of 221/222. The opaque format is
    /// retained, not reinterpreted as a pointer or claimed to be decoded lips.
    pub(super) fn sound_lip_bytes(&self, logical: u32, legacy: bool) -> Option<Arc<[u8]>> {
        if logical >= 64 {
            return None;
        }
        let sidecars = &self
            .sound_channels
            .get(&self.sound_selected_channel(logical))?
            .task
            .sidecars;
        if legacy {
            sidecars.legacy_lip.clone()
        } else {
            sidecars.lip.clone()
        }
    }
}

fn without_extension(source: &[u8]) -> &[u8] {
    match source.iter().rposition(|&byte| matches!(byte, b'/' | b'\\' | b'.')) {
        Some(at) if source[at] == b'.' => &source[..at],
        _ => source,
    }
}

fn replace_extension(encoding: ScriptTextEncoding, source: &str, extension: &str) -> Result<String> {
    let encoded = encode_name(encoding, source)?;
    let mut name = without_extension(trim_nul(&encoded)).to_vec();
    name.push(b'.');
    name.extend_from_slice(extension.as_bytes());
    if name.len() >= 260 {
        return Err(audio_script_error("sound sidecar name exceeds 259 encoded bytes"));
    }
    decode_name(encoding, &name)
}

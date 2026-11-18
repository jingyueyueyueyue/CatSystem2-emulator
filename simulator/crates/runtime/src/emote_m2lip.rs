//! Independent M2 LPD sidecar reader and phoneme-level curve.
//! Header1ab250,record lookup1ab2d0,curve1a1b60,time sample1a1b00.
//! No external audio engine or original DLL is loaded.

use super::{EmoteError, Result};
use std::sync::Arc;

const MAX_RECORDS: usize = 1_000_000;
const MAX_FRAMES: usize = 4_000_000;

#[derive(Debug, Clone)]
pub struct CpuM2LpdRecord {
    pub frame: u32,
    pub phoneme: u32,
    pub words: [u32; 8],
}

#[derive(Debug, Clone)]
pub struct CpuM2Lpd {
    pub threshold: f32,
    pub records: Vec<CpuM2LpdRecord>,
}

#[derive(Debug, Clone)]
pub struct CpuM2LipCurve {
    values: Arc<[f32]>,
    pub origin_tick: u32,
    pub begin_value: f32,
    pub end_value: f32,
    scaled: bool,
    /// Parameters live on the retained b4 evaluator, independently of the
    /// currently selected SOUND sidecar type (184ee0/1754a1).
    preset: CpuM2LipPreset,
    // A failed destructive open must not expose the previous sound's values.
    // Keep its strict diagnostic until a later successful open replaces it.
    reopen_error: Option<EmoteError>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CpuM2LipSidecarKind {
    Lpd,
    LegacyLv,
}

/// Original1a19e0 default evaluator parameters. Overrides must come from the
/// actual #clipper/#clipper_default owner, not #lip_table waveform thresholds.
#[derive(Debug, Clone, PartialEq)]
pub struct CpuM2LipPreset {
    pub begin_value: f32,
    pub end_value: f32,
    pub phoneme_weights: [f32; 6],
    pub smoothing_radius: usize,
}

impl Default for CpuM2LipPreset {
    fn default() -> Self {
        Self {
            begin_value: 0.0,
            end_value: 5.0,
            phoneme_weights: [0.0, 1.0, 0.4, 0.2, 0.8, 0.6],
            smoothing_radius: 1,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CpuM2LipBinding {
    pub dimension: u32,
    pub pattern_cp932: Vec<u8>,
    pub preset: CpuM2LipPreset,
}

fn invalid() -> EmoteError {
    EmoteError::InvalidResource("invalid M2 LPD sidecar")
}
fn word(bytes: &[u8], offset: usize) -> Result<u32> {
    let chunk = bytes.get(offset..offset + 4).ok_or_else(invalid)?;
    Ok(u32::from_le_bytes(chunk.try_into().map_err(|_| invalid())?))
}
impl CpuM2Lpd {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < 20 || word(bytes, 0)? != 0x0044504c || word(bytes, 8)? != 32 {
            return Err(invalid());
        }
        let count = word(bytes, 12)? as usize;
        if count == 0 || count > MAX_RECORDS {
            return Err(invalid());
        }
        let end = count
            .checked_mul(32)
            .and_then(|n| n.checked_add(20))
            .filter(|end| *end <= bytes.len())
            .ok_or_else(invalid)?;
        let threshold = f32::from_bits(word(bytes, 16)?);
        if !threshold.is_finite() || threshold >= -20.0 {
            return Err(invalid());
        }
        let mut records = Vec::with_capacity(count);
        for row in bytes[20..end].chunks_exact(32) {
            let mut words = [0; 8];
            for (index, chunk) in row.chunks_exact(4).enumerate() {
                words[index] = u32::from_le_bytes(chunk.try_into().map_err(|_| invalid())?);
            }
            if words[1] > 5
                || !f32::from_bits(words[7]).is_finite()
                || records
                    .last()
                    .is_some_and(|last: &CpuM2LpdRecord| last.frame >= words[0])
            {
                return Err(invalid());
            }
            records.push(CpuM2LpdRecord {
                frame: words[0],
                phoneme: words[1],
                words,
            });
        }
        Ok(Self { threshold, records })
    }

    ///1ab2d0 begins at record1 and retains record0 as the predecessor.
    fn record_at(&self, frame: u32) -> Option<&CpuM2LpdRecord> {
        if self.records.len() <= 1 {
            return None;
        }
        //1ab30a returns the exact record on equality, including the last.
        //Only a time strictly after the last boundary returns null.
        if frame > self.records.last()?.frame {
            return None;
        }
        let index = self.records.partition_point(|record| record.frame <= frame);
        self.records.get(index.saturating_sub(1))
    }

    ///Build the exact bounded curve used by the original sidecar evaluator.
    ///Default smoothing radius is1; endpoints remain unsmoothed.
    pub fn curve(
        &self,
        origin_tick: u32,
        begin_value: f32,
        end_value: f32,
        phoneme_weights: [f32; 6],
        smoothing_radius: usize,
    ) -> Result<CpuM2LipCurve> {
        if ![begin_value, end_value]
            .iter()
            .chain(&phoneme_weights)
            .all(|v| v.is_finite())
            || smoothing_radius > 4096
        {
            return Err(invalid());
        }
        let count = (self.records.last().ok_or_else(invalid)?.frame as usize)
            .checked_add(1)
            .filter(|n| *n <= MAX_FRAMES)
            .ok_or(EmoteError::Capacity)?;
        let upper = -self.threshold;
        let denominator = upper - 20.0;
        let mut values = Vec::with_capacity(count);
        for frame in 0..count {
            let value = if let Some(record) = self.record_at(frame as u32) {
                let strength = (-f32::from_bits(record.words[7])).clamp(20.0, upper);
                let level = ((denominator - (strength - 20.0)) / denominator) * 0.4 + 0.6;
                level * phoneme_weights[record.phoneme as usize]
            } else {
                0.0
            };
            if !value.is_finite() {
                return Err(invalid());
            }
            values.push(value);
        }
        if smoothing_radius > 0 && smoothing_radius < count.saturating_sub(smoothing_radius) {
            let original = values.clone();
            let width = smoothing_radius * 2 + 1;
            for index in smoothing_radius..count - smoothing_radius {
                let mut sum = 0.0f32;
                for value in &original[index - smoothing_radius..=index + smoothing_radius] {
                    sum += *value;
                }
                values[index] = sum / width as f32;
            }
        }
        Ok(CpuM2LipCurve {
            values: values.into(),
            origin_tick,
            begin_value,
            end_value,
            scaled: true,
            preset: CpuM2LipPreset {
                begin_value,
                end_value,
                phoneme_weights,
                smoothing_radius,
            },
            reopen_error: None,
        })
    }
}
impl CpuM2LipCurve {
    ///1a1a96/1a1ad7 clear the old arrays without destroying the virtual
    ///receiver or its parameters. The LV receiver also frees arrays first.
    ///The import still returns the original diagnostic; this empty receiver
    ///is never a successful substitute for an unsupported resource.
    pub(super) fn failed_reopen(
        kind: CpuM2LipSidecarKind,
        preset: &CpuM2LipPreset,
        origin_tick: u32,
        error: EmoteError,
    ) -> Self {
        Self {
            values: Vec::<f32>::new().into(),
            origin_tick,
            begin_value: preset.begin_value,
            end_value: preset.end_value,
            scaled: kind == CpuM2LipSidecarKind::Lpd,
            preset: preset.clone(),
            reopen_error: Some(error),
        }
    }

    pub fn reopen_error(&self) -> Option<&EmoteError> {
        self.reopen_error.as_ref()
    }

    pub(super) fn sample_checked(&self, now: u32) -> Result<f32> {
        if let Some(error) = &self.reopen_error {
            return Err(error.clone());
        }
        Ok(self.sample(now))
    }

    pub(super) fn preset(&self) -> &CpuM2LipPreset {
        &self.preset
    }

    ///175400..175414 changes the retained evaluator's parameters after open.
    ///The LV virtual sampler1a1f00 does not consume the dirty2c flag: changing
    ///radius here must not smooth the already imported LV data a second time.
    pub(super) fn apply_legacy_parameters(&mut self, preset: &CpuM2LipPreset, origin: u32) {
        self.preset = preset.clone();
        self.begin_value = preset.begin_value;
        self.end_value = preset.end_value;
        self.origin_tick = origin;
    }

    pub(super) fn sidecar_kind(&self) -> CpuM2LipSidecarKind {
        if self.scaled {
            CpuM2LipSidecarKind::Lpd
        } else {
            CpuM2LipSidecarKind::LegacyLv
        }
    }

    ///1a1df0 copies a headerless f32 array, then smooths interior values.
    ///1a1f00 returns these already-scaled values directly. It does not apply
    ///the LPD begin/end range or phoneme weights a second time.
    pub fn from_legacy_lv(bytes: &[u8], origin_tick: u32, preset: &CpuM2LipPreset) -> Result<Self> {
        if bytes.is_empty()
            || bytes.len() % 4 != 0
            || bytes.len() / 4 > MAX_FRAMES
            || !preset.begin_value.is_finite()
            || preset.smoothing_radius > 4096
        {
            return Err(invalid());
        }
        let mut values = bytes
            .chunks_exact(4)
            .map(|chunk| {
                let value = f32::from_le_bytes(chunk.try_into().map_err(|_| invalid())?);
                value.is_finite().then_some(value).ok_or_else(invalid)
            })
            .collect::<Result<Vec<_>>>()?;
        let radius = preset.smoothing_radius;
        let count = values.len();
        if radius > 0 && radius < count.saturating_sub(radius) {
            let original = values.clone();
            let width = radius * 2 + 1;
            for index in radius..count - radius {
                let mut sum = 0.0f32;
                for value in &original[index - radius..=index + radius] {
                    sum += *value;
                }
                let value = sum / width as f32;
                if !value.is_finite() {
                    return Err(invalid());
                }
                values[index] = value;
            }
        }
        Ok(Self {
            values: values.into(),
            origin_tick,
            begin_value: preset.begin_value,
            end_value: preset.end_value,
            scaled: false,
            preset: preset.clone(),
            reopen_error: None,
        })
    }
    pub fn from_lpd(lpd: &CpuM2Lpd, origin_tick: u32, preset: &CpuM2LipPreset) -> Result<Self> {
        lpd.curve(
            origin_tick,
            preset.begin_value,
            preset.end_value,
            preset.phoneme_weights,
            preset.smoothing_radius,
        )
    }

    pub fn default_from_lpd(
        lpd: &CpuM2Lpd,
        origin_tick: u32,
        begin_value: f32,
        end_value: f32,
    ) -> Result<Self> {
        lpd.curve(
            origin_tick,
            begin_value,
            end_value,
            [0.0, 1.0, 0.4, 0.2, 0.8, 0.6],
            1,
        )
    }
    pub fn values(&self) -> &[f32] {
        &self.values
    }
    ///1a1b00 preserves begin outside the sidecar range and before origin.
    pub fn sample(&self, now: u32) -> f32 {
        if now < self.origin_tick {
            return self.begin_value;
        }
        let index = now.wrapping_sub(self.origin_tick) as usize;
        self.values.get(index).map_or(self.begin_value, |value| {
            if self.scaled {
                (self.end_value - self.begin_value) * value + self.begin_value
            } else {
                *value
            }
        })
    }
}

//! Independent #clipper_default/#clipper import from18bb10 and185fe0.
//!175270 applies these to every matching live lip record after LPD open.
//! No FreeMote/SDK implementation is included.

use super::{
    CpuM2Config, CpuM2LipBinding, CpuM2LipCurve, CpuM2LipPreset, CpuM2LipSidecarKind,
    CpuM2LiveOwner, CpuM2Lpd, EmoteError, Result,
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Default)]
pub(super) struct CpuM2ClipperProfile {
    default: Option<(CpuM2LipPreset, u32)>,
    overrides: BTreeMap<(u32, Vec<u8>), (CpuM2LipPreset, u32)>,
}

fn invalid(why: &'static str) -> EmoteError {
    EmoteError::InvalidResource(why)
}
fn word(bytes: &[u8]) -> Result<u32> {
    std::str::from_utf8(bytes)
        .ok()
        .and_then(|s| s.parse::<i32>().ok())
        .map(|value| value as u32)
        .ok_or_else(|| invalid("M2 clipper integer"))
}
fn preset(row: &[Vec<u8>]) -> Result<(CpuM2LipPreset, u32)> {
    if row.len() < 8 {
        return Err(invalid("M2 clipper preset record"));
    }
    let mut output = CpuM2LipPreset::default();
    for (value, token) in output.phoneme_weights.iter_mut().zip(row) {
        *value = std::str::from_utf8(token)
            .ok()
            .and_then(|s| s.parse::<f32>().ok())
            .filter(|v| v.is_finite())
            .ok_or_else(|| invalid("M2 clipper finite weight"))?;
    }
    let origin = word(&row[6])?;
    output.smoothing_radius = word(&row[7])? as usize;
    if output.smoothing_radius > 4096 {
        return Err(EmoteError::Capacity);
    }
    Ok((output, origin))
}

impl CpuM2ClipperProfile {
    pub fn from_expanded<'a>(configs: impl IntoIterator<Item = &'a CpuM2Config>) -> Result<Self> {
        let mut output = Self::default();
        for config in configs {
            for row in config.records(b"clipper_default") {
                //18bcbc copies32 bytes into the existing default record.
                output.default = Some(preset(row)?);
            }
            for row in config.records(b"clipper") {
                if row.len() < 10 {
                    return Err(invalid("M2 clipper dimension/pattern record"));
                }
                let dimension = word(&row[0])?
                    .checked_sub(1)
                    .filter(|n| *n < 4096)
                    .ok_or_else(|| invalid("M2 clipper dimension"))?;
                if row[1].is_empty() || row[1].len() > 4095 || row[1].contains(&0) {
                    return Err(invalid("M2 clipper CP932 pattern"));
                }
                //185f3e..185f43 does not overwrite an existing exact key.
                output
                    .overrides
                    .entry((dimension, row[1].clone()))
                    .or_insert(preset(&row[2..])?);
            }
        }
        Ok(output)
    }

    pub fn select(&self, dimension: u32, pattern: &[u8]) -> Result<(CpuM2LipPreset, u32)> {
        if let Some(value) = self.overrides.get(&(dimension, pattern.to_vec())) {
            return Ok(value.clone());
        }
        if let Some(value) = &self.default {
            return Ok(value.clone());
        }
        //186cf9..186d38 allocates40 bytes but initializes only map+20/+24.
        //The first32 bytes returned by185fe0 on a miss are uninitialized,
        //including when no clipper rows exist. Evaluator defaults are not
        //the profile default and cannot stand in for these indeterminate bytes.
        Err(EmoteError::Unsupported {
            native: 924,
            reason: "M2 clipper miss reads an undeclared, uninitialized profile default record",
        })
    }
}

/// Validate target identities before opening. After that point each virtual
///open is destructive, including a failed one (1a1a90/1a1df0). The caller
///must publish this owner on both Ok and Err, then return the diagnostic.
pub(super) fn bind_sidecar_records(
    owner: &mut CpuM2LiveOwner,
    bytes: &[u8],
    kind: CpuM2LipSidecarKind,
    records: &[(CpuM2LipBinding, u32)],
) -> Result<()> {
    if records.len() > 4096 {
        return Err(EmoteError::Capacity);
    }
    let mut lpd: Option<Result<CpuM2Lpd>> = None;
    let mut keys = BTreeSet::new();
    for (binding, _) in records {
        if !keys.insert((binding.dimension, binding.pattern_cp932.as_slice())) {
            return Err(invalid("duplicate M2 sidecar binding target"));
        }
        owner.lip_curve(binding.dimension, &binding.pattern_cp932)?;
    }
    let mut ordered: Vec<_> = records.iter().collect();
    ordered.sort_by(|(a, _), (b, _)| {
        (a.dimension, &a.pattern_cp932).cmp(&(b.dimension, &b.pattern_cp932))
    });
    let mut curves: Vec<(
        CpuM2LipSidecarKind,
        CpuM2LipPreset,
        CpuM2LipPreset,
        u32,
        CpuM2LipCurve,
    )> = Vec::new();
    let mut allocated = 0usize;
    let mut first_error = None;
    for (binding, origin) in ordered {
        //184ee0/1754a1 construct only a null b4. The existing virtual open
        //decides how bytes are read, even if the new SOUND reports another
        //sidecar type. Keep both its pre-open and post-open parameters.
        let previous = owner
            .lip_curve(binding.dimension, &binding.pattern_cp932)?
            .cloned();
        let decoder = previous
            .as_ref()
            .map(CpuM2LipCurve::sidecar_kind)
            .unwrap_or(kind);
        let initial_preset = CpuM2LipPreset::default();
        let open_preset = if decoder == CpuM2LipSidecarKind::LegacyLv {
            previous
                .as_ref()
                .map(CpuM2LipCurve::preset)
                .unwrap_or(&initial_preset)
        } else {
            &binding.preset
        };
        let result = (|| -> Result<CpuM2LipCurve> {
            if let Some((_, _, _, _, curve)) =
                curves.iter().find(|(class, before, after, at, _)| {
                    *class == decoder
                        && before == open_preset
                        && after == &binding.preset
                        && at == origin
                })
            {
                Ok(curve.clone())
            } else {
                if decoder == CpuM2LipSidecarKind::Lpd && lpd.is_none() {
                    //1a1a90 clears then calls the LPD reader. A class/data mismatch
                    //is diagnosed; never replace that receiver with a LV parser.
                    lpd = Some(CpuM2Lpd::parse(bytes));
                }
                let frames = if decoder == CpuM2LipSidecarKind::Lpd {
                    let lpd = lpd
                        .as_ref()
                        .ok_or_else(|| invalid("M2 retained LPD decoder"))?
                        .as_ref()
                        .map_err(Clone::clone)?;
                    (lpd.records
                        .last()
                        .ok_or_else(|| invalid("M2 LPD has no records"))?
                        .frame as usize)
                        .checked_add(1)
                        .ok_or(EmoteError::Capacity)?
                } else {
                    bytes.len() / 4
                };
                allocated = allocated
                    .checked_add(frames.checked_mul(4).ok_or(EmoteError::Capacity)?)
                    .filter(|n| *n <= 64 * 1024 * 1024)
                    .ok_or(EmoteError::Capacity)?;
                let curve = if decoder == CpuM2LipSidecarKind::Lpd {
                    let lpd = lpd
                        .as_ref()
                        .ok_or_else(|| invalid("M2 retained LPD decoder"))?
                        .as_ref()
                        .map_err(Clone::clone)?;
                    CpuM2LipCurve::from_lpd(lpd, *origin, &binding.preset)?
                } else {
                    let mut curve = CpuM2LipCurve::from_legacy_lv(bytes, *origin, open_preset)?;
                    curve.apply_legacy_parameters(&binding.preset, *origin);
                    curve
                };
                curves.push((
                    decoder,
                    open_preset.clone(),
                    binding.preset.clone(),
                    *origin,
                    curve.clone(),
                ));
                Ok(curve)
            }
        })();
        match result {
            Ok(curve) => owner.bind_lip_curve(binding.dimension, &binding.pattern_cp932, curve)?,
            Err(error) => {
                //184f1e branches around the flow reset, then continues the
                //map.1753e1 applies post-open parameters even on failure.
                owner.retain_failed_lip_open(
                    binding.dimension,
                    &binding.pattern_cp932,
                    CpuM2LipCurve::failed_reopen(decoder, &binding.preset, *origin, error.clone()),
                )?;
                first_error.get_or_insert(error);
            }
        }
    }
    match first_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

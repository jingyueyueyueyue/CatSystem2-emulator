//! Actual enabled PSB clampControl consumer (driver13530 ->1c930).
//! Applied after controller/timeline differences, before layer parameter cursors.

use super::{EmoteError, PsbValue, Result};
use std::collections::BTreeMap;

fn invalid(reason: &'static str) -> EmoteError {
    EmoteError::InvalidResource(reason)
}

fn number(value: &PsbValue, key: &str) -> Result<f32> {
    value.get(key).and_then(PsbValue::number).map(|n| n as f32)
        .filter(|n| n.is_finite()).ok_or_else(|| invalid("clampControl finite number"))
}

///1cd67..1cfb5. The original both-nonzero gate precedes the type branch.
///Type0 changes the radial mapping even inside the unit disk; it is not a
///rectangle clamp. Type1 uses a quantized atan2 angle only outside the disk.
fn pair(kind: i32, low: f32, high: f32, raw: [f32; 2]) -> Result<[f32; 2]> {
    let extent = high - low;
    if !extent.is_finite() || extent == 0.0 {
        return Err(invalid("clampControl nonzero finite range"));
    }
    let mut v = raw.map(|value| ((value - low) / extent) * 2.0 - 1.0);
    if !v.into_iter().all(f32::is_finite) {
        return Err(invalid("clampControl normalized range"));
    }
    if v[0] != 0.0 && v[1] != 0.0 {
        match kind {
            0 => {
                let mut slope = (v[0] / v[1]).abs();
                if slope > 1.0 { slope = 1.0 / slope; }
                let square = slope * slope + 1.0;
                let diagonal_scale = 1.0 / (f64::from(square).sqrt() as f32);
                let reduced = v.map(|value| value * diagonal_scale);
                let square = reduced[0] * reduced[0] + reduced[1] * reduced[1];
                let radius = f64::from(square).sqrt() as f32;
                let angle = (slope * std::f32::consts::PI) * 0.5;
                let weight = 1.0 - f64::from(angle).cos() as f32;
                let angle = (radius * std::f32::consts::PI) * 0.5;
                let sine = f64::from(angle).sin() as f32;
                let scale = weight * (sine / radius - 1.0) + 1.0;
                v = reduced.map(|value| value * scale);
            }
            1 => {
                let square = v[0] * v[0] + v[1] * v[1];
                let radius = f64::from(square).sqrt() as f32;
                if radius > 1.0 {
                    //1ce47 stores the original extended atan2 result to f32
                    //before the separate double cosine/sine calls.
                    let angle = f64::from(v[1]).atan2(f64::from(v[0])) as f32;
                    v = [f64::from(angle).cos() as f32, f64::from(angle).sin() as f32];
                }
            }
            _ => return Err(invalid("clampControl type outside recovered0/1")),
        }
    }
    let result = v.map(|value| ((value + 1.0) * 0.5) * extent + low);
    if !result.into_iter().all(f32::is_finite) {
        return Err(invalid("clampControl output outside finite domain"));
    }
    Ok(result)
}

pub(super) fn apply(root: &PsbValue, values: &mut BTreeMap<String, f64>) -> Result<()> {
    let Some(controls) = root.get("metadata").and_then(|m| m.get("clampControl")) else {
        return Ok(());
    };
    if matches!(controls, PsbValue::Null) { return Ok(()); }
    let controls = controls.list().filter(|v| v.len() <= 4096)
        .ok_or_else(|| invalid("clampControl bounded list"))?;
    //1c930 reads the independent controller54/difference64 banks. Writing
    //driver variables13710 does not feed a prior clamp into the next record.
    let mut writes = Vec::new();
    for control in controls {
        if control.get("enabled").and_then(PsbValue::number) == Some(0.0) { continue; }
        let name = |key| control.get(key).and_then(PsbValue::text)
            .filter(|s| !s.is_empty() && s.len() <= 4095)
            .ok_or_else(|| invalid("clampControl variable name"));
        let names = [name("var_lr")?, name("var_ud")?];
        let raw = names.map(|key| *values.get(key).unwrap_or(&0.0) as f32);
        let kind = number(control, "type")?;
        if !matches!(kind, 0.0 | 1.0) {
            return Err(invalid("clampControl type outside recovered0/1"));
        }
        let result = pair(kind as i32, number(control, "min")?, number(control, "max")?, raw)?;
        writes.push((names, result));
    }
    for (names, result) in writes {
        for (name, value) in names.into_iter().zip(result) {
            if let Some(existing) = values.get_mut(name) {
                *existing = f64::from(value);
            } else {
                values.insert(name.to_owned(), f64::from(value));
            }
        }
    }
    Ok(())
}

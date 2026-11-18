//! Actual controller receivers: eye6750,eyebrow91a0,mouth132e2,
//! selectorControl d260->d060. Graph/option queues live in m2timed; automatic
//! blink and mouth talk composition retain their separate strict diagnostics.

use super::{EmoteError, PsbValue, Result};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CpuEmoteImmediateTarget {
    Float,
    Selector,
    MouthSelector,
}

pub fn emote_immediate_target(root: &PsbValue, label: &str) -> Option<CpuEmoteImmediateTarget> {
    let metadata = root.get("metadata")?;
    for (section, kind) in [
        ("selectorControl", CpuEmoteImmediateTarget::Selector),
        ("eyeControl", CpuEmoteImmediateTarget::Float),
        ("eyebrowControl", CpuEmoteImmediateTarget::Float),
        ("mouthControl", CpuEmoteImmediateTarget::MouthSelector),
    ] {
        if metadata
            .get(section)
            .and_then(PsbValue::list)
            .unwrap_or(&[])
            .iter()
            .any(|control| {
                control.get("label").and_then(PsbValue::text) == Some(label)
                    && control.get("enabled").and_then(PsbValue::number) != Some(0.0)
                    && (section != "eyeControl"
                        || control
                            .get("blinkEnabled")
                            .and_then(PsbValue::number)
                            .is_none_or(|value| value == 0.0))
            })
        {
            return Some(kind);
        }
    }
    None
}

/// Original CVTTSS2SI including its signed out-of-range/NaN sentinel.
pub fn emote_control_integer(raw: f32) -> i32 {
    if !raw.is_finite() || raw < -2147483648.0 || raw >= 2147483648.0 {
        i32::MIN
    } else {
        raw.trunc() as i32
    }
}

///13300 stores the mouth label as an integer selector, independently of its
///talkLabel float/tween. Eye/eyebrow zero-duration writes retain their f32.
pub fn evaluate_emote_immediate_controls(
    root: &PsbValue,
    variables: &mut BTreeMap<String, f64>,
) -> Result<()> {
    apply_graph_defaults(root, variables)?;
    if let Some(controls) = root.get("metadata").and_then(|m| m.get("mouthControl")) {
        let controls = controls
            .list()
            .ok_or(EmoteError::InvalidResource("PSB mouthControl list"))?;
        for control in controls {
            if control.get("enabled").and_then(PsbValue::number) == Some(0.0) {
                continue;
            }
            let label = control
                .get("label")
                .and_then(PsbValue::text)
                .ok_or(EmoteError::InvalidResource("PSB mouthControl label"))?;
            if let Some(value) = variables.get_mut(label) {
                *value = f64::from(emote_control_integer(*value as f32));
            }
        }
    }
    evaluate_emote_immediate_selectors(root, variables)
}

///62e0/8e00 initialize current controller values from the signed beginFrame,
///narrowed to f32. Keep a live write (including an active graph sample) over
///that immutable default. This bank precedes timeline offsets and clampControl.
fn apply_graph_defaults(root: &PsbValue, variables: &mut BTreeMap<String, f64>) -> Result<()> {
    let Some(metadata) = root.get("metadata") else {
        return Ok(());
    };
    for section in ["eyeControl", "eyebrowControl"] {
        let Some(entries) = metadata.get(section) else {
            continue;
        };
        let entries = entries.list().ok_or(EmoteError::InvalidResource("graph controller list"))?;
        if entries.len() > 4096 {
            return Err(EmoteError::Capacity);
        }
        for entry in entries {
            if entry.get("enabled").and_then(PsbValue::number) == Some(0.0) {
                continue;
            }
            let label = entry.get("label").and_then(PsbValue::text)
                .filter(|name| !name.is_empty() && name.len() <= 4095)
                .ok_or(EmoteError::InvalidResource("graph controller label"))?;
            let begin = entry.get("beginFrame").and_then(PsbValue::number)
                .filter(|n| n.is_finite() && n.fract() == 0.0
                    && *n >= i32::MIN as f64 && *n <= i32::MAX as f64)
                .ok_or(EmoteError::InvalidResource("graph controller beginFrame"))?;
            variables.entry(label.to_owned()).or_insert(f64::from(begin as i32 as f32));
        }
    }
    Ok(())
}

///68d0 emits its graph value unchanged when blink is disabled and the declared
///begin/end interval is nondegenerate. Do not accept an automatic blink merely
///because the explicit graph target receiver is present.
pub(super) fn graph_render_diagnostics(root: &PsbValue) -> Result<Vec<String>> {
    let mut diagnostics = Vec::new();
    if let Some(entries) = root.get("metadata").and_then(|m| m.get("eyeControl")) {
        let entries = entries.list().ok_or(EmoteError::InvalidResource("eyeControl list"))?;
        if entries.len() > 4096 {
            return Err(EmoteError::Capacity);
        }
        for entry in entries {
            if entry.get("enabled").and_then(PsbValue::number) == Some(0.0) {
                continue;
            }
            let label = entry.get("label").and_then(PsbValue::text).unwrap_or("?");
            if entry.get("blinkEnabled").and_then(PsbValue::number) != Some(0.0) {
                diagnostics.push(format!("eyeControl {label}: automatic blink owner is not recovered"));
                continue;
            }
            let begin = entry.get("beginFrame").and_then(PsbValue::number);
            let end = entry.get("endFrame").and_then(PsbValue::number);
            if !matches!((begin, end), (Some(a), Some(b)) if b.is_finite()
                && b.fract() == 0.0 && b >= i32::MIN as f64 && b <= i32::MAX as f64
                && a < b)
            {
                diagnostics.push(format!("eyeControl {label}: degenerate/reversed blink interval is not recovered"));
            }
        }
    }
    Ok(diagnostics)
}

pub fn evaluate_emote_immediate_selectors(
    root: &PsbValue,
    variables: &mut BTreeMap<String, f64>,
) -> Result<()> {
    let Some(value) = root.get("metadata").and_then(|m| m.get("selectorControl")) else {
        return Ok(());
    };
    let controls = value
        .list()
        .ok_or(EmoteError::InvalidResource("PSB selectorControl list"))?;
    if controls.len() > 4096 {
        return Err(EmoteError::Capacity);
    }
    for control in controls {
        if control.get("enabled").and_then(PsbValue::number) == Some(0.0) {
            continue;
        }
        let label = control
            .get("label")
            .and_then(PsbValue::text)
            .ok_or(EmoteError::InvalidResource("PSB selectorControl label"))?;
        let raw = *variables.get(label).unwrap_or(&0.0) as f32;
        //d2af CVTTSS2SI retains the original signed out-of-range sentinel.
        let selected = emote_control_integer(raw);
        let options = control
            .get("optionList")
            .and_then(PsbValue::list)
            .ok_or(EmoteError::InvalidResource("PSB selector optionList"))?;
        if options.len() > 100_000 {
            return Err(EmoteError::Capacity);
        }
        for (index, option) in options.iter().enumerate() {
            let label = option
                .get("label")
                .and_then(PsbValue::text)
                .ok_or(EmoteError::InvalidResource("PSB selector option label"))?;
            let key = if selected == index as i32 {
                "onValue"
            } else {
                "offValue"
            };
            let value = option
                .get(key)
                .and_then(PsbValue::number)
                .map(|v| v as f32)
                .filter(|v| v.is_finite())
                .ok_or(EmoteError::InvalidResource("PSB selector option scalar"))?;
            variables.insert(label.to_owned(), f64::from(value));
        }
    }
    Ok(())
}

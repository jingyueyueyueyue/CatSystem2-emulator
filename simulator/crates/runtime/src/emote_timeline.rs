//! Independent frame sampler from the identified driver's original bytes.
//!
//! Driver SHA25685eb459e...903ce: parser36dd0 sets hidden/type0, held/type2
//! and interpolated/type3 flags. Evaluator3d680 guards against a hidden next
//! frame, computes a raw f32 ratio, optionally quantizes by content.ti, and
//! interpolates scalars/vectors/control points. Angle helper2b8f0 uses the
//! shortest turn. Nonlinear ccc/acc/zcc/scc/occ and cp remain diagnostic.

use super::{EmoteError, PsbValue, Result};
use std::collections::{BTreeMap, BTreeSet};

///One parser-created reference holder per physical layer/instance path.
///36321 creates its player once;41b83 releases the selected motion on hide.
///No texture, geometry or native pointer is retained by these clock records.
#[derive(Debug, Clone, Default)]
pub(super) struct CpuReferenceTimelines {
    root: Option<(String, String)>,
    owners: BTreeMap<Vec<usize>, ReferenceClock>,
    seen: BTreeSet<Vec<usize>>,
    updated_tick: Option<f32>,
    updated_variables: BTreeMap<String, f64>,
}

#[derive(Debug, Clone)]
struct ReferenceClock {
    selected: Option<(String, String)>,
    raw: f32,
    sample: f32,
    observed_tick: f32,
    playing: bool,
}

#[derive(Debug, Clone)]
pub(super) struct CpuReferenceSample {
    pub path: Vec<usize>,
    pub object: String,
    pub motion: String,
    pub frame: f64,
    pub raw_frame: f64,
}

impl CpuReferenceTimelines {
    pub fn begin(&mut self, object: &str, motion: &str) {
        let identity = (object.to_owned(), motion.to_owned());
        if self.root.as_ref() != Some(&identity) {
            self.owners.clear();
            self.root = Some(identity);
            self.updated_tick = None;
            self.updated_variables.clear();
        }
        self.seen.clear();
    }

    pub fn mark_tick(&mut self, tick: f32, variables: &BTreeMap<String, f64>) {
        self.updated_tick = Some(tick);
        self.updated_variables = variables.clone();
    }

    pub fn has_tick(&self, tick: f32, variables: &BTreeMap<String, f64>) -> bool {
        self.updated_tick == Some(tick) && self.updated_variables == *variables
    }

    ///Actual mask0/nonloop family in all135 fixed PSZs. Target parameters
    ///override delta. Ordinary references initialize once, then consume the
    ///player update delta, including when their caller uses a parameter clock.
    pub fn select(
        &mut self,
        path: Vec<usize>,
        object: &str,
        motion: &str,
        initial: f32,
        end: f32,
        tick: f32,
        parameter_frame: Option<f32>,
    ) -> Result<CpuReferenceSample> {
        if ![initial, end, tick].into_iter().all(f32::is_finite)
            || parameter_frame.is_some_and(|v| !v.is_finite()) || end < 0.0
        {
            return Err(invalid("reference clock outside recovered finite domain"));
        }
        let identity = (object.to_owned(), motion.to_owned());
        let update_delta = self.updated_tick.map(|before| tick - before).unwrap_or(0.0);
        if !update_delta.is_finite() {
            return Err(invalid("reference update delta overflow"));
        }
        let changed = self.owners.get(&path).and_then(|v| v.selected.as_ref()) != Some(&identity);
        if changed {
            //Replacing a motion destroys its physical layer subtree; sibling
            //holders and this reference's own parser-created holder survive.
            self.owners.retain(|key, _| key.len() <= path.len() || !key.starts_with(&path));
            let mut raw = initial.max(0.0);
            //3acc0..3acf8: a newly selected ordinary motion beginning at
            //zero starts at lastTime on a negative parent update. f9 then
            //prevents adding that first delta a second time (3ade2..3adea).
            if parameter_frame.is_none() && update_delta < 0.0 && raw == 0.0 {
                raw = end;
            }
            self.owners.insert(path.clone(), ReferenceClock {
                selected: Some(identity), raw, sample: raw.min(end),
                observed_tick: tick,
                playing: if update_delta < 0.0 { raw >= 0.0 } else { raw < end },
            });
        }
        self.seen.insert(path.clone());
        let owner = self.owners.get_mut(&path).unwrap();
        if let Some(frame) = parameter_frame {
            //3ac12/3ac1a and3ac38/3ac40 do not clamp to lastTime.
            owner.raw = frame;
            owner.sample = frame;
        } else if !changed {
            let delta = tick - owner.observed_tick;
            if !delta.is_finite() {
                return Err(invalid("reference clock delta overflow"));
            }
            if owner.playing {
                let raw = owner.raw + delta;
                if !raw.is_finite() {
                    return Err(invalid("reference clock overflow"));
                }
                if delta < 0.0 && raw < 0.0 {
                    //3af14..3af4f, actual loopTime=-1 family: only crossing
                    //below zero stops reverse playback. At exactly zero it
                    //remains active, unlike forward's >=lastTime stop.
                    owner.raw = 0.0;
                    owner.sample = 0.0;
                    owner.playing = false;
                } else {
                    owner.raw = raw;
                    owner.sample = raw.min(end);
                    if delta >= 0.0 {
                        owner.playing = raw < end;
                    }
                }
            }
        }
        owner.observed_tick = tick;
        Ok(CpuReferenceSample { path, object: object.to_owned(), motion: motion.to_owned(),
                                frame: f64::from(owner.sample), raw_frame: f64::from(owner.raw) })
    }

    pub fn finish(&mut self) {
        //A hidden physical layer has no visited reference;41b83 clears the
        //motion name/state but does not delete its holder. Drop its descendants
        //because they belonged to the released selected motion's layer tree.
        let hidden: Vec<_> = self.owners.keys().filter(|path| !self.seen.contains(*path)).cloned().collect();
        self.owners.retain(|path, _| !hidden.iter().any(|parent|
            path.len() > parent.len() && path.starts_with(parent)));
        for path in hidden {
            if let Some(owner) = self.owners.get_mut(&path) {
                owner.selected = None;
                owner.raw = 0.0;
                owner.sample = 0.0;
                owner.playing = false;
            }
        }
        self.seen.clear();
    }
}

#[derive(Debug, Clone)]
pub struct CpuTimelineSample {
    pub content: Option<PsbValue>,
    ///41057 subtracts the selected key's start, not its next boundary.
    pub frame_begin: Option<f32>,
    pub interpolated: bool,
    pub warnings: Vec<String>,
}

fn invalid(reason: &'static str) -> EmoteError {
    EmoteError::InvalidResource(reason)
}

fn number(value: &PsbValue, key: &str, default: f32) -> Result<f32> {
    match value.get(key) {
        None | Some(PsbValue::Null) => Ok(default),
        Some(value) => {
            let n = value
                .number()
                .ok_or_else(|| invalid("non-numeric frame field"))? as f32;
            if n.is_finite() {
                Ok(n)
            } else {
                Err(invalid("non-finite frame field"))
            }
        }
    }
}

fn vector(value: Option<&PsbValue>) -> Result<Vec<f32>> {
    let values: Vec<f32> = match value {
        None | Some(PsbValue::Null) => Vec::new(),
        Some(PsbValue::List(values)) => values
            .iter()
            .map(|v| {
                v.number()
                    .map(|n| n as f32)
                    .ok_or_else(|| invalid("non-numeric frame vector"))
            })
            .collect::<Result<_>>()?,
        Some(PsbValue::Integers(values)) => values.iter().map(|&n| n as f32).collect(),
        _ => return Err(invalid("frame vector type")),
    };
    if values.iter().any(|n| !n.is_finite()) {
        return Err(invalid("non-finite frame vector"));
    }
    Ok(values)
}

fn linear(a: f32, b: f32, t: f32) -> Result<f32> {
    let value = a * (1.0 - t) + b * t;
    if value.is_finite() {
        Ok(value)
    } else {
        Err(invalid("frame interpolation overflow"))
    }
}

/// Original2b8f0 adds/subtracts one360-degree turn and chooses a shortest arc
/// when the endpoint difference exceeds180. Equal endpoints remain untouched.
pub fn interpolate_emote_angle(a: f32, b: f32, t: f32) -> Result<f32> {
    if ![a, b, t].iter().all(|v| v.is_finite()) {
        return Err(invalid("non-finite frame angle"));
    }
    if a == b {
        return Ok(a);
    }
    let end = if b > a && b - a > 180.0 {
        b - 360.0
    } else if a > b && a - b > 180.0 {
        b + 360.0
    } else {
        b
    };
    let value = linear(a, end, t)?;
    Ok(if value < 0.0 {
        value + 360.0
    } else if value >= 360.0 {
        value - 360.0
    } else {
        value
    })
}

/// Original2b9a0: floor(256*t) fixed-point packed-channel interpolation.
/// All shifts and wrapping DWORD products follow the instruction sequence.
pub fn interpolate_emote_color(start: u32, end: u32, ratio: f32) -> Result<u32> {
    if !ratio.is_finite() || !(0.0..=1.0).contains(&ratio) {
        return Err(invalid("packed color interpolation ratio"));
    }
    if start == end {
        return Ok(start);
    }
    let weight = (ratio * 256.0).trunc() as u32;
    let inverse = 256 - weight;
    let odd_start = ((start >> 8) & 0xffff00ff).wrapping_mul(inverse);
    let odd_end = ((end >> 8) & 0xffff00ff).wrapping_mul(weight);
    let even = ((end & 0x00ff00ff)
        .wrapping_mul(weight)
        .wrapping_add((start & 0x00ff00ff).wrapping_mul(inverse)))
        >> 8;
    let shifted = ((start >> 8) & 0x00ff00ff)
        .wrapping_mul(inverse)
        .wrapping_add(((end >> 8) & 0x00ff00ff).wrapping_mul(weight));
    Ok(((even ^ odd_start.wrapping_add(odd_end)) & 0x00ff00ff) ^ shifted)
}

fn nonlinear(value: Option<&PsbValue>) -> bool {
    !matches!(value, None | Some(PsbValue::Null))
}

///37274 defaults to255;37569 narrows the parsed opacity to a byte.
///Keep the recovered resource domain explicit rather than clamping an
///unrecovered out-of-range field into apparently valid pixels.
pub(super) fn frame_opacity(content: &PsbValue) -> Result<u8> {
    let value = number(content, "opa", 255.0)?;
    if value.fract() != 0.0 || !(0.0..=255.0).contains(&value) {
        return Err(invalid("frame opacity outside recovered byte domain"));
    }
    Ok(value as u8)
}

/// Samples the actual frameList layout. It never interpolates into type0.
/// Source/icon/blend/flip remain from the preceding frame until the boundary.
/// Returned warnings flow into render_strict; they are never silently dropped.
pub fn sample_emote_layer(layer: &PsbValue, time: f64) -> Result<CpuTimelineSample> {
    let time = time as f32;
    if !time.is_finite() {
        return Err(invalid("non-finite frame time"));
    }
    let frames = layer
        .get("frameList")
        .and_then(PsbValue::list)
        .ok_or_else(|| invalid("frameList must be a list"))?;
    let mut previous = None;
    let mut next = None;
    let mut last = f32::NEG_INFINITY;
    for (index, frame) in frames.iter().enumerate() {
        let at = number(frame, "time", 0.0)?;
        if at < last {
            return Err(invalid("unsorted frameList"));
        }
        last = at;
        let kind = number(frame, "type", 2.0)?;
        if !matches!(kind, 0.0 | 2.0 | 3.0) {
            return Err(invalid("unsupported frameList interpolation type"));
        }
        //The last record is the next-boundary sentinel, never the current
        //sample.3bc40/3bc50 clamps random seek to count-2;3c845 and3bf27
        //stop forward object/parameter cursors there. Actual NEKO layers
        //end in type0: selecting it as current erased the model at its end.
        if frames.len() >= 2 && index == frames.len() - 1 {
            if next.is_none() {
                next = Some(frame);
            }
        } else if at <= time {
            previous = Some(frame);
        } else if next.is_none() {
            next = Some(frame);
        }
    }
    let mut sample = CpuTimelineSample {
        content: None,
        frame_begin: None,
        interpolated: false,
        warnings: Vec::new(),
    };
    let Some(a) = previous else {
        return Ok(sample);
    };
    sample.frame_begin = Some(number(a, "time", 0.0)?);
    if number(a, "type", 2.0)? == 0.0 {
        return Ok(sample);
    }
    let start = a
        .get("content")
        .ok_or_else(|| invalid("visible frame lacks content"))?;
    let mut map = start
        .object()
        .ok_or_else(|| invalid("frame content must be an object"))?
        .clone();
    sample.content = Some(start.clone());
    if number(a, "type", 2.0)? != 3.0 {
        return Ok(sample);
    }
    let Some(b) = next else {
        return Ok(sample);
    };
    if number(b, "type", 2.0)? == 0.0 {
        return Ok(sample);
    }
    let end = b
        .get("content")
        .ok_or_else(|| invalid("visible next frame lacks content"))?;
    let begin = number(a, "time", 0.0)?;
    let finish = number(b, "time", begin)?;
    if finish <= begin {
        return Err(invalid("empty interpolation interval"));
    }
    let mut elapsed = time - begin;
    let interval = number(start, "ti", 0.0)?;
    if interval < 0.0 || interval.fract() != 0.0 || interval > u32::MAX as f32 {
        return Err(invalid("invalid frame ti interval"));
    }
    if interval > 0.0 {
        elapsed = (elapsed / interval).trunc() * interval;
    }
    let ratio = (elapsed / (finish - begin)).clamp(0.0, 1.0);
    // At a keyframe, no interpolation/easing is needed and exact raw values win.
    if ratio == 0.0 {
        return Ok(sample);
    }
    for key in ["ccc", "acc", "zcc", "scc", "occ", "cp"] {
        if nonlinear(start.get(key)) {
            sample
                .warnings
                .push(format!("timeline curve/path {key} is not recovered"));
        }
    }
    // Actual PSB scalar zoom/shear fields are zx/zy/sx/sy, not a scale vector.
    for (key, default) in [
        ("zx", 1.0),
        ("zy", 1.0),
        ("sx", 0.0),
        ("sy", 0.0),
    ] {
        if start.get(key).is_some() || end.get(key).is_some() {
            let a = number(start, key, default)?;
            let b = number(end, key, default)?;
            map.insert(
                key.into(),
                PsbValue::Number(f64::from(linear(a, b, ratio)?)),
            );
        }
    }
    if start.get("opa").is_some() || end.get("opa").is_some() {
        //3d9d0..3da3d interpolates byte endpoints, then29550 rounds the
        //nonnegative value before storing DWORD284. Fractional opacity is
        //not carried through the physical hierarchy or into the shader.
        let opacity = linear(
            f32::from(frame_opacity(start)?),
            f32::from(frame_opacity(end)?),
            ratio,
        )?;
        map.insert("opa".into(), PsbValue::Integer((opacity + 0.5).floor() as i64));
    }
    if start.get("angle").is_some() || end.get("angle").is_some() {
        map.insert(
            "angle".into(),
            PsbValue::Number(f64::from(interpolate_emote_angle(
                number(start, "angle", 0.0)?,
                number(end, "angle", 0.0)?,
                ratio,
            )?)),
        );
    }
    if start.get("coord").is_some() || end.get("coord").is_some() {
        let a = vector(start.get("coord"))?;
        let b = vector(end.get("coord"))?;
        if (!a.is_empty() && a.len() != 3) || (!b.is_empty() && b.len() != 3) {
            return Err(invalid("frame coord must have three components"));
        }
        let values = (0..3)
            .map(|i| {
                linear(
                    a.get(i).copied().unwrap_or(0.0),
                    b.get(i).copied().unwrap_or(0.0),
                    ratio,
                )
                .map(|n| PsbValue::Number(f64::from(n)))
            })
            .collect::<Result<Vec<_>>>()?;
        map.insert("coord".into(), PsbValue::List(values));
    }
    //3db7d/3dba4 substitute global97df8 when exactly one BP endpoint is
    //empty. Initializer296d0 supplies16 row-major(x/3,y/3) pairs. An empty
    //endpoint is the identity patch, not a hold of the other endpoint.
    let am = start.get("mesh").filter(|v| !matches!(v, PsbValue::Null));
    let bm = end.get("mesh").filter(|v| !matches!(v, PsbValue::Null));
    if am.is_some() || bm.is_some() {
        if am.is_some_and(|m| m.object().is_none()) || bm.is_some_and(|m| m.object().is_none()) {
            return Err(invalid("frame mesh must be an object"));
        }
        let mut a = vector(am.and_then(|m| m.get("bp")))?;
        let mut b = vector(bm.and_then(|m| m.get("bp")))?;
        if !vector(am.and_then(|m| m.get("cc")))?.is_empty()
            || !vector(bm.and_then(|m| m.get("cc")))?.is_empty()
        {
            sample.warnings.push("timeline mesh BP interpolation curve cc is not recovered".into());
        }
        if !a.is_empty() || !b.is_empty() {
            let identity = || super::CpuBezierPatch::identity().points().iter()
                .flat_map(|point| point.iter().copied()).collect::<Vec<_>>();
            if a.is_empty() {
                a = identity();
            }
            if b.is_empty() {
                b = identity();
            }
            if a.len() == 32 && b.len() == 32 {
                let mut mesh = am.or(bm).and_then(PsbValue::object)
                    .ok_or_else(|| invalid("mesh object"))?.clone();
                let values = a
                    .into_iter()
                    .zip(b)
                    .map(|(a, b)| linear(a, b, ratio).map(|n| PsbValue::Number(f64::from(n))))
                    .collect::<Result<Vec<_>>>()?;
                mesh.insert("bp".into(), PsbValue::List(values));
                map.insert("mesh".into(), PsbValue::Object(mesh));
            } else {
                sample
                    .warnings
                    .push("timeline mesh endpoint shape is not recovered".into());
            }
        }
    }
    if start.get("color") != end.get("color") {
        if nonlinear(start.get("occ")) {
            sample
                .warnings
                .push("timeline packed color curve is not recovered".into());
        } else if let (Some(a), Some(b)) = (color_endpoint(start), color_endpoint(end)) {
            if ![a, b]
                .iter()
                .all(|&n| n.is_finite() && n.fract() == 0.0 && (0.0..=u32::MAX as f64).contains(&n))
            {
                return Err(invalid("packed color DWORD"));
            }
            map.insert(
                "color".into(),
                PsbValue::Integer(i64::from(interpolate_emote_color(
                    a as u32, b as u32, ratio,
                )?)),
            );
        } else {
            sample
                .warnings
                .push("timeline corner color endpoint layout is not recovered".into());
        }
    }
    sample.content = Some(PsbValue::Object(map));
    sample.interpolated = true;
    Ok(sample)
}

fn color_endpoint(content: &PsbValue) -> Option<f64> {
    match content.get("color") {
        //3724b..3725f fills all four corners before parsing an optional color.
        None | Some(PsbValue::Null) => Some(f64::from(0x808080ffu32)),
        Some(value) => value.number(),
    }
}

/// Mouth receiver output, not a guessed blend of the two parameters.
///196a2/19712 constructs the integer lane from beginFrame;bc84..bc99
///initializes talk to zero. bf10 emits selector and talk to separate outputs,
///and19c5b/19d3e/19e2c stores them under label and talkLabel respectively.
///The existing scalar queue supplies the live talk value. Apply these bases
///before timeline differences/clamp, as for the other controller outputs.
pub(super) fn apply_mouth_outputs(
    root: &PsbValue,
    variables: &mut std::collections::BTreeMap<String, f64>,
) -> Result<()> {
    let Some(controls) = root.get("metadata").and_then(|v| v.get("mouthControl")) else {
        return Ok(());
    };
    if matches!(controls, PsbValue::Null) {
        return Ok(());
    }
    let controls = controls.list().ok_or_else(|| invalid("mouthControl list"))?;
    if controls.len() > 4096 {
        return Err(EmoteError::Capacity);
    }
    for control in controls {
        if number(control, "enabled", 1.0)? == 0.0 {
            continue;
        }
        let name = |key| -> Result<&str> {
            control.get(key).and_then(PsbValue::text)
                .filter(|s| !s.is_empty() && s.len() <= 4095 && !s.contains('\0'))
                .ok_or_else(|| invalid("mouth controller output label"))
        };
        let label = name("label")?;
        let talk = name("talkLabel")?;
        let begin = control.get("beginFrame").and_then(PsbValue::number)
            .filter(|n| n.is_finite() && n.fract() == 0.0
                && *n >= i32::MIN as f64 && *n <= i32::MAX as f64)
            .ok_or_else(|| invalid("mouth controller beginFrame"))?;
        let selector = variables.get(label)
            .map(|value| super::emote_control_integer(*value as f32) as f32)
            .unwrap_or(begin as i32 as f32);
        variables.insert(label.to_owned(), f64::from(selector));
        let value = variables.get(talk).copied().unwrap_or(0.0) as f32;
        if !value.is_finite() {
            return Err(invalid("mouth talk output finite f32"));
        }
        variables.insert(talk.to_owned(), f64::from(value));
    }
    Ok(())
}

///19e80 creates d510(1) for each enabled transitionControl label. d5f1
///zeros its scalar storage;1335b uses da10 and1a263 uses dbb0, the same
///live scalar already owned by CpuM2TimedVariables. Publish its base before
///independent timeline offsets, without inventing an autonomous animation.
pub(super) fn apply_transition_outputs(
    root: &PsbValue,
    variables: &mut std::collections::BTreeMap<String, f64>,
) -> Result<()> {
    let Some(controls) = root.get("metadata").and_then(|v| v.get("transitionControl")) else {
        return Ok(());
    };
    if matches!(controls, PsbValue::Null) {
        return Ok(());
    }
    let controls = controls.list().ok_or_else(|| invalid("transitionControl list"))?;
    if controls.len() > 4096 {
        return Err(EmoteError::Capacity);
    }
    for control in controls {
        if number(control, "enabled", 1.0)? == 0.0 {
            continue;
        }
        let label = control.get("label").and_then(PsbValue::text)
            .filter(|s| !s.is_empty() && s.len() <= 4095 && !s.contains('\0'))
            .ok_or_else(|| invalid("transition controller label"))?;
        let value = variables.get(label).copied().unwrap_or(0.0) as f32;
        if !value.is_finite() {
            return Err(invalid("transition output finite f32"));
        }
        variables.insert(label.to_owned(), f64::from(value));
    }
    Ok(())
}

/// Affine part of original3d1b0: four transformOrder operations, in stored
/// order:0 mirror,1 rotate,2 zoom,3 shear. Translation comes from coord.
/// Column-vector output is[a,b,c,d,tx,ty], i.e. x'=a*x+c*y+tx.
pub fn emote_layer_affine(layer: &PsbValue, content: &PsbValue) -> Result<[f64; 6]> {
    let order = vector(layer.get("transformOrder"))?;
    if order.len() != 4
        || order
            .iter()
            .any(|&n| n.fract() != 0.0 || !(0.0..=3.0).contains(&n))
    {
        return Err(invalid(
            "transformOrder must contain four operation indices",
        ));
    }
    let mut a = 1.0f32;
    let mut b = 0.0f32;
    let mut c = 0.0f32;
    let mut d = 1.0f32;
    let truth = |key| -> Result<bool> {
        match content.get(key) {
            None | Some(PsbValue::Null) => Ok(false),
            Some(PsbValue::Bool(value)) => Ok(*value),
            _ => Ok(number(content, key, 0.0)? != 0.0),
        }
    };
    for op in order {
        match op as u8 {
            0 => {
                if truth("fx")? {
                    a = -a;
                    b = -b;
                }
                if truth("fy")? {
                    c = -c;
                    d = -d;
                }
            }
            1 => {
                let angle = number(content, "angle", 0.0)?;
                if angle != 0.0 {
                    // Original uses f32 PI, then calls double sin/cos and
                    // converts each result back to f32 before matrix products.
                    let radians = angle * std::f32::consts::PI * 2.0 / 360.0;
                    let sin = f64::from(radians).sin() as f32;
                    let cos = f64::from(radians).cos() as f32;
                    (a, b, c, d) = (
                        a * cos - c * sin,
                        b * cos - d * sin,
                        c * cos + a * sin,
                        d * cos + b * sin,
                    );
                }
            }
            2 => {
                let zx = number(content, "zx", 1.0)?;
                let zy = number(content, "zy", 1.0)?;
                a *= zx;
                b *= zx;
                c *= zy;
                d *= zy;
            }
            3 => {
                let sx = number(content, "sx", 0.0)?;
                let sy = number(content, "sy", 0.0)?;
                (a, b, c, d) = (a + c * sx, b + d * sx, c + a * sy, d + b * sy);
            }
            _ => unreachable!(),
        }
    }
    let coord = vector(content.get("coord"))?;
    if !coord.is_empty() && coord.len() != 3 {
        return Err(invalid("frame coord must have three components"));
    }
    let result = [
        a,
        b,
        c,
        d,
        coord.first().copied().unwrap_or(0.0),
        coord.get(1).copied().unwrap_or(0.0),
    ];
    if result.iter().any(|n| !n.is_finite()) {
        return Err(invalid("layer affine overflow"));
    }
    Ok(result.map(f64::from))
}

//! M2 #spline resource, curve builder1a2a50 and wrapper+fc owner185ba0.
//! These labels contribute to183580, separately from PSB timelineControl.

use std::{collections::BTreeMap, sync::Arc};

use super::{EmoteError, Result};

const MAX_POINTS: usize = 100_000;

fn invalid(why: &'static str) -> EmoteError { EmoteError::InvalidResource(why) }
fn finite(value: f32) -> Result<f32> {
    if value.is_finite() { Ok(value) } else { Err(invalid("M2 spline scalar overflow")) }
}
fn scalar(bytes: &[u8]) -> Result<f32> {
    finite(std::str::from_utf8(bytes).ok().and_then(|v| v.parse().ok())
        .ok_or_else(|| invalid("M2 spline scalar"))?)
}

/// Original20-byte entry: timestamp, value and cubic b/c/d coefficients.
#[derive(Debug, Clone)]
struct Point { time: f32, value: f32, b: f32, c: f32, d: f32 }

#[derive(Debug, Clone, Default)]
struct Label {
    looping: bool,
    tracks: BTreeMap<Vec<u8>, Vec<Point>>,
}

#[derive(Debug, Clone, Default)]
pub struct CpuM2SplineConfig { labels: BTreeMap<Vec<u8>, Label> }

impl CpuM2SplineConfig {
    ///1a1f60 keeps the first same-name label and first same-name track.
    ///Each value,duration token first advances the cumulative timestamp.
    pub fn append_cp932(&mut self, bytes: &[u8]) -> Result<()> {
        let cleaned = super::m2config::clean_cp932(bytes)?;
        let mut next = self.clone();
        let mut current = None;
        let mut total = next.labels.values().flat_map(|l| l.tracks.values())
            .map(Vec::len).sum::<usize>();
        for line in cleaned.split(|&b| b == b'\n') {
            let words: Vec<_> = line.split(u8::is_ascii_whitespace)
                .filter(|word| !word.is_empty()).collect();
            if words.is_empty() { continue; }
            if words.iter().any(|word| word.len() > 4095 || word.contains(&0)) {
                return Err(EmoteError::Capacity);
            }
            if words[0].starts_with(b"#") {
                let name = &words[0][1..];
                if name.is_empty() { return Err(invalid("M2 spline label")); }
                if !next.labels.contains_key(name) {
                    if words[1..].iter().any(|&word| word != b"loop") {
                        return Err(invalid("M2 spline header option"));
                    }
                    next.labels.insert(name.to_vec(), Label {
                        looping: words[1..].contains(&b"loop".as_slice()),
                        tracks: BTreeMap::new(),
                    });
                    current = Some(name.to_vec());
                }
                continue;
            }
            let Some(label) = current.as_ref().and_then(|name| next.labels.get_mut(name)) else {
                continue;
            };
            if label.tracks.contains_key(words[0]) { continue; }
            let (name, _, errors) = encoding_rs::SHIFT_JIS.decode(words[0]);
            if errors || name.is_empty() { return Err(invalid("M2 spline variable CP932")); }
            let mut points = Vec::new();
            let mut time = 0.0;
            for word in &words[1..] {
                let mut pair = word.split(|&b| b == b',');
                let value = scalar(pair.next().unwrap_or_default())?;
                let duration = scalar(pair.next().ok_or_else(|| invalid("M2 spline value,duration"))?)?;
                if pair.next().is_some() || duration <= 0.0 {
                    return Err(invalid("M2 spline point duration"));
                }
                time = finite(time + duration)?;
                points.push(Point { time, value, b: 0.0, c: 0.0, d: 0.0 });
            }
            if points.is_empty() { return Err(invalid("M2 spline empty curve")); }
            if label.looping {
                let original = points.len();
                for index in 0..original {
                    let mut point = points[index].clone();
                    point.time = finite(time + point.time)?;
                    points.push(point);
                }
            }
            total = total.checked_add(points.len()).filter(|&n| n <= MAX_POINTS)
                .ok_or(EmoteError::Capacity)?;
            build(&mut points)?;
            label.tracks.insert(words[0].to_vec(), points);
        }
        *self = next;
        Ok(())
    }
}

///1a2a50 solves the original tridiagonal system, including its endpoint
///conditions. A generic natural spline would change the first/last segment.
fn build(p: &mut [Point]) -> Result<()> {
    let n = p.len().saturating_sub(1);
    if n < 2 { return Ok(()); }
    p[0].d = finite(p[1].time - p[0].time)?;
    p[1].c = finite((p[1].value - p[0].value) / p[0].d)?;
    for i in 1..n {
        p[i].d = finite(p[i + 1].time - p[i].time)?;
        p[i].b = finite((p[i].d + p[i - 1].d) * 2.0)?;
        p[i + 1].c = finite((p[i + 1].value - p[i].value) / p[i].d)?;
        p[i].c = finite(p[i + 1].c - p[i].c)?;
    }
    p[0].b = -p[0].d;
    p[n].b = -p[n - 1].d;
    if n == 2 {
        p[0].c = 0.0;
        p[n].c = 0.0;
    } else {
        p[0].c = finite(p[2].c / (p[3].time - p[1].time)
            - p[1].c / (p[2].time - p[0].time))?;
        p[n].c = finite(p[n - 1].c / (p[n].time - p[n - 2].time)
            - p[n - 2].c / (p[n - 1].time - p[n - 3].time))?;
        //1a1f30 returns a rounded f32 pow result; the following x87
        //multiply/divide is rounded at its destination store.
        let first_square = (f64::from(p[0].d).powi(2)) as f32;
        let last_square = (f64::from(p[n - 1].d).powi(2)) as f32;
        p[0].c = finite((f64::from(first_square) * f64::from(p[0].c)
            / f64::from(p[3].time - p[1].time)) as f32)?;
        p[n].c = finite(-((f64::from(last_square) * f64::from(p[n].c)
            / f64::from(p[n].time - p[n - 3].time)) as f32))?;
    }
    for i in 1..=n {
        let ratio = finite(p[i - 1].d / p[i - 1].b)?;
        p[i].b = finite(p[i].b - p[i - 1].d * ratio)?;
        p[i].c = finite(p[i].c - p[i - 1].c * ratio)?;
    }
    p[n].c = finite(p[n].c / p[n].b)?;
    for i in (0..n).rev() {
        p[i].c = finite((p[i].c - p[i + 1].c * p[i].d) / p[i].b)?;
    }
    p[n].b = finite((p[n].value - p[n - 1].value) / p[n - 1].d
        + (p[n].c * 2.0 + p[n - 1].c) * p[n - 1].d)?;
    for i in 0..n {
        p[i].b = finite((p[i + 1].value - p[i].value) / p[i].d
            - (p[i].c * 2.0 + p[i + 1].c) * p[i].d)?;
        p[i].d = finite((p[i + 1].c - p[i].c) / p[i].d)?;
        p[i].c = finite(p[i].c * 3.0)?;
    }
    p[n].d = p[n - 1].d;
    Ok(())
}

fn sample(points: &[Point], looping: bool, elapsed: u32) -> Result<f32> {
    let last = points.last().ok_or_else(|| invalid("M2 spline missing points"))?;
    let mut time = elapsed as f32;
    let rounded_elapsed = time as u32;
    if looping && rounded_elapsed >= last.time as u32 {
        let period = points[(points.len() - 1) >> 1].time as u32;
        if period == 0 { return Err(invalid("M2 spline loop period")); }
        time = period.wrapping_add(rounded_elapsed % period) as f32;
    }
    let index = points.partition_point(|point| point.time <= time);
    if index == points.len() { time = last.time; }
    let point = &points[index.saturating_sub(1)];
    let delta = time - point.time;
    //1a29f7..1a2a38 computes b*t+a+c*t*t+d*t*t*t in this order.
    finite(((point.b * delta + point.value) + point.c * (delta * delta))
        + ((delta * delta) * delta) * point.d)
}

#[derive(Debug, Clone, Default)]
pub(super) struct CpuM2SplineOwner {
    pub config: Arc<CpuM2SplineConfig>,
    label: Vec<u8>,
    enabled: bool,
    first: bool,
    origin: u32,
    wrote: bool,
}
impl CpuM2SplineOwner {
    ///174d40 starts even an empty/unknown name; it does not create a curve.
    pub fn start(&mut self, label: &[u8]) {
        self.label = label.to_vec(); self.enabled = true;
        self.first = true; self.origin = 0; self.wrote = false;
    }
    ///174d90 clears only the name and enabled word, retaining prior counters.
    pub fn stop(&mut self) { self.label.clear(); self.enabled = false; }

    /// The empty wrapper already owns+fc before its first driver exists.
    /// Transfer only its playback fields after import binds the real config.
    /// Replaying just the last command would lose the counters retained by
    /// start-then-stop; copying the whole owner would overwrite that config.
    pub(super) fn inherit_playback_from(&mut self, previous: &Self) {
        self.label.clone_from(&previous.label);
        self.enabled = previous.enabled;
        self.first = previous.first;
        self.origin = previous.origin;
        self.wrote = previous.wrote;
    }

    pub fn advance(&mut self, tick: u32, materials: &BTreeMap<String, super::CpuM2Material>,
        force: &mut super::m2force::CpuM2Force) -> Result<()> {
        if !self.enabled { return Ok(()); }
        if self.first { self.origin = tick; self.first = false; }
        let mut wrote = false;
        if let Some(label) = self.config.labels.get(&self.label) {
            for (raw_name, points) in &label.tracks {
                let (name, _, errors) = encoding_rs::SHIFT_JIS.decode(raw_name);
                if errors { return Err(invalid("M2 spline output CP932")); }
                if let Some(material) = materials.get(name.as_ref()) {
                    //1944a0 returns node+44; receiver reads+18 = node+5c,
                    //the fixed output scale, rather than current maximum+b4.
                    let value = finite(material.output_scale * sample(points, label.looping, tick.wrapping_sub(self.origin))?)?;
                    force.add_offset(&name, value)?;
                    wrote = true;
                }
            }
        }
        self.wrote = wrote;
        Ok(())
    }
}

impl super::CpuM2LiveOwner {
    ///175900 rebuilds the driver at wrapper+4, not the spline owner at+fc.
    ///Keep its label, enable, first-update flag, origin and last-write state
    ///across that replacement. In particular, an already started curve must
    ///not restart from the new player's frame zero. Import subsequently
    ///binds the manager's actual spline config; advance uses the new live
    ///material table and force accumulator, with no old driver references.
    pub(in crate::emote) fn inherit_spline_owner(&mut self, previous: &Self) {
        self.spline = previous.spline.clone();
    }
}

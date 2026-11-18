//! Fixed Campanella 659d5f motion source (104/49aa30, 48ac70..48ae30).
//! Independent wire reader and CPU sampler. Channel numbers come from
//! 49a1b0/49a280/49a350, not from Collada names or an assumed FOV channel.
use std::collections::{BTreeMap, BTreeSet};
use super::{identity, word, Matrix};

#[derive(Debug, Default)]
pub(super) struct Clips(BTreeMap<Vec<u8>, Clip>);

#[derive(Debug)]
struct Clip {
    channels: BTreeMap<u32, Track>,
    first: f32,
    last: f32,
}

#[derive(Debug, Default)]
struct Track {
    // mode, time, incoming(time,value), value, outgoing(time,value).
    arrays: [Option<Vec<u32>>; 5],
}

fn records<'a>(bytes: &'a [u8], mut at: usize, end: usize)
    -> anyhow::Result<Vec<(&'a [u8], Option<usize>)>>
{
    let mut out = Vec::new();
    let mut visited = BTreeSet::new();
    loop {
        if !visited.insert(at) || visited.len() > 65536 || at.checked_add(12).is_none_or(|n| n > end) {
            anyhow::bail!("MDL motion record chain exceeds its bounds");
        }
        let size = word(bytes, at + 8)? as usize;
        let last = at.checked_add(12).and_then(|n| n.checked_add(size))
            .filter(|&n| n <= end).ok_or_else(|| anyhow::anyhow!("MDL motion payload exceeds chunk104"))?;
        let child = word(bytes, at + 4)? as usize;
        let child = if child == 0 { None } else {
            Some(at.checked_add(child).filter(|&n| n >= last && n < end)
                .ok_or_else(|| anyhow::anyhow!("MDL motion child overlaps its record or leaves chunk104"))?)
        };
        out.push((&bytes[at + 12..last], child));
        let next = word(bytes, at)? as usize;
        if next == 0 { break; }
        at = at.checked_add(next).filter(|&n| n >= last && n < end)
            .ok_or_else(|| anyhow::anyhow!("MDL motion sibling overlaps its record or leaves chunk104"))?;
    }
    Ok(out)
}

impl Clips {
    pub(super) fn read_chunk(&mut self, bytes: &[u8], start: usize, end: usize) -> anyhow::Result<()> {
        for (name, children) in records(bytes, start, end)? {
            if name.len() != 64 { anyhow::bail!("MDL motion name record requires64 bytes"); }
            let name = super::string(name)?;
            let mut channels = BTreeMap::new();
            if let Some(children) = children {
                for (channel, arrays) in records(bytes, children, end)? {
                    if channel.len() != 4 { anyhow::bail!("MDL motion channel requires one DWORD"); }
                    let mut track = Track::default();
                    if let Some(arrays) = arrays {
                        for (raw, child) in records(bytes, arrays, end)? {
                            if child.is_some() || raw.len() < 8 { anyhow::bail!("MDL motion array shape is invalid"); }
                            let count = word(raw, 0)? as usize;
                            let kind = word(raw, 4)? as usize;
                            if count > 1_000_000 || count.checked_mul(4).and_then(|n| n.checked_add(8)) != Some(raw.len()) {
                                anyhow::bail!("MDL motion array count exceeds its payload");
                            }
                            // 4a1ed0 skips unrecognized array types; retain the
                            // recognized wire words without imposing float rules on modes.
                            if kind < 5 {
                                let values = raw[8..].chunks_exact(4)
                                    .map(|b| u32::from_le_bytes(b.try_into().unwrap())).collect();
                                if track.arrays[kind].replace(values).is_some() { anyhow::bail!("MDL duplicate motion array"); }
                            }
                        }
                    }
                    if channels.insert(word(channel, 0)?, track).is_some() { anyhow::bail!("MDL duplicate motion channel"); }
                }
            }
            let mut first = f32::MAX;
            let mut last = 0f32;
            for track in channels.values() {
                if let Some(times) = &track.arrays[1] {
                    if let Some(&time) = times.first() { first = first.min(finite(time)?); }
                    if let Some(&time) = times.last() { last = last.max(finite(time)?); }
                }
            }
            if self.0.len() >= 65536 || self.0.insert(name, Clip { channels, first, last }).is_some() {
                anyhow::bail!("MDL duplicate/excessive named motion clips");
            }
        }
        Ok(())
    }
}

fn finite(bits: u32) -> anyhow::Result<f32> {
    let n = f32::from_bits(bits);
    if !n.is_finite() { anyhow::bail!("MDL motion contains a non-finite sample"); }
    Ok(n)
}

impl Track {
    fn values(&self, kind: usize) -> anyhow::Result<&[u32]> {
        self.arrays[kind].as_deref().ok_or_else(|| anyhow::anyhow!("MDL consumed motion channel lacks array{kind}"))
    }

    fn sample(&self, time: f32, dimension: usize) -> anyhow::Result<Vec<f32>> {
        let times = self.values(1)?;
        let values = self.values(3)?;
        if times.is_empty() || values.len() != times.len() * dimension {
            anyhow::bail!("MDL consumed motion channel has inconsistent time/value counts");
        }
        let times = times.iter().map(|&v| finite(v)).collect::<anyhow::Result<Vec<_>>>()?;
        if times.windows(2).any(|t| t[0] > t[1]) { anyhow::bail!("MDL motion times are unordered"); }
        let value = |key: usize, axis: usize| finite(values[key * dimension + axis]);
        // 4a2220/4a2650 multiply source+90 by this channel's last key time.
        let time = time * times[times.len() - 1];
        if !time.is_finite() { anyhow::bail!("MDL motion sample time is non-finite"); }
        let endpoint = if time < times[0] { Some(0) }
            else if time >= times[times.len() - 1] { Some(times.len() - 1) } else { None };
        if let Some(key) = endpoint { return (0..dimension).map(|axis| value(key, axis)).collect(); }
        // An exact interior key belongs to the preceding segment.
        let right = times.iter().position(|&t| t >= time).unwrap_or(times.len() - 1).max(1);
        let left = right - 1;
        if times[right] == times[left] { return (0..dimension).map(|axis| value(left, axis)).collect(); }
        let mode = *self.values(0)?.get(left)
            .ok_or_else(|| anyhow::anyhow!("MDL consumed motion segment lacks its interpolation mode"))?;
        let u = (time - times[left]) / (times[right] - times[left]);
        (0..dimension).map(|axis| {
            let a = value(left, axis)?;
            let b = value(right, axis)?;
            let result = match mode {
                1 => a + (b - a) * u,
                0 => {
                    let incoming = self.values(2)?;
                    let outgoing = self.values(4)?;
                    let handle = |words: &[u32], key: usize| -> anyhow::Result<f32> {
                        finite(*words.get((key * dimension + axis) * 2 + 1)
                            .ok_or_else(|| anyhow::anyhow!("MDL Bezier handle array is truncated"))?)
                    };
                    // 4a2bb0 uses handle VALUES and segment time u, without
                    // solving the handle-time curve. The matrices are fixed at5dc780.
                    let c = handle(outgoing, left)?;
                    let d = handle(incoming, right)?;
                    let v = 1. - u;
                    a * v * v * v + 3. * c * u * v * v + 3. * d * u * u * v + b * u * u * u
                }
                _ => anyhow::bail!("MDL consumed motion interpolation mode{mode} is not recovered"),
            };
            if !result.is_finite() { anyhow::bail!("MDL motion interpolation produced non-finite geometry"); }
            Ok(result)
        }).collect()
    }
}

#[derive(Clone, Debug, Default)]
pub(super) struct MotionSource {
    clip: Option<Vec<u8>>,
    source_enabled: bool,
    playing: u32,
    origin: u32,
    duration: f32,
    loops: i32,
    first: f32,
    last: f32,
    time: f32,
    // command362 writes node+208. Keep the exact word; no graph-render read
    // of it exists in48a7e0/48ad30/48d3b8 (not an invented looping flag).
    completion_word: u32,
}

impl MotionSource {
    pub(super) fn bind(&mut self, clips: &Clips, name: Option<&[u8]>) {
        if let Some(name) = name {
            // 48ce40 preserves the previous binding on lookup failure, but
            // 41fb8f still enables its source because the name pointer is nonnull.
            if let Some(clip) = clips.0.get(name) {
                self.clip = Some(name.to_vec()); self.first = clip.first; self.last = clip.last;
            }
        } else { self.clip = None; self.last = 0.; }
        self.source_enabled = name.is_some();
    }

    pub(super) fn play(&mut self, now: u32, words: [u32; 4]) -> anyhow::Result<()> {
        // 41fbe9 pushes record+4 first, then record+8. Thus48cea0
        // reads duration at stack+4 (record+8) and loops at stack+8
        // (record+4): enable, signed loops, unsigned duration, completion.
        if words[0] != 0 && words[2] == 0 { anyhow::bail!("MDL motion duration zero has no finite CPU sampling contract"); }
        self.completion_word = words[3];
        if words[0] != 0 { self.origin = now; self.duration = words[2] as f32; self.loops = words[1] as i32; }
        self.playing = words[0];
        Ok(())
    }

    pub(super) fn pending(&self) -> u32 { u32::from(self.time < self.last) }

    pub(super) fn end_word(&self) -> u32 {
        // 41fcb6..41fcde: truncate via signed64 x87 FISTP, return low DWORD.
        // Out-of-range produces integer-indefinite (low DWORD0), not a Rust
        // saturating float-to-integer cast. Parsed key times are finite.
        let value=f64::from(self.last);
        if value >= i64::MIN as f64 && value < -(i64::MIN as f64) {value as i64 as u32} else {0}
    }

    pub(super) fn advance(&mut self, now: u32) -> anyhow::Result<()> {
        let mut elapsed = now.wrapping_sub(self.origin) as f32;
        if self.loops > 0 {
            // Preserve each float subtraction from48ad30 rather than using
            // a quotient/modulo shortcut with different rounding.
            if self.duration <= 0. { anyhow::bail!("MDL positive loop count lacks a positive duration"); }
            let mut steps=0;
            while elapsed >= self.duration {
                if steps==65536 {anyhow::bail!("MDL motion cycle catch-up exceeds65536 exact subtractions");}
                steps+=1;
                self.loops-=1;
                if self.loops == 0 && self.playing != 0 { self.time = self.last; self.playing = 0; }
                elapsed-=self.duration;
                // PE's inner subtraction loop is governed by elapsed, not
                // by loops>0 after entry; preserve the signed count crossing0.
            }
        }
        if self.playing != 0 {
            // Preserve this PE's actual plus/min normalization, including
            // its asymmetry; do not replace it with a conventional lerp.
            let time = (elapsed / self.duration + self.first) / (self.last + self.first);
            if !time.is_finite() { anyhow::bail!("MDL motion clock normalization is non-finite"); }
            self.time = time;
        }
        Ok(())
    }

    pub(super) fn matrix(&self, clips: &Clips, original: Matrix, base: &[u32; 11]) -> anyhow::Result<Matrix> {
        let Some(transform)=self.transform(clips,base)? else {return Ok(original);};
        // Reuse the existing quaternion/matrix consumer; no second saved
        // transform bank, matrix decomposition or mutation of source geometry.
        Ok(cs2_runtime::model_state::ModelTransformOverrides { primary: Some(transform), secondary: None }
            .world_matrix(identity(), identity(), &Default::default())?)
    }

    pub(super) fn transform(&self, clips: &Clips, base: &[u32; 11]) -> anyhow::Result<Option<[u32; 11]>> {
        let Some(clip) = self.clip.as_ref().filter(|_| self.source_enabled).and_then(|name| clips.0.get(name)) else { return Ok(None); };
        let mut position = [finite(base[0])?, finite(base[1])?, finite(base[2])?];
        let mut scale = [finite(base[3])?, finite(base[4])?, finite(base[5])?];
        for (group, first, axes) in [(1, 2, &mut position), (9, 10, &mut scale)] {
            if let Some(track) = clip.channels.get(&group) {
                axes.copy_from_slice(&track.sample(self.time, 3)?);
            } else {
                for (axis, value) in axes.iter_mut().enumerate() {
                    if let Some(track) = clip.channels.get(&(first + axis as u32)) { *value = track.sample(self.time, 1)?[0]; }
                }
            }
        }
        // The real transform getter consumes rotation channel5. No sample
        // resource in this inventory uses it; never silently keep its base pose.
        if clip.channels.contains_key(&5) { anyhow::bail!("MDL motion rotation channel5 requires quaternion sampling"); }
        // Channel7 in both tower clips has no transform getter in this fixed
        // source vtable. Preserve its raw arrays; do not reinterpret it as FOV.
        for channel in clip.channels.keys() {
            if !matches!(*channel, 1..=4 | 7 | 9..=12) { anyhow::bail!("MDL motion channel{channel} has no recovered consumer"); }
        }
        let mut transform = *base;
        for i in 0..3 { transform[i] = position[i].to_bits(); transform[i + 3] = scale[i].to_bits(); }
        Ok(Some(transform))
    }
}

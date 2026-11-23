//! Independent metadata.timelineControl additive sampler.
//!
//! Driver1d720/1d8a0/1d9a0 owns the active label list,1f1c0/1f5c0
//! schedules scalar targets,1f950 wraps loops,20170 adds their actual values.
//! This is separate from object.motion/frameList sampling. Absolute targets
//! and timed instant variables need their actual controller receivers.
//! Recovered immediate eye/eyebrow/selector and integer mouth targets use
//! their real owner write; timed graph and selector option targets use the
//! shared driver bank, retaining strict errors for unrecovered conditions.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::Arc,
};

use super::{CpuEmoteControllerTarget, CpuEmoteImmediateTarget, EmoteError, PsbValue, Result};

const MAX_TRACKS: usize = 4096;
const MAX_FRAMES: usize = 100_000;
const MAX_ACTIVE: usize = 256;
const MAX_LOOP_CROSSINGS: usize = 65_536;

fn invalid(why: &'static str) -> EmoteError {
    EmoteError::InvalidResource(why)
}
fn number(value: &PsbValue, key: &str) -> Result<f32> {
    value
        .get(key)
        .and_then(PsbValue::number)
        .map(|n| n as f32)
        .filter(|n| n.is_finite())
        .ok_or_else(|| invalid("timelineControl finite numeric field"))
}
fn label(value: &PsbValue) -> Result<&str> {
    value
        .get("label")
        .and_then(PsbValue::text)
        .filter(|s| !s.is_empty() && s.len() <= 4095 && !s.contains('\0'))
        .ok_or_else(|| invalid("timelineControl label"))
}
fn entries(root: &PsbValue) -> Result<&[PsbValue]> {
    match root.get("metadata").and_then(|m| m.get("timelineControl")) {
        None | Some(PsbValue::Null) => Ok(&[]),
        Some(value) => value.list().ok_or_else(|| invalid("timelineControl list")),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CpuVariableTimelineInfo {
    pub label: String,
    /// Metadata classification only. Playback mixing is selected by flags&2.
    pub difference_label: bool,
    pub flags: u32,
    pub frame: f32,
    pub loop_begin: f32,
    pub loop_end: f32,
    pub last_time: f32,
    pub tracks: usize,
}

#[derive(Debug, Clone)]
struct Key {
    time: f32,
    target: Option<(f32, f32)>,
}
#[derive(Debug, Clone)]
struct Track {
    name: String,
    keys: Vec<Key>,
    immediate_target: Option<CpuEmoteImmediateTarget>,
}
#[derive(Debug, Clone)]
struct Definition {
    label: String,
    difference_label: bool,
    loop_begin: f32,
    loop_end: f32,
    last_time: f32,
    tracks: Vec<Track>,
    ///11ef0 collects definition labels even when an empty/skipped track
    ///does not need a running tween in the sampler.
    owner_names: BTreeSet<String>,
}
impl Definition {
    fn parse(root: &PsbValue, name: &str, flags: u32) -> Result<Self> {
        // Original flags2 is the PFL-selected additive lane. No guessed direct
        // variable/controller target is accepted for flags0/1.
        if flags & 2 == 0 || flags & !7 != 0 {
            return Err(EmoteError::Backend(
                "timelineControl requires recovered additive flags2 (optional bits1/4)".into(),
            ));
        }
        let mut found = entries(root)?
            .iter()
            .filter(|v| label(v).ok() == Some(name));
        let value = found
            .next()
            .ok_or_else(|| invalid("timelineControl label absent"))?;
        if found.next().is_some() {
            return Err(invalid("ambiguous timelineControl label"));
        }
        let instant: BTreeSet<_> = root
            .get("metadata")
            .and_then(|m| m.get("instantVariableList"))
            .and_then(PsbValue::list)
            .unwrap_or(&[])
            .iter()
            .map(|v| v.text().ok_or_else(|| invalid("instant variable name")))
            .collect::<Result<_>>()?;
        let variables = value
            .get("variableList")
            .and_then(PsbValue::list)
            .filter(|v| v.len() <= MAX_TRACKS)
            .ok_or_else(|| invalid("timelineControl variableList"))?;
        let mut tracks = Vec::new();
        let mut owner_names = BTreeSet::new();
        let mut names = BTreeSet::new();
        let mut maximum = 0.0f32;
        let mut total = 0usize;
        for variable in variables {
            let name = label(variable)?;
            owner_names.insert(name.to_owned());
            let frames = variable
                .get("frameList")
                .and_then(PsbValue::list)
                .ok_or_else(|| invalid("timelineControl frameList"))?;
            total = total
                .checked_add(frames.len())
                .filter(|n| *n <= MAX_FRAMES)
                .ok_or(EmoteError::Capacity)?;
            if frames.is_empty() {
                continue;
            }
            let is_instant = instant.contains(name);
            let immediate_target = if is_instant && flags & 4 == 0 {
                super::emote_immediate_target(root, name)
            } else {
                None
            };
            if is_instant && flags & 4 == 0 && immediate_target.is_none() {
                return Err(EmoteError::Backend(format!(
                    "timelineControl instant target {name} needs its actual controller receiver"
                )));
            }
            let mut keys = Vec::with_capacity(frames.len());
            for frame in frames {
                let time = number(frame, "time")?;
                if time < 0.0 || keys.last().is_some_and(|p: &Key| p.time >= time) {
                    return Err(invalid("unordered timelineControl times"));
                }
                maximum = maximum.max(time);
                let target = match number(frame, "type")? {
                    0.0 => None,
                    2.0 => {
                        let content = frame
                            .get("content")
                            .ok_or_else(|| invalid("timelineControl content"))?;
                        let value = number(content, "value")?;
                        let easing = number(content, "easing")?;
                        emote_variable_easing_exponent(easing)?;
                        Some((value, easing))
                    }
                    _ => return Err(invalid("unsupported timelineControl frame type")),
                };
                keys.push(Key { time, target });
            }
            if !is_instant || immediate_target.is_some() {
                if !names.insert(name.to_owned()) {
                    return Err(invalid("duplicate timelineControl track"));
                }
                // The scheduler always accesses current+1 when scheduling a
                // value. The actual models terminate with an inactive frame.
                if keys.last().is_some_and(|k| k.target.is_some()) {
                    return Err(invalid("timelineControl missing inactive terminal frame"));
                }
                tracks.push(Track {
                    name: name.to_owned(),
                    keys,
                    immediate_target,
                });
            }
        }
        let loop_begin = number(value, "loopBegin")?;
        let loop_end = number(value, "loopEnd")?;
        if loop_end >= 0.0 && (loop_begin < 0.0 || loop_end <= loop_begin) {
            return Err(invalid("timelineControl loop interval"));
        }
        let last_time = match number(value, "lastTime")? {
            n if n < 0.0 => maximum,
            n => n,
        };
        Ok(Self {
            label: name.to_owned(),
            difference_label: number(value, "diff")? != 0.0,
            loop_begin,
            loop_end,
            last_time,
            tracks,
            owner_names,
        })
    }
}

/// Driver131d5 maps the signed easing scalar into a positive power. The SSE2
/// helper6fa30 falls back at70566 to710c4, whose math-error label80f60 is "pow"
/// and whose7111a/6d090 path evaluates 2^(exponent*log2(progress)).
pub fn emote_variable_easing_exponent(easing: f32) -> Result<f32> {
    if !easing.is_finite() {
        return Err(invalid("timelineControl easing"));
    }
    let exponent = if easing == 0.0 {
        1.0
    } else if easing > 0.0 {
        easing + 1.0
    } else {
        1.0 / (1.0 - easing)
    };
    if !exponent.is_finite() || exponent <= 0.0 {
        return Err(invalid("timelineControl easing power overflow"));
    }
    Ok(exponent)
}

/// Scalar tween from driverda10/dbb0. Target durations are next_time-now-1,
/// not an interpolation between adjacent key values at their key timestamps.
#[derive(Debug, Clone, Default)]
struct ScalarTween {
    current: f32,
    from: f32,
    target: f32,
    progress: f32,
    rate: f32,
    exponent: f32,
    active: bool,
    queue: VecDeque<(f32, f32, f32)>,
}
impl ScalarTween {
    ///d920 commits the newest queued endpoint and clears active interpolation.
    fn finish_all(&mut self) {
        if let Some((value, _, _)) = self.queue.back() {
            self.current = *value;
        } else if self.active {
            self.current = self.target;
        }
        self.queue.clear();
        self.active = false;
    }

    fn target(&mut self, value: f32, duration: f32, easing: f32) -> Result<()> {
        if !duration.is_finite() || !value.is_finite() {
            return Err(invalid("timelineControl target overflow"));
        }
        if duration <= 0.0 {
            self.queue.clear();
            self.active = false;
            self.current = value;
        } else {
            // Driverda94 preserves an existing equal endpoint when its
            // remaining duration is shorter than this new requested one.
            if self.active
                && value.to_bits() == self.target.to_bits()
                && duration > (1.0 - self.progress) / self.rate
            {
                return Ok(());
            }
            self.queue.clear();
            self.active = false;
            self.queue
                .push_back((value, duration, emote_variable_easing_exponent(easing)?));
        }
        Ok(())
    }
    fn advance(&mut self, delta: f32) -> Result<()> {
        loop {
            if !self.active {
                let Some((value, duration, exponent)) = self.queue.pop_front() else {
                    return Ok(());
                };
                self.from = self.current;
                self.target = value;
                self.progress = 0.0;
                self.rate = 1.0 / duration;
                self.exponent = exponent;
                self.active = true;
            }
            self.progress += self.rate * delta;
            if !self.progress.is_finite() {
                return Err(invalid("timelineControl tween overflow"));
            }
            // Exact f32 constant at driver7e034 is -0.0001.
            if self.progress - 1.0 >= -0.0001 {
                self.progress = 1.0;
                self.current = self.target;
                self.active = false;
                continue;
            }
            // dbb0 widens both f32 inputs, evaluates pow(double,double),
            // narrows its output, then uses scalar-f32 endpoint interpolation.
            let ratio = f64::from(self.progress).powf(f64::from(self.exponent)) as f32;
            self.current = self.from + (self.target - self.from) * ratio;
            if !self.current.is_finite() {
                return Err(invalid("timelineControl variable overflow"));
            }
            return Ok(());
        }
    }
}

#[derive(Debug, Clone)]
struct Instance {
    definition: Arc<Definition>,
    flags: u32,
    frame: f32,
    cursors: Vec<usize>,
    tweens: Vec<ScalarTween>,
    immediate_writes: BTreeMap<String, f32>,
    controller_targets: Vec<CpuEmoteControllerTarget>,
}
impl Instance {
    fn new(definition: Definition, flags: u32) -> Result<Self> {
        let count = definition.tracks.len();
        let mut instance = Self {
            definition: Arc::new(definition),
            flags,
            frame: 0.0,
            cursors: vec![0; count],
            tweens: vec![ScalarTween::default(); count],
            immediate_writes: BTreeMap::new(),
            controller_targets: Vec::new(),
        };
        instance.seek(0.0)?;
        Ok(instance)
    }
    fn target(&mut self, track: usize, key: usize, now: f32) -> Result<()> {
        let keys = &self.definition.tracks[track].keys;
        if let Some((value, easing)) = keys[key].target {
            let next = keys
                .get(key + 1)
                .ok_or_else(|| invalid("timelineControl terminal target"))?;
            let duration = (next.time - now - 1.0).max(0.0);
            if let Some(kind) = self.definition.tracks[track].immediate_target {
                if matches!(
                    kind,
                    CpuEmoteImmediateTarget::Float | CpuEmoteImmediateTarget::Selector
                ) {
                    if self.controller_targets.len() >= MAX_FRAMES {
                        return Err(EmoteError::Capacity);
                    }
                    self.controller_targets.push(CpuEmoteControllerTarget {
                        name: self.definition.tracks[track].name.clone(),
                        value,
                        duration,
                        easing,
                    });
                    return Ok(());
                }
                let value = match kind {
                    CpuEmoteImmediateTarget::Float => value,
                    CpuEmoteImmediateTarget::Selector | CpuEmoteImmediateTarget::MouthSelector => {
                        super::emote_control_integer(value) as f32
                    }
                };
                self.immediate_writes
                    .insert(self.definition.tracks[track].name.clone(), value);
            } else {
                self.tweens[track].target(value, duration, easing)?;
            }
        }
        Ok(())
    }
    /// Driver1f1c0 recreates cursor positions at a seek/loop boundary without
    /// clearing the current tween values. Start itself resets them to zero.
    fn seek(&mut self, now: f32) -> Result<()> {
        for track in 0..self.definition.tracks.len() {
            let keys = &self.definition.tracks[track].keys;
            let mut cursor = keys.len().saturating_sub(1);
            let mut previous_active = None;
            for index in 0..keys.len().saturating_sub(1) {
                if keys[index].target.is_some() {
                    previous_active = Some(index);
                }
                if now >= keys[index].time && now < keys[index + 1].time {
                    cursor = index;
                    break;
                }
            }
            self.cursors[track] = cursor;
            if let Some(index) = previous_active {
                self.target(track, index, now)?;
            }
        }
        self.frame = now;
        Ok(())
    }
    fn schedule(&mut self, now: f32, include_boundary: bool) -> Result<()> {
        for track in 0..self.definition.tracks.len() {
            loop {
                let next = self.cursors[track] + 1;
                let Some(key) = self.definition.tracks[track].keys.get(next) else {
                    break;
                };
                let crossed = if include_boundary {
                    now >= key.time
                } else {
                    now > key.time
                };
                if !crossed {
                    break;
                }
                self.cursors[track] = next;
                self.target(track, next, now)?;
            }
        }
        self.frame = now;
        Ok(())
    }
    fn advance(&mut self, delta: f32) -> Result<bool> {
        let end = self.definition.loop_end;
        let mut remaining = delta;
        if end >= 0.0 {
            let mut crossings = 0usize;
            while self.frame + remaining >= end {
                if crossings >= MAX_LOOP_CROSSINGS {
                    return Err(EmoteError::Capacity);
                }
                crossings += 1;
                remaining -= end - self.frame;
                self.schedule(end, false)?;
                self.seek(self.definition.loop_begin)?;
            }
            remaining = remaining.max(0.0);
        }
        self.schedule(self.frame + remaining, true)?;
        for tween in &mut self.tweens {
            tween.advance(remaining)?;
        }
        Ok(end >= 0.0 || self.frame < self.definition.last_time)
    }
    fn info(&self) -> CpuVariableTimelineInfo {
        CpuVariableTimelineInfo {
            label: self.definition.label.clone(),
            difference_label: self.definition.difference_label,
            flags: self.flags,
            frame: self.frame,
            loop_begin: self.definition.loop_begin,
            loop_end: self.definition.loop_end,
            last_time: self.definition.last_time,
            tracks: self.definition.tracks.len(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct CpuVariableTimelines {
    active: Vec<Instance>,
    immediate_writes: BTreeMap<String, f32>,
    controller_targets: Vec<CpuEmoteControllerTarget>,
}
impl CpuVariableTimelines {
    ///1fbf0 submits nonloop timelines through lastTime and removes them.
    ///Looping labels remain selected, with their current scalar targets ended.
    pub(super) fn finish_all(&mut self) -> Result<()> {
        let mut next = self.clone();
        let mut retained = Vec::with_capacity(next.active.len());
        for mut instance in next.active.drain(..) {
            let looping = instance.definition.loop_end >= 0.0;
            if looping {
                for tween in &mut instance.tweens { tween.finish_all(); }
            } else {
                instance.schedule(instance.definition.last_time, true)?;
            }
            next.immediate_writes.extend(std::mem::take(&mut instance.immediate_writes));
            next.controller_targets.append(&mut instance.controller_targets);
            if next.controller_targets.len() > MAX_FRAMES { return Err(EmoteError::Capacity); }
            if looping { retained.push(instance); }
        }
        next.active = retained;
        *self = next;
        Ok(())
    }

    ///Driver11dc0 collects active timeline variable names before checking
    ///receiver timers. Looping timelines themselves do not keepf0 busy;
    ///non-looping timelines do until removed by their actual lastTime.
    pub(super) fn owner_busy_inputs(&self) -> (bool, BTreeSet<String>) {
        (
            self.active
                .iter()
                .any(|instance| instance.definition.loop_end < 0.0),
            self.active
                .iter()
                .flat_map(|instance| instance.definition.owner_names.iter().cloned())
                .collect(),
        )
    }
    pub(super) fn timed_controller_affects(&self, name: &str) -> bool {
        self.active.iter().any(|instance| {
            instance.definition.tracks.iter().any(|track| {
                track.name == name
                    && matches!(
                        track.immediate_target,
                        Some(CpuEmoteImmediateTarget::Float | CpuEmoteImmediateTarget::Selector)
                    )
                    && track.keys.windows(2).any(|keys| {
                        keys[0].target.is_some() && keys[1].time - keys[0].time - 1.0 > 0.0
                    })
            })
        })
    }

    pub fn labels(root: &PsbValue) -> Result<Vec<String>> {
        entries(root)?
            .iter()
            .map(|v| label(v).map(str::to_owned))
            .collect()
    }
    pub fn active(&self) -> Vec<CpuVariableTimelineInfo> {
        self.active.iter().map(Instance::info).collect()
    }
    pub(super) fn affects(&self, name: &str) -> bool {
        self.active.iter().any(|instance| {
            instance
                .definition
                .tracks
                .iter()
                .any(|track| track.name == name)
        })
    }
    pub fn is_playing(&self, name: &str) -> bool {
        if name.is_empty() {
            !self.active.is_empty()
        } else {
            self.active.iter().any(|t| t.definition.label == name)
        }
    }
    /// Bit1 preserves other active labels. Replaying a label restarts its
    /// scalar banks in place; flags2 alone stops other labels as the EXE does.
    pub fn play(&mut self, root: &PsbValue, name: &str, flags: u32) -> Result<()> {
        let mut instance = Instance::new(Definition::parse(root, name, flags)?, flags)?;
        if flags & 1 != 0
            && self.active.len() >= MAX_ACTIVE
            && !self.active.iter().any(|t| t.definition.label == name)
        {
            return Err(EmoteError::Capacity);
        }
        self.immediate_writes
            .extend(std::mem::take(&mut instance.immediate_writes));
        self.controller_targets
            .append(&mut instance.controller_targets);
        if flags & 1 == 0 {
            self.active.clear();
        }
        if let Some(current) = self.active.iter_mut().find(|t| t.definition.label == name) {
            *current = instance;
        } else {
            self.active.push(instance);
        }
        Ok(())
    }
    /// Empty label stops all; stopping leaves the owner's base variables intact.
    pub fn stop(&mut self, name: &str) {
        if name.is_empty() {
            self.active.clear();
        } else {
            self.active.retain(|t| t.definition.label != name);
        }
    }
    /// Actual immediate controller commands modify the owner store once.
    /// They persist after the timeline is stopped and are never added as diffs.
    pub(super) fn take_immediate_writes(&mut self) -> BTreeMap<String, f32> {
        std::mem::take(&mut self.immediate_writes)
    }
    pub(super) fn take_controller_targets(&mut self) -> Vec<CpuEmoteControllerTarget> {
        std::mem::take(&mut self.controller_targets)
    }
    /// Whole update commits atomically; no half-advanced model on a diagnostic.
    pub fn advance(&mut self, delta: f32) -> Result<()> {
        if !delta.is_finite() || delta < 0.0 {
            return Err(invalid("timelineControl nonnegative delta"));
        }
        if delta == 0.0 {
            return Ok(());
        }
        let mut next = self.clone();
        let mut retained = Vec::with_capacity(next.active.len());
        for mut instance in next.active.drain(..) {
            let retained_instance = instance.advance(delta)?;
            next.immediate_writes
                .extend(std::mem::take(&mut instance.immediate_writes));
            next.controller_targets
                .append(&mut instance.controller_targets);
            if next.controller_targets.len() > MAX_FRAMES {
                return Err(EmoteError::Capacity);
            }
            if retained_instance {
                retained.push(instance);
            }
        }
        self.active = retained;
        self.immediate_writes = next.immediate_writes;
        self.controller_targets = next.controller_targets;
        Ok(())
    }
    /// Actual additive values in active-label order, in f32 like20170/13530.
    /// Never accumulate last frame's difference into the next frame's baseline.
    pub fn evaluate(&self, base: &BTreeMap<String, f64>) -> Result<BTreeMap<String, f64>> {
        let mut output = base.clone();
        self.apply(&mut output)?;
        Ok(output)
    }

    pub(super) fn apply(&self, values: &mut BTreeMap<String, f64>) -> Result<()> {
        let mut sums = BTreeMap::<&str, f32>::new();
        for instance in &self.active {
            for (track, tween) in instance.definition.tracks.iter().zip(&instance.tweens) {
                if track.immediate_target.is_none() {
                    *sums.entry(&track.name).or_default() += tween.current;
                }
            }
        }
        for (name, offset) in sums {
            let value = *values.get(name).unwrap_or(&0.0) as f32 + offset;
            if !value.is_finite() {
                return Err(invalid("timelineControl additive overflow"));
            }
            if let Some(existing) = values.get_mut(name) {
                *existing = f64::from(value);
            } else {
                values.insert(name.to_owned(), f64::from(value));
            }
        }
        Ok(())
    }
}

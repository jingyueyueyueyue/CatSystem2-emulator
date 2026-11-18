//! Driver scalar da10/dbb0 and eye/eyebrow 9710/9da0 paths.
//! All arithmetic retains the original f32 state and double pow narrowing.
//! The graph planner retains original search and free node jumps. Automatic
//! blink remains a separate controller family. Selector integer choices own
//! scalar option tweens, not a tween of the integer selection itself.

use super::{
    CpuEmoteImmediateTarget, CpuM2ChainKind, EmoteError, PsbValue, Result, emote_control_integer,
    emote_immediate_target, emote_variable_easing_exponent,
};
use std::collections::{BTreeMap, BTreeSet};

#[path = "emote_m2mouth.rs"]
mod mouth;

#[derive(Debug, Clone, PartialEq)]
pub struct CpuM2TimedRequest {
    pub name: String,
    pub target: f32,
    pub frames: u32,
    pub kind: CpuM2ChainKind,
    ///1766b0 appends matching enabled entries in pattern/entry order.
    ///These are owned samples, not an inferred queue of timed targets.
    pub samples: Vec<f32>,
}

#[derive(Debug, Clone)]
pub struct CpuEmoteControllerTarget {
    pub name: String,
    pub value: f32,
    pub duration: f32,
    pub easing: f32,
}

impl CpuM2TimedRequest {
    pub fn driver_duration(&self, immediate: bool) -> Result<f32> {
        self.driver_duration_with_speed(immediate, 1.0)
    }

    pub(super) fn driver_duration_with_speed(&self, immediate: bool, speed: f32) -> Result<f32> {
        if self.samples.is_empty()
            || self.samples.len() > 100_000
            || !self.samples.iter().all(|value| value.is_finite())
        {
            return Err(invalid("M2 chain sample vector"));
        }
        if immediate || matches!(self.kind, CpuM2ChainKind::Fix | CpuM2ChainKind::Fade) {
            return Ok(0.0);
        }
        //16d42f/489/4e3 divide duration by record+5c, the actual speed.
        //16cf70 stores count-1 at+58, a distinct field;16f010 supplies
        //speed+5c from registered value+28 with default1. No inferred
        //equal-time segments or sample-count division reach the driver.
        let duration = self.frames as f32 / speed;
        if duration.is_nan() {
            return Err(unrecovered(
                "M2 zero-frame zero-speed chain produces NaN duration",
            ));
        }
        Ok(duration)
    }

    pub fn driver_easing(&self) -> Result<f32> {
        //16d411/46b/4c5 send these raw values to bridge2b10->13090.
        match self.kind {
            //Enum5(fade) jumps16d3ae with duration/easing still zero.
            //It retains its non-fix owner kind; it is not a new sine curve
            //or an inferred old-model surface blend.
            CpuM2ChainKind::Fix | CpuM2ChainKind::Fade => Ok(0.0),
            //16d7e8 enum1(linear) and enum4(sin_ss) both jump16d411.
            //Do not infer a sine curve from the configuration token.
            CpuM2ChainKind::Linear | CpuM2ChainKind::SinSs => Ok(1.0),
            CpuM2ChainKind::SinFs => Ok(-1.2),
            CpuM2ChainKind::SinSf => Ok(1.2),
            CpuM2ChainKind::NoMove => Err(invalid(
                "M2 NoMove selects owner banks without issuing a driver target",
            )),
        }
    }
}

fn invalid(why: &'static str) -> EmoteError {
    EmoteError::InvalidResource(why)
}
fn unrecovered(reason: &'static str) -> EmoteError {
    EmoteError::Unsupported {
        native: 906,
        reason,
    }
}

#[derive(Debug, Clone)]
enum State {
    MouthTalk(mouth::CpuMouthTalk),
    Scalar {
        from: f32,
        target: f32,
        value: f32,
        progress: f32,
        rate: f32,
        exponent: f32,
    },
    Graph {
        value: f32,
        target: f32,
        route: super::m2graph::Route,
        segment_target: Option<f32>,
        direction: f32,
        distance: f32,
        length: f32,
        rate: f32,
        exponent: f32,
    },
}

impl State {
    fn value(&self) -> f32 {
        match self {
            Self::MouthTalk(state) => state.value(),
            Self::Scalar { value, .. } | Self::Graph { value, .. } => *value,
        }
    }
    fn target(&self) -> f32 {
        match self {
            Self::MouthTalk(state) => state.target(),
            Self::Scalar { target, .. } | Self::Graph { target, .. } => *target,
        }
    }
    fn retain_equal_target(&self, target: f32, duration: f32) -> bool {
        let remaining = match self {
            Self::MouthTalk(state) => return state.retain_equal_target(target, duration),
            Self::Scalar { progress, rate, .. } => (1.0 - *progress) / *rate,
            Self::Graph {
                distance,
                length,
                exponent,
                rate,
                ..
            } => {
                if *length == 0.0 {
                    return self.target() == target && duration > 0.0;
                }
                let phase = f64::from(*distance / *length).powf(f64::from(1.0 / *exponent)) as f32;
                (1.0 - phase) / *rate
            }
        };
        self.target() == target && duration > remaining
    }
    fn advance(&mut self, delta: f32) -> Result<bool> {
        match self {
            Self::MouthTalk(state) => return state.advance(delta),
            Self::Scalar {
                from,
                target,
                value,
                progress,
                rate,
                exponent,
            } => {
                *progress += *rate * delta;
                if *progress - 1.0 >= -0.0001 {
                    *value = *target;
                    return Ok(true);
                }
                let ratio = f64::from(*progress).powf(f64::from(*exponent)) as f32;
                *value = *from + (*target - *from) * ratio;
            }
            Self::Graph {
                value,
                route,
                segment_target,
                direction,
                distance,
                length,
                rate,
                exponent,
                ..
            } => {
                loop {
                    //69e0/941e loads the next segment and skips zero-length
                    //pairs immediately. Its start can be a free node jump.
                    if segment_target.is_none() {
                        let Some(segment) = route.segments.pop_front() else {
                            return Ok(true);
                        };
                        *value = segment.from;
                        if segment.from == segment.to {
                            *value = segment.to;
                            continue;
                        }
                        *segment_target = Some(segment.to);
                        *direction = if segment.to >= segment.from {
                            1.0
                        } else {
                            -1.0
                        };
                    }
                    //68d0/9310: invert accumulated distance to the time phase,
                    //advance actual delta and map the phase back to distance.
                    let fraction = *distance / *length;
                    let root = f64::from(fraction).powf(f64::from(1.0 / *exponent)) as f32;
                    let phase = root + *rate * delta;
                    let ratio = f64::from(phase).powf(f64::from(*exponent)) as f32;
                    let travelled = ratio * *length;
                    let increment = travelled - *distance;
                    let prospective = *value + *direction * increment;
                    if !prospective.is_finite() || !travelled.is_finite() {
                        //Do not invent residual-time interpolation for the
                        //original 69c3/9401 correction when pow leaves its
                        //finite domain across a segment boundary.
                        return Err(unrecovered(
                            "M2 graph inter-segment distance/pow left the finite domain",
                        ));
                    }
                    *value = prospective;
                    let end = segment_target.unwrap();
                    let crossed = if *direction > 0.0 {
                        *value - end >= -0.0001
                    } else {
                        *value - end <= 0.0001
                    };
                    if !crossed {
                        *distance += increment;
                        break;
                    }
                    *value = end;
                    //69bb..69db and93f9..9419 use this f32 correction before
                    //processing the next queued pair with the same delta.
                    *distance += (end - prospective) * *direction;
                    *segment_target = None;
                }
            }
        }
        if !self.value().is_finite() {
            return Err(invalid("M2 timed driver value overflow"));
        }
        Ok(false)
    }
}

fn prepare(
    root: &PsbValue,
    name: &str,
    from: f32,
    target: f32,
    duration: f32,
    easing: f32,
) -> Result<Option<State>> {
    if ![from, target].iter().all(|v| v.is_finite()) || duration.is_nan() || duration < 0.0 {
        return Err(invalid("M2 timed request finite values"));
    }
    if duration == 0.0 {
        return Ok(None);
    }
    let exponent = emote_variable_easing_exponent(easing)?;
    if mouth::is_talk_label(root, name) {
        return Ok(Some(State::MouthTalk(mouth::CpuMouthTalk::pending(
            from, target, duration, exponent,
        ))));
    }
    match emote_immediate_target(root, name) {
        Some(CpuEmoteImmediateTarget::MouthSelector) => Ok(None),
        Some(CpuEmoteImmediateTarget::Selector) => Err(unrecovered(
            "M2 timed selector requires actual option target queues",
        )),
        Some(CpuEmoteImmediateTarget::Float) => {
            let metadata = root.get("metadata").ok_or_else(|| invalid("M2 metadata"))?;
            let control = ["eyeControl", "eyebrowControl"]
                .into_iter()
                .flat_map(|section| {
                    metadata
                        .get(section)
                        .and_then(PsbValue::list)
                        .unwrap_or(&[])
                })
                .find(|control| {
                    control.get("label").and_then(PsbValue::text) == Some(name)
                        && control.get("enabled").and_then(PsbValue::number) != Some(0.0)
                })
                .ok_or_else(|| invalid("M2 timed graph controller"))?;
            let route = super::m2graph::plan(control, from, target)?;
            Ok(Some(State::Graph {
                value: from,
                target,
                length: route.length,
                route,
                segment_target: None,
                direction: 0.0,
                distance: 0.0,
                rate: 1.0 / duration,
                exponent,
            }))
        }
        None => {
            if root
                .get("metadata")
                .and_then(|m| m.get("instantVariableList"))
                .and_then(PsbValue::list)
                .unwrap_or(&[])
                .iter()
                .any(|v| v.text() == Some(name))
            {
                return Err(unrecovered("unrecovered M2 instant controller target"));
            }
            Ok(Some(State::Scalar {
                from,
                target,
                value: from,
                progress: 0.0,
                rate: 1.0 / duration,
                exponent,
            }))
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct CpuM2TimedVariables {
    active: BTreeMap<String, State>,
    /// Selector outputs are scalar receiver values, including after their
    /// tween finishes. Keep these bases separate from instantaneous on/off
    /// evaluation, and apply timeline differences only after these outputs.
    selector_options: BTreeMap<String, Vec<String>>,
}

impl CpuM2TimedVariables {
    pub(super) fn has_active_except(&self, excluded: &BTreeSet<String>) -> bool {
        self.active.keys().any(|name| !excluded.contains(name))
    }
    pub fn active_count(&self) -> usize {
        self.active.len()
    }
    pub fn validate(
        root: &PsbValue,
        current: &BTreeMap<String, f64>,
        requests: &[CpuM2TimedRequest],
    ) -> Result<()> {
        let mut bank = Self::default();
        bank.submit(root, current, &mut current.clone(), requests, false)
    }

    /// Driver targets begin from actual current controller values, not a
    /// constructor seed. All requests validate before this transaction commits.
    pub fn submit(
        &mut self,
        root: &PsbValue,
        current: &BTreeMap<String, f64>,
        base: &mut BTreeMap<String, f64>,
        requests: &[CpuM2TimedRequest],
        immediate: bool,
    ) -> Result<()> {
        if requests.len() > 4096 {
            return Err(EmoteError::Capacity);
        }
        let targets = requests
            .iter()
            .map(|request| {
                Ok(CpuEmoteControllerTarget {
                    name: request.name.clone(),
                    value: request.target,
                    duration: request.driver_duration(immediate)?,
                    easing: request.driver_easing()?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        self.submit_targets(root, current, base, &targets)
    }

    pub fn submit_targets(
        &mut self,
        root: &PsbValue,
        current: &BTreeMap<String, f64>,
        base: &mut BTreeMap<String, f64>,
        requests: &[CpuEmoteControllerTarget],
    ) -> Result<()> {
        if requests.len() > 4096 {
            return Err(EmoteError::Capacity);
        }
        let mut next = self.clone();
        let mut writes = base.clone();
        for request in requests {
            if request.name.is_empty() || request.name.len() > 4095 || request.name.contains('\0') {
                return Err(invalid("M2 controller target name"));
            }
            if ![request.value, request.easing]
                .iter()
                .all(|v| v.is_finite())
                || request.duration.is_nan()
            {
                return Err(invalid("M2 controller target finite fields"));
            }
            if request.duration < 0.0 {
                return Err(invalid("M2 controller negative target duration"));
            }
            if emote_immediate_target(root, &request.name)
                == Some(CpuEmoteImmediateTarget::Selector)
            {
                next.submit_selector(root, current, &mut writes, request)?;
                continue;
            }
            let from = next
                .active
                .get(&request.name)
                .map(State::value)
                .unwrap_or_else(|| *current.get(&request.name).unwrap_or(&0.0) as f32);
            let duration = request.duration;
            let target = if matches!(
                emote_immediate_target(root, &request.name),
                Some(CpuEmoteImmediateTarget::MouthSelector | CpuEmoteImmediateTarget::Selector)
            ) {
                emote_control_integer(request.value) as f32
            } else {
                request.value
            };
            //da94..db08,eye67df..6848,eyebrow922c..928f retain the
            //existing equal endpoint if its remaining time is shorter.
            if duration > 0.0
                && next.active.get(&request.name).is_some_and(|state| {
                    state.retain_equal_target(target, duration)
                })
            {
                continue;
            }
            if let Some(state) =
                prepare(root, &request.name, from, target, duration, request.easing)?
            {
                next.active.insert(request.name.clone(), state);
                writes.insert(request.name.clone(), f64::from(from));
            } else {
                next.active.remove(&request.name);
                writes.insert(request.name.clone(), f64::from(target));
            }
        }
        *self = next;
        *base = writes;
        Ok(())
    }

    pub fn cancel(&mut self, name: &str) {
        self.active.remove(name);
        if let Some(options) = self.selector_options.remove(name) {
            for option in options {
                self.active.remove(&option);
            }
        }
    }

    pub fn apply_selector_outputs(
        &self,
        base: &BTreeMap<String, f64>,
        variables: &mut BTreeMap<String, f64>,
    ) {
        for option in self.selector_options.values().flatten() {
            if let Some(value) = base.get(option) {
                variables.insert(option.clone(), *value);
            }
        }
    }

    fn submit_selector(
        &mut self,
        root: &PsbValue,
        current: &BTreeMap<String, f64>,
        writes: &mut BTreeMap<String, f64>,
        request: &CpuEmoteControllerTarget,
    ) -> Result<()> {
        let control = root
            .get("metadata")
            .and_then(|v| v.get("selectorControl"))
            .and_then(PsbValue::list)
            .unwrap_or(&[])
            .iter()
            .find(|v| v.get("label").and_then(PsbValue::text) == Some(request.name.as_str()))
            .ok_or_else(|| invalid("M2 timed selector metadata"))?;
        let options = control
            .get("optionList")
            .and_then(PsbValue::list)
            .filter(|v| v.len() <= 100_000)
            .ok_or_else(|| invalid("M2 timed selector options"))?;
        let selected = emote_control_integer(request.value);
        let exponent = emote_variable_easing_exponent(request.easing)?;
        let mut names = Vec::new();
        for (index, option) in options.iter().enumerate() {
            let name = option
                .get("label")
                .and_then(PsbValue::text)
                .filter(|v| !v.is_empty() && v.len() <= 4095 && !v.contains('\0'))
                .ok_or_else(|| invalid("M2 timed selector option label"))?;
            //d060 only owns scalar option receivers. Do not recursively
            //guess a graph/selector receiver from an overlapping label.
            if emote_immediate_target(root, name).is_some() {
                return Err(unrecovered(
                    "M2 selector option overlaps a non-scalar controller",
                ));
            }
            let number = |key| {
                option
                    .get(key)
                    .and_then(PsbValue::number)
                    .map(|v| v as f32)
                    .filter(|v| v.is_finite())
                    .ok_or_else(|| invalid("M2 timed selector option values"))
            };
            let on = number("onValue")?;
            let off = number("offValue")?;
            let target = if selected == index as i32 { on } else { off };
            let managed = self
                .selector_options
                .values()
                .flatten()
                .any(|option| option == name);
            let from = self.active.get(name).map(State::value).unwrap_or_else(|| {
                *(if managed {
                    writes.get(name)
                } else {
                    current.get(name)
                })
                .unwrap_or(&0.0) as f32
            });
            if !from.is_finite() {
                return Err(invalid("M2 selector current option value"));
            }
            //d0dd queries the actual option at delta0. d0fd/d119 also keep
            //an existing scalar tween alive even when the target is equal.
            //7df78 is exactly f32 EPSILON, not the endpoint epsilon1e-4.
            if self.active.contains_key(name) || (from - target).abs() >= f32::EPSILON {
                //d101..d132 scales each option's duration by remaining
                //fraction of its own on/off range. The integer selector
                //itself is assigned immediately, never interpolated.
                let duration = if request.duration == 0.0 {
                    0.0
                } else {
                    ((from - target) / (on - off)).abs() * request.duration
                };
                if !duration.is_finite() {
                    return Err(unrecovered(
                        "M2 timed selector degenerate on/off range produces non-finite duration",
                    ));
                }
                let retain = duration > 0.0
                    && self.active.get(name).is_some_and(|old| {
                        old.retain_equal_target(target, duration)
                    });
                if !retain {
                    if duration > 0.0 && from != target {
                        self.active.insert(
                            name.to_owned(),
                            State::Scalar {
                                from,
                                target,
                                value: from,
                                progress: 0.0,
                                rate: 1.0 / duration,
                                exponent,
                            },
                        );
                        writes.insert(name.to_owned(), f64::from(from));
                    } else {
                        self.active.remove(name);
                        writes.insert(name.to_owned(), f64::from(target));
                    }
                }
            } else {
                //d130 skips a settled option without changing its receiver.
                writes.insert(name.to_owned(), f64::from(from));
            }
            names.push(name.to_owned());
        }
        self.active.remove(&request.name);
        self.selector_options.insert(request.name.clone(), names);
        writes.insert(request.name.clone(), f64::from(selected));
        if self.active.len() > 100_000 {
            return Err(EmoteError::Capacity);
        }
        Ok(())
    }
    pub fn finish_all(&mut self, base: &mut BTreeMap<String, f64>) {
        for (name, state) in std::mem::take(&mut self.active) {
            base.insert(name, f64::from(state.target()));
        }
    }
    pub fn advance(&mut self, delta: f32, base: &mut BTreeMap<String, f64>) -> Result<()> {
        if !delta.is_finite() || delta < 0.0 {
            return Err(invalid("M2 timed advance delta"));
        }
        let mut next = self.clone();
        let mut writes = base.clone();
        let mut finished = Vec::new();
        for (name, state) in &mut next.active {
            if state.advance(delta)? {
                finished.push(name.clone());
            }
            writes.insert(name.clone(), f64::from(state.value()));
        }
        for name in finished {
            next.active.remove(&name);
        }
        *self = next;
        *base = writes;
        Ok(())
    }
}

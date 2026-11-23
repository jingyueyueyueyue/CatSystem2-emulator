//! Exact atomic controls and worker-owned cursors; no clock reconstruction.
use super::*;
use cs2_runtime::graph_serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(crate) struct SignalState {
    enabled: bool, cached: u32, consumed: u64, completed: u32, period: u32,
    windows: Vec<u64>,
}
impl AudioSignal {
    pub(crate) fn checkpoint(&self) -> SignalState {
        SignalState { enabled: self.enabled.load(Ordering::Acquire),
            cached: self.cached_level.load(Ordering::Acquire),
            consumed: self.consumed_frames.load(Ordering::Acquire),
            completed: self.completed_passes.load(Ordering::Acquire),
            period: self.period.load(Ordering::Acquire),
            windows: self.windows.iter().map(|v| v.load(Ordering::Acquire)).collect() }
    }
}
impl SignalState {
    fn restore(&self) -> RuntimeResult<Arc<AudioSignal>> {
        if self.windows.len() != 80 || self.period == 0 || self.cached > u16::MAX as u32
            || self.consumed / u64::from(self.period) >= (1u64 << 48) - 1 {
            return Err(error("sound signal checkpoint is invalid"));
        }
        Ok(Arc::new(AudioSignal { enabled: AtomicBool::new(self.enabled),
            cached_level: AtomicU32::new(self.cached), consumed_frames: AtomicU64::new(self.consumed),
            completed_passes: AtomicU32::new(self.completed), period: AtomicU32::new(self.period),
            windows: std::array::from_fn(|i| AtomicU64::new(self.windows[i])) }))
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(crate) struct ParametersState { volume: u32, pan: u32, repeats: u64, signal: SignalState }
impl AudioParameters {
    pub(crate) fn checkpoint(&self) -> ParametersState {
        ParametersState { volume: self.volume.load(Ordering::Acquire), pan: self.pan.load(Ordering::Acquire),
            repeats: self.repeats.load(Ordering::Acquire), signal: self.signal.checkpoint() }
    }
}
impl ParametersState {
    pub(crate) fn validate_consumed(&self, consumed: u64) -> RuntimeResult<()> {
        if self.signal.consumed != consumed {
            return Err(error("SOUND signal/device consumed-frame identity differs"));
        }
        Ok(())
    }
    pub(crate) fn signal_matches(&self, signal: &AudioSignal) -> bool { self.signal == signal.checkpoint() }
    pub(crate) fn restore(&self) -> RuntimeResult<Arc<AudioParameters>> {
        let (volume, pan) = (f32::from_bits(self.volume), f32::from_bits(self.pan));
        if !volume.is_finite() || !(0.0..=1.0).contains(&volume)
            || !pan.is_finite() || !(-1.0..=1.0).contains(&pan) || self.repeats & REPEAT_CLOSED != 0 {
            return Err(error("active PCM checkpoint has invalid controls/repeat state"));
        }
        Ok(Arc::new(AudioParameters { volume: AtomicU32::new(self.volume), pan: AtomicU32::new(self.pan),
            repeats: AtomicU64::new(self.repeats), signal: self.signal.restore()? }))
    }
}
impl AudioParameters {
    pub(crate) fn signal_arc(&self) -> Arc<AudioSignal> { self.signal.clone() }
}

/// Held only by the prepared component. Poison/missing ownership is rejected
/// before commit; commit installs the real prepared parameter Arcs atomically.
pub(crate) struct RegistryLease {
    controls: std::sync::MutexGuard<'static, HashMap<u64, Weak<AudioParameters>>>,
    entries: Vec<(u64, Weak<AudioParameters>)>,
    remove: Vec<u64>,
    next: u64,
}
pub(crate) fn prepare_registry(
    entries: Vec<(u64, Arc<AudioParameters>)>, remove: Vec<u64>, retained: &std::collections::BTreeSet<u64>,
) -> RuntimeResult<RegistryLease> {
    let next = entries.iter().map(|(id, _)| *id).chain(retained.iter().copied()).max().unwrap_or(0)
        .checked_add(1).ok_or_else(|| error("saved audio handle space exhausted"))?;
    if entries.iter().any(|(id, _)| *id == 0) || retained.contains(&0) { return Err(error("zero saved audio handle")); }
    let controls = CONTROLS.get_or_init(|| Mutex::new(HashMap::new())).lock()
        .map_err(|_| error("desktop audio controls were poisoned"))?;
    if entries.iter().map(|(id, _)| id).chain(retained).any(|id| !remove.contains(id)
        && controls.get(id).and_then(Weak::upgrade).is_some()) {
        return Err(error("saved SOUND handle belongs to another live audio host"));
    }
    Ok(RegistryLease { controls, entries: entries.into_iter().map(|(id, p)| (id, Arc::downgrade(&p))).collect(), remove, next })
}
impl RegistryLease {
    pub(crate) fn commit(mut self) {
        for id in self.remove { self.controls.remove(&id); }
        for (id, parameters) in self.entries { self.controls.insert(id, parameters); }
        NEXT_HANDLE.fetch_max(self.next, Ordering::AcqRel);
    }
}

pub(crate) mod signal {
    use super::*;
    pub(crate) fn serialize<S: cs2_runtime::graph_serde::Serializer>(
        value: &Option<Arc<AudioSignal>>, s: S,
    ) -> Result<S::Ok, S::Error> { value.as_ref().map(|v| v.checkpoint()).serialize(s) }
    pub(crate) fn deserialize<'de, D: cs2_runtime::graph_serde::Deserializer<'de>>(
        d: D,
    ) -> Result<Option<Arc<AudioSignal>>, D::Error> {
        Option::<SignalState>::deserialize(d)?.map(|v| v.restore()).transpose()
            .map_err(cs2_runtime::graph_serde::de::Error::custom)
    }
}

impl PcmCursor {
    pub(crate) fn validate_checkpoint(&mut self, frames: usize, spec: AudioLoop) -> RuntimeResult<()> {
        let original = Self::new(frames, spec)?;
        // Earlier cursors retained the declared endpoint even past EOF.
        let legacy_end = usize::try_from(spec.range[1]).ok()
            .is_some_and(|end| end > frames && self.end == end && !self.valid);
        if self.frames != original.frames || self.begin != original.begin
            || self.whole != original.whole || self.position > frames
            || !legacy_end && (self.end != original.end || self.valid != original.valid)
        {
            return Err(error("PCM checkpoint source bounds/cursor differ"));
        }
        self.end = original.end;
        self.valid = original.valid;
        Ok(())
    }
}
impl SignalMeter {
    pub(crate) fn validate_checkpoint(&self, rate: u32, consumed: u64, queued: u64) -> RuntimeResult<()> {
        if self.rate != rate || self.window != (rate / 20).max(1) || self.count >= self.window
            || self.submitted_frames != consumed.checked_add(queued).ok_or_else(|| error("PCM timeline overflow"))?
            || self.window_start > self.submitted_frames || self.pass_ends.len() > 65_536
            || self.pass_ends.iter().any(|&end| end <= consumed || end > self.submitted_frames)
            || self.pass_ends.iter().zip(self.pass_ends.iter().skip(1)).any(|(a,b)| a > b) {
            return Err(error("PCM checkpoint device/signal timeline differs"));
        }
        Ok(())
    }
}
impl PlaybackRepeats {
    pub(crate) fn validate_checkpoint(&self, parameters: &AudioParameters) -> RuntimeResult<()> {
        // A concurrent repeat write can precede the worker's next refresh;
        // preserve both observed and pending policy rather than normalizing.
        if self.observed & REPEAT_CLOSED != 0 || self.exhausted && self.remaining != 1
            || parameters.repeat_state() & REPEAT_CLOSED != 0 {
            return Err(error("PCM checkpoint repeat cursor is invalid"));
        }
        Ok(())
    }
}

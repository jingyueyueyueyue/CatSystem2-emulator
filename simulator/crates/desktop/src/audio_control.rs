//! Live PCM parameters, addressed by globally unique desktop playback handles.
use crate::media_process::error;
use cs2_runtime::RuntimeResult;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};

#[path = "audio_control_state.rs"]
pub(crate) mod checkpoint;

static NEXT_HANDLE: AtomicU64 = AtomicU64::new(1);
static CONTROLS: OnceLock<Mutex<HashMap<u64, Weak<AudioParameters>>>> = OnceLock::new();
const REPEAT_CLOSED: u64 = 1 << 63;
const REPEAT_GENERATION: u64 = 0x7fff_ffff_0000_0000;

pub(crate) struct AudioParameters {
    volume: AtomicU32,
    pan: AtomicU32,
    repeats: AtomicU64,
    signal: Arc<AudioSignal>,
}

#[derive(Debug)]
pub(crate) struct AudioSignal {
    enabled: AtomicBool,
    cached_level: AtomicU32,
    consumed_frames: AtomicU64,
    completed_passes: AtomicU32,
    period: AtomicU32,
    windows: [AtomicU64; 80],
}

impl Default for AudioSignal {
    fn default() -> Self {
        Self {
            enabled: AtomicBool::new(false),
            cached_level: AtomicU32::new(0),
            consumed_frames: AtomicU64::new(0),
            completed_passes: AtomicU32::new(0),
            period: AtomicU32::new(1),
            windows: std::array::from_fn(|_| AtomicU64::new(0)),
        }
    }
}

impl PartialEq for AudioSignal {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self, other)
    }
}

impl AudioSignal {
    pub(crate) fn completed_passes(&self) -> u32 {
        self.completed_passes.load(Ordering::Acquire)
    }

    pub(crate) fn enable(&self, enabled: bool) {
        if !enabled && self.enabled.load(Ordering::Acquire) {
            self.cached_level
                .store(self.window(self.current_window()), Ordering::Release);
        }
        self.enabled.store(enabled, Ordering::Release);
    }
    pub(crate) fn level(&self) -> u32 {
        if !self.enabled.load(Ordering::Acquire) {
            return self.cached_level.load(Ordering::Acquire);
        }
        let value = self.window(self.current_window());
        self.cached_level.store(value, Ordering::Release);
        value
    }

    pub(crate) fn copy_window(&self, output: &mut [u32]) -> usize {
        let start = self.current_window();
        let count = output.len().min(80);
        for (index, value) in output[..count].iter_mut().enumerate() {
            *value = self.window(start.wrapping_add(index as u64));
        }
        count
    }

    fn current_window(&self) -> u64 {
        self.consumed_frames.load(Ordering::Acquire)
            / u64::from(self.period.load(Ordering::Acquire).max(1))
    }

    fn window(&self, index: u64) -> u32 {
        let slot = (index % self.windows.len() as u64) as usize;
        let packed = self.windows[slot].load(Ordering::Acquire);
        if packed >> 16 == index + 1 {
            (packed & 0xffff) as u32
        } else {
            0
        }
    }

    fn publish(&self, index: u64, value: u32) {
        // PCM16 adjacent differences fit16 bits. One atomic owns BOTH that
        // value and its window identity, so a recycled slot cannot tear.
        let packed = ((index + 1) << 16) | u64::from(value.min(u32::from(u16::MAX)));
        let slot = (index % self.windows.len() as u64) as usize;
        self.windows[slot].store(packed, Ordering::Release);
    }
}

impl AudioParameters {
    pub(crate) fn new(volume: f32, pan: f32) -> RuntimeResult<Arc<Self>> {
        validate(volume, pan)?;
        Ok(Arc::new(Self {
            volume: AtomicU32::new(volume.clamp(0., 1.).to_bits()),
            pan: AtomicU32::new(pan.clamp(-1., 1.).to_bits()),
            repeats: AtomicU64::new(1),
            signal: Arc::new(AudioSignal::default()),
        }))
    }
    pub(crate) fn values(&self) -> (f32, f32) {
        (
            f32::from_bits(self.volume.load(Ordering::Acquire)),
            f32::from_bits(self.pan.load(Ordering::Acquire)),
        )
    }
    pub(crate) fn repeat_state(&self) -> u64 {
        self.repeats.load(Ordering::Acquire)
    }
    pub(crate) fn reset_repeat_count(&self, count: u32) -> RuntimeResult<()> {
        self.repeats
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |old| {
                if old & REPEAT_CLOSED != 0 {
                    None
                } else {
                    Some((old.wrapping_add(1 << 32) & REPEAT_GENERATION) | u64::from(count))
                }
            })
            .map(|_| ())
            .map_err(|_| error("audio playback has completed; repeat update does not restart it"))
    }

    fn claim_completion(&self, observed: u64) -> bool {
        self.repeats
            .compare_exchange(
                observed,
                observed | REPEAT_CLOSED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }

    pub(crate) fn enable_signal(&self, enabled: bool) {
        self.signal.enable(enabled);
    }
}

pub(crate) fn allocate_handle() -> RuntimeResult<u64> {
    NEXT_HANDLE
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |handle| {
            handle.checked_add(1)
        })
        .map_err(|_| error("desktop audio handle space exhausted"))
}

pub(crate) fn register(handle: u64, parameters: &Arc<AudioParameters>) -> RuntimeResult<()> {
    let mut controls = CONTROLS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .map_err(|_| error("desktop audio controls were poisoned"))?;
    controls.retain(|_, parameters| parameters.strong_count() != 0);
    controls.insert(handle, Arc::downgrade(parameters));
    Ok(())
}

/// Changes the next PCM submissions without seeking, decoding or restarting
/// playback. Non-desktop hosts return an explicit unsupported-handle error.
pub fn set_playback_properties(handle: u64, volume: f32, pan: f32) -> RuntimeResult<()> {
    validate(volume, pan)?;
    let controls = CONTROLS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .map_err(|_| error("desktop audio controls were poisoned"))?;
    let parameters = controls
        .get(&handle)
        .and_then(Weak::upgrade)
        .ok_or_else(|| {
            error(format!(
                "audio handle {handle} has no live desktop PCM controls"
            ))
        })?;
    parameters
        .volume
        .store(volume.clamp(0., 1.).to_bits(), Ordering::Release);
    parameters
        .pan
        .store(pan.clamp(-1., 1.).to_bits(), Ordering::Release);
    Ok(())
}

/// Changes the remaining passes of the existing worker. Zero loops forever;
/// a nonzero count includes the current pass. This neither opens a device nor
/// seeks, flushes queued PCM, resumes a paused device, or starts idle playback.
pub fn set_playback_repeat_count(handle: u64, count: u32) -> RuntimeResult<()> {
    let controls = CONTROLS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .map_err(|_| error("desktop audio controls were poisoned"))?;
    let parameters = controls
        .get(&handle)
        .and_then(Weak::upgrade)
        .ok_or_else(|| {
            error(format!(
                "audio handle {handle} has no live desktop repeat control"
            ))
        })?;
    parameters.reset_repeat_count(count)
}

pub(crate) fn has_desktop_controls(handle: u64) -> bool {
    CONTROLS
        .get()
        .and_then(|controls| controls.lock().ok())
        .and_then(|controls| controls.get(&handle).and_then(Weak::upgrade))
        .is_some()
}

pub(crate) fn signal_monitor(handle: u64) -> RuntimeResult<Arc<AudioSignal>> {
    Ok(signal_parameters(handle)?.signal.clone())
}

#[derive(Clone, Copy, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(crate) struct AudioLoop {
    pub range: [i64; 2],
    pub whole: bool,
}

/// Source frame bounds for actual PCM submission. A repeat-policy update
/// changes the next endpoint without resetting the current source position.
#[derive(Clone, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(crate) struct PcmCursor {
    pub position: usize,
    frames: usize,
    begin: usize,
    end: usize,
    whole: bool,
    valid: bool,
}

impl PcmCursor {
    pub(crate) fn new(frames: usize, spec: AudioLoop) -> RuntimeResult<Self> {
        if frames == 0 {
            return Err(error("cannot play an empty PCM source"));
        }
        let [begin, end] = spec.range;
        let begin_value = usize::try_from(begin);
        let end = if end == -1 {
            frames
        } else {
            usize::try_from(end).map_or(usize::MAX, |end| end.min(frames))
        };
        let valid = begin_value
            .as_ref()
            .is_ok_and(|begin| *begin < frames && *begin < end)
            && end <= frames;
        Ok(Self {
            position: 0,
            frames,
            begin: begin_value.unwrap_or(0),
            end,
            whole: spec.whole,
            valid,
        })
    }
    pub(crate) fn end(&self, parameters: &AudioParameters) -> RuntimeResult<usize> {
        let end = if self.whole || parameters.repeat_state() as u32 == 1 {
            self.frames
        } else {
            if !self.valid {
                return Err(error("sound loop range cannot consume source frames"));
            }
            self.end
        };
        if self.position > end {
            return Err(error(
                "sound loop endpoint moved behind queued source position",
            ));
        }
        Ok(end)
    }
    pub(crate) fn rewind(&mut self) {
        self.position = if self.whole { 0 } else { self.begin };
    }
}

fn signal_parameters(handle: u64) -> RuntimeResult<Arc<AudioParameters>> {
    CONTROLS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .map_err(|_| error("desktop audio controls were poisoned"))?
        .get(&handle)
        .and_then(Weak::upgrade)
        .ok_or_else(|| {
            error(format!(
                "audio handle {handle} has no desktop signal controls"
            ))
        })
}

/// Adjacent-sample differences of the actual unscaled PCM submitted to the
/// device, before volume/pan. Publication is at most the bounded queue ahead.
#[derive(Clone, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(crate) struct SignalMeter {
    rate: u32,
    window: u32,
    submitted_frames: u64,
    window_start: u64,
    count: u32,
    total: u64,
    previous: Option<[i16; 2]>,
    pass_ends: VecDeque<u64>,
}

impl SignalMeter {
    pub(crate) fn new(rate: u32) -> Self {
        Self {
            rate,
            window: (rate / 20).max(1),
            submitted_frames: 0,
            window_start: 0,
            count: 0,
            total: 0,
            previous: None,
            pass_ends: VecDeque::new(),
        }
    }

    /// Register an actual whole-source endpoint in the device submission
    /// timeline. Prefetch alone never completes a pass.
    pub(crate) fn source_pass_end(&mut self, parameters: &AudioParameters) -> RuntimeResult<()> {
        if self.submitted_frames <= parameters.signal.consumed_frames.load(Ordering::Acquire) {
            parameters.signal.completed_passes.fetch_add(1, Ordering::AcqRel);
        } else {
            if self.pass_ends.len() >= 65_536 {
                return Err(error("PCM pending completion markers exceed65536"));
            }
            self.pass_ends.push_back(self.submitted_frames);
        }
        Ok(())
    }

    pub(crate) fn submitted(
        &mut self,
        samples: &[i16],
        channels: u8,
        parameters: &AudioParameters,
    ) {
        parameters
            .signal
            .period
            .store(self.window, Ordering::Release);
        if !parameters.signal.enabled.load(Ordering::Acquire) {
            self.count = 0;
            self.total = 0;
            self.previous = None;
            self.submitted_frames = self
                .submitted_frames
                .wrapping_add((samples.len() / usize::from(channels)) as u64);
            self.window_start = self.submitted_frames;
            return;
        }
        for frame in samples.chunks_exact(usize::from(channels)) {
            self.submitted_frames = self.submitted_frames.wrapping_add(1);
            let current = [frame[0], if channels == 2 { frame[1] } else { frame[0] }];
            if let Some(previous) = self.previous {
                let left = (i32::from(current[0]) - i32::from(previous[0])).unsigned_abs();
                let right = (i32::from(current[1]) - i32::from(previous[1])).unsigned_abs();
                self.total += u64::from((left + right) / 2);
                self.count += 1;
                if self.count == self.window {
                    parameters.signal.publish(
                        self.window_start / u64::from(self.window),
                        (self.total / u64::from(self.count)) as u32,
                    );
                    self.total = 0;
                    self.count = 0;
                    self.window_start = self.submitted_frames;
                }
            }
            self.previous = Some(current);
        }
        if self.count != 0 {
            // Short sounds and a partial final window already have real PCM.
            // Publish the available average now, instead of leaving their
            // whole audible lifetime at zero until device EOF.
            parameters.signal.publish(
                self.window_start / u64::from(self.window),
                (self.total / u64::from(self.count)) as u32,
            );
        }
    }

    pub(crate) fn finish(&mut self, parameters: &AudioParameters) {
        if self.count != 0 && parameters.signal.enabled.load(Ordering::Acquire) {
            parameters.signal.publish(
                self.window_start / u64::from(self.window),
                (self.total / u64::from(self.count)) as u32,
            );
        }
        // Called only after the physical device reports drained. This also
        // resolves a final sub-frame rounding loss in a seconds-based clock.
        let count = self.pass_ends.len() as u32;
        self.pass_ends.clear();
        parameters.signal.completed_passes.fetch_add(count, Ordering::AcqRel);
    }

    /// Device time, never the decoder's submitted/loop cursor. It freezes
    /// when output is paused and includes the real front-silence consumption.
    pub(crate) fn consumed(&mut self, seconds: f64, parameters: &AudioParameters) -> RuntimeResult<()> {
        if !seconds.is_finite() || seconds < 0. {
            return Err(error("PCM signal device clock is invalid"));
        }
        let frames = seconds * f64::from(self.rate);
        if frames / f64::from(self.window) >= ((1u64 << 48) - 1) as f64 {
            return Err(error("PCM signal device clock exceeds its frame range"));
        }
        let frames = frames.floor() as u64;
        self.consumed_frames(frames, parameters)
    }

    pub(crate) fn consumed_frames(&mut self, frames: u64, parameters: &AudioParameters) -> RuntimeResult<()> {
        if frames / u64::from(self.window) >= (1u64 << 48) - 1 {
            return Err(error("PCM exact device clock exceeds its signal range"));
        }
        parameters
            .signal
            .consumed_frames
            .store(frames, Ordering::Release);
        let mut count = 0u32;
        while self.pass_ends.front().is_some_and(|&end| end <= frames) {
            self.pass_ends.pop_front();
            count = count.wrapping_add(1);
        }
        parameters.signal.completed_passes.fetch_add(count, Ordering::AcqRel);
        Ok(())
    }
}

/// Repeat policy for one PCM worker. The sample cursor belongs to the worker,
/// so observing a new policy cannot reset the current sample position.
#[derive(Clone, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(crate) struct PlaybackRepeats {
    observed: u64,
    remaining: u32,
    exhausted: bool,
}

impl PlaybackRepeats {
    pub(crate) fn new(parameters: &AudioParameters) -> Self {
        let observed = parameters.repeat_state();
        Self {
            observed,
            remaining: observed as u32,
            exhausted: false,
        }
    }

    pub(crate) fn refresh(&mut self, parameters: &AudioParameters) -> bool {
        let observed = parameters.repeat_state();
        if observed == self.observed {
            return false;
        }
        self.observed = observed;
        self.remaining = observed as u32;
        self.exhausted = false;
        true
    }

    /// Called only after reaching the source boundary. True permits wrapping
    /// to the source start; false leaves the source cursor at its current end.
    pub(crate) fn repeat_at_end(&mut self) -> bool {
        if self.exhausted {
            return false;
        }
        if self.remaining == 0 {
            return true;
        }
        if self.remaining > 1 {
            self.remaining -= 1;
            return true;
        }
        self.exhausted = true;
        false
    }

    pub(crate) fn exhausted(&self) -> bool {
        self.exhausted
    }

    /// Completion and a new repeat count share one atomic ordering point.
    /// If an update wins, the worker must observe it before closing output.
    pub(crate) fn claim_completion(&self, parameters: &AudioParameters) -> bool {
        parameters.claim_completion(self.observed)
    }
}

fn validate(volume: f32, pan: f32) -> RuntimeResult<()> {
    if !volume.is_finite() || !pan.is_finite() {
        Err(error("audio volume and pan must be finite"))
    } else {
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(crate) struct AudioFade {
    pub current: f32,
    pub target: f32,
    pub remaining: i64,
}

impl AudioFade {
    /// Returns true while interpolation remains active. Each arithmetic step
    /// stays f32, and elapsed time is the task's wrapping DWORD tick delta.
    pub(crate) fn advance(&mut self, delta: u32) -> bool {
        self.remaining -= i64::from(delta);
        if self.remaining <= 0 {
            self.current = self.target;
            false
        } else {
            self.current += (self.target - self.current) / self.remaining as f32;
            true
        }
    }

    pub(crate) fn raw_value(&self) -> u32 {
        self.current.trunc() as i64 as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loop_endpoint_past_eof_preserves_the_loop_start() {
        let parameters = AudioParameters::new(1.0, 0.0).unwrap();
        parameters.reset_repeat_count(0).unwrap();
        let spec = AudioLoop { range: [43_392, 5_088_000], whole: false };
        let mut cursor = PcmCursor::new(4_895_100, spec).unwrap();
        assert_eq!(cursor.end(&parameters).unwrap(), 4_895_100);
        cursor.position = 4_895_100;
        cursor.rewind();
        assert_eq!(cursor.position, 43_392);

        // An older checkpoint can contain these bounds from one-shot playback.
        cursor.end = 5_088_000;
        cursor.valid = false;
        cursor.validate_checkpoint(4_895_100, spec).unwrap();
        assert_eq!(cursor.end(&parameters).unwrap(), 4_895_100);
    }

    #[test]
    fn loop_bounds_still_reject_empty_ranges_and_invalid_end_sentinels() {
        let parameters = AudioParameters::new(1.0, 0.0).unwrap();
        parameters.reset_repeat_count(0).unwrap();
        for range in [[-1, 120], [100, 120], [40, 39], [40, -2]] {
            let cursor = PcmCursor::new(100, AudioLoop { range, whole: false }).unwrap();
            assert!(cursor.end(&parameters).is_err(), "{range:?}");
        }
        let cursor = PcmCursor::new(100, AudioLoop { range: [40, 80], whole: false }).unwrap();
        assert_eq!(cursor.end(&parameters).unwrap(), 80);
        parameters.reset_repeat_count(1).unwrap();
        assert_eq!(cursor.end(&parameters).unwrap(), 100);
    }
}

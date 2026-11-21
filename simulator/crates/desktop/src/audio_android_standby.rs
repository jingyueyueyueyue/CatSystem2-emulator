//! Short-lived AAudio leases owned by their worker until handoff. For
//! drivers that stay STARTING, retain an idle opened device without silence.
use super::{
    AAudioStream_getFramesRead, AAudioStream_getFramesWritten, AAudioStream_getState,
    AAudioStream_getXRunCount, AAudioStream_setBufferSizeInFrames, OPEN, Output, STARTED, STARTING,
    StagedDevice,
};
use crate::movie_audio::MovieAudioOutput;
use std::sync::{
    Arc, Mutex, mpsc,
    atomic::{AtomicBool, AtomicU8, Ordering},
};
use std::time::{Duration, Instant};

const WARMING: u8 = 0;
const READY: u8 = 1;
const DONE: u8 = 2;
const CLAIM_TIMEOUT: Duration = Duration::from_millis(7);
const KEEP_ALIVE: Duration = Duration::from_secs(8);
const REFILL_INTERVAL: Duration = Duration::from_millis(5);
const STARTED_GRACE: Duration = Duration::from_millis(150);
const OPEN_POLL: Duration = Duration::from_millis(100);

// Ogg, WAV and replacement hosts can all own pools. A cancelled worker
// keeps its slot while the driver finishes opening or closing its stream.
const MAX_STANDBY_WORKERS: usize = 2;
static STANDBY_WORKERS: Mutex<Vec<std::thread::JoinHandle<()>>> = Mutex::new(Vec::new());

pub(crate) struct StandbyPool {
    devices: Vec<Standby>,
    legacy_formats: [Arc<AtomicBool>; 5],
}

impl Default for StandbyPool {
    fn default() -> Self {
        Self {
            devices: Vec::new(),
            legacy_formats: std::array::from_fn(|_| Arc::new(AtomicBool::new(false))),
        }
    }
}

impl StandbyPool {
    pub(crate) fn prepare(&mut self, rate: u32, channels: u8) {
        // The 44.1 kHz dialogue/stereo and mono effect formats commonly
        // follow one another. Retain at most one device of either layout.
        let formats: &[(u8, usize)] = match (rate, channels) {
            (44_100, 1) => &[(1, 0), (2, 1)],
            (44_100, 2) => &[(2, 1), (1, 0)],
            (48_000, 1) => &[(1, 2)],
            (48_000, 2) => &[(2, 3)],
            (22_050, 1) => &[(1, 4)],
            _ => return,
        };
        self.devices.retain(|standby| {
            !standby.done() && formats.iter().any(|&(channels, _)| {
                standby.matches_format(rate, channels)
            })
        });
        for &(channels, format) in formats {
            if let Some(standby) = self.devices.iter().find(|s| s.matches_format(rate, channels)) {
                standby.extend();
            } else {
                if let Some(standby) = Standby::spawn(rate, channels, self.legacy_formats[format].clone()) {
                    self.devices.push(standby);
                }
            }
        }
    }

    pub(crate) fn take(&mut self, rate: u32, channels: u8) -> Option<Standby> {
        self.devices.iter().position(|standby| standby.ready_for(rate, channels))
            .map(|index| self.devices.swap_remove(index))
    }
}

pub(crate) struct Standby {
    format: (u32, u8),
    flags: Arc<Flags>,
    claim: mpsc::Sender<mpsc::SyncSender<Option<Output>>>,
}

struct Flags {
    state: AtomicU8,
    extend: AtomicBool,
}

impl Standby {
    fn spawn(rate: u32, channels: u8, legacy: Arc<AtomicBool>) -> Option<Self> {
        let mut workers = STANDBY_WORKERS.lock().unwrap_or_else(|error| error.into_inner());
        workers.retain(|worker| !worker.is_finished());
        if workers.len() >= MAX_STANDBY_WORKERS {
            return None;
        }
        let (claim, commands) = mpsc::channel();
        let flags = Arc::new(Flags {
            state: AtomicU8::new(WARMING),
            extend: AtomicBool::new(false),
        });
        let worker_flags = flags.clone();
        let worker = std::thread::Builder::new()
            .name("cs2-aaudio-standby".into())
            .spawn(move || {
                let _ = run(rate, channels, &commands, &worker_flags, &legacy);
                worker_flags.state.store(DONE, Ordering::Release);
            })
            .ok()?;
        workers.push(worker);
        Some(Self { format: (rate, channels), flags, claim })
    }

    pub(crate) fn ready_for(&self, rate: u32, channels: u8) -> bool {
        self.format == (rate, channels) && self.flags.state.load(Ordering::Acquire) == READY
    }

    pub(crate) fn done(&self) -> bool {
        self.flags.state.load(Ordering::Acquire) == DONE
    }

    pub(crate) fn matches_format(&self, rate: u32, channels: u8) -> bool {
        self.format == (rate, channels)
    }

    pub(crate) fn extend(&self) {
        self.flags.extend.store(true, Ordering::Release);
    }

    pub(crate) fn claim(self) -> Option<Output> {
        if self.flags.state.load(Ordering::Acquire) != READY {
            return None;
        }
        let (reply, result) = mpsc::sync_channel(1);
        self.claim.send(reply).ok()?;
        result.recv_timeout(CLAIM_TIMEOUT).ok().flatten()
    }
}

fn run(
    rate: u32,
    channels: u8,
    commands: &ClaimReceiver,
    flags: &Flags,
    legacy: &AtomicBool,
) -> cs2_runtime::RuntimeResult<()> {
    if !legacy.load(Ordering::Acquire) {
        match run_silent(rate, channels, commands, flags) {
            Ok(WarmExit::Finished) => return Ok(()),
            Ok(WarmExit::OpenOnly) => legacy.store(true, Ordering::Release),
            Err(_) => {}
        }
    }
    run_opened(rate, channels, commands, flags)
}

type ClaimReceiver = mpsc::Receiver<mpsc::SyncSender<Option<Output>>>;

enum WarmExit {
    Finished,
    OpenOnly,
}

fn run_silent(
    rate: u32,
    channels: u8,
    commands: &ClaimReceiver,
    flags: &Flags,
) -> cs2_runtime::RuntimeResult<WarmExit> {
    let mut output = Output::open_pcm(rate, channels)?;
    let stream = output.validate_device()?;
    // A synchronous driver start can take most of the default buffer
    // duration. Use the known device capacity for the initial silence; the
    // running lease below queues only up to the bounded handoff window.
    let size = unsafe {
        AAudioStream_setBufferSizeInFrames(stream.as_ptr(), output.device_buffer_capacity)
    };
    if size > output.device_buffer_size && size <= output.device_buffer_capacity {
        output.device_buffer_size = size;
    }
    let step = u64::from(rate / 100);
    let silence = vec![0_i16; step as usize * usize::from(channels)];
    for _ in 0..8 {
        if !output.submit(&silence)? {
            break;
        }
    }
    if matches!(commands.try_recv(), Err(mpsc::TryRecvError::Disconnected)) {
        return Ok(WarmExit::Finished);
    }
    // A standby is real only after the normal AAudio start confirmation.
    output.pause(false)?;
    let started = Instant::now();
    let mut deadline = started + KEEP_ALIVE;
    let mut last_claimable = started;
    while Instant::now() < deadline {
        if flags.extend.swap(false, Ordering::AcqRel) {
            deadline = Instant::now() + KEEP_ALIVE;
        }
        output.pump()?;
        let Some((read, _, _, _)) = output.standby_counters() else {
            return Ok(WarmExit::OpenOnly);
        };
        if output.accepted_frames.saturating_sub(read) < 3 * step {
            output.submit(&silence)?;
        }
        let can_claim = output.promotion_origin(2 * step).is_some();
        if can_claim {
            last_claimable = Instant::now();
        } else if last_claimable.elapsed() >= STARTED_GRACE {
            // Some routes report STARTED without usable device progress.
            // Keep an opened lease and confirm the start when PLAY arrives.
            return Ok(WarmExit::OpenOnly);
        }
        flags.state.store(if can_claim { READY } else { WARMING }, Ordering::Release);
        match commands.recv_timeout(REFILL_INTERVAL) {
            Ok(reply) => {
                let promoted = output.promote();
                let _ = reply.send(promoted);
                break;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    Ok(WarmExit::Finished)
}

fn run_opened(
    rate: u32,
    channels: u8,
    commands: &ClaimReceiver,
    flags: &Flags,
) -> cs2_runtime::RuntimeResult<()> {
    flags.state.store(WARMING, Ordering::Release);
    if matches!(commands.try_recv(), Err(mpsc::TryRecvError::Disconnected)) {
        return Ok(());
    }
    let mut output = Output::open_pcm(rate, channels)?;
    let mut deadline = Instant::now() + KEEP_ALIVE;
    while Instant::now() < deadline && output.can_activate_opened() {
        if flags.extend.swap(false, Ordering::AcqRel) {
            deadline = Instant::now() + KEEP_ALIVE;
        }
        flags.state.store(READY, Ordering::Release);
        match commands.recv_timeout(OPEN_POLL) {
            Ok(reply) => {
                let staged = if output.can_activate_opened() {
                    output.staged_device = Some(StagedDevice::Open);
                    Some(output)
                } else {
                    None
                };
                let _ = reply.send(staged);
                break;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    Ok(())
}

impl Output {
    fn standby_counters(&mut self) -> Option<(u64, u64, i32, i32)> {
        let stream = self.validate_device().ok()?;
        let state = unsafe { AAudioStream_getState(stream.as_ptr()) };
        if !matches!(state, STARTING | STARTED) {
            return None;
        }
        let read = unsafe { AAudioStream_getFramesRead(stream.as_ptr()) };
        let written = unsafe { AAudioStream_getFramesWritten(stream.as_ptr()) };
        let xruns = unsafe { AAudioStream_getXRunCount(stream.as_ptr()) };
        if read < 0 || written < read || xruns < 0 {
            return None;
        }
        Some((read as u64, written as u64, xruns, state))
    }

    fn promotion_origin(&mut self, minimum_lead: u64) -> Option<(u64, i32)> {
        let (read, written, xruns, state) = self.standby_counters()?;
        let step = u64::from(self.rate / 100);
        let lead = written - read;
        if state == STARTED && self.pending.is_empty() && written == self.accepted_frames && lead >= minimum_lead
            && lead <= 4 * step
        {
            Some((written, xruns))
        } else {
            None
        }
    }

    fn promote(mut self) -> Option<Self> {
        let (origin, xruns) = self.promotion_origin(u64::from(self.rate / 100))?;
        self.frame_origin = origin;
        self.last_consumed = 0;
        self.accepted_frames = 0;
        self.staged_device = Some(StagedDevice::Started(xruns));
        self.paused = true;
        Some(self)
    }

    pub(super) fn can_activate_standby(&mut self, expected_xruns: i32) -> bool {
        self.standby_counters().is_some_and(|(read, written, xruns, state)| {
            state == STARTED && written == self.frame_origin && written - read >= u64::from(self.rate / 200)
                && xruns == expected_xruns
        })
    }

    pub(super) fn can_activate_opened(&mut self) -> bool {
        self.validate_device().is_ok_and(|stream| unsafe {
            AAudioStream_getState(stream.as_ptr()) == OPEN
                && AAudioStream_getFramesRead(stream.as_ptr()) == 0
                && AAudioStream_getFramesWritten(stream.as_ptr()) == 0
        })
    }
}

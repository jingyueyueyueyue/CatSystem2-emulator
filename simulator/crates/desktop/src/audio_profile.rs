//! Timing shared with the PCM worker only during a profiled audio command.
use std::{
    cell::RefCell,
    fmt::Write,
    sync::{Arc, Mutex, atomic::{AtomicU64, Ordering}},
    time::{Duration, Instant},
};

const MAX_ROWS: usize = 3;
const STAGES: [&str; Stage::StateWait as usize + 1] = [
    "read", "prepare", "sidecars", "register", "spawn", "ack",
    "reader", "open", "prime", "activate", "request", "confirm", "close", "standby",
    "confirm_pump", "confirm_expand", "state_wait",
];

#[derive(Clone, Copy)]
pub(crate) enum Stage {
    Read,
    Prepare,
    Sidecars,
    Register,
    Spawn,
    Ack,
    Reader,
    Open,
    Prime,
    Activate,
    Request,
    Confirm,
    Close,
    Standby,
    ConfirmPump,
    ConfirmExpand,
    StateWait,
}

type Times = [Duration; STAGES.len()];
static NEXT_COMMAND: AtomicU64 = AtomicU64::new(1);

pub(crate) struct CommandTiming {
    pub(crate) id: u64,
    pub(crate) channel: u32,
    name: [u8; 40],
    name_len: usize,
    started: Instant,
    times: Mutex<Times>,
}
impl CommandTiming {
    #[cfg(target_os = "android")]
    pub(crate) fn name(&self) -> &str {
        std::str::from_utf8(&self.name[..self.name_len]).unwrap_or("?")
    }
    #[cfg(target_os = "android")]
    pub(crate) fn elapsed(&self) -> Duration { self.started.elapsed() }
}
pub(crate) type Shared = Arc<CommandTiming>;

thread_local! {
    static ACTIVE: RefCell<Option<Shared>> = const { RefCell::new(None) };
}

pub(crate) fn active() -> Option<Shared> {
    ACTIVE.with_borrow(Clone::clone)
}

pub(crate) fn clear() {
    let _ = ACTIVE.with(|slot| slot.take());
}

struct Activation(Option<Shared>);

impl Activation {
    fn replace(next: Option<Shared>) -> Self {
        Self(ACTIVE.with(|slot| slot.replace(next)))
    }
}

impl Drop for Activation {
    fn drop(&mut self) {
        ACTIVE.with(|slot| slot.replace(self.0.take()));
    }
}

pub(crate) fn with_worker<T>(times: Option<Shared>, run: impl FnOnce() -> T) -> T {
    let _activation = Activation::replace(times);
    run()
}

pub(crate) struct Phase(Option<(Shared, Instant, Stage)>);

impl Stage {
    pub(crate) fn begin(self) -> Phase {
        Phase(active().map(|times| (times, Instant::now(), self)))
    }
}

impl Drop for Phase {
    fn drop(&mut self) {
        if let Some((times, start, stage)) = self.0.take() {
            let elapsed = start.elapsed();
            times.times.lock().unwrap_or_else(|error| error.into_inner())[stage as usize] += elapsed;
        }
    }
}

pub(super) struct Sample {
    id: u64,
    task: u32,
    channel: u32,
    operation: &'static str,
    code: Option<u32>,
    name: [u8; 40],
    name_len: usize,
    elapsed: Duration,
    times: Times,
}

pub(crate) struct Profile(Option<(Activation, Shared, Instant, Sample)>);

impl Profile {
    pub(crate) fn new(
        task: u32,
        channel: u32,
        operation: &'static str,
        code: Option<u32>,
        resource: &str,
    ) -> Self {
        if !super::DETAIL.with_borrow(|slot| slot.is_some()) {
            return Self(None);
        }
        let (name, name_len) = super::detail_name(resource.as_bytes());
        let times = Arc::new(CommandTiming {
            id: NEXT_COMMAND.fetch_add(1, Ordering::Relaxed),
            channel, name, name_len, started: Instant::now(), times: Mutex::new(Times::default()),
        });
        let activation = Activation::replace(Some(times.clone()));
        let sample = Sample {
            id: times.id,
            task,
            channel: times.channel,
            operation,
            code,
            name: times.name,
            name_len: times.name_len,
            elapsed: Duration::ZERO,
            times: Times::default(),
        };
        let started = times.started;
        Self(Some((activation, times, started, sample)))
    }
}

impl Drop for Profile {
    fn drop(&mut self) {
        let Some((activation, times, start, mut sample)) = self.0.take() else {
            return;
        };
        drop(activation);
        sample.elapsed = start.elapsed();
        sample.times = *times.times.lock().unwrap_or_else(|error| error.into_inner());
        if sample.elapsed.as_micros() < u128::from(super::SLOW_US) {
            return;
        }
        super::DETAIL.with_borrow_mut(|slot| {
            if let Some(detail) = slot {
                super::keep_slowest(&mut detail.audio, MAX_ROWS, sample, |sample| {
                    sample.elapsed.as_micros().min(u128::from(u64::MAX)) as u64
                });
            }
        });
    }
}

pub(super) fn write_rows(rows: &mut String, mut audio: Vec<Sample>, session: u64, frame: u64, call: u64) {
    audio.sort_by_key(|sample| std::cmp::Reverse(sample.elapsed));
    for sample in audio {
        let name = std::str::from_utf8(&sample.name[..sample.name_len]).unwrap_or("?");
        let mut row = format!(
            "audio_detail session={session} frame={frame} call={call} task={} channel={} operation={} elapsed_us={} audio_id={}",
            sample.task, sample.channel, sample.operation, sample.elapsed.as_micros(), sample.id,
        );
        if let Some(code) = sample.code {
            let _ = write!(row, " code={code}");
        }
        if !name.is_empty() {
            let _ = write!(row, " name={name}");
        }
        for (stage, duration) in STAGES.iter().zip(sample.times) {
            if !duration.is_zero() {
                let _ = write!(row, " {stage}_us={}", duration.as_micros());
            }
        }
        super::append_row(rows, &row);
    }
}

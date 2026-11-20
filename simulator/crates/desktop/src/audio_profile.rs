//! Timing shared with the PCM worker only during a profiled audio command.
use std::{
    cell::RefCell,
    fmt::Write,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

const MAX_ROWS: usize = 3;
const STAGES: [&str; Stage::Standby as usize + 1] = [
    "read", "prepare", "sidecars", "register", "spawn", "ack",
    "reader", "open", "prime", "activate", "request", "confirm", "close", "standby",
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
}

type Times = [Duration; STAGES.len()];
pub(crate) type Shared = Arc<Mutex<Times>>;

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
            times.lock().unwrap_or_else(|error| error.into_inner())[stage as usize] += elapsed;
        }
    }
}

pub(super) struct Sample {
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
        let times = Arc::new(Mutex::new(Times::default()));
        let activation = Activation::replace(Some(times.clone()));
        let sample = Sample {
            task,
            channel,
            operation,
            code,
            name,
            name_len,
            elapsed: Duration::ZERO,
            times: Times::default(),
        };
        Self(Some((activation, times, Instant::now(), sample)))
    }
}

impl Drop for Profile {
    fn drop(&mut self) {
        let Some((activation, times, start, mut sample)) = self.0.take() else {
            return;
        };
        drop(activation);
        sample.elapsed = start.elapsed();
        sample.times = *times.lock().unwrap_or_else(|error| error.into_inner());
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
            "audio_detail session={session} frame={frame} call={call} task={} channel={} operation={} elapsed_us={}",
            sample.task, sample.channel, sample.operation, sample.elapsed.as_micros(),
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

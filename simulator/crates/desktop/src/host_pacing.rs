//! Fixed script ticks with bounded wall-clock debt; no change to script time units.
use std::time::{Duration, Instant};

pub(super) const TICK: Duration = Duration::from_nanos(1_000_000_000 / 60);
pub(super) const MAX_CATCHUP_TICKS: u32 = 4;

pub(super) fn completed_deadline(previous: Instant, now: Instant) -> Instant {
    let oldest = now.checked_sub(TICK * MAX_CATCHUP_TICKS).unwrap_or(now);
    (previous + TICK).max(oldest)
}

pub(super) struct TickPacer { next: Instant }

impl TickPacer {
    pub(super) fn new(now: Instant) -> Self { Self { next: now } }
    pub(super) fn reset(&mut self, now: Instant) { self.next = now; }
    pub(super) fn completed(&mut self, now: Instant) { self.next = completed_deadline(self.next, now); }
    pub(super) fn delay_ms(&self, now: Instant) -> u32 {
        self.next.saturating_duration_since(now).as_nanos().div_ceil(1_000_000).min(100) as u32
    }
}

#[test]
fn fixed_step_clock_catches_up_without_skipping_a_game_tick_or_counting_retries() {
    let start = Instant::now();
    let mut clock = TickPacer::new(start);
    clock.completed(start);
    assert_eq!(clock.delay_ms(start), 17);
    let late = start + TICK * 3;
    assert_eq!(clock.delay_ms(late), 0);
    // Querying/retrying does not consume a scheduled game tick.
    assert_eq!(clock.delay_ms(late), 0);
    clock.completed(late);
    assert_eq!(clock.delay_ms(late), 0);
    clock.completed(late);
    clock.completed(late);
    assert_eq!(clock.delay_ms(late), 17);
    let resumed = start + Duration::from_secs(120);
    clock.reset(resumed);
    clock.completed(resumed);
    assert_eq!(clock.delay_ms(resumed), 17, "pause time cannot become game-time debt");
}

#[test]
fn fixed_step_clock_caps_debt_after_a_long_stall() {
    let start = Instant::now();
    let mut clock = TickPacer::new(start);
    let late = start + Duration::from_secs(30);
    clock.completed(late);
    let mut caught = 0;
    while clock.delay_ms(late) == 0 {
        clock.completed(late); caught += 1;
        assert!(caught <= MAX_CATCHUP_TICKS + 1);
    }
    assert_eq!(caught, MAX_CATCHUP_TICKS + 1);
}

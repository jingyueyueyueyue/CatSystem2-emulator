use crate::{HeadlessPlayer, MovieFrameSurface};
use anyhow::{Context, Result};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Keep exposure/resize events from advancing the simulation faster than 60 Hz.
pub(super) struct WindowPacer {
    pub(super) next: Instant,
    tick: Instant,
}

impl Default for WindowPacer {
    fn default() -> Self {
        let now = Instant::now();
        Self { next: now, tick: now }
    }
}

impl WindowPacer {
    const TICK: Duration = Duration::from_nanos(1_000_000_000 / 60);

    pub(super) fn completed(&mut self, now: Instant) {
        let oldest_tick = now.checked_sub(Self::TICK * 2).unwrap_or(now);
        self.tick = (self.tick + Self::TICK).max(oldest_tick);
        self.next = self.tick;
    }

    pub(super) fn retry(&mut self, now: Instant, in_progress: bool) {
        self.next = now + if in_progress { Duration::from_millis(1) } else { Self::TICK };
    }

    pub(super) fn suspend(&mut self, now: Instant) {
        self.next = now + Duration::from_millis(100);
        self.tick = self.next;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retries_do_not_consume_ticks_and_stalls_have_bounded_debt() {
        let start = Instant::now();
        let mut pacer = WindowPacer { next: start, tick: start };
        pacer.retry(start, true);
        assert_eq!(pacer.next, start + Duration::from_millis(1));
        pacer.completed(pacer.next);
        assert_eq!(pacer.next, start + WindowPacer::TICK);
        let late = start + Duration::from_secs(1);
        pacer.completed(late);
        assert_eq!(pacer.next, late - WindowPacer::TICK * 2);
        for _ in 0..3 { pacer.completed(late); }
        assert_eq!(pacer.next, late + WindowPacer::TICK);
    }
}

pub(super) struct WindowFrame<'a> {
    pub(super) rgba: &'a [u8],
    movies: Vec<(Arc<MovieFrameSurface>, u64, f64)>,
}

impl WindowFrame<'_> {
    pub(super) fn acknowledge(&self) -> Result<()> {
        for (surface, handle, time) in &self.movies {
            surface
                .mark_presented(*handle, *time)
                .context("acknowledge presented movie frame")?;
        }
        Ok(())
    }
}

pub(super) fn prepare(player: &HeadlessPlayer) -> WindowFrame<'_> {
    WindowFrame {
        rgba: player.frame_rgba(),
        movies: player
            .movie_host
            .scene_movie_presentations()
            .into_iter()
            .map(|receipt| (receipt.surface, receipt.playback, receipt.time))
            .collect(),
    }
}

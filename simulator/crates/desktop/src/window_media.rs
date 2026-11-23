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
    const TICK: Duration = crate::host_pacing::TICK;

    pub(super) fn reset(&mut self, now: Instant) {
        self.next = now;
        self.tick = now;
    }

    pub(super) fn completed(&mut self, now: Instant) {
        self.tick = crate::host_pacing::completed_deadline(self.tick, now);
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
    fn focus_resume_discards_background_debt_and_suspend_delay() {
        let start = Instant::now();
        let mut pacer = WindowPacer { next: start, tick: start };
        pacer.suspend(start);
        let resumed = start + Duration::from_secs(120);
        pacer.reset(resumed);
        assert_eq!(pacer.next, resumed);
        pacer.completed(resumed);
        assert_eq!(pacer.next, resumed + WindowPacer::TICK);
    }

    #[test]
    fn fixed_step_pacer_preserves_bounded_debt_instead_of_resetting_after_one_late_frame() {
        let start = Instant::now();
        let mut pacer = WindowPacer { next: start, tick: start };
        let late = start + WindowPacer::TICK * 6;
        pacer.completed(late);
        assert_eq!(pacer.next, late - WindowPacer::TICK * 4);
        let before = pacer.next;
        pacer.retry(late, true);
        pacer.completed(late);
        assert_eq!(pacer.next, before + WindowPacer::TICK);
    }

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
        assert_eq!(pacer.next, late - WindowPacer::TICK * 4);
        for _ in 0..5 { pacer.completed(late); }
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

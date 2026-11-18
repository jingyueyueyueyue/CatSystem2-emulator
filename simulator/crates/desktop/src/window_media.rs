use crate::{HeadlessPlayer, HeadlessReport, MovieFrameSurface};
use anyhow::{Context, Result};
use std::borrow::Cow;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Keep exposure/resize events from advancing the simulation faster than 60 Hz.
pub(super) struct WindowPacer {
    pub(super) next: Instant,
}

impl Default for WindowPacer {
    fn default() -> Self {
        Self {
            next: Instant::now(),
        }
    }
}

impl WindowPacer {
    pub(super) fn consume(&mut self) -> bool {
        let now = Instant::now();
        if now < self.next {
            return false;
        }
        self.next += Duration::from_nanos(1_000_000_000 / 60);
        if now.saturating_duration_since(self.next) > Duration::from_millis(250) {
            self.next = now + Duration::from_nanos(1_000_000_000 / 60);
        }
        true
    }
}

pub(super) struct WindowFrame<'a> {
    pub(super) rgba: Cow<'a, [u8]>,
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

pub(super) fn prepare<'a>(
    player: &HeadlessPlayer,
    report: &'a HeadlessReport,
) -> Result<WindowFrame<'a>> {
    Ok(WindowFrame {
        rgba: Cow::Borrowed(&report.frame_rgba),
        movies: player
            .movie_host
            .scene_movie_presentations()
            .into_iter()
            .map(|receipt| (receipt.surface, receipt.playback, receipt.time))
            .collect(),
    })
}

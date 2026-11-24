use crate::{HeadlessPlayer, HostPresentationFrame};
use anyhow::{Context, Result};
#[cfg(test)]
use cs2_renderer::Canvas;
use std::{
    sync::{Arc, Condvar, Mutex, mpsc},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

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

    pub(super) fn retry(&mut self, now: Instant, in_progress: bool, delay_ms: u32) {
        self.next = now + if in_progress || delay_ms == 0 {
            Duration::from_millis(u64::from(delay_ms))
        } else {
            Self::TICK
        };
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
        pacer.retry(late, true, 0);
        assert_eq!(pacer.next, late);
        pacer.completed(late);
        assert_eq!(pacer.next, before + WindowPacer::TICK);
    }

    #[test]
    fn retries_do_not_consume_ticks_and_stalls_have_bounded_debt() {
        let start = Instant::now();
        let mut pacer = WindowPacer { next: start, tick: start };
        pacer.retry(start, true, 1);
        assert_eq!(pacer.next, start + Duration::from_millis(1));
        pacer.completed(pacer.next);
        assert_eq!(pacer.next, start + WindowPacer::TICK);
        let late = start + Duration::from_secs(1);
        pacer.completed(late);
        assert_eq!(pacer.next, late - WindowPacer::TICK * 4);
        for _ in 0..5 { pacer.completed(late); }
        assert_eq!(pacer.next, late + WindowPacer::TICK);
    }

    #[test]
    fn slow_window_presentation_keeps_the_latest_completed_frame() -> Result<()> {
        let (rendered, received) = mpsc::channel();
        let (resume, resume_worker) = mpsc::channel();
        let presenter = WindowPresenter::start(move |frame| {
            let value = frame.rgba()[0];
            rendered.send(value)?;
            if value == 1 { resume_worker.recv_timeout(Duration::from_secs(2))?; }
            Ok(true)
        })?;
        let frame = |value| -> Result<WindowFrame> {
            let mut canvas = Canvas::new(1, 1)?;
            canvas.clear(cs2_renderer::Color::rgb(value, 0, 0));
            Ok(WindowFrame { canvas, movies: Vec::new() })
        };
        presenter.publish(frame(1)?)?;
        assert_eq!(received.recv_timeout(Duration::from_secs(2))?, 1);
        presenter.publish(frame(2)?)?;
        presenter.discard_pending();
        presenter.publish(frame(3)?)?;
        presenter.publish(frame(4)?)?;
        resume.send(())?;
        assert_eq!(received.recv_timeout(Duration::from_secs(2))?, 4);
        presenter.check()?;
        drop(presenter);
        assert!(received.try_recv().is_err());
        Ok(())
    }
}

type WindowFrame = HostPresentationFrame;

impl HostPresentationFrame {
    fn acknowledge(&self) -> Result<()> {
        for receipt in &self.movies {
            receipt.surface
                .mark_presented(receipt.playback, receipt.time)
                .context("acknowledge presented movie frame")?;
        }
        Ok(())
    }
}

pub(super) fn prepare(player: &mut HeadlessPlayer) -> WindowFrame {
    player.host_presentation_frame()
}

struct PendingFrame {
    frame: WindowFrame,
    receipt: Option<mpsc::SyncSender<Result<(), String>>>,
}

#[derive(Default)]
struct PresentationState {
    pending: Option<PendingFrame>,
    stopped: bool,
    error: Option<String>,
}

/// Owns the Windows presentation endpoint. There can be one completed frame
/// pending and one in presentation, bounded by the logical canvas size.
pub(super) struct WindowPresenter {
    shared: Arc<(Mutex<PresentationState>, Condvar)>,
    thread: Option<JoinHandle<()>>,
}

impl WindowPresenter {
    pub(super) fn start(mut draw: impl FnMut(&WindowFrame) -> Result<bool> + Send + 'static) -> Result<Self> {
        let shared = Arc::new((Mutex::new(PresentationState::default()), Condvar::new()));
        let worker = Arc::clone(&shared);
        let thread = thread::Builder::new().name("CS2 presenter".into()).spawn(move || {
            let (state, wake) = &*worker;
            loop {
                let next = {
                    let ready = state.lock().unwrap();
                    let mut ready = wake.wait_while(ready, |ready| !ready.stopped && ready.pending.is_none()).unwrap();
                    if ready.stopped {
                        return;
                    }
                    ready.pending.take().unwrap()
                };
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    if draw(&next.frame)? { next.frame.acknowledge()?; }
                    Ok(())
                })).unwrap_or_else(|_| Err(anyhow::anyhow!("window presentation worker panicked")))
                    .map_err(|error| format!("{error:#}"));
                if let Some(receipt) = next.receipt {
                    let _ = receipt.send(result.clone());
                }
                if let Err(error) = result {
                    let mut ready = state.lock().unwrap();
                    ready.error = Some(error);
                    ready.stopped = true;
                    let pending = ready.pending.take();
                    drop(ready);
                    drop(pending);
                    return;
                }
            }
        }).context("start window presentation worker")?;
        Ok(Self { shared, thread: Some(thread) })
    }

    pub(super) fn publish(&self, frame: WindowFrame) -> Result<()> {
        let (receipt, receive) = if frame.movies.is_empty() {
            (None, None)
        } else {
            // The logic thread needs the actual movie acknowledgement before
            // advancing a dependent playback state.
            let (send, receive) = mpsc::sync_channel(1);
            (Some(send), Some(receive))
        };
        let (state, wake) = &*self.shared;
        let old = {
            let mut ready = state.lock().unwrap();
            Self::check_state(&ready)?;
            ready.pending.replace(PendingFrame { frame, receipt })
        };
        wake.notify_one();
        drop(old);
        if let Some(receive) = receive {
            receive.recv().context("receive window presentation result")?.map_err(anyhow::Error::msg)?;
        }
        Ok(())
    }

    fn check_state(state: &PresentationState) -> Result<()> {
        if let Some(error) = &state.error {
            anyhow::bail!("{error}");
        }
        anyhow::ensure!(!state.stopped, "window presentation worker is stopped");
        Ok(())
    }

    pub(super) fn check(&self) -> Result<()> {
        let state = self.shared.0.lock().unwrap();
        Self::check_state(&state)?;
        anyhow::ensure!(
            !self.thread.as_ref().is_some_and(JoinHandle::is_finished),
            "window presentation worker exited unexpectedly",
        );
        Ok(())
    }

    pub(super) fn discard_pending(&self) {
        let pending = self.shared.0.lock().unwrap().pending.take();
        drop(pending);
    }
}

impl Drop for WindowPresenter {
    fn drop(&mut self) {
        let (state, wake) = &*self.shared;
        let pending = {
            let mut ready = state.lock().unwrap();
            ready.stopped = true;
            ready.pending.take()
        };
        wake.notify_one();
        drop(pending);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

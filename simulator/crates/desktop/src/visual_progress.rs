//! Read-only, wall-clock sampled diagnostics. Scene versions are not display acknowledgements.
use std::time::{Duration, Instant};
use super::{HeadlessPlayer, perf_detail};

#[derive(Default)]
pub(super) struct VisualProgress {
    pub(super) last: Option<(Instant, u64, u64, u32, u32)>,
    pub(super) frame_clock_sampled: u32,
}

pub(super) fn token(bytes: &[u8]) -> String {
    bytes.iter().take(24).map(|&b| if b.is_ascii_alphanumeric() || b"_-./".contains(&b) {
        b as char
    } else { '_' }).collect()
}

pub(super) fn optional(value: Option<u32>) -> String {
    value.map_or_else(|| "na".into(), |value| value.to_string())
}

impl HeadlessPlayer {
    pub(super) fn trace_visual_progress(&mut self) {
        if !perf_detail::enabled() { return; }
        let now = Instant::now();
        let frame = self.state.clock.frame;
        let tick = self.state.clock.elapsed_ticks;
        let sampled = self.visual_progress.frame_clock_sampled;
        let requested = self.data_services.top_clock.requested;
        let previous = self.visual_progress.last;
        if previous.is_some_and(|(at, _, _, old_sampled, old_requested)|
            now.duration_since(at) < Duration::from_millis(500)
                && old_sampled == sampled && old_requested == requested) { return; }
        let (wall_ms, frames, ticks, reset) = previous.map_or((0, 0, 0, false), |(at, f, t, _, _)|
            (now.duration_since(at).as_millis(), frame.saturating_sub(f), tick.saturating_sub(t), frame < f || tick < t));
        self.visual_progress.last = Some((now, frame, tick, sampled, requested));
        let pending_audio = self.sound_channels.values().filter(|channel| channel.pending_start.is_some()).count();
        perf_detail::visual_note("visual_clock", format!(
            "logic_frame={frame} tick={tick} wall_ms={wall_ms} frame_delta={frames} tick_delta={ticks} reset={} clock_sampled={sampled} clock_requested={requested} clock_next={} phase={:?} host_paused={} input_deferred={} configuration_deferred={} pending_audio={pending_audio}",
            u8::from(reset), self.data_services.top_clock.sampled, self.frame_phase,
            u8::from(self.data_services.host_pause.paused), u8::from(self.audio_input_deferred),
            u8::from(self.audio_configuration_deferred)));
        let (fes, fes_kept) = self.data_services.tasks.fes.trace_visual_progress(&self.data_services.tasks);
        let (text, text_kept) = self.data_services.fes_text.trace_visual_progress();
        let (poses, poses_kept) = self.data_services.emote.trace_visual_progress();
        perf_detail::visual_note("visual_sources", format!(
            "fes={fes} fes_kept={fes_kept} text={text} text_kept={text_kept} poses={poses} poses_kept={poses_kept} snapshot_us={}", now.elapsed().as_micros()));
    }
}

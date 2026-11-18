//! Kind13/202/203 queries over real loaded sources and device consumption.
use cs2_script::Result;

use super::{LoggingHost, SoundChannelState, audio_script_error};

#[derive(Debug, Clone, Default, PartialEq, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(super) struct SourceQueries {
    counter_origin: u32,
    live_counter: bool,
    loop_start: Option<i64>,
}

impl SoundChannelState {
    pub(super) fn source_query_loaded(&mut self) {
        self.task.source_queries = SourceQueries::default();
    }

    pub(super) fn source_query_started(&mut self, counter_origin: u32, count: u32) {
        self.task.source_queries = SourceQueries {
            counter_origin,
            live_counter: true,
            loop_start: Some(if count == 1 { 0 } else { self.task.loop_range[0] }),
        };
    }

    pub(super) fn source_query_stopped(&mut self) {
        self.task.source_queries.live_counter = false;
        self.task.source_queries.counter_origin = 0;
    }

    pub(super) fn source_query_repeat_changed(&mut self, count: u32) {
        let live = self.task.source_queries.live_counter;
        let origin = self.task.signal.as_ref().map_or(0, |signal| signal.completed_passes());
        self.source_query_started(origin, count);
        self.task.source_queries.live_counter = live;
    }
}

impl LoggingHost<'_> {
    /// PE 202 reads resource+0x174. Its constructor initializes zero, and
    /// it resets on
    /// physical play/stop, and increments only the whole-buffer completion
    /// branch. The streaming loop bookkeeping uses separate fields.
    pub(super) fn sound_source_completed_passes(&self, logical: u32) -> Result<u32> {
        if logical >= 64 {
            return Ok(0);
        }
        let channel = self.sound_selected_channel(logical);
        let Some(state) = self.sound_channels.get(&channel).filter(|state| state.resource.is_some())
        else {
            return Ok(0);
        };
        let queries = &state.task.source_queries;
        if !queries.live_counter {
            return Ok(0);
        }
        if !state.task.whole_buffer {
            return Ok(0);
        }
        let signal = state.task.signal.as_ref().ok_or_else(|| {
            audio_script_error("sound202 requires an actual device completion counter")
        })?;
        Ok(signal.completed_passes().wrapping_sub(queries.counter_origin))
    }

    /// 203's pure getter reads the first loop/seek boundary as an i64.
    /// Its task receiver deliberately discards that value and leaves the
    /// caller's output DWORD untouched; direct hosts can use this safe API.
    pub(super) fn sound_source_loop_start(&self, physical: u32) -> Option<i64> {
        (physical < 64)
            .then(|| self.sound_channels.get(&physical))
            .flatten()
            .filter(|state| state.resource.is_some())
            .and_then(|state| state.task.source_queries.loop_start)
    }
}

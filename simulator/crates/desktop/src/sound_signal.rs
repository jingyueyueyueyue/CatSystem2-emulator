//! Kind13/214: explicit bounded signal-window output, including the nested
//! caller-VM record. Slot values come from PCM monitoring or a parsed PVL file.
use cs2_script::Result;

use super::{LoggingHost, audio_script_error};

pub(super) const MAX_SIGNAL_VALUES: usize = 4096;

impl LoggingHost<'_> {
    pub(super) fn sound_signal_capacity(&self, logical: u32, requested: u32) -> Result<usize> {
        if logical >= 64 {
            return Ok(0);
        }
        let channel = self.sound_selected_channel(logical);
        let Some(state) = self.sound_channels.get(&channel) else {
            return Ok(0);
        };
        let precomputed = state.task.sidecars.signal.is_some();
        if !precomputed && state.resource.is_none() {
            return Ok(0);
        }
        let requested = requested as usize;
        if !precomputed {
            return Ok(requested.min(80));
        }
        if requested > MAX_SIGNAL_VALUES {
            return Err(audio_script_error(
                "PVL signal window exceeds4096 output values",
            ));
        }
        Ok(requested)
    }

    /// None probes the documented80-value window without returning an address.
    /// Present output receives raw DWORD levels. PVL returns actual available
    /// values; the PCM ring returns up to80 positions, zeroing unfilled slots.
    pub(super) fn sound_signal_window(
        &self,
        logical: u32,
        output: Option<&mut [u32]>,
    ) -> Result<u32> {
        if logical >= 64 {
            return Ok(0);
        }
        let channel = self.sound_selected_channel(logical);
        let Some(state) = self.sound_channels.get(&channel) else {
            return Ok(0);
        };
        if state.task.sidecars.signal.is_none() && state.resource.is_none() {
            return Ok(0);
        }
        let Some(output) = output else {
            return Ok(80);
        };
        if output.len() > MAX_SIGNAL_VALUES {
            return Err(audio_script_error(
                "signal output exceeds its bounded window",
            ));
        }
        if let Some(signal) = &state.task.sidecars.signal {
            return Ok(signal.window(self.host_millis(), state.task.started_tick, output) as u32);
        }
        let count = output.len().min(80);
        output[..count].fill(0);
        if let Some(signal) = &state.task.signal {
            return Ok(signal.copy_window(&mut output[..count]) as u32);
        }
        if state.handle.is_some() {
            return Err(audio_script_error(
                "active custom audio host has no PCM signal window",
            ));
        }
        // Before first play, or after release, there are no device samples.
        // This does not start audio or assert decoder/device success.
        Ok(if state.resource.is_some() {
            count as u32
        } else {
            0
        })
    }
}

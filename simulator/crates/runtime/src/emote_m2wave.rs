//! Actual M2 waveform/lip_flow operators174610,184130,184f90,185080,1851a0.
//! Input is the original owned DWORD window and its separately supplied raw
//! extent. A scalar meter value is not fabricated into a window.

use super::{CpuM2LipDefinition, EmoteError, Result};

#[derive(Debug, Clone)]
pub struct CpuM2Waveform {
    definition: CpuM2LipDefinition,
    pub enabled: bool,
    start: u32,
    phase: usize,
    from: f32,
    to: f32,
    duration: f32,
    next_duration: f32,
    finish_check: u32,
    next_finish_check: u32,
    finish_requested: bool,
    quiet_tail: bool,
    /// Actual node+78. A loud observation clears it; a nonzero value skips
    /// quiet lookahead1746be without disabling the current evaluator.
    pub quiet_scan_inhibited: bool,
    base: f32,
}

fn invalid(why: &'static str) -> EmoteError {
    EmoteError::InvalidResource(why)
}

impl CpuM2Waveform {
    pub fn new(definition: CpuM2LipDefinition, base: f32) -> Result<Self> {
        if definition.flow.len() < 2
            || definition.flow.len() > 4096
            || definition.values.is_empty()
            || !base.is_finite()
            || definition
                .flow
                .iter()
                .any(|step| !step.frames.is_finite() || step.frames <= 0.0)
            || definition.values.iter().any(|value| !value.is_finite())
            || !definition.level_min.is_finite()
            || definition.level_min < 0.0
            || definition.level_min.fract() != 0.0
            || definition.level_min >= u32::MAX as f32
        {
            return Err(invalid("M2 waveform actual flow/threshold"));
        }
        if !matches!(
            definition.interpolation_cp932.as_slice(),
            b"fix" | b"linear"
        ) {
            return Err(EmoteError::Unsupported {
                native: 924,
                reason: "M2 waveform lip_table interpolation is not recovered",
            });
        }
        let mut output = Self {
            definition,
            enabled: false,
            start: 0,
            phase: 0,
            from: base,
            to: base,
            duration: 1.0,
            next_duration: 1.0,
            finish_check: 0,
            next_finish_check: 0,
            finish_requested: false,
            quiet_tail: false,
            quiet_scan_inhibited: false,
            base,
        };
        output.reset_flow();
        Ok(output)
    }
    fn table_value(&self, phase: usize) -> f32 {
        let index = self.definition.flow[phase].pattern as usize;
        self.definition
            .values
            .get(index)
            .copied()
            .unwrap_or(self.definition.values[0])
    }
    fn reset_flow(&mut self) {
        //184f90 starts at flow0 and preloads flow1; no guessed oscillation.
        self.start = 0;
        self.phase = 0;
        self.finish_requested = false;
        self.from = self.table_value(0);
        self.to = self.table_value(1);
        self.duration = self.definition.flow[0].frames;
        self.next_duration = self.definition.flow[1].frames;
        self.finish_check = self.definition.flow[0].finish_check;
        self.next_finish_check = self.definition.flow[1].finish_check;
    }
    pub fn disable(&mut self) {
        self.enabled = false;
        self.start = 0;
    }
    ///184bd0 bank selection clears record60 while preserving flow94..b0.
    pub(super) fn reset_timestamp(&mut self) {
        self.start = 0;
    }
    pub fn request_finish(&mut self) {
        self.finish_requested = true;
        self.to = self.definition.values[0];
    }
    ///174610 uses sample1 as current. Its lookahead scans indices1..extent/4
    ///and requires floor(extent/10) loud entries before declaring a quiet tail.
    ///The host passes the original extent; it is not derived from byte count.
    pub fn observe(&mut self, samples: &[u32], original_extent: u32) -> Result<()> {
        if original_extent <= 1 {
            return Ok(());
        }
        let scanned = (original_extent >> 2) as usize;
        if samples.len() < scanned.max(2) || scanned > 1_000_000 {
            return Err(invalid("M2 waveform owned window extent"));
        }
        let threshold = self.definition.level_min as u32;
        if samples[1] >= threshold {
            if !self.enabled {
                self.reset_flow();
            }
            self.enabled = true;
            self.quiet_scan_inhibited = false;
        } else if self.enabled && !self.quiet_scan_inhibited {
            let required = original_extent / 10;
            let mut count = 0u32;
            for value in &samples[1..scanned.max(1)] {
                if *value >= threshold {
                    count = count.wrapping_add(1);
                    if count >= required {
                        return Ok(());
                    }
                }
            }
            self.quiet_tail = true;
        }
        Ok(())
    }
    fn next_phase(&mut self) {
        //185080 shifts preloaded fields and wraps the actual flow vector.
        self.phase = (self.phase + 1) % self.definition.flow.len();
        self.from = self.to;
        self.duration = self.next_duration;
        self.finish_check = self.next_finish_check;
        self.finish_requested = false;
        let following = (self.phase + 1) % self.definition.flow.len();
        self.to = self.table_value(following);
        self.next_duration = self.definition.flow[following].frames;
        self.next_finish_check = self.definition.flow[following].finish_check;
    }
    ///184130 advances at most one flow boundary per original update, resetting
    ///the timestamp to now instead of consuming a guessed leftover delta.
    pub fn sample(&mut self, now: u32) -> Result<f32> {
        if !self.enabled {
            return Ok(self.base);
        }
        if self.start == 0 {
            self.start = now;
        }
        let ratio = now.wrapping_sub(self.start) as f32 / self.duration;
        let value = if ratio >= 1.0 {
            if self.finish_requested {
                self.disable();
                self.reset_flow();
                //1841cc..18421c clears all the completed phase fields,
                //including b0.184f90 alone does not clear that latch.
                self.quiet_tail = false;
                self.base
            } else {
                if self.quiet_tail && self.finish_check != 0 {
                    //1851a0 requests one final interval to table0.
                    self.request_finish();
                } else {
                    self.next_phase();
                }
                self.start = now;
                self.from
            }
        } else if self.definition.interpolation_cp932 == b"linear" {
            (self.to - self.from) * ratio + self.from
        } else {
            self.from
        };
        if !value.is_finite() {
            return Err(invalid("M2 waveform interpolation overflow"));
        }
        Ok(value)
    }
}

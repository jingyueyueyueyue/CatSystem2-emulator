//! Enabled mouthControl.talkLabel receiver: be00 -> bf10, fixed NEKO driver.
//! This is the replacement-target path already issued by M2. The independent
//! integer mouth label never enters this lane. No autonomous speech is made.

use super::{EmoteError, PsbValue, Result};

pub(super) fn is_talk_label(root: &PsbValue, name: &str) -> bool {
    root.get("metadata")
        .and_then(|metadata| metadata.get("mouthControl"))
        .and_then(PsbValue::list)
        .unwrap_or(&[])
        .iter()
        .any(|control| {
            control.get("enabled").and_then(PsbValue::number) != Some(0.0)
                && control.get("talkLabel").and_then(PsbValue::text) == Some(name)
        })
}

#[derive(Debug, Clone)]
struct Segment {
    from: f32,
    progress: f32,
    rate: f32,
}

/// The receiver holds its current output while a positive-duration request
/// waits for bf7f to dequeue it. In particular a second write before the next
/// tick replaces the pending request; it is not an already-running segment
/// eligible for be79's equal-target/remaining-time shortcut.
#[derive(Debug, Clone)]
pub(super) struct CpuMouthTalk {
    value: f32,
    target: f32,
    duration: f32,
    exponent: f32,
    pending: bool,
    segment: Option<Segment>,
}

impl CpuMouthTalk {
    pub(super) fn pending(value: f32, target: f32, duration: f32, exponent: f32) -> Self {
        Self { value, target, duration, exponent, pending: true, segment: None }
    }

    pub(super) fn value(&self) -> f32 { self.value }
    pub(super) fn target(&self) -> f32 { self.target }

    pub(super) fn retain_equal_target(&self, target: f32, duration: f32) -> bool {
        //be73..bea2 only tests the ACTIVE state word+18. A pending tuple in
        //the owned queue does not qualify. Equality and division are f32.
        self.segment.as_ref().is_some_and(|segment| {
            self.target == target && duration > (1.0 - segment.progress) / segment.rate
        })
    }

    pub(super) fn advance(&mut self, delta: f32) -> Result<bool> {
        if self.pending {
            //bf7f..bfff: dequeue, snapshot CURRENT output+20 into+24, then
            //start phase0/rate1/duration. There is no value write at enqueue.
            self.pending = false;
            self.segment = Some(Segment { from: self.value, progress: 0.0, rate: 1.0 / self.duration });
        }
        let Some(segment) = self.segment.as_mut() else { return Ok(true); };
        segment.progress += segment.rate * delta;
        if segment.progress - 1.0 >= -0.0001 {
            //bf69..bf76 commits the exact endpoint and releases active state.
            //A replacement/zero-duration write and finish_all use the same
            //existing owner cancellation/removal path in m2timed.
            self.value = self.target;
            self.segment = None;
            return Ok(true);
        }
        let ratio = f64::from(segment.progress).powf(f64::from(self.exponent)) as f32;
        self.value = segment.from + (self.target - segment.from) * ratio;
        if !self.value.is_finite() {
            return Err(EmoteError::InvalidResource("mouth talk controller value overflow"));
        }
        Ok(false)
    }
}

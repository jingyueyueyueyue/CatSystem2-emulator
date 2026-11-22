//! NEKO M2 surface owner: 17add5/17b7a3 and 19b6e0/19c640/19c8f0.
//! Captured RGBA surfaces are independent of the final task-quad tint.

use super::{EmoteError, EmoteRasterFrame, Result};

// None leaves the incoming alpha classification on the host device.
pub type EmoteSurfaceMixer = fn(&EmoteRasterFrame, &EmoteRasterFrame, u32, Option<bool>)
    -> Option<EmoteRasterFrame>;
pub type EmoteSurfacePrepare = fn(&EmoteRasterFrame, &EmoteRasterFrame)
    -> Option<(EmoteRasterFrame, EmoteRasterFrame, Option<bool>)>;
pub type EmoteSurfaceExpand = fn(&EmoteRasterFrame, [u32; 2]) -> Option<EmoteRasterFrame>;

#[derive(Clone)]
struct Blend {
    old: EmoteRasterFrame,
    new: EmoteRasterFrame,
    shown: EmoteRasterFrame,
    new_is_transparent: Option<bool>,
    duration: u32,
    start: Option<u32>,
}

impl Blend {
    fn holds_at(&self, now: u32) -> bool {
        now.wrapping_sub(self.start.unwrap_or(now)).min(self.duration) as f32
            / (self.duration as f32) < 1.0
    }
    fn new(
        mut old: EmoteRasterFrame, mut new: EmoteRasterFrame, duration: u32,
        prepare: Option<EmoteSurfacePrepare>,
    ) -> Result<Self> {
        if old.width != new.width || old.height != new.height || old.origin != new.origin {
            return Err(EmoteError::InvalidFrame);
        }
        if let Some((old, new, new_is_transparent)) = prepare.and_then(|prepare| prepare(&old, &new)) {
            return Ok(Self { shown: old.clone(), old, new, new_is_transparent, duration, start: None });
        }
        // 19c8f0 runs at fraction zero. Fill only fully transparent RGB;
        // neither source's alpha changes. The OR uses the incoming alpha.
        let mut incoming_alpha = 0_u8;
        for (a, b) in old.rgba_mut().chunks_exact_mut(4).zip(new.rgba_mut().chunks_exact_mut(4)) {
            incoming_alpha |= b[3];
            if a[3] == 0 && b[3] != 0 {
                a[..3].copy_from_slice(&b[..3]);
            }
            if a[3] != 0 && b[3] == 0 {
                b[..3].copy_from_slice(&a[..3]);
            }
        }
        Ok(Self { shown: old.clone(), old, new,
            new_is_transparent: Some(incoming_alpha == 0), duration, start: None })
    }

    /// False at retirement permits the same host tick to continue into the
    /// real driver update. No driver/owner clock is advanced on a held tick.
    fn hold(&mut self, now: u32, mixer: Option<EmoteSurfaceMixer>) -> bool {
        // 17b7ca arms+24; 17ade4 replaces+30 on the first following update.
        let start = *self.start.get_or_insert(now);
        let elapsed = now.wrapping_sub(start).min(self.duration);
        // SSE separately converts the unsigned DWORD numerator/denominator
        // to f32, then divides; do not replace by a f64 elapsed/duration.
        let fraction = elapsed as f32 / self.duration as f32;
        if fraction >= 1.0 {
            self.shown = self.new.clone();
            return false;
        }
        // 3a0320 truncates the finite nonnegative product to an integer.
        let new_weight = (fraction * 256.0_f32) as u32;
        let old_weight = 256 - new_weight;
        if new_weight == 0 {
            self.shown = self.old.clone();
            return true;
        }
        if let Some(shown) = mixer.and_then(|mix| mix(&self.old, &self.new, new_weight, self.new_is_transparent)) {
            self.shown = shown;
            return true;
        }
        if self.shown.device().is_some() {
            self.shown = self.old.clone();
        }
        if *self.new_is_transparent.get_or_insert_with(||
            !self.new.rgba().chunks_exact(4).any(|pixel| pixel[3] != 0))
        {
            // 19c7f0 preserves RGB rather than lerping toward black.
            for (out, old) in self.shown.rgba_mut().chunks_exact_mut(4)
                .zip(self.old.rgba().chunks_exact(4)) {
                out[..3].copy_from_slice(&old[..3]);
                out[3] = ((u32::from(old[3]) * old_weight) >> 8) as u8;
            }
        } else {
            for ((out, old), new) in self.shown.rgba_mut().iter_mut()
                .zip(self.old.rgba()).zip(self.new.rgba()) {
                *out = ((u32::from(*old) * old_weight
                    + u32::from(*new) * new_weight) >> 8) as u8;
            }
        }
        true
    }
}

#[derive(Clone)]
pub(super) struct CpuM2SurfaceOwner {
    duration: u32,
    default_duration: u32,
    pending: Option<EmoteRasterFrame>,
    deferred_capture: bool,
    presented: Option<EmoteRasterFrame>,
    blend: Option<Blend>,
}

impl Default for CpuM2SurfaceOwner {
    fn default() -> Self {
        // 19f560 initializes config+b8 to30; m2view #fade can replace it.
        Self { duration: 30, default_duration: 30, pending: None, deferred_capture: false,
            presented: None, blend: None }
    }
}

impl CpuM2SurfaceOwner {
    pub(super) fn set_default(&mut self, duration: u32) {
        self.duration = duration;
        self.default_duration = duration;
    }

    pub(super) fn request(&mut self, duration: Option<u32>, previous: EmoteRasterFrame, selection_changed: bool) {
        if let Some(duration) = duration { self.duration = duration; }
        if selection_changed { self.blend = None; }
        // 17a990 cancels a blend on selection load. A bare545 only changes
        // the player's pending fields; an already active manager keeps holding.
        self.pending = Some(previous);
        self.deferred_capture = false;
    }

    ///1743c0 writes only+a0/+9c. The manager captures the old surface at
    ///17afe7, after any existing hold has retired and before its next update.
    pub(super) fn request_capture(&mut self, duration: u32) {
        self.duration = duration;
        self.deferred_capture = true;
    }

    pub(super) fn needs_capture(&self) -> bool {
        self.deferred_capture && self.pending.is_none()
    }

    pub(super) fn capture_enabled(&self, enabled: bool) -> bool {
        enabled && self.duration != 0
    }

    ///1741dc consumes9c on the real update even if17afd5..17afe0 skipped
    ///capture for disabled controls or zero duration. Keep the current image.
    pub(super) fn discard_pending(&mut self) {
        self.pending = None;
        self.deferred_capture = false;
    }

    pub(super) fn pending(&self) -> bool { self.pending.is_some() || self.deferred_capture }

    ///17bfa0 queries manager+20, not a pending wrapper surface request.
    pub(super) fn blending(&self) -> bool { self.blend.is_some() }

    ///17c2a0 calls19b6e0 with fraction1, then clears manager+20.
    pub(super) fn finish_blend(&mut self) {
        if let Some(blend) = self.blend.take() {
            if self.pending.is_some() { self.pending = Some(blend.new.clone()); }
            self.presented = Some(blend.new);
        }
    }

    pub(super) fn default_duration(&self) -> u32 { self.default_duration }

    pub(super) fn resource_rebuilt(&mut self) { self.duration = self.default_duration; }

    pub(super) fn holds_at(&self, now: u32) -> bool {
        self.blend.as_ref().is_some_and(|blend| blend.holds_at(now))
    }

    pub(super) fn frame(&self) -> Option<&EmoteRasterFrame> {
        self.blend.as_ref().map(|b| &b.shown).or(self.pending.as_ref())
    }

    pub(super) fn capture(&self) -> Option<&EmoteRasterFrame> {
        self.frame().or(self.presented.as_ref())
    }

    pub(super) fn remember(&mut self, frame: EmoteRasterFrame) -> Option<EmoteRasterFrame> {
        self.presented.replace(frame)
    }

    pub(super) fn clear(&mut self) {
        self.pending = None;
        self.deferred_capture = false;
        self.presented = None;
        self.blend = None;
    }

    pub(super) fn selection_loaded(&mut self) { self.blend = None; }

    pub(super) fn begin(
        &mut self, next: EmoteRasterFrame, enabled: bool, prepare: Option<EmoteSurfacePrepare>,
    ) -> Result<()> {
        if self.needs_capture() {
            return Err(EmoteError::InvalidResource(
                "M2 deferred surface request must capture before driver update",
            ));
        }
        let Some(previous) = self.pending.as_ref() else { return Ok(()); };
        let blend = if enabled && self.duration != 0 {
            Some(Blend::new(previous.clone(), next.clone(), self.duration, prepare)?)
        } else { None };
        self.pending = None;
        self.deferred_capture = false;
        self.presented = Some(next);
        self.blend = blend;
        Ok(())
    }

    pub(super) fn hold(&mut self, now: u32, mixer: Option<EmoteSurfaceMixer>) -> bool {
        let held = self.blend.as_mut().is_some_and(|blend| blend.hold(now, mixer));
        if !held {
            if let Some(blend) = self.blend.take() {
                if self.pending.is_some() { self.pending = Some(blend.new.clone()); }
                self.presented = Some(blend.new);
            }
        }
        held
    }
}

//! SKFE sampled curves: fixed PE reader eddc0, sampler ede60, coordinate
//! change edfa0 and magnification ee000. Independent implementation.
use super::Error;

#[derive(Debug, Clone)]
pub(super) struct FCurve {
    pub edges: [i32; 2],
    pub random_offset: f32,
    pub frame_offset: i32,
    pub length: i32,
    pub frequency: i32,
    pub keys: Vec<f32>,
    pub default: f32,
}
impl FCurve {
    fn output(&self, v: f32) -> Result<f32, Error> {
        let value = v + self.random_offset;
        if value.is_finite() {
            Ok(value)
        } else {
            Err(Error::Host("nonfinite FCurve sample"))
        }
    }
    pub fn value(&self, age: f32) -> Result<f32, Error> {
        if !age.is_finite() || !(-2147483648.0..2147483648.0).contains(&age) {
            return Err(Error::Host(
                "FCurve time outside native signed32 frame domain",
            ));
        }
        if self.keys.is_empty() {
            return self.output(self.default);
        }
        let mut frame = (age as i32).wrapping_sub(self.frame_offset);
        if frame < 0 {
            if self.edges[0] == 0 {
                return self.output(self.keys[0]);
            }
            let before = frame
                .checked_neg()
                .ok_or(Error::Host("FCurve native INT_MIN edge"))?;
            frame = if self.edges[0] == 1 {
                self.length - before % self.length
            } else {
                before % self.length
            };
        }
        if frame > self.length {
            if self.edges[1] == 0 {
                return self.output(self.keys[self.keys.len() - 1]);
            }
            let after = frame - self.length;
            frame = if self.edges[1] == 1 {
                after % self.length
            } else {
                self.length - after % self.length
            };
        }
        if frame == self.length {
            return self.output(self.keys[self.keys.len() - 1]);
        }
        let index = (frame / self.frequency) as usize;
        let a = *self.keys.get(index).ok_or(Error::Host(
            "FCurve native key address outside declared keys",
        ))?;
        let b = *self.keys.get(index + 1).ok_or(Error::Host(
            "FCurve native terminal interval lacks next key",
        ))?;
        let begin = (index as i32) * self.frequency;
        // PE compares index with count-1 after reading index+1. Preserve the
        // valid address subset; do not invent a corrected terminal interval.
        let span = if index == self.keys.len() - 1 {
            self.length - begin
        } else {
            self.frequency
        };
        self.output((b - a) / span as f32 * (frame - begin) as f32 + a)
    }
    pub fn multiply(&mut self, k: f32) {
        self.random_offset *= k;
        for value in &mut self.keys {
            *value *= k;
        }
        // Native Maginify/ChangeCoordinate do not modify the empty default.
    }
}

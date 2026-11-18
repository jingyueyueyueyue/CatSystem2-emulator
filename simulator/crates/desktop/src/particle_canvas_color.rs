//! Object-source RGBA: 41e870 -> 48a4e0 -> 48a510 -> V_DIFCOL.
//! Its packed message is low-byte R/G/B/A, independently of D3DCOLOR's
//! vertex-buffer ARGB representation. No task opacity or parent tint applies.

#[derive(Clone, Copy, Debug)]
pub(super) struct Color([f32; 4]);

impl Default for Color {
    fn default() -> Self { Self([1.; 4]) } // 48a478..48a48c
}

impl Color {
    pub(super) fn set(&mut self, packed: u32) {
        // The x87 helper divides byte values by its double constant255,
        // then stores each source channel as f32.
        self.0 = [0, 8, 16, 24].map(|shift| (f64::from((packed >> shift) & 255) / 255.) as f32);
    }

    pub(super) fn rgba(self) -> [f32; 4] { self.0 }

    pub(super) fn packed(self) -> u32 {
        // 41e8d0 multiplies the stored f32 by double255 and truncates;
        // preserving that order matters for the setter/query round trip.
        self.0.into_iter().enumerate().fold(0, |packed, (i, channel)| {
            packed | (((f64::from(channel) * 255.).trunc() as u32 & 255) << (i * 8))
        })
    }
}

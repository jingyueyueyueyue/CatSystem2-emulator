//! PE d2e50 sine/cosine kernel for the live manager rotation setter.
//! Independently transcribed arithmetic and stored constants, no engine FFI.
use super::Error;

pub(super) fn sin_cos(angle: f32) -> Result<(f32, f32), Error> {
    let half = f32::from_bits((angle.to_bits() & 0xbf000000) | 0x3f000000);
    let quotient = angle * f32::from_bits(0x3e22f983) + half;
    if !quotient.is_finite() || !(-2147483648.0..2147483648.0).contains(&quotient) {
        return Err(Error::Host(
            "Effekseer angle outside native signed reduction domain",
        ));
    }
    let x = angle - (quotient as i32) as f32 * f32::from_bits(0x40c90fdb);
    let x2 = x * x;
    let x4 = (x2 * x) * x;
    let x6 = (x4 * x) * x;
    let x8 = (x6 * x) * x;
    let x10 = (x8 * x) * x;
    // Preserve the target's separate f32 multiplies/adds and powers. In
    // particular this is not Horner evaluation or platform sinf/cosf.
    let mut s = 1.0 - x2 * f32::from_bits(0x3e2aaaab);
    let mut c = 1.0 - x2 * 0.5;
    s += x4 * f32::from_bits(0x3c088889);
    c += x4 * f32::from_bits(0x3d2aaaab);
    s -= x6 * f32::from_bits(0x39500d01);
    c -= x6 * f32::from_bits(0x3ab60b61);
    s += x8 * f32::from_bits(0x3638ef1d);
    c += x8 * f32::from_bits(0x37d00d01);
    s -= x10 * f32::from_bits(0x32d7322b);
    c -= x10 * f32::from_bits(0x3493f27e);
    let s = s * x;
    if s.is_finite() && c.is_finite() {
        Ok((s, c))
    } else {
        Err(Error::Host(
            "nonfinite native Effekseer trigonometric output",
        ))
    }
}

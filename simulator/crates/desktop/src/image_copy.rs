//! Bounded CPU rectangle receiver recovered independently from the fixed
//! V401/Yukikoi copy helpers. Coordinates and traversal start at the bottom.
use cs2_renderer::RgbaFrame;
use cs2_script::Result;

use super::super::native_state_error;
use crate::plane_native::CopyRect;

#[derive(Clone, Copy)]
pub(super) enum SamplingProfile { V401, Yukikoi }

impl SamplingProfile {
    pub(super) fn for_sha(sha: &str) -> Option<Self> {
        match sha {
            "0e1dde51e86e9152d8e1d27b82efb15916cb9f82d23dd30990212fafd96c8b9f" => Some(Self::V401),
            "63de0e14cd8ce11ad2d68e7ed6076641ff5e5b73d822603cc52764009d812976" => Some(Self::Yukikoi),
            _ => None,
        }
    }
}

fn local(r: CopyRect, added: [i32; 2], frame: &RgbaFrame) -> Option<[u32; 4]> {
    let offset = frame.metadata().offset;
    let x = r.x.wrapping_add(added[0]);
    let y = r.y.wrapping_add(added[1]);
    let left = x.max(offset[0]);
    let bottom = y.max(offset[1]);
    let right = x.wrapping_add(r.width).min(offset[0].wrapping_add(frame.width() as i32));
    let top = y.wrapping_add(r.height).min(offset[1].wrapping_add(frame.height() as i32));
    (left < right && bottom < top).then(|| [left.wrapping_sub(offset[0]) as u32,
        bottom.wrapping_sub(offset[1]) as u32, right.wrapping_sub(left) as u32,
        top.wrapping_sub(bottom) as u32])
}

pub(super) fn copy_wrapper(target: &RgbaFrame, source: &RgbaFrame,
    to: CopyRect, from: CopyRect, alias: bool, profile: SamplingProfile)
    -> Result<Option<RgbaFrame>> {
    if target.is_empty() || source.is_empty() { return Ok(None); }
    let Some(mut src) = local(from, [0; 2], source) else { return Ok(None); };
    let Some(mut dst) = local(to, source.metadata().offset, target) else { return Ok(None); };
    if to.width == from.width && to.height == from.height {
        for axis in 2..4 { src[axis] = src[axis].min(dst[axis]); dst[axis] = src[axis]; }
    }
    let depth = source.metadata().bit_depth;
    let target_depth = target.metadata().bit_depth;
    if !matches!(depth, 24 | 32) || !matches!(target_depth, 24 | 32) {
        return Err(native_state_error("native rectangle copy pixel-format conversion is not recovered"));
    }
    if alias && (source.dimensions() != target.dimensions() || depth != target_depth) {
        return Err(native_state_error("native rectangle copy shared pixel storage disagrees"));
    }
    if depth != target_depth && (src[2] > dst[2].saturating_mul(4)
        || src[3] > dst[3].saturating_mul(4)) {
        match profile {
            // 55efba..55efc7 returns false before writing. The native wrapper
            // ignores that result; do not manufacture converted destination.
            SamplingProfile::V401 => return Ok(None),
            SamplingProfile::Yukikoi if depth == 32 => return Err(native_state_error(
                "native BGRA32-to-BGR24 averaging writes beyond each target pixel")),
            _ => (),
        }
    }
    let mut output = target.pixels().to_vec();
    sample_pixels(source, [target.width(), target.height()], &mut output, src, dst, alias, profile, target_depth)?;
    let frame = RgbaFrame::from_rgba8(target.width(), target.height(), output)
        .map_err(|_| native_state_error("native rectangle copy frame construction failed"))?
        .with_raw_metadata(target.metadata()).with_hg_attributes(target.hg_attributes().clone());
    Ok(Some(frame))
}

fn at(size: [u32; 2], x: u32, y: u32) -> usize {
    ((size[1] - 1 - y) as usize * size[0] as usize + x as usize) * 4
}

fn sample(source: &RgbaFrame, output: &[u8], alias: bool, x: u32, y: u32) -> [u8; 4] {
    let offset = at([source.width(), source.height()], x, y);
    let pixels = if alias { output } else { source.pixels() };
    pixels[offset..offset + 4].try_into().expect("bounded native copy pixel")
}

/// Both rectangles use local bottom-origin XYWH. The whole-image 730 path
/// supplies origin zero; 116/735 retain their independently clipped origins.
pub(super) fn sample_pixels(source: &RgbaFrame, target: [u32; 2], output: &mut [u8],
    src: [u32; 4], dst: [u32; 4], alias: bool, profile: SamplingProfile, target_depth: u8) -> Result<()> {
    let opaque = source.metadata().bit_depth == 24;
    if src[2..] == dst[2..] {
        for row in 0..dst[3] {
            let s = at([source.width(), source.height()], src[0], src[1] + row);
            let d = at(target, dst[0], dst[1] + row);
            let length = dst[2] as usize * 4;
            // The native row mover observes preceding destination rows when
            // storage aliases. Never snapshot all source rows up front.
            if source.metadata().bit_depth != target_depth {
                // Native format table: BGR24->BGRA32 adds FF; the reverse
                // copies three colour bytes and drops source Alpha.
                for x in 0..dst[2] as usize {
                    output[d + x * 4..d + x * 4 + 3]
                        .copy_from_slice(&source.pixels()[s + x * 4..s + x * 4 + 3]);
                    output[d + x * 4 + 3] = 255;
                }
            } else if alias { output.copy_within(s..s + length, d); }
            else { output[d..d + length].copy_from_slice(&source.pixels()[s..s + length]); }
        }
    } else if src[2] > dst[2].saturating_mul(4) || src[3] > dst[3].saturating_mul(4) {
        average(source, target, output, src, dst, alias, profile, opaque, target_depth)?;
    } else {
        interpolate(source, target, output, src, dst, alias, opaque, target_depth);
    }
    Ok(())
}

fn interpolate(source: &RgbaFrame, target: [u32; 2], output: &mut [u8],
    src: [u32; 4], dst: [u32; 4], alias: bool, opaque: bool, target_depth: u8) {
    let dx = (src[2].wrapping_shl(15) as i32) / dst[2] as i32;
    let dy = (src[3].wrapping_shl(15) as i32) / dst[3] as i32;
    // Source origins are added WITHOUT shifting them into fixed point.
    let mut yf = src[1] as i32;
    for y in 0..dst[3] {
        let sy = (yf >> 15) as u32;
        let fy = yf as u32 & 0x7fff;
        let mut xf = src[0] as i32;
        for x in 0..dst[2] {
            let sx = (xf >> 15) as u32;
            if sx < source.width() && sy < source.height() {
                let first = sample(source, output, alias, sx, sy);
                // Bounds refer to the whole source image, not the clipped
                // request. Each absent neighbour independently repeats FIRST.
                let mut p = [first; 4];
                if sx + 1 < source.width() { p[1] = sample(source, output, alias, sx + 1, sy); }
                if sy + 1 < source.height() { p[2] = sample(source, output, alias, sx, sy + 1); }
                if sx + 1 < source.width() && sy + 1 < source.height() {
                    p[3] = sample(source, output, alias, sx + 1, sy + 1);
                }
                let alpha: u32 = if opaque { 1020 } else { p.iter().map(|p| u32::from(p[3])).sum() };
                if alpha != 0 {
                    let fx = xf as u32 & 0x7fff;
                    let weights = [((0x8000 - fx) * (0x8000 - fy)) >> 15,
                        (fx * (0x8000 - fy)) >> 15, ((0x8000 - fx) * fy) >> 15,
                        (fx * fy) >> 15];
                    let d = at(target, dst[0] + x, dst[1] + y);
                    for channel in 0..if target_depth == 24 { 3 } else { 4 } {
                        let sum: u32 = p.iter().zip(weights)
                            .map(|(p, weight)| u32::from(p[channel]) * weight).sum();
                        output[d + channel] = (sum >> 15) as u8;
                    }
                    if target_depth == 24 || alpha == 1020 { output[d + 3] = 255; }
                }
            }
            xf = xf.wrapping_add(dx);
        }
        yf = yf.wrapping_add(dy);
    }
}

fn truncate(value: f32) -> i32 {
    if value.is_finite() && (-2_147_483_648.0..2_147_483_648.0).contains(&f64::from(value)) {
        value.trunc() as i32
    } else { i32::MIN }
}

fn boundary(index: u32, source: u32, destination: u32) -> i32 {
    // Native IMUL retains the low DWORD before signed IDIV.
    (index.wrapping_mul(source) as i32) / destination as i32
}

fn average(source: &RgbaFrame, target: [u32; 2], output: &mut [u8],
    src: [u32; 4], dst: [u32; 4], alias: bool, profile: SamplingProfile,
    opaque: bool, target_depth: u8) -> Result<()> {
    let ratio = [src[2] as f32 / dst[2] as f32, src[3] as f32 / dst[3] as f32];
    let window = ratio.map(|r| truncate(r).max(1) as u32);
    let mut previous_y = 0i32;
    for y in 0..dst[3] {
        let next_y = boundary(y + 1, src[3], dst[3]);
        let (oy, ny) = match profile {
            SamplingProfile::Yukikoi => (truncate(y as f32 * ratio[1]) as u32,
                window[1].min(source.height())),
            // The legacy BGR24 branch includes the far boundary (+1),
            // unlike its BGRA32 branch. Never clamp an unbacked far sample.
            SamplingProfile::V401 => (previous_y as u32,
                next_y.wrapping_sub(previous_y).wrapping_add(i32::from(opaque)).max(1) as u32),
        };
        let mut previous_x = 0i32;
        for x in 0..dst[2] {
            let next_x = boundary(x + 1, src[2], dst[2]);
            let (ox, nx) = match profile {
                SamplingProfile::Yukikoi => (truncate(x as f32 * ratio[0]) as u32,
                    window[0].min(source.width())),
                SamplingProfile::V401 => (previous_x as u32,
                    next_x.wrapping_sub(previous_x).wrapping_add(i32::from(opaque)).max(1) as u32),
            };
            let sx = src[0].checked_add(ox);
            let sy = src[1].checked_add(oy);
            // Yukikoi compares LOCAL window indices to full source bounds;
            // V401 has no bounds guard here. Reject unbacked reads explicitly.
            let (Some(sx), Some(sy)) = (sx, sy) else {
                return Err(native_state_error("native rectangle averaging origin overflows"));
            };
            if sx.checked_add(nx).is_none_or(|end| end > source.width())
                || sy.checked_add(ny).is_none_or(|end| end > source.height()) {
                return Err(native_state_error("native rectangle averaging window exceeds source pixels"));
            }
            let count = nx.checked_mul(ny).filter(|&v| v != 0 && v <= i32::MAX as u32)
                .ok_or_else(|| native_state_error("native rectangle averaging count is invalid"))?;
            let d = at(target, dst[0] + x, dst[1] + y);
            for channel in 0..if opaque { 3 } else { 4 } {
                let mut sum = 0u32;
                for row in 0..ny {
                    for column in 0..nx {
                        sum = sum.wrapping_add(u32::from(sample(source, output, alias,
                            sx + column, sy + row)[channel]));
                    }
                }
                output[d + channel] = match profile {
                    SamplingProfile::Yukikoi => (sum / count) as u8,
                    SamplingProfile::V401 => ((sum as i32) / count as i32) as u8,
                };
            }
            // Yukikoi's 24->32 averaging writes only three source bytes,
            // leaving the destination's existing Alpha untouched.
            if target_depth == 24 { output[d + 3] = 255; }
            previous_x = next_x;
        }
        previous_y = next_y;
    }
    Ok(())
}

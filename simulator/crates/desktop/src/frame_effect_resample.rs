//! Software scaling used by retained mosaic images. Independent recovery
//! from the fixed PE's pixel-format dispatch, not a device-scaling guess.
use anyhow::{Context, Result, bail};
use cs2_renderer::RgbaFrame;

use super::super::check_work;

const ONE: u32 = 1 << 15;

#[derive(Clone, Copy)]
pub(super) enum Profile {
    V401,
    Yukikoi,
}

impl Profile {
    pub(super) fn from_sha(sha: &str) -> Option<Self> {
        match sha {
            super::V401 => Some(Self::V401),
            super::YUKIKOI => Some(Self::Yukikoi),
            _ => None,
        }
    }
}

/// The two reviewed format32 hard-expand receivers share this write order.
/// This is deliberately separate from nearest-neighbour resizing: the
/// vertical middle loop tests block*source_row, not the destination cursor.
pub(super) fn expand_blocks(
    source: &RgbaFrame,
    size: [u32; 2],
    block: u32,
    _profile: Profile,
) -> Result<RgbaFrame> {
    check_work(size)?;
    if block == 0 || size.contains(&0) {
        bail!("hard mosaic expansion has a zero block or destination extent");
    }
    let columns = block_axis(source.width(), size[0], block, false)?;
    let rows = block_axis(source.height(), size[1], block, true)?;
    let mut output = vec![0; size[0] as usize * size[1] as usize * 4];
    for (y, &sy) in rows.iter().enumerate() {
        for (x, &sx) in columns.iter().enumerate() {
            let from = (sy as usize * source.width() as usize + sx as usize) * 4;
            let to = (y * size[0] as usize + x) * 4;
            output[to..to + 3].copy_from_slice(&source.pixels()[from..from + 3]);
            // Both A8R8G8B8 and X8R8G8B8 format32 branches OR 0xff000000.
            output[to + 3] = 255;
        }
    }
    Ok(RgbaFrame::from_rgba8(size[0], size[1], output)?)
}

fn block_axis(source: u32, destination: u32, block: u32, vertical: bool) -> Result<Vec<u32>> {
    if source == 0 {
        bail!("hard mosaic expansion has an empty source");
    }
    let mut samples = vec![0; (block / 2).min(destination) as usize];
    // The source pointer advances once before this loop, even when the
    // source has only one pixel. Never replace its unbacked tail by a clamp.
    for sample in 1..source.saturating_sub(1) {
        let remaining = destination as usize - samples.len();
        if remaining == 0 {
            break;
        }
        let count = if vertical {
            let nominal = sample.checked_mul(block)
                .context("hard mosaic row guard overflows its DWORD extent")?;
            block.min(destination.saturating_sub(nominal))
        } else {
            block
        };
        let count = (count as usize).min(remaining);
        samples.resize(samples.len() + count, sample);
        if vertical && count == 0 {
            // Later nominal rows cannot write either; the source pointer
            // still reaches the last source row before the tail loop.
            break;
        }
    }
    if samples.len() < destination as usize {
        let tail = source.saturating_sub(1).max(1);
        if tail >= source {
            bail!("hard mosaic tail reads beyond its retained source extent");
        }
        samples.resize(destination as usize, tail);
    }
    Ok(samples)
}

pub(super) fn resize(
    source: &RgbaFrame,
    size: [u32; 2],
    previous: Option<&RgbaFrame>,
    profile: Profile,
) -> Result<RgbaFrame> {
    check_work(size)?;
    let from = [source.width(), source.height()];
    if from.contains(&0) || size.contains(&0) {
        bail!("frame effect resampling has a zero extent");
    }
    if from == size {
        // Equal dimensions select the format4->4 row-copy receiver.
        return Ok(RgbaFrame::from_rgba8(size[0], size[1], source.pixels().to_vec())?);
    }
    let length = size[0] as usize * size[1] as usize * 4;
    let mut output = match previous {
        Some(previous) => {
            if [previous.width(), previous.height()] != size {
                bail!("frame effect previous resampling target has different dimensions");
            }
            previous.pixels().to_vec()
        }
        None => vec![0; length],
    };
    // The original dispatcher selects the box path when either source axis
    // exceeds four times its destination. All other unequal sizes share the
    // fixed-point sampler, including mixed-axis resize and enlargement.
    if (0..2).any(|axis| u64::from(from[axis]) > u64::from(size[axis]) * 4) {
        match profile {
            Profile::V401 => box_scale_v401(source, size, &mut output)?,
            Profile::Yukikoi => box_scale_yukikoi(source, size, &mut output)?,
        }
    } else {
        bilinear(source, size, &mut output)?;
    }
    Ok(RgbaFrame::from_rgba8(size[0], size[1], output)?)
}

fn pixel(source: &RgbaFrame, x: u32, y: u32) -> [u8; 4] {
    let at = (y as usize * source.width() as usize + x as usize) * 4;
    source.pixels()[at..at + 4].try_into().unwrap()
}

fn bilinear(source: &RgbaFrame, size: [u32; 2], output: &mut [u8]) -> Result<()> {
    let from = [source.width(), source.height()];
    let step: [u32; 2] = std::array::from_fn(|axis| {
        ((u64::from(from[axis]) << 15) / u64::from(size[axis])) as u32
    });
    // Native signed SHL/IDIV must have a positive numerator. Report an
    // unsupported arithmetic extent instead of silently widening its ABI.
    if from.iter().any(|&value| value > (i32::MAX as u32 >> 15)) {
        bail!("frame effect resampling source exceeds its signed fixed-point extent");
    }
    for y in 0..size[1] {
        let sy = y.checked_mul(step[1]).context("frame effect vertical sample overflow")?;
        for x in 0..size[0] {
            let sx = x.checked_mul(step[0]).context("frame effect horizontal sample overflow")?;
            let [ix, iy] = [sx >> 15, sy >> 15];
            if ix >= from[0] || iy >= from[1] {
                bail!("frame effect fixed-point sample is outside its source");
            }
            let first = pixel(source, ix, iy);
            // The fourth sample deliberately repeats first when either axis
            // reaches its last texel; it is not independently clamped XY.
            let samples = [
                first,
                if ix + 1 < from[0] { pixel(source, ix + 1, iy) } else { first },
                if iy + 1 < from[1] { pixel(source, ix, iy + 1) } else { first },
                if ix + 1 < from[0] && iy + 1 < from[1] {
                    pixel(source, ix + 1, iy + 1)
                } else { first },
            ];
            let alpha_sum: u32 = samples.iter().map(|sample| u32::from(sample[3])).sum();
            if alpha_sum == 0 {
                // This leaves the actual retained destination unchanged.
                continue;
            }
            let [fx, fy] = [sx & (ONE - 1), sy & (ONE - 1)];
            // Every product truncates before the channel sum, then the sum
            // truncates again. A floating interpolation changes low bits.
            let weights = [
                ((ONE - fx) * (ONE - fy)) >> 15,
                (fx * (ONE - fy)) >> 15,
                ((ONE - fx) * fy) >> 15,
                (fx * fy) >> 15,
            ];
            let at = (y as usize * size[0] as usize + x as usize) * 4;
            for channel in 0..4 {
                output[at + channel] = if channel == 3 && alpha_sum == 4 * 255 {
                    255
                } else {
                    (samples.iter().zip(weights).map(|(sample, weight)| {
                        u32::from(sample[channel]) * weight
                    }).sum::<u32>() >> 15) as u8
                };
            }
        }
    }
    Ok(())
}

fn box_scale_v401(source: &RgbaFrame, size: [u32; 2], output: &mut [u8]) -> Result<()> {
    let from = [source.width(), source.height()];
    let mut top = 0;
    for y in 0..size[1] {
        let bottom = signed_boundary(y + 1, from[1], size[1])?;
        let height = bottom.saturating_sub(top).max(1);
        let mut left = 0;
        for x in 0..size[0] {
            let right = signed_boundary(x + 1, from[0], size[0])?;
            let width = right.saturating_sub(left).max(1);
            let count = width.checked_mul(height).filter(|&value| value <= i32::MAX as u32)
                .context("frame effect box sample count exceeds signed division")?;
            if left + width > from[0] || top + height > from[1] {
                bail!("frame effect box sample is outside its source");
            }
            let mut sums = [0u32; 4];
            for sy in top..top + height {
                for sx in left..left + width {
                    for (sum, byte) in sums.iter_mut().zip(pixel(source, sx, sy)) {
                        *sum = sum.wrapping_add(u32::from(byte));
                    }
                }
            }
            let at = (y as usize * size[0] as usize + x as usize) * 4;
            for channel in 0..4 {
                output[at + channel] = ((sums[channel] as i32) / count as i32) as u8;
            }
            left = right;
        }
        top = bottom;
    }
    Ok(())
}

fn signed_boundary(index: u32, source: u32, destination: u32) -> Result<u32> {
    let numerator = index.checked_mul(source).filter(|&value| value <= i32::MAX as u32)
        .context("legacy frame effect box boundary exceeds signed arithmetic")?;
    Ok(numerator / destination)
}

fn box_scale_yukikoi(source: &RgbaFrame, size: [u32; 2], output: &mut [u8]) -> Result<()> {
    let from = [source.width(), source.height()];
    // SSE CVTDQ2PS/DIVSS, then CVTTSS2SI: unlike the older receiver,
    // this uses a fixed truncated kernel and separately rounded origins.
    let ratio: [f32; 2] = std::array::from_fn(|axis| from[axis] as f32 / size[axis] as f32);
    let kernel: [u32; 2] = std::array::from_fn(|axis| (ratio[axis].trunc() as u32).max(1));
    let count = kernel[0].checked_mul(kernel[1])
        .context("frame effect box sample count overflow")?;
    for y in 0..size[1] {
        let top = (y as f32 * ratio[1]).trunc() as u32;
        for x in 0..size[0] {
            let left = (x as f32 * ratio[0]).trunc() as u32;
            if left.checked_add(kernel[0]).is_none_or(|end| end > from[0])
                || top.checked_add(kernel[1]).is_none_or(|end| end > from[1])
            {
                bail!("current frame effect box sample is outside its source");
            }
            let mut sums = [0u32; 4];
            for sy in top..top + kernel[1] {
                for sx in left..left + kernel[0] {
                    for (sum, byte) in sums.iter_mut().zip(pixel(source, sx, sy)) {
                        *sum = sum.wrapping_add(u32::from(byte));
                    }
                }
            }
            let at = (y as usize * size[0] as usize + x as usize) * 4;
            for channel in 0..4 {
                // Current software receiver uses unsigned DIV, not IDIV.
                output[at + channel] = (sums[channel] / count) as u8;
            }
        }
    }
    Ok(())
}

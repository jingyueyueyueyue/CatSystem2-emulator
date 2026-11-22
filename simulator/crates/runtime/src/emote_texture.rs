//! Bounded PSB texture conversion, independently derived from format facts and
//! driver's common-platform converters545d0/546d0/547e0. No SDK/DLL loading.

use super::{EmoteError, Result, cpu::decode_rgba_rle_with_order};

const MAX_TEXTURE_BYTES: usize = 128 * 1024 * 1024;

/// Wrapper134 is read by1778f0 only while175900 rebuilds the driver.
/// The original callbacks receive BGRA; the decoded storage here is RGBA.
/// Each luminance contribution truncates separately through a byte table.
pub(super) fn apply_m2_texture_filter(rgba: &mut [u8], raw: u32) {
    if !matches!(raw, 1..=3) {
        return;
    }
    let red = f32::from_bits(0x3e990afe);
    let green = f32::from_bits(0x3f162c23);
    let blue = f32::from_bits(0x3dea7371);
    for pixel in rgba.chunks_exact_mut(4) {
        if raw == 3 {
            //196ca0 leaves the fourth byte intact.
            pixel[0] = 255 - pixel[0];
            pixel[1] = 255 - pixel[1];
            pixel[2] = 255 - pixel[2];
            continue;
        }
        let gray = ((f32::from(pixel[2]) * blue) as u8)
            .wrapping_add((f32::from(pixel[0]) * red) as u8)
            .wrapping_add((f32::from(pixel[1]) * green) as u8);
        if raw == 1 {
            pixel[..3].fill(gray);
        } else {
            //196ce0: B=max(gray-35,0),G=min(gray+5,255),R=min(gray+30,255).
            pixel[0] = gray.saturating_add(30);
            pixel[1] = gray.saturating_add(5);
            pixel[2] = gray.saturating_sub(35);
        }
    }
}

/// Returns straight R,G,B,A. Win RGBA8 retains its documented BGRA byte order;
/// common16-bit layouts follow the original software-conversion bit fields.
/// Unknown compression/platform/layout combinations fail explicitly.
pub fn decode_emote_texture(
    kind: &str,
    platform: &str,
    width: usize,
    height: usize,
    bytes: &[u8],
) -> Result<Vec<u8>> {
    let pixels = width
        .checked_mul(height)
        .filter(|&n| n != 0)
        .ok_or(EmoteError::Capacity)?;
    let size = pixels
        .checked_mul(4)
        .filter(|&n| n <= MAX_TEXTURE_BYTES)
        .ok_or(EmoteError::Capacity)?;
    if !matches!(platform, "win" | "common") {
        return Err(EmoteError::InvalidResource("unrecovered texture platform"));
    }
    if kind == "RGBA8" {
        if bytes.len() != size {
            return decode_rgba_rle_with_order(bytes, size, platform == "win");
        }
        if platform == "win" {
            let rgba: Vec<[u8; 4]> = bytes.chunks_exact(4)
                .map(|pixel| [pixel[2], pixel[1], pixel[0], pixel[3]])
                .collect();
            return Ok(rgba.into_flattened());
        }
        return Ok(bytes.to_vec());
    }
    if platform != "common" || !matches!(kind, "RGBA5650" | "RGBA5551" | "RGBA4444") {
        return Err(EmoteError::Backend(format!(
            "texture format/platform {kind}/{platform} is not recovered"
        )));
    }
    if bytes.len() != pixels.checked_mul(2).ok_or(EmoteError::Capacity)? {
        return Err(EmoteError::InvalidResource(
            "16-bit texture requires exact raw bytes; compressed layout is not recovered",
        ));
    }
    let mut rgba = Vec::new();
    rgba.try_reserve_exact(size)
        .map_err(|_| EmoteError::Capacity)?;
    let five = |n: u16| -> u8 { ((n << 3) | (n >> 2)) as u8 };
    let six = |n: u16| -> u8 { ((n << 2) | (n >> 4)) as u8 };
    let four = |n: u16| -> u8 { ((n << 4) | n) as u8 };
    for pair in bytes.chunks_exact(2) {
        let word = u16::from_le_bytes([pair[0], pair[1]]);
        let pixel = match kind {
            "RGBA5650" => [
                five((word >> 11) & 31),
                six((word >> 5) & 63),
                five(word & 31),
                255,
            ],
            "RGBA5551" => [
                five((word >> 11) & 31),
                five((word >> 6) & 31),
                five((word >> 1) & 31),
                if word & 1 == 0 { 0 } else { 255 },
            ],
            "RGBA4444" => [
                four((word >> 12) & 15),
                four((word >> 8) & 15),
                four((word >> 4) & 15),
                four(word & 15),
            ],
            _ => unreachable!(),
        };
        rgba.extend_from_slice(&pixel);
    }
    Ok(rgba)
}

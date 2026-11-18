//! The canonical Windows BMP subset read by ordinary image native 364.
//!
//! The original decoder starts pixels at byte 54 even for other headers.
//! Accept only BITMAPINFOHEADER, BI_RGB and that canonical offset; unsupported
//! layouts are diagnosed instead of passing their headers off as pixel bytes.
use crate::util::{MAX_BUFFER, range, u32_at};
use thiserror::Error;

const MAX_PIXELS: usize = 128 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BitmapImage {
    pub width: u32,
    pub height: u32,
    /// Original file bit depth. Decoded storage is always RGBA8.
    pub bit_depth: u16,
    /// Top-down, tightly packed RGBA8; 32-bit input retains raw Alpha.
    pub pixels: Vec<u8>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum BitmapError {
    #[error("invalid BMP signature")]
    InvalidSignature,
    #[error("BMP header or pixel data is truncated")]
    Truncated,
    #[error("BMP input exceeds 64 MiB")]
    TooLarge,
    #[error("BMP layout is not the native canonical 24/32-bit BI_RGB subset")]
    UnsupportedLayout,
    #[error("BMP dimensions are invalid")]
    InvalidDimensions,
    #[error("BMP RGBA pixels exceed 128 MiB")]
    PixelOutputTooLarge,
    #[error("BMP RGBA allocation failed")]
    AllocationFailed,
}

impl BitmapImage {
    /// Decode ordinary 364's canonical BMP subset. The image has no frames;
    /// native 364 deliberately ignores its requested frame ID for this type.
    pub fn decode(data: &[u8]) -> Result<Self, BitmapError> {
        if data.len() > MAX_BUFFER { return Err(BitmapError::TooLarge); }
        if data.get(..2) != Some(b"BM") { return Err(BitmapError::InvalidSignature); }
        let header = range(data, 0, 54).ok_or(BitmapError::Truncated)?;
        let word = |at: usize| u16::from_le_bytes([header[at], header[at + 1]]);
        if u32_at(header, 10) != Some(54)
            || u32_at(header, 14) != Some(40)
            || word(26) != 1
            || !matches!(word(28), 24 | 32)
            || u32_at(header, 30) != Some(0)
        {
            return Err(BitmapError::UnsupportedLayout);
        }
        let width = u32_at(header, 18).ok_or(BitmapError::Truncated)?;
        let height = u32_at(header, 22).ok_or(BitmapError::Truncated)?;
        if width == 0 || height == 0 || width > i32::MAX as u32 || height > i32::MAX as u32 {
            // The PE treats negative heights as enormous unsigned images.
            // Do not pretend to implement top-down BMP through that path.
            return Err(BitmapError::InvalidDimensions);
        }
        let bit_depth = word(28);
        let depth = usize::from(bit_depth / 8);
        let length = (width as usize).checked_mul(height as usize)
            .and_then(|pixels| pixels.checked_mul(4))
            .filter(|&length| length <= MAX_PIXELS)
            .ok_or(BitmapError::PixelOutputTooLarge)?;
        let stride = (width as usize).checked_mul(depth)
            .and_then(|row| row.checked_add(3)).map(|row| row & !3)
            .ok_or(BitmapError::PixelOutputTooLarge)?;
        let source_length = stride.checked_mul(height as usize)
            .ok_or(BitmapError::PixelOutputTooLarge)?;
        let source = range(data, 54, source_length).ok_or(BitmapError::Truncated)?;
        let mut pixels = Vec::new();
        pixels.try_reserve_exact(length).map_err(|_| BitmapError::AllocationFailed)?;
        pixels.resize(length, 0);
        for y in 0..height as usize {
            let input = &source[(height as usize - y - 1) * stride..][..width as usize * depth];
            let output = &mut pixels[y * width as usize * 4..][..width as usize * 4];
            for (input, output) in input.chunks_exact(depth).zip(output.chunks_exact_mut(4)) {
                output.copy_from_slice(&[
                    input[2], input[1], input[0], if depth == 4 { input[3] } else { 255 },
                ]);
            }
        }
        Ok(Self { width, height, bit_depth, pixels })
    }
}

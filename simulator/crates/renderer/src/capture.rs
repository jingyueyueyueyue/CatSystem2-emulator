//! Opaque capture of the completed logical surface. The source is canonical
//! top-down RGBA; a positive-height BI_RGB file stores its rows bottom-up.
use super::{Canvas, RendererError, Result};

/// Ordinary441's image writer uses 24-bit BGR, four-byte scanline alignment,
/// a 54-byte header, and an eight-byte-aligned output allocation. bfSize does
/// not include that final allocation padding. Capture readback has already
/// forced every alpha byte to 255, so flattening onto white preserves RGB.
pub fn encode_capture_bmp(canvas: &Canvas) -> Result<Vec<u8>> {
    let (width, height) = canvas.dimensions();
    if width == 0 || height == 0 || width > i32::MAX as u32 || height > i32::MAX as u32 {
        return Err(RendererError::InvalidDimensions { width, height });
    }
    let stride = (width as usize).checked_mul(3)
        .and_then(|value| value.checked_add(3)).map(|value| value & !3)
        .ok_or(RendererError::ScaledDimensionsOverflow)?;
    let image_size = stride.checked_mul(height as usize)
        .ok_or(RendererError::ScaledDimensionsOverflow)?;
    let file_size = image_size.checked_add(54)
        .filter(|&value| value <= u32::MAX as usize)
        .ok_or(RendererError::ScaledDimensionsOverflow)?;
    let allocation = file_size.checked_add(7).map(|value| value & !7)
        .ok_or(RendererError::ScaledDimensionsOverflow)?;
    let rgba = canvas.try_pixels()?;
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(allocation)
        .map_err(|_| RendererError::AllocationFailed { bytes: allocation })?;
    bytes.resize(allocation, 0);
    bytes[..2].copy_from_slice(b"BM");
    bytes[2..6].copy_from_slice(&(file_size as u32).to_le_bytes());
    bytes[10..14].copy_from_slice(&54u32.to_le_bytes());
    bytes[14..18].copy_from_slice(&40u32.to_le_bytes());
    bytes[18..22].copy_from_slice(&width.to_le_bytes());
    bytes[22..26].copy_from_slice(&height.to_le_bytes());
    bytes[26..28].copy_from_slice(&1u16.to_le_bytes());
    bytes[28..30].copy_from_slice(&24u16.to_le_bytes());
    bytes[34..38].copy_from_slice(&(image_size as u32).to_le_bytes());
    let source_stride = width as usize * 4;
    for (file_y, source_y) in (0..height as usize).rev().enumerate() {
        let source = &rgba[source_y * source_stride..(source_y + 1) * source_stride];
        let target = &mut bytes[54 + file_y * stride..54 + file_y * stride + width as usize * 3];
        for (rgba, bgr) in source.chunks_exact(4).zip(target.chunks_exact_mut(3)) {
            bgr.copy_from_slice(&[rgba[2], rgba[1], rgba[0]]);
        }
    }
    Ok(bytes)
}

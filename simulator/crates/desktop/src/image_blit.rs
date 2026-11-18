//! Ordinary 115: bottom-origin image mixing and top-origin canvas copying.
//! Implemented from docs/graphics.md and this workspace's independent integer
//! pixel arithmetic. It does not use a generic normalized SourceOver formula
//! or copy a reference engine's implementation.
use cs2_renderer::RgbaFrame;
use cs2_script::{NativeReturn, Result};

use super::{LoggingHost, native_state_error, read_u32};

#[derive(Clone, Copy)]
struct Request {
    at: [i32; 2],
    source: RawRect,
}

#[derive(Clone, Copy)]
struct RawRect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

impl RawRect {
    fn xywh(x: i32, y: i32, width: i32, height: i32) -> Self {
        Self { left: x, top: y, right: x.wrapping_add(width), bottom: y.wrapping_add(height) }
    }

    fn bounds(width: u32, height: u32) -> Result<Self> {
        Ok(Self {
            left: 0, top: 0,
            right: i32::try_from(width).map_err(|_| native_state_error(
                "native 115 image width exceeds signed coordinates",
            ))?,
            bottom: i32::try_from(height).map_err(|_| native_state_error(
                "native 115 image height exceeds signed coordinates",
            ))?,
        })
    }

    fn intersect(self, other: Self) -> Option<Self> {
        let clipped = Self {
            left: self.left.max(other.left), top: self.top.max(other.top),
            right: self.right.min(other.right), bottom: self.bottom.min(other.bottom),
        };
        (clipped.left < clipped.right && clipped.top < clipped.bottom).then_some(clipped)
    }
}

impl LoggingHost<'_> {
    /// Main integration: `115 => self.image_blit_native(arguments)`.
    /// Source zero requires only six DWORDs and never resolves the target.
    pub(super) fn image_blit_native(&mut self, arguments: &[u8]) -> Result<NativeReturn> {
        let source_id = read_u32(arguments, 20)?;
        if source_id == 0 {
            return Ok(NativeReturn::Continue);
        }
        let target_id = read_u32(arguments, 0)?;
        let at = [read_u32(arguments, 4)? as i32, read_u32(arguments, 8)? as i32];
        // These two ABI fields are deliberately consumed but do not scale.
        let _requested_size = [read_u32(arguments, 12)?, read_u32(arguments, 16)?];
        let request = Request {
            at,
            source: RawRect::xywh(
                read_u32(arguments, 24)? as i32, read_u32(arguments, 28)? as i32,
                read_u32(arguments, 32)? as i32, read_u32(arguments, 36)? as i32,
            ),
        };
        let source = self.image_handles.get(&source_id).cloned()
            .ok_or_else(|| native_state_error("native 115 invalid source image wrapper"))?;
        if target_id == 0 {
            if source.is_empty() { return Ok(NativeReturn::Continue); }
            self.copy_image_to_graphics_canvas(&source, request.at, [
                request.source.left, request.source.top,
                request.source.right.wrapping_sub(request.source.left),
                request.source.bottom.wrapping_sub(request.source.top),
            ]).map_err(|_| native_state_error("native 115 canvas copy failed"))?;
            return Ok(NativeReturn::Continue);
        }
        let target = self.image_handles.get(&target_id)
            .ok_or_else(|| native_state_error("native 115 invalid target image wrapper"))?;
        let shared = target_id == source_id || match (
            self.data_services.image_wrapper_pixels.get(&target_id),
            self.data_services.image_wrapper_pixels.get(&source_id),
        ) {
            (Some(target), Some(source)) => target == source,
            _ => false,
        };
        if shared && target.dimensions() != source.dimensions() {
            return Err(native_state_error("native 115 shared pixel dimensions disagree"));
        }
        if let Some(frame) = blend_images(target, &source, request, shared)? {
            // The canonical shared-pixel service updates every live alias.
            self.replace_image_wrapper_pixels(target_id, frame)?;
        }
        Ok(NativeReturn::Continue)
    }
}

fn blend_images(
    target: &RgbaFrame, source: &RgbaFrame, request: Request, shared: bool,
) -> Result<Option<RgbaFrame>> {
    if target.is_empty() || source.is_empty() { return Ok(None); }
    let source_bounds = RawRect::bounds(source.width(), source.height())?;
    let target_bounds = RawRect::bounds(target.width(), target.height())?;
    let Some(from) = request.source.intersect(source_bounds) else { return Ok(None) };
    let offset = source.metadata().offset;
    let at = [request.at[0].wrapping_add(offset[0]), request.at[1].wrapping_add(offset[1])];
    let to = RawRect::xywh(
        at[0], at[1],
        request.source.right.wrapping_sub(request.source.left),
        request.source.bottom.wrapping_sub(request.source.top),
    );
    let Some(to) = to.intersect(target_bounds) else { return Ok(None) };
    // Clip both ends independently. A clipped left/bottom edge never moves
    // the opposite end's first pixel.
    let width = (from.right - from.left).min(to.right - to.left);
    let height = (from.bottom - from.top).min(to.bottom - to.top);
    let mut pixels = target.pixels().to_vec();
    for row in 0..height {
        let source_y = from.top + row;
        let target_y = to.top + row;
        let first = source_pixel(&pixels, source, shared, from.left, source_y);
        if to.left > 0 {
            let left = bottom_offset(target.width(), target.height(), to.left - 1, target_y);
            seed_rgb(&mut pixels[left..left + 4], first);
        }
        for column in 0..width {
            // In the shared case each source read sees all earlier writes,
            // including horizontal feedback and preceding physical rows.
            let input = source_pixel(&pixels, source, shared, from.left + column, source_y);
            let output = bottom_offset(target.width(), target.height(), to.left + column, target_y);
            let destination = &mut pixels[output..output + 4];
            blend_pixel_integer(destination, input);
        }
        if to.left + width < target_bounds.right {
            // Re-read after this row's writes, which matters for shared pixels.
            let last = source_pixel(&pixels, source, shared, from.left + width - 1, source_y);
            let right = bottom_offset(target.width(), target.height(), to.left + width, target_y);
            seed_rgb(&mut pixels[right..right + 4], last);
        }
    }
    let frame = RgbaFrame::from_rgba8(target.width(), target.height(), pixels)
        .map_err(|_| native_state_error("native 115 target image rebuild failed"))?
        .with_raw_metadata(target.metadata())
        .with_hg_attributes(target.hg_attributes().clone());
    Ok(Some(frame))
}

fn source_pixel(
    target: &[u8], source: &RgbaFrame, shared: bool, x: i32, y: i32,
) -> [u8; 4] {
    let offset = bottom_offset(source.width(), source.height(), x, y);
    let pixels = if shared { target } else { source.pixels() };
    pixels[offset..offset + 4].try_into().expect("clipped source pixel is four bytes")
}

fn bottom_offset(width: u32, height: u32, x: i32, y: i32) -> usize {
    ((height as usize - 1 - y as usize) * width as usize + x as usize) * 4
}

pub(super) fn seed_rgb(target: &mut [u8], source: [u8; 4]) {
    if target.iter().all(|&byte| byte == 0) {
        target[..3].copy_from_slice(&source[..3]);
    }
}

/// Opacity is fixed at 255 by native 115, so
/// `(255 * (source_alpha + 1)) >> 8` equals source_alpha for every byte.
/// Keep the separate weights, forced low bit and truncated reciprocal: replacing
/// them with normalized straight-alpha division changes low-alpha colors.
/// Static PE evidence: YukikoiMelt 2.6.1.90, 0x5df6b5..0x5df760;
/// binary identity and dispatch provenance are recorded in image_blit_B_20261117.md.
pub(super) fn blend_pixel_integer(target: &mut [u8], source: [u8; 4]) {
    if target.iter().all(|&byte| byte == 0) {
        target.copy_from_slice(&source);
        return;
    }
    let alpha = u32::from(source[3]);
    if alpha == 0 {
        return;
    }
    let destination_alpha = u32::from(target[3]);
    if alpha == 255 || destination_alpha == 0 {
        target.copy_from_slice(&source);
        return;
    }
    if destination_alpha == 255 {
        for (output, input) in target[..3].iter_mut().zip(&source[..3]) {
            let difference = i32::from(*input) - i32::from(*output);
            *output = (i32::from(*output) + ((difference * alpha as i32) >> 8)) as u8;
        }
        return;
    }
    let overlap = (alpha * destination_alpha) >> 8;
    let target_weight = ((256 - alpha) * destination_alpha) >> 8;
    let source_weight = (((256 - destination_alpha) * alpha) >> 8) + overlap;
    let output_alpha = (source_weight + target_weight) | 1;
    let reciprocal = 65_536 / output_alpha;
    for (output, input) in target[..3].iter_mut().zip(&source[..3]) {
        let weighted = u32::from(*input) * source_weight + u32::from(*output) * target_weight;
        *output = ((weighted * reciprocal) >> 16).min(255) as u8;
    }
    target[3] = output_alpha as u8;
}


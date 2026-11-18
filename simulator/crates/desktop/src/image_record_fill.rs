//! Fixed Yukikoi 736: mutate the current resource pixels, not its layer plan.
//! Independent recovery from 5a93f0 -> 5e42b0 -> 5de170 and its format table.
use cs2_renderer::RgbaFrame;
use cs2_script::{NativeReturn, Result};

use crate::{LoggingHost, native_state_error, read_u32};

impl LoggingHost<'_> {
    /// Call before the old 736 branch. None leaves an unreviewed PE's route
    /// intact; no shared updated flag or list geometry is changed here.
    pub(crate) fn image_record_fill_native736(&mut self, args: &[u8])
        -> Result<Option<NativeReturn>> {
        if self.data_services.native_profile.map(|p| p.pe_sha256) != Some(
            "63de0e14cd8ce11ad2d68e7ed6076641ff5e5b73d822603cc52764009d812976") {
            return Ok(None);
        }
        if args.len() != 24 {
            return Err(native_state_error("native 736 requires exactly 24 argument bytes"));
        }
        let handle = read_u32(args, 0)?;
        let rectangle = [read_u32(args, 4)? as i32, read_u32(args, 8)? as i32,
            read_u32(args, 12)? as i32, read_u32(args, 16)? as i32];
        if handle == 0 {
            // 5a9426 forwards only XYWH to the persistent tile clear path.
            // Color is not read, no canvas is a no-op, and no alpha blend runs.
            self.clear_graphics_canvas_rect(rectangle)
                .map_err(|_| native_state_error("native 736 persistent canvas clear failed"))?;
            return Ok(Some(NativeReturn::Continue));
        }
        let current = self.image_resources.get(&handle)
            .ok_or_else(|| native_state_error("native 736 image resource is invalid"))?.image.clone();
        if let Some(frame) = fill_current(&current, rectangle, read_u32(args, 20)?)? {
            self.image_resources.get_mut(&handle).unwrap().image = frame.clone();
            // The sole static source aliases current pixels. A composite or
            // ANM source remains separate and must not inherit this edit.
            self.data_services.image_manager.synchronize_single_static_source(handle, &frame);
            self.synchronize_list_resource_pixels_only(handle, &frame)
                .map_err(|_| native_state_error("native 736 held pixel synchronization failed"))?;
        }
        Ok(Some(NativeReturn::Continue))
    }
}

fn fill_current(frame: &RgbaFrame, [x, y, width, height]: [i32; 4], argb: u32)
    -> Result<Option<RgbaFrame>> {
    if frame.is_empty() { return Ok(None); }
    // IntersectRect clips local bottom-origin pixels. The raw helper never
    // adds the HG offset or rebases the resource's cached source geometry.
    let left = x.max(0);
    let bottom = y.max(0);
    let right = x.wrapping_add(width).min(frame.width() as i32);
    let top = y.wrapping_add(height).min(frame.height() as i32);
    if left >= right || bottom >= top { return Ok(None); }
    let depth = frame.metadata().bit_depth;
    if !matches!(depth, 24 | 32) {
        return Err(native_state_error("native 736 pixel format requires its unrecovered fill kernel"));
    }
    let [red, green, blue, alpha] = [(argb >> 16) as u8, (argb >> 8) as u8,
        argb as u8, (argb >> 24) as u8];
    if depth == 24 && alpha == 0 { return Ok(None); }
    let mut pixels = frame.pixels().to_vec();
    for local_y in bottom..top {
        let row = frame.height() as usize - 1 - local_y as usize;
        for local_x in left..right {
            let offset = (row * frame.width() as usize + local_x as usize) * 4;
            let pixel = &mut pixels[offset..offset + 4];
            if depth == 32 {
                // 5e0d20 stores the complete DWORD, including zero alpha.
                pixel.copy_from_slice(&[red, green, blue, alpha]);
            } else if alpha == 255 {
                // 5e0e80 writes R,G,B in native BGR storage, hence the
                // reversed red/blue in the host's RGBA representation.
                pixel.copy_from_slice(&[blue, green, red, 255]);
            } else {
                // 5e0f03..5e0f3d multiplies EACH channel delta by that
                // channel, not by alpha; SAR and byte stores are retained.
                for (target, channel) in pixel[..3].iter_mut().zip([red, green, blue]) {
                    let old = i32::from(*target);
                    let source = i32::from(channel);
                    *target = (old + (((source - old) * source) >> 8)) as u8;
                }
                pixel[3] = 255;
            }
        }
    }
    RgbaFrame::from_rgba8_with_metadata(frame.width(), frame.height(), pixels, frame.metadata())
        .map(|image| Some(image.with_raw_metadata(frame.metadata())
            .with_hg_attributes(frame.hg_attributes().clone())))
        .map_err(|_| native_state_error("native 736 filled image dimensions are invalid"))
}

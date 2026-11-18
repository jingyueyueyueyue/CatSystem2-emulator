//! Native468's literal right-angle pixel/geometry transforms and 564's raw
//! image control DWORD. Geometry follows the PE's bottom-origin formulas.
use cs2_renderer::{HgFrameAttributes, RgbaFrame};
use cs2_script::{NativeReturn, Result};
use super::{LoggingHost, native_state_error, read_u32};

impl LoggingHost<'_> {
    pub(super) fn image_transform_native(&mut self, index: u32, args: &[u8]) -> Result<NativeReturn> {
        let handle = read_u32(args, 0)?;
        if handle == 0 { return Ok(NativeReturn::Continue); }
        let value = read_u32(args, 4)?;
        let image = self.image_handles.get(&handle)
            .ok_or_else(|| native_state_error("image transform has an invalid/wrong-type wrapper"))?;
        if index == 564 {
            let pixel = self.data_services.image_wrapper_pixels.get(&handle).copied();
            let aliases = self.image_handles.keys().copied().filter(|&alias| alias == handle
                || pixel.is_some_and(|pixel| self.data_services.image_wrapper_pixels.get(&alias) == Some(&pixel)))
                .collect::<Vec<_>>();
            for alias in aliases { self.data_services.image_control_values.insert(alias, value); }
            return Ok(NativeReturn::Continue);
        }
        if value == 0 || !matches!(value, 90 | 180 | 270) { return Ok(NativeReturn::Continue); }
        let frame = rotate_frame(image, value)?;
        self.replace_image_wrapper_pixels(handle, frame)?;
        Ok(NativeReturn::Continue)
    }

    pub(super) fn image_control_value(&self, handle: u32) -> Result<u32> {
        if !self.image_handles.contains_key(&handle) {
            return Err(native_state_error("image control query has an invalid wrapper"));
        }
        Ok(self.data_services.image_control_values.get(&handle).copied().unwrap_or(0))
    }
}

pub(super) fn rotate_frame(image: &RgbaFrame, value: u32) -> Result<RgbaFrame> {
        if !matches!(value, 90 | 180 | 270) { return Ok(image.clone()); }
        let [width, height] = [image.width(), image.height()];
        let [output_width, output_height] = if value == 180 { [width, height] } else { [height, width] };
        // The original sets an unaligned 24-bit row stride to literal FOUR,
        // potentially allocating less than its copy loop writes. Do not
        // manufacture a conventional padded interpretation for that branch.
        if !image.is_empty() && image.metadata().bit_depth == 24 && output_width.wrapping_mul(3) % 4 != 0 {
            return Err(native_state_error("native 468 original unaligned BGR24 stride is unsafe/unrecovered"));
        }
        let length = (output_width as usize).checked_mul(output_height as usize)
            .and_then(|length| length.checked_mul(4)).filter(|&length| length <= 128 * 1024 * 1024)
            .ok_or_else(|| native_state_error("native 468 transformed pixels exceed 128 MiB"))?;
        let mut pixels = vec![0; length];
        for y in 0..height {
            for x in 0..width {
                let [tx, ty] = match value {
                    90 => [y, width - 1 - x],
                    180 => [width - 1 - x, height - 1 - y],
                    270 => [height - 1 - y, x],
                    _ => unreachable!(),
                };
                let source = (y as usize * width as usize + x as usize) * 4;
                let target = (ty as usize * output_width as usize + tx as usize) * 4;
                pixels[target..target + 4].copy_from_slice(&image.pixels()[source..source + 4]);
            }
        }
        let mut metadata = image.metadata();
        let [cw, ch] = metadata.canvas_size.map(|word| word as i32);
        let [ox, oy] = metadata.offset;
        metadata.offset = match value {
            90 => [ch.wrapping_sub(oy).wrapping_sub(height as i32), ox],
            180 => [cw.wrapping_sub(ox).wrapping_sub(width as i32),
                ch.wrapping_sub(oy).wrapping_sub(height as i32)],
            270 => [oy, cw.wrapping_sub(ox).wrapping_sub(width as i32)],
            _ => unreachable!(),
        };
        metadata.base = rotate_point(value, metadata.base, [cw, ch]);
        if value != 180 { metadata.canvas_size.swap(0, 1); }
        let mut attributes = image.hg_attributes().as_map().clone();
        for words in attributes.values_mut() {
            let point = rotate_point(value, [words[0] as i32, words[1] as i32], [cw, ch]);
            words[..2].copy_from_slice(&point.map(|word| word as u32));
        }
        let frame = RgbaFrame::from_native_rgba8(output_width, output_height, pixels, metadata)
            .map_err(|_| native_state_error("native 468 transformed image rebuild failed"))?
            .with_raw_metadata(metadata).with_hg_attributes(HgFrameAttributes::new(attributes));
        Ok(frame)
}

fn rotate_point(degrees: u32, point: [i32; 2], canvas: [i32; 2]) -> [i32; 2] {
    match degrees {
        90 => [canvas[1].wrapping_sub(point[1]), point[0]],
        180 => [canvas[0].wrapping_sub(point[0]), canvas[1].wrapping_sub(point[1])],
        270 => [point[1], canvas[0].wrapping_sub(point[0])],
        _ => point,
    }
}

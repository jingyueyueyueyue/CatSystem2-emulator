//! Ordinary 367: globally clipped source/target and the native integer blend.
//! The baseline dispatcher rejects unequal final sizes; this entry does not
//! resample. Its requested target size participates only in the first clip.
use cs2_renderer::RgbaFrame;
use cs2_script::{NativeReturn, Result};

use super::{LoggingHost, image_blit, native_state_error, read_u32};

#[derive(Clone, Copy)]
struct Rect {
    left: i32,
    bottom: i32,
    right: i32,
    top: i32,
}

impl Rect {
    fn xywh(at: [i32; 2], size: [i32; 2]) -> Self {
        Self {
            left: at[0], bottom: at[1],
            right: at[0].wrapping_add(size[0]), top: at[1].wrapping_add(size[1]),
        }
    }

    fn intersect(self, other: Self) -> Option<Self> {
        let rect = Self {
            left: self.left.max(other.left), bottom: self.bottom.max(other.bottom),
            right: self.right.min(other.right), top: self.top.min(other.top),
        };
        (rect.left < rect.right && rect.bottom < rect.top).then_some(rect)
    }

    fn local(self, offset: [i32; 2]) -> Self {
        Self {
            left: self.left.wrapping_sub(offset[0]),
            right: self.right.wrapping_sub(offset[0]),
            bottom: self.bottom.wrapping_sub(offset[1]),
            top: self.top.wrapping_sub(offset[1]),
        }
    }

    fn size(self) -> [i32; 2] {
        [self.right - self.left, self.top - self.bottom]
    }
}

fn global_bounds(frame: &RgbaFrame) -> Result<Rect> {
    let size = [
        i32::try_from(frame.width()).map_err(|_| native_state_error(
            "native 367 image width exceeds signed coordinates",
        ))?,
        i32::try_from(frame.height()).map_err(|_| native_state_error(
            "native 367 image height exceeds signed coordinates",
        ))?,
    ];
    Ok(Rect::xywh(frame.metadata().offset, size))
}

impl LoggingHost<'_> {
    pub(super) fn image_global_blit_native(&mut self, arguments: &[u8]) -> Result<NativeReturn> {
        let source_id = read_u32(arguments, 20)?;
        if source_id == 0 {
            return Ok(NativeReturn::Continue);
        }
        let target_id = read_u32(arguments, 0)?;
        if target_id == 0 {
            // The PE calls the identical manager/canvas method for 115/367.
            return self.image_blit_native(arguments);
        }
        let source = self.image_handles.get(&source_id)
            .ok_or_else(|| native_state_error("native 367 invalid source image wrapper"))?;
        let target = self.image_handles.get(&target_id)
            .ok_or_else(|| native_state_error("native 367 invalid target image wrapper"))?;
        if source.is_empty() || target.is_empty() { return Ok(NativeReturn::Continue); }
        let request = Rect::xywh(
            [read_u32(arguments, 24)? as i32, read_u32(arguments, 28)? as i32],
            [read_u32(arguments, 32)? as i32, read_u32(arguments, 36)? as i32],
        );
        let Some(from) = request.intersect(global_bounds(source)?) else {
            return Ok(NativeReturn::Continue);
        };
        let from = from.local(source.metadata().offset);
        let source_offset = source.metadata().offset;
        let to = Rect::xywh(
            [
                (read_u32(arguments, 4)? as i32).wrapping_add(source_offset[0]),
                (read_u32(arguments, 8)? as i32).wrapping_add(source_offset[1]),
            ],
            [read_u32(arguments, 12)? as i32, read_u32(arguments, 16)? as i32],
        );
        let Some(to) = to.intersect(global_bounds(target)?) else {
            return Ok(NativeReturn::Continue);
        };
        let to = to.local(target.metadata().offset);
        // 0x596572..0x596586 uses the *source* clipped size for the
        // destination request. Target width/height only gated the clip above.
        let target_local = Rect::xywh([0, 0], [
            target.width() as i32, target.height() as i32,
        ]);
        let Some(to) = Rect::xywh([to.left, to.bottom], from.size()).intersect(target_local) else {
            return Ok(NativeReturn::Continue);
        };
        let [width, height] = [
            from.size()[0].min(to.size()[0]), from.size()[1].min(to.size()[1]),
        ];
        let shared = target_id == source_id || match (
            self.data_services.image_wrapper_pixels.get(&target_id),
            self.data_services.image_wrapper_pixels.get(&source_id),
        ) {
            (Some(target), Some(source)) => target == source,
            _ => false,
        };
        if shared && target.dimensions() != source.dimensions() {
            return Err(native_state_error("native 367 shared pixel dimensions disagree"));
        }
        let mut pixels = target.pixels().to_vec();
        for row in 0..height {
            let source_y = from.bottom + row;
            let target_y = to.bottom + row;
            let read = |pixels: &[u8], x: i32| -> [u8; 4] {
                let bytes = if shared { pixels } else { source.pixels() };
                let index = bottom_offset(source, x, source_y);
                bytes[index..index + 4].try_into().expect("clipped image pixel")
            };
            let first = read(&pixels, from.left);
            if to.left > 0 {
                let index = bottom_offset(target, to.left - 1, target_y);
                image_blit::seed_rgb(&mut pixels[index..index + 4], first);
            }
            for column in 0..width {
                let input = read(&pixels, from.left + column);
                let index = bottom_offset(target, to.left + column, target_y);
                image_blit::blend_pixel_integer(&mut pixels[index..index + 4], input);
            }
            if to.left + width < target.width() as i32 {
                let last = read(&pixels, from.left + width - 1);
                let index = bottom_offset(target, to.left + width, target_y);
                image_blit::seed_rgb(&mut pixels[index..index + 4], last);
            }
        }
        let frame = RgbaFrame::from_rgba8(target.width(), target.height(), pixels)
            .map_err(|_| native_state_error("native 367 target image rebuild failed"))?
            .with_raw_metadata(target.metadata())
            .with_hg_attributes(target.hg_attributes().clone());
        self.replace_image_wrapper_pixels(target_id, frame)?;
        Ok(NativeReturn::Continue)
    }
}

fn bottom_offset(frame: &RgbaFrame, x: i32, y: i32) -> usize {
    ((frame.height() as usize - 1 - y as usize) * frame.width() as usize + x as usize) * 4
}

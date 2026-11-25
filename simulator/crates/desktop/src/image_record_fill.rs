//! The registered 736 image-fill wrapper updates the current resource pixels.
use cs2_renderer::RgbaFrame;
use cs2_script::{NativeReturn, Result};

use crate::{LoggingHost, native_state_error, read_u32};

impl LoggingHost<'_> {
    /// Apply the raw-pixel or canvas behavior when the registered wrapper
    /// and its receivers match the recovered implementation.
    pub(crate) fn image_record_fill_native736(
        &mut self,
        args: &[u8],
    ) -> Result<Option<NativeReturn>> {
        if !self
            .data_services
            .ordinary_native_table
            .as_ref()
            .is_some_and(|table| table.image_fill_uses_current_pixels())
        {
            return Ok(None);
        }
        if args.len() != 24 {
            return Err(native_state_error(
                "native 736 requires exactly 24 argument bytes",
            ));
        }
        let handle = read_u32(args, 0)?;
        let rectangle = [
            read_u32(args, 4)? as i32,
            read_u32(args, 8)? as i32,
            read_u32(args, 12)? as i32,
            read_u32(args, 16)? as i32,
        ];
        if handle == 0 {
            // The zero-handle branch forwards only XYWH to the tile clear.
            self.clear_graphics_canvas_rect(rectangle)
                .map_err(|_| native_state_error("native 736 persistent canvas clear failed"))?;
            return Ok(Some(NativeReturn::Continue));
        }
        let current = self
            .image_resources
            .get(&handle)
            .ok_or_else(|| native_state_error("native 736 image resource is invalid"))?
            .image
            .clone();
        if let Some(frame) = fill_current(&current, rectangle, read_u32(args, 20)?)? {
            self.image_resources.get_mut(&handle).unwrap().image = frame.clone();
            // The sole static source aliases current pixels. A composite or
            // ANM source remains separate and must not inherit this edit.
            self.data_services
                .image_manager
                .synchronize_single_static_source(handle, &frame);
            self.synchronize_list_resource_pixels_only(handle, &frame)
                .map_err(|_| native_state_error("native 736 held pixel synchronization failed"))?;
        }
        Ok(Some(NativeReturn::Continue))
    }
}

fn fill_current(
    frame: &RgbaFrame,
    [x, y, width, height]: [i32; 4],
    argb: u32,
) -> Result<Option<RgbaFrame>> {
    if frame.is_empty() {
        return Ok(None);
    }
    // IntersectRect clips local bottom-origin pixels. The raw helper never
    // adds the HG offset or rebases the resource's cached source geometry.
    let left = x.max(0);
    let bottom = y.max(0);
    let right = x.wrapping_add(width).min(frame.width() as i32);
    let top = y.wrapping_add(height).min(frame.height() as i32);
    if left >= right || bottom >= top {
        return Ok(None);
    }
    let depth = frame.metadata().bit_depth;
    if !matches!(depth, 24 | 32) {
        return Err(native_state_error(
            "native 736 pixel format requires its unrecovered fill kernel",
        ));
    }
    let [red, green, blue, alpha] = [
        (argb >> 16) as u8,
        (argb >> 8) as u8,
        argb as u8,
        (argb >> 24) as u8,
    ];
    if depth == 24 && alpha == 0 {
        return Ok(None);
    }
    let mut pixels = frame.pixels().to_vec();
    for local_y in bottom..top {
        let row = frame.height() as usize - 1 - local_y as usize;
        for local_x in left..right {
            let offset = (row * frame.width() as usize + local_x as usize) * 4;
            let pixel = &mut pixels[offset..offset + 4];
            if depth == 32 {
                // The BGRA32 kernel stores the complete DWORD.
                pixel.copy_from_slice(&[red, green, blue, alpha]);
            } else if alpha == 255 {
                // The opaque kernel writes R,G,B into native BGR storage.
                pixel.copy_from_slice(&[blue, green, red, 255]);
            } else {
                // Each delta is multiplied by its source channel, then
                // arithmetic-shifted right before the byte store.
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
        .map(|image| {
            Some(
                image
                    .with_raw_metadata(frame.metadata())
                    .with_hg_attributes(frame.hg_attributes().clone()),
            )
        })
        .map_err(|_| native_state_error("native 736 filled image dimensions are invalid"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cs2_renderer::FrameMetadata;
    use cs2_script::Vm;

    #[test]
    fn native736_kernels_preserve_clipping_and_channel_math() -> anyhow::Result<()> {
        let frame_for_depth = |bit_depth| {
            let metadata = FrameMetadata {
                bit_depth,
                ..FrameMetadata::raster([0; 2])
            };
            RgbaFrame::from_rgba8_with_metadata(2, 2, [50, 100, 150, 255].repeat(4), metadata)
        };
        let bgr = frame_for_depth(24)?;
        assert!(fill_current(&bgr, [0, 0, 2, 2], 0x0010_2080)?.is_none());
        let opaque = fill_current(&bgr, [-1, 0, 2, 1], 0xff10_2080)?.unwrap();
        assert_eq!(opaque.pixels()[0..8], bgr.pixels()[0..8]);
        assert_eq!(opaque.pixels()[8..12], [128, 32, 16, 255]);
        assert_eq!(opaque.pixels()[12..16], bgr.pixels()[12..16]);
        let partial = fill_current(&bgr, [0, 0, 1, 1], 0x8010_2080)?.unwrap();
        assert_eq!(partial.pixels()[8..12], [47, 91, 139, 255]);

        let bgra = frame_for_depth(32)?;
        let transparent = fill_current(&bgra, [1, 1, 1, 1], 0x0010_2080)?.unwrap();
        assert_eq!(transparent.pixels()[4..8], [16, 32, 128, 0]);
        assert_eq!(transparent.pixels()[8..], bgra.pixels()[8..]);
        assert!(fill_current(&bgra, [9, 9, 2, 2], 0)?.is_none());
        Ok(())
    }

    #[test]
    fn native736_uses_registered_wrapper_without_a_game_profile() -> anyhow::Result<()> {
        let sample = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../games/cs2_full_v401/system/cs2_open.exe");
        if !sample.is_file() {
            return Ok(());
        }
        let temp = crate::tests::TempDir::new();
        let mut player =
            crate::HeadlessPlayer::open_with_save_root(&temp.0, temp.0.join("profile"))?;
        let table =
            cs2_runtime::native_table::OrdinaryNativeTable::from_pe(&std::fs::read(sample)?)
                .expect("ordinary V401 native table");
        assert!(table.image_fill_uses_current_pixels());
        player.with_host(|host| {
            assert!(host.data_services.native_profile.is_none());
            let zeros = [0; 24];
            assert_eq!(host.image_record_fill_native736(&zeros)?, None);

            host.data_services.ordinary_native_table = Some(table);
            let mut vm = Vm::new();
            assert_eq!(
                host.image_resource_native(736, &zeros, &mut vm)?,
                NativeReturn::Continue
            );
            assert!(
                host.image_resource_native(736, &zeros[..23], &mut vm)
                    .is_err()
            );
            assert!(host.image_resource_native(736, &[0; 25], &mut vm).is_err());

            let dimensions = [1u32, 1].map(u32::to_le_bytes).concat();
            let NativeReturn::Value(handle) =
                host.image_resource_native(733, &dimensions, &mut vm)?
            else {
                anyhow::bail!("native 733 did not return an image");
            };
            host.image_resources.get_mut(&handle).unwrap().updated = false;
            let args = [handle, 0, 0, 1, 1, 0x0010_2080]
                .map(u32::to_le_bytes)
                .concat();
            assert_eq!(
                host.image_resource_native(736, &args, &mut vm)?,
                NativeReturn::Continue
            );
            let image = &host.image_resources[&handle];
            assert!(!image.updated);
            assert_eq!(image.image.pixels(), [16, 32, 128, 0]);
            Ok(())
        })
    }
}

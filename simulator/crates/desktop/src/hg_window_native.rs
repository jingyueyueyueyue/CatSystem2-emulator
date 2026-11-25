//! Raw-HG nine-piece image construction. This produces an ordinary owned
//! image wrapper; the raw source and the decoded raster have separate lives.
use cs2_formats::HgFile;
use cs2_renderer::{FrameMetadata, RgbaFrame};
use cs2_runtime::native_table::HgWindowBlend;
use cs2_script::{NativeReturn, Result};

use super::super::{LoggingHost, image_blit, native_state_error, read_u32};

const MAX_PIXELS: usize = 128 * 1024 * 1024;
const MAX_BLEND_WORK: u64 = 256 * 1024 * 1024;

pub(crate) struct HgWindowNative;
impl HgWindowNative {
    pub(crate) fn supports(index: u32) -> bool { index == 648 }
}

impl LoggingHost<'_> {
    pub(crate) fn hg_window_native(&mut self, args: &[u8]) -> Result<NativeReturn> {
        // Both wrappers read raw-HG, width, height and register the resulting
        // wrapper with the invoking task. The older one fixes format=4 in
        // its initializer; the newer one passes the same format explicitly.
        let blend = self.data_services.ordinary_native_table.as_ref()
            .and_then(|table| table.hg_window_blend())
            .or_else(|| self.data_services.startup_companion_natives.as_ref()
                .and_then(|companion| companion.hg_window_blend))
            .ok_or_else(|| native_state_error("native 648 needs a registered HG window constructor and blend"))?;
        let raw = read_u32(args, 0)?;
        let size = [read_u32(args, 4)?, read_u32(args, 8)?];
        let bytes = self.data_services.ordinary.raw_image(raw)?;
        let image = compose(&bytes, size, blend)?;
        Ok(NativeReturn::Value(self.register_image_wrapper(image, true)?))
    }
}

fn compose(bytes: &[u8], size: [u32; 2], blend: HgWindowBlend) -> Result<RgbaFrame> {
    if size.contains(&0) || size.iter().any(|&axis| axis > i32::MAX as u32) {
        return Err(native_state_error("native 648 needs positive signed image dimensions"));
    }
    let length = (size[0] as usize).checked_mul(size[1] as usize)
        .and_then(|n| n.checked_mul(4)).filter(|&n| n <= MAX_PIXELS)
        .ok_or_else(|| native_state_error("native 648 raster exceeds 128 MiB"))?;
    let file = HgFile::parse(bytes)
        .map_err(|_| native_state_error("native 648 source is not a supported HG2/HG3 file"))?;
    let mut pixels = Vec::new();
    pixels.try_reserve_exact(length)
        .map_err(|_| native_state_error("native 648 raster allocation failed"))?;
    pixels.resize(length, 0);
    let mut work = 0u64;
    for id in 0..9 {
        // HG2 0x10 has one unnumbered image; its original decoder ignores id.
        let frame = if file.signature == *b"HG-2" && file.version as u16 == 0x10 {
            file.frames.first()
        } else {
            file.frames.iter().find(|frame| frame.id == id)
        };
        let Some(frame) = frame else { continue; };
        let [width, height] = [frame.width, frame.height];
        let source_length = (width as usize).checked_mul(height as usize)
            .and_then(|n| n.checked_mul(4)).filter(|&n| n <= MAX_PIXELS)
            .ok_or_else(|| native_state_error("native 648 decoded tile exceeds 128 MiB"))?;
        if width == 0 || height == 0 || width > i32::MAX as u32 || height > i32::MAX as u32 {
            return Err(native_state_error("native 648 tile has invalid dimensions"));
        }
        let horizontal = matches!(id, 6..=8);
        let vertical = matches!(id, 4 | 5 | 8);
        if (horizontal && size[0] < width) || (vertical && size[1] < height) {
            return Err(native_state_error("native 648 tiled axis would underflow its native boundary"));
        }
        let source = frame.decode_pixels_rgba8(bytes)
            .map_err(|_| native_state_error("native 648 HG tile decoding failed"))?;
        if source.len() != source_length {
            return Err(native_state_error("native 648 decoded tile length disagrees with dimensions"));
        }
        // Each tile uses its own width/height as both margins. No metadata
        // offsets, source canvas sizes or corner-derived margins are applied.
        let x_start = if horizontal { i64::from(width) }
            else if matches!(id, 1 | 3 | 5) { i64::from(size[0])-i64::from(width) } else { 0 };
        let y_start = if vertical { i64::from(height) }
            else if matches!(id, 2 | 3 | 7) { i64::from(size[1])-i64::from(height) } else { 0 };
        let x_end = if horizontal { i64::from(size[0]-width) } else { x_start+1 };
        let y_end = if vertical { i64::from(size[1]-height) } else { y_start+1 };
        let mut y = y_start;
        while y < y_end {
            let copy_height = if vertical { i64::from(height).min(y_end-y) } else { i64::from(height) };
            let mut x = x_start;
            while x < x_end {
                let copy_width = if horizontal { i64::from(width).min(x_end-x) } else { i64::from(width) };
                mix_tile(&mut pixels, size, &source, [width, height], [x, y],
                    [copy_width, copy_height], &mut work, blend)?;
                x += if horizontal { i64::from(width) } else { 1 };
            }
            y += if vertical { i64::from(height) } else { 1 };
        }
    }
    // The native allocates a plain format-4 image with zero geometry fields.
    // Its alpha flag is not recomputed from the resulting pixel contents.
    RgbaFrame::from_rgba8_with_metadata(size[0], size[1], pixels, FrameMetadata::raster([0, 0]))
        .map_err(|_| native_state_error("native 648 output image construction failed"))
}

fn mix_tile(
    target: &mut [u8], size: [u32; 2], source: &[u8], source_size: [u32; 2],
    at: [i64; 2], copy: [i64; 2], work: &mut u64, blend: HgWindowBlend,
) -> Result<()> {
    // The native raster operation clips source and destination independently.
    // Destination clipping therefore does not advance the source origin.
    let x = at[0].max(0);
    let y = at[1].max(0);
    let width = (at[0]+copy[0]).min(i64::from(size[0]))-x;
    let height = (at[1]+copy[1]).min(i64::from(size[1]))-y;
    if width <= 0 || height <= 0 { return Ok(()); }
    *work = work.checked_add((width as u64+2)*height as u64)
        .filter(|&n| n <= MAX_BLEND_WORK)
        .ok_or_else(|| native_state_error("native 648 tile blend work exceeds its bounded operation limit"))?;
    for row in 0..height as usize {
        let from = (source_size[1] as usize-1-row)*source_size[0] as usize*4;
        let to = ((size[1] as usize-1-y as usize-row)*size[0] as usize+x as usize)*4;
        let first = source[from..from+4].try_into().expect("bounded HG pixel");
        if blend == HgWindowBlend::SeedAdjacentRgb && x > 0 { image_blit::seed_rgb(&mut target[to-4..to], first); }
        for column in 0..width as usize {
            let input: [u8; 4] = source[from+column*4..from+column*4+4].try_into().expect("bounded HG pixel");
            // The old 4/4 kernel b5c50 skips alpha-zero source pixels before
            // examining the destination. It has no adjacent RGB seeding.
            if blend == HgWindowBlend::SkipTransparent && input[3] == 0 { continue; }
            image_blit::blend_pixel_integer(&mut target[to+column*4..to+column*4+4], input);
        }
        if blend == HgWindowBlend::SeedAdjacentRgb && x+width < i64::from(size[0]) {
            let last = source[from+(width as usize-1)*4..from+width as usize*4]
                .try_into().expect("bounded HG pixel");
            let right = to+width as usize*4;
            image_blit::seed_rgb(&mut target[right..right+4], last);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{HeadlessPlayer, tests::TempDir};
    use cs2_script::{KcsProgram, Vm};
    use std::path::Path;

    #[test]
    #[ignore = "requires both installed HG window engines"]
    fn native_hg_window_registered_pixels_and_ownership() -> anyhow::Result<()> {
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 100)
            .encode(&[48, 112, 176], 1, 1, image::ExtendedColorType::Rgb8)?;
        let mut hg = b"HG-3".to_vec();
        hg.extend([12u32, 0x300, 0, 6].into_iter().flat_map(u32::to_le_bytes));
        hg.extend_from_slice(b"stdinfo\0");
        hg.extend([56u32, 40, 1, 1, 32, 0, 0, 1, 1, 1, 0, 0].into_iter().flat_map(u32::to_le_bytes));
        hg.extend_from_slice(b"img_jpg\0");
        hg.extend([16 + jpeg.len() as u32, jpeg.len() as u32].into_iter().flat_map(u32::to_le_bytes));
        hg.extend(&jpeg);
        hg.extend_from_slice(b"img_al\0\0");
        hg.extend([0u32, 17, 9, 1].into_iter().flat_map(u32::to_le_bytes));
        hg.extend([0x78, 0x9c, 0x63, 0, 0, 0, 1, 0, 1]); // Zlib-compressed zero alpha.
        let file = HgFile::parse(&hg)?;
        let transparent = file.decode_frame_rgba8(&hg, 0)?;
        assert_eq!(transparent[3], 0);
        assert_ne!(&transparent[..3], &[0; 3]);

        let games = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../games");
        for (game, expected) in [("Yukikoi Melt", transparent.repeat(3)), ("祝福のカンパネラ", vec![0; 12])] {
            let scratch = TempDir::new();
            let mut player = HeadlessPlayer::open_with_save_root(games.join(game), &scratch.0)?;
            player.data_services.native_profile = None;
            player.data_services.css_task_pe_sha256 = None;
            let raw = 0x8000_0000;
            let mut snapshot = player.script_data_objects_snapshot()?;
            snapshot.raw_images.insert(raw, hg.clone());
            player.commit_script_data_objects_restore(HeadlessPlayer::prepare_script_data_objects_restore(&snapshot)?);
            let mut vm = Vm::from_program(KcsProgram::from_code(&[0x3b], vec![0; 8], 0)?)?;
            player.with_host(|host| {
                let args = [raw, 3, 1].map(u32::to_le_bytes).concat();
                let NativeReturn::Value(image) = host.dispatch_kcs_native(648, &args, &mut vm)? else {
                    anyhow::bail!("expected an owned raster from {game}");
                };
                assert_eq!(host.data_services.image_wrapper_owners.get(&image), Some(&host.current_task_id));
                assert_eq!(host.dispatch_kcs_native(366, &raw.to_le_bytes(), &mut vm)?, NativeReturn::Continue);
                let frame = &host.image_handles[&image];
                assert_eq!([frame.width(), frame.height()], [3, 1]);
                assert_eq!(frame.pixels(), expected.as_slice(), "{game}");
                host.data_services.ordinary_native_table = None;
                host.data_services.startup_companion_natives = None;
                let error = host.dispatch_kcs_native(648, &args, &mut vm).unwrap_err();
                assert!(error.to_string().contains("needs a registered HG window constructor"));
                Ok(())
            })?;
        }
        Ok(())
    }
}

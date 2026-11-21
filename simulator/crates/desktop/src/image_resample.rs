//! Independent implementation of the whole-image 5e4f10 -> 5dd1f0 path.
//! Uses original f32 geometry and integer sampling, without a texture filter.
use cs2_renderer::{HgFrameAttributes, RgbaFrame};
use cs2_script::{NativeReturn, Result};

use super::{LoggingHost, native_state_error, read_u32};

#[path = "image_copy.rs"]
mod copy;
use copy::SamplingProfile;

impl LoggingHost<'_> {
    fn image_sampling_profile(&self) -> Option<SamplingProfile> {
        self.data_services.native_profile
            .and_then(|profile| SamplingProfile::for_sha(profile.pe_sha256))
    }

    /// Exact reviewed profiles replace the old approximate rectangle kernel.
    /// Other PEs retain their existing receiver until its profile is recovered.
    pub(super) fn image_wrapper_copy_native116(&mut self, args: &[u8]) -> Result<Option<NativeReturn>> {
        let Some(profile) = self.image_sampling_profile() else { return Ok(None); };
        let target_id = read_u32(args, 0)?;
        if target_id == 0 { return Ok(Some(NativeReturn::Continue)); }
        let source_id = read_u32(args, 20)?;
        if source_id == 0 { return Ok(Some(NativeReturn::Continue)); }
        let target = self.image_handles.get(&target_id)
            .ok_or_else(|| native_state_error("native 116 target is not an image wrapper"))?;
        let source = self.image_handles.get(&source_id)
            .ok_or_else(|| native_state_error("native 116 source is not an image wrapper"))?;
        let shared = target_id == source_id || self.data_services.image_wrapper_pixels.get(&target_id)
            .is_some_and(|pixel| self.data_services.image_wrapper_pixels.get(&source_id) == Some(pixel));
        if let Some(frame) = copy::copy_wrapper(target, source,
            request_rect(args, 4)?, request_rect(args, 24)?, shared, profile)? {
            self.replace_image_wrapper_pixels(target_id, frame)?;
        }
        Ok(Some(NativeReturn::Continue))
    }

    pub(super) fn resize_image_canvas(&self, source: &RgbaFrame, requested: [u32; 2]) -> Result<RgbaFrame> {
        match self.image_sampling_profile() {
            Some(profile) => resize_canvas_profile(source, requested, profile),
            // Keep the pre-existing Yukikoi baseline for unreviewed PEs;
            // this does not establish their strong-shrink version semantics.
            None => resize_canvas(source, requested),
        }
    }

    /// 735 and 116 feed the same independently implemented rectangle copy
    /// consumer. Resource copy changes pixels without setting resource.updated.
    pub(super) fn image_resource_copy_native735(&mut self, args: &[u8]) -> Result<NativeReturn> {
        let source_id = read_u32(args, 0)?;
        let source = if source_id == 0 { None } else {
            Some(self.image_resources.get(&source_id)
                .ok_or_else(|| native_state_error("native 735 invalid source resource"))?.image.clone())
        };
        let target_id = read_u32(args, 20)?;
        if target_id == 0 { return Ok(NativeReturn::Continue); }
        let source = source.ok_or_else(|| native_state_error("native 735 zero source resource"))?;
        let target = self.image_resources.get(&target_id)
            .ok_or_else(|| native_state_error("native 735 invalid target resource"))?.image.clone();
        if args.len() != 40 {
            return Err(native_state_error("native 735 requires two resources and eight rectangle DWORDs"));
        }
        // 5a90e7/5a9134 independently clip source and target, then equal raw
        // request sizes share the minimum clipped extents. No nearest filter.
        let to = request_rect(args, 4)?;
        let from = request_rect(args, 24)?;
        let result = match self.image_sampling_profile() {
            Some(profile) => copy::copy_wrapper(&target, &source, to, from, source_id == target_id, profile),
            None => crate::plane_native::copy_wrapper(&target, &source, to, from, source_id == target_id, [false; 2]),
        };
        let Some(frame) = result? else { return Ok(NativeReturn::Continue); };
        self.image_resources.get_mut(&target_id).unwrap().image = frame.clone();
        self.data_services.image_manager.synchronize_single_static_source(target_id, &frame);
        // Existing held wrappers alias current pixels. Keep their bytes in
        // sync without invalidating list geometry or altering the shared flag.
        self.synchronize_list_resource_pixels_only(target_id, &frame)
            .map_err(|_| native_state_error("native 735 held pixel synchronization failed"))?;
        Ok(NativeReturn::Continue)
    }

    pub(super) fn image_resize_native705(&mut self, args: &[u8]) -> Result<NativeReturn> {
        let handle = read_u32(args, 0)?;
        if handle == 0 { return Ok(NativeReturn::Continue); }
        if args.len() != 12 {
            return Err(native_state_error("native 705 requires an image wrapper and two dimensions"));
        }
        let source = self.image_handles.get(&handle)
            .ok_or_else(|| native_state_error("native 705 invalid/wrong-type image wrapper"))?;
        let frame = self.resize_image_canvas(source, [read_u32(args, 4)?, read_u32(args, 8)?])?;
        // Mutate the existing pixel identity, including 118 aliases and the
        // frame cache borrowed by 709. No new task-owned wrapper is created.
        self.replace_image_wrapper_pixels(handle, frame)?;
        Ok(NativeReturn::Continue)
    }
}

fn request_rect(args: &[u8], at: usize) -> Result<crate::plane_native::CopyRect> {
    Ok(crate::plane_native::CopyRect { x: read_u32(args, at)? as i32,
        y: read_u32(args, at + 4)? as i32, width: read_u32(args, at + 8)? as i32,
        height: read_u32(args, at + 12)? as i32 })
}

pub(super) fn resize_canvas(source: &RgbaFrame, requested: [u32; 2]) -> Result<RgbaFrame> {
    resize_canvas_profile(source, requested, SamplingProfile::Yukikoi)
}

fn resize_canvas_profile(source: &RgbaFrame, requested: [u32; 2], profile: SamplingProfile) -> Result<RgbaFrame> {
    let mut metadata = source.metadata();
    if !matches!(metadata.bit_depth, 24 | 32) {
        return Err(native_state_error("native image resampling requires BGR24 or BGRA32 pixels"));
    }
    // Raw canvas zeroes do not fall back to the pixel dimensions here.
    let ratio = [requested[0] as f32 / metadata.canvas_size[0] as f32,
        requested[1] as f32 / metadata.canvas_size[1] as f32];
    let size = [unsigned_scaled(source.width(), ratio[0]), unsigned_scaled(source.height(), ratio[1])];
    if source.is_empty() {
        // The equal-size copy declines empty storage before metadata writes.
        // Otherwise the interpolation path would divide by an empty axis.
        return if size == [source.width(), source.height()] { Ok(source.clone()) } else {
            Err(native_state_error("native image resampling would interpolate an empty source"))
        };
    }
    if metadata.canvas_size.contains(&0) {
        return Err(native_state_error("native image resampling has a zero original canvas axis"));
    }
    if size.iter().any(|&axis| axis > i32::MAX as u32) {
        return Err(native_state_error("native resampled dimensions exceed signed coordinates"));
    }
    let length = (size[0] as usize).checked_mul(size[1] as usize)
        .and_then(|n| n.checked_mul(4)).filter(|&n| n <= 128 * 1024 * 1024)
        .ok_or_else(|| native_state_error("native resampled pixels exceed 128 MiB"))?;
    let pixels = if size == [source.width(), source.height()] {
        // Equal sizes take the copy path, including transparent RGB bytes.
        source.pixels().to_vec()
    } else {
        let mut pixels = Vec::new();
        pixels.try_reserve_exact(length)
            .map_err(|_| native_state_error("native image resampling allocation failed"))?;
        pixels.resize(length, 0);
        if metadata.bit_depth == 24 {
            // Native zero-filled BGR bytes still represent opaque black.
            for pixel in pixels.chunks_exact_mut(4) { pixel[3] = 255; }
        }
        if !size.contains(&0) {
            copy::sample_pixels(source, size, &mut pixels,
                [0, 0, source.width(), source.height()], [0, 0, size[0], size[1]],
                false, profile, metadata.bit_depth)?;
        }
        pixels
    };
    metadata.canvas_size = [unsigned_scaled(metadata.canvas_size[0], ratio[0]),
        unsigned_scaled(metadata.canvas_size[1], ratio[1])];
    metadata.offset = [unsigned_scaled(metadata.offset[0] as u32, ratio[0]) as i32,
        unsigned_scaled(metadata.offset[1] as u32, ratio[1]) as i32];
    metadata.base = [signed_truncate(metadata.base[0] as f32 * ratio[0] + 0.5),
        signed_truncate(metadata.base[1] as f32 * ratio[1] + 0.5)];
    let mut attributes = source.hg_attributes().as_map().clone();
    for words in attributes.values_mut() {
        for (i, word) in words[..4].iter_mut().enumerate() {
            *word = signed_truncate(*word as i32 as f32 * ratio[i % 2]) as u32;
        }
    }
    RgbaFrame::from_native_rgba8(size[0], size[1], pixels, metadata)
        .map(|frame| frame.with_raw_metadata(metadata).with_hg_attributes(HgFrameAttributes::new(attributes)))
        .map_err(|_| native_state_error("native resampled frame construction failed"))
}

/// SSE conversion to signed i64, keeping its low DWORD. Invalid conversion
/// produces i64::MIN, whose low DWORD is zero (not Rust's saturating cast).
fn unsigned_scaled(value: u32, ratio: f32) -> u32 {
    let value = f64::from(value as f32 * ratio + 0.5);
    if !value.is_finite() || !(-9_223_372_036_854_775_808.0..9_223_372_036_854_775_808.0).contains(&value) {
        0
    } else { value.trunc() as i64 as u32 }
}

fn signed_truncate(value: f32) -> i32 {
    if value.is_finite() && (-2_147_483_648.0..2_147_483_648.0).contains(&f64::from(value)) {
        value.trunc() as i32
    } else { i32::MIN }
}


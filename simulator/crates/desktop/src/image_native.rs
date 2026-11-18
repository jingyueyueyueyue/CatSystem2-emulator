//! Ordinary image-wrapper capture, CSS exchange and metadata. No task or font handle is
//! interchangeable with one of these typed image wrappers.
use cs2_renderer::{AnimationFrameSelection, HgFrameAttributes, RgbaFrame};
use cs2_script::{NativeReturn, Result, Vm};

use super::{LoggingHost, native_state_error, read_u32};

impl LoggingHost<'_> {
    pub(super) fn image_wrapper_native(
        &mut self, index: u32, arguments: &[u8], vm: &mut Vm,
    ) -> Result<NativeReturn> {
        match index {
            375 => {
                // Resolve the VM destination before expanding the filename.
                let target = read_u32(arguments, 4)?;
                vm.read(target, 1)?;
                let name = super::expand_kcs_format(vm, &vm.read_string(read_u32(arguments, 0)?)?, 0)?;
                let name = super::trim_nul(&name);
                if name.len() >= 260 {
                    return Err(native_state_error("native 375 image name exceeds its 259-byte buffer"));
                }
                let Some(bytes) = self.read_image_header_prefix(name)? else {
                    return Ok(NativeReturn::Value(0));
                };
                let Some(fields) = cs2_runtime::native_image_metadata(&bytes, 0)? else {
                    return Ok(NativeReturn::Value(0));
                };
                // The original reads nine DWORDs from the resource header;
                // failure preserves the VM output (there is no initial clear).
                let output: Vec<u8> = fields.into_iter().flat_map(u32::to_le_bytes).collect();
                vm.write(target, &output)?;
                Ok(NativeReturn::Value(1))
            }
            113 => {
                let Some(canvas) = self.canvas.as_ref() else {
                    return Ok(NativeReturn::Value(0));
                };
                let snapshot = canvas.snapshot();
                let frame = RgbaFrame::from_rgba8(
                    snapshot.width(), snapshot.height(), snapshot.rgba().to_vec(),
                ).map_err(|_| native_state_error("native 113 frame capture failed"))?;
                Ok(NativeReturn::Value(self.register_image_wrapper(frame, true)?))
            }
            118 => {
                if arguments.len() != 4 {
                    return Err(native_state_error("native 118 requires one image wrapper"));
                }
                Ok(NativeReturn::Value(self.share_image_wrapper(read_u32(arguments, 0)?)?))
            }
            121 => {
                let handle = read_u32(arguments, 0)?;
                if handle == 0 { return Ok(NativeReturn::Continue); }
                let percent = [
                    read_u32(arguments, 4)?, read_u32(arguments, 8)?, read_u32(arguments, 12)?,
                ];
                if percent == [100; 3] { return Ok(NativeReturn::Continue); }
                let image = self.image_handles.get(&handle)
                    .ok_or_else(|| native_state_error("native 121 invalid image wrapper"))?;
                if image.metadata().bit_depth == 24 { return Ok(NativeReturn::Continue); }
                let frame = image.adjust_rgb_percent(percent)
                    .map_err(|_| native_state_error("native 121 RGB adjustment failed"))?
                    .with_raw_metadata(image.metadata());
                self.replace_image_wrapper_pixels(handle, frame)?;
                Ok(NativeReturn::Continue)
            }
            119 => {
                if arguments.len() != 12 {
                    return Err(native_state_error("native 119 requires an image and two output addresses"));
                }
                let handle = read_u32(arguments, 0)?;
                let width = read_u32(arguments, 4)?;
                let height = read_u32(arguments, 8)?;
                vm.read(width, 4)?;
                vm.read(height, 4)?;
                vm.write(width, &0u32.to_le_bytes())?;
                vm.write(height, &0u32.to_le_bytes())?;
                if handle != 0 {
                    let image = self.image_handles.get(&handle)
                        .ok_or_else(|| native_state_error("native 119 invalid image wrapper"))?;
                    vm.write(width, &image.width().to_le_bytes())?;
                    vm.write(height, &image.height().to_le_bytes())?;
                }
                Ok(NativeReturn::Continue)
            }
            241 => {
                let container = read_u32(arguments, 0)?;
                if container == 0 {
                    return Ok(NativeReturn::Value(0));
                }
                let record = read_u32(arguments, 4)?;
                let image = read_u32(arguments, 8)?;
                if image == 0 {
                    return Ok(NativeReturn::Value(0));
                }
                let container = self.css_handles.get_mut(&container)
                    .ok_or_else(|| native_state_error("native 241 invalid CSS container"))?;
                let image = self.image_handles.get(&image)
                    .ok_or_else(|| native_state_error("native 241 invalid image wrapper"))?;
                let bytes = image.encode_hg2()
                    .map_err(|_| native_state_error("native 241 HG-2 encoding failed"))?;
                container.append(record, &bytes)
                    .map_err(|_| native_state_error("native 241 CSS record append failed"))?;
                Ok(NativeReturn::Value(1))
            }
            251 => {
                let container = read_u32(arguments, 0)?;
                if container == 0 {
                    return Ok(NativeReturn::Value(0));
                }
                let record = read_u32(arguments, 4)?;
                let container = self.css_handles.get(&container)
                    .ok_or_else(|| native_state_error("native 251 invalid CSS container"))?;
                let Some(bytes) = container.try_record(record)
                    .map_err(|_| native_state_error("native 251 malformed CSS record storage"))? else {
                    return Ok(NativeReturn::Value(0));
                };
                if bytes.get(..4) != Some(b"HG-2") {
                    return Err(native_state_error("native 251 CSS image is not HG-2"));
                }
                let frame = RgbaFrame::decode_hg(bytes, AnimationFrameSelection::Index(0))
                    .map_err(|_| native_state_error("native 251 HG-2 decoding failed"))?;
                Ok(NativeReturn::Value(self.register_image_wrapper(frame, true)?))
            }
            362 => {
                let handle = read_u32(arguments, 0)?;
                let width = read_u32(arguments, 4)?;
                let height = read_u32(arguments, 8)?;
                // Clearing and final writes are deliberately ordered: output
                // addresses can alias, including partial DWORD overlap.
                vm.write(width, &0u32.to_le_bytes())?;
                vm.write(height, &0u32.to_le_bytes())?;
                if handle != 0 {
                    let image = self.image_handles.get(&handle)
                        .ok_or_else(|| native_state_error("native 362 invalid image wrapper"))?;
                    let canvas = crate::image_manager::native_canvas_dimensions(image);
                    vm.write(width, &canvas[0].to_le_bytes())?;
                    vm.write(height, &canvas[1].to_le_bytes())?;
                }
                Ok(NativeReturn::Continue)
            }
            363 => {
                let handle = read_u32(arguments, 0)?;
                let x = read_u32(arguments, 4)?;
                let y = read_u32(arguments, 8)?;
                // Native resolves both addresses before clearing either. Keep
                // ordered stores when outputs alias or partially overlap.
                vm.read(x, 4)?;
                vm.read(y, 4)?;
                vm.write(x, &0u32.to_le_bytes())?;
                vm.write(y, &0u32.to_le_bytes())?;
                if handle != 0 {
                    let image = self.image_handles.get(&handle)
                        .ok_or_else(|| native_state_error("native 363 invalid image wrapper"))?;
                    let offset = image.metadata().offset;
                    vm.write(x, &offset[0].to_le_bytes())?;
                    vm.write(y, &offset[1].to_le_bytes())?;
                }
                Ok(NativeReturn::Continue)
            }
            364 => {
                let raw = read_u32(arguments, 0)?;
                if raw == 0 {
                    // The native skips even the frame argument on this path.
                    return Ok(NativeReturn::Value(0));
                }
                let id = read_u32(arguments, 4)?;
                let bytes = self.data_services.ordinary.raw_image(raw)?;
                // The raw-data decoder shares actual format handling with
                // the manager, without reading or mutating its cache state.
                let Some(mut frame) = self.decode_managed_image_frame(&bytes, id)? else {
                    return Ok(NativeReturn::Value(0));
                };
                // 0x59794d..0x597962 converts decoded BGR24 storage to
                // BGRA32, preserving geometry/attributes and adding FF alpha.
                if frame.metadata().bit_depth == 24 {
                    let mut metadata = frame.metadata();
                    metadata.bit_depth = 32;
                    frame = frame.with_raw_metadata(metadata);
                }
                // Decoded pixels own their storage independently of 365/366.
                Ok(NativeReturn::Value(self.register_image_wrapper(frame, true)?))
            }
            369 | 376 => {
                let handle = read_u32(arguments, 0)?;
                if handle == 0 { return Ok(NativeReturn::Continue); }
                let value = read_u32(arguments, 4)?;
                if index == 376 && !matches!(value, 1..=3) {
                    return Ok(NativeReturn::Continue);
                }
                let image = self.image_handles.get(&handle)
                    .ok_or_else(|| native_state_error("native image color operation has an invalid wrapper"))?;
                if image.metadata().bit_depth == 24 { return Ok(NativeReturn::Continue); }
                let mut pixels = image.pixels().to_vec();
                for pixel in pixels.chunks_exact_mut(4) {
                    if index == 369 {
                        let factors = [
                            (value >> 16) as u8, (value >> 8) as u8, value as u8, (value >> 24) as u8,
                        ];
                        for (channel, factor) in pixel.iter_mut().zip(factors) {
                            *channel = (u32::from(*channel) * u32::from(factor) / 255) as u8;
                        }
                    } else if value == 3 {
                        for channel in &mut pixel[..3] { *channel = !*channel; }
                    } else {
                        // The PE builds three byte tables using f32 products
                        // and truncates each contribution before adding.
                        let gray = ((f32::from(pixel[0]) * f32::from_bits(0x3e99_0afe)) as u8)
                            .wrapping_add((f32::from(pixel[1]) * f32::from_bits(0x3f16_2c23)) as u8)
                            .wrapping_add((f32::from(pixel[2]) * f32::from_bits(0x3dea_7371)) as u8);
                        if value == 1 {
                            pixel[..3].fill(gray);
                        } else {
                            pixel[0] = gray.saturating_add(30);
                            pixel[1] = gray.saturating_add(5);
                            pixel[2] = gray.saturating_sub(35);
                        }
                    }
                }
                let frame = RgbaFrame::from_native_rgba8(image.width(), image.height(), pixels, image.metadata())
                    .map_err(|_| native_state_error("native image color rebuild failed"))?
                    .with_raw_metadata(image.metadata())
                    .with_hg_attributes(image.hg_attributes().clone());
                self.replace_image_wrapper_pixels(handle, frame)?;
                Ok(NativeReturn::Continue)
            }
            370 => {
                let handle = read_u32(arguments, 0)?;
                if handle == 0 { return Ok(NativeReturn::Value(0)); }
                let image = self.image_handles.get(&handle)
                    .ok_or_else(|| native_state_error("native 370 invalid image wrapper"))?;
                if image.metadata().bit_depth == 24 {
                    return Ok(NativeReturn::Value(255));
                }
                let offset = image.metadata().offset;
                let x = read_u32(arguments, 4)?.wrapping_sub(offset[0] as u32);
                let y = read_u32(arguments, 8)?.wrapping_sub(offset[1] as u32);
                let alpha = if x < image.width() && y < image.height() {
                    // The native uses the raw bottom-origin pixel accessor,
                    // unlike the top-origin HG decode accessor.
                    image.pixels()[((image.height() - 1 - y) as usize * image.width() as usize
                        + x as usize) * 4 + 3]
                } else { 0 };
                Ok(NativeReturn::Value(u32::from(alpha)))
            }
            371 => {
                let handle = read_u32(arguments, 0)?;
                if handle == 0 { return Ok(NativeReturn::Value(0)); }
                let x = read_u32(arguments, 4)?;
                let y = read_u32(arguments, 8)?;
                vm.read(x, 4)?;
                vm.read(y, 4)?;
                let base = self.image_handles.get(&handle)
                    .ok_or_else(|| native_state_error("native 371 invalid image wrapper"))?
                    .metadata().base;
                vm.write(x, &base[0].to_le_bytes())?;
                vm.write(y, &base[1].to_le_bytes())?;
                Ok(NativeReturn::Value(1))
            }
            372..=374 => {
                let handle = read_u32(arguments, 0)?;
                if handle == 0 { return Ok(NativeReturn::Value(0)); }
                let values = [read_u32(arguments, 4)?, read_u32(arguments, 8)?];
                let image = self.image_handles.get(&handle)
                    .ok_or_else(|| native_state_error("native image geometry update has an invalid wrapper"))?;
                let mut metadata = image.metadata();
                match index {
                    372 => metadata.canvas_size = values,
                    373 => metadata.offset = [values[0] as i32, values[1] as i32],
                    374 => metadata.base = [values[0] as i32, values[1] as i32],
                    _ => unreachable!(),
                }
                let frame = image.clone().with_raw_metadata(metadata);
                self.replace_image_wrapper_pixels(handle, frame)?;
                Ok(NativeReturn::Value(1))
            }
            536 => {
                let handle = read_u32(arguments, 0)?;
                if handle == 0 {
                    return Ok(NativeReturn::Value(0));
                }
                let attribute = read_u32(arguments, 4)? as u16;
                let image = self.image_handles.get(&handle)
                    .ok_or_else(|| native_state_error("native 536 invalid image wrapper"))?;
                let Some(values) = image.attribute(attribute) else {
                    return Ok(NativeReturn::Value(0));
                };
                let mut outputs = [0; 5];
                for (index, output) in outputs.iter_mut().enumerate() {
                    *output = read_u32(arguments, 8 + index * 4)?;
                    vm.read(*output, 4)?;
                }
                for (output, value) in outputs.into_iter().zip(values) {
                    vm.write(output, &value.to_le_bytes())?;
                }
                Ok(NativeReturn::Value(1))
            }
            537 => {
                let target = read_u32(arguments, 0)?;
                let source = read_u32(arguments, 4)?;
                if target == 0 || source == 0 { return Ok(NativeReturn::Value(0)); }
                let input = self.image_handles.get(&source)
                    .ok_or_else(|| native_state_error("native 537 invalid source image wrapper"))?;
                let input = input.hg_attributes().as_map().clone();
                let frame = self.image_handles.get(&target)
                    .ok_or_else(|| native_state_error("native 537 invalid target image wrapper"))?;
                let mut attributes = frame.hg_attributes().as_map().clone();
                // Only source IDs overwrite. Unmentioned destination records
                // survive; aliasing source/destination is supported.
                attributes.extend(input);
                let frame = frame.clone().with_hg_attributes(HgFrameAttributes::new(attributes));
                self.replace_image_wrapper_pixels(target, frame)?;
                Ok(NativeReturn::Value(1))
            }
            538 => {
                let handle = read_u32(arguments, 0)?;
                if handle == 0 { return Ok(NativeReturn::Continue); }
                let radius = [read_u32(arguments, 4)?, read_u32(arguments, 8)?];
                let source = self.image_handles.get(&handle)
                    .ok_or_else(|| native_state_error("native 538 invalid image wrapper"))?;
                let frame = blur_wrapper_bgra32(source, radius)?;
                self.replace_image_wrapper_pixels(handle, frame)?;
                Ok(NativeReturn::Continue)
            }
            558 => {
                let handle = read_u32(arguments, 0)?;
                if handle == 0 { return Ok(NativeReturn::Continue); }
                let horizontal = read_u32(arguments, 4)? != 0;
                let image = self.image_handles.get(&handle)
                    .ok_or_else(|| native_state_error("native 558 invalid image wrapper"))?;
                let width = image.width() as usize;
                let height = image.height() as usize;
                let mut pixels = image.pixels().to_vec();
                for y in 0..height {
                    for x in 0..width {
                        let source = (y * width + x) * 4;
                        let output = if horizontal {
                            (y * width + width - 1 - x) * 4
                        } else {
                            ((height - 1 - y) * width + x) * 4
                        };
                        pixels[output..output + 4].copy_from_slice(&image.pixels()[source..source + 4]);
                    }
                }
                let mut metadata = image.metadata();
                let axis = usize::from(!horizontal);
                // Raw canvas zero deliberately does not fall back to pixels.
                let extent = metadata.canvas_size[axis];
                let size = if horizontal { image.width() } else { image.height() };
                metadata.offset[axis] = extent.wrapping_sub(metadata.offset[axis] as u32)
                    .wrapping_sub(size) as i32;
                metadata.base[axis] = extent.wrapping_sub(metadata.base[axis] as u32) as i32;
                let mut attributes = image.hg_attributes().as_map().clone();
                for values in attributes.values_mut() {
                    values[axis] = extent.wrapping_sub(values[axis]);
                }
                let frame = RgbaFrame::from_native_rgba8(image.width(), image.height(), pixels, metadata)
                    .map_err(|_| native_state_error("native 558 pixel flip failed"))?
                    .with_raw_metadata(metadata)
                    .with_hg_attributes(HgFrameAttributes::new(attributes));
                self.replace_image_wrapper_pixels(handle, frame)?;
                Ok(NativeReturn::Continue)
            }
            641 => {
                let handle = read_u32(arguments, 0)?;
                if handle == 0 { return Ok(NativeReturn::Continue); }
                let percent = read_u32(arguments, 4)?;
                let image = self.image_handles.get(&handle)
                    .ok_or_else(|| native_state_error("native 641 invalid image wrapper"))?;
                if image.metadata().bit_depth == 24 { return Ok(NativeReturn::Continue); }
                if percent == 100 { return Ok(NativeReturn::Continue); }
                let mut pixels = image.pixels().to_vec();
                for pixel in pixels.chunks_exact_mut(4) {
                    pixel[3] = (u32::from(pixel[3]).wrapping_mul(percent) / 100).min(255) as u8;
                }
                let frame = RgbaFrame::from_native_rgba8(image.width(), image.height(), pixels, image.metadata())
                    .map_err(|_| native_state_error("native 641 image rebuild failed"))?
                    .with_raw_metadata(image.metadata())
                    .with_hg_attributes(image.hg_attributes().clone());
                self.replace_image_wrapper_pixels(handle, frame)?;
                Ok(NativeReturn::Continue)
            }
            1026 => {
                let handle = read_u32(arguments, 0)?;
                if handle == 0 { return Ok(NativeReturn::Value(0)); }
                let image = self.image_handles.get(&handle)
                    .ok_or_else(|| native_state_error("native 1026 invalid image wrapper"))?;
                let offset = image.metadata().offset;
                let x = read_u32(arguments, 4)?.wrapping_sub(offset[0] as u32);
                let y = read_u32(arguments, 8)?.wrapping_sub(offset[1] as u32);
                if x >= image.width() || y >= image.height() { return Ok(NativeReturn::Value(0)); }
                let at = ((image.height() - 1 - y) as usize * image.width() as usize + x as usize) * 4;
                let rgba = &image.pixels()[at..at + 4];
                let alpha = if image.metadata().bit_depth == 24 { 0 } else { rgba[3] };
                Ok(NativeReturn::Value(u32::from_le_bytes([rgba[2], rgba[1], rgba[0], alpha])))
            }
            _ => Err(cs2_script::ScriptError::UnimplementedNativeCall { index }),
        }
    }

    fn read_image_header_prefix(&self, name: &[u8]) -> Result<Option<Vec<u8>>> {
        use std::io::Read;
        use cs2_runtime::resource_router::ResourceMode;
        let name = super::ScriptTextEncoding::Cp932.decode(name);
        let names = self.resources.category_names(b"image", &name)
            .map_err(|_| native_state_error("native 375 image category resolution failed"))?;
        for name in names {
            if let Some(bytes) = self.data_services.ordinary.cached_resource(&name) {
                return Ok(Some(bytes[..bytes.len().min(1024)].to_vec()));
            }
            let Some(found) = self.resources.resolve_mode_utf16(
                &name.encode_utf16().collect::<Vec<_>>(), ResourceMode::LooseFirst,
            ).map_err(|_| native_state_error("native 375 image routing failed"))? else { continue; };
            let candidate = found.value;
            let mut source = self.resources.open_media_candidate(candidate, 4 * 1024 * 1024 * 1024)
                .map_err(|_| native_state_error("native 375 image prefix could not be opened"))?;
            let mut bytes = vec![0; candidate.size.min(1024) as usize];
            source.read_exact(&mut bytes)
                .map_err(|_| native_state_error("native 375 image prefix read was truncated"))?;
            return Ok(Some(bytes));
        }
        Ok(None)
    }
}

/// First raw-radius expansion at 5e5a20. It neither filters nor moves base.
pub(super) fn expand_frame_border(source: &RgbaFrame, raw: [u32; 2]) -> Result<RgbaFrame> {
    if source.is_empty() { return Ok(source.clone()); }
    // 5e5a20 returns without a write if ANY signed border is negative.
    if raw.iter().any(|&value| (value as i32) < 0) { return Ok(source.clone()); }
    let mut m = source.metadata();
    let x = u32::try_from(m.offset[0])
        .map_err(|_| native_state_error("resource border begins outside its raw canvas"))?;
    let y = u32::try_from(m.offset[1])
        .map_err(|_| native_state_error("resource border begins outside its raw canvas"))?;
    let room_x = m.canvas_size[0].checked_sub(x).and_then(|n| n.checked_sub(source.width()))
        .ok_or_else(|| native_state_error("resource border crop exceeds its raw canvas"))?;
    let room_y = m.canvas_size[1].checked_sub(y).and_then(|n| n.checked_sub(source.height()))
        .ok_or_else(|| native_state_error("resource border crop exceeds its raw canvas"))?;
    let [left, bottom] = [x.min(raw[0]), y.min(raw[1])];
    let [right, top] = [room_x.min(raw[0]), room_y.min(raw[1])];
    let width = source.width().checked_add(left).and_then(|n| n.checked_add(right))
        .ok_or_else(|| native_state_error("resource border width overflows"))?;
    let height = source.height().checked_add(bottom).and_then(|n| n.checked_add(top))
        .ok_or_else(|| native_state_error("resource border height overflows"))?;
    let length = (width as usize).checked_mul(height as usize).and_then(|n| n.checked_mul(4))
        .filter(|&n| n <= 128 * 1024 * 1024)
        .ok_or_else(|| native_state_error("resource border pixels exceed 128 MiB"))?;
    let mut pixels = zero_blur_pixels(length)?;
    if m.bit_depth == 24 { for p in pixels.chunks_exact_mut(4) { p[3] = 255; } }
    let row = source.width() as usize * 4;
    for y in 0..source.height() as usize {
        let to = ((y + top as usize) * width as usize + left as usize) * 4;
        pixels[to..to + row].copy_from_slice(&source.pixels()[y * row..(y + 1) * row]);
    }
    m.offset = [(x - left) as i32, (y - bottom) as i32];
    // First-stage expansion does not move the base or HG attributes.
    RgbaFrame::from_rgba8(width, height, pixels)
        .map(|frame| frame.with_raw_metadata(m).with_hg_attributes(source.hg_attributes().clone()))
        .map_err(|_| native_state_error("resource border frame construction failed"))
}

/// 5e33b0 dispatches distinct 24/32-bit kernels and adjusts the base.
pub(super) fn blur_wrapper_bgra32(source: &RgbaFrame, raw_radius: [u32; 2]) -> Result<RgbaFrame> {
    // The pixel helper returns false before expansion for an empty allocation;
    // its ordinary wrapper ignores that result and retains the binding.
    if source.is_empty() { return Ok(source.clone()); }
    if source.metadata().bit_depth == 24 { return blur_wrapper_bgr24(source, raw_radius); }
    if source.metadata().bit_depth != 32 {
        return Err(native_state_error("native 538 unsupported native pixel depth"));
    }
    let radius = raw_radius.map(|value| (value as i32).clamp(0, 255) as usize);
    let original = source.metadata();
    let x = u32::try_from(original.offset[0])
        .map_err(|_| native_state_error("native 538 crop begins outside its raw canvas"))?;
    let y = u32::try_from(original.offset[1])
        .map_err(|_| native_state_error("native 538 crop begins outside its raw canvas"))?;
    let right = original.canvas_size[0].checked_sub(x)
        .and_then(|room| room.checked_sub(source.width()))
        .ok_or_else(|| native_state_error("native 538 crop exceeds its raw canvas"))?;
    let top = original.canvas_size[1].checked_sub(y)
        .and_then(|room| room.checked_sub(source.height()))
        .ok_or_else(|| native_state_error("native 538 crop exceeds its raw canvas"))?;
    // The original border helper reads raw canvas dimensions, never the
    // zero-axis fallback getter. Diagnose its negative-copy cases up front.
    let left = x.min(radius[0] as u32);
    let bottom = y.min(radius[1] as u32);
    let right = right.min(radius[0] as u32);
    let top = top.min(radius[1] as u32);
    let width = source.width().checked_add(left).and_then(|size| size.checked_add(right))
        .ok_or_else(|| native_state_error("native 538 expanded width overflow"))?;
    let height = source.height().checked_add(bottom).and_then(|size| size.checked_add(top))
        .ok_or_else(|| native_state_error("native 538 expanded height overflow"))?;
    let length = (width as usize).checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(4)).filter(|&size| size <= 128 * 1024 * 1024)
        .ok_or_else(|| native_state_error("native 538 expanded pixels exceed 128 MiB"))?;
    let mut expanded = zero_blur_pixels(length)?;
    let row_bytes = source.width() as usize * 4;
    for row in 0..source.height() as usize {
        // Native rows start at the bottom; the public frame is top-down.
        let destination = ((row + top as usize) * width as usize + left as usize) * 4;
        expanded[destination..destination + row_bytes]
            .copy_from_slice(&source.pixels()[row * row_bytes..(row + 1) * row_bytes]);
    }
    let mut filtered = zero_blur_pixels(length)?;
    if radius[0] == 0 {
        filtered.copy_from_slice(&expanded);
    } else {
        let weights = triangular_weights(radius[0]);
        for y in 0..height as usize {
            for x in 0..width as usize {
                if let Some(pixel) = blur_pixel(
                    &expanded, [width as usize, height as usize], [x, y], radius[0], &weights, true,
                ) {
                    let start = (y * width as usize + x) * 4;
                    filtered[start..start + 4].copy_from_slice(&pixel);
                }
            }
        }
    }
    if radius[1] != 0 {
        // The PE snapshots each source column before writing it back. One
        // complete horizontal snapshot is equivalent because columns never
        // sample one another, and preserves all-transparent no-write pixels.
        expanded.copy_from_slice(&filtered);
        let weights = triangular_weights(radius[1]);
        for y in 0..height as usize {
            for x in 0..width as usize {
                if let Some(pixel) = blur_pixel(
                    &expanded, [width as usize, height as usize], [x, y], radius[1], &weights, false,
                ) {
                    let start = (y * width as usize + x) * 4;
                    filtered[start..start + 4].copy_from_slice(&pixel);
                }
            }
        }
    }
    let mut metadata = original;
    metadata.offset = [(x - left) as i32, (y - bottom) as i32];
    metadata.base = [
        original.base[0].wrapping_add(left as i32),
        original.base[1].wrapping_add(bottom as i32),
    ];
    RgbaFrame::from_rgba8(width, height, filtered)
        .map(|frame| frame.with_raw_metadata(metadata).with_hg_attributes(source.hg_attributes().clone()))
        .map_err(|_| native_state_error("native 538 filtered frame construction failed"))
}

/// 5e5c30: raw-radius border BEFORE signed clamping, then native BGR sums.
/// The horizontal branch writes only `width` bytes of each `width*3` row
/// into HeapAlloc(flags=0) storage. Reject those uninitialised RGB reads;
/// the zero-horizontal-radius copy plus vertical kernel is fully defined.
fn blur_wrapper_bgr24(source: &RgbaFrame, raw_radius: [u32; 2]) -> Result<RgbaFrame> {
    if raw_radius.iter().any(|&v| (v as i32) < 0) { return Ok(source.clone()); }
    if source.is_empty() { return Ok(source.clone()); }
    let radius = raw_radius.map(|v| (v as i32).clamp(0, 255) as usize);
    if radius[0] != 0 {
        return Err(native_state_error(
            "native 538 BGR24 horizontal blur reads unwritten HeapAlloc(flags=0) RGB bytes"));
    }
    let mut expanded = expand_frame_border(source, raw_radius)?;
    let mut pixels = expanded.pixels().to_vec();
    for pixel in pixels.chunks_exact_mut(4) { pixel[3] = 255; }
    if radius[1] != 0 {
        let weights = triangular_weights(radius[1]);
        let scale = 256 / (radius[1] as u32 + 1);
        let width = expanded.width() as usize;
        let height = expanded.height() as usize;
        // Native scratch selection compares width with height+radius, but
        // the column writer needs height+2*radius triples. Do not silently
        // substitute a bounded result for the original out-of-bounds case.
        if width > height + radius[1] && width < height + radius[1] * 2 {
            return Err(native_state_error("native 538 BGR24 vertical scratch allocation is too short"));
        }
        for y in 0..height {
            for x in 0..width {
                let to = (y * width + x) * 4;
                for channel in 0..3 {
                    let mut sum = 0u32;
                    for (i, &weight) in weights.iter().enumerate() {
                        let row = y.saturating_add(i).saturating_sub(radius[1]).min(height - 1);
                        sum = sum.wrapping_add(u32::from(expanded.pixels()[(row * width + x) * 4 + channel])
                            .wrapping_mul(weight));
                    }
                    pixels[to + channel] = (sum.wrapping_mul(scale) >> 16) as u8;
                }
                pixels[to + 3] = 255;
            }
        }
    }
    let mut m = expanded.metadata();
    let old = source.metadata();
    m.base = [old.base[0].wrapping_add(old.offset[0].wrapping_sub(m.offset[0])),
        old.base[1].wrapping_add(old.offset[1].wrapping_sub(m.offset[1]))];
    expanded = RgbaFrame::from_rgba8(expanded.width(), expanded.height(), pixels)
        .map_err(|_| native_state_error("native 538 BGR24 filtered frame construction failed"))?
        .with_raw_metadata(m).with_hg_attributes(source.hg_attributes().clone());
    Ok(expanded)
}

fn zero_blur_pixels(length: usize) -> Result<Vec<u8>> {
    let mut pixels = Vec::new();
    pixels.try_reserve_exact(length)
        .map_err(|_| native_state_error("native 538 scratch allocation failed"))?;
    pixels.resize(length, 0);
    Ok(pixels)
}

fn triangular_weights(radius: usize) -> Vec<u32> {
    let denominator = radius as u32 + 1;
    (0..=radius * 2).map(|index| {
        ((radius + 1 - index.abs_diff(radius)) as u32 * 256 + radius as u32 / 2) / denominator
    }).collect()
}

fn blur_pixel(
    source: &[u8], size: [usize; 2], at: [usize; 2],
    radius: usize, weights: &[u32], horizontal: bool,
) -> Option<[u8; 4]> {
    let axis = usize::from(!horizontal);
    let pixel = |index: usize| {
        let position = at[axis].saturating_add(index).saturating_sub(radius).min(size[axis] - 1);
        let x = if horizontal { position } else { at[0] };
        let y = if horizontal { at[1] } else { position };
        let start = (y * size[0] + x) * 4;
        &source[start..start + 4]
    };
    let mut mean = [0u32; 3];
    let mut count = 0u32;
    for index in 0..weights.len() {
        let input = pixel(index);
        if input[3] == 0 { continue; }
        for channel in 0..3 { mean[channel] += u32::from(input[channel]); }
        count += 1;
    }
    if count == 0 { return None; }
    for value in &mut mean { *value /= count; }
    let mut sums = [0u32; 4];
    for (index, &weight) in weights.iter().enumerate() {
        let input = pixel(index);
        let alpha = u32::from(input[3]);
        for channel in 0..3 {
            let value = match alpha {
                0 => mean[channel],
                255 => u32::from(input[channel]),
                _ => (u32::from(input[channel]) * alpha + mean[channel] * (255 - alpha)) >> 8,
            };
            sums[channel] = sums[channel].wrapping_add(value.wrapping_mul(weight));
        }
        sums[3] = sums[3].wrapping_add(alpha.wrapping_mul(weight));
    }
    let scale = 0x10000 / (radius as u32 + 1);
    Some(sums.map(|sum| {
        let product = sum.wrapping_mul(scale);
        ((product >> 24) + ((product >> 23) & 1)) as u8
    }))
}

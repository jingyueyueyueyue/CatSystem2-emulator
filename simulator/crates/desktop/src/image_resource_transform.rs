//! Resource transforms visit every retained source frame, not the display cache.
use super::*;

impl LoggingHost<'_> {
    pub(crate) fn transform_image_resource_layers(&mut self, handle: u32, index: u32,
        args: &[u8]) -> Result<bool> {
        if !matches!(index, 713 | 723 | 726 | 727 | 730 | 737) { return Ok(false); }
        let Some(original) = self.data_services.image_manager.resource_layers.get(&handle) else {
            if self.image_resources.get(&handle).is_some_and(|r| r.has_source_layer) {
                return Err(native_state_error("image resource retained source storage is missing"));
            }
            return Ok(false);
        };
        // Prepare without touching live pixels or the ANM continuation. A bad
        // frame must not leave only the earlier directory frames transformed.
        let mut layers = original.clone();
        let resource = self.image_resources.get(&handle)
            .ok_or_else(|| native_state_error("retained image source has no resource"))?;
        let preserve_alpha = resource.uses_alpha;
        let mut geometry = resource.geometry;
        let mut current = resource.image.clone();
        let single_static = layers.layers.len() == 1 && layers.layers[0].animation.timeline().is_none();
        let mut bytes = 0usize;
        for (index_in_list, layer) in layers.layers.iter_mut().enumerate() {
            let previous_offset = layer.frames[&layer.selected].metadata().offset;
            if index == 726 && layer.animation.timeline().is_none() {
                current = crate::image_native::expand_frame_border(&current,
                    [read_u32(args, 4)?, read_u32(args, 8)?])?;
                // 69a93b -> 69a969 records ONLY size/offset at this point.
                geometry.size = [current.width(), current.height()];
                geometry.offset = current.metadata().offset;
                if single_static { layer.frames.insert(layer.selected, current.clone()); }
            }
            for frame in layer.frames.values_mut() {
                let raw_metadata = frame.metadata();
                let transformed = match index {
                    713 => {
                        if frame.is_empty() || frame.metadata().bit_depth == 24 { frame.clone() } else {
                            frame.adjust_rgb_percent([read_u32(args, 4)?, read_u32(args, 8)?, read_u32(args, 12)?])
                                .map_err(|_| native_state_error("resource RGB frame transform failed"))?
                        }
                    }
                    723 => crate::image_transform_native::rotate_frame(frame, read_u32(args, 4)?)?,
                    726 => crate::image_native::blur_wrapper(frame,
                        [read_u32(args, 4)?, read_u32(args, 8)?])?,
                    727 => flip_frame(frame, read_u32(args, 4)? != 0)?,
                    // The PE passes the requested dimensions to 5e4f10 for
                    // EACH frame. Each frame uses its own raw canvas ratio.
                    730 => self.resize_image_canvas(frame,
                        [read_u32(args, 4)?, read_u32(args, 8)?])?,
                    737 => {
                        // All three native pixel helpers decline <=3-byte
                        // storage; their resource wrapper still invalidates.
                        if frame.is_empty() || frame.metadata().bit_depth == 24 { frame.clone() } else {
                            frame.apply_filter(match read_u32(args, 4)? {
                                1 => cs2_renderer::FrameFilter::Mono,
                                2 => cs2_renderer::FrameFilter::Sepia,
                                3 => cs2_renderer::FrameFilter::Negative,
                                _ => return Ok(true),
                            }).map_err(|_| native_state_error("resource filter frame transform failed"))?
                        }
                    }
                    _ => unreachable!(),
                };
                // Pixel-only helpers never normalize raw geometry.
                *frame = if matches!(index, 713 | 737) {
                    transformed.with_raw_metadata(raw_metadata)
                } else { transformed };
                bytes = bytes.checked_add(frame.pixels().len()).filter(|&n| n <= MAX_CACHE)
                    .ok_or_else(|| native_state_error("transformed source frames exceed 256 MiB"))?;
            }
            if matches!(index, 713 | 726 | 737) && let Some(surface) = &mut layers.surface {
                if index == 726 {
                    resource_surface::invalidate_blurred_source(surface, layer,
                        index_in_list, previous_offset)?;
                } else {
                    resource_surface::invalidate(surface, layer, index_in_list, false)?;
                }
            }
        }
        // 713/737 only invalidate the existing list, as does 726. Rebuilding
        // after a colour operation would discard an earlier expanded output.
        if matches!(index, 723 | 727 | 730) {
            layers.geometry = ResourceLayers::directory_geometry(&layers.layers)?;
            let tile = layers.surface.as_ref().map(|s| s.tile);
            layers.surface = resource_surface::create(&layers.layers, &layers.geometry, tile)?;
        }
        // 69a900/69c260/69b140 do not call the compositor. A sole static
        // source aliases current; a composite redraws on its next list update.
        let image = if matches!(index, 713 | 726 | 737) && !single_static { current } else {
            layers.compose()?.ok_or_else(|| native_state_error(
                "resource transform produced an empty retained directory"))?
        };
        if matches!(index, 723 | 727 | 730) { geometry = ResourceGeometry::from_frame(&image); }
        let resource = self.image_resources.get_mut(&handle).unwrap();
        resource.attributes = image.hg_attributes().iter()
            .map(|(&id, &values)| (u32::from(id), values)).collect();
        resource.image = image;
        resource.geometry = geometry;
        resource.uses_alpha = preserve_alpha;
        resource.updated = true;
        layers.changed = true;
        self.data_services.image_manager.resource_layers.insert(handle, layers);
        self.synchronize_list_resource(handle)
            .map_err(|_| native_state_error("resource transform list pixel synchronization failed"))?;
        Ok(true)
    }
}

fn flip_frame(image: &RgbaFrame, horizontal: bool) -> Result<RgbaFrame> {
    let width = image.width() as usize;
    let height = image.height() as usize;
    let mut pixels = image.pixels().to_vec();
    for y in 0..height {
        for x in 0..width {
            let source = (y * width + x) * 4;
            let target = if horizontal { (y * width + width - 1 - x) * 4 }
                else { ((height - 1 - y) * width + x) * 4 };
            pixels[target..target + 4].copy_from_slice(&image.pixels()[source..source + 4]);
        }
    }
    let mut meta = image.metadata();
    let axis = usize::from(!horizontal);
    let extent = meta.canvas_size[axis];
    meta.offset[axis] = extent.wrapping_sub(meta.offset[axis] as u32)
        .wrapping_sub(if horizontal { image.width() } else { image.height() }) as i32;
    meta.base[axis] = extent.wrapping_sub(meta.base[axis] as u32) as i32;
    let mut attributes = image.hg_attributes().as_map().clone();
    for words in attributes.values_mut() { words[axis] = extent.wrapping_sub(words[axis]); }
    RgbaFrame::from_native_rgba8(image.width(), image.height(), pixels, meta)
        .map(|frame| frame.with_raw_metadata(meta).with_hg_attributes(HgFrameAttributes::new(attributes)))
        .map_err(|_| native_state_error("resource flip frame construction failed"))
}

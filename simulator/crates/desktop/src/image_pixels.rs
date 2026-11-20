//! Ordinary image loading, shared pixel identity and bottom-origin filling.
//! Wrapper IDs and underlying pixel IDs use the global allocator but remain
//! different types. Shared wrappers observe updates and survive owner release.
use cs2_renderer::{FrameMetadata, RgbaFrame};
use cs2_script::{NativeReturn, Result, Vm};

use super::{LoggingHost, expand_kcs_format, native_state_error, read_u32, trim_nul};

const MAX_PIXELS: usize = 128 * 1024 * 1024;
const MAX_WRAPPERS: usize = 1_000_000;

impl LoggingHost<'_> {
    pub(super) fn image_pixels_native(
        &mut self, index: u32, args: &[u8], vm: &mut Vm,
    ) -> Result<NativeReturn> {
        match index {
            111 => {
                let raw = vm.read_string(read_u32(args, 0)?)?;
                let names = expand_kcs_format(vm, &raw, 0)?;
                let mut image = None;
                // Only ASCII space is a separator in this native. Tabs,
                // newlines and quote bytes are part of a resource name.
                for name in trim_nul(&names).split(|&byte| byte == b' ').filter(|name| !name.is_empty()) {
                    let (name, id) = image_reference(name)?;
                    let name = self.text_encoding.decode(&name);
                    let candidates = self.resources.category_names(b"image", &name)
                        .map_err(|_| native_state_error("native 111 image category resolution failed"))?;
                    let Some(name) = candidates.iter()
                        .find(|name| self.resources.find(name).is_some()).or_else(|| candidates.first())
                    else { continue; };
                    let Some(next) = self.load_managed_image(name, id, false)? else { continue; };
                    image = Some(match image {
                        None => next,
                        Some(target) => composite_colors(&target, &next)?,
                    });
                }
                Ok(NativeReturn::Value(match image {
                    Some(image) => {
                        // Cache insertion precedes this native's 24→32
                        // conversion, so the independent cache keeps 24-bit
                        // storage metadata until copied into this wrapper.
                        let mut metadata = image.metadata();
                        metadata.bit_depth = 32;
                        self.register_image_wrapper(image.with_raw_metadata(metadata), true)?
                    }
                    None => 0,
                }))
            }
            112 => {
                let size = [read_u32(args, 0)?, read_u32(args, 4)?];
                let length = (size[0] as usize).checked_mul(size[1] as usize)
                    .and_then(|n| n.checked_mul(4)).filter(|&n| n <= MAX_PIXELS)
                    .ok_or_else(|| native_state_error("native 112 image exceeds 128 MiB"))?;
                let mut pixels = Vec::new();
                pixels.try_reserve_exact(length)
                    .map_err(|_| native_state_error("native 112 pixel allocation failed"))?;
                pixels.resize(length, 0);
                let frame = RgbaFrame::from_native_rgba8(size[0], size[1], pixels,
                    FrameMetadata::raster([0, 0]))
                    .map_err(|_| native_state_error("native 112 invalid dimensions"))?;
                Ok(NativeReturn::Value(self.register_image_wrapper(frame, true)?))
            }
            114 => {
                self.release_image_wrapper(read_u32(args, 0)?)?;
                Ok(NativeReturn::Continue)
            }
            117 => {
                let handle = read_u32(args, 0)?;
                let x = read_u32(args, 4)? as i32;
                let y = read_u32(args, 8)? as i32;
                let width = read_u32(args, 12)? as i32;
                let height = read_u32(args, 16)? as i32;
                if handle == 0 {
                    self.clear_graphics_canvas_rect([x, y, width, height])
                        .map_err(|_| native_state_error("native 117 canvas clear failed"))?;
                    return Ok(NativeReturn::Continue);
                }
                let color = super::packed_color(read_u32(args, 20)?);
                let frame = self.image_handles.get(&handle).cloned()
                    .ok_or_else(|| native_state_error("native 117 invalid image wrapper"))?;
                let mut pixels = frame.pixels().to_vec();
                let right = x.wrapping_add(width);
                let bottom = y.wrapping_add(height);
                let left = x.max(0).min(frame.width() as i32);
                let top = y.max(0).min(frame.height() as i32);
                let right = right.max(0).min(frame.width() as i32);
                let bottom = bottom.max(0).min(frame.height() as i32);
                if left < right {
                    let stride = frame.width() as usize * 4;
                    for row in top..bottom {
                        let start = (frame.height() as usize - 1 - row as usize) * stride;
                        pixels[start + left as usize * 4..start + right as usize * 4]
                            .as_chunks_mut::<4>().0.fill([color.r, color.g, color.b, color.a]);
                    }
                }
                self.replace_image_wrapper_pixels(handle, rebuild(&frame, pixels)?)?;
                Ok(NativeReturn::Continue)
            }
            _ => Err(cs2_script::ScriptError::UnimplementedNativeCall { index }),
        }
    }

    pub(super) fn register_image_wrapper(&mut self, frame: RgbaFrame, owned: bool) -> Result<u32> {
        if self.image_handles.len() >= MAX_WRAPPERS || frame.pixels().len() > MAX_PIXELS {
            return Err(native_state_error("ordinary image storage limit exceeded"));
        }
        let handle = self.allocate_handle();
        let pixel = self.allocate_handle();
        self.image_handles.insert(handle, frame);
        self.data_services.image_wrapper_pixels.insert(handle, pixel);
        if owned { self.data_services.image_wrapper_owners.insert(handle, self.current_task_id); }
        Ok(handle)
    }

    pub(super) fn share_image_wrapper(&mut self, source: u32) -> Result<u32> {
        if source == 0 { return Ok(0); }
        let frame = self.image_handles.get(&source).cloned()
            .ok_or_else(|| native_state_error("native 118 invalid image wrapper"))?;
        if self.image_handles.len() >= MAX_WRAPPERS {
            return Err(native_state_error("image wrapper count exceeded"));
        }
        let pixel = match self.data_services.image_wrapper_pixels.get(&source) {
            Some(&pixel) => pixel,
            None => {
                let pixel = self.allocate_handle();
                self.data_services.image_wrapper_pixels.insert(source, pixel);
                pixel
            }
        };
        let handle = self.allocate_handle();
        self.image_handles.insert(handle, frame);
        self.data_services.image_wrapper_pixels.insert(handle, pixel);
        if let Some(value) = self.data_services.image_control_values.get(&source).copied() {
            self.data_services.image_control_values.insert(handle, value);
        }
        // share() has no owner registration. Native 114 releases the alias.
        Ok(handle)
    }

    pub(super) fn replace_image_wrapper_pixels(&mut self, handle: u32, frame: RgbaFrame) -> Result<()> {
        if !self.image_handles.contains_key(&handle) {
            return Err(native_state_error("image pixel update has an invalid wrapper"));
        }
        let aliases = self.data_services.image_wrapper_pixels.get(&handle).copied()
            .map(|pixel| self.data_services.image_wrapper_pixels.iter()
                .filter_map(|(&wrapper, &candidate)| (candidate == pixel).then_some(wrapper))
                .collect::<Vec<_>>())
            .unwrap_or_else(|| vec![handle]);
        for alias in aliases {
            if self.image_handles.contains_key(&alias) { self.image_handles.insert(alias, frame.clone()); }
        }
        Ok(())
    }

    pub(super) fn release_image_wrapper(&mut self, handle: u32) -> Result<()> {
        if handle == 0 { return Ok(()); }
        if self.image_handles.remove(&handle).is_none() {
            return Err(native_state_error("native 114 invalid image wrapper"));
        }
        self.data_services.release_image_pixel_identity(handle);
        self.data_services.image_control_values.remove(&handle);
        self.data_services.image_wrapper_owners.remove(&handle);
        Ok(())
    }
}

fn rebuild(frame: &RgbaFrame, pixels: Vec<u8>) -> Result<RgbaFrame> {
    RgbaFrame::from_native_rgba8(frame.width(), frame.height(), pixels, frame.metadata())
        .map(|rebuilt| rebuilt.with_raw_metadata(frame.metadata()).with_hg_attributes(frame.hg_attributes().clone()))
        .map_err(|_| native_state_error("ordinary image pixel rebuild failed"))
}

/// Native 111 deliberately uses opacity zero. It seeds transparent RGB only
/// where the destination DWORD is entirely zero, preserving the first image's
/// geometry/alpha. Both horizontal neighbour pixels follow the same rule.
fn composite_colors(target: &RgbaFrame, source: &RgbaFrame) -> Result<RgbaFrame> {
    let mut pixels = target.pixels().to_vec();
    let to = target.metadata().offset;
    let from = source.metadata().offset;
    let left = i64::from(to[0]).max(i64::from(from[0]));
    let top = i64::from(to[1]).max(i64::from(from[1]));
    let right = (i64::from(to[0]) + i64::from(target.width()))
        .min(i64::from(from[0]) + i64::from(target.width()));
    let bottom = (i64::from(to[1]) + i64::from(target.height()))
        .min(i64::from(from[1]) + i64::from(target.height()));
    if right <= left || bottom <= top { return Ok(target.clone()); }
    let x = (left - i64::from(to[0])) as usize;
    let y = (top - i64::from(to[1])) as usize;
    let width = (right - left).min(i64::from(source.width())) as usize;
    let height = (bottom - top).min(i64::from(source.height())) as usize;
    for row in 0..height {
        let source_y = source.height() as usize - 1 - row;
        let Some(target_y) = (target.height() as usize).checked_sub(y + row + 1) else { continue; };
        for column in 0..width {
            let source_offset = (source_y * source.width() as usize + column) * 4;
            let target_offset = (target_y * target.width() as usize + x + column) * 4;
            seed_rgb(&mut pixels[target_offset..target_offset + 4], &source.pixels()[source_offset..source_offset + 4]);
        }
        if width != 0 {
            if x != 0 {
                let offset = (target_y * target.width() as usize + x - 1) * 4;
                let source_offset = source_y * source.width() as usize * 4;
                seed_rgb(&mut pixels[offset..offset + 4], &source.pixels()[source_offset..source_offset + 4]);
            }
            if x + width < target.width() as usize {
                let offset = (target_y * target.width() as usize + x + width) * 4;
                let source_offset = (source_y * source.width() as usize + width - 1) * 4;
                seed_rgb(&mut pixels[offset..offset + 4], &source.pixels()[source_offset..source_offset + 4]);
            }
        }
    }
    rebuild(target, pixels)
}

fn seed_rgb(target: &mut [u8], source: &[u8]) {
    if target.iter().all(|&byte| byte == 0) { target[..3].copy_from_slice(&source[..3]); }
}

pub(super) fn image_reference(input: &[u8]) -> Result<(Vec<u8>, u32)> {
    let input = trim_nul(input);
    if input.len() >= 260 { return Err(native_state_error("native 111 image name exceeds 259 bytes")); }
    let Some(colon) = input.iter().position(|&byte| byte == b':') else { return Ok((input.to_vec(), 0)); };
    let slash = input.iter().rposition(|&byte| matches!(byte, b'/' | b'\\'));
    let dot = input.iter().rposition(|&byte| byte == b'.').filter(|&dot| slash.is_none_or(|slash| dot > slash));
    let mut name = input[..dot.unwrap_or(input.len())].to_vec();
    let number_end = if colon < name.len() { name.len() } else { input.len() };
    let id = integer_prefix(&input[colon + 1..number_end]);
    if colon < name.len() { name.truncate(colon); }
    if let Some(dot) = name.iter().rposition(|&byte| byte == b'.')
        .filter(|&dot| name.iter().rposition(|&byte| matches!(byte, b'/' | b'\\')).is_none_or(|slash| dot > slash))
    { name.truncate(dot); }
    name.push(b'.');
    if let Some(dot) = dot { name.extend_from_slice(&input[dot + 1..]); }
    Ok((name, id))
}

fn integer_prefix(bytes: &[u8]) -> u32 {
    let mut at = usize::from(matches!(bytes.first(), Some(b'+' | b'-')));
    let negative = bytes.first() == Some(&b'-');
    let radix = if bytes.get(at..).is_some_and(|bytes| bytes.starts_with(b"0x") || bytes.starts_with(b"0X")) {
        at += 2; 16
    } else { 10 };
    let mut value = 0_u32;
    while let Some(&byte) = bytes.get(at) {
        let digit = match byte {
            b'0'..=b'9' => u32::from(byte - b'0'),
            b'a'..=b'f' => u32::from(byte - b'a' + 10),
            b'A'..=b'F' => u32::from(byte - b'A' + 10),
            _ => break,
        };
        if digit >= radix { break; }
        value = value.wrapping_mul(radix).wrapping_add(digit);
        at += 1;
    }
    if radix == 10 && bytes.get(at) == Some(&b'.') { return 0; }
    if negative { value.wrapping_neg() } else { value }
}

use crate::util::{MAX_BUFFER, MAX_ITEMS, inflate_exact, range, u32_at};
use image::{ImageReader, Limits};
use std::io::Cursor;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HgTag {
    pub name: [u8; 8],
    pub offset_next: u32,
    pub length: u32,
    pub data_offset: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HgImageData {
    pub data_offset: u64,
    pub compressed_data_length: u32,
    pub decompressed_data_length: u32,
    pub compressed_command_length: u32,
    pub decompressed_command_length: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HgImageSlice {
    pub id: u32,
    pub start_row: u32,
    pub row_count: u32,
    pub image: HgImageData,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HgImageEncoding {
    Jpeg,
    Webp,
    Jpa,
    Alpha,
    JpegAlpha,
    WebpAlpha,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HgEncodedImage {
    pub encoding: HgImageEncoding,
    pub data_offset: Option<u64>,
    pub data_length: u32,
    pub alpha_data_offset: Option<u64>,
    pub alpha_compressed_length: Option<u32>,
    pub alpha_decompressed_length: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HgPixelFormat {
    /// Four tightly packed bytes per pixel in blue, green, red, alpha order.
    Bgra8,
    /// Four tightly packed bytes per pixel in red, green, blue, alpha order.
    Rgba8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HgAttribute {
    pub id: u32,
    pub values: [u32; 5],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HgFrame {
    pub id: u32,
    pub width: u32,
    pub height: u32,
    pub pixels_rgba: Vec<u8>,
    pub bit_depth: u32,
    pub offset: [i32; 2],
    pub canvas_size: [u32; 2],
    pub is_transparent: u32,
    pub base: [i32; 2],
    pub frame_offset: u64,
    pub tags: Vec<HgTag>,
    pub image: Option<HgImageData>,
    pub encoded_image: Option<HgEncodedImage>,
    pub attributes: Vec<HgAttribute>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HgFile {
    pub signature: [u8; 4],
    pub header_size: u32,
    pub version: u32,
    pub frames: Vec<HgFrame>,
}

#[derive(Debug, Error)]
pub enum HgError {
    #[error("HG file is truncated")]
    Truncated,
    #[error("HG file exceeds the format reader limit")]
    TooLarge,
    #[error("unsupported HG signature")]
    InvalidSignature,
    #[error("HG frame is invalid")]
    InvalidFrame,
    #[error("HG compressed payload is invalid")]
    InvalidCompression,
    #[error("HG header is unsupported")]
    InvalidHeader,
    #[error("HG image encoding is unsupported")]
    UnsupportedImageEncoding,
    #[error("HG encoded image payload cannot be decoded")]
    InvalidEncodedImage,
    #[error("HG image slice is unsupported")]
    UnsupportedImageSlice,
    #[error("HG native image slices are invalid: {0}")]
    InvalidImageSlices(&'static str),
    #[error("HG pixel bit depth {0} is unsupported")]
    UnsupportedBitDepth(u32),
    #[error("HG image dimensions are invalid")]
    InvalidDimensions,
    #[error("HG pixel command stream is invalid")]
    InvalidPixelCommands,
    #[error("HG pixel output is too large")]
    PixelOutputTooLarge,
}

impl HgFile {
    pub fn parse(data: &[u8]) -> Result<Self, HgError> {
        if data.len() > MAX_BUFFER {
            return Err(HgError::TooLarge);
        }
        let signature: [u8; 4] = data.get(..4).ok_or(HgError::Truncated)?.try_into().unwrap();
        match &signature {
            b"HG-3" => parse_hg3(data),
            b"HG-2" => parse_hg2(data),
            _ => Err(HgError::InvalidSignature),
        }
    }

    /// Ordinary 431's metadata-only HG aggregate. Width/height are separate
    /// signed maxima, not the union of crop right/bottom coordinates. Its
    /// cleared depth slot stays zero. HG-3 takes the first nonsentinel base;
    /// HG-2 0x25 instead takes its signed minimum over all resolved frames.
    pub fn native_aggregate_metadata(&self) -> Result<[u32; 9], HgError> {
        let hg2 = self.signature == *b"HG-2";
        if (hg2 && self.version as u16 != 0x25)
            || (!hg2 && self.signature != *b"HG-3") {
            // The HG-2 aggregate has an explicit signed version >= 0x25
            // gate. Its older-version stack branches are unreachable for
            // immutable input and cannot define a portable older aggregate.
            return Err(HgError::InvalidHeader);
        }
        let mut result = [0u32; 9];
        for index in [0, 1, 7, 8] { result[index] = i32::MAX as u32; }
        let mut first_by_id = std::collections::BTreeMap::new();
        for frame in &self.frames {
            // The PE enumerates frame records but resolves metadata by ID
            // again, selecting the first matching record for duplicate IDs.
            let frame = *first_by_id.entry(frame.id).or_insert(frame);
            result[0] = (result[0] as i32).min(frame.offset[0]) as u32;
            result[1] = (result[1] as i32).min(frame.offset[1]) as u32;
            for (index, value) in [
                (2, frame.width), (3, frame.height),
                (4, frame.canvas_size[0]), (5, frame.canvas_size[1]),
            ] {
                result[index] = (result[index] as i32).max(value as i32) as u32;
            }
            for axis in 0..2 {
                if hg2 {
                    result[7 + axis] = (result[7 + axis] as i32).min(frame.base[axis]) as u32;
                } else if result[7 + axis] == i32::MAX as u32 {
                    result[7 + axis] = frame.base[axis] as u32;
                }
            }
        }
        Ok(result)
    }

    /// Decode one frame to tightly packed top-down BGRA8 pixels.
    pub fn decode_frame(&self, data: &[u8], frame_index: usize) -> Result<Vec<u8>, HgError> {
        self.decode_frame_with_format(data, frame_index, HgPixelFormat::Bgra8)
    }

    /// Decode one frame to tightly packed top-down RGBA8 pixels.
    pub fn decode_frame_rgba8(&self, data: &[u8], frame_index: usize) -> Result<Vec<u8>, HgError> {
        self.decode_frame_with_format(data, frame_index, HgPixelFormat::Rgba8)
    }

    /// Decode one frame using the requested tightly packed 8-bit channel order.
    pub fn decode_frame_with_format(
        &self,
        data: &[u8],
        frame_index: usize,
        format: HgPixelFormat,
    ) -> Result<Vec<u8>, HgError> {
        self.frames
            .get(frame_index)
            .ok_or(HgError::InvalidFrame)?
            .decode_pixels_with_format(data, format)
    }
}

impl HgFrame {
    /// Decode this frame to tightly packed top-down BGRA8 pixels.
    ///
    /// The frame must have come from parsing `data`. The returned buffer has
    /// exactly `width * height * 4` bytes and does not include row padding.
    pub fn decode_pixels(&self, data: &[u8]) -> Result<Vec<u8>, HgError> {
        self.decode_pixels_with_format(data, HgPixelFormat::Bgra8)
    }

    /// Decode this frame to tightly packed top-down RGBA8 pixels.
    pub fn decode_pixels_rgba8(&self, data: &[u8]) -> Result<Vec<u8>, HgError> {
        self.decode_pixels_with_format(data, HgPixelFormat::Rgba8)
    }

    /// Decode this frame using the requested tightly packed 8-bit channel order.
    pub fn decode_pixels_with_format(
        &self,
        data: &[u8],
        format: HgPixelFormat,
    ) -> Result<Vec<u8>, HgError> {
        let signature = data.get(..4).ok_or(HgError::Truncated)?;
        if signature != b"HG-2" && signature != b"HG-3" {
            return Err(HgError::InvalidSignature);
        }
        if signature == b"HG-3" {
            decode_hg3_pixels(self, data, format)
        } else {
            decode_hg2_pixels(self, data, format)
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct PixelLayout {
    depth_bytes: usize,
    stride: usize,
    visible_bytes: usize,
}

impl PixelLayout {
    fn for_frame(frame: &HgFrame) -> Result<Self, HgError> {
        Self::for_dimensions(frame.width, frame.height, frame.bit_depth)
    }

    fn for_dimensions(width: u32, height: u32, bit_depth: u32) -> Result<Self, HgError> {
        if width == 0 || height == 0 {
            return Err(HgError::InvalidDimensions);
        }
        // The native pixel reader selects depth through a signed WORD.
        // Preserve the complete DWORD as metadata instead of rejecting its
        // unrelated high bits or silently normalizing the stored header.
        let depth_bytes = match bit_depth as u16 as i16 {
            24 => 3,
            32 => 4,
            _ => return Err(HgError::UnsupportedBitDepth(bit_depth)),
        };
        let width = usize::try_from(width).map_err(|_| HgError::InvalidDimensions)?;
        let height = usize::try_from(height).map_err(|_| HgError::InvalidDimensions)?;
        let row_bytes = width
            .checked_mul(depth_bytes)
            .ok_or(HgError::PixelOutputTooLarge)?;
        // Both HG native readers store width * channel count as their
        // predictor stride. Four-plane run padding belongs to the complete
        // expanded scratch buffer, not to every visible raster row.
        let stride = row_bytes;
        let raw_size = stride
            .checked_mul(height)
            .ok_or(HgError::PixelOutputTooLarge)?;
        let visible_bytes = width
            .checked_mul(height)
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or(HgError::PixelOutputTooLarge)?;
        if raw_size > MAX_BUFFER || visible_bytes > MAX_BUFFER {
            return Err(HgError::PixelOutputTooLarge);
        }
        Ok(Self {
            depth_bytes,
            stride,
            visible_bytes,
        })
    }
}

fn decode_hg3_pixels(
    frame: &HgFrame,
    data: &[u8],
    format: HgPixelFormat,
) -> Result<Vec<u8>, HgError> {
    if u32_at(data, 4) != Some(12) || u32_at(data, 8) != Some(0x300) {
        return Err(HgError::InvalidHeader);
    }
    if let Some(encoded) = frame.encoded_image.as_ref() {
        return decode_encoded_pixels(frame, data, encoded, format);
    }
    let layout = PixelLayout::for_frame(frame)?;
    let slices = native_image_slices(frame, data)?;
    if slices.len() == 1 {
        let (literals, commands) = read_pixel_streams(data, &slices[0].image)?;
        return decode_native_pixels(
            &literals, &commands, frame.height, layout, format, 0, NativeRowOrder::BottomUp,
        );
    }
    let mut pixels = vec![0; layout.visible_bytes];
    let row_bytes = (frame.width as usize).checked_mul(4).ok_or(HgError::PixelOutputTooLarge)?;
    for slice in slices {
        let (literals, commands) = read_pixel_streams(data, &slice.image)?;
        let layout = PixelLayout::for_dimensions(frame.width, slice.row_count, frame.bit_depth)?;
        // Each slice starts with a horizontal predictor. Later rows predict
        // only from rows in the same slice, never from the preceding tag.
        let decoded = decode_native_pixels(
            &literals, &commands, slice.row_count, layout, format, 0, NativeRowOrder::BottomUp,
        )?;
        let start = (slice.start_row as usize).checked_mul(row_bytes)
            .ok_or(HgError::PixelOutputTooLarge)?;
        let end = start.checked_add(decoded.len()).ok_or(HgError::PixelOutputTooLarge)?;
        pixels.get_mut(start..end).ok_or(HgError::InvalidFrame)?.copy_from_slice(&decoded);
    }
    Ok(pixels)
}

/// Native HG-3 slice fields, independently recovered from the original reader:
/// DWORD 0 is a top-origin starting row; DWORD 1 is the number of rows.
/// The reader searches canonical img0000, img0001, ... names independently of
/// tag order. Require full, nonoverlapping coverage rather than expose any
/// uninitialized rows from malformed original input.
pub fn native_image_slices(frame: &HgFrame, data: &[u8]) -> Result<Vec<HgImageSlice>, HgError> {
    let mut slices = Vec::new();
    let mut expanded = [0usize; 2];
    for tag in &frame.tags {
        let name = &tag.name[..tag.name.iter().position(|&byte| byte == 0).unwrap_or(8)];
        if !is_numbered_tag(name, b"img") {
            continue;
        }
        let id = name[3..].iter().try_fold(0u32, |id, byte|
            id.checked_mul(10)?.checked_add(u32::from(*byte - b'0')))
            .ok_or(HgError::InvalidImageSlices("slice number overflow"))?;
        if format!("img{id:04}").as_bytes() != name {
            return Err(HgError::InvalidImageSlices("noncanonical slice name"));
        }
        let offset = usize_from_u64(tag.data_offset)?;
        let payload = range(data, offset, tag.length as usize).ok_or(HgError::Truncated)?;
        if payload.len() < 24 {
            return Err(HgError::InvalidImageSlices("short slice header"));
        }
        let start_row = u32_at(payload, 0).ok_or(HgError::Truncated)?;
        let row_count = u32_at(payload, 4).ok_or(HgError::Truncated)?;
        if row_count == 0 || start_row.checked_add(row_count).is_none_or(|end| end > frame.height) {
            return Err(HgError::InvalidImageSlices("slice rows escape the frame"));
        }
        let image = HgImageData {
            data_offset: tag.data_offset.checked_add(24).ok_or(HgError::InvalidFrame)?,
            compressed_data_length: u32_at(payload, 8).ok_or(HgError::Truncated)?,
            decompressed_data_length: u32_at(payload, 12).ok_or(HgError::Truncated)?,
            compressed_command_length: u32_at(payload, 16).ok_or(HgError::Truncated)?,
            decompressed_command_length: u32_at(payload, 20).ok_or(HgError::Truncated)?,
        };
        if 24usize.checked_add(image.compressed_data_length as usize)
            .and_then(|size| size.checked_add(image.compressed_command_length as usize))
            .is_none_or(|size| size > payload.len())
        {
            return Err(HgError::InvalidImageSlices("compressed stream escapes its slice"));
        }
        for (total, length) in expanded.iter_mut().zip([
            image.decompressed_data_length, image.decompressed_command_length,
        ]) {
            *total = total.checked_add(length as usize).filter(|&total| total <= MAX_BUFFER)
                .ok_or(HgError::PixelOutputTooLarge)?;
        }
        slices.push(HgImageSlice { id, start_row, row_count, image });
    }
    if slices.is_empty() {
        return Err(HgError::InvalidImageSlices("no native image payload"));
    }
    slices.sort_unstable_by_key(|slice| slice.id);
    if slices.iter().enumerate().any(|(index, slice)| slice.id as usize != index) {
        return Err(HgError::InvalidImageSlices("missing or duplicate slice number"));
    }
    let mut rows: Vec<_> = slices.iter().map(|slice| (slice.start_row, slice.row_count)).collect();
    rows.sort_unstable();
    let mut next_row = 0;
    for (start, count) in rows {
        if start != next_row {
            return Err(HgError::InvalidImageSlices("overlapping or missing slice rows"));
        }
        next_row = start.checked_add(count).ok_or(HgError::InvalidFrame)?;
    }
    if next_row != frame.height {
        return Err(HgError::InvalidImageSlices("native image does not cover the full frame"));
    }
    Ok(slices)
}

fn decode_encoded_pixels(
    frame: &HgFrame,
    data: &[u8],
    encoded: &HgEncodedImage,
    format: HgPixelFormat,
) -> Result<Vec<u8>, HgError> {
    let width = usize::try_from(frame.width).map_err(|_| HgError::InvalidDimensions)?;
    let height = usize::try_from(frame.height).map_err(|_| HgError::InvalidDimensions)?;
    if width == 0 || height == 0 {
        return Err(HgError::InvalidDimensions);
    }
    let pixel_count = width
        .checked_mul(height)
        .ok_or(HgError::PixelOutputTooLarge)?;
    let output_length = pixel_count.checked_mul(4)
        .filter(|&length| length <= MAX_BUFFER)
        .ok_or(HgError::PixelOutputTooLarge)?;
    let expected_alpha = pixel_count;

    if matches!(encoded.encoding, HgImageEncoding::Jpa) {
        return Err(HgError::UnsupportedImageEncoding);
    }
    let mut rgba = if let Some(offset) = encoded.data_offset {
        let image_length =
            usize::try_from(encoded.data_length).map_err(|_| HgError::InvalidFrame)?;
        let payload =
            range(data, usize_from_u64(offset)?, image_length).ok_or(HgError::Truncated)?;
        let mut reader = ImageReader::new(Cursor::new(payload))
            .with_guessed_format()
            .map_err(|_| HgError::InvalidEncodedImage)?;
        let format_matches = matches!(
            (encoded.encoding, reader.format()),
            (HgImageEncoding::Jpeg | HgImageEncoding::JpegAlpha, Some(image::ImageFormat::Jpeg))
                | (HgImageEncoding::Webp | HgImageEncoding::WebpAlpha, Some(image::ImageFormat::WebP))
        );
        if !format_matches {
            return Err(HgError::InvalidEncodedImage);
        }
        let mut limits = Limits::default();
        limits.max_image_width = Some(frame.width);
        limits.max_image_height = Some(frame.height);
        limits.max_alloc = Some(MAX_BUFFER as u64);
        reader.limits(limits);
        let decoded = reader
            .decode()
            .map_err(|_| HgError::InvalidEncodedImage)?
            .to_rgba8();
        if decoded.width() != frame.width || decoded.height() != frame.height {
            return Err(HgError::InvalidDimensions);
        }
        decoded.into_raw()
    } else {
        if !matches!(encoded.encoding, HgImageEncoding::Alpha) {
            return Err(HgError::InvalidFrame);
        }
        vec![255; output_length]
    };

    if let (Some(alpha_offset), Some(alpha_compressed_length), Some(alpha_decompressed_length)) = (
        encoded.alpha_data_offset,
        encoded.alpha_compressed_length,
        encoded.alpha_decompressed_length,
    ) {
        let compressed_length =
            usize::try_from(alpha_compressed_length).map_err(|_| HgError::InvalidFrame)?;
        let compressed = range(data, usize_from_u64(alpha_offset)?, compressed_length)
            .ok_or(HgError::Truncated)?;
        let decompressed_length =
            usize::try_from(alpha_decompressed_length).map_err(|_| HgError::InvalidFrame)?;
        let alpha =
            inflate_exact(compressed, decompressed_length).ok_or(HgError::InvalidCompression)?;
        if alpha.len() < expected_alpha {
            return Err(HgError::InvalidFrame);
        }
        match encoded.encoding {
            HgImageEncoding::Alpha => {
                for (index, pixel) in rgba.chunks_exact_mut(4).enumerate() {
                    let gray = 255u8.wrapping_sub(alpha[index]);
                    pixel.copy_from_slice(&[gray, gray, gray, 255]);
                }
            }
            HgImageEncoding::JpegAlpha | HgImageEncoding::WebpAlpha => {
                for (index, pixel) in rgba.chunks_exact_mut(4).enumerate() {
                    pixel[3] = alpha[index];
                }
            }
            _ => return Err(HgError::InvalidFrame),
        }
    }

    if matches!(format, HgPixelFormat::Bgra8) {
        for pixel in rgba.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }
    }
    Ok(rgba)
}

fn decode_hg2_pixels(
    frame: &HgFrame,
    data: &[u8],
    format: HgPixelFormat,
) -> Result<Vec<u8>, HgError> {
    let version = u32_at(data, 8).ok_or(HgError::Truncated)? as u16;
    if !matches!(version, 0x10 | 0x20 | 0x25) {
        return Err(HgError::InvalidHeader);
    }
    let image = frame.image.as_ref().ok_or(HgError::InvalidFrame)?;
    let layout = PixelLayout::for_frame(frame)?;
    let (literals, commands) = read_pixel_streams(data, image)?;
    decode_native_pixels(
        &literals, &commands, frame.height, layout, format,
        (frame.bit_depth >> 16) as u16 as i16, NativeRowOrder::TopDown,
    )
}

fn read_pixel_streams(data: &[u8], image: &HgImageData) -> Result<(Vec<u8>, Vec<u8>), HgError> {
    let data_offset = usize_from_u64(image.data_offset)?;
    let data_length =
        usize::try_from(image.compressed_data_length).map_err(|_| HgError::InvalidFrame)?;
    let command_length =
        usize::try_from(image.compressed_command_length).map_err(|_| HgError::InvalidFrame)?;
    let command_offset = data_offset
        .checked_add(data_length)
        .ok_or(HgError::InvalidFrame)?;
    let compressed_data = range(data, data_offset, data_length).ok_or(HgError::Truncated)?;
    let compressed_commands =
        range(data, command_offset, command_length).ok_or(HgError::Truncated)?;
    let decompressed_data_length =
        usize::try_from(image.decompressed_data_length).map_err(|_| HgError::InvalidFrame)?;
    let decompressed_command_length =
        usize::try_from(image.decompressed_command_length).map_err(|_| HgError::InvalidFrame)?;
    let literals = inflate_exact(compressed_data, decompressed_data_length)
        .ok_or(HgError::InvalidCompression)?;
    let commands = inflate_exact(compressed_commands, decompressed_command_length)
        .ok_or(HgError::InvalidCompression)?;
    Ok((literals, commands))
}

enum NativeRowOrder {
    TopDown,
    BottomUp,
}

fn decode_native_pixels(
    literals: &[u8],
    commands: &[u8],
    frame_height: u32,
    layout: PixelLayout,
    format: HgPixelFormat,
    channel_precision: i16,
    row_order: NativeRowOrder,
) -> Result<Vec<u8>, HgError> {
    let height = usize::try_from(frame_height).map_err(|_| HgError::InvalidDimensions)?;
    let minimum_size = layout
        .stride
        .checked_mul(height)
        .ok_or(HgError::PixelOutputTooLarge)?;
    let planes = expand_runs(literals, commands, minimum_size)?;
    let mut decoded = vec![0; planes.len()];
    let plane_size = planes.len() / 4;
    if plane_size == 0 || planes.len() % 4 != 0 {
        return Err(HgError::InvalidPixelCommands);
    }

    for (index, pixel) in decoded.chunks_exact_mut(4).enumerate() {
        for (channel, value) in pixel.iter_mut().enumerate() {
            let mut packed = 0u8;
            for plane in 0..4 {
                let source = planes[plane * plane_size + index];
                let pair = (source >> (channel * 2)) & 3;
                packed |= pair << (6 - plane * 2);
            }
            *value = (packed >> 1) ^ 0u8.wrapping_sub(packed & 1);
        }
    }

    for index in layout.depth_bytes..layout.stride {
        decoded[index] = decoded[index].wrapping_add(decoded[index - layout.depth_bytes]);
    }
    for row in 1..height {
        let start = row * layout.stride;
        let previous = start - layout.stride;
        for offset in 0..layout.stride {
            decoded[start + offset] =
                decoded[start + offset].wrapping_add(decoded[previous + offset]);
        }
    }
    if channel_precision != 0 {
        // HG-2's high WORD is a separate signed channel-precision field.
        // Preserve x86's masked DWORD/byte shift counts and the distinct
        // unmasked byte remainder path, including unusual raw WORD values.
        let bits = i32::from(channel_precision);
        let left = 8i32.wrapping_sub(bits) as u32 & 31;
        let right = bits.wrapping_sub(8i32.wrapping_sub(bits)) as u32 & 31;
        let mask = (0xffu32.wrapping_shl(left) & 0xff).wrapping_mul(0x0101_0101);
        for row in decoded[..minimum_size].chunks_exact_mut(layout.stride) {
            let mut words = row.chunks_exact_mut(4);
            for word in words.by_ref() {
                let value = u32::from_le_bytes([word[0], word[1], word[2], word[3]]);
                let expanded = (value.wrapping_shr(right) & !mask)
                    | (value.wrapping_shl(left) & mask);
                word.copy_from_slice(&expanded.to_le_bytes());
            }
            for byte in words.into_remainder() {
                let value = u32::from(*byte);
                *byte = (value.wrapping_shr(right) | value.wrapping_shl(left)) as u8;
            }
        }
    }

    let mut output = Vec::with_capacity(layout.visible_bytes);
    for row in 0..height {
        let source_row = match row_order {
            NativeRowOrder::TopDown => row,
            NativeRowOrder::BottomUp => height - 1 - row,
        };
        let start = source_row * layout.stride;
        for pixel in decoded[start..start + layout.stride].chunks_exact(layout.depth_bytes) {
            match (layout.depth_bytes, format) {
                (3, HgPixelFormat::Bgra8) => {
                    output.extend_from_slice(pixel);
                    output.push(255);
                }
                (3, HgPixelFormat::Rgba8) => {
                    output.extend_from_slice(&[pixel[2], pixel[1], pixel[0], 255]);
                }
                (4, HgPixelFormat::Bgra8) => output.extend_from_slice(pixel),
                (4, HgPixelFormat::Rgba8) => {
                    output.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
                }
                _ => return Err(HgError::InvalidFrame),
            }
        }
    }
    Ok(output)
}

fn expand_runs(literals: &[u8], commands: &[u8], minimum_size: usize) -> Result<Vec<u8>, HgError> {
    let mut bits = BitReader::new(commands);
    let mut copy = bits.bit()?;
    let size = bits.gamma()?;
    if size < minimum_size {
        return Err(HgError::InvalidPixelCommands);
    }
    if size > MAX_BUFFER || size % 4 != 0 {
        return Err(HgError::InvalidPixelCommands);
    }

    let mut output = vec![0; size];
    let mut position = 0usize;
    let mut source = 0usize;
    while position < size {
        let count = bits.gamma()?;
        let end = position
            .checked_add(count)
            .filter(|&end| end <= size)
            .ok_or(HgError::InvalidPixelCommands)?;
        if copy {
            let source_end = source
                .checked_add(count)
                .ok_or(HgError::InvalidPixelCommands)?;
            let input = literals.get(source..source_end).ok_or(HgError::Truncated)?;
            output[position..end].copy_from_slice(input);
            source = source_end;
        }
        position = end;
        copy = !copy;
    }
    Ok(output)
}

fn usize_from_u64(value: u64) -> Result<usize, HgError> {
    usize::try_from(value).map_err(|_| HgError::InvalidFrame)
}

struct BitReader<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> BitReader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    fn bit(&mut self) -> Result<bool, HgError> {
        let byte = self.position / 8;
        let shift = self.position % 8;
        let value = *self.bytes.get(byte).ok_or(HgError::Truncated)?;
        self.position = self
            .position
            .checked_add(1)
            .ok_or(HgError::InvalidPixelCommands)?;
        Ok(value & (1 << shift) != 0)
    }

    fn gamma(&mut self) -> Result<usize, HgError> {
        let mut zeros = 0u32;
        while !self.bit()? {
            zeros = zeros.checked_add(1).ok_or(HgError::InvalidPixelCommands)?;
            if zeros >= usize::BITS {
                return Err(HgError::InvalidPixelCommands);
            }
        }
        let mut value = 1usize;
        for _ in 0..zeros {
            value = value.checked_shl(1).ok_or(HgError::InvalidPixelCommands)?;
            if self.bit()? {
                value |= 1;
            }
        }
        Ok(value)
    }
}

fn parse_hg3(data: &[u8]) -> Result<HgFile, HgError> {
    if data.len() < 12 {
        return Err(HgError::Truncated);
    }
    let header_size = u32_at(data, 4).ok_or(HgError::Truncated)?;
    let version = u32_at(data, 8).ok_or(HgError::Truncated)?;
    if header_size != 12 || version != 0x300 {
        return Err(HgError::InvalidHeader);
    }

    let mut frames = Vec::new();
    let mut frame_offset = 12usize;
    let mut tag_count = 0usize;
    loop {
        if frames.len() == MAX_ITEMS {
            return Err(HgError::TooLarge);
        }
        let frame_header = range(data, frame_offset, 8).ok_or(HgError::Truncated)?;
        let offset_next = u32_at(frame_header, 0).ok_or(HgError::Truncated)?;
        let frame_end = if offset_next == 0 {
            data.len()
        } else {
            let next = usize::try_from(offset_next).map_err(|_| HgError::InvalidFrame)?;
            if next < 8 {
                return Err(HgError::InvalidFrame);
            }
            frame_offset
                .checked_add(next)
                .ok_or(HgError::InvalidFrame)?
        };
        if frame_end > data.len() {
            return Err(HgError::Truncated);
        }

        let id = u32_at(frame_header, 4).ok_or(HgError::Truncated)?;
        let mut frame = HgFrame {
            id,
            width: 0,
            height: 0,
            pixels_rgba: Vec::new(),
            bit_depth: 0,
            offset: [0; 2],
            canvas_size: [0; 2],
            is_transparent: 0,
            base: [0; 2],
            frame_offset: frame_offset as u64,
            tags: Vec::new(),
            image: None,
            encoded_image: None,
            attributes: Vec::new(),
        };
        let mut has_stdinfo = false;
        let mut encoded_data: Option<(HgImageEncoding, u64, u32)> = None;
        let mut alpha_data: Option<(u64, u32, u32)> = None;

        let mut tag_offset = frame_offset + 8;
        loop {
            if tag_count == MAX_ITEMS {
                return Err(HgError::TooLarge);
            }
            tag_count += 1;
            if tag_offset.checked_add(16).is_none_or(|end| end > frame_end) {
                return Err(HgError::InvalidFrame);
            }
            let tag_header = range(data, tag_offset, 16).ok_or(HgError::Truncated)?;
            let name: [u8; 8] = tag_header[..8].try_into().unwrap();
            let next = u32_at(tag_header, 8).ok_or(HgError::Truncated)?;
            let length = u32_at(tag_header, 12).ok_or(HgError::Truncated)?;
            let length_usize = usize::try_from(length).map_err(|_| HgError::InvalidFrame)?;
            let payload = tag_offset
                .checked_add(16)
                .and_then(|offset| range(data, offset, length_usize))
                .ok_or(HgError::Truncated)?;
            if tag_offset + 16 + length_usize > frame_end {
                return Err(HgError::InvalidFrame);
            }
            frame.tags.push(HgTag {
                name,
                offset_next: next,
                length,
                data_offset: (tag_offset + 16) as u64,
            });
            let tag_end = tag_offset
                .checked_add(16)
                .and_then(|end| end.checked_add(length_usize))
                .ok_or(HgError::InvalidFrame)?;

            let tag_name = &name[..name.iter().position(|&byte| byte == 0).unwrap_or(8)];
            if tag_name == b"stdinfo" && !has_stdinfo {
                if payload.len() < 40 {
                    return Err(HgError::InvalidFrame);
                }
                has_stdinfo = true;
                frame.width = u32_at(payload, 0).ok_or(HgError::Truncated)?;
                frame.height = u32_at(payload, 4).ok_or(HgError::Truncated)?;
                frame.bit_depth = u32_at(payload, 8).ok_or(HgError::Truncated)?;
                frame.offset = [
                    u32_at(payload, 12).ok_or(HgError::Truncated)? as i32,
                    u32_at(payload, 16).ok_or(HgError::Truncated)? as i32,
                ];
                frame.canvas_size = [
                    u32_at(payload, 20).ok_or(HgError::Truncated)?,
                    u32_at(payload, 24).ok_or(HgError::Truncated)?,
                ];
                frame.is_transparent = u32_at(payload, 28).ok_or(HgError::Truncated)?;
                frame.base = [
                    u32_at(payload, 32).ok_or(HgError::Truncated)? as i32,
                    u32_at(payload, 36).ok_or(HgError::Truncated)? as i32,
                ];
            } else if tag_name == b"img_jpg" {
                if encoded_data
                    .replace((HgImageEncoding::Jpeg, (tag_offset + 16) as u64, length))
                    .is_some()
                {
                    return Err(HgError::InvalidFrame);
                }
            } else if tag_name == b"img_wbp" {
                if encoded_data
                    .replace((HgImageEncoding::Webp, (tag_offset + 16) as u64, length))
                    .is_some()
                {
                    return Err(HgError::InvalidFrame);
                }
            } else if tag_name == b"img_jpa" {
                if encoded_data
                    .replace((HgImageEncoding::Jpa, (tag_offset + 16) as u64, length))
                    .is_some()
                {
                    return Err(HgError::InvalidFrame);
                }
            } else if tag_name == b"img_al" {
                if payload.len() < 8 {
                    return Err(HgError::InvalidFrame);
                }
                let compressed_length = u32_at(payload, 0).ok_or(HgError::Truncated)?;
                let decompressed_length = u32_at(payload, 4).ok_or(HgError::Truncated)?;
                let compressed_length_usize =
                    usize::try_from(compressed_length).map_err(|_| HgError::InvalidFrame)?;
                if 8usize
                    .checked_add(compressed_length_usize)
                    .is_none_or(|end| end > payload.len())
                {
                    return Err(HgError::InvalidFrame);
                }
                if alpha_data
                    .replace((
                        (tag_offset + 16 + 8) as u64,
                        compressed_length,
                        decompressed_length,
                    ))
                    .is_some()
                {
                    return Err(HgError::InvalidFrame);
                }
            } else if is_numbered_tag(tag_name, b"img") {
                if payload.len() < 24 {
                    return Err(HgError::InvalidFrame);
                }
                let compressed_data_length = u32_at(payload, 8).ok_or(HgError::Truncated)?;
                let decompressed_data_length = u32_at(payload, 12).ok_or(HgError::Truncated)?;
                let compressed_command_length = u32_at(payload, 16).ok_or(HgError::Truncated)?;
                let decompressed_command_length = u32_at(payload, 20).ok_or(HgError::Truncated)?;
                let compressed_data_length_usize =
                    usize::try_from(compressed_data_length).map_err(|_| HgError::InvalidFrame)?;
                let compressed_command_length_usize = usize::try_from(compressed_command_length)
                    .map_err(|_| HgError::InvalidFrame)?;
                if 24usize
                    .checked_add(compressed_data_length_usize)
                    .and_then(|size| size.checked_add(compressed_command_length_usize))
                    .is_none_or(|size| size > payload.len())
                {
                    return Err(HgError::InvalidFrame);
                }
                let image = HgImageData {
                    data_offset: (tag_offset + 16 + 24) as u64,
                    compressed_data_length,
                    decompressed_data_length,
                    compressed_command_length,
                    decompressed_command_length,
                };
                if frame.image.is_none() {
                    frame.image = Some(image);
                }
            } else if let Some(id) = attribute_tag_id(tag_header) {
                if payload.len() < 20 {
                    return Err(HgError::InvalidFrame);
                }
                let values = [
                    u32_at(payload, 0).ok_or(HgError::Truncated)?,
                    u32_at(payload, 4).ok_or(HgError::Truncated)?,
                    u32_at(payload, 8).ok_or(HgError::Truncated)?,
                    u32_at(payload, 12).ok_or(HgError::Truncated)?,
                    u32_at(payload, 16).ok_or(HgError::Truncated)?,
                ];
                // IDs are low sixteen bits, and later declarations replace
                // the first value even when a different spelling aliases it.
                if let Some(attribute) = frame.attributes.iter_mut()
                    .find(|attribute| attribute.id == u32::from(id))
                {
                    attribute.values = values;
                } else {
                    frame.attributes.push(HgAttribute { id: u32::from(id), values });
                }
            }

            if next == 0 {
                let padding = data.get(tag_end..frame_end).ok_or(HgError::InvalidFrame)?;
                let padded_control_tag = padding.len() == 9
                    && offset_next != 0
                    && tag_name == b"cptype"
                    && frame_end.is_multiple_of(8);
                if (padding.len() > 7 && !padded_control_tag) || padding.iter().any(|&byte| byte != 0)
                {
                    return Err(HgError::InvalidFrame);
                }
                break;
            }
            let next_usize = usize::try_from(next).map_err(|_| HgError::InvalidFrame)?;
            if next_usize < 16 + length_usize
                || tag_offset
                    .checked_add(next_usize)
                    .is_none_or(|end| end > frame_end)
            {
                return Err(HgError::InvalidFrame);
            }
            tag_offset += next_usize;
        }
        if encoded_data.is_some() || alpha_data.is_some() {
            let (encoding, data_offset, data_length) =
                encoded_data.unwrap_or((HgImageEncoding::Alpha, 0, 0));
            let encoding = match (encoding, alpha_data.is_some()) {
                (HgImageEncoding::Jpeg, true) => HgImageEncoding::JpegAlpha,
                (HgImageEncoding::Webp, true) => HgImageEncoding::WebpAlpha,
                (encoding, _) => encoding,
            };
            frame.encoded_image = Some(HgEncodedImage {
                encoding,
                data_offset: (data_length != 0).then_some(data_offset),
                data_length,
                alpha_data_offset: alpha_data.map(|value| value.0),
                alpha_compressed_length: alpha_data.map(|value| value.1),
                alpha_decompressed_length: alpha_data.map(|value| value.2),
            });
        }

        frames.push(frame);
        if offset_next == 0 {
            break;
        }
        frame_offset = frame_end;
    }

    Ok(HgFile {
        signature: *b"HG-3",
        header_size,
        version,
        frames,
    })
}

fn parse_hg2(data: &[u8]) -> Result<HgFile, HgError> {
    if data.len() < 12 {
        return Err(HgError::Truncated);
    }
    let header_size = u32_at(data, 4).ok_or(HgError::Truncated)?;
    let version = u32_at(data, 8).ok_or(HgError::Truncated)?;
    let layout_version = version as u16;
    if header_size != 12 || !matches!(layout_version, 0x10 | 0x20 | 0x25) {
        return Err(HgError::InvalidHeader);
    }

    let mut frames = Vec::new();
    let mut frame_offset = 12usize;
    loop {
        if frames.len() == MAX_ITEMS {
            return Err(HgError::TooLarge);
        }
        let base_length = match layout_version { 0x10 => 36, 0x20 => 68, _ => 76 };
        let header = range(data, frame_offset, base_length).ok_or(HgError::Truncated)?;
        let offset_next = if layout_version == 0x10 { 0 } else {
            u32_at(header, 64).ok_or(HgError::Truncated)?
        };
        let frame_end = if offset_next == 0 {
            data.len()
        } else {
            let next = usize::try_from(offset_next).map_err(|_| HgError::InvalidFrame)?;
            if next < base_length {
                return Err(HgError::InvalidFrame);
            }
            frame_offset
                .checked_add(next)
                .ok_or(HgError::InvalidFrame)?
        };
        if frame_end > data.len() {
            return Err(HgError::Truncated);
        }

        let compressed_data_length = u32_at(header, 20).ok_or(HgError::Truncated)?;
        let compressed_command_length = u32_at(header, 28).ok_or(HgError::Truncated)?;
        let payload_length = usize::try_from(compressed_data_length)
            .ok()
            .and_then(|data_len| {
                usize::try_from(compressed_command_length)
                    .ok()
                    .and_then(|command_len| data_len.checked_add(command_len))
            })
            .ok_or(HgError::InvalidFrame)?;
        // In 0x20/0x25 the DWORD at +0x24 is relative to that DWORD,
        // rather than to the frame or file. The native reader follows it
        // independently of the next-frame link and nominal header length.
        let payload_offset = if layout_version == 0x10 { 36 } else {
            36usize.checked_add(u32_at(header, 36).ok_or(HgError::Truncated)? as usize)
                .ok_or(HgError::InvalidFrame)?
        };
        if payload_offset < base_length || payload_offset
            .checked_add(payload_length)
            .is_none_or(|length| length > frame_end - frame_offset)
        {
            return Err(HgError::InvalidFrame);
        }
        let width = u32_at(header, 0).ok_or(HgError::Truncated)?;
        let height = u32_at(header, 4).ok_or(HgError::Truncated)?;

        frames.push(HgFrame {
            id: if layout_version == 0x10 { 0 } else {
                u32_at(header, 40).ok_or(HgError::Truncated)?
            },
            width,
            height,
            pixels_rgba: Vec::new(),
            bit_depth: u32_at(header, 8).ok_or(HgError::Truncated)?,
            offset: if layout_version == 0x10 { [0; 2] } else { [
                u32_at(header, 52).ok_or(HgError::Truncated)? as i32,
                u32_at(header, 56).ok_or(HgError::Truncated)? as i32,
            ] },
            canvas_size: if layout_version == 0x10 { [width, height] } else { [
                u32_at(header, 44).ok_or(HgError::Truncated)?,
                u32_at(header, 48).ok_or(HgError::Truncated)?,
            ] },
            is_transparent: if layout_version == 0x10 { 0 } else {
                u32_at(header, 60).ok_or(HgError::Truncated)?
            },
            base: if layout_version == 0x25 {
                [
                    u32_at(header, 68).ok_or(HgError::Truncated)? as i32,
                    u32_at(header, 72).ok_or(HgError::Truncated)? as i32,
                ]
            } else {
                [0; 2]
            },
            frame_offset: frame_offset as u64,
            tags: Vec::new(),
            image: Some(HgImageData {
                data_offset: (frame_offset + payload_offset) as u64,
                compressed_data_length,
                decompressed_data_length: u32_at(header, 24).ok_or(HgError::Truncated)?,
                compressed_command_length,
                decompressed_command_length: u32_at(header, 32).ok_or(HgError::Truncated)?,
            }),
            encoded_image: None,
            attributes: Vec::new(),
        });

        if offset_next == 0 {
            break;
        }
        frame_offset = frame_end;
    }

    Ok(HgFile {
        signature: *b"HG-2",
        header_size,
        version,
        frames,
    })
}

/// Parse a lowercase HG-3 `ats` tag using the engine's integer-prefix rules.
///
/// Supply the full sixteen-byte tag header, not just the eight name bytes:
/// an unterminated name can continue into the following little-endian link.
/// Parsing stays within the supplied header and stops at its first zero.
pub fn attribute_tag_id(header: &[u8]) -> Option<u16> {
    let text = header.split(|&byte| byte == 0).next()?;
    let text = text.strip_prefix(b"ats")?;
    let mut cursor = 0;
    let negative = match text.first() {
        Some(b'-') => { cursor = 1; true }
        Some(b'+') => { cursor = 1; false }
        _ => false,
    };
    let hexadecimal = text.get(cursor..)
        .is_some_and(|text| text.starts_with(b"0x") || text.starts_with(b"0X"));
    if hexadecimal {
        cursor += 2;
    }
    let first_digit = cursor;
    let radix = if hexadecimal { 16 } else { 10 };
    let mut value = 0u32;
    while let Some(&byte) = text.get(cursor) {
        let Some(digit) = (byte as char).to_digit(radix) else { break };
        value = value.wrapping_mul(radix).wrapping_add(digit);
        cursor += 1;
    }
    if cursor == first_digit || (!hexadecimal && text.get(cursor) == Some(&b'.')) {
        return None;
    }
    Some(if negative { value.wrapping_neg() } else { value } as u16)
}

fn is_numbered_tag(name: &[u8], prefix: &[u8]) -> bool {
    name.strip_prefix(prefix)
        .is_some_and(|suffix| !suffix.is_empty() && suffix.iter().all(u8::is_ascii_digit))
}

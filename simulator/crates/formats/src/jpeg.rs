//! Standalone JPEG headers and RGB pixels used by the native image dispatcher.
//! Decoder machinery comes from the workspace's existing `image` dependency;
//! native geometry, frame selection and storage contracts are independent.
use crate::util::MAX_BUFFER;
use image::{ImageFormat, ImageReader, Limits};
use std::io::Cursor;
use thiserror::Error;

const MAX_PIXELS: usize = 128 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JpegMetadata {
    pub width: u32,
    pub height: u32,
    pub precision: u8,
    pub components: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JpegImage {
    pub metadata: JpegMetadata,
    /// Top-down RGB converted to tightly packed RGBA8, with opaque Alpha.
    pub pixels: Vec<u8>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum JpegError {
    #[error("invalid JPEG signature")]
    InvalidSignature,
    #[error("JPEG header is truncated")]
    Truncated,
    #[error("JPEG input exceeds 64 MiB")]
    TooLarge,
    #[error("JPEG header is invalid or unsupported")]
    InvalidHeader,
    #[error("JPEG pixel stream cannot be decoded")]
    InvalidPixels,
    #[error("JPEG native RGB decode requires eight-bit gray or three-component input")]
    UnsupportedColorEncoding,
    #[error("JPEG RGBA pixels exceed 128 MiB")]
    PixelOutputTooLarge,
}

impl JpegMetadata {
    pub fn parse(data: &[u8]) -> Result<Self, JpegError> {
        if data.len() > MAX_BUFFER { return Err(JpegError::TooLarge); }
        if data.get(..2) != Some(b"\xff\xd8") { return Err(JpegError::InvalidSignature); }
        if data.len() < 11 { return Err(JpegError::Truncated); }
        // Reading dimensions validates JPEG headers without decoding the
        // entropy-coded pixels or reserving a pixel-sized output allocation.
        let (width, height) = ImageReader::with_format(Cursor::new(data), ImageFormat::Jpeg)
            .into_dimensions().map_err(|_| JpegError::InvalidHeader)?;
        let mut offset = 2usize;
        while offset < data.len() {
            if data[offset] != 0xff { return Err(JpegError::InvalidHeader); }
            while data.get(offset) == Some(&0xff) { offset += 1; }
            let marker = *data.get(offset).ok_or(JpegError::Truncated)?;
            offset += 1;
            if matches!(marker, 0x00 | 0xd8 | 0xd9 | 0xda) {
                return Err(JpegError::InvalidHeader);
            }
            if matches!(marker, 0x01 | 0xd0..=0xd7) { continue; }
            let size = data.get(offset..offset + 2).ok_or(JpegError::Truncated)?;
            let size = usize::from(u16::from_be_bytes([size[0], size[1]]));
            if size < 2 { return Err(JpegError::InvalidHeader); }
            let end = offset.checked_add(size).ok_or(JpegError::TooLarge)?;
            let payload = data.get(offset + 2..end).ok_or(JpegError::Truncated)?;
            if matches!(marker, 0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf) {
                let fields = payload.get(..6).ok_or(JpegError::InvalidHeader)?;
                let components = fields[5];
                let declared_height = u32::from(u16::from_be_bytes([fields[1], fields[2]]));
                let declared_width = u32::from(u16::from_be_bytes([fields[3], fields[4]]));
                if components == 0 || payload.len() != 6 + usize::from(components) * 3
                    || width != declared_width || height != declared_height {
                    return Err(JpegError::InvalidHeader);
                }
                return Ok(Self { width, height, precision: fields[0], components });
            }
            offset = end;
        }
        Err(JpegError::InvalidHeader)
    }
}

impl JpegImage {
    pub fn decode(data: &[u8]) -> Result<Self, JpegError> {
        let metadata = JpegMetadata::parse(data)?;
        if metadata.precision != 8 || !matches!(metadata.components, 1 | 3) {
            return Err(JpegError::UnsupportedColorEncoding);
        }
        let length = (metadata.width as usize).checked_mul(metadata.height as usize)
            .and_then(|pixels| pixels.checked_mul(4)).filter(|&length| length <= MAX_PIXELS)
            .ok_or(JpegError::PixelOutputTooLarge)?;
        let mut reader = ImageReader::with_format(Cursor::new(data), ImageFormat::Jpeg);
        let mut limits = Limits::default();
        limits.max_image_width = Some(metadata.width);
        limits.max_image_height = Some(metadata.height);
        limits.max_alloc = Some(MAX_PIXELS as u64);
        reader.limits(limits);
        let image = reader.decode().map_err(|_| JpegError::InvalidPixels)?.to_rgba8();
        if image.width() != metadata.width || image.height() != metadata.height
            || image.len() != length {
            return Err(JpegError::InvalidPixels);
        }
        Ok(Self { metadata, pixels: image.into_raw() })
    }
}

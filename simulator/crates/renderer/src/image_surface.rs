//! Native pixel allocations may be bound even when either extent is zero.
//! This constructor is separate from decoded file/canvas dimension checks.
use super::{Arc, FrameMetadata, FramePixels, HgFrameAttributes, RendererError, Result, RgbaFrame};

const NATIVE_SURFACE_LIMIT: usize = 128 * 1024 * 1024;

fn native_rgba_len(width: u32, height: u32, limit: usize) -> Result<usize> {
    if width > i32::MAX as u32 || height > i32::MAX as u32 {
        return Err(RendererError::InvalidDimensions { width, height });
    }
    (width as usize)
        .checked_mul(height as usize)
        .and_then(|length| length.checked_mul(4))
        .filter(|&length| length <= limit)
        .ok_or(RendererError::InvalidDimensions { width, height })
}

impl RgbaFrame {
    /// Allocates a transparent native surface within the caller's byte budget.
    pub fn blank_native_rgba8(width: u32, height: u32, limit: usize) -> Result<Self> {
        let bytes = native_rgba_len(width, height, limit)?;
        let mut pixels = Vec::new();
        pixels
            .try_reserve_exact(bytes)
            .map_err(|_| RendererError::AllocationFailed { bytes })?;
        pixels.resize(bytes, 0);
        Ok(Self {
            width,
            height,
            pixels: Arc::new(FramePixels::new(pixels)),
            metadata: FrameMetadata::raster([width, height]),
            attributes: HgFrameAttributes::default(),
        })
    }

    /// Builds a bound native surface, retaining raw geometry (including zero
    /// canvas axes). An empty allocation is not an unbound image wrapper.
    pub fn from_native_rgba8(
        width: u32,
        height: u32,
        pixels: Vec<u8>,
        metadata: FrameMetadata,
    ) -> Result<Self> {
        let expected = native_rgba_len(width, height, NATIVE_SURFACE_LIMIT)?;
        if pixels.len() != expected {
            return Err(RendererError::BufferLength { expected, actual: pixels.len() });
        }
        Ok(Self { width, height, pixels: Arc::new(FramePixels::new(pixels)), metadata, attributes: HgFrameAttributes::default() })
    }

    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }
}

//! Native pixel allocations may be bound even when either extent is zero.
//! This constructor is separate from decoded file/canvas dimension checks.
use super::{FrameMetadata, HgFrameAttributes, RendererError, Result, RgbaFrame};

impl RgbaFrame {
    /// Builds a bound native surface, retaining raw geometry (including zero
    /// canvas axes). An empty allocation is not an unbound image wrapper.
    pub fn from_native_rgba8(
        width: u32,
        height: u32,
        pixels: Vec<u8>,
        metadata: FrameMetadata,
    ) -> Result<Self> {
        if width > i32::MAX as u32 || height > i32::MAX as u32 {
            return Err(RendererError::InvalidDimensions { width, height });
        }
        let expected = (width as usize)
            .checked_mul(height as usize)
            .and_then(|length| length.checked_mul(4))
            .filter(|&length| length <= 128 * 1024 * 1024)
            .ok_or(RendererError::InvalidDimensions { width, height })?;
        if pixels.len() != expected {
            return Err(RendererError::BufferLength { expected, actual: pixels.len() });
        }
        Ok(Self { width, height, pixels: std::sync::Arc::new(pixels), metadata, attributes: HgFrameAttributes::default() })
    }

    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }
}

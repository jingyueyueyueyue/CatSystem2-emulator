//! Portable CPU-side rendering primitives for CatSystem2-compatible hosts.
//!
//! The crate deliberately owns no window, audio, or GPU state. A canvas is a
//! fixed-size RGBA8 buffer, and all drawing operations are deterministic.

use std::{
    collections::BTreeMap,
    fmt,
    io::Write,
    ops::Deref,
    sync::{Arc, Mutex, OnceLock, Weak},
};

use cs2_formats::HgFile;
use flate2::{Compression, write::ZlibEncoder};

mod text_layout;
mod capture;
mod image_surface;
mod raster;
mod text_cache;
pub use capture::encode_capture_bmp;

const FNV_OFFSET_BASIS: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x100000001b3;
const MAX_GLYPH_MASK_BYTES: usize = 16 * 1024 * 1024;
const MAX_HG_FRAMES: usize = 4096;
const MAX_HG_TAGS: usize = 65536;

pub type Result<T> = std::result::Result<T, RendererError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RendererError {
    InvalidDimensions {
        width: u32,
        height: u32,
    },
    BufferLength {
        expected: usize,
        actual: usize,
    },
    CoordinateOutOfBounds {
        x: i64,
        y: i64,
        width: u32,
        height: u32,
    },
    RegionOutOfBounds {
        rect: Rect,
        width: u32,
        height: u32,
    },
    InvalidScale {
        scale_x_bits: u32,
        scale_y_bits: u32,
    },
    NonFiniteScale,
    NonFiniteTransform,
    ScaledDimensionsOverflow,
    MaskDimensions {
        mask_width: u32,
        mask_height: u32,
        canvas_width: u32,
        canvas_height: u32,
    },
    ViewportSourceDimensions {
        source_width: u32,
        source_height: u32,
        logical_width: u32,
        logical_height: u32,
    },
    InvalidRegion {
        rect: Rect,
        width: u32,
        height: u32,
    },
    InvalidEffectParameter {
        name: &'static str,
    },
    FrameDimensionsMismatch {
        first_width: u32,
        first_height: u32,
        second_width: u32,
        second_height: u32,
    },
    Decode {
        message: String,
    },
    AllocationFailed {
        bytes: usize,
    },
    InvalidResourceHandle {
        handle: u32,
    },
    ResourceReferenceUnderflow {
        handle: u32,
    },
    ResourceHandleExhausted,
    DuplicateResourceHandle {
        handle: u32,
    },
    ResourceReferenceOverflow {
        handle: u32,
    },
    ResourceLeasePoolMismatch {
        handle: u32,
    },
    InvalidLayerIndex {
        index: usize,
    },
    FrameIndexOutOfBounds {
        index: usize,
        count: usize,
    },
    FrameIdNotFound {
        id: u32,
    },
    AmbiguousFrameId {
        id: u32,
    },
    ConflictingHgAttribute {
        id: u16,
    },
    InvalidHgStructure {
        message: String,
    },
    UnsupportedHgEncoding {
        tag: [u8; 8],
    },
    UnsupportedHgSlice {
        tag: [u8; 8],
    },
    UnsupportedHgAttributes {
        container: HgContainer,
    },
    UnsupportedTextMarker {
        marker: char,
    },
}

impl fmt::Display for RendererError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDimensions { width, height } => {
                write!(f, "dimensions must be non-zero: {width}x{height}")
            }
            Self::BufferLength { expected, actual } => {
                write!(f, "RGBA buffer length is {actual}, expected {expected}")
            }
            Self::CoordinateOutOfBounds {
                x,
                y,
                width,
                height,
            } => {
                write!(f, "pixel coordinate ({x}, {y}) is outside {width}x{height}")
            }
            Self::RegionOutOfBounds {
                rect,
                width,
                height,
            } => {
                write!(
                    f,
                    "region {rect:?} is outside {width}x{height} and has no drawable pixels"
                )
            }
            Self::InvalidScale {
                scale_x_bits,
                scale_y_bits,
            } => {
                write!(
                    f,
                    "scale must be positive, got {}x{}",
                    f32::from_bits(*scale_x_bits),
                    f32::from_bits(*scale_y_bits)
                )
            }
            Self::NonFiniteScale => f.write_str("scale must be finite and positive"),
            Self::NonFiniteTransform => f.write_str("rotation and pivot values must be finite"),
            Self::ScaledDimensionsOverflow => f.write_str("scaled image dimensions overflow"),
            Self::MaskDimensions {
                mask_width,
                mask_height,
                canvas_width,
                canvas_height,
            } => write!(
                f,
                "mask dimensions {mask_width}x{mask_height} do not match canvas {canvas_width}x{canvas_height}"
            ),
            Self::ViewportSourceDimensions {
                source_width,
                source_height,
                logical_width,
                logical_height,
            } => write!(
                f,
                "viewport expects a {logical_width}x{logical_height} frame, got {source_width}x{source_height}"
            ),
            Self::InvalidRegion {
                rect,
                width,
                height,
            } => write!(
                f,
                "region {rect:?} is outside frame dimensions {width}x{height}"
            ),
            Self::InvalidEffectParameter { name } => {
                write!(f, "invalid renderer effect parameter: {name}")
            }
            Self::FrameDimensionsMismatch {
                first_width,
                first_height,
                second_width,
                second_height,
            } => write!(
                f,
                "frame dimensions {first_width}x{first_height} do not match {second_width}x{second_height}"
            ),
            Self::Decode { message } => write!(f, "image decode failed: {message}"),
            Self::AllocationFailed { bytes } => {
                write!(f, "could not allocate {bytes} bytes for RGBA data")
            }
            Self::InvalidResourceHandle { handle } => {
                write!(f, "invalid renderer resource handle {handle}")
            }
            Self::ResourceReferenceUnderflow { handle } => {
                write!(f, "renderer resource {handle} has no reference to release")
            }
            Self::ResourceHandleExhausted => {
                f.write_str("renderer resource handle space is exhausted")
            }
            Self::DuplicateResourceHandle { handle } => {
                write!(
                    f,
                    "renderer resource handle {handle} appears more than once"
                )
            }
            Self::ResourceReferenceOverflow { handle } => {
                write!(f, "renderer resource {handle} reference count overflowed")
            }
            Self::ResourceLeasePoolMismatch { handle } => {
                write!(
                    f,
                    "renderer resource lease {handle} belongs to another pool"
                )
            }
            Self::InvalidLayerIndex { index } => {
                write!(f, "invalid renderer layer index {index}")
            }
            Self::FrameIndexOutOfBounds { index, count } => {
                write!(f, "animation frame index {index} is outside 0..{count}")
            }
            Self::FrameIdNotFound { id } => {
                write!(f, "animation frame ID {id} was not found")
            }
            Self::AmbiguousFrameId { id } => {
                write!(f, "animation frame ID {id} is ambiguous")
            }
            Self::ConflictingHgAttribute { id } => {
                write!(f, "HG attribute {id} has conflicting values across layers")
            }
            Self::InvalidHgStructure { message } => {
                write!(f, "invalid HG structure: {message}")
            }
            Self::UnsupportedHgEncoding { tag } => {
                write!(f, "unsupported HG image encoding tag {:?}", tag)
            }
            Self::UnsupportedHgSlice { tag } => {
                write!(f, "unsupported HG image slice tag {:?}", tag)
            }
            Self::UnsupportedHgAttributes { container } => {
                write!(f, "HG attributes cannot be represented in {container:?}")
            }
            Self::UnsupportedTextMarker { marker } => {
                write!(f, "unsupported text marker \\{marker}")
            }
        }
    }
}

impl std::error::Error for RendererError {}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Default)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

impl Point {
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }
}

/// Maps a physical window position to the logical canvas using the same
/// independent-axis stretch used by the desktop presenters.
pub fn map_viewport_point(
    position: [f64; 2],
    window_width: u32,
    window_height: u32,
    logical_width: u32,
    logical_height: u32,
) -> Option<Point> {
    let [x, y] = position;
    if window_width == 0
        || window_height == 0
        || logical_width == 0
        || logical_height == 0
        || !x.is_finite()
        || !y.is_finite()
        || x < 0.0
        || y < 0.0
        || x >= f64::from(window_width)
        || y >= f64::from(window_height)
    {
        return None;
    }

    Some(Point::new(
        (x * f64::from(logical_width) / f64::from(window_width)).floor() as i32,
        (y * f64::from(logical_height) / f64::from(window_height)).floor() as i32,
    ))
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Default)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    fn right(self) -> i64 {
        i64::from(self.x) + i64::from(self.width)
    }

    fn bottom(self) -> i64 {
        i64::from(self.y) + i64::from(self.height)
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Default)]
pub enum ViewportMode {
    #[default]
    Stretch,
    Fit,
    Fill,
    IntegerFit,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Default)]
pub enum ScaleFilter {
    #[default]
    Nearest,
    Linear,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Default)]
pub enum MosaicAlgorithm {
    #[default]
    SampleCenter,
    OpaqueBlocks,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Default)]
pub enum FrameFilter {
    #[default]
    None,
    Mono,
    Sepia,
    Negative,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Default)]
pub enum HgContainer {
    #[default]
    None,
    Hg2V10,
    Hg2V20,
    Hg2V25,
    Hg3V300,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HgTagKind {
    StandardInfo,
    NativeImage,
    Jpeg,
    Wbp,
    Jpa,
    Alpha,
    ImageSlice,
    Attribute,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HgTagInfo {
    pub name: [u8; 8],
    pub kind: HgTagKind,
    pub offset: usize,
    pub length: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HgFrameInfo {
    pub id: u32,
    pub offset: usize,
    pub width: u32,
    pub height: u32,
    pub bit_depth: u32,
    pub image_offset: [i32; 2],
    pub canvas_size: [u32; 2],
    pub alpha_flag: u32,
    pub base: [i32; 2],
    pub tags: Vec<HgTagInfo>,
    pub attributes: BTreeMap<u16, [u32; 5]>,
}

/// Frame-local `atsNNNN` values decoded from an HG container.
///
/// The renderer keeps these values beside the decoded raster rather than
/// folding them into `FrameMetadata`: attributes belong to one HG frame and
/// therefore do not have a meaningful value for a window or multi-frame
/// composition. The decoder remains responsible for interpreting the five
/// DWORDs; this type only preserves their identity and values.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HgFrameAttributes {
    values: BTreeMap<u16, [u32; 5]>,
}

impl HgFrameAttributes {
    pub fn new(values: BTreeMap<u16, [u32; 5]>) -> Self {
        Self { values }
    }

    pub fn from_frame_info(frame: &HgFrameInfo) -> Self {
        Self::new(frame.attributes.clone())
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn get(&self, id: u16) -> Option<[u32; 5]> {
        self.values.get(&id).copied()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&u16, &[u32; 5])> {
        self.values.iter()
    }

    pub fn as_map(&self) -> &BTreeMap<u16, [u32; 5]> {
        &self.values
    }

    pub fn into_map(self) -> BTreeMap<u16, [u32; 5]> {
        self.values
    }
}

impl HgFrameInfo {
    pub fn attributes_view(&self) -> HgFrameAttributes {
        HgFrameAttributes::from_frame_info(self)
    }

    pub fn attribute(&self, id: u16) -> Option<[u32; 5]> {
        self.attributes.get(&id).copied()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HgInspection {
    pub container: HgContainer,
    pub header_size: u32,
    pub version: u32,
    pub frames: Vec<HgFrameInfo>,
}

impl HgInspection {
    pub fn has_native_pixels(&self) -> bool {
        self.frames.iter().any(|frame| {
            frame
                .tags
                .iter()
                .any(|tag| matches!(tag.kind, HgTagKind::NativeImage | HgTagKind::ImageSlice))
        })
    }

    pub fn unsupported_tags(&self) -> impl Iterator<Item = &HgTagInfo> {
        self.frames.iter().flat_map(|frame| {
            frame
                .tags
                .iter()
                .filter(|tag| tag.kind == HgTagKind::Jpa)
        })
    }

    pub fn require_native_pixels(&self) -> Result<()> {
        if let Some(frame) = self
            .frames
            .iter()
            .find(|frame| !matches!(frame.bit_depth as u16 as i16, 24 | 32))
        {
            return Err(RendererError::InvalidHgStructure {
                message: format!(
                    "unsupported HG bit depth {} in frame {}",
                    frame.bit_depth, frame.id
                ),
            });
        }
        if let Some(tag) = self
            .frames
            .iter()
            .flat_map(|frame| frame.tags.iter())
            .find(|tag| tag.kind == HgTagKind::Jpa)
        {
            return Err(RendererError::UnsupportedHgEncoding { tag: tag.name });
        }
        if let Some(frame) = self.frames.iter().find(|frame| {
            !frame
                .tags
                .iter()
                .any(|tag| matches!(tag.kind, HgTagKind::NativeImage | HgTagKind::ImageSlice))
        }) {
            return Err(RendererError::InvalidHgStructure {
                message: format!("HG frame {} has no native image payload", frame.id),
            });
        }
        Ok(())
    }

    pub fn require_decodable_pixels(&self) -> Result<()> {
        if let Some(frame) = self
            .frames
            .iter()
            .find(|frame| !matches!(frame.bit_depth as u16 as i16, 24 | 32))
        {
            return Err(RendererError::InvalidHgStructure {
                message: format!(
                    "unsupported HG bit depth {} in frame {}",
                    frame.bit_depth, frame.id
                ),
            });
        }
        if let Some(tag) = self.unsupported_tags().next() {
            return Err(RendererError::UnsupportedHgEncoding { tag: tag.name });
        }
        if let Some(frame) = self.frames.iter().find(|frame| {
            !frame.tags.iter().any(|tag| {
                matches!(
                    tag.kind,
                    HgTagKind::NativeImage | HgTagKind::ImageSlice | HgTagKind::Jpeg
                        | HgTagKind::Wbp | HgTagKind::Alpha
                )
            })
        }) {
            return Err(RendererError::InvalidHgStructure {
                message: format!("HG frame {} has no decodable image payload", frame.id),
            });
        }
        Ok(())
    }

    pub fn frame(&self, id: u32) -> Option<&HgFrameInfo> {
        self.frames.iter().find(|frame| frame.id == id)
    }

    pub fn attribute(&self, frame_id: u32, id: u16) -> Option<[u32; 5]> {
        self.frame(frame_id).and_then(|frame| frame.attribute(id))
    }

    pub fn attributes(&self, frame_id: u32) -> Option<HgFrameAttributes> {
        self.frame(frame_id).map(HgFrameAttributes::from_frame_info)
    }

    /// Builds a frame set from pixels decoded by the formats layer.
    ///
    /// The renderer does not decode HG streams here. The caller supplies one
    /// native payload per inspected frame, and this method attaches the
    /// matching `stdinfo`, frame ID, crop geometry, alpha flag, and every
    /// `atsNNNN` value before handing the frame to the renderer.
    pub fn from_decoded_frames(&self, decoded: &[HgDecodedFrame]) -> Result<RgbaFrameSet> {
        self.require_decodable_pixels()?;
        if decoded.len() != self.frames.len() {
            return Err(RendererError::InvalidHgStructure {
                message: format!(
                    "HG decoded frame count {} does not match inspection frame count {}",
                    decoded.len(),
                    self.frames.len()
                ),
            });
        }
        let mut frames = Vec::with_capacity(decoded.len());
        for (info, decoded) in self.frames.iter().zip(decoded) {
            let metadata = FrameMetadata::from_hg_frame(self.container, info);
            let frame = match decoded.layout {
                HgPixelLayout::Rgba8 => {
                    if info.bit_depth as u16 as i16 != 32 {
                        return Err(RendererError::InvalidHgStructure {
                            message: format!(
                                "HG frame {} supplies RGBA8 pixels for {}-bit metadata",
                                info.id, info.bit_depth
                            ),
                        });
                    }
                    RgbaFrame::from_hg_rgba8_with_attributes(
                        info.width,
                        info.height,
                        &decoded.pixels,
                        metadata,
                        info.attributes_view(),
                    )?
                }
                HgPixelLayout::Bgra8 => {
                    if info.bit_depth as u16 as i16 != 32 {
                        return Err(RendererError::InvalidHgStructure {
                            message: format!(
                                "HG frame {} supplies BGRA8 pixels for {}-bit metadata",
                                info.id, info.bit_depth
                            ),
                        });
                    }
                    RgbaFrame::from_hg_bgra8_with_attributes(
                        info.width,
                        info.height,
                        &decoded.pixels,
                        metadata,
                        info.attributes_view(),
                    )?
                }
                HgPixelLayout::Bgr24 => {
                    if info.bit_depth as u16 as i16 != 24 {
                        return Err(RendererError::InvalidHgStructure {
                            message: format!(
                                "HG frame {} supplies BGR24 pixels for {}-bit metadata",
                                info.id, info.bit_depth
                            ),
                        });
                    }
                    RgbaFrame::from_hg_bgr24_with_attributes(
                        info.width,
                        info.height,
                        &decoded.pixels,
                        metadata,
                        info.attributes_view(),
                    )?
                }
            };
            frames.push(frame);
        }
        RgbaFrameSet::new(frames)
    }
}

/// Inspects HG-2/HG-3 metadata without inventing pixels.
///
/// Native image payloads are reported separately from JPEG/WBP/JPA/alpha and
/// numbered-slice tags. Callers may then route native payloads to the formats
/// decoder or return the structured diagnostic to the host.
pub fn inspect_hg(data: &[u8]) -> Result<HgInspection> {
    if data.len() < 12 {
        return Err(RendererError::InvalidHgStructure {
            message: "truncated file header".into(),
        });
    }
    let signature: [u8; 4] = data[..4].try_into().unwrap();
    let header_size = read_u32(data, 4)?;
    let version = read_u32(data, 8)?;
    match &signature {
        b"HG-2" => inspect_hg2(data, header_size, version),
        b"HG-3" => inspect_hg3(data, header_size, version),
        _ => Err(RendererError::Decode {
            message: "unsupported HG signature".into(),
        }),
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Default)]
pub struct FrameMetadata {
    pub container: HgContainer,
    pub frame_id: Option<u32>,
    pub bit_depth: u8,
    pub offset: [i32; 2],
    pub canvas_size: [u32; 2],
    pub base: [i32; 2],
    pub alpha_flag: u32,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum HgPixelLayout {
    Rgba8,
    Bgra8,
    Bgr24,
}

/// Pixel bytes decoded from one native HG image tag, in the matching
/// `HgInspection::frames` order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HgDecodedFrame {
    pub layout: HgPixelLayout,
    pub pixels: Vec<u8>,
}

impl HgDecodedFrame {
    pub fn new(layout: HgPixelLayout, pixels: Vec<u8>) -> Self {
        Self { layout, pixels }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum AnimationFrameSelection {
    Index(usize),
    Id(u32),
}

impl FrameMetadata {
    pub const fn raster(size: [u32; 2]) -> Self {
        Self {
            container: HgContainer::None,
            frame_id: None,
            bit_depth: 32,
            offset: [0; 2],
            canvas_size: size,
            base: [0; 2],
            alpha_flag: 0,
        }
    }

    /// Applies the image-list geometry `position - base + crop offset`.
    pub fn screen_position(&self, position: Point) -> Point {
        Point::new(
            position
                .x
                .wrapping_sub(self.base[0])
                .wrapping_add(self.offset[0]),
            position
                .y
                .wrapping_sub(self.base[1])
                .wrapping_add(self.offset[1]),
        )
    }

    pub const fn composed(size: [u32; 2]) -> Self {
        Self {
            container: HgContainer::None,
            frame_id: None,
            bit_depth: 32,
            offset: [0; 2],
            canvas_size: size,
            base: [0; 2],
            alpha_flag: 0,
        }
    }

    pub fn from_hg_frame(container: HgContainer, frame: &HgFrameInfo) -> Self {
        Self {
            container,
            frame_id: Some(frame.id),
            bit_depth: frame.bit_depth as u16 as u8,
            offset: frame.image_offset,
            canvas_size: frame.canvas_size,
            base: frame.base,
            alpha_flag: frame.alpha_flag,
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct AnimationDisplayState {
    pub frame_id: i32,
    pub opacity: i32,
    pub visible: i32,
    pub offset: Point,
}

impl Default for AnimationDisplayState {
    fn default() -> Self {
        Self {
            frame_id: -1,
            opacity: 255,
            visible: 1,
            offset: Point::default(),
        }
    }
}

impl AnimationDisplayState {
    pub fn compose(self, position: [f32; 2], opacity: i32, visible: i32) -> ComposedDisplayState {
        let base = position.map(|value| {
            if (-2_147_483_648.0..2_147_483_648.0).contains(&value) {
                value as i32
            } else {
                i32::MIN
            }
        });
        let product = (self.opacity as u32).wrapping_mul(opacity as u32);
        ComposedDisplayState {
            frame_id: self.frame_id,
            position: Point::new(
                base[0].wrapping_add(self.offset.x),
                base[1].wrapping_add(self.offset.y),
            ),
            opacity: if product < 0xff00 {
                (product / 255) as u8
            } else {
                255
            },
            visible: visible != 0 && self.visible != 0,
            raw_visible: if visible == 0 { 0 } else { self.visible },
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct ComposedDisplayState {
    pub frame_id: i32,
    pub position: Point,
    pub opacity: u8,
    pub visible: bool,
    pub raw_visible: i32,
}

/// Logical-to-window mapping shared by frame presentation and pointer input.
/// `Fill` keeps the logical aspect ratio and crops equally on opposite sides.
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Viewport {
    window_width: u32,
    window_height: u32,
    logical_width: u32,
    logical_height: u32,
    left: f64,
    top: f64,
    width: f64,
    height: f64,
    scale_x: f64,
    scale_y: f64,
}

impl Viewport {
    pub fn new(
        window_width: u32,
        window_height: u32,
        logical_width: u32,
        logical_height: u32,
        mode: ViewportMode,
    ) -> Result<Self> {
        for (width, height) in [
            (window_width, window_height),
            (logical_width, logical_height),
        ] {
            if width == 0 || height == 0 {
                return Err(RendererError::InvalidDimensions { width, height });
            }
        }

        let window_width_f = f64::from(window_width);
        let window_height_f = f64::from(window_height);
        let logical_width_f = f64::from(logical_width);
        let logical_height_f = f64::from(logical_height);
        let (width, height) = match mode {
            ViewportMode::Stretch => (window_width_f, window_height_f),
            ViewportMode::Fit | ViewportMode::IntegerFit | ViewportMode::Fill => {
                let fit =
                    (window_width_f / logical_width_f).min(window_height_f / logical_height_f);
                let scale = match mode {
                    ViewportMode::Fill => {
                        (window_width_f / logical_width_f).max(window_height_f / logical_height_f)
                    }
                    ViewportMode::IntegerFit if fit >= 1.0 => fit.floor(),
                    _ => fit,
                };
                (logical_width_f * scale, logical_height_f * scale)
            }
        };
        let left = (window_width_f - width) * 0.5;
        let top = (window_height_f - height) * 0.5;
        Ok(Self {
            window_width,
            window_height,
            logical_width,
            logical_height,
            left,
            top,
            width,
            height,
            scale_x: width / logical_width_f,
            scale_y: height / logical_height_f,
        })
    }

    pub fn window_size(&self) -> (u32, u32) {
        (self.window_width, self.window_height)
    }

    pub fn logical_size(&self) -> (u32, u32) {
        (self.logical_width, self.logical_height)
    }

    /// Returns the possibly letterboxed or cropped destination rectangle in
    /// physical window coordinates.
    pub fn destination(&self) -> [f64; 4] {
        [self.left, self.top, self.width, self.height]
    }

    pub fn scale(&self) -> [f64; 2] {
        [self.scale_x, self.scale_y]
    }

    pub fn map_point(&self, position: [f64; 2]) -> Option<Point> {
        let [x, y] = position;
        if !x.is_finite()
            || !y.is_finite()
            || x < 0.0
            || y < 0.0
            || x >= f64::from(self.window_width)
            || y >= f64::from(self.window_height)
            || x < self.left
            || y < self.top
            || x >= self.left + self.width
            || y >= self.top + self.height
        {
            return None;
        }
        let logical_x = ((x - self.left) / self.scale_x).floor();
        let logical_y = ((y - self.top) / self.scale_y).floor();
        let x = i32::try_from(logical_x as i64).ok()?;
        let y = i32::try_from(logical_y as i64).ok()?;
        (x >= 0 && y >= 0 && (x as u32) < self.logical_width && (y as u32) < self.logical_height)
            .then_some(Point::new(x, y))
    }

    /// Scales a logical frame into the physical window, clearing any Fit-mode
    /// bars to `background`.
    pub fn present(
        &self,
        source: &RgbaFrame,
        background: Color,
        filter: ScaleFilter,
    ) -> Result<Canvas> {
        if source.dimensions() != (self.logical_width, self.logical_height) {
            return Err(RendererError::ViewportSourceDimensions {
                source_width: source.width,
                source_height: source.height,
                logical_width: self.logical_width,
                logical_height: self.logical_height,
            });
        }

        let mut output = Canvas::new(self.window_width, self.window_height)?;
        output.clear(background);
        let left = self.left.floor().max(0.0).min(f64::from(self.window_width)) as u32;
        let top = self.top.floor().max(0.0).min(f64::from(self.window_height)) as u32;
        let right = (self.left + self.width)
            .ceil()
            .max(0.0)
            .min(f64::from(self.window_width)) as u32;
        let bottom = (self.top + self.height)
            .ceil()
            .max(0.0)
            .min(f64::from(self.window_height)) as u32;

        for y in top..bottom {
            for x in left..right {
                let window_x = f64::from(x) + 0.5;
                let window_y = f64::from(y) + 0.5;
                if window_x < self.left
                    || window_y < self.top
                    || window_x >= self.left + self.width
                    || window_y >= self.top + self.height
                {
                    continue;
                }
                let source_x = (window_x - self.left) / self.scale_x;
                let source_y = (window_y - self.top) / self.scale_y;
                let color = match filter {
                    ScaleFilter::Nearest => source.pixel_unchecked(
                        (source_x.floor() as u32).min(source.width - 1),
                        (source_y.floor() as u32).min(source.height - 1),
                    ),
                    ScaleFilter::Linear => sample_linear(source, source_x - 0.5, source_y - 0.5),
                };
                let index = ((y as usize) * (self.window_width as usize) + x as usize) * 4;
                output.pixels[index..index + 4]
                    .copy_from_slice(&[color.r, color.g, color.b, color.a]);
            }
        }
        Ok(output)
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Default)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const TRANSPARENT: Self = Self::rgba(0, 0, 0, 0);
    pub const BLACK: Self = Self::rgba(0, 0, 0, 255);
    pub const WHITE: Self = Self::rgba(255, 255, 255, 255);

    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self::rgba(r, g, b, 255)
    }

    fn with_alpha(self, alpha: u8) -> Self {
        Self { a: alpha, ..self }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum MaskTransitionChannel {
    Red,
    Green,
    Blue,
    Alpha,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum BlendMode {
    #[default]
    SourceOver = 0,
    AddSource = 1,
    AddDestination = 2,
    Multiply = 3,
    InvertDestination = 4,
    AddMultiply = 5,
    ListAlphaFromBlue = 6,
    ListAlphaFromGreen = 7,
    ListAlphaFromRed = 8,
    ListAlphaFromAlpha = 9,
}

impl BlendMode {
    pub const fn from_raw(value: u32) -> Self {
        match value {
            1 => Self::AddSource,
            2 => Self::AddDestination,
            3 => Self::Multiply,
            4 => Self::InvertDestination,
            5 => Self::AddMultiply,
            _ => Self::SourceOver,
        }
    }

    pub const fn raw(self) -> u32 {
        self as u32
    }

    pub const fn list_alpha_mask(channel: u32) -> Option<Self> {
        match channel {
            1 => Some(Self::ListAlphaFromBlue),
            2 => Some(Self::ListAlphaFromGreen),
            3 => Some(Self::ListAlphaFromRed),
            4 => Some(Self::ListAlphaFromAlpha),
            _ => None,
        }
    }

    pub const fn is_list_alpha_mask(self) -> bool {
        matches!(self, Self::ListAlphaFromBlue | Self::ListAlphaFromGreen
            | Self::ListAlphaFromRed | Self::ListAlphaFromAlpha)
    }
}

/// A CatSystem2-style two-dimensional object transform. `position` is where
/// the local `pivot` lands; scale is allowed to be negative for mirroring.
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Transform2D {
    pub position: [f32; 2],
    pub pivot: [f32; 2],
    pub scale: [f32; 2],
    pub rotation: f32,
}

impl Default for Transform2D {
    fn default() -> Self {
        Self {
            position: [0.0; 2],
            pivot: [0.0; 2],
            scale: [1.0; 2],
            rotation: 0.0,
        }
    }
}

impl Transform2D {
    pub const fn new(position: [f32; 2], pivot: [f32; 2], scale: [f32; 2], rotation: f32) -> Self {
        Self {
            position,
            pivot,
            scale,
            rotation,
        }
    }

    pub fn validate(self) -> Result<()> {
        if self
            .position
            .into_iter()
            .chain(self.pivot)
            .chain(self.scale)
            .chain([self.rotation])
            .any(|value| !value.is_finite())
        {
            return Err(RendererError::NonFiniteTransform);
        }
        if self.scale.iter().any(|&value| value == 0.0) {
            return Err(RendererError::InvalidScale {
                scale_x_bits: self.scale[0].to_bits(),
                scale_y_bits: self.scale[1].to_bits(),
            });
        }
        Ok(())
    }

    pub fn map_source_point(self, source: [f32; 2]) -> [f32; 2] {
        let (sin, cos) = self.rotation.sin_cos();
        let local_x = (source[0] - self.pivot[0]) * self.scale[0];
        let local_y = (source[1] - self.pivot[1]) * self.scale[1];
        [
            self.position[0] + local_x * cos - local_y * sin,
            self.position[1] + local_x * sin + local_y * cos,
        ]
    }

    pub fn inverse_map_point(self, destination: [f32; 2]) -> [f32; 2] {
        let (sin, cos) = self.rotation.sin_cos();
        let x = destination[0] - self.position[0];
        let y = destination[1] - self.position[1];
        [
            (x * cos + y * sin) / self.scale[0] + self.pivot[0],
            (-x * sin + y * cos) / self.scale[1] + self.pivot[1],
        ]
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Canvas {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl Canvas {
    pub fn new(width: u32, height: u32) -> Result<Self> {
        let bytes = rgba_len(width, height)?;
        let mut pixels = Vec::new();
        pixels
            .try_reserve_exact(bytes)
            .map_err(|_| RendererError::AllocationFailed { bytes })?;
        pixels.resize(bytes, 0);
        Ok(Self {
            width,
            height,
            pixels,
        })
    }

    pub fn from_rgba8(width: u32, height: u32, pixels: Vec<u8>) -> Result<Self> {
        let expected = rgba_len(width, height)?;
        if pixels.len() != expected {
            return Err(RendererError::BufferLength {
                expected,
                actual: pixels.len(),
            });
        }
        Ok(Self {
            width,
            height,
            pixels,
        })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    pub fn pixels_mut(&mut self) -> &mut [u8] {
        &mut self.pixels
    }

    pub fn clear(&mut self, color: Color) {
        for pixel in self.pixels.chunks_exact_mut(4) {
            pixel.copy_from_slice(&[color.r, color.g, color.b, color.a]);
        }
    }

    pub fn pixel(&self, x: i32, y: i32) -> Result<Color> {
        let index = self.pixel_index(x, y)?;
        Ok(Color::rgba(
            self.pixels[index],
            self.pixels[index + 1],
            self.pixels[index + 2],
            self.pixels[index + 3],
        ))
    }

    pub fn set_pixel(&mut self, x: i32, y: i32, color: Color) -> Result<()> {
        let index = self.pixel_index(x, y)?;
        self.pixels[index..index + 4].copy_from_slice(&[color.r, color.g, color.b, color.a]);
        Ok(())
    }

    pub fn blend_pixel(&mut self, x: i32, y: i32, color: Color) -> Result<()> {
        let index = self.pixel_index(x, y)?;
        let dst = Color::rgba(
            self.pixels[index],
            self.pixels[index + 1],
            self.pixels[index + 2],
            self.pixels[index + 3],
        );
        let blended = blend_rgba(dst, color);
        self.pixels[index..index + 4]
            .copy_from_slice(&[blended.r, blended.g, blended.b, blended.a]);
        Ok(())
    }

    pub fn blend_pixel_with_mode(
        &mut self,
        x: i32,
        y: i32,
        color: Color,
        blend_mode: BlendMode,
    ) -> Result<()> {
        let index = self.pixel_index(x, y)?;
        let dst = Color::rgba(
            self.pixels[index],
            self.pixels[index + 1],
            self.pixels[index + 2],
            self.pixels[index + 3],
        );
        let blended = blend_with_mode(dst, color, blend_mode);
        self.pixels[index..index + 4]
            .copy_from_slice(&[blended.r, blended.g, blended.b, blended.a]);
        Ok(())
    }

    pub fn apply_mask(&mut self, mask: &AlphaMask, clip: Option<Rect>) -> Result<()> {
        validate_mask(Some(mask), self)?;
        let Some(bounds) = self.drawable_bounds(Rect::new(0, 0, self.width, self.height), clip)
        else {
            return Ok(());
        };
        for y in bounds.1..bounds.3 {
            for x in bounds.0..bounds.2 {
                let index = ((y as usize) * self.width as usize + x as usize) * 4;
                self.pixels[index + 3] =
                    modulate_alpha(self.pixels[index + 3], mask.alpha_unchecked(x, y));
            }
        }
        Ok(())
    }

    pub fn fill_rect(&mut self, rect: Rect, color: Color) -> Result<()> {
        self.fill_rect_clipped(rect, color, None)
    }

    pub fn fill_rect_clipped(
        &mut self,
        rect: Rect,
        color: Color,
        clip: Option<Rect>,
    ) -> Result<()> {
        self.fill_rect_with_blend(rect, color, 255, clip, BlendMode::SourceOver)
    }

    pub fn fill_rect_with_blend(
        &mut self,
        rect: Rect,
        color: Color,
        opacity: u8,
        clip: Option<Rect>,
        blend_mode: BlendMode,
    ) -> Result<()> {
        let Some(bounds) = self.drawable_bounds(rect, clip) else {
            return Ok(());
        };
        for y in bounds.1..bounds.3 {
            for x in bounds.0..bounds.2 {
                self.blend_pixel_unchecked_with_mode(
                    x,
                    y,
                    color.with_alpha(modulate_alpha(color.a, opacity)),
                    blend_mode,
                );
            }
        }
        Ok(())
    }

    pub fn draw_frame(&mut self, frame: &RgbaFrame, options: &ImageDrawOptions) -> Result<()> {
        self.draw_frame_with_workers(frame, options, 1)
    }

    /// Draws a frame with an optional worker budget for large, uncached linear
    /// affine transforms. The budget includes the caller and is capped at four.
    /// Other paths, including cache hits, draw serially in layer order.
    pub fn draw_frame_with_workers(
        &mut self,
        frame: &RgbaFrame,
        options: &ImageDrawOptions,
        workers: usize,
    ) -> Result<()> {
        if !options.visible || frame.is_empty() {
            return Ok(());
        }
        validate_mask(options.mask.as_ref(), self)?;
        if !options.rotation_radians.is_finite()
            || options.pivot.iter().any(|value| !value.is_finite())
        {
            return Err(RendererError::NonFiniteTransform);
        }
        let (scaled_width, scaled_height) = scaled_dimensions(frame.width, frame.height, options)?;
        if options.rotation_radians != 0.0
            || options.pivot != [0.0, 0.0]
            || options.flip_x
            || options.flip_y
            || options.scale_x < 0.0
            || options.scale_y < 0.0
        {
            return self.draw_transformed_frame(frame, options, workers);
        }
        let Some((left, top, right, bottom)) = self.drawable_bounds(
            Rect::new(
                options.position.x,
                options.position.y,
                scaled_width,
                scaled_height,
            ),
            options.clip,
        ) else {
            return Ok(());
        };

        if options.scale_x == 1.0
            && options.scale_y == 1.0
            && options.filter == ScaleFilter::Nearest
            && options.blend_mode == BlendMode::SourceOver
            && options.opacity == 255
            && options.mask.is_none()
        {
            let source_x = (i64::from(left) - i64::from(options.position.x)) as usize;
            let source_top = (i64::from(top) - i64::from(options.position.y)) as usize;
            let row_bytes = (right - left) as usize * 4;
            let opaque = frame.pixels.is_opaque();
            for (row, y) in (top..bottom).enumerate() {
                let source_at = ((source_top + row) * frame.width as usize + source_x) * 4;
                let target_at = (y as usize * self.width as usize + left as usize) * 4;
                let source = &frame.pixels[source_at..source_at + row_bytes];
                let target = &mut self.pixels[target_at..target_at + row_bytes];
                if opaque {
                    target.copy_from_slice(source);
                } else {
                    for (target, source) in target
                        .as_chunks_mut::<4>().0.iter_mut()
                        .zip(source.as_chunks::<4>().0)
                    {
                        blend_source_over(target, source);
                    }
                }
            }
            return Ok(());
        }

        raster::draw_axis_aligned(self, frame, options, (left, top, right, bottom));
        Ok(())
    }

    pub fn draw_frame_with_metadata(
        &mut self,
        frame: &RgbaFrame,
        position: Point,
        opacity: u8,
        clip: Option<Rect>,
    ) -> Result<()> {
        let mut options = ImageDrawOptions::new(frame.screen_position(position));
        options.opacity = opacity;
        options.clip = clip;
        self.draw_frame(frame, &options)
    }

    /// Directly copies a frame into the canvas. This is used by image-list
    /// resize/capture paths where source alpha must not be composited.
    pub fn copy_frame(&mut self, frame: &RgbaFrame, options: &ImageDrawOptions) -> Result<()> {
        if !options.visible || frame.is_empty() {
            return Ok(());
        }
        validate_mask(options.mask.as_ref(), self)?;
        let (scaled_width, scaled_height) = scaled_dimensions(frame.width, frame.height, options)?;
        if options.rotation_radians != 0.0
            || options.pivot != [0.0, 0.0]
            || options.flip_x
            || options.flip_y
            || options.scale_x < 0.0
            || options.scale_y < 0.0
        {
            return Err(RendererError::InvalidEffectParameter {
                name: "copy_frame transform",
            });
        }
        let Some(bounds) = self.drawable_bounds(
            Rect::new(
                options.position.x,
                options.position.y,
                scaled_width,
                scaled_height,
            ),
            options.clip,
        ) else {
            return Ok(());
        };
        if options.scale_x == 1.0
            && options.scale_y == 1.0
            && options.filter == ScaleFilter::Nearest
            && options.opacity == 255
            && options.mask.is_none()
        {
            let source_x = (i64::from(bounds.0) - i64::from(options.position.x)) as usize;
            let row_bytes = (bounds.2 - bounds.0) as usize * 4;
            for y in bounds.1..bounds.3 {
                let source_y = (i64::from(y) - i64::from(options.position.y)) as usize;
                let source_at = (source_y * frame.width as usize + source_x) * 4;
                let target_at = (y as usize * self.width as usize + bounds.0 as usize) * 4;
                self.pixels[target_at..target_at + row_bytes]
                    .copy_from_slice(&frame.pixels[source_at..source_at + row_bytes]);
            }
            return Ok(());
        }
        for y in bounds.1..bounds.3 {
            for x in bounds.0..bounds.2 {
                let source_x =
                    (f64::from(x) - f64::from(options.position.x)) / f64::from(options.scale_x);
                let source_y =
                    (f64::from(y) - f64::from(options.position.y)) / f64::from(options.scale_y);
                let source = sample_frame(frame, source_x, source_y, options.filter);
                let source = if options.blend_mode.is_list_alpha_mask() { source } else {
                    apply_opacity_and_mask(source, options.opacity, options.mask.as_ref(), x, y)
                };
                let index = ((y as usize) * self.width as usize + x as usize) * 4;
                self.pixels[index..index + 4]
                    .copy_from_slice(&[source.r, source.g, source.b, source.a]);
            }
        }
        Ok(())
    }

    pub fn crossfade_frames(
        &mut self,
        from: &RgbaFrame,
        to: &RgbaFrame,
        progress: u16,
        position: Point,
        clip: Option<Rect>,
    ) -> Result<()> {
        ensure_same_dimensions(from, to)?;
        let weight = u32::from(progress.min(256));
        let rect = Rect::new(position.x, position.y, from.width, from.height);
        let Some(bounds) = self.drawable_bounds(rect, clip) else {
            return Ok(());
        };
        for y in bounds.1..bounds.3 {
            for x in bounds.0..bounds.2 {
                let source_x = (i64::from(x) - i64::from(position.x)) as usize;
                let source_y = (i64::from(y) - i64::from(position.y)) as usize;
                let index = (source_y * from.width as usize + source_x) * 4;
                let pixel: [u8; 4] = std::array::from_fn(|channel| {
                    ((u32::from(from.pixels[index + channel]) * (256 - weight)
                        + u32::from(to.pixels[index + channel]) * weight)
                        >> 8) as u8
                });
                let target = ((y as usize) * self.width as usize + x as usize) * 4;
                self.pixels[target..target + 4].copy_from_slice(&pixel);
            }
        }
        Ok(())
    }

    /// Applies the observed CatSystem2 mask transition. The progress scale is
    /// 0..512; a mask sample of 0 starts the transition later than 255.
    pub fn mask_transition(
        &mut self,
        from: &RgbaFrame,
        to: &RgbaFrame,
        mask: &RgbaFrame,
        progress: u16,
        channel: MaskTransitionChannel,
        position: Point,
        clip: Option<Rect>,
    ) -> Result<()> {
        ensure_same_dimensions(from, to)?;
        ensure_same_dimensions(from, mask)?;
        let weight = i32::from(progress.min(512));
        let rect = Rect::new(position.x, position.y, from.width, from.height);
        let Some(bounds) = self.drawable_bounds(rect, clip) else {
            return Ok(());
        };
        for y in bounds.1..bounds.3 {
            for x in bounds.0..bounds.2 {
                let source_x = (i64::from(x) - i64::from(position.x)) as usize;
                let source_y = (i64::from(y) - i64::from(position.y)) as usize;
                let index = (source_y * from.width as usize + source_x) * 4;
                let shade = match channel {
                    MaskTransitionChannel::Red => mask.pixels[index],
                    MaskTransitionChannel::Green => mask.pixels[index + 1],
                    MaskTransitionChannel::Blue => mask.pixels[index + 2],
                    MaskTransitionChannel::Alpha => mask.pixels[index + 3],
                };
                let local_weight = (weight + i32::from(shade) - 256).clamp(0, 256) as u32;
                let pixel: [u8; 4] = std::array::from_fn(|channel| {
                    ((u32::from(from.pixels[index + channel]) * (256 - local_weight)
                        + u32::from(to.pixels[index + channel]) * local_weight)
                        >> 8) as u8
                });
                let target = ((y as usize) * self.width as usize + x as usize) * 4;
                self.pixels[target..target + 4].copy_from_slice(&pixel);
            }
        }
        Ok(())
    }

    pub fn draw_mosaic(
        &mut self,
        frame: &RgbaFrame,
        block_size: u32,
        algorithm: MosaicAlgorithm,
        position: Point,
        opacity: u8,
        clip: Option<Rect>,
    ) -> Result<()> {
        let mosaic = frame.pixelated(block_size, algorithm)?;
        let mut options = ImageDrawOptions::new(position);
        options.opacity = opacity;
        options.clip = clip;
        self.draw_frame(&mosaic, &options)
    }

    /// Builds a nine-frame IMGWND-style window from frame IDs 0..8.
    pub fn draw_window(
        &mut self,
        frames: &[Option<RgbaFrame>; 9],
        area: Rect,
        opacity: u8,
    ) -> Result<()> {
        self.draw_window_clipped(frames, area, opacity, None)
    }

    pub fn draw_window_clipped(
        &mut self,
        frames: &[Option<RgbaFrame>; 9],
        area: Rect,
        opacity: u8,
        clip: Option<Rect>,
    ) -> Result<()> {
        let frame = Self::compose_window(frames, [area.width, area.height])?;
        let mut options = ImageDrawOptions::new(Point::new(area.x, area.y));
        options.opacity = opacity;
        options.clip = clip;
        self.draw_frame(&frame, &options)
    }

    /// Composes the nine source frames into a standalone raster. Missing
    /// frame IDs remain transparent and are never replaced with a synthetic
    /// pixel.
    pub fn compose_window(frames: &[Option<RgbaFrame>; 9], size: [u32; 2]) -> Result<RgbaFrame> {
        if size[0] == 0 || size[1] == 0 {
            return Err(RendererError::InvalidEffectParameter {
                name: "window dimensions",
            });
        }
        let mut canvas = Canvas::new(size[0], size[1])?;
        canvas.draw_window_tiles(frames, Rect::new(0, 0, size[0], size[1]), 255)?;
        let mut metadata = FrameMetadata::composed(size);
        metadata.alpha_flag = u32::from(canvas.pixels.chunks_exact(4).any(|pixel| pixel[3] != 255));
        RgbaFrame::from_rgba8_with_metadata(size[0], size[1], canvas.pixels, metadata)
    }

    /// Composes an IMGWND raster and applies its window opacity to both the
    /// pixels and the resulting alpha metadata.
    pub fn compose_window_with_opacity(
        frames: &[Option<RgbaFrame>; 9],
        size: [u32; 2],
        opacity: u8,
    ) -> Result<RgbaFrame> {
        Self::compose_window(frames, size)?.with_alpha(opacity)
    }

    pub fn draw_window_transformed(
        &mut self,
        frames: &[Option<RgbaFrame>; 9],
        area: Rect,
        options: &ImageDrawOptions,
    ) -> Result<()> {
        if area.width == 0 || area.height == 0 {
            return Err(RendererError::InvalidEffectParameter {
                name: "window dimensions",
            });
        }
        let frame = Self::compose_window(frames, [area.width, area.height])?;
        self.draw_frame(&frame, options)
    }

    fn draw_window_tiles(
        &mut self,
        frames: &[Option<RgbaFrame>; 9],
        area: Rect,
        opacity: u8,
    ) -> Result<()> {
        if area.width == 0 || area.height == 0 {
            return Err(RendererError::InvalidEffectParameter {
                name: "window dimensions",
            });
        }
        let [window_width, window_height] = [area.width, area.height];
        for frame in frames.iter().flatten() {
            if frame.width == 0 || frame.height == 0 {
                return Err(RendererError::InvalidEffectParameter {
                    name: "window tile dimensions",
                });
            }
        }
        for (index, frame) in frames.iter().enumerate() {
            let Some(frame) = frame else {
                continue;
            };
            let horizontal = matches!(index, 6..=8);
            let vertical = matches!(index, 4 | 5 | 8);
            if (horizontal && window_width < frame.width)
                || (vertical && window_height < frame.height)
            {
                return Err(RendererError::InvalidEffectParameter {
                    name: "window is smaller than a tiled edge",
                });
            }
            let x_values = if horizontal {
                (frame.width..window_width.saturating_sub(frame.width))
                    .step_by(frame.width as usize)
                    .collect::<Vec<_>>()
            } else {
                vec![if matches!(index, 1 | 3 | 5) {
                    window_width.saturating_sub(frame.width)
                } else {
                    0
                }]
            };
            let y_values = if vertical {
                (frame.height..window_height.saturating_sub(frame.height))
                    .step_by(frame.height as usize)
                    .collect::<Vec<_>>()
            } else {
                vec![if matches!(index, 2 | 3 | 7) {
                    window_height.saturating_sub(frame.height)
                } else {
                    0
                }]
            };
            for y in y_values {
                for x in x_values.iter().copied() {
                    let tile_width = if horizontal {
                        frame.width.min(window_width - x - frame.width)
                    } else {
                        frame.width
                    };
                    let tile_height = if vertical {
                        frame.height.min(window_height - y - frame.height)
                    } else {
                        frame.height
                    };
                    if tile_width == 0 || tile_height == 0 {
                        continue;
                    }
                    let mut options = ImageDrawOptions::new(Point::new(
                        area.x.saturating_add(x as i32),
                        area.y.saturating_add(y as i32),
                    ));
                    options.opacity = opacity;
                    options.clip = Some(Rect::new(
                        area.x.saturating_add(x as i32),
                        area.y.saturating_add(y as i32),
                        tile_width,
                        tile_height,
                    ));
                    self.draw_frame(frame, &options)?;
                }
            }
        }
        Ok(())
    }

    fn draw_transformed_frame(
        &mut self,
        frame: &RgbaFrame,
        options: &ImageDrawOptions,
        workers: usize,
    ) -> Result<()> {
        let scale_x = f64::from(options.scale_x) * if options.flip_x { -1.0 } else { 1.0 };
        let scale_y = f64::from(options.scale_y) * if options.flip_y { -1.0 } else { 1.0 };
        let pivot_x = f64::from(options.pivot[0]);
        let pivot_y = f64::from(options.pivot[1]);
        let angle = f64::from(options.rotation_radians);
        let (sin, cos) = angle.sin_cos();
        let position_x = f64::from(options.position.x);
        let position_y = f64::from(options.position.y);

        let transform = |source_x: f64, source_y: f64| {
            let local_x = (source_x - pivot_x) * scale_x;
            let local_y = (source_y - pivot_y) * scale_y;
            [
                position_x + local_x * cos - local_y * sin,
                position_y + local_x * sin + local_y * cos,
            ]
        };
        let corners = [
            transform(0.0, 0.0),
            transform(f64::from(frame.width), 0.0),
            transform(0.0, f64::from(frame.height)),
            transform(f64::from(frame.width), f64::from(frame.height)),
        ];
        let min_x = corners
            .iter()
            .map(|point| point[0])
            .fold(f64::INFINITY, f64::min);
        let min_y = corners
            .iter()
            .map(|point| point[1])
            .fold(f64::INFINITY, f64::min);
        let max_x = corners
            .iter()
            .map(|point| point[0])
            .fold(f64::NEG_INFINITY, f64::max);
        let max_y = corners
            .iter()
            .map(|point| point[1])
            .fold(f64::NEG_INFINITY, f64::max);
        let Some((left, top, right, bottom)) =
            self.drawable_float_bounds(min_x, min_y, max_x, max_y, options.clip)
        else {
            return Ok(());
        };

        raster::draw_sampled(
            self, frame, options, (left, top, right, bottom), true, workers, |x, y| {
                let relative_x = (f64::from(x) + 0.5) - position_x;
                let relative_y = (f64::from(y) + 0.5) - position_y;
                let source_x = (relative_x * cos + relative_y * sin) / scale_x + pivot_x;
                let source_y = (-relative_x * sin + relative_y * cos) / scale_y + pivot_y;
                if source_x < 0.0
                    || source_y < 0.0
                    || source_x >= f64::from(frame.width)
                    || source_y >= f64::from(frame.height)
                {
                    return None;
                }
                Some(sample_frame(frame, source_x, source_y, options.filter))
            },
        );
        Ok(())
    }

    pub fn draw_particle(&mut self, particle: ParticlePoint) -> Result<()> {
        self.draw_particle_clipped(particle, None, None)
    }

    pub fn draw_particle_clipped(
        &mut self,
        particle: ParticlePoint,
        clip: Option<Rect>,
        mask: Option<&AlphaMask>,
    ) -> Result<()> {
        self.draw_particle_with_blend(particle, 255, clip, mask, BlendMode::SourceOver)
    }

    pub fn draw_particle_with_blend(
        &mut self,
        particle: ParticlePoint,
        opacity: u8,
        clip: Option<Rect>,
        mask: Option<&AlphaMask>,
        blend_mode: BlendMode,
    ) -> Result<()> {
        validate_mask(mask, self)?;
        if !self.in_bounds(particle.position.x, particle.position.y)
            || !clip.is_none_or(|clip| point_in_rect(particle.position, clip))
        {
            return Ok(());
        }
        let color = apply_opacity_and_mask(
            particle.color,
            opacity,
            mask,
            particle.position.x as u32,
            particle.position.y as u32,
        );
        self.blend_pixel_unchecked_with_mode(
            particle.position.x as u32,
            particle.position.y as u32,
            color,
            blend_mode,
        );
        Ok(())
    }

    pub fn draw_particles(
        &mut self,
        particles: &[ParticlePoint],
        clip: Option<Rect>,
        mask: Option<&AlphaMask>,
    ) -> Result<()> {
        for &particle in particles {
            self.draw_particle_clipped(particle, clip, mask)?;
        }
        Ok(())
    }

    pub fn draw_particle_sprite(
        &mut self,
        particle: &Particle,
        clip: Option<Rect>,
        mask: Option<&AlphaMask>,
        blend_mode: BlendMode,
    ) -> Result<()> {
        validate_mask(mask, self)?;
        particle.validate()?;
        if !particle.is_alive() {
            return Ok(());
        }
        if particle.size <= 1.0 {
            return self.draw_particle_with_blend(
                ParticlePoint::new(
                    Point::new(
                        particle.position[0].floor() as i32,
                        particle.position[1].floor() as i32,
                    ),
                    particle.color,
                ),
                particle.opacity(),
                clip,
                mask,
                blend_mode,
            );
        }

        let radius = f64::from(particle.size) * 0.5;
        let left = (f64::from(particle.position[0]) - radius).floor() as i64;
        let top = (f64::from(particle.position[1]) - radius).floor() as i64;
        let right = (f64::from(particle.position[0]) + radius).ceil() as i64;
        let bottom = (f64::from(particle.position[1]) + radius).ceil() as i64;
        for y in top..bottom {
            for x in left..right {
                let point = Point::new(x as i32, y as i32);
                if !self.in_bounds(point.x, point.y)
                    || !clip.is_none_or(|clip| point_in_rect(point, clip))
                {
                    continue;
                }
                let dx = x as f64 + 0.5 - f64::from(particle.position[0]);
                let dy = y as f64 + 0.5 - f64::from(particle.position[1]);
                let distance = (dx * dx + dy * dy).sqrt();
                if distance > radius {
                    continue;
                }
                let edge = (radius - distance).clamp(0.0, 1.0);
                let coverage = (edge * 255.0).round() as u8;
                let color = apply_opacity_and_mask(
                    particle
                        .color
                        .with_alpha(modulate_alpha(particle.color.a, coverage)),
                    particle.opacity(),
                    mask,
                    point.x as u32,
                    point.y as u32,
                );
                self.blend_pixel_unchecked_with_mode(
                    point.x as u32,
                    point.y as u32,
                    color,
                    blend_mode,
                );
            }
        }
        Ok(())
    }

    pub fn draw_glyph(
        &mut self,
        glyph: &GlyphMask,
        position: Point,
        color: Color,
        opacity: u8,
        clip: Option<Rect>,
        mask: Option<&AlphaMask>,
    ) -> Result<()> {
        self.draw_glyph_with_blend(
            glyph,
            position,
            color,
            opacity,
            clip,
            mask,
            BlendMode::SourceOver,
        )
    }

    pub fn draw_glyph_with_blend(
        &mut self,
        glyph: &GlyphMask,
        position: Point,
        color: Color,
        opacity: u8,
        clip: Option<Rect>,
        mask: Option<&AlphaMask>,
        blend_mode: BlendMode,
    ) -> Result<()> {
        validate_mask(mask, self)?;
        let Some(bounds) = self.drawable_bounds(
            Rect::new(position.x, position.y, glyph.width, glyph.height),
            clip,
        ) else {
            return Ok(());
        };

        for y in bounds.1..bounds.3 {
            for x in bounds.0..bounds.2 {
                let glyph_x = (i64::from(x) - i64::from(position.x)) as u32;
                let glyph_y = (i64::from(y) - i64::from(position.y)) as u32;
                let glyph_alpha = glyph.alpha_unchecked(glyph_x, glyph_y);
                let source = color.with_alpha(modulate_alpha(color.a, glyph_alpha));
                let source = apply_opacity_and_mask(source, opacity, mask, x, y);
                self.blend_pixel_unchecked_with_mode(x, y, source, blend_mode);
            }
        }
        Ok(())
    }

    pub fn draw_glyph_at_baseline(
        &mut self,
        glyph: &GlyphMask,
        baseline: Point,
        color: Color,
        opacity: u8,
        clip: Option<Rect>,
        mask: Option<&AlphaMask>,
        blend_mode: BlendMode,
    ) -> Result<()> {
        let position = Point::new(
            baseline.x.saturating_add(glyph.metrics.bearing_x),
            baseline.y.saturating_sub(glyph.metrics.bearing_y),
        );
        self.draw_glyph_with_blend(glyph, position, color, opacity, clip, mask, blend_mode)
    }

    pub fn draw_glyph_run(
        &mut self,
        glyphs: &[PositionedGlyph],
        color: Color,
        opacity: u8,
        clip: Option<Rect>,
        mask: Option<&AlphaMask>,
        blend_mode: BlendMode,
    ) -> Result<()> {
        validate_mask(mask, self)?;
        for glyph in glyphs {
            self.draw_glyph_at_baseline(
                &glyph.glyph,
                glyph.baseline,
                color,
                opacity,
                clip,
                mask,
                blend_mode,
            )?;
        }
        Ok(())
    }

    pub fn render_queue(&mut self, queue: &RenderQueue) -> Result<()> {
        queue.render_into(self)
    }

    pub fn snapshot(&self) -> FrameSnapshot {
        FrameSnapshot {
            width: self.width,
            height: self.height,
            rgba: self.pixels.clone(),
            hash: frame_hash(self.width, self.height, &self.pixels),
        }
    }

    pub fn hash(&self) -> u64 {
        frame_hash(self.width, self.height, &self.pixels)
    }

    fn pixel_index(&self, x: i32, y: i32) -> Result<usize> {
        if !self.in_bounds(x, y) {
            return Err(RendererError::CoordinateOutOfBounds {
                x: i64::from(x),
                y: i64::from(y),
                width: self.width,
                height: self.height,
            });
        }
        Ok(((y as usize) * (self.width as usize) + (x as usize)) * 4)
    }

    fn in_bounds(&self, x: i32, y: i32) -> bool {
        x >= 0 && y >= 0 && (x as u32) < self.width && (y as u32) < self.height
    }

    fn drawable_bounds(&self, rect: Rect, clip: Option<Rect>) -> Option<(u32, u32, u32, u32)> {
        let mut left = i64::from(rect.x).max(0);
        let mut top = i64::from(rect.y).max(0);
        let mut right = rect.right().min(i64::from(self.width));
        let mut bottom = rect.bottom().min(i64::from(self.height));
        if let Some(clip) = clip {
            left = left.max(i64::from(clip.x));
            top = top.max(i64::from(clip.y));
            right = right.min(clip.right());
            bottom = bottom.min(clip.bottom());
        }
        if left >= right || top >= bottom {
            return None;
        }
        Some((left as u32, top as u32, right as u32, bottom as u32))
    }

    fn drawable_float_bounds(
        &self,
        left: f64,
        top: f64,
        right: f64,
        bottom: f64,
        clip: Option<Rect>,
    ) -> Option<(u32, u32, u32, u32)> {
        let mut left = left.floor().max(0.0).min(f64::from(self.width)) as i64;
        let mut top = top.floor().max(0.0).min(f64::from(self.height)) as i64;
        let mut right = right.ceil().max(0.0).min(f64::from(self.width)) as i64;
        let mut bottom = bottom.ceil().max(0.0).min(f64::from(self.height)) as i64;
        if let Some(clip) = clip {
            left = left.max(i64::from(clip.x));
            top = top.max(i64::from(clip.y));
            right = right.min(clip.right());
            bottom = bottom.min(clip.bottom());
        }
        if left >= right || top >= bottom {
            return None;
        }
        Some((left as u32, top as u32, right as u32, bottom as u32))
    }

    fn blend_pixel_unchecked_with_mode(
        &mut self,
        x: u32,
        y: u32,
        color: Color,
        blend_mode: BlendMode,
    ) {
        let index = ((y as usize) * (self.width as usize) + (x as usize)) * 4;
        blend_into(
            self.pixels[index..].first_chunk_mut().unwrap(),
            color,
            blend_mode,
        );
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RgbaFrame {
    width: u32,
    height: u32,
    pixels: Arc<FramePixels>,
    metadata: FrameMetadata,
    attributes: HgFrameAttributes,
}

#[derive(Debug)]
struct FramePixels {
    bytes: Vec<u8>,
    opaque: OnceLock<bool>,
}

impl FramePixels {
    fn new(bytes: Vec<u8>) -> Self {
        Self {
            bytes,
            opaque: OnceLock::new(),
        }
    }

    fn is_opaque(&self) -> bool {
        *self.opaque.get_or_init(|| {
            self.bytes.as_chunks::<4>().0.iter().all(|pixel| pixel[3] == 255)
        })
    }
}

impl PartialEq for FramePixels {
    fn eq(&self, other: &Self) -> bool {
        self.bytes == other.bytes
    }
}

impl Eq for FramePixels {}

impl Deref for FramePixels {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        &self.bytes
    }
}

impl RgbaFrame {
    pub fn from_rgba8(width: u32, height: u32, pixels: Vec<u8>) -> Result<Self> {
        Self::from_rgba8_with_metadata(
            width,
            height,
            pixels,
            FrameMetadata::raster([width, height]),
        )
    }

    pub fn from_rgba8_with_metadata(
        width: u32,
        height: u32,
        pixels: Vec<u8>,
        mut metadata: FrameMetadata,
    ) -> Result<Self> {
        let expected = rgba_len(width, height)?;
        if pixels.len() != expected {
            return Err(RendererError::BufferLength {
                expected,
                actual: pixels.len(),
            });
        }
        if metadata.canvas_size == [0, 0] {
            metadata.canvas_size = [width, height];
        }
        Ok(Self {
            width,
            height,
            pixels: Arc::new(FramePixels::new(pixels)),
            metadata,
            attributes: HgFrameAttributes::default(),
        })
    }

    /// Converts an already-decoded HG BGRA8 frame to the renderer's RGBA8
    /// storage while preserving the container's crop and canvas geometry.
    pub fn from_hg_bgra8(
        width: u32,
        height: u32,
        pixels: &[u8],
        metadata: FrameMetadata,
    ) -> Result<Self> {
        Self::from_hg_bgra8_with_attributes(
            width,
            height,
            pixels,
            metadata,
            HgFrameAttributes::default(),
        )
    }

    pub fn from_hg_bgra8_with_attributes(
        width: u32,
        height: u32,
        pixels: &[u8],
        metadata: FrameMetadata,
        attributes: HgFrameAttributes,
    ) -> Result<Self> {
        let expected = rgba_len(width, height)?;
        if pixels.len() != expected {
            return Err(RendererError::BufferLength {
                expected,
                actual: pixels.len(),
            });
        }
        let mut rgba = Vec::new();
        rgba.try_reserve_exact(expected)
            .map_err(|_| RendererError::AllocationFailed { bytes: expected })?;
        for pixel in pixels.chunks_exact(4) {
            rgba.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
        }
        let mut frame = Self::from_rgba8_with_metadata(width, height, rgba, metadata)?;
        frame.attributes = attributes;
        Ok(frame)
    }

    pub fn from_hg_bgra8_with_frame_info(
        pixels: &[u8],
        container: HgContainer,
        info: &HgFrameInfo,
    ) -> Result<Self> {
        Self::from_hg_bgra8_with_attributes(
            info.width,
            info.height,
            pixels,
            FrameMetadata::from_hg_frame(container, info),
            info.attributes_view(),
        )
    }

    pub fn from_hg_rgba8(
        width: u32,
        height: u32,
        pixels: &[u8],
        metadata: FrameMetadata,
    ) -> Result<Self> {
        Self::from_hg_rgba8_with_attributes(
            width,
            height,
            pixels,
            metadata,
            HgFrameAttributes::default(),
        )
    }

    pub fn from_hg_rgba8_with_attributes(
        width: u32,
        height: u32,
        pixels: &[u8],
        metadata: FrameMetadata,
        attributes: HgFrameAttributes,
    ) -> Result<Self> {
        let expected = rgba_len(width, height)?;
        if pixels.len() != expected {
            return Err(RendererError::BufferLength {
                expected,
                actual: pixels.len(),
            });
        }
        let mut frame = Self::from_rgba8_with_metadata(width, height, pixels.to_vec(), metadata)?;
        frame.attributes = attributes;
        Ok(frame)
    }

    pub fn from_hg_rgba8_with_frame_info(
        pixels: &[u8],
        container: HgContainer,
        info: &HgFrameInfo,
    ) -> Result<Self> {
        Self::from_hg_rgba8_with_attributes(
            info.width,
            info.height,
            pixels,
            FrameMetadata::from_hg_frame(container, info),
            info.attributes_view(),
        )
    }

    /// Converts a decoded 24-bit HG frame; HG's absent alpha channel is opaque.
    pub fn from_hg_bgr24(
        width: u32,
        height: u32,
        pixels: &[u8],
        metadata: FrameMetadata,
    ) -> Result<Self> {
        Self::from_hg_bgr24_with_attributes(
            width,
            height,
            pixels,
            metadata,
            HgFrameAttributes::default(),
        )
    }

    pub fn from_hg_bgr24_with_attributes(
        width: u32,
        height: u32,
        pixels: &[u8],
        metadata: FrameMetadata,
        attributes: HgFrameAttributes,
    ) -> Result<Self> {
        let count = alpha_len(width, height)?;
        let expected = count
            .checked_mul(3)
            .ok_or(RendererError::ScaledDimensionsOverflow)?;
        if pixels.len() != expected {
            return Err(RendererError::BufferLength {
                expected,
                actual: pixels.len(),
            });
        }
        let rgba_bytes = count
            .checked_mul(4)
            .ok_or(RendererError::ScaledDimensionsOverflow)?;
        let mut rgba = Vec::new();
        rgba.try_reserve_exact(rgba_bytes)
            .map_err(|_| RendererError::AllocationFailed { bytes: rgba_bytes })?;
        for pixel in pixels.chunks_exact(3) {
            rgba.extend_from_slice(&[pixel[2], pixel[1], pixel[0], 255]);
        }
        let mut frame = Self::from_rgba8_with_metadata(width, height, rgba, metadata)?;
        frame.attributes = attributes;
        Ok(frame)
    }

    pub fn from_hg_bgr24_with_frame_info(
        pixels: &[u8],
        container: HgContainer,
        info: &HgFrameInfo,
    ) -> Result<Self> {
        Self::from_hg_bgr24_with_attributes(
            info.width,
            info.height,
            pixels,
            FrameMetadata::from_hg_frame(container, info),
            info.attributes_view(),
        )
    }

    pub fn from_image(image: image::RgbaImage) -> Result<Self> {
        let (width, height) = image.dimensions();
        Self::from_rgba8(width, height, image.into_raw())
    }

    pub fn decode(encoded: &[u8]) -> Result<Self> {
        let image = image::load_from_memory(encoded)
            .map_err(|error| RendererError::Decode {
                message: error.to_string(),
            })?
            .to_rgba8();
        Self::from_image(image)
    }

    /// Decodes one HG-2/HG-3 frame, including the non-native HG-3 image tags
    /// used by real game archives. Pixel decoding, including JPEG/WebP and
    /// optional `img_al` masks, is delegated to the formats crate; the
    /// renderer attaches the crop, canvas, base point and frame attributes.
    pub fn decode_hg(encoded: &[u8], selection: AnimationFrameSelection) -> Result<Self> {
        let file = HgFile::parse(encoded).map_err(|error| RendererError::Decode {
            message: error.to_string(),
        })?;
        let frame_index = match selection {
            AnimationFrameSelection::Index(index) => index,
            AnimationFrameSelection::Id(id) => file
                .frames
                .iter()
                .position(|frame| frame.id == id)
                .ok_or(RendererError::FrameIdNotFound { id })?,
        };
        if frame_index >= file.frames.len() {
            return Err(RendererError::FrameIndexOutOfBounds {
                index: frame_index,
                count: file.frames.len(),
            });
        }
        let inspection = inspect_hg(encoded)?;
        let info = inspection
            .frames
            .get(frame_index)
            .ok_or(RendererError::InvalidHgStructure {
                message: "HG frame metadata and format frame count differ".into(),
            })?;
        let pixels = file
            .decode_frame_rgba8(encoded, frame_index)
            .map_err(|error| RendererError::Decode {
                message: error.to_string(),
            })?;
        Self::from_hg_rgba8_with_frame_info(&pixels, inspection.container, info)
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    pub fn metadata(&self) -> FrameMetadata {
        self.metadata
    }

    /// Preserve explicit engine image geometry, including a zero canvas size.
    /// This changes metadata only; dimensions and pixel allocation stay valid.
    pub fn with_raw_metadata(mut self, metadata: FrameMetadata) -> Self {
        self.metadata = metadata;
        self
    }

    pub fn hg_attributes(&self) -> &HgFrameAttributes {
        &self.attributes
    }

    pub fn attribute(&self, id: u16) -> Option<[u32; 5]> {
        self.attributes.get(id)
    }

    pub fn with_hg_attributes(mut self, attributes: HgFrameAttributes) -> Self {
        self.attributes = attributes;
        self
    }

    pub fn set_hg_attributes(&mut self, attributes: HgFrameAttributes) {
        self.attributes = attributes;
    }

    fn rebuild(
        &self,
        width: u32,
        height: u32,
        pixels: Vec<u8>,
        metadata: FrameMetadata,
    ) -> Result<Self> {
        let mut frame = if width == 0 || height == 0 {
            Self::from_native_rgba8(width, height, pixels, metadata)?
        } else {
            Self::from_rgba8_with_metadata(width, height, pixels, metadata)?
        };
        frame.attributes = self.attributes.clone();
        Ok(frame)
    }

    pub fn has_alpha(&self) -> bool {
        self.metadata.alpha_flag != 0 || !self.pixels.is_opaque()
    }

    pub fn with_alpha(&self, alpha: u8) -> Result<Self> {
        let mut pixels = self.pixels().to_vec();
        for pixel in pixels.chunks_exact_mut(4) {
            pixel[3] = modulate_alpha(pixel[3], alpha);
        }
        let mut metadata = self.metadata;
        metadata.alpha_flag = u32::from(alpha != 255 || self.has_alpha());
        self.rebuild(self.width, self.height, pixels, metadata)
    }

    pub fn canvas_dimensions(&self) -> (u32, u32) {
        if self.metadata.canvas_size == [0, 0] {
            self.dimensions()
        } else {
            (self.metadata.canvas_size[0], self.metadata.canvas_size[1])
        }
    }

    pub fn screen_position(&self, position: Point) -> Point {
        self.metadata.screen_position(position)
    }

    /// Composes static frames in their HG canvas coordinate system.
    ///
    /// The pixel bounds are the union of all frame crop rectangles. Canvas
    /// dimensions use the largest declared canvas, and the first frame keeps
    /// its base point. HG attributes are retained only when every layer has
    /// the same attribute map; conflicting frame-local metadata is rejected
    /// instead of being attached to an unrelated composite. An empty input
    /// returns `None` because no pixel source exists to render.
    pub fn from_layers(layers: &[Self]) -> Result<Option<Self>> {
        let Some(first) = layers.first() else {
            return Ok(None);
        };
        if layers.len() == 1 {
            return Ok(Some(first.clone()));
        }
        let attributes = first.attributes.clone();
        for layer in &layers[1..] {
            if layer.attributes != attributes {
                let id = attributes
                    .iter()
                    .find_map(|(&id, values)| {
                        (layer.attributes.get(id) != Some(*values)).then_some(id)
                    })
                    .or_else(|| {
                        layer.attributes.iter().find_map(|(&id, values)| {
                            (attributes.get(id) != Some(*values)).then_some(id)
                        })
                    })
                    .unwrap_or_default();
                return Err(RendererError::ConflictingHgAttribute { id });
            }
        }
        let mut left = i64::from(first.metadata.offset[0]);
        let mut top = i64::from(first.metadata.offset[1]);
        let mut right = left + i64::from(first.width);
        let mut bottom = top + i64::from(first.height);
        let mut canvas_size = first.canvas_dimensions();
        for layer in &layers[1..] {
            let layer_left = i64::from(layer.metadata.offset[0]);
            let layer_top = i64::from(layer.metadata.offset[1]);
            left = left.min(layer_left);
            top = top.min(layer_top);
            right = right.max(layer_left + i64::from(layer.width));
            bottom = bottom.max(layer_top + i64::from(layer.height));
            canvas_size.0 = canvas_size.0.max(layer.canvas_dimensions().0);
            canvas_size.1 = canvas_size.1.max(layer.canvas_dimensions().1);
        }
        let width =
            u32::try_from(right - left).map_err(|_| RendererError::ScaledDimensionsOverflow)?;
        let height =
            u32::try_from(bottom - top).map_err(|_| RendererError::ScaledDimensionsOverflow)?;
        if width == 0 || height == 0 {
            return Ok(None);
        }
        let mut canvas = Canvas::new(width, height)?;
        for layer in layers {
            let x = i64::from(layer.metadata.offset[0]) - left + i64::from(first.metadata.base[0])
                - i64::from(layer.metadata.base[0]);
            let y = i64::from(layer.metadata.offset[1]) - top + i64::from(first.metadata.base[1])
                - i64::from(layer.metadata.base[1]);
            let mut options = ImageDrawOptions::new(Point::new(
                i32::try_from(x).map_err(|_| RendererError::ScaledDimensionsOverflow)?,
                i32::try_from(y).map_err(|_| RendererError::ScaledDimensionsOverflow)?,
            ));
            options.clip = Some(Rect::new(0, 0, width, height));
            let pixel_layer = Self::from_rgba8(layer.width, layer.height, layer.pixels().to_vec())?;
            canvas.draw_frame(&pixel_layer, &options)?;
        }
        let metadata = FrameMetadata {
            container: HgContainer::None,
            frame_id: None,
            bit_depth: 32,
            offset: [
                i32::try_from(left).map_err(|_| RendererError::ScaledDimensionsOverflow)?,
                i32::try_from(top).map_err(|_| RendererError::ScaledDimensionsOverflow)?,
            ],
            canvas_size: canvas_size.into(),
            base: first.metadata.base,
            alpha_flag: u32::from(layers.iter().any(|layer| {
                layer.metadata.alpha_flag != 0
                    || layer.pixels.chunks_exact(4).any(|pixel| pixel[3] != 255)
            })),
        };
        Self::from_rgba8_with_metadata(width, height, canvas.pixels, metadata)
            .map(|frame| Some(frame.with_hg_attributes(attributes)))
    }

    pub fn crop(&self, rect: Rect) -> Result<Self> {
        let right = rect.right();
        let bottom = rect.bottom();
        if rect.width == 0
            || rect.height == 0
            || rect.x < 0
            || rect.y < 0
            || right > i64::from(self.width)
            || bottom > i64::from(self.height)
        {
            return Err(RendererError::InvalidRegion {
                rect,
                width: self.width,
                height: self.height,
            });
        }
        let width = rect.width as usize;
        let height = rect.height as usize;
        let mut pixels = Vec::new();
        let bytes = width
            .checked_mul(height)
            .and_then(|length| length.checked_mul(4))
            .ok_or(RendererError::ScaledDimensionsOverflow)?;
        pixels
            .try_reserve_exact(bytes)
            .map_err(|_| RendererError::AllocationFailed { bytes })?;
        for y in rect.y as usize..bottom as usize {
            let start = (y * self.width as usize + rect.x as usize) * 4;
            pixels.extend_from_slice(&self.pixels[start..start + width * 4]);
        }
        let mut metadata = self.metadata;
        metadata.offset[0] = metadata.offset[0].wrapping_add(rect.x);
        metadata.offset[1] = metadata.offset[1].wrapping_add(rect.y);
        self.rebuild(rect.width, rect.height, pixels, metadata)
    }

    /// Mirrors the decoded pixels and updates the HG crop/base geometry in
    /// the same canvas coordinate system as the source image.
    pub fn flip_horizontal(&self) -> Result<Self> {
        let mut pixels = vec![0; self.pixels.len()];
        for y in 0..self.height {
            for x in 0..self.width {
                let source = ((y as usize) * self.width as usize + x as usize) * 4;
                let target =
                    ((y as usize) * self.width as usize + (self.width - 1 - x) as usize) * 4;
                pixels[target..target + 4].copy_from_slice(&self.pixels[source..source + 4]);
            }
        }
        let mut metadata = self.metadata;
        let canvas_width = self.canvas_dimensions().0 as i32;
        metadata.offset[0] = canvas_width
            .wrapping_sub(metadata.offset[0])
            .wrapping_sub(self.width as i32);
        metadata.base[0] = canvas_width.wrapping_sub(metadata.base[0]);
        self.rebuild(self.width, self.height, pixels, metadata)
    }

    /// Mirrors the decoded pixels vertically and updates the HG geometry.
    pub fn flip_vertical(&self) -> Result<Self> {
        let mut pixels = vec![0; self.pixels.len()];
        for y in 0..self.height {
            let source = (y as usize) * self.width as usize * 4;
            let target = ((self.height - 1 - y) as usize) * self.width as usize * 4;
            pixels[target..target + self.width as usize * 4]
                .copy_from_slice(&self.pixels[source..source + self.width as usize * 4]);
        }
        let mut metadata = self.metadata;
        let canvas_height = self.canvas_dimensions().1 as i32;
        metadata.offset[1] = canvas_height
            .wrapping_sub(metadata.offset[1])
            .wrapping_sub(self.height as i32);
        metadata.base[1] = canvas_height.wrapping_sub(metadata.base[1]);
        self.rebuild(self.width, self.height, pixels, metadata)
    }

    /// Rotates the decoded image clockwise by a right angle.
    ///
    /// CatSystem2 accepts only 0, 90, 180 and 270 degree transforms at this
    /// layer. Unsupported angles are reported as `Ok(false)` so callers can
    /// preserve the original image without manufacturing a raster.
    pub fn rotate_right_angle(&self, degrees: i32) -> Result<Option<Self>> {
        if degrees == 0 {
            return Ok(Some(self.clone()));
        }
        if !matches!(degrees, 90 | 180 | 270) {
            return Ok(None);
        }
        let [width, height] = [self.width, self.height];
        let [new_width, new_height] = if degrees == 180 {
            [width, height]
        } else {
            [height, width]
        };
        let mut pixels = if self.is_empty() { Vec::new() } else {
            vec![0; rgba_len(new_width, new_height)?]
        };
        for y in 0..height {
            for x in 0..width {
                let [target_x, target_y] = match degrees {
                    90 => [height - 1 - y, x],
                    180 => [width - 1 - x, height - 1 - y],
                    270 => [y, width - 1 - x],
                    _ => unreachable!(),
                };
                let source = ((y as usize) * width as usize + x as usize) * 4;
                let target = ((target_y as usize) * new_width as usize + target_x as usize) * 4;
                pixels[target..target + 4].copy_from_slice(&self.pixels[source..source + 4]);
            }
        }

        let canvas = self.canvas_dimensions();
        let rotate_point = |point: [i32; 2], extent: [u32; 2]| -> [i32; 2] {
            let x = i64::from(point[0]);
            let y = i64::from(point[1]);
            let w = i64::from(extent[0]);
            let h = i64::from(extent[1]);
            let canvas_width = i64::from(canvas.0);
            let canvas_height = i64::from(canvas.1);
            let output = match degrees {
                90 => [y, canvas_width - x - w],
                180 => [canvas_width - x - w, canvas_height - y - h],
                270 => [canvas_height - y - h, x],
                _ => unreachable!(),
            };
            [output[0] as i32, output[1] as i32]
        };

        let mut metadata = self.metadata;
        metadata.offset = rotate_point(metadata.offset, [width, height]);
        metadata.base = rotate_point(metadata.base, [0, 0]);
        if degrees != 180 {
            metadata.canvas_size.swap(0, 1);
        }
        self.rebuild(new_width, new_height, pixels, metadata)
            .map(Some)
    }

    pub fn pixelated(&self, block_size: u32, algorithm: MosaicAlgorithm) -> Result<Self> {
        if block_size == 0 {
            return Err(RendererError::InvalidEffectParameter {
                name: "mosaic block size",
            });
        }
        if block_size == 1 {
            return Ok(self.clone());
        }
        let mut pixels = vec![0; self.pixels.len()];
        for y in 0..self.height {
            for x in 0..self.width {
                let sample_x = x
                    .saturating_div(block_size)
                    .saturating_mul(block_size)
                    .saturating_add(block_size / 2)
                    .min(self.width - 1);
                let sample_y = y
                    .saturating_div(block_size)
                    .saturating_mul(block_size)
                    .saturating_add(block_size / 2)
                    .min(self.height - 1);
                let source = self.pixel_unchecked(sample_x, sample_y);
                let target = ((y as usize) * self.width as usize + x as usize) * 4;
                pixels[target..target + 4]
                    .copy_from_slice(&[source.r, source.g, source.b, source.a]);
                if algorithm == MosaicAlgorithm::OpaqueBlocks {
                    pixels[target + 3] = 255;
                }
            }
        }
        self.rebuild(self.width, self.height, pixels, self.metadata)
    }

    pub fn adjust_rgb_percent(&self, rgb: [u32; 3]) -> Result<Self> {
        let mut pixels = self.pixels().to_vec();
        if rgb != [100; 3] {
            for pixel in pixels.chunks_exact_mut(4) {
                for (value, percent) in pixel[..3].iter_mut().zip(rgb) {
                    *value = (u32::from(*value).wrapping_mul(percent) / 100).min(255) as u8;
                }
            }
        }
        self.rebuild(self.width, self.height, pixels, self.metadata)
    }

    pub fn apply_filter(&self, filter: FrameFilter) -> Result<Self> {
        let mut pixels = self.pixels().to_vec();
        for pixel in pixels.chunks_exact_mut(4) {
            match filter {
                FrameFilter::None => (),
                FrameFilter::Negative => {
                    pixel[..3].iter_mut().for_each(|value| *value = !*value);
                }
                FrameFilter::Mono | FrameFilter::Sepia => {
                    let gray = (f32::from(pixel[0]) * 0.298_912) as u8
                        + (f32::from(pixel[1]) * 0.586_611) as u8
                        + (f32::from(pixel[2]) * 0.114_478) as u8;
                    if filter == FrameFilter::Mono {
                        pixel[..3].fill(gray);
                    } else {
                        pixel[0] = gray.saturating_add(30);
                        pixel[1] = gray.saturating_add(5);
                        pixel[2] = gray.saturating_sub(35);
                    }
                }
            }
        }
        self.rebuild(self.width, self.height, pixels, self.metadata)
    }

    pub fn resize_canvas(&self, canvas: [u32; 2], filter: ScaleFilter) -> Result<Self> {
        if canvas.contains(&0) {
            return Err(RendererError::InvalidDimensions {
                width: canvas[0],
                height: canvas[1],
            });
        }
        let old_canvas = self.canvas_dimensions();
        if old_canvas.0 == 0 || old_canvas.1 == 0 {
            return Err(RendererError::InvalidEffectParameter {
                name: "source canvas dimensions",
            });
        }
        let ratio_x = f64::from(canvas[0]) / f64::from(old_canvas.0);
        let ratio_y = f64::from(canvas[1]) / f64::from(old_canvas.1);
        let width = (f64::from(self.width) * ratio_x + 0.5).floor();
        let height = (f64::from(self.height) * ratio_y + 0.5).floor();
        if width < 1.0
            || height < 1.0
            || width > f64::from(u32::MAX)
            || height > f64::from(u32::MAX)
        {
            return Err(RendererError::ScaledDimensionsOverflow);
        }
        let width = width as u32;
        let height = height as u32;
        let mut pixels = vec![0; rgba_len(width, height)?];
        for y in 0..height {
            for x in 0..width {
                let source_x = f64::from(x) / ratio_x;
                let source_y = f64::from(y) / ratio_y;
                let color = sample_frame(self, source_x, source_y, filter);
                let at = (y as usize * width as usize + x as usize) * 4;
                pixels[at..at + 4].copy_from_slice(&[color.r, color.g, color.b, color.a]);
            }
        }
        let scale_i32 = |value: i32, ratio: f64| -> Result<i32> {
            let scaled = (f64::from(value) * ratio + 0.5).floor();
            if !scaled.is_finite() || scaled < f64::from(i32::MIN) || scaled > f64::from(i32::MAX) {
                return Err(RendererError::ScaledDimensionsOverflow);
            }
            Ok(scaled as i32)
        };
        let mut metadata = self.metadata;
        metadata.canvas_size = canvas;
        metadata.offset = [
            scale_i32(metadata.offset[0], ratio_x)?,
            scale_i32(metadata.offset[1], ratio_y)?,
        ];
        metadata.base = [
            scale_i32(metadata.base[0], ratio_x)?,
            scale_i32(metadata.base[1], ratio_y)?,
        ];
        self.rebuild(width, height, pixels, metadata)
    }

    pub fn encode_hg2(&self) -> Result<Vec<u8>> {
        encode_hg_container(self, HgContainer::Hg2V25)
    }

    pub fn encode_hg2_v20(&self) -> Result<Vec<u8>> {
        encode_hg_container(self, HgContainer::Hg2V20)
    }

    pub fn encode_hg3(&self) -> Result<Vec<u8>> {
        encode_hg_container(self, HgContainer::Hg3V300)
    }

    pub fn pixel(&self, x: i32, y: i32) -> Result<Color> {
        if x < 0 || y < 0 || (x as u32) >= self.width || (y as u32) >= self.height {
            return Err(RendererError::CoordinateOutOfBounds {
                x: i64::from(x),
                y: i64::from(y),
                width: self.width,
                height: self.height,
            });
        }
        Ok(self.pixel_unchecked(x as u32, y as u32))
    }

    fn pixel_unchecked(&self, x: u32, y: u32) -> Color {
        let index = ((y as usize) * (self.width as usize) + (x as usize)) * 4;
        Color::rgba(
            self.pixels[index],
            self.pixels[index + 1],
            self.pixels[index + 2],
            self.pixels[index + 3],
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RgbaFrameSet {
    frames: Vec<RgbaFrame>,
}

impl RgbaFrameSet {
    pub fn new(frames: Vec<RgbaFrame>) -> Result<Self> {
        if frames.is_empty() {
            return Err(RendererError::InvalidHgStructure {
                message: "HG frame set contains no frames".into(),
            });
        }
        Ok(Self { frames })
    }

    pub fn frames(&self) -> &[RgbaFrame] {
        &self.frames
    }

    pub fn frame(&self, index: usize) -> Option<&RgbaFrame> {
        self.frames.get(index)
    }

    pub fn frame_by_id(&self, id: u32) -> Option<&RgbaFrame> {
        self.frames
            .iter()
            .find(|frame| frame.metadata.frame_id == Some(id))
    }

    pub fn select(&self, selection: AnimationFrameSelection) -> Result<&RgbaFrame> {
        match selection {
            AnimationFrameSelection::Index(index) => {
                self.frame(index)
                    .ok_or(RendererError::FrameIndexOutOfBounds {
                        index,
                        count: self.frames.len(),
                    })
            }
            AnimationFrameSelection::Id(id) => {
                let mut matches = self
                    .frames
                    .iter()
                    .filter(|frame| frame.metadata.frame_id == Some(id));
                let first = matches
                    .next()
                    .ok_or(RendererError::FrameIdNotFound { id })?;
                if matches.next().is_some() {
                    return Err(RendererError::AmbiguousFrameId { id });
                }
                Ok(first)
            }
        }
    }

    /// Resolves the frame ID emitted by the ANM interpreter. Negative IDs are
    /// the hidden/no-frame state and therefore produce `Ok(None)`.
    pub fn select_animation(&self, state: AnimationDisplayState) -> Result<Option<&RgbaFrame>> {
        if state.frame_id < 0 {
            return Ok(None);
        }
        let id = u32::try_from(state.frame_id).map_err(|_| RendererError::FrameIdNotFound {
            id: state.frame_id as u32,
        })?;
        self.select(AnimationFrameSelection::Id(id)).map(Some)
    }

    pub fn has_alpha(&self) -> bool {
        self.frames.iter().any(RgbaFrame::has_alpha)
    }

    pub fn encode_hg2(&self) -> Result<Vec<u8>> {
        self.encode_hg2_container(HgContainer::Hg2V25)
    }

    pub fn encode_hg2_v20(&self) -> Result<Vec<u8>> {
        self.encode_hg2_container(HgContainer::Hg2V20)
    }

    fn encode_hg2_container(&self, container: HgContainer) -> Result<Vec<u8>> {
        let mut encoded = Vec::new();
        let mut header = None;
        for frame in &self.frames {
            let bytes = match container {
                HgContainer::Hg2V20 => frame.encode_hg2_v20()?,
                HgContainer::Hg2V25 => frame.encode_hg2()?,
                _ => {
                    return Err(RendererError::InvalidEffectParameter {
                        name: "HG-2 frame set container",
                    });
                }
            };
            if header.is_none() {
                header = Some(bytes[..12].to_vec());
            }
            let mut part = bytes[12..].to_vec();
            let next =
                u32::try_from(part.len()).map_err(|_| RendererError::ScaledDimensionsOverflow)?;
            part[64..68].copy_from_slice(&next.to_le_bytes());
            encoded.push(part);
        }
        let mut output = header.unwrap();
        for (index, mut part) in encoded.into_iter().enumerate() {
            if index + 1 == self.frames.len() {
                part[64..68].copy_from_slice(&0_u32.to_le_bytes());
            }
            output.extend_from_slice(&part);
        }
        Ok(output)
    }

    pub fn encode_hg3(&self) -> Result<Vec<u8>> {
        let mut output = b"HG-3".to_vec();
        output.extend_from_slice(&12_u32.to_le_bytes());
        output.extend_from_slice(&0x300_u32.to_le_bytes());
        for (index, frame) in self.frames.iter().enumerate() {
            let bytes = frame.encode_hg3()?;
            let mut part = bytes[12..].to_vec();
            if index + 1 == self.frames.len() {
                part[..4].copy_from_slice(&0_u32.to_le_bytes());
            } else {
                let next = u32::try_from(part.len())
                    .map_err(|_| RendererError::ScaledDimensionsOverflow)?;
                part[..4].copy_from_slice(&next.to_le_bytes());
            }
            output.extend_from_slice(&part);
        }
        Ok(output)
    }
}

#[derive(Debug, Clone)]
struct FrameResource {
    frame: Arc<RgbaFrame>,
    references: usize,
}

#[derive(Debug, Default)]
struct LeaseState {
    pending: Mutex<Vec<u32>>,
}

impl LeaseState {
    fn enqueue(&self, handle: u32) {
        match self.pending.lock() {
            Ok(mut pending) => pending.push(handle),
            Err(poisoned) => poisoned.into_inner().push(handle),
        }
    }

    fn take_pending(&self) -> Vec<u32> {
        match self.pending.lock() {
            Ok(mut pending) => std::mem::take(&mut *pending),
            Err(poisoned) => std::mem::take(&mut *poisoned.into_inner()),
        }
    }
}

/// A stable decoded-frame snapshot with one retained pool reference.
///
/// Dropping the lease queues its retained reference for automatic collection
/// by the originating pool. `release` remains available for deterministic
/// collection at a known point. A replacement can update the handle while
/// this lease continues to expose the frame version it acquired.
#[derive(Debug)]
pub struct FrameResourceLease {
    handle: u32,
    frame: Arc<RgbaFrame>,
    state: Weak<LeaseState>,
    released: bool,
}

impl FrameResourceLease {
    pub fn handle(&self) -> u32 {
        self.handle
    }

    pub fn frame(&self) -> &RgbaFrame {
        &self.frame
    }

    pub fn release(mut self, resources: &mut FrameResources) -> Result<bool> {
        let state = self
            .state
            .upgrade()
            .ok_or(RendererError::ResourceLeasePoolMismatch {
                handle: self.handle,
            })?;
        if !Arc::ptr_eq(&state, &resources.lease_state) {
            return Err(RendererError::ResourceLeasePoolMismatch {
                handle: self.handle,
            });
        }
        self.released = true;
        resources.release(self.handle)
    }
}

impl Deref for FrameResourceLease {
    type Target = RgbaFrame;

    fn deref(&self) -> &Self::Target {
        self.frame()
    }
}

impl Drop for FrameResourceLease {
    fn drop(&mut self) {
        if !self.released {
            if let Some(state) = self.state.upgrade() {
                state.enqueue(self.handle);
            }
        }
    }
}

/// Handles allocated for the frames of one ANM/HG animation resource.
///
/// The handles are borrowed from `FrameResources`; releasing this collection
/// releases the initial pool reference for every frame. A selected frame can
/// then be acquired as an ordinary RAII lease.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnimationResourceHandles {
    handles: Vec<u32>,
    frame_ids: Vec<Option<u32>>,
}

impl AnimationResourceHandles {
    pub fn handles(&self) -> &[u32] {
        &self.handles
    }

    pub fn len(&self) -> usize {
        self.handles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.handles.is_empty()
    }

    pub fn handle(&self, selection: AnimationFrameSelection) -> Result<u32> {
        match selection {
            AnimationFrameSelection::Index(index) => {
                self.handles
                    .get(index)
                    .copied()
                    .ok_or(RendererError::FrameIndexOutOfBounds {
                        index,
                        count: self.handles.len(),
                    })
            }
            AnimationFrameSelection::Id(id) => {
                let mut matches = self
                    .frame_ids
                    .iter()
                    .enumerate()
                    .filter(|(_, frame_id)| **frame_id == Some(id));
                let first = matches
                    .next()
                    .map(|(index, _)| self.handles[index])
                    .ok_or(RendererError::FrameIdNotFound { id })?;
                if matches.next().is_some() {
                    return Err(RendererError::AmbiguousFrameId { id });
                }
                Ok(first)
            }
        }
    }

    pub fn acquire(
        &self,
        resources: &mut FrameResources,
        selection: AnimationFrameSelection,
    ) -> Result<FrameResourceLease> {
        resources.acquire(self.handle(selection)?)
    }

    pub fn release(self, resources: &mut FrameResources) -> Result<()> {
        resources.release_all(self.handles)
    }
}

/// CPU-side decoded frame ownership shared by layers, transitions and
/// temporary captures. A released resource is removed immediately when its
/// last reference is gone.
#[derive(Debug)]
pub struct FrameResources {
    next_handle: u32,
    entries: BTreeMap<u32, FrameResource>,
    lease_state: Arc<LeaseState>,
}

impl Clone for FrameResources {
    fn clone(&self) -> Self {
        Self {
            next_handle: self.next_handle,
            entries: self.entries.clone(),
            lease_state: Arc::new(LeaseState::default()),
        }
    }
}

impl Default for FrameResources {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameResources {
    pub fn new() -> Self {
        Self {
            next_handle: 1,
            entries: BTreeMap::new(),
            lease_state: Arc::new(LeaseState::default()),
        }
    }

    fn collect_dropped(&mut self) {
        for handle in self.lease_state.take_pending() {
            let _ = self.release_inner(handle);
        }
    }

    /// Collects references returned by leases that were dropped without an
    /// explicit `release` call. It is also invoked by every mutating pool
    /// operation.
    pub fn sweep(&mut self) {
        self.collect_dropped();
    }

    pub fn insert(&mut self, frame: RgbaFrame) -> Result<u32> {
        self.collect_dropped();
        let handle = self.next_handle;
        self.next_handle = self
            .next_handle
            .checked_add(1)
            .ok_or(RendererError::ResourceHandleExhausted)?;
        self.entries.insert(
            handle,
            FrameResource {
                frame: Arc::new(frame),
                references: 1,
            },
        );
        Ok(handle)
    }

    pub fn insert_set(&mut self, set: &RgbaFrameSet) -> Result<Vec<u32>> {
        self.collect_dropped();
        let mut handles = Vec::with_capacity(set.frames.len());
        for frame in set.frames() {
            match self.insert(frame.clone()) {
                Ok(handle) => handles.push(handle),
                Err(error) => {
                    for handle in handles {
                        let _ = self.release(handle);
                    }
                    return Err(error);
                }
            }
        }
        Ok(handles)
    }

    pub fn contains(&self, handle: u32) -> bool {
        self.entries.contains_key(&handle)
    }

    pub fn get(&self, handle: u32) -> Result<&RgbaFrame> {
        self.entries
            .get(&handle)
            .map(|entry| entry.frame.as_ref())
            .ok_or(RendererError::InvalidResourceHandle { handle })
    }

    pub fn get_mut(&mut self, handle: u32) -> Result<&mut RgbaFrame> {
        self.collect_dropped();
        self.entries
            .get_mut(&handle)
            .map(|entry| Arc::make_mut(&mut entry.frame))
            .ok_or(RendererError::InvalidResourceHandle { handle })
    }

    pub fn replace(&mut self, handle: u32, frame: RgbaFrame) -> Result<()> {
        // Replacement adopts the incoming frame's metadata and attributes.
        // Existing leases keep their old Arc snapshot.
        self.collect_dropped();
        let entry = self
            .entries
            .get_mut(&handle)
            .ok_or(RendererError::InvalidResourceHandle { handle })?;
        entry.frame = Arc::new(frame);
        Ok(())
    }

    pub fn replace_set(&mut self, handles: &[u32], set: &RgbaFrameSet) -> Result<()> {
        self.collect_dropped();
        if handles.len() != set.frames.len() {
            return Err(RendererError::InvalidEffectParameter {
                name: "frame resource replacement count",
            });
        }
        for (index, &handle) in handles.iter().enumerate() {
            if !self.entries.contains_key(&handle) {
                return Err(RendererError::InvalidResourceHandle { handle });
            }
            if handles[..index].contains(&handle) {
                return Err(RendererError::DuplicateResourceHandle { handle });
            }
        }
        for (&handle, frame) in handles.iter().zip(set.frames()) {
            self.entries.get_mut(&handle).unwrap().frame = Arc::new(frame.clone());
        }
        Ok(())
    }

    pub fn references(&self, handle: u32) -> Result<usize> {
        self.entries
            .get(&handle)
            .map(|entry| entry.references)
            .ok_or(RendererError::InvalidResourceHandle { handle })
    }

    pub fn retain(&mut self, handle: u32) -> Result<()> {
        self.collect_dropped();
        let entry = self
            .entries
            .get_mut(&handle)
            .ok_or(RendererError::InvalidResourceHandle { handle })?;
        entry.references = entry
            .references
            .checked_add(1)
            .ok_or(RendererError::ResourceReferenceOverflow { handle })?;
        Ok(())
    }

    pub fn acquire(&mut self, handle: u32) -> Result<FrameResourceLease> {
        self.collect_dropped();
        self.retain(handle)?;
        let frame = Arc::clone(
            &self
                .entries
                .get(&handle)
                .ok_or(RendererError::InvalidResourceHandle { handle })?
                .frame,
        );
        Ok(FrameResourceLease {
            handle,
            frame,
            state: Arc::downgrade(&self.lease_state),
            released: false,
        })
    }

    pub fn with_frame<T>(
        &mut self,
        handle: u32,
        callback: impl FnOnce(&RgbaFrame) -> Result<T>,
    ) -> Result<T> {
        let lease = self.acquire(handle)?;
        let result = callback(lease.frame());
        let released = lease.release(self);
        match (result, released) {
            (Ok(value), Ok(_)) => Ok(value),
            (Err(error), Ok(_)) => Err(error),
            (Ok(_), Err(error)) => Err(error),
            (Err(error), Err(_)) => Err(error),
        }
    }

    pub fn release(&mut self, handle: u32) -> Result<bool> {
        self.collect_dropped();
        self.release_inner(handle)
    }

    fn release_inner(&mut self, handle: u32) -> Result<bool> {
        let entry = self
            .entries
            .get_mut(&handle)
            .ok_or(RendererError::InvalidResourceHandle { handle })?;
        if entry.references == 0 {
            return Err(RendererError::ResourceReferenceUnderflow { handle });
        }
        entry.references -= 1;
        if entry.references == 0 {
            self.entries.remove(&handle);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub fn clear(&mut self) {
        self.collect_dropped();
        self.entries.clear();
    }

    pub fn release_all<I>(&mut self, handles: I) -> Result<()>
    where
        I: IntoIterator<Item = u32>,
    {
        self.collect_dropped();
        for handle in handles {
            self.release_inner(handle)?;
        }
        Ok(())
    }

    pub fn insert_animation(&mut self, set: &RgbaFrameSet) -> Result<AnimationResourceHandles> {
        let handles = self.insert_set(set)?;
        Ok(AnimationResourceHandles {
            handles,
            frame_ids: set
                .frames()
                .iter()
                .map(|frame| frame.metadata.frame_id)
                .collect(),
        })
    }

    pub fn acquire_animation(
        &mut self,
        animation: &AnimationResourceHandles,
        selection: AnimationFrameSelection,
    ) -> Result<FrameResourceLease> {
        animation.acquire(self, selection)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlphaMask {
    width: u32,
    height: u32,
    alpha: Vec<u8>,
}

impl AlphaMask {
    pub fn new(width: u32, height: u32, alpha: Vec<u8>) -> Result<Self> {
        let expected = alpha_len(width, height)?;
        if alpha.len() != expected {
            return Err(RendererError::BufferLength {
                expected,
                actual: alpha.len(),
            });
        }
        Ok(Self {
            width,
            height,
            alpha,
        })
    }

    pub fn solid(width: u32, height: u32, alpha: u8) -> Result<Self> {
        let count = alpha_len(width, height)?;
        let mut alpha_data = Vec::new();
        alpha_data
            .try_reserve_exact(count)
            .map_err(|_| RendererError::AllocationFailed { bytes: count })?;
        alpha_data.resize(count, alpha);
        Ok(Self {
            width,
            height,
            alpha: alpha_data,
        })
    }

    pub fn from_frame(frame: &RgbaFrame, channel: MaskChannel) -> Result<Self> {
        let alpha = frame
            .pixels
            .chunks_exact(4)
            .map(|pixel| match channel {
                MaskChannel::Red => pixel[0],
                MaskChannel::Green => pixel[1],
                MaskChannel::Blue => pixel[2],
                MaskChannel::Alpha => pixel[3],
            })
            .collect();
        Self::new(frame.width, frame.height, alpha)
    }

    pub fn multiply(&self, other: &Self) -> Result<Self> {
        if self.width != other.width || self.height != other.height {
            return Err(RendererError::MaskDimensions {
                mask_width: other.width,
                mask_height: other.height,
                canvas_width: self.width,
                canvas_height: self.height,
            });
        }
        let alpha = self
            .alpha
            .iter()
            .zip(&other.alpha)
            .map(|(&left, &right)| modulate_alpha(left, right))
            .collect();
        Self::new(self.width, self.height, alpha)
    }

    pub fn lerp(&self, other: &Self, progress: u16) -> Result<Self> {
        if self.width != other.width || self.height != other.height {
            return Err(RendererError::MaskDimensions {
                mask_width: other.width,
                mask_height: other.height,
                canvas_width: self.width,
                canvas_height: self.height,
            });
        }
        let weight = u32::from(progress.min(255));
        let alpha = self
            .alpha
            .iter()
            .zip(&other.alpha)
            .map(|(&left, &right)| {
                ((u32::from(left) * (255 - weight) + u32::from(right) * weight + 127) / 255) as u8
            })
            .collect();
        Self::new(self.width, self.height, alpha)
    }

    pub fn threshold(&self, threshold: u8) -> Self {
        Self {
            width: self.width,
            height: self.height,
            alpha: self
                .alpha
                .iter()
                .map(|&value| if value >= threshold { 255 } else { 0 })
                .collect(),
        }
    }

    pub fn inverted(&self) -> Self {
        Self {
            width: self.width,
            height: self.height,
            alpha: self.alpha.iter().map(|&value| 255 - value).collect(),
        }
    }

    pub fn crop(&self, rect: Rect) -> Result<Self> {
        let right = rect.right();
        let bottom = rect.bottom();
        if rect.width == 0
            || rect.height == 0
            || rect.x < 0
            || rect.y < 0
            || right > i64::from(self.width)
            || bottom > i64::from(self.height)
        {
            return Err(RendererError::InvalidRegion {
                rect,
                width: self.width,
                height: self.height,
            });
        }
        let mut alpha = Vec::with_capacity(alpha_len(rect.width, rect.height)?);
        for y in rect.y as u32..bottom as u32 {
            let start = (y as usize * self.width as usize) + rect.x as usize;
            alpha.extend_from_slice(&self.alpha[start..start + rect.width as usize]);
        }
        Self::new(rect.width, rect.height, alpha)
    }

    pub fn flip_horizontal(&self) -> Result<Self> {
        let mut alpha = vec![0; self.alpha.len()];
        for y in 0..self.height {
            for x in 0..self.width {
                alpha[y as usize * self.width as usize + (self.width - 1 - x) as usize] =
                    self.alpha[y as usize * self.width as usize + x as usize];
            }
        }
        Self::new(self.width, self.height, alpha)
    }

    pub fn flip_vertical(&self) -> Result<Self> {
        let row = self.width as usize;
        let mut alpha = vec![0; self.alpha.len()];
        for y in 0..self.height as usize {
            let source = y * row;
            let target = (self.height as usize - 1 - y) * row;
            alpha[target..target + row].copy_from_slice(&self.alpha[source..source + row]);
        }
        Self::new(self.width, self.height, alpha)
    }

    pub fn rotate_right_angle(&self, degrees: i32) -> Result<Option<Self>> {
        if degrees == 0 {
            return Ok(Some(self.clone()));
        }
        if !matches!(degrees, 90 | 180 | 270) {
            return Ok(None);
        }
        let [new_width, new_height] = if degrees == 180 {
            [self.width, self.height]
        } else {
            [self.height, self.width]
        };
        let mut alpha = vec![0; alpha_len(new_width, new_height)?];
        for y in 0..self.height {
            for x in 0..self.width {
                let [target_x, target_y] = match degrees {
                    90 => [self.height - 1 - y, x],
                    180 => [self.width - 1 - x, self.height - 1 - y],
                    270 => [y, self.width - 1 - x],
                    _ => unreachable!(),
                };
                alpha[target_y as usize * new_width as usize + target_x as usize] =
                    self.alpha[y as usize * self.width as usize + x as usize];
            }
        }
        Self::new(new_width, new_height, alpha).map(Some)
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn alpha(&self) -> &[u8] {
        &self.alpha
    }

    pub fn at(&self, x: i32, y: i32) -> Result<u8> {
        if x < 0 || y < 0 || (x as u32) >= self.width || (y as u32) >= self.height {
            return Err(RendererError::CoordinateOutOfBounds {
                x: i64::from(x),
                y: i64::from(y),
                width: self.width,
                height: self.height,
            });
        }
        Ok(self.alpha_unchecked(x as u32, y as u32))
    }

    fn alpha_unchecked(&self, x: u32, y: u32) -> u8 {
        self.alpha[(y as usize) * (self.width as usize) + (x as usize)]
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum MaskChannel {
    Red,
    Green,
    Blue,
    Alpha,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlyphMask {
    width: u32,
    height: u32,
    alpha: Arc<Vec<u8>>,
    metrics: GlyphMetrics,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Default)]
pub struct GlyphMetrics {
    pub bearing_x: i32,
    pub bearing_y: i32,
    pub advance: i32,
}

impl GlyphMask {
    pub fn from_alpha(width: u32, height: u32, alpha: Vec<u8>) -> Result<Self> {
        Self::from_alpha_with_metrics(
            width,
            height,
            alpha,
            GlyphMetrics {
                advance: i32::try_from(width).unwrap_or(i32::MAX),
                ..GlyphMetrics::default()
            },
        )
    }

    pub fn from_alpha_with_metrics(
        width: u32,
        height: u32,
        alpha: Vec<u8>,
        metrics: GlyphMetrics,
    ) -> Result<Self> {
        let expected = glyph_len(width, height)?;
        if alpha.len() != expected {
            return Err(RendererError::BufferLength {
                expected,
                actual: alpha.len(),
            });
        }
        Ok(Self {
            width,
            height,
            alpha: Arc::new(alpha),
            metrics,
        })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn alpha(&self) -> &[u8] {
        &self.alpha
    }

    pub fn metrics(&self) -> GlyphMetrics {
        self.metrics
    }

    pub fn advance(&self) -> i32 {
        self.metrics.advance
    }

    fn alpha_unchecked(&self, x: u32, y: u32) -> u8 {
        self.alpha[(y as usize) * (self.width as usize) + (x as usize)]
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PositionedGlyph {
    pub glyph: GlyphMask,
    pub baseline: Point,
}

impl PositionedGlyph {
    pub const fn new(glyph: GlyphMask, baseline: Point) -> Self {
        Self { glyph, baseline }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GlyphRun {
    glyphs: Vec<PositionedGlyph>,
    advance: Point,
}

impl GlyphRun {
    pub fn layout(
        text: &str,
        origin: Point,
        line_height: i32,
        letter_spacing: i32,
        mut rasterize: impl FnMut(char) -> Result<GlyphMask>,
        mut kerning: impl FnMut(char, char) -> i32,
    ) -> Result<Self> {
        let mut run = Self::default();
        let mut pen_x = origin.x;
        let mut baseline_y = origin.y;
        let mut previous = None;
        let mut max_advance = 0;
        let mut line_count = 1_i32;

        for character in text.chars() {
            match character {
                '\r' => continue,
                '\n' => {
                    max_advance = max_advance.max(pen_x.saturating_sub(origin.x));
                    pen_x = origin.x;
                    baseline_y = baseline_y.saturating_add(line_height);
                    line_count = line_count.saturating_add(1);
                    previous = None;
                    continue;
                }
                _ => {}
            }
            if let Some(previous) = previous {
                pen_x = pen_x.saturating_add(kerning(previous, character));
            }
            let glyph = rasterize(character)?;
            let advance = glyph.advance();
            run.glyphs
                .push(PositionedGlyph::new(glyph, Point::new(pen_x, baseline_y)));
            pen_x = pen_x.saturating_add(advance).saturating_add(letter_spacing);
            previous = Some(character);
        }
        max_advance = max_advance.max(pen_x.saturating_sub(origin.x));
        run.advance = Point::new(max_advance, line_height.saturating_mul(line_count));
        Ok(run)
    }

    pub fn glyphs(&self) -> &[PositionedGlyph] {
        &self.glyphs
    }

    pub fn advance(&self) -> Point {
        self.advance
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct GlyphEffects {
    pub outline: bool,
    pub shadow: bool,
    pub shadow_offset: [i32; 2],
}

impl Default for GlyphEffects {
    fn default() -> Self {
        Self {
            outline: false,
            shadow: false,
            shadow_offset: [1, 1],
        }
    }
}

impl Canvas {
    pub fn draw_styled_glyph(
        &mut self,
        glyph: &GlyphMask,
        baseline: Point,
        options: &GlyphDrawOptions,
    ) -> Result<()> {
        validate_mask(options.mask.as_ref(), self)?;
        let position = Point::new(
            baseline.x.saturating_add(glyph.metrics.bearing_x),
            baseline.y.saturating_sub(glyph.metrics.bearing_y),
        );

        if options.effects.outline {
            self.draw_glyph_outline(glyph, position, options)?;
        }
        if options.effects.shadow {
            let shadow_position = Point::new(
                position.x.saturating_add(options.effects.shadow_offset[0]),
                position.y.saturating_add(options.effects.shadow_offset[1]),
            );
            self.draw_glyph_mask_color(
                glyph,
                shadow_position,
                options.shadow_color,
                options.opacity,
                options.clip,
                options.mask.as_ref(),
                options.blend_mode,
            )?;
        }
        self.draw_glyph_mask_color(
            glyph,
            position,
            options.color,
            options.opacity,
            options.clip,
            options.mask.as_ref(),
            options.blend_mode,
        )
    }

    pub fn draw_styled_glyph_run(
        &mut self,
        run: &GlyphRun,
        options: &GlyphDrawOptions,
    ) -> Result<()> {
        validate_mask(options.mask.as_ref(), self)?;
        for glyph in run.glyphs() {
            self.draw_styled_glyph(&glyph.glyph, glyph.baseline, options)?;
        }
        Ok(())
    }

    pub fn draw_text_run(
        &mut self,
        run: &StyledTextRun,
        opacity: u8,
        clip: Option<Rect>,
        mask: Option<&AlphaMask>,
        blend_mode: BlendMode,
        effects: GlyphEffects,
    ) -> Result<()> {
        validate_mask(mask, self)?;
        for glyph in run.glyphs() {
            let mut options = GlyphDrawOptions::new(if glyph.ruby {
                glyph.style.ruby_color
            } else {
                glyph.style.color
            });
            options.shadow_color = glyph.style.shadow_color;
            options.opacity = opacity;
            options.clip = clip;
            options.mask = mask.cloned();
            options.blend_mode = blend_mode;
            options.effects = effects;
            self.draw_styled_glyph(&glyph.glyph, glyph.baseline, &options)?;
        }
        Ok(())
    }

    fn draw_glyph_outline(
        &mut self,
        glyph: &GlyphMask,
        position: Point,
        options: &GlyphDrawOptions,
    ) -> Result<()> {
        let expanded = Rect::new(
            position.x.saturating_sub(1),
            position.y.saturating_sub(1),
            glyph.width.saturating_add(2),
            glyph.height.saturating_add(2),
        );
        let Some((left, top, right, bottom)) = self.drawable_bounds(expanded, options.clip) else {
            return Ok(());
        };
        for y in top..bottom {
            for x in left..right {
                let glyph_x = i64::from(x) - i64::from(position.x);
                let glyph_y = i64::from(y) - i64::from(position.y);
                let mut coverage = 0;
                for offset_y in -1_i64..=1 {
                    for offset_x in -1_i64..=1 {
                        let sample_x = glyph_x + offset_x;
                        let sample_y = glyph_y + offset_y;
                        if sample_x >= 0
                            && sample_y >= 0
                            && sample_x < i64::from(glyph.width)
                            && sample_y < i64::from(glyph.height)
                        {
                            coverage = coverage
                                .max(glyph.alpha_unchecked(sample_x as u32, sample_y as u32));
                        }
                    }
                }
                if coverage == 0 {
                    continue;
                }
                let source = options
                    .shadow_color
                    .with_alpha(modulate_alpha(options.shadow_color.a, coverage));
                let source =
                    apply_opacity_and_mask(source, options.opacity, options.mask.as_ref(), x, y);
                self.blend_pixel_unchecked_with_mode(x, y, source, options.blend_mode);
            }
        }
        Ok(())
    }

    fn draw_glyph_mask_color(
        &mut self,
        glyph: &GlyphMask,
        position: Point,
        color: Color,
        opacity: u8,
        clip: Option<Rect>,
        mask: Option<&AlphaMask>,
        blend_mode: BlendMode,
    ) -> Result<()> {
        self.draw_glyph_with_blend(glyph, position, color, opacity, clip, mask, blend_mode)
    }
}

#[derive(Debug, Clone)]
pub struct GlyphDrawOptions {
    pub color: Color,
    pub shadow_color: Color,
    pub opacity: u8,
    pub clip: Option<Rect>,
    pub mask: Option<AlphaMask>,
    pub blend_mode: BlendMode,
    pub effects: GlyphEffects,
}

impl GlyphDrawOptions {
    pub fn new(color: Color) -> Self {
        Self {
            color,
            shadow_color: Color::BLACK,
            opacity: 255,
            clip: None,
            mask: None,
            blend_mode: BlendMode::SourceOver,
            effects: GlyphEffects::default(),
        }
    }
}

impl Default for GlyphDrawOptions {
    fn default() -> Self {
        Self::new(Color::WHITE)
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct TextStyle {
    pub color: Color,
    pub ruby_color: Color,
    pub shadow_color: Color,
    pub size: u32,
    pub ruby_size: u32,
    pub bold: bool,
    pub italic: bool,
}

impl Default for TextStyle {
    fn default() -> Self {
        Self {
            color: Color::WHITE,
            ruby_color: Color::WHITE,
            shadow_color: Color::BLACK,
            size: 20,
            ruby_size: 10,
            bold: false,
            italic: false,
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum TextAlignment {
    Left,
    Center,
    Right,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Default)]
pub enum TextEncoding {
    #[default]
    Cp932,
    Unicode,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct TextSizePresets {
    pub text: [u32; 5],
    pub ruby: [u32; 5],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextLayoutSettings {
    pub origin: Point,
    pub width: i32,
    pub line_height: i32,
    pub letter_spacing: i32,
    pub line_spacing: i32,
    pub ruby_offset: i32,
    pub break_before: Vec<char>,
    pub break_after: Vec<char>,
}

impl Default for TextLayoutSettings {
    fn default() -> Self {
        Self {
            origin: Point::default(),
            width: i32::MAX,
            line_height: 20,
            letter_spacing: 0,
            line_spacing: 0,
            ruby_offset: 0,
            break_before: vec!['、', '。', '，', '．', '！', '？', '）', '］', '」', '』'],
            break_after: vec!['（', '［', '「', '『'],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextToken {
    Glyph {
        character: char,
        style: TextStyle,
        ruby: bool,
        group: Option<u32>,
    },
    Newline,
    Continue,
    WaitCharacters(u32),
    WaitFrames(u32),
    ResetWait,
    Align(TextAlignment),
    PositionX(u32),
    PositionY(u32),
    Identifier {
        key: i32,
        value: u32,
    },
    Control(u8),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TextMarkup {
    tokens: Vec<TextToken>,
    identifiers: BTreeMap<i32, u32>,
    encoding: TextEncoding,
}

impl TextMarkup {
    pub fn parse(input: &str, style: TextStyle) -> Result<Self> {
        Self::parse_with_encoding(input, style, TextEncoding::Cp932)
    }

    pub fn parse_with_sizes(
        input: &str,
        style: TextStyle,
        encoding: TextEncoding,
        sizes: TextSizePresets,
    ) -> Result<Self> {
        Self::parse_inner(input, style, encoding, Some(sizes))
    }

    pub fn parse_with_encoding(
        input: &str,
        style: TextStyle,
        encoding: TextEncoding,
    ) -> Result<Self> {
        Self::parse_inner(input, style, encoding, None)
    }

    fn parse_inner(
        input: &str,
        mut style: TextStyle,
        encoding: TextEncoding,
        sizes: Option<TextSizePresets>,
    ) -> Result<Self> {
        let chars: Vec<char> = input.chars().collect();
        let mut tokens = Vec::new();
        let mut identifiers = BTreeMap::new();
        let mut cursor = 0usize;
        let mut grouped = false;
        let mut ruby = false;
        let mut group = None;
        let mut next_group = 0_u32;
        let defaults = style;
        let mut restore_color = style.color;
        let default_sizes = sizes.unwrap_or(TextSizePresets {
            text: [defaults.size; 5],
            ruby: [defaults.ruby_size; 5],
        });
        while cursor < chars.len() {
            let character = chars[cursor];
            cursor += 1;
            if character == '[' && !grouped {
                grouped = true;
                ruby = false;
                group = Some(next_group);
                continue;
            }
            if character == ']' && grouped {
                grouped = false;
                ruby = false;
                group = None;
                next_group = next_group.wrapping_add(1);
                continue;
            }
            if character == '/' && grouped && !ruby {
                ruby = true;
                continue;
            }
            if character != '\\' {
                if character == '\n' {
                    tokens.push(TextToken::Newline);
                } else {
                    tokens.push(TextToken::Glyph {
                        character,
                        style,
                        ruby,
                        group,
                    });
                }
                continue;
            }

            if cursor == chars.len() {
                tokens.push(TextToken::Newline);
                break;
            }
            let marker = chars[cursor];
            cursor += 1;
            match marker.to_ascii_lowercase() {
                'n' => tokens.push(TextToken::Newline),
                '@' => tokens.push(TextToken::Continue),
                's' => {
                    tokens.push(TextToken::Glyph {
                        character: match encoding {
                            TextEncoding::Cp932 => '\u{3000}',
                            TextEncoding::Unicode => '\u{8140}',
                        },
                        style,
                        ruby,
                        group,
                    });
                    tokens.push(TextToken::Newline);
                }
                'r' => tokens.push(TextToken::Control(8)),
                'i' => {
                    if chars
                        .get(cursor)
                        .is_some_and(|value| value.eq_ignore_ascii_case(&'d'))
                    {
                        cursor += 1;
                        let key = parse_text_number(&chars, &mut cursor).unwrap_or(0) as i32;
                        let value = if chars.get(cursor) == Some(&':') {
                            cursor += 1;
                            parse_text_number(&chars, &mut cursor).unwrap_or(0)
                        } else {
                            0
                        };
                        identifiers.entry(key).or_insert(value);
                        tokens.push(TextToken::Identifier { key, value });
                    } else {
                        tokens.push(TextToken::Control(6));
                    }
                }
                'u' => {
                    if encoding == TextEncoding::Unicode {
                        if chars
                            .get(cursor)
                            .is_some_and(|value| value.eq_ignore_ascii_case(&'i'))
                        {
                            cursor += 1;
                        }
                        tokens.push(TextToken::Control(6));
                    } else {
                        tokens.push(TextToken::Glyph {
                            character: marker,
                            style,
                            ruby,
                            group,
                        });
                    }
                }
                'p' => {
                    let alignment = match chars.get(cursor).copied() {
                        Some('c' | 'C') => {
                            cursor += 1;
                            TextAlignment::Center
                        }
                        Some('r' | 'R') => {
                            cursor += 1;
                            TextAlignment::Right
                        }
                        _ => TextAlignment::Left,
                    };
                    tokens.push(TextToken::Align(alignment));
                }
                'x' | 'y' => {
                    if let Some(value) = parse_text_number(&chars, &mut cursor) {
                        if marker.eq_ignore_ascii_case(&'x') {
                            tokens.push(TextToken::PositionX(value));
                        } else {
                            tokens.push(TextToken::PositionY(value));
                        }
                    }
                }
                'w' => match chars
                    .get(cursor)
                    .copied()
                    .map(|value| value.to_ascii_lowercase())
                {
                    Some('r') => {
                        cursor += 1;
                        tokens.push(TextToken::ResetWait);
                    }
                    Some('f') => {
                        cursor += 1;
                        if let Some(value) = parse_text_number(&chars, &mut cursor) {
                            tokens.push(TextToken::WaitFrames(value));
                        }
                    }
                    _ => {
                        if let Some(value) = parse_text_number(&chars, &mut cursor) {
                            tokens.push(TextToken::WaitCharacters(value));
                        }
                    }
                },
                'c' => {
                    let suffix = chars
                        .get(cursor)
                        .copied()
                        .map(|value| value.to_ascii_lowercase());
                    match suffix {
                        Some('r') => {
                            cursor += 1;
                            if let Some(value) = parse_text_number(&chars, &mut cursor) {
                                style.ruby_color = text_color(value);
                            } else {
                                if cursor < chars.len() {
                                    let consumed = chars[cursor];
                                    cursor += 1;
                                    if consumed == ';' && cursor < chars.len() {
                                        cursor += 1;
                                    }
                                }
                                style.ruby_color = defaults.ruby_color;
                            }
                        }
                        Some('s') => {
                            cursor += 1;
                            if let Some(value) = parse_text_number(&chars, &mut cursor) {
                                style.shadow_color = text_color(value);
                            } else {
                                style.shadow_color = defaults.shadow_color;
                            }
                        }
                        Some('d') => {
                            cursor += 1;
                            if let Some(value) = parse_text_number(&chars, &mut cursor) {
                                restore_color = text_color(value);
                            } else {
                                restore_color = defaults.color;
                            }
                        }
                        _ => {
                            style.color = parse_text_number(&chars, &mut cursor)
                                .map(text_color)
                                .unwrap_or(restore_color);
                        }
                    }
                }
                'f' => {
                    match chars
                        .get(cursor)
                        .copied()
                        .map(|value| value.to_ascii_lowercase())
                    {
                        Some('r') => {
                            cursor += 1;
                            style.ruby_size = parse_text_number(&chars, &mut cursor)
                                .unwrap_or_else(|| {
                                    if cursor < chars.len() {
                                        let consumed = chars[cursor];
                                        cursor += 1;
                                        if consumed == ';' && cursor < chars.len() {
                                            cursor += 1;
                                        }
                                    }
                                    defaults.ruby_size
                                });
                        }
                        Some('n') => {
                            cursor += 1;
                            style.size = default_sizes.text[2];
                            style.ruby_size = default_sizes.ruby[2];
                        }
                        Some('s' | 'l') => {
                            let first = chars[cursor].to_ascii_lowercase();
                            cursor += 1;
                            let doubled = chars
                                .get(cursor)
                                .is_some_and(|value| value.to_ascii_lowercase() == first);
                            if doubled {
                                cursor += 1;
                            }
                            let index = match (first, doubled) {
                                ('s', false) => 1,
                                ('s', true) => 0,
                                ('l', false) => 3,
                                ('l', true) => 4,
                                _ => unreachable!(),
                            };
                            style.size = default_sizes.text[index];
                            style.ruby_size = default_sizes.ruby[index];
                            if chars.get(cursor) == Some(&';') {
                                cursor += 1;
                            }
                        }
                        Some('+') | Some('-') => {
                            let sign = chars[cursor];
                            cursor += 1;
                            if let Some(value) = parse_text_number(&chars, &mut cursor) {
                                if sign == '+' {
                                    style.size = style.size.wrapping_add(value);
                                } else {
                                    style.size = style.size.wrapping_sub(value);
                                }
                            }
                        }
                        _ => {
                            style.size =
                                parse_text_number(&chars, &mut cursor).unwrap_or_else(|| {
                                    if chars.get(cursor) == Some(&';') {
                                        cursor += 1;
                                    }
                                    defaults.size
                                });
                        }
                    }
                }
                't' => {
                    style.bold = false;
                    style.italic = false;
                    while let Some(value) = chars.get(cursor).copied() {
                        match value.to_ascii_lowercase() {
                            'b' => style.bold = true,
                            'i' => style.italic = true,
                            ';' => {
                                cursor += 1;
                                break;
                            }
                            _ => break,
                        }
                        cursor += 1;
                    }
                }
                'z' => {
                    return Err(RendererError::UnsupportedTextMarker { marker });
                }
                _ => tokens.push(TextToken::Glyph {
                    character: marker,
                    style,
                    ruby,
                    group,
                }),
            }
        }
        Ok(Self {
            tokens,
            identifiers,
            encoding,
        })
    }

    pub fn tokens(&self) -> &[TextToken] {
        &self.tokens
    }

    pub fn identifiers(&self) -> &BTreeMap<i32, u32> {
        &self.identifiers
    }

    /// The message-object path remains significant after decoding the input:
    /// for example, control record eight is a newline only in Unicode pages.
    pub fn encoding(&self) -> TextEncoding {
        self.encoding
    }

    pub fn layout(
        &self,
        origin: Point,
        line_height: i32,
        letter_spacing: i32,
        rasterize: impl FnMut(char, u32, bool, bool) -> Result<GlyphMask>,
    ) -> Result<StyledTextRun> {
        if line_height <= 0 {
            return Err(RendererError::InvalidEffectParameter {
                name: "text line height",
            });
        }
        self.layout_with_width(
            origin,
            line_height,
            letter_spacing,
            None,
            None,
            None,
            0,
            rasterize,
        )
    }

    pub fn layout_in_width(
        &self,
        origin: Point,
        width: i32,
        line_height: i32,
        letter_spacing: i32,
        rasterize: impl FnMut(char, u32, bool, bool) -> Result<GlyphMask>,
    ) -> Result<StyledTextRun> {
        if width < 0 || line_height <= 0 {
            return Err(RendererError::InvalidEffectParameter {
                name: "text layout dimensions",
            });
        }
        self.layout_with_width(
            origin,
            line_height,
            letter_spacing,
            Some(width),
            None,
            None,
            0,
            rasterize,
        )
    }

    pub fn layout_with_settings(
        &self,
        settings: &TextLayoutSettings,
        rasterize: impl FnMut(char, u32, bool, bool) -> Result<GlyphMask>,
    ) -> Result<StyledTextRun> {
        if settings.width < 0
            || settings.line_height <= 0
            || settings.line_height.saturating_add(settings.line_spacing) <= 0
        {
            return Err(RendererError::InvalidEffectParameter {
                name: "text layout dimensions",
            });
        }
        self.layout_with_width(
            settings.origin,
            settings.line_height.saturating_add(settings.line_spacing),
            settings.letter_spacing,
            Some(settings.width),
            Some(&settings.break_before),
            Some(&settings.break_after),
            settings.ruby_offset,
            rasterize,
        )
    }

    fn layout_with_width(
        &self,
        origin: Point,
        line_height: i32,
        letter_spacing: i32,
        available_width: Option<i32>,
        break_before: Option<&[char]>,
        break_after: Option<&[char]>,
        ruby_offset: i32,
        rasterize: impl FnMut(char, u32, bool, bool) -> Result<GlyphMask>,
    ) -> Result<StyledTextRun> {
        text_layout::layout(
            self,
            origin,
            line_height,
            letter_spacing,
            available_width,
            break_before,
            break_after,
            ruby_offset,
            rasterize,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StyledTextGlyph {
    pub glyph: GlyphMask,
    pub baseline: Point,
    pub style: TextStyle,
    pub ruby: bool,
    pub group: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StyledTextRun {
    glyphs: Vec<StyledTextGlyph>,
    advance: Point,
}

impl StyledTextRun {
    pub fn glyphs(&self) -> &[StyledTextGlyph] {
        &self.glyphs
    }

    pub fn advance(&self) -> Point {
        self.advance
    }
}

fn parse_text_number(chars: &[char], cursor: &mut usize) -> Option<u32> {
    let start = *cursor;
    let radix =
        if chars.get(*cursor) == Some(&'0') && matches!(chars.get(*cursor + 1), Some('x' | 'X')) {
            *cursor += 2;
            16
        } else {
            10
        };
    let mut value = 0u32;
    while let Some(character) = chars.get(*cursor).copied() {
        let Some(digit) = character.to_digit(radix) else {
            break;
        };
        value = value.wrapping_mul(radix).wrapping_add(digit);
        *cursor += 1;
    }
    if chars.get(*cursor) == Some(&';') {
        *cursor += 1;
    }
    (*cursor > start).then_some(value)
}

fn text_color(value: u32) -> Color {
    Color::rgba(
        (value & 0xff) as u8,
        ((value >> 8) & 0xff) as u8,
        ((value >> 16) & 0xff) as u8,
        (value >> 24) as u8,
    )
}

/// A fontdue-backed glyph service. It deliberately accepts explicit font
/// bytes so the desktop host can choose the same game-provided font every run.
#[derive(Debug, Clone)]
pub struct FontRasterizer {
    font: fontdue::Font,
    glyph_cache: Arc<Mutex<text_cache::GlyphCache>>,
    layout_cache: Arc<Mutex<text_cache::LayoutCache>>,
}

impl FontRasterizer {
    pub fn from_bytes(bytes: &[u8], collection_index: u32) -> Result<Self> {
        if bytes.len() > 64 * 1024 * 1024 {
            return Err(RendererError::Decode {
                message: "font data exceeds the 64 MiB limit".into(),
            });
        }
        let font = fontdue::Font::from_bytes(
            bytes,
            fontdue::FontSettings {
                collection_index,
                load_substitutions: false,
                ..Default::default()
            },
        )
        .map_err(|message| RendererError::Decode {
            message: format!("font decode failed: {message}"),
        })?;
        Ok(Self {
            font,
            glyph_cache: Arc::default(),
            layout_cache: Arc::default(),
        })
    }

    pub fn has_character(&self, character: char) -> bool {
        self.font.lookup_glyph_index(character) != 0
    }

    pub fn measure(&self, character: char, size: u32) -> Result<i32> {
        let metrics = self.metrics(character, size)?;
        Ok(metrics.advance_width.round() as i32)
    }

    pub fn baseline(&self, size: u32) -> i32 {
        self.font
            .horizontal_line_metrics(size as f32)
            .map_or(size as i32, |line| line.ascent.round() as i32)
            .min(size as i32)
    }

    pub fn rasterize(&self, character: char, size: u32) -> Result<GlyphMask> {
        let metrics = self.metrics(character, size)?;
        let baseline = self.baseline(size);
        let (_, coverage) = self.font.rasterize(character, size as f32);
        let coverage = coverage
            .into_iter()
            .map(|value| {
                let level = (u32::from(value) * 64 + 127) / 255;
                (level * 255 / 64) as u8
            })
            .collect();
        GlyphMask::from_alpha_with_metrics(
            metrics.width as u32,
            metrics.height as u32,
            coverage,
            GlyphMetrics {
                bearing_x: metrics.xmin.saturating_add(1).max(0),
                bearing_y: baseline
                    .saturating_sub(metrics.ymin)
                    .saturating_sub(metrics.height as i32)
                    .saturating_add(1),
                advance: metrics.advance_width.round() as i32,
            },
        )
    }

    pub fn measure_styled(
        &self,
        character: char,
        size: u32,
        bold: bool,
        italic: bool,
    ) -> Result<i32> {
        let glyph = self.rasterize_styled_shared(character, size, bold, italic)?;
        Ok(glyph.advance())
    }

    pub fn rasterize_styled(
        &self,
        character: char,
        size: u32,
        bold: bool,
        italic: bool,
    ) -> Result<GlyphMask> {
        let key = (character, size, bold, italic);
        if let Ok(mut cache) = self.glyph_cache.lock()
            && let Some(glyph) = cache.get(key)
        {
            return Ok(glyph.as_ref().clone());
        }
        Ok(self.rasterize_styled_miss(key)?.as_ref().clone())
    }

    /// Shares the cached mask and its immutable coverage with other draws.
    pub fn rasterize_styled_shared(
        &self,
        character: char,
        size: u32,
        bold: bool,
        italic: bool,
    ) -> Result<Arc<GlyphMask>> {
        let key = (character, size, bold, italic);
        if let Ok(mut cache) = self.glyph_cache.lock()
            && let Some(glyph) = cache.get(key)
        {
            return Ok(Arc::clone(glyph));
        }
        self.rasterize_styled_miss(key)
    }

    fn rasterize_styled_miss(&self, key: (char, u32, bool, bool)) -> Result<Arc<GlyphMask>> {
        let (character, size, bold, italic) = key;
        let glyph = self.rasterize(character, size)?;
        let glyph = if bold { embolden_glyph(&glyph)? } else { glyph };
        let glyph = if italic { italicize_glyph(&glyph)? } else { glyph };
        let glyph = Arc::new(glyph);
        if let Ok(mut cache) = self.glyph_cache.lock() {
            return Ok(cache.insert(key, glyph));
        }
        Ok(glyph)
    }

    /// Cached layout using this font alone. `visible_tokens` is a prefix length
    /// in parsed tokens (including controls), not a glyph or UTF-8 byte count.
    /// Use `usize::MAX` to lay out all tokens, including when reveal is applied
    /// after layout. Callback-based mixed-font layout is intentionally uncached.
    pub fn layout_cached(
        &self,
        markup: &TextMarkup,
        settings: &TextLayoutSettings,
        visible_tokens: usize,
    ) -> Result<Arc<StyledTextRun>> {
        let tokens = &markup.tokens[..visible_tokens.min(markup.tokens.len())];
        let encoding = markup.encoding;
        if let Ok(mut cache) = self.layout_cache.lock()
            && let Some(run) = cache.get(tokens, encoding, settings)
        {
            return Ok(run);
        }
        let prefix;
        let markup = if tokens.len() == markup.tokens.len() {
            markup
        } else {
            prefix = TextMarkup {
                tokens: tokens.to_vec(),
                encoding,
                identifiers: BTreeMap::new(),
            };
            &prefix
        };
        let run = Arc::new(
            markup.layout_with_settings(settings, |character, size, bold, italic| {
                self.rasterize_styled(character, size, bold, italic)
            })?,
        );
        if let Ok(mut cache) = self.layout_cache.lock() {
            return Ok(cache.insert(tokens, encoding, settings, run));
        }
        Ok(run)
    }

    fn metrics(&self, character: char, size: u32) -> Result<fontdue::Metrics> {
        if size == 0 {
            return Err(RendererError::InvalidEffectParameter { name: "font size" });
        }
        if size > i32::MAX as u32 {
            return Err(RendererError::ScaledDimensionsOverflow);
        }
        if !self.has_character(character) {
            return Err(RendererError::Decode {
                message: format!("font is missing U+{:04X}", character as u32),
            });
        }
        let metrics = self.font.metrics(character, size as f32);
        let advance = metrics.advance_width.round();
        if !metrics.advance_width.is_finite()
            || advance < i32::MIN as f32
            || f64::from(advance) > f64::from(i32::MAX)
            || metrics.width > i32::MAX as usize
            || metrics.height > i32::MAX as usize
        {
            return Err(RendererError::ScaledDimensionsOverflow);
        }
        glyph_len(metrics.width as u32, metrics.height as u32)?;
        Ok(metrics)
    }
}

fn embolden_glyph(glyph: &GlyphMask) -> Result<GlyphMask> {
    let width = glyph
        .width
        .checked_add(1)
        .ok_or(RendererError::ScaledDimensionsOverflow)?;
    let mut alpha = vec![0; glyph_len(width, glyph.height)?];
    for y in 0..glyph.height {
        for x in 0..glyph.width {
            let value = glyph.alpha_unchecked(x, y);
            let at = y as usize * width as usize + x as usize;
            alpha[at] = alpha[at].max(value);
            alpha[at + 1] = alpha[at + 1].max(value);
        }
    }
    GlyphMask::from_alpha_with_metrics(
        width,
        glyph.height,
        alpha,
        GlyphMetrics {
            advance: glyph.metrics.advance.saturating_add(1),
            ..glyph.metrics
        },
    )
}

fn italicize_glyph(glyph: &GlyphMask) -> Result<GlyphMask> {
    let slant = glyph.height.div_ceil(4);
    let width = glyph
        .width
        .checked_add(slant)
        .ok_or(RendererError::ScaledDimensionsOverflow)?;
    let mut alpha = vec![0; glyph_len(width, glyph.height)?];
    for y in 0..glyph.height {
        let shift = slant.saturating_mul(glyph.height - 1 - y) / glyph.height.max(1);
        for x in 0..glyph.width {
            let source = glyph.alpha_unchecked(x, y);
            let target = y as usize * width as usize + (x + shift) as usize;
            alpha[target] = alpha[target].max(source);
        }
    }
    GlyphMask::from_alpha_with_metrics(
        width,
        glyph.height,
        alpha,
        GlyphMetrics {
            bearing_x: glyph.metrics.bearing_x,
            bearing_y: glyph.metrics.bearing_y,
            advance: glyph.metrics.advance.saturating_add(slant as i32 / 2),
        },
    )
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct ParticlePoint {
    pub position: Point,
    pub color: Color,
}

impl ParticlePoint {
    pub const fn new(position: Point, color: Color) -> Self {
        Self { position, color }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Particle {
    pub position: [f32; 2],
    pub velocity: [f32; 2],
    pub acceleration: [f32; 2],
    pub color: Color,
    pub size: f32,
    pub age: u32,
    pub lifetime: u32,
    pub opacity_start: u8,
    pub opacity_end: u8,
}

impl Particle {
    pub fn new(position: [f32; 2], color: Color) -> Self {
        Self {
            position,
            velocity: [0.0; 2],
            acceleration: [0.0; 2],
            color,
            size: 1.0,
            age: 0,
            lifetime: 0,
            opacity_start: 255,
            opacity_end: 255,
        }
    }

    pub fn validate(&self) -> Result<()> {
        if self
            .position
            .into_iter()
            .chain(self.velocity)
            .chain(self.acceleration)
            .chain([self.size])
            .any(|value| !value.is_finite())
            || self.size <= 0.0
        {
            return Err(RendererError::InvalidEffectParameter {
                name: "particle geometry",
            });
        }
        Ok(())
    }

    pub fn is_alive(&self) -> bool {
        self.lifetime == 0 || self.age < self.lifetime
    }

    pub fn opacity(&self) -> u8 {
        if self.lifetime == 0 || self.lifetime <= 1 {
            return self.opacity_end;
        }
        let progress = self.age.min(self.lifetime - 1) as u32;
        let span = i32::from(self.opacity_end) - i32::from(self.opacity_start);
        (i32::from(self.opacity_start) + span * progress as i32 / (self.lifetime - 1) as i32)
            .clamp(0, 255) as u8
    }

    pub fn advance(&mut self, ticks: u32) -> Result<bool> {
        self.validate()?;
        let ticks = ticks.min(u32::MAX / 2);
        let delta = ticks as f32;
        self.position[0] += self.velocity[0] * delta + self.acceleration[0] * delta * delta * 0.5;
        self.position[1] += self.velocity[1] * delta + self.acceleration[1] * delta * delta * 0.5;
        self.velocity[0] += self.acceleration[0] * delta;
        self.velocity[1] += self.acceleration[1] * delta;
        self.age = self.age.saturating_add(ticks);
        self.validate()?;
        Ok(self.is_alive())
    }
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct ParticleSystem {
    particles: Vec<Particle>,
}

impl ParticleSystem {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn particles(&self) -> &[Particle] {
        &self.particles
    }

    pub fn particles_mut(&mut self) -> &mut [Particle] {
        &mut self.particles
    }

    pub fn spawn(&mut self, particle: Particle) -> Result<()> {
        particle.validate()?;
        self.particles.push(particle);
        Ok(())
    }

    pub fn update(&mut self, ticks: u32) -> Result<()> {
        for particle in &mut self.particles {
            particle.advance(ticks)?;
        }
        self.particles.retain(Particle::is_alive);
        Ok(())
    }

    pub fn clear(&mut self) {
        self.particles.clear();
    }

    pub fn draw(
        &self,
        canvas: &mut Canvas,
        clip: Option<Rect>,
        mask: Option<&AlphaMask>,
        blend_mode: BlendMode,
    ) -> Result<()> {
        for particle in &self.particles {
            canvas.draw_particle_sprite(particle, clip, mask, blend_mode)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct ImageDrawOptions {
    pub position: Point,
    pub scale_x: f32,
    pub scale_y: f32,
    pub opacity: u8,
    pub visible: bool,
    pub clip: Option<Rect>,
    pub mask: Option<AlphaMask>,
    pub pivot: [f32; 2],
    pub rotation_radians: f32,
    pub flip_x: bool,
    pub flip_y: bool,
    pub blend_mode: BlendMode,
    pub filter: ScaleFilter,
}

impl ImageDrawOptions {
    pub fn new(position: Point) -> Self {
        Self {
            position,
            scale_x: 1.0,
            scale_y: 1.0,
            opacity: 255,
            visible: true,
            clip: None,
            mask: None,
            pivot: [0.0; 2],
            rotation_radians: 0.0,
            flip_x: false,
            flip_y: false,
            blend_mode: BlendMode::SourceOver,
            filter: ScaleFilter::Nearest,
        }
    }

    pub fn transform(&self) -> Transform2D {
        Transform2D::new(
            [self.position.x as f32, self.position.y as f32],
            self.pivot,
            [
                if self.flip_x {
                    -self.scale_x
                } else {
                    self.scale_x
                },
                if self.flip_y {
                    -self.scale_y
                } else {
                    self.scale_y
                },
            ],
            self.rotation_radians,
        )
    }

    pub fn set_transform(&mut self, transform: Transform2D) {
        let _ = self.try_set_transform(transform);
    }

    pub fn try_set_transform(&mut self, transform: Transform2D) -> Result<()> {
        transform.validate()?;
        self.position = Point::new(
            transform.position[0].round() as i32,
            transform.position[1].round() as i32,
        );
        self.pivot = transform.pivot;
        self.scale_x = transform.scale[0].abs();
        self.scale_y = transform.scale[1].abs();
        self.flip_x = transform.scale[0].is_sign_negative();
        self.flip_y = transform.scale[1].is_sign_negative();
        self.rotation_radians = transform.rotation;
        Ok(())
    }
}

impl Default for ImageDrawOptions {
    fn default() -> Self {
        Self::new(Point::default())
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
#[repr(i32)]
pub enum LayerKind {
    Background = 0,
    Backdrop = 100,
    Plane = 200,
    Image = 300,
    Particle = 400,
    Text = 500,
    Ui = 600,
    Transition = 700,
}

impl LayerKind {
    pub const fn priority(self) -> i32 {
        self as i32
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct LayerKey {
    pub kind: LayerKind,
    pub depth: i32,
}

impl LayerKey {
    pub const fn priority(self) -> i32 {
        self.kind.priority().saturating_add(self.depth)
    }
}

#[derive(Debug, Clone)]
pub enum RenderCommand {
    FillRect {
        rect: Rect,
        color: Color,
        clip: Option<Rect>,
    },
    StyledRect {
        rect: Rect,
        color: Color,
        opacity: u8,
        clip: Option<Rect>,
        blend_mode: BlendMode,
    },
    Frame {
        frame: RgbaFrame,
        options: ImageDrawOptions,
    },
    Crossfade {
        from: RgbaFrame,
        to: RgbaFrame,
        progress: u16,
        position: Point,
        clip: Option<Rect>,
    },
    MaskTransition {
        from: RgbaFrame,
        to: RgbaFrame,
        mask: RgbaFrame,
        progress: u16,
        channel: MaskTransitionChannel,
        position: Point,
        clip: Option<Rect>,
    },
    Particle {
        particle: ParticlePoint,
        clip: Option<Rect>,
        mask: Option<AlphaMask>,
    },
    StyledParticle {
        particle: ParticlePoint,
        opacity: u8,
        clip: Option<Rect>,
        mask: Option<AlphaMask>,
        blend_mode: BlendMode,
    },
    ParticleBatch {
        particles: Vec<Particle>,
        clip: Option<Rect>,
        mask: Option<AlphaMask>,
        blend_mode: BlendMode,
    },
    Glyph {
        glyph: GlyphMask,
        position: Point,
        color: Color,
        opacity: u8,
        clip: Option<Rect>,
        mask: Option<AlphaMask>,
    },
    StyledGlyph {
        glyph: GlyphMask,
        baseline: Point,
        options: GlyphDrawOptions,
    },
    GlyphRun {
        run: GlyphRun,
        options: GlyphDrawOptions,
    },
    TextRun {
        run: StyledTextRun,
        opacity: u8,
        clip: Option<Rect>,
        mask: Option<AlphaMask>,
        blend_mode: BlendMode,
        effects: GlyphEffects,
    },
}

#[derive(Debug, Clone)]
pub struct RenderNode {
    pub priority: i32,
    pub command: RenderCommand,
    sequence: u64,
}

impl RenderNode {
    pub fn new(priority: i32, command: RenderCommand) -> Self {
        Self {
            priority,
            command,
            sequence: 0,
        }
    }

    pub fn layered(layer: LayerKey, command: RenderCommand) -> Self {
        Self::new(layer.priority(), command)
    }
}

#[derive(Debug, Default, Clone)]
pub struct RenderQueue {
    nodes: Vec<RenderNode>,
    next_sequence: u64,
}

impl RenderQueue {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, mut node: RenderNode) {
        node.sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.wrapping_add(1);
        self.nodes.push(node);
    }

    pub fn push_layer(&mut self, layer: LayerKey, command: RenderCommand) {
        self.push(RenderNode::layered(layer, command));
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn nodes(&self) -> &[RenderNode] {
        &self.nodes
    }

    pub fn clear(&mut self) {
        self.nodes.clear();
        self.next_sequence = 0;
    }

    pub fn render_into(&self, canvas: &mut Canvas) -> Result<()> {
        let mut order: Vec<usize> = (0..self.nodes.len()).collect();
        order.sort_by_key(|&index| (self.nodes[index].priority, self.nodes[index].sequence));
        for index in order {
            render_command(canvas, &self.nodes[index].command)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct LayerEntry {
    pub key: LayerKey,
    pub frame: RgbaFrame,
    pub options: ImageDrawOptions,
    sequence: u64,
}

/// Stable, reusable image-layer composition. The stack owns the decoded
/// frames and keeps equal-depth layers in insertion order.
#[derive(Debug, Default, Clone)]
pub struct LayerStack {
    entries: Vec<LayerEntry>,
    next_sequence: u64,
}

impl LayerStack {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, key: LayerKey, frame: RgbaFrame, options: ImageDrawOptions) {
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.wrapping_add(1);
        self.entries.push(LayerEntry {
            key,
            frame,
            options,
            sequence,
        });
    }

    pub fn push_frame(
        &mut self,
        kind: LayerKind,
        depth: i32,
        frame: RgbaFrame,
        options: ImageDrawOptions,
    ) {
        self.push(LayerKey { kind, depth }, frame, options);
    }

    pub fn entries(&self) -> &[LayerEntry] {
        &self.entries
    }

    pub fn entry(&self, index: usize) -> Option<&LayerEntry> {
        self.entries.get(index)
    }

    pub fn replace(
        &mut self,
        index: usize,
        key: LayerKey,
        frame: RgbaFrame,
        options: ImageDrawOptions,
    ) -> Result<()> {
        let entry = self
            .entries
            .get_mut(index)
            .ok_or(RendererError::InvalidLayerIndex { index })?;
        entry.key = key;
        entry.frame = frame;
        entry.options = options;
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.next_sequence = 0;
    }

    pub fn remove(&mut self, index: usize) -> Option<LayerEntry> {
        (index < self.entries.len()).then(|| self.entries.remove(index))
    }

    pub fn retain(&mut self, mut keep: impl FnMut(&LayerEntry) -> bool) {
        self.entries.retain(|entry| keep(entry));
    }

    pub fn render_into(&self, canvas: &mut Canvas) -> Result<()> {
        let mut order: Vec<usize> = (0..self.entries.len()).collect();
        order.sort_by_key(|&index| {
            (
                self.entries[index].key.priority(),
                self.entries[index].sequence,
            )
        });
        for index in order {
            let entry = &self.entries[index];
            canvas.draw_frame(&entry.frame, &entry.options)?;
        }
        Ok(())
    }

    pub fn render_snapshot(&self, size: [u32; 2], background: Color) -> Result<FrameSnapshot> {
        let mut canvas = Canvas::new(size[0], size[1])?;
        canvas.clear(background);
        self.render_into(&mut canvas)?;
        Ok(canvas.snapshot())
    }

    pub fn render_commands(&self) -> RenderQueue {
        let mut queue = RenderQueue::new();
        let mut order: Vec<usize> = (0..self.entries.len()).collect();
        order.sort_by_key(|&index| {
            (
                self.entries[index].key.priority(),
                self.entries[index].sequence,
            )
        });
        for index in order {
            let entry = &self.entries[index];
            queue.push_layer(
                entry.key,
                RenderCommand::Frame {
                    frame: entry.frame.clone(),
                    options: entry.options.clone(),
                },
            );
        }
        queue
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameSnapshot {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
    hash: u64,
}

impl FrameSnapshot {
    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub fn rgba(&self) -> &[u8] {
        &self.rgba
    }

    pub fn hash(&self) -> u64 {
        self.hash
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameTransition {
    from: RgbaFrame,
    to: RgbaFrame,
    duration: u32,
    elapsed: u32,
}

impl FrameTransition {
    pub fn crossfade(from: RgbaFrame, to: RgbaFrame, duration: u32) -> Result<Self> {
        ensure_same_dimensions(&from, &to)?;
        Ok(Self {
            from,
            to,
            duration: duration.max(1),
            elapsed: 0,
        })
    }

    pub fn from(&self) -> &RgbaFrame {
        &self.from
    }

    pub fn to(&self) -> &RgbaFrame {
        &self.to
    }

    pub fn duration(&self) -> u32 {
        self.duration
    }

    pub fn elapsed(&self) -> u32 {
        self.elapsed
    }

    pub fn progress(&self) -> u16 {
        ((u64::from(self.elapsed.min(self.duration)) * 256) / u64::from(self.duration)).min(256)
            as u16
    }

    pub fn complete(&self) -> bool {
        self.elapsed >= self.duration
    }

    pub fn advance(&mut self, ticks: u32) -> u16 {
        self.elapsed = self.elapsed.saturating_add(ticks).min(self.duration);
        self.progress()
    }

    pub fn reset(&mut self) {
        self.elapsed = 0;
    }

    pub fn render(&self, canvas: &mut Canvas, position: Point, clip: Option<Rect>) -> Result<()> {
        canvas.crossfade_frames(&self.from, &self.to, self.progress(), position, clip)
    }

    pub fn render_masked(
        &self,
        canvas: &mut Canvas,
        mask: &RgbaFrame,
        channel: MaskTransitionChannel,
        position: Point,
        clip: Option<Rect>,
    ) -> Result<()> {
        let progress = self.progress().saturating_mul(2).min(512);
        canvas.mask_transition(
            &self.from, &self.to, mask, progress, channel, position, clip,
        )
    }
}

fn render_command(canvas: &mut Canvas, command: &RenderCommand) -> Result<()> {
    match command {
        RenderCommand::FillRect { rect, color, clip } => {
            canvas.fill_rect_clipped(*rect, *color, *clip)
        }
        RenderCommand::StyledRect {
            rect,
            color,
            opacity,
            clip,
            blend_mode,
        } => canvas.fill_rect_with_blend(*rect, *color, *opacity, *clip, *blend_mode),
        RenderCommand::Frame { frame, options } => canvas.draw_frame(frame, options),
        RenderCommand::Crossfade {
            from,
            to,
            progress,
            position,
            clip,
        } => canvas.crossfade_frames(from, to, *progress, *position, *clip),
        RenderCommand::MaskTransition {
            from,
            to,
            mask,
            progress,
            channel,
            position,
            clip,
        } => canvas.mask_transition(from, to, mask, *progress, *channel, *position, *clip),
        RenderCommand::Particle {
            particle,
            clip,
            mask,
        } => canvas.draw_particle_clipped(*particle, *clip, mask.as_ref()),
        RenderCommand::StyledParticle {
            particle,
            opacity,
            clip,
            mask,
            blend_mode,
        } => {
            canvas.draw_particle_with_blend(*particle, *opacity, *clip, mask.as_ref(), *blend_mode)
        }
        RenderCommand::ParticleBatch {
            particles,
            clip,
            mask,
            blend_mode,
        } => {
            let system = ParticleSystem {
                particles: particles.clone(),
            };
            system.draw(canvas, *clip, mask.as_ref(), *blend_mode)
        }
        RenderCommand::Glyph {
            glyph,
            position,
            color,
            opacity,
            clip,
            mask,
        } => canvas.draw_glyph(glyph, *position, *color, *opacity, *clip, mask.as_ref()),
        RenderCommand::StyledGlyph {
            glyph,
            baseline,
            options,
        } => canvas.draw_styled_glyph(glyph, *baseline, options),
        RenderCommand::GlyphRun { run, options } => canvas.draw_styled_glyph_run(run, options),
        RenderCommand::TextRun {
            run,
            opacity,
            clip,
            mask,
            blend_mode,
            effects,
        } => canvas.draw_text_run(run, *opacity, *clip, mask.as_ref(), *blend_mode, *effects),
    }
}

fn rgba_len(width: u32, height: u32) -> Result<usize> {
    if width == 0 || height == 0 {
        return Err(RendererError::InvalidDimensions { width, height });
    }
    let pixels = (width as usize)
        .checked_mul(height as usize)
        .ok_or(RendererError::ScaledDimensionsOverflow)?;
    pixels
        .checked_mul(4)
        .ok_or(RendererError::ScaledDimensionsOverflow)
}

fn alpha_len(width: u32, height: u32) -> Result<usize> {
    if width == 0 || height == 0 {
        return Err(RendererError::InvalidDimensions { width, height });
    }
    (width as usize)
        .checked_mul(height as usize)
        .ok_or(RendererError::ScaledDimensionsOverflow)
}

fn glyph_len(width: u32, height: u32) -> Result<usize> {
    (width as usize)
        .checked_mul(height as usize)
        .filter(|&length| length <= MAX_GLYPH_MASK_BYTES)
        .ok_or(RendererError::ScaledDimensionsOverflow)
}

fn ensure_same_dimensions(first: &RgbaFrame, second: &RgbaFrame) -> Result<()> {
    if first.dimensions() != second.dimensions() {
        return Err(RendererError::FrameDimensionsMismatch {
            first_width: first.width,
            first_height: first.height,
            second_width: second.width,
            second_height: second.height,
        });
    }
    Ok(())
}

fn encode_hg_container(frame: &RgbaFrame, container: HgContainer) -> Result<Vec<u8>> {
    let metadata = frame.metadata;
    if !frame.attributes.is_empty() && container != HgContainer::Hg3V300 {
        return Err(RendererError::UnsupportedHgAttributes { container });
    }
    let [width, height] = [frame.width, frame.height];
    let [canvas_width, canvas_height] = if metadata.canvas_size == [0, 0] {
        [width, height]
    } else {
        metadata.canvas_size
    };
    if canvas_width == 0
        || canvas_height == 0
        || metadata.offset[0] < 0
        || metadata.offset[1] < 0
        || i64::from(metadata.offset[0]) + i64::from(width) > i64::from(canvas_width)
        || i64::from(metadata.offset[1]) + i64::from(height) > i64::from(canvas_height)
    {
        return Err(RendererError::InvalidHgStructure {
            message: "HG crop and canvas geometry are outside the declared canvas".into(),
        });
    }
    if metadata.bit_depth != 0 && metadata.bit_depth != 32 {
        return Err(RendererError::InvalidHgStructure {
            message: format!("unsupported HG encoder bit depth {}", metadata.bit_depth),
        });
    }
    let mut bgra = Vec::new();
    bgra.try_reserve_exact(frame.pixels.len())
        .map_err(|_| RendererError::AllocationFailed {
            bytes: frame.pixels.len(),
        })?;
    let mut has_alpha = metadata.alpha_flag != 0;
    let mut bounds = [width, height, 0, 0];
    for pixel in frame.pixels.chunks_exact(4) {
        let alpha = pixel[3];
        has_alpha |= alpha != 255;
        let index = bgra.len() / 4;
        if alpha != 0 {
            bounds[0] = bounds[0].min(index as u32 % width);
            bounds[1] = bounds[1].min(index as u32 / width);
            bounds[2] = bounds[2].max(index as u32 % width + 1);
            bounds[3] = bounds[3].max(index as u32 / width + 1);
        }
        if alpha == 0 {
            bgra.extend_from_slice(&[0, 0, 0, 0]);
        } else {
            bgra.extend_from_slice(&[pixel[2], pixel[1], pixel[0], alpha]);
        }
    }
    let mut encode_width = width;
    let mut encode_height = height;
    let mut encode_offset = metadata.offset;
    if matches!(container, HgContainer::Hg2V20 | HgContainer::Hg2V25) {
        if bounds[2] == 0 {
            bounds = [0, 0, width.min(2), height.min(2)];
        }
        let [left, top, right, bottom] = bounds;
        encode_width = right - left;
        encode_height = bottom - top;
        encode_offset = [
            encode_offset[0].wrapping_add(left as i32),
            encode_offset[1].wrapping_add(top as i32),
        ];
        let mut cropped = Vec::new();
        let cropped_len = rgba_len(encode_width, encode_height)?;
        cropped
            .try_reserve_exact(cropped_len)
            .map_err(|_| RendererError::AllocationFailed { bytes: cropped_len })?;
        for y in top..bottom {
            let start = ((y * width + left) as usize) * 4;
            let end = ((y * width + right) as usize) * 4;
            cropped.extend_from_slice(&bgra[start..end]);
        }
        bgra = cropped;
    }
    if i64::from(encode_offset[0]) < 0
        || i64::from(encode_offset[1]) < 0
        || i64::from(encode_offset[0]) + i64::from(encode_width) > i64::from(canvas_width)
        || i64::from(encode_offset[1]) + i64::from(encode_height) > i64::from(canvas_height)
    {
        return Err(RendererError::InvalidHgStructure {
            message: "encoded HG crop and canvas geometry are outside the declared canvas".into(),
        });
    }
    let (literals, commands) = encode_hg_planes(
        &bgra,
        [encode_width, encode_height],
        container == HgContainer::Hg3V300,
    )?;
    let compressed_literals = compress_zlib(&literals)?;
    let compressed_commands = compress_zlib(&commands)?;
    let frame_id = metadata.frame_id.unwrap_or(0);
    let bit_depth = 32_u32;
    let alpha_flag = u32::from(has_alpha);

    match container {
        HgContainer::Hg2V20 => {
            let mut output = b"HG-2".to_vec();
            for value in [
                12,
                0x20,
                encode_width,
                encode_height,
                bit_depth,
                0,
                0,
                compressed_literals.len() as u32,
                literals.len() as u32,
                compressed_commands.len() as u32,
                commands.len() as u32,
                40,
                frame_id,
                canvas_width,
                canvas_height,
                encode_offset[0] as u32,
                encode_offset[1] as u32,
                alpha_flag,
                0,
            ] {
                output.extend_from_slice(&value.to_le_bytes());
            }
            output.extend_from_slice(&compressed_literals);
            output.extend_from_slice(&compressed_commands);
            Ok(output)
        }
        HgContainer::Hg2V25 => {
            let mut output = b"HG-2".to_vec();
            for value in [
                12,
                0x25,
                encode_width,
                encode_height,
                bit_depth,
                0,
                0,
                compressed_literals.len() as u32,
                literals.len() as u32,
                compressed_commands.len() as u32,
                commands.len() as u32,
                40,
                frame_id,
                canvas_width,
                canvas_height,
                encode_offset[0] as u32,
                encode_offset[1] as u32,
                alpha_flag,
                0,
                metadata.base[0] as u32,
                metadata.base[1] as u32,
            ] {
                output.extend_from_slice(&value.to_le_bytes());
            }
            output.extend_from_slice(&compressed_literals);
            output.extend_from_slice(&compressed_commands);
            Ok(output)
        }
        HgContainer::Hg3V300 => {
            let stdinfo = {
                let mut payload = Vec::with_capacity(40);
                for value in [
                    width,
                    height,
                    bit_depth,
                    metadata.offset[0] as u32,
                    metadata.offset[1] as u32,
                    canvas_width,
                    canvas_height,
                    alpha_flag,
                    metadata.base[0] as u32,
                    metadata.base[1] as u32,
                ] {
                    payload.extend_from_slice(&value.to_le_bytes());
                }
                payload
            };
            let mut image_payload =
                Vec::with_capacity(24 + compressed_literals.len() + compressed_commands.len());
            for value in [
                0,
                height,
                compressed_literals.len() as u32,
                literals.len() as u32,
                compressed_commands.len() as u32,
                commands.len() as u32,
            ] {
                image_payload.extend_from_slice(&value.to_le_bytes());
            }
            image_payload.extend_from_slice(&compressed_literals);
            image_payload.extend_from_slice(&compressed_commands);
            let mut output = b"HG-3".to_vec();
            output.extend_from_slice(&12_u32.to_le_bytes());
            output.extend_from_slice(&0x300_u32.to_le_bytes());
            output.extend_from_slice(&0_u32.to_le_bytes());
            output.extend_from_slice(&frame_id.to_le_bytes());
            write_hg3_tag(
                &mut output,
                *b"stdinfo\0",
                16 + stdinfo.len() as u32,
                &stdinfo,
            );
            for (&id, values) in frame.attributes.iter() {
                let name = format!("ats{id:04}");
                let mut tag_name = [0_u8; 8];
                let name_bytes = name.as_bytes();
                if name_bytes.is_empty() || name_bytes.len() > tag_name.len() {
                    return Err(RendererError::InvalidHgStructure {
                        message: format!("HG attribute ID {id} does not fit its tag name"),
                    });
                }
                tag_name[..name_bytes.len()].copy_from_slice(name_bytes);
                let mut payload = Vec::with_capacity(20);
                for value in values {
                    payload.extend_from_slice(&value.to_le_bytes());
                }
                write_hg3_tag(&mut output, tag_name, 36, &payload);
            }
            write_hg3_tag(&mut output, *b"img0000\0", 0, &image_payload);
            Ok(output)
        }
        HgContainer::None | HgContainer::Hg2V10 => Err(RendererError::InvalidEffectParameter {
            name: "HG container",
        }),
    }
}

fn write_hg3_tag(output: &mut Vec<u8>, name: [u8; 8], next: u32, payload: &[u8]) {
    output.extend_from_slice(&name);
    output.extend_from_slice(&next.to_le_bytes());
    output.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    output.extend_from_slice(payload);
}

fn compress_zlib(bytes: &[u8]) -> Result<Vec<u8>> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::best());
    encoder
        .write_all(bytes)
        .map_err(|error| RendererError::Decode {
            message: format!("HG compression failed: {error}"),
        })?;
    encoder.finish().map_err(|error| RendererError::Decode {
        message: format!("HG compression failed: {error}"),
    })
}

fn encode_hg_planes(
    pixels: &[u8],
    dimensions: [u32; 2],
    bottom_up: bool,
) -> Result<(Vec<u8>, Vec<u8>)> {
    let size = rgba_len(dimensions[0], dimensions[1])?;
    if pixels.len() != size {
        return Err(RendererError::BufferLength {
            expected: size,
            actual: pixels.len(),
        });
    }
    let stride = dimensions[0] as usize * 4;
    let mut deltas = Vec::with_capacity(size);
    if bottom_up {
        for row in pixels.chunks_exact(stride).rev() {
            deltas.extend_from_slice(row);
        }
    } else {
        deltas.extend_from_slice(pixels);
    }
    for index in (stride..size).rev() {
        deltas[index] = deltas[index].wrapping_sub(deltas[index - stride]);
    }
    for index in (4..stride).rev() {
        deltas[index] = deltas[index].wrapping_sub(deltas[index - 4]);
    }
    let padded = size.max(1024);
    let quarter = padded / 4;
    let mut planes = vec![0; padded];
    for (index, pixel) in deltas.chunks_exact(4).enumerate() {
        for (channel, &delta) in pixel.iter().enumerate() {
            let packed = if delta < 128 {
                delta << 1
            } else {
                (!delta << 1) | 1
            };
            for plane in 0..4 {
                planes[plane * quarter + index] |=
                    ((packed >> (6 - plane * 2)) & 3) << (channel * 2);
            }
        }
    }

    let mut writer = HgBitWriter::default();
    let mut copy = planes[0] != 0;
    writer.bit(copy);
    writer.gamma(padded);
    let mut literals = Vec::new();
    let mut start = 0;
    while start < padded {
        let count = planes[start..]
            .iter()
            .take_while(|&&value| (value != 0) == copy)
            .count();
        writer.gamma(count);
        if copy {
            literals.extend_from_slice(&planes[start..start + count]);
        }
        start += count;
        copy = !copy;
    }
    Ok((literals, writer.bytes))
}

#[derive(Default)]
struct HgBitWriter {
    bytes: Vec<u8>,
    position: usize,
}

impl HgBitWriter {
    fn bit(&mut self, value: bool) {
        if self.position.is_multiple_of(8) {
            self.bytes.push(0);
        }
        if value {
            *self.bytes.last_mut().unwrap() |= 1 << (self.position % 8);
        }
        self.position += 1;
    }

    fn gamma(&mut self, value: usize) {
        let highest = usize::BITS - value.leading_zeros() - 1;
        for _ in 0..highest {
            self.bit(false);
        }
        self.bit(true);
        for bit in (0..highest).rev() {
            self.bit(value & (1 << bit) != 0);
        }
    }
}

fn scaled_dimensions(width: u32, height: u32, options: &ImageDrawOptions) -> Result<(u32, u32)> {
    if !options.scale_x.is_finite() || !options.scale_y.is_finite() {
        return Err(RendererError::NonFiniteScale);
    }
    if options.scale_x == 0.0 || options.scale_y == 0.0 {
        return Err(RendererError::InvalidScale {
            scale_x_bits: options.scale_x.to_bits(),
            scale_y_bits: options.scale_y.to_bits(),
        });
    }
    let scaled_width = (f64::from(width) * f64::from(options.scale_x.abs())).ceil();
    let scaled_height = (f64::from(height) * f64::from(options.scale_y.abs())).ceil();
    if scaled_width < 1.0
        || scaled_height < 1.0
        || scaled_width > f64::from(u32::MAX)
        || scaled_height > f64::from(u32::MAX)
    {
        return Err(RendererError::ScaledDimensionsOverflow);
    }
    Ok((scaled_width as u32, scaled_height as u32))
}

fn validate_mask(mask: Option<&AlphaMask>, canvas: &Canvas) -> Result<()> {
    if let Some(mask) = mask
        && (mask.width != canvas.width || mask.height != canvas.height)
    {
        return Err(RendererError::MaskDimensions {
            mask_width: mask.width,
            mask_height: mask.height,
            canvas_width: canvas.width,
            canvas_height: canvas.height,
        });
    }
    Ok(())
}

fn point_in_rect(point: Point, rect: Rect) -> bool {
    i64::from(point.x) >= i64::from(rect.x)
        && i64::from(point.y) >= i64::from(rect.y)
        && i64::from(point.x) < rect.right()
        && i64::from(point.y) < rect.bottom()
}

fn apply_opacity_and_mask(
    source: Color,
    opacity: u8,
    mask: Option<&AlphaMask>,
    x: u32,
    y: u32,
) -> Color {
    let mut alpha = modulate_alpha(source.a, opacity);
    if let Some(mask) = mask {
        alpha = modulate_alpha(alpha, mask.alpha_unchecked(x, y));
    }
    source.with_alpha(alpha)
}

fn modulate_alpha(left: u8, right: u8) -> u8 {
    ((u16::from(left) * u16::from(right) + 127) / 255) as u8
}

fn blend_rgba(dst: Color, src: Color) -> Color {
    let source_alpha = u32::from(src.a);
    let destination_alpha = u32::from(dst.a);
    if source_alpha == 0 {
        return dst;
    }
    if source_alpha == 255 {
        return src;
    }

    let inverse_source_alpha = 255 - source_alpha;
    let output_alpha = source_alpha + ((destination_alpha * inverse_source_alpha + 127) / 255);
    if output_alpha == 0 {
        return Color::TRANSPARENT;
    }

    let channel = |source: u8, destination: u8| -> u8 {
        let numerator = u32::from(source) * source_alpha * 255
            + u32::from(destination) * destination_alpha * inverse_source_alpha;
        let denominator = output_alpha * 255;
        ((numerator + denominator / 2) / denominator) as u8
    };

    Color::rgba(
        channel(src.r, dst.r),
        channel(src.g, dst.g),
        channel(src.b, dst.b),
        output_alpha as u8,
    )
}

fn blend_with_mode(dst: Color, src: Color, mode: BlendMode) -> Color {
    let mut target = [dst.r, dst.g, dst.b, dst.a];
    blend_into(&mut target, src, mode);
    Color::rgba(target[0], target[1], target[2], target[3])
}

#[inline]
fn blend_into(target: &mut [u8; 4], src: Color, mode: BlendMode) {
    let mask = match mode {
        BlendMode::ListAlphaFromBlue => Some(src.b),
        BlendMode::ListAlphaFromGreen => Some(src.g),
        BlendMode::ListAlphaFromRed => Some(src.r),
        BlendMode::ListAlphaFromAlpha => Some(src.a),
        _ => None,
    };
    if let Some(mask) = mask {
        target[3] = (((u32::from(target[3]) + 1) * (u32::from(mask) + 1) - 1) >> 8) as u8;
        return;
    }
    let source = [src.r, src.g, src.b, src.a];
    if mode == BlendMode::SourceOver {
        blend_source_over(target, &source);
    } else {
        blend_special(target, &source, mode);
    }
}

#[inline]
fn blend_source_over(target: &mut [u8; 4], source: &[u8; 4]) {
    let alpha = u32::from(source[3]);
    if alpha == 0 {
        return;
    }
    if alpha == 255 || target[3] == 0 {
        target[..3].copy_from_slice(&source[..3]);
        target[3] = alpha as u8;
        return;
    }
    if target[3] == 255 {
        for (destination, value) in target[..3].iter_mut().zip(&source[..3]) {
            let difference = i32::from(*value) - i32::from(*destination);
            *destination = (i32::from(*destination) + ((difference * alpha as i32) >> 8)) as u8;
        }
        return;
    }

    let destination_alpha = u32::from(target[3]);
    let overlap = (alpha * destination_alpha) >> 8;
    let destination_weight = ((256 - alpha) * destination_alpha) >> 8;
    let source_weight = (((256 - destination_alpha) * alpha) >> 8) + overlap;
    let output_alpha = (source_weight + destination_weight) | 1;
    let reciprocal = 65536 / output_alpha;
    for (destination, value) in target[..3].iter_mut().zip(&source[..3]) {
        let sum = u32::from(*value) * source_weight + u32::from(*destination) * destination_weight;
        *destination = ((sum * reciprocal) >> 16).min(255) as u8;
    }
    target[3] = output_alpha as u8;
}

#[inline]
fn blend_special(target: &mut [u8; 4], source: &[u8; 4], mode: BlendMode) {
    if source[3] == 0 {
        return;
    }
    let alpha = u32::from(source[3]);
    match mode {
        BlendMode::AddSource => {
            blend_channels(target, source, |src, dst| src * alpha + dst * 255)
        }
        BlendMode::AddDestination => {
            blend_channels(target, source, |src, dst| src * 255 + dst * (255 - alpha))
        }
        BlendMode::Multiply => blend_channels(target, source, |src, dst| src * dst),
        BlendMode::InvertDestination => blend_channels(target, source, |src, dst| src * (255 - dst)),
        BlendMode::AddMultiply => blend_channels(target, source, |src, dst| dst * (src + 255)),
        _ => unreachable!(),
    }
}

#[inline]
fn blend_channels(target: &mut [u8; 4], source: &[u8; 4], numerator: impl Fn(u32, u32) -> u32) {
    for (destination, value) in target.iter_mut().zip(source) {
        *destination = ((numerator(u32::from(*value), u32::from(*destination)) + 127) / 255)
            .min(255) as u8;
    }
}

fn sample_frame(frame: &RgbaFrame, x: f64, y: f64, filter: ScaleFilter) -> Color {
    match filter {
        ScaleFilter::Nearest => frame.pixel_unchecked(
            (x as u32).min(frame.width - 1),
            (y as u32).min(frame.height - 1),
        ),
        ScaleFilter::Linear => sample_linear(frame, x - 0.5, y - 0.5),
    }
}

fn sample_linear(frame: &RgbaFrame, x: f64, y: f64) -> Color {
    let x0 = (x as u32).min(frame.width - 1);
    let y0 = (y as u32).min(frame.height - 1);
    let x1 = x0.saturating_add(1).min(frame.width - 1);
    let y1 = y0.saturating_add(1).min(frame.height - 1);
    let tx = (x - f64::from(x0)).clamp(0.0, 1.0);
    let ty = (y - f64::from(y0)).clamp(0.0, 1.0);
    let top_left = frame.pixel_unchecked(x0, y0);
    let top_right = frame.pixel_unchecked(x1, y0);
    let bottom_left = frame.pixel_unchecked(x0, y1);
    let bottom_right = frame.pixel_unchecked(x1, y1);
    let interpolate = |a: u8, b: u8, c: u8, d: u8| {
        let top = f64::from(a) * (1.0 - tx) + f64::from(b) * tx;
        let bottom = f64::from(c) * (1.0 - tx) + f64::from(d) * tx;
        (top * (1.0 - ty) + bottom * ty).round().clamp(0.0, 255.0) as u8
    };
    Color::rgba(
        interpolate(top_left.r, top_right.r, bottom_left.r, bottom_right.r),
        interpolate(top_left.g, top_right.g, bottom_left.g, bottom_right.g),
        interpolate(top_left.b, top_right.b, bottom_left.b, bottom_right.b),
        interpolate(top_left.a, top_right.a, bottom_left.a, bottom_right.a),
    )
}

fn frame_hash(width: u32, height: u32, pixels: &[u8]) -> u64 {
    let mut hash = FNV_OFFSET_BASIS;
    for byte in width.to_le_bytes().into_iter().chain(height.to_le_bytes()) {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    for &byte in pixels {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

fn read_u32(data: &[u8], offset: usize) -> Result<u32> {
    let bytes = data.get(offset..offset.saturating_add(4)).ok_or_else(|| {
        RendererError::InvalidHgStructure {
            message: format!("missing DWORD at 0x{offset:x}"),
        }
    })?;
    Ok(u32::from_le_bytes(bytes.try_into().unwrap()))
}

fn read_tag_name(data: &[u8], offset: usize) -> Result<[u8; 8]> {
    let bytes = data.get(offset..offset.saturating_add(8)).ok_or_else(|| {
        RendererError::InvalidHgStructure {
            message: format!("missing tag name at 0x{offset:x}"),
        }
    })?;
    Ok(bytes.try_into().unwrap())
}

fn hg_tag_kind(name: [u8; 8]) -> HgTagKind {
    let trimmed = &name[..name.iter().position(|&byte| byte == 0).unwrap_or(8)];
    if name == *b"stdinfo\0" {
        HgTagKind::StandardInfo
    } else if name == *b"img_jpg\0" {
        HgTagKind::Jpeg
    } else if name == *b"img_wbp\0" {
        HgTagKind::Wbp
    } else if name == *b"img_jpa\0" {
        HgTagKind::Jpa
    } else if name == *b"img_al\0\0" {
        HgTagKind::Alpha
    } else if trimmed == b"img0000" {
        HgTagKind::NativeImage
    } else if trimmed.starts_with(b"img")
        && trimmed.len() > 3
        && trimmed[3..].iter().all(u8::is_ascii_digit)
    {
        HgTagKind::ImageSlice
    } else if trimmed.starts_with(b"ats") {
        HgTagKind::Attribute
    } else {
        HgTagKind::Other
    }
}

fn inspect_hg2(data: &[u8], header_size: u32, version: u32) -> Result<HgInspection> {
    // Share the format reader's version, relative payload pointer and frame
    // boundary checks. Fixed header-size arithmetic would reject legal 0x10
    // input or inspect different bytes than the decoder for extended headers.
    let parsed = HgFile::parse(data).map_err(|error| RendererError::InvalidHgStructure {
        message: error.to_string(),
    })?;
    if parsed.frames.len() > MAX_HG_FRAMES {
        return Err(RendererError::InvalidHgStructure {
            message: "HG-2 frame count exceeds renderer limit".into(),
        });
    }
    let mut frames = Vec::with_capacity(parsed.frames.len());
    for frame in parsed.frames {
        if frame.width == 0 || frame.height == 0 {
            return Err(RendererError::InvalidHgStructure {
                message: "HG-2 frame dimensions are zero".into(),
            });
        }
        let image = frame.image.as_ref().ok_or_else(|| RendererError::InvalidHgStructure {
            message: "HG-2 frame has no native image".into(),
        })?;
        let payload = image.compressed_data_length
            .checked_add(image.compressed_command_length)
            .ok_or_else(|| RendererError::InvalidHgStructure {
                message: "HG-2 payload length overflowed".into(),
            })?;
        frames.push(HgFrameInfo {
            id: frame.id,
            offset: frame.frame_offset as usize,
            width: frame.width,
            height: frame.height,
            bit_depth: frame.bit_depth,
            image_offset: frame.offset,
            canvas_size: frame.canvas_size,
            alpha_flag: frame.is_transparent,
            base: frame.base,
            tags: vec![HgTagInfo {
                name: *b"img0000\0",
                kind: HgTagKind::NativeImage,
                offset: image.data_offset as usize,
                length: payload,
            }],
            attributes: BTreeMap::new(),
        });
    }
    Ok(HgInspection {
        container: match version as u16 {
            0x10 => HgContainer::Hg2V10,
            0x20 => HgContainer::Hg2V20,
            _ => HgContainer::Hg2V25,
        },
        header_size,
        version,
        frames,
    })
}

fn inspect_hg3(data: &[u8], header_size: u32, version: u32) -> Result<HgInspection> {
    if header_size != 12 || version != 0x300 {
        return Err(RendererError::InvalidHgStructure {
            message: "unsupported HG-3 header or version".into(),
        });
    }
    let parsed = HgFile::parse(data).map_err(|error| RendererError::InvalidHgStructure {
        message: error.to_string(),
    })?;
    let mut frames = Vec::new();
    let mut frame_offset = 12usize;
    let mut tag_count = 0usize;
    loop {
        if frames.len() == MAX_HG_FRAMES {
            return Err(RendererError::InvalidHgStructure {
                message: "HG-3 frame count exceeds renderer limit".into(),
            });
        }
        let frame_end_relative = read_u32(data, frame_offset)? as usize;
        let frame_end = if frame_end_relative == 0 {
            data.len()
        } else {
            frame_offset
                .checked_add(frame_end_relative)
                .ok_or_else(|| RendererError::InvalidHgStructure {
                    message: "HG-3 frame offset overflowed".into(),
                })?
        };
        if frame_end <= frame_offset + 8 || frame_end > data.len() {
            return Err(RendererError::InvalidHgStructure {
                message: format!("invalid HG-3 frame boundary at 0x{frame_offset:x}"),
            });
        }
        let id = read_u32(data, frame_offset + 4)?;
        let mut tags = Vec::new();
        let mut tag_offset = frame_offset + 8;
        let mut standard = None;
        let mut native_count = 0usize;
        let mut attributes = BTreeMap::new();
        loop {
            tag_count = tag_count.saturating_add(1);
            if tag_count > MAX_HG_TAGS {
                return Err(RendererError::InvalidHgStructure {
                    message: "HG-3 tag count exceeds renderer limit".into(),
                });
            }
            if tag_offset + 16 > frame_end {
                return Err(RendererError::InvalidHgStructure {
                    message: "HG-3 tag header exceeds frame boundary".into(),
                });
            }
            let name = read_tag_name(data, tag_offset)?;
            let next = read_u32(data, tag_offset + 8)? as usize;
            let length = read_u32(data, tag_offset + 12)?;
            let payload_start = tag_offset + 16;
            let payload_end = payload_start.checked_add(length as usize).ok_or_else(|| {
                RendererError::InvalidHgStructure {
                    message: "HG-3 tag payload length overflowed".into(),
                }
            })?;
            if payload_end > frame_end || payload_end > data.len() {
                return Err(RendererError::InvalidHgStructure {
                    message: format!(
                        "HG-3 tag payload at 0x{tag_offset:x} ends at 0x{payload_end:x}, outside frame end 0x{frame_end:x} or file length 0x{:x}",
                        data.len()
                    ),
                });
            }
            let kind = hg_tag_kind(name);
            if kind == HgTagKind::StandardInfo && standard.is_none() {
                if length < 40 {
                    return Err(RendererError::InvalidHgStructure {
                        message: "HG-3 stdinfo payload is shorter than 40 bytes".into(),
                    });
                }
                standard = Some((
                    read_u32(data, payload_start)?,
                    read_u32(data, payload_start + 4)?,
                    read_u32(data, payload_start + 8)?,
                    [
                        read_u32(data, payload_start + 12)? as i32,
                        read_u32(data, payload_start + 16)? as i32,
                    ],
                    [
                        read_u32(data, payload_start + 20)?,
                        read_u32(data, payload_start + 24)?,
                    ],
                    read_u32(data, payload_start + 28)?,
                    [
                        read_u32(data, payload_start + 32)? as i32,
                        read_u32(data, payload_start + 36)? as i32,
                    ],
                ));
            }
            if matches!(kind, HgTagKind::NativeImage | HgTagKind::ImageSlice) {
                native_count += 1;
            }
            if kind == HgTagKind::Attribute {
                if let Some(id) = cs2_formats::hg::attribute_tag_id(
                    &data[tag_offset..payload_start],
                ) {
                    if length < 20 {
                        return Err(RendererError::InvalidHgStructure {
                            message: "HG-3 attribute payload is shorter than 20 bytes".into(),
                        });
                    }
                    let mut values = [0_u32; 5];
                    for (index, value) in values.iter_mut().enumerate() {
                        *value = read_u32(data, payload_start + index * 4)?;
                    }
                    attributes.insert(id, values);
                }
            }
            tags.push(HgTagInfo {
                name,
                kind,
                offset: tag_offset,
                length,
            });
            if next == 0 {
                break;
            }
            if next < 16 + length as usize
                || tag_offset
                    .checked_add(next)
                    .is_none_or(|next| next >= frame_end)
            {
                return Err(RendererError::InvalidHgStructure {
                    message: "HG-3 tag offset overlaps or escapes frame".into(),
                });
            }
            tag_offset += next;
        }
        let Some((width, height, bit_depth, image_offset, canvas_size, alpha_flag, base)) =
            standard
        else {
            return Err(RendererError::InvalidHgStructure {
                message: "HG-3 frame has no stdinfo tag".into(),
            });
        };
        if width == 0 || height == 0 {
            return Err(RendererError::InvalidHgStructure {
                message: "HG-3 frame dimensions are zero".into(),
            });
        }
        if native_count != 0 {
            let frame = parsed.frames.get(frames.len()).ok_or_else(|| {
                RendererError::InvalidHgStructure {
                    message: "HG-3 frame metadata disagrees with format parser".into(),
                }
            })?;
            cs2_formats::hg::native_image_slices(frame, data)
                .map_err(|error| RendererError::InvalidHgStructure { message: error.to_string() })?;
        }
        frames.push(HgFrameInfo {
            id,
            offset: frame_offset,
            width,
            height,
            bit_depth,
            image_offset,
            canvas_size,
            alpha_flag,
            base,
            tags,
            attributes,
        });
        if frame_end_relative == 0 {
            break;
        }
        frame_offset = frame_end;
    }
    Ok(HgInspection {
        container: HgContainer::Hg3V300,
        header_size,
        version,
        frames,
    })
}

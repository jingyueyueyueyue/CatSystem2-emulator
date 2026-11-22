//! Partial type54 E-mote bridge, independently derived from NEKOPARA bytes.
//!
//! PSZ decoding and a bounded PSB/CPU backend are implemented in sibling modules.
//! The original controller/manager ABI remains partial; strict drawing rejects
//! unsupported features. No blank-frame or constant-success backend exists.
//! Evidence: reports/recovery_A_20261116_emote.json.

use std::{collections::BTreeMap, fmt, fs, sync::{Arc, OnceLock}, time::SystemTime};

use flate2::{Decompress, FlushDecompress, Status};

use crate::{ResourceCandidate, ResourceLocation};

#[path = "emote_clamp.rs"]
mod clamp;
#[path = "emote_control.rs"]
mod control;
#[path = "emote_cpu.rs"]
mod cpu;
#[path = "emote_dispatch.rs"]
mod dispatch;
#[path = "emote_driver_transform.rs"]
mod driver_transform;
#[path = "emote_host.rs"]
mod host;
#[path = "emote_m2action.rs"]
mod m2action;
#[path = "emote_m2banks.rs"]
mod m2banks;
#[path = "emote_m2chain.rs"]
mod m2chain;
#[path = "emote_m2config.rs"]
mod m2config;
#[path = "emote_m2force.rs"]
mod m2force;
#[path = "emote_m2graph.rs"]
mod m2graph;
#[path = "emote_m2lip.rs"]
mod m2lip;
#[path = "emote_m2live.rs"]
mod m2live;
#[path = "emote_m2physics.rs"]
mod m2physics;
#[path = "emote_m2presets.rs"]
mod m2presets;
#[path = "emote_m2selector.rs"]
mod m2selector;
#[path = "emote_m2settings.rs"]
mod m2settings;
#[path = "emote_m2spline.rs"]
mod m2spline;
#[path = "emote_m2timed.rs"]
mod m2timed;
#[path = "emote_m2values.rs"]
mod m2values;
#[path = "emote_m2wave.rs"]
mod m2wave;
#[path = "emote_mesh.rs"]
mod mesh;
#[path = "emote_psb.rs"]
mod psb;
#[path = "emote_stencil.rs"]
mod stencil;
#[path = "emote_surface.rs"]
mod surface;
pub use surface::{EmoteSurfaceExpand, EmoteSurfaceMixer, EmoteSurfacePrepare};
#[path = "emote_texture.rs"]
mod texture;
#[path = "emote_timeline.rs"]
mod timeline;
#[path = "emote_variable_timeline.rs"]
mod variable_timeline;
pub use control::{
    CpuNative907PhaseBank, CpuNative907PhaseReset, CpuNative907State, CpuNative907TransitionReset,
    CpuNative907TransitionState, CpuNative907VariableBank, CpuNative907VariableReset,
    CpuSceneEnableOverride,
};
pub use cpu::{
    CpuEmoteBackend, CpuEmoteResources, CpuMotionModel, CpuPlayer, CpuTexture, CpuViewport,
    DiagnosticFrame, EmoteDrawDiagnostics, EmoteDrawTarget, EmoteSpriteMesh, EmoteSpritePass,
    EmoteRepairScratch, EmoteTextureSpan, OwnedEmoteSpriteMesh, decode_rgba_rle, rasterize_emote_stencil_triangle, repair_emote_raster,
    visit_emote_texture_spans,
};
pub use dispatch::{
    EMOTE_NATIVE_DESCRIPTORS, EmoteBatchValue, EmoteCommandSelector, EmoteDispatchShape,
    EmoteNativeDescriptor, EmoteOperation, EmotePacking, EmoteReturnKind, emote_native_descriptor,
    m2plane_tint_argb,
};
pub use driver_transform::CpuDriverTransform;
pub use host::{
    CpuD3d9Projection, CpuOriginalDisplayMapping, CpuOriginalTaskClock,
    NEKO_ORIGINAL_TASK_UNITS_PER_FRAME,
};
pub use m2banks::{
    CpuM2BlinkDefinition, CpuM2ControllerConfig, CpuM2LipDefinition, CpuM2LipFlowStep,
    CpuM2ResetBankSeeds,
};
pub use m2action::CpuM2ActionConfig;
pub use m2chain::{CpuM2ChainKind, CpuM2ChainSelection, CpuM2PatternChains};
pub use m2config::{CpuM2Config, CpuM2DifferenceTransform, CpuM2LoadSpec, CpuM2ProfileSelection};
pub use m2lip::{
    CpuM2LipBinding, CpuM2LipCurve, CpuM2LipPreset, CpuM2LipSidecarKind, CpuM2Lpd, CpuM2LpdRecord,
};
pub use m2live::{CpuM2LiveOwner, CpuM2Load, CpuM2OwnerInfo};
pub use m2physics::CpuM2Material;
pub use m2settings::CpuM2MaterialSetting;
pub use m2spline::CpuM2SplineConfig;
pub use m2selector::{
    CpuEmoteImmediateTarget, emote_control_integer, emote_immediate_target,
    evaluate_emote_immediate_controls, evaluate_emote_immediate_selectors,
};
pub use m2timed::{CpuEmoteControllerTarget, CpuM2TimedRequest, CpuM2TimedVariables};
pub use m2wave::CpuM2Waveform;
pub use mesh::{CpuBezierPatch, CpuMeshGrid};
pub use psb::{PsbDocument, PsbValue};
pub use stencil::{
    CpuStencilBinding, CpuStencilBuffer, CpuStencilWrite, resolve_emote_stencil_bindings,
};
pub use texture::decode_emote_texture;
pub use timeline::{
    CpuTimelineSample, emote_layer_affine, interpolate_emote_angle, interpolate_emote_color,
    sample_emote_layer,
};
pub use variable_timeline::{
    CpuVariableTimelineInfo, CpuVariableTimelines, emote_variable_easing_exponent,
};

pub const NEKOPARA_PE_SHA256: &str =
    "c71c5b85e6eca660474f156d96b0021a53a5f05690dd077d7861d72ff5b11adc";
pub const M2PLANE_TASK_TYPE: u32 = 54;
pub const MAX_PSB_BYTES: usize = 256 * 1024 * 1024;
pub const MAX_FRAME_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_EMOTE_TASKS: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmoteError {
    UnknownProfile,
    UnknownTask(u32),
    WrongTaskType(u32),
    DuplicateTask(u32),
    Capacity,
    InvalidResource(&'static str),
    Decompression,
    EncryptedPsbHeader { version: u16, flags: u16 },
    MissingBackend,
    Unsupported { native: u32, reason: &'static str },
    Backend(String),
    InvalidReply { native: u32 },
    InvalidFrame,
}

impl fmt::Display for EmoteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownProfile => write!(f, "E-mote requires the exact identified PE profile"),
            Self::UnknownTask(id) => write!(f, "E-mote task {id} is not registered"),
            Self::WrongTaskType(kind) => write!(f, "E-mote requires task type54, received {kind}"),
            Self::DuplicateTask(id) => write!(f, "E-mote task {id} is already registered"),
            Self::Capacity => write!(f, "E-mote resource/state capacity exceeded"),
            Self::InvalidResource(why) => write!(f, "invalid E-mote resource: {why}"),
            Self::Decompression => write!(f, "PSZ zlib stream or declared lengths are invalid"),
            Self::EncryptedPsbHeader { version, flags } => write!(
                f,
                "PSB v{version} header flags {flags:#x} require a verified decryptor; no timeline or pixels were decoded"
            ),
            Self::MissingBackend => write!(
                f,
                "E-mote PSB timeline/geometry/pixel backend is not connected"
            ),
            Self::Unsupported { native, reason } => {
                write!(f, "E-mote native {native} requires recovery: {reason}")
            }
            Self::Backend(why) => write!(f, "E-mote backend: {why}"),
            Self::InvalidReply { native } => {
                write!(
                    f,
                    "E-mote backend returned the wrong ABI result for native {native}"
                )
            }
            Self::InvalidFrame => write!(f, "E-mote backend did not provide a valid RGBA frame"),
        }
    }
}

impl std::error::Error for EmoteError {}

pub type Result<T> = std::result::Result<T, EmoteError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PsbHeader {
    pub version: u16,
    pub flags: u16,
}

#[derive(Debug, Clone)]
pub struct EmoteResource {
    pub header: PsbHeader,
    pub psz_reserved: Option<u32>,
    bytes: Vec<u8>,
}

/// Identity of the archive payload which won resource routing, together with
/// the filesystem version observed around its successful decode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmoteArchiveVersion {
    location: ResourceLocation,
    len: u64,
    modified: SystemTime,
    created: Option<SystemTime>,
    #[cfg(unix)]
    inode: (u64, u64, i64, i64),
}

impl EmoteArchiveVersion {
    pub fn observe(candidate: &ResourceCandidate) -> Option<Self> {
        let ResourceLocation::Archive { path, offset, size, .. } = &candidate.location else {
            return None;
        };
        let metadata = fs::metadata(path).ok()?;
        if !metadata.is_file() || offset.checked_add(*size)? > metadata.len() {
            return None;
        }
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        Some(Self {
            location: candidate.location.clone(),
            len: metadata.len(),
            modified: metadata.modified().ok()?,
            created: metadata.created().ok(),
            #[cfg(unix)]
            inode: (metadata.dev(), metadata.ino(), metadata.ctime(), metadata.ctime_nsec()),
        })
    }
}

pub enum CpuM2ModelSource<'a> {
    Decoded(&'a EmoteResource, Option<EmoteArchiveVersion>),
    Shared(Arc<CpuMotionModel>, EmoteArchiveVersion),
}

impl EmoteResource {
    /// Decode the original sixteen-byte PSZ envelope, or accept raw PSB bytes.
    /// PSB metadata is identified only; header encryption is not bypassed.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let (bytes, psz_reserved) = if input.starts_with(b"PSZ\0") {
            if input.len() < 16 {
                return Err(EmoteError::InvalidResource("truncated PSZ header"));
            }
            let compressed = word(input, 4)? as usize;
            let expanded = word(input, 8)? as usize;
            if compressed.checked_add(16) != Some(input.len()) || expanded < 8 {
                return Err(EmoteError::InvalidResource("PSZ length fields"));
            }
            if expanded > MAX_PSB_BYTES {
                return Err(EmoteError::Capacity);
            }
            // One extra byte distinguishes an understated expanded length from
            // a stream that merely filled the caller's buffer.
            let capacity = expanded.checked_add(1).ok_or(EmoteError::Capacity)?;
            let mut output = Vec::new();
            output
                .try_reserve_exact(capacity)
                .map_err(|_| EmoteError::Capacity)?;
            output.resize(capacity, 0);
            let mut decoder = Decompress::new(true);
            let status = decoder
                .decompress(&input[16..], &mut output, FlushDecompress::Finish)
                .map_err(|_| EmoteError::Decompression)?;
            if status != Status::StreamEnd
                || decoder.total_in() != compressed as u64
                || decoder.total_out() != expanded as u64
            {
                return Err(EmoteError::Decompression);
            }
            output.truncate(expanded);
            (output, Some(word(input, 12)?))
        } else {
            if input.len() > MAX_PSB_BYTES {
                return Err(EmoteError::Capacity);
            }
            let mut output = Vec::new();
            output
                .try_reserve_exact(input.len())
                .map_err(|_| EmoteError::Capacity)?;
            output.extend_from_slice(input);
            (output, None)
        };
        if bytes.len() < 8 || &bytes[..4] != b"PSB\0" {
            return Err(EmoteError::InvalidResource("decoded body is not PSB"));
        }
        let header = PsbHeader {
            version: u16::from_le_bytes([bytes[4], bytes[5]]),
            flags: u16::from_le_bytes([bytes[6], bytes[7]]),
        };
        Ok(Self {
            header,
            psz_reserved,
            bytes,
        })
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// A plain header still requires complete PSB object/timeline validation.
    pub fn require_plaintext_header(&self) -> Result<()> {
        if self.header.flags != 0 {
            return Err(EmoteError::EncryptedPsbHeader {
                version: self.header.version,
                flags: self.header.flags,
            });
        }
        Ok(())
    }
}

fn word(bytes: &[u8], offset: usize) -> Result<u32> {
    let end = offset.checked_add(4).ok_or(EmoteError::Capacity)?;
    let value = bytes
        .get(offset..end)
        .ok_or(EmoteError::InvalidResource("truncated DWORD"))?;
    Ok(u32::from_le_bytes([value[0], value[1], value[2], value[3]]))
}

/// Exact command-buffer layout, with no process pointer encoded in a DWORD.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmoteBuffer {
    None,
    Bytes(Vec<u8>),
    FloatBits(u32),
    ThreeWords([u32; 3]),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmoteCommand {
    pub native: u32,
    pub command: u32,
    pub data0: u32,
    pub data1: u32,
    pub buffer0: EmoteBuffer,
    pub buffer1: EmoteBuffer,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmoteReply {
    Continue,
    Value(u32),
    ThreeWords([u32; 3]),
    /// Owned byte string, without a terminating NUL. The host writes it to
    /// the caller's actual output storage; no VM/process pointer is retained.
    ByteString(Vec<u8>),
    /// Descriptor columns are independent: None preserves that destination.
    /// Native1030 returns whether the first column exists.
    DescriptorColumns([Option<Vec<u8>>; 2]),
}

/// All time values remain host-supplied raw values. This module does not invent
/// seconds/frames conversion, looping rules or timeline interpolation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmoteTick {
    pub task_elapsed: u32,
    /// Original173d30's second clock, sourced from NEKO sound219.
    /// It is independent of the M2Plane task's own clock.
    pub lip_elapsed: Option<u32>,
    pub increment: u32,
}

/// Read-only integration status. A loaded resource does not imply that every
/// controller or rendering feature in that resource is implemented.
#[derive(Debug, Clone, PartialEq)]
pub struct EmotePlaybackInfo {
    pub resource_name: Vec<u8>,
    pub resource_generation: u64,
    pub default_object: String,
    pub default_motion: String,
    pub object: String,
    pub motion: String,
    pub frame: f64,
    pub native907_banks_bound: bool,
    pub original_task_clock: bool,
    pub original_clock_fields: [u32; 2],
    pub active_m2_timed_variables: usize,
    pub active_timeline_labels: Vec<String>,
    pub m2_owner: Option<CpuM2OwnerInfo>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmoteServiceStatus {
    pub identified_profile: bool,
    pub backend_attached: bool,
    pub live_tasks: usize,
}

/// A device-backed pose uses the same dimensions as its CPU materialization.
pub trait EmoteDeviceReadback: std::any::Any + fmt::Debug + Send + Sync {
    fn read_rgba(&self, output: &mut [u8]) -> bool;
    fn as_any(&self) -> &dyn std::any::Any;
}

/// Actual evaluated and rasterized pixels. There is no default/empty frame.
#[derive(Debug, Clone)]
pub struct EmoteRasterFrame {
    width: u32,
    height: u32,
    origin: [i32; 2],
    pixels: Arc<EmotePixels>,
}

#[derive(Debug, Clone)]
struct EmotePixels {
    rgba: OnceLock<Vec<u8>>,
    device: Option<Arc<dyn EmoteDeviceReadback>>,
    alpha_bounds: OnceLock<Option<[u32; 4]>>,
}

impl EmoteRasterFrame {
    fn frame_bytes(width: u32, height: u32) -> Result<usize> {
        (width as usize)
            .checked_mul(height as usize)
            .and_then(|n| n.checked_mul(4))
            .filter(|&n| n <= MAX_FRAME_BYTES && width != 0 && height != 0)
            .ok_or(EmoteError::InvalidFrame)
    }

    pub fn new(width: u32, height: u32, origin: [i32; 2], rgba: Vec<u8>) -> Result<Self> {
        let size = Self::frame_bytes(width, height)?;
        if rgba.len() != size {
            return Err(EmoteError::InvalidFrame);
        }
        Ok(Self {
            width,
            height,
            origin,
            pixels: Arc::new(EmotePixels {
                rgba: OnceLock::from(rgba), device: None, alpha_bounds: OnceLock::new(),
            }),
        })
    }

    pub fn from_device(
        width: u32, height: u32, origin: [i32; 2], device: Arc<dyn EmoteDeviceReadback>,
    ) -> Result<Self> {
        Self::frame_bytes(width, height)?;
        Ok(Self {
            width, height, origin,
            pixels: Arc::new(EmotePixels {
                rgba: OnceLock::new(), device: Some(device), alpha_bounds: OnceLock::new(),
            }),
        })
    }

    pub fn width(&self) -> u32 {
        self.width
    }
    pub fn height(&self) -> u32 {
        self.height
    }
    pub fn origin(&self) -> [i32; 2] {
        self.origin
    }
    pub fn device(&self) -> Option<&dyn EmoteDeviceReadback> {
        self.pixels.device.as_deref()
    }
    pub fn shares_pixels(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.pixels, &other.pixels)
    }
    pub fn rgba(&self) -> &[u8] {
        self.pixels.rgba.get_or_init(|| {
            let size = Self::frame_bytes(self.width, self.height).unwrap();
            let mut rgba = vec![0; size];
            if !self.pixels.device.as_ref().is_some_and(|device| device.read_rgba(&mut rgba)) {
                panic!("E-mote device readback failed; rebuild the pose instead of using stale pixels");
            }
            rgba
        })
    }

    /// Return the original frame when possible, otherwise clip and pad it to a full viewport.
    pub fn into_viewport(self, size: [u32; 2]) -> Result<Self> {
        if self.origin == [0, 0] && self.width == size[0] && self.height == size[1] {
            return Ok(self);
        }
        let byte_count = (size[0] as usize)
            .checked_mul(size[1] as usize)
            .and_then(|n| n.checked_mul(4))
            .filter(|&n| n <= MAX_FRAME_BYTES)
            .ok_or(EmoteError::InvalidFrame)?;
        if size.contains(&0) {
            return Err(EmoteError::InvalidFrame);
        }
        let origin = self.origin.map(i64::from);
        let left = origin[0].max(0);
        let top = origin[1].max(0);
        let right = (origin[0] + i64::from(self.width)).min(i64::from(size[0]));
        let bottom = (origin[1] + i64::from(self.height)).min(i64::from(size[1]));
        let mut rgba = vec![0; byte_count];
        if right > left && bottom > top {
            let source_x = (left - origin[0]) as usize;
            let source_y = (top - origin[1]) as usize;
            let source_stride = self.width as usize;
            let target_stride = size[0] as usize;
            let row_bytes = (right - left) as usize * 4;
            for row in 0..(bottom - top) as usize {
                let source = ((source_y + row) * source_stride + source_x) * 4;
                let target = ((top as usize + row) * target_stride + left as usize) * 4;
                rgba[target..target + row_bytes]
                    .copy_from_slice(&self.rgba()[source..source + row_bytes]);
            }
        }
        Self::new(size[0], size[1], [0, 0], rgba)
    }

    fn rgba_mut(&mut self) -> &mut [u8] {
        self.rgba();
        let pixels = Arc::make_mut(&mut self.pixels);
        pixels.device = None;
        pixels.alpha_bounds.take();
        pixels.rgba.get_mut().unwrap()
    }

    fn try_recycle(self) -> Option<Vec<u8>> {
        Arc::try_unwrap(self.pixels).ok().and_then(|pixels| pixels.rgba.into_inner())
    }

    /// Nonzero-alpha bounds in local pixels, with exclusive right and bottom.
    pub fn alpha_bounds(&self) -> Option<[u32; 4]> {
        *self.pixels.alpha_bounds.get_or_init(|| {
            let mut bounds = [self.width, self.height, 0, 0];
            for (row, pixels) in self.rgba().chunks_exact(self.width as usize * 4).enumerate() {
                let mut pixels = pixels.chunks_exact(4);
                if let Some(first) = pixels.position(|pixel| pixel[3] != 0) {
                    let last = pixels.rposition(|pixel| pixel[3] != 0)
                        .map_or(first, |remaining| first + 1 + remaining);
                    bounds[0] = bounds[0].min(first as u32);
                    bounds[1] = bounds[1].min(row as u32);
                    bounds[2] = bounds[2].max(last as u32 + 1);
                    bounds[3] = row as u32 + 1;
                }
            }
            (bounds[0] < bounds[2]).then_some(bounds)
        })
    }
}

/// The backend must own a real evaluator/resource decoder and renderer.
/// No default success or state-only implementation is provided.
pub trait EmoteBackend: Send {
    fn set_m2_force_position(&mut self, _task: u32, _position: [f32; 2], _linked: bool) -> Result<()> {
        Err(EmoteError::Unsupported { native: 84, reason: "backend has no M2 force position receiver" })
    }
    fn set_m2_material_group(&mut self, _setting: &CpuM2MaterialSetting) -> Result<()> {
        Err(EmoteError::Unsupported { native: 906, reason: "backend has no live M2 material groups" })
    }
    fn recompute_m2_controls(&mut self, _settings: &[CpuM2MaterialSetting]) -> Result<()> {
        Err(EmoteError::Unsupported { native: 932, reason: "backend has no M2 manager recomputation" })
    }
    fn debug_m2_text_cp932(&self) -> Result<Vec<u8>> {
        Err(EmoteError::Unsupported { native: 933, reason: "backend has no live M2 bank text snapshot" })
    }
    fn m2_action_busy(&self, _task: u32) -> Result<bool> {
        Err(EmoteError::Unsupported { native: 1028, reason: "backend has no M2 action owner query" })
    }
    fn create_task(&mut self, task: u32) -> Result<()>;
    fn release_task(&mut self, task: u32) -> Result<()>;
    fn import_resource(&mut self, task: u32, name: &[u8], resource: &EmoteResource) -> Result<()>;
    fn import_m2_resource(
        &mut self,
        _task: u32,
        _name: &[u8],
        _source: CpuM2ModelSource<'_>,
        _load: CpuM2Load,
    ) -> Result<()> {
        Err(EmoteError::Unsupported {
            native: 906,
            reason: "backend has no M2 manager receiver",
        })
    }
    fn cached_m2_model(
        &self, _task: u32, _name: &[u8], _version: &EmoteArchiveVersion,
    ) -> Option<Arc<CpuMotionModel>> {
        None
    }
    fn command(&mut self, task: u32, command: &EmoteCommand) -> Result<EmoteReply>;
    fn advance(&mut self, task: u32, tick: EmoteTick) -> Result<()>;
    fn advance_with_random(
        &mut self,
        task: u32,
        tick: EmoteTick,
        _random: &mut dyn FnMut() -> u32,
    ) -> Result<()> {
        self.advance(task, tick)
    }
    fn render_rgba(&mut self, task: u32) -> Result<EmoteRasterFrame>;
    fn scene_raster_player(&self, _task: u32) -> Option<(&CpuPlayer, CpuViewport)> {
        None
    }
    fn can_defer_scene_raster(&self, _task: u32) -> bool {
        false
    }
    fn bind_m2_lip_curve(
        &mut self,
        _task: u32,
        _dimension: u32,
        _pattern: &[u8],
        _curve: CpuM2LipCurve,
    ) -> Result<()> {
        Err(EmoteError::Unsupported {
            native: 924,
            reason: "backend has no live M2 lip evaluator",
        })
    }
    /// Actual owned sidecar bytes and recovered source origin/presets. This
    /// call cannot manufacture a time mapping from a channel number.
    fn bind_m2_lpd(
        &mut self,
        _task: u32,
        _bytes: &[u8],
        _origin_tick: u32,
        _bindings: &[CpuM2LipBinding],
    ) -> Result<usize> {
        Err(EmoteError::Unsupported {
            native: 924,
            reason: "backend has no M2 LPD batch binding",
        })
    }
    fn bind_m2_lip_sidecar(
        &mut self,
        task: u32,
        bytes: &[u8],
        kind: CpuM2LipSidecarKind,
        origin_tick: u32,
        bindings: &[CpuM2LipBinding],
    ) -> Result<usize> {
        match kind {
            CpuM2LipSidecarKind::Lpd => self.bind_m2_lpd(task, bytes, origin_tick, bindings),
            CpuM2LipSidecarKind::LegacyLv => Err(EmoteError::Unsupported {
                native: 924,
                reason: "backend has no actual legacy LV evaluator",
            }),
        }
    }
    fn bind_m2_profile_lip_sidecar(
        &mut self,
        _task: u32,
        _bytes: &[u8],
        _kind: CpuM2LipSidecarKind,
    ) -> Result<usize> {
        Err(EmoteError::Unsupported {
            native: 924,
            reason: "backend has no M2 profile sidecar receiver",
        })
    }
    fn update_m2_waveform(
        &mut self,
        _task: u32,
        _samples: &[u32],
        _original_extent: u32,
    ) -> Result<usize> {
        Err(EmoteError::Unsupported {
            native: 924,
            reason: "backend has no actual M2 waveform window receiver",
        })
    }
    fn control_m2_waveform(&mut self, _task: u32, _immediate_stop: bool) -> Result<()> {
        Err(EmoteError::Unsupported {
            native: 924,
            reason: "backend has no actual M2 waveform finish receiver",
        })
    }
    fn playback_info(&self, _task: u32) -> Result<EmotePlaybackInfo> {
        Err(EmoteError::Backend(
            "backend does not expose playback inspection".into(),
        ))
    }
    fn set_viewport(&mut self, _viewport: CpuViewport) -> Result<()> {
        Err(EmoteError::Backend(
            "backend does not accept an explicit viewport".into(),
        ))
    }
    fn play_timeline(&mut self, _task: u32, _label: &str, _flags: u32) -> Result<()> {
        Err(EmoteError::Backend(
            "backend does not evaluate metadata.timelineControl".into(),
        ))
    }
    fn stop_timeline(&mut self, _task: u32, _label: &str) -> Result<()> {
        Err(EmoteError::Backend(
            "backend does not own metadata.timelineControl".into(),
        ))
    }
    fn bind_native907_state(&mut self, _task: u32, _state: CpuNative907State) -> Result<()> {
        Err(EmoteError::Unsupported {
            native: 907,
            reason: "backend cannot own the original controller reset banks",
        })
    }
    fn set_root_affine(&mut self, _task: u32, _affine: [f64; 6]) -> Result<()> {
        Err(EmoteError::Backend(
            "backend cannot accept a resolved host object transform".into(),
        ))
    }
    fn set_driver_transform(&mut self, _task: u32, _transform: CpuDriverTransform) -> Result<()> {
        Err(EmoteError::Backend("backend does not accept M2 root property overrides".into()))
    }
    fn set_surface_fade_default(&mut self, _task: u32, _duration: u32) -> Result<()> {
        Err(EmoteError::Backend("backend does not accept M2 surface ownership".into()))
    }
    fn surface_holds_tick(&self, _task: u32, _now: u32) -> Result<bool> {
        Err(EmoteError::Backend("backend does not expose M2 surface ownership".into()))
    }
    /// Original M2Plane+388 is ARGB, defaultffffffff in constructor237cf0.
    /// Hosts propagate actual task color here; this is separate from native919.
    fn set_base_argb(&mut self, _task: u32, _argb: u32) -> Result<()> {
        Err(EmoteError::Backend(
            "backend does not accept the original M2Plane base ARGB".into(),
        ))
    }
}

#[derive(Debug, Clone, Default)]
pub struct EmoteTaskState {
    pub requested_enable_raw: Option<u32>,
    /// Accepted backend values; unset does not imply a guessed native default.
    pub controller_a_raw: Option<u32>,
    pub controller_b_raw: Option<u32>,
    pub property_134_raw: Option<u32>,
    pub property_a0_raw: Option<u32>,
    pub color_percentages_raw: Option<[u32; 3]>,
    pub last_completed_tick: Option<EmoteTick>,
    pub successful_resource_imports: u32,
    pub successful_pixel_frames: u64,
}

#[derive(Default)]
pub struct EmoteServices {
    bound: bool,
    tasks: BTreeMap<u32, EmoteTaskState>,
    backend: Option<Box<dyn EmoteBackend>>,
}

impl EmoteServices {
    pub fn status(&self) -> EmoteServiceStatus {
        EmoteServiceStatus {
            identified_profile: self.bound,
            backend_attached: self.backend.is_some(),
            live_tasks: self.tasks.len(),
        }
    }

    pub fn task_ids(&self) -> impl Iterator<Item = u32> + '_ {
        self.tasks.keys().copied()
    }

    pub fn backend_ready(&self) -> bool {
        self.bound && self.backend.is_some()
    }

    pub fn bind_pe_sha256(&mut self, sha256: &str) -> bool {
        let matched = NEKOPARA_PE_SHA256.eq_ignore_ascii_case(sha256);
        // Binding is initialization only. Do not silently transplant live
        // objects or a backend from one game into another profile.
        if !matched {
            self.bound = false;
        } else if self.tasks.is_empty() {
            self.bound = true;
        }
        self.bound && matched
    }

    pub fn set_backend(&mut self, backend: Box<dyn EmoteBackend>) -> Result<()> {
        if !self.tasks.is_empty() {
            return Err(EmoteError::Backend(
                "cannot replace backend while tasks are alive".into(),
            ));
        }
        self.backend = Some(backend);
        Ok(())
    }

    pub fn register_task(&mut self, task: u32, actual_type: u32) -> Result<()> {
        self.require_profile()?;
        if actual_type != M2PLANE_TASK_TYPE {
            return Err(EmoteError::WrongTaskType(actual_type));
        }
        if self.tasks.contains_key(&task) {
            return Err(EmoteError::DuplicateTask(task));
        }
        if self.tasks.len() >= MAX_EMOTE_TASKS {
            return Err(EmoteError::Capacity);
        }
        // Creating an object is separate from loading a PSB. There can be no
        // successful native dispatch/draw until a backend is attached.
        let backend = self.backend.as_mut().ok_or(EmoteError::MissingBackend)?;
        backend.create_task(task)?;
        self.tasks.insert(task, EmoteTaskState::default());
        Ok(())
    }

    pub fn release_task(&mut self, task: u32) -> Result<()> {
        // Destruction remains available after a failed profile bind. Only
        // dispatch/load/draw require a matching running executable profile.
        if !self.tasks.contains_key(&task) {
            return Err(EmoteError::UnknownTask(task));
        }
        self.backend
            .as_mut()
            .ok_or(EmoteError::MissingBackend)?
            .release_task(task)?;
        self.tasks.remove(&task);
        Ok(())
    }

    /// Release in stable ID order. On a backend error, already released tasks
    /// stay released and the failed/unvisited tasks remain owned for retry.
    pub fn release_all(&mut self) -> Result<()> {
        let tasks: Vec<_> = self.task_ids().collect();
        for task in tasks {
            self.release_task(task)?;
        }
        Ok(())
    }

    /// Game/session teardown. Drops the archive provider with its backend only
    /// after all task destruction succeeds. Safe after a profile mismatch.
    pub fn shutdown(&mut self) -> Result<()> {
        self.release_all()?;
        self.backend = None;
        self.bound = false;
        Ok(())
    }

    pub fn state(&self, task: u32) -> Option<&EmoteTaskState> {
        self.tasks.get(&task)
    }

    pub fn import_psz(&mut self, task: u32, name: &[u8], bytes: &[u8]) -> Result<()> {
        self.require_task(task)?;
        let resource = EmoteResource::decode(bytes)?;
        self.backend
            .as_mut()
            .ok_or(EmoteError::MissingBackend)?
            .import_resource(task, name, &resource)?;
        let state = self
            .tasks
            .get_mut(&task)
            .ok_or(EmoteError::UnknownTask(task))?;
        state.successful_resource_imports = state
            .successful_resource_imports
            .checked_add(1)
            .ok_or(EmoteError::Capacity)?;
        Ok(())
    }

    pub fn set_m2_material_group(&mut self, setting: &CpuM2MaterialSetting) -> Result<()> {
        self.backend.as_mut().ok_or(EmoteError::MissingBackend)?.set_m2_material_group(setting)
    }
    pub fn recompute_m2_controls(&mut self, settings: &[CpuM2MaterialSetting]) -> Result<()> {
        self.require_profile()?;
        self.backend.as_mut().ok_or(EmoteError::MissingBackend)?.recompute_m2_controls(settings)
    }

    pub fn debug_m2_text_cp932(&self) -> Result<Vec<u8>> {
        self.require_profile()?;
        self.backend.as_ref().ok_or(EmoteError::MissingBackend)?.debug_m2_text_cp932()
    }

    pub fn m2_action_busy(&self, task: u32) -> Result<bool> {
        self.require_task(task)?;
        self.backend.as_ref().ok_or(EmoteError::MissingBackend)?.m2_action_busy(task)
    }

    pub fn set_m2_force_position(&mut self, task: u32, position: [f32; 2], linked: bool) -> Result<()> {
        self.require_task(task)?;
        self.backend.as_mut().ok_or(EmoteError::MissingBackend)?.set_m2_force_position(task, position, linked)
    }

    pub fn import_m2_resource(
        &mut self,
        task: u32,
        name: &[u8],
        source: CpuM2ModelSource<'_>,
        load: CpuM2Load,
    ) -> Result<()> {
        self.require_task(task)?;
        let imports = self.tasks[&task]
            .successful_resource_imports
            .checked_add(1)
            .ok_or(EmoteError::Capacity)?;
        self.backend
            .as_mut()
            .ok_or(EmoteError::MissingBackend)?
            .import_m2_resource(task, name, source, load)?;
        self.tasks
            .get_mut(&task)
            .ok_or(EmoteError::UnknownTask(task))?
            .successful_resource_imports = imports;
        Ok(())
    }

    pub fn cached_m2_model(
        &self, task: u32, name: &[u8], version: &EmoteArchiveVersion,
    ) -> Option<Arc<CpuMotionModel>> {
        self.backend.as_ref()?.cached_m2_model(task, name, version)
    }

    pub fn dispatch(&mut self, task: u32, command: &EmoteCommand) -> Result<EmoteReply> {
        self.require_task(task)?;
        let descriptor =
            emote_native_descriptor(command.native).ok_or(EmoteError::Unsupported {
                native: command.native,
                reason: "native is absent from the identified E-mote descriptor table",
            })?;
        let operation = descriptor.packet_operation(command)?;
        let reply = self
            .backend
            .as_mut()
            .ok_or(EmoteError::MissingBackend)?
            .command(task, command)?;
        if !descriptor.reply.accepts(&reply) {
            return Err(EmoteError::InvalidReply {
                native: command.native,
            });
        }
        let state = self
            .tasks
            .get_mut(&task)
            .ok_or(EmoteError::UnknownTask(task))?;
        if operation == EmoteOperation::LoadResource {
            state.successful_resource_imports = state
                .successful_resource_imports
                .checked_add(1)
                .ok_or(EmoteError::Capacity)?;
        }
        if operation == EmoteOperation::EnableControls {
            state.requested_enable_raw = Some(command.data1);
        }
        match command.command {
            541 => state.controller_a_raw = Some(command.data1),
            542 => state.controller_b_raw = Some(command.data1),
            535 => state.property_134_raw = Some(command.data1),
            545 => state.property_a0_raw = Some(command.data1),
            536 => {
                if let EmoteBuffer::ThreeWords(words) = &command.buffer1 {
                    state.color_percentages_raw = Some(*words);
                }
            }
            _ => {}
        }
        Ok(reply)
    }

    pub fn advance(&mut self, task: u32, tick: EmoteTick) -> Result<()> {
        self.require_task(task)?;
        self.backend
            .as_mut()
            .ok_or(EmoteError::MissingBackend)?
            .advance(task, tick)?;
        self.tasks
            .get_mut(&task)
            .ok_or(EmoteError::UnknownTask(task))?
            .last_completed_tick = Some(tick);
        Ok(())
    }

    pub fn advance_with_random(
        &mut self,
        task: u32,
        tick: EmoteTick,
        random: &mut dyn FnMut() -> u32,
    ) -> Result<()> {
        self.require_task(task)?;
        self.backend
            .as_mut()
            .ok_or(EmoteError::MissingBackend)?
            .advance_with_random(task, tick, random)?;
        self.tasks
            .get_mut(&task)
            .ok_or(EmoteError::UnknownTask(task))?
            .last_completed_tick = Some(tick);
        Ok(())
    }

    pub fn render_rgba(&mut self, task: u32) -> Result<EmoteRasterFrame> {
        self.require_task(task)?;
        self.backend
            .as_mut()
            .ok_or(EmoteError::MissingBackend)?
            .render_rgba(task)
    }

    pub fn scene_raster_player(&self, task: u32) -> Option<(&CpuPlayer, CpuViewport)> {
        self.backend.as_ref()?.scene_raster_player(task)
    }

    pub fn can_defer_scene_raster(&self, task: u32) -> bool {
        self.backend.as_ref().is_some_and(|backend| backend.can_defer_scene_raster(task))
    }

    pub fn playback_info(&self, task: u32) -> Result<EmotePlaybackInfo> {
        self.require_task(task)?;
        self.backend
            .as_ref()
            .ok_or(EmoteError::MissingBackend)?
            .playback_info(task)
    }

    /// Supply the actual offscreen target mapping, not the final display quad.
    pub fn set_viewport(&mut self, viewport: CpuViewport) -> Result<()> {
        self.require_profile()?;
        self.backend
            .as_mut()
            .ok_or(EmoteError::MissingBackend)?
            .set_viewport(viewport)
    }

    /// PFL motname selects metadata.timelineControl, not an object.motion name.
    /// Original171ec0 starts that label with additive flags2.
    pub fn play_timeline(&mut self, task: u32, label: &str, flags: u32) -> Result<()> {
        self.require_task(task)?;
        self.backend
            .as_mut()
            .ok_or(EmoteError::MissingBackend)?
            .play_timeline(task, label, flags)
    }

    pub fn stop_timeline(&mut self, task: u32, label: &str) -> Result<()> {
        self.require_task(task)?;
        self.backend
            .as_mut()
            .ok_or(EmoteError::MissingBackend)?
            .stop_timeline(task, label)
    }

    pub fn bind_m2_lip_curve(
        &mut self,
        task: u32,
        dimension: u32,
        pattern: &[u8],
        curve: CpuM2LipCurve,
    ) -> Result<()> {
        self.require_task(task)?;
        self.backend
            .as_mut()
            .ok_or(EmoteError::MissingBackend)?
            .bind_m2_lip_curve(task, dimension, pattern, curve)
    }

    pub fn bind_m2_lpd(
        &mut self,
        task: u32,
        bytes: &[u8],
        actual_origin_tick: u32,
        bindings: &[CpuM2LipBinding],
    ) -> Result<usize> {
        self.require_task(task)?;
        self.backend
            .as_mut()
            .ok_or(EmoteError::MissingBackend)?
            .bind_m2_lpd(task, bytes, actual_origin_tick, bindings)
    }

    pub fn bind_m2_lip_sidecar(
        &mut self,
        task: u32,
        bytes: &[u8],
        kind: CpuM2LipSidecarKind,
        actual_origin_tick: u32,
        bindings: &[CpuM2LipBinding],
    ) -> Result<usize> {
        self.require_task(task)?;
        self.backend
            .as_mut()
            .ok_or(EmoteError::MissingBackend)?
            .bind_m2_lip_sidecar(task, bytes, kind, actual_origin_tick, bindings)
    }
    ///175270/175460: use the actual live profile's dimension/pattern presets.
    ///Each LPD record can carry its own source origin; LV keeps constructor0.
    pub fn bind_m2_profile_lip_sidecar(
        &mut self,
        task: u32,
        bytes: &[u8],
        kind: CpuM2LipSidecarKind,
    ) -> Result<usize> {
        self.require_task(task)?;
        self.backend
            .as_mut()
            .ok_or(EmoteError::MissingBackend)?
            .bind_m2_profile_lip_sidecar(task, bytes, kind)
    }

    pub fn update_m2_waveform(
        &mut self,
        task: u32,
        actual_samples: &[u32],
        original_extent: u32,
    ) -> Result<usize> {
        self.require_task(task)?;
        self.backend
            .as_mut()
            .ok_or(EmoteError::MissingBackend)?
            .update_m2_waveform(task, actual_samples, original_extent)
    }

    pub fn control_m2_waveform(&mut self, task: u32, immediate_stop: bool) -> Result<()> {
        self.require_task(task)?;
        self.backend
            .as_mut()
            .ok_or(EmoteError::MissingBackend)?
            .control_m2_waveform(task, immediate_stop)
    }

    pub fn set_base_argb(&mut self, task: u32, argb: u32) -> Result<()> {
        self.require_task(task)?;
        self.backend
            .as_mut()
            .ok_or(EmoteError::MissingBackend)?
            .set_base_argb(task, argb)
    }

    /// Bind the actual controller-owner snapshot after resource/profile load.
    /// Baselines with external dependencies must be refreshed by that owner.
    pub fn bind_native907_state(&mut self, task: u32, state: CpuNative907State) -> Result<()> {
        self.require_task(task)?;
        self.backend
            .as_mut()
            .ok_or(EmoteError::MissingBackend)?
            .bind_native907_state(task, state)
    }
    pub fn set_root_affine(&mut self, task: u32, affine: [f64; 6]) -> Result<()> {
        self.require_task(task)?;
        self.backend
            .as_mut()
            .ok_or(EmoteError::MissingBackend)?
            .set_root_affine(task, affine)
    }
    pub fn set_driver_transform(&mut self, task: u32, transform: CpuDriverTransform) -> Result<()> {
        self.require_task(task)?;
        self.backend.as_mut().ok_or(EmoteError::MissingBackend)?.set_driver_transform(task, transform)
    }
    pub fn set_surface_fade_default(&mut self, task: u32, duration: u32) -> Result<()> {
        self.require_task(task)?;
        self.backend.as_mut().ok_or(EmoteError::MissingBackend)?
            .set_surface_fade_default(task, duration)
    }
    pub fn surface_holds_tick(&self, task: u32, now: u32) -> Result<bool> {
        self.require_task(task)?;
        self.backend.as_ref().ok_or(EmoteError::MissingBackend)?
            .surface_holds_tick(task, now)
    }

    /// Call only after actual CPU submission succeeds, never after state update.
    pub fn record_pixel_submission(&mut self, task: u32) -> Result<()> {
        self.require_task(task)?;
        let state = self
            .tasks
            .get_mut(&task)
            .ok_or(EmoteError::UnknownTask(task))?;
        state.successful_pixel_frames = state
            .successful_pixel_frames
            .checked_add(1)
            .ok_or(EmoteError::Capacity)?;
        Ok(())
    }

    fn require_profile(&self) -> Result<()> {
        if self.bound {
            Ok(())
        } else {
            Err(EmoteError::UnknownProfile)
        }
    }

    fn require_task(&self, task: u32) -> Result<()> {
        self.require_profile()?;
        if self.tasks.contains_key(&task) {
            Ok(())
        } else {
            Err(EmoteError::UnknownTask(task))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn psz(body: &[u8]) -> Vec<u8> {
        let mut compressor =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        compressor.write_all(body).unwrap();
        let packed = compressor.finish().unwrap();
        let mut bytes = b"PSZ\0".to_vec();
        bytes.extend_from_slice(&(packed.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&(body.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&packed);
        bytes
    }

    #[test]
    fn encrypted_psb_decompresses_but_never_becomes_playable() {
        let body = b"PSB\0\x04\0\x01\0unreadable-encrypted-header";
        let resource = EmoteResource::decode(&psz(body)).unwrap();
        assert_eq!(resource.bytes(), body);
        assert!(matches!(
            resource.require_plaintext_header(),
            Err(EmoteError::EncryptedPsbHeader {
                version: 4,
                flags: 1
            })
        ));
    }

    #[test]
    fn psz_rejects_false_lengths_and_trailing_compressed_streams() {
        let body = b"PSB\0\x04\0\0\0synthetic";
        let mut wrong_size = psz(body);
        wrong_size[8..12].copy_from_slice(&8u32.to_le_bytes());
        assert!(EmoteResource::decode(&wrong_size).is_err());
        let mut trailing = psz(body);
        trailing.extend_from_slice(b"extra");
        let length = (trailing.len() - 16) as u32;
        trailing[4..8].copy_from_slice(&length.to_le_bytes());
        assert!(matches!(
            EmoteResource::decode(&trailing),
            Err(EmoteError::Decompression)
        ));
    }

    #[test]
    fn raster_frame_viewport_materialization_clips_and_reuses_matching_frames() -> Result<()> {
        let matching = EmoteRasterFrame::new(1, 1, [0, 0], vec![1, 2, 3, 4])?;
        let address = matching.rgba().as_ptr();
        let matching = matching.into_viewport([1, 1])?;
        assert_eq!(matching.rgba().as_ptr(), address);

        let cropped = EmoteRasterFrame::new(2, 2, [1, 1], vec![
            1, 2, 3, 255, 4, 5, 6, 255,
            7, 8, 9, 255, 10, 11, 12, 255,
        ])?;
        let expanded = cropped.clone().into_viewport([3, 2])?;
        assert_eq!(expanded.origin(), [0, 0]);
        assert_eq!(expanded.rgba(), [
            [0u8, 0, 0, 0], [0, 0, 0, 0], [0, 0, 0, 0],
            [0, 0, 0, 0], [1, 2, 3, 255], [4, 5, 6, 255],
        ].concat().as_slice());

        let clipped = EmoteRasterFrame::new(2, 2, [-1, 0], cropped.rgba().to_vec())?
            .into_viewport([2, 2])?;
        assert_eq!(clipped.rgba(), [
            [4u8, 5, 6, 255], [0, 0, 0, 0],
            [10, 11, 12, 255], [0, 0, 0, 0],
        ].concat().as_slice());
        let outside = EmoteRasterFrame::new(1, 1, [5, 5], vec![255; 4])?
            .into_viewport([2, 2])?;
        assert!(outside.rgba().iter().all(|&byte| byte == 0));
        assert!(outside.into_viewport([0, 2]).is_err());
        assert!(cropped.into_viewport([u32::MAX, u32::MAX]).is_err());
        Ok(())
    }

    #[test]
    fn no_backend_cannot_register_an_apparently_working_task() {
        let mut services = EmoteServices::default();
        assert!(!services.bind_pe_sha256(&"0".repeat(64)));
        assert!(services.bind_pe_sha256(NEKOPARA_PE_SHA256));
        assert!(!services.backend_ready());
        assert!(matches!(
            services.register_task(7, 54),
            Err(EmoteError::MissingBackend)
        ));
        assert!(services.state(7).is_none());
        assert!(EmoteRasterFrame::new(0, 0, [0; 2], Vec::new()).is_err());
        assert!(EmoteRasterFrame::new(1, 1, [0; 2], vec![0; 3]).is_err());
    }
}

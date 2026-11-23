//! Shared raster operations for the logical canvas.
//!
//! Ordered draws can retain a device-owned frame until an actual CPU consumer
//! requests it. The packed path keeps recovery pixels until readback succeeds.

use anyhow::{Context, Result, bail};
use cs2_renderer::{BlendMode, Canvas, CanvasReadback, FrameAsset, FramePixelIdentity, ImageDrawOptions, Rect, RgbaFrame, ScaleFilter};
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, HashSet, VecDeque};
use std::sync::{Arc, atomic::{AtomicBool, AtomicU64, Ordering}, mpsc};
use std::time::Duration;
use super::perf_detail::{GpuDecision, ReadbackKind, ReadbackTimer, list_gpu_decision};
use super::canvas_native::VramOverflow;
use super::native_tasks::WipeStep;

#[cfg(target_os = "android")]
#[path = "gpu_raster_present.rs"]
mod present;
#[cfg(target_os = "android")]
pub(super) use present::{candidate as surface_candidate, detach, display_stats, present_canvas, set_display_stats_enabled};

#[path = "gpu_emote.rs"]
mod emote_raster;

#[derive(Clone, Copy, PartialEq, Eq)]
enum RasterStatus {
    NotCreated,
    Ready,
    Disabled,
    InitializationFailed,
}

thread_local! {
    static RASTER: RefCell<Option<Raster>> = const { RefCell::new(None) };
    static RASTER_STATUS: Cell<RasterStatus> = const { Cell::new(RasterStatus::NotCreated) };
}

fn set_raster_status(status: RasterStatus) {
    RASTER_STATUS.with(|current| current.set(status));
}

fn missing_emote_raster_reason(status: RasterStatus) -> &'static str {
    match status {
        RasterStatus::NotCreated => "Raster has not been created",
        RasterStatus::Ready => "Raster slot is empty despite ready status",
        RasterStatus::Disabled => "Raster is disabled or was discarded after a device error",
        RasterStatus::InitializationFailed => "Raster initialization failed",
    }
}

pub(super) fn emote_missing_raster_reason() -> &'static str {
    RASTER_STATUS.with(|status| missing_emote_raster_reason(status.get()))
}

pub(super) fn enable() {
    RASTER.with_borrow_mut(|slot| {
        *slot = match Raster::new() {
            Ok(raster) => {
                set_raster_status(RasterStatus::Ready);
                Some(raster)
            }
            Err(error) => {
                set_raster_status(RasterStatus::InitializationFailed);
                report(&format!("backend=cpu reason={error:#}"));
                None
            }
        };
    });
}

#[cfg(windows)]
pub(super) fn disable() {
    let raster = RASTER.with_borrow_mut(Option::take);
    set_raster_status(RasterStatus::Disabled);
    drop(raster);
}

pub(super) fn available() -> bool {
    RASTER.with_borrow(|slot| slot.is_some())
}

pub(super) use emote_raster::{
    begin_emote_batch, blend_emote, draw_emote, expand_emote_surface, mix_emote_surfaces,
    draw_emote_surface, prepare_emote_poses, prepare_emote_surfaces, prewarm_emote, stage_emote_model,
    tint_emote_surface,
};

pub(super) fn textured_planes_requested() -> bool {
    // Device sampling uses f32. Keep the f64 path available for pixel comparisons.
    #[cfg(target_os = "android")]
    return !android_property(c"debug.cs2.textured_planes", |value| value == b"0").unwrap_or(false);
    #[cfg(windows)]
    return std::env::var("CS2_GPU_TEXTURED_PLANES").as_deref() == Ok("1");
}

#[cfg(windows)]
pub(super) fn emote_requested() -> bool {
    !std::env::var_os("CS2_EMOTE_GPU").is_some_and(|value| value == "0")
}

#[cfg(target_os = "android")]
pub(super) fn emote_requested() -> bool {
    !android_property(c"debug.cs2.emote_gpu", |value| value == b"0").unwrap_or(false)
}

#[cfg(windows)]
fn emote_device_blend_requested() -> bool {
    !std::env::var_os("CS2_EMOTE_DEVICE_BLEND").is_some_and(|value| value == "0")
}

#[cfg(target_os = "android")]
fn emote_device_blend_requested() -> bool {
    !android_property(c"debug.cs2.emote_device_blend", |value| value == b"0").unwrap_or(false)
}

#[cfg(windows)]
pub(super) fn emote_batch_requested() -> bool {
    available() && emote_device_blend_requested()
        && !std::env::var_os("CS2_EMOTE_BATCH").is_some_and(|value| value == "0")
}

#[cfg(target_os = "android")]
pub(super) fn emote_batch_requested() -> bool {
    available() && emote_device_blend_requested()
        && !android_property(c"debug.cs2.single_barrier", |value| value == b"0").unwrap_or(false)
}

#[cfg(target_os = "android")]
fn android_property<T>(name: &std::ffi::CStr, parse: impl FnOnce(&[u8]) -> T) -> Option<T> {
    use std::ffi::c_char;
    unsafe extern "C" {
        fn __system_property_get(name: *const c_char, value: *mut c_char) -> i32;
    }
    // Bionic's PROP_VALUE_MAX includes room for the terminal NUL.
    let mut value = [0_u8; 92];
    let length = unsafe {
        __system_property_get(name.as_ptr(), value.as_mut_ptr().cast())
    };
    value.get(..usize::try_from(length).ok()?).map(parse)
}

pub(super) fn latest_gpu_timing() -> Option<(u64, u64)> {
    RASTER.with_borrow(|slot| slot.as_ref()?.timing.as_ref()?.latest)
}

pub(super) fn frame_revision(canvas: &Canvas) -> Option<u64> {
    Some(device_frame(canvas)?.revision)
}

fn device_frame(canvas: &Canvas) -> Option<&DeviceFrame> {
    canvas.device_pixels()?.as_any().downcast_ref()
}

pub(super) fn can_use_atlas(frame: &RgbaFrame, atlas: Option<&VramOverflow>) -> bool {
    RASTER.with_borrow_mut(|slot| {
        let Some(raster) = slot.as_mut() else { return false };
        let accepted = raster.accepts_atlas(frame, atlas);
        if !accepted && !raster.reported_atlas_fallback {
            raster.reported_atlas_fallback = true;
            let [width, height] = atlas_dimensions(frame, atlas);
            report(&format!("atlas_fallback width={width} height={height} overflow={} page_limit={}",
                atlas.is_some(), raster.atlas_limit()));
        }
        accepted
    })
}

/// Publish an immediately preceding full raster output under its immutable
/// frame identity. The caller must not modify the canvas before freezing it.
pub(super) fn retain_rasterized_frame(frame: &RgbaFrame) {
    RASTER.with_borrow_mut(|slot| {
        if let Some(raster) = slot.as_mut() { raster.retain_full_target(frame); }
    });
}

#[cfg(windows)]
pub(super) fn report(message: &str) {
    eprintln!("CS2Gpu {message}");
}

#[cfg(target_os = "android")]
pub(super) fn report(message: &str) {
    #[link(name = "log")]
    unsafe extern "C" {
        fn __android_log_write(priority: std::ffi::c_int, tag: *const std::ffi::c_char,
            text: *const std::ffi::c_char) -> std::ffi::c_int;
    }
    if let Ok(message) = std::ffi::CString::new(message) {
        // The tag and message remain valid for the duration of the call.
        unsafe { __android_log_write(4, c"CS2Gpu".as_ptr(), message.as_ptr()); }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct RasterRect {
    pub(super) origin: [u32; 2],
    pub(super) size: [u32; 2],
}

impl RasterRect {
    fn workgroups(self) -> [u32; 2] {
        self.size.map(|size| size.div_ceil(8))
    }

    fn overlaps(self, other: Self) -> bool {
        (0..2).all(|axis| {
            self.origin[axis] < other.origin[axis] + other.size[axis]
                && other.origin[axis] < self.origin[axis] + self.size[axis]
        })
    }

    fn contains(self, other: Self) -> bool {
        (0..2).all(|axis| self.origin[axis] <= other.origin[axis]
            && self.origin[axis] + self.size[axis] >= other.origin[axis] + other.size[axis])
    }

    fn union(self, other: Self) -> Self {
        let origin = std::array::from_fn(|axis| self.origin[axis].min(other.origin[axis]));
        let size = std::array::from_fn(|axis| (self.origin[axis] + self.size[axis])
            .max(other.origin[axis] + other.size[axis]) - origin[axis]);
        Self { origin, size }
    }

    fn pixels(self) -> u64 { u64::from(self.size[0]) * u64::from(self.size[1]) }
}

#[derive(Clone, Copy)]
pub(super) enum Draw<'a> {
    PlaneSpan { source_origin: [u32; 2], opacity: u8, blend: BlendMode },
    PlaneTriangles { vertices: &'a [[[f32; 6]; 3]], source_origin: [u32; 2], argb: u32, blend: BlendMode },
    ListSpan { source_origin: [u32; 2], width: u32, left_halo: bool, opacity: u32 },
    ListAffine { inverse: [i32; 4], offset: [i32; 2], base: [i32; 2],
        opacity: u32, blend: BlendMode },
    Copy { source_origin: [u32; 2] },
    ImageSpan { source_origin: [u32; 2], opacity: u8 },
    DiffuseFrame { alpha: u8 },
    Wipe { mask: &'a RgbaFrame, step: WipeStep },
    EffectSpans { taps: &'a [([i32; 2], u8)] },
}

#[derive(Clone, Copy)]
pub(super) struct RasterJob<'a> {
    pub(super) rect: RasterRect,
    pub(super) source: Option<&'a RgbaFrame>,
    /// Some(None) is the viewport with no overflow tiles.
    pub(super) atlas: Option<Option<&'a VramOverflow>>,
    pub(super) operation: Draw<'a>,
}

enum RasterOutput {
    Canvas,
    CpuPixels,
}

fn blend_code(mode: BlendMode) -> u32 {
    match mode {
        BlendMode::SourceOver => 0,
        BlendMode::AddSource => 1,
        BlendMode::AddDestination => 2,
        BlendMode::Multiply => 3,
        BlendMode::InvertDestination => 4,
        BlendMode::AddMultiply => 5,
        BlendMode::ListAlphaFromRed => 6,
        BlendMode::ListAlphaFromGreen => 7,
        BlendMode::ListAlphaFromBlue => 8,
        BlendMode::ListAlphaFromAlpha => 9,
    }
}

impl Draw<'_> {
    fn label(&self) -> &'static str {
        match self {
            Self::PlaneSpan { .. } => "plane_span",
            Self::PlaneTriangles { .. } => "plane_triangles",
            Self::ListSpan { .. } => "list_span",
            Self::ListAffine { .. } => "list_affine",
            Self::Copy { .. } => "copy",
            Self::ImageSpan { .. } => "image_span",
            Self::DiffuseFrame { .. } => "transition_diffuse",
            Self::Wipe { .. } => "transition_wipe",
            Self::EffectSpans { .. } => "effect_spans",
        }
    }

    fn code(&self) -> usize {
        match self {
            Self::PlaneSpan { .. } => 0,
            Self::PlaneTriangles { .. } => 1,
            Self::ListSpan { .. } => 2,
            Self::ListAffine { .. } => 3,
            Self::Copy { .. } => 4,
            Self::ImageSpan { .. } => 5,
            Self::DiffuseFrame { .. } => 6,
            Self::Wipe { .. } => 7,
            Self::EffectSpans { .. } => 8,
        }
    }

    fn append_args(self, args: &mut Vec<u32>) {
        match self {
            Self::PlaneSpan { source_origin, opacity, blend } => {
                args.extend(source_origin);
                args.extend([u32::from(opacity), blend_code(blend)]);
            }
            Self::PlaneTriangles { vertices, source_origin, argb, blend } => {
                args.extend([argb, blend_code(blend), vertices.len() as u32]);
                args.extend(source_origin);
                args.extend(vertices.iter().flatten().flatten().map(|value| value.to_bits()));
            }
            Self::ListSpan { source_origin, width, left_halo, opacity } => {
                args.extend(source_origin);
                args.extend([width, u32::from(left_halo), opacity]);
            }
            Self::ListAffine { inverse, offset, base, opacity, blend } => {
                args.extend(inverse.into_iter().chain(offset).chain(base).map(|value| value as u32));
                args.extend([opacity, blend_code(blend)]);
            }
            Self::Copy { source_origin } => args.extend(source_origin),
            Self::ImageSpan { source_origin, opacity } => {
                args.extend(source_origin);
                args.push(u32::from(opacity));
            }
            Self::DiffuseFrame { alpha } => args.push(u32::from(alpha)),
            Self::Wipe { step, .. } => args.extend(match step {
                WipeStep::Hard(threshold) => [0, threshold],
                WipeStep::Soft(progress) => [1, progress as u32],
            }),
            Self::EffectSpans { taps } => {
                args.push(taps.len() as u32);
                for &([x, y], alpha) in taps {
                    args.extend([x as u32, y as u32, u32::from(alpha)]);
                }
            }
        }
    }
}

pub(super) fn draw(
    target: &mut Canvas, source: Option<&RgbaFrame>, rect: RasterRect, operation: Draw<'_>,
) -> bool {
    draw_job(target, RasterJob { rect, source, atlas: None, operation })
}

pub(super) fn draw_job(target: &mut Canvas, job: RasterJob<'_>) -> bool {
    let RasterJob { rect, operation, .. } = job;
    // Transfers and synchronization dominate small draws. Triangles and
    // affine interpolation do more arithmetic per pixel than integer spans.
    let min_area = match operation {
        Draw::PlaneSpan { .. } | Draw::ListSpan { .. } | Draw::Copy { .. }
        | Draw::ImageSpan { .. } | Draw::DiffuseFrame { .. } | Draw::Wipe { .. } => 128 * 1024,
        Draw::PlaneTriangles { .. } | Draw::ListAffine { .. } | Draw::EffectSpans { .. } => 32 * 1024,
    };
    let area = u64::from(rect.size[0]) * u64::from(rect.size[1]);
    let device_continuation = device_frame(target).is_some();
    #[cfg(target_os = "android")]
    let device_continuation = device_continuation
        && !android_property(c"debug.cs2.single_barrier", |value| value == b"0").unwrap_or(false);
    let list = match operation {
        Draw::ListSpan { .. } => Some((GpuDecision::SpanGpu, GpuDecision::SpanSmall)),
        Draw::ListAffine { .. } => Some((GpuDecision::AffineGpu, GpuDecision::AffineSmall)),
        _ => None,
    };
    if area < min_area && !device_continuation {
        if let Some((_, reason)) = list { list_gpu_decision(reason, area); }
        return false;
    }
    let note = |reason| {
        if list.is_some() { list_gpu_decision(reason, area); }
    };
    RASTER.with_borrow_mut(|slot| {
        let Some(raster) = slot.as_mut() else {
            note(GpuDecision::BackendUnavailable);
            return false;
        };
        let label = operation.label();
        let code = operation.code();
        match raster.draw_jobs(target, &[rect], &[job], RasterOutput::Canvas, false) {
            Ok(true) => {
                if let Some((reason, _)) = list { list_gpu_decision(reason, area); }
                if !raster.reported[code] {
                    report(&format!("draw={label} backend={}", raster.backend));
                    raster.reported[code] = true;
                }
                true
            }
            Ok(false) => {
                note(GpuDecision::DispatchLimits);
                false
            }
            Err(error) => {
                note(GpuDecision::DispatchFailed);
                report(&format!("draw={label} backend=cpu reason={error:#}"));
                set_raster_status(RasterStatus::Disabled);
                *slot = None;
                false
            }
        }
    })
}

pub(super) fn image_job<'a>(
    size: [u32; 2], frame: &'a RgbaFrame, options: &ImageDrawOptions,
) -> Option<RasterJob<'a>> {
    if !options.visible || options.opacity == 0 || frame.is_empty()
        || options.scale_x != 1. || options.scale_y != 1. || options.rotation_radians != 0.
        || options.pivot != [0.; 2] || options.flip_x || options.flip_y || options.mask.is_some()
        || options.filter != ScaleFilter::Nearest || options.blend_mode != BlendMode::SourceOver
    {
        return None;
    }
    let position = [i64::from(options.position.x), i64::from(options.position.y)];
    let mut start = position.map(|coordinate| coordinate.max(0));
    let mut end = std::array::from_fn::<_, 2, _>(|axis| {
        (position[axis] + i64::from([frame.width(), frame.height()][axis])).min(i64::from(size[axis]))
    });
    if let Some(clip) = options.clip {
        let origin = [i64::from(clip.x), i64::from(clip.y)];
        for axis in 0..2 {
            start[axis] = start[axis].max(origin[axis]);
            end[axis] = end[axis].min(origin[axis] + i64::from([clip.width, clip.height][axis]));
        }
    }
    if (0..2).any(|axis| start[axis] >= end[axis]) { return None; }
    Some(RasterJob {
        rect: RasterRect { origin: start.map(|value| value as u32), size: std::array::from_fn(|axis| (end[axis] - start[axis]) as u32) },
        source: Some(frame),
        atlas: None,
        operation: Draw::ImageSpan { source_origin: std::array::from_fn(|axis| (start[axis] - position[axis]) as u32), opacity: options.opacity },
    })
}

pub(super) fn draw_image(canvas: &mut Canvas, frame: &RgbaFrame, options: &ImageDrawOptions) -> bool {
    image_job([canvas.width(), canvas.height()], frame, options)
        .is_some_and(|job| draw_job(canvas, job))
}

/// Draw a complete ordered image-list pass using a shared device target.
pub(super) fn draw_list_group(canvas: &mut Canvas, draws: &[RasterJob<'_>]) -> bool {
    let continuation = device_frame(canvas).is_some();
    #[cfg(target_os = "android")]
    let continuation = continuation
        && !android_property(c"debug.cs2.single_barrier", |value| value == b"0").unwrap_or(false);
    if draws.is_empty() || draws.len() > 1024 || (!continuation && draws.len() < 2) {
        return false;
    }
    let (area, pixels) = group_bounds(draws);
    if pixels < 192 * 1024 && !continuation { return false; }
    let packed;
    let regions = if area.pixels() > 2 * pixels {
        packed = pack_dirty_regions(draws);
        if packed.iter().map(|region| region.pixels()).sum::<u64>() > 3 * pixels { return false; }
        packed.as_slice()
    } else {
        std::slice::from_ref(&area)
    };
    RASTER.with_borrow_mut(|slot| {
        let Some(raster) = slot.as_mut() else { return false };
        // The list store immediately commits CPU pixels and copies dirty areas to VRAM.
        match raster.draw_jobs(canvas, regions, draws, RasterOutput::CpuPixels, false) {
            Ok(true) => {
                for RasterJob { rect, operation, .. } in draws {
                    let decision = match operation {
                        Draw::Copy { .. } => GpuDecision::CopyGpu,
                        _ => GpuDecision::SpanGpu,
                    };
                    list_gpu_decision(decision, u64::from(rect.size[0]) * u64::from(rect.size[1]));
                }
                if !raster.reported_batch {
                    report(&format!("draw=list_span_batch backend={} draws={} targets={}", raster.backend, draws.len(), regions.len()));
                    raster.reported_batch = true;
                }
                true
            }
            Ok(false) => false,
            Err(error) => {
                report(&format!("draw=list_span_batch backend=cpu reason={error:#}"));
                set_raster_status(RasterStatus::Disabled);
                *slot = None;
                false
            }
        }
    })
}

fn pack_dirty_regions(draws: &[RasterJob<'_>]) -> Vec<RasterRect> {
    let mut rectangles: Vec<RasterRect> = Vec::new();
    for job in draws {
        let mut merged = job.rect;
        let mut at = 0;
        while at < rectangles.len() {
            if rectangles[at].overlaps(merged) {
                merged = merged.union(rectangles.swap_remove(at));
                // Enlarging a region may add an earlier intersection.
                at = 0;
            } else {
                at += 1;
            }
        }
        rectangles.push(merged);
    }
    rectangles
}

fn group_bounds(draws: &[RasterJob<'_>]) -> (RasterRect, u64) {
    let mut start = [u32::MAX; 2];
    let mut end = [0; 2];
    let mut pixels = 0;
    for RasterJob { rect, .. } in draws {
        for axis in 0..2 {
            start[axis] = start[axis].min(rect.origin[axis]);
            end[axis] = end[axis].max(rect.origin[axis] + rect.size[axis]);
        }
        pixels += u64::from(rect.size[0]) * u64::from(rect.size[1]);
    }
    (RasterRect { origin: start, size: std::array::from_fn(|axis| end[axis] - start[axis]) }, pixels)
}

pub(super) fn draw_plane_group(canvas: &mut Canvas, draws: &[RasterJob<'_>]) -> bool {
    if !(1..=256).contains(&draws.len()) {
        return false;
    }
    let (area, pixels) = group_bounds(draws);
    if pixels < 128 * 1024 { return false; }
    let packed;
    let regions = if area.pixels() > 2 * pixels {
        packed = pack_dirty_regions(draws);
        packed.as_slice()
    } else {
        std::slice::from_ref(&area)
    };
    RASTER.with_borrow_mut(|slot| {
        let Some(raster) = slot.as_mut() else { return false };
        match raster.draw_jobs(canvas, regions, draws, RasterOutput::Canvas, true) {
            Ok(true) => {
                if !raster.reported_plane_group {
                    report(&format!("draw=plane_group backend={} layers={}", raster.backend, draws.len()));
                    raster.reported_plane_group = true;
                }
                true
            }
            Ok(false) => false,
            Err(error) => {
                report(&format!("draw=plane_group backend=cpu reason={error:#}"));
                set_raster_status(RasterStatus::Disabled);
                *slot = None;
                false
            }
        }
    })
}

struct Raster {
    #[cfg(target_os = "android")]
    instance: wgpu::Instance,
    #[cfg(target_os = "android")]
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
    device_blend: wgpu::ComputePipeline,
    device_blend_layout: wgpu::BindGroupLayout,
    device_blend_params: wgpu::Buffer,
    empty_source: wgpu::TextureView,
    empty_atlas: wgpu::TextureView,
    empty_page_table: wgpu::Buffer,
    sources: VecDeque<CachedTexture>,
    atlas: Option<AtlasTexture>,
    source_bytes: usize,
    full_target: Option<([u32; 2], usize)>,
    target: Arc<wgpu::Buffer>,
    spares: Vec<Arc<wgpu::Buffer>>,
    args_buffer: wgpu::Buffer,
    readback: wgpu::Buffer,
    timing: Option<GpuTiming>,
    emote: Option<emote_raster::EmoteRaster>,
    reported_emote_result: Option<bool>,
    emote_initialization_failed: bool,
    scratch: Vec<u8>,
    previous_planes: Option<CompletedPlanes>,
    failed: Arc<AtomicBool>,
    backend: wgpu::Backend,
    reported: [bool; 9],
    reported_batch: bool,
    reported_plane_group: bool,
    reported_atlas_fallback: bool,
    #[cfg(all(test, windows))]
    test_direct_output: bool,
    #[cfg(target_os = "android")]
    presenter: Option<present::Presenter>,
    #[cfg(target_os = "android")]
    failed_window: Option<usize>,
}

impl Drop for Raster {
    fn drop(&mut self) {
        // Frames can outlive the renderer through runtime pose/transition
        // caches. Retiring this generation invalidates those device snapshots.
        self.failed.store(true, Ordering::Relaxed);
    }
}

struct CachedTexture {
    identity: FramePixelIdentity,
    asset: Option<(FrameAsset, u64)>,
    size: [u32; 2],
    bytes: usize,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

struct AtlasTexture {
    id: u64,
    revision: u64,
    dimensions: [u32; 2],
    viewport: RgbaFrame,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    page_table: wgpu::Buffer,
    pages: BTreeMap<u32, u32>,
}

struct GpuTiming {
    queries: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    latest: Option<(u64, u64)>,
}

struct CompletedPlanes {
    area: RasterRect,
    parameters: Vec<u8>,
    inputs: Vec<(Option<RgbaFrame>, Option<(u64, u64)>)>,
    before: Vec<u8>,
    before_revision: Option<u64>,
    after: CompletedOutput,
    seed: Option<wgpu::Buffer>,
}

enum CompletedOutput {
    Cpu(Vec<u8>),
    Device(Canvas),
}

#[derive(Debug)]
struct DeviceFrame {
    revision: u64,
    size: [u32; 2],
    buffer: Arc<wgpu::Buffer>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    failed: Arc<AtomicBool>,
}

impl CanvasReadback for DeviceFrame {
    #[track_caller]
    fn read_rgba(&self, output: &mut [u8]) -> bool {
        self.read_rgba_at(output, std::panic::Location::caller())
    }

    fn read_rgba_at(&self, output: &mut [u8], location: &'static std::panic::Location) -> bool {
        static LOCATIONS: std::sync::Mutex<BTreeSet<String>> = std::sync::Mutex::new(BTreeSet::new());
        if let Ok(mut locations) = LOCATIONS.lock() && locations.insert(location.to_string()) {
            report(&format!("device_barrier bytes={} caller={location}", self.size[0] * self.size[1] * 4));
        }
        if self.failed.load(Ordering::Relaxed) { return false; }
        let size = self.size.into_iter().map(u64::from).product::<u64>() * 4;
        if output.len() as u64 != size { return false; }
        let measurement = ReadbackTimer::start(ReadbackKind::Device, size);
        let readback = buffer(&self.device, size, DOWNLOAD);
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_buffer_to_buffer(&self.buffer, 0, &readback, 0, size);
        let submitted = self.queue.submit(Some(encoder.finish()));
        let slice = readback.slice(..);
        let (send, receive) = mpsc::sync_channel(1);
        slice.map_async(wgpu::MapMode::Read, move |value| { let _ = send.send(value); });
        if self.device.poll(wgpu::PollType::Wait {
            submission_index: Some(submitted), timeout: Some(Duration::from_millis(250)),
        }).is_err() || !matches!(receive.try_recv(), Ok(Ok(()))) || self.failed.load(Ordering::Relaxed) {
            self.failed.store(true, Ordering::Relaxed);
            report("device_frame_readback_failed");
            return false;
        }
        output.copy_from_slice(&slice.get_mapped_range());
        readback.unmap();
        if let Some(measurement) = measurement { measurement.complete(); }
        true
    }

    fn as_any(&self) -> &dyn std::any::Any { self }
}

fn buffer(device: &wgpu::Device, size: u64, usage: wgpu::BufferUsages) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("CS2 raster storage"),
        size,
        usage,
        mapped_at_creation: false,
    })
}

const UPLOAD: wgpu::BufferUsages = wgpu::BufferUsages::STORAGE.union(wgpu::BufferUsages::COPY_DST);
const TARGET: wgpu::BufferUsages = UPLOAD.union(wgpu::BufferUsages::COPY_SRC);
const DOWNLOAD: wgpu::BufferUsages = wgpu::BufferUsages::MAP_READ.union(wgpu::BufferUsages::COPY_DST);
const SOURCE_CACHE_BYTES: usize = 48 * 1024 * 1024;
const SOURCE_CACHE_ENTRIES: usize = 48;
// Keep the stride aligned to storage binding offsets and in sync with WGSL.
const ARGS_BYTES: u64 = 4096;
const TIMING_BYTES: u64 = 16;
const ATLAS_TILE: u32 = 256;
const ATLAS_MAX_PAGES: u32 = 256;

impl Raster {
    fn new() -> Result<Self> {
        #[cfg(windows)]
        let backends = wgpu::Backends::DX12;
        #[cfg(target_os = "android")]
        let backends = wgpu::Backends::VULKAN;
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends,
            ..Default::default()
        });
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            ..Default::default()
        })).context("request GPU raster adapter")?;
        let info = adapter.get_info();
        if matches!(info.device_type, wgpu::DeviceType::Cpu) {
            bail!("software adapter {}", info.name);
        }
        let mut requested = wgpu::Features::TIMESTAMP_QUERY;
        if info.backend == wgpu::Backend::Vulkan {
            requested |= wgpu::Features::VULKAN_GOOGLE_DISPLAY_TIMING;
        }
        let mut request = wgpu::DeviceDescriptor {
            label: Some("CS2 raster device"),
            required_features: adapter.features() & requested,
            required_limits: wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()),
            ..Default::default()
        };
        let (device, queue) = pollster::block_on(adapter.request_device(&request)).or_else(|error| {
            if !request.required_features.contains(wgpu::Features::VULKAN_GOOGLE_DISPLAY_TIMING) {
                return Err(error);
            }
            report(&format!("vulkan_display_timing=disabled reason={error}"));
            request.required_features.remove(wgpu::Features::VULKAN_GOOGLE_DISPLAY_TIMING);
            pollster::block_on(adapter.request_device(&request))
        }).context("open GPU raster device")?;
        let required_features = device.features();
        let failed = Arc::new(AtomicBool::new(false));
        let error_flag = Arc::clone(&failed);
        device.on_uncaptured_error(Arc::new(move |error| {
            report(&format!("device_error={error}"));
            error_flag.store(true, Ordering::Relaxed);
        }));
        let lost_flag = Arc::clone(&failed);
        device.set_device_lost_callback(move |reason, _| {
            if reason != wgpu::DeviceLostReason::Destroyed {
                lost_flag.store(true, Ordering::Relaxed);
            }
        });
        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("CS2 raster shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("gpu_raster.wgsl").into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("CS2 raster"),
            layout: None,
            module: &shader,
            entry_point: Some("raster"),
            compilation_options: Default::default(),
            cache: None,
        });
        let device_blend = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("CS2 device pose blend"),
            layout: None,
            module: &shader,
            entry_point: Some("device_span"),
            compilation_options: Default::default(),
            cache: None,
        });
        if let Some(error) = pollster::block_on(device.pop_error_scope()) {
            bail!("compile raster shader: {error}");
        }
        let layout = pipeline.get_bind_group_layout(0);
        let device_blend_layout = device_blend.get_bind_group_layout(0);
        let device_blend_params = buffer(&device, 48, wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST);
        let source = rgba_texture(&device, [1; 2]).create_view(&Default::default());
        let empty_atlas = atlas_array(&device, 1).create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array), ..Default::default()
        });
        let empty_page_table = buffer(&device, 4, UPLOAD);
        let target = Arc::new(buffer(&device, 4, TARGET));
        let args_buffer = buffer(&device, ARGS_BYTES, UPLOAD);
        let readback = buffer(&device, 4 + TIMING_BYTES, DOWNLOAD);
        let timing = required_features.contains(wgpu::Features::TIMESTAMP_QUERY).then(|| GpuTiming {
            queries: device.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some("CS2 raster timer"), ty: wgpu::QueryType::Timestamp, count: 2,
            }),
            resolve: buffer(&device, TIMING_BYTES,
                wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC),
            latest: None,
        });
        report(&format!("backend={} adapter={}", info.backend, info.name));
        Ok(Self {
            device, queue, pipeline, layout, device_blend, device_blend_layout, device_blend_params,
            empty_source: source, empty_atlas, empty_page_table,
            sources: VecDeque::new(), source_bytes: 0,
            target, args_buffer, readback, timing, atlas: None, full_target: None,
            emote: None, reported_emote_result: None, emote_initialization_failed: false,
            scratch: Vec::new(), previous_planes: None, failed, backend: info.backend, reported: [false; 9],
            spares: Vec::new(), reported_atlas_fallback: false,
            #[cfg(all(test, windows))]
            test_direct_output: false,
            #[cfg(target_os = "android")]
            instance,
            #[cfg(target_os = "android")]
            adapter,
            #[cfg(target_os = "android")]
            presenter: None,
            #[cfg(target_os = "android")]
            failed_window: None,
            reported_batch: false, reported_plane_group: false,
        })
    }

    fn direct_output_enabled(&self) -> bool {
        #[cfg(all(test, windows))]
        if self.test_direct_output { return true; }
        #[cfg(windows)]
        {
            static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
            *ENABLED.get_or_init(|| std::env::var_os("CS2_REPLAY_DEVICE_OUTPUT").is_some_and(|value| value == "1"))
        }
        #[cfg(target_os = "android")]
        return !android_property(c"debug.cs2.single_barrier", |value| value == b"0").unwrap_or(false);
    }

    fn source_texture(&mut self, frame: Option<&RgbaFrame>) -> wgpu::TextureView {
        let Some(frame) = frame else { return self.empty_source.clone() };
        let size = [frame.width(), frame.height()];
        let index = self.sources.iter().position(|entry| entry.size == size && entry.identity.matches(frame))
            .or_else(|| {
                let asset = frame.asset_source()?;
                self.sources.iter().position(|entry| entry.size == size && entry.asset.as_ref()
                    .is_some_and(|(key, revision)| key == asset && *revision == frame.asset_revision()))
            });
        if let Some(index) = index {
            let mut entry = self.sources.remove(index).unwrap();
            let view = entry.view.clone();
            entry.identity = frame.pixel_identity();
            self.sources.push_back(entry);
            return view;
        }
        let bytes = frame.pixels().len();
        let texture = rgba_texture(&self.device, size);
        let patch = frame.pixel_patch().and_then(|(previous, region)| {
            let patch_bytes = u64::from(region.width) * u64::from(region.height) * 4;
            self.sources.iter().find(|entry| entry.size == size && entry.identity.same_allocation(previous))
                .filter(|_| patch_bytes < bytes as u64).map(|entry| (&entry.texture, region))
        });
        if let Some((previous, region)) = patch {
            let mut encoder = self.device.create_command_encoder(&Default::default());
            encoder.copy_texture_to_texture(previous.as_image_copy(), texture.as_image_copy(), texture.size());
            self.queue.submit(Some(encoder.finish()));
            let x = region.x as u32;
            let y = region.y as u32;
            let start = (y as usize * size[0] as usize + x as usize) * 4;
            self.queue.write_texture(wgpu::TexelCopyTextureInfo {
                origin: wgpu::Origin3d { x, y, z: 0 }, ..texture.as_image_copy()
            }, &frame.pixels()[start..], wgpu::TexelCopyBufferLayout {
                offset: 0, bytes_per_row: Some(size[0] * 4), rows_per_image: Some(region.height),
            }, wgpu::Extent3d { width: region.width, height: region.height, depth_or_array_layers: 1 });
        } else {
            self.queue.write_texture(texture.as_image_copy(), frame.pixels(), wgpu::TexelCopyBufferLayout {
                offset: 0, bytes_per_row: Some(size[0] * 4), rows_per_image: Some(size[1]),
            }, texture.size());
        }
        let view = texture.create_view(&Default::default());
        self.remember_texture(frame, texture, view.clone());
        view
    }

    fn remember_texture(&mut self, frame: &RgbaFrame, texture: wgpu::Texture, view: wgpu::TextureView) {
        let bytes = frame.pixels().len();
        if bytes > SOURCE_CACHE_BYTES { return; }
        let asset = frame.asset_source().map(|key| (key.clone(), frame.asset_revision()));
        self.sources.retain(|entry| {
            let keep = !entry.identity.matches(frame) && !asset.as_ref().zip(entry.asset.as_ref())
                .is_some_and(|((key, _), (old, _))| key == old);
            if !keep { self.source_bytes -= entry.bytes; }
            keep
        });
        while self.source_bytes + bytes > SOURCE_CACHE_BYTES || self.sources.len() >= SOURCE_CACHE_ENTRIES {
            self.source_bytes -= self.sources.pop_front().unwrap().bytes;
        }
        self.source_bytes += bytes;
        self.sources.push_back(CachedTexture { identity: frame.pixel_identity(), asset,
            size: [frame.width(), frame.height()], bytes, texture, view });
    }

    fn retain_full_target(&mut self, frame: &RgbaFrame) {
        let size = [frame.width(), frame.height()];
        let bytes = frame.pixels().len();
        let row_bytes = size[0] * 4;
        if self.full_target.take() != Some((size, frame.pixels().as_ptr() as usize))
            || bytes > SOURCE_CACHE_BYTES || !self.accepts_texture(frame)
            || !row_bytes.is_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
        {
            return;
        }
        let texture = rgba_texture(&self.device, size);
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_buffer_to_texture(wgpu::TexelCopyBufferInfo {
            buffer: &self.target, layout: wgpu::TexelCopyBufferLayout {
                offset: 0, bytes_per_row: Some(row_bytes), rows_per_image: Some(size[1]),
            },
        }, texture.as_image_copy(), texture.size());
        self.queue.submit(Some(encoder.finish()));
        let view = texture.create_view(&Default::default());
        self.remember_texture(frame, texture, view);
    }

    fn accepts_texture(&self, frame: &RgbaFrame) -> bool {
        !frame.is_empty() && frame.width().max(frame.height()) <= self.device.limits().max_texture_dimension_2d
    }

    fn atlas_limit(&self) -> u32 {
        self.device.limits().max_texture_array_layers.min(ATLAS_MAX_PAGES)
    }

    fn accepts_atlas(&self, frame: &RgbaFrame, overflow: Option<&VramOverflow>) -> bool {
        if frame.is_empty() { return false; }
        let dimensions = atlas_dimensions(frame, overflow);
        let grid = dimensions.map(|size| size.div_ceil(ATLAS_TILE));
        let count = u64::from(grid[0]) * u64::from(grid[1]);
        count * 4 <= u64::from(self.device.limits().max_storage_buffer_binding_size)
            && (count <= u64::from(self.atlas_limit()) || {
                let (id, revision) = atlas_version(overflow);
                self.atlas.as_ref().is_some_and(|atlas| atlas.id == id && atlas.revision == revision
                    && atlas.dimensions == dimensions && atlas.viewport.dimensions() == frame.dimensions())
                    || atlas_pages(frame, overflow, grid[0], None).len() <= self.atlas_limit() as usize
            })
    }

    fn atlas_texture(&mut self, viewport: &RgbaFrame, overflow: Option<&VramOverflow>) -> (wgpu::TextureView, wgpu::Buffer) {
        let (id, revision) = atlas_version(overflow);
        let dimensions = atlas_dimensions(viewport, overflow);
        let grid = dimensions.map(|size| size.div_ceil(ATLAS_TILE));
        let limit = self.atlas_limit();
        let mut dirty = BTreeSet::new();
        let previous = self.atlas.as_ref().filter(|atlas| atlas.id == id && atlas.dimensions == dimensions
            && atlas.viewport.dimensions() == viewport.dimensions());
        let unchanged = previous.is_some_and(|atlas| same_source(Some(&atlas.viewport), Some(viewport)));
        if let Some(previous) = previous.filter(|atlas| atlas.revision == revision && unchanged) {
            return (previous.view.clone(), previous.page_table.clone());
        }
        if let Some(previous) = previous {
            if !unchanged {
                visit_changed_viewport(&previous.viewport, viewport, &mut |origin, size, _, _| {
                    mark_pages(&mut dirty, grid[0], origin, size);
                });
            }
            if previous.revision != revision {
                dirty.extend(atlas_pages(viewport, overflow, grid[0], Some(previous.revision)));
            }
        }
        let reuse = previous.is_some_and(|atlas| {
            let additions = dirty.iter().filter(|index| !atlas.pages.contains_key(index)).count();
            atlas.pages.len() + additions <= atlas.texture.depth_or_array_layers() as usize
        });
        if !reuse {
            dirty = atlas_pages(viewport, overflow, grid[0], None);
            let capacity = (dirty.len() as u32).next_power_of_two().min(limit);
            let texture = atlas_array(&self.device, capacity);
            let view = texture.create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2Array), ..Default::default()
            });
            let page_table = buffer(&self.device, u64::from(grid[0]) * u64::from(grid[1]) * 4, UPLOAD);
            self.atlas = Some(AtlasTexture { id, revision, dimensions, viewport: viewport.clone(),
                texture, view, page_table, pages: BTreeMap::new() });
        }
        let atlas = self.atlas.as_mut().unwrap();
        atlas.viewport = viewport.clone();
        atlas.revision = revision;
        let mut pixels = vec![0u8; (ATLAS_TILE * ATLAS_TILE * 4) as usize];
        for page in dirty {
            let next_slot = atlas.pages.len() as u32;
            let slot = *atlas.pages.entry(page).or_insert_with(|| {
                self.queue.write_buffer(&atlas.page_table, u64::from(page) * 4, &(next_slot + 1).to_le_bytes());
                next_slot
            });
            let origin = [(page % grid[0]) * ATLAS_TILE, (page / grid[0]) * ATLAS_TILE];
            read_atlas_page(viewport, overflow, origin, &mut pixels);
            self.queue.write_texture(wgpu::TexelCopyTextureInfo {
                origin: wgpu::Origin3d { x: 0, y: 0, z: slot }, ..atlas.texture.as_image_copy()
            }, &pixels, wgpu::TexelCopyBufferLayout {
                offset: 0, bytes_per_row: Some(ATLAS_TILE * 4), rows_per_image: Some(ATLAS_TILE),
            }, wgpu::Extent3d { width: ATLAS_TILE, height: ATLAS_TILE, depth_or_array_layers: 1 });
        }
        (atlas.view.clone(), atlas.page_table.clone())
    }

    fn draw_jobs(
        &mut self, canvas: &mut Canvas, regions: &[RasterRect], draws: &[RasterJob<'_>],
        output: RasterOutput, retain_result: bool,
    ) -> Result<bool> {
        self.full_target = None;
        if self.failed.load(Ordering::Relaxed) {
            bail!("raster device is unavailable");
        }
        let bounds = RasterRect { origin: [0; 2], size: [canvas.width(), canvas.height()] };
        let direct = matches!(output, RasterOutput::Canvas) && self.direct_output_enabled();
        let full_region = [bounds];
        let regions = if direct { full_region.as_slice() } else { regions };
        let mut target_size = 0u64;
        let mut offsets = Vec::with_capacity(regions.len());
        for region in regions {
            if region.size.contains(&0) || !bounds.contains(*region) { return Ok(false); }
            offsets.push(target_size);
            target_size += region.pixels() * 4;
        }
        let limits = self.device.limits();
        let storage_limit = u64::from(limits.max_storage_buffer_binding_size);
        if draws.is_empty() || target_size == 0 || target_size > storage_limit {
            return Ok(false);
        }
        let arg_size = draws.len() as u64 * ARGS_BYTES;
        if arg_size > limits.max_buffer_size || arg_size > storage_limit { return Ok(false); }
        let mut parameters = vec![0; arg_size as usize];
        let mut sources = HashSet::new();
        let mut source_bytes = 0u64;
        let mut atlas_binding = None;
        let mut atlas_versions = Vec::with_capacity(draws.len());
        for (job, &RasterJob { rect, source, atlas, operation }) in draws.iter().enumerate() {
            let [left, top] = rect.origin;
            let [w, h] = rect.size;
            if atlas.is_none() && source.is_some_and(|frame| !self.accepts_texture(frame))
                || w == 0 || h == 0 || w.max(h).div_ceil(8) > limits.max_compute_workgroups_per_dimension
            {
                return Ok(false);
            }
            let Some((region, &offset)) = regions.iter().zip(&offsets).find(|(region, _)| region.contains(rect)) else {
                return Ok(false);
            };
            let source_size = match (source, atlas) {
                (Some(frame), Some(atlas)) if self.accepts_atlas(frame, atlas) => {
                    let (id, revision) = atlas_version(atlas);
                    let binding = (id, frame.pixels().as_ptr());
                    if atlas_binding.is_some_and(|bound| bound != binding) { return Ok(false); }
                    atlas_binding = Some(binding);
                    atlas_versions.push(Some((id, revision)));
                    atlas_dimensions(frame, atlas)
                }
                (_, Some(_)) => return Ok(false),
                (frame, None) => {
                    atlas_versions.push(None);
                    frame.map_or([0; 2], |frame| [frame.width(), frame.height()])
                }
            };
            if atlas.is_none() && let Some(frame) = source && sources.insert(frame.pixels().as_ptr()) {
                source_bytes += frame.pixels().len() as u64;
                if source_bytes > 64 * 1024 * 1024 { return Ok(false); }
            }
            let mut args = vec![region.origin[0], region.origin[1], region.size[0], (offset / 4) as u32,
                operation.code() as u32, left, top, w, h,
                source_size[0], source_size[1]];
            operation.append_args(&mut args);
            let start = job * ARGS_BYTES as usize;
            let length = args.len() * 4;
            if length > ARGS_BYTES as usize - 4 { return Ok(false); }
            for (bytes, word) in parameters[start..start + length].chunks_exact_mut(4).zip(args) {
                bytes.copy_from_slice(&word.to_le_bytes());
            }
            if atlas.is_some() {
                parameters[start + ARGS_BYTES as usize - 4] = 1;
            }
        }
        let stride = canvas.width() as usize * 4;
        let target_rect = regions[0];
        let [first, row_size] = byte_range(target_rect, stride);
        let retain_result = retain_result && regions.len() == 1 && target_size <= 16 * 1024 * 1024;
        let previous_device = direct.then(|| device_frame(canvas))
            .flatten().filter(|frame| Arc::ptr_eq(&frame.failed, &self.failed))
            .map(|frame| (frame.revision, frame.buffer.clone()));
        let before_revision = previous_device.as_ref().map(|(revision, _)| *revision);
        self.ensure_target_size(target_size);
        let input = if previous_device.is_some() {
            &[][..]
        } else if regions.len() == 1 && row_size == stride {
            &canvas.pixels()[first..first + target_size as usize]
        } else {
            self.scratch.clear();
            for region in regions {
                let [first, row_size] = byte_range(*region, stride);
                for row in 0..region.size[1] as usize {
                    let start = first + row * stride;
                    self.scratch.extend_from_slice(&canvas.pixels()[start..start + row_size]);
                }
            }
            &self.scratch
        };
        if retain_result && let Some(previous) = &self.previous_planes
            && previous.area == target_rect && previous.parameters == parameters
            && previous.before_revision == before_revision && previous.before == input
            && previous.inputs.iter().zip(draws.iter().zip(&atlas_versions)).all(|((old, version), (new, next_version))| {
                version == next_version && equal_pixels(old.as_ref(), new.source)
            })
        {
            match &previous.after {
                CompletedOutput::Cpu(pixels) => replace_rows(canvas, first, stride, row_size, pixels),
                CompletedOutput::Device(verified) if direct => *canvas = verified.clone(),
                CompletedOutput::Device(verified) => replace_rows(canvas, first, stride, row_size, verified.pixels()),
            }
            return Ok(true);
        }
        let mut encoder = self.device.create_command_encoder(&Default::default());
        let seed = if let Some((_, previous)) = previous_device {
            encoder.copy_buffer_to_buffer(&previous, 0, &self.target, 0, target_size);
            retain_result.then_some((Vec::new(), None))
        } else if retain_result {
            let previous = self.previous_planes.take();
            let reuse = previous.as_ref().is_some_and(|old| old.area == target_rect && old.seed.is_some()
                && old.before == input);
            let (mut before, seed) = previous.map(|old| (old.before, old.seed))
                .and_then(|(before, seed)| seed.filter(|seed| seed.size() == target_size).map(|seed| (before, seed)))
                .unwrap_or_else(|| (Vec::new(), buffer(&self.device, target_size,
                    wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC)));
            if !reuse {
                before.clear();
                before.extend_from_slice(input);
                self.queue.write_buffer(&seed, 0, input);
            }
            encoder.copy_buffer_to_buffer(&seed, 0, &self.target, 0, target_size);
            Some((before, Some(seed)))
        } else {
            self.queue.write_buffer(&self.target, 0, input);
            None
        };
        if self.args_buffer.size() < arg_size {
            self.args_buffer = buffer(&self.device, arg_size, UPLOAD);
        }
        self.queue.write_buffer(&self.args_buffer, 0, &parameters);
        let mut first_job = 0;
        while first_job < draws.len() {
            let RasterJob { source, atlas, operation, .. } = draws[first_job];
            let (source_texture, atlas_texture, page_table) = match (source, atlas) {
                (Some(viewport), Some(overflow)) => {
                    let (texture, table) = self.atlas_texture(viewport, overflow);
                    (self.empty_source.clone(), texture, table)
                }
                _ => (self.source_texture(source), self.empty_atlas.clone(), self.empty_page_table.clone()),
            };
            let mask_view = match operation {
                Draw::Wipe { mask, .. } => self.source_texture(Some(mask)),
                _ => self.empty_source.clone(),
            };
            let [groups_x, groups_y] = draws[first_job].rect.workgroups();
            let mut end_job = first_job + 1;
            // Disjoint rectangles can share a dispatch. Overlapping rectangles
            // must dispatch in order because blending reads the prior result.
            while let Some(&RasterJob { rect, source: next, atlas: next_atlas, .. }) = draws.get(end_job) {
                if rect.workgroups() != [groups_x, groups_y]
                    || !same_source(source, next)
                    || atlas.is_some() != next_atlas.is_some()
                    || matches!(operation, Draw::Wipe { .. })
                    || matches!(draws[end_job].operation, Draw::Wipe { .. })
                    || draws[first_job..end_job].iter().any(|earlier| earlier.rect.overlaps(rect))
                {
                    break;
                }
                end_job += 1;
            }
            let jobs = (end_job - first_job) as u32;
            let bindings = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("CS2 raster buffers"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: self.target.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&source_texture) },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                            buffer: &self.args_buffer, offset: first_job as u64 * ARGS_BYTES,
                            size: std::num::NonZeroU64::new(u64::from(jobs) * ARGS_BYTES),
                        }),
                    },
                    wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&atlas_texture) },
                    wgpu::BindGroupEntry { binding: 4, resource: page_table.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::TextureView(&mask_view) },
                ],
            });
            let timestamp_writes = self.timing.as_ref().filter(|_| !direct && (first_job == 0 || end_job == draws.len()))
                .map(|timing| wgpu::ComputePassTimestampWrites {
                    query_set: &timing.queries,
                    beginning_of_pass_write_index: (first_job == 0).then_some(0),
                    end_of_pass_write_index: (end_job == draws.len()).then_some(1),
                });
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: None, timestamp_writes });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bindings, &[]);
            pass.dispatch_workgroups(groups_x, groups_y, jobs);
            first_job = end_job;
        }
        if direct {
            self.queue.submit(Some(encoder.finish()));
            let frame = self.publish_device_frame(bounds.size);
            canvas.set_deferred_device_pixels(frame);
            if let Some((before, seed)) = seed {
                self.previous_planes = Some(CompletedPlanes {
                    area: target_rect, before, before_revision, seed, parameters,
                    after: CompletedOutput::Device(canvas.clone()),
                    inputs: draws.iter().zip(atlas_versions).map(|(job, version)| (job.source.cloned(), version)).collect(),
                });
            }
            return Ok(true);
        }
        encoder.copy_buffer_to_buffer(&self.target, 0, &self.readback, 0, target_size);
        let map_size = if let Some(timing) = self.timing.as_ref().filter(|_| !direct) {
            encoder.resolve_query_set(&timing.queries, 0..2, &timing.resolve, 0);
            encoder.copy_buffer_to_buffer(&timing.resolve, 0, &self.readback, target_size, TIMING_BYTES);
            target_size + TIMING_BYTES
        } else {
            target_size
        };
        let measurement = ReadbackTimer::start(ReadbackKind::Packed, map_size);
        let submission = self.queue.submit(Some(encoder.finish()));
        let slice = self.readback.slice(..map_size);
        let (sender, receiver) = mpsc::sync_channel(1);
        slice.map_async(wgpu::MapMode::Read, move |result| { let _ = sender.send(result); });
        let ready = self.device.poll(wgpu::PollType::Wait {
            submission_index: Some(submission),
            timeout: Some(Duration::from_millis(250)),
        }).context("wait for raster readback")
            .and_then(|_| receiver.try_recv().context("receive raster readback")?.context("map raster readback"))
            .and_then(|_| {
                if self.failed.load(Ordering::Relaxed) {
                    bail!("raster device failed during dispatch");
                }
                Ok(())
            });
        if let Err(error) = ready {
            self.readback.unmap();
            return Err(error);
        }
        {
            let output = slice.get_mapped_range();
            if let Some(timing) = &mut self.timing {
                let timestamps = &output[target_size as usize..];
                let start = u64::from_le_bytes(timestamps[..8].try_into().unwrap());
                let end = u64::from_le_bytes(timestamps[8..].try_into().unwrap());
                if let Some(ticks) = end.checked_sub(start).filter(|&ticks| ticks != 0) {
                    let ns = (ticks as f64 * f64::from(self.queue.get_timestamp_period())).round() as u64;
                    timing.latest = Some((timing.latest.map_or(1, |(sequence, _)| sequence + 1), ns));
                }
            }
            for (&region, offset) in regions.iter().zip(offsets) {
                let [first, row_size] = byte_range(region, stride);
                let at = offset as usize;
                replace_rows(canvas, first, stride, row_size, &output[at..at + region.pixels() as usize * 4]);
            }
            if let Some((before, seed)) = seed {
                self.previous_planes = Some(CompletedPlanes {
                    area: target_rect, parameters, before, before_revision, seed,
                    after: CompletedOutput::Cpu(output[..target_size as usize].to_vec()),
                    inputs: draws.iter().zip(atlas_versions).map(|(job, version)| (job.source.cloned(), version)).collect(),
                });
            }
        }
        self.readback.unmap();
        if let Some(measurement) = measurement { measurement.complete(); }
        if regions == [bounds] {
            self.full_target = Some((bounds.size, canvas.pixels().as_ptr() as usize));
        }
        Ok(true)
    }

    fn publish_device_frame(&self, size: [u32; 2]) -> Arc<DeviceFrame> {
        static NEXT_FRAME: AtomicU64 = AtomicU64::new(1);
        Arc::new(DeviceFrame {
            revision: NEXT_FRAME.fetch_add(1, Ordering::Relaxed), size,
            buffer: self.target.clone(), device: self.device.clone(), queue: self.queue.clone(),
            failed: self.failed.clone(),
        })
    }

    fn ensure_target_size(&mut self, size: u64) {
        if self.target.size() < size || Arc::strong_count(&self.target) != 1 {
            let target = self.spares.iter()
                .position(|slot| slot.size() >= size && Arc::strong_count(slot) == 1)
                .map(|index| self.spares.swap_remove(index))
                .unwrap_or_else(|| Arc::new(buffer(&self.device, size, TARGET)));
            let previous = std::mem::replace(&mut self.target, target);
            if self.spares.len() < 3 { self.spares.push(previous); }
        }
        if self.readback.size() < size + TIMING_BYTES {
            self.readback = buffer(&self.device, size + TIMING_BYTES, DOWNLOAD);
        }
    }
}

fn atlas_dimensions(frame: &RgbaFrame, overflow: Option<&VramOverflow>) -> [u32; 2] {
    let [width, height] = overflow.map_or([0; 2], VramOverflow::dimensions);
    [width.max(frame.width()), height.max(frame.height())]
}

fn atlas_version(overflow: Option<&VramOverflow>) -> (u64, u64) {
    overflow.map_or((0, 0), VramOverflow::gpu_version)
}

fn atlas_array(device: &wgpu::Device, pages: u32) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("CS2 canvas pages"),
        size: wgpu::Extent3d { width: ATLAS_TILE, height: ATLAS_TILE, depth_or_array_layers: pages },
        mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Uint,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

fn mark_pages(pages: &mut BTreeSet<u32>, columns: u32, [x, y]: [u32; 2], [w, h]: [u32; 2]) {
    if w == 0 || h == 0 { return; }
    for row in y / ATLAS_TILE..=(y + h - 1) / ATLAS_TILE {
        for col in x / ATLAS_TILE..=(x + w - 1) / ATLAS_TILE {
            pages.insert(row * columns + col);
        }
    }
}

fn atlas_pages(
    frame: &RgbaFrame, overflow: Option<&VramOverflow>, columns: u32, after: Option<u64>,
) -> BTreeSet<u32> {
    let mut pages = BTreeSet::new();
    if after.is_none() {
        mark_pages(&mut pages, columns, [0; 2], [frame.width(), frame.height()]);
    }
    if let Some(overflow) = overflow {
        overflow.visit_gpu_tiles(after, |[x, y], [w, h], _| {
            let viewport_width = frame.width().saturating_sub(x).min(w);
            let viewport_height = frame.height().saturating_sub(y).min(h);
            mark_pages(&mut pages, columns, [x + viewport_width, y], [w - viewport_width, h]);
            mark_pages(&mut pages, columns, [x, y + viewport_height], [viewport_width, h - viewport_height]);
        });
    }
    pages
}

fn read_atlas_page(frame: &RgbaFrame, overflow: Option<&VramOverflow>, [x, y]: [u32; 2], pixels: &mut [u8]) {
    pixels.fill(0);
    let stride = frame.width() as usize * 4;
    for (dy, row) in pixels.chunks_exact_mut((ATLAS_TILE * 4) as usize).enumerate() {
        let y = y + dy as u32;
        let visible = if y < frame.height() { frame.width().saturating_sub(x).min(ATLAS_TILE) } else { 0 };
        if visible != 0 {
            let at = y as usize * stride + x as usize * 4;
            row[..(visible * 4) as usize].copy_from_slice(&frame.pixels()[at..at + (visible * 4) as usize]);
        }
        if let Some(overflow) = overflow {
            let mut written = visible;
            while written < ATLAS_TILE {
                let (count, source) = overflow.row_span(x + written, y, ATLAS_TILE - written);
                if let Some(source) = source {
                    row[(written * 4) as usize..((written + count) * 4) as usize].copy_from_slice(source);
                }
                written += count;
            }
        }
    }
}

fn visit_changed_viewport(
    old: &RgbaFrame, new: &RgbaFrame,
    visit: &mut impl FnMut([u32; 2], [u32; 2], &[u8], u32),
) {
    const TILE: u32 = 128;
    let stride = new.width() * 4;
    let old_pixels = old.pixels();
    let new_pixels = new.pixels();
    for y in (0..new.height()).step_by(TILE as usize) {
        let height = TILE.min(new.height() - y);
        for x in (0..new.width()).step_by(TILE as usize) {
            let width = TILE.min(new.width() - x);
            let start = (y * stride + x * 4) as usize;
            let changed = (0..height).any(|row| {
                let at = start + (row * stride) as usize;
                let end = at + (width * 4) as usize;
                old_pixels[at..end] != new_pixels[at..end]
            });
            if changed { visit([x, y], [width, height], &new_pixels[start..], stride); }
        }
    }
}

fn rgba_texture(device: &wgpu::Device, [width, height]: [u32; 2]) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("CS2 immutable RGBA texture"), size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Uint,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

fn byte_range(region: RasterRect, stride: usize) -> [usize; 2] {
    [region.origin[1] as usize * stride + region.origin[0] as usize * 4, region.size[0] as usize * 4]
}

fn same_source(left: Option<&RgbaFrame>, right: Option<&RgbaFrame>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => std::ptr::eq(left.pixels(), right.pixels()),
        (None, None) => true,
        _ => false,
    }
}

fn equal_pixels(left: Option<&RgbaFrame>, right: Option<&RgbaFrame>) -> bool {
    same_source(left, right) || left.zip(right).is_some_and(|(left, right)| left.pixels() == right.pixels())
}

fn replace_rows(canvas: &mut Canvas, first: usize, stride: usize, row_size: usize, bytes: &[u8]) {
    let pixels = canvas.pixels_mut();
    if row_size == stride {
        pixels[first..first + bytes.len()].copy_from_slice(bytes);
    } else {
        for (row, source) in bytes.chunks_exact(row_size).enumerate() {
            let start = first + row * stride;
            pixels[start..start + row_size].copy_from_slice(source);
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn missing_emote_raster_reason_distinguishes_lifecycle_states() {
        assert_eq!(
            missing_emote_raster_reason(RasterStatus::NotCreated),
            "Raster has not been created"
        );
        assert_eq!(
            missing_emote_raster_reason(RasterStatus::InitializationFailed),
            "Raster initialization failed"
        );
        assert!(missing_emote_raster_reason(RasterStatus::Disabled).contains("disabled"));
        assert!(missing_emote_raster_reason(RasterStatus::Ready).contains("empty"));
    }

    #[test]
    #[ignore = "requires a DX12 adapter"]
    fn direct_draw_keeps_verified_pixels_across_layers() -> Result<()> {
        let mut raster = Raster::new()?;
        let mut packed = Canvas::new(1280, 720)?;
        let mut direct = packed.clone();
        let rect = RasterRect { origin: [0; 2], size: [1280, 720] };
        let source = RgbaFrame::from_rgba8(1280, 720, [26, 91, 153, 211].repeat(1280 * 720))?;
        let mut previous = None;
        for alpha in [179, 223] {
            let jobs = [RasterJob { rect, source: Some(&source), atlas: None,
                operation: Draw::DiffuseFrame { alpha } }];
            raster.test_direct_output = false;
            assert!(raster.draw_jobs(&mut packed, &[rect], &jobs, RasterOutput::CpuPixels, false)?);
            raster.test_direct_output = true;
            assert!(raster.draw_jobs(&mut direct, &[rect], &jobs, RasterOutput::Canvas, false)?);
            assert_eq!(packed.pixels(), direct.pixels());
            let frame = device_frame(&direct).context("missing direct output")?;
            assert_ne!(previous, Some(frame.revision));
            previous = Some(frame.revision);
            let mut pixels = vec![0; 1280 * 720 * 4];
            assert!(frame.read_rgba(&mut pixels));
            assert_eq!(pixels, direct.pixels());
        }
        Ok(())
    }

    #[test]
    #[ignore = "requires a DX12 adapter"]
    fn small_device_plane_matches_cpu_for_every_blend_mode() -> Result<()> {
        let mut raster = Raster::new()?;
        raster.test_direct_output = true;
        let background: Vec<u8> = (0..64 * 64).flat_map(|index| {
            let value = index as u8;
            [value, value.wrapping_add(29), value.wrapping_add(61), match index % 4 { 0 => 0, 1 => 255, _ => 191 }]
        }).collect();
        let source_rgba: Vec<u8> = (0..16 * 16).flat_map(|index| {
            let value = index as u8;
            [value.wrapping_add(17), value.wrapping_add(43), value, [0, 1, 128, 255][index % 4]]
        }).collect();
        let background_frame = RgbaFrame::from_rgba8(64, 64, background.clone())?;
        let source = RgbaFrame::from_rgba8(16, 16, source_rgba)?;
        let full = RasterRect { origin: [0; 2], size: [64, 64] };
        RASTER.with_borrow_mut(|slot| *slot = Some(raster));
        for blend in [
            BlendMode::SourceOver, BlendMode::AddSource, BlendMode::AddDestination,
            BlendMode::Multiply, BlendMode::InvertDestination, BlendMode::AddMultiply,
            BlendMode::ListAlphaFromRed, BlendMode::ListAlphaFromGreen,
            BlendMode::ListAlphaFromBlue, BlendMode::ListAlphaFromAlpha,
        ] {
            let mut expected = Canvas::from_rgba8(64, 64, background.clone())?;
            let mut actual = expected.clone();
            let seed = [RasterJob { rect: full, source: Some(&background_frame), atlas: None,
                operation: Draw::Copy { source_origin: [0; 2] } }];
            assert!(RASTER.with_borrow_mut(|slot| slot.as_mut().unwrap()
                .draw_jobs(&mut actual, &[full], &seed, RasterOutput::Canvas, false))?);
            let rect = RasterRect { origin: [3, 5], size: [16, 16] };
            assert!(draw_job(&mut actual, RasterJob { rect, source: Some(&source), atlas: None,
                operation: Draw::PlaneSpan { source_origin: [0; 2], opacity: 173, blend } }));
            // The reference applies the same opacity before the integer blend.
            expected = Canvas::from_rgba8(64, 64, background.clone())?;
            let modulated: Vec<u8> = source.pixels().chunks_exact(4).flat_map(|pixel| {
                [pixel[0], pixel[1], pixel[2], ((u32::from(pixel[3]) * 173) / 255) as u8]
            }).collect();
            expected.blend_rgba_rows(Rect::new(3, 5, 16, 16), &modulated, 16 * 4, blend)?;
            assert_eq!(actual.pixels(), expected.pixels(), "{blend:?}");
        }
        RASTER.with_borrow_mut(Option::take);
        Ok(())
    }
}

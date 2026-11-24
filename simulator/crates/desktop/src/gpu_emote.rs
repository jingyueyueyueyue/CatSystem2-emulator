//! E-mote pose raster on the shared device. Indexed hardware coverage is the
//! primary path; f64 scanline/compute remains a compatibility fallback. Ordered
//! compute composition preserves straight RGBA, serial masks and layer order.

use super::{DOWNLOAD, RASTER, Raster, TARGET, UPLOAD, blend_code, buffer, device_frame, report};
use crate::perf_detail::{ReadbackKind, ReadbackTimer, emote_gpu_pose, emote_gpu_start, emote_gpu_sync};
use anyhow::{Context, Result, bail};
use cs2_renderer::{BlendMode, Canvas};
use cs2_runtime::emote::{
    CpuMotionModel, CpuPlayer, CpuPlayerRenderState, CpuStencilBuffer, CpuStencilWrite, CpuTexture, CpuViewport, EmoteDrawTarget,
    EmoteDeviceReadback, EmoteError, EmoteRasterFrame, EmoteRepairScratch, EmoteSpriteMesh, EmoteSpritePass,
    EmoteTextureSpan, OwnedEmoteSpriteMesh, repair_emote_raster, visit_emote_texture_spans,
};
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::num::NonZeroU64;
use std::sync::{Arc, Mutex, OnceLock, Weak, atomic::{AtomicBool, Ordering}, mpsc};
use std::time::{Duration, Instant};

#[path = "gpu_emote_mesh.rs"]
mod mesh;
#[path = "emote_pose_jobs.rs"]
mod pose_jobs;
pub(in crate) use pose_jobs::{PoseScheduler, ScenePose, ScenePoseContext, pose_jobs_available};
#[cfg(test)]
pub(in crate) use pose_jobs::PoseTestGate;

thread_local! {
    static BATCHING: Cell<bool> = const { Cell::new(false) };
    static CAPTURING: Cell<bool> = const { Cell::new(false) };
    static REQUIRE_RESIDENT_ATLASES: Cell<bool> = const { Cell::new(false) };
    static PREPARED: RefCell<Vec<PreparedPose>> = const { RefCell::new(Vec::new()) };
    static POSE_BATCHES: RefCell<Vec<CommandBatch>> = const { RefCell::new(Vec::new()) };
    static INDEXED_CACHES: RefCell<Vec<IndexedPoseCache>> = const { RefCell::new(Vec::new()) };
    #[cfg(test)]
    static INDEXED_RENDER_PATHS: Cell<[usize; 2]> = const { Cell::new([0; 2]) };
}

fn batching() -> bool {
    BATCHING.with(Cell::get)
}

fn mesh_preview() -> bool { batching() || CAPTURING.with(Cell::get) }

struct EndpointCapture(bool);
impl Drop for EndpointCapture {
    fn drop(&mut self) { CAPTURING.with(|capture| capture.set(self.0)); }
}

pub(in crate) struct EmoteBatch {
    active: bool,
}

pub(in crate) fn begin_emote_batch() -> Result<EmoteBatch> {
    RASTER.with_borrow_mut(|slot| {
        if let Some(emote) = slot.as_mut().and_then(|raster| raster.emote.as_mut()) {
            if !emote.pending.is_empty() {
                bail!("E-mote batch started with unfinished poses");
            }
            emote.begin_atlas_batch();
        }
        BATCHING.with(|batching| batching.set(true));
        Ok(EmoteBatch { active: true })
    })
}

impl EmoteBatch {
    pub(in crate) fn seal(&self) {
        BATCHING.with(|batching| batching.set(false));
        PREPARED.with_borrow_mut(Vec::clear);
        RASTER.with_borrow(|slot| {
            if let Some(raster) = slot.as_ref()
                && let Some(mesh) = raster.emote.as_ref().and_then(|emote| emote.mesh.as_ref())
            {
                mesh.submission.flush(&raster.queue);
            }
        });
    }

    pub(in crate) fn finish(mut self) -> Result<()> {
        self.active = false;
        finish_emote_batch()
    }
}

impl Drop for EmoteBatch {
    fn drop(&mut self) {
        if self.active {
            let _ = finish_emote_batch();
        }
    }
}

fn finish_emote_batch() -> Result<()> {
    BATCHING.with(|batching| batching.set(false));
    PREPARED.with_borrow_mut(Vec::clear);
    RASTER.with_borrow_mut(|slot| {
        let Some(raster) = slot.as_mut() else { return Ok(()) };
        let Some(emote) = raster.emote.as_mut() else { return Ok(()) };
        emote.finish_pending(&raster.device, &raster.queue)
    })
}

fn report_missing_raster_once() {
    static REPORTED: AtomicBool = AtomicBool::new(false);
    if !REPORTED.swap(true, Ordering::Relaxed) {
        report(&format!(
            "draw=emote backend=cpu reason={}",
            super::emote_missing_raster_reason()
        ));
    }
}

pub(in crate) fn prewarm_emote() {
    RASTER.with_borrow_mut(|slot| {
        if let Some(raster) = slot.as_mut() {
            let started = Instant::now();
            match raster.ensure_emote() {
                Ok(()) => report(&format!("emote_prewarm_us={}", started.elapsed().as_micros())),
                Err(error) => report(&format!("emote_prewarm_failed={error:#}")),
            }
        }
    });
}

/// Use one bounded upload for each new logical frame, before its script runs.
pub(in crate) fn stage_emote_model(model: &Arc<CpuMotionModel>) {
    RASTER.with_borrow_mut(|slot| {
        let Some(raster) = slot.as_mut() else { return };
        let Some(emote) = raster.emote.as_mut() else { return };
        let started = Instant::now();
        emote.atlas_upload_window.refresh(started);
        let budget = emote.atlas_transfers.allowance(&raster.device,
            emote.atlas_upload_window.speculative_remaining, Duration::ZERO);
        if budget == 0 { return; }
        match emote.stage_atlas_with_budget(&raster.device, &raster.queue, model, budget) {
            Ok(0) => {}
            Ok(bytes) => {
                emote.atlas_upload_window.speculative_remaining -= bytes;
                let submitted = super::submit_gpu_commands(&raster.queue, []);
                emote.atlas_transfers.submitted(submitted, bytes);
                report(&format!("emote_atlas_stage_bytes={bytes} submit_us={}", started.elapsed().as_micros()));
            }
            Err(error) => report(&format!("emote_atlas_stage_failed={error:#}")),
        }
    });
}

/// Wgpu uses one bounded readback per pose. The manager keeps the logical
/// viewport dimensions for its held images and cross-fade surfaces.
pub(in crate) fn draw_emote(
    player: &CpuPlayer, viewport: CpuViewport, variables: &BTreeMap<String, f64>,
) -> Option<EmoteRasterFrame> {
    RASTER.with_borrow_mut(|slot| {
        let Some(raster) = slot.as_mut() else {
            report_missing_raster_once();
            return None;
        };
        let result = raster.render_emote(player, viewport, variables, false);
        let success = result.is_ok();
        if raster.reported_emote_result != Some(success) {
            raster.reported_emote_result = Some(success);
            match &result {
                Ok(_) => report(&format!("draw=emote backend={}", raster.backend)),
                Err(error) => report(&format!("draw=emote backend=cpu reason={error:#}")),
            }
        }
        result.ok()
    })
}

pub(in crate) fn blend_emote(
    canvas: &mut Canvas, frame: &EmoteRasterFrame, blend: BlendMode,
) -> Result<bool> {
    let Some(pose) = frame.device().and_then(|device| device.as_any().downcast_ref::<DevicePose>()) else {
        return Ok(false);
    };
    RASTER.with_borrow_mut(|slot| {
        let Some(raster) = slot.as_mut() else { return Ok(false) };
        raster.blend_emote(canvas, frame, pose, blend)
    })
}

/// Modulate the completed manager surface, preserving integer rounding and
/// transparent RGB. A CPU frame or a retired device deliberately returns None.
pub(in crate) fn tint_emote_surface(frame: &EmoteRasterFrame, argb: u32) -> Option<EmoteRasterFrame> {
    if !super::emote_device_blend_requested() { return None; }
    RASTER.with_borrow_mut(|slot| {
        let raster = slot.as_mut()?;
        let source = raster.fade_device_pose(frame)?.buffer.clone();
        let size = [frame.width(), frame.height()];
        let bytes = fade_extent(&raster.device, size)?;
        let emote = raster.emote.as_mut()?;
        let output = emote.fade_output(&raster.device, bytes);
        let params = [size[0], size[1], argb, 0, 0, 0, 0, 0];
        let params: Vec<_> = params.into_iter().flat_map(u32::to_le_bytes).collect();
        raster.queue.write_buffer(&emote.fade_params, 0, &params);
        let bindings = raster.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("CS2 E-mote final tint"), layout: &emote.tint_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: emote.fade_params.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: source.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: output.as_entire_binding() },
            ],
        });
        dispatch_fade(&raster.device, &raster.queue, &emote.tint, &bindings, size);
        raster.bind_fade_output(size, frame.origin(), output, bytes, None)
    })
}

pub(in crate) fn mix_emote_surfaces(
    old: &EmoteRasterFrame, new: &EmoteRasterFrame, weight: u32, transparent: Option<bool>,
) -> Option<EmoteRasterFrame> {
    if !super::emote_device_blend_requested() { return None; }
    RASTER.with_borrow_mut(|slot| {
        let raster = slot.as_mut()?;
        if !raster.direct_output_enabled() || raster.failed.load(Ordering::Relaxed) { return None; }
        let device = &raster.device;
        let size = [old.width(), old.height()];
        if size != [new.width(), new.height()] || old.origin() != new.origin() {
            return None;
        }
        let bytes = fade_extent(device, size)?;
        let alpha = if transparent.is_none() {
            Some(raster.fade_device_pose(new)?.fade_alpha.as_ref()?.clone())
        } else { None };
        let emote = raster.emote.as_mut()?;
        let queue = &raster.queue;
        let old_buffer = emote.fade_source(device, queue, &raster.failed, old)?;
        let new_buffer = emote.fade_source(device, queue, &raster.failed, new)?;
        let output = emote.fade_output(device, bytes);
        let params = [size[0], size[1], weight, transparent.map_or(2, u32::from), 0, 0, 0, 0];
        let params: Vec<_> = params.into_iter().flat_map(u32::to_le_bytes).collect();
        queue.write_buffer(&emote.fade_params, 0, &params);
        let bindings = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("CS2 E-mote surface fade"), layout: &emote.fade_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: emote.fade_params.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: old_buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: new_buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: output.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 6, resource: alpha.as_deref().unwrap_or(&emote.fade_empty_alpha).as_entire_binding() },
            ],
        });
        dispatch_fade(device, queue, &emote.fade, &bindings, size);
        static REPORTED: AtomicBool = AtomicBool::new(false);
        if !REPORTED.swap(true, Ordering::Relaxed) {
            report(&format!("draw=emote_surface_fade backend={}", raster.backend));
        }
        raster.bind_fade_output(size, old.origin(), output, bytes, None)
    })
}

pub(in crate) fn expand_emote_surface(
    frame: &EmoteRasterFrame, size: [u32; 2],
) -> Option<EmoteRasterFrame> {
    if !super::emote_device_blend_requested() { return None; }
    RASTER.with_borrow_mut(|slot| {
        let raster = slot.as_mut()?;
        let old = raster.fade_device_pose(frame)?;
        if [frame.width(), frame.height()] == size && frame.origin() == [0, 0] {
            return Some(frame.clone());
        }
        let [width, height] = size;
        let bytes = fade_extent(&raster.device, size)?;
        fade_extent(&raster.device, [frame.width(), frame.height()])?;
        let emote = raster.emote.as_mut()?;
        let output = emote.fade_output(&raster.device, bytes);
        let [x, y] = frame.origin();
        let params = [width, height, 0, 0, x as u32, y as u32, frame.width(), frame.height()];
        let params: Vec<_> = params.into_iter().flat_map(u32::to_le_bytes).collect();
        raster.queue.write_buffer(&emote.fade_params, 0, &params);
        let bindings = raster.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("CS2 E-mote surface expand"), layout: &emote.fade_expand_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: emote.fade_params.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: old.buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: output.as_entire_binding() },
            ],
        });
        dispatch_fade(&raster.device, &raster.queue, &emote.fade_expand, &bindings, size);
        raster.bind_fade_output(size, [0, 0], output, bytes, None)
    })
}

pub(in crate) fn draw_emote_surface(
    player: &CpuPlayer, viewport: CpuViewport, variables: &BTreeMap<String, f64>,
) -> Option<EmoteRasterFrame> {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    let enabled = *ENABLED.get_or_init(|| {
        #[cfg(windows)]
        let enabled = !std::env::var("CS2_EMOTE_DEVICE_ENDPOINT").is_ok_and(|value| value == "0");
        #[cfg(target_os = "android")]
        let enabled = !super::android_property(c"debug.cs2.emote_device_endpoint", |value| value == b"0")
            .unwrap_or(false);
        enabled
    });
    if !enabled || !super::emote_device_blend_requested() { return None; }
    if !RASTER.with_borrow(|slot| slot.as_ref().is_some_and(|raster|
        raster.direct_output_enabled() && !raster.failed.load(Ordering::Relaxed))) { return None; }
    // Endpoint capture chooses the pure GPU shaders without changing batch
    // ownership. A span fallback still finishes synchronously when necessary;
    // it cannot strand a pending readback after this guard restores the flag.
    let _capture = EndpointCapture(CAPTURING.with(|capture| capture.replace(true)));
    let frame = draw_emote(player, viewport, variables)?;
    expand_emote_surface(&frame, [viewport.width, viewport.height])
}

pub(in crate) fn prepare_emote_surfaces(
    old: &EmoteRasterFrame, new: &EmoteRasterFrame,
) -> Option<(EmoteRasterFrame, EmoteRasterFrame, Option<bool>)> {
    if !super::emote_device_blend_requested() || (old.device().is_none() && new.device().is_none()) { return None; }
    RASTER.with_borrow_mut(|slot| {
        let raster = slot.as_mut()?;
        if !raster.direct_output_enabled() || raster.failed.load(Ordering::Relaxed) { return None; }
        let size = [old.width(), old.height()];
        if size != [new.width(), new.height()] || old.origin() != new.origin() {
            return None;
        }
        let [width, height] = size;
        let bytes = fade_extent(&raster.device, size)?;
        let cpu_transparent = new.device().is_none().then(|| !new.rgba().chunks_exact(4).any(|pixel| pixel[3] != 0));
        let emote = raster.emote.as_mut()?;
        let prior = emote.fade_source(&raster.device, &raster.queue, &raster.failed, old)?;
        let next = emote.fade_source(&raster.device, &raster.queue, &raster.failed, new)?;
        let adjusted_old = emote.fade_output(&raster.device, bytes);
        let adjusted_new = emote.fade_output(&raster.device, bytes);
        let alpha = cpu_transparent.is_none().then(|| Arc::new(buffer(&raster.device, 4, TARGET)));
        let params = [width, height, 0, 0, 0, 0, 0, 0];
        let params: Vec<_> = params.into_iter().flat_map(u32::to_le_bytes).collect();
        raster.queue.write_buffer(&emote.fade_params, 0, &params);
        let bindings = raster.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("CS2 E-mote surface prepare"), layout: &emote.fade_prepare_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: emote.fade_params.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: prior.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: next.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: adjusted_old.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 4, resource: adjusted_new.as_entire_binding() },
            ],
        });
        dispatch_fade(&raster.device, &raster.queue, &emote.fade_prepare, &bindings, size);
        if let Some(alpha) = &alpha {
            let bindings = raster.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("CS2 E-mote capture alpha"), layout: &emote.fade_alpha_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: emote.fade_params.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 2, resource: next.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 5, resource: alpha.as_entire_binding() },
                ],
            });
            dispatch_fade(&raster.device, &raster.queue, &emote.fade_alpha, &bindings, size);
        }
        Some((
            raster.bind_fade_output(size, old.origin(), adjusted_old, bytes, None)?,
            raster.bind_fade_output(size, new.origin(), adjusted_new, bytes, alpha)?,
            cpu_transparent,
        ))
    })
}

fn fade_extent(device: &wgpu::Device, [width, height]: [u32; 2]) -> Option<u64> {
    let bytes = u64::from(width).checked_mul(u64::from(height))?.checked_mul(4)?;
    let limits = device.limits();
    (bytes != 0
        && bytes <= cs2_runtime::emote::MAX_FRAME_BYTES as u64
        && bytes <= u64::from(limits.max_storage_buffer_binding_size)
        && [width, height].into_iter().all(|axis| axis.div_ceil(8) <= limits.max_compute_workgroups_per_dimension))
        .then_some(bytes)
}

fn dispatch_fade(
    device: &wgpu::Device, queue: &wgpu::Queue, pipeline: &wgpu::ComputePipeline,
    bindings: &wgpu::BindGroup, [width, height]: [u32; 2],
) {
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_compute_pass(&Default::default());
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, bindings, &[]);
        pass.dispatch_workgroups(width.div_ceil(8), height.div_ceil(8), 1);
    }
    super::submit_gpu_commands(queue, Some(encoder.finish()));
}

impl Raster {
    fn fade_device_pose<'a>(&self, frame: &'a EmoteRasterFrame) -> Option<&'a DevicePose> {
        if !self.direct_output_enabled() || self.failed.load(Ordering::Relaxed) { return None; }
        let pose = frame.device()?.as_any().downcast_ref::<DevicePose>()?;
        if !Arc::ptr_eq(&self.failed, &pose.failed) { return None; }
        pose.flush_mesh();
        Some(pose)
    }

    fn bind_fade_output(
        &self, size: [u32; 2], origin: [i32; 2], buffer: Arc<wgpu::Buffer>, bytes: u64,
        fade_alpha: Option<Arc<wgpu::Buffer>>,
    ) -> Option<EmoteRasterFrame> {
        EmoteRasterFrame::from_device(size[0], size[1], origin, Arc::new(DevicePose {
            buffer, device: self.device.clone(), queue: self.queue.clone(), bytes,
            failed: self.failed.clone(), fade_alpha, mesh_submission: None,
        })).ok()
    }
}

const ATLAS_STAGE_LIMIT: usize = 256 * 1024 * 1024;
const MAX_ATLAS_BYTES: usize = 128 * 1024 * 1024;
// Active scene demand has a separate hard budget from speculative uploads.
// When it cannot fit, decline GPU work rather than evict another active atlas.
const ATLAS_DRAW_LIMIT: usize = 256 * 1024 * 1024;
const ATLAS_STAGE_BYTES: usize = 2 * 1024 * 1024;
// The speculative Start phase can use the other 2 MiB of the frame's upload budget.
const SCENE_ATLAS_STAGE_BYTES: usize = 6 * 1024 * 1024;
const ATLAS_TRANSFER_BYTES: usize = ATLAS_STAGE_BYTES + SCENE_ATLAS_STAGE_BYTES;
const ATLAS_IN_FLIGHT_BYTES: usize = 3 * ATLAS_TRANSFER_BYTES;
const ATLAS_UPLOAD_WINDOW: Duration = Duration::from_nanos(16_666_667);
const COMMAND_BYTES: usize = 16 * 1024 * 1024;
const COMMAND_WORDS: usize = COMMAND_BYTES / size_of::<u32>();
const MAX_DRAWS: usize = 1024;
const MAX_PREP_WORKERS: usize = 4;
const MAX_INDEXED_PREP_POSES: usize = 16;
const INDEXED_PREP_GEOMETRY_BYTES: usize = 64 * 1024 * 1024;
const SPAN_CACHE_SLOTS: usize = 128;
const SPAN_CACHE_ENTRY_BYTES: usize = 32 * 1024;

fn atlas_eviction_candidate(mut protected: impl Iterator<Item = bool>) -> Option<usize> {
    protected.position(|protected| !protected)
}

#[derive(Clone, Copy, Default)]
struct Stencil {
    serial: u32,
    clear: u8,
}

struct Draw<'a> {
    mesh: EmoteSpriteMesh<'a>,
    stencil: Stencil,
    reference: u8,
    operation: Option<CpuStencilWrite>,
    bounds: [u32; 4],
    offset: usize,
    words: usize,
}

struct Pose<'a> {
    model: &'a CpuMotionModel,
    draws: Vec<Draw<'a>>,
    serial: u32,
    geometry_bytes: usize,
    has_warnings: bool,
    cancelled: Option<&'a AtomicBool>,
}

impl EmoteDrawTarget for Pose<'_> {
    type Stencil = Stencil;

    fn new_stencil(&mut self, _: CpuViewport, clear: u8) -> cs2_runtime::emote::Result<Stencil> {
        self.serial += 1;
        if self.serial > 0x3fffff {
            return Err(EmoteError::Capacity);
        }
        Ok(Stencil { serial: self.serial, clear })
    }

    fn draw_sprite(
        &mut self,
        _: CpuViewport,
        mesh: EmoteSpriteMesh<'_>,
        pass: EmoteSpritePass<'_, Stencil>,
    ) -> cs2_runtime::emote::Result<()> {
        if self.cancelled.is_some_and(|cancelled| cancelled.load(Ordering::Relaxed)) {
            return Err(EmoteError::Capacity);
        }
        if self.draws.len() >= MAX_DRAWS || !mesh.opacity.is_finite() {
            return Err(EmoteError::Capacity);
        }
        let mesh = mesh.retain(self.model)?;
        self.geometry_bytes += std::mem::size_of_val(mesh.vertices.as_slice());
        if self.geometry_bytes > COMMAND_BYTES {
            return Err(EmoteError::Capacity);
        }
        let (stencil, reference, operation) = match pass {
            EmoteSpritePass::Color(None) => (Stencil::default(), 0, None),
            EmoteSpritePass::Color(Some((stencil, reference))) => (*stencil, reference, None),
            EmoteSpritePass::Mask { buffer, reference, operation } => {
                (*buffer, reference, Some(operation))
            }
        };
        self.draws.push(Draw {
            mesh, stencil, reference, operation, bounds: [0; 4], offset: 0, words: 0,
        });
        Ok(())
    }
}

#[derive(Default)]
struct CommandBatch {
    commands: Vec<u32>,
    rows: Vec<Vec<[u32; 8]>>,
    phases: [Duration; 2],
    span_cache: Vec<SpanCacheSlot>,
    span_hits: usize,
    span_misses: usize,
}

#[derive(Default)]
struct SpanCacheSlot {
    previous: Option<u64>,
    cached: Option<CachedSpans>,
}

struct CachedSpans {
    hash: u64,
    size: [u32; 2],
    grid: [u32; 2],
    vertices: Box<[[f64; 4]]>,
    bounds: [u32; 4],
    words: Box<[u32]>,
}

impl CachedSpans {
    fn matches(&self, hash: u64, size: [u32; 2], grid: [u32; 2], vertices: &[[f64; 4]]) -> bool {
        self.hash == hash && self.size == size && self.grid == grid
            && self.vertices.len() == vertices.len()
            && self.vertices.iter().flatten().zip(vertices.iter().flatten())
                .all(|(a, b)| a.to_bits() == b.to_bits())
    }

    fn bytes(&self) -> usize {
        std::mem::size_of_val(&*self.vertices) + std::mem::size_of_val(&*self.words)
    }
}

fn span_cache_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        #[cfg(windows)]
        let enabled = std::env::var("CS2_EMOTE_SPAN_CACHE").is_ok_and(|value| value == "1");
        #[cfg(target_os = "android")]
        let enabled = super::android_property(c"debug.cs2.emote_span_cache", |value| value == b"1")
            .unwrap_or(false);
        report(&format!("emote_span_cache={enabled}"));
        enabled
    })
}

impl CommandBatch {
    fn prepare(
        &mut self, draws: &mut [Draw<'_>], viewport: CpuViewport, alignment: usize, profile: bool,
        word_limit: usize,
    ) -> cs2_runtime::emote::Result<()> {
        self.commands.clear();
        self.rows.resize_with(viewport.height as usize, Vec::new);
        self.phases = [Duration::ZERO; 2];
        self.span_hits = 0;
        self.span_misses = 0;
        if self.span_cache.is_empty() && span_cache_enabled() {
            self.span_cache.resize_with(SPAN_CACHE_SLOTS, SpanCacheSlot::default);
        }
        for row in &mut self.rows { row.clear(); }
        for draw in draws {
            draw.words = 0;
            self.prepare_draw(draw, viewport, alignment, profile, word_limit)?;
        }
        Ok(())
    }

    fn report_span_cache(&self, profile: bool) {
        if profile && !self.span_cache.is_empty() {
            crate::perf_detail::emote_span_cache(
                self.span_hits, self.span_misses,
                self.span_cache.iter().filter_map(|slot| slot.cached.as_ref()).map(CachedSpans::bytes).sum(),
            );
        }
    }

    fn prepare_draw(
        &mut self, draw: &mut Draw<'_>, viewport: CpuViewport, alignment: usize, profile: bool,
        word_limit: usize,
    ) -> cs2_runtime::emote::Result<()> {
        let Draw { mesh, .. } = draw;
        let mut bounds = [viewport.width, viewport.height, 0, 0];
        let mut span_count = 0;
        let budget = word_limit.saturating_sub(self.commands.len()) / 8;
        let scanlines_started = profile.then(Instant::now);
        let size = [viewport.width, viewport.height];
        let grid = mesh.mesh_dimensions();
        let mut cache_candidate = None;
        if !self.span_cache.is_empty() && std::mem::size_of_val(mesh.vertices.as_slice()) < SPAN_CACHE_ENTRY_BYTES {
            let mut hasher = DefaultHasher::new();
            size.hash(&mut hasher);
            grid.hash(&mut hasher);
            for value in mesh.vertices.iter().flatten() {
                hasher.write_u64(value.to_bits());
            }
            let hash = hasher.finish();
            let slot = &mut self.span_cache[hash as usize % SPAN_CACHE_SLOTS];
            if let Some(cached) = slot.cached.as_ref().filter(|cached|
                cached.matches(hash, size, grid, &mesh.vertices))
            {
                self.span_hits += 1;
                let pack_started = scanlines_started.map(|start| {
                    let now = Instant::now();
                    self.phases[0] += now - start;
                    now
                });
                // The row table is independent of texture identity, opacity and stencil state.
                let offset = self.commands.len().next_multiple_of(alignment);
                let words = 12 + cached.words.len();
                if offset + words > word_limit {
                    return Err(EmoteError::Capacity);
                }
                self.commands.resize(offset + 12, 0);
                self.commands.extend_from_slice(&cached.words);
                set_draw_header(draw, cached.bounds, offset, words, &mut self.commands);
                self.finish_pack_timing(pack_started);
                return Ok(());
            }
            self.span_misses += 1;
            // Admit repeated geometry only, so animated meshes do not constantly
            // allocate span tables and replace reusable entries.
            if slot.previous.replace(hash) == Some(hash) {
                cache_candidate = Some(hash);
            }
        }
        for triangle in mesh.triangle_indices() {
            let mut invalid_uv = false;
            visit_emote_texture_spans(triangle.map(|i| mesh.vertices[i]), size, |span| {
                span_count += 1;
                if span_count <= budget && !invalid_uv {
                    extend_bounds(&mut bounds, [span.x.start, span.y, span.x.end, span.y + 1]);
                    let row = span.y as usize;
                    if let Some(words) = pack_span(span) {
                        self.rows[row].push(words);
                    } else {
                        invalid_uv = true;
                    }
                }
            })?;
            if span_count > budget {
                return Err(EmoteError::Capacity);
            }
            if invalid_uv {
                return Err(EmoteError::InvalidResource("E-mote UV exceeds accelerated precision"));
            }
        }
        let pack_started = self.finish_scanline_timing(scanlines_started);
        if span_count == 0 {
            return Ok(());
        }

        // One row table per dispatch. Rows retain the triangle submission order
        // even when a deformed mesh folds and two triangles cover the same pixel.
        let offset = self.commands.len().next_multiple_of(alignment);
        let height = (bounds[3] - bounds[1]) as usize;
        let first_span = 12 + height + 1;
        let words = first_span + span_count * 8;
        if offset + words > word_limit {
            return Err(EmoteError::Capacity);
        }
        self.commands.resize(offset + words, 0);
        let block = &mut self.commands[offset..];
        let mut index = first_span;
        for (y, spans) in self.rows[bounds[1] as usize..bounds[3] as usize].iter_mut().enumerate() {
            block[12 + y] = index as u32;
            let words = spans.as_flattened();
            let end = index + words.len();
            block[index..end].copy_from_slice(words);
            index = end;
            spans.clear();
        }
        block[12 + height] = index as u32;
        if let Some(hash) = cache_candidate
            && std::mem::size_of_val(mesh.vertices.as_slice()) + std::mem::size_of_val(&block[12..]) <= SPAN_CACHE_ENTRY_BYTES
        {
            self.span_cache[hash as usize % SPAN_CACHE_SLOTS].cached = Some(CachedSpans {
                hash, size, grid, vertices: mesh.vertices.clone().into_boxed_slice(), bounds,
                words: block[12..].into(),
            });
        }
        set_draw_header(draw, bounds, offset, words, &mut self.commands);
        self.finish_pack_timing(pack_started);
        Ok(())
    }

    fn finish_scanline_timing(&mut self, started: Option<Instant>) -> Option<Instant> {
        started.map(|start| {
            let now = Instant::now();
            self.phases[0] += now - start;
            now
        })
    }

    fn finish_pack_timing(&mut self, started: Option<Instant>) {
        if let Some(start) = started {
            self.phases[1] += start.elapsed();
        }
    }
}

#[inline]
fn pack_span(span: EmoteTextureSpan) -> Option<[u32; 8]> {
    // Keep the integer and fractional parts separate for large atlases.
    let base = span.uv.map(f64::floor);
    let step = span.step.map(|n| n as f32);
    if base.into_iter().any(|n| !n.is_finite() || n.abs() > 1e6)
        || step.into_iter().any(|n| !n.is_finite() || n.abs() > 1e6)
    {
        return None;
    }
    let [u, v] = std::array::from_fn(|axis| (span.uv[axis] - base[axis]) as f32);
    Some([
        span.x.start, span.x.end, u.to_bits(), v.to_bits(), step[0].to_bits(), step[1].to_bits(),
        base[0] as i32 as u32, base[1] as i32 as u32,
    ])
}

fn set_draw_header(draw: &mut Draw<'_>, bounds: [u32; 4], offset: usize, words: usize, commands: &mut [u32]) {
    commands[offset..offset + 12].copy_from_slice(&draw_header_words(
        bounds, draw.stencil, draw.reference, draw.operation, draw.mesh.texture, draw.mesh.opacity,
    ));
    draw.bounds = bounds;
    draw.offset = offset;
    draw.words = words;
}

fn draw_header_words(bounds: [u32; 4], stencil: Stencil, reference: u8,
    operation: Option<CpuStencilWrite>, texture: &CpuTexture, opacity: f64,
) -> [u32; 12] {
    [
        bounds[2] - bounds[0], bounds[3] - bounds[1], bounds[0], bounds[1],
        0, 0, 0, stencil.serial, u32::from(stencil.clear), u32::from(reference),
        (match operation {
            None => 0,
            Some(CpuStencilWrite::IncrementWrapping) => 1,
            Some(CpuStencilWrite::DecrementWrapping) => 255,
        }) | (u32::from(texture.is_bgra()) << 8),
        (opacity as f32).to_bits(),
    ]
}

fn clamp_prep_workers(requested: usize, available: usize) -> usize {
    requested.clamp(1, available.max(1).min(MAX_PREP_WORKERS))
}

fn prep_worker_limit() -> usize {
    static LIMIT: OnceLock<usize> = OnceLock::new();
    *LIMIT.get_or_init(|| {
        let available = std::thread::available_parallelism().map_or(1, |count| count.get());
        #[cfg(windows)]
        let (requested, source) = std::env::var("CS2_EMOTE_PREP_WORKERS").ok()
            .and_then(|value| value.parse::<usize>().ok())
            .map_or((MAX_PREP_WORKERS, "default"), |workers| (workers, "CS2_EMOTE_PREP_WORKERS"));
        #[cfg(target_os = "android")]
        let (requested, source) = super::android_property(c"debug.cs2.emote_prep_workers", |value| {
            std::str::from_utf8(value).ok()?.parse::<usize>().ok()
        }).flatten().map_or((MAX_PREP_WORKERS, "default"), |workers| (workers, "debug.cs2.emote_prep_workers"));
        let limit = clamp_prep_workers(requested, available);
        report(&format!(
            "emote_prep_workers={limit} requested={requested} available={available} source={source} max={MAX_PREP_WORKERS}"
        ));
        limit
    })
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PrepMode {
    Serial,
    AcrossPoses,
    WithinPose,
    Combined,
}

fn prep_mode() -> PrepMode {
    static MODE: OnceLock<PrepMode> = OnceLock::new();
    *MODE.get_or_init(|| {
        let parse = |value: &[u8]| match value {
            b"serial" => PrepMode::Serial,
            b"across" => PrepMode::AcrossPoses,
            b"within" => PrepMode::WithinPose,
            _ => PrepMode::Combined,
        };
        #[cfg(windows)]
        let mode = std::env::var("CS2_EMOTE_PREP_MODE").ok().map(|value| parse(value.as_bytes()));
        #[cfg(target_os = "android")]
        let mode = super::android_property(c"debug.cs2.emote_prep_mode", parse);
        let mode = mode.unwrap_or(PrepMode::Combined);
        report(&format!("emote_prep_mode={mode:?}"));
        mode
    })
}

#[cfg(windows)]
fn default_windows_multi_pose_workers() -> usize {
    static WORKERS: OnceLock<usize> = OnceLock::new();
    *WORKERS.get_or_init(|| {
        if std::env::var_os("CS2_EMOTE_PREP_WORKERS").is_some()
            || std::env::var_os("CS2_EMOTE_PREP_MODE").is_some()
        {
            return 1;
        }
        let available = std::thread::available_parallelism().map_or(1, |count| count.get());
        if available < MAX_PREP_WORKERS {
            return 1;
        }
        report("emote_prep_multi_pose=AcrossPoses");
        MAX_PREP_WORKERS
    })
}

fn prepare_commands(
    draws: &mut [Draw<'_>], batches: &mut [CommandBatch; MAX_PREP_WORKERS],
    viewport: CpuViewport, alignment: usize, profile: bool, commands: &mut Vec<u32>,
) -> cs2_runtime::emote::Result<[Duration; 2]> {
    commands.clear();
    if draws.is_empty() {
        return Ok([Duration::ZERO; 2]);
    }
    let desired = if matches!(prep_mode(), PrepMode::WithinPose | PrepMode::Combined)
        && draws.len() >= 12 && u64::from(viewport.width) * u64::from(viewport.height) >= 256 * 1024
    {
        prep_worker_limit()
    } else {
        1
    };
    if desired == 1 {
        let batch = &mut batches[0];
        batch.prepare(draws, viewport, alignment, profile, COMMAND_WORDS)?;
        batch.report_span_cache(profile);
        std::mem::swap(commands, &mut batch.commands);
        return Ok(batch.phases);
    }
    let chunk_size = draws.len().div_ceil(desired);
    let count = draws.len().div_ceil(chunk_size);
    let word_limit = (COMMAND_WORDS / count / alignment) * alignment;
    let active = &mut batches[..count];
    let errors = Mutex::new(std::array::from_fn::<_, MAX_PREP_WORKERS, _>(|_| None));
    let started = profile.then(Instant::now);
    let main_thread = profile.then(|| std::thread::current().id());
    let mut main_completion = [None; MAX_PREP_WORKERS];
    cs2_renderer::parallel_for_each(
        active.iter_mut().zip(draws.chunks_mut(chunk_size)).zip(&mut main_completion).enumerate(),
        count,
        |(index, ((batch, draws), completed))| {
            if let Err(error) = batch.prepare(draws, viewport, alignment, profile, word_limit) {
                let mut errors = errors.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                errors[index] = Some(error);
            }
            *completed = main_thread.filter(|id| *id == std::thread::current().id()).map(|_| Instant::now());
        },
    );
    let joined = started.map(|_| Instant::now());
    let errors = errors.into_inner().unwrap_or_else(|poisoned| poisoned.into_inner());
    let result = errors.into_iter().flatten().next().map_or(Ok(()), Err);
    if matches!(result, Err(EmoteError::Capacity)) {
        let batch = &mut batches[0];
        batch.prepare(draws, viewport, alignment, profile, COMMAND_WORDS)?;
        batch.report_span_cache(profile);
        std::mem::swap(commands, &mut batch.commands);
        return Ok(batch.phases);
    }
    result?;
    let mut phases = [Duration::ZERO; 2];
    // The order of the GPU dispatches also determines stencil semantics.
    for (batch, chunk) in active.iter().zip(draws.chunks_mut(chunk_size)) {
        batch.report_span_cache(profile);
        let start = commands.len().next_multiple_of(alignment);
        if start + batch.commands.len() > COMMAND_WORDS {
            return Err(EmoteError::Capacity);
        }
        commands.resize(start, 0);
        commands.extend_from_slice(&batch.commands);
        for draw in chunk.iter_mut().filter(|draw| draw.words != 0) {
            draw.offset += start;
        }
        for (total, elapsed) in phases.iter_mut().zip(batch.phases) {
            *total += elapsed;
        }
    }
    if let (Some(started), Some(joined)) = (started, joined) {
        let main_finished = main_completion.into_iter().flatten().max().unwrap_or(started);
        let copied_bytes = active.iter().map(|batch| std::mem::size_of_val(batch.commands.as_slice())).sum();
        crate::perf_detail::emote_prep_parallel(joined - started, joined - main_finished, copied_bytes);
    }
    Ok(phases)
}

fn extend_bounds(bounds: &mut [u32; 4], other: [u32; 4]) {
    bounds[0] = bounds[0].min(other[0]);
    bounds[1] = bounds[1].min(other[1]);
    bounds[2] = bounds[2].max(other[2]);
    bounds[3] = bounds[3].max(other[3]);
}

struct CachedAtlas {
    owner: Weak<CpuMotionModel>,
    identity: usize,
    bytes: usize,
    view: wgpu::TextureView,
    texture: wgpu::Texture,
    uploaded_rows: u32,
    used_by_draw: bool,
}

#[derive(Default)]
struct AtlasUploadWindow {
    deadline: Option<Instant>,
    speculative_remaining: usize,
    scene_remaining: usize,
}

#[derive(Default)]
struct AtlasTransfers {
    pending: VecDeque<(wgpu::SubmissionIndex, usize)>,
    bytes: usize,
}

impl AtlasTransfers {
    fn allowance(&mut self, device: &wgpu::Device, requested: usize, timeout: Duration) -> usize {
        while requested > ATLAS_IN_FLIGHT_BYTES - self.bytes {
            let Some((submission, bytes)) = self.pending.front() else { break };
            if !gpu_submission_finished(device, submission, timeout) { break; }
            self.bytes -= *bytes;
            self.pending.pop_front();
        }
        requested.min(ATLAS_IN_FLIGHT_BYTES - self.bytes)
    }

    fn submitted(&mut self, submission: wgpu::SubmissionIndex, bytes: usize) {
        debug_assert!(bytes <= ATLAS_IN_FLIGHT_BYTES - self.bytes);
        self.pending.push_back((submission, bytes));
        self.bytes += bytes;
    }
}

fn gpu_submission_finished(device: &wgpu::Device, submission: &wgpu::SubmissionIndex, timeout: Duration) -> bool {
    device.poll(wgpu::PollType::Wait {
        submission_index: Some(submission.clone()), timeout: Some(timeout),
    }).is_ok()
}

impl AtlasUploadWindow {
    fn refresh(&mut self, now: Instant) {
        if self.deadline.is_none_or(|deadline| now >= deadline) {
            self.deadline = Some(now + ATLAS_UPLOAD_WINDOW);
            self.speculative_remaining = ATLAS_STAGE_BYTES;
            self.scene_remaining = SCENE_ATLAS_STAGE_BYTES;
        }
    }
}

impl CachedAtlas {
    fn resident(&self) -> bool {
        self.uploaded_rows == self.texture.height()
    }

    fn upload(&mut self, queue: &wgpu::Queue, pixels: &[u8], byte_limit: usize) -> usize {
        let size = self.texture.size();
        let stride = size.width as usize * 4;
        let start = self.uploaded_rows;
        let rows = (size.height - start).min((byte_limit / stride) as u32);
        if rows == 0 {
            return 0;
        }
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                origin: wgpu::Origin3d { x: 0, y: start, z: 0 },
                ..self.texture.as_image_copy()
            },
            &pixels[start as usize * stride..(start + rows) as usize * stride],
            wgpu::TexelCopyBufferLayout {
                offset: 0, bytes_per_row: Some(size.width * 4), rows_per_image: Some(rows),
            },
            wgpu::Extent3d { height: rows, ..size },
        );
        self.uploaded_rows += rows;
        rows as usize * stride
    }
}

#[derive(Clone)]
struct OwnedDraw {
    mesh: OwnedEmoteSpriteMesh,
    stencil: Stencil,
    reference: u8,
    operation: Option<CpuStencilWrite>,
    bounds: [u32; 4],
    offset: usize,
    words: usize,
}

#[derive(Clone)]
struct PreparedPose {
    player: usize,
    state: Arc<CpuPlayerRenderState>,
    draws: Arc<[OwnedDraw]>,
    words: Vec<u32>,
    phases: [Duration; 2],
    indexed: Option<Arc<mesh::IndexedGeometry>>,
    has_warnings: bool,
}

struct IndexedPoseCache {
    player: usize,
    model: Weak<CpuMotionModel>,
    geometry: mesh::GeometryCache,
    prepared: Option<PreparedPose>,
    reservation_bytes: usize,
}

fn reserve_indexed_geometry(reserved: &std::sync::atomic::AtomicUsize, bytes: usize) -> bool {
    let mut used = reserved.load(Ordering::Relaxed);
    loop {
        let Some(next) = used.checked_add(bytes).filter(|total| *total <= INDEXED_PREP_GEOMETRY_BYTES) else { return false };
        match reserved.compare_exchange_weak(used, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return true,
            Err(current) => used = current,
        }
    }
}

fn prepare_indexed_poses(players: &[(&CpuPlayer, CpuViewport)]) {
    // Preserve one cache per actual player/model pair, in this frame's scene
    // order. Jobs mutate disjoint caches and only CPU data; GPU encoding remains
    // on the render thread in the original pose and primitive order.
    // Retire unconsumed previous snapshots before taking this frame's budget.
    PREPARED.with_borrow_mut(Vec::clear);
    if players.is_empty() || players.len() > MAX_INDEXED_PREP_POSES {
        INDEXED_CACHES.with_borrow_mut(Vec::clear);
        if !players.is_empty() { report(&format!("emote_indexed_prep fallback=serial reason=pose_budget poses={} max={MAX_INDEXED_PREP_POSES}", players.len())); }
        return;
    }
    let workers = if matches!(prep_mode(), PrepMode::Serial | PrepMode::WithinPose) { 1 }
        else { prep_worker_limit().min(players.len()) };
    let started = Instant::now();
    let caller = std::thread::current().id();
    let worker_jobs = std::sync::atomic::AtomicUsize::new(0);
    let reserved = std::sync::atomic::AtomicUsize::new(0);
    let budget_fallbacks = std::sync::atomic::AtomicUsize::new(0);
    let pose_hits = std::sync::atomic::AtomicUsize::new(0);
    let pose_evaluations = std::sync::atomic::AtomicUsize::new(0);
    let evaluate_work_us = std::sync::atomic::AtomicU64::new(0);
    let geometry_work_us = std::sync::atomic::AtomicU64::new(0);
    let mut poses: Vec<_> = std::iter::repeat_with(|| None).take(players.len()).collect();
    let counters = INDEXED_CACHES.with_borrow_mut(|caches| {
        let mut previous = std::mem::take(caches);
        for &(player, _) in players {
            let identity = player as *const CpuPlayer as usize;
            let model = Arc::downgrade(player.shared_motion_model());
            let cache = previous.iter().position(|cache| cache.player == identity && Weak::ptr_eq(&cache.model, &model))
                .map(|i| previous.swap_remove(i)).unwrap_or_else(|| IndexedPoseCache {
                    player: identity, model, geometry: mesh::GeometryCache::default(), prepared: None, reservation_bytes: 0,
                });
            caches.push(cache);
        }
        drop(previous);
        cs2_renderer::parallel_for_each(poses.iter_mut().zip(players).zip(caches.iter_mut()), workers,
            |((slot, &(player, viewport)), cache)| {
                if std::thread::current().id() != caller { worker_jobs.fetch_add(1, Ordering::Relaxed); }
                let Ok(variables) = player.effective_variables() else {
                    cache.prepared = None;
                    cache.geometry = mesh::GeometryCache::default();
                    return;
                };
                if let Some(prepared) = &cache.prepared
                    && prepared.state.matches(player, viewport, &variables)
                {
                    if reserve_indexed_geometry(&reserved, cache.reservation_bytes) {
                        pose_hits.fetch_add(1, Ordering::Relaxed);
                        *slot = Some(prepared.clone());
                    } else {
                        cache.prepared = None;
                        cache.geometry = mesh::GeometryCache::default();
                        budget_fallbacks.fetch_add(1, Ordering::Relaxed);
                    }
                    return;
                }
                // Effective controls and reference clocks have already advanced
                // in scene order. Only pure primitive evaluation is cached.
                cache.prepared = None;
                pose_evaluations.fetch_add(1, Ordering::Relaxed);
                let evaluating = Instant::now();
                let pose = collect_pose(player, viewport, &variables, true);
                evaluate_work_us.fetch_add(evaluating.elapsed().as_micros() as u64, Ordering::Relaxed);
                let Ok(pose) = pose else {
                    cache.geometry = mesh::GeometryCache::default();
                    return;
                };
                if mesh::compatible(&pose.draws).is_err() {
                    cache.geometry = mesh::GeometryCache::default();
                    return;
                }
                let reservation_bytes = cache.geometry.reservation_bytes(&pose.draws);
                if !reserve_indexed_geometry(&reserved, reservation_bytes) {
                    cache.geometry = mesh::GeometryCache::default();
                    budget_fallbacks.fetch_add(1, Ordering::Relaxed);
                    return;
                }
                let packing = Instant::now();
                let Ok(indexed) = cache.geometry.prepare(&pose.draws, viewport) else {
                    cache.geometry = mesh::GeometryCache::default();
                    return;
                };
                geometry_work_us.fetch_add(packing.elapsed().as_micros() as u64, Ordering::Relaxed);
                let prepared = PreparedPose {
                    player: cache.player, state: Arc::new(player.render_state_snapshot(viewport, &variables)),
                    has_warnings: pose.has_warnings, draws: own_draws(pose.draws).into(),
                    words: Vec::new(), phases: [Duration::ZERO; 2], indexed: Some(indexed),
                };
                cache.reservation_bytes = reservation_bytes;
                cache.prepared = Some(prepared.clone());
                *slot = Some(prepared);
            });
        caches.iter().map(|cache| cache.geometry.counters()).fold([0; 2], |total, next|
            [total[0] + next[0], total[1] + next[1]])
    });
    let completed = poses.iter().flatten().count();
    let warning_poses = poses.iter().flatten().filter(|pose| pose.has_warnings).count();
    PREPARED.with_borrow_mut(|prepared| {
        prepared.clear();
        prepared.extend(poses.into_iter().flatten());
    });
    static PREPARATIONS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    static POSE_COUNTS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let sample = PREPARATIONS.fetch_add(1, Ordering::Relaxed);
    let count_bit = 1 << players.len();
    let first_count = POSE_COUNTS.fetch_or(count_bit, Ordering::Relaxed) & count_bit == 0;
    if sample < 3 || first_count || sample.is_multiple_of(120) || budget_fallbacks.load(Ordering::Relaxed) != 0 {
        report(&format!("emote_indexed_prep poses={} prepared={completed} warnings={warning_poses} workers={workers} worker_jobs={} cpu_wall_us={} geometry_hits={} geometry_misses={} geometry_reserved_bytes={} geometry_budget_bytes={INDEXED_PREP_GEOMETRY_BYTES} budget_fallbacks={} pose_cache_hits={} pose_evaluations={} evaluate_work_us={} geometry_work_us={}",
            players.len(), worker_jobs.load(Ordering::Relaxed), started.elapsed().as_micros(), counters[0], counters[1],
            reserved.load(Ordering::Relaxed), budget_fallbacks.load(Ordering::Relaxed),
            pose_hits.load(Ordering::Relaxed), pose_evaluations.load(Ordering::Relaxed),
            evaluate_work_us.load(Ordering::Relaxed), geometry_work_us.load(Ordering::Relaxed)));
    }
}

pub(in crate) fn prepare_emote_poses(players: &[(&CpuPlayer, CpuViewport)]) {
    protect_emote_models(players.iter().map(|(player, _)| player.shared_motion_model()));
    if mesh::enabled() {
        prepare_indexed_poses(players);
        return;
    }
    let mode = prep_mode();
    let workers = prep_worker_limit().min(players.len());
    #[cfg(windows)]
    let (mode, workers) = if (3..=MAX_PREP_WORKERS).contains(&players.len()) {
        let across_workers = default_windows_multi_pose_workers();
        if across_workers > 1 {
            (PrepMode::AcrossPoses, across_workers.min(players.len()))
        } else {
            (mode, workers)
        }
    } else {
        (mode, workers)
    };
    // Preserve the synchronous compatibility path when indexed coverage
    // is explicitly disabled on this device.
    if workers < 2 || players.len() > 4 || matches!(mode, PrepMode::Serial | PrepMode::WithinPose) {
        return;
    }
    prepare_emote_poses_with_spans(players, mode, workers);
}

pub(in crate) fn protect_emote_models<'a>(models: impl Iterator<Item = &'a Arc<CpuMotionModel>>) {
    RASTER.with_borrow_mut(|slot| {
        if let Some(emote) = slot.as_mut().and_then(|raster| raster.emote.as_mut()) {
            emote.atlas_batch_models.extend(models.map(Arc::downgrade));
        }
    });
}

fn prepare_emote_poses_with_spans(players: &[(&CpuPlayer, CpuViewport)], mode: PrepMode, workers: usize) {
    let Some(alignment) = RASTER.with_borrow(|slot| slot.as_ref().map(|raster|
        raster.device.limits().min_storage_buffer_offset_alignment as usize / 4)) else { return };
    let profile = emote_gpu_start().is_some();
    if mode == PrepMode::AcrossPoses {
        prepare_across_poses(players, alignment, workers, profile);
        return;
    }
    let mut poses: Vec<_> = std::iter::repeat_with(|| None).take(players.len()).collect();
    cs2_renderer::parallel_for_each(poses.iter_mut().zip(players), workers, |(slot, &(player, viewport))| {
        let Ok(variables) = player.effective_variables() else { return };
        let Ok(pose) = collect_pose(player, viewport, &variables, false) else { return };
        *slot = Some((variables, pose.draws));
    });
    let groups_per_pose = prep_worker_limit().div_ceil(players.len());
    let word_limit = (COMMAND_WORDS / groups_per_pose / alignment) * alignment;
    let mut packed: Vec<_> = (0..players.len()).map(|_| (Vec::new(), [Duration::ZERO; 2])).collect();
    let completed = POSE_BATCHES.with_borrow_mut(|batches| {
        let mut jobs = Vec::new();
        for (index, (pose, &(_, viewport))) in poses.iter_mut().zip(players).enumerate() {
            if let Some((_, draws)) = pose
                && !draws.is_empty()
            {
                let chunk = draws.len().div_ceil(groups_per_pose);
                jobs.extend(draws.chunks_mut(chunk).map(|draws| (index, viewport, draws, false)));
            }
        }
        batches.resize_with(jobs.len(), CommandBatch::default);
        cs2_renderer::parallel_for_each(jobs.iter_mut().zip(batches.iter_mut()), prep_worker_limit(),
            |((_, viewport, draws, ready), batch)| {
                *ready = batch.prepare(draws, *viewport, alignment, profile, word_limit).is_ok();
            });
        if jobs.iter().any(|(_, _, _, ready)| !ready) {
            return false;
        }
        for ((index, _, draws, _), batch) in jobs.into_iter().zip(batches) {
            batch.report_span_cache(profile);
            let (words, phases) = &mut packed[index];
            let start = words.len().next_multiple_of(alignment);
            words.resize(start, 0);
            words.extend_from_slice(&batch.commands);
            for draw in draws.iter_mut().filter(|draw| draw.words != 0) {
                draw.offset += start;
            }
            for (total, elapsed) in phases.iter_mut().zip(batch.phases) {
                *total += elapsed;
            }
        }
        true
    });
    if completed {
        PREPARED.with_borrow_mut(|prepared| {
            for ((pose, &(player, viewport)), (words, phases)) in poses.into_iter().zip(players).zip(packed) {
                if let Some((variables, draws)) = pose {
                    prepared.push(PreparedPose {
                        player: player as *const CpuPlayer as usize, state: Arc::new(player.render_state_snapshot(viewport, &variables)),
                        draws: own_draws(draws).into(), words, phases, indexed: None, has_warnings: false,
                    });
                }
            }
        });
    }
}

fn prepare_across_poses(players: &[(&CpuPlayer, CpuViewport)], alignment: usize, workers: usize, profile: bool) {
    let mut poses: Vec<_> = std::iter::repeat_with(|| None).take(players.len()).collect();
    POSE_BATCHES.with_borrow_mut(|batches| {
        batches.resize_with(players.len(), CommandBatch::default);
        let started = profile.then(Instant::now);
        let main_thread = profile.then(|| std::thread::current().id());
        let jobs = poses.iter_mut().zip(players).zip(batches.iter_mut());
        cs2_renderer::parallel_for_each(jobs, workers, |((slot, &(player, viewport)), batch)| {
            let Ok(variables) = player.effective_variables() else { return };
            let Ok(mut pose) = collect_pose(player, viewport, &variables, false) else { return };
            if batch.prepare(&mut pose.draws, viewport, alignment, profile, COMMAND_WORDS).is_err() { return; }
            *slot = Some((PreparedPose {
                player: player as *const CpuPlayer as usize, state: Arc::new(player.render_state_snapshot(viewport, &variables)),
                draws: own_draws(pose.draws).into(), words: std::mem::take(&mut batch.commands), phases: batch.phases, indexed: None,
                has_warnings: false,
            }, main_thread.filter(|id| *id == std::thread::current().id()).map(|_| Instant::now())));
        });
        if let Some(started) = started {
            let finished = Instant::now();
            let main_finished = poses.iter().flatten().filter_map(|(_, time)| *time).max().unwrap_or(started);
            crate::perf_detail::emote_prep_parallel(finished - started, finished - main_finished, 0);
        }
        PREPARED.with_borrow_mut(|prepared| {
            for ((pose, _), batch) in poses.into_iter().zip(batches).filter_map(|(pose, batch)|
                pose.map(|pose| (pose, batch)))
            {
                batch.report_span_cache(profile);
                prepared.push(pose);
            }
        });
    });
}

fn collect_pose<'a>(
    player: &'a CpuPlayer, viewport: CpuViewport, variables: &BTreeMap<String, f64>,
    allow_warnings: bool,
) -> Result<Pose<'a>> {
    collect_pose_until_cancelled(player, viewport, variables, allow_warnings, None)
}

fn collect_pose_until_cancelled<'a>(
    player: &'a CpuPlayer, viewport: CpuViewport, variables: &BTreeMap<String, f64>,
    allow_warnings: bool, cancelled: Option<&'a AtomicBool>,
) -> Result<Pose<'a>> {
    let mut pose = Pose { model: player.shared_motion_model(), draws: Vec::new(), serial: 0, geometry_bytes: 0, has_warnings: false, cancelled };
    let diagnostics = player.draw_primitives_resolved(viewport, variables, &mut pose)?;
    pose.has_warnings = !diagnostics.warnings.is_empty();
    if !allow_warnings && !diagnostics.warnings.is_empty() {
        bail!("E-mote pose has CPU evaluator warnings");
    }
    Ok(pose)
}

fn own_draws(draws: Vec<Draw<'_>>) -> Vec<OwnedDraw> {
    draws.into_iter().map(|draw| OwnedDraw {
        mesh: draw.mesh.into_owned(), stencil: draw.stencil,
        reference: draw.reference, operation: draw.operation,
        bounds: draw.bounds, offset: draw.offset, words: draw.words,
    }).collect()
}

fn take_prepared_pose(
    player: &CpuPlayer, viewport: CpuViewport, variables: &BTreeMap<String, f64>,
) -> Option<PreparedPose> {
    PREPARED.with_borrow_mut(|poses| {
        let index = poses.iter().position(|pose| {
            pose.player == player as *const CpuPlayer as usize
                && pose.state.matches(player, viewport, variables)
        })?;
        Some(poses.swap_remove(index))
    })
}

struct PendingEmote {
    readback: wgpu::Buffer,
    receiver: mpsc::Receiver<std::result::Result<(), wgpu::BufferAsyncError>>,
    submission: wgpu::SubmissionIndex,
    timing: Option<ReadbackTimer>,
    model: Arc<CpuMotionModel>,
    rgba: Arc<wgpu::Buffer>,
    draws: Vec<OwnedDraw>,
    viewport: CpuViewport,
    bounds: [u32; 4],
    milestones: [Option<Instant>; 4],
    phases: [Duration; 2],
    batch_count: usize,
}

#[derive(Debug)]
pub(super) struct DevicePose {
    buffer: Arc<wgpu::Buffer>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    bytes: u64,
    failed: Arc<AtomicBool>,
    fade_alpha: Option<Arc<wgpu::Buffer>>,
    mesh_submission: Option<Arc<mesh::MeshSubmission>>,
}

impl DevicePose {
    fn flush_mesh(&self) {
        if let Some(submission) = &self.mesh_submission {
            submission.flush(&self.queue);
        }
    }
}

impl EmoteDeviceReadback for DevicePose {
    fn is_valid(&self) -> bool { !self.failed.load(Ordering::Relaxed) }

    fn read_rgba(&self, output: &mut [u8]) -> bool {
        if self.failed.load(Ordering::Relaxed) || output.len() as u64 != self.bytes { return false; }
        self.flush_mesh();
        let measurement = ReadbackTimer::start(ReadbackKind::Emote, self.bytes);
        let readback = buffer(&self.device, self.bytes, DOWNLOAD);
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_buffer_to_buffer(&self.buffer, 0, &readback, 0, self.bytes);
        let submitted = super::submit_gpu_commands(&self.queue, Some(encoder.finish()));
        let slice = readback.slice(..);
        let (send, receive) = mpsc::sync_channel(1);
        slice.map_async(wgpu::MapMode::Read, move |result| { let _ = send.send(result); });
        let ready = self.device.poll(wgpu::PollType::Wait {
            submission_index: Some(submitted), timeout: Some(Duration::from_millis(250)),
        });
        if ready.is_err() || !matches!(receive.try_recv(), Ok(Ok(()))) || self.failed.load(Ordering::Relaxed) {
            self.failed.store(true, Ordering::Relaxed);
            report("emote_device_readback_failed");
            return false;
        }
        output.copy_from_slice(&slice.get_mapped_range());
        readback.unmap();
        if let Some(measurement) = measurement { measurement.complete(); }
        true
    }

    fn as_any(&self) -> &dyn std::any::Any { self }
}

const OUTPUT_POOL_BYTES: u64 = 32 * 1024 * 1024;
const OUTPUT_POOL_SLOTS: usize = 8;

struct OutputPool<T> {
    entries: VecDeque<(u64, Arc<T>)>,
    bytes: u64,
}

impl<T> OutputPool<T> {
    fn new() -> Self { Self { entries: VecDeque::new(), bytes: 0 } }

    fn acquire(&mut self, bytes: u64, limit: u64, create: impl FnOnce(u64) -> T) -> (Arc<T>, bool) {
        let best = self.entries.iter().enumerate()
            .filter(|(_, (capacity, output))| *capacity >= bytes
                && *capacity <= bytes.saturating_mul(2).max(64 * 1024)
                && Arc::strong_count(output) == 1)
            .min_by_key(|(_, (capacity, _))| *capacity)
            .map(|(index, _)| index);
        if let Some(index) = best {
            let entry = self.entries.remove(index).unwrap();
            let output = entry.1.clone();
            self.entries.push_back(entry);
            return (output, true);
        }
        let capacity = bytes.next_multiple_of(64 * 1024).min(limit);
        let output = Arc::new(create(capacity));
        if capacity <= OUTPUT_POOL_BYTES {
            while self.bytes + capacity > OUTPUT_POOL_BYTES || self.entries.len() >= OUTPUT_POOL_SLOTS {
                let Some(index) = self.entries.iter().position(|(_, entry)| Arc::strong_count(entry) == 1) else {
                    return (output, false);
                };
                self.bytes -= self.entries.remove(index).unwrap().0;
            }
            self.entries.push_back((capacity, output.clone()));
            self.bytes += capacity;
        }
        (output, false)
    }
}

pub(super) struct EmoteRaster {
    mesh: Option<mesh::MeshRaster>,
    sprite: wgpu::ComputePipeline,
    sprite_layout: wgpu::BindGroupLayout,
    pack: wgpu::ComputePipeline,
    pack_layout: wgpu::BindGroupLayout,
    preview_pipelines: Option<(wgpu::ComputePipeline, wgpu::ComputePipeline)>,
    repair: wgpu::ComputePipeline,
    repair_layout: wgpu::BindGroupLayout,
    fade: wgpu::ComputePipeline,
    fade_layout: wgpu::BindGroupLayout,
    fade_expand: wgpu::ComputePipeline,
    fade_expand_layout: wgpu::BindGroupLayout,
    fade_prepare: wgpu::ComputePipeline,
    fade_prepare_layout: wgpu::BindGroupLayout,
    fade_alpha: wgpu::ComputePipeline,
    fade_alpha_layout: wgpu::BindGroupLayout,
    tint: wgpu::ComputePipeline,
    tint_layout: wgpu::BindGroupLayout,
    fade_params: wgpu::Buffer,
    fade_empty_alpha: wgpu::Buffer,
    fade_sources: VecDeque<(EmoteRasterFrame, wgpu::Buffer)>,
    fade_outputs: OutputPool<wgpu::Buffer>,
    atlases: VecDeque<CachedAtlas>,
    atlas_bytes: usize,
    atlas_batch_models: Vec<Weak<CpuMotionModel>>,
    atlas_upload_window: AtlasUploadWindow,
    atlas_transfers: AtlasTransfers,
    commands: wgpu::Buffer,
    batch_info: wgpu::Buffer,
    pixels: wgpu::Buffer,
    rgba: Arc<wgpu::Buffer>,
    rgba_spares: Vec<Arc<wgpu::Buffer>>,
    repairs: wgpu::Buffer,
    size: wgpu::Buffer,
    readback: wgpu::Buffer,
    readback_spares: Vec<wgpu::Buffer>,
    pending: Vec<PendingEmote>,
    repair_scratch: EmoteRepairScratch,
    batches: [CommandBatch; MAX_PREP_WORKERS],
    words: Vec<u32>,
}

impl EmoteRaster {
    #[cfg(target_os = "android")]
    pub(super) fn trim_android_caches(&mut self) {
        self.atlases.retain(|atlas| atlas.used_by_draw);
        self.atlas_bytes = self.atlases.iter().map(|atlas| atlas.bytes).sum();
        self.fade_sources.clear();
        self.fade_outputs = OutputPool::new();
        self.rgba_spares.clear();
        self.readback_spares.clear();
    }

    pub(super) fn new(device: &wgpu::Device) -> Result<Self> {
        #[cfg(target_os = "android")]
        let validation = crate::gpu_raster::lock_validation_scope();
        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("CS2 E-mote spans"),
            source: wgpu::ShaderSource::Wgsl(include_str!("gpu_emote.wgsl").into()),
        });
        let pipeline = |entry, layout: Option<&wgpu::PipelineLayout>, constants: &[(&str, f64)]| device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some(entry), module: &module, entry_point: Some(entry),
            layout,
            compilation_options: wgpu::PipelineCompilationOptions { constants, ..Default::default() },
            cache: None,
        });
        let storage = |binding, read_only, has_dynamic_offset| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only },
                has_dynamic_offset,
                min_binding_size: None,
            },
            count: None,
        };
        let sprite_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("CS2 E-mote sprite"),
            entries: &[
                storage(0, true, false),
                storage(1, false, false),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Uint,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                storage(3, true, true),
            ],
        });
        let sprite_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None, bind_group_layouts: &[&sprite_layout], push_constant_ranges: &[],
        });
        let sprite = pipeline("sprite", Some(&sprite_pipeline_layout), &[]);
        let pack = pipeline("pack", None, &[]);
        let repair = pipeline("repair", None, &[]);
        let fade = pipeline("surface_fade", None, &[]);
        let fade_expand = pipeline("surface_expand", None, &[]);
        let fade_prepare = pipeline("surface_prepare", None, &[]);
        let fade_alpha = pipeline("surface_alpha", None, &[]);
        let tint = pipeline("surface_tint", None, &[]);
        #[cfg(target_os = "android")]
        let preview_enabled = true;
        #[cfg(windows)]
        let preview_enabled = std::env::var_os("CS2_REPLAY_EMOTE_PREVIEW").is_some_and(|value| value == "1");
        let preview_pipelines = preview_enabled.then(|| {
            let layout = |group| device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: None, bind_group_layouts: &[group], push_constant_ranges: &[],
            });
            let constants = &[("track_repairs", 0.0)];
            let pack_layout = layout(&pack.get_bind_group_layout(0));
            (
                pipeline("sprite", Some(&sprite_pipeline_layout), constants),
                pipeline("pack", Some(&pack_layout), constants),
            )
        });
        if let Some(error) = pollster::block_on(device.pop_error_scope()) {
            bail!("compile E-mote shader: {error}");
        }
        #[cfg(target_os = "android")]
        drop(validation);
        let source = |size, usage| buffer(device, size, usage);
        let mesh = match mesh::MeshRaster::new(device) {
            Ok(mesh) => Some(mesh),
            Err(error) => { report(&format!("emote_mesh_fallback reason={error:#}")); None }
        };
        Ok(Self {
            mesh,
            sprite_layout, pack_layout: pack.get_bind_group_layout(0),
            repair_layout: repair.get_bind_group_layout(0), sprite, pack, preview_pipelines, repair,
            fade_layout: fade.get_bind_group_layout(0), fade,
            fade_expand_layout: fade_expand.get_bind_group_layout(0), fade_expand,
            fade_prepare_layout: fade_prepare.get_bind_group_layout(0), fade_prepare,
            fade_alpha_layout: fade_alpha.get_bind_group_layout(0), fade_alpha,
            tint_layout: tint.get_bind_group_layout(0), tint,
            fade_params: source(32, wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST),
            fade_empty_alpha: buffer(device, 4, TARGET),
            fade_sources: VecDeque::new(), fade_outputs: OutputPool::new(),
            atlases: VecDeque::new(), atlas_bytes: 0, atlas_batch_models: Vec::new(),
            atlas_upload_window: AtlasUploadWindow::default(),
            atlas_transfers: AtlasTransfers::default(),
            commands: source(4, UPLOAD), batch_info: source(4, UPLOAD),
            pixels: source(8, UPLOAD), rgba: Arc::new(source(4, TARGET)),
            rgba_spares: Vec::new(), repairs: source(8, UPLOAD),
            size: source(16, wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST),
            readback: source(4, DOWNLOAD), readback_spares: Vec::new(), pending: Vec::new(),
            repair_scratch: EmoteRepairScratch::default(), batches: Default::default(), words: Vec::new(),
        })
    }

    fn fade_source(&mut self, device: &wgpu::Device, queue: &wgpu::Queue,
        failed: &Arc<AtomicBool>, frame: &EmoteRasterFrame,
    ) -> Option<wgpu::Buffer> {
        if let Some(source) = frame.device() {
            let pose = source.as_any().downcast_ref::<DevicePose>()?;
            if !Arc::ptr_eq(failed, &pose.failed) { return None; }
            pose.flush_mesh();
            return Some((*pose.buffer).clone());
        }
        let entry = if let Some(index) = self.fade_sources.iter().position(|(prior, _)| prior.shares_pixels(frame)) {
            self.fade_sources.remove(index).unwrap()
        } else {
            let bytes = u64::from(frame.width()) * u64::from(frame.height()) * 4;
            let source = buffer(device, bytes, UPLOAD);
            queue.write_buffer(&source, 0, frame.rgba());
            (frame.clone(), source)
        };
        let source = entry.1.clone();
        if self.fade_sources.len() == 4 { self.fade_sources.pop_front(); }
        self.fade_sources.push_back(entry);
        Some(source)
    }

    fn fade_output(&mut self, device: &wgpu::Device, bytes: u64) -> Arc<wgpu::Buffer> {
        self.acquire_output(device, bytes).0
    }

    fn acquire_output(&mut self, device: &wgpu::Device, bytes: u64) -> (Arc<wgpu::Buffer>, bool) {
        self.fade_outputs.acquire(bytes, u64::from(device.limits().max_storage_buffer_binding_size),
            |capacity| buffer(device, capacity, TARGET))
    }

    fn atlas(
        &mut self, device: &wgpu::Device, queue: &wgpu::Queue,
        owner: &Arc<CpuMotionModel>, source: &CpuTexture,
    ) -> Result<wgpu::TextureView> {
        if REQUIRE_RESIDENT_ATLASES.with(Cell::get) {
            let owner = Arc::downgrade(owner);
            let identity = std::ptr::from_ref(source) as usize;
            let atlas = self.atlases.iter_mut().find(|entry| {
                entry.identity == identity && Weak::ptr_eq(&entry.owner, &owner) && entry.resident()
            }).context("E-mote scene atlas is not fully resident")?;
            atlas.used_by_draw = true;
            return Ok(atlas.view.clone());
        }
        let index = self.atlas_storage(device, owner, source, false)?
            .context("E-mote atlas allocation was declined")?;
        if !self.atlases[index].resident() {
            let started = Instant::now();
            let pixels = source.upload_pixels().0;
            let stride = source.width * 4;
            let mut bytes = 0;
            while !self.atlases[index].resident() {
                let tail = pixels.len() - self.atlases[index].uploaded_rows as usize * stride;
                let allowance = self.atlas_transfers.allowance(device, tail.min(ATLAS_TRANSFER_BYTES), Duration::from_millis(16));
                let transferred = self.atlases[index].upload(queue, pixels, allowance);
                if transferred == 0 { bail!("E-mote atlas staging is busy"); }
                bytes += transferred;
                let submitted = super::submit_gpu_commands(queue, []);
                self.atlas_transfers.submitted(submitted, transferred);
            }
            report(&format!("emote_atlas_demand_bytes={bytes} write_us={}", started.elapsed().as_micros()));
        }
        Ok(self.atlases[index].view.clone())
    }

    fn stage_scene_atlas(
        &mut self, device: &wgpu::Device, queue: &wgpu::Queue,
        model: &Arc<CpuMotionModel>, texture: &CpuTexture, budget: &mut usize,
    ) -> Result<Option<bool>> {
        let stride = texture.width * 4;
        if *budget < stride {
            return Ok(Some(self.scene_atlas_tail(model, texture) == 0));
        }
        let Some(index) = self.atlas_storage(device, model, texture, false)? else { return Ok(None) };
        let atlas = &mut self.atlases[index];
        *budget -= atlas.upload(queue, texture.upload_pixels().0, *budget);
        Ok(Some(atlas.resident()))
    }

    fn scene_atlas_tail(&self, model: &Arc<CpuMotionModel>, texture: &CpuTexture) -> usize {
        let owner = Arc::downgrade(model);
        let identity = std::ptr::from_ref(texture) as usize;
        let uploaded_rows = self.atlases.iter().find(|atlas|
            atlas.identity == identity && Weak::ptr_eq(&atlas.owner, &owner))
            .map_or(0, |atlas| atlas.uploaded_rows as usize);
        (texture.height - uploaded_rows) * (texture.width * 4)
    }

    #[cfg(test)]
    fn stage_atlas(&mut self, device: &wgpu::Device, queue: &wgpu::Queue,
        model: &Arc<CpuMotionModel>,
    ) -> Result<usize> {
        self.stage_atlas_with_budget(device, queue, model, ATLAS_STAGE_BYTES)
    }

    fn stage_atlas_with_budget(&mut self, device: &wgpu::Device, queue: &wgpu::Queue,
        model: &Arc<CpuMotionModel>, budget: usize,
    ) -> Result<usize> {
        for texture in model.textures.values() {
            let Some(index) = self.atlas_storage(device, model, texture, true)? else { return Ok(0) };
            let atlas = &mut self.atlases[index];
            let bytes = atlas.upload(queue, texture.upload_pixels().0, budget);
            if bytes != 0 {
                return Ok(bytes);
            }
        }
        Ok(0)
    }

    fn begin_atlas_batch(&mut self) {
        self.atlas_batch_models.clear();
        for atlas in &mut self.atlases {
            atlas.used_by_draw = false;
        }
    }

    fn atlas_storage(
        &mut self, device: &wgpu::Device, owner: &Arc<CpuMotionModel>, source: &CpuTexture,
        preloading: bool,
    ) -> Result<Option<usize>> {
        let identity = std::ptr::from_ref(source) as usize;
        let owner_id = Arc::downgrade(owner);
        if let Some(index) = self.atlases.iter().position(|entry|
            entry.identity == identity && Weak::ptr_eq(&entry.owner, &owner_id))
        {
            if preloading {
                return Ok(Some(index));
            }
            let mut entry = self.atlases.remove(index).unwrap();
            entry.used_by_draw = true;
            self.atlases.push_back(entry);
            return Ok(Some(self.atlases.len() - 1));
        }
        let limit = device.limits().max_texture_dimension_2d as usize;
        let (pixels, _) = source.upload_pixels();
        let bytes = pixels.len();
        if source.width.max(source.height) > limit || source.width.min(source.height) == 0
            || source.width * source.height * 4 != bytes || bytes > MAX_ATLAS_BYTES
        {
            bail!("E-mote atlas exceeds device limits");
        }
        self.atlases.retain(|entry| {
            let keep = entry.owner.strong_count() != 0;
            if !keep { self.atlas_bytes -= entry.bytes; }
            keep
        });
        // Speculation may replace inactive entries, but must leave every draw
        // and current scene model resident. Both budgets have explicit bounds.
        let budget = if preloading { ATLAS_STAGE_LIMIT } else { ATLAS_DRAW_LIMIT };
        let mut evicted = 0;
        let mut evicted_bytes = 0;
        while self.atlas_bytes + bytes > budget {
            let Some(index) = atlas_eviction_candidate(self.atlases.iter().map(|atlas| {
                atlas.used_by_draw || self.atlas_batch_models.iter()
                    .any(|owner| Weak::ptr_eq(owner, &atlas.owner))
            })) else {
                static REPORTS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
                if REPORTS.fetch_add(1, Ordering::Relaxed) < 16 {
                    report(&format!("emote_atlas_declined reason=active_budget speculative={preloading} resident_bytes={} requested_bytes={bytes} limit={budget}", self.atlas_bytes));
                }
                return Ok(None);
            };
            let atlas = self.atlases.remove(index).unwrap();
            self.atlas_bytes -= atlas.bytes;
            evicted += 1;
            evicted_bytes += atlas.bytes;
        }
        let size = wgpu::Extent3d {
            width: source.width as u32, height: source.height as u32, depth_or_array_layers: 1,
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("CS2 E-mote atlas"), size, mip_level_count: 1, sample_count: 1,
            dimension: wgpu::TextureDimension::D2, format: wgpu::TextureFormat::Rgba8Uint,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        self.atlas_bytes += bytes;
        if evicted != 0 {
            report(&format!(
                "emote_atlas_evictions={evicted} evicted_bytes={evicted_bytes} resident_bytes={}",
                self.atlas_bytes,
            ));
        }
        let atlas = CachedAtlas {
            owner: owner_id, identity, bytes, view, texture, uploaded_rows: 0,
            used_by_draw: !preloading,
        };
        if preloading {
            self.atlases.push_front(atlas);
            Ok(Some(0))
        } else {
            self.atlases.push_back(atlas);
            Ok(Some(self.atlases.len() - 1))
        }
    }

    fn finish_pending(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) -> Result<()> {
        if let Some(mesh) = &self.mesh {
            mesh.submission.flush(queue);
        }
        let pending = std::mem::take(&mut self.pending);
        if pending.is_empty() { return Ok(()) }
        let last_submission = pending.last().unwrap().submission.clone();
        let ready = device.poll(wgpu::PollType::Wait {
            submission_index: Some(last_submission), timeout: Some(Duration::from_millis(250)),
        });
        if let Err(error) = ready {
            for pose in pending { pose.readback.unmap(); }
            return Err(error).context("wait for batched E-mote poses");
        }
        emote_gpu_sync(pending.len());
        for pose in pending {
            let PendingEmote {
                readback, receiver, submission: _, timing, model, rgba, draws, viewport, bounds,
                milestones, phases, batch_count,
            } = pose;
            receiver.try_recv().context("receive batched E-mote readback")?
                .context("map batched E-mote pose")?;
            let [x, y, right, bottom] = bounds;
            let width = right - x;
            let count = width * (bottom - y);
            let flag_bytes = u64::from(count.div_ceil(32)) * 4;
            let slice = readback.slice(..flag_bytes);
            let mapped = slice.get_mapped_range();
            let mut repair = vec![Vec::new(); viewport.height as usize];
            for (word, bytes) in mapped.as_chunks::<4>().0.iter().enumerate() {
                let mut bits = u32::from_le_bytes(*bytes);
                while bits != 0 {
                    let pixel = word * 32 + bits.trailing_zeros() as usize;
                    let row = y as usize + pixel / width as usize;
                    repair[row].push(x as usize + pixel % width as usize);
                    bits &= bits - 1;
                }
            }
            drop(mapped);
            readback.unmap();
            if let Some(timing) = timing { timing.complete(); }
            let downloaded = Instant::now();
            let draw_count = draws.len();
            let repaired = repair.iter().map(Vec::len).sum::<usize>();
            if repaired != 0 {
                repair_emote_raster(
                    viewport, &repair, [x, y], width, &mut self.repair_scratch, |target| {
                        let mut stencils = BTreeMap::<u32, CpuStencilBuffer>::new();
                        for draw in draws {
                            let pass = if draw.stencil.serial == 0 {
                                EmoteSpritePass::Color(None)
                            } else {
                                let buffer = match stencils.entry(draw.stencil.serial) {
                                    std::collections::btree_map::Entry::Occupied(entry) => entry.into_mut(),
                                    std::collections::btree_map::Entry::Vacant(entry) => {
                                        entry.insert(target.new_stencil(viewport, draw.stencil.clear)?)
                                    }
                                };
                                match draw.operation {
                                    Some(operation) => EmoteSpritePass::Mask {
                                        buffer, reference: draw.reference, operation,
                                    },
                                    None => EmoteSpritePass::Color(Some((buffer, draw.reference))),
                                }
                            };
                            target.draw_sprite(viewport, draw.mesh.restore(&model)?, pass)?;
                        }
                        Ok(())
                    },
                )?;
                let corrections = self.repair_scratch.corrections();
                let correction_count = corrections.len() / 2;
                let correction_bytes = corrections.len() as u64 * 4;
                if self.repairs.size() < correction_bytes {
                    self.repairs = buffer(device, correction_bytes, UPLOAD);
                }
                let bytes = unsafe {
                    std::slice::from_raw_parts(corrections.as_ptr().cast::<u8>(), correction_bytes as usize)
                };
                queue.write_buffer(&self.repairs, 0, bytes);
                queue.write_buffer(&self.size, 0, &(correction_count as u32).to_le_bytes());
                let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("CS2 batched sparse repair"), layout: &self.repair_layout,
                    entries: &[
                        wgpu::BindGroupEntry { binding: 0, resource: self.size.as_entire_binding() },
                        wgpu::BindGroupEntry { binding: 1, resource: self.repairs.as_entire_binding() },
                        wgpu::BindGroupEntry { binding: 2, resource: rgba.as_entire_binding() },
                    ],
                });
                let mut encoder = device.create_command_encoder(&Default::default());
                {
                    let mut pass = encoder.begin_compute_pass(&Default::default());
                    pass.set_pipeline(&self.repair);
                    pass.set_bind_group(0, &group, &[]);
                    pass.dispatch_workgroups(correction_count.div_ceil(64) as u32, 1, 1);
                }
                super::submit_gpu_commands(queue, Some(encoder.finish()));
            }
            if let [Some(started), Some(evaluated), Some(collected), Some(encoded)] = milestones {
                emote_gpu_pose(
                    [started, evaluated, collected, encoded, downloaded], phases,
                    draw_count, batch_count, u64::from(count), repaired,
                );
            }
            if self.readback_spares.len() < 4 { self.readback_spares.push(readback); }
        }
        Ok(())
    }

    fn render(
        &mut self, device: &wgpu::Device, queue: &wgpu::Queue, player: &CpuPlayer,
        viewport: CpuViewport, variables: &BTreeMap<String, f64>, allow_warnings: bool,
        failed: &Arc<AtomicBool>,
    ) -> Result<EmoteRasterFrame> {
        let started = emote_gpu_start();
        if !batching() {
            self.begin_atlas_batch();
        }
        let model = player.shared_motion_model();
        let model_id = Arc::downgrade(model);
        if !self.atlas_batch_models.iter().any(|owner| Weak::ptr_eq(owner, &model_id)) {
            self.atlas_batch_models.push(model_id);
        }
        // Pixel-exact reference consumers retain f64 span setup. The production
        // batched path and device endpoints use hardware coverage directly.
        if mesh::enabled() && mesh_preview() && self.mesh.is_some() {
            let (draws, geometry) = if let Some(prepared) = take_prepared_pose(player, viewport, variables) {
                if prepared.has_warnings && !allow_warnings { bail!("E-mote pose has CPU evaluator warnings"); }
                if let Some(geometry) = prepared.indexed {
                    let evaluated = started.map(|_| Instant::now());
                    return self.render_prepared_indexed(device, queue, model, &prepared.draws,
                        failed, started, evaluated, geometry);
                }
                let draws = prepared.draws.iter().cloned().map(|draw| Ok(Draw {
                    mesh: draw.mesh.restore(model)?, stencil: draw.stencil,
                    reference: draw.reference, operation: draw.operation,
                    bounds: draw.bounds, offset: draw.offset, words: draw.words,
                })).collect::<cs2_runtime::emote::Result<Vec<_>>>()?;
                (draws, None)
            } else {
                (collect_pose(player, viewport, variables, allow_warnings)?.draws, None)
            };
            match mesh::compatible(&draws) {
                Ok(()) => {
                    let evaluated = started.map(|_| Instant::now());
                    return self.render_indexed(device, queue, model, viewport, draws, failed, started, evaluated, geometry);
                }
                Err(reason) => {
                    static REPORTS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
                    if REPORTS.fetch_add(1, Ordering::Relaxed) < 16 {
                        report(&format!("emote_mesh_fallback reason={reason:#}"));
                    }
                }
            }
        }
        if let Some(mesh) = &self.mesh {
            mesh.submission.flush(queue);
        }
        let prepared = take_prepared_pose(player, viewport, variables).filter(|pose| pose.indexed.is_none());
        let (mut draws, packed) = if let Some(prepared) = prepared {
            let draws = prepared.draws.iter().cloned().map(|draw| Ok(Draw {
                mesh: draw.mesh.restore(model)?, stencil: draw.stencil,
                reference: draw.reference, operation: draw.operation,
                bounds: draw.bounds, offset: draw.offset, words: draw.words,
            })).collect::<cs2_runtime::emote::Result<Vec<_>>>()?;
            self.words = prepared.words;
            (draws, Some(prepared.phases))
        } else {
            (collect_pose(player, viewport, variables, allow_warnings)?.draws, None)
        };
        let evaluated = started.map(|_| Instant::now());
        let alignment = device.limits().min_storage_buffer_offset_alignment as usize / 4;
        let phases = match packed {
            Some(phases) => phases,
            None => prepare_commands(
                &mut draws, &mut self.batches, viewport, alignment, started.is_some(), &mut self.words,
            )?,
        };
        draws.retain(|draw| draw.words != 0);
        let collected = started.map(|_| Instant::now());
        if draws.is_empty() {
            return Ok(EmoteRasterFrame::new(1, 1, [0, 0], vec![0; 4])?);
        }
        let mut bounds = [viewport.width, viewport.height, 0, 0];
        for draw in &draws {
            extend_bounds(&mut bounds, draw.bounds);
        }
        let [x, y, right, bottom] = bounds;
        let width = right - x;
        let count = width * (bottom - y);
        let color_bytes = u64::from(count) * 4;
        #[cfg(target_os = "android")]
        let preview = batching() && !super::android_property(c"debug.cs2.emote_preview", |value| value == b"0").unwrap_or(false);
        #[cfg(windows)]
        let preview = batching();
        let (sprite, pack, flag_bytes) = match self.preview_pipelines.as_ref().filter(|_| preview) {
            Some((sprite, pack)) => (sprite.clone(), pack.clone(), 0),
            None => (self.sprite.clone(), self.pack.clone(), u64::from(count.div_ceil(32)) * 4),
        };
        let download_bytes = color_bytes + flag_bytes;
        let command_bytes = self.words.len() as u64 * 4;
        let limits = device.limits();
        let groups = count.div_ceil(64);
        if color_bytes * 2 > u64::from(limits.max_storage_buffer_binding_size)
            || command_bytes > limits.max_buffer_size || groups > limits.max_compute_workgroups_per_dimension
            || draws.iter().any(|draw| draw.words as u64 * 4 > u64::from(limits.max_storage_buffer_binding_size))
        {
            bail!("E-mote pose exceeds device limits");
        }
        let grow = |current: &mut wgpu::Buffer, bytes, usage| {
            if current.size() < bytes {
                *current = buffer(device, bytes, usage);
            }
        };
        grow(&mut self.commands, command_bytes, UPLOAD);
        grow(&mut self.pixels, color_bytes * 2, UPLOAD);
        if self.rgba.size() < download_bytes || Arc::strong_count(&self.rgba) != 1 {
            let next = self.rgba_spares.iter().position(|slot|
                slot.size() >= download_bytes && Arc::strong_count(slot) == 1)
                .map(|index| self.rgba_spares.swap_remove(index))
                .unwrap_or_else(|| Arc::new(buffer(device, download_bytes, TARGET)));
            let previous = std::mem::replace(&mut self.rgba, next);
            if self.rgba_spares.len() < 4 { self.rgba_spares.push(previous); }
        }
        if self.readback.size() < flag_bytes
            && let Some(index) = self.readback_spares.iter().position(|buffer| buffer.size() >= flag_bytes)
        {
            self.readback = self.readback_spares.swap_remove(index);
        }
        if flag_bytes != 0 {
            grow(&mut self.readback, flag_bytes, DOWNLOAD);
        }
        for draw in &draws {
            self.words[draw.offset + 4..draw.offset + 7].copy_from_slice(&[width, x, y]);
        }
        // Android arm64 and Windows targets use little-endian u32, as required
        // by the shader's storage representation. The byte view does not escape.
        let words = &self.words;
        let bytes = unsafe { std::slice::from_raw_parts(words.as_ptr().cast::<u8>(), words.len() * 4) };
        queue.write_buffer(&self.commands, 0, bytes);
        queue.write_buffer(&self.size, 0, &count.to_le_bytes());
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.clear_buffer(&self.pixels, 0, Some(color_bytes * 2));
        let mut batches = Vec::<(usize, usize)>::new();
        let mut first_draw = 0;
        while first_draw < draws.len() {
            let first = &draws[first_draw];
            let mut end = first_draw + 1;
            while let Some(next) = draws.get(end) {
                let overlap = draws[first_draw..end].iter().any(|earlier| {
                    let a = earlier.bounds;
                    let b = next.bounds;
                    a[0] < b[2] && b[0] < a[2] && a[1] < b[3] && b[1] < a[3]
                });
                if !std::ptr::eq(first.mesh.texture, next.mesh.texture) || overlap
                    || end - first_draw >= limits.max_compute_workgroups_per_dimension as usize
                {
                    break;
                }
                end += 1;
            }
            batches.push((first_draw, end));
            first_draw = end;
        }
        let info_size = batches.iter().map(|&(first, end)| (end - first) * 4).max().unwrap();
        let info_stride = info_size.next_multiple_of(limits.min_storage_buffer_offset_alignment as usize);
        let mut offsets = vec![0; batches.len() * info_stride];
        for (batch, &(first, end)) in batches.iter().enumerate() {
            let start = batch * info_stride;
            for (word, draw) in offsets[start..][..(end - first) * 4].chunks_exact_mut(4).zip(&draws[first..end]) {
                word.copy_from_slice(&(draw.offset as u32).to_le_bytes());
            }
        }
        grow(&mut self.batch_info, offsets.len() as u64, UPLOAD);
        queue.write_buffer(&self.batch_info, 0, &offsets);
        let mut atlas_groups = HashMap::new();
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&sprite);
            for (batch, &(first_draw, end)) in batches.iter().enumerate() {
                let texture = draws[first_draw].mesh.texture;
                let group = match atlas_groups.entry(std::ptr::from_ref(texture)) {
                    std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
                    std::collections::hash_map::Entry::Vacant(entry) => {
                        let atlas = self.atlas(device, queue, model, texture)?;
                        entry.insert(device.create_bind_group(&wgpu::BindGroupDescriptor {
                            label: Some("CS2 E-mote sprite"),
                            layout: &self.sprite_layout,
                            entries: &[
                                wgpu::BindGroupEntry { binding: 0, resource: self.commands.as_entire_binding() },
                                wgpu::BindGroupEntry { binding: 1, resource: self.pixels.as_entire_binding() },
                                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&atlas) },
                                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                                    buffer: &self.batch_info, offset: 0,
                                    size: NonZeroU64::new(info_size as u64),
                                }) },
                            ],
                        }))
                    }
                };
                let mut groups = [0; 2];
                for draw in &draws[first_draw..end] {
                    groups[0] = groups[0].max((draw.bounds[2] - draw.bounds[0]).div_ceil(8));
                    groups[1] = groups[1].max((draw.bounds[3] - draw.bounds[1]).div_ceil(8));
                }
                pass.set_bind_group(0, &*group, &[(batch * info_stride) as u32]);
                pass.dispatch_workgroups(groups[0], groups[1], (end - first_draw) as u32);
            }
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("CS2 E-mote RGBA"),
                layout: &self.pack_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: self.size.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: self.pixels.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 2, resource: self.rgba.as_entire_binding() },
                ],
            });
            pass.set_pipeline(&pack);
            pass.set_bind_group(0, &group, &[]);
            pass.dispatch_workgroups(groups, 1, 1);
        }
        if flag_bytes == 0 {
            super::submit_gpu_commands(queue, Some(encoder.finish()));
            if let (Some(started), Some(evaluated), Some(collected)) = (started, evaluated, collected) {
                let now = Instant::now();
                emote_gpu_pose([started, evaluated, collected, now, now], phases,
                    draws.len(), batches.len(), u64::from(count), 0);
            }
            return Ok(EmoteRasterFrame::from_device(
                width, bottom - y, [x as i32, y as i32],
                Arc::new(DevicePose {
                    buffer: self.rgba.clone(), device: device.clone(), queue: queue.clone(),
                    bytes: color_bytes, failed: failed.clone(), fade_alpha: None, mesh_submission: None,
                }),
            )?);
        }
        encoder.copy_buffer_to_buffer(&self.rgba, color_bytes, &self.readback, 0, flag_bytes);
        let timing = ReadbackTimer::start(ReadbackKind::Emote, flag_bytes);
        let encoded = collected.map(|_| Instant::now());
        let submission = super::submit_gpu_commands(queue, Some(encoder.finish()));
        let slice = self.readback.slice(..flag_bytes);
        let (send, receive) = mpsc::sync_channel(1);
        slice.map_async(wgpu::MapMode::Read, move |result| { let _ = send.send(result); });
        if batching() {
            let frame = EmoteRasterFrame::from_device(
                width, bottom - y, [x as i32, y as i32],
                Arc::new(DevicePose {
                    buffer: self.rgba.clone(), device: device.clone(), queue: queue.clone(),
                    bytes: color_bytes, failed: failed.clone(), fade_alpha: None, mesh_submission: None,
                }),
            )?;
            let draws = own_draws(draws);
            let readback = std::mem::replace(&mut self.readback, buffer(device, 4, DOWNLOAD));
            self.pending.push(PendingEmote {
                readback, receiver: receive, submission, timing, model: model.clone(),
                rgba: self.rgba.clone(), draws,
                viewport, bounds, milestones: [started, evaluated, collected, encoded], phases,
                batch_count: batches.len(),
            });
            return Ok(frame);
        }
        let ready = device.poll(wgpu::PollType::Wait {
            submission_index: Some(submission), timeout: Some(Duration::from_millis(250)),
        });
        if let Err(error) = ready {
            self.readback.unmap();
            return Err(error).context("wait for E-mote pose");
        }
        emote_gpu_sync(1);
        receive.try_recv().context("receive E-mote readback")?.context("map E-mote pose")?;
        let mapped = slice.get_mapped_range();
        let mut repair = vec![Vec::new(); viewport.height as usize];
        for (word, bytes) in mapped.as_chunks::<4>().0.iter().enumerate() {
            let mut bits = u32::from_le_bytes(*bytes);
            while bits != 0 {
                let pixel = word * 32 + bits.trailing_zeros() as usize;
                let row = y as usize + pixel / width as usize;
                repair[row].push(x as usize + pixel % width as usize);
                bits &= bits - 1;
            }
        }
        drop(mapped);
        self.readback.unmap();
        if let Some(timing) = timing { timing.complete(); }
        let downloaded = encoded.map(|_| Instant::now());
        let draw_count = draws.len();
        if repair.iter().any(|row| !row.is_empty()) {
            repair_emote_raster(
                viewport, &repair, [x, y], width, &mut self.repair_scratch, |target| {
                    let mut stencils = BTreeMap::<u32, CpuStencilBuffer>::new();
                    for draw in draws {
                        let pass = if draw.stencil.serial == 0 {
                            EmoteSpritePass::Color(None)
                        } else {
                            let buffer = match stencils.entry(draw.stencil.serial) {
                                std::collections::btree_map::Entry::Occupied(entry) => entry.into_mut(),
                                std::collections::btree_map::Entry::Vacant(entry) => {
                                    entry.insert(target.new_stencil(viewport, draw.stencil.clear)?)
                                }
                            };
                            match draw.operation {
                                Some(operation) => EmoteSpritePass::Mask {
                                    buffer, reference: draw.reference, operation,
                                },
                                None => EmoteSpritePass::Color(Some((buffer, draw.reference))),
                            }
                        };
                        target.draw_sprite(viewport, draw.mesh, pass)?;
                    }
                    Ok(())
                },
            )?;
            let corrections = self.repair_scratch.corrections();
            let correction_count = corrections.len() / 2;
            let correction_bytes = corrections.len() as u64 * 4;
            grow(&mut self.repairs, correction_bytes, UPLOAD);
            let bytes = unsafe {
                std::slice::from_raw_parts(corrections.as_ptr().cast::<u8>(), correction_bytes as usize)
            };
            queue.write_buffer(&self.repairs, 0, bytes);
            queue.write_buffer(&self.size, 0, &(correction_count as u32).to_le_bytes());
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("CS2 E-mote sparse repair"), layout: &self.repair_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: self.size.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: self.repairs.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 2, resource: self.rgba.as_entire_binding() },
                ],
            });
            let mut encoder = device.create_command_encoder(&Default::default());
            {
                let mut pass = encoder.begin_compute_pass(&Default::default());
                pass.set_pipeline(&self.repair);
                pass.set_bind_group(0, &group, &[]);
                pass.dispatch_workgroups(correction_count.div_ceil(64) as u32, 1, 1);
            }
            super::submit_gpu_commands(queue, Some(encoder.finish()));
        }
        #[cfg(test)]
        eprintln!("emote_repair pixels={}", repair.iter().map(Vec::len).sum::<usize>());
        if let (Some(started), Some(evaluated), Some(collected), Some(encoded), Some(downloaded)) =
            (started, evaluated, collected, encoded, downloaded)
        {
            emote_gpu_pose(
                [started, evaluated, collected, encoded, downloaded],
                phases,
                draw_count,
                batches.len(),
                u64::from(count),
                repair.iter().map(Vec::len).sum(),
            );
        }
        Ok(EmoteRasterFrame::from_device(
            width,
            bottom - y,
            [x as i32, y as i32],
            Arc::new(DevicePose {
                buffer: self.rgba.clone(), device: device.clone(), queue: queue.clone(),
                bytes: color_bytes, failed: failed.clone(), fade_alpha: None, mesh_submission: None,
            }),
        )?)
    }
}

impl Raster {
    fn blend_emote(
        &mut self, canvas: &mut Canvas, frame: &EmoteRasterFrame, pose: &DevicePose, blend: BlendMode,
    ) -> Result<bool> {
        if self.failed.load(Ordering::Relaxed) || !Arc::ptr_eq(&self.failed, &pose.failed) {
            bail!("E-mote pose belongs to an unavailable device");
        }
        pose.flush_mesh();
        let bounds = super::RasterRect { origin: [0; 2], size: [canvas.width(), canvas.height()] };
        let origin = frame.origin().map(i64::from);
        let end = [origin[0] + i64::from(frame.width()), origin[1] + i64::from(frame.height())];
        let start = [origin[0].max(0), origin[1].max(0)];
        let clipped_end = [end[0].min(i64::from(canvas.width())), end[1].min(i64::from(canvas.height()))];
        let target = if blend == BlendMode::SourceOver {
            if (0..2).any(|axis| start[axis] >= clipped_end[axis]) { return Ok(true); }
            super::RasterRect {
                origin: start.map(|value| value as u32),
                size: std::array::from_fn(|axis| (clipped_end[axis] - start[axis]) as u32),
            }
        } else {
            bounds
        };
        let target_bytes = u64::from(canvas.width()) * u64::from(canvas.height()) * 4;
        let limits = self.device.limits();
        if target_bytes > u64::from(limits.max_storage_buffer_binding_size)
            || pose.bytes > u64::from(limits.max_storage_buffer_binding_size)
            || target.size.iter().any(|&size| size.div_ceil(8) > limits.max_compute_workgroups_per_dimension)
        {
            return Ok(false);
        }
        self.full_target = None;
        self.previous_planes = None;
        self.ensure_target_size(target_bytes);
        let previous = device_frame(canvas)
            .filter(|frame| Arc::ptr_eq(&frame.failed, &self.failed) && !frame.failed.load(Ordering::Relaxed))
            .map(|frame| frame.buffer.clone());
        let mut encoder = self.device.create_command_encoder(&Default::default());
        if let Some(previous) = previous {
            encoder.copy_buffer_to_buffer(&previous, 0, &self.target, 0, target_bytes);
        } else {
            self.queue.write_buffer(&self.target, 0, canvas.pixels());
        }
        let params = [
            target.origin[0], target.origin[1], target.size[0], target.size[1],
            frame.origin()[0] as u32, frame.origin()[1] as u32, frame.width(), frame.height(),
            canvas.width(), blend_code(blend), 0, 0,
        ];
        let params: Vec<_> = params.into_iter().flat_map(u32::to_le_bytes).collect();
        self.queue.write_buffer(&self.device_blend_params, 0, &params);
        let group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("CS2 device pose blend"), layout: &self.device_blend_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.target.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: pose.buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: self.device_blend_params.as_entire_binding() },
            ],
        });
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&self.device_blend);
            pass.set_bind_group(0, &group, &[]);
            pass.dispatch_workgroups(target.size[0].div_ceil(8), target.size[1].div_ceil(8), 1);
        }
        super::submit_gpu_commands(&self.queue, Some(encoder.finish()));
        let output = self.publish_device_frame(bounds.size);
        if self.direct_output_enabled() {
            canvas.set_deferred_device_pixels(output);
        } else {
            let mut pixels = vec![0; target_bytes as usize];
            if !cs2_renderer::CanvasReadback::read_rgba(output.as_ref(), &mut pixels) {
                bail!("read device-composited E-mote frame");
            }
            canvas.set_verified_device_pixels(output, &pixels)?;
        }
        static REPORTED: AtomicBool = AtomicBool::new(false);
        if !REPORTED.swap(true, Ordering::Relaxed) {
            report(&format!("draw=emote_device_blend backend={}", self.backend));
        }
        Ok(true)
    }

    fn ensure_emote(&mut self) -> Result<()> {
        if self.failed.load(Ordering::Relaxed) { bail!("E-mote device unavailable"); }
        if self.emote_initialization_failed { bail!("E-mote shader initialization failed"); }
        if self.emote.is_none() {
            let renderer = EmoteRaster::new(&self.device).inspect_err(|_| {
                self.emote_initialization_failed = true;
            })?;
            self.emote = Some(renderer);
        }
        Ok(())
    }

    pub(super) fn render_emote(
        &mut self, player: &CpuPlayer, viewport: CpuViewport,
        variables: &BTreeMap<String, f64>, allow_warnings: bool,
    ) -> Result<EmoteRasterFrame> {
        self.ensure_emote()?;
        let mut frame = self.emote.as_mut().unwrap().render(&self.device, &self.queue, player, viewport, variables, allow_warnings, &self.failed)?;
        if self.failed.load(Ordering::Relaxed) { bail!("E-mote device failed during draw"); }
        if !super::emote_device_blend_requested() {
            frame = EmoteRasterFrame::new(
                frame.width(), frame.height(), frame.origin(), frame.rgba().to_vec(),
            )?;
        }
        Ok(frame)
    }
}


#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use cs2_runtime::{ResourceIndex, emote::EmoteResource};
    use std::{path::PathBuf, time::Instant};

    #[test]
    fn atlas_upload_window_shares_deadline_across_retries_and_preloads() {
        let start = Instant::now();
        let mut window = AtlasUploadWindow::default();
        window.refresh(start);
        assert_eq!(window.speculative_remaining + window.scene_remaining, 8 * 1024 * 1024);
        window.scene_remaining = 0;
        window.speculative_remaining /= 2;
        window.refresh(start + Duration::from_millis(16));
        assert_eq!(window.scene_remaining, 0);
        assert_eq!(window.speculative_remaining, ATLAS_STAGE_BYTES / 2);
        window.refresh(start + ATLAS_UPLOAD_WINDOW);
        assert_eq!(window.speculative_remaining, ATLAS_STAGE_BYTES);
        assert_eq!(window.scene_remaining, SCENE_ATLAS_STAGE_BYTES);
    }

    #[test]
    #[ignore = "requires hardware DX12 and the local PSZ fixture"]
    fn interactive_scene_composes_while_one_pose_worker_is_delayed() -> Result<()> {
        use cs2_runtime::emote::{EmoteTick, NEKOPARA_PE_SHA256};
        struct Scratch(PathBuf);
        impl Drop for Scratch {
            fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
        }
        let scratch = Scratch(std::env::temp_dir().join(format!("cs2-emote-jobs-{}", std::process::id())));
        std::fs::create_dir_all(&scratch.0)?;
        let root = PathBuf::from(std::env::var_os("CS2_EMOTE_FIXTURE_ROOT").context("model directory")?);
        let mut index = ResourceIndex::scan_with_name_seed(&root, Some(0x0F6BB0AE))?;
        index.initialize_resource_router()?;
        let bytes = index.read_archive_with_name_seed("psz.int", b"ca01_l_body_1.psz", 0x0F6BB0AE)?;
        let mut host = crate::HeadlessPlayer::open_with_save_root(&scratch.0, scratch.0.join("save"))?;
        crate::gpu_raster::enable();
        host.data_services.emote.initialize(&index, NEKOPARA_PE_SHA256)?;
        host.enable_host_emote_pose_jobs();
        assert!(pose_jobs_available(), "test requires the interactive indexed GPU path");
        assert!(prep_worker_limit() >= 2, "test requires two pose workers");
        host.canvas = Canvas::new(1280, 720)?;
        host.graphics_canvas = Canvas::new(1280, 720)?;
        let mut actors = Vec::new();
        for _ in 0..2 {
            let task = host.with_host(|host| Ok(host.create_task_object(54, 0, b"emote-worker", b"")?))?;
            actors.push(task);
            host.data_services.emote.state.import_psz(task, b"ca01_l_body_1.psz", &bytes)?;
            let node = host.scene_nodes.get_mut(&task).unwrap();
            node.visible = true;
            node.initialized = true;
        }
        let advance = |host: &mut crate::HeadlessPlayer, tick: u32| -> Result<()> {
            host.state.clock.frame = u64::from(tick);
            for &task in &actors {
                host.data_services.emote.state.advance(task, EmoteTick {
                    task_elapsed: tick, lip_elapsed: None, increment: 1,
                })?;
            }
            Ok(())
        };
        let [slow, other] = [actors[0], actors[1]];
        advance(&mut host, 1)?;
        let first_scene_start = Instant::now();
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut preflight_retries = 0;
        while !host.preflight_scene_emote_poses()? {
            assert!(Instant::now() < deadline, "cold actor failed to prepare");
            preflight_retries += 1;
            std::thread::sleep(Duration::from_millis(1));
        }
        let first_scene_ready = Instant::now();
        host.render_scene()?;
        println!("emote_first_scene actors=2 preflight_retries={preflight_retries} ready_us={} render_us={} total_us={}",
            (first_scene_ready - first_scene_start).as_micros(), first_scene_ready.elapsed().as_micros(),
            first_scene_start.elapsed().as_micros());
        assert_eq!(host.data_services.emote.displayed_pose_tick(slow), Some(1));
        assert_eq!(host.data_services.emote.displayed_pose_tick(other), Some(1));

        let gate = Arc::new(PoseTestGate::default());
        host.data_services.emote.set_pose_test_gate(slow, gate.clone());
        advance(&mut host, 2)?;
        assert!(host.preflight_scene_emote_poses()?, "prior valid samples allow immediate composition");
        host.render_scene()?;
        let deadline = Instant::now() + Duration::from_secs(2);
        while !gate.started.load(Ordering::Acquire) {
            assert!(Instant::now() < deadline, "test worker failed to reach its gate");
            std::thread::sleep(Duration::from_millis(1));
        }
        let held = Instant::now();
        while held.elapsed() < Duration::from_millis(200) {
            assert!(host.preflight_scene_emote_poses()?, "slow actor cannot gate ordinary composition");
            host.render_scene()?;
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(host.data_services.emote.displayed_pose_tick(slow), Some(1));
        assert_eq!(host.data_services.emote.displayed_pose_tick(other), Some(2));
        gate.released.store(true, Ordering::Release);
        let deadline = Instant::now() + Duration::from_secs(2);
        while host.data_services.emote.displayed_pose_tick(slow) != Some(2) {
            assert!(Instant::now() < deadline, "delayed actor failed to resume");
            assert!(host.preflight_scene_emote_poses()?);
            host.render_scene()?;
            std::thread::sleep(Duration::from_millis(1));
        }

        // Captures require the current pose and keep the last full scene
        // visible while the same background worker is held.
        host.data_services.frame_capture.enabled = 1;
        let previous = host.frame_rgba().to_vec();
        gate.started.store(false, Ordering::Release);
        gate.released.store(false, Ordering::Release);
        advance(&mut host, 3)?;
        assert!(!host.preflight_scene_emote_poses()?);
        let deadline = Instant::now() + Duration::from_secs(2);
        while !gate.started.load(Ordering::Acquire) {
            assert!(Instant::now() < deadline, "capture pose failed to reach its gate");
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(!host.preflight_scene_emote_poses()?);
        assert_eq!(host.frame_rgba(), previous);
        gate.released.store(true, Ordering::Release);
        while !host.preflight_scene_emote_poses()? {
            assert!(Instant::now() < deadline, "capture pose failed to prepare");
            std::thread::sleep(Duration::from_millis(1));
        }
        host.render_scene()?;
        assert_eq!(host.data_services.emote.displayed_pose_tick(slow), Some(3));
        let asynchronous = host.frame_rgba().to_vec();
        host.render_scene()?;
        assert_eq!(host.frame_rgba(), asynchronous);
        Ok(())
    }

    #[test]
    fn active_four_large_atlases_fit_without_lru_churn() {
        assert!(ATLAS_DRAW_LIMIT >= 4 * 64 * 1024 * 1024,
            "four active 64MiB atlases must fit the bounded scene budget");
    }

    #[test]
    fn active_atlas_overbudget_declines_without_evicting_live_entries() {
        assert_eq!(atlas_eviction_candidate([true, true, true, true].into_iter()), None);
        assert_eq!(atlas_eviction_candidate([true, false, true].into_iter()), Some(1));
        assert_eq!(atlas_eviction_candidate([].into_iter()), None);
    }

    #[test]
    #[ignore = "requires hardware DX12 and CS2_EMOTE_DEVICE_BLEND=1"]
    fn gpu_final_tint_preserves_rounding_transparent_rgb_and_endpoint_order() -> Result<()> {
        let mut raster = Raster::new()?;
        raster.test_direct_output = true;
        raster.ensure_emote()?;
        let pixels: Vec<u8> = (0..=255).flat_map(|v| [v, 255 - v, v / 2, v]).collect();
        let source = Arc::new(buffer(&raster.device, pixels.len() as u64, TARGET));
        raster.queue.write_buffer(&source, 0, &pixels);
        let frame = raster.bind_fade_output([256, 1], [-2, 3], source, pixels.len() as u64, None).unwrap();
        RASTER.with_borrow_mut(|slot| *slot = Some(raster));
        for argb in [0x00000000u32, 0xffffffff, 0x7f814023, 0x01010101, 0x807f8081] {
            let actual = tint_emote_surface(&frame, argb).context("tint must retain a resident device frame")?;
            assert!(actual.device().is_some());
            assert_eq!(actual.origin(), frame.origin());
            let tint = [argb >> 16 & 255, argb >> 8 & 255, argb & 255, argb >> 24];
            let expected: Vec<_> = pixels.iter().enumerate().map(|(i, &v)| ((u32::from(v) * tint[i % 4] + 127) / 255) as u8).collect();
            assert_eq!(actual.rgba(), expected);
        }
        // Tinting is a final operation and never modifies a held endpoint.
        assert_eq!(frame.rgba(), pixels);
        let cpu = EmoteRasterFrame::new(1, 1, [0, 0], vec![1, 2, 3, 0])?;
        assert!(tint_emote_surface(&cpu, 0x80808080).is_none());
        RASTER.with_borrow_mut(Option::take);
        Ok(())
    }

    #[test]
    #[ignore = "requires hardware DX12"]
    fn gpu_retiring_renderer_invalidates_held_device_pose_without_readback() -> Result<()> {
        let raster = Raster::new()?;
        let output = Arc::new(buffer(&raster.device, 4, TARGET));
        let frame = raster.bind_fade_output([1, 1], [0, 0], output, 4, None).unwrap();
        assert!(frame.is_valid());
        drop(raster);
        assert!(!frame.is_valid());
        assert!(!frame.device().unwrap().read_rgba(&mut [0; 4]));
        Ok(())
    }

    #[test]
    #[ignore = "requires hardware DX12 and the local PSZ fixture"]
    fn gpu_four_active_atlases_finish_staging_and_stay_resident() -> Result<()> {
        let root = PathBuf::from(std::env::var_os("CS2_EMOTE_FIXTURE_ROOT").context("model directory")?);
        let index = ResourceIndex::scan_with_name_seed(&root, Some(0x0F6BB0AE))?;
        let bytes = index.read_archive_with_name_seed("psz.int", b"ca01_l_body_1.psz", 0x0F6BB0AE)?;
        let resource = EmoteResource::decode(&bytes)?;
        let template = CpuMotionModel::load(&resource)?;
        let models: Vec<_> = (0..4).map(|i| {
            let mut model = template.clone();
            model.textures.clear();
            model.textures.insert("test".into(), CpuTexture::from_rgba(4096, 4096, vec![i; 64 * 1024 * 1024]).unwrap());
            Arc::new(model)
        }).collect();
        let mut raster = Raster::new()?;
        raster.ensure_emote()?;
        let emote = raster.emote.as_mut().unwrap();
        emote.begin_atlas_batch();
        emote.atlas_batch_models.extend(models.iter().map(Arc::downgrade));
        for _ in 0..32 {
            for model in &models {
                assert_eq!(emote.stage_atlas(&raster.device, &raster.queue, model)?, ATLAS_STAGE_BYTES);
            }
            super::super::submit_gpu_commands(&raster.queue, []);
        }
        assert_eq!(emote.atlases.len(), 4);
        assert_eq!(emote.atlas_bytes, 256 * 1024 * 1024);
        for _ in 0..4 {
            emote.begin_atlas_batch();
            emote.atlas_batch_models.extend(models.iter().map(Arc::downgrade));
            for model in &models {
                let source = &model.textures["test"];
                let index = emote.atlas_storage(&raster.device, model, source, false)?.unwrap();
                let atlas = &mut emote.atlases[index];
                assert_eq!(atlas.uploaded_rows, 4096);
                assert_eq!(atlas.upload(&raster.queue, source.upload_pixels().0, usize::MAX), 0);
            }
        }
        let overflow = Arc::new((*models[0]).clone());
        assert!(emote.atlas_storage(&raster.device, &overflow, &overflow.textures["test"], false)?.is_none());
        assert_eq!(emote.atlases.len(), 4);
        assert_eq!(emote.atlas_bytes, ATLAS_DRAW_LIMIT);
        Ok(())
    }

    #[test]
    #[ignore = "requires hardware DX12 and the local PSZ fixture"]
    fn gpu_scene_atlas_requires_full_upload_and_exact_path_can_finish_tail() -> Result<()> {
        let root = PathBuf::from(std::env::var_os("CS2_EMOTE_FIXTURE_ROOT").context("model directory")?);
        let index = ResourceIndex::scan_with_name_seed(&root, Some(0x0F6BB0AE))?;
        let bytes = index.read_archive_with_name_seed("psz.int", b"ca01_l_body_1.psz", 0x0F6BB0AE)?;
        let resource = EmoteResource::decode(&bytes)?;
        let mut model = CpuMotionModel::load(&resource)?;
        model.textures.clear();
        let atlas_bytes = 8 * 1024 * 1024;
        model.textures.insert("test".into(), CpuTexture::from_rgba(2048, 1024, vec![37; atlas_bytes])?);
        let model = Arc::new(model);
        let texture = &model.textures["test"];
        let mut raster = Raster::new()?;
        raster.ensure_emote()?;
        let emote = raster.emote.as_mut().unwrap();
        let mut remaining = SCENE_ATLAS_STAGE_BYTES;
        assert_eq!(emote.stage_scene_atlas(&raster.device, &raster.queue, &model, texture, &mut remaining)?, Some(false));
        assert_eq!(remaining, 0);
        super::super::submit_gpu_commands(&raster.queue, []);

        REQUIRE_RESIDENT_ATLASES.with(|resident| {
            resident.set(true);
            let result = emote.atlas(&raster.device, &raster.queue, &model, texture);
            resident.set(false);
            assert!(result.is_err(), "interactive drawing must reject incomplete atlas pixels");
        });
        assert_eq!(emote.atlases[0].uploaded_rows, 768);
        assert_eq!(emote.atlas_bytes, atlas_bytes);
        assert_eq!(emote.stage_scene_atlas(&raster.device, &raster.queue, &model, texture, &mut remaining)?, Some(false));

        // Exact consumers can complete the remaining rows in order.
        let exact = emote.atlas(&raster.device, &raster.queue, &model, texture)?;
        super::super::submit_gpu_commands(&raster.queue, []);
        assert_eq!(emote.atlases[0].uploaded_rows, 1024);
        REQUIRE_RESIDENT_ATLASES.with(|resident| -> Result<()> {
            resident.set(true);
            let result = emote.atlas(&raster.device, &raster.queue, &model, texture);
            resident.set(false);
            assert_eq!(result?, exact);
            Ok(())
        })?;
        Ok(())
    }

    #[test]
    #[ignore = "requires hardware DX12, CS2_EMOTE_DEVICE_BLEND=1 and the local PSZ fixture"]
    fn gpu_device_endpoint_restores_capture_mode_and_never_strands_pending_readbacks() -> Result<()> {
        let root = PathBuf::from(std::env::var_os("CS2_EMOTE_FIXTURE_ROOT").context("model directory")?);
        let index = ResourceIndex::scan_with_name_seed(&root, Some(0x0F6BB0AE))?;
        let bytes = index.read_archive_with_name_seed("psz.int", b"ca01_l_body_1.psz", 0x0F6BB0AE)?;
        let resource = EmoteResource::decode(&bytes)?;
        let mut player = CpuPlayer::new(CpuMotionModel::load(&resource)?);
        let viewport = CpuViewport { width: 64, height: 64, origin_x: 32., origin_y: 60., scale: 0.05, time_units_per_frame: 1. };
        player.advance_frames(1.)?;
        player.update_reference_timelines()?;
        assert!(player.render_preview(viewport)?.warnings.is_empty(), "endpoint fixture must follow the ordinary player reference-update protocol");
        let variables = player.effective_variables()?;
        let mut raster = Raster::new()?;
        raster.test_direct_output = true;
        raster.ensure_emote()?;
        RASTER.with_borrow_mut(|slot| *slot = Some(raster));
        for fallback in [false, true] {
            if fallback { RASTER.with_borrow_mut(|slot| slot.as_mut().unwrap().emote.as_mut().unwrap().mesh = None); }
            let frame = draw_emote_surface(&player, viewport, &variables).context("resident endpoint")?;
            assert!(frame.device().is_some());
            assert_eq!([frame.width(), frame.height()], [64, 64]);
            assert!(!batching());
            assert!(!CAPTURING.with(Cell::get));
            assert!(RASTER.with_borrow(|slot| slot.as_ref().unwrap().emote.as_ref().unwrap().pending.is_empty()));
        }
        RASTER.with_borrow_mut(Option::take);
        assert!(draw_emote_surface(&player, viewport, &variables).is_none());
        assert!(!CAPTURING.with(Cell::get));
        Ok(())
    }

    #[test]
    fn indexed_geometry_reservation_is_bounded_and_rejection_preserves_budget() {
        let reserved = std::sync::atomic::AtomicUsize::new(0);
        for _ in 0..4 { assert!(reserve_indexed_geometry(&reserved, INDEXED_PREP_GEOMETRY_BYTES / 4)); }
        assert!(!reserve_indexed_geometry(&reserved, 1));
        assert!(!reserve_indexed_geometry(&reserved, usize::MAX));
        assert_eq!(reserved.load(Ordering::Relaxed), INDEXED_PREP_GEOMETRY_BYTES);
    }

    #[test]
    #[ignore = "requires the local PSZ fixture"]
    fn indexed_unchanged_pose_skips_evaluation_and_invalidates_exact_inputs() -> Result<()> {
        let root = PathBuf::from(std::env::var_os("CS2_EMOTE_FIXTURE_ROOT").context("model directory")?);
        let index = ResourceIndex::scan_with_name_seed(&root, Some(0x0F6BB0AE))?;
        let bytes = index.read_archive_with_name_seed("psz.int", b"ca01_l_body_1.psz", 0x0F6BB0AE)?;
        let resource = EmoteResource::decode(&bytes)?;
        let mut players = vec![CpuPlayer::new(CpuMotionModel::load(&resource)?); 2];
        let viewport = CpuViewport { width: 1280, height: 720, origin_x: 640., origin_y: 600., scale: 1., time_units_per_frame: 1. };
        INDEXED_CACHES.with_borrow_mut(Vec::clear);
        let prepare = |players: &[CpuPlayer], viewport| {
            prepare_indexed_poses(&players.iter().map(|p| (p, viewport)).collect::<Vec<_>>());
            INDEXED_CACHES.with_borrow(|caches| caches.iter().map(|c| c.geometry.counters()).collect::<Vec<_>>())
        };
        let first = prepare(&players, viewport);
        let unchanged = prepare(&players, viewport);
        assert_eq!(unchanged, first, "identical exact state must skip mesh inspection and primitive evaluation");
        assert_eq!(PREPARED.with_borrow(Vec::len), 2, "cache hits still supply both actual actors");
        players[0].set_variable("head_UD", 20.)?;
        let changed = prepare(&players, viewport);
        assert_ne!(changed[0], unchanged[0], "changed effective variables invalidate actor zero");
        assert_eq!(changed[1], unchanged[1], "one actor's changes cannot invalidate the other");
        players[1].frame += 1.;
        let advanced = prepare(&players, viewport);
        assert_ne!(advanced[1], changed[1], "motion clock invalidates even with unchanged variables");
        players[0].set_root_affine([1., 0., 0., 1., 1., 0.])?;
        let moved = prepare(&players, viewport);
        assert_ne!(moved[0], advanced[0], "root movement invalidates the exact state");
        let resized = prepare(&players, CpuViewport { width: 1279, ..viewport });
        assert_ne!(resized, moved, "viewport changes invalidate both actors");
        prepare_indexed_poses(&[]);
        Ok(())
    }

    #[test]
    #[ignore = "requires the local PSZ fixture"]
    fn indexed_five_pose_preparation_uses_bounded_workers_and_is_consumed() -> Result<()> {
        let root = PathBuf::from(std::env::var_os("CS2_EMOTE_FIXTURE_ROOT").context("model directory")?);
        let index = ResourceIndex::scan_with_name_seed(&root, Some(0x0F6BB0AE))?;
        let bytes = index.read_archive_with_name_seed("psz.int", b"ca01_l_body_1.psz", 0x0F6BB0AE)?;
        let resource = EmoteResource::decode(&bytes)?;
        let players = vec![CpuPlayer::new(CpuMotionModel::load(&resource)?); 5];
        let viewport = CpuViewport { width: 1280, height: 720, origin_x: 640., origin_y: 600., scale: 1., time_units_per_frame: 1. };
        prepare_indexed_poses(&players.iter().map(|player| (player, viewport)).collect::<Vec<_>>());
        assert_eq!(PREPARED.with_borrow(Vec::len), 5, "actor count must not be confused with the worker count");
        let four_actor_reservation = INDEXED_CACHES.with_borrow(|caches|
            caches.iter().take(4).map(|cache| cache.reservation_bytes).sum::<usize>());
        assert!(four_actor_reservation <= INDEXED_PREP_GEOMETRY_BYTES,
            "actual four-actor geometry must fit the bounded preparation budget");
        println!("indexed_geometry_budget actors=4 reserved_bytes={four_actor_reservation} limit_bytes={INDEXED_PREP_GEOMETRY_BYTES}");
        for player in &players {
            let prepared = take_prepared_pose(player, viewport, &player.effective_variables()?).context("prepared fifth actor")?;
            assert!(prepared.indexed.is_some());
            assert!(!prepared.draws.is_empty());
        }
        assert_eq!(PREPARED.with_borrow(Vec::len), 0);
        prepare_indexed_poses(&[]);
        assert_eq!(INDEXED_CACHES.with_borrow(Vec::len), 0);
        Ok(())
    }

    #[test]
    #[ignore = "requires hardware DX12 and the local PSZ fixture"]
    fn headless_scene_prepares_single_and_multiple_emote_actors_on_changed_frames() -> Result<()> {
        use cs2_runtime::emote::{EmoteTick, NEKOPARA_PE_SHA256};

        struct Scratch(PathBuf);
        impl Drop for Scratch {
            fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
        }
        let suffix = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos();
        let scratch = Scratch(std::env::temp_dir().join(format!("cs2-emote-scene-{suffix}")));
        std::fs::create_dir_all(&scratch.0)?;
        let root = PathBuf::from(std::env::var_os("CS2_EMOTE_FIXTURE_ROOT").context("model directory")?);
        let mut index = ResourceIndex::scan_with_name_seed(&root, Some(0x0F6BB0AE))?;
        index.initialize_resource_router()?;
        let bytes = index.read_archive_with_name_seed("psz.int", b"ca01_l_body_1.psz", 0x0F6BB0AE)?;
        let mut host = crate::HeadlessPlayer::open_with_save_root(&scratch.0, scratch.0.join("save"))?;
        crate::gpu_raster::enable();
        assert!(crate::gpu_raster::emote_batch_requested(), "test requires the production batched GPU path");
        assert!(mesh::enabled(), "test requires indexed geometry");
        host.data_services.emote.initialize(&index, NEKOPARA_PE_SHA256)?;
        host.canvas = Canvas::new(1280, 720)?;
        host.graphics_canvas = Canvas::new(1280, 720)?;
        let viewport = CpuViewport { width: 1280, height: 720, origin_x: 640., origin_y: 600., scale: 1., time_units_per_frame: 1. };
        host.data_services.emote.state.set_viewport(viewport)?;
        let mut actors = Vec::new();
        let mut progress_versions = BTreeMap::new();
        for count in 1..=2 {
            let task = host.with_host(|host| Ok(host.create_task_object(54, 0, b"emote-regression", b"")?))?;
            actors.push(task);
            host.data_services.emote.state.import_psz(task, b"ca01_l_body_1.psz", &bytes)?;
            let node = host.scene_nodes.get_mut(&task).unwrap();
            node.visible = true;
            node.initialized = true;
            let mut previous_pixels = None;
            for tick in 0..3 {
                for (i, &task) in actors.iter().enumerate() {
                    host.data_services.emote.state.set_root_affine(task,
                        [1., 0., 0., 1., (i * 12 + tick * 7) as f64, 0.])?;
                    host.data_services.emote.state.advance(task, EmoteTick {
                        task_elapsed: (count * 3 + tick) as u32, lip_elapsed: None, increment: 1,
                    })?;
                }
                INDEXED_RENDER_PATHS.with(|counts| counts.set([0; 2]));
                let started = Instant::now();
                let (result, details) = crate::perf_detail::HostPerfDetails::capture(false, || -> Result<()> {
                    host.render_scene()?;
                    host.data_services.emote.trace_visual_progress();
                    Ok(())
                });
                result?;
                let progress = details.rows(91, (count * 3 + tick) as u64, 1);
                println!("{progress}");
                for &task in &actors {
                    let row = progress.lines().find(|line| line.starts_with("visual_pose ")
                        && line.split_whitespace().any(|field| field == format!("task={task}")))
                        .expect("real scene entry must record each actor");
                    let value = |key: &str| -> u64 { row.split_whitespace()
                        .find_map(|field| field.strip_prefix(key)).unwrap().parse().unwrap() };
                    assert_eq!(value("input_known="), 1, "{row}");
                    let revision = value("pose_in=");
                    let previous = progress_versions.insert(task, revision).unwrap_or(0);
                    assert!(revision > previous, "changed actual player inputs must change pose version: {row}");
                    assert_eq!(value("render_in="), revision, "{row}");
                    assert_eq!(value("scene_input="), revision, "{row}");
                    assert_eq!(value("surface_scene="), value("surface_rev="), "{row}");
                }
                let render_us = started.elapsed().as_micros();
                let [prepared, fallback] = INDEXED_RENDER_PATHS.with(Cell::get);
                println!("headless_scene_entry actors={count} tick={tick} prepared_consumed={prepared} fallback_geometry={fallback} render_us={render_us}");
                assert_eq!([prepared, fallback], [count, 0],
                    "every actual scene actor must consume prepared geometry, including one actor");
                assert_eq!(PREPARED.with_borrow(Vec::len), 0);
                let pixels = host.canvas.pixels().to_vec();
                assert!(pixels.chunks_exact(4).any(|pixel| pixel[..3] != [0; 3]), "actual scene must contain the imported model");
                if let Some(previous) = previous_pixels {
                    assert!(pixels != previous, "changed actor transforms must change actual scene pixels");
                }
                previous_pixels = Some(pixels);
            }
        }
        crate::gpu_raster::disable();
        INDEXED_CACHES.with_borrow_mut(Vec::clear);
        Ok(())
    }

    #[test]
    #[ignore = "requires hardware DX12 and the local PSZ fixture"]
    fn gpu_indexed_multi_pose_preparation_is_consumed_and_invalidated() -> Result<()> {
        let root = PathBuf::from(std::env::var_os("CS2_EMOTE_FIXTURE_ROOT").context("model directory")?);
        let index = ResourceIndex::scan_with_name_seed(&root, Some(0x0F6BB0AE))?;
        let bytes = index.read_archive_with_name_seed("psz.int", b"ca01_l_body_1.psz", 0x0F6BB0AE)?;
        let resource = EmoteResource::decode(&bytes)?;
        let mut players = vec![CpuPlayer::new(CpuMotionModel::load(&resource)?); 2];
        players[1].set_root_affine([1., 0., 0., 1., 8., 0.])?;
        let viewport = CpuViewport { width: 1280, height: 720, origin_x: 640., origin_y: 600., scale: 1., time_units_per_frame: 1. };
        let mut raster = Raster::new()?;
        raster.ensure_emote()?;
        while raster.emote.as_mut().unwrap().stage_atlas(&raster.device, &raster.queue, &players[0].model)? != 0 {
            super::super::submit_gpu_commands(&raster.queue, []);
        }
        RASTER.with_borrow_mut(|slot| *slot = Some(raster));
        let references: Vec<_> = players.iter().map(|player| (player, viewport)).collect();
        prepare_emote_poses(&references);
        assert_eq!(PREPARED.with_borrow(Vec::len), 2, "indexed poses must use bounded CPU preparation");
        let batch = begin_emote_batch()?;
        let mut first_rgba = Vec::new();
        for (i, player) in players.iter().enumerate() {
            let variables = player.effective_variables()?;
            let frame = RASTER.with_borrow_mut(|slot| slot.as_mut().unwrap()
                .render_emote(player, viewport, &variables, true))?;
            assert!(frame.device().is_some());
            assert!(frame.rgba().chunks_exact(4).any(|pixel| pixel[3] != 0));
            if i == 0 { first_rgba.extend_from_slice(frame.rgba()); }
            assert_eq!(PREPARED.with_borrow(Vec::len), 1 - i, "prepared geometry must be consumed");
        }
        batch.finish()?;
        prepare_emote_poses(&players.iter().map(|player| (player, viewport)).collect::<Vec<_>>());
        let batch = begin_emote_batch()?;
        let replay = RASTER.with_borrow_mut(|slot| slot.as_mut().unwrap().render_emote(
            &players[0], viewport, &players[0].effective_variables()?, true))?;
        assert_eq!(replay.rgba(), first_rgba, "switching back to a cached earlier pose restores its GPU geometry");
        assert!(RASTER.with_borrow(|slot| slot.as_ref().unwrap().emote.as_ref().unwrap()
            .mesh.as_ref().unwrap().geometry_upload_bytes > 0), "an older cached pose must replace the last resident pose");
        batch.finish()?;
        prepare_emote_poses(&players.iter().map(|player| (player, viewport)).collect::<Vec<_>>());
        let prior_frame = players[0].frame.to_bits();
        let prior_variables = players[0].effective_variables()?;
        players[0].update_reference_timelines()?;
        assert_eq!(players[0].frame.to_bits(), prior_frame);
        assert_eq!(players[0].effective_variables()?, prior_variables);
        assert!(take_prepared_pose(&players[0], viewport, &prior_variables).is_none(),
            "same-frame reference-clock changes must reject previously prepared geometry");
        players[0].set_variable("head_UD", 20.)?;
        prepare_emote_poses(&players.iter().map(|player| (player, viewport)).collect::<Vec<_>>());
        let changed = players[0].effective_variables()?;
        assert!(take_prepared_pose(&players[0], viewport, &changed).is_some());
        players[1].set_variable("head_UD", -20.)?;
        assert!(take_prepared_pose(&players[1], viewport, &players[1].effective_variables()?).is_none(),
            "changed variables must reject previously prepared geometry");
        prepare_emote_poses(&players.iter().map(|player| (player, viewport)).collect::<Vec<_>>());
        players[0].frame += 1.;
        assert!(take_prepared_pose(&players[0], viewport, &players[0].effective_variables()?).is_none(),
            "motion clock changes must reject previously prepared geometry");
        players[1].set_root_affine([1., 0., 0., 1., 9., 0.])?;
        assert!(take_prepared_pose(&players[1], viewport, &players[1].effective_variables()?).is_none(),
            "root transforms must reject previously prepared geometry");
        PREPARED.with_borrow_mut(Vec::clear);
        RASTER.with_borrow_mut(Option::take);
        Ok(())
    }

    #[test]
    #[ignore = "requires hardware DX12 and the local PSZ fixture"]
    fn gpu_indexed_group_uses_independent_uploads_and_flushes_before_consumers() -> Result<()> {
        let root = PathBuf::from(std::env::var_os("CS2_EMOTE_FIXTURE_ROOT").context("model directory")?);
        let index = ResourceIndex::scan_with_name_seed(&root, Some(0x0F6BB0AE))?;
        let bytes = index.read_archive_with_name_seed("psz.int", b"ca01_l_body_1.psz", 0x0F6BB0AE)?;
        let model = CpuMotionModel::load(&EmoteResource::decode(&bytes)?)?;
        let mut players = vec![CpuPlayer::new(model); 4];
        for (index, player) in players.iter_mut().enumerate() {
            let x = index as f64;
            player.set_root_affine([1., 0., x * 0.05, 1., 85. * (x - 1.), 0.])?;
            player.set_variable("head_UD", x * 8. - 12.)?;
        }
        let viewport = CpuViewport {
            width: 1280, height: 720, origin_x: 640., origin_y: 600., scale: 1., time_units_per_frame: 1.,
        };
        let mut raster = Raster::new()?;
        raster.ensure_emote()?;
        while raster.emote.as_mut().unwrap().stage_atlas(&raster.device, &raster.queue, &players[0].model)? != 0 {
            super::super::submit_gpu_commands(&raster.queue, []);
        }
        let submission = raster.emote.as_ref().unwrap().mesh.as_ref().unwrap().submission.clone();
        RASTER.with_borrow_mut(|slot| *slot = Some(raster));
        let references: Vec<_> = players.iter().map(|player| (player, viewport)).collect();
        let render = |player: &CpuPlayer| -> Result<EmoteRasterFrame> {
            let variables = player.effective_variables()?;
            RASTER.with_borrow_mut(|slot| slot.as_mut().unwrap().render_emote(player, viewport, &variables, true))
        };
        let batch = begin_emote_batch()?;
        prepare_emote_poses(&references);
        let mut expected = Vec::new();
        for player in &players {
            let frame = render(player)?;
            let rgba = frame.rgba().to_vec();
            assert!(rgba.chunks_exact(4).any(|pixel| pixel[3] != 0));
            assert_eq!(submission.pending_poses(), 0, "CPU readback must submit the pose");
            let mut checksum = DefaultHasher::new();
            (frame.origin(), &rgba).hash(&mut checksum);
            println!("emote_coverage_fixture actor={} checksum={:016x}", expected.len(), checksum.finish());
            expected.push((frame.origin(), rgba));
        }
        let tinted_expected = tint_emote_surface(&render(&players[1])?, 0xff80ff40).context("reference tint")?;
        assert_eq!(submission.pending_poses(), 0, "tint must submit the source pose");
        let tinted_expected = tinted_expected.rgba().to_vec();
        batch.finish()?;
        assert!(expected.windows(2).all(|pair| pair[0] != pair[1]));

        // Force a generation wrap during the four-pose group. The queued
        // first pose still needs its original coverage before the clear.
        RASTER.with_borrow_mut(|slot| {
            let mesh = slot.as_mut().unwrap().emote.as_mut().unwrap().mesh.as_mut().unwrap();
            mesh.coverage_stamp = 16_777_145;
        });
        let batch = begin_emote_batch()?;
        prepare_emote_poses(&references);
        let mut frames = Vec::new();
        for (index, player) in players.iter().enumerate() {
            frames.push(render(player)?);
            let pending = if index < 3 && mesh::batch_submissions_enabled() { index + 1 } else { 0 };
            assert_eq!(submission.pending_poses(), pending);
        }
        batch.seal();
        for (frame, (origin, rgba)) in frames.iter().zip(&expected) {
            assert_eq!(frame.origin(), *origin);
            assert_eq!(frame.rgba(), rgba.as_slice());
        }
        batch.finish()?;

        let batch = begin_emote_batch()?;
        prepare_emote_poses(&references);
        let first = render(&players[0])?;
        let second = render(&players[1])?;
        if mesh::batch_submissions_enabled() {
            assert_eq!(submission.pending_poses(), 2);
        }
        let second = tint_emote_surface(&second, 0xff80ff40).context("batched tint")?;
        assert_eq!(submission.pending_poses(), 0);
        assert_eq!(first.rgba(), expected[0].1.as_slice());
        assert_eq!(second.rgba(), tinted_expected.as_slice());
        batch.finish()?;
        RASTER.with_borrow_mut(Option::take);
        Ok(())
    }

    #[test]
    #[ignore = "requires hardware DX12 and the local PSZ fixture"]
    fn gpu_indexed_mesh_mask_fold_and_uv_matches_cpu() -> Result<()> {
        use cs2_runtime::emote::CpuMeshGrid;
        let root = PathBuf::from(std::env::var_os("CS2_EMOTE_FIXTURE_ROOT").context("model directory")?);
        let index = ResourceIndex::scan_with_name_seed(&root, Some(0x0F6BB0AE))?;
        let bytes = index.read_archive_with_name_seed("psz.int", b"ca01_l_body_1.psz", 0x0F6BB0AE)?;
        let resource = EmoteResource::decode(&bytes)?;
        let mut model = CpuMotionModel::load(&resource)?;
        model.textures.clear();
        model.textures.insert("mask".into(), CpuTexture::from_rgba(4, 4, [255u8; 4].repeat(16))?);
        let color: Vec<_> = (0..16).flat_map(|i| [i * 11, 255 - i * 9, i * 7, [0, 64, 65, 255][usize::from(i % 4)]]).collect();
        model.textures.insert("color".into(), CpuTexture::from_rgba(4, 4, color)?);
        let model = Arc::new(model);
        let viewport = CpuViewport { width: 16, height: 16, origin_x: 0., origin_y: 0., scale: 1., time_units_per_frame: 1. };
        let rect = vec![[1., 1., 0., 0.], [14., 1., 4., 0.], [1., 14., 0., 4.], [14., 14., 4., 4.]];
        let folded = vec![[2., 2., 0., 0.], [12., 2., 4., 0.], [2., 12., 0., 4.], [2., 2., 0., 0.]];
        let mask = Stencil { serial: 1, clear: 0 };
        let specs = [
            ("color", vec![[1., 1., 0., 0.], [5., 1., 4., 0.], [1., 5., 0., 4.], [5., 5., 4., 4.]], 0.4, Stencil::default(), 0, None),
            ("color", vec![[8., 1., 0., 0.], [12., 1., 4., 0.], [8., 5., 0., 4.], [12., 5., 4., 4.]], 0.4, Stencil::default(), 0, None),
            ("color", rect.clone(), 0.45, Stencil::default(), 0, None),
            ("mask", rect.clone(), 1., mask, 0, Some(CpuStencilWrite::IncrementWrapping)),
            ("color", folded, 0.7, mask, 1, None),
            ("mask", rect.clone(), 1., mask, 1, Some(CpuStencilWrite::DecrementWrapping)),
            ("color", rect, 0.1, mask, 0, None),
        ];
        let build_mesh = |source: &'static str, vertices: Vec<[f64; 4]>, opacity| {
            EmoteSpriteMesh::from_grid(source, &model.textures[source], opacity, vertices,
                CpuMeshGrid { columns: 1, rows: 1 })
        };
        let draws = || specs.iter().map(|(source, vertices, opacity, stencil, reference, operation)| Ok(Draw {
            mesh: build_mesh(source, vertices.clone(), *opacity)?, stencil: *stencil,
            reference: *reference, operation: *operation, bounds: [0; 4], offset: 0, words: 0,
        })).collect::<cs2_runtime::emote::Result<Vec<_>>>();
        assert!(mesh::compatible(&draws()?).is_ok());
        let rows: Vec<_> = (0..16).map(|_| (0..16usize).collect()).collect();
        let mut scratch = EmoteRepairScratch::default();
        repair_emote_raster(viewport, &rows, [0, 0], 16, &mut scratch, |target| {
            let mut stencil = target.new_stencil(viewport, 0)?;
            for (source, vertices, opacity, mask, reference, operation) in &specs {
                let pass = if mask.serial == 0 { EmoteSpritePass::Color(None) }
                else if let Some(operation) = operation {
                    EmoteSpritePass::Mask { buffer: &mut stencil, reference: *reference, operation: *operation }
                } else { EmoteSpritePass::Color(Some((&stencil, *reference))) };
                target.draw_sprite(viewport, build_mesh(source, vertices.clone(), *opacity)?, pass)?;
            }
            Ok(())
        })?;
        let mut expected = vec![0u8; 16 * 16 * 4];
        for pair in scratch.corrections().chunks_exact(2) {
            expected[pair[0] as usize * 4..][..4].copy_from_slice(&pair[1].to_le_bytes());
        }
        let mut raster = Raster::new()?;
        raster.ensure_emote()?;
        assert!(raster.emote.as_ref().unwrap().mesh.is_some(), "hardware pipeline must initialize");
        for (pose, wrap) in [false, false, true, false].into_iter().enumerate() {
            if wrap {
                let mesh = raster.emote.as_mut().unwrap().mesh.as_mut().unwrap();
                // A previously larger coverage target must retain integer
                // sample locations when the next pose draws into its subrect.
                let limit = raster.device.limits().max_texture_dimension_2d;
                assert!(mesh.ensure_coverage(&raster.device, 17, 13, limit));
                assert!(!mesh.ensure_coverage(&raster.device, 13, 13, limit));
                mesh.coverage_stamp = 16_777_215;
            }
            let frame = raster.emote.as_mut().unwrap().render_indexed(&raster.device, &raster.queue,
                &model, viewport, draws()?, &raster.failed, None, None, None)?.into_viewport([16, 16])?;
            let uploaded = raster.emote.as_ref().unwrap().mesh.as_ref().unwrap().geometry_upload_bytes;
            let [runs, batches, bindings] = raster.emote.as_ref().unwrap().mesh.as_ref().unwrap().encoding_counts;
            assert!(batches < runs, "disjoint initial draws must share their actual coverage/compute passes");
            assert_eq!(bindings, 2, "each unique atlas has one binding regardless of masks, alpha, or ordered runs");
            if pose == 0 { assert!(uploaded > 0, "the first pose uploads actual indexed geometry"); }
            else { assert_eq!(uploaded, 0, "unchanged cached geometry remains on the GPU, including generation wrap"); }
            let max = frame.rgba().iter().zip(&expected).map(|(a, b)| a.abs_diff(*b)).max().unwrap();
            assert!(max <= 2, "hardware integer-sample coverage/mask/overlap diverged: max={max} wrap={wrap}");
        }
        Ok(())
    }

    #[test]
    #[ignore = "requires hardware DX12 and the local PSZ fixture"]
    fn gpu_mesh_preview_error_profile_for_one_to_four_poses() -> Result<()> {
        let root = PathBuf::from(std::env::var_os("CS2_EMOTE_FIXTURE_ROOT").context("model directory")?);
        let index = ResourceIndex::scan_with_name_seed(&root, Some(0x0F6BB0AE))?;
        let bytes = index.read_archive_with_name_seed("psz.int", b"ca01_l_body_1.psz", 0x0F6BB0AE)?;
        let resource = EmoteResource::decode(&bytes)?;
        let template = CpuPlayer::new(CpuMotionModel::load(&resource)?);
        let mut players = vec![template; 4];
        let viewport = CpuViewport { width: 1280, height: 720, origin_x: 640., origin_y: 600., scale: 1., time_units_per_frame: 1. };
        let mut raster = Raster::new()?;
        raster.ensure_emote()?;
        while raster.emote.as_mut().unwrap().stage_atlas(&raster.device, &raster.queue, &players[0].model)? != 0 {
            super::super::submit_gpu_commands(&raster.queue, []);
        }
        for count in 1..=4 {
            let mut prior_hashes = vec![None; count];
            let mut changed = vec![false; count];
            for tick in 0..3 {
                let mut frames = Vec::new();
                let mut cpu_us = 0;
                for (i, player) in players[..count].iter_mut().enumerate() {
                    player.set_variable("head_UD", [0., 20., -20., 10.][(i + tick) % 4])?;
                    player.advance_frames(1.)?;
                    player.update_reference_timelines()?;
                    let start = Instant::now();
                    frames.push(player.render_preview(viewport)?.frame);
                    cpu_us += start.elapsed().as_micros();
                    let mut hasher = DefaultHasher::new();
                    frames.last().unwrap().rgba().hash(&mut hasher);
                    let hash = hasher.finish();
                    changed[i] |= prior_hashes[i].is_some_and(|prior| prior != hash);
                    prior_hashes[i] = Some(hash);
                    println!("mesh_preview_input poses={count} tick={tick} instance={i} head_ud={} cpu_rgba_hash={hash:016x}", [0., 20., -20., 10.][(i + tick) % 4]);
                }
                raster.emote.as_mut().unwrap().begin_atlas_batch();
                let prepare_started = Instant::now();
                prepare_indexed_poses(&players[..count].iter().map(|player| (player, viewport)).collect::<Vec<_>>());
                let parallel_prepare_us = prepare_started.elapsed().as_micros();
                let mut encode_us = 0;
                for (i, player) in players[..count].iter().enumerate() {
                    let vars = player.effective_variables()?;
                    BATCHING.with(|batch| batch.set(true));
                    let start = Instant::now();
                    let rendered = raster.render_emote(player, viewport, &vars, true);
                    encode_us += start.elapsed().as_micros();
                    BATCHING.with(|batch| batch.set(false));
                    let rendered = rendered?;
                    let rgba = rendered.into_viewport([1280, 720])?;
                    let mut above = [0u64; 3];
                    let mut max = 0;
                    let mut alpha_differences = 0u64;
                    let mut transparent_rgb = 0u64;
                    let mut squared = 0u64;
                    for (j, (&a, &b)) in frames[i].rgba().iter().zip(rgba.rgba()).enumerate() {
                        let delta = a.abs_diff(b);
                        max = max.max(delta);
                        squared += u64::from(delta) * u64::from(delta);
                        for (n, threshold) in [1, 3, 8].into_iter().enumerate() { above[n] += u64::from(delta > threshold); }
                        if j % 4 == 3 { alpha_differences += u64::from(delta != 0); }
                        else if frames[i].rgba()[j / 4 * 4 + 3] == 0 && delta != 0 { transparent_rgb += 1; }
                    }
                    let psnr = if squared == 0 { f64::INFINITY } else {
                        10. * (255. * 255. * (1280 * 720 * 4) as f64 / squared as f64).log10()
                    };
                    println!("mesh_preview poses={count} tick={tick} instance={i} channels_gt1={} channels_gt3={} channels_gt8={} max={max} alpha_differences={alpha_differences} transparent_rgb_differences={transparent_rgb} psnr={psnr:.3}", above[0], above[1], above[2]);
                    if count == 4 && tick == 2
                        && let Some(output) = std::env::var_os("CS2_GPU_QA_OUTPUT")
                    {
                        let output = PathBuf::from(output);
                        std::fs::create_dir_all(&output)?;
                        image::save_buffer(output.join(format!("mesh-preview-instance{i}-cpu.png")), frames[i].rgba(), 1280, 720, image::ColorType::Rgba8)?;
                        image::save_buffer(output.join(format!("mesh-preview-instance{i}-gpu.png")), rgba.rgba(), 1280, 720, image::ColorType::Rgba8)?;
                        let diff: Vec<_> = frames[i].rgba().chunks_exact(4).zip(rgba.rgba().chunks_exact(4)).flat_map(|(a, b)|
                            [a[0].abs_diff(b[0]).saturating_mul(16), a[1].abs_diff(b[1]).saturating_mul(16), a[2].abs_diff(b[2]).saturating_mul(16), 255]).collect();
                        image::save_buffer(output.join(format!("mesh-preview-instance{i}-difference-x16.png")), &diff, 1280, 720, image::ColorType::Rgba8)?;
                    }
                    assert!(psnr >= 40., "hardware preview error exceeds the fixture acceptance threshold");
                }
                println!("mesh_preview_wall poses={count} tick={tick} cpu_reference_us={cpu_us} gpu_evaluate_prepare_encode_us={} parallel_prepare_us={parallel_prepare_us} ordered_encode_us={encode_us} gpu_execution_time=unavailable",
                    parallel_prepare_us + encode_us);
            }
            assert!(changed.into_iter().all(|changed| changed), "every profiled instance must change actual pixels across ticks");
        }
        Ok(())
    }

    #[test]
    fn prep_worker_budget_supports_ab_settings_and_stays_bounded() {
        for requested in [1, 2, 4] {
            assert_eq!(clamp_prep_workers(requested, 8), requested);
        }
        assert_eq!(clamp_prep_workers(4, 2), 2);
        assert_eq!(clamp_prep_workers(8, 8), MAX_PREP_WORKERS);
        assert_eq!(clamp_prep_workers(0, 8), 1);
        assert_eq!(clamp_prep_workers(4, 0), 1);
    }

    #[test]
    #[ignore = "requires hardware DX12 and CS2_EMOTE_DEVICE_BLEND=1"]
    fn gpu_surface_fade_matches_padded_source_and_transparent_rgb() -> Result<()> {
        let mut raster = Raster::new()?;
        raster.test_direct_output = true;
        raster.emote = Some(EmoteRaster::new(&raster.device)?);
        let old_rgba = vec![30, 40, 50, 255, 60, 70, 80, 127, 90, 100, 110, 0];
        let crop = Arc::new(buffer(&raster.device, old_rgba.len() as u64, TARGET));
        raster.queue.write_buffer(&crop, 0, &old_rgba);
        let cpu_old = EmoteRasterFrame::new(3, 1, [-1, 1], old_rgba)?.into_viewport([4, 3])?;
        let old = raster.bind_fade_output([3, 1], [-1, 1], crop, 12, None).unwrap();
        RASTER.with_borrow_mut(|slot| *slot = Some(raster));
        let expanded = expand_emote_surface(&old, [4, 3]).context("device surface expand")?;
        assert!(expanded.device().is_some());

        for transparent in [false, true] {
            let mut new_rgba = [12, 34, 56, 0].repeat(12);
            if !transparent {
                new_rgba[4..8].copy_from_slice(&[120, 222, 31, 87]);
            }
            let device_next = RASTER.with_borrow(|slot| {
                let raster = slot.as_ref().unwrap();
                let pixels = Arc::new(buffer(&raster.device, new_rgba.len() as u64, TARGET));
                raster.queue.write_buffer(&pixels, 0, &new_rgba);
                raster.bind_fade_output([4, 3], [0, 0], pixels, new_rgba.len() as u64, None)
            }).context("resident incoming frame")?;
            let (old_adjusted, new_adjusted, alpha_class) =
                prepare_emote_surfaces(&expanded, &device_next).context("device fade endpoint preparation")?;
            assert_eq!(alpha_class, None);
            assert!(old_adjusted.device().is_some() && new_adjusted.device().is_some());
            let mut expected_old = cpu_old.rgba().to_vec();
            for (prior, next) in expected_old.chunks_exact_mut(4).zip(new_rgba.chunks_exact_mut(4)) {
                if prior[3] == 0 && next[3] != 0 {
                    prior[..3].copy_from_slice(&next[..3]);
                }
                if prior[3] != 0 && next[3] == 0 {
                    next[..3].copy_from_slice(&prior[..3]);
                }
            }
            for weight in [1, 128, 255] {
                let mixed = mix_emote_surfaces(&old_adjusted, &new_adjusted, weight, alpha_class)
                    .context("device fade mix")?;
                let expected: Vec<_> = expected_old.iter().zip(&new_rgba).enumerate()
                    .map(|(i, (&old, &new))| {
                        if transparent && i % 4 != 3 {
                            old
                        } else {
                            ((u32::from(old) * (256 - weight) + u32::from(new) * weight) >> 8) as u8
                        }
                    }).collect();
                assert_eq!(mixed.rgba(), expected);
            }
            assert_eq!(old_adjusted.rgba(), expected_old);
            assert_eq!(new_adjusted.rgba(), new_rgba);
        }
        assert_eq!(expanded.rgba(), cpu_old.rgba());
        RASTER.with_borrow_mut(Option::take);
        Ok(())
    }

    #[test]
    #[ignore = "requires the local PSZ fixture and a hardware DX12 device"]
    fn gpu_emote_matches_local_model() -> Result<()> {
        let root = PathBuf::from(std::env::var_os("CS2_EMOTE_FIXTURE_ROOT").context("model directory")?);
        let index = ResourceIndex::scan_with_name_seed(&root, Some(0x0F6BB0AE))?;
        let bytes = index.read_archive_with_name_seed(
            "psz.int", b"ca01_l_body_1.psz", 0x0F6BB0AE,
        )?;
        let resource = EmoteResource::decode(&bytes)?;
        let mut player = CpuPlayer::new(CpuMotionModel::load(&resource)?);
        let mut raster = Raster::new()?;
        raster.ensure_emote()?;
        let atlas = player.model.textures.values().next().context("fixture atlas")?;
        assert!(atlas.is_bgra());
        let mut staged = 0;
        loop {
            let bytes = raster.emote.as_mut().unwrap().stage_atlas(&raster.device, &raster.queue, &player.model)?;
            if bytes == 0 { break; }
            assert!(bytes <= ATLAS_STAGE_BYTES);
            staged += bytes;
            super::super::submit_gpu_commands(&raster.queue, []);
        }
        assert_eq!(staged, atlas.upload_pixels().0.len());
        let viewport = CpuViewport {
            width: 1280, height: 720, origin_x: 640.0, origin_y: 600.0,
            scale: 1.0, time_units_per_frame: 1.0,
        };
        let mut failures = 0;
        for tick in 0..18 {
            let head = [0.0, 20.0, -20.0][tick % 3];
            player.set_variable("head_UD", head)?;
            player.set_variable("move_UD", head / 2.0)?;
            player.advance_frames(1.0)?;
            player.update_reference_timelines()?;
            let start = Instant::now();
            let cpu = player.render_preview(viewport)?;
            let cpu_us = start.elapsed().as_micros();
            let vars = player.effective_variables()?;
            let start = Instant::now();
            let gpu = raster.render_emote(&player, viewport, &vars, true)?;
            let gpu_us = start.elapsed().as_micros();
            let width = gpu.width() as usize;
            let height = gpu.height();
            let origin = gpu.origin().map(|n| n as usize);
            let rgba = gpu.rgba();
            let [mut different, mut interior, mut max, mut interior_max, mut squared] = [0u64; 5];
            for (i, a) in cpu.frame.rgba().chunks_exact(4).enumerate() {
                let y = i / 1280;
                let x = i % 1280;
                let b: &[u8] = if x >= origin[0] && y >= origin[1]
                    && x < origin[0] + width && y < origin[1] + height as usize
                {
                    let offset = ((y - origin[1]) * width + x - origin[0]) * 4;
                    &rgba[offset..offset + 4]
                } else {
                    &[0; 4]
                };
                if a == b { continue; }
                let alpha = a[3];
                let near_edge = (y.saturating_sub(1)..=(y+1).min(719)).any(|py|
                    (x.saturating_sub(1)..=(x+1).min(1279)).any(|px|
                        (cpu.frame.rgba()[(py * 1280 + px) * 4 + 3] == 0) != (alpha == 0)));
                different += 1;
                interior += u64::from(!near_edge);
                for (&a, &b) in a.iter().zip(b) {
                    let delta = u64::from(a.abs_diff(b));
                    squared += delta * delta;
                    max = max.max(delta);
                    if !near_edge { interior_max = interior_max.max(delta); }
                }
            }
            let psnr = if squared == 0 { f64::INFINITY } else {
                10.0 * (255.0 * 255.0 * cpu.frame.rgba().len() as f64 / squared as f64).log10()
            };
            println!("tick={tick} layers={} triangles={} warnings={} cpu_us={cpu_us} gpu_us={gpu_us} gpu_width={width} gpu_height={} different={different} interior={interior} max={max} interior_max={interior_max} psnr={psnr:.2}",
                cpu.layers, cpu.triangles, cpu.warnings.len(), height);
            if different as f64 > 1280.0 * 720.0 * 0.003
                || interior * 100 > different || interior_max > 2 || psnr < 40.0 {
                failures += 1;
            }
            let modes: &[BlendMode] = if tick == 0 {
                &[BlendMode::SourceOver, BlendMode::AddSource, BlendMode::AddDestination,
                    BlendMode::Multiply, BlendMode::InvertDestination, BlendMode::AddMultiply,
                    BlendMode::ListAlphaFromRed, BlendMode::ListAlphaFromGreen,
                    BlendMode::ListAlphaFromBlue, BlendMode::ListAlphaFromAlpha]
            } else {
                &[BlendMode::SourceOver]
            };
            let background: Vec<u8> = (0..1280 * 720).flat_map(|index| {
                let value = index as u8;
                [value, value.wrapping_add(31), value.wrapping_add(79), match index % 5 { 0 => 0, 1 => 255, _ => 173 }]
            }).collect();
            for &blend in modes {
                let mut expected = Canvas::from_rgba8(1280, 720, background.clone())?;
                let mut actual = expected.clone();
                crate::emote_native::submit_rgba(&mut expected, &cpu.frame, blend)
                    .map_err(|error| anyhow::anyhow!(error))?;
                raster.test_direct_output = true;
                let pose = gpu.device().unwrap().as_any().downcast_ref::<DevicePose>().unwrap();
                anyhow::ensure!(raster.blend_emote(&mut actual, &gpu, pose, blend)?);
                anyhow::ensure!(actual.pixels() == expected.pixels(), "device blend differs for {blend:?} at tick {tick}");
            }
        }
        anyhow::ensure!(failures == 0, "{failures} poses outside pixel tolerance");
        Ok(())
    }
    #[test]
    fn fast_forward_twenty_output_pool_cycles() {
        let mut before: Vec<(u64, Arc<u64>)> = Vec::new();
        let mut after = OutputPool::new();
        let (mut misses_before, mut misses_after) = (0, 0);
        let (mut prior_before, mut prior_after) = (Vec::new(), Vec::new());
        for round in 0..20 {
            assert_eq!(prior_before.len(), usize::from(round != 0) * 4);
            let mut next_before = Vec::new();
            let mut next_after = Vec::new();
            // Two poses and two tinted outputs. A second frame is still
            // owned while the next one renders and its bounds change.
            for output in 0..4 {
                let bytes = (1_386_000 + round * 960 + output * 128) as u64;
                let old = before.iter().find(|(capacity, value)| *capacity == bytes && Arc::strong_count(value) == 1);
                next_before.push(if let Some((_, value)) = old { value.clone() } else {
                    misses_before += 1;
                    if before.len() == 5 { before.remove(0); }
                    let value = Arc::new(bytes);
                    before.push((bytes, value.clone()));
                    value
                });
                let (value, hit) = after.acquire(bytes, OUTPUT_POOL_BYTES, |capacity| capacity);
                misses_after += usize::from(!hit);
                assert!(*value >= bytes);
                assert!(prior_after.iter().chain(&next_after).all(|other| !Arc::ptr_eq(other, &value)));
                next_after.push(value);
            }
            prior_before = next_before;
            prior_after = next_after;
        }
        println!("fast_forward cycles=20 output_allocations_before={misses_before} output_allocations_after={misses_after} output_pool_bytes={}", after.bytes);
        assert_eq!(misses_before, 80);
        assert_eq!(misses_after, 8);
    }
}

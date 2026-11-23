//! E-mote pose raster on the shared device. CPU scanlines retain the original
//! f64 coverage rule. The compute passes sample, test/write stencil, and
//! blend the straight RGBA results in the original primitive order.

use super::{DOWNLOAD, RASTER, Raster, TARGET, UPLOAD, blend_code, buffer, device_frame, report};
use crate::perf_detail::{ReadbackKind, ReadbackTimer, emote_gpu_pose, emote_gpu_start, emote_gpu_sync};
use anyhow::{Context, Result, bail};
use cs2_renderer::{BlendMode, Canvas};
use cs2_runtime::emote::{
    CpuMotionModel, CpuPlayer, CpuStencilBuffer, CpuStencilWrite, CpuTexture, CpuViewport, EmoteDrawTarget,
    EmoteDeviceReadback, EmoteError, EmoteRasterFrame, EmoteRepairScratch, EmoteSpriteMesh, EmoteSpritePass,
    EmoteTextureSpan, OwnedEmoteSpriteMesh, repair_emote_raster, visit_emote_texture_spans,
};
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::num::NonZeroU64;
use std::sync::{Arc, Mutex, OnceLock, Weak, atomic::{AtomicBool, Ordering}, mpsc};
use std::time::{Duration, Instant};

thread_local! {
    static BATCHING: Cell<bool> = const { Cell::new(false) };
    static PREPARED: RefCell<Vec<PreparedPose>> = const { RefCell::new(Vec::new()) };
    static POSE_BATCHES: RefCell<Vec<CommandBatch>> = const { RefCell::new(Vec::new()) };
}

fn batching() -> bool {
    BATCHING.with(Cell::get)
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
        match emote.stage_atlas(&raster.device, &raster.queue, model) {
            Ok(0) => {}
            Ok(bytes) => {
                raster.queue.submit([]);
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
        let enabled = std::env::var("CS2_EMOTE_DEVICE_ENDPOINT").is_ok_and(|value| value == "1");
        #[cfg(target_os = "android")]
        let enabled = super::android_property(c"debug.cs2.emote_device_endpoint", |value| value == b"1")
            .unwrap_or(false);
        enabled
    });
    if !enabled || !super::emote_device_blend_requested() { return None; }
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
    queue.submit(Some(encoder.finish()));
}

impl Raster {
    fn fade_device_pose<'a>(&self, frame: &'a EmoteRasterFrame) -> Option<&'a DevicePose> {
        if !self.direct_output_enabled() || self.failed.load(Ordering::Relaxed) { return None; }
        let pose = frame.device()?.as_any().downcast_ref::<DevicePose>()?;
        Arc::ptr_eq(&self.failed, &pose.failed).then_some(pose)
    }

    fn bind_fade_output(
        &self, size: [u32; 2], origin: [i32; 2], buffer: Arc<wgpu::Buffer>, bytes: u64,
        fade_alpha: Option<Arc<wgpu::Buffer>>,
    ) -> Option<EmoteRasterFrame> {
        EmoteRasterFrame::from_device(size[0], size[1], origin, Arc::new(DevicePose {
            buffer, device: self.device.clone(), queue: self.queue.clone(), bytes,
            failed: self.failed.clone(), fade_alpha,
        })).ok()
    }
}

const ATLAS_STAGE_LIMIT: usize = 128 * 1024 * 1024;
#[cfg(windows)]
const ATLAS_DRAW_LIMIT: usize = 192 * 1024 * 1024;
#[cfg(target_os = "android")]
const ATLAS_DRAW_LIMIT: usize = ATLAS_STAGE_LIMIT;
const ATLAS_STAGE_BYTES: usize = 2 * 1024 * 1024;
const COMMAND_BYTES: usize = 16 * 1024 * 1024;
const COMMAND_WORDS: usize = COMMAND_BYTES / size_of::<u32>();
const MAX_DRAWS: usize = 1024;
const MAX_PREP_WORKERS: usize = 4;
const SPAN_CACHE_SLOTS: usize = 128;
const SPAN_CACHE_ENTRY_BYTES: usize = 32 * 1024;

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
    commands[offset..offset + 12].copy_from_slice(&[
        bounds[2] - bounds[0], bounds[3] - bounds[1], bounds[0], bounds[1],
        0, 0, 0, draw.stencil.serial, u32::from(draw.stencil.clear), u32::from(draw.reference),
        (match draw.operation {
            None => 0,
            Some(CpuStencilWrite::IncrementWrapping) => 1,
            Some(CpuStencilWrite::DecrementWrapping) => 255,
        }) | (u32::from(draw.mesh.texture.is_bgra()) << 8),
        (draw.mesh.opacity as f32).to_bits(),
    ]);
    draw.bounds = bounds;
    draw.offset = offset;
    draw.words = words;
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
            .map_or((1, "default"), |workers| (workers, "CS2_EMOTE_PREP_WORKERS"));
        #[cfg(target_os = "android")]
        let (requested, source) = super::android_property(c"debug.cs2.emote_prep_workers", |value| {
            std::str::from_utf8(value).ok()?.parse::<usize>().ok()
        }).flatten().map_or((1, "default"), |workers| (workers, "debug.cs2.emote_prep_workers"));
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

impl CachedAtlas {
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

struct OwnedDraw {
    mesh: OwnedEmoteSpriteMesh,
    stencil: Stencil,
    reference: u8,
    operation: Option<CpuStencilWrite>,
    bounds: [u32; 4],
    offset: usize,
    words: usize,
}

struct PreparedPose {
    player: usize,
    variables: BTreeMap<String, f64>,
    viewport: CpuViewport,
    draws: Vec<OwnedDraw>,
    words: Vec<u32>,
    phases: [Duration; 2],
}

pub(in crate) fn prepare_emote_poses(players: &[(&CpuPlayer, CpuViewport)]) {
    RASTER.with_borrow_mut(|slot| {
        if let Some(emote) = slot.as_mut().and_then(|raster| raster.emote.as_mut()) {
            emote.atlas_batch_models.extend(
                players.iter().map(|(player, _)| Arc::downgrade(player.shared_motion_model())),
            );
        }
    });
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
    if workers < 2 || players.len() > 4 || matches!(mode, PrepMode::Serial | PrepMode::WithinPose) {
        return;
    }
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
                        player: player as *const CpuPlayer as usize, variables, viewport,
                        draws: own_draws(draws), words, phases,
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
                player: player as *const CpuPlayer as usize, variables, viewport,
                draws: own_draws(pose.draws), words: std::mem::take(&mut batch.commands), phases: batch.phases,
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
    let mut pose = Pose { model: player.shared_motion_model(), draws: Vec::new(), serial: 0, geometry_bytes: 0 };
    let diagnostics = player.draw_primitives_resolved(viewport, variables, &mut pose)?;
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
            let prior = pose.viewport;
            pose.player == player as *const CpuPlayer as usize
                && [prior.width, prior.height] == [viewport.width, viewport.height]
                && [prior.origin_x, prior.origin_y, prior.scale].map(f64::to_bits)
                    == [viewport.origin_x, viewport.origin_y, viewport.scale].map(f64::to_bits)
                && pose.variables.len() == variables.len()
                && pose.variables.iter().zip(variables).all(|((name, value), (other, sample))|
                    name == other && value.to_bits() == sample.to_bits())
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
}

impl EmoteDeviceReadback for DevicePose {
    fn read_rgba(&self, output: &mut [u8]) -> bool {
        if self.failed.load(Ordering::Relaxed) || output.len() as u64 != self.bytes { return false; }
        let measurement = ReadbackTimer::start(ReadbackKind::Emote, self.bytes);
        let readback = buffer(&self.device, self.bytes, DOWNLOAD);
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_buffer_to_buffer(&self.buffer, 0, &readback, 0, self.bytes);
        let submitted = self.queue.submit(Some(encoder.finish()));
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

pub(super) struct EmoteRaster {
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
    fade_params: wgpu::Buffer,
    fade_empty_alpha: wgpu::Buffer,
    fade_sources: VecDeque<(EmoteRasterFrame, wgpu::Buffer)>,
    fade_outputs: Vec<Arc<wgpu::Buffer>>,
    atlases: VecDeque<CachedAtlas>,
    atlas_bytes: usize,
    atlas_batch_models: Vec<Weak<CpuMotionModel>>,
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
    pub(super) fn new(device: &wgpu::Device) -> Result<Self> {
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
        let source = |size, usage| buffer(device, size, usage);
        Ok(Self {
            sprite_layout, pack_layout: pack.get_bind_group_layout(0),
            repair_layout: repair.get_bind_group_layout(0), sprite, pack, preview_pipelines, repair,
            fade_layout: fade.get_bind_group_layout(0), fade,
            fade_expand_layout: fade_expand.get_bind_group_layout(0), fade_expand,
            fade_prepare_layout: fade_prepare.get_bind_group_layout(0), fade_prepare,
            fade_alpha_layout: fade_alpha.get_bind_group_layout(0), fade_alpha,
            fade_params: source(32, wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST),
            fade_empty_alpha: buffer(device, 4, TARGET),
            fade_sources: VecDeque::new(), fade_outputs: Vec::new(),
            atlases: VecDeque::new(), atlas_bytes: 0, atlas_batch_models: Vec::new(),
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
            return Arc::ptr_eq(failed, &pose.failed).then(|| (*pose.buffer).clone());
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
        if let Some(output) = self.fade_outputs.iter().find(|output|
            Arc::strong_count(output) == 1 && output.size() == bytes)
        {
            return output.clone();
        }
        let output = Arc::new(buffer(device, bytes, TARGET));
        if self.fade_outputs.len() == 5 { self.fade_outputs.remove(0); }
        self.fade_outputs.push(output.clone());
        output
    }

    fn atlas(
        &mut self, device: &wgpu::Device, queue: &wgpu::Queue,
        owner: &Arc<CpuMotionModel>, source: &CpuTexture,
    ) -> Result<wgpu::TextureView> {
        let atlas = self.atlas_storage(device, owner, source, false)?
            .context("E-mote atlas allocation was declined")?;
        let extent = atlas.texture.size();
        if atlas.uploaded_rows < extent.height {
            let started = Instant::now();
            let pixels = source.upload_pixels().0;
            let bytes = atlas.upload(queue, pixels, pixels.len());
            report(&format!("emote_atlas_demand_bytes={bytes} write_us={}", started.elapsed().as_micros()));
        }
        Ok(atlas.view.clone())
    }

    fn stage_atlas(&mut self, device: &wgpu::Device, queue: &wgpu::Queue,
        model: &Arc<CpuMotionModel>,
    ) -> Result<usize> {
        for texture in model.textures.values() {
            let Some(atlas) = self.atlas_storage(device, model, texture, true)? else { return Ok(0) };
            let bytes = atlas.upload(queue, texture.upload_pixels().0, ATLAS_STAGE_BYTES);
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
    ) -> Result<Option<&mut CachedAtlas>> {
        let identity = std::ptr::from_ref(source) as usize;
        let owner_id = Arc::downgrade(owner);
        if let Some(index) = self.atlases.iter().position(|entry|
            entry.identity == identity && Weak::ptr_eq(&entry.owner, &owner_id))
        {
            if preloading {
                return Ok(self.atlases.get_mut(index));
            }
            let mut entry = self.atlases.remove(index).unwrap();
            entry.used_by_draw = true;
            self.atlases.push_back(entry);
            return Ok(self.atlases.back_mut());
        }
        let limit = device.limits().max_texture_dimension_2d as usize;
        let (pixels, _) = source.upload_pixels();
        let bytes = pixels.len();
        if source.width.max(source.height) > limit || source.width.min(source.height) == 0
            || source.width * source.height * 4 != bytes || bytes > ATLAS_STAGE_LIMIT
        {
            bail!("E-mote atlas exceeds device limits");
        }
        self.atlases.retain(|entry| {
            let keep = entry.owner.strong_count() != 0;
            if !keep { self.atlas_bytes -= entry.bytes; }
            keep
        });
        // Speculation must leave resident draw textures in the cache.
        if preloading && self.atlas_bytes + bytes > ATLAS_STAGE_LIMIT {
            return Ok(None);
        }
        let mut evicted = 0;
        let mut evicted_bytes = 0;
        while self.atlas_bytes + bytes > ATLAS_DRAW_LIMIT {
            // Prefer the oldest inactive atlas. Draw order breaks ties if the active
            // models alone exceed the hard cap. Encoded commands own their views.
            let index = self.atlases.iter().position(|atlas| {
                !atlas.used_by_draw && !self.atlas_batch_models.iter()
                    .any(|owner| Weak::ptr_eq(owner, &atlas.owner))
            }).unwrap_or(0);
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
            Ok(self.atlases.front_mut())
        } else {
            self.atlases.push_back(atlas);
            Ok(self.atlases.back_mut())
        }
    }

    fn finish_pending(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) -> Result<()> {
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
                queue.submit(Some(encoder.finish()));
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
        let prepared = take_prepared_pose(player, viewport, variables);
        let (mut draws, packed) = if let Some(prepared) = prepared {
            let draws = prepared.draws.into_iter().map(|draw| Ok(Draw {
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
            queue.submit(Some(encoder.finish()));
            if let (Some(started), Some(evaluated), Some(collected)) = (started, evaluated, collected) {
                let now = Instant::now();
                emote_gpu_pose([started, evaluated, collected, now, now], phases,
                    draws.len(), batches.len(), u64::from(count), 0);
            }
            return Ok(EmoteRasterFrame::from_device(
                width, bottom - y, [x as i32, y as i32],
                Arc::new(DevicePose {
                    buffer: self.rgba.clone(), device: device.clone(), queue: queue.clone(),
                    bytes: color_bytes, failed: failed.clone(), fade_alpha: None,
                }),
            )?);
        }
        encoder.copy_buffer_to_buffer(&self.rgba, color_bytes, &self.readback, 0, flag_bytes);
        let timing = ReadbackTimer::start(ReadbackKind::Emote, flag_bytes);
        let encoded = collected.map(|_| Instant::now());
        let submission = queue.submit(Some(encoder.finish()));
        let slice = self.readback.slice(..flag_bytes);
        let (send, receive) = mpsc::sync_channel(1);
        slice.map_async(wgpu::MapMode::Read, move |result| { let _ = send.send(result); });
        if batching() {
            let frame = EmoteRasterFrame::from_device(
                width, bottom - y, [x as i32, y as i32],
                Arc::new(DevicePose {
                    buffer: self.rgba.clone(), device: device.clone(), queue: queue.clone(),
                    bytes: color_bytes, failed: failed.clone(), fade_alpha: None,
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
            queue.submit(Some(encoder.finish()));
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
                bytes: color_bytes, failed: failed.clone(), fade_alpha: None,
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
        self.queue.submit(Some(encoder.finish()));
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
            raster.queue.submit([]);
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
}

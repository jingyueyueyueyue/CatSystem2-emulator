//! E-mote pose raster on the shared device. CPU scanlines retain the original
//! f64 coverage rule. The compute passes sample, test/write stencil, and
//! blend the straight RGBA results in the original primitive order.

use super::{DOWNLOAD, RASTER, Raster, TARGET, UPLOAD, buffer, report};
use crate::perf_detail::{ReadbackKind, ReadbackTimer, emote_gpu_pose, emote_gpu_start};
use anyhow::{Context, Result, bail};
use cs2_runtime::emote::{
    CpuMotionModel, CpuPlayer, CpuStencilBuffer, CpuStencilWrite, CpuTexture, CpuViewport, EmoteDrawTarget,
    EmoteError, EmoteRasterFrame, EmoteSpriteMesh, EmoteSpritePass, EmoteTextureSpan,
    repair_emote_raster, visit_emote_texture_spans,
};
use std::collections::{BTreeMap, VecDeque};
use std::num::NonZeroU64;
use std::sync::{Arc, Mutex, OnceLock, Weak, atomic::Ordering, mpsc};
use std::time::{Duration, Instant};

/// Wgpu uses one bounded readback per pose. The manager keeps the logical
/// viewport dimensions for its held images and cross-fade surfaces.
pub(in crate) fn draw_emote(
    player: &CpuPlayer, viewport: CpuViewport, variables: &BTreeMap<String, f64>,
) -> Option<EmoteRasterFrame> {
    RASTER.with_borrow_mut(|slot| {
        let raster = slot.as_mut()?;
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

const ATLAS_BYTES: usize = 128 * 1024 * 1024;
const COMMAND_BYTES: usize = 16 * 1024 * 1024;
const COMMAND_WORDS: usize = COMMAND_BYTES / size_of::<u32>();
const MAX_DRAWS: usize = 1024;
const MAX_PREP_WORKERS: usize = 4;

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
    rows: Vec<Vec<EmoteTextureSpan>>,
    phases: [Duration; 2],
}

impl CommandBatch {
    fn prepare(
        &mut self, draws: &mut [Draw<'_>], viewport: CpuViewport, alignment: usize, profile: bool,
        word_limit: usize,
    ) -> cs2_runtime::emote::Result<()> {
        self.commands.clear();
        self.rows.resize_with(viewport.height as usize, Vec::new);
        self.phases = [Duration::ZERO; 2];
        for row in &mut self.rows { row.clear(); }
        for draw in draws {
            draw.words = 0;
            self.prepare_draw(draw, viewport, alignment, profile, word_limit)?;
        }
        Ok(())
    }

    fn prepare_draw(
        &mut self, draw: &mut Draw<'_>, viewport: CpuViewport, alignment: usize, profile: bool,
        word_limit: usize,
    ) -> cs2_runtime::emote::Result<()> {
        let Draw { mesh, stencil, reference, operation, .. } = draw;
        let mut bounds = [viewport.width, viewport.height, 0, 0];
        let mut span_count = 0;
        let budget = word_limit.saturating_sub(self.commands.len()) / 8;
        let scanlines_started = profile.then(Instant::now);
        for triangle in mesh.triangle_indices() {
            visit_emote_texture_spans(triangle.map(|i| mesh.vertices[i]), [viewport.width, viewport.height], |span| {
                span_count += 1;
                if span_count <= budget {
                    extend_bounds(&mut bounds, [span.x.start, span.y, span.x.end, span.y + 1]);
                    self.rows[span.y as usize].push(span);
                }
            })?;
            if span_count > budget {
                return Err(EmoteError::Capacity);
            }
        }
        let pack_started = scanlines_started.map(|start| {
            let now = Instant::now();
            self.phases[0] += now - start;
            now
        });
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
        block[..12].copy_from_slice(&[
            bounds[2] - bounds[0], height as u32, bounds[0], bounds[1],
            0, 0, 0, stencil.serial, u32::from(stencil.clear), u32::from(*reference),
            match operation {
                None => 0,
                Some(CpuStencilWrite::IncrementWrapping) => 1,
                Some(CpuStencilWrite::DecrementWrapping) => 255,
            },
            (mesh.opacity as f32).to_bits(),
        ]);
        let mut index = first_span;
        for (y, spans) in self.rows[bounds[1] as usize..bounds[3] as usize].iter_mut().enumerate() {
            block[12 + y] = index as u32;
            for span in spans.drain(..) {
                // Split the integer texel origin so a 4096-wide atlas
                // doesn't round away the subpixel precision before sampling.
                let base = span.uv.map(|n| n.floor());
                let uv = std::array::from_fn::<_, 2, _>(|axis| (span.uv[axis] - base[axis]) as f32);
                let step = span.step.map(|n| n as f32);
                if base.into_iter().any(|n| !n.is_finite() || n.abs() > 1e6)
                    || step.into_iter().any(|n| !n.is_finite() || n.abs() > 1e6) {
                    return Err(EmoteError::InvalidResource("E-mote UV exceeds accelerated precision"));
                }
                block[index..index + 8].copy_from_slice(&[
                    span.x.start, span.x.end, uv[0].to_bits(), uv[1].to_bits(),
                    step[0].to_bits(), step[1].to_bits(),
                    base[0] as i32 as u32, base[1] as i32 as u32,
                ]);
                index += 8;
            }
        }
        block[12 + height] = index as u32;
        draw.bounds = bounds;
        draw.offset = offset;
        draw.words = words;
        if let Some(start) = pack_started {
            self.phases[1] += start.elapsed();
        }
        Ok(())
    }
}

fn prep_worker_limit() -> usize {
    static LIMIT: OnceLock<usize> = OnceLock::new();
    *LIMIT.get_or_init(|| {
        let available = std::thread::available_parallelism().map_or(1, |count| count.get());
        #[cfg(windows)]
        let limit = std::env::var("CS2_EMOTE_PREP_WORKERS").ok()
            .and_then(|value| value.parse::<usize>().ok()).unwrap_or(4);
        #[cfg(target_os = "android")]
        let limit = super::android_property(c"debug.cs2.emote_prep_workers", |value| {
            std::str::from_utf8(value).ok()?.parse::<usize>().ok()
        }).flatten().unwrap_or(2);
        limit.clamp(1, available.min(MAX_PREP_WORKERS))
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
    let desired = if draws.len() >= 12 && u64::from(viewport.width) * u64::from(viewport.height) >= 256 * 1024 {
        prep_worker_limit()
    } else {
        1
    };
    if desired == 1 {
        let batch = &mut batches[0];
        batch.prepare(draws, viewport, alignment, profile, COMMAND_WORDS)?;
        std::mem::swap(commands, &mut batch.commands);
        return Ok(batch.phases);
    }
    let chunk_size = draws.len().div_ceil(desired);
    let count = draws.len().div_ceil(chunk_size);
    let word_limit = (COMMAND_WORDS / count / alignment) * alignment;
    let active = &mut batches[..count];
    let jobs = Mutex::new(active.iter_mut().zip(draws.chunks_mut(chunk_size)));
    let prepare = || -> cs2_runtime::emote::Result<()> {
        loop {
            let next = jobs.lock().unwrap().next();
            let Some((batch, draws)) = next else { return Ok(()) };
            batch.prepare(draws, viewport, alignment, profile, word_limit)?;
        }
    };
    let result = std::thread::scope(|scope| {
        let mut workers = Vec::with_capacity(count - 1);
        for _ in 1..count {
            if let Ok(worker) = std::thread::Builder::new().spawn_scoped(scope, &prepare) {
                workers.push(worker);
            }
        }
        let mut result = prepare();
        for worker in workers {
            let worker_result = worker.join().unwrap();
            result = result.and(worker_result);
        }
        result
    });
    if matches!(result, Err(EmoteError::Capacity)) {
        let batch = &mut batches[0];
        batch.prepare(draws, viewport, alignment, profile, COMMAND_WORDS)?;
        std::mem::swap(commands, &mut batch.commands);
        return Ok(batch.phases);
    }
    result?;
    let mut phases = [Duration::ZERO; 2];
    // The order of the GPU dispatches also determines stencil semantics.
    for (batch, chunk) in active.iter().zip(draws.chunks_mut(chunk_size)) {
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
}

pub(super) struct EmoteRaster {
    sprite: wgpu::ComputePipeline,
    sprite_layout: wgpu::BindGroupLayout,
    pack: wgpu::ComputePipeline,
    pack_layout: wgpu::BindGroupLayout,
    atlases: VecDeque<CachedAtlas>,
    atlas_bytes: usize,
    commands: wgpu::Buffer,
    pixels: wgpu::Buffer,
    rgba: wgpu::Buffer,
    size: wgpu::Buffer,
    readback: wgpu::Buffer,
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
        let pipeline = |entry| device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some(entry), layout: None, module: &module, entry_point: Some(entry),
            compilation_options: Default::default(), cache: None,
        });
        let sprite = pipeline("sprite");
        let pack = pipeline("pack");
        if let Some(error) = pollster::block_on(device.pop_error_scope()) {
            bail!("compile E-mote shader: {error}");
        }
        let source = |size, usage| buffer(device, size, usage);
        Ok(Self {
            sprite_layout: sprite.get_bind_group_layout(0), pack_layout: pack.get_bind_group_layout(0),
            sprite, pack,
            atlases: VecDeque::new(), atlas_bytes: 0,
            commands: source(4, UPLOAD), pixels: source(8, UPLOAD), rgba: source(4, TARGET),
            size: source(16, wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST),
            readback: source(4, DOWNLOAD), batches: Default::default(), words: Vec::new(),
        })
    }

    fn atlas(
        &mut self, device: &wgpu::Device, queue: &wgpu::Queue,
        owner: &Arc<CpuMotionModel>, source: &CpuTexture,
    ) -> Result<wgpu::TextureView> {
        let identity = std::ptr::from_ref(source) as usize;
        let owner_id = Arc::downgrade(owner);
        if let Some(index) = self.atlases.iter().position(|entry|
            entry.identity == identity && Weak::ptr_eq(&entry.owner, &owner_id))
        {
            let entry = self.atlases.remove(index).unwrap();
            let view = entry.view.clone();
            self.atlases.push_back(entry);
            return Ok(view);
        }
        let limit = device.limits().max_texture_dimension_2d as usize;
        let bytes = source.rgba.len();
        if source.width.max(source.height) > limit || source.width.min(source.height) == 0
            || source.width * source.height * 4 != bytes || bytes > ATLAS_BYTES
        {
            bail!("E-mote atlas exceeds device limits");
        }
        self.atlases.retain(|entry| {
            let keep = entry.owner.strong_count() != 0;
            if !keep { self.atlas_bytes -= entry.bytes; }
            keep
        });
        while self.atlas_bytes + bytes > ATLAS_BYTES {
            self.atlas_bytes -= self.atlases.pop_front().unwrap().bytes;
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
        queue.write_texture(texture.as_image_copy(), &source.rgba, wgpu::TexelCopyBufferLayout {
            offset: 0, bytes_per_row: Some(size.width * 4), rows_per_image: Some(size.height),
        }, size);
        let view = texture.create_view(&Default::default());
        self.atlas_bytes += bytes;
        self.atlases.push_back(CachedAtlas { owner: owner_id, identity, bytes, view: view.clone() });
        Ok(view)
    }

    fn render(
        &mut self, device: &wgpu::Device, queue: &wgpu::Queue, player: &CpuPlayer,
        viewport: CpuViewport, variables: &BTreeMap<String, f64>, allow_warnings: bool,
    ) -> Result<EmoteRasterFrame> {
        let started = emote_gpu_start();
        let model = player.shared_motion_model();
        let mut pose = Pose { model, draws: Vec::new(), serial: 0, geometry_bytes: 0 };
        let diagnostics = player.draw_primitives_resolved(viewport, variables, &mut pose)?;
        if !allow_warnings && !diagnostics.warnings.is_empty() {
            bail!("E-mote pose has CPU evaluator warnings");
        }
        let evaluated = started.map(|_| Instant::now());
        let alignment = device.limits().min_storage_buffer_offset_alignment as usize / 4;
        let phases = prepare_commands(
            &mut pose.draws, &mut self.batches, viewport, alignment, started.is_some(), &mut self.words,
        )?;
        pose.draws.retain(|draw| draw.words != 0);
        let collected = started.map(|_| Instant::now());
        if pose.draws.is_empty() {
            return Ok(EmoteRasterFrame::new(1, 1, [0, 0], vec![0; 4])?);
        }
        let mut bounds = [viewport.width, viewport.height, 0, 0];
        for draw in &pose.draws {
            extend_bounds(&mut bounds, draw.bounds);
        }
        let [x, y, right, bottom] = bounds;
        let width = right - x;
        let count = width * (bottom - y);
        let color_bytes = u64::from(count) * 4;
        let download_bytes = color_bytes + u64::from(count.div_ceil(32)) * 4;
        let command_bytes = self.words.len() as u64 * 4;
        let limits = device.limits();
        let groups = count.div_ceil(64);
        if color_bytes * 2 > u64::from(limits.max_storage_buffer_binding_size)
            || command_bytes > limits.max_buffer_size || groups > limits.max_compute_workgroups_per_dimension
            || pose.draws.iter().any(|draw| draw.words as u64 * 4 > u64::from(limits.max_storage_buffer_binding_size))
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
        grow(&mut self.rgba, download_bytes, TARGET);
        grow(&mut self.readback, download_bytes, DOWNLOAD);
        for draw in &pose.draws {
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
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&self.sprite);
            for draw in &pose.draws {
                let atlas = self.atlas(device, queue, model, draw.mesh.texture)?;
                let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("CS2 E-mote sprite"),
                    layout: &self.sprite_layout,
                    entries: &[
                        wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                            buffer: &self.commands, offset: draw.offset as u64 * 4,
                            size: NonZeroU64::new(draw.words as u64 * 4),
                        }) },
                        wgpu::BindGroupEntry { binding: 1, resource: self.pixels.as_entire_binding() },
                        wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&atlas) },
                    ],
                });
                let [left, top, right, bottom] = draw.bounds;
                pass.set_bind_group(0, &group, &[]);
                pass.dispatch_workgroups((right - left).div_ceil(8), (bottom - top).div_ceil(8), 1);
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
            pass.set_pipeline(&self.pack);
            pass.set_bind_group(0, &group, &[]);
            pass.dispatch_workgroups(groups, 1, 1);
        }
        encoder.copy_buffer_to_buffer(&self.rgba, 0, &self.readback, 0, download_bytes);
        let timing = ReadbackTimer::start(ReadbackKind::Packed, download_bytes);
        let encoded = collected.map(|_| Instant::now());
        let submission = queue.submit(Some(encoder.finish()));
        let slice = self.readback.slice(..download_bytes);
        let (send, receive) = mpsc::sync_channel(1);
        slice.map_async(wgpu::MapMode::Read, move |result| { let _ = send.send(result); });
        let ready = device.poll(wgpu::PollType::Wait {
            submission_index: Some(submission), timeout: Some(Duration::from_millis(250)),
        });
        if let Err(error) = ready {
            self.readback.unmap();
            return Err(error).context("wait for E-mote pose");
        }
        receive.try_recv().context("receive E-mote readback")?.context("map E-mote pose")?;
        let mapped = slice.get_mapped_range();
        let (pixels, flags) = mapped.split_at(color_bytes as usize);
        let mut rgba = pixels.to_vec();
        let mut repair = vec![Vec::new(); viewport.height as usize];
        for (word, bytes) in flags.as_chunks::<4>().0.iter().enumerate() {
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
        let draw_count = pose.draws.len();
        if repair.iter().any(|row| !row.is_empty()) {
            let reference = repair_emote_raster(viewport, &repair, |target| {
                let mut stencils = BTreeMap::<u32, CpuStencilBuffer>::new();
                for draw in pose.draws {
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
            })?;
            for (row, columns) in repair.iter().enumerate().skip(y as usize).take((bottom - y) as usize) {
                for &column in columns {
                    let src = (row * viewport.width as usize + column) * 4;
                    let dst = ((row - y as usize) * width as usize + column - x as usize) * 4;
                    rgba[dst..dst + 4].copy_from_slice(&reference.rgba()[src..src + 4]);
                }
            }
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
                u64::from(count),
                repair.iter().map(Vec::len).sum(),
            );
        }
        Ok(EmoteRasterFrame::new(width, bottom - y, [x as i32, y as i32], rgba)?)
    }
}

impl Raster {
    pub(super) fn render_emote(
        &mut self, player: &CpuPlayer, viewport: CpuViewport,
        variables: &BTreeMap<String, f64>, allow_warnings: bool,
    ) -> Result<EmoteRasterFrame> {
        if self.failed.load(Ordering::Relaxed) { bail!("E-mote device unavailable"); }
        if self.emote_initialization_failed { bail!("E-mote shader initialization failed"); }
        if self.emote.is_none() {
            let renderer = EmoteRaster::new(&self.device).inspect_err(|_| {
                self.emote_initialization_failed = true;
            })?;
            self.emote = Some(renderer);
        }
        let frame = self.emote.as_mut().unwrap().render(&self.device, &self.queue, player, viewport, variables, allow_warnings)?;
        if self.failed.load(Ordering::Relaxed) { bail!("E-mote device failed during draw"); }
        Ok(frame)
    }
}


#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use cs2_runtime::{ResourceIndex, emote::EmoteResource};
    use std::{path::PathBuf, time::Instant};

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
        }
        anyhow::ensure!(failures == 0, "{failures} poses outside pixel tolerance");
        Ok(())
    }
}

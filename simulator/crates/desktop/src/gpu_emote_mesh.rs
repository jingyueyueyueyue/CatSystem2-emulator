//! Indexed hardware triangle coverage followed by ordered integer composition.
//! Folded meshes are split into ordered non-overlapping triangle runs: one UV
//! attachment cannot represent multiple samples at the same pixel at once.
use super::*;

pub(super) fn enabled() -> bool {
    #[cfg(windows)]
    { !std::env::var("CS2_EMOTE_MESH").is_ok_and(|value| value == "0") }
    #[cfg(target_os = "android")]
    { !super::super::android_property(c"debug.cs2.emote_mesh", |value| value == b"0").unwrap_or(false) }
}

fn triangle_area(v: [[f64; 4]; 3]) -> f64 {
    (v[1][0] - v[0][0]) * (v[2][1] - v[0][1])
        - (v[1][1] - v[0][1]) * (v[2][0] - v[0][0])
}

fn interiors_overlap(a: [[f64; 4]; 3], b: [[f64; 4]; 3]) -> bool {
    for axis in 0..2 {
        let range = |triangle: [[f64; 4]; 3]| {
            [triangle.iter().map(|v| v[axis]).fold(f64::INFINITY, f64::min),
             triangle.iter().map(|v| v[axis]).fold(f64::NEG_INFINITY, f64::max)]
        };
        let x = range(a); let y = range(b);
        if x[1] <= y[0] || y[1] <= x[0] { return false; }
    }
    let mut shared = [[0.; 4]; 3];
    let mut shared_count = 0;
    for vertex in a {
        if b.iter().any(|v| vertex[..2] == v[..2]) {
            shared[shared_count] = vertex;
            shared_count += 1;
        }
    }
    if shared_count == 2 {
        let edge = &shared[..2];
        let side = |v: &[f64; 4]| (edge[1][0] - edge[0][0]) * (v[1] - edge[0][1])
            - (edge[1][1] - edge[0][1]) * (v[0] - edge[0][0]);
        let p = a.iter().find(|v| !edge.iter().any(|s| v[..2] == s[..2])).map(side);
        let q = b.iter().find(|v| !edge.iter().any(|s| v[..2] == s[..2])).map(side);
        if p.zip(q).is_some_and(|(p, q)| p * q < 0.0) { return false; }
    }
    // Separating axes with equality excluded: a shared edge or vertex is legal.
    for triangle in [a, b] {
        for edge in 0..3 {
            let p = triangle[edge];
            let q = triangle[(edge + 1) % 3];
            let axis = [q[1] - p[1], p[0] - q[0]];
            let project = |v: [f64; 4]| v[0] * axis[0] + v[1] * axis[1];
            let range = |triangle: [[f64; 4]; 3]| {
                let v = triangle.map(project);
                [v.into_iter().fold(f64::INFINITY, f64::min),
                    v.into_iter().fold(f64::NEG_INFINITY, f64::max)]
            };
            let x = range(a);
            let y = range(b);
            if x[1] <= y[0] || y[1] <= x[0] { return false; }
        }
    }
    true
}

fn validate_mesh(mesh: &EmoteSpriteMesh<'_>) -> Result<()> {
    let [columns, rows] = mesh.mesh_dimensions();
    let vertices = (u64::from(columns) + 1) * (u64::from(rows) + 1);
    if vertices != mesh.vertices.len() as u64 || u64::from(columns) * u64::from(rows) > 256 {
        bail!("mesh_topology_limit");
    }
    if mesh.vertices.iter().flatten().any(|v| !v.is_finite() || v.abs() > 1_000_000.0) {
        bail!("mesh_coordinate_precision");
    }
    Ok(())
}

fn ordered_runs(triangles: &[[[f64; 4]; 3]]) -> Vec<std::ops::Range<u32>> {
    // Compute broad-phase bounds once, rather than recomputing them for each
    // pair of triangles in a subdivided mesh.
    let bounds: Vec<[f64; 4]> = triangles.iter().map(|triangle| std::array::from_fn(|i| {
        if i < 2 { triangle.iter().map(|v| v[i]).fold(f64::INFINITY, f64::min) }
        else { triangle.iter().map(|v| v[i - 2]).fold(f64::NEG_INFINITY, f64::max) }
    })).collect();
    let visible: Vec<_> = triangles.iter().map(|v| triangle_area(*v).abs() >= 1e-12).collect();
    // A conservative spatial broad phase limits exact SAT tests to nearby
    // triangles. Every bin touched by a triangle is included; order and the
    // exact overlap predicate still decide every run boundary.
    const BINS: usize = 32;
    let extent: [f64; 4] = std::array::from_fn(|axis| bounds.iter().map(|v| v[axis])
        .fold(if axis < 2 { f64::INFINITY } else { f64::NEG_INFINITY },
            if axis < 2 { f64::min } else { f64::max }));
    let axis = usize::from(extent[3] - extent[1] > extent[2] - extent[0]);
    let scale = BINS as f64 / (extent[axis + 2] - extent[axis]).max(1e-12);
    let bins_for = |bounds: [f64; 4]| {
        let bin = |v: f64| (((v - extent[axis]) * scale).floor() as usize).min(BINS - 1);
        bin(bounds[axis])..=bin(bounds[axis + 2])
    };
    let mut bins: [Vec<usize>; BINS] = std::array::from_fn(|_| Vec::new());
    let mut visited = vec![usize::MAX; triangles.len()];
    let mut runs = Vec::new();
    let mut start = 0;
    for (i, triangle) in triangles.iter().enumerate() {
        let mut overlap = false;
        if visible[i] {
            'search: for bin in bins_for(bounds[i]) {
                for &j in &bins[bin] {
                    if visited[j] == i { continue; }
                    visited[j] = i;
                    let a = bounds[j]; let b = bounds[i];
                    if a[0] < b[2] && b[0] < a[2] && a[1] < b[3] && b[1] < a[3]
                        && interiors_overlap(triangles[j], *triangle)
                    {
                        overlap = true;
                        break 'search;
                    }
                }
            }
        }
        if overlap {
            runs.push(start as u32 * 3..i as u32 * 3);
            start = i;
            for bin in &mut bins { bin.clear(); }
        }
        if visible[i] { for bin in bins_for(bounds[i]) { bins[bin].push(i); } }
    }
    if start < triangles.len() { runs.push(start as u32 * 3..triangles.len() as u32 * 3); }
    runs
}

pub(super) fn compatible(draws: &[Draw<'_>]) -> Result<()> {
    for draw in draws { validate_mesh(&draw.mesh)?; }
    Ok(())
}

struct CachedMesh {
    grid: [u32; 2],
    vertices: Box<[[u64; 4]]>,
    bounds: [f64; 4],
    base: [f64; 2],
    indices: Arc<Vec<u32>>,
    runs: Vec<std::ops::Range<u32>>,
}

struct IndexedDraw {
    bounds: [u32; 4],
    base: [f64; 2],
    ranges: Vec<std::ops::Range<u32>>,
}

pub(super) struct IndexedGeometry {
    viewport: [u32; 2],
    keys: Vec<Arc<CachedMesh>>,
    bounds: [u32; 4],
    vertices: Vec<u32>,
    indices: Arc<Vec<u32>>,
    draws: Vec<IndexedDraw>,
}

/// Only the preceding pose is retained for each player. Batch preparation has
/// its own aggregate reservation in addition to the per-pose COMMAND_BYTES cap.
#[derive(Default)]
pub(super) struct GeometryCache {
    meshes: Vec<Arc<CachedMesh>>,
    frame: Option<Arc<IndexedGeometry>>,
    hits: usize,
    misses: usize,
}

impl GeometryCache {
    pub(super) fn reservation_bytes(draws: &[Draw<'_>]) -> usize {
        draws.iter().map(|draw| {
            let [columns, rows] = draw.mesh.mesh_dimensions();
            // Evaluated/owned vertices + exact cache keys + packed GPU vertices;
            // local/combined indices and worst-case one ordered run per triangle.
            // Include spare Vec capacity and the transient Draw -> OwnedDraw
            // conversion, rather than budgeting only initialized elements.
            draw.mesh.vertices.len() * (2 * size_of::<[f64; 4]>() + 2 * size_of::<[f32; 4]>())
                + columns as usize * rows as usize * 2 * (12 * size_of::<u32>() + 3 * size_of::<std::ops::Range<u32>>())
                + 2 * size_of::<CachedMesh>() + 2 * size_of::<IndexedDraw>()
                + 2 * size_of::<OwnedDraw>() + 2 * size_of::<Draw<'_>>() + size_of::<IndexedGeometry>()
                + 8 * size_of::<Arc<CachedMesh>>() + 2 * draw.mesh.source.len()
        }).sum()
    }

    pub(super) fn counters(&self) -> [usize; 2] { [self.hits, self.misses] }

    pub(super) fn prepare(&mut self, draws: &[Draw<'_>], viewport: CpuViewport) -> Arc<IndexedGeometry> {
        let keys: Vec<_> = draws.iter().enumerate().map(|(i, draw)| {
            let mesh = &draw.mesh;
            let grid = mesh.mesh_dimensions();
            let prior = self.meshes.get(i);
            if let Some(prior) = prior
                && prior.grid == grid && prior.vertices.len() == mesh.vertices.len()
                && prior.vertices.iter().zip(&mesh.vertices).all(|(a, b)| *a == b.map(f64::to_bits))
            {
                self.hits += 1;
                return prior.clone();
            }
            self.misses += 1;
            let bounds = std::array::from_fn(|i| {
                let values = mesh.vertices.iter().map(|v| v[i % 2]);
                if i < 2 { values.fold(f64::INFINITY, f64::min) }
                else { values.fold(f64::NEG_INFINITY, f64::max) }
            });
            let base = std::array::from_fn(|axis| mesh.vertices.iter().map(|v| v[axis + 2])
                .fold(f64::INFINITY, f64::min).floor());
            let indices = prior.filter(|prior| prior.grid == grid).map(|prior| prior.indices.clone())
                .unwrap_or_else(|| Arc::new(mesh.triangle_indices().flatten().map(|v| v as u32).collect()));
            let triangles: Vec<_> = indices.chunks_exact(3).map(|ids|
                [mesh.vertices[ids[0] as usize], mesh.vertices[ids[1] as usize], mesh.vertices[ids[2] as usize]]).collect();
            Arc::new(CachedMesh {
                grid, vertices: mesh.vertices.iter().map(|v| v.map(f64::to_bits)).collect(),
                bounds, base, indices, runs: ordered_runs(&triangles),
            })
        }).collect();
        let size = [viewport.width, viewport.height];
        if let Some(frame) = &self.frame
            && frame.viewport == size && frame.keys.len() == keys.len()
            && frame.keys.iter().zip(&keys).all(|(a, b)| Arc::ptr_eq(a, b))
        {
            return frame.clone();
        }
        let mut bounds = [viewport.width, viewport.height, 0, 0];
        let mut prepared: Vec<_> = keys.iter().map(|mesh| {
            let draw_bounds = std::array::from_fn(|i|
                mesh.bounds[i].ceil().clamp(0., f64::from(size[i % 2])) as u32);
            if draw_bounds[0] < draw_bounds[2] && draw_bounds[1] < draw_bounds[3] {
                extend_bounds(&mut bounds, draw_bounds);
            }
            IndexedDraw { bounds: draw_bounds, base: mesh.base, ranges: Vec::new() }
        }).collect();
        let [x, y, right, bottom] = bounds;
        let [width, height] = [right.saturating_sub(x), bottom.saturating_sub(y)];
        let reused_indices = self.frame.as_ref().filter(|frame|
            frame.keys.len() == keys.len()
                && frame.keys.iter().zip(&keys).all(|(a, b)| Arc::ptr_eq(&a.indices, &b.indices))
                && frame.draws.iter().zip(&prepared).all(|(a, b)|
                    (a.bounds[0] < a.bounds[2] && a.bounds[1] < a.bounds[3])
                        == (b.bounds[0] < b.bounds[2] && b.bounds[1] < b.bounds[3])))
            .map(|frame| frame.indices.clone());
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        let mut index_count = 0;
        for ((draw, mesh), prepared) in draws.iter().zip(&keys).zip(&mut prepared) {
            let [left, top, right, bottom] = prepared.bounds;
            if left >= right || top >= bottom { continue; }
            let first_vertex = vertices.len() as u32 / 4;
            let first_index = index_count;
            index_count += mesh.indices.len() as u32;
            for vertex in &draw.mesh.vertices {
                vertices.extend([
                    ((vertex[0] - f64::from(x) + 0.5) / f64::from(width) * 2.0 - 1.0) as f32,
                    (1.0 - (vertex[1] - f64::from(y) + 0.5) / f64::from(height) * 2.0) as f32,
                    (vertex[2] - mesh.base[0]) as f32, (vertex[3] - mesh.base[1]) as f32,
                ].map(f32::to_bits));
            }
            if reused_indices.is_none() { indices.extend(mesh.indices.iter().map(|v| v + first_vertex)); }
            prepared.ranges = mesh.runs.iter().map(|range| range.start + first_index..range.end + first_index).collect();
        }
        let indices = reused_indices.unwrap_or_else(|| Arc::new(indices));
        self.meshes = keys.clone();
        let frame = Arc::new(IndexedGeometry { viewport: size, keys, bounds, vertices, indices, draws: prepared });
        self.frame = Some(frame.clone());
        frame
    }
}

pub(super) struct MeshRaster {
    raster: wgpu::RenderPipeline,
    composite: wgpu::ComputePipeline,
    pack: wgpu::ComputePipeline,
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    prior_indices: Vec<u32>,
    prior_vertices: Vec<u32>,
    resident_geometry: Weak<IndexedGeometry>,
    resident_indices: Weak<Vec<u32>>,
    pub(super) geometry_upload_bytes: u64,
    geometry: GeometryCache,
    coverage: Option<(wgpu::Texture, wgpu::TextureView)>,
    pub(super) coverage_stamp: u32,
    poses: u64,
}

impl MeshRaster {
    pub(super) fn new(device: &wgpu::Device) -> Result<Self> {
        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let source = format!("{}\n{}", include_str!("gpu_emote.wgsl"), include_str!("gpu_emote_mesh.wgsl"));
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("CS2 indexed E-mote"), source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let constants = &[("track_repairs", 0.0)];
        let raster = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("CS2 E-mote indexed coverage"), layout: None,
            vertex: wgpu::VertexState {
                module: &module, entry_point: Some("mesh_vertex"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: 16, step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2],
                }],
            },
            primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
            depth_stencil: None, multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module, entry_point: Some("mesh_fragment"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba32Float, blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview: None, cache: None,
        });
        let compute = |entry, constants: &[(&str, f64)]| device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some(entry), module: &module, entry_point: Some(entry), layout: None,
            compilation_options: wgpu::PipelineCompilationOptions { constants, ..Default::default() },
            cache: None,
        });
        let storage = |binding, read_only| wgpu::BindGroupLayoutEntry {
            binding, visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only },
                has_dynamic_offset: binding == 0, min_binding_size: None,
            }, count: None,
        };
        let texture = |binding, sample_type| wgpu::BindGroupLayoutEntry {
            binding, visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Texture { sample_type, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
            count: None,
        };
        let bindings = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("CS2 E-mote indexed composition"),
            entries: &[
                storage(0, true), storage(1, false),
                texture(2, wgpu::TextureSampleType::Uint),
                texture(4, wgpu::TextureSampleType::Float { filterable: false }),
            ],
        });
        let composite_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None, bind_group_layouts: &[&bindings], push_constant_ranges: &[],
        });
        let compose = |constants: &[(&str, f64)]| device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("CS2 E-mote ordered mesh composite"), module: &module,
            entry_point: Some("mesh_sprite"), layout: Some(&composite_layout),
            compilation_options: wgpu::PipelineCompilationOptions { constants, ..Default::default() }, cache: None,
        });
        let composite = compose(constants);
        let pack = compute("pack", constants);
        if let Some(error) = pollster::block_on(device.pop_error_scope()) {
            bail!("indexed_mesh_pipeline: {error}");
        }
        Ok(Self {
            raster, composite, pack,
            vertices: buffer(device, 16, wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST),
            indices: buffer(device, 4, wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST),
            prior_indices: Vec::new(), prior_vertices: Vec::new(), geometry: GeometryCache::default(),
            resident_geometry: Weak::new(), resident_indices: Weak::new(), geometry_upload_bytes: 0,
            coverage: None, coverage_stamp: 0, poses: 0,
        })
    }

    fn upload_geometry(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, geometry: &Arc<IndexedGeometry>) {
        self.geometry_upload_bytes = 0;
        if self.resident_geometry.upgrade().is_some_and(|prior| Arc::ptr_eq(&prior, geometry)) { return; }
        fn upload(device: &wgpu::Device, queue: &wgpu::Queue, buffer: &mut wgpu::Buffer,
            prior: &mut Vec<u32>, values: &[u32], usage: wgpu::BufferUsages,
        ) -> u64 {
            if prior == values { return 0; }
            let bytes = (values.len() * 4) as u64;
            if buffer.size() < bytes { *buffer = super::buffer(device, bytes, usage | wgpu::BufferUsages::COPY_DST); }
            let data = unsafe { std::slice::from_raw_parts(values.as_ptr().cast::<u8>(), values.len() * 4) };
            queue.write_buffer(buffer, 0, data);
            prior.clear(); prior.extend_from_slice(values);
            bytes
        }
        self.geometry_upload_bytes += upload(device, queue, &mut self.vertices, &mut self.prior_vertices, &geometry.vertices, wgpu::BufferUsages::VERTEX);
        if !self.resident_indices.upgrade().is_some_and(|prior| Arc::ptr_eq(&prior, &geometry.indices)) {
            self.geometry_upload_bytes += upload(device, queue, &mut self.indices, &mut self.prior_indices, &geometry.indices, wgpu::BufferUsages::INDEX);
        }
        self.resident_indices = Arc::downgrade(&geometry.indices);
        self.resident_geometry = Arc::downgrade(geometry);
    }
}

impl EmoteRaster {
    pub(super) fn render_indexed(&mut self, device: &wgpu::Device, queue: &wgpu::Queue,
        model: &Arc<CpuMotionModel>, viewport: CpuViewport, mut draws: Vec<Draw<'_>>,
        failed: &Arc<AtomicBool>, started: Option<Instant>, evaluated: Option<Instant>,
        geometry: Option<Arc<IndexedGeometry>>,
    ) -> Result<EmoteRasterFrame> {
        let preparation_started = Instant::now();
        #[cfg(test)]
        INDEXED_RENDER_PATHS.with(|counts| {
            let [prepared, fallback] = counts.get();
            counts.set([prepared + usize::from(geometry.is_some()), fallback + usize::from(geometry.is_none())]);
        });
        let geometry = geometry.unwrap_or_else(|| self.mesh.as_mut().unwrap().geometry.prepare(&draws, viewport));
        let active: Vec<_> = draws.iter_mut().zip(&geometry.draws).filter(|(_, info)|
            info.bounds[0] < info.bounds[2] && info.bounds[1] < info.bounds[3]).collect();
        if active.is_empty() { return Ok(EmoteRasterFrame::new(1, 1, [0, 0], vec![0; 4])?); }
        let [x, y, right, bottom] = geometry.bounds;
        let [width, height] = [right - x, bottom - y];
        let bytes = fade_extent(device, [width, height]).context("mesh_output_limit")?;
        if bytes * 2 > u64::from(device.limits().max_storage_buffer_binding_size)
            || width.max(height) > device.limits().max_texture_dimension_2d {
            bail!("mesh_output_limit");
        }
        // Resolve every atlas before encoding: failure leaves no partial pose.
        let atlases: Vec<_> = active.iter().map(|(draw, _)| self.atlas(device, queue, model, draw.mesh.texture)).collect::<Result<_>>()?;
        let alignment = device.limits().min_storage_buffer_offset_alignment as usize / 4;
        let stride = 16usize.next_multiple_of(alignment);
        let run_count = active.iter().map(|(_, info)| info.ranges.len()).sum::<usize>();
        if run_count * stride > COMMAND_WORDS { bail!("mesh_run_command_limit"); }
        let mesh = self.mesh.as_mut().context("mesh_pipeline_unavailable")?;
        // Float32 stores these generations exactly. On wrap a single clear
        // retires all old generations before numbering restarts at one.
        let clear_coverage = u64::from(mesh.coverage_stamp) + run_count as u64 > 16_777_215;
        if clear_coverage { mesh.coverage_stamp = 0; }
        let mut headers = vec![0u32; run_count * stride];
        let mut active = active;
        let mut run = 0;
        for (draw, info) in &mut active {
            for _ in &info.ranges {
                let offset = run * stride;
                set_draw_header(draw, info.bounds, offset, 12, &mut headers);
                headers[offset + 4..offset + 7].copy_from_slice(&[width, x, y]);
                headers[offset + 12..offset + 14].copy_from_slice(&info.base.map(|v| v as i32 as u32));
                mesh.coverage_stamp += 1;
                headers[offset + 14] = mesh.coverage_stamp;
                run += 1;
            }
        }
        let grow = |current: &mut wgpu::Buffer, size, usage| {
            if current.size() < size { *current = buffer(device, size, usage); }
        };
        grow(&mut self.commands, (headers.len() * 4) as u64, UPLOAD);
        grow(&mut self.pixels, bytes * 2, UPLOAD);
        let output = self.fade_output(device, bytes);
        let data = unsafe { std::slice::from_raw_parts(headers.as_ptr().cast::<u8>(), headers.len() * 4) };
        queue.write_buffer(&self.commands, 0, data);
        queue.write_buffer(&self.size, 0, &(width * height).to_le_bytes());
        let mesh = self.mesh.as_mut().context("mesh_pipeline_unavailable")?;
        mesh.upload_geometry(device, queue, &geometry);
        if mesh.coverage.as_ref().is_none_or(|(texture, _)| texture.width() != width || texture.height() != height) {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("CS2 E-mote hardware UV coverage"),
                size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
                mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba32Float,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let view = texture.create_view(&Default::default());
            mesh.coverage = Some((texture, view));
        }
        let coverage = &mesh.coverage.as_ref().unwrap().1;
        let composite = &mesh.composite;
        let pack = &mesh.pack;
        let triangle_count = geometry.indices.len() / 3;
        mesh.poses += 1;
        let sample_log = mesh.poses <= 3 || mesh.poses % 120 == 0;
        let prepared = started.map(|_| Instant::now());
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.clear_buffer(&self.pixels, 0, Some(bytes * 2));
        let mut run = 0;
        for ((_, info), atlas) in active.iter().zip(&atlases) {
            // One atlas binding is shared by all ordered runs in this draw.
            let bindings = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("CS2 E-mote mesh composite"), layout: &composite.get_bind_group_layout(0),
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &self.commands, offset: 0, size: NonZeroU64::new(64),
                    }) },
                    wgpu::BindGroupEntry { binding: 1, resource: self.pixels.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(atlas) },
                    wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(coverage) },
                ],
            });
          for range in &info.ranges {
            let offset = run * stride;
            let stamp = headers[offset + 14];
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("CS2 ordered indexed E-mote mesh"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: coverage, depth_slice: None, resolve_target: None,
                        ops: wgpu::Operations {
                            load: if clear_coverage && run == 0 { wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT) }
                                else { wgpu::LoadOp::Load },
                            store: wgpu::StoreOp::Store,
                        },
                    })], ..Default::default()
                });
                pass.set_pipeline(&mesh.raster);
                pass.set_vertex_buffer(0, mesh.vertices.slice(..));
                pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(range.clone(), 0, stamp..stamp + 1);
            }
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(composite);
            pass.set_bind_group(0, &bindings, &[(offset * 4) as u32]);
            pass.dispatch_workgroups(headers[offset].div_ceil(8), headers[offset + 1].div_ceil(8), 1);
            run += 1;
          }
        }
        let bindings = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("CS2 indexed E-mote RGBA"), layout: &pack.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.size.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: self.pixels.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: output.as_entire_binding() },
            ],
        });
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(pack); pass.set_bind_group(0, &bindings, &[]);
            pass.dispatch_workgroups((width * height).div_ceil(64), 1, 1);
        }
        queue.submit(Some(encoder.finish()));
        let draw_count = active.len();
        if sample_log {
            report(&format!("emote_mesh draws={draw_count} triangles={triangle_count} runs={run_count} preview=true cpu_prepare_encode_us={} uv_bytes={} vertex_upload_capacity={} index_upload_capacity={} geometry_upload_bytes={} coverage_clears={} composite_bindings={draw_count} fallback_geometry_hits={} fallback_geometry_misses={}",
                preparation_started.elapsed().as_micros(), u64::from(width) * u64::from(height) * 16, mesh.vertices.size(), mesh.indices.size(),
                mesh.geometry_upload_bytes, u32::from(clear_coverage), mesh.geometry.hits, mesh.geometry.misses));
            let counters = model.node_cache_counters();
            report(&format!("emote_node_cache affine_hits={} affine_misses={} mesh_hits={} mesh_misses={} retained_bytes={}",
                counters[0], counters[1], counters[2], counters[3], counters[4]));
        }
        if let (Some(start), Some(evaluated), Some(prepared)) = (started, evaluated, prepared) {
            let now = Instant::now();
            emote_gpu_pose([start, evaluated, prepared, now, now], [Duration::ZERO, prepared - evaluated],
                draw_count, run_count, u64::from(width) * u64::from(height), 0);
        }
        static REPORTED: AtomicBool = AtomicBool::new(false);
        if !REPORTED.swap(true, Ordering::Relaxed) { report("draw=emote_mesh backend=indexed scanline_commands=0"); }
        #[cfg(test)]
        eprintln!("emote_mesh draws={draw_count} triangles={triangle_count} runs={run_count} preview=true");
        Ok(EmoteRasterFrame::from_device(width, height, [x as i32, y as i32], Arc::new(DevicePose {
            buffer: output, device: device.clone(), queue: queue.clone(), bytes, failed: failed.clone(), fade_alpha: None,
        }))?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indexed_geometry_cache_checks_exact_uv_topology_and_viewport() -> Result<()> {
        use cs2_runtime::emote::CpuMeshGrid;
        let texture = CpuTexture::from_rgba(1, 1, vec![255; 4])?;
        let vertices = vec![[0., 0., 0., 0.], [4., 0., 1., 0.], [0., 4., 0., 1.], [4., 4., 1., 1.], [0., 8., 0., 2.], [4., 8., 1., 2.]];
        let mut draws = vec![Draw {
            mesh: EmoteSpriteMesh::from_grid("texture", &texture, 0.5, vertices, CpuMeshGrid { columns: 1, rows: 2 })?,
            stencil: Stencil::default(), reference: 0, operation: None, bounds: [0; 4], offset: 0, words: 0,
        }];
        let mut viewport = CpuViewport { width: 16, height: 16, origin_x: 0., origin_y: 0., scale: 1., time_units_per_frame: 1. };
        let mut cache = GeometryCache::default();
        let first = cache.prepare(&draws, viewport);
        assert_eq!(first.indices.as_slice(), &[0, 1, 2, 2, 1, 3, 2, 3, 4, 4, 3, 5]);
        draws[0].mesh.opacity = 0.7;
        draws[0].reference = 3;
        let same = cache.prepare(&draws, viewport);
        assert!(Arc::ptr_eq(&first, &same), "live alpha and stencil headers do not change coverage geometry");
        draws[0].mesh.vertices[0][2] = f64::from_bits(1);
        let uv = cache.prepare(&draws, viewport);
        assert!(!Arc::ptr_eq(&same, &uv), "even UV changes below f32 precision must invalidate the exact cache");
        assert!(Arc::ptr_eq(&same.keys[0].indices, &uv.keys[0].indices), "deforming geometry keeps its exact topology");
        assert!(Arc::ptr_eq(&same.indices, &uv.indices), "unchanged topology and visibility reuse the combined GPU index data");
        viewport.height = 6;
        let clipped = cache.prepare(&draws, viewport);
        assert!(!Arc::ptr_eq(&uv, &clipped));
        assert_eq!(clipped.bounds, [0, 0, 4, 6]);
        let changed = EmoteSpriteMesh::from_grid("texture", &texture, 0.7,
            draws.remove(0).mesh.vertices, CpuMeshGrid { columns: 2, rows: 1 })?;
        draws.push(Draw { mesh: changed, stencil: Stencil::default(), reference: 0,
            operation: None, bounds: [0; 4], offset: 0, words: 0 });
        let topology = cache.prepare(&draws, viewport);
        assert_eq!(topology.indices.as_slice(), &[0, 1, 3, 3, 1, 4, 1, 2, 4, 4, 2, 5]);
        assert!(!Arc::ptr_eq(&uv.keys[0].indices, &topology.keys[0].indices));
        Ok(())
    }

    #[test]
    fn spatial_runs_match_brute_force_for_folded_and_degenerate_triangles() {
        let mut seed = 0x319afu32;
        let mut random = || {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            f64::from((seed >> 16) % 32)
        };
        for count in [0, 1, 2, 64, 256, 512] {
            let triangles: Vec<_> = (0..count).map(|i| {
                let mut triangle = std::array::from_fn(|_| [random(), random(), 0., 0.]);
                if i % 7 == 0 { triangle[2] = triangle[0]; }
                triangle
            }).collect();
            let mut expected = Vec::new();
            let mut start = 0;
            for i in 0..triangles.len() {
                if triangle_area(triangles[i]).abs() >= 1e-12 && (start..i).any(|j|
                    triangle_area(triangles[j]).abs() >= 1e-12 && interiors_overlap(triangles[j], triangles[i]))
                {
                    expected.push(start as u32 * 3..i as u32 * 3);
                    start = i;
                }
            }
            if start < triangles.len() { expected.push(start as u32 * 3..triangles.len() as u32 * 3); }
            assert_eq!(ordered_runs(&triangles), expected, "broad-phase pruning must preserve every ordered boundary");
        }
    }

    #[test]
    fn hardware_mesh_rejects_overlap_but_accepts_shared_edges() {
        let a = [[0., 0., 0., 0.], [2., 0., 2., 0.], [0., 2., 0., 2.]];
        let b = [[0., 2., 0., 2.], [2., 0., 2., 0.], [2., 2., 2., 2.]];
        assert!(!interiors_overlap(a, b));
        assert!(interiors_overlap(a, a));
        let shifted = a.map(|mut v| { v[0] += 0.1; v[1] += 0.1; v });
        assert!(interiors_overlap(a, shifted));
        assert_eq!(ordered_runs(&[a, b]), vec![0..6]);
        assert_eq!(ordered_runs(&[a, a]), vec![0..3, 3..6]);
    }
}

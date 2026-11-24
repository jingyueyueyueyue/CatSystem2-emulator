//! Indexed hardware triangle coverage followed by ordered integer composition.
//! Folded meshes are split into ordered non-overlapping triangle runs: one UV
//! attachment cannot represent multiple samples at the same pixel at once.
use super::*;
use crate::perf_detail::emote_gpu_encode_elapsed;

const OVERLAP_BINS: usize = 32;
const MAX_BATCH_RUNS: usize = 128;
const MAX_BATCHED_POSES: usize = 4;
const MAX_IN_FLIGHT_MESH_BATCHES: usize = 4;
const MAX_BATCHED_POSE_UPLOAD: usize = 2 * 1024 * 1024;
const SLOW_MESH_BATCH_PHASE: Duration = Duration::from_millis(8);

pub(super) fn enabled() -> bool {
    #[cfg(windows)]
    { !std::env::var("CS2_EMOTE_MESH").is_ok_and(|value| value == "0") }
    #[cfg(target_os = "android")]
    { !super::super::android_property(c"debug.cs2.emote_mesh", |value| value == b"0").unwrap_or(false) }
}

pub(super) fn batch_submissions_enabled() -> bool {
    #[cfg(windows)]
    {
        static ENABLED: OnceLock<bool> = OnceLock::new();
        *ENABLED.get_or_init(|| !std::env::var("CS2_EMOTE_MESH_BATCH").is_ok_and(|value| value == "0"))
    }
    #[cfg(target_os = "android")]
    { true }
}

fn layered_coverage_enabled() -> bool {
    #[cfg(windows)]
    {
        static ENABLED: OnceLock<bool> = OnceLock::new();
        *ENABLED.get_or_init(|| !std::env::var("CS2_EMOTE_MESH_LAYERED").is_ok_and(|value| value == "0"))
    }
    #[cfg(target_os = "android")]
    { !super::super::android_property(c"debug.cs2.emote_mesh_layered", |value| value == b"0").unwrap_or(false) }
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

struct CachedRun {
    indices: std::ops::Range<u32>,
    bounds: [f64; 4],
}

fn ordered_runs(triangles: &[[[f64; 4]; 3]]) -> Vec<CachedRun> {
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
    let extent: [f64; 4] = std::array::from_fn(|axis| bounds.iter().map(|v| v[axis])
        .fold(if axis < 2 { f64::INFINITY } else { f64::NEG_INFINITY },
            if axis < 2 { f64::min } else { f64::max }));
    let axis = usize::from(extent[3] - extent[1] > extent[2] - extent[0]);
    let scale = OVERLAP_BINS as f64 / (extent[axis + 2] - extent[axis]).max(1e-12);
    let bins_for = |bounds: [f64; 4]| {
        let bin = |v: f64| (((v - extent[axis]) * scale).floor() as usize).min(OVERLAP_BINS - 1);
        bin(bounds[axis])..=bin(bounds[axis + 2])
    };
    let mut bins: [Vec<usize>; OVERLAP_BINS] = std::array::from_fn(|_| Vec::new());
    let mut visited = vec![usize::MAX; triangles.len()];
    let mut runs = Vec::new();
    let mut start = 0;
    let make_run = |start: usize, end: usize| CachedRun {
        indices: start as u32 * 3..end as u32 * 3,
        bounds: if end - start == triangles.len() { extent } else {
            bounds[start..end].iter().fold([f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY], |a, b|
                [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])])
        },
    };
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
            runs.push(make_run(start, i));
            start = i;
            for bin in &mut bins { bin.clear(); }
        }
        if visible[i] { for bin in bins_for(bounds[i]) { bins[bin].push(i); } }
    }
    if start < triangles.len() { runs.push(make_run(start, triangles.len())); }
    runs
}

pub(super) fn compatible(draws: &[Draw<'_>]) -> Result<()> {
    for draw in draws { validate_mesh(&draw.mesh)?; }
    Ok(())
}

fn coverage_batches(bounds: &[[u32; 4]]) -> Vec<std::ops::Range<usize>> {
    let mut batches = Vec::new();
    let mut start = 0;
    for (i, b) in bounds.iter().enumerate() {
        if i - start == MAX_BATCH_RUNS || bounds[start..i].iter().any(|a| coverage_overlap(a, b))
        {
            batches.push(start..i);
            start = i;
        }
    }
    if start < bounds.len() { batches.push(start..bounds.len()); }
    batches
}

fn coverage_overlap(a: &[u32; 4], b: &[u32; 4]) -> bool {
    a[0] < b[2] && b[0] < a[2] && a[1] < b[3] && b[1] < a[3]
}

struct CoverageSchedule {
    ranges: Vec<std::ops::Range<usize>>,
    order: Vec<usize>,
    adjacent_count: usize,
}

impl CoverageSchedule {
    fn new(bounds: &[[u32; 4]], layered: bool) -> Self {
        let ranges = coverage_batches(bounds);
        let adjacent_count = ranges.len();
        if layered {
            let mut order = Vec::with_capacity(bounds.len());
            let mut layers = Vec::with_capacity(adjacent_count);
            for (chunk_index, chunk) in bounds.chunks(MAX_BATCH_RUNS).enumerate() {
                let mut levels = [0; MAX_BATCH_RUNS];
                let mut highest = 0;
                for (index, bounds) in chunk.iter().enumerate() {
                    // A dispatch reads and writes only inside this rectangle.
                    // Every earlier overlapping run must finish first.
                    let mut next_level = 0;
                    for (prior, &level) in chunk[..index].iter().zip(&levels[..index]) {
                        if level >= next_level && coverage_overlap(prior, bounds) {
                            next_level = level + 1;
                        }
                    }
                    levels[index] = next_level;
                    highest = highest.max(next_level);
                }
                let chunk_start = chunk_index * MAX_BATCH_RUNS;
                for level in 0..=highest {
                    let start = order.len();
                    order.extend(levels[..chunk.len()].iter().enumerate().filter_map(|(index, &other)|
                        (other == level).then_some(chunk_start + index)));
                    layers.push(start..order.len());
                }
            }
            if layers.len() < adjacent_count {
                return Self { ranges: layers, order, adjacent_count };
            }
        }
        Self { ranges, order: Vec::new(), adjacent_count }
    }

    fn runs(&self, batch: &std::ops::Range<usize>) -> impl Iterator<Item = usize> + '_ {
        batch.clone().map(|index| self.order.get(index).copied().unwrap_or(index))
    }
}

struct CachedMesh {
    grid: [u32; 2],
    vertices: Box<[[u64; 4]]>,
    bounds: [f64; 4],
    base: [f64; 2],
    indices: Arc<Vec<u32>>,
    runs: Arc<Vec<CachedRun>>,
}

struct IndexedRun {
    indices: std::ops::Range<u32>,
    bounds: [u32; 4],
}

struct IndexedDraw {
    bounds: [u32; 4],
    base: [f64; 2],
    runs: Vec<IndexedRun>,
}

pub(super) struct IndexedGeometry {
    viewport: [u32; 2],
    keys: Vec<Arc<CachedMesh>>,
    bounds: [u32; 4],
    vertices: Vec<u32>,
    indices: Arc<Vec<u32>>,
    draws: Vec<IndexedDraw>,
    coverage: CoverageSchedule,
}

impl IndexedGeometry {
    pub(super) fn active_draws(&self) -> impl Iterator<Item = usize> + '_ {
        self.draws.iter().enumerate().filter_map(|(index, draw)| (!draw.runs.is_empty()).then_some(index))
    }
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
    pub(super) fn retained_bytes(&self) -> usize {
        // Count allocated CPU geometry once even when the current frame,
        // mesh slots, or multiple topologies share immutable Arc allocations.
        fn shared_vec<T>(values: &Arc<Vec<T>>, seen: &mut std::collections::HashSet<usize>) -> usize {
            if !seen.insert(Arc::as_ptr(values) as usize) { return 0; }
            size_of::<Vec<T>>() + 2 * size_of::<usize>() + values.capacity() * size_of::<T>()
        }
        let mut seen = std::collections::HashSet::new();
        let mut bytes = self.meshes.capacity() * size_of::<Arc<CachedMesh>>();
        for mesh in self.meshes.iter().chain(self.frame.iter().flat_map(|frame| frame.keys.iter())) {
            if seen.insert(Arc::as_ptr(mesh) as usize) {
                bytes += size_of::<CachedMesh>() + 2 * size_of::<usize>()
                    + mesh.vertices.len() * size_of::<[u64; 4]>()
                    + shared_vec(&mesh.indices, &mut seen) + shared_vec(&mesh.runs, &mut seen);
            }
        }
        if let Some(frame) = &self.frame {
            bytes += size_of::<IndexedGeometry>() + 2 * size_of::<usize>()
                + frame.keys.capacity() * size_of::<Arc<CachedMesh>>()
                + frame.vertices.capacity() * size_of::<u32>()
                + frame.draws.capacity() * size_of::<IndexedDraw>()
                + frame.draws.iter().map(|draw| draw.runs.capacity() * size_of::<IndexedRun>()).sum::<usize>()
                + frame.coverage.ranges.capacity() * size_of::<std::ops::Range<usize>>()
                + frame.coverage.order.capacity() * size_of::<usize>()
                + shared_vec(&frame.indices, &mut seen);
        }
        bytes
    }

    pub(super) fn reservation_bytes(&self, draws: &[Draw<'_>]) -> usize {
        // Indexed-preparation admission estimate: exact retained geometry plus
        // prospective buffers and worst-case overlap scratch. Model/node
        // caches, evaluator work before admission, and GPU resources are separate.
        let prospective = draws.iter().map(|draw| {
            let [columns, rows] = draw.mesh.mesh_dimensions();
            let triangles = columns as usize * rows as usize * 2;
            // Owned vertices reuse the actual evaluated allocation. Exact keys
            // use boxed slices; growing packed/index/run Vecs allow 2x capacity.
            let vertices = draw.mesh.vertices.capacity() * size_of::<[f64; 4]>()
                + draw.mesh.vertices.len() * (size_of::<[u64; 4]>() + 2 * size_of::<[f32; 4]>());
            let indices_and_runs = 4 * size_of::<[u32; 3]>() + 2 * (size_of::<CachedRun>() + size_of::<IndexedRun>()
                + size_of::<std::ops::Range<usize>>() + size_of::<usize>() + size_of::<[u32; 4]>());
            let overlap_scratch = size_of::<[[f64; 4]; 3]>() + size_of::<[f64; 4]>()
                + size_of::<usize>() + size_of::<bool>() + 2 * OVERLAP_BINS * size_of::<usize>();
            // Each spatial bin can retain entries for every triangle. Include
            // minimum Vec capacities and allocation headers in addition to 2x growth.
            let minimum_capacities = OVERLAP_BINS * (4 * size_of::<usize>() + size_of::<Vec<usize>>())
                + 8 * size_of::<u32>() + 4 * (size_of::<CachedRun>() + size_of::<IndexedRun>()) + 16 * size_of::<usize>();
            vertices + triangles * (indices_and_runs + overlap_scratch) + minimum_capacities
                + 2 * size_of::<CachedMesh>() + 2 * size_of::<IndexedDraw>()
                + 2 * size_of::<OwnedDraw>() + 2 * size_of::<Draw<'_>>() + size_of::<IndexedGeometry>()
                + 8 * size_of::<Arc<CachedMesh>>() + 2 * draw.mesh.source.len()
        }).fold(0usize, usize::saturating_add);
        self.retained_bytes().saturating_add(prospective)
    }

    pub(super) fn counters(&self) -> [usize; 2] { [self.hits, self.misses] }

    pub(super) fn prepare(&mut self, draws: &[Draw<'_>], viewport: CpuViewport) -> Result<Arc<IndexedGeometry>> {
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
            // Coverage and ordered overlap runs depend only on exact XY and
            // topology. Animated atlas/UV selection must not redo SAT work.
            let coverage = prior.filter(|prior| prior.grid == grid
                && prior.vertices.len() == mesh.vertices.len()
                && prior.vertices.iter().zip(&mesh.vertices).all(|(a, b)|
                    [a[0], a[1]] == [b[0], b[1]].map(f64::to_bits)));
            let bounds = coverage.map(|prior| prior.bounds).unwrap_or_else(|| std::array::from_fn(|i| {
                let values = mesh.vertices.iter().map(|v| v[i % 2]);
                if i < 2 { values.fold(f64::INFINITY, f64::min) }
                else { values.fold(f64::NEG_INFINITY, f64::max) }
            }));
            let base = std::array::from_fn(|axis| mesh.vertices.iter().map(|v| v[axis + 2])
                .fold(f64::INFINITY, f64::min).floor());
            let indices = prior.filter(|prior| prior.grid == grid).map(|prior| prior.indices.clone())
                .unwrap_or_else(|| Arc::new(mesh.triangle_indices().flatten().map(|v| v as u32).collect()));
            let runs = coverage.map(|prior| prior.runs.clone()).unwrap_or_else(|| {
                let triangles: Vec<_> = indices.chunks_exact(3).map(|ids|
                    [mesh.vertices[ids[0] as usize], mesh.vertices[ids[1] as usize], mesh.vertices[ids[2] as usize]]).collect();
                Arc::new(ordered_runs(&triangles))
            });
            Arc::new(CachedMesh {
                grid, vertices: mesh.vertices.iter().map(|v| v.map(f64::to_bits)).collect(),
                bounds, base, indices, runs,
            })
        }).collect();
        let size = [viewport.width, viewport.height];
        if let Some(frame) = &self.frame
            && frame.viewport == size && frame.keys.len() == keys.len()
            && frame.keys.iter().zip(&keys).all(|(a, b)| Arc::ptr_eq(a, b))
        {
            return Ok(frame.clone());
        }
        let mut bounds = [viewport.width, viewport.height, 0, 0];
        let mut prepared: Vec<_> = keys.iter().map(|mesh| {
            let draw_bounds = std::array::from_fn(|i|
                mesh.bounds[i].ceil().clamp(0., f64::from(size[i % 2])) as u32);
            if draw_bounds[0] < draw_bounds[2] && draw_bounds[1] < draw_bounds[3] {
                extend_bounds(&mut bounds, draw_bounds);
            }
            IndexedDraw { bounds: draw_bounds, base: mesh.base, runs: Vec::new() }
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
        let mut vertex_words = 0;
        let mut index_words = 0;
        for ((draw, mesh), prepared) in draws.iter().zip(&keys).zip(&prepared) {
            let [left, top, right, bottom] = prepared.bounds;
            if left < right && top < bottom {
                vertex_words += draw.mesh.vertices.len() * 4;
                index_words += mesh.indices.len();
            }
        }
        // Reserve the packed pose once. Growing the vertex vector in the
        // inner loop can retain the old and new allocations during realloc.
        vertices.try_reserve_exact(vertex_words).context("reserve E-mote packed vertices")?;
        if reused_indices.is_none() {
            indices.try_reserve_exact(index_words).context("reserve E-mote packed indices")?;
        }
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
            prepared.runs = mesh.runs.iter().filter_map(|run| {
                // Leave a pixel around the tighter bounds for hardware's
                // subpixel setup, then intersect with the original scissor.
                let bounds = if mesh.runs.len() == 1 { prepared.bounds } else {
                    std::array::from_fn(|axis| {
                        let edge = run.bounds[axis].ceil();
                        if axis < 2 { ((edge - 1.) as u32).max(prepared.bounds[axis]) }
                        else { ((edge + 1.) as u32).min(prepared.bounds[axis]) }
                    })
                };
                (bounds[0] < bounds[2] && bounds[1] < bounds[3]).then_some(IndexedRun {
                    indices: run.indices.start + first_index..run.indices.end + first_index,
                    bounds,
                })
            }).collect();
        }
        let indices = reused_indices.unwrap_or_else(|| Arc::new(indices));
        let run_bounds: Vec<_> = prepared.iter().flat_map(|draw| draw.runs.iter().map(|run| run.bounds)).collect();
        let coverage = CoverageSchedule::new(&run_bounds, layered_coverage_enabled());
        self.meshes = keys.clone();
        let frame = Arc::new(IndexedGeometry { viewport: size, keys, bounds, vertices, indices, draws: prepared, coverage });
        self.frame = Some(frame.clone());
        Ok(frame)
    }
}

pub(super) struct MeshRaster {
    raster: wgpu::RenderPipeline,
    composite: wgpu::ComputePipeline,
    composite_layout: wgpu::BindGroupLayout,
    stencil: wgpu::Buffer,
    uploads: Vec<MeshUpload>,
    pub(super) submission: Arc<MeshSubmission>,
    pub(super) geometry_upload_bytes: u64,
    pub(super) encoding_counts: [usize; 3],
    geometry: GeometryCache,
    coverage: Option<(wgpu::Texture, wgpu::TextureView)>,
    pub(super) coverage_stamp: u32,
    poses: u64,
}

struct MeshUpload {
    commands: wgpu::Buffer,
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    prior_indices: Vec<u32>,
    prior_vertices: Vec<u32>,
    resident_geometry: Weak<IndexedGeometry>,
    resident_indices: Weak<Vec<u32>>,
}

// Device poses may be read from another thread. Hold the submission lock
// while encoding so a reader cannot release an upload slot in mid-command.
#[derive(Debug, Default)]
pub(super) struct MeshSubmission {
    pending: Mutex<MeshSubmissions>,
}

#[derive(Debug, Default)]
struct MeshSubmissions {
    batch: Option<MeshCommandBatch>,
    in_flight: VecDeque<wgpu::SubmissionIndex>,
}

#[derive(Debug)]
struct MeshCommandBatch {
    encoder: wgpu::CommandEncoder,
    outputs: Vec<Arc<wgpu::Buffer>>,
    poses: usize,
    coverage_passes: usize,
    encode_time: Duration,
    sample_log: bool,
}

impl MeshCommandBatch {
    fn new(device: &wgpu::Device) -> Self {
        Self {
            encoder: device.create_command_encoder(&Default::default()),
            outputs: Vec::new(),
            poses: 0,
            coverage_passes: 0,
            encode_time: Duration::ZERO,
            sample_log: false,
        }
    }

    fn submit(self, queue: &wgpu::Queue) -> (wgpu::SubmissionIndex, Duration, Duration) {
        #[cfg(windows)]
        let thread_before_finish = mesh_thread_cpu_time();
        let finish_started = Instant::now();
        let commands = self.encoder.finish();
        let finish_time = finish_started.elapsed();
        #[cfg(windows)]
        let thread_before_submit = thread_before_finish.and_then(|_| mesh_thread_cpu_time());
        let submit_started = Instant::now();
        let submission = crate::gpu_raster::submit_gpu_commands(queue, Some(commands));
        let submit_time = submit_started.elapsed();
        let slow = [self.encode_time, finish_time, submit_time].into_iter()
            .any(|time| time > SLOW_MESH_BATCH_PHASE);
        #[cfg(windows)]
        let thread_cpu = thread_before_finish.zip(thread_before_submit)
            .filter(|_| slow)
            .and_then(|(before_finish, before_submit)| mesh_thread_cpu_time().map(|after_submit| (
                before_submit.saturating_sub(before_finish) / 10,
                after_submit.saturating_sub(before_submit) / 10,
            )));
        if self.sample_log || slow {
            report(&format!(
                "emote_mesh_batch poses={} cpu_encode_us={} cpu_submit_us={} submits=1 cpu_record_us={} cpu_finish_us={} slow={} coverage_passes={}",
                self.poses, (self.encode_time + finish_time).as_micros(), submit_time.as_micros(),
                self.encode_time.as_micros(), finish_time.as_micros(), u32::from(slow), self.coverage_passes,
            ));
        }
        #[cfg(windows)]
        if let Some((finish_us, submit_us)) = thread_cpu {
            report(&format!("emote_mesh_thread_cpu poses={} finish_thread_us={} submit_thread_us={}",
                self.poses, finish_us, submit_us));
        }
        (submission, finish_time, submit_time)
    }

    fn submit_deferred(self, queue: &wgpu::Queue) -> wgpu::SubmissionIndex {
        // Per-pose timing ended after command recording. Charge the
        // shared encoder finalization and submission to this frame.
        let (submission, finish_time, submit_time) = self.submit(queue);
        emote_gpu_encode_elapsed(finish_time + submit_time);
        submission
    }
}

#[cfg(windows)]
fn mesh_thread_cpu_time() -> Option<u64> {
    #[repr(C)]
    #[derive(Default)]
    struct FileTime {
        low: u32,
        high: u32,
    }

    impl FileTime {
        fn ticks(&self) -> u64 {
            u64::from(self.high) << 32 | u64::from(self.low)
        }
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentThread() -> isize;
        fn GetThreadTimes(thread: isize, created: *mut FileTime, exited: *mut FileTime,
            kernel: *mut FileTime, user: *mut FileTime) -> i32;
    }

    static ENABLED: OnceLock<bool> = OnceLock::new();
    if !*ENABLED.get_or_init(|| std::env::var("CS2_EMOTE_MESH_THREAD_TIME").is_ok_and(|value| value == "1")) {
        return None;
    }
    let [mut created, mut exited, mut kernel, mut user] = std::array::from_fn(|_| FileTime::default());
    // The current thread pseudo-handle is valid for the caller and needs no close.
    let ok = unsafe { GetThreadTimes(GetCurrentThread(), &mut created, &mut exited, &mut kernel, &mut user) };
    (ok != 0).then(|| kernel.ticks() + user.ticks())
}

impl MeshSubmission {
    pub(super) fn flush(&self, queue: &wgpu::Queue) {
        let mut pending = self.pending.lock().unwrap();
        if let Some(batch) = pending.batch.take() {
            pending.in_flight.push_back(batch.submit_deferred(queue));
        }
    }

    #[cfg(test)]
    pub(super) fn pending_poses(&self) -> usize {
        self.pending.lock().unwrap().batch.as_ref().map_or(0, |batch| batch.poses)
    }
}

struct MeshMaterial<'a> {
    texture: &'a CpuTexture,
    opacity: f64,
    stencil: Stencil,
    reference: u8,
    operation: Option<CpuStencilWrite>,
}

impl MeshRaster {
    pub(super) fn new(device: &wgpu::Device) -> Result<Self> {
        #[cfg(target_os = "android")]
        let validation = crate::gpu_raster::lock_validation_scope();
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
                storage(0, true),
                texture(2, wgpu::TextureSampleType::Uint),
                texture(4, wgpu::TextureSampleType::Float { filterable: false }),
                storage(5, false), storage(6, false),
            ],
        });
        let composite_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None, bind_group_layouts: &[&bindings], push_constant_ranges: &[],
        });
        let composite = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("CS2 E-mote ordered mesh composite"), module: &module,
            entry_point: Some("mesh_sprite"), layout: Some(&composite_layout),
            compilation_options: wgpu::PipelineCompilationOptions { constants, ..Default::default() }, cache: None,
        });
        if let Some(error) = pollster::block_on(device.pop_error_scope()) {
            bail!("indexed_mesh_pipeline: {error}");
        }
        #[cfg(target_os = "android")]
        drop(validation);
        Ok(Self {
            raster, composite, composite_layout: bindings,
            stencil: buffer(device, 4, UPLOAD),
            uploads: vec![MeshUpload::new(device)], submission: Arc::default(),
            geometry: GeometryCache::default(), geometry_upload_bytes: 0, encoding_counts: [0; 3],
            coverage: None, coverage_stamp: 0, poses: 0,
        })
    }

    pub(super) fn ensure_coverage(&mut self, device: &wgpu::Device, width: u32, height: u32, limit: u32) -> bool {
        // Keep a little room for moving bounds. The viewport handles the
        // unpadded extent and the coverage generation excludes old contents.
        let padded = |axis: u32| {
            let step = axis.min(64).next_power_of_two();
            axis.next_multiple_of(step).min(limit)
        };
        let [padded_width, padded_height] = [padded(width), padded(height)];
        let max_retained_area = 2 * u64::from(padded_width) * u64::from(padded_height);
        if self.coverage.as_ref().is_some_and(|(texture, _)| {
            texture.width() >= width && texture.height() >= height
                && u64::from(texture.width()) * u64::from(texture.height()) <= max_retained_area
        }) {
            return false;
        }
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("CS2 E-mote hardware UV coverage"),
            size: wgpu::Extent3d { width: padded_width, height: padded_height, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        self.coverage = Some((texture, view));
        true
    }
}

impl MeshUpload {
    fn new(device: &wgpu::Device) -> Self {
        Self {
            commands: buffer(device, 4, UPLOAD),
            vertices: buffer(device, 16, wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST),
            indices: buffer(device, 4, wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST),
            prior_indices: Vec::new(), prior_vertices: Vec::new(),
            resident_geometry: Weak::new(), resident_indices: Weak::new(),
        }
    }

    fn upload_geometry(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, geometry: &Arc<IndexedGeometry>) -> u64 {
        if self.resident_geometry.upgrade().is_some_and(|prior| Arc::ptr_eq(&prior, geometry)) { return 0; }
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
        let mut uploaded = upload(device, queue, &mut self.vertices, &mut self.prior_vertices, &geometry.vertices, wgpu::BufferUsages::VERTEX);
        if !self.resident_indices.upgrade().is_some_and(|prior| Arc::ptr_eq(&prior, &geometry.indices)) {
            uploaded += upload(device, queue, &mut self.indices, &mut self.prior_indices, &geometry.indices, wgpu::BufferUsages::INDEX);
        }
        self.resident_indices = Arc::downgrade(&geometry.indices);
        self.resident_geometry = Arc::downgrade(geometry);
        uploaded
    }
}

impl EmoteRaster {
    pub(super) fn render_indexed(&mut self, device: &wgpu::Device, queue: &wgpu::Queue,
        model: &Arc<CpuMotionModel>, viewport: CpuViewport, draws: Vec<Draw<'_>>,
        failed: &Arc<AtomicBool>, started: Option<Instant>, evaluated: Option<Instant>,
        geometry: Option<Arc<IndexedGeometry>>,
    ) -> Result<EmoteRasterFrame> {
        let preparation_started = Instant::now();
        #[cfg(test)]
        INDEXED_RENDER_PATHS.with(|counts| {
            let [prepared, fallback] = counts.get();
            counts.set([prepared + usize::from(geometry.is_some()), fallback + usize::from(geometry.is_none())]);
        });
        let geometry = match geometry {
            Some(geometry) => geometry,
            None => self.mesh.as_mut().unwrap().geometry.prepare(&draws, viewport)?,
        };
        let materials = draws.iter().map(|draw| MeshMaterial {
            texture: draw.mesh.texture, opacity: draw.mesh.opacity,
            stencil: draw.stencil, reference: draw.reference, operation: draw.operation,
        }).collect();
        self.render_indexed_materials(device, queue, model, materials, failed, started, evaluated, geometry, preparation_started)
    }

    pub(super) fn render_prepared_indexed(&mut self, device: &wgpu::Device, queue: &wgpu::Queue,
        model: &Arc<CpuMotionModel>, draws: &[OwnedDraw],
        failed: &Arc<AtomicBool>, started: Option<Instant>, evaluated: Option<Instant>,
        geometry: Arc<IndexedGeometry>,
    ) -> Result<EmoteRasterFrame> {
        let preparation_started = Instant::now();
        #[cfg(test)]
        INDEXED_RENDER_PATHS.with(|counts| { let [prepared, fallback] = counts.get(); counts.set([prepared + 1, fallback]); });
        let materials = draws.iter().map(|draw| {
            let (texture, opacity) = draw.mesh.texture_opacity(model)?;
            Ok(MeshMaterial { texture, opacity, stencil: draw.stencil,
                reference: draw.reference, operation: draw.operation })
        }).collect::<cs2_runtime::emote::Result<Vec<_>>>()?;
        self.render_indexed_materials(device, queue, model, materials, failed, started, evaluated, geometry, preparation_started)
    }

    fn render_indexed_materials(&mut self, device: &wgpu::Device, queue: &wgpu::Queue,
        model: &Arc<CpuMotionModel>, materials: Vec<MeshMaterial<'_>>,
        failed: &Arc<AtomicBool>, started: Option<Instant>, evaluated: Option<Instant>,
        geometry: Arc<IndexedGeometry>, preparation_started: Instant,
    ) -> Result<EmoteRasterFrame> {
        let active: Vec<_> = materials.iter().zip(&geometry.draws).filter(|(_, info)| !info.runs.is_empty()).collect();
        if active.is_empty() { return Ok(EmoteRasterFrame::new(1, 1, [0, 0], vec![0; 4])?); }
        let [x, y, right, bottom] = geometry.bounds;
        let [width, height] = [right - x, bottom - y];
        let bytes = fade_extent(device, [width, height]).context("mesh_output_limit")?;
        let limits = device.limits();
        if width.max(height) > limits.max_texture_dimension_2d {
            bail!("mesh_output_limit");
        }
        // Resolve every atlas before encoding: failure leaves no partial pose.
        let atlases: Vec<_> = active.iter().map(|(draw, _)| self.atlas(device, queue, model, draw.texture)).collect::<Result<_>>()?;
        let alignment = limits.min_storage_buffer_offset_alignment as usize / 4;
        let stride = 16usize.next_multiple_of(alignment);
        let runs: Vec<_> = active.iter().enumerate().flat_map(|(draw, (_, info))|
            info.runs.iter().map(move |run| (draw, run))).collect();
        let run_count = runs.len();
        if run_count * stride > COMMAND_WORDS { bail!("mesh_run_command_limit"); }
        let mesh = self.mesh.as_mut().context("mesh_pipeline_unavailable")?;
        // Float32 stores these generations exactly. On wrap a single clear
        // retires all old generations before numbering restarts at one.
        let clear_coverage = u64::from(mesh.coverage_stamp) + run_count as u64 > 16_777_215;
        if clear_coverage { mesh.coverage_stamp = 0; }
        let mut headers = vec![0u32; run_count * stride];
        for (index, (draw, run)) in runs.iter().enumerate() {
            let (draw, info) = active[*draw];
            let offset = index * stride;
            headers[offset..offset + 12].copy_from_slice(&draw_header_words(run.bounds,
                draw.stencil, draw.reference, draw.operation, draw.texture, draw.opacity));
            headers[offset + 4..offset + 7].copy_from_slice(&[width, x, y]);
            headers[offset + 12..offset + 14].copy_from_slice(&info.base.map(|v| v as i32 as u32));
            mesh.coverage_stamp += 1;
            headers[offset + 14] = mesh.coverage_stamp;
        }
        let header_bytes = (headers.len() * 4) as u64;
        let output_started = started.map(|_| Instant::now());
        let (output, output_reused) = self.acquire_output(device, bytes);
        let output_us = output_started.map_or(0, |start| start.elapsed().as_micros());
        let mesh = self.mesh.as_mut().context("mesh_pipeline_unavailable")?;
        let submission = mesh.submission.clone();
        let mut pending = submission.pending.lock().unwrap();
        let upload_words = headers.len() + geometry.vertices.len() + geometry.indices.len();
        let batches = &geometry.coverage.ranges;
        let defer = batching() && !CAPTURING.with(Cell::get) && batch_submissions_enabled()
            && upload_words <= MAX_BATCHED_POSE_UPLOAD / size_of::<u32>();
        if !defer && let Some(batch) = pending.batch.take() {
            pending.in_flight.push_back(batch.submit_deferred(queue));
        }
        // Offscreen batches can be produced without a swapchain acquire.
        // Reap old command buffers before the driver allocates another batch.
        if pending.in_flight.len() >= MAX_IN_FLIGHT_MESH_BATCHES {
            let earliest = pending.in_flight.front().unwrap();
            if !gpu_submission_finished(device, earliest, ATLAS_UPLOAD_WINDOW) {
                bail!("E-mote command queue is busy");
            }
            pending.in_flight.pop_front();
        }
        let slot = pending.batch.as_ref().map_or(0, |batch| batch.poses);
        if slot == mesh.uploads.len() {
            mesh.uploads.push(MeshUpload::new(device));
        }
        let stencil_needed = active.iter().any(|(draw, _)| draw.stencil.serial != 0);
        if stencil_needed && mesh.stencil.size() < bytes {
            mesh.stencil = buffer(device, bytes, UPLOAD);
        }
        let coverage_allocated = mesh.ensure_coverage(device, width, height, limits.max_texture_dimension_2d);
        let upload = &mut mesh.uploads[slot];
        if upload.commands.size() < header_bytes {
            upload.commands = buffer(device, header_bytes, UPLOAD);
        }
        let data = unsafe { std::slice::from_raw_parts(headers.as_ptr().cast::<u8>(), headers.len() * 4) };
        queue.write_buffer(&upload.commands, 0, data);
        mesh.geometry_upload_bytes = upload.upload_geometry(device, queue, &geometry);
        let (coverage_texture, coverage) = mesh.coverage.as_ref().unwrap();
        let composite = &mesh.composite;
        let triangle_count = geometry.indices.len() / 3;
        mesh.poses += 1;
        let sample_log = mesh.poses <= 3 || mesh.poses % 120 == 0;
        let prepared = started.map(|_| Instant::now());
        let commands_started = Instant::now();
        let command_batch = pending.batch.get_or_insert_with(|| MeshCommandBatch::new(device));
        command_batch.outputs.push(output.clone());
        let encoder = &mut command_batch.encoder;
        // Each ordered draw writes the final packed RGBA directly. Mask-only
        // state has its own scratch buffer and is untouched for unmasked poses.
        encoder.clear_buffer(&output, 0, Some(bytes));
        if stencil_needed { encoder.clear_buffer(&mesh.stencil, 0, Some(bytes)); }
        // Header offsets carry alpha/mask state, so every draw using the same
        // actual atlas view can share one immutable resource binding.
        let mut composite_bindings: Vec<(wgpu::TextureView, wgpu::BindGroup)> = Vec::new();
        let binding_indices: Vec<_> = atlases.iter().map(|atlas| {
            if let Some(index) = composite_bindings.iter().position(|(view, _)| view == atlas) { return index; }
            let binding = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("CS2 E-mote mesh composite"), layout: &mesh.composite_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &upload.commands, offset: 0, size: NonZeroU64::new(64),
                    }) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(atlas) },
                    wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(coverage) },
                    wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &output, offset: 0, size: NonZeroU64::new(bytes),
                    }) },
                    wgpu::BindGroupEntry { binding: 6, resource: mesh.stencil.as_entire_binding() },
                ],
            });
            let index = composite_bindings.len();
            composite_bindings.push((atlas.clone(), binding));
            index
        }).collect();
        for batch in batches {
            // Coverage and integer composition can be grouped only when their
            // entire pixel-write rectangles are disjoint. Overlapping runs,
            // including folded meshes and serial masks, retain their barriers.
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("CS2 ordered indexed E-mote meshes"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: coverage, depth_slice: None, resolve_target: None,
                        ops: wgpu::Operations {
                            load: if clear_coverage && batch.start == 0 { wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT) }
                                else { wgpu::LoadOp::Load },
                            store: wgpu::StoreOp::Store,
                        },
                    })], ..Default::default()
                });
                pass.set_pipeline(&mesh.raster);
                pass.set_viewport(0., 0., width as f32, height as f32, 0., 1.);
                pass.set_vertex_buffer(0, upload.vertices.slice(..));
                pass.set_index_buffer(upload.indices.slice(..), wgpu::IndexFormat::Uint32);
                for index in geometry.coverage.runs(batch) {
                    let run = runs[index].1;
                    let [left, top, right, bottom] = run.bounds;
                    pass.set_scissor_rect(left - x, top - y, right - left, bottom - top);
                    let stamp = headers[index * stride + 14];
                    pass.draw_indexed(run.indices.clone(), 0, stamp..stamp + 1);
                }
            }
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(composite);
            for run in geometry.coverage.runs(batch) {
                let offset = run * stride;
                let binding = &composite_bindings[binding_indices[runs[run].0]].1;
                pass.set_bind_group(0, binding, &[(offset * 4) as u32]);
                pass.dispatch_workgroups(headers[offset].div_ceil(8), headers[offset + 1].div_ceil(8), 1);
            }
        }
        let encode_time = commands_started.elapsed();
        command_batch.poses += 1;
        command_batch.coverage_passes += batches.len();
        command_batch.encode_time += encode_time;
        command_batch.sample_log |= sample_log;
        let submitted = !defer || command_batch.poses == MAX_BATCHED_POSES;
        let (finish_time, submit_time) = if submitted {
            let (submission, finish_time, submit_time) = pending.batch.take().unwrap().submit(queue);
            pending.in_flight.push_back(submission);
            (finish_time, submit_time)
        } else {
            (Duration::ZERO, Duration::ZERO)
        };
        drop(pending);
        let draw_count = active.len();
        mesh.encoding_counts = [run_count, batches.len(), composite_bindings.len()];
        if sample_log {
            let coverage_bytes = u64::from(coverage_texture.width()) * u64::from(coverage_texture.height()) * 16;
            let composite_pixels = runs.iter().map(|(_, run)| run.bounds).map(|[left, top, right, bottom]|
                u64::from(right - left) * u64::from(bottom - top)).sum::<u64>();
            report(&format!("emote_mesh draws={draw_count} triangles={triangle_count} runs={run_count} preview=true cpu_prepare_encode_us={} uv_bytes={} vertex_upload_capacity={} index_upload_capacity={} geometry_upload_bytes={} coverage_clears={} composite_bindings={} coverage_batches={} adjacent_coverage_batches={} ordered_passes_saved={} fallback_geometry_hits={} fallback_geometry_misses={} cpu_prepare_us={} cpu_encode_us={} cpu_submit_us={} coverage_bytes={coverage_bytes} coverage_allocations={} header_upload_bytes={header_bytes} buffer_clear_bytes={} composite_pixels={composite_pixels} pack_passes=0 submits={}",
                preparation_started.elapsed().as_micros(), u64::from(width) * u64::from(height) * 16, upload.vertices.size(), upload.indices.size(),
                mesh.geometry_upload_bytes, u32::from(clear_coverage), composite_bindings.len(), batches.len(),
                geometry.coverage.adjacent_count, (run_count - batches.len()) * 2, mesh.geometry.hits, mesh.geometry.misses,
                (commands_started - preparation_started).as_micros(), (encode_time + finish_time).as_micros(),
                submit_time.as_micros(), u32::from(coverage_allocated), bytes * (1 + u64::from(stencil_needed)), u32::from(submitted)));
            let counters = model.node_cache_counters();
            report(&format!("emote_mesh_output bytes={bytes} capacity={} reused={} acquire_us={output_us}",
                output.size(), u8::from(output_reused)));
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
            mesh_submission: (!submitted).then_some(submission),
        }))?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coverage_batches_join_only_consecutive_disjoint_pixel_regions() {
        // Adjacent regions may share an edge, while any intersecting write
        // region must retire coverage before a later run overwrites it.
        let regions = [[0, 0, 4, 4], [4, 0, 8, 4], [2, 2, 6, 6], [8, 0, 12, 4], [8, 0, 12, 4]];
        assert_eq!(coverage_batches(&regions), vec![0..2, 2..4, 4..5]);
        assert!(coverage_batches(&[]).is_empty());
    }

    #[test]
    fn coverage_layers_preserve_every_pixel_dependency() {
        let a = [0, 0, 4, 4];
        let b = [4, 0, 8, 4];
        let mut regions = Vec::new();
        for _ in 0..40 {
            regions.extend([a, a, b, b]);
        }
        let layers = CoverageSchedule::new(&regions, true);
        assert!(layers.ranges.len() < layers.adjacent_count);
        let mut slots = vec![None; regions.len()];
        for (layer, batch) in layers.ranges.iter().enumerate() {
            for run in layers.runs(batch) {
                assert!(slots[run].replace(layer).is_none());
            }
        }
        assert!(slots.iter().all(Option::is_some));
        for (index, current) in regions.iter().enumerate() {
            for (prior, previous) in regions[..index].iter().enumerate() {
                if coverage_overlap(current, previous) {
                    assert!(slots[prior] < slots[index]);
                }
            }
        }
        assert_eq!(layers.runs(&layers.ranges[0]).collect::<Vec<_>>(), [0, 2]);
        let stable = CoverageSchedule::new(&[a, a, a], true);
        assert!(stable.order.is_empty());
        assert_eq!(stable.ranges, [0..1, 1..2, 2..3]);
    }

    #[test]
    fn indexed_reservation_covers_retained_geometry_and_overlap_scratch() -> Result<()> {
        use cs2_runtime::emote::CpuMeshGrid;
        let texture = CpuTexture::from_rgba(1, 1, vec![255; 4])?;
        let vertices = (0..=16).flat_map(|y| (0..=16).map(move |x|
            [f64::from(x), f64::from(y), 0., 0.])).collect();
        let mut draws = vec![Draw {
            mesh: EmoteSpriteMesh::from_grid("texture", &texture, 1., vertices, CpuMeshGrid { columns: 16, rows: 16 })?,
            stencil: Stencil::default(), reference: 0, operation: None, bounds: [0; 4], offset: 0, words: 0,
        }];
        let viewport = CpuViewport { width: 32, height: 32, origin_x: 0., origin_y: 0., scale: 1., time_units_per_frame: 1. };
        let mut cache = GeometryCache::default();
        let previous = cache.prepare(&draws, viewport).unwrap();
        draws[0].mesh.vertices[0][0] = 0.125;
        // During changed-pose overlap analysis the previous immutable frame,
        // current evaluated vertices, and overlap scratch are all live. This
        // lower bound deliberately excludes spatial-bin capacity and metadata.
        let retained = previous.vertices.capacity() * size_of::<u32>()
            + previous.indices.capacity() * size_of::<u32>()
            + previous.keys.iter().map(|key| key.vertices.len() * size_of::<[u64; 4]>()
                + key.indices.capacity() * size_of::<u32>()
                + key.runs.capacity() * size_of::<CachedRun>()).sum::<usize>();
        let evaluated = draws[0].mesh.vertices.capacity() * size_of::<[f64; 4]>();
        let triangles = draws[0].mesh.triangle_indices().count();
        let scratch_minimum = triangles * (size_of::<[[f64; 4]; 3]>()
            + size_of::<[f64; 4]>() + size_of::<usize>() + size_of::<bool>());
        let reserved = cache.reservation_bytes(&draws);
        assert!(reserved >= retained + evaluated + scratch_minimum,
            "reservation={reserved} cannot cover retained={retained} evaluated={evaluated} overlap_scratch_minimum={scratch_minimum}");
        let smaller = vec![Draw {
            mesh: EmoteSpriteMesh::from_grid("texture", &texture, 1.,
                vec![[0., 0., 0., 0.], [1., 0., 0., 0.], [0., 1., 0., 0.], [1., 1., 0., 0.]],
                CpuMeshGrid { columns: 1, rows: 1 })?,
            stencil: Stencil::default(), reference: 0, operation: None, bounds: [0; 4], offset: 0, words: 0,
        }];
        assert!(cache.reservation_bytes(&smaller) >= retained,
            "shrinking topology still accounts for the previous large frame until replacement");
        Ok(())
    }

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
        let first = cache.prepare(&draws, viewport).unwrap();
        assert_eq!(first.indices.as_slice(), &[0, 1, 2, 2, 1, 3, 2, 3, 4, 4, 3, 5]);
        draws[0].mesh.opacity = 0.7;
        draws[0].reference = 3;
        let same = cache.prepare(&draws, viewport).unwrap();
        assert!(Arc::ptr_eq(&first, &same), "live alpha and stencil headers do not change coverage geometry");
        draws[0].mesh.vertices[0][2] = f64::from_bits(1);
        let uv = cache.prepare(&draws, viewport).unwrap();
        assert!(!Arc::ptr_eq(&same, &uv), "even UV changes below f32 precision must invalidate the exact cache");
        assert!(Arc::ptr_eq(&same.keys[0].indices, &uv.keys[0].indices), "deforming geometry keeps its exact topology");
        assert!(Arc::ptr_eq(&same.indices, &uv.indices), "unchanged topology and visibility reuse the combined GPU index data");
        assert!(Arc::ptr_eq(&same.keys[0].runs, &uv.keys[0].runs), "UV-only changes must reuse exact XY overlap analysis");
        viewport.height = 6;
        let clipped = cache.prepare(&draws, viewport).unwrap();
        assert!(!Arc::ptr_eq(&uv, &clipped));
        assert_eq!(clipped.bounds, [0, 0, 4, 6]);
        let changed = EmoteSpriteMesh::from_grid("texture", &texture, 0.7,
            draws.remove(0).mesh.vertices, CpuMeshGrid { columns: 2, rows: 1 })?;
        draws.push(Draw { mesh: changed, stencil: Stencil::default(), reference: 0,
            operation: None, bounds: [0; 4], offset: 0, words: 0 });
        let topology = cache.prepare(&draws, viewport).unwrap();
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
            let actual: Vec<_> = ordered_runs(&triangles).into_iter().map(|run| run.indices).collect();
            assert_eq!(actual, expected, "broad-phase pruning must preserve every ordered boundary");
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
        let adjacent = ordered_runs(&[a, b]);
        assert_eq!(adjacent.len(), 1);
        assert_eq!(adjacent[0].indices, 0..6);
        let folded = ordered_runs(&[a, shifted]);
        assert_eq!(folded.len(), 2);
        assert_eq!(folded[0].indices, 0..3);
        assert_eq!(folded[0].bounds, [0., 0., 2., 2.]);
        assert_eq!(folded[1].indices, 3..6);
        assert_eq!(folded[1].bounds, [0.1, 0.1, 2.1, 2.1]);
    }
}

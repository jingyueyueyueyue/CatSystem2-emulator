//! Ordered, bilinear scene triangles on the existing raster device.
//! Each invocation owns one destination pixel. Tile lists retain submission
//! order, so overlapping particles need neither atomics nor device readback.

use super::{RASTER, Raster, RasterRect, UPLOAD, blend_code, buffer, device_frame,
    report, scene_context, scene_cpu_fallback};
use anyhow::{Context, Result, bail};
use cs2_renderer::{BlendMode, Canvas, Rect, RgbaFrame};
use std::sync::{Arc, atomic::Ordering};

#[derive(Clone, Copy)]
pub(in crate) struct SceneVertex {
    pub point: [f64; 2],
    pub rhw: f64,
    pub uv_over_w: [f64; 2],
    pub color: [f64; 4],
}

pub(in crate) type SceneTriangle = [SceneVertex; 3];

pub(in crate) struct SceneRaster {
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
    parameters: wgpu::Buffer,
    triangles: wgpu::Buffer,
    tiles: wgpu::Buffer,
    indices: wgpu::Buffer,
}

impl SceneRaster {
    fn new(device: &wgpu::Device) -> Result<Self> {
        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("CS2 ordered scene triangles"),
            source: wgpu::ShaderSource::Wgsl(include_str!("gpu_scene_triangles.wgsl").into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("CS2 ordered scene triangles"), layout: None, module: &shader,
            entry_point: Some("scene_triangles"), compilation_options: Default::default(), cache: None,
        });
        if let Some(error) = pollster::block_on(device.pop_error_scope()) {
            bail!("compile scene triangle shader: {error}");
        }
        let layout = pipeline.get_bind_group_layout(0);
        Ok(Self { pipeline, layout,
            parameters: buffer(device, 48, wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST),
            triangles: buffer(device, 160, UPLOAD), tiles: buffer(device, 8, UPLOAD),
            indices: buffer(device, 4, UPLOAD) })
    }
}

/// False explicitly preserves the software reference for unsupported numeric
/// ranges, disabled direct output, device generations and hardware limits.
pub(in crate) fn draw_scene_triangles(
    canvas: &mut Canvas, triangles: &[SceneTriangle], source: Option<&RgbaFrame>,
    mode: BlendMode, clip: Option<Rect>, half_centers: bool, flip_v: bool,
) -> bool {
    if triangles.is_empty() { return true; }
    RASTER.with_borrow_mut(|slot| {
        let Some(raster) = slot.as_mut() else {
            scene_cpu_fallback("raster_unavailable");
            return false;
        };
        match raster.draw_scene_triangles(canvas, triangles, source, mode, clip, half_centers, flip_v) {
            Ok(()) => true,
            Err(error) => {
                scene_cpu_fallback(&format!("{error:#}"));
                false
            }
        }
    })
}

struct Prepared {
    region: RasterRect,
    triangles: Vec<u8>,
    tiles: Vec<u8>,
    indices: Vec<u8>,
}

fn edge(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> f64 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

fn words(output: &mut Vec<u8>, input: impl IntoIterator<Item = u32>) {
    output.extend(input.into_iter().flat_map(u32::to_le_bytes));
}

fn prepare(size: [u32; 2], input: &[SceneTriangle], clip: Option<Rect>, half: bool, textured: bool) -> Result<Option<Prepared>> {
    let mut limits = [0i64, 0, i64::from(size[0]), i64::from(size[1])];
    if let Some(clip) = clip {
        limits[0] = limits[0].max(i64::from(clip.x));
        limits[1] = limits[1].max(i64::from(clip.y));
        limits[2] = limits[2].min(i64::from(clip.x) + i64::from(clip.width));
        limits[3] = limits[3].min(i64::from(clip.y) + i64::from(clip.height));
    }
    if limits[0] >= limits[2] || limits[1] >= limits[3] { return Ok(None); }
    if input.len() > 65_536 { bail!("scene_batch_limit"); }
    let mut triangles = Vec::new();
    let mut bounds = Vec::new();
    let mut region = None::<RasterRect>;
    for &vertices in input {
        let mut vertices = vertices;
        for vertex in &vertices {
            if vertex.point.iter().chain(&vertex.uv_over_w).chain(&vertex.color)
                .chain(std::iter::once(&vertex.rhw)).any(|value| !value.is_finite()) {
                bail!("non_finite_vertex");
            }
            // The shader uses portable f32 arithmetic. Extreme projections,
            // subnormal RHW and large UV cancellation keep the f64 path.
            if vertex.point.iter().any(|value| value.abs() > 32768.)
                || textured && (vertex.rhw.abs() < 1e-12 || vertex.rhw.abs() > 1e12
                    || vertex.uv_over_w.iter().any(|value| (value / vertex.rhw).abs() > 16384.)) {
                bail!("vertex_precision_range");
            }
        }
        if textured && vertices.iter().any(|vertex| vertex.rhw.is_sign_positive() != vertices[0].rhw.is_sign_positive()) {
            bail!("mixed_reciprocal_w_sign");
        }
        let area = edge(vertices[0].point, vertices[1].point, vertices[2].point);
        if area == 0. { continue; }
        if area.abs() < 1e-4 { bail!("triangle_precision_area"); }
        if area < 0. { vertices.swap(1, 2); }
        // Depth clipping introduces f64 intersections. A valid software
        // triangle may collapse or reverse when its coordinates are uploaded.
        let [a, b, c] = vertices.map(|vertex| vertex.point.map(|value| value as f32));
        let device_area = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
        if !device_area.is_finite() || device_area <= 0. { bail!("triangle_quantization_precision"); }
        let lower: [i64; 2] = std::array::from_fn(|axis| {
            let value = vertices.iter().map(|vertex| vertex.point[axis]).fold(f64::INFINITY, f64::min);
            ((if half { value.floor() } else { value.ceil() }) as i64).max(limits[axis])
        });
        let upper: [i64; 2] = std::array::from_fn(|axis| {
            (vertices.iter().map(|vertex| vertex.point[axis]).fold(f64::NEG_INFINITY, f64::max).ceil() as i64)
                .min(limits[axis + 2])
        });
        if (0..2).any(|axis| lower[axis] >= upper[axis]) { continue; }
        // A positive total area is insufficient: long edges can subtract two
        // ~1e8 products and turn a genuinely positive unit edge into zero on
        // devices that do not contract multiply/subtract. Reserve low bits of
        // f32's 24-bit significand before subtraction. Bound each edge term
        // over the *visible* sample rectangle; normal 1280x720 and note quads
        // stay below this conservative range without scanning any pixels.
        const EDGE_PRODUCT_LIMIT: f64 = (1u64 << 22) as f64;
        // Normalize by triangle area as well: scaling a long thin triangle
        // down must not evade the guard. Limiting this condition number to
        // 2^10 retains fourteen significand bits for ordinary interpolation.
        const EDGE_CONDITION_LIMIT: f64 = (1u64 << 10) as f64;
        let center = if half { 0.5 } else { 0. };
        for index in 0..3 {
            let a = vertices[index].point;
            let b = vertices[(index + 1) % 3].point;
            let x = [lower[0] as f64 + center, (upper[0] - 1) as f64 + center];
            let y = [lower[1] as f64 + center, (upper[1] - 1) as f64 + center];
            let x_term = y.map(|y| ((b[0] - a[0]) * (y - a[1])).abs());
            let y_term = x.map(|x| ((b[1] - a[1]) * (x - a[0])).abs());
            if x_term.into_iter().chain(y_term).any(|term|
                term > EDGE_PRODUCT_LIMIT || term / area.abs() > EDGE_CONDITION_LIMIT) {
                bail!("triangle_edge_product_precision");
            }
        }
        let rect = RasterRect { origin: lower.map(|value| value as u32),
            size: std::array::from_fn(|axis| (upper[axis] - lower[axis]) as u32) };
        region = Some(region.map_or(rect, |region| region.union(rect)));
        for vertex in vertices {
            let values = [vertex.point[0], vertex.point[1], vertex.rhw, 0.,
                vertex.uv_over_w[0], vertex.uv_over_w[1], 0., 0.,
                vertex.color[0], vertex.color[1], vertex.color[2], vertex.color[3]];
            words(&mut triangles, values.map(|value| (value as f32).to_bits()));
        }
        words(&mut triangles, [rect.origin[0], rect.origin[1], rect.size[0], rect.size[1]]);
        bounds.push(rect);
    }
    let Some(region) = region else { return Ok(None); };
    let columns = region.size[0].div_ceil(8);
    let rows = region.size[1].div_ceil(8);
    let tile_count = u64::from(columns) * u64::from(rows);
    if tile_count > 1_048_576 || bounds.len() > 65_536 { bail!("scene_batch_limit"); }
    let mut counts = vec![0u32; tile_count as usize];
    let mut entries = 0usize;
    let tile_ranges: Vec<_> = bounds.iter().map(|rect| {
        let start: [u32; 2] = std::array::from_fn(|axis| (rect.origin[axis] - region.origin[axis]) / 8);
        let end: [u32; 2] = std::array::from_fn(|axis| {
            (rect.origin[axis] + rect.size[axis] - region.origin[axis]).div_ceil(8)
        });
        (start, end)
    }).collect();
    for &(start, end) in &tile_ranges {
        entries += ((end[0] - start[0]) * (end[1] - start[1])) as usize;
        if entries > 4 * 1024 * 1024 { bail!("scene_tile_index_limit"); }
        for y in start[1]..end[1] {
            for x in start[0]..end[0] { counts[(y * columns + x) as usize] += 1; }
        }
    }
    let mut tiles = Vec::with_capacity(tile_count as usize * 8);
    let mut cursors = Vec::with_capacity(tile_count as usize);
    let mut offset = 0u32;
    for count in counts {
        words(&mut tiles, [offset, count]);
        cursors.push(offset);
        offset += count;
    }
    let mut packed_indices = vec![0u32; entries];
    for (index, &(start, end)) in tile_ranges.iter().enumerate() {
        for y in start[1]..end[1] {
            for x in start[0]..end[0] {
                let cursor = &mut cursors[(y * columns + x) as usize];
                packed_indices[*cursor as usize] = index as u32;
                *cursor += 1;
            }
        }
    }
    let mut indices = Vec::with_capacity(entries * 4);
    words(&mut indices, packed_indices);
    Ok(Some(Prepared { region, triangles, tiles, indices }))
}

impl Raster {
    #[allow(clippy::too_many_arguments)]
    fn draw_scene_triangles(
        &mut self, canvas: &mut Canvas, triangles: &[SceneTriangle], source: Option<&RgbaFrame>,
        mode: BlendMode, clip: Option<Rect>, half_centers: bool, flip_v: bool,
    ) -> Result<()> {
        if self.failed.load(Ordering::Relaxed) { bail!("raster_device_failed"); }
        if !self.direct_output_enabled() { bail!("direct_output_disabled"); }
        let size = [canvas.width(), canvas.height()];
        let Some(prepared) = prepare(size, triangles, clip, half_centers, source.is_some())? else { return Ok(()) };
        let target_bytes = u64::from(size[0]) * u64::from(size[1]) * 4;
        let limits = self.device.limits();
        let storage_limit = u64::from(limits.max_storage_buffer_binding_size).min(limits.max_buffer_size);
        if [target_bytes, prepared.triangles.len() as u64, prepared.tiles.len() as u64,
            prepared.indices.len() as u64].iter().any(|bytes| *bytes > storage_limit)
            || prepared.region.size.iter().any(|size| size.div_ceil(8) > limits.max_compute_workgroups_per_dimension)
            || source.is_some_and(|frame| frame.is_empty() || !self.accepts_texture(frame)) {
            bail!("scene_dispatch_or_texture_limits");
        }
        if let Some(previous) = device_frame(canvas) && !Arc::ptr_eq(&previous.failed, &self.failed) {
            bail!("foreign_device_generation");
        }
        if self.scene_triangles.is_none() { self.scene_triangles = Some(SceneRaster::new(&self.device)?); }
        let source_view = self.source_texture(source);
        self.full_target = None;
        self.previous_planes = None;
        let previous = device_frame(canvas).map(|frame| frame.buffer.clone());
        self.ensure_target_size(target_bytes);
        let mut encoder = self.device.create_command_encoder(&Default::default());
        if let Some(previous) = previous {
            encoder.copy_buffer_to_buffer(&previous, 0, &self.target, 0, target_bytes);
        } else {
            self.queue.write_buffer(&self.target, 0, canvas.pixels());
        }
        let scene = self.scene_triangles.as_mut().context("scene raster missing after initialization")?;
        for (buffer_slot, bytes) in [(&mut scene.triangles, &prepared.triangles),
            (&mut scene.tiles, &prepared.tiles), (&mut scene.indices, &prepared.indices)] {
            if buffer_slot.size() < bytes.len() as u64 {
                *buffer_slot = buffer(&self.device, bytes.len() as u64, UPLOAD);
            }
            self.queue.write_buffer(buffer_slot, 0, bytes);
        }
        let params = [prepared.region.origin[0], prepared.region.origin[1],
            prepared.region.size[0], prepared.region.size[1], size[0], blend_code(mode),
            u32::from(half_centers), u32::from(flip_v), u32::from(source.is_some()),
            prepared.region.size[0].div_ceil(8), 0, 0];
        let mut parameters = Vec::with_capacity(48);
        words(&mut parameters, params);
        self.queue.write_buffer(&scene.parameters, 0, &parameters);
        let bindings = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("CS2 ordered scene triangle buffers"), layout: &scene.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.target.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&source_view) },
                wgpu::BindGroupEntry { binding: 2, resource: scene.triangles.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: scene.parameters.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 4, resource: scene.tiles.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 5, resource: scene.indices.as_entire_binding() },
            ],
        });
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&scene.pipeline);
            pass.set_bind_group(0, &bindings, &[]);
            pass.dispatch_workgroups(prepared.region.size[0].div_ceil(8), prepared.region.size[1].div_ceil(8), 1);
        }
        self.queue.submit(Some(encoder.finish()));
        canvas.set_deferred_device_pixels(self.publish_device_frame(size));
        static REPORTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if !REPORTED.swap(true, Ordering::Relaxed) {
            report(&format!("draw=scene_triangles backend={} {} triangles={} tile_indices={}",
                self.backend, scene_context(), prepared.triangles.len() / 160, prepared.indices.len() / 4));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f64_clipped_geometry_that_collapses_on_device_uses_precision_fallback() {
        let vertex = |point| SceneVertex { point, rhw: 1., uv_over_w: [0.; 2], color: [255.; 4] };
        // Covers the half-center (32000.5, 32000.5) in f64, but all three X
        // values round to 32000.5 in f32 and device coverage disappears.
        let triangle = [vertex([32000.4999, 32000.]), vertex([32000.5003, 32000.]),
            vertex([32000.4999, 32001.])];
        assert!(prepare([32768; 2], &[triangle], None, true, false).is_err(),
            "f32-degenerate clipped geometry must retain the f64 software path");
    }

    #[test]
    fn large_cancelling_edge_products_keep_the_software_precision_path() {
        let vertex = |point| SceneVertex { point, rhw: 1., uv_over_w: [0.; 2], color: [255.; 4] };
        let triangle = [vertex([-9500., -9500.]), vertex([500., 501.]), vertex([501., 503.])];
        assert_eq!(edge(triangle[0].point, triangle[1].point, triangle[2].point), 9999.);
        assert!(prepare([1280, 720], &[triangle], None, false, false).is_err(),
            "positive total area does not guarantee portable f32 edge coverage");
    }

    #[test]
    fn scaled_cancelling_edges_keep_the_same_precision_fallback() {
        let vertex = |point| SceneVertex { point, rhw: 1., uv_over_w: [0.; 2], color: [255.; 4] };
        let triangle = [vertex([-1187.875, -1188.]), vertex([62.125, 62.125]), vertex([62.25, 62.375])];
        assert_eq!(edge(triangle[0].point, triangle[1].point, triangle[2].point), 156.234375);
        assert!(prepare([1280, 720], &[triangle], None, false, false).is_err(),
            "uniform scaling must not bypass the edge-conditioning fallback");
    }
}

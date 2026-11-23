//! Ordered submission for particle triangle strips. Script geometry, RHW and UV
//! remain distinct until rasterization; textures use bottom-origin UVs.

use cs2_renderer::{BlendMode, Canvas, Color, RgbaFrame};
use super::particle_vm::{ParticleProjection, ParticleRenderSnapshot, ParticleVertex};

pub(super) fn draw(
    canvas: &mut Canvas,
    snapshot: &ParticleRenderSnapshot,
    texture: Option<&RgbaFrame>,
) -> anyhow::Result<()> {
    draw_range(canvas, snapshot, texture, f32::NEG_INFINITY, f32::INFINITY)
}

pub(super) fn draw_range(
    canvas: &mut Canvas,
    snapshot: &ParticleRenderSnapshot,
    texture: Option<&RgbaFrame>,
    lower: f32,
    upper: f32,
) -> anyhow::Result<()> {
    draw_range_with_projection(canvas, snapshot, texture, &ParticleProjection::default(), lower, upper)
}

pub(super) fn draw_range_with_projection(
    canvas: &mut Canvas, snapshot: &ParticleRenderSnapshot,
    texture: Option<&RgbaFrame>, view: &ParticleProjection,
    lower: f32, upper: f32,
) -> anyhow::Result<()> {
    #[cfg(all(feature = "gpu-raster", any(windows, target_os = "android")))]
    let _renderer = super::gpu_raster::scene_renderer_scope("particle_triangles");
    if !snapshot.frame_complete { return Ok(()); }
    // The scene range selects the registered owner callback, before that
    // callback projects any of its vertices (Yuki1d0ca8..1d0e08). The loader
    // registers record+108 as that owner's base Z (14ec3c..14ec6d). Projected
    // vertex Z is normalized by65000 and belongs to the rasterizer; comparing
    // it with these scene priorities incorrectly drops568/effect slices.
    let depth = snapshot.canvas.depth_offset;
    if depth < lower || depth >= upper { return Ok(()); }
    let mode = BlendMode::from_raw(snapshot.render_mode);
    let triangles: Vec<_> = snapshot.projected_quads(view)?.into_iter().flat_map(|quad| [
        [quad.vertices[0], quad.vertices[1], quad.vertices[2]],
        [quad.vertices[2], quad.vertices[1], quad.vertices[3]],
    ]).collect();
    #[cfg(all(feature = "gpu-raster", any(windows, target_os = "android")))]
    if try_gpu_triangles(canvas, &triangles, texture, mode) { return Ok(()); }
    for vertices in triangles {
        triangle_cpu(canvas, vertices, texture, mode)?;
    }
    Ok(())
}

fn edge(a: [f32; 3], b: [f32; 3], x: f64, y: f64) -> f64 {
    (f64::from(b[0]) - f64::from(a[0])) * (y - f64::from(a[1]))
        - (f64::from(b[1]) - f64::from(a[1])) * (x - f64::from(a[0]))
}

fn top_left(a: [f32; 3], b: [f32; 3]) -> bool {
    b[1] < a[1] || (b[1] == a[1] && b[0] > a[0])
}

pub(super) fn triangle(
    canvas: &mut Canvas,
    vertices: [ParticleVertex; 3],
    texture: Option<&RgbaFrame>,
    mode: BlendMode,
) -> anyhow::Result<()> {
    #[cfg(all(feature = "gpu-raster", any(windows, target_os = "android")))]
    {
        let _renderer = super::gpu_raster::scene_renderer_scope("particle_triangles");
        if try_gpu_triangles(canvas, &[vertices], texture, mode) { return Ok(()); }
        return triangle_cpu(canvas, vertices, texture, mode);
    }
    #[cfg(not(all(feature = "gpu-raster", any(windows, target_os = "android"))))]
    triangle_cpu(canvas, vertices, texture, mode)
}

#[cfg(all(feature = "gpu-raster", any(windows, target_os = "android")))]
fn try_gpu_triangles(
    canvas: &mut Canvas, triangles: &[[ParticleVertex; 3]], texture: Option<&RgbaFrame>, mode: BlendMode,
) -> bool {
    use super::gpu_raster::{SceneTriangle, SceneVertex};
    let geometry: Vec<SceneTriangle> = triangles.iter().map(|vertices| vertices.map(|vertex| SceneVertex {
        point: [f64::from(vertex.position[0]), f64::from(vertex.position[1])],
        rhw: f64::from(vertex.rhw),
        uv_over_w: vertex.uv.map(|value| f64::from(value) * f64::from(vertex.rhw)),
        color: [(vertex.color >> 16) as u8, (vertex.color >> 8) as u8,
            vertex.color as u8, (vertex.color >> 24) as u8].map(f64::from),
    })).collect();
    // Z is not a shader interpolant but remains part of the submission contract.
    if triangles.iter().flatten().any(|vertex| !vertex.position[2].is_finite()) {
        super::gpu_raster::scene_cpu_fallback("non_finite_particle_depth");
        return false;
    }
    super::gpu_raster::draw_scene_triangles(canvas, &geometry, texture, mode, None, false, true)
}

fn triangle_cpu(
    canvas: &mut Canvas, mut vertices: [ParticleVertex; 3], texture: Option<&RgbaFrame>, mode: BlendMode,
) -> anyhow::Result<()> {
    if vertices.iter().any(|vertex| {
        vertex.position.iter().chain(vertex.uv.iter()).any(|value| !value.is_finite())
            || !vertex.rhw.is_finite()
    }) {
        anyhow::bail!("particle triangle contains non-finite coordinates");
    }
    let mut area = edge(vertices[0].position, vertices[1].position,
        f64::from(vertices[2].position[0]), f64::from(vertices[2].position[1]));
    if area == 0.0 { return Ok(()); }
    if area < 0.0 {
        vertices.swap(1, 2);
        area = -area;
    }
    let bounds = |axis: usize, limit: u32| {
        let lower = vertices.iter().map(|vertex| vertex.position[axis])
            .fold(f32::INFINITY, f32::min).ceil().max(0.0).min(limit as f32) as i32;
        let upper = vertices.iter().map(|vertex| vertex.position[axis])
            .fold(f32::NEG_INFINITY, f32::max).ceil().max(0.0).min(limit as f32) as i32;
        (lower, upper)
    };
    let (left, right) = bounds(0, canvas.width());
    let (top, bottom) = bounds(1, canvas.height());
    let inclusions = [
        top_left(vertices[1].position, vertices[2].position),
        top_left(vertices[2].position, vertices[0].position),
        top_left(vertices[0].position, vertices[1].position),
    ];
    for y in top..bottom {
        for x in left..right {
            // Pretransformed D3D9 coordinates sample integer pixel centers.
            let edges = [
                edge(vertices[1].position, vertices[2].position, f64::from(x), f64::from(y)),
                edge(vertices[2].position, vertices[0].position, f64::from(x), f64::from(y)),
                edge(vertices[0].position, vertices[1].position, f64::from(x), f64::from(y)),
            ];
            if (0..3).any(|index| edges[index] < 0.0 ||
                (edges[index] == 0.0 && !inclusions[index])) { continue; }
            let weight = edges.map(|value| value / area);
            let mut rgba = [0.0; 4];
            for (index, vertex) in vertices.iter().enumerate() {
                let channels = [(vertex.color >> 16) as u8, (vertex.color >> 8) as u8,
                    vertex.color as u8, (vertex.color >> 24) as u8];
                for channel in 0..4 {
                    rgba[channel] += weight[index] * f64::from(channels[channel]);
                }
            }
            if let Some(texture) = texture {
                let reciprocal = (0..3).map(|index|
                    weight[index] * f64::from(vertices[index].rhw)).sum::<f64>();
                if reciprocal == 0.0 { continue; }
                let uv = [0, 1].map(|axis| (0..3).map(|index|
                    weight[index] * f64::from(vertices[index].rhw) *
                    f64::from(vertices[index].uv[axis])).sum::<f64>() / reciprocal);
                let texel = sample(texture, uv);
                for channel in 0..4 { rgba[channel] *= texel[channel] / 255.0; }
            }
            canvas.blend_pixel_with_mode(x, y, Color::rgba(
                rgba[0].round().clamp(0.0, 255.0) as u8,
                rgba[1].round().clamp(0.0, 255.0) as u8,
                rgba[2].round().clamp(0.0, 255.0) as u8,
                rgba[3].round().clamp(0.0, 255.0) as u8), mode)?;
        }
    }
    Ok(())
}

fn sample(texture: &RgbaFrame, uv: [f64; 2]) -> [f64; 4] {
    let [width, height] = [texture.width(), texture.height()];
    if width == 0 || height == 0 { return [0.0; 4]; }
    let x = (uv[0] * f64::from(width) - 0.5).clamp(0.0, f64::from(width - 1));
    let y = ((1.0 - uv[1]) * f64::from(height) - 0.5).clamp(0.0, f64::from(height - 1));
    let [x0, y0] = [x.floor() as u32, y.floor() as u32];
    let [x1, y1] = [(x0 + 1).min(width - 1), (y0 + 1).min(height - 1)];
    let [fx, fy] = [x - f64::from(x0), y - f64::from(y0)];
    let mut output = [0.0; 4];
    for (px, py, weight) in [(x0, y0, (1.0 - fx) * (1.0 - fy)),
        (x1, y0, fx * (1.0 - fy)), (x0, y1, (1.0 - fx) * fy), (x1, y1, fx * fy)] {
        let offset = (py as usize * width as usize + px as usize) * 4;
        for channel in 0..4 {
            output[channel] += weight * f64::from(texture.pixels()[offset + channel]);
        }
    }
    output
}

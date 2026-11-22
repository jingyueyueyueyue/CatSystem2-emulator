//! Independently implemented CPU equivalents of the fixed-function work passes.
//! Coordinates remain in the original pretransformed pixel convention (-0.5
//! at the upper left). Sampling uses the auxiliary power-of-two texture size.
use anyhow::{Context, Result, bail};
use cs2_renderer::{Canvas, ImageDrawOptions, Point, RgbaFrame};

use super::MAX_WORK_BYTES;

const MAX_EXACT_QUAD_AREA: u32 = 1 << 23;

#[derive(Clone, Copy)]
struct Vertex {
    xy: [f32; 2],
    uv: [f32; 2],
    z: f32,
    rhw: f32,
}

pub(super) fn validate_configuration(kind: u32, words: &[u32; 22]) -> Result<()> {
    let depth = match kind {
        35 => 20,
        36 => 6,
        37 => 1,
        _ => unreachable!(),
    };
    let lower = f32::from_bits(words[depth]);
    let upper = f32::from_bits(words[depth + 1]);
    if !lower.is_finite() || !upper.is_finite() {
        bail!("render depth interval contains a non-finite value");
    }
    let float_count = match kind {
        35 => 19,
        36 => 8,
        _ => 0,
    };
    if words[..float_count]
        .iter()
        .any(|word| !f32::from_bits(*word).is_finite())
    {
        bail!("render sampling configuration contains a non-finite value");
    }
    Ok(())
}

/// 004d25ea..004d2ab6: render a shrunken quad to a separate auxiliary
/// texture, then use that quad's shrunken XY as the second pass's UV.
pub(super) fn mosaic(
    target: &mut Canvas,
    source: &RgbaFrame,
    words: &[u32; 22],
    rectangle: [u32; 4],
) -> Result<RgbaFrame> {
    let [width, height] = [source.width(), source.height()];
    let texture = texture_size(source)?;
    let [x, y, w, h] = rectangle.map(f32::from_bits);
    let divisor = [f32::from_bits(words[4]), f32::from_bits(words[5])];
    let original = quad(
        [x - 0.5, y - 0.5, x + w - 0.5, y + h - 0.5],
        [
            0.,
            0.,
            (x + w) / texture[0] as f32,
            (y + h) / texture[1] as f32,
        ],
    );
    let mut small = original;
    for vertex in &mut small {
        for axis in 0..2 {
            vertex.xy[axis] = if divisor[axis] == 0. {
                0.
            } else {
                vertex.xy[axis] / divisor[axis]
            };
        }
    }
    let mut auxiliary = Canvas::new(width, height)?;
    draw_quad(&mut auxiliary, source, texture, small, 0xffff_ffff, false)?;
    let auxiliary = auxiliary.into_frame();
    let mut enlarged = original;
    for (vertex, small) in enlarged.iter_mut().zip(small) {
        vertex.uv = [
            small.xy[0] / texture[0] as f32,
            small.xy[1] / texture[1] as f32,
        ];
    }
    draw_quad(target, &auxiliary, texture, enlarged, words[8], false)?;
    Ok(auxiliary)
}

/// 004d1d17..004d1e9e: current source plus the transformed previous auxiliary
/// surface, with the first callback after every active command skipping the
/// feedback pass. This is temporal image feedback, rather than a box filter.
pub(super) fn blur(
    source: &RgbaFrame,
    previous: Option<&RgbaFrame>,
    words: &[u32; 22],
    fresh: bool,
) -> Result<RgbaFrame> {
    let mut output = Canvas::new(source.width(), source.height())?;
    output.copy_frame(source, &ImageDrawOptions::new(Point::new(0, 0)))?;
    if !fresh {
        let previous = previous.context("Blur previous auxiliary surface is absent")?;
        let texture = texture_size(previous)?;
        let mut vertices = full_quad(source, texture);
        // The original center uses unsigned integer halves, then negates.
        let center = [
            -((source.width() >> 1) as f32),
            -((source.height() >> 1) as f32),
        ];
        let matrix: [f32; 16] = std::array::from_fn(|index| f32::from_bits(words[index]));
        for vertex in &mut vertices {
            let input = [vertex.xy[0] + center[0], vertex.xy[1] + center[1], 0.5, 1.];
            let mut transformed = [0.; 4];
            // D3DX row-vector layout, used by the recovered Vec4 transform.
            for column in 0..4 {
                transformed[column] = ((input[0] * matrix[column] + input[1] * matrix[4 + column])
                    + input[2] * matrix[8 + column])
                    + input[3] * matrix[12 + column];
            }
            vertex.xy = [
                transformed[0] + f32::from_bits(words[16]) - center[0],
                transformed[1] + f32::from_bits(words[17]) - center[1],
            ];
            vertex.z = transformed[2] + f32::from_bits(words[18]);
            vertex.rhw = transformed[3];
        }
        draw_quad(&mut output, previous, texture, vertices, words[19], false)?;
    }
    Ok(output.into_frame())
}

/// 004d2cc0/004d35b0: 49 full-screen translated quads, ordered by the
/// recovered square-ring index. Every quad has one constant radial alpha;
/// these are not per-pixel grayscale or radial-mask operations.
pub(super) fn shade(target: &mut Canvas, source: &RgbaFrame, radius: Option<i32>) -> Result<()> {
    let texture = texture_size(source)?;
    let taps = shade_taps(radius);
    if exact_unit_quad(source) {
        draw_unit_spans(target, source, &taps);
        return Ok(());
    }
    shade_quads(target, source, texture, &taps)
}

fn shade_taps(radius: Option<i32>) -> Vec<([i32; 2], u8)> {
    const OFFSETS: [(i32, i32, i32); 49] = shade_offsets();
    OFFSETS
        .into_iter()
        .filter_map(|(_, x, y)| {
            let alpha = if let Some(radius) = radius {
                // The binary calculates distance from the first quad vertex,
                // including the pretransformed half-pixel offset.
                let px = x as f32 - 0.5;
                let py = y as f32 - 0.5;
                let squared = px * px + py * py;
                let distance = (f64::from(squared).sqrt() as f32).min(radius as f32);
                let value = ((radius as f32 - distance) * 255.) / radius as f32;
                // x87's indefinite integer on NaN/overflow has low byte zero.
                if !value.is_finite() {
                    0
                } else {
                    (value.trunc() as i32) as u8
                }
            } else {
                255
            };
            (alpha != 0).then_some(([x, y], alpha))
        })
        .collect()
}

const fn shade_offsets() -> [(i32, i32, i32); 49] {
    let mut offsets = [(0, 0, 0); 49];
    let mut i = 0;
    while i < offsets.len() {
        let x = i as i32 / 7 - 3;
        let y = i as i32 % 7 - 3;
        let index = ring_index(x, y);
        let mut j = i;
        // Iteration is already ordered by X and Y for equal ring indices.
        while j > 0 && offsets[j - 1].0 > index {
            offsets[j] = offsets[j - 1];
            j -= 1;
        }
        offsets[j] = (index, x, y);
        i += 1;
    }
    offsets
}

fn shade_quads(
    target: &mut Canvas,
    source: &RgbaFrame,
    texture: [u32; 2],
    taps: &[([i32; 2], u8)],
) -> Result<()> {
    for &([x, y], alpha) in taps {
        let mut vertices = full_quad(source, texture);
        for vertex in &mut vertices {
            vertex.xy[0] += x as f32;
            vertex.xy[1] += y as f32;
        }
        draw_quad(
            target,
            source,
            texture,
            vertices,
            u32::from(alpha) << 24 | 0x00ff_ffff,
            false,
        )?;
    }
    Ok(())
}

fn exact_unit_quad(source: &RgbaFrame) -> bool {
    let (width, height) = source.dimensions();
    (1..=4096).contains(&width)
        && (1..=4096).contains(&height)
        && width * height <= MAX_EXACT_QUAD_AREA
}

/// Integer-aligned translations of the logical source rectangle. Each row
/// sees the original draw order and stays hot across all Shade taps.
fn draw_unit_spans(target: &mut Canvas, source: &RgbaFrame, taps: &[([i32; 2], u8)]) {
    if taps.is_empty() || target.width() == 0 || target.height() == 0 {
        return;
    }
    #[cfg(all(feature = "gpu-raster", any(windows, target_os = "android")))]
    {
        use crate::gpu_raster::{Draw, RasterRect, draw};
        let extent = std::array::from_fn::<_, 2, _>(|axis| {
            taps.iter()
                .map(|(offset, _)| offset[axis])
                .max()
                .unwrap_or(0)
                .max(0) as u32
        });
        let size = [
            target.width().min(source.width() + extent[0]),
            target.height().min(source.height() + extent[1]),
        ];
        if draw(
            target,
            Some(source),
            RasterRect {
                origin: [0; 2],
                size,
            },
            Draw::EffectSpans { taps },
        ) {
            return;
        }
    }
    let width = target.width() as usize;
    let (source_width, source_height) = source.dimensions();
    let input = source.pixels();
    let stride = source_width as usize * 4;
    let spans: Vec<_> = taps
        .iter()
        .filter_map(|&([x, y], alpha)| {
            let left = x.max(0) as usize;
            let right = (x + source_width as i32).max(0).min(width as i32) as usize;
            (left < right).then(|| {
                (
                    left * 4..right * 4,
                    (left as i32 - x) as usize * 4,
                    y,
                    alpha,
                )
            })
        })
        .collect();
    let workers = effect_workers(
        source_width,
        source_height,
        u64::from(source_width) * u64::from(source_height) * spans.len() as u64,
    );
    cs2_renderer::draw_pixel_rows(target.pixels_mut(), width * 4, 0, workers, |row, y| {
        for (range, start, offset_y, alpha) in &spans {
            let sy = (y as i64 - i64::from(*offset_y)) as usize;
            if sy >= source_height as usize {
                continue;
            }
            let from = sy * stride + start;
            blend_span(
                &mut row[range.clone()],
                &input[from..from + range.len()],
                *alpha,
            );
        }
    });
}

pub(super) fn effect_workers(width: u32, height: u32, pixels: u64) -> usize {
    if width >= 128 && height >= 64 && pixels >= 512 * 1024 {
        std::thread::available_parallelism().map_or(1, |count| count.get().min(4))
    } else {
        1
    }
}

fn blend_span(target: &mut [u8], source: &[u8], opacity: u8) {
    let opacity = u32::from(opacity);
    for (out, src) in target
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(source.as_chunks::<4>().0)
    {
        let alpha = (u32::from(src[3]) * opacity + 127) / 255;
        match alpha {
            0 => {}
            255 => out.copy_from_slice(src),
            _ => {
                for channel in 0..3 {
                    out[channel] = blend_channel(src[channel].into(), out[channel], alpha);
                }
                out[3] = blend_channel(alpha, out[3], alpha);
            }
        }
    }
}

#[inline]
fn blend_channel(sample: u32, destination: u8, alpha: u32) -> u8 {
    ((sample * alpha + u32::from(destination) * (255 - alpha) + 127) / 255) as u8
}

pub(super) fn composite(
    target: &mut Canvas,
    source: &RgbaFrame,
    alpha: u8,
    diffuse_alpha_only: bool,
) -> Result<()> {
    let size = texture_size(source)?;
    if !diffuse_alpha_only && exact_unit_quad(source) {
        if alpha != 0 {
            draw_unit_spans(target, source, &[([0; 2], alpha)]);
        }
        return Ok(());
    }
    if diffuse_alpha_only
        && target.dimensions() == (source.width(), source.height())
        && exact_unit_quad(source)
    {
        #[cfg(all(feature = "gpu-raster", any(windows, target_os = "android")))]
        if alpha > 0 && (alpha < 255 || target.device_pixels().is_some()) {
            use crate::gpu_raster::{Draw, RasterRect, draw};
            if draw(
                target,
                Some(source),
                RasterRect {
                    origin: [0; 2],
                    size: [source.width(), source.height()],
                },
                Draw::DiffuseFrame { alpha },
            ) {
                return Ok(());
            }
        }
        composite_unit_diffuse(target, source, alpha);
        return Ok(());
    }
    draw_quad(
        target,
        source,
        size,
        full_quad(source, size),
        u32::from(alpha) << 24 | 0x00ff_ffff,
        diffuse_alpha_only,
    )
}

fn composite_unit_diffuse(target: &mut Canvas, source: &RgbaFrame, alpha: u8) {
    match alpha {
        0 => {}
        255 => {
            target.pixels_mut().copy_from_slice(source.pixels());
            for pixel in target.pixels_mut().chunks_exact_mut(4) {
                pixel[3] = 255;
            }
        }
        _ => {
            let factor = u16::from(alpha);
            let inverse = 255 - factor;
            for (destination, sample) in target
                .pixels_mut()
                .chunks_exact_mut(4)
                .zip(source.pixels().chunks_exact(4))
            {
                for channel in 0..3 {
                    destination[channel] = ((u16::from(sample[channel]) * factor
                        + u16::from(destination[channel]) * inverse
                        + 127)
                        / 255) as u8;
                }
                destination[3] =
                    ((factor * factor + u16::from(destination[3]) * inverse + 127) / 255) as u8;
            }
        }
    }
}

const fn ring_index(x: i32, y: i32) -> i32 {
    let r = if x.abs() >= y.abs() { x.abs() } else { y.abs() };
    if r == 0 {
        return 0;
    }
    let offset = if x.abs() == r {
        if y > 0 { 5 * r - y } else { r + y }
    } else if x > 0 {
        x + 3 * r
    } else {
        7 * r - x
    };
    1 + 4 * r * (r - 1) + offset
}

fn full_quad(source: &RgbaFrame, texture: [u32; 2]) -> [Vertex; 4] {
    quad(
        [
            -0.5,
            -0.5,
            source.width() as f32 - 0.5,
            source.height() as f32 - 0.5,
        ],
        [
            0.,
            0.,
            source.width() as f32 / texture[0] as f32,
            source.height() as f32 / texture[1] as f32,
        ],
    )
}

fn quad([left, top, right, bottom]: [f32; 4], [u0, v0, u1, v1]: [f32; 4]) -> [Vertex; 4] {
    [
        Vertex {
            xy: [left, top],
            uv: [u0, v0],
            z: 0.5,
            rhw: 1.,
        },
        Vertex {
            xy: [right, top],
            uv: [u1, v0],
            z: 0.5,
            rhw: 1.,
        },
        Vertex {
            xy: [left, bottom],
            uv: [u0, v1],
            z: 0.5,
            rhw: 1.,
        },
        Vertex {
            xy: [right, bottom],
            uv: [u1, v1],
            z: 0.5,
            rhw: 1.,
        },
    ]
}

fn texture_size(source: &RgbaFrame) -> Result<[u32; 2]> {
    let width = source
        .width()
        .checked_next_power_of_two()
        .context("render texture width overflow")?;
    let height = source
        .height()
        .checked_next_power_of_two()
        .context("render texture height overflow")?;
    if u64::from(width) * u64::from(height) * 4 > MAX_WORK_BYTES as u64 {
        bail!("render auxiliary texture exceeds 128 MiB");
    }
    Ok([width, height])
}

fn draw_quad(
    target: &mut Canvas,
    texture: &RgbaFrame,
    texture_size: [u32; 2],
    vertices: [Vertex; 4],
    argb: u32,
    diffuse_alpha_only: bool,
) -> Result<()> {
    for vertex in &vertices {
        if vertex.xy.iter().chain(&vertex.uv).any(|v| !v.is_finite())
            || !vertex.z.is_finite()
            || !vertex.rhw.is_finite()
        {
            bail!("render transformed quad contains non-finite coordinates");
        }
    }
    // The strip uses TL,TR,BL,BR, and the two triangles share only one edge.
    triangle(
        target,
        texture,
        texture_size,
        [vertices[0], vertices[1], vertices[2]],
        argb,
        diffuse_alpha_only,
    )?;
    triangle(
        target,
        texture,
        texture_size,
        [vertices[2], vertices[1], vertices[3]],
        argb,
        diffuse_alpha_only,
    )
}

fn triangle(
    target: &mut Canvas,
    texture: &RgbaFrame,
    texture_size: [u32; 2],
    mut vertices: [Vertex; 3],
    argb: u32,
    diffuse_alpha_only: bool,
) -> Result<()> {
    let mut area = edge(vertices[0].xy, vertices[1].xy, vertices[2].xy);
    if !area.is_finite() {
        bail!("render transformed triangle area overflow");
    }
    if area == 0. {
        return Ok(());
    }
    if area < 0. {
        vertices.swap(1, 2);
        area = -area;
    }
    let minimum = [0, 1].map(|axis| {
        vertices
            .iter()
            .map(|v| v.xy[axis])
            .fold(f32::INFINITY, f32::min)
    });
    let maximum = [0, 1].map(|axis| {
        vertices
            .iter()
            .map(|v| v.xy[axis])
            .fold(f32::NEG_INFINITY, f32::max)
    });
    let left = (minimum[0].ceil() as i32).max(0);
    let top = (minimum[1].ceil() as i32).max(0);
    let right = (maximum[0].floor() as i32).min(target.width() as i32 - 1);
    let bottom = (maximum[1].floor() as i32).min(target.height() as i32 - 1);
    if left > right || top > bottom {
        return Ok(());
    }
    let colors = [
        (argb >> 16) as u8,
        (argb >> 8) as u8,
        argb as u8,
        (argb >> 24) as u8,
    ];
    let edges = [0, 1, 2].map(|i| {
        let a = vertices[(i + 1) % 3].xy;
        let b = vertices[(i + 2) % 3].xy;
        (a, [b[0] - a[0], b[1] - a[1]], top_left(a, b))
    });
    let stride = target.width() as usize * 4;
    let pixels = target.pixels_mut();
    for y in top..=bottom {
        let row = &mut pixels[y as usize * stride..][..stride];
        let edges = edges
            .map(|(a, delta, inclusive)| (a[0], delta[0] * (y as f32 - a[1]), delta[1], inclusive));
        for x in left..=right {
            let e = edges.map(|(start, base, slope, _)| base - slope * (x as f32 - start));
            let inside = e
                .iter()
                .zip(edges)
                .all(|(&value, (_, _, _, inclusive))| value > 0. || (value == 0. && inclusive));
            if !inside {
                continue;
            }
            let bary = e.map(|v| v / area);
            let z = bary.iter().zip(vertices).map(|(b, v)| b * v.z).sum::<f32>();
            if !(0.0..=1.0).contains(&z) {
                continue;
            }
            let weights = [0, 1, 2].map(|i| bary[i] * vertices[i].rhw);
            let denominator = weights.iter().sum::<f32>();
            if denominator == 0. {
                bail!("render perspective interpolation has zero reciprocal W");
            }
            let uv = [0, 1].map(|axis| {
                weights
                    .iter()
                    .enumerate()
                    .map(|(i, w)| w * vertices[i].uv[axis])
                    .sum::<f32>()
                    / denominator
            });
            let mut sample = sample_point(texture, texture_size, uv)?;
            for channel in 0..4 {
                sample[channel] = if channel == 3 && diffuse_alpha_only {
                    colors[channel]
                } else {
                    ((u16::from(sample[channel]) * u16::from(colors[channel]) + 127) / 255) as u8
                };
            }
            // D3D's fixed-function source-alpha factors apply to all four
            // channels. Canvas's Porter-Duff alpha would incorrectly keep A=255.
            let index = x as usize * 4;
            let destination = &mut row[index..index + 4];
            let alpha = u32::from(sample[3]);
            for channel in 0..4 {
                destination[channel] = ((u32::from(sample[channel]) * alpha
                    + u32::from(destination[channel]) * (255 - alpha)
                    + 127)
                    / 255) as u8;
            }
        }
    }
    Ok(())
}

fn edge(a: [f32; 2], b: [f32; 2], p: [f32; 2]) -> f32 {
    (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0])
}
fn top_left(a: [f32; 2], b: [f32; 2]) -> bool {
    let dy = b[1] - a[1];
    dy < 0. || (dy == 0. && b[0] > a[0])
}
fn sample_point(texture: &RgbaFrame, size: [u32; 2], uv: [f32; 2]) -> Result<[u8; 4]> {
    if uv.iter().any(|v| !v.is_finite()) {
        bail!("render texture coordinate is non-finite");
    }
    let [x, y] = [0, 1].map(|axis| {
        let value = uv[axis];
        let extent = size[axis];
        if (0.0..1.0).contains(&value) {
            // The cast rounds toward zero and the extent is a power of two.
            (value * extent as f32) as u32
        } else {
            (value.rem_euclid(1.) * extent as f32).floor() as u32 % extent
        }
    });
    if x >= texture.width() || y >= texture.height() {
        return Ok([0; 4]);
    }
    let index = (y as usize * texture.width() as usize + x as usize) * 4;
    Ok(texture.pixels()[index..index + 4].try_into().unwrap())
}

/// Shared fixed-function triangle submission for retained frame-effect meshes.
/// Each vertex is screen X/Y, depth, reciprocal W, and normalized U/V.
pub(super) fn effect_triangle(
    target: &mut Canvas,
    texture: &RgbaFrame,
    vertices: [[f32; 6]; 3],
    argb: u32,
) -> Result<()> {
    effect_triangle_alpha(target, texture, vertices, argb, false)
}

/// WBreak selects D3DTA_DIFFUSE for alpha, independently of texture alpha.
/// Yukikoi55817c ->5c7200(1,0,0), V4014d9555 ->548c10(1,0,0).
pub(super) fn fragment_triangle(
    target: &mut Canvas,
    texture: &RgbaFrame,
    vertices: [[f32; 6]; 3],
    argb: u32,
) -> Result<()> {
    effect_triangle_alpha(target, texture, vertices, argb, true)
}

fn effect_triangle_alpha(
    target: &mut Canvas,
    texture: &RgbaFrame,
    vertices: [[f32; 6]; 3],
    argb: u32,
    diffuse_alpha_only: bool,
) -> Result<()> {
    if vertices.iter().flatten().any(|value| !value.is_finite()) {
        bail!("frame effect mesh contains a non-finite vertex");
    }
    triangle(
        target,
        texture,
        texture_size(texture)?,
        vertices.map(|vertex| Vertex {
            xy: [vertex[0], vertex[1]],
            z: vertex[2],
            rhw: vertex[3],
            uv: [vertex[4], vertex[5]],
        }),
        argb,
        diffuse_alpha_only,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(width: u32, height: u32, seed: u32) -> Vec<u8> {
        let mut state = seed;
        (0..width * height * 4)
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (state >> 24) as u8
            })
            .collect()
    }

    fn compare_composite(
        width: u32,
        height: u32,
        alpha: u8,
        diffuse: bool,
        destination: [u32; 2],
    ) -> Result<()> {
        let source = RgbaFrame::from_rgba8(width, height, bytes(width, height, 7))?;
        let [target_width, target_height] = destination;
        let mut baseline = Canvas::from_rgba8(
            target_width,
            target_height,
            bytes(target_width, target_height, 42),
        )?;
        let mut actual = baseline.clone();
        let texture = texture_size(&source)?;
        draw_quad(
            &mut baseline,
            &source,
            texture,
            full_quad(&source, texture),
            u32::from(alpha) << 24 | 0x00ff_ffff,
            diffuse,
        )?;
        composite(&mut actual, &source, alpha, diffuse)?;
        assert!(
            baseline.pixels() == actual.pixels(),
            "RGBA mismatch for {width}x{height} -> {destination:?}, alpha={alpha}, diffuse={diffuse}"
        );
        Ok(())
    }

    #[test]
    fn unit_diffuse_composite_matches_triangle_rgba() -> Result<()> {
        for alpha in 0..=255 {
            compare_composite(5, 7, alpha, true, [5, 7])?;
        }
        for alpha in [0, 1, 127, 254, 255] {
            compare_composite(1280, 720, alpha, true, [1280, 720])?;
        }
        compare_composite(4096, 1, 173, true, [4096, 1])?;
        compare_composite(1, 4096, 173, true, [1, 4096])?;
        Ok(())
    }

    #[test]
    fn retained_quad_fallback_matches_triangle_rgba() -> Result<()> {
        compare_composite(7, 5, 213, true, [4, 8])?;
        compare_composite(4097, 1, 213, true, [4097, 1])
    }

    #[test]
    fn texture_alpha_spans_match_triangle_rgba() -> Result<()> {
        for alpha in [0, 1, 127, 254, 255] {
            compare_composite(1280, 720, alpha, false, [1280, 720])?;
            compare_composite(7, 5, alpha, false, [4, 8])?;
        }
        Ok(())
    }

    #[test]
    fn ordered_shade_spans_match_triangle_rgba() -> Result<()> {
        let source = RgbaFrame::from_rgba8(1280, 720, bytes(1280, 720, 17))?;
        let texture = texture_size(&source)?;
        for radius in [None, Some(-1), Some(0), Some(2), Some(7)] {
            let mut expected = Canvas::from_rgba8(1277, 725, bytes(1277, 725, 9))?;
            let mut actual = expected.clone();
            shade_quads(&mut expected, &source, texture, &shade_taps(radius))?;
            shade(&mut actual, &source, radius)?;
            assert!(actual.pixels() == expected.pixels(), "radius={radius:?}");
        }
        Ok(())
    }

    #[cfg(all(feature = "gpu-raster", windows))]
    #[test]
    #[ignore = "requires a DX12 adapter"]
    fn gpu_effect_spans_match_triangle_rgba() -> Result<()> {
        use crate::gpu_raster::{Draw, RasterRect, available, draw, enable};
        enable();
        assert!(available());
        let source = RgbaFrame::from_rgba8(320, 240, bytes(320, 240, 17))?;
        let texture = texture_size(&source)?;
        let mut expected = Canvas::from_rgba8(319, 244, bytes(319, 244, 9))?;
        let mut actual = expected.clone();
        let taps = shade_taps(Some(7));
        shade_quads(&mut expected, &source, texture, &taps)?;
        assert!(draw(
            &mut actual,
            Some(&source),
            RasterRect {
                origin: [0; 2],
                size: [319, 243]
            },
            Draw::EffectSpans { taps: &taps },
        ));
        assert!(actual.pixels() == expected.pixels());
        Ok(())
    }
}

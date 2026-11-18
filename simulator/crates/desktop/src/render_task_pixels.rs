//! Independently implemented CPU equivalents of the fixed-function work passes.
//! Coordinates remain in the original pretransformed pixel convention (-0.5
//! at the upper left). Sampling uses the auxiliary power-of-two texture size.
use anyhow::{Context, Result, bail};
use cs2_renderer::{Canvas, Color, ImageDrawOptions, Point, RgbaFrame};

use super::{MAX_WORK_BYTES, snapshot};

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
    auxiliary.clear(Color::TRANSPARENT);
    draw_quad(&mut auxiliary, source, texture, small, 0xffff_ffff, false)?;
    let auxiliary = snapshot(&auxiliary)?;
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
    snapshot(&output)
}

/// 004d2cc0/004d35b0: 49 full-screen translated quads, ordered by the
/// recovered square-ring index. Every quad has one constant radial alpha;
/// these are not per-pixel grayscale or radial-mask operations.
pub(super) fn shade(target: &mut Canvas, source: &RgbaFrame, radius: Option<i32>) -> Result<()> {
    let texture = texture_size(source)?;
    let mut offsets = Vec::with_capacity(49);
    for y in -3i32..=3 {
        for x in -3i32..=3 {
            offsets.push((ring_index(x, y), x, y));
        }
    }
    offsets.sort_unstable();
    for (_, x, y) in offsets {
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
        if alpha == 0 {
            continue;
        }
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

pub(super) fn composite(
    target: &mut Canvas,
    source: &RgbaFrame,
    alpha: u8,
    diffuse_alpha_only: bool,
) -> Result<()> {
    let size = texture_size(source)?;
    draw_quad(
        target,
        source,
        size,
        full_quad(source, size),
        u32::from(alpha) << 24 | 0x00ff_ffff,
        diffuse_alpha_only,
    )
}

fn ring_index(x: i32, y: i32) -> i32 {
    let r = x.abs().max(y.abs());
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
    let colors = [
        (argb >> 16) as u8,
        (argb >> 8) as u8,
        argb as u8,
        (argb >> 24) as u8,
    ];
    for y in top..=bottom {
        for x in left..=right {
            let point = [x as f32, y as f32];
            let e = [
                edge(vertices[1].xy, vertices[2].xy, point),
                edge(vertices[2].xy, vertices[0].xy, point),
                edge(vertices[0].xy, vertices[1].xy, point),
            ];
            let inside = e.iter().enumerate().all(|(index, &value)| {
                value > 0.
                    || (value == 0.
                        && top_left(vertices[(index + 1) % 3].xy, vertices[(index + 2) % 3].xy))
            });
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
            let index = ((y as usize) * target.width() as usize + x as usize) * 4;
            let destination = &mut target.pixels_mut()[index..index + 4];
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
    let [x, y] = [0, 1]
        .map(|axis| (uv[axis].rem_euclid(1.) * size[axis] as f32).floor() as u32 % size[axis]);
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

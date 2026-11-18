//! Actual sprite/ring mesh generation. Independent format/behavior code;
//! rings are two radial bands per segment, never a substitute rectangle.

use super::{Curve3, Error, Vertex, curve::FCurve};

#[derive(Debug, Clone)]
pub(super) enum ColorCurve {
    Keys([FCurve; 4]),
    Fixed([u8; 4]),
    FixedHsva([u8; 4]),
    Ease {
        start: [u8; 4],
        end: [u8; 4],
        abc: [f32; 3],
        hsva: bool,
    },
}
impl ColorCurve {
    pub(super) fn value(&self, age: f32, life: f32) -> Result<[u8; 4], Error> {
        Ok(match self {
            Self::Keys(c) => [
                c[0].value(age)?,
                c[1].value(age)?,
                c[2].value(age)?,
                c[3].value(age)?,
            ]
            .map(|v| v.clamp(0.0, 255.0) as u8),
            Self::Fixed(v) => *v,
            Self::FixedHsva(v) => hsva_color(*v),
            Self::Ease {
                start,
                end,
                abc,
                hsva,
            } => {
                // PE ed903..ed936 / f0d5d..f0d91: Horner evaluation,
                // then f1af0 truncates the raw channels before conversion.
                let t = age / life;
                let d = ((abc[0] * t + abc[1]) * t + abc[2]) * t;
                if !d.is_finite() {
                    return Err(Error::Host("nonfinite SKFE color easing"));
                }
                let raw = std::array::from_fn(|i| {
                    (start[i] as f32 + (end[i] as f32 - start[i] as f32) * d).clamp(0.0, 255.0)
                        as u8
                });
                if *hsva { hsva_color(raw) } else { raw }
            }
        })
    }
}

fn hsva_color(raw: [u8; 4]) -> [u8; 4] {
    // PE ecab0..ecbed. These are the target's stored f32 reciprocals,
    // including their rounding; hue is a byte in the 0..252 color space.
    let h = raw[0].min(252) as u32;
    let sector = h / 42;
    let fraction = h as f32 * f32::from_bits(0x3cc30c31) - sector as f32;
    let saturation = raw[1] as f32 * f32::from_bits(0x3b808081);
    let value = raw[2] as f32;
    let p = ((1.0 - saturation) * value) as u8;
    let q = ((1.0 - saturation * fraction) * value) as u8;
    let t = ((1.0 - (1.0 - fraction) * saturation) * value) as u8;
    let v = raw[2];
    let [r, g, b] = match sector % 6 {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        _ => [v, p, q],
    };
    [r, g, b, raw[3]]
}
#[derive(Debug, Clone)]
pub(super) struct Ring {
    pub segments: usize,
    pub angle: Curve3,
    pub outer: Curve3,
    pub inner: Curve3,
    pub ratio: Curve3,
    pub colors: [ColorCurve; 3], // outer, center, inner
}
#[derive(Debug, Clone)]
pub(super) enum Geometry {
    Ribbon(super::ribbon::Ribbon),
    Sprite {
        color: ColorCurve,
        corners: Option<[[u8; 4]; 4]>,
        positions: [[f32; 2]; 4],
    },
    Ring(Ring),
}
impl Geometry {
    pub fn quads(
        &self,
        uv: [f32; 4],
        age: f32,
        life: f32,
        fade: f32,
    ) -> Result<Vec<[Vertex; 4]>, Error> {
        Ok(match self {
            Self::Ribbon(_) => {
                return Err(Error::Host("ribbon requires its owning instance group"));
            }
            Self::Sprite {
                color,
                corners,
                positions,
            } => {
                // edd09..edd40 updates allColor alpha before corner and
                // play-global modulation in the renderer.
                let all = fade_alpha(color.value(age, life)?, fade)?;
                vec![std::array::from_fn(|j| Vertex {
                    position: [positions[j][0], positions[j][1], 0.0],
                    uv: [
                        uv[0] + if j % 2 == 1 { uv[2] } else { 0.0 },
                        uv[1] + if j < 2 { uv[3] } else { 0.0 },
                    ],
                    color: std::array::from_fn(|k| {
                        corners
                            .as_ref()
                            .map_or(all[k], |c| modulate(all[k], c[j][k]))
                    }),
                })]
            }
            Self::Ring(ring) => {
                let outer = ring.outer.value(age, life)?;
                let inner = ring.inner.value(age, life)?;
                let ratio = ring.ratio.value(age, life)?[0];
                let center: [f32; 3] =
                    std::array::from_fn(|i| inner[i] + (outer[i] - inner[i]) * ratio);
                let sections = [outer, center, inner];
                let colors = [
                    fade_alpha(ring.colors[0].value(age, life)?, fade)?,
                    fade_alpha(ring.colors[1].value(age, life)?, fade)?,
                    fade_alpha(ring.colors[2].value(age, life)?, fade)?,
                ];
                // Native renderer's literal is3.141592f, not f32::consts::PI.
                let radians = ring.angle.value(age, life)?[0] / 180.0 * 3.141592;
                let step = radians / ring.segments as f32;
                let (step_s, step_c) = step.sin_cos();
                let (mut sn, mut cs) = (-radians * 0.5).sin_cos();
                let tex_step = uv[2] / ring.segments as f32;
                let mut u = uv[0];
                let mut out = Vec::with_capacity(ring.segments * 2);
                for _ in 0..ring.segments {
                    let (next_s, next_c) = (sn * step_c + cs * step_s, cs * step_c - sn * step_s);
                    let next_u = u + tex_step;
                    for band in 0..2 {
                        // Vertex order preserves the original index buffer
                        // diagonal: (0,1,2),(2,1,3), shared center edge.
                        let q = std::array::from_fn(|j| {
                            let section = band + j % 2;
                            let next = j >= 2;
                            let radial = sections[section];
                            Vertex {
                                position: [
                                    if next { next_c } else { cs } * radial[0],
                                    if next { next_s } else { sn } * radial[0],
                                    radial[1],
                                ],
                                uv: [
                                    if next { next_u } else { u },
                                    uv[1] + uv[3] * section as f32 * 0.5,
                                ],
                                color: colors[section],
                            }
                        });
                        out.push(q);
                    }
                    (sn, cs) = (next_s, next_c);
                    u = next_u;
                }
                out
            }
        })
    }
}

pub(super) fn fade_alpha(mut color: [u8; 4], fade: f32) -> Result<[u8; 4], Error> {
    // edd38..edd40 / f0de0..f0de8 / eeb15..eeb1d: CVTTSS2SI,
    // then store AL. Keep byte wrapping for finite signed-conversion
    // results; clamping alpha or fade changes the target's output.
    let alpha = color[3] as f32 * fade;
    if !alpha.is_finite() || !(-2147483648.0..2147483648.0).contains(&alpha) {
        return Err(Error::Host(
            "SKFE faded alpha outside native signed conversion domain",
        ));
    }
    color[3] = (alpha as i32) as u8;
    Ok(color)
}

pub(super) fn modulate(a: u8, b: u8) -> u8 {
    // Target SIMD ed463..ed47b/ee74b..ee767/f00ae: mask at5fa550
    // is sixteen-bit1, followed by OR with the product shifted right8.
    let product = a as u16 * b as u16;
    ((product >> 8) | (product & 1)) as u8
}

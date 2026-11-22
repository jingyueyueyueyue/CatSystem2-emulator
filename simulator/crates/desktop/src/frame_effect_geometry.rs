//! Independently recovered page, cell and triangle-fragment geometry.
use anyhow::{Context, Result, bail};
use cs2_renderer::{Canvas, RgbaFrame};

use super::super::pixels;

#[path = "effect_tail_native.rs"]
mod effect_tail;

const MAX_TRIANGLES: usize = 65_536;
const GLOBAL_DEPTH: f32 = 65_000.;

#[derive(Clone)]
struct Vertex {
    original: [f32; 2],
    local: [f32; 3],
    screen: [f32; 4],
    uv: [f32; 2],
}

#[derive(Clone)]
struct Triangle {
    vertices: [Vertex; 3],
    side: usize,
    argb: u32,
    group: usize,
}

#[derive(Clone)]
struct Fragment {
    center: [f32; 3],
    velocity: [f32; 3],
    rotation: [[f32; 3]; 3],
    angular: [f32; 3],
}

#[derive(Clone)]
pub(super) struct Geometry {
    kind: u32,
    size: [u32; 2],
    words: Vec<u32>,
    triangles: Vec<Triangle>,
    fragments: Vec<Fragment>,
    previous: u32,
    configured: bool,
    flip: Option<effect_tail::FlipGeometry>,
}

impl Geometry {
    pub(super) fn new(kind: u32, size: [u32; 2]) -> Result<Self> {
        let result = Self {
            kind,
            size,
            words: Vec::new(),
            triangles: Vec::new(),
            fragments: Vec::new(),
            previous: 0,
            configured: false,
            flip: if matches!(kind, 57 | 58) {
                Some(effect_tail::FlipGeometry::new(kind, size)?)
            } else { None },
        };
        Ok(result)
    }

    pub(super) fn configure(
        &mut self,
        words: &[u32],
        current: Option<bool>,
        random: &mut impl FnMut() -> f32,
    ) -> Result<()> {
        if let Some(flip) = self.flip.as_mut() {
            return flip.configure(
                words,
                current.context("effect geometry needs an identified receiver version")?,
            );
        }
        self.words = words.to_vec();
        self.configured = true;
        match self.kind {
            52 => {
                self.triangles = grid(self.size, [words[3], words[4]], false)?;
                self.fragments.clear();
                let speed = value(words, 5)?;
                let angular = value(words, 6)? * std::f32::consts::PI / 180.;
                let center = self.size.map(|value| value as f32 * 0.5);
                let radius = center[0].hypot(center[1]);
                if radius == 0. {
                    bail!("fragment effect has an empty radial basis");
                }
                for (index, triangle) in self.triangles.iter_mut().enumerate() {
                    triangle.group = index;
                    let pivot: [f32; 3] = std::array::from_fn(|axis| {
                        if axis == 2 {
                            0.
                        } else {
                            triangle
                                .vertices
                                .iter()
                                .map(|v| v.original[axis])
                                .sum::<f32>()
                                / 3.
                        }
                    });
                    let relative = [pivot[0] - center[0], pivot[1] - center[1]];
                    let tilt =
                        relative[0].hypot(relative[1]) / radius * std::f32::consts::FRAC_PI_4;
                    let direction = relative[0].atan2(-relative[1]);
                    let mut velocity = rotate_z(
                        rotate_x([0., 0., random() * speed], tilt.sin_cos()),
                        direction.sin_cos(),
                    );
                    // The receiver flattens the initial radial velocity after
                    // transforming it; only shard rotation creates local Z.
                    velocity[2] = 0.;
                    let rates = std::array::from_fn(|_| angular * 0.5 - random() * angular);
                    self.fragments.push(Fragment {
                        center: pivot,
                        velocity,
                        rotation: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
                        angular: rates,
                    });
                    for vertex in &mut triangle.vertices {
                        vertex.local = [
                            vertex.original[0] - pivot[0],
                            vertex.original[1] - pivot[1],
                            0.,
                        ];
                    }
                }
                self.previous = 0;
            }
            _ => bail!("unrecognized retained effect geometry owner"),
        }
        Ok(())
    }

    pub(super) fn restart(&mut self) {
        if self.kind == 52 {
            self.previous = 0;
        }
    }

    pub(super) fn advance(&mut self, now: u32, started: u32, end: u32) -> Result<()> {
        if let Some(flip) = self.flip.as_mut() {
            return flip.advance(now, started, end);
        }
        if !self.configured {
            bail!("frame effect cannot animate before its geometry is configured");
        }
        let elapsed = now.min(end).wrapping_sub(started);
        let duration = end.wrapping_sub(started);
        self.advance_fragments(elapsed, duration)
    }

    fn advance_fragments(&mut self, elapsed: u32, duration: u32) -> Result<()> {
        let delta = elapsed.wrapping_sub(self.previous) as f32;
        let alpha = 255
            - elapsed
                .wrapping_mul(255)
                .checked_div(duration.max(1))
                .unwrap()
                .min(255);
        let texture = self.size.map(|value| value.next_power_of_two() as f32);
        for fragment in &mut self.fragments {
            for axis in 0..3 {
                fragment.center[axis] += fragment.velocity[axis] * delta;
            }
            // Increment the retained matrix in X/Y/Z order. Summing Euler
            // angles changes the result after the second render call.
            let [x, y, z] = fragment.angular.map(|angular| (angular * delta).sin_cos());
            for row in &mut fragment.rotation {
                *row = rotate_z(rotate_y(rotate_x(*row, x), y), z);
            }
        }
        for triangle in &mut self.triangles {
            let fragment = &self.fragments[triangle.group];
            triangle.argb = alpha << 24 | 0x00ff_ffff;
            for vertex in &mut triangle.vertices {
                let mut local: [f32; 3] = std::array::from_fn(|axis| {
                    (vertex.local[0] * fragment.rotation[0][axis]
                        + vertex.local[1] * fragment.rotation[1][axis])
                        + vertex.local[2] * fragment.rotation[2][axis]
                });
                for axis in 0..3 {
                    local[axis] += fragment.center[axis];
                }
                let depth = GLOBAL_DEPTH - (local[2] + 32_500.);
                if local[2] == 0. {
                    vertex.screen = [local[0], local[1], depth * (1. / GLOBAL_DEPTH), 1.];
                } else {
                    // Preserve the receiver's subtraction order and its
                    // explicit equal-depth divisor, including large-Z cases.
                    let denominator = depth - (GLOBAL_DEPTH - 32_500. - 1000.);
                    let denominator = if denominator == 0. { 1. } else { denominator };
                    let scale = (1000. / denominator).max(0.);
                    vertex.screen = [
                        (local[0] - texture[0]) * scale + texture[0],
                        (local[1] - texture[1]) * scale + texture[1],
                        depth * (1. / GLOBAL_DEPTH),
                        1. / denominator,
                    ];
                }
            }
        }
        self.previous = elapsed;
        Ok(())
    }

    pub(super) fn draw(&self, target: &mut Canvas, sources: [Option<&RgbaFrame>; 2]) -> Result<()> {
        if let Some(flip) = self.flip.as_ref() {
            return flip.draw(target, sources);
        }
        for triangle in &self.triangles {
            let source = sources[triangle.side]
                .context("frame effect auxiliary texture has been released")?;
            let vertices = triangle.vertices.each_ref().map(|vertex| {
                [
                    vertex.screen[0],
                    vertex.screen[1],
                    vertex.screen[2],
                    vertex.screen[3],
                    vertex.uv[0],
                    vertex.uv[1],
                ]
            });
            pixels::fragment_triangle(target, source, vertices, triangle.argb)?;
        }
        Ok(())
    }
}

fn grid(size: [u32; 2], step: [u32; 2], clipped: bool) -> Result<Vec<Triangle>> {
    if size.contains(&0) || step.contains(&0) {
        bail!("frame effect grid has a zero extent or cell dimension");
    }
    let counts = [size[0].div_ceil(step[0]), size[1].div_ceil(step[1])];
    let count = (counts[0] as usize)
        .checked_mul(counts[1] as usize)
        .and_then(|value| value.checked_mul(2))
        .context("frame effect grid capacity overflow")?;
    if count > MAX_TRIANGLES {
        bail!("frame effect grid exceeds 65536 triangles");
    }
    let texture = size.map(|value| value.next_power_of_two() as f32);
    let mut triangles = Vec::with_capacity(count);
    for row in 0..counts[1] {
        for column in 0..counts[0] {
            let left = (u64::from(column) * u64::from(step[0])) as f32;
            let top = (u64::from(row) * u64::from(step[1])) as f32;
            let right = left + step[0] as f32;
            let bottom = top + step[1] as f32;
            let right = if clipped {
                right.min(size[0] as f32)
            } else {
                right
            };
            let bottom = if clipped {
                bottom.min(size[1] as f32)
            } else {
                bottom
            };
            let vertices =
                [[left, top], [right, top], [left, bottom], [right, bottom]].map(|point| {
                    let original = point.map(|value| value - 0.5);
                    Vertex {
                        original,
                        local: [original[0], original[1], 0.],
                        screen: [original[0], original[1], 0., 1.],
                        uv: [point[0] / texture[0], point[1] / texture[1]],
                    }
                });
            for indexes in [[0, 1, 2], [1, 3, 2]] {
                triangles.push(Triangle {
                    vertices: indexes.map(|index| vertices[index].clone()),
                    side: 0,
                    argb: 0xffff_ffff,
                    group: 0,
                });
            }
        }
    }
    Ok(triangles)
}

fn value(words: &[u32], index: usize) -> Result<f32> {
    let value = f32::from_bits(
        *words
            .get(index)
            .context("frame effect geometry record is truncated")?,
    );
    if !value.is_finite() {
        bail!("frame effect geometry contains a non-finite float");
    }
    Ok(value)
}

fn rotate_x([x, y, z]: [f32; 3], (sin, cos): (f32, f32)) -> [f32; 3] {
    [x, y * cos + z * sin, z * cos - y * sin]
}
fn rotate_y([x, y, z]: [f32; 3], (sin, cos): (f32, f32)) -> [f32; 3] {
    [x * cos - z * sin, y, z * cos + x * sin]
}
fn rotate_z([x, y, z]: [f32; 3], (sin, cos): (f32, f32)) -> [f32; 3] {
    [x * cos + y * sin, y * cos - x * sin, z]
}

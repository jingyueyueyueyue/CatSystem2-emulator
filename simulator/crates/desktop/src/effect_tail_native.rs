//! Page/cell flip mesh receivers (ordinary967/968/970/971/972).
//! Written independently from fixed V401/Yukikoi PE data-flow and public
//! record layouts. No original code, DLL, or reference implementation is used.
use anyhow::{Context, Result, bail};
use cs2_renderer::{Canvas, RgbaFrame};

use super::super::super::pixels;

const MAX_TRIANGLES: usize = 65_536;
const GLOBAL_DEPTH: f32 = 65_000.;
const FOCAL: f32 = 2_000.;

#[derive(Clone)]
struct Vertex {
    original: [f32; 2],
    local: [f32; 2],
    screen: [[f32; 4]; 2],
    uv: [f32; 2],
}

#[derive(Clone)]
struct Triangle {
    vertices: [Vertex; 3],
    cell: usize,
}

#[derive(Clone)]
struct Cell {
    pivot: [f32; 2],
    side: usize,
}

#[derive(Clone)]
pub(super) struct FlipGeometry {
    kind: u32,
    size: [u32; 2],
    words: Vec<u32>,
    triangles: Vec<Triangle>,
    cells: Vec<Cell>,
    bounds: [f32; 2],
    configured: bool,
}

impl FlipGeometry {
    pub(super) fn new(kind: u32, size: [u32; 2]) -> Result<Self> {
        if !matches!(kind, 57 | 58) || size.contains(&0) {
            bail!("flip geometry needs a real page/cell owner and nonempty surface");
        }
        Ok(Self {
            kind, size, words: Vec::new(),
            triangles: if kind == 57 { grid(size, [32, 32])? } else { Vec::new() },
            cells: Vec::new(), bounds: [0.; 2], configured: false,
        })
    }

    pub(super) fn configure(&mut self, words: &[u32], current: bool) -> Result<()> {
        let count = if self.kind == 57 { 6 } else { 9 };
        let words = words.get(..count).context("flip configuration record is truncated")?;
        finite(words, 1)?;
        let mut next = self.clone();
        next.words = words.to_vec();
        if self.kind == 57 {
            next.configure_page()?;
        } else {
            next.configure_cells(current)?;
        }
        next.configured = true;
        *self = next;
        Ok(())
    }

    fn configure_page(&mut self) -> Result<()> {
        let direction = finite(&self.words, 3)?;
        finite(&self.words, 4)?;
        finite(&self.words, 5)?;
        let center = self.size.map(|value| value as f32 * 0.5);
        let depth = baseline(&self.words)?;
        let projected_origin = project([0.; 3], depth)?;
        self.bounds = [f32::NEG_INFINITY, f32::INFINITY];
        for triangle in &mut self.triangles {
            for vertex in &mut triangle.vertices {
                vertex.local = rotate([vertex.original[0] - center[0],
                                       vertex.original[1] - center[1]], direction);
                self.bounds[0] = self.bounds[0].max(vertex.local[1]);
                self.bounds[1] = self.bounds[1].min(vertex.local[1]);
                // V4010xd9b34..0xd9b77 resets Z/RHW in both retained
                // vertex arrays. It deliberately preserves generated X/Y.
                vertex.screen[0][2] = projected_origin[2];
                vertex.screen[0][3] = projected_origin[3];
            }
        }
        Ok(())
    }

    fn configure_cells(&mut self, current: bool) -> Result<()> {
        let step = [self.words[3], if current {
            // Yukikoi0x15a3c4 multiplies scaled height by original height.
            self.words[4].wrapping_mul(self.words[4])
        } else { self.words[4] }];
        let direction = finite(&self.words, 5)? + finite(&self.words, 7)?;
        if !direction.is_finite() { bail!("cell flip direction sum is non-finite"); }
        let width = finite(&self.words, 6)?;
        self.triangles = grid(self.size, step)?;
        self.cells.clear();
        self.bounds = [f32::NEG_INFINITY, f32::INFINITY];
        let center = self.size.map(|value| value as f32 * 0.5);
        for (index, cell) in self.triangles.chunks_exact_mut(2).enumerate() {
            // PE0xdb26d/0xdb2a8 uses the nominal cell extent, even for
            // a clipped edge. Vertices are half-pixel shifted; pivots are not.
            let first = cell[0].vertices[0].original;
            let pivot = rotate([first[0] + 0.5 + step[0] as f32 * 0.5 - center[0],
                                first[1] + 0.5 + step[1] as f32 * 0.5 - center[1]], direction);
            self.bounds[0] = self.bounds[0].max(pivot[1]);
            self.bounds[1] = self.bounds[1].min(pivot[1]);
            self.cells.push(Cell { pivot, side: 0 });
            for triangle in cell {
                triangle.cell = index;
                for vertex in &mut triangle.vertices {
                    vertex.local = rotate([vertex.original[0] - center[0],
                                           vertex.original[1] - center[1]], direction);
                }
            }
        }
        self.bounds[1] -= width;
        if self.bounds.iter().any(|value| !value.is_finite()) {
            bail!("cell flip wave bounds are non-finite");
        }
        Ok(())
    }

    pub(super) fn advance(&mut self, now: u32, started: u32, end: u32) -> Result<()> {
        if !self.configured { bail!("flip geometry cannot animate before configuration"); }
        let duration = end.wrapping_sub(started);
        if duration == 0 { bail!("flip duration zero has no finite mesh interpolation"); }
        // PE compares absolute unsigned endpoint first, then subtracts the
        // origin in DWORD arithmetic. Do not replace this with wall time.
        let fraction = now.min(end).wrapping_sub(started) as f32 / duration as f32;
        if self.kind == 57 { self.advance_page(fraction) } else { self.advance_cells(fraction) }
    }

    fn advance_page(&mut self, fraction: f32) -> Result<()> {
        let direction = finite(&self.words, 3)?;
        let radius = finite(&self.words, 4)?;
        let curl = finite(&self.words, 5)?;
        let circumference = radius * std::f32::consts::TAU;
        let arc = (curl / std::f32::consts::TAU) * circumference;
        let tip = roll([0., 0., radius], curl);
        let tangent = roll([0., 1., 0.], curl);
        let front = self.bounds[0] + fraction * ((self.bounds[1] - radius) - self.bounds[0]);
        let center = self.size.map(|value| value as f32 * 0.5);
        let depth = baseline(&self.words)?;
        for triangle in &mut self.triangles {
            for vertex in &mut triangle.vertices {
                let distance = vertex.local[1] - front;
                if distance < 0. { continue; } // Preserve the last generated position.
                let mut point = if distance < arc {
                    if circumference == 0. { bail!("page flip arc has a zero divisor"); }
                    let angle = (distance / circumference) * std::f32::consts::PI * 2.;
                    roll([0., 0., radius], angle)
                } else {
                    std::array::from_fn(|axis| tip[axis] + tangent[axis] * (distance - arc))
                };
                point[0] += vertex.local[0];
                point[1] += front;
                point[2] -= radius;
                let projected = project(point, depth)?;
                let xy = rotate([projected[0], projected[1]], -direction);
                vertex.screen[0] = [xy[0] + center[0], xy[1] + center[1],
                                    projected[2], projected[3]];
            }
        }
        Ok(())
    }

    fn advance_cells(&mut self, fraction: f32) -> Result<()> {
        let direction = finite(&self.words, 5)? + finite(&self.words, 7)?;
        let axis = finite(&self.words, 7)?;
        let width = finite(&self.words, 6)?;
        let front = self.bounds[0] + fraction * (self.bounds[1] - self.bounds[0]);
        let center = self.size.map(|value| value as f32 * 0.5);
        let depth = baseline(&self.words)?;
        for (index, cell) in self.cells.iter_mut().enumerate() {
            let distance = cell.pivot[1] - front;
            let angle = if distance < 0. { 0. } else if distance < width {
                distance / width * std::f32::consts::PI
            } else { std::f32::consts::PI };
            cell.side = usize::from(angle >= std::f32::consts::FRAC_PI_2);
            for triangle in &mut self.triangles[index * 2..index * 2 + 2] {
                for vertex in &mut triangle.vertices {
                    let point = if angle == 0. || angle >= std::f32::consts::PI {
                        // PE0xdbd0b/0xdc037/0xdc230: complete faces use
                        // the original absolute screen position, without an
                        // inverse direction rotation or an edge-cell shift.
                        project([vertex.original[0], vertex.original[1], 0.], depth)?
                    } else {
                        let relative = rotate([vertex.local[0] - cell.pivot[0],
                                               vertex.local[1] - cell.pivot[1]], -axis);
                        let face_angle = if cell.side == 0 { angle }
                            else { angle - std::f32::consts::PI };
                        let rolled = roll([relative[0], relative[1], 0.], face_angle);
                        let rotated = rotate([rolled[0], rolled[1]], axis);
                        let projected = project([rotated[0], rotated[1], rolled[2]], depth)?;
                        let positioned = rotate([projected[0] + cell.pivot[0],
                                                 projected[1] + cell.pivot[1]], -direction);
                        [positioned[0] + center[0], positioned[1] + center[1],
                         projected[2], projected[3]]
                    };
                    vertex.screen[cell.side] = point;
                }
            }
        }
        Ok(())
    }

    pub(super) fn draw(&self, target: &mut Canvas, sources: [Option<&RgbaFrame>; 2]) -> Result<()> {
        if self.kind == 58 && self.configured { background(target, self.words[8])?; }
        // PE0xdbaaf..0xdbb38 submits the complete front array before the
        // complete back array. Interleaving sides per cell changes overlap.
        for side in 0..if self.kind == 57 { 1 } else { 2 } {
            for triangle in &self.triangles {
                if self.kind == 58 && self.cells[triangle.cell].side != side { continue; }
                let source = sources[side].context("flip auxiliary texture has been released")?;
                if [source.width(), source.height()] != self.size {
                    bail!("flip auxiliary texture changed dimensions without recreating the owner");
                }
                let vertices = triangle.vertices.each_ref().map(|vertex| {
                    let [x, y, z, rhw] = vertex.screen[side];
                    [x, y, z, rhw, vertex.uv[0], vertex.uv[1]]
                });
                pixels::effect_triangle(target, source, vertices, 0xffff_ffff)?;
            }
        }
        Ok(())
    }
}

fn grid(size: [u32; 2], step: [u32; 2]) -> Result<Vec<Triangle>> {
    if step.contains(&0) { bail!("flip cell width/height is zero after receiver scaling"); }
    let count = u64::from(size[0].div_ceil(step[0]))
        .checked_mul(u64::from(size[1].div_ceil(step[1])))
        .and_then(|value| value.checked_mul(2))
        .context("flip mesh capacity overflow")?;
    if count > MAX_TRIANGLES as u64 { bail!("flip mesh exceeds65536 triangles"); }
    let texture = [
        size[0].checked_next_power_of_two().context("flip texture width overflow")? as f32,
        size[1].checked_next_power_of_two().context("flip texture height overflow")? as f32,
    ];
    let mut triangles = Vec::with_capacity(count as usize);
    for row in 0..size[1].div_ceil(step[1]) {
        for column in 0..size[0].div_ceil(step[0]) {
            let left = u64::from(column) * u64::from(step[0]);
            let top = u64::from(row) * u64::from(step[1]);
            let right = (left + u64::from(step[0])).min(u64::from(size[0]));
            let bottom = (top + u64::from(step[1])).min(u64::from(size[1]));
            let points = [[left, top], [right, top], [left, bottom], [right, bottom]];
            let vertices = points.map(|point| {
                let original = point.map(|value| value as f32 - 0.5);
                Vertex {
                    original, local: original,
                    screen: [[original[0], original[1], 0., 1.]; 2],
                    uv: [point[0] as f32 / texture[0], point[1] as f32 / texture[1]],
                }
            });
            let cell = triangles.len() / 2;
            // Fixed PE writes TL/TR/BL, TR/BR/BL. Preserve that diagonal.
            for indexes in [[0, 1, 2], [1, 3, 2]] {
                triangles.push(Triangle { vertices: indexes.map(|index| vertices[index].clone()), cell });
            }
        }
    }
    Ok(triangles)
}

fn finite(words: &[u32], index: usize) -> Result<f32> {
    let value = f32::from_bits(*words.get(index).context("flip record is truncated")?);
    if !value.is_finite() { bail!("flip record contains a non-finite float"); }
    Ok(value)
}

fn baseline(words: &[u32]) -> Result<f32> {
    let result = GLOBAL_DEPTH - (finite(words, 1)? + 1_000.);
    if !result.is_finite() { bail!("flip baseline overflow"); }
    Ok(result)
}

fn project([x, y, z]: [f32; 3], depth: f32) -> Result<[f32; 4]> {
    // V4010x14f340 builds a diagonal matrix, normally1,1,1/65000,1/2000.
    // Preserve its equal-plane fallback and f32 subtraction for extreme
    // depth values.0x155c30 uses a plain Vec4 transform, without division
    // by W: curling Z never determines the X/Y scale.
    let near = depth - FOCAL;
    let span = if depth == near { 1. } else { depth - near };
    let scale = (FOCAL / span).max(0.);
    let result = [x * scale, y * scale, (depth + z) * (1. / GLOBAL_DEPTH), 1. / span];
    if result.iter().any(|value| !value.is_finite()) { bail!("flip projection is non-finite"); }
    Ok(result)
}

fn rotate([x, y]: [f32; 2], angle: f32) -> [f32; 2] {
    let sine = f64::from(angle).sin() as f32;
    let cosine = f64::from(angle).cos() as f32;
    [x * cosine + y * sine, y * cosine - x * sine]
}

fn roll([x, y, z]: [f32; 3], angle: f32) -> [f32; 3] {
    let sine = f64::from(angle).sin() as f32;
    let cosine = f64::from(angle).cos() as f32;
    [x, y * cosine + z * sine, z * cosine - y * sine]
}

fn background(target: &mut Canvas, argb: u32) -> Result<()> {
    let white = RgbaFrame::from_rgba8(1, 1, vec![255; 4])?;
    let corners = [[0., 0.], [target.width() as f32, 0.],
                   [0., target.height() as f32], [target.width() as f32, target.height() as f32]];
    for indexes in [[0, 1, 2], [1, 3, 2]] {
        pixels::effect_triangle(target, &white,
            indexes.map(|index| [corners[index][0], corners[index][1], 0.5, 1., 0., 0.]), argb)?;
    }
    Ok(())
}

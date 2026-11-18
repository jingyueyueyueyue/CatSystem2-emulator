//! Independent16-control-point patch operators from driver2c150/2bdb0.
//!
//! These operate on normalized interleaved x/y points. They do not encode an
//! unproven mesh hierarchy, adaptive tessellation or stencil policy.

use super::{EmoteError, Result};

#[derive(Debug, Clone)]
pub struct CpuBezierPatch {
    points: [[f32; 2]; 16],
}

#[derive(Debug, Clone, Copy)]
pub struct CpuMeshGrid {
    pub columns: u32,
    pub rows: u32,
}

impl CpuMeshGrid {
    ///58762/5876d and2f095/2f099 add one vertex to each subdivision axis.
    ///Keep the frame-local raster grid bounded before allocating its XY/UVs.
    pub(super) fn raster_layout(self) -> Result<(usize, usize)> {
        if self.columns == 0 || self.rows == 0 || self.columns > 512 || self.rows > 512 {
            return Err(invalid("invalid mesh raster grid"));
        }
        let stride = self.columns as usize + 1;
        Ok((stride, stride * (self.rows as usize + 1)))
    }

    fn split(width: u32, height: u32, total: u32) -> Result<Self> {
        let sum = width
            .checked_add(height)
            .filter(|n| *n != 0)
            .ok_or_else(|| invalid("invalid mesh icon dimensions"))?;
        if width == 0 || height == 0 {
            return Err(invalid("invalid mesh icon dimensions"));
        }
        //3ffac imul is a DWORD product before unsigned div.
        let columns = total.wrapping_mul(width) / sum;
        let rows = total.checked_sub(columns).ok_or(EmoteError::Capacity)?;
        let grid = Self {
            columns: columns.checked_add(1).ok_or(EmoteError::Capacity)?,
            rows: rows.checked_add(1).ok_or(EmoteError::Capacity)?,
        };
        if grid.columns > 512 || grid.rows > 512 {
            return Err(EmoteError::Capacity);
        }
        Ok(grid)
    }

    /// Original3909c/3ff01: truncate(meshDivision*quality), split the integer
    /// total by icon aspect ratio, then add one subdivision on each axis.
    /// Driver3078c initializes quality to1.0; its setter25c0 can change it.
    pub fn from_division(width: u32, height: u32, division: u32, quality: f32) -> Result<Self> {
        if width == 0 || height == 0 || !quality.is_finite() || quality <= 0.0 {
            return Err(invalid("invalid mesh quality/dimensions"));
        }
        let total = (division as f32 * quality).trunc();
        if !total.is_finite() || !(0.0..=65535.0).contains(&total) {
            return Err(EmoteError::Capacity);
        }
        Self::split(width, height, total as u32)
    }

    ///3ff31..3ffd5: nearest active ancestor supplies division and icon size.
    ///Scale its integer total by the current/ancestor dimension sums first;
    ///then split by current aspect ratio. Quality is truncated only once.
    pub fn from_inherited_division(
        width: u32,
        height: u32,
        ancestor_width: u32,
        ancestor_height: u32,
        ancestor_division: u32,
        quality: f32,
    ) -> Result<Self> {
        let sum = width
            .checked_add(height)
            .filter(|n| *n != 0)
            .ok_or_else(|| invalid("invalid inherited mesh dimensions"))?;
        let ancestor_sum = ancestor_width
            .checked_add(ancestor_height)
            .filter(|n| *n != 0)
            .ok_or_else(|| invalid("invalid mesh ancestor dimensions"))?;
        if ancestor_width == 0 || ancestor_height == 0 || !quality.is_finite() || quality <= 0.0 {
            return Err(invalid("invalid inherited mesh quality/ancestor"));
        }
        let total = (ancestor_division as f32 * quality).trunc();
        if !total.is_finite() || !(0.0..=65535.0).contains(&total) {
            return Err(EmoteError::Capacity);
        }
        let inherited = (total as u32).wrapping_mul(sum) / ancestor_sum;
        Self::split(width, height, inherited)
    }
}

fn invalid(why: &'static str) -> EmoteError {
    EmoteError::InvalidResource(why)
}

impl CpuBezierPatch {
    pub fn from_interleaved(values: &[f64]) -> Result<Self> {
        if values.len() != 32 {
            return Err(invalid("Bezier BP requires32 interleaved values"));
        }
        let mut points = [[0.0; 2]; 16];
        for (point, pair) in points.iter_mut().zip(values.chunks_exact(2)) {
            *point = [pair[0] as f32, pair[1] as f32];
        }
        if points.iter().flatten().any(|n| !n.is_finite()) {
            return Err(invalid("non-finite Bezier control point"));
        }
        Ok(Self { points })
    }

    pub fn identity() -> Self {
        let mut points = [[0.0; 2]; 16];
        for y in 0..4 {
            for x in 0..4 {
                points[y * 4 + x] = [x as f32 / 3.0, y as f32 / 3.0];
            }
        }
        Self { points }
    }

    pub fn points(&self) -> &[[f32; 2]; 16] {
        &self.points
    }

    /// Driver2c150 uses raw u/v. Outside-unit inputs extrapolate; clamping
    /// those inputs would incorrectly flatten children outside the patch.
    pub fn evaluate(&self, u: f64, v: f64) -> Result<[f64; 2]> {
        let u = u as f32;
        let v = v as f32;
        if !u.is_finite() || !v.is_finite() {
            return Err(invalid("non-finite Bezier coordinate"));
        }
        fn basis(t: f32) -> [f32; 4] {
            let s = 1.0 - t;
            [s * s * s, 3.0 * s * s * t, 3.0 * s * t * t, t * t * t]
        }
        let x = basis(u);
        let y = basis(v);
        let mut result = [0.0f32; 2];
        for axis in 0..2 {
            let mut rows = [0.0; 4];
            for j in 0..4 {
                rows[j] = self.points[j * 4][axis] * x[0]
                    + self.points[j * 4 + 1][axis] * x[1]
                    + self.points[j * 4 + 2][axis] * x[2]
                    + self.points[j * 4 + 3][axis] * x[3];
            }
            result[axis] = rows[0] * y[0] + rows[1] * y[1] + rows[2] * y[2] + rows[3] * y[3];
        }
        if result.iter().any(|n| !n.is_finite()) {
            return Err(invalid("Bezier evaluation overflow"));
        }
        Ok(result.map(f64::from))
    }

    /// Exact pointwise operation of2bdb0: destination += source - identity.
    /// Callers must still supply a proven meshCombine ownership chain.
    pub fn combine_offsets(&mut self, source: &Self) -> Result<()> {
        let identity = Self::identity();
        let mut next = self.clone();
        for i in 0..16 {
            for axis in 0..2 {
                next.points[i][axis] += source.points[i][axis] - identity.points[i][axis];
            }
        }
        if next.points.iter().flatten().any(|n| !n.is_finite()) {
            return Err(invalid("combined Bezier patch overflow"));
        }
        *self = next;
        Ok(())
    }
}

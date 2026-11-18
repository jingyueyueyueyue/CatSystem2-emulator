//! Exact-PE host sidecars for original task clocks and captured D3D transforms.
//!
//! No monitor dimensions, centering, frame rate or PSB screenSize is inferred.
//! Driver56180 reads WORLD/VIEW/PROJECTION/GetViewport from the real host;
//! M2Plane238090 applies the separate display offset/scale and -0.5 adjustment.

use super::{CpuViewport, EmoteError, Result};

pub const NEKO_ORIGINAL_TASK_UNITS_PER_FRAME: f64 = 1.0;

impl CpuViewport {
    /// EXE19c050's default offscreen camera,with world translation168=0
    /// (constructor19b393). Dimensions are the real M2 target,not a monitor.
    /// Role/profile position and scale remain separate object transforms.
    pub fn original_neko_target(width: u32, height: u32) -> Result<Self> {
        let viewport = Self {
            width,
            height,
            origin_x: f64::from(width) * 0.5 - 0.5,
            origin_y: f64::from(height) * 0.5 - 0.5,
            scale: 1.0,
            time_units_per_frame: NEKO_ORIGINAL_TASK_UNITS_PER_FRAME,
        };
        viewport.check()?;
        Ok(viewport)
    }
}

/// Original174040 fields164/168. The zero previous168 sentinel is retained,
/// including its first-call and zero-clock behavior; no wall-time conversion.
#[derive(Debug, Clone, Copy, Default)]
pub struct CpuOriginalTaskClock {
    clock_164: u32,
    previous_168: u32,
}

impl CpuOriginalTaskClock {
    pub fn advance(&mut self, task_clock: u32) -> f32 {
        let previous = if self.previous_168 == 0 {
            task_clock.wrapping_sub(1)
        } else {
            self.clock_164
        };
        self.previous_168 = previous;
        self.clock_164 = task_clock;
        task_clock.wrapping_sub(previous) as f32
    }
    pub fn raw_fields(&self) -> [u32; 2] {
        [self.clock_164, self.previous_168]
    }
}

#[derive(Debug, Clone, Copy)]
pub struct CpuOriginalDisplayMapping {
    pub logical_width: u32,
    pub logical_height: u32,
    /// Actual render-context67ca20 result; zero only when the host says so.
    pub host_offset: [f32; 2],
    /// Actual67ca40 result. The original no-context fallback is1.0.
    pub host_scale: f32,
}

impl CpuOriginalDisplayMapping {
    /// Original final M2Plane quad position, not the internal PSB origin.
    pub fn point(&self, logical: [f32; 2]) -> Result<[f32; 2]> {
        if !self.host_scale.is_finite()
            || self.host_scale <= 0.0
            || self
                .host_offset
                .iter()
                .chain(logical.iter())
                .any(|n| !n.is_finite())
            || self.logical_width == 0
            || self.logical_height == 0
        {
            return Err(EmoteError::InvalidResource(
                "invalid original display mapping",
            ));
        }
        Ok([
            logical[0] * self.host_scale + self.host_offset[0] - 0.5,
            logical[1] * self.host_scale + self.host_offset[1] - 0.5,
        ])
    }
    pub fn quad(&self) -> Result<[[f32; 2]; 4]> {
        Ok([
            self.point([0.0, 0.0])?,
            self.point([self.logical_width as f32, 0.0])?,
            self.point([0.0, self.logical_height as f32])?,
            self.point([self.logical_width as f32, self.logical_height as f32])?,
        ])
    }
}

/// Row-major D3D9 matrices with row-vector multiplication, captured/provided by
/// the independent host. Driver56180 reads transform IDs256,2,3 and viewport.
#[derive(Debug, Clone)]
pub struct CpuD3d9Projection {
    pub world: [f32; 16],
    pub view: [f32; 16],
    pub projection: [f32; 16],
    /// x,y,width,height of the original D3DVIEWPORT9.
    pub viewport: [u32; 4],
    pub min_z: f32,
    pub max_z: f32,
}

impl CpuD3d9Projection {
    /// Exact default-camera construction of EXE19c050. Its eye is
    /// (-0.5,0.5,-height/2),target(-0.5,0.5,0),up(0,-1,0); then only the
    /// look-at matrix's first element is negated. This yields positive X/Y
    /// pixel scale1 on z=0 and the half-pixel-adjusted center.
    pub fn original_neko_target(width: u32, height: u32, world_offset: f32) -> Result<Self> {
        if width == 0 || height == 0 || !world_offset.is_finite() {
            return Err(EmoteError::InvalidResource("invalid original M2 target"));
        }
        let mut world = [0.0; 16];
        for n in [0, 5, 10, 15] {
            world[n] = 1.0;
        }
        world[12] = world_offset;
        world[13] = world_offset;
        let mut view = [0.0; 16];
        view[0] = 1.0;
        view[5] = -1.0;
        view[10] = 1.0;
        view[15] = 1.0;
        view[12] = -0.5;
        view[13] = 0.5;
        view[14] = height as f32 * 0.5;
        let mut projection = [0.0; 16];
        projection[0] = 1.0 / (width as f32 / height as f32);
        projection[5] = 1.0;
        projection[10] = 1.0;
        projection[11] = 1.0;
        Ok(Self {
            world,
            view,
            projection,
            viewport: [0, 0, width, height],
            min_z: 0.0,
            max_z: 1.0,
        })
    }

    fn transform(vector: [f32; 4], matrix: &[f32; 16]) -> [f32; 4] {
        std::array::from_fn(|column| {
            vector[0] * matrix[column]
                + vector[1] * matrix[4 + column]
                + vector[2] * matrix[8 + column]
                + vector[3] * matrix[12 + column]
        })
    }
    pub fn project(&self, point: [f32; 3]) -> Result<[f32; 3]> {
        if self
            .world
            .iter()
            .chain(self.view.iter())
            .chain(self.projection.iter())
            .chain(point.iter())
            .chain([&self.min_z, &self.max_z])
            .any(|n| !n.is_finite())
            || self.viewport[2] == 0
            || self.viewport[3] == 0
            || self.max_z < self.min_z
        {
            return Err(EmoteError::InvalidResource(
                "invalid original projection snapshot",
            ));
        }
        let world = Self::transform([point[0], point[1], point[2], 1.0], &self.world);
        let view = Self::transform(world, &self.view);
        let clip = Self::transform(view, &self.projection);
        if !clip[3].is_finite() || clip[3] == 0.0 {
            return Err(EmoteError::InvalidResource(
                "original projection has zero W",
            ));
        }
        let result = [
            self.viewport[0] as f32 + (1.0 + clip[0] / clip[3]) * self.viewport[2] as f32 * 0.5,
            self.viewport[1] as f32 + (1.0 - clip[1] / clip[3]) * self.viewport[3] as f32 * 0.5,
            self.min_z + clip[2] / clip[3] * (self.max_z - self.min_z),
        ];
        if result.iter().any(|n| !n.is_finite()) {
            return Err(EmoteError::InvalidResource("original projection overflow"));
        }
        Ok(result)
    }

    /// Reduces a proven uniform axis-aligned plane to the current CPU viewport.
    /// A perspective/rotated/nonuniform snapshot needs the full vertex path;
    /// rejecting it prevents an invented origin or scale from entering play.
    pub fn axis_aligned_viewport(&self, width: u32, height: u32) -> Result<CpuViewport> {
        for axis in [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0]] {
            let vector = Self::transform(
                Self::transform(Self::transform(axis, &self.world), &self.view),
                &self.projection,
            );
            if vector[3] != 0.0 {
                return Err(EmoteError::Backend(
                    "perspective plane cannot be reduced to CpuViewport".into(),
                ));
            }
        }
        let a = self.project([0.0, 0.0, 0.0])?;
        let b = self.project([1.0, 0.0, 0.0])?;
        let c = self.project([0.0, 1.0, 0.0])?;
        let d = self.project([100.0, 100.0, 0.0])?;
        let sx = b[0] - a[0];
        let sy = c[1] - a[1];
        let tolerance = 0.001;
        if sx <= 0.0
            || sy <= 0.0
            || (sx - sy).abs() > tolerance
            || (b[1] - a[1]).abs() > tolerance
            || (c[0] - a[0]).abs() > tolerance
            || (d[0] - (a[0] + 100.0 * sx)).abs() > tolerance
            || (d[1] - (a[1] + 100.0 * sy)).abs() > tolerance
        {
            return Err(EmoteError::Backend(
                "original projection cannot be reduced to a uniform CPU viewport".into(),
            ));
        }
        Ok(CpuViewport {
            width,
            height,
            origin_x: f64::from(a[0]),
            origin_y: f64::from(a[1]),
            scale: f64::from(sx),
            time_units_per_frame: NEKO_ORIGINAL_TASK_UNITS_PER_FRAME,
        })
    }
}

//! Portable FES control state. Resource lookup, drawing and input ownership
//! remain host services; arithmetic retains the original signed DWORD steps.

use crate::{RuntimeError, RuntimeResult};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlDirection {
    Left,
    Right,
    Up,
    Down,
}

impl ControlDirection {
    pub fn step(self, vertical: bool) -> Option<i32> {
        match (self, vertical) {
            (Self::Left, false) | (Self::Up, true) => Some(-1),
            (Self::Right, false) | (Self::Down, true) => Some(1),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MetadataReference {
    pub name: Vec<u8>,
    pub frame: u32,
}

#[derive(Clone, Debug)]
pub struct ViewportState {
    pub position: [f32; 2],
    pub size: [f32; 2],
    pub position_source: Option<MetadataReference>,
    pub size_source: Option<MetadataReference>,
    pub rebuild: bool,
    pub dirty: bool,
}

impl Default for ViewportState {
    fn default() -> Self {
        Self {
            position: [0.; 2],
            size: [100_000.; 2],
            position_source: None,
            size_source: None,
            rebuild: false,
            dirty: false,
        }
    }
}

impl ViewportState {
    pub fn set_position(&mut self, position: [f32; 2]) {
        if self.position != position {
            self.position = position;
            self.dirty = true;
        }
    }

    pub fn set_size(&mut self, size: [f32; 2]) {
        if self.size != size {
            self.size = size;
            self.dirty = true;
        }
    }

    pub fn set_position_source(&mut self, source: MetadataReference) {
        self.position_source = Some(source);
    }

    pub fn set_size_source(&mut self, source: MetadataReference) {
        self.size_source = Some(source);
        self.rebuild = true;
    }

    /// Metadata is read only for a SIZE2 rebuild. A lone POS2 stays pending.
    /// Successful earlier writes survive a later resource error on retry.
    pub fn update(
        &mut self,
        mut read: impl FnMut(&MetadataReference) -> RuntimeResult<Option<([i32; 2], [i32; 2])>>,
    ) -> RuntimeResult<()> {
        self.dirty = false;
        if !self.rebuild {
            return Ok(());
        }
        let size = match &self.size_source {
            Some(source) => read(source)?.map_or([0; 2], |(_, size)| size),
            None => [0; 2],
        };
        self.set_size(size.map(|value| value as f32));
        if let Some(source) = &self.position_source {
            let position = read(source)?.map_or([0; 2], |(position, _)| position);
            self.set_position(position.map(|value| value as f32));
            self.position_source = None;
        }
        self.rebuild = false;
        Ok(())
    }

    pub fn bounds(&self) -> [i32; 4] {
        let x = float_to_i32(self.position[0]);
        let y = float_to_i32(self.position[1]);
        [
            x,
            y,
            x.wrapping_add(float_to_i32(self.size[0])),
            y.wrapping_add(float_to_i32(self.size[1])),
        ]
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SliderControl {
    pub range: [i32; 2],
    pub travel: [i32; 2],
    pub position: i32,
    pub pixel: i32,
    pub dragging: bool,
}

impl Default for SliderControl {
    fn default() -> Self {
        Self {
            range: [0; 2],
            travel: [0; 2],
            position: 0,
            pixel: 0,
            dragging: false,
        }
    }
}

impl SliderControl {
    pub fn set_range(&mut self, range: [i32; 2]) {
        self.range = range;
    }

    pub fn set_position(&mut self, position: i32) -> RuntimeResult<()> {
        self.position = position.max(self.range[0]).min(self.range[1]);
        if self.dragging {
            return Ok(());
        }
        let span = self.range[1].wrapping_sub(self.range[0]).wrapping_abs();
        if self.position <= 0 {
            self.position = 0;
            self.pixel = 0;
        } else if self.position >= span {
            self.position = span;
            self.pixel = self.travel[1];
        } else {
            let extent = self.travel[1].wrapping_sub(self.travel[0]).wrapping_abs();
            self.pixel = divide(
                divide(extent.wrapping_mul(self.position).wrapping_mul(100), span)?,
                100,
            )?;
        }
        Ok(())
    }

    pub fn value_from_pixel(&self, pixel: i32) -> RuntimeResult<i32> {
        let span = self.range[1].wrapping_sub(self.range[0]).wrapping_abs();
        let extent = self.travel[1].wrapping_sub(self.travel[0]).wrapping_abs();
        let step = divide(extent.wrapping_mul(100), span)?;
        threshold_position(pixel, span, step)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scroll2Control {
    pub range: [i32; 2],
    pub page: i32,
    pub track_extent: i32,
    pub minimum_extent: i32,
    pub knob_extent: i32,
    pub travel: [i32; 2],
    pub position: i32,
    pub pixel: i32,
    pub dragging: bool,
}

impl Default for Scroll2Control {
    fn default() -> Self {
        Self {
            range: [0, 10],
            page: 4,
            track_extent: 0,
            minimum_extent: 0,
            knob_extent: 0,
            travel: [0; 2],
            position: 0,
            pixel: 0,
            dragging: false,
        }
    }
}

impl Scroll2Control {
    pub fn set_range(&mut self, range: [i32; 2], page: i32) {
        self.range = range;
        self.page = page;
    }

    pub fn rebuild_geometry(&mut self) -> RuntimeResult<()> {
        let span = self.range[1].wrapping_sub(self.range[0]).wrapping_abs();
        if span <= self.page {
            self.knob_extent = self.track_extent;
            self.travel[1] = self.travel[0];
        } else if span > 0 {
            self.knob_extent =
                divide(self.track_extent.wrapping_mul(self.page), span)?.max(self.minimum_extent);
            self.travel[1] = self.track_extent.wrapping_sub(self.knob_extent);
        }
        Ok(())
    }

    pub fn set_position(&mut self, position: i32) -> RuntimeResult<()> {
        self.position = position.max(self.range[0]).min(self.range[1]);
        if self.dragging {
            return Ok(());
        }
        let span = self.range[1]
            .wrapping_sub(self.range[0])
            .wrapping_abs()
            .wrapping_sub(self.page);
        if self.position <= 0 {
            self.position = 0;
            self.pixel = 0;
        } else if self.position >= span {
            self.position = span;
            self.pixel = self.travel[1];
        } else {
            let step = divide(
                self.track_extent
                    .wrapping_sub(self.knob_extent)
                    .wrapping_mul(100),
                span,
            )?;
            self.pixel = divide(step.wrapping_mul(self.position), 100)?;
        }
        Ok(())
    }

    pub fn value_from_pixel(&self, pixel: i32) -> RuntimeResult<i32> {
        let span = self.range[1]
            .wrapping_sub(self.range[0])
            .wrapping_abs()
            .wrapping_sub(self.page);
        if span == 0 {
            return Ok(0);
        }
        let step = divide(
            self.track_extent
                .wrapping_sub(self.knob_extent)
                .wrapping_mul(100),
            span,
        )?;
        threshold_position(pixel, span, step)
    }
}

fn threshold_position(pixel: i32, span: i32, step: i32) -> RuntimeResult<i32> {
    if span <= 0 {
        return Ok(0);
    }
    let value = pixel.wrapping_mul(100);
    if step == 0 {
        return Ok(if value >= 0 { span } else { 0 });
    }
    if span > 1_000_000 {
        return Err(RuntimeError::Host(
            "control position query exceeds 1000000 steps".into(),
        ));
    }
    let mut threshold = divide(step, 2)?;
    let mut position = 0;
    while position < span && value >= threshold {
        position += 1;
        threshold = threshold.wrapping_add(step);
    }
    Ok(position)
}

fn divide(left: i32, right: i32) -> RuntimeResult<i32> {
    left.checked_div(right).ok_or_else(|| {
        RuntimeError::Host("control division has a zero divisor or signed quotient overflow".into())
    })
}

fn float_to_i32(value: f32) -> i32 {
    if !value.is_finite() || value < i32::MIN as f32 || value >= 2_147_483_648. {
        i32::MIN
    } else {
        value.trunc() as i32
    }
}

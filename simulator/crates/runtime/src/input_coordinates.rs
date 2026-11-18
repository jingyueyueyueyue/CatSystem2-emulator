//! Original single-precision input coordinate arithmetic. Host client/window
//! layout is supplied explicitly; forward and reverse scales are independent.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InputCoordinateMap {
    pub device_offset: [f32; 2],
    pub device_scale: f32,
    pub configuration_offset: [i32; 2],
    pub logical_scale: [f32; 2],
    pub device_units_per_logical: [f32; 2],
    pub logical_size: [i32; 2],
}

impl Default for InputCoordinateMap {
    fn default() -> Self {
        Self {
            device_offset: [0.; 2], device_scale: 1., configuration_offset: [0; 2],
            logical_scale: [1.; 2], device_units_per_logical: [1.; 2], logical_size: [0; 2],
        }
    }
}

impl InputCoordinateMap {
    pub fn contains(&self, point: [i32; 2]) -> bool {
        point.into_iter().zip(self.logical_size).all(|(value, limit)| value >= 0 && value < limit)
    }

    pub fn logical_axis(&self, axis: usize, value: i32) -> i32 {
        let reciprocal = 1_f32 / self.device_scale;
        let translated = value as f32 - self.device_offset[axis];
        let device = x86_truncate(translated * reciprocal);
        let configured = device.wrapping_sub(self.configuration_offset[axis]);
        x86_truncate(configured as f32 * self.logical_scale[axis])
    }

    pub fn to_logical(&self, point: [i32; 2]) -> ([i32; 2], bool) {
        let output = [self.logical_axis(0, point[0]), self.logical_axis(1, point[1])];
        (output, self.contains(output))
    }

    pub fn device_axis(&self, axis: usize, value: i32) -> i32 {
        let scaled = x86_truncate(value as f32 * self.device_units_per_logical[axis]);
        let configured = scaled.wrapping_add(self.configuration_offset[axis]);
        let reciprocal = 1_f32 / self.device_scale;
        let device = configured as f32 / reciprocal;
        x86_truncate(device + self.device_offset[axis])
    }

    pub fn to_device(&self, point: [i32; 2]) -> ([i32; 2], bool) {
        ([self.device_axis(0, point[0]), self.device_axis(1, point[1])], self.contains(point))
    }
}

pub fn x86_truncate(value: f32) -> i32 {
    if !value.is_finite() || value < -2_147_483_648_f32 || value >= 2_147_483_648_f32 {
        i32::MIN
    } else { value.trunc() as i32 }
}

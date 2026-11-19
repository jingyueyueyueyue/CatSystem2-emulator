//! NEKOPARA driver root-property overrides, before each root's transformOrder.
//! Driver2820/cb60,1bd69..1be39,3e9eb..3eabb; no final-quad transform.
use super::{EmoteError, PsbValue, Result};

#[derive(Debug, Clone, Copy)]
pub struct CpuDriverTransform {
    position: [f32; 2],
    scale: f32,
    angle_degrees: f32,
}

impl CpuDriverTransform {
    pub(super) fn render_bits(self) -> [u32; 4] {
        [self.position[0], self.position[1], self.scale, self.angle_degrees].map(f32::to_bits)
    }

    /// SetRot receives radians. With frame=0 the driver normalizes by repeated
    /// f32 additions/subtractions, then converts to degrees for root overrides.
    pub fn new(position: [f32; 2], scale: f32, rotation: f32) -> Result<Self> {
        if position.iter().chain([&scale, &rotation]).any(|n| !n.is_finite()) {
            return Err(EmoteError::InvalidResource("nonfinite M2 driver transform"));
        }
        let mut angle = rotation;
        let tau = f32::from_bits(0x40c9_0fdb);
        let mut budget = 65_536;
        while angle < 0.0 || angle >= tau {
            let next = if angle < 0.0 { angle + tau } else { angle - tau };
            if budget == 0 || next == angle {
                return Err(EmoteError::InvalidResource("M2 rotation normalization does not progress within budget"));
            }
            angle = next;
            budget -= 1;
        }
        Ok(Self { position, scale, angle_degrees: angle * 360.0_f32 / tau })
    }

    /// Only the top-level motion roots receive player overrides. Their own
    /// coordinate is added to position without being scaled or rotated by an
    /// invented parent matrix; local angle/zoom enter the stored operation order.
    pub(super) fn apply(self, content: &mut PsbValue) -> Result<()> {
        fn scalar(value: Option<&PsbValue>, default: f32) -> Result<f32> {
            let number = match value {
                None | Some(PsbValue::Null) => default,
                Some(value) => value.number().ok_or(EmoteError::InvalidResource("nonnumeric M2 root property"))? as f32,
            };
            if !number.is_finite() { return Err(EmoteError::InvalidResource("nonfinite M2 root property")); }
            Ok(number)
        }
        let PsbValue::Object(fields) = content else {
            return Err(EmoteError::InvalidResource("M2 root sample is not an object"));
        };
        let mut coord = match fields.get("coord") {
            None | Some(PsbValue::Null) => [0.0; 3],
            Some(PsbValue::List(values)) if values.len() == 3 => [
                scalar(values.first(), 0.0)?, scalar(values.get(1), 0.0)?, scalar(values.get(2), 0.0)?,
            ],
            Some(PsbValue::Integers(values)) if values.len() == 3 => [
                values[0] as f32, values[1] as f32, values[2] as f32,
            ],
            _ => return Err(EmoteError::InvalidResource("M2 root coord must have three values")),
        };
        coord[0] = self.position[0] + coord[0];
        coord[1] = self.position[1] + coord[1];
        let angle = self.angle_degrees + scalar(fields.get("angle"), 0.0)?;
        let zx = self.scale * scalar(fields.get("zx"), 1.0)?;
        let zy = self.scale * scalar(fields.get("zy"), 1.0)?;
        if coord.iter().chain([&angle, &zx, &zy]).any(|n| !n.is_finite()) {
            return Err(EmoteError::InvalidResource("M2 root property arithmetic overflow"));
        }
        fields.insert("coord".into(), PsbValue::List(coord.into_iter().map(|n| PsbValue::Number(f64::from(n))).collect()));
        for (key, value) in [("angle", angle), ("zx", zx), ("zy", zy)] {
            fields.insert(key.into(), PsbValue::Number(f64::from(value)));
        }
        Ok(())
    }
}

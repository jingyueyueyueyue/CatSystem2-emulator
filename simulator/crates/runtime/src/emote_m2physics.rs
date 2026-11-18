//! Actual M2 material spring operator from EXE194730/1948c0.
//! Live profile/variable impulse production and output consumption are in
//! emote_m2force; dynamic material group controls remain separate.

use super::{EmoteError, Result};

#[derive(Debug, Clone)]
pub struct CpuM2Material {
    pub name: String,
    pub mass: f32,
    pub spring: f32,
    pub resistance: f32,
    pub microstep: f32,
    pub maximum: f32,
    pub mirror: bool,
    pub output_scale: f32,
    pub output_shift: f32,
    pub enabled: bool,
    ///1937d0 preserves the complete raw hold word; nonzero skips integration.
    pub held_raw_d0: u32,
    pub displacement: f32,
    pub velocity: f32,
    pub normalized: f32,
}

impl CpuM2Material {
    pub fn from_record(row: &[Vec<u8>]) -> Result<Self> {
        let invalid = || EmoteError::InvalidResource("M2 material record");
        if row.len() != 9 {
            return Err(invalid());
        }
        let (name, _, errors) = encoding_rs::SHIFT_JIS.decode(&row[0]);
        if errors || name.is_empty() || name.contains('\0') {
            return Err(invalid());
        }
        let number = |bytes: &[u8]| -> Result<f32> {
            std::str::from_utf8(bytes)
                .ok()
                .and_then(|n| n.parse::<f32>().ok())
                .filter(|n| n.is_finite())
                .ok_or_else(invalid)
        };
        let material = Self {
            name: name.into_owned(),
            mass: number(&row[1])?,
            spring: number(&row[2])?,
            resistance: number(&row[3])?,
            microstep: number(&row[4])?,
            maximum: number(&row[5])?,
            mirror: number(&row[6])? != 0.0,
            output_scale: number(&row[7])?,
            output_shift: number(&row[8])?,
            enabled: true,
            held_raw_d0: 0,
            displacement: 0.0,
            velocity: 0.0,
            normalized: 0.0,
        };
        if material.mass <= 0.0 || material.maximum <= 0.0 || material.microstep <= 0.0 {
            return Err(invalid());
        }
        Ok(material)
    }

    ///193090/194500 subtract the already evaluated axis/variable impulse.
    pub(super) fn inject_impulse(&mut self, impulse: f32) -> Result<()> {
        let value = self.displacement - impulse;
        if !value.is_finite() {
            return Err(EmoteError::InvalidResource("M2 material impulse overflow"));
        }
        self.displacement = value;
        Ok(())
    }

    ///1937d0/193860 clear spring history only when this raw word changes.
    pub(super) fn set_held_raw(&mut self, value: u32) {
        if self.held_raw_d0 != value {
            self.held_raw_d0 = value;
            self.displacement = 0.0;
            self.velocity = 0.0;
            self.normalized = 0.0;
        }
    }

    /// One original integer-frame spring step. Preserve f32 operation order:
    /// clamp position, integrate acceleration into velocity, integrate position.
    pub fn step(&mut self) -> Result<()> {
        if !self.enabled {
            self.displacement = 0.0;
            self.velocity = 0.0;
            self.normalized = 0.0;
            return Ok(());
        }
        if self.held_raw_d0 != 0 {
            return Ok(());
        }
        let position = self.displacement.clamp(-self.maximum, self.maximum);
        let acceleration =
            (-(self.spring * position) - self.resistance * self.velocity) / self.mass;
        let velocity = acceleration * self.microstep + self.velocity;
        let displacement = self.microstep * velocity + position;
        let normalized = displacement / self.maximum;
        if ![velocity, displacement, normalized]
            .iter()
            .all(|n| n.is_finite())
        {
            return Err(EmoteError::InvalidResource(
                "M2 material integration overflow",
            ));
        }
        self.velocity = velocity;
        self.displacement = displacement;
        self.normalized = normalized;
        Ok(())
    }

    ///1948e0 adds the signed scaled response to the owner variable map.
    ///C0/shift is retained as an input; that field is not added by this path.
    pub fn variable_offset(&self) -> f32 {
        let value = self.normalized * self.output_scale;
        if self.mirror { -value } else { value }
    }
}

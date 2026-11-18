//! M2 force owner193090/194500 and its distinct183690/1837a0 accumulator.
//! Profile movement and observed driver variables feed the actual material
//! springs. Accumulator baselines are rebuilt each update, never fed back
//! from the last rendered frame.

use std::collections::BTreeMap;

use super::{
    CpuEmoteControllerTarget, CpuM2ControllerConfig, CpuM2Material,
    CpuNative907TransitionState, EmoteError, Result,
};

#[derive(Debug, Clone)]
struct Difference {
    observed: String,
    material: String,
    power: f32,
    reciprocal_range: f32,
    construction_maximum: f32,
    first_impulse: bool,
}

#[derive(Debug, Clone)]
pub(super) struct CpuM2Force {
    /// Aggregator+c: explicit #defval overrides, primary then secondary.
    baselines: BTreeMap<String, f32>,
    /// Aggregator+4: registered names only;1948c0 never creates a name.
    offsets: BTreeMap<String, f32>,
    ///183260 can replace a baseline after construction (for example hittest).
    overrides: BTreeMap<String, f32>,
    differences: Vec<Difference>,
    pub secondary_defaults: BTreeMap<String, f64>,
    pub profile_position: [f32; 2],
    /// Wrapper+160: set by the actual51 position command, retained by wrapper.
    pub position_linked: bool,
}

fn finite(value: f32) -> Result<f32> {
    if value.is_finite() {
        Ok(value)
    } else {
        Err(EmoteError::InvalidResource("M2 force scalar overflow"))
    }
}

impl CpuM2Force {
    pub fn new(
        config: &CpuM2ControllerConfig,
        materials: &BTreeMap<String, CpuM2Material>,
        secondary_defaults: &BTreeMap<String, f64>,
    ) -> Result<Self> {
        let mut baselines = config.default_variables()?;
        baselines.extend(secondary_defaults.clone());
        let baselines = baselines.into_iter()
            .map(|(name, value)| Ok((name, finite(value as f32)?)))
            .collect::<Result<BTreeMap<_, _>>>()?;
        let differences = config.inertia_dif.iter().map(|(observed, material, power, range)| {
            Ok(Difference {
                observed: observed.clone(),
                material: material.clone(),
                power: *power,
                reciprocal_range: finite(1.0 / *range)?,
                construction_maximum: materials.get(material).map_or(0.0, |m| m.maximum),
                first_impulse: false,
            })
        }).collect::<Result<Vec<_>>>()?;
        Ok(Self {
            offsets: baselines.clone(),
            baselines,
            overrides: BTreeMap::new(),
            differences,
            secondary_defaults: secondary_defaults.clone(),
            profile_position: [0.0; 2],
            position_linked: false,
        })
    }

    ///183690 precedes the ordinary value/chain pass on every owner update.
    pub fn prepare(&mut self) {
        for (name, value) in &mut self.offsets {
            *value = self.baselines.get(name).copied().unwrap_or(0.0);
        }
    }

    pub fn offset(&self, name: &str) -> f32 {
        self.offsets.get(name).copied().unwrap_or(0.0)
    }

    ///183260 first writes map+c and captures virtual94 into map+4 only if
    ///absent. It does not run prepare or overwrite this update's accumulator.
    pub fn set_baseline(&mut self, name: &str, value: f32,
        driver: &BTreeMap<String, f64>) -> Result<()> {
        let value = finite(value)?;
        let current = finite(*driver.get(name).unwrap_or(&0.0) as f32)?;
        self.baselines.insert(name.to_owned(), value);
        self.overrides.insert(name.to_owned(), value);
        self.offsets.entry(name.to_owned()).or_insert(current);
        Ok(())
    }

    ///The wrapper's aggregator survives a driver replacement. New explicit
    ///#defval rows take precedence; otherwise retain its later native writes.
    pub fn inherit_overrides(&mut self, previous: &Self,
        driver: &BTreeMap<String, f64>) -> Result<()> {
        for (name, value) in &previous.overrides {
            if !self.baselines.contains_key(name) {
                self.set_baseline(name, *value, driver)?;
            }
        }
        Ok(())
    }

    ///183580 adds only to a registered accumulator name.
    pub fn add_offset(&mut self, name: &str, value: f32) -> Result<()> {
        if let Some(offset) = self.offsets.get_mut(name) {
            *offset = finite(*offset + value)?;
        }
        Ok(())
    }

    ///178570/1943d0: only differing resources in the secondary's same group.
    pub fn group_change(&mut self, config: &CpuM2ControllerConfig, from: &[u8], to: &[u8]) {
        if config.changes_within_group(from, to) {
            for definition in &mut self.differences {
                if config.group_inertia.contains(&definition.observed) { definition.first_impulse = true; }
            }
        }
    }

    ///193400 resets histories only when the raw enable word changes.
    pub fn set_enable(&self, value: u32, transition: &mut CpuNative907TransitionState,
                      driver: &BTreeMap<String, f64>) -> Result<()> {
        if transition.enable_54 != value {
            self.seed_history(transition, driver)?;
            transition.previous_words_08_0c = transition.words_00_04;
        }
        transition.enable_54 = value;
        Ok(())
    }

    ///192b5c/192f0b seed histories from virtual94 plus the accumulator,
    ///including actual selector outputs rather than unevaluated base fields.
    pub fn seed_history(
        &self, transition: &mut CpuNative907TransitionState, driver: &BTreeMap<String, f64>,
    ) -> Result<()> {
        let observed = self.observed(driver)?;
        for record in transition.group_20_24.iter_mut()
            .chain(&mut transition.group_2c_30).chain(&mut transition.group_48_4c) {
            let value = *observed.get(&record.name).unwrap_or(&0.0) as f32;
            record.from = value;
            record.to = value;
        }
        Ok(())
    }

    ///1835f0 queries the current driver and adds this update's accumulator.
    pub fn observed(&self, driver: &BTreeMap<String, f64>) -> Result<BTreeMap<String, f64>> {
        let mut actual = driver.clone();
        for (name, offset) in &self.offsets {
            let value = finite(*driver.get(name).unwrap_or(&0.0) as f32 + *offset)?;
            actual.insert(name.clone(), f64::from(value));
        }
        Ok(actual)
    }

    ///193090: input tick0 is1, first interval is1, subsequent intervals use
    ///DWORD subtraction. No elapsed spring steps are capped or discarded.
    pub fn advance(
        &mut self,
        tick: u32,
        transition: &mut CpuNative907TransitionState,
        materials: &mut BTreeMap<String, CpuM2Material>,
        driver: &BTreeMap<String, f64>,
    ) -> Result<()> {
        let enabled = transition.enable_54 != 0;
        transition.previous_words_08_0c = if enabled {
            transition.words_00_04
        } else {
            self.profile_position.map(f32::to_bits)
        };
        transition.words_00_04 = self.profile_position.map(f32::to_bits);
        let now = tick.max(1);
        if transition.phase_58 == 0 {
            transition.phase_58 = now.wrapping_sub(1);
        }
        for (axis, records) in [
            (0, &mut transition.group_20_24),
            (1, &mut transition.group_2c_30),
        ] {
            let current = self.profile_position[axis];
            let previous = f32::from_bits(transition.previous_words_08_0c[axis]);
            for record in records {
                // Missing target materials are skipped by the original map
                // lookup, including the history writes inside that branch.
                if let Some(material) = materials.get_mut(&record.name) {
                    record.from = current;
                    record.to = if enabled { previous } else { current };
                    if enabled {
                        material.inject_impulse(current - previous)?;
                    }
                }
            }
        }
        let observed = self.observed(driver)?;
        //194500 samples every observed name before injecting any impulse.
        for record in &mut transition.group_48_4c {
            record.to = record.from;
            if enabled {
                record.from = *observed.get(&record.name).unwrap_or(&0.0) as f32;
            }
        }
        for (record, definition) in transition.group_48_4c.iter_mut().zip(&mut self.differences) {
            if let Some(material) = materials.get_mut(&definition.material) {
                if definition.first_impulse {
                    //194690 resets previous, consumes44 once and omits power30.
                    record.to = record.from;
                    definition.first_impulse = false;
                    material.inject_impulse(definition.construction_maximum * definition.reciprocal_range)?;
                } else if record.from != record.to {
                    //1946c9..1946d9: preserve reciprocal/max/power order.
                    let impulse = ((record.from - record.to) * definition.reciprocal_range)
                        * definition.construction_maximum * definition.power;
                    material.inject_impulse(impulse)?;
                }
            }
        }
        if enabled {
            for _ in 0..now.wrapping_sub(transition.phase_58) {
                for material in materials.values_mut() {
                    material.step()?;
                }
            }
        }
        //1948c0 adds material output only to already registered variables.
        for (name, material) in materials {
            if let Some(value) = self.offsets.get_mut(name) {
                *value = finite(*value + material.variable_offset())?;
            }
        }
        transition.phase_58 = now;
        Ok(())
    }

    ///1837a0 is after both spring and optional spline contributions.
    pub fn targets(&self, driver: &BTreeMap<String, f64>) -> Result<Vec<CpuEmoteControllerTarget>> {
        self.offsets.iter().map(|(name, offset)| {
            //1837a0 uses driver current only without an explicit baseline.
            let base = if self.baselines.contains_key(name) { 0.0 }
                else { *driver.get(name).unwrap_or(&0.0) as f32 };
            Ok(CpuEmoteControllerTarget {
                name: name.clone(), value: finite(base + *offset)?,
                duration: 0.0, easing: 0.0,
            })
        }).collect()
    }
}

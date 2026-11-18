//! Independent M2 value/chain ownership from16df30,16e520/630/770,16d250.
//! A bank writes absolute owner values after the chain pass. An initialized
//! non-fix chain retains its driver target and only queries the current
//! driver output; changing its last raw/scaled sample does not restart it.

use super::{
    CpuEmoteControllerTarget, CpuM2ChainKind, CpuM2TimedRequest, CpuM2TimedVariables, EmoteError,
    PsbValue, Result,
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone)]
struct Value {
    speed_28: f32,
    weight_2c: f32,
    current_30: f32,
    //16df30 is an absolute write. Force offsets require the distinct setter
    //16dea0; no unimplemented force output is invented here.
    offset_34: f32,
    dirty_3c: bool,
}

impl Default for Value {
    fn default() -> Self {
        Self { speed_28: 1.0, weight_2c: 1.0, current_30: 0.0,
            offset_34: 0.0, dirty_3c: false }
    }
}

#[derive(Debug, Clone)]
struct Chain {
    request: CpuM2TimedRequest,
    raw: Vec<f32>,
    scaled: Vec<f32>,
    speed_5c: f32,
    weight_60: f32,
    initialized_84: bool,
}

#[derive(Debug, Clone, Default)]
pub(super) struct CpuM2ValueTable {
    values: BTreeMap<String, Value>,
    chains: BTreeMap<String, Chain>,
}

impl CpuM2ValueTable {
    ///16e423..16e4f5 walks the retained registered-name tree. With a driver,
    ///16e461..16e478 reads virtual94 and stores its result as f32. Only an
    ///absent driver reads the live owner word+30 (16e47d), without weight,
    ///speed, force-offset, chain-target or configuration-default substitution.
    ///The supplied map must be resolved current driver samples, not load
    ///seeds or only a partial override map:133e0 may fall back to the model.
    ///An absent input-map entry follows the caller's unknown-control zero
    ///read; this value owner does not infer PSB model values from metadata.
    pub fn debug_registered_values(
        &self,
        driver: Option<&BTreeMap<String, f64>>,
    ) -> Vec<(String, f32)> {
        self.values.iter().map(|(name, value)| {
            let sample = match driver {
                Some(actual) => *actual.get(name).unwrap_or(&0.0) as f32,
                None => value.current_30,
            };
            (name.clone(), sample)
        }).collect()
    }

    ///Register existing driver controls and parsed primary/secondary names.
    pub fn register(&mut self, config: &super::CpuM2ControllerConfig,
        driver: &BTreeMap<String, f64>) -> Result<()> {
        for name in driver.keys() {
            self.values.entry(name.clone()).or_default();
        }
        for entries in config.patterns.values().flat_map(|patterns| patterns.values()) {
            for (bytes, _) in entries {
                let (name, _, invalid) = encoding_rs::SHIFT_JIS.decode(bytes);
                if invalid { return Err(EmoteError::InvalidResource("M2 registered variable CP932")); }
                let name = name.into_owned();
                self.values.entry(name).or_default();
            }
        }
        Ok(())
    }

    ///530 writes value+28 and chain+5c;532 writes value+2c and chain+60.
    ///Unknown names do not create records. All applies to current records only.
    pub fn set_scalar(&mut self, name: Option<&str>, value: f32, weight: bool) -> Result<()> {
        if !value.is_finite() {
            return Err(EmoteError::InvalidResource("nonfinite M2 owner scalar"));
        }
        for (key, entry) in &mut self.values {
            if name.is_none_or(|name| name == key) {
                if weight { entry.weight_2c = value; } else { entry.speed_28 = value; }
            }
        }
        for (key, chain) in &mut self.chains {
            if name.is_none_or(|name| name == key) {
                if weight { chain.weight_60 = value; } else { chain.speed_5c = value; }
            }
        }
        Ok(())
    }

    pub fn chain_count(&self) -> usize {
        self.chains.len()
    }

    ///17c234..17c255 releases only the pending chain map, retaining values.
    pub fn clear_chains(&mut self) { self.chains.clear(); }
    pub fn deferred_count(&self) -> usize {
        self.values.values().filter(|v| v.dirty_3c).count()
    }
    pub fn value_count(&self) -> usize {
        self.values.len()
    }

    pub fn queue_pattern(&mut self, request: &CpuM2TimedRequest) {
        let speed = self.values.get(&request.name).map_or(1.0, |v| v.speed_28);
        let weight = self.values.get(&request.name).map_or(1.0, |v| v.weight_2c);
        self.chains.insert(
            request.name.clone(),
            Chain {
                request: request.clone(),
                raw: request.samples.clone(),
                scaled: request.samples.iter().map(|value| value * weight).collect(),
                speed_5c: speed,
                weight_60: weight,
                initialized_84: false,
            },
        );
    }

    ///16df30 first compares both actual driver output and owner current.
    ///It updates the final chain samples even when no owner change is needed.
    ///Weight and speed belong to the registered owner, independently of the
    ///absolute value written by blink/lip banks.
    pub fn write_absolute(&mut self, name: &str, value: f32, driver: &BTreeMap<String, f64>) {
        let current = *driver.get(name).unwrap_or(&0.0) as f32;
        let equal = current == value
            && self
                .values
                .get(name)
                .is_none_or(|entry| entry.current_30 == value);
        if !equal {
            let entry = self.values.entry(name.to_owned()).or_default();
            entry.current_30 = value;
            entry.dirty_3c = true;
        }
        if let Some(chain) = self.chains.get_mut(name) {
            chain.request.target = value;
            if let Some(last) = chain.raw.last_mut() {
                *last = value;
            }
            if let Some(last) = chain.scaled.last_mut() {
                *last = value * chain.weight_60;
            }
        }
    }

    ///173f1f precedes173fde(bank advance) and174196(driver advance).
    ///This pass consumes prior bank writes, rather than mutating rendered
    ///driver values in the middle of the bank pass.
    pub fn begin_update(
        &mut self,
        root: &PsbValue,
        driver: &BTreeMap<String, f64>,
        variables: &mut BTreeMap<String, f64>,
        timers: &mut CpuM2TimedVariables,
        engine_zero: bool,
        nonloop_timeline_busy: bool,
        timeline_owned_names: &BTreeSet<String>,
    ) -> Result<()> {
        if self.values.len() > 100_000 || self.chains.len() > 100_000 {
            return Err(EmoteError::Capacity);
        }
        //16e770 first refreshes raw/scaled last samples for existing chains.
        //Its38 flag suppresses creation of another chain for that same name.
        for (name, value) in &mut self.values {
            if let Some(chain) = self.chains.get_mut(name) {
                chain.weight_60 = value.weight_2c;
                let raw_last = *chain
                    .raw
                    .last()
                    .ok_or(EmoteError::InvalidResource("M2 empty owned samples"))?;
                value.current_30 = raw_last;
                if let Some(last) = chain.scaled.last_mut() {
                    *last = raw_last * chain.weight_60;
                }
                value.dirty_3c = false;
            } else if value.dirty_3c {
                //16e96e creates current->absolute-target, default kind0,
                //duration0, when no existing chain owns this variable.
                self.chains.insert(
                    name.clone(),
                    Chain {
                        request: CpuM2TimedRequest {
                            name: name.clone(),
                            target: value.current_30,
                            frames: 0,
                            kind: CpuM2ChainKind::Fix,
                            samples: vec![
                                *driver.get(name).unwrap_or(&0.0) as f32,
                                value.current_30,
                            ],
                        },
                        raw: vec![*driver.get(name).unwrap_or(&0.0) as f32, value.current_30],
                        scaled: vec![
                            *driver.get(name).unwrap_or(&0.0) as f32,
                            value.current_30 * value.weight_2c,
                        ],
                        speed_5c: value.speed_28,
                        weight_60: value.weight_2c,
                        initialized_84: false,
                    },
                );
                value.dirty_3c = false;
            }
        }
        for (name, chain) in &mut self.chains {
            if !chain.raw.iter().chain(&chain.scaled).all(|v| v.is_finite()) {
                return Err(EmoteError::InvalidResource("M2 owner last sample overflow"));
            }
            //16d2d2..16d366: initialized non-fix records query virtual94
            //without reissuing virtual90, including a newly changed last
            //sample. Timelines may independently retarget the real driver.
            if chain.initialized_84 && chain.request.kind != CpuM2ChainKind::Fix {
                if let Some(value) = self.values.get_mut(name) {
                    value.current_30 = *driver.get(name).unwrap_or(&0.0) as f32;
                }
                continue;
            }
            timers.submit_targets(
                root,
                driver,
                variables,
                &[CpuEmoteControllerTarget {
                    name: name.clone(),
                    //16d43f uses actual target80, not a corrected sample.
                    //16d58b adds the distinct value-map offset34 afterwards.
                    value: chain.request.target * chain.weight_60
                        + self.values.get(name).map_or(0.0, |v| v.offset_34),
                    duration: chain.request.driver_duration_with_speed(engine_zero, chain.speed_5c)?,
                    easing: chain.request.driver_easing()?,
                }],
            )?;
            chain.initialized_84 = true;
            //16d5e6: duration0 stores the target; otherwise query the actual
            //driver value. For the supported primary lane offset is zero.
            if let Some(value) = self.values.get_mut(name) {
                value.current_30 = *variables.get(name).unwrap_or(&0.0) as f32;
            }
        }
        //16d787: disabled engine unconditionally clears. virtualf0 otherwise
        //checks actual live receiver timers, excluding active timeline-owned
        //names, and treats a non-looping timeline as busy (11f31..11f5a).
        if engine_zero
            || (!nonloop_timeline_busy && !timers.has_active_except(timeline_owned_names))
        {
            self.chains.clear();
        }
        Ok(())
    }
}

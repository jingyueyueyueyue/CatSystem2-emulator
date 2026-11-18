//! Native907's portable receiver from1744a0/174a60/174790/193400.
//!
//! Inputs are actual owned host-controller records. Missing banks are never
//! replaced with guessed defaults. Reset writes are applied to the same named
//! variables consumed by the real PSB sampler. This is not a physics evaluator.

use super::{EmoteError, Result};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy)]
pub struct CpuSceneEnableOverride {
    pub active: u32,
    pub engine_enable: u32,
    pub transition_enable: u32,
}

#[derive(Debug, Clone)]
pub struct CpuNative907VariableReset {
    pub active: bool,
    pub name: String,
    /// Original record+40; preserve the stored f32 value, not a guessed zero.
    pub value: f32,
    pub counter_60: u32,
    pub counter_64: u32,
}

#[derive(Debug, Clone, Default)]
pub struct CpuNative907VariableBank {
    pub records: Vec<CpuNative907VariableReset>,
    pub counter_20: u32,
    pub counter_24: u32,
    pub dirty: u32,
}

#[derive(Debug, Clone)]
pub struct CpuNative907PhaseReset {
    pub active: bool,
    pub phase_enabled: bool,
    pub first_phase_words: Vec<u32>,
    pub current_phase_98: u32,
    pub restart_ac: u32,
}

#[derive(Debug, Clone, Default)]
pub struct CpuNative907PhaseBank {
    pub records: Vec<CpuNative907PhaseReset>,
    pub counter_20: u32,
    pub counter_24: u32,
}

/// Resolved baseline produced by the transition controller's real owner.
/// First group values are from5835f0; other groups can add current driver
/// named-variable values to a profile baseline as in19357b/193723.
#[derive(Debug, Clone)]
pub struct CpuNative907TransitionReset {
    pub name: String,
    pub baseline: f32,
    pub add_current_variable: bool,
    pub from: f32,
    pub to: f32,
}

#[derive(Debug, Clone)]
pub struct CpuNative907TransitionState {
    pub enable_54: u32,
    pub group_20_24: Vec<CpuNative907TransitionReset>,
    pub group_2c_30: Vec<CpuNative907TransitionReset>,
    pub group_48_4c: Vec<CpuNative907TransitionReset>,
    pub words_00_04: [u32; 2],
    pub previous_words_08_0c: [u32; 2],
    pub phase_58: u32,
}

#[derive(Debug, Clone)]
pub struct CpuNative907State {
    pub scene_override_138: Option<CpuSceneEnableOverride>,
    pub engine_link_present: bool,
    pub engine_enable_10: Option<u32>,
    pub transition: Option<CpuNative907TransitionState>,
    pub variable_banks_84: Vec<CpuNative907VariableBank>,
    pub phase_banks_90: Vec<CpuNative907PhaseBank>,
    pub requested_raw_10c: Option<u32>,
}

impl CpuNative907State {
    pub(super) fn validate(&self) -> Result<()> {
        let count = self
            .variable_banks_84
            .iter()
            .map(|b| b.records.len())
            .sum::<usize>()
            .checked_add(
                self.phase_banks_90
                    .iter()
                    .map(|b| b.records.len())
                    .sum::<usize>(),
            )
            .ok_or(EmoteError::Capacity)?;
        if count > 100_000 {
            return Err(EmoteError::Capacity);
        }
        for record in self.variable_banks_84.iter().flat_map(|b| &b.records) {
            if record.name.len() > 4095 || record.name.contains('\0') || !record.value.is_finite() {
                return Err(EmoteError::InvalidResource(
                    "invalid native907 variable reset",
                ));
            }
        }
        if let Some(state) = &self.transition {
            for record in state
                .group_20_24
                .iter()
                .chain(&state.group_2c_30)
                .chain(&state.group_48_4c)
            {
                if record.name.len() > 4095
                    || record.name.contains('\0')
                    || !record.baseline.is_finite()
                {
                    return Err(EmoteError::InvalidResource(
                        "invalid native907 transition reset",
                    ));
                }
            }
        }
        Ok(())
    }

    /// Original raw DWORD resolution; nonzero input remains raw unless a
    /// present scene override substitutes the two independent effective flags.
    pub fn effective_flags(&self, requested: u32) -> [u32; 2] {
        if requested != 0 {
            if let Some(scene) = self.scene_override_138 {
                return if scene.active == 0 {
                    [0, 0]
                } else {
                    [scene.engine_enable, scene.transition_enable]
                };
            }
        }
        [requested, requested]
    }

    /// Execute the full recovered record-reset operation atomically. The
    /// caller must bind actual banks and resolved transition baselines first.
    /// No implicit empty banks and no visibility flag are introduced here.
    pub fn apply(&mut self, requested: u32, variables: &mut BTreeMap<String, f64>) -> Result<()> {
        let current = variables.clone();
        self.apply_with_current(requested, variables, &current)
    }

    /// Use the driver's currently evaluated values for groups that add them
    /// to a profile baseline. The mutable base store remains separate from
    /// additive timeline outputs, preventing cumulative difference feedback.
    pub fn apply_with_current(
        &mut self,
        requested: u32,
        variables: &mut BTreeMap<String, f64>,
        driver_current: &BTreeMap<String, f64>,
    ) -> Result<()> {
        self.validate()?;
        let mut next = self.clone();
        let mut writes = variables.clone();
        let flags = self.effective_flags(requested);
        next.set_effective_controls(flags, driver_current)?;
        for bank in &mut next.variable_banks_84 {
            bank.counter_20 = 0;
            bank.counter_24 = 0;
            for record in &mut bank.records {
                if !record.active {
                    continue;
                }
                writes.insert(record.name.clone(), f64::from(record.value));
                record.counter_60 = 0;
                record.counter_64 = 0;
                bank.dirty = 1;
            }
        }
        for bank in &mut next.phase_banks_90 {
            bank.counter_20 = 0;
            bank.counter_24 = 0;
            for record in &mut bank.records {
                if record.active && record.phase_enabled {
                    record.restart_ac = 1;
                    if let Some(&word) = record.first_phase_words.first() {
                        record.current_phase_98 = word;
                    }
                }
            }
        }
        next.requested_raw_10c = Some(requested);
        *self = next;
        *variables = writes;
        Ok(())
    }

    ///193400 is shared by907 and932. Recompute alone does not perform907's
    ///unconditional active-bank restoration or overwrite the requested word.
    pub(super) fn set_effective_controls(
        &mut self, [engine, control]: [u32; 2], driver_current: &BTreeMap<String, f64>,
    ) -> Result<()> {
        if self.engine_link_present {
            self.engine_enable_10 = Some(engine);
        }
        if let Some(state) = &mut self.transition {
            if state.enable_54 != control {
                for record in state
                    .group_20_24
                    .iter_mut()
                    .chain(&mut state.group_2c_30)
                    .chain(&mut state.group_48_4c)
                {
                    let current = if record.add_current_variable {
                        *driver_current.get(&record.name).ok_or_else(|| {
                            EmoteError::Backend(format!(
                                "native907 requires actual driver variable {}",
                                record.name
                            ))
                        })? as f32
                    } else {
                        0.0
                    };
                    let value = record.baseline + current;
                    if !value.is_finite() {
                        return Err(EmoteError::InvalidResource("native907 reset overflow"));
                    }
                    record.from = value;
                    record.to = value;
                }
                state.previous_words_08_0c = state.words_00_04;
            }
            state.enable_54 = control;
            if control == 0 {
                state.phase_58 = 0;
            }
        }
        Ok(())
    }
}

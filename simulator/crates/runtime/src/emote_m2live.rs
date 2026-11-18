//! Live selected M2 blink/lip banks. Selection184ae0/184bd0, reset1744a0,
//! and blink scheduling183f10 are independently written from original bytes.

use super::m2values::CpuM2ValueTable;
use super::m2force::CpuM2Force;
use super::{
    CpuM2BlinkDefinition, CpuM2ChainKind, CpuM2ControllerConfig, CpuM2LipCurve, CpuM2LipDefinition,
    CpuM2Material, CpuM2TimedRequest, CpuM2Waveform, CpuNative907State,
    CpuNative907TransitionReset, CpuNative907TransitionState, CpuSceneEnableOverride, EmoteError,
    Result,
};
use std::collections::BTreeMap;

#[path = "emote_m2profile.rs"]
mod profile;
#[path = "emote_m2touch.rs"]
pub(super) mod touch;
#[path = "emote_m2debug.rs"]
mod debug;
#[path = "emote_m2program.rs"]
pub(super) mod program;
pub(in crate::emote) use debug::CpuM2DebugText;

/// Prepared same-resource profile switch. The CPU owns the final publish and
/// captures the old surface before replacing either of these cloned owners.
#[derive(Debug, Clone)]
pub(super) struct CpuM2ProfileReuse {
    pub owner: CpuM2LiveOwner,
    pub variables: BTreeMap<String, f64>,
    pub surface_duration: Option<u32>,
}

/// Fully resolved user resource/config inputs. The manager owns players by
/// (real task id, internal selector); M2Plane906 always addresses selector0.
#[derive(Debug, Clone)]
pub struct CpuM2Load {
    pub profile_alias_cp932: Vec<u8>,
    pub player_name_cp932: Vec<u8>,
    pub controller_config: CpuM2ControllerConfig,
    pub pattern_selectors_cp932: Vec<Vec<u8>>,
    pub secondary_variables: BTreeMap<String, f64>,
    pub secondary_force_defaults: BTreeMap<String, f64>,
    pub secondary_config: CpuM2ControllerConfig,
    pub spline_config: std::sync::Arc<super::CpuM2SplineConfig>,
    pub action_config: std::sync::Arc<super::CpuM2ActionConfig>,
    pub material_settings: Vec<super::CpuM2MaterialSetting>,
    pub timeline_label: String,
    pub root_affine: [f64; 6],
    pub transition_raw: u32,
}

#[derive(Debug, Clone)]
struct Blink {
    definition: CpuM2BlinkDefinition,
    bank: usize,
    record: usize,
}

#[derive(Debug, Clone)]
struct Lip {
    bank: usize,
    record: usize,
    variable: String,
    base: f32,
    dimension: u32,
    pattern_cp932: Vec<u8>,
    curve: Option<CpuM2LipCurve>,
    waveform: Option<CpuM2Waveform>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CpuM2OwnerInfo {
    pub selected_patterns: Vec<Vec<u8>>,
    pub secondary_patterns: Vec<Vec<u8>>,
    pub blink_records: usize,
    pub active_blink_records: usize,
    pub lip_records: usize,
    pub active_lip_records: usize,
    pub running_lip_phases: usize,
    /// Actual zero-based dimension/CP932 pattern targets for batch sidecar
    /// binding. The host supplies recovered presets and source origin.
    pub lip_curve_targets: Vec<(u32, Vec<u8>)>,
    pub requested_enable: Option<u32>,
    pub blink_enable: u32,
    pub lip_enable: u32,
    pub scene_blink_enable: u32,
    pub scene_lip_enable: u32,
    pub material_records: usize,
    pub pending_effects: Vec<String>,
    pub pending_timed_targets: usize,
    pub waveform_evaluators: usize,
    pub bound_sidecar_evaluators: usize,
    pub failed_sidecar_evaluators: usize,
    pub owner_value_records: usize,
    pub owner_chain_records: usize,
    pub deferred_bank_writes: usize,
    ///Actual wrapper+100 enable bit; it does not imply generated wave nodes.
    pub program_enabled: bool,
}

#[derive(Debug, Clone)]
pub struct CpuM2LiveOwner {
    pub state: CpuNative907State,
    pub selected_patterns: Vec<Vec<u8>>,
    pub blink_enable: u32,
    pub lip_enable: u32,
    pub scene_blink_enable: u32,
    pub scene_lip_enable: u32,
    pub(super) config: CpuM2ControllerConfig,
    pub(super) requested_blink_enable: u32,
    pub(super) requested_lip_enable: u32,
    secondary_config: CpuM2ControllerConfig,
    secondary_patterns: Vec<Vec<u8>>,
    blink: Vec<Blink>,
    lip: Vec<Lip>,
    pub lip_definitions: Vec<CpuM2LipDefinition>,
    /// Explicit diagnostics for unsupported live effect consumers.
    pub pending_effects: Vec<String>,
    pub materials: BTreeMap<String, CpuM2Material>,
    material_settings: super::m2settings::CpuM2MaterialSettings,
    pub(super) force: CpuM2Force,
    pub(super) spline: super::m2spline::CpuM2SplineOwner,
    pub(super) action: super::m2action::CpuM2ActionOwner,
    program: program::CpuM2ProgramOwner,
    pub(super) pending_timed: Vec<CpuM2TimedRequest>,
    pub(super) value_table: CpuM2ValueTable,
}

impl CpuM2LiveOwner {
    ///933/16e3f0 snapshots every name retained by this live value owner,
    ///including old-profile names and subsequent absolute writes. Pass the
    ///actual current driver map when a driver exists; None reads owner+30.
    ///The owned UTF-8-name/f32 pairs plug directly into
    ///CpuM2DebugText::insert_bank, without advancing or clearing any owner.
    pub fn debug_registered_values(
        &self,
        driver: Option<&BTreeMap<String, f64>>,
    ) -> Vec<(String, f32)> {
        self.value_table.debug_registered_values(driver)
    }

    /// Same primary difference owner. Changed dimensions queue the recovered
    /// timed driver targets; material/transition history remains owned here.
    /// A timeline-only change rebuilds banks through m2profile before this
    /// common selector, while preserving the shared value/force owners.
    pub(super) fn reselect(
        &self,
        config: CpuM2ControllerConfig,
        requested: &[Vec<u8>],
        transition_raw: u32,
        variables: &mut BTreeMap<String, f64>,
        driver: &BTreeMap<String, f64>,
    ) -> Result<Self> {
        self.reselect_with_bank_selection(config, requested, transition_raw, variables, driver, false)
    }

    fn reselect_with_bank_selection(
        &self,
        config: CpuM2ControllerConfig,
        requested: &[Vec<u8>],
        transition_raw: u32,
        variables: &mut BTreeMap<String, f64>,
        driver: &BTreeMap<String, f64>,
        refresh_banks: bool,
    ) -> Result<Self> {
        let selected = config.pattern_selection(requested)?;
        //174210 installs the current configuration even when172610's
        //selection comparison subsequently returns without changing banks.
        //16cb00 registers names without replacing their existing owner words.
        let mut next = self.clone();
        next.value_table.register(&config, driver)?;
        next.config = config;
        if self.selected_patterns == selected && !refresh_banks {
            return Ok(next);
        }
        let mut requests = BTreeMap::<String, CpuM2TimedRequest>::new();
        let mut reset_selected_banks = false;
        for (dimension, to) in selected.iter().enumerate() {
            let from = self
                .selected_patterns
                .get(dimension)
                .map(Vec::as_slice)
                .unwrap_or(b"");
            if from == to {
                continue;
            }
            let chain = next.config.chains.select(dimension as u32, from, to)?;
            if chain.kind == CpuM2ChainKind::NoMove {
                //17329a enum6 skips1766b0 and does not set changed90.
                //The new pattern/bank selection still takes effect through
                //the common173022/1730ea/173150 selection path below.
                //Keep existing value/chain ownership, including live motion.
                continue;
            }
            reset_selected_banks = true;
            let kind = if transition_raw == 0 || self.state.requested_raw_10c == Some(0) {
                CpuM2ChainKind::Fix
            } else {
                chain.kind
            };
            for request in next.config.chain_requests(dimension as u32, &chain, kind, driver)? {
                request.driver_easing()?;
                request.driver_duration(false)?;
                //17329a calls1766b0 in dimension order.16d17b replaces
                //an existing same-name record, including samples and flag84.
                //The later dimension wins; NoMove above leaves it intact.
                requests.insert(request.name.clone(), request);
            }
        }
        //The real banks84/90 were created by177270 on resource import.
        //174210 only rebinds configs;173119/17317d select their existing
        //records. Do not reconstruct them from a replacement config or
        //overwrite sound phases, spring history or shared value/chain state.
        next.selected_patterns = selected;
        for bank in &mut next.state.variable_banks_84 {
            for record in &mut bank.records {
                record.active = false;
                record.counter_60 = 0;
                record.counter_64 = 0;
            }
        }
        for blink in &next.blink {
            next.state.variable_banks_84[blink.bank].records[blink.record].active = next
                .selected_patterns
                .get(blink.definition.dimension as usize)
                .is_some_and(|pattern| pattern == &blink.definition.pattern_cp932);
        }
        for bank in &mut next.state.phase_banks_90 {
            for record in &mut bank.records {
                record.active = false;
            }
        }
        for lip in &mut next.lip {
            next.state.phase_banks_90[lip.bank].records[lip.record].active = next
                .selected_patterns
                .get(lip.dimension as usize)
                .is_some_and(|pattern| pattern == &lip.pattern_cp932);
            if let Some(waveform) = lip.waveform.as_mut() {
                waveform.reset_timestamp();
            }
        }
        for request in requests.values() {
            next.value_table.queue_pattern(request);
        }
        next.pending_timed = self
            .pending_timed
            .iter()
            .filter(|request| !requests.contains_key(&request.name))
            .cloned()
            .collect();
        next.pending_timed.extend(requests.into_values());
        //1733d8/173416 restore/reset active banks after a changed pattern.
        //1744a0 uses16df30: stage absolute owner values, not driver writes.
        for bank in next
            .state
            .variable_banks_84
            .iter_mut()
            .filter(|_| reset_selected_banks)
        {
            bank.counter_20 = 0;
            bank.counter_24 = 0;
            for record in &mut bank.records {
                if record.active {
                    next.value_table
                        .write_absolute(&record.name, record.value, variables);
                    record.counter_60 = 0;
                    record.counter_64 = 0;
                    bank.dirty = 1;
                }
            }
        }
        for bank in next
            .state
            .phase_banks_90
            .iter_mut()
            .filter(|_| reset_selected_banks)
        {
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
        if reset_selected_banks {
            for lip in &mut next.lip {
                let record = &next.state.phase_banks_90[lip.bank].records[lip.record];
                if record.active && record.phase_enabled {
                    if let Some(waveform) = lip.waveform.as_mut() {
                        waveform.request_finish();
                    }
                }
            }
        }
        Ok(next)
    }

    pub fn new(
        config: CpuM2ControllerConfig,
        requested: &[Vec<u8>],
        variables: &mut BTreeMap<String, f64>,
        secondary_defaults: &BTreeMap<String, f64>,
    ) -> Result<Self> {
        let selected = config.pattern_selection(requested)?;
        variables.extend(config.default_variables()?);
        for (dimension, label) in selected.iter().enumerate() {
            variables.extend(config.pattern_variables(dimension as u32, label)?);
        }
        let profile::LiveBanks { variable_banks, phase_banks, blink, lip } =
            profile::new_banks(&config, &selected)?;
        let reset = |name: &String, add_current_variable: bool| CpuNative907TransitionReset {
            name: name.clone(),
            baseline: if add_current_variable {
                0.0
            } else {
                *variables.get(name).unwrap_or(&0.0) as f32
            },
            add_current_variable,
            from: *variables.get(name).unwrap_or(&0.0) as f32,
            to: *variables.get(name).unwrap_or(&0.0) as f32,
        };
        let transition = CpuNative907TransitionState {
            enable_54: 1,
            group_20_24: config.inertia_lr.iter().map(|n| reset(n, false)).collect(),
            group_2c_30: config.inertia_ud.iter().map(|n| reset(n, true)).collect(),
            group_48_4c: config
                .inertia_dif
                .iter()
                .map(|(n, _, _, _)| reset(n, true))
                .collect(),
            words_00_04: [0; 2],
            previous_words_08_0c: [0; 2],
            phase_58: 0,
        };
        let pending_effects = Vec::new();
        let mut materials = BTreeMap::new();
        for row in &config.material_rows {
            let material = CpuM2Material::from_record(row)?;
            materials.entry(material.name.clone()).or_insert(material);
        }
        let force = CpuM2Force::new(&config, &materials, secondary_defaults)?;
        let material_settings = super::m2settings::CpuM2MaterialSettings::new(config.material_groups.clone(), &materials);
        let lip_definitions = config.lip.clone();
        let mut value_table = CpuM2ValueTable::default();
        value_table.register(&config, variables)?;
        Ok(Self {
            //1803d1: default scene active/engine/transition are all1.
            //These are actual startup defaults, not fabricated empty banks.
            state: CpuNative907State {
                scene_override_138: Some(CpuSceneEnableOverride {
                    active: 1,
                    engine_enable: 1,
                    transition_enable: 1,
                }),
                engine_link_present: true,
                engine_enable_10: Some(1),
                transition: Some(transition),
                variable_banks_84: variable_banks,
                phase_banks_90: phase_banks,
                requested_raw_10c: Some(1),
            },
            selected_patterns: selected,
            blink_enable: 1,
            lip_enable: 1,
            requested_blink_enable: 1,
            requested_lip_enable: 1,
            scene_blink_enable: 1,
            scene_lip_enable: 1,
            config,
            secondary_config: CpuM2ControllerConfig::default(),
            secondary_patterns: Vec::new(),
            blink,
            lip,
            lip_definitions,
            pending_effects,
            materials,
            material_settings,
            force,
            spline: super::m2spline::CpuM2SplineOwner::default(),
            action: super::m2action::CpuM2ActionOwner::default(),
            program: program::CpuM2ProgramOwner::default(),
            pending_timed: Vec::new(),
            value_table,
        })
    }

    pub(super) fn install_secondary(&mut self, config: CpuM2ControllerConfig,
        variables: &BTreeMap<String, f64>) -> Result<()> {
        self.value_table.register(&config, variables)?;
        self.secondary_patterns = config.pattern_selection(&[])?;
        self.secondary_config = config;
        Ok(())
    }

    ///173570 uses the same1766b0/16cf70 owner as primary changes. Its first
    ///pass chooses a surface fade before any dimension submits a new chain.
    ///Run on an owned clone so failed resource/driver validation cannot publish
    ///half a dimension vector to the live task.
    pub(super) fn select_secondary(
        &mut self,
        requested: &[Vec<u8>],
        transition_raw: u32,
        surface_requested: bool,
        driver: &BTreeMap<String, f64>,
    ) -> Result<(bool, Option<u32>)> {
        if requested.len() > 4096 {
            return Err(EmoteError::Capacity);
        }
        let selected = if requested.is_empty() {
            self.secondary_config.pattern_selection(&[])?
        } else {
            requested.to_vec()
        };
        fn label(selected: &[Vec<u8>], dimension: usize) -> &[u8] {
            match selected.get(dimension).map(Vec::as_slice) {
                None | Some(b"$") => b"",
                Some(value) => value,
            }
        }
        let dimensions = selected.len().max(self.secondary_patterns.len());
        let enabled = self.state.requested_raw_10c.unwrap_or(1) != 0;
        let mut fade = None;
        let mut changed = false;
        for dimension in 0..dimensions {
            let from = label(&self.secondary_patterns, dimension);
            let to = label(&selected, dimension);
            if from == to {
                continue;
            }
            changed = true;
            let chain = self.secondary_config.chains.select(dimension as u32, from, to)?;
            //173733..173754: a later changed Fade dimension wins+a0.
            if enabled && chain.kind == CpuM2ChainKind::Fade {
                fade = Some(chain.frames);
            }
        }
        let mut requests = BTreeMap::<String, CpuM2TimedRequest>::new();
        for dimension in 0..dimensions {
            let from = label(&self.secondary_patterns, dimension);
            let to = label(&selected, dimension);
            if from == to {
                continue;
            }
            if from.is_empty() || to.is_empty() {
                //1739ab/173a86 ->1761a0. A one-sided endpoint submits
                //every enabled entry as a fixed owned record; removing
                //a dimension retains that endpoint, never an invented zero.
                let endpoint = if to.is_empty() { from } else { to };
                let exists = self.secondary_config.patterns.get(&(dimension as u32))
                    .is_some_and(|patterns| patterns.contains_key(endpoint));
                if exists {
                    for (name, value) in self.secondary_config
                        .pattern_variables(dimension as u32, endpoint)?
                    {
                        requests.insert(name.clone(), CpuM2TimedRequest {
                            name, target: value as f32, frames: 0,
                            kind: CpuM2ChainKind::Fix, samples: vec![value as f32],
                        });
                    }
                }
                continue;
            }
            //Missing registered dimensions/patterns follow173ba6/173c6e.
            let exists = self.secondary_config.patterns.get(&(dimension as u32))
                .is_some_and(|patterns| patterns.contains_key(from) && patterns.contains_key(to));
            if !exists {
                continue;
            }
            let chain = self.secondary_config.chains.select(dimension as u32, from, to)?;
            if chain.kind == CpuM2ChainKind::NoMove {
                continue;
            }
            let kind = if !enabled || transition_raw == 0 || surface_requested || fade.is_some() {
                CpuM2ChainKind::Fix
            } else { chain.kind };
            for request in self.secondary_config.chain_requests(dimension as u32, &chain, kind, driver)? {
                request.driver_easing()?;
                request.driver_duration(false)?;
                requests.insert(request.name.clone(), request);
            }
        }
        self.pending_timed.retain(|request| !requests.contains_key(&request.name));
        for request in requests.values() {
            self.value_table.queue_pattern(request);
        }
        self.pending_timed.extend(requests.into_values());
        //173c7e..173ccc replaces+78 after dispatch; primary blink/lip
        //banks, material springs, clocks and active timelines remain owned.
        self.secondary_patterns = selected;
        Ok((changed, fade))
    }

    pub(super) fn set_material_group(&mut self, setting: &super::CpuM2MaterialSetting) {
        if let Some(transition) = self.state.transition.as_mut() {
            self.material_settings.apply(setting, &mut self.materials, transition);
        }
    }

    pub fn apply_907(
        &mut self,
        raw: u32,
        variables: &mut BTreeMap<String, f64>,
        current: &BTreeMap<String, f64>,
    ) -> Result<()> {
        let actual = self.transition_baselines(current)?;
        let before = variables.clone();
        self.state.apply_with_current(raw, variables, &actual)?;
        //1744a0's active bank restoration calls16df30 rather than virtual90.
        for record in self.state.variable_banks_84.iter().flat_map(|bank| &bank.records)
            .filter(|r| r.active)
        {
            if let Some(&value) = variables.get(&record.name) {
                self.value_table.write_absolute(&record.name, value as f32, current);
                if let Some(&old) = before.get(&record.name) {
                    variables.insert(record.name.clone(), old);
                } else { variables.remove(&record.name); }
            }
        }
        for lip in &mut self.lip {
            let record = &self.state.phase_banks_90[lip.bank].records[lip.record];
            if record.active && record.phase_enabled {
                if let Some(waveform) = lip.waveform.as_mut() { waveform.request_finish(); }
            }
        }
        Ok(())
    }

    fn transition_baselines(&mut self, current: &BTreeMap<String, f64>) -> Result<BTreeMap<String, f64>> {
        //193400 resets all three histories from the actual1835f0 query.
        //The distinct accumulator remains live across enable changes.
        let observed = self.force.observed(current)?;
        if let Some(transition) = &mut self.state.transition {
            for record in &mut transition.group_20_24 {
                record.baseline = *observed.get(&record.name).unwrap_or(&0.0) as f32;
            }
            for record in transition.group_2c_30.iter_mut().chain(&mut transition.group_48_4c) {
                record.baseline = self.force.offset(&record.name);
            }
        }
        let mut actual = current.clone();
        //Driver337e0/33981 returns its zero default for absent names.
        if let Some(transition) = &self.state.transition {
            for record in transition.group_2c_30.iter().chain(&transition.group_48_4c) {
                actual.entry(record.name.clone()).or_insert(0.0);
            }
        }
        Ok(actual)
    }

    ///174dc0 preserves wrapper10c/14c/150 and masks each effective value by
    ///the current scene, then reuses real material and bank consumers.
    pub(super) fn recompute_controls(
        &mut self, settings: &[super::CpuM2MaterialSetting],
        variables: &mut BTreeMap<String, f64>, current: &BTreeMap<String, f64>,
    ) -> Result<()> {
        let requested = self.state.requested_raw_10c.unwrap_or(1);
        let flags = match self.state.scene_override_138 {
            Some(scene) if scene.active == 0 => [0, 0],
            Some(scene) => [if scene.engine_enable == 0 { 0 } else { requested },
                            if scene.transition_enable == 0 { 0 } else { requested }],
            None => [requested; 2],
        };
        self.state.validate()?;
        let actual = self.transition_baselines(current)?;
        self.state.set_effective_controls(flags, &actual)?;
        for setting in settings { self.set_material_group(setting); }
        self.set_blink_enable(self.requested_blink_enable, variables);
        self.set_lip_enable(self.requested_lip_enable, variables);
        Ok(())
    }

    pub fn info(&self) -> CpuM2OwnerInfo {
        CpuM2OwnerInfo {
            selected_patterns: self.selected_patterns.clone(),
            program_enabled: self.program.enabled(),
            secondary_patterns: self.secondary_patterns.clone(),
            blink_records: self.blink.len(),
            active_blink_records: self
                .state
                .variable_banks_84
                .iter()
                .flat_map(|b| &b.records)
                .filter(|r| r.active)
                .count(),
            lip_records: self.lip.len(),
            active_lip_records: self
                .state
                .phase_banks_90
                .iter()
                .flat_map(|b| &b.records)
                .filter(|r| r.active)
                .count(),
            running_lip_phases: self
                .state
                .phase_banks_90
                .iter()
                .flat_map(|b| &b.records)
                .filter(|r| r.active && r.phase_enabled)
                .count(),
            lip_curve_targets: self
                .lip
                .iter()
                .map(|lip| (lip.dimension, lip.pattern_cp932.clone()))
                .collect(),
            requested_enable: self.state.requested_raw_10c,
            blink_enable: self.blink_enable,
            lip_enable: self.lip_enable,
            scene_blink_enable: self.scene_blink_enable,
            scene_lip_enable: self.scene_lip_enable,
            material_records: self.materials.len(),
            pending_effects: self.pending_effects.clone(),
            pending_timed_targets: self.pending_timed.len(),
            waveform_evaluators: self.lip.iter().filter(|lip| lip.waveform.is_some()).count(),
            bound_sidecar_evaluators: self.lip.iter().filter(|lip| lip.curve.is_some()).count(),
            failed_sidecar_evaluators: self
                .lip
                .iter()
                .filter(|lip| {
                    lip.curve
                        .as_ref()
                        .is_some_and(|c| c.reopen_error().is_some())
                })
                .count(),
            owner_value_records: self.value_table.value_count(),
            owner_chain_records: self.value_table.chain_count(),
            deferred_bank_writes: self.value_table.deferred_count(),
        }
    }

    ///174c20/184440/184d70: zero restores active variables and resets bank
    ///timestamps; nonzero resets all record timestamps without inventing play.
    pub fn set_blink_enable(&mut self, raw: u32, variables: &mut BTreeMap<String, f64>) {
        self.requested_blink_enable = raw;
        let raw = if self
            .state
            .scene_override_138
            .is_some_and(|scene| scene.active == 0 || self.scene_blink_enable == 0)
        {
            0
        } else {
            raw
        };
        self.blink_enable = raw;
        for bank in &mut self.state.variable_banks_84 {
            if raw == 0 {
                bank.counter_20 = 0;
                bank.counter_24 = 0;
            }
            for record in &mut bank.records {
                if raw != 0 || record.active {
                    record.counter_60 = 0;
                    record.counter_64 = 0;
                }
                if raw == 0 && record.active {
                    self.value_table
                        .write_absolute(&record.name, record.value, variables);
                    bank.dirty = 1;
                }
            }
        }
    }

    ///184720/184cd0. External sidecars re-enable selected phases; waveform
    ///phases wait for another actual loud window after this reset.
    pub fn set_lip_enable(&mut self, raw: u32, variables: &mut BTreeMap<String, f64>) {
        self.requested_lip_enable = raw;
        let raw = if self
            .state
            .scene_override_138
            .is_some_and(|scene| scene.active == 0 || self.scene_lip_enable == 0)
        {
            0
        } else {
            raw
        };
        self.lip_enable = raw;
        for lip in &mut self.lip {
            let record = &mut self.state.phase_banks_90[lip.bank].records[lip.record];
            if raw == 0 && record.active {
                self.value_table
                    .write_absolute(&lip.variable, lip.base, variables);
            }
            record.phase_enabled = raw != 0 && record.active && lip.curve.is_some();
            if let Some(waveform) = lip.waveform.as_mut() {
                waveform.disable();
            }
            if raw != 0 {
                record.current_phase_98 = 0;
                record.restart_ac = 0;
            }
        }
        if raw == 0 {
            for bank in &mut self.state.phase_banks_90 {
                bank.counter_20 = 0;
                bank.counter_24 = 0;
            }
        }
    }

    /// Actual random stream is supplied by the host. Original183f72 consumes
    /// one integer only when an active bank's two timestamps are equal.
    pub fn advance_blink(
        &mut self,
        now: u32,
        variables: &mut BTreeMap<String, f64>,
        random: &mut dyn FnMut() -> u32,
    ) -> Result<()> {
        if self.blink_enable == 0 {
            return Ok(());
        }
        for blink in &self.blink {
            let bank = &mut self.state.variable_banks_84[blink.bank];
            let record = &mut bank.records[blink.record];
            if !record.active {
                continue;
            }
            let definition = &blink.definition;
            let duration = definition.speed as u32;
            if duration == 0 || definition.speed != duration as f32 {
                return Err(EmoteError::InvalidResource(
                    "M2 blink duration must be original integer frames",
                ));
            }
            if record.counter_60 == record.counter_64 {
                let span = definition
                    .interval_max
                    .wrapping_sub(definition.interval_min);
                let modulus = if span == 0 { 1 } else { span };
                record.counter_60 = now
                    .wrapping_add(random() % modulus)
                    .wrapping_add(definition.interval_min);
                record.counter_64 = record.counter_60.wrapping_add(duration);
            }
            if record.counter_60 > now {
                continue;
            }
            if record.counter_64 <= now {
                self.value_table
                    .write_absolute(&record.name, record.value, variables);
                record.counter_64 = record.counter_60;
            } else {
                let phase = (now.wrapping_sub(record.counter_60) as f32 / duration as f32) * 2.0;
                let value = if phase <= 1.0 {
                    (definition.end - definition.begin) * phase + definition.begin
                } else {
                    (definition.begin - definition.end) * (phase - 1.0) + definition.end
                };
                self.value_table
                    .write_absolute(&record.name, value, variables);
            }
            bank.dirty = 1;
        }
        Ok(())
    }

    ///External-evaluator path183431c; no guessed audio amplitude is supplied.
    pub(super) fn lip_curve(
        &self,
        dimension: u32,
        pattern: &[u8],
    ) -> Result<Option<&CpuM2LipCurve>> {
        self.lip
            .iter()
            .find(|lip| lip.dimension == dimension && lip.pattern_cp932 == pattern)
            .map(|lip| lip.curve.as_ref())
            .ok_or(EmoteError::InvalidResource("M2 lip curve target is absent"))
    }

    pub fn bind_lip_curve(
        &mut self,
        dimension: u32,
        pattern: &[u8],
        curve: CpuM2LipCurve,
    ) -> Result<()> {
        let lip = self
            .lip
            .iter_mut()
            .find(|lip| lip.dimension == dimension && lip.pattern_cp932 == pattern)
            .ok_or(EmoteError::InvalidResource("M2 lip curve target is absent"))?;
        //184ee0/1754a1 only construct an evaluator when b4 is null.
        //A different existing evaluator class is not silently replaced.
        if lip
            .curve
            .as_ref()
            .is_some_and(|old| old.sidecar_kind() != curve.sidecar_kind())
        {
            return Err(EmoteError::Unsupported {
                native: 924,
                reason: "sidecar import must preserve the existing LV/LPD virtual receiver class",
            });
        }
        lip.curve = Some(curve);
        let record = &mut self.state.phase_banks_90[lip.bank].records[lip.record];
        record.phase_enabled = record.active && self.lip_enable != 0;
        record.current_phase_98 = 0;
        record.restart_ac = 0;
        Ok(())
    }

    ///184f1e skips184f90 on failed virtual open. Preserve the current phase
    ///and flow timestamps, but publish the destructively cleared receiver.
    pub(super) fn retain_failed_lip_open(
        &mut self,
        dimension: u32,
        pattern: &[u8],
        curve: CpuM2LipCurve,
    ) -> Result<()> {
        let lip = self
            .lip
            .iter_mut()
            .find(|lip| lip.dimension == dimension && lip.pattern_cp932 == pattern)
            .ok_or(EmoteError::InvalidResource("M2 lip curve target is absent"))?;
        lip.curve = Some(curve);
        Ok(())
    }

    pub(super) fn sidecar_diagnostics(&self) -> impl Iterator<Item = String> + '_ {
        self.lip.iter().filter_map(|lip| {
            lip.curve.as_ref()?.reopen_error().map(|error| {
                format!(
                    "M2 lip dimension{} pattern{:x?} failed reopen: {error}",
                    lip.dimension, lip.pattern_cp932
                )
            })
        })
    }

    pub(super) fn profile_sidecar_bindings(
        &self,
        kind: super::CpuM2LipSidecarKind,
    ) -> Result<Vec<(super::CpuM2LipBinding, u32)>> {
        self.lip
            .iter()
            .map(|lip| {
                //175460 LV never applies175270's clipper override loop.
                let (preset, origin) = if kind == super::CpuM2LipSidecarKind::Lpd {
                    self.config
                        .clipper
                        .select(lip.dimension, &lip.pattern_cp932)?
                } else {
                    lip.curve
                        .as_ref()
                        .map(|curve| (curve.preset().clone(), curve.origin_tick))
                        .unwrap_or_else(|| (super::CpuM2LipPreset::default(), 0))
                };
                Ok((
                    super::CpuM2LipBinding {
                        dimension: lip.dimension,
                        pattern_cp932: lip.pattern_cp932.clone(),
                        preset,
                    },
                    origin,
                ))
            })
            .collect()
    }

    pub(super) fn retain_lip_evaluators_from(&mut self, previous: &Self) {
        for lip in &mut self.lip {
            if let Some(source) = previous.lip.iter().find(|source| {
                source.dimension == lip.dimension && source.pattern_cp932 == lip.pattern_cp932
            }) {
                lip.curve = source.curve.clone();
                let record = &mut self.state.phase_banks_90[lip.bank].records[lip.record];
                record.phase_enabled = lip.curve.is_some() && self.lip_enable != 0 && record.active;
            }
        }
    }

    pub fn advance_lip(
        &mut self,
        now: Option<u32>,
        variables: &mut BTreeMap<String, f64>,
    ) -> Result<()> {
        if self.lip_enable == 0 {
            for bank in &mut self.state.phase_banks_90 {
                bank.counter_20 = bank.counter_24;
            }
            return Ok(());
        }
        for lip in &mut self.lip {
            let record = &mut self.state.phase_banks_90[lip.bank].records[lip.record];
            if !record.active || !record.phase_enabled {
                continue;
            }
            let now = now.ok_or(EmoteError::Unsupported {
                native: 924,
                reason: "bound lip evaluator requires the actual independent sound-task clock",
            })?;
            let value = if let Some(curve) = lip.curve.as_ref() {
                curve.sample_checked(now)?
            } else if let Some(waveform) = lip.waveform.as_mut() {
                let value = waveform.sample(now)?;
                record.phase_enabled = waveform.enabled;
                value
            } else {
                return Err(EmoteError::InvalidResource(
                    "active M2 lip phase has no real evaluator",
                ));
            };
            if !value.is_finite() {
                return Err(EmoteError::InvalidResource("M2 lip value overflow"));
            }
            self.value_table
                .write_absolute(&lip.variable, value, variables);
        }
        //184410 commits after the evaluator pass, not when174610 observes.
        for bank in &mut self.state.phase_banks_90 {
            bank.counter_20 = bank.counter_24;
        }
        Ok(())
    }

    ///174610 consumes the original window once per changed sample1. The bank
    ///retains both old/new raw values; no scalar meter is repeated into samples.
    pub fn update_waveform(&mut self, samples: &[u32], original_extent: u32) -> Result<usize> {
        if original_extent <= 1 {
            return Ok(0);
        }
        let current = *samples
            .get(1)
            .ok_or(EmoteError::InvalidResource("M2 waveform sample1"))?;
        let mut next = self.clone();
        let mut changed_banks = Vec::new();
        for (index, bank) in next.state.phase_banks_90.iter_mut().enumerate() {
            bank.counter_24 = current;
            if bank.counter_20 != current {
                changed_banks.push(index);
            }
        }
        let mut updated = 0;
        for lip in &mut next.lip {
            let record = &mut next.state.phase_banks_90[lip.bank].records[lip.record];
            if !record.active || !changed_banks.contains(&lip.bank) {
                continue;
            }
            let definition = next
                .lip_definitions
                .iter()
                .find(|definition| {
                    definition.dimension == lip.dimension
                        && definition.pattern_cp932 == lip.pattern_cp932
                })
                .ok_or(EmoteError::InvalidResource(
                    "M2 waveform actual definition absent",
                ))?;
            if lip.waveform.is_none() {
                lip.waveform = Some(CpuM2Waveform::new(definition.clone(), lip.base)?);
            }
            let waveform = lip
                .waveform
                .as_mut()
                .ok_or(EmoteError::InvalidResource("M2 waveform owner"))?;
            //174610 does not check b4: its threshold path can activate a
            //retained sidecar receiver.184150 still samples that receiver.
            waveform.enabled = record.phase_enabled;
            waveform.observe(samples, original_extent)?;
            record.phase_enabled = waveform.enabled && next.lip_enable != 0;
            updated += 1;
        }
        *self = next;
        Ok(updated)
    }

    pub(super) fn advance_force(
        &mut self, tick: u32, driver: &BTreeMap<String, f64>,
    ) -> Result<Vec<super::CpuEmoteControllerTarget>> {
        let transition = self.state.transition.as_mut()
            .ok_or(EmoteError::InvalidResource("M2 force transition owner"))?;
        //173f3e..173f7f suppresses force until a51 position command linked it.
        if !self.force.position_linked { self.force.set_enable(0, transition, driver)?; }
        self.force.advance(tick, transition, &mut self.materials, driver)?;
        if !self.force.position_linked {
            self.force.set_enable(self.state.requested_raw_10c.unwrap_or(1), transition, driver)?;
        }
        if self.state.scene_override_138.is_some_and(|scene| scene.active != 0 && scene.transition_enable != 0) {
            self.spline.advance(tick, &self.materials, &mut self.force)?;
        }
        self.force.targets(driver)
    }
    ///174790 requests a final waveform interval;174870 restores actual base
    ///through the actual owner when the linked source no longer plays.
    pub fn control_waveform(
        &mut self,
        immediate_stop: bool,
        variables: &mut BTreeMap<String, f64>,
    ) {
        for bank in &mut self.state.phase_banks_90 {
            bank.counter_20 = 0;
            bank.counter_24 = 0;
        }
        for lip in &mut self.lip {
            let record = &mut self.state.phase_banks_90[lip.bank].records[lip.record];
            if !record.active {
                continue;
            }
            //174870 restores every active record, including b4 sidecars.
            //174790 only requests a final interval; sidecar virtual sampling
            //ignores ac and remains live until the actual source stops.
            if immediate_stop {
                self.value_table
                    .write_absolute(&lip.variable, lip.base, variables);
                record.phase_enabled = false;
            } else if record.phase_enabled {
                record.restart_ac = 1;
                record.current_phase_98 = lip.base.to_bits();
            }
            if let Some(waveform) = lip.waveform.as_mut() {
                if immediate_stop {
                    waveform.disable();
                } else if record.phase_enabled {
                    waveform.request_finish();
                }
            }
        }
    }
}

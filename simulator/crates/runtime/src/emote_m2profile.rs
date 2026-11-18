//! Same-PSB M2 profile rebinding:174210 ->172610 ->1766b0/16cf70.
//! A retained resource keeps its driver/value/force owners. A changed timeline
//! specifically releases and reconstructs blink/lip banks (171d50), independent
//! of906's transition word. A different base object remains explicit.

use super::{Blink, Lip, CpuM2LiveOwner, CpuM2Load, CpuM2ProfileReuse};
use super::super::{CpuM2ChainKind, EmoteError, Result};
use std::collections::{BTreeMap, BTreeSet};

fn unsupported(reason: &'static str) -> EmoteError {
    EmoteError::Unsupported { native: 906, reason }
}

fn label(patterns: &[Vec<u8>], dimension: usize) -> &[u8] {
    match patterns.get(dimension).map(Vec::as_slice) {
        None | Some(b"$") => b"",
        Some(value) => value,
    }
}

impl CpuM2LiveOwner {
    ///926/534 ->17dd79 reads secondary config+68, not current selection+78.
    ///The returned CP932 bytes exclude the destination's trailing NUL. Bind
    ///configs with from_expanded_registered_defaults before exposing534.
    pub fn secondary_default_pattern_query_cp932(&self) -> Result<Vec<u8>> {
        self.secondary_config.registered_default_pattern_query_cp932()
    }

    /// Prepare the concrete retained-resource path without touching the live
    /// CPU player. Call only after exact resource-name equality is established.
    /// The CPU captures any requested old RGBA before publishing this result;
    /// it keeps the same PSB player and driver timers. On a changed timeline,
    /// the CPU's existing stop/play-label path must run before publication.
    ///
    /// Static real inputs:ca01/ca01l share five body PSZs and the body.dif
    /// secondary owner. Its sixteen #smooth names use actual driver samples
    /// through the existing chain_requests, not constructor default values.
    pub(in crate::emote) fn prepare_profile_reuse(
        &self,
        load: &CpuM2Load,
        previous_player_name: &[u8],
        previous_timeline: &str,
        variables: &BTreeMap<String, f64>,
        driver: &BTreeMap<String, f64>,
        surface_enabled: bool,
        default_surface_duration: u32,
    ) -> Result<CpuM2ProfileReuse> {
        //171be0 can reopen the driver when the base character changes.
        //No supplied ca01/ca01l shared-resource pair exercises that branch.
        if previous_player_name != load.player_name_cp932.as_slice() {
            return Err(unsupported(
                "M2 retained-resource profile changes the base object; rebuild transition is not recovered",
            ));
        }
        let timeline_changed = previous_timeline != load.timeline_label.as_str();
        let selected = load.controller_config.pattern_selection(&load.pattern_selectors_cp932)?;
        if selected.len() != self.selected_patterns.len() {
            return Err(unsupported(
                "M2 retained-resource profile changes the primary dimension layout",
            ));
        }
        //172aa6/172ba8 prepass is independent of the load transition word.
        //An empty endpoint uses constructor+a4; a later Fade chain overrides
        //the duration. NoMove leaves the old shared chain untouched.
        let mut surface_duration = None;
        if surface_enabled && self.state.requested_raw_10c.unwrap_or(1) != 0 {
            for dimension in 0..selected.len() {
                let from = label(&self.selected_patterns, dimension);
                let to = label(&selected, dimension);
                if from == to {
                    continue;
                }
                if from.is_empty() || to.is_empty() {
                    surface_duration = Some(default_surface_duration);
                    continue;
                }
                let chain = load.controller_config.chains.select(dimension as u32, from, to)?;
                if chain.kind == CpuM2ChainKind::Fade {
                    surface_duration = Some(chain.frames);
                }
            }
        }
        let mut configured = self.clone();
        //174243/174246 assign configs, then1742f2/174372 register names.
        //Do not directly seed driver variables, clear secondary selection+78,
        //reset weight/speed or reconstruct the force aggregator.171d50 below
        //stages #defval only if the stored timeline actually changes.
        configured.value_table.register(&load.controller_config, driver)?;
        configured.value_table.register(&load.secondary_config, driver)?;
        configured.secondary_config = load.secondary_config.clone();
        if timeline_changed {
            configured.rebuild_profile_timeline_banks(&load.controller_config, driver)?;
        }
        let mut variables = variables.clone();
        let owner = configured.reselect_with_bank_selection(
            load.controller_config.clone(),
            &load.pattern_selectors_cp932,
            //1732a5: a captured surface forces the scalar chain to Fix.
            //Its actual image interpolation remains owned by CpuM2Surface.
            if surface_duration.is_some() { 0 } else { load.transition_raw },
            &mut variables,
            driver,
            //171e89 sets110: even equal primary labels must select the
            //new banks.172e36 still skips equal scalar endpoints, so do
            //not fabricate a new chain or restart retained driver timers.
            timeline_changed,
        )?;
        //The retained secondary vector is intentionally not reselected here:
        //171a00's equal resource vector skips the rebuild/forced173570 call.
        //A later917 still uses select_secondary on this newly bound config,
        //the same live value table and the same actual driver samples.
        Ok(CpuM2ProfileReuse { owner, variables, surface_duration })
    }

    ///171d50 changes only the stored timeline, default owner values and the
    ///two bank vectors. It never runs172070's full resource reconstruction.
    fn rebuild_profile_timeline_banks(
        &mut self,
        config: &super::CpuM2ControllerConfig,
        driver: &BTreeMap<String, f64>,
    ) -> Result<()> {
        //171e10..171e39 enumerates187700's #defval vector, staging through
        //16df30. The existing value table preserves speed/weight and chains.
        for (name, value) in config.default_variables()? {
            self.value_table.write_absolute(&name, value as f32, driver);
        }
        //177a10 restores active blink bases before deleting old banks;
        //178080 releases lip/waveform/sidecar owners rather than copying
        //their playback phase into a different timeline.
        for record in self.state.variable_banks_84.iter().flat_map(|bank| &bank.records) {
            if record.active {
                self.value_table.write_absolute(&record.name, record.value, driver);
            }
        }
        let banks = new_banks(config, &[])?;
        self.state.variable_banks_84 = banks.variable_banks;
        self.state.phase_banks_90 = banks.phase_banks;
        self.blink = banks.blink;
        self.lip = banks.lip;
        self.lip_definitions = config.lip.clone();
        Ok(())
    }
}

impl super::CpuM2ControllerConfig {
    ///188980/188d08 stores the fourth header token's exact lower-case
    ///"default" comparison in pattern-node+58.187400 scans those nodes in
    ///byte-key order, then falls back to the first registered pattern or "$".
    ///Normalize this metadata while retaining the existing shared decoder for
    ///variables/chains/banks/materials. Native926 never reads selected_patterns.
    pub fn from_expanded_registered_defaults<'a>(
        configs: impl IntoIterator<Item = &'a super::super::CpuM2Config>,
    ) -> Result<Self> {
        let configs: Vec<_> = configs.into_iter().collect();
        let mut config = Self::from_expanded(configs.iter().copied())?;
        let mut dimensions = BTreeSet::new();
        let mut flags = BTreeMap::<u32, BTreeMap<Vec<u8>, bool>>::new();
        let dimension = |bytes: &[u8]| -> Result<u32> {
            std::str::from_utf8(bytes).ok().and_then(|value| value.parse::<u32>().ok())
                .and_then(|value| value.checked_sub(1))
                .filter(|&value| value < 4096)
                .ok_or(EmoteError::InvalidResource("M2 registered pattern dimension"))
        };
        for source in configs {
            for row in source.records(b"dif") {
                let key = row.first().ok_or(EmoteError::InvalidResource("M2 dif declaration"))?;
                dimensions.insert(dimension(key)?);
            }
            for row in source.records(b"pattern") {
                let Some(key) = row.first().filter(|key| key.first() == Some(&b'%')) else {
                    continue;
                };
                let index = dimension(&key[1..])?;
                let label = row.get(1).ok_or(EmoteError::InvalidResource("M2 pattern header"))?;
                //18ee60 inserts a registered node without replacing a prior
                //same-key node. Multiple default flags are legal:187480
                //chooses the first marked node in map order, not file order.
                flags.entry(index).or_default().entry(label.clone())
                    .or_insert(row.get(3).is_some_and(|word| word.as_slice() == b"default"));
            }
        }
        config.default_patterns.clear();
        //188c97 ignores headers whose #dif dimension is not registered.
        config.patterns.retain(|index, _| dimensions.contains(index));
        //+34 is map size, not a stored fallback/default label. Keep empty
        //declared dimensions in the registry, including sparse dimension IDs.
        for index in dimensions {
            let patterns = config.patterns.entry(index).or_default();
            let marked = flags.get(&index).and_then(|flags| flags.iter()
                .find(|(key, marked)| **marked && patterns.contains_key(*key))
                .map(|(key, _)| key.clone()));
            if let Some(label) = marked.or_else(|| patterns.keys().next().cloned()) {
                config.default_patterns.insert(index, label);
            }
        }
        Ok(config)
    }

    ///187400 appends one label per registered dimension, with no padding for
    ///missing dimension IDs.17ddd0 joins them with raw commas into the output.
    ///Requires the canonical registered-default constructor above, because the
    ///legacy from_expanded discards multiple flags and treats DEFAULT loosely.
    pub fn registered_default_pattern_query_cp932(&self) -> Result<Vec<u8>> {
        let mut output = Vec::new();
        for (ordinal, (dimension, patterns)) in self.patterns.iter().enumerate() {
            let label = self.default_patterns.get(dimension)
                .or_else(|| patterns.keys().next()).map(Vec::as_slice).unwrap_or(b"$");
            if label.len() > 4095 || label.contains(&0) {
                return Err(EmoteError::InvalidResource("M2 registered pattern query label"));
            }
            if ordinal != 0 {
                output.push(b',');
            }
            output.extend_from_slice(label);
        }
        Ok(output)
    }
}

/// Bank-only construction shared by initial import and171e65..171e7c.
/// It does not construct/reset material, force, secondary or value owners.
pub(super) struct LiveBanks {
    pub(super) variable_banks: Vec<super::super::CpuNative907VariableBank>,
    pub(super) phase_banks: Vec<super::super::CpuNative907PhaseBank>,
    pub(super) blink: Vec<super::Blink>,
    pub(super) lip: Vec<super::Lip>,
}

pub(super) fn new_banks(
    config: &super::CpuM2ControllerConfig,
    selected: &[Vec<u8>],
) -> Result<LiveBanks> {
    let seeds = config.reset_bank_seeds()?;
    let mut variable_banks = Vec::new();
    let mut phase_banks = Vec::new();
    let mut blink = Vec::new();
    let mut lip = Vec::new();
    for (dimension, mut bank) in seeds.variable_banks {
        let mut definitions = BTreeMap::new();
        for definition in &config.blink {
            if definition.dimension == dimension
                && config
                    .patterns
                    .get(&dimension)
                    .and_then(|p| p.get(&definition.pattern_cp932))
                    .is_some_and(|v| !v.is_empty())
            {
                definitions
                    .entry(definition.pattern_cp932.clone())
                    .or_insert(definition.clone());
            }
        }
        for (record, definition) in bank.records.iter_mut().zip(definitions.values()) {
            //184ae0 clears every record and activates only this key.
            record.active = selected
                .get(dimension as usize)
                .is_some_and(|label| label == &definition.pattern_cp932);
            record.counter_60 = 0;
            record.counter_64 = 0;
        }
        let bank_index = variable_banks.len();
        for (record, definition) in definitions.into_values().enumerate() {
            blink.push(Blink {
                definition,
                bank: bank_index,
                record,
            });
        }
        variable_banks.push(bank);
    }
    for (dimension, mut bank) in seeds.phase_banks {
        let mut definitions = BTreeMap::new();
        for definition in &config.lip {
            if definition.dimension == dimension
                && config
                    .patterns
                    .get(&dimension)
                    .and_then(|p| p.get(&definition.pattern_cp932))
                    .is_some_and(|v| !v.is_empty())
            {
                definitions
                    .entry(definition.pattern_cp932.clone())
                    .or_insert(definition);
            }
        }
        for (index, (record, definition)) in bank
            .records
            .iter_mut()
            .zip(definitions.values())
            .enumerate()
        {
            record.active = selected
                .get(dimension as usize)
                .is_some_and(|label| label == &definition.pattern_cp932);
            // No sound-root/flow has been connected at construction.
            //184bd0 sets active, not phase_enabled.924 must supply audio.
            record.phase_enabled = false;
            let first = config
                .patterns
                .get(&dimension)
                .and_then(|p| p.get(&definition.pattern_cp932))
                .and_then(|v| v.first())
                .ok_or(EmoteError::InvalidResource("M2 lip pattern variable"))?;
            let variable = definition
                .linked_variable_cp932
                .as_ref()
                .unwrap_or(&first.0);
            let (variable, _, invalid) = encoding_rs::SHIFT_JIS.decode(variable);
            if invalid {
                return Err(EmoteError::InvalidResource("M2 lip variable CP932"));
            }
            lip.push(Lip {
                bank: phase_banks.len(),
                record: index,
                variable: variable.into_owned(),
                base: definition.values.first().copied().unwrap_or(first.1),
                dimension,
                pattern_cp932: definition.pattern_cp932.clone(),
                curve: None,
                waveform: None,
            });
        }
        phase_banks.push(bank);
    }
    Ok(LiveBanks { variable_banks, phase_banks, blink, lip })
}

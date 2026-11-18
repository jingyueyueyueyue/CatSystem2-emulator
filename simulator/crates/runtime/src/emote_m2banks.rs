//! Actual M2 #pattern/#blink/#lip_* import from the selected difference data.
//!
//! EXE177c10 reads primary profile+50 (#blink);178190 reads+58 (#lip_table
//! and #lip_flow). These are owned construction inputs. Constructor records
//! alone do not establish live enable/phase/transition state for native907.

use std::collections::{BTreeMap, BTreeSet};

use super::{
    CpuM2ChainKind, CpuM2ChainSelection, CpuM2Config, CpuM2PatternChains, CpuM2TimedRequest,
    CpuNative907PhaseBank, CpuNative907PhaseReset, CpuNative907VariableBank,
    CpuNative907VariableReset, EmoteError, Result,
};

type PatternValues = Vec<(Vec<u8>, f32)>;
type Patterns = BTreeMap<u32, BTreeMap<Vec<u8>, PatternValues>>;

fn invalid(why: &'static str) -> EmoteError {
    EmoteError::InvalidResource(why)
}
fn u32_word(bytes: &[u8]) -> Result<u32> {
    std::str::from_utf8(bytes)
        .ok()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| invalid("M2 control unsigned integer"))
}
fn scalar(bytes: &[u8]) -> Result<f32> {
    std::str::from_utf8(bytes)
        .ok()
        .and_then(|s| s.parse::<f32>().ok())
        .filter(|n| n.is_finite())
        .ok_or_else(|| invalid("M2 control finite scalar"))
}
fn name(bytes: &[u8]) -> Result<String> {
    let (name, _, errors) = encoding_rs::SHIFT_JIS.decode(bytes);
    if errors || name.is_empty() || name.len() > 4095 || name.contains('\0') {
        return Err(invalid("M2 control variable name"));
    }
    Ok(name.into_owned())
}

#[derive(Debug, Clone)]
pub struct CpuM2BlinkDefinition {
    /// Original parser subtracts1 for the player's zero-based dimension.
    pub dimension: u32,
    pub pattern_cp932: Vec<u8>,
    pub begin: f32,
    pub end: f32,
    pub speed: f32,
    pub interval_min: u32,
    pub interval_max: u32,
}

#[derive(Debug, Clone)]
pub struct CpuM2LipFlowStep {
    pub pattern: u32,
    pub frames: f32,
    pub finish_check: u32,
}

#[derive(Debug, Clone)]
pub struct CpuM2LipDefinition {
    pub dimension: u32,
    pub pattern_cp932: Vec<u8>,
    pub interpolation_cp932: Vec<u8>,
    pub level_min: f32,
    pub level_max: f32,
    pub values: Vec<f32>,
    pub linked_variable_cp932: Option<Vec<u8>>,
    pub flow: Vec<CpuM2LipFlowStep>,
}

/// Initial records proven by183ac0/183cf0. The actual per-player owner must
/// evaluate selection/current flags before promoting these to live907 banks.
#[derive(Debug, Clone)]
pub struct CpuM2ResetBankSeeds {
    pub variable_banks: BTreeMap<u32, CpuNative907VariableBank>,
    pub phase_banks: BTreeMap<u32, CpuNative907PhaseBank>,
    pub blink: Vec<CpuM2BlinkDefinition>,
    pub lip: Vec<CpuM2LipDefinition>,
}

#[derive(Debug, Clone, Default)]
pub struct CpuM2ControllerConfig {
    pub(super) patterns: Patterns,
    ///Secondary profile+88 is independently parsed for965 program directives.
    pub(super) program_input: super::m2live::program::CpuM2ProgramInput,
    ///1898a0 constructs config+48;187760 uses a signed exact key lookup.
    pub(super) attributes: BTreeMap<i32, [u32; 3]>,
    defaults: BTreeMap<Vec<u8>, f32>,
    pub(super) default_patterns: BTreeMap<u32, Vec<u8>>,
    pub(super) dimensions: usize,
    pub(super) inertia_lr: Vec<String>,
    pub(super) inertia_ud: Vec<String>,
    pub(super) inertia_dif: Vec<(String, String, f32, f32)>,
    pub(super) group_inertia: Vec<String>,
    ///18bff0 appends the first token of each #hittest row to profile+194.
    pub(super) hittest: Vec<String>,
    ///18b490 builds config+1a0;1758e0 queries the primary config+64.
    pub(super) touch_descriptors: super::m2live::touch::CpuM2TouchRegistry,
    resource_groups: BTreeMap<Vec<u8>, usize>,
    pub(super) material_rows: Vec<Vec<Vec<u8>>>,
    pub(super) material_groups: super::m2settings::CpuM2MaterialGroups,
    same: BTreeMap<String, Vec<f32>>,
    start: BTreeMap<String, Vec<(f32, f32)>>,
    smooth: BTreeSet<String>,
    pub(super) clipper: super::m2presets::CpuM2ClipperProfile,
    pub chains: CpuM2PatternChains,
    pub blink: Vec<CpuM2BlinkDefinition>,
    pub lip: Vec<CpuM2LipDefinition>,
}

impl CpuM2ControllerConfig {
    /// Configs must already be expanded in actual include/record order.
    /// Duplicate defval names retain the first value as in187f..188120.
    pub fn from_expanded<'a>(configs: impl IntoIterator<Item = &'a CpuM2Config>) -> Result<Self> {
        let mut output = Self::default();
        let configs: Vec<_> = configs.into_iter().collect();
        if configs.len() > 128 {
            return Err(EmoteError::Capacity);
        }
        output.program_input =
            super::m2live::program::CpuM2ProgramInput::from_expanded(configs.iter().copied());
        output.touch_descriptors =
            super::m2live::touch::CpuM2TouchRegistry::from_expanded(configs.iter().copied())?;
        output.chains = CpuM2PatternChains::from_expanded(configs.iter().copied())?;
        output.material_groups = super::m2settings::CpuM2MaterialGroups::from_expanded(configs.iter().copied())?;
        output.clipper =
            super::m2presets::CpuM2ClipperProfile::from_expanded(configs.iter().copied())?;
        let total_records = configs.iter().try_fold(0usize, |total, config| {
            config.sections.values().try_fold(total, |n, rows| {
                n.checked_add(rows.len())
                    .filter(|n| *n <= 100_000)
                    .ok_or(EmoteError::Capacity)
            })
        })?;
        let _ = total_records;
        for config in &configs {
            for row in config.records(b"attribute") {
                if row.is_empty() { continue; }
                let integer = |bytes: &[u8]| -> Result<i32> {
                    std::str::from_utf8(bytes).ok().and_then(|s| s.parse().ok())
                        .ok_or_else(|| invalid("M2 attribute signed DWORD"))
                };
                let key = integer(&row[0])?;
                let mut values = [0; 3];
                for (value, bytes) in values.iter_mut().zip(&row[1..]) {
                    *value = integer(bytes)? as u32;
                }
                //18f010 is unique insertion: repeated keys keep the first row.
                output.attributes.entry(key).or_insert(values);
            }
            for row in config.records(b"group") {
                let group = output.resource_groups.values().copied().max().map_or(0, |value| value + 1);
                for resource in row { output.resource_groups.insert(resource.clone(), group); }
            }
            for row in config.records(b"group_inertia") {
                let first = row.first().ok_or_else(|| invalid("M2 group_inertia record"))?;
                output.group_inertia.push(name(first)?);
            }
            for row in config.records(b"hittest") {
                if let Some(first) = row.first() {
                    output.hittest.push(name(first)?);
                }
            }
            //1893f0/1895a0/189740 populate profile+30/+38/+40.
            for row in config.records(b"same") {
                if row.len() < 2 {
                    return Err(invalid("M2 same record"));
                }
                output.same.entry(name(&row[0])?).or_default().extend(
                    row[1..]
                        .iter()
                        .map(|v| scalar(v))
                        .collect::<Result<Vec<_>>>()?,
                );
            }
            for row in config.records(b"start") {
                if row.len() < 3 {
                    return Err(invalid("M2 start record"));
                }
                output
                    .start
                    .entry(name(&row[0])?)
                    .or_default()
                    .push((scalar(&row[1])?, scalar(&row[2])?));
            }
            for row in config.records(b"smooth") {
                let first = row.first().ok_or_else(|| invalid("M2 smooth record"))?;
                output.smooth.insert(name(first)?);
            }
            for row in config.records(b"dif") {
                let dimension = row.first().ok_or_else(|| invalid("M2 dif declaration"))?;
                let dimension = u32_word(dimension)?;
                if dimension == 0 || dimension > 4096 {
                    return Err(invalid("M2 dif dimension"));
                }
                output.dimensions = output.dimensions.max(dimension as usize);
            }
            for row in config.records(b"defval") {
                if row.len() != 2 {
                    return Err(invalid("M2 defval record"));
                }
                output
                    .defaults
                    .entry(row[0].clone())
                    .or_insert(scalar(&row[1])?);
            }
            let mut current = None;
            for row in config.records(b"pattern") {
                let first = row.first().ok_or_else(|| invalid("M2 pattern record"))?;
                if first.first() == Some(&b'%') {
                    if row.len() < 3 {
                        return Err(invalid("M2 pattern header"));
                    }
                    let dimension = u32_word(&first[1..])?
                        .checked_sub(1)
                        .ok_or_else(|| invalid("M2 pattern dimension"))?;
                    if dimension >= 4096 {
                        return Err(EmoteError::Capacity);
                    }
                    let key = (dimension, row[1].clone());
                    if row[3..].iter().any(|v| v.eq_ignore_ascii_case(b"default")) {
                        output
                            .default_patterns
                            .entry(dimension)
                            .or_insert(row[1].clone());
                    }
                    output
                        .patterns
                        .entry(key.0)
                        .or_default()
                        .entry(key.1.clone())
                        .or_default();
                    current = Some(key);
                } else {
                    if row.len() != 2 {
                        return Err(invalid("M2 pattern variable record"));
                    }
                    let key = current
                        .as_ref()
                        .ok_or_else(|| invalid("M2 pattern variable before header"))?;
                    output
                        .patterns
                        .get_mut(&key.0)
                        .and_then(|p| p.get_mut(&key.1))
                        .ok_or_else(|| invalid("M2 pattern owner"))?
                        .push((first.clone(), scalar(&row[1])?));
                }
            }
            for row in config.records(b"inertia_lr") {
                if row.len() != 1 {
                    return Err(invalid("M2 inertia_lr record"));
                }
                output.inertia_lr.push(name(&row[0])?);
            }
            for row in config.records(b"inertia_ud") {
                if row.len() != 1 {
                    return Err(invalid("M2 inertia_ud record"));
                }
                output.inertia_ud.push(name(&row[0])?);
            }
            for row in config.records(b"inertia_dif") {
                if row.len() != 4 {
                    return Err(invalid("M2 inertia_dif record"));
                }
                output.inertia_dif.push((
                    name(&row[0])?,
                    name(&row[1])?,
                    scalar(&row[2])?,
                    scalar(&row[3])?,
                ));
            }
            output
                .material_rows
                .extend_from_slice(config.records(b"material"));
            for row in config.records(b"blink") {
                if row.len() != 7 {
                    return Err(invalid("M2 blink record"));
                }
                let dimension = u32_word(&row[0])?
                    .checked_sub(1)
                    .ok_or_else(|| invalid("M2 blink dimension"))?;
                let interval_min = u32_word(&row[5])?;
                let interval_max = u32_word(&row[6])?;
                let speed = scalar(&row[4])?;
                if interval_max < interval_min || speed <= 0.0 {
                    return Err(invalid("M2 blink interval/speed"));
                }
                output.blink.push(CpuM2BlinkDefinition {
                    dimension,
                    pattern_cp932: row[1].clone(),
                    begin: scalar(&row[2])?,
                    end: scalar(&row[3])?,
                    speed,
                    interval_min,
                    interval_max,
                });
            }
            for row in config.records(b"lip_table") {
                if row.len() < 6 {
                    return Err(invalid("M2 lip_table record"));
                }
                let dimension = u32_word(&row[0])?
                    .checked_sub(1)
                    .ok_or_else(|| invalid("M2 lip dimension"))?;
                output.lip.push(CpuM2LipDefinition {
                    dimension,
                    pattern_cp932: row[1].clone(),
                    interpolation_cp932: row[2].clone(),
                    level_min: scalar(&row[3])?,
                    level_max: scalar(&row[4])?,
                    values: row[5..].iter().map(|v| scalar(v)).collect::<Result<_>>()?,
                    linked_variable_cp932: None,
                    flow: Vec::new(),
                });
            }
        }
        for config in &configs {
            for row in config.records(b"lip_variable") {
                if row.len() != 3 {
                    return Err(invalid("M2 lip_variable record"));
                }
                let dimension = u32_word(&row[0])?
                    .checked_sub(1)
                    .ok_or_else(|| invalid("M2 lip variable dimension"))?;
                for lip in output
                    .lip
                    .iter_mut()
                    .filter(|v| v.dimension == dimension && v.pattern_cp932 == row[1])
                {
                    lip.linked_variable_cp932 = Some(row[2].clone());
                }
            }
            for row in config.records(b"lip_flow") {
                if row.len() < 3 {
                    return Err(invalid("M2 lip_flow record"));
                }
                let dimension = u32_word(&row[0])?
                    .checked_sub(1)
                    .ok_or_else(|| invalid("M2 lip flow dimension"))?;
                let mut steps = Vec::new();
                for token in &row[2..] {
                    let parts: Vec<_> = token.split(|b| *b == b',').collect();
                    if parts.len() != 3 {
                        return Err(invalid("M2 lip_flow integer,float,integer tuple"));
                    }
                    steps.push(CpuM2LipFlowStep {
                        pattern: u32_word(parts[0])?,
                        frames: scalar(parts[1])?,
                        finish_check: u32_word(parts[2])?,
                    });
                }
                for lip in output
                    .lip
                    .iter_mut()
                    .filter(|v| v.dimension == dimension && v.pattern_cp932 == row[1])
                {
                    lip.flow = steps.clone();
                }
            }
        }
        if output.blink.len() > 100_000 || output.lip.len() > 100_000 {
            return Err(EmoteError::Capacity);
        }
        Ok(output)
    }

    ///187400: explicit default marker wins; otherwise the unselected "$"
    ///sentinel is used for the ordinary zero-attribute dimension.
    pub fn pattern_selection(&self, requested: &[Vec<u8>]) -> Result<Vec<Vec<u8>>> {
        if requested.len() > 4096 {
            return Err(EmoteError::Capacity);
        }
        let mut values = Vec::with_capacity(requested.len().max(self.dimensions));
        for index in 0..requested.len().max(self.dimensions) {
            let value = requested.get(index).cloned().unwrap_or_else(|| {
                self.default_patterns
                    .get(&(index as u32))
                    .cloned()
                    .unwrap_or_else(|| b"$".to_vec())
            });
            if value.len() > 4095 || value.contains(&0) {
                return Err(invalid("M2 selected pattern label"));
            }
            values.push(value);
        }
        Ok(values)
    }

    pub fn pattern_variables(
        &self,
        dimension: u32,
        requested: &[u8],
    ) -> Result<Vec<(String, f64)>> {
        let key = if requested.is_empty() || requested == b"$" {
            b"0".as_slice()
        } else {
            requested
        };
        let Some(patterns) = self.patterns.get(&dimension) else {
            return Ok(Vec::new());
        };
        let Some(values) = patterns.get(key) else {
            if key == b"0" {
                return Ok(Vec::new());
            }
            return Err(invalid("M2 selected pattern is absent"));
        };
        values
            .iter()
            .map(|(key, value)| Ok((name(key)?, f64::from(*value))))
            .collect()
    }

    ///178570 strips directory/extension and1878b0 compares registered groups.
    pub(super) fn changes_within_group(&self, from: &[u8], to: &[u8]) -> bool {
        fn stem(value: &[u8]) -> &[u8] {
            let start = value.iter().rposition(|byte| matches!(byte, b'/' | b'\\')).map_or(0, |at| at + 1);
            let value = &value[start..];
            &value[..value.iter().rposition(|byte| *byte == b'.').unwrap_or(value.len())]
        }
        let (from, to) = (stem(from), stem(to));
        !from.eq_ignore_ascii_case(to) && self.resource_groups.get(from)
            .zip(self.resource_groups.get(to)).is_some_and(|(a, b)| a == b)
    }

    pub fn default_variables(&self) -> Result<BTreeMap<String, f64>> {
        self.defaults
            .iter()
            .map(|(key, value)| Ok((name(key)?, f64::from(*value))))
            .collect()
    }

    ///1766b0 shared lane. Every enabled entry of each pattern after `from`
    ///introduces an output name; collect *all* matching entries
    ///from the complete sequence, preserving duplicates and missing entries.
    pub(super) fn chain_requests(
        &self,
        dimension: u32,
        chain: &CpuM2ChainSelection,
        kind: CpuM2ChainKind,
        driver: &BTreeMap<String, f64>,
    ) -> Result<Vec<CpuM2TimedRequest>> {
        if chain.labels_cp932.len() > 100_000 {
            return Err(EmoteError::Capacity);
        }
        let patterns = chain
            .labels_cp932
            .iter()
            .map(|label| self.pattern_variables(dimension, label))
            .collect::<Result<Vec<_>>>()?;
        let names: BTreeSet<_> = patterns
            .iter()
            .skip(1)
            //176aa5..176aca advances the entry pointer by0x28 and resets
            //the per-entry name loop at176851. Its limit1 belongs to one
            //entry, not to the complete pattern vector.
            .flat_map(|entries| entries.iter().map(|(name, _)| name.clone()))
            .collect();
        let mut requests = Vec::new();
        for name in names {
            let mut samples: Vec<f32> = patterns
                .iter()
                .flatten()
                .filter(|(key, _)| key == &name)
                .map(|(_, value)| *value as f32)
                .collect();
            if samples.len() > 100_000 {
                return Err(EmoteError::Capacity);
            }
            let Some(&target) = samples.last() else {
                continue;
            };
            //176f90 saves the actual target *before* sample correction.
            if self.smooth.contains(&name) {
                samples[0] = *driver.get(&name).unwrap_or(&0.0) as f32;
            } else {
                if let Some(nodes) = self.same.get(&name) {
                    //1965f0: exact members stay. Otherwise include the
                    //original value among nearest-to-other candidates;
                    //equal distance chooses the lower scalar.
                    let nearest = |value: f32, other: f32| {
                        if nodes.contains(&value) {
                            return value;
                        }
                        let mut result = value;
                        let mut distance = (other - value).abs();
                        for &node in nodes {
                            let next = (other - node).abs();
                            if next < distance || (next == distance && node < result) {
                                result = node;
                                distance = next;
                            }
                        }
                        result
                    };
                    for index in 1..samples.len() {
                        samples[index - 1] = nearest(samples[index - 1], samples[index]);
                        samples[index] = nearest(samples[index], samples[index - 1]);
                    }
                }
                if let Some(replacement) = self.start.get(&name).and_then(|rows| {
                    rows.iter()
                        .find(|(from, _)| *from == samples[0])
                        .map(|(_, to)| *to)
                }) {
                    samples[0] = replacement;
                }
            }
            if !samples.iter().all(|v| v.is_finite()) {
                return Err(invalid("M2 corrected samples"));
            }
            requests.push(CpuM2TimedRequest {
                name,
                target,
                frames: chain.frames,
                kind,
                samples,
            });
        }
        Ok(requests)
    }

    /// Import the actual stored reset values, preserving constructor flags and
    /// raw f32 phase words. Does not invent selection/transition runtime state.
    pub fn reset_bank_seeds(&self) -> Result<CpuM2ResetBankSeeds> {
        let mut variable_banks = BTreeMap::<u32, CpuNative907VariableBank>::new();
        let mut phase_banks = BTreeMap::<u32, CpuNative907PhaseBank>::new();
        let mut ordered_blink = BTreeMap::new();
        for blink in &self.blink {
            ordered_blink
                .entry((blink.dimension, &blink.pattern_cp932))
                .or_insert(blink);
        }
        for blink in ordered_blink.values() {
            let Some((variable, value)) = self
                .patterns
                .get(&blink.dimension)
                .and_then(|p| p.get(&blink.pattern_cp932))
                .and_then(|v| v.first())
            else {
                continue;
            };
            variable_banks
                .entry(blink.dimension)
                .or_default()
                .records
                .push(CpuNative907VariableReset {
                    active: true,
                    name: name(variable)?,
                    value: *value,
                    counter_60: 0,
                    counter_64: 0,
                });
        }
        let mut ordered_lip = BTreeMap::new();
        for lip in &self.lip {
            ordered_lip
                .entry((lip.dimension, &lip.pattern_cp932))
                .or_insert(lip);
        }
        for lip in ordered_lip.values() {
            if self
                .patterns
                .get(&lip.dimension)
                .and_then(|p| p.get(&lip.pattern_cp932))
                .and_then(|v| v.first())
                .is_none()
            {
                continue;
            }
            phase_banks
                .entry(lip.dimension)
                .or_default()
                .records
                .push(CpuNative907PhaseReset {
                    //183cf0 stores zero in record+5c at construction. A live
                    //selection/evaluation may subsequently enable the record.
                    active: false,
                    phase_enabled: false,
                    first_phase_words: lip.values.iter().map(|v| v.to_bits()).collect(),
                    current_phase_98: 0,
                    restart_ac: 0,
                });
        }
        Ok(CpuM2ResetBankSeeds {
            variable_banks,
            phase_banks,
            blink: self.blink.clone(),
            lip: self.lip.clone(),
        })
    }
}

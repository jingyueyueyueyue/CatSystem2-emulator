//! NEKO193c30/194190: named material groups feed the live spring owner.
use std::collections::BTreeMap;

use super::{CpuM2Config, CpuM2Material, CpuNative907TransitionState, EmoteError, Result};

#[derive(Debug, Clone)]
pub enum CpuM2MaterialSetting {
    Level { name: String, percent: i32 },
    Enabled { name: String, value: u32 },
}

#[derive(Debug, Clone, Default)]
pub(super) struct CpuM2MaterialGroups {
    minimum: BTreeMap<String, CpuM2Material>,
    maximum: BTreeMap<String, CpuM2Material>,
    levels: BTreeMap<String, Vec<String>>,
    enables: BTreeMap<String, Vec<String>>,
}

fn name(bytes: &[u8]) -> Result<String> {
    let (value, _, errors) = encoding_rs::SHIFT_JIS.decode(bytes);
    if errors || value.is_empty() || value.contains('\0') {
        return Err(EmoteError::InvalidResource("M2 material group name"));
    }
    Ok(value.into_owned())
}

impl CpuM2MaterialGroups {
    pub fn from_expanded<'a>(configs: impl IntoIterator<Item = &'a CpuM2Config>) -> Result<Self> {
        let mut output = Self::default();
        for config in configs {
            for (key, table) in [(b"material_min".as_slice(), &mut output.minimum),
                                 (b"material_max".as_slice(), &mut output.maximum)] {
                for row in config.records(key) {
                    let material = CpuM2Material::from_record(row)?;
                    table.entry(material.name.clone()).or_insert(material);
                }
            }
            for (key, table) in [(b"inertia_lv".as_slice(), &mut output.levels),
                                 (b"inertia_enable".as_slice(), &mut output.enables)] {
                let mut current = None;
                for row in config.records(key) {
                    let first = row.first().ok_or(EmoteError::InvalidResource("M2 group row"))?;
                    if first.first() == Some(&b'%') {
                        let group = name(&first[1..])?;
                        table.entry(group.clone()).or_default();
                        current = Some(group);
                    } else if let Some(group) = &current {
                        table.get_mut(group).expect("group declared above").push(name(first)?);
                    }
                    //1a82e6/1a85d1: rows before a % header have no owner.
                }
            }
        }
        Ok(output)
    }
}

#[derive(Debug, Clone)]
pub(super) struct CpuM2MaterialSettings {
    groups: CpuM2MaterialGroups,
    base: BTreeMap<String, [f32; 5]>,
}

fn lanes(material: &CpuM2Material) -> [f32; 5] {
    [material.microstep, material.spring, material.mass, material.resistance, material.maximum]
}

fn reset(material: &mut CpuM2Material) {
    material.normalized = 0.0;
    material.displacement = 0.0;
    material.velocity = 0.0;
}

impl CpuM2MaterialSettings {
    pub fn new(groups: CpuM2MaterialGroups, materials: &BTreeMap<String, CpuM2Material>) -> Self {
        Self { groups, base: materials.iter().map(|(name, material)| (name.clone(), lanes(material))).collect() }
    }

    pub fn apply(&self, setting: &CpuM2MaterialSetting,
                 materials: &mut BTreeMap<String, CpuM2Material>,
                 transition: &mut CpuNative907TransitionState) {
        let names = match setting {
            CpuM2MaterialSetting::Level { name, .. } => self.groups.levels.get(name),
            CpuM2MaterialSetting::Enabled { name, .. } => self.groups.enables.get(name),
        };
        let Some(names) = names else { return; };
        for name in names {
            let Some(material) = materials.get_mut(name) else { continue; };
            match setting {
                CpuM2MaterialSetting::Level { percent, .. } => {
                    let base = self.base[name];
                    let weight = (*percent).clamp(0, 200) as f32 * 0.01;
                    let values = if weight == 1.0 { base } else {
                        let (from, to, t) = if weight < 1.0 {
                            (self.groups.minimum.get(name).map(lanes).unwrap_or(base), base, weight)
                        } else {
                            (base, self.groups.maximum.get(name).map(lanes).unwrap_or(base), weight - 1.0)
                        };
                        std::array::from_fn(|index| (to[index] - from[index]) * t + from[index])
                    };
                    [material.microstep, material.spring, material.mass, material.resistance, material.maximum] = values;
                }
                CpuM2MaterialSetting::Enabled { value, .. } => material.enabled = *value != 0,
            }
            reset(material);
        }
        //193f26..1940xx and1943xx reset every force history after a known group.
        for record in transition.group_20_24.iter_mut()
            .chain(&mut transition.group_2c_30).chain(&mut transition.group_48_4c) {
            record.from = 0.0;
            record.to = 0.0;
        }
    }
}

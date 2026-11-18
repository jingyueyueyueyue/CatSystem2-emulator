//! Fixed NEKO FES material settings, shared with actual loaded/future M2 owners.
use std::collections::BTreeMap;

use anyhow::Result;
use cs2_runtime::emote::{CpuM2Config, CpuM2MaterialSetting};
use cs2_script::ExpressionState;

use super::super::fes_driver::{FesDriverEffect, FesScriptDriver};
use super::super::{LoggingHost, trim_nul};

#[derive(Default)]
pub(super) struct M2Settings {
    levels: BTreeMap<String, i32>,
    enables: BTreeMap<String, u32>,
}

fn name(bytes: &[u8]) -> Result<String> {
    let (name, _, errors) = encoding_rs::SHIFT_JIS.decode(trim_nul(bytes));
    anyhow::ensure!(!errors && !name.is_empty(), "invalid CP932 M2 material group");
    Ok(name.into_owned())
}

impl M2Settings {
    pub fn register(&mut self, config: &CpuM2Config) -> Result<()> {
        for row in config.records(b"inertia_lv_label") {
            if let Some(first) = row.first() { self.levels.entry(name(first)?).or_insert(100); }
        }
        for row in config.records(b"inertia_enable_label") {
            if let Some(first) = row.first() { self.enables.entry(name(first)?).or_insert(1); }
        }
        Ok(())
    }

    pub fn current(&self) -> Vec<CpuM2MaterialSetting> {
        self.levels.iter().map(|(name, percent)| CpuM2MaterialSetting::Level { name: name.clone(), percent: *percent })
            .chain(self.enables.iter().map(|(name, value)| CpuM2MaterialSetting::Enabled { name: name.clone(), value: *value }))
            .collect()
    }
}

pub(crate) fn apply_effect(host: &mut LoggingHost<'_>, driver: &mut FesScriptDriver,
                           effect: &FesDriverEffect, shared: &mut ExpressionState) -> Result<Option<bool>> {
    let FesDriverEffect::Command(command) = effect else { return Ok(None); };
    let Some(verb) = command.parameters.first().map(Vec::as_slice) else { return Ok(None); };
    if !matches!(verb, b"m2_getconf" | b"m2_setconf") { return Ok(None); }
    //38699e/386e7e: an absent manager consumes the statement without a write.
    if !host.data_services.emote.state.backend_ready() { return Ok(Some(true)); }
    let Some(key) = command.parameters.get(1).map(|v| trim_nul(v)) else { return Ok(Some(true)); };
    if !matches!(key, b"iner_lv" | b"iner_enable") { return Ok(None); }
    let value = match command.parameters.get(2).filter(|v| !v.is_empty()) {
        Some(source) => driver.evaluate_parameter(source, 0, shared)?,
        None if verb == b"m2_getconf" => return Ok(Some(true)),
        None => if key == b"iner_lv" { 100 } else { 1 },
    };
    let Some(group) = command.parameters.get(3) else { return Ok(Some(true)); };
    let group = name(group)?;
    let service = &mut host.data_services.emote;
    if verb == b"m2_getconf" {
        let result = if key == b"iner_lv" { *service.material_settings.levels.get(&group).unwrap_or(&100) as u32 }
            else { *service.material_settings.enables.get(&group).unwrap_or(&1) };
        if let Some(target) = usize::try_from(value).ok().and_then(|index| shared.shared.get_mut(index)) {
            *target = result as i32;
        }
    } else {
        let setting = if key == b"iner_lv" {
            let Some(target) = service.material_settings.levels.get_mut(&group) else { return Ok(Some(true)); };
            *target = value.clamp(0, 200);
            CpuM2MaterialSetting::Level { name: group, percent: *target }
        } else {
            let Some(target) = service.material_settings.enables.get_mut(&group) else { return Ok(Some(true)); };
            *target = value as u32;
            CpuM2MaterialSetting::Enabled { name: group, value: *target }
        };
        service.state.set_m2_material_group(&setting)?;
    }
    Ok(Some(true))
}

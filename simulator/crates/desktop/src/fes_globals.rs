//! FES sound-configuration queries. Every entry shares the driver's expression
//! scope and the player's random stream. Retry cursors retain successful
//! evaluations so an error cannot replay earlier assignment/random effects.
use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use cs2_script::ExpressionState;

use super::fes_driver::{FesDriverEffect, FesScriptDriver};
use super::fes_tasks::FesTaskView;
use super::{DesktopSoundPart, FesRuntime, LoggingHost, trim_nul};

#[derive(Default, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(super) struct FesGlobalStore {
    pending: BTreeMap<(u32, usize), QueryProgress>,
}

#[derive(Default, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct QueryProgress {
    output: Option<i32>,
    parameter: usize,
    component: usize,
    paths: Vec<Vec<i32>>,
    values: Vec<i32>,
}

impl FesGlobalStore {
    pub(crate) fn capture_checkpoint(&self, kinds: &BTreeMap<u32, u32>) -> Result<Vec<u8>> {
        self.validate_checkpoint(kinds)?;
        Ok(cs2_runtime::object_graph::encode_component(self)?)
    }
    pub(crate) fn prepare_checkpoint(bytes: &[u8], kinds: &BTreeMap<u32, u32>) -> Result<Self> {
        let saved: Self = cs2_runtime::object_graph::decode_component(bytes)?;
        saved.validate_checkpoint(kinds)?;
        Ok(saved)
    }
    fn validate_checkpoint(&self, kinds: &BTreeMap<u32, u32>) -> Result<()> {
        if self.pending.len() > 65_536 || self.pending.iter().any(|((owner, _), progress)|
            kinds.get(owner) != Some(&34) || progress.parameter > 1_000_000
                || progress.component > 1_000_000 || progress.paths.len() > 1_000_000
                || progress.values.len() > 1_000_000
                || progress.paths.iter().any(|path| path.len() > 1_000_000))
        { bail!("FES global query checkpoint has invalid owners or cursors"); }
        Ok(())
    }
}

pub(super) fn apply_effect(
    host: &mut LoggingHost<'_>, view: &FesTaskView, _runtime: &mut FesRuntime,
    driver: &mut FesScriptDriver, effect: &FesDriverEffect, shared: &mut ExpressionState,
) -> Result<Option<bool>> {
    let FesDriverEffect::Command(command) = effect else { return Ok(None); };
    let Some(verb) = command.parameters.first().map(Vec::as_slice) else { return Ok(None); };
    if !matches!(verb, b"sysvoiceex" | b"getsndconfex") { return Ok(None); }
    let args = &command.parameters[1..];
    if args.is_empty() { return Ok(Some(true)); }
    let query = args.get(1).map(Vec::as_slice);
    let needs_config = if verb == b"sysvoiceex" {
        host.sound_config_enabled != 0
    } else { query.is_some() };
    if needs_config && !host.sound_config.source_loaded {
        bail!("FES sound query requires a loaded sound configuration");
    }
    if verb == b"getsndconfex" {
        if let Some(query) = query {
            if !matches!(query, b"volume_id" | b"sysplay_id" | b"play_id"
                | b"volume" | b"voicebgm" | b"volume_type" | b"play_type")
            { bail!("getsndconfex query {:?} has no documented host contract", String::from_utf8_lossy(query)); }
            if query == b"volume" && args.get(2).is_some_and(|name| trim_nul(name) == b"master") {
                host.sound_config.master_volume.context("FES master volume is unavailable")?;
            }
        }
    }
    let key = (view.id, command.offset);
    let mut progress = host.data_services.fes_globals.pending.remove(&key).unwrap_or_default();
    let result = (|| -> Result<()> {
        let output = match progress.output {
            Some(output) => output,
            None => {
                let output = driver.evaluate_parameter(&args[0], 0, shared)?;
                progress.output = Some(output);
                output
            }
        };
        if verb == b"sysvoiceex" {
            if host.sound_config_enabled == 0 {
                write(shared, output, -1);
                return Ok(());
            }
            while progress.parameter < args.len().saturating_sub(1) {
                let source = &args[1 + progress.parameter];
                let components: Vec<_> = source.split(|&byte| byte == b',').collect();
                if progress.paths.len() <= progress.parameter { progress.paths.push(Vec::new()); }
                while progress.component < components.len() {
                    let value = driver.evaluate_parameter(components[progress.component], 0, shared)?;
                    progress.paths[progress.parameter].push(value);
                    progress.component += 1;
                }
                progress.parameter += 1;
                progress.component = 0;
            }
            let candidates = voice_candidates(&host.sound_config.parts, &progress.paths);
            let selected = if candidates.is_empty() { -1 } else {
                let index = host.script_random.next_integer() as usize % candidates.len();
                candidates[index]
            };
            write(shared, output, selected);
            return Ok(());
        }
        let Some(query) = query else { return Ok(()); };
        let value = if matches!(query, b"volume_id" | b"sysplay_id" | b"play_id") {
            while progress.parameter < args.len().saturating_sub(2) {
                let value = driver.evaluate_parameter(&args[2 + progress.parameter], 0, shared)?;
                progress.values.push(value);
                progress.parameter += 1;
            }
            match query {
                b"volume_id" => host.sound_config.volume(&progress.values),
                b"sysplay_id" => host.sound_config.system_voice_enabled(&progress.values),
                _ => host.sound_config.play_enabled(&progress.values),
            }
        } else {
            let Some(name) = args.get(2).map(|name| trim_nul(name)) else { return Ok(()); };
            if name.is_empty() { return Ok(()); }
            match query {
                b"volume" if name == b"master" =>
                    host.sound_config.master_volume.context("FES master volume is unavailable")?,
                b"volume" => host.sound_config.override_value(name, 0)
                    .unwrap_or_else(|| host.sound_config.group_volume(name)),
                b"voicebgm" => host.sound_config.override_value(name, 1)
                    .unwrap_or_else(|| host.sound_config.group_voice_volume(name)),
                b"volume_type" => host.sound_config.type_part(name).map_or(100, |part| part.volume),
                b"play_type" => host.sound_config.type_part(name).map_or(0, |part| part.play_enabled),
                _ => unreachable!(),
            }
        };
        write(shared, output, value);
        Ok(())
    })();
    if result.is_err() { host.data_services.fes_globals.pending.insert(key, progress); }
    result.map(|_| Some(true))
}

pub(super) fn release_owner(host: &mut LoggingHost<'_>, owner: u32) {
    host.data_services.fes_globals.pending.retain(|(id, _), _| *id != owner);
}

fn write(shared: &mut ExpressionState, output: i32, value: i32) {
    if output >= 0 {
        if let Some(slot) = shared.shared.get_mut(output as usize) { *slot = value; }
    }
}

fn voice_candidates(parts: &[DesktopSoundPart], paths: &[Vec<i32>]) -> Vec<i32> {
    if paths.is_empty() {
        let mut stack: Vec<_> = parts.iter().rev().collect();
        let mut candidates = Vec::new();
        while let Some(part) = stack.pop() {
            if part.system_voice == 1 { candidates.push(part.id); }
            stack.extend(part.children.iter().rev());
        }
        candidates
    } else {
        paths.iter().filter_map(|path| {
            let mut level = parts;
            let mut enabled = 1;
            for id in path {
                enabled = 1;
                if let Some(part) = level.iter().find(|part| part.id == *id) {
                    enabled = part.system_voice;
                    level = &part.children;
                }
            }
            path.last().copied().filter(|_| enabled != 0)
        }).collect()
    }
}

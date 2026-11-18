//! FES PARTICLE banks over the existing kind16 receiver, VM/PPI and Canvas.
//! Commands edit retained records. The owner media pass submits load, parameter,
//! opacity and action latches in PE order (Yuki1b6ce0); it never advances a
//! second interpreter or projects particle pixels through an image-list layer.
use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, bail};
use cs2_script::{ExpressionState, FesEvent, FesObjectDefinition, FesObjectInitialization};

use super::fes_driver::{FesDriverCommand, FesDriverEffect, FesScriptDriver};
use super::fes_tasks::FesTaskView;
use super::{FesRuntime, LoggingHost, ScriptTextEncoding};
use cs2_runtime::graph_serde::{Deserialize, Serialize};

#[path = "fes_particle_state.rs"]
pub(crate) mod checkpoint;

type Key = (u32, Vec<u8>, i32);
const MAX_BANKS: usize = 65_536;

#[derive(Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct ParticleBank {
    lower: u32,
    serial: u64,
    plugin: u32,
    program: Vec<u8>,
    image: Vec<u8>,
    count: u32,
    depth: i32,
    #[serde(with = "checkpoint::parameters")]
    parameters: [u8; 128],
    opacity: i32,
    load_dirty: bool,
    parameters_dirty: bool,
    start_pending: bool,
    stop_pending: bool,
    playing: bool,
    released: bool,
    /// Completed category/path expansion belongs to this load attempt, even
    /// when a later resource read fails. No caller VM address is retained.
    load_record: Option<Vec<u8>>,
}

impl ParticleBank {
    fn new(lower: u32, serial: u64) -> Self {
        // Yuki2aa880: empty0x8c init record, zero128B parameters,
        // alpha255; load/parameter/start/stop flags initially clear.
        Self { lower, serial, plugin: 0, program: Vec::new(), image: Vec::new(),
            count: 0, depth: 0, parameters: [0; 128], opacity: 255,
            load_dirty: false, parameters_dirty: false, start_pending: false,
            stop_pending: false, playing: false, released: false, load_record: None }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct CommandProgress {
    keys: Vec<Key>,
    values: Vec<i32>,
}

#[derive(Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct UpdateProgress {
    now: u32,
    keys: Vec<Key>,
    cursor: usize,
    stage: u8,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(super) struct FesParticleStore {
    banks: BTreeMap<Key, ParticleBank>,
    declarations: BTreeSet<(u32, usize)>,
    creations: BTreeMap<(u32, usize), usize>,
    initializers: BTreeMap<(u32, usize), usize>,
    commands: BTreeMap<(u32, usize), CommandProgress>,
    updates: BTreeMap<u32, UpdateProgress>,
    next_serial: u64,
}

impl FesParticleStore {
    /// The outer transaction must include checkpoint::PreparedParticles before
    /// excluding this count from its opaque guard. Never discard receipts.
    pub(super) fn graph_object_count(&self) -> usize {
        self.banks.len() + self.declarations.len() + self.creations.len() + self.initializers.len()
            + self.commands.len() + self.updates.len()
    }

    fn keys(&self, owner: u32, name: &[u8], bank: Option<i32>) -> Vec<Key> {
        let mut keys: Vec<_> = self.banks.keys().filter(|(id, object, index)|
            *id == owner && object == name && bank.is_none_or(|bank| bank == -1 || bank == *index))
            .cloned().collect();
        keys.sort_by_key(|key| self.banks[key].serial);
        keys
    }

    fn create(
        &mut self, host: &mut LoggingHost<'_>, view: &FesTaskView,
        definition: &FesObjectDefinition, driver: &FesScriptDriver,
    ) -> Result<()> {
        let stamp = (view.id, definition.offset);
        if self.declarations.contains(&stamp) { return Ok(()); }
        let count = driver.adapter.object(&definition.name)
            .context("PARTICLE declaration has no resolved bank metadata")?.definition.bank_count;
        let start = *self.creations.entry(stamp).or_insert(0);
        if self.banks.len().saturating_add(count.saturating_sub(start)) > MAX_BANKS {
            bail!("FES PARTICLE bank limit exceeded");
        }
        for index in start..count {
            let key = (view.id, definition.name.clone(), index as i32);
            if self.banks.contains_key(&key) {
                bail!("duplicate PARTICLE declaration needs its own lower-task binding");
            }
            let serial = self.next_serial;
            let next_serial = serial.checked_add(1).context("PARTICLE declaration order exhausted")?;
            // Yuki17bbe0 creates kind16 as an actual child of this FES owner,
            // named "FE Particle", then records the resulting real task id.
            let lower = host.create_task_object(16, view.id, b"FE Particle", &view.context)?;
            self.banks.insert(key, ParticleBank::new(lower, serial));
            self.next_serial = next_serial;
            self.creations.insert(stamp, index + 1);
        }
        self.creations.remove(&stamp);
        self.declarations.insert(stamp);
        Ok(())
    }

    fn initialize(
        &mut self, owner: u32, init: &FesObjectInitialization,
        driver: &mut FesScriptDriver, shared: &mut ExpressionState, encoding: ScriptTextEncoding,
    ) -> Result<bool> {
        let (name, bank) = split_target(&init.target, encoding)?;
        if self.keys(owner, name, None).is_empty() { return Ok(false); }
        // The interpreter already evaluated an initialization bank target.
        let bank = bank.map(|bank| -> Result<i32> { Ok(std::str::from_utf8(bank)?.parse()?) }).transpose()?;
        let keys = self.keys(owner, name, bank);
        let stamp = (owner, init.offset);
        let cursor = *self.initializers.get(&stamp).unwrap_or(&0);
        for (index, (column, raw)) in init.columns.iter().zip(&init.values).enumerate().skip(cursor) {
            // The retained alpha is the same DWORD used by blend/326. Do not
            // reinterpret image geometry, NUM or ANM as particle init records.
            if !matches!(column.as_slice(), b"ALPHA" | b"BLEND") {
                bail!("PARTICLE initializer {:?} has no recovered bank receiver", String::from_utf8_lossy(column));
            }
            let value = driver.evaluate_parameter(raw, 255, shared)?;
            for key in &keys {
                let record = self.banks.get_mut(key).unwrap();
                record.opacity = value;
                record.parameters_dirty = true;
            }
            self.initializers.insert(stamp, index + 1);
        }
        Ok(true)
    }

    fn command(
        &mut self, view: &FesTaskView, command: &FesDriverCommand,
        driver: &mut FesScriptDriver, shared: &mut ExpressionState, encoding: ScriptTextEncoding,
    ) -> Result<bool> {
        let Some(verb) = command.parameters.get(1).map(Vec::as_slice) else { return Ok(false); };
        if !matches!(verb, b"init" | b"param" | b"start" | b"stop" | b"blend") {
            return Ok(false); // Mixed delete and other shared services retain their own owners.
        }
        let (name, bank) = split_target(&command.parameters[0], encoding)?;
        if self.keys(view.id, name, None).is_empty() { return Ok(false); }
        let stamp = (view.id, command.offset);
        if !self.commands.contains_key(&stamp) {
            let index = bank.map(|source|
                driver.evaluate_parameter(source, command.context.current, shared)).transpose()?;
            let keys = self.keys(view.id, name, index);
            if keys.is_empty() {
                driver.fallback_command(command, shared)?;
                return Ok(true);
            }
            self.commands.insert(stamp, CommandProgress { keys, values: Vec::new() });
        }
        let mut progress = self.commands.remove(&stamp).unwrap();
        let args = &command.parameters[2..];
        let result = (|| -> Result<()> {
            if verb == b"init" && args.len() < 5 {
                bail!("PARTICLE init needs mode, program, texture, count and depth");
            }
            let number_count = match verb {
                b"init" => 3,
                b"param" => args.len().min(32),
                b"blend" => 1,
                _ => 0,
            };
            while progress.values.len() < number_count {
                let at = progress.values.len();
                let source_index = if verb == b"init" { [0, 3, 4][at] } else { at };
                let default = if verb == b"blend" { 255 } else { 0 };
                let value = match args.get(source_index) {
                    Some(source) => driver.evaluate_parameter(source, default, shared)?,
                    None => default,
                };
                progress.values.push(value);
            }
            let names = if verb == b"init" {
                // Original init stores two64-byte script-encoded strings.
                Some((bounded_name(&args[1], 64)?, bounded_name(&args[2], 64)?))
            } else { None };
            let mut parameters = [0; 128];
            if verb == b"param" {
                // Yuki2d41d0 clears all128 bytes and cvtdq2ps converts each
                // evaluated integer. Copying integer bits changes every model.
                for (word, value) in parameters.chunks_exact_mut(4).zip(&progress.values) {
                    word.copy_from_slice(&(*value as f32).to_bits().to_le_bytes());
                }
            }
            for key in &progress.keys {
                let Some(record) = self.banks.get_mut(key) else { continue; };
                if record.released { bail!("PARTICLE command targets a released bank"); }
                match verb {
                    b"init" => {
                        let (program, image) = names.as_ref().unwrap();
                        record.plugin = progress.values[0] as u32;
                        record.program = program.clone();
                        record.image = image.clone();
                        record.count = progress.values[1] as u32;
                        record.depth = progress.values[2];
                        record.load_dirty = true;
                        record.load_record = None;
                    }
                    b"param" => { record.parameters = parameters; record.parameters_dirty = true; }
                    b"blend" => { record.opacity = progress.values[0]; record.parameters_dirty = true; }
                    // Separate latches: start then stop in a single media pass
                    // must execute both, in that order, not just the last verb.
                    b"start" => record.start_pending = true,
                    b"stop" => record.stop_pending = true,
                    _ => unreachable!(),
                }
            }
            Ok(())
        })();
        if result.is_err() { self.commands.insert(stamp, progress); }
        result.map(|_| true)
    }
}

pub(super) fn apply_effect(
    host: &mut LoggingHost<'_>, view: &FesTaskView, _runtime: &mut FesRuntime,
    driver: &mut FesScriptDriver, effect: &FesDriverEffect, shared: &mut ExpressionState,
) -> Result<Option<bool>> {
    let mut store = std::mem::take(&mut host.data_services.fes_particles);
    let result = (|| -> Result<Option<bool>> {
        match effect {
            FesDriverEffect::Initialize { event: FesEvent::CreateObject(definition), .. }
                if definition.kind.eq_ignore_ascii_case(b"PARTICLE") => {
                store.create(host, view, definition, driver)?;
                Ok(Some(true))
            }
            FesDriverEffect::Initialize { event: FesEvent::InitializeObject(init), .. } =>
                Ok(store.initialize(view.id, init, driver, shared, host.text_encoding)?.then_some(true)),
            FesDriverEffect::Command(command) =>
                Ok(store.command(view, command, driver, shared, host.text_encoding)?.then_some(true)),
            _ => Ok(None),
        }
    })();
    host.data_services.fes_particles = store;
    result
}

/// Submit each bank once in declaration order at the owner media boundary.
/// Keep the exact stage on failure: a later alpha/query failure must not
/// reload resources or restart an already-started runner on continuation.
pub(super) fn update_owner(
    host: &mut LoggingHost<'_>, view: &FesTaskView, _runtime: &mut FesRuntime,
    driver: &mut FesScriptDriver,
) -> Result<bool> {
    let mut store = std::mem::take(&mut host.data_services.fes_particles);
    let mut update = store.updates.remove(&view.id).unwrap_or_else(|| {
        let mut keys: Vec<_> = store.banks.keys().filter(|(owner, _, _)| *owner == view.id).cloned().collect();
        keys.sort_by_key(|key| store.banks[key].serial);
        UpdateProgress { now: view.now, keys, cursor: 0, stage: 0 }
    });
    let result = (|| -> Result<bool> {
        if update.now != view.now { bail!("PARTICLE media continuation changed its logical tick"); }
        while let Some(key) = update.keys.get(update.cursor) {
            let Some(record) = store.banks.get_mut(key) else {
                update.cursor += 1; update.stage = 0; continue;
            };
            if driver.adapter.object_bank(&key.1, key.2 as usize).is_none() {
                release_record(host, record)?;
                host.request_ordinary_task_delete(record.lower)?;
                hide_lower(host, record.lower);
                store.banks.remove(key);
                update.cursor += 1; update.stage = 0; continue;
            }
            if record.released { update.cursor += 1; update.stage = 0; continue; }
            if update.stage == 0 {
                if record.load_dirty {
                    if record.load_record.is_none() {
                        record.load_record = Some(host.prepare_fes_particle_load(
                            record.plugin, &record.program, &record.image, record.count, record.depth)?);
                    }
                    host.submit_fes_particle_command(record.lower, 320, 0, record.load_record.as_deref())?;
                    record.load_dirty = false;
                    record.load_record = None;
                }
                update.stage = 1;
            }
            if update.stage == 1 {
                if record.parameters_dirty {
                    host.submit_fes_particle_command(record.lower, 325, 0, Some(&record.parameters))?;
                    update.stage = 2;
                } else { update.stage = 3; }
            }
            if update.stage == 2 {
                host.submit_fes_particle_command(record.lower, 326, record.opacity as u32, None)?;
                record.parameters_dirty = false;
                update.stage = 3;
            }
            if update.stage == 3 {
                if record.start_pending {
                    host.submit_fes_particle_command(record.lower, 321, 0, None)?;
                    record.start_pending = false;
                    record.playing = true;
                }
                update.stage = 4;
            }
            if update.stage == 4 {
                if record.stop_pending {
                    host.submit_fes_particle_command(record.lower, 322, 0, None)?;
                    record.stop_pending = false;
                    record.playing = false;
                }
                update.stage = 5;
            }
            if update.stage == 5 && record.playing {
                record.playing = host.submit_fes_particle_command(record.lower, 324, 0, None)? != 0;
            }
            update.cursor += 1;
            update.stage = 0;
        }
        Ok(true)
    })();
    if !matches!(result, Ok(true)) { store.updates.insert(view.id, update); }
    host.data_services.fes_particles = store;
    result
}

pub(super) fn bank_release_targets(
    store: &FesParticleStore, owner: u32, name: &[u8], bank: i32,
) -> Option<Vec<u32>> {
    store.banks.get(&(owner, name.to_vec(), bank)).map(|record| vec![record.lower])
}

/// Invoke BEFORE marking the planned lower task deleted. A deleted task's
/// normal message receiver consumes323 without running its release helper.
/// The mixed releaser then requests task deletion, hides it, and forgets this
/// bank. Retain the lower identity here until those later steps have returned.
pub(super) fn release_bank(
    host: &mut LoggingHost<'_>, owner: u32, name: &[u8], bank: i32,
) -> Result<()> {
    let mut store = std::mem::take(&mut host.data_services.fes_particles);
    let result = match store.banks.get_mut(&(owner, name.to_vec(), bank)) {
        Some(record) => release_record(host, record),
        None => Ok(()), // An already committed mixed release is idempotent.
    };
    host.data_services.fes_particles = store;
    result
}

pub(super) fn forget_released_bank(
    store: &mut FesParticleStore, owner: u32, name: &[u8], bank: i32,
) {
    let key = (owner, name.to_vec(), bank);
    store.banks.remove(&key);
    for ((id, _), progress) in &mut store.commands {
        if *id == owner { progress.keys.retain(|item| item != &key); }
    }
    if let Some(update) = store.updates.get_mut(&owner) {
        let before = update.keys.iter().take(update.cursor).filter(|item| **item == key).count();
        let current_removed = update.keys.get(update.cursor) == Some(&key);
        update.keys.retain(|item| item != &key);
        update.cursor -= before;
        if current_removed { update.stage = 0; }
    }
}

pub(super) fn release_owner(host: &mut LoggingHost<'_>, owner: u32) -> Result<()> {
    let mut store = std::mem::take(&mut host.data_services.fes_particles);
    let result = (|| -> Result<()> {
        let keys: Vec<_> = store.banks.keys().filter(|(id, _, _)| *id == owner).cloned().collect();
        for key in keys {
            let record = store.banks.get_mut(&key).unwrap();
            release_record(host, record)?;
            host.request_ordinary_task_delete(record.lower)?;
            hide_lower(host, record.lower);
            store.banks.remove(&key);
        }
        store.declarations.retain(|(id, _)| *id != owner);
        store.creations.retain(|(id, _), _| *id != owner);
        store.initializers.retain(|(id, _), _| *id != owner);
        store.commands.retain(|(id, _), _| *id != owner);
        store.updates.remove(&owner);
        Ok(())
    })();
    host.data_services.fes_particles = store;
    result
}

fn release_record(host: &mut LoggingHost<'_>, record: &mut ParticleBank) -> Result<()> {
    if record.released { return Ok(()); }
    match host.data_services.tasks.message_task_state(record.lower) {
        Some((16, 0, _)) => { host.submit_fes_particle_command(record.lower, 323, 0, None)?; }
        Some((16, _, _)) | None => {
            // External/owner deletion may already have marked the lower task.
            // Run the actual destructor rather than claim its ignored323 ran.
            host.release_particle_program(record.lower);
        }
        _ => bail!("PARTICLE bank lower identity was replaced by another task kind"),
    }
    record.released = true;
    record.playing = false;
    record.load_record = None;
    Ok(())
}

fn hide_lower(host: &mut LoggingHost<'_>, lower: u32) {
    if let Some(node) = host.scene_nodes.get_mut(&lower) {
        node.visible = false;
        node.initialized = false;
    }
}

fn bounded_name(source: &[u8], capacity: usize) -> Result<Vec<u8>> {
    if source.contains(&0) || source.len() >= capacity {
        bail!("PARTICLE name exceeds its original {} byte field", capacity - 1);
    }
    Ok(source.to_vec())
}

fn split_target(target: &[u8], encoding: ScriptTextEncoding) -> Result<(&[u8], Option<&[u8]>)> {
    let mut at = 0;
    while at < target.len() {
        let byte = target[at];
        if byte == b'[' {
            if at == 0 || target.last() != Some(&b']') || at + 2 >= target.len() {
                bail!("PARTICLE bank target is malformed");
            }
            return Ok((&target[..at], Some(&target[at + 1..target.len() - 1])));
        }
        let lead = match encoding {
            ScriptTextEncoding::Cp932 => matches!(byte, 0x81..=0x9f | 0xe0..=0xfc),
            ScriptTextEncoding::Gbk => matches!(byte, 0x81..=0xfe),
            ScriptTextEncoding::Utf8 => false,
        };
        at += if lead && at + 1 < target.len() { 2 } else { 1 };
    }
    Ok((target, None))
}

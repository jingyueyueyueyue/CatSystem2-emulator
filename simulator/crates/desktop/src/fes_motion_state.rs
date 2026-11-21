//! Shared FES motion-owner continuation. No declaration or VM update is replayed.
use super::*;
use cs2_runtime::graph_serde::{Deserializer, Serializer};
use cs2_runtime::object_graph as wire;
use crate::motion_vm::checkpoint::{ProgramState, RunnerState};
use crate::fes_tasks::FesTaskStore;

/// Proof from the matching prepared consumers, never inferred from raw kind
/// numbers or manufactured property nodes.
#[derive(Default)]
pub(crate) struct CheckpointExtensions {
    pub(crate) inert: BTreeSet<(u32, Vec<u8>, i32)>,
    pub(crate) sliders: BTreeSet<(u32, Vec<u8>, i32)>,
    pub(crate) draws: BTreeMap<(u32, Vec<u8>, i32), (Vec<u8>, i32)>,
    pub(crate) creating_draws: BTreeSet<u32>,
    pub(crate) sounds: BTreeSet<(u32, Vec<u8>, i32)>,
    pub(crate) texts: BTreeSet<(u32, Vec<u8>, i32)>,
}

#[derive(Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct TaskState {
    name: Vec<u8>, group: i32, parameters: Vec<u32>, saved_data: Vec<u8>,
    flags: u32, external: u32, origin: u32, elapsed: u32, running: i32, finishing: i32,
    runner: RunnerState,
}
#[derive(Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct ControllerUpdate {
    now: u32, skip_unstarted: bool, finishing: bool, ids: Vec<u32>, cursor: usize,
    updated: bool, speed: u32, step_done: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct Controller {
    base: Vec<u32>, properties: Vec<u32>, speed: u32, external: u32, next: u32,
    tasks: BTreeMap<u32, TaskState>, pending: Option<ControllerUpdate>,
}
impl Controller {
    fn capture(value: &MotionControllerState) -> Result<Self> {
        validate_controller(value)?;
        Ok(Self { base: value.base_properties.to_vec(), properties: value.properties.to_vec(),
            speed: value.speed_bits, external: value.external_value, next: value.next_task_id,
            tasks: value.tasks.iter().map(|(&id, task)| Ok((id, TaskState {
                name: task.name.clone(), group: task.group, parameters: task.parameters.to_vec(),
                saved_data: task.saved_data.clone(), flags: task.snapshot_flags, external: task.external_value,
                origin: task.origin, elapsed: task.elapsed, running: task.running, finishing: task.finishing,
                runner: RunnerState::capture(&task.runner)?,
            }))).collect::<Result<_>>()?, pending: value.pending.as_ref().map(|p| ControllerUpdate {
                now: p.now, skip_unstarted: p.skip_unstarted, finishing: p.finishing, ids: p.ids.clone(),
                cursor: p.cursor, updated: p.updated, speed: p.speed_bits, step_done: p.step_done }) })
    }
    fn restore(self) -> Result<MotionControllerState> {
        let value = MotionControllerState {
            base_properties: words(self.base)?, properties: words(self.properties)?, speed_bits: self.speed,
            external_value: self.external, next_task_id: self.next,
            tasks: self.tasks.into_iter().map(|(id, task)| Ok((id, MotionTaskState {
                name: task.name, group: task.group, parameters: words(task.parameters)?, saved_data: task.saved_data,
                snapshot_flags: task.flags, external_value: task.external, origin: task.origin,
                elapsed: task.elapsed, running: task.running, finishing: task.finishing, runner: task.runner.restore()?,
            }))).collect::<Result<_>>()?, pending: self.pending.map(|p| crate::MotionControllerUpdate {
                now: p.now, skip_unstarted: p.skip_unstarted, finishing: p.finishing, ids: p.ids, cursor: p.cursor,
                updated: p.updated, speed_bits: p.speed, step_done: p.step_done }),
        };
        validate_controller(&value)?;
        Ok(value)
    }
}
fn words<const N: usize>(value: Vec<u32>) -> Result<[u32; N]> {
    value.try_into().map_err(|_| anyhow::anyhow!("motion checkpoint DWORD count must be {N}"))
}
fn validate_controller(value: &MotionControllerState) -> Result<()> {
    if value.tasks.len() > MAX_NODES || value.next_task_id == 0
        || value.tasks.iter().any(|(&id, task)| id == 0 || id >= value.next_task_id
            || task.name.len() > 259 || task.saved_data.len() > MAX_BYTES as usize)
    { bail!("motion controller checkpoint has invalid task identities/storage"); }
    if let Some(p) = &value.pending {
        if p.ids.len() > MAX_NODES || p.cursor > p.ids.len()
            || p.ids.iter().copied().collect::<BTreeSet<_>>().len() != p.ids.len()
            || p.ids.iter().any(|&id| id == 0 || id >= value.next_task_id)
            || p.step_done && p.cursor == p.ids.len()
        { bail!("motion controller checkpoint has an invalid update cursor"); }
    }
    for (&id, task) in &value.tasks {
        task.runner.validate_checkpoint()?;
        if task.runner.is_pending() && value.pending.as_ref().is_none_or(|p|
            p.step_done || p.ids.get(p.cursor) != Some(&id)
                || task.runner.checkpoint_invocation_finishing() != Some(p.finishing))
        { bail!("motion VM continuation has no matching controller invocation"); }
        // Mirrored parameters/control words are intentionally not compared:
        // a yielded VM has not yet reached the controller's writeback phase.
    }
    Ok(())
}

pub(super) mod controller {
    use super::*;
    pub(crate) fn serialize<S: Serializer>(value: &MotionControllerState, s: S) -> std::result::Result<S::Ok, S::Error> {
        Controller::capture(value).map_err(cs2_runtime::graph_serde::ser::Error::custom)?.serialize(s)
    }
    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<MotionControllerState, D::Error> {
        Controller::deserialize(d)?.restore().map_err(cs2_runtime::graph_serde::de::Error::custom)
    }
}
pub(super) mod writeback {
    use super::*;
    pub(crate) fn serialize<S: Serializer>(value: &Option<[u32; 36]>, s: S) -> std::result::Result<S::Ok, S::Error> {
        value.map(|v| v.to_vec()).serialize(s)
    }
    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<Option<[u32; 36]>, D::Error> {
        Option::<Vec<u32>>::deserialize(d)?.map(words).transpose().map_err(cs2_runtime::graph_serde::de::Error::custom)
    }
}
pub(super) mod programs {
    use super::*;
    pub(crate) fn serialize<S: Serializer>(value: &BTreeMap<String, KcsProgram>, s: S) -> std::result::Result<S::Ok, S::Error> {
        value.iter().map(|(k, p)| (k.clone(), ProgramState::capture(p))).collect::<BTreeMap<_, _>>().serialize(s)
    }
    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<BTreeMap<String, KcsProgram>, D::Error> {
        BTreeMap::<String, ProgramState>::deserialize(d)?.into_iter()
            .map(|(k, p)| p.restore().map(|p| (k, p))).collect::<Result<_>>()
            .map_err(cs2_runtime::graph_serde::de::Error::custom)
    }
}

impl FesMotionStore {
    pub(crate) fn checkpoint_has_object(&self, owner: u32, name: &[u8], bank: i32) -> bool {
        self.owners.get(&owner).is_some_and(|state| state.nodes.contains_key(&(name.to_vec(), bank)))
    }

    pub(crate) fn capture_checkpoint(&self, tasks: &FesTaskStore, encoding: ScriptTextEncoding) -> Result<Vec<u8>> {
        self.capture_checkpoint_with_extensions(tasks, encoding, &CheckpointExtensions::default())
    }
    pub(crate) fn capture_checkpoint_with_extensions(&self, tasks: &FesTaskStore, encoding: ScriptTextEncoding,
        extensions: &CheckpointExtensions) -> Result<Vec<u8>> {
        self.validate_checkpoint(tasks, encoding, extensions)?;
        Ok(wire::encode_component(&(2u32, self))?)
    }
    pub(crate) fn prepare_checkpoint(bytes: &[u8], tasks: &FesTaskStore, encoding: ScriptTextEncoding) -> Result<Self> {
        Self::prepare_checkpoint_with_extensions(bytes, tasks, encoding, &CheckpointExtensions::default())
    }
    pub(crate) fn prepare_checkpoint_with_extensions(bytes: &[u8], tasks: &FesTaskStore, encoding: ScriptTextEncoding,
        extensions: &CheckpointExtensions) -> Result<Self> {
        let (version, store): (u32, Self) = wire::decode_component(bytes)?;
        if version != 2 { bail!("unsupported shared FES motion checkpoint revision (full text pose requires revision2)"); }
        store.validate_checkpoint(tasks, encoding, extensions)?;
        Ok(store)
    }
    fn validate_checkpoint(&self, tasks: &FesTaskStore, encoding: ScriptTextEncoding,
        extensions: &CheckpointExtensions) -> Result<()> {
        if self.owners.len() > MAX_NODES { bail!("FES motion checkpoint exceeds owner bound"); }
        for (owner, name, bank) in tasks.platform.checkpoint_object_identities() {
            if self.owners.get(owner).is_none_or(|state| !state.nodes.contains_key(&(name.clone(), *bank))) {
                bail!("FES CPU checkpoint has no matching shared motion/property owner");
            }
        }
        for (owner, name, bank) in extensions.inert.iter().chain(&extensions.sliders)
            .chain(&extensions.sounds).chain(&extensions.texts) {
            if !self.checkpoint_has_object(*owner, name, *bank) {
                bail!("FES extension checkpoint bank has no original shared motion identity");
            }
        }
        for (owner, name, bank) in extensions.draws.keys() {
            if !self.checkpoint_has_object(*owner, name, *bank) && !extensions.creating_draws.contains(owner) {
                bail!("FES DRAW checkpoint lost its actual common motion node");
            }
        }
        for (&id, owner) in &self.owners {
            let runtime = tasks.runtime(id).context("motion checkpoint owner has no actual FES runtime")?;
            let driver = runtime.driver.as_ref().context("motion checkpoint owner driver is borrowed")?;
            for ((_, key), _) in &owner.sound_blends {
                if !extensions.sounds.contains(&(id, key.0.clone(), key.1))
                    || owner.nodes.get(key).is_none_or(|node| node.kind != Kind::Sound) {
                    bail!("FES SOUND blend checkpoint has no matching physical bank and motion node");
                }
            }
            if owner.nodes.len() > MAX_NODES || owner.commands.len() > MAX_NODES
                || owner.waits.len() > MAX_NODES || owner.declarations.len() > MAX_NODES
                || owner.programs.len() > MAX_NODES || owner.resources.len() > MAX_NODES
                || owner.initial_values.len() > MAX_NODES || owner.initial_parents.len() > MAX_NODES
            { bail!("FES motion checkpoint exceeds registry bounds"); }
            let mut serials = BTreeSet::new();
            for (key, node) in &owner.nodes {
                let definition = usize::try_from(key.1).ok()
                    .and_then(|bank| driver.adapter.object_bank_definition(&key.0, bank))
                    .context("FES motion checkpoint bank has no live declaration")?;
                if node.kind != kind(&definition.kind_name) || node.plane_canvas != (definition.kind_name == b"PLANE")
                    || node.serial >= owner.next_serial || !serials.insert(node.serial)
                    || node.parent.as_ref().is_some_and(|parent| !owner.nodes.contains_key(parent))
                { bail!("FES motion checkpoint identity/kind/parent differs from its live bank"); }
                let identity = (id, key.0.clone(), key.1);
                match node.kind {
                    Kind::Base if definition.kind_name == b"DRAW" => {
                        if extensions.draws.get(&identity) != node.parent.as_ref() {
                            bail!("FES DRAW checkpoint motion parent differs from its actual PLANE owner");
                        }
                    },
                    Kind::Base | Kind::Image | Kind::Plane => {},
                    Kind::Sound if extensions.sounds.contains(&identity)
                        || tasks.checkpoint_sound_creation_pending(id, &key.0) => {},
                    Kind::Slider if extensions.sliders.contains(&identity)
                        || tasks.checkpoint_extension_creation_pending(id, &key.0, false) => {},
                    Kind::Text if extensions.texts.contains(&identity)
                        || tasks.checkpoint_extension_creation_pending(id, &key.0, false) => {},
                    Kind::Other if definition.kind_name.eq_ignore_ascii_case(b"IMGWND")
                        && extensions.inert.contains(&identity) => {},
                    Kind::Other if (extensions.inert.contains(&identity)
                            || extensions.texts.contains(&identity)
                            || tasks.checkpoint_extension_creation_pending(id, &key.0, false)
                            || matches!(definition.kind_name.as_slice(), b"PARTICLE" | b"SAVEPANEL")
                                && tasks.checkpoint_bank_creation_pending(id, &key.0, &definition.kind_name))
                        && node.controller.tasks.is_empty() && node.controller.pending.is_none() => {},
                    _ => bail!("FES motion checkpoint needs the matching object/physical-task consumer"),
                }
                validate_controller(&node.controller)?;
                // A CreateObject effect registers motion before the CPU bank
                // is published. Only existing CPU bindings require pixels here.
                if let Some(cpu_key) = tasks.platform.checkpoint_object_key(id, &key.0, key.1) {
                    if runtime_key(key, runtime, encoding)? != cpu_key {
                        bail!("FES motion checkpoint CPU/runtime bank identities differ");
                    }
                }
            }
            if forest(owner)?.len() != owner.nodes.len() { bail!("FES motion checkpoint has a disconnected parent cycle"); }
            let validate_update = |u: &Update| -> Result<()> {
                if u.order.len() > MAX_NODES || u.cursor > u.order.len()
                    || u.order.iter().collect::<BTreeSet<_>>().len() != u.order.len()
                    || u.order.iter().any(|key| !owner.nodes.contains_key(key))
                    || u.writeback.is_some() && u.cursor == u.order.len()
                    || u.writeback.is_some() && u.order.get(u.cursor).is_some_and(|key| {
                        owner.nodes.get(key).is_some_and(|node| node.kind == Kind::Other)
                            && usize::try_from(key.1).ok()
                                .and_then(|bank| driver.adapter.object_bank_definition(&key.0, bank))
                                .is_none_or(|definition| !definition.kind_name.eq_ignore_ascii_case(b"IMGWND"))
                    })
                { bail!("FES motion checkpoint update/writeback cursor is invalid"); }
                Ok(())
            };
            if let Some(u) = &owner.update { validate_update(u)?; }
            for command in owner.commands.values() {
                if command.cursor > command.keys.len() || command.keys.len() > MAX_NODES
                    || command.values.len() > 16 || command.parameters.as_ref().is_some_and(|p| p.len() > 256)
                    || command.keys.iter().any(|key| !owner.nodes.contains_key(key))
                { bail!("FES motion command checkpoint has an invalid argument/bank cursor"); }
                if let Some(mapped) = &command.mapped { mapped.validate_checkpoint()?; }
                if let Some(u) = &command.stopping { validate_update(u)?; }
            }
            for keys in owner.waits.values().flatten() {
                if keys.len() > MAX_NODES || keys.iter().any(|key| !owner.nodes.contains_key(key)) {
                    bail!("FES motion wait checkpoint contains an absent bank");
                }
            }
        }
        Ok(())
    }
}

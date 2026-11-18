//! One transaction component for retained FES banks and their real kind16
//! consumers. Prepare never calls load/start/update, paints, or draws randoms.
//! C's enclosing graph must retain this component, admit kind16, and add
//! store().checkpoint_bank_keys() to the matching motion-extension proof.
//! Validate_fes_links uses PREPARED drivers; commit installs the pair once.
use super::*;
use crate::{HeadlessPlayer, ParticleObjectState, ResourceIndex};
use crate::fes_tasks::FesTaskStore;
use crate::particle_vm::checkpoint::{Bindings, RunnerState};
use crate::particle_plugin::checkpoint::{ModuleEmissionState, PluginState};
use cs2_runtime::object_graph::{self as wire, ImageFrameSnapshot};
use cs2_runtime::resource_router::ResourceMode;
use cs2_runtime::graph_serde::{Deserializer, Serializer};

// Version4 includes the real process-module emission state, even when its
// last instance has been released. Older unpublished layouts lack that state
// and cannot prove a correct later Concent/Dash load; never invent value1.
const VERSION: u32 = 4;
const LIMIT: usize = 64 * 1024 * 1024;

#[derive(Serialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct Capture<'a> {
    version: u32, store: &'a FesParticleStore,
    particles: BTreeMap<u32, ObjectState>, bindings: Bindings,
    modules: ModuleEmissionState,
}
#[derive(Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct Saved {
    version: u32, store: FesParticleStore,
    particles: BTreeMap<u32, ObjectState>, bindings: Bindings,
    modules: ModuleEmissionState,
}

#[derive(Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct SourceIdentity { digest: String, bytes: usize }

#[derive(Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct ObjectState {
    program_name: String, image_name: String, grid_count: u32,
    position: [i32; 2], color: [u8; 4], depth_bits: u32, opacity: u32, enabled: bool,
    #[serde(with = "parameters")]
    parameters: [u8; 128],
    transform_source: u32, coordinate_group: u32, elapsed: u32,
    points: Vec<([i32; 2], [u8; 4])>, runner: Option<RunnerState>, plugin: Option<PluginState>,
    texture: Option<ImageFrameSnapshot>, rendered: Option<crate::particle_vm::ParticleRenderSnapshot>,
    program_source: Option<SourceIdentity>, image_source: Option<SourceIdentity>,
    creation_config: Vec<u8>, external_extent_bound: bool,
}

/// Owns replacement storage for the existing consumers, never another live VM
/// or bank service. Install only after the entire outer graph has prepared.
pub(crate) struct PreparedParticles {
    store: FesParticleStore,
    particles: BTreeMap<u32, ParticleObjectState>,
    modules: ModuleEmissionState,
}

impl PreparedParticles {
    pub(crate) fn store(&self) -> &FesParticleStore { &self.store }

    pub(crate) fn validate_fes_links(&self, tasks: &FesTaskStore) -> Result<()> {
        self.store.validate_fes_links(tasks)
    }

    pub(crate) fn commit(self, player: &mut HeadlessPlayer) {
        // No I/O, interpreter replay, resource resolution, SDK start, clock
        // rebasing or RNG replacement belongs to the commit phase.
        player.particles = self.particles;
        player.data_services.fes_particles = self.store;
        self.modules.commit();
    }
}

impl HeadlessPlayer {
    pub(crate) fn capture_particle_checkpoint(&self, kinds: &BTreeMap<u32, u32>) -> Result<Vec<u8>> {
        let store = &self.data_services.fes_particles;
        store.validate_checkpoint_links(kinds, &self.task_parents)?;
        store.validate_fes_links(&self.data_services.tasks.fes)?;
        let (bindings, ids) = Bindings::capture(self.particles.iter()
            .filter_map(|(&id, state)| state.runner.as_ref().map(|runner| (id, runner))))?;
        let mut particles = BTreeMap::new();
        for (&id, state) in &self.particles {
            if kinds.get(&id) != Some(&16) { bail!("particle checkpoint has no kind16 task owner"); }
            particles.insert(id, ObjectState::capture(state, &self.scan.resources, ids.get(&id).copied())?);
        }
        validate_bank_consumers(store, &self.particles)?;
        let modules = ModuleEmissionState::capture();
        Ok(wire::encode_component(&Capture { version: VERSION, store, particles, bindings, modules })?)
    }
}

impl FesParticleStore {
    pub(crate) fn checkpoint_bank_keys(&self) -> impl Iterator<Item = &Key> { self.banks.keys() }

    pub(crate) fn checkpoint_lower_owner(&self, lower: u32) -> Option<u32> {
        self.banks.iter().find_map(|((owner, _, _), bank)| (bank.lower == lower).then_some(*owner))
    }

    /// This can run before the prepared FES drivers. Validate_fes_links must
    /// then run against those prepared drivers before a graph can commit.
    pub(crate) fn prepare_checkpoint(
        bytes: &[u8], resources: &ResourceIndex, kinds: &BTreeMap<u32, u32>, parents: &BTreeMap<u32, u32>,
    ) -> Result<PreparedParticles> {
        let saved: Saved = wire::decode_component(bytes)
            .context("particle checkpoint requires v4 including shared module emission state")?;
        if saved.version != VERSION || saved.particles.len() > MAX_BANKS {
            bail!("particle checkpoint version or task storage is unsupported");
        }
        saved.store.validate_checkpoint_links(kinds, parents)?;
        let bindings = saved.bindings.prepare()?;
        let mut particles = BTreeMap::new();
        for (id, state) in saved.particles {
            if kinds.get(&id) != Some(&16) { bail!("saved particle consumer has no kind16 task owner"); }
            particles.insert(id, state.prepare(resources, &bindings)?);
        }
        validate_bank_consumers(&saved.store, &particles)?;
        Ok(PreparedParticles { store: saved.store, particles, modules: saved.modules })
    }

    pub(crate) fn validate_checkpoint_links(
        &self, kinds: &BTreeMap<u32, u32>, parents: &BTreeMap<u32, u32>,
    ) -> Result<()> {
        let owner = |id: &u32| kinds.get(id) == Some(&34);
        if [self.banks.len(), self.declarations.len(), self.creations.len(), self.initializers.len(),
            self.commands.len(), self.updates.len()].into_iter().any(|n| n > MAX_BANKS) {
            bail!("particle bank checkpoint exceeds live storage bounds");
        }
        let mut lower = BTreeSet::new();
        let mut serial = BTreeSet::new();
        for ((id, name, index), bank) in &self.banks {
            if !owner(id) || name.is_empty() || name.len() > 31 || name.contains(&0) || *index < 0
                || kinds.get(&bank.lower) != Some(&16) || parents.get(&bank.lower) != Some(id)
                || !lower.insert(bank.lower) || bank.serial >= self.next_serial || !serial.insert(bank.serial)
                || bank.program.len() >= 64 || bank.program.contains(&0) || bank.image.len() >= 64 || bank.image.contains(&0)
                || bank.load_record.as_ref().is_some_and(|v| v.len() != 0x290 || !bank.load_dirty || bank.released)
                || bank.released && bank.playing {
                bail!("particle bank checkpoint owner/order/load record is invalid");
            }
        }
        if self.declarations.iter().any(|(id, _)| !owner(id))
            || self.creations.iter().any(|((id, offset), cursor)| !owner(id) || *cursor > MAX_BANKS
                || self.declarations.contains(&(*id, *offset)))
            || self.initializers.iter().any(|((id, _), cursor)| !owner(id) || *cursor > MAX_BANKS) {
            bail!("particle bank checkpoint declaration/initializer continuation is invalid");
        }
        for ((id, _), command) in &self.commands {
            if !owner(id) || command.values.len() > 32 || command.keys.len() > MAX_BANKS
                || command.keys.iter().any(|key| key.0 != *id || !self.banks.contains_key(key))
                || command.keys.iter().collect::<BTreeSet<_>>().len() != command.keys.len() {
                bail!("particle command checkpoint lost evaluated parameters or bank ownership");
            }
        }
        for (id, update) in &self.updates {
            if !owner(id) || update.stage > 5 || update.cursor > update.keys.len() || update.keys.len() > MAX_BANKS
                || update.keys.iter().any(|key| key.0 != *id || !self.banks.contains_key(key))
                || update.keys.iter().collect::<BTreeSet<_>>().len() != update.keys.len()
                || update.cursor == update.keys.len() && update.stage != 0 {
                bail!("particle media checkpoint phase or declaration order is invalid");
            }
            let mut previous = None;
            for key in &update.keys {
                let next = self.banks[key].serial;
                if previous.is_some_and(|last| last >= next) { bail!("particle media checkpoint bank order changed"); }
                previous = Some(next);
            }
        }
        Ok(())
    }

    pub(crate) fn validate_fes_links(&self, tasks: &FesTaskStore) -> Result<()> {
        for (owner, name, index) in self.banks.keys() {
            tasks.checkpoint_bank_definition(*owner, name, *index, b"PARTICLE")?;
        }
        for owner in self.declarations.iter().map(|(id, _)| *id)
            .chain(self.creations.keys().map(|(id, _)| *id))
            .chain(self.initializers.keys().map(|(id, _)| *id))
            .chain(self.commands.keys().map(|(id, _)| *id)).chain(self.updates.keys().copied()) {
            if tasks.runtime(owner).and_then(|runtime| runtime.driver.as_ref()).is_none() {
                bail!("particle checkpoint pending receipt has no FES driver");
            }
        }
        for (owner, offset) in &self.declarations {
            let driver = tasks.runtime(*owner).unwrap().driver.as_ref().unwrap();
            if !driver.interpreter.program.initialization_plan()?.definitions.iter()
                .any(|d| d.offset == *offset && d.kind.eq_ignore_ascii_case(b"PARTICLE")) {
                bail!("particle declaration receipt lost its original FES producer");
            }
        }
        for ((owner, offset), cursor) in &self.creations {
            let definition = tasks.checkpoint_pending_effects(*owner)?.find_map(|effect| match effect {
                FesDriverEffect::Initialize { event: FesEvent::CreateObject(d), .. }
                    if d.offset == *offset && d.kind.eq_ignore_ascii_case(b"PARTICLE") => Some(d),
                _ => None,
            }).context("particle creation checkpoint lost its retained constructor effect")?;
            let driver = tasks.runtime(*owner).unwrap().driver.as_ref().unwrap();
            if driver.adapter.object(&definition.name).is_none_or(|object| *cursor > object.definition.bank_count) {
                bail!("particle creation checkpoint cursor exceeds its real declaration");
            }
        }
        for ((owner, offset), cursor) in &self.initializers {
            let driver = tasks.runtime(*owner).unwrap().driver.as_ref().unwrap();
            let plan = driver.interpreter.program.initialization_plan()?;
            let init = plan.initializers.iter().find(|init| init.offset == *offset)
                .context("particle initializer receipt lost its original FES row")?;
            if *cursor > init.columns.len().min(init.values.len()) {
                bail!("particle initializer checkpoint column cursor is invalid");
            }
        }
        for ((owner, offset), progress) in &self.commands {
            let command = tasks.checkpoint_pending_effects(*owner)?.find_map(|effect| match effect {
                FesDriverEffect::Command(command) if command.offset == *offset => Some(command),
                _ => None,
            }).context("particle command checkpoint lost its retained driver effect")?;
            let verb = command.parameters.get(1).context("particle checkpoint command verb absent")?;
            let count = match verb.as_slice() {
                b"init" => 3, b"param" => command.parameters.len().saturating_sub(2).min(32),
                b"blend" => 1, b"start" | b"stop" => 0,
                _ => bail!("particle command checkpoint belongs to another receiver"),
            };
            let target = command.parameters.first().context("particle checkpoint command target absent")?;
            if progress.values.len() > count || progress.keys.iter().any(|key|
                target.as_slice() != key.1.as_slice() && !target.strip_prefix(key.1.as_slice())
                    .is_some_and(|suffix| suffix.first() == Some(&b'[') && suffix.last() == Some(&b']'))) {
                bail!("particle command checkpoint evaluated arguments differ from its producer");
            }
        }
        for (owner, update) in &self.updates {
            tasks.validate_checkpoint_media_tick(*owner, update.now)?;
        }
        Ok(())
    }
}

impl SourceIdentity {
    fn capture(bytes: &[u8]) -> Self {
        Self { digest: crate::particle_plugin_digest::sha256(bytes), bytes: bytes.len() }
    }
    fn verify(&self, bytes: &[u8]) -> Result<()> {
        if self.bytes > LIMIT || self.bytes != bytes.len() || self.digest != crate::particle_plugin_digest::sha256(bytes) {
            bail!("particle checkpoint resource identity changed; live graph retained");
        }
        Ok(())
    }
}

fn resource(resources: &ResourceIndex, name: &str, plugin: bool) -> Result<Vec<u8>> {
    if name.is_empty() || name.contains('\0') || name.encode_utf16().count() >= 260 {
        bail!("particle checkpoint source name is invalid");
    }
    let bytes = if plugin {
        let candidate = resources.resolve_plain_file(name)?.context("saved PPI plain-file source is absent")?;
        resources.read_candidate(candidate)?
    } else {
        resources.read_mode(name, ResourceMode::LooseFirst)?.context("saved particle archive/loose source is absent")?
    };
    if bytes.len() > LIMIT { bail!("particle checkpoint resource exceeds64 MiB"); }
    Ok(bytes)
}

fn validate_bank_consumers(store: &FesParticleStore, particles: &BTreeMap<u32, ParticleObjectState>) -> Result<()> {
    for bank in store.banks.values() {
        let loaded = particles.get(&bank.lower).is_some_and(|p| p.runner.is_some() || p.plugin.is_some());
        // playing is the last324 observation, not an alias of current active.
        // A failed destructive reload or an ordinary323 may leave it stale
        // until the retained owner media phase queries324 again.
        if bank.released && loaded {
            bail!("particle bank checkpoint lost its actual loaded/released lower consumer");
        }
    }
    Ok(())
}

impl ObjectState {
    fn capture(state: &ParticleObjectState, resources: &ResourceIndex, bindings: Option<(usize, usize)>) -> Result<Self> {
        if state.external_draw != 0 { bail!("external particle shared-graph checkpoint is not implemented"); }
        let live = state.runner.is_some() || state.plugin.is_some();
        let program_source = if live {
            let bytes = resource(resources, &state.program_name, state.plugin.is_some())?;
            if let Some(runner) = &state.runner {
                if crate::particle_vm::ParticleProgram::parse(&bytes)?.program() != runner.program().program() {
                    bail!("particle program resource differs from its retained interpreter");
                }
            }
            if let Some(plugin) = &state.plugin {
                let digest = crate::particle_plugin_digest::sha256(&bytes);
                if plugin.checkpoint_digest() != digest.as_str() {
                    bail!("PPI resource differs from its retained exact-SHA model");
                }
            }
            Some(SourceIdentity::capture(&bytes))
        } else { None };
        let image_source = if let Some(texture) = &state.texture {
            let bytes = resource(resources, &state.image_name, false)?;
            let decoded = crate::image_manager::decode_image_frame(&bytes, 0)?.context("saved particle texture frame0 absent")?;
            // Current real callers use raw frame0. Do not silently lose an
            // unrelated cached/translated replacement by rebinding raw pixels.
            if &decoded != texture { bail!("particle texture differs from its canonical resource; replacement codec required"); }
            Some(SourceIdentity::capture(&bytes))
        } else { None };
        let saved = Self { program_name: state.program_name.clone(), image_name: state.image_name.clone(), grid_count: state.grid_count,
            position: [state.position.x, state.position.y], color: [state.color.r, state.color.g, state.color.b, state.color.a],
            depth_bits: state.depth_bits, opacity: state.opacity, enabled: state.enabled, parameters: state.parameters,
            transform_source: state.transform_source, coordinate_group: state.coordinate_group, elapsed: state.elapsed,
            points: state.points.iter().map(|p| ([p.position.x, p.position.y], [p.color.r, p.color.g, p.color.b, p.color.a])).collect(),
            runner: state.runner.as_ref().map(|runner| RunnerState::capture(runner, bindings.context("particle runner alias absent")?)).transpose()?,
            plugin: state.plugin.as_ref().map(|plugin| plugin.capture_checkpoint()).transpose()?,
            texture: state.texture.as_ref().map(crate::image_manager::capture_frame), rendered: state.rendered.clone(),
            program_source, image_source, creation_config: state.creation_config.clone(), external_extent_bound: state.external_extent_bound };
        saved.validate()?;
        Ok(saved)
    }

    fn validate(&self) -> Result<()> {
        let live = self.runner.is_some() || self.plugin.is_some();
        if self.runner.is_some() && self.plugin.is_some() || live != self.program_source.is_some()
            || self.texture.is_some() != self.image_source.is_some()
            || !live && (self.texture.is_some() || self.rendered.is_some() || self.enabled || !self.points.is_empty())
            || self.creation_config.len() > 256 || self.creation_config.contains(&0)
            || self.program_name.len() > 4096 || self.image_name.len() > 4096
            || (self.grid_count as usize).checked_mul(244).is_none_or(|n| n > LIMIT)
            || self.points.len() > self.grid_count as usize
            || self.rendered.as_ref().is_some_and(|r| r.quads.len() != self.grid_count as usize
                || r.current_quad > self.grid_count || !r.frame_complete
                || r.quads.iter().enumerate().any(|(i, q)| q.index as usize != i)) {
            bail!("particle checkpoint retained control/geometry/texture state is invalid");
        }
        Ok(())
    }

    fn prepare(self, resources: &ResourceIndex, bindings: &crate::particle_vm::checkpoint::PreparedBindings) -> Result<ParticleObjectState> {
        self.validate()?;
        let program_bytes = self.program_source.as_ref().map(|source| -> Result<Vec<u8>> {
            let bytes = resource(resources, &self.program_name, self.plugin.is_some())?;
            source.verify(&bytes)?;
            Ok(bytes)
        }).transpose()?;
        let texture = match (&self.texture, &self.image_source) {
            (Some(saved), Some(source)) => {
                let bytes = resource(resources, &self.image_name, false)?;
                source.verify(&bytes)?;
                let frame = crate::image_manager::restore_frame(saved)?;
                if crate::image_manager::decode_image_frame(&bytes, 0)?.as_ref() != Some(&frame) {
                    bail!("particle checkpoint pixels/metadata differ from their rebound frame0");
                }
                Some(frame)
            }
            (None, None) => None,
            _ => bail!("saved particle texture identity is incomplete"),
        };
        let extent = texture.as_ref().map_or([0.; 2], |f| [f.width() as f32, f.height() as f32]);
        let runner = self.runner.map(|state| state.prepare(bindings)).transpose()?;
        if let Some(runner) = &runner {
            let source = crate::particle_vm::ParticleProgram::parse(program_bytes.as_deref().context("saved particle KCS bytes absent")?)?;
            if source.program() != runner.program().program() || runner.quads().len() != self.grid_count as usize || runner.image_size() != extent {
                bail!("particle checkpoint VM differs from its rebound program/grid/texture extent");
            }
        }
        let plugin = self.plugin.map(|state| state.prepare(program_bytes.as_deref().context("saved PPI bytes absent")?, self.grid_count, extent)).transpose()?;
        Ok(ParticleObjectState { program_name: self.program_name, image_name: self.image_name, grid_count: self.grid_count,
            position: crate::Point::new(self.position[0], self.position[1]),
            color: crate::Color::rgba(self.color[0], self.color[1], self.color[2], self.color[3]),
            depth_bits: self.depth_bits, opacity: self.opacity, enabled: self.enabled, parameters: self.parameters,
            transform_source: self.transform_source, coordinate_group: self.coordinate_group, elapsed: self.elapsed,
            points: self.points.into_iter().map(|(p, c)| crate::ParticlePoint::new(crate::Point::new(p[0], p[1]),
                crate::Color::rgba(c[0], c[1], c[2], c[3]))).collect(), runner, plugin, texture, rendered: self.rendered,
            external_draw: 0, creation_config: self.creation_config, external_extent_bound: self.external_extent_bound })
    }
}

pub(super) mod parameters {
    use super::*;
    pub(crate) fn serialize<S: Serializer>(value: &[u8; 128], serializer: S) -> std::result::Result<S::Ok, S::Error> {
        value.as_slice().serialize(serializer)
    }
    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> std::result::Result<[u8; 128], D::Error> {
        Vec::<u8>::deserialize(deserializer)?.try_into().map_err(|_| cs2_runtime::graph_serde::de::Error::custom("particle parameter record must be128 bytes"))
    }
}

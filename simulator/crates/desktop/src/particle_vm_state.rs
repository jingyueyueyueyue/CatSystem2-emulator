//! Host checkpoint of the actual particle interpreter, including budget yields.
use super::*;
use anyhow::{Context, Result, bail};
use cs2_script::VmSnapshotGroup;
use std::collections::BTreeMap;

#[derive(Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(crate) struct RunnerState {
    vm: VmSnapshotGroup,
    canvas: ParticleCanvas,
    quads: Vec<[ParticleVertex; 4]>, saved_quads: Vec<[ParticleVertex; 4]>,
    offsets: Vec<[f32; 4]>, flags: Vec<u32>,
    external: usize, parameters: usize, parameter_external_offset: Option<u32>,
    elapsed: u32, previous_elapsed: u32, active: bool, current_quad: u32,
    object_id: i32, source: Option<ParticleSource>, render_mode: u32, opacity: u32,
    debug_messages: Vec<Vec<u8>>, image_size: [f32; 2], origin: u32,
    previous_time: u32, round_open: bool, invocation_open: bool,
    ordinary_draw_registered: bool,
}

/// Host-side aliases are independent of the VM's per-slice external copy.
/// Keep both: an unrun runner may legitimately hold an older VM copy.
#[derive(Default, Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(crate) struct Bindings { external: Vec<Vec<u8>>, parameters: Vec<Vec<u8>> }

pub(crate) struct PreparedBindings {
    external: Vec<ParticleExternal>, parameters: Vec<ParticleParameters>,
}

impl Bindings {
    pub(crate) fn capture<'a>(runners: impl Iterator<Item = (u32, &'a ParticleVm)>)
        -> Result<(Self, BTreeMap<u32, (usize, usize)>)> {
        let mut saved = Self::default();
        let mut external = BTreeMap::<usize, (usize, usize, usize)>::new();
        let mut parameters = BTreeMap::<usize, (usize, usize, usize)>::new();
        let mut ids = BTreeMap::new();
        for (id, runner) in runners {
            let ep = Arc::as_ptr(&runner.external) as usize;
            let pp = Arc::as_ptr(&runner.state.parameters) as usize;
            if !external.contains_key(&ep) {
                let bytes = runner.external.lock().map_err(|_| anyhow::anyhow!("particle external lock poisoned"))?.clone();
                if bytes.len() > MAX_STORAGE { bail!("particle external checkpoint exceeds storage limit"); }
                external.insert(ep, (saved.external.len(), 0, Arc::strong_count(&runner.external)));
                saved.external.push(bytes);
            }
            if !parameters.contains_key(&pp) {
                let bytes = *runner.state.parameters.lock().map_err(|_| anyhow::anyhow!("particle parameter lock poisoned"))?;
                parameters.insert(pp, (saved.parameters.len(), 0, Arc::strong_count(&runner.state.parameters)));
                saved.parameters.push(bytes.to_vec());
            }
            let e = external.get_mut(&ep).unwrap(); e.1 += 1;
            let p = parameters.get_mut(&pp).unwrap(); p.1 += 1;
            ids.insert(id, (e.0, p.0));
        }
        // No observed FES producer exposes these Arcs. Reject unowned outside
        // aliases rather than serialize an address or silently split ownership.
        if external.values().chain(parameters.values()).any(|(_, used, live)| used != live) {
            bail!("particle checkpoint has a host alias outside its runner group");
        }
        Ok((saved, ids))
    }

    pub(crate) fn prepare(self) -> Result<PreparedBindings> {
        if self.external.len() > 65_536 || self.parameters.len() > 65_536
            || self.external.iter().any(|v| v.len() > MAX_STORAGE)
            || self.parameters.iter().any(|v| v.len() != PARAMETER_BYTES) {
            bail!("particle checkpoint shared binding storage is invalid");
        }
        Ok(PreparedBindings {
            external: self.external.into_iter().map(|v| Arc::new(Mutex::new(v))).collect(),
            parameters: self.parameters.into_iter().map(|v|
                Arc::new(Mutex::new(v.try_into().unwrap()))).collect(),
        })
    }
}

impl RunnerState {
    pub(crate) fn capture(runner: &ParticleVm, bindings: (usize, usize)) -> Result<Self> {
        runner.validate_checkpoint()?;
        let s = &runner.state;
        Ok(Self { vm: VmSnapshotGroup::capture([(0, &runner.vm)])?, canvas: s.canvas,
            quads: s.quads.clone(), saved_quads: s.saved_quads.clone(), offsets: s.offsets.clone(), flags: s.flags.clone(),
            external: bindings.0, parameters: bindings.1, parameter_external_offset: s.parameter_external_offset,
            elapsed: s.elapsed, previous_elapsed: s.previous_elapsed, active: s.active, current_quad: s.current_quad,
            object_id: s.object_id, source: s.source, render_mode: s.render_mode, opacity: s.opacity,
            debug_messages: s.debug_messages.clone(), image_size: runner.image_size, origin: runner.origin,
            previous_time: runner.previous_time, round_open: runner.round_open, invocation_open: runner.invocation_open,
            ordinary_draw_registered: runner.ordinary_draw_registered })
    }

    pub(crate) fn prepare(self, bindings: &PreparedBindings) -> Result<ParticleVm> {
        if self.vm.programs.len() != 1 || !self.vm.programs.contains_key(&0) || self.vm.external_regions.len() != 1 {
            bail!("particle checkpoint must contain its one actual interpreter");
        }
        let vm = self.vm.restore()?.remove(&0).context("particle checkpoint VM absent")?;
        let program = ParticleProgram::from_program(vm.program().context("particle checkpoint program absent")?.clone())?;
        let runner = ParticleVm { program, vm, state: ParticleState {
            canvas: self.canvas, quads: self.quads, saved_quads: self.saved_quads, offsets: self.offsets, flags: self.flags,
            parameters: bindings.parameters.get(self.parameters).context("particle parameter alias absent")?.clone(),
            parameter_external_offset: self.parameter_external_offset, elapsed: self.elapsed,
            previous_elapsed: self.previous_elapsed, active: self.active, current_quad: self.current_quad,
            object_id: self.object_id, source: self.source, render_mode: self.render_mode, opacity: self.opacity,
            debug_messages: self.debug_messages },
            external: bindings.external.get(self.external).context("particle external alias absent")?.clone(),
            image_size: self.image_size, origin: self.origin, previous_time: self.previous_time,
            round_open: self.round_open, invocation_open: self.invocation_open,
            ordinary_draw_registered: self.ordinary_draw_registered, fault: None };
        runner.validate_checkpoint()?;
        Ok(runner)
    }
}

impl ParticleVm {
    fn validate_checkpoint(&self) -> Result<()> {
        let s = &self.state;
        let count = s.quads.len();
        if self.fault.is_some() { bail!("faulted particle runner has no portable error-state codec"); }
        if self.vm.native_context_identity().is_some() || self.vm.program() != Some(self.program.program())
            || count.checked_mul(QUAD_BYTES * 2 + 20).is_none_or(|n| n > MAX_STORAGE)
            || s.saved_quads.len() != count || s.offsets.len() != count || s.flags.len() != count
            || s.current_quad as usize > count || self.invocation_open && !self.round_open
            || s.debug_messages.len() > 65_536
            || s.debug_messages.iter().try_fold(0usize, |a, v| a.checked_add(v.len())).is_none_or(|n| n > MAX_STORAGE) {
            bail!("particle checkpoint interpreter/geometry continuation is invalid");
        }
        // Validates an external parameter range without changing either copy.
        self.parameters()?;
        Ok(())
    }
}

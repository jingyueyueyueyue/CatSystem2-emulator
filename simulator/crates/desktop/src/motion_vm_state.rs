//! Host checkpoint of a motion invocation, including an instruction-budget yield.
//! The original motion snapshot is an idle script ABI and cannot represent this.
use super::*;
use anyhow::{Context, Result, bail};
use cs2_runtime::graph_serde::{Deserialize, Serialize};
use cs2_script::{KcsHeader, VmSnapshotGroup};

#[derive(Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(crate) struct ProgramState {
    header: KcsHeader, code: Vec<u8>, data: Vec<u8>, stack: usize,
}
impl ProgramState {
    pub(crate) fn capture(program: &KcsProgram) -> Self {
        Self { header: program.header.clone(), code: program.code.clone(),
            data: program.data.clone(), stack: program.stack_size }
    }
    pub(crate) fn restore(self) -> Result<KcsProgram> {
        if self.header.format_version != 2 || self.header.header_length != 52
            || self.header.code_length as usize != self.code.len()
            || self.header.data_length as usize != self.data.len()
            || self.header.stack_length as usize != self.stack
            || self.code.len() > LIMIT || self.data.len().checked_add(self.stack).is_none_or(|n| n > LIMIT)
        { bail!("motion checkpoint cached program header/storage is invalid"); }
        let mut program = KcsProgram::from_code(&self.code, self.data, self.stack)?;
        program.header = self.header;
        Ok(program)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(crate) struct RunnerState {
    vm: VmSnapshotGroup, base_time: u32, stack_end: usize, storage_end: usize,
    invocation: Option<(u32, [u32; 3])>, name: Vec<u16>, flags: u32,
    snapshot_data: Option<(u32, usize)>,
}
impl RunnerState {
    pub(crate) fn capture(runner: &MotionRunner) -> Result<Self> {
        runner.validate_checkpoint()?;
        Ok(Self { vm: VmSnapshotGroup::capture([(0, &runner.vm)])?,
            base_time: runner.base_time, stack_end: runner.stack_end, storage_end: runner.storage_end,
            invocation: runner.invocation.map(|v| match v {
                Invocation::Update => (0, [0; 3]), Invocation::Finish(words) => (1, words) }),
            name: runner.name.clone(), flags: runner.snapshot_flags, snapshot_data: runner.snapshot_data })
    }
    pub(crate) fn restore(self) -> Result<MotionRunner> {
        if self.vm.programs.len() != 1 || !self.vm.programs.contains_key(&0)
            || self.vm.external_regions.len() != 1 || self.vm.external_regions[0].len() != EXTERNAL_SIZE
        { bail!("motion checkpoint must contain its one private VM/external region"); }
        let mut vms = self.vm.restore()?;
        let vm = vms.remove(&0).context("motion checkpoint VM is absent")?;
        let program = MotionProgram::from_program(vm.program().context("motion checkpoint program is absent")?.clone())?;
        let invocation = match self.invocation {
            None => None, Some((0, [0, 0, 0])) => Some(Invocation::Update),
            Some((1, words)) => Some(Invocation::Finish(words)),
            _ => bail!("motion checkpoint invocation tag is invalid"),
        };
        let runner = MotionRunner { program, vm, base_time: self.base_time, stack_end: self.stack_end,
            storage_end: self.storage_end, invocation, name: self.name, snapshot_flags: self.flags,
            snapshot_data: self.snapshot_data };
        runner.validate_checkpoint()?;
        Ok(runner)
    }
}

impl MotionRunner {
    pub(crate) fn checkpoint_invocation_finishing(&self) -> Option<bool> {
        self.invocation.map(|invocation| matches!(invocation, Invocation::Finish(_)))
    }

    pub(crate) fn validate_checkpoint(&self) -> Result<()> {
        if self.vm.native_context_identity().is_some() || self.vm.external_bytes()?.len() != EXTERNAL_SIZE
            || self.vm.program() != Some(self.program.program())
            || self.stack_end != self.program.program.data.len() + self.program.program.stack_size
            || self.storage_end < self.stack_end || self.storage_end > LIMIT || self.name.len() > 31
            || String::from_utf16(&self.name).is_err()
        { bail!("motion checkpoint runner metadata differs from its actual VM"); }
        if let Some((address, length)) = self.snapshot_data {
            if address == 0 || length > LIMIT { bail!("motion checkpoint saved-data registration is invalid"); }
            self.read_memory(address, length)?;
        }
        // Live script pointer words may be changed before native4 registers a
        // new snapshot source. Preserve those words separately, as execution does.
        self.state()?;
        Ok(())
    }
}

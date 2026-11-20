//! Host checkpoint of a motion invocation, including an instruction-budget yield.
//! The original motion snapshot is an idle script ABI and cannot represent this.
use super::*;
use anyhow::{Context, Result, bail};
use cs2_runtime::graph_serde::{Deserialize, Serialize};
use cs2_script::{KcsHeader, VmSnapshotGroup};

const SHORT_EXTERNAL_TAG: &[u8; 4] = b"CM01";

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
        let mut vm = VmSnapshotGroup::capture([(0, &runner.vm)])?;
        if runner.layout == ExternalLayout::Campanella {
            vm.external_regions[0].extend_from_slice(SHORT_EXTERNAL_TAG);
            vm.external_regions[0].extend_from_slice(&runner.short_display.to_le_bytes());
        }
        Ok(Self { vm,
            base_time: runner.base_time, stack_end: runner.stack_end, storage_end: runner.storage_end,
            invocation: runner.invocation.map(|v| match v {
                Invocation::Update => (0, [0; 3]), Invocation::Finish(words) => (1, words) }),
            name: runner.name.clone(), flags: runner.snapshot_flags, snapshot_data: runner.snapshot_data })
    }
    pub(crate) fn restore(self) -> Result<MotionRunner> {
        if self.vm.programs.len() != 1 || !self.vm.programs.contains_key(&0)
            || self.vm.external_regions.len() != 1
        { bail!("motion checkpoint must contain its one private VM/external region"); }
        let mut snapshot = self.vm;
        let external = &mut snapshot.external_regions[0];
        let short_size = ExternalLayout::Campanella.size();
        let (layout, short_display) = match external.len() {
            EXTERNAL_SIZE => (ExternalLayout::Default, 0),
            len if len == short_size + 8 && &external[short_size..short_size + 4] == SHORT_EXTERNAL_TAG => {
                let display = word_at(external, short_size + 4);
                external.truncate(short_size);
                (ExternalLayout::Campanella, display)
            }
            _ => bail!("motion checkpoint external layout is invalid"),
        };
        let mut vms = snapshot.restore()?;
        let vm = vms.remove(&0).context("motion checkpoint VM is absent")?;
        let program = MotionProgram::from_program(vm.program().context("motion checkpoint program is absent")?.clone())?;
        let invocation = match self.invocation {
            None => None, Some((0, [0, 0, 0])) => Some(Invocation::Update),
            Some((1, words)) => Some(Invocation::Finish(words)),
            _ => bail!("motion checkpoint invocation tag is invalid"),
        };
        let runner = MotionRunner { program, vm, layout, short_display, base_time: self.base_time, stack_end: self.stack_end,
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
        if self.vm.native_context_identity().is_some() || self.vm.external_bytes()?.len() != self.layout.size()
            || self.layout == ExternalLayout::Default && self.short_display != 0
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

#[cfg(test)]
mod tests {
    use super::*;
    use cs2_runtime::object_graph::{decode_component, encode_component};
    use cs2_script::Operand;

    fn emit(code: &mut Vec<u8>, opcode: u8, operands: &[Operand]) {
        code.push(opcode);
        for operand in operands {
            code.push(operand.flags.bits());
            code.extend_from_slice(&operand.payload.to_le_bytes());
        }
    }

    fn fade_program(elapsed: u32, finishing: u32, params: usize) -> MotionProgram {
        let elapsed = Operand::new(0x13, EXTERNAL + elapsed);
        let finishing = Operand::new(0x13, EXTERNAL + finishing);
        let alpha = Operand::new(0x11, EXTERNAL + 0x73);
        let param = |index: u32| Operand::new(0x14, EXTERNAL + params as u32 + index * 4);
        let stack = Operand::new(0x43, 0);
        let stack_float = Operand::new(0x44, 0);
        let imm = Operand::immediate;
        let mut code = Vec::new();
        emit(&mut code, 0x36, &[elapsed]);
        emit(&mut code, 0x11, &[stack_float, param(0)]);
        emit(&mut code, 0x07, &[stack, finishing]);
        let branch = code.len();
        emit(&mut code, 0x28, &[imm(0)]);
        for operand in [param(1), param(2), imm(0), param(0)] {
            emit(&mut code, 0x25, &[operand]);
        }
        emit(&mut code, 0x36, &[elapsed]);
        emit(&mut code, 0x2b, &[imm(41), imm(20)]);
        emit(&mut code, 0x39, &[stack_float]);
        emit(&mut code, 0x26, &[alpha]);
        code.push(0x3b);
        let endpoint = code.len() as u32;
        code[branch + 2..branch + 6].copy_from_slice(&endpoint.to_le_bytes());
        emit(&mut code, 0x39, &[param(2)]);
        emit(&mut code, 0x26, &[alpha]);
        emit(&mut code, 0x2b, &[imm(38), imm(0)]);
        code.push(0x3b);
        MotionProgram::from_program(KcsProgram::from_code(&code, Vec::new(), 64).unwrap()).unwrap()
    }

    #[test]
    fn campanella_and_default_fade_checkpoint() {
        for (elapsed, running, finishing, params, save, external_value, size, layout) in [
            (0x8c, 0x90, 0x94, 0x98, 0x498, 0x4a0, 0x4a4, ExternalLayout::Campanella),
            (0x90, 0x94, 0x98, 0x9c, 0x49c, 0x4a4, 0x4a8, ExternalLayout::Default),
        ] {
            let mut properties = [0; 36];
            properties[28] = 0xffff_ffff;
            properties[35] = 1;
            let mut parameters = [0; 256];
            parameters[0] = 60_f32.to_bits();
            parameters[2] = 255_f32.to_bits();
            let start = MotionStart {
                name: b"motFade", parameters, external_value: 123, properties,
                base_time: 10, saved_data: b"saved", snapshot_flags: 2,
            };
            let program = fade_program(elapsed, finishing, params);
            let mut runner = if layout == ExternalLayout::Default {
                MotionRunner::from_program(program, start).unwrap()
            } else {
                MotionRunner::with_external_layout(program, start, layout).unwrap()
            };
            let initial = runner.external_bytes().unwrap();
            assert_eq!(initial.len(), size);
            assert_eq!(word_at(&initial, running), 1);
            assert_eq!(word_at(&initial, params), parameters[0]);
            assert_ne!(word_at(&initial, save), 0);
            assert_eq!(word_at(&initial, save + 4), 5);
            assert_eq!(word_at(&initial, external_value), 123);

            let mut host = MotionHost::default();
            assert_eq!(runner.update(9, &mut host, 64).unwrap(), MotionProgress::NotStarted);
            assert_eq!(runner.update(10, &mut host, 64).unwrap(), MotionProgress::Complete(Step::Halted));
            assert_eq!(runner.properties().unwrap()[28] >> 24, 0);
            let mut properties = runner.properties().unwrap();
            properties[35] = 3;
            runner.set_properties(&properties).unwrap();
            assert_eq!(runner.update(40, &mut host, 1).unwrap(), MotionProgress::Yield);
            let state = RunnerState::capture(&runner).unwrap();
            if layout == ExternalLayout::Default {
                assert_eq!(state.vm, VmSnapshotGroup::capture([(0, &runner.vm)]).unwrap());
                assert_eq!(word_at(&state.vm.external_regions[0], 0x8c), 3);
            } else {
                assert_eq!(&state.vm.external_regions[0][size..], b"CM01\x03\0\0\0");
            }
            let encoded = encode_component(&state).unwrap();
            let mut restored = decode_component::<RunnerState>(&encoded).unwrap().restore().unwrap();
            assert!(restored.is_pending());
            assert_eq!(restored.layout, layout);
            assert_eq!(restored.state().unwrap(), runner.state().unwrap());
            assert_eq!(restored.update(70, &mut host, 64).unwrap(), MotionProgress::Complete(Step::Halted));
            let mid = restored.state().unwrap();
            assert_eq!((mid.elapsed, mid.running, mid.finishing), (30, 1, 0));
            assert_eq!(mid.properties[28], 0x7fff_ffff);
            assert_eq!(mid.properties[35], 3);
            assert_eq!(restored.snapshot(40, 2).unwrap().record().unwrap(),
                runner_snapshot_record(&mut runner, &mut host));

            assert!(matches!(restored.finish(&mut host, 64).unwrap(), MotionProgress::Complete(_)));
            let forced = restored.state().unwrap();
            assert_eq!((forced.elapsed, forced.running, forced.finishing), (30, 1, 0));
            assert_eq!(forced.properties[28], 0xffff_ffff);
            assert!(matches!(restored.update(70, &mut host, 64).unwrap(), MotionProgress::Complete(_)));
            let end = restored.state().unwrap();
            assert_eq!((end.elapsed, end.running, end.finishing), (60, 0, 0));
            assert_eq!(end.properties[28], 0xffff_ffff);
            assert_eq!(end.properties[35], 3);
        }
    }

    fn runner_snapshot_record(runner: &mut MotionRunner, host: &mut MotionHost) -> Vec<u8> {
        runner.update(40, host, 64).unwrap();
        runner.snapshot(40, 2).unwrap().record().unwrap()
    }
}

//! Complete VM execution state and group-level shared external identities.
//! Snapshotting happens between native callbacks; group owners must suspend
//! task execution while collecting a coherent state.
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::{
    CallFrame, DebugState, ExecutionControl, KcsHeader, KcsProgram, MAX_SCRIPT_BYTES,
    MAX_SCRIPT_ITEMS, Result, ScriptError, SharedExternalStorage, Vm, VmRegionSnapshot,
};

const SNAPSHOT_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VmExecutionSnapshot {
    version: u32,
    #[serde(default)]
    native_context_identity: Option<u32>,
    header: KcsHeader,
    code: Vec<u8>,
    initial_data: Vec<u8>,
    stack_size: usize,
    memory: Vec<u8>,
    regions: Vec<VmRegionSnapshot>,
    instruction_pointer: usize,
    stack_pointer: usize,
    frame_base: usize,
    frame_top: usize,
    frame_floors: Vec<usize>,
    debug_variable_floors: Vec<usize>,
    frames: Vec<CallFrame>,
    switch_value: u32,
    debug: DebugState,
    control: ExecutionControl,
    /// Index into VmSnapshotGroup.external_regions, never a process address.
    external_id: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VmSnapshotGroup {
    version: u32,
    pub programs: BTreeMap<u32, VmExecutionSnapshot>,
    pub external_regions: Vec<Vec<u8>>,
}

fn invalid(message: &'static str) -> ScriptError {
    ScriptError::InvalidStructuredText { offset: 0, message }
}

impl Vm {
    fn capture_execution(&self, external_id: usize) -> Result<VmExecutionSnapshot> {
        if self.native_arguments.is_some() {
            return Err(invalid("cannot snapshot a VM inside an active native callback"));
        }
        let program = self.program.as_ref().ok_or(ScriptError::NoProgram)?;
        Ok(VmExecutionSnapshot {
            version: SNAPSHOT_VERSION,
            native_context_identity: self.native_context_identity,
            header: program.header.clone(),
            code: program.code.clone(),
            initial_data: program.data.clone(),
            stack_size: program.stack_size,
            memory: self.memory.clone(),
            regions: self.region_snapshots(),
            instruction_pointer: self.instruction_pointer,
            stack_pointer: self.sp,
            frame_base: self.frame_base,
            frame_top: self.frame_top,
            frame_floors: self.frame_floors.clone(),
            debug_variable_floors: self.debug_variable_floors.clone(),
            frames: self.frames.clone(),
            switch_value: self.switch_value,
            debug: self.debug.clone(),
            control: self.control.clone(),
            external_id,
        })
    }
}

impl VmExecutionSnapshot {
    pub fn native_context_identity(&self) -> Option<u32> { self.native_context_identity }

    /// Validates decoded instruction boundaries, all storage domains and
    /// execution cursors before returning a runnable replacement VM.
    fn restore(&self, external: SharedExternalStorage) -> Result<Vm> {
        if self.version != SNAPSHOT_VERSION {
            return Err(invalid("unsupported VM snapshot version"));
        }
        if self.code.len() > MAX_SCRIPT_BYTES || self.initial_data.len() > MAX_SCRIPT_BYTES
            || self.frames.len() > MAX_SCRIPT_ITEMS || self.frame_floors.len() > MAX_SCRIPT_ITEMS
            || self.debug_variable_floors.len() > MAX_SCRIPT_ITEMS
            || self.debug.source.len() > MAX_SCRIPT_BYTES || self.debug.scopes.len() > MAX_SCRIPT_ITEMS
            || self.debug.variables.len() > MAX_SCRIPT_ITEMS
        {
            return Err(invalid("VM snapshot metadata exceeds storage limits"));
        }
        let length = self.initial_data.len().checked_add(self.stack_size)
            .filter(|&length| length <= MAX_SCRIPT_BYTES)
            .ok_or_else(|| invalid("VM snapshot data and stack size is invalid"))?;
        if self.memory.len() != length || self.header.data_length as usize != self.initial_data.len()
            || self.header.stack_length as usize != self.stack_size
            || self.header.code_length as usize != self.code.len()
            || self.header.format_version != 2 || self.header.header_length != 52
        {
            return Err(invalid("VM snapshot header does not describe its stored program"));
        }
        let program = KcsProgram::from_code_with_header(
            self.code.clone(), self.initial_data.clone(), self.stack_size, self.header.clone(),
        )?;
        let boundary = |position: usize| {
            position == program.code.len() || program.boundaries.contains(&position)
        };
        if !boundary(self.instruction_pointer)
            || self.control.call_return.is_some_and(|position| !boundary(position))
            || self.stack_pointer > length || self.frame_base > length || self.frame_top > length
            || self.frame_floors.iter().any(|&position| position > length)
            || self.debug_variable_floors.iter().any(|&position| position > self.debug.variables.len())
            || self.frames.iter().any(|frame| {
                frame.base > length || frame.top > length || frame.parameter_bytes > length
                    || !boundary(frame.return_pc)
            })
        {
            return Err(invalid("VM snapshot has invalid execution or frame cursors"));
        }
        if self.frame_floors.len() != self.frames.len().saturating_add(1)
            || self.debug_variable_floors.len() != self.frames.len()
            || self.control.message.is_some() && self.control.resume_started_at_end.is_some()
            || self.control.waiting && self.control.receiver.is_none()
            || self.control.message.is_some() && self.control.receiver.is_none()
        {
            return Err(invalid("VM snapshot continuation state is inconsistent"));
        }
        let mut vm = Vm::new();
        if let Some(identity) = self.native_context_identity {
            vm.bind_native_context_identity(identity)?;
        }
        vm.load_program_with_shared_external(program, external)?;
        vm.memory.clone_from(&self.memory);
        vm.restore_regions(&self.regions)?;
        vm.instruction_pointer = self.instruction_pointer;
        vm.sp = self.stack_pointer;
        vm.frame_base = self.frame_base;
        vm.frame_top = self.frame_top;
        vm.frame_floors.clone_from(&self.frame_floors);
        vm.debug_variable_floors.clone_from(&self.debug_variable_floors);
        vm.frames.clone_from(&self.frames);
        vm.switch_value = self.switch_value;
        vm.debug.clone_from(&self.debug);
        vm.control.clone_from(&self.control);
        if let Some(receiver) = vm.control.receiver {
            vm.read(receiver, 16)?;
        }
        vm.sync_stack_view();
        Ok(vm)
    }
}

impl VmSnapshotGroup {
    pub fn capture<'a>(programs: impl IntoIterator<Item = (u32, &'a Vm)>) -> Result<Self> {
        let mut bindings: Vec<SharedExternalStorage> = Vec::new();
        let mut snapshots = BTreeMap::new();
        let mut contexts = BTreeSet::new();
        for (id, vm) in programs {
            if let Some(identity) = vm.native_context_identity()
                && !contexts.insert(identity)
            { return Err(invalid("different VM entries share a native object identity")); }
            if snapshots.contains_key(&id) || snapshots.len() >= MAX_SCRIPT_ITEMS {
                return Err(invalid("duplicate VM identity or snapshot group size limit"));
            }
            let external = vm.external_binding();
            let external_id = match bindings.iter().position(|binding| binding.same_storage(&external)) {
                Some(index) => index,
                None => {
                    bindings.push(external);
                    bindings.len() - 1
                }
            };
            snapshots.insert(id, vm.capture_execution(external_id)?);
        }
        let external_regions = bindings.iter().map(SharedExternalStorage::snapshot)
            .collect::<Result<Vec<_>>>()?;
        Ok(Self { version: SNAPSHOT_VERSION, programs: snapshots, external_regions })
    }

    /// Reconstructs every VM first. A failure leaves the caller's live group
    /// untouched; shared external bindings are recreated by identity.
    pub fn restore(&self) -> Result<BTreeMap<u32, Vm>> {
        if self.version != SNAPSHOT_VERSION || self.programs.len() > MAX_SCRIPT_ITEMS
            || self.external_regions.len() > self.programs.len()
        {
            return Err(invalid("invalid VM snapshot group version or count"));
        }
        let bindings = self.external_regions.iter()
            .map(|bytes| SharedExternalStorage::new(bytes.clone()))
            .collect::<Result<Vec<_>>>()?;
        let mut restored = BTreeMap::new();
        let mut contexts = BTreeSet::new();
        for (&id, snapshot) in &self.programs {
            if let Some(identity) = snapshot.native_context_identity
                && !contexts.insert(identity)
            { return Err(invalid("saved VM object identities collide")); }
            let binding = bindings.get(snapshot.external_id)
                .ok_or_else(|| invalid("VM snapshot refers to an absent shared external region"))?;
            restored.insert(id, snapshot.restore(binding.clone())?);
        }
        Ok(restored)
    }
}

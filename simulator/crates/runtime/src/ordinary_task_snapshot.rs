//! Execution metadata paired with a VM from the same shared snapshot group.
use super::*;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrdinaryTaskSnapshot {
    pub version: u32,
    pub task_id: i32,
    pub has_vm: bool,
    pub clock: TaskClock,
    pub running: u32,
    pub deletion: u32,
    pub property: u32,
    pub accumulated_increment: u32,
    pub system_time_ms: u32,
    queue: VecDeque<TaskMessage>,
    current_message: Option<TaskMessage>,
    phase: Option<UpdatePhase>,
    command: Option<CommandPhase>,
    creation_context: Vec<u8>,
}

impl OrdinaryTask {
    pub fn execution_snapshot(&self) -> RuntimeResult<OrdinaryTaskSnapshot> {
        let saved = OrdinaryTaskSnapshot {
            version: 1, task_id: self.task_id, has_vm: self.vm.is_some(),
            clock: self.clock.clone(), running: self.running, deletion: self.deletion,
            property: self.property, accumulated_increment: self.accumulated_increment,
            system_time_ms: self.system_time_ms, queue: self.queue.clone(),
            current_message: self.current_message, phase: self.phase, command: self.command,
            creation_context: self.creation_context.clone(),
        };
        saved.validate()?;
        Ok(saved)
    }
}

impl OrdinaryTaskSnapshot {
    pub fn validate(&self) -> RuntimeResult<()> {
        if self.version != 1 || self.queue.len() > 1_000_000
            || self.creation_context.len() > 31 || self.creation_context.contains(&0)
            || self.phase.is_some() && self.command.is_some()
            || self.current_message.is_some() && self.phase != Some(UpdatePhase::Messages)
            || !self.has_vm && (self.phase.is_some() || self.command.is_some())
        {
            return Err(RuntimeError::InvalidSave("invalid ordinary task execution snapshot".into()));
        }
        Ok(())
    }

    pub fn restore(&self, vm: Option<Vm>) -> RuntimeResult<OrdinaryTask> {
        self.validate()?;
        if self.has_vm != vm.is_some() {
            return Err(RuntimeError::InvalidSave("ordinary task VM is absent or has another owner".into()));
        }
        Ok(OrdinaryTask {
            task_id: self.task_id, vm, clock: self.clock.clone(), running: self.running,
            deletion: self.deletion, property: self.property,
            accumulated_increment: self.accumulated_increment, system_time_ms: self.system_time_ms,
            queue: self.queue.clone(), current_message: self.current_message,
            phase: self.phase, command: self.command, creation_context: self.creation_context.clone(),
        })
    }
}

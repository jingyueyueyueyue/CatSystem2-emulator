//! Portable ordinary-task execution. All times and script identities retain
//! the original DWORD representation; no process addresses are dereferenced.
use std::collections::VecDeque;

use cs2_script::{Host, NativeReturn, ResumeStep, ScriptError, Vm};
use serde::{Deserialize, Serialize};

use crate::{RuntimeError, RuntimeResult, TaskCommand};

#[path = "ordinary_task_snapshot.rs"]
pub mod snapshot;

pub type TaskMessage = [u32; 4];

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskClock {
    pub origin: u32,
    pub last_update: u32,
    pub current: u32,
    pub paused: u32,
    pub single_step: u32,
}

impl TaskClock {
    pub fn update(&mut self, now: u32) {
        let delta = now.wrapping_sub(self.last_update);
        if self.paused != 0 {
            self.single_step = 0;
            self.origin = self.origin.wrapping_add(delta);
        } else {
            self.current = now;
            if self.single_step != 0 {
                self.single_step = 0;
                self.origin = self.origin.wrapping_add(delta.wrapping_sub(1));
            }
        }
        self.last_update = now;
    }

    pub fn elapsed(&self) -> u32 {
        self.current.wrapping_sub(self.origin)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum UpdatePhase {
    Messages,
    Resume,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum CommandPhase {
    Message(TaskMessage),
    Delete(TaskMessage),
    Control { command: TaskCommand, message: TaskMessage },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TaskCommandOutcome {
    pub done: bool,
    pub propagate: bool,
}

/// Explicit command-buffer borrows replace native process pointers.
/// data1 is writable for the creation-context query (command 25).
#[derive(Default)]
pub struct TaskCommandBuffers<'a> {
    pub data0: Option<&'a [u8]>,
    pub data1: Option<&'a mut [u8]>,
}

pub struct OrdinaryTask {
    pub task_id: i32,
    pub vm: Option<Vm>,
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
    pub fn new(task_id: i32, vm: Option<Vm>) -> Self {
        let running = u32::from(vm.is_some());
        Self {
            task_id,
            vm,
            clock: TaskClock::default(),
            running,
            deletion: 0,
            property: u32::MAX,
            accumulated_increment: 0,
            system_time_ms: 0,
            queue: VecDeque::new(),
            current_message: None,
            phase: None,
            command: None,
            creation_context: Vec::new(),
        }
    }

    pub fn queued_messages(&self) -> impl Iterator<Item = &TaskMessage> {
        self.queue.iter()
    }

    /// Incoming host effects may append during a suspended message phase.
    /// The cursor is a queue head, so newly appended messages are consumed in
    /// this phase; appends during the final resume wait for the next update.
    pub fn enqueue_message(&mut self, message: TaskMessage) {
        self.queue.push_back(message);
    }

    pub fn has_pending_update(&self) -> bool {
        self.phase.is_some()
    }

    pub fn has_pending_command(&self) -> bool {
        self.command.is_some()
    }

    pub fn set_creation_context(&mut self, bytes: &[u8]) -> RuntimeResult<()> {
        let bytes = bytes.split(|byte| *byte == 0).next().unwrap_or_default();
        if bytes.len() > 31 {
            return Err(RuntimeError::Host("task creation context exceeds 31 bytes".into()));
        }
        self.creation_context = bytes.to_vec();
        Ok(())
    }

    pub fn creation_context(&self) -> &[u8] {
        &self.creation_context
    }

    pub fn copy_creation_context(&self, destination: &mut [u8]) -> RuntimeResult<usize> {
        let needed = self.creation_context.len() + 1;
        if destination.len() < needed {
            return Err(RuntimeError::Host("task context destination is too small".into()));
        }
        destination[..needed - 1].copy_from_slice(&self.creation_context);
        destination[needed - 1] = 0;
        Ok(needed)
    }

    pub fn start_execution(&mut self, source: u32, mode: u32) -> RuntimeResult<()> {
        self.require_idle()?;
        if let Some(vm) = &mut self.vm {
            vm.set_execution_mode(if source == self.task_id as u32 { mode } else { 1 })?;
            self.running = 1;
        }
        Ok(())
    }

    pub fn stop_execution(&mut self) -> RuntimeResult<()> {
        self.require_idle()?;
        if let Some(vm) = &mut self.vm {
            vm.set_execution_mode(0)?;
        }
        self.running = 0;
        Ok(())
    }

    /// Command 161's immediate initialization has ordinary host semantics,
    /// but does not update the clock, consume the queue, or add increment.
    pub fn initialize_execution(&mut self, host: &mut impl Host, budget: usize) -> RuntimeResult<bool> {
        self.require_idle()?;
        let Some(vm) = self.vm.as_mut() else { return Ok(true) };
        let mut host = OrdinaryHost {
            inner: host, elapsed: self.clock.elapsed(), system_time_ms: self.system_time_ms,
        };
        match vm.resume(&mut host, budget)? {
            ResumeStep::Yield => Ok(false),
            step => {
                self.running = activity(step);
                Ok(true)
            }
        }
    }

    fn require_idle(&self) -> RuntimeResult<()> {
        if self.phase.is_some() || self.command.is_some() {
            return Err(RuntimeError::Host("task has a pending execution continuation".into()));
        }
        Ok(())
    }

    pub fn update(
        &mut self,
        now: u32,
        increment: u32,
        host: &mut impl Host,
        budget: usize,
    ) -> RuntimeResult<bool> {
        if self.phase.is_none() {
            if self.command.is_some() {
                return Err(RuntimeError::Host("finish synchronous task command before updating".into()));
            }
            self.clock.update(now);
        }
        self.update_body(increment, host, budget)
    }

    /// Tree scheduling updates the clock before descendants, and calls this
    /// body afterwards. Budget continuation never advances the clock twice.
    pub fn update_body(
        &mut self,
        increment: u32,
        host: &mut impl Host,
        budget: usize,
    ) -> RuntimeResult<bool> {
        self.update_body_with_messages(increment, host, budget, &mut |_| Vec::new())
    }

    /// A reentrant host stores effects for the temporarily detached object.
    /// Drain them after every VM slice, so appends during the message phase
    /// remain in that phase; appends during final resume wait until next tick.
    pub fn update_body_with_messages<H: Host>(
        &mut self,
        increment: u32,
        host: &mut H,
        budget: usize,
        take_messages: &mut impl FnMut(&mut H) -> Vec<TaskMessage>,
    ) -> RuntimeResult<bool> {
        self.queue.extend(take_messages(host));
        if self.command.is_some() {
            return Err(RuntimeError::Host("finish synchronous task command before updating".into()));
        }
        if self.phase.is_none() {
            if self.deletion != 0 || self.running == 0 || self.vm.is_none() {
                return Ok(true);
            }
            if budget == 0 {
                return Ok(false);
            }
            self.accumulated_increment = self.accumulated_increment.wrapping_add(increment);
            self.phase = Some(UpdatePhase::Messages);
        }
        let mut calls_left = budget;
        while calls_left != 0 {
            calls_left -= 1;
            let vm = self.vm.as_mut().ok_or(RuntimeError::NotInitialized)?;
            let mut host = OrdinaryHost {
                inner: &mut *host,
                elapsed: self.clock.elapsed(),
                system_time_ms: self.system_time_ms,
            };
            match self.phase.expect("active update has a phase") {
                UpdatePhase::Messages => {
                    if self.current_message.is_none() {
                        self.current_message = self.queue.pop_front();
                    }
                    let Some(message) = &mut self.current_message else {
                        self.phase = Some(UpdatePhase::Resume);
                        continue;
                    };
                    let result = vm.dispatch_message(message, &mut host, budget)?;
                    self.queue.extend(take_messages(host.inner));
                    match result {
                        Some(ResumeStep::Yield) => return Ok(false),
                        Some(result) => {
                            self.running = activity(result);
                            self.current_message = None;
                        }
                        None => self.current_message = None,
                    }
                }
                UpdatePhase::Resume => {
                    let result = vm.resume(&mut host, budget)?;
                    self.queue.extend(take_messages(host.inner));
                    match result {
                    ResumeStep::Yield => return Ok(false),
                    result => {
                        self.running = activity(result);
                        self.phase = None;
                        return Ok(true);
                    }
                    }
                }
            }
        }
        Ok(false)
    }

    /// Synchronous messages bypass running/mode and leave task clock/running
    /// unchanged. A false result retains the packet and all VM side effects.
    pub fn dispatch_message(
        &mut self,
        message: &mut TaskMessage,
        output: &mut u32,
        host: &mut impl Host,
        budget: usize,
    ) -> RuntimeResult<bool> {
        // The original synchronous call has no instruction-budget boundary.
        // Deletion blocks a new call, but cannot abandon a message whose VM
        // stack is suspended solely by the portable scheduler's budget.
        if self.deletion != 0 && self.command.is_none() {
            return Ok(true);
        }
        if self.phase.is_some() {
            return Err(RuntimeError::Host("task update is suspended; synchronous entry would reenter its VM".into()));
        }
        match self.command {
            Some(CommandPhase::Message(packet)) if packet == *message => {}
            Some(_) => return Err(RuntimeError::Host("task command continuation has a different packet".into())),
            None => self.command = Some(CommandPhase::Message(*message)),
        }
        let Some(vm) = &mut self.vm else {
            self.command = None;
            return Ok(true);
        };
        let mut host = OrdinaryHost {
            inner: &mut *host,
            elapsed: self.clock.elapsed(),
            system_time_ms: self.system_time_ms,
        };
        match vm.dispatch_message(message, &mut host, budget)? {
            Some(ResumeStep::Yield) => Ok(false),
            Some(result) => {
                *output = activity(result);
                self.command = None;
                Ok(true)
            }
            None => {
                *output = 0;
                self.command = None;
                Ok(true)
            }
        }
    }

    pub fn request_delete(&mut self, host: &mut impl Host, budget: usize) -> RuntimeResult<bool> {
        if self.deletion != 0 {
            return Ok(true);
        }
        if self.phase.is_some() {
            return Err(RuntimeError::Host("finish ordinary update before synchronous delete notification".into()));
        }
        let mut message = match self.command {
            Some(CommandPhase::Delete(packet)) => packet,
            Some(_) => return Err(RuntimeError::Host("finish task command before delete notification".into())),
            None => [u32::MAX, 0xffff_0006, 0, 0],
        };
        self.command = Some(CommandPhase::Delete(message));
        if let Some(vm) = &mut self.vm {
            let mut host = OrdinaryHost {
                inner: &mut *host,
                elapsed: self.clock.elapsed(),
                system_time_ms: self.system_time_ms,
            };
            if vm.dispatch_message(&mut message, &mut host, budget)? == Some(ResumeStep::Yield) {
                return Ok(false);
            }
        }
        self.command = None;
        self.deletion = 1;
        Ok(true)
    }

    pub fn dispatch_control(
        &mut self,
        object_type: u32,
        command: TaskCommand,
        output: &mut u32,
        host: &mut impl Host,
        budget: usize,
    ) -> RuntimeResult<TaskCommandOutcome> {
        self.dispatch_control_with_buffers(
            object_type, command, output, &mut TaskCommandBuffers::default(), host, budget,
        )
    }

    pub fn dispatch_control_with_buffers(
        &mut self,
        object_type: u32,
        command: TaskCommand,
        output: &mut u32,
        buffers: &mut TaskCommandBuffers<'_>,
        host: &mut impl Host,
        budget: usize,
    ) -> RuntimeResult<TaskCommandOutcome> {
        let consumed = TaskCommandOutcome { done: true, propagate: false };
        if self.deletion != 0 {
            return Ok(consumed);
        }
        if self.phase.is_some() {
            return Err(RuntimeError::Host("finish task update before synchronous control".into()));
        }
        let pending = match self.command {
            Some(CommandPhase::Control { command: original, message }) if original == command => Some(message),
            Some(_) => return Err(RuntimeError::Host("task control continuation has a different command".into())),
            None => None,
        };
        match command.code {
            162 => {
                self.start_execution(command.data[0], command.data[1])?;
                return Ok(consumed);
            }
            163 => {
                self.stop_execution()?;
                return Ok(consumed);
            }
            25 | 26 => {
                if command.data[1] != 0 {
                    let bytes = buffers.data1.as_deref_mut()
                        .ok_or_else(|| RuntimeError::Host("task context command requires data1 buffer".into()))?;
                    if command.code == 25 {
                        self.copy_creation_context(bytes)?;
                    } else {
                        let end = bytes.iter().position(|&byte| byte == 0)
                            .ok_or_else(|| RuntimeError::Host("task context is not terminated".into()))?;
                        self.set_creation_context(&bytes[..end])?;
                    }
                }
                return Ok(consumed);
            }
            20 | 21 => {
                self.enqueue_message([
                    command.sender as u32,
                    if command.code == 20 { 0xffff_0012 } else { 0xffff_0013 },
                    0, 0,
                ]);
                *output = 1;
                return Ok(consumed);
            }
            3 | 4 | 6 | 9 | 10 | 18 | 22 | 30 | 31 | 64 | 65 | 452 => {}
            // These belong to the ordinary object, so they must not silently
            // fall through as an unknown, propagating base-class command.
            161 | 164..=177 => return Err(RuntimeError::Host(format!(
                "ordinary task command {} requires an unimplemented object service", command.code,
            ))),
            _ => return Ok(TaskCommandOutcome {
                done: true,
                propagate: dispatch_base_control(
                    object_type, &mut self.deletion, &mut self.property, &mut self.clock,
                    command, output,
                ),
            }),
        }

        let Some(vm) = self.vm.as_mut() else {
            if command.code == 3 {
                self.deletion = 1;
            }
            return Ok(consumed);
        };
        let mut message = match pending {
            Some(message) => message,
            None => {
                let mut packet = [command.sender as u32, 0, command.data[0], command.data[1]];
                packet[1] = match command.code {
                    3 => { packet = [u32::MAX, 0, 0, 0]; 0xffff_0006 }
                    4 => 0xffff_0001,
                    6 => {
                        packet[2] = packed_point(buffers.data0)?;
                        packet[3] = packed_point(buffers.data1.as_deref())?;
                        0xffff_0002
                    }
                    9 => 0xffff_0003,
                    10 => 0xffff_0004,
                    18 => { packet = [u32::MAX, 0, 0, 0]; 0xffff_0016 }
                    22 => { packet[2] = 0; 0xffff_0014 }
                    30 => 0xffff_0018,
                    31 => 0xffff_0019,
                    64 => 0xffff_0005,
                    65 => 0xffff_0017,
                    452 => { packet[2] = 0; 0xffff_0015 }
                    _ => unreachable!("only known notification commands reach this stage"),
                };
                if matches!(command.code, 9 | 10 | 30 | 31 | 65) {
                    *output = 1;
                }
                packet
            }
        };
        self.command = Some(CommandPhase::Control { command, message });
        let mut ordinary_host = OrdinaryHost {
            inner: host,
            elapsed: self.clock.elapsed(),
            system_time_ms: self.system_time_ms,
        };
        let result = vm.dispatch_message(&mut message, &mut ordinary_host, budget)?;
        if result == Some(ResumeStep::Yield) {
            return Ok(TaskCommandOutcome { done: false, propagate: false });
        }
        self.command = None;
        match command.code {
            3 => self.deletion = 1,
            9 | 10 => *output = 0,
            22 => *output = result.map(activity).unwrap_or(0),
            _ => {}
        }
        Ok(consumed)
    }
}

fn packed_point(bytes: Option<&[u8]>) -> RuntimeResult<u32> {
    let bytes = bytes.and_then(|bytes| bytes.get(..8))
        .ok_or_else(|| RuntimeError::Host("pointer notification requires two 8-byte point buffers".into()))?;
    let x = u32::from_le_bytes(bytes[..4].try_into().unwrap());
    let y = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
    Ok((x & 0xffff) | ((y & 0xffff) << 16))
}

pub(crate) fn dispatch_base_control(
    object_type: u32,
    deletion: &mut u32,
    property: &mut u32,
    clock: &mut TaskClock,
    command: TaskCommand,
    output: &mut u32,
) -> bool {
    match command.code {
        3 => *deletion = 1,
        13 => *output = object_type,
        17 => {}
        23 => *output = *property,
        24 => *property = command.data[1],
        29 => clock.paused = command.data[1],
        _ => return true,
    }
    false
}

fn activity(step: ResumeStep) -> u32 {
    match step {
        ResumeStep::Completed { active, .. } => u32::from(active),
        ResumeStep::Paused | ResumeStep::Yield => 1,
    }
}

struct OrdinaryHost<'a, H> {
    inner: &'a mut H,
    elapsed: u32,
    system_time_ms: u32,
}

impl<H: Host> Host for OrdinaryHost<'_, H> {
    fn should_yield(&self) -> bool { self.inner.should_yield() }

    fn renew_instruction_budget(&self, budget: usize) -> bool {
        self.inner.renew_instruction_budget(budget)
    }

    fn call(
        &mut self,
        index: u32,
        arguments: &[u8],
        vm: &mut Vm,
    ) -> cs2_script::Result<NativeReturn> {
        match index {
            4 => Ok(NativeReturn::Value(self.system_time_ms)),
            5 => Ok(NativeReturn::Value(self.elapsed)),
            65 => Ok(NativeReturn::YieldVoid),
            71 => {
                let raw = arguments.get(..4).ok_or(ScriptError::CallParameterUnderflow)?;
                vm.wait_for_message(u32::from_le_bytes(raw.try_into().unwrap()))?;
                Ok(NativeReturn::YieldVoid)
            }
            _ => self.inner.call(index, arguments, vm),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cs2_script::{KcsProgram, NullHost};

    #[test]
    fn paused_clock_and_zero_delta_single_step_keep_dword_semantics() {
        let mut clock = TaskClock {
            origin: 10, last_update: 100, current: 100, paused: 7, single_step: 1,
        };
        clock.update(110);
        assert_eq!((clock.origin, clock.current, clock.single_step, clock.elapsed()), (20, 100, 0, 80));
        clock.paused = 0;
        clock.single_step = 1;
        clock.update(110);
        assert_eq!((clock.origin, clock.current, clock.single_step, clock.elapsed()), (19, 110, 0, 91));
    }

    #[test]
    fn ordinary_queue_and_final_resume_finish_across_small_budgets_without_retiming() {
        let code = [
            0x25, 3, 0, 0, 0, 0, // push receiver address
            0x2b, 3, 71, 0, 0, 0, 3, 4, 0, 0, 0, // native 71, four-byte argument
            0x3b,
        ];
        let vm = Vm::from_program(KcsProgram::from_code(&code, vec![0; 16], 16).unwrap()).unwrap();
        let mut task = OrdinaryTask::new(5, Some(vm));
        assert!(task.update(10, 3, &mut NullHost, 16).unwrap());
        assert!(task.vm.as_ref().unwrap().waiting_for_message());
        task.enqueue_message([7, 8, 9, 10]);
        assert!(!task.update(20, 4, &mut NullHost, 1).unwrap());
        assert!(!task.update(999, 100, &mut NullHost, 1).unwrap());
        assert!(task.update(1000, 100, &mut NullHost, 1).unwrap());
        assert_eq!(task.clock.current, 20);
        assert_eq!(task.accumulated_increment, 7);
        assert_eq!(task.running, 0);
        assert_eq!(task.vm.as_ref().unwrap().read_u32(0).unwrap(), 7);
        assert_eq!(task.queued_messages().count(), 0);
    }

    #[test]
    fn ordinary_task_renews_instruction_budget_and_resumes_native_retry_without_retiming() {
        struct DeadlineHost {
            deadline_reached: bool,
            calls: Vec<u32>,
        }

        impl Host for DeadlineHost {
            fn should_yield(&self) -> bool {
                self.deadline_reached
            }

            fn renew_instruction_budget(&self, budget: usize) -> bool {
                budget == 2 && !self.deadline_reached
            }

            fn call(&mut self, index: u32, _: &[u8], _: &mut Vm) -> cs2_script::Result<NativeReturn> {
                self.calls.push(index);
                if self.calls == [10, 11] {
                    return Ok(NativeReturn::Retry);
                }
                Ok(NativeReturn::Continue)
            }
        }

        let code = [
            0x00,
            0x2b, 3, 10, 0, 0, 0, 3, 0, 0, 0, 0,
            0x2b, 3, 11, 0, 0, 0, 3, 0, 0, 0, 0,
            0x2b, 3, 12, 0, 0, 0, 3, 0, 0, 0, 0,
            0x3b,
        ];
        let vm = Vm::from_program(KcsProgram::from_code(&code, vec![0; 16], 16).unwrap()).unwrap();
        let mut task = OrdinaryTask::new(5, Some(vm));
        let mut host = DeadlineHost { deadline_reached: false, calls: Vec::new() };

        assert!(!task.update(10, 3, &mut host, 2).unwrap());
        assert_eq!(host.calls, [10, 11]);

        host.deadline_reached = true;
        assert!(!task.update(20, 5, &mut host, 2).unwrap());
        assert_eq!(host.calls, [10, 11]);

        host.deadline_reached = false;
        assert!(task.update(30, 7, &mut host, 2).unwrap());
        assert_eq!(host.calls, [10, 11, 11, 12]);
        assert_eq!((task.clock.current, task.accumulated_increment, task.running), (10, 3, 0));
    }

    #[test]
    fn synchronous_control_retains_packet_and_output_phase_across_budget_yields() {
        let code = [
            0x25, 3, 0, 0, 0, 0,
            0x2b, 3, 71, 0, 0, 0, 3, 4, 0, 0, 0,
            0x00, 0x00, 0x3b,
        ];
        let vm = Vm::from_program(KcsProgram::from_code(&code, vec![0; 16], 16).unwrap()).unwrap();
        let mut task = OrdinaryTask::new(5, Some(vm));
        assert!(task.update(10, 3, &mut NullHost, 16).unwrap());
        task.running = 0;
        let command = TaskCommand { target: 5, sender: -2, code: 9, data: [7, 8, 1] };
        let mut output = 99;
        assert!(!task.dispatch_control(8, command, &mut output, &mut NullHost, 1).unwrap().done);
        assert_eq!(output, 1);
        let mut other = command;
        other.data[0] = 9;
        assert!(task.dispatch_control(8, other, &mut output, &mut NullHost, 16).is_err());
        let result = task.dispatch_control(8, command, &mut output, &mut NullHost, 16).unwrap();
        assert_eq!(result, TaskCommandOutcome { done: true, propagate: false });
        assert_eq!((output, task.running, task.clock.current, task.accumulated_increment), (0, 0, 10, 3));
        let vm = task.vm.as_ref().unwrap();
        assert_eq!((0..4).map(|index| vm.read_u32(index * 4).unwrap()).collect::<Vec<_>>(),
            vec![(-2i32) as u32, 0xffff_0003, 7, 8]);
    }

    #[test]
    fn host_queue_effects_during_message_phase_are_consumed_before_final_resume() {
        struct Effects {
            pending: Vec<TaskMessage>,
            delivered: bool,
        }
        impl Host for Effects {
            fn call(&mut self, index: u32, _: &[u8], _: &mut Vm) -> cs2_script::Result<NativeReturn> {
                assert_eq!(index, 999);
                if !self.delivered {
                    self.pending.push([2, 3, 4, 5]);
                    self.delivered = true;
                }
                Ok(NativeReturn::Continue)
            }
        }
        let code = [
            0x25, 3, 0, 0, 0, 0,
            0x2b, 3, 71, 0, 0, 0, 3, 4, 0, 0, 0,
            0x2b, 3, 0xe7, 3, 0, 0, 3, 0, 0, 0, 0,
            0x25, 3, 0, 0, 0, 0,
            0x2b, 3, 71, 0, 0, 0, 3, 4, 0, 0, 0,
            0x3b,
        ];
        let mut task = OrdinaryTask::new(1, Some(
            Vm::from_program(KcsProgram::from_code(&code, vec![0; 16], 32).unwrap()).unwrap()));
        let mut host = Effects { pending: Vec::new(), delivered: false };
        assert!(task.update(10, 1, &mut host, 64).unwrap());
        task.enqueue_message([1, 2, 3, 4]);
        assert!(task.update_body_with_messages(1, &mut host, 64,
            &mut |host| std::mem::take(&mut host.pending)).unwrap());
        assert_eq!(task.vm.as_ref().unwrap().read_u32(0).unwrap(), 2);
        assert_eq!(task.queued_messages().count(), 0);
    }
}

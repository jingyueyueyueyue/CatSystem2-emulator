//! Lower SLIDER/SCROLL2 command continuations over the real FES owner.
//! Mutation, pixel refresh and synchronous callback have separate cursors.
use cs2_runtime::{TaskCommand, TaskCommandBuffers, TaskCommandOutcome};
use cs2_script::{Result, Vm};
use cs2_renderer::Point;

use super::{LoggingHost, OwnedTaskBuffers};
use crate::fes_controls::{
    ControlNotification, LowerControlOutcome, LowerInputContext, LowerInputOutcome,
};
use crate::native_state_error;

#[path = "task_controls_state.rs"]
pub(crate) mod checkpoint;

pub(super) struct LowerPending {
    command: TaskCommand,
    input: Option<Vec<u8>>,
    output: u32,
    owner: u32,
    notification: Option<ControlNotification>,
    commands: Vec<TaskCommand>,
    command_cursor: usize,
    refresh: bool,
    stage: u8,
}

impl LoggingHost<'_> {
    pub(crate) fn receive_lower_task_command(
        &mut self, command: TaskCommand, buffers: &mut TaskCommandBuffers<'_>,
        output: &mut u32, budget: usize,
    ) -> Result<Option<TaskCommandOutcome>> {
        let id = command.target as u32;
        if matches!(command.code, 6 | 9 | 10 | 64 | 65) {
            let pointer = if command.code == 6 {
                let first = buffers.data0.as_deref().and_then(|bytes| bytes.get(..8))
                    .ok_or_else(|| native_state_error("lower movement requires its previous logical point"))?;
                let second = buffers.data1.as_deref().and_then(|bytes| bytes.get(..8))
                    .ok_or_else(|| native_state_error("lower movement requires its current logical point"))?;
                // Decode both explicit records; the lower receiver consumes
                // the current point and retains its own captured drag origin.
                let _previous = point(first);
                Some(point(second))
            } else {
                Some(Point::new(self.pointer_position[0], self.pointer_position[1]))
            };
            let context = LowerInputContext {
                button_mask: None,
                left_button: None,
                pointer,
                input_task: self.input_task.owner_id,
            };
            return self.route_lower_control_input(command, &context, output, budget);
        }
        if !matches!(command.code, 365..=382) { return Ok(None); }
        if budget == 0 {
            return Ok(Some(TaskCommandOutcome { done: false, propagate: false }));
        }
        let input = buffers.data1.as_deref();
        let mut pending = match self.data_services.tasks.lower_commands.remove(&id) {
            Some(pending) => {
                if pending.command != command || pending.input.as_deref() != input {
                    self.data_services.tasks.lower_commands.insert(id, pending);
                    return Err(native_state_error("lower control continuation changed its command/buffer"));
                }
                pending
            }
            None => {
                let outcome = self.data_services.fes_controls.lower_command(command, input, output)
                    .map_err(|error| self.task_error(id, cs2_runtime::RuntimeError::Host(error.to_string())))?;
                let (owner, notification) = match outcome {
                    LowerControlOutcome::Unhandled => return Ok(None),
                    LowerControlOutcome::Complete => return Ok(Some(TaskCommandOutcome {
                        done: true, propagate: false,
                    })),
                    LowerControlOutcome::Refresh { owner } => (owner, None),
                    LowerControlOutcome::Notify(notification) => (notification.owner, Some(notification)),
                };
                LowerPending {
                    command, input: input.map(ToOwned::to_owned), output: *output,
                    owner, notification, commands: Vec::new(), command_cursor: 0,
                    refresh: true, stage: 0,
                }
            }
        };
        self.advance_lower_pending(id, &mut pending, output, budget)
    }

    /// The input producer supplies actual button state/key classification and
    /// decoded logical coordinates. No mouse-button mapping is guessed here.
    pub(crate) fn route_lower_control_input(
        &mut self, command: TaskCommand, context: &LowerInputContext,
        output: &mut u32, budget: usize,
    ) -> Result<Option<TaskCommandOutcome>> {
        let id = command.target as u32;
        if !self.task_is_reachable(id) || !self.data_services.fes_controls.owns_lower(id) {
            return Ok(None);
        }
        if budget == 0 {
            return Ok(Some(TaskCommandOutcome { done: false, propagate: false }));
        }
        let mut pending = match self.data_services.tasks.lower_commands.remove(&id) {
            Some(pending) => {
                if pending.command != command || pending.input.is_some() {
                    self.data_services.tasks.lower_commands.insert(id, pending);
                    return Err(native_state_error("lower input continuation changed its command"));
                }
                pending
            }
            None => {
                let mut supplied = LowerInputContext {
                    button_mask: context.button_mask, left_button: context.left_button,
                    pointer: context.pointer, input_task: context.input_task,
                };
                let mut outcome = self.data_services.fes_controls.lower_input(command, &supplied)
                    .map_err(|error| self.task_error(id, cs2_runtime::RuntimeError::Host(error.to_string())))?;
                if let LowerInputOutcome::QueryButtonMask(query) = outcome {
                    let mut value = 0;
                    let result = self.dispatch_registered_task_buffers(
                        query, &mut OwnedTaskBuffers::default(), &mut value, budget,
                    )?;
                    if !result.done {
                        return Ok(Some(TaskCommandOutcome { done: false, propagate: false }));
                    }
                    supplied.button_mask = Some(value);
                    outcome = self.data_services.fes_controls.lower_input(command, &supplied)
                        .map_err(|error| self.task_error(id, cs2_runtime::RuntimeError::Host(error.to_string())))?;
                }
                let effects = match outcome {
                    LowerInputOutcome::Unhandled => return Ok(None),
                    LowerInputOutcome::Applied(effects) => effects,
                    LowerInputOutcome::QueryButtonMask(_) => return Err(native_state_error(
                        "lower input receiver repeated a completed BUTTON state query",
                    )),
                };
                LowerPending {
                    command, input: None, output: *output,
                    owner: effects.owner, notification: effects.notification,
                    commands: effects.commands, command_cursor: 0,
                    refresh: effects.refresh, stage: 0,
                }
            }
        };
        self.advance_lower_pending(id, &mut pending, output, budget)
    }

    fn advance_lower_pending(
        &mut self, id: u32, pending: &mut LowerPending, output: &mut u32, budget: usize,
    ) -> Result<Option<TaskCommandOutcome>> {
        let result = (|| {
            for _ in 0..budget {
                match pending.stage {
                    0 => {
                        if let Some(command) = pending.commands.get(pending.command_cursor).copied() {
                            let mut action_output = 0;
                            let result = self.dispatch_registered_task_buffers(
                                command, &mut OwnedTaskBuffers::default(), &mut action_output, budget,
                            )?;
                            if !result.done { return Ok(false); }
                            pending.command_cursor += 1;
                        } else {
                            pending.stage = 1;
                        }
                    }
                    1 => {
                        if pending.refresh && !self.refresh_fes_lower_control(pending.owner, id)? {
                            return Ok(false);
                        }
                        pending.stage = 2;
                    }
                    2 => {
                        if let Some(notification) = &pending.notification {
                            if !self.notify_fes_lower_control(notification, budget)? { return Ok(false); }
                        }
                        pending.stage = 3;
                    }
                    3 => {
                        if let Some(notification) = &pending.notification {
                            self.data_services.fes_controls.finish_lower_notification(
                                notification.command.sender as u32, false,
                            ).map_err(|_| native_state_error("lower control notification completion failed"))?;
                        }
                        return Ok(true);
                    }
                    _ => unreachable!("lower command cursor is bounded"),
                }
            }
            Ok(false)
        })();
        *output = pending.output;
        if !matches!(result, Ok(true)) {
            // Move without requiring Clone on F's notification/effect types.
            let retained = LowerPending {
                command: pending.command, input: pending.input.take(), output: pending.output,
                owner: pending.owner, notification: pending.notification.take(),
                commands: std::mem::take(&mut pending.commands), command_cursor: pending.command_cursor,
                refresh: pending.refresh, stage: pending.stage,
            };
            self.data_services.tasks.lower_commands.insert(id, retained);
        }
        result.map(|done| Some(TaskCommandOutcome { done, propagate: false }))
    }

    pub(crate) fn dispatch_lower_control_message(
        &mut self, target: u32, packet: [u32; 4], queued: bool,
        output: &mut u32, vm: Option<&mut Vm>,
    ) -> Result<Option<TaskCommandOutcome>> {
        let code = packet[1];
        if !matches!(code, 6 | 9 | 10 | 64 | 65 | 365..=382) { return Ok(None); }
        let command = TaskCommand {
            target: target as i32, sender: packet[0] as i32,
            code, data: [packet[2], packet[3], 0],
        };
        if !queued {
            if let Some(mut buffers) = self.pending_owned_task_buffers(command) {
                return self.route_task_command_buffers(command, &mut buffers, output, 4096);
            }
        }
        let first = if code == 6 {
            Some(vm.as_deref().ok_or_else(|| native_state_error(
                "raw lower movement requires its caller VM",
            ))?.read(packet[2], 8)?)
        } else { None };
        let size = match code {
            6 => Some(8),
            365 | 374 => Some(0x124),
            368 => Some(8),
            376 => Some(12),
            _ => None,
        };
        let input = if let Some(size) = size {
            Some(vm.ok_or_else(|| native_state_error(
                "lower control raw buffer needs its caller VM; use owned buffers from FES",
            ))?.read(packet[3], size)?)
        } else { None };
        let mut buffers = OwnedTaskBuffers::new(first, input)?;
        if queued {
            let found = self.queue_task_command_buffers(command, &buffers, output)?;
            Ok(found.then_some(TaskCommandOutcome { done: true, propagate: false }))
        } else {
            self.route_task_command_buffers(command, &mut buffers, output, 4096)
        }
    }
}

fn point(bytes: &[u8]) -> Point {
    Point::new(
        i32::from_le_bytes(bytes[..4].try_into().unwrap()),
        i32::from_le_bytes(bytes[4..8].try_into().unwrap()),
    )
}

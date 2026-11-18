//! Root clock control. The root samples this DWORD after updating its tree;
//! the next platform iteration keeps dispatching while holding game time.
use cs2_runtime::{TaskCommand, TaskCommandOutcome};
use cs2_script::{NativeReturn, Result};
use crate::{LoggingHost, native_state_error, read_u32};

#[path = "root_clock_profile.rs"]
mod profile;
use profile::command_for;

#[derive(Default)]
pub(super) struct TopClockState {
    pub(super) requested: u32,
    pub(super) sampled: u32,
}

impl LoggingHost<'_> {
    pub(super) fn top_clock_native(&mut self, args: &[u8]) -> Result<NativeReturn> {
        let value = read_u32(args, 0)?;
        let code = command_for(self.data_services.native_profile.map_or("", |p| p.pe_sha256))
            .ok_or_else(|| native_state_error("root clock hold needs its exact PE receiver descriptor"))?;
        self.receive_top_clock_command(TaskCommand {
            target: 0, sender: self.current_task_id as i32, code, data: [0, value, 0],
        })?.ok_or_else(|| native_state_error("root clock hold receiver is absent"))?;
        Ok(NativeReturn::Continue)
    }

    pub(crate) fn receive_top_clock_command(
        &mut self, command: TaskCommand,
    ) -> Result<Option<TaskCommandOutcome>> {
        let code = command_for(self.data_services.native_profile.map_or("", |p| p.pe_sha256));
        if command.target != 0 || code != Some(command.code) { return Ok(None); }
        self.data_services.top_clock.requested = command.data[1];
        Ok(Some(TaskCommandOutcome { done: true, propagate: false }))
    }
}

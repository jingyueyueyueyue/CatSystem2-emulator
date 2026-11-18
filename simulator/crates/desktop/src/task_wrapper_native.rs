//! Additional wrappers for the already implemented common task receiver.
use cs2_runtime::{TaskCommand, TaskCommandBuffers};
use cs2_script::{NativeReturn, Result};
use super::{LoggingHost, read_u32};

pub(super) fn root_task_number_lookup(host: &LoggingHost<'_>, number: u32) -> Result<bool> {
    Ok(host.data_services.tasks.root_number_index_contains(number))
}

impl LoggingHost<'_> {
    pub(super) fn reset_task_property_native(&mut self, arguments: &[u8]) -> Result<NativeReturn> {
        let target = read_u32(arguments, 0)?;
        self.receive_task_command(TaskCommand {
            target: target as i32, sender: self.current_task_id as i32,
            code: 24, data: [0, u32::MAX, 0],
        }, &mut TaskCommandBuffers::default(), &mut 0, 4096)?;
        Ok(NativeReturn::Continue)
    }
}

//! The real confirmation/cancel path's ordered BUTTON control requests.
//! This is a descendant of fes_driver, sharing its handler and notice queues.
use super::super::*;
use crate::{LoggingHost, fes_tasks::FesTaskView};
use anyhow::Context;
use cs2_runtime::TaskCommand;

#[derive(Clone, Debug, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(crate) enum Request {
    BaseButtonPriority(i32),
    ResetActive,
    /// The preceding real UNFOCUS handler has completed. Retain this stage
    /// in the existing deferred queue, including across a budget continuation.
    ResetSelection,
}

impl FesScriptDriver {
    pub(super) fn control_flow_command(&mut self, command: &FesDriverCommand)
        -> Result<Option<Option<FesDriverEffect>>> {
        let verb = command.parameters.first().map(Vec::as_slice).unwrap_or_default();
        let request = match verb {
            b"basebtnpri" => Request::BaseButtonPriority(command.parameters.get(1)
                .map(|source| self.number(source)).transpose()?.unwrap_or(-1)),
            b"resetactive" => Request::ResetActive,
            _ => return Ok(None),
        };
        let effect = FesDriverEffect::ControlFlow(request);
        if self.handler.is_some() {
            self.defer(effect)?;
            // PE 2db320 returns -3; 2dde90 returns zero. The latter must
            // finish its calling handler before consuming the queued reset.
            if verb == b"basebtnpri" {
                return self.finish_handler(false).map(|effect| Some(Some(effect)));
            }
            return Ok(Some(None));
        }
        Ok(Some(Some(effect)))
    }

    fn reset_unfocus(&mut self, target: FesDriverTarget) -> Result<()> {
        if self.handler.is_some() || self.deferred.len() >= MAX_NOTIFICATIONS
            || self.notices.len() >= MAX_NOTIFICATIONS {
            bail!("FES resetactive cannot enter its ordered UNFOCUS notification");
        }
        let effect = self.begin_input(FesDriverInput {
            code: 0, target: Some(target), event: Some(b"UNFOCUS".to_vec()),
        })?;
        // No more fallible work after entering the original handler. Its
        // saved control/wait/current and variables remain the driver's own.
        self.deferred.push_front(FesDriverEffect::ControlFlow(Request::ResetSelection));
        if let Some(effect) = effect { self.notices.push_back(effect); }
        Ok(())
    }
}

fn input_owner(host: &LoggingHost<'_>) -> Result<Option<u32>> {
    let input = host.input_task.owner_id;
    if !host.input_task.alive || input.is_some_and(|id| {
        host.data_services.tasks.message_task_state(id)
            .is_none_or(|(kind, deleted, _)| kind != 11 || deleted != 0)
    }) {
        bail!("FES control request requires its live device input receiver");
    }
    Ok(input)
}

fn send_input(host: &mut LoggingHost<'_>, owner: u32, code: u32, data: [u32; 3]) -> Result<()> {
    let input = input_owner(host)?;
    let command = TaskCommand { target: input.unwrap_or(0) as i32, sender: owner as i32, code, data };
    let mut buffers = Default::default();
    let mut output = 0;
    if input.is_none() {
        host.receive_input_region_command(command, &mut buffers, &mut output)?;
        return Ok(());
    }
    let outcome = host.receive_device_input_buffers(command, &mut buffers, &mut output)?
        .context("FES control request lost its actual input receiver")?;
    if !outcome.done { bail!("FES synchronous control request did not complete"); }
    Ok(())
}

pub(crate) fn apply_effect(host: &mut LoggingHost<'_>, view: &FesTaskView,
    driver: &mut FesScriptDriver, effect: &FesDriverEffect) -> Result<Option<bool>> {
    let FesDriverEffect::ControlFlow(request) = effect else { return Ok(None); };
    match request {
        Request::BaseButtonPriority(value) => {
            let button = host.data_services.tasks.fes.platform.auxiliary_button(view.id)
                .context("FES basebtnpri has no original low button")?;
            if host.scene_nodes.get(&button).is_none_or(|node| node.kind != 6 || node.parent != view.id)
                || !host.input_task.regions.iter().any(|region| region.id() == button) {
                bail!("FES basebtnpri low button is absent or unregistered");
            }
            let priority = if *value != -1 { *value } else {
                // PE 17cb66 starts at one, including a direct type-1 parent.
                let mut priority = 1i32;
                let mut parent = view.parent;
                while parent != 0 {
                    let node = host.scene_nodes.get(&parent)
                        .context("FES basebtnpri ancestor was released")?;
                    if node.kind == 1 { break; }
                    if priority >= 256 { bail!("FES basebtnpri ancestor chain exceeds 256"); }
                    priority += 1;
                    parent = *host.task_parents.get(&parent)
                        .context("FES basebtnpri ancestor has no parent")?;
                }
                priority
            };
            // Reorders the existing actual input-region table. This priority
            // belongs to the auxiliary BUTTON, not the drawn IMAGE layers.
            send_input(host, view.id, 129, [button, priority as u32, 1])?;
        }
        Request::ResetActive => {
            input_owner(host)?;
            // PE 17ffc0 samples these before UNFOCUS, and skips the write
            // while the physical device sample is active (input command153).
            if host.data_services.messages.device_sample_active == 0 {
                let point = host.query_device_pointer()?;
                driver.set_named_value("_MOUSE_X_", 0, point[0]);
                driver.set_named_value("_MOUSE_Y_", 0, point[1]);
            }
            let target = host.data_services.messages.fes_button_controllers.get(&view.id)
                .and_then(|state| state.focus.clone()).or_else(|| driver.focus().cloned());
            if let Some(target) = target {
                driver.reset_unfocus(target)?;
                if let Some(state) = host.data_services.messages.fes_button_controllers.get_mut(&view.id) {
                    state.focus = None;
                    if state.focus_lock == 0 { state.pressed = None; }
                }
            } else {
                send_input(host, view.id, 146, [0, 1, 1])?;
            }
        }
        Request::ResetSelection => send_input(host, view.id, 146, [0, 1, 1])?,
    }
    Ok(Some(true))
}

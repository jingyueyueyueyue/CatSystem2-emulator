//! Narrow host bridge. Always put back the original store, including on a
//! failed source/glyph decode, so a retry retains its prepared command stage.
use super::*;
use crate::{FesRuntime, fes_driver::FesDriverEffect, fes_tasks::FesTaskView};

fn with_store<'a, T>(host: &mut LoggingHost<'a>, f: impl FnOnce(&mut FesSavePanelStore,
    &mut LoggingHost<'a>) -> Result<T>) -> Result<T> {
    let mut store = std::mem::take(&mut host.data_services.tasks.fes.platform.save_panels);
    let result = f(&mut store, host);
    host.data_services.tasks.fes.platform.save_panels = store;
    result
}

pub(crate) fn apply_effect(host: &mut LoggingHost<'_>, view: &FesTaskView,
    runtime: &mut FesRuntime, driver: &mut FesScriptDriver,
    effect: &FesDriverEffect, shared: &mut ExpressionState) -> Result<Option<bool>> {
    let result = with_store(host, |store, host| store.apply_effect(host, view, runtime, driver, effect, shared))?;
    if result.is_some() && matches!(effect,
        FesDriverEffect::Initialize { event: cs2_script::FesEvent::CreateObject(definition), .. }
            if definition.kind == b"SAVEPANEL") {
        crate::fes_task_platform::bind_save_panel_layers(host, view.id)?;
    }
    Ok(result)
}

pub(crate) fn update_owner(host: &mut LoggingHost<'_>, view: &FesTaskView,
    runtime: &mut FesRuntime) -> Result<()> {
    with_store(host, |store, host| store.update_owner(host, view, runtime))
}
pub(crate) fn release_bank(host: &mut LoggingHost<'_>, owner: u32,
    name: &[u8], index: i32, runtime: &mut FesRuntime) -> Result<()> {
    host.data_services.tasks.fes.platform.save_panels.release_bank(owner, name, index, runtime)
}
pub(crate) fn release_owner(host: &mut LoggingHost<'_>, owner: u32,
    runtime: &mut FesRuntime) -> Result<()> {
    host.data_services.tasks.fes.platform.save_panels.release_owner(owner, runtime)
}

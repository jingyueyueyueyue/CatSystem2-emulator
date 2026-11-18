//! Extension chain for real FES object services, kept separate from the CPU
//! image/list platform. None means unhandled without effects.
use anyhow::Result;
use cs2_script::ExpressionState;

use super::fes_driver::{FesDriverEffect, FesScriptDriver};
use super::fes_tasks::FesTaskView;
use super::{FesRuntime, LoggingHost};

pub(super) fn apply_effect(
    host: &mut LoggingHost<'_>, view: &FesTaskView, runtime: &mut FesRuntime,
    driver: &mut FesScriptDriver, effect: &FesDriverEffect, shared: &mut ExpressionState,
) -> Result<Option<bool>> {
    if let Some(result) = super::fes_driver::main_flow::apply_effect(host, view, driver, effect)? {
        return Ok(Some(result));
    }
    super::fes_motion::observe(host, view, driver, effect)?;
    if let Some(result) = super::emote_native::settings::apply_effect(host, driver, effect, shared)? {
        return Ok(Some(result));
    }
    if let Some(result) = super::fes_settings::apply_effect(host, view, runtime, driver, effect, shared)? {
        return Ok(Some(result));
    }
    if matches!(effect, FesDriverEffect::Waiting { .. } | FesDriverEffect::CancelWait { .. }) {
        if let Some(result) = super::fes_motion::apply_effect(host, view, runtime, driver, effect, shared)? {
            return Ok(Some(result));
        }
    }
    if let Some(result) = super::fes_draw::apply_effect(host, view, runtime, driver, effect, shared)? {
        return Ok(Some(result));
    }
    if let Some(result) = super::fes_motion::apply_effect(host, view, runtime, driver, effect, shared)? {
        return Ok(Some(result));
    }
    if let Some(result) = super::fes_save_panel::apply_effect(host, view, runtime, driver, effect, shared)? {
        return Ok(Some(result));
    }
    if let Some(result) = super::fes_controls::apply_effect(host, view, runtime, driver, effect, shared)? {
        return Ok(Some(result));
    }
    if let Some(result) = super::fes_controls::apply_text_viewport_initializer(host, view, runtime, driver, effect, shared)? {
        return Ok(Some(result));
    }
    if let Some(result) = super::fes_globals::apply_effect(host, view, runtime, driver, effect, shared)? {
        return Ok(Some(result));
    }
    if let Some(result) = driver.apply_file_effect(host, effect, shared)? {
        return Ok(Some(result));
    }
    if let Some(result) = super::fes_particle::apply_effect(host, view, runtime, driver, effect, shared)? {
        return Ok(Some(result));
    }
    if let Some(result) = super::fes_sound::apply_effect(host, view, runtime, driver, effect, shared)? {
        return Ok(Some(result));
    }
    super::fes_text::apply_effect(host, view, runtime, driver, effect, shared)
}

pub(super) fn finish(
    host: &mut LoggingHost<'_>, view: &FesTaskView, runtime: &mut FesRuntime,
    driver: &mut FesScriptDriver,
) -> Result<bool> {
    if !super::fes_controls::update_owner(host, view, runtime, driver)? { return Ok(false); }
    super::fes_text::update_owner(host, view, runtime, driver)?;
    super::fes_save_panel::update_owner(host, view, runtime)?;
    if !super::fes_particle::update_owner(host, view, runtime, driver)? { return Ok(false); }
    if !super::fes_sound::update_owner(host, view, runtime, driver)? { return Ok(false); }
    super::fes_draw::update_owner(host, view, runtime, driver)
}

pub(super) fn release(host: &mut LoggingHost<'_>, owner: u32) -> Result<()> {
    super::fes_sound::prepare_release_owner(host, owner)?;
    super::fes_particle::release_owner(host, owner)?;
    super::fes_motion::release_owner(host, owner);
    super::fes_draw::release_owner(host, owner)?;
    super::fes_controls::release_owner(host, owner)?;
    super::fes_text::release_owner(host, owner);
    super::fes_sound::release_owner(host, owner);
    super::fes_globals::release_owner(host, owner);
    super::fes_settings::release_owner(host, owner);
    Ok(())
}

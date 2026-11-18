//! FES PLANE draw and wipe lifetimes backed by the native transition tasks.
//! Temporary DRAW identities belong to the FES owner; the native task and
//! captured pixels are released only through their normal ownership paths.
use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use cs2_renderer::RgbaFrame;
use cs2_script::{ExpressionState, FesEvent, FesObjectDefinition, KcsProgram};
use cs2_runtime::graph_serde::{Serialize, Deserialize};

use super::fes_driver::{FesDriverEffect, FesScriptDriver};
use super::fes_tasks::FesTaskView;
use super::{FesRuntime, LoggingHost};

pub(super) const DRAW_NAME: &[u8] = b"__drawobj__";
pub(super) const WIPE_NAME: &[u8] = b"__wipeobj__";

#[path = "fes_draw_state.rs"]
mod state;

#[derive(Default, Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(super) struct FesDrawStore {
    owners: BTreeMap<u32, Draw>,
    restore_lists: BTreeMap<u32, u32>,
}

impl FesDrawStore {
    pub(crate) fn check_graph_boundary(&self) -> Result<()> {
        if !self.owners.is_empty() || !self.restore_lists.is_empty() {
            bail!("FES DRAW transition owner checkpoints are not yet supported");
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct Draw {
    command: usize,
    parent: String,
    parent_motion: (Vec<u8>, i32),
    duration: u32,
    creating: bool,
    native_task: Option<u32>,
    snapshot: Option<u32>,
    list: Option<u32>,
    parameters: Option<Vec<u8>>,
    phase: Option<i32>,
    last_frame: Option<u32>,
    prepared_hidden: bool,
    // Active FES wipes use a kind-26 native receiver. The optional portable
    // DRAW component still uses its version-1 kind-27 wire.
    #[serde(skip)]
    wipe: Option<Wipe>,
}

struct Wipe {
    resource: String,
    mask: Option<u32>,
}

impl Draw {
    fn temporary_name(&self) -> &'static [u8] {
        if self.wipe.is_some() { WIPE_NAME } else { DRAW_NAME }
    }

    fn task_kind(&self) -> u32 {
        if self.wipe.is_some() { 26 } else { 27 }
    }

    fn prepare_code(&self) -> u32 {
        if self.wipe.is_some() { 280 } else { 281 }
    }

    fn release_images(&mut self, host: &mut LoggingHost<'_>) -> Result<()> {
        for handle in std::iter::once(&mut self.snapshot)
            .chain(self.wipe.iter_mut().map(|wipe| &mut wipe.mask))
        {
            if let Some(id) = *handle {
                if host.image_handles.contains_key(&id) { host.release_image_wrapper(id)?; }
                *handle = None;
            }
        }
        Ok(())
    }
}

pub(super) fn has_owner(store: &FesDrawStore, owner: u32) -> bool {
    store.owners.contains_key(&owner)
}

pub(super) fn apply_effect(
    host: &mut LoggingHost<'_>, view: &FesTaskView, runtime: &mut FesRuntime,
    driver: &mut FesScriptDriver, effect: &FesDriverEffect, shared: &mut ExpressionState,
) -> Result<Option<bool>> {
    match effect {
        FesDriverEffect::Command(command)
            if command.parameters.get(1).is_some_and(|name| matches!(name.as_slice(), b"draw" | b"wipe")) =>
        {
            let target = command.parameters.first().context("FES draw target is absent")?;
            let (name, bank) = split_target(target)?;
            let Some(definition) = driver.adapter.object(name).map(|object| object.definition.clone()) else {
                // Other services may own text with a private binding; only a
                // completely absent name follows whole-line expression fallback.
                return Ok(None);
            };
            if definition.kind_name != b"PLANE" { return Ok(None); }
            if definition.bank_count != 1 && bank.is_none() { return Ok(Some(true)); }
            if let Some(draw) = host.data_services.fes_draw.owners.get(&view.id) {
                if !draw.creating || draw.command != command.offset {
                    if let Some(source) = bank {
                        driver.evaluate_parameter(source, command.context.current, shared)?;
                    }
                    return Ok(Some(true));
                }
            } else {
                let requested = bank.map(|source|
                    driver.evaluate_parameter(source, command.context.current, shared)).transpose()?;
                let index = if definition.bank_count == 1 { 0 } else { requested.unwrap() };
                if index < 0 || driver.adapter.object_bank(name, index as usize).is_none() {
                    driver.fallback_command(command, shared)?;
                    return Ok(Some(true));
                }
                let raw_name = host.text_encoding.decode(name);
                let key = if definition.bank_count == 1 { raw_name }
                    else { format!("{raw_name}[{index}]") };
                if !runtime.planes.contains_key(&key) {
                    bail!("FES draw references a PLANE without a live geometry binding");
                }
                // Resolve the target before checking the single scene-wide
                // temporary name, and never evaluate duration on a duplicate.
                if [DRAW_NAME, WIPE_NAME].iter().any(|name|
                    driver.adapter.object_bank(name, 0).is_some())
                {
                    return Ok(Some(true));
                }
                let duration = command.parameters.get(2)
                    .map(|value| driver.evaluate_parameter(value, 0, shared))
                    .transpose()?.unwrap_or(30).max(1) as u32;
                let wipe = if command.parameters[1] == b"wipe" {
                    let resource = command.parameters.get(3).filter(|name| !name.is_empty())
                        .context("FES PLANE wipe requires a mask resource")?;
                    Some(Wipe { resource: host.text_encoding.decode(resource), mask: None })
                } else {
                    None
                };
                let (kind, temporary) = if wipe.is_some() {
                    (b"WIPE".as_slice(), WIPE_NAME)
                } else {
                    (b"DRAW".as_slice(), DRAW_NAME)
                };
                let previous_status = driver.adapter.status.clone();
                driver.adapter.dispatch(FesEvent::CreateObject(FesObjectDefinition {
                    offset: command.offset, kind: kind.to_vec(), name: temporary.to_vec(),
                    bank_expression: None, banks: 1,
                }))?;
                driver.adapter.status = previous_status;
                host.data_services.fes_draw.owners.insert(view.id, Draw {
                    command: command.offset, parent: key, duration, creating: true,
                    parent_motion: (name.to_vec(), index),
                    native_task: None, snapshot: None, list: None, parameters: None,
                    phase: None, last_frame: None, prepared_hidden: false, wipe,
                });
            }
            // The documented dummy motion starts with an empty parameter
            // array. A missing source is allowed; an unreadable source is not.
            let names = host.resources.category_names(b"mot", "dummy")?;
            let mut program = None;
            for name in names {
                if let Some(resource) = host.resources.find(&name) {
                    let bytes = host.resources.read_candidate(resource)?;
                    if !bytes.is_empty() { program = Some(KcsProgram::parse(&bytes)?); }
                    break;
                }
            }
            let draw = &host.data_services.fes_draw.owners[&view.id];
            let parent = draw.parent_motion.clone();
            let name = draw.temporary_name();
            super::fes_motion::attach_draw_motion(host, view.id, name, parent, program)?;
            host.data_services.fes_draw.owners.get_mut(&view.id).unwrap().creating = false;
            Ok(Some(true))
        }
        _ => Ok(None),
    }
}

pub(super) fn update_owner(
    host: &mut LoggingHost<'_>, view: &FesTaskView, runtime: &mut FesRuntime,
    driver: &mut FesScriptDriver,
) -> Result<bool> {
    restore_submission(host, view.id)?;
    let Some(mut draw) = host.data_services.fes_draw.owners.remove(&view.id) else { return Ok(true); };
    let result = (|| -> Result<bool> {
        if draw.creating { bail!("FES DRAW has an incomplete resource operation"); }
        prepare(host, view, runtime, &mut draw)?;
        if draw.last_frame == Some(view.now) { return Ok(true); }
        let task = draw.native_task.context("FES DRAW native fade task is absent")?;
        match draw.phase {
            None => {}
            Some(-2) => draw.phase = Some(-1),
            Some(-1) => {
                host.transition_command_bytes(task, draw.prepare_code(), draw.parameters.as_deref(), &mut 0)?;
                draw.phase = Some(0);
            }
            Some(0) => {
                host.transition_command_bytes(task, 263, None, &mut 0)?;
                let elapsed = host.data_services.tasks.task_clock(view.id)
                    .context("FES DRAW owner clock is absent")?.elapsed();
                draw.phase = Some(elapsed as i32);
            }
            Some(_) => {
                let mut active = 0;
                host.transition_command_bytes(task, 265, None, &mut active)?;
                if active == 0 {
                    // Mark restoration before clearing the temporary name;
                    // host errors retain this exact object for cleanup retry.
                    if let Some(list) = draw.list {
                        host.data_services.fes_draw.restore_lists.insert(view.id, list);
                    }
                    restore_submission(host, view.id)?;
                    host.request_ordinary_task_delete(task)?;
                    draw.release_images(host)?;
                    let name = draw.temporary_name();
                    driver.adapter.remove_object_bank(name, 0);
                    super::fes_motion::detach_draw_motion(host, view.id, name);
                    draw.native_task = None;
                    draw.phase = None;
                    return Ok(true);
                }
                if draw.wipe.is_none() && !super::fes_motion::draw_motion_active(host, view.id)? {
                    host.transition_command_bytes(task, 264, None, &mut 0)?;
                }
            }
        }
        draw.last_frame = Some(view.now);
        Ok(true)
    })();
    let removed = draw.phase.is_none() && draw.native_task.is_none() && draw.snapshot.is_none();
    if !removed || result.is_err() { host.data_services.fes_draw.owners.insert(view.id, draw); }
    if matches!(result, Ok(true)) && !host.data_services.fes_draw.owners.contains_key(&view.id) {
        // Empty control waits also include image fades/ANM. Their own host
        // queries complete those; no elapsed-time approximation is used here.
        let media_active = runtime.media.values().any(|layer|
            layer.fade.is_some() || layer.animation.as_ref().and_then(|animation| animation.timeline())
                .is_some_and(|timeline| !timeline.is_finished()));
        if !media_active && !super::fes_motion::owner_motion_active(host, view.id) {
            driver.resume_control(b"");
        }
    }
    result
}

fn prepare(host: &mut LoggingHost<'_>, view: &FesTaskView, runtime: &FesRuntime, draw: &mut Draw) -> Result<()> {
    if draw.native_task.is_none() {
        let name = if draw.wipe.is_some() { b"FE Wipe".as_slice() } else { b"FE Draw".as_slice() };
        draw.native_task = Some(host.create_task_object(draw.task_kind(), view.id, name, b"")?);
        draw.phase = Some(-2);
    }
    if draw.parameters.is_none() {
        let plane = runtime.planes.get(&draw.parent).context("DRAW parent PLANE was removed")?;
        let binding = view.list.as_ref().context("FES DRAW requires a bound image list")?;
        let list = host.data_services.tasks.fes.platform.lists.get(&binding.task)
            .context("FES DRAW shared image list was released")?;
        let source = list.output;
        let list_offset = list.offset;
        let position = plane.vram;
        let size = plane.size;
        let image = host.image_handles.get(&source).context("DRAW shared output wrapper was released")?;
        let snapshot = snapshot_region(image, position, size)?;
        if draw.snapshot.is_none() { draw.snapshot = Some(host.register_image_wrapper(snapshot, true)?); }
        let mut words = vec![
            draw.duration, draw.snapshot.context("FES transition snapshot was not registered")?, 0, 0, source,
            position[0] as u32, position[1] as u32,
        ];
        if let Some(wipe) = &mut draw.wipe {
            if wipe.mask.is_none() {
                let (mask, _, _) = super::load_fes_hg_image(host.resources, &wipe.resource, 0)?;
                wipe.mask = Some(host.register_image_wrapper(mask, true)?);
            }
            words.push(wipe.mask.context("FES wipe mask was not registered")?);
        }
        words.extend([
            list_offset[0].wrapping_add(position[0]) as u32,
            list_offset[1].wrapping_add(position[1]) as u32,
        ]);
        if draw.wipe.is_some() {
            words.extend([0, 0, 0, 0]);
        } else {
            words.extend(size);
        }
        draw.parameters = Some(words.into_iter().flat_map(u32::to_le_bytes).collect());
        draw.list = Some(binding.task);
    }
    if !draw.prepared_hidden {
        let list = host.data_services.tasks.fes.platform.lists.get_mut(&draw.list.unwrap())
            .context("DRAW submission target was released")?;
        list.submit = false;
        draw.prepared_hidden = true;
    }
    Ok(())
}

fn restore_submission(host: &mut LoggingHost<'_>, owner: u32) -> Result<()> {
    if let Some(&id) = host.data_services.fes_draw.restore_lists.get(&owner) {
        let list = host.data_services.tasks.fes.platform.lists.get_mut(&id)
            .context("DRAW shared image list is unavailable during submission restore")?;
        list.submit = true;
        list.enabled = true;
        // A transition can consume its last dirty rectangle while the list
        // is hidden. Republish its complete pixels when returning ownership.
        super::fes_task_platform::compose_list(host, id, true)?;
        host.data_services.fes_draw.restore_lists.remove(&owner);
    }
    Ok(())
}

pub(super) fn release_owner(host: &mut LoggingHost<'_>, owner: u32) -> Result<()> {
    if let Some(mut draw) = host.data_services.fes_draw.owners.remove(&owner) {
        let result = (|| -> Result<()> {
            if let Some(list) = draw.list {
                if let Some(list) = host.data_services.tasks.fes.platform.lists.get_mut(&list) {
                    list.submit = true;
                    list.enabled = true;
                    list.force = true;
                }
            }
            if let Some(task) = draw.native_task {
                host.request_ordinary_task_delete(task)?;
                draw.native_task = None;
            }
            draw.release_images(host)?;
            Ok(())
        })();
        if result.is_err() { host.data_services.fes_draw.owners.insert(owner, draw); }
        result?;
    }
    super::fes_motion::detach_draw_motion(host, owner, DRAW_NAME);
    super::fes_motion::detach_draw_motion(host, owner, WIPE_NAME);
    host.data_services.fes_draw.restore_lists.remove(&owner);
    Ok(())
}

fn snapshot_region(source: &RgbaFrame, at: [i32; 2], size: [u32; 2]) -> Result<RgbaFrame> {
    let count = (size[0] as usize).checked_mul(size[1] as usize).and_then(|n| n.checked_mul(4))
        .filter(|&n| n <= 128 * 1024 * 1024).context("DRAW snapshot exceeds 128 MiB")?;
    let mut pixels = Vec::new();
    pixels.try_reserve_exact(count)?;
    pixels.resize(count, 0);
    // The list is publicly stored top-first. Its plane VRAM selects the
    // corresponding screen rectangle; the native record separately retains
    // bottom-origin source coordinates for the shared new image.
    let top = source.height() as i64 - i64::from(at[1]) - i64::from(size[1]);
    for row in 0..size[1] {
        let sy = top + i64::from(row);
        if !(0..i64::from(source.height())).contains(&sy) { continue; }
        for column in 0..size[0] {
            let sx = i64::from(at[0]) + i64::from(column);
            if !(0..i64::from(source.width())).contains(&sx) { continue; }
            let from = (sy as usize * source.width() as usize + sx as usize) * 4;
            let to = (row as usize * size[0] as usize + column as usize) * 4;
            pixels[to..to + 4].copy_from_slice(&source.pixels()[from..from + 4]);
        }
    }
    Ok(RgbaFrame::from_rgba8(size[0], size[1], pixels)?)
}

fn split_target(target: &[u8]) -> Result<(&[u8], Option<&[u8]>)> {
    let Some(open) = target.iter().position(|&byte| byte == b'[') else { return Ok((target, None)); };
    if target.last() != Some(&b']') { bail!("FES DRAW bank target is not closed"); }
    Ok((&target[..open], Some(&target[open + 1..target.len() - 1])))
}

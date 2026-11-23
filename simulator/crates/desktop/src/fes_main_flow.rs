//! Main-flow focus notifications and BUTTON navigation over live scene state.
//! Evidence: pinned FES/graphics contracts and the existing title/select/log FES.
//! This is a child of fes_driver so notifications share its ordered queue and
//! checkpoint, rather than acquiring a second scene or input-state registry.
use anyhow::{Context, Result, bail};
use cs2_runtime::{TaskCommand, fes_controls::{ControlDirection, MetadataReference}, fes_motion::x86_integer};
use cs2_script::ExpressionState;
use crate::{FesRuntime, LoggingHost};
use crate::fes_tasks::FesTaskView;
use crate::message_platform::FesButtonController;
use super::{FesDriverCommand, FesDriverEffect, FesDriverTarget, FesScriptDriver};

#[path = "fes_controls_flow.rs"]
pub(crate) mod controls_flow;

#[derive(Clone, Debug, Default)]
pub(crate) struct ImageNavigation {
    pub(crate) size: [i32; 2],
    pub(crate) mode: u32,
    pub(crate) offset: [i32; 2],
    pub(crate) control: Option<(Vec<u8>, i32)>,
}
impl ImageNavigation {
    pub(crate) fn set_size(&mut self, size: [i32; 2]) {
        self.size = size;
        if self.mode == 0 { self.offset = [size[0].wrapping_sub(16), size[1].wrapping_sub(8)]; }
    }
    pub(crate) fn set_key_position(&mut self, mode: u32, value: [i32; 2]) {
        self.mode = mode;
        if mode != 0 { self.offset = value; }
        else {
            for axis in 0..2 {
                let size = self.size[axis];
                let delta = if size > 32 { size - 16 } else if size > 16 { size - 8 } else { size / 2 };
                self.offset[axis] = self.offset[axis].wrapping_add(delta);
            }
        }
    }
}

/// Resource references live beside the geometry they update. SIZE2 persists;
/// POS2 is consumed once at the actual media/property update boundary.
#[derive(Clone, Debug, Default)]
pub(crate) struct GeometryMetadata {
    pub(crate) position_source: Option<MetadataReference>,
    pub(crate) size_source: Option<MetadataReference>,
    pub(crate) size_dirty: bool,
}

impl FesScriptDriver {
    /// Native single-statement ABI: noact returns zero, lockactive returns
    /// -3, and both append notices for the next ordinary body. The alias
    /// caller's parked handler and waiting position are untouched.
    pub(super) fn alias_focus_command(&mut self, command: &FesDriverCommand,
        shared: &mut ExpressionState) -> Result<i32> {
        std::mem::swap(&mut self.expression.shared, &mut shared.shared);
        std::mem::swap(&mut self.expression.strings, &mut shared.strings);
        let previous = self.expression.current;
        self.expression.current = command.context.current;
        let result = self.main_flow_command(command);
        self.expression.current = previous;
        std::mem::swap(&mut self.expression.strings, &mut shared.strings);
        std::mem::swap(&mut self.expression.shared, &mut shared.shared);
        let effect = result?.context("alias focus dispatcher received an unrelated command")?;
        if let Some(effect) = effect { self.defer(effect)?; }
        Ok(if command.parameters.first().is_some_and(|verb| verb == b"lockactive") { -3 } else { 0 })
    }

    /// Some(None) consumes an inspected no-op; None leaves other commands alone.
    pub(super) fn main_flow_command(&mut self, command: &FesDriverCommand)
        -> Result<Option<Option<FesDriverEffect>>> {
        if let Some(effect) = self.control_flow_command(command)? { return Ok(Some(effect)); }
        let parameters = &command.parameters;
        let verb = parameters.first().map(Vec::as_slice).unwrap_or_default();
        if verb == b"lockactive" {
            let value = parameters.get(1).map(|source| self.number(source)).transpose()?.unwrap_or(0);
            let effect = FesDriverEffect::FocusLock(value as u32);
            if self.handler.is_some() {
                self.defer(effect)?;
                return self.finish_handler(false).map(|effect| Some(Some(effect)));
            }
            return Ok(Some(Some(effect)));
        }
        if parameters.get(1).map(Vec::as_slice) != Some(b"noact") { return Ok(None); }
        let source = if verb == b"@" {
            let Some(target) = &command.object_event else {
                self.interpreter.eval_fallback(&command.source, &mut self.expression)?;
                return Ok(Some(None));
            };
            target.parameter()
        } else { verb.to_vec() };
        // Match complete declared names before inspecting the bank suffix;
        // a legacy multibyte trail byte may itself equal '['.
        let definition = self.adapter.objects.iter().map(|object| &object.definition)
            .filter(|definition| source == definition.name || source.strip_prefix(definition.name.as_slice())
                .is_some_and(|tail| tail.starts_with(b"[") && tail.ends_with(b"]")))
            .max_by_key(|definition| definition.name.len()).cloned();
        let Some(definition) = definition else {
            self.interpreter.eval_fallback(&command.source, &mut self.expression)?;
            return Ok(Some(None));
        };
        let name = definition.name.as_slice();
        let tail = &source[name.len()..];
        let bank_source = (!tail.is_empty()).then(|| &tail[1..tail.len() - 1]);
        if bank_source.is_none() && definition.bank_count != 1 { return Ok(Some(None)); }
        let bank = bank_source.map(|source| self.number(source)).transpose()?.unwrap_or(0);
        let Some(kind) = usize::try_from(bank).ok()
            .and_then(|bank| self.adapter.object_bank_definition(name, bank)).map(|bank| bank.kind_name.clone()) else {
            self.interpreter.eval_fallback(&command.source, &mut self.expression)?;
            return Ok(Some(None));
        };
        if let Some(value) = parameters.get(2) { self.number(value)?; }
        if !kind.eq_ignore_ascii_case(b"BUTTON") { return Ok(Some(None)); }
        let effect = FesDriverEffect::PreferredFocus(FesDriverTarget {
            name: name.to_vec(), bank, default_group: None, value: -1,
        });
        if self.handler.is_some() { self.defer(effect)?; Ok(Some(None)) }
        else { Ok(Some(Some(effect))) }
    }
}

pub(crate) fn apply_effect(host: &mut LoggingHost<'_>, view: &FesTaskView, driver: &FesScriptDriver,
    effect: &FesDriverEffect) -> Result<Option<bool>> {
    match effect {
        FesDriverEffect::PreferredFocus(target) => {
            let mut target = target.clone();
            // Store the raw object reference even if it is currently disabled.
            // Navigation revalidates it against the same live bindings.
            if let Some((actual, _, _)) = crate::fes_task_platform::navigation_binding(host, view.id, &target.name, target.bank) {
                target.value = actual.value;
            }
            host.data_services.messages.fes_button_controllers.entry(view.id)
                .or_insert_with(|| FesButtonController { focus: driver.focus().cloned(), ..Default::default() })
                .preferred_focus = Some(target);
        }
        FesDriverEffect::FocusLock(value) => {
            host.data_services.messages.fes_button_controllers.entry(view.id)
                .or_insert_with(|| FesButtonController { focus: driver.focus().cloned(), ..Default::default() })
                .focus_lock = *value;
        }
        _ => return Ok(None),
    }
    Ok(Some(true))
}

/// Narrow property consumer, sharing the image and plane records used to draw.
pub(crate) fn object_command(host: &LoggingHost<'_>, _view: &FesTaskView,
    runtime: &mut FesRuntime, driver: &mut FesScriptDriver,
    command: &FesDriverCommand, shared: &mut ExpressionState) -> Result<bool> {
    let verb = command.parameters.get(1).map(Vec::as_slice).unwrap_or_default();
    if !matches!(verb, b"keypos" | b"attach" | b"pos2" | b"getsize") { return Ok(false); }
    let source = if command.parameters.first().is_some_and(|value| value == b"@") {
        command.object_event.as_ref().context("navigation property @ has no event object")?.parameter()
    } else { command.parameters.first().cloned().unwrap_or_default() };
    let (name, bank) = crate::fes_media::split_target(&source, host.text_encoding)?;
    let Some(definition) = driver.adapter.object(name).map(|object| object.definition.clone()) else {
        if verb == b"getsize" { return Ok(false); }
        driver.fallback_command(command, shared)?;
        return Ok(true);
    };
    if verb == b"getsize" {
        if definition.kind_name != b"PLANE" { return Ok(false); }
        let bank = match bank {
            Some(source) => driver.evaluate_parameter(source, command.context.current, shared)?,
            None if definition.bank_count == 1 => 0,
            None => return Ok(true),
        };
        if usize::try_from(bank).ok().and_then(|bank| driver.adapter.object_bank(name, bank)).is_none() {
            return Ok(true);
        }
        let key = host.text_encoding.decode(name);
        let key = if definition.bank_count == 1 { key } else { format!("{key}[{bank}]") };
        let plane = runtime.planes.get_mut(&key).context("FES getsize plane is absent")?;
        update_plane_size(host, plane, runtime.media.get_mut(&key))?;
        let size = plane.size;
        let mut destinations = [-1; 2];
        for (destination, source) in destinations.iter_mut().zip(&command.parameters[2..]) {
            *destination = driver.evaluate_parameter(source, command.context.current, shared)?;
        }
        for (destination, value) in destinations.into_iter().zip(size) {
            if let Some(slot) = shared.shared.get_mut(destination as usize) {
                *slot = value as i32;
            }
        }
        return Ok(true);
    }
    if verb == b"pos2" && !matches!(definition.kind_name.as_slice(), b"IMAGE" | b"CIMAGE" | b"DIFIMAGE" | b"BUTTON" | b"PLANE" | b"CPLANE") {
        return Ok(false); // SLIDER and STRING retain their dedicated receivers.
    }
    let count = definition.bank_count;
    let bank = bank.map(|source| driver.evaluate_parameter(source, command.context.current, shared)).transpose()?;
    let banks: Vec<_> = match bank {
        None | Some(-1) => (0..count as i32).collect(),
        Some(value) => vec![value],
    };
    if banks.first().is_none_or(|bank| usize::try_from(*bank).ok()
        .and_then(|bank| driver.adapter.object_bank(name, bank)).is_none()) {
        driver.fallback_command(command, shared)?;
        return Ok(true);
    }
    let mut values = [0; 3];
    let mut control = None;
    let mut position_source = None;
    if verb == b"keypos" {
        for (index, value) in values.iter_mut().enumerate() {
            if let Some(source) = command.parameters.get(index + 2) {
                *value = driver.evaluate_parameter(source, 0, shared)?;
            }
        }
    } else if verb == b"pos2" {
        let Some(resource) = command.parameters.get(2) else { return Ok(true); };
        if resource.len() > 127 || resource.contains(&0) { bail!("POS2 resource exceeds its bounded field"); }
        let frame = command.parameters.get(3)
            .map(|source| driver.evaluate_parameter(source, 0, shared)).transpose()?.unwrap_or(0);
        position_source = Some(MetadataReference { name: resource.clone(), frame: frame as u32 });
    } else if let Some(target) = command.parameters.get(2) {
        let (name, bank) = crate::fes_media::split_target(target, host.text_encoding)?;
        let bank = bank.map(|source| driver.evaluate_parameter(source, command.context.current, shared))
            .transpose()?.unwrap_or(0);
        control = Some((name.to_vec(), bank));
    }
    for bank in banks {
        let Some(definition) = usize::try_from(bank).ok()
            .and_then(|bank| driver.adapter.object_bank_definition(name, bank)) else { break; };
        let key = host.text_encoding.decode(name);
        let key = if definition.bank_count == 1 { key } else { format!("{key}[{bank}]") };
        if let Some(layer) = runtime.media.get_mut(&key) {
            if verb == b"keypos" && matches!(layer.kind.as_str(), "IMAGE" | "CIMAGE" | "BUTTON") {
                layer.navigation.set_key_position(values[0] as u32, [values[1], values[2]]);
            } else if verb == b"attach" && layer.kind == "BUTTON" {
                layer.navigation.control = control.clone();
            } else if verb == b"pos2" && matches!(layer.kind.as_str(), "IMAGE" | "CIMAGE" | "DIFIMAGE" | "BUTTON") {
                layer.metadata.position_source = position_source.clone();
            }
        }
    }
    Ok(true)
}

pub(crate) fn initialize_field(runtime: &mut FesRuntime, driver: &mut FesScriptDriver,
    shared: &mut ExpressionState, encoding: crate::ScriptTextEncoding,
    key: &str, kind: &[u8], column: &[u8], source: &[u8]) -> Result<bool> {
    if column == b"EXPOS" && matches!(kind, b"PLANE" | b"CPLANE") {
        let value = driver.evaluate_parameter(source, 0, shared)?;
        runtime.planes.get_mut(key).context("EXPOS plane is absent")?.expose = value as u32;
        return Ok(true);
    }
    if matches!(column, b"POS2" | b"SIZE2") {
        let reference = crate::fes_controls::metadata_reference(source, encoding)?;
        let metadata = if matches!(kind, b"PLANE" | b"CPLANE") {
            &mut runtime.planes.get_mut(key).context("metadata plane is absent")?.metadata
        } else {
            &mut runtime.media.get_mut(key).context("metadata image is absent")?.metadata
        };
        if column == b"POS2" { metadata.position_source = Some(reference); }
        else { metadata.size_source = Some(reference); metadata.size_dirty = true; }
        return Ok(true);
    }
    Ok(false)
}

fn update_plane_size(host: &LoggingHost<'_>, plane: &mut crate::FesPlaneLayer,
    layer: Option<&mut crate::FesMediaLayer>) -> Result<()> {
    if !plane.metadata.size_dirty { return Ok(()); }
    if let Some(reference) = &plane.metadata.size_source {
        let size = crate::fes_controls::read_metadata(host, reference)?.map_or([0; 2], |(_, size)| size);
        plane.size = size.map(|value| value as u32);
        if let Some(layer) = layer {
            layer.clip = Some(cs2_renderer::Rect::new(0, 0, plane.size[0], plane.size[1]));
            layer.navigation.set_size(size);
            crate::fes_media::rebuild_solid(layer)?;
        }
    }
    plane.metadata.size_dirty = false;
    Ok(())
}

/// Resolve geometry before the CPU plane/list projection and input rectangles.
/// Uses the same resource category and HG metadata reader
/// as the existing viewport/slider consumers; this never decodes image pixels.
pub(crate) fn update_geometry(host: &LoggingHost<'_>, _view: &FesTaskView,
    runtime: &mut FesRuntime) -> Result<()> {
    for (key, plane) in &mut runtime.planes {
        update_plane_size(host, plane, runtime.media.get_mut(key))?;
        if let Some(reference) = &plane.metadata.position_source {
            let position = crate::fes_controls::read_metadata(host, reference)?.map_or([0; 2], |(position, _)| position);
            plane.position = cs2_renderer::Point::new(position[0], position[1]);
            if let Some(words) = &mut plane.motion {
                words[6] = (position[0] as f32).to_bits();
                words[7] = (position[1] as f32).to_bits();
            }
            if let Some(layer) = runtime.media.get_mut(key) { layer.position = plane.position; }
            plane.metadata.position_source = None;
        }
    }
    for (key, layer) in &mut runtime.media {
        if runtime.planes.contains_key(key) || !matches!(layer.kind.as_str(), "IMAGE" | "CIMAGE" | "DIFIMAGE" | "BUTTON") { continue; }
        if layer.metadata.size_dirty {
            // Ordinary IMAGE/BUTTON files retain SIZE2 for their own source
            // path. Their standard file rebuild does not read that reference.
            if layer.kind == "CIMAGE" && let Some(reference) = &layer.metadata.size_source {
                let size = crate::fes_controls::read_metadata(host, reference)?.map_or([0; 2], |(_, size)| size);
                layer.clip = Some(cs2_renderer::Rect::new(0, 0, size[0] as u32, size[1] as u32));
                layer.navigation.set_size(size);
                layer.metadata.size_dirty = false;
                crate::fes_media::rebuild_solid(layer)?;
            }
            layer.metadata.size_dirty = false;
        }
        if let Some(reference) = &layer.metadata.position_source {
            let position = crate::fes_controls::read_metadata(host, reference)?.map_or([0; 2], |(position, _)| position);
            layer.position = cs2_renderer::Point::new(position[0], position[1]);
            if let Some(words) = &mut layer.motion {
                words[6] = (position[0] as f32).to_bits();
                words[7] = (position[1] as f32).to_bits();
            }
            layer.metadata.position_source = None;
        }
    }
    Ok(())
}

#[derive(Clone, Debug)]
pub(crate) struct NavigationDispatch {
    pub(crate) key: u32,
    pub(crate) pointer: Option<[i32; 2]>,
    pub(crate) control: Option<TaskCommand>,
    pub(crate) stage: u8,
}

struct Button {
    target: FesDriverTarget,
    key: String,
    enabled: bool,
    grid: i32,
    cell: [i32; 2],
    position: [i32; 2],
}

fn same(left: &FesDriverTarget, right: &FesDriverTarget) -> bool {
    left.name == right.name && left.bank == right.bank
}
fn grid_binding(driver: &FesScriptDriver, target: &FesDriverTarget) -> (i32, [i32; 2]) {
    let explicit = target.parameter();
    for binding in driver.adapter.keyblock_bindings.iter().rev() {
        if binding.target == explicit || target.bank == 0 && binding.target == target.name {
            return (binding.block, binding.position);
        }
    }
    (-1, [0; 2])
}
fn buttons(host: &LoggingHost<'_>, owner: u32, runtime: &FesRuntime,
    driver: &FesScriptDriver) -> Result<Vec<Button>> {
    crate::fes_task_platform::navigation_bindings(host, owner).into_iter().filter_map(|(target, key, _)| {
        if usize::try_from(target.bank).ok().and_then(|bank| driver.adapter.object_bank(&target.name, bank)).is_none() { return None; }
        Some((|| {
            let layer = runtime.media.get(&key).context("navigation BUTTON has no image properties")?;
            let point = crate::fes_motion::button_image_position(host, owner, runtime, &key)?;
            let (grid, cell) = grid_binding(driver, &target);
            Ok(Button { target, key, enabled: layer.enabled, grid, cell,
                position: [point.x, point.y] })
        })())
    }).collect()
}

fn cell(buttons: &[Button], grid: i32, at: [i32; 2], steps: &mut usize) -> Result<Option<usize>> {
    *steps += 1;
    if *steps > 1_000_000 { bail!("FES KEYBLOCK navigation exceeds its bounded search"); }
    // A disabled first binding masks other buttons occupying the same cell.
    Ok(buttons.iter().position(|button| button.grid == grid && button.cell == at)
        .filter(|&index| buttons[index].enabled))
}
fn local_grid(buttons: &[Button], grid: i32, size: [i32; 2], start: [i32; 2],
    direction: usize, steps: &mut usize) -> Result<Option<usize>> {
    let axis = usize::from(direction >= 2);
    let step = if direction % 2 == 0 { -1 } else { 1 };
    let primary = 1 - axis;
    let mut at = start;
    at[primary] = at[primary].wrapping_add(step);
    while at[primary] >= 0 && at[primary] < size[primary] {
        if let Some(found) = cell(buttons, grid, at, steps)? { return Ok(Some(found)); }
        // The baseline's horizontal branch also scans X on this same row,
        // centred on the source Y. Preserve that axis choice.
        let center = start[axis];
        for distance in 1..size[0].max(0) {
            *steps += 1;
            if *steps > 1_000_000 { bail!("FES KEYBLOCK navigation exceeds its bounded search"); }
            for x in [center.wrapping_sub(distance), center.wrapping_add(distance)] {
                if x >= 0 && x < size[0] {
                    if let Some(found) = cell(buttons, grid, [x, at[1]], steps)? { return Ok(Some(found)); }
                }
            }
        }
        at[primary] = at[primary].wrapping_add(step);
    }
    Ok(None)
}
fn next_grid(buttons: &[Button], driver: &FesScriptDriver, source: usize,
    direction: usize, steps: &mut usize) -> Result<Option<usize>> {
    let Some(table) = &driver.adapter.keyblocks else { return Ok(None); };
    let mut grid = buttons[source].grid;
    let origin = buttons[source].position.map(|value| value as f32);
    for _ in 0..=table.blocks.len() {
        let Some(block) = table.blocks.iter().find(|block| block.index as i32 == grid) else { return Ok(None); };
        grid = block.neighbors[direction];
        if grid < 0 || !table.blocks.iter().any(|block| block.index as i32 == grid) { return Ok(None); }
        let mut closest = None;
        let mut limit = 100_000_000f32;
        for (index, button) in buttons.iter().enumerate().filter(|(_, button)| button.enabled && button.grid == grid) {
            let dx = button.position[0] as f32 - origin[0];
            let dy = button.position[1] as f32 - origin[1];
            let distance = dx * dx + dy * dy;
            if distance < limit { limit = distance; closest = Some(index); }
        }
        if let Some(index) = closest {
            if let Some(found) = cell(buttons, grid, buttons[index].cell, steps)? { return Ok(Some(found)); }
        }
    }
    bail!("FES KEYBLOCK neighbor chain does not terminate")
}

/// Prepare once before any synchronous media callback. Its result is retained
/// by the existing owner-dispatch continuation, including the pointer request.
pub(crate) fn prepare_navigation(host: &mut LoggingHost<'_>, owner: u32, key: u32,
    state: &mut FesButtonController) -> Result<(NavigationDispatch, Option<u32>)> {
    let direction = (key - 4) as usize;
    let (buttons, selected, control_ref, neighbor, response, mut steps) = {
    let runtime = host.data_services.tasks.fes.runtime(owner).context("navigation needs its live FES")?;
    let driver = runtime.driver.as_ref().context("navigation driver is executing")?;
    let buttons = buttons(host, owner, runtime, driver)?;
    let mut source = state.focus.as_ref().and_then(|target| buttons.iter().position(|button| same(&button.target, target)));
    if source.is_none() {
        let point = host.query_device_pointer()?;
        // Hit fallback uses declaration order, while cell lookup above uses
        // current name/bank dictionary order.
        let mut record_index = 0;
        while let Some(record) = crate::fes_task_platform::button_record_lookup(host, owner, record_index)? {
            let ordinal = record_index;
            record_index += 1;
            let Some(index) = buttons.iter().position(|button| same(&button.target, &record.target)) else { continue; };
            let button = &buttons[index];
            let layer = &runtime.media[&button.key];
            let local = [point[0].wrapping_sub(button.position[0]), point[1].wrapping_sub(button.position[1])];
            if local[0] < 0 || local[1] < 0 || local[0] >= layer.navigation.size[0] || local[1] >= layer.navigation.size[1] { continue; }
            let record = crate::fes_task_platform::button_record_image_lookup(host, owner, ordinal)?
                .context("navigation BUTTON declaration disappeared")?;
            if record.raw_type != 0 && let Some(image) = &record.image {
                let offset = image.metadata().offset;
                let pixel = [local[0].wrapping_sub(offset[0]), local[1].wrapping_sub(offset[1])];
                if pixel[0] < 0 || pixel[1] < 0 || pixel[0] as u32 >= image.width() || pixel[1] as u32 >= image.height() { continue; }
                if image.pixel(pixel[0], pixel[1])?.a == 0 { continue; }
            }
            source = Some(index); break;
        }
    }
    let preferred = state.preferred_focus.as_ref().and_then(|target|
        buttons.iter().position(|button| button.enabled && same(&button.target, target)));
    let mut selected = None;
    let mut control_ref = None;
    let mut neighbor = None;
    let mut steps = 0;
    let response;
    if let Some(index) = source {
        let button = &buttons[index];
        if button.enabled && button.grid >= 0 {
            let table = driver.adapter.keyblocks.as_ref().context("enabled BUTTON grid lacks KEYBLOCK table")?;
            let grid = table.blocks.iter().find(|grid| grid.index as i32 == button.grid).context("BUTTON grid index is absent")?;
            selected = local_grid(&buttons, button.grid, grid.size, button.cell, direction, &mut steps)?;
            if selected.is_none() {
                control_ref = runtime.media[&button.key].navigation.control.clone();
                neighbor = Some(index);
            }
        }
        response = if button.enabled { Some(1) } else { selected = preferred; None };
    } else { selected = preferred; response = Some(1); }
    (buttons, selected, control_ref, neighbor, response, steps)
    };
    let control = if let Some((name, bank)) = control_ref {
        let direction = [ControlDirection::Up, ControlDirection::Down, ControlDirection::Left, ControlDirection::Right][direction];
        host.data_services.fes_controls.navigate_attached_slider(owner, &name, bank, direction)?
    } else { None };
    let mut dispatch = NavigationDispatch { key, pointer: None, control, stage: 0 };
    // The real slider consumes the direction before any neighboring grid is
    // inspected, even if that grid would be invalid or cyclic.
    if dispatch.control.is_some() { return Ok((dispatch, None)); }
    let runtime = host.data_services.tasks.fes.runtime(owner).context("navigation lost its FES")?;
    let driver = runtime.driver.as_ref().context("navigation lost its driver")?;
    let next = if selected.is_none() {
        neighbor.map(|source| next_grid(&buttons, driver, source, direction, &mut steps)).transpose()?.flatten()
    } else { None };
    // Own the actual point before releasing the immutable scene borrow.
    let target_point = |index: usize| -> Result<(FesDriverTarget, [i32; 2])> {
        let button = &buttons[index];
        let layer = &runtime.media[&button.key];
        let mut point = [0, 1].map(|axis| button.position[axis].wrapping_add(layer.navigation.offset[axis]));
        if let Some(plane) = layer.plane.as_deref().and_then(|name| runtime.driver_plane_key(name))
            .filter(|name| runtime.planes[*name].expose != 0) {
            let view = FesTaskView { id: owner, parent: 0, context: Vec::new(), list: None, now: 0, paused: 0 };
            let words = crate::fes_motion::plane_render_properties(host, &view, runtime, plane)?;
            for axis in 0..2 { point[axis] = point[axis].wrapping_add(x86_integer(f32::from_bits(words[6 + axis]))); }
        }
        Ok((button.target.clone(), point))
    };
    let prepared = selected.or(next).map(target_point).transpose()?;
    if let Some((target, point)) = prepared {
        state.focus = Some(target);
        dispatch.pointer = Some(point);
        Ok((dispatch, None))
    } else { Ok((dispatch, response)) }
}

pub(crate) fn advance_navigation(host: &mut LoggingHost<'_>, owner: u32,
    pending: &mut NavigationDispatch, budget: usize) -> Result<bool> {
    if pending.stage == 0 {
        if let Some(control) = pending.control {
            let outcome = host.receive_task_command(control, &mut Default::default(), &mut 0, budget)?
                .context("navigation associated control receiver is absent")?;
            if !outcome.done { return Ok(false); }
        } else if pending.pointer.is_some() {
            let value = host.data_services.messages.fes_button_controllers.get(&owner)
                .and_then(|state| state.focus.as_ref()).context("navigation lost its selected focus")?.value;
            let hook = host.data_services.messages.fes_button_named.context("navigation needs live named-value callback")?;
            hook(host, owner, ["_PUSH_UP_", "_PUSH_DOWN_", "_PUSH_LEFT_", "_PUSH_RIGHT_"][(pending.key - 4) as usize], value)?;
        }
        pending.stage = 1;
    }
    if pending.stage == 1 {
        if let Some(point) = pending.pointer {
            host.move_fes_navigation_pointer(owner, point)?;
        }
        pending.stage = 2;
    }
    Ok(true)
}

pub(crate) fn input_error(host: &mut LoggingHost<'_>, owner: u32,
    error: impl std::fmt::Display) -> cs2_script::ScriptError {
    let detail = format!("FES task={owner} navigation: {error:#}");
    host.events.push(crate::LogicEvent {
        frame: host.frame, kind: "task-error".into(),
        detail: detail.clone(),
    });
    crate::native_state_error_owned(detail)
}

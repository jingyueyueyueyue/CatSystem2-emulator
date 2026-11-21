//! FES motion ownership over the existing KCS motion VM. Command cursors keep
//! successful expression evaluations and resource work across host failures.
use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, bail};
use cs2_renderer::{Canvas, Color, ImageDrawOptions, Point, RgbaFrame};
use cs2_runtime::ResourceLocation;
use cs2_runtime::fes_motion::{FesMotionArguments, FesMotionContext, x86_integer};
use cs2_script::{ExpressionState, FesEvent, FesSubcommand, KcsProgram, NativeReturn};

use super::fes_driver::{FesDriverCommand, FesDriverEffect, FesScriptDriver};
use super::fes_tasks::FesTaskView;
use super::motion_vm::{ExternalLayout, MotionProgram, MotionRunner, MotionStart};
use super::{
    DesktopMotionServices, FesRuntime, LoggingHost, MotionControllerState, MotionTaskState,
    ScriptTextEncoding, advance_motion_controller, create_motion_runner,
};

#[path = "fes_motion_state.rs"]
pub(crate) mod checkpoint;
use cs2_runtime::graph_serde::{Serialize, Deserialize};

type Id = (Vec<u8>, i32);
const MAX_NODES: usize = 65_536;
const MAX_BYTES: u64 = 64 * 1024 * 1024;
use super::fes_draw::{DRAW_NAME, WIPE_NAME};
pub(super) const IDENTITY: [f32; 16] = [
    1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
];

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
enum Kind {
    Base,
    Image,
    Plane,
    Slider,
    Sound,
    Other,
    Text,
}

#[derive(Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct Node {
    kind: Kind,
    plane_canvas: bool,
    parent: Option<Id>,
    serial: u64,
    delay: u32,
    #[serde(with = "checkpoint::controller")]
    controller: MotionControllerState,
}

#[derive(Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct Update {
    now: u32,
    order: Vec<Id>,
    cursor: usize,
    #[serde(with = "checkpoint::writeback")]
    writeback: Option<[u32; 36]>,
}

#[derive(Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct CommandProgress {
    verb: Vec<u8>,
    keys: Vec<Id>,
    cursor: usize,
    values: Vec<i32>,
    mapped: Option<FesMotionArguments>,
    binding: Option<FesSubcommand>,
    parameters: Option<Vec<u32>>,
    position_written: bool,
    baseline_captured: bool,
    stopping: Option<Update>,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct Owner {
    nodes: BTreeMap<Id, Node>,
    declarations: BTreeSet<usize>,
    commands: BTreeMap<usize, CommandProgress>,
    sound_blends: BTreeMap<(usize, Id), i32>,
    initial_values: BTreeMap<(usize, usize), Vec<i32>>,
    initial_parents: BTreeMap<(usize, usize), Option<Id>>,
    resources: BTreeMap<Vec<u8>, Vec<String>>,
    #[serde(with = "checkpoint::programs")]
    programs: BTreeMap<String, KcsProgram>,
    update: Option<Update>,
    completed: Option<u32>,
    waits: BTreeMap<u64, Option<Vec<Id>>>,
    next_serial: u64,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(super) struct FesMotionStore {
    owners: BTreeMap<u32, Owner>,
}

impl FesMotionStore {
    /// Synchronize only fields written by the existing text receiver. The
    /// controller is the sole owner of its full plane pose, not a second bank.
    pub(super) fn synchronize_text(
        &mut self, store: &super::fes_text::FesTextStore, owner: u32,
        name: &[u8], bank: i32, field: &[u8],
    ) -> Result<()> {
        let Some(node) = self.owners.get_mut(&owner)
            .and_then(|owner| owner.nodes.get_mut(&(name.to_vec(), bank)))
            .filter(|node| node.kind == Kind::Text) else { return Ok(()); };
        super::fes_text::synchronize_motion_properties(
            store, owner, name, bank, field, &mut node.controller.properties,
        )
    }

    pub(super) fn text_properties(
        &self, store: &super::fes_text::FesTextStore, owner: u32,
        name: &[u8], bank: i32,
    ) -> Result<Option<[u32; 36]>> {
        let Some(node) = self.owners.get(&owner)
            .and_then(|owner| owner.nodes.get(&(name.to_vec(), bank)))
            .filter(|node| node.kind == Kind::Text) else { return Ok(None); };
        let mut words = super::fes_text::motion_properties(
            store, owner, name, bank, node.controller.properties,
        )?;
        // Capture exposes independent PRI in word5; rendering uses base Z.
        words[5] = node.controller.properties[5];
        Ok(Some(words))
    }

    pub(crate) fn check_graph_boundary(&self) -> Result<()> {
        for owner in self.owners.values() {
            if owner.nodes.values().any(|node| !matches!(node.kind, Kind::Base | Kind::Image | Kind::Plane))
                || !owner.sound_blends.is_empty()
            { bail!("FES motion checkpoint needs the matching sound/control/extension owner adapter"); }
        }
        Ok(())
    }
}

fn synchronize_control_parents(host: &LoggingHost<'_>, owner_id: u32, owner: &mut Owner) {
    for target in host
        .data_services
        .fes_controls
        .object_parent_links(owner_id)
    {
        if let Some(node) = owner.nodes.get_mut(&(target.name, target.bank)) {
            node.parent = target.parent;
        }
    }
}

fn kind(bytes: &[u8]) -> Kind {
    if bytes.eq_ignore_ascii_case(b"BASE") || bytes.eq_ignore_ascii_case(b"DRAW") {
        Kind::Base
    } else if [b"IMAGE".as_slice(), b"CIMAGE", b"DIFIMAGE", b"BUTTON", b"SAVEPANEL"]
        .iter()
        .any(|name| bytes.eq_ignore_ascii_case(name))
    {
        Kind::Image
    } else if bytes.eq_ignore_ascii_case(b"PLANE") || bytes.eq_ignore_ascii_case(b"CPLANE") {
        Kind::Plane
    } else if [b"SLIDER".as_slice(), b"SCROLL2", b"SCROLL"]
        .iter()
        .any(|name| bytes.eq_ignore_ascii_case(name))
    {
        Kind::Slider
    } else if bytes.eq_ignore_ascii_case(b"SOUND") || bytes.eq_ignore_ascii_case(b"SNDTABLE") {
        Kind::Sound
    } else if [b"STRING".as_slice(), b"STRING8", b"NVSTRING", b"NVSTRING8"]
        .iter()
        .any(|name| bytes.eq_ignore_ascii_case(name))
    {
        Kind::Text
    } else {
        Kind::Other
    }
}

/// Observing declarations is separate from claiming an effect. The object's
/// existing image/text/control service still performs its real construction.
pub(super) fn observe(
    host: &mut LoggingHost<'_>,
    view: &FesTaskView,
    driver: &FesScriptDriver,
    effect: &FesDriverEffect,
) -> Result<()> {
    let FesDriverEffect::Initialize {
        event: FesEvent::CreateObject(definition),
        ..
    } = effect
    else {
        return Ok(());
    };
    let owner = host
        .data_services
        .fes_motion
        .owners
        .entry(view.id)
        .or_default();
    if owner.declarations.contains(&definition.offset) {
        return Ok(());
    }
    let count = driver
        .adapter
        .objects
        .iter()
        .rfind(|object| {
            object.definition.name == definition.name
                && object.definition.kind_name == definition.kind
        })
        .context("motion declaration has no resolved bank count")?
        .definition
        .bank_count;
    if owner
        .nodes
        .len()
        .checked_add(count)
        .is_none_or(|count| count > MAX_NODES)
    {
        bail!("FES motion object table exceeds 65536 bindings");
    }
    for bank in 0..count as i32 {
        let key = (definition.name.clone(), bank);
        if owner.nodes.contains_key(&key) {
            continue;
        }
        let serial = owner.next_serial;
        owner.next_serial = serial.checked_add(1).context("FES object order overflow")?;
        let node_kind = kind(&definition.kind);
        let mut controller = MotionControllerState::default();
        if node_kind == Kind::Sound {
            controller.properties = super::sound_media::sound_motion_properties(100, 0);
            controller.base_properties = controller.properties;
        }
        owner.nodes.insert(
            key,
            Node {
                kind: node_kind,
                plane_canvas: definition.kind.eq_ignore_ascii_case(b"PLANE"),
                parent: None,
                serial,
                delay: 0,
                controller,
            },
        );
    }
    owner.declarations.insert(definition.offset);
    Ok(())
}

pub(super) fn initialize_field(
    host: &mut LoggingHost<'_>,
    view: &FesTaskView,
    runtime: &mut FesRuntime,
    driver: &mut FesScriptDriver,
    shared: &mut ExpressionState,
    name: &[u8],
    bank: i32,
    offset: usize,
    field: usize,
    column: &[u8],
    value: &[u8],
) -> Result<bool> {
    if !matches!(
        column,
        b"PARENT"
            | b"POS"
            | b"BASE"
            | b"SCALE"
            | b"ROT"
            | b"MATRIX"
            | b"PRI"
            | b"DISP"
            | b"BLEND"
            | b"COLOR"
    ) {
        return Ok(false);
    }
    let id = (name.to_vec(), bank);
    let mut owner = host
        .data_services
        .fes_motion
        .owners
        .remove(&view.id)
        .context("FES object initialization has no motion registry")?;
    let result = (|| {
        let node_kind = owner
            .nodes
            .get(&id)
            .context("FES initialization motion binding is absent")?
            .kind;
        if column == b"PARENT" {
            let parent = if let Some(parent) = owner.initial_parents.get(&(offset, field)) {
                parent.clone()
            } else {
                let parent = if value.is_empty() {
                    None
                } else {
                    let (name, bank) = split(value, host.text_encoding)?;
                    let bank = bank
                        .map(|source| driver.evaluate_parameter(source, 0, shared))
                        .transpose()?
                        .unwrap_or(0);
                    Some((name.to_vec(), bank))
                };
                owner
                    .initial_parents
                    .insert((offset, field), parent.clone());
                parent
            };
            owner.nodes.get_mut(&id).unwrap().parent = parent;
            return Ok(true);
        }
        if !matches!(
            node_kind,
            Kind::Image | Kind::Plane | Kind::Base | Kind::Sound
        ) {
            return Ok(false);
        }
        if node_kind == Kind::Base || node_kind == Kind::Sound {
            return Ok(true);
        }
        if column == b"COLOR" && node_kind == Kind::Image {
            let key = runtime_key(&id, runtime, host.text_encoding)?;
            return Ok(runtime.media.get(&key).is_some_and(|layer| layer.kind != "CIMAGE"));
        }
        let values = owner.initial_values.entry((offset, field)).or_default();
        let parts: Vec<_> = value.split(|&byte| byte == b',').collect();
        while values.len() < parts.len() {
            values.push(driver.evaluate_parameter(parts[values.len()], 0, shared)?);
        }
        let values = values.clone();
        let previous_base_z = if node_kind == Kind::Plane {
            let key = runtime_key(&id, runtime, host.text_encoding)?;
            runtime.planes[&key].motion.map_or(0, |words| words[5])
        } else {
            0
        };
        let mut properties = capture(host, view.id, &owner, &id, runtime, host.text_encoding)?;
        match column {
            b"POS" | b"BASE" | b"SCALE" | b"ROT" => {
                let start = match column {
                    b"POS" => 6,
                    b"BASE" => 3,
                    b"SCALE" => 25,
                    _ => 29,
                };
                let count = if node_kind == Kind::Image && column != b"SCALE" {
                    2
                } else {
                    3
                };
                for (axis, &value) in values.iter().take(count).enumerate() {
                    let value = if column == b"ROT" {
                        value as f32 * std::f32::consts::PI / 180.
                    } else {
                        value as f32
                    };
                    properties[start + axis] = value.to_bits();
                }
            }
            b"MATRIX" => {
                if node_kind != Kind::Plane || values.len() != 16 {
                    bail!("FES explicit matrix needs a plane and exactly sixteen values");
                }
                for (word, &value) in properties[9..25].iter_mut().zip(&values) {
                    *word = (value as f32).to_bits();
                }
            }
            b"PRI" => {
                let priority = values[0].wrapping_add(bank);
                if node_kind == Kind::Plane {
                    let key = layer_key(&id, driver, host.text_encoding)?;
                    runtime
                        .planes
                        .get_mut(&key)
                        .context("plane priority target is absent")?
                        .priority = priority;
                    if let Some(layer) = runtime.media.get_mut(&key) {
                        layer.priority = priority;
                    }
                }
                if node_kind == Kind::Image {
                    let key = runtime_key(&id, runtime, host.text_encoding)?;
                    let layer = runtime
                        .media
                        .get_mut(&key)
                        .context("image priority target is absent")?;
                    if layer.priority != priority {
                        layer.priority = priority.wrapping_add(20);
                    }
                    properties[5] = (layer.priority.wrapping_sub(20) as f32).to_bits();
                }
            }
            b"DISP" => properties[35] = values[0] as u32,
            b"BLEND" => {
                properties[28] = (properties[28] & 0x00ff_ffff) | ((values[0] as u32 & 255) << 24)
            }
            b"COLOR" => properties[28] = values[0] as u32,
            _ => {}
        }
        if node_kind == Kind::Plane && column != b"BASE" {
            properties[5] = previous_base_z;
        }
        writeback(
            host,
            view.id,
            &mut owner,
            &id,
            runtime,
            host.text_encoding,
            properties,
        )?;
        Ok(true)
    })();
    host.data_services.fes_motion.owners.insert(view.id, owner);
    result
}

pub(super) fn apply_effect(
    host: &mut LoggingHost<'_>,
    view: &FesTaskView,
    runtime: &mut FesRuntime,
    driver: &mut FesScriptDriver,
    effect: &FesDriverEffect,
    shared: &mut ExpressionState,
) -> Result<Option<bool>> {
    if let FesDriverEffect::Waiting {
        wait:
            super::fes_driver::FesDriverWait::Control {
                kind,
                arguments,
                entered,
            },
        handler_local: false,
    } = effect
    {
        let target_name = if kind.is_empty() {
            None
        } else {
            Some(split(kind, host.text_encoding)?.0)
        };
        if kind.is_empty() || target_name.is_some_and(|name| driver.adapter.object(name).is_some())
        {
            let mut owner = host
                .data_services
                .fes_motion
                .owners
                .remove(&view.id)
                .unwrap_or_default();
            let result = (|| {
                if !owner.waits.contains_key(entered) {
                    let keys = if !kind.is_empty() {
                        let target = kind;
                        let (name, bank) = split(target, host.text_encoding)?;
                        let bank = bank
                            .map(|source| driver.evaluate_parameter(source, 0, shared))
                            .transpose()?;
                        let keys = owner
                            .nodes
                            .keys()
                            .filter(|(candidate, index)| {
                                candidate == name
                                    && bank.is_none_or(|bank| bank == -1 || bank == *index)
                            })
                            .cloned()
                            .collect();
                        Some(keys)
                    } else {
                        None
                    };
                    owner.waits.insert(*entered, keys);
                }
                let active = match &owner.waits[entered] {
                    Some(keys) => {
                        let mut active = false;
                        for key in keys {
                            active |= subtree(&owner, key)?
                                .iter()
                                .any(|key| active_node(&owner.nodes[key]));
                        }
                        active
                    }
                    None => {
                        // Bare wait excludes audio objects (fes.md execution
                        // table). Named object/subtree waits above include
                        // SOUND, as do queue advancement and ismot queries.
                        owner.nodes.values().any(|node| node.kind != Kind::Sound && active_node(node))
                            || [DRAW_NAME, WIPE_NAME].iter().any(|name|
                                driver.adapter.object_bank(name, 0).is_some())
                    }
                };
                // Root wait always commits one full boundary before resume.
                let ready = !active && (!kind.is_empty() || view.now as u64 > *entered);
                if ready {
                    driver.resume_control(kind);
                    owner.waits.remove(entered);
                }
                Ok(Some(true))
            })();
            host.data_services.fes_motion.owners.insert(view.id, owner);
            return result;
        }
    }
    if let FesDriverEffect::CancelWait {
        finish_motion: true,
        ..
    } = effect
    {
        if let Some(owner) = host.data_services.fes_motion.owners.get_mut(&view.id) {
            for node in owner.nodes.values_mut() {
                for task in node.controller.tasks.values_mut() {
                    task.finishing = 1;
                }
            }
        }
        return Ok(Some(true));
    }
    match effect {
        FesDriverEffect::Initialize {
            event: FesEvent::CreateObject(definition),
            ..
        } if kind(&definition.kind) == Kind::Base => return Ok(Some(true)),
        FesDriverEffect::Initialize {
            event: FesEvent::InitializeObject(initializer),
            ..
        } => {
            let (name, bank) = split(&initializer.target, host.text_encoding)?;
            let Some(object) = driver.adapter.object(name) else {
                return Ok(None);
            };
            let node_kind = kind(&object.definition.kind_name);
            if !matches!(node_kind, Kind::Base | Kind::Sound) {
                return Ok(None);
            }
            let count = driver.adapter.object(name).unwrap().definition.bank_count;
            let bank = bank
                .map(|source| -> Result<i32> { Ok(std::str::from_utf8(source)?.parse()?) })
                .transpose()?;
            for index in 0..count as i32 {
                if bank.is_some_and(|bank| bank != index) {
                    continue;
                }
                for (field, (column, value)) in initializer
                    .columns
                    .iter()
                    .zip(&initializer.values)
                    .enumerate()
                {
                    if column == b"PARENT" {
                        initialize_field(
                            host,
                            view,
                            runtime,
                            driver,
                            shared,
                            name,
                            index,
                            initializer.offset,
                            field,
                            column,
                            value,
                        )?;
                    }
                }
            }
            return Ok((node_kind == Kind::Base).then_some(true));
        }
        _ => {}
    }
    let FesDriverEffect::Command(command) = effect else {
        return Ok(None);
    };
    if command
        .parameters
        .first()
        .is_some_and(|name| name == b"decode")
    {
        return super::fes_task_platform::finish_task_media(host, view, runtime, driver).map(Some);
    }
    let Some(verb) = command.parameters.get(1) else {
        return Ok(None);
    };
    if matches!(verb.as_slice(), b"getpos" | b"getpos2") {
        return query_position(host, view, runtime, driver, command, shared, verb == b"getpos2");
    }
    let legacy_mcpend = verb == b"mcpend"
        && host.data_services.css_task_pe_sha256 == Some(super::css_model::SHA);
    let built_in = matches!(
        verb.as_slice(),
        b"pos" | b"blend" | b"color" | b"arc" | b"mcarc" | b"delay" | b"ismot" | b"stop"
    ) || legacy_mcpend;
    let binding = (!built_in)
        .then(|| driver.adapter.subcommand(verb).cloned())
        .flatten();
    if !built_in && binding.is_none() {
        return Ok(None);
    }
    let (raw_name, _) = split(
        command.parameters.first().map_or(&[][..], Vec::as_slice),
        host.text_encoding,
    )?;
    let Some(definition) = driver.adapter.object(raw_name) else {
        return Ok(None);
    };
    let kind_name = &definition.definition.kind_name;
    if legacy_mcpend && !kind_name.eq_ignore_ascii_case(b"IMAGE") {
        return Ok(None);
    }
    let node_kind = kind(kind_name);
    if node_kind == Kind::Other {
        let window_fade = verb == b"fade" && binding.as_ref()
            .is_some_and(|binding| binding.target.eq_ignore_ascii_case(b"motFade"));
        if !kind_name.eq_ignore_ascii_case(b"IMGWND") || (verb != b"stop" && !window_fade) {
            return Ok(None);
        }
    }
    if verb == b"color" && !matches!(node_kind, Kind::Image | Kind::Plane) {
        return Ok(None);
    }
    if verb == b"pos" && node_kind == Kind::Slider {
        return Ok(None);
    }
    // SOUND keeps the full signed blend request before capturing its byte
    // motion properties; its command service calls queue_sound_blend below.
    if verb == b"blend" && node_kind == Kind::Sound {
        return Ok(None);
    }
    let mut owner = host
        .data_services
        .fes_motion
        .owners
        .remove(&view.id)
        .context("FES motion command has no object registry")?;
    synchronize_control_parents(host, view.id, &mut owner);
    let mut pending = match owner.commands.remove(&command.offset) {
        Some(pending) => pending,
        None => {
            let (name, bank) = split(&command.parameters[0], host.text_encoding)?;
            let bank = bank
                .map(|source| driver.evaluate_parameter(source, command.context.current, shared))
                .transpose();
            let bank = match bank {
                Ok(bank) => bank,
                Err(error) => {
                    host.data_services.fes_motion.owners.insert(view.id, owner);
                    return Err(error);
                }
            };
            let count = driver.adapter.object(name).unwrap().definition.bank_count;
            let keys = match bank {
                Some(bank) if bank >= 0 => vec![(name.to_vec(), bank)],
                None | Some(-1) => (0..count as i32)
                    .map(|bank| (name.to_vec(), bank))
                    .collect(),
                _ => Vec::new(),
            };
            if !matches!(verb.as_slice(), b"blend" | b"color")
                && keys
                    .first()
                    .is_none_or(|key| !owner.nodes.contains_key(key))
            {
                let result = driver.fallback_command(command, shared);
                host.data_services.fes_motion.owners.insert(view.id, owner);
                result?;
                return Ok(Some(true));
            }
            CommandProgress {
                verb: verb.clone(),
                keys,
                cursor: 0,
                values: Vec::new(),
                mapped: None,
                binding,
                parameters: None,
                position_written: false,
                baseline_captured: false,
                stopping: None,
            }
        }
    };
    let result = run_command(
        host,
        view,
        runtime,
        driver,
        shared,
        command,
        &mut owner,
        &mut pending,
    );
    if !matches!(result, Ok(true)) {
        owner.commands.insert(command.offset, pending);
    }
    host.data_services.fes_motion.owners.insert(view.id, owner);
    result.map(Some)
}

fn run_command(
    host: &mut LoggingHost<'_>,
    view: &FesTaskView,
    runtime: &mut FesRuntime,
    driver: &mut FesScriptDriver,
    shared: &mut ExpressionState,
    command: &FesDriverCommand,
    owner: &mut Owner,
    pending: &mut CommandProgress,
) -> Result<bool> {
    let arguments = &command.parameters[2..];
    let query = pending.verb == b"ismot";
    if query && arguments.is_empty() {
        return Ok(true);
    }
    let count = if matches!(pending.verb.as_slice(), b"delay" | b"ismot") {
        1
    } else if matches!(pending.verb.as_slice(), b"blend" | b"color") {
        1
    } else if pending.verb == b"pos" {
        3
    } else if matches!(pending.verb.as_slice(), b"arc" | b"mcarc") {
        5
    } else if pending.verb == b"mcpend" {
        4
    } else {
        0
    };
    while pending.values.len() < count {
        let index = pending.values.len();
        let current = if pending.verb == b"pos"
            && owner
                .nodes
                .get(&pending.keys[0])
                .is_some_and(|node| matches!(node.kind, Kind::Image | Kind::Plane | Kind::Text))
        {
            let properties = capture(
                host,
                view.id,
                owner,
                &pending.keys[0],
                runtime,
                host.text_encoding,
            )?;
            x86_integer(f32::from_bits(properties[6 + index]))
        } else {
            0
        };
        let default = if pending.verb == b"blend" {
            255
        } else if pending.verb == b"pos" {
            current
        } else if matches!(pending.verb.as_slice(), b"arc" | b"mcarc" | b"mcpend")
            && index == 0
        {
            30
        } else {
            0
        };
        pending.values.push(match arguments.get(index) {
            Some(source) => driver.evaluate_parameter(source, current, shared)?,
            None => default,
        });
    }
    if query {
        let mut active = false;
        for key in &pending.keys {
            if !owner.nodes.contains_key(key) {
                return Ok(true);
            }
            active |= subtree(owner, key)?
                .iter()
                .any(|key| active_node(&owner.nodes[key]));
        }
        if let Some(slot) = shared.shared.get_mut(pending.values[0] as u32 as usize) {
            *slot = i32::from(active);
        }
        return Ok(true);
    }
    while let Some(key) = pending.keys.get(pending.cursor).cloned() {
        if !owner.nodes.contains_key(&key) {
            if matches!(pending.verb.as_slice(), b"pos" | b"stop" | b"blend" | b"color") {
                pending.cursor += 1;
                continue;
            }
            return Ok(true);
        }
        if pending.verb == b"delay" {
            owner.nodes.get_mut(&key).unwrap().delay = pending.values[0] as u32;
        } else if pending.verb == b"stop" {
            if pending.stopping.is_none() {
                let order = subtree(owner, &key)?;
                for id in &order {
                    for task in owner
                        .nodes
                        .get_mut(id)
                        .unwrap()
                        .controller
                        .tasks
                        .values_mut()
                    {
                        task.finishing = 1;
                    }
                }
                pending.stopping = Some(Update {
                    now: 0,
                    order,
                    cursor: 0,
                    writeback: None,
                });
            }
            if !advance_update(
                host,
                view.id,
                runtime,
                owner,
                pending.stopping.as_mut().unwrap(),
                4096,
            )? {
                return Ok(false);
            }
            let node = owner.nodes.get_mut(&key).unwrap();
            node.controller.tasks.clear();
            node.controller.pending = None;
            node.delay = 0;
            if node.kind == Kind::Sound {
                // PE RVA 2c9d00 calls the SOUND stop request after the shared
                // subtree motion update and target queue/delay cleanup.
                super::fes_sound::request_stop(
                    &mut host.data_services.fes_sound,
                    view.id,
                    &key.0,
                    key.1,
                )?;
            }
            pending.stopping = None;
        } else {
            let node_kind = owner.nodes[&key].kind;
            if matches!(pending.verb.as_slice(), b"pos" | b"blend") && node_kind == Kind::Base
                || pending.verb == b"pos" && node_kind == Kind::Sound
            {
                pending.cursor += 1;
                continue;
            }
            if pending.parameters.is_none() {
                if let Some(binding) = &pending.binding {
                    if pending.mapped.is_none() {
                        pending.mapped = Some(FesMotionArguments::new(context(
                            host,
                            view.id,
                            owner,
                            &key,
                            runtime,
                            host.text_encoding,
                        )?));
                    }
                    let values = pending.mapped.as_mut().unwrap().complete(
                        binding,
                        arguments,
                        |source, current| {
                            driver
                                .evaluate_parameter(source, current, shared)
                                .map_err(|error| cs2_runtime::RuntimeError::Host(error.to_string()))
                        },
                    )?;
                    pending.parameters = Some(values.to_vec());
                } else if pending.verb == b"pos" {
                    let current = capture(host, view.id, owner, &key, runtime, host.text_encoding)?;
                    let mut values: Vec<_> = pending
                        .values
                        .iter()
                        .enumerate()
                        .map(|(axis, &value)| {
                            let value = if arguments.get(axis).is_none() {
                                x86_integer(f32::from_bits(current[6 + axis]))
                            } else {
                                value
                            };
                            (value as f32).to_bits()
                        })
                        .collect();
                    if node_kind == Kind::Image {
                        values[2] = 0;
                    }
                    pending.parameters = Some(values);
                } else if pending.verb == b"mcpend" {
                    let values = pending.values.as_slice().try_into()?;
                    pending.parameters = Some(mcpend_parameters(values).to_vec());
                } else if matches!(pending.verb.as_slice(), b"arc" | b"mcarc") {
                    let mut values: Vec<_> = pending
                        .values
                        .iter()
                        .map(|&value| (value as f32).to_bits())
                        .collect();
                    values[0] = (pending.values[0].max(0) as f32).to_bits();
                    for index in [1, 2] {
                        values[index] =
                            (pending.values[index] as f32 * std::f32::consts::PI / 180.).to_bits();
                    }
                    values.push(0);
                    pending.parameters = Some(values);
                } else if pending.verb == b"blend" {
                    let current = if node_kind == Kind::Text {
                        capture(host, view.id, owner, &key, runtime, host.text_encoding)?[28]
                    } else if node_kind == Kind::Plane {
                        context(host, view.id, owner, &key, runtime, host.text_encoding)?.argb
                    } else {
                        u32::MAX
                    };
                    pending.parameters = Some(vec![
                        (pending.values[0] as f32).to_bits(),
                        (((current >> 16) & 255) as f32).to_bits(),
                        (((current >> 8) & 255) as f32).to_bits(),
                        ((current & 255) as f32).to_bits(),
                    ]);
                } else if pending.verb == b"color" {
                    let argb = pending.values[0] as u32;
                    pending.parameters = Some(
                        [24, 16, 8, 0]
                            .map(|shift| (((argb >> shift) & 255) as f32).to_bits())
                            .to_vec(),
                    );
                } else {
                    bail!("FES motion command lacks its mapped parameter vector");
                }
            }
            if pending.verb == b"pos" && !pending.position_written {
                let mut properties =
                    capture(host, view.id, owner, &key, runtime, host.text_encoding)?;
                properties[6..9].copy_from_slice(&pending.parameters.as_ref().unwrap()[..3]);
                if node_kind == Kind::Text {
                    properties[5] = owner.nodes[&key].controller.properties[5];
                } else if node_kind == Kind::Plane {
                    let key = runtime_key(&key, runtime, host.text_encoding)?;
                    properties[5] = runtime.planes[&key].motion.map_or(0, |words| words[5]);
                }
                writeback(
                    host,
                    view.id,
                    owner,
                    &key,
                    runtime,
                    host.text_encoding,
                    properties,
                )?;
                pending.position_written = true;
            }
            if matches!(pending.verb.as_slice(), b"blend" | b"color") && !pending.position_written {
                if pending.verb == b"color" && node_kind == Kind::Image {
                    let name = runtime_key(&key, runtime, host.text_encoding)?;
                    let layer = runtime.media.get_mut(&name)
                        .context("FES color image was released")?;
                    let argb = pending.values[0] as u32;
                    layer.color = Color::rgba(
                        (argb >> 16) as u8,
                        (argb >> 8) as u8,
                        argb as u8,
                        (argb >> 24) as u8,
                    );
                    if layer.kind == "CIMAGE" {
                        super::fes_media::rebuild_solid(layer)?;
                    }
                } else {
                    let mut properties =
                        capture(host, view.id, owner, &key, runtime, host.text_encoding)?;
                    properties[28] = if pending.verb == b"color" {
                        pending.values[0] as u32
                    } else {
                        (properties[28] & 0x00ff_ffff) | ((pending.values[0] as u32 & 255) << 24)
                    };
                    if node_kind == Kind::Text {
                        properties[5] = owner.nodes[&key].controller.properties[5];
                    } else if node_kind == Kind::Plane {
                        let name = runtime_key(&key, runtime, host.text_encoding)?;
                        properties[5] = runtime.planes[&name].motion.map_or(0, |words| words[5]);
                    }
                    writeback(
                        host,
                        view.id,
                        owner,
                        &key,
                        runtime,
                        host.text_encoding,
                        properties,
                    )?;
                }
                pending.position_written = true;
            }
            let resource = if let Some(binding) = &pending.binding {
                binding.target.clone()
            } else {
                match pending.verb.as_slice() {
                    b"pos" => b"pPos".to_vec(),
                    b"blend" | b"color" => b"pColor".to_vec(),
                    b"arc" => b"motArc".to_vec(),
                    b"mcpend" => b"mcPend".to_vec(),
                    _ => b"mcArc".to_vec(),
                }
            };
            if !pending.baseline_captured {
                let inactive = !subtree(owner, &key)?
                    .iter()
                    .any(|key| active_node(&owner.nodes[key]));
                if inactive && node_kind != Kind::Base {
                    let properties =
                        capture(host, view.id, owner, &key, runtime, host.text_encoding)?;
                    let controller = &mut owner.nodes.get_mut(&key).unwrap().controller;
                    controller.base_properties = properties;
                    controller.properties = properties;
                }
                pending.baseline_captured = true;
            }
            if let Some((program, layout)) = load_program(host, owner, &resource)? {
                let node = owner.nodes.get_mut(&key).unwrap();
                let mut parameters = [0; 256];
                let mapped = pending.parameters.as_ref().unwrap();
                parameters[..mapped.len()].copy_from_slice(mapped);
                let mut runner = create_fes_motion_runner(
                    program,
                    layout,
                    &resource,
                    parameters,
                    node.controller.base_properties,
                )?;
                runner.set_base_time(node.delay)?;
                let id = node.controller.next_task_id;
                node.controller.next_task_id =
                    id.checked_add(1).context("FES motion number overflow")?;
                node.controller.tasks.insert(
                    id,
                    MotionTaskState {
                        name: resource,
                        group: i32::from(matches!(pending.verb.as_slice(), b"mcarc" | b"mcpend")),
                        parameters,
                        saved_data: Vec::new(),
                        snapshot_flags: 0,
                        external_value: 0,
                        origin: 0,
                        elapsed: 0,
                        running: 1,
                        finishing: 0,
                        runner,
                    },
                );
            }
        }
        pending.cursor += 1;
        pending.parameters = None;
        pending.mapped = None;
        pending.position_written = false;
        pending.baseline_captured = false;
    }
    Ok(true)
}

fn mcpend_parameters([period, x_degrees, y_degrees, z_degrees]: [i32; 4]) -> [u32; 4] {
    let radians = |degrees: i32| (f64::from(degrees) * f64::from(std::f32::consts::PI) / 180.0) as f32;
    [
        period.max(0) as f32,
        radians(x_degrees),
        radians(y_degrees),
        radians(z_degrees),
    ]
    .map(f32::to_bits)
}

/// Call after SOUND has saved its full signed volume. The captured words are
/// supplied by that service because its bank store may be temporarily moved
/// out of DataServices while the command is executing.
pub(super) fn queue_sound_blend(
    host: &mut LoggingHost<'_>,
    view: &FesTaskView,
    command_offset: usize,
    name: &[u8],
    bank: i32,
    value: i32,
    properties: [u32; 36],
) -> Result<()> {
    let mut owner = host
        .data_services
        .fes_motion
        .owners
        .remove(&view.id)
        .context("SOUND blend has no common motion registry")?;
    let id = (name.to_vec(), bank);
    let stamp = (command_offset, id.clone());
    synchronize_control_parents(host, view.id, &mut owner);
    let result = (|| {
        if owner
            .nodes
            .get(&id)
            .is_none_or(|node| node.kind != Kind::Sound)
        {
            bail!("SOUND blend target has no common motion bank");
        }
        if let Some(previous) = owner.sound_blends.get(&stamp) {
            if *previous != value {
                bail!("SOUND blend continuation changed its evaluated value");
            }
        } else {
            if !subtree(&owner, &id)?
                .iter()
                .any(|id| active_node(&owner.nodes[id]))
            {
                let controller = &mut owner.nodes.get_mut(&id).unwrap().controller;
                controller.base_properties = properties;
                controller.properties = properties;
            }
            owner.sound_blends.insert(stamp.clone(), value);
        }
        if let Some((program, layout)) = load_program(host, &mut owner, b"pColor")? {
            let node = owner.nodes.get_mut(&id).unwrap();
            let mut parameters = [0; 256];
            parameters[..4].copy_from_slice(&[
                (value as f32).to_bits(),
                255f32.to_bits(),
                255f32.to_bits(),
                255f32.to_bits(),
            ]);
            let next = node
                .controller
                .next_task_id
                .checked_add(1)
                .context("SOUND motion number overflow")?;
            let mut runner = create_fes_motion_runner(
                program,
                layout,
                b"pColor",
                parameters,
                node.controller.base_properties,
            )?;
            runner.set_base_time(node.delay)?;
            node.controller.tasks.insert(
                node.controller.next_task_id,
                MotionTaskState {
                    name: b"pColor".to_vec(),
                    group: 0,
                    parameters,
                    saved_data: Vec::new(),
                    snapshot_flags: 0,
                    external_value: 0,
                    origin: 0,
                    elapsed: 0,
                    running: 1,
                    finishing: 0,
                    runner,
                },
            );
            node.controller.next_task_id = next;
        }
        Ok(())
    })();
    if result.is_ok() {
        owner.sound_blends.remove(&stamp);
    }
    host.data_services.fes_motion.owners.insert(view.id, owner);
    result
}

pub(super) fn advance_owner(
    host: &mut LoggingHost<'_>,
    view: &FesTaskView,
    runtime: &mut FesRuntime,
    _driver: &mut FesScriptDriver,
    budget: usize,
) -> Result<bool> {
    let Some(mut owner) = host.data_services.fes_motion.owners.remove(&view.id) else {
        return Ok(true);
    };
    if owner.update.is_none() {
        synchronize_control_parents(host, view.id, &mut owner);
    }
    if owner.completed == Some(view.now) {
        host.data_services.fes_motion.owners.insert(view.id, owner);
        return Ok(true);
    }
    let mut update = match owner.update.take() {
        Some(update) => update,
        None => match forest(&owner) {
            Ok(order) => Update {
                now: view.now,
                order,
                cursor: 0,
                writeback: None,
            },
            Err(error) => {
                host.data_services.fes_motion.owners.insert(view.id, owner);
                return Err(error);
            }
        },
    };
    let result = if update.now != view.now {
        Err(anyhow::anyhow!("FES motion continuation changed its time"))
    } else {
        advance_update(host, view.id, runtime, &mut owner, &mut update, budget)
    };
    if matches!(result, Ok(true)) {
        owner.completed = Some(view.now);
    } else {
        owner.update = Some(update);
    }
    host.data_services.fes_motion.owners.insert(view.id, owner);
    result
}

fn advance_update(
    host: &mut LoggingHost<'_>,
    owner_id: u32,
    runtime: &mut FesRuntime,
    owner: &mut Owner,
    update: &mut Update,
    budget: usize,
) -> Result<bool> {
    if budget == 0 {
        return Ok(false);
    }
    let mut count = 0;
    while let Some(key) = update.order.get(update.cursor) {
        let Some(node) = owner.nodes.get_mut(key) else {
            update.cursor += 1;
            continue;
        };
        if !node.controller.tasks.is_empty() && update.writeback.is_none() {
            let result = advance_motion_controller(
                &mut node.controller,
                update.now,
                false,
                false,
                &mut DesktopMotionServices {
                    random: host.script_random,
                    events: host.events,
                    frame: host.frame,
                },
                budget,
            )?;
            if matches!(result, NativeReturn::Retry) {
                return Ok(false);
            }
            update.writeback = Some(node.controller.properties);
        }
        if let Some(properties) = update.writeback {
            writeback(
                host,
                owner_id,
                owner,
                key,
                runtime,
                host.text_encoding,
                properties,
            )?;
            update.writeback = None;
        }
        update.cursor += 1;
        count += 1;
        if count >= budget && update.cursor < update.order.len() {
            return Ok(false);
        }
    }
    for key in &update.order {
        let Some(node) = owner.nodes.get_mut(key) else {
            continue;
        };
        if !node.controller.tasks.values().any(|task| task.running != 0) {
            node.controller.tasks.clear();
        }
    }
    Ok(true)
}

fn active_node(node: &Node) -> bool {
    node.controller.tasks.values().any(|task| task.running != 0)
}

fn forest(owner: &Owner) -> Result<Vec<Id>> {
    let mut roots: Vec<_> = owner
        .nodes
        .iter()
        .filter(|(_, node)| node.parent.is_none())
        .map(|(id, node)| (node.serial, id.clone()))
        .collect();
    roots.sort_by_key(|(serial, _)| *serial);
    let mut result = Vec::new();
    let mut seen = BTreeSet::new();
    for (_, root) in roots {
        visit(owner, &root, &mut seen, &mut result, 0)?;
    }
    Ok(result)
}

fn subtree(owner: &Owner, root: &Id) -> Result<Vec<Id>> {
    let mut result = Vec::new();
    visit(owner, root, &mut BTreeSet::new(), &mut result, 0)?;
    Ok(result)
}

fn visit(
    owner: &Owner,
    id: &Id,
    seen: &mut BTreeSet<Id>,
    result: &mut Vec<Id>,
    depth: usize,
) -> Result<()> {
    if depth > 256 || !seen.insert(id.clone()) {
        bail!("FES motion parent tree contains a cycle or exceeds 256 levels");
    }
    let mut children: Vec<_> = owner
        .nodes
        .iter()
        .filter(|(_, node)| node.parent.as_ref() == Some(id))
        .map(|(id, node)| (node.serial, id.clone()))
        .collect();
    children.sort_by_key(|(serial, _)| *serial);
    for (_, child) in children {
        visit(owner, &child, seen, result, depth + 1)?;
    }
    result.push(id.clone());
    Ok(())
}

fn context(
    host: &LoggingHost<'_>,
    owner_id: u32,
    owner: &Owner,
    id: &Id,
    runtime: &FesRuntime,
    encoding: ScriptTextEncoding,
) -> Result<FesMotionContext> {
    let node = &owner.nodes[id];
    if node.kind == Kind::Text {
        return super::fes_text::motion_context(
            &host.data_services.fes_text, owner_id, &id.0, id.1,
            node.controller.properties,
        );
    }
    if node.kind == Kind::Sound {
        let words = capture(host, owner_id, owner, id, runtime, encoding)?;
        return Ok(FesMotionContext {
            argb: words[28] & 0xff00_0000,
            ..Default::default()
        });
    }
    if node.kind == Kind::Slider || node.kind == Kind::Base {
        return Ok(FesMotionContext {
            argb: 0,
            ..Default::default()
        });
    }
    if node.kind == Kind::Other {
        let words = capture(host, owner_id, owner, id, runtime, encoding)?;
        return Ok(FesMotionContext { argb: words[28], ..Default::default() });
    }
    let words = capture(host, owner_id, owner, id, runtime, encoding)?;
    let key = runtime_key(id, runtime, encoding)?;
    let argb = if node.kind == Kind::Image {
        let color = runtime.media[&key].color;
        u32::from(color.a) << 24
            | u32::from(color.r) << 16
            | u32::from(color.g) << 8
            | u32::from(color.b)
    } else {
        words[28]
    };
    let mut base = [
        f32::from_bits(words[3]),
        f32::from_bits(words[4]),
        f32::from_bits(words[5]),
    ];
    let velocity = if node.kind == Kind::Plane && node.plane_canvas {
        let vram = runtime.planes[&key].vram;
        [vram[0] as f32, vram[1] as f32, 0.]
    } else {
        [0.; 3]
    };
    if node.kind == Kind::Plane {
        base[2] = runtime.planes[&key]
            .motion
            .as_ref()
            .map_or(0., |words| f32::from_bits(words[5]));
    } else {
        base[2] = runtime.media[&key].priority as f32;
    }
    Ok(FesMotionContext {
        base,
        position: std::array::from_fn(|axis| f32::from_bits(words[6 + axis])),
        velocity,
        rotation: std::array::from_fn(|axis| f32::from_bits(words[29 + axis])),
        scale: std::array::from_fn(|axis| f32::from_bits(words[25 + axis])),
        argb,
    })
}

fn capture(
    host: &LoggingHost<'_>,
    owner_id: u32,
    owner: &Owner,
    id: &Id,
    runtime: &FesRuntime,
    encoding: ScriptTextEncoding,
) -> Result<[u32; 36]> {
    let node = &owner.nodes[id];
    if node.kind == Kind::Text {
        return super::fes_text::motion_properties(
            &host.data_services.fes_text, owner_id, &id.0, id.1,
            node.controller.properties,
        );
    }
    if node.kind == Kind::Base {
        return Ok(node.controller.properties);
    }
    if matches!(node.kind, Kind::Slider | Kind::Other) {
        return host
            .data_services
            .fes_controls
            .alpha_motion_properties(owner_id, &id.0, id.1)
            .context("FES motion alpha target has no live control bank");
    }
    if node.kind == Kind::Sound {
        return super::fes_sound::motion_properties(
            &host.data_services.fes_sound,
            owner_id,
            &id.0,
            id.1,
        )
        .context("motion target SOUND bank was released");
    }
    let key = runtime_key(id, runtime, encoding)?;
    if node.kind == Kind::Image {
        let layer = runtime
            .media
            .get(&key)
            .context("FES motion image source is absent")?;
        let mut words = layer.motion.unwrap_or([0; 36]);
        if layer.motion.is_none() {
            for axis in 0..2 {
                words[3 + axis] = (layer.image_base[axis] as f32).to_bits();
                words[6 + axis] = ([layer.position.x, layer.position.y][axis] as f32).to_bits();
            }
            for word in &mut words[25..28] {
                *word = 1f32.to_bits();
            }
        }
        words[5] = (layer.priority.wrapping_sub(20) as f32).to_bits();
        words[28] = u32::from(layer.opacity) << 24;
        if (words[35] != 0) != layer.visible {
            words[35] = u32::from(layer.visible);
        }
        return Ok(words);
    }
    if node.kind == Kind::Plane {
        let plane = runtime
            .planes
            .get(&key)
            .context("FES motion plane source is absent")?;
        let mut words = plane.motion.unwrap_or([0; 36]);
        if plane.motion.is_none() {
            words[6] = (plane.position.x as f32).to_bits();
            words[7] = (plane.position.y as f32).to_bits();
            words[9..25].copy_from_slice(&IDENTITY.map(f32::to_bits));
            for word in &mut words[25..28] {
                *word = 1f32.to_bits();
            }
            words[28] = 0x00ff_ffff;
        }
        words[5] = (plane.priority as f32).to_bits();
        words[28] = (words[28] & 0x00ff_ffff) | (u32::from(plane.opacity) << 24);
        if (words[35] != 0) != plane.visible {
            words[35] = u32::from(plane.visible);
        }
        return Ok(words);
    }
    bail!("FES motion property capture has no contract for this object type")
}

fn query_position(
    host: &mut LoggingHost<'_>,
    view: &FesTaskView,
    runtime: &FesRuntime,
    driver: &mut FesScriptDriver,
    command: &FesDriverCommand,
    shared: &mut ExpressionState,
    adjusted: bool,
) -> Result<Option<bool>> {
    let (name, bank) = split(&command.parameters[0], host.text_encoding)?;
    let Some(object) = driver.adapter.object(name) else {
        return Ok(None);
    };
    let media_kind = object.definition.kind_name.as_slice();
    let image = matches!(media_kind, b"IMAGE" | b"CIMAGE" | b"DIFIMAGE" | b"BUTTON");
    let local_plane = matches!(media_kind, b"PLANE" | b"CPLANE" | b"STRING" | b"STRING8");
    if !(image || local_plane && !adjusted) {
        return Ok(None);
    }
    let bank = match bank {
        Some(source) => driver.evaluate_parameter(source, command.context.current, shared)?,
        None if object.definition.bank_count == 1 => 0,
        None => return Ok(Some(true)),
    };
    if bank < 0 || driver.adapter.object_bank(name, bank as usize).is_none() {
        return Ok(Some(true));
    }
    let mut destinations = [-1; 3];
    for (destination, source) in destinations.iter_mut().zip(&command.parameters[2..]) {
        *destination = driver.evaluate_parameter(source, command.context.current, shared)?;
    }
    let id = (name.to_vec(), bank);
    let position = if adjusted {
        let key = runtime_key(&id, runtime, host.text_encoding)?;
        let point = button_image_position(host, view.id, runtime, &key)?;
        [point.x, point.y, 0]
    } else {
        let owner = host.data_services.fes_motion.owners.get(&view.id)
            .filter(|owner| owner.nodes.contains_key(&id))
            .context("FES position query has no live motion object")?;
        let words = capture(host, view.id, owner, &id, runtime, host.text_encoding)?;
        let [x, y, z] = [6, 7, 8].map(|axis| x86_integer(f32::from_bits(words[axis])));
        [x, y, if local_plane { z } else { 0 }]
    };
    for (destination, value) in destinations.into_iter().zip(position) {
        if let Some(slot) = shared.shared.get_mut(destination as usize) {
            *slot = value;
        }
    }
    let name = decode(name, host.text_encoding)?;
    let verb = if adjusted { "getpos2" } else { "getpos" };
    host.events.push(super::LogicEvent {
        frame: u64::from(view.now),
        kind: "fes-media".into(),
        detail: format!("object={name:?} command={verb} targets=1"),
    });
    Ok(Some(true))
}

fn writeback(
    host: &mut LoggingHost<'_>,
    owner_id: u32,
    owner: &mut Owner,
    id: &Id,
    runtime: &mut FesRuntime,
    encoding: ScriptTextEncoding,
    mut words: [u32; 36],
) -> Result<()> {
    match owner.nodes[id].kind {
        Kind::Base => {}
        Kind::Text => {
            super::fes_text::apply_motion_properties(
                &mut host.data_services.fes_text, owner_id, &id.0, id.1, words,
            )?;
            // Full pose lives in the existing controller, including matrix,
            // fractional position and base Z not present in the 2D facade.
            // Direct property writes must not replace its motion baseline.
            owner.nodes.get_mut(id).unwrap().controller.properties = words;
        }
        Kind::Slider | Kind::Other => {
            if !host
                .data_services
                .fes_controls
                .apply_alpha_motion_properties(owner_id, &id.0, id.1, &words)
            {
                bail!("FES motion alpha target has no live control bank");
            }
        }
        Kind::Sound => {
            super::fes_sound::apply_motion_properties(
                &mut host.data_services.fes_sound,
                owner_id,
                &id.0,
                id.1,
                words,
            )?;
        }
        Kind::Image => {
            let key = runtime_key(id, runtime, encoding)?;
            let layer = runtime
                .media
                .get_mut(&key)
                .context("FES motion image was released")?;
            layer.position = Point::new(
                x86_integer(f32::from_bits(words[6])),
                x86_integer(f32::from_bits(words[7])),
            );
            layer.image_base = [
                x86_integer(f32::from_bits(words[3])),
                x86_integer(f32::from_bits(words[4])),
            ];
            let priority = x86_integer(f32::from_bits(words[5]));
            if layer.priority != priority {
                layer.priority = priority.wrapping_add(20);
            }
            layer.opacity = (words[28] >> 24) as u8;
            layer.visible = words[35] != 0;
            words[0..3].fill(0);
            words[8..25].fill(0);
            words[27] = 1f32.to_bits();
            words[28] &= 0xff00_0000;
            words[29] = 0;
            words[30] = 0;
            words[32..35].fill(0);
            layer.motion = Some(words);
        }
        Kind::Plane => {
            let key = runtime_key(id, runtime, encoding)?;
            let plane = runtime
                .planes
                .get_mut(&key)
                .context("FES motion plane was released")?;
            plane.position = Point::new(
                x86_integer(f32::from_bits(words[6])),
                x86_integer(f32::from_bits(words[7])),
            );
            plane.opacity = (words[28] >> 24) as u8;
            plane.visible = words[35] != 0;
            plane.motion = Some(words);
            if !owner.nodes[id].plane_canvas {
                if let Some(layer) = runtime.media.get_mut(&key) {
                    layer.visible = words[35] != 0;
                    layer.opacity = (words[28] >> 24) as u8;
                    let color = Color::rgba(
                        (words[28] >> 16) as u8,
                        (words[28] >> 8) as u8,
                        words[28] as u8,
                        (words[28] >> 24) as u8,
                    );
                    if layer.color != color || layer.image.is_none() {
                        layer.color = color;
                        super::fes_media::rebuild_solid(layer)?;
                    }
                }
            }
        }
    }
    Ok(())
}

fn create_fes_motion_runner(
    program: KcsProgram,
    layout: ExternalLayout,
    name: &[u8],
    parameters: [u32; 256],
    properties: [u32; 36],
) -> cs2_script::Result<MotionRunner> {
    MotionRunner::with_external_layout(MotionProgram::from_program(program)?, MotionStart {
        name, parameters, properties, external_value: 0, base_time: 0,
        saved_data: &[], snapshot_flags: 0,
    }, layout)
}

fn uses_short_fes_motion_layout(pe_sha: Option<&str>) -> bool {
    const UTATE_MEGURI_SHA: &str =
        "a9f8b08713aac278671853b2293a13b2623a122ede9341cb921901b09601cb52";
    matches!(
        pe_sha,
        Some(super::particle_external::CAMPANELLA_SHA | super::css_model::SHA | UTATE_MEGURI_SHA)
    )
}

fn fes_motion_external_layout(
    pe_sha: Option<&str>, archive_name: Option<&str>, member: &str, code: &[u8],
) -> ExternalLayout {
    if !uses_short_fes_motion_layout(pe_sha)
        || !archive_name.is_some_and(|name| name.eq_ignore_ascii_case("mot.int"))
    {
        return ExternalLayout::Default;
    }
    let digest = match member.rsplit(['/', '\\']).next().unwrap_or(member) {
        "motfade.kcs" => "ac0c5bd4bde966f0749bdefcae54164d1bde8a86225977ffb0e2c2d063b2755c",
        "motfade2.kcs" => "5e68e41efb92c31ea3b592b91b9c03dc6ca16e15d6123f906aac6d01db1afdf3",
        "motmove.kcs" => "f0c7cc2432f9fd07da58233610c9a4128fce3c228fca126826bdcdd4a1d4b039",
        "motrot.kcs" => "f6a23ac5449351fe0abfe6be05c1a20e9f492f06c4a86b2fffebc3e8759ea75e",
        "motscale.kcs" => "d461e36f2f52481330c66cb517fb62dc7c909bcf0a65733509330f035b53f886",
        "mcfade.kcs" => "8eebe432198f723e930205237fa80a4d016a1d39cbbe506f148cc1465f395a17",
        "mcmove.kcs" => "eb626eae7b4de39214dcc13391d7b39547859684f4604dc7f49b3f12702fd63b",
        "mcpend.kcs" if pe_sha == Some(super::css_model::SHA) => {
            "5d29c4b32edc46fb15fbcbbe088a430a953a0b0e737c3532ec26f2d45a997b7d"
        }
        "mcrot.kcs" => "2550e754e0fac92fcd380c22503e04084f990433303f49d163ffee1dbce2645b",
        "mcscalecos.kcs" => "d8cd5f1df6cd30dd281710d94103b8bfb16709adff1729359529fe71816683d7",
        "mcscalesin.kcs" => "ad5fcc72b72d7a85a9c4395143c4071d5148a51757f8daa2800d647f0a733cba",
        "pbase.kcs" => "171e58f8afd181083a48f3e5ed12982b7568fa7d4b111161d84ab900751c0c7b",
        "pcolor.kcs" => "0559f4afdf8dbf0b89cc8c292ed75dd64f691ecfed40a62711b7fe092a0aba9a",
        "pdepth.kcs" => "dafa40f545276cf893411f40f9a893a3048452d0372e55e88adf46f29c2ccb06",
        "ppos.kcs" => "73a23866f2f5c2d3129a174da3113d3d43f9e0acab68f150c4cebdbd972866e2",
        "prot.kcs" => "c163f9c82795d91ec177a9703016d3b735ce5c4efbd6025439d5a673767c8efd",
        "pscale.kcs" => "8eb68956e3ce9a9d7a307ba4e43009a418149c7f28f4486f3e0b8ff10fbbd749",
        "pvram.kcs" => "6b9531f7ced26052a7b2d73c57e422a5bec7087144e28b0543b51467ec629778",
        _ => return ExternalLayout::Default,
    };
    if super::particle_plugin_digest::sha256(code) == digest {
        ExternalLayout::Campanella
    } else {
        ExternalLayout::Default
    }
}

fn load_program(
    host: &LoggingHost<'_>,
    owner: &mut Owner,
    raw_name: &[u8],
) -> Result<Option<(KcsProgram, ExternalLayout)>> {
    if !owner.resources.contains_key(raw_name) {
        owner.resources.insert(
            raw_name.to_vec(),
            host.resources
                .category_names(b"mot", &decode(raw_name, host.text_encoding)?)?,
        );
    }
    let pe_sha = host.data_services.css_task_pe_sha256;
    for name in &owner.resources[raw_name] {
        let key = name.to_lowercase();
        let layout = |program: &KcsProgram, location: &ResourceLocation| {
            let archive = match location {
                ResourceLocation::Archive { path, .. } => path.file_name().and_then(|name| name.to_str()),
                ResourceLocation::File { .. } => None,
            };
            fes_motion_external_layout(pe_sha, archive, &key, &program.code)
        };
        if let Some(program) = owner.programs.get(&key) {
            let external = if uses_short_fes_motion_layout(pe_sha) {
                host.resources.resolve(name)?.map_or(ExternalLayout::Default, |resource| {
                    layout(program, &resource.location)
                })
            } else {
                ExternalLayout::Default
            };
            return Ok(Some((program.clone(), external)));
        }
        if let Some(resource) = host.resources.resolve(name)? {
            if resource.size > MAX_BYTES {
                bail!("FES motion resource exceeds 64 MiB");
            }
            let program = KcsProgram::parse(&host.resources.read_candidate(resource)?)?;
            let external = layout(&program, &resource.location);
            owner.programs.insert(key, program.clone());
            return Ok(Some((program, external)));
        }
    }
    Ok(None)
}

fn layer_key(id: &Id, driver: &FesScriptDriver, encoding: ScriptTextEncoding) -> Result<String> {
    let name = decode(&id.0, encoding)?;
    Ok(
        if driver
            .adapter
            .object(&id.0)
            .is_some_and(|object| object.definition.bank_count == 1)
            && id.1 == 0
        {
            name
        } else {
            format!("{name}[{}]", id.1)
        },
    )
}

fn runtime_key(id: &Id, runtime: &FesRuntime, encoding: ScriptTextEncoding) -> Result<String> {
    let name = decode(&id.0, encoding)?;
    let banked = format!("{name}[{}]", id.1);
    if runtime.media.contains_key(&banked) || runtime.planes.contains_key(&banked) {
        Ok(banked)
    } else if id.1 == 0 {
        Ok(name)
    } else {
        bail!("FES motion object bank has no live media binding")
    }
}

fn decode(bytes: &[u8], encoding: ScriptTextEncoding) -> Result<String> {
    let codec = match encoding {
        ScriptTextEncoding::Cp932 => encoding_rs::SHIFT_JIS,
        ScriptTextEncoding::Gbk => encoding_rs::GBK,
        ScriptTextEncoding::Utf8 => encoding_rs::UTF_8,
    };
    let (text, invalid) = codec.decode_without_bom_handling(bytes);
    if invalid {
        bail!("FES motion name is invalid in the selected script encoding");
    }
    Ok(text.into_owned())
}

fn split(bytes: &[u8], encoding: ScriptTextEncoding) -> Result<(&[u8], Option<&[u8]>)> {
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'[' {
            if !bytes.ends_with(b"]") {
                bail!("FES motion target bank is not closed");
            }
            return Ok((&bytes[..index], Some(&bytes[index + 1..bytes.len() - 1])));
        }
        let lead = match encoding {
            ScriptTextEncoding::Cp932 => matches!(bytes[index], 0x81..=0x9f | 0xe0..=0xfc),
            ScriptTextEncoding::Gbk => matches!(bytes[index], 0x81..=0xfe),
            ScriptTextEncoding::Utf8 => false,
        };
        index += if lead { 2 } else { 1 };
    }
    Ok((bytes, None))
}

pub(super) fn release_owner(host: &mut LoggingHost<'_>, owner: u32) {
    host.data_services.fes_motion.owners.remove(&owner);
}

#[derive(Clone, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(super) struct MotionReleaseIdentity {
    pub(super) owner: u32,
    pub(super) name: Vec<u8>,
    pub(super) bank: i32,
    serial: u64,
}

/// Capture the complete child-first identity order without removing a node.
/// The serial prevents a resumed delete from releasing a reused name.
pub(super) fn plan_release_subtree(
    host: &mut LoggingHost<'_>, owner_id: u32, name: &[u8], bank: i32,
) -> Result<Vec<MotionReleaseIdentity>> {
    let parents = host.data_services.fes_controls.object_parent_links(owner_id);
    let Some(owner) = host.data_services.fes_motion.owners.get_mut(&owner_id) else {
        return Ok(Vec::new());
    };
    for target in parents {
        if let Some(node) = owner.nodes.get_mut(&(target.name, target.bank)) {
            node.parent = target.parent;
        }
    }
    let key = (name.to_vec(), bank);
    if !owner.nodes.contains_key(&key) { return Ok(Vec::new()); }
    Ok(subtree(owner, &key)?.into_iter().map(|id| MotionReleaseIdentity {
        owner: owner_id, serial: owner.nodes[&id].serial, name: id.0, bank: id.1,
    }).collect())
}

pub(super) fn validate_release_identity(
    store: &FesMotionStore, identity: &MotionReleaseIdentity,
) -> Result<bool> {
    let Some(owner) = store.owners.get(&identity.owner) else { return Ok(false); };
    let key = (identity.name.clone(), identity.bank);
    let Some(node) = owner.nodes.get(&key) else { return Ok(false); };
    if node.serial != identity.serial {
        bail!("FES delete identity was replaced before its release completed");
    }
    if owner.nodes.values().any(|node| node.parent.as_ref() == Some(&key)) {
        bail!("FES delete must release every child before its parent");
    }
    Ok(true)
}

pub(super) fn commit_release_identity(
    store: &mut FesMotionStore, identity: &MotionReleaseIdentity,
) -> Result<()> {
    if !validate_release_identity(store, identity)? { return Ok(()); }
    let owner = store.owners.get_mut(&identity.owner).unwrap();
    remove_motion_nodes(owner, &BTreeSet::from([(identity.name.clone(), identity.bank)]));
    Ok(())
}

fn remove_motion_nodes(owner: &mut Owner, deleted: &BTreeSet<Id>) {
    owner.commands.retain(|_, command| !command.keys.iter().any(|id| deleted.contains(id)));
    owner.sound_blends.retain(|(_, id), _| !deleted.contains(id));
    for keys in owner.waits.values_mut().flatten() {
        keys.retain(|id| !deleted.contains(id));
    }
    for update in owner.update.iter_mut().chain(
        owner.commands.values_mut().filter_map(|command| command.stopping.as_mut()),
    ) {
        if update.order.get(update.cursor).is_some_and(|id| deleted.contains(id)) {
            update.writeback = None;
        }
        let removed_before = update.order.iter().take(update.cursor)
            .filter(|id| deleted.contains(*id)).count();
        update.order.retain(|id| !deleted.contains(id));
        update.cursor -= removed_before;
    }
    owner.nodes.retain(|id, _| !deleted.contains(id));
    owner.initial_parents.retain(|_, parent| {
        !parent.as_ref().is_some_and(|id| deleted.contains(id))
    });
}

/// SOUND destruction releases object motions, never its shared physical
/// channel. Validate the whole subtree before dropping any controller.
pub(super) fn release_sound_bank(
    store: &mut FesMotionStore,
    owner_id: u32,
    name: &[u8],
    bank: i32,
) -> Result<Vec<(Vec<u8>, i32)>> {
    let Some(owner) = store.owners.get_mut(&owner_id) else {
        return Ok(Vec::new());
    };
    let key = (name.to_vec(), bank);
    if !owner.nodes.contains_key(&key) {
        return Ok(Vec::new());
    }
    let order = subtree(owner, &key)?;
    if order.iter().any(|id| owner.nodes[id].kind != Kind::Sound) {
        bail!("SOUND delete has a non-SOUND descendant; no motion objects were released");
    }
    let deleted: BTreeSet<_> = order.iter().cloned().collect();
    remove_motion_nodes(owner, &deleted);
    Ok(order)
}

/// Dynamic DRAW has the default BASE property layout, but is a real child of
/// its selected PLANE. The ordinary FES boundary is its only motion updater.
pub(super) fn attach_draw_motion(
    host: &mut LoggingHost<'_>,
    owner_id: u32,
    name: &[u8],
    parent: (Vec<u8>, i32),
    program: Option<KcsProgram>,
) -> Result<()> {
    let owner = host.data_services.fes_motion.owners.get_mut(&owner_id)
        .context("FES DRAW has no shared motion owner")?;
    let key = (name.to_vec(), 0);
    if owner.nodes.contains_key(&key) {
        bail!("FES DRAW still has an earlier common motion identity");
    }
    if !owner.nodes.contains_key(&parent) {
        bail!("FES DRAW parent has no common PLANE identity");
    }
    if owner.nodes.len() >= MAX_NODES {
        bail!("FES DRAW exceeds the common motion object limit");
    }
    let serial = owner.next_serial;
    let next_serial = serial.checked_add(1).context("FES DRAW object order overflow")?;
    let mut controller = MotionControllerState::default();
    if let Some(program) = program {
        let parameters = [0; 256];
        let runner = create_motion_runner(
            program, b"dummy", &parameters, &[], 0, controller.base_properties, 0,
        )?;
        controller.tasks.insert(1, MotionTaskState {
            name: b"dummy".to_vec(), group: 0, parameters,
            saved_data: Vec::new(), snapshot_flags: 0, external_value: 0,
            origin: 0, elapsed: 0, running: 1, finishing: 0, runner,
        });
        controller.next_task_id = 2;
    }
    owner.nodes.insert(key, Node {
        kind: Kind::Base, plane_canvas: false, parent: Some(parent), serial,
        delay: 0, controller,
    });
    owner.next_serial = next_serial;
    Ok(())
}

pub(super) fn draw_motion_active(host: &LoggingHost<'_>, owner_id: u32) -> Result<bool> {
    let owner = host.data_services.fes_motion.owners.get(&owner_id)
        .context("FES DRAW common motion owner disappeared")?;
    let key = (DRAW_NAME.to_vec(), 0);
    if !owner.nodes.contains_key(&key) {
        bail!("FES DRAW common motion identity disappeared");
    }
    Ok(subtree(owner, &key)?.iter().any(|id| active_node(&owner.nodes[id])))
}

pub(super) fn owner_motion_active(host: &LoggingHost<'_>, owner_id: u32) -> bool {
    host.data_services.fes_motion.owners.get(&owner_id)
        .is_some_and(|owner| owner.nodes.values().any(active_node))
}

pub(super) fn detach_draw_motion(host: &mut LoggingHost<'_>, owner_id: u32, name: &[u8]) {
    let Some(owner) = host.data_services.fes_motion.owners.get_mut(&owner_id) else {
        return;
    };
    let key = (name.to_vec(), 0);
    owner.nodes.remove(&key);
    // A wait keeps the old identity; a later DRAW with the same temporary name
    // must not keep that completed wait alive.
    for keys in owner.waits.values_mut().flatten() {
        keys.retain(|id| id != &key);
    }
}

pub(super) fn plane_render_properties(
    host: &LoggingHost<'_>,
    view: &FesTaskView,
    runtime: &FesRuntime,
    name: &str,
) -> Result<[u32; 36]> {
    let owner = host
        .data_services
        .fes_motion
        .owners
        .get(&view.id)
        .context("FES plane has no common property owner")?;
    let id = owner
        .nodes
        .keys()
        .find(|id| {
            owner.nodes[*id].kind == Kind::Plane
                && runtime_key(id, runtime, host.text_encoding).ok().as_deref() == Some(name)
        })
        .context("FES plane has no declaration binding")?;
    let plane = &runtime.planes[name];
    let mut words = capture(host, view.id, owner, id, runtime, host.text_encoding)?;
    words[5] = plane.motion.map_or(0, |properties| properties[5]);
    let mut positions = vec![[0, 1, 2].map(|axis| f32::from_bits(words[6 + axis]))];
    let mut bases = vec![[f32::from_bits(words[3]), f32::from_bits(words[4])]];
    let mut parent = owner.nodes[id].parent.as_ref();
    let mut add_base = true;
    let mut seen = BTreeSet::new();
    while let Some(id) = parent {
        if !seen.insert(id.clone()) || seen.len() > 256 {
            bail!("FES render plane parent cycle");
        }
        let Some(node) = owner.nodes.get(id).filter(|node| node.kind == Kind::Plane) else {
            break;
        };
        let properties = capture(host, view.id, owner, id, runtime, host.text_encoding)?;
        positions.push([0, 1, 2].map(|axis| f32::from_bits(properties[6 + axis])));
        if add_base && node.plane_canvas {
            bases.push([f32::from_bits(properties[3]), f32::from_bits(properties[4])]);
        } else {
            add_base = false;
        }
        if properties[35] == 0 {
            words[35] = 0;
        }
        parent = node.parent.as_ref();
    }
    for axis in 0..3 {
        words[6 + axis] = positions
            .iter()
            .rev()
            .fold(0f32, |sum, position| sum + position[axis])
            .to_bits();
    }
    for axis in 0..2 {
        words[3 + axis] = bases
            .iter()
            .rev()
            .fold(0f32, |sum, base| sum + base[axis])
            .to_bits();
    }
    Ok(words)
}

/// Keep integral image translations as real list entries. Other image poses
/// are projected into the bound list; lower plane matrices are consumed once
/// by the plane drawable rather than baked into each image.
pub(super) fn image_list_placement(
    host: &LoggingHost<'_>,
    view: &FesTaskView,
    runtime: &FesRuntime,
    name: &str,
    frame: &RgbaFrame,
) -> Result<super::list_native::AffinePlacement> {
    let layer = &runtime.media[name];
    let parent = inherited_image_position(host, view.id, runtime, name)?;
    let animation = layer.animation_display.map_or([0; 2], |display| [display.x, display.y]);
    let vram = layer.plane.as_deref().and_then(|name| runtime.driver_plane_key(name))
        .filter(|name| !runtime.media.contains_key(*name))
        .map_or([0; 2], |name| runtime.planes[name].vram);
    let mut position = [layer.position.x, layer.position.y].map(|value| value as f32);
    let mut base = layer.image_base.map(|value| value as f32);
    let mut offset = layer.image_offset;
    let mut scale = [1.; 2];
    let mut rotation = 0.;
    if let Some(words) = layer.motion.as_ref() {
        for axis in 0..2 {
            let motion = f32::from_bits(words[6 + axis]);
            position[axis] += motion - x86_integer(motion) as f32;
            base[axis] = f32::from_bits(words[3 + axis]);
            scale[axis] = f32::from_bits(words[25 + axis]);
        }
        rotation = f32::from_bits(words[31]);
    }
    let metadata = frame.metadata();
    let raw_frame = layer.image.is_none() && layer.source.has_raw_source();
    for axis in 0..2 {
        position[axis] += parent[axis] + animation[axis] as f32 + vram[axis] as f32;
        if raw_frame {
            base[axis] += metadata.base[axis] as f32;
        }
        if raw_frame || layer.image.is_some() {
            offset[axis] = offset[axis].wrapping_add(metadata.offset[axis]);
        }
    }
    Ok(super::list_native::AffinePlacement { position, base, offset, scale, rotation })
}

pub(super) fn prepare_projection(
    host: &LoggingHost<'_>,
    view: &FesTaskView,
    runtime: &FesRuntime,
    name: &str,
    image: &RgbaFrame,
    mut options: ImageDrawOptions,
) -> Result<(RgbaFrame, ImageDrawOptions)> {
    let layer = &runtime.media[name];
    let parent_position = inherited_image_position(host, view.id, runtime, name)?;
    options.position.x = options
        .position
        .x
        .wrapping_add(x86_integer(parent_position[0]));
    options.position.y = options
        .position
        .y
        .wrapping_add(x86_integer(parent_position[1]));
    // Plane references select VRAM in driver_media_draw_options. Their
    // world transforms are consumed by the actual lower drawable once.
    if layer.motion.is_none() {
        return Ok((image.clone(), options));
    }
    if let Some(position) = integral_projection_position(&options, layer.motion.as_ref()) {
        options.position = position;
        options.pivot = [0.; 2];
        return Ok((image.clone(), options));
    }
    if options.blend_mode.is_list_alpha_mask() {
        return Ok((image.clone(), options));
    }
    let Some(binding) = &view.list else {
        return Ok((image.clone(), options));
    };
    let list = host
        .data_services
        .tasks
        .fes
        .platform
        .lists
        .get(&binding.task)
        .context("motion projection needs the actual bound image list")?;
    if u64::from(list.size[0]) * u64::from(list.size[1]) * 4 > MAX_BYTES {
        bail!("FES motion projection exceeds 64 MiB");
    }
    // Geometry sampling must not consume the list's global opacity. Keeping
    // source alpha here lets the final list use its documented integer lookup
    // once, including alpha=1 at half opacity. Special modes keep their own
    // existing raster path because they depend on the intermediate target.
    let list_opacity = if options.blend_mode == cs2_renderer::BlendMode::SourceOver
        && options.mask.is_none()
    {
        let opacity = options.opacity;
        options.opacity = 255;
        opacity
    } else {
        255
    };
    let mut canvas = Canvas::new(list.size[0], list.size[1])?;
    super::plane_native::draw_fes_motion_frame(
        &mut canvas,
        image,
        &options,
        layer.motion.as_ref(),
        None,
        None,
    )?;
    let frame = RgbaFrame::from_rgba8(list.size[0], list.size[1], canvas.pixels().to_vec())?;
    let mut projected = ImageDrawOptions::new(Point::new(0, 0));
    projected.opacity = list_opacity;
    Ok((frame, projected))
}

fn integral_projection_position(
    options: &ImageDrawOptions,
    words: Option<&[u32; 36]>,
) -> Option<Point> {
    if options.scale_x != 1.
        || options.scale_y != 1.
        || options.rotation_radians != 0.
        || options.flip_x
        || options.flip_y
    {
        return None;
    }
    if let Some(words) = words {
        for axis in 0..2 {
            let value = f32::from_bits(words[6 + axis]);
            if !value.is_finite() || value != x86_integer(value) as f32 {
                // The general projection retains this fractional correction.
                return None;
            }
        }
    }
    let mut point = [0; 2];
    for axis in 0..2 {
        let pivot = options.pivot[axis];
        if !pivot.is_finite() || pivot != x86_integer(pivot) as f32 {
            return None;
        }
        let position = [options.position.x, options.position.y][axis];
        point[axis] = i32::try_from(i64::from(position) - i64::from(x86_integer(pivot))).ok()?;
    }
    Some(Point::new(point[0], point[1]))
}

fn inherited_image_position(
    host: &LoggingHost<'_>,
    owner_id: u32,
    runtime: &FesRuntime,
    name: &str,
) -> Result<[f32; 2]> {
    let Some(owner) = host.data_services.fes_motion.owners.get(&owner_id) else {
        return Ok([0.; 2]);
    };
    let Some(id) = owner
        .nodes
        .keys()
        .find(|id| runtime_key(id, runtime, host.text_encoding).ok().as_deref() == Some(name))
    else {
        return Ok([0.; 2]);
    };
    let mut values = Vec::new();
    let mut parent = owner.nodes[id].parent.as_ref();
    let mut visited = BTreeSet::new();
    while let Some(id) = parent {
        if !visited.insert(id.clone()) || visited.len() > 256 {
            bail!("FES image parent cycle");
        }
        let Some(node) = owner.nodes.get(id).filter(|node| node.kind == Kind::Image) else {
            break;
        };
        let key = runtime_key(id, runtime, host.text_encoding)?;
        let layer = &runtime.media[&key];
        if !matches!(layer.kind.as_str(), "IMAGE" | "CIMAGE" | "DIFIMAGE") {
            break;
        }
        let words = capture(host, owner_id, owner, id, runtime, host.text_encoding)?;
        values.push([0, 1].map(|axis| {
            layer.image_offset[axis] as f32 + f32::from_bits(words[6 + axis])
                - f32::from_bits(words[3 + axis])
        }));
        parent = node.parent.as_ref();
    }
    Ok(values.iter().rev().fold([0f32; 2], |sum, value| {
        [sum[0] + value[0], sum[1] + value[1]]
    }))
}

pub(super) fn input_position(
    host: &LoggingHost<'_>,
    owner: u32,
    runtime: &FesRuntime,
    name: &str,
    position: Point,
) -> Result<Point> {
    let parent = inherited_image_position(host, owner, runtime, name)?;
    Ok(Point::new(
        position.x.wrapping_add(x86_integer(parent[0])),
        position.y.wrapping_add(x86_integer(parent[1])),
    ))
}

/// BUTTON input uses the effective screen position before image rotation or
/// scale. PLANE VRAM selects texture pixels and contributes no screen offset.
pub(super) fn button_input_position(
    host: &LoggingHost<'_>,
    view: &FesTaskView,
    runtime: &FesRuntime,
    name: &str,
) -> Result<Point> {
    let mut point = button_image_coordinates(host, view.id, runtime, name)?;
    let layer = &runtime.media[name];
    if let Some(plane) = layer.plane.as_deref().and_then(|name| runtime.driver_plane_key(name))
        .filter(|name| !runtime.media.contains_key(*name)) {
        let words = plane_render_properties(host, view, runtime, plane)?;
        for axis in 0..2 {
            point[axis] = point[axis] + f32::from_bits(words[6 + axis]) - f32::from_bits(words[3 + axis]);
        }
    }
    Ok(Point::new(x86_integer(point[0]), x86_integer(point[1])))
}

/// Direction navigation queries image coordinates before the associated plane.
pub(crate) fn button_image_position(
    host: &LoggingHost<'_>, owner: u32, runtime: &FesRuntime, name: &str,
) -> Result<Point> {
    let point = button_image_coordinates(host, owner, runtime, name)?;
    Ok(Point::new(x86_integer(point[0]), x86_integer(point[1])))
}

fn button_image_coordinates(
    host: &LoggingHost<'_>, owner: u32, runtime: &FesRuntime, name: &str,
) -> Result<[f32; 2]> {
    if let Some(position) = super::fes_task_platform::button_locked_position(host, owner, name) {
        let parent = inherited_image_position(host, owner, runtime, name)?;
        return Ok([0, 1].map(|axis| position[axis] + parent[axis]));
    }
    button_unlocked_coordinates(host, owner, runtime, name)
}

pub(super) fn button_unlocked_coordinates(
    host: &LoggingHost<'_>, owner: u32, runtime: &FesRuntime, name: &str,
) -> Result<[f32; 2]> {
    let layer = runtime
        .media
        .get(name)
        .context("BUTTON image properties are absent")?;
    let parent = inherited_image_position(host, owner, runtime, name)?;
    let animation = layer
        .animation_display
        .map_or([0; 2], |display| [display.x, display.y]);
    let position = layer.motion.map_or(
        [layer.position.x as f32, layer.position.y as f32],
        |words| [f32::from_bits(words[6]), f32::from_bits(words[7])],
    );
    let base = layer
        .motion
        .map_or(layer.image_base.map(|value| value as f32), |words| {
            [f32::from_bits(words[3]), f32::from_bits(words[4])]
        });
    Ok([0, 1].map(|axis| {
        layer.image_offset[axis] as f32 + position[axis] - base[axis]
            + animation[axis] as f32
            + parent[axis]
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{HeadlessPlayer, fes_services};
    use cs2_script::{FesDocument, FesLine, FesSection};

    #[test]
    fn tsunaban_mcpend_maps_all_four_motion_words() {
        assert_eq!(
            mcpend_parameters([150, 0, 0, 2]),
            [150.0_f32.to_bits(), 0, 0, 2.0_f32.to_radians().to_bits()],
        );
        assert_eq!(
            mcpend_parameters([-1, 90, -180, 45]),
            [0, std::f32::consts::FRAC_PI_2.to_bits(), (-std::f32::consts::PI).to_bits(), std::f32::consts::FRAC_PI_4.to_bits()],
        );
    }

    #[test]
    fn imgwnd_stop_reuses_shared_motion_for_indexed_and_all_banks() -> Result<()> {
        let unique = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?;
        let directory = std::env::temp_dir().join(format!(
            "cs2-fes-imgwnd-stop-{}-{}", std::process::id(), unique.as_nanos(),
        ));
        std::fs::create_dir(&directory)?;
        let result = HeadlessPlayer::open_with_save_root(&directory, directory.join("profile"))
            .and_then(|mut player| player.with_host(|host| {
                let mut offset = 0;
                let mut section = |header: &[u8], lines: &[&[u8]]| FesSection {
                    header: header.to_vec(),
                    lines: lines.iter().map(|bytes| {
                        let line = FesLine { offset, bytes: bytes.to_vec() };
                        offset += bytes.len() + 1;
                        line
                    }).collect(),
                };
                let document = FesDocument { sections: vec![
                    section(b"DEFINE", &[b"IMGWND window[2]", b"VIEWPORT clip"]),
                    section(b"START", &[b"window[1] stop", b"window stop", b"clip stop"]),
                ] };
                let mut driver = FesScriptDriver::new(&document)?;
                let mut runtime = FesRuntime::new(document);
                let mut shared = ExpressionState::default();
                let id = host.create_task_object(34, 0, b"imgwnd stop test", b"")?;
                let view = FesTaskView { id, parent: 0, context: Vec::new(), list: None, now: 50, paused: 0 };
                while let Some(effect) = driver.advance_initialization(&mut shared)? {
                    if matches!(&effect, FesDriverEffect::Initialize { event: FesEvent::CreateObject(_), .. }) {
                        assert_eq!(fes_services::apply_effect(host, &view, &mut runtime, &mut driver, &effect, &mut shared)?, Some(true));
                    }
                }

                let first = (b"window".to_vec(), 0);
                let second = (b"window".to_vec(), 1);
                let child = (b"child".to_vec(), 0);
                let program = KcsProgram::from_code(&[0x3b], vec![0; 16], 16)?;
                attach_draw_motion(host, id, &child.0, second.clone(), Some(program))?;
                let nodes = &mut host.data_services.fes_motion.owners.get_mut(&id).unwrap().nodes;
                nodes.get_mut(&first).unwrap().delay = 10;
                nodes.get_mut(&second).unwrap().delay = 20;
                let child_node = nodes.get_mut(&child).unwrap();
                child_node.delay = 30;
                child_node.controller.tasks.get_mut(&1).unwrap().origin = u32::MAX - 1;

                for (expected, first_delay) in [(Some(true), 10), (Some(true), 0), (None, 0)] {
                    let effect = driver.advance(u64::from(view.now), &mut shared, 64)?;
                    assert!(matches!(&effect, FesDriverEffect::Command(_)), "{effect:?}");
                    assert_eq!(fes_services::apply_effect(host, &view, &mut runtime, &mut driver, &effect, &mut shared)?, expected);
                    let nodes = &host.data_services.fes_motion.owners[&id].nodes;
                    assert_eq!(nodes[&first].delay, first_delay);
                    assert_eq!(nodes[&second].delay, 0);
                    assert_eq!(nodes[&child].delay, 30);
                    let task = &nodes[&child].controller.tasks[&1];
                    assert_eq!((task.finishing, task.elapsed, task.running), (1, 2, 1));
                }
                Ok(())
            }));
        std::fs::remove_dir(&directory)?;
        result
    }
}

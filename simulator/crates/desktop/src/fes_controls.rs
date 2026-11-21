//! VIEWPORT, IMGWND and numeric slider controls for the FES extension chain.
//! Pixel projections use independent list keys; text remains owned by fes_text.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use anyhow::{Context, Result, bail};
use cs2_formats::HgFile;
use cs2_renderer::{Canvas, Color, FrameMetadata, ImageDrawOptions, Point, Rect, RgbaFrame};
use cs2_runtime::fes_controls::{
    ControlDirection, MetadataReference, Scroll2Control, SliderControl, ViewportState,
};
use cs2_runtime::{RuntimeError, RuntimeResult, TaskCommand};
use cs2_script::{ExpressionState, FesEvent, FesObjectDefinition, FesObjectInitialization};
use cs2_runtime::graph_serde::{Serialize, Deserialize};

use super::fes_driver::{
    FesDriverCommand, FesDriverEffect, FesDriverInput, FesDriverTarget, FesScriptDriver,
};
use super::fes_tasks::FesTaskView;
use super::{FesRuntime, HeadlessPlayer, InputRegionState, LoggingHost, ScriptTextEncoding};

type Key = (u32, Vec<u8>, i32);
const MAX_BANKS: usize = 65_536;
const MAX_IMAGE: u64 = 64 * 1024 * 1024;

#[path = "fes_controls_state.rs"]
mod state;
#[path = "fes_controls_legacy_scroll.rs"]
mod legacy_scroll;

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
enum Kind {
    Viewport,
    Window,
    Slider,
    Scroll2,
    Scroll,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct Reference {
    name: Vec<u8>,
    bank: i32,
}

#[derive(Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct Bank {
    kind: Kind,
    #[serde(with = "state::viewport")]
    viewport: ViewportState,
    file: Vec<u8>,
    file_resolved: bool,
    expanded_file: Option<Vec<u8>>,
    plane: Option<Reference>,
    parent: Option<Reference>,
    position: [i32; 2],
    base: [i32; 2],
    size: [i32; 2],
    #[serde(with = "state::metadata")]
    size_source: Option<MetadataReference>,
    #[serde(with = "state::metadata")]
    position_source: Option<MetadataReference>,
    priority: i32,
    display: u32,
    enabled: u32,
    applied_enabled: Option<u32>,
    alpha: u32,
    alpha_pending: bool,
    frame_number: i32,
    frames: [u32; 32],
    mapped: bool,
    file_dirty: bool,
    size_dirty: bool,
    visual_dirty: bool,
    projection: Option<ControlProjection>,
    // Immutable HG metadata is rebuilt from these same retained bytes during
    // prepare, without resource lookup or replaying a control update.
    #[serde(skip)]
    source: Option<HgFile>,
    source_bytes: Vec<u8>,
    #[serde(with = "state::slider")]
    slider: SliderControl,
    #[serde(with = "state::scroll")]
    scroll: Scroll2Control,
    rectangle: [i32; 4],
    direction: u32,
    state: usize,
    lower: Option<u32>,
    buttons: legacy_scroll::Buttons,
    drag_origin: Option<[i32; 2]>,
    #[serde(with = "crate::fes_tasks::state::frame")]
    control_image: Option<RgbaFrame>,
    initial_region: bool,
    requested_position: i32,
    requested_range: [i32; 2],
    requested_page: Option<i32>,
    position_dirty: bool,
    range_pending: bool,
    list_override: Option<u32>,
    plane_offset: Option<[i32; 2]>,
}

impl Bank {
    fn new(kind: Kind) -> Self {
        let mut frames = [0; 32];
        frames[..4].copy_from_slice(&[0, 0, 2, 3]);
        Self {
            kind,
            viewport: ViewportState::default(),
            file: Vec::new(),
            file_resolved: false,
            expanded_file: None,
            plane: None,
            parent: None,
            position: [0; 2],
            base: [0; 2],
            size: [0; 2],
            size_source: None,
            position_source: None,
            priority: if matches!(kind, Kind::Slider | Kind::Scroll2 | Kind::Scroll) {
                10
            } else {
                20
            },
            display: 0,
            enabled: 1,
            applied_enabled: None,
            alpha: 255,
            alpha_pending: false,
            frame_number: 0,
            frames,
            mapped: false,
            file_dirty: false,
            size_dirty: false,
            visual_dirty: false,
            projection: None,
            source: None,
            source_bytes: Vec::new(),
            slider: SliderControl::default(),
            scroll: Scroll2Control::default(),
            rectangle: [0; 4],
            direction: 0,
            state: 0,
            lower: None,
            buttons: legacy_scroll::Buttons::default(),
            drag_origin: None,
            control_image: None,
            initial_region: false,
            requested_position: 0,
            requested_range: [0; 2],
            requested_page: None,
            position_dirty: false,
            range_pending: false,
            list_override: None,
            plane_offset: None,
        }
    }

    fn is_slider(&self) -> bool {
        self.kind == Kind::Slider || self.kind.is_scroll()
    }
    fn button_regions(&self) -> impl Iterator<Item = u32> + '_ {
        self.buttons.primary.into_iter().chain(self.buttons.arrows).flatten()
    }
    fn has_region(&self, region: u32) -> bool {
        self.button_regions().any(|id| id == region)
    }
    fn position_value(&self) -> i32 {
        self.requested_position
    }
    fn lower_value(&self) -> i32 {
        if self.kind.is_scroll() {
            self.scroll.position
        } else {
            self.slider.position
        }
    }
    fn pixel(&self) -> i32 {
        if self.kind.is_scroll() {
            self.scroll.pixel
        } else {
            self.slider.pixel
        }
    }
    fn target(&self, name: &[u8], bank: i32) -> FesDriverTarget {
        FesDriverTarget {
            name: name.to_vec(),
            bank,
            default_group: None,
            value: self.lower.map_or(-1, |id| id as i32),
        }
    }
}

impl Kind {
    fn is_scroll(self) -> bool {
        matches!(self, Self::Scroll2 | Self::Scroll)
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(super) struct ControlProjection {
    #[serde(with = "state::frame")]
    pub(super) frame: RgbaFrame,
    #[serde(with = "state::options")]
    pub(super) options: ImageDrawOptions,
    pub(super) priority: i32,
    pub(super) owner: u32,
    pub(super) list: Option<u32>,
    name: Vec<u8>,
    bank: i32,
}

/// FES identity and parent links for the common motion traversal. Lower task
/// numbers do not identify nodes in the FES object tree.
#[derive(Clone, Debug)]
pub(super) struct SliderMotionTarget {
    pub name: Vec<u8>,
    pub bank: i32,
    pub parent: Option<(Vec<u8>, i32)>,
}

impl ControlProjection {
    pub(super) fn layer_key(&self) -> String {
        control_layer_key(&self.name, self.bank)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct Consumer {
    reference: Reference,
    task: Option<u32>,
    last: Option<[i32; 4]>,
    applied: bool,
    layer_key: String,
}

#[derive(Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct Initializer {
    keys: Vec<Key>,
    cursor: usize,
    fields: Vec<usize>,
    priority_bank: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct PreparedCommand {
    keys: Vec<Key>,
    verb: Vec<u8>,
    arguments: Vec<i32>,
    raw: Option<Vec<u8>>,
    cursor: usize,
    bank_values: Vec<i32>,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(super) struct FesControlStore {
    banks: BTreeMap<Key, Bank>,
    declarations: BTreeSet<(u32, usize)>,
    creations: BTreeMap<(u32, usize), usize>,
    initializers: BTreeMap<(u32, usize), Initializer>,
    commands: BTreeMap<(u32, usize), PreparedCommand>,
    consumers: BTreeMap<Key, Consumer>,
    text_viewport_initializers: BTreeMap<(u32, usize), usize>,
    pending_inputs: VecDeque<(u32, FesDriverInput)>,
}

impl FesControlStore {
    pub(crate) fn check_graph_boundary(&self) -> Result<()> {
        if !self.banks.is_empty() || !self.declarations.is_empty() || !self.creations.is_empty()
            || !self.initializers.is_empty() || !self.commands.is_empty() || !self.consumers.is_empty()
            || !self.text_viewport_initializers.is_empty() || !self.pending_inputs.is_empty()
        { bail!("FES viewport/slider owner checkpoints are not yet supported"); }
        Ok(())
    }
}

pub(super) struct ControlNotification {
    pub owner: u32,
    pub command: TaskCommand,
    pub event: FesDriverInput,
}

pub(super) enum LowerControlOutcome {
    Unhandled,
    Complete,
    /// The task adapter must finish refresh_lower before acknowledging this
    /// message; source reads, geometry and pixels have not yet been submitted.
    Refresh {
        owner: u32,
    },
    /// Complete the synchronous owner callback before returning to the sender.
    Notify(ControlNotification),
}

/// The button mask is the result of the actual BUTTON 96 query. A configured
/// key classification is required for SCROLL2; a VM key slot is not a mouse
/// device code. Movement coordinates are decoded from both explicit buffers.
#[derive(Default)]
pub(super) struct LowerInputContext {
    pub button_mask: Option<u32>,
    pub left_button: Option<bool>,
    pub pointer: Option<Point>,
    pub input_task: Option<u32>,
}

pub(super) struct ControlInputEffects {
    pub owner: u32,
    pub refresh: bool,
    pub commands: Vec<TaskCommand>,
    pub notification: Option<ControlNotification>,
}

pub(super) enum LowerInputOutcome {
    Unhandled,
    /// Query once, then call lower_input again with its result. This stage
    /// has not changed the control or acknowledged the original message.
    QueryButtonMask(TaskCommand),
    /// Execute commands and the synchronous notification exactly once before
    /// acknowledging the input. Refresh commits real pixels and hit regions.
    Applied(ControlInputEffects),
}

impl FesControlStore {
    /// Only an outer FES media-position stage consumes its request after the
    /// callback. A standalone lower command leaves the parent's new position
    /// pending for the next ordinary media pass.
    pub(super) fn finish_lower_notification(
        &mut self,
        sender: u32,
        consume_media_position: bool,
    ) -> Result<()> {
        let object = self
            .banks
            .values_mut()
            .find(|object| object.lower == Some(sender))
            .context("control notification sender has been released")?;
        if consume_media_position {
            object.position_dirty = false;
        }
        Ok(())
    }

    pub(super) fn captured_region(&self) -> Option<u32> {
        self.banks
            .values()
            .find(|object| object.drag_origin.is_some())
            .and_then(|object| object.buttons.primary[1])
    }

    pub(super) fn owns_lower(&self, task: u32) -> bool {
        self.banks.values().any(|object| object.lower == Some(task))
    }

    pub(super) fn region_receiver(&self, region: u32) -> Option<u32> {
        self.banks
            .values()
            .find(|object| object.is_slider() && object.has_region(region))?
            .lower
    }

    pub(super) fn legacy_scroll_react_region(
        &self, owner: u32, name: &[u8], bank: i32,
    ) -> Option<Option<u32>> {
        let object = self.banks.get(&(owner, name.to_vec(), bank))?;
        (object.kind == Kind::Scroll).then(|| object.buttons.react_region())
    }

    /// The BUTTON controller first checks its own ENABLE/KEYBLOCK and the
    /// current grid. This fallback ignores the slider's DISP/ENABLE. Reading
    /// the range consumes its request even when the direction cannot move it.
    pub(super) fn navigate_attached_slider(
        &mut self,
        owner: u32,
        name: &[u8],
        bank: i32,
        direction: ControlDirection,
    ) -> Result<Option<TaskCommand>> {
        let Some(object) = self.banks.get_mut(&(owner, name.to_vec(), bank)) else {
            return Ok(None);
        };
        if !object.is_slider() {
            return Ok(None);
        }
        object.range_pending = false;
        let Some(step) = direction.step(object.direction != 0) else {
            return Ok(None);
        };
        let (minimum, maximum) = if object.kind == Kind::Scroll {
            let [minimum, maximum] = object.requested_range;
            (minimum, maximum.saturating_sub(object.requested_page.unwrap_or(0)).max(minimum))
        } else if object.kind == Kind::Scroll2 {
            (
                object.requested_range[0],
                object.requested_range[1].wrapping_sub(
                    object
                        .requested_page
                        .context("attached SCROLL2 needs its explicit FES page value")?,
                ),
            )
        } else {
            (object.requested_range[0], object.requested_range[1])
        };
        let value = object.requested_position.wrapping_add(step);
        if value < minimum || value > maximum {
            return Ok(None);
        }
        let lower = object
            .lower
            .context("attached slider has no real lower task")?;
        object.requested_position = value;
        object.position_dirty = true;
        Ok(Some(TaskCommand {
            target: lower as i32,
            sender: owner as i32,
            code: if object.kind.is_scroll() {
                377
            } else {
                369
            },
            data: [1, value as u32, 0],
        }))
    }

    pub(super) fn alpha_motion_properties(
        &self,
        owner: u32,
        name: &[u8],
        bank: i32,
    ) -> Option<[u32; 36]> {
        let object = self.banks.get(&(owner, name.to_vec(), bank))?;
        if !object.is_slider() && object.kind != Kind::Window {
            return None;
        }
        let mut properties = [0; 36];
        properties[28] = (object.alpha & 255) << 24;
        Some(properties)
    }

    pub(super) fn slider_motion_targets(&self, owner: u32) -> Vec<SliderMotionTarget> {
        self.banks
            .iter()
            .filter_map(|((id, name, bank), object)| {
                (*id == owner && object.is_slider()).then(|| SliderMotionTarget {
                    name: name.clone(),
                    bank: *bank,
                    parent: object
                        .parent
                        .as_ref()
                        .map(|parent| (parent.name.clone(), parent.bank)),
                })
            })
            .collect()
    }

    pub(super) fn object_parent_links(&self, owner: u32) -> Vec<SliderMotionTarget> {
        self.banks.iter().filter_map(|((id, name, bank), object)| {
            (*id == owner).then(|| SliderMotionTarget {
                name: name.clone(), bank: *bank,
                parent: object.parent.as_ref().map(|parent| (parent.name.clone(), parent.bank)),
            })
        }).collect()
    }

    pub(super) fn slider_alpha_update_pending(
        &self,
        owner: u32,
        name: &[u8],
        bank: i32,
    ) -> Option<bool> {
        self.banks
            .get(&(owner, name.to_vec(), bank))
            .filter(|object| object.is_slider())
            .map(|object| object.alpha_pending)
    }

    pub(super) fn apply_alpha_motion_properties(
        &mut self,
        owner: u32,
        name: &[u8],
        bank: i32,
        properties: &[u32; 36],
    ) -> bool {
        let Some(object) = self.banks.get_mut(&(owner, name.to_vec(), bank)) else {
            return false;
        };
        if !object.is_slider() && object.kind != Kind::Window {
            return false;
        }
        object.alpha = properties[28] >> 24;
        object.alpha_pending = object.is_slider();
        true
    }

    pub(super) fn lower_input(
        &mut self,
        command: TaskCommand,
        context: &LowerInputContext,
    ) -> Result<LowerInputOutcome> {
        if !matches!(command.code, 6 | 9 | 10 | 64 | 65) {
            return Ok(LowerInputOutcome::Unhandled);
        }
        let Some(key) = self.banks.iter().find_map(|(key, object)| {
            (object.lower == Some(command.target as u32)).then(|| key.clone())
        }) else {
            return Ok(LowerInputOutcome::Unhandled);
        };
        let object = &self.banks[&key];
        let mut effects = ControlInputEffects {
            owner: key.0,
            refresh: false,
            commands: Vec::new(),
            notification: None,
        };
        if command.code == 65 {
            effects.commands.push(TaskCommand {
                target: context
                    .input_task
                    .context("control 65 needs the real root input task")?
                    as i32,
                sender: command.target,
                code: 156,
                data: [0; 3],
            });
            return Ok(LowerInputOutcome::Applied(effects));
        }
        if command.code == 64 {
            let knob = object.buttons.primary[1] == Some(command.data[0]);
            let object = self
                .banks
                .get_mut(&key)
                .context("control focus bank disappeared")?;
            if knob && object.drag_origin.is_none() {
                object.state = if command.data[1] != 0 { 2 } else { 0 };
                object.visual_dirty = true;
                effects.refresh = true;
            }
            if object.kind == Kind::Scroll
                && object.buttons.focus(command.data[0], command.data[1] != 0)
            {
                object.visual_dirty = true;
                effects.refresh = true;
            }
            effects.notification = Some(ControlNotification {
                owner: key.0,
                command: TaskCommand {
                    target: key.0 as i32,
                    sender: command.target,
                    code: 28,
                    data: [0, command.data[1], 0],
                },
                event: FesDriverInput {
                    code: 0,
                    target: Some(object.target(&key.1, key.2)),
                    event: Some(if command.data[1] != 0 {
                        b"FOCUS".to_vec()
                    } else {
                        b"UNFOCUS".to_vec()
                    }),
                },
            });
            return Ok(LowerInputOutcome::Applied(effects));
        }
        let region = if command.code == 6 {
            if object.drag_origin.is_none() {
                return Ok(LowerInputOutcome::Applied(effects));
            }
            object.buttons.primary[1].context("dragging control has no knob button")?
        } else {
            command.data[0]
        };
        let mask = if command.code == 9 {
            let Some(mask) = context.button_mask else {
                return Ok(LowerInputOutcome::QueryButtonMask(TaskCommand {
                    target: command.sender,
                    sender: command.target,
                    code: 96,
                    data: [0; 3],
                }));
            };
            mask
        } else {
            2
        };
        let known = object.has_region(region);
        let left = if object.kind == Kind::Scroll && command.code == 10 {
            // Re-selecting a region may synthesize a release without a
            // physical edge. Only an active legacy press can be released.
            object.buttons.has_pressed_region() && context.left_button != Some(false)
        } else if known && object.kind == Kind::Scroll && command.code == 9 && mask & 2 != 0 {
            context.left_button == Some(true)
        } else if known && object.kind == Kind::Scroll2 && command.code != 6 && mask & 2 != 0 {
            context.left_button.context(
                "SCROLL2 input requires an explicit configured left-button classification",
            )?
        } else {
            true
        };
        if !known || mask & 2 == 0 || !left {
            effects.commands.push(TaskCommand {
                target: command.sender,
                sender: command.target,
                code: 95,
                data: [0, 1, 0],
            });
            return Ok(LowerInputOutcome::Applied(effects));
        }
        // SCROLL2 track release leaves its state and geometry unchanged.
        if command.code == 10 && object.kind == Kind::Scroll2 && object.buttons.primary[0] == Some(region) {
            return Ok(LowerInputOutcome::Applied(effects));
        }
        if matches!(command.code, 6 | 9) && context.pointer.is_none() {
            bail!("control press/movement requires both decoded logical coordinates");
        }
        let code = match command.code {
            9 => 2,
            10 => 4,
            _ => 6,
        };
        let (_, event) = self
            .pointer_input(region, code, context.pointer, false)?
            .context("control input button disappeared")?;
        effects.refresh = true;
        if let Some(event) = event {
            effects.notification = Some(ControlNotification {
                owner: key.0,
                command: TaskCommand {
                    target: key.0 as i32,
                    sender: command.target,
                    code: 27,
                    data: [0, self.banks[&key].lower_value() as u32, 0],
                },
                event,
            });
        }
        Ok(LowerInputOutcome::Applied(effects))
    }

    fn keys(&self, owner: u32, name: &[u8], bank: Option<i32>) -> Vec<Key> {
        self.banks
            .keys()
            .filter(|(id, object, index)| {
                *id == owner && object == name && bank.is_none_or(|bank| bank == *index)
            })
            .cloned()
            .collect()
    }

    fn create(
        &mut self,
        host: &mut LoggingHost<'_>,
        owner: u32,
        definition: &FesObjectDefinition,
        driver: &FesScriptDriver,
    ) -> Result<bool> {
        let Some(kind) = control_kind(&definition.kind) else {
            return Ok(false);
        };
        if self.declarations.contains(&(owner, definition.offset)) {
            return Ok(true);
        }
        let count = driver
            .adapter
            .object(&definition.name)
            .context("control declaration has no bank metadata")?
            .definition
            .bank_count;
        if self.banks.len().saturating_add(count) > MAX_BANKS {
            bail!("FES control bank count exceeds 65536");
        }
        let identity = (owner, definition.offset);
        let start = *self.creations.entry(identity).or_insert(0);
        for bank in start..count {
            let key = (owner, definition.name.clone(), bank as i32);
            if self.banks.contains_key(&key) {
                if kind != Kind::Viewport {
                    bail!(
                        "duplicate IMGWND/SLIDER/SCROLL2 declarations need per-record wrapper/task ownership"
                    );
                }
                self.creations.insert(identity, bank + 1);
                continue;
            }
            let mut object = Bank::new(kind);
            if object.is_slider() {
                object.lower = Some(host.create_task_object(
                    if kind.is_scroll() { 41 } else { 42 },
                    owner,
                    match kind {
                        Kind::Scroll => b"FE scroll",
                        Kind::Scroll2 => b"FE scroll2",
                        _ => b"FE slider",
                    },
                    b"",
                )?);
            }
            self.banks.insert(key, object);
            self.creations.insert(identity, bank + 1);
        }
        self.declarations.insert((owner, definition.offset));
        Ok(true)
    }

    fn initialize(
        &mut self,
        owner: u32,
        initializer: &FesObjectInitialization,
        driver: &mut FesScriptDriver,
        shared: &mut ExpressionState,
        encoding: ScriptTextEncoding,
    ) -> Result<bool> {
        let (name, bank) = split_target(&initializer.target, encoding)?;
        if self.keys(owner, name, None).is_empty() {
            return Ok(false);
        }
        let identity = (owner, initializer.offset);
        if !self.initializers.contains_key(&identity) {
            let bank = bank
                .map(|bank| driver.evaluate_parameter(bank, 0, shared))
                .transpose()?;
            let keys = self.keys(owner, name, bank.filter(|bank| *bank != -1));
            let mut fields: Vec<_> =
                (0..initializer.columns.len().min(initializer.values.len())).collect();
            if keys.first().is_some_and(|key| self.banks[key].is_slider()) {
                fields.sort_by_key(|&index| match initializer.columns[index].as_slice() {
                    b"PARENT" => 0,
                    b"FILE" => 1,
                    b"PL" => 2,
                    b"POS2" => 3,
                    b"PRI" => 4,
                    b"DISP" => 5,
                    b"ENABLE" => 6,
                    _ => 7,
                });
            }
            self.initializers.insert(
                identity,
                Initializer {
                    keys,
                    cursor: 0,
                    fields,
                    priority_bank: bank.is_some_and(|value| value != -1),
                },
            );
        }
        let fields = initializer.columns.len().min(initializer.values.len());
        loop {
            let progress = &self.initializers[&identity];
            if progress.cursor >= fields {
                break;
            }
            let field = progress.fields[progress.cursor];
            let keys = progress.keys.clone();
            let priority_bank = progress.priority_bank;
            let column = &initializer.columns[field];
            let value = &initializer.values[field];
            let kind = keys
                .first()
                .and_then(|key| self.banks.get(key))
                .map(|bank| bank.kind);
            let mut values = Vec::new();
            let reference = if matches!(column.as_slice(), b"PL" | b"PARENT") {
                Some(reference(value, driver, shared, encoding)?)
            } else {
                None
            };
            let metadata = if matches!(column.as_slice(), b"POS2" | b"SIZE2") {
                Some(metadata_reference(value, encoding)?)
            } else {
                None
            };
            let numeric = match kind {
                Some(Kind::Viewport) => matches!(column.as_slice(), b"POS" | b"SIZE"),
                Some(Kind::Window) => {
                    matches!(
                        column.as_slice(),
                        b"POS" | b"BASE" | b"PRI" | b"DISP" | b"IDN" | b"BLEND"
                    ) || column.starts_with(b"ID.")
                }
                Some(Kind::Slider | Kind::Scroll2 | Kind::Scroll) =>
                    matches!(column.as_slice(), b"PRI" | b"DISP" | b"ENABLE")
                        || kind == Some(Kind::Scroll) && column == b"PRI_P",
                None => false,
            };
            if numeric {
                if matches!(column.as_slice(), b"POS" | b"SIZE" | b"BASE") {
                    for part in value.split(|&byte| byte == b',').take(2) {
                        values.push(driver.evaluate_parameter(part, 0, shared)?);
                    }
                } else {
                    values.push(driver.evaluate_parameter(value, 0, shared)?);
                }
            }
            for key in keys {
                let object = self
                    .banks
                    .get_mut(&key)
                    .context("control initializer bank disappeared")?;
                match column.as_slice() {
                    b"PARENT" => object.parent = reference.clone(),
                    b"FILE" if object.kind != Kind::Viewport => {
                        bounded(value, 255)?;
                        if !value.is_empty() {
                            object.file = value.clone();
                            object.file_resolved = false;
                            object.file_dirty = true;
                        }
                    }
                    b"PL" if object.kind != Kind::Viewport => object.plane = reference.clone(),
                    b"POS2" => {
                        let metadata = metadata.clone().context("POS2 metadata was not parsed")?;
                        if object.kind == Kind::Viewport {
                            object.viewport.set_position_source(metadata);
                        } else {
                            object.position_source = Some(metadata);
                        }
                    }
                    b"SIZE2" if matches!(object.kind, Kind::Viewport | Kind::Window) => {
                        let metadata = metadata.clone().context("SIZE2 metadata was not parsed")?;
                        if object.kind == Kind::Viewport {
                            object.viewport.set_size_source(metadata);
                        } else {
                            object.size_source = Some(metadata);
                            object.size_dirty = true;
                        }
                    }
                    b"POS" if matches!(object.kind, Kind::Viewport | Kind::Window) => {
                        for (axis, value) in values.iter().enumerate() {
                            if object.kind == Kind::Viewport {
                                object.viewport.position[axis] = *value as f32;
                            } else {
                                object.position[axis] = *value;
                            }
                        }
                        object.viewport.dirty = true;
                    }
                    b"BASE" if object.kind == Kind::Window => {
                        for (axis, value) in values.iter().enumerate() {
                            object.base[axis] = *value;
                        }
                    }
                    b"SIZE" if object.kind == Kind::Viewport => {
                        for (axis, value) in values.iter().enumerate() {
                            object.viewport.size[axis] = *value as f32;
                        }
                        object.viewport.dirty = true;
                    }
                    b"PRI" if object.kind != Kind::Viewport => {
                        let value = values[0].wrapping_add(if priority_bank { key.2 } else { 0 });
                        object.priority = value.wrapping_add(20);
                    }
                    b"PRI_P" if object.kind == Kind::Scroll => {
                        object.buttons.input_priority = Some(values[0]);
                    }
                    b"DISP" if object.kind != Kind::Viewport => object.display = values[0] as u32,
                    b"ENABLE" if object.is_slider() => object.enabled = values[0] as u32,
                    b"BLEND" if object.kind == Kind::Window => object.alpha = values[0] as u32,
                    b"IDN" if object.kind == Kind::Window => {
                        object.frame_number = values[0];
                        object.file_dirty = true;
                    }
                    column if column.starts_with(b"ID.") && object.kind == Kind::Window => {
                        let slot = match &column[3..] {
                            b"N" => 0,
                            b"D" => 1,
                            b"F" => 2,
                            b"P" => 3,
                            suffix => integer_literal(suffix) as usize,
                        };
                        if slot < 32 {
                            object.frames[slot] = values[0] as u32;
                            object.mapped = true;
                            object.file_dirty = true;
                        }
                    }
                    // Explicitly inapplicable columns, per the object-specific
                    // initialization contracts, are consumed without mutation.
                    _ => {}
                }
            }
            self.initializers
                .get_mut(&identity)
                .context("initializer continuation disappeared")?
                .cursor += 1;
        }
        Ok(true)
    }

    fn command(
        &mut self,
        owner: u32,
        command: &FesDriverCommand,
        driver: &mut FesScriptDriver,
        shared: &mut ExpressionState,
        encoding: ScriptTextEncoding,
        runtime: &FesRuntime,
    ) -> Result<bool> {
        let Some(raw) = command.parameters.first() else {
            return Ok(false);
        };
        let target = if raw == b"@" {
            let Some(object) = &command.object_event else {
                return Ok(false);
            };
            object.parameter()
        } else {
            raw.clone()
        };
        let (name, bank_source) = split_target(&target, encoding)?;
        let available = self.keys(owner, name, None);
        if available.is_empty() {
            return Ok(false);
        }
        let verb = command
            .parameters
            .get(1)
            .context("FES control command has no subcommand")?;
        if matches!(verb.as_slice(), b"stop" | b"oneshot" | b"continue") {
            return Ok(false);
        }
        let identity = (owner, command.offset);
        if !self.commands.contains_key(&identity) {
            let bank = bank_source
                .map(|bank| driver.evaluate_parameter(bank, command.context.current, shared))
                .transpose()?;
            let keys = self.keys(owner, name, bank.filter(|bank| *bank != -1));
            if keys.is_empty() {
                driver.fallback_command(command, shared)?;
                return Ok(true);
            }
            let args = &command.parameters[2..];
            let kind = keys
                .first()
                .and_then(|key| self.banks.get(key))
                .map(|bank| bank.kind);
            if kind == Some(Kind::Viewport)
                && !matches!(
                    verb.as_slice(),
                    b"pos" | b"size" | b"pos2" | b"getpos" | b"getpos2" | b"getsize"
                )
            {
                bail!(
                    "VIEWPORT has no documented handler for {:?}",
                    String::from_utf8_lossy(verb)
                );
            }
            let query = matches!(
                verb.as_slice(),
                b"getpos" | b"getpos2" | b"getsize" | b"getid"
            );
            if query && bank.is_none() && available.len() > 1 {
                return Ok(true);
            }
            if verb == b"pri" && args.is_empty() {
                return Ok(true);
            }
            let mut arguments = Vec::new();
            let mut bank_values = Vec::new();
            let mut raw_argument = None;
            match verb.as_slice() {
                b"disp" | b"enable" | b"alpha" | b"pri" | b"setid" => {
                    let current = keys.first().map(|key| &self.banks[key]);
                    let previous = match verb.as_slice() {
                        b"disp" => current.map_or(0, |object| object.display as i32),
                        b"enable" => current.map_or(1, |object| object.enabled as i32),
                        b"alpha" => current.map_or(255, |object| object.alpha as i32),
                        b"pri" => current.map_or(10, |object| object.priority.wrapping_sub(20)),
                        _ => current.map_or(0, |object| object.frame_number),
                    };
                    let default = if verb == b"disp" { 1 } else { 0 };
                    arguments.push(eval_optional(
                        driver,
                        shared,
                        args.first(),
                        previous,
                        default,
                    )?);
                }
                b"pos" | b"size" | b"page" | b"getpos" | b"getpos2" | b"getsize" => {
                    let count = if verb == b"size" { 2 } else { 3 };
                    for axis in 0..count {
                        let previous = keys.first().map_or(0, |key| {
                            let object = &self.banks[key];
                            if object.is_slider() && axis == 0 {
                                object.position_value()
                            } else if axis < 2 && verb == b"pos" {
                                object.position[axis]
                            } else if axis < 2 && verb == b"size" {
                                object.size[axis]
                            } else {
                                0
                            }
                        });
                        arguments.push(eval_optional(
                            driver,
                            shared,
                            args.get(axis),
                            previous,
                            if query {
                                -1
                            } else if kind == Some(Kind::Window) && verb == b"pos" {
                                previous
                            } else {
                                0
                            },
                        )?);
                    }
                }
                b"getid" => arguments.push(eval_optional(driver, shared, args.first(), 0, 0)?),
                b"settbl" => {
                    for index in 0..2 {
                        arguments.push(eval_optional(driver, shared, args.get(index), 0, 0)?);
                    }
                }
                b"load" => {
                    raw_argument = Some(args.first().cloned().unwrap_or_default());
                    for key in &keys {
                        bank_values.push(eval_optional(
                            driver,
                            shared,
                            args.get(1),
                            self.banks[key].frame_number,
                            0,
                        )?);
                    }
                }
                b"pos2" => {
                    raw_argument = args.first().cloned();
                    if raw_argument.is_some() {
                        arguments.push(eval_optional(driver, shared, args.get(1), 0, 0)?);
                    }
                }
                b"keypos" => {
                    for axis in 0..3 {
                        arguments.push(eval_optional(driver, shared, args.get(axis), 0, 0)?);
                    }
                }
                b"lock" => {
                    arguments.push(eval_optional(driver, shared, args.first(), 0, 0)?);
                }
                b"pl" => {
                    if let Some(raw) = args.first().filter(|raw| !raw.is_empty()) {
                        let plane = reference(raw, driver, shared, encoding)?;
                        if kind == Some(Kind::Window) {
                            for key in &keys {
                                self.banks
                                    .get_mut(key)
                                    .context("window plane bank disappeared")?
                                    .plane = Some(plane.clone());
                            }
                        }
                    }
                    return Ok(true);
                }
                b"attach" if matches!(kind, Some(Kind::Slider | Kind::Scroll2 | Kind::Scroll)) => {
                    return Ok(true);
                }
                _ => bail!(
                    "unsupported FES control subcommand {:?}",
                    String::from_utf8_lossy(verb)
                ),
            }
            self.commands.insert(
                identity,
                PreparedCommand {
                    keys,
                    verb: verb.clone(),
                    arguments,
                    raw: raw_argument,
                    cursor: 0,
                    bank_values,
                },
            );
        }
        loop {
            let prepared = &self.commands[&identity];
            let Some(key) = prepared.keys.get(prepared.cursor).cloned() else {
                break;
            };
            let verb = prepared.verb.clone();
            let values = prepared.arguments.clone();
            let bank_value = prepared
                .bank_values
                .get(prepared.cursor)
                .copied()
                .unwrap_or(0);
            let raw = prepared.raw.clone();
            let object = self
                .banks
                .get_mut(&key)
                .context("FES control bank disappeared")?;
            match verb.as_slice() {
                b"getpos" | b"getpos2" | b"getsize" if object.kind != Kind::Viewport => {
                    let result = if object.is_slider() {
                        [object.position_value(), 0, 0]
                    } else if verb == b"getsize" {
                        [object.size[0], object.size[1], 0]
                    } else if verb == b"getpos2" {
                        let position = window_position(object, runtime, encoding)?;
                        [position[0], position[1], 0]
                    } else {
                        [object.position[0], object.position[1], 0]
                    };
                    if !(object.is_slider() && verb == b"getpos2") {
                        for (&destination, value) in values.iter().zip(result) {
                            if let Some(slot) = shared.shared.get_mut(destination as u32 as usize) {
                                *slot = value;
                            }
                        }
                    }
                }
                b"getid" if object.kind == Kind::Window => {
                    if let Some(slot) = shared.shared.get_mut(values[0] as u32 as usize) {
                        *slot = object.frame_number;
                    }
                }
                b"disp" if object.kind != Kind::Viewport => {
                    object.display = u32::from(values[0] != 0)
                }
                b"enable" if object.is_slider() => object.enabled = u32::from(values[0] != 0),
                b"alpha" if object.kind != Kind::Viewport => {
                    object.alpha = values[0] as u32;
                    if object.is_slider() {
                        object.alpha_pending = true;
                    }
                }
                b"pri" if object.kind != Kind::Viewport => {
                    object.priority = values[0].wrapping_add(20)
                }
                b"pos" if object.is_slider() => {
                    object.requested_position = values[0];
                    object.position_dirty = true;
                }
                b"page" if object.is_slider() => {
                    object.requested_range = [values[0], values[1]];
                    object.range_pending = true;
                    if object.kind.is_scroll() {
                        object.requested_page = Some(values[2]);
                    }
                }
                b"pos" if object.kind == Kind::Window => object.position = [values[0], values[1]],
                b"size" if object.kind == Kind::Window => {
                    object.size = [values[0], values[1]];
                    object.size_dirty = true;
                }
                b"load" if object.kind != Kind::Viewport => {
                    if let Some(raw) = raw.filter(|raw| !raw.is_empty()) {
                        bounded(&raw, 255)?;
                        object.file = raw;
                        object.file_resolved = false;
                        object.file_dirty = true;
                    }
                    if object.kind == Kind::Window {
                        object.frame_number = bank_value;
                        object.file_dirty = true;
                    }
                }
                b"setid" if object.kind == Kind::Window => {
                    if !object.mapped || (0..32).contains(&values[0]) {
                        object.frame_number = values[0];
                        object.file_dirty = true;
                    }
                }
                b"settbl" if object.kind != Kind::Viewport => {
                    let capacity = if object.is_slider() { 4 } else { 32 };
                    if values[0] >= 0 && values[0] < capacity {
                        object.frames[values[0] as usize] = values[1] as u32;
                        object.mapped = true;
                        object.visual_dirty = true;
                    }
                }
                b"pos2" if object.kind == Kind::Window => {
                    if let Some(raw) = raw {
                        let mut source = metadata_reference(&raw, encoding)?;
                        if !raw.contains(&b':') {
                            source.frame = values.first().copied().unwrap_or(0) as u32;
                        }
                        object.position_source = Some(source);
                    }
                }
                _ => {}
            }
            self.commands
                .get_mut(&identity)
                .context("control continuation disappeared")?
                .cursor += 1;
        }
        self.commands.remove(&identity);
        Ok(true)
    }

    /// Real control input, addressed by the owned BUTTON ids. Main calls this
    /// before generic FES hit dispatch and forwards the optional driver event.
    pub(super) fn region_input(
        &mut self,
        region: u32,
        code: u32,
        pointer: Option<Point>,
    ) -> Result<Option<(u32, Option<FesDriverInput>)>> {
        self.pointer_input(region, code, pointer, true)
    }

    fn pointer_input(
        &mut self,
        region: u32,
        code: u32,
        pointer: Option<Point>,
        respect_enabled: bool,
    ) -> Result<Option<(u32, Option<FesDriverInput>)>> {
        let key = self
            .banks
            .iter()
            .find(|(_, object)| object.is_slider() && object.has_region(region))
            .map(|(key, _)| key.clone());
        let Some(key) = key else { return Ok(None) };
        let object = self
            .banks
            .get_mut(&key)
            .context("control input bank disappeared")?;
        if (respect_enabled && object.enabled == 0)
            || (object.kind == Kind::Scroll && (object.enabled == 0 || object.display == 0))
        {
            return Ok(Some((key.0, None)));
        }
        if matches!(code, 2 | 6) && pointer.is_none() {
            bail!("control pointer input needs both logical coordinates");
        }
        let knob = object.buttons.primary[1] == Some(region);
        let coordinate = pointer.map(|point| {
            if object.direction == 0 {
                point.x
            } else {
                point.y
            }
        });
        let origin = object.rectangle[object.direction as usize];
        let before = object.lower_value();
        if object.kind == Kind::Scroll {
            legacy_scroll::pointer_input(object, region, code, coordinate);
            let event = (before != object.lower_value()).then(|| {
                object.requested_position = object.lower_value();
                object.position_dirty = true;
                FesDriverInput {
                    code: 8,
                    target: Some(object.target(&key.1, key.2)),
                    event: Some(b"SCRL_P".to_vec()),
                }
            });
            return Ok(Some((key.0, event)));
        }
        let mut event = None;
        match code {
            2 => {
                if knob {
                    if object.kind != Kind::Scroll2
                        || object.scroll.travel[0] < object.scroll.travel[1]
                    {
                        object.drag_origin = coordinate.map(|value| [value, object.pixel()]);
                        object.slider.dragging = true;
                        object.scroll.dragging = true;
                        object.state = 3;
                    }
                } else if let Some(coordinate) = coordinate {
                    if object.kind == Kind::Scroll2 {
                        let start = origin.wrapping_add(object.scroll.pixel);
                        let value = if coordinate < start {
                            object
                                .scroll
                                .position
                                .wrapping_sub(object.scroll.page)
                                .max(0)
                        } else {
                            object
                                .scroll
                                .position
                                .wrapping_add(object.scroll.page)
                                .min(object.scroll.range[1].wrapping_sub(object.scroll.page))
                        };
                        object.scroll.set_position(value)?;
                    } else {
                        let extent = object.control_image.as_ref().map_or(0, |image| {
                            if object.direction == 0 {
                                image.metadata().canvas_size[0]
                            } else {
                                image.metadata().canvas_size[1]
                            }
                        }) as i32;
                        object.slider.pixel =
                            coordinate.wrapping_sub(origin).wrapping_sub(extent / 2);
                        object.slider.position =
                            object.slider.value_from_pixel(object.slider.pixel)?;
                        object.drag_origin = Some([coordinate, object.slider.pixel]);
                        object.slider.dragging = true;
                        object.state = 3;
                    }
                }
            }
            4 => {
                object.drag_origin = None;
                object.state = 0;
                object.slider.dragging = false;
                object.scroll.dragging = false;
                if object.kind == Kind::Scroll2 {
                    object.scroll.set_position(object.scroll.position)?;
                } else {
                    object.slider.set_position(object.slider.position)?;
                }
            }
            6 if object.drag_origin.is_some() => {
                if let (Some(coordinate), Some([pressed, pixel])) = (coordinate, object.drag_origin)
                {
                    let pixel = pixel.wrapping_add(coordinate.wrapping_sub(pressed));
                    if object.kind == Kind::Scroll2 {
                        object.scroll.pixel = pixel
                            .max(object.scroll.travel[0])
                            .min(object.scroll.travel[1]);
                        object.scroll.position =
                            object.scroll.value_from_pixel(object.scroll.pixel)?;
                    } else {
                        object.slider.pixel = pixel
                            .max(object.slider.travel[0])
                            .min(object.slider.travel[1]);
                        object.slider.position =
                            object.slider.value_from_pixel(object.slider.pixel)?;
                    }
                }
            }
            _ => {}
        }
        if (before != object.lower_value() || (code == 2 && !knob)) && code != 4 {
            object.requested_position = object.lower_value();
            object.position_dirty = true;
            event = Some(FesDriverInput {
                code: 8,
                target: Some(object.target(&key.1, key.2)),
                event: Some(b"SCRL_P".to_vec()),
            });
        }
        object.visual_dirty |= code != 6;
        Ok(Some((key.0, event)))
    }

    pub(super) fn region_focus(
        &mut self,
        region: u32,
        focused: bool,
    ) -> Option<(u32, FesDriverInput)> {
        let (key, object) = self
            .banks
            .iter_mut()
            .find(|(_, object)| object.is_slider() && object.has_region(region))?;
        if object.drag_origin.is_none() && object.buttons.primary[1] == Some(region) {
            object.state = if focused { 2 } else { 0 };
        }
        if object.kind == Kind::Scroll {
            object.buttons.focus(region, focused);
        }
        object.visual_dirty = true;
        Some((
            key.0,
            FesDriverInput {
                code: 0,
                target: Some(object.target(&key.1, key.2)),
                event: Some(if focused {
                    b"FOCUS".to_vec()
                } else {
                    b"UNFOCUS".to_vec()
                }),
            },
        ))
    }

    /// Typed lower-task receiver for the declared SLIDER/SCROLL2 tasks. Buffer
    /// ownership is explicit. A Refresh/Notify result is a continuation, never
    /// an acknowledgment that the platform effects already happened.
    pub(super) fn lower_command(
        &mut self,
        command: TaskCommand,
        buffer: Option<&[u8]>,
        output: &mut u32,
    ) -> Result<LowerControlOutcome> {
        let Some(key) = self
            .banks
            .iter()
            .find(|(_, object)| object.lower == Some(command.target as u32))
            .map(|(key, _)| key.clone())
        else {
            return Ok(LowerControlOutcome::Unhandled);
        };
        let object = self
            .banks
            .get_mut(&key)
            .context("control lower task disappeared")?;
        let operation = match (object.kind, command.code) {
            (Kind::Slider, 365..=373) => command.code - 365,
            (Kind::Scroll2 | Kind::Scroll, 374..=382) => match command.code {
                374 => 0,
                375 => 2,
                376 => 3,
                377 => 4,
                378 => 5,
                379 => 1,
                380 => 6,
                381 => 7,
                _ => 8,
            },
            _ => return Ok(LowerControlOutcome::Unhandled),
        };
        match operation {
            0 => {
                let bytes = buffer
                    .filter(|bytes| bytes.len() >= 0x124)
                    .context("SLIDER/SCROLL2 configuration needs 0x124 explicit bytes")?;
                let filename = &bytes[4..264];
                let length = filename
                    .iter()
                    .position(|&byte| byte == 0)
                    .context("control configuration resource has no terminating zero")?;
                object.file = filename[..length].to_vec();
                object.file_resolved = true;
                object.priority = read_dword(bytes, 264)? as i32;
                object.rectangle = [
                    read_dword(bytes, 268)? as i32,
                    read_dword(bytes, 272)? as i32,
                    read_dword(bytes, 276)? as i32,
                    read_dword(bytes, 280)? as i32,
                ];
                object.list_override = Some(read_dword(bytes, 0)?);
                object.plane_offset = Some([
                    read_dword(bytes, 284)? as i32,
                    read_dword(bytes, 288)? as i32,
                ]);
                object.file_dirty = true;
                Ok(LowerControlOutcome::Refresh { owner: key.0 })
            }
            1 => {
                object.display = command.data[1];
                Ok(LowerControlOutcome::Refresh { owner: key.0 })
            }
            2 => {
                object.enabled = command.data[1];
                if object.kind == Kind::Slider {
                    object.state = usize::from(command.data[1] == 0);
                    object.applied_enabled = Some(command.data[1]);
                    object.visual_dirty = true;
                }
                Ok(LowerControlOutcome::Refresh { owner: key.0 })
            }
            3 => {
                let count = if object.kind.is_scroll() { 12 } else { 8 };
                let bytes = buffer
                    .filter(|bytes| bytes.len() >= count)
                    .context("control range requires an explicit integer buffer")?;
                let range = [read_dword(bytes, 0)? as i32, read_dword(bytes, 4)? as i32];
                if object.kind.is_scroll() {
                    object.scroll.set_range(range, read_dword(bytes, 8)? as i32);
                    object.file_dirty = true;
                    Ok(LowerControlOutcome::Refresh { owner: key.0 })
                } else {
                    object.slider.set_range(range);
                    Ok(LowerControlOutcome::Complete)
                }
            }
            4 => {
                if object.kind == Kind::Scroll {
                    legacy_scroll::set_position(object, command.data[1] as i32);
                } else if object.kind == Kind::Scroll2 {
                    object.scroll.set_position(command.data[1] as i32)?;
                } else {
                    object.slider.set_position(command.data[1] as i32)?;
                }
                object.visual_dirty = true;
                if command.data[0] == 0 {
                    Ok(LowerControlOutcome::Refresh { owner: key.0 })
                } else {
                    object.requested_position = object.lower_value();
                    object.position_dirty = true;
                    Ok(LowerControlOutcome::Notify(ControlNotification {
                        owner: key.0,
                        command: TaskCommand {
                            target: key.0 as i32,
                            sender: object.lower.context("control lower identity is absent")?
                                as i32,
                            code: 27,
                            data: [0, object.lower_value() as u32, 0],
                        },
                        event: FesDriverInput {
                            code: 8,
                            target: Some(object.target(&key.1, key.2)),
                            event: Some(b"SCRL_P".to_vec()),
                        },
                    }))
                }
            }
            5 => {
                *output = object.direction;
                Ok(LowerControlOutcome::Complete)
            }
            6 => {
                object.alpha = command.data[1];
                Ok(LowerControlOutcome::Refresh { owner: key.0 })
            }
            7 => {
                if command.data[0] < 4 {
                    object.frames[command.data[0] as usize] = command.data[1];
                }
                Ok(LowerControlOutcome::Complete)
            }
            8 => {
                object.visual_dirty = true;
                Ok(LowerControlOutcome::Refresh { owner: key.0 })
            }
            _ => unreachable!("lower command operation is mapped above"),
        }
    }
}

/// Extends STRING initialization at its exact column position without taking
/// ownership of the text backend. Prefix calls share the text initializer's
/// saved cursor, so completed expression side effects are not replayed.
pub(super) fn apply_text_viewport_initializer(
    host: &mut LoggingHost<'_>,
    view: &FesTaskView,
    runtime: &mut FesRuntime,
    driver: &mut FesScriptDriver,
    effect: &FesDriverEffect,
    shared: &mut ExpressionState,
) -> Result<Option<bool>> {
    let FesDriverEffect::Initialize {
        event: FesEvent::InitializeObject(initializer),
        dispatch,
    } = effect
    else {
        return Ok(None);
    };
    if !initializer
        .columns
        .iter()
        .any(|column| column == b"VIEWPORT")
    {
        return Ok(None);
    }
    let (name, bank) = split_target(&initializer.target, host.text_encoding)?;
    let Some(definition) = driver
        .adapter
        .object(name)
        .map(|object| object.definition.clone())
    else {
        return Ok(None);
    };
    if !definition.kind_name.eq_ignore_ascii_case(b"STRING")
        && !definition.kind_name.eq_ignore_ascii_case(b"STRING8")
        && !definition.kind_name.eq_ignore_ascii_case(b"LOGSTRING")
        && !definition.kind_name.eq_ignore_ascii_case(b"LOGSTRING8")
    {
        return Ok(None);
    }
    let bank = bank
        .map(|source| -> Result<i32> { Ok(std::str::from_utf8(source)?.parse()?) })
        .transpose()?;
    let banks: Vec<_> = (0..definition.bank_count as i32)
        .filter(|&index| {
            bank.is_none_or(|bank| bank == index)
                && host
                    .data_services
                    .fes_text
                    .text(view.id, name, index)
                    .is_some()
        })
        .collect();
    let count = initializer.columns.len().min(initializer.values.len());
    let identity = (view.id, initializer.offset);
    let start = *host
        .data_services
        .fes_controls
        .text_viewport_initializers
        .get(&identity)
        .unwrap_or(&0);
    for index in start..count {
        if initializer.columns[index] != b"VIEWPORT" {
            continue;
        }
        apply_text_initializer_prefix(
            host,
            view,
            runtime,
            driver,
            initializer,
            dispatch,
            shared,
            index,
        )?;
        if !banks.is_empty() {
            let reference = reference_fn(
                &initializer.values[index],
                driver,
                shared,
                host.text_encoding,
            )?;
            for &bank in &banks {
                let key = (view.id, name.to_vec(), bank);
                let mut layer_key = control_layer_key(name, bank);
                layer_key.replace_range(.."\0control:".len(), "\0text:");
                let store = &mut host.data_services.fes_controls;
                let last = store.consumers.get(&key).and_then(|consumer| consumer.last);
                store.consumers.insert(
                    key,
                    Consumer {
                        reference: reference.clone(),
                        task: None,
                        last,
                        applied: false,
                        layer_key,
                    },
                );
            }
        }
        host.data_services
            .fes_controls
            .text_viewport_initializers
            .insert(identity, index + 1);
    }
    let result = apply_text_initializer_prefix(
        host,
        view,
        runtime,
        driver,
        initializer,
        dispatch,
        shared,
        count,
    )?;
    if result == Some(true) {
        host.data_services
            .fes_controls
            .text_viewport_initializers
            .insert(identity, count);
    }
    Ok(result)
}

fn apply_text_initializer_prefix(
    host: &mut LoggingHost<'_>,
    view: &FesTaskView,
    runtime: &mut FesRuntime,
    driver: &mut FesScriptDriver,
    initializer: &FesObjectInitialization,
    dispatch: &cs2_runtime::FesDispatch,
    shared: &mut ExpressionState,
    end: usize,
) -> Result<Option<bool>> {
    let mut filtered = initializer.clone();
    filtered.columns.clear();
    filtered.values.clear();
    for (column, value) in initializer
        .columns
        .iter()
        .zip(&initializer.values)
        .take(end)
    {
        if column != b"VIEWPORT" {
            filtered.columns.push(column.clone());
            filtered.values.push(value.clone());
        }
    }
    let effect = FesDriverEffect::Initialize {
        event: FesEvent::InitializeObject(filtered),
        dispatch: dispatch.clone(),
    };
    let result = super::fes_text::apply_effect(host, view, runtime, driver, &effect, shared)?;
    if result != Some(true) {
        bail!("STRING viewport initialization lost its text owner");
    }
    Ok(result)
}

pub(super) fn apply_effect(
    host: &mut LoggingHost<'_>,
    view: &FesTaskView,
    runtime: &mut FesRuntime,
    driver: &mut FesScriptDriver,
    effect: &FesDriverEffect,
    shared: &mut ExpressionState,
) -> Result<Option<bool>> {
    if let Some(done) = super::fes_driver::main_flow::controls_flow::apply_effect(host, view, driver, effect)? {
        return Ok(Some(done));
    }
    let mut store = std::mem::take(&mut host.data_services.fes_controls);
    let result = match effect {
        FesDriverEffect::Initialize {
            event: FesEvent::CreateObject(definition),
            ..
        } => store
            .create(host, view.id, definition, driver)
            .map(|done| done.then_some(true)),
        FesDriverEffect::Initialize {
            event: FesEvent::InitializeObject(initializer),
            ..
        } => store
            .initialize(view.id, initializer, driver, shared, host.text_encoding)
            .map(|done| done.then_some(true)),
        FesDriverEffect::Command(command) => store
            .command(
                view.id,
                command,
                driver,
                shared,
                host.text_encoding,
                runtime,
            )
            .map(|done| done.then_some(true)),
        _ => Ok(None),
    };
    host.data_services.fes_controls = store;
    result
}

pub(super) fn update_owner(
    host: &mut LoggingHost<'_>,
    view: &FesTaskView,
    runtime: &mut FesRuntime,
    driver: &mut FesScriptDriver,
) -> Result<bool> {
    let mut store = std::mem::take(&mut host.data_services.fes_controls);
    let result = update_store(&mut store, host, view, runtime, driver, None);
    host.data_services.fes_controls = store;
    result.map(|()| true)
}

/// Commits one lower task before its synchronous parent notification. Other
/// FES requests, including position edits made by a callback, remain pending.
/// The shared list update uses its ordinary wrapper-alias-aware pixel writer.
pub(super) fn refresh_lower(
    host: &mut LoggingHost<'_>,
    view: &FesTaskView,
    runtime: &mut FesRuntime,
    driver: &FesScriptDriver,
    task: u32,
) -> Result<bool> {
    let Some(key) = host
        .data_services
        .fes_controls
        .banks
        .iter()
        .find_map(|(key, object)| (object.lower == Some(task)).then(|| key.clone()))
    else {
        return Ok(false);
    };
    if key.0 != view.id {
        bail!("lower control refresh needs its actual FES owner");
    }
    let layer_key = control_layer_key(&key.1, key.2);
    let object = &host.data_services.fes_controls.banks[&key];
    if object.file_dirty || object.size_dirty {
        submit_control_projection(host, view, &layer_key, None)?;
    }
    let mut store = std::mem::take(&mut host.data_services.fes_controls);
    let result = update_store(&mut store, host, view, runtime, driver, Some(task));
    let projection = store
        .banks
        .get(&key)
        .and_then(|object| object.projection.clone());
    host.data_services.fes_controls = store;
    result?;
    submit_control_projection(host, view, &layer_key, projection)?;
    Ok(true)
}

fn submit_control_projection(
    host: &mut LoggingHost<'_>,
    view: &FesTaskView,
    key: &str,
    projection: Option<ControlProjection>,
) -> Result<()> {
    let target = projection.as_ref().and_then(|projection| {
        projection
            .list
            .or_else(|| view.list.as_ref().map(|list| list.task))
    });
    if let Some(target) = target {
        if !host
            .data_services
            .tasks
            .fes
            .platform
            .lists
            .contains_key(&target)
        {
            bail!("lower control image list is unavailable");
        }
    }
    let identity = (view.id, key.to_owned());
    let mut changed = BTreeSet::new();
    for (&id, list) in &mut host.data_services.tasks.fes.platform.lists {
        if list.layers.remove(&identity).is_some() {
            changed.insert(id);
        }
    }
    if let (Some(target), Some(projection)) = (target, projection) {
        host.data_services
            .tasks
            .fes
            .platform
            .lists
            .get_mut(&target)
            .context("lower control image list disappeared")?
            .layers
            .insert(
                identity,
                super::fes_task_platform::Layer {
                    frame: projection.frame,
                    options: projection.options,
                    priority: projection.priority,
                    affine: None,
                },
            );
        changed.insert(target);
    }
    for id in changed {
        if host.data_services.tasks.fes.platform.lists[&id].enabled {
            super::fes_task_platform::compose_list(host, id, false)?;
        }
    }
    Ok(())
}

fn update_store(
    store: &mut FesControlStore,
    host: &mut LoggingHost<'_>,
    view: &FesTaskView,
    runtime: &mut FesRuntime,
    driver: &FesScriptDriver,
    lower_only: Option<u32>,
) -> Result<()> {
    let keys: Vec<_> = store
        .banks
        .keys()
        .filter(|key| {
            key.0 == view.id && lower_only.is_none_or(|task| store.banks[*key].lower == Some(task))
        })
        .cloned()
        .collect();
    for key in &keys {
        let object = store
            .banks
            .get_mut(key)
            .context("control update bank disappeared")?;
        if object.kind == Kind::Viewport {
            object
                .viewport
                .update(|reference| read_metadata(host, reference))?;
        }
    }
    for key in &keys {
        let Some(object) = store.banks.get_mut(key) else {
            continue;
        };
        if object.kind == Kind::Viewport {
            continue;
        }
        if object.kind == Kind::Scroll && (object.display == 0 || object.enabled == 0 || view.paused != 0) {
            legacy_scroll::cancel_drag(object);
        }
        if lower_only.is_none() && !object.file_resolved {
            let expanded = expand_resource_name(host, &object.file)?;
            if object
                .expanded_file
                .as_ref()
                .is_some_and(|previous| previous != &expanded)
            {
                object.file_dirty = true;
            }
            object.expanded_file = Some(expanded);
        }
        if lower_only.is_none() && matches!(object.kind, Kind::Slider | Kind::Scroll) {
            let enabled = if view.paused == 0 { object.enabled } else { 0 };
            if object.applied_enabled != Some(enabled) {
                object.state = usize::from(enabled == 0);
                object.applied_enabled = Some(enabled);
                object.visual_dirty = true;
            }
        }
        let mut source_attempt = object.file_dirty || object.size_dirty;
        if source_attempt {
            prepare_control_source(host, object, lower_only.is_none())?;
        }
        if lower_only.is_none() && object.is_slider() && object.range_pending {
            let page = if object.kind.is_scroll() {
                Some(
                    object
                        .requested_page
                        .context("FES SCROLL2 range request has no page value")?,
                )
            } else {
                None
            };
            object.range_pending = false;
            if let Some(page) = page {
                object.scroll.set_range(object.requested_range, page);
                object.file_dirty = true;
                prepare_control_source(host, object, false)?;
                source_attempt = true;
            } else {
                object.slider.set_range(object.requested_range);
            }
        }
        if object.kind == Kind::Window && !source_attempt {
            if let Some(reference) = &object.position_source {
                object.position =
                    read_metadata(host, reference)?.map_or([0; 2], |(position, _)| position);
                object.position_source = None;
            }
        }
        if lower_only.is_none() && object.kind == Kind::Scroll && object.position_dirty {
            let position = object.requested_position;
            legacy_scroll::set_position(object, position);
            object.requested_position = object.lower_value();
            object.position_dirty = false;
            object.visual_dirty = true;
        }
        if object.visual_dirty && object.is_slider() && object.source.is_some()
            && (!source_attempt || object.kind == Kind::Scroll)
        {
            prepare_slider(object, false)?;
            object.visual_dirty = false;
        }
        if object.is_slider() {
            if lower_only.is_none() && object.position_dirty {
                if object.kind == Kind::Scroll2 {
                    object.scroll.set_position(object.requested_position)?;
                } else {
                    object.slider.set_position(object.requested_position)?;
                }
                object.position_dirty = false;
            }
            if lower_only.is_none() {
                // FES consumes the alpha request before forwarding properties;
                // the stored value remains available if list submission fails.
                object.alpha_pending = false;
            }
            if object.buttons.primary.iter().any(Option::is_none) && object.control_image.is_some() {
                let parent = object.lower.context("FES slider has no lower task")?;
                for slot in &mut object.buttons.primary {
                    if slot.is_none() {
                        *slot = Some(host.create_task_object(6, parent, b"control button", b"")?);
                    }
                }
                object.initial_region = true;
            }
            if object.kind == Kind::Scroll && object.control_image.is_some() {
                let parent = object.lower.context("FES scroll has no lower task")?;
                for slot in &mut object.buttons.arrows {
                    if slot.is_none() {
                        *slot = Some(host.create_task_object(6, parent, b"scroll arrow", b"")?);
                    }
                }
            }
            update_regions(
                host,
                object,
                if lower_only.is_none() { view.paused } else { 0 },
            )?;
        }
    }
    if lower_only.is_none() {
        update_viewport_consumers(store, host, view, driver)?;
    }
    for key in keys {
        let Some(object) = store.banks.get(&key) else {
            continue;
        };
        if object.kind == Kind::Viewport {
            continue;
        }
        // Selecting a missing slider state zeros the frame wrapper. The
        // independent image-list entry retains its last nonzero pixels.
        let image = object.control_image.clone().or_else(|| {
            (object.is_slider() && object.source.is_some())
                .then(|| {
                    object
                        .projection
                        .as_ref()
                        .map(|projection| projection.frame.clone())
                })
                .flatten()
        });
        let Some(image) = image else {
            if let Some(object) = store.banks.get_mut(&key) {
                object.projection = None;
            }
            continue;
        };
        let mut position = [
            object.position[0].wrapping_sub(object.base[0]),
            object.position[1].wrapping_sub(object.base[1]),
        ];
        if object.kind == Kind::Window {
            position = window_position(object, runtime, host.text_encoding)?;
        }
        if object.is_slider() {
            position = [object.rectangle[0], object.rectangle[1]];
            if object.kind != Kind::Scroll {
                position[object.direction as usize] =
                    position[object.direction as usize].wrapping_add(object.pixel());
            }
        }
        let plane_name = object
            .plane
            .as_ref()
            .map(|reference| runtime_name(&reference.name, reference.bank, host.text_encoding))
            .transpose()?;
        let plane = plane_name.as_ref().and_then(|name| {
            runtime
                .planes
                .get(name)
                .or_else(|| runtime.planes.get(&format!("{name}[0]")))
        });
        if let Some(offset) = object.plane_offset {
            position[0] = position[0].wrapping_add(offset[0]);
            position[1] = position[1].wrapping_add(offset[1]);
        } else if let Some(plane) = plane {
            position[0] = position[0].wrapping_add(plane.position.x);
            position[1] = position[1].wrapping_add(plane.position.y);
        }
        let position = Point::new(position[0], position[1]);
        let mut options = ImageDrawOptions::new(if object.kind == Kind::Slider {
            image.screen_position(position)
        } else {
            position
        });
        options.visible = object.display != 0 && (lower_only.is_some() || view.paused == 0);
        options.opacity = u8::try_from(object.alpha)
            .context("FES control alpha exceeds renderer's byte range")?;
        if let Some(plane) = &plane_name {
            apply_store_layer_clip(store, view.id, plane, &mut options);
        }
        let projection = ControlProjection {
            frame: image,
            options,
            priority: object.priority,
            owner: key.0,
            list: object.list_override,
            name: key.1.clone(),
            bank: key.2,
        };
        store
            .banks
            .get_mut(&key)
            .context("control projection bank disappeared")?
            .projection = Some(projection);
    }
    Ok(())
}

fn update_viewport_consumers(
    store: &mut FesControlStore,
    host: &mut LoggingHost<'_>,
    view: &FesTaskView,
    driver: &FesScriptDriver,
) -> Result<()> {
    // Each consumer's last successful bounds are retained across errors.
    let consumers: Vec<_> = store
        .consumers
        .keys()
        .filter(|(owner, _, _)| *owner == view.id)
        .cloned()
        .collect();
    for key in consumers {
        let consumer = &store.consumers[&key];
        let bounds = match lookup_viewport(store, view.id, &consumer.reference) {
            Some(bounds) => Some(bounds),
            None if consumer.reference.bank >= 0
                && driver
                    .adapter
                    .object_bank(&consumer.reference.name, consumer.reference.bank as usize)
                    .is_some() =>
            {
                None
            }
            None => continue,
        };
        if consumer.applied && consumer.last == bounds {
            continue;
        }
        let mut bytes: Vec<u8> = bounds
            .into_iter()
            .flatten()
            .flat_map(i32::to_le_bytes)
            .collect();
        let mut output = 0;
        if let Some(task) = consumer.task {
            let command = TaskCommand {
                target: task as i32,
                sender: view.id as i32,
                code: 89,
                data: [0, u32::from(bounds.is_some()), 0],
            };
            if !host.dispatch_plane_command(
                command,
                &mut super::plane_native::PlaneCommandBuffers {
                    data0: None,
                    data1: bounds.map(|_| bytes.as_mut_slice()),
                },
                &mut output,
            )? {
                bail!("VIEWPORT consumer needs a real plane clip receiver");
            }
        }
        let consumer = store
            .consumers
            .get_mut(&key)
            .context("viewport consumer disappeared")?;
        consumer.last = bounds;
        consumer.applied = true;
    }
    Ok(())
}

fn prepare_control_source(
    host: &LoggingHost<'_>,
    object: &mut Bank,
    read_references: bool,
) -> Result<()> {
    if object.is_slider() {
        object.control_image = None;
        object.projection = None;
        object.source = None;
        object.source_bytes.clear();
    }
    let file = if object.file_resolved {
        object.file.clone()
    } else {
        expand_resource_name(host, &object.file)?
    };
    let Some(bytes) = optional_image(host, &file)? else {
        // A missing window source retains the last pixels and pending size.
        // Slider reloads have already removed the independent list source.
        object.file_dirty = false;
        return Ok(());
    };
    let parsed = HgFile::parse(&bytes)?;
    if parsed.signature != *b"HG-3" {
        bail!("FES controls require HG-3 image resources");
    }
    if read_references {
        if let Some(reference) = &object.size_source {
            object.size = read_metadata(host, reference)?.map_or([0; 2], |(_, size)| size);
        }
        if let Some(reference) = &object.position_source {
            let (position, size) = read_metadata(host, reference)?.unwrap_or(([0; 2], [0; 2]));
            if object.is_slider() {
                object.rectangle = [
                    position[0],
                    position[1],
                    position[0].wrapping_add(size[0]),
                    position[1].wrapping_add(size[1]),
                ];
            } else {
                object.position = position;
            }
            object.position_source = None;
        }
    }
    object.source = Some(parsed);
    object.source_bytes = std::sync::Arc::unwrap_or_clone(bytes);
    if object.kind == Kind::Window {
        if object.size[0] > 0 && object.size[1] > 0 {
            object.control_image = Some(compose_window(
                object
                    .source
                    .as_ref()
                    .context("window image container disappeared")?,
                &object.source_bytes,
                [object.size[0] as u32, object.size[1] as u32],
            )?);
        }
        object.source = None;
        object.source_bytes.clear();
    } else {
        prepare_slider(object, true)?;
    }
    object.file_dirty = false;
    object.size_dirty = false;
    object.visual_dirty = false;
    Ok(())
}

fn window_position(
    object: &Bank,
    runtime: &FesRuntime,
    encoding: ScriptTextEncoding,
) -> Result<[i32; 2]> {
    let mut position = [
        object.position[0].wrapping_sub(object.base[0]),
        object.position[1].wrapping_sub(object.base[1]),
    ];
    if let Some(reference) = &object.parent {
        let name = runtime_name(&reference.name, reference.bank, encoding)?;
        if let Some(parent) = runtime
            .media
            .get(&name)
            .or_else(|| runtime.media.get(&format!("{name}[0]")))
            .filter(|parent| matches!(parent.kind.as_str(), "CIMAGE" | "IMAGE"))
        {
            // Image parent accumulation excludes the separate PLANE binding.
            // Further parent links are absent from the current layer model.
            let at = parent.draw_position(&BTreeMap::new());
            position[0] = position[0].wrapping_add(at.x);
            position[1] = position[1].wrapping_add(at.y);
        }
    }
    Ok(position)
}

fn prepare_slider(object: &mut Bank, reloaded: bool) -> Result<()> {
    if object.kind == Kind::Scroll {
        return legacy_scroll::prepare(object);
    }
    let source = object
        .source
        .as_ref()
        .context("slider image container is absent")?;
    let first = source
        .frames
        .iter()
        .find(|frame| frame.id == 0)
        .context("SLIDER/SCROLL2 require image frame zero")?;
    let width = object.rectangle[2].wrapping_sub(object.rectangle[0]);
    let height = object.rectangle[3].wrapping_sub(object.rectangle[1]);
    object.direction = if object.kind == Kind::Slider {
        u32::from((width as u32) < (height as u32))
    } else {
        u32::from(width < height)
    };
    let axis = object.direction as usize;
    let extent = if axis == 0 { width } else { height };
    if object.kind == Kind::Slider {
        let canvas = first.canvas_size[axis] as i32;
        object.slider.travel = [0, extent.wrapping_sub(canvas)];
        let frame = object.frames[object.state];
        object.control_image = source
            .frames
            .iter()
            .find(|candidate| candidate.id == frame)
            .map(|frame| decode_frame(frame, &object.source_bytes))
            .transpose()?;
    } else {
        let part = |id: u32| {
            source
                .frames
                .iter()
                .find(|frame| frame.id == id)
                .map_or(
                    0,
                    |frame| if axis == 0 { frame.width } else { frame.height },
                ) as i32
        };
        object.scroll.track_extent = extent.wrapping_abs();
        object.scroll.minimum_extent = part(1).wrapping_add(part(2));
        object.scroll.rebuild_geometry()?;
        object.scroll.travel = [0, extent.wrapping_sub(object.scroll.knob_extent)];
        object.control_image = Some(compose_scroll(
            source,
            &object.source_bytes,
            if reloaded { 0 } else { object.state as u32 },
            axis,
            object.scroll.knob_extent,
            object.scroll.minimum_extent,
        )?);
    }
    Ok(())
}

pub(super) fn projections(
    data: &super::data_services::DataServices,
    owner: u32,
) -> Vec<ControlProjection> {
    data.fes_controls
        .banks
        .iter()
        .filter(|((id, _, _), _)| *id == owner)
        .filter_map(|(_, bank)| bank.projection.clone())
        .collect()
}

/// The text service can consume the same resolved viewport without sharing
/// mutable text storage with controls. Missing names retain its previous clip.
pub(super) fn viewport_bounds(
    data: &super::data_services::DataServices,
    owner: u32,
    name: &[u8],
    bank: i32,
) -> Option<[i32; 4]> {
    lookup_viewport(
        &data.fes_controls,
        owner,
        &Reference {
            name: name.to_vec(),
            bank,
        },
    )
}

/// CPU image/list composition uses this even when the plane has no standalone
/// drawable. It keeps clipping in the real submitted layer options.
pub(super) fn apply_layer_clip(
    data: &super::data_services::DataServices,
    owner: u32,
    layer: &str,
    options: &mut ImageDrawOptions,
) {
    apply_store_layer_clip(&data.fes_controls, owner, layer, options);
}

fn apply_store_layer_clip(
    store: &FesControlStore,
    owner: u32,
    layer: &str,
    options: &mut ImageDrawOptions,
) {
    if let Some(consumer) = store
        .consumers
        .iter()
        .find_map(|((id, _, bank), consumer)| {
            (*id == owner
                && (consumer.layer_key == layer
                    || (*bank == 0 && format!("{}[0]", consumer.layer_key) == layer)))
                .then_some(consumer)
        })
    {
        if let Some(bounds) = lookup_viewport(store, owner, &consumer.reference).or(consumer.last) {
            let clip = rect_from_bounds(bounds);
            options.clip = Some(match options.clip {
                Some(original) => intersect_clip(original, clip),
                None => clip,
            });
        }
    }
}

fn intersect_clip(left: Rect, right: Rect) -> Rect {
    let x = i64::from(left.x).max(i64::from(right.x));
    let y = i64::from(left.y).max(i64::from(right.y));
    let end_x = (i64::from(left.x) + i64::from(left.width))
        .min(i64::from(right.x) + i64::from(right.width));
    let end_y = (i64::from(left.y) + i64::from(left.height))
        .min(i64::from(right.y) + i64::from(right.height));
    Rect::new(
        x as i32,
        y as i32,
        (end_x - x).max(0) as u32,
        (end_y - y).max(0) as u32,
    )
}

pub(super) fn bind_viewport_consumer(
    host: &mut LoggingHost<'_>,
    owner: u32,
    name: Vec<u8>,
    bank: i32,
    task: u32,
    reference: &[u8],
    driver: &mut FesScriptDriver,
    shared: &mut ExpressionState,
) -> Result<()> {
    let reference = reference_fn(reference, driver, shared, host.text_encoding)?;
    let layer_key = runtime_name(&name, bank, host.text_encoding)?;
    let identity = (owner, name, bank);
    let last = host
        .data_services
        .fes_controls
        .consumers
        .get(&identity)
        .and_then(|consumer| consumer.last);
    host.data_services.fes_controls.consumers.insert(
        identity,
        Consumer {
            reference,
            task: Some(task),
            last,
            applied: false,
            layer_key,
        },
    );
    Ok(())
}

pub(super) fn release_owner(host: &mut LoggingHost<'_>, owner: u32) -> Result<()> {
    let tasks: Vec<_> = host
        .data_services
        .fes_controls
        .banks
        .iter()
        .filter(|((id, _, _), _)| *id == owner)
        .filter_map(|(_, object)| object.lower)
        .collect();
    for task in tasks {
        host.request_ordinary_task_delete(task)?;
    }
    let store = &mut host.data_services.fes_controls;
    store.banks.retain(|(id, _, _), _| *id != owner);
    store.consumers.retain(|(id, _, _), _| *id != owner);
    store.declarations.retain(|(id, _)| *id != owner);
    store.creations.retain(|(id, _), _| *id != owner);
    store.initializers.retain(|(id, _), _| *id != owner);
    store.commands.retain(|(id, _), _| *id != owner);
    store.pending_inputs.retain(|(id, _)| *id != owner);
    store
        .text_viewport_initializers
        .retain(|(id, _), _| *id != owner);
    Ok(())
}

pub(super) fn bank_release_targets(
    store: &FesControlStore, owner: u32, name: &[u8], bank: i32,
) -> Option<(Vec<u32>, String)> {
    let object = store.banks.get(&(owner, name.to_vec(), bank))?;
    let tasks = object.button_regions().chain(object.lower).collect();
    Some((tasks, control_layer_key(name, bank)))
}

/// Called after the planned lower tasks and list entries have been released.
/// References from surviving consumers retain their last submitted clip.
pub(super) fn forget_released_bank(
    store: &mut FesControlStore, owner: u32, name: &[u8], bank: i32,
) {
    let key = (owner, name.to_vec(), bank);
    store.banks.remove(&key);
    store.consumers.remove(&key);
    for ((id, _), pending) in &mut store.commands {
        if *id != owner { continue; }
        let before = pending.keys.iter().take(pending.cursor).filter(|id| *id == &key).count();
        pending.bank_values = pending.bank_values.iter().copied().enumerate()
            .filter(|(index, _)| pending.keys.get(*index) != Some(&key))
            .map(|(_, value)| value).collect();
        pending.keys.retain(|id| id != &key);
        pending.cursor -= before;
    }
    for ((id, _), pending) in &mut store.initializers {
        if *id != owner { continue; }
        pending.keys.retain(|id| id != &key);
    }
    store.pending_inputs.retain(|(id, input)| {
        *id != owner || !input.target.as_ref().is_some_and(|target|
            target.name == name && target.bank == bank)
    });
}

impl HeadlessPlayer {
    pub(super) fn flush_control_inputs(&mut self) -> Result<()> {
        while let Some((owner, event)) = self
            .data_services
            .fes_controls
            .pending_inputs
            .front()
            .cloned()
        {
            self.data_services.tasks.fes.queue_input(owner, event)?;
            self.data_services.fes_controls.pending_inputs.pop_front();
        }
        Ok(())
    }

    pub(super) fn queue_control_region_focus(
        &mut self,
        region: u32,
        focused: bool,
    ) -> Result<bool> {
        if self
            .data_services
            .fes_controls
            .region_receiver(region)
            .is_none()
        {
            return Ok(false);
        }
        if self.data_services.fes_controls.pending_inputs.len() >= 4096 {
            bail!("control input continuation queue exceeds 4096 entries");
        }
        let Some((owner, event)) = self
            .data_services
            .fes_controls
            .region_focus(region, focused)
        else {
            return Ok(false);
        };
        self.data_services
            .fes_controls
            .pending_inputs
            .push_back((owner, event));
        Ok(true)
    }

    pub(super) fn queue_control_region_input(
        &mut self,
        region: u32,
        code: u32,
        point: Point,
    ) -> Result<bool> {
        if !matches!(code, 2 | 4) {
            return Ok(false);
        }
        self.queue_control_pointer_event(region, code, point)
    }

    pub(super) fn queue_control_pointer_motion(&mut self, point: Point) -> Result<()> {
        if let Some(region) = self.data_services.fes_controls.captured_region() {
            self.queue_control_pointer_event(region, 6, point)?;
        }
        Ok(())
    }

    fn queue_control_pointer_event(
        &mut self,
        region: u32,
        code: u32,
        point: Point,
    ) -> Result<bool> {
        if self
            .data_services
            .fes_controls
            .region_receiver(region)
            .is_none()
        {
            return Ok(false);
        }
        if self.data_services.fes_controls.pending_inputs.len() >= 4096 {
            bail!("control input continuation queue exceeds 4096 entries");
        }
        let Some((owner, event)) =
            self.data_services
                .fes_controls
                .region_input(region, code, Some(point))?
        else {
            return Ok(false);
        };
        if let Some(event) = event {
            self.data_services
                .fes_controls
                .pending_inputs
                .push_back((owner, event));
        }
        Ok(true)
    }
}

fn update_regions(host: &mut LoggingHost<'_>, object: &mut Bank, paused: u32) -> Result<()> {
    let enabled = if paused != 0 || (object.kind == Kind::Scroll && object.display == 0) {
        0
    } else {
        object.enabled
    };
    let Some(image) = &object.control_image else {
        // This is input135 even when the addressed region is absent, or its
        // enable DWORD already has this value. Registration is independent.
        if object.button_regions().next().is_some() {
            host.mark_input_geometry_dirty();
        }
        for region in &mut host.input_task.regions {
            if object.has_region(region.id()) {
                region.words[8] = enabled;
                if enabled == 0 {
                    region.words[7] = 0;
                }
            }
        }
        return Ok(());
    };
    let mut knob = object.rectangle;
    let axis = object.direction as usize;
    knob[axis] = knob[axis].wrapping_add(object.pixel());
    let mut dimensions = [image.width() as i32, image.height() as i32];
    if object.initial_region && object.kind == Kind::Slider {
        let offset = image.metadata().offset;
        knob[0] = knob[0].wrapping_add(offset[0]);
        knob[1] = knob[1].wrapping_add(offset[1]);
    } else if object.kind == Kind::Scroll2 && !object.initial_region {
        if let Some(first) = object
            .source
            .as_ref()
            .and_then(|source| source.frames.iter().find(|frame| frame.id == 0))
        {
            dimensions = [first.width as i32, first.height as i32];
            dimensions[axis] = object.scroll.knob_extent;
        }
    }
    knob[2] = knob[0].wrapping_add(dimensions[0]);
    knob[3] = knob[1].wrapping_add(dimensions[1]);
    if object.kind == Kind::Scroll2 && object.drag_origin.is_some() {
        knob = [0; 4];
    }
    let (ids, bounds) = if object.kind == Kind::Scroll {
        (
            [object.buttons.primary[0], object.buttons.primary[1], object.buttons.arrows[0], object.buttons.arrows[1]],
            legacy_scroll::regions(object),
        )
    } else {
        ([object.buttons.primary[0], object.buttons.primary[1], None, None], [object.rectangle, knob, [0; 4], [0; 4]])
    };
    let priority = object.buttons.input_priority.unwrap_or(object.priority);
    for (index, (id, bounds)) in ids.into_iter().zip(bounds).enumerate() {
        let Some(id) = id else {
            continue;
        };
        if let Some(region) = host
            .input_task
            .regions
            .iter_mut()
            .find(|region| region.id() == id)
        {
            region.words[3..7].copy_from_slice(&bounds.map(|value| value as u32));
            region.words[8] = enabled;
            if enabled == 0 {
                region.words[7] = 0;
            }
            host.mark_input_geometry_dirty();
        } else {
            if !object.initial_region {
                // A rectangle/enable update still invalidates input's cached
                // geometry when its previous registration has disappeared.
                host.mark_input_geometry_dirty();
            }
            if host.input_task.regions.len() >= 1_000_000 {
                bail!("control input region limit exceeded");
            }
            let mut words = [0; 15];
            words[0] = id;
            words[1] = priority.wrapping_add(index as i32) as u32;
            words[2] = 7;
            words[3..7].copy_from_slice(&bounds.map(|value| value as u32));
            words[8] = enabled;
            words[9] = 1;
            words[14] = 1;
            host.input_task.regions.push(InputRegionState { words });
        }
    }
    host.input_task
        .regions
        .sort_by_key(|region| std::cmp::Reverse(region.words[1] as i32));
    object.initial_region = false;
    Ok(())
}

fn lookup_viewport(store: &FesControlStore, owner: u32, reference: &Reference) -> Option<[i32; 4]> {
    store
        .banks
        .get(&(owner, reference.name.clone(), reference.bank))
        .filter(|bank| bank.kind == Kind::Viewport)
        .map(|bank| bank.viewport.bounds())
}

fn rect_from_bounds(bounds: [i32; 4]) -> Rect {
    Rect::new(
        bounds[0],
        bounds[1],
        bounds[2].wrapping_sub(bounds[0]).max(0) as u32,
        bounds[3].wrapping_sub(bounds[1]).max(0) as u32,
    )
}

fn reference_fn(
    bytes: &[u8],
    driver: &mut FesScriptDriver,
    shared: &mut ExpressionState,
    encoding: ScriptTextEncoding,
) -> Result<Reference> {
    let (name, bank) = split_target(bytes, encoding)?;
    bounded(name, 31)?;
    let bank = bank
        .map(|value| driver.evaluate_parameter(value, 0, shared))
        .transpose()?
        .unwrap_or(0);
    Ok(Reference {
        name: name.to_vec(),
        bank,
    })
}

fn reference(
    bytes: &[u8],
    driver: &mut FesScriptDriver,
    shared: &mut ExpressionState,
    encoding: ScriptTextEncoding,
) -> Result<Reference> {
    reference_fn(bytes, driver, shared, encoding)
}

pub(super) fn metadata_reference(bytes: &[u8], _encoding: ScriptTextEncoding) -> Result<MetadataReference> {
    let mut fields = bytes.splitn(2, |&byte| byte == b':');
    let name = fields.next().unwrap_or_default();
    bounded(name, 127)?;
    let frame = fields
        .next()
        .map_or(0, |field| integer_literal(field) as u32);
    Ok(MetadataReference {
        name: name.to_vec(),
        frame,
    })
}

pub(super) fn read_metadata(
    host: &LoggingHost<'_>,
    reference: &MetadataReference,
) -> RuntimeResult<Option<([i32; 2], [i32; 2])>> {
    let result = (|| -> Result<_> {
        let name = expand_resource_name(host, &reference.name)?;
        let bytes = optional_image(host, &name)?;
        let Some(bytes) = bytes else { return Ok(None) };
        let image = HgFile::parse(&bytes)?;
        Ok(image
            .frames
            .iter()
            .find(|frame| frame.id == reference.frame)
            .map(|frame| (frame.offset, [frame.width as i32, frame.height as i32])))
    })();
    result.map_err(|error| RuntimeError::Host(error.to_string()))
}

fn optional_image(host: &LoggingHost<'_>, name: &[u8]) -> Result<Option<std::sync::Arc<Vec<u8>>>> {
    if name.is_empty() {
        return Ok(None);
    }
    let name = decode(name, host.text_encoding)?;
    let names = host.resources.category_names(b"image", &name)?;
    for name in names {
        if let Some(resource) = host.resources.find(&name) {
            if resource.size > MAX_IMAGE {
                bail!("FES control image resource exceeds 64 MiB");
            }
            let bytes = host.read_encoded_image_candidate(resource)?;
            if bytes.len() as u64 > MAX_IMAGE {
                bail!("FES control image resource exceeds 64 MiB");
            }
            return Ok(Some(bytes));
        }
    }
    Ok(None)
}

fn expand_resource_name(host: &LoggingHost<'_>, name: &[u8]) -> Result<Vec<u8>> {
    Ok(host.data_services.language.languages.expand_image(name)?)
}

fn decode_frame(frame: &cs2_formats::hg::HgFrame, bytes: &[u8]) -> Result<RgbaFrame> {
    if frame.bit_depth != 32 || frame.encoded_image.is_some() {
        bail!("FES window/scroll image needs a native 32-bit HG-3 frame");
    }
    let mut metadata = FrameMetadata::raster(frame.canvas_size);
    metadata.offset = frame.offset;
    metadata.base = frame.base;
    metadata.frame_id = Some(frame.id);
    metadata.alpha_flag = frame.is_transparent;
    Ok(
        RgbaFrame::from_rgba8(frame.width, frame.height, frame.decode_pixels_rgba8(bytes)?)?
            .with_raw_metadata(metadata),
    )
}

fn compose_window(source: &HgFile, bytes: &[u8], size: [u32; 2]) -> Result<RgbaFrame> {
    checked_size(size)?;
    let mut canvas = Canvas::new(size[0], size[1])?;
    for id in 0..9 {
        let Some(frame) = source.frames.iter().find(|frame| frame.id == id) else {
            continue;
        };
        let image = decode_frame(frame, bytes)?;
        let (w, h) = (image.width(), image.height());
        if w == 0 || h == 0 {
            continue;
        }
        if size[0] < w || size[1] < h {
            bail!("IMGWND target is smaller than its tile");
        }
        let (xs, ys): (Vec<u32>, Vec<u32>) = match id {
            0 => (vec![0], vec![0]),
            1 => (vec![size[0] - w], vec![0]),
            2 => (vec![0], vec![size[1] - h]),
            3 => (vec![size[0] - w], vec![size[1] - h]),
            4 => (vec![0], tiles(h, size[1] - h, h)?),
            5 => (vec![size[0] - w], tiles(h, size[1] - h, h)?),
            6 => (tiles(w, size[0] - w, w)?, vec![0]),
            7 => (tiles(w, size[0] - w, w)?, vec![size[1] - h]),
            _ => (tiles(w, size[0] - w, w)?, tiles(h, size[1] - h, h)?),
        };
        if xs
            .len()
            .checked_mul(ys.len())
            .is_none_or(|blocks| blocks > 1_000_000)
        {
            bail!("window composition exceeds 1000000 tile blocks");
        }
        for &y in &ys {
            for &x in &xs {
                let region = Rect::new(
                    0,
                    0,
                    if id >= 6 {
                        (size[0] - w).saturating_sub(x).min(w)
                    } else {
                        w
                    },
                    if matches!(id, 4 | 5 | 8) {
                        (size[1] - h).saturating_sub(y).min(h)
                    } else {
                        h
                    },
                );
                blend_bottom(&mut canvas, &image, [x as i32, y as i32], region)?;
            }
        }
    }
    Ok(
        RgbaFrame::from_rgba8(size[0], size[1], canvas.pixels().to_vec())?
            .with_raw_metadata(FrameMetadata::raster([0; 2])),
    )
}

fn compose_scroll(
    source: &HgFile,
    bytes: &[u8],
    state: u32,
    axis: usize,
    extent: i32,
    minimum: i32,
) -> Result<RgbaFrame> {
    let mut parts = Vec::new();
    for part in [1, 0, 2] {
        let image = source
            .frames
            .iter()
            .find(|frame| frame.id == state * 100 + part)
            .map(|frame| decode_frame(frame, bytes))
            .transpose()?;
        parts.push(image);
    }
    let cross = parts
        .iter()
        .flatten()
        .map(|image| {
            if axis == 0 {
                image.height()
            } else {
                image.width()
            }
        })
        .max()
        .unwrap_or(0);
    let length = u32::try_from(extent).context("SCROLL2 knob length is negative")?;
    let size = if axis == 0 {
        [length, cross]
    } else {
        [cross, length]
    };
    checked_size(size)?;
    let mut canvas = Canvas::new(size[0], size[1])?;
    let mut at: i32 = 0;
    for (index, image) in parts.into_iter().enumerate() {
        let Some(image) = image else { continue };
        let part_extent = if index == 1 {
            extent.wrapping_sub(minimum)
        } else if axis == 0 {
            image.width() as i32
        } else {
            image.height() as i32
        };
        if part_extent > 0 {
            let target_size = if axis == 0 {
                [part_extent as u32, image.height()]
            } else {
                [image.width(), part_extent as u32]
            };
            let mut position = [0; 2];
            position[axis] = at;
            copy_scaled_bottom(&mut canvas, &image, position, target_size)?;
        }
        at = at.wrapping_add(part_extent);
    }
    if let Some(frame) = source
        .frames
        .iter()
        .find(|frame| frame.id == state * 100 + 3)
    {
        let image = decode_frame(frame, bytes)?;
        let mut position = frame.offset;
        position[axis] = (size[axis] / 2) as i32
            - (if axis == 0 {
                image.width()
            } else {
                image.height()
            } / 2) as i32;
        blend_bottom(
            &mut canvas,
            &image,
            position,
            Rect::new(0, 0, image.width(), image.height()),
        )?;
    }
    Ok(
        RgbaFrame::from_rgba8(size[0], size[1], canvas.pixels().to_vec())?
            .with_raw_metadata(FrameMetadata::raster([0; 2])),
    )
}

fn copy_scaled_bottom(
    canvas: &mut Canvas,
    source: &RgbaFrame,
    at: [i32; 2],
    size: [u32; 2],
) -> Result<()> {
    if size[0] == 0 || size[1] == 0 || source.width() == 0 || source.height() == 0 {
        return Ok(());
    }
    let exact = size == [source.width(), source.height()];
    let average = u64::from(source.width()) > u64::from(size[0]) * 4
        || u64::from(source.height()) > u64::from(size[1]) * 4;
    let step = [
        (u64::from(source.width()) << 15) / u64::from(size[0]),
        (u64::from(source.height()) << 15) / u64::from(size[1]),
    ];
    for y in 0..size[1] {
        for x in 0..size[0] {
            let to = [
                i64::from(at[0]) + i64::from(x),
                i64::from(at[1]) + i64::from(y),
            ];
            if to[0] < 0
                || to[1] < 0
                || to[0] >= i64::from(canvas.width())
                || to[1] >= i64::from(canvas.height())
            {
                continue;
            }
            let fx = u64::from(x) * step[0];
            let fy = u64::from(y) * step[1];
            let sx = (fx >> 15) as u32;
            let sy = (fy >> 15) as u32;
            let pixel = |x: u32, y: u32| {
                let index = ((source.height() - 1 - y.min(source.height() - 1)) as usize
                    * source.width() as usize
                    + x.min(source.width() - 1) as usize)
                    * 4;
                &source.pixels()[index..index + 4]
            };
            let values = [
                pixel(sx, sy),
                pixel((sx + 1).min(source.width() - 1), sy),
                pixel(sx, (sy + 1).min(source.height() - 1)),
                pixel(
                    (sx + 1).min(source.width() - 1),
                    (sy + 1).min(source.height() - 1),
                ),
            ];
            if !exact && !average && values.iter().all(|pixel| pixel[3] == 0) {
                continue;
            }
            let u = fx & 32767;
            let v = fy & 32767;
            let weights = [
                ((32768 - u) * (32768 - v)) >> 15,
                (u * (32768 - v)) >> 15,
                ((32768 - u) * v) >> 15,
                (u * v) >> 15,
            ];
            let mut result: [u8; 4] = std::array::from_fn(|axis| {
                ((0..4)
                    .map(|i| u64::from(values[i][axis]) * weights[i])
                    .sum::<u64>()
                    >> 15) as u8
            });
            if exact {
                result.copy_from_slice(values[0]);
            } else if average {
                let ratio = [
                    source.width() as f32 / size[0] as f32,
                    source.height() as f32 / size[1] as f32,
                ];
                let sx = (x as f32 * ratio[0]) as u32;
                let sy = (y as f32 * ratio[1]) as u32;
                let nx = (ratio[0] as u32).max(1).min(source.width() - sx);
                let ny = (ratio[1] as u32).max(1).min(source.height() - sy);
                let mut sum = [0u64; 4];
                for dy in 0..ny {
                    for dx in 0..nx {
                        let value = pixel(sx + dx, sy + dy);
                        for axis in 0..4 {
                            sum[axis] += u64::from(value[axis]);
                        }
                    }
                }
                let count = u64::from(nx) * u64::from(ny);
                if count == 0 {
                    bail!("control resampling window is empty");
                }
                result = sum.map(|value| (value / count) as u8);
            } else if values.iter().all(|pixel| pixel[3] == 255) {
                result[3] = 255;
            }
            canvas.set_pixel(
                to[0] as i32,
                canvas.height() as i32 - 1 - to[1] as i32,
                Color::rgba(result[0], result[1], result[2], result[3]),
            )?;
        }
    }
    Ok(())
}

fn blend_bottom(canvas: &mut Canvas, image: &RgbaFrame, at: [i32; 2], region: Rect) -> Result<()> {
    let mut options = ImageDrawOptions::new(Point::new(
        at[0],
        canvas.height() as i32 - at[1] - image.height() as i32,
    ));
    options.clip = Some(Rect::new(
        at[0],
        canvas.height() as i32 - at[1] - region.height as i32,
        region.width,
        region.height,
    ));
    canvas.draw_frame(image, &options)?;
    Ok(())
}

fn tiles(start: u32, end: u32, step: u32) -> Result<Vec<u32>> {
    if step == 0 {
        bail!("window tile has a zero pixel extent");
    }
    if end <= start {
        return Ok(Vec::new());
    }
    let count = (end - start).div_ceil(step) as usize;
    if count > 1_000_000 {
        bail!("window tiling exceeds 1000000 blocks");
    }
    Ok((start..end).step_by(step as usize).collect())
}

fn checked_size(size: [u32; 2]) -> Result<()> {
    let bytes = u64::from(size[0])
        .checked_mul(u64::from(size[1]))
        .and_then(|value| value.checked_mul(4));
    if size[0] == 0 || size[1] == 0 || bytes.is_none_or(|bytes| bytes > MAX_IMAGE) {
        bail!("control pixels are empty or exceed 64 MiB");
    }
    Ok(())
}

fn control_kind(kind: &[u8]) -> Option<Kind> {
    if kind.eq_ignore_ascii_case(b"VIEWPORT") {
        Some(Kind::Viewport)
    } else if kind.eq_ignore_ascii_case(b"IMGWND") {
        Some(Kind::Window)
    } else if kind.eq_ignore_ascii_case(b"SLIDER") {
        Some(Kind::Slider)
    } else if kind.eq_ignore_ascii_case(b"SCROLL2") {
        Some(Kind::Scroll2)
    } else if kind.eq_ignore_ascii_case(b"SCROLL") {
        Some(Kind::Scroll)
    } else {
        None
    }
}

fn control_layer_key(name: &[u8], bank: i32) -> String {
    let mut key = String::from("\0control:");
    use std::fmt::Write;
    for byte in name {
        let _ = write!(key, "{byte:02x}");
    }
    let _ = write!(key, "[{bank}]");
    key
}

fn eval_optional(
    driver: &mut FesScriptDriver,
    shared: &mut ExpressionState,
    value: Option<&Vec<u8>>,
    current: i32,
    default: i32,
) -> Result<i32> {
    value
        .map(|value| driver.evaluate_parameter(value, current, shared))
        .transpose()
        .map(|value| value.unwrap_or(default))
}

fn split_target(bytes: &[u8], encoding: ScriptTextEncoding) -> Result<(&[u8], Option<&[u8]>)> {
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'[' {
            if !bytes.ends_with(b"]") {
                bail!("control target bank is not closed");
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

fn runtime_name(name: &[u8], bank: i32, encoding: ScriptTextEncoding) -> Result<String> {
    let name = decode(name, encoding)?;
    Ok(if bank == 0 {
        name
    } else {
        format!("{name}[{bank}]")
    })
}

fn decode(bytes: &[u8], encoding: ScriptTextEncoding) -> Result<String> {
    let codec = match encoding {
        ScriptTextEncoding::Cp932 => encoding_rs::SHIFT_JIS,
        ScriptTextEncoding::Gbk => encoding_rs::GBK,
        ScriptTextEncoding::Utf8 => encoding_rs::UTF_8,
    };
    let (text, failed) = codec.decode_without_bom_handling(bytes);
    if failed {
        bail!("control name is invalid in the explicitly selected script encoding");
    }
    Ok(text.into_owned())
}

fn bounded(bytes: &[u8], maximum: usize) -> Result<()> {
    if bytes.len() > maximum {
        bail!("control name exceeds {maximum} bytes");
    }
    Ok(())
}

fn integer_literal(bytes: &[u8]) -> i32 {
    let negative = bytes.first() == Some(&b'-');
    let bytes = if matches!(bytes.first(), Some(b'+' | b'-')) {
        &bytes[1..]
    } else {
        bytes
    };
    let (bytes, radix) = if bytes.starts_with(b"0x") || bytes.starts_with(b"0X") {
        (&bytes[2..], 16)
    } else {
        (bytes, 10)
    };
    let mut value = 0u32;
    for &byte in bytes {
        let digit = match byte {
            b'0'..=b'9' => u32::from(byte - b'0'),
            b'a'..=b'f' if radix == 16 => u32::from(byte - b'a' + 10),
            b'A'..=b'F' if radix == 16 => u32::from(byte - b'A' + 10),
            _ => break,
        };
        value = value.wrapping_mul(radix).wrapping_add(digit);
    }
    (if negative {
        value.wrapping_neg()
    } else {
        value
    }) as i32
}

fn read_dword(bytes: &[u8], offset: usize) -> Result<u32> {
    let value = bytes
        .get(offset..offset + 4)
        .context("control configuration is truncated")?;
    Ok(u32::from_le_bytes(value.try_into().map_err(|_| {
        anyhow::anyhow!("control DWORD is truncated")
    })?))
}

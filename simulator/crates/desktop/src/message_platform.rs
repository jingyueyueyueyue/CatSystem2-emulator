//! Platform entry points kept separate so the main agent can add module
//! declarations and three call sites without altering LoggingHost construction.

use std::collections::BTreeMap;

use cs2_formats::ConfigXml;
use cs2_renderer::RgbaFrame;
use cs2_runtime::message_object::{
    ButtonSource, DeviceBinding, DeviceKeySample, DeviceKeys, InputButtonState,
    InputFallbackDisposition, InputHitMask, InputRouteReceipt, KeyScanStep, KeyTransition,
    KeyTransitionKind, MessageAtom, MessageBackend, MessageBreakCharacters, MessageCommandResult,
    MessageConfiguration, MessageDefaults, MessageEncoding, MessageLayout, MessagePaintResult,
    MessagePlaneDisplay, MessageRenderRequest, MessageUserGlyph, RoutedPhysicalButton,
};
use cs2_script::{NativeReturn, Result, ScriptError, Vm};

use super::{
    HeadlessPlayer, LoggingHost, SceneNode,
    message_backend::DesktopMessageDevice,
    message_native::{LEGACY_STRING_KIND_45, MessageNativeContext, MessageRoute, UTATE_STRING_KIND},
    native_tasks::NativeTaskStore,
    ordinary_native::OrdinaryNativeServices,
};

#[path = "kx_native.rs"]
pub(crate) mod kx_native;

#[derive(Clone, Debug)]
pub(super) struct DeviceKeyDispatch {
    transition: KeyTransition,
    sender: u32,
    point: [i32; 2],
    next: usize,
    current: Option<([u32; 15], usize)>,
    output: u32,
    buffers: super::native_tasks::OwnedTaskBuffers,
    visited: usize,
}

#[derive(Clone, Debug)]
pub(super) struct InputSelectionDispatch {
    point: [i32; 2],
    candidate: Option<u32>,
    selected: Option<u32>,
    next: usize,
    current: Option<([u32; 15], usize, u32)>,
    buffers: super::native_tasks::OwnedTaskBuffers,
    output: u32,
    stage: u8,
    visited: usize,
    all_routed: bool,
    routed_any: bool,
}

#[derive(Clone, Debug)]
pub(super) struct PointerInputDispatch {
    code: u32,
    point: [i32; 2],
    next: usize,
    current: Option<([u32; 15], usize)>,
    buffers: super::native_tasks::OwnedTaskBuffers,
    output: u32,
    stage: u8,
    sampled_mask: u32,
    visited: usize,
    motion_routed: bool,
    motion_delivered: bool,
    motion_started: bool,
}

pub(super) type ButtonOwnerHook = for<'host> fn(
    &mut LoggingHost<'host>,
    cs2_runtime::TaskCommand,
    [i32; 2],
    &mut super::native_tasks::OwnedTaskBuffers,
    &mut u32,
    usize,
) -> Result<Option<bool>>;

/// Resolves the actual registered region binding, including old FES record
/// ids. Parentage alone does not establish a BUTTON event receiver.
pub(super) type ButtonBindingLookup = for<'host> fn(&LoggingHost<'host>, u32) -> Option<u32>;

pub(super) type FesButtonTargetLookup =
    for<'host> fn(
        &LoggingHost<'host>,
        u32,
    ) -> Option<(u32, Option<super::fes_driver::FesDriverTarget>)>;

/// Refreshes the actual FES objects/media at its saved elapsed time, without
/// entering the normal FES body or completing a queued input instead.
pub(super) type FesButtonMediaHook =
    for<'host> fn(&mut LoggingHost<'host>, u32, usize) -> Result<bool>;

pub(super) type FesButtonNamedHook =
    for<'host> fn(&mut LoggingHost<'host>, u32, &'static str, i32) -> Result<()>;

#[derive(Clone, Debug)]
pub(super) struct FesButtonRecord {
    pub(super) target: super::fes_driver::FesDriverTarget,
    pub(super) enabled: u32,
    pub(super) syskey: i32,
}

/// Index is declaration-record order, including duplicates. Each call reads
/// the object's current properties after earlier handlers have completed.
pub(super) type FesButtonRecordLookup =
    for<'host> fn(&LoggingHost<'host>, u32, usize) -> Result<Option<FesButtonRecord>>;

#[derive(Clone, Debug, Default)]
pub(super) struct FesButtonController {
    pub(super) focus: Option<super::fes_driver::FesDriverTarget>,
    pub(super) preferred_focus: Option<super::fes_driver::FesDriverTarget>,
    pub(super) pressed: Option<super::fes_driver::FesDriverTarget>,
    pub(super) focus_lock: u32,
    pub(super) raw_state: u32,
    pub(super) controller_state: u32,
}

#[derive(Clone, Debug)]
pub(super) struct FesButtonOwnerDispatch {
    command: cs2_runtime::TaskCommand,
    navigation: Option<super::fes_driver::main_flow::NavigationDispatch>,
    input: Option<super::fes_driver::FesDriverInput>,
    response: Option<u32>,
    send_selection: bool,
    clear_selection: bool,
    refresh: bool,
    named: Option<(&'static str, i32)>,
    syskey: bool,
    record: usize,
    record_object_handler: bool,
    record_default_handler: bool,
    stage: u8,
}

#[derive(Clone, Debug)]
pub(super) struct ButtonInputContext {
    pub(super) key: u32,
    pub(super) group: Option<u32>,
    pub(super) raw_mask: u32,
    pub(super) pressed: u32,
    pub(super) repeated: bool,
    pub(super) physical: Vec<RoutedPhysicalButton>,
}

#[derive(Clone, Debug)]
pub(super) struct InputButtonRuntime {
    pub(super) state: InputButtonState,
    began: Option<u64>,
    written: Option<u64>,
    registration_bound: bool,
}

impl InputButtonRuntime {
    pub(crate) fn execution_checkpoint(&self) -> (InputButtonState, Option<u64>, Option<u64>, bool) {
        (self.state.clone(), self.began, self.written, self.registration_bound)
    }
    pub(crate) fn from_execution_checkpoint(value: (InputButtonState, Option<u64>, Option<u64>, bool)) -> Self {
        Self { state: value.0, began: value.1, written: value.2, registration_bound: value.3 }
    }
}

#[derive(Clone, Debug)]
pub(super) struct ButtonDispatch {
    command: cs2_runtime::TaskCommand,
    forward: cs2_runtime::TaskCommand,
    buffers: super::native_tasks::OwnedTaskBuffers,
    pointer: [i32; 2],
    notification_output: u32,
    original: super::native_tasks::OwnedTaskBuffers,
    group: Option<u32>,
    ordinary_buffers: Option<super::native_tasks::OwnedTaskBuffers>,
    owner_completed: bool,
    raw_mask: u32,
    repeated: bool,
    physical: Vec<RoutedPhysicalButton>,
}

/// A platform owner hook supplies the actual object/bank event and response
/// decision. Its callback is completed once before response and region-clear
/// actions; failures resume at the action that has not completed.
#[derive(Clone, Debug)]
pub(super) struct FesButtonDispatch {
    command: cs2_runtime::TaskCommand,
    input: super::fes_driver::FesDriverInput,
    response: Option<u32>,
    clear_selection: bool,
    stage: u8,
}

fn button_done() -> cs2_runtime::TaskCommandOutcome {
    cs2_runtime::TaskCommandOutcome {
        done: true,
        propagate: false,
    }
}

// slot, minimum DWORD prefix, command, route target zero, return output
const BUTTON_NATIVES: &[(u32, usize, u32, bool, bool)] = &[
    (99, 1, 91, false, false),
    (104, 7, 90, true, false),
    (105, 2, 93, true, false),
    (108, 2, 95, true, false),
    (109, 1, 96, true, true),
    (110, 2, 97, true, false),
    (848, 2, 99, true, false),
    (849, 2, 100, true, false),
];

const INPUT_REGION_NATIVES: &[(u32, usize, u32)] = &[
    (145, 0, 128),
    (146, 2, 129),
    (147, 2, 130),
    (148, 2, 132),
    (154, 2, 143),
    (286, 0, 146),
    (401, 1, 150),
];

// slot, minimum DWORD prefix, grouped record, first receiver command.
// The old file wrappers and speech1070..1073 share the real kind13 service
// query/continuation. Speech owns its nested text rather than a VM pointer.
const SOUND_PREPARE_NATIVES: &[(u32, usize, bool, u32)] = &[
    (155, 4, false, 180),
    (604, 5, true, 206),
    (1070, 3, false, 225),
    (1071, 4, true, 226),
    (1072, 5, false, 225),
    (1073, 6, true, 226),
];

/// Root+8 indexes strict descendants by their actual task number. It includes
/// released-but-not-yet-collected reservations, but not the root's own-number
/// shortcut used by ordinary task lookup. Scene nodes, reachability and
/// deletion flags cannot answer this query. Supply the actual root index.
pub(super) type RootTaskNumberLookup = for<'host> fn(&LoggingHost<'host>, u32) -> Result<bool>;

#[derive(Clone, Debug)]
pub(super) struct SoundPrepareNativeDispatch {
    index: u32,
    arguments: Vec<u8>,
    root_queried: bool,
    sound: Option<u32>,
    record: Option<Vec<u8>>,
}

#[derive(Clone, Debug)]
pub(super) struct InputNativeDispatch {
    index: u32,
    arguments: Vec<u8>,
    input: Option<u32>,
    buffers: Option<super::native_tasks::OwnedTaskBuffers>,
}

#[derive(Clone, Debug)]
struct CursorBroadcast {
    visible: u32,
    stack: Vec<(u32, bool, Option<i32>)>,
    visited: usize,
}

#[derive(Clone, Debug)]
pub(super) struct InputCursorDispatch {
    command: cs2_runtime::TaskCommand,
    initialized: bool,
    broadcast: Option<CursorBroadcast>,
}

#[derive(Clone, Debug)]
pub(super) struct InputCursorFrame {
    changed: bool,
    position_changed: bool,
    moving: bool,
    original_nonzero: bool,
    active: u32,
    foreground: bool,
    initialized: bool,
    broadcast: Option<CursorBroadcast>,
}

fn input_button_receiver(
    host: &mut LoggingHost<'_>,
    command: cs2_runtime::TaskCommand,
    buffers: &mut super::native_tasks::OwnedTaskBuffers,
    output: &mut u32,
    budget: usize,
) -> Result<Option<cs2_runtime::TaskCommandOutcome>> {
    host.receive_input_button_command(command, buffers, output, budget)
}

fn device_input_receiver(
    host: &mut LoggingHost<'_>,
    command: cs2_runtime::TaskCommand,
    buffers: &mut super::native_tasks::OwnedTaskBuffers,
    output: &mut u32,
    budget: usize,
) -> Result<Option<cs2_runtime::TaskCommandOutcome>> {
    host.receive_device_input_buffers_with_budget(command, buffers, output, budget)
}

fn invalid(message: &'static str) -> ScriptError {
    ScriptError::InvalidStructuredText { offset: 0, message }
}

fn masked_region_at(
    regions: &[super::InputRegionState],
    masks: &BTreeMap<u32, InputHitMask>,
    point: [i32; 2],
    ready: impl Fn(u32) -> bool,
) -> Result<Option<[u32; 15]>> {
    for region in regions {
        if region.words[8] == 0 || region.words[9] == 0 || !ready(region.id()) {
            continue;
        }
        let [left, top, right, bottom] = region.bounds();
        let hit = if region.words[11] != 0 {
            masks
                .get(&region.id())
                .ok_or_else(|| invalid("input selection's registered hit map is absent"))?
                .contains([left, top], point)
        } else {
            left < right && top < bottom && region.contains(point)
        };
        if hit {
            return Ok(Some(region.words));
        }
    }
    Ok(None)
}

impl LoggingHost<'_> {
    pub(super) fn effective_input_region_receiver(&self, id: u32) -> Option<u32> {
        match self.data_services.messages.input_buttons.get(&id) {
            Some(button) if button.registration_bound => button.state.receiver,
            _ => self.scene_nodes.get(&id)
                .and_then(|node| self.input_button_region_receiver(id, node.kind)),
        }
    }

    pub(super) fn host_input_region_ready(&self, id: u32) -> bool {
        !self.effective_input_region_receiver(id).and_then(|owner| self.data_services.tasks.fes.runtime(owner))
            .and_then(|runtime| runtime.driver.as_ref())
            .is_some_and(super::fes_driver::FesScriptDriver::initialization_pending)
    }
}

fn live_region_index(
    regions: &[super::InputRegionState],
    words: [u32; 15],
    previous: usize,
) -> Result<usize> {
    regions
        .get(previous)
        .filter(|region| region.words[0] == words[0])
        .map(|_| previous)
        .or_else(|| regions.iter().position(|region| region.words == words))
        .or_else(|| {
            let mut matches = regions
                .iter()
                .enumerate()
                .filter(|(_, region)| region.id() == words[0]);
            let first = matches.next().map(|(index, _)| index)?;
            matches.next().is_none().then_some(first)
        })
        .ok_or_else(|| invalid("input callback removed or replaced its current region"))
}

fn host_button(key: &cs2_runtime::InputKey) -> Option<(ButtonSource, u8)> {
    use cs2_runtime::InputKey;
    match key {
        InputKey::Keyboard(code) => Some((ButtonSource::Keyboard, *code as u8)),
        InputKey::MouseButton(button) => Some((
            ButtonSource::Mouse,
            match button {
                0 => 1, // left
                1 => 3, // right
                2 => 2, // middle
                3 => 4,
                4 => 5,
                _ => return None,
            },
        )),
        InputKey::MouseWheel(_) => None,
    }
}

fn read_inline_image_resource(
    resources: &cs2_runtime::ResourceIndex,
    filename: &[u8],
) -> Result<Option<Vec<u8>>> {
    let name = encoding_rs::SHIFT_JIS
        .decode_without_bom_handling_and_without_replacement(filename)
        .ok_or_else(|| invalid("inline message image filename is not valid CP932"))?;
    let names = resources
        .category_names(b"image", &name)
        .map_err(|_| invalid("inline message image category resolution failed"))?;
    for name in names {
        let Some(candidate) = resources.find(&name) else {
            continue;
        };
        if candidate.size > 64 * 1024 * 1024 {
            return Err(invalid("inline message image resource exceeds 64 MiB"));
        }
        return match resources.read_candidate(candidate) {
            Ok(bytes) if bytes.is_empty() => Ok(None),
            Ok(bytes) if bytes.len() <= 64 * 1024 * 1024 => Ok(Some(bytes)),
            Ok(_) => Err(invalid("inline message image resource exceeds 64 MiB")),
            Err(
                cs2_runtime::RuntimeError::ResourceNotFound(_)
                | cs2_runtime::RuntimeError::Io { .. },
            ) => Ok(None),
            Err(_) => Err(invalid("inline message image resource read failed")),
        };
    }
    Ok(None)
}

impl LoggingHost<'_> {
    /// Called for input130/135/136 and equivalent typed producer mutations.
    /// Registration, priority, selection and Alpha replacement do not call it.
    pub(super) fn mark_input_geometry_dirty(&mut self) {
        self.data_services.messages.input_geometry.mark_changed();
    }

    pub(super) fn install_device_input_receiver(&mut self) {
        // Keep an independently installed, more complete input receiver.
        if let Some(previous) = self
            .data_services
            .tasks
            .install_buffer_receiver(11, device_input_receiver)
        {
            self.data_services
                .tasks
                .install_buffer_receiver(11, previous);
        }
        for kind in [6, 2] {
            if let Some(previous) = self
                .data_services
                .tasks
                .install_buffer_receiver(kind, input_button_receiver)
            {
                self.data_services
                    .tasks
                    .install_buffer_receiver(kind, previous);
            }
        }
    }

    pub(super) fn receive_device_input_command(
        &mut self,
        command: cs2_runtime::TaskCommand,
        output: &mut u32,
    ) -> Result<Option<cs2_runtime::TaskCommandOutcome>> {
        self.receive_device_input_buffers(command, &mut Default::default(), output)
    }

    pub(super) fn receive_device_input_buffers(
        &mut self,
        command: cs2_runtime::TaskCommand,
        buffers: &mut super::native_tasks::OwnedTaskBuffers,
        output: &mut u32,
    ) -> Result<Option<cs2_runtime::TaskCommandOutcome>> {
        self.receive_device_input_buffers_with_budget(command, buffers, output, 4096)
    }

    fn receive_device_input_buffers_with_budget(
        &mut self,
        command: cs2_runtime::TaskCommand,
        buffers: &mut super::native_tasks::OwnedTaskBuffers,
        output: &mut u32,
        budget: usize,
    ) -> Result<Option<cs2_runtime::TaskCommandOutcome>> {
        if !matches!(command.code, 126..=139 | 143 | 145..=147 | 149..=154 | 156 | 158..=160) {
            return Ok(None);
        }
        let id = command.target as u32;
        if self
            .data_services
            .tasks
            .message_task_state(id)
            .is_none_or(|(kind, deletion, _)| kind != 11 || deletion != 0)
            || !self.input_task.alive
        {
            return Ok(None);
        }
        self.input_task.owner_id = Some(id);
        match command.code {
            143 | 159 => {
                return Ok(Some(cs2_runtime::TaskCommandOutcome {
                    done: self.advance_input_cursor_command(command, budget)?,
                    propagate: false,
                }));
            }
            126 => {
                let point = self.query_device_pointer()?;
                buffers.write_data0(&point[0].to_le_bytes())?;
                buffers.write_data1(&point[1].to_le_bytes())?;
            }
            127..=136 | 146 | 147 | 154 => {
                self.receive_input_region_command(command, buffers, output)?;
            }
            149 | 150 | 160 => {
                self.receive_input_cursor_command(command, buffers)?;
            }
            137 => {
                *output = match self.data_services.messages.input_event_mask {
                    16 => 2,
                    mask => mask,
                };
            }
            138 | 139 => {
                self.request_device_pointer(Some(id), command.code, command.data)?;
            }
            145 => {
                // Store each completed OR, retaining the prefix on error.
                *output = 0;
                for group in 0..4 {
                    *output |= self.sample_device_key(command.data[0], group, false)?.mask;
                }
            }
            151 => {
                self.data_services
                    .messages
                    .key_scan
                    .set_enabled(command.data[0], command.data[1])?;
                self.input_task.key_enabled[command.data[0] as usize] = command.data[1] != 0;
            }
            152 => {
                self.data_services.messages.key_scan.enable_all();
                self.input_task.key_enabled.fill(true);
            }
            153 => {
                *output = u32::from(self.data_services.messages.device_sample_active == 0);
            }
            156 => {
                self.data_services
                    .messages
                    .device_keys
                    .clear_source(ButtonSource::Touch);
                self.data_services
                    .messages
                    .physical_buttons
                    .retain(|(source, _)| *source != ButtonSource::Touch);
                self.input_task.key_masks = *self.data_services.messages.device_keys.masks();
            }
            158 => {
                self.data_services.messages.wheel_buttons.cancel_repeat();
                self.input_task.wheel_repeat = 0;
            }
            _ => unreachable!("device input command was filtered"),
        }
        Ok(Some(cs2_runtime::TaskCommandOutcome {
            done: true,
            propagate: false,
        }))
    }

    fn receive_input_cursor_command(
        &mut self,
        command: cs2_runtime::TaskCommand,
        buffers: &mut super::native_tasks::OwnedTaskBuffers,
    ) -> Result<()> {
        match command.code {
            149 => buffers.write_data1(
                &self
                    .data_services
                    .messages
                    .device_cursor
                    .shown
                    .to_le_bytes(),
            )?,
            150 => {
                self.data_services
                    .message_device
                    .show_device_cursor(command.data[1]);
                self.input_task.cursor_requested =
                    self.data_services.message_device.device_cursor_visible();
            }
            160 if buffers.data1().is_some() => {
                let snapshot = *self.data_services.messages.device_history.snapshot()?;
                buffers.write_data1(&snapshot)?;
            }
            160 if command.data[1] == 0 => {}
            160 => return Err(invalid("input160 history output buffer is absent")),
            _ => return Err(invalid("input cursor command is outside its receiver")),
        }
        Ok(())
    }

    fn advance_input_cursor_command(
        &mut self,
        command: cs2_runtime::TaskCommand,
        budget: usize,
    ) -> Result<bool> {
        let id = command.target as u32;
        let mut pending = self
            .data_services
            .messages
            .cursor_commands
            .remove(&id)
            .unwrap_or_else(|| InputCursorDispatch {
                command,
                initialized: false,
                broadcast: None,
            });
        if pending.command != command {
            self.data_services
                .messages
                .cursor_commands
                .insert(id, pending);
            return Err(invalid(
                "finish the pending cursor command before another configuration",
            ));
        }
        let result = (|| {
            if !pending.initialized {
                if command.code == 143 {
                    self.input_task.auto_hide = command.data[0];
                    self.input_task.idle_limit = self
                        .data_services
                        .messages
                        .device_cursor
                        .update_rate
                        .wrapping_mul(command.data[1])
                        / 60;
                }
                if self.input_task.auto_hide != 0 && self.input_task.cursor_requested != 0 {
                    // Automatic state changes before the device and callback.
                    // A nested input150 may change the manual readback.
                    self.data_services.messages.device_cursor.shown = 1;
                    self.data_services.message_device.show_device_cursor(1);
                    pending.broadcast = Some(CursorBroadcast {
                        visible: 1,
                        stack: vec![(0, false, None)],
                        visited: 0,
                    });
                }
                pending.initialized = true;
            }
            if let Some(broadcast) = &mut pending.broadcast {
                if !self.advance_cursor_broadcast(id, broadcast, budget)? {
                    return Ok(false);
                }
            }
            if command.code == 159 {
                self.data_services.messages.device_cursor.idle_count = 0;
            }
            Ok(true)
        })();
        if !matches!(result, Ok(true)) {
            self.data_services
                .messages
                .cursor_commands
                .insert(id, pending);
        }
        result
    }

    fn advance_device_cursor_frame(&mut self, budget: usize) -> Result<bool> {
        if self.data_services.messages.cursor_frame_done {
            return Ok(true);
        }
        let mut pending = self.data_services.messages.cursor_frame.take()
            .ok_or_else(|| invalid("input cursor frame has no captured pointer sample"))?;
        let result = (|| {
            if !pending.initialized {
                let auto = self.input_task.auto_hide != 0 && self.input_task.cursor_requested != 0;
                // Selection callbacks may issue input146. Read its state only
                // after they finish, and clear it after the cursor callback.
                let forced = self.data_services.messages.input_selection_forced;
                let cursor = &mut self.data_services.messages.device_cursor;
                let mut action = None;
                if pending.moving {
                    cursor.idle_count = 0;
                    if pending.original_nonzero && auto && cursor.shown == 0 {
                        action = Some((1, true));
                    }
                } else if pending.changed {
                    if pending.position_changed && !forced {
                        cursor.idle_count = 0;
                        if auto && cursor.shown == 0 {
                            action = Some((1, true));
                        }
                    }
                } else if pending.active != 0 && pending.foreground {
                    cursor.idle_count = cursor.idle_count.wrapping_add(1);
                    if auto && cursor.shown != 0 && cursor.idle_count >= self.input_task.idle_limit {
                        action = Some((0, true));
                    }
                } else if pending.active == 0 && !forced {
                    cursor.idle_count = 0;
                    if auto && cursor.shown == 0 {
                        action = Some((1, false));
                    }
                }
                if let Some((visible, notify)) = action {
                    cursor.shown = visible;
                    self.data_services.message_device.show_device_cursor(visible);
                    if notify {
                        pending.broadcast = Some(CursorBroadcast {
                            visible, stack: vec![(0, false, None)], visited: 0,
                        });
                    }
                }
                pending.initialized = true;
            }
            if let Some(broadcast) = &mut pending.broadcast {
                let sender = self.input_task.owner_id.unwrap_or(0);
                if !self.advance_cursor_broadcast(sender, broadcast, budget)? {
                    return Ok(false);
                }
            }
            Ok(true)
        })();
        if matches!(result, Ok(true)) {
            if pending.changed && !pending.moving {
                self.data_services.messages.input_selection_forced = false;
            }
            self.data_services.messages.input_selection_previous_device =
                Some(self.data_services.messages.device_task_position);
            self.data_services.messages.cursor_frame_done = true;
        } else {
            self.data_services.messages.cursor_frame = Some(pending);
        }
        result
    }

    /// Broadcast22 ignores consumption and walks live children in signed-id
    /// order. Ordinary tasks use their documented FFFF0014 VM notification,
    /// including C's real external-root continuation and active-VM guard.
    fn advance_cursor_broadcast(
        &mut self,
        sender: u32,
        pending: &mut CursorBroadcast,
        budget: usize,
    ) -> Result<bool> {
        let saved = (self.current_task_id, self.task_time, self.task_origin);
        self.current_task_id = sender;
        if let Some(clock) = self.data_services.tasks.task_clock(sender) {
            self.task_time = clock.current;
            self.task_origin = clock.origin;
        }
        let result = (|| {
            for _ in 0..budget {
                let Some((id, entered, child_cursor)) = pending.stack.last().copied() else {
                    return Ok(true);
                };
                if pending.stack.len() > 256 || pending.visited > 1_000_000 {
                    return Err(invalid(
                        "cursor22 notification exceeds task traversal bounds",
                    ));
                }
                if !entered {
                    if !self.task_is_reachable(id)
                        || (id != 0 && !self.data_services.tasks.task_is_registered(id))
                    {
                        pending.stack.pop();
                        continue;
                    }
                    let state = self.data_services.tasks.message_task_state(id);
                    if state.is_none_or(|(_, deletion, _)| deletion == 0) {
                        let done = if state.is_some_and(|(kind, _, _)| matches!(kind, 0 | 12)) {
                            self.send_fes_task_message(
                                id,
                                [sender, 0xffff_0014, 0, pending.visible],
                            )?
                        } else {
                            !self
                                .route_task_command_buffers(
                                    cs2_runtime::TaskCommand {
                                        target: id as i32,
                                        sender: sender as i32,
                                        code: 22,
                                        data: [0, pending.visible, 0],
                                    },
                                    &mut Default::default(),
                                    &mut 0,
                                    budget,
                                )?
                                .is_some_and(|outcome| !outcome.done)
                        };
                        if !done {
                            return Ok(false);
                        }
                    }
                    pending.visited += 1;
                    pending.stack.last_mut().unwrap().1 = true;
                    continue;
                }
                let child = self
                    .task_parents
                    .iter()
                    .filter_map(|(&child, &parent)| {
                        (parent == id
                            && child != id
                            && child_cursor.is_none_or(|last| (child as i32) > last)
                            && self.data_services.tasks.task_is_registered(child))
                        .then_some(child)
                    })
                    .min_by_key(|id| *id as i32);
                if let Some(child) = child {
                    pending.stack.last_mut().unwrap().2 = Some(child as i32);
                    pending.stack.push((child, false, None));
                } else {
                    pending.stack.pop();
                }
            }
            Ok(pending.stack.is_empty())
        })();
        (self.current_task_id, self.task_time, self.task_origin) = saved;
        result
    }

    pub(super) fn receive_input_region_command(
        &mut self,
        command: cs2_runtime::TaskCommand,
        buffers: &mut super::native_tasks::OwnedTaskBuffers,
        output: &mut u32,
    ) -> Result<()> {
        if matches!(command.code, 130 | 135 | 136) {
            self.mark_input_geometry_dirty();
        }
        let id = command.data[0];
        let at = self
            .input_task
            .regions
            .iter()
            .position(|region| region.id() == id);
        match command.code {
            127 => {
                let bytes = buffers
                    .data1()
                    .and_then(|bytes| bytes.get(..60))
                    .ok_or_else(|| invalid("input127 requires sixty owned region bytes"))?;
                let mut words: [u32; 15] = std::array::from_fn(|n| {
                    u32::from_le_bytes(bytes[n * 4..n * 4 + 4].try_into().unwrap())
                });
                words[1] = command.data[0];
                words[7] = 0;
                words[8] = 1;
                words[9] = 1;
                words[11..14].fill(0);
                words[14] = 1;
                if self.input_task.regions.len() >= 1_000_000 {
                    return Err(invalid("input region limit exceeded"));
                }
                self.input_task
                    .regions
                    .push(super::InputRegionState { words });
                self.input_task
                    .regions
                    .sort_by_key(|region| std::cmp::Reverse(region.words[1] as i32));
            }
            128 => {
                if let Some(at) = self
                    .input_task
                    .regions
                    .iter()
                    .position(|region| region.id() == command.data[1])
                {
                    let region = self.input_task.regions.remove(at);
                    self.data_services.messages.input_masks.remove(&region.id());
                }
            }
            129 => {
                if let Some(at) = at {
                    let mut region = self.input_task.regions.remove(at);
                    region.words[1] = command.data[1];
                    self.input_task.regions.push(region);
                    self.input_task
                        .regions
                        .sort_by_key(|region| std::cmp::Reverse(region.words[1] as i32));
                }
            }
            130 => {
                if let Some(at) = at {
                    let bounds = if let Some(bytes) = buffers.data1() {
                        let bytes = bytes
                            .get(..16)
                            .ok_or_else(|| invalid("input130 rectangle is truncated"))?;
                        std::array::from_fn(|n| {
                            u32::from_le_bytes(bytes[n * 4..n * 4 + 4].try_into().unwrap())
                        })
                    } else if command.data[1] == 0 {
                        [0; 4]
                    } else {
                        return Err(invalid("input130 rectangle buffer is absent"));
                    };
                    self.input_task.regions[at].words[3..7].copy_from_slice(&bounds);
                }
            }
            131 => {
                if let Some(at) = at {
                    if buffers.data1().is_some() {
                        let bytes: Vec<_> = self.input_task.regions[at].words[3..7]
                            .iter()
                            .flat_map(|value| value.to_le_bytes())
                            .collect();
                        buffers.write_data1(&bytes)?;
                    } else if command.data[1] != 0 {
                        return Err(invalid("input131 output rectangle is absent"));
                    }
                }
            }
            132 => {
                if let Some(at) = at {
                    self.input_task.regions[at].words[2] = command.data[1];
                }
            }
            133 => {
                for region in &mut self.input_task.regions {
                    if region.id() == id {
                        region.words[7] = command.data[1];
                    } else if command.data[1] != 0 {
                        region.words[7] = 0;
                    }
                }
            }
            134 => {
                if let Some(at) = at {
                    *output = self.input_task.regions[at].words[7];
                }
            }
            135 | 136 | 154 => {
                if let Some(at) = at {
                    let field = match command.code {
                        135 => 8,
                        136 => 9,
                        _ => 14,
                    };
                    self.input_task.regions[at].words[field] = command.data[1];
                }
            }
            146 => self.clear_input_region_selection(),
            147 => {
                self.replace_input_hit_mask(id, command.data[1])?;
            }
            _ => return Err(invalid("input region command is outside its receiver")),
        }
        Ok(())
    }

    fn clear_input_region_selection(&mut self) {
        for region in &mut self.input_task.regions {
            region.words[7] = 0;
        }
        self.input_task.selected_region = None;
        self.input_task.pointer_position = [-1; 2];
        self.data_services.messages.input_selection_forced = true;
        self.data_services.messages.input_selection_previous_device = Some([-1; 2]);
    }

    pub(super) fn replace_input_hit_mask(&mut self, id: u32, handle: u32) -> Result<()> {
        let Some(at) = self
            .input_task
            .regions
            .iter()
            .position(|region| region.id() == id)
        else {
            return Ok(());
        };
        if handle == 0 {
            if self.input_task.regions[at].words[11] != 0 {
                self.data_services.messages.input_masks.remove(&id);
                self.input_task.regions[at].words[11..14].fill(0);
            }
            return Ok(());
        }
        self.data_services.messages.input_masks.remove(&id);
        self.input_task.regions[at].words[11] = 0;
        // Task commands carry underlying pixel identities. Existing explicit
        // image adapters can still supply their typed wrapper handles.
        let wrapper = self
            .data_services
            .image_wrapper_pixels
            .iter()
            .find_map(|(&wrapper, &pixels)| {
                (pixels == handle && self.image_handles.contains_key(&wrapper)).then_some(wrapper)
            })
            .unwrap_or(handle);
        let image = self
            .image_handles
            .get(&wrapper)
            .ok_or_else(|| invalid("input147 source is not a live decoded image wrapper"))?;
        let metadata = image.metadata();
        let crop = [image.width(), image.height()];
        let size = InputHitMask::image_size(crop, metadata.canvas_size);
        self.input_task.regions[at].words[12..14].copy_from_slice(&size);
        let mask =
            InputHitMask::from_rgba8(crop, metadata.canvas_size, metadata.offset, image.pixels())?;
        self.data_services.messages.input_masks.insert(id, mask);
        self.input_task.regions[at].words[11] = 1;
        Ok(())
    }

    /// FES TYPE uses the exact image wrapper for this media declaration.
    /// Only a source-update with nonzero TYPE and readable pixels replaces
    /// the saved map; disabling TYPE or losing pixels preserves the old map.
    pub(super) fn sync_fes_button_hit_image(
        &mut self,
        id: u32,
        raw_type: u32,
        source_updated: bool,
        image: Option<&RgbaFrame>,
    ) -> Result<bool> {
        if raw_type == 0 || !source_updated {
            return Ok(false);
        }
        let Some(image) = image.filter(|image| !image.pixels().is_empty()) else {
            return Ok(false);
        };
        let Some(at) = self
            .input_task
            .regions
            .iter()
            .position(|region| region.id() == id)
        else {
            return Ok(false);
        };
        self.data_services.messages.input_masks.remove(&id);
        self.input_task.regions[at].words[11] = 0;
        let metadata = image.metadata();
        let crop = [image.width(), image.height()];
        let size = InputHitMask::image_size(crop, metadata.canvas_size);
        self.input_task.regions[at].words[12..14].copy_from_slice(&size);
        let mask =
            InputHitMask::from_rgba8(crop, metadata.canvas_size, metadata.offset, image.pixels())?;
        self.data_services.messages.input_masks.insert(id, mask);
        self.input_task.regions[at].words[11] = 1;
        Ok(true)
    }

    fn input_button_region_receiver(&self, id: u32, kind: u32) -> Option<u32> {
        match kind {
            2 => self.data_services.planes.input_receiver(id),
            6 => self.data_services.fes_controls.region_receiver(id)
                .or_else(|| {
                    self.data_services.messages.button_binding_lookup
                        .and_then(|lookup| lookup(self, id))
                })
                .or_else(|| {
                    self.data_services.messages.fes_button_lookup
                        .and_then(|lookup| lookup(self, id))
                        .map(|(owner, _)| owner)
                }),
            _ => None,
        }
    }

    pub(super) fn ensure_input_button(&mut self, id: u32) -> Result<bool> {
        let Some(kind) = self.scene_nodes.get(&id).map(|node| node.kind) else {
            return Ok(false);
        };
        if !matches!(kind, 2 | 6) {
            return Ok(false);
        }
        if self
            .data_services
            .tasks
            .message_task_state(id)
            .is_none_or(|(registered, deletion, _)| registered != kind || deletion != 0)
        {
            return Ok(false);
        }
        if !self.data_services.messages.input_buttons.contains_key(&id) {
            let mut state = InputButtonState::new(id);
            if let Some(region) = self
                .input_task
                .regions
                .iter()
                .find(|region| region.id() == id)
            {
                state.rectangle = region.bounds();
                state.priority = region.words[1] as i32;
                state.receiver = self.input_button_region_receiver(id, kind);
            }
            let registration_bound = state.receiver.is_some();
            self.data_services.messages.input_buttons.insert(
                id,
                InputButtonRuntime {
                    state,
                    began: None,
                    written: None,
                    registration_bound,
                },
            );
        }
        if !self.data_services.messages.input_buttons[&id].registration_bound
            && self
                .input_task
                .regions
                .iter()
                .any(|region| region.id() == id)
        {
            if let Some(receiver) = self.input_button_region_receiver(id, kind) {
                let button = self
                    .data_services
                    .messages
                    .input_buttons
                    .get_mut(&id)
                    .unwrap();
                button.state.receiver = Some(receiver);
                button.registration_bound = true;
            }
        }
        Ok(true)
    }

    /// Called only after the platform has actually registered this region.
    /// Repeated media updates preserve an already configured 97 receiver.
    pub(super) fn bind_input_button_owner(&mut self, id: u32, owner: u32) -> Result<bool> {
        let previously_bound = self
            .data_services
            .messages
            .input_buttons
            .get(&id)
            .is_some_and(|button| button.registration_bound);
        if !self
            .input_task
            .regions
            .iter()
            .any(|region| region.id() == id)
            || !self.ensure_input_button(id)?
        {
            return Ok(false);
        }
        if previously_bound {
            return Ok(false);
        }
        if owner == u32::MAX || !self.task_is_reachable(owner) {
            return Err(invalid("BUTTON region binding owner is unreachable"));
        }
        self.ensure_task_record(owner)?;
        let button = self
            .data_services
            .messages
            .input_buttons
            .get_mut(&id)
            .unwrap();
        button.state.receiver = Some(owner);
        button.state.response = 1;
        button.written = Some(self.frame);
        button.registration_bound = true;
        Ok(true)
    }

    /// Synchronous execution bridge for a FesTaskPlatform owner hook. `input`
    /// must come from its real region/object binding and raw 9/10/64/65
    /// controller; a queued driver's boolean is not a completion result.
    pub(super) fn dispatch_resolved_fes_button_event(
        &mut self,
        command: cs2_runtime::TaskCommand,
        input: super::fes_driver::FesDriverInput,
        response: Option<u32>,
        clear_selection: bool,
        budget: usize,
    ) -> Result<bool> {
        let owner = command.target as u32;
        if !matches!(command.code, 9 | 10 | 64 | 65)
            || self
                .data_services
                .tasks
                .message_task_state(owner)
                .is_none_or(|(kind, deleted, _)| kind != 34 || deleted != 0)
        {
            return Err(invalid(
                "resolved BUTTON event requires its live FES receiver",
            ));
        }
        if input.event.as_deref().is_none_or(|event| event.is_empty()) {
            return Err(invalid(
                "resolved BUTTON event needs an explicit original event name",
            ));
        }
        let mut pending = match self
            .data_services
            .messages
            .fes_button_dispatches
            .remove(&owner)
        {
            Some(pending) => {
                if pending.command != command
                    || pending.input.code != input.code
                    || pending.input.target != input.target
                    || pending.input.event != input.event
                    || pending.response != response
                    || pending.clear_selection != clear_selection
                {
                    self.data_services
                        .messages
                        .fes_button_dispatches
                        .insert(owner, pending);
                    return Err(invalid(
                        "resolved FES BUTTON continuation changed its event",
                    ));
                }
                pending
            }
            None => FesButtonDispatch {
                command,
                input,
                response,
                clear_selection,
                stage: 0,
            },
        };
        let result = (|| {
            for _ in 0..budget {
                match pending.stage {
                    0 => {
                        let notification = super::fes_controls::ControlNotification {
                            owner,
                            command: pending.command,
                            event: pending.input.clone(),
                        };
                        if !self.notify_fes_lower_control(&notification, budget)? {
                            return Ok(false);
                        }
                        pending.stage = 1;
                    }
                    1 => {
                        if let Some(response) = pending.response {
                            if !self.reply_input_button(command.sender as u32, response)? {
                                return Err(invalid(
                                    "completed FES BUTTON response target is absent",
                                ));
                            }
                        }
                        pending.stage = 2;
                    }
                    2 => {
                        if pending.clear_selection && self.input_task.alive {
                            // Host-provided input can own regions without a
                            // kind-11 script task. Apply the same state update as 146.
                            self.clear_input_region_selection();
                        }
                        pending.stage = 3;
                    }
                    _ => return Ok(true),
                }
            }
            Ok(pending.stage == 3)
        })();
        if !matches!(result, Ok(true)) {
            self.data_services
                .messages
                .fes_button_dispatches
                .insert(owner, pending);
        }
        result
    }

    /// Raw owner receiver for the documented basic BUTTON notifications.
    /// The lookup returns the platform's real record binding; event numbers
    /// are decoded from original command data, before any FES UI fallback.
    pub(super) fn receive_fes_button_owner(
        &mut self,
        command: cs2_runtime::TaskCommand,
        budget: usize,
    ) -> Result<Option<bool>> {
        let owner = command.target as u32;
        if !matches!(command.code, 9 | 10 | 64 | 65)
            || self
                .data_services
                .tasks
                .message_task_state(owner)
                .is_none_or(|(kind, deleted, _)| kind != 34 || deleted != 0)
        {
            return Ok(None);
        }
        let mut pending = match self.data_services.messages.fes_button_owners.remove(&owner) {
            Some(pending) => {
                if pending.command != command {
                    self.data_services
                        .messages
                        .fes_button_owners
                        .insert(owner, pending);
                    return Err(invalid(
                        "FES BUTTON owner continuation changed its original command",
                    ));
                }
                pending
            }
            None => self.prepare_fes_button_owner(command)?,
        };
        let result = (|| {
            for _ in 0..budget {
                match pending.stage {
                    0 => {
                        if let Some(navigation) = &mut pending.navigation {
                            if !super::fes_driver::main_flow::advance_navigation(self, owner, navigation, budget)
                                .map_err(|error| super::fes_driver::main_flow::input_error(self, owner, error))? {
                                return Ok(false);
                            }
                        }
                        if let Some((name, value)) = pending.named {
                            let hook =
                                self.data_services
                                    .messages
                                    .fes_button_named
                                    .ok_or_else(|| {
                                        invalid(
                                            "FES BUTTON named input needs its live driver adapter",
                                        )
                                    })?;
                            hook(self, owner, name, value)?;
                        }
                        pending.stage = 1;
                    }
                    1 => {
                        if pending.syskey && pending.input.is_none() {
                            if pending.record >= 1_000_000 {
                                return Err(invalid(
                                    "FES SYSKEY exceeds one million declaration records",
                                ));
                            }
                            let lookup = self
                                .data_services
                                .messages
                                .fes_button_records
                                .ok_or_else(|| {
                                    invalid("FES SYSKEY needs its ordered live declaration records")
                                })?;
                            let Some(record) = lookup(self, owner, pending.record)? else {
                                pending.stage = 2;
                                continue;
                            };
                            if record.enabled == 0
                                || record.syskey == -1
                                || record.syskey as u32 != command.data[1]
                            {
                                pending.record += 1;
                                continue;
                            }
                            let (target, object_handler, default_handler) =
                                self.resolve_fes_button_handler(owner, record.target, b"PUSH_L")?;
                            pending.record_object_handler = object_handler;
                            pending.record_default_handler = default_handler;
                            pending.input = Some(super::fes_driver::FesDriverInput {
                                code: 0,
                                target: Some(target),
                                event: Some(b"PUSH_L".to_vec()),
                            });
                        }
                        if let Some(input) = &pending.input {
                            let notification = super::fes_controls::ControlNotification {
                                owner,
                                command,
                                event: input.clone(),
                            };
                            if !self.notify_fes_lower_control(&notification, budget)? {
                                return Ok(false);
                            }
                        }
                        if pending.syskey {
                            if pending.record_object_handler {
                                pending.response = None;
                            }
                            if pending.record_object_handler || pending.record_default_handler {
                                pending.send_selection = false;
                            }
                            pending.input = None;
                            pending.record += 1;
                            continue;
                        }
                        pending.stage = 2;
                    }
                    2 => {
                        if let Some(response) = pending.response {
                            if !self.reply_input_button(command.sender as u32, response)? {
                                return Err(invalid("FES BUTTON owner response target is absent"));
                            }
                        }
                        pending.stage = 3;
                    }
                    3 => {
                        if pending.send_selection {
                            self.send_fes_button_selection(owner, command.sender as u32)?;
                        }
                        if pending.clear_selection && self.input_task.alive {
                            self.clear_input_region_selection();
                        }
                        pending.stage = 4;
                    }
                    4 => {
                        if pending.refresh {
                            let hook =
                                self.data_services
                                    .messages
                                    .fes_button_media
                                    .ok_or_else(|| {
                                        invalid(
                                            "FES BUTTON owner needs its real media update adapter",
                                        )
                                    })?;
                            if !hook(self, owner, budget)? {
                                return Ok(false);
                            }
                        }
                        pending.stage = 5;
                    }
                    _ => return Ok(true),
                }
            }
            Ok(pending.stage == 5)
        })();
        if !matches!(result, Ok(true)) {
            self.data_services
                .messages
                .fes_button_owners
                .insert(owner, pending);
        }
        result.map(Some)
    }

    fn send_fes_button_selection(&mut self, owner: u32, button: u32) -> Result<()> {
        let value = self.data_services.tasks.fes.runtime(owner)
            .and_then(|runtime| runtime.driver.as_ref())
            .ok_or_else(|| invalid("FES BUTTON selection needs its live interpreter"))?
            .send_button_selection();
        self.receive_input_button_command(
            cs2_runtime::TaskCommand {
                target: button as i32, sender: owner as i32, code: 94, data: [0, value, 1],
            },
            &mut Default::default(), &mut 0, 1,
        )?;
        Ok(())
    }

    fn prepare_fes_button_owner(
        &mut self,
        command: cs2_runtime::TaskCommand,
    ) -> Result<FesButtonOwnerDispatch> {
        let owner = command.target as u32;
        let lookup = self
            .data_services
            .messages
            .fes_button_lookup
            .ok_or_else(|| invalid("FES BUTTON needs the actual region/object binding lookup"))?;
        let matched = lookup(self, command.data[0])
            .filter(|(actual_owner, _)| *actual_owner == owner)
            .and_then(|(_, target)| target);
        let runtime = self
            .data_services
            .tasks
            .fes
            .runtime(owner)
            .ok_or_else(|| invalid("FES BUTTON owner has no loaded scene"))?;
        let driver = runtime
            .driver
            .as_ref()
            .ok_or_else(|| invalid("FES BUTTON owner driver is already executing"))?;
        let mut state = self
            .data_services
            .messages
            .fes_button_controllers
            .get(&owner)
            .cloned()
            .unwrap_or_else(|| FesButtonController {
                focus: driver.focus().cloned(),
                ..Default::default()
            });
        let enabled = |target: &super::fes_driver::FesDriverTarget| {
            let Some(definition) = usize::try_from(target.bank)
                .ok()
                .and_then(|bank| driver.adapter.object_bank_definition(&target.name, bank))
            else {
                return false;
            };
            let base = self.text_encoding.decode(&target.name);
            let key = if definition.bank_count == 1 {
                base
            } else {
                format!("{base}[{}]", target.bank)
            };
            runtime
                .media
                .get(&key)
                .is_some_and(|layer| layer.kind == "BUTTON" && layer.enabled)
        };
        let mut input = None;
        let mut response = None;
        let mut send_selection = false;
        let mut clear_selection = false;
        let mut refresh = true;
        let mut named = None;
        let mut syskey = false;
        let mut navigation = None;
        let mut event: Option<(
            Option<super::fes_driver::FesDriverTarget>,
            u32,
            &'static [u8],
        )> = None;
        match command.code {
            64 => {
                if let Some(target) = matched {
                    if command.data[1] != 0 {
                        state.focus = Some(target.clone());
                        event = Some((Some(target), 0, b"FOCUS"));
                    } else {
                        state.focus = None;
                        if state.focus_lock == 0 {
                            state.pressed = None;
                        }
                        event = Some((Some(target), 0, b"UNFOCUS"));
                    }
                } else if state.focus_lock == 0 {
                    state.focus = None;
                    state.pressed = None;
                }
            }
            65 => {
                refresh = matched.is_none();
                if let Some(target) = matched {
                    named = Some(("_TOUCH_", target.value));
                    event = Some((Some(target), 21, b"TOUCH"));
                }
            }
            9 | 10 => {
                send_selection = true;
                let key = command.data[1];
                let target = if command.code == 9 {
                    state.raw_state = match self.data_services.messages.input_event_mask {
                        16 => 2,
                        mask => mask,
                    };
                    state.controller_state = state.raw_state;
                    state.pressed = state
                        .focus
                        .as_ref()
                        .filter(|target| enabled(target))
                        .cloned();
                    state.pressed.clone()
                } else {
                    state.raw_state = 0;
                    state.pressed.clone()
                };
                let value = target.as_ref().map_or(-1, |target| target.value);
                match (command.code, key) {
                    (9, 0) => event = Some((target, 2, b"PUSH_L")),
                    (9, 1) => event = Some((target, 3, b"PUSH_R")),
                    (10, 0) => {
                        event = Some((target, 4, b"RELEASE_L"));
                        named = Some(("_CLICK_L_", 0));
                    }
                    (10, 1) => {
                        event = Some((target, 5, b"RELEASE_R"));
                        named = Some(("_CLICK_R_", 0));
                    }
                    (9, 8) => named = Some(("_SCRL_U_", value)),
                    (9, 9) => named = Some(("_SCRL_D_", value)),
                    (9, 10) => named = Some(("_PAGE_U_", value)),
                    (9, 11) => named = Some(("_PAGE_D_", value)),
                    (10, 4..=7) => named = Some((
                        ["_RELEASE_UP_", "_RELEASE_DOWN_", "_RELEASE_LEFT_", "_RELEASE_RIGHT_"][(key - 4) as usize],
                        value,
                    )),
                    (9, 2 | 3 | 12..=u32::MAX) => syskey = true,
                    _ => {}
                }
                if matches!(key, 4..=7) && command.code == 9 {
                    let (dispatch, reply) = super::fes_driver::main_flow::prepare_navigation(
                        self, owner, key, &mut state,
                    ).map_err(|error| super::fes_driver::main_flow::input_error(self, owner, error))?;
                    send_selection = dispatch.pointer.is_none() && dispatch.control.is_none();
                    navigation = Some(dispatch);
                    response = reply;
                } else {
                    response = Some(1);
                }
            }
            _ => unreachable!("FES BUTTON command was checked"),
        }
        if let Some((target, code, name)) = event {
            if let Some(target) = target {
                let (target, object_handler, default_handler) =
                    self.resolve_fes_button_handler(owner, target, name)?;
                response = if command.code == 64 {
                    None
                } else if object_handler {
                    None
                } else {
                    Some(1)
                };
                send_selection &= !object_handler && !default_handler;
                clear_selection = command.code == 10
                    && matches!(command.data[1], 0 | 1)
                    && (object_handler || default_handler);
                input = Some(super::fes_driver::FesDriverInput {
                    code,
                    target: Some(target),
                    event: Some(name.to_vec()),
                });
            } else if command.code == 9 {
                named = Some((
                    if command.data[1] == 0 {
                        "_CLICK_L_"
                    } else {
                        "_CLICK_R_"
                    },
                    -1,
                ));
            }
        }
        self.data_services
            .messages
            .fes_button_controllers
            .insert(owner, state);
        Ok(FesButtonOwnerDispatch {
            command,
            navigation,
            input,
            response,
            send_selection,
            clear_selection,
            refresh,
            named,
            syskey,
            record: 0,
            record_object_handler: false,
            record_default_handler: false,
            stage: 0,
        })
    }

    fn resolve_fes_button_handler(
        &self,
        owner: u32,
        mut target: super::fes_driver::FesDriverTarget,
        event: &[u8],
    ) -> Result<(super::fes_driver::FesDriverTarget, bool, bool)> {
        let driver = self
            .data_services
            .tasks
            .fes
            .runtime(owner)
            .and_then(|runtime| runtime.driver.as_ref())
            .ok_or_else(|| invalid("FES BUTTON handler needs its live interpreter"))?;
        if target.default_group.is_none() {
            if let Some(default) = driver
                .adapter
                .object_bank(&target.name, target.bank as usize)
                .and_then(|bank| {
                    bank.initializers
                        .iter()
                        .rev()
                        .find(|property| property.column.eq_ignore_ascii_case(b"DEFAULT"))
                })
            {
                let mut source = b"default ".to_vec();
                source.extend_from_slice(&default.value);
                target.default_group = driver
                    .interpreter
                    .program
                    .command_parameters(&source, self.string_values)?
                    .get(1)
                    .filter(|group| !group.is_empty())
                    .cloned();
            }
        }
        let exists = |group: &[u8]| {
            let mut header = group.to_vec();
            header.push(b'.');
            header.extend_from_slice(event);
            driver.interpreter.program.block(&header).is_some()
        };
        let object_handler = exists(&target.name);
        let default_handler = target.default_group.as_deref().is_some_and(exists);
        Ok((target, object_handler, default_handler))
    }

    /// Task Begin hook, before updating its clock/children. The state remains
    /// present during callbacks, allowing nested 95/96 to use the same slots.
    pub(super) fn begin_input_button_update(&mut self, id: u32) -> Result<()> {
        if self.ensure_input_button(id)? {
            let runtime = self
                .data_services
                .messages
                .input_buttons
                .get_mut(&id)
                .unwrap();
            if runtime.began != Some(self.frame) {
                runtime.state.begin_update();
                runtime.began = Some(self.frame);
            }
        }
        Ok(())
    }

    pub(super) fn reply_input_button(&mut self, id: u32, value: u32) -> Result<bool> {
        if !self.ensure_input_button(id)? {
            return Ok(false);
        }
        let runtime = self
            .data_services
            .messages
            .input_buttons
            .get_mut(&id)
            .unwrap();
        runtime.state.response = value;
        runtime.written = Some(self.frame);
        Ok(true)
    }

    /// Available only while this BUTTON is inside its synchronous callback.
    /// This is provenance, not a completion receipt or a FES event number.
    pub(super) fn active_input_button_event(&self, id: u32) -> Option<&ButtonInputContext> {
        self.data_services.messages.button_contexts.get(&id)
    }

    /// Plane property controllers can perform their state/lock processing,
    /// then delegate the original basic notification here without installing
    /// another response slot or inferring consumption from their owner.
    pub(super) fn dispatch_button_base_notification(
        &mut self,
        command: cs2_runtime::TaskCommand,
        buffers: &mut super::native_tasks::OwnedTaskBuffers,
        output: &mut u32,
        budget: usize,
    ) -> Result<Option<cs2_runtime::TaskCommandOutcome>> {
        if !matches!(command.code, 4..=8) || !self.ensure_input_button(command.target as u32)? {
            return Ok(None);
        }
        let done = self.dispatch_input_button_event(command, buffers, output, budget)?;
        Ok(Some(cs2_runtime::TaskCommandOutcome {
            done,
            propagate: false,
        }))
    }

    pub(super) fn receive_input_button_command(
        &mut self,
        command: cs2_runtime::TaskCommand,
        buffers: &mut super::native_tasks::OwnedTaskBuffers,
        output: &mut u32,
        budget: usize,
    ) -> Result<Option<cs2_runtime::TaskCommandOutcome>> {
        if let Some(outcome) = self.receive_legacy_plane_input91(command)? {
            return Ok(Some(outcome));
        }
        let id = command.target as u32;
        if !self.ensure_input_button(id)? {
            return Ok(None);
        }
        let kind = self.scene_nodes[&id].kind;
        if kind == 2 && !matches!(command.code, 4..=8 | 95..=98) {
            return Ok(None);
        }
        match command.code {
            4..=8 => {
                let done = self.dispatch_input_button_event(command, buffers, output, budget)?;
                return Ok(Some(cs2_runtime::TaskCommandOutcome {
                    done,
                    propagate: false,
                }));
            }
            90 => {
                *output = 0;
                if self.data_services.messages.input_buttons[&id]
                    .state
                    .receiver
                    .is_some()
                {
                    return Ok(Some(button_done()));
                }
                let bytes = buffers
                    .data1()
                    .and_then(|bytes| bytes.get(..24))
                    .ok_or_else(|| invalid("BUTTON90 needs six configuration DWORDs"))?;
                let words: [u32; 6] = std::array::from_fn(|n| {
                    u32::from_le_bytes(bytes[n * 4..n * 4 + 4].try_into().unwrap())
                });
                let (bounds, full) =
                    self.button_actual_bounds(std::array::from_fn(|n| words[n] as i32));
                let mut region = [0; 15];
                region[0] = id;
                region[1] = words[4];
                region[2] = 7;
                region[3..7].copy_from_slice(&bounds.map(|value| value as u32));
                region[8] = 1;
                region[9] = 1;
                region[14] = 1;
                if self.input_task.regions.len() >= 1_000_000 {
                    return Err(invalid("BUTTON region limit exceeded"));
                }
                self.input_task
                    .regions
                    .push(super::InputRegionState { words: region });
                self.input_task
                    .regions
                    .sort_by_key(|region| std::cmp::Reverse(region.words[1] as i32));
                let state = &mut self
                    .data_services
                    .messages
                    .input_buttons
                    .get_mut(&id)
                    .unwrap()
                    .state;
                state.rectangle = bounds;
                state.priority = words[4] as i32;
                state.parameter = words[5];
                state.receiver = Some(command.sender as u32);
                state.full_screen = full;
                state.response = 1;
                let button = self
                    .data_services
                    .messages
                    .input_buttons
                    .get_mut(&id)
                    .unwrap();
                button.written = Some(self.frame);
                button.registration_bound = true;
                *output = 1;
            }
            91 | 3 => {
                if let Some(at) = self
                    .input_task
                    .regions
                    .iter()
                    .position(|region| region.id() == id)
                {
                    self.input_task.regions.remove(at);
                }
                self.data_services.messages.input_masks.remove(&id);
                let button = self
                    .data_services
                    .messages
                    .input_buttons
                    .get_mut(&id)
                    .unwrap();
                button.state.receiver = None;
                button.registration_bound = false;
                if command.code == 91 {
                    button.state.response = 1;
                    button.written = Some(self.frame);
                } else {
                    self.request_ordinary_task_delete(id)?;
                }
            }
            93 | 94 | 99 | 100 => {
                if self.data_services.messages.input_buttons[&id]
                    .state
                    .receiver
                    .is_some()
                {
                    let code = match command.code {
                        93 => 135,
                        94 => 133,
                        99 => 136,
                        _ => 154,
                    };
                    self.receive_input_region_command(
                        cs2_runtime::TaskCommand {
                            code,
                            data: [id, command.data[1], 0],
                            ..command
                        },
                        buffers,
                        output,
                    )?;
                    if command.code == 93 && command.data[1] == 0 {
                        self.receive_input_region_command(
                            cs2_runtime::TaskCommand {
                                code: 133,
                                data: [id, 0, 0],
                                ..command
                            },
                            buffers,
                            output,
                        )?;
                    }
                }
            }
            95 => {
                let state = self
                    .data_services
                    .messages
                    .input_buttons
                    .get_mut(&id)
                    .unwrap();
                state.state.response = command.data[1];
                state.written = Some(self.frame);
            }
            96 => {
                *output = match self.data_services.messages.input_event_mask {
                    16 => 2,
                    value => value,
                }
            }
            97 => {
                let button = self
                    .data_services
                    .messages
                    .input_buttons
                    .get_mut(&id)
                    .unwrap();
                button.state.set_receiver(command.data[1]);
                button.registration_bound = true;
            }
            98 => self.replace_input_hit_mask(id, command.data[1])?,
            101 => {
                self.mark_input_geometry_dirty();
                let bytes = buffers
                    .data1()
                    .and_then(|bytes| bytes.get(..16))
                    .ok_or_else(|| invalid("BUTTON101 rectangle buffer is absent"))?;
                let (bounds, full) = self.button_actual_bounds(std::array::from_fn(|n| {
                    i32::from_le_bytes(bytes[n * 4..n * 4 + 4].try_into().unwrap())
                }));
                self.data_services
                    .messages
                    .input_buttons
                    .get_mut(&id)
                    .unwrap()
                    .state
                    .full_screen = full;
                if let Some(region) = self
                    .input_task
                    .regions
                    .iter_mut()
                    .find(|region| region.id() == id)
                {
                    region.words[3..7].copy_from_slice(&bounds.map(|value| value as u32));
                }
            }
            19 => {
                if self.data_services.messages.input_buttons[&id]
                    .state
                    .full_screen
                {
                    self.mark_input_geometry_dirty();
                    if let Some(region) = self
                        .input_task
                        .regions
                        .iter_mut()
                        .find(|region| region.id() == id)
                    {
                        region.words[3..7].copy_from_slice(
                            &[-100000_i32, -100000, 100000, 100000].map(|value| value as u32),
                        );
                    }
                }
            }
            47 | 48 => {
                let state = self.data_services.messages.input_buttons[&id].state.clone();
                if state.receiver.is_some() {
                    if command.code == 47 {
                        self.data_services
                            .messages
                            .input_buttons
                            .get_mut(&id)
                            .unwrap()
                            .state
                            .show = command.data[1];
                    }
                    let show = self.data_services.messages.input_buttons[&id].state.show;
                    let inherited = if command.code == 47 && state.parent_bound {
                        let parent = self
                            .task_parents
                            .get(&id)
                            .and_then(|parent| self.scene_nodes.get(parent))
                            .filter(|node| node.initialized);
                        if parent.is_none() {
                            return Ok(Some(button_done()));
                        }
                        parent.unwrap().visible as u32
                    } else {
                        command.data[1]
                    };
                    // BUTTON47/48 sends input135 even for an equal enable
                    // value or a missing region (Yukikoi136e31/136edd).
                    self.mark_input_geometry_dirty();
                    if let Some(region) = self
                        .input_task
                        .regions
                        .iter_mut()
                        .find(|region| region.id() == id)
                    {
                        region.words[8] = if command.code == 48 && show == 0 {
                            0
                        } else if state.parent_bound && show == 0 {
                            0
                        } else {
                            inherited
                        };
                        if command.code == 47 && inherited == 0 {
                            region.words[7] = 0;
                        }
                    }
                }
            }
            53 => {
                let bytes = buffers
                    .data1()
                    .and_then(|bytes| bytes.get(..12))
                    .ok_or_else(|| invalid("BUTTON53 offset buffer is absent"))?;
                let offset = std::array::from_fn(|n| {
                    f32::from_le_bytes(bytes[n * 4..n * 4 + 4].try_into().unwrap())
                });
                let state = &mut self
                    .data_services
                    .messages
                    .input_buttons
                    .get_mut(&id)
                    .unwrap()
                    .state;
                state.parent_offset = offset;
                state.parent_bound = true;
                self.rebind_input_button(id)?;
            }
            50 | 52 => self.rebind_input_button(id)?,
            54 => {
                self.data_services
                    .messages
                    .input_buttons
                    .get_mut(&id)
                    .unwrap()
                    .state
                    .parent_bound = false
            }
            _ => return Ok(None),
        }
        Ok(Some(button_done()))
    }

    fn button_actual_bounds(&self, bounds: [i32; 4]) -> ([i32; 4], bool) {
        let full = self.canvas.as_ref().is_some_and(|canvas| {
            bounds[0] <= 0
                && bounds[1] <= 0
                && i64::from(bounds[2]) >= i64::from(canvas.width())
                && i64::from(bounds[3]) >= i64::from(canvas.height())
        });
        (
            if full {
                [-100000, -100000, 100000, 100000]
            } else {
                bounds
            },
            full,
        )
    }

    fn rebind_input_button(&mut self, id: u32) -> Result<()> {
        let state = self.data_services.messages.input_buttons[&id].state.clone();
        if !state.parent_bound {
            return Ok(());
        }
        let offset = state.parent_offset;
        let parent = self.task_parents.get(&id).copied().unwrap_or(0);
        let mut position = [0u8; 12];
        let mut base = [0u8; 12];
        for (code, bytes) in [(62, &mut position), (61, &mut base)] {
            self.dispatch_plane_command(
                cs2_runtime::TaskCommand {
                    target: parent as i32,
                    sender: id as i32,
                    code,
                    data: [0; 3],
                },
                &mut super::plane_native::PlaneCommandBuffers {
                    data0: None,
                    data1: Some(bytes),
                },
                &mut 0,
            )?;
        }
        let coordinate = |axis: usize| {
            let position = f32::from_le_bytes(position[axis * 4..axis * 4 + 4].try_into().unwrap());
            let base = f32::from_le_bytes(base[axis * 4..axis * 4 + 4].try_into().unwrap());
            let value = (position + base) + offset[axis];
            if !value.is_finite()
                || value < i32::MIN as f32
                || f64::from(value) > f64::from(i32::MAX)
            {
                i32::MIN
            } else {
                value as i32
            }
        };
        let left = coordinate(0);
        let top = coordinate(1);
        let bounds = [
            left,
            top,
            left.wrapping_add(state.rectangle[2].wrapping_sub(state.rectangle[0])),
            top.wrapping_add(state.rectangle[3].wrapping_sub(state.rectangle[1])),
        ];
        self.data_services
            .messages
            .input_buttons
            .get_mut(&id)
            .unwrap()
            .state
            .rectangle = bounds;
        self.mark_input_geometry_dirty();
        if let Some(region) = self
            .input_task
            .regions
            .iter_mut()
            .find(|region| region.id() == id)
        {
            region.words[3..7].copy_from_slice(&bounds.map(|value| value as u32));
        }
        Ok(())
    }

    fn dispatch_input_button_event(
        &mut self,
        command: cs2_runtime::TaskCommand,
        buffers: &mut super::native_tasks::OwnedTaskBuffers,
        output: &mut u32,
        budget: usize,
    ) -> Result<bool> {
        let id = command.target as u32;
        let pending = self.data_services.messages.button_dispatches.remove(&id);
        let mut dispatch = if let Some(pending) = pending {
            if pending.command != command
                || (pending.original != *buffers && pending.buffers != *buffers)
            {
                self.data_services
                    .messages
                    .button_dispatches
                    .insert(id, pending);
                return Err(invalid(
                    "BUTTON synchronous event continuation changed its packet",
                ));
            }
            pending
        } else {
            if self.data_services.messages.button_active.contains(&id) {
                return Err(invalid(
                    "BUTTON event reentered its active synchronous callback",
                ));
            }
            if command.code == 4 && self.data_services.messages.input_route_receipts.len() >= 65_536
            {
                return Err(invalid(
                    "typed input route receipts exceed 65536 callbacks in one frame",
                ));
            }
            self.data_services
                .messages
                .button_event_completions
                .remove(&(id, command.code));
            let runtime = self
                .data_services
                .messages
                .input_buttons
                .get_mut(&id)
                .unwrap();
            if runtime.began != Some(self.frame) && runtime.written != Some(self.frame) {
                runtime.state.begin_update();
                runtime.began = Some(self.frame);
            }
            let receiver = runtime.state.receiver.unwrap_or(u32::MAX);
            let code = match command.code {
                4 => {
                    if command.data[1] != 0 {
                        9
                    } else {
                        10
                    }
                }
                5 => 65,
                6 => 6,
                _ => 64,
            };
            let data = match command.code {
                4 | 5 => [id, command.data[0], 0],
                6 => [command.data[0], command.data[1], 0],
                7 => [id, 1, 0],
                _ => [id, 0, 0],
            };
            if matches!(command.code, 4..=6) {
                *output = 1;
            }
            ButtonDispatch {
                command,
                forward: cs2_runtime::TaskCommand {
                    target: receiver as i32,
                    sender: id as i32,
                    code,
                    data,
                },
                buffers: buffers.clone(),
                pointer: self.input_task.pointer_position,
                notification_output: 1,
                original: buffers.clone(),
                group: self
                    .data_services
                    .messages
                    .input_key_context
                    .filter(|(key, _, region)| *key == command.data[0] && *region == id)
                    .map(|(_, group, _)| group),
                ordinary_buffers: None,
                owner_completed: false,
                raw_mask: 0,
                repeated: false,
                physical: Vec::new(),
            }
        };
        if dispatch.group.is_some() && dispatch.physical.is_empty() && dispatch.raw_mask == 0 {
            self.capture_button_physical_sources(&mut dispatch);
        }
        let saved = (
            self.current_task_id,
            self.task_time,
            self.task_origin,
            self.pointer_position,
        );
        self.current_task_id = id;
        self.pointer_position = dispatch.pointer;
        self.data_services.messages.button_active.insert(id);
        if command.code == 4 {
            self.data_services.messages.button_contexts.insert(
                id,
                ButtonInputContext {
                    key: command.data[0],
                    group: dispatch.group,
                    raw_mask: dispatch.raw_mask,
                    pressed: command.data[1],
                    repeated: dispatch.repeated,
                    physical: dispatch.physical.clone(),
                },
            );
        }
        if let Some(clock) = self.data_services.tasks.task_clock(id) {
            self.task_time = clock.current;
            self.task_origin = clock.origin;
        }
        let result = self.forward_input_button_event(&mut dispatch, budget);
        self.data_services.messages.button_active.remove(&id);
        self.data_services.messages.button_contexts.remove(&id);
        (
            self.current_task_id,
            self.task_time,
            self.task_origin,
            self.pointer_position,
        ) = saved;
        *buffers = dispatch.buffers.clone();
        match result {
            Ok(true) => {
                if dispatch.owner_completed && matches!(command.code, 5..=8) {
                    self.data_services
                        .messages
                        .button_event_completions
                        .insert((id, command.code));
                }
                if matches!(command.code, 4..=6) {
                    let response = self
                        .data_services
                        .messages
                        .input_buttons
                        .get_mut(&id)
                        .unwrap()
                        .state
                        .take_response();
                    *output = response;
                    self.record_button_input_route(&dispatch, response);
                }
                Ok(true)
            }
            result => {
                self.data_services
                    .messages
                    .button_dispatches
                    .insert(id, dispatch);
                result
            }
        }
    }

    fn forward_input_button_event(
        &mut self,
        dispatch: &mut ButtonDispatch,
        budget: usize,
    ) -> Result<bool> {
        let command = dispatch.forward;
        let target = command.target as u32;
        if !self.task_is_reachable(target) {
            return Ok(true);
        }
        self.ensure_task_record(target)?;
        let kind = self
            .data_services
            .tasks
            .message_task_state(target)
            .filter(|(_, deleted, _)| *deleted == 0)
            .map(|(kind, _, _)| kind);
        if kind == Some(55) && matches!(command.code, 9 | 10) {
            let done = self
                .dispatch_javascript_input(
                    target,
                    command.sender as u32,
                    cs2_runtime::javascript::JavaScriptInput {
                        value: command.data[1],
                        kind: (command.code == 9) as u32,
                        pointer: dispatch.pointer,
                    },
                    budget,
                )
                .map_err(|cause| self.task_error(target, cause))?;
            dispatch.owner_completed = done;
            return Ok(done);
        }
        if matches!(kind, Some(41 | 42)) && self.data_services.fes_controls.owns_lower(target) {
            let pointer = if command.code == 6 {
                let bytes = dispatch
                    .buffers
                    .data1()
                    .and_then(|bytes| bytes.get(..8))
                    .ok_or_else(|| invalid("lower control movement has no current shared point"))?;
                [
                    i32::from_le_bytes(bytes[..4].try_into().unwrap()),
                    i32::from_le_bytes(bytes[4..8].try_into().unwrap()),
                ]
            } else {
                dispatch.pointer
            };
            let pointer_sources: Vec<_> = dispatch
                .physical
                .iter()
                .filter(|button| matches!(button.source, ButtonSource::Mouse | ButtonSource::Touch))
                .collect();
            let left = (!pointer_sources.is_empty())
                .then(|| pointer_sources.iter().any(|button| button.code == 1));
            let context = super::fes_controls::LowerInputContext {
                button_mask: Some(self.data_services.messages.input_event_mask),
                left_button: left,
                pointer: Some(cs2_renderer::Point::new(pointer[0], pointer[1])),
                input_task: self.input_task.owner_id,
            };
            if let Some(outcome) = self.route_lower_control_input(
                command,
                &context,
                &mut dispatch.notification_output,
                budget,
            )? {
                dispatch.owner_completed = outcome.done;
                return Ok(outcome.done);
            }
        }
        if let Some(hook) = self.data_services.messages.button_owner_hook {
            if let Some(done) = hook(
                self,
                command,
                dispatch.pointer,
                &mut dispatch.buffers,
                &mut dispatch.notification_output,
                budget,
            )? {
                dispatch.owner_completed = done;
                return Ok(done);
            }
        }
        if matches!(kind, Some(0 | 12)) && matches!(command.code, 6 | 9 | 10 | 64 | 65) {
            if dispatch.ordinary_buffers.is_none() {
                let packed_point = |bytes: Option<&[u8]>| -> Result<u32> {
                    let bytes = bytes.and_then(|bytes| bytes.get(..8)).ok_or_else(|| {
                        invalid("BUTTON movement needs both logical point buffers")
                    })?;
                    let x = u32::from_le_bytes(bytes[..4].try_into().unwrap());
                    let y = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
                    Ok((x & 0xffff) | ((y & 0xffff) << 16))
                };
                let code = match command.code {
                    6 => 0xffff0002,
                    9 => 0xffff0003,
                    10 => 0xffff0004,
                    64 => 0xffff0005,
                    _ => 0xffff0017,
                };
                let data = if command.code == 6 {
                    [
                        packed_point(dispatch.buffers.data0())?,
                        packed_point(dispatch.buffers.data1())?,
                    ]
                } else {
                    [command.data[0], command.data[1]]
                };
                let bytes = [command.sender as u32, code, data[0], data[1]]
                    .into_iter()
                    .flat_map(u32::to_le_bytes)
                    .collect();
                dispatch.ordinary_buffers = Some(super::native_tasks::OwnedTaskBuffers::new(
                    None,
                    Some(bytes),
                )?);
            }
            let result = self.route_task_command_buffers(
                cs2_runtime::TaskCommand {
                    code: 173,
                    data: [0, 1, 0],
                    ..command
                },
                dispatch.ordinary_buffers.as_mut().unwrap(),
                &mut dispatch.notification_output,
                budget,
            )?;
            dispatch.owner_completed = result.is_some_and(|outcome| outcome.done);
            return Ok(result.is_none_or(|outcome| outcome.done));
        }
        if kind == Some(34) && matches!(command.code, 9 | 10 | 64 | 65) {
            let done = self
                .receive_fes_button_owner(command, budget)?
                .ok_or_else(|| invalid("FES BUTTON synchronous receiver rejected its owner"))?;
            dispatch.owner_completed = done;
            return Ok(done);
        }
        if kind.is_none() {
            return Ok(true);
        }
        let mut second = dispatch.buffers.data1().map(<[u8]>::to_vec);
        let result = self.receive_task_command(
            command,
            &mut cs2_runtime::TaskCommandBuffers {
                data0: dispatch.buffers.data0(),
                data1: second.as_deref_mut(),
            },
            &mut dispatch.notification_output,
            budget,
        );
        if let Some(bytes) = second {
            dispatch.buffers.write_data1(&bytes)?;
        }
        let result = result?;
        dispatch.owner_completed = result.is_some_and(|outcome| outcome.done && !outcome.propagate);
        Ok(result.is_none_or(|outcome| outcome.done))
    }

    fn capture_button_physical_sources(&self, dispatch: &mut ButtonDispatch) {
        if dispatch.command.code != 4 {
            return;
        }
        let Some(group) = dispatch.group else {
            return;
        };
        let key = dispatch.command.data[0] as usize;
        let messages = &self.data_services.messages;
        let Some(binding) = messages.device_keys.bindings().get(key) else {
            return;
        };
        let Some(state) = messages
            .key_scan
            .records()
            .get(key)
            .and_then(|row| row.get(group as usize))
        else {
            return;
        };
        dispatch.raw_mask = state.mask;
        dispatch.repeated = state.count & 0x8000_0000 != 0;
        if !messages.key_sample_physical.contains(&(key as u32, group)) {
            return;
        }
        let pressed = dispatch.command.data[1] != 0;
        let mut candidates = std::collections::BTreeSet::new();
        if pressed {
            candidates.extend(messages.physical_buttons.iter().copied());
            candidates.extend(
                messages
                    .physical_edges
                    .iter()
                    .filter(|(_, _, value)| *value != 0)
                    .map(|&(source, code, _)| (source, code)),
            );
        } else {
            candidates.extend(
                messages
                    .physical_edges
                    .iter()
                    .filter(|(_, _, value)| *value == 0)
                    .map(|&(source, code, _)| (source, code)),
            );
            candidates.extend(
                messages
                    .typed_physical_owners
                    .iter()
                    .filter(|(_, (saved_key, saved_group, _))| {
                        *saved_key == key as u32 && *saved_group == group
                    })
                    .map(|(&button, _)| button),
            );
        }
        for (source, code) in candidates {
            let matched = match source {
                ButtonSource::Mouse | ButtonSource::Touch => {
                    binding.codes[1][group as usize] == code
                        || binding.codes[3][group as usize] == code
                }
                _ => binding.codes[source.index()][group as usize] == code,
            };
            if state.mask & source.mask() != 0 && matched {
                dispatch
                    .physical
                    .push(RoutedPhysicalButton { source, code });
            }
        }
    }

    fn record_button_input_route(&mut self, dispatch: &ButtonDispatch, response: u32) {
        if dispatch.command.code != 4 || !dispatch.owner_completed {
            return;
        }
        let Some(group) = dispatch.group else {
            return;
        };
        let messages = &mut self.data_services.messages;
        let key = dispatch.command.data[0];
        let receiver = dispatch.forward.target as u32;
        let pressed = dispatch.command.data[1];
        for physical in &dispatch.physical {
            let button = (physical.source, physical.code);
            if pressed != 0 {
                messages
                    .typed_physical_owners
                    .insert(button, (key, group, receiver));
            } else if messages.typed_physical_owners.get(&button).is_some_and(
                |&(saved_key, saved_group, _)| saved_key == key && saved_group == group,
            ) {
                messages.typed_physical_owners.remove(&button);
            }
        }
        messages.input_route_receipts.push(InputRouteReceipt {
            frame: self.frame,
            key,
            group,
            raw_mask: dispatch.raw_mask,
            pressed,
            repeated: dispatch.repeated,
            region: dispatch.command.target as u32,
            receiver,
            response,
            physical: dispatch.physical.clone(),
        });
    }

    pub(super) fn call_message_native(
        &mut self,
        index: u32,
        arguments: &[u8],
        vm: &mut Vm,
    ) -> Result<NativeReturn> {
        match index {
            77 => return self.root_task_number_native(arguments),
            740 => return self.message_lower_edge_native(arguments, vm),
            slot if SOUND_PREPARE_NATIVES.iter().any(|entry| entry.0 == slot) => {
                return self.legacy_sound_prepare_native(slot, arguments, vm);
            }
            slot if INPUT_REGION_NATIVES.iter().any(|entry| entry.0 == slot) => {
                return self.input_region_native(slot, arguments, vm);
            }
            149 => return self.device_pointer_query_native(arguments, vm),
            81 | 82 | 346 => return self.kx_native(index, arguments, vm),
            204 | 206 => return Self::message_matrix_native(index, arguments, vm),
            index if BUTTON_NATIVES.iter().any(|spec| spec.0 == index) => {
                return self.input_button_native(index, arguments);
            }
            150 => return self.move_device_pointer_native(arguments),
            394 => return self.input_hit_image_native(arguments),
            403 => {
                if arguments.len() != 8 {
                    return Err(invalid(
                        "native403 requires a logical key and raw enable value",
                    ));
                }
                if !self.input_task.alive {
                    return Ok(NativeReturn::Continue);
                }
                let key = super::read_u32(arguments, 0)?;
                let value = super::read_u32(arguments, 4)?;
                self.data_services
                    .messages
                    .key_scan
                    .set_enabled(key, value)?;
                self.input_task.key_enabled[key as usize] = value != 0;
                return Ok(NativeReturn::Continue);
            }
            404 => {
                if !self.input_task.alive {
                    return Ok(NativeReturn::Continue);
                }
                self.data_services.messages.key_scan.enable_all();
                self.input_task.key_enabled.fill(true);
                return Ok(NativeReturn::Continue);
            }
            937 => {
                if !self.input_task.alive {
                    return Ok(NativeReturn::Continue);
                }
                self.data_services.messages.wheel_buttons.cancel_repeat();
                self.input_task.wheel_repeat = 0;
                return Ok(NativeReturn::Continue);
            }
            _ => {}
        }
        if index == 285 {
            if arguments.len() != 4 {
                return Err(invalid("native285 requires one DWORD logical key slot"));
            }
            if !self.input_task.alive {
                return Ok(NativeReturn::Value(0));
            }
            if !self.data_services.messages.device_keys_loaded {
                self.data_services.message_device.load_fonts(self.resources);
                initialize_settings(self.data_services, self.resources, Some(self.save_root))?;
            }
            let key = super::read_u32(arguments, 0)?;
            let mut value = 0;
            for group in 0..4 {
                value |= self.sample_device_key(key, group, false)?.mask;
            }
            return Ok(NativeReturn::Value(value));
        }
        if index == 92 {
            return self.message_visibility_native(arguments, vm);
        }
        if index == 69 {
            return self.message_raw_display_native(arguments, vm);
        }
        if index == 70 {
            if arguments.len() != 16 {
                return Err(invalid("native70 requires four DWORDs"));
            }
            if let Some(result) = self.kx_raw_native(arguments, vm, true)? {
                return Ok(result);
            }
            if let Some(result) = self.input_graphic_raw_native(arguments, vm, true)? {
                return Ok(result);
            }
            let target = super::read_u32(arguments, 0)?;
            let packet = [
                self.current_task_id,
                super::read_u32(arguments, 4)?,
                super::read_u32(arguments, 8)?,
                super::read_u32(arguments, 12)?,
            ];
            let mut output = 0;
            if self
                .dispatch_buffered_text_message(target, packet, true, &mut output, Some(&mut *vm))?
                .is_some()
            {
                return Ok(NativeReturn::Value(output));
            }
            return self.task_message_native(index, arguments, vm);
        }
        if super::message_native::is_body_native(index) {
            let origins = &mut self.data_services.cst_text_origins;
            let task = self.current_task_id;
            let legacy_gbk = index == 392 && super::read_u32(arguments, 4).ok().is_some_and(|address| {
                let gbk = super::ScriptTextEncoding::Gbk;
                if let Some((_, codec)) = origins.sourced_vm_string(task, address, vm) {
                    return codec == gbk;
                }
                // Kanmi's speaker handler copies the CST buffer into a local
                // stack buffer in KCS bytecode before invoking native 392.
                vm.instruction_pointer == 0x11b8
                    && vm.program_code_sha256() == Some(super::story_trace::story_trace_09::SSCRIPT_CODE)
                    && origins.sourced_vm_string(task, 0x5d70a, vm).is_some_and(|(record, codec)| {
                        codec == gbk && vm.read_string(address).is_ok_and(|bytes| bytes == record)
                    })
            });
            let result = super::message_native::call_message_body_native(index, arguments, vm, legacy_gbk)?;
            let target = super::read_u32(arguments, 0)?;
            origins.invalidate_returned(task, target);
            if legacy_gbk && matches!(result, NativeReturn::Value(length) if length > 0) {
                origins.record(task, target, &vm.read_string(target)?, super::ScriptTextEncoding::Gbk);
            }
            return Ok(result);
        }
        let empty_source = match index {
            136 => arguments.len() >= 8 && arguments.len() % 4 == 0,
            620 => arguments.len() == 8,
            _ => false,
        } && arguments[4..8] == [0; 4];
        if empty_source {
            return Ok(NativeReturn::Continue);
        }
        let target = arguments
            .get(..4)
            .map(|word| u32::from_le_bytes(word.try_into().unwrap()));
        let utate_string_receiver = matches!(index, 611..=621)
            && target
                .and_then(|id| self.data_services.tasks.message_task_state(id))
                .is_some_and(|(kind, _, _)| kind == UTATE_STRING_KIND)
            && self.bind_utate_plane_input_profile()?;
        // Both PEs use a 0xbc config and the kind-45 layout receiver for this group.
        let legacy_string_receiver_45 = matches!(index, 611..=619)
            && matches!(self.css_task_sha(), Some(super::particle_external::CAMPANELLA_SHA) | Some(super::css_model::SHA))
            && target
                .and_then(|id| self.data_services.tasks.message_task_state(id))
                .is_some_and(|(kind, _, _)| kind == LEGACY_STRING_KIND_45);
        self.data_services.message_device.load_fonts(self.resources);
        if !self.data_services.message_device.settings_loaded {
            initialize_settings(self.data_services, self.resources, Some(self.save_root))?;
        }
        let data = &mut *self.data_services;
        let mut context = MessagePlatformContext {
            device: &mut data.message_device,
            tasks: &data.tasks,
            ordinary: &data.ordinary,
            images: self.image_handles,
            nodes: self.scene_nodes,
            parents: self.task_parents,
            sender: self.current_task_id,
            drawing_available: self.canvas.is_some(),
            utate_string_receiver,
            legacy_string_receiver_45,
            legacy_text_decode_offset: None,
        };
        let complete_before_utate_append = data.story_trace && utate_string_receiver && index == 614
            && target.and_then(|id| data.messages.object(id))
                .is_some_and(|object| object.complete_snapshot() && object.last_complete() && !object.active());
        let result = data
            .messages
            .call(index, arguments, vm, self.scene_nodes, &mut context);
        // Only receiver branches requesting real preparation set the pending
        // flag. Queries and passive settings leave it untouched.
        if result.is_ok()
            && arguments.len() >= 4
            && super::legacy_text_native::LegacyTextNativeServices::paints_after_call(index)
        {
            let id = u32::from_le_bytes(arguments[..4].try_into().unwrap());
            data.messages.legacy.paint_pending(id, context.device)?;
        }
        if let Some(offset) = context.legacy_text_decode_offset {
            data.note_text_decode(self.events, self.frame, format!(
                "legacy StringPlane command=104 codec=Cp932 bad_at={offset}",
            ));
        }
        if data.story_trace && utate_string_receiver
            && self.cst_contexts.values().any(|cst| cst.story.is_scene("00_01"))
            && (matches!(index, 612..=615 | 617..=619) && result.is_ok()
                || index == 616 && matches!(&result, Ok(NativeReturn::Value(1))))
            && let Some(id) = target
            && let Some(object) = data.messages.object(id)
        {
            let caller = self.current_task_id;
            let configured = object.configured();
            let remaining = configured.then(|| object.remaining_all().ok()).flatten();
            let append = (index == 614 && configured).then(|| {
                let address = super::read_u32(arguments, 4).ok()?;
                let raw = vm.read_string(address).ok()?;
                let expanded = super::expand_kcs_format(vm, &raw, 0).ok()?;
                let appended = expanded.split(|&byte| byte == 0).next().unwrap_or_default();
                let unchanged = raw.as_slice() == appended;
                let source = unchanged.then(|| data.cst_text_origins.returned_body(caller, address, appended)).flatten();
                Some((address, raw.len(), appended.len(), unchanged, source))
            }).flatten();
            self.events.push(crate::LogicEvent {
                frame: self.frame,
                kind: "utate-string-receiver".into(),
                detail: format!("native={index} id={id} caller={caller} configured={configured} \
                    remaining={remaining:?} generation={} return_one={} \
                    append(address,raw_bytes,appended_bytes,unchanged,source)={append:?}",
                    object.surface_generation(), matches!(&result, Ok(NativeReturn::Value(1)))),
            });
        }
        if data.story_trace && utate_string_receiver && result.is_ok()
            && let Some(id) = target
        {
            let caller = self.current_task_id;
            let object = data.messages.object(id);
            let generation = object.map(|object| object.surface_generation());
            let remaining = object.filter(|object| object.configured())
                .and_then(|object| object.remaining_all().ok());
            let newline_tail = object.is_some_and(|object| {
                matches!(object.raw_appends(), [body, tail] if !body.is_empty() && tail == b"\\n")
            });
            let same_draw = |draw: &super::message_native::UtateBodyDraw| {
                draw.caller == caller && Some(draw.generation) == generation
            };
            match index {
                614 if newline_tail && data.messages.utate_body_draws.get(&id).is_some_and(same_draw) => {
                    if let Some(draw) = data.messages.utate_body_draws.get_mut(&id) {
                        draw.completed_before_newline = complete_before_utate_append && draw.completed.is_some();
                    }
                }
                619 if newline_tail && remaining == Some(0)
                    && data.messages.utate_body_draws.get(&id).is_some_and(same_draw) => {}
                612..=615 | 617..=621 => {
                    data.messages.utate_body_draws.remove(&id);
                    if index == 619 && matches!(remaining, Some(1..)) && let Some(generation) = generation {
                        data.messages.utate_body_draws.insert(id, super::message_native::UtateBodyDraw {
                            caller, generation, started: self.frame, completed: None, completed_before_newline: false,
                        });
                    }
                }
                616 if remaining == Some(0) && matches!(&result, Ok(NativeReturn::Value(1))) => {
                    if let Some(draw) = data.messages.utate_body_draws.get_mut(&id)
                        && draw.caller == caller && Some(draw.generation) == generation
                        && draw.started < self.frame
                    {
                        draw.completed.get_or_insert(self.frame);
                    }
                }
                _ => {}
            }
        }
        if result.is_ok() && index == 612 && (utate_string_receiver || legacy_string_receiver_45) {
            let id = target.unwrap();
            if let Some((size, _)) = data.messages.object(id)
                .and_then(|object| object.registered_geometry())
                && let Some(node) = self.scene_nodes.get_mut(&id)
            {
                node.size = size.map(|extent| extent as f32);
                node.initialized = true;
            }
        }
        result
    }

    fn input_button_native(&mut self, index: u32, arguments: &[u8]) -> Result<NativeReturn> {
        let &(_, words, code, route_zero, return_output) = BUTTON_NATIVES
            .iter()
            .find(|spec| spec.0 == index)
            .ok_or(ScriptError::UnimplementedNativeCall { index })?;
        let target = super::read_u32(arguments, 0)?;
        if target == 0 && !route_zero {
            return Ok(if return_output {
                NativeReturn::Value(0)
            } else {
                NativeReturn::Continue
            });
        }
        if arguments.len() < words * 4 || arguments.len() % 4 != 0 {
            return Err(invalid(
                "BUTTON native is missing its documented DWORD prefix",
            ));
        }
        self.install_device_input_receiver();
        let data = if index == 104 {
            [0; 3]
        } else if words == 2 {
            [0, super::read_u32(arguments, 4)?, 0]
        } else {
            [0; 3]
        };
        let command = cs2_runtime::TaskCommand {
            target: target as i32,
            sender: self.current_task_id as i32,
            code,
            data,
        };
        let mut buffers = if let Some(saved) = self.pending_owned_task_buffers(command) {
            saved
        } else if index == 104 {
            let left = super::read_u32(arguments, 4)?;
            let top = super::read_u32(arguments, 8)?;
            let right = left.wrapping_add(super::read_u32(arguments, 12)?);
            let bottom = top.wrapping_add(super::read_u32(arguments, 16)?);
            let bytes = [
                left,
                top,
                right,
                bottom,
                super::read_u32(arguments, 20)?,
                super::read_u32(arguments, 24)?,
            ]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect();
            super::native_tasks::OwnedTaskBuffers::new(None, Some(bytes))?
        } else {
            Default::default()
        };
        let mut output = 0;
        let result = self.route_task_command_buffers(command, &mut buffers, &mut output, 4096)?;
        Ok(if result.is_some_and(|outcome| !outcome.done) {
            NativeReturn::Retry
        } else if return_output {
            NativeReturn::Value(output)
        } else {
            NativeReturn::Continue
        })
    }

    fn input_hit_image_native(&mut self, arguments: &[u8]) -> Result<NativeReturn> {
        if arguments.len() < 8 || arguments.len() % 4 != 0 {
            return Err(invalid(
                "native394 requires its target and image-object DWORD prefix",
            ));
        }
        let target = super::read_u32(arguments, 0)?;
        let wrapper = super::read_u32(arguments, 4)?;
        // The native calls the image object's getter even when target is zero.
        // Underlying pixel identity is a distinct allocator type.
        let pixels = if wrapper == 0 {
            0
        } else {
            if !self.image_handles.contains_key(&wrapper) {
                return Err(invalid("native394 source is not a live typed image object"));
            }
            *self
                .data_services
                .image_wrapper_pixels
                .get(&wrapper)
                .ok_or_else(|| invalid("native394 image object has no typed pixel identity"))?
        };
        let command = cs2_runtime::TaskCommand {
            target: target as i32,
            sender: self.current_task_id as i32,
            code: 98,
            data: [0, pixels, 0],
        };
        self.install_device_input_receiver();
        let mut buffers = self.pending_owned_task_buffers(command).unwrap_or_default();
        let mut output = 0;
        let result = self.route_task_command_buffers(command, &mut buffers, &mut output, 4096)?;
        Ok(if result.is_some_and(|outcome| !outcome.done) {
            NativeReturn::Retry
        } else {
            NativeReturn::Continue
        })
    }

    fn message_matrix_native(index: u32, arguments: &[u8], vm: &mut Vm) -> Result<NativeReturn> {
        if arguments.len() < 16 || arguments.len() % 4 != 0 {
            return Err(invalid(
                "matrix native requires destination and three float DWORDs",
            ));
        }
        let address = super::read_u32(arguments, 0)?;
        let floats: [f32; 3] = std::array::from_fn(|axis| {
            f32::from_le_bytes(arguments[4 + axis * 4..8 + axis * 4].try_into().unwrap())
        });
        let bytes = vm.read(address, 64)?;
        let mut matrix: [f32; 16] = std::array::from_fn(|n| {
            f32::from_le_bytes(bytes[n * 4..n * 4 + 4].try_into().unwrap())
        });
        let identity = || std::array::from_fn(|n| if n / 4 == n % 4 { 1.0 } else { 0.0 });
        let multiply = |left: [f32; 16], right: [f32; 16]| -> [f32; 16] {
            std::array::from_fn(|n| {
                let row = n / 4;
                let column = n % 4;
                let first = left[row * 4] * right[column];
                let second = left[row * 4 + 1] * right[4 + column];
                let third = left[row * 4 + 2] * right[8 + column];
                let fourth = left[row * 4 + 3] * right[12 + column];
                ((first + second) + third) + fourth
            })
        };
        if index == 206 {
            let mut scale: [f32; 16] = identity();
            for axis in 0..3 {
                scale[axis * 5] = floats[axis];
            }
            matrix = multiply(matrix, scale);
            vm.write(
                address,
                &matrix
                    .into_iter()
                    .flat_map(f32::to_le_bytes)
                    .collect::<Vec<_>>(),
            )?;
        } else {
            // Each original helper writes back before the next axis runs.
            for (axis, angle) in floats.into_iter().enumerate() {
                let (sine, cosine) = angle.sin_cos();
                let mut rotation: [f32; 16] = identity();
                let (first, second) = match axis {
                    0 => (1, 2),
                    1 => (2, 0),
                    _ => (0, 1),
                };
                rotation[first * 5] = cosine;
                rotation[second * 5] = cosine;
                rotation[first * 4 + second] = sine;
                rotation[second * 4 + first] = -sine;
                matrix = multiply(matrix, rotation);
                vm.write(
                    address,
                    &matrix
                        .into_iter()
                        .flat_map(f32::to_le_bytes)
                        .collect::<Vec<_>>(),
                )?;
            }
        }
        Ok(NativeReturn::Continue)
    }

    fn input_graphic_raw_native(
        &mut self,
        arguments: &[u8],
        vm: &mut Vm,
        queued: bool,
    ) -> Result<Option<NativeReturn>> {
        let id = super::read_u32(arguments, 0)?;
        let code = super::read_u32(arguments, 4)?;
        let kind = self.scene_nodes.get(&id).map(|node| node.kind);
        let supported = match kind {
            Some(11) => matches!(code,126..=139|143|145..=147|149..=154|156|158..=160),
            Some(6) => matches!(code,3..=8|19|47|48|50|52..=54|90|91|93..=101),
            Some(2) => matches!(code,4..=8|95..=98),
            _ => false,
        };
        if !supported {
            return Ok(None);
        }
        self.install_device_input_receiver();
        let data = [
            super::read_u32(arguments, 8)?,
            super::read_u32(arguments, 12)?,
            0,
        ];
        let command = cs2_runtime::TaskCommand {
            target: id as i32,
            sender: self.current_task_id as i32,
            code,
            data,
        };
        let pointer_query = kind == Some(11) && code == 126;
        let live_input = kind == Some(11)
            && self.input_task.alive
            && self
                .data_services
                .tasks
                .message_task_state(id)
                .is_some_and(|(_, deletion, _)| deletion == 0);
        let query_length = if !live_input || data[1] == 0 {
            0
        } else if code == 131
            && self
                .input_task
                .regions
                .iter()
                .any(|region| region.id() == data[0])
        {
            16
        } else {
            match code {
                149 => 4,
                160 => 32,
                _ => 0,
            }
        };
        if queued && (query_length != 0 || pointer_query) {
            return Err(invalid(
                "queued input query cannot retain a caller VM destination",
            ));
        }
        let mut buffers = if let Some(saved) = self.pending_owned_task_buffers(command) {
            saved
        } else {
            let first = if pointer_query {
                (data[0] != 0).then(|| vm.read(data[0], 4)).transpose()?
            } else if code == 6 && data[0] != 0 {
                Some(vm.read(data[0], 8)?)
            } else {
                None
            };
            let size = match (kind, code) {
                (Some(11), 127) => 60,
                (Some(11), 130) => 16,
                (Some(6), 90) => 24,
                (Some(6), 101) => 16,
                (Some(6), 53) => 12,
                (_, 6) => 8,
                _ => 0,
            };
            let second = if pointer_query {
                (data[1] != 0).then(|| vm.read(data[1], 4)).transpose()?
            } else if query_length != 0 {
                // Retain caller bytes if a concrete receiver declines or
                // fails before writing, including a missing history provider.
                Some(vm.read(data[1], query_length)?)
            } else if size != 0 && data[1] != 0 {
                Some(vm.read(data[1], size)?)
            } else {
                None
            };
            super::native_tasks::OwnedTaskBuffers::new(first, second)?
        };
        let mut output = 0;
        if queued {
            self.queue_task_command_buffers(command, &buffers, &mut output)?;
            return Ok(Some(NativeReturn::Value(output)));
        }
        let result = self.route_task_command_buffers(command, &mut buffers, &mut output, 4096);
        if pointer_query {
            if let Some(bytes) = buffers.data0() {
                vm.write(data[0], &bytes[..4])?;
            }
            if let Some(bytes) = buffers.data1() {
                vm.write(data[1], &bytes[..4])?;
            }
        }
        if query_length != 0 && result.is_ok() {
            if let Some(bytes) = buffers.data1() {
                vm.write(data[1], &bytes[..query_length])?;
            }
        }
        Ok(Some(if result?.is_some_and(|outcome| !outcome.done) {
            NativeReturn::Retry
        } else {
            NativeReturn::Value(output)
        }))
    }

    fn move_device_pointer_native(&mut self, arguments: &[u8]) -> Result<NativeReturn> {
        self.move_device_pointer_native_command(arguments, 138)
    }

    /// UTATEMEGURI 45b240: the real ordinary703 is (x,y,time,force),
    /// querying its input service with31 then sending128/129. These two
    /// messages correspond to the shared semantic input138/139 receivers;
    /// the force word selects whether to bypass EFFECT/auto_c.
    pub(super) fn move_selection_pointer_native703(&mut self, arguments: &[u8]) -> Result<NativeReturn> {
        if arguments.len() != 16 {
            return Err(invalid("legacy native703 requires X, Y, remaining and force DWORDs"));
        }
        let code = if super::read_u32(arguments, 12)? == 0 { 138 } else { 139 };
        self.move_device_pointer_native_command(arguments, code)
    }

    fn move_device_pointer_native_command(&mut self, arguments: &[u8], code: u32) -> Result<NativeReturn> {
        if arguments.len() < 12 || arguments.len() % 4 != 0 {
            return Err(invalid("native150 requires its three DWORD prefix"));
        }
        self.install_device_input_receiver();
        let caller = self.current_task_id;
        let identity = [arguments[..12].to_vec(), code.to_le_bytes().to_vec()].concat();
        let request = self
            .data_services
            .messages
            .pointer_natives
            .entry(caller)
            .or_insert_with(|| (identity.clone(), None));
        if request.0 != identity {
            return Err(invalid(
                "native150 continuation changed its coordinate arguments",
            ));
        }
        let mut input = request.1.unwrap_or(0);
        if request.1.is_none() {
            let root = self.route_task_command_buffers(
                cs2_runtime::TaskCommand {
                    target: 0,
                    sender: self.current_task_id as i32,
                    code: 34,
                    data: [0; 3],
                },
                &mut Default::default(),
                &mut input,
                4096,
            )?;
            if root.is_some_and(|outcome| !outcome.done) {
                return Ok(NativeReturn::Retry);
            }
            // The desktop's implicit input service also tracks the real kind11
            // object when a root implementation does not yet answer query34.
            if !self
                .scene_nodes
                .get(&input)
                .is_some_and(|node| node.kind == 11)
            {
                if let Some(id) =
                    self.input_task
                        .owner_id
                        .filter(|&id| {
                            self.data_services
                                .tasks
                                .message_task_state(id)
                                .is_some_and(|(kind, deleted, _)| kind == 11 && deleted == 0)
                        })
                        .or_else(|| {
                            self.scene_nodes.iter().find_map(|(&id, node)| {
                                (node.kind == 11
                                    && self.data_services.tasks.message_task_state(id).is_some_and(
                                        |(kind, deleted, _)| kind == 11 && deleted == 0,
                                    ))
                                .then_some(id)
                            })
                        })
                {
                    input = id;
                }
            }
            self.data_services
                .messages
                .pointer_natives
                .get_mut(&caller)
                .unwrap()
                .1 = Some(input);
        }
        let x = super::read_u32(arguments, 0)? as u16;
        let y = super::read_u32(arguments, 4)? as u16;
        let remaining = super::read_u32(arguments, 8)?;
        let result = self.route_task_command_buffers(
            cs2_runtime::TaskCommand {
                target: input as i32,
                sender: self.current_task_id as i32,
                code,
                data: [remaining, u32::from(x) | (u32::from(y) << 16), 0],
            },
            &mut Default::default(),
            &mut 0,
            4096,
        )?;
        if result.is_some_and(|outcome| !outcome.done) {
            Ok(NativeReturn::Retry)
        } else {
            self.data_services.messages.pointer_natives.remove(&caller);
            Ok(NativeReturn::Continue)
        }
    }

    pub(super) fn query_device_pointer(&self) -> Result<[i32; 2]> {
        let coordinates = self
            .data_services
            .host_input_coordinates
            .ok_or_else(|| invalid("pointer query has no host coordinate model"))?;
        let device = if self.data_services.messages.pointer_saved.is_some() {
            self.data_services.messages.device_pointer
        } else {
            // Window/Java hosts explicitly feed logical coordinates. Their
            // virtual device has the common map's chosen inverse transform.
            coordinates.to_device(self.pointer_position).0
        };
        Ok(coordinates.to_logical(device).0)
    }

    fn input_region_native(
        &mut self,
        index: u32,
        args: &[u8],
        vm: &mut Vm,
    ) -> Result<NativeReturn> {
        let &(_, words, code) = INPUT_REGION_NATIVES
            .iter()
            .find(|entry| entry.0 == index)
            .ok_or(ScriptError::UnimplementedNativeCall { index })?;
        if words != 0 && args.len() != words * 4 {
            return Err(invalid(
                "input region native requires its fixed DWORD argument count",
            ));
        }
        let args = if words == 0 { &[][..] } else { args };
        let caller = self.current_task_id;
        let mut pending = self
            .data_services
            .messages
            .input_natives
            .remove(&caller)
            .unwrap_or_else(|| InputNativeDispatch {
                index,
                arguments: args.to_vec(),
                input: None,
                buffers: None,
            });
        if pending.index != index || pending.arguments != args {
            self.data_services
                .messages
                .input_natives
                .insert(caller, pending);
            return Err(invalid(
                "input region native continuation changed its invocation",
            ));
        }
        let result = (|| {
            if pending.input.is_none() {
                let mut input = 0;
                let outcome = self.route_task_command_buffers(
                    cs2_runtime::TaskCommand {
                        target: 0,
                        sender: caller as i32,
                        code: 34,
                        data: [0; 3],
                    },
                    &mut Default::default(),
                    &mut input,
                    4096,
                )?;
                if outcome.is_some_and(|outcome| !outcome.done) {
                    return Ok(NativeReturn::Retry);
                }
                if !self
                    .data_services
                    .tasks
                    .message_task_state(input)
                    .is_some_and(|(kind, deleted, _)| kind == 11 && deleted == 0)
                {
                    input = self.input_task.owner_id.unwrap_or(input);
                }
                pending.input = Some(input);
            }
            let data = if index == 145 {
                [0, caller, 0]
            } else if index == 401 {
                [0, super::read_u32(args, 0)?, 0]
            } else if words == 0 {
                [0; 3]
            } else {
                [super::read_u32(args, 0)?, super::read_u32(args, 4)?, 0]
            };
            if pending.buffers.is_none() {
                // 147 copies only after the root query. Caller changes during
                // receiver yields cannot replace this accepted rectangle.
                let rectangle = if index == 147 && data[1] != 0 {
                    Some(vm.read(data[1], 16)?)
                } else {
                    None
                };
                pending.buffers =
                    Some(super::native_tasks::OwnedTaskBuffers::new(None, rectangle)?);
            }
            let input = pending.input.unwrap();
            let command = cs2_runtime::TaskCommand {
                target: input as i32,
                sender: caller as i32,
                code,
                data,
            };
            let mut output = 0;
            let outcome = if input == 0
                && self.input_task.owner_id.is_none()
                && self.input_task.alive
            {
                let done = if matches!(command.code, 143 | 159) {
                    self.advance_input_cursor_command(command, 4096)?
                } else if command.code == 150 {
                    self.receive_input_cursor_command(command, pending.buffers.as_mut().unwrap())?;
                    true
                } else {
                    self.receive_input_region_command(
                        command,
                        pending.buffers.as_mut().unwrap(),
                        &mut output,
                    )?;
                    true
                };
                Some(cs2_runtime::TaskCommandOutcome {
                    done,
                    propagate: false,
                })
            } else {
                self.install_device_input_receiver();
                self.route_task_command_buffers(
                    command,
                    pending.buffers.as_mut().unwrap(),
                    &mut output,
                    4096,
                )?
            };
            Ok(if outcome.is_some_and(|outcome| !outcome.done) {
                NativeReturn::Retry
            } else {
                NativeReturn::Continue
            })
        })();
        if !matches!(result, Ok(NativeReturn::Continue)) {
            self.data_services
                .messages
                .input_natives
                .insert(caller, pending);
        }
        result
    }

    /// Ordinary77 queries global number occupancy through root command33.
    /// A sentinel distinguishes an absent receiver from an actual false
    /// answer. Deleted roots preserve the wrapper's initial zero output.
    fn root_task_number_native(&mut self, args: &[u8]) -> Result<NativeReturn> {
        if args.len() < 4 {
            return Err(invalid("native77 requires one DWORD task number"));
        }
        let number = super::read_u32(args, 0)?;
        let mut output = u32::MAX;
        let outcome = self.route_task_command_buffers(
            cs2_runtime::TaskCommand {
                target: 0,
                sender: self.current_task_id as i32,
                code: 33,
                data: [number, 0, 0],
            },
            &mut Default::default(),
            &mut output,
            4096,
        )?;
        if outcome.is_some_and(|outcome| !outcome.done) {
            return Ok(NativeReturn::Retry);
        }
        if outcome.is_none() {
            return Err(invalid("native77 root receiver is unavailable"));
        }
        if self
            .data_services
            .tasks
            .message_task_state(0)
            .is_some_and(|(_, deletion, _)| deletion != 0)
        {
            return Ok(NativeReturn::Value(0));
        }
        if output == u32::MAX {
            let lookup = self
                .data_services
                .messages
                .root_task_number_lookup
                .ok_or_else(|| {
                    invalid("root command33 requires the global task-number index adapter")
                })?;
            output = u32::from(lookup(self, number)?);
        }
        if output > 1 {
            return Err(invalid(
                "root command33 returned a non-boolean index result",
            ));
        }
        Ok(NativeReturn::Value(output))
    }

    /// Ordinary740 is the full_pos-dependent lower-edge query, not pointer
    /// position or fullscreen_mw_pos. The selected software device presents
    /// the logical Canvas; outer window/Java letterboxing is a host transform.
    fn message_lower_edge_native(&mut self, args: &[u8], vm: &mut Vm) -> Result<NativeReturn> {
        if args.len() < 8 {
            return Err(invalid("native740 requires two VM output addresses"));
        }
        let x = super::read_u32(args, 0)?;
        let y = super::read_u32(args, 4)?;
        vm.read(x, 1)?;
        vm.read(y, 1)?;
        self.ensure_configuration_state()
            .map_err(|_| invalid("native740 platform configuration could not be loaded"))?;
        self.ensure_canvas_state()
            .map_err(|_| invalid("native740 drawing configuration could not be loaded"))?;
        let coordinates = self
            .data_services
            .host_input_coordinates
            .ok_or_else(|| invalid("native740 has no bound host coordinate model"))?;
        let mode = self
            .data_services
            .configuration
            .platform_value("FULL_SCREEN/full_pos")
            .ok_or_else(|| invalid("native740 full_pos configuration is unavailable"))?;
        let full_height = self.data_services.configuration.screen_dimensions()[3];
        let bottom = self
            .canvas
            .as_deref()
            .ok_or_else(|| invalid("native740 has no bound software rendering device"))?
            .height();
        let offset_y = if self.data_services.canvas_native.mode == 0
            && self.data_services.canvas_native.enabled
        {
            cs2_runtime::input_coordinates::x86_truncate(coordinates.device_offset[1]) as u32
        } else {
            0
        };
        // Do not clear Y here: unknown full_pos values only clear X, and the
        // mode2 fast path writes the raw virtual-height WORD then returns.
        vm.write(x, &0_u32.to_le_bytes())?;
        let height = match mode {
            0 => bottom.wrapping_add(offset_y),
            1 => u32::from(full_height),
            2 => {
                let virtual_height = u32::from(coordinates.logical_size[1] as u16);
                vm.write(y, &virtual_height.to_le_bytes())?;
                return Ok(NativeReturn::Continue);
            }
            _ => return Ok(NativeReturn::Continue),
        };
        vm.write(y, &height.to_le_bytes())?;
        // A shared or partially overlapping destination must observe X's
        // completed write before reading the current value for Y.
        for (axis, address) in [x, y].into_iter().enumerate() {
            let current = super::read_u32(&vm.read(address, 4)?, 0)? as i32;
            let logical = coordinates.logical_axis(axis, current) as u32;
            vm.write(address, &logical.to_le_bytes())?;
        }
        Ok(NativeReturn::Continue)
    }

    /// Root query precedes format expansion. Both wrappers share the same
    /// record parser and real kind13 receiver, including group allocation,
    /// loop ranges, decoder, channel ownership and ordinary output.
    fn legacy_sound_prepare_native(
        &mut self,
        index: u32,
        args: &[u8],
        vm: &mut Vm,
    ) -> Result<NativeReturn> {
        let &(_, words, grouped, first_command) = SOUND_PREPARE_NATIVES
            .iter()
            .find(|entry| entry.0 == index)
            .ok_or(ScriptError::UnimplementedNativeCall { index })?;
        if args.len() < words * 4 {
            return Err(invalid(
                "legacy sound preparation requires its minimum DWORD prefix",
            ));
        }
        let args = &args[..words * 4];
        let caller = self.current_task_id;
        let mut pending = self
            .data_services
            .messages
            .sound_prepare_natives
            .remove(&caller)
            .unwrap_or_else(|| SoundPrepareNativeDispatch {
                index,
                arguments: args.to_vec(),
                root_queried: false,
                sound: None,
                record: None,
            });
        if pending.index != index || pending.arguments != args {
            self.data_services
                .messages
                .sound_prepare_natives
                .insert(caller, pending);
            return Err(invalid(
                "legacy sound preparation continuation changed its invocation",
            ));
        }
        let result = (|| {
            if !pending.root_queried {
                let mut sound = u32::MAX;
                let outcome = self.route_task_command_buffers(
                    cs2_runtime::TaskCommand {
                        target: 0,
                        sender: caller as i32,
                        code: 35,
                        data: [0; 3],
                    },
                    &mut Default::default(),
                    &mut sound,
                    4096,
                )?;
                if outcome.is_some_and(|outcome| !outcome.done) {
                    return Ok(NativeReturn::Retry);
                }
                if outcome.is_none() {
                    return Err(invalid("legacy sound preparation root is unavailable"));
                }
                if self
                    .data_services
                    .tasks
                    .message_task_state(0)
                    .is_some_and(|(_, deletion, _)| deletion != 0)
                {
                    return Err(invalid(
                        "legacy sound preparation root was deleted before its service query",
                    ));
                }
                // An explicit root binding is authoritative, including
                // configurations with more than one real sound task.
                if sound != u32::MAX {
                    if !self
                        .data_services
                        .tasks
                        .message_task_state(sound)
                        .is_some_and(|(kind, _, _)| kind == 13)
                    {
                        return Err(invalid(
                            "root35 returned a task without a kind13 sound receiver",
                        ));
                    }
                    pending.sound = Some(sound);
                } else {
                    // The existing desktop root may expose its implicit
                    // singleton service instead of implementing command35.
                    pending.sound = self.default_sound_task()?;
                }
                pending.root_queried = true;
            }
            let speech = matches!(index, 1070..=1073);
            if speech && pending.record.is_none() {
                pending.record = Some(super::sound_speech_load::prepare_native_record(
                    index, args, vm,
                )?);
            }
            if pending.record.is_none() {
                let source = vm.read_string(super::read_u32(args, 8)?)?;
                let expanded = super::expand_kcs_format(vm, &source, 0)?;
                let name = self.legacy_sound_record_name(&expanded)?;
                let key = super::read_u32(args, 0)?;
                let mut record = vec![0_u8; 292];
                if grouped {
                    // 604's root output occupies this otherwise unused word.
                    // Classification at +8 stays zero, including by_key=0.
                    record[..4].copy_from_slice(&pending.sound.unwrap_or(0).to_le_bytes());
                    record[4..8].copy_from_slice(&key.to_le_bytes());
                } else {
                    record[..4].copy_from_slice(&key.to_le_bytes());
                    // 155 uses the unused +4 word as root35's output scratch.
                    record[4..8].copy_from_slice(&pending.sound.unwrap_or(0).to_le_bytes());
                }
                record[12..12 + name.len()].copy_from_slice(&name);
                record[272..276].copy_from_slice(&u32::from(key >> 16 == 0).to_le_bytes());
                pending.record = Some(record);
            }
            let selector = if speech {
                1
            } else {
                super::read_u32(args, (words - 1) * 4)?
            };
            if !matches!(selector, 1 | 2) {
                return Err(invalid(
                    "legacy sound preparation selector has no defined native result",
                ));
            }
            let command = first_command + selector - 1
                + if grouped && super::read_u32(args, 12)? == 0 {
                    if speech { 1 } else { 2 }
                } else {
                    0
                };
            let sound = pending.sound.ok_or_else(|| {
                invalid("legacy sound preparation has no real kind13 sound service")
            })?;
            let mut output = 0;
            match command {
                180 | 181 | 206..=209 => self.dispatch_sound_load_record(
                    sound,
                    command,
                    pending.record.as_deref_mut().expect("sound load record prepared"),
                ),
                225..=227 => {
                    if !self.dispatch_sound_task_command(
                        sound,
                        cs2_runtime::TaskCommand {
                            target: sound as i32,
                            sender: caller as i32,
                            code: command,
                            data: [0; 3],
                        },
                        &mut cs2_runtime::TaskCommandBuffers {
                            data0: None,
                            data1: pending.record.as_deref_mut(),
                        },
                        &mut output,
                    )? {
                        return Err(invalid(
                            "legacy sound preparation receiver did not consume its record",
                        ));
                    }
                    Ok(NativeReturn::Value(output))
                }
                _ => Err(invalid(
                    "legacy sound preparation has no recovered receiver command",
                )),
            }
        })();
        if !matches!(result, Ok(NativeReturn::Value(_))) {
            self.data_services
                .messages
                .sound_prepare_natives
                .insert(caller, pending);
        }
        result
    }

    /// Use the desktop's real old/new sound-category and INT adapters.
    /// Compatibility candidates follow configured names and are selected
    /// only when the configured resource is absent, as for ordinary935.
    fn legacy_sound_record_name(&mut self, input: &[u8]) -> Result<Vec<u8>> {
        let input = super::trim_nul(input);
        if input.len() >= 260 {
            return Err(invalid(
                "expanded legacy sound name exceeds 259 encoded bytes",
            ));
        }
        let name = super::sound_native::decode_name(self.text_encoding, input)?.replace('\\', "/");
        let mut names = if name.rfind('/').is_some_and(|offset| offset > 0) {
            vec![name.clone()]
        } else {
            let category = if self.sound_config_enabled != 0 {
                self.sound_config
                    .file_part(input)
                    .map(|part| part.archive.clone())
            } else {
                self.legacy_sound_archive_name(input)?
            };
            self.media_category_candidates(category.as_deref().unwrap_or(b"sound"), &name)?
                .unwrap_or_default()
        };
        names.extend([name.clone(), format!("system/{name}")]);
        if std::path::Path::new(&name).extension().is_none() {
            for extension in ["ogg", "wav"] {
                names.extend([
                    format!("{name}.{extension}"),
                    format!("system/{name}.{extension}"),
                    format!("sound/{name}.{extension}"),
                ]);
            }
        }
        let selected = names
            .iter()
            .find(|candidate| self.resources.find(candidate).is_some())
            .or_else(|| names.first())
            .ok_or_else(|| invalid("legacy sound category produced no resource name"))?;
        let encoded = super::sound_native::encode_name(self.text_encoding, selected)?;
        if encoded.len() >= 260 {
            return Err(invalid(
                "completed legacy sound name exceeds 259 encoded bytes",
            ));
        }
        Ok(encoded)
    }

    fn device_pointer_query_native(&mut self, args: &[u8], vm: &mut Vm) -> Result<NativeReturn> {
        if args.len() < 8 || args.len() % 4 != 0 {
            return Err(invalid("native149 requires two VM output addresses"));
        }
        let x = super::read_u32(args, 0)?;
        let y = super::read_u32(args, 4)?;
        vm.read(x, 1)?;
        vm.read(y, 1)?;
        let caller = self.current_task_id;
        let pending = self
            .data_services
            .messages
            .pointer_queries
            .entry(caller)
            .or_insert_with(|| (args[..8].to_vec(), None));
        if pending.0 != args[..8] {
            return Err(invalid(
                "native149 continuation changed its output addresses",
            ));
        }
        let mut input = pending.1.unwrap_or(0);
        if pending.1.is_none() {
            let result = self.route_task_command_buffers(
                cs2_runtime::TaskCommand {
                    target: 0,
                    sender: caller as i32,
                    code: 34,
                    data: [0; 3],
                },
                &mut Default::default(),
                &mut input,
                4096,
            )?;
            if result.is_some_and(|outcome| !outcome.done) {
                return Ok(NativeReturn::Retry);
            }
            if !self
                .data_services
                .tasks
                .message_task_state(input)
                .is_some_and(|(kind, deleted, _)| kind == 11 && deleted == 0)
            {
                input = self.input_task.owner_id.unwrap_or(input);
            }
            self.data_services
                .messages
                .pointer_queries
                .get_mut(&caller)
                .unwrap()
                .1 = Some(input);
        }
        if input == 0 && self.input_task.owner_id.is_none() && self.input_task.alive {
            // Existing desktop input has an explicit implicit-root identity
            // until a kind-eleven task is registered. Read its real sample.
            let point = self.query_device_pointer()?;
            vm.write(x, &point[0].to_le_bytes())?;
            vm.write(y, &point[1].to_le_bytes())?;
        } else {
            self.install_device_input_receiver();
            let command = cs2_runtime::TaskCommand {
                target: input as i32,
                sender: caller as i32,
                code: 126,
                data: [1, 1, 0],
            };
            let mut buffers = if let Some(buffers) = self.pending_owned_task_buffers(command) {
                buffers
            } else {
                super::native_tasks::OwnedTaskBuffers::new(
                    Some(vm.read(x, 4)?),
                    Some(vm.read(y, 4)?),
                )?
            };
            let mut output = 0;
            let result =
                self.route_task_command_buffers(command, &mut buffers, &mut output, 4096)?;
            if result.is_some_and(|outcome| !outcome.done) {
                return Ok(NativeReturn::Retry);
            }
            if result.is_some() {
                if let Some(bytes) = buffers.data0() {
                    vm.write(x, &bytes[..4])?;
                }
                if let Some(bytes) = buffers.data1() {
                    vm.write(y, &bytes[..4])?;
                }
            }
        }
        self.data_services.messages.pointer_queries.remove(&caller);
        Ok(NativeReturn::Continue)
    }

    pub(super) fn move_fes_pointer(&mut self, owner: u32, point: [i32; 2], frames: u32) -> Result<()> {
        let packed = u32::from(point[0] as u16) | (u32::from(point[1] as u16) << 16);
        let data = [frames, packed, 0];
        if let Some(input) = self.input_task.owner_id {
            let command = cs2_runtime::TaskCommand {
                target: input as i32,
                sender: owner as i32,
                code: 139,
                data,
            };
            self.receive_device_input_buffers(command, &mut Default::default(), &mut 0)?
                .ok_or_else(|| invalid("navigation input receiver is absent"))?;
        } else if self.input_task.alive {
            // Older roots provide the input service without a kind-11 task.
            self.request_device_pointer(None, 139, data)?;
        } else {
            return Err(invalid("navigation input service is unavailable"));
        }
        Ok(())
    }

    pub(super) fn fes_auto_pointer_enabled(&mut self) -> Result<bool> {
        self.ensure_fes_settings().map_err(|_| {
            invalid("pointer movement cannot read the real EFFECT/auto_c setting")
        })?;
        self.system_config_blocks[2].get(12..16)
            .map(|bytes| u32::from_le_bytes(bytes.try_into().unwrap()) != 0)
            .ok_or_else(|| invalid("pointer control option block is truncated"))
    }

    fn request_device_pointer(&mut self, input: Option<u32>, code: u32, data: [u32; 3]) -> Result<()> {
        if code == 138 && !self.fes_auto_pointer_enabled()? {
            return Ok(());
        }
        // UTATE43ab1f/43ab32 go directly to coordinate conversion and the
        // saved smooth-move fields; this version has no foreground gate.
        let legacy_pointer = self.data_services.css_task_pe_sha256 ==
            Some("a9f8b08713aac278671853b2293a13b2623a122ede9341cb921901b09601cb52");
        if !legacy_pointer && !self.data_services.messages.device_foreground {
            return Ok(());
        }
        let logical = [(data[1] as u16) as i32, ((data[1] >> 16) as u16) as i32];
        let coordinates = self
            .data_services
            .host_input_coordinates
            .ok_or_else(|| invalid("pointer movement has no host coordinate model"))?;
        let (target, inside) = coordinates.to_device(logical);
        if !inside {
            return Ok(());
        }
        if self.data_services.messages.input_suppressed != 0 {
            self.data_services.messages.key_scan.clear_active_enabled();
            self.data_services.messages.device_keys.clear_pressed();
            self.data_services.messages.key_sample_physical.clear();
            self.data_services.messages.input_suppressed = 0;
        }
        let elapsed = input
            .and_then(|id| self.data_services.tasks.message_task_state(id))
            .map_or(self.task_time, |(_, _, elapsed)| elapsed);
        self.data_services
            .messages
            .pointer_move
            .request(target, data[0], elapsed);
        Ok(())
    }

    /// Live device query shared with the input scan receiver. Extra queries
    /// consume force counts, even when repeat metadata is not requested.
    pub(super) fn sample_device_key(
        &mut self,
        key: u32,
        group: u32,
        repeat_requested: bool,
    ) -> Result<DeviceKeySample> {
        if !self.data_services.messages.device_keys_loaded {
            self.data_services.message_device.load_fonts(self.resources);
            initialize_settings(self.data_services, self.resources, Some(self.save_root))?;
        }
        let sample =
            self.data_services
                .messages
                .device_keys
                .sample(key, group, repeat_requested)?;
        self.input_task.key_masks[key as usize][group as usize] = sample.mask;
        Ok(sample)
    }

    /// Resume the same scan after a task callback yields. Samples, count
    /// changes, candidate traversal and output values remain task-owned.
    pub(super) fn scan_device_keys(&mut self, budget: usize) -> Result<bool> {
        self.install_device_input_receiver();
        self.install_kx_receiver();
        if !self.input_task.alive {
            return Ok(true);
        }
        if self.data_services.messages.input_frame_complete {
            return Ok(true);
        }
        if self.input_task.owner_id.is_none() {
            self.input_task.owner_id = self.scene_nodes.iter().find_map(|(&id, node)| {
                (node.kind == 11
                    && self
                        .data_services
                        .tasks
                        .message_task_state(id)
                        .is_some_and(|(kind, deleted, _)| kind == 11 && deleted == 0))
                .then_some(id)
            });
        }
        if !self.data_services.messages.cursor_frame_done
            && self.data_services.messages.cursor_frame.is_none()
        {
            let messages = &self.data_services.messages;
            let forced = messages.input_selection_forced;
            let position_changed = messages.input_selection_previous_device.unwrap_or([0; 2])
                != messages.device_pointer_observed;
            self.data_services.messages.cursor_frame = Some(InputCursorFrame {
                changed: forced || messages.input_geometry.is_dirty() || position_changed,
                position_changed,
                moving: messages.device_pointer_moving,
                original_nonzero: messages.device_pointer_observed != [0; 2],
                active: messages.device_sample_active,
                foreground: messages.device_foreground,
                initialized: false,
                broadcast: None,
            });
        }
        if !self.advance_device_input_selection(budget)? {
            return Ok(false);
        }
        if !self.advance_device_cursor_frame(budget)? {
            return Ok(false);
        }
        for _ in 0..budget {
            // Existing commands may alter the switches from inside a key
            // handler. Their bool view must not normalize raw nonzero values.
            for key in 0..80 {
                let current = self.data_services.messages.key_scan.enabled()[key];
                let enabled = self.input_task.key_enabled[key];
                if (current != 0) != enabled {
                    self.data_services
                        .messages
                        .key_scan
                        .set_enabled(key as u32, enabled as u32)?;
                }
            }
            match self.data_services.messages.key_scan.step() {
                KeyScanStep::Sample { key, group } => {
                    let forced = self
                        .data_services
                        .messages
                        .device_keys
                        .forced_queries(key)?
                        != 0;
                    let sample = self.sample_device_key(key, group, true)?;
                    if sample.mask != 0 {
                        if forced {
                            self.data_services
                                .messages
                                .key_sample_physical
                                .remove(&(key, group));
                        } else {
                            self.data_services
                                .messages
                                .key_sample_physical
                                .insert((key, group));
                        }
                    }
                    self.data_services.messages.key_scan.sampled(sample)?;
                }
                KeyScanStep::Dispatch(transition) => {
                    let Some(consumed) = self.dispatch_device_key_transition(transition, budget)?
                    else {
                        return Ok(false);
                    };
                    self.data_services.messages.key_scan.dispatched(consumed)?;
                    if transition.pressed == 0 {
                        self.data_services
                            .messages
                            .key_sample_physical
                            .remove(&(transition.key, transition.group));
                    }
                }
                KeyScanStep::Complete { .. } => {
                    let complete = self.advance_device_pointer_events(budget)?;
                    if complete {
                        self.data_services.messages.input_geometry.finish_update();
                        self.data_services.messages.input_geometry.commit();
                        // Input160 is readable by scripts later in this same
                        // logical update. Render completion must not delay it.
                        let history = self.data_services.messages.frame_device_history.take();
                        self.data_services.messages.device_history.complete_frame(history);
                        self.data_services.messages.input_frame_complete = true;
                    }
                    return Ok(complete);
                }
            }
        }
        Ok(false)
    }

    fn advance_device_pointer_events(&mut self, budget: usize) -> Result<bool> {
        if self.data_services.messages.pointer_tail_done {
            return Ok(true);
        }
        let mut pending = self
            .data_services
            .messages
            .pointer_dispatch
            .take()
            .unwrap_or_else(|| PointerInputDispatch {
                code: 5,
                point: self.data_services.messages.device_logical_pointer,
                next: 0,
                current: None,
                buffers: Default::default(),
                output: 0,
                stage: 0,
                sampled_mask: 0,
                visited: 0,
                motion_routed: true,
                motion_started: false,
                motion_delivered: false,
            });
        let result = (|| {
            for _ in 0..budget {
                match pending.stage {
                    0 => {
                        if self.data_services.messages.wheel_observed != 0 {
                            self.data_services.messages.pointer_origin =
                                Some(self.data_services.messages.device_pointer);
                            pending.stage = 1;
                        } else {
                            pending.stage = 2;
                        }
                    }
                    1 => {
                        if !self.advance_pointer_region_dispatch(&mut pending, budget)? {
                            return Ok(false);
                        }
                        pending.stage = 2;
                    }
                    2 => {
                        pending.sampled_mask = self.sample_device_key(0, 0, false)?.mask;
                        pending.stage = 3;
                    }
                    3 => {
                        pending.sampled_mask |= self.sample_device_key(0, 1, false)?.mask;
                        pending.stage = 4;
                    }
                    4 => {
                        let device = self.data_services.messages.device_pointer;
                        let origin = self.data_services.messages.pointer_origin.unwrap_or(device);
                        let delta = std::array::from_fn::<_, 2, _>(|axis| {
                            device[axis].wrapping_sub(origin[axis]).unsigned_abs()
                        });
                        let threshold = if self.data_services.messages.input_suppressed == 0 {
                            self.data_services.messages.device_cursor.drag_threshold
                        } else {
                            1
                        };
                        if pending.sampled_mask & 2 == 0
                            || !delta.into_iter().any(|n| n >= threshold)
                        {
                            pending.stage = 6;
                            continue;
                        }
                        self.data_services.messages.input_suppressed = 1;
                        let coordinates =
                            self.data_services.host_input_coordinates.ok_or_else(|| {
                                invalid("pointer motion has no host coordinate model")
                            })?;
                        let previous = coordinates.to_logical(origin).0;
                        let current = coordinates.to_logical(device).0;
                        let bytes = |point: [i32; 2]| {
                            point.into_iter().flat_map(i32::to_le_bytes).collect()
                        };
                        pending.code = 6;
                        pending.buffers = super::native_tasks::OwnedTaskBuffers::new(
                            Some(bytes(previous)),
                            Some(bytes(current)),
                        )?;
                        pending.next = 0;
                        pending.current = None;
                        pending.visited = 0;
                        pending.motion_started = true;
                        pending.stage = 5;
                    }
                    5 => {
                        if !self.advance_pointer_region_dispatch(&mut pending, budget)? {
                            return Ok(false);
                        }
                        self.data_services.messages.pointer_origin =
                            Some(self.data_services.messages.device_pointer);
                        pending.stage = 6;
                    }
                    _ => return Ok(true),
                }
            }
            Ok(pending.stage == 6)
        })();
        if matches!(result, Ok(true)) {
            if pending.motion_started && pending.motion_routed && pending.motion_delivered {
                self.data_services.messages.pointer_motion_frame = Some(self.frame);
            }
            self.data_services.messages.pointer_tail_done = true;
        } else {
            self.data_services.messages.pointer_dispatch = Some(pending);
        }
        result
    }

    fn advance_pointer_region_dispatch(
        &mut self,
        pending: &mut PointerInputDispatch,
        budget: usize,
    ) -> Result<bool> {
        for _ in 0..budget {
            if pending.current.is_none() {
                let Some(region) = self.input_task.regions.get(pending.next).copied() else {
                    return Ok(true);
                };
                pending.visited += 1;
                if pending.visited > 1_000_000 {
                    return Err(invalid(
                        "pointer region traversal exceeds one million records",
                    ));
                }
                let point = if pending.code == 6 {
                    let bytes = pending
                        .buffers
                        .data0()
                        .and_then(|bytes| bytes.get(..8))
                        .ok_or_else(|| {
                            invalid("pointer motion's shared previous buffer is absent")
                        })?;
                    [
                        i32::from_le_bytes(bytes[..4].try_into().unwrap()),
                        i32::from_le_bytes(bytes[4..8].try_into().unwrap()),
                    ]
                } else {
                    pending.point
                };
                let [left, top, right, bottom] = region.bounds();
                let mut eligible = region.words[8] != 0 && self.host_input_region_ready(region.id())
                    && (left >= right || top >= bottom || region.contains(point));
                if pending.code == 6 {
                    eligible &= region.words[14] != 0 && region.words[2] & 2 != 0;
                } else if eligible && region.words[11] != 0 {
                    eligible = self
                        .data_services
                        .messages
                        .input_masks
                        .get(&region.id())
                        .ok_or_else(|| invalid("wheel hit map is registered but absent"))?
                        .contains([left, top], point);
                }
                if !eligible {
                    pending.next += 1;
                    continue;
                }
                pending.current = Some((region.words, pending.next));
                pending.output = 0;
            }
            let (words, previous) = pending.current.unwrap();
            let command = cs2_runtime::TaskCommand {
                target: words[0] as i32,
                sender: self.input_task.owner_id.unwrap_or(0) as i32,
                code: pending.code,
                data: [0; 3],
            };
            let saved = self.input_task.pointer_position;
            self.input_task.pointer_position = pending.point;
            let result = self.dispatch_input_region_buffers(
                command,
                &mut pending.buffers,
                &mut pending.output,
                budget,
            );
            self.input_task.pointer_position = saved;
            if result?.is_some_and(|outcome| !outcome.done) {
                return Ok(false);
            }
            let routed = self
                .data_services
                .messages
                .button_event_completions
                .remove(&(words[0], pending.code));
            if pending.code == 6 {
                pending.motion_routed &= routed;
                pending.motion_delivered |= routed;
            }
            pending.next = live_region_index(&self.input_task.regions, words, previous)? + 1;
            pending.current = None;
        }
        Ok(false)
    }

    fn advance_device_input_selection(&mut self, budget: usize) -> Result<bool> {
        if self.data_services.messages.key_selection_done {
            return Ok(true);
        }
        let retained_selection = self.data_services.messages.input_selection_frame
            .filter(|previous| previous.checked_add(1) == Some(self.frame))
            .and(self.input_task.selected_region);
        let mut dispatch = if let Some(pending) = self.data_services.messages.key_selection.take() {
            pending
        } else {
            let messages = &self.data_services.messages;
            if (messages.device_pointer_moving && messages.device_pointer_observed == [0; 2])
                || (messages.input_geometry_tracking_complete
                    && !messages.input_geometry.is_dirty()
                    && !messages.input_selection_forced
                    && !messages.device_pointer_moving
                    && retained_selection.is_none()
                    && messages.input_selection_previous_device.unwrap_or([0; 2])
                        == messages.device_pointer)
            {
                self.data_services.messages.key_selection_done = true;
                return Ok(true);
            }
            let active = self.data_services.messages.device_sample_active != 0;
            let point = if active {
                self.data_services.messages.device_logical_pointer
            } else {
                [i32::MIN; 2]
            };
            // Inactive means no candidate, not an extremely negative point.
            // A legitimate rectangle may include MIN, and its Alpha source
            // must not be queried at all for an inactive device.
            let selected = if active {
                masked_region_at(
                    &self.input_task.regions,
                    &self.data_services.messages.input_masks,
                    point,
                    |id| self.host_input_region_ready(id),
                )?
                .map(|words| words[0])
                .filter(|&id| id != u32::MAX)
            } else {
                None
            };
            InputSelectionDispatch {
                point,
                candidate: selected,
                selected,
                next: 0,
                current: None,
                buffers: Default::default(),
                output: 0,
                stage: 0,
                visited: 0,
                all_routed: true,
                routed_any: false,
            }
        };
        let result = (|| {
            for _ in 0..budget {
                if dispatch.current.is_none() {
                    if dispatch.stage == 0 {
                        let Some(region) = self.input_task.regions.get(dispatch.next).copied()
                        else {
                            dispatch.stage = 1;
                            continue;
                        };
                        dispatch.visited += 1;
                        if dispatch.visited > 1_000_000 {
                            return Err(invalid("input selection exceeds one million records"));
                        }
                        if region.words[8] == 0 || region.words[9] == 0 || region.words[7] == 0
                            || !self.host_input_region_ready(region.id())
                        {
                            dispatch.next += 1;
                            continue;
                        }
                        if region.id() == dispatch.candidate.unwrap_or(u32::MAX) {
                            dispatch.candidate = None;
                            dispatch.next += 1;
                            continue;
                        }
                        dispatch.current = Some((region.words, dispatch.next, 8));
                    } else if dispatch.stage == 1 {
                        if dispatch.candidate.is_some_and(|id| !self.host_input_region_ready(id)) {
                            dispatch.candidate = None;
                            dispatch.selected = None;
                        }
                        let enter = dispatch.candidate.and_then(|id| {
                            self.input_task
                                .regions
                                .iter()
                                .position(|region| region.id() == id && region.words[7] == 0)
                        });
                        if let Some(at) = enter {
                            self.input_task.regions[at].words[7] = 1;
                            dispatch.current = Some((self.input_task.regions[at].words, at, 7));
                        } else {
                            if dispatch.candidate.is_some() {
                                dispatch.selected = dispatch.selected.filter(|id| {
                                    self.input_task
                                        .regions
                                        .iter()
                                        .any(|region| region.id() == *id && region.words[7] != 0)
                                });
                            }
                            dispatch.stage = 2;
                            continue;
                        }
                    } else {
                        if dispatch.all_routed
                            && (dispatch.routed_any
                                || self.input_task.selected_region == dispatch.selected)
                        {
                            self.input_task.selected_region = dispatch.selected;
                            if dispatch.routed_any || retained_selection.is_some_and(|region| {
                                dispatch.selected == Some(region)
                            }) {
                                self.data_services.messages.input_selection_frame =
                                    Some(self.frame);
                            }
                        }
                        return Ok(true);
                    }
                    dispatch.buffers = Default::default();
                    dispatch.output = 0;
                }
                let (words, previous, code) = dispatch.current.unwrap();
                let command = cs2_runtime::TaskCommand {
                    target: words[0] as i32,
                    sender: self.input_task.owner_id.unwrap_or(0) as i32,
                    code,
                    data: [dispatch.point[0] as u32, dispatch.point[1] as u32, 0],
                };
                let saved = self.input_task.pointer_position;
                self.input_task.pointer_position = dispatch.point;
                let result = self.dispatch_input_region_buffers(
                    command,
                    &mut dispatch.buffers,
                    &mut dispatch.output,
                    budget,
                );
                self.input_task.pointer_position = saved;
                if result?.is_some_and(|outcome| !outcome.done) {
                    return Ok(false);
                }
                let routed = self
                    .data_services
                    .messages
                    .button_event_completions
                    .remove(&(words[0], code));
                dispatch.all_routed &= routed;
                dispatch.routed_any |= routed;
                if code == 8 {
                    let at = live_region_index(&self.input_task.regions, words, previous)?;
                    self.input_task.regions[at].words[7] = 0;
                    dispatch.next = at + 1;
                } else {
                    dispatch.stage = 2;
                }
                dispatch.current = None;
            }
            Ok(false)
        })();
        if matches!(result, Ok(true)) {
            self.data_services.messages.key_selection_done = true;
        } else {
            self.data_services.messages.key_selection = Some(dispatch);
        }
        result
    }

    fn dispatch_input_region_buffers(
        &mut self,
        command: cs2_runtime::TaskCommand,
        buffers: &mut super::native_tasks::OwnedTaskBuffers,
        output: &mut u32,
        budget: usize,
    ) -> Result<Option<cs2_runtime::TaskCommandOutcome>> {
        let kind = self
            .data_services
            .tasks
            .message_task_state(command.target as u32)
            .filter(|(_, deleted, _)| *deleted == 0)
            .map(|(kind, _, _)| kind);
        if matches!(kind, Some(2 | 6)) {
            self.dispatch_registered_task_buffers(command, buffers, output, budget)
                .map(Some)
        } else {
            self.route_task_command_buffers(command, buffers, output, budget)
        }
    }

    fn dispatch_device_key_transition(
        &mut self,
        transition: KeyTransition,
        budget: usize,
    ) -> Result<Option<bool>> {
        let mut dispatch = if let Some(pending) = self.data_services.messages.key_dispatch.take() {
            if pending.transition != transition {
                self.data_services.messages.key_dispatch = Some(pending);
                return Err(invalid(
                    "device key callback continuation changed its transition",
                ));
            }
            pending
        } else {
            if transition.kind == KeyTransitionKind::Change {
                self.data_services.messages.input_suppressed = 0;
            }
            if self.data_services.messages.input_suppressed != 0 {
                return Ok(Some(false));
            }
            self.data_services.messages.input_event_mask = transition.mask;
            if transition.kind == KeyTransitionKind::Change && transition.pressed != 0 {
                self.data_services.messages.pointer_origin =
                    Some(self.data_services.messages.device_pointer);
                self.data_services
                    .messages
                    .key_capture
                    .remove(&transition.key);
            }
            let next = if transition.kind == KeyTransitionKind::Repeat {
                let capture = self
                    .data_services
                    .messages
                    .key_capture
                    .get(&transition.key)
                    .copied()
                    .unwrap_or(0);
                if capture == 0 {
                    0
                } else {
                    self.input_task
                        .regions
                        .iter()
                        .position(|region| region.id() == capture)
                        .unwrap_or(self.input_task.regions.len())
                }
            } else {
                0
            };
            DeviceKeyDispatch {
                transition,
                // The implicit desktop input service occupies root identity
                // zero until the real kind11 input task is registered.
                sender: self.input_task.owner_id.unwrap_or(0),
                point: if self.data_services.messages.pointer_saved.is_some() {
                    self.data_services.messages.device_task_pointer
                } else {
                    self.pointer_position
                },
                next,
                current: None,
                output: 1,
                buffers: Default::default(),
                visited: 0,
            }
        };
        let result = self.advance_device_key_dispatch(&mut dispatch, budget);
        if !matches!(result, Ok(Some(_))) {
            self.data_services.messages.key_dispatch = Some(dispatch);
        }
        result
    }

    pub(super) fn device_press_region_eligible(
        &self,
        region: super::InputRegionState,
        event_mask: u32,
        point: [i32; 2],
    ) -> Result<bool> {
        if region.words[8] == 0 || !self.host_input_region_ready(region.id())
            || region.words[2] & event_mask == 0
        {
            return Ok(false);
        }
        if event_mask & ButtonSource::Mouse.mask() == 0 {
            return Ok(true);
        }
        let [left, top, right, bottom] = region.bounds();
        let has_bounds = left < right && top < bottom;
        // FES object buttons can be registered before their image supplies
        // dimensions. An empty object has no mouse hit area, even without a mask.
        if !has_bounds && self.data_services.messages.fes_button_lookup
            .and_then(|lookup| lookup(self, region.id()))
            .is_some_and(|(_, target)| target.is_some())
        {
            return Ok(false);
        }
        if has_bounds && !region.contains(point) {
            return Ok(false);
        }
        if region.words[11] == 0 {
            return Ok(true);
        }
        let mask = self.data_services.messages.input_masks.get(&region.id())
            .ok_or_else(|| invalid("input region's registered hit map is absent"))?;
        Ok(mask.contains([left, top], point))
    }

    fn advance_device_key_dispatch(
        &mut self,
        dispatch: &mut DeviceKeyDispatch,
        budget: usize,
    ) -> Result<Option<bool>> {
        let event = dispatch.transition;
        for _ in 0..budget {
            if dispatch.current.is_none() {
                let Some(region) = self.input_task.regions.get(dispatch.next).copied() else {
                    if event.kind == KeyTransitionKind::Change && event.pressed == 0 {
                        self.data_services.messages.key_capture.remove(&event.key);
                    }
                    return Ok(Some(false));
                };
                dispatch.visited += 1;
                if dispatch.visited > 1_000_000 {
                    return Err(invalid(
                        "device key region traversal exceeds one million records",
                    ));
                }
                let capture = self
                    .data_services
                    .messages
                    .key_capture
                    .get(&event.key)
                    .copied()
                    .unwrap_or(0);
                let eligible = if event.pressed == 0 {
                    region.words[8] != 0 && self.host_input_region_ready(region.id())
                        && (capture == 0 || capture == region.id())
                } else {
                    self.device_press_region_eligible(region, event.mask, dispatch.point)?
                };
                if !eligible {
                    if event.kind == KeyTransitionKind::Repeat {
                        return Ok(Some(false));
                    }
                    dispatch.next += 1;
                    continue;
                }
                dispatch.current = Some((region.words, dispatch.next));
                dispatch.output = 1;
                dispatch.buffers = Default::default();
            }
            let (words, previous_index) = dispatch.current.unwrap();
            let consumed = {
                let command = cs2_runtime::TaskCommand {
                    target: words[0] as i32,
                    sender: dispatch.sender as i32,
                    code: 4,
                    data: [event.key, event.pressed, 0],
                };
                let saved_point = self.input_task.pointer_position;
                let saved_key = self.data_services.messages.input_key_context;
                self.input_task.pointer_position = dispatch.point;
                self.data_services.messages.input_key_context =
                    Some((event.key, event.group, words[0]));
                // The BUTTON and its owner keep their own continuations.
                // An outer route at root zero would block a root callback's
                // nested 95/96 queries with a different pending packet.
                let result = self.dispatch_input_region_buffers(
                    command,
                    &mut dispatch.buffers,
                    &mut dispatch.output,
                    budget,
                );
                self.input_task.pointer_position = saved_point;
                self.data_services.messages.input_key_context = saved_key;
                let result = result?;
                if result.is_some_and(|outcome| !outcome.done) {
                    return Ok(None);
                }
                // Only an actual reached callback's zero output consumes.
                result.is_some() && dispatch.output == 0
            };
            if consumed {
                if event.kind == KeyTransitionKind::Change {
                    if event.pressed != 0 {
                        self.data_services
                            .messages
                            .key_capture
                            .insert(event.key, words[0]);
                    } else {
                        self.data_services.messages.key_capture.remove(&event.key);
                    }
                }
                return Ok(Some(true));
            }
            if event.kind == KeyTransitionKind::Repeat {
                return Ok(Some(false));
            }
            // Read the live successor after callbacks may reorder the table.
            // Unique ids remain trackable when priority changes. Duplicate
            // ids require the saved current record or its original position.
            let current = live_region_index(&self.input_task.regions, words, previous_index)?;
            dispatch.next = current + 1;
            dispatch.current = None;
        }
        Ok(None)
    }

    /// Ordinary92 remains the shared plane47 wrapper. Existing image/movie
    /// receivers retain their own implementation; text descendants receive
    /// the corresponding plane48 event even when display did not change.
    fn message_visibility_native(&mut self, arguments: &[u8], vm: &mut Vm) -> Result<NativeReturn> {
        let target = super::read_u32(arguments, 0)?;
        if target == 0 {
            return Ok(NativeReturn::Continue);
        }
        let value = super::read_u32(arguments, 4)?;
        if self
            .data_services
            .tasks
            .message_task_state(target)
            .is_some_and(|(_, deletion, _)| deletion != 0)
        {
            return Ok(NativeReturn::Continue);
        }
        let text = self.scene_nodes.get(&target).is_some_and(|node| {
            super::message_native::MessageNativeServices::handles_task_kind(node.kind)
        });
        if text {
            self.dispatch_message_plane_display(target, 47, value)?;
        } else {
            let handled = if self.movie_plane_native(92, arguments, vm)?.is_some() {
                true
            } else {
                self.plane_native(92, arguments, vm)?.is_some()
            };
            if !handled {
                if let Some(node) = self.scene_nodes.get_mut(&target) {
                    node.visible = value != 0;
                    self.data_services
                        .planes
                        .observe_existing(92, arguments, self.scene_nodes)?;
                }
            }
            if let Some(kind) = self.data_services.messages.legacy_string_kind(target)
                && let Some((live_kind, 0, elapsed)) = self.data_services.tasks.message_task_state(target)
                && live_kind == kind
            {
                let display = if self.canvas.is_none()
                    || !self.data_services.message_device.drawing_enabled
                {
                    MessagePlaneDisplay::Unreachable
                } else if self
                    .scene_nodes
                    .get(&target)
                    .is_some_and(|node| node.visible)
                {
                    MessagePlaneDisplay::Visible
                } else {
                    MessagePlaneDisplay::Hidden
                };
                let device = &mut self.data_services.message_device;
                self.data_services.messages.update_utate_display(
                    target,
                    display,
                    elapsed,
                    device.time_scale,
                    device,
                )?;
            }
            // Existing plane recursion accepts kind2; forwarding into text is
            // consumed here after those receivers finish their own effects.
            self.forward_message_plane_display(target, value, 0, false)?;
        }
        Ok(NativeReturn::Continue)
    }

    fn message_raw_display_native(
        &mut self,
        arguments: &[u8],
        vm: &mut Vm,
    ) -> Result<NativeReturn> {
        if arguments.len() != 16 {
            return Err(invalid("native69 requires four DWORDs"));
        }
        if let Some(result) = self.kx_raw_native(arguments, vm, false)? {
            return Ok(result);
        }
        if let Some(result) = self.input_graphic_raw_native(arguments, vm, false)? {
            return Ok(result);
        }
        let target = super::read_u32(arguments, 0)?;
        let command = super::read_u32(arguments, 4)?;
        let value = super::read_u32(arguments, 12)?;
        let mut output = 0;
        if matches!(command, 137..=139 | 145 | 151 | 152 | 158)
            && self
                .scene_nodes
                .get(&target)
                .is_some_and(|node| node.kind == 11)
        {
            self.install_device_input_receiver();
            let result = self.route_task_command_buffers(
                cs2_runtime::TaskCommand {
                    target: target as i32,
                    sender: self.current_task_id as i32,
                    code: command,
                    data: [super::read_u32(arguments, 8)?, value, 0],
                },
                &mut Default::default(),
                &mut output,
                4096,
            )?;
            return Ok(if result.is_some_and(|outcome| !outcome.done) {
                NativeReturn::Retry
            } else {
                NativeReturn::Value(output)
            });
        }
        let packet = [
            self.current_task_id,
            command,
            super::read_u32(arguments, 8)?,
            value,
        ];
        if self
            .dispatch_buffered_text_message(target, packet, false, &mut output, Some(&mut *vm))?
            .is_some()
        {
            return Ok(NativeReturn::Value(output));
        }
        let result = if let Some(result) = self.movie_plane_native(69, arguments, vm)? {
            result
        } else if let Some(result) = self.plane_native(69, arguments, vm)? {
            result
        } else {
            return self.task_message_native(69, arguments, vm);
        };
        if matches!(command, 47 | 48)
            && !self
                .data_services
                .tasks
                .message_task_state(target)
                .is_some_and(|(_, deletion, _)| deletion != 0)
        {
            self.forward_message_plane_display(target, value, 0, false)?;
        }
        Ok(result)
    }

    /// Concrete text reception for the shared native69/70 traversal. A queued
    /// visibility packet owns its four DWORDs and is consumed on the CPU frame,
    /// including repeated show events. None leaves other protocols to routing.
    pub(super) fn dispatch_buffered_text_message(
        &mut self,
        target: u32,
        packet: [u32; 4],
        queued: bool,
        output: &mut u32,
        vm: Option<&mut Vm>,
    ) -> Result<Option<bool>> {
        let Some(node) = self.scene_nodes.get(&target).filter(|node| {
            super::message_native::MessageNativeServices::handles_task_kind(node.kind)
        }) else {
            return Ok(None);
        };
        if self
            .data_services
            .tasks
            .message_task_state(target)
            .is_none_or(|(kind, deletion, _)| kind != node.kind || deletion != 0)
        {
            return Ok(Some(false));
        }
        // Raw zero participates in the same ancestor/subtree search as other
        // task ids; only native92 has the documented early-zero wrapper.
        let mut ancestor = Some(packet[0]);
        let mut reachable = false;
        for _ in 0..=256 {
            let Some(id) = ancestor else { break };
            if in_subtree(self.task_parents, id, target) {
                reachable = true;
                break;
            }
            ancestor = self
                .task_parents
                .get(&id)
                .copied()
                .filter(|parent| *parent != id);
        }
        if !reachable {
            return Ok(None);
        }
        if matches!(packet[1], 47 | 48) {
            if queued {
                self.data_services.messages.queue_display(target, packet)?;
                *output = 1;
            } else {
                self.dispatch_message_plane_display(target, packet[1], packet[3])?;
            }
            return Ok(Some(false));
        }
        if !queued && let Some(vm) = vm {
            if self.dispatch_message_text_command(
                target,
                packet[1],
                [packet[2], packet[3], 0],
                output,
                vm,
            )? {
                return Ok(Some(false));
            }
        }
        Ok(None)
    }

    /// Raw CP932/Unicode task command adapter; no invented ordinary wrapper
    /// numbers. Pointer fields are translated through the caller VM here.
    fn dispatch_message_text_command(
        &mut self,
        target: u32,
        command: u32,
        data_words: [u32; 3],
        output: &mut u32,
        vm: &mut Vm,
    ) -> Result<bool> {
        let encoding = match self.scene_nodes.get(&target).map(|node| node.kind) {
            Some(48) => MessageEncoding::Cp932,
            Some(63) => MessageEncoding::Unicode,
            _ => return Ok(false),
        };
        let base = if encoding == MessageEncoding::Unicode {
            476
        } else {
            455
        };
        let Some(operation) = command.checked_sub(base) else {
            return Ok(false);
        };
        if !matches!(operation, 0..=17 | 19 | 20)
            && !(operation == 21 && encoding == MessageEncoding::Unicode)
        {
            return Ok(false);
        }
        self.data_services.message_device.load_fonts(self.resources);
        if !self.data_services.message_device.settings_loaded {
            initialize_settings(self.data_services, self.resources, Some(self.save_root))?;
        }
        let configured = self
            .data_services
            .messages
            .object(target)
            .is_some_and(|object| object.configured());
        if operation == 11 {
            // Command466/487 reads the cached cursor without preparing
            // pending appends. An absent display object leaves outputs alone.
            if configured {
                let position = self
                    .data_services
                    .messages
                    .object(target)
                    .unwrap()
                    .cached_cursor()
                    .unwrap_or([0; 2]);
                vm.write(data_words[0], &(position[0] as u32).to_le_bytes())?;
                vm.write(data_words[1], &(position[1] as u32).to_le_bytes())?;
            }
            return Ok(true);
        }
        let expected = if encoding == MessageEncoding::Unicode {
            0x12c
        } else {
            0xe8
        };
        let buffer = if operation <= 2 {
            if data_words[1] == 0 {
                return Ok(true);
            }
            let tag = vm.read(data_words[1], 4)?;
            if tag == (expected as u32).to_le_bytes() {
                Some(vm.read(data_words[1], expected)?)
            } else {
                Some(tag)
            }
        } else if matches!(operation, 3 | 13 | 16) && configured {
            Some(vm.read_string(data_words[1])?)
        } else {
            None
        };
        let source = if operation == 9 && configured && data_words[1] != 0 {
            Some(self.data_services.ordinary.raw_image(data_words[1])?)
        } else {
            None
        };
        let inline_source = if operation == 21 && configured {
            let filename = vm.read_string(data_words[1])?;
            read_inline_image_resource(self.resources, &filename)?
        } else {
            None
        };
        if operation == 0 {
            if buffer.as_ref().is_some_and(|bytes| bytes.len() == expected) {
                let defaults = self
                    .data_services
                    .messages
                    .defaults
                    .as_ref()
                    .ok_or_else(|| invalid("message raw defaults have not been initialized"))?;
                vm.write(data_words[1], defaults.configuration(encoding)?.bytes())?;
            }
            return Ok(true);
        }
        let appended = buffer.as_deref().filter(|_| operation == 3).map(|bytes| {
            let codec = self.data_services.cst_text_origins
                .returned(self.current_task_id, data_words[1], bytes)
                .unwrap_or(match encoding {
                    MessageEncoding::Cp932 => super::ScriptTextEncoding::Cp932,
                    MessageEncoding::Unicode => super::ScriptTextEncoding::Utf8,
                });
            let (text, bad_at) = codec.decode_message(bytes);
            if let Some(bad_at) = bad_at {
                self.data_services.note_text_decode(self.events, self.frame, format!(
                    "message command={command} codec={codec:?} bad_at={bad_at}",
                ));
            }
            text
        });
        let data = &mut *self.data_services;
        let mut context = MessagePlatformContext {
            device: &mut data.message_device,
            tasks: &data.tasks,
            ordinary: &data.ordinary,
            images: self.image_handles,
            nodes: self.scene_nodes,
            parents: self.task_parents,
            sender: self.current_task_id,
            drawing_available: self.canvas.is_some(),
            utate_string_receiver: false,
            legacy_string_receiver_45: false,
            legacy_text_decode_offset: None,
        };
        let Some(route) = context
            .route_message(target)?
            .filter(|route| route.deletion == 0)
        else {
            return Ok(true);
        };
        data.messages.register_encoding(target, encoding)?;
        let scale = context.message_time_scale();
        let object = data.messages.object_mut(target).unwrap();
        match operation {
            3 => {
                if let Some((bytes, text)) = buffer.as_deref().zip(appended.as_deref()) {
                    object.append_decoded(bytes, text)?;
                }
            }
            9 => {
                if configured {
                    object.set_user_glyph_source(source)?;
                }
            }
            21 => {
                // Failed resource reads preserve the previous source.
                if let Some(source) = inline_source {
                    object.set_inline_image_source(Some(source))?;
                }
            }
            20 if encoding == MessageEncoding::Unicode => {
                let area = object.draw_area(scale, &mut context)?;
                vm.write(data_words[0], &area[0].to_le_bytes())?;
                vm.write(data_words[1], &area[1].to_le_bytes())?;
            }
            _ => match object.text_command(
                command,
                data_words,
                buffer.as_deref(),
                route.display,
                route.elapsed,
                scale,
                &mut context,
            )? {
                MessageCommandResult::Unhandled => return Ok(false),
                MessageCommandResult::Handled => {}
                MessageCommandResult::Value(value) => *output = value,
            },
        }
        Ok(true)
    }

    /// Shared command hook for explicit raw47/48 dispatchers. The ordinary92
    /// path above already calls it; output slots remain owned by the sender.
    pub(super) fn dispatch_message_plane_display(
        &mut self,
        target: u32,
        command: u32,
        value: u32,
    ) -> Result<bool> {
        self.message_plane_display_inner(target, command, value, 0)
    }

    fn message_plane_display_inner(
        &mut self,
        target: u32,
        command: u32,
        value: u32,
        depth: usize,
    ) -> Result<bool> {
        if depth > 256 {
            return Err(invalid(
                "message plane display propagation exceeds 256 levels",
            ));
        }
        if !matches!(command, 47 | 48) {
            return Ok(false);
        }
        let Some(node) = self.scene_nodes.get(&target).cloned().filter(|node| {
            super::message_native::MessageNativeServices::handles_task_kind(node.kind)
        }) else {
            return Ok(false);
        };
        let Some((kind, deletion, elapsed)) = self.data_services.tasks.message_task_state(target)
        else {
            return Ok(true);
        };
        if deletion != 0 || kind != node.kind {
            return Ok(true);
        }
        let submit = if kind == super::legacy_text_native::LEGACY_STRING_KIND {
            self.data_services
                .messages
                .legacy
                .object(target)
                .is_some_and(|object| object.configuration.submit())
        } else {
            self.data_services
                .messages
                .object(target)
                .and_then(|object| object.configuration())
                .is_some_and(|configuration| configuration.ordinary_submit())
        };
        if command == 47 && submit {
            self.data_services.messages.set_plane_show(target, value);
        }
        let self_show = self.data_services.messages.plane_show(target);
        let base_reachable = command == 48 || submit;
        if base_reachable {
            if let Some(node) = self.scene_nodes.get_mut(&target) {
                node.visible = self_show != 0 && value != 0;
            }
        }
        let visible = self
            .scene_nodes
            .get(&target)
            .is_some_and(|node| node.visible)
            && self.canvas.is_some()
            && self.data_services.message_device.drawing_enabled;
        if kind == super::legacy_text_native::LEGACY_STRING_KIND {
            self.data_services.messages.legacy.display_command(
                target,
                value,
                command == 48,
                self_show,
                visible,
                elapsed,
                &self.data_services.message_device,
            )?;
        }
        if base_reachable {
            self.forward_message_plane_display(target, value, depth, true)?;
        }
        Ok(true)
    }

    fn forward_message_plane_display(
        &mut self,
        parent: u32,
        value: u32,
        depth: usize,
        update_planes: bool,
    ) -> Result<()> {
        if depth > 256 {
            return Err(invalid(
                "message plane display propagation exceeds 256 levels",
            ));
        }
        let mut children: Vec<_> = self
            .task_parents
            .iter()
            .filter_map(|(&id, &owner)| (owner == parent && id != parent).then_some(id))
            .collect();
        children.sort_by_key(|id| *id as i32);
        for id in children {
            let Some(node) = self.scene_nodes.get(&id) else {
                continue;
            };
            if self
                .data_services
                .tasks
                .message_task_state(id)
                .is_some_and(|(_, deletion, _)| deletion != 0)
            {
                continue;
            }
            if super::message_native::MessageNativeServices::handles_task_kind(node.kind) {
                // This call consumes its text branch and forwards its children.
                self.message_plane_display_inner(id, 48, value, depth + 1)?;
            } else if node.kind == 2 {
                if update_planes {
                    let mut output = 0;
                    self.dispatch_plane_command(
                        cs2_runtime::TaskCommand {
                            target: id as i32,
                            sender: parent as i32,
                            code: 48,
                            data: [0, value, 0],
                        },
                        &mut super::plane_native::PlaneCommandBuffers::default(),
                        &mut output,
                    )?;
                }
                self.forward_message_plane_display(id, value, depth + 1, false)?;
            }
        }
        Ok(())
    }
}

impl HeadlessPlayer {
    /// Plane flushes update existing region rectangles directly. Observe that
    /// real producer here without changing plane-owned storage or algorithms.
    /// Registration, removal, priority and selected-value writes are deliberately
    /// excluded: the original input127/128/129/133 do not set geometry dirty.
    pub(super) fn synchronize_message_input_geometry(&mut self) {
        let before: Vec<[u32; 4]> = self.input_task.regions.iter()
            .map(|region| region.words[3..7].try_into().unwrap())
            .collect();
        self.synchronize_plane_input_geometry();
        if before.iter().zip(&self.input_task.regions)
            .any(|(bounds, region)| bounds.as_slice() != &region.words[3..7])
        {
            self.mark_host_input_geometry_dirty();
        }
    }

    /// Producers with direct access to DesktopInputTask must call this for
    /// the same operations as input130/135/136, including missing targets.
    pub(super) fn mark_host_input_geometry_dirty(&mut self) {
        self.data_services.messages.input_geometry.mark_changed();
    }

    /// Enable only after all direct plane/FES producers publish dirty epochs.
    /// Until then selection retains its existing per-frame compatibility pass.
    pub(super) fn set_host_input_geometry_tracking(&mut self, complete: bool) {
        self.data_services.messages.input_geometry_tracking_complete = complete;
    }

    pub fn host_input_geometry_dirty(&self) -> bool {
        self.data_services.messages.input_geometry.is_dirty()
    }

    pub fn take_host_cursor_visibility_request(&mut self) -> Option<bool> {
        self.data_services
            .message_device
            .take_cursor_visibility_request()
    }

    pub fn host_cursor_visible(&self) -> bool {
        self.data_services.message_device.device_cursor_visible() != 0
    }

    /// A raw device snapshot, not a synthetic serialization of InputState.
    /// Like physical input, it is frozen at the next input-frame beginning.
    pub fn observe_host_device_history(&mut self, history: [u8; 32]) {
        self.data_services.messages.pending_device_history = Some(history);
    }

    pub fn host_input_cursor_state(&self) -> &cs2_runtime::message_object::InputCursorState {
        &self.data_services.messages.device_cursor
    }

    pub fn set_host_input_update_rate(&mut self, rate: u32) {
        self.data_services.messages.device_cursor.update_rate = rate;
    }

    pub fn set_host_input_drag_threshold(&mut self, threshold: u32) {
        self.data_services.messages.device_cursor.drag_threshold = threshold;
    }

    /// Bind NativeTaskStore's actual global number index, including its
    /// uncollected reservations. This is independent of scene visibility.
    pub(super) fn set_root_task_number_lookup(&mut self, lookup: Option<RootTaskNumberLookup>) {
        self.data_services.messages.root_task_number_lookup = lookup;
    }

    pub(super) fn set_fes_button_adapters(
        &mut self,
        lookup: Option<FesButtonTargetLookup>,
        media: Option<FesButtonMediaHook>,
        named: Option<FesButtonNamedHook>,
    ) {
        self.data_services.messages.fes_button_lookup = lookup;
        self.data_services.messages.fes_button_media = media;
        self.data_services.messages.fes_button_named = named;
    }

    pub(super) fn set_fes_button_focus_lock(&mut self, owner: u32, value: u32) {
        self.data_services
            .messages
            .fes_button_controllers
            .entry(owner)
            .or_default()
            .focus_lock = value;
    }

    pub(super) fn set_fes_button_record_lookup(&mut self, lookup: Option<FesButtonRecordLookup>) {
        self.data_services.messages.fes_button_records = lookup;
    }

    pub(super) fn fes_button_controller(&self, owner: u32) -> Option<&FesButtonController> {
        self.data_services
            .messages
            .fes_button_controllers
            .get(&owner)
    }

    /// A typed focus change completed or the typed picker revalidated the
    /// previous frame's focus. This gates focus only, not physical key edges.
    pub fn host_input_selection_routed(&self) -> bool {
        self.data_services.messages.input_selection_frame == Some(self.state.clock.frame)
    }

    /// The typed drag notification completed for every reached region.
    /// The legacy captured-control movement fallback can skip this frame.
    pub fn host_pointer_motion_routed(&self) -> bool {
        self.data_services.messages.pointer_motion_frame == Some(self.state.clock.frame)
    }

    /// Frame that opened the scan whose completed callbacks are retained.
    /// Deferred audio can keep a scan open across several logical frames.
    pub fn host_input_route_scan_frame(&self) -> Option<u64> {
        self.data_services.messages.key_scan_frame
    }

    /// Completed owner callbacks from the current scan, with their completion
    /// frames. Cleared when the next scan starts. Nonzero responses record
    /// delivery, while zero consumes the typed scan.
    pub fn host_input_route_receipts(&self) -> &[InputRouteReceipt] {
        &self.data_services.messages.input_route_receipts
    }

    /// Check after advance_host_device_keys_scan returns true. Pass the
    /// source and code saved with the original physical event, before any
    /// translation into a FES driver event. This only gates the legacy FES
    /// fallback; ordinary71/CST messages have their own delivery contract.
    pub fn host_button_fallback_disposition(
        &self,
        source: ButtonSource,
        code: u32,
        pressed: u32,
    ) -> InputFallbackDisposition {
        let messages = &self.data_services.messages;
        let code = code as u8;
        if messages.input_route_receipts.iter().any(|receipt| {
            receipt.frame == self.state.clock.frame
                && (receipt.pressed != 0) == (pressed != 0)
                && receipt
                    .physical
                    .iter()
                    .any(|button| button.source == source && button.code == code)
        }) {
            InputFallbackDisposition::TypedComplete
        } else if pressed == 0
            && messages.physical_edges.contains(&(source, code, 0))
            && messages.typed_physical_owners.contains_key(&(source, code))
        {
            InputFallbackDisposition::TypedOwnedRelease
        } else {
            InputFallbackDisposition::Unrouted
        }
    }

    /// Convenience adapter for a retained host event. It uses the same real
    /// device-code mapping as the observer; FES numeric codes are not inputs.
    pub fn host_input_fallback_disposition(
        &self,
        event: &cs2_runtime::InputEvent,
    ) -> InputFallbackDisposition {
        use cs2_runtime::{InputEvent, InputKey};
        match event {
            InputEvent::KeyDown(InputKey::MouseWheel(delta)) if *delta != 0 => self
                .host_button_fallback_disposition(
                    ButtonSource::Mouse,
                    if *delta > 0 { 6 } else { 7 },
                    1,
                ),
            InputEvent::KeyDown(key) | InputEvent::KeyUp(key) => {
                host_button(key).map_or(InputFallbackDisposition::Unrouted, |(source, code)| {
                    self.host_button_fallback_disposition(
                        source,
                        u32::from(code),
                        matches!(event, InputEvent::KeyDown(_)) as u32,
                    )
                })
            }
            _ => InputFallbackDisposition::Unrouted,
        }
    }

    pub(super) fn set_button_owner_hook(&mut self, hook: Option<ButtonOwnerHook>) {
        self.data_services.messages.button_owner_hook = hook;
    }

    pub(super) fn set_button_binding_lookup(&mut self, lookup: Option<ButtonBindingLookup>) {
        self.data_services.messages.button_binding_lookup = lookup;
    }

    pub fn input_button_state(&self, id: u32) -> Option<&InputButtonState> {
        self.data_services
            .messages
            .input_buttons
            .get(&id)
            .map(|button| &button.state)
    }

    pub fn set_input_hit_mask(
        &mut self,
        id: u32,
        mask: Option<InputHitMask>,
    ) -> anyhow::Result<()> {
        let Some(at) = self
            .input_task
            .regions
            .iter()
            .position(|region| region.id() == id)
        else {
            return Ok(());
        };
        if let Some(mask) = mask {
            mask.validate()?;
            let size = mask.size();
            self.data_services.messages.input_masks.insert(id, mask);
            self.input_task.regions[at].words[11] = 1;
            self.input_task.regions[at].words[12..14].copy_from_slice(&size);
        } else {
            self.data_services.messages.input_masks.remove(&id);
            self.input_task.regions[at].words[11..14].fill(0);
        }
        Ok(())
    }

    /// Same first-match priority order as input regions; selection uses the
    /// retained bitmap directly, or a positive-area rectangle when absent.
    pub fn masked_input_region_at(&self, point: [i32; 2]) -> anyhow::Result<Option<[u32; 15]>> {
        Ok(masked_region_at(
            &self.input_task.regions,
            &self.data_services.messages.input_masks,
            point,
            |_| true,
        )?)
    }

    pub fn host_replay_input_regions(&self) -> impl Iterator<Item = [u32; 15]> + '_ {
        self.input_task.regions.iter().map(|region| region.words)
    }

    pub fn mouse_press_candidate_regions(&mut self, point: [i32; 2]) -> anyhow::Result<Vec<u32>> {
        self.with_host(|host| {
            let mut matches = Vec::new();
            for &region in &host.input_task.regions {
                if host.device_press_region_eligible(region, ButtonSource::Mouse.mask(), point)? {
                    matches.push(region.id());
                }
            }
            Ok(matches)
        })
    }

    pub fn device_key_bindings(&self) -> &[DeviceBinding; 80] {
        self.data_services.messages.device_keys.bindings()
    }

    pub fn set_device_key_binding(
        &mut self,
        key: u32,
        binding: DeviceBinding,
    ) -> anyhow::Result<()> {
        self.ensure_device_keys()?;
        self.data_services
            .messages
            .device_keys
            .set_binding(key, binding)?;
        self.data_services
            .messages
            .key_sample_physical
            .retain(|(slot, _)| *slot != key);
        self.data_services
            .messages
            .typed_physical_owners
            .retain(|_, (slot, _, _)| *slot != key);
        self.publish_device_key_masks();
        Ok(())
    }

    pub fn apply_device_key_user_xml(&mut self, bytes: &[u8]) -> anyhow::Result<()> {
        self.ensure_device_keys()?;
        self.data_services
            .messages
            .device_keys
            .apply_user_xml(bytes)?;
        Ok(())
    }

    pub fn force_device_key(&mut self, key: u32, queries: u32) -> anyhow::Result<()> {
        self.ensure_device_keys()?;
        self.data_services
            .messages
            .device_keys
            .force(key, queries)?;
        Ok(())
    }

    pub fn device_key_scan_state(&self) -> &cs2_runtime::message_object::Keys {
        &self.data_services.messages.key_scan
    }

    /// Logical screen position requested by native150/raw138/139. Window
    /// hosts convert it through their actual viewport before warping an OS
    /// cursor; touch hosts may present this as a virtual pointer.
    pub fn take_host_pointer_request(&mut self) -> Option<[i32; 2]> {
        self.data_services.messages.pointer_request.take()
    }

    pub fn host_pointer_move_state(&self) -> &cs2_runtime::message_object::PointerMoveState {
        &self.data_services.messages.pointer_move
    }

    pub fn host_pointer_device_position(&self) -> [i32; 2] {
        self.data_services.messages.device_pointer
    }

    pub fn host_pointer_logical_position(&self) -> [i32; 2] {
        self.data_services.messages.device_logical_pointer
    }

    /// Optional raw activity metadata from a real pointer device poll.
    /// This introduces no second pointer/key event route. Samples arriving
    /// during a suspended frame become visible at the next frame beginning.
    pub fn observe_host_device_activity(&mut self, active: u32) {
        self.data_services.messages.pending_device_active = Some(active);
    }

    pub fn host_device_activity(&self) -> u32 {
        self.data_services.messages.device_sample_active
    }

    /// The scan owns the active region callback, which may enter a FES owner
    /// through a lower BUTTON rather than through the root script's VM.
    pub(super) fn input_dispatch_waits_for_audio(&mut self) -> anyhow::Result<bool> {
        if self.root_dispatch_waits_for_audio()? { return Ok(true); }
        let messages = &self.data_services.messages;
        let targets = [
            messages.key_selection.as_ref().and_then(|dispatch| dispatch.current.map(|(words, _, _)| words[0])),
            messages.key_dispatch.as_ref().and_then(|dispatch| dispatch.current.map(|(words, _)| words[0])),
            messages.pointer_dispatch.as_ref().and_then(|dispatch| dispatch.current.map(|(words, _)| words[0])),
        ];
        self.with_host(|host| Ok(targets.into_iter().flatten().any(|target| {
            host.task_waits_for_audio(target)
                || host.data_services.messages.button_binding_lookup.and_then(|lookup| lookup(host, target))
                    .is_some_and(|owner| host.task_waits_for_audio(owner))
                || host.data_services.messages.fes_button_lookup.and_then(|lookup| lookup(host, target))
                    .is_some_and(|(owner, _)| host.task_waits_for_audio(owner))
        })))
    }

    /// Logical input callbacks may run scripts and yield. The frame phase
    /// calling this method must remain pending until it returns true.
    pub(super) fn advance_host_device_keys_scan(&mut self, budget: usize) -> anyhow::Result<bool> {
        if self.data_services.messages.key_scan_frame != Some(self.state.clock.frame)
            && !self.audio_input_deferred
        {
            if self.data_services.messages.key_dispatch.is_some()
                || self.data_services.messages.key_selection.is_some()
                || self.data_services.messages.pointer_dispatch.is_some()
                || self.data_services.messages.cursor_frame.is_some()
            {
                anyhow::bail!("finish the previous input callback before starting another frame");
            }
            self.data_services.messages.key_scan.begin_scan()?;
            self.data_services.messages.input_route_receipts.clear();
            self.data_services.messages.button_event_completions.clear();
            self.data_services.messages.key_selection_done = false;
            self.data_services.messages.pointer_tail_done = false;
            self.data_services.messages.cursor_frame_done = false;
            self.data_services.messages.input_frame_complete = false;
            self.data_services.messages.key_scan_frame = Some(self.state.clock.frame);
        }
        if !self.service_pending_root_dispatch(budget)? {
            return Ok(false);
        }
        self.with_host(|host| Ok(host.scan_device_keys(budget)?))
    }

    pub fn host_tick_state(&self) -> String {
        let messages = &self.data_services.messages;
        let selection = messages.key_selection.as_ref().map(|pending| {
            let target = pending.current.map(|(words, _, command)| (words[0], command));
            let receiver = target.and_then(|(id, _)| messages.input_buttons.get(&id))
                .and_then(|button| button.state.receiver);
            let driver = receiver.and_then(|id| self.data_services.tasks.fes.runtime(id))
                .and_then(|runtime| runtime.driver.as_ref());
            (pending.stage, pending.next, target, receiver, driver.map(super::fes_driver::FesScriptDriver::initialization_pending),
                receiver.and_then(|id| messages.fes_button_owners.get(&id)).map(|dispatch| dispatch.stage))
        });
        format!("frame={} phase={:?} root={:?} selection={} pending_selection={selection:?} cursor={} pointer={} scan_complete={} cursor_broadcast={:?}",
            self.state.clock.frame, self.frame_phase, self.data_services.tasks.pending_root_dispatch(),
            messages.key_selection_done, messages.cursor_frame_done, messages.pointer_tail_done,
            messages.input_frame_complete, messages.cursor_frame.as_ref().and_then(|cursor| cursor.broadcast.as_ref())
                .and_then(|broadcast| broadcast.stack.last()))
    }

    /// Joypad/touch backends supply actual device button codes here. Keyboard
    /// codes are engine Windows virtual-key bytes, not Android KeyEvent codes.
    pub fn apply_host_device_button(
        &mut self,
        source: ButtonSource,
        code: u32,
        pressed: u32,
    ) -> anyhow::Result<()> {
        self.ensure_device_keys()?;
        if source == ButtonSource::Joypad {
            self.with_host(|host| host.ensure_configuration_state())?;
            if self.data_services.configuration.platform_value("KEY/enable_joypad").unwrap_or(0) == 0 { return Ok(()); }
        }
        if self.frame_in_progress() || self.audio_input_deferred {
            let pending = &mut self.data_services.messages.pending_device_buttons;
            if pending.len() >= 4096 {
                anyhow::bail!("platform device button queue exceeds 4096 events");
            }
            pending.push_back((source, code, pressed));
            return Ok(());
        }
        self.observe_device_button(source, code, pressed);
        Ok(())
    }

    /// Call in the common host-input path after its pending-event gate and
    /// before applying the event. This observer is idempotent for held keys.
    pub(super) fn observe_host_device_input(
        &mut self,
        event: &cs2_runtime::InputEvent,
    ) -> anyhow::Result<()> {
        use cs2_runtime::{InputEvent, InputKey};
        self.ensure_device_keys()?;
        match event {
            InputEvent::MouseMove { x, y } => {
                let coordinates = self.data_services.host_input_coordinates.ok_or_else(|| {
                    invalid("host input has no explicitly bound coordinate model")
                })?;
                let logical = [*x, *y];
                self.data_services.messages.device_pointer = coordinates.to_device(logical).0;
                self.data_services.messages.device_logical_pointer = logical;
                self.data_services.messages.device_task_pointer = logical;
                self.data_services.messages.pointer_saved = Some(logical);
            }
            InputEvent::FocusChanged(value) => {
                self.data_services.messages.device_foreground = *value;
                if !value {
                    self.data_services.messages.device_keys.clear_pressed();
                    self.data_services.messages.physical_buttons.clear();
                    self.data_services.messages.physical_edges.clear();
                    self.data_services.messages.typed_physical_owners.clear();
                    self.data_services.messages.ime_device_keys.clear();
                    self.data_services.messages.ime_device_pulses.clear();
                    self.data_services.messages.pending_device_buttons.clear();
                    self.data_services.messages.wheel_buttons.cancel_repeat();
                    self.data_services.messages.wheel_buttons.finish();
                    self.data_services.messages.wheel_observed = 0;
                    self.publish_device_key_masks();
                }
            }
            InputEvent::KeyDown(key) | InputEvent::KeyUp(key) => {
                let down = matches!(event, InputEvent::KeyDown(_));
                match key {
                    InputKey::MouseWheel(delta) if down && *delta != 0 => {
                        let length = self.state.input.wheel_events.len();
                        let observed = self.data_services.messages.wheel_observed.min(length);
                        let unseen = self.state.input.wheel_events[observed..].to_vec();
                        for value in unseen.into_iter().filter(|value| *value != 0) {
                            self.observe_device_wheel(value > 0);
                        }
                        self.observe_device_wheel(*delta > 0);
                        // The common host applies this event immediately
                        // after its observer; frame-start fallback skips it.
                        self.data_services.messages.wheel_observed = length + 1;
                    }
                    _ => {
                        if let Some((source, code)) = host_button(key) {
                            self.observe_device_button(source, u32::from(code), down as u32);
                        }
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Frame-start bridge for platforms that apply InputState directly. Call
    /// once before task/FES key queries; edge snapshots preserve short taps.
    pub(super) fn begin_host_device_keys_frame(&mut self) -> anyhow::Result<()> {
        if self.audio_input_deferred { return Ok(()); }
        self.ensure_device_keys()?;
        self.synchronize_message_input_geometry();
        if self.input_task.owner_id.is_none() {
            self.input_task.owner_id = self.scene_nodes.iter().find_map(|(&id, node)| {
                (node.kind == 11
                    && self
                        .data_services
                        .tasks
                        .message_task_state(id)
                        .is_some_and(|(kind, deletion, _)| kind == 11 && deletion == 0))
                .then_some(id)
            });
        }
        self.data_services.messages.device_foreground = self.state.input.focused;
        self.data_services.messages.device_sample_active = self
            .data_services
            .messages
            .pending_device_active
            .take()
            .unwrap_or(u32::from(self.state.input.focused));
        self.data_services.messages.frame_device_history =
            self.data_services.messages.pending_device_history.take();
        let observed = [self.state.input.cursor_x, self.state.input.cursor_y];
        let coordinates = self
            .data_services
            .host_input_coordinates
            .ok_or_else(|| invalid("input frame has no explicitly bound coordinate model"))?;
        let elapsed = self
            .input_task
            .owner_id
            .and_then(|id| self.data_services.tasks.message_task_state(id))
            .map_or(self.state.clock.elapsed_ticks as u32, |(_, _, elapsed)| {
                elapsed
            });
        let messages = &mut self.data_services.messages;
        if messages.pointer_saved != Some(observed) {
            messages.device_pointer = coordinates.to_device(observed).0;
        }
        messages.device_pointer_observed = messages.device_pointer;
        let update = messages
            .pointer_move
            .advance(messages.device_pointer, elapsed);
        let (device, task) = update.map_or(
            (messages.device_pointer, messages.device_pointer),
            |update| (update.requested, update.task_position),
        );
        messages.device_pointer = device;
        let logical = coordinates.to_logical(device).0;
        messages.device_logical_pointer = logical;
        messages.device_task_pointer = coordinates.to_logical(task).0;
        messages.device_task_position = task;
        messages.device_pointer_moving = update.is_some();
        if update.is_some() {
            messages.pointer_request = Some(logical);
        }
        // Real logical input used by selection/queries, with the task's saved
        // coordinate pair kept separately for message4 and movement history.
        self.state.apply_input(cs2_runtime::InputEvent::MouseMove {
            x: logical[0],
            y: logical[1],
        });
        self.input_task.pointer_position = messages.device_task_pointer;
        messages.pointer_saved = Some(logical);
        while let Some((source, code, pressed)) = self
            .data_services
            .messages
            .pending_device_buttons
            .pop_front()
        {
            self.observe_device_button(source, code, pressed);
        }
        let actual: std::collections::BTreeSet<_> = self
            .state
            .input
            .pressed
            .iter()
            .filter_map(host_button)
            .filter(|(source, code)| {
                *source != ButtonSource::Keyboard
                    || !(self.data_services.messages.ime_device_keys.contains(code)
                        || self.data_services.messages.ime_device_pulses.contains(code))
            })
            .collect();
        let old: Vec<_> = self
            .data_services
            .messages
            .physical_buttons
            .iter()
            .filter(|(source, _)| matches!(source, ButtonSource::Keyboard | ButtonSource::Mouse))
            .copied()
            .collect();
        for (source, code) in old {
            if !actual.contains(&(source, code)) {
                self.observe_device_button(source, u32::from(code), 0);
            }
        }
        for &(source, code) in &actual {
            if !self
                .data_services
                .messages
                .physical_buttons
                .contains(&(source, code))
            {
                self.observe_device_button(source, u32::from(code), 1);
            }
        }
        let short: Vec<_> = self
            .state
            .input
            .just_pressed
            .iter()
            .filter_map(host_button)
            .filter(|button| !actual.contains(button))
            .filter(|(source, code)| {
                *source != ButtonSource::Keyboard
                    || !(self.data_services.messages.ime_device_keys.contains(code)
                        || self.data_services.messages.ime_device_pulses.contains(code))
            })
            .collect();
        for (source, code) in short {
            if !self
                .data_services
                .messages
                .physical_edges
                .contains(&(source, code, 1))
            {
                self.observe_device_button(source, u32::from(code), 1);
                self.observe_device_button(source, u32::from(code), 0);
            }
        }
        let length = self.state.input.wheel_events.len();
        let observed = self.data_services.messages.wheel_observed.min(length);
        let wheel = self.state.input.wheel_events[observed..].to_vec();
        for delta in wheel.into_iter().filter(|delta| *delta != 0) {
            self.observe_device_wheel(delta > 0);
        }
        self.data_services.messages.wheel_observed = length;
        self.publish_device_key_masks();
        Ok(())
    }

    pub(super) fn ensure_device_keys(&mut self) -> Result<()> {
        if !self.data_services.messages.device_keys_loaded {
            self.data_services
                .message_device
                .load_fonts(&self.scan.resources);
            initialize_settings(
                &mut self.data_services,
                &self.scan.resources,
                Some(&self.save_root),
            )?;
        }
        self.data_services
            .messages
            .device_keys
            .set_debug_mode(self.debug_mode);
        Ok(())
    }

    fn observe_device_button(&mut self, source: ButtonSource, code: u32, pressed: u32) {
        if source == ButtonSource::Joypad
            && self.data_services.configuration.platform_value("KEY/enable_joypad").unwrap_or(0) == 0 { return; }
        let button = (source, code as u8);
        let changed = if pressed != 0 {
            self.data_services.messages.physical_buttons.insert(button)
        } else {
            self.data_services.messages.physical_buttons.remove(&button)
        };
        if changed {
            self.data_services.messages.physical_edges.insert((
                source,
                code as u8,
                (pressed != 0) as u32,
            ));
            if pressed != 0 {
                self.data_services
                    .messages
                    .typed_physical_owners
                    .remove(&button);
            }
        }
        self.data_services
            .messages
            .device_keys
            .set_debug_mode(self.debug_mode);
        self.data_services
            .messages
            .device_keys
            .button(source, code, pressed);
        self.publish_device_key_masks();
    }

    fn observe_device_wheel(&mut self, up: bool) {
        let event = self.data_services.messages.wheel_buttons.scroll(up);
        self.data_services.messages.physical_edges.insert((
            ButtonSource::Mouse,
            event.code as u8,
            1,
        ));
        self.data_services
            .messages
            .typed_physical_owners
            .remove(&(ButtonSource::Mouse, event.code as u8));
        self.data_services.messages.device_keys.button(
            ButtonSource::Mouse,
            event.code,
            event.pressed,
        );
        self.publish_device_key_masks();
    }

    fn publish_device_key_masks(&mut self) {
        self.input_task.key_masks = *self.data_services.messages.device_keys.masks();
    }
    pub(super) fn set_host_joypad_enabled(&mut self, value: u32) {
        self.data_services.configuration.set_input_joypad_enabled(value);
        if value == 0 {
            let messages = &mut self.data_services.messages;
            messages.physical_buttons.retain(|(source, _)| *source != ButtonSource::Joypad);
            messages.physical_edges.retain(|(source, _, _)| *source != ButtonSource::Joypad);
            messages.pending_device_buttons.retain(|(source, _, _)| *source != ButtonSource::Joypad);
            messages.typed_physical_owners.retain(|(source, _), _| *source != ButtonSource::Joypad);
        }
    }

    /// UTF-8 commit/preedit bridge shared by Android JNI and window hosts.
    /// Composition does not enter the game's committed text queue.
    pub fn apply_host_text_input(
        &mut self,
        event: cs2_runtime::message_object::MessageTextInputEvent,
    ) -> anyhow::Result<()> {
        use cs2_runtime::message_object::MessageTextInputEvent;
        let focus = match &event {
            MessageTextInputEvent::Focus(value) => Some(*value),
            _ => None,
        };
        let committed = self.data_services.messages.text_input.apply(event)?;
        if let Some(focused) = focus {
            if !focused {
                self.observe_host_device_input(&cs2_runtime::InputEvent::FocusChanged(false))?;
            }
            self.apply_host_input(cs2_runtime::InputEvent::FocusChanged(focused))?;
        }
        if let Some(text) = committed {
            self.apply_host_input(cs2_runtime::InputEvent::Text(text))?;
        }
        Ok(())
    }

    pub fn host_text_input_state(&self) -> &cs2_runtime::message_object::MessageTextInputState {
        &self.data_services.messages.text_input
    }

    /// Feed hardware keys here while an IME may own Enter/Space/arrows.
    /// Physical key state is retained; preedit suppresses game/FES activation.
    pub fn apply_host_text_key(&mut self, key: u32, down: bool) -> anyhow::Result<()> {
        let consumed = self
            .data_services
            .messages
            .text_input
            .consumes_key(key, down)?;
        if consumed {
            self.ensure_device_keys()?;
            self.data_services
                .messages
                .ime_device_pulses
                .insert(key as u8);
            if down {
                self.data_services
                    .messages
                    .ime_device_keys
                    .insert(key as u8);
                self.observe_device_button(ButtonSource::Keyboard, key, 0);
            } else {
                self.data_services
                    .messages
                    .ime_device_keys
                    .remove(&(key as u8));
            }
        }
        let key = cs2_runtime::InputKey::Keyboard(key);
        let event = if down {
            cs2_runtime::InputEvent::KeyDown(key)
        } else {
            cs2_runtime::InputEvent::KeyUp(key)
        };
        if consumed {
            self.state.apply_input(event);
            Ok(())
        } else {
            if !self.frame_in_progress() {
                self.observe_host_device_input(&event)?;
            }
            self.apply_host_input(event)
        }
    }

    pub fn message_font_substitutions(
        &self,
    ) -> Vec<cs2_runtime::message_object::MessageFontSubstitution> {
        self.data_services.message_device.font_substitutions()
    }
    /// Optional explicit startup wiring. Without global defaults native 611
    /// diagnoses their absence, as the document specifies; native 612 can still
    /// accept a fully supplied configuration. Call after startup XML/settings.
    pub(super) fn initialize_message_objects(
        &mut self,
        defaults: Option<MessageDefaults>,
        time_scale: Option<u32>,
    ) {
        self.data_services.messages.defaults = defaults;
        self.data_services.message_device.time_scale = time_scale;
        self.data_services.message_device.drawing_enabled = true;
        self.data_services.message_device.settings_loaded = true;
    }

    /// Optional resource initializer for the documented global timing setting.
    /// No fabricated time_scale=1: missing GAME/strframe_frame means zero in
    /// this startup adapter, while an uninitialized independent device is None.
    pub(super) fn initialize_message_timing(&mut self) -> Result<()> {
        self.data_services
            .message_device
            .load_fonts(&self.scan.resources);
        initialize_settings(
            &mut self.data_services,
            &self.scan.resources,
            Some(&self.save_root),
        )
    }

    /// Signature for the main player's frame hook. Run once after ordinary task
    /// clocks/children have advanced, before render_scene. No private font field
    /// needs to be added to LoggingHost.
    pub(super) fn advance_message_objects(&mut self) -> anyhow::Result<()> {
        self.data_services
            .message_device
            .load_fonts(&self.scan.resources);
        if !self.data_services.message_device.settings_loaded {
            initialize_settings(
                &mut self.data_services,
                &self.scan.resources,
                Some(&self.save_root),
            )?;
        }
        self.with_host(|host| {
            while let Some((target, packet)) = host.data_services.messages.pending_display() {
                // Already accepted by native70: deletion removes the packet,
                // while a live packet uses reception-time task elapsed time.
                host.dispatch_message_plane_display(target, packet[1], packet[3])?;
                host.data_services.messages.finish_display();
            }
            Ok(())
        })?;
        let ids: Vec<_> = self
            .data_services
            .messages
            .objects()
            .keys()
            .copied()
            .collect();
        for id in ids {
            let state = self.data_services.tasks.message_task_state(id);
            let legacy_kind = self.data_services.messages.legacy_string_kind(id);
            let live = state.is_some_and(|(kind, deletion, _)| {
                deletion == 0
                    && (matches!(kind, 48 | 63) || legacy_kind == Some(kind))
                    && self
                        .scene_nodes
                        .get(&id)
                        .is_some_and(|node| node.kind == kind)
            });
            if !live {
                self.data_services.messages.release(id);
                self.data_services.message_device.release(id as i32);
                continue;
            }
            let (_, deletion, elapsed) = state.unwrap();
            let data = &mut self.data_services;
            let mut context = MessagePlatformContext {
                device: &mut data.message_device,
                tasks: &data.tasks,
                ordinary: &data.ordinary,
                images: &self.image_handles,
                nodes: &self.scene_nodes,
                parents: &self.task_parents,
                sender: self.current_task_id,
                drawing_available: true,
                utate_string_receiver: legacy_kind == Some(UTATE_STRING_KIND),
                legacy_string_receiver_45: legacy_kind == Some(LEGACY_STRING_KIND_45),
                legacy_text_decode_offset: None,
            };
            data.messages
                .frame_update(id, elapsed, &self.scene_nodes, deletion, &mut context)?;
            if let Some(object) = data.messages.object(id) {
                context.device.sync_object(object)?;
            }
        }
        let legacy_ids: Vec<_> = self
            .data_services
            .messages
            .legacy
            .objects()
            .keys()
            .copied()
            .collect();
        for id in legacy_ids {
            let live = self.data_services.tasks.message_task_state(id).is_some_and(
                |(kind, deletion, _)| {
                    kind == super::legacy_text_native::LEGACY_STRING_KIND && deletion == 0
                },
            ) && self
                .scene_nodes
                .get(&id)
                .is_some_and(|node| node.kind == super::legacy_text_native::LEGACY_STRING_KIND);
            if !live {
                self.data_services.messages.release(id);
            } else {
                let elapsed = self.data_services.tasks.message_task_state(id).unwrap().2;
                let visible = self.scene_nodes.get(&id).is_some_and(|node| node.visible);
                self.data_services.messages.legacy.frame_update(
                    id,
                    elapsed,
                    visible,
                    Some(60),
                    &self.data_services.message_device,
                )?;
            }
        }
        Ok(())
    }

    /// Commit physical edge state only after a complete logical frame renders.
    /// Script budget continuations must keep the current samples intact.
    pub(super) fn commit_host_device_keys_frame(&mut self) {
        if self.audio_input_deferred { return; }
        for event in self.data_services.messages.wheel_buttons.finish() {
            self.data_services.messages.device_keys.button(
                ButtonSource::Mouse,
                event.code,
                event.pressed,
            );
        }
        self.data_services.messages.device_keys.commit();
        self.data_services.messages.physical_edges.clear();
        self.data_services.messages.ime_device_pulses.clear();
        self.data_services.messages.wheel_observed = 0;
        self.publish_device_key_masks();
        // Windows also feeds InputState directly. Consume its physical edge
        // snapshots at this same successful-frame boundary so begin's bridge
        // cannot replay one short tap or wheel message on later frames.
        self.state.input.commit();
    }

    /// Composite all submitted message and legacy text surfaces.
    pub(super) fn render_message_objects(&mut self) -> anyhow::Result<()> {
        self.render_message_objects_range(f32::NEG_INFINITY, f32::INFINITY)
    }

    pub(super) fn project_message_plane<'node>(
        &self,
        node: &'node super::SceneNode,
    ) -> Result<std::borrow::Cow<'node, super::SceneNode>> {
        if self.data_services.messages.legacy_string_kind(node.id) != Some(node.kind)
            || self.data_services.planes.state(node.id).is_none()
        {
            return Ok(std::borrow::Cow::Borrowed(node));
        }
        let view = super::plane_native::kx_plane_view(node, &self.data_services.planes)?;
        let mut projected = node.clone();
        projected.position[0] -= f32::from_bits(view.words[3]);
        projected.position[1] -= f32::from_bits(view.words[4]);
        projected.position[2] = f32::from_bits(view.words[5]);
        Ok(std::borrow::Cow::Owned(projected))
    }

    /// Composite into the current work surface using the original node depth.
    /// Repeated calls may draw the same objects into different work surfaces.
    pub(super) fn render_message_objects_range(
        &mut self,
        lower: f32,
        upper: f32,
    ) -> anyhow::Result<()> {
        let mut ids: Vec<_> = self
            .data_services
            .messages
            .objects()
            .keys()
            .copied()
            .collect();
        ids.sort_by_key(|id| {
            self.scene_nodes
                .get(id)
                .map(|node| (node.position[2] as i32, *id as i32))
        });
        for id in ids {
            let Some(node) = self.scene_nodes.get(&id) else {
                continue;
            };
            let legacy = self.data_services.messages.legacy_string_kind(id) == Some(node.kind);
            if (!matches!(node.kind, 48 | 63) && !legacy) || !node.visible {
                continue;
            }
            if self
                .data_services
                .tasks
                .message_task_state(id)
                .is_none_or(|(kind, deletion, _)| kind != node.kind || deletion != 0)
            {
                continue;
            }
            let projection = self.project_message_plane(node)?;
            let node = projection.as_ref();
            if !(lower <= node.position[2] && node.position[2] < upper) {
                continue;
            }
            let Some(object) = self.data_services.messages.object(id) else {
                continue;
            };
            self.data_services.message_device.sync_object(object)?;
            let Some(configuration) = object.configuration() else {
                continue;
            };
            let Some((registered_size, registered_offset)) = object.registered_geometry() else {
                continue;
            };
            if node
                .position
                .iter()
                .chain(node.scale.iter())
                .any(|value| !value.is_finite())
            {
                return Err(anyhow::anyhow!(
                    "message plane contains nonfinite coordinates"
                ));
            }
            let current_offset = configuration.canvas_offset();
            // Reconfigure can move the copied pixels inside the fixed source
            // rectangle. Keep that clip before scaling the registered plane.
            let shift = std::array::from_fn(|axis| i64::from(current_offset[axis]) - i64::from(registered_offset[axis]));
            let tint = super::message_backend::message_plane_tint(node);
            let Some(frame) = self.data_services.message_device.projection_frame_shifted(id as i32, tint, shift)? else {
                continue;
            };
            super::message_backend::draw_message_plane_with_tint(
                &mut self.canvas,
                frame,
                node,
                registered_offset,
                registered_size,
                u32::MAX,
            )?;
        }
        let mut legacy_ids: Vec<_> = self
            .data_services
            .messages
            .legacy
            .objects()
            .keys()
            .copied()
            .collect();
        legacy_ids.sort_by_key(|id| {
            self.scene_nodes
                .get(id)
                .map(|node| (node.position[2] as i32, *id as i32))
        });
        for id in legacy_ids {
            let Some(node) = self.scene_nodes.get(&id) else {
                continue;
            };
            if !(lower <= node.position[2] && node.position[2] < upper) {
                continue;
            }
            if self
                .data_services
                .tasks
                .message_task_state(id)
                .is_none_or(|(kind, deletion, _)| {
                    kind != super::legacy_text_native::LEGACY_STRING_KIND || deletion != 0
                })
            {
                continue;
            }
            if let Some(object) = self.data_services.messages.legacy.object(id) {
                self.canvas.try_pixels()?;
                object.draw(&mut self.canvas, node)?;
            }
        }
        Ok(())
    }
}

struct MessagePlatformContext<'a> {
    device: &'a mut DesktopMessageDevice,
    tasks: &'a NativeTaskStore,
    ordinary: &'a OrdinaryNativeServices,
    images: &'a BTreeMap<u32, RgbaFrame>,
    nodes: &'a BTreeMap<u32, SceneNode>,
    parents: &'a BTreeMap<u32, u32>,
    sender: u32,
    drawing_available: bool,
    utate_string_receiver: bool,
    legacy_string_receiver_45: bool,
    legacy_text_decode_offset: Option<usize>,
}

impl MessageBackend for MessagePlatformContext<'_> {
    fn layout_with_break_characters(
        &mut self,
        configuration: &MessageConfiguration,
        atoms: &[MessageAtom],
        previous: Option<&MessageLayout>,
        characters: &MessageBreakCharacters,
    ) -> Result<MessageLayout> {
        self.device
            .layout_with_break_characters(configuration, atoms, previous, characters)
    }
    fn redraw_message(
        &mut self,
        object: &cs2_runtime::message_object::MessageObject,
    ) -> Result<bool> {
        self.device.redraw_message(object)
    }
    fn clear_message(&mut self, object: &cs2_runtime::message_object::MessageObject) -> Result<()> {
        if self.drawing_available {
            self.device.clear_message(object)
        } else {
            self.device.sync_object(object)
        }
    }
    fn layout(
        &mut self,
        configuration: &MessageConfiguration,
        atoms: &[MessageAtom],
        previous: Option<&MessageLayout>,
    ) -> Result<MessageLayout> {
        self.device.layout(configuration, atoms, previous)
    }
    fn decode_user_glyph(&mut self, source: &[u8], frame: u32) -> Result<Option<MessageUserGlyph>> {
        if self.utate_string_receiver {
            super::message_native::decode_utate_user_glyph(source, frame)
        } else {
            self.device.decode_user_glyph(source, frame)
        }
    }
    fn paint(&mut self, request: MessageRenderRequest<'_>) -> Result<MessagePaintResult> {
        self.device.paint(request)
    }
}

impl MessageNativeContext for MessagePlatformContext<'_> {
    fn note_legacy_text_decode(&mut self, offset: usize) {
        self.legacy_text_decode_offset = Some(offset);
    }

    fn utate_string_receiver(&self) -> bool {
        self.utate_string_receiver
    }

    fn legacy_string_receiver_45(&self) -> bool {
        self.legacy_string_receiver_45
    }

    fn route_message(&mut self, raw_target: u32) -> Result<Option<MessageRoute>> {
        // Mirror TaskTree::route's subtree-then-parent search using the
        // desktop's live parent map. This searches raw zero too.
        let mut from = Some(self.sender);
        let mut reachable = false;
        for _ in 0..=256 {
            let Some(ancestor) = from else {
                break;
            };
            if in_subtree(self.parents, ancestor, raw_target) {
                reachable = true;
                break;
            }
            from = self
                .parents
                .get(&ancestor)
                .copied()
                .filter(|parent| *parent != ancestor);
        }
        if !reachable {
            return Ok(None);
        }
        let Some((kind, deletion, elapsed)) = self.tasks.message_task_state(raw_target) else {
            return Ok(None);
        };
        // Wrong-type handling is left to the adapter's SceneNode check; retain
        // the raw target and deletion so quantity queries get a diagnostic.
        let text_plane = matches!(
            kind,
            48 | 63 | super::legacy_text_native::LEGACY_STRING_KIND
        ) || (self.utate_string_receiver && kind == UTATE_STRING_KIND)
            || (self.legacy_string_receiver_45 && kind == LEGACY_STRING_KIND_45);
        let display = if !text_plane || !self.drawing_available || !self.device.drawing_enabled {
            MessagePlaneDisplay::Unreachable
        } else if self.nodes.get(&raw_target).is_some_and(|node| node.visible) {
            MessagePlaneDisplay::Visible
        } else {
            MessagePlaneDisplay::Hidden
        };
        Ok(Some(MessageRoute {
            id: raw_target,
            deletion,
            elapsed,
            display,
        }))
    }
    fn raw_message_image(&mut self, handle: u32) -> Result<Vec<u8>> {
        self.ordinary.raw_image(handle)
    }
    fn message_image(&mut self, handle: u32) -> Result<RgbaFrame> {
        self.images
            .get(&handle)
            .cloned()
            .ok_or_else(|| invalid("legacy message source is not a live image wrapper"))
    }
    fn message_ruby_font_name(&self) -> Vec<u16> {
        self.device.ruby_font_name.clone()
    }
    fn message_time_scale(&self) -> Option<u32> {
        self.device.time_scale
    }
    fn legacy_frame_rate(&self) -> u32 {
        // This player's task clocks advance one logical tick per 1/60-second
        // frame. Do not use the unrelated global string-time ratio here.
        60
    }
}

fn in_subtree(parents: &BTreeMap<u32, u32>, ancestor: u32, target: u32) -> bool {
    let mut cursor = Some(target);
    for _ in 0..=256 {
        let Some(id) = cursor else {
            return false;
        };
        if id == ancestor {
            return true;
        }
        cursor = parents.get(&id).copied().filter(|parent| *parent != id);
    }
    false
}

/// Defaults follow the documented settings constructor: outline/shadow/bold/
/// proportional/fixed = [1,1,0,1,0]. An absent face uses the font service's
/// unnamed request; it never invents a font family. All scalar fields retain
/// their raw DWORDs until MessageDefaults applies the documented bool choices.
fn initialize_settings(
    data: &mut super::data_services::DataServices,
    resources: &cs2_runtime::ResourceIndex,
    save_root: Option<&std::path::Path>,
) -> Result<()> {
    let startup = resources
        .startup_configuration()
        .map_err(|_| invalid("message startup configuration load failed"))?;
    let load_keys = !data.messages.device_keys_loaded;
    if load_keys {
        data.messages.device_keys = startup
            .as_ref()
            .map(DeviceKeys::from_configuration)
            .unwrap_or_default();
    }
    let mut controls = [1u32, 1, 0, 1, 0];
    let mut main = Vec::new();
    let mut ruby = Vec::new();
    if let Some(xml) = &startup {
        apply_font_xml(
            xml,
            &data.message_device,
            &mut main,
            &mut ruby,
            &mut controls,
        )?;
    }
    // gamedef explicitly preserves font fields; user settings override them.
    // Configuration Services may supply another path through apply_message_settings_xml.
    for name in ["settings.xml", "config.xml"] {
        if let Some(xml) = resources
            .read_optional_configuration(name)
            .map_err(|_| invalid("message settings configuration load failed"))?
        {
            if load_keys {
                data.messages.device_keys.apply_user_configuration(&xml);
            }
            apply_font_xml(
                &xml,
                &data.message_device,
                &mut main,
                &mut ruby,
                &mut controls,
            )?;
        }
    }
    if data.messages.font_panel_defaults.is_none() {
        data.messages.font_panel_defaults = Some(MessageDefaults {
            font_name: main.clone(), bold: controls[2] != 0, outline: controls[0], shadow: controls[1],
            glyph_parameter: Some(if controls[3] == 0 { 1 } else { controls[4] }), gradual: false,
        });
    }
    if let Some(root) = save_root {
        for name in ["settings.xml", "config.xml", "setup.xml"] {
            let path = root.join(name);
            match std::fs::read(path) {
                Ok(bytes) => {
                    let xml = ConfigXml::parse_runtime(&bytes)
                        .map_err(|_| invalid("user message font settings XML is malformed"))?;
                    if load_keys {
                        data.messages.device_keys.apply_user_configuration(&xml);
                    }
                    apply_font_xml(
                        &xml,
                        &data.message_device,
                        &mut main,
                        &mut ruby,
                        &mut controls,
                    )?;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return Err(invalid("user message font settings could not be read")),
            }
        }
    }
    let scale = startup.as_ref().map_or(0, |xml| {
        super::xml_integer(xml, "document/GAME/strframe_frame").unwrap_or(0)
    });
    let defaults = MessageDefaults {
        font_name: main,
        bold: controls[2] != 0,
        outline: controls[0],
        shadow: controls[1],
        glyph_parameter: Some(if controls[3] == 0 { 1 } else { controls[4] }),
        gradual: false,
    };
    if data.messages.defaults.is_none() {
        data.messages.defaults = Some(defaults);
    }
    data.message_device.ruby_font_name = ruby;
    data.message_device.set_raw_font_controls(controls);
    if data.message_device.time_scale.is_none() {
        data.message_device.time_scale = Some(scale);
    }
    data.message_device.settings_loaded = true;
    data.messages.device_keys_loaded = true;
    Ok(())
}

impl HeadlessPlayer {
    /// A settings host with an authoritative user-config path can apply the
    /// already parsed XML here, avoiding any filename assumptions.
    pub(super) fn apply_message_settings_xml(&mut self, xml: &ConfigXml) -> Result<()> {
        self.ensure_device_keys()?;
        self.data_services
            .messages
            .device_keys
            .apply_user_configuration(xml);
        self.data_services
            .message_device
            .load_fonts(&self.scan.resources);
        let old = self.data_services.messages.defaults.as_ref();
        let mut main = old.map_or_else(Vec::new, |defaults| defaults.font_name.clone());
        let mut ruby = self.data_services.message_device.ruby_font_name.clone();
        let mut controls = self
            .data_services
            .message_device
            .raw_font_controls()
            .unwrap_or([1, 1, 0, 1, 0]);
        apply_font_xml(
            xml,
            &self.data_services.message_device,
            &mut main,
            &mut ruby,
            &mut controls,
        )?;
        self.data_services.messages.defaults = Some(MessageDefaults {
            font_name: main,
            bold: controls[2] != 0,
            outline: controls[0],
            shadow: controls[1],
            glyph_parameter: Some(if controls[3] == 0 { 1 } else { controls[4] }),
            gradual: old.is_some_and(|defaults| defaults.gradual),
        });
        self.data_services.message_device.ruby_font_name = ruby;
        self.data_services
            .message_device
            .set_raw_font_controls(controls);
        Ok(())
    }
}

fn apply_font_xml(
    xml: &ConfigXml,
    device: &DesktopMessageDevice,
    main: &mut Vec<u16>,
    ruby: &mut Vec<u16>,
    controls: &mut [u32; 5],
) -> Result<()> {
    if let Some(name) = configured_font(xml, "face", device)? {
        *main = name;
    }
    if let Some(name) = configured_font(xml, "faceruby", device)? {
        *ruby = name;
    }
    for (index, path) in [
        (0, "document/FONT/shadow/around"),
        (1, "document/FONT/shadow/rightdown"),
        (2, "document/FONT/bold"),
        (3, "document/FONT/proportional"),
    ] {
        if let Some(value) = super::xml_integer(xml, path) {
            controls[index] = value;
        }
    }
    controls[4] = super::xml_integer(xml, "document/FONT/fixed").unwrap_or(controls[3]);
    Ok(())
}

fn configured_font(
    xml: &ConfigXml,
    field: &str,
    device: &DesktopMessageDevice,
) -> Result<Option<Vec<u16>>> {
    let prefix = [b"document".as_slice(), b"FONT".as_slice(), field.as_bytes()];
    let mut candidates = Vec::new();
    let children: Vec<_> = xml
        .fields
        .iter()
        .filter(|entry| {
            entry.path.len() == 4
                && entry
                    .path
                    .iter()
                    .take(3)
                    .map(Vec::as_slice)
                    .eq(prefix.iter().copied())
        })
        .collect();
    if children.is_empty() {
        let path = format!("document/FONT/{field}");
        if let Some(bytes) = super::xml_text(xml, &path) {
            candidates.push(decode_font_name(bytes)?);
        }
    } else {
        // Startup candidate order is f00, f01, ...; select an actually installed
        // face from that declared list. Never substitute an undeclared family.
        let mut children = children;
        children.sort_by(|left, right| left.path[3].cmp(&right.path[3]));
        for entry in children {
            candidates.push(decode_font_name(&entry.text)?);
        }
    }
    if candidates.is_empty() {
        return Ok(None);
    }
    let selected = candidates
        .iter()
        .find(|name| name.is_empty() || device.has_font_name(name))
        .unwrap_or(&candidates[0]);
    let wide: Vec<_> = selected.encode_utf16().collect();
    if wide.len() > 63 {
        return Err(invalid("configured message font exceeds 63 UTF-16 units"));
    }
    Ok(Some(wide))
}

fn decode_font_name(bytes: &[u8]) -> Result<String> {
    let text = if let Ok(text) = std::str::from_utf8(bytes) {
        text.to_owned()
    } else {
        let (text, _, errors) = encoding_rs::SHIFT_JIS.decode(bytes);
        if errors {
            return Err(invalid(
                "message font name is neither UTF-8 nor valid CP932",
            ));
        }
        text.into_owned()
    };
    Ok(text.trim().to_owned())
}

#[cfg(test)]
mod focus_tests {
    use super::*;
    use cs2_runtime::{InputEvent, InputKey};

    #[test]
    fn input_async_wait_requires_an_unfinished_receiver_not_an_unrelated_audio_task() {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let directory = std::env::temp_dir().join(format!("cs2-input-wait-{stamp}"));
        std::fs::create_dir_all(&directory).unwrap();
        let mut player = HeadlessPlayer::open_with_save_root(&directory, directory.join("profile")).unwrap();
        player.sound_channels.insert(6, crate::SoundChannelState {
            pending_start: Some(crate::sound_async::PendingSoundStart {
                started: std::time::Instant::now(), owner: 123, index: 935, value: 0,
                configured: None, transition: None, operation: crate::sound_async::Operation::PrepareLoad,
            }), ..Default::default()
        });
        assert!(!player.input_dispatch_waits_for_audio().unwrap());
        player.data_services.messages.cursor_frame = Some(InputCursorFrame {
            changed: false, position_changed: false, moving: false, original_nonzero: false,
            active: 1, foreground: true, initialized: true,
            broadcast: Some(CursorBroadcast { visible: 1, stack: vec![(123, true, None)], visited: 0 }),
        });
        assert!(!player.input_dispatch_waits_for_audio().unwrap(), "an entered receiver is already complete; retain a budget-only slice");
        player.data_services.messages.cursor_frame.as_mut().unwrap().broadcast.as_mut().unwrap().stack[0].1 = false;
        assert!(!player.input_dispatch_waits_for_audio().unwrap(), "a newly pushed cursor child has not necessarily attempted a callback");
        let mut words = [0; 15];
        words[0] = 123;
        player.data_services.messages.key_selection = Some(InputSelectionDispatch {
            point: [0; 2], candidate: None, selected: None, next: 0,
            current: Some((words, 0, 8)), buffers: Default::default(), output: 0,
            stage: 0, visited: 0, all_routed: false, routed_any: false,
        });
        assert!(player.input_dispatch_waits_for_audio().unwrap());
        player.sound_channels.clear();
        assert!(!player.input_dispatch_waits_for_audio().unwrap());
        drop(player);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn typed_focus_survives_overlapping_legacy_region_and_touch_release() {
        let fixture = std::env::temp_dir().join(format!(
            "cs2-focus-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
                .unwrap().as_nanos(),
        ));
        std::fs::create_dir(&fixture).unwrap();
        struct Fixture(std::path::PathBuf);
        impl Drop for Fixture {
            fn drop(&mut self) { let _ = std::fs::remove_dir(&self.0); }
        }
        let fixture = Fixture(fixture);
        let mut player = HeadlessPlayer::open_with_save_root(&fixture.0, fixture.0.join("profile"))
            .unwrap();
        let owner = player.current_task_id;
        let document = cs2_script::FesDocument {
            sections: vec![cs2_script::FesSection {
                header: b"START".to_vec(),
                lines: vec![cs2_script::FesLine { offset: 0, bytes: b"wait 20".to_vec() }],
            }],
        };
        player.with_host(|host| {
            Ok(host.begin_direct_fes_entry(owner, document, Default::default())?)
        }).unwrap();
        player.run_frames(1, 10_000).unwrap();
        let typed = player.input_task.regions.iter_mut().find(|region| {
            player.scene_nodes[&region.id()].name == "low button"
        }).unwrap();
        let typed_id = typed.id();
        typed.words[3..7].copy_from_slice(&[5, 5, 25, 25]);
        let mut legacy = typed.words;
        legacy[0] = 0x7fff_ffff;
        legacy[1] = i32::MAX as u32;
        legacy[7] = 0;
        legacy[9] = 0;
        player.input_task.regions.insert(0, crate::InputRegionState { words: legacy });
        player.apply_host_input(InputEvent::FocusChanged(true)).unwrap();
        let inside = InputEvent::MouseMove { x: 10, y: 10 };
        assert_eq!(player.input_region_at([10, 10]).unwrap()[0], legacy[0]);
        assert_eq!(player.masked_input_region_at([10, 10]).unwrap().unwrap()[0], typed_id);
        let candidates = player.mouse_press_candidate_regions([10, 10]).unwrap();
        assert_eq!(candidates.first(), Some(&legacy[0]));
        assert!(candidates.contains(&typed_id));
        assert!(!player.mouse_press_candidate_regions([40, 32]).unwrap().contains(&typed_id));
        assert!(player.host_input_route_receipts().is_empty());
        player.apply_host_input(inside.clone()).unwrap();
        player.run_frames(1, 10_000).unwrap();
        assert_eq!(player.input_task.selected_region, Some(typed_id));

        for _ in 0..2 {
            player.run_frames(1, 10_000).unwrap();
            assert_eq!(player.input_task.selected_region, Some(typed_id));
            assert!(player.host_input_selection_routed());
        }
        player.set_host_input_geometry_tracking(true);
        let mut binding = DeviceBinding { enabled: true, ..Default::default() };
        binding.codes[ButtonSource::Mouse.index()][0] = 1;
        player.set_device_key_binding(0, binding).unwrap();
        let button = InputKey::MouseButton(0);
        player.apply_host_input(InputEvent::KeyDown(button.clone())).unwrap();
        player.run_frames(1, 10_000).unwrap();
        assert!(player.state.input.is_pressed(&button));
        assert_eq!(player.device_key_scan_state().records()[0][0].mask, 2);
        player.apply_host_input(inside.clone()).unwrap();
        player.apply_host_input(InputEvent::KeyUp(button.clone())).unwrap();
        player.run_frames(1, 10_000).unwrap();
        assert!(!player.state.input.is_pressed(&button));
        assert_eq!(player.input_task.selected_region, Some(typed_id));
        player.run_frames(1, 10_000).unwrap();
        assert_eq!(player.device_key_scan_state().records()[0][0].mask, 0);
        assert!(player.host_input_route_receipts().iter().any(|receipt| {
            receipt.region == typed_id && receipt.pressed == 0
                && receipt.physical.iter().any(|button| button.source == ButtonSource::Mouse)
        }));
        assert_eq!(player.input_task.selected_region, Some(typed_id));

        player.apply_host_input(InputEvent::MouseMove { x: 40, y: 32 }).unwrap();
        player.run_frames(1, 10_000).unwrap();
        assert_eq!(player.input_task.selected_region, None);
        player.apply_host_input(inside).unwrap();
        player.run_frames(1, 10_000).unwrap();
        assert_eq!(player.input_task.selected_region, Some(typed_id));
        player.input_task.regions.retain(|region| region.id() != typed_id);
        player.run_frames(1, 10_000).unwrap();
        assert!(!player.host_input_selection_routed());
        assert_eq!(player.input_task.selected_region, Some(legacy[0]));
    }
}

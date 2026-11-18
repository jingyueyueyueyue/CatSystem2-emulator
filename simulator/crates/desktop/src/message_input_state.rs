//! Input caches captured at a frozen frame boundary. This component preserves
//! sampled data and never polls an OS device while preparing a restoration.
use std::collections::BTreeMap;

use cs2_runtime::message_object::{DeviceKeys, Keys, WheelButtons};
use cs2_runtime::message_object::input_snapshot::{
    InputCoordinateMapSnapshot, InputSamplingStateSnapshot, InputTaskSamplingSnapshot,
};
use cs2_script::Result;

use crate::{DesktopInputTask, HeadlessPlayer, InputRegionState, native_state_error};

pub(crate) struct PreparedInputSamplingState {
    saved: InputSamplingStateSnapshot,
    task: DesktopInputTask,
    device_keys: DeviceKeys,
    key_scan: Keys,
    wheel_buttons: WheelButtons,
}

impl HeadlessPlayer {
    /// Per-button runtime, FES controllers, message surfaces, clock, queued
    /// task/FES packets and callback adapters are other graph components.
    pub fn input_sampling_state_snapshot(&self) -> Result<InputSamplingStateSnapshot> {
        self.check_input_sampling_restore_boundary()?;
        self.capture_input_sampling_for_graph()
    }

    pub(crate) fn capture_input_sampling_for_graph(&self) -> Result<InputSamplingStateSnapshot> {
        self.check_input_callbacks_for_graph()?;
        let messages = &self.data_services.messages;
        let saved = InputSamplingStateSnapshot {
            version: 2,
            frame: self.state.clock.frame,
            coordinates: self.data_services.host_input_coordinates.map(InputCoordinateMapSnapshot::capture),
            host_input: self.state.input.clone(),
            pending_host_input: self.data_services.pending_host_input.iter().cloned().collect(),
            task: InputTaskSamplingSnapshot {
                owner_id: self.input_task.owner_id,
                alive: self.input_task.alive,
                regions: self.input_task.regions.iter().map(|region| region.words).collect(),
                key_masks: self.input_task.key_masks.to_vec(),
                key_enabled: self.input_task.key_enabled.to_vec(),
                pointer_position: self.input_task.pointer_position,
                cursor_requested: self.input_task.cursor_requested,
                auto_hide: self.input_task.auto_hide,
                idle_limit: self.input_task.idle_limit,
                wheel_repeat: self.input_task.wheel_repeat,
                selected_region: self.input_task.selected_region,
            },
            text_input: messages.text_input.clone(),
            device_keys: messages.device_keys.input_sampling_snapshot(),
            device_keys_loaded: messages.device_keys_loaded,
            physical_buttons: messages.physical_buttons.clone(),
            ime_device_keys: messages.ime_device_keys.clone(),
            ime_device_pulses: messages.ime_device_pulses.clone(),
            pending_device_buttons: messages.pending_device_buttons.iter().copied().collect(),
            key_scan: messages.key_scan.input_sampling_snapshot()?,
            wheel_buttons: messages.wheel_buttons.input_sampling_snapshot(),
            wheel_observed: messages.wheel_observed,
            key_scan_frame: messages.key_scan_frame,
            key_sample_physical: messages.key_sample_physical.clone(),
            key_selection_done: messages.key_selection_done,
            input_geometry: messages.input_geometry.clone(),
            input_geometry_tracking_complete: messages.input_geometry_tracking_complete,
            input_selection_previous_device: messages.input_selection_previous_device,
            input_selection_forced: messages.input_selection_forced,
            pointer_tail_done: messages.pointer_tail_done,
            pointer_origin: messages.pointer_origin,
            pointer_motion_frame: messages.pointer_motion_frame,
            input_selection_frame: messages.input_selection_frame,
            input_event_mask: messages.input_event_mask,
            input_suppressed: messages.input_suppressed,
            key_capture: messages.key_capture.clone(),
            pointer_move: messages.pointer_move,
            device_pointer: messages.device_pointer,
            device_logical_pointer: messages.device_logical_pointer,
            device_task_pointer: messages.device_task_pointer,
            device_task_position: messages.device_task_position,
            device_pointer_moving: messages.device_pointer_moving,
            pointer_saved: messages.pointer_saved,
            pointer_request: messages.pointer_request,
            device_foreground: messages.device_foreground,
            device_sample_active: messages.device_sample_active,
            pending_device_active: messages.pending_device_active,
            device_history: messages.device_history.clone(),
            device_cursor: messages.device_cursor.clone(),
            pending_device_history: messages.pending_device_history,
            frame_device_history: messages.frame_device_history,
            input_frame_complete: messages.input_frame_complete,
            cursor_frame_done: messages.cursor_frame_done,
            device_pointer_observed: messages.device_pointer_observed,
            input_masks: messages.input_masks.clone(),
            button_event_completions: messages.button_event_completions.clone(),
            physical_edges: messages.physical_edges.clone(),
            input_route_receipts: messages.input_route_receipts.clone(),
            typed_physical_owners: messages.typed_physical_owners.clone(),
            cursor_visible: self.data_services.message_device.device_cursor_visible() != 0,
        };
        saved.validate()?;
        validate_input_task_owner(&saved, |id| self.data_services.tasks.message_task_state(id)
            .map(|(kind, _, _)| kind))?;
        Ok(saved)
    }

    /// A complete graph loader supplies its prepared clock and actual task
    /// kinds. This method does not mutate live services or allocate identities.
    pub(crate) fn prepare_input_sampling_state_restore(
        saved: &InputSamplingStateSnapshot,
        rebuilt_frame: u64,
        rebuilt_task_kinds: &BTreeMap<u32, u32>,
    ) -> Result<PreparedInputSamplingState> {
        saved.validate()?;
        if saved.frame != rebuilt_frame {
            return Err(native_state_error("input sampling snapshot frame differs from the rebuilt clock"));
        }
        validate_input_task_owner(saved, |id| rebuilt_task_kinds.get(&id).copied())?;
        let task = DesktopInputTask {
            owner_id: saved.task.owner_id,
            alive: saved.task.alive,
            regions: saved.task.regions.iter().copied().map(|words| InputRegionState { words }).collect(),
            key_masks: std::array::from_fn(|key| saved.task.key_masks[key]),
            key_enabled: std::array::from_fn(|key| saved.task.key_enabled[key]),
            pointer_position: saved.task.pointer_position,
            cursor_requested: saved.task.cursor_requested,
            auto_hide: saved.task.auto_hide,
            idle_limit: saved.task.idle_limit,
            wheel_repeat: saved.task.wheel_repeat,
            selected_region: saved.task.selected_region,
        };
        Ok(PreparedInputSamplingState {
            task,
            device_keys: saved.device_keys.restore()?,
            key_scan: saved.key_scan.restore()?,
            wheel_buttons: saved.wheel_buttons.restore()?,
            saved: saved.clone(),
        })
    }

    /// Check this before committing any graph component. Mid-frame input
    /// continuations need their own complete callback-stack snapshot schema.
    pub(crate) fn check_input_sampling_restore_boundary(&self) -> Result<()> {
        if self.frame_in_progress() || self.data_services.tasks.root_dispatch.is_some() {
            return Err(native_state_error("finish the frame before restoring standalone input state"));
        }
        self.check_input_callbacks_for_graph()
    }

    /// A full graph also restores the exact frame/task cursors. It may freeze
    /// a yielded VM, but cannot discard an unsaved input callback stack.
    pub(crate) fn check_input_callbacks_for_graph(&self) -> Result<()> {
        let messages = &self.data_services.messages;
        let tasks = &self.data_services.tasks;
        if tasks.records.values().any(|record| record.active)
            || messages.key_dispatch.is_some() || messages.key_selection.is_some()
            || messages.pointer_dispatch.is_some() || messages.cursor_frame.is_some()
            || !messages.cursor_commands.is_empty() || !messages.pointer_natives.is_empty()
            || !messages.pointer_queries.is_empty() || !messages.input_natives.is_empty()
            || !messages.sound_prepare_natives.is_empty() || !messages.button_dispatches.is_empty()
            || !messages.button_active.is_empty() || !messages.button_contexts.is_empty()
            || !messages.fes_button_owners.is_empty() || !messages.fes_button_dispatches.is_empty()
            || messages.input_key_context.is_some()
        {
            return Err(native_state_error("finish the frame and active input callbacks before restoring sampling state"));
        }
        Ok(())
    }

    /// Install with the prepared task graph and clock. Callback hooks and
    /// per-button objects are retained for their respective graph components.
    pub(crate) fn commit_input_sampling_state_restore(
        &mut self, prepared: PreparedInputSamplingState,
    ) -> Result<()> {
        self.check_input_sampling_restore_boundary()?;
        if self.state.clock.frame != prepared.saved.frame {
            return Err(native_state_error("commit input sampling state with its matching clock"));
        }
        validate_input_task_owner(&prepared.saved, |id| self.data_services.tasks.message_task_state(id)
            .map(|(kind, _, _)| kind))?;
        self.install_prepared_input_for_graph(prepared);
        Ok(())
    }

    /// Caller validated the complete replacement graph before its first write.
    pub(crate) fn install_prepared_input_for_graph(&mut self, prepared: PreparedInputSamplingState) {
        let PreparedInputSamplingState { saved, task, device_keys, key_scan, wheel_buttons } = prepared;
        self.input_task = task;
        self.state.input = saved.host_input;
        self.data_services.host_input_coordinates = saved.coordinates.map(InputCoordinateMapSnapshot::restore);
        self.data_services.pending_host_input = saved.pending_host_input.into();
        let messages = &mut self.data_services.messages;
        messages.text_input = saved.text_input;
        messages.device_keys = device_keys;
        messages.device_keys_loaded = saved.device_keys_loaded;
        messages.physical_buttons = saved.physical_buttons;
        messages.ime_device_keys = saved.ime_device_keys;
        messages.ime_device_pulses = saved.ime_device_pulses;
        messages.pending_device_buttons = saved.pending_device_buttons.into();
        messages.key_scan = key_scan;
        messages.wheel_buttons = wheel_buttons;
        messages.wheel_observed = saved.wheel_observed;
        messages.key_scan_frame = saved.key_scan_frame;
        messages.key_sample_physical = saved.key_sample_physical;
        messages.key_selection_done = saved.key_selection_done;
        messages.input_geometry = saved.input_geometry;
        messages.input_geometry_tracking_complete = saved.input_geometry_tracking_complete;
        messages.input_selection_previous_device = saved.input_selection_previous_device;
        messages.input_selection_forced = saved.input_selection_forced;
        messages.pointer_tail_done = saved.pointer_tail_done;
        messages.pointer_origin = saved.pointer_origin;
        messages.pointer_motion_frame = saved.pointer_motion_frame;
        messages.input_selection_frame = saved.input_selection_frame;
        messages.input_event_mask = saved.input_event_mask;
        messages.input_suppressed = saved.input_suppressed;
        messages.key_capture = saved.key_capture;
        messages.pointer_move = saved.pointer_move;
        messages.device_pointer = saved.device_pointer;
        messages.device_logical_pointer = saved.device_logical_pointer;
        messages.device_task_pointer = saved.device_task_pointer;
        messages.device_task_position = saved.device_task_position;
        messages.device_pointer_moving = saved.device_pointer_moving;
        messages.pointer_saved = saved.pointer_saved;
        messages.pointer_request = saved.pointer_request;
        messages.device_foreground = saved.device_foreground;
        messages.device_sample_active = saved.device_sample_active;
        messages.pending_device_active = saved.pending_device_active;
        messages.device_history = saved.device_history;
        messages.device_cursor = saved.device_cursor;
        messages.pending_device_history = saved.pending_device_history;
        messages.frame_device_history = saved.frame_device_history;
        messages.input_frame_complete = saved.input_frame_complete;
        messages.cursor_frame_done = saved.cursor_frame_done;
        messages.device_pointer_observed = saved.device_pointer_observed;
        messages.input_masks = saved.input_masks;
        messages.button_event_completions = saved.button_event_completions;
        messages.physical_edges = saved.physical_edges;
        messages.input_route_receipts = saved.input_route_receipts;
        messages.typed_physical_owners = saved.typed_physical_owners;
        // Reconcile the real presenter once after restore; polling a fresh
        // device here would discard the saved history/short-tap state.
        self.data_services.message_device.show_device_cursor(u32::from(saved.cursor_visible));
    }
}

fn validate_input_task_owner(
    saved: &InputSamplingStateSnapshot, mut task_kind: impl FnMut(u32) -> Option<u32>,
) -> Result<()> {
    if saved.task.owner_id.is_some_and(|id| task_kind(id) != Some(11)) {
        return Err(native_state_error("input sampling owner is not an actual rebuilt kind11 task"));
    }
    Ok(())
}

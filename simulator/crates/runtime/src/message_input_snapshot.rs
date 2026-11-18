//! Portable input sampling state. This component is captured between frames;
//! task/FES callback stacks and per-button objects belong to the host graph.
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::{
    ButtonSource, DeviceBinding, DeviceKeys, InputCursorState, InputDeviceHistory,
    InputGeometryEpoch, InputHitMask, InputRouteReceipt, KeyState, Keys,
    MessageTextInputState, PointerMoveState, Result, WheelButtons, invalid,
};
use crate::{InputEvent, InputState, input_coordinates::InputCoordinateMap};

const MAX_ITEMS: usize = 1_000_000;
const MAX_BYTES: usize = 64 * 1024 * 1024;
const PHYSICAL_MASK: u32 = 1 | 2 | 4 | 16;

/// Store exact single-precision values, including signed zero. Host window
/// layout must be explicitly rebound after restore if its device changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputCoordinateMapSnapshot {
    pub device_offset_bits: [u32; 2],
    pub device_scale_bits: u32,
    pub configuration_offset: [i32; 2],
    pub logical_scale_bits: [u32; 2],
    pub device_units_per_logical_bits: [u32; 2],
    pub logical_size: [i32; 2],
}

impl InputCoordinateMapSnapshot {
    pub fn capture(map: InputCoordinateMap) -> Self {
        Self {
            device_offset_bits: map.device_offset.map(f32::to_bits),
            device_scale_bits: map.device_scale.to_bits(),
            configuration_offset: map.configuration_offset,
            logical_scale_bits: map.logical_scale.map(f32::to_bits),
            device_units_per_logical_bits: map.device_units_per_logical.map(f32::to_bits),
            logical_size: map.logical_size,
        }
    }

    pub fn restore(self) -> InputCoordinateMap {
        InputCoordinateMap {
            device_offset: self.device_offset_bits.map(f32::from_bits),
            device_scale: f32::from_bits(self.device_scale_bits),
            configuration_offset: self.configuration_offset,
            logical_scale: self.logical_scale_bits.map(f32::from_bits),
            device_units_per_logical: self.device_units_per_logical_bits.map(f32::from_bits),
            logical_size: self.logical_size,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceBindingSnapshot {
    pub enabled: bool,
    pub codes: [[u8; 4]; 4],
    pub repeat: [[u32; 4]; 4],
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceKeysSnapshot {
    pub bindings: Vec<DeviceBindingSnapshot>,
    pub current: Vec<[u32; 4]>,
    pub held: Vec<[u32; 4]>,
    pub repeat: Vec<u32>,
    pub forced: Vec<u32>,
    pub debug_mode: u32,
}

impl DeviceKeys {
    pub fn input_sampling_snapshot(&self) -> DeviceKeysSnapshot {
        DeviceKeysSnapshot {
            bindings: self.bindings.iter().map(|binding| DeviceBindingSnapshot {
                enabled: binding.enabled, codes: binding.codes, repeat: binding.repeat,
            }).collect(),
            current: self.current.to_vec(), held: self.held.to_vec(),
            repeat: self.repeat.to_vec(), forced: self.forced.to_vec(),
            debug_mode: self.debug_mode,
        }
    }
}

impl DeviceKeysSnapshot {
    pub fn restore(&self) -> Result<DeviceKeys> {
        if [self.bindings.len(), self.current.len(), self.held.len(),
            self.repeat.len(), self.forced.len()].iter().any(|&n| n != 80)
            || self.current.iter().flatten().chain(self.held.iter().flatten())
                .any(|mask| mask & !PHYSICAL_MASK != 0)
            || self.current.iter().zip(&self.held).any(|(current, held)|
                current.iter().zip(held).any(|(current, held)| held & !current != 0))
        {
            return Err(invalid("device key snapshot has invalid dimensions or physical masks"));
        }
        let mut restored = DeviceKeys::default();
        for key in 0..80 {
            let binding = &self.bindings[key];
            restored.bindings[key] = DeviceBinding {
                enabled: binding.enabled, codes: binding.codes, repeat: binding.repeat,
            };
            restored.current[key] = self.current[key];
            restored.held[key] = self.held[key];
            restored.repeat[key] = self.repeat[key];
            restored.forced[key] = self.forced[key];
        }
        restored.debug_mode = self.debug_mode;
        Ok(restored)
    }
}

/// Complete scan state at a frame boundary. A callback or an unfinished scan
/// is rejected instead of silently resetting its cursor and consumption bits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeysSnapshot {
    pub records: Vec<[(u32, u32, bool); 4]>,
    pub enabled: Vec<u32>,
    pub cursor: usize,
    /// Result of the completed scan; true means a callback consumed input.
    pub consumed: bool,
}

impl Keys {
    pub fn input_sampling_snapshot(&self) -> Result<KeysSnapshot> {
        let Some(consumed) = self.complete else {
            return Err(invalid("finish the input scan before capturing frame-boundary sampling state"));
        };
        if self.pending.is_some() {
            return Err(invalid("finish the input key callback before capturing sampling state"));
        }
        Ok(KeysSnapshot {
            records: self.records.iter().map(|row| row.map(|state|
                (state.count, state.mask, state.consumed))).collect(),
            enabled: self.enabled.to_vec(), cursor: self.cursor, consumed,
        })
    }
}

impl KeysSnapshot {
    pub fn restore(&self) -> Result<Keys> {
        if self.records.len() != 80 || self.enabled.len() != 80 || self.cursor > 320
            || !self.consumed && self.cursor != 320
            || self.records.iter().flatten().any(|&(_, mask, _)| mask & !PHYSICAL_MASK != 0)
        {
            return Err(invalid("input scan snapshot has invalid dimensions, masks or completion cursor"));
        }
        let mut restored = Keys::default();
        for key in 0..80 {
            restored.records[key] = self.records[key].map(|(count, mask, consumed)|
                KeyState { count, mask, consumed });
            restored.enabled[key] = self.enabled[key];
        }
        restored.cursor = self.cursor;
        restored.complete = Some(self.consumed);
        Ok(restored)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WheelButtonsSnapshot {
    pub direction: u32,
    pub repeats: u32,
    pub pulses: [bool; 2],
}

impl WheelButtons {
    pub fn input_sampling_snapshot(&self) -> WheelButtonsSnapshot {
        WheelButtonsSnapshot {
            direction: self.direction, repeats: self.repeats, pulses: self.pulses,
        }
    }
}

impl WheelButtonsSnapshot {
    pub fn restore(&self) -> Result<WheelButtons> {
        if !matches!(self.direction, 0 | 6 | 7) {
            return Err(invalid("wheel sampling snapshot has an invalid repeat direction"));
        }
        Ok(WheelButtons {
            direction: self.direction, repeats: self.repeats, pulses: self.pulses,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputTaskSamplingSnapshot {
    pub owner_id: Option<u32>,
    pub alive: bool,
    /// Declaration/priority order is significant. It is never sorted by ID.
    pub regions: Vec<[u32; 15]>,
    pub key_masks: Vec<[u32; 4]>,
    pub key_enabled: Vec<bool>,
    pub pointer_position: [i32; 2],
    pub cursor_requested: u32,
    pub auto_hide: u32,
    pub idle_limit: u32,
    pub wheel_repeat: u32,
    pub selected_region: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InputSamplingStateSnapshot {
    pub version: u32,
    pub frame: u64,
    pub coordinates: Option<InputCoordinateMapSnapshot>,
    pub host_input: InputState,
    pub pending_host_input: Vec<InputEvent>,
    pub task: InputTaskSamplingSnapshot,
    pub text_input: MessageTextInputState,
    pub device_keys: DeviceKeysSnapshot,
    pub device_keys_loaded: bool,
    pub physical_buttons: BTreeSet<(ButtonSource, u8)>,
    pub ime_device_keys: BTreeSet<u8>,
    pub ime_device_pulses: BTreeSet<u8>,
    pub pending_device_buttons: Vec<(ButtonSource, u32, u32)>,
    pub key_scan: KeysSnapshot,
    pub wheel_buttons: WheelButtonsSnapshot,
    pub wheel_observed: usize,
    pub key_scan_frame: Option<u64>,
    pub key_sample_physical: BTreeSet<(u32, u32)>,
    pub key_selection_done: bool,
    pub input_geometry: InputGeometryEpoch,
    pub input_geometry_tracking_complete: bool,
    pub input_selection_previous_device: Option<[i32; 2]>,
    pub input_selection_forced: bool,
    pub pointer_tail_done: bool,
    pub pointer_origin: Option<[i32; 2]>,
    pub pointer_motion_frame: Option<u64>,
    pub input_selection_frame: Option<u64>,
    pub input_event_mask: u32,
    pub input_suppressed: u32,
    pub key_capture: BTreeMap<u32, u32>,
    pub pointer_move: PointerMoveState,
    pub device_pointer: [i32; 2],
    pub device_logical_pointer: [i32; 2],
    pub device_task_pointer: [i32; 2],
    pub device_task_position: [i32; 2],
    pub device_pointer_moving: bool,
    pub pointer_saved: Option<[i32; 2]>,
    pub pointer_request: Option<[i32; 2]>,
    pub device_foreground: bool,
    pub device_sample_active: u32,
    pub pending_device_active: Option<u32>,
    pub device_history: InputDeviceHistory,
    pub device_cursor: InputCursorState,
    pub pending_device_history: Option<[u8; 32]>,
    pub frame_device_history: Option<[u8; 32]>,
    pub input_frame_complete: bool,
    pub cursor_frame_done: bool,
    pub device_pointer_observed: [i32; 2],
    pub input_masks: BTreeMap<u32, InputHitMask>,
    pub button_event_completions: BTreeSet<(u32, u32)>,
    pub physical_edges: BTreeSet<(ButtonSource, u8, u32)>,
    pub input_route_receipts: Vec<InputRouteReceipt>,
    pub typed_physical_owners: BTreeMap<(ButtonSource, u8), (u32, u32, u32)>,
    /// Actual device presentation state can differ from cursor-requested.
    pub cursor_visible: bool,
}

impl InputSamplingStateSnapshot {
    pub fn validate(&self) -> Result<()> {
        self.device_keys.restore()?;
        self.key_scan.restore()?;
        self.wheel_buttons.restore()?;
        if self.version != 2 || self.task.key_masks.len() != 80
            || self.task.key_enabled.len() != 80 || self.pending_device_buttons.len() > 4096
            || self.key_sample_physical.len() > 320 || self.key_capture.len() > 80
            || self.typed_physical_owners.len() > 1024 || self.physical_buttons.len() > 1024
            || self.physical_edges.len() > 2048 || self.input_route_receipts.len() > 65_536
            || self.task.regions.len() > MAX_ITEMS || self.input_masks.len() > MAX_ITEMS
            || self.pending_host_input.len() > 4096
            || self.button_event_completions.len() > MAX_ITEMS
            || self.host_input.pressed.len() > MAX_ITEMS
            || self.host_input.just_pressed.len() > MAX_ITEMS
            || self.host_input.just_released.len() > MAX_ITEMS
            || self.host_input.wheel_events.len() > MAX_ITEMS
            || self.host_input.text_events.len() > MAX_ITEMS || self.wheel_observed > MAX_ITEMS
        {
            return Err(invalid("input sampling snapshot has invalid version or collection sizes"));
        }
        if self.key_scan_frame.is_some_and(|frame| frame > self.frame)
            || self.pointer_motion_frame.is_some_and(|frame| frame > self.frame)
            || self.input_selection_frame.is_some_and(|frame| frame > self.frame)
            || self.key_sample_physical.iter().any(|&(key, group)| key >= 80 || group >= 4)
            || self.key_capture.keys().any(|&key| key >= 80)
            || self.typed_physical_owners.values().any(|&(key, group, _)| key >= 80 || group >= 4)
            || self.physical_edges.iter().any(|&(_, _, pressed)| pressed > 1)
            || self.task.key_masks.iter().flatten().any(|mask| mask & !PHYSICAL_MASK != 0)
        {
            return Err(invalid("input sampling snapshot has invalid frame, key or physical state"));
        }
        // Registration appends even duplicate IDs. Region removal does not
        // clear selection/capture fields; retain those stale scalar values.
        let fixed_bytes = self.task.regions.len().checked_mul(60)
            .and_then(|length| length.checked_add(self.host_input.wheel_events.len().checked_mul(4)?))
            .and_then(|length| length.checked_add(self.input_route_receipts.len().checked_mul(64)?))
            .and_then(|length| length.checked_add(self.button_event_completions.len().checked_mul(8)?))
            .and_then(|length| length.checked_add(self.host_input.pressed.len().checked_mul(16)?))
            .and_then(|length| length.checked_add(self.host_input.just_pressed.len().checked_mul(16)?))
            .and_then(|length| length.checked_add(self.host_input.just_released.len().checked_mul(16)?))
            .and_then(|length| length.checked_add(self.host_input.text_events.len().checked_mul(24)?))
            .and_then(|length| length.checked_add(self.input_masks.len().checked_mul(32)?))
            .filter(|&length| length <= MAX_BYTES)
            .ok_or_else(|| invalid("input sampling collections exceed 64 MiB"))?;
        let mut bytes = self.host_input.text_events.iter().try_fold(fixed_bytes, |sum, text|
            sum.checked_add(text.len()).filter(|&length| length <= MAX_BYTES))
            .ok_or_else(|| invalid("input sampling text cache exceeds 64 MiB"))?;
        for event in &self.pending_host_input {
            if let InputEvent::Text(text) = event {
                bytes = bytes.checked_add(text.len()).filter(|&length| length <= MAX_BYTES)
                    .ok_or_else(|| invalid("input sampling pending text exceeds 64 MiB"))?;
            }
        }
        for mask in self.input_masks.values() {
            mask.validate()?;
            bytes = bytes.checked_add(mask.bytes().len()).filter(|&length| length <= MAX_BYTES)
                .ok_or_else(|| invalid("input sampling hit maps exceed 64 MiB"))?;
        }
        let mut physical_associations = 0usize;
        for receipt in &self.input_route_receipts {
            if receipt.frame > self.frame || receipt.key >= 80 || receipt.group >= 4
                || receipt.pressed > 1 || receipt.physical.len() > 1024
            {
                return Err(invalid("input sampling receipt has invalid frame, key or physical associations"));
            }
            physical_associations = physical_associations.checked_add(receipt.physical.len())
                .filter(|&count| count <= MAX_ITEMS)
                .ok_or_else(|| invalid("input sampling receipt associations exceed one million"))?;
            bytes = bytes.checked_add(receipt.physical.len() * 2)
                .filter(|&length| length <= MAX_BYTES)
                .ok_or_else(|| invalid("input sampling receipts exceed 64 MiB"))?;
        }
        validate_text_input(&self.text_input)
    }
}

fn validate_text_input(state: &MessageTextInputState) -> Result<()> {
    if state.preedit.len() > 1024 * 1024 || state.preedit.contains('\0')
        || state.composition_keys.len() > 4096
        || state.composition_keys.iter().copied().collect::<BTreeSet<_>>().len()
            != state.composition_keys.len()
    {
        return Err(invalid("input sampling IME state exceeds its bounds or repeats held keys"));
    }
    if let Some([start, end]) = state.selection_utf16 {
        let mut boundaries = BTreeSet::from([0u32]);
        let mut length = 0;
        for character in state.preedit.chars() {
            length += character.len_utf16() as u32;
            boundaries.insert(length);
        }
        if start > end || !boundaries.contains(&start) || !boundaries.contains(&end) {
            return Err(invalid("input sampling IME selection splits a UTF-16 character"));
        }
    }
    Ok(())
}

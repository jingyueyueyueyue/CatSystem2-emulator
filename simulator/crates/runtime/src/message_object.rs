//! Portable ordinary message state, independently written from docs/message.md.
//! No reference-engine code, device font, host pointer, or renderer dependency.
//!
//! The backend is deliberately required. A host must supply actual layout and
//! pixel work; a made-up font advance or a successful empty painter is not a
//! substitute. Native routing belongs to the desktop adapter.

use std::collections::BTreeMap;
use std::ops::Range;

use cs2_script::{Result, ScriptError};
use serde::{Deserialize, Serialize};

#[path = "kx.rs"]
pub mod kx;

#[path = "message_input_snapshot.rs"]
pub mod input_snapshot;

pub const MESSAGE_OBJECT_TYPE: u32 = 48;
pub const MESSAGE_CONFIGURATION_SIZE: usize = 0xe8;
pub const MESSAGE_UNPREPARED: u32 = u32::MAX;
const MAX_BYTES: usize = 64 * 1024 * 1024;
const MAX_ENTRIES: usize = 1_000_000;

fn invalid(message: &'static str) -> ScriptError {
    ScriptError::InvalidStructuredText { offset: 0, message }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MessageEncoding {
    Cp932,
    Unicode,
}

/// Device sources are distinct from binding groups. Touch uses bit sixteen;
/// the input event layer may translate that bit to its mouse event category.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ButtonSource {
    Keyboard,
    Mouse,
    Joypad,
    Touch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RoutedPhysicalButton {
    pub source: ButtonSource,
    pub code: u8,
}

/// One completed typed callback, before legacy input fallbacks are considered.
/// Device binding and observed physical sources establish the association.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputRouteReceipt {
    pub frame: u64,
    pub key: u32,
    pub group: u32,
    pub raw_mask: u32,
    pub pressed: u32,
    pub repeated: bool,
    pub region: u32,
    pub receiver: u32,
    pub response: u32,
    pub physical: Vec<RoutedPhysicalButton>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputFallbackDisposition {
    Unrouted,
    TypedComplete,
    /// A short-tap release can precede the following scan's logical release.
    /// The completed typed press owns that release; do not synthesize a
    /// second release through the legacy FES queue.
    TypedOwnedRelease,
}

impl ButtonSource {
    pub fn index(self) -> usize {
        match self {
            Self::Keyboard => 0,
            Self::Mouse => 1,
            Self::Joypad => 2,
            Self::Touch => 3,
        }
    }
    pub fn mask(self) -> u32 {
        match self {
            Self::Keyboard => 1,
            Self::Mouse => 2,
            Self::Joypad => 4,
            Self::Touch => 16,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DeviceBinding {
    pub enabled: bool,
    /// [Keyboard/Mouse/Joypad/Touch][first/second/third/fourth binding].
    pub codes: [[u8; 4]; 4],
    pub repeat: [[u32; 4]; 4],
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DeviceKeySample {
    pub mask: u32,
    pub repeat: u32,
}

/// Independent physical-to-logical button model. Short presses remain in the
/// current sample even after release, until the input frame commits.
#[derive(Clone, Debug)]
pub struct DeviceKeys {
    bindings: [DeviceBinding; 80],
    current: [[u32; 4]; 80],
    held: [[u32; 4]; 80],
    repeat: [u32; 80],
    forced: [u32; 80],
    debug_mode: u32,
}

impl Default for DeviceKeys {
    fn default() -> Self {
        Self::new([DeviceBinding::default(); 80])
    }
}

impl DeviceKeys {
    pub fn new(bindings: [DeviceBinding; 80]) -> Self {
        Self {
            bindings,
            current: [[0; 4]; 80],
            held: [[0; 4]; 80],
            repeat: [0; 80],
            forced: [0; 80],
            debug_mode: 0,
        }
    }

    pub fn from_xml(bytes: &[u8]) -> Result<Self> {
        let xml = cs2_formats::ConfigXml::parse_runtime(bytes)
            .map_err(|_| invalid("device key startup XML is malformed"))?;
        Ok(Self::from_configuration(&xml))
    }

    pub fn from_configuration(xml: &cs2_formats::ConfigXml) -> Self {
        let mut bindings = [DeviceBinding::default(); 80];
        for (key, binding) in bindings.iter_mut().enumerate() {
            let name = format!("key{key:02}");
            binding.enabled = xml
                .text(&[b"document", b"KEYCUSTOMIZE", name.as_bytes(), b"name"])
                .is_some_and(|name| name.first().is_some_and(|&byte| byte != 0));
            for (source, prefix) in ["keyboard", "mouse", "joypad", "touch"].iter().enumerate() {
                for group in 0..4 {
                    let code = format!("{prefix}{}", group + 1);
                    let repeat = format!("rep_{prefix}{}", group + 1);
                    binding.codes[source][group] = xml.integer(
                        &[
                            b"document",
                            b"KEYCUSTOMIZE",
                            name.as_bytes(),
                            code.as_bytes(),
                        ],
                        0,
                    ) as u8;
                    binding.repeat[source][group] = u32::from(xml.integer(
                        &[
                            b"document",
                            b"KEYCUSTOMIZE",
                            name.as_bytes(),
                            repeat.as_bytes(),
                        ],
                        0,
                    ) as u8);
                }
            }
        }
        Self::new(bindings)
    }

    pub fn bindings(&self) -> &[DeviceBinding; 80] {
        &self.bindings
    }

    pub fn masks(&self) -> &[[u32; 4]; 80] {
        &self.current
    }

    pub fn set_debug_mode(&mut self, value: u32) {
        self.debug_mode = value;
    }

    pub fn set_binding(&mut self, key: u32, binding: DeviceBinding) -> Result<()> {
        let key = Self::key(key)?;
        self.bindings[key] = binding;
        self.current[key] = [0; 4];
        self.held[key] = [0; 4];
        self.repeat[key] = 0;
        self.forced[key] = 0;
        Ok(())
    }

    pub fn apply_user_xml(&mut self, bytes: &[u8]) -> Result<()> {
        let xml = cs2_formats::ConfigXml::parse_runtime(bytes)
            .map_err(|_| invalid("device key user XML is malformed"))?;
        self.apply_user_configuration(&xml);
        Ok(())
    }

    /// User overrides preserve enable flags, touch bindings and sampled state.
    /// This exposes current configuration to a host-owned writer, without
    /// defining a second XML serialization or disk-write service.
    pub fn apply_user_configuration(&mut self, xml: &cs2_formats::ConfigXml) {
        for (key, binding) in self.bindings.iter_mut().enumerate() {
            if !binding.enabled {
                continue;
            }
            for (source, prefix) in ["kb", "ms", "jp"].iter().enumerate() {
                for group in 0..4 {
                    let code = format!("{prefix}{}_{key:02}", group + 1);
                    let repeat = format!("{prefix}{}r_{key:02}", group + 1);
                    binding.codes[source][group] = xml.integer(
                        &[b"document", b"KEY", code.as_bytes()],
                        i32::from(binding.codes[source][group]),
                    ) as u8;
                    binding.repeat[source][group] = u32::from(xml.integer(
                        &[b"document", b"KEY", repeat.as_bytes()],
                        (binding.repeat[source][group] as u8) as i32,
                    ) as u8);
                }
            }
        }
    }

    pub fn button(&mut self, source: ButtonSource, code: u32, pressed: u32) {
        let code = code as u8;
        let mask = source.mask();
        let limit = if self.debug_mode == 0 { 32 } else { 80 };
        for group in 0..4 {
            for key in 0..limit {
                let binding = self.bindings[key];
                if !binding.enabled {
                    continue;
                }
                let matched = match source {
                    ButtonSource::Mouse | ButtonSource::Touch => {
                        if binding.codes[1][group] == code {
                            Some(1)
                        } else if binding.codes[3][group] == code {
                            Some(3)
                        } else {
                            None
                        }
                    }
                    _ => (binding.codes[source.index()][group] == code).then_some(source.index()),
                };
                let Some(binding_source) = matched else {
                    continue;
                };
                self.repeat[key] = binding.repeat[binding_source][group];
                if pressed != 0 {
                    self.current[key][group] |= mask;
                    self.held[key][group] |= mask;
                } else {
                    self.held[key][group] &= !mask;
                }
            }
        }
    }

    pub fn sample(
        &mut self,
        key: u32,
        group: u32,
        repeat_requested: bool,
    ) -> Result<DeviceKeySample> {
        let key = Self::key(key)?;
        let group = usize::try_from(group)
            .ok()
            .filter(|&group| group < 4)
            .ok_or_else(|| invalid("device key binding group must be in 0..3"))?;
        let mask = if self.forced[key] != 0 {
            self.forced[key] -= 1;
            1
        } else {
            self.current[key][group]
        };
        Ok(DeviceKeySample {
            mask,
            repeat: if repeat_requested {
                self.repeat[key]
            } else {
                0
            },
        })
    }

    pub fn force(&mut self, key: u32, queries: u32) -> Result<()> {
        self.forced[Self::key(key)?] = queries;
        Ok(())
    }

    pub fn forced_queries(&self, key: u32) -> Result<u32> {
        Ok(self.forced[Self::key(key)?])
    }

    pub fn commit(&mut self) {
        self.current = self.held;
    }

    pub fn clear_pressed(&mut self) {
        self.current.fill([0; 4]);
        self.held.fill([0; 4]);
    }

    pub fn clear_source(&mut self, source: ButtonSource) {
        let keep = !source.mask();
        for key in 0..80 {
            for group in 0..4 {
                self.current[key][group] &= keep;
                self.held[key][group] &= keep;
            }
        }
    }

    fn key(key: u32) -> Result<usize> {
        usize::try_from(key)
            .ok()
            .filter(|&key| key < 80)
            .ok_or_else(|| invalid("device key slot must be in 0..79"))
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KeyState {
    pub count: u32,
    pub mask: u32,
    pub consumed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyTransitionKind {
    Change,
    Repeat,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyTransition {
    pub key: u32,
    pub group: u32,
    pub pressed: u32,
    /// Event category only: raw touch sixteen becomes mouse category two.
    pub mask: u32,
    pub kind: KeyTransitionKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyScanStep {
    Sample { key: u32, group: u32 },
    Dispatch(KeyTransition),
    Complete { consumed: bool },
}

/// Four groups of eighty independently counted logical keys. Sampling and
/// dispatch are separate steps so a synchronous task callback can yield
/// without advancing a key twice or querying the device again.
#[derive(Clone, Debug)]
pub struct Keys {
    records: [[KeyState; 4]; 80],
    enabled: [u32; 80],
    cursor: usize,
    pending: Option<KeyTransition>,
    complete: Option<bool>,
}

impl Default for Keys {
    fn default() -> Self {
        Self {
            records: [[KeyState::default(); 4]; 80],
            enabled: [1; 80],
            cursor: 320,
            pending: None,
            complete: Some(false),
        }
    }
}

impl Keys {
    pub fn records(&self) -> &[[KeyState; 4]; 80] {
        &self.records
    }

    pub fn enabled(&self) -> &[u32; 80] {
        &self.enabled
    }

    pub fn set_enabled(&mut self, key: u32, value: u32) -> Result<()> {
        self.enabled[DeviceKeys::key(key)?] = value;
        Ok(())
    }

    pub fn enable_all(&mut self) {
        self.enabled.fill(1);
    }

    /// Smooth pointer requests clear only active, enabled scan records.
    /// Capture identities and the current event's raw state live elsewhere.
    pub fn clear_active_enabled(&mut self) {
        for (key, records) in self.records.iter_mut().enumerate() {
            if self.enabled[key] != 0 {
                for state in records {
                    if state.count != 0 {
                        *state = KeyState::default();
                    }
                }
            }
        }
    }

    pub fn begin_scan(&mut self) -> Result<()> {
        if self.complete.is_none() {
            return Err(invalid(
                "key scan must finish its pending device or dispatch step",
            ));
        }
        self.cursor = 0;
        self.pending = None;
        self.complete = None;
        Ok(())
    }

    pub fn step(&mut self) -> KeyScanStep {
        if let Some(consumed) = self.complete {
            return KeyScanStep::Complete { consumed };
        }
        if let Some(transition) = self.pending {
            return KeyScanStep::Dispatch(transition);
        }
        while self.cursor < 320 {
            let key = self.cursor % 80;
            if self.enabled[key] != 0 {
                return KeyScanStep::Sample {
                    key: key as u32,
                    group: (self.cursor / 80) as u32,
                };
            }
            self.cursor += 1;
        }
        self.complete = Some(false);
        KeyScanStep::Complete { consumed: false }
    }

    pub fn sampled(&mut self, sample: DeviceKeySample) -> Result<()> {
        if self.complete.is_some() || self.pending.is_some() || self.cursor >= 320 {
            return Err(invalid("key sample supplied outside its scan query"));
        }
        let key = self.cursor % 80;
        let group = self.cursor / 80;
        if self.enabled[key] == 0 {
            return Err(invalid("key scan query was disabled before sampling"));
        }
        let state = &mut self.records[key][group];
        let mut event = None;
        if sample.mask != 0 {
            if sample.mask == state.mask && sample.mask != 16 {
                state.count = state.count.wrapping_add(1);
            } else {
                state.mask = sample.mask;
                state.count = 1;
            }
            if state.count == 1 {
                event = Some((1, KeyTransitionKind::Change, state.mask));
            } else if state.count == 30 && sample.repeat != 0 {
                state.count = 0x8000_0000;
                event = Some((1, KeyTransitionKind::Repeat, state.mask));
            } else if (state.count as i32) < 0 {
                if state.count & 0x7fff_ffff == 5 {
                    state.count = 0x8000_0000;
                    event = Some((1, KeyTransitionKind::Repeat, state.mask));
                }
            } else if state.count != 0 && state.consumed {
                self.complete = Some(true);
            }
        } else if state.count != 0 {
            state.count = 0;
            state.consumed = false;
            event = Some((0, KeyTransitionKind::Change, state.mask));
        }
        if let Some((pressed, kind, mask)) = event {
            self.pending = Some(KeyTransition {
                key: key as u32,
                group: group as u32,
                pressed,
                mask: if mask == 16 { 2 } else { mask },
                kind,
            });
        } else if self.complete.is_none() {
            self.cursor += 1;
        }
        Ok(())
    }

    /// Commit only after the synchronous receiver has finished. A callback
    /// error or budget yield leaves the exact pending transition available.
    pub fn dispatched(&mut self, consumed: bool) -> Result<()> {
        let transition = self
            .pending
            .take()
            .ok_or_else(|| invalid("key dispatch has no pending transition"))?;
        let state = &mut self.records[transition.key as usize][transition.group as usize];
        state.consumed = consumed;
        if transition.pressed == 0 {
            state.mask = 0;
        }
        if consumed {
            self.complete = Some(true);
        } else {
            self.cursor += 1;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WheelButtonEvent {
    pub code: u32,
    pub pressed: u32,
}

/// Wheel message pulses and the original signed, wrapping repeat counter.
/// The repeat direction is independent from both pending release flags.
#[derive(Clone, Debug, Default)]
pub struct WheelButtons {
    direction: u32,
    repeats: u32,
    pulses: [bool; 2],
}

impl WheelButtons {
    pub fn direction(&self) -> u32 {
        self.direction
    }
    pub fn repeats(&self) -> u32 {
        self.repeats
    }
    pub fn pulses(&self) -> [bool; 2] {
        self.pulses
    }

    pub fn scroll(&mut self, up: bool) -> WheelButtonEvent {
        let code = if up { 6 } else { 7 };
        self.pulses[(code - 6) as usize] = true;
        self.repeats = if self.direction == code {
            self.repeats.wrapping_add(1)
        } else {
            0
        };
        self.direction = code;
        WheelButtonEvent { code, pressed: 1 }
    }

    /// Run after scanning and before DeviceKeys::commit. Release takes an
    /// entire finish turn; only a later turn can replay the same direction.
    pub fn finish(&mut self) -> Vec<WheelButtonEvent> {
        let mut events = Vec::with_capacity(2);
        for index in 0..2 {
            let code = index as u32 + 6;
            if self.pulses[index] {
                self.pulses[index] = false;
                events.push(WheelButtonEvent { code, pressed: 0 });
            } else if self.direction == code {
                let pressed = if self.repeats as i32 > 0 {
                    self.repeats = self.repeats.wrapping_sub(1);
                    (self.repeats as i32 > 0) as u32
                } else {
                    0
                };
                self.pulses[index] = pressed != 0;
                events.push(WheelButtonEvent { code, pressed });
            }
        }
        events
    }

    pub fn cancel_repeat(&mut self) {
        self.direction = 0;
        self.repeats = 0;
    }
}

/// Owned one-bit hit map; source image lifetime and later pixel writes are
/// independent after construction. Rows are byte-aligned, least-significant
/// bit first, with transparent padding at each row's right edge.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputHitMask {
    size: [u32; 2],
    bytes: Vec<u8>,
}

impl InputHitMask {
    pub fn new(size: [u32; 2], bytes: Vec<u8>) -> Result<Self> {
        let length = Self::length(size)?;
        if bytes.len() != length {
            return Err(invalid(
                "input hit map byte length differs from its row-aligned size",
            ));
        }
        Ok(Self { size, bytes })
    }

    fn length(size: [u32; 2]) -> Result<usize> {
        if size.iter().any(|&side| side > i32::MAX as u32) {
            return Err(invalid(
                "input hit map dimensions exceed signed coordinates",
            ));
        }
        (size[0] as usize)
            .div_ceil(8)
            .checked_mul(size[1] as usize)
            .filter(|&length| length <= MAX_BYTES)
            .ok_or_else(|| invalid("input hit map exceeds 64 MiB"))
    }

    pub fn image_size(crop: [u32; 2], canvas: [u32; 2]) -> [u32; 2] {
        std::array::from_fn(|axis| {
            if canvas[axis] == 0 {
                crop[axis]
            } else {
                canvas[axis]
            }
        })
    }

    pub fn from_rgba8(
        crop: [u32; 2],
        canvas: [u32; 2],
        offset: [i32; 2],
        rgba: &[u8],
    ) -> Result<Self> {
        let length = (crop[0] as usize)
            .checked_mul(crop[1] as usize)
            .and_then(|length| length.checked_mul(4))
            .filter(|&length| length <= MAX_BYTES)
            .ok_or_else(|| invalid("input hit map source exceeds 64 MiB"))?;
        if rgba.len() != length {
            return Err(invalid(
                "input hit map source RGBA length differs from its crop",
            ));
        }
        let size = Self::image_size(crop, canvas);
        let mut bytes = vec![0; Self::length(size)?];
        let stride = (size[0] as usize).div_ceil(8);
        for y in 0..crop[1] {
            let dy = i64::from(offset[1]) + i64::from(y);
            if dy < 0 || dy >= i64::from(size[1]) {
                continue;
            }
            for x in 0..crop[0] {
                let dx = i64::from(offset[0]) + i64::from(x);
                if dx < 0 || dx >= i64::from(size[0]) {
                    continue;
                }
                let source = (y as usize * crop[0] as usize + x as usize) * 4;
                if rgba[source + 3] != 0 {
                    bytes[dy as usize * stride + dx as usize / 8] |= 1 << (dx as u32 & 7);
                }
            }
        }
        Self::new(size, bytes)
    }

    pub fn size(&self) -> [u32; 2] {
        self.size
    }
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn validate(&self) -> Result<()> {
        if Self::length(self.size)? != self.bytes.len() {
            return Err(invalid("input hit map snapshot size is invalid"));
        }
        Ok(())
    }

    pub fn contains(&self, origin: [i32; 2], point: [i32; 2]) -> bool {
        let x = point[0].wrapping_sub(origin[0]);
        let y = point[1].wrapping_sub(origin[1]);
        if x < 0 || y < 0 || x as u32 >= self.size[0] || y as u32 >= self.size[1] {
            return false;
        }
        let Some(at) = (y as usize)
            .checked_mul((self.size[0] as usize).div_ceil(8))
            .and_then(|row| row.checked_add(x as usize / 8))
        else {
            return false;
        };
        self.bytes
            .get(at)
            .is_some_and(|byte| byte & (1 << (x as u32 & 7)) != 0)
    }
}

/// Consume geometry changes accepted by one complete input update. Later
/// script mutations need a new epoch, even before the frame renders.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputGeometryEpoch {
    generation: u64,
    consumed: u64,
    pending_consumption: Option<u64>,
}

impl InputGeometryEpoch {
    pub fn mark_changed(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }
    pub fn is_dirty(&self) -> bool {
        self.generation != self.consumed
    }
    pub fn finish_update(&mut self) {
        self.pending_consumption.get_or_insert(self.generation);
    }
    pub fn commit(&mut self) {
        if let Some(completed) = self.pending_consumption.take() {
            self.consumed = completed;
        }
    }
}

/// The initial history cache is documented zero. After an actual update, a
/// missing raw provider is unknown, rather than another all-zero snapshot.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputDeviceHistory {
    committed: Option<[u8; 32]>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputCursorState {
    pub shown: u32,
    pub idle_count: u32,
    pub update_rate: u32,
    pub drag_threshold: u32,
}

impl Default for InputCursorState {
    fn default() -> Self {
        Self {
            shown: 1,
            idle_count: 0,
            update_rate: 0,
            drag_threshold: 2,
        }
    }
}

impl Default for InputDeviceHistory {
    fn default() -> Self {
        Self {
            committed: Some([0; 32]),
        }
    }
}

impl InputDeviceHistory {
    pub fn snapshot(&self) -> Result<&[u8; 32]> {
        self.committed
            .as_ref()
            .ok_or_else(|| invalid("input history requires an actual device snapshot"))
    }
    pub fn complete_frame(&mut self, sample: Option<[u8; 32]>) {
        self.committed = sample;
    }
}

/// Type-six state, separate from drawable plane geometry. None corresponds
/// to the original all-ones receiver sentinel; task zero is a valid receiver.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InputButtonState {
    pub id: u32,
    pub receiver: Option<u32>,
    pub rectangle: [i32; 4],
    pub priority: i32,
    pub parameter: u32,
    pub response: u32,
    pub show: u32,
    pub parent_offset: [f32; 3],
    pub parent_bound: bool,
    pub full_screen: bool,
}

impl InputButtonState {
    pub fn new(id: u32) -> Self {
        Self {
            id,
            receiver: None,
            rectangle: [0; 4],
            priority: 0,
            parameter: 0,
            response: 0,
            show: 1,
            parent_offset: [0.0; 3],
            parent_bound: false,
            full_screen: false,
        }
    }
    pub fn set_receiver(&mut self, value: u32) {
        self.receiver = (value != u32::MAX).then_some(value);
    }
    pub fn begin_update(&mut self) {
        self.response = 0;
    }
    pub fn take_response(&mut self) -> u32 {
        std::mem::take(&mut self.response)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PointerMoveState {
    pub target: [i32; 2],
    pub remaining: i32,
    pub started: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PointerMoveUpdate {
    pub requested: [i32; 2],
    /// The original input body saves next Y into its X slot, retaining the
    /// sampled Y. Selection and device movement still use the full next point.
    pub task_position: [i32; 2],
}

impl PointerMoveState {
    pub fn request(&mut self, target: [i32; 2], frames: u32, elapsed: u32) {
        self.target = target;
        self.remaining = frames as i32;
        self.started = elapsed;
    }

    pub fn advance(&mut self, sampled: [i32; 2], now: u32) -> Option<PointerMoveUpdate> {
        if self.remaining <= 0 {
            return None;
        }
        let next = std::array::from_fn(|axis| {
            sampled[axis]
                .wrapping_add(self.target[axis].wrapping_sub(sampled[axis]) / self.remaining)
        });
        self.remaining = if self.remaining > 1 {
            ((self.remaining as u32)
                .wrapping_add(self.started)
                .wrapping_sub(now) as i32)
                .max(1)
        } else {
            0
        };
        Some(PointerMoveUpdate {
            requested: next,
            task_position: [next[1], sampled[1]],
        })
    }
}

/// A platform substitution records the actual SFNT face that supplied glyphs.
/// It does not rename the face or change the game's requested configuration.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct MessageFontSubstitution {
    pub requested_name: Vec<u16>,
    pub selected_name: String,
    pub source: String,
    pub collection_index: u32,
    pub weight: u16,
    pub italic: bool,
    pub synthesized_bold: bool,
    pub synthesized_italic: bool,
}

/// IME selection offsets are UTF-16 code units, matching mobile text bridges.
/// Preedit remains separate from committed InputEvent::Text payloads.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MessageTextInputEvent {
    ImeEnabled(bool),
    Preedit {
        text: String,
        selection: Option<[u32; 2]>,
    },
    Commit(String),
    HardwareText(String),
    Focus(bool),
    Cancel,
}

impl MessageTextInputEvent {
    /// Window hosts can convert UTF-8 byte cursors without silently clamping
    /// a cursor in the middle of a multibyte character.
    pub fn preedit_utf8(text: String, selection: Option<[usize; 2]>) -> Result<Self> {
        if text.len() > 1024 * 1024 {
            return Err(invalid("IME preedit exceeds one MiB"));
        }
        let selection = selection
            .map(|[start, end]| {
                let convert = |offset: usize| -> Result<u32> {
                    let prefix = text
                        .get(..offset)
                        .ok_or_else(|| invalid("IME UTF-8 cursor is not a character boundary"))?;
                    u32::try_from(prefix.encode_utf16().count())
                        .map_err(|_| invalid("IME UTF-16 cursor exceeds DWORD"))
                };
                Ok::<_, ScriptError>([convert(start)?, convert(end)?])
            })
            .transpose()?;
        Ok(Self::Preedit { text, selection })
    }

    pub fn commit_utf16(units: &[u16]) -> Result<Self> {
        if units.len() > 1024 * 1024 {
            return Err(invalid("IME UTF-16 commit exceeds its storage limit"));
        }
        String::from_utf16(units)
            .map(Self::Commit)
            .map_err(|_| invalid("IME commit contains an isolated UTF-16 surrogate"))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageTextInputState {
    pub ime_enabled: bool,
    pub focused: bool,
    pub preedit: String,
    pub selection_utf16: Option<[u32; 2]>,
    pub commit_count: u64,
    #[serde(default)]
    composition_keys: Vec<u32>,
}

impl Default for MessageTextInputState {
    fn default() -> Self {
        Self {
            ime_enabled: false,
            focused: true,
            preedit: String::new(),
            selection_utf16: None,
            commit_count: 0,
            composition_keys: Vec::new(),
        }
    }
}

impl MessageTextInputState {
    /// A returned String is one committed text event. Validation happens before
    /// replacing composition state; callers can retry a rejected payload.
    pub fn apply(&mut self, event: MessageTextInputEvent) -> Result<Option<String>> {
        let validate = |text: &str| {
            if text.len() > 1024 * 1024 || text.contains('\0') {
                Err(invalid("text input contains NUL or exceeds one MiB"))
            } else {
                Ok(())
            }
        };
        match event {
            MessageTextInputEvent::ImeEnabled(enabled) => {
                self.ime_enabled = enabled;
                if !enabled {
                    self.cancel();
                }
            }
            MessageTextInputEvent::Preedit { text, selection } => {
                validate(&text)?;
                if let Some([start, end]) = selection {
                    let mut boundaries = vec![0u32];
                    let mut offset = 0u32;
                    for character in text.chars() {
                        offset += character.len_utf16() as u32;
                        boundaries.push(offset);
                    }
                    if start > end
                        || boundaries.binary_search(&start).is_err()
                        || boundaries.binary_search(&end).is_err()
                    {
                        return Err(invalid(
                            "IME selection splits a UTF-16 character or exceeds preedit",
                        ));
                    }
                }
                if self.focused {
                    self.ime_enabled = true;
                    self.preedit = text;
                    self.selection_utf16 = selection;
                }
            }
            MessageTextInputEvent::Commit(text) => {
                validate(&text)?;
                if self.focused {
                    self.cancel();
                    if !text.is_empty() {
                        self.commit_count = self.commit_count.wrapping_add(1);
                        return Ok(Some(text));
                    }
                }
            }
            MessageTextInputEvent::HardwareText(text) => {
                validate(&text)?;
                if self.focused && !self.ime_enabled {
                    let text: String = text
                        .chars()
                        .filter(|character| !character.is_control())
                        .collect();
                    if !text.is_empty() {
                        self.commit_count = self.commit_count.wrapping_add(1);
                        return Ok(Some(text));
                    }
                }
            }
            MessageTextInputEvent::Focus(focused) => {
                self.focused = focused;
                if !focused {
                    self.ime_enabled = false;
                    self.cancel();
                    self.composition_keys.clear();
                }
            }
            MessageTextInputEvent::Cancel => self.cancel(),
        }
        Ok(None)
    }

    pub fn composing(&self) -> bool {
        !self.preedit.is_empty()
    }
    /// Retain ownership through key-up even if Commit or Cancel cleared the
    /// preedit in between, so the game never receives an unmatched release.
    pub fn consumes_key(&mut self, key: u32, down: bool) -> Result<bool> {
        if down && self.composing() {
            if !self.composition_keys.contains(&key) {
                if self.composition_keys.len() >= 4096 {
                    return Err(invalid("IME held-key collection exceeds its limit"));
                }
                self.composition_keys.push(key);
            }
            Ok(true)
        } else if !down && self.composition_keys.contains(&key) {
            self.composition_keys.retain(|&held| held != key);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn cancel(&mut self) {
        self.preedit.clear();
        self.selection_utf16 = None;
    }
}

impl MessageEncoding {
    pub fn configuration_size(self) -> usize {
        match self {
            Self::Cp932 => 0xe8,
            Self::Unicode => 0x12c,
        }
    }
}

/// Unlisted DWORDs and font-name units are retained verbatim.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageConfiguration {
    bytes: Vec<u8>,
    encoding: MessageEncoding,
}

impl MessageConfiguration {
    pub fn from_bytes(bytes: &[u8], encoding: MessageEncoding) -> Result<Self> {
        let length = encoding.configuration_size();
        if bytes.len() < length || bytes.get(..4) != Some((length as u32).to_le_bytes().as_slice())
        {
            return Err(invalid(
                "message configuration size tag or capacity is invalid",
            ));
        }
        Ok(Self {
            bytes: bytes[..length].to_vec(),
            encoding,
        })
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn encoding(&self) -> MessageEncoding {
        self.encoding
    }
    pub fn word(&self, offset: usize) -> Result<u32> {
        let bytes = self
            .bytes
            .get(offset..offset.saturating_add(4))
            .ok_or_else(|| invalid("message configuration DWORD is outside its buffer"))?;
        Ok(u32::from_le_bytes(bytes.try_into().unwrap()))
    }
    fn value(&self, offset: usize) -> u32 {
        // Only used with the documented, validated configuration offsets.
        u32::from_le_bytes(self.bytes[offset..offset + 4].try_into().unwrap())
    }
    pub fn size(&self) -> [u32; 2] {
        [self.value(8), self.value(12)]
    }
    pub fn display_mode(&self) -> u32 {
        self.value(4)
    }
    pub fn ordinary_submit(&self) -> bool {
        self.value(self.canvas_fields() + 8) != 0
    }
    pub fn canvas_offset(&self) -> [i32; 2] {
        [
            self.value(self.canvas_fields()) as i32,
            self.value(self.canvas_fields() + 4) as i32,
        ]
    }
    pub fn reveal_width(&self) -> u32 {
        if self.display_mode() == 1 {
            self.value(self.tail() + 0x10)
        } else {
            0
        }
    }
    pub fn font_weight(&self) -> u32 {
        self.value(self.tail())
    }
    pub fn glyph_parameter(&self) -> u32 {
        self.value(self.tail() + 12)
    }
    pub fn font_backend(&self) -> u32 {
        self.value(self.tail() + 20)
    }
    pub fn measure_parameter(&self) -> u32 {
        self.value(0x80)
    }
    pub fn text_sizes(&self) -> [u32; 5] {
        std::array::from_fn(|index| self.value(0x1c + index * 4))
    }
    pub fn ruby_sizes(&self) -> [u32; 5] {
        std::array::from_fn(|index| self.value(0x30 + index * 4))
    }
    pub fn font_name(&self) -> Vec<u16> {
        self.bytes[0x84..self.tail()]
            .chunks_exact(2)
            .map(|bytes| u16::from_le_bytes(bytes.try_into().unwrap()))
            .take_while(|&unit| unit != 0)
            .collect()
    }
    fn tail(&self) -> usize {
        match self.encoding {
            MessageEncoding::Cp932 => 0xc4,
            MessageEncoding::Unicode => 0x104,
        }
    }
    fn canvas_fields(&self) -> usize {
        match self.encoding {
            MessageEncoding::Cp932 => 0xdc,
            MessageEncoding::Unicode => 0x120,
        }
    }
    fn initial_style(&self) -> MessageStyle {
        MessageStyle {
            text_color: self.value(0x10),
            shadow_color: self.value(0x14),
            ruby_color: self.value(0x18),
            text_size: self.value(0x24),
            ruby_size: self.value(0x38),
            bold: false,
            italic: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageDefaults {
    pub font_name: Vec<u16>,
    pub bold: bool,
    pub shadow: u32,
    pub outline: u32,
    pub glyph_parameter: Option<u32>,
    pub gradual: bool,
}

impl MessageDefaults {
    pub fn configuration(&self, encoding: MessageEncoding) -> Result<MessageConfiguration> {
        let length = encoding.configuration_size();
        let tail = if encoding == MessageEncoding::Cp932 {
            0xc4
        } else {
            0x104
        };
        let name: Vec<_> = self
            .font_name
            .iter()
            .copied()
            .take_while(|&unit| unit != 0)
            .collect();
        if name.len() >= (tail - 0x84) / 2 {
            return Err(invalid(
                "message default font name exceeds its UTF-16 capacity",
            ));
        }
        let mut bytes = vec![0; length];
        let mut set = |offset: usize, value: u32| {
            bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        };
        set(0, length as u32);
        set(4, u32::from(self.gradual));
        set(8, 800);
        set(12, 600);
        set(0x10, 0xffff_ffff);
        set(0x14, 0xff00_0000);
        set(0x18, 0xffff_ffff);
        for index in 0..5 {
            set(0x1c + index * 4, 24);
            set(0x30 + index * 4, 10);
        }
        set(0x60, 2);
        set(0x64, 1);
        set(0x6c, 1);
        set(tail, if self.bold { 700 } else { 400 });
        set(tail + 4, self.shadow);
        set(tail + 8, self.outline);
        set(tail + 12, self.glyph_parameter.unwrap_or(1));
        set(tail + 16, 60);
        for (index, unit) in name.into_iter().enumerate() {
            bytes[0x84 + index * 2..0x86 + index * 2].copy_from_slice(&unit.to_le_bytes());
        }
        MessageConfiguration::from_bytes(&bytes, encoding)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageStyle {
    /// Engine AABBGGRR, not renderer ARGB.
    pub text_color: u32,
    pub ruby_color: u32,
    pub shadow_color: u32,
    pub text_size: u32,
    pub ruby_size: u32,
    pub bold: bool,
    pub italic: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MessageAtomKind {
    Glyph {
        character: char,
        encoding: MessageEncoding,
        style: MessageStyle,
        group: Option<u32>,
        ruby: bool,
    },
    Separator,
    Newline,
    Control(u32),
    Align(u32),
    /// The value is a raw record category, NOT the final X coordinate.
    RawX(u32),
    /// The value is the raw record's X field, NOT the final Y coordinate.
    RawY(u32),
    Continue,
    ResetWait,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageAtom {
    pub kind: MessageAtomKind,
    pub source: Range<usize>,
    pub deadline: u32,
    /// Original z-stack depth, used to select a retained UTF-16 font override.
    #[serde(default)]
    pub font_depth: u32,
    /// Unicode ui keeps character code ten for layout/counting and stores its
    /// independent HG frame DWORD here. It is not a newline control record.
    #[serde(default)]
    pub inline_image: Option<u32>,
}

/// One entry per atom, plus any automatically inserted newline entries.
/// `atom: None` is permitted only for an automatic newline.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageLayoutEntry {
    pub atom: Option<usize>,
    pub position: [i32; 2],
    pub size: [i32; 2],
    pub line: u32,
    pub display_anchor: Option<usize>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageLayout {
    pub entries: Vec<MessageLayoutEntry>,
    pub line_count: u32,
    pub cursor: [i32; 2],
    pub bottom: i32,
}

impl MessageLayout {
    pub fn validate(&self, atoms: &[MessageAtom]) -> Result<()> {
        if self.entries.len() > MAX_ENTRIES {
            return Err(invalid("message layout entry limit exceeded"));
        }
        let mut seen = vec![false; atoms.len()];
        let mut previous = None;
        for (index, entry) in self.entries.iter().enumerate() {
            if let Some(atom) = entry.atom {
                let duplicate = seen
                    .get_mut(atom)
                    .ok_or_else(|| invalid("message layout references an unknown atom"))?;
                if *duplicate || previous.is_some_and(|old| atom <= old) {
                    return Err(invalid(
                        "message layout changed atom order or duplicated an atom",
                    ));
                }
                *duplicate = true;
                previous = Some(atom);
            } else if entry.display_anchor.is_some() {
                return Err(invalid("automatic newline cannot have a ruby anchor"));
            }
            if let Some(anchor) = entry.display_anchor {
                if anchor >= index
                    || !matches!(
                        entry.atom.and_then(|i| atoms.get(i)).map(|atom| &atom.kind),
                        Some(MessageAtomKind::Glyph { ruby: true, .. })
                    )
                    || !matches!(
                        self.entries[anchor]
                            .atom
                            .and_then(|i| atoms.get(i))
                            .map(|atom| &atom.kind),
                        Some(MessageAtomKind::Glyph { ruby: false, .. })
                    )
                {
                    return Err(invalid(
                        "message ruby anchor must reference an earlier body glyph",
                    ));
                }
            }
        }
        if seen.iter().any(|value| !value) {
            return Err(invalid("message layout omitted an atom"));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum MessageGlyphState {
    #[default]
    Hidden,
    Revealing,
    Complete,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageUserGlyph {
    pub size: [u32; 2],
    /// Full original canvas, top-down BGRA; transparent RGB is preserved.
    pub bgra: Vec<u8>,
}

impl MessageUserGlyph {
    pub fn validate(&self) -> Result<()> {
        let length = (self.size[0] as usize)
            .checked_mul(self.size[1] as usize)
            .and_then(|length| length.checked_mul(4))
            .filter(|&length| length <= MAX_BYTES)
            .ok_or_else(|| invalid("message user glyph canvas exceeds storage limit"))?;
        if length != self.bgra.len() {
            return Err(invalid(
                "message user glyph BGRA length differs from its canvas",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageUserGlyphMapping {
    /// Full DWORD frame id. None means missing frame or no source at mapping time.
    pub frame: u32,
    pub decoded: Option<MessageUserGlyph>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessagePlaneDisplay {
    Unreachable,
    Hidden,
    Visible,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageBreakCharacters {
    pub no_start: Vec<char>,
    pub no_end: Vec<char>,
    pub repeated: Vec<char>,
    pub indent_open: Vec<char>,
    pub indent_keep: Vec<char>,
}

impl MessageBreakCharacters {
    pub fn for_encoding(encoding: MessageEncoding) -> Self {
        let no_end = "([{\u{ff08}\u{ff3b}\u{300c}\u{300e}\u{3010}";
        let mut result = Self {
            no_start: "\u{3001}\u{3002},.!?:;\u{ff01}\u{ff1f}\u{ff0c}\u{ff0e}\u{ff09}\u{ff3d}\u{300d}\u{300f}\u{3011}\u{30fc}".chars().collect(),
            no_end: no_end.chars().collect(),
            repeated: "\u{2026}\u{2025}\u{2015}\u{30fc}".chars().collect(),
            indent_open: no_end.chars().collect(),
            indent_keep: "\u{300d}\u{300f}\u{ff09}\u{ff3d}\u{3011}".chars().collect(),
        };
        if encoding == MessageEncoding::Unicode {
            result.no_start.extend("\u{2019}\u{201d}\u{2014}".chars());
            result.no_end.extend("\u{2018}\u{201c}".chars());
        }
        result
    }
    fn collection_mut(&mut self, index: u32) -> Option<&mut Vec<char>> {
        match index {
            0 => Some(&mut self.no_start),
            1 => Some(&mut self.no_end),
            2 => Some(&mut self.repeated),
            3 => Some(&mut self.indent_open),
            4 => Some(&mut self.indent_keep),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageCopy {
    /// Update persistent surfaces, retaining dirty state.
    None,
    /// Copy only a nonempty dirty rectangle.
    Dirty,
    /// Visible startup copies the whole surface even when ordinary submit is off.
    Full,
}

/// Consumed void commands preserve the caller's return slot. Unhandled is
/// explicit; a dispatcher must never substitute Value(0) for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageCommandResult {
    Unhandled,
    Handled,
    Value(u32),
}

/// All references remain valid for the synchronous backend call only.
pub struct MessageRenderRequest<'a> {
    pub object: &'a MessageObject,
    pub configuration: &'a MessageConfiguration,
    pub layout: &'a MessageLayout,
    /// Hidden entries requiring rasterization, in registration order.
    pub prepare: &'a [usize],
    /// Ordinary due entries, or ALL unfinished in-range entries for gradual mode.
    pub process: &'a [usize],
    pub reveal_phase: Option<i32>,
    pub force: bool,
    pub copy: MessageCopy,
    /// Change means rebuild pixels, never retain a previous object's texture.
    pub surface_generation: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MessagePaintResult {
    /// Successfully rasterized entries. A gradual painter may complete only some.
    pub prepared: Vec<usize>,
    pub completed: Vec<usize>,
    /// True only after an actual nonempty dirty-rectangle copy.
    pub copied_dirty: bool,
    /// A painter that fails AFTER drawing a prefix returns that prefix and the
    /// diagnostic here. A plain Err is reserved for failure before any mutation.
    pub failure: Option<MessagePaintFailure>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MessagePaintFailure {
    pub offset: usize,
    pub message: &'static str,
}

/// Font selection, exact layout, persistent BGRA pixels and drawing-context
/// reachability are host concerns. No method has a fake-success default.
pub trait MessageBackend {
    fn layout(
        &mut self,
        configuration: &MessageConfiguration,
        atoms: &[MessageAtom],
        previous: Option<&MessageLayout>,
    ) -> Result<MessageLayout>;
    fn layout_with_break_characters(
        &mut self,
        configuration: &MessageConfiguration,
        atoms: &[MessageAtom],
        previous: Option<&MessageLayout>,
        characters: &MessageBreakCharacters,
    ) -> Result<MessageLayout> {
        if *characters != MessageBreakCharacters::for_encoding(configuration.encoding()) {
            return Err(invalid(
                "message backend has no custom break-character layout",
            ));
        }
        self.layout(configuration, atoms, previous)
    }
    /// Returns true after an actual full submission. No fake-success default.
    fn redraw_message(&mut self, _object: &MessageObject) -> Result<bool> {
        Err(invalid(
            "message backend has no completed-glyph redraw service",
        ))
    }
    /// Task clear copies an empty plane only when its submit setting allows
    /// it. The page is not prepared and the completion snapshot is not updated.
    fn clear_message(&mut self, _object: &MessageObject) -> Result<()> {
        Err(invalid("message backend has no clear-and-submit service"))
    }
    fn decode_user_glyph(&mut self, source: &[u8], frame: u32) -> Result<Option<MessageUserGlyph>>;
    fn paint(&mut self, request: MessageRenderRequest<'_>) -> Result<MessagePaintResult>;
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MessageRevealUpdate {
    pub newly_prepared: Vec<usize>,
    pub newly_complete: Vec<usize>,
    pub complete: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct ParserState {
    style: MessageStyle,
    restore_color: u32,
    next_group: u32,
    accumulated_units: u32,
    #[serde(default = "initial_font_overrides")]
    font_overrides: Vec<Vec<u16>>,
    #[serde(default)]
    font_depth: u32,
}

fn initial_font_overrides() -> Vec<Vec<u16>> {
    vec![Vec::new()]
}

impl ParserState {
    fn new(configuration: &MessageConfiguration) -> Self {
        let style = configuration.initial_style();
        Self {
            style,
            restore_color: style.text_color,
            next_group: 0,
            accumulated_units: 0,
            font_overrides: initial_font_overrides(),
            font_depth: 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageObject {
    id: i32,
    encoding: MessageEncoding,
    configuration: Option<MessageConfiguration>,
    source: String,
    /// The exact appended byte payloads; separators are recorded separately below.
    raw_appends: Vec<Vec<u8>>,
    separator_offsets: Vec<usize>,
    parsed_bytes: usize,
    parser: Option<ParserState>,
    atoms: Vec<MessageAtom>,
    identifiers: BTreeMap<i32, u32>,
    layout: Option<MessageLayout>,
    deadlines: Vec<u32>,
    states: Vec<MessageGlyphState>,
    valid: bool,
    speed: u32,
    time_origin: u32,
    deadline_origin: u32,
    last_scan_time: u32,
    reached_deadline: u32,
    segment_limit: u32,
    active: bool,
    playback_time: u32,
    last_input_time: u32,
    complete_snapshot: bool,
    last_complete: bool,
    surface_generation: u64,
    surface_change: MessageSurfaceChange,
    /// Original registration geometry survives reconfiguration.
    registered_size: Option<[u32; 2]>,
    registered_offset: [i32; 2],
    local_offset: [i32; 2],
    break_characters: MessageBreakCharacters,
    user_glyph_source: Option<Vec<u8>>,
    user_glyphs: BTreeMap<u16, MessageUserGlyphMapping>,
    #[serde(default)]
    inline_image_source: Option<Vec<u8>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageObjectSnapshot {
    pub version: u32,
    pub object: MessageObject,
}

/// Optional CPU/device snapshot for exact restoration of partial Alpha and
/// dirty/submitted state. RGBA is explicit here; mapped image glyphs stay BGRA.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessagePixelSnapshot {
    pub version: u32,
    pub object_id: i32,
    pub surface_generation: u64,
    pub size: [u32; 2],
    pub visible_rgba: Vec<u8>,
    pub source_rgba: Option<Vec<u8>>,
    pub submitted_rgba: Vec<u8>,
    pub dirty: Option<[i32; 4]>,
    /// Entry index, position, local bounds, reveal start.
    pub glyphs: Vec<(usize, [i32; 2], [i32; 4], i32)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MessageSurfaceChange {
    Recreate,
    Reconfigure,
    Clear,
    Restore,
}

/// Borrowed presentation view. `prepared == false` deliberately exposes the
/// last successful layout together with the pending complete source string.
/// The painter must keep actual partial Alpha/bounds in its surface cache.
pub struct MessageRenderState<'a> {
    pub id: i32,
    pub prepared: bool,
    pub source: &'a str,
    pub configuration: Option<&'a MessageConfiguration>,
    pub atoms: &'a [MessageAtom],
    pub font_overrides: &'a [Vec<u16>],
    pub layout: Option<&'a MessageLayout>,
    pub deadlines: &'a [u32],
    pub glyph_states: &'a [MessageGlyphState],
    pub user_glyphs: &'a BTreeMap<u16, MessageUserGlyphMapping>,
    pub inline_image_source: Option<&'a [u8]>,
    pub surface_generation: u64,
    pub active: bool,
    pub playback_time: u32,
    pub segment_limit: u32,
    pub complete_snapshot: bool,
}

impl MessageObject {
    pub fn new(id: i32, encoding: MessageEncoding) -> Self {
        Self {
            id,
            encoding,
            configuration: None,
            source: String::new(),
            raw_appends: Vec::new(),
            separator_offsets: Vec::new(),
            parsed_bytes: 0,
            parser: None,
            atoms: Vec::new(),
            identifiers: BTreeMap::new(),
            layout: None,
            deadlines: Vec::new(),
            states: Vec::new(),
            valid: false,
            speed: 10,
            time_origin: 0,
            deadline_origin: 0,
            last_scan_time: 0,
            reached_deadline: 0,
            segment_limit: 0,
            active: false,
            playback_time: 0,
            last_input_time: 0,
            complete_snapshot: false,
            last_complete: false,
            surface_generation: 0,
            surface_change: MessageSurfaceChange::Recreate,
            registered_size: None,
            registered_offset: [0; 2],
            local_offset: [0; 2],
            break_characters: MessageBreakCharacters::for_encoding(encoding),
            user_glyph_source: None,
            user_glyphs: BTreeMap::new(),
            inline_image_source: None,
        }
    }

    pub fn id(&self) -> i32 {
        self.id
    }
    pub fn encoding(&self) -> MessageEncoding {
        self.encoding
    }
    pub fn configured(&self) -> bool {
        self.configuration.is_some()
    }
    pub fn configuration(&self) -> Option<&MessageConfiguration> {
        self.configuration.as_ref()
    }
    pub fn configuration_bytes(&self) -> Option<&[u8]> {
        self.configuration.as_ref().map(MessageConfiguration::bytes)
    }
    pub fn source(&self) -> &str {
        &self.source
    }
    pub fn raw_appends(&self) -> &[Vec<u8>] {
        &self.raw_appends
    }
    pub fn separator_offsets(&self) -> &[usize] {
        &self.separator_offsets
    }
    pub fn atoms(&self) -> &[MessageAtom] {
        &self.atoms
    }
    /// Retained z names from the latest successful prepare. Selection follows
    /// the original depth counter; closing a scope does not remove its name.
    pub fn font_overrides(&self) -> &[Vec<u16>] {
        self.parser
            .as_ref()
            .map_or(&[], |parser| parser.font_overrides.as_slice())
    }
    pub fn atom_font_name(&self, atom: usize) -> &[u16] {
        let Some(atom) = self.atoms.get(atom) else {
            return &[];
        };
        let names = self.font_overrides();
        names
            .get(atom.font_depth as usize)
            .or_else(|| names.first())
            .map_or(&[], Vec::as_slice)
    }
    pub fn layout(&self) -> Option<&MessageLayout> {
        self.layout.as_ref()
    }
    pub fn deadlines(&self) -> &[u32] {
        &self.deadlines
    }
    pub fn glyph_states(&self) -> &[MessageGlyphState] {
        &self.states
    }
    pub fn prepared(&self) -> bool {
        self.valid
    }
    pub fn active(&self) -> bool {
        self.active
    }
    pub fn speed(&self) -> u32 {
        self.speed
    }
    pub fn playback_time(&self) -> u32 {
        self.playback_time
    }
    pub fn segment_limit(&self) -> u32 {
        self.segment_limit
    }
    pub fn complete_snapshot(&self) -> bool {
        self.complete_snapshot
    }
    pub fn last_complete(&self) -> bool {
        self.last_complete
    }
    pub fn surface_generation(&self) -> u64 {
        self.surface_generation
    }
    pub fn surface_change(&self) -> MessageSurfaceChange {
        self.surface_change
    }
    pub fn registered_geometry(&self) -> Option<([u32; 2], [i32; 2])> {
        self.registered_size
            .map(|size| (size, self.registered_offset))
    }
    pub fn local_offset(&self) -> [i32; 2] {
        self.local_offset
    }
    pub fn break_characters(&self) -> &MessageBreakCharacters {
        &self.break_characters
    }

    /// Command470/491. Pending source is not parsed and cached cursor/line/
    /// time fields are not changed. The CPU backend repaints Complete only.
    pub fn set_shift(&mut self, value: u32, backend: &mut impl MessageBackend) -> Result<()> {
        if !self.configured() {
            return Ok(());
        }
        let next = [0, value as i32];
        let difference = [
            next[0].wrapping_sub(self.local_offset[0]),
            next[1].wrapping_sub(self.local_offset[1]),
        ];
        self.local_offset = next;
        if let Some(layout) = &mut self.layout {
            for entry in &mut layout.entries {
                entry.position[0] = entry.position[0].wrapping_add(difference[0]);
                entry.position[1] = entry.position[1].wrapping_add(difference[1]);
            }
        }
        if backend.redraw_message(self)? {
            self.complete_snapshot = self.valid && self.remaining_all()? == 0;
        }
        Ok(())
    }

    /// Collection edits deliberately do not invalidate/prepare the current
    /// layout. A subsequent append or reconfigure consumes the saved sets.
    pub fn clear_break_characters(&mut self, index: u32) {
        if self.configured() {
            if let Some(collection) = self.break_characters.collection_mut(index) {
                collection.clear();
            }
        }
    }
    pub fn add_break_characters(&mut self, index: u32, bytes: &[u8]) -> Result<()> {
        if !self.configured() || index > 4 {
            return Ok(());
        }
        let bytes = bytes.split(|&byte| byte == 0).next().unwrap_or_default();
        let text = match self.encoding {
            MessageEncoding::Cp932 => {
                let (text, _, errors) = encoding_rs::SHIFT_JIS.decode(bytes);
                if errors {
                    return Err(invalid("message character collection has invalid CP932"));
                }
                text.into_owned()
            }
            MessageEncoding::Unicode => {
                let text = std::str::from_utf8(bytes)
                    .map_err(|_| invalid("message character collection has invalid UTF-8"))?;
                if text.encode_utf16().count() >= 2048 {
                    return Err(invalid(
                        "message Unicode character collection exceeds its input capacity",
                    ));
                }
                text.to_owned()
            }
        };
        if text.chars().any(|character| character as u32 > 0xffff) {
            return Err(invalid(
                "non-BMP message character collection is not recovered",
            ));
        }
        let collection = self.break_characters.collection_mut(index).unwrap();
        for character in text.chars() {
            if !collection.contains(&character) {
                collection.push(character);
            }
        }
        Ok(())
    }
    pub fn remove_break_characters(&mut self, index: u32, bytes: &[u8]) -> Result<()> {
        if !self.configured() || index > 4 {
            return Ok(());
        }
        let bytes = bytes.split(|&byte| byte == 0).next().unwrap_or_default();
        let mut removed = Vec::new();
        let mut cursor = 0;
        while cursor < bytes.len() {
            let first = bytes[cursor];
            removed.push(if let Some(&next) = bytes.get(cursor + 1) {
                u16::from_be_bytes([first, next])
            } else {
                first as i8 as i16 as u16
            });
            cursor += if matches!(first, 0x81..=0x9f | 0xe0..=0xfc) {
                2
            } else {
                1
            };
        }
        let encoding = self.encoding;
        self.break_characters
            .collection_mut(index)
            .unwrap()
            .retain(|&character| {
                let code = if encoding == MessageEncoding::Unicode {
                    character as u32 as u16
                } else {
                    let mut buffer = [0; 4];
                    let (encoded, _, _) =
                        encoding_rs::SHIFT_JIS.encode(character.encode_utf8(&mut buffer));
                    match encoded.as_ref() {
                        [first] => u16::from(*first),
                        [first, second] => u16::from_be_bytes([*first, *second]),
                        _ => return true,
                    }
                };
                !removed.contains(&code)
            });
        Ok(())
    }
    pub fn user_glyph_source(&self) -> Option<&[u8]> {
        self.user_glyph_source.as_deref()
    }
    pub fn user_glyphs(&self) -> &BTreeMap<u16, MessageUserGlyphMapping> {
        &self.user_glyphs
    }
    pub fn identifier(&self, key: i32) -> u32 {
        self.identifiers.get(&key).copied().unwrap_or(0)
    }
    pub fn is_visible(&self, entry: usize) -> Option<bool> {
        self.states
            .get(entry)
            .map(|state| *state == MessageGlyphState::Complete)
    }
    pub fn render_state(&self) -> MessageRenderState<'_> {
        MessageRenderState {
            id: self.id,
            prepared: self.valid,
            source: &self.source,
            configuration: self.configuration.as_ref(),
            atoms: &self.atoms,
            font_overrides: self.font_overrides(),
            layout: self.layout.as_ref(),
            deadlines: &self.deadlines,
            glyph_states: &self.states,
            user_glyphs: &self.user_glyphs,
            inline_image_source: self.inline_image_source.as_deref(),
            surface_generation: self.surface_generation,
            active: self.active,
            playback_time: self.playback_time,
            segment_limit: self.segment_limit,
            complete_snapshot: self.complete_snapshot,
        }
    }

    pub fn snapshot(&self) -> MessageObjectSnapshot {
        MessageObjectSnapshot {
            version: 1,
            object: self.clone(),
        }
    }

    /// Pixel surfaces are external. Restore requires repainting saved states
    /// before exposing the texture to the renderer; this changes its generation.
    pub fn from_snapshot(snapshot: MessageObjectSnapshot) -> Result<Self> {
        if snapshot.version != 1 {
            return Err(invalid("unsupported message snapshot version"));
        }
        let mut object = snapshot.object;
        object.validate_snapshot()?;
        object.surface_generation = object.surface_generation.wrapping_add(1);
        object.surface_change = MessageSurfaceChange::Restore;
        Ok(object)
    }

    fn validate_snapshot(&self) -> Result<()> {
        if let Some(parser) = &self.parser {
            let name_units = parser
                .font_overrides
                .iter()
                .try_fold(0usize, |sum, name| sum.checked_add(name.len()));
            if parser.font_overrides.is_empty()
                || parser.font_overrides.len() > MAX_ENTRIES
                || parser.font_depth as usize >= parser.font_overrides.len()
                || name_units.is_none_or(|length| length > MAX_BYTES / 2)
                || parser
                    .font_overrides
                    .iter()
                    .any(|name| name.contains(&0) || String::from_utf16(name).is_err())
                || self
                    .atoms
                    .iter()
                    .any(|atom| atom.font_depth as usize >= parser.font_overrides.len())
            {
                return Err(invalid("message snapshot z font-name state is invalid"));
            }
        }
        for collection in [
            &self.break_characters.no_start,
            &self.break_characters.no_end,
            &self.break_characters.repeated,
            &self.break_characters.indent_open,
            &self.break_characters.indent_keep,
        ] {
            if collection.len() > 65536
                || collection
                    .iter()
                    .any(|&character| character == '\0' || character as u32 > 0xffff)
            {
                return Err(invalid("message snapshot character collection is invalid"));
            }
        }
        let raw_bytes = self
            .raw_appends
            .iter()
            .try_fold(0usize, |total, bytes| total.checked_add(bytes.len()));
        if self.source.len() > MAX_BYTES
            || self.source.contains('\0')
            || self.atoms.len() > MAX_ENTRIES
            || self.parsed_bytes > self.source.len()
            || !self.source.is_char_boundary(self.parsed_bytes)
            || self.user_glyphs.len() > 65536
            || raw_bytes.is_none_or(|length| length > MAX_BYTES)
            || self.raw_appends.len() > MAX_ENTRIES
            || self.raw_appends.iter().any(|bytes| bytes.contains(&0))
            || self
                .user_glyph_source
                .as_ref()
                .is_some_and(|source| source.len() > MAX_BYTES)
            || self
                .inline_image_source
                .as_ref()
                .is_some_and(|source| source.len() > MAX_BYTES)
        {
            return Err(invalid(
                "invalid message snapshot storage or parser boundary",
            ));
        }
        if let Some(configuration) = &self.configuration {
            MessageConfiguration::from_bytes(configuration.bytes(), self.encoding)?;
            if configuration.encoding != self.encoding || self.parser.is_none() {
                return Err(invalid(
                    "message snapshot configuration/parser encoding differs",
                ));
            }
        } else if self.parser.is_some() || self.layout.is_some() || self.valid {
            return Err(invalid(
                "unconfigured message snapshot contains prepared state",
            ));
        }
        if let Some(layout) = &self.layout {
            layout.validate(&self.atoms)?;
            if self.deadlines.len() != layout.entries.len()
                || self.states.len() != layout.entries.len()
            {
                return Err(invalid(
                    "message snapshot display arrays differ from layout",
                ));
            }
            if self
                .states
                .iter()
                .enumerate()
                .any(|(index, state)| *state != MessageGlyphState::Hidden && !self.is_glyph(index))
            {
                return Err(invalid(
                    "message snapshot marks a control record as displayed",
                ));
            }
        } else if self.valid || !self.deadlines.is_empty() || !self.states.is_empty() {
            return Err(invalid(
                "message snapshot has display state without a layout",
            ));
        }
        if self.valid && self.parsed_bytes != self.source.len() {
            return Err(invalid("prepared message snapshot contains pending text"));
        }
        let mut last_separator = None;
        for &offset in &self.separator_offsets {
            if last_separator.is_some_and(|last| offset <= last)
                || self
                    .source
                    .get(offset..)
                    .is_none_or(|text| !text.starts_with('\u{ffff}'))
            {
                return Err(invalid("message snapshot separator offset is invalid"));
            }
            last_separator = Some(offset);
        }
        for atom in &self.atoms {
            if atom.inline_image.is_some()
                && (self.encoding != MessageEncoding::Unicode
                    || !matches!(
                        atom.kind,
                        MessageAtomKind::Glyph {
                            character: '\n',
                            encoding: MessageEncoding::Unicode,
                            group: None,
                            ruby: false,
                            ..
                        }
                    ))
            {
                return Err(invalid("message snapshot has an invalid ui image record"));
            }
            if atom.source.start > atom.source.end
                || atom.source.end > self.parsed_bytes
                || !self.source.is_char_boundary(atom.source.start)
                || !self.source.is_char_boundary(atom.source.end)
            {
                return Err(invalid("message snapshot atom source is invalid"));
            }
        }
        for glyph in self
            .user_glyphs
            .values()
            .filter_map(|mapping| mapping.decoded.as_ref())
        {
            glyph.validate()?;
        }
        Ok(())
    }

    /// Command 456 destroys the old registration/source BEFORE checking the tag.
    /// Source means message text here; the independent user image source survives.
    pub fn recreate(
        &mut self,
        bytes: &[u8],
        time_scale: Option<u32>,
        backend: &mut impl MessageBackend,
    ) -> Result<()> {
        self.clear();
        self.configuration = None;
        self.parser = None;
        self.layout = None;
        self.registered_size = None;
        let configuration = MessageConfiguration::from_bytes(bytes, self.encoding)?;
        let mut replacement = Self::new(self.id, self.encoding);
        replacement.user_glyph_source = self.user_glyph_source.clone();
        replacement.user_glyphs = self.user_glyphs.clone();
        replacement.inline_image_source = self.inline_image_source.clone();
        replacement.break_characters = self.break_characters.clone();
        replacement.surface_generation = self.surface_generation;
        replacement.install_configuration(configuration, time_scale, backend)?;
        replacement.surface_change = MessageSurfaceChange::Recreate;
        replacement.registered_size = replacement
            .configuration
            .as_ref()
            .map(MessageConfiguration::size);
        replacement.registered_offset = replacement.configuration.as_ref().unwrap().canvas_offset();
        *self = replacement;
        Ok(())
    }

    /// Command 457 reparses complete source with the new defaults. Failure keeps
    /// the old object, pending append text, speed and saved display states.
    pub fn reconfigure(
        &mut self,
        bytes: &[u8],
        time_scale: Option<u32>,
        backend: &mut impl MessageBackend,
    ) -> Result<()> {
        if !self.configured() {
            return self.recreate(bytes, time_scale, backend);
        }
        let configuration = MessageConfiguration::from_bytes(bytes, self.encoding)?;
        let mut replacement = Self::new(self.id, self.encoding);
        replacement.source = self.source.clone();
        replacement.raw_appends = self.raw_appends.clone();
        replacement.separator_offsets = self.separator_offsets.clone();
        replacement.speed = self.speed;
        replacement.playback_time = self.playback_time;
        replacement.last_input_time = self.last_input_time;
        replacement.active = self.active;
        replacement.segment_limit = self.segment_limit;
        replacement.registered_size = self.registered_size;
        replacement.registered_offset = self.registered_offset;
        replacement.local_offset = self.local_offset;
        replacement.break_characters = self.break_characters.clone();
        replacement.user_glyph_source = self.user_glyph_source.clone();
        replacement.user_glyphs = self.user_glyphs.clone();
        replacement.inline_image_source = self.inline_image_source.clone();
        replacement.surface_generation = self.surface_generation;
        replacement.install_configuration(configuration, time_scale, backend)?;
        replacement.surface_change = MessageSurfaceChange::Reconfigure;
        *self = replacement;
        Ok(())
    }

    fn install_configuration(
        &mut self,
        configuration: MessageConfiguration,
        time_scale: Option<u32>,
        backend: &mut impl MessageBackend,
    ) -> Result<()> {
        self.parser = Some(ParserState::new(&configuration));
        self.configuration = Some(configuration);
        self.surface_generation = self.surface_generation.wrapping_add(1);
        self.surface_change = MessageSurfaceChange::Reconfigure;
        self.rebuild_user_glyphs(backend)?;
        self.prepare(time_scale, backend)?;
        Ok(())
    }

    pub fn append(&mut self, bytes: &[u8]) -> Result<()> {
        self.append_with_legacy_codec(bytes, encoding_rs::SHIFT_JIS)
    }

    pub fn append_legacy_gbk(&mut self, bytes: &[u8]) -> Result<()> {
        self.append_with_legacy_codec(bytes, encoding_rs::GBK)
    }

    fn append_with_legacy_codec(&mut self, bytes: &[u8], codec: &'static encoding_rs::Encoding) -> Result<()> {
        if !self.configured() {
            return Ok(());
        }
        if bytes.contains(&0) {
            return Err(invalid("message append contains an embedded zero"));
        }
        let text = match self.encoding {
            MessageEncoding::Cp932 => {
                // The original append front-end (0x1ea047) recognizes a raw
                // FF before CP932 decoding and stores the U+FFFF separator.
                // SHIFT_JIS alone would turn FF into a private-use glyph.
                let mut text = String::new();
                for (index, part) in bytes.split(|&byte| byte == 0xff).enumerate() {
                    if index != 0 {
                        text.push('\u{ffff}');
                    }
                    let decoded = if std::ptr::eq(codec, encoding_rs::GBK) {
                        codec.decode_without_bom_handling_and_without_replacement(part)
                            .ok_or_else(|| invalid("message append has invalid GBK input"))?
                    } else {
                        let (decoded, _, errors) = codec.decode(part);
                        if errors {
                            return Err(invalid("message append has invalid CP932 input"));
                        }
                        decoded
                    };
                    text.push_str(&decoded);
                }
                text
            }
            MessageEncoding::Unicode => {
                let text = std::str::from_utf8(bytes)
                    .map_err(|_| invalid("message append has invalid UTF-8 input"))?;
                text.to_owned()
            }
        };
        self.append_decoded(bytes, &text)
    }

    pub fn append_decoded(&mut self, bytes: &[u8], text: &str) -> Result<()> {
        if !self.configured() {
            return Ok(());
        }
        if bytes.contains(&0) || text.contains('\0') {
            return Err(invalid("message append contains an embedded zero"));
        }
        if self.encoding == MessageEncoding::Unicode && text.encode_utf16().count() >= 4096 {
            return Err(invalid("Unicode message append needs fewer than 4096 UTF-16 units"));
        }
        if self.source.len().saturating_add(text.len()) > MAX_BYTES
            || self.raw_appends.len() >= MAX_ENTRIES
            || self
                .raw_appends
                .iter()
                .map(Vec::len)
                .sum::<usize>()
                .saturating_add(bytes.len())
                > MAX_BYTES
        {
            return Err(invalid("message append storage limit exceeded"));
        }
        self.source.push_str(text);
        self.raw_appends.push(bytes.to_vec());
        self.valid = false;
        self.complete_snapshot = false;
        Ok(())
    }

    /// Separators invalidate preparation but do not change the completion snapshot.
    pub fn append_separator(&mut self) -> Result<()> {
        if !self.configured() {
            return Ok(());
        }
        if self.source.len().saturating_add(3) > MAX_BYTES {
            return Err(invalid("message separator storage limit exceeded"));
        }
        self.separator_offsets.push(self.source.len());
        self.source.push('\u{ffff}');
        self.valid = false;
        Ok(())
    }

    pub fn clear(&mut self) {
        self.source.clear();
        self.raw_appends.clear();
        self.separator_offsets.clear();
        self.parsed_bytes = 0;
        self.atoms.clear();
        self.identifiers.clear();
        self.layout = None;
        self.deadlines.clear();
        self.states.clear();
        self.parser = self.configuration.as_ref().map(ParserState::new);
        self.valid = false;
        self.active = false;
        self.playback_time = 0;
        self.last_input_time = 0;
        self.segment_limit = 0;
        self.time_origin = 0;
        self.deadline_origin = 0;
        self.last_scan_time = 0;
        self.reached_deadline = 0;
        self.last_complete = false;
        // Dirty state and complete_snapshot belong to actual canvas submissions.
        self.surface_generation = self.surface_generation.wrapping_add(1);
        self.surface_change = MessageSurfaceChange::Clear;
    }

    pub fn invalidate_layout(&mut self) {
        self.valid = false;
    }

    /// Task command459/480 calls the full-plane copy helper after clear.
    /// Keep `clear()` usable for the lower display/page-only operation.
    pub fn clear_with_backend(&mut self, backend: &mut impl MessageBackend) -> Result<()> {
        if !self.configured() {
            return Ok(());
        }
        self.clear();
        backend.clear_message(self)
    }

    pub fn prepare(
        &mut self,
        time_scale: Option<u32>,
        backend: &mut impl MessageBackend,
    ) -> Result<bool> {
        if self.valid {
            return Ok(false);
        }
        let configuration = self
            .configuration
            .as_ref()
            .ok_or_else(|| invalid("message object is not configured"))?;
        let mut parser = self
            .parser
            .clone()
            .ok_or_else(|| invalid("message parser is missing"))?;
        let mut atoms = self.atoms.clone();
        let mut identifiers = self.identifiers.clone();
        parse_append(
            &self.source,
            self.parsed_bytes,
            self.encoding,
            configuration,
            time_scale,
            &mut parser,
            &mut atoms,
            &mut identifiers,
        )?;
        let mut previous = self.layout.clone();
        if let Some(layout) = &mut previous {
            for entry in &mut layout.entries {
                entry.position[0] = entry.position[0].wrapping_sub(self.local_offset[0]);
                entry.position[1] = entry.position[1].wrapping_sub(self.local_offset[1]);
            }
        }
        let mut layout = backend.layout_with_break_characters(
            configuration,
            &atoms,
            previous.as_ref(),
            &self.break_characters,
        )?;
        for entry in &mut layout.entries {
            entry.position[0] = entry.position[0].wrapping_add(self.local_offset[0]);
            entry.position[1] = entry.position[1].wrapping_add(self.local_offset[1]);
        }
        layout.cursor[0] = layout.cursor[0].wrapping_add(self.local_offset[0]);
        layout.cursor[1] = layout.cursor[1].wrapping_add(self.local_offset[1]);
        layout.bottom = layout.bottom.wrapping_add(self.local_offset[1]);
        layout.validate(&atoms)?;
        let mut old = BTreeMap::new();
        if let Some(previous) = &self.layout {
            for (index, entry) in previous.entries.iter().enumerate() {
                if let Some(atom) = entry.atom {
                    old.insert(atom, (self.states[index], self.deadlines[index]));
                }
            }
        }
        let mut states = vec![MessageGlyphState::Hidden; layout.entries.len()];
        let mut deadlines = vec![0; layout.entries.len()];
        for (index, entry) in layout.entries.iter().enumerate() {
            if let Some(atom) = entry.atom {
                deadlines[index] = atoms[atom].deadline;
                if let Some(&(state, deadline)) = old.get(&atom) {
                    states[index] = state;
                    deadlines[index] = deadline;
                }
                if let Some(anchor) = entry.display_anchor {
                    deadlines[index] = deadlines[anchor];
                }
            }
        }
        self.atoms = atoms;
        self.identifiers = identifiers;
        self.parser = Some(parser);
        self.layout = Some(layout);
        self.states = states;
        self.deadlines = deadlines;
        self.parsed_bytes = self.source.len();
        self.valid = true;
        Ok(true)
    }

    pub fn remaining(&self, segment_limit: u32) -> Result<u32> {
        if !self.configured() {
            return Err(invalid(
                "remaining query requires a configured message object",
            ));
        }
        if !self.valid {
            return Ok(MESSAGE_UNPREPARED);
        }
        let (end, _) = self.range_end(segment_limit)?;
        let count = (0..end)
            .filter(|&index| {
                self.is_glyph(index) && self.states[index] != MessageGlyphState::Complete
            })
            .count();
        u32::try_from(count).map_err(|_| invalid("message remaining count exceeds DWORD"))
    }
    pub fn remaining_all(&self) -> Result<u32> {
        self.remaining(u32::MAX)
    }
    pub fn remaining_segment(&self) -> Result<u32> {
        self.remaining(self.segment_limit)
    }

    /// Object command469/490 prepares pending source and then queries the
    /// current layout. Unlike remaining queries, this may request font widths.
    /// It does not reveal glyphs, submit pixels, or advance the task clock.
    pub fn line_count(
        &mut self,
        time_scale: Option<u32>,
        backend: &mut impl MessageBackend,
    ) -> Result<u32> {
        self.prepare(time_scale, backend)?;
        self.layout
            .as_ref()
            .map(|layout| layout.line_count)
            .ok_or_else(|| invalid("message line query has no successfully prepared layout"))
    }

    /// A passive cached query; shift deliberately leaves this cursor unchanged
    /// until a subsequent successful prepare regenerates it.
    pub fn cached_cursor(&self) -> Option<[i32; 2]> {
        self.layout.as_ref().map(|layout| layout.cursor)
    }

    /// Unicode command496 measures prepared record geometry, including ruby
    /// and ui's nominal font advance/height, rather than decoded ink bounds.
    /// The original receiver compares wrapping DWORD extents as unsigned.
    pub fn draw_area(
        &mut self,
        time_scale: Option<u32>,
        backend: &mut impl MessageBackend,
    ) -> Result<[u32; 2]> {
        if !self.configured() {
            return Ok([0; 2]);
        }
        self.prepare(time_scale, backend)?;
        let mut area = [0u32; 2];
        for entry in &self.layout.as_ref().unwrap().entries {
            if entry.atom.is_some_and(|index| {
                matches!(
                    self.atoms[index].kind,
                    MessageAtomKind::Glyph { character, .. }
                        if character as u32 > 9 && character != '\u{ffff}'
                )
            }) {
                for axis in 0..2 {
                    area[axis] = area[axis]
                        .max((entry.position[axis] as u32).wrapping_add(entry.size[axis] as u32));
                }
            }
        }
        Ok(area)
    }

    /// Portable receiver for the documented text/control commands. The caller
    /// owns routing, deletion checks and raw-image handle resolution. Command
    ///455/476 (mutable defaults output), 464/485 (image handle) and 497
    /// (CP932 resource filename) remain
    /// explicit host operations instead of treating DWORDs as pointers.
    pub fn text_command(
        &mut self,
        command: u32,
        data: [u32; 3],
        buffer: Option<&[u8]>,
        display: MessagePlaneDisplay,
        elapsed: u32,
        time_scale: Option<u32>,
        backend: &mut impl MessageBackend,
    ) -> Result<MessageCommandResult> {
        let base = match self.encoding {
            MessageEncoding::Cp932 => 455,
            MessageEncoding::Unicode => 476,
        };
        let Some(operation) = command.checked_sub(base) else {
            return Ok(MessageCommandResult::Unhandled);
        };
        match operation {
            1 => {
                if let Some(bytes) = buffer {
                    self.recreate(bytes, time_scale, backend)?;
                }
            }
            2 => {
                if self.configured() {
                    if let Some(bytes) = buffer {
                        self.reconfigure(bytes, time_scale, backend)?;
                        self.start(self.segment_limit, display, elapsed, time_scale, backend)?;
                    }
                }
            }
            3 => {
                if let Some(bytes) = buffer {
                    let bytes = bytes.split(|&byte| byte == 0).next().unwrap_or_default();
                    self.append(bytes)?;
                }
            }
            4 => self.clear_with_backend(backend)?,
            5 => return self.remaining_all().map(MessageCommandResult::Value),
            6 => {
                self.force_complete(display, time_scale, backend)?;
            }
            7 => self.append_separator()?,
            8 => {
                self.start(data[1], display, elapsed, time_scale, backend)?;
            }
            10 => self.map_user_glyph(data[0], data[1], backend)?,
            12 => self.set_speed(data[1]),
            13 => {
                if let Some(bytes) = buffer {
                    self.remove_break_characters(data[0], bytes)?;
                }
            }
            14 => {
                return self
                    .line_count(time_scale, backend)
                    .map(MessageCommandResult::Value);
            }
            15 => self.set_shift(data[1], backend)?,
            16 => {
                if let Some(bytes) = buffer {
                    self.add_break_characters(data[0], bytes)?;
                }
            }
            17 => return self.remaining_segment().map(MessageCommandResult::Value),
            19 => self.clear_break_characters(data[0]),
            20 if self.encoding == MessageEncoding::Cp932 => {
                // Command475 queries dimensions and discards them, preserving
                // the output DWORD and its source/output buffers.
                let _ = self.configuration.as_ref().map(MessageConfiguration::size);
            }
            _ => return Ok(MessageCommandResult::Unhandled),
        }
        Ok(MessageCommandResult::Handled)
    }

    pub fn set_speed(&mut self, value: u32) {
        if !self.configured() || self.speed == value {
            return;
        }
        self.speed = value;
        self.time_origin = self.last_scan_time;
        self.deadline_origin = self.reached_deadline;
    }

    pub fn set_user_glyph_source(&mut self, source: Option<Vec<u8>>) -> Result<()> {
        if source.as_ref().is_some_and(|bytes| bytes.len() > MAX_BYTES) {
            return Err(invalid("message user glyph source exceeds 64 MiB"));
        }
        self.user_glyph_source = source;
        // Existing decoded glyphs deliberately survive source replacement.
        Ok(())
    }

    /// Unicode command497's independent retained raw image. Replacing it does
    /// not repaint completed records or change the user-font mappings.
    pub fn set_inline_image_source(&mut self, source: Option<Vec<u8>>) -> Result<()> {
        if self.encoding != MessageEncoding::Unicode {
            return Err(invalid(
                "inline message images require the Unicode receiver",
            ));
        }
        if source.as_ref().is_some_and(|bytes| bytes.len() > MAX_BYTES) {
            return Err(invalid("inline message image source exceeds 64 MiB"));
        }
        self.inline_image_source = source;
        Ok(())
    }

    pub fn inline_image_source(&self) -> Option<&[u8]> {
        self.inline_image_source.as_deref()
    }

    pub fn map_user_glyph(
        &mut self,
        character: u32,
        frame: u32,
        backend: &mut impl MessageBackend,
    ) -> Result<()> {
        let decoded = self
            .user_glyph_source
            .as_deref()
            .map(|bytes| backend.decode_user_glyph(bytes, frame))
            .transpose()?
            .flatten();
        if let Some(glyph) = &decoded {
            glyph.validate()?;
        }
        self.user_glyphs
            .insert(character as u16, MessageUserGlyphMapping { frame, decoded });
        Ok(())
    }

    fn rebuild_user_glyphs(&mut self, backend: &mut impl MessageBackend) -> Result<()> {
        let mut replacements = BTreeMap::new();
        for (&character, mapping) in &self.user_glyphs {
            let decoded = self
                .user_glyph_source
                .as_deref()
                .map(|bytes| backend.decode_user_glyph(bytes, mapping.frame))
                .transpose()?
                .flatten();
            if let Some(glyph) = &decoded {
                glyph.validate()?;
            }
            replacements.insert(
                character,
                MessageUserGlyphMapping {
                    frame: mapping.frame,
                    decoded,
                },
            );
        }
        self.user_glyphs = replacements;
        Ok(())
    }

    /// Returns None for an unmapped character, Some(None) for an empty mapped
    /// glyph. The latter must NOT fall back to the font service.
    pub fn user_glyph(
        &self,
        character: char,
        encoding: MessageEncoding,
    ) -> Option<Option<&MessageUserGlyph>> {
        let code = match encoding {
            MessageEncoding::Unicode => u16::try_from(character as u32).ok()?,
            MessageEncoding::Cp932 => {
                let mut buffer = [0; 4];
                let (bytes, _, errors) =
                    encoding_rs::SHIFT_JIS.encode(character.encode_utf8(&mut buffer));
                if errors {
                    return None;
                }
                match bytes.as_ref() {
                    [single] => u16::from(*single),
                    [high, low] => u16::from_be_bytes([*high, *low]),
                    _ => return None,
                }
            }
        };
        self.user_glyphs
            .get(&code)
            .map(|mapping| mapping.decoded.as_ref())
    }

    pub fn start(
        &mut self,
        limit: u32,
        display: MessagePlaneDisplay,
        elapsed: u32,
        time_scale: Option<u32>,
        backend: &mut impl MessageBackend,
    ) -> Result<Option<MessageRevealUpdate>> {
        if !self.configured() {
            return Ok(None);
        }
        #[cfg(not(target_os = "android"))]
        if self.reveal_trace_enabled() && !self.source.is_empty() {
            eprintln!(
                "message-reveal start id={} gen={} task={} previous_task={} playback={} limit={} display={:?} scale={:?}",
                self.id, self.surface_generation, elapsed, self.last_input_time,
                self.playback_time, limit, display, time_scale,
            );
        }
        let continuing = self.active && self.segment_limit == limit;
        self.segment_limit = limit;
        match display {
            MessagePlaneDisplay::Unreachable => Ok(None),
            MessagePlaneDisplay::Hidden => {
                self.active = false;
                Ok(None)
            }
            MessagePlaneDisplay::Visible => {
                self.prepare(time_scale, backend)?;
                if !continuing {
                    self.last_input_time = elapsed;
                }
                self.active = true;
                // Full startup copy must not refresh complete_snapshot.
                self.advance_display(self.playback_time, MessageCopy::Full, backend)
                    .map(Some)
            }
        }
    }

    pub fn force_complete(
        &mut self,
        display: MessagePlaneDisplay,
        time_scale: Option<u32>,
        backend: &mut impl MessageBackend,
    ) -> Result<Option<MessageRevealUpdate>> {
        if !self.configured() || display == MessagePlaneDisplay::Unreachable {
            return Ok(None);
        }
        self.prepare(time_scale, backend)?;
        let copy = self.ordinary_copy();
        let update = self.advance_display(u32::MAX, copy, backend)?;
        self.active = false;
        Ok(Some(update))
    }

    /// Called in the message task BODY, after its clock and children update.
    /// Rendering uses the saved playback time, then adds this round's input
    /// delta. Paused task clocks still enter this method.
    pub fn frame_update(
        &mut self,
        elapsed: u32,
        time_scale: Option<u32>,
        backend: &mut impl MessageBackend,
    ) -> Result<Option<MessageRevealUpdate>> {
        if !self.active || !self.configured() {
            return Ok(None);
        }
        #[cfg(not(target_os = "android"))]
        if self.reveal_trace_enabled() && !self.source.is_empty() {
            eprintln!(
                "message-reveal tick id={} gen={} task={} previous_task={} delta={} playback={}",
                self.id, self.surface_generation, elapsed, self.last_input_time,
                elapsed.wrapping_sub(self.last_input_time), self.playback_time,
            );
        }
        self.prepare(time_scale, backend)?;
        let update = self.advance_display(self.playback_time, self.ordinary_copy(), backend)?;
        // Original body 0x187186..0x187192 stores !advance.complete.
        // Its play time still receives this round's delta after completion.
        self.active = !update.complete;
        self.playback_time = self
            .playback_time
            .wrapping_add(elapsed.wrapping_sub(self.last_input_time));
        self.last_input_time = elapsed;
        Ok(Some(update))
    }

    #[cfg(not(target_os = "android"))]
    pub fn reveal_trace_enabled(&self) -> bool {
        static TARGET: std::sync::OnceLock<Option<RevealTraceTarget>> = std::sync::OnceLock::new();
        match TARGET.get_or_init(|| match std::env::var("CS2_MESSAGE_REVEAL_TRACE").ok()?.trim() {
            "fes" => Some(RevealTraceTarget::Fes),
            id => id.parse().ok().map(RevealTraceTarget::Object),
        }) {
            Some(RevealTraceTarget::Fes) => self.id < 0,
            Some(RevealTraceTarget::Object(id)) => self.id == *id,
            None => false,
        }
    }

    fn ordinary_copy(&self) -> MessageCopy {
        if self
            .configuration
            .as_ref()
            .is_some_and(MessageConfiguration::ordinary_submit)
        {
            MessageCopy::Dirty
        } else {
            MessageCopy::None
        }
    }

    fn is_glyph(&self, index: usize) -> bool {
        matches!(
            self.layout
                .as_ref()
                .and_then(|layout| layout.entries.get(index))
                .and_then(|entry| entry.atom)
                .and_then(|atom| self.atoms.get(atom))
                .map(|atom| &atom.kind),
            Some(MessageAtomKind::Glyph { .. })
        )
    }

    fn range_end(&self, limit: u32) -> Result<(usize, bool)> {
        let layout = self
            .layout
            .as_ref()
            .ok_or_else(|| invalid("message layout is not prepared"))?;
        let mut separators = 0i32;
        for (index, entry) in layout.entries.iter().enumerate() {
            if matches!(
                entry
                    .atom
                    .and_then(|atom| self.atoms.get(atom))
                    .map(|atom| &atom.kind),
                Some(MessageAtomKind::Separator)
            ) {
                if limit != u32::MAX && separators >= limit as i32 {
                    return Ok((index, true));
                }
                separators = separators.wrapping_add(1);
            }
        }
        Ok((layout.entries.len(), false))
    }

    pub fn advance_display(
        &mut self,
        now: u32,
        copy: MessageCopy,
        backend: &mut impl MessageBackend,
    ) -> Result<MessageRevealUpdate> {
        if !self.valid {
            return Err(invalid("message advance requires successful preparation"));
        }
        let configuration = self.configuration.as_ref().unwrap();
        let (end, stopped_at_separator) = self.range_end(self.segment_limit)?;
        let force = now == u32::MAX;
        let gradual = configuration.reveal_width() != 0;
        let elapsed = now.wrapping_sub(self.time_origin).wrapping_mul(self.speed) / 10;
        let phase = if gradual {
            Some(if (now as i32) < 0 || configuration.value(0x74) == 0 {
                i32::MAX
            } else {
                (now.wrapping_mul(configuration.value(0x24)).wrapping_mul(2)
                    / configuration.value(0x74)) as i32
            })
        } else {
            None
        };
        let mut prepare = Vec::new();
        let mut process = Vec::new();
        let mut complete = true;
        let mut reached = self.reached_deadline;
        for index in 0..end {
            if !self.is_glyph(index) {
                continue;
            }
            if gradual {
                if self.states[index] != MessageGlyphState::Complete {
                    process.push(index);
                    if self.states[index] == MessageGlyphState::Hidden {
                        prepare.push(index);
                    }
                }
            } else {
                let due =
                    force || self.deadlines[index].wrapping_sub(self.deadline_origin) <= elapsed;
                complete &= due;
                if due {
                    reached = reached.max(self.deadlines[index]);
                    if self.states[index] != MessageGlyphState::Complete {
                        prepare.push(index);
                        process.push(index);
                    }
                }
            }
        }
        #[cfg(not(target_os = "android"))]
        if self.reveal_trace_enabled() && !self.source.is_empty() {
            let first_pending = (0..end)
                .find(|&index| {
                    self.states[index] != MessageGlyphState::Complete && self.is_glyph(index)
                })
                .map(|index| (index, self.deadlines[index]));
            eprintln!(
                "message-reveal scan id={} gen={} playback={} effective={} speed={} time_origin={} deadline_origin={} char_delay={} limit={} end={} separator={} first_pending={:?} prepare={} process={} phase={:?} force={} copy={:?}",
                self.id, self.surface_generation, now, elapsed, self.speed, self.time_origin,
                self.deadline_origin, configuration.value(0x74), self.segment_limit, end,
                stopped_at_separator, first_pending, prepare.len(), process.len(), phase, force, copy,
            );
        }
        let painted = backend.paint(MessageRenderRequest {
            object: self,
            configuration,
            layout: self.layout.as_ref().unwrap(),
            prepare: &prepare,
            process: &process,
            reveal_phase: phase,
            force,
            copy,
            surface_generation: self.surface_generation,
        })?;
        validate_paint(&painted, &prepare, &process, &self.states, gradual)?;
        if painted.copied_dirty && copy != MessageCopy::Dirty {
            return Err(invalid(
                "message backend reported dirty copy for a different copy mode",
            ));
        }
        for &index in &painted.prepared {
            self.states[index] = if gradual {
                MessageGlyphState::Revealing
            } else {
                MessageGlyphState::Complete
            };
        }
        for &index in &painted.completed {
            self.states[index] = MessageGlyphState::Complete;
        }
        if gradual {
            complete = (0..end).all(|index| {
                !self.is_glyph(index) || self.states[index] == MessageGlyphState::Complete
            });
        }
        self.reached_deadline = reached;
        if !stopped_at_separator {
            self.last_scan_time = now;
        }
        if painted.failure.is_some() {
            complete = false;
        }
        self.last_complete = complete;
        if painted.copied_dirty {
            // Snapshot concerns the whole page, even when this advance stopped
            // at a separator and its current range was already complete.
            self.complete_snapshot = self.remaining_all()? == 0;
        }
        if let Some(failure) = painted.failure {
            return Err(ScriptError::InvalidStructuredText {
                offset: failure.offset,
                message: failure.message,
            });
        }
        Ok(MessageRevealUpdate {
            newly_prepared: painted.prepared,
            newly_complete: painted.completed,
            complete,
        })
    }
}

#[cfg(not(target_os = "android"))]
enum RevealTraceTarget {
    Fes,
    Object(i32),
}

#[cfg(test)]
mod reveal_tests {
    use super::*;

    #[derive(Default)]
    struct RecordingBackend {
        copy: Option<MessageCopy>,
    }

    impl MessageBackend for RecordingBackend {
        fn layout(
            &mut self,
            _configuration: &MessageConfiguration,
            atoms: &[MessageAtom],
            _previous: Option<&MessageLayout>,
        ) -> Result<MessageLayout> {
            Ok(MessageLayout {
                entries: (0..atoms.len())
                    .map(|index| MessageLayoutEntry {
                        atom: Some(index),
                        position: [index as i32 * 10, 0],
                        size: [10, 10],
                        line: 0,
                        display_anchor: None,
                    })
                    .collect(),
                line_count: u32::from(!atoms.is_empty()),
                ..Default::default()
            })
        }

        fn decode_user_glyph(&mut self, _: &[u8], _: u32) -> Result<Option<MessageUserGlyph>> {
            unreachable!()
        }

        fn paint(&mut self, request: MessageRenderRequest<'_>) -> Result<MessagePaintResult> {
            self.copy = Some(request.copy);
            Ok(MessagePaintResult {
                prepared: request.prepare.to_vec(),
                completed: request.process.to_vec(),
                copied_dirty: request.copy == MessageCopy::Dirty && !request.process.is_empty(),
                failure: None,
            })
        }
    }

    fn timed_message(backend: &mut RecordingBackend, scale: Option<u32>) -> MessageObject {
        let encoding = MessageEncoding::Cp932;
        let configuration = MessageDefaults {
            font_name: Vec::new(),
            bold: false,
            shadow: 0,
            outline: 0,
            glyph_parameter: None,
            gradual: false,
        }
        .configuration(encoding)
        .unwrap();
        let mut bytes = configuration.bytes().to_vec();
        bytes[0x74..0x78].copy_from_slice(&7u32.to_le_bytes());
        let submit = configuration.canvas_fields() + 8;
        bytes[submit..submit + 4].copy_from_slice(&1u32.to_le_bytes());
        let mut message = MessageObject::new(-1, encoding);
        message.recreate(&bytes, scale, backend).unwrap();
        message.append_decoded("\u{300c}AB".as_bytes(), "\u{300c}AB").unwrap();
        message.append_separator().unwrap();
        message.append(b"C").unwrap();
        message
    }

    #[test]
    fn message_reveal_ticks_cross_character_deadlines_without_input() {
        let mut backend = RecordingBackend::default();
        let scale = Some(5);
        let mut message = timed_message(&mut backend, scale);
        let start = message
            .start(0, MessagePlaneDisplay::Visible, 100, scale, &mut backend)
            .unwrap()
            .unwrap();
        assert_eq!(message.deadlines()[..3], [0, 3, 7]);
        assert_eq!(start.newly_complete, [0]);
        assert_eq!(backend.copy, Some(MessageCopy::Full));
        for (task_time, playback, expected) in [
            (100, 0, &[][..]),
            (103, 3, &[][..]),
            (104, 4, &[1][..]),
            (107, 7, &[][..]),
            (108, 8, &[2][..]),
        ] {
            let update = message.frame_update(task_time, scale, &mut backend).unwrap().unwrap();
            assert_eq!(message.playback_time(), playback);
            assert_eq!(update.newly_complete, expected);
            assert_eq!(backend.copy, Some(MessageCopy::Dirty));
        }
        assert!(!message.active());
        assert_eq!(message.remaining_segment().unwrap(), 0);
        assert_eq!(message.remaining_all().unwrap(), 1);
        assert!(message.frame_update(109, scale, &mut backend).unwrap().is_none());
    }

    #[test]
    fn repeated_message_start_preserves_character_deadlines_and_task_delta() {
        let mut backend = RecordingBackend::default();
        let scale = Some(5);
        let mut message = timed_message(&mut backend, scale);
        for (task, playback, expected) in [
            (952, 0, &[0][..]),
            (953, 1, &[][..]),
            (953, 1, &[][..]),
            (954, 2, &[][..]),
            (955, 3, &[][..]),
            (956, 4, &[1][..]),
            (957, 5, &[][..]),
            (958, 6, &[][..]),
            (959, 7, &[][..]),
            (960, 8, &[2][..]),
        ] {
            let start = message.start(0, MessagePlaneDisplay::Visible, task, scale, &mut backend)
                .unwrap().unwrap();
            let tick = message.frame_update(task, scale, &mut backend).unwrap().unwrap();
            assert_eq!(message.playback_time(), playback);
            assert_eq!(start.newly_complete, expected);
            assert!(tick.newly_complete.is_empty());
        }
        assert!(!message.active());
        assert_eq!(message.remaining_segment().unwrap(), 0);
        assert_eq!(message.remaining_all().unwrap(), 1);

        message.start(0, MessagePlaneDisplay::Visible, 1000, scale, &mut backend).unwrap();
        assert_eq!(message.last_input_time, 1000);
        message.start(1, MessagePlaneDisplay::Visible, 1001, scale, &mut backend).unwrap();
        message.frame_update(1001, scale, &mut backend).unwrap();
        assert_eq!(message.playback_time(), 8);
    }
}

fn validate_paint(
    painted: &MessagePaintResult,
    prepare: &[usize],
    process: &[usize],
    states: &[MessageGlyphState],
    gradual: bool,
) -> Result<()> {
    for (indices, allowed) in [(&painted.prepared, prepare), (&painted.completed, process)] {
        let mut last = None;
        for &index in indices {
            if last.is_some_and(|previous| index <= previous)
                || allowed.binary_search(&index).is_err()
            {
                return Err(invalid(
                    "message painter returned unordered or out-of-range glyph entries",
                ));
            }
            last = Some(index);
        }
    }
    if painted.failure.is_none()
        && !gradual
        && (painted.prepared != prepare || painted.completed != process)
    {
        return Err(invalid(
            "ordinary message painter did not complete the requested glyphs",
        ));
    }
    if painted.failure.is_none() && gradual && painted.prepared != prepare {
        return Err(invalid(
            "gradual message painter did not pregenerate the requested glyphs",
        ));
    }
    if painted.completed.iter().any(|&index| {
        states[index] == MessageGlyphState::Hidden
            && painted.prepared.binary_search(&index).is_err()
    }) {
        return Err(invalid(
            "message painter completed a glyph that has not been prepared",
        ));
    }
    Ok(())
}

/// Extracts ordinary392/1049's body without preparing a display or requesting
/// fonts. CP932 units retain their numeric source bytes, including invalid
/// single bytes; Unicode follows the old decoder's positive byte strides.
/// The returned buffer excludes its final terminator but may contain zeros
/// introduced by a numeric character in the CP932 output path.
pub fn extract_message_body(input: &[u8], encoding: MessageEncoding) -> Result<Vec<u8>> {
    if input.len() > MAX_BYTES {
        return Err(invalid("message body source exceeds 64 MiB"));
    }
    let end = input
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(input.len());
    let input = &input[..end];
    let mut source = String::new();
    let mut offsets = BTreeMap::new();
    let mut cursor = 0usize;
    while cursor < input.len() {
        if offsets.len() >= MAX_ENTRIES {
            return Err(invalid("message body character limit exceeded"));
        }
        let start = cursor;
        let first = input[cursor];
        let character = match encoding {
            MessageEncoding::Cp932 => {
                if first == 0xff {
                    cursor += 1;
                    '\u{ffff}'
                } else if message_cp932_pair(input, cursor) {
                    let value = u16::from_be_bytes([first, input[cursor + 1]]);
                    cursor += 2;
                    // Valid lead-byte ranges never generate a surrogate.
                    char::from_u32(u32::from(value)).unwrap()
                } else {
                    cursor += 1;
                    char::from(first)
                }
            }
            MessageEncoding::Unicode => {
                if first == 0xff
                    && (input.get(cursor + 1) == Some(&0xff) || cursor + 1 == input.len())
                {
                    cursor += if input.get(cursor + 1) == Some(&0xff) {
                        2
                    } else {
                        1
                    };
                    '\u{ffff}'
                } else {
                    let leading = first.leading_ones();
                    if leading == 1 {
                        break;
                    }
                    let width = if leading == 0 || leading >= 6 {
                        1
                    } else {
                        leading as usize
                    };
                    // The old stride checks intermediate continuation bytes,
                    // but leaves the final byte for its conversion helper.
                    if (1..width.saturating_sub(1)).any(|index| {
                        input
                            .get(cursor + index)
                            .is_none_or(|byte| byte & 0xc0 != 0x80)
                    }) {
                        break;
                    }
                    let unit = input.get(cursor..cursor + width).ok_or_else(|| {
                        ScriptError::InvalidStructuredText {
                            offset: start,
                            message: "message body UTF-8 unit is truncated",
                        }
                    })?;
                    cursor += width;
                    let decoded = String::from_utf8_lossy(unit);
                    let character = decoded.chars().next().unwrap_or('\u{fffd}');
                    // Conversion retained the first UTF-16 unit, whose
                    // isolated high surrogate exports as replacement UTF-8.
                    if character as u32 > 0xffff {
                        '\u{fffd}'
                    } else {
                        character
                    }
                }
            }
        };
        offsets.insert(source.len(), start);
        source.push(character);
    }
    offsets.insert(source.len(), cursor);
    let configuration = MessageDefaults {
        font_name: Vec::new(),
        bold: false,
        shadow: 0,
        outline: 0,
        glyph_parameter: None,
        gradual: false,
    }
    .configuration(encoding)?;
    let mut atoms = Vec::new();
    let mut state = ParserState::new(&configuration);
    parse_append(
        &source,
        0,
        encoding,
        &configuration,
        Some(0),
        &mut state,
        &mut atoms,
        &mut BTreeMap::new(),
    )
    .map_err(|error| match error {
        ScriptError::InvalidStructuredText { offset, message } => {
            ScriptError::InvalidStructuredText {
                offset: offsets
                    .range(..=offset)
                    .next_back()
                    .map_or(0, |(_, &raw)| raw),
                message,
            }
        }
        error => error,
    })?;
    let mut output = Vec::new();
    for atom in atoms {
        let MessageAtomKind::Glyph {
            character,
            encoding: glyph_encoding,
            ruby: false,
            ..
        } = atom.kind
        else {
            continue;
        };
        let mut value = character as u32;
        if value <= 9 || value == 0xffff {
            continue;
        }
        match encoding {
            MessageEncoding::Cp932 => {
                if glyph_encoding == MessageEncoding::Cp932 && character == '\u{3000}' {
                    value = 0x8140; // The parser's generated CP932 \s.
                }
                if value & 0xff00 != 0 {
                    output.push((value >> 8) as u8);
                }
                output.push(value as u8);
            }
            MessageEncoding::Unicode => {
                let mut unit = [0; 4];
                output.extend_from_slice(character.encode_utf8(&mut unit).as_bytes());
            }
        }
        if output.len() >= MAX_BYTES {
            return Err(invalid(
                "message body output exceeds its terminator capacity",
            ));
        }
    }
    Ok(output)
}

fn message_cp932_pair(bytes: &[u8], cursor: usize) -> bool {
    matches!(bytes[cursor], 0x81..=0x9f | 0xe0..=0xef)
        && bytes
            .get(cursor + 1)
            .is_some_and(|byte| matches!(byte, 0x40..=0x7e | 0x80..=0xfc))
}

/// Independent markup parser. Each successful prepare closes unclosed groups;
/// the style and accumulated timing units persist, while pending appends parse
/// together. Failed parsing never commits partially changed page state.
fn parse_append(
    source: &str,
    start: usize,
    encoding: MessageEncoding,
    configuration: &MessageConfiguration,
    time_scale: Option<u32>,
    state: &mut ParserState,
    atoms: &mut Vec<MessageAtom>,
    identifiers: &mut BTreeMap<i32, u32>,
) -> Result<()> {
    let units: Vec<_> = source[start..]
        .char_indices()
        .map(|(offset, character)| (start + offset, character))
        .collect();
    let mut parser = MarkupParser {
        units: &units,
        source_end: source.len(),
        cursor: 0,
        encoding,
        configuration,
        time_scale,
        state,
        atoms,
        identifiers,
        group: None,
        ruby: false,
    };
    parser.run()
}

struct MarkupParser<'a> {
    units: &'a [(usize, char)],
    source_end: usize,
    cursor: usize,
    encoding: MessageEncoding,
    configuration: &'a MessageConfiguration,
    time_scale: Option<u32>,
    state: &'a mut ParserState,
    atoms: &'a mut Vec<MessageAtom>,
    identifiers: &'a mut BTreeMap<i32, u32>,
    group: Option<u32>,
    ruby: bool,
}

impl MarkupParser<'_> {
    fn peek(&self) -> Option<char> {
        self.units.get(self.cursor).map(|unit| unit.1)
    }
    fn lower(&self) -> Option<char> {
        self.peek().map(|c| c.to_ascii_lowercase())
    }
    fn take(&mut self) -> Option<char> {
        let value = self.peek()?;
        self.cursor += 1;
        Some(value)
    }
    fn offset(&self) -> usize {
        self.units
            .get(self.cursor)
            .map_or(self.source_end, |unit| unit.0)
    }
    fn error(&self, offset: usize, message: &'static str) -> ScriptError {
        ScriptError::InvalidStructuredText { offset, message }
    }
    fn number(&mut self) -> Option<u32> {
        if !self.peek().is_some_and(|c| c.is_ascii_digit()) {
            return None;
        }
        let mut radix = 10;
        if self.peek() == Some('0')
            && self
                .units
                .get(self.cursor + 1)
                .is_some_and(|unit| matches!(unit.1, 'x' | 'X'))
        {
            self.cursor += 2;
            radix = 16;
        }
        let mut result = 0u32;
        while let Some(value) = self.peek().and_then(|c| c.to_digit(radix)) {
            result = result.wrapping_mul(radix).wrapping_add(value);
            self.cursor += 1;
        }
        if self.peek() == Some(';') {
            self.cursor += 1;
        }
        Some(result)
    }
    fn emit(&mut self, kind: MessageAtomKind, start: usize) -> Result<()> {
        if self.atoms.len() >= MAX_ENTRIES {
            return Err(self.error(start, "message atom limit exceeded"));
        }
        let deadline = self.time_scale.map_or(0, |scale| {
            self.state.accumulated_units.wrapping_mul(scale) / 10
        });
        self.atoms.push(MessageAtom {
            kind,
            source: start..self.offset(),
            deadline,
            font_depth: self.state.font_depth,
            inline_image: None,
        });
        Ok(())
    }
    fn glyph(&mut self, character: char, encoding: MessageEncoding, start: usize) -> Result<()> {
        if character as u32 > 0xffff {
            return Err(self.error(start, "non-BMP message display counting is not recovered"));
        }
        if character == '\0' {
            return Err(self.error(start, "message contains a zero scalar"));
        }
        if character == '\u{ffff}' {
            return self.emit(MessageAtomKind::Separator, start);
        }
        self.emit(
            MessageAtomKind::Glyph {
                character,
                encoding,
                style: self.state.style,
                group: self.group,
                ruby: self.ruby,
            },
            start,
        )?;
        if self.ruby {
            self.atoms.last_mut().unwrap().deadline = 0;
        }
        if self.time_scale.is_some() {
            let skip = self.configuration.value(0x7c) != 0 && matches!(character, ' ' | '\u{3000}');
            if !skip && !self.ruby {
                let period = character == '\u{3002}'
                    || (self.encoding == MessageEncoding::Unicode && character == '.');
                self.state.accumulated_units = self
                    .state
                    .accumulated_units
                    .wrapping_add(self.configuration.value(if period { 0x78 } else { 0x74 }));
            }
        }
        Ok(())
    }
    fn control(&mut self, value: u32, start: usize) -> Result<()> {
        // Control six/eight are outside the surrounding no-break/ruby group.
        self.emit(MessageAtomKind::Control(value), start)
    }
    fn run(&mut self) -> Result<()> {
        while self.cursor < self.units.len() {
            let start = self.offset();
            let character = self.take().unwrap();
            if self.configuration.value(0x64) == 0 && self.configuration.value(0x68) == 0 {
                // CP932 original parser 0x1eaac0 gates backslash recognition
                // on these two raw flags. Its outer 0x1ed460 parser retains a
                // special '[' record-zero/newline path even with escapes off.
                match character {
                    '[' | '\n' => self.emit(MessageAtomKind::Newline, start)?,
                    '\u{6}' | '\u{8}' | '\u{9}' => self.control(character as u32, start)?,
                    _ => self.glyph(character, self.encoding, start)?,
                }
                continue;
            }
            match character {
                '[' if self.group.is_none() => {
                    self.group = Some(self.state.next_group);
                    self.ruby = false;
                }
                ']' if self.group.is_some() => {
                    self.group = None;
                    self.ruby = false;
                    self.state.next_group = self.state.next_group.wrapping_add(1);
                }
                '/' if self.group.is_some() && !self.ruby => self.ruby = true,
                '\\' => self.escape(start)?,
                '\n' => self.emit(MessageAtomKind::Newline, start)?,
                '\u{6}' | '\u{8}' | '\u{9}' => self.control(character as u32, start)?,
                _ => self.glyph(character, self.encoding, start)?,
            }
        }
        if self.group.is_some() {
            self.state.next_group = self.state.next_group.wrapping_add(1);
        }
        Ok(())
    }
    fn escape(&mut self, start: usize) -> Result<()> {
        let Some(marker) = self.take() else {
            return self.emit(MessageAtomKind::Newline, start);
        };
        match marker.to_ascii_lowercase() {
            'n' => self.emit(MessageAtomKind::Newline, start),
            'r' => self.control(8, start),
            '@' => self.emit(MessageAtomKind::Continue, start),
            's' => {
                let saved_group = self.group;
                let saved_ruby = self.ruby;
                self.group = None;
                self.ruby = false;
                self.glyph(
                    if self.encoding == MessageEncoding::Cp932 {
                        '\u{3000}'
                    } else {
                        '\u{8140}'
                    },
                    self.encoding,
                    start,
                )?;
                self.emit(MessageAtomKind::Newline, start)?;
                self.group = saved_group;
                self.ruby = saved_ruby;
                Ok(())
            }
            'i' if self.lower() == Some('d') => {
                self.cursor += 1;
                let key = self.number().unwrap_or(0) as i32;
                let value = if self.peek() == Some(':') {
                    self.cursor += 1;
                    self.number().unwrap_or(0)
                } else {
                    // Baseline replays the final consumed character at EOF.
                    if self.cursor == self.units.len() {
                        self.cursor = self.cursor.saturating_sub(1);
                    }
                    0
                };
                self.identifiers.entry(key).or_insert(value);
                Ok(())
            }
            'i' => self.control(6, start),
            'u' if self.encoding == MessageEncoding::Unicode => {
                if self.lower() == Some('i') {
                    self.cursor += 1;
                    let frame = self.number().unwrap_or(0);
                    self.emit(
                        MessageAtomKind::Glyph {
                            character: '\n',
                            encoding: MessageEncoding::Unicode,
                            style: self.state.style,
                            group: None,
                            ruby: false,
                        },
                        start,
                    )?;
                    self.atoms.last_mut().unwrap().inline_image = Some(frame);
                    if self.time_scale.is_some() {
                        self.state.accumulated_units = self
                            .state
                            .accumulated_units
                            .wrapping_add(self.configuration.value(0x74));
                    }
                    return Ok(());
                }
                self.control(6, start)
            }
            'p' => {
                let alignment = match self.lower() {
                    Some('l') => {
                        self.cursor += 1;
                        0
                    }
                    Some('c') => {
                        self.cursor += 1;
                        1
                    }
                    Some('r') => {
                        self.cursor += 1;
                        2
                    }
                    _ => 0,
                };
                self.emit(MessageAtomKind::Align(alignment), start)
            }
            'x' | 'y' => {
                if let Some(value) = self.number() {
                    self.emit(
                        if marker.eq_ignore_ascii_case(&'x') {
                            MessageAtomKind::RawX(value)
                        } else {
                            MessageAtomKind::RawY(value)
                        },
                        start,
                    )?;
                }
                Ok(())
            }
            'w' => {
                match self.lower() {
                    Some('r') => {
                        self.cursor += 1;
                        self.state.accumulated_units = 0;
                        self.emit(MessageAtomKind::ResetWait, start)?;
                    }
                    Some('f') => {
                        self.cursor += 1;
                        if let Some(value) = self.number() {
                            self.state.accumulated_units =
                                self.state.accumulated_units.wrapping_add(value);
                        }
                    }
                    _ => {
                        if let Some(value) = self.number() {
                            let scale = self.time_scale.ok_or_else(|| {
                                self.error(
                                    start,
                                    "message character wait requires a global time context",
                                )
                            })?;
                            if scale as i32 > 0 {
                                self.state.accumulated_units = self
                                    .configuration
                                    .value(0x74)
                                    .wrapping_mul(value)
                                    .wrapping_mul(10)
                                    / scale;
                            }
                        }
                    }
                }
                Ok(())
            }
            'c' => {
                self.color();
                Ok(())
            }
            'f' => self.font_size(start),
            't' => {
                self.state.style.bold = false;
                self.state.style.italic = false;
                let mut replay = marker;
                loop {
                    match self.lower() {
                        Some('b') => self.state.style.bold = true,
                        Some('i') => self.state.style.italic = true,
                        Some(';') => {
                            self.cursor += 1;
                            return Ok(());
                        }
                        _ => break,
                    }
                    replay = self.take().unwrap();
                }
                if self.cursor == self.units.len() {
                    self.glyph(replay, self.encoding, start)?;
                }
                Ok(())
            }
            '$' => {
                let begin = self.cursor;
                while self.peek().is_some_and(|c| c != ';') {
                    self.cursor += 1;
                }
                let end = self.cursor;
                if self.peek() == Some(';') {
                    self.cursor += 1;
                }
                if begin == end {
                    return Ok(());
                }
                let text: String = self.units[begin..end].iter().map(|unit| unit.1).collect();
                let value = integer_prefix(&text) as u16;
                if value == 0 {
                    return self.emit(MessageAtomKind::Newline, start);
                }
                let character = char::from_u32(u32::from(value)).ok_or_else(|| {
                    self.error(start, "surrogate message character is not recovered")
                })?;
                self.glyph(character, MessageEncoding::Unicode, start)
            }
            'z' => {
                let begin = self.cursor;
                while self.peek().is_some_and(|character| character != ';') {
                    self.cursor += 1;
                }
                let end = self.cursor;
                if self.peek() == Some(';') {
                    self.cursor += 1;
                }
                if begin == end {
                    self.state.font_depth = self.state.font_depth.saturating_sub(1);
                } else {
                    if self.state.font_overrides.len() >= MAX_ENTRIES {
                        return Err(
                            self.error(start, "message z font-name collection exceeds its limit")
                        );
                    }
                    let name: String = self.units[begin..end].iter().map(|unit| unit.1).collect();
                    let wide: Vec<_> = name.encode_utf16().collect();
                    if self
                        .state
                        .font_overrides
                        .iter()
                        .map(Vec::len)
                        .try_fold(wide.len(), |sum, length| sum.checked_add(length))
                        .is_none_or(|length| length > MAX_BYTES / 2)
                    {
                        return Err(
                            self.error(start, "message z font-name storage exceeds its limit")
                        );
                    }
                    self.state.font_overrides.push(wide);
                    self.state.font_depth = self.state.font_depth.wrapping_add(1);
                }
                Ok(())
            }
            _ => self.glyph(marker, self.encoding, start),
        }
    }
    fn consume_restored_ruby_suffix(&mut self) {
        if self.take() == Some(';') {
            self.take();
        }
    }
    fn color(&mut self) {
        let channel = match self.lower() {
            Some('r') | Some('s') | Some('d') => self.take().unwrap().to_ascii_lowercase(),
            _ => 'c',
        };
        let value = self.number();
        match channel {
            'r' => {
                self.state.style.ruby_color = value.unwrap_or(self.configuration.value(0x18));
                if value.is_none() {
                    self.consume_restored_ruby_suffix();
                }
            }
            's' => self.state.style.shadow_color = value.unwrap_or(self.configuration.value(0x14)),
            'd' => self.state.restore_color = value.unwrap_or(self.configuration.value(0x10)),
            _ => {
                self.state.style.text_color = value.unwrap_or(self.state.restore_color);
                if value.is_none() && self.peek() == Some(';') {
                    self.cursor += 1;
                }
            }
        }
    }
    fn font_size(&mut self, start: usize) -> Result<()> {
        if let Some(size @ ('s' | 'n' | 'l')) = self.lower() {
            self.cursor += 1;
            let index = match size {
                's' if self.lower() == Some('s') => {
                    self.cursor += 1;
                    0
                }
                's' => 1,
                'n' => 2,
                'l' if self.lower() == Some('l') => {
                    self.cursor += 1;
                    4
                }
                _ => 3,
            };
            self.state.style.text_size = self.configuration.text_sizes()[index];
            self.state.style.ruby_size = self.configuration.ruby_sizes()[index];
            if size != 'n' && self.peek() == Some(';') {
                self.cursor += 1;
            }
            return Ok(());
        }
        match self.lower() {
            Some('r') => {
                self.cursor += 1;
                let value = self.number();
                self.state.style.ruby_size = value.unwrap_or(self.configuration.value(0x38));
                if value.is_none() {
                    self.consume_restored_ruby_suffix();
                }
            }
            Some(sign @ ('+' | '-')) => {
                self.cursor += 1;
                self.state.style.text_size = match self.number() {
                    Some(value) if sign == '+' => self.state.style.text_size.wrapping_add(value),
                    Some(value) => self.state.style.text_size.wrapping_sub(value),
                    None => self.configuration.value(0x24),
                };
            }
            _ => {
                let value = self.number();
                self.state.style.text_size = value.unwrap_or(self.configuration.value(0x24));
                if value.is_none() && self.peek() == Some(';') {
                    self.cursor += 1;
                }
                if value.is_none()
                    && self.cursor == self.units.len()
                    && self
                        .units
                        .last()
                        .is_some_and(|unit| unit.1.eq_ignore_ascii_case(&'f'))
                {
                    self.glyph(self.units.last().unwrap().1, self.encoding, start)?;
                }
            }
        }
        Ok(())
    }
}

fn integer_prefix(text: &str) -> u32 {
    let bytes = text.as_bytes();
    let mut cursor = 0usize;
    let negative = bytes.first() == Some(&b'-');
    if matches!(bytes.first(), Some(b'+' | b'-')) {
        cursor += 1;
    }
    let hexadecimal =
        bytes.get(cursor) == Some(&b'0') && matches!(bytes.get(cursor + 1), Some(b'x' | b'X'));
    if hexadecimal {
        cursor += 2;
    }
    let radix = if hexadecimal { 16 } else { 10 };
    let mut value = 0u32;
    while let Some(digit) = bytes
        .get(cursor)
        .and_then(|&byte| (byte as char).to_digit(radix))
    {
        value = value.wrapping_mul(radix).wrapping_add(digit);
        cursor += 1;
    }
    if !hexadecimal && bytes.get(cursor) == Some(&b'.') {
        return 0;
    }
    if negative {
        value.wrapping_neg()
    } else {
        value
    }
}

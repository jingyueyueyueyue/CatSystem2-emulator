//! Ordinary message-native adapter. Owned by DataServices, not LoggingHost.
//! Independently implemented from the documented command and message contracts.
//!
//! Wiring requires a real task router (including target zero), SceneNode
//! validation, a font/layout backend and a pixel submitter. See the E report.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use cs2_renderer::{AnimationFrameSelection, HgContainer, HgTagKind, RgbaFrame, inspect_hg};
use cs2_runtime::message_object::{
    ButtonSource, DeviceKeys, InputHitMask, InputRouteReceipt, Keys, MESSAGE_CONFIGURATION_SIZE,
    MESSAGE_OBJECT_TYPE, MessageBackend, MessageCommandResult, MessageDefaults, MessageEncoding,
    MessageObject, MessageObjectSnapshot, MessagePlaneDisplay, MessageRevealUpdate,
    MessageTextInputState, MessageUserGlyph, PointerMoveState, WheelButtons, extract_message_body,
};
use cs2_script::{NativeReturn, Result, ScriptError, Vm};

use super::{SceneNode, expand_kcs_format};

pub(super) const SUPPORTED_SLOTS: [u32; 13] = [
    611, 612, 613, 614, 615, 616, 617, 618, 619, 620, 621, 695, 884,
];

const MAX_BYTES: usize = 64 * 1024 * 1024;
const MAX_OBJECTS: usize = 65_536;

const BODY_NATIVES: &[(u32, usize, MessageEncoding)] = &[
    (392, 2, MessageEncoding::Cp932),
    (1049, 2, MessageEncoding::Unicode),
];

pub(super) fn is_body_native(index: u32) -> bool {
    BODY_NATIVES.iter().any(|spec| spec.0 == index)
}
pub(super) fn is_display_native(index: u32) -> bool {
    matches!(index, 69 | 70 | 92)
}

/// Pure VM-memory wrapper. Invalid destination starts return zero only after
/// source formatting, matching the identified wrappers' operation order.
pub(super) fn call_message_body_native(
    index: u32,
    arguments: &[u8],
    vm: &mut Vm,
) -> Result<NativeReturn> {
    let &(_, words, encoding) = BODY_NATIVES
        .iter()
        .find(|spec| spec.0 == index)
        .ok_or(ScriptError::UnimplementedNativeCall { index })?;
    if arguments.len() != words * 4 {
        return Err(invalid("message body native requires exactly two DWORDs"));
    }
    let target = u32::from_le_bytes(arguments[..4].try_into().unwrap());
    let address = u32::from_le_bytes(arguments[4..8].try_into().unwrap());
    let source = vm.read_string(address)?;
    let formatted = expand_kcs_format(vm, &source, 0)?;
    if vm.read(target, 1).is_err() {
        return Ok(NativeReturn::Value(0));
    }
    let mut body = extract_message_body(&formatted, encoding)?;
    let length = body
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(body.len()) as u32;
    body.push(0);
    vm.write(target, &body)?;
    Ok(NativeReturn::Value(length))
}

fn invalid(message: &'static str) -> ScriptError {
    ScriptError::InvalidStructuredText { offset: 0, message }
}

/// The task registry owns deletion, which is absent from today's SceneNode.
/// A raw target must first pass the ordinary task command router. In
/// particular, raw zero is neither an automatic miss nor a success shortcut.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct MessageRoute {
    pub id: u32,
    pub deletion: u32,
    /// Already incorporates primitive registration, drawing-context enabled
    /// state and the plane query's nonzero-mode/unreachable behavior.
    pub display: MessagePlaneDisplay,
    /// Message task elapsed time, not the caller's clock or wall time.
    pub elapsed: u32,
}

/// Raw image lookup must read ordinary native 365's original bytes. A rendered
/// single-frame image handle does not carry the correct lifetime or frame ids.
pub(super) trait MessageNativeContext: MessageBackend {
    fn route_message(&mut self, raw_target: u32) -> Result<Option<MessageRoute>>;
    fn raw_message_image(&mut self, handle: u32) -> Result<Vec<u8>>;
    /// Old command117 takes the current pixels of an image wrapper, unlike
    /// new command464's retained, multi-frame raw image resource.
    fn message_image(&mut self, _handle: u32) -> Result<RgbaFrame> {
        Err(invalid(
            "legacy message image-wrapper service is unavailable",
        ))
    }
    fn message_ruby_font_name(&self) -> Vec<u16> {
        Vec::new()
    }
    fn message_time_scale(&self) -> Option<u32>;
    /// Ordinary task time base, independent of GAME/strframe_frame.
    fn legacy_frame_rate(&self) -> u32;
}

#[derive(Clone, Debug, Default)]
pub(super) struct MessageNativeServices {
    pub(super) kx: super::message_platform::kx_native::KxServices,
    pub(super) defaults: Option<MessageDefaults>,
    /// Resource/startup font defaults, captured before private user overrides.
    pub(super) font_panel_defaults: Option<MessageDefaults>,
    pub(super) legacy: super::legacy_text_native::LegacyTextNativeServices,
    pub(super) text_input: MessageTextInputState,
    pub(super) device_keys: DeviceKeys,
    pub(super) device_keys_loaded: bool,
    pub(super) physical_buttons: BTreeSet<(ButtonSource, u8)>,
    pub(super) ime_device_keys: BTreeSet<u8>,
    pub(super) ime_device_pulses: BTreeSet<u8>,
    pub(super) pending_device_buttons: VecDeque<(ButtonSource, u32, u32)>,
    pub(super) key_scan: Keys,
    pub(super) wheel_buttons: WheelButtons,
    pub(super) wheel_observed: usize,
    pub(super) key_scan_frame: Option<u64>,
    pub(super) key_sample_physical: BTreeSet<(u32, u32)>,
    pub(super) key_dispatch: Option<super::message_platform::DeviceKeyDispatch>,
    pub(super) key_selection: Option<super::message_platform::InputSelectionDispatch>,
    pub(super) key_selection_done: bool,
    pub(super) input_geometry: cs2_runtime::message_object::InputGeometryEpoch,
    pub(super) input_geometry_tracking_complete: bool,
    pub(super) input_selection_previous_device: Option<[i32; 2]>,
    pub(super) input_selection_forced: bool,
    pub(super) pointer_dispatch: Option<super::message_platform::PointerInputDispatch>,
    pub(super) pointer_tail_done: bool,
    pub(super) pointer_origin: Option<[i32; 2]>,
    pub(super) pointer_motion_frame: Option<u64>,
    pub(super) input_selection_frame: Option<u64>,
    pub(super) input_event_mask: u32,
    pub(super) input_suppressed: u32,
    pub(super) key_capture: BTreeMap<u32, u32>,
    pub(super) pointer_move: PointerMoveState,
    pub(super) device_pointer: [i32; 2],
    pub(super) device_logical_pointer: [i32; 2],
    pub(super) device_task_pointer: [i32; 2],
    pub(super) device_task_position: [i32; 2],
    pub(super) device_pointer_moving: bool,
    pub(super) pointer_saved: Option<[i32; 2]>,
    pub(super) pointer_request: Option<[i32; 2]>,
    pub(super) device_foreground: bool,
    /// Original raw pointer-active sample from the beginning of this frame.
    /// Distinct from later foreground changes observed by the host.
    pub(super) device_sample_active: u32,
    pub(super) pending_device_active: Option<u32>,
    pub(super) device_history: cs2_runtime::message_object::InputDeviceHistory,
    pub(super) device_cursor: cs2_runtime::message_object::InputCursorState,
    pub(super) pending_device_history: Option<[u8; 32]>,
    pub(super) frame_device_history: Option<[u8; 32]>,
    pub(super) input_frame_complete: bool,
    pub(super) cursor_commands: BTreeMap<u32, super::message_platform::InputCursorDispatch>,
    pub(super) cursor_frame: Option<super::message_platform::InputCursorFrame>,
    pub(super) cursor_frame_done: bool,
    pub(super) device_pointer_observed: [i32; 2],
    pub(super) pointer_natives: BTreeMap<u32, (Vec<u8>, Option<u32>)>,
    pub(super) pointer_queries: BTreeMap<u32, (Vec<u8>, Option<u32>)>,
    pub(super) input_natives: BTreeMap<u32, super::message_platform::InputNativeDispatch>,
    pub(super) sound_prepare_natives:
        BTreeMap<u32, super::message_platform::SoundPrepareNativeDispatch>,
    pub(super) root_task_number_lookup: Option<super::message_platform::RootTaskNumberLookup>,
    pub(super) input_masks: BTreeMap<u32, InputHitMask>,
    pub(super) input_buttons: BTreeMap<u32, super::message_platform::InputButtonRuntime>,
    pub(super) button_dispatches: BTreeMap<u32, super::message_platform::ButtonDispatch>,
    pub(super) button_active: BTreeSet<u32>,
    pub(super) button_contexts: BTreeMap<u32, super::message_platform::ButtonInputContext>,
    pub(super) button_event_completions: BTreeSet<(u32, u32)>,
    pub(super) button_owner_hook: Option<super::message_platform::ButtonOwnerHook>,
    pub(super) button_binding_lookup: Option<super::message_platform::ButtonBindingLookup>,
    pub(super) fes_button_lookup: Option<super::message_platform::FesButtonTargetLookup>,
    pub(super) fes_button_media: Option<super::message_platform::FesButtonMediaHook>,
    pub(super) fes_button_named: Option<super::message_platform::FesButtonNamedHook>,
    pub(super) fes_button_records: Option<super::message_platform::FesButtonRecordLookup>,
    pub(super) fes_button_controllers: BTreeMap<u32, super::message_platform::FesButtonController>,
    pub(super) fes_button_owners: BTreeMap<u32, super::message_platform::FesButtonOwnerDispatch>,
    pub(super) fes_button_dispatches: BTreeMap<u32, super::message_platform::FesButtonDispatch>,
    pub(super) input_key_context: Option<(u32, u32, u32)>,
    pub(super) physical_edges: BTreeSet<(ButtonSource, u8, u32)>,
    pub(super) input_route_receipts: Vec<InputRouteReceipt>,
    pub(super) typed_physical_owners: BTreeMap<(ButtonSource, u8), (u32, u32, u32)>,
    objects: BTreeMap<u32, MessageObject>,
    /// Plane47's raw self-show value is distinct from inherited display.
    plane_show: BTreeMap<u32, u32>,
    pending_display: VecDeque<(u32, [u32; 4])>,
}

impl MessageNativeServices {
    /// Shared scene rendering must not draw a solid placeholder rectangle for
    /// these text-plane task types before composing their submitted texture.
    pub(super) fn handles_task_kind(kind: u32) -> bool {
        matches!(kind, 8 | 48 | 63)
    }

    pub(super) fn owns_task(&self, id: u32) -> bool {
        self.objects.contains_key(&id) || self.legacy.object(id).is_some()
    }
    pub(super) fn supports(index: u32) -> bool {
        SUPPORTED_SLOTS.contains(&index)
            || matches!(
                index,
                77 | 81 | 82 | 145..=149 | 154 | 155 | 286 | 346 | 401 | 604 | 740 | 1070..=1073
            )
            || matches!(
                index,
                99 | 104 | 105 | 108
                    ..=110 | 150 | 204 | 206 | 285 | 394 | 403 | 404 | 848 | 849 | 937
            )
            || is_display_native(index)
            || is_body_native(index)
            || super::legacy_text_native::LegacyTextNativeServices::supports(index)
    }

    pub(super) fn object(&self, id: u32) -> Option<&MessageObject> {
        self.objects.get(&id)
    }
    pub(super) fn plane_show(&self, id: u32) -> u32 {
        self.plane_show.get(&id).copied().unwrap_or(1)
    }
    pub(super) fn set_plane_show(&mut self, id: u32, value: u32) {
        self.plane_show.insert(id, value);
    }
    pub(super) fn objects(&self) -> &BTreeMap<u32, MessageObject> {
        &self.objects
    }
    pub(super) fn object_mut(&mut self, id: u32) -> Option<&mut MessageObject> {
        self.objects.get_mut(&id)
    }

    /// Called on task creation, or lazily after a routed node passes validation.
    /// Existing state is never replaced by an unrelated native call.
    pub(super) fn register(&mut self, id: u32) -> Result<()> {
        self.register_encoding(id, MessageEncoding::Cp932)
    }

    pub(super) fn register_encoding(&mut self, id: u32, encoding: MessageEncoding) -> Result<()> {
        if let Some(object) = self.objects.get(&id) {
            if object.encoding() != encoding {
                return Err(invalid("message task changed its text receiver encoding"));
            }
            return Ok(());
        }
        if !self.objects.contains_key(&id) {
            if self.objects.len() >= MAX_OBJECTS {
                return Err(invalid("message object count exceeds its storage limit"));
            }
            self.objects
                .insert(id, MessageObject::new(id as i32, encoding));
        }
        Ok(())
    }

    /// Call from task resource release, after the ordinary deletion protocol.
    pub(super) fn release(&mut self, id: u32) -> Option<MessageObject> {
        self.kx.objects.remove(&id);
        self.kx.names.remove(&id);
        self.kx.release_metadata(id);
        self.input_natives.remove(&id);
        self.cursor_commands.remove(&id);
        self.sound_prepare_natives.remove(&id);
        self.pointer_queries.remove(&id);
        self.pointer_natives.remove(&id);
        self.input_masks.remove(&id);
        self.input_buttons.remove(&id);
        self.button_dispatches.remove(&id);
        self.button_active.remove(&id);
        self.button_contexts.remove(&id);
        self.button_event_completions
            .retain(|(region, _)| *region != id);
        self.fes_button_dispatches.remove(&id);
        self.fes_button_controllers.remove(&id);
        self.fes_button_owners.remove(&id);
        self.legacy.release(id);
        self.plane_show.remove(&id);
        self.pending_display.retain(|(target, _)| *target != id);
        self.objects.remove(&id)
    }

    pub(super) fn queue_display(&mut self, target: u32, packet: [u32; 4]) -> Result<()> {
        if !matches!(packet[1], 47 | 48) {
            return Err(invalid("message visibility queue received another command"));
        }
        if self.pending_display.len() >= 65_536 {
            return Err(invalid("message visibility queue exceeds 65536 packets"));
        }
        self.pending_display.push_back((target, packet));
        Ok(())
    }

    pub(super) fn pending_display(&self) -> Option<(u32, [u32; 4])> {
        self.pending_display.front().copied()
    }

    pub(super) fn finish_display(&mut self) {
        self.pending_display.pop_front();
    }

    pub(super) fn snapshot(&self, id: u32) -> Option<MessageObjectSnapshot> {
        self.objects.get(&id).map(MessageObject::snapshot)
    }

    pub(super) fn restore(&mut self, id: u32, snapshot: MessageObjectSnapshot) -> Result<()> {
        if snapshot.object.id() as u32 != id
            || self
                .objects
                .get(&id)
                .is_some_and(|object| object.encoding() != snapshot.object.encoding())
        {
            return Err(invalid(
                "ordinary message snapshot id or encoding does not match its task",
            ));
        }
        let object = MessageObject::from_snapshot(snapshot)?;
        if !self.objects.contains_key(&id) && self.objects.len() >= MAX_OBJECTS {
            return Err(invalid("message snapshot restore exceeds the object limit"));
        }
        self.objects.insert(id, object);
        Ok(())
    }

    /// Call from the message task's body, after its clock/child update. The
    /// context must retain pixels by (task id, surface_generation). Presentation
    /// must also inspect objects that became inactive after clear/force-finish.
    pub(super) fn frame_update(
        &mut self,
        id: u32,
        elapsed: u32,
        nodes: &BTreeMap<u32, SceneNode>,
        deletion: u32,
        context: &mut impl MessageNativeContext,
    ) -> Result<Option<MessageRevealUpdate>> {
        if deletion != 0
            || nodes
                .get(&id)
                .is_none_or(|node| !matches!(node.kind, 48 | 63))
        {
            return Ok(None);
        }
        let time_scale = context.message_time_scale();
        let Some(object) = self.objects.get_mut(&id) else {
            return Ok(None);
        };
        let expected_kind = if object.encoding() == MessageEncoding::Unicode {
            63
        } else {
            48
        };
        if nodes.get(&id).is_none_or(|node| node.kind != expected_kind) {
            return Ok(None);
        }
        object.frame_update(elapsed, time_scale, context)
    }

    pub(super) fn call(
        &mut self,
        index: u32,
        arguments: &[u8],
        vm: &mut Vm,
        nodes: &BTreeMap<u32, SceneNode>,
        context: &mut impl MessageNativeContext,
    ) -> Result<NativeReturn> {
        if is_body_native(index) {
            return call_message_body_native(index, arguments, vm);
        }
        if super::legacy_text_native::LegacyTextNativeServices::supports(index) {
            return self
                .legacy
                .call(index, arguments, vm, nodes, self.defaults.as_ref(), context);
        }
        let words = match index {
            611..=614 | 619 | 620 | 695 => 2,
            621 => 3,
            615..=618 | 884 => 1,
            _ => return Err(ScriptError::UnimplementedNativeCall { index }),
        };
        if arguments.len() != words * 4 {
            return Err(invalid(
                "ordinary message native requires its exact documented DWORD count",
            ));
        }
        let word = |index: usize| {
            u32::from_le_bytes(arguments[index * 4..index * 4 + 4].try_into().unwrap())
        };
        // native 620 is uniquely an early no-op: zero source must not resolve
        // the target, clear an existing source, or touch the image registry.
        if index == 620 && word(1) == 0 {
            return Ok(NativeReturn::Continue);
        }

        // Buffer packaging happens before command dispatch. A valid nonzero
        // address with a bad tag lends only its DWORD; no arbitrary 0xe8 read.
        // Address zero is VM address zero, not a host nullptr.
        let mut configuration = if matches!(index, 611..=613) {
            Some(ConfigurationBorrow::read(vm, word(1))?)
        } else {
            None
        };
        let append = if index == 614 {
            let input = vm.read_string(word(1))?;
            let formatted = expand_kcs_format(vm, &input, 0)?;
            let length = formatted
                .iter()
                .position(|&byte| byte == 0)
                .unwrap_or(formatted.len());
            if length > MAX_BYTES {
                return Err(invalid("formatted message exceeds 64 MiB"));
            }
            let mut buffer = formatted[..length].to_vec();
            buffer.push(0);
            Some(buffer)
        } else {
            None
        };
        let target = word(0);
        let routed = context.route_message(target)?;
        let route = routed.filter(|route| {
            route.deletion == 0
                && nodes
                    .get(&route.id)
                    .is_some_and(|node| node.kind == MESSAGE_OBJECT_TYPE)
        });
        let Some(route) = route else {
            return if matches!(index, 616 | 884) {
                Err(invalid(
                    "message quantity query has no reachable live kind-48 handler",
                ))
            } else {
                // Ordinary commands to missing/deleted/wrong-type targets have
                // no message-specific handler. Their void return stays status 2.
                Ok(NativeReturn::Continue)
            };
        };
        self.register(route.id)?;
        let scale = context.message_time_scale();
        let object = self.objects.get_mut(&route.id).unwrap();
        let result = (|| {
            match index {
                611 => {
                    let buffer = configuration.as_mut().unwrap();
                    if buffer.bytes.len() != MESSAGE_CONFIGURATION_SIZE {
                        return Ok(NativeReturn::Continue);
                    }
                    let defaults = self.defaults.as_ref().ok_or_else(|| {
                        invalid("message default configuration has not been supplied")
                    })?;
                    let bytes = defaults.configuration(MessageEncoding::Cp932)?;
                    buffer.bytes.copy_from_slice(bytes.bytes());
                }
                620 => {
                    let bytes = context.raw_message_image(word(1))?;
                    object.set_user_glyph_source(Some(bytes))?;
                }
                _ => {
                    let command = match index {
                        612..=619 => index - 156,
                        621 => 465,
                        695 => 467,
                        884 => 472,
                        _ => return Err(ScriptError::UnimplementedNativeCall { index }),
                    };
                    let data = match index {
                        619 | 695 => [0, word(1), 0],
                        621 => [word(1), word(2), 0],
                        _ => [0; 3],
                    };
                    let buffer = configuration
                        .as_ref()
                        .map(|buffer| buffer.bytes.as_slice())
                        .or_else(|| append.as_deref());
                    return match object.text_command(
                        command,
                        data,
                        buffer,
                        route.display,
                        route.elapsed,
                        scale,
                        context,
                    )? {
                        MessageCommandResult::Handled => Ok(NativeReturn::Continue),
                        MessageCommandResult::Value(value) => Ok(NativeReturn::Value(value)),
                        MessageCommandResult::Unhandled => {
                            Err(ScriptError::UnimplementedNativeCall { index })
                        }
                    };
                }
            }
            Ok(NativeReturn::Continue)
        })();
        // Explicitly write back after dispatch, including before returning an
        // error. Any handler-produced bytes survive partial failure. vm.write
        // uses the high bit to select external storage and never masks it away.
        if let Some(buffer) = configuration {
            buffer.write_back(vm)?;
        }
        result
    }
}

struct ConfigurationBorrow {
    address: u32,
    bytes: Vec<u8>,
}

impl ConfigurationBorrow {
    fn read(vm: &Vm, address: u32) -> Result<Self> {
        let tag = vm.read(address, 4)?;
        let bytes = if tag == (MESSAGE_CONFIGURATION_SIZE as u32).to_le_bytes() {
            vm.read(address, MESSAGE_CONFIGURATION_SIZE)?
        } else {
            tag
        };
        Ok(Self { address, bytes })
    }
    fn write_back(self, vm: &mut Vm) -> Result<()> {
        vm.write(self.address, &self.bytes)
    }
}

/// Concrete user-glyph decoding for backend implementations. The retained raw
/// source owns its lifetime; missing frame ids become explicit empty glyphs.
pub(super) fn decode_inline_image(source: &[u8], frame_id: u32) -> Result<Option<RgbaFrame>> {
    let inspection = inspect_hg(source)
        .map_err(|_| invalid("inline message source is not a supported HG image"))?;
    if inspection.frame(frame_id).is_none() {
        return Ok(None);
    }
    // Unlike user-font mappings, ui draws the decoded native frame rectangle:
    // no original-canvas expansion, square scaling or text effects.
    RgbaFrame::decode_hg(source, AnimationFrameSelection::Id(frame_id))
        .map(Some)
        .map_err(|_| invalid("inline message HG frame decoding failed"))
}

pub(super) fn decode_user_glyph(source: &[u8], frame_id: u32) -> Result<Option<MessageUserGlyph>> {
    let inspection = inspect_hg(source)
        .map_err(|_| invalid("message user glyph source is not a supported HG image"))?;
    if inspection.container != HgContainer::Hg3V300 {
        return Err(invalid("message user glyph requires native 32-bit HG-3"));
    }
    let Some(info) = inspection.frame(frame_id) else {
        return Ok(None);
    };
    if info.bit_depth != 32
        || !info
            .tags
            .iter()
            .any(|tag| tag.kind == HgTagKind::NativeImage)
        || info.tags.iter().any(|tag| {
            matches!(
                tag.kind,
                HgTagKind::Jpeg | HgTagKind::Wbp | HgTagKind::Jpa | HgTagKind::ImageSlice
            )
        })
    {
        return Err(invalid(
            "message user glyph frame is not native 32-bit HG-3",
        ));
    }
    let image = RgbaFrame::decode_hg(source, AnimationFrameSelection::Id(frame_id))
        .map_err(|_| invalid("message user glyph HG-3 frame decoding failed"))?;
    let metadata = image.metadata();
    let [width, height] = metadata.canvas_size;
    let length = (width as usize)
        .checked_mul(height as usize)
        .and_then(|length| length.checked_mul(4))
        .filter(|&length| length <= MAX_BYTES)
        .ok_or_else(|| invalid("message user glyph original canvas exceeds 64 MiB"))?;
    if width == 0 || height == 0 {
        return Err(invalid("message user glyph original canvas is empty"));
    }
    let mut bgra = vec![0; length];
    for y in 0..image.height() {
        for x in 0..image.width() {
            let dx = i64::from(metadata.offset[0]) + i64::from(x);
            let dy = i64::from(metadata.offset[1]) + i64::from(y);
            if dx < 0 || dy < 0 || dx >= i64::from(width) || dy >= i64::from(height) {
                continue;
            }
            let input = ((y as usize * image.width() as usize) + x as usize) * 4;
            let output = ((dy as usize * width as usize) + dx as usize) * 4;
            let pixel = &image.pixels()[input..input + 4];
            bgra[output..output + 4].copy_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
        }
    }
    let glyph = MessageUserGlyph {
        size: [width, height],
        bgra,
    };
    glyph.validate()?;
    Ok(Some(glyph))
}

/// Fixed-point square resampling for mapped image glyphs. This preserves source
/// color (including transparent RGB); text color and coverage doubling do not
/// apply. Effects and the (1,1) temporary-surface placement belong to the painter.
pub(super) fn scale_user_glyph(glyph: &MessageUserGlyph, size: u32) -> Result<MessageUserGlyph> {
    glyph.validate()?;
    let [width, height] = glyph.size;
    if size == 0 || width == 0 || height == 0 {
        return Err(invalid(
            "message user glyph scaling requires nonzero dimensions",
        ));
    }
    if u64::from(width) > u64::from(size) * 4 || u64::from(height) > u64::from(size) * 4 {
        return shrink_user_glyph(glyph, size);
    }
    let length = (size as usize)
        .checked_mul(size as usize)
        .and_then(|length| length.checked_mul(4))
        .filter(|&length| length <= MAX_BYTES)
        .ok_or_else(|| invalid("scaled message user glyph exceeds 64 MiB"))?;
    let step_x = (u64::from(width) << 15) / u64::from(size);
    let step_y = (u64::from(height) << 15) / u64::from(size);
    let mut pixels = vec![0; length];
    for y in 0..size {
        let fy = u64::from(y) * step_y;
        let sy = ((fy >> 15) as u32).min(height - 1);
        let wy = (fy & 0x7fff) as u32;
        for x in 0..size {
            let fx = u64::from(x) * step_x;
            let sx = ((fx >> 15) as u32).min(width - 1);
            let wx = (fx & 0x7fff) as u32;
            let sample = |x: u32, y: u32| {
                let offset = ((y as usize * width as usize) + x as usize) * 4;
                &glyph.bgra[offset..offset + 4]
            };
            let a = sample(sx, sy);
            let b = sample((sx + 1).min(width - 1), sy);
            let c = sample(sx, (sy + 1).min(height - 1));
            let d = if sx + 1 < width && sy + 1 < height {
                sample(sx + 1, sy + 1)
            } else {
                a
            };
            if [a, b, c, d].iter().all(|pixel| pixel[3] == 0) {
                continue;
            }
            let weights = [
                ((0x8000 - wx) * (0x8000 - wy)) >> 15,
                (wx * (0x8000 - wy)) >> 15,
                ((0x8000 - wx) * wy) >> 15,
                (wx * wy) >> 15,
            ];
            let target = ((y as usize * size as usize) + x as usize) * 4;
            for channel in 0..4 {
                pixels[target + channel] = ((u64::from(a[channel]) * u64::from(weights[0])
                    + u64::from(b[channel]) * u64::from(weights[1])
                    + u64::from(c[channel]) * u64::from(weights[2])
                    + u64::from(d[channel]) * u64::from(weights[3]))
                    >> 15)
                    .min(255) as u8;
            }
            if [a, b, c, d].iter().all(|pixel| pixel[3] == 255) {
                pixels[target + 3] = 255;
            }
        }
    }
    Ok(MessageUserGlyph {
        size: [size, size],
        bgra: pixels,
    })
}

/// Original large-shrink branch uses a floor-sized rectangular sample block,
/// with single-precision origins and independent integer BGRA averages. It
/// neither weights fractional edges nor premultiplies transparent RGB.
fn shrink_user_glyph(glyph: &MessageUserGlyph, size: u32) -> Result<MessageUserGlyph> {
    let [width, height] = glyph.size;
    let length = (size as usize)
        .checked_mul(size as usize)
        .and_then(|length| length.checked_mul(4))
        .filter(|&length| length <= MAX_BYTES)
        .ok_or_else(|| invalid("area-scaled message user glyph exceeds 64 MiB"))?;
    let ratio_x = width as f32 / size as f32;
    let ratio_y = height as f32 / size as f32;
    let block_x = (ratio_x as u32).max(1);
    let block_y = (ratio_y as u32).max(1);
    let mut bgra = vec![0; length];
    for y in 0..size {
        let sy = (y as f32 * ratio_y) as u32;
        let end_y = sy.saturating_add(block_y).min(height);
        for x in 0..size {
            let sx = (x as f32 * ratio_x) as u32;
            let end_x = sx.saturating_add(block_x).min(width);
            let mut sums = [0u64; 4];
            let mut count = 0u64;
            for row in sy..end_y {
                for column in sx..end_x {
                    let at = (row as usize * width as usize + column as usize) * 4;
                    for channel in 0..4 {
                        sums[channel] += u64::from(glyph.bgra[at + channel]);
                    }
                    count += 1;
                }
            }
            if count != 0 {
                let at = (y as usize * size as usize + x as usize) * 4;
                for channel in 0..4 {
                    bgra[at + channel] = (sums[channel] / count) as u8;
                }
            }
        }
    }
    Ok(MessageUserGlyph {
        size: [size, size],
        bgra,
    })
}

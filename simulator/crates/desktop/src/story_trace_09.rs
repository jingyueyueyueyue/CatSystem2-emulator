use cs2_runtime::message_object::{ButtonSource, DeviceBinding, InputButtonState};
use cs2_script::KcsProgram;

use crate::cst_context_native::{CstBodySource, CstSourceProbe, CstSourceRecordObservation};
use crate::fes_driver::FesDriverWait;
use crate::particle_plugin_digest::sha256;
use crate::{HeadlessPlayer, ScriptTextEncoding};

use super::{current_committed_body, submitted_body_pixels};

const WINDOW: &str = "meswnd";
const SCENE: &str = "sc01";
const ROOT_WAIT: u32 = 0x0cae;
const ENTER: u8 = 13;
const LOGICAL_ENTER: usize = 2;
const BOTTOM_BOUNDS: [i32; 4] = [-100000, -100000, 100000, 100000];
const BOTTOM_PRIORITY: i32 = 6;
const SSCRIPT_CODE: &str = "7a068090a8d37c7a446a74e98448458e3d6ed75d94bb1a00022eaeb07d7a3921";
const SSCRIPT_DATA: &str = "cda42ae0bdd55920fd91baf7c5267995cf9b497fca4fe4530ef0469adbe14a14";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct KanmiPage {
    pub(super) frame: u64,
    pub(super) scene: &'static str,
    pub(super) group: i32,
    pub(super) record: i32,
    pub(super) window: &'static str,
    pub(super) owner: u32,
    pub(super) receiver: u32,
    pub(super) region: u32,
    pub(super) source: CstBodySource,
    pub(super) source_epoch: u64,
}

pub(super) fn observe(player: &HeadlessPlayer, frame: u64) -> Option<KanmiPage> {
    let services = &player.data_services;
    let input = &services.messages;
    if !services.story_trace
        || player.verified_kanmi_runtime_pe_sha256().is_none()
        || player.loaded.as_ref()?.name != "kcs.int/main.kcs"
        || player.state.clock.frame != frame
        || player.frame_in_progress()
        || !player.state.input.focused
        || !player.state.input.pressed.is_empty()
        || services.host_pause.paused
        || services.host_quit_requested
        || !player.movie_playbacks.is_empty()
        || !player.pending_messages.is_empty()
        || !player.pending_fes_events.is_empty()
        || !services.pending_host_input.is_empty()
        || player.message_wait_address != Some(ROOT_WAIT)
        || !player.vm.as_ref()?.waiting_for_message()
        || !player.input_task.alive
        || !input.device_keys_loaded
        || !input.device_foreground
        || !input.physical_buttons.is_empty()
        || !input.physical_edges.is_empty()
        || !input.pending_device_buttons.is_empty()
        || input.key_dispatch.is_some()
        || input.key_selection.is_some()
        || !input.button_dispatches.is_empty()
        || !input.button_active.is_empty()
        || input.key_capture.contains_key(&(LOGICAL_ENTER as u32))
        || input.typed_physical_owners.contains_key(&(ButtonSource::Keyboard, ENTER))
        || input.device_keys.forced_queries(LOGICAL_ENTER as u32).ok()? != 0
        || input.device_keys.forced_queries(0).ok()? != 0
        || !enter_mapping(player.device_key_bindings(), &player.input_task.key_enabled)
    {
        return None;
    }

    let mut contexts = player.cst_contexts.iter().filter(|(_, context)| context.document.is_some());
    let (&handle, context) = contexts.next()?;
    if contexts.next().is_some() || context.story.scene != SCENE || context.localization.is_some() {
        return None;
    }
    let (marked, group, record) = context.story.settled_body_record(frame)?;
    let absolute_record = source_record(&context.story.scene, group, record)?;
    let document = context.document.as_ref()?;
    let group_range = document.inputs.get(usize::try_from(group).ok()?)?;
    let wait_record = usize::try_from(record).ok()?;
    if !group_range.contains(&absolute_record)
        || !group_range.contains(&wait_record)
        || document.records.get(absolute_record)?.kind != 0x20
        || document.records.get(wait_record)?.kind != 0x02
    {
        return None;
    }

    let tasks = &services.tasks;
    let mut flow = None;
    let mut owner = None;
    for (id, name, runtime, wait_key) in tasks.fes.story_runtimes() {
        if !matches!(tasks.message_task_state(id), Some((34, 0, _))) {
            return None;
        }
        let driver = runtime.driver.as_ref()?;
        let pending = driver.has_pending_handler() || driver.initialization_pending();
        let block = driver.interpreter.current_block();
        match name {
            b"flow" | b"flow.fes" => {
                if !settled_poll(block, b"GAME_WAIT", pending, driver.waiting(), frame)
                    || wait_key || flow.replace(id).is_some()
                {
                    return None;
                }
            }
            b"meswnd" | b"meswnd.fes" => {
                let locals = driver.interpreter.local_values();
                let prompt = runtime.media.get("keywait")?;
                if !settled_poll(block, b"WAIT", pending, driver.waiting(), frame)
                    || !message_ready(wait_key, locals.get(100).copied(), locals.get(52).copied(),
                        locals.get(101).copied(), prompt.visible, prompt.opacity)
                    || driver.kanmi_message_fes_source != Some(true)
                    || owner.replace(id).is_some()
                {
                    return None;
                }
            }
            _ => return None,
        }
    }
    let flow = flow?;
    let owner = owner?;
    if tasks.fes.runtimes().count() != 2 {
        return None;
    }
    let receiver = *player.task_parents.get(&owner)?;
    if !live_child(player, flow, 0, 34)
        || !live_child(player, receiver, flow, 0)
        || !live_child(player, owner, receiver, 34)
        || !tasks.task_clock(receiver).is_some_and(|clock| clock.paused == 0)
    {
        return None;
    }

    let generation = services.fes_text.story_body_generation(owner)?;
    let origins = &services.cst_text_origins;
    let [source] = services.fes_text.story_body_sources(owner, generation.source_epoch, origins)? else {
        return None;
    };
    if source.context_handle != handle
        || source.group != group
        || source.absolute_record != absolute_record
        || !origins.source_record_observations(handle).iter().any(|entry|
            exact_source(entry, *source, context.story.loaded, generation.source_frame))
        || !current_committed_body(generation, context.story.loaded, marked, frame)
        || !body_and_prompt_submitted(player, owner)
    {
        return None;
    }

    let mut bottom_buttons = player.scene_nodes.iter().filter(|(_, node)| node.name == "bottom button");
    let (&region, _) = bottom_buttons.next()?;
    if bottom_buttons.next().is_some() || !live_child(player, region, receiver, 6) {
        return None;
    }
    let mut registrations = player.input_task.regions.iter().filter(|entry| entry.id() == region);
    let registration = registrations.next()?;
    let button = input.input_buttons.get(&region)?;
    if registrations.next().is_some()
        || !bottom_registration(&registration.words, &button.state, region, receiver, button.execution_checkpoint().3)
        || input.input_masks.contains_key(&region)
        || !sscript_program(player.task_vms.get(&receiver)?.program()?)
    {
        return None;
    }

    Some(KanmiPage {
        frame, scene: SCENE, group, record, window: WINDOW, owner, receiver, region,
        source: *source, source_epoch: generation.source_epoch,
    })
}

fn source_record(scene: &str, group: i32, wait: i32) -> Option<usize> {
    match (scene, group, wait) {
        (SCENE, 0, 41) => Some(40),
        (SCENE, 1, 43) => Some(42),
        _ => None,
    }
}

fn exact_source(entry: &CstSourceRecordObservation, source: CstBodySource, loaded: u64, source_frame: u64) -> bool {
    entry.probe == CstSourceProbe::Kanmi && entry.source == source && source.load_id != 0
        && entry.native == 327 && entry.kind == 0x20 && entry.body_tagged && !entry.localized
        && entry.stored_codec == ScriptTextEncoding::Cp932 && entry.output_codec == ScriptTextEncoding::Cp932
        && entry.raw_bytes != 0 && entry.raw_bytes == entry.output_bytes
        && loaded <= entry.frame && entry.frame <= source_frame
}

fn live_child(player: &HeadlessPlayer, id: u32, parent: u32, kind: u32) -> bool {
    player.task_parents.get(&id) == Some(&parent)
        && player.scene_nodes.get(&id).is_some_and(|node| node.parent == parent && node.kind == kind)
        && player.data_services.tasks.message_task_state(id).is_some_and(|(live_kind, deleted, _)|
            live_kind == kind && deleted == 0)
}

fn settled_poll(block: &[u8], expected: &[u8], pending: bool, wait: Option<&FesDriverWait>, frame: u64) -> bool {
    block == expected && !pending
        && matches!(wait, Some(FesDriverWait::Frame { after }) if *after == frame)
}

fn message_ready(wait_key: bool, input: Option<i32>, animation: Option<i32>, prompt: Option<i32>, visible: bool, opacity: u8) -> bool {
    wait_key && input.is_some_and(|value| value != 0) && animation.is_some_and(|value| value != 0)
        && prompt == Some(1) && visible && opacity != 0
}

fn body_and_prompt_submitted(player: &HeadlessPlayer, owner: u32) -> bool {
    let body_key = (owner, "\0text:737472[0]".to_owned());
    let prompt_key = (owner, "keywait".to_owned());
    let mut lists = player.data_services.tasks.fes.platform.lists.values().filter(|list|
        list.layers.contains_key(&body_key) || list.layers.contains_key(&prompt_key));
    let Some(list) = lists.next() else { return false; };
    if lists.next().is_some() || !list.enabled || !list.submit {
        return false;
    }
    let Some(output) = player.image_handles.get(&list.output) else { return false; };
    [body_key, prompt_key].iter().all(|key| list.layers.get(key)
        .is_some_and(|layer| submitted_body_pixels(layer, output)))
}

fn bottom_registration(words: &[u32; 15], state: &InputButtonState, region: u32, receiver: u32, bound: bool) -> bool {
    bound && words[0] == region && words[1] as i32 == BOTTOM_PRIORITY
        && words[2] & ButtonSource::Keyboard.mask() != 0
        && words[3..7].iter().map(|value| *value as i32).eq(BOTTOM_BOUNDS)
        && words[8] != 0 && words[9] != 0 && words[11] == 0
        && state.id == region && state.receiver == Some(receiver) && state.show != 0
        && state.full_screen && state.rectangle == BOTTOM_BOUNDS
        && state.priority == BOTTOM_PRIORITY && state.parameter == 0
}

fn enter_mapping(bindings: &[DeviceBinding; 80], scan_enabled: &[bool; 80]) -> bool {
    let keyboard = ButtonSource::Keyboard.index();
    scan_enabled[LOGICAL_ENTER] && bindings[LOGICAL_ENTER].enabled
        && bindings[LOGICAL_ENTER].codes[keyboard][0] == ENTER
        && bindings.iter().enumerate().all(|(logical, binding)| {
            !binding.enabled || !scan_enabled[logical] || binding.codes[keyboard].iter().enumerate()
                .all(|(group, &code)| code != ENTER || group == 0 && matches!(logical, 0 | LOGICAL_ENTER))
        })
}

fn sscript_program(program: &KcsProgram) -> bool {
    program.code.len() == 919272 && program.data.len() == 497540 && program.stack_size == 10240
        && sha256(&program.code) == SSCRIPT_CODE && sha256(&program.data) == SSCRIPT_DATA
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kanmi_guard_accepts_only_the_first_two_exact_scene_waits() {
        assert_eq!(source_record("sc01", 0, 41), Some(40));
        assert_eq!(source_record("sc01", 1, 43), Some(42));
        assert_eq!(source_record("sc02", 0, 41), None);
        assert_eq!(source_record("sc01", 1, 41), None);
        assert_eq!(source_record("sc01", 1, 42), None);
        assert_eq!(source_record("sc01", 2, 45), None);
    }

    #[test]
    fn kanmi_guard_rejects_stale_or_altered_body_provenance() {
        let source = CstBodySource { context_handle: 155, load_id: 9, group: 1, absolute_record: 42 };
        let observation = CstSourceRecordObservation {
            source, frame: 910, probe: CstSourceProbe::Kanmi, native: 327, kind: 0x20,
            stored_codec: ScriptTextEncoding::Cp932, output_codec: ScriptTextEncoding::Cp932,
            raw_bytes: 44, output_bytes: 44, body_tagged: true, localized: false,
        };
        assert!(exact_source(&observation, source, 448, 911));
        assert!(!exact_source(&observation, source, 448, 909));
        assert!(!exact_source(&observation, CstBodySource { load_id: 10, ..source }, 448, 911));
        assert!(!exact_source(&observation, CstBodySource { absolute_record: 40, ..source }, 448, 911));
        assert!(!exact_source(&CstSourceRecordObservation { localized: true, ..observation }, source, 448, 911));
        assert!(!exact_source(&CstSourceRecordObservation { body_tagged: false, ..observation }, source, 448, 911));
        assert!(!exact_source(&CstSourceRecordObservation { probe: CstSourceProbe::Eden, ..observation }, source, 448, 911));
    }

    #[test]
    fn kanmi_guard_requires_settled_wait_and_visible_prompt() {
        let wait = FesDriverWait::Frame { after: 900 };
        assert!(settled_poll(b"WAIT", b"WAIT", false, Some(&wait), 900));
        assert!(!settled_poll(b"MES_SHOW", b"WAIT", false, Some(&wait), 900));
        assert!(!settled_poll(b"WAIT", b"WAIT", true, Some(&wait), 900));
        assert!(!settled_poll(b"WAIT", b"WAIT", false, Some(&wait), 901));
        assert!(message_ready(true, Some(1), Some(1), Some(1), true, 255));
        assert!(!message_ready(true, Some(1), Some(1), Some(1), true, 0));
        assert!(!message_ready(true, Some(1), Some(1), Some(1), false, 255));
        assert!(!message_ready(true, Some(1), Some(1), Some(0), true, 255));
        assert!(!message_ready(false, Some(1), Some(1), Some(1), true, 255));
    }

    #[test]
    fn kanmi_guard_requires_the_bound_live_keyboard_region() {
        let region = 760;
        let receiver = 58;
        let mut words = [region, 6, 7, 0, 0, 0, 0, 0, 1, 1, 0, 0, 0, 0, 1];
        words[3..7].copy_from_slice(&BOTTOM_BOUNDS.map(|value| value as u32));
        let mut state = InputButtonState::new(region);
        state.receiver = Some(receiver);
        state.rectangle = BOTTOM_BOUNDS;
        state.full_screen = true;
        state.priority = 6;
        assert!(bottom_registration(&words, &state, region, receiver, true));
        assert!(!bottom_registration(&words, &state, region, receiver, false));
        assert!(!bottom_registration(&words, &state, region, 8, true));
        assert!(!bottom_registration(&words, &state, 60, receiver, true));
        words[8] = 0;
        assert!(!bottom_registration(&words, &state, region, receiver, true));
        words[8] = 1;
        words[2] = ButtonSource::Mouse.mask();
        assert!(!bottom_registration(&words, &state, region, receiver, true));
        words[2] = ButtonSource::Keyboard.mask();
        words[11] = 1;
        assert!(!bottom_registration(&words, &state, region, receiver, true));
    }

    #[test]
    fn kanmi_guard_rejects_disabled_moved_or_ambiguous_enter_bindings() {
        let mut bindings = [DeviceBinding::default(); 80];
        let mut enabled = [true; 80];
        let keyboard = ButtonSource::Keyboard.index();
        bindings[2].enabled = true;
        bindings[2].codes[keyboard][0] = ENTER;
        bindings[0] = bindings[2];
        assert!(enter_mapping(&bindings, &enabled));
        enabled[2] = false;
        assert!(!enter_mapping(&bindings, &enabled));
        enabled[2] = true;
        bindings[2].codes[keyboard][1] = ENTER;
        assert!(!enter_mapping(&bindings, &enabled));
        bindings[2].codes[keyboard][1] = 0;
        bindings[4] = bindings[2];
        assert!(!enter_mapping(&bindings, &enabled));
        bindings[4].enabled = false;
        assert!(enter_mapping(&bindings, &enabled));
        bindings[2].codes[keyboard][0] = 32;
        assert!(!enter_mapping(&bindings, &enabled));
    }
}

use cs2_runtime::message_object::ButtonSource;
use cs2_script::FesProgram;

use crate::cst_context_native::CstSourceProbe;
use crate::fes_driver::FesScriptDriver;
use crate::particle_plugin_digest::sha256;
use crate::{HeadlessPlayer, ScriptTextEncoding};

use super::story_trace_09::{enter_mapping, live_child, settled_poll};
use super::{current_committed_body, submitted_body_pixels};

pub(crate) const PE: &str = "c48d383c335c5b283bb0c17bd9f14b2b8bfef4a2e3d635365157b1c69498df78";
pub(crate) const CST: &str = "9911a9f8daecd80062fe4fcdf3f438845b5ecafdf55c1292a97ce18cd0233087";
pub(crate) const POST_OP_CST: &str = "6fc067e685d21ec1e19d59e62c040d7d3364f6f4a1b3d987d7b845b038ac3bbb";
pub(crate) const STARTUP: &str = "kcs.int/main.kcs";
const MAIN_CODE: &str = "c042279f365354ad248d2db84e20fa25a472c4b639e40c8918c1e99fac7803aa";
pub(crate) const SSCRIPT_CODE: &str = "cb6cbd257ae3986337adc791d8191fa3833ef164f9cfd20b22c706acfc5b4af9";
const FLOW: &str = "679d9f5370ae5afecbadd0362df560009072cc0746770449e1def0bf255909d7";
const TITLE: &str = "4e6e23a102f2a07446e3daa1d15e8ee2a232ac3d0380ba210d63564bc04ed9df";
pub(crate) const MESWND: &str = "9a720b45d68f7014f52d5a8c78cd6c0b18ab3dacc0f9c99b9aab280efc2fbea4";
const START_REGION: u32 = 15;
// (input wait, nonempty body), in CST group order.
const OP001_WAITS: [(i32, usize); 62] = [
    (69, 68), (72, 71), (92, 91), (100, 99), (109, 108), (148, 147),
    (151, 150), (154, 153), (157, 156), (161, 160), (164, 163), (167, 166),
    (170, 169), (174, 173), (179, 178), (195, 194), (206, 205), (210, 209),
    (214, 213), (218, 217), (222, 221), (226, 225), (230, 229), (237, 236),
    (241, 240), (245, 244), (249, 248), (253, 252), (257, 256), (261, 260),
    (279, 276), (301, 300), (306, 305), (318, 317), (322, 321), (341, 333),
    (348, 347), (380, 359), (384, 383), (406, 395), (410, 409), (422, 421),
    (426, 425), (448, 437), (452, 451), (464, 463), (468, 467), (480, 479),
    (484, 483), (496, 495), (500, 499), (512, 511), (562, 561), (567, 566),
    (579, 578), (583, 582), (591, 587), (596, 595), (601, 600), (613, 612),
    (616, 615), (619, 618),
];
const POST_OP_WAITS: [(i32, usize); 11] = [
    (42, 41), (46, 45), (51, 50), (55, 54), (60, 59), (64, 63),
    (69, 68), (73, 72), (78, 77), (82, 81), (90, 86),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HatsumiraPage {
    pub owner: u32,
    pub region: u32,
    pub receiver: u32,
    pub context: u32,
    pub load: u64,
    pub loaded: u64,
    pub marked: u64,
    pub epoch: u64,
    pub source_frame: u64,
    pub read_frame: u64,
    pub complete: u64,
}

impl HeadlessPlayer {
    pub fn host_hatsumira_12_op001_records(group: i32) -> Option<(i32, usize)> {
        OP001_WAITS.get(usize::try_from(group).ok()?).copied()
    }

    pub fn host_hatsumira_12_post_op_records(group: i32) -> Option<(i32, usize)> {
        POST_OP_WAITS.get(usize::try_from(group).ok()?).copied()
    }

    /// Flow and live title owners for the default New Game screen.
    pub fn host_hatsumira_12_title(&mut self) -> Option<(u32, u32)> {
        quiet(self)?;
        if self.cst_contexts.values().any(|context| context.document.is_some()) {
            return None;
        }
        let (flow, flow_driver) = window(self, b"flow", b"LOOP_TITLE", false)?;
        let (owner, title_driver) = window(self, b"title", b"WAIT_USER", false)?;
        if title_driver.interpreter.local_values().first() != Some(&0)
            || !live_child(self, flow, 0, 34)
            || !live_child(self, owner, flow, 34)
            || !live_child(self, START_REGION, owner, 6)
            || !live_child(self, 37, owner, 6)
            || !live_child(self, 7, flow, 6)
            || !verified_fes(self, "flow", FLOW, flow_driver)
            || !verified_fes(self, "title", TITLE, title_driver)
            || !self.input_button_state(START_REGION)
                .is_some_and(|state| state.show != 0 && state.receiver == Some(owner))
        {
            return None;
        }
        self.with_host(|host| Ok(crate::fes_task_platform::button_target_lookup(host, START_REGION)))
            .ok().flatten().filter(|(receiver, target)| {
                *receiver == owner && target.as_ref().is_some_and(|target|
                    target.name.as_slice() == b"btn_def" && target.bank == 1)
            }).map(|_| (flow, owner))
    }

    /// The current page and its actual enabled keyboard receiver.
    pub fn host_hatsumira_12_page(&self, group: i32, record: i32) -> Result<HatsumiraPage, &'static str> {
        let (expected, body) = Self::host_hatsumira_12_op001_records(group)
            .ok_or("unsupported CST group")?;
        if record != expected {
            return Err("unsupported CST position");
        }
        self.hatsumira_page("op001", group, record, body, CstSourceProbe::Hatsumira12)
    }

    pub fn host_hatsumira_12_post_op(&self, group: i32) -> Result<HatsumiraPage, &'static str> {
        let (wait, body) = Self::host_hatsumira_12_post_op_records(group).ok_or("unsupported post-OP group")?;
        self.hatsumira_page("OPMOVIE_END", group, wait, body, CstSourceProbe::Hatsumira12PostOp)
    }

    fn hatsumira_page(&self, scene: &str, group: i32, record: i32, body: usize, probe: CstSourceProbe)
        -> Result<HatsumiraPage, &'static str>
    {
        quiet(self).ok_or("root wait or input state")?;
        let frame = self.state.clock.frame;
        let mut contexts = self.cst_contexts.iter().filter(|(_, context)| context.document.is_some());
        let (&handle, context) = contexts.next().ok_or("no loaded CST")?;
        if contexts.next().is_some() || !context.story.is_scene(scene)
            || context.localization.is_some() || context.encoding != ScriptTextEncoding::Cp932
        {
            return Err("CST context identity");
        }
        let (marked, actual_group, actual_record) = context.story.settled_body_record(frame)
            .ok_or("no settled kind-2 CST wait")?;
        if (actual_group, actual_record) != (group, record) {
            return Err("unexpected CST wait");
        }
        let document = context.document.as_ref().ok_or("missing op001 CST document")?;
        let group_records = document.inputs.get(group as usize).ok_or("missing op001 CST group")?;
        if !group_records.contains(&body) || !group_records.contains(&(record as usize))
            || !document.records.get(body).is_some_and(|row| row.kind == 0x20 && !row.text.is_empty())
            || !document.records.get(record as usize).is_some_and(|row| row.kind == 0x02 && row.text.is_empty())
        {
            return Err("op001 body or wait record changed");
        }
        let (flow, flow_driver) = window(self, b"flow", b"LOOP_GAME", false).ok_or("flow/LOOP_GAME")?;
        let (owner, driver) = window(self, b"meswnd", b"WAIT_USER", true).ok_or("meswnd/WAIT_USER/WAIT_KEY")?;
        let services = &self.data_services;
        let receiver = *self.task_parents.get(&owner).ok_or("missing message parent")?;
        if !live_child(self, flow, 0, 34) || !live_child(self, receiver, flow, 0)
            || !live_child(self, owner, receiver, 34)
            || self.task_vms.get(&receiver).and_then(|vm| vm.program_code_sha256()) != Some(SSCRIPT_CODE)
            || !services.tasks.task_clock(receiver).is_some_and(|clock| clock.paused == 0)
        {
            return Err("sscript lifetime or code identity");
        }
        let generation = services.fes_text.story_body_generation(owner).ok_or("missing body generation")?;
        if !current_committed_body(generation, context.story.loaded, marked, frame) {
            return Err("body generation is not naturally complete and committed");
        }
        let origins = &services.cst_text_origins;
        let [source] = services.fes_text.story_body_sources(owner, generation.source_epoch, origins)
            .ok_or("missing current body origin")? else {
            return Err("multiple or empty body origins");
        };
        if (source.context_handle, source.group, source.absolute_record) != (handle, group, body)
            || source.load_id == 0
        {
            return Err("body origin is not the expected CST record");
        }
        let read = origins.source_record_observations(handle).iter().find(|entry| {
            entry.probe == probe && entry.source == *source
                && entry.native == 327 && entry.kind == 0x20 && entry.body_tagged && !entry.localized
                && entry.stored_codec == ScriptTextEncoding::Cp932 && entry.output_codec == ScriptTextEncoding::Cp932
                && entry.raw_bytes != 0 && entry.raw_bytes == entry.output_bytes
                && context.story.loaded <= entry.frame && entry.frame <= generation.source_frame
        }).ok_or("no matching native 327 source read")?;
        if !body_submitted(self, owner)
            || !verified_fes(self, "flow", FLOW, flow_driver)
            || !verified_fes(self, "meswnd", MESWND, driver)
        {
            return Err("body composition or FES identity");
        }
        let region = bottom_button(self, receiver).ok_or("no enabled bottom-button Enter route")?;
        Ok(HatsumiraPage {
            owner, region, receiver, context: handle, load: source.load_id, loaded: context.story.loaded, marked,
            epoch: generation.source_epoch, source_frame: generation.source_frame,
            read_frame: read.frame, complete: generation.natural_complete.ok_or("missing natural completion")?.1,
        })
    }
}

fn quiet(player: &HeadlessPlayer) -> Option<()> {
    let services = &player.data_services;
    let input = &services.messages;
    let vm = player.vm.as_ref()?;
    (services.story_trace && player.host_story_pe_sha256() == Some(PE)
        && player.default_entry() == Some(STARTUP) && player.loaded.as_ref()?.name == STARTUP
        && vm.program_code_sha256()? == MAIN_CODE
        && !player.frame_in_progress() && player.state.input.focused && player.state.input.pressed.is_empty()
        && !services.host_pause.paused && !services.host_quit_requested && player.movie_playbacks.is_empty()
        && player.pending_messages.is_empty() && player.pending_fes_events.is_empty()
        && services.pending_host_input.is_empty() && services.tasks.pending_root_dispatch().is_none()
        && player.message_wait_address == Some(0xc29) && vm.waiting_for_message()
        && player.input_task.alive && input.device_keys_loaded && input.device_foreground
        && input.physical_buttons.is_empty() && input.physical_edges.is_empty() && input.pending_device_buttons.is_empty()
        && input.key_dispatch.is_none() && input.key_selection.is_none()
        && input.button_dispatches.is_empty() && input.button_active.is_empty()).then_some(())
}

fn window<'a>(player: &'a HeadlessPlayer, name: &[u8], block: &[u8], wait_key: bool) -> Option<(u32, &'a FesScriptDriver)> {
    let fes = &player.data_services.tasks.fes;
    if fes.runtimes().count() != 2 {
        return None;
    }
    let mut matches = fes.story_runtimes().filter(|(_, actual, _, _)| actual.strip_suffix(b".fes").unwrap_or(actual) == name);
    let (owner, _, runtime, actual_wait_key) = matches.next()?;
    let driver = runtime.driver.as_ref()?;
    (matches.next().is_none() && actual_wait_key == wait_key
        && settled_poll(driver.interpreter.current_block(), block,
            driver.has_pending_handler() || driver.initialization_pending(), driver.waiting(), player.state.clock.frame))
        .then_some((owner, driver))
}

fn verified_fes(player: &HeadlessPlayer, name: &str, expected: &str, driver: &FesScriptDriver) -> bool {
    let resources = player.resources();
    resources.category_names(b"fes", name).ok()
        .and_then(|names| names.iter().find_map(|name| resources.find(name)))
        .and_then(|candidate| resources.read_candidate(candidate).ok())
        .filter(|bytes| sha256(bytes) == expected)
        .and_then(|bytes| FesProgram::parse(&bytes).ok())
        .is_some_and(|program| program == driver.interpreter.program)
}

fn body_submitted(player: &HeadlessPlayer, owner: u32) -> bool {
    let key = (owner, "\0text:737472[0]".to_owned());
    let mut lists = player.data_services.tasks.fes.platform.lists.values()
        .filter_map(|list| list.layers.get(&key).map(|layer| (list, layer)));
    let Some((list, layer)) = lists.next() else { return false; };
    lists.next().is_none() && list.enabled && list.submit && player.image_handles.get(&list.output)
        .is_some_and(|output| submitted_body_pixels(layer, output))
}

fn bottom_button(player: &HeadlessPlayer, receiver: u32) -> Option<u32> {
    let input = &player.data_services.messages;
    if input.key_capture.contains_key(&0) || input.key_capture.contains_key(&2)
        || input.typed_physical_owners.contains_key(&(ButtonSource::Keyboard, 13))
        || input.device_keys.forced_queries(0).ok()? != 0 || input.device_keys.forced_queries(2).ok()? != 0
        || !enter_mapping(player.device_key_bindings(), &player.input_task.key_enabled)
    {
        return None;
    }
    let mut buttons = player.scene_nodes.iter().filter(|(_, node)| node.name == "bottom button");
    let (&region, _) = buttons.next()?;
    if buttons.next().is_some() || !live_child(player, region, receiver, 6) {
        return None;
    }
    let mut registrations = player.input_task.regions.iter().filter(|entry| entry.id() == region);
    let words = &registrations.next()?.words;
    let button = input.input_buttons.get(&region)?;
    (registrations.next().is_none() && words[2] & ButtonSource::Keyboard.mask() != 0 && words[8] != 0
        && button.execution_checkpoint().3 && button.state.show != 0 && button.state.receiver == Some(receiver))
        .then_some(region)
}

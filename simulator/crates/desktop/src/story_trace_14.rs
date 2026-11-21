use cs2_runtime::message_object::ButtonSource;

use crate::cst_context_native::CstSourceProbe;
use crate::{HeadlessPlayer, ScriptTextEncoding};

use super::FesDriverWait;
use super::current_committed_body;
use super::story_trace_09::{body_and_prompt_submitted, enter_mapping, live_child, settled_poll};

const ROOT_WAIT: u32 = 0x918;
pub(crate) const PE: &str = "9f3a5ad0ed22074420685f1e2a3bc92436d68289c7bac854d5f7d7331dc3ef67";
pub(crate) const OP_CST: &str = "2596c7c430dabed8e1e3144c50c4b8672b59b1d452e4b92b7e18cf0c9f409817";
const STARTUP: &str = "kcs.int/main.kcs";
fn op_body_record(group: i32, wait: i32) -> Option<usize> {
    match (group, wait) {
        (0, 6) => Some(5),
        (1, 8) => Some(7),
        (2, 78) => Some(77),
        (3, 80) => Some(79),
        (4, 82) => Some(81),
        (5, 115) => Some(114),
        (6, 124) => Some(123),
        (7, 126) => Some(125),
        (8, 130) => Some(129),
        _ => None,
    }
}

const TITLE: u32 = 11;
const TITLE_REGION: u32 = 37;
const RECEIVER: u32 = 8;
const BOTTOM: u32 = 61;
const TOP: u32 = 6;
const MESWND: u32 = 12;
const KEYBOARD_CHAIN: [(u32, u32); 11] = [
    (3, 0), (26, MESWND), (24, MESWND), (23, MESWND), (21, MESWND),
    (19, MESWND), (16, MESWND), (27, MESWND), (BOTTOM, RECEIVER),
    (28, MESWND), (7, TOP),
];

pub(super) fn profile(pe: &str, startup: Option<&str>) -> bool {
    pe == PE && startup == Some(STARTUP)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Grisaia14Body {
    pub context: u32,
    pub load: u64,
    pub owner: u32,
    pub epoch: u64,
    pub source_frame: u64,
    pub marked: u64,
}

fn quiet(player: &HeadlessPlayer) -> bool {
    let services = &player.data_services;
    let input = &services.messages;
    services.story_trace
        && player.host_story_pe_sha256() == Some(PE)
        && player.default_entry() == Some(STARTUP)
        && player
            .loaded
            .as_ref()
            .is_some_and(|loaded| loaded.name == STARTUP)
        && !player.frame_in_progress()
        && player.state.input.focused
        && player.state.input.pressed.is_empty()
        && !services.host_pause.paused
        && !services.host_quit_requested
        && player.movie_playbacks.is_empty()
        && player.pending_messages.is_empty()
        && player.pending_fes_events.is_empty()
        && services.pending_host_input.is_empty()
        && services.tasks.pending_root_dispatch().is_none()
        && player.message_wait_address == Some(ROOT_WAIT)
        && player
            .vm
            .as_ref()
            .is_some_and(|vm| vm.waiting_for_message())
        && player.input_task.alive
        && input.device_keys_loaded
        && input.device_foreground
        && input.physical_buttons.is_empty()
        && input.physical_edges.is_empty()
        && input.pending_device_buttons.is_empty()
        && input.key_capture.is_empty()
        && input.key_dispatch.is_none()
        && input.key_selection.is_none()
        && input.button_dispatches.is_empty()
        && input.button_active.is_empty()
        && input
            .key_scan
            .records()
            .iter()
            .flatten()
            .all(|key| key.mask == 0 && key.count == 0)
}

impl HeadlessPlayer {
    /// Settled title and its live FES New Game binding before pointer input.
    pub fn host_grisaia_14_title(&self) -> bool {
        if !quiet(self)
            || self
                .cst_contexts
                .values()
                .any(|context| context.document.is_some())
        {
            return false;
        }
        let tasks = &self.data_services.tasks;
        let frame = self.state.clock.frame;
        let mut ready = 0;
        for (id, name, runtime, wait_key) in tasks.fes.story_runtimes() {
            let Some(driver) = runtime.driver.as_ref() else {
                return false;
            };
            let block = match (id, name.strip_suffix(b".fes").unwrap_or(name)) {
                (TOP, b"flow") => b"FES_WAIT".as_slice(),
                (8, b"title_f") => b"FES_WAIT".as_slice(),
                (TITLE, b"title") => b"KEYWAIT".as_slice(),
                _ => return false,
            };
            if wait_key
                || driver.initialization_pending()
                || !settled_poll(
                    driver.interpreter.current_block(),
                    block,
                    driver.has_pending_handler(),
                    driver.waiting(),
                    frame,
                )
            {
                return false;
            }
            ready += 1;
        }
        ready == 3
            && live_child(self, TOP, 0, 34)
            && live_child(self, 8, TOP, 34)
            && live_child(self, TITLE, 8, 34)
            && live_child(self, TITLE_REGION, TITLE, 6)
            && tasks.fes.platform.region_target(TITLE_REGION).is_some_and(|(owner, target)| {
                owner == TITLE && target.is_some_and(|target| target.name.as_slice() == b"btnmap")
            })
            && self.input_button_state(TITLE_REGION).is_none_or(|button| {
                button.receiver == Some(TITLE) && button.show != 0
            })
    }

    /// Resolve the current FES body, original OP CST, exact executed input marker,
    /// and actual root VM wait. Requiring the binding is only necessary before Enter.
    pub fn host_grisaia_14_body(
        &self, group: i32, wait: i32, entry_pressed: u64,
    ) -> Result<Grisaia14Body, &'static str> {
        if !quiet(self) {
            return Err("root VM or input state is not quiet");
        }
        let body = op_body_record(group, wait).ok_or("OP checkpoint is outside the route")?;
        let mut contexts = self
            .cst_contexts
            .iter()
            .filter(|(_, context)| context.document.is_some());
        let (&handle, context) = contexts.next().ok_or("OP CST context missing")?;
        if contexts.next().is_some()
            || context.story.scene != "OP"
            || context.localization.is_some()
            || context.encoding != ScriptTextEncoding::Gbk
        {
            return Err("OP CST identity or encoding differs");
        }
        let frame = self.state.clock.frame;
        let (marked, actual_group, actual_wait) = context.story.settled_body_record(frame)
            .ok_or("executed CST input marker missing or still pending")?;
        if (actual_group, actual_wait) != (group, wait) {
            return Err("active CST input marker differs");
        }

        let tasks = &self.data_services.tasks;
        if tasks.fes.runtimes().count() != 2
            || !live_child(self, TOP, 0, 34)
            || !live_child(self, RECEIVER, TOP, 0)
            || !live_child(self, MESWND, RECEIVER, 34)
            || !tasks
                .task_clock(RECEIVER)
                .is_some_and(|clock| clock.paused == 0)
        {
            return Err("flow, script or meswnd ancestry differs");
        }
        let mut flow_ready = false;
        let mut window_ready = false;
        for (id, name, runtime, _) in tasks.fes.story_runtimes() {
            let pending_driver = runtime.driver.as_ref().ok_or("FES driver missing")?;
            let expected = match (id, name.strip_suffix(b".fes").unwrap_or(name)) {
                (TOP, b"flow") => {
                    flow_ready = true;
                    b"GAME_WAIT".as_slice()
                }
                (MESWND, b"meswnd") => {
                    window_ready = true;
                    b"MES_SHOW".as_slice()
                }
                _ => return Err("unexpected live FES driver"),
            };
            if !settled_poll(
                pending_driver.interpreter.current_block(),
                expected,
                pending_driver.has_pending_handler() || pending_driver.initialization_pending(),
                pending_driver.waiting(),
                frame,
            ) {
                return Err("flow or meswnd has not settled in its expected block");
            }
        }
        if !flow_ready || !window_ready {
            return Err("flow or meswnd driver missing");
        }

        let origins = &self.data_services.cst_text_origins;
        let text = &self.data_services.fes_text;
        let generation = text.story_body_generation(MESWND).ok_or("body generation missing")?;
        // The observation is the first read of this record in the pinned OP load.
        let entry = origins.source_record_observations(handle).iter().find(|entry| {
            (entry.source.context_handle, entry.source.group, entry.source.absolute_record)
                == (handle, group, body)
                && entry.source.load_id != 0
                && entry.probe == CstSourceProbe::Grisaia14
                && entry.native == 327
                && entry.kind == 0x20
                && entry.stored_codec == ScriptTextEncoding::Gbk
                && entry.output_codec == ScriptTextEncoding::Gbk
                && entry.raw_bytes != 0
                && entry.raw_bytes == entry.output_bytes
                && entry.body_tagged
                && !entry.localized
                && (context.story.loaded..=marked).contains(&entry.frame)
        }).ok_or("original GBK CST source observation missing")?;
        // The pinned OP load queues g6 on the consuming Enter and runs r119
        // before reaching r124. The early generation still needs the body match
        // and natural completion after its executed marker.
        let delay = match (group, wait) {
            (5, 115) => Some(1),
            (6, 124) if context.document.as_ref().and_then(|doc| doc.records.get(119))
                .is_some_and(|record| record.kind == 0x30 && record.text == b"wait 30") => Some(30),
            _ => None,
        };
        let queued_before_marker = group == 2 || (generation.source_frame == entry_pressed
            && delay.is_some_and(|frames| entry_pressed.checked_add(frames) == Some(marked)));
        if generation.source_frame < entry_pressed
            || (generation.source_frame < marked && (!queued_before_marker
                || generation.natural_complete.is_none_or(|(_, completed)| completed < marked)))
            || !current_committed_body(generation, context.story.loaded, marked, frame)
        {
            return Err("current body has no natural completion and submission");
        }
        if !body_and_prompt_submitted(self, MESWND) {
            return Err("body or keywait layer did not reach the submitted list");
        }
        let raw = &context.document.as_ref().ok_or("OP document missing")?
            .records.get(body).ok_or("OP body record missing")?.text;
        match text.story_body_sources(MESWND, generation.source_epoch, origins) {
            Some(sources) if sources != [entry.source] =>
                return Err("body provenance differs from the active OP page"),
            Some(_) => {}
            None => {
                // The current bank can lose its tag during FES text handling.
                // Compare every queued text byte from this committed generation.
                if !text.story_body_appends_match_gbk(MESWND, generation.source_epoch, raw) {
                    return Err("current FES body does not match the original OP record");
                }
            }
        }
        Ok(Grisaia14Body {
            context: handle,
            load: entry.source.load_id,
            owner: MESWND,
            epoch: generation.source_epoch,
            source_frame: generation.source_frame,
            marked,
        })
    }

    /// Snapshot the bank even when it failed the route guard.
    pub fn host_grisaia_14_body_diagnostics(&self) -> String {
        let services = &self.data_services;
        let generation = services.fes_text.story_body_generation(MESWND);
        let sources = generation.and_then(|body| {
            services.fes_text.story_body_sources(MESWND, body.source_epoch, &services.cst_text_origins)
        });
        let (_, _, scene, group, wait) = self.host_story_trace();
        let expected = self.cst_contexts.values().find_map(|context| {
            if scene != "OP" || context.story.scene != scene {
                return None;
            }
            let record = op_body_record(group, wait)?;
            let bytes = context.document.as_ref()?.records.get(record)?.text.len();
            let (marked, _, _) = context.story.settled_body_record(self.state.clock.frame)?;
            Some((record, bytes, marked))
        });
        let appends = services.fes_text.story_body_append_diagnostics(MESWND);
        format!("14 body owner={MESWND} generation={generation:?} sources={sources:?} expected_record_bytes_and_marked={expected:?} {appends}")
    }

    /// Read the live scan and bound button at the same completed frame as the OP wait.
    /// Candidate traversal is read-only. Callback consumption is checked after the press.
    pub fn host_grisaia_14_enter_binding(
        &mut self,
        group: usize,
    ) -> anyhow::Result<Result<Vec<(u32, u32)>, String>> {
        if !quiet(self) {
            return Ok(Err("root or input is not quiet".into()));
        }
        let input = &self.data_services.messages;
        let bindings = self.device_key_bindings();
        if !enter_mapping(bindings, &self.input_task.key_enabled)
            || !self.input_task.key_enabled[0]
            || !bindings[0].enabled
            || bindings[0].codes[ButtonSource::Keyboard.index()][0] != 13
            || input
                .typed_physical_owners
                .contains_key(&(ButtonSource::Keyboard, 13))
            || input.device_keys.forced_queries(0)? != 0
            || input.device_keys.forced_queries(2)? != 0
        {
            return Ok(Err("physical Enter mapping or scan differs".into()));
        }
        if !live_child(self, BOTTOM, RECEIVER, 6)
            || !self
                .task_vms
                .get(&RECEIVER)
                .is_some_and(|vm| vm.program().is_some())
            || !self
                .scene_nodes
                .get(&BOTTOM)
                .is_some_and(|node| node.name == "bottom button")
            || !input.input_buttons.get(&BOTTOM).is_some_and(|button| {
                button.execution_checkpoint().3
                    && button.state.receiver == Some(RECEIVER)
                    && button.state.show != 0
            })
        {
            return Ok(Err("bottom button 61 is not registered and shown under script 8".into()));
        }
        let optional_toolbar = (22, MESWND);
        let live_optional_toolbar = live_child(self, optional_toolbar.0, MESWND, 6)
            && self.data_services.tasks.fes.platform.region_target(optional_toolbar.0)
                .is_some_and(|(owner, target)| owner == MESWND && target.is_some());
        self.with_host(|host| {
            let mut actual = Vec::new();
            for region in host.input_task.regions.iter().copied() {
                if !host.device_press_region_eligible(
                    region,
                    ButtonSource::Keyboard.mask(),
                    [0, 0],
                )? {
                    continue;
                }
                let id = region.id();
                if id == BOTTOM && (region.words[1] != 6 || region.words[11] != 0) {
                    return Ok(Err(format!("bottom registration differs: {:?}", region.words)));
                }
                actual.push((id, host.effective_input_region_receiver(id)));
            }
            if !matches!(group, 6 | 7) {
                // Before g6 the full traversal has already been verified in replay.
                return Ok(if actual.iter().map(|&(id, _)| id).eq(KEYBOARD_CHAIN.map(|(id, _)| id)) {
                    Ok(KEYBOARD_CHAIN.to_vec())
                } else {
                    Err(format!("historical candidate chain differs: {actual:?}"))
                });
            }
            // Toolbar eligibility can change between pages. Retain the live order
            // but only arm when every candidate has a previously verified receiver.
            let mut chain = Vec::new();
            for &(id, receiver) in &actual {
                let Some(receiver) = receiver else {
                    return Ok(Err(format!("candidate without a live receiver: {actual:?}")));
                };
                let pair = (id, receiver);
                let known = KEYBOARD_CHAIN.contains(&pair)
                    || (pair == optional_toolbar && live_optional_toolbar);
                if !known || chain.contains(&pair) {
                    return Ok(Err(format!("unexpected or duplicate candidate: {actual:?}")));
                }
                chain.push(pair);
            }
            Ok(if chain.contains(&(BOTTOM, RECEIVER)) {
                Ok(chain)
            } else {
                Err(format!("bottom button is not keyboard eligible: {actual:?}"))
            })
        })
    }
}

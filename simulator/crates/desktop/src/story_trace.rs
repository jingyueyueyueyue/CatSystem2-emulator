use super::HeadlessPlayer;
use super::cst_context_native::{CstBodySource, body_control_only};
use super::fes_driver::{FesDriverWait, FesScriptDriver};
use super::fes_task_platform::Layer;
use super::fes_text::StoryBodyGeneration;
use super::{fes_driver, particle_plugin_digest};
use cs2_renderer::RgbaFrame;
use cs2_script::CstDocument;

#[path = "story_trace_08.rs"]
pub(crate) mod story_trace_08;
#[path = "story_trace_09.rs"]
mod story_trace_09;
#[path = "story_trace_10.rs"]
pub(crate) mod story_trace_10;
#[path = "story_trace_12.rs"]
pub(super) mod story_trace_12;
#[path = "story_trace_20.rs"]
mod story_trace_20;
#[path = "story_trace_14.rs"]
pub(super) mod story_trace_14;
#[path = "story_trace_15.rs"]
pub(super) mod story_trace_15;
#[path = "story_trace_18.rs"]
pub(super) mod story_trace_18;
#[path = "legacy_story_wait.rs"]
pub(crate) mod legacy_story_wait;
#[path = "story_trace_daydream_ordinary.rs"]
mod story_trace_daydream_ordinary;

pub(super) const DAYDREAM_PE: &str = "1f3288d5fd5408bee4d395faab708aca78c9eeaa6c172dc06dcce09c844ba9d5";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StoryTraceProfile {
    Island,
    Daydream,
    Nekopara,
    Rebellions,
    KanmiObservation,
    Eyecatch08Observation,
    LegacyFesBodyObservation,
    Legacy07Observation,
    Djibril10Observation,
    Legacy11Observation,
    Legacy13Observation,
    Grisaia14Observation,
    Campanella18Observation,
    Labyrinth15Observation,
    Hatsumira12Observation,
    FirstBodyObservation,
}

#[derive(Clone, Copy)]
struct BodyWaitShell<'a> {
    window: &'static str,
    owner: u32,
    scene: &'a str,
    context: u32,
    loaded: u64,
    marked: u64,
    group: i32,
    record: i32,
}

fn verified_observation_profile(kanmi_pe: Option<&str>) -> Option<StoryTraceProfile> {
    kanmi_pe.is_some().then_some(StoryTraceProfile::KanmiObservation)
}

fn story_trace_profile(pe_sha256: &str, startup: Option<&str>) -> Option<StoryTraceProfile> {
    if story_trace_08::is_profile(pe_sha256, startup) {
        return Some(StoryTraceProfile::Eyecatch08Observation);
    }
    if pe_sha256 == story_trace_10::PE && startup == Some("kcs.int/main.kcs") {
        return Some(StoryTraceProfile::Djibril10Observation);
    }
    if story_trace_14::profile(pe_sha256, startup) {
        return Some(StoryTraceProfile::Grisaia14Observation);
    }
    if pe_sha256 == story_trace_18::PE && startup == Some("kcs.int/main.kcs") {
        return Some(StoryTraceProfile::Campanella18Observation);
    }
    if pe_sha256 == story_trace_12::PE && startup == Some(story_trace_12::STARTUP) {
        return Some(StoryTraceProfile::Hatsumira12Observation);
    }
    if pe_sha256 == story_trace_15::PE && startup == Some("kcs.int/main.kcs") {
        return Some(StoryTraceProfile::Labyrinth15Observation);
    }
    if legacy_story_wait::legacy_body_profile(pe_sha256, startup).is_some() {
        return Some(StoryTraceProfile::LegacyFesBodyObservation);
    }
    match (pe_sha256, startup) {
        (
            "ad70ab1bb00cc8ff9e62a333d12be2e700c1d57880fec0a6b0f8e41cad93f446"
            | "a9f8b08713aac278671853b2293a13b2623a122ede9341cb921901b09601cb52",
            Some("kcs.int/main.kcs"),
        ) => Some(StoryTraceProfile::FirstBodyObservation),
        (
            "474cc11d875d7be9df9581f3a55d64698ccf4e760c44013df403cced85e505f1"
            | "7bd9f6dcc7d62868d4ad9cc68d5580452e71bd907540ec4f993d960d051d2c45",
            _,
        ) => Some(StoryTraceProfile::Island),
        (
            DAYDREAM_PE,
            Some("kcs.int/main.kcs"),
        ) => Some(StoryTraceProfile::Daydream),
        (
            "c71c5b85e6eca660474f156d96b0021a53a5f05690dd077d7861d72ff5b11adc",
            Some("kcs.int/main.kcs"),
        ) => Some(StoryTraceProfile::Nekopara),
        (
            "a099ac1cee7af60c8f32869485c9df4aab344b5eb91c1018ae8bb7df2c1f39f3",
            Some("kcs.int/main.kcs"),
        ) => Some(StoryTraceProfile::Rebellions),
        (
            "f6af7437475b8c066be973245aa79688bea510ca1192264add4f1914ebee4c0e",
            Some("kcs.int/main.kcs"),
        ) => Some(StoryTraceProfile::Legacy07Observation),
        (
            "4ccb48ad4bb6bfb5a15a45b8e1f2c2e62072cfad9f1487f869561a473770ed94",
            Some("kcs.int/main.kcs"),
        ) => Some(StoryTraceProfile::Legacy11Observation),
        (
            "c2af2ee9baf4b26a3c2cf8a1c867f15f92b54ad1ac5fac381858046a47818f6c",
            Some("kcs.int/main.kcs"),
        ) => Some(StoryTraceProfile::Legacy13Observation),
        _ => None,
    }
}

#[derive(Clone, Debug, Default)]
pub(super) struct CstStoryPosition {
    scene: String,
    loaded: u64,
    candidate: Option<(u64, u32, i32, i32)>,
    page: Option<(u64, u32, i32, i32)>,
}

impl CstStoryPosition {
    pub(super) fn scene_name(&self) -> &str {
        &self.scene
    }

    pub(super) fn is_scene(&self, name: &str) -> bool {
        self.scene == name
    }

    pub(super) fn loaded_at(&self) -> u64 {
        self.loaded
    }

    pub(super) fn loaded(name: &[u8], frame: u64) -> Self {
        let name = name
            .rsplit(|byte| matches!(byte, b'/' | b'\\'))
            .next()
            .unwrap_or_default();
        let name = name
            .strip_suffix(b".cst")
            .or_else(|| name.strip_suffix(b".CST"))
            .unwrap_or(name);
        let scene = if name.len() <= 256
            && name != b"-"
            && name
                .iter()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(byte))
        {
            String::from_utf8_lossy(name).into_owned()
        } else {
            String::new()
        };
        Self {
            scene,
            loaded: frame,
            candidate: None,
            page: None,
        }
    }

    pub(super) fn observe_wait(&mut self, frame: u64, kind: u32, group: i32, record: i32) {
        self.candidate = Some((frame, kind, group, record));
    }

    pub(super) fn observe_page(&mut self, frame: u64, kind: u32) {
        self.page = self
            .candidate
            .take()
            .and_then(|(observed, query_kind, group, record)| {
                (observed == frame && observed >= self.loaded && query_kind == kind)
                    .then_some((frame, kind, group, record))
            });
    }

    fn observation_only(&self, frame: u64) -> (u64, &'static str, &str, i32, i32) {
        let (group, record) = self
            .page
            .filter(|(marked, _, group, record)| *marked < frame && *group >= 0 && *record >= 0)
            .map_or((-1, -1), |(_, _, group, record)| (group, record));
        (frame, "active", &self.scene, group, record)
    }

    fn settled_body_record(&self, frame: u64) -> Option<(u64, i32, i32)> {
        let (marked, kind, group, record) = self.page?;
        (self.candidate.is_none() && kind == 0x02 && self.loaded <= marked && marked < frame
            && group >= 0 && record >= 0).then_some((marked, group, record))
    }

    fn island_replay_wait(&self, frame: u64) -> Option<(u32, i32, i32, bool)> {
        let (marked, kind, group, record) = self.page?;
        if let Some((observed, next_kind, next_group, next_record)) = self.candidate
            && (next_kind, next_group, next_record) != (kind, group, record)
        {
            return (kind == 0x03 && next_kind == 0x02
                && marked < observed && observed < frame
                && next_group > group && next_record > record)
                .then_some((next_kind, next_group, next_record, true));
        }
        (marked < frame).then_some((kind, group, record, false))
    }
}

impl HeadlessPlayer {
    fn host_story_pe_sha256(&self) -> Option<&str> {
        self.scan.engine.primary_executable.as_ref().map(|binary| binary.sha256.as_str())
    }

    fn host_story_trace_profile(&self) -> Option<StoryTraceProfile> {
        verified_observation_profile(self.verified_kanmi_runtime_pe_sha256()).or_else(|| {
            story_trace_profile(self.host_story_pe_sha256()?, self.default_entry())
        })
    }

    pub fn enable_host_story_trace(&mut self) {
        self.data_services.story_trace = self.host_story_trace_profile().is_some();
    }

    pub fn host_story_trace(&self) -> (u64, &'static str, &str, i32, i32) {
        self.host_replay_story_position().0
    }

    pub fn host_replay_story_position(&self) -> ((u64, &'static str, &str, i32, i32), Option<(&'static str, u32)>) {
        let mut window = None;
        let position = self.host_story_trace_position(&mut window);
        (position, window)
    }

    pub fn host_replay_eye08_cst_load(&self) -> Option<(u32, u64, u64)> {
        let mut scenes = self.cst_contexts.iter().filter(|(_, context)| !context.story.scene.is_empty());
        let (&handle, context) = scenes.next()?;
        if scenes.next().is_some() || !context.story.is_scene("c01_01") || context.localization.is_some() {
            return None;
        }
        let load = self.data_services.cst_text_origins.eye08_load(handle)?;
        Some((handle, load, context.story.loaded))
    }

    pub fn host_replay_eye08_owner(&self) -> Option<u32> {
        self.data_services.story_trace.then_some(())?;
        story_trace_08::observe(self, self.state.clock.frame).map(|(_, owner)| owner)
    }

    pub fn host_replay_kanmi_position(&self) -> Option<((&'static str, &'static str, i32, i32), (&'static str, u32))> {
        if let Some(owner) = self.host_kanmi_exact_title_owner() {
            return Some((("title", "-", -1, -1), ("title", owner)));
        }
        let page = story_trace_09::observe(self, self.state.clock.frame)?;
        Some((("page", page.scene, page.group, page.record), (page.window, page.owner)))
    }

    pub fn host_replay_eye08_entry_owner(&self, card: bool) -> Option<u32> {
        let frame = self.state.clock.frame;
        if !self.data_services.story_trace || self.host_story_trace_profile() != Some(StoryTraceProfile::Eyecatch08Observation)
            || self.frame_in_progress() || !self.state.input.focused || !self.state.input.pressed.is_empty()
            || self.data_services.host_pause.paused || self.data_services.host_quit_requested
            || !self.movie_playbacks.is_empty() || !self.pending_messages.is_empty() || !self.pending_fes_events.is_empty()
            || !self.data_services.pending_host_input.is_empty() || !self.cst_contexts.is_empty()
            || self.message_wait_address != Some(0x32f) || !self.vm.as_ref()?.waiting_for_message()
        {
            return None;
        }
        let (member, node_name, region, bank) = if card {
            (b"storysel".as_slice(), "card", 32, 2)
        } else {
            (b"title".as_slice(), "btn", 13, 0)
        };
        let mut flow = None;
        let mut target = None;
        for (owner, name, runtime, wait_key) in self.data_services.tasks.fes.story_runtimes() {
            let name = name.strip_suffix(b".fes").unwrap_or(name);
            let driver = runtime.driver.as_ref()?;
            if wait_key || driver.has_pending_handler() || driver.initialization_pending()
                || !matches!(driver.waiting(), Some(FesDriverWait::Frame { after }) if *after == frame)
            {
                return None;
            }
            let block = driver.interpreter.current_block();
            if name == b"flow" && block == (if card { b"SS_WAIT".as_slice() } else { b"TITLE_WAIT" })
                && flow.replace(owner).is_none()
            {
                continue;
            }
            if name != member || block != b"WAIT" || target.replace(owner).is_some() {
                return None;
            }
        }
        let (flow, owner) = (flow?, target?);
        let node = self.scene_nodes.get(&region)?;
        let (receiver, target) = self.data_services.tasks.fes.platform.region_target(region)?;
        let target = target?;
        (story_trace_09::live_child(self, owner, flow, 34)
            && story_trace_09::live_child(self, region, owner, 6)
            && node.name == node_name && target.name == node_name.as_bytes() && target.bank == bank
            && receiver == owner).then_some(owner)
    }

    fn host_story_trace_position(&self, window: &mut Option<(&'static str, u32)>) -> (u64, &'static str, &str, i32, i32) {
        let frame = self.state.clock.frame;
        let unknown = (frame, "unknown", "-", -1, -1);
        if !self.data_services.story_trace {
            return unknown;
        }
        let Some(profile) = self.host_story_trace_profile() else {
            return unknown;
        };
        if self.data_services.host_quit_requested {
            return if profile == StoryTraceProfile::Island {
                (frame, "ended", "-", -1, -1)
            } else {
                unknown
            };
        }
        if profile == StoryTraceProfile::Rebellions {
            return if self.rebellions_title_ready(frame) {
                (frame, "title", "-", -1, -1)
            } else if self.rebellions_episode_ready(frame) {
                (frame, "choice", "epselect", -1, -1)
            } else {
                unknown
            };
        }
        if profile == StoryTraceProfile::Nekopara && self.cst_contexts.is_empty() {
            if let Some(owner) = self.host_neko_startup_owner(NekoStartupScreen::Language, frame) {
                *window = Some(("langselect", owner));
                return (frame, "language_select", "langselect", -1, -1);
            }
            if let Some(owner) = self.host_neko_startup_owner(NekoStartupScreen::Title, frame) {
                *window = Some(("title", owner));
                return (frame, "title", "-", -1, -1);
            }
        }
        let mut story = self
            .cst_contexts
            .values()
            .map(|context| &context.story)
            .filter(|story| !story.scene.is_empty())
            .filter(|story| story.loaded <= frame);
        let Some(position) = story.next() else {
            return unknown;
        };
        if story.next().is_some() {
            return unknown;
        }
        if matches!(profile, StoryTraceProfile::LegacyFesBodyObservation | StoryTraceProfile::Legacy07Observation
            | StoryTraceProfile::Legacy11Observation | StoryTraceProfile::Legacy13Observation
            | StoryTraceProfile::Djibril10Observation
            | StoryTraceProfile::Grisaia14Observation | StoryTraceProfile::Hatsumira12Observation
            | StoryTraceProfile::FirstBodyObservation | StoryTraceProfile::Labyrinth15Observation)
        {
            return position.observation_only(frame);
        }
        let scene = position.scene.as_str();
        let active = (frame, "active", scene, -1, -1);
        if !self.state.input.focused
            || self.data_services.host_pause.paused
            || !self.movie_playbacks.is_empty()
        {
            return active;
        }
        if profile != StoryTraceProfile::Island {
            if self.data_services.tasks.fes.story_runtimes().any(|(_, name, _, _)| {
                name.strip_suffix(b".fes").unwrap_or(name).eq_ignore_ascii_case(b"title")
            }) {
                return unknown;
            }
            if let Some(shell) = self.host_body_wait_shell(frame) {
                if profile == StoryTraceProfile::Daydream
                    && (shell.scene, shell.group, shell.record) == ("prologue", 20, 96)
                {
                    if self.cst_contexts.get(&shell.context).and_then(|context| context.document.as_ref())
                        .is_some_and(daydream_r96_probe_document)
                    {
                        *window = Some((shell.window, shell.owner));
                        return (frame, "page_probe", shell.scene, shell.group, shell.record);
                    }
                    return position.observation_only(frame);
                }
                if self.host_current_body(shell, frame).2 {
                    *window = Some((shell.window, shell.owner));
                    return (frame, "page", shell.scene, shell.group, shell.record);
                }
            }
            return position.observation_only(frame);
        }
        let replay_wait = position.island_replay_wait(frame);
        let mut ready_windows = 0;
        let mut live_text_windows = 0;
        let mut fullwnd_present = false;
        let mut fullwnd_probe_owner = None;
        let mut settled_window = None;
        let mut selecting = false;
        let mut novel_prompt = false;
        let mut prefetched_window_ready = false;
        for (owner, name, runtime, wait_key) in self.data_services.tasks.fes.story_runtimes() {
            let name = name.strip_suffix(b".fes").unwrap_or(name);
            if name.eq_ignore_ascii_case(b"title") {
                return unknown;
            }
            let fullwnd = name.eq_ignore_ascii_case(b"mes_nownd");
            fullwnd_present |= fullwnd;
            if fullwnd || [b"meswnd".as_slice(), b"fullwnd", b"novelwnd"]
                .into_iter()
                .any(|window| name.eq_ignore_ascii_case(window))
            {
                live_text_windows += 1;
            }
            let Some(driver) = &runtime.driver else {
                continue;
            };
            let block = driver.interpreter.current_block();
            if fullwnd {
                if island_fullwnd_probe_ready(
                    replay_wait, block, wait_key, driver.has_pending_handler(), driver.waiting(), frame,
                ) {
                    fullwnd_probe_owner = Some(owner);
                }
                continue;
            }
            if name.eq_ignore_ascii_case(b"select") {
                selecting = true;
                if !driver.has_pending_handler()
                    && block == b"WAIT_USER"
                    && runtime
                        .media
                        .values()
                        .any(|layer| layer.kind == "BUTTON" && layer.enabled)
                {
                    if scene == "com01_selC" && self.task_parents.get(&owner) == Some(&8) {
                        *window = Some(("select", owner));
                    }
                    return (frame, "choice", scene, -1, -1);
                }
            }
            let novel_state = replay_wait.and_then(|(kind, _, _, prefetched)| {
                if !name.eq_ignore_ascii_case(b"novelwnd")
                    || driver.has_pending_handler()
                    || !matches!(driver.waiting(), Some(FesDriverWait::Frame { .. }))
                    || !self.data_services.fes_text.story_novel_segment_ready(owner)
                {
                    return None;
                }
                if prefetched {
                    let ready = prefetched_novel_input_ready(block, wait_key, driver.waiting(), frame);
                    prefetched_window_ready |= ready;
                    return ready.then_some("page");
                }
                let locals = driver.interpreter.local_values();
                let icon_visibility = |name| runtime.media.get(name).map(|layer| layer.visible);
                let hidden_draw = position.candidate.is_none()
                    && matches!(driver.waiting(), Some(FesDriverWait::Frame { after }) if *after == frame)
                    && settled_novel_draw_without_icons(
                        locals.get(30).copied(), icon_visibility("img_wait[1]"), icon_visibility("img_anim"),
                    );
                novel_wait_state(
                    kind,
                    block,
                    wait_key,
                    locals.get(33).copied(),
                    runtime.media.get("img_wait[1]").is_some_and(|layer| layer.visible && layer.opacity > 0),
                    hidden_draw,
                )
            });
            novel_prompt |= novel_state == Some("page_prompt");
            let message_ready = message_window_ready(name, block, wait_key, driver.has_pending_handler());
            if novel_state.is_some() || message_ready {
                ready_windows += 1;
            }
            if novel_state.is_some() && replay_wait.is_some_and(|(_, _, _, prefetched)| !prefetched) {
                settled_window = Some(("novelwnd", owner));
            } else if message_ready && name.eq_ignore_ascii_case(b"meswnd") {
                settled_window = Some(("meswnd", owner));
            }
        }
        if selecting
            || self.message_wait_address.is_none()
            || !self.pending_messages.is_empty()
            || !self.vm.as_ref().is_some_and(|vm| vm.waiting_for_message())
        {
            return active;
        }
        if fullwnd_present {
            if live_text_windows == 1 && let Some(owner) = fullwnd_probe_owner
                && let Some((_, group, record, _)) = replay_wait
            {
                *window = Some(("fullwnd", owner));
                return (frame, "page_probe", scene, group, record);
            }
            return active;
        }
        if ready_windows != 1 {
            return active;
        }
        if let Some((_, group, record, prefetched)) = replay_wait {
            if !prefetched || prefetched_window_ready {
                *window = (live_text_windows == 1).then_some(settled_window).flatten();
                return (frame, if novel_prompt { "page_prompt" } else { "page" }, scene, group, record);
            }
        }
        active
    }

    pub fn host_island_meswnd_anchor(
        &self, expected_scene: &str, anchor: (i32, i32), next: Option<(i32, i32)>, owner: u32,
    ) -> Option<(u32, u64, u32)> {
        let ((frame, state, scene, group, record), window) = self.host_replay_story_position();
        if self.host_story_pe_sha256() != Some("474cc11d875d7be9df9581f3a55d64698ccf4e760c44013df403cced85e505f1")
            || self.default_entry() != Some("kcs.int/main.kcs")
            || !matches!(expected_scene, "com01" | "OPMOVIE_END" | "com01_selA_1"
                | "com01_flowA_1" | "com01_flowA" | "com01_flowB_1" | "com01_selB_1"
                | "com01_selC_1" | "BAD_02")
            || (state, scene, group, record) != ("page", expected_scene, anchor.0, anchor.1)
            || window != Some(("meswnd", owner)) || self.host_replay_menu_ready().is_some()
            || self.task_parents.get(&8) != Some(&6) || self.task_parents.get(&owner) != Some(&8)
            || self.data_services.tasks.task_property(8).is_none()
            || !self.data_services.tasks.task_is_registered(8) || !self.data_services.tasks.task_is_registered(owner)
            || self.frame_in_progress()
            || self.message_wait_address != Some(0xc29)
            || !self.data_services.pending_host_input.is_empty() || !self.pending_fes_events.is_empty()
        {
            return None;
        }
        let runtime = self.data_services.tasks.fes.runtime(owner)?;
        let driver = runtime.driver.as_ref()?;
        if driver.interpreter.current_block() != b"MES_SHOW" || driver.has_pending_handler()
            || !matches!(driver.waiting(), Some(FesDriverWait::Frame { after }) if *after == frame)
        {
            return None;
        }
        let mut contexts = self.cst_contexts.iter().filter(|(_, context)| !context.story.scene.is_empty());
        let (&handle, context) = contexts.next()?;
        let (_, live_group, live_record) = context.story.settled_body_record(frame)?;
        if contexts.next().is_some() || context.story.scene != scene || (live_group, live_record) != anchor {
            return None;
        }
        let document = context.document.as_ref()?;
        if !adjacent_cst_input_waits(document, anchor, next) {
            return None;
        }
        if expected_scene == "com01" && anchor == (121, 810)
            && next.is_none() && !island_com01_has_opmovie_exit(document)
        {
            return None;
        }
        if next.is_none() {
            let tail: Option<&[(usize, &[u8])]> = match (expected_scene, anchor) {
                ("OPMOVIE_END", (332, 2063)) => Some(&[
                    (2064, b"frameoff"), (2065, b"wait"), (2071, b"rwipe wipe01 30"),
                    (2072, b"wait"), (2073, b"next com01B"),
                ]),
                ("com01_selA_1", (22, 94)) => Some(&[
                    (95, b"if((#525&(1<<4)) == 0) next com01_flowA_1"), (96, b"next com01_flowA_2"),
                ]),
                ("com01_flowA_1", (0, 1)) => Some(&[(2, b"next com01_flowA")]),
                ("com01_flowA", (2, 5)) => Some(&[
                    (7, b"if((#525&(1<<4)) == 0) next com01_flowB_1"), (8, b"next com01_flowB_2"),
                ]),
                ("com01_flowB_1", (0, 1)) => Some(&[(2, b"next com01_selB")]),
                ("com01_selB_1", (51, 361)) => Some(&[(362, b"next com01_selC")]),
                ("com01_selC_1", (5, 14)) => Some(&[(15, b"#723 = (#723|1)"), (16, b"next BAD_02")]),
                _ => None,
            };
            if let Some(expected) = tail {
                let group = usize::try_from(anchor.0).ok()?.checked_add(1)?;
                let range = document.inputs.get(group)?;
                if !island_cst_tail_matches(document, range, expected) {
                    return None;
                }
            }
        }
        Some((handle, context.story.loaded, 8))
    }

    pub fn host_island_beach_fullwnd(&self, origin_load: u64) -> bool {
        let ((frame, state, scene, group, record), window) = self.host_replay_story_position();
        (state, scene, group, record) == ("page_probe", "com01_selB_1", 0, 26)
            && window.is_some_and(|(name, owner)| {
                name == "fullwnd" && self.task_parents.get(&owner) == Some(&8)
                    && self.data_services.tasks.task_is_registered(owner)
            })
            && self.movie_playbacks.is_empty()
            && self.cst_contexts.values().any(|context| {
                context.story.scene == scene && context.story.loaded > origin_load
                    && context.story.settled_body_record(frame).is_some_and(|(_, g, r)| (g, r) == (0, 26))
                    && context.document.as_ref().is_some_and(|document| {
                        adjacent_cst_input_waits(document, (0, 26), Some((1, 67)))
                    })
            })
    }

    pub fn host_island_beach_choice_loaded(&self, context: u32, previous_load: u64) -> bool {
        let ((frame, state, scene, _, _), window) = self.host_replay_story_position();
        (state, scene) == ("choice", "com01_selC")
            && self.movie_playbacks.is_empty()
            && self.pending_messages.is_empty()
            && self.message_wait_address == Some(0xc29)
            && self.vm.as_ref().is_some_and(|vm| vm.waiting_for_message())
            && window.is_some_and(|(name, owner)| {
                name == "select" && self.data_services.tasks.task_is_registered(owner)
                    && self.data_services.tasks.fes.runtime(owner).and_then(|runtime| runtime.driver.as_ref())
                        .is_some_and(|driver| matches!(driver.waiting(), Some(FesDriverWait::Frame { after }) if *after == frame))
            })
            && self.cst_contexts.get(&context).is_some_and(|context| {
                context.story.scene == scene && context.story.loaded > previous_load
            })
    }

    pub fn host_island_bad02_choice(&mut self) -> anyhow::Result<Option<(u32, u64, u32, u32)>> {
        let ((_, state, scene, _, _), window) = self.host_replay_story_position();
        let Some(("select", owner)) = window else { return Ok(None); };
        let mut contexts = self.cst_contexts.iter().filter(|(_, context)| !context.story.scene.is_empty());
        let Some((&handle, context)) = contexts.next() else { return Ok(None); };
        let loaded = context.story.loaded;
        if (state, scene) != ("choice", "com01_selC") || contexts.next().is_some()
            || self.host_replay_menu_ready().is_some() || self.frame_in_progress()
            || !self.pending_fes_events.is_empty() || !self.data_services.pending_host_input.is_empty()
            || !self.host_island_beach_choice_loaded(handle, loaded.saturating_sub(1))
            || self.data_services.tasks.fes.story_runtimes().filter(|(_, name, _, _)| {
                name.strip_suffix(b".fes").unwrap_or(name).eq_ignore_ascii_case(b"select")
            }).count() != 1
            || !context.document.as_ref().is_some_and(|document| {
                let Some(rows) = document.records.get(..5) else { return false; };
                document.records.len() == 5 && rows.iter().all(|row| row.kind == 0x30)
                    && rows[0].text.trim_ascii() == b"#192 = 3"
                    && rows[1].text.trim_ascii() == b"fselect select 0"
                    && rows[2].text.starts_with(b"0 com01_selC_1 ")
                    && rows[3].text.starts_with(b"1 com01_selC_2 ")
                    && rows[4].text.starts_with(b"2 com01_selC_3 ")
            })
        {
            return Ok(None);
        }
        let mut regions = [0; 3];
        for (row, y) in [206, 326, 446].into_iter().enumerate() {
            let point = [640, y];
            let hit = self.masked_input_region_at(point)?;
            let candidates = self.mouse_press_candidate_regions(point)?;
            let target = hit.and_then(|hit| self.data_services.tasks.fes.platform.region_target(hit[0]));
            let layer = self.data_services.tasks.fes.runtime(owner)
                .and_then(|runtime| runtime.media.get(&format!("btn_frip[{row}]")));
            eprintln!("ISLAND choice row={row} owner={owner} point={point:?} hit={hit:?} target={target:?} candidates={candidates:?} layer={:?}",
                layer.map(|layer| (&layer.kind, layer.enabled, layer.visible, layer.opacity)));
            let Some(hit) = hit else { continue; };
            if hit[8] != 0 && hit[9] != 0 && candidates.contains(&hit[0])
                && self.input_button_state(hit[0]).is_some_and(|button| button.receiver == Some(owner))
                && target.is_some_and(|(bound, target)| bound == owner && target.is_some_and(|target| {
                    target.name == b"btn_frip" && target.bank == row as i32
                }))
                && layer.is_some_and(|layer| layer.kind == "BUTTON" && layer.enabled && layer.visible && layer.opacity > 0)
            {
                regions[row] = hit[0];
            }
        }
        Ok((!regions.contains(&0) && regions[0] != regions[1] && regions[0] != regions[2] && regions[1] != regions[2])
            .then_some((handle, loaded, owner, regions[0])))
    }

    pub fn host_island_bad02_card(&self) -> Option<(u32, u64, u32)> {
        let ((frame, state, scene, group, record), window) = self.host_replay_story_position();
        let ("fullwnd", owner) = window? else { return None; };
        if (state, scene, group, record) != ("page_probe", "BAD_02", 37, 207)
            || self.host_replay_menu_ready().is_some() || self.frame_in_progress()
            || !self.pending_fes_events.is_empty() || !self.data_services.pending_host_input.is_empty()
            || self.task_parents.get(&owner) != Some(&8) || self.message_wait_address != Some(0xc29)
        {
            return None;
        }
        let (&handle, context) = self.cst_contexts.iter().find(|(_, context)| context.story.scene == scene)?;
        let (_, 37, 207) = context.story.settled_body_record(frame)? else { return None; };
        let document = context.document.as_ref()?;
        let row = document.records.get(206)?;
        let expected: [(usize, &[u8]); 3] = [(210, b"view 1 meswnd"), (211, b"wait"), (212, b"#722 = (#722|(1<<1))")];
        (row.kind == 0x20 && !body_control_only(&row.text) && document.inputs.get(37)?.contains(&206)
            && island_cst_tail_matches(document, document.inputs.get(38)?, &expected))
            .then_some((handle, context.story.loaded, owner))
    }

    pub fn host_island_opmovie_playing(&self) -> bool {
        self.movie_playbacks.keys().any(|handle| {
            self.movie_resources.get(handle).is_some_and(|resource| {
                resource.rsplit(['/', '\\']).next().is_some_and(|name| name.eq_ignore_ascii_case("opmovie.webm"))
            })
        })
    }

    pub fn host_island_opmovie_ended(&self, origin_load: u64) -> bool {
        if !self.movie_playbacks.is_empty() {
            return false;
        }
        let ((frame, state, scene, group, record), window) = self.host_replay_story_position();
        if (state, scene, group, record) != ("page", "OPMOVIE_END", 0, 19) || window.is_none() {
            return false;
        }
        self.cst_contexts.values().any(|context| {
            context.story.scene == scene && context.story.loaded > origin_load
                && context.story.settled_body_record(frame).is_some_and(|(_, group, record)| (group, record) == (0, 19))
                && context.document.as_ref().is_some_and(|document| adjacent_cst_input_waits(document, (0, 19), None))
        })
    }

    fn host_neko_startup_owner(&self, screen: NekoStartupScreen, frame: u64) -> Option<u32> {
        if !self.state.input.focused || self.data_services.host_pause.paused
            || !self.movie_playbacks.is_empty() || !self.pending_messages.is_empty()
            || !self.data_services.pending_host_input.is_empty()
            || self.message_wait_address != Some(0xd6d)
            || !self.vm.as_ref().is_some_and(|vm| vm.waiting_for_message())
        {
            return None;
        }
        let (_, button, _) = screen.contract();
        let mut windows = self.data_services.tasks.fes.story_runtimes().map(|(owner, name, runtime, wait_key)| {
            let driver = runtime.driver.as_ref();
            StartupDriver {
                owner, name, wait_key,
                block: driver.map(|driver| driver.interpreter.current_block()).unwrap_or_default(),
                pending: driver.is_none_or(|driver| driver.has_pending_handler()),
                wait: driver.and_then(|driver| driver.waiting()),
            }
        });
        let owner = neko_startup_fes_owner(screen, &mut windows, frame)?;
        let runtime = self.data_services.tasks.fes.runtime(owner)?;
        (self.data_services.tasks.task_is_registered(owner)
            && (self.task_parents.contains_key(&owner) || self.scene_nodes.contains_key(&owner))
            && runtime.media.get(button).is_some_and(|layer| {
                layer.kind == "BUTTON" && layer.enabled && layer.visible && layer.opacity > 0
            })).then_some(owner)
    }

    pub fn host_neko_composited_film(&self) -> Option<(&'static str, u32, u64, f64)> {
        if !self.data_services.story_trace
            || self.host_story_trace_profile()? != StoryTraceProfile::Nekopara
        {
            return None;
        }
        let (&handle, &playback) = self.movie_playbacks.iter().next()?;
        if self.movie_playbacks.len() != 1 {
            return None;
        }
        let member = self.movie_resources.get(&handle)?.rsplit(['/', '\\']).next()?;
        let name = match member.to_ascii_lowercase().as_str() {
            "nekopara3op.wmv" => "op",
            "nekopara3ed.wmv" => "ed",
            _ => return None,
        };
        let surface = self.movie_host.frame_surface();
        let receipt = self.movie_host.scene_movie_presentations().into_iter().find(|receipt| {
            receipt.playback == playback && std::sync::Arc::ptr_eq(&receipt.surface, &surface)
        })?;
        Some((name, handle, playback, receipt.time))
    }

    pub fn host_neko_branch_flags(&self) -> Option<[u32; 2]> {
        (self.host_story_trace_profile()? == StoryTraceProfile::Nekopara).then_some(())?;
        Some([*self.shared_values.get(1003)?, *self.shared_values.get(1004)?])
    }

    pub fn host_neko_cleared_title(&self) -> Option<u32> {
        if self.shared_values.get(550) != Some(&1) {
            return None;
        }
        self.host_neko_title_load()
    }

    pub fn host_neko_startup_language_matches(&self, expected: &str) -> bool {
        if !self.data_services.story_trace || self.host_story_trace_profile() != Some(StoryTraceProfile::Nekopara) {
            return false;
        }
        let languages = &self.data_services.language.languages;
        if languages.prefix(0) != Some(b"en".as_slice())
            || languages.prefix(1) != Some(expected.as_bytes())
        {
            return false;
        }
        let selection = [languages.primary_index, languages.message_index, languages.subtitle_index];
        match self.host_replay_story_position().1 {
            Some(("langselect", owner)) => self.neko_language_locals(owner).is_some_and(|locals| {
                neko_startup_language_matches(expected, selection, Some(locals))
            }),
            _ => neko_startup_language_matches(expected, selection, None),
        }
    }

    pub fn host_neko_slot_zero_absent(&self) -> bool {
        super::save_path(&self.save_root, "save0000.dat").is_ok_and(|path| {
            std::fs::symlink_metadata(path).is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
        })
    }

    /// The full-window auxiliary FES button, with no toolbar action at this point.
    pub fn host_neko_body_click_region(&self, owner: u32, point: [i32; 2]) -> Option<u32> {
        let platform = &self.data_services.tasks.fes.platform;
        let region = platform.auxiliary_button(owner)?;
        let runtime = self.data_services.tasks.fes.runtime(owner)?;
        let button = self.input_button_state(region)?;
        (self.task_parents.get(&region) == Some(&owner)
            && matches!(platform.region_target(region), Some((bound, None)) if bound == owner)
            && button.show != 0 && button.receiver == Some(owner)
            && runtime.input_for_driver(2, Some(cs2_renderer::Point::new(point[0], point[1])))
                .target.is_none()).then_some(region)
    }

    pub fn host_neko_body_root_owner(&self, owner: u32, region: u32) -> Option<u32> {
        let root = *self.task_parents.get(&owner)?;
        let parent = *self.task_parents.get(&root)?;
        let button = self.input_button_state(region)?;
        (button.show != 0 && button.receiver == Some(root)
            && story_trace_09::live_child(self, root, parent, 0)
            && story_trace_09::live_child(self, owner, root, 34)
            && story_trace_09::live_child(self, region, root, 6)).then_some(root)
    }

    pub fn host_neko_save_target(&self, owner: u32, region: u32, name: &str, bank: i32) -> bool {
        self.input_button_state(region).is_some_and(|button| button.show != 0 && button.receiver == Some(owner))
            && self.data_services.tasks.fes.platform.region_target(region).is_some_and(|(bound, target)| {
                bound == owner && target.is_some_and(|target| target.name == name.as_bytes() && target.bank == bank)
            })
    }

    pub fn host_neko_save_panel(&self, anchor: (u32, u64, u32, u64, u64)) -> Option<u32> {
        if !self.host_neko_slot_zero_absent() {
            return None;
        }
        self.host_neko_save_panel_mode(anchor, 0)
    }

    pub fn host_neko_saved_panel(&self, anchor: (u32, u64, u32, u64, u64)) -> Option<u32> {
        self.host_neko_save_panel_mode(anchor, 1)
    }

    fn host_neko_save_panel_mode(&self, anchor: (u32, u64, u32, u64, u64), exists: u32) -> Option<u32> {
        let (context, load, message_owner, epoch, _) = anchor;
        let frame = self.state.clock.frame;
        if !self.host_neko_menu_ready() {
            return None;
        }
        let retained = &self.cst_contexts.get(&context)?.story;
        if !retained.is_scene("01")
            || retained.settled_body_record(frame).map(|(_, group, record)| (group, record)) != Some((24, 218))
            || !self.data_services.tasks.task_is_registered(message_owner)
            || !self.data_services.fes_text.story_body_sources(message_owner, epoch, &self.data_services.cst_text_origins)
                .is_some_and(|sources| sources.iter().any(|source| {
                    (source.context_handle, source.load_id, source.group, source.absolute_record) == (context, load, 24, 216)
                }))
        {
            return None;
        }
        let (owner, locals) = self.host_neko_menu_driver(b"saveload", b"WAIT")?;
        self.host_neko_slot_mode(locals, [0, 1, exists]).then_some(owner)
    }

    /// An ordinary overwrite can only act on the page retained from the live body guard.
    pub fn host_neko_checkpoint_save_panel(
        &self, anchor: (u32, u64, u32, u64, u64), scene: &str, position: (i32, i32),
        confirm: bool, expected_panel: Option<u32>,
    ) -> Option<u32> {
        if !self.host_neko_menu_ready() {
            return None;
        }
        let (context, load, owner, epoch, source_frame) = anchor;
        let frame = self.state.clock.frame;
        let retained = self.cst_contexts.get(&context)?;
        let (marked, group, record) = retained.story.settled_body_record(frame)?;
        let generation = self.data_services.fes_text.story_body_generation(owner)?;
        let sources = self.data_services.fes_text.story_body_sources(owner, epoch, &self.data_services.cst_text_origins)?;
        let shell = BodyWaitShell {
            window: "meswnd", owner, scene: retained.story.scene.as_str(), context, loaded: retained.story.loaded,
            marked, group, record,
        };
        if !retained.story.is_scene(scene) || (group, record) != position
            || !self.data_services.tasks.task_is_registered(owner)
            || (generation.source_epoch, generation.source_frame) != (epoch, source_frame)
            || sources.first()?.load_id != load
            || !self.host_body_sources_match(retained, shell, Some(sources))
        {
            return None;
        }
        let (panel, locals) = self.host_neko_menu_driver(b"saveload", if confirm { b"CONFIRM_WAIT" } else { b"WAIT" })?;
        if !self.host_neko_slot_mode(locals, [0, 1, 1]) || expected_panel.is_some_and(|expected| panel != expected) {
            return None;
        }
        if !confirm {
            return Some(panel);
        }
        if locals.first() != Some(&1) || locals.get(101) != Some(&0) || locals.get(102) != Some(&0) {
            return None;
        }
        let (child, _) = self.host_neko_menu_driver(b"confirm2", b"WAIT")?;
        (self.task_parents.get(&child) == Some(&panel)).then_some(child)
    }

    fn host_neko_menu_ready(&self) -> bool {
        self.data_services.story_trace && self.host_story_trace_profile() == Some(StoryTraceProfile::Nekopara)
            && !self.frame_in_progress() && self.state.input.focused && !self.data_services.host_pause.paused
            && !self.data_services.host_quit_requested && self.movie_playbacks.is_empty()
            && self.pending_messages.is_empty() && self.data_services.pending_host_input.is_empty()
            && self.message_wait_address == Some(0xd6d)
            && self.vm.as_ref().is_some_and(|vm| vm.waiting_for_message())
    }

    fn host_neko_menu_driver(&self, name: &[u8], block: &[u8]) -> Option<(u32, &[i32])> {
        let mut windows = self.data_services.tasks.fes.story_runtimes().filter(|(_, fes, _, _)| {
            fes.strip_suffix(b".fes").unwrap_or(fes) == name
        });
        let (owner, _, runtime, _) = windows.next()?;
        let driver = runtime.driver.as_ref()?;
        let frame = self.state.clock.frame;
        (windows.next().is_none() && !driver.has_pending_handler() && driver.interpreter.current_block() == block
            && matches!(driver.waiting(), Some(FesDriverWait::Frame { after }) if *after == frame)
            && self.data_services.tasks.task_is_registered(owner))
            .then_some((owner, driver.interpreter.local_values()))
    }

    fn host_neko_slot_mode(&self, locals: &[i32], expected: [u32; 3]) -> bool {
        let Ok(shared) = self.shared_numeric_state_snapshot() else { return false; };
        let root = if shared.active_identity == 0 { Some(&shared.active_values) } else { shared.inactive_values.get(&0) };
        root.is_some_and(|root| [800, 801, 860].map(|slot| root.get(slot).copied()) == expected.map(Some))
            && locals.get(100) == Some(&0) && locals.get(110) == Some(&(expected[0] as i32))
    }

    pub fn host_neko_title_load(&self) -> Option<u32> {
        if !self.host_neko_menu_ready() || !self.cst_contexts.is_empty() {
            return None;
        }
        self.host_neko_startup_owner(NekoStartupScreen::Title, self.state.clock.frame)
    }

    pub fn host_neko_load_panel(&self, confirm: bool) -> Option<u32> {
        if !self.host_neko_menu_ready() || !self.cst_contexts.is_empty() {
            return None;
        }
        self.host_neko_menu_driver(b"ttlflow", b"LOAD_WAIT")?;
        let (panel, locals) = self.host_neko_menu_driver(b"saveload", if confirm { b"CONFIRM_WAIT" } else { b"WAIT" })?;
        if !self.host_neko_slot_mode(locals, [1, 0, 1]) {
            return None;
        }
        if !confirm {
            return Some(panel);
        }
        if locals.get(101) != Some(&0) || locals.get(102) != Some(&0) {
            return None;
        }
        let (owner, _) = self.host_neko_menu_driver(b"confirm2", b"WAIT")?;
        (self.task_parents.get(&owner) == Some(&panel)).then_some(owner)
    }

    pub fn host_neko_slot_sha256(&self) -> anyhow::Result<Option<String>> {
        let path = super::save_path(&self.save_root, "save0000.dat")?;
        match std::fs::metadata(&path) {
            Ok(info) if info.is_file() && info.len() <= 64 * 1024 * 1024 => {}
            Ok(_) => anyhow::bail!("invalid ordinary slot 0 file"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        }
        let bytes = std::fs::read(path)?;
        anyhow::ensure!(cs2_formats::CssFile::from_css(&bytes)?.is_some(), "invalid ordinary slot 0 CSS");
        Ok(Some(particle_plugin_digest::sha256(&bytes)))
    }

    fn neko_language_locals(&self, owner: u32) -> Option<[i32; 3]> {
        let runtime = self.data_services.tasks.fes.runtime(owner)?;
        let locals = runtime.driver.as_ref()?.interpreter.local_values();
        Some([*locals.get(55)?, *locals.get(56)?, *locals.get(57)?])
    }

    fn rebellions_title_ready(&self, frame: u64) -> bool {
        if !self.state.input.focused
            || self.data_services.host_pause.paused
            || !self.movie_playbacks.is_empty()
            || self.message_wait_address.is_none()
            || !self.pending_messages.is_empty()
            || !self.vm.as_ref().is_some_and(|vm| vm.waiting_for_message())
        {
            return false;
        }
        let mut screens = self.data_services.tasks.fes.story_runtimes().filter(|(_, name, _, _)| {
            let name = name.strip_suffix(b".fes").unwrap_or(name);
            name.eq_ignore_ascii_case(b"title") || name.eq_ignore_ascii_case(b"epselect")
        });
        let Some((_, name, runtime, _)) = screens.next() else {
            return false;
        };
        screens.next().is_none() && runtime.driver.as_ref().is_some_and(|driver| {
            rebellions_title_fes_ready(
                name, driver.interpreter.current_block(), driver.has_pending_handler(), driver.waiting(), frame,
            )
        })
    }

    fn rebellions_episode_ready(&self, frame: u64) -> bool {
        if !self.state.input.focused
            || self.data_services.host_pause.paused
            || !self.movie_playbacks.is_empty()
            || self.message_wait_address.is_none()
            || !self.pending_messages.is_empty()
            || !self.vm.as_ref().is_some_and(|vm| vm.waiting_for_message())
        {
            return false;
        }
        let windows = self.data_services.tasks.fes.story_runtimes().map(|(_, name, runtime, _)| {
            let name = name.strip_suffix(b".fes").unwrap_or(name);
            let driver = runtime.driver.as_ref();
            EpisodeWindow {
                name,
                block: driver.map(|driver| driver.interpreter.current_block()).unwrap_or_default(),
                pending: driver.is_none_or(|driver| driver.has_pending_handler()),
                wait: driver.and_then(|driver| driver.waiting()),
                episode_a_ready: name.eq_ignore_ascii_case(b"epselect")
                    && runtime.media.get("epbtn[0]").is_some_and(|layer| {
                        layer.kind == "BUTTON" && layer.enabled && layer.visible && layer.opacity > 0
                    }),
            }
        });
        rebellions_episode_fes_ready(windows, frame)
    }

    pub fn host_replay_menu_ready(&self) -> Option<&'static str> {
        if !self.data_services.story_trace
            || self.host_story_trace_profile() != Some(StoryTraceProfile::Island)
            || !self.state.input.focused
            || self.data_services.host_pause.paused || !self.movie_playbacks.is_empty()
        {
            return None;
        }
        self.data_services.tasks.fes.story_runtimes().find_map(|(_, name, runtime, _)| {
            let driver = runtime.driver.as_ref()?;
            if driver.has_pending_handler() || driver.interpreter.current_block() != b"WAIT_USER" {
                return None;
            }
            let name = name.strip_suffix(b".fes").unwrap_or(name);
            if name.eq_ignore_ascii_case(b"title") {
                Some("title")
            } else if [b"saveload".as_slice(), b"save", b"load", b"config", b"log"]
                .iter().any(|menu| name.eq_ignore_ascii_case(menu))
            {
                Some("menu")
            } else {
                None
            }
        })
    }

    pub fn host_daydream_prologue_tail(&self, owner: u32) -> bool {
        if self.host_story_trace_profile() != Some(StoryTraceProfile::Daydream) {
            return false;
        }
        let Some(shell) = self.host_body_wait_shell(self.state.clock.frame) else { return false; };
        if (shell.scene, shell.group, shell.record, shell.owner) != ("prologue", 40, 221, owner) {
            return false;
        }
        self.cst_contexts.get(&shell.context).and_then(|context| context.document.as_ref())
            .is_some_and(|document| {
                let expected: [(usize, &[u8]); 9] = [
                    (222, b"frameoff"), (225, b"rdraw 60"), (228, b"rdraw 120"),
                    (237, b"initialize_header"), (238, b"view 0 default"), (242, b"fesds"),
                    (272, b"drawdef"), (273, b"wipedef"), (274, b"wait"),
                ];
                document.inputs.get(41) == Some(&(222..275)) && document.records.len() == 275
                    && document.records[222..275].iter().all(|row| row.kind == 0x30)
                    && expected.into_iter().all(|(record, command)| document.records[record].text.trim_ascii() == command)
            })
    }

    pub fn host_daydream_post_prologue_title(&mut self) -> Option<(u32, u32, u32, u32, Option<[i32; 4]>)> {
        let frame = self.state.clock.frame;
        if self.host_story_trace_profile() != Some(StoryTraceProfile::Daydream)
            || !self.data_services.story_trace || self.frame_in_progress()
            || !self.state.input.focused || self.data_services.host_pause.paused
            || self.data_services.host_quit_requested || !self.movie_playbacks.is_empty()
            || !self.cst_contexts.is_empty()
            || !self.pending_messages.is_empty() || !self.data_services.pending_host_input.is_empty()
        {
            return None;
        }
        let (mut flow, mut ttlflow, mut title) = (None, None, None);
        for (owner, name, runtime, _) in self.data_services.tasks.fes.story_runtimes() {
            let driver = runtime.driver.as_ref()?;
            if driver.initialization_pending() || driver.has_pending_handler()
                || !matches!(driver.waiting(), Some(FesDriverWait::Frame { after }) if *after == frame)
                || !self.data_services.tasks.task_is_registered(owner)
            {
                return None;
            }
            let name = name.strip_suffix(b".fes").unwrap_or(name);
            let block = driver.interpreter.current_block();
            let target = match (name, block) {
                (b"flow", b"TITLE_WAIT") => &mut flow,
                (b"ttlflow", b"FES_WAIT") => &mut ttlflow,
                (b"title", b"WAIT") if runtime.media.get("btn1[0]").is_some_and(|layer| {
                    layer.kind == "BUTTON" && layer.enabled && layer.visible && layer.opacity > 0
                }) => &mut title,
                _ => return None,
            };
            if target.replace(owner).is_some() {
                return None;
            }
        }
        let snapshot = self.shared_numeric_state_snapshot().ok()?;
        let root = if snapshot.active_identity == 0 {
            &snapshot.active_values
        } else {
            snapshot.inactive_values.get(&0)?
        };
        if root.get(542) != Some(&1) || root.get(541) != Some(&0) {
            return None;
        }
        let (flow, ttlflow, title) = (flow?, ttlflow?, title?);
        if self.task_parents.get(&ttlflow) != Some(&flow)
            || self.task_parents.get(&title) != Some(&ttlflow)
        {
            return None;
        }
        let button = self.with_host(|host| {
            Ok(super::fes_task_platform::button_record_lookup(host, title, 0)?)
        }).ok()??;
        if button.enabled == 0 || button.target.name.as_slice() != b"btn1" || button.target.bank != 0 {
            return None;
        }
        let region = u32::try_from(button.target.value).ok()?;
        let (owner, target) = self.data_services.tasks.fes.platform.region_target(region)?;
        let target = target?;
        if (owner, target.name.as_slice(), target.bank) != (title, b"btn1".as_slice(), 0)
            || self.task_parents.get(&region) != Some(&title)
            || !self.data_services.tasks.task_is_registered(region)
        {
            return None;
        }
        // FES refreshes the hit table independently of the type-six state rectangle.
        let mut registrations = self.input_task.regions.iter().filter(|entry| entry.id() == region);
        let rectangle = registrations.next().and_then(|entry| {
            let bounds @ [left, top, right, bottom] = entry.bounds();
            (registrations.next().is_none() && entry.words[8] != 0 && entry.words[9] != 0
                && left < right && top < bottom
                && self.input_button_state(region).is_some_and(|input| input.receiver == Some(title) && input.show != 0))
                .then_some(bounds)
        });
        Some((flow, ttlflow, title, region, rectangle))
    }

    pub fn host_daydream_unexpected_transition_window(
        &self,
        started: u64,
        anchor: (u32, u64, u32, u64, u64),
        root_receiver: u32,
        default_window: Option<(u32, u64, u64)>,
        fesds_frame: Option<u64>,
        prologue_eof_frame: Option<u64>,
    ) -> bool {
        let frame = self.state.clock.frame;
        let (context_handle, load_id, cinema_owner, _, _) = anchor;
        let mut flow_in_game = false;
        let mut flow_starting_title = false;
        let mut meswnd = None;
        let mut window_count = 0;
        for (owner, name, runtime, wait_key) in self.data_services.tasks.fes.story_runtimes() {
            window_count += 1;
            let waiting_in = |blocks: &[&[u8]]| {
                !wait_key && self.data_services.tasks.task_is_registered(owner)
                    && runtime.driver.as_ref().is_some_and(|driver| {
                        blocks.contains(&driver.interpreter.current_block()) && !driver.has_pending_handler()
                            && matches!(driver.waiting(), Some(FesDriverWait::Frame { after }) if *after >= frame)
                    })
            };
            match name.strip_suffix(b".fes").unwrap_or(name) {
                b"flow" => {
                    flow_in_game = waiting_in(&[b"GAME_WAIT"]);
                    flow_starting_title = waiting_in(&[b"START_TITLE"]);
                }
                b"meswnd" if meswnd.is_none() => {
                    let expected_window = default_window.is_some_and(|(window, _, created)| {
                        owner == window
                            && fesds_frame.is_some_and(|issued| created <= issued && issued <= frame)
                    });
                    let setstate = waiting_in(&[b"MES_SETSTATE"]) && expected_window;
                    if !setstate && !waiting_in(&[b"WAIT", b"MES_SHOW"]) {
                        return true;
                    }
                    let returned_to_wait = expected_window && waiting_in(&[b"WAIT"])
                        && runtime.driver.as_ref().is_some_and(|driver| daydream_meswnd_main_wait(b"meswnd", driver));
                    meswnd = Some((owner, setstate || returned_to_wait));
                }
                b"cinema" | b"ttlflow" | b"title" => {}
                _ => return true,
            }
        }
        let Some((meswnd, meswnd_ready_for_title)) = meswnd else { return false; };
        // The final CST group has 53 records. Its next GetLine marks the observed title handoff.
        let title_handoff = flow_starting_title && meswnd_ready_for_title
            && fesds_frame.zip(prologue_eof_frame).is_some_and(|(issued, eof)| issued <= eof && eof <= frame);
        // Group 41 restores the default message window at r238 before the CST exits.
        // Keep the pre-Enter page as an identity check only. It cannot authorize input.
        if window_count != 2 || !(flow_in_game || title_handoff)
            || self.task_parents.get(&meswnd) != Some(&root_receiver)
            || meswnd == cinema_owner || self.message_wait_address != Some(0xd6d)
            || !self.vm.as_ref().is_some_and(|vm| vm.waiting_for_message())
            || !self.pending_messages.is_empty() || !self.movie_playbacks.is_empty()
        {
            return true;
        }
        let Some((owner, view_frame, created_frame)) = default_window else { return true; };
        if owner != meswnd || view_frame <= started || created_frame < view_frame || created_frame > frame {
            return true;
        }
        // START_TITLE can outlive the released CST while the same message window winds down.
        if title_handoff && self.cst_contexts.is_empty() {
            return false;
        }
        let mut scenes = self.cst_contexts.iter().filter(|(_, context)| !context.story.scene.is_empty());
        let Some((&handle, context)) = scenes.next() else { return true; };
        if handle != context_handle || scenes.next().is_some() || context.story.scene != "prologue"
            || context.story.candidate.is_some()
            || !matches!(context.story.page, Some((marked, 0x02, 40, 221)) if marked < started)
            || !self.data_services.cst_text_origins.body_is_current(&[CstBodySource {
                context_handle, load_id, group: 40, absolute_record: 215,
            }])
        {
            return true;
        }
        if created_frame == frame {
            eprintln!(
                "frame {frame}: Daydream r238 default meswnd owner={meswnd} parent={root_receiver} context={context_handle} load={load_id} view_frame={view_frame} wait_key=false",
            );
        }
        false
    }

    pub fn host_daydream_case_blocked(&self, started: u64, advancing: bool) -> bool {
        if self.host_story_trace_profile() != Some(StoryTraceProfile::Daydream) {
            return true;
        }
        let frame = self.state.clock.frame;
        let mut stories = self.cst_contexts.values().map(|context| &context.story)
            .filter(|story| !story.scene.is_empty() && story.loaded > started);
        let story = stories.next();
        if stories.next().is_some() {
            return true;
        }
        let expected = story.and_then(|story| daydream_case_records(&story.scene));
        if story.is_some_and(|story| {
            // Loading CASE 1 can query g2/r70 before publishing any page.
            let load_probe = !advancing
                && story.is_scene("c01_01_01")
                && story.page.is_none()
                && story.candidate == Some((story.loaded, 0x02, 2, 70));
            let candidate = story.candidate.filter(|_| !load_probe);
            [story.page, candidate].into_iter().flatten().any(|(_, kind, group, record)| {
                matches!(kind, 0x02 | 0x03)
                    && (kind != 0x02 || !expected.is_some_and(|(_, pages)| {
                        (group, record) == (0, pages[0].0) || advancing && (group, record) == (1, pages[1].0)
                    }))
            })
        }) {
            return true;
        }
        self.data_services.tasks.fes.story_runtimes().any(|(_, name, runtime, wait_key)| {
            let name = name.strip_suffix(b".fes").unwrap_or(name);
            let Some(driver) = runtime.driver.as_ref() else { return false; };
            let block = driver.interpreter.current_block();
            if wait_key {
                return !expected.is_some_and(|(window, _)| name == window.as_bytes()
                    && (block == b"MES_SHOW" || daydream_meswnd_main_wait(name, driver)));
            }
            !matches!(name, b"flow" | b"ttlflow" | b"title" | b"meswnd" | b"cinema")
                && !driver.has_pending_handler()
                && matches!(driver.waiting(), Some(FesDriverWait::Frame { after }) if *after == frame)
        })
    }

    pub fn host_daydream_case_body(
        &self, started: u64, scene: &str, group: usize,
    ) -> Option<(i32, (u32, u64, u32, u64, u64), String)> {
        let frame = self.state.clock.frame;
        if self.host_story_trace_profile() != Some(StoryTraceProfile::Daydream) {
            return None;
        }
        let (window, pages) = daydream_case_records(scene)?;
        let &(record, expected) = pages.get(group)?;
        let shell = self.host_body_wait_shell(frame)?;
        if shell.loaded <= started
            || (shell.scene, shell.group, shell.record, shell.window) != (scene, group as i32, record, window)
        {
            return None;
        }
        let (sources, _, composed) = self.host_current_body(shell, frame);
        let Some(sources) = sources.filter(|sources| composed
            && sources.iter().map(|source| source.absolute_record).eq(expected.iter().copied())
            && self.data_services.cst_text_origins.body_is_current(sources)) else { return None; };
        let generation = self.data_services.fes_text.story_body_generation(shell.owner)?;
        let (_, complete) = generation.natural_complete?;
        let stage = if group == 0 { "first" } else { "second" };
        let load = sources[0].load_id;
        let anchor = (shell.context, load, shell.owner, generation.source_epoch, generation.source_frame);
        let evidence = format!(
            "frame {frame}: Daydream {stage} CASE scene={scene} group={group} record={record} window={window} owner={} context={} load={load} loaded={} marked={} sources={expected:?} epoch={} source_frame={} natural_complete={complete} composed=true",
            shell.owner, shell.context, shell.loaded, shell.marked, generation.source_epoch, generation.source_frame,
        );
        Some((record, anchor, evidence))
    }

    pub fn host_daydream_case_enter(&self, scene: &str, context: u32, owner: u32) -> Option<(u32, u32)> {
        let (_, [(first, _), (second, _)]) = daydream_case_records(scene)?;
        let document = self.cst_contexts.get(&context)?.document.as_ref()?;
        if !adjacent_cst_input_waits(document, (0, first), Some((1, second))) {
            return None;
        }
        let between = document.records.get(first as usize + 1..second as usize)?;
        let layout: &[(u32, &[u8])] = match scene {
            "c01_01_01" => &[(0x20, b"")],
            "c02_01_01" => &[(0x30, b"rdraw"), (0x30, b"pcm K_c02_01_002"), (0x21, b""), (0x20, b"")],
            "c03_01_01" => &[(0x30, b"se 0 se4029"), (0x20, b"")],
            _ => return None,
        };
        if between.len() != layout.len() || !between.iter().zip(layout).all(|(row, &(kind, command))| {
            u32::from(row.kind) == kind && if kind == 0x30 {
                row.text.trim_ascii() == command
            } else {
                !row.text.trim_ascii().is_empty()
            }
        }) {
            return None;
        }
        self.legacy_bottom_enter(owner)
    }


    pub fn host_daydream_end1_body(
        &self, scene: &str, group: i32, record: i32, loaded_after: u64,
    ) -> Option<((u32, u64, u32, u64, u64), (u32, u32))> {
        if self.host_story_trace_profile() != Some(StoryTraceProfile::Daydream) {
            return None;
        }
        let frame = self.state.clock.frame;
        let shell = self.host_body_wait_shell(frame)?;
        if !shell.scene.eq_ignore_ascii_case(scene) || (shell.group, shell.record) != (group, record)
            || shell.loaded < loaded_after
        {
            return None;
        }
        let generation = self.data_services.fes_text.story_body_generation(shell.owner)?;
        let binding = self.legacy_bottom_enter(shell.owner)?;
        let (sources, sources_match, composed) = self.host_current_body(shell, frame);
        // The verified message window can accept Enter while its prompt has no submitted pixels.
        let body_only = !composed && sources_match
            && self.data_services.tasks.fes.runtime(shell.owner)
                .and_then(|runtime| runtime.driver.as_ref())
                .is_some_and(|driver| daydream_meswnd_main_wait(shell.window.as_bytes(), driver))
            && self.host_body_committed(shell, generation, frame)
            && self.host_body_list_components(shell) == Some((true, false));
        let sources = sources.filter(|sources| (composed || body_only)
            && self.data_services.cst_text_origins.body_is_current(sources))?;
        let anchor = (shell.context, sources.first()?.load_id, shell.owner, generation.source_epoch, generation.source_frame);
        Some((anchor, binding))
    }

    pub fn host_daydream_end1_choice(
        &mut self, scene: &str, targets: &[&str], loaded_after: u64,
    ) -> anyhow::Result<Option<([i32; 2], u32, u32)>> {
        if self.host_story_trace_profile() != Some(StoryTraceProfile::Daydream)
            || !self.data_services.story_trace || self.frame_in_progress()
            || !self.state.input.focused || self.data_services.host_pause.paused
            || self.data_services.host_quit_requested || !self.state.input.pressed.is_empty()
            || !self.pending_messages.is_empty() || !self.pending_fes_events.is_empty()
            || !self.data_services.pending_host_input.is_empty() || !self.movie_playbacks.is_empty()
            || self.message_wait_address != Some(0xd6d)
            || !self.vm.as_ref().is_some_and(|vm| vm.waiting_for_message())
        {
            return Ok(None);
        }
        let mut contexts = self.cst_contexts.values().filter(|context| !context.story.scene.is_empty());
        let Some(context) = contexts.next() else { return Ok(None); };
        if contexts.next().is_some() || !context.story.scene.eq_ignore_ascii_case(scene)
            || context.story.loaded < loaded_after
        {
            return Ok(None);
        }
        let Some(document) = &context.document else { return Ok(None); };
        let mut choices = document.records.iter().filter(|row| row.kind == 0x30)
            .skip_while(|row| row.text.trim_ascii().split(|b| b.is_ascii_whitespace()).next() != Some(b"fselect".as_slice()));
        if choices.next().is_none() || !targets.iter().all(|target| choices.any(|row| {
            row.text.split(|b| b.is_ascii_whitespace()).any(|word| word.eq_ignore_ascii_case(target.as_bytes()))
        })) {
            return Ok(None);
        }
        let frame = self.state.clock.frame;
        let mut flow = false;
        let mut selected = None;
        for (owner, name, runtime, wait_key) in self.data_services.tasks.fes.story_runtimes() {
            let name = name.strip_suffix(b".fes").unwrap_or(name);
            let Some(driver) = &runtime.driver else { return Ok(None); };
            if driver.initialization_pending() || driver.has_pending_handler()
                || !matches!(driver.waiting(), Some(FesDriverWait::Frame { after }) if *after == frame)
                || !self.data_services.tasks.task_is_registered(owner)
            {
                return Ok(None);
            }
            let block = driver.interpreter.current_block();
            match name {
                b"flow" if !flow && !wait_key && block == b"GAME_WAIT" => flow = true,
                b"select" if selected.is_none() && block == b"WAIT"
                    && driver.interpreter.local_values().get(400) == Some(&1) => selected = Some((owner, runtime)),
                b"meswnd" | b"cinema" | b"messh" if !wait_key => {}
                _ => return Ok(None),
            }
        }
        let Some((owner, runtime)) = selected.filter(|_| flow) else { return Ok(None); };
        let visible: Vec<_> = (0..10).filter(|bank| runtime.media.get(&format!("fr1[{bank}]")).is_some_and(|layer| {
            layer.kind == "BUTTON" && layer.enabled && layer.visible && layer.opacity > 0
        })).collect();
        if !visible.iter().copied().eq(0..targets.len()) {
            return Ok(None);
        }
        let mut regions = Vec::new();
        for bank in visible {
            let mut bindings = self.input_task.regions.iter().filter_map(|entry| {
                let region = entry.id();
                let (receiver, target) = self.data_services.tasks.fes.platform.region_target(region)?;
                let target = target?;
                (receiver == owner && target.name == b"fr1" && target.bank == bank as i32).then_some(region)
            });
            let Some(region) = bindings.next() else { return Ok(None); };
            if bindings.next().is_some() || self.task_parents.get(&region) != Some(&owner) {
                return Ok(None);
            }
            let Some(button) = self.input_button_state(region) else { return Ok(None); };
            let [left, top, right, bottom] = button.rectangle;
            if button.receiver != Some(owner) || button.show == 0 || left >= right || top >= bottom {
                return Ok(None);
            }
            let point = [left.midpoint(right), top.midpoint(bottom)];
            if !self.masked_input_region_at(point)?.is_some_and(|hit| hit[0] == region && hit[8] != 0 && hit[9] != 0)
                || !self.mouse_press_candidate_regions(point)?.contains(&region)
            {
                return Ok(None);
            }
            regions.push((point, region, owner));
        }
        Ok(regions.pop())
    }

    pub fn host_daydream_ordinary_save_button(&mut self, owner: u32) -> anyhow::Result<Option<([i32; 2], u32)>> {
        let eligible = self.host_story_trace_profile() == Some(StoryTraceProfile::Daydream)
            && self.host_neko_slot_zero_absent()
            && self.host_body_wait_shell(self.state.clock.frame).is_some_and(|shell| {
                (shell.scene, shell.group, shell.record, shell.window, shell.owner)
                    == ("c01_01_01", 13, 121, "meswnd", owner)
            });
        if !eligible {
            return Ok(None);
        }
        self.host_daydream_save_control(owner, "btn", 6)
    }

    pub fn host_daydream_ordinary_save_panel(
        &mut self, anchor: (u32, u64, u32, u64, u64),
    ) -> anyhow::Result<Option<(u32, u32)>> {
        let (context, load, body_owner, epoch, _) = anchor;
        let frame = self.state.clock.frame;
        if self.host_story_trace_profile() != Some(StoryTraceProfile::Daydream)
            || self.frame_in_progress() || !self.state.input.focused || self.data_services.host_pause.paused
            || self.data_services.host_quit_requested || !self.state.input.pressed.is_empty()
            || !self.pending_messages.is_empty() || !self.pending_fes_events.is_empty()
            || !self.data_services.pending_host_input.is_empty() || !self.movie_playbacks.is_empty()
            || !self.host_neko_slot_zero_absent()
        {
            return Ok(None);
        }
        let retained = self.cst_contexts.get(&context).is_some_and(|context| {
            context.story.is_scene("c01_01_01")
                && context.story.settled_body_record(frame).is_some_and(|(_, group, record)| (group, record) == (13, 121))
        });
        let source = self.data_services.fes_text.story_body_sources(body_owner, epoch, &self.data_services.cst_text_origins);
        if !retained || !self.data_services.tasks.task_is_registered(body_owner)
            || !source.is_some_and(|sources| matches!(sources, [source]
                if (source.context_handle, source.load_id, source.group, source.absolute_record)
                    == (context, load, 13, 120)))
        {
            return Ok(None);
        }
        let Some((owner, locals)) = self.host_neko_menu_driver(b"saveload", b"WAIT") else { return Ok(None); };
        let snapshot = self.shared_numeric_state_snapshot()?;
        let root = if snapshot.active_identity == 0 { Some(&snapshot.active_values) } else { snapshot.inactive_values.get(&0) };
        if locals.get(100) != Some(&0) || locals.get(110) != Some(&0)
            || !root.is_some_and(|root| [800, 801, 870].map(|slot| root.get(slot).copied()) == [Some(0), Some(1), Some(0)])
        {
            return Ok(None);
        }
        Ok(self.host_daydream_save_control(owner, "btn_data", 0)?.map(|(_, region)| (owner, region)))
    }

    fn host_daydream_save_control(
        &mut self, owner: u32, name: &str, bank: i32,
    ) -> anyhow::Result<Option<([i32; 2], u32)>> {
        let visible = self.data_services.tasks.fes.runtime(owner).is_some_and(|runtime| {
            let ready = runtime.driver.as_ref().is_some_and(|driver| !driver.initialization_pending());
            let button = runtime.media.get(&format!("{name}[{bank}]"));
            ready && button.is_some_and(|layer| {
                layer.kind == "BUTTON" && layer.enabled && if name == "btn_data" {
                    // The slot card supplies the pixels. The button is its transparent hit region.
                    runtime.media.get(&format!("data[{bank}]")).is_some_and(|card| {
                        card.kind == "SAVEPANEL" && card.visible && card.opacity > 0 && card.image.is_some()
                    })
                } else {
                    layer.visible && layer.opacity > 0
                }
            })
        });
        if !visible {
            return Ok(None);
        }
        let mut entries = self.input_task.regions.iter().filter(|entry| {
            self.host_neko_save_target(owner, entry.id(), name, bank)
        });
        let Some(entry) = entries.next() else { return Ok(None); };
        let region = entry.id();
        let [left, top, right, bottom] = entry.bounds();
        if entries.next().is_some() || self.task_parents.get(&region) != Some(&owner)
            || !self.data_services.tasks.task_is_registered(region)
            || left >= right || top >= bottom || entry.words[8] == 0 || entry.words[9] == 0
        {
            return Ok(None);
        }
        let point = [left.midpoint(right), top.midpoint(bottom)];
        if !self.masked_input_region_at(point)?.is_some_and(|hit| hit[0] == region && hit[8] != 0 && hit[9] != 0)
            || !self.mouse_press_candidate_regions(point)?.contains(&region)
        {
            return Ok(None);
        }
        Ok(Some((point, region)))
    }

    fn host_body_wait_shell(&self, frame: u64) -> Option<BodyWaitShell<'_>> {
        self.host_body_wait_shell_inner(frame, false)
    }

    pub fn host_replay_eden_entry_ready(&self, title: bool) -> bool {
        let frame = self.state.clock.frame;
        self.host_story_trace_profile() == Some(StoryTraceProfile::Legacy13Observation)
            && story_trace_09::quiet_root_at(self, frame, 0xad0).is_some()
            && self.cst_contexts.values().all(|context| context.story.scene.is_empty())
            && (!title || self.data_services.tasks.fes.story_runtimes().any(|(_, name, runtime, _)| {
                name.strip_suffix(b".fes").unwrap_or(name) == b"title"
                    && runtime.driver.as_ref().is_some_and(|driver| {
                        driver.interpreter.current_block() == b"WAIT_USER"
                            && !driver.has_pending_handler()
                    })
            }))
    }

    fn host_body_wait_shell_inner(&self, frame: u64, radio_successor: bool) -> Option<BodyWaitShell<'_>> {
        let profile = self.host_story_trace_profile()?;
        let fes_profile = (profile == StoryTraceProfile::LegacyFesBodyObservation)
            .then(|| legacy_story_wait::legacy_body_profile(self.host_story_pe_sha256()?, self.default_entry()))
            .flatten();
        let (body, flow_wait, permits_idle_touch, address) = match profile {
            StoryTraceProfile::Nekopara => ("meswnd", b"WAIT_GAME".as_slice(), true, 0xd6d),
            StoryTraceProfile::Daydream | StoryTraceProfile::Legacy11Observation =>
                ("cinema", b"GAME_WAIT".as_slice(), false, 0xd6d),
            StoryTraceProfile::Legacy13Observation => ("meswnd", b"LOOP_GAME".as_slice(), false, 0xad0),
            StoryTraceProfile::LegacyFesBodyObservation => {
                let profile = fes_profile?;
                (profile.window(), profile.flow_block, false, profile.root_wait)
            }
            _ => return None,
        };
        let legacy = self.legacy_11_13_profile();
        if !self.data_services.story_trace || self.frame_in_progress() || !self.state.input.focused
            || self.data_services.host_pause.paused || self.data_services.host_quit_requested
            || !self.pending_messages.is_empty()
            || !self.data_services.pending_host_input.is_empty()
            || self.message_wait_address != Some(address)
            || !self.vm.as_ref().is_some_and(|vm| vm.waiting_for_message())
        {
            return None;
        }
        let mut contexts = self.cst_contexts.iter().map(|(&id, context)| (id, &context.story))
            .filter(|(_, story)| !story.scene.is_empty() && story.loaded <= frame);
        let (context, story) = contexts.next()?;
        if contexts.next().is_some() {
            return None;
        }
        let (marked, group, record) = story.settled_body_record(frame)?;
        let movie_ready = match legacy {
            Some(StoryTraceProfile::Legacy11Observation) if story.is_scene("00_001") && (group, record) == (3, 94) =>
                if radio_successor { self.legacy_11_radio_movie() } else { self.legacy_11_radio_movie_stopped() },
            Some(StoryTraceProfile::Legacy11Observation)
                if story.is_scene("00_001") && (group, record) == (4, 97) =>
                radio_successor && (self.legacy_11_radio_movie() || self.legacy_11_radio_movie_stopped()),
            Some(StoryTraceProfile::Legacy11Observation) => self.legacy_11_radio_movie(),
            _ => self.movie_playbacks.is_empty(),
        };
        if !movie_ready {
            return None;
        }
        let body = match (profile, daydream_case_records(&story.scene)) {
            (StoryTraceProfile::LegacyFesBodyObservation, _) =>
                fes_profile?.body_window(&story.scene, group),
            (StoryTraceProfile::Daydream, Some((window, pages)))
                if usize::try_from(group).ok().and_then(|group| pages.get(group))
                    .is_some_and(|&(wait, _)| wait == record) => window,
            (StoryTraceProfile::Daydream, _) if story.scene != "prologue" => {
                let mut active = self.data_services.tasks.fes.story_runtimes().filter_map(|(_, name, _, wait_key)| {
                    if !wait_key {
                        return None;
                    }
                    match name.strip_suffix(b".fes").unwrap_or(name) {
                        b"meswnd" => Some("meswnd"),
                        b"cinema" => Some("cinema"),
                        b"messh" => Some("messh"),
                        _ => None,
                    }
                });
                let window = active.next()?;
                if active.next().is_some() {
                    return None;
                }
                window
            }
            _ => body,
        };
        if legacy.is_some_and(|profile| legacy_11_13_records(profile, &story.scene, group, record).is_none()
            || !self.loaded.as_ref().is_some_and(|entry| entry.kind == "KCS" && entry.name == "kcs.int/main.kcs"))
        {
            return None;
        }
        let mut body_owner = None;
        let mut settled_flow = false;
        let mut ambient_line = None;
        for (owner, name, runtime, wait_key) in self.data_services.tasks.fes.story_runtimes() {
            let name = name.strip_suffix(b".fes").unwrap_or(name);
            let driver = runtime.driver.as_ref()?;
            let block = driver.interpreter.current_block();
            if legacy.is_some() && (driver.initialization_pending()
                || !self.data_services.tasks.task_is_registered(owner))
            {
                return None;
            }
            if name.eq_ignore_ascii_case(body.as_bytes()) {
                let wait_key = fes_profile.map_or(wait_key,
                    |profile| profile.wait_signal(wait_key, driver.interpreter.local_values()));
                let main_wait = (profile == StoryTraceProfile::Daydream && daydream_meswnd_main_wait(name, driver))
                    || (profile == StoryTraceProfile::LegacyFesBodyObservation
                        && matches!(name, b"cinema" | b"meswnd")
                        && driver.newton_message_fes_source == Some(true) && block == b"WAIT");
                let confirmed = if main_wait { b"MES_SHOW".as_slice() } else { block };
                if !settled_body_window(confirmed, wait_key, driver.has_pending_handler(), driver.waiting(), frame)
                    || body_owner.replace(owner).is_some()
                {
                    return None;
                }
            } else {
                if wait_key || driver.has_pending_handler()
                    || !matches!(driver.waiting(), Some(FesDriverWait::Frame { after }) if *after == frame)
                {
                    return None;
                }
                if name.eq_ignore_ascii_case(b"flow") && block == flow_wait && !settled_flow {
                    settled_flow = true;
                } else if address == 0xe0d && name.eq_ignore_ascii_case(b"line2") && block == b"WAIT" {
                    if driver.initialization_pending() || ambient_line.replace(owner).is_some() {
                        return None;
                    }
                } else if !(permits_idle_touch && name.eq_ignore_ascii_case(b"touch") && block == b"WAIT") {
                    return None;
                }
            }
        }
        let owner = body_owner?;
        if let Some(line) = ambient_line
            && !story_trace_09::live_child(self, line, *self.task_parents.get(&owner)?, 34)
        {
            return None;
        }
        let live_owner = self.data_services.tasks.task_is_registered(owner)
            && (self.task_parents.contains_key(&owner) || self.scene_nodes.contains_key(&owner));
        (settled_flow && live_owner).then_some(BodyWaitShell {
            window: body, owner, scene: story.scene.as_str(), context, loaded: story.loaded,
            marked, group, record,
        })
    }

    fn host_current_body(&self, shell: BodyWaitShell<'_>, frame: u64) -> (Option<&[CstBodySource]>, bool, bool) {
        let Some(generation) = self.data_services.fes_text.story_body_generation(shell.owner) else {
            return (None, false, false);
        };
        let sources = self.data_services.fes_text.story_body_sources(
            shell.owner, generation.source_epoch, &self.data_services.cst_text_origins,
        );
        let sources_match = self.cst_contexts.get(&shell.context)
            .is_some_and(|context| self.host_body_sources_match(context, shell, sources));
        let committed = sources_match && self.host_body_committed(shell, generation, frame);
        let composited = committed && self.host_body_list_submitted(shell);
        (sources, sources_match, composited)
    }

    fn host_body_sources_match(&self, context: &super::CstContextState, shell: BodyWaitShell<'_>,
        sources: Option<&[CstBodySource]>,
    ) -> bool {
        let Some(document) = context.document.as_ref() else { return false; };
        let localized = (self.host_story_trace_profile() == Some(StoryTraceProfile::Nekopara)).then(|| {
            let language = self.data_services.language.languages.prefix(1)?;
            context.languages.binary_search_by(|name| name.as_slice().cmp(language)).ok()?;
            let table = context.localization.as_ref()?;
            let column = table.languages.iter().position(|name| name.as_bytes() == language)?;
            let message = table.messages.get(shell.group as usize).and_then(|row| row.get(column));
            Some(message.map_or("", |message| message.content.as_str()))
        }).flatten();
        body_sources_match(document, shell, sources, localized)
    }

    fn host_eden_layout_complete(&self, shell: BodyWaitShell<'_>) -> Option<(u64, u64)> {
        (self.legacy_11_13_profile() == Some(StoryTraceProfile::Legacy13Observation)
            && shell.window == "meswnd" && shell.scene == "BLAOP1_END"
            && legacy_11_13_records(StoryTraceProfile::Legacy13Observation, shell.scene, shell.group, shell.record)
                .is_some_and(|(probe, records)| probe == super::cst_context_native::CstSourceProbe::Eden
                    && records.len() == 1))
            .then(|| self.data_services.fes_text.eden_layout_complete(shell.owner)).flatten()
    }

    fn host_body_committed(&self, shell: BodyWaitShell<'_>, generation: StoryBodyGeneration, frame: u64) -> bool {
        let yukikoi_layout = || {
            (shell.scene == "__tar01" && shell.window == "meswnd"
                && self.host_story_pe_sha256() == Some(legacy_story_wait::YUKIKOI_PE))
                .then(|| self.data_services.fes_text.eden_layout_complete(shell.owner)).flatten()
        };
        committed_body_at(generation, generation.natural_complete
            .or_else(yukikoi_layout).or_else(|| self.host_eden_layout_complete(shell)),
            shell.loaded, shell.marked, frame)
    }

    pub fn host_replay_body_route_anchor(
        &self, scene: &str, anchor: (i32, i32), next: Option<(i32, i32)>, owner: u32,
    ) -> Option<(u32, u64, u32, u64, u64)> {
        if self.legacy_11_13_profile().is_some() {
            return None;
        }
        let frame = self.state.clock.frame;
        let shell = self.host_body_wait_shell(frame)?;
        if (shell.scene, shell.group, shell.record, shell.owner) != (scene, anchor.0, anchor.1, owner) {
            return None;
        }
        let document = self.cst_contexts.get(&shell.context)?.document.as_ref()?;
        if !adjacent_cst_input_waits(document, anchor, next) {
            return None;
        }
        let (sources, _, composited) = self.host_current_body(shell, frame);
        if !composited {
            return None;
        }
        let sources = sources?;
        if shell.window == "cinema" && scene == "prologue" {
            let empty_case = match anchor {
                (20, 96) => Some((90, 95, [38, 48])),
                (40, 221) => Some((215, 220, [36, 42])),
                _ => None,
            };
            if let Some((body, empty, lengths)) = empty_case
                && (!matches!(sources, [source] if source.absolute_record == body)
                    || !daydream_empty_body_lengths(document, anchor, body, empty)
                        .is_some_and(|[body_bytes, empty_bytes]| lengths.contains(&body_bytes) && empty_bytes == 0))
            {
                return None;
            }
        }
        let load = sources.first()?.load_id;
        let generation = self.data_services.fes_text.story_body_generation(owner)?;
        Some((shell.context, load, owner, generation.source_epoch, generation.source_frame))
    }

    /// Timing and source records for a matching, previously composited NEKO anchor.
    pub fn host_neko_cstl_body_proof(
        &self, scene: &str, position: (i32, i32), anchor: (u32, u64, u32, u64, u64),
    ) -> Option<(u64, u64, u64, Vec<usize>)> {
        let (context, load, owner, epoch, source_frame) = anchor;
        let frame = self.state.clock.frame;
        let shell = self.host_body_wait_shell(frame)?;
        if self.host_story_trace_profile()? != StoryTraceProfile::Nekopara
            || (shell.scene, shell.group, shell.record, shell.context, shell.owner)
                != (scene, position.0, position.1, context, owner)
        {
            return None;
        }
        let generation = self.data_services.fes_text.story_body_generation(owner)?;
        let (complete_epoch, complete) = generation.natural_complete?;
        if (generation.source_epoch, generation.source_frame, complete_epoch) != (epoch, source_frame, epoch)
            || !self.host_body_committed(shell, generation, frame)
        {
            return None;
        }
        let sources = self.data_services.fes_text.story_body_sources(
            owner, epoch, &self.data_services.cst_text_origins,
        )?;
        if sources.is_empty() || !sources.iter().all(|source| {
            (source.context_handle, source.load_id, source.group) == (context, load, position.0)
        }) {
            return None;
        }
        Some((shell.loaded, shell.marked, complete, sources.iter().map(|source| source.absolute_record).collect()))
    }

    fn host_body_list_submitted(&self, shell: BodyWaitShell<'_>) -> bool {
        self.host_body_list_components(shell).is_some_and(|(body, prompt)| {
            body && (prompt || self.host_daydream_meswnd_keywait_visible(shell) == Some(false))
        })
    }

    fn host_daydream_meswnd_keywait_visible(&self, shell: BodyWaitShell<'_>) -> Option<bool> {
        if self.host_story_trace_profile() != Some(StoryTraceProfile::Daydream)
            || shell.window != "meswnd" || shell.scene == "prologue"
        {
            return None;
        }
        let (_, _, runtime, _) = self.data_services.tasks.fes.story_runtimes().find(|(owner, name, _, wait_key)| {
            *owner == shell.owner && *wait_key && name.strip_suffix(b".fes").unwrap_or(name) == b"meswnd"
        })?;
        let driver = runtime.driver.as_ref()?;
        if driver.initialization_pending()
            || (driver.interpreter.current_block() != b"MES_SHOW" && !daydream_meswnd_main_wait(b"meswnd", driver))
            || !driver.interpreter.program.block(b"MES_SHOW")?.lines.iter().any(|line| {
                line.bytes.split(|byte| byte.is_ascii_whitespace()).filter(|part| !part.is_empty())
                    .eq([b"keywait".as_slice(), b"disp", b"0"])
            })
        {
            return None;
        }
        Some(runtime.media.get("keywait")?.visible)
    }

    fn host_body_list_components(&self, shell: BodyWaitShell<'_>) -> Option<(bool, bool)> {
        let platform = &self.data_services.tasks.fes.platform;
        let mut lists = platform.lists.values().filter_map(|list| {
            let layer = list.layers.get(&(shell.owner, "\0text:737472[0]".to_owned()))?;
            Some((list, layer))
        });
        let (list, layer) = lists.next()?;
        if lists.next().is_some() || !list.enabled || !list.submit {
            return None;
        }
        let output = self.image_handles.get(&list.output)?;
        let body = submitted_body_pixels(layer, output);
        let prompt = !matches!(shell.window, "meswnd" | "meswnd01")
            || self.legacy_11_13_profile() == Some(StoryTraceProfile::Legacy13Observation)
            || list.layers
            .get(&(shell.owner, legacy_story_wait::legacy_body_profile(
                self.host_story_pe_sha256().unwrap_or_default(), self.default_entry(),
            ).map_or("keywait", |profile| profile.prompt).to_owned()))
            .is_some_and(|prompt| submitted_body_pixels(prompt, output));
        Some((body, prompt))
    }

    fn host_observed_body_owner(&self) -> Option<u32> {
        let name = match self.host_story_trace_profile()? {
            StoryTraceProfile::FirstBodyObservation => b"meswnd".as_slice(),
            StoryTraceProfile::Nekopara => b"meswnd".as_slice(),
            StoryTraceProfile::Daydream | StoryTraceProfile::Legacy11Observation => b"cinema".as_slice(),
            StoryTraceProfile::Legacy13Observation => b"meswnd".as_slice(),
            StoryTraceProfile::LegacyFesBodyObservation =>
                legacy_story_wait::legacy_body_profile(self.host_story_pe_sha256()?, self.default_entry())?
                    .window().as_bytes(),
            StoryTraceProfile::Rebellions => b"meswnd01".as_slice(),
            _ => return None,
        };
        let mut owners = self.data_services.tasks.fes.story_runtimes().filter_map(|(owner, candidate, _, _)| {
            candidate.strip_suffix(b".fes").unwrap_or(candidate).eq_ignore_ascii_case(name).then_some(owner)
        });
        let owner = owners.next()?;
        owners.next().is_none().then_some(owner)
    }


    fn legacy_11_13_profile(&self) -> Option<StoryTraceProfile> {
        let profile = self.host_story_trace_profile()?;
        matches!(profile, StoryTraceProfile::Legacy11Observation | StoryTraceProfile::Legacy13Observation)
            .then_some(profile)
    }

    fn legacy_11_radio_movie(&self) -> bool {
        let Some((&handle, &playback)) = self.movie_playbacks.iter().next() else { return false; };
        if self.movie_playbacks.len() != 1
            || self.movie_resources.get(&handle).map(String::as_str) != Some("movie.int/radio_wave.mpg")
        {
            return false;
        }
        self.movie_host.planes.movies.get(&handle).is_some_and(|movie| {
            movie.target == 31 && matches!(movie.player.replay_playback_sample(playback),
                Ok(Some((true, Some(pts)))) if pts.is_finite())
                && self.movie_host.scene_movie_presentations().iter().any(|receipt| {
                    receipt.playback == playback && receipt.time.is_finite()
                        && std::sync::Arc::ptr_eq(&receipt.surface, &movie.player.frame_surface())
                })
        })
    }

    fn legacy_11_radio_movie_stopped(&self) -> bool {
        let mut playbacks = self.movie_playbacks.iter();
        let Some((&handle, &playback)) = playbacks.next() else { return true; };
        if playbacks.next().is_some()
            || self.movie_resources.get(&handle).map(String::as_str) != Some("movie.int/radio_wave.mpg")
        {
            return false;
        }
        self.movie_host.planes.movies.get(&handle).is_some_and(|movie| {
            movie.target == 31 && matches!(movie.player.replay_playback_sample(playback), Ok(None))
                && !self.movie_host.scene_movie_presentations().iter().any(|receipt| {
                    receipt.playback == playback && std::sync::Arc::ptr_eq(&receipt.surface, &movie.player.frame_surface())
                })
        })
    }

    /// Exact read-only page and live Enter binding for the scoped 11/13 waits.
    /// The normal story trace deliberately exposes these profiles as observation only.
    pub fn host_replay_11_13_body(&self, group: i32, record: i32) -> Option<((u32, u32), (u32, u32, u64), (u64, u64, u64))> {
        self.host_replay_11_13_body_inner(group, record, false)
    }

    pub fn host_replay_11_radio_body(&self, group: i32, record: i32)
        -> Option<((u32, u32), (u32, u32, u64), (u64, u64, u64))>
    {
        if self.legacy_11_13_profile()? != StoryTraceProfile::Legacy11Observation {
            return None;
        }
        self.host_replay_11_13_body_inner(group, record, true)
    }

    fn host_replay_11_13_body_inner(&self, group: i32, record: i32, radio_successor: bool)
        -> Option<((u32, u32), (u32, u32, u64), (u64, u64, u64))>
    {
        let frame = self.state.clock.frame;
        let profile = self.legacy_11_13_profile()?;
        let shell = self.host_body_wait_shell_inner(frame, radio_successor)?;
        if (shell.group, shell.record) != (group, record) {
            return None;
        }
        let (probe, expected) = legacy_11_13_records(profile, shell.scene, group, record)?;
        let (sources, _, composed) = self.host_current_body(shell, frame);
        let sources = sources?;
        let origins = &self.data_services.cst_text_origins;
        let generation = self.data_services.fes_text.story_body_generation(shell.owner)?;
        if !composed || !sources.iter().map(|source| source.absolute_record).eq(expected.iter().copied())
            || !sources.iter().all(|source| origins.source_record_observations(shell.context).iter().any(|entry| {
                entry.source == *source && entry.probe == probe && entry.body_tagged && !entry.localized
                    && entry.kind == 0x20 && entry.raw_bytes > 0 && entry.output_bytes > 0
                    && shell.loaded <= entry.frame && entry.frame <= generation.source_frame
                    && match probe {
                        super::cst_context_native::CstSourceProbe::August => entry.native == 1037
                            && entry.stored_codec == super::ScriptTextEncoding::Cp932
                            && entry.output_codec == super::ScriptTextEncoding::Utf8,
                        super::cst_context_native::CstSourceProbe::Eden => entry.native == 327
                            && entry.stored_codec == super::ScriptTextEncoding::Gbk
                            && entry.output_codec == super::ScriptTextEncoding::Gbk,
                        _ => false,
                    }
            }))
        {
            return None;
        }
        let binding = self.legacy_bottom_enter(shell.owner)?;
        Some((binding, (shell.owner, shell.context, sources.first()?.load_id),
            (generation.source_epoch, generation.source_frame, shell.marked)))
    }

    fn legacy_bottom_enter(&self, owner: u32) -> Option<(u32, u32)> {
        self.legacy_bottom_enter_for_kind(owner, 34)
    }

    fn legacy_bottom_enter_for_kind(&self, owner: u32, kind: u32) -> Option<(u32, u32)> {
        use cs2_runtime::message_object::ButtonSource;
        let services = &self.data_services;
        let input = &services.messages;
        if !self.state.input.pressed.is_empty() || !self.pending_fes_events.is_empty()
            || services.tasks.pending_root_dispatch().is_some()
            || !self.input_task.alive || !input.device_keys_loaded || !input.device_foreground
            || !input.physical_buttons.is_empty() || !input.physical_edges.is_empty()
            || !input.pending_device_buttons.is_empty() || input.key_dispatch.is_some()
            || input.key_selection.is_some() || !input.button_dispatches.is_empty()
            || !input.button_active.is_empty() || input.key_capture.contains_key(&2)
            || input.typed_physical_owners.contains_key(&(ButtonSource::Keyboard, 13))
            || input.device_keys.forced_queries(0).ok()? != 0
            || input.device_keys.forced_queries(2).ok()? != 0
            || !self.device_key_scan_state().records().iter().flatten().all(|key| key.mask & 3 == 0)
            || !story_trace_09::enter_mapping(self.device_key_bindings(), &self.input_task.key_enabled)
        {
            return None;
        }
        let mut buttons = self.scene_nodes.iter().filter(|(_, node)| node.name == "bottom button");
        let (&region, _) = buttons.next()?;
        let receiver = *self.task_parents.get(&owner)?;
        let parent = *self.task_parents.get(&receiver)?;
        if buttons.next().is_some() || !story_trace_09::live_child(self, region, receiver, 6)
            || !story_trace_09::live_child(self, receiver, parent, 0)
            || !story_trace_09::live_child(self, owner, receiver, kind)
            || !services.tasks.task_clock(receiver).is_some_and(|clock| clock.paused == 0)
            || self.task_vms.get(&receiver)?.program().is_none()
        {
            return None;
        }
        let mut registrations = self.input_task.regions.iter().filter(|entry| entry.id() == region);
        let words = &registrations.next()?.words;
        let button = input.input_buttons.get(&region)?;
        (registrations.next().is_none() && button.execution_checkpoint().3
            && words[2] & ButtonSource::Keyboard.mask() != 0 && words[8] != 0 && words[9] != 0
            && words[11] == 0 && button.state.id == region && button.state.receiver == Some(receiver)
            && button.state.show != 0 && button.state.full_screen && button.state.parameter == 0
            && !input.input_masks.contains_key(&region))
            .then_some((region, receiver))
    }

    pub fn host_replay_wait_diagnostics(&self) -> String {
        let (frame, state, scene, group, record) = self.host_story_trace();
        let eyecatch_08 = (self.data_services.story_trace
            && self.host_story_trace_profile() == Some(StoryTraceProfile::Eyecatch08Observation))
            .then(|| story_trace_08::observe(self, frame)).flatten();
        let legacy_07 = (self.data_services.story_trace
            && self.host_story_trace_profile() == Some(StoryTraceProfile::Legacy07Observation))
            .then(|| legacy_story_wait::observe_07(self, frame)).flatten();
        let contexts: Vec<_> = self.cst_contexts.iter().map(|(id, context)| {
            (id, &context.story.scene, context.story.loaded, context.story.candidate, context.story.page)
        }).collect();
        let windows: Vec<_> = self.data_services.tasks.fes.story_runtimes()
            .map(|(owner, name, runtime, wait_key)| {
                let driver = runtime.driver.as_ref().map(|driver| {
                    (String::from_utf8_lossy(driver.interpreter.current_block()),
                        driver.has_pending_handler(), driver.waiting())
                });
                let novel_prompt = name.strip_suffix(b".fes").unwrap_or(name)
                    .eq_ignore_ascii_case(b"novelwnd").then(|| {
                        let locals = runtime.driver.as_ref().map(|driver| driver.interpreter.local_values());
                        let local = |slot| locals.and_then(|values| values.get(slot).copied());
                        let display = |name| runtime.media.get(name).map(|layer| (layer.visible, layer.opacity));
                        (local(33), display("img_wait[1]"), display("img_anim"), local(30))
                    });
                (owner, String::from_utf8_lossy(name), wait_key, driver,
                    self.data_services.fes_text.story_novel_segment_ready(owner), novel_prompt)
            }).collect();
        let body_shell = self.host_body_wait_shell(frame).or_else(|| story_trace_20::body_shell(self, frame));
        let radio_shell = (self.legacy_11_13_profile() == Some(StoryTraceProfile::Legacy11Observation))
            .then(|| self.host_body_wait_shell_inner(frame, true)).flatten()
            .filter(|shell| shell.scene == "00_001" && matches!((shell.group, shell.record), (3, 94) | (4, 97)));
        let radio_r94_enter = radio_shell
            .and_then(|shell| self.host_replay_11_radio_body(shell.group, shell.record))
            .map(|(binding, _, _)| binding);
        let body_shell = body_shell.or(radio_shell);
        let body_wait_shell = body_shell.map(|shell| {
            (shell.window, shell.owner, shell.scene, shell.loaded, shell.marked, shell.group, shell.record)
        });
        let body_page = body_shell.map(|shell| {
            let (sources, matches, composited) = self.host_current_body(shell, frame);
            (shell.context, sources, matches, composited)
        });
        let eden_layout_complete = body_shell.and_then(|shell| self.host_eden_layout_complete(shell));
        let eden_live_enter = body_shell.filter(|shell| {
            self.legacy_11_13_profile() == Some(StoryTraceProfile::Legacy13Observation)
                && legacy_11_13_records(StoryTraceProfile::Legacy13Observation, shell.scene, shell.group, shell.record).is_some()
        }).and_then(|shell| self.legacy_bottom_enter(shell.owner));
        let labyrinth15 = (self.host_story_trace_profile() == Some(StoryTraceProfile::Labyrinth15Observation))
            .then(|| story_trace_15::observe(self, frame)).flatten();
        let djibril10 = (self.host_story_trace_profile() == Some(StoryTraceProfile::Djibril10Observation))
            .then(|| story_trace_10::observe(self, frame)).flatten();
        let legacy_fes_body = self.legacy_fes_body_diagnostic(frame);
        let legacy_11_13_enter = body_shell
            .and_then(|shell| self.host_replay_11_13_body(shell.group, shell.record))
            .map(|(binding, _, _)| binding);
        let daydream_empty_body = body_shell.filter(|shell| shell.window == "cinema" && shell.scene == "prologue")
            .and_then(|shell| {
                let (body, empty) = match (shell.group, shell.record) {
                    (20, 96) => (90, 95),
                    (40, 221) => (215, 220),
                    _ => return None,
                };
                let document = self.cst_contexts.get(&shell.context)?.document.as_ref()?;
                let lengths = daydream_empty_body_lengths(document, (shell.group, shell.record), body, empty)?;
                Some((shell.context, body, lengths[0], empty, lengths[1]))
            });
        let kanmi_meswnd = (self.host_story_trace_profile() == Some(StoryTraceProfile::KanmiObservation))
            .then(|| self.data_services.tasks.fes.story_runtimes().filter_map(|(owner, name, runtime, _)| {
                if !name.strip_suffix(b".fes").unwrap_or(name).eq_ignore_ascii_case(b"meswnd") {
                    return None;
                }
                let locals = runtime.driver.as_ref().map(|driver| driver.interpreter.local_values());
                let slot = |index| locals.and_then(|values| values.get(index)).copied();
                let keywait = runtime.media.get("keywait").map(|layer| (layer.visible, layer.opacity));
                Some((owner, slot(100), slot(52), slot(101), keywait,
                    self.data_services.fes_text.story_body_generation(owner)))
            }).collect::<Vec<_>>());
        let daydream_meswnd = body_shell.and_then(|shell| {
            let keywait_visible = self.host_daydream_meswnd_keywait_visible(shell)?;
            let generation = self.data_services.fes_text.story_body_generation(shell.owner);
            let committed = generation.is_some_and(|item| self.host_body_committed(shell, item, frame));
            Some((shell.owner, generation, committed, self.host_body_list_components(shell),
                keywait_visible, self.legacy_bottom_enter(shell.owner)))
        });
        let body_generation = self.host_observed_body_owner().map(|owner| {
            let generation = self.data_services.fes_text.story_body_generation(owner);
            let current_committed = body_shell.zip(body_page).zip(generation).is_some_and(|((shell, body), generation)| {
                owner == shell.owner && body.2 && self.host_body_committed(shell, generation, frame)
            });
            (owner, generation, current_committed)
        });
        let first_body_sources = matches!(self.host_story_trace_profile(),
            Some(StoryTraceProfile::FirstBodyObservation | StoryTraceProfile::LegacyFesBodyObservation))
            .then(|| {
                let (owner, generation, _) = body_generation?;
                self.data_services.fes_text.story_body_sources(owner, generation?.source_epoch,
                    &self.data_services.cst_text_origins)
            }).flatten();
        let first_body_keyboard_regions = matches!(self.host_story_trace_profile(),
            Some(StoryTraceProfile::FirstBodyObservation | StoryTraceProfile::LegacyFesBodyObservation))
            .then(|| {
                let input = &self.data_services.messages;
                self.input_task.regions.iter().filter_map(|registration| {
                    let region = registration.id();
                    let button = input.input_buttons.get(&region)?;
                    let receiver = button.state.receiver?;
                    (button.execution_checkpoint().3
                        && self.data_services.tasks.task_is_registered(receiver)
                        && registration.words[8] != 0
                        && registration.words[2] & cs2_runtime::message_object::ButtonSource::Keyboard.mask() != 0)
                        .then_some((region, receiver))
                }).collect::<Vec<_>>()
            });
        let neko_startup = (self.host_story_trace_profile() == Some(StoryTraceProfile::Nekopara)
            && self.cst_contexts.is_empty()).then(|| {
            let languages = &self.data_services.language.languages;
            let local = self.data_services.tasks.fes.story_runtimes().find_map(|(owner, name, _, _)| {
                name.strip_suffix(b".fes").unwrap_or(name).eq_ignore_ascii_case(b"langselect")
                    .then(|| self.neko_language_locals(owner)).flatten()
            });
            ([languages.primary_index, languages.message_index, languages.subtitle_index], local)
        });
        let daydream_save_panel = self.host_daydream_save_panel_diagnostic();
        let rebellions_input = story_trace_20::ordinary_input_diagnostic(self, frame);
        format!(
            "trace_enabled={} profile={:?} pe_sha256={:?} startup={:?} \
             frame={frame} state={state} scene={scene} group={group} record={record} \
             focused={} paused={} message_wait={:?} vm_waiting_for_message={} pending_messages={} movies={} \
             contexts(id,scene,loaded,candidate,page)={contexts:?} \
             windows(owner,name,wait_key,driver(block,pending,wait),novel_ready,novel_prompt(mode,input_icon,page_anim,draw_active))={windows:?} \
             body_wait_shell(window,owner,scene,loaded,marked,group,record)={body_wait_shell:?} \
             body_generation(owner,generation,current_committed)={body_generation:?} \
             body_page(context,sources,exact_match,composited)={body_page:?} \
             daydream_meswnd(owner,generation,committed,body_and_prompt_submitted,keywait_visible,enter)={daydream_meswnd:?} \
             first_body_sources={first_body_sources:?} first_body_keyboard_regions={first_body_keyboard_regions:?} \
             kanmi_verified_pe={:?} kanmi_meswnd(owner,local100,local52,local101,keywait,generation)={kanmi_meswnd:?} \
             neko_startup(config_ui_main_sub,locals55_56_57)={neko_startup:?} \
             legacy_fes_body={legacy_fes_body:?} legacy_07={legacy_07:?} daydream_empty_body(context,body,raw_bytes,empty,raw_bytes)={daydream_empty_body:?} \
             eyecatch_08(window,owner)={eyecatch_08:?} legacy_11_13_enter={legacy_11_13_enter:?} \
             eden_layout_complete={eden_layout_complete:?} eden_live_enter={eden_live_enter:?} labyrinth15={labyrinth15:?} \
             djibril10={djibril10:?} radio_r94_enter={radio_r94_enter:?} daydream_save_panel={daydream_save_panel:?} \
             rebellions_input={rebellions_input:?}",
            self.data_services.story_trace,
            self.host_story_trace_profile(),
            self.host_story_pe_sha256(),
            self.default_entry(),
            self.state.input.focused,
            self.data_services.host_pause.paused,
            self.message_wait_address,
            self.vm.as_ref().is_some_and(|vm| vm.waiting_for_message()),
            self.pending_messages.len(),
            self.movie_playbacks.len(),
            self.verified_kanmi_runtime_pe_sha256(),
        )
    }
}

fn legacy_11_13_records(
    profile: StoryTraceProfile, scene: &str, group: i32, wait: i32,
) -> Option<(super::cst_context_native::CstSourceProbe, &'static [usize])> {
    use super::cst_context_native::CstSourceProbe::{August, Eden};
    match (profile, scene, group, wait) {
        (StoryTraceProfile::Legacy11Observation, "00_001", 0, 81) => Some((August, &[79, 80])),
        (StoryTraceProfile::Legacy11Observation, "00_001", 1, 86) => Some((August, &[84, 85])),
        (StoryTraceProfile::Legacy11Observation, "00_001", 2, 90) => Some((August, &[89])),
        (StoryTraceProfile::Legacy11Observation, "00_001", 3, 94) => Some((August, &[93])),
        (StoryTraceProfile::Legacy11Observation, "00_001", 4, 97) => Some((August, &[96])),
        (StoryTraceProfile::Legacy13Observation, "BLAOP1_END", 4, 103) => Some((Eden, &[102])),
        (StoryTraceProfile::Legacy13Observation, "BLAOP1_END", 5, 107) => Some((Eden, &[106])),
        (StoryTraceProfile::Legacy13Observation, "BLAOP1_END", 6, 109) => Some((Eden, &[108])),
        (StoryTraceProfile::Legacy13Observation, "BLAOP1_END", 7, 111) => Some((Eden, &[110])),
        (StoryTraceProfile::Legacy13Observation, "BLAOP1_END", 8, 113) => Some((Eden, &[112])),
        (StoryTraceProfile::Legacy13Observation, "BLAOP1_END", 9, 115) => Some((Eden, &[114])),
        (StoryTraceProfile::Legacy13Observation, "BLAOP1_END", 10, 119) => Some((Eden, &[118])),
        (StoryTraceProfile::Legacy13Observation, "BLAOP1_END", 11, 121) => Some((Eden, &[120])),
        (StoryTraceProfile::Legacy13Observation, "BLAOP1_END", 12, 139) => Some((Eden, &[138])),
        (StoryTraceProfile::Legacy13Observation, "BLAOP1_END", 13, 151) => Some((Eden, &[150])),
        (StoryTraceProfile::Legacy13Observation, "BLAOP1_END", 14, 163) => Some((Eden, &[162])),
        (StoryTraceProfile::Legacy13Observation, "BLAOP1_END", 15, 174) => Some((Eden, &[173])),
        (StoryTraceProfile::Legacy13Observation, "BLAOP1_END", 16, 186) => Some((Eden, &[185])),
        (StoryTraceProfile::Legacy13Observation, "BLAOP1_END", 17, 188) => Some((Eden, &[187])),
        _ => None,
    }
}

type DaydreamCasePage = (i32, &'static [usize]);

fn daydream_case_records(scene: &str) -> Option<(&'static str, [DaydreamCasePage; 2])> {
    match scene {
        "c01_01_01" => Some(("cinema", [(66, &[65]), (68, &[67])])),
        "c02_01_01" => Some(("meswnd", [(66, &[64, 65]), (71, &[70])])),
        "c03_01_01" => Some(("cinema", [(62, &[60, 61]), (65, &[64])])),
        _ => None,
    }
}

fn body_sources_match(document: &CstDocument, shell: BodyWaitShell<'_>, sources: Option<&[CstBodySource]>,
    localized: Option<&str>,
) -> bool {
    let Some(sources) = sources.filter(|sources| !sources.is_empty()) else { return false; };
    let (Some(range), Ok(wait)) = (document.inputs.get(shell.group as usize), usize::try_from(shell.record)) else {
        return false;
    };
    if !range.contains(&wait) || document.records.get(wait).is_none_or(|record| record.kind != 0x02) {
        return false;
    }
    let load = sources[0].load_id;
    if load == 0 || !sources.iter().all(|source| {
        source.context_handle == shell.context && source.load_id == load && source.group == shell.group
    }) {
        return false;
    }
    let Some(prefix) = document.records.get(range.start..wait) else { return false; };
    let start = prefix.iter().rposition(|record| matches!(record.kind, 0x02 | 0x03)).map_or(0, |index| index + 1);
    let mut fragments = localized.map(|message| message.split('\u{1f}'));
    prefix.iter().enumerate().filter(|(_, record)| record.kind == 0x20)
        .filter_map(|(index, record)| {
            let text = fragments.as_mut().map_or(record.text.as_slice(), |fragments| {
                fragments.next().unwrap_or("").split('\0').next().unwrap_or("").as_bytes()
            });
            (index >= start && !body_control_only(text)).then_some(range.start + index)
        })
        .eq(sources.iter().map(|source| source.absolute_record))
}

fn submitted_body_pixels(layer: &Layer, output: &RgbaFrame) -> bool {
    let options = &layer.options;
    if !options.visible || options.opacity == 0 || options.mask.is_some() || layer.affine.is_some()
        || options.scale_x != 1.0 || options.scale_y != 1.0 || options.rotation_radians != 0.0
        || options.pivot != [0.0; 2] || options.flip_x || options.flip_y || layer.frame.width() == 0
    {
        return false;
    }
    let width = layer.frame.width() as usize;
    layer.frame.pixels().chunks_exact(4).enumerate().any(|(index, pixel)| {
        if pixel[3] == 0 { return false; }
        let x = i64::from(options.position.x) + (index % width) as i64;
        let y = i64::from(options.position.y) + (index / width) as i64;
        (0..i64::from(output.width())).contains(&x) && (0..i64::from(output.height())).contains(&y)
            && options.clip.is_none_or(|clip| {
                (i64::from(clip.x)..i64::from(clip.x) + i64::from(clip.width)).contains(&x)
                    && (i64::from(clip.y)..i64::from(clip.y) + i64::from(clip.height)).contains(&y)
            })
            && output.pixels()[(y as usize * output.width() as usize + x as usize) * 4 + 3] != 0
    })
}

fn current_committed_body(generation: StoryBodyGeneration, loaded: u64, marked: u64, frame: u64) -> bool {
    committed_body_at(generation, generation.natural_complete, loaded, marked, frame)
}

fn committed_body_at(
    generation: StoryBodyGeneration, completion: Option<(u64, u64)>, loaded: u64, marked: u64, frame: u64,
) -> bool {
    let Some((complete_epoch, complete_frame)) = completion else { return false; };
    generation.source_epoch != 0
        && loaded <= marked && marked < frame && loaded <= generation.source_frame
        && generation.source_frame <= complete_frame && complete_frame <= frame
        && generation.source_epoch == complete_epoch
        && generation.committed_epoch == Some(complete_epoch)
        && generation.remaining == 0 && generation.remaining_segment == 0
        && !generation.pending_text && !generation.drawing && !generation.skip_pending && !generation.forced
        && generation.projection.is_some_and(|(visible, opacity)| visible && opacity > 0)
}

fn settled_body_window(
    block: &[u8], wait_key: bool, pending: bool, waiting: Option<&FesDriverWait>, frame: u64,
) -> bool {
    block == b"MES_SHOW" && wait_key && !pending
        && matches!(waiting, Some(FesDriverWait::Frame { after }) if *after == frame)
}

fn daydream_meswnd_main_wait(name: &[u8], driver: &FesScriptDriver) -> bool {
    name == b"meswnd" && driver.bounded_message_fes_source == Some(true)
        && driver.interpreter.current_block() == b"WAIT"
}

#[derive(Clone, Copy)]
enum NekoStartupScreen {
    Language,
    Title,
}

impl NekoStartupScreen {
    fn contract(self) -> (&'static [u8], &'static str, &'static [u8]) {
        match self {
            Self::Language => (b"langselect", "decide", b"WAIT_SELLANG"),
            Self::Title => (b"title", "btn1[0]", b"FES_WAIT"),
        }
    }
}

#[derive(Clone, Copy)]
struct StartupDriver<'a> {
    owner: u32,
    name: &'a [u8],
    block: &'a [u8],
    wait_key: bool,
    pending: bool,
    wait: Option<&'a FesDriverWait>,
}

fn neko_startup_fes_owner<'a>(
    screen: NekoStartupScreen,
    windows: impl IntoIterator<Item = StartupDriver<'a>>,
    frame: u64,
) -> Option<u32> {
    let (name, _, parent_block) = screen.contract();
    let expected = [(b"flow".as_slice(), b"WAIT_TITLE".as_slice()), (b"ttlflow", parent_block), (name, b"WAIT")];
    let mut owners = [None; 3];
    for window in windows {
        let name = window.name.strip_suffix(b".fes").unwrap_or(window.name);
        let index = expected.iter().position(|(fes, _)| name.eq_ignore_ascii_case(fes))?;
        if window.block != expected[index].1 || window.wait_key || window.pending
            || !matches!(window.wait, Some(FesDriverWait::Frame { after }) if *after == frame)
            || owners[index].replace(window.owner).is_some()
        {
            return None;
        }
    }
    match owners {
        [Some(flow), Some(parent), Some(owner)] if flow != parent && flow != owner && parent != owner => Some(owner),
        _ => None,
    }
}

fn neko_startup_language_matches(expected: &str, config: [u32; 3], locals: Option<[i32; 3]>) -> bool {
    let (index, display) = match expected {
        "jp" => (1, 0),
        "cn" => (2, 2),
        _ => return false,
    };
    config == [0, index, u32::MAX] && locals.is_none_or(|locals| locals == [1, display, 3])
}

fn rebellions_title_fes_ready(
    name: &[u8],
    block: &[u8],
    pending: bool,
    waiting: Option<&FesDriverWait>,
    frame: u64,
) -> bool {
    name.strip_suffix(b".fes").unwrap_or(name).eq_ignore_ascii_case(b"title")
        && block == b"WAIT00"
        && !pending
        && matches!(waiting, Some(FesDriverWait::Frame { after }) if *after == frame)
}

#[derive(Clone, Copy)]
struct EpisodeWindow<'a> {
    name: &'a [u8],
    block: &'a [u8],
    pending: bool,
    wait: Option<&'a FesDriverWait>,
    episode_a_ready: bool,
}

fn rebellions_episode_fes_ready<'a>(
    windows: impl IntoIterator<Item = EpisodeWindow<'a>>,
    frame: u64,
) -> bool {
    let mut flow_seen = false;
    let mut episode_seen = false;
    for window in windows {
        if window.name.eq_ignore_ascii_case(b"title") {
            return false;
        }
        let (seen, expected_block) = if window.name.eq_ignore_ascii_case(b"flow") {
            (&mut flow_seen, b"EPSEL_WAIT".as_slice())
        } else if window.name.eq_ignore_ascii_case(b"epselect") {
            if !window.episode_a_ready {
                return false;
            }
            (&mut episode_seen, b"WAIT00".as_slice())
        } else {
            continue;
        };
        if *seen || window.block != expected_block || window.pending
            || !matches!(window.wait, Some(FesDriverWait::Frame { after }) if *after == frame)
        {
            return false;
        }
        *seen = true;
    }
    flow_seen && episode_seen
}

fn settled_novel_draw_without_icons(
    draw_active: Option<i32>,
    input_icon: Option<bool>,
    page_anim: Option<bool>,
) -> bool {
    (draw_active, input_icon, page_anim) == (Some(1), Some(false), Some(false))
}

fn island_fullwnd_probe_ready(
    replay_wait: Option<(u32, i32, i32, bool)>,
    block: &[u8],
    wait_key: bool,
    pending: bool,
    waiting: Option<&FesDriverWait>,
    frame: u64,
) -> bool {
    matches!(replay_wait, Some((0x02, group, record, false)) if group >= 0 && record >= 0)
        && !pending
        && block == b"MES_SHOW"
        && wait_key
        && matches!(waiting, Some(FesDriverWait::Frame { after }) if *after == frame)
}

fn prefetched_novel_input_ready(
    block: &[u8],
    wait_key: bool,
    waiting: Option<&FesDriverWait>,
    frame: u64,
) -> bool {
    block == b"MES_SHOW"
        && wait_key
        && matches!(waiting, Some(FesDriverWait::Frame { after }) if *after == frame)
}

fn novel_wait_state(
    kind: u32,
    block: &[u8],
    wait_key: bool,
    wait_mode: Option<i32>,
    input_icon_visible: bool,
    hidden_draw: bool,
) -> Option<&'static str> {
    match (kind, block) {
        (0x02 | 0x03, b"MES_SHOW") => Some("page"),
        (0x02, b"MES_DRAW") if input_icon_visible => Some("page_prompt"),
        (0x03, b"MES_DRAW") if wait_key && wait_mode == Some(2) => Some("page_prompt"),
        (0x03, b"MES_DRAW") if wait_key && wait_mode == Some(0) && hidden_draw => Some("page_prompt"),
        _ => None,
    }
}

fn daydream_r96_probe_document(document: &CstDocument) -> bool {
    adjacent_cst_input_waits(document, (19, 83), Some((20, 96)))
        && daydream_empty_body_lengths(document, (20, 96), 90, 95).is_some()
}

fn daydream_empty_body_lengths(
    document: &CstDocument, (group, wait): (i32, i32), body: usize, empty: usize,
) -> Option<[usize; 2]> {
    let wait = usize::try_from(wait).ok()?;
    let range = document.inputs.get(usize::try_from(group).ok()?)?;
    if !range.contains(&wait) || !range.contains(&body) || !range.contains(&empty)
        || body >= empty || empty.checked_add(1) != Some(wait)
        || document.records.get(wait)?.kind != 0x02
    {
        return None;
    }
    let (body_row, empty_row) = (document.records.get(body)?, document.records.get(empty)?);
    (body_row.kind == 0x20 && !body_control_only(&body_row.text)
        && empty_row.kind == 0x20 && empty_row.text.is_empty())
        .then_some([body_row.text.len(), empty_row.text.len()])
}

fn island_com01_has_opmovie_exit(document: &CstDocument) -> bool {
    let Some(tail) = document.inputs.get(122) else { return false; };
    let expected: [(usize, &[u8]); 3] = [(823, b"rdraw 60"), (824, b"wait"), (827, b"next OPMOVIE")];
    island_cst_tail_matches(document, tail, &expected)
}

fn island_cst_tail_matches(
    document: &CstDocument, tail: &std::ops::Range<usize>, expected: &[(usize, &[u8])],
) -> bool {
    expected.iter().copied().all(|(offset, command)| {
        tail.contains(&offset) && document.records.get(offset).is_some_and(|row| {
            row.kind == 0x30 && row.text.trim_ascii() == command
        })
    }) && document.records.get(tail.clone()).is_some_and(|rows| {
        rows.iter().all(|row| !matches!(row.kind, 0x02 | 0x03))
    })
}

fn adjacent_cst_input_waits(document: &CstDocument, anchor: (i32, i32), next: Option<(i32, i32)>) -> bool {
    let is_input = |(group, record): (i32, i32)| {
        usize::try_from(group).ok().zip(usize::try_from(record).ok())
            .is_some_and(|(group, record)| {
                document.inputs.get(group).is_some_and(|range| range.contains(&record))
                    && document.records.get(record).is_some_and(|row| row.kind == 0x02)
            })
    };
    is_input(anchor) && next.is_none_or(|next| {
        anchor.0.checked_add(1) == Some(next.0) && next.1 > anchor.1 && is_input(next)
            && document.records.get((anchor.1 as usize + 1)..next.1 as usize)
                .is_some_and(|records| !records.iter().any(|row| matches!(row.kind, 0x02 | 0x03)))
    })
}

fn message_window_ready(name: &[u8], block: &[u8], wait_key: bool, pending: bool) -> bool {
    wait_key
        && !pending
        && match block {
            b"MES_SHOW" | b"WAIT_USER" => [b"meswnd".as_slice(), b"fullwnd", b"novelwnd"]
                .into_iter()
                .any(|window| name.eq_ignore_ascii_case(window)),
            _ => false,
        }
}

#[cfg(test)]
mod tests {
    use super::{
        CstStoryPosition, EpisodeWindow, FesDriverWait, StoryTraceProfile, island_fullwnd_probe_ready,
        message_window_ready, novel_wait_state, prefetched_novel_input_ready, rebellions_episode_fes_ready,
        rebellions_title_fes_ready, settled_novel_draw_without_icons, story_trace_profile,
    };

    #[test]
    fn kanmi_observes_confirmed_cst_without_releasing_input() {
        use super::verified_observation_profile;

        let loader = "2044cd9c096c509da8609c0c2174038e0e03255aeaffd57fea6e817406e802ab";
        let runtime = "c264b2465889c78723079a694d2654ef1091253db657a735e92f58b75031aa26";
        let entry = Some("kcs.int/main.kcs");
        assert_eq!(story_trace_profile(loader, entry), None);
        assert_eq!(story_trace_profile(runtime, entry), None);
        assert_eq!(verified_observation_profile(None), None);
        assert_eq!(verified_observation_profile(Some(runtime)), Some(StoryTraceProfile::KanmiObservation));

        let mut cst = CstStoryPosition::loaded(b"sc01.cst", 520);
        cst.observe_wait(618, 0x02, 0, 41);
        assert_eq!(cst.observation_only(619), (619, "active", "sc01", -1, -1));
        cst.observe_page(618, 0x02);
        assert_eq!(cst.observation_only(619), (619, "active", "sc01", 0, 41));
    }

    #[test]
    fn body_generation_requires_fresh_natural_completion_and_the_same_committed_epoch() {
        use super::{StoryBodyGeneration, current_committed_body};

        let valid = StoryBodyGeneration {
            source_epoch: 3, source_frame: 768, natural_complete: Some((3, 1132)),
            committed_epoch: Some(3), remaining: 0, remaining_segment: 0,
            pending_text: false, drawing: false, skip_pending: false, forced: false,
            projection: Some((true, 255)),
        };
        let matches = |generation| current_committed_body(generation, 761, 952, 1250);
        assert!(matches(valid));
        for wrong in [
            StoryBodyGeneration { source_frame: 760, ..valid },
            StoryBodyGeneration { natural_complete: Some((3, 1251)), ..valid },
            StoryBodyGeneration { natural_complete: Some((2, 1132)), ..valid },
            StoryBodyGeneration { natural_complete: None, ..valid },
            StoryBodyGeneration { committed_epoch: Some(2), ..valid },
            StoryBodyGeneration { remaining: 1, ..valid },
            StoryBodyGeneration { remaining_segment: 1, ..valid },
            StoryBodyGeneration { pending_text: true, ..valid },
            StoryBodyGeneration { drawing: true, ..valid },
            StoryBodyGeneration { skip_pending: true, ..valid },
            StoryBodyGeneration { forced: true, ..valid },
            StoryBodyGeneration { projection: Some((false, 255)), ..valid },
            StoryBodyGeneration { projection: Some((true, 0)), ..valid },
        ] {
            assert!(!matches(wrong), "{wrong:?}");
        }
    }

    #[test]
    fn observation_profiles_require_the_verified_pe_and_startup() {
        let island = "474cc11d875d7be9df9581f3a55d64698ccf4e760c44013df403cced85e505f1";
        let daydream = "1f3288d5fd5408bee4d395faab708aca78c9eeaa6c172dc06dcce09c844ba9d5";
        let nekopara = "c71c5b85e6eca660474f156d96b0021a53a5f05690dd077d7861d72ff5b11adc";
        let rebellions = "a099ac1cee7af60c8f32869485c9df4aab344b5eb91c1018ae8bb7df2c1f39f3";
        let eyecatch = "72c9c1e7335a92bc2f1c731275a8da34d638d1730b9d5003fb6a8e85fe5b15c8";
        let startup = Some("kcs.int/main.kcs");
        assert_eq!(story_trace_profile(island, None), Some(StoryTraceProfile::Island));
        assert_eq!(story_trace_profile(daydream, startup), Some(StoryTraceProfile::Daydream));
        assert_eq!(story_trace_profile(nekopara, startup), Some(StoryTraceProfile::Nekopara));
        assert_eq!(story_trace_profile(rebellions, startup), Some(StoryTraceProfile::Rebellions));
        assert_eq!(story_trace_profile(eyecatch, startup), Some(StoryTraceProfile::Eyecatch08Observation));
        assert_eq!(story_trace_profile(eyecatch, None), None);
        let legacy = "f6af7437475b8c066be973245aa79688bea510ca1192264add4f1914ebee4c0e";
        assert_eq!(story_trace_profile(legacy, startup), Some(StoryTraceProfile::Legacy07Observation));
        assert_eq!(story_trace_profile(legacy, None), None);
        assert_eq!(story_trace_profile(daydream, None), None);
        assert_eq!(story_trace_profile(nekopara, Some("main.kcs")), None);
        assert_eq!(story_trace_profile(rebellions, None), None);
        assert_eq!(story_trace_profile("unverified", startup), None);
    }

    #[test]
    fn island_meswnd_cst_edge_requires_adjacent_absolute_input_waits() {
        use cs2_script::{CstDocument, CstRecord};
        let mut document = CstDocument {
            inputs: vec![0..2, 2..6, 6..8],
            records: [0x20, 0x02, 0x30, 0x20, 0x02, 0x30, 0x20, 0x02].map(|kind| {
                CstRecord { kind, text: vec![] }
            }).to_vec(),
        };
        let edge = super::adjacent_cst_input_waits;
        assert!(edge(&document, (0, 1), Some((1, 4))));
        assert!(edge(&document, (1, 4), None));
        assert!(!edge(&document, (0, 1), Some((2, 7))));
        assert!(!edge(&document, (0, 1), Some((1, 2))));
        assert!(!edge(&document, (0, 1), Some((1, 7))));
        document.records[3].kind = 0x03;
        assert!(!edge(&document, (0, 1), Some((1, 4))));
    }

    #[test]
    fn neko_body_sources_include_nonempty_localized_fragments_in_empty_cst_slots() {
        use cs2_script::{CstDocument, CstRecord};
        let document = CstDocument {
            inputs: vec![0..3],
            records: vec![
                CstRecord { kind: 0x20, text: b"base".to_vec() },
                CstRecord { kind: 0x20, text: Vec::new() },
                CstRecord { kind: 0x02, text: Vec::new() },
            ],
        };
        let shell = super::BodyWaitShell {
            window: "meswnd", owner: 12, scene: "02", context: 1, loaded: 10,
            marked: 11, group: 0, record: 2,
        };
        let sources = [0, 1].map(|absolute_record| super::CstBodySource {
            context_handle: 1, load_id: 9, group: 0, absolute_record,
        });
        let matches = super::body_sources_match;
        assert!(matches(&document, shell, Some(&sources), Some("line\u{1f}extra")));
        assert!(!matches(&document, shell, Some(&sources[..1]), Some("line\u{1f}extra")));
        assert!(matches(&document, shell, Some(&sources[..1]), None));
        assert!(!matches(&document, shell, Some(&sources), None));
    }

    #[test]
    fn newer_cst_wait_supersedes_confirmed_page_without_confirming_the_candidate() {
        let mut story = CstStoryPosition::loaded(b"com01.cst", 127);
        story.observe_wait(267, 0x03, 8, 92);
        story.observe_page(267, 0x03);
        assert_eq!(story.island_replay_wait(276), Some((0x03, 8, 92, false)));
        story.observe_wait(277, 0x02, 9, 109);
        assert_eq!(story.island_replay_wait(277), None);
        assert_eq!(story.island_replay_wait(278), Some((0x02, 9, 109, true)));
        story.observe_wait(864, 0x02, 9, 109);
        story.observe_page(864, 0x02);
        assert_eq!(story.island_replay_wait(865), Some((0x02, 9, 109, false)));
        story.observe_wait(866, 0x02, 10, 111);
        assert_eq!(story.island_replay_wait(867), None);
    }

    #[test]
    fn prefetched_novel_input_does_not_make_a_transition_clickable() {
        let wait = FesDriverWait::Frame { after: 3876 };
        assert!(!prefetched_novel_input_ready(b"MES_SHOW", false, Some(&wait), 3876));
        assert!(!prefetched_novel_input_ready(b"MES_DRAW", true, Some(&wait), 3876));
        assert!(!prefetched_novel_input_ready(b"MES_SHOW", true, None, 3876));
        assert!(!prefetched_novel_input_ready(b"MES_SHOW", true, Some(&wait), 3877));
        assert!(prefetched_novel_input_ready(b"MES_SHOW", true, Some(&wait), 3876));
    }

    #[test]
    fn neko_startup_requires_exact_stable_drivers_and_explicit_language() {
        use super::{StartupDriver, neko_startup_fes_owner, neko_startup_language_matches};
        use super::NekoStartupScreen::{Language, Title};

        let frame = 1800;
        let wait = FesDriverWait::Frame { after: frame };
        let flow = StartupDriver { owner: 6, name: b"flow", block: b"WAIT_TITLE", wait_key: false, pending: false, wait: Some(&wait) };
        let parent = StartupDriver { owner: 8, name: b"ttlflow", block: b"WAIT_SELLANG", ..flow };
        let modal = StartupDriver { owner: 12, name: b"langselect.fes", block: b"WAIT", ..flow };
        let title = StartupDriver { name: b"title", ..modal };
        assert_eq!(neko_startup_fes_owner(Language, [flow, parent, modal], frame), Some(12));
        assert_eq!(neko_startup_fes_owner(Title, [flow, StartupDriver { block: b"FES_WAIT", ..parent }, title], frame), Some(12));
        assert_eq!(neko_startup_fes_owner(Title, [flow, parent, title], frame), None);
        assert_eq!(neko_startup_fes_owner(Language, [flow, parent, modal, title], frame), None);
        assert_eq!(neko_startup_fes_owner(Language, [flow, parent, modal, modal], frame), None);
        assert_eq!(neko_startup_fes_owner(Language, [flow, parent, modal], frame + 1), None);
        assert_eq!(neko_startup_fes_owner(Language, [flow, parent, StartupDriver { pending: true, ..modal }], frame), None);
        assert!(neko_startup_language_matches("jp", [0, 1, u32::MAX], Some([1, 0, 3])));
        assert!(neko_startup_language_matches("cn", [0, 2, u32::MAX], Some([1, 2, 3])));
        assert!(!neko_startup_language_matches("jp", [0, 1, u32::MAX], Some([1, 1, 3])));
        assert!(!neko_startup_language_matches("en", [0, 0, u32::MAX], Some([1, 1, 3])));
        assert!(!neko_startup_language_matches("cn", [0, 0, u32::MAX], Some([1, 2, 3])));
    }

    #[test]
    fn rebellions_title_requires_the_settled_title_driver() {
        let wait = FesDriverWait::Frame { after: 2100 };
        assert!(rebellions_title_fes_ready(b"title.fes", b"WAIT00", false, Some(&wait), 2100));
        assert!(!rebellions_title_fes_ready(b"epselect", b"WAIT00", false, Some(&wait), 2100));
        assert!(!rebellions_title_fes_ready(b"flow", b"FES_WAIT", false, Some(&wait), 2100));
        assert!(!rebellions_title_fes_ready(b"title", b"GO_STORY", false, Some(&wait), 2100));
        assert!(!rebellions_title_fes_ready(b"title", b"WAIT00", true, Some(&wait), 2100));
        assert!(!rebellions_title_fes_ready(b"title", b"WAIT00", false, None, 2100));
        assert!(!rebellions_title_fes_ready(b"title", b"WAIT00", false, Some(&wait), 2101));
    }

    #[test]
    fn island_fullwnd_probe_requires_confirmed_input_and_settled_nownd() {
        let frame = 4383;
        let wait = FesDriverWait::Frame { after: frame };
        let mut story = CstStoryPosition::loaded(b"com01.cst", 127);
        story.observe_wait(789, 0x02, 29, 209);
        let ready = |candidate, block, key, pending, wait, frame| {
            island_fullwnd_probe_ready(candidate, block, key, pending, wait, frame)
        };
        assert!(!ready(story.island_replay_wait(frame), b"MES_SHOW", true, false, Some(&wait), frame));
        story.observe_page(789, 0x02);
        let confirmed = story.island_replay_wait(frame);
        assert!(ready(confirmed, b"MES_SHOW", true, false, Some(&wait), frame));
        assert!(!ready(confirmed, b"MES_SHOW", false, false, Some(&wait), frame));
        assert!(!ready(confirmed, b"MES_SHOW", true, true, Some(&wait), frame));
        assert!(!ready(confirmed, b"MES_DRAW", true, false, Some(&wait), frame));
        assert!(!ready(confirmed, b"MES_SHOW", true, false, None, frame));
        assert!(!ready(confirmed, b"MES_SHOW", true, false, Some(&wait), frame + 1));
        assert!(!ready(Some((0x02, 29, 209, true)), b"MES_SHOW", true, false, Some(&wait), frame));
        assert!(!ready(Some((0x03, 29, 209, false)), b"MES_SHOW", true, false, Some(&wait), frame));
    }

    #[test]
    fn rebellions_episode_requires_one_settled_parent_and_enabled_episode_screen() {
        let frame = 2120;
        let wait = FesDriverWait::Frame { after: frame };
        let flow = EpisodeWindow {
            name: b"flow", block: b"EPSEL_WAIT", pending: false, wait: Some(&wait), episode_a_ready: false,
        };
        let episode = EpisodeWindow { name: b"EPSELECT", block: b"WAIT00", episode_a_ready: true, ..flow };
        let title = EpisodeWindow { name: b"title", ..episode };
        assert!(rebellions_episode_fes_ready([episode, flow], frame));
        assert!(!rebellions_episode_fes_ready([episode], frame));
        assert!(!rebellions_episode_fes_ready([flow, episode, title], frame));
        assert!(!rebellions_episode_fes_ready([flow, episode, episode], frame));
        assert!(!rebellions_episode_fes_ready([flow, flow, episode], frame));
        assert!(!rebellions_episode_fes_ready([EpisodeWindow { block: b"EPSELECT", ..flow }, episode], frame));
        assert!(!rebellions_episode_fes_ready([flow, EpisodeWindow { block: b"FADEOUT", ..episode }], frame));
        assert!(!rebellions_episode_fes_ready([flow, EpisodeWindow { episode_a_ready: false, ..episode }], frame));
        assert!(!rebellions_episode_fes_ready([EpisodeWindow { pending: true, ..flow }, episode], frame));
        assert!(!rebellions_episode_fes_ready([flow, EpisodeWindow { pending: true, ..episode }], frame));
        assert!(!rebellions_episode_fes_ready([flow, episode], frame + 1));
    }

    #[test]
    fn observation_reports_positions_without_marking_a_page_ready() {
        let mut story = CstStoryPosition::loaded(b"prologue.cst", 11);
        assert_eq!(story.observation_only(12), (12, "active", "prologue", -1, -1));
        story.observe_wait(12, 0x02, 1, 8);
        assert_eq!(story.observation_only(13), (13, "active", "prologue", -1, -1));
        story.observe_page(12, 0x02);
        assert_eq!(story.observation_only(12), (12, "active", "prologue", -1, -1));
        assert_eq!(story.observation_only(13), (13, "active", "prologue", 1, 8));
    }

    #[test]
    fn story_position_exposes_only_asset_tokens_and_starts_without_a_wait() {
        let story = CstStoryPosition::loaded(b"scene\\BAD_02.cst", 11);
        assert_eq!(story.scene, "BAD_02");
        assert_eq!(story.loaded, 11);
        assert_eq!(story.page, None);
        assert!(
            CstStoryPosition::loaded(b"some scene.cst", 2)
                .scene
                .is_empty()
        );
        assert!(CstStoryPosition::loaded(&[0xff], 2).scene.is_empty());
    }

    #[test]
    fn story_position_requires_the_cst_wait_at_the_current_page_boundary() {
        let mut story = CstStoryPosition::loaded(b"BAD_02", 11);
        story.observe_wait(12, 0x02, 36, 183);
        story.observe_page(13, 0x02);
        assert_eq!(story.page, None);
        story.observe_wait(14, 0x02, 37, 207);
        assert_eq!(story.page, None);
        story.observe_page(14, 0x02);
        assert_eq!(story.page, Some((14, 0x02, 37, 207)));
        story.observe_wait(15, 0x03, 0, 54);
        story.observe_page(15, 0x02);
        assert_eq!(story.page, None);
        story.observe_wait(16, 0x03, 1, 62);
        story.observe_page(16, 0x03);
        assert_eq!(story.page, Some((16, 0x03, 1, 62)));
    }

    #[test]
    fn novel_page_boundary_requires_page_wait_lock_in_draw_block() {
        let mut story = CstStoryPosition::loaded(b"com01.cst", 127);
        story.observe_wait(271, 0x03, 8, 92);
        story.observe_page(271, 0x03);
        let (_, kind, _, _) = story.page.unwrap();

        assert_eq!(novel_wait_state(kind, b"MES_SHOW", false, None, false, false), Some("page"));
        assert_eq!(novel_wait_state(kind, b"MES_DRAW", true, Some(2), false, false), Some("page_prompt"));
        assert_eq!(novel_wait_state(kind, b"MES_DRAW", false, Some(2), true, false), None);
        assert_eq!(novel_wait_state(kind, b"MES_DRAW", true, Some(1), true, false), None);
        assert_eq!(novel_wait_state(kind, b"MES_DRAW", true, None, true, false), None);
        assert_eq!(novel_wait_state(kind, b"WAIT_USER", true, Some(2), true, false), None);
        assert_eq!(novel_wait_state(0x02, b"MES_SHOW", false, None, false, false), Some("page"));
        assert_eq!(novel_wait_state(0x02, b"MES_DRAW", false, Some(1), true, false), Some("page_prompt"));
        assert_eq!(novel_wait_state(0x02, b"MES_DRAW", true, Some(2), false, false), None);
        assert_eq!(novel_wait_state(0x30, b"MES_SHOW", true, Some(2), true, false), None);
    }

    #[test]
    fn novel_page_without_icons_requires_active_draw_and_key_wait() {
        let hidden_draw = settled_novel_draw_without_icons(Some(1), Some(false), Some(false));
        assert!(hidden_draw);
        assert!(!settled_novel_draw_without_icons(Some(0), Some(false), Some(false)));
        assert!(!settled_novel_draw_without_icons(None, Some(false), Some(false)));
        assert!(!settled_novel_draw_without_icons(Some(1), None, Some(false)));
        assert!(!settled_novel_draw_without_icons(Some(1), Some(false), None));
        assert!(!settled_novel_draw_without_icons(Some(1), Some(true), Some(false)));
        assert!(!settled_novel_draw_without_icons(Some(1), Some(false), Some(true)));
        assert_eq!(novel_wait_state(0x03, b"MES_DRAW", true, Some(0), false, hidden_draw), Some("page_prompt"));
        assert_eq!(novel_wait_state(0x03, b"MES_DRAW", false, Some(0), false, hidden_draw), None);
        assert_eq!(novel_wait_state(0x03, b"MES_DRAW", true, Some(0), false, false), None);
        assert_eq!(novel_wait_state(0x03, b"MES_DRAW", true, None, false, hidden_draw), None);
        assert_eq!(novel_wait_state(0x03, b"MES_ISDRAW", true, Some(0), false, hidden_draw), None);
        assert_eq!(novel_wait_state(0x02, b"MES_DRAW", true, Some(0), false, hidden_draw), None);
    }

    #[test]
    fn message_window_requires_a_settled_key_wait() {
        assert!(message_window_ready(b"novelwnd", b"MES_SHOW", true, false));
        assert!(message_window_ready(b"meswnd", b"WAIT_USER", true, false));
        assert!(!message_window_ready(
            b"novelwnd",
            b"MES_SHOW",
            false,
            false
        ));
        assert!(!message_window_ready(b"novelwnd", b"MES_SHOW", true, true));
        assert!(!message_window_ready(
            b"novelwnd",
            b"MES_ISDRAW",
            true,
            false
        ));
        assert!(!message_window_ready(b"select", b"WAIT_USER", true, false));
    }

    #[test]
    fn ordinary_body_shell_requires_a_confirmed_input_record_from_an_earlier_frame() {
        let mut cst = CstStoryPosition::loaded(b"prologue.cst", 644);
        cst.observe_wait(1105, 0x02, 0, 18);
        assert_eq!(cst.settled_body_record(1106), None);
        cst.observe_page(1105, 0x02);
        assert_eq!(cst.settled_body_record(1105), None);
        assert_eq!(cst.settled_body_record(1150), Some((1105, 0, 18)));
        cst.observe_wait(1151, 0x02, 1, 22);
        assert_eq!(cst.settled_body_record(1152), None);
        cst.observe_wait(1153, 0x03, 1, 22);
        cst.observe_page(1153, 0x03);
        assert_eq!(cst.settled_body_record(1154), None);
    }

    #[test]
    fn ordinary_body_shell_rejects_early_wait_and_pending_handler() {
        let frame = FesDriverWait::Frame { after: 1500 };
        assert!(super::settled_body_window(b"MES_SHOW", true, false, Some(&frame), 1500));
        assert!(!super::settled_body_window(b"MES_SHOW", false, false, Some(&frame), 1500));
        assert!(!super::settled_body_window(b"MES_SHOW", true, true, Some(&frame), 1500));
        assert!(!super::settled_body_window(b"MES_SHOW", true, false, Some(&frame), 1501));
        assert!(!super::settled_body_window(b"WAIT_USER", true, false, Some(&frame), 1500));
        assert!(!super::settled_body_window(b"MES_SHOW", true, false, None, 1500));
    }
}


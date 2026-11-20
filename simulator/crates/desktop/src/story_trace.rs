use super::HeadlessPlayer;
use super::cst_context_native::{CstBodySource, body_control_only};
use super::fes_driver::FesDriverWait;
use super::fes_task_platform::Layer;
use super::fes_text::StoryBodyGeneration;
use super::{fes_driver, particle_plugin_digest};
use cs2_renderer::RgbaFrame;
use cs2_script::CstDocument;

#[path = "story_trace_08.rs"]
mod story_trace_08;
#[path = "legacy_story_wait.rs"]
mod legacy_story_wait;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StoryTraceProfile {
    Island,
    Daydream,
    Nekopara,
    Rebellions,
    KanmiObservation,
    Eyecatch08Observation,
    Legacy07Observation,
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
    match (pe_sha256, startup) {
        (
            "474cc11d875d7be9df9581f3a55d64698ccf4e760c44013df403cced85e505f1"
            | "7bd9f6dcc7d62868d4ad9cc68d5580452e71bd907540ec4f993d960d051d2c45",
            _,
        ) => Some(StoryTraceProfile::Island),
        (
            "1f3288d5fd5408bee4d395faab708aca78c9eeaa6c172dc06dcce09c844ba9d5",
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
        if profile == StoryTraceProfile::Legacy07Observation {
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
        &self, anchor: (i32, i32), next: Option<(i32, i32)>, owner: u32,
    ) -> Option<(u32, u64, u32)> {
        let ((frame, state, scene, group, record), window) = self.host_replay_story_position();
        if self.host_story_pe_sha256() != Some("474cc11d875d7be9df9581f3a55d64698ccf4e760c44013df403cced85e505f1")
            || self.default_entry() != Some("kcs.int/main.kcs")
            || (state, scene, group, record) != ("page", "com01", anchor.0, anchor.1)
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
        Some((handle, context.story.loaded, 8))
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

    fn host_body_wait_shell(&self, frame: u64) -> Option<BodyWaitShell<'_>> {
        let (body, flow_wait, permits_idle_touch) = match self.host_story_trace_profile()? {
            StoryTraceProfile::Nekopara => ("meswnd", b"WAIT_GAME".as_slice(), true),
            StoryTraceProfile::Daydream => ("cinema", b"GAME_WAIT".as_slice(), false),
            _ => return None,
        };
        if !self.data_services.story_trace || self.frame_in_progress() || !self.state.input.focused
            || self.data_services.host_pause.paused || self.data_services.host_quit_requested
            || !self.movie_playbacks.is_empty() || !self.pending_messages.is_empty()
            || !self.data_services.pending_host_input.is_empty()
            || self.message_wait_address != Some(0xd6d)
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
        let mut body_owner = None;
        let mut settled_flow = false;
        for (owner, name, runtime, wait_key) in self.data_services.tasks.fes.story_runtimes() {
            let name = name.strip_suffix(b".fes").unwrap_or(name);
            let driver = runtime.driver.as_ref()?;
            let block = driver.interpreter.current_block();
            if name.eq_ignore_ascii_case(body.as_bytes()) {
                if !settled_body_window(block, wait_key, driver.has_pending_handler(), driver.waiting(), frame)
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
                } else if !(permits_idle_touch && name.eq_ignore_ascii_case(b"touch") && block == b"WAIT") {
                    return None;
                }
            }
        }
        let owner = body_owner?;
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
            .and_then(|context| context.document.as_ref())
            .is_some_and(|document| body_sources_match(document, shell, sources));
        let committed = sources_match && current_committed_body(generation, shell.loaded, shell.marked, frame);
        let composited = committed && self.host_body_list_submitted(shell);
        (sources, sources_match, composited)
    }

    pub fn host_replay_body_route_anchor(
        &self, scene: &str, anchor: (i32, i32), next: Option<(i32, i32)>, owner: u32,
    ) -> Option<(u32, u64, u32, u64, u64)> {
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
        let load = sources?.first()?.load_id;
        let generation = self.data_services.fes_text.story_body_generation(owner)?;
        Some((shell.context, load, owner, generation.source_epoch, generation.source_frame))
    }

    fn host_body_list_submitted(&self, shell: BodyWaitShell<'_>) -> bool {
        let platform = &self.data_services.tasks.fes.platform;
        let mut lists = platform.lists.values().filter_map(|list| {
            let layer = list.layers.get(&(shell.owner, "\0text:737472[0]".to_owned()))?;
            Some((list, layer))
        });
        let Some((list, layer)) = lists.next() else { return false; };
        if lists.next().is_some() || !list.enabled || !list.submit {
            return false;
        }
        let Some(output) = self.image_handles.get(&list.output) else { return false; };
        submitted_body_pixels(layer, output) && (shell.window != "meswnd" || list.layers
            .get(&(shell.owner, "keywait".to_owned())).is_some_and(|prompt| submitted_body_pixels(prompt, output)))
    }

    fn host_observed_body_owner(&self) -> Option<u32> {
        let name = match self.host_story_trace_profile()? {
            StoryTraceProfile::Nekopara => b"meswnd".as_slice(),
            StoryTraceProfile::Daydream => b"cinema".as_slice(),
            _ => return None,
        };
        let mut owners = self.data_services.tasks.fes.story_runtimes().filter_map(|(owner, candidate, _, _)| {
            candidate.strip_suffix(b".fes").unwrap_or(candidate).eq_ignore_ascii_case(name).then_some(owner)
        });
        let owner = owners.next()?;
        owners.next().is_none().then_some(owner)
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
        let body_shell = self.host_body_wait_shell(frame);
        let body_wait_shell = body_shell.map(|shell| {
            (shell.window, shell.owner, shell.scene, shell.loaded, shell.marked, shell.group, shell.record)
        });
        let body_page = body_shell.map(|shell| {
            let (sources, matches, composited) = self.host_current_body(shell, frame);
            (shell.context, sources, matches, composited)
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
        let body_generation = self.host_observed_body_owner().map(|owner| {
            let generation = self.data_services.fes_text.story_body_generation(owner);
            let current_committed = body_shell.zip(body_page).zip(generation).is_some_and(|((shell, body), generation)| {
                owner == shell.owner && body.2 && current_committed_body(generation, shell.loaded, shell.marked, frame)
            });
            (owner, generation, current_committed)
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
        format!(
            "trace_enabled={} profile={:?} pe_sha256={:?} startup={:?} \
             frame={frame} state={state} scene={scene} group={group} record={record} \
             focused={} paused={} message_wait={:?} vm_waiting_for_message={} pending_messages={} movies={} \
             contexts(id,scene,loaded,candidate,page)={contexts:?} \
             windows(owner,name,wait_key,driver(block,pending,wait),novel_ready,novel_prompt(mode,input_icon,page_anim,draw_active))={windows:?} \
             body_wait_shell(window,owner,scene,loaded,marked,group,record)={body_wait_shell:?} \
             body_generation(owner,generation,current_committed)={body_generation:?} \
             body_page(context,sources,exact_match,composited)={body_page:?} \
             kanmi_verified_pe={:?} kanmi_meswnd(owner,local100,local52,local101,keywait,generation)={kanmi_meswnd:?} \
             neko_startup(config_ui_main_sub,locals55_56_57)={neko_startup:?} \
             eyecatch_08(window,owner)={eyecatch_08:?} legacy_07={legacy_07:?}",
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

fn body_sources_match(document: &CstDocument, shell: BodyWaitShell<'_>, sources: Option<&[CstBodySource]>) -> bool {
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
    prefix.iter().enumerate().skip(start)
        .filter(|(_, record)| record.kind == 0x20 && !body_control_only(&record.text))
        .map(|(index, _)| range.start + index)
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
    let Some((complete_epoch, complete_frame)) = generation.natural_complete else { return false; };
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
        && document.inputs[20].contains(&90) && document.inputs[20].contains(&95)
        && document.records.get(90).is_some_and(|row| row.kind == 0x20 && !body_control_only(&row.text))
        && document.records.get(95).is_some_and(|row| row.kind == 0x20 && row.text.is_empty())
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


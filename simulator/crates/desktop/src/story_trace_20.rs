use cs2_runtime::message_object::ButtonSource;
use cs2_script::{FesProgram, KcsProgram};

use crate::cst_context_native::{CstSourceProbe, body_control_only, rebellions_gbk_member_digest};
use crate::fes_driver::FesDriverWait;
use crate::fes_tasks::REBELLIONS_NOVEL_FES;
use crate::particle_plugin_digest::sha256;
use crate::{HeadlessPlayer, ScriptTextEncoding};

use super::{BodyWaitShell, StoryTraceProfile, current_committed_body, submitted_body_pixels};

const SCENE: &str = "A_01_01";
const SECOND_SCENE: &str = "A_01_02";
const THIRD_SCENE: &str = "A_01_03";
const A_LINE: &[&str] = &[
    SCENE, SECOND_SCENE, THIRD_SCENE, "A_01_04", "A_01_05", "A_01_06", "A_01_07",
    "A_01_08", "A_01_09", "A_01_10", "A_01_11", "A_01_12", "A_01_13", "A_01_14",
    "A_01_15", "A_01_16", "A_01_17", "A_01_18", "A_01_19", "A_01_20", "A_02_01",
    "A_02_02", "A_02_03", "A_02_04", "A_02_05", "A_02_06", "A_02_07", "A_02_08",
    "A_02_09", "A_02_10", "A_02_11", "A_02_12", "A_03_01", "A_03_02", "A_03_03",
];
const OPENING_WAITS: [i32; 123] = [
    13, 15, 26, 30, 32, 34, 38, 40, 42, 44, 48, 58,
    61, 150, 152, 154, 158, 160, 162, 166, 168, 170, 172, 174,
    178, 180, 189, 211, 213, 215, 220, 222, 225, 228, 232, 234, 236,
    238, 243, 245, 247, 249, 266, 268, 270, 278, 282, 301, 308,
    310, 312, 316, 318, 320, 322, 326, 337, 353, 355, 363, 365,
    367, 373, 375, 377, 379, 385, 395, 397, 401, 403, 405, 413,
    423, 425, 429, 441, 443, 445, 447, 449, 464, 466, 469, 471,
    475, 478, 487, 491, 493, 495, 497, 510, 512, 514, 517, 523,
    525, 527, 529, 531, 537, 540, 542, 548, 550, 552, 554, 556,
    567, 569, 571, 573, 575, 577, 579, 583, 585, 587, 593, 606,
    608, 610,
];
const SECOND_WAITS: [i32; 89] = [
    85, 89, 93, 104, 107, 110, 112, 119, 122, 124, 127, 129, 134, 138, 142, 147,
    149, 151, 153, 159, 163, 165, 175, 177, 190, 194, 198, 207, 209, 211, 306, 308,
    311, 323, 325, 327, 329, 335, 337, 339, 345, 347, 349, 351, 353, 359, 364, 366,
    368, 370, 372, 374, 382, 385, 387, 393, 395, 401, 407, 409, 421, 431, 447, 463,
    465, 467, 469, 473, 484, 486, 488, 494, 500, 508, 528, 537, 539, 543, 550, 552,
    554, 560, 562, 564, 571, 573, 575, 577, 583,
];
const THIRD_WAITS: [i32; 3] = [88, 90, 97];
const WINDOW: &str = "meswnd01";
const ENTER: u8 = 13;
const LOGICAL_ENTER: usize = 2;
const CONFIRM_FES: &str = "b65e053d7572cb4ed0c784828aee9ccf2f095891f7eb4181bec52e538d663628";
const SAVELOAD_FES: (&str, usize, &str) = ("saveload.fes", 415,
    "934d91395958615f3c0e9b8ab88fd8cd31414320925636d38bffa41a859287a0");
const SAVELOAD_MAIN_FES: (&str, usize, &str) = ("saveload_main.fes", 5217,
    "b95227e30457f074f3b17cef57b7e416ce55a511579fb053afe490dac3078667");
const SSCRIPT_CODE: &str = "508090f8d1f1716e69c42101e298802f5b0d706474161b4790a1a084883a2ccc";
const SSCRIPT_DATA: &str = "8440cbf74c6949b050ed0ad39baadb4f43238192d4d50fd924ad09d1327a1599";

pub(super) type Body = (&'static str, (i32, i32), (u32, u32, u32), (u32, u64, u64, u64, u64));

impl HeadlessPlayer {
    pub(crate) fn verified_rebellions_sscript(program: &KcsProgram) -> bool {
        exact_sscript(program)
    }

    pub fn host_rebellions_save_retained(&self, retained: Body) -> bool {
        let frame = self.state.clock.frame;
        let (_, _, (owner, _, _), _) = retained;
        quiet_root(self, frame).is_some()
            && self.rebellions_retained_source(retained, frame)
            && message_owner(self, frame, b"MES_SETSTATE")
                .or_else(|| message_owner(self, frame, b"WAIT")) == Some(owner)
    }

    fn rebellions_retained_source(&self, retained: Body, frame: u64) -> bool {
        let (scene, (group, record), (owner, _, receiver), (handle, load, epoch, source_frame, _)) = retained;
        let Some(context) = self.cst_contexts.get(&handle) else { return false; };
        let story = &context.story;
        let services = &self.data_services;
        story.is_scene(scene) && story.settled_body_record(frame).is_some_and(|(_, g, r)| (g, r) == (group, record))
            && live_child(self, owner, receiver, 34)
            && services.fes_text.story_body_generation(owner).is_some_and(|generation|
                (generation.source_epoch, generation.source_frame) == (epoch, source_frame))
            && services.fes_text.story_body_sources(owner, epoch, &services.cst_text_origins)
                .is_some_and(|sources| matches!(sources, [source]
                    if (source.context_handle, source.load_id, source.group, source.absolute_record)
                        == (handle, load, group, record as usize - 1)))
    }

    pub fn host_rebellions_ordinary_panel(&self, retained: Option<Body>, occupied: bool) -> Option<u32> {
        let frame = self.state.clock.frame;
        quiet_root(self, frame)?;
        let (wrapper, _) = self.host_neko_menu_driver(b"saveload", b"WAIT")?;
        let (panel, locals) = self.host_neko_menu_driver(b"saveload_main", b"WAIT")?;
        if !live_child(self, panel, wrapper, 34) || locals.get(102) != Some(&8)
            || !match_fes_member(self, wrapper, SAVELOAD_FES) || !match_fes_member(self, panel, SAVELOAD_MAIN_FES)
        {
            return None;
        }
        let snapshot = self.shared_numeric_state_snapshot().ok()?;
        let vars = if snapshot.active_identity == 0 { &snapshot.active_values } else { snapshot.inactive_values.get(&0)? };
        if vars.get(804) != Some(&0) || (locals.get(53)? & (1 << 2) != 0) != occupied {
            return None;
        }
        if let Some(anchor) = retained {
            let flow = self.host_neko_menu_driver(b"flow", b"GAME_WAIT")?.0;
            let (_, _, (_, _, receiver), _) = anchor;
            if !self.rebellions_retained_source(anchor, frame)
                || !live_child(self, wrapper, receiver, 34) || !live_child(self, receiver, flow, 0)
                || vars.get(800) != Some(&0) || vars.get(801) != Some(&1)
            {
                return None;
            }
        } else {
            let flow = self.host_neko_menu_driver(b"flow", b"LOAD_WAIT")?.0;
            if !occupied || !self.cst_contexts.is_empty() || !live_child(self, wrapper, flow, 34)
                || vars.get(800) != Some(&1) || vars.get(801) != Some(&0)
            {
                return None;
            }
        }
        Some(panel)
    }

    pub fn host_rebellions_ordinary_control(&mut self, owner: u32, name: &[u8], bank: i32, point: Option<[i32; 2]>)
        -> anyhow::Result<Option<([i32; 2], u32)>>
    {
        let binding = self.with_host(|host| Ok(crate::fes_task_platform::navigation_binding(host, owner, name, bank)))?;
        let Some((target, _, _)) = binding else { return Ok(None); };
        let Ok(region) = u32::try_from(target.value) else { return Ok(None); };
        let Some(entry) = self.input_task.regions.iter().find(|entry| entry.id() == region) else { return Ok(None); };
        let [left, top, right, bottom] = entry.bounds();
        if left >= right || top >= bottom || !self.host_rebellions_button_bound(owner, region, name, bank) {
            return Ok(None);
        }
        let point = point.unwrap_or_else(|| [left.midpoint(right), top.midpoint(bottom)]);
        Ok((self.masked_input_region_at(point)?.is_some_and(|hit| hit[0] == region && hit[8] != 0 && hit[9] != 0)
            && self.mouse_press_candidate_regions(point)?.contains(&region)).then_some((point, region)))
    }

    pub(crate) fn verified_rebellions_novel_fes(
        resources: &cs2_runtime::ResourceIndex, program: &FesProgram,
    ) -> Option<&'static str> {
        REBELLIONS_NOVEL_FES.into_iter().find_map(|(member, size, digest)| {
            let names = resources.category_names(b"fes", member).ok()?;
            let candidate = names.iter().find_map(|name| resources.find(name))?;
            let bytes = resources.read_candidate_bounded(candidate, size).ok()??;
            (sha256(&bytes) == digest && FesProgram::parse(&bytes).ok()? == *program).then_some(member)
        })
    }

    pub fn host_rebellions_scene_waits(scene: &str) -> &'static [i32] {
        match scene {
            SCENE => &OPENING_WAITS,
            SECOND_SCENE => &SECOND_WAITS,
            THIRD_SCENE => &THIRD_WAITS,
            _ => &[],
        }
    }

    /// Settled entry screen owner. Recheck immediately before dispatch.
    pub fn host_rebellions_entry_owner(&mut self, screen: &str) -> Option<u32> {
        let frame = self.state.clock.frame;
        quiet_root(self, frame)?;
        match screen {
            "title" | "epselect" => {
                let (ready, flow_block) = if screen == "title" {
                    (self.rebellions_title_ready(frame), b"FES_WAIT".as_slice())
                } else {
                    (self.rebellions_episode_ready(frame), b"EPSEL_WAIT".as_slice())
                };
                if !ready || !self.cst_contexts.is_empty() {
                    return None;
                }
                let flow = window(self, b"flow", flow_block, false, 2, frame)?;
                let owner = window(self, screen.as_bytes(), b"WAIT00", false, 2, frame)?;
                (live_child(self, flow, 0, 34) && live_child(self, owner, flow, 34)).then_some(owner)
            }
            "confirm2" => {
                let flow = window(self, b"flow", b"GAME_WAIT", false, 3, frame)?;
                let owner = window(self, b"confirm2", b"WAIT", false, 3, frame)?;
                let body = window(self, b"meswnd01", b"MES_SETSTATE", false, 3, frame)
                    .or_else(|| window(self, b"meswnd01", b"MES_SHOW", false, 3, frame))?;
                let receiver = parent_sscript(self, flow, body)?;
                let mut contexts = self.cst_contexts.values().filter(|context| context.document.is_some());
                if !live_child(self, owner, receiver, 34) || contexts.next()?.story.scene != "0000"
                    || contexts.next().is_some()
                {
                    return None;
                }
                let runtime = self.data_services.tasks.fes.runtime(owner)?;
                let driver = runtime.driver.as_ref()?;
                let resources = self.resources();
                let names = resources.category_names(b"fes", "confirm2").ok()?;
                let candidate = names.iter().find_map(|name| resources.find(name))?;
                let bytes = resources.read_candidate(candidate).ok()?;
                if sha256(&bytes) != CONFIRM_FES || FesProgram::parse(&bytes).ok()? != driver.interpreter.program {
                    return None;
                }
                self.with_host(|host| {
                    Ok(host.shared_values_for(receiver).is_some_and(|vars|
                        vars.get(520) == Some(&10) && vars.get(798) == Some(&1)).then_some(owner))
                }).ok()?
            }
            _ => None,
        }
    }

    pub fn host_rebellions_button_bound(&mut self, owner: u32, region: u32, name: &[u8], bank: i32) -> bool {
        let live = live_child(self, region, owner, 6);
        let registered = self.input_task.regions.iter().filter(|entry| entry.id() == region);
        let enabled = registered.clone().count() == 1 && registered.clone().any(|entry|
            entry.words[8] != 0 && entry.words[9] != 0 && entry.words[2] & ButtonSource::Mouse.mask() != 0);
        let binding = self.with_host(|host| {
            // A FES lower button may have only the registered region until its
            // first command. Resolve the receiver as input dispatch does.
            let receiver = host.effective_input_region_receiver(region);
            let target = crate::fes_task_platform::button_target_lookup(host, region);
            let object = crate::fes_task_platform::navigation_binding(host, owner, name, bank);
            Ok((receiver, target, object))
        });
        let Ok((receiver, target, object)) = binding else { return false; };
        let exact = target.as_ref().is_some_and(|(actual_owner, target)|
            *actual_owner == owner && target.as_ref().is_some_and(|target|
                target.name == name && target.bank == bank && target.value == region as i32));
        let runtime = self.data_services.tasks.fes.runtime(owner);
        let button = object.and_then(|(target, key, _)| {
            (target.value == region as i32).then_some(key)
        }).and_then(|key| runtime?.media.get(&key));
        let bound_button = button.is_some_and(|layer| layer.kind == "BUTTON" && layer.enabled);
        let display = if name == b"btn1" && matches!(bank, 0 | 1) {
            // title.fes draws the New Game label on a separate IMAGE bank.
            runtime.and_then(|runtime| runtime.media.get(&format!("btn_1i[{bank}]")))
                .filter(|layer| layer.kind == "IMAGE")
        } else if name == b"data_btn" {
            runtime.and_then(|runtime| runtime.media.get(&format!("data[{bank}]")))
                .filter(|layer| layer.kind == "SAVEPANEL" && layer.image.is_some())
        } else {
            button
        };
        let visible = display.is_some_and(|layer| layer.visible && layer.opacity != 0);
        if !live || !enabled || receiver != Some(owner) || !exact || !bound_button || !visible {
            eprintln!("Rebellions button region={region} owner={owner} live={live} enabled={enabled} receiver={receiver:?} target={target:?} bound_button={bound_button} display_visible={visible}");
            return false;
        }
        true
    }

    /// Position, live text/button/receiver IDs, and CST generation evidence.
    pub fn host_rebellions_body(&self) -> Option<Body> {
        let frame = self.state.clock.frame;
        let (shell, kind) = if let Some(shell) = body_shell(self, frame) {
            (shell, 0x02)
        } else {
            novel_shell(self, frame)?
        };
        self.rebellions_body(shell, frame, kind, &[shell.record as usize - 1])
    }

    pub fn host_rebellions_planned_body(
        &self, position: (&str, i32, i32), kind: u32, novel: bool, records: &[usize],
    ) -> Option<Body> {
        let (scene, group, record) = position;
        let scene = A_LINE.iter().copied().find(|name| *name == scene)?;
        let frame = self.state.clock.frame;
        quiet_root(self, frame)?;
        let mut contexts = self.cst_contexts.iter();
        let (&handle, context) = contexts.next()?;
        let story = &context.story;
        let (marked, actual_kind, actual_group, actual_record) = story.page?;
        let document = context.document.as_ref()?;
        let record_index = usize::try_from(record).ok()?;
        let range = document.inputs.get(usize::try_from(group).ok()?)?;
        let prefix = document.records.get(range.start..record_index)?;
        let first = prefix.iter().rposition(|entry| matches!(entry.kind, 0x02 | 0x03))
            .map_or(0, |index| index + 1);
        let expected_records = prefix.iter().enumerate().skip(first).filter_map(|(index, entry)|
            (entry.kind == 0x20 && !body_control_only(&entry.text))
                .then_some(range.start + index));
        if contexts.next().is_some() || !story.is_scene(scene) || story.candidate.is_some()
            || story.loaded > marked || marked >= frame || (actual_kind, actual_group, actual_record) != (kind, group, record)
            || !range.contains(&record_index) || u32::from(document.records.get(record_index)?.kind) != kind
            || records.is_empty() || !expected_records.eq(records.iter().copied())
            || context.encoding != ScriptTextEncoding::Gbk || context.localization.is_some()
        {
            return None;
        }
        let (owner, member) = if novel {
            novel_owner(self, frame, kind)?
        } else {
            if kind != 0x02 { return None; }
            (message_owner(self, frame, b"MES_SETSTATE")?, WINDOW)
        };
        self.rebellions_body(BodyWaitShell { window: member, owner, scene, context: handle,
            loaded: story.loaded, marked, group, record }, frame, kind, records)
    }

    pub fn host_rebellions_planned_scene(scene: &str, digest: &str) -> bool {
        A_LINE.contains(&scene) && rebellions_gbk_member_digest(&format!("{scene}.cst")) == Some(digest)
    }

    pub fn host_rebellions_departure(&self, previous: &str, advanced: u64) -> Option<String> {
        let frame = self.state.clock.frame;
        quiet_root(self, frame)?;
        let mut contexts = self.cst_contexts.values();
        let story = &contexts.next()?.story;
        let (marked, kind, group, record) = story.page?;
        (contexts.next().is_none() && !story.is_scene(previous) && story.loaded > advanced
            && story.candidate.is_none() && marked < frame).then(|| format!(
                "scene={} load_frame={} group={group} record={record} kind={kind}", story.scene, story.loaded))
    }

    fn rebellions_body(&self, shell: BodyWaitShell<'_>, frame: u64, kind: u32, expected: &[usize]) -> Option<Body> {
        let services = &self.data_services;
        let generation = services.fes_text.story_body_generation(shell.owner)?;
        let novel = shell.window != WINDOW;
        let (sources, composited) = if novel {
            let sources = services.fes_text.story_body_sources(
                shell.owner, generation.source_epoch, &services.cst_text_origins,
            );
            let prompt = if kind == 0x03 { "keywait" } else { "keywait2" };
            let composed = current_committed_body(generation, shell.loaded, shell.marked, frame)
                && services.fes_text.story_novel_segment_ready(shell.owner)
                && services.fes_text.rebellions_novel_composed(shell.owner, generation.source_epoch, frame)
                && prompt_submitted(self, shell.owner, prompt);
            (sources, composed)
        } else {
            let (sources, _, composited) = self.host_current_body(shell, frame);
            (sources, composited)
        };
        let sources = sources?;
        let source = sources.first()?;
        if !composited || source.load_id == 0 || !sources.iter().map(|source| source.absolute_record).eq(expected.iter().copied())
            || sources.iter().any(|entry| (entry.context_handle, entry.load_id, entry.group)
                != (shell.context, source.load_id, shell.group))
            || !self.events().iter().rev()
                .take_while(|event| event.frame >= shell.marked)
                .any(|event| event.kind == "native-call" && wait_packet(&event.detail, shell.owner, kind))
        {
            return None;
        }
        let scene = A_LINE.iter().copied().find(|name| *name == shell.scene)?;
        let probe = match scene {
            SCENE => CstSourceProbe::Rebellions,
            SECOND_SCENE => CstSourceProbe::RebellionsSecond,
            THIRD_SCENE => CstSourceProbe::RebellionsThird,
            _ => CstSourceProbe::RebellionsLater,
        };
        let observations = services.cst_text_origins.source_record_observations(shell.context);
        let observed = sources.iter().try_fold(0, |latest, source| {
            let read = observations.iter().rev().find(|entry| {
                entry.probe == probe && entry.source == *source
                    && entry.native == 327 && entry.kind == 0x20 && entry.body_tagged && !entry.localized
                    && entry.stored_codec == ScriptTextEncoding::Gbk && entry.output_codec == ScriptTextEncoding::Gbk
                    && entry.raw_bytes > 0 && entry.raw_bytes == entry.output_bytes
                    && shell.loaded <= entry.frame && entry.frame <= generation.source_frame
            })?;
            Some(latest.max(read.frame))
        })?;
        let flow = window(self, b"flow", b"GAME_WAIT", false, 2, frame)?;
        let receiver = parent_sscript(self, flow, shell.owner)?;
        let region = enter_region(self, receiver)?;
        Some((scene, (shell.group, shell.record), (shell.owner, region, receiver),
            (shell.context, source.load_id, generation.source_epoch, generation.source_frame, observed)))
    }

    /// Identify the guarded checkpoint in the log without serializing text.
    pub fn host_rebellions_body_receipt(&self, _group: i32, owner: u32, context: u32) -> Option<String> {
        let generation = self.data_services.fes_text.story_body_generation(owner)?;
        let story = &self.cst_contexts.get(&context)?.story;
        let (marked, kind, _, _) = story.page?;
        let (_, complete) = generation.natural_complete?;
        let driver = self.data_services.tasks.fes.runtime(owner)?.driver.as_ref()?;
        let window = if driver.is_rebellions_message_fes_source() {
            "meswnd01.fes"
        } else {
            Self::verified_rebellions_novel_fes(self.resources(), &driver.interpreter.program)?
        };
        Some(format!(
            "kind={kind} member={window} marked={marked} complete={complete}",
        ))
    }

    pub fn host_rebellions_novel_diagnostic(&self) -> Option<String> {
        let frame = self.state.clock.frame;
        let services = &self.data_services;
        let (&context, context_state) = self.cst_contexts.iter().next()?;
        let story = &context_state.story;
        let (marked, kind, group, record) = story.page?;
        if !story.is_scene(SCENE) || !(27..=46).contains(&group) {
            return None;
        }
        let (owner, name, runtime, wait_key) = services.tasks.fes.story_runtimes().find(|(_, name, _, _)|
            REBELLIONS_NOVEL_FES.iter().any(|(member, _, _)| *name == member.as_bytes()))?;
        let driver = runtime.driver.as_ref()?;
        let mode = driver.interpreter.local_values().get(100);
        let (prompt, other) = if kind == 0x03 { ("keywait", "keywait2") } else { ("keywait2", "keywait") };
        let layer_state = |key| runtime.media.get(key).map(|layer| (layer.visible, layer.opacity));
        let prompt_state = layer_state(prompt);
        let other_state = layer_state(other);
        let generation = services.fes_text.story_body_generation(owner);
        let sources = generation.and_then(|generation| services.fes_text.story_body_sources(
            owner, generation.source_epoch, &services.cst_text_origins,
        ));
        let records = services.cst_text_origins.source_record_observations(context);
        let read = records.iter().find(|entry| entry.source.group == group
            && entry.source.absolute_record + 1 == record as usize);
        let committed = generation.is_some_and(|generation|
            current_committed_body(generation, story.loaded, marked, frame));
        let composed = generation.is_some_and(|generation|
            services.fes_text.rebellions_novel_composed(owner, generation.source_epoch, frame));
        let prompt_submitted = prompt_submitted(self, owner, prompt);
        let wait_packet = self.events().iter().rev().take_while(|event| event.frame >= marked)
            .find(|event| event.kind == "native-call" && wait_packet(&event.detail, owner, kind))
            .map(|event| event.frame);
        let receiver = window(self, b"flow", b"GAME_WAIT", false, 2, frame)
            .and_then(|flow| parent_sscript(self, flow, owner));
        let region = receiver.and_then(|receiver| enter_region(self, receiver));
        let shell = novel_shell(self, frame).is_some();
        let name = String::from_utf8_lossy(name);
        Some(format!("Rebellions novel frame={frame} scene={SCENE} group={group} record={record} \
            mark={marked} kind={kind} owner={owner} window={name} mode={mode:?} wait_key={wait_key} \
            shell={shell} prompt={prompt}:{prompt_state:?} other={other}:{other_state:?} \
            generation={generation:?} sources={sources:?} read={read:?} committed={committed} \
            composed={composed} prompt_submitted={prompt_submitted} wait_packet={wait_packet:?} \
            receiver={receiver:?} region={region:?}"))
    }
}

fn match_fes_member(player: &HeadlessPlayer, owner: u32, (name, size, digest): (&str, usize, &str)) -> bool {
    let resources = player.resources();
    let program = player.data_services.tasks.fes.runtime(owner).and_then(|runtime| runtime.driver.as_ref())
        .map(|driver| &driver.interpreter.program);
    resources.category_names(b"fes", name).ok()
        .and_then(|names| names.iter().find_map(|name| resources.find(name)))
        .and_then(|candidate| resources.read_candidate_bounded(candidate, size).ok().flatten())
        .filter(|bytes| sha256(bytes) == digest)
        .and_then(|bytes| FesProgram::parse(&bytes).ok())
        .as_ref() == program && program.is_some()
}

fn quiet_root(player: &HeadlessPlayer, frame: u64) -> Option<()> {
    let services = &player.data_services;
    let input = &services.messages;
    (services.story_trace && player.host_story_trace_profile() == Some(StoryTraceProfile::Rebellions)
        && player.loaded.as_ref()?.name == "kcs.int/main.kcs"
        && player.state.clock.frame == frame && !player.frame_in_progress()
        && player.state.input.focused && player.state.input.pressed.is_empty()
        && !services.host_pause.paused && !services.host_quit_requested && player.movie_playbacks.is_empty()
        && player.pending_messages.is_empty() && player.pending_fes_events.is_empty()
        && services.tasks.pending_root_dispatch().is_none() && services.pending_host_input.is_empty()
        && player.message_wait_address == Some(0xad0) && player.vm.as_ref()?.waiting_for_message()
        && player.input_task.alive && input.device_keys_loaded && input.device_foreground
        && input.physical_buttons.is_empty() && input.physical_edges.is_empty() && input.pending_device_buttons.is_empty()
        && input.key_dispatch.is_none() && input.key_selection.is_none()
        && input.button_dispatches.is_empty() && input.button_active.is_empty()).then_some(())
}

fn window(player: &HeadlessPlayer, expected_name: &[u8], block: &[u8], wait_key: bool, count: usize, frame: u64) -> Option<u32> {
    let fes = &player.data_services.tasks.fes;
    if fes.runtimes().count() != count || fes.story_runtimes().count() != count {
        return None;
    }
    let mut matches = fes.story_runtimes().filter(|(_, name, _, _)| {
        name.strip_suffix(b".fes").unwrap_or(name) == expected_name
    });
    let (owner, _, runtime, actual_wait_key) = matches.next()?;
    let driver = runtime.driver.as_ref()?;
    (matches.next().is_none() && actual_wait_key == wait_key
        && !driver.has_pending_handler() && !driver.initialization_pending()
        && driver.interpreter.current_block() == block
        && matches!(driver.waiting(), Some(FesDriverWait::Frame { after }) if *after == frame)).then_some(owner)
}

fn message_owner(player: &HeadlessPlayer, frame: u64, block: &[u8]) -> Option<u32> {
    window(player, b"flow", b"GAME_WAIT", false, 2, frame)?;
    let owner = window(player, WINDOW.as_bytes(), block, true, 2, frame)?;
    let runtime = player.data_services.tasks.fes.runtime(owner)?;
    runtime.driver.as_ref()?.is_rebellions_message_fes_source().then_some(owner)
}

pub(super) fn body_shell(player: &HeadlessPlayer, frame: u64) -> Option<BodyWaitShell<'_>> {
    quiet_root(player, frame)?;
    let mut contexts = player.cst_contexts.iter();
    let (&handle, context) = contexts.next()?;
    if contexts.next().is_some() || context.document.is_none()
        || context.encoding != ScriptTextEncoding::Gbk || context.localization.is_some()
    {
        return None;
    }
    let scene = match context.story.scene.as_str() {
        SCENE => SCENE,
        SECOND_SCENE => SECOND_SCENE,
        THIRD_SCENE => THIRD_SCENE,
        _ => return None,
    };
    let (marked, group, record) = context.story.settled_body_record(frame)?;
    if scene == SCENE && (27..=46).contains(&group)
        || HeadlessPlayer::host_rebellions_scene_waits(scene).get(usize::try_from(group).ok()?) != Some(&record)
    {
        return None;
    }
    let owner = message_owner(player, frame, b"MES_SETSTATE")?;
    Some(BodyWaitShell { window: WINDOW, owner, scene, context: handle, loaded: context.story.loaded,
        marked, group, record })
}

pub(super) fn ordinary_input_diagnostic(player: &HeadlessPlayer, frame: u64) -> Option<String> {
    let shell = body_shell(player, frame)?;
    let flow = window(player, b"flow", b"GAME_WAIT", false, 2, frame)?;
    let receiver = *player.task_parents.get(&shell.owner)?;
    let exact_receiver = parent_sscript(player, flow, shell.owner);
    let input = &player.data_services.messages;
    let regions: Vec<_> = player.scene_nodes.iter().filter(|(_, node)| node.name == "bottom button")
        .map(|(&id, node)| {
            let registration = player.input_task.regions.iter().find(|entry| entry.id() == id);
            let button = input.input_buttons.get(&id);
            (id, node.parent, registration.map(|entry| &entry.words),
                button.map(|button| (&button.state, button.execution_checkpoint().3)),
                input.input_masks.contains_key(&id))
        }).collect();
    let wait_packet_seen = player.events().iter().rev().take_while(|event| event.frame >= shell.marked)
        .any(|event| event.kind == "native-call" && wait_packet(&event.detail, shell.owner, 0x02));
    let ready_region = exact_receiver.and_then(|receiver| enter_region(player, receiver));
    Some(format!("receiver={receiver} exact_receiver={exact_receiver:?} ready_region={ready_region:?} \
        wait_packet={wait_packet_seen} key_enabled={} forced_enter={:?} forced_select={:?} \
        regions(id,parent,registration,state_and_active,masked)={regions:?}",
        player.input_task.key_enabled[LOGICAL_ENTER], input.device_keys.forced_queries(LOGICAL_ENTER as u32),
        input.device_keys.forced_queries(0)))
}


fn novel_shell(player: &HeadlessPlayer, frame: u64) -> Option<(BodyWaitShell<'_>, u32)> {
    quiet_root(player, frame)?;
    let mut contexts = player.cst_contexts.iter();
    let (&handle, context) = contexts.next()?;
    let story = &context.story;
    let (marked, kind, group, record) = story.page?;
    let expected_kind = if matches!(group, 29 | 32 | 37 | 41) { 0x03 } else { 0x02 };
    if contexts.next().is_some() || !story.is_scene(SCENE) || story.candidate.is_some()
        || story.loaded > marked || marked >= frame || !(27..=46).contains(&group)
        || kind != expected_kind || OPENING_WAITS.get(group as usize) != Some(&record)
        || u32::from(context.document.as_ref()?.records.get(record as usize)?.kind) != kind
        || context.encoding != ScriptTextEncoding::Gbk || context.localization.is_some()
    {
        return None;
    }
    let (owner, member) = novel_owner(player, frame, kind)?;
    Some((BodyWaitShell { window: member, owner, scene: SCENE, context: handle,
        loaded: story.loaded, marked, group, record }, kind))
}

fn novel_owner(player: &HeadlessPlayer, frame: u64, kind: u32) -> Option<(u32, &'static str)> {
    let flow = window(player, b"flow", b"GAME_WAIT", false, 2, frame)?;
    let (owner, _, runtime, _) = player.data_services.tasks.fes.story_runtimes()
        .find(|(owner, _, _, _)| *owner != flow)?;
    let driver = runtime.driver.as_ref()?;
    let (mode, prompt, other) = if kind == 0x03 {
        (2, "keywait", "keywait2")
    } else {
        (1, "keywait2", "keywait")
    };
    let visible = |name| runtime.media.get(name).is_some_and(|layer| layer.visible && layer.opacity > 0);
    if driver.has_pending_handler() || driver.initialization_pending()
        || !matches!(driver.waiting(), Some(FesDriverWait::Frame { after }) if *after == frame)
        || driver.story_skip_flags() != (false, false, false, false)
        || driver.interpreter.local_values().get(100) != Some(&mode)
        || !visible(prompt) || visible(other)
    {
        return None;
    }
    let member = HeadlessPlayer::verified_rebellions_novel_fes(player.resources(), &driver.interpreter.program)?;
    Some((owner, member))
}

fn prompt_submitted(player: &HeadlessPlayer, owner: u32, prompt: &str) -> bool {
    player.data_services.tasks.fes.platform.lists.values().any(|list| {
        list.enabled && list.submit
            && list.layers.get(&(owner, prompt.to_owned())).zip(player.image_handles.get(&list.output))
                .is_some_and(|(layer, output)| submitted_body_pixels(layer, output))
    })
}

fn wait_packet(detail: &str, owner: u32, kind: u32) -> bool {
    let Some(args) = detail.strip_prefix("index=630 arguments=16 bytes [") else { return false; };
    let Some((target, call)) = args.split_at_checked(8) else { return false; };
    let exact = match kind {
        0x02 => matches!(call, "73de040082de040000000000]" | "84de040093de040000000000]"),
        0x03 => matches!(call, "95de0400a3de040000000000]" | "a5de0400b3de040000000000]"),
        _ => false,
    };
    exact
        && u32::from_str_radix(target, 16).is_ok_and(|value| value.swap_bytes() == owner)
}

fn live_child(player: &HeadlessPlayer, task: u32, parent: u32, kind: u32) -> bool {
    player.task_parents.get(&task) == Some(&parent)
        && player.scene_nodes.get(&task).is_some_and(|node| node.parent == parent && node.kind == kind)
        && matches!(player.data_services.tasks.message_task_state(task), Some((actual, 0, _)) if actual == kind)
}

fn parent_sscript(player: &HeadlessPlayer, flow: u32, owner: u32) -> Option<u32> {
    let receiver = *player.task_parents.get(&owner)?;
    let program = player.task_vms.get(&receiver)?.program()?;
    (live_child(player, flow, 0, 34) && live_child(player, receiver, flow, 0)
        && live_child(player, owner, receiver, 34)
        && player.data_services.tasks.task_clock(receiver).is_some_and(|clock| clock.paused == 0)
        && exact_sscript(program)).then_some(receiver)
}

fn exact_sscript(program: &KcsProgram) -> bool {
    program.code.len() == 826_331 && program.data.len() == 416_746 && program.stack_size == 10240
        && sha256(&program.code) == SSCRIPT_CODE && sha256(&program.data) == SSCRIPT_DATA
}

fn enter_region(player: &HeadlessPlayer, receiver: u32) -> Option<u32> {
    let input = &player.data_services.messages;
    let mut regions = player.scene_nodes.iter().filter(|(_, node)| node.name == "bottom button");
    let (&region, _) = regions.next()?;
    if regions.next().is_some() || !live_child(player, region, receiver, 6)
        || input.key_capture.contains_key(&(LOGICAL_ENTER as u32)) || input.key_capture.contains_key(&0)
        || input.typed_physical_owners.contains_key(&(ButtonSource::Keyboard, ENTER))
        || input.device_keys.forced_queries(LOGICAL_ENTER as u32).ok()? != 0
        || input.device_keys.forced_queries(0).ok()? != 0
    {
        return None;
    }
    let bindings = player.device_key_bindings();
    let enabled = &player.input_task.key_enabled;
    let keyboard = ButtonSource::Keyboard.index();
    if !enabled[LOGICAL_ENTER] || !bindings[LOGICAL_ENTER].enabled
        || bindings[LOGICAL_ENTER].codes[keyboard][0] != ENTER
        || bindings.iter().enumerate().any(|(logical, binding)| enabled[logical] && binding.enabled
            && binding.codes[keyboard].iter().enumerate().any(|(group, &code)|
                code == ENTER && (group != 0 || !matches!(logical, 0 | LOGICAL_ENTER))))
    {
        return None;
    }
    let mut registrations = player.input_task.regions.iter().filter(|entry| entry.id() == region);
    let words = &registrations.next()?.words;
    let button = input.input_buttons.get(&region)?;
    let state = &button.state;
    (registrations.next().is_none() && button.execution_checkpoint().3
        && words[2] & ButtonSource::Keyboard.mask() != 0 && words[8] != 0 && words[9] != 0 && words[11] == 0
        && state.id == region && state.receiver == Some(receiver) && state.show != 0
        && state.full_screen && state.rectangle == [-100000, -100000, 100000, 100000]
        && words[3..7].iter().map(|&word| word as i32).eq(state.rectangle)
        && state.priority == words[1] as i32 && state.parameter == 0
        && !input.input_masks.contains_key(&region)).then_some(region)
}

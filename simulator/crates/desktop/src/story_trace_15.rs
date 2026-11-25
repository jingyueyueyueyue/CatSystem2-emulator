use cs2_script::FesProgram;

use crate::cst_context_native::CstBodySource;
use crate::particle_plugin_digest::sha256;
use crate::{CstContextState, HeadlessPlayer, ResourceIndex, ScriptTextEncoding};

use super::story_trace_09::{live_child, quiet_root_at, settled_poll};
use super::{committed_body_at, submitted_body_pixels};

pub(crate) const PE: &str = "ac226c8455eff72114d6b1b9929fae01ac49ba90b8ba579f049f179c4c6dd8d5";
const SSCRIPT: &str = "a918cca161a8bc1c656cc3db704404ad75cbb1af2a5d1e96ae92611f0c3cae92";
pub(crate) const MESWND: (usize, &str) =
    (4282, "df7a762af66600a4a3b88ae8b3ec3a24345a2f6d4522fdd3741f4aad9b428905");
const FIRST_BODY: &str = "f24a27f1b25913d93a35eef4d034738f381d9dca401075f6a53d0ca36eceb822";

const SECOND_BODY: &str = "de7bb881ec44615efba5005165d0226093ebace3839611e53f8cbe872db43942";

pub(crate) fn opening_scene(context: &CstContextState) -> bool {
    context.story.is_scene("yum_h01")
        && context.encoding == ScriptTextEncoding::Cp932 && context.localization.is_none()
}

pub(crate) fn first_body(context: &CstContextState, source: CstBodySource) -> bool {
    (source.group, source.absolute_record) == (0, 6)
        && opening_body(context, source, 0, 7)
}

fn opening_body(context: &CstContextState, source: CstBodySource, group: i32, wait: i32) -> bool {
    let (absolute_record, hash) = match (group, wait) {
        (0, 7) => (6, FIRST_BODY),
        (1, 9) => (8, SECOND_BODY),
        (2, 11) => (10, "b3270831c388787931a7844d3dfe9f7237bdba7de5fd11e05ea1a88b646ef145"),
        (3, 13) => (12, "b53a0f579523cbbe9621723917b66a7e57025a9d1c8b6f3fa197ed4fae56e86e"),
        (4, 15) => (14, "62a27a6edf426186385c67347d7c66cdd1112ffcd1a7913dc074b69953f401a2"),
        (5, 17) => (16, "d35deedfed9c76731531ed3c4cdca0678f124f84ef0398469844a4f762bc9ec9"),
        (6, 19) => (18, "30f7b9b6f96c8961a3b946ecc94a688a617e88373f2ebeb30e6c187559421378"),
        (7, 38) => (37, "33a0d72c5066a5d77ec8477055b44c5eb298c42dc5397b8f8f7e52fc9089962a"),
        (8, 41) => (40, "537cb5c446a3a18ccb747c412a48faef9a28a6157cdf6e859e402c4de8b0c262"),
        _ => return false,
    };
    let Some(document) = context.document.as_ref() else { return false; };
    let Some(range) = document.inputs.get(group as usize) else { return false; };
    opening_scene(context) && source.load_id != 0 && source.group == group
        && source.absolute_record == absolute_record
        && range.contains(&absolute_record) && range.contains(&(wait as usize))
        && document.records.get(wait as usize).is_some_and(|record| record.kind == 0x02)
        && document.records.get(absolute_record)
            .is_some_and(|record| record.kind == 0x20 && sha256(&record.text) == hash)
}

pub(crate) fn verified_meswnd(resources: &ResourceIndex, program: &FesProgram) -> Option<()> {
    let (size, digest) = MESWND;
    let names = resources.category_names(b"fes", "meswnd").ok()?;
    let candidate = names.iter().find_map(|name| resources.find(name))?;
    let bytes = resources.read_candidate_bounded(candidate, size).ok()??;
    (bytes.len() == size && sha256(&bytes) == digest
        && &FesProgram::parse(&bytes).ok()? == program).then_some(())
}

struct Observation {
    owner: u32,
    handle: u32,
    loaded: u64,
    wait: Option<(u64, i32, i32)>,
    local100: Option<i32>,
    generation: Option<super::StoryBodyGeneration>,
    source: Option<CstBodySource>,
    layout_complete: Option<(u64, u64)>,
    committed: bool,
    composited: bool,
    enter: Option<(u32, u32)>,
}

fn observe_state(player: &HeadlessPlayer, frame: u64) -> Option<Observation> {
    quiet_root_at(player, frame, 0xa04)?;
    let mut contexts = player.cst_contexts.iter().filter(|(_, context)| context.document.is_some());
    let (&handle, context) = contexts.next()?;
    if contexts.next().is_some() || !opening_scene(context) {
        return None;
    }
    let fes = &player.data_services.tasks.fes;
    if fes.runtimes().count() != 2 {
        return None;
    }
    let window = |name: &[u8], block: &[u8]| {
        let mut windows = fes.story_runtimes().filter(|(_, actual, _, _)| {
            actual.strip_suffix(b".fes").unwrap_or(actual) == name
        });
        let (owner, _, runtime, _) = windows.next()?;
        let driver = runtime.driver.as_ref()?;
        (windows.next().is_none() && settled_poll(driver.interpreter.current_block(), block,
            driver.has_pending_handler() || driver.initialization_pending(), driver.waiting(), frame))
            .then_some((owner, driver))
    };
    let (flow, _) = window(b"flow", b"LOOP_GAME")?;
    let (owner, driver) = window(b"meswnd", b"MES_SHOW")
        .or_else(|| window(b"meswnd", b"WAIT_USER"))?;
    let receiver = *player.task_parents.get(&owner)?;
    if !live_child(player, flow, 0, 34) || !live_child(player, receiver, flow, 0)
        || !live_child(player, owner, receiver, 34)
        || player.task_vms.get(&receiver)?.program_code_sha256() != Some(SSCRIPT)
    {
        return None;
    }
    verified_meswnd(player.resources(), &driver.interpreter.program)?;
    let wait = context.story.settled_body_record(frame);
    let local100 = driver.interpreter.local_values().get(100).copied();
    let text = &player.data_services.fes_text;
    let generation = text.story_body_generation(owner);
    let sources = generation.and_then(|generation| text.story_body_sources(
        owner, generation.source_epoch, &player.data_services.cst_text_origins,
    ));
    let source = match sources { Some([source]) if source.context_handle == handle => Some(*source), _ => None };
    let layout_complete = text.eden_layout_complete(owner);
    let committed = wait.zip(generation).is_some_and(|((marked, group, record), generation)| {
        let completion = match (group, record) {
            (0, 7) => generation.natural_complete.or(layout_complete),
            _ => generation.natural_complete,
        };
        source.is_some_and(|source| opening_body(context, source, group, record))
            && committed_body_at(generation, completion, context.story.loaded, marked, frame)
    });
    let body = (owner, "\0text:737472[0]".to_owned());
    let prompt = (owner, "img_wait".to_owned());
    let mut lists = fes.platform.lists.values().filter(|list| {
        list.layers.contains_key(&body) || list.layers.contains_key(&prompt)
    });
    let composited = lists.next().is_some_and(|list| {
        lists.next().is_none() && list.enabled && list.submit
            && player.image_handles.get(&list.output).is_some_and(|output| {
                [&body, &prompt].into_iter().all(|key| list.layers.get(key)
                    .is_some_and(|layer| submitted_body_pixels(layer, output)))
            })
    });
    let enter = player.legacy_bottom_enter(owner).filter(|(_, target)| *target == receiver);
    Some(Observation { owner, handle, loaded: context.story.loaded, wait, local100,
        generation, source, layout_complete, committed, composited, enter })
}

pub(super) fn observe(player: &HeadlessPlayer, frame: u64) -> Option<String> {
    if player.cst_contexts.is_empty() {
        let tasks = &player.data_services.tasks;
        let (owner, _, _, _) = tasks.fes.story_runtimes()
            .find(|(_, name, _, _)| matches!(*name, b"title" | b"title.fes"))?;
        let clock = tasks.task_clock(owner);
        let audio: Vec<_> = player.sound_channels.iter().filter_map(|(channel, state)| {
            let pending = state.pending_start.as_ref()?;
            Some((*channel, pending.owner, pending.index))
        }).collect();
        let drive = tasks.fes.story_drive_diagnostic(owner)?;
        return Some(format!("title owner={owner} clock={clock:?} pending_audio={audio:?} {drive}"));
    }
    let Observation { owner, handle, loaded, wait, local100, generation, source, layout_complete,
        committed, composited, enter } = observe_state(player, frame)?;
    let sources = source.map(|source| [source]);
    Some(format!("owner={owner} context={handle} loaded={loaded} wait={wait:?} local100={local100:?} \
        generation={generation:?} sources={sources:?} layout_complete={layout_complete:?} committed={committed} composited={composited} enter={enter:?}"))
}

impl HeadlessPlayer {
    pub fn host_replay_labyrinth15_body(&self, group: i32, record: i32)
        -> Option<((u32, u32), (u32, u32, u64), (u64, u64, u64))>
    {
        if self.host_story_trace_profile() != Some(super::StoryTraceProfile::Labyrinth15Observation) {
            return None;
        }
        let observation = observe_state(self, self.state.clock.frame)?;
        let (marked, live_group, live_record) = observation.wait?;
        let generation = observation.generation?;
        if (group, record) != (live_group, live_record) || observation.local100 != Some(1)
            || !observation.committed || !observation.composited
        {
            return None;
        }
        Some((observation.enter?, (observation.owner, observation.handle, observation.source?.load_id),
            (generation.source_epoch, generation.source_frame, marked)))
    }
}

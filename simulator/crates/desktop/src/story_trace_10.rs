use cs2_script::FesProgram;

use crate::cst_context_native::{CstBodySource, CstSourceProbe, CstTextOrigins};
use crate::particle_plugin_digest::sha256;
use crate::{HeadlessPlayer, ScriptTextEncoding};

use super::committed_body_at;
use super::story_trace_09::{body_and_prompt_submitted, quiet_root_at, settled_poll};

pub(crate) const PE: &str = "012c71635932c241cde4df986377516279a50b907166757e89ea7b86ff3f813f";
const SSCRIPT: &str = "9d6af8058b561905845711074189168b9e4ab27b98ec3b95bfec50d9fcc38f74";
const MESWND: &str = "d7e196be3192489a0754989f9a9c5ed58588a6aa6a052b0e12d80fe6b2348ba7";
pub(crate) const CST: &str = "53c269463eb31817ef041755c3f45a52443107353a2b6d919bd231df2fed66dd";
const OPENING: &[(i32, i32, &[(usize, usize, &str)])] = &[
    (0, 4, &[(3, 20, "b437f620c4cbfc51ad4dc7d8074502e44d761ef98b8254265cb0afb6ccbdceb9")]),
    (1, 15, &[
        (13, 48, "910158d2992c29ef52b87a3a6d9686a418d5eec6cec611492d8e10ad4e2bedb5"),
        (14, 18, "c91aebd034281d564778d929366735cf5a42b10ba4d12280e0d9ad122776744c"),
    ]),
];

pub(crate) fn opening_scene(context: &crate::CstContextState) -> bool {
    context.story.is_scene("OP01") && context.encoding == ScriptTextEncoding::Cp932
        && context.localization.is_none()
}

pub(crate) fn verified_meswnd(resources: &crate::ResourceIndex, program: &FesProgram) -> bool {
    const SIZE: usize = 5116;
    let Ok(names) = resources.category_names(b"fes", "meswnd") else { return false; };
    let Some(candidate) = names.iter().find_map(|name| resources.find(name)) else { return false; };
    let Ok(Some(bytes)) = resources.read_candidate_bounded(candidate, SIZE) else { return false; };
    bytes.len() == SIZE && sha256(&bytes) == MESWND
        && FesProgram::parse(&bytes).is_ok_and(|actual| &actual == program)
}

pub(crate) fn source_read(origins: &CstTextOrigins, source: CstBodySource, since: u64, through: u64) -> bool {
    origins.source_record_observations(source.context_handle).iter().any(|entry| {
        entry.probe == CstSourceProbe::Djibril10 && entry.source == source
            && entry.native == 327 && entry.kind == 0x20 && entry.body_tagged && !entry.localized
            && entry.stored_codec == ScriptTextEncoding::Cp932 && entry.output_codec == ScriptTextEncoding::Cp932
            && entry.raw_bytes > 0 && entry.raw_bytes == entry.output_bytes
            && since <= entry.frame && entry.frame <= through
    })
}

struct Observation<'a> {
    owner: u32,
    handle: u32,
    loaded: u64,
    wait: Option<(u64, i32, i32)>,
    block: &'a [u8],
    root: bool,
    poll: bool,
    wait_key: bool,
    local100: Option<i32>,
    fes: bool,
    sscript: bool,
    generation: Option<super::StoryBodyGeneration>,
    sources: Option<&'a [CstBodySource]>,
    exact: bool,
    committed: bool,
    composited: bool,
    enter: Option<(u32, u32)>,
}

impl Observation<'_> {
    fn ready(&self) -> bool {
        self.root && self.poll && self.local100 == Some(1) && self.fes && self.sscript
            && self.enter.is_some() && self.committed && self.composited
    }
}

fn observe_state(player: &HeadlessPlayer, frame: u64) -> Option<Observation<'_>> {
    let mut contexts = player.cst_contexts.iter().filter(|(_, context)| context.document.is_some());
    let (&handle, context) = contexts.next()?;
    if contexts.next().is_some() || !opening_scene(context) {
        return None;
    }
    let document = context.document.as_ref()?;
    let wait = context.story.settled_body_record(frame);
    let mut windows = player.data_services.tasks.fes.story_runtimes().filter(|(_, name, _, _)| {
        name.strip_suffix(b".fes").unwrap_or(name) == b"meswnd"
    });
    let (owner, _, runtime, wait_key) = windows.next()?;
    if windows.next().is_some() {
        return None;
    }
    let driver = runtime.driver.as_ref()?;
    let block = driver.interpreter.current_block();
    let poll = settled_poll(block, b"MES_SHOW",
        driver.has_pending_handler() || driver.initialization_pending(), driver.waiting(), frame);
    let local100 = driver.interpreter.local_values().get(100).copied();
    let root = quiet_root_at(player, frame, 0x9f6).is_some();
    let fes = verified_meswnd(player.resources(), &driver.interpreter.program);
    let receiver = player.task_parents.get(&owner).copied();
    let sscript = receiver.and_then(|id| player.task_vms.get(&id))
        .is_some_and(|vm| vm.program_code_sha256() == Some(SSCRIPT));
    let enter = player.legacy_bottom_enter(owner);
    let text = &player.data_services.fes_text;
    let generation = text.story_body_generation(owner);
    let sources = generation.and_then(|generation| text.story_body_sources(
        owner, generation.source_epoch, &player.data_services.cst_text_origins,
    ));
    let loaded = context.story.loaded;
    let exact = wait.zip(sources).zip(generation).is_some_and(|(((_, group, record), sources), generation)| {
        let Some((_, _, expected)) = OPENING.iter().find(|&&(g, r, _)| (g, r) == (group, record))
            else { return false; };
        let Some(range) = document.inputs.get(group as usize) else { return false; };
        let record = record as usize;
        range.contains(&record) && document.records.get(record).is_some_and(|wait| wait.kind == 0x02)
            && sources.len() == expected.len()
            && sources.iter().zip(*expected).all(|(&source, &(absolute, size, digest))| {
                (source.context_handle, source.group, source.absolute_record) == (handle, group, absolute)
                    && source.load_id != 0 && range.contains(&absolute)
                    && document.records.get(absolute).is_some_and(|record|
                        record.kind == 0x20 && record.text.len() == size && sha256(&record.text) == digest)
                    && source_read(&player.data_services.cst_text_origins, source, loaded, generation.source_frame)
            })
    });
    let committed = wait.zip(generation).is_some_and(|((marked, _, _), generation)| {
        exact && committed_body_at(generation, generation.natural_complete, loaded, marked, frame)
    });
    let composited = body_and_prompt_submitted(player, owner);
    Some(Observation { owner, handle, loaded, wait, block, root, poll, wait_key, local100, fes, sscript,
        generation, sources, exact, committed, composited, enter })
}

pub(super) fn observe(player: &HeadlessPlayer, frame: u64) -> Option<String> {
    let view = observe_state(player, frame)?;
    let ready = view.ready();
    let Observation { owner, handle, loaded, wait, block, root, poll, wait_key, local100, fes, sscript,
        generation, sources, exact, committed, composited, enter } = view;
    let block = String::from_utf8_lossy(block);
    Some(format!("owner={owner} context={handle} loaded={loaded} wait={wait:?} block={block:?} \
        root={root} poll={poll} wait_key={wait_key} local100={local100:?} fes={fes} sscript={sscript} \
        generation={generation:?} sources={sources:?} exact={exact} committed={committed} \
        composited={composited} enter={enter:?} ready={ready}"))
}

impl HeadlessPlayer {
    pub fn host_replay_djibril10_body(&self, group: i32, record: i32)
        -> Option<((u32, u32), (u32, u32, u64), (u64, u64, u64))>
    {
        if self.host_story_trace_profile() != Some(super::StoryTraceProfile::Djibril10Observation) {
            return None;
        }
        let view = observe_state(self, self.state.clock.frame)?;
        let (marked, live_group, live_record) = view.wait?;
        if (live_group, live_record) != (group, record) || !view.ready() {
            return None;
        }
        let source = view.sources?.first()?;
        let generation = view.generation?;
        Some((view.enter?, (view.owner, view.handle, source.load_id),
            (generation.source_epoch, generation.source_frame, marked)))
    }
}

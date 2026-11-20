use super::{BodyWaitShell, HeadlessPlayer, body_sources_match, current_committed_body, submitted_body_pixels};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Sample {
    Yukikoi,
    Kokonatsu,
    Kamikaze,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Page {
    pub group: i32,
    pub body: usize,
    pub wait: i32,
}

#[derive(Clone, Copy)]
pub(crate) struct Profile {
    sample: Sample,
    pe: &'static str,
    sscript: &'static str,
    fes: &'static str,
    cst: &'static str,
    pub scene: &'static str,
    pub root_wait: u32,
    pub flow_block: &'static [u8],
    pub prompt: &'static str,
    pub first: Page,
    pub next: Page,
}

const PROFILES: [Profile; 3] = [
    Profile {
        sample: Sample::Yukikoi,
        pe: "63de0e14cd8ce11ad2d68e7ed6076641ff5e5b73d822603cc52764009d812976",
        sscript: "797acd759f64c280fbb39725fdc413b055d56343e821d16036cc7e38516b4d95",
        fes: "2bb76c39f9f74874023af57f0d6186b3a5d1ca5e2645415f396ef87f5e1e1176",
        cst: "7acf10b32a2184fbadd42544b01b8b0eeb0254ad109dd9bb653a80c67102922d",
        scene: "__tar01",
        root_wait: 0xea8,
        flow_block: b"LOOP_GAME",
        prompt: "img_wait",
        first: Page { group: 0, body: 53, wait: 54 },
        next: Page { group: 1, body: 66, wait: 67 },
    },
    Profile {
        sample: Sample::Kokonatsu,
        pe: "c47c9d883126f641ce07d92eb57b6ac9833fe7495996001c12c68c55b99974d3",
        sscript: "5c20bbc42cde82bee76e19fddc017845cbcfa2c0609f8b0cfce2de9caf189b68",
        fes: "121cb875089ba0a72519b2985e1f5a7212a9bed2f64140cde97982f61016b438",
        cst: "0e65ce910fc3068cd42f0d968bb72d67c3451dfdf1d323e5451b76c334623e8e",
        scene: "C01_01",
        root_wait: 0xc72,
        flow_block: b"WAIT_GAME",
        prompt: "keywait",
        first: Page { group: 0, body: 61, wait: 65 },
        next: Page { group: 1, body: 98, wait: 107 },
    },
    Profile {
        sample: Sample::Kamikaze,
        pe: "f6af7437475b8c066be973245aa79688bea510ca1192264add4f1914ebee4c0e",
        sscript: "e45413514526ffe3c52febd8081009d3404953c8d7c26fffdca01d8142646f49",
        fes: "342adb5f26579ad7da5bd45557c57fa16a0b2c2ba303d004612b6e858c22b99a",
        cst: "bccd3463db8d0f173101609a1ad6e57f4edc5d7f2c084763a17826863d8ce086",
        scene: "com01",
        root_wait: 0x9b3,
        flow_block: b"WAIT_GAME",
        prompt: "keywait",
        first: Page { group: 0, body: 56, wait: 57 },
        next: Page { group: 1, body: 61, wait: 62 },
    },
];

pub(crate) fn verified_profile(
    pe: &str, startup: &str, sscript: &str, fes: &str, cst: &str,
) -> Option<Profile> {
    PROFILES.into_iter().find(|profile| {
        startup == "kcs.int/main.kcs" && pe == profile.pe && sscript == profile.sscript
            && fes == profile.fes && cst == profile.cst
    })
}

fn legacy_body_profile(pe: &str, startup: Option<&str>) -> Option<Profile> {
    PROFILES.into_iter().find(|profile| profile.sample != Sample::Kamikaze
        && profile.pe == pe && startup == Some("kcs.int/main.kcs"))
}

pub(super) fn is_04_06_profile(pe: &str, startup: Option<&str>) -> bool {
    legacy_body_profile(pe, startup).is_some()
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct LoadedLegacyResources<'a> {
    pub sscript: Option<(u32, &'a str)>,
    pub meswnd: Option<(u32, &'a str)>,
    pub cst: Option<(u32, u64, &'a str)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ResourceMatch {
    Missing,
    Unbound,
    Mismatch,
    Exact,
}

fn resource_match(expected: &str, actual: Option<(&str, bool)>) -> ResourceMatch {
    match actual {
        None => ResourceMatch::Missing,
        Some((_, false)) => ResourceMatch::Unbound,
        Some((digest, true)) if digest == expected => ResourceMatch::Exact,
        Some(_) => ResourceMatch::Mismatch,
    }
}

fn observed_body_page(profile: Profile, group: i32, record: i32) -> Option<Page> {
    [profile.first, profile.next].into_iter().find(|page| page.group == group && page.wait == record)
}

fn layer_submissions(player: &HeadlessPlayer, owner: u32, name: &str) -> (usize, Option<(u32, u32)>) {
    let key = (owner, name.to_owned());
    let lists = &player.data_services.tasks.fes.platform.lists;
    let mut matches = lists.iter().filter_map(|(&id, list)| {
        list.layers.get(&key).map(|layer| (id, list, layer))
    });
    let Some((id, list, layer)) = matches.next() else { return (0, None); };
    let count = 1 + matches.count();
    let submitted = (count == 1 && list.enabled && list.submit
        && player.image_handles.get(&list.output).is_some_and(|output| submitted_body_pixels(layer, output)))
        .then_some((id, list.output));
    (count, submitted)
}

pub(super) fn observe_04_06(
    player: &HeadlessPlayer, frame: u64, resources: LoadedLegacyResources<'_>,
) -> Option<String> {
    let profile = legacy_body_profile(player.host_story_pe_sha256()?, player.default_entry())?;
    if !player.loaded.as_ref().is_some_and(|entry| entry.kind == "KCS" && entry.name == "kcs.int/main.kcs") {
        return None;
    }
    let tasks = &player.data_services.tasks;
    let live_task = |id, kind| {
        tasks.records.get(&id).is_some_and(|record| record.kind == kind) && tasks.task_is_registered(id)
    };
    let origins = &player.data_services.cst_text_origins;
    let text = &player.data_services.fes_text;
    let mut windows = tasks.fes.story_runtimes().filter(|(_, name, _, _)| {
        name.strip_suffix(b".fes").unwrap_or(name).eq_ignore_ascii_case(b"meswnd")
    });
    let first = windows.next();
    let window_count = usize::from(first.is_some()) + windows.count();
    let window = first.filter(|(owner, _, _, _)| window_count == 1 && live_task(*owner, 34));
    let owner = window.map(|(owner, _, _, _)| owner);
    let window_state = window.map(|(owner, _, runtime, wait_key)| {
        (owner, wait_key, runtime.driver.as_ref().map(|driver| {
            (String::from_utf8_lossy(driver.interpreter.current_block()), driver.has_pending_handler(), driver.waiting())
        }))
    });
    let mut contexts = player.cst_contexts.iter().filter(|(_, context)| {
        context.document.is_some() && !context.story.scene.is_empty() && context.story.loaded <= frame
    });
    let first = contexts.next();
    let context_count = usize::from(first.is_some()) + contexts.count();
    let context = first.filter(|_| context_count == 1);
    let position = context.map(|(&id, context)| {
        let story = &context.story;
        (id, story.scene.as_str(), story.loaded, story.candidate, story.page)
    });
    let shell = context.zip(owner).and_then(|((&id, context), owner)| {
        let story = &context.story;
        let (marked, group, record) = story.settled_body_record(frame)?;
        (story.scene == profile.scene).then_some(BodyWaitShell {
            window: "meswnd", owner, scene: &story.scene, context: id, loaded: story.loaded, marked, group, record,
        })
    });
    let generation = owner.and_then(|owner| text.story_body_generation(owner));
    let sources = owner.zip(generation).and_then(|(owner, generation)| {
        text.story_body_sources(owner, generation.source_epoch, origins)
    });
    let cst_load = context.zip(sources).and_then(|((&id, _), sources)| {
        let load = sources.first()?.load_id;
        (load != 0 && sources.iter().all(|source| source.context_handle == id && source.load_id == load))
            .then_some((id, load))
    });
    let exact_body = shell.zip(sources).is_some_and(|(shell, sources)| {
        let Some(page) = observed_body_page(profile, shell.group, shell.record) else { return false; };
        matches!(sources, [source] if source.absolute_record == page.body)
            && player.cst_contexts.get(&shell.context).and_then(|context| context.document.as_ref())
                .is_some_and(|document| body_sources_match(document, shell, Some(sources)))
    });
    let same_epoch = shell.zip(generation).is_some_and(|(shell, generation)| {
        exact_body && frame == player.state.clock.frame && !player.frame_in_progress()
            && current_committed_body(generation, shell.loaded, shell.marked, frame)
    });
    let body = owner.map(|owner| layer_submissions(player, owner, "\0text:737472[0]"));
    let prompt = owner.map(|owner| layer_submissions(player, owner, profile.prompt));
    let body_submitted = same_epoch && body.is_some_and(|(_, submission)| submission.is_some());
    let sscript_match = resource_match(profile.sscript, resources.sscript.map(|(task, digest)| {
        (digest, live_task(task, 0))
    }));
    let fes_match = resource_match(profile.fes, resources.meswnd.map(|(task, digest)| (digest, Some(task) == owner)));
    let cst_match = resource_match(profile.cst, resources.cst.map(|(id, load, digest)| {
        (digest, Some((id, load)) == cst_load)
    }));
    Some(format!(
        "legacy_body sample={:?} frame={frame} input_authorized=false trace_enabled={} \
         focused={} paused={} root_wait={:?} vm_waiting_for_message={} pending_messages={} movies={} \
         contexts={context_count} position(context,scene,loaded,candidate,page)={position:?} \
         meswnds={window_count} meswnd(owner,wait_key,driver)={window_state:?} \
         loaded_resources={resources:?} resource_match(sscript,meswnd,cst)=({sscript_match:?},{fes_match:?},{cst_match:?}) \
         cst_load={cst_load:?} sources={sources:?} exact_body={exact_body} generation={generation:?} same_epoch={same_epoch} \
         body(lists,submitted_list_output)={body:?} body_submitted={body_submitted} \
         prompt(name,lists_submission)=({:?},{prompt:?})",
        profile.sample, player.data_services.story_trace, player.state.input.focused,
        player.data_services.host_pause.paused, player.message_wait_address,
        player.vm.as_ref().is_some_and(|vm| vm.waiting_for_message()),
        player.pending_messages.len(), player.movie_playbacks.len(), profile.prompt,
    ))
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct LocalWait07 {
    pub owner: u32,
    pub named_wait_key: bool,
    pub local_0: Option<i32>,
    pub local_52: Option<i32>,
    pub local_100: Option<i32>,
    pub keywait: Option<(bool, u8)>,
}

impl Profile {
    pub(crate) fn observe_local_wait(
        self, owner: u32, named_wait_key: bool, locals: &[i32], keywait: Option<(bool, u8)>,
    ) -> Option<LocalWait07> {
        (self.sample == Sample::Kamikaze && owner != 0).then(|| LocalWait07 {
            owner,
            named_wait_key,
            local_0: locals.first().copied(),
            local_52: locals.get(52).copied(),
            local_100: locals.get(100).copied(),
            keywait,
        })
    }
}

pub(super) fn observe_07(player: &HeadlessPlayer, frame: u64) -> Option<String> {
    let profile = PROFILES[2];
    if player.host_story_pe_sha256() != Some(profile.pe)
        || player.default_entry() != Some("kcs.int/main.kcs")
        || !player.loaded.as_ref().is_some_and(|entry| entry.kind == "KCS" && entry.name == "kcs.int/main.kcs")
    {
        return None;
    }

    let contexts: Vec<_> = player.cst_contexts.iter().map(|(handle, context)| {
        let story = &context.story;
        (handle, &story.scene, story.loaded, story.candidate, story.page,
            context.position, context.document.is_some())
    }).collect();
    let platform = &player.data_services.tasks.fes.platform;
    let mut windows = Vec::new();
    let mut meswnd = Vec::new();
    for (owner, name, runtime, named_wait_key) in player.data_services.tasks.fes.story_runtimes() {
        let driver = runtime.driver.as_ref();
        windows.push((owner, String::from_utf8_lossy(name), named_wait_key, driver.map(|driver| {
            (String::from_utf8_lossy(driver.interpreter.current_block()),
                driver.has_pending_handler(), driver.waiting())
        })));
        if !name.strip_suffix(b".fes").unwrap_or(name).eq_ignore_ascii_case(b"meswnd") {
            continue;
        }
        let locals = driver.map_or(&[][..], |driver| driver.interpreter.local_values());
        let media = runtime.media.get(profile.prompt).map(|layer| (layer.visible, layer.opacity));
        let prompt = (owner, profile.prompt.to_owned());
        let projections: Vec<_> = platform.lists.iter().filter_map(|(id, list)| {
            let layer = list.layers.get(&prompt)?;
            let output = player.image_handles.get(&list.output);
            let supported_pixels = list.enabled && list.submit
                && output.is_some_and(|output| submitted_body_pixels(layer, output));
            Some((id, list.output, list.enabled, list.submit, layer.options.visible,
                layer.options.opacity, layer.options.position, layer.options.clip, supported_pixels))
        }).collect();
        meswnd.push((profile.observe_local_wait(owner, named_wait_key, locals, media), projections,
            player.data_services.fes_text.story_body_generation(owner)));
    }
    Some(format!(
        "legacy_07 frame={frame} focused={} paused={} message_wait={:?} vm_waiting_for_message={} pending_messages={} movies={} \
         windows(owner,name,named_wait_key,driver)={windows:?} \
         cst(handle,scene,loaded,candidate,page,position,has_document)={contexts:?} \
         meswnd(locals,keywait_lists(id,output,enabled,submit,visible,opacity,position,clip,supported_pixels),body_generation)={meswnd:?}",
        player.state.input.focused, player.data_services.host_pause.paused,
        player.message_wait_address, player.vm.as_ref().is_some_and(|vm| vm.waiting_for_message()),
        player.pending_messages.len(), player.movie_playbacks.len(),
    ))
}

#[derive(Clone, Copy)]
pub(crate) struct PageProof {
    pub profile: Profile,
    pub page: Page,
    pub owner: u32,
    pub context: u32,
    pub load: u64,
    pub frame: u64,
    pub marked: u64,
    pub body_epoch: u64,
    pub source_frame: u64,
}

impl PageProof {
    fn settled(self) -> bool {
        self.owner != 0 && self.context != 0 && self.load != 0 && self.body_epoch != 0
            && self.marked < self.frame && self.source_frame <= self.frame
    }
}

#[derive(Clone, Copy)]
enum Phase {
    Initial,
    Armed(PageProof),
    Received(PageProof, u64),
    Stopped,
}

pub(crate) struct SingleAdvance(Phase);

impl SingleAdvance {
    pub(crate) fn new() -> Self {
        Self(Phase::Initial)
    }

    pub(crate) fn arm(&mut self, before: PageProof) -> Option<u32> {
        if !matches!(self.0, Phase::Initial) || before.profile.sample == Sample::Kamikaze
            || before.page != before.profile.first || !before.settled()
        {
            return None;
        }
        self.0 = Phase::Armed(before);
        Some(13)
    }

    pub(crate) fn root_resume(&mut self, frame: u64, words: [u32; 4]) -> bool {
        let Phase::Armed(before) = self.0 else {
            self.0 = Phase::Stopped;
            return false;
        };
        if frame < before.frame || frame - before.frame > 1 || words != [2, 0, 0, 0] {
            self.0 = Phase::Stopped;
            return false;
        }
        self.0 = Phase::Received(before, frame);
        true
    }

    pub(crate) fn successor(&mut self, after: PageProof, key_released: bool) -> bool {
        let Phase::Received(before, receipt) = self.0 else { return false; };
        if !key_released {
            return false;
        }
        self.0 = Phase::Stopped;
        let profile = before.profile;
        after.profile.sample == profile.sample && after.page == profile.next && after.settled()
            && (after.owner, after.context, after.load) == (before.owner, before.context, before.load)
            && after.marked >= receipt && after.source_frame >= receipt
            && after.body_epoch > before.body_epoch
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proof(profile: Profile) -> PageProof {
        PageProof { profile, page: profile.first, owner: 12, context: 42, load: 1,
            frame: 900, marked: 852, body_epoch: 3, source_frame: 852 }
    }

    fn next(before: PageProof) -> PageProof {
        PageProof { page: before.profile.next, frame: 1300, marked: 1250,
            body_epoch: 4, source_frame: 1240, ..before }
    }

    #[test]
    fn legacy_body_observation_keeps_missing_and_unbound_resources_distinct() {
        for profile in PROFILES.into_iter().take(2) {
            assert!(is_04_06_profile(profile.pe, Some("kcs.int/main.kcs")));
            assert!(!is_04_06_profile(profile.pe, Some("main.kcs")));
            assert_eq!(observed_body_page(profile, profile.first.group, profile.first.wait), Some(profile.first));
            assert_eq!(observed_body_page(profile, profile.next.group, profile.next.wait), Some(profile.next));
            assert_eq!(observed_body_page(profile, profile.first.group, profile.first.wait - 1), None);
            assert_eq!(resource_match(profile.cst, None), ResourceMatch::Missing);
            assert_eq!(resource_match(profile.cst, Some((profile.cst, false))), ResourceMatch::Unbound);
            assert_eq!(resource_match(profile.cst, Some((profile.fes, true))), ResourceMatch::Mismatch);
            assert_eq!(resource_match(profile.cst, Some((profile.cst, true))), ResourceMatch::Exact);
        }
        assert!(!is_04_06_profile(PROFILES[2].pe, Some("kcs.int/main.kcs")));
    }

    #[test]
    fn legacy_wait_identity_is_exact_and_07_cannot_arm_input() {
        let profile = PROFILES[2];
        assert!(verified_profile(profile.pe, "kcs.int/main.kcs", profile.sscript, profile.fes, profile.cst).is_some());
        assert!(verified_profile(profile.pe, "kcs.int/main.kcs", profile.sscript, PROFILES[1].fes, profile.cst).is_none());
        assert_eq!(SingleAdvance::new().arm(proof(profile)), None);
        let mut locals = [0; 101];
        locals[52] = 1;
        locals[100] = 1;
        let wait = profile.observe_local_wait(12, false, &locals, Some((true, 255))).unwrap();
        assert_eq!((wait.named_wait_key, wait.local_52, wait.local_100), (false, Some(1), Some(1)));
        assert_eq!(profile.observe_local_wait(12, false, &locals[..100], None).unwrap().local_100, None);
    }

    #[test]
    fn legacy_first_advance_needs_one_root_receipt_and_a_fresh_successor() {
        for profile in PROFILES.into_iter().take(2) {
            let before = proof(profile);
            let after = next(before);
            let mut press = SingleAdvance::new();
            assert_eq!(press.arm(before), Some(13));
            assert_eq!(press.arm(before), None);
            assert!(!press.successor(after, true));
            assert!(press.root_resume(901, [2, 0, 0, 0]));
            assert!(!press.successor(after, false));
            assert!(press.successor(after, true));
            assert_eq!(press.arm(before), None);
        }
    }

    #[test]
    fn legacy_first_advance_rejects_wrong_duplicate_or_stale_evidence() {
        let before = proof(PROFILES[0]);
        let after = next(before);
        for (frame, words) in [(901, [2, 638, 446, 0]), (903, [2, 0, 0, 0])] {
            let mut press = SingleAdvance::new();
            press.arm(before);
            assert!(!press.root_resume(frame, words));
            assert!(!press.successor(after, true));
        }
        for invalid in [PageProof { body_epoch: before.body_epoch, ..after },
                        PageProof { load: before.load + 1, ..after },
                        PageProof { source_frame: before.frame, ..after },
                        PageProof { page: before.page, ..after }] {
            let mut press = SingleAdvance::new();
            press.arm(before);
            assert!(press.root_resume(901, [2, 0, 0, 0]));
            assert!(!press.successor(invalid, true));
        }
        let mut press = SingleAdvance::new();
        press.arm(before);
        assert!(press.root_resume(901, [2, 0, 0, 0]));
        assert!(!press.root_resume(901, [2, 0, 0, 0]));
        assert!(!press.successor(after, true));
    }
}

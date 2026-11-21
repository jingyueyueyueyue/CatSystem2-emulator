use cs2_runtime::message_object::ButtonSource;
use cs2_script::{FesProgram, KcsProgram};
use crate::{
    ScriptTextEncoding,
    cst_context_native::{CstSourceProbe, NEWTON_LOCALIZED_PE, NEWTON_PE, happiness_route_data, newton_route_data},
    particle_plugin_digest::sha256,
};
use super::story_trace_09::{body_and_prompt_submitted, bottom_registration, enter_mapping, live_child, quiet_root_at, settled_poll};
use super::{BodyWaitShell, HeadlessPlayer, body_sources_match, current_committed_body, submitted_body_pixels};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Sample {
    Happiness,
    Yukikoi,
    Kokonatsu,
    Kamikaze,
    Newton,
    Sudden,
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

pub(super) const YUKIKOI_PE: &str = "63de0e14cd8ce11ad2d68e7ed6076641ff5e5b73d822603cc52764009d812976";
const UTATE_PE: &str = "ad70ab1bb00cc8ff9e62a333d12be2e700c1d57880fec0a6b0f8e41cad93f446";
const UTATE_ROOT_WAIT: u32 = 0x929;

const PROFILES: [Profile; 6] = [
    Profile {
        sample: Sample::Yukikoi,
        pe: YUKIKOI_PE,
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
    Profile {
        sample: Sample::Happiness,
        pe: "21dfeb88edafc4bf19226d158e040687738f592282c578740e9d972127938df7",
        sscript: "a853f3e0a1e1a1476cedd4bbc293a9ef93e996cec2db9986b1dd793ca71a3d15",
        fes: "b86c9f0669ee9339444ae716ddf4c7d73ecba8faaa2460fad3801c1fe2899d37",
        cst: happiness_route_data::FIRST_SHA256,
        scene: "0_01_01",
        root_wait: 0xe0d,
        flow_block: b"WAIT_GAME",
        prompt: "keywait",
        first: Page { group: 0, body: 100, wait: 101 },
        next: Page { group: 1, body: 103, wait: 123 },
    },
    Profile {
        sample: Sample::Newton,
        pe: NEWTON_PE,
        sscript: "1f581def5d97d2f8a5ecb465f4ee5803fa30788d7c8fba0852b4b26ce769c5c4",
        fes: "707cf6541aff274d25f7bfc1647ab9518013dd279fce50a9bc0f7bcc8deddd09",
        cst: "91e7241ee4dc6b0c36d697913cf22bb9649d7ea5b924b88e199a3bcb6e8f4e5b",
        scene: "com_001A",
        root_wait: 0xd0c,
        flow_block: b"GAME_WAIT",
        prompt: "keywait",
        first: Page { group: 0, body: 15, wait: 16 },
        next: Page { group: 1, body: 23, wait: 24 },
    },
    Profile {
        sample: Sample::Sudden,
        pe: "35ec21bcfcee0f324206e945eef1c082e35fd8af594ea0e738a46cd941f5b3f3",
        sscript: "1153e10b75c189b7908858d0165c6b2020a09d859aa2005704a4e710ea293a95",
        fes: "43cd548dc7bad4e08df4aca6a8495a598bb425a78d2353ba390f53e7a38acc96",
        cst: "dea050b2a240e487c5ceed1aca21cfa6e1fb985b71a3f856b1a5aea0f92a51c3",
        scene: "common00",
        root_wait: 0x918,
        flow_block: b"GAME_WAIT",
        prompt: "keywait",
        first: Page { group: 0, body: 7, wait: 8 },
        next: Page { group: 1, body: 9, wait: 10 },
    },
];

const KOKONATSU_C02: Profile = Profile {
    scene: "C02_01",
    cst: "40e66b54ddf95e209a8fca215374ad273cc9ce4a97fe868c8ac99dc09062b04d",
    first: Page { group: 0, body: 46, wait: 47 },
    next: Page { group: 1, body: 48, wait: 49 },
    ..PROFILES[1]
};

const HAPPINESS_SECOND: Profile = Profile {
    scene: "0_01_02",
    cst: happiness_route_data::SECOND_SHA256,
    first: Page { group: 0, body: 62, wait: 63 },
    next: Page { group: 1, body: 68, wait: 74 },
    ..PROFILES[3]
};

const HAPPINESS_THIRD: Profile = Profile {
    scene: "0_01_03",
    cst: happiness_route_data::THIRD_SHA256,
    first: Page { group: 0, body: 73, wait: 102 },
    next: Page { group: 1, body: 106, wait: 107 },
    ..PROFILES[3]
};

const NEWTON_SECOND: Profile = Profile {
    scene: "com_001B",
    cst: newton_route_data::SECOND_SHA256,
    first: Page { group: 0, body: 27, wait: 28 },
    next: Page { group: 1, body: 29, wait: 30 },
    ..PROFILES[4]
};

pub(crate) fn verified_profile(
    pe: &str, startup: &str, sscript: &str, fes: &str, cst: &str,
) -> Option<Profile> {
    PROFILES.into_iter().find(|profile| {
        startup == "kcs.int/main.kcs" && profile.matches_pe(pe) && sscript == profile.sscript
            && fes == profile.fes && cst == profile.cst
    })
}

pub(super) fn legacy_body_profile(pe: &str, startup: Option<&str>) -> Option<Profile> {
    PROFILES.into_iter().find(|profile| profile.sample != Sample::Kamikaze
        && startup == Some("kcs.int/main.kcs")
        && (profile.matches_pe(pe) || profile.sample == Sample::Sudden
            && pe == "d158c8915d4982c82badc631b4f92e1f140b246df5929ff2d9924d242c66765c"))
}

pub(super) fn is_04_06_profile(pe: &str, startup: Option<&str>) -> bool {
    legacy_body_profile(pe, startup)
        .is_some_and(|profile| matches!(profile.sample, Sample::Yukikoi | Sample::Kokonatsu))
}

pub(crate) fn cst_probe_04_06(pe: Option<&str>, member: &str, bytes: &[u8]) -> Option<CstSourceProbe> {
    let pe = pe?;
    if !is_04_06_profile(pe, Some("kcs.int/main.kcs")) {
        return None;
    }
    let profile = legacy_body_profile(pe, Some("kcs.int/main.kcs"))?;
    let stem = member.rsplit_once('.')?;
    let profile = profile.for_scene(stem.0);
    if !stem.0.eq_ignore_ascii_case(profile.scene) || !stem.1.eq_ignore_ascii_case("cst")
        || sha256(bytes) != profile.cst
    {
        return None;
    }
    Some(profile.body_probe())
}

impl Profile {
    fn for_scene(self, scene: &str) -> Self {
        match self.sample {
            Sample::Kokonatsu if scene.eq_ignore_ascii_case(KOKONATSU_C02.scene) => KOKONATSU_C02,
            Sample::Happiness if scene.eq_ignore_ascii_case(HAPPINESS_SECOND.scene) => HAPPINESS_SECOND,
            Sample::Happiness if scene.eq_ignore_ascii_case(HAPPINESS_THIRD.scene) => HAPPINESS_THIRD,
            Sample::Newton if scene.eq_ignore_ascii_case(NEWTON_SECOND.scene) => NEWTON_SECOND,
            _ => self,
        }
    }

    fn matches_pe(self, pe: &str) -> bool {
        // Both Newton 2.6.1.78 executables use the same CST and input receivers.
        self.pe == pe || self.sample == Sample::Newton && pe == NEWTON_LOCALIZED_PE
    }

    pub(super) fn window(self) -> &'static str {
        if self.sample == Sample::Newton { "cinema" } else { "meswnd" }
    }

    pub(super) fn body_window(self, scene: &str, group: i32) -> &'static str {
        // com_001A r67 switches from the cinema window to the default message window.
        if self.sample == Sample::Newton
            && (scene == "com_001A" && (12..=56).contains(&group)
                || scene.eq_ignore_ascii_case(NEWTON_SECOND.scene)
                    && !matches!(group, 19..=36 | 209..=214))
        {
            "meswnd"
        } else {
            self.window()
        }
    }

    pub(super) fn wait_signal(self, named_wait_key: bool, locals: &[i32]) -> bool {
        if self.sample == Sample::Sudden {
            // MES_WAITINPUT writes $100, and MES_SHOW writes visibility to $52.
            locals.get(100) == Some(&1) && locals.get(52).is_some_and(|&visible| visible != 0)
        } else {
            named_wait_key
        }
    }

    fn body_probe(self) -> CstSourceProbe {
        match self.sample {
            Sample::Happiness if self.scene == HAPPINESS_SECOND.scene => CstSourceProbe::HappinessSecond,
            Sample::Happiness if self.scene == HAPPINESS_THIRD.scene => CstSourceProbe::HappinessThird,
            Sample::Happiness => CstSourceProbe::Happiness02,
            Sample::Yukikoi => CstSourceProbe::Yukikoi,
            Sample::Kokonatsu if self.scene == KOKONATSU_C02.scene => CstSourceProbe::KokonatsuC02,
            Sample::Kokonatsu => CstSourceProbe::Kokonatsu,
            Sample::Kamikaze => CstSourceProbe::Kamikaze,
            Sample::Newton if self.scene == NEWTON_SECOND.scene => CstSourceProbe::Newton16B,
            Sample::Newton => CstSourceProbe::Newton16,
            Sample::Sudden => CstSourceProbe::Sudden19,
        }
    }
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
    match (profile.sample, group, record) {
        (Sample::Happiness, 2, 125) => Some(Page { group, body: 124, wait: record }),
        (Sample::Kokonatsu, 2, 110) => Some(Page { group, body: 109, wait: record }),
        (Sample::Kamikaze, 2, 68) => Some(Page { group, body: 67, wait: record }),
        (Sample::Newton, 2, 27) if profile.scene != NEWTON_SECOND.scene =>
            Some(Page { group, body: 25, wait: record }),
        (Sample::Sudden, 2, 12) => Some(Page { group, body: 11, wait: record }),
        _ => {
            if let Some((wait, body)) = profile.body_probe().legacy_continuation_page(group) {
                return (record == wait).then_some(Page { group, body: body[0], wait });
            }
            [profile.first, profile.next].into_iter()
                .find(|page| page.group == group && page.wait == record)
        }
    }
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
                .is_some_and(|document| body_sources_match(document, shell, Some(sources), None))
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

type LegacyBody = ((u32, u32), (u32, u32, u64), (u64, u64, u64));

impl HeadlessPlayer {
    pub const fn host_happiness_opening_waits() -> &'static [(i32, i32)] {
        &happiness_route_data::WAITS
    }

    pub const fn host_newton_opening_waits() -> &'static [(i32, i32)] {
        newton_route_data::WAITS
    }

    /// Read-only proof for a single Enter in supported legacy FES routes.
    pub fn host_replay_legacy_fes_body(&self, address: u32, group: i32, record: i32) -> Option<LegacyBody> {
        if address == UTATE_ROOT_WAIT && self.host_story_pe_sha256() == Some(UTATE_PE) {
            return self.legacy_utate_body(Some((group, record))).ok();
        }
        let profile = legacy_body_profile(self.host_story_pe_sha256()?, self.default_entry())?;
        (address == profile.root_wait).then_some(())?;
        self.legacy_fes_body(profile, (group, record)).ok()
    }

    pub(super) fn legacy_fes_body_diagnostic(&self, frame: u64) -> Option<String> {
        if self.host_story_pe_sha256() == Some(UTATE_PE) {
            return Some(format!("sample=Utate gate={:?}", self.legacy_utate_body(None)));
        }
        let profile = legacy_body_profile(self.host_story_pe_sha256()?, self.default_entry())?;
        let gate = self.host_body_wait_shell(frame)
            .map(|shell| self.legacy_fes_body(profile, (shell.group, shell.record)));
        let local_wait = (profile.sample == Sample::Sudden).then(|| {
            self.data_services.tasks.fes.story_runtimes().find_map(|(owner, name, runtime, named)| {
                if !name.strip_suffix(b".fes").unwrap_or(name).eq_ignore_ascii_case(b"meswnd") {
                    return None;
                }
                let locals = runtime.driver.as_ref()?.interpreter.local_values();
                let prompt = runtime.media.get(profile.prompt).map(|layer| (layer.visible, layer.opacity));
                Some((owner, named, locals.get(52).copied(), locals.get(100).copied(), prompt))
            })
        }).flatten();
        Some(format!("sample={:?} gate={gate:?} local_wait(owner,named,52,100,prompt)={local_wait:?}", profile.sample))
    }

    fn legacy_utate_body(&self, expected: Option<(i32, i32)>) -> Result<LegacyBody, &'static str> {
        use crate::message_native::UTATE_STRING_KIND;
        let frame = self.state.clock.frame;
        quiet_root_at(self, frame, UTATE_ROOT_WAIT).ok_or("root_busy")?;
        let mut contexts = self.cst_contexts.iter().filter(|(_, state)| !state.story.scene.is_empty());
        let (&context, state) = contexts.next().ok_or("cst_context")?;
        if contexts.next().is_some() || !state.story.is_scene("00_01") {
            return Err("scene");
        }
        let (marked, group, record) = state.story.settled_body_record(frame).ok_or("wait_record")?;
        let probe = CstSourceProbe::Utate05;
        let (wait, records) = probe.legacy_continuation_page(group).ok_or("route_page")?;
        if record != wait || expected.is_some_and(|expected| expected != (group, record)) {
            return Err("wait_position");
        }
        let body = records[0];
        let services = &self.data_services;
        let document = state.document.as_ref().ok_or("cst_document")?;
        let raw = &document.records.get(body).ok_or("body_record")?.text;
        let observed = services.cst_text_origins.source_record_observations(context).iter().find(|entry| {
            let source = entry.source;
            (source.context_handle, source.group, source.absolute_record) == (context, group, body)
                && entry.probe == probe && entry.native == 327 && entry.kind == 0x20
                && entry.body_tagged && !entry.localized && !raw.is_empty()
                && entry.stored_codec == ScriptTextEncoding::Cp932 && entry.output_codec == ScriptTextEncoding::Cp932
                && entry.raw_bytes == raw.len() && entry.output_bytes == raw.len()
                && (state.story.loaded..=marked).contains(&entry.frame)
        }).ok_or("cst_read")?;
        let source = observed.source;

        let flow = kamikaze_window(self, b"flow", b"GAME_WAIT", frame).ok_or("flow")?;
        if !live_child(self, flow, 0, 34) || services.tasks.fes.story_runtimes().count() != 1 {
            return Err("flow_chain");
        }
        let mut windows = self.scene_nodes.values().filter(|node| {
            node.name == "meswnd" && live_child(self, node.id, node.parent, 12)
        });
        let window = windows.next().ok_or("ordinary_meswnd")?;
        let (owner, receiver) = (window.id, window.parent);
        if windows.next().is_some() || !live_child(self, receiver, flow, 0)
            || self.task_vms.get(&receiver).and_then(|vm| vm.program_code_sha256())
                != Some("5c07d458eca2528dde2233a733b0b3ad02c48d5180628ae01109ba3bc0a50745")
            || self.task_vms.get(&owner).and_then(|vm| vm.program_code_sha256())
                != Some("57d9b62ea4395517c333394614103f683cb603093084a185149fc059bfbc37ae")
        {
            return Err("ordinary_window_chain");
        }
        let (region, target) = self.legacy_bottom_enter_for_kind(owner, 12).ok_or("enter_registration")?;
        if target != receiver {
            return Err("enter_receiver");
        }
        let visible_child = |parent, kind, name| {
            let mut nodes = self.scene_nodes.values().filter(|node| {
                node.name == name && node.visible && live_child(self, node.id, parent, kind)
            });
            let node = nodes.next()?;
            nodes.next().is_none().then_some(node)
        };
        let plane = visible_child(owner, 2, "message main").ok_or("message_plane")?;
        let node = visible_child(plane.id, UTATE_STRING_KIND, "string").ok_or("string_node")?;
        let shell = BodyWaitShell { window: "meswnd", owner, scene: "00_01", context,
            loaded: state.story.loaded, marked, group, record };
        if !body_sources_match(document, shell, Some(&[source]), None)
            || services.messages.legacy_string_kind(node.id) != Some(UTATE_STRING_KIND)
        {
            return Err("body_record");
        }
        let object = services.messages.object(node.id).ok_or("string_object")?;
        let generation = object.surface_generation();
        let newline_tail = matches!(object.raw_appends(), [prefix, tail]
            if prefix == raw && tail == b"\\n"
                && usize::try_from(wait).ok().and_then(|wait| wait.checked_sub(1))
                    .filter(|&tail_record| tail_record > body)
                    .and_then(|tail_record| document.records.get(tail_record))
                    .is_some_and(|record| record.kind == 0x20 && record.text == b"\\n"));
        if !object.prepared() || object.active() || object.remaining_all().ok() != Some(0)
            || !object.last_complete()
            || !(newline_tail || object.raw_appends().iter().flatten().eq(raw.iter()))
        {
            return Err("body_bytes_or_reveal");
        }
        let draw = services.messages.utate_body_draws.get(&node.id).ok_or("native619")?;
        let completed = draw.completed.ok_or("native616")?;
        if (draw.caller, draw.generation) != (owner, generation)
            || draw.started < observed.frame || completed <= draw.started || completed > frame
        {
            return Err("draw_receipt");
        }
        if !(object.complete_snapshot() || newline_tail && draw.completed_before_newline) {
            return Err("body_completion_snapshot");
        }
        let surface = services.message_device.surface(node.id as i32).ok_or("body_surface")?;
        if surface.generation != generation || surface.dirty.is_some() {
            return Err("body_submission");
        }
        let projected = self.project_message_plane(node).map_err(|_| "body_projection")?;
        let ((matched, opaque), _, clipped) = self.message_plane_rgb_probe(object, &projected, surface.submitted)
            .ok_or("body_composition")?;
        if opaque == 0 || matched != opaque || clipped != (0, 0) {
            return Err("body_pixels");
        }
        Ok(((region, receiver), (owner, context, source.load_id), (generation, draw.started, marked)))
    }

    fn legacy_fes_body(&self, profile: Profile, expected: (i32, i32)) -> Result<LegacyBody, &'static str> {
        let frame = self.state.clock.frame;
        quiet_root_at(self, frame, profile.root_wait).ok_or("root_busy")?;
        let shell = self.host_body_wait_shell(frame).ok_or("wait_shell")?;
        let profile = profile.for_scene(shell.scene);
        if shell.scene != profile.scene || (shell.group, shell.record) != expected {
            return Err("wait_position");
        }
        let page = observed_body_page(profile, shell.group, shell.record).ok_or("route_page")?;

        let services = &self.data_services;
        let generation = services.fes_text.story_body_generation(shell.owner).ok_or("generation")?;
        let (sources, exact, composited) = self.host_current_body(shell, frame);
        let sources = sources.ok_or("body_sources")?;
        let expected: &[usize] = match (profile.sample, page.group) {
            (Sample::Happiness, 1) if profile.scene == "0_01_01" => &[103, 104],
            (Sample::Newton, 2) if profile.scene != NEWTON_SECOND.scene => &[25, 26],
            _ => profile.body_probe().legacy_continuation_page(page.group)
                .map_or(std::slice::from_ref(&page.body), |(_, body)| body),
        };
        if !exact || !sources.iter().map(|source| source.absolute_record).eq(expected.iter().copied()) {
            return Err("body_record");
        }
        let cst = services.cst_text_origins.source_record_observations(shell.context);
        if !sources.iter().all(|source| cst.iter().any(|entry| {
            entry.source == *source && entry.probe == profile.body_probe() && entry.body_tagged
                && entry.raw_bytes > 0 && entry.output_bytes > 0
                && (shell.loaded..=generation.source_frame).contains(&entry.frame)
                && (profile.sample != Sample::Kokonatsu
                    || !entry.localized && entry.stored_codec == ScriptTextEncoding::Cp932
                        && entry.output_codec == ScriptTextEncoding::Cp932
                        && entry.raw_bytes == entry.output_bytes)
                && (profile.sample != Sample::Happiness
                    || !entry.localized && entry.stored_codec == ScriptTextEncoding::Cp932)
                && (!matches!(profile.sample, Sample::Newton | Sample::Sudden)
                    || !entry.localized && entry.stored_codec == ScriptTextEncoding::Gbk
                        && entry.output_codec == ScriptTextEncoding::Gbk)
        })) {
            return Err("cst_read");
        }
        if !composited {
            return Err("body_or_prompt_submission");
        }

        let (region, receiver) = self.legacy_bottom_enter(shell.owner).ok_or("enter_registration")?;
        let flow = kamikaze_window(self, b"flow", profile.flow_block, frame).ok_or("flow")?;
        if !live_child(self, flow, 0, 34) || !live_child(self, receiver, flow, 0) {
            return Err("task_chain");
        }
        let runtime = services.tasks.fes.runtime(shell.owner).ok_or("body_window")?;
        let driver = runtime.driver.as_ref().ok_or("body_driver")?;
        if driver.initialization_pending() || profile.sample != Sample::Newton
            && !runtime.media.get(profile.prompt).is_some_and(|layer| layer.visible && layer.opacity != 0)
        {
            return Err("wait_prompt");
        }
        let resources = self.resources();
        let winner = |category: &[u8], member: &str, expected: &str| {
            let candidate = resources.category_names(category, member).ok()?.iter()
                .find_map(|name| resources.find(name))?;
            let bytes = resources.read_candidate(candidate).ok()?;
            (sha256(&bytes) == expected).then_some(bytes)
        };
        if profile.sample == Sample::Happiness {
            let main = winner(b"kcs", "main.kcs",
                "d0c55d768b149cb0a63d4f69ace7184f0a6e0d89dda01e63b536076e86bb207a")
                .and_then(|bytes| KcsProgram::parse(&bytes).ok()).ok_or("main_source")?;
            if Some(&main) != self.vm.as_ref().and_then(|vm| vm.program()) {
                return Err("main_program");
            }
        }
        let kcs = winner(b"kcs", "sscript.kcs", profile.sscript)
            .and_then(|bytes| KcsProgram::parse(&bytes).ok()).ok_or("sscript_source")?;
        let fes_digest = if profile.sample == Sample::Newton && shell.window == "meswnd" {
            crate::fes_tasks::NEWTON_MESSAGE_FES.1
        } else {
            profile.fes
        };
        let fes = winner(b"fes", &format!("{}.fes", shell.window), fes_digest)
            .and_then(|bytes| cs2_script::FesDocument::parse(&bytes).ok())
            .and_then(|document| FesProgram::from_document(&document).ok()).ok_or("body_window_source")?;
        if Some(&kcs) != self.task_vms.get(&receiver).and_then(|vm| vm.program())
            || fes != driver.interpreter.program
        {
            return Err("live_program");
        }
        let load = sources.first().ok_or("body_sources")?.load_id;
        Ok(((region, receiver), (shell.owner, shell.context, load),
            (generation.source_epoch, generation.source_frame, shell.marked)))
    }

    pub fn host_yukikoi_save_control(
        &mut self, anchor: (u32, u32, u64, u64), stage: u8,
    ) -> anyhow::Result<Option<([i32; 2], u32, u32)>> {
        let (body_owner, context, load, epoch) = anchor;
        let frame = self.state.clock.frame;
        if self.host_story_pe_sha256() != Some(YUKIKOI_PE)
            || quiet_root_at(self, frame, 0xea8).is_none()
            || !self.host_neko_slot_zero_absent()
        {
            return Ok(None);
        }
        let retained = self.cst_contexts.get(&context).is_some_and(|context| {
            context.story.is_scene("__tar01")
                && context.story.settled_body_record(frame).map(|(_, group, record)| (group, record)) == Some((2, 70))
        });
        let sources = self.data_services.fes_text.story_body_sources(body_owner, epoch, &self.data_services.cst_text_origins);
        if !retained || !matches!(sources, Some([source])
            if (source.context_handle, source.load_id, source.group, source.absolute_record) == (context, load, 2, 69))
        {
            return Ok(None);
        }
        let snapshot = self.shared_numeric_state_snapshot()?;
        let root = if snapshot.active_identity == 0 { Some(&snapshot.active_values) } else { snapshot.inactive_values.get(&0) };
        let Some(root) = root else { return Ok(None); };
        if root.get(900) != Some(&0) || root.get(801) != Some(&0) {
            return Ok(None);
        }

        let (owner, name, bank) = match stage {
            0 | 3 => {
                if root.get(531) != Some(&1)
                    || self.host_replay_legacy_fes_body(0xea8, 2, 70)
                        .is_none_or(|(_, lifetime, body)| lifetime != (body_owner, context, load) || body.0 != epoch)
                {
                    return Ok(None);
                }
                (body_owner, if stage == 0 { "btn_menu" } else { "btn_menuopen" }, if stage == 0 { 8 } else { 0 })
            }
            1 => {
                let Some(owner) = self.legacy_save_window("saveload", b"MENU_WAIT_USER",
                    "a9d5b65ad9bede82266127e9cb3918cf22190e42240c5c63da390d05a09c4998")
                else { return Ok(None); };
                if root.get(1030) != Some(&0) {
                    return Ok(None);
                }
                (owner, "btn_menu", 0)
            }
            2 => {
                let Some(owner) = self.legacy_save_window("_saveload", b"WAIT_USER",
                    "fadd24bc6091533d59248c94f7f52a42f51cf4dfc84562c773d35ff109f17921")
                else { return Ok(None); };
                let parent = self.task_parents.get(&owner).copied();
                let nested = self.data_services.tasks.fes.story_runtimes().any(|(id, name, ..)| {
                    Some(id) == parent && name.strip_suffix(b".fes").unwrap_or(name) == b"saveload"
                });
                let locals = self.data_services.tasks.fes.runtime(owner)
                    .and_then(|runtime| runtime.driver.as_ref())
                    .map(|driver| driver.interpreter.local_values());
                if !nested || [800, 802].map(|slot| root.get(slot).copied()) != [Some(0), Some(0)]
                    || root.get(10).is_none_or(|&handle| !self.css_handles.contains_key(&handle))
                    || locals.and_then(|locals| locals.get(10)) != Some(&0)
                {
                    return Ok(None);
                }
                (owner, "btn_data", 0)
            }
            _ => return Ok(None),
        };
        self.legacy_save_button(owner, name, bank)
    }

    pub fn host_happiness_save_control(
        &mut self, anchor: (u32, u32, u64, u64), stage: u8,
    ) -> anyhow::Result<Option<([i32; 2], u32, u32)>> {
        let (owner, context, load, epoch) = anchor;
        let frame = self.state.clock.frame;
        if self.host_story_pe_sha256() != Some(PROFILES[3].pe) || !self.host_neko_slot_zero_absent() {
            return Ok(None);
        }
        let source = self.data_services.fes_text.story_body_sources(owner, epoch, &self.data_services.cst_text_origins);
        if !self.cst_contexts.get(&context).is_some_and(|state| {
            state.story.is_scene("0_01_01")
                && state.story.settled_body_record(frame).map(|(_, group, record)| (group, record)) == Some((46, 657))
        }) || !matches!(source, Some([body])
            if (body.context_handle, body.load_id, body.group, body.absolute_record) == (context, load, 46, 621))
        {
            return Ok(None);
        }
        if stage == 0 {
            // The toolbar hides keywait. Keep the verified source epoch and
            // require the live message window to be back in its control loop.
            let ready = quiet_root_at(self, frame, 0xe0d).is_some()
                && self.data_services.fes_text.story_body_generation(owner)
                    .is_some_and(|generation| generation.source_epoch == epoch)
                && (self.legacy_save_window("meswnd", b"WAIT", PROFILES[3].fes) == Some(owner)
                    || self.legacy_save_window("meswnd", b"MES_SHOW", PROFILES[3].fes) == Some(owner));
            return if ready { self.legacy_save_button(owner, "btn", 6) } else { Ok(None) };
        }
        if stage == 2 {
            if self.host_replay_legacy_fes_body(0xe0d, 46, 657)
                .is_none_or(|(_, lifetime, current)| lifetime != (owner, context, load) || current.0 != epoch)
            {
                return Ok(None);
            }
            let y = self.data_services.tasks.fes.runtime(owner)
                .and_then(|runtime| runtime.driver.as_ref())
                .and_then(|driver| driver.interpreter.local_values().get(95).copied());
            // The original toolbar opens between message origin y+230 and y+260.
            return Ok(y.and_then(|y| y.checked_add(245)).map(|y| ([640, y], 0, owner)));
        }
        let Some(panel) = self.legacy_save_window("saveload", b"WAIT",
            "975cecc6ae1e026954481fcc3d0abc1632e5f238b195af1d4a9f30eec75859a2")
        else { return Ok(None); };
        let locals = self.data_services.tasks.fes.runtime(panel)
            .and_then(|runtime| runtime.driver.as_ref())
            .map(|driver| driver.interpreter.local_values());
        let snapshot = self.shared_numeric_state_snapshot()?;
        let root = if snapshot.active_identity == 0 { Some(&snapshot.active_values) } else { snapshot.inactive_values.get(&0) };
        if stage != 1 || !locals.is_some_and(|loc| loc.get(100) == Some(&0) && loc.get(110) == Some(&0))
            || !root.is_some_and(|root| root.get(800) == Some(&0) && root.get(802) == Some(&1) && root.get(860) == Some(&0))
        {
            return Ok(None);
        }
        self.legacy_save_button(panel, "btdata", 0)
    }

    pub fn host_happiness_save_button_focused(&self, owner: u32, region: u32) -> bool {
        self.data_services.tasks.fes.runtime(owner)
            .and_then(|runtime| runtime.driver.as_ref())
            .is_some_and(|driver| driver.interpreter.current_block() == b"WAIT"
                && !driver.has_pending_handler()
                && driver.focus().is_some_and(|focus| focus.value == region as i32))
    }

    pub fn host_happiness_load_control(
        &mut self, stage: u8,
    ) -> anyhow::Result<Option<([i32; 2], u32, u32)>> {
        let profile = PROFILES[3];
        if self.host_story_pe_sha256() != Some(profile.pe)
            || self.default_entry() != Some("kcs.int/main.kcs") || !self.cst_contexts.is_empty()
        {
            return Ok(None);
        }
        const PANEL_SHA256: &str = "975cecc6ae1e026954481fcc3d0abc1632e5f238b195af1d4a9f30eec75859a2";
        let target = match stage {
            0 | 1 => self.legacy_save_window("title2", b"WAIT",
                "596d1da60523623adeb1f6e6b06c69a19dba2c1843463b4caa6aa104fc212ff5")
                .map(|owner| if stage == 0 { (owner, "btn1", 0) } else { (owner, "btn3", 1) }),
            2 => {
                let Some(owner) = self.legacy_save_window("saveload", b"WAIT", PANEL_SHA256)
                else { return Ok(None); };
                let locals = self.data_services.tasks.fes.runtime(owner)
                    .and_then(|runtime| runtime.driver.as_ref())
                    .map(|driver| driver.interpreter.local_values());
                let snapshot = self.shared_numeric_state_snapshot()?;
                let root = if snapshot.active_identity == 0 { Some(&snapshot.active_values) }
                    else { snapshot.inactive_values.get(&0) };
                (locals.is_some_and(|loc| loc.get(100) == Some(&0) && loc.get(110) == Some(&1))
                    && root.is_some_and(|root| root.get(800) == Some(&1)))
                    .then_some((owner, "btdata", 0))
            }
            3 => {
                let Some(panel) = self.legacy_save_window("saveload", b"CONFIRM_WAIT", PANEL_SHA256)
                else { return Ok(None); };
                self.legacy_save_window("confirm", b"WAIT",
                    "20f2e83b95401cb702913ec095ba05e8fefa9d8e246f87d3f3444405875d208c")
                    .filter(|&owner| live_child(self, owner, panel, 34))
                    .map(|owner| (owner, "btn", 0))
            }
            _ => None,
        };
        match target {
            Some((owner, name, bank)) => self.legacy_save_button(owner, name, bank),
            None => Ok(None),
        }
    }

    fn legacy_save_button(&mut self, owner: u32, name: &str, bank: i32) -> anyhow::Result<Option<([i32; 2], u32, u32)>> {
        let mut bindings = self.input_task.regions.iter().filter(|entry| {
            self.host_neko_save_target(owner, entry.id(), name, bank)
        });
        let Some(entry) = bindings.next() else { return Ok(None); };
        let region = entry.id();
        let [left, top, right, bottom] = entry.bounds();
        if bindings.next().is_some() || !live_child(self, region, owner, 6) || left >= right || top >= bottom {
            return Ok(None);
        }
        let point = [left.midpoint(right), top.midpoint(bottom)];
        let accepted = self.masked_input_region_at(point)?.is_some_and(|hit| hit[0] == region);
        if !accepted || !self.mouse_press_candidate_regions(point)?.contains(&region) {
            return Ok(None);
        }
        Ok(Some((point, region, owner)))
    }

    fn legacy_save_window(&self, member: &str, block: &[u8], digest: &str) -> Option<u32> {
        let owner = kamikaze_window(self, member.as_bytes(), block, self.state.clock.frame)?;
        let runtime = self.data_services.tasks.fes.runtime(owner)?;
        let driver = runtime.driver.as_ref()?;
        let resources = self.resources();
        let filename = format!("{member}.fes");
        let candidate = resources.category_names(b"fes", &filename).ok()?.iter()
            .find_map(|name| resources.find(name))?;
        let data = resources.read_candidate(candidate).ok()?;
        if sha256(&data) != digest {
            return None;
        }
        let document = cs2_script::FesDocument::parse(&data).ok()?;
        (FesProgram::from_document(&document).ok()? == driver.interpreter.program).then_some(owner)
    }

    pub fn host_kamikaze_replay_identity(&self) -> bool {
        let profile = PROFILES[2];
        self.host_story_pe_sha256() == Some(profile.pe)
            && self.default_entry() == Some("kcs.int/main.kcs")
    }

    pub fn host_kamikaze_title_owner(&self) -> Option<u32> {
        let profile = PROFILES[2];
        let frame = self.state.clock.frame;
        self.host_kamikaze_replay_identity().then_some(())?;
        quiet_root_at(self, frame, profile.root_wait)?;
        if !self.cst_contexts.is_empty() {
            return None;
        }
        let flow = kamikaze_window(self, b"flow", b"WAIT_TITLE", frame)?;
        let ttlflow = kamikaze_window(self, b"ttlflow", b"FES_WAIT", frame)?;
        let title = kamikaze_window(self, b"title", b"WAIT", frame)?;
        (self.data_services.tasks.fes.runtimes().count() == 3
            && live_child(self, flow, 0, 34)
            && live_child(self, ttlflow, flow, 34)
            && live_child(self, title, ttlflow, 34))
            .then_some(title)
    }

    pub fn kamikaze_replay_page(group: i32) -> Option<(i32, usize)> {
        let profile = PROFILES[2];
        match group {
            0 => Some((profile.first.wait, profile.first.body)),
            1 => Some((profile.next.wait, profile.next.body)),
            2 => Some((68, 67)),
            _ => profile.body_probe().legacy_continuation_page(group)
                .and_then(|(wait, body)| if let [body] = body { Some((wait, *body)) } else { None }),
        }
    }

    // The result carries independently resolved owners, the active CST load,
    // the absolute wait record, and the completed body generation.
    pub fn host_kamikaze_body(&self) -> Option<(u32, u32, u32, u32, u64, i32, i32, u64, u64, u64)> {
        let profile = PROFILES[2];
        let frame = self.state.clock.frame;
        self.host_kamikaze_replay_identity().then_some(())?;
        quiet_root_at(self, frame, profile.root_wait)?;
        let services = &self.data_services;
        let input = &services.messages;
        if !enter_mapping(self.device_key_bindings(), &self.input_task.key_enabled)
            || input.typed_physical_owners.contains_key(&(ButtonSource::Keyboard, 13))
            || input.key_capture.contains_key(&0) || input.key_capture.contains_key(&2)
            || input.device_keys.forced_queries(0).ok()? != 0
            || input.device_keys.forced_queries(2).ok()? != 0
        {
            return None;
        }

        let mut contexts = self.cst_contexts.iter().filter(|(_, context)| context.document.is_some());
        let (&handle, context) = contexts.next()?;
        let story = &context.story;
        if contexts.next().is_some() || context.localization.is_some() || story.scene != profile.scene {
            return None;
        }
        let (marked, group, record) = story.settled_body_record(frame)?;
        let page = observed_body_page(profile, group, record)?;
        let document = context.document.as_ref()?;
        let flow = kamikaze_window(self, b"flow", profile.flow_block, frame)?;
        let owner = kamikaze_window(self, b"meswnd", b"MES_SHOW", frame)?;
        let receiver = *self.task_parents.get(&owner)?;
        let tasks = &services.tasks;
        if tasks.fes.runtimes().count() != 2
            || !live_child(self, flow, 0, 34)
            || !live_child(self, receiver, flow, 0)
            || !live_child(self, owner, receiver, 34)
            || !tasks.task_clock(receiver).is_some_and(|clock| clock.paused == 0)
            || !body_and_prompt_submitted(self, owner)
        {
            return None;
        }
        let (_, _, runtime, named_wait_key) = tasks.fes.story_runtimes().find(|(id, ..)| *id == owner)?;
        let driver = runtime.driver.as_ref()?;
        let locals = driver.interpreter.local_values();
        let prompt = runtime.media.get(profile.prompt)?;
        if named_wait_key || locals.get(100) != Some(&1) || !prompt.visible || prompt.opacity == 0 {
            return None;
        }
        let generation = services.fes_text.story_body_generation(owner)?;
        let (_, completed) = generation.natural_complete?;
        let origins = &services.cst_text_origins;
        let sources @ [source] = services.fes_text.story_body_sources(owner, generation.source_epoch, origins)? else {
            return None;
        };
        let shell = BodyWaitShell {
            window: "meswnd", owner, scene: profile.scene, context: handle,
            loaded: story.loaded, marked, group, record,
        };
        if source.absolute_record != page.body
            || !body_sources_match(document, shell, Some(sources), None)
            || !current_committed_body(generation, story.loaded, marked, frame)
            || !origins.source_record_observations(handle).iter().any(|entry| {
                entry.probe == CstSourceProbe::Kamikaze && entry.source == *source && entry.body_tagged
                    && entry.native == 327 && !entry.localized && entry.stored_codec == ScriptTextEncoding::Cp932
                    && entry.output_codec == ScriptTextEncoding::Cp932
                    && entry.raw_bytes != 0 && entry.raw_bytes == entry.output_bytes
                    && (story.loaded..=generation.source_frame).contains(&entry.frame)
            })
        {
            return None;
        }

        let mut buttons = self.scene_nodes.iter().filter(|(_, node)| node.name == "bottom button");
        let (&region, _) = buttons.next()?;
        let button = input.input_buttons.get(&region)?;
        let mut registrations = self.input_task.regions.iter().filter(|entry| entry.id() == region);
        let registration = registrations.next()?;
        if buttons.next().is_some() || registrations.next().is_some()
            || !live_child(self, region, receiver, 6)
            || !bottom_registration(&registration.words, &button.state, region, receiver, button.execution_checkpoint().3)
            || input.input_masks.contains_key(&region)
        {
            return None;
        }

        // Check the programs running on the live tasks against the resource
        // winners before accepting the body. This is reached once per page.
        let resources = self.resources();
        let read_winner = |category: &[u8], member: &str, expected: &str| {
            let candidate = resources.category_names(category, member).ok()?.iter()
                .find_map(|name| resources.find(name))?;
            let bytes = resources.read_candidate(candidate).ok()?;
            (sha256(&bytes) == expected).then_some(bytes)
        };
        let kcs = read_winner(b"kcs", "sscript.kcs", profile.sscript)?;
        let fes = read_winner(b"fes", "meswnd.fes", profile.fes)?;
        let fes = cs2_script::FesDocument::parse(&fes).ok()?;
        if &KcsProgram::parse(&kcs).ok()? != self.task_vms.get(&receiver)?.program()?
            || FesProgram::from_document(&fes).ok()? != driver.interpreter.program
        {
            return None;
        }
        Some((owner, region, receiver, handle, source.load_id, group, record,
            generation.source_epoch, generation.source_frame, completed))
    }
}

fn kamikaze_window(player: &HeadlessPlayer, name: &[u8], block: &[u8], frame: u64) -> Option<u32> {
    let mut windows = player.data_services.tasks.fes.story_runtimes().filter(|(_, actual, ..)| {
        actual.strip_suffix(b".fes").unwrap_or(actual).eq_ignore_ascii_case(name)
    });
    let (owner, _, runtime, _) = windows.next()?;
    let driver = runtime.driver.as_ref()?;
    (windows.next().is_none() && settled_poll(driver.interpreter.current_block(), block,
        driver.initialization_pending() || driver.has_pending_handler(), driver.waiting(), frame))
        .then_some(owner)
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
    fn newton_localized_executable_uses_the_same_verified_body_profile() {
        let startup = "kcs.int/main.kcs";
        let profile = legacy_body_profile(NEWTON_PE, Some(startup)).unwrap();
        assert_eq!(profile.sample, Sample::Newton);
        assert_eq!(legacy_body_profile(NEWTON_LOCALIZED_PE, Some(startup)).unwrap().sample, Sample::Newton);
        assert!(verified_profile(NEWTON_LOCALIZED_PE, startup, profile.sscript, profile.fes, profile.cst).is_some());
        assert!(verified_profile(NEWTON_LOCALIZED_PE, startup, profile.sscript, profile.fes, "other").is_none());
        assert!(legacy_body_profile(NEWTON_LOCALIZED_PE, Some("main.kcs")).is_none());
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

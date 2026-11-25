use super::{
    Action, Decision, KeyboardPress, KeyboardPressReceipt, MousePress, NovelWindowLifetime,
    consuming_mouse_resume_in_scan,
};
use anyhow::ensure;
use cs2_desktop::{HeadlessPlayer, LogicEvent};
use cs2_runtime::InputKey;
use serde::Deserialize;

#[path = "rebellions_ordinary.rs"]
mod ordinary;

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Mode {
    EnterOnce,
    ObserveR26,
    OpeningSegment,
    ObserveR13,
    ObserveR61,
    ObserveR189,
    GuardNovelR211,
    GuardNovelR215,
    GuardRestoredR301,
    GuardRestoredR610,
    GuardSecondR93,
    GuardSecondR583,
    GuardThirdR97,
    SaveR61,
    LoadR61,
    ResumeR61Third,
}

const OPENING: &str = "A_01_01";
const SECOND: &str = "A_01_02";
const THIRD: &str = "A_01_03";

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub(super) enum Config {
    Preset(Mode),
    Campaign(Campaign),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Campaign {
    schema: String,
    scenes: Vec<Scene>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Scene {
    scene: String,
    sha256: String,
    waits: Vec<Page>,
    boundary: Boundary,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Boundary {
    Next { record: i32, scene: String },
    ObserveFlow { record: i32, commands: Vec<(i32, String)> },
    ObserveEnd { record: i32 },
}

impl Boundary {
    fn record(&self) -> i32 {
        match self {
            Self::Next { record, .. } | Self::ObserveFlow { record, .. } | Self::ObserveEnd { record } => *record,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Page {
    group: i32,
    record: i32,
    kind: u32,
    window: Window,
    bodies: Vec<usize>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Window { Message, Novel }

impl Campaign {
    fn validate(&self) -> anyhow::Result<()> {
        ensure!(self.schema == "cs2-rebellions-story-plan/v1" && (1..=35).contains(&self.scenes.len()),
            "unsupported Rebellions campaign schema or scene count");
        ensure!(self.scenes[0].scene == OPENING && self.scenes[0].waits.get(12).is_some_and(|page|
            (page.record, page.kind, page.window, page.bodies.as_slice()) == (61, 2, Window::Message, &[60])),
            "Rebellions campaign does not start from the verified ordinary slot");
        for (index, scene) in self.scenes.iter().enumerate() {
            ensure!(HeadlessPlayer::host_rebellions_planned_scene(&scene.scene, &scene.sha256)
                && !self.scenes[..index].iter().any(|previous| previous.scene == scene.scene),
                "unverified or duplicate Rebellions scene {}", scene.scene);
            let mut previous = -1;
            for (group, page) in scene.waits.iter().enumerate() {
                ensure!(page.group == group as i32 && (previous + 1..scene.boundary.record()).contains(&page.record)
                    && (page.kind == 2 || page.kind == 3 && page.window == Window::Novel)
                    && page.bodies.first().is_some_and(|first| *first as i64 > i64::from(previous))
                    && page.bodies.last().is_some_and(|last| *last < page.record as usize)
                    && page.bodies.windows(2).all(|pair| pair[0] < pair[1]),
                    "invalid Rebellions page {} r{}", scene.scene, page.record);
                previous = page.record;
            }
            ensure!(previous >= 0, "Rebellions scene {} has no pages", scene.scene);
            match (&scene.boundary, self.scenes.get(index + 1)) {
                (Boundary::Next { scene: destination, .. }, Some(next)) if destination == &next.scene => {}
                (Boundary::ObserveFlow { record, commands }, None)
                    if commands.first().is_some_and(|(index, _)| index == record) => {}
                (Boundary::ObserveEnd { .. }, None) => {}
                _ => anyhow::bail!("invalid Rebellions successor for {}", scene.scene),
            }
        }
        Ok(())
    }

    fn next(&self, before: Body) -> Option<(&Scene, &Page, bool)> {
        let index = self.scenes.iter().position(|scene| scene.scene == before.scene)?;
        let scene = &self.scenes[index];
        let group = usize::try_from(before.position.0).ok()?;
        let current = scene.waits.get(group)?;
        let (next_scene, next) = if let Some(next) = scene.waits.get(group + 1) {
            (scene, next)
        } else {
            let next_scene = self.scenes.get(index + 1)?;
            (next_scene, next_scene.waits.first()?)
        };
        Some((next_scene, next, current.window != next.window))
    }
}

impl Mode {
    fn terminal(self) -> (&'static str, i32) {
        match self {
            Self::ObserveR13 => (OPENING, 0),
            Self::EnterOnce => (OPENING, 1),
            Self::ObserveR26 => (OPENING, 2),
            Self::OpeningSegment => (OPENING, 10),
            Self::ObserveR61 => (OPENING, 12),
            Self::ObserveR189 => (OPENING, 26),
            Self::GuardNovelR211 => (OPENING, 27),
            Self::GuardNovelR215 => (OPENING, 29),
            Self::GuardRestoredR301 => (OPENING, 47),
            Self::GuardRestoredR610 => (OPENING, 122),
            Self::GuardSecondR93 => (SECOND, 2),
            Self::GuardSecondR583 => (SECOND, 88),
            Self::GuardThirdR97 => (THIRD, 2),
            Self::SaveR61 => (OPENING, 12),
            Self::LoadR61 => (OPENING, 13),
            Self::ResumeR61Third => (THIRD, 2),
        }
    }
}

struct Entry {
    screen: &'static str,
    point: [i32; 2],
    region: u32,
    press_path: &'static [u32],
    button: &'static [u8],
    bank: i32,
    forward: Option<u32>,
}

const ENTRY: [Entry; 3] = [
    Entry { screen: "title", point: [480, 667], region: 16, press_path: &[3, 16],
        button: b"btn1", bank: 0, forward: Some(3) },
    Entry { screen: "epselect", point: [353, 308], region: 14, press_path: &[3, 14],
        button: b"epbtn", bank: 0, forward: Some(3) },
    Entry { screen: "confirm2", point: [723, 418], region: 15, press_path: &[15],
        button: b"btn", bank: 1, forward: None },
];

#[derive(Clone, Copy)]
struct Body {
    scene: &'static str,
    position: (i32, i32),
    tasks: (u32, u32, u32),
    context: u32,
    load: u64,
    epoch: u64,
    source_frame: u64,
}

impl Body {
    fn observe(player: &HeadlessPlayer) -> Option<Self> {
        player.host_rebellions_body().map(Self::from_receipt)
    }

    fn planned(player: &HeadlessPlayer, scene: &Scene, page: &Page) -> Option<Self> {
        player.host_rebellions_planned_body((&scene.scene, page.group, page.record), page.kind,
            page.window == Window::Novel, &page.bodies).map(Self::from_receipt)
    }

    fn from_receipt(receipt: (&'static str, (i32, i32), (u32, u32, u32), (u32, u64, u64, u64, u64))) -> Self {
        let (scene, position, tasks, (context, load, epoch, source_frame, _)) = receipt;
        Self { scene, position, tasks, context, load, epoch, source_frame }
    }

    fn next(self) -> Option<(&'static str, i32, i32)> {
        let group = self.position.0 + 1;
        let waits = HeadlessPlayer::host_rebellions_scene_waits(self.scene);
        if let Some(&record) = waits.get(group as usize) {
            return Some((self.scene, group, record));
        }
        if group as usize != waits.len() {
            return None;
        }
        let scene = match self.scene {
            OPENING => SECOND,
            SECOND => THIRD,
            _ => return None,
        };
        Some((scene, 0, HeadlessPlayer::host_rebellions_scene_waits(scene)[0]))
    }

    fn log(self, player: &HeadlessPlayer, frame: u64) {
        let (group, record) = self.position;
        let (owner, region, receiver) = self.tasks;
        println!("replay frame={frame} state=page scene={} group={group} record={record}", self.scene);
        eprintln!("frame {frame}: Rebellions body owner={owner} region={region} receiver={receiver}");
        if let Some(wait) = player.host_rebellions_body_receipt(group, owner, self.context) {
            let (context, load, epoch, source_frame) = (self.context, self.load, self.epoch, self.source_frame);
            eprintln!("frame {frame}: Rebellions guarded body context={context} load={load} epoch={epoch} source_frame={source_frame} {wait}");
        }
        eprintln!("frame {frame}:\n{}", player.host_composition_diagnostic());
        eprintln!("frame {frame}: wait {}", player.host_replay_wait_diagnostics());
    }
}

enum Phase {
    Entry(usize),
    Hover { index: usize, owner: u32 },
    Click { index: usize, owner: u32, started: u64 },
    FirstPage,
    Enter {
        before: Body,
        started: u64,
        receipt: KeyboardPressReceipt,
        lifetimes: [NovelWindowLifetime; 3],
    },
    Failed(&'static str),
}

pub(super) struct RebellionsRoute {
    mode: Mode,
    campaign: Option<Campaign>,
    phase: Phase,
    since: u64,
    ordinary: Option<ordinary::Ordinary>,
}

impl RebellionsRoute {
    pub(super) fn new(config: Config) -> anyhow::Result<Self> {
        let (mode, campaign) = match config {
            Config::Preset(mode) => (mode, None),
            Config::Campaign(campaign) => {
                campaign.validate()?;
                (Mode::ResumeR61Third, Some(campaign))
            }
        };
        let ordinary = matches!(mode, Mode::LoadR61 | Mode::ResumeR61Third).then(ordinary::Ordinary::load);
        Ok(Self { mode, campaign, phase: Phase::Entry(0), since: 0, ordinary })
    }

    pub(super) fn observe_lifecycle_events(&mut self, events: &[LogicEvent], batch_start: usize) {
        if let Phase::Enter { before, lifetimes, .. } = &mut self.phase {
            let (owner, region, receiver) = before.tasks;
            for (lifetime, id) in lifetimes.iter_mut().zip([owner, region, receiver]) {
                lifetime.observe(id, events, batch_start);
            }
        }
    }

    pub(super) fn observe_frame(&mut self, player: &HeadlessPlayer) {
        if let Some(ordinary) = &mut self.ordinary {
            ordinary.observe(player);
            return;
        }
        let frame = player.host_story_trace().0;
        match &mut self.phase {
            Phase::Click { index, owner, started } if frame > *started => {
                let expected_scan = *started + 1;
                let Some(scan) = player.host_input_route_scan_frame() else { return; };
                if scan != expected_scan {
                    if scan > expected_scan {
                        self.phase = Phase::Failed("Rebellions input scan ended without the expected click receipt and root resume");
                    }
                    return;
                }
                let entry = &ENTRY[*index];
                let expected = MousePress {
                    candidates: entry.press_path.to_vec(), region: entry.region, key: 0, group: 0, response: 0,
                };
                if root_wait(player, frame)
                    && consuming_mouse_resume_in_scan(player, &expected, scan, frame, *owner, entry.point, entry.forward)
                {
                    eprintln!("frame {frame}: verified Rebellions {} click region={} receiver={} and VM resume (scan opened at {scan})",
                        entry.screen, entry.region, owner);
                    self.phase = if *index + 1 == ENTRY.len() { Phase::FirstPage } else { Phase::Entry(*index + 1) };
                    self.since = frame;
                }
            }
            Phase::Enter { before, started, receipt, .. } => {
                let (_, region, receiver) = before.tasks;
                let expected = KeyboardPress { region, receiver, key: 2, group: 0, response: 0 };
                let verified = receipt.observe(&expected, *started, player.host_input_route_receipts(), player.events());
                let consumers = player.host_input_route_receipts().iter().filter(|item|
                    item.frame == frame && item.pressed != 0 && item.response == 0).count();
                if frame == *started + 1 {
                    if let Some(hit) = verified.filter(|_| consumers == 1 && root_wait(player, frame)) {
                        eprintln!("frame {frame}: verified Rebellions Enter and root resume: {hit:?}");
                    } else {
                        self.phase = Phase::Failed("Rebellions Enter did not produce the expected consume and root resume");
                    }
                } else if *receipt == KeyboardPressReceipt::Rejected
                    || !matches!(self.mode, Mode::EnterOnce | Mode::ObserveR26) && consumers != 0
                {
                    self.phase = Phase::Failed("Rebellions saw extra input consumption after Enter");
                }
            }
            _ => {}
        }
    }

    pub(super) fn poll(&mut self, player: &mut HeadlessPlayer, frame: u64, idle: u64) -> anyhow::Result<Decision> {
        if let Some(ordinary) = &mut self.ordinary {
            if let Some(page) = ordinary.restored(player)? {
                page.log(player, frame);
                self.ordinary = None;
                return Ok(self.press(player, frame, page));
            }
            return ordinary.poll(player, frame, idle);
        }
        match &self.phase {
            Phase::Entry(index) => {
                let entry = &ENTRY[*index];
                if let Some(owner) = player.host_rebellions_entry_owner(entry.screen) {
                    self.phase = Phase::Hover { index: *index, owner };
                    self.since = frame;
                    let [x, y] = entry.point;
                    return Ok(Decision::Input(Action::Pointer { x, y, button: None }));
                }
            }
            Phase::Hover { index, owner } => {
                let entry = &ENTRY[*index];
                let Some(live_owner) = player.host_rebellions_entry_owner(entry.screen) else {
                    return Ok(self.wait(player, frame, idle));
                };
                let hit = player.masked_input_region_at(entry.point)?;
                let candidates = player.mouse_press_candidate_regions(entry.point)?;
                let bound = player.host_rebellions_button_bound(live_owner, entry.region, entry.button, entry.bank);
                eprintln!("frame {frame}: Rebellions {} hover owner={live_owner} hit={hit:?} candidates={candidates:?} bound={bound}", entry.screen);
                if live_owner != *owner || player.host_pointer_logical_position() != entry.point
                    || !hit.is_some_and(|words| words[0] == entry.region && words[8] != 0)
                    // The verified consumer ends dispatch before any later candidate.
                    || !candidates.starts_with(entry.press_path) || !bound
                {
                    return Ok(Decision::Stop("checkpoint_mismatch", format!("Rebellions {} live target changed", entry.screen)));
                }
                self.phase = Phase::Click { index: *index, owner: *owner, started: frame };
                let [x, y] = entry.point;
                return Ok(Decision::Input(Action::Pointer { x, y, button: Some(0) }));
            }
            Phase::Click { started, .. } => {
                return Ok(if frame - *started < idle { Decision::Wait } else {
                    Decision::Stop("input_missed", "Rebellions input scan did not confirm the click before the idle limit".into())
                });
            }
            Phase::FirstPage => {
                let Some(page) = Body::observe(player) else { return Ok(self.wait(player, frame, idle)); };
                if page.scene != OPENING || page.position != (0, 13) {
                    return Ok(Decision::Stop("checkpoint_mismatch", format!("expected Rebellions g0/r13, got {:?}", page.position)));
                }
                page.log(player, frame);
                if self.mode.terminal() == (page.scene, 0) {
                    return Ok(Decision::Done);
                }
                return Ok(self.press(player, frame, page));
            }
            Phase::Enter { before, started, receipt, lifetimes } => {
                // The outer replay loop calls poll only after the device scan observes release.
                if *receipt != KeyboardPressReceipt::Verified {
                    return Ok(Decision::Stop("input_missed", "Rebellions Enter receipt frame missing".into()));
                }
                let (scene, group, record, switches_window, after) = if let Some(campaign) = &self.campaign {
                    let Some((scene, page, switches_window)) = campaign.next(*before) else {
                        if let Some(observed) = player.host_rebellions_departure(before.scene, *started) {
                            return Ok(Decision::Stop("unrecognized_wait", format!("Rebellions observed campaign boundary: {observed}")));
                        }
                        return Ok(self.wait(player, frame, idle));
                    };
                    (scene.scene.as_str(), page.group, page.record, switches_window, Body::planned(player, scene, page))
                } else {
                    let Some((scene, group, record)) = before.next() else {
                        return Ok(Decision::Stop("unrecognized_wait", "Rebellions route has no registered successor".into()));
                    };
                    (scene, group, record, scene == OPENING && matches!(group, 27 | 47), Body::observe(player))
                };
                let switches_scene = scene != before.scene;
                if lifetimes[1..] != [NovelWindowLifetime::Continuous; 2]
                    || if switches_window || switches_scene {
                        !matches!(lifetimes[0], NovelWindowLifetime::Continuous | NovelWindowLifetime::Recreated { .. })
                    } else {
                        lifetimes[0] != NovelWindowLifetime::Continuous
                    }
                {
                    return Ok(Decision::Stop("checkpoint_mismatch", format!("Rebellions task lifetime changed: {lifetimes:?}")));
                }
                let Some(after) = after else { return Ok(self.wait(player, frame, idle)); };
                if after.scene == before.scene && after.position == before.position && after.tasks == before.tasks
                    && (after.context, after.load, after.epoch) == (before.context, before.load, before.epoch)
                {
                    return Ok(self.wait(player, frame, idle));
                }
                let same_receiver = (after.tasks.1, after.tasks.2) == (before.tasks.1, before.tasks.2);
                let window_matches = if switches_window {
                    before.tasks.0 != after.tasks.0 || matches!(lifetimes[0], NovelWindowLifetime::Recreated { .. })
                } else if switches_scene && matches!(lifetimes[0], NovelWindowLifetime::Recreated { .. }) {
                    true
                } else {
                    before.tasks.0 == after.tasks.0 && after.epoch > before.epoch
                };
                // sscript can read adjacent CST bodies at load time. The live
                // body gate ties that read to the newly committed FES source.
                let load_matches = if switches_scene {
                    after.load > before.load
                } else {
                    (after.context, after.load) == (before.context, before.load)
                };
                if after.scene != scene || after.position != (group, record)
                    || !same_receiver || !window_matches || !load_matches
                    || after.source_frame <= *started
                {
                    return Ok(Decision::Stop("checkpoint_mismatch", "Rebellions successor identity or generation changed".into()));
                }
                eprintln!("frame {frame}: Rebellions Enter release observed");
                after.log(player, frame);
                if self.campaign.is_none() && (scene, group) == self.mode.terminal() {
                    if matches!(self.mode, Mode::SaveR61) {
                        return self.ordinary.insert(ordinary::Ordinary::save(after, frame)).poll(player, frame, idle);
                    }
                    return Ok(Decision::Done);
                }
                return Ok(self.press(player, frame, after));
            }
            Phase::Failed(reason) => return Ok(Decision::Stop("input_missed", (*reason).into())),
        }
        Ok(self.wait(player, frame, idle))
    }

    fn press(&mut self, player: &HeadlessPlayer, frame: u64, before: Body) -> Decision {
        let lifetime = NovelWindowLifetime::Armed { first_event: player.events().len() };
        self.phase = Phase::Enter { before, started: frame,
            receipt: KeyboardPressReceipt::Awaiting, lifetimes: [lifetime; 3] };
        self.since = frame;
        Decision::Input(Action::Key(InputKey::Keyboard(13)))
    }

    fn wait(&self, player: &HeadlessPlayer, frame: u64, idle: u64) -> Decision {
        if frame - self.since < idle {
            return Decision::Wait;
        }
        let state = match &self.phase {
            Phase::Entry(index) | Phase::Hover { index, .. } => ENTRY[*index].screen.into(),
            Phase::FirstPage => "g0/r13".into(),
            Phase::Enter { before, .. } => if let Some(campaign) = &self.campaign {
                campaign.next(*before).map_or_else(|| "campaign boundary".into(),
                    |(scene, page, _)| format!("{} g{}/r{}", scene.scene, page.group, page.record))
            } else {
                before.next().map_or("unknown successor".into(),
                    |(scene, group, record)| format!("{scene} g{group}/r{record}"))
            },
            _ => "entry receipt".into(),
        };
        if let Some(diagnostic) = player.host_rebellions_novel_diagnostic() {
            eprintln!("{diagnostic}");
            eprintln!("frame {frame}:\n{}", player.host_composition_diagnostic());
        }
        Decision::Stop("unrecognized_wait", format!("Rebellions {state} did not settle"))
    }
}

fn root_wait(player: &HeadlessPlayer, frame: u64) -> bool {
    player.events().iter().rev().take_while(|event| event.frame >= frame).any(|event|
        event.frame == frame && event.kind == "input-wait" && event.detail == "native 71 address=0xad0")
}

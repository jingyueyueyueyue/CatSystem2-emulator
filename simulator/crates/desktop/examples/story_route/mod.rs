use super::Action;
use anyhow::{Context, Result, ensure};
use cs2_desktop::{HeadlessPlayer, LogicEvent};
use cs2_runtime::InputKey;
use cs2_runtime::message_object::{ButtonSource, InputRouteReceipt, RoutedPhysicalButton};
use serde::Deserialize;
use std::collections::BTreeSet;
use std::path::Path;

type StoryWindow = (&'static str, u32);
type IslandCstAnchor = (u32, u64, u32);

const ISLAND_MESWND_POINTS: [[i32; 2]; 2] = [[557, 530], [557, 324]];

const DAYDREAM_PROLOGUE_WAITS: [i32; 20] = [
    18, 22, 25, 28, 31, 33, 35, 37, 40, 44, 47, 55, 57, 62, 64, 75, 77, 79, 81, 83,
];

fn is_neko_runtime(player: &HeadlessPlayer) -> bool {
    player.default_entry() == Some("kcs.int/main.kcs")
        && player.scan().engine.primary_executable.as_ref().is_some_and(|binary| {
            binary.sha256 == "c71c5b85e6eca660474f156d96b0021a53a5f05690dd077d7861d72ff5b11adc"
        })
}

fn is_daydream_runtime(player: &HeadlessPlayer) -> bool {
    player.default_entry() == Some("kcs.int/main.kcs")
        && player.scan().engine.primary_executable.as_ref().is_some_and(|binary| {
            binary.sha256 == "1f3288d5fd5408bee4d395faab708aca78c9eeaa6c172dc06dcce09c844ba9d5"
        })
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Position {
    state: &'static str,
    scene: String,
    group: i32,
    record: i32,
}

impl Position {
    fn observe(player: &HeadlessPlayer) -> (Self, Option<StoryWindow>) {
        let ((_, state, scene, group, record), window) = player.host_replay_story_position();
        if let Some(menu) = player.host_replay_menu_ready() {
            return (Self { state: menu, scene: String::new(), group: -1, record: -1 }, None);
        }
        (Self { state, scene: scene.to_owned(), group, record }, window)
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Hit {
    point: [i32; 2],
    region: u32,
}

impl Hit {
    fn matches(&self, player: &HeadlessPlayer) -> Result<bool> {
        Ok(player.masked_input_region_at(self.point)?
            .is_some_and(|region| region[0] == self.region && region[8] != 0))
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Condition {
    state: String,
    scene: Option<String>,
    group: Option<i32>,
    record: Option<i32>,
    window: Option<String>,
    hit: Option<Hit>,
}

impl Condition {
    fn is_neko_startup_screen(&self, state: &str, scene: &str, window: &str) -> bool {
        self.state == state && self.scene.as_deref() == Some(scene) && self.window.as_deref() == Some(window)
            && self.group.is_none() && self.record.is_none()
    }

    fn neko_opening_page(&self) -> Option<usize> {
        if self.state != "page" || self.scene.as_deref() != Some("01") || self.window.as_deref() != Some("meswnd") {
            return None;
        }
        const OPENING_WAITS: [i32; 12] = [59, 87, 92, 98, 103, 112, 117, 122, 127, 136, 141, 146];
        let group = usize::try_from(self.group?).ok()?;
        (OPENING_WAITS.get(group).copied()? == self.record?).then_some(group)
    }

    fn daydream_opening_page(&self) -> Option<usize> {
        if self.state != "page" || self.scene.as_deref() != Some("prologue") || self.window.as_deref() != Some("cinema") {
            return None;
        }
        let group = usize::try_from(self.group?).ok()?;
        (DAYDREAM_PROLOGUE_WAITS.get(group).copied()? == self.record?).then_some(group)
    }

    fn island_meswnd_page(&self) -> Option<(i32, i32)> {
        let (group, record) = (self.group?, self.record?);
        (self.state == "page" && self.scene.as_deref() == Some("com01")
            && self.window.as_deref() == Some("meswnd") && (53..=121).contains(&group)
            && (514..=810).contains(&record) && (group != 53 || record == 514)
            && (group != 121 || record == 810)).then_some((group, record))
    }

    fn has_exact_record(&self) -> bool {
        self.scene.as_ref().is_some_and(|scene| !scene.is_empty())
            && self.group.is_some_and(|group| group >= 0)
            && self.record.is_some_and(|record| record >= 0)
    }

    fn same_record(&self, other: &Self) -> bool {
        (self.scene.as_deref(), self.group, self.record)
            == (other.scene.as_deref(), other.group, other.record)
    }

    fn matches_position(&self, position: &Position, window: Option<StoryWindow>) -> bool {
        (self.state == position.state
            || self.state == "page_or_prompt" && matches!(position.state, "page" | "page_prompt"))
            && self.scene.as_ref().is_none_or(|scene| scene == &position.scene)
            && self.group.is_none_or(|group| group == position.group)
            && self.record.is_none_or(|record| record == position.record)
            && self.window.as_deref().is_none_or(|expected| window.is_some_and(|(name, _)| name == expected))
    }

    fn matches(&self, position: &Position, window: Option<StoryWindow>, player: &HeadlessPlayer) -> Result<bool> {
        if !self.matches_position(position, window) {
            return Ok(false);
        }
        if let Some(hit) = &self.hit {
            return hit.matches(player);
        }
        Ok(true)
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Step {
    label: String,
    wait: Condition,
    click: Option<[i32; 2]>,
    mouse_press: Option<MousePress>,
    keyboard_press: Option<KeyboardPress>,
    neko_startup_language: Option<String>,
    key: Option<u32>,
    after: Option<Condition>,
    #[serde(default)]
    same_novel_window: bool,
    #[serde(default)]
    same_full_window: bool,
    #[serde(default)]
    continue_same_record_prompt: bool,
    #[serde(default)]
    capture: bool,
    #[serde(default)]
    probe_points: Vec<[i32; 2]>,
    #[serde(default)]
    shared_flags: Vec<SharedFlag>,
}

impl Step {
    fn is_daydream_enter(&self) -> bool {
        let (Some(page), Some(after), Some(press)) = (self.wait.daydream_opening_page(), &self.after, &self.keyboard_press) else {
            return false;
        };
        self.key == Some(13) && self.click.is_none() && self.mouse_press.is_none()
            && self.wait.hit.is_none() && after.hit.is_none()
            && after.daydream_opening_page() == Some(page + 1)
            && (press.region, press.receiver, press.key, press.group, press.response) == (62, 8, 2, 0, 0)
    }

    fn is_neko_startup_press(&self) -> bool {
        let (Some(after), Some(press), Some(language)) = (&self.after, &self.mouse_press, self.neko_startup_language.as_deref()) else {
            return false;
        };
        if !matches!(language, "jp" | "cn") || self.key.is_some() || after.hit.is_some()
            || (press.key, press.group, press.response) != (0, 0, 0)
        {
            return false;
        }
        let (point, region, candidates) = if self.wait.is_neko_startup_screen("language_select", "langselect", "langselect")
            && after.is_neko_startup_screen("title", "-", "title")
        {
            ([784, 405], 27, [3, 27, 28, 11, 7])
        } else if self.wait.is_neko_startup_screen("title", "-", "title") && after.neko_opening_page() == Some(0) {
            ([1155, 235], 14, [3, 14, 19, 11, 7])
        } else {
            return false;
        };
        self.click == Some(point) && self.wait.hit.as_ref().is_some_and(|hit| hit.point == point && hit.region == region)
            && press.region == region && press.candidates == candidates
    }

    fn continuous_window(&self) -> Option<&'static str> {
        if self.same_novel_window { Some("novelwnd") }
        else if self.same_full_window { Some("fullwnd") }
        else if self.is_neko_body_press() || self.island_meswnd_edge().is_some() { Some("meswnd") }
        else { None }
    }

    fn is_neko_body_press(&self) -> bool {
        let (Some(page), Some(after), Some(press)) = (self.wait.neko_opening_page(), &self.after, &self.mouse_press) else {
            return false;
        };
        after.neko_opening_page() == Some(page + 1) && after.hit.is_none()
            && self.click == Some([625, 478]) && self.key.is_none()
            && self.wait.hit.as_ref().is_some_and(|hit| hit.point == [625, 478] && hit.region == 77)
            && press.candidates == [3, 77, 52, 57, 7]
            && (press.region, press.key, press.group, press.response) == (52, 0, 0, 1)
    }

    fn expects_same_novel_prompt(&self) -> bool {
        self.wait.state == "page"
            && self.wait.window.as_deref() == Some("novelwnd")
            && self.after.as_ref().is_some_and(|after| {
                after.state == "page_prompt" && after.window.as_deref() == Some("novelwnd")
                    && after.same_record(&self.wait)
            })
    }

    fn exits_full_window(&self) -> bool {
        self.wait.state == "page_probe" && self.wait.window.as_deref() == Some("fullwnd")
            && !self.same_full_window && self.mouse_press.is_some() && self.after.as_ref().is_some_and(|after| {
                matches!(after.state.as_str(), "page" | "page_or_prompt")
                    && after.window.as_deref() == Some("novelwnd") && after.hit.is_none()
            })
    }

    fn exits_novel_window(&self) -> bool {
        self.is_island_meswnd_probe_exit()
            || (self.wait.state == "page" && self.wait.window.as_deref() == Some("novelwnd")
                && !self.same_novel_window && self.mouse_press.is_some() && self.after.as_ref().is_some_and(|after| {
                    after.state == "page_probe" && after.window.as_deref() == Some("fullwnd") && after.hit.is_none()
                }))
    }

    fn is_island_meswnd_probe_exit(&self) -> bool {
        let (Some(after), Some(press)) = (&self.after, &self.mouse_press) else { return false; };
        self.wait.state == "page" && self.wait.scene.as_deref() == Some("com01")
            && self.wait.group == Some(52) && self.wait.record == Some(477)
            && self.wait.window.as_deref() == Some("novelwnd")
            && after.state == "page" && after.scene == self.wait.scene
            && after.group == Some(53) && after.record == Some(514)
            && after.window.as_deref() == Some("meswnd") && after.hit.is_none()
            && !self.same_novel_window && !self.same_full_window && self.key.is_none()
            && self.click == Some([557, 324])
            && self.wait.hit.as_ref().is_some_and(|hit| hit.point == [557, 324] && hit.region == 73)
            && press.candidates == [3, 73, 43, 7]
            && (press.region, press.key, press.group, press.response) == (43, 0, 0, 1)
    }

    fn island_meswnd_edge(&self) -> Option<((i32, i32), (i32, i32))> {
        let after = self.after.as_ref()?;
        let press = self.mouse_press.as_ref()?;
        let origin = self.wait.island_meswnd_page()?;
        let target = after.island_meswnd_page()?;
        (target.0 == origin.0 + 1 && target.1 > origin.1
            && (origin.0 != 53 || target == (54, 518)) && after.hit.is_none()
            && !self.same_novel_window && !self.same_full_window && self.key.is_none()
            && self.click == Some(ISLAND_MESWND_POINTS[1]) && self.probe_points == ISLAND_MESWND_POINTS
            && self.wait.hit.as_ref().is_some_and(|hit| hit.point == ISLAND_MESWND_POINTS[1] && hit.region == 82)
            && press.candidates == [3, 82, 7] && (press.region, press.key, press.group, press.response) == (82, 2, 0, 0))
            .then_some((origin, target))
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MousePress {
    candidates: Vec<u32>,
    region: u32,
    key: u32,
    group: u32,
    response: u32,
}

impl MousePress {
    fn matches_receipt(&self, receipt: &InputRouteReceipt, started: u64, receiver: u32) -> bool {
        receipt.frame > started
            && receipt.frame - started <= 3
            && receipt.key == self.key
            && receipt.group == self.group
            && receipt.raw_mask == ButtonSource::Mouse.mask()
            && receipt.pressed == 1
            && !receipt.repeated
            && receipt.region == self.region
            && receipt.receiver == receiver
            && receipt.response == self.response
            && receipt.physical == [RoutedPhysicalButton { source: ButtonSource::Mouse, code: 1 }]
    }

    fn island_root_receipt<'a>(&self, started: u64, receiver: u32, receipts: &'a [InputRouteReceipt], events: &[LogicEvent]) -> Option<&'a InputRouteReceipt> {
        let left = RoutedPhysicalButton { source: ButtonSource::Mouse, code: 1 };
        let frame = started.checked_add(1)?;
        let mut consumers = receipts.iter().filter(|receipt| {
            receipt.frame == frame && receipt.pressed != 0 && receipt.response == 0 && receipt.physical.contains(&left)
        });
        let receipt = consumers.next()?;
        let mut resumes = events.iter().rev().take_while(|event| event.frame >= frame)
            .filter(|event| event.frame == frame && event.kind == "message-resume");
        let resumed = resumes.next().is_some_and(|event| event.detail == "message=[2, 557, 324, 0]");
        (consumers.next().is_none() && self.matches_receipt(receipt, started, receiver)
            && resumed && resumes.next().is_none()).then_some(receipt)
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct KeyboardPress {
    region: u32,
    receiver: u32,
    key: u32,
    group: u32,
    response: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum KeyboardPressReceipt {
    #[default]
    Awaiting,
    Verified,
    Rejected,
}

impl KeyboardPressReceipt {
    fn observe<'a>(&mut self, expected: &KeyboardPress, started: u64, receipts: &'a [InputRouteReceipt], events: &[LogicEvent]) -> Option<&'a InputRouteReceipt> {
        let enter = RoutedPhysicalButton { source: ButtonSource::Keyboard, code: 13 };
        let mut consumed = receipts.iter().filter(|receipt| {
            receipt.frame > started && receipt.pressed != 0 && receipt.response == 0 && receipt.physical.contains(&enter)
        });
        let receipt = consumed.next()?;
        if *self != Self::Awaiting || consumed.next().is_some()
            || receipt.frame - started != 1 || receipt.repeated || receipt.pressed != 1
            || receipt.raw_mask != ButtonSource::Keyboard.mask() || receipt.physical != [enter]
            || (receipt.region, receipt.receiver, receipt.key, receipt.group, receipt.response)
                != (expected.region, expected.receiver, expected.key, expected.group, expected.response)
            || !events.iter().rev().any(|event| {
                event.frame == receipt.frame && event.kind == "message-resume" && event.detail == "message=[2, 0, 0, 0]"
            })
        {
            *self = Self::Rejected;
            return None;
        }
        *self = Self::Verified;
        Some(receipt)
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SharedFlag {
    slot: usize,
    mask: u32,
    expected: u32,
}

impl SharedFlag {
    fn matches(&self, value: Option<u32>) -> bool {
        value.is_some_and(|value| value & self.mask == self.expected)
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Route {
    label: String,
    idle_frames: u64,
    page_scenes: BTreeSet<String>,
    page_click: Option<Hit>,
    page_fallback_before: Option<usize>,
    steps: Vec<Step>,
}

impl Route {
    fn allows_page_fallback(&self, next: usize, scene: &str) -> bool {
        self.page_fallback_before.is_none_or(|end| next < end) && self.page_scenes.contains(scene)
    }
}

struct Pending {
    origin: Position,
    started: u64,
    step: Option<usize>,
    mouse_press: Option<PendingMousePress>,
    keyboard_press: KeyboardPressReceipt,
    island_meswnd: Option<IslandMeswndPress>,
}

struct IslandMeswndPress {
    anchor: IslandCstAnchor,
    window_owner: u32,
    root_lifetime: NovelWindowLifetime,
}

struct PendingMousePress {
    receiver: u32,
    confirmed: bool,
    lifetime: Option<NovelWindowLifetime>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NovelWindowLifetime {
    Armed { first_event: usize },
    Continuous,
    Recreated { frame: u64 },
    MissingEvents,
}

impl NovelWindowLifetime {
    fn observe(&mut self, owner: u32, events: &[LogicEvent], batch_start: usize) {
        let first = match *self {
            Self::Armed { first_event } => first_event,
            Self::Continuous => batch_start,
            Self::Recreated { .. } | Self::MissingEvents => return,
        };
        let Some(events) = events.get(first..).filter(|_| first >= batch_start) else {
            *self = Self::MissingEvents;
            return;
        };
        *self = events.iter().find(|event| {
            event.kind == "task-create"
                && event.detail.split_ascii_whitespace().next()
                    .and_then(|word| word.strip_prefix("id="))
                    .and_then(|id| id.parse::<u32>().ok()) == Some(owner)
        }).map_or(Self::Continuous, |event| Self::Recreated { frame: event.frame });
    }
}

impl PendingMousePress {
    fn confirms_novel_owner(&self, window: Option<StoryWindow>) -> bool {
        self.confirmed && window == Some(("novelwnd", self.receiver))
    }

    fn confirms_window_instance(&self, window: Option<StoryWindow>, name: &'static str) -> bool {
        self.confirmed && window == Some((name, self.receiver))
            && self.lifetime == Some(NovelWindowLifetime::Continuous)
    }
}

pub(super) enum Decision {
    Wait,
    Input(Action),
    Done,
    Stop(&'static str, String),
}

pub(super) struct Controller {
    route: Route,
    next: usize,
    pending: Option<Pending>,
    last_position: Option<Position>,
    changed: u64,
}

impl Controller {
    pub(super) fn idle_frames(&self) -> u64 {
        self.route.idle_frames
    }

    pub(super) fn read(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path).context("read replay route")?;
        let route: Route = serde_json::from_slice(&bytes).context("decode replay route")?;
        Self::from_route(route)
    }

    fn from_route(route: Route) -> Result<Self> {
        ensure!(route.idle_frames > 3 && !route.steps.is_empty(), "route requires steps and a wait limit");
        let last = route.steps.len() - 1;
        ensure!(route.page_fallback_before.is_none_or(|end| end <= route.steps.len()),
            "page fallback limit must fall within the route steps");
        for (index, step) in route.steps.iter().enumerate() {
            let input = step.click.is_some() || step.key.is_some();
            ensure!(!step.label.is_empty(), "route step label must not be empty");
            ensure!(step.click.is_none() || step.key.is_none(), "route step cannot send two inputs");
            ensure!(input == step.after.is_some(), "route input requires an expected result");
            ensure!(index != last || !input, "last route step must be a checkpoint");
            ensure!(!(step.same_novel_window && step.same_full_window)
                && (step.continuous_window().is_none() || step.mouse_press.is_some()),
                "a continuous window step requires one window contract and a verified mouse press");
            ensure!(!step.continue_same_record_prompt || step.same_novel_window && step.expects_same_novel_prompt(),
                "prompt continuation requires a guarded same-record novel-window prompt");
            if step.wait.state == "page_or_prompt" {
                ensure!(!input && step.wait.window.as_deref() == Some("novelwnd") && step.wait.has_exact_record(),
                    "a page or prompt checkpoint requires an exact novel-window record and cannot send input");
            }
            if step.wait.window.as_deref() == Some("novelwnd") {
                ensure!(!route.allows_page_fallback(index, step.wait.scene.as_deref().unwrap_or_default()),
                    "an explicit novel-window step cannot use scene-wide page fallback");
                ensure!(!input || step.mouse_press.is_some(),
                    "an explicit novel-window input requires a verified mouse press");
            }
            if let Some(point) = step.click {
                ensure!(step.wait.hit.as_ref().is_some_and(|hit| hit.point == point),
                    "route click needs a matching hit region");
            }
            if step.wait.state == "page_probe" && input {
                ensure!(step.mouse_press.is_some(), "page probe input requires a verified mouse press");
            }
            if step.wait.window.as_deref() == Some("meswnd") && input {
                ensure!(step.is_neko_body_press() || step.island_meswnd_edge().is_some(),
                    "a meswnd input requires an exact guarded NEKO or ISLAND route");
            }
            if step.keyboard_press.is_some() || input && step.wait.daydream_opening_page().is_some() {
                ensure!(step.is_daydream_enter() && route.page_scenes.is_empty()
                    && step.continuous_window().is_none() && !step.continue_same_record_prompt
                    && step.neko_startup_language.is_none(),
                    "Daydream Enter requires its exact opening step and keyboard receipt, with no fallback");
                let after = step.after.as_ref().unwrap();
                let next = &route.steps[index + 1];
                ensure!(next.wait.daydream_opening_page() == after.daydream_opening_page()
                    && next.wait.hit.is_none() && (index + 1 == last || next.is_daydream_enter()),
                    "Daydream Enter requires an exact next guarded page or input-free checkpoint");
            }
            if step.neko_startup_language.is_some() || input && step.wait.state == "language_select" {
                ensure!(step.is_neko_startup_press() && route.page_scenes.is_empty()
                    && step.continuous_window().is_none() && !step.continue_same_record_prompt,
                    "NEKO startup requires a verified left press and explicit jp or cn, with no fallback");
                let after = step.after.as_ref().unwrap();
                let next = &route.steps[index + 1];
                let next_is_guarded = if after.state == "title" {
                    next.is_neko_startup_press() && next.neko_startup_language == step.neko_startup_language
                } else {
                    next.is_neko_body_press()
                };
                ensure!(next.wait.state == after.state && next.wait.window == after.window && next.wait.same_record(after)
                    && (index + 1 == last && next.wait.hit.is_none() || next_is_guarded),
                    "NEKO startup requires an exact guarded successor or an input-free terminal probe");
                continue;
            }
            if let Some(press) = &step.mouse_press {
                let fullwnd = step.wait.state == "page_probe"
                    && step.wait.window.as_deref().is_none_or(|name| name == "fullwnd");
                let novel = matches!(step.wait.state.as_str(), "page" | "page_prompt")
                    && step.wait.window.as_deref() == Some("novelwnd");
                let neko = step.is_neko_body_press();
                let island_meswnd = step.island_meswnd_edge().is_some();
                ensure!((fullwnd || novel || neko || island_meswnd) && step.click.is_some(),
                    "verified mouse press requires an explicit guarded window");
                let after = step.after.as_ref().unwrap();
                let fullwnd_successor = novel && after.state == "page_probe"
                    && after.window.as_deref() == Some("fullwnd");
                let island_meswnd_probe_exit = step.is_island_meswnd_probe_exit();
                let after_ready = if fullwnd_successor || step.same_full_window || island_meswnd_probe_exit {
                    true
                } else if novel || after.state == "page_or_prompt" {
                    matches!(after.state.as_str(), "page" | "page_prompt" | "page_or_prompt")
                        && after.window.as_deref() == Some("novelwnd")
                } else {
                    after.state == "page"
                };
                let later_page = after.group > step.wait.group && after.record > step.wait.record;
                let exact_next_checkpoint = |next: &Step| {
                    next.wait.state == after.state && next.wait.window == after.window
                        && next.wait.same_record(after) && after.hit.is_none()
                };
                let no_fallback = route.page_scenes.is_empty()
                    || route.page_fallback_before.is_some_and(|end| end <= index);
                let terminal_checkpoint = index + 1 == last && exact_next_checkpoint(&route.steps[last])
                    && route.steps[last].wait.hit.is_none();
                if island_meswnd_probe_exit {
                    let next = &route.steps[index + 1];
                    ensure!(no_fallback && exact_next_checkpoint(next) && next.probe_points == ISLAND_MESWND_POINTS
                        && (terminal_checkpoint || next.island_meswnd_edge().is_some()),
                        "the ISLAND novel-to-message transition requires a double probe or exact guarded continuation without fallback");
                }
                if island_meswnd {
                    let next = &route.steps[index + 1];
                    let linked = index.checked_sub(1).is_some_and(|previous| {
                        let prior = &route.steps[previous];
                        (prior.is_island_meswnd_probe_exit() || prior.island_meswnd_edge().is_some())
                            && prior.after.as_ref().is_some_and(|after| after.same_record(&step.wait))
                    });
                    ensure!(no_fallback && linked && exact_next_checkpoint(next) && next.probe_points == ISLAND_MESWND_POINTS
                        && (terminal_checkpoint || next.island_meswnd_edge().is_some()),
                        "ISLAND meswnd requires a contiguous guarded chain and a double-point input-free stop without fallback");
                }
                if neko {
                    let next = &route.steps[index + 1];
                    ensure!(!step.same_novel_window && !step.same_full_window && no_fallback
                        && exact_next_checkpoint(next) && (terminal_checkpoint || next.is_neko_body_press()),
                        "a NEKO body press must lead to the exact guarded successor or an input-free stop, without fallback");
                }
                let same_record_prompt = step.expects_same_novel_prompt();
                let guarded_prompt_continuation = step.continue_same_record_prompt
                    && route.steps.get(index + 1).is_some_and(|next| {
                        exact_next_checkpoint(next) && next.mouse_press.is_some()
                            && (next.same_novel_window || next.after.as_ref().is_some_and(|successor| {
                                successor.state == "page_probe" && successor.window.as_deref() == Some("fullwnd")
                            }))
                    });
                if same_record_prompt {
                    ensure!(no_fallback && if step.continue_same_record_prompt { guarded_prompt_continuation }
                        else { terminal_checkpoint },
                        "a same-record prompt requires an exact terminal checkpoint or an explicit same-window guarded press, with no fallback");
                }
                if fullwnd_successor {
                    let guarded_continuation = index + 2 <= last && route.steps.get(index + 1).is_some_and(|next| {
                        exact_next_checkpoint(next) && next.mouse_press.is_some()
                            && (next.same_full_window || next.after.as_ref().is_some_and(|successor| {
                                let following = &route.steps[index + 2];
                                let terminal = index + 2 == last;
                                (successor.state == "page" || terminal && successor.state == "page_or_prompt")
                                    && successor.window.as_deref() == Some("novelwnd")
                                    && successor.hit.is_none()
                                    && following.wait.state == successor.state && following.wait.window == successor.window
                                    && following.wait.same_record(successor)
                                    && if terminal { following.wait.hit.is_none() } else {
                                        following.same_novel_window || following.exits_novel_window()
                                    }
                            }))
                    });
                    ensure!(no_fallback && (terminal_checkpoint || guarded_continuation),
                        "a full-window successor requires an exact terminal probe or an exact guarded segment, without scene-wide fallback");
                }
                if step.same_full_window {
                    let next = &route.steps[index + 1];
                    ensure!(fullwnd && step.wait.window.as_deref() == Some("fullwnd") && no_fallback
                        && later_page && after.state == "page_probe" && after.window == step.wait.window
                        && exact_next_checkpoint(next) && (terminal_checkpoint || next.same_full_window || next.exits_full_window()),
                        "continuous full-window steps require exact later full-window probes and no scene-wide fallback");
                }
                if step.exits_full_window() && index.checked_sub(1).is_some_and(|previous| route.steps[previous].same_full_window) {
                    let next = &route.steps[index + 1];
                    ensure!(no_fallback && exact_next_checkpoint(next)
                        && (terminal_checkpoint || after.state == "page" && (next.same_novel_window || next.exits_novel_window())),
                        "leaving a continuous full-window segment requires an exact novel-window probe or a guarded first-stage segment, without fallback");
                }
                if step.same_novel_window {
                    let next = &route.steps[index + 1];
                    let from_guarded_prompt = step.wait.state == "page_prompt"
                        && index.checked_sub(1).is_some_and(|previous| route.steps[previous].continue_same_record_prompt);
                    ensure!(novel && (step.wait.state == "page" || from_guarded_prompt) && no_fallback
                        && after.window == step.wait.window && exact_next_checkpoint(next)
                        && if same_record_prompt {
                            terminal_checkpoint || guarded_prompt_continuation
                        } else if index + 1 == last {
                            later_page && terminal_checkpoint && matches!(after.state.as_str(), "page" | "page_or_prompt")
                        } else { later_page && after.state == "page" && (next.same_novel_window || next.exits_novel_window()) },
                        "continuous novel-window steps require exact, later first-stage pages or an explicit same-record prompt boundary");
                }
                ensure!(step.wait.has_exact_record() && after.has_exact_record()
                    && after_ready && after.scene == step.wait.scene
                    && (later_page || same_record_prompt),
                    "verified mouse press requires an exact later ready page, terminal full-window probe, or terminal same-record novel prompt");
                ensure!(press.candidates.iter().filter(|&&region| region == press.region).count() == 1,
                    "verified mouse press needs one target in its ordered candidate list");
            }
            for flag in &step.shared_flags {
                ensure!(flag.mask != 0 && flag.expected & !flag.mask == 0,
                    "shared flag requires a nonzero mask containing all expected bits");
            }
        }
        Ok(Self { route, next: 0, pending: None, last_position: None, changed: 0 })
    }

    pub(super) fn observe_lifecycle_events(&mut self, events: &[LogicEvent], batch_start: usize) {
        let Some(pending) = self.pending.as_mut() else { return; };
        let Some(press) = pending.mouse_press.as_mut() else { return; };
        let owner = if let Some(island) = &mut pending.island_meswnd {
            island.root_lifetime.observe(press.receiver, events, batch_start);
            island.window_owner
        } else {
            press.receiver
        };
        if let Some(lifetime) = &mut press.lifetime {
            lifetime.observe(owner, events, batch_start);
        }
    }

    pub(super) fn observe_frame(&mut self, player: &HeadlessPlayer) {
        let Some(pending) = &mut self.pending else { return; };
        let Some(index) = pending.step else { return; };
        let step = &self.route.steps[index];
        if let Some(expected) = &step.keyboard_press {
            if let Some(receipt) = pending.keyboard_press.observe(expected, pending.started, player.host_input_route_receipts(), player.events()) {
                eprintln!("replay frame={}: verified keyboard press and VM resume {:?}: {receipt:?}", receipt.frame, step.label);
            }
        }
        let Some(press) = &mut pending.mouse_press else { return; };
        if press.confirmed {
            return;
        }
        let expected = step.mouse_press.as_ref().unwrap();
        let receipts = player.host_input_route_receipts();
        let receipt = if pending.island_meswnd.is_some() {
            expected.island_root_receipt(pending.started, press.receiver, receipts, player.events())
        } else {
            receipts.iter().find(|receipt| expected.matches_receipt(receipt, pending.started, press.receiver))
        };
        if let Some(receipt) = receipt {
            press.confirmed = true;
            eprintln!("replay frame={}: verified mouse press {:?}: {receipt:?}", receipt.frame, step.label);
        }
    }

    pub(super) fn poll(&mut self, player: &mut HeadlessPlayer, frame: u64, output: &Path) -> Result<Decision> {
        let (position, window) = Position::observe(player);
        if self.last_position.as_ref() != Some(&position) {
            println!("replay frame={frame} state={} scene={} group={} record={}",
                position.state, position.scene, position.group, position.record);
            self.changed = frame;
            self.last_position = Some(position.clone());
        } else if frame % 120 == 0 {
            println!("replay frame={frame} heartbeat");
        }

        if let Some(pending) = &self.pending {
            let changed = pending.origin != position;
            let ready = matches!(position.state, "page" | "page_prompt" | "choice" | "title" | "menu" | "language_select");
            if let Some(index) = pending.step {
                let step = &self.route.steps[index];
                if pending.mouse_press.as_ref().is_some_and(|press| !press.confirmed) {
                    return Ok(Decision::Stop("input_missed", format!(
                        "no matching mouse press receipt after {:?}", step.label,
                    )));
                }
                if step.keyboard_press.is_some() && pending.keyboard_press != KeyboardPressReceipt::Verified {
                    return Ok(Decision::Stop("input_missed", format!(
                        "keyboard press not verified after {:?}: receipt={:?}", step.label, pending.keyboard_press,
                    )));
                }
                let same_instance = if let Some(island) = &pending.island_meswnd {
                    let (_, target) = step.island_meswnd_edge().unwrap();
                    island.root_lifetime == NovelWindowLifetime::Continuous
                        && pending.mouse_press.as_ref().is_some_and(|press| press.lifetime == Some(NovelWindowLifetime::Continuous))
                        && (!step.after.as_ref().unwrap().matches_position(&position, window)
                            || player.host_island_meswnd_anchor(target, None, island.window_owner) == Some(island.anchor))
                } else {
                    step.continuous_window().is_none_or(|name| pending.mouse_press.as_ref().is_some_and(|press| {
                        press.confirms_window_instance(window, name)
                    }))
                };
                if same_instance && (!step.expects_same_novel_prompt() || pending.mouse_press.as_ref()
                    .is_some_and(|press| press.confirms_novel_owner(window)))
                    && step.after.as_ref().unwrap().matches(&position, window, player)?
                {
                    if step.neko_startup_language.as_deref().is_some_and(|language| !player.host_neko_startup_language_matches(language)) {
                        return Ok(Decision::Stop("checkpoint_mismatch", format!("NEKO startup language changed after {:?}", step.label)));
                    }
                    if step.continue_same_record_prompt
                        && !self.route.steps[self.next].wait.matches(&position, window, player)?
                    {
                        return Ok(Decision::Stop("checkpoint_mismatch", format!(
                            "prompt continuation withheld after {:?}: the follow-up hit did not match {position:?} window={window:?}", step.label,
                        )));
                    }
                    println!("replay frame={frame} verified={:?}", step.label);
                    if pending.mouse_press.is_some() || step.keyboard_press.is_some() {
                        eprintln!("frame {frame}: guarded press successor={position:?} window={window:?}");
                    }
                    self.pending = None;
                } else if (changed || !same_instance) && (ready || (pending.mouse_press.is_some() || step.keyboard_press.is_some()) && position.state == "page_probe") {
                    if step.continuous_window().is_some() {
                        let point = step.click.unwrap();
                        let hit = player.masked_input_region_at(point)?;
                        let candidates = player.mouse_press_candidate_regions(point)?;
                        let lifetime = pending.mouse_press.as_ref().and_then(|press| press.lifetime);
                        eprintln!("frame {frame}: window segment stopped {position:?} window={window:?} same_instance={same_instance} lifetime={lifetime:?} point={point:?} masked={hit:?} press_candidates={candidates:?}");
                    }
                    return Ok(Decision::Stop("unexpected_branch", format!("after {:?}: {position:?} window={window:?}", step.label)));
                } else if frame - pending.started >= self.route.idle_frames {
                    let status = if changed { "unrecognized_wait" } else { "input_missed" };
                    return Ok(Decision::Stop(status, format!("after {:?}: {position:?}", step.label)));
                } else {
                    return Ok(Decision::Wait);
                }
            } else if changed && ready && !(pending.origin.state == "page_prompt"
                && position.state == "page" && pending.origin.scene == position.scene
                && pending.origin.record == position.record)
            {
                self.pending = None;
            } else if frame - pending.started >= self.route.idle_frames {
                let status = if changed { "unrecognized_wait" } else { "input_missed" };
                return Ok(Decision::Stop(status, format!("after page: {position:?}")));
            } else {
                return Ok(Decision::Wait);
            }
        }

        let Some(step) = self.route.steps.get(self.next) else {
            return Ok(Decision::Done);
        };
        let neko_body = is_neko_runtime(player) && position.state == "page"
            && window.is_some_and(|(name, _)| name == "meswnd");
        let daydream_body = is_daydream_runtime(player) && position.state == "page"
            && window.is_some_and(|(name, _)| name == "cinema");
        if step.wait.matches(&position, window, player)? {
            if daydream_body && (step.click.is_some() || step.key.is_some()) && !step.is_daydream_enter()
                || step.keyboard_press.is_some() && !daydream_body
            {
                return Ok(Decision::Stop("checkpoint_mismatch", "Daydream input requires an exact cinema page and guarded Enter route".into()));
            }
            if let Some(press) = &step.keyboard_press {
                let button = player.input_button_state(press.region);
                eprintln!("frame {frame}: guarded Enter window={window:?} region={} receiver={:?}",
                    press.region, button.and_then(|button| button.receiver));
                if !button.is_some_and(|button| button.show != 0 && button.receiver == Some(press.receiver)) {
                    return Ok(Decision::Stop("checkpoint_mismatch", format!(
                        "Enter withheld at {:?}: region {} is not enabled and bound to bottom-button receiver {}",
                        step.label, press.region, press.receiver,
                    )));
                }
            }
            let neko_startup = is_neko_runtime(player) && matches!(position.state, "language_select" | "title");
            if neko_startup && (step.click.is_some() || step.key.is_some()) {
                eprintln!("frame {frame}: NEKO startup {}", player.host_replay_wait_diagnostics());
                if !step.is_neko_startup_press() || !step.neko_startup_language.as_deref()
                    .is_some_and(|language| player.host_neko_startup_language_matches(language))
                {
                    return Ok(Decision::Stop("checkpoint_mismatch", format!(
                        "NEKO startup press withheld at {:?}: explicit language and live selection did not match", step.label,
                    )));
                }
            } else if step.is_neko_startup_press() {
                return Ok(Decision::Stop("checkpoint_mismatch", "NEKO startup runtime did not match".into()));
            }
            if step.is_neko_body_press() && !neko_body
                || neko_body && (step.click.is_some() || step.key.is_some()) && !step.is_neko_body_press()
            {
                return Ok(Decision::Stop("checkpoint_mismatch", format!(
                    "NEKO press withheld at {:?}: exact runtime and guarded body route required", step.label,
                )));
            }
            for &point in &step.probe_points {
                let masked = player.masked_input_region_at(point)?;
                let press_candidates = player.mouse_press_candidate_regions(point)?;
                let press_receivers: Vec<_> = press_candidates.iter().map(|&region| {
                    (region, player.input_button_state(region).and_then(|button| button.receiver))
                }).collect();
                eprintln!("frame {frame}: probe {point:?} masked={masked:?} press_candidates={press_candidates:?} window={window:?} press_receivers={press_receivers:?}");
            }
            if !step.probe_points.is_empty() && step.wait.window.is_some() {
                eprintln!("frame {frame}: probe wait {}", player.host_replay_wait_diagnostics());
            }
            let mut pending_mouse_press = None;
            let mut island_meswnd = None;
            if let Some(press) = &step.mouse_press {
                let point = step.click.unwrap();
                let candidates = player.mouse_press_candidate_regions(point)?;
                let owner = window.map(|(_, owner)| owner);
                let receiver = player.input_button_state(press.region).and_then(|button| button.receiver);
                let mut expected_receiver = owner;
                if let Some((origin, target)) = step.island_meswnd_edge() {
                    let anchor = owner.and_then(|owner| player.host_island_meswnd_anchor(origin, Some(target), owner));
                    let both_points_match = player.masked_input_region_at(ISLAND_MESWND_POINTS[0])?
                        .is_some_and(|region| region[0] == 82 && region[8] != 0)
                        && player.mouse_press_candidate_regions(ISLAND_MESWND_POINTS[0])? == press.candidates;
                    let Some((anchor, window_owner)) = anchor.zip(owner).filter(|(anchor, owner)| {
                        both_points_match && anchor.2 != *owner
                    }) else {
                        return Ok(Decision::Stop("checkpoint_mismatch", format!(
                            "ISLAND root press withheld at {:?}: two-point hit or live CST edge did not match; {}",
                            step.label, player.host_replay_wait_diagnostics(),
                        )));
                    };
                    expected_receiver = Some(anchor.2);
                    island_meswnd = Some(IslandMeswndPress { anchor, window_owner,
                        root_lifetime: NovelWindowLifetime::Armed { first_event: player.events().len() } });
                }
                eprintln!("frame {frame}: guarded press {point:?} candidates={candidates:?} window={window:?} receiver={receiver:?}");
                if candidates != press.candidates || receiver.is_none() || receiver != expected_receiver {
                    return Ok(Decision::Stop("checkpoint_mismatch", format!(
                        "mouse press withheld at {:?}: expected {:?} and region {} receiver {:?}",
                        step.label, press.candidates, press.region, expected_receiver,
                    )));
                }
                pending_mouse_press = receiver.map(|receiver| PendingMousePress {
                    receiver,
                    confirmed: false,
                    lifetime: step.continuous_window().map(|_| NovelWindowLifetime::Armed { first_event: player.events().len() }),
                });
            }
            if !step.shared_flags.is_empty() {
                let snapshot = player.shared_numeric_state_snapshot()?;
                let root = if snapshot.active_identity == 0 {
                    Some(&snapshot.active_values)
                } else {
                    snapshot.inactive_values.get(&0)
                };
                for flag in &step.shared_flags {
                    let actual = root.and_then(|values| values.get(flag.slot)).copied();
                    println!("replay frame={frame} checkpoint={:?} shared_slot={} mask={:#x} expected={:#x} actual={actual:?}",
                        step.label, flag.slot, flag.mask, flag.expected);
                    if !flag.matches(actual) {
                        return Ok(Decision::Stop("checkpoint_mismatch", format!(
                            "shared slot #{} at {:?} did not match", flag.slot, step.label,
                        )));
                    }
                }
            }
            if step.capture {
                super::capture(player, output, frame)?;
            }
            println!("replay frame={frame} checkpoint={:?}", step.label);
            let step_index = self.next;
            self.next += 1;
            let action = step.click.map(|[x, y]| Action::Pointer { x, y, button: Some(0) })
                .or_else(|| step.key.map(|key| Action::Key(InputKey::Keyboard(key))));
            if let Some(action) = action {
                self.pending = Some(Pending { origin: position, started: frame, step: Some(step_index),
                    mouse_press: pending_mouse_press, keyboard_press: KeyboardPressReceipt::Awaiting, island_meswnd });
                return Ok(Decision::Input(action));
            }
            return Ok(if self.next == self.route.steps.len() { Decision::Done } else { Decision::Wait });
        }

        match position.state {
            "page" | "page_prompt"
                if !neko_body && !daydream_body && self.route.allows_page_fallback(self.next, &position.scene) => {
                let action = if let Some(hit) = &self.route.page_click {
                    if !hit.matches(player)? {
                        if frame - self.changed >= self.route.idle_frames {
                            return Ok(Decision::Stop("input_missed", format!(
                                "page point {:?} did not reach region {} at {position:?}", hit.point, hit.region,
                            )));
                        }
                        return Ok(Decision::Wait);
                    }
                    let [x, y] = hit.point;
                    Action::Pointer { x, y, button: Some(0) }
                } else {
                    Action::Key(InputKey::Keyboard(13))
                };
                self.pending = Some(Pending { origin: position, started: frame, step: None, mouse_press: None,
                    keyboard_press: KeyboardPressReceipt::Awaiting, island_meswnd: None });
                Ok(Decision::Input(action))
            }
            "menu" | "choice" | "page" | "page_prompt" | "page_probe" | "language_select" => Ok(Decision::Stop("unknown_state", format!("{position:?} window={window:?}"))),
            "title" if step.wait.state != "title" => Ok(Decision::Stop("unknown_state", "unexpected title".into())),
            _ if frame - self.changed >= self.route.idle_frames => {
                Ok(Decision::Stop("unrecognized_wait", format!("{position:?}")))
            }
            _ => Ok(Decision::Wait),
        }
    }

    pub(super) fn label(&self) -> &str {
        &self.route.label
    }
}

#[cfg(test)]
mod tests {
    use super::{ButtonSource, Condition, Controller, InputRouteReceipt, KeyboardPressReceipt, LogicEvent, MousePress, NovelWindowLifetime, PendingMousePress, Position, Route,
        RoutedPhysicalButton, SharedFlag};
    use serde_json::json;

    #[test]
    fn daydream_enter_requires_an_exact_guarded_chain_and_terminal_probe() {
        let scenario: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../../scripts/analysis/pc_story_17_r18_to_r25_guarded_enter.json"
        )).unwrap();
        let route = scenario["17"]["route"].clone();
        let validate = |route| Controller::from_route(serde_json::from_value::<Route>(route).unwrap());
        let controller = validate(route.clone()).unwrap();
        assert!(controller.route.steps[..2].iter().all(|step| step.is_daydream_enter()));
        assert!(controller.route.steps[2].key.is_none());
        for (step, field, value) in [
            (0, "keyboard_press", json!(null)),
            (1, "key", json!(32)),
            (2, "key", json!(13)),
        ] {
            let mut invalid = route.clone();
            invalid["steps"][step][field] = value;
            assert!(validate(invalid).is_err(), "{step} {field}");
        }
        let mut invalid = route.clone();
        invalid["steps"][1]["keyboard_press"]["receiver"] = json!(65);
        assert!(validate(invalid).is_err());
        let mut invalid = route.clone();
        invalid["steps"][1]["after"]["record"] = json!(28);
        assert!(validate(invalid).is_err());
        let mut invalid = route;
        invalid["page_scenes"] = json!(["prologue"]);
        assert!(validate(invalid).is_err());
    }

    #[test]
    fn daydream_prologue_chain_cannot_skip_records_or_input_after_the_last_verified_anchor() {
        let gate = |group, record| json!({"state":"page","scene":"prologue","group":group,"record":record,"window":"cinema"});
        let page = |group: usize| gate(group, super::DAYDREAM_PROLOGUE_WAITS[group]);
        let step = |wait, after| json!({"label":"one guarded Enter", "wait":wait, "after":after,
            "key":13, "keyboard_press":{"region":62,"receiver":8,"key":2,"group":0,"response":0}});
        let initial = (0..19).map(|group| step(page(group), page(group + 1))).collect::<Vec<_>>();
        let mut route = json!({"label":"Daydream prologue", "idle_frames":720,"page_scenes":[], "steps":initial});
        let terminal = json!({"label":"observe only","wait":page(19)});
        route["steps"].as_array_mut().unwrap().push(terminal);
        let validate = |route| Controller::from_route(serde_json::from_value::<Route>(route).unwrap());
        assert!(validate(route.clone()).is_ok());

        let mut skipped = route.clone();
        skipped["steps"][10]["after"] = page(12);
        skipped["steps"].as_array_mut().unwrap().remove(11);
        assert!(validate(skipped).is_err());
        let steps = route["steps"].as_array_mut().unwrap();
        steps[19] = step(page(19), gate(20, 96));
        steps.push(json!({"label":"outside the tested segment","wait":gate(20, 96)}));
        assert!(validate(route).is_err());
    }

    #[test]
    fn daydream_enter_accepts_only_one_fresh_bottom_button_consumer_with_vm_resume() {
        let expected = super::KeyboardPress { region: 62, receiver: 8, key: 2, group: 0, response: 0 };
        let started = 1164;
        let enter = RoutedPhysicalButton { source: ButtonSource::Keyboard, code: 13 };
        let routed = InputRouteReceipt { frame: started + 1, key: 2, group: 0, raw_mask: 1,
            pressed: 1, repeated: false, region: 62, receiver: 8, response: 0, physical: vec![enter] };
        let cinema_pass = InputRouteReceipt { region: 79, receiver: 65, response: 1, ..routed.clone() };
        let resume = LogicEvent { frame: started + 1, kind: "message-resume".into(), detail: "message=[2, 0, 0, 0]".into() };
        let events = std::slice::from_ref(&resume);
        let mut state = KeyboardPressReceipt::Awaiting;
        assert!(state.observe(&expected, started, &[cinema_pass.clone(), routed.clone()], events).is_some());
        assert_eq!(state, KeyboardPressReceipt::Verified);
        assert!(state.observe(&expected, started, &[], &[]).is_none());
        assert_eq!(state, KeyboardPressReceipt::Verified);
        let second = InputRouteReceipt { frame: started + 2, ..routed.clone() };
        assert!(state.observe(&expected, started, &[second], events).is_none());
        assert_eq!(state, KeyboardPressReceipt::Rejected);

        for wrong in [
            InputRouteReceipt { frame: started, ..routed.clone() },
            InputRouteReceipt { frame: started + 2, ..routed.clone() },
            InputRouteReceipt { key: 0, ..routed.clone() },
            InputRouteReceipt { group: 1, ..routed.clone() },
            InputRouteReceipt { receiver: 65, ..routed.clone() },
            InputRouteReceipt { raw_mask: 3, ..routed.clone() },
            InputRouteReceipt { response: 1, ..routed.clone() },
            InputRouteReceipt { repeated: true, ..routed.clone() },
            InputRouteReceipt { physical: vec![RoutedPhysicalButton { code: 32, ..enter }], ..routed.clone() },
        ] {
            let mut state = KeyboardPressReceipt::Awaiting;
            assert!(state.observe(&expected, started, &[wrong], events).is_none());
            assert_ne!(state, KeyboardPressReceipt::Verified);
        }
        let mut state = KeyboardPressReceipt::Awaiting;
        let cinema_consumes = InputRouteReceipt { response: 0, ..cinema_pass };
        assert!(state.observe(&expected, started, &[routed.clone(), cinema_consumes], events).is_none());
        assert_eq!(state, KeyboardPressReceipt::Rejected);
        let mut state = KeyboardPressReceipt::Awaiting;
        let wrong_frame = LogicEvent { frame: started + 2, ..resume };
        assert!(state.observe(&expected, started, &[routed], &[wrong_frame]).is_none());
        assert_eq!(state, KeyboardPressReceipt::Rejected);
    }

    #[test]
    fn novel_checkpoint_requires_the_exact_ready_record_and_window() {
        let condition: Condition = serde_json::from_value(json!({
            "state": "page_or_prompt", "scene": "com01", "group": 30, "record": 222, "window": "novelwnd"
        })).unwrap();
        let mut position = Position { state: "page", scene: "com01".into(), group: 30, record: 222 };
        let novel = Some(("novelwnd", 15));
        assert!(condition.matches_position(&position, novel));
        position.state = "page_prompt";
        assert!(condition.matches_position(&position, novel));
        for wrong in [None, Some(("meswnd", 15)), Some(("fullwnd", 15))] {
            assert!(!condition.matches_position(&position, wrong));
        }
        for state in ["active", "unknown", "page_probe", "choice"] {
            position.state = state;
            assert!(!condition.matches_position(&position, novel));
        }
        position.state = "page";
        position.record = 226;
        assert!(!condition.matches_position(&position, novel));
    }

    #[test]
    fn novel_route_rejects_fallback_and_requires_a_distinct_ready_successor() {
        let mut route = json!({
            "label": "novel probe", "idle_frames": 3600, "page_scenes": ["com01"], "page_fallback_before": 1,
            "steps": [
                {"label": "prefix", "wait": {"state": "page", "scene": "com01", "group": 29, "record": 209}},
                {"label": "probe", "wait": {"state": "page_or_prompt", "scene": "com01", "group": 30,
                    "record": 222, "window": "novelwnd"}},
                {"label": "stop", "wait": {"state": "page_or_prompt", "scene": "com01", "group": 31,
                    "record": 226, "window": "novelwnd"}}
            ]
        });
        let validate = |route| Controller::from_route(serde_json::from_value::<Route>(route).unwrap());
        let controller = validate(route.clone()).unwrap();
        assert!(controller.route.allows_page_fallback(0, "com01"));
        assert!(!controller.route.allows_page_fallback(1, "com01"));
        assert!(!controller.route.allows_page_fallback(2, "com01"));
        route["page_fallback_before"] = json!(2);
        assert!(validate(route.clone()).is_err());
        route["page_fallback_before"] = json!(1);
        route["steps"][1]["wait"]["window"] = json!(null);
        assert!(validate(route.clone()).is_err());
        route["steps"][1]["wait"]["window"] = json!("novelwnd");
        route["steps"][1]["after"] = route["steps"][2]["wait"].clone();
        route["steps"][1]["key"] = json!(13);
        assert!(validate(route.clone()).is_err());
        route["steps"][1]["wait"]["state"] = json!("page");
        assert!(validate(route.clone()).is_err());
        route["steps"][1]["key"] = json!(null);
        route["steps"][1]["click"] = json!([557, 530]);
        route["steps"][1]["wait"]["hit"] = json!({"point": [557, 530], "region": 73});
        route["steps"][1]["mouse_press"] = json!({"candidates": [3, 73, 77, 7], "region": 77,
            "key": 0, "group": 0, "response": 1});
        assert!(validate(route.clone()).is_ok());
        route["steps"][1]["after"]["window"] = json!("meswnd");
        assert!(validate(route.clone()).is_err());
        route["steps"][1]["after"]["window"] = json!("novelwnd");
        route["steps"][1]["after"]["record"] = json!(222);
        assert!(validate(route).is_err());
    }

    #[test]
    fn same_novel_prompt_requires_a_fresh_receipt_on_the_same_owner_and_a_terminal_probe() {
        let prompt = json!({"state": "page_prompt", "scene": "com01", "group": 31, "record": 226, "window": "novelwnd"});
        let mut route = json!({
            "label": "single stage test", "idle_frames": 3600, "page_scenes": ["com01"], "page_fallback_before": 0,
            "steps": [
                {"label": "press", "wait": {"state": "page", "scene": "com01", "group": 31, "record": 226,
                    "window": "novelwnd", "hit": {"point": [557, 324], "region": 73}}, "click": [557, 324],
                    "mouse_press": {"candidates": [3, 73, 43, 7], "region": 43, "key": 0, "group": 0, "response": 1},
                    "after": prompt.clone()},
                {"label": "stop", "wait": prompt, "probe_points": [[557, 324]]}
            ]
        });
        let validate = |route| Controller::from_route(serde_json::from_value::<Route>(route).unwrap());
        let controller = validate(route.clone()).unwrap();
        let step = &controller.route.steps[0];
        assert!(step.expects_same_novel_prompt());
        let after = step.after.as_ref().unwrap();
        let mut position = Position { state: "active", scene: "com01".into(), group: 31, record: 226 };
        let owner = Some(("novelwnd", 15));
        assert!(!after.matches_position(&position, owner));
        position.state = "page";
        assert!(!after.matches_position(&position, owner));
        position.state = "page_prompt";
        assert!(after.matches_position(&position, owner));
        let mut press = PendingMousePress { receiver: 15, confirmed: false, lifetime: None };
        assert!(!press.confirms_novel_owner(owner));
        press.confirmed = true;
        assert!(press.confirms_novel_owner(owner));
        assert!(!press.confirms_novel_owner(Some(("novelwnd", 16))));
        assert!(!press.confirms_novel_owner(Some(("fullwnd", 15))));
        route["steps"][1]["wait"]["state"] = json!("page_or_prompt");
        assert!(validate(route.clone()).is_err());
        route["steps"][1]["wait"]["state"] = json!("page_prompt");
        let extra = route["steps"][1].clone();
        route["steps"].as_array_mut().unwrap().push(extra);
        assert!(validate(route).is_err());
    }

    fn task_creation(owner: u32, frame: u64) -> LogicEvent {
        LogicEvent { frame, kind: "task-create".into(),
            detail: format!("id={owner} kind=34 parent=8 name=\"meswnd\" ordinary_unloaded=false") }
    }

    #[test]
    fn novel_lifetime_survives_compaction_of_historical_creation() {
        let owner = Some(("novelwnd", 15));
        let mut events = vec![task_creation(15, 859), task_creation(150, 894)];
        let mut press = PendingMousePress { receiver: 15, confirmed: false,
            lifetime: Some(NovelWindowLifetime::Armed { first_event: 1 }) };
        assert!(!press.confirms_window_instance(owner, "novelwnd"));
        press.lifetime.as_mut().unwrap().observe(15, &events, 0);
        assert!(!press.confirms_window_instance(owner, "novelwnd"));
        events.drain(..1);
        events.push(task_creation(16, 897));
        press.lifetime.as_mut().unwrap().observe(15, &events, 1);
        press.confirmed = true;
        assert!(press.confirms_window_instance(owner, "novelwnd"));
        assert!(!press.confirms_window_instance(Some(("novelwnd", 16)), "novelwnd"));
        assert!(!press.confirms_window_instance(Some(("fullwnd", 15)), "novelwnd"));
    }

    #[test]
    fn novel_lifetime_rejects_same_id_recreation_even_after_compaction() {
        let owner = Some(("novelwnd", 15));
        let mut press = PendingMousePress { receiver: 15, confirmed: true,
            lifetime: Some(NovelWindowLifetime::Armed { first_event: 0 }) };
        press.lifetime.as_mut().unwrap().observe(15, &[], 0);
        assert!(press.confirms_window_instance(owner, "novelwnd"));
        press.lifetime.as_mut().unwrap().observe(15, &[task_creation(15, 895)], 0);
        press.lifetime.as_mut().unwrap().observe(15, &[], 0);
        assert_eq!(press.lifetime, Some(NovelWindowLifetime::Recreated { frame: 895 }));
        assert!(!press.confirms_window_instance(owner, "novelwnd"));
    }

    #[test]
    fn novel_lifetime_rejects_missing_first_batch() {
        let mut lifetime = NovelWindowLifetime::Armed { first_event: 2 };
        lifetime.observe(15, &[task_creation(16, 895)], 0);
        assert_eq!(lifetime, NovelWindowLifetime::MissingEvents);
        lifetime.observe(15, &[], 0);
        assert_eq!(lifetime, NovelWindowLifetime::MissingEvents);
    }

    #[test]
    fn island_36_stage_once_requires_a_same_instance_terminal_prompt() {
        let scenario: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../../scripts/analysis/pc_story_island_novel_36_stage_once.json"
        )).unwrap();
        let controller = Controller::from_route(serde_json::from_value::<Route>(scenario["3"]["route"].clone()).unwrap()).unwrap();
        let steps = &controller.route.steps;
        assert!(steps[12].same_novel_window && steps[12].expects_same_novel_prompt());
        assert_eq!(steps[12].wait.record, Some(270));
        assert!(steps[13].click.is_none() && steps[13].key.is_none());
    }

    #[test]
    fn neko_opening_chain_stops_at_exact_later_anchors_without_skipping() {
        let records = [59,87,92,98,103,112,117,122,127,136,141,146];
        let page = |group: usize| json!({"state":"page","scene":"01","group":group,
            "record":records[group],"window":"meswnd"});
        let validate = |route| Controller::from_route(serde_json::from_value::<Route>(route).unwrap());
        for terminal in [6, 11] {
            let mut steps = (0..terminal).map(|group| {
                let mut wait = page(group);
                wait["hit"] = json!({"point":[625,478],"region":77});
                json!({"label":"guarded NEKO opening page", "wait":wait, "after":page(group + 1),
                    "click":[625,478], "mouse_press":{"candidates":[3,77,52,57,7],"region":52,
                    "key":0,"group":0,"response":1}})
            }).collect::<Vec<_>>();
            steps.push(json!({"label":"no-input stop", "wait":page(terminal)}));
            let route = json!({"label":"NEKO opening", "idle_frames":720,"page_scenes":[], "steps":steps});
            let controller = validate(route.clone()).unwrap();
            assert!(controller.route.steps[..terminal].iter().all(|step| step.is_neko_body_press()));
            let last = &controller.route.steps[terminal];
            assert_eq!(last.wait.record, Some(records[terminal]));
            assert!(last.click.is_none() && last.key.is_none());

            let mut skipped = route;
            skipped["steps"][3]["after"] = page(5);
            skipped["steps"].as_array_mut().unwrap().remove(4);
            assert!(validate(skipped).is_err());
        }
    }

    #[test]
    fn neko_opening_guard_accepts_only_the_proven_mouse_path() {
        let scenario: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../../scripts/analysis/pc_story_neko_first_press_once.json"
        )).unwrap();
        let route = scenario["1"]["route"].clone();
        let validate = |route| Controller::from_route(serde_json::from_value::<Route>(route).unwrap());
        let controller = validate(route.clone()).unwrap();
        let step = &controller.route.steps[1];
        assert_eq!(step.continuous_window(), Some("meswnd"));
        assert!(controller.route.steps[2].click.is_none() && controller.route.steps[2].key.is_none());
        let mut position = Position { state: "page", scene: "01".into(), group: 0, record: 59 };
        assert!(step.wait.matches_position(&position, Some(("meswnd", 35))));
        position.state = "active";
        assert!(!step.wait.matches_position(&position, Some(("meswnd", 35))));
        let mut press = PendingMousePress { receiver: 35, confirmed: true,
            lifetime: Some(NovelWindowLifetime::Continuous) };
        assert!(press.confirms_window_instance(Some(("meswnd", 35)), "meswnd"));
        assert!(!press.confirms_window_instance(Some(("meswnd", 36)), "meswnd"));
        press.lifetime.as_mut().unwrap().observe(35, &[task_creation(35, 1254)], 0);
        assert!(!press.confirms_window_instance(Some(("meswnd", 35)), "meswnd"));

        for (section, field, value) in [
            ("wait", "group", json!(1)),
            ("wait", "scene", json!("02")),
            ("after", "record", json!(92)),
            ("after", "window", json!("cinema")),
            ("mouse_press", "candidates", json!([3, 77, 57, 52, 7])),
            ("mouse_press", "response", json!(0)),
        ] {
            let mut invalid = route.clone();
            invalid["steps"][1][section][field] = value;
            assert!(validate(invalid).is_err(), "{section} {field}");
        }
        let mut invalid = route.clone();
        invalid["steps"][1]["mouse_press"] = json!(null);
        assert!(validate(invalid).is_err());
        let mut invalid = route;
        invalid["page_fallback_before"] = json!(2);
        invalid["page_scenes"] = json!(["01"]);
        assert!(validate(invalid).is_err());
    }

    #[test]
    fn full_window_segment_needs_exact_successors_live_owner_and_a_fresh_receipt() {
        let ready = |group, record| json!({"state": "page_probe", "scene": "com01", "group": group,
            "record": record, "window": "fullwnd"});
        let guarded = |mut wait: serde_json::Value, after| {
            wait["hit"] = json!({"point": [557, 324], "region": 73});
            json!({"label": "press", "wait": wait, "after": after, "same_full_window": true,
                "click": [557, 324], "mouse_press": {"candidates": [3, 73, 14, 7], "region": 14,
                    "key": 0, "group": 0, "response": 1}})
        };
        let route = json!({"label": "full-window segment", "idle_frames": 3600, "page_scenes": ["com01"],
            "page_fallback_before": 0, "steps": [guarded(ready(37, 291), ready(38, 305)),
                guarded(ready(38, 305), ready(39, 319)), {"label": "stop", "wait": ready(39, 319)}]});
        let validate = |route| Controller::from_route(serde_json::from_value::<Route>(route).unwrap());
        assert!(validate(route.clone()).is_ok());
        for (step, section, field, wrong) in [
            (1, "wait", "record", json!(304)),
            (1, "wait", "window", json!("novelwnd")),
            (1, "after", "state", json!("active")),
            (2, "wait", "group", json!(38)),
        ] {
            let mut invalid = route.clone();
            invalid["steps"][step][section][field] = wrong;
            assert!(validate(invalid).is_err(), "{step} {section} {field}");
        }
        let mut invalid = route.clone();
        invalid["steps"][1]["same_full_window"] = json!(false);
        assert!(validate(invalid).is_err());
        let mut invalid = route;
        invalid["page_fallback_before"] = json!(1);
        assert!(validate(invalid).is_err());

        let owner = Some(("fullwnd", 12));
        let mut press = PendingMousePress { receiver: 12, confirmed: false,
            lifetime: Some(NovelWindowLifetime::Continuous) };
        assert!(!press.confirms_window_instance(owner, "fullwnd"));
        press.confirmed = true;
        assert!(press.confirms_window_instance(owner, "fullwnd"));
        assert!(!press.confirms_window_instance(Some(("fullwnd", 13)), "fullwnd"));
        assert!(!press.confirms_window_instance(Some(("novelwnd", 12)), "fullwnd"));
        press.lifetime.as_mut().unwrap().observe(12, &[task_creation(12, 925)], 0);
        assert!(!press.confirms_window_instance(owner, "fullwnd"));
    }

    #[test]
    fn novel_to_full_window_requires_a_later_terminal_probe_without_input() {
        let full = json!({"state": "page_probe", "scene": "com01", "group": 32, "record": 247, "window": "fullwnd"});
        let mut route = json!({
            "label": "full-window successor", "idle_frames": 3600, "page_scenes": ["com01"], "page_fallback_before": 0,
            "steps": [
                {"label": "press", "wait": {"state": "page", "scene": "com01", "group": 31, "record": 226,
                    "window": "novelwnd", "hit": {"point": [557, 324], "region": 73}}, "click": [557, 324],
                    "mouse_press": {"candidates": [3, 73, 43, 7], "region": 43, "key": 0, "group": 0, "response": 1},
                    "after": full.clone()},
                {"label": "stop", "wait": full.clone(), "probe_points": [[557, 324]]}
            ]
        });
        let validate = |route| Controller::from_route(serde_json::from_value::<Route>(route).unwrap());
        let controller = validate(route.clone()).unwrap();
        let after = controller.route.steps[0].after.as_ref().unwrap();
        let mut position = Position { state: "page_probe", scene: "com01".into(), group: 32, record: 247 };
        assert!(after.matches_position(&position, Some(("fullwnd", 12))));
        assert!(!after.matches_position(&position, Some(("novelwnd", 15))));
        position.state = "active";
        assert!(!after.matches_position(&position, Some(("fullwnd", 12))));
        route["steps"][1]["wait"]["record"] = json!(248);
        assert!(validate(route.clone()).is_err());
        route["steps"][1]["wait"] = full;
        route["steps"][0]["wait"]["record"] = json!(247);
        assert!(validate(route.clone()).is_err());
        route["steps"][0]["wait"]["record"] = json!(226);
        let extra = route["steps"][1].clone();
        route["steps"].as_array_mut().unwrap().push(extra);
        assert!(validate(route).is_err());
    }

    #[test]
    fn neko_cold_start_confirms_only_the_explicit_language_then_probes_title() {
        let scenario: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../../scripts/analysis/pc_story_neko_startup_segments_candidate.json"
        )).unwrap();
        let validate = |route| Controller::from_route(serde_json::from_value::<Route>(route).unwrap());
        for language in ["jp", "cn"] {
            let route = &scenario["startup_title_probe"]["routes"][language];
            let controller = validate(route.clone()).unwrap();
            assert!(controller.route.steps[0].is_neko_startup_press());
            assert!(controller.route.steps[1].click.is_none() && controller.route.steps[1].key.is_none());
            for (field, value) in [("neko_startup_language", json!(null)), ("neko_startup_language", json!("en")), ("mouse_press", json!(null))] {
                let mut invalid = route.clone();
                invalid["steps"][0][field] = value;
                assert!(validate(invalid).is_err(), "{field}");
            }
            let mut invalid = route.clone();
            invalid["steps"][1]["wait"]["window"] = json!("langselect");
            assert!(validate(invalid).is_err());
        }
    }

    #[test]
    fn island_exact_window_segments_link_in_both_directions() {
        let scenario: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../../scripts/analysis/pc_story_island_novel_45_prompt_to_fullwnd_46_probe.json"
        )).unwrap();
        let mut route = scenario["3"]["route"].clone();
        let validate = |route| Controller::from_route(serde_json::from_value::<Route>(route).unwrap());
        assert!(validate(route.clone()).is_ok());

        let novel = |group, record| json!({"state":"page","scene":"com01","group":group,"record":record,"window":"novelwnd"});
        let full = |group, record| json!({"state":"page_probe","scene":"com01","group":group,"record":record,"window":"fullwnd"});
        let press = |mut wait: serde_json::Value, after, region| {
            wait["hit"] = json!({"point":[557,324],"region":73});
            json!({"label":"guarded press","wait":wait,"after":after,"click":[557,324],
                "mouse_press":{"candidates":[3,73,region,7],"region":region,"key":0,"group":0,"response":1}})
        };
        let steps = route["steps"].as_array_mut().unwrap();
        steps.pop();
        let mut same_full = press(full(46, 401), full(47, 415), 14);
        same_full["same_full_window"] = json!(true);
        steps.extend([
            same_full,
            press(full(47, 415), novel(48, 428), 14),
            press(novel(48, 428), full(50, 460), 43),
            json!({"label":"observe only","wait":full(50, 460)}),
        ]);
        assert!(validate(route.clone()).is_ok());
        for (step, field, value) in [(25, "mouse_press", json!(null)), (26, "mouse_press", json!(null)), (27, "key", json!(13))] {
            let mut invalid = route.clone();
            invalid["steps"][step][field] = value;
            assert!(validate(invalid).is_err(), "{step} {field}");
        }
        route["steps"][26]["wait"]["record"] = json!(429);
        assert!(validate(route).is_err());
    }

    #[test]
    fn novel_to_full_window_can_continue_once_to_an_exact_novel_stop() {
        let scenario: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../../scripts/analysis/pc_story_island_fullwnd_32_once.json"
        )).unwrap();
        let route = scenario["3"]["route"].clone();
        let validate = |route| Controller::from_route(serde_json::from_value::<Route>(route).unwrap());
        let controller = validate(route.clone()).unwrap();
        let full = &controller.route.steps[7];
        let after = full.after.as_ref().unwrap();
        let mut position = Position { state: "page_probe", scene: "com01".into(), group: 32, record: 247 };
        assert!(full.wait.matches_position(&position, Some(("fullwnd", 12))));
        assert!(!full.wait.matches_position(&position, Some(("novelwnd", 15))));
        position = Position { state: "page", scene: "com01".into(), group: 33, record: 258 };
        assert!(after.matches_position(&position, Some(("novelwnd", 15))));
        assert!(!after.matches_position(&position, Some(("fullwnd", 12))));
        position.state = "active";
        assert!(!after.matches_position(&position, Some(("novelwnd", 15))));

        for (step, field, value) in [
            (7, "window", json!(null)),
            (7, "record", json!(248)),
            (8, "record", json!(259)),
            (8, "scene", json!("other")),
        ] {
            let mut invalid = route.clone();
            invalid["steps"][step]["wait"][field] = value;
            assert!(validate(invalid).is_err());
        }
        let mut invalid = route.clone();
        invalid["steps"][7].as_object_mut().unwrap().remove("mouse_press");
        assert!(validate(invalid).is_err());
        let mut invalid = route;
        invalid["page_fallback_before"] = json!(7);
        assert!(validate(invalid).is_err());
    }

    #[test]
    fn novel_segment_requires_an_exact_guarded_chain_and_an_input_free_end() {
        let scenario: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../../scripts/analysis/pc_story_island_novel_33_segment.json"
        )).unwrap();
        let route = scenario["3"]["route"].clone();
        let validate = |route| Controller::from_route(serde_json::from_value::<Route>(route).unwrap());
        let controller = validate(route.clone()).unwrap();
        assert!(controller.route.steps[8..11].iter().all(|step| step.same_novel_window));
        let last = &controller.route.steps[11];
        assert!(last.click.is_none() && last.key.is_none());
        assert_eq!(last.wait.record, Some(270));
        for (index, field, value) in [
            (8, "same_novel_window", json!(false)),
            (9, "mouse_press", json!(null)),
            (10, "click", json!(null)),
            (11, "same_novel_window", json!(true)),
        ] {
            let mut invalid = route.clone();
            invalid["steps"][index][field] = value;
            assert!(validate(invalid).is_err());
        }
        for (index, field, value) in [
            (9, "state", json!("page_prompt")),
            (9, "window", json!("fullwnd")),
            (11, "record", json!(271)),
        ] {
            let mut invalid = route.clone();
            invalid["steps"][index]["wait"][field] = value;
            assert!(validate(invalid).is_err());
        }
    }

    #[test]
    fn island_novel_to_meswnd_requires_exact_receipt_and_input_free_terminal_probe() {
        let scenario: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../../scripts/analysis/pc_story_island_novel_52_to_meswnd_53_probe.json"
        )).unwrap();
        let route = scenario["3"]["route"].clone();
        let validate = |route| Controller::from_route(serde_json::from_value::<Route>(route).unwrap());
        let controller = validate(route.clone()).unwrap();
        let last = controller.route.steps.len() - 1;
        assert!(controller.route.steps[last - 2].same_novel_window);
        assert!(controller.route.steps[last - 1].is_island_meswnd_probe_exit());
        assert!(controller.route.steps[last].click.is_none() && controller.route.steps[last].key.is_none());

        let mut wrong_wait = route.clone();
        wrong_wait["steps"][last - 1]["after"]["record"] = json!(515);
        wrong_wait["steps"][last]["wait"]["record"] = json!(515);
        assert!(validate(wrong_wait).is_err());
        let mut wrong_receipt = route.clone();
        wrong_receipt["steps"][last - 1]["mouse_press"]["response"] = json!(0);
        assert!(validate(wrong_receipt).is_err());
        let mut missing_probe = route.clone();
        missing_probe["steps"][last]["probe_points"] = json!([[557,324]]);
        assert!(validate(missing_probe).is_err());
        let mut fallback = route.clone();
        fallback["page_fallback_before"] = json!(last);
        assert!(validate(fallback).is_err());
        let mut not_terminal = route;
        not_terminal["steps"].as_array_mut().unwrap().push(json!({"label":"later stop",
            "wait":{"state":"page","scene":"com01","group":54,"record":520,"window":"meswnd"}}));
        assert!(validate(not_terminal).is_err());
    }

    #[test]
    fn island_meswnd_single_press_requires_an_exact_guarded_successor() {
        let scenario: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../../scripts/analysis/pc_story_island_meswnd_53_to_54_probe.json"
        )).unwrap();
        let route = scenario["3"]["route"].clone();
        let validate = |route| Controller::from_route(serde_json::from_value::<Route>(route).unwrap());
        let controller = validate(route.clone()).unwrap();
        let last = controller.route.steps.len() - 1;
        assert_eq!(controller.route.steps[last - 1].island_meswnd_edge(), Some(((53, 514), (54, 518))));
        assert!(controller.route.steps[last].click.is_none());
        for (field, value) in [("region", json!(43)), ("response", json!(1)), ("key", json!(0))] {
            let mut wrong = route.clone();
            wrong["steps"][last - 1]["mouse_press"][field] = value;
            assert!(validate(wrong).is_err(), "{field}");
        }
        let mut skipped = route;
        skipped["steps"][last - 1]["after"]["record"] = json!(522);
        skipped["steps"][last]["wait"]["record"] = json!(522);
        assert!(validate(skipped).is_err());
    }

    #[test]
    fn novel_segment_allows_only_a_guarded_terminal_same_record_prompt() {
        let scenario: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../../scripts/analysis/pc_story_island_novel_35_prompt_probe.json"
        )).unwrap();
        let route = scenario["3"]["route"].clone();
        let validate = |route| Controller::from_route(serde_json::from_value::<Route>(route).unwrap());
        let controller = validate(route.clone()).unwrap();
        let first_stage = &controller.route.steps[10];
        let terminal = &controller.route.steps[11];
        assert!(first_stage.same_novel_window && first_stage.expects_same_novel_prompt());
        assert!(terminal.click.is_none() && terminal.key.is_none());
        assert_eq!(terminal.wait.record, Some(266));

        for (index, field, value) in [
            (9, "same_novel_window", json!(false)),
            (10, "same_novel_window", json!(false)),
            (10, "mouse_press", json!(null)),
            (11, "click", json!([557, 324])),
        ] {
            let mut invalid = route.clone();
            invalid["steps"][index][field] = value;
            assert!(validate(invalid).is_err(), "{index}: {field}");
        }
        for (field, value) in [
            ("record", json!(267)),
            ("state", json!("page_or_prompt")),
            ("window", json!("fullwnd")),
        ] {
            let mut invalid = route.clone();
            invalid["steps"][10]["after"][field] = value.clone();
            invalid["steps"][11]["wait"][field] = value;
            assert!(validate(invalid).is_err(), "{field}");
        }
        let mut invalid = route.clone();
        invalid["page_fallback_before"] = json!(11);
        assert!(validate(invalid).is_err());
        let mut invalid = route;
        let steps = invalid["steps"].as_array_mut().unwrap();
        steps.push(steps[11].clone());
        assert!(validate(invalid).is_err());
    }

    #[test]
    fn guarded_prompt_continuation_requires_an_explicit_second_press_and_a_later_checkpoint() {
        let scenario: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../../scripts/analysis/pc_story_island_novel_35_prompt_once.json"
        )).unwrap();
        let route = scenario["3"]["route"].clone();
        let validate = |route| Controller::from_route(serde_json::from_value::<Route>(route).unwrap());
        let controller = validate(route.clone()).unwrap();
        let steps = &controller.route.steps;
        assert!(steps[10].continue_same_record_prompt && steps[10].expects_same_novel_prompt());
        assert!(steps[11].same_novel_window && !steps[11].continue_same_record_prompt);
        assert!(steps[12].click.is_none() && steps[12].key.is_none());
        let window = Some(("novelwnd", 15));
        let mut position = Position { state: "page", scene: "com01".into(), group: 35, record: 266 };
        assert!(!steps[11].wait.matches_position(&position, window));
        position.state = "page_prompt";
        assert!(steps[11].wait.matches_position(&position, window));
        assert!(!steps[11].wait.matches_position(&position, Some(("fullwnd", 15))));
        assert!(!steps[12].wait.matches_position(&position, window));
        position.group = 36;
        position.record = 270;
        assert!(steps[12].wait.matches_position(&position, window));
        position.state = "page";
        assert!(steps[12].wait.matches_position(&position, window));
        position.state = "active";
        assert!(!steps[12].wait.matches_position(&position, window));

        let expected = steps[11].mouse_press.as_ref().unwrap();
        let mut receipt = InputRouteReceipt {
            frame: 899, key: 0, group: 0, raw_mask: 2, pressed: 1, repeated: false,
            region: 43, receiver: 15, response: 1,
            physical: vec![RoutedPhysicalButton { source: ButtonSource::Mouse, code: 1 }],
        };
        assert!(!expected.matches_receipt(&receipt, 903, 15));
        receipt.frame = 904;
        assert!(expected.matches_receipt(&receipt, 903, 15));

        for (index, field, value) in [
            (10, "continue_same_record_prompt", json!(false)),
            (10, "same_novel_window", json!(false)),
            (11, "same_novel_window", json!(false)),
            (11, "mouse_press", json!(null)),
            (11, "continue_same_record_prompt", json!(true)),
            (11, "after", route["steps"][10]["after"].clone()),
        ] {
            let mut invalid = route.clone();
            invalid["steps"][index][field] = value;
            assert!(validate(invalid).is_err(), "{index}: {field}");
        }
        for (index, field, value) in [
            (11, "record", json!(267)),
            (11, "state", json!("page")),
            (11, "hit", json!(null)),
            (12, "record", json!(271)),
        ] {
            let mut invalid = route.clone();
            invalid["steps"][index]["wait"][field] = value;
            assert!(validate(invalid).is_err(), "{index}: {field}");
        }
        let mut invalid = route;
        invalid["page_fallback_before"] = json!(12);
        assert!(validate(invalid).is_err());
    }

    #[test]
    fn shared_flag_checks_only_requested_bits_and_rejects_a_missing_slot() {
        let mut flag = SharedFlag { slot: 722, mask: 0b10, expected: 0 };
        assert!(flag.matches(Some(0b01)));
        assert!(!flag.matches(Some(0b10)));
        assert!(!flag.matches(None));
        flag.expected = 0b10;
        assert!(flag.matches(Some(0b11)));
        assert!(!flag.matches(Some(0b01)));
    }

    #[test]
    fn guarded_press_receipt_requires_fresh_mouse_down_on_the_live_owner() {
        let expected = MousePress { candidates: vec![3, 73, 14, 7], region: 14, key: 0, group: 0, response: 1 };
        let receipt = InputRouteReceipt {
            frame: 804, key: 0, group: 0, raw_mask: 2, pressed: 1, repeated: false,
            region: 14, receiver: 12, response: 1,
            physical: vec![RoutedPhysicalButton { source: ButtonSource::Mouse, code: 1 }],
        };
        assert!(expected.matches_receipt(&receipt, 803, 12));
        assert!(!expected.matches_receipt(&receipt, 804, 12));
        assert!(!expected.matches_receipt(&receipt, 800, 12));
        assert!(!expected.matches_receipt(&receipt, 803, 8));
        for wrong in [
            InputRouteReceipt { pressed: 0, ..receipt.clone() },
            InputRouteReceipt { repeated: true, ..receipt.clone() },
            InputRouteReceipt { region: 73, ..receipt.clone() },
            InputRouteReceipt { key: 2, ..receipt.clone() },
            InputRouteReceipt { group: 1, ..receipt.clone() },
            InputRouteReceipt { response: 0, ..receipt.clone() },
            InputRouteReceipt { physical: vec![RoutedPhysicalButton { source: ButtonSource::Keyboard, code: 13 }], ..receipt.clone() },
        ] {
            assert!(!expected.matches_receipt(&wrong, 803, 12));
        }
    }
}

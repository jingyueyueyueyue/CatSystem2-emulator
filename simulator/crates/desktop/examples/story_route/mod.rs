mod campanella;
mod daydream_case03_end1;
mod grisaia;
mod hatsumira;
mod island_bad02;
mod kamikaze;
pub(crate) use kamikaze::Opening as NaturalOpening;
mod kanmi_ordinary;
mod neko_ordinary;
mod rebellions;

use super::Action;
use anyhow::{Context, Result, ensure};
use cs2_desktop::{Eye08Body, HeadlessPlayer, LogicEvent};
use cs2_runtime::InputKey;
use cs2_runtime::message_object::{ButtonSource, InputRouteReceipt, RoutedPhysicalButton};
use serde::Deserialize;
use std::collections::BTreeSet;
use std::path::Path;

type StoryWindow = (&'static str, u32);
type IslandCstAnchor = (u32, u64, u32);
type DaydreamBodyAnchor = (u32, u64, u32, u64, u64);

const ISLAND_MESWND_POINTS: [[i32; 2]; 2] = [[557, 530], [557, 324]];

const DAYDREAM_PROLOGUE_WAITS: [i32; 41] = [
    18, 22, 25, 28, 31, 33, 35, 37, 40, 44, 47, 55, 57, 62, 64, 75, 77, 79, 81, 83,
    96, 107, 113, 117, 120, 122, 124, 129, 131, 145, 151, 153, 158, 163, 174, 179, 183,
    186, 199, 205, 221,
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
    fn observe(player: &HeadlessPlayer, kanmi: bool) -> (Self, Option<StoryWindow>) {
        if kanmi {
            let Some(((state, scene, group, record), window)) = player.host_replay_kanmi_position() else {
                return (Self { state: "unknown", scene: "-".into(), group: -1, record: -1 }, None);
            };
            return (Self { state, scene: scene.into(), group, record }, Some(window));
        }
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
        const OPENING_WAITS: [i32; 26] = [
            59, 87, 92, 98, 103, 112, 117, 122, 127, 136, 141, 146,
            151, 156, 160, 165, 170, 175, 180, 185, 190, 195, 200, 213, 218, 337,
        ];
        let group = usize::try_from(self.group?).ok()?;
        (OPENING_WAITS.get(group).copied()? == self.record?).then_some(group)
    }

    fn daydream_opening_page(&self) -> Option<usize> {
        if self.is_daydream_r96_probe() {
            return Some(20);
        }
        if self.state != "page" || self.scene.as_deref() != Some("prologue") || self.window.as_deref() != Some("cinema") {
            return None;
        }
        let group = usize::try_from(self.group?).ok()?;
        (group != 20 && DAYDREAM_PROLOGUE_WAITS.get(group).copied()? == self.record?).then_some(group)
    }

    fn is_daydream_r96_probe(&self) -> bool {
        self.state == "page_probe" && self.scene.as_deref() == Some("prologue")
            && self.window.as_deref() == Some("cinema") && self.group == Some(20)
            && self.record == Some(96) && self.hit.is_none()
    }

    fn is_daydream_title(&self) -> bool {
        self.state == "title" && self.scene.as_deref() == Some("-") && self.group == Some(-1)
            && self.record == Some(-1) && self.window.as_deref() == Some("title") && self.hit.is_none()
    }

    fn island_meswnd_page(&self) -> Option<(i32, i32)> {
        let (group, record) = (self.group?, self.record?);
        let valid_position = match self.scene.as_deref() {
            Some("com01") => (53..=121).contains(&group) && (514..=810).contains(&record)
                && (group != 53 || record == 514) && (group != 121 || record == 810),
            Some("OPMOVIE_END") => (0..=332).contains(&group) && (19..=2063).contains(&record),
            Some("com01_selA_1") => (0..=22).contains(&group) && (19..=94).contains(&record),
            Some("com01_flowA_1" | "com01_flowB_1") => (group, record) == (0, 1),
            Some("com01_flowA") => (0..=2).contains(&group) && record == group * 2 + 1,
            Some("com01_selB_1") => (0..=51).contains(&group) && (26..=361).contains(&record),
            _ => false,
        };
        (self.state == "page" && self.window.as_deref() == Some("meswnd") && valid_position)
            .then_some((group, record))
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
            && (after.daydream_opening_page() == Some(page + 1) || self.is_daydream_prologue_exit())
            && (press.region, press.receiver, press.key, press.group, press.response) == (62, 8, 2, 0, 0)
    }

    fn is_daydream_prologue_exit(&self) -> bool {
        self.wait.daydream_opening_page() == Some(40)
            && self.after.as_ref().is_some_and(Condition::is_daydream_title)
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

    fn is_neko_save_entry(&self) -> bool {
        self.wait.neko_opening_page() == Some(24)
            && self.click == Some([1147, 612]) && self.key.is_none()
            && self.wait.hit.as_ref().is_some_and(|hit| hit.point == [1147, 612] && hit.region == 45)
            && self.after.as_ref().is_some_and(|after| {
                after.state == "neko_save_panel" && after.scene.as_deref() == Some("01")
                    && after.group == Some(24) && after.record == Some(218)
                    && after.window.as_deref() == Some("saveload") && after.hit.is_none()
            })
            && self.mouse_press.as_ref().is_some_and(|press| {
                press.candidates == [3, 45, 77, 52, 57, 7]
                    && (press.region, press.key, press.group, press.response) == (45, 0, 0, 0)
            })
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

    fn is_island_beach_fullwnd_exit(&self) -> bool {
        let (Some(after), Some(press)) = (&self.after, &self.mouse_press) else { return false; };
        self.wait.state == "page_probe" && self.wait.scene.as_deref() == Some("com01_selB_1")
            && self.wait.window.as_deref() == Some("fullwnd")
            && (self.wait.group, self.wait.record) == (Some(0), Some(26))
            && after.scene == self.wait.scene && after.island_meswnd_page() == Some((1, 67))
            && after.hit.is_none() && self.click == Some(ISLAND_MESWND_POINTS[1])
            && self.probe_points == ISLAND_MESWND_POINTS
            && self.wait.hit.as_ref().is_some_and(|hit| hit.point == ISLAND_MESWND_POINTS[1] && hit.region == 108)
            && press.candidates == [3, 108, 24, 7]
            && (press.region, press.key, press.group, press.response) == (24, 0, 0, 1)
            && !self.same_full_window && !self.same_novel_window && self.key.is_none()
    }

    fn island_meswnd_press_origin(&self) -> Option<(i32, i32)> {
        let press = self.mouse_press.as_ref()?;
        let origin = self.wait.island_meswnd_page()?;
        let region = match self.wait.scene.as_deref() {
            Some("OPMOVIE_END") if origin.0 >= 136 => 108,
            Some("OPMOVIE_END") => 81,
            Some("com01") => 82,
            _ => 108,
        };
        let matches_candidates = if self.wait.scene.as_deref() == Some("com01_selB_1") {
            press.candidates == [3, 108, 61, 7]
        } else {
            press.candidates == [3, region, 7]
        };
        (!self.same_novel_window && !self.same_full_window && self.key.is_none()
            && self.click == Some(ISLAND_MESWND_POINTS[1]) && self.probe_points == ISLAND_MESWND_POINTS
            && self.wait.hit.as_ref().is_some_and(|hit| hit.point == ISLAND_MESWND_POINTS[1] && hit.region == region)
            && matches_candidates && (press.region, press.key, press.group, press.response) == (region, 2, 0, 0))
            .then_some(origin)
    }

    fn island_meswnd_edge(&self) -> Option<((i32, i32), (i32, i32))> {
        let origin = self.island_meswnd_press_origin()?;
        let after = self.after.as_ref()?;
        let target = after.island_meswnd_page()?;
        (self.wait.scene == after.scene && target.0 == origin.0 + 1 && target.1 > origin.1
            && (self.wait.scene.as_deref() != Some("com01") || origin.0 != 53 || target == (54, 518))
            && after.hit.is_none())
            .then_some((origin, target))
    }

    fn rebuilds_island_meswnd(&self) -> bool {
        self.wait.scene.as_deref() == Some("OPMOVIE_END")
            && self.island_meswnd_edge() == Some(((135, 906), (136, 931)))
    }

    fn is_island_opmovie_exit(&self) -> bool {
        self.wait.scene.as_deref() == Some("com01") && self.island_meswnd_press_origin() == Some((121, 810))
            && matches!(self.shared_flags.as_slice(), [SharedFlag { slot: 849, mask: u32::MAX, expected: 0 }])
            && self.after.as_ref().is_some_and(|after| {
                after.state == "page" && after.scene.as_deref() == Some("OPMOVIE_END")
                    && after.group == Some(0) && after.record == Some(19)
                    && after.window.as_deref().is_none_or(|window| window == "meswnd") && after.hit.is_none()
            })
    }

    fn is_island_beach_choice_entry(&self) -> bool {
        self.wait.scene.as_deref() == Some("com01_selB_1")
            && self.island_meswnd_press_origin() == Some((51, 361))
            && self.after.as_ref().is_some_and(|after| {
                after.state == "choice" && after.scene.as_deref() == Some("com01_selC")
                    && after.window.as_deref() == Some("select") && after.group.is_none()
                    && after.record.is_none() && after.hit.is_none()
            })
    }

    fn is_island_first_play_branch(&self) -> bool {
        let Some(origin) = self.island_meswnd_press_origin() else { return false; };
        let Some(after) = &self.after else { return false; };
        let flags = self.shared_flags.as_slice();
        let first_play = matches!(flags, [
            SharedFlag { slot: 722, mask: u32::MAX, expected: 0 },
            SharedFlag { slot: 525, mask: u32::MAX, expected: 0 },
        ]);
        let branch_locked = matches!(flags, [SharedFlag { slot: 525, mask: 16, expected: 0 }]);
        let (target, record) = match (self.wait.scene.as_deref(), origin) {
            (Some("OPMOVIE_END"), (332, 2063)) if first_play => ("com01_selA_1", 19),
            (Some("com01_selA_1"), (22, 94)) if branch_locked => ("com01_flowA_1", 1),
            (Some("com01_flowA_1"), (0, 1)) if flags.is_empty() => ("com01_flowA", 1),
            (Some("com01_flowA"), (2, 5)) if branch_locked => ("com01_flowB_1", 1),
            (Some("com01_flowB_1"), (0, 1)) if branch_locked => {
                return after.scene.as_deref() == Some("com01_selB_1")
                    && after.state == "page_probe" && after.window.as_deref() == Some("fullwnd")
                    && after.group == Some(0) && after.record == Some(26) && after.hit.is_none();
            }
            _ => return false,
        };
        after.scene.as_deref() == Some(target) && after.island_meswnd_page() == Some((0, record))
            && after.hit.is_none()
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
            && self.matches_target(receipt, receiver)
    }

    fn matches_target(&self, receipt: &InputRouteReceipt, receiver: u32) -> bool {
        receipt.key == self.key
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
        {
            *self = Self::Rejected;
            return None;
        }
        let mut resumes = events.iter().rev().take_while(|event| event.frame >= receipt.frame)
            .filter(|event| event.frame == receipt.frame && event.kind == "message-resume");
        if !resumes.next().is_some_and(|event| event.detail == "message=[2, 0, 0, 0]") || resumes.next().is_some() {
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
    #[serde(default)]
    eyecatch_08_exit: bool,
    #[serde(default)]
    eyecatch_08_first_body: bool,
    #[serde(default)]
    eyecatch_08_next_body: bool,
    #[serde(default)]
    daydream_case_start: bool,
    #[serde(default)]
    daydream_case_first_advance: bool,
    daydream_case03_end1: Option<daydream_case03_end1::Plan>,
    #[serde(default)]
    island_bad02: bool,
    kanmi_exact: Option<KanmiExact>,
    kamikaze_post_op: Option<kamikaze::Contract>,
    #[serde(default)]
    grisaia_14_first_advance: bool,
    #[serde(default)]
    campanella_18_first_advance: bool,
    guarded_body_last_group: Option<usize>,
    rebellions_r15: Option<rebellions::Config>,
    hatsumira_r72: Option<hatsumira::Mode>,
    neko_ordinary: Option<neko_ordinary::Config>,
    steps: Vec<Step>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct KanmiExact {
    contract: String,
    startup: String,
    runtime_pe_sha256: String,
    flow_fes_sha256: String,
    title_fes_sha256: String,
    scene_cst_sha256: String,
    #[serde(skip)]
    save_state: Option<kanmi_ordinary::Save>,
    #[serde(skip)]
    load_state: Option<kanmi_ordinary::Load>,
}

impl KanmiExact {
    fn sc01_wait_count(&self) -> Option<usize> {
        match self.contract.as_str() {
            "kanmi-title-r43/v1" => Some(2),
            "kanmi-title-r47/v1" => Some(4),
            "kanmi-title-r53/v1" => Some(7),
            "kanmi-title-r80/v1" => Some(9),
            "kanmi-title-r102/v1" => Some(19),
            "kanmi-title-r134/v1" => Some(28),
            "kanmi-title-r256/v1" => Some(52),
            "kanmi-title-r983/v1" => Some(208),
            "kanmi-title-sc02-r10/v1" | "kanmi-title-sc02-r147/v1" | "kanmi-title-sc02-r547/v1"
            | "kanmi-title-sc02-r562/v1" | "kanmi-title-sc02-r1379/v1" | "kanmi-title-sc02-r147-save/v1" => Some(294),
            _ => None,
        }
    }

    fn sc02_wait_count(&self) -> usize {
        match self.contract.as_str() {
            "kanmi-title-sc02-r10/v1" => 1,
            "kanmi-title-sc02-r147/v1" | "kanmi-title-sc02-r147-save/v1" => 26,
            "kanmi-title-sc02-r547/v1" => 101,
            "kanmi-title-sc02-r562/v1" => 102,
            "kanmi-title-sc02-r1379/v1" => 265,
            _ => 0,
        }
    }

    fn load_terminal_group(&self) -> Option<usize> {
        match self.contract.as_str() {
            "kanmi-load-sc02-r147/v1" => Some(26),
            "kanmi-load-sc02-r562/v1" => Some(101),
            "kanmi-load-sc02-r1379/v1" | "kanmi-load-sc02-post-op/v1" => Some(264),
            _ => None,
        }
    }

    fn matches(&self) -> bool {
        (self.sc01_wait_count().is_some() || self.load_terminal_group().is_some()) && self.startup == "kcs.int/main.kcs"
            && self.runtime_pe_sha256 == "c264b2465889c78723079a694d2654ef1091253db657a735e92f58b75031aa26"
            && self.flow_fes_sha256 == "8a22b583e1e6dca573c2cbc56097a2134a5f3a3c21a4075fea077bd1c1fa5fea"
            && self.title_fes_sha256 == "b737c7557f43be7c399173cd181532dc6af01cbe4f6ebf5cbda3df5861ab40fb"
            && self.scene_cst_sha256 == "7c0d796d069db416291effc217aa3664ba449f23ce43b88a11a5d13cfff1a55e"
    }
}

impl Route {
    fn allows_page_fallback(&self, next: usize, scene: &str) -> bool {
        self.page_fallback_before.is_none_or(|end| next < end) && self.page_scenes.contains(scene)
    }

    fn valid_kanmi_steps(&self, contract: &KanmiExact) -> bool {
        let Some(count) = contract.sc01_wait_count() else { return false; };
        let Some(sc01) = HeadlessPlayer::host_kanmi_scene_waits("sc01").get(..count) else {
            return false;
        };
        let Some(sc02) = HeadlessPlayer::host_kanmi_scene_waits("sc02").get(..contract.sc02_wait_count()) else {
            return false;
        };
        let waits: Vec<_> = [("sc01", sc01), ("sc02", sc02)].into_iter().flat_map(|(scene, waits)| {
            waits.iter().enumerate().map(move |(group, &record)| (scene, group as i32, record))
        }).collect();
        let [title, body @ .., stop] = self.steps.as_slice() else { return false; };
        if body.len() + 1 != waits.len() {
            return false;
        }
        let page = |condition: &Condition, (scene, group, record)| {
            condition.state == "page" && condition.scene.as_deref() == Some(scene)
                && condition.group == Some(group) && condition.record == Some(record)
                && condition.window.as_deref() == Some("meswnd") && condition.hit.is_none()
        };
        self.page_scenes.is_empty() && self.page_click.is_none() && self.page_fallback_before.is_none()
            && self.steps.iter().all(|step| !step.label.is_empty() && !step.same_novel_window
                && !step.same_full_window && !step.continue_same_record_prompt && step.neko_startup_language.is_none())
            && title.wait.state == "title" && title.wait.window.as_deref() == Some("title")
            && title.wait.scene.is_none() && title.wait.group.is_none() && title.wait.record.is_none()
            && title.wait.hit.as_ref().is_some_and(|hit| hit.point == [140, 354] && hit.region == 14)
            && title.click == Some([140, 354]) && title.key.is_none() && title.keyboard_press.is_none()
            && title.mouse_press.as_ref().is_some_and(|press| press.candidates == [3, 14, 23, 7]
                && (press.region, press.key, press.group, press.response) == (14, 0, 0, 0))
            && title.after.as_ref().is_some_and(|after| page(after, waits[0]))
            && body.iter().zip(waits.windows(2)).all(|(step, edge)| {
                page(&step.wait, edge[0]) && step.click.is_none() && step.mouse_press.is_none()
                    && step.key == Some(13) && step.keyboard_press.as_ref().is_some_and(|press| {
                        (press.key, press.group, press.response) == (2, 0, 0)
                    })
                    && step.after.as_ref().is_some_and(|after| page(after, edge[1]))
            })
            && waits.last().is_some_and(|&position| page(&stop.wait, position))
            && stop.after.is_none() && stop.click.is_none()
            && stop.key.is_none() && stop.mouse_press.is_none() && stop.keyboard_press.is_none()
    }
}

struct Pending {
    origin: Position,
    started: u64,
    step: Option<usize>,
    mouse_press: Option<PendingMousePress>,
    keyboard_press: KeyboardPressReceipt,
    island_meswnd: Option<IslandMeswndPress>,
    kanmi_body: Option<KanmiBodyPress>,
    daydream_body: Option<DaydreamBodyAnchor>,
    daydream_tail: Option<DaydreamTailEvidence>,
    neko_save_anchor: Option<(u32, u64, u32, u64, u64)>,
}

#[derive(Debug)]
struct DaydreamTailEvidence {
    receiver: u32,
    view_frame: Option<u64>,
    window: Option<(u32, u64, u64)>,
    fesds_frame: Option<u64>,
    prologue_eof_frame: Option<u64>,
}

impl DaydreamTailEvidence {
    fn observe(&mut self, started: u64, batch: &[LogicEvent]) {
        for event in batch.iter().filter(|event| event.frame > started) {
            if event.kind == "native-call" && event.detail.starts_with("index=1040 ")
                && event.detail.ends_with(r#" debug="[command] view 0 default""#)
            {
                if self.view_frame != Some(event.frame) {
                    self.view_frame = Some(event.frame);
                    self.window = None;
                    self.fesds_frame = None;
                    self.prologue_eof_frame = None;
                }
            } else if event.kind == "native-call" && event.detail.starts_with("index=1040 ")
                && event.detail.ends_with(r#" debug="[command] fesds""#)
                && self.window.is_some_and(|(_, _, created)| event.frame >= created)
            {
                self.fesds_frame.get_or_insert(event.frame);
            } else if event.kind == "native-call" && event.detail.starts_with("index=1 ")
                && event.detail.ends_with(r#" debug="GetLine error!! [prologue(41,53)]""#)
                && self.fesds_frame.is_some_and(|issued| issued <= event.frame)
            {
                self.prologue_eof_frame.get_or_insert(event.frame);
            }
            let Some(view_frame) = self.view_frame else { continue; };
            if event.kind != "task-create" || event.frame < view_frame {
                continue;
            }
            let mut fields = event.detail.split_ascii_whitespace();
            let Some(owner) = fields.next().and_then(|word| word.strip_prefix("id="))
                .and_then(|id| id.parse::<u32>().ok()) else { continue; };
            let kind = fields.next();
            let parent = fields.next().and_then(|word| word.strip_prefix("parent="))
                .and_then(|id| id.parse::<u32>().ok());
            let name = fields.next();
            let is_default = (kind, parent, name)
                == (Some("kind=34"), Some(self.receiver), Some(r#"name="meswnd""#));
            if let Some((window, _, created_frame)) = self.window {
                // A recycled task ID needs a new view command.
                if owner == window && (!is_default || event.frame != created_frame) {
                    self.view_frame = None;
                    self.window = None;
                    self.fesds_frame = None;
                    self.prologue_eof_frame = None;
                }
            } else if is_default {
                self.window = Some((owner, view_frame, event.frame));
            }
        }
    }
}

const DAYDREAM_CASE_PATHS: [[&str; 6]; 3] = [
    ["start", "c00_00", "brunch", "brunch_A", "c01_01", "c01_01_01"],
    ["start", "c00_00", "brunch", "brunch_A", "c02_01", "c02_01_01"],
    ["start", "c00_00", "brunch", "brunch_A", "c03_01", "c03_01_01"],
];

struct DaydreamCasePress {
    started: u64,
    title: u32,
    region: u32,
    receipt: Option<u64>,
    loads: Vec<String>,
    unexpected: bool,
    turn: Option<DaydreamCaseTurn>,
    ending: Option<daydream_case03_end1::Replay>,
    cold: Option<daydream_case03_end1::ordinary::Load>,
}

struct DaydreamCaseTurn {
    started: u64,
    anchor: DaydreamBodyAnchor,
    expected: KeyboardPress,
    receipt: KeyboardPressReceipt,
}

impl DaydreamCasePress {
    fn observe_receipts(&mut self, player: &HeadlessPlayer) {
        let receipts = player.host_input_route_receipts();
        if let Some(turn) = &mut self.turn {
            let consumers = receipts.iter().filter(|receipt| {
                receipt.frame > turn.started && receipt.pressed != 0 && receipt.response == 0
            }).count();
            if consumers > 0 {
                let events = player.events();
                let verified = turn.receipt.observe(&turn.expected, turn.started, receipts, events);
                if let Some(receipt) = verified.filter(|receipt| consumers == 1
                    && events.iter().rev().take_while(|event| event.frame >= receipt.frame).any(|event| {
                        event.frame == receipt.frame && event.kind == "input-wait"
                            && event.detail == "native 71 address=0xd6d"
                    }))
                {
                    eprintln!("frame {}: Daydream verified CASE Enter and VM resume {receipt:?}", receipt.frame);
                } else {
                    turn.receipt = KeyboardPressReceipt::Rejected;
                }
            }
            return;
        }
        for receipt in player.host_input_route_receipts().iter().filter(|receipt| {
            receipt.frame > self.started && receipt.pressed != 0 && receipt.response <= 1
        }) {
            let target = (receipt.region, receipt.receiver) == (self.region, self.title);
            let physical_left = receipt.frame - self.started <= 3
                && receipt.key == 0 && receipt.group == 0 && receipt.pressed == 1 && !receipt.repeated
                && receipt.raw_mask == ButtonSource::Mouse.mask()
                && receipt.physical == [RoutedPhysicalButton { source: ButtonSource::Mouse, code: 1 }];
            if target && physical_left {
                self.unexpected |= self.receipt.replace(receipt.frame).is_some();
                eprintln!("frame {}: Daydream verified title left press {receipt:?}", receipt.frame);
            } else if target || receipt.response == 0 {
                self.unexpected = true;
            }
        }
    }

    fn observe_events(&mut self, events: &[LogicEvent]) {
        for event in events {
            if event.frame <= self.started || event.kind != "native-call" {
                continue;
            }
            if event.detail.contains(" debug=\"[command] fselect") {
                self.unexpected = true;
            }
            if let Some(scene) = event.detail.split_once(" debug=\"sscript loaded > ")
                .and_then(|(_, value)| value.strip_suffix('"'))
            {
                self.loads.push(scene.to_owned());
                if !DAYDREAM_CASE_PATHS.iter().any(|path| {
                    self.loads.len() <= path.len()
                        && self.loads.iter().map(String::as_str).eq(path[..self.loads.len()].iter().copied())
                }) {
                    self.unexpected = true;
                }
            }
        }
    }

    fn poll(
        &mut self, player: &mut HeadlessPlayer, frame: u64, idle: u64, advance: bool,
        ending: Option<&daydream_case03_end1::Plan>,
    ) -> Result<Decision> {
        if let (Some(replay), Some(plan)) = (&mut self.ending, ending) {
            return replay.poll(plan, player, frame, idle);
        }
        Ok(self.poll_entry(player, frame, idle, advance, ending))
    }

    fn poll_entry(
        &mut self, player: &HeadlessPlayer, frame: u64, idle: u64, advance: bool,
        ending: Option<&daydream_case03_end1::Plan>,
    ) -> Decision {
        if self.unexpected || player.host_daydream_case_blocked(self.started, self.turn.is_some()) {
            return Decision::Stop("unexpected_branch", format!(
                "unexpected Daydream input route, menu, or CST load after title: {:?}", self.loads,
            ));
        }
        if self.receipt.is_none() {
            return Decision::Stop("input_missed", "missing Daydream title left press receipt".into());
        }
        if self.loads.len() == DAYDREAM_CASE_PATHS[0].len() && let Some(scene) = self.loads.last() {
            if let Some(turn) = &self.turn {
                if turn.receipt != KeyboardPressReceipt::Verified {
                    return Decision::Stop("input_missed", format!("Daydream CASE Enter receipt: {:?}", turn.receipt));
                }
                if let Some((record, (context, load, owner, epoch, source_frame), evidence)) =
                    player.host_daydream_case_body(self.started, scene, 1)
                {
                    let (before_context, before_load, before_owner, before_epoch, _) = turn.anchor;
                    if (context, load, owner) != (before_context, before_load, before_owner)
                        || epoch <= before_epoch || source_frame <= turn.started
                    {
                        return Decision::Stop("checkpoint_mismatch", "Daydream CASE successor is not a fresh same-window body".into());
                    }
                    eprintln!("{evidence}");
                    println!("replay frame={frame} state=page scene={scene} group=1 record={record}");
                    if let Some(plan) = ending {
                        self.ending = Some(daydream_case03_end1::Replay::from_second_wait(plan, &self.loads, scene, frame));
                        return Decision::Wait;
                    }
                    println!("replay frame={frame} checkpoint=\"Daydream: stop at the second CASE body wait\"");
                    return Decision::Done;
                }
            } else if let Some((record, anchor, evidence)) = player.host_daydream_case_body(self.started, scene, 0) {
                eprintln!("{evidence}");
                println!("replay frame={frame} state=page scene={scene} group=0 record={record}");
                if !advance {
                    println!("replay frame={frame} checkpoint=\"Daydream: stop at the first CASE body wait\"");
                    return Decision::Done;
                }
                let (context, _, owner, _, _) = anchor;
                let Some((region, receiver)) = player.host_daydream_case_enter(scene, context, owner) else {
                    return Decision::Stop("checkpoint_mismatch", "Daydream CASE Enter withheld: adjacent CST or live bottom button did not match".into());
                };
                eprintln!("frame {frame}: Daydream CASE Enter scene={scene} owner={owner} region={region} receiver={receiver}");
                self.turn = Some(DaydreamCaseTurn {
                    started: frame, anchor, expected: KeyboardPress { region, receiver, key: 2, group: 0, response: 0 },
                    receipt: KeyboardPressReceipt::Awaiting,
                });
                return Decision::Input(Action::Key(InputKey::Keyboard(13)));
            }
        }
        let started = self.turn.as_ref().map_or(self.started, |turn| turn.started);
        if frame - started >= idle {
            return Decision::Stop("unrecognized_wait", format!(
                "Daydream CASE body wait not confirmed after frame {started}: {:?}", self.loads,
            ));
        }
        Decision::Wait
    }
}

struct KanmiBodyPress {
    owner: u32,
    context: u32,
    load: u64,
    epoch: u64,
    receiver: u32,
    region: u32,
    lifetime: NovelWindowLifetime,
    receiver_lifetime: NovelWindowLifetime,
    region_lifetime: NovelWindowLifetime,
}

fn log_kanmi_body(frame: u64, owner: u32, region: u32, receiver: u32, context: u32,
    load: u64, record: usize, epoch: u64, source_frame: u64,
) {
    println!("frame={frame} kanmi-route-body owner={owner} region={region} receiver={receiver} context={context} load={load} body={record} epoch={epoch} source_frame={source_frame}");
}

struct IslandMeswndPress {
    anchor: IslandCstAnchor,
    window_owner: u32,
    root_lifetime: NovelWindowLifetime,
    saw_opening_movie: bool,
    replacement_window: Option<u32>,
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

type Eye08ExitPoll = Box<dyn FnMut(&HeadlessPlayer) -> Result<bool>>;
const EYE08_WAITS: [i32; 28] = [
    144, 148, 150, 166, 170, 174, 180, 184, 188, 192, 196, 200, 204, 208,
    212, 216, 220, 224, 228, 232, 236, 240, 244, 248, 252, 256, 260, 264,
];

enum Eye08Route {
    Entry { card: bool, since: u64, hovered_owner: Option<u32> },
    EntryPress { card: bool, frame: u64, owner: u32 },
    AwaitEye { since: u64 },
    Hover { frame: u64, owner: u32 },
    Exit { started: u64, poll: Eye08ExitPoll, scene: Option<(u32, u64, u64)> },
    FirstBody { pressed: u64, since: u64, scene: (u32, u64, u64) },
    NextBody {
        group: usize,
        pressed: u64,
        scene: (u32, u64, u64),
        first: Eye08Body,
        region: u32,
        candidates: Vec<(u32, u32)>,
        receipt: KeyboardPressReceipt,
    },
    Done,
    Failed(String),
}

fn eye08_entry(card: bool) -> ([i32; 2], u32, &'static [u32]) {
    if card { ([120, 185], 32, &[32, 6]) } else { ([634, 90], 13, &[13, 22, 6]) }
}

impl Eye08Route {
    fn observe_frame(&mut self, player: &HeadlessPlayer) {
        let next = match self {
            Self::EntryPress { card, frame, owner } => {
                let (point, region, _) = eye08_entry(*card);
                let frame = *frame + 1;
                let press = MousePress { candidates: vec![], region, key: 0, group: 0, response: 0 };
                if consuming_mouse_resume(player, &press, frame, *owner, point, None) {
                    eprintln!("frame {frame}: verified 08 region {region} receiver {owner} and VM resume");
                    if *card { Self::AwaitEye { since: frame } }
                    else { Self::Entry { card: true, since: frame, hovered_owner: None } }
                } else {
                    Self::Failed(format!("missing unique 08 press or VM resume at frame {frame} region {region}"))
                }
            }
            Self::Exit { started, poll, scene } => match poll(player) {
                Ok(false) => return,
                Ok(true) => match *scene {
                    Some(scene) => {
                        let since = player.host_story_trace().0;
                        eprintln!("frame {since}: verified 08 eye EXIT, unbind and release");
                        Self::FirstBody { pressed: *started, since, scene }
                    }
                    None => Self::Done,
                },
                Err(error) => Self::Failed(format!("{error:#}")),
            },
            Self::NextBody { pressed, first, region, candidates, receipt, .. } => {
                let frame = player.host_story_trace().0;
                let routes = player.host_input_route_receipts();
                let positive = routes.iter().filter(|route| route.pressed != 0);
                let rebound = player.events().iter().rev().take_while(|event| event.frame >= frame)
                    .filter(|event| event.frame == frame && event.kind == "task-create")
                    .filter_map(|event| event.detail.split_ascii_whitespace().next())
                    .filter_map(|word| word.strip_prefix("id=")?.parse::<u32>().ok())
                    .any(|id| [first.flow, first.script, first.owner, first.plane, first.string, *region].contains(&id));
                if rebound {
                    Self::Failed("08 guarded task ID was reused after Enter".into())
                } else if *receipt == KeyboardPressReceipt::Awaiting {
                    let enter = KeyboardPress { region: *region, receiver: first.script, key: 2, group: 0, response: 0 };
                    let keyboard = [RoutedPhysicalButton { source: ButtonSource::Keyboard, code: 13 }];
                    if !positive.into_iter().all(|route| {
                        route.frame == frame && route.pressed == 1 && !route.repeated
                            && route.physical == keyboard && route.group == 0 && route.raw_mask == 1
                            && matches!(route.key, 0 | 2) && candidates.contains(&(route.region, route.receiver))
                    }) || receipt.observe(&enter, *pressed, routes, player.events()).is_none() {
                        Self::Failed("08 Enter receipt or root VM resume differs".into())
                    } else {
                        eprintln!("frame {frame}: 08 verified Enter key=2 group=0 region={region} receiver={} and VM resume", first.script);
                        return;
                    }
                } else if positive.count() != 0 {
                    Self::Failed("08 received another press after Enter".into())
                } else {
                    return;
                }
            }
            _ => return,
        };
        *self = next;
    }

    fn poll(&mut self, player: &mut HeadlessPlayer, frame: u64, idle_frames: u64,
        first_body: bool, last_group: usize,
    ) -> Result<Decision> {
        match self {
            Self::Entry { card, since, hovered_owner } => {
                let (point @ [x, y], region, candidates) = eye08_entry(*card);
                if let Some(owner) = player.host_replay_eye08_entry_owner(*card) {
                    if hovered_owner.is_some_and(|previous| previous != owner) {
                        return Ok(Decision::Stop("checkpoint_mismatch", "08 entry owner changed after hover".into()));
                    }
                    if hovered_owner.is_none() || player.host_pointer_logical_position() != point {
                        *hovered_owner = Some(owner);
                        eprintln!("frame {frame}: 08 entry hover {point:?} region={region} owner={owner}");
                        return Ok(Decision::Input(Action::Pointer { x, y, button: None }));
                    }
                    let mask = player.masked_input_region_at(point)?;
                    let live_candidates = player.mouse_press_candidate_regions(point)?;
                    if mask.is_some_and(|words| words[0] == region && words[7] != 0 && words[8] != 0)
                        && live_candidates == candidates
                    {
                        eprintln!("frame {frame}: guarded 08 entry {point:?} region={region} owner={owner}");
                        *self = Self::EntryPress { card: *card, frame, owner };
                        return Ok(Decision::Input(Action::Key(InputKey::MouseButton(0))));
                    }
                    if frame % 120 == 0 {
                        eprintln!("frame {frame}: 08 entry {point:?} region={region} owner={owner} live_mask={mask:?} press_candidates={live_candidates:?}");
                    }
                }
                let limit = if *card { 600 } else { idle_frames };
                if frame - *since >= limit {
                    return Ok(Decision::Stop("unrecognized_wait", format!("08 entry target {point:?} did not settle (hovered owner {hovered_owner:?})")));
                }
            }
            Self::AwaitEye { since } => {
                if let Some(owner) = player.host_replay_eye08_owner() {
                    eprintln!("frame {frame}: 08 eye owner={owner}, moving without a press");
                    *self = Self::Hover { frame, owner };
                    return Ok(Decision::Input(Action::Pointer { x: 640, y: 360, button: None }));
                }
                if frame - *since >= 1200 {
                    return Ok(Decision::Stop("unrecognized_wait", "08 eye WAIT did not settle".into()));
                }
            }
            Self::Hover { frame: moved, owner } => {
                if moved.checked_add(1) == Some(frame) && player.host_replay_eye08_owner() == Some(*owner) {
                    match player.host_prepare_eye08_click() {
                        Ok(Some(poll)) => {
                            let scene = player.host_replay_eye08_cst_load();
                            if first_body && scene.is_none() {
                                return Ok(Decision::Stop("checkpoint_mismatch", "08 first-body CST did not match".into()));
                            }
                            *self = Self::Exit { started: frame, poll, scene: scene.filter(|_| first_body) };
                            return Ok(Decision::Input(Action::Key(InputKey::MouseButton(0))));
                        }
                        Ok(None) => {}
                        Err(error) => return Ok(Decision::Stop("checkpoint_mismatch", format!("{error:#}"))),
                    }
                }
                return Ok(Decision::Stop("checkpoint_mismatch", "08 eye guard did not arm after the hover frame".into()));
            }
            Self::Exit { started, .. } if frame - *started >= 600 => {
                return Ok(Decision::Stop("unrecognized_wait", "08 eye exit timed out".into()));
            }
            Self::FirstBody { pressed, since, scene } => {
                if player.host_replay_eye08_cst_load() != Some(*scene) {
                    return Ok(Decision::Stop("unexpected_branch", "08 CST changed after eye release".into()));
                }
                match player.host_replay_eye08_body(0, *pressed) {
                    Ok(first) => {
                        log_eye08_body(frame, 0, 144, *scene, first);
                        if last_group == 0 {
                            return Ok(Decision::Done);
                        }
                        let scene = *scene;
                        return self.arm_body(player, frame, 0, scene, first);
                    }
                    Err(reason) if frame - *since >= 600 => {
                        eprintln!("frame {frame}: {}", player.host_replay_eye08_wait_diagnostics());
                        return Ok(Decision::Stop("unrecognized_wait", format!("08 g0/144 did not settle: {reason}")));
                    }
                    Err(_) => {}
                }
            }
            Self::NextBody { group, pressed, scene, first, receipt, .. } => {
                if *receipt != KeyboardPressReceipt::Verified || player.host_replay_eye08_cst_load() != Some(*scene) {
                    return Ok(Decision::Stop("checkpoint_mismatch", "08 CST load or Enter receipt differs".into()));
                }
                let next_group = *group + 1;
                let wait = EYE08_WAITS[next_group];
                match player.host_replay_eye08_body(next_group as i32, *pressed) {
                    Ok(next) => {
                        if !first.same_tasks(next) || next.generation <= first.generation {
                            return Ok(Decision::Stop("checkpoint_mismatch", "08 successor tasks or body generation differ".into()));
                        }
                        let scene = *scene;
                        log_eye08_body(frame, next_group as i32, wait, scene, next);
                        if next_group == last_group {
                            return Ok(Decision::Done);
                        }
                        return self.arm_body(player, frame, next_group, scene, next);
                    }
                    Err(reason) if frame - *pressed >= 600 => {
                        return Ok(Decision::Stop("unrecognized_wait", format!("08 g{next_group}/{wait} did not settle: {reason}")));
                    }
                    Err(_) => {}
                }
            }
            Self::Done => {
                eprintln!("frame {frame}: verified 08 eye EXIT, unbind and release");
                return Ok(Decision::Done);
            }
            Self::Failed(reason) => return Ok(Decision::Stop("checkpoint_mismatch", reason.clone())),
            _ => {}
        }
        Ok(Decision::Wait)
    }

    fn arm_body(
        &mut self, player: &mut HeadlessPlayer, frame: u64, group: usize,
        scene: (u32, u64, u64), first: Eye08Body,
    ) -> Result<Decision> {
        let Some((region, candidates)) = player.host_replay_eye08_enter(first)? else {
            let wait = EYE08_WAITS[group];
            return Ok(Decision::Stop("checkpoint_mismatch",
                format!("08 g{group}/{wait} has no verified bottom-button Enter binding")));
        };
        eprintln!("frame {frame}: 08 armed Enter region={region} receiver={} candidates={candidates:?}", first.script);
        *self = Self::NextBody {
            group, pressed: frame, scene, first, region, candidates,
            receipt: KeyboardPressReceipt::Awaiting,
        };
        Ok(Decision::Input(Action::Key(InputKey::Keyboard(13))))
    }
}

fn log_eye08_body(frame: u64, group: i32, wait: i32, (context, load, loaded): (u32, u64, u64), body: Eye08Body) {
    println!("frame={frame} eye08-ordinary-body group={group} wait={wait} context={context} load={load} loaded={loaded} body={body:?}");
}

fn consuming_mouse_resume(
    player: &HeadlessPlayer, press: &MousePress, frame: u64, owner: u32, point: [i32; 2],
    forwarding_region: Option<u32>,
) -> bool {
    consuming_mouse_resume_in_scan(player, press, frame, frame, owner, point, forwarding_region)
}

fn consuming_mouse_resume_in_scan(
    player: &HeadlessPlayer, press: &MousePress, scan_frame: u64, frame: u64,
    owner: u32, [x, y]: [i32; 2],
    forwarding_region: Option<u32>,
) -> bool {
    let mut receipts = player.host_input_route_receipts().iter().filter(|receipt| {
        (scan_frame..=frame).contains(&receipt.frame) && receipt.pressed != 0
    }).peekable();
    // Some scans first offer the click to the global forwarding region.
    let _ = receipts.next_if(|receipt| {
        Some(receipt.region) == forwarding_region
            && (receipt.receiver, receipt.response, receipt.key, receipt.group) == (0, 1, 0, 0)
            && receipt.pressed == 1 && !receipt.repeated && receipt.raw_mask == ButtonSource::Mouse.mask()
            && receipt.physical == [RoutedPhysicalButton { source: ButtonSource::Mouse, code: 1 }]
    });
    let mut resumes = player.events().iter().rev().take_while(|event| event.frame >= frame)
        .filter(|event| event.frame == frame && event.kind == "message-resume");
    receipts.next().is_some_and(|receipt| receipt.frame == frame && press.matches_target(receipt, owner))
        && receipts.next().is_none()
        && resumes.next().is_some_and(|event| event.detail == format!("message=[2, {x}, {y}, 0]"))
        && resumes.next().is_none()
}

pub(super) struct Controller {
    route: Route,
    next: usize,
    pending: Option<Pending>,
    daydream_case_press: Option<DaydreamCasePress>,
    island_bad02: Option<island_bad02::IslandBad02>,
    last_position: Option<Position>,
    changed: u64,
    eye08: Option<Eye08Route>,
    kamikaze: Option<kamikaze::Replay>,
    grisaia: Option<grisaia::GrisaiaRoute>,
    campanella: Option<campanella::Replay>,
    hatsumira: Option<hatsumira::HatsumiraRoute>,
    neko_ordinary: Option<neko_ordinary::Ordinary>,
    rebellions: Option<rebellions::RebellionsRoute>,
}

impl Controller {
    pub(super) fn idle_frames(&self) -> u64 {
        self.route.idle_frames
    }

    pub(super) fn read(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path).context("read replay route")?;
        let route: Route = serde_json::from_slice(&bytes).context("decode replay route")?;
        ensure!(!route.eyecatch_08_next_body || route.eyecatch_08_first_body,
            "08 second body requires the first body checkpoint");
        ensure!(!route.eyecatch_08_first_body || route.eyecatch_08_exit,
            "08 first body requires the guarded eye exit");
        Self::from_route(route)
    }

    fn from_route(mut route: Route) -> Result<Self> {
        if route.island_bad02 {
            ensure!(route.steps.len() >= 2
                && route.steps[route.steps.len() - 2].is_island_beach_choice_entry()
                && route.steps.last().is_some_and(|step| step.wait.state == "choice"
                    && step.wait.scene.as_deref() == Some("com01_selC")
                    && step.wait.window.as_deref() == Some("select") && step.click.is_none()
                    && step.key.is_none() && !step.capture)
                && !route.daydream_case_start && !route.eyecatch_08_exit && route.kanmi_exact.is_none()
                && route.kamikaze_post_op.is_none() && !route.grisaia_14_first_advance
                && !route.campanella_18_first_advance && route.rebellions_r15.is_none()
                && route.hatsumira_r72.is_none() && route.neko_ordinary.is_none(),
                "ISLAND BAD_02 requires a guarded route to the beach choice");
        }
        if let Some(plan) = &route.daydream_case03_end1 {
            plan.validate()?;
            if let Some(sha) = &plan.ordinary_load_sha256 {
                ensure!(route.idle_frames > 3 && route.steps.is_empty() && route.page_scenes.is_empty()
                    && route.page_click.is_none() && route.page_fallback_before.is_none()
                    && !route.daydream_case_start && !route.daydream_case_first_advance
                    && !route.eyecatch_08_exit && !route.campanella_18_first_advance
                    && !route.grisaia_14_first_advance && !route.island_bad02
                    && route.kanmi_exact.is_none() && route.kamikaze_post_op.is_none()
                    && route.guarded_body_last_group.is_none() && route.rebellions_r15.is_none()
                    && route.hatsumira_r72.is_none() && route.neko_ordinary.is_none(),
                    "Daydream cold load requires only the ordinary load plan");
                let cold = daydream_case03_end1::ordinary::Load::new(sha.clone());
                return Ok(Self { route, next: 0, pending: None, last_position: None, changed: 0,
                    daydream_case_press: Some(DaydreamCasePress {
                        started: 0, title: 0, region: 0, receipt: None, loads: Vec::new(),
                        unexpected: false, turn: None, ending: None, cold: Some(cold),
                    }),
                    island_bad02: None, eye08: None, kamikaze: None, grisaia: None, campanella: None,
                    hatsumira: None, neko_ordinary: None, rebellions: None,
                });
            }
            ensure!(route.daydream_case_first_advance, "Daydream CASE 3 END1 requires the guarded first CASE advance");
        }
        ensure!(!route.daydream_case_first_advance || route.daydream_case_start,
            "Daydream first CASE advance requires the guarded title route");
        if route.daydream_case_start {
            ensure!(route.kanmi_exact.is_none() && !route.eyecatch_08_exit
                && route.kamikaze_post_op.is_none() && !route.grisaia_14_first_advance
                && !route.campanella_18_first_advance && !route.island_bad02
                && route.rebellions_r15.is_none() && route.hatsumira_r72.is_none()
                && route.neko_ordinary.is_none() && route.page_scenes.is_empty()
                && route.page_click.is_none() && route.page_fallback_before.is_none()
                && route.steps.len() == DAYDREAM_PROLOGUE_WAITS.len() + 1
                && route.steps[..DAYDREAM_PROLOGUE_WAITS.len()].iter().enumerate().all(|(group, step)| {
                    step.wait.daydream_opening_page() == Some(group) && step.is_daydream_enter()
                }),
                "Daydream CASE start requires the complete guarded prologue and settled title");
        }
        if let Some(group) = route.guarded_body_last_group {
            ensure!(
                (route.eyecatch_08_next_body && matches!(group, 1 | 2 | 5 | 6 | 27))
                    || (route.grisaia_14_first_advance && matches!(group, 1 | 3 | 5 | 8))
                    || (route.campanella_18_first_advance && matches!(group, 0 | 1 | 3 | 6 | 9)),
                "guarded_body_last_group is unsupported for this route"
            );
        }
        if route.campanella_18_first_advance {
            ensure!(
                !route.eyecatch_08_exit && !route.grisaia_14_first_advance
                    && route.kamikaze_post_op.is_none() && route.kanmi_exact.is_none()
                    && route.rebellions_r15.is_none() && route.hatsumira_r72.is_none()
                    && route.neko_ordinary.is_none() && route.steps.is_empty()
                    && route.page_scenes.is_empty() && route.page_click.is_none()
                    && route.page_fallback_before.is_none() && route.idle_frames > 3,
                "18 requires its dedicated title and body route"
            );
            return Ok(Self {
                route, next: 0, pending: None, daydream_case_press: None, island_bad02: None, last_position: None, changed: 0,
                eye08: None, kamikaze: None, grisaia: None, rebellions: None,
                hatsumira: None, neko_ordinary: None, campanella: Some(campanella::Replay::default()),
            });
        }
        if let Some(mode) = route.hatsumira_r72 {
            ensure!(
                route.steps.is_empty() && route.page_scenes.is_empty() && route.page_click.is_none()
                    && route.page_fallback_before.is_none() && route.kanmi_exact.is_none()
                    && !route.eyecatch_08_exit && route.kamikaze_post_op.is_none()
                    && !route.grisaia_14_first_advance && route.rebellions_r15.is_none()
                    && route.neko_ordinary.is_none() && route.idle_frames > 3,
                "Hatsumira r72 uses its dedicated title and op001 checkpoints"
            );
            return Ok(Self {
                route, next: 0, pending: None, daydream_case_press: None, island_bad02: None, last_position: None, changed: 0,
                eye08: None, kamikaze: None, grisaia: None, campanella: None, rebellions: None,
                hatsumira: Some(hatsumira::HatsumiraRoute::new(mode)), neko_ordinary: None,
            });
        }
        if let Some(config) = &route.neko_ordinary {
            config.validate(&route)?;
        }
        let neko_ordinary = route.neko_ordinary.clone().map(neko_ordinary::Ordinary::new);
        if let Some(config) = route.rebellions_r15.take() {
            ensure!(route.steps.is_empty() && route.page_scenes.is_empty() && route.page_click.is_none()
                && route.page_fallback_before.is_none() && route.kanmi_exact.is_none()
                && !route.eyecatch_08_exit && route.kamikaze_post_op.is_none()
                && !route.grisaia_14_first_advance && route.idle_frames > 3,
                "Rebellions r15 uses its fixed entry and body route");
            return Ok(Self { route, next: 0, pending: None, daydream_case_press: None, island_bad02: None, last_position: None, changed: 0,
                eye08: None, kamikaze: None, grisaia: None, campanella: None,
                rebellions: Some(rebellions::RebellionsRoute::new(config)?),
                hatsumira: None, neko_ordinary });
        }
        if route.grisaia_14_first_advance {
            ensure!(!route.eyecatch_08_exit && route.kamikaze_post_op.is_none()
                && route.kanmi_exact.is_none() && route.steps.is_empty()
                && route.page_scenes.is_empty() && route.page_click.is_none()
                && route.page_fallback_before.is_none() && route.idle_frames > 3,
                "14 first advance requires its dedicated title, first page and terminal checkpoint");
            return Ok(Self { route, next: 0, pending: None, daydream_case_press: None, island_bad02: None, last_position: None, changed: 0,
                eye08: None, kamikaze: None, grisaia: Some(grisaia::GrisaiaRoute::Title { moved: false }), campanella: None,
                rebellions: None,
                hatsumira: None, neko_ordinary });
        }
        if let Some(contract) = &route.kamikaze_post_op {
            ensure!(contract.valid() && route.steps.is_empty() && route.idle_frames > 3
                && route.page_scenes.is_empty() && route.page_click.is_none()
                && route.page_fallback_before.is_none() && route.kanmi_exact.is_none()
                && !route.eyecatch_08_exit, "07 requires the post-OP route without generic steps");
            return Ok(Self { route, next: 0, pending: None, daydream_case_press: None, island_bad02: None, last_position: None, changed: 0,
                eye08: None, kamikaze: Some(kamikaze::Replay::default()), grisaia: None, campanella: None, rebellions: None,
                hatsumira: None, neko_ordinary });
        }
        if route.eyecatch_08_exit {
            ensure!(route.steps.is_empty() && route.page_scenes.is_empty() && route.page_click.is_none()
                && route.page_fallback_before.is_none() && route.kanmi_exact.is_none() && route.idle_frames > 3,
                "08 eyecatch exit uses only its verified entry and terminal click");
            return Ok(Self { route, next: 0, pending: None, daydream_case_press: None, island_bad02: None, last_position: None, changed: 0,
                eye08: Some(Eye08Route::Entry { card: false, since: 0, hovered_owner: None }), kamikaze: None, grisaia: None, campanella: None,
                rebellions: None,
                hatsumira: None, neko_ordinary });
        }
        if let Some(contract) = &route.kanmi_exact {
            let load_terminal = contract.load_terminal_group();
            ensure!(route.idle_frames > 3 && contract.matches() && if load_terminal.is_some() {
                route.steps.is_empty() && route.page_scenes.is_empty() && route.page_click.is_none()
                    && route.page_fallback_before.is_none() && route.neko_ordinary.is_none()
            } else { route.valid_kanmi_steps(contract) }, "Kanmi requires its exact title and adjacent body checkpoints");
            if let Some(group) = load_terminal {
                let through_op = contract.contract == "kanmi-load-sc02-post-op/v1";
                route.kanmi_exact.as_mut().unwrap().load_state = Some(kanmi_ordinary::Load::new(group, through_op));
            }
            return Ok(Self { route, next: 0, pending: None, daydream_case_press: None, island_bad02: None, last_position: None, changed: 0,
                eye08: None, kamikaze: None, grisaia: None, campanella: None, rebellions: None,
                hatsumira: None, neko_ordinary });
        }
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
            let daydream_enter = step.is_daydream_enter();
            if step.wait.state == "page_probe" && input {
                ensure!(step.mouse_press.is_some() || step.wait.is_daydream_r96_probe() && daydream_enter,
                    "page probe input requires a verified mouse press or the exact Daydream r96 Enter");
            }
            if step.is_neko_save_entry() {
                let stop = &route.steps[index + 1];
                ensure!(index == 26 && index + 1 == last && route.page_scenes.is_empty()
                    && route.page_click.is_none() && route.page_fallback_before.unwrap_or(0) == 0
                    && route.steps[0].is_neko_startup_press() && route.steps[0].wait.state == "language_select"
                    && route.steps[1].is_neko_startup_press() && route.steps[1].wait.state == "title"
                    && route.steps[2..index].iter().all(Step::is_neko_body_press)
                    && stop.wait.state == "neko_save_panel" && stop.wait.window == step.after.as_ref().unwrap().window
                    && stop.wait.same_record(&step.wait) && stop.wait.hit.is_none() && stop.click.is_none()
                    && stop.key.is_none() && !stop.capture && stop.probe_points == [[285, 184]]
                    && step.probe_points == [[625, 478], [1147, 612]]
                    && step.continuous_window().is_none() && !step.capture
                    && step.neko_startup_language.is_none() && step.keyboard_press.is_none()
                    && step.shared_flags.is_empty() && stop.shared_flags.is_empty(),
                    "NEKO Save entry requires the exact cold-start r218 chain and an input-free slot probe");
                continue;
            }
            if step.wait.window.as_deref() == Some("meswnd") && input {
                ensure!(step.is_neko_body_press() || step.island_meswnd_edge().is_some() || step.is_island_beach_choice_entry()
                    || step.is_island_opmovie_exit() || step.is_island_first_play_branch(),
                    "a meswnd input requires an exact guarded NEKO or ISLAND route");
            }
            if step.keyboard_press.is_some() || input && step.wait.daydream_opening_page().is_some() {
                ensure!(daydream_enter && route.page_scenes.is_empty()
                    && step.continuous_window().is_none() && !step.continue_same_record_prompt
                    && step.neko_startup_language.is_none(),
                    "Daydream Enter requires its exact opening step and keyboard receipt, with no fallback");
                let after = step.after.as_ref().unwrap();
                let next = &route.steps[index + 1];
                let valid_next = if step.is_daydream_prologue_exit() {
                    index + 1 == last && next.wait.is_daydream_title()
                        && next.key.is_none() && next.click.is_none()
                        && next.keyboard_press.is_none() && next.mouse_press.is_none()
                        && !next.capture && next.probe_points.is_empty()
                } else {
                    next.wait.daydream_opening_page() == after.daydream_opening_page()
                        && next.wait.hit.is_none() && (index + 1 == last || next.is_daydream_enter())
                };
                ensure!(valid_next,
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
                let island_opmovie = step.is_island_opmovie_exit();
                let island_first_play = step.is_island_first_play_branch();
                let island_beach_exit = step.is_island_beach_fullwnd_exit();
                let island_beach_choice = step.is_island_beach_choice_entry();
                ensure!((fullwnd || novel || neko || island_meswnd || island_opmovie || island_first_play || island_beach_choice)
                    && step.click.is_some(),
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
                if island_meswnd || island_opmovie || island_first_play || island_beach_choice {
                    let next = &route.steps[index + 1];
                    let linked = index.checked_sub(1).is_some_and(|previous| {
                        let prior = &route.steps[previous];
                        (prior.is_island_meswnd_probe_exit() || prior.island_meswnd_edge().is_some() || prior.is_island_beach_fullwnd_exit()
                            || prior.is_island_opmovie_exit() || prior.is_island_first_play_branch())
                            && prior.after.as_ref().is_some_and(|after| after.same_record(&step.wait))
                    });
                    let valid_successor = terminal_checkpoint
                        || next.island_meswnd_edge().is_some() || next.is_island_first_play_branch()
                        || island_meswnd && next.is_island_beach_choice_entry()
                        || island_first_play && next.is_island_beach_fullwnd_exit()
                        || island_meswnd && next.is_island_opmovie_exit();
                    ensure!(no_fallback && linked && exact_next_checkpoint(next) && next.probe_points == ISLAND_MESWND_POINTS
                        && valid_successor,
                        "ISLAND meswnd requires a contiguous guarded chain and a double-point input-free stop without fallback");
                }
                if island_beach_exit {
                    let next = &route.steps[index + 1];
                    let linked = index.checked_sub(1).is_some_and(|previous| {
                        let prior = &route.steps[previous];
                        prior.is_island_first_play_branch()
                            && prior.after.as_ref().is_some_and(|after| after.same_record(&step.wait))
                    });
                    ensure!(no_fallback && linked && exact_next_checkpoint(next)
                        && next.probe_points == ISLAND_MESWND_POINTS && (terminal_checkpoint || next.island_meswnd_edge().is_some()),
                        "ISLAND beach full-window input requires the first-play chain and a guarded message-window successor");
                }
                if neko {
                    let next = &route.steps[index + 1];
                    ensure!(!step.same_novel_window && !step.same_full_window && no_fallback
                        && exact_next_checkpoint(next) && (terminal_checkpoint || next.is_neko_body_press() || next.is_neko_save_entry()),
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
                ensure!(step.wait.has_exact_record()
                    && (island_beach_choice && terminal_checkpoint || after.has_exact_record() && after_ready
                        || island_first_play && after.has_exact_record() && after.state == "page_probe")
                    && (island_opmovie || island_first_play || island_beach_choice
                        || after.scene == step.wait.scene && (later_page || same_record_prompt)),
                    "verified mouse press requires an exact later ready page, terminal full-window probe, or terminal same-record novel prompt");
                ensure!(press.candidates.iter().filter(|&&region| region == press.region).count() == 1,
                    "verified mouse press needs one target in its ordered candidate list");
            }
            for flag in &step.shared_flags {
                ensure!(flag.mask != 0 && flag.expected & !flag.mask == 0,
                    "shared flag requires a nonzero mask containing all expected bits");
            }
        }
        Ok(Self { route, next: 0, pending: None, daydream_case_press: None, island_bad02: None, last_position: None, changed: 0,
            eye08: None, kamikaze: None, grisaia: None, campanella: None, rebellions: None,
            hatsumira: None, neko_ordinary })
    }

    pub(super) fn observe_lifecycle_events(&mut self, events: &[LogicEvent], batch_start: usize) {
        if let Some(press) = &mut self.daydream_case_press {
            if let Some(batch) = events.get(batch_start..) {
                if let Some(cold) = &mut press.cold {
                    cold.events(batch);
                } else if let (Some(replay), Some(plan)) = (&mut press.ending, &self.route.daydream_case03_end1) {
                    replay.observe_events(plan, batch);
                } else {
                    press.observe_events(batch);
                }
            }
            return;
        }
        if let Some(campanella) = &mut self.campanella {
            campanella.observe_lifetimes(events, batch_start);
            return;
        }
        if let Some(neko) = &mut self.neko_ordinary && neko.active() {
            if let Some(batch) = events.get(batch_start..) {
                neko.observe_events(batch);
            }
            return;
        }
        if let Some(route) = &mut self.hatsumira {
            route.observe_lifecycle_events(events, batch_start);
            return;
        }
        if let Some(route) = &mut self.rebellions {
            route.observe_lifecycle_events(events, batch_start);
            return;
        }
        if let Some(grisaia) = &mut self.grisaia {
            grisaia.observe_lifetimes(events, batch_start);
            return;
        }
        if let Some(replay) = &mut self.kamikaze {
            replay.observe_events(events, batch_start);
            return;
        }
        let Some(pending) = self.pending.as_mut() else { return; };
        // Save the r238 command and task creation before the host trims diagnostic events.
        if let Some(tail) = &mut pending.daydream_tail {
            if let Some(batch) = events.get(batch_start..) {
                tail.observe(pending.started, batch);
            }
        }
        if let Some(body) = &mut pending.kanmi_body {
            body.lifetime.observe(body.owner, events, batch_start);
            body.receiver_lifetime.observe(body.receiver, events, batch_start);
            body.region_lifetime.observe(body.region, events, batch_start);
        }
        let Some(press) = pending.mouse_press.as_mut() else { return; };
        let owner = if let Some(island) = &mut pending.island_meswnd {
            island.root_lifetime.observe(press.receiver, events, batch_start);
            if pending.step.is_some_and(|index| self.route.steps[index].rebuilds_island_meswnd()) {
                for event in events.iter().filter(|event| {
                    event.frame > pending.started && event.kind == "task-create"
                }) {
                    let mut fields = event.detail.split_ascii_whitespace();
                    let Some(owner) = fields.next().and_then(|word| word.strip_prefix("id="))
                        .and_then(|value| value.parse::<u32>().ok()) else {
                        continue;
                    };
                    if fields.next() == Some("kind=34") && fields.next() == Some("parent=8")
                        && fields.next() == Some("name=\"meswnd\"")
                    {
                        island.replacement_window = Some(owner);
                    }
                }
            }
            island.window_owner
        } else {
            press.receiver
        };
        if let Some(lifetime) = &mut press.lifetime {
            lifetime.observe(owner, events, batch_start);
        }
    }

    pub(super) fn observe_frame(&mut self, player: &HeadlessPlayer) {
        if let Some(save) = self.route.kanmi_exact.as_mut().and_then(|route| route.save_state.as_mut()) {
            save.observe(player);
            return;
        }
        if let Some(load) = self.route.kanmi_exact.as_mut().and_then(|route| route.load_state.as_mut()) {
            load.observe(player);
            return;
        }
        if let Some(replay) = &mut self.island_bad02 {
            replay.observe_frame(player);
            return;
        }
        if let Some(press) = &mut self.daydream_case_press {
            if let Some(cold) = &mut press.cold {
                cold.receipts(player);
            } else if let Some(replay) = &mut press.ending {
                replay.observe_receipts(player);
            } else {
                press.observe_receipts(player);
            }
            return;
        }
        if let Some(campanella) = &mut self.campanella {
            campanella.observe_frame(player);
            return;
        }
        if let Some(neko) = &mut self.neko_ordinary && neko.active() {
            neko.observe(player);
            return;
        }
        if let Some(route) = &mut self.hatsumira {
            route.observe_frame(player);
            return;
        }
        if let Some(route) = &mut self.rebellions {
            route.observe_frame(player);
            return;
        }
        if let Some(grisaia) = &mut self.grisaia {
            grisaia.observe_frame(player);
            return;
        }
        if let (Some(replay), Some(contract)) = (&mut self.kamikaze, &self.route.kamikaze_post_op) {
            replay.observe_frame(player, contract);
            return;
        }
        if let Some(eye) = &mut self.eye08 {
            eye.observe_frame(player);
            return;
        }
        let Some(pending) = &mut self.pending else { return; };
        let Some(index) = pending.step else { return; };
        let step = &self.route.steps[index];
        if step.is_island_opmovie_exit() && let Some(island) = &mut pending.island_meswnd
            && !island.saw_opening_movie && player.host_island_opmovie_playing()
        {
            island.saw_opening_movie = true;
            eprintln!("replay: observed active ISLAND opmovie.webm");
        }
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
        if self.route.kanmi_exact.is_some() {
            let frame = pending.started + 1;
            press.confirmed = consuming_mouse_resume(player, expected, frame, press.receiver, [140, 354], Some(3));
            if press.confirmed {
                eprintln!("frame {frame}: verified Kanmi New Game mouse press and VM resume");
            }
            return;
        }
        let receipts = player.host_input_route_receipts();
        let receipt = if pending.island_meswnd.is_some() {
            expected.island_root_receipt(pending.started, press.receiver, receipts, player.events())
        } else if pending.neko_save_anchor.is_some() {
            let named = |receipt: &InputRouteReceipt| {
                let live_response = MousePress { response: receipt.response, candidates: vec![],
                    region: expected.region, key: expected.key, group: expected.group };
                receipt.response <= 1 && live_response.matches_receipt(receipt, pending.started, press.receiver)
            };
            let mut matches = receipts.iter().filter(|receipt| named(receipt));
            let receipt = matches.next();
            let foreign_consumer = receipts.iter().any(|receipt| {
                receipt.frame > pending.started && receipt.frame - pending.started <= 3
                    && receipt.key == 0 && receipt.group == 0 && receipt.pressed == 1 && !receipt.repeated
                    && receipt.response == 0 && !named(receipt)
                    && receipt.physical == [RoutedPhysicalButton { source: ButtonSource::Mouse, code: 1 }]
            });
            receipt.filter(|_| matches.next().is_none() && !foreign_consumer)
        } else {
            receipts.iter().find(|receipt| expected.matches_receipt(receipt, pending.started, press.receiver))
        };
        if let Some(receipt) = receipt {
            press.confirmed = true;
            eprintln!("replay frame={}: verified mouse press {:?}: {receipt:?}", receipt.frame, step.label);
        }
    }

    pub(super) fn poll(&mut self, player: &mut HeadlessPlayer, frame: u64, output: &Path) -> Result<Decision> {
        if let Some(save) = self.route.kanmi_exact.as_mut().and_then(|route| route.save_state.as_mut()) {
            return save.poll(player, frame);
        }
        if let Some(load) = self.route.kanmi_exact.as_mut().and_then(|route| route.load_state.as_mut()) {
            return load.poll(player, frame, self.route.idle_frames);
        }
        if let Some(replay) = &mut self.island_bad02 {
            return replay.poll(player, frame, self.route.idle_frames, output);
        }
        if let Some(press) = &mut self.daydream_case_press {
            if let Some(cold) = &mut press.cold {
                let plan = self.route.daydream_case03_end1.as_ref().expect("cold load requires a route plan");
                let decision = cold.poll(player, frame, self.route.idle_frames, plan.resume_after_load)?;
                if let Some((opened, restored)) = cold.take_resume() {
                    press.cold = None;
                    press.ending = Some(daydream_case03_end1::Replay::from_ordinary_load(plan, frame, opened, restored));
                    return press.poll(player, frame, self.route.idle_frames, false, Some(plan));
                }
                return Ok(decision);
            }
            return press.poll(player, frame, self.route.idle_frames, self.route.daydream_case_first_advance,
                self.route.daydream_case03_end1.as_ref());
        }
        if let Some(campanella) = &mut self.campanella {
            return campanella.poll(player, frame, self.route.idle_frames, self.route.guarded_body_last_group.unwrap_or(1));
        }
        if let Some(neko) = &mut self.neko_ordinary && neko.active() {
            return neko.poll(player, frame, self.route.idle_frames);
        }
        if let Some(route) = &mut self.hatsumira {
            return route.poll(player, frame, self.route.idle_frames);
        }
        if let Some(route) = &mut self.rebellions {
            return route.poll(player, frame, self.route.idle_frames);
        }
        if let Some(grisaia) = &mut self.grisaia {
            return grisaia.poll(player, frame, self.route.idle_frames, self.route.guarded_body_last_group.unwrap_or(1));
        }
        if let (Some(replay), Some(contract)) = (&mut self.kamikaze, &self.route.kamikaze_post_op) {
            return replay.poll(player, frame, contract);
        }
        if let Some(eye) = &mut self.eye08 {
            let last_group = self.route.guarded_body_last_group
                .unwrap_or(usize::from(self.route.eyecatch_08_next_body));
            return eye.poll(player, frame, self.route.idle_frames,
                self.route.eyecatch_08_first_body, last_group);
        }
        let kanmi = self.route.kanmi_exact.is_some();
        let (mut position, mut window) = Position::observe(player, kanmi);
        if let Some(anchor) = self.pending.as_ref().and_then(|pending| pending.neko_save_anchor)
            && let Some(owner) = player.host_neko_save_panel(anchor)
        {
            position = Position { state: "neko_save_panel", scene: "01".into(), group: 24, record: 218 };
            window = Some(("saveload", owner));
        }
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
            let ready = matches!(position.state, "page" | "page_prompt" | "choice" | "title" | "menu" | "language_select" | "neko_save_panel");
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
                if step.is_daydream_prologue_exit() {
                    if let Some((flow, ttlflow, title, region, rectangle)) = player.host_daydream_post_prologue_title() {
                        let probe = if let Some([left, top, right, bottom]) = rectangle {
                            let point = [left.midpoint(right), top.midpoint(bottom)];
                            let hit = player.masked_input_region_at(point)?;
                            let candidates = player.mouse_press_candidate_regions(point)?;
                            Some(([left, top, right, bottom], point, hit, candidates))
                        } else {
                            None
                        };
                        let Some((rect, point, hit, candidates)) = probe.as_ref().filter(|(_, _, hit, candidates)| {
                            hit.is_some_and(|mask| mask[0] == region && mask[8] != 0 && mask[9] != 0)
                                && candidates.contains(&region)
                        }) else {
                            let timed_out = frame - pending.started >= self.route.idle_frames;
                            if timed_out || frame % 120 == 0 {
                                let button_rectangle = player.input_button_state(region).map(|button| button.rectangle);
                                eprintln!("frame {frame}: Daydream title input not ready title={title} region={region} button_rectangle={button_rectangle:?} live_rectangle={rectangle:?} probe={probe:?}");
                            }
                            if timed_out {
                                return Ok(Decision::Stop("unrecognized_wait", "Daydream title input did not become ready".into()));
                            }
                            return Ok(Decision::Wait);
                        };
                        let [x, y] = *point;
                        let terminal = &self.route.steps[self.next];
                        eprintln!("frame {frame}: Daydream post-prologue title wait flow={flow} ttlflow={ttlflow} title={title} button=btn1[0] region={region}");
                        println!("replay frame={frame} checkpoint={:?}", terminal.label);
                        self.next += 1;
                        self.pending = None;
                        if !self.route.daydream_case_start {
                            return Ok(Decision::Done);
                        }
                        eprintln!("frame {frame}: Daydream title probe title={title} region={region} rectangle={rect:?} point=[{x}, {y}] masked={hit:?} candidates={candidates:?}");
                        self.daydream_case_press = Some(DaydreamCasePress {
                            started: frame, title, region,
                            receipt: None, loads: Vec::new(), unexpected: false, turn: None, ending: None, cold: None,
                        });
                        return Ok(Decision::Input(Action::Pointer { x, y, button: Some(0) }));
                    }
                    let Some(anchor) = pending.daydream_body else {
                        return Ok(Decision::Stop("checkpoint_mismatch", "missing verified r221 body anchor".into()));
                    };
                    let receiver = step.keyboard_press.as_ref().unwrap().receiver;
                    let default_window = pending.daydream_tail.as_ref().and_then(|tail| tail.window);
                    let fesds_frame = pending.daydream_tail.as_ref().and_then(|tail| tail.fesds_frame);
                    let prologue_eof_frame = pending.daydream_tail.as_ref().and_then(|tail| tail.prologue_eof_frame);
                    if player.host_daydream_unexpected_transition_window(
                        pending.started, anchor, receiver, default_window, fesds_frame, prologue_eof_frame,
                    )
                        || position.scene != "-" && !position.scene.is_empty() && position.scene != "prologue"
                        || matches!(position.state, "choice" | "menu" | "language_select")
                        || changed && matches!(position.state, "page" | "page_prompt" | "page_probe" | "title")
                    {
                        return Ok(Decision::Stop("unexpected_branch", format!(
                            "unexpected Daydream post-prologue wait: {position:?} window={window:?} r238={default_window:?} r242={fesds_frame:?} prologue_eof={prologue_eof_frame:?} oldest_event_frame={:?}",
                            player.events().first().map(|event| event.frame),
                        )));
                    }
                    if frame - pending.started >= self.route.idle_frames {
                        return Ok(Decision::Stop("unrecognized_wait", format!(
                            "Daydream title wait not confirmed after r221: {position:?} window={window:?}",
                        )));
                    }
                    return Ok(Decision::Wait);
                }
                let same_instance = if let Some(body) = &pending.kanmi_body {
                    let after = step.after.as_ref().unwrap();
                    body.lifetime == NovelWindowLifetime::Continuous
                        && body.receiver_lifetime == NovelWindowLifetime::Continuous
                        && body.region_lifetime == NovelWindowLifetime::Continuous
                        && (!after.matches_position(&position, window)
                            || player.host_kanmi_exact_body().is_some_and(|(owner, region, receiver, context, load, record, epoch, source_frame, _)| {
                                let same_target = (owner, region, receiver) == (body.owner, body.region, body.receiver);
                                let load_matches = if pending.origin.scene == position.scene {
                                    (context, load) == (body.context, body.load)
                                } else {
                                    self.route.kanmi_exact.as_ref().is_some_and(|contract| contract.sc02_wait_count() != 0)
                                        && pending.origin.scene == "sc01"
                                        && (pending.origin.group, pending.origin.record) == (293, 1324)
                                        && position.scene == "sc02" && (position.group, position.record) == (0, 10)
                                        && load > body.load && player.host_kanmi_sc02_loaded_after(pending.started)
                                };
                                same_target && load_matches && Some(record) == HeadlessPlayer::host_kanmi_body_record(
                                    &position.scene, position.group, position.record,
                                )
                                    && epoch > body.epoch && source_frame > pending.started
                            }))
                } else if let Some((context, load, before_owner, epoch, _)) = pending.daydream_body {
                    !step.after.as_ref().unwrap().matches_position(&position, window)
                        || window.and_then(|(_, owner)| player.host_replay_body_route_anchor(
                            &position.scene, (position.group, position.record), None, owner,
                        )).is_some_and(|(after_context, after_load, owner, after_epoch, source_frame)| {
                            (after_context, after_load) == (context, load)
                                && (owner != before_owner || after_epoch != epoch)
                                && source_frame > pending.started
                        })
                } else if step.is_island_beach_fullwnd_exit() {
                    !step.after.as_ref().unwrap().matches_position(&position, window)
                        || window.is_some_and(|(_, owner)| {
                            player.host_island_meswnd_anchor(&position.scene, (1, 67), Some((2, 93)), owner).is_some()
                        })
                } else if let Some(island) = &pending.island_meswnd {
                    if let Some((_, target)) = step.island_meswnd_edge() {
                        let expected_owner = if step.rebuilds_island_meswnd() {
                            island.replacement_window.filter(|owner| *owner != island.window_owner)
                        } else {
                            pending.mouse_press.as_ref()
                                .filter(|press| press.lifetime == Some(NovelWindowLifetime::Continuous))
                                .map(|_| island.window_owner)
                        };
                        island.root_lifetime == NovelWindowLifetime::Continuous
                            && (!step.after.as_ref().unwrap().matches_position(&position, window)
                                || expected_owner.is_some_and(|owner| {
                                    player.host_island_meswnd_anchor(&position.scene, target, None, owner) == Some(island.anchor)
                                }))
                    } else if step.is_island_first_play_branch() {
                        island.root_lifetime == NovelWindowLifetime::Continuous
                            && (!step.after.as_ref().unwrap().matches_position(&position, window)
                                || if position.state == "page_probe" {
                                    player.host_island_beach_fullwnd(island.anchor.1)
                                } else {
                                    window.is_some_and(|(_, owner)| {
                                        player.host_island_meswnd_anchor(&position.scene,
                                            (position.group, position.record), None, owner)
                                            .is_some_and(|(_, load, root)| load > island.anchor.1 && root == island.anchor.2)
                                    })
                                })
                    } else if step.is_island_beach_choice_entry() {
                        island.root_lifetime == NovelWindowLifetime::Continuous
                            && (!step.after.as_ref().unwrap().matches_position(&position, window)
                                || player.host_island_beach_choice_loaded(island.anchor.0, island.anchor.1))
                    } else {
                        island.root_lifetime == NovelWindowLifetime::Continuous
                            && (!step.after.as_ref().unwrap().matches_position(&position, window)
                                || island.saw_opening_movie && player.host_island_opmovie_ended(island.anchor.1))
                    }
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
                    if let Some(anchor @ (context, load, owner, _, _)) = pending.neko_save_anchor {
                        let panel = window.unwrap().1;
                        let point = [285, 184];
                        let slot = player.masked_input_region_at(point)?;
                        let candidates = player.mouse_press_candidate_regions(point)?;
                        let receivers: Vec<_> = candidates.iter().map(|&region| {
                            (region, player.input_button_state(region).and_then(|button| button.receiver))
                        }).collect();
                        let enabled_slot = slot.is_some_and(|mask| mask[8] != 0 && candidates.contains(&mask[0])
                            && player.host_neko_save_target(panel, mask[0], "btdata", 0));
                        eprintln!("frame {frame}: probe {point:?} masked={slot:?} press_candidates={candidates:?} window={window:?} press_receivers={receivers:?}");
                        eprintln!("frame {frame}: probe wait {}", player.host_replay_wait_diagnostics());
                        if !enabled_slot || !self.route.steps[0].neko_startup_language.as_deref()
                            .is_some_and(|language| player.host_neko_startup_language_matches(language))
                        {
                            return Ok(Decision::Stop("checkpoint_mismatch", "NEKO Save panel slot 0 or language did not match".into()));
                        }
                        let slot_region = slot.unwrap()[0];
                        println!("frame={frame} neko-save-panel {}", serde_json::json!({
                            "owner": panel, "block": "WAIT", "driver_frame": frame, "slot": 0, "slot_exists": false,
                            "globals": {"800": 0, "801": 1, "860": 0}, "locals": {"100": 0, "110": 0},
                            "retained": ["01", 24, 218, context, load, owner],
                            "slot_region": slot_region, "point": point, "candidates": candidates, "receivers": receivers,
                        }));
                        println!("replay frame={frame} verified={:?}", step.label);
                        if let Some(neko) = &mut self.neko_ordinary {
                            self.pending = None;
                            return neko.start_save(player, frame, anchor, panel);
                        }
                        println!("replay frame={frame} checkpoint={:?}", self.route.steps[self.next].label);
                        return Ok(Decision::Done);
                    }
                    println!("replay frame={frame} verified={:?}", step.label);
                    if pending.mouse_press.is_some() || step.keyboard_press.is_some() {
                        eprintln!("frame {frame}: guarded press successor={position:?} window={window:?}");
                    }
                    if pending.kanmi_body.is_some()
                        && self.route.kanmi_exact.as_ref().is_some_and(|contract| contract.contract != "kanmi-title-r43/v1")
                        && let Some((owner, region, receiver, context, load, record, epoch, source_frame, prefix)) =
                            player.host_kanmi_exact_body()
                    {
                        log_kanmi_body(frame, owner, region, receiver, context, load, record, epoch, source_frame);
                        if (position.scene.as_str(), position.group, position.record) == ("sc02", 101, 562) {
                            println!("frame={frame} kanmi-body-prefix record={prefix:?}");
                        }
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
        let daydream_body = is_daydream_runtime(player) && matches!(position.state, "page" | "page_probe")
            && window.is_some_and(|(name, _)| name == "cinema");
        if step.wait.matches(&position, window, player)? {
            let daydream_enter = step.is_daydream_enter();
            if daydream_body && (step.click.is_some() || step.key.is_some()) && !daydream_enter
                || step.keyboard_press.is_some() && !kanmi && !(daydream_body && daydream_enter)
            {
                return Ok(Decision::Stop("checkpoint_mismatch", "Daydream input requires an exact cinema page and guarded Enter route".into()));
            }
            if step.is_neko_body_press() && step.wait.neko_opening_page() == Some(24) {
                let anchor = window.and_then(|(_, owner)| {
                    player.host_replay_body_route_anchor("01", (24, 218), Some((25, 337)), owner)
                });
                if anchor.is_none() {
                    return Ok(Decision::Stop("checkpoint_mismatch",
                        "NEKO kitchen turn withheld: composed r218 or adjacent CST wait did not match".into()));
                }
            }
            let mut kanmi_body = None;
            let mut daydream_anchor = None;
            if let Some(press) = &step.keyboard_press && kanmi {
                let actual = player.host_kanmi_exact_body();
                let Some((owner, region, receiver, context, load, record, epoch, source_frame, _)) = actual
                    .filter(|(_, _, _, _, _, body, _, _, _)|
                        Some(*body) == HeadlessPlayer::host_kanmi_body_record(
                            &position.scene, position.group, position.record,
                        ))
                else {
                    return Ok(Decision::Stop("checkpoint_mismatch", "Kanmi live body changed".into()));
                };
                eprintln!("frame {frame}: Kanmi Enter expected region={} receiver={}, live region={region} receiver={receiver}",
                    press.region, press.receiver);
                if (region, receiver) != (press.region, press.receiver) {
                    return Ok(Decision::Stop("checkpoint_mismatch", "Kanmi Enter target did not match the live body".into()));
                }
                if self.route.kanmi_exact.as_ref().is_some_and(|contract| contract.contract != "kanmi-title-r43/v1") {
                    log_kanmi_body(frame, owner, region, receiver, context, load, record, epoch, source_frame);
                }
                let lifetime = NovelWindowLifetime::Armed { first_event: player.events().len() };
                kanmi_body = Some(KanmiBodyPress { owner, context, load, epoch, region, receiver,
                    lifetime, receiver_lifetime: lifetime, region_lifetime: lifetime });
            } else if let Some(press) = &step.keyboard_press {
                if step.wait.daydream_opening_page().is_some_and(|group| group >= 20) {
                    let after = step.after.as_ref().unwrap();
                    daydream_anchor = window.and_then(|(_, owner)| player.host_replay_body_route_anchor(
                        &position.scene, (position.group, position.record),
                        after.group.zip(after.record).filter(|&(group, _)| group >= 0), owner,
                    ).filter(|_| !step.is_daydream_prologue_exit() || player.host_daydream_prologue_tail(owner)));
                    if daydream_anchor.is_none() {
                        return Ok(Decision::Stop("checkpoint_mismatch", format!(
                            "Enter withheld at {:?}: current composed body or adjacent CST wait did not match", step.label,
                        )));
                    }
                }
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
            let neko_save_entry = step.is_neko_save_entry();
            if (step.is_neko_body_press() || neko_save_entry) && !neko_body
                || neko_body && (step.click.is_some() || step.key.is_some()) && !(step.is_neko_body_press() || neko_save_entry)
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
            if !step.probe_points.is_empty() && window.is_some() {
                eprintln!("frame {frame}: probe wait {}", player.host_replay_wait_diagnostics());
            }
            let neko_save_anchor = if neko_save_entry {
                let anchor = window.and_then(|(_, owner)| {
                    (player.host_neko_save_target(owner, 45, "btn", 4) && player.host_neko_slot_zero_absent())
                        .then(|| player.host_replay_body_route_anchor("01", (24, 218), None, owner)).flatten()
                });
                if anchor.is_none() || !self.route.steps[0].neko_startup_language.as_deref()
                    .is_some_and(|language| player.host_neko_startup_language_matches(language))
                {
                    return Ok(Decision::Stop("checkpoint_mismatch", "NEKO Save withheld: r218, named button, empty slot or language did not match".into()));
                }
                anchor
            } else { None };
            let mut pending_mouse_press = None;
            let mut island_meswnd = None;
            if let Some(press) = &step.mouse_press {
                let point = step.click.unwrap();
                let candidates = player.mouse_press_candidate_regions(point)?;
                let owner = window.map(|(_, owner)| owner);
                let receiver = player.input_button_state(press.region).and_then(|button| button.receiver);
                if kanmi && player.host_kanmi_exact_new_game_target() != owner.map(|owner| (press.region, owner)) {
                    return Ok(Decision::Stop("checkpoint_mismatch", "Kanmi New Game live target changed".into()));
                }
                let mut expected_receiver = owner;
                if step.is_island_beach_fullwnd_exit() {
                    let second_hit = player.masked_input_region_at(ISLAND_MESWND_POINTS[0])?;
                    if !player.host_island_beach_fullwnd(0)
                        || !second_hit.is_some_and(|region| region[0] == 108 && region[8] != 0)
                        || player.mouse_press_candidate_regions(ISLAND_MESWND_POINTS[0])? != press.candidates
                    {
                        return Ok(Decision::Stop("checkpoint_mismatch", "ISLAND beach full-window target changed".into()));
                    }
                }
                let island_press = step.island_meswnd_edge().map(|(origin, target)| (origin, Some(target)))
                    .or_else(|| step.is_island_opmovie_exit().then_some(((121, 810), None)))
                    .or_else(|| step.is_island_beach_choice_entry().then_some(((51, 361), None)))
                    .or_else(|| {
                        step.is_island_first_play_branch().then(|| (step.island_meswnd_press_origin().unwrap(), None))
                    });
                if let Some((origin, target)) = island_press {
                    let anchor = owner.and_then(|owner| player.host_island_meswnd_anchor(&position.scene, origin, target, owner));
                    let both_points_match = player.masked_input_region_at(ISLAND_MESWND_POINTS[0])?
                        .is_some_and(|region| region[0] == press.region && region[8] != 0)
                        && player.mouse_press_candidate_regions(ISLAND_MESWND_POINTS[0])? == press.candidates;
                    let beach_window_matches = position.scene != "com01_selB_1"
                        || player.input_button_state(61).is_some_and(|button| button.show != 0 && button.receiver == owner);
                    let Some((anchor, window_owner)) = anchor.zip(owner).filter(|(anchor, owner)| {
                        both_points_match && beach_window_matches && anchor.2 != *owner
                    }) else {
                        return Ok(Decision::Stop("checkpoint_mismatch", format!(
                            "ISLAND root press withheld at {:?}: two-point hit or live CST edge did not match; {}",
                            step.label, player.host_replay_wait_diagnostics(),
                        )));
                    };
                    expected_receiver = Some(anchor.2);
                    island_meswnd = Some(IslandMeswndPress { anchor, window_owner,
                        root_lifetime: NovelWindowLifetime::Armed { first_event: player.events().len() },
                        saw_opening_movie: false, replacement_window: None });
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
            if self.next + 1 == self.route.steps.len()
                && let Some(neko) = &mut self.neko_ordinary
            {
                return neko.start_load(player, frame);
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
                let daydream_tail = step.is_daydream_prologue_exit().then(|| DaydreamTailEvidence {
                    receiver: step.keyboard_press.as_ref().unwrap().receiver, view_frame: None, window: None,
                    fesds_frame: None,
                    prologue_eof_frame: None,
                });
                self.pending = Some(Pending { origin: position, started: frame, step: Some(step_index),
                    mouse_press: pending_mouse_press, keyboard_press: KeyboardPressReceipt::Awaiting,
                    island_meswnd, kanmi_body, daydream_body: daydream_anchor, daydream_tail, neko_save_anchor });
                return Ok(Decision::Input(action));
            }
            if self.next == self.route.steps.len() && self.route.island_bad02 {
                let replay = self.island_bad02.insert(island_bad02::IslandBad02::start());
                return replay.poll(player, frame, self.route.idle_frames, output);
            }
            if self.next == self.route.steps.len() && let Some(route) = &mut self.route.kanmi_exact
                && route.contract == "kanmi-title-sc02-r147-save/v1"
            {
                let Some((owner, _, _, context, load, _, epoch, _, _)) = player.host_kanmi_exact_body() else {
                    return Ok(Decision::Stop("checkpoint_mismatch", "Kanmi ordinary Save body anchor missing".into()));
                };
                let save = route.save_state.insert(kanmi_ordinary::Save::new((context, load, owner, epoch), frame));
                return save.poll(player, frame);
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
                    keyboard_press: KeyboardPressReceipt::Awaiting, island_meswnd: None,
                    kanmi_body: None, daydream_body: None, daydream_tail: None, neko_save_anchor: None });
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
    fn daydream_prologue_chain_can_observe_r96_or_continue_to_r221() {
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
        let mut probe = gate(20, 96);
        probe["state"] = json!("page_probe");
        let steps = route["steps"].as_array_mut().unwrap();
        steps[19] = step(page(19), probe.clone());
        steps.push(json!({"label":"observe r96","wait":probe}));
        assert!(validate(route.clone()).is_ok());
        for (field, value) in [("state", json!("page")), ("record", json!(107)), ("window", json!("meswnd"))] {
            let mut invalid = route.clone();
            invalid["steps"][19]["after"][field] = value.clone();
            invalid["steps"][20]["wait"][field] = value;
            assert!(validate(invalid).is_err());
        }
        let probe = route["steps"][20]["wait"].clone();
        route["steps"][20] = step(probe, page(21));
        route["steps"].as_array_mut().unwrap().push(json!({"label":"observe r107","wait":page(21)}));
        assert!(validate(route.clone()).is_ok());

        let mut wrong_receiver = route.clone();
        wrong_receiver["steps"][20]["keyboard_press"]["receiver"] = json!(65);
        assert!(validate(wrong_receiver).is_err());

        let mut skipped = route.clone();
        skipped["steps"][20]["after"] = page(22);
        skipped["steps"][21]["wait"] = page(22);
        assert!(validate(skipped).is_err());

        let steps = route["steps"].as_array_mut().unwrap();
        steps.pop();
        steps.extend((21..40).map(|group| step(page(group), page(group + 1))));
        steps.push(json!({"label":"observe r221","wait":page(40)}));
        assert!(validate(route.clone()).is_ok());

        let title = json!({"state":"title","scene":"-","group":-1,"record":-1,"window":"title"});
        let mut exit = route.clone();
        exit["steps"][40] = step(page(40), title.clone());
        exit["steps"].as_array_mut().unwrap().push(json!({"label":"observe title","wait":title}));
        assert!(validate(exit.clone()).is_ok());

        let mut title_input = exit.clone();
        title_input["steps"][41]["key"] = json!(13);
        title_input["steps"][41]["after"] = json!({"state":"choice"});
        title_input["steps"].as_array_mut().unwrap().push(json!({"label":"observe choice","wait":{"state":"choice"}}));
        assert!(validate(title_input).is_err());

        exit["steps"][40]["after"]["state"] = json!("choice");
        exit["steps"][41]["wait"]["state"] = json!("choice");
        assert!(validate(exit).is_err());

        route["steps"][40] = step(page(40), gate(41, 222));
        route["steps"].as_array_mut().unwrap().push(json!({"label":"observe r222","wait":gate(41, 222)}));
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
        let records = [
            59, 87, 92, 98, 103, 112, 117, 122, 127, 136, 141, 146,
            151, 156, 160, 165, 170, 175, 180, 185, 190, 195, 200, 213, 218, 337,
        ];
        let page = |group: usize| json!({"state":"page","scene":"01","group":group,
            "record":records[group],"window":"meswnd"});
        let validate = |route| Controller::from_route(serde_json::from_value::<Route>(route).unwrap());
        for terminal in [6, 11, 22, 23, 24, 25] {
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

            let skip = terminal - 2;
            let mut skipped = route.clone();
            skipped["steps"][skip]["after"] = page(terminal);
            skipped["steps"].as_array_mut().unwrap().remove(skip + 1);
            assert!(validate(skipped).is_err());

            if terminal == 25 {
                let mut wrong_record = route;
                wrong_record["steps"][24]["after"]["record"] = json!(338);
                wrong_record["steps"][25]["wait"]["record"] = json!(338);
                assert!(validate(wrong_record).is_err());
            }
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

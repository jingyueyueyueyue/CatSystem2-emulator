//! Ordinary slot 0 replay after the guarded Save entry or a settled title.
use super::{Action, Decision, Route};
use anyhow::{Result, ensure};
use cs2_desktop::{HeadlessPlayer, LogicEvent};
use cs2_runtime::message_object::{ButtonSource, InputRouteReceipt, RoutedPhysicalButton};
use serde::Deserialize;
use serde_json::{Value, json};

#[path = "neko_cstl.rs"]
mod neko_cstl;

type Anchor = (u32, u64, u32, u64, u64);

const BODY_POINT: [i32; 2] = [625, 478];
const BODY_WAITS: [i32; 12] = [218, 337, 368, 381, 399, 405, 410, 461, 475, 481, 483, 485];

fn physical_left(receipt: &InputRouteReceipt, started: u64) -> bool {
    receipt.key == 0 && physical_mouse(receipt, started)
}

fn physical_mouse(receipt: &InputRouteReceipt, started: u64) -> bool {
    receipt.frame > started && receipt.frame - started <= 3
        && receipt.group == 0 && receipt.raw_mask == ButtonSource::Mouse.mask()
        && receipt.pressed == 1 && !receipt.repeated
        && receipt.physical == [RoutedPhysicalButton { source: ButtonSource::Mouse, code: 1 }]
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Config {
    mode: Mode,
    language: String,
    slot_sha256: Option<String>,
    continue_to: Option<i32>,
    cstl_pages: Option<Vec<neko_cstl::Page>>,
    #[serde(default)]
    observe_branch: bool,
    #[serde(default)]
    branch_chapters: Vec<neko_cstl::Chapter>,
    resume_scene: Option<String>,
    resume_at: Option<i32>,
    #[serde(default)]
    save_after: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Mode { Save, Load }

impl Config {
    fn cstl_page(&self, index: usize) -> Option<&neko_cstl::Page> {
        self.cstl_pages.as_ref()?.get(index.checked_sub(BODY_WAITS.len() - 1)?)
    }

    fn body_wait(&self, index: usize) -> (i32, i32) {
        self.cstl_page(index).map_or_else(|| (24 + index as i32, BODY_WAITS[index]),
            |page| (page.group, page.wait))
    }

    fn resume_index(&self) -> Option<usize> {
        let record = self.resume_at?;
        if let Some(chapter) = self.resume_chapter() {
            return self.branch_chapters[chapter].pages.iter().position(|page| page.wait == record);
        }
        self.cstl_pages.as_ref()?.iter().position(|page| page.wait == record)
            .map(|index| index + BODY_WAITS.len() - 1)
    }

    fn resume_chapter(&self) -> Option<usize> {
        let scene = self.resume_scene.as_deref()?;
        self.branch_chapters.iter().position(|chapter| chapter.scene == scene)
    }

    fn restored_position(&self) -> (i32, i32) {
        let index = self.resume_index().unwrap_or(0);
        self.resume_chapter().map_or_else(|| self.body_wait(index), |chapter| {
            let page = &self.branch_chapters[chapter].pages[index];
            (page.group, page.wait)
        })
    }

    fn save_page(&self) -> Option<(&str, &neko_cstl::Page)> {
        if !self.save_after {
            return None;
        }
        if self.observe_branch {
            let chapter = self.branch_chapters.last()?;
            return Some((&chapter.scene, chapter.pages.last()?));
        }
        Some(("01", self.cstl_pages.as_ref()?.last()?))
    }

    fn body_label(&self, index: usize) -> String {
        if index == 0 {
            "Turn at restored 01/24/218".into()
        } else {
            let (group, record) = self.body_wait(index);
            format!("Turn at 01/{group}/{record}")
        }
    }

    pub(super) fn validate(&self, route: &Route) -> Result<()> {
        ensure!(matches!(self.language.as_str(), "jp" | "cn") && !route.eyecatch_08_exit
            && route.kanmi_exact.is_none() && route.kamikaze_post_op.is_none()
            && !route.grisaia_14_first_advance && route.rebellions_r15.is_none()
            && route.page_scenes.is_empty() && route.page_fallback_before.unwrap_or(0) == 0
            && route.page_click.is_none(), "ordinary NEKO requires an explicit JP/CN route");
        let valid_terminal = match &self.cstl_pages {
            Some(pages) => self.mode == Mode::Load && neko_cstl::valid_pages(pages, self.continue_to),
            None => self.continue_to.is_none_or(|record| self.mode == Mode::Load
                && BODY_WAITS[1..].contains(&record)),
        };
        let valid_resume = self.resume_at.is_none() && self.resume_scene.is_none()
            || self.resume_index().is_some() && match self.resume_chapter() {
                Some(chapter) => chapter >= 2 && self.observe_branch,
                None => self.resume_scene.is_none() && self.resume_at.is_some_and(|record| record >= 547),
            };
        ensure!(valid_resume, "invalid ordinary NEKO saved page");
        ensure!(valid_terminal && (!self.observe_branch || self.continue_to == Some(5386)),
            "invalid ordinary NEKO continuation terminal");
        let chapters = self.branch_chapters.as_slice();
        let valid_chapters = chapters.is_empty() || (2..=neko_cstl::FIRST_CHAPTERS.len()).contains(&chapters.len())
            && chapters.iter().map(|chapter| chapter.scene.as_str())
                .eq(neko_cstl::FIRST_CHAPTERS.iter().copied().take(chapters.len()))
            && (chapters.len() == 2 || chapters[..chapters.len() - 1].iter().all(neko_cstl::Chapter::complete));
        ensure!(valid_chapters && (chapters.is_empty() || self.observe_branch)
            && chapters.iter().all(neko_cstl::Chapter::valid), "invalid NEKO branch chapter routes");
        ensure!(!self.save_after || self.mode == Mode::Load && self.save_page().is_some_and(|(scene, page)| {
            let has_incoming = self.resume_scene.as_deref().unwrap_or("01") != scene
                || self.resume_at.is_none_or(|record| record < page.wait);
            has_incoming && if scene == "01" { (547..5386).contains(&page.wait) } else { chapters.len() >= 3 }
        }), "ordinary NEKO Save requires a proven incoming edge to the selected page");
        let (last, prefix) = route.steps.split_last().ok_or_else(|| anyhow::anyhow!("missing NEKO checkpoint"))?;
        ensure!(last.click.is_none() && last.key.is_none() && last.after.is_none()
            && last.wait.hit.is_none() && !last.capture && last.shared_flags.is_empty(),
            "ordinary NEKO requires an input-free checkpoint");
        let valid = match self.mode {
            Mode::Save => self.slot_sha256.is_none() && prefix.len() == 27
                && prefix[0].neko_startup_language.as_ref() == Some(&self.language)
                && prefix[26].is_neko_save_entry() && last.wait.state == "neko_save_panel",
            Mode::Load => self.slot_sha256.as_ref().is_some_and(|sha| sha.len() == 64
                && sha.bytes().all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f')))
                && last.wait.is_neko_startup_screen("title", "-", "title")
                && (prefix.is_empty() || prefix.len() == 1 && prefix[0].is_neko_startup_press()
                    && prefix[0].wait.state == "language_select"
                    && prefix[0].neko_startup_language.as_ref() == Some(&self.language)),
        };
        ensure!(valid, "invalid ordinary NEKO prefix or slot digest");
        Ok(())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Control { OpenSave, SaveSlot, ConfirmSave, TitleLoad, LoadSlot, ConfirmLoad }

impl Control {
    fn target(self) -> (&'static str, &'static str, i32, [i32; 2], &'static str) {
        match self {
            Self::OpenSave => ("meswnd", "btn", 4, [1147, 612], "Open NEKO checkpoint Save"),
            Self::SaveSlot => ("saveload", "btdata", 0, [285, 184], "Write ordinary slot 0"),
            Self::ConfirmSave => ("confirm2", "btn", 0, [559, 377], "Confirm NEKO checkpoint overwrite"),
            Self::TitleLoad => ("title", "btn1", 1, [1155, 325], "Open NEKO Load"),
            Self::LoadSlot => ("saveload", "btdata", 0, [285, 184], "Select ordinary slot 0"),
            Self::ConfirmLoad => ("confirm2", "btn", 0, [559, 377], "Confirm ordinary Load"),
        }
    }
}

struct Press {
    control: Control,
    frame: u64,
    owner: u32,
    region: u32,
    details: Value,
    receipt: Option<InputRouteReceipt>,
    ambiguous: bool,
    file_event: Option<LogicEvent>,
}

struct BodyPress {
    index: usize,
    started: u64,
    anchor: Anchor,
    region: u32,
    receipt: Option<InputRouteReceipt>,
    ambiguous: bool,
    created: Vec<(u64, u32)>,
    root: Option<neko_cstl::RootPress>,
    opening: Option<neko_cstl::Opening>,
    eyecatch: Option<neko_cstl::Eyecatch>,
    branch: Option<(&'static str, i32)>,
    branch_loads: Vec<LogicEvent>,
}

pub(super) struct Ordinary {
    config: Config,
    anchor: Option<Anchor>,
    panel: Option<u32>,
    pending: Option<Press>,
    body_press: Option<BodyPress>,
    cstl_reads: Option<neko_cstl::Reads>,
    chapter: Option<usize>,
}

impl Ordinary {
    pub(super) fn new(config: Config) -> Self {
        let cstl_reads = config.cstl_pages.as_ref().map(|_| neko_cstl::Reads::default());
        let chapter = config.resume_chapter();
        Self { config, anchor: None, panel: None, pending: None, body_press: None, cstl_reads, chapter }
    }

    fn scene(&self) -> &str {
        self.chapter.map_or("01", |chapter| self.config.branch_chapters[chapter].scene.as_str())
    }

    fn cstl_page(&self, index: usize) -> Option<&neko_cstl::Page> {
        self.chapter.map_or_else(|| self.config.cstl_page(index),
            |chapter| self.config.branch_chapters[chapter].pages.get(index))
    }

    fn body_wait(&self, index: usize) -> (i32, i32) {
        match self.chapter {
            Some(chapter) => {
                let page = &self.config.branch_chapters[chapter].pages[index];
                (page.group, page.wait)
            }
            None => self.config.body_wait(index),
        }
    }

    fn body_label(&self, index: usize) -> String {
        if self.chapter.is_none() {
            return self.config.body_label(index);
        }
        let (group, record) = self.body_wait(index);
        format!("Turn at {}/{group}/{record}", self.scene())
    }

    pub(super) fn start_save(&mut self, player: &mut HeadlessPlayer, frame: u64, anchor: Anchor, panel: u32) -> Result<Decision> {
        ensure!(self.config.mode == Mode::Save, "wrong ordinary NEKO direction");
        self.anchor = Some(anchor);
        self.panel = Some(panel);
        self.press(player, frame, Control::SaveSlot)
    }

    pub(super) fn start_load(&mut self, player: &mut HeadlessPlayer, frame: u64) -> Result<Decision> {
        ensure!(self.config.mode == Mode::Load, "wrong ordinary NEKO direction");
        self.press(player, frame, Control::TitleLoad)
    }

    pub(super) fn active(&self) -> bool {
        self.pending.is_some() || self.body_press.is_some()
    }

    pub(super) fn observe(&mut self, player: &HeadlessPlayer) {
        if let Some(press) = &mut self.body_press {
            let started = press.started;
            if let Some(opening) = &mut press.opening {
                opening.observe_composition(player);
            }
            if let Some(root) = &mut press.root {
                root.observe(player, started);
            }
            for receipt in player.host_input_route_receipts().iter()
                .filter(|receipt| physical_left(receipt, started))
            {
                if (receipt.region, receipt.receiver, receipt.response) == (press.region, press.anchor.2, 1) {
                    press.ambiguous |= press.receipt.replace(receipt.clone()).is_some();
                } else if receipt.response == 0 || receipt.receiver == press.anchor.2 {
                    press.ambiguous = true;
                }
            }
            return;
        }
        let Some(press) = &mut self.pending else { return; };
        let started = press.frame;
        for receipt in player.host_input_route_receipts().iter()
            .filter(|receipt| physical_left(receipt, started))
        {
            if (receipt.region, receipt.receiver) == (press.region, press.owner) && receipt.response <= 1 {
                press.ambiguous |= press.receipt.replace(receipt.clone()).is_some();
            } else if receipt.response == 0 {
                press.ambiguous = true;
            }
        }
    }

    pub(super) fn observe_events(&mut self, events: &[LogicEvent]) {
        if let Some(reads) = &mut self.cstl_reads {
            reads.observe(events);
        }
        if let Some(press) = &mut self.body_press {
            let started = press.started;
            if let Some(opening) = &mut press.opening {
                opening.observe_events(events, started);
            }
            if let Some(eyecatch) = &mut press.eyecatch {
                eyecatch.observe(events, started);
            }
            if press.branch.is_some() {
                press.branch_loads.extend(events.iter().filter(|event| event.frame > started
                    && event.kind == "neko-boundary-load").cloned());
            }
            for event in events.iter().filter(|event| event.frame > started && event.kind == "task-create") {
                let mut fields = event.detail.split_ascii_whitespace();
                let owner = fields.next().and_then(|word| word.strip_prefix("id="))
                    .and_then(|id| id.parse::<u32>().ok());
                if let Some(owner) = owner
                    && fields.next() == Some("kind=34")
                    && fields.next().is_some_and(|word| word.starts_with("parent="))
                    && fields.next() == Some("name=\"meswnd\"")
                {
                    press.created.push((event.frame, owner));
                }
            }
            return;
        }
        let Some(press) = &mut self.pending else { return; };
        let started = press.frame;
        for event in events.iter().filter(|event| event.frame > started) {
            let relevant = match press.control {
                Control::OpenSave | Control::SaveSlot | Control::ConfirmSave => event.kind == "fes-datasave",
                Control::ConfirmLoad => event.kind == "neko-slot-open",
                _ => false,
            };
            if relevant {
                press.ambiguous |= press.file_event.replace(event.clone()).is_some();
            }
        }
    }

    pub(super) fn poll(&mut self, player: &mut HeadlessPlayer, frame: u64, idle: u64) -> Result<Decision> {
        if self.body_press.is_some() {
            return self.poll_body(player, frame, idle);
        }
        let press = self.pending.as_ref().unwrap();
        if !player.host_neko_startup_language_matches(&self.config.language) || press.ambiguous {
            return Ok(Decision::Stop("checkpoint_mismatch", "ordinary NEKO language or receipt mismatch".into()));
        }
        let Some(receipt) = &press.receipt else {
            return Ok(Decision::Stop("input_missed", "ordinary NEKO control did not receive the physical press".into()));
        };
        let (_, _, _, _, label) = press.control.target();
        if press.control == Control::OpenSave && receipt.response != 0 {
            return Ok(Decision::Stop("input_missed", "NEKO Save toolbar did not consume the press".into()));
        }
        let next = match press.control {
            Control::TitleLoad => Some(Control::LoadSlot),
            Control::LoadSlot => Some(Control::ConfirmLoad),
            Control::OpenSave => Some(Control::SaveSlot),
            Control::SaveSlot if self.config.save_after => Some(Control::ConfirmSave),
            _ => None,
        };
        let restored = self.config.restored_position();
        let opened_panel = if press.control == Control::OpenSave {
            self.checkpoint_panel(player, false)
        } else {
            None
        };
        let mut ready = opened_panel.is_some() || next.is_some_and(|control| self.owner(player, control).is_some());
        if let Some(event) = &press.file_event {
            let valid = receipt.frame <= event.frame && match press.control {
                Control::ConfirmSave | Control::SaveSlot if next.is_none() =>
                    event.detail.starts_with("slot=0 container=") && event.detail.ends_with(" written=true"),
                Control::ConfirmLoad => self.config.slot_sha256.as_ref()
                    .is_some_and(|sha| event.detail.ends_with(&format!(" sha256={sha}"))),
                _ => false,
            };
            if !valid {
                return Ok(Decision::Stop("checkpoint_mismatch", format!("ordinary NEKO file receipt: {}", event.detail)));
            }
            ready = match press.control {
                Control::SaveSlot => player.host_neko_saved_panel(self.anchor.unwrap()) == self.panel
                    && self.panel.is_some() && player.host_neko_slot_sha256()?.is_some(),
                Control::ConfirmSave => self.checkpoint_panel(player, false) == self.panel
                    && self.panel.is_some()
                    && player.host_neko_slot_sha256()?.is_some_and(|sha| Some(&sha) != self.config.slot_sha256.as_ref()),
                Control::ConfirmLoad => {
                    let ((_, _, scene, _, _), window) = player.host_replay_story_position();
                    scene.eq_ignore_ascii_case(self.scene()) && window.is_some_and(|(name, owner)| {
                        name == "meswnd" && player.host_replay_body_route_anchor(scene, restored, None, owner)
                            .is_some_and(|(_, _, _, _, source)| event.frame <= source)
                    }) && player.host_neko_slot_sha256()? == self.config.slot_sha256
                }
                _ => false,
            };
        }
        if !ready {
            return Ok(if frame - press.frame >= idle {
                Decision::Stop("unrecognized_wait", format!("after {label}: {}", player.host_replay_wait_diagnostics()))
            } else { Decision::Wait });
        }

        let mut details = press.details.clone();
        details["response"] = json!(receipt.response);
        println!("frame={} neko-save-control {details}", press.frame);
        eprintln!("replay frame={}: verified mouse press {label:?}: {receipt:?}", receipt.frame);
        println!("replay frame={frame} verified={label:?}");
        let control = press.control;
        let file_frame = press.file_event.as_ref().map(|event| event.frame);
        self.pending = None;
        if let Some(next) = next {
            if let Some(panel) = opened_panel {
                self.panel = Some(panel);
            }
            return self.press(player, frame, next);
        }
        if control == Control::ConfirmSave {
            let sha = player.host_neko_slot_sha256()?.unwrap();
            let (group, record) = restored;
            let (scene, page) = self.config.save_page().unwrap();
            println!("frame={frame} neko-checkpoint-written {}", json!({
                "scene": scene, "group": page.group, "record": page.wait, "slot": 0,
                "language": self.config.language, "sha256": sha, "restored_from": [group, record],
                "previous_sha256": self.config.slot_sha256, "write_frame": file_frame,
            }));
            println!("replay frame={frame} checkpoint=\"Stop after NEKO checkpoint Save\"");
            return Ok(Decision::Done);
        }
        if self.config.continue_to.is_some() {
            if let Some(index) = self.config.resume_index() {
                return self.body_turn(player, frame, index, file_frame);
            }
            return self.body_turn(player, frame, 0, None);
        }
        let terminal = match self.config.mode {
            Mode::Save => "Stop after ordinary Save",
            Mode::Load => "Stop at restored 01/24/218",
        };
        eprintln!("frame {frame}: probe wait {}", player.host_replay_wait_diagnostics());
        println!("replay frame={frame} checkpoint={terminal:?}");
        Ok(Decision::Done)
    }

    fn body_turn(
        &mut self, player: &mut HeadlessPlayer, frame: u64, index: usize, earliest: Option<u64>,
    ) -> Result<Decision> {
        let scene = self.scene();
        let (group, record) = self.body_wait(index);
        let terminal = self.chapter.map_or(self.config.continue_to == Some(record),
            |chapter| self.config.branch_chapters[chapter].pages.len() == index + 1);
        let branch = if terminal && self.config.observe_branch && self.chapter.is_none() {
            let Some([a, b]) = player.host_neko_branch_flags() else {
                return Ok(Decision::Stop("checkpoint_mismatch", "01/491/5386: branch flags unavailable".into()));
            };
            println!("frame={frame} neko-branch-flags scene=01 group=491 record=5386 flag1003={a} flag1004={b}");
            Some(if a != 0 && b == 0 { ("H01", 51) } else { ("01_02", 24) })
        } else if terminal && self.config.branch_chapters.len() >= 3 {
            self.chapter.and_then(|chapter| {
                let name = neko_cstl::FIRST_CHAPTERS.get(chapter + 1)?;
                let next = self.config.branch_chapters.get(chapter + 1)?;
                Some((*name, next.pages.first()?.wait))
            })
        } else {
            None
        };
        let next = (!terminal).then(|| self.body_wait(index + 1));
        let ((_, _, observed, _, _), window) = player.host_replay_story_position();
        let observed = observed.to_owned();
        let anchor = window.and_then(|(name, owner)| {
            (name == "meswnd" && observed.eq_ignore_ascii_case(scene)).then_some(())?;
            player.host_replay_body_route_anchor(&observed, (group, record), next, owner)
        });
        let mask = player.masked_input_region_at(BODY_POINT)?;
        let candidates = player.mouse_press_candidate_regions(BODY_POINT)?;
        let receivers: Vec<_> = candidates.iter().map(|&region| {
            (region, player.input_button_state(region).and_then(|button| button.receiver))
        }).collect();
        eprintln!("frame {frame}: probe {BODY_POINT:?} masked={mask:?} press_candidates={candidates:?} window={window:?} press_receivers={receivers:?}");
        eprintln!("frame {frame}: probe wait {}", player.host_replay_wait_diagnostics());

        let target = anchor.and_then(|anchor| player.host_neko_body_click_region(anchor.2, BODY_POINT)
            .map(|region| (anchor, region)));
        let Some(((anchor, region), hit)) = target.zip(mask).filter(|&((anchor, region), hit)| {
            hit[8] != 0 && candidates.contains(&hit[0])
                && receivers.iter().filter(|(_, receiver)| *receiver == Some(anchor.2)).copied()
                    .eq([(region, Some(anchor.2))])
        }) else {
            return Ok(Decision::Stop("checkpoint_mismatch", format!("{scene}/{group}/{record}: live body target or CST edge changed")));
        };
        let root = if let Some(page) = self.cstl_page(index).cloned() {
            let Some(root) = neko_cstl::RootPress::live(player, anchor.2, hit[0]) else {
                return Ok(Decision::Stop("checkpoint_mismatch", format!(
                    "{scene}/{group}/{record}: masked region {} is not a live meswnd parent button", hit[0],
                )));
            };
            let Some((earliest, proof)) = earliest.zip(player.host_neko_cstl_body_proof(&observed, (group, record), anchor)) else {
                return Ok(Decision::Stop("checkpoint_mismatch", format!(
                    "{scene}/{group}/{record}: missing same-generation CSTL body proof or prior press frame",
                )));
            };
            let earliest = if self.chapter.is_some() && index == 0
                || self.config.resume_chapter() == self.chapter && self.config.resume_index() == Some(index) {
                earliest.max(proof.0)
            } else { earliest };
            let scene = scene.to_owned();
            if let Err(reason) = self.cstl_reads.as_mut().unwrap().verify(
                frame, &scene, &page, &self.config.language, anchor, proof, earliest,
            ) {
                return Ok(Decision::Stop("checkpoint_mismatch", format!("{scene}/{group}/{record}: {reason}")));
            }
            Some(root)
        } else {
            None
        };
        let scene = self.scene();
        let save = terminal && self.config.save_page().is_some_and(|(target, _)| target == scene);
        let label = if save {
            format!("Save at {scene}/{group}/{record}")
        } else if terminal && branch.is_none() {
            format!("Stop at {scene}/{group}/{record} without input")
        } else {
            self.body_label(index)
        };
        let details = json!({"label": label, "group": group, "record": record, "owner": anchor.2,
            "control": "meswnd low button", "point": BODY_POINT, "hit_region": hit[0],
            "region": region, "candidates": candidates, "receivers": receivers});
        println!("frame={frame} neko-body-target {details}");
        println!("replay frame={frame} checkpoint={label:?}");
        if terminal && branch.is_none() {
            if save {
                self.anchor = Some(anchor);
                return self.press(player, frame, Control::OpenSave);
            }
            return Ok(Decision::Done);
        }

        let owner = anchor.2;
        eprintln!("frame {frame}: guarded press {BODY_POINT:?} candidates={candidates:?} window={window:?} receiver=Some({owner})");
        self.body_press = Some(BodyPress {
            index, started: frame, anchor, region, receipt: None, ambiguous: false, created: Vec::new(), root,
            opening: (scene == "01" && record == 3631).then(neko_cstl::Opening::default),
            eyecatch: (scene == "01" && record == 4575).then(neko_cstl::Eyecatch::default),
            branch,
            branch_loads: Vec::new(),
        });
        let [x, y] = BODY_POINT;
        Ok(Decision::Input(Action::Pointer { x, y, button: Some(0) }))
    }

    fn poll_body(&mut self, player: &mut HeadlessPlayer, frame: u64, idle: u64) -> Result<Decision> {
        // Controller::poll runs only after the device scan has observed release.
        let press = self.body_press.as_ref().unwrap();
        if press.ambiguous || !player.host_neko_startup_language_matches(&self.config.language) {
            return Ok(Decision::Stop("checkpoint_mismatch", "NEKO body language or receipt mismatch".into()));
        }
        let Some(receipt) = &press.receipt else {
            return Ok(Decision::Stop("input_missed", "NEKO body did not receive the physical press".into()));
        };
        if let Some(root) = &press.root
            && let Err(reason) = root.check(receipt.frame)
        {
            let (group, record) = self.body_wait(press.index);
            return Ok(Decision::Stop("input_missed", format!("{}/{group}/{record}: {reason}", self.scene())));
        }
        if let Some((scene, record)) = press.branch {
            let previous_scene = self.scene();
            let previous_position = self.body_wait(press.index);
            let ((_, state, observed_scene, group, observed_record), window) = player.host_replay_story_position();
            if state == "page" && observed_scene.eq_ignore_ascii_case(scene) && (group, observed_record) == (0, record)
                && let Some(("meswnd", owner)) = window
                && player.host_replay_body_route_anchor(observed_scene, (0, record), None, owner)
                    .is_some_and(|(_, load, _, _, source)| load != press.anchor.1 && source >= receipt.frame)
            {
                let chapter = self.config.branch_chapters.iter().position(|chapter| chapter.scene.eq_ignore_ascii_case(scene));
                if let Some(chapter) = chapter {
                    let expected = neko_cstl::chapter_path(previous_scene, &self.config.branch_chapters[chapter].scene)
                        .expect("validated NEKO chapter transition");
                    let matched = press.branch_loads.len() == expected.len()
                        && press.branch_loads.iter().zip(expected).all(|(event, member)|
                            neko_cstl::chapter_load_matches(event, member, receipt.frame));
                    if !matched {
                        return Ok(Decision::Stop("unexpected_branch", format!("NEKO branch CST sequence changed: {:?}", press.branch_loads)));
                    }
                }
                if let Some(root) = &press.root {
                    root.log(frame, press.started, receipt);
                }
                let label = self.body_label(press.index);
                eprintln!("replay frame={}: verified mouse press {label:?}: {receipt:?}", receipt.frame);
                println!("replay frame={frame} verified={label:?}");
                let earliest = receipt.frame;
                self.body_press = None;
                if let Some(chapter) = chapter {
                    self.chapter = Some(chapter);
                    return self.body_turn(player, frame, 0, Some(earliest));
                }
                println!("replay frame={frame} checkpoint={:?}", format!("Stop at {scene}/0/{record} without input"));
                return Ok(Decision::Done);
            }
            if matches!(state, "page" | "choice" | "title" | "menu")
                && !(state == "page" && (observed_scene.eq_ignore_ascii_case(previous_scene)
                        && (group, observed_record) == previous_position
                    || observed_scene.eq_ignore_ascii_case(scene) && (group, observed_record) == (0, record)))
            {
                return Ok(Decision::Stop("unexpected_branch", format!("NEKO branch at {observed_scene}/{group}/{observed_record} {state}")));
            }
            return Ok(if frame - press.started >= idle {
                Decision::Stop("unrecognized_wait", format!("NEKO branch did not reach {scene}/0/{record}"))
            } else { Decision::Wait });
        }
        let (next_group, next_record) = self.body_wait(press.index + 1);
        let ((_, state, scene, group, record), window) = player.host_replay_story_position();
        let expected_scene = self.scene();
        let position = (group, record);
        let known_wait = state == "page" && [self.body_wait(press.index), (next_group, next_record)].contains(&position);
        if (scene != "-" && !scene.eq_ignore_ascii_case(expected_scene)) || !(matches!(state, "active" | "unknown") || known_wait)
        {
            return Ok(Decision::Stop("unexpected_branch", format!("NEKO body at {scene}/{group}/{record} {state}")));
        }
        if let Some((_, owner)) = window
            && let Some((context, load, _, epoch, source_frame)) = player.host_replay_body_route_anchor(
                scene, (next_group, next_record), None, owner,
            )
        {
            let (old_context, old_load, old_owner, old_epoch, _) = press.anchor;
            let same_window = press.created.is_empty() && owner == old_owner;
            let rebuilt_window = matches!(press.created.as_slice(),
                [(created, created_owner)] if *created >= receipt.frame && *created_owner == owner);
            let kitchen_rebuild = self.chapter.is_none() && press.index == 0 && rebuilt_window;
            let valid_window = if let Some(opening) = &press.opening {
                player.host_neko_cstl_body_proof(scene, (next_group, next_record),
                    (context, load, owner, epoch, source_frame))
                    .is_some_and(|(_, marked, complete, _)| {
                        opening.completed(receipt.frame, marked.min(complete))
                            && (same_window && epoch != old_epoch || rebuilt_window)
                    })
            } else if let Some(eyecatch) = &press.eyecatch {
                rebuilt_window && owner != old_owner && eyecatch.completed(receipt.frame, frame)
            } else {
                same_window && epoch != old_epoch || kitchen_rebuild
            };
            if (context, load) != (old_context, old_load) || source_frame < receipt.frame
                || !valid_window
            {
                return Ok(Decision::Stop("checkpoint_mismatch", format!(
                    "{expected_scene}/{next_group}/{next_record}: CST load, body source or meswnd lifetime changed; creations={:?}",
                    press.created,
                )));
            }
            if let Some(root) = &press.root {
                root.log(frame, press.started, receipt);
            }
            let label = self.body_label(press.index);
            eprintln!("replay frame={}: verified mouse press {label:?}: {receipt:?}", receipt.frame);
            println!("replay frame={frame} verified={label:?}");
            let next_index = press.index + 1;
            let earliest = receipt.frame;
            self.body_press = None;
            return self.body_turn(player, frame, next_index, Some(earliest));
        }
        let idle = if press.opening.is_some() { idle.max(14_400) } else { idle };
        Ok(if frame - press.started >= idle {
            Decision::Stop("unrecognized_wait", format!("NEKO body did not reach {expected_scene}/{next_group}/{next_record}"))
        } else {
            Decision::Wait
        })
    }

    fn checkpoint_panel(&self, player: &HeadlessPlayer, confirm: bool) -> Option<u32> {
        let (scene, page) = self.config.save_page()?;
        player.host_neko_checkpoint_save_panel(self.anchor?, scene, (page.group, page.wait), confirm, self.panel)
    }

    fn owner(&self, player: &HeadlessPlayer, control: Control) -> Option<u32> {
        match control {
            Control::OpenSave => Some(self.anchor?.2),
            Control::ConfirmSave => self.checkpoint_panel(player, true),
            Control::SaveSlot if self.config.save_after => self.checkpoint_panel(player, false)
                .filter(|owner| Some(*owner) == self.panel),
            Control::SaveSlot => player.host_neko_save_panel(self.anchor?).filter(|owner| Some(*owner) == self.panel),
            Control::TitleLoad => player.host_neko_title_load(),
            Control::LoadSlot => player.host_neko_load_panel(false),
            Control::ConfirmLoad => player.host_neko_load_panel(true),
        }
    }

    fn press(&mut self, player: &mut HeadlessPlayer, frame: u64, control: Control) -> Result<Decision> {
        let (window, name, bank, point, label) = control.target();
        if !player.host_neko_startup_language_matches(&self.config.language)
            || player.host_neko_slot_sha256()? != self.config.slot_sha256
        {
            return Ok(Decision::Stop("checkpoint_mismatch", format!("{label}: language or slot 0 identity changed")));
        }
        let owner = self.owner(player, control);
        let mask = player.masked_input_region_at(point)?;
        let candidates = player.mouse_press_candidate_regions(point)?;
        let receivers: Vec<_> = candidates.iter().map(|&region| {
            (region, player.input_button_state(region).and_then(|button| button.receiver))
        }).collect();
        let Some((owner, mask)) = owner.zip(mask).filter(|&(owner, mask)| {
            mask[8] != 0 && candidates.iter().filter(|&&candidate| candidate == mask[0]).count() == 1
                && player.host_neko_save_target(owner, mask[0], name, bank)
        }) else {
            return Ok(Decision::Stop("checkpoint_mismatch", format!(
                "{label}: {window} {name}[{bank}] point={point:?} owner={owner:?} mask={mask:?} candidates={candidates:?} receivers={receivers:?}"
            )));
        };
        let region = mask[0];
        if control == Control::SaveSlot
            && !self.config.save_after
            && (owner, region, candidates.as_slice()) != (12, 32, &[3, 32, 77, 52, 57, 123, 7])
        {
            return Ok(Decision::Stop("checkpoint_mismatch", format!(
                "ordinary Save slot 0 changed: owner={owner} region={region} candidates={candidates:?}"
            )));
        }
        let control_name = match control {
            Control::SaveSlot | Control::LoadSlot => "btdata[0]",
            Control::TitleLoad => "title btn1[1]",
            Control::ConfirmLoad | Control::ConfirmSave => "confirm2 btn[0]",
            Control::OpenSave => "meswnd btn[4]",
        };
        let mut details = json!({"label": label, "window": window, "owner": owner, "control": control_name,
            "block": "WAIT", "point": point, "hit_region": region, "candidates": candidates, "receivers": receivers,
            "region": region, "ready": true, "driver_frame": frame});
        match control {
            Control::SaveSlot | Control::LoadSlot => {
                let load = control == Control::LoadSlot;
                let exists = load || self.config.save_after;
                details["globals"] = json!({"800": u8::from(load), "801": u8::from(!load), "860": u8::from(exists)});
                details["locals"] = json!({"100": 0, "110": u8::from(load)});
                details["slot"] = json!(0);
                details["slot_exists"] = json!(exists);
                if load { details["ttlflow"] = json!("LOAD_WAIT"); }
            }
            Control::TitleLoad => {
                details["flow"] = json!("WAIT_TITLE");
                details["ttlflow"] = json!("FES_WAIT");
            }
            Control::OpenSave => { details["block"] = json!("MES_SHOW"); }
            Control::ConfirmLoad | Control::ConfirmSave => {
                details["saveload"] = json!("CONFIRM_WAIT");
                details["slot"] = json!(0);
            }
        }
        if let Some((context, load, owner, _, _)) = self.anchor {
            let (scene, group, record) = self.config.save_page().map_or(("01", 24, 218),
                |(scene, page)| (scene, page.group, page.wait));
            details["retained"] = json!([scene, group, record, context, load, owner]);
        }
        eprintln!("frame {frame}: guarded press {point:?} candidates={candidates:?} window={:?} receiver={:?}",
            Some((window, owner)), Some(owner));
        println!("replay frame={frame} checkpoint={label:?}");
        self.pending = Some(Press { control, frame, owner, region, details, receipt: None, ambiguous: false, file_event: None });
        let [x, y] = point;
        Ok(Decision::Input(Action::Pointer { x, y, button: Some(0) }))
    }
}

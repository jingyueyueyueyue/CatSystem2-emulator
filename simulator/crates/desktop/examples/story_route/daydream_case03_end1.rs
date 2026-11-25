use super::{Action, ButtonSource, DaydreamBodyAnchor, Decision, KeyboardPress, KeyboardPressReceipt, LogicEvent, RoutedPhysicalButton};
use anyhow::{Result, ensure};
use cs2_desktop::HeadlessPlayer;
use cs2_runtime::InputKey;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};

#[path = "daydream_ordinary.rs"]
pub(super) mod ordinary;

const MOVIE_STARTED: &str = " movie_daydream_end1_started handle=";
const MOVIE_FINISHED: &str = " movie_daydream_end1_released handle=";
pub(super) const SAVE_SCENE: &str = "c01_01_01";
pub(super) const SAVE_GROUP: usize = 13;
pub(super) const SAVE_RECORD: i32 = 121;
pub(super) const SAVE_SUCCESSOR: i32 = 123;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Plan {
    paths: Vec<Vec<String>>,
    pages: BTreeMap<String, Vec<i32>>,
    #[serde(default)]
    save_entry_at_r121: bool,
    #[serde(default)]
    write_slot_at_r121: bool,
    pub(super) ordinary_load_sha256: Option<String>,
    #[serde(default)]
    pub(super) resume_after_load: bool,
}

impl Plan {
    pub(super) fn validate(&self) -> Result<()> {
        if let Some(sha) = &self.ordinary_load_sha256 {
            ensure!(!self.save_entry_at_r121
                && !self.write_slot_at_r121 && sha.len() == 64
                && sha.bytes().all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f')),
                "invalid Daydream ordinary cold load plan");
            if !self.resume_after_load {
                ensure!(self.paths.is_empty() && self.pages.is_empty(),
                    "Daydream load-only plan cannot include a continuation route");
                return Ok(());
            }
            ensure!(self.waits(SAVE_SCENE).get(SAVE_GROUP..=SAVE_GROUP + 1)
                == Some(&[SAVE_RECORD, SAVE_SUCCESSOR]), "Daydream continuation must contain the saved and successor pages");
        } else {
            ensure!(!self.resume_after_load, "Daydream continuation requires a verified ordinary slot hash");
        }
        ensure!(!self.write_slot_at_r121 || self.save_entry_at_r121,
            "Daydream slot write must use the guarded Save entry");
        ensure!(self.paths.len() == 6 && self.paths.iter().all(|path| {
            path.len() > 100
                && path[..4] == ["start", "c00_00", "brunch", "brunch_a"]
                && path[path.len() - 4..] == ["c03_06_h", "c03_ed", "c03_ed_x", "c99_10"]
        }), "Daydream CASE 3 END1 requires the six fresh-profile paths");
        let starts: BTreeSet<_> = self.paths.iter().map(|path| path[5].as_str()).collect();
        ensure!(starts == BTreeSet::from(["c01_01_01", "c02_01_01", "c03_01_01"])
            && self.pages.iter().all(|(scene, waits)| {
                !waits.is_empty() && waits[0] >= 0 && waits.windows(2).all(|pair| pair[0] < pair[1])
                    && self.paths.iter().any(|path| path.contains(scene))
            }), "Daydream CASE 3 END1 has invalid CST wait lists");
        Ok(())
    }

    fn waits(&self, scene: &str) -> &[i32] {
        self.pages.get(scene).map_or(&[], Vec::as_slice)
    }
}

struct Choice {
    option: u32,
    targets: &'static [&'static str],
}

fn choice(scene: &str) -> Option<Choice> {
    let (option, targets): (u32, &[&str]) = match scene {
        "c99_02_a_01" => (3, &["c99_02_a_01_a", "c99_02_a_01_b", "c99_02_x_01"]),
        "c99_02_b_01" => (6, &["c99_02_b_01_a", "c99_02_b_01_b", "c99_02_x_01"]),
        "c99_02_c_01" => (9, &["c99_02_c_01_a", "c99_02_x_01"]),
        "brunch_2" => (12, &["c01_04", "c02_04", "c03_04"]),
        _ => return None,
    };
    Some(Choice { option, targets })
}

enum Press {
    Body {
        frame: u64,
        binding: KeyboardPress,
        receipt: KeyboardPressReceipt,
    },
    Menu {
        frame: u64,
        region: u32,
        owner: u32,
        received: bool,
    },
}

impl Press {
    fn verified(&self) -> bool {
        match self {
            Self::Body { receipt, .. } => *receipt == KeyboardPressReceipt::Verified,
            Self::Menu { received, .. } => *received,
        }
    }
}

pub(super) struct Replay {
    paths: Vec<usize>,
    scene_index: usize,
    scene: String,
    load_frame: u64,
    next_body: usize,
    last_body: Option<DaydreamBodyAnchor>,
    body_owner_created: Option<(u64, u32)>,
    save_entry: Option<DaydreamBodyAnchor>,
    save_click: Option<(u32, ordinary::Click)>,
    save_written: Option<u64>,
    last_progress: u64,
    pressed: Option<Press>,
    choice_sent: bool,
    movie_start: Option<(u64, u32)>,
    movie_end: Option<u64>,
    failure: Option<String>,
}

impl Replay {
    pub(super) fn from_second_wait(plan: &Plan, prefix: &[String], scene: &str, frame: u64) -> Self {
        let normalized: Vec<_> = prefix.iter().map(|name| name.to_ascii_lowercase()).collect();
        let paths = plan.paths.iter().enumerate().filter_map(|(index, path)| {
            path.starts_with(&normalized).then_some(index)
        }).collect();
        Self::at_case_start(paths, scene, frame)
    }

    pub(super) fn from_ordinary_load(plan: &Plan, frame: u64, opened: u64, restored: DaydreamBodyAnchor) -> Self {
        let paths = plan.paths.iter().enumerate().filter_map(|(index, path)| {
            (path[5] == SAVE_SCENE).then_some(index)
        }).collect();
        let mut replay = Self::at_case_start(paths, SAVE_SCENE, frame);
        replay.load_frame = opened;
        replay.next_body = SAVE_GROUP + 1;
        replay.last_body = Some(restored);
        replay.last_progress = restored.4;
        replay
    }

    fn at_case_start(paths: Vec<usize>, scene: &str, frame: u64) -> Self {
        Self {
            paths, scene_index: 5, scene: scene.to_owned(), load_frame: 0, next_body: 1,
            last_body: None, body_owner_created: None, save_entry: None, last_progress: frame, pressed: None, choice_sent: false,
            save_click: None, save_written: None, movie_start: None, movie_end: None, failure: None,
        }
    }

    pub(super) fn observe_events(&mut self, plan: &Plan, events: &[LogicEvent]) {
        for event in events {
            if self.failure.is_some() {
                break;
            }
            if plan.write_slot_at_r121 && event.kind == "fes-datasave" {
                let valid = self.save_click.as_ref().is_some_and(|(_, click)| event.frame > click.started)
                    && ordinary::file_fields(&event.detail, "slot=0 container=", " written=true").is_some();
                if !valid || self.save_written.replace(event.frame).is_some() {
                    self.failure = Some("Daydream ordinary slot write receipt mismatch".into());
                }
            }
            if event.kind == "task-create" && event.frame > self.last_progress
                && let Some((_, _, owner, _, _)) = self.last_body
                && let Some((id, parent)) = message_window_creation(&event.detail)
                && id == owner
            {
                self.body_owner_created = Some((event.frame, parent));
            }
            if event.kind != "native-call" {
                continue;
            }
            let detail = event.detail.as_str();
            if let Some(raw) = detail.split_once(" debug=\"sscript loaded > ").and_then(|(_, name)| name.strip_suffix('"')) {
                let next = raw.to_ascii_lowercase();
                if self.next_body != plan.waits(&self.scene).len() || choice(&self.scene).is_some() && !self.choice_sent {
                    self.failure = Some(format!("{} left before its guarded waits or choice for {next}", self.scene));
                    continue;
                }
                let reroll = next == self.scene && matches!(next.as_str(), "brunch_b_12" | "brunch_b_13" | "brunch_b_23");
                if !reroll {
                    self.scene_index += 1;
                    self.paths.retain(|&i| plan.paths[i].get(self.scene_index) == Some(&next));
                }
                if self.paths.is_empty() {
                    self.failure = Some(format!("unlisted Daydream load {next} at index {}", self.scene_index));
                }
                self.scene = next;
                self.load_frame = event.frame;
                self.next_body = 0;
                self.last_body = None;
                self.body_owner_created = None;
                self.last_progress = event.frame;
                self.choice_sent = false;
                eprintln!("frame {}: Daydream END1 load={} index={} reroll={reroll}", event.frame, self.scene, self.scene_index);
            }
            if detail.contains(" debug=\"[command] fselect") && choice(&self.scene).is_none() {
                self.failure = Some(format!("unlisted Daydream choice in {}", self.scene));
            }
            if detail.contains(" movie_daydream_end1_interrupted handle=") {
                self.failure = Some("ed_c03 was interrupted while active".into());
            }
            if let Some(handle) = number_after(detail, MOVIE_STARTED) {
                if self.scene != "c03_ed" || self.movie_start.replace((event.frame, handle)).is_some() {
                    self.failure = Some("unexpected ed_c03 playback start".into());
                }
                self.last_progress = event.frame;
            }
            if let Some(handle) = number_after(detail, MOVIE_FINISHED) {
                if self.scene != "c03_ed" || !self.movie_start.is_some_and(|(start, id)| id == handle && start < event.frame)
                    || self.movie_end.replace(event.frame).is_some()
                {
                    self.failure = Some("ed_c03 natural release did not match the active instance".into());
                }
                self.last_progress = event.frame;
            }
        }
    }

    pub(super) fn observe_receipts(&mut self, player: &HeadlessPlayer) {
        if let Some((_, click)) = &mut self.save_click {
            click.observe(player);
            return;
        }
        let Some(press) = &mut self.pressed else { return; };
        let receipts = player.host_input_route_receipts();
        match press {
            Press::Body { frame, binding, receipt } => {
                let consumed = receipts.iter().filter(|item| item.frame > *frame && item.pressed != 0 && item.response == 0).count();
                if consumed == 0 {
                    return;
                }
                let events = player.events();
                let verified = receipt.observe(binding, *frame, receipts, events);
                if let Some(item) = verified.filter(|item| consumed == 1
                    && events.iter().rev().take_while(|event| event.frame >= item.frame).any(|event| {
                        event.frame == item.frame && event.kind == "input-wait" && event.detail == "native 71 address=0xd6d"
                    }))
                {
                    eprintln!("frame {}: Daydream END1 Enter {item:?}", item.frame);
                } else {
                    *receipt = KeyboardPressReceipt::Rejected;
                }
            }
            Press::Menu { frame, region, owner, received } => {
                for item in receipts.iter().filter(|item| item.frame > *frame && item.pressed != 0 && item.response <= 1) {
                    let target = (item.region, item.receiver) == (*region, *owner);
                    let valid = item.frame - *frame <= 3 && item.key == 0 && item.group == 0
                        && item.pressed == 1 && !item.repeated && item.raw_mask == ButtonSource::Mouse.mask()
                        && item.physical == [RoutedPhysicalButton { source: ButtonSource::Mouse, code: 1 }];
                    if target && valid && !*received {
                        *received = true;
                        eprintln!("frame {}: Daydream END1 choice press {item:?}", item.frame);
                    } else if target || item.response == 0 {
                        self.failure = Some("ambiguous Daydream END1 choice receipt".into());
                    }
                }
            }
        }
    }

    pub(super) fn poll(&mut self, plan: &Plan, player: &mut HeadlessPlayer, frame: u64, idle: u64) -> Result<Decision> {
        if let Some(reason) = &self.failure {
            return Ok(Decision::Stop("unexpected_branch", reason.clone()));
        }
        if self.paths.is_empty() {
            return Ok(Decision::Stop("unexpected_branch", "Daydream END1 prefix did not match a fresh route".into()));
        }
        if let Some(press) = self.pressed.take() && !press.verified() {
            if self.save_entry.is_some() && frame - self.last_progress < 3 {
                self.pressed = Some(press);
                return Ok(Decision::Wait);
            }
            return Ok(Decision::Stop("input_missed", format!("Daydream END1 input receipt missing in {}", self.scene)));
        }
        if let Some(anchor) = self.save_entry {
            if let Some((panel, click)) = &self.save_click {
                if !click.rejected && click.received.is_none() && frame - click.started < idle {
                    return Ok(Decision::Wait);
                }
                let Some(received) = click.received.filter(|_| !click.rejected) else {
                    return Ok(Decision::Stop("input_missed", "Daydream empty-slot click lacked a unique physical receipt".into()));
                };
                if let Some(written) = self.save_written {
                    if received > written {
                        return Ok(Decision::Stop("checkpoint_mismatch", "Daydream datasave preceded the physical slot receipt".into()));
                    }
                    if player.host_daydream_ordinary_saved_panel(anchor, *panel)?
                        && let Some(sha) = player.host_neko_slot_sha256()?
                    {
                        println!("frame={frame} daydream-ordinary-written scene=c01_01_01 group=13 record=121 slot=0 write_frame={written} sha256={sha}");
                        println!("replay frame={frame} checkpoint=\"Daydream ordinary save written at r121\"");
                        return Ok(Decision::Done);
                    }
                }
                if frame - click.started >= idle {
                    return Ok(Decision::Stop("unrecognized_wait", "Daydream ordinary write or returned Save panel was not confirmed".into()));
                }
                return Ok(Decision::Wait);
            }
            if let Some((point, owner, region)) = player.host_daydream_ordinary_empty_slot(anchor)? {
                eprintln!("frame {frame}: Daydream ordinary Save panel owner={owner} empty_slot=0 region={region}");
                if plan.write_slot_at_r121 {
                    let (click, decision) = ordinary::Click::send(frame, "write", point, owner, region);
                    self.save_click = Some((owner, click));
                    return Ok(decision);
                }
                println!("replay frame={frame} checkpoint=\"Daydream: r121 ordinary Save panel with empty slot 0\"");
                return Ok(Decision::Done);
            }
            if frame - self.last_progress >= idle {
                return Ok(Decision::Stop("unrecognized_wait", "Daydream r121 ordinary Save panel was not confirmed".into()));
            }
            return Ok(Decision::Wait);
        }
        if self.scene == "c99_10" {
            let flags = root_flags(player, &[399, 214, 224, 234, 215, 225, 235, 760, 1000, 1001, 1002, 1003])?;
            if self.movie_end.is_none_or(|end| end > self.load_frame)
                || flags != [3, 1, 1, 1, 0, 0, 1, 1, 0, 0, 0, 1]
            {
                return Ok(Decision::Stop("checkpoint_mismatch", format!("Daydream CASE 3 END1 ending or clear flags: {flags:?}")));
            }
            eprintln!("frame {frame}: Daydream CASE 3 END1 natural_movie_frame={} next=c99_10 #399=3 #214=1 #224=1 #234=1 #215=0 #225=0 #235=1 #760=1 #1000=0 #1001=0 #1002=0 #1003=1",
                self.movie_end.unwrap());
            println!("replay frame={frame} ending_verified=DAYDREAM_CASE03_END1");
            return Ok(Decision::Done);
        }
        if let Some(choice) = choice(&self.scene) && !self.choice_sent {
            let flags_match = match self.scene.as_str() {
                "c99_02_c_01" => root_flags(player, &[290])? == [0],
                "brunch_2" => root_flags(player, &[399, 214, 224, 234, 215, 225, 235, 233])? == [3, 1, 1, 1, 0, 0, 0, 0],
                _ => true,
            };
            if flags_match && let Some((point, region, owner)) =
                player.host_daydream_end1_choice(&self.scene, choice.targets, self.load_frame)?
            {
                let [x, y] = point;
                eprintln!("frame {frame}: Daydream END1 choice scene={} option={} target={} owner={owner} region={region} point={point:?}",
                    self.scene, choice.option, choice.targets.last().unwrap());
                self.pressed = Some(Press::Menu { frame, owner, region, received: false });
                self.choice_sent = true;
                self.last_progress = frame;
                return Ok(Decision::Input(Action::Pointer { x, y, button: Some(0) }));
            }
        }
        if let Some(&record) = plan.waits(&self.scene).get(self.next_body)
            && let Some((anchor, (region, receiver))) = player.host_daydream_end1_body(
                &self.scene, self.next_body as i32, record, self.load_frame,
            )
        {
            let (context, load, owner, epoch, published) = anchor;
            let recreated = self.body_owner_created.is_some_and(|(created, parent)| {
                parent == receiver && self.last_progress < created && created <= published
            });
            if self.last_body.is_some_and(|(before_context, before_load, before_owner, before_epoch, _)| {
                (context, load) != (before_context, before_load) || published <= self.last_progress
                    || owner == before_owner && !recreated && epoch <= before_epoch
            }) {
                return Ok(Decision::Stop("checkpoint_mismatch", format!(
                    "Daydream END1 adjacent wait reused a stale body: previous={:?} current={anchor:?} after={} creation={:?}",
                    self.last_body, self.last_progress, self.body_owner_created,
                )));
            }
            if let Some((created, _)) = self.body_owner_created.filter(|_| recreated
                && self.last_body.is_some_and(|(_, _, before_owner, _, _)| owner == before_owner))
            {
                eprintln!("frame {frame}: Daydream END1 window recreated owner={owner} receiver={receiver} created={created} published={published}");
            }
            if plan.save_entry_at_r121 && (self.scene.as_str(), self.next_body, record) == ("c01_01_01", 13, 121) {
                let Some((point, region)) = player.host_daydream_ordinary_save_button(owner)? else {
                    if frame - self.last_progress >= idle {
                        return Ok(Decision::Stop("unrecognized_wait", "Daydream r121 Save control did not become clickable".into()));
                    }
                    return Ok(Decision::Wait);
                };
                let [x, y] = point;
                eprintln!("frame {frame}: Daydream r121 Save button owner={owner} region={region} point={point:?} context={context} load={load} epoch={epoch}");
                self.save_entry = Some(anchor);
                self.last_progress = frame;
                self.pressed = Some(Press::Menu { frame, region, owner, received: false });
                return Ok(Decision::Input(Action::Pointer { x, y, button: Some(0) }));
            }
            eprintln!("frame {frame}: Daydream END1 body scene={} group={} record={record} context={context} load={load} owner={owner} epoch={epoch} published={published} region={region} receiver={receiver}",
                self.scene, self.next_body);
            self.last_body = Some(anchor);
            self.body_owner_created = None;
            self.next_body += 1;
            self.last_progress = frame;
            self.pressed = Some(Press::Body {
                frame, binding: KeyboardPress { region, receiver, key: 2, group: 0, response: 0 },
                receipt: KeyboardPressReceipt::Awaiting,
            });
            return Ok(Decision::Input(Action::Key(InputKey::Keyboard(13))));
        }
        let idle = if self.scene == "c03_ed" { idle.max(24_000) } else { idle.max(12_000) };
        if frame - self.last_progress >= idle {
            return Ok(Decision::Stop("unrecognized_wait", format!("Daydream END1 scene={} next_group={} index={}",
                self.scene, self.next_body, self.scene_index)));
        }
        if frame % 120 == 0 {
            println!("replay frame={frame} heartbeat");
        }
        Ok(Decision::Wait)
    }
}

fn message_window_creation(detail: &str) -> Option<(u32, u32)> {
    let mut fields = detail.split_ascii_whitespace();
    let owner = fields.next()?.strip_prefix("id=")?.parse().ok()?;
    if fields.next()? != "kind=34" {
        return None;
    }
    let parent = fields.next()?.strip_prefix("parent=")?.parse().ok()?;
    matches!(fields.next()?, "name=\"meswnd\"" | "name=\"cinema\"" | "name=\"messh\"")
        .then_some((owner, parent))
}

fn number_after(detail: &str, marker: &str) -> Option<u32> {
    detail.split_once(marker)?.1.split_ascii_whitespace().next()?.parse().ok()
}

fn root_flags(player: &HeadlessPlayer, slots: &[usize]) -> Result<Vec<u32>> {
    let snapshot = player.shared_numeric_state_snapshot()?;
    let values = if snapshot.active_identity == 0 { Some(&snapshot.active_values) } else { snapshot.inactive_values.get(&0) };
    slots.iter().map(|&slot| {
        values.and_then(|values| values.get(slot)).copied().ok_or_else(|| anyhow::anyhow!("missing root flag #{slot}"))
    }).collect()
}

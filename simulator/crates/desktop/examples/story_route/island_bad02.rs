use super::{Decision, MousePress, Position};
use crate::{Action, capture};
use anyhow::Result;
use cs2_desktop::HeadlessPlayer;
use cs2_runtime::message_object::{ButtonSource, RoutedPhysicalButton};
use std::path::Path;

const BRANCH_WAITS: [i32; 6] = [4, 6, 8, 10, 12, 14];
const ENDING_WAITS: [i32; 38] = [
    14, 16, 18, 20, 22, 24, 26, 28, 30, 42, 44, 46, 58, 60, 62, 64, 66, 78, 80, 92, 94,
    96, 98, 100, 102, 115, 131, 149, 152, 154, 156, 158, 160, 162, 164, 176, 183, 207,
];
const FIRST_CHOICE: [i32; 2] = [640, 206];
const BODY_POINT: [i32; 2] = [557, 324];

pub(super) struct IslandBad02 {
    next: usize,
    context: Option<(u32, u64)>,
    press: Option<Press>,
}

struct Press {
    expected: MousePress,
    owner: u32,
    sent: u64,
    confirmed: bool,
    card_button: Option<(u32, u32)>,
}

impl IslandBad02 {
    pub(super) fn start() -> Self {
        Self { next: 0, context: None, press: None }
    }

    pub(super) fn observe_frame(&mut self, player: &HeadlessPlayer) {
        let Some(press) = &mut self.press else { return; };
        if press.confirmed {
            return;
        }
        let mut matches = player.host_input_route_receipts().iter()
            .filter(|receipt| press.expected.matches_receipt(receipt, press.sent, press.owner));
        if let Some(receipt) = matches.next()
            && matches.next().is_none()
            && receipt.frame == press.sent + 1
        {
            let left = RoutedPhysicalButton { source: ButtonSource::Mouse, code: 1 };
            let foreign_consumer = player.host_input_route_receipts().iter().any(|other| {
                other.frame == receipt.frame && other.response == 0 && other.pressed == 1
                    && other.physical == [left] && other.region != receipt.region
            });
            let card_received = press.card_button.is_none_or(|(region, owner)| {
                let button = MousePress { candidates: Vec::new(), region, key: 0, group: 0, response: 1 };
                player.host_input_route_receipts().iter().filter(|other| {
                    other.frame == receipt.frame && button.matches_receipt(other, press.sent, owner)
                }).count() == 1
            });
            if !foreign_consumer && card_received {
                press.confirmed = true;
                eprintln!("ISLAND BAD_02 verified press: {receipt:?}");
            }
        }
    }

    pub(super) fn poll(&mut self, player: &mut HeadlessPlayer, frame: u64, idle: u64, output: &Path) -> Result<Decision> {
        if let Some(press) = &self.press && !press.confirmed {
            return Ok(Decision::Stop("input_missed", format!(
                "ISLAND BAD_02: no matching receipt for region {} owner {}", press.expected.region, press.owner,
            )));
        }
        let (position, window) = Position::observe(player, false);
        if self.next == 0 {
            let Some((context, load, owner, region)) = player.host_island_bad02_choice()? else {
                return Ok(Decision::Stop("checkpoint_mismatch", "ISLAND BAD_02: beach choice row binding did not match".into()));
            };
            self.context = Some((context, load));
            return self.send(player, frame, FIRST_CHOICE, region, owner, (0, 1), None);
        }
        let branch = self.next <= BRANCH_WAITS.len();
        let (scene, group, waits): (&str, usize, &[i32]) = if branch {
            ("com01_selC_1", self.next - 1, &BRANCH_WAITS)
        } else {
            ("BAD_02", self.next - 1 - BRANCH_WAITS.len(), &ENDING_WAITS)
        };
        let prior = self.press.as_ref().unwrap();
        if group == waits.len() {
            if position.state == "title" && player.host_replay_menu_ready() == Some("title") {
                let clear = flags(player, 722)?;
                eprintln!("ISLAND BAD_02 title frame={frame} shared_slot=722 value={clear:?}");
                if clear.is_some_and(|value| value & 2 == 2) {
                    println!("replay frame={frame} ending_verified=BAD_02");
                    return Ok(Decision::Done);
                }
                return Ok(Decision::Stop("checkpoint_mismatch", "ISLAND BAD_02: clear bit missing at title".into()));
            }
            return Ok(wait_or_stop(frame, prior.sent, idle, &position));
        }
        let record = waits[group];
        let card = !branch && group == ENDING_WAITS.len() - 1;
        let expected_state = if card { "page_probe" } else { "page" };
        if (position.state, position.scene.as_str(), position.group, position.record)
            != (expected_state, scene, group as i32, record)
        {
            return Ok(wait_or_stop(frame, prior.sent, idle, &position));
        }
        let next_wait = waits.get(group + 1)
            .filter(|&&record| branch || record != 207)
            .map(|&record| (group as i32 + 1, record));
        let anchor = if card {
            player.host_island_bad02_card()
        } else {
            window.filter(|(name, _)| *name == "meswnd").and_then(|(_, owner)| {
                player.host_island_meswnd_anchor(scene, (group as i32, record), next_wait, owner)
            })
        };
        let Some((context, loaded, owner)) = anchor else {
            return Ok(wait_or_stop(frame, prior.sent, idle, &position));
        };
        let (previous_context, previous_load) = self.context.unwrap();
        if context != previous_context || (group == 0 && loaded <= previous_load)
            || (group != 0 && loaded != previous_load)
        {
            return Ok(Decision::Stop("checkpoint_mismatch", format!("ISLAND BAD_02 unexpected CST load at {position:?}")));
        }
        if !branch && group == 0 && !flags(player, 723)?.is_some_and(|value| value & 1 == 1) {
            return Ok(Decision::Stop("checkpoint_mismatch", "ISLAND BAD_02: branch bit missing".into()));
        }
        self.context = Some((context, loaded));
        if card {
            capture(player, output, frame)?;
            println!("replay frame={frame} ending_card=BAD_02 scene=BAD_02 group=37 body_record=206 wait_record=207");
        }
        let candidates = player.mouse_press_candidate_regions(BODY_POINT)?;
        let mut regions = candidates.into_iter().filter(|&region| {
            player.input_button_state(region).is_some_and(|button| button.show != 0 && button.receiver == Some(owner))
        });
        let Some(region) = regions.next().filter(|_| regions.next().is_none()) else {
            return Ok(Decision::Stop("checkpoint_mismatch", format!("ISLAND BAD_02 ambiguous click at {position:?} owner={owner}")));
        };
        println!("replay frame={frame} checkpoint={scene}:{group}/{record}");
        if card {
            // The window receives the raw left press, then the story root consumes the advance.
            if !player.mouse_press_candidate_regions(BODY_POINT)?.contains(&108)
                || !player.input_button_state(108).is_some_and(|button| button.show != 0 && button.receiver == Some(8))
            {
                return Ok(Decision::Stop("checkpoint_mismatch", "ISLAND BAD_02: end-card advance receiver changed".into()));
            }
            return self.send(player, frame, BODY_POINT, 108, 8, (2, 0), Some((region, owner)));
        }
        self.send(player, frame, BODY_POINT, region, owner, (2, 0), None)
    }

    fn send(
        &mut self, player: &mut HeadlessPlayer, frame: u64, point: [i32; 2], region: u32, owner: u32,
        (key, response): (u32, u32), card_button: Option<(u32, u32)>,
    ) -> Result<Decision> {
        let candidates = player.mouse_press_candidate_regions(point)?;
        eprintln!("ISLAND BAD_02 click frame={frame} point={point:?} region={region} owner={owner} candidates={candidates:?}");
        let expected = MousePress { candidates, region, key, group: 0, response };
        self.press = Some(Press { expected, owner, sent: frame, confirmed: false, card_button });
        self.next += 1;
        let [x, y] = point;
        Ok(Decision::Input(Action::Pointer { x, y, button: Some(0) }))
    }
}

fn flags(player: &HeadlessPlayer, slot: usize) -> Result<Option<u32>> {
    let snapshot = player.shared_numeric_state_snapshot()?;
    let values = if snapshot.active_identity == 0 {
        Some(&snapshot.active_values)
    } else {
        snapshot.inactive_values.get(&0)
    };
    Ok(values.and_then(|values| values.get(slot)).copied())
}

fn wait_or_stop(frame: u64, sent: u64, idle: u64, position: &Position) -> Decision {
    let elapsed = frame - sent;
    if elapsed >= idle || matches!(position.state, "menu" | "choice") && position.scene != "com01_selC" {
        return Decision::Stop("unrecognized_wait", format!("ISLAND BAD_02 awaiting guarded successor: {position:?}"));
    }
    if elapsed % 120 == 0 {
        println!("replay frame={frame} heartbeat");
    }
    Decision::Wait
}

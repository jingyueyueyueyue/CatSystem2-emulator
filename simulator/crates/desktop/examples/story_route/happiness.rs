//! Observed opening waits and source-pinned successor waits for Happiness! 2.

use super::{Action, HeadlessPlayer, Result};
use cs2_runtime::message_object::{ButtonSource, RoutedPhysicalButton};

pub(super) const OPENING_WAITS: &[(i32, i32)] = HeadlessPlayer::host_happiness_opening_waits();

pub(super) fn scene_successor(previous: (i32, i32), next: (i32, i32)) -> Option<&'static str> {
    match (previous, next) {
        ((46, 657), (0, 63)) => Some("0_01_02"),
        ((51, 702), (0, 102)) => Some("0_01_03"),
        _ => None,
    }
}

pub(super) enum SaveDecision {
    Wait,
    Input(Action),
    Stop(&'static str),
}

pub(super) enum LoadDecision {
    Wait,
    Input(Action),
    Restored,
    Stop(&'static str),
}

type Anchor = (u32, u32, u64, u64);

pub(super) struct Save {
    anchor: Anchor,
    since: u64,
    stage: u8,
    hovered: bool,
    pending: Option<(u64, u32, u32)>,
    received: bool,
    written: bool,
}

impl Save {
    pub(super) fn new(anchor: Anchor, frame: u64) -> Self {
        Self { anchor, since: frame, stage: 0, hovered: false, pending: None, received: false, written: false }
    }

    pub(super) fn poll(&mut self, player: &mut HeadlessPlayer, frame: u64) -> Result<SaveDecision> {
        if let Some((pressed, _, _)) = self.pending {
            if !self.received && frame >= pressed + 3 {
                return Ok(SaveDecision::Stop("input_missed"));
            }
            if self.written && let Some(sha256) = player.host_neko_slot_sha256()? {
                println!("02 ordinary_save frame={frame} scene=0_01_01 group=46 record=657 slot=0 sha256={sha256}");
                return Ok(SaveDecision::Stop("completed"));
            }
            if self.received && self.stage == 0
                && let Some(hit) = player.host_happiness_save_control(self.anchor, 1)?
                && let Some(action) = self.click_ready_control(player, frame, 1, hit)
            {
                return Ok(action);
            }
        } else if let Some(hit) = player.host_happiness_save_control(self.anchor, 0)? {
            if let Some(action) = self.click_ready_control(player, frame, 0, hit) {
                return Ok(action);
            }
        } else if !self.hovered
            && let Some(([x, y], _, owner)) = player.host_happiness_save_control(self.anchor, 2)?
        {
            self.hovered = true;
            println!("02 ordinary_save_hover frame={frame} owner={owner} point=[{x}, {y}]");
            return Ok(SaveDecision::Input(Action::Pointer { x, y, button: None }));
        }
        if frame.saturating_sub(self.since) >= 360 {
            eprintln!("02 ordinary Save stage={} timed out. {}", self.stage, player.host_replay_wait_diagnostics());
            return Ok(SaveDecision::Stop("unrecognized_wait"));
        }
        Ok(SaveDecision::Wait)
    }

    fn click_ready_control(
        &mut self, player: &HeadlessPlayer, frame: u64, stage: u8,
        hit @ ([x, y], region, owner): ([i32; 2], u32, u32),
    ) -> Option<SaveDecision> {
        if player.host_pointer_logical_position() != [x, y] {
            println!("02 ordinary_save_hover_control frame={frame} stage={stage} owner={owner} region={region} point=[{x}, {y}]");
            return Some(SaveDecision::Input(Action::Pointer { x, y, button: None }));
        }
        if !player.host_happiness_save_button_focused(owner, region) {
            return None;
        }
        self.stage = stage;
        self.since = frame;
        Some(self.press(frame, hit))
    }

    fn press(&mut self, frame: u64, ([x, y], region, owner): ([i32; 2], u32, u32)) -> SaveDecision {
        self.pending = Some((frame, region, owner));
        self.received = false;
        println!("02 ordinary_save_guard frame={frame} stage={} owner={owner} region={region} point=[{x}, {y}]", self.stage);
        SaveDecision::Input(Action::Pointer { x, y, button: Some(0) })
    }

    pub(super) fn observe(&mut self, player: &HeadlessPlayer, frame: u64) -> Option<&'static str> {
        let (pressed, region, owner) = self.pending?;
        for receipt in player.host_input_route_receipts().iter().filter(|receipt| {
            receipt.frame == frame && frame > pressed && receipt.key == 0 && receipt.group == 0
                && receipt.raw_mask == ButtonSource::Mouse.mask() && receipt.pressed == 1 && !receipt.repeated
                && receipt.physical == [RoutedPhysicalButton { source: ButtonSource::Mouse, code: 1 }]
        }) {
            if (receipt.region, receipt.receiver) == (region, owner) {
                if receipt.response > 1 || self.received {
                    return Some("input_missed");
                }
                self.received = true;
                println!("02 ordinary_save_received frame={frame} stage={} owner={owner} region={region} response={}", self.stage, receipt.response);
            } else if receipt.response == 0 {
                return Some("input_missed");
            }
        }
        for event in player.events().iter().rev().take_while(|event| event.frame == frame) {
            if event.kind == "fes-datasave" {
                if self.stage != 1 || !self.received || self.written
                    || !event.detail.starts_with("slot=0 container=") || !event.detail.ends_with(" written=true")
                {
                    return Some("checkpoint_mismatch");
                }
                self.written = true;
            }
        }
        None
    }
}

pub(super) struct Load {
    sha256: String,
    stage: u8,
    since: u64,
    pending: Option<(u64, u32, u32)>,
}

impl Load {
    pub(super) fn new(player: &HeadlessPlayer, sha256: String) -> Result<Self> {
        anyhow::ensure!(player.host_neko_slot_sha256()?.as_ref() == Some(&sha256),
            "Happiness ordinary slot SHA-256 mismatch");
        Ok(Self { sha256, stage: 0, since: 0, pending: None })
    }

    pub(super) fn poll(&mut self, player: &mut HeadlessPlayer, frame: u64) -> Result<LoadDecision> {
        if let Some((pressed, _, _)) = self.pending {
            return Ok(if frame >= pressed + 3 { LoadDecision::Stop("input_missed") }
                else { LoadDecision::Wait });
        }
        if self.stage >= 3
            && let Some((_, (owner, context, load), (epoch, _, _))) =
                player.host_replay_legacy_fes_body(0xe0d, 46, 657)
        {
            if player.host_neko_slot_sha256()?.as_ref() != Some(&self.sha256) {
                return Ok(LoadDecision::Stop("checkpoint_mismatch"));
            }
            println!("02 ordinary_load frame={frame} scene=0_01_01 group=46 record=657 \
                owner={owner} context={context} load={load} epoch={epoch} slot=0 sha256={}", self.sha256);
            return Ok(LoadDecision::Restored);
        }
        if frame - self.since >= if self.stage == 0 { 2400 } else { 900 } {
            eprintln!("02 ordinary Load stage={} timed out. {}", self.stage, player.host_replay_wait_diagnostics());
            return Ok(LoadDecision::Stop("unrecognized_wait"));
        }
        // title2 opens START GAME on FOCUS and disables the parent button.
        if self.stage == 0 {
            if player.host_happiness_load_control(1)?.is_some() {
                self.stage = 1;
                self.since = frame;
                println!("02 ordinary_load_submenu frame={frame} opened=true");
            } else if let Some(([x, y], _, owner)) = player.host_happiness_load_control(0)?
                && player.host_pointer_logical_position() != [x, y]
            {
                println!("02 ordinary_load_hover frame={frame} owner={owner} point=[{x}, {y}]");
                return Ok(LoadDecision::Input(Action::Pointer { x, y, button: None }));
            } else {
                return Ok(LoadDecision::Wait);
            }
        }
        let Some(([x, y], region, owner)) = player.host_happiness_load_control(self.stage)?
        else { return Ok(LoadDecision::Wait); };
        if player.host_pointer_logical_position() != [x, y] {
            return Ok(LoadDecision::Input(Action::Pointer { x, y, button: None }));
        }
        if !player.host_happiness_save_button_focused(owner, region) {
            return Ok(LoadDecision::Wait);
        }
        self.pending = Some((frame, region, owner));
        println!("02 ordinary_load_guard frame={frame} stage={} owner={owner} region={region} point=[{x}, {y}]", self.stage);
        Ok(LoadDecision::Input(Action::Pointer { x, y, button: Some(0) }))
    }

    pub(super) fn observe(&mut self, player: &HeadlessPlayer, frame: u64) -> Option<&'static str> {
        let (pressed, region, owner) = self.pending?;
        let mut consumed = player.host_input_route_receipts().iter().filter(|receipt| {
            receipt.frame == frame && frame > pressed && receipt.key == 0 && receipt.group == 0
                && receipt.raw_mask == ButtonSource::Mouse.mask() && receipt.pressed == 1 && !receipt.repeated
                && receipt.physical == [RoutedPhysicalButton { source: ButtonSource::Mouse, code: 1 }]
                && receipt.response == 0
        });
        let receipt = consumed.next()?;
        if (receipt.region, receipt.receiver) != (region, owner) || consumed.next().is_some() {
            return Some("input_missed");
        }
        println!("02 ordinary_load_received frame={frame} stage={} owner={owner} region={region}", self.stage);
        self.stage += 1;
        self.since = frame;
        self.pending = None;
        None
    }
}

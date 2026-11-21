//! Ordinary Save from the verified __tar01 wait at 2/70.
use super::{Action, HeadlessPlayer, Result};
use cs2_runtime::message_object::{ButtonSource, RoutedPhysicalButton};

type Anchor = (u32, u32, u64, u64);

pub(super) enum Decision {
    Wait,
    Input(Action),
    Stop(&'static str),
}

pub(super) struct Save {
    anchor: Anchor,
    stage: u8,
    since: u64,
    hover: u8,
    pending: Option<(u64, u32, u32)>,
    consumed: Option<u64>,
    writes: u32,
}

impl Save {
    pub(super) fn new(anchor: Anchor, frame: u64) -> Self {
        Self { anchor, stage: 0, since: frame, hover: 0, pending: None, consumed: None, writes: 0 }
    }

    pub(super) fn poll(&mut self, player: &mut HeadlessPlayer, frame: u64) -> Result<Decision> {
        if let Some((started, _, _)) = self.pending {
            if self.consumed.is_none() && frame >= started + 3 {
                eprintln!("04 ordinary Save stage={} missing physical mouse receipt", self.stage);
                return Ok(Decision::Stop("input_missed"));
            }
            if self.consumed.is_some() && self.stage == 2 && self.writes == 1
                && let Some(sha256) = player.host_neko_slot_sha256()?
            {
                println!("04 ordinary_save frame={frame} scene=__tar01 group=2 record=70 slot=0 sha256={sha256}");
                return Ok(Decision::Stop("completed"));
            }
            if self.consumed.is_some() && self.stage < 2 {
                if let Some(hit) = player.host_yukikoi_save_control(self.anchor, self.stage + 1)? {
                    self.stage += 1;
                    self.since = frame;
                    self.pending = None;
                    self.consumed = None;
                    return Ok(self.press(frame, hit));
                }
            }
        } else if let Some(hit) = player.host_yukikoi_save_control(self.anchor, self.stage)? {
            return Ok(self.press(frame, hit));
        } else if self.stage == 0 && self.hover < 2
            && let Some(([x, y], region, owner)) = player.host_yukikoi_save_control(self.anchor, 3)?
        {
            let x = if self.hover == 0 { x } else { 165 };
            self.hover += 1;
            println!("04 ordinary_save_hover frame={frame} step={} owner={owner} region={region} point=[{x}, {y}]", self.hover);
            return Ok(Decision::Input(Action::Pointer { x, y, button: None }));
        }

        if frame.saturating_sub(self.since) >= 360 {
            eprintln!("04 ordinary Save stage={} timed out. {}", self.stage, player.host_replay_wait_diagnostics());
            return Ok(Decision::Stop("unrecognized_wait"));
        }
        Ok(Decision::Wait)
    }

    fn press(&mut self, frame: u64, ([x, y], region, owner): ([i32; 2], u32, u32)) -> Decision {
        self.pending = Some((frame, region, owner));
        println!("04 ordinary_save_guard frame={frame} stage={} owner={owner} region={region} point=[{x}, {y}]", self.stage);
        Decision::Input(Action::Pointer { x, y, button: Some(0) })
    }

    pub(super) fn observe(&mut self, player: &HeadlessPlayer, frame: u64) -> Option<&'static str> {
        let Some((started, region, owner)) = self.pending else { return None; };
        let mut receipts = player.host_input_route_receipts().iter().filter(|receipt| {
            receipt.frame == frame && receipt.frame > started && receipt.key == 0 && receipt.group == 0
                && receipt.raw_mask == ButtonSource::Mouse.mask() && receipt.pressed == 1 && !receipt.repeated
                && receipt.physical == [RoutedPhysicalButton { source: ButtonSource::Mouse, code: 1 }]
        });
        let mut matches = receipts.clone().filter(|receipt| (receipt.region, receipt.receiver) == (region, owner));
        let matched = matches.next();
        if matches.next().is_some()
            || receipts.any(|receipt| receipt.response == 0 && (receipt.region, receipt.receiver) != (region, owner))
        {
            return Some("input_missed");
        }
        if let Some(receipt) = matched {
            if receipt.response > 1 || self.consumed.replace(frame).is_some() {
                return Some("input_missed");
            }
            println!("04 ordinary_save_received frame={frame} stage={} region={region} owner={owner} response={}", self.stage, receipt.response);
        }
        for event in player.events().iter().rev().take_while(|event| event.frame == frame) {
            if event.kind == "fes-datasave" {
                self.writes += 1;
                if self.stage != 2 || self.writes != 1 || self.consumed.is_none()
                    || !event.detail.starts_with("slot=0 container=") || !event.detail.ends_with(" written=true")
                {
                    return Some("checkpoint_mismatch");
                }
            }
        }
        None
    }
}

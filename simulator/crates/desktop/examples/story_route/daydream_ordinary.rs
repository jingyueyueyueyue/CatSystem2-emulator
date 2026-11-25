use super::{Action, ButtonSource, DaydreamBodyAnchor, Decision, KeyboardPress, KeyboardPressReceipt, LogicEvent, RoutedPhysicalButton,
    SAVE_GROUP, SAVE_RECORD, SAVE_SCENE, SAVE_SUCCESSOR};
use anyhow::Result;
use cs2_desktop::HeadlessPlayer;
use cs2_runtime::InputKey;

pub(super) struct Click {
    pub(super) started: u64,
    owner: u32,
    region: u32,
    name: &'static str,
    pub(super) received: Option<u64>,
    pub(super) rejected: bool,
}

impl Click {
    pub(super) fn send(frame: u64, name: &'static str, point: [i32; 2], owner: u32, region: u32) -> (Self, Decision) {
        let [x, y] = point;
        println!("frame={frame} daydream-ordinary-control name={name} owner={owner} region={region} x={x} y={y}");
        (Self { started: frame, owner, region, name, received: None, rejected: false },
            Decision::Input(Action::Pointer { x, y, button: Some(0) }))
    }

    pub(super) fn observe(&mut self, player: &HeadlessPlayer) {
        let Some(scan) = player.host_input_route_scan_frame() else { return; };
        if scan > self.started + 1 {
            self.rejected |= self.received.is_none();
        }
        if scan != self.started + 1 {
            return;
        }
        let frame = player.state().clock.frame;
        for item in player.host_input_route_receipts().iter().filter(|item| {
            item.frame == frame && item.pressed != 0 && item.response == 0
        }) {
            let target = (item.region, item.receiver) == (self.region, self.owner);
            let physical = item.key == 0 && item.group == 0
                && item.pressed == 1 && !item.repeated && item.raw_mask == ButtonSource::Mouse.mask()
                && item.physical == [RoutedPhysicalButton { source: ButtonSource::Mouse, code: 1 }];
            if target && physical && self.received.is_none() {
                self.received = Some(item.frame);
                eprintln!("frame {}: Daydream ordinary {} {item:?}", item.frame, self.name);
            } else {
                self.rejected = true;
            }
        }
    }
}

pub(super) fn file_fields(detail: &str, prefix: &str, suffix: &str) -> Option<u32> {
    let handle = detail.strip_prefix(prefix)?.strip_suffix(suffix)?.parse().ok()?;
    (handle != 0).then_some(handle)
}

enum Stage {
    Title,
    Slot,
    Confirm(u32),
    Restored,
    Successor {
        anchor: DaydreamBodyAnchor,
        binding: KeyboardPress,
        started: u64,
        receipt: KeyboardPressReceipt,
    },
}

pub(in super::super) struct Load {
    sha: String,
    stage: Stage,
    click: Option<Click>,
    opened: Option<u64>,
    enter: Option<u64>,
    resume: Option<(u64, DaydreamBodyAnchor)>,
    failure: bool,
    last_progress: u64,
}

impl Load {
    pub(in super::super) fn new(sha: String) -> Self {
        Self { sha, stage: Stage::Title, click: None, opened: None, enter: None, resume: None, failure: false, last_progress: 0 }
    }

    pub(in super::super) fn take_resume(&mut self) -> Option<(u64, DaydreamBodyAnchor)> {
        self.resume.take()
    }

    pub(in super::super) fn events(&mut self, events: &[LogicEvent]) {
        for event in events {
            match event.kind.as_str() {
                "fes-datasave" => self.failure = true,
                "daydream-slot-open" => {
                    let suffix = format!(" sha256={}", self.sha);
                    let valid = matches!(self.stage, Stage::Restored)
                        && self.click.as_ref().is_some_and(|click| event.frame > click.started)
                        && file_fields(&event.detail, "native=247 slot=0 file=\"save0000.dat\" handle=", &suffix).is_some();
                    self.failure |= !valid || self.opened.replace(event.frame).is_some();
                }
                _ => {}
            }
        }
    }

    pub(in super::super) fn receipts(&mut self, player: &HeadlessPlayer) {
        let receipts = player.host_input_route_receipts();
        if let Some(click) = &mut self.click {
            click.observe(player);
        }
        if let Stage::Successor { binding, started, receipt, .. } = &mut self.stage {
            let consumers = receipts.iter().filter(|item| item.frame > *started && item.pressed != 0 && item.response == 0).count();
            if consumers == 0 {
                return;
            }
            let events = player.events();
            let hit = receipt.observe(binding, *started, receipts, events);
            if let Some(item) = hit.filter(|item| consumers == 1 && events.iter().rev()
                .take_while(|event| event.frame >= item.frame).any(|event| {
                    event.frame == item.frame && event.kind == "input-wait" && event.detail == "native 71 address=0xd6d"
                }))
            {
                self.enter = Some(item.frame);
                eprintln!("frame {}: Daydream ordinary enter {item:?}", item.frame);
            } else {
                *receipt = KeyboardPressReceipt::Rejected;
            }
        }
    }

    pub(in super::super) fn poll(&mut self, player: &mut HeadlessPlayer, frame: u64, idle: u64, resume_after_load: bool) -> Result<Decision> {
        if self.failure {
            return Ok(Decision::Stop("checkpoint_mismatch", "Daydream ordinary slot open/write receipt mismatch".into()));
        }
        if let Some(click) = &self.click {
            if click.rejected || (click.received.is_none() && frame - click.started >= idle) {
                return Ok(Decision::Stop("input_missed", format!("Daydream ordinary {} physical receipt mismatch", click.name)));
            }
            if click.received.is_none() {
                return Ok(Decision::Wait);
            }
        }
        if let Stage::Successor { anchor, started, receipt, .. } = self.stage {
            if receipt != KeyboardPressReceipt::Verified {
                return Ok(Decision::Stop("input_missed", "Daydream restored page Enter was not verified".into()));
            }
            let opened = self.opened.unwrap();
            if let Some((next, _)) = player.host_daydream_end1_body(SAVE_SCENE, SAVE_GROUP as i32 + 1, SAVE_SUCCESSOR, opened) {
                if (next.0, next.1, next.2) != (anchor.0, anchor.1, anchor.2)
                    || next.3 <= anchor.3 || next.4 <= started || next.4 < self.enter.unwrap()
                    || player.host_neko_slot_sha256()?.as_deref() != Some(self.sha.as_str())
                {
                    return Ok(Decision::Stop("checkpoint_mismatch", "Daydream restored successor changed CST, window or slot identity".into()));
                }
                println!("frame={frame} daydream-ordinary-successor scene=c01_01_01 group=14 record=123 source=122 context={} load={} owner={} epoch={} published={}",
                    next.0, next.1, next.2, next.3, next.4);
                if resume_after_load {
                    self.resume = Some((opened, anchor));
                    return Ok(Decision::Wait);
                }
                println!("replay frame={frame} checkpoint=\"Daydream ordinary load successor r123\"");
                return Ok(Decision::Done);
            }
        } else if matches!(self.stage, Stage::Restored) {
            if let Some(opened) = self.opened {
                if self.click.as_ref().and_then(|click| click.received).is_none_or(|received| received > opened) {
                    return Ok(Decision::Stop("checkpoint_mismatch", "Daydream CSS open preceded the physical Load receipt".into()));
                }
                if let Some((anchor, (region, receiver))) = player.host_daydream_end1_body(SAVE_SCENE, SAVE_GROUP as i32, SAVE_RECORD, opened) {
                    if anchor.4 < opened || player.host_neko_slot_sha256()?.as_deref() != Some(self.sha.as_str()) {
                        return Ok(Decision::Stop("checkpoint_mismatch", "Daydream restored page did not match the opened slot".into()));
                    }
                    println!("frame={frame} daydream-ordinary-restored scene=c01_01_01 group=13 record=121 source=120 context={} load={} owner={} epoch={} published={} region={region} receiver={receiver}",
                        anchor.0, anchor.1, anchor.2, anchor.3, anchor.4);
                    self.click = None;
                    self.last_progress = frame;
                    self.stage = Stage::Successor { anchor, started: frame,
                        binding: KeyboardPress { region, receiver, key: 2, group: 0, response: 0 },
                        receipt: KeyboardPressReceipt::Awaiting };
                    return Ok(Decision::Input(Action::Key(InputKey::Keyboard(13))));
                }
            }
        } else {
            let (name, panel) = match self.stage {
                Stage::Title => ("title", None),
                Stage::Slot => ("slot", None),
                Stage::Confirm(panel) => ("confirm", Some(panel)),
                _ => unreachable!(),
            };
            if let Some((point, owner, region, confirm)) = player.host_daydream_ordinary_load_control(name, panel)? {
                if name == "title" && player.host_neko_slot_sha256()?.as_deref() != Some(self.sha.as_str()) {
                    return Ok(Decision::Stop("checkpoint_mismatch", "Daydream cold profile has a different ordinary slot".into()));
                }
                self.stage = match self.stage {
                    Stage::Title => Stage::Slot,
                    Stage::Slot if confirm => Stage::Confirm(owner),
                    _ => Stage::Restored,
                };
                let (click, decision) = Click::send(frame, name, point, owner, region);
                self.click = Some(click);
                self.last_progress = frame;
                return Ok(decision);
            }
        }
        if frame - self.last_progress >= idle {
            return Ok(Decision::Stop("unrecognized_wait", format!("Daydream ordinary load stage={} CSS open={:?}",
                match self.stage {
                    Stage::Title => "title", Stage::Slot => "slot", Stage::Confirm(_) => "confirm",
                    Stage::Restored => "restored r121", Stage::Successor { .. } => "successor r123",
                }, self.opened)));
        }
        if frame % 120 == 0 {
            println!("replay frame={frame} heartbeat");
        }
        Ok(Decision::Wait)
    }
}

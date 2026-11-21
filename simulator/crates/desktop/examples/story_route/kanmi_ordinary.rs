use super::{Action, Decision, HeadlessPlayer, KeyboardPress, KeyboardPressReceipt, Result};
use cs2_runtime::InputKey;
use cs2_runtime::message_object::{ButtonSource, RoutedPhysicalButton};

#[derive(Debug)]
struct Press {
    started: u64,
    region: u32,
    owner: u32,
    received: bool,
}

impl Press {
    fn observe(&mut self, player: &HeadlessPlayer, frame: u64) -> Result<Option<u32>, &'static str> {
        let started = self.started;
        let mut response = None;
        for receipt in player.host_input_route_receipts().iter().filter(|receipt| {
            receipt.frame == frame && frame > started && receipt.key == 0 && receipt.group == 0
                && receipt.raw_mask == ButtonSource::Mouse.mask() && receipt.pressed == 1 && !receipt.repeated
                && receipt.physical == [RoutedPhysicalButton { source: ButtonSource::Mouse, code: 1 }]
        }) {
            if (receipt.region, receipt.receiver) == (self.region, self.owner) {
                if receipt.response > 1 || std::mem::replace(&mut self.received, true) {
                    return Err("unexpected mouse receipt");
                }
                response = Some(receipt.response);
            } else if receipt.response == 0 {
                return Err("another target consumed mouse input");
            }
        }
        Ok(response)
    }
}

#[derive(Debug)]
pub(super) struct Save {
    anchor: (u32, u64, u32, u64),
    since: u64,
    slot: bool,
    press: Option<Press>,
    written: bool,
    failure: Option<(&'static str, &'static str)>,
}

impl Save {
    pub(super) fn new(anchor: (u32, u64, u32, u64), frame: u64) -> Self {
        Self { anchor, since: frame, slot: false, press: None, written: false, failure: None }
    }

    pub(super) fn poll(&mut self, player: &mut HeadlessPlayer, frame: u64) -> Result<Decision> {
        if let Some((status, detail)) = self.failure {
            return Ok(Decision::Stop(status, detail.into()));
        }
        let control = if let Some(press) = &self.press {
            if !press.received && frame >= press.started + 3 {
                return Ok(Decision::Stop("input_missed", format!("Kanmi ordinary Save slot={} mouse receipt missing", self.slot)));
            }
            if self.slot && press.received && self.written && let Some(sha256) = player.host_neko_slot_sha256()? {
                println!("replay frame={frame} kanmi_ordinary_save scene=sc02 group=25 record=147 slot=0 sha256={sha256}");
                return Ok(Decision::Done);
            }
            if !self.slot && press.received && let Some(point) = player.host_kanmi_save_point(self.anchor, true)? {
                self.slot = true;
                self.since = frame;
                self.press = None;
                Some(point)
            } else {
                None
            }
        } else {
            player.host_kanmi_save_point(self.anchor, self.slot)?
        };
        if let Some((point, region, owner)) = control {
            let [x, y] = point;
            if player.host_pointer_logical_position() != point {
                return Ok(Decision::Input(Action::Pointer { x, y, button: None }));
            }
            println!("replay frame={frame} kanmi_ordinary_save_control slot={} region={region} owner={owner} point={point:?}", self.slot);
            self.press = Some(Press { started: frame, region, owner, received: false });
            return Ok(Decision::Input(Action::Pointer { x, y, button: Some(0) }));
        }
        if frame.saturating_sub(self.since) >= 360 {
            return Ok(Decision::Stop("unrecognized_wait", format!("Kanmi ordinary Save slot={} did not settle", self.slot)));
        }
        Ok(Decision::Wait)
    }

    pub(super) fn observe(&mut self, player: &HeadlessPlayer) {
        let Some(press) = self.press.as_mut() else { return; };
        let frame = player.host_story_trace().0;
        match press.observe(player, frame) {
            Ok(Some(response)) => println!("replay frame={frame} kanmi_ordinary_save_received slot={} region={} owner={} response={response}",
                self.slot, press.region, press.owner),
            Err(detail) => self.failure = Some(("input_missed", detail)),
            Ok(None) => {}
        }
        for event in player.events().iter().rev().take_while(|event| event.frame == frame) {
            if event.kind != "fes-datasave" { continue; }
            let exact_write = self.slot && press.received && !self.written
                && event.detail.starts_with("slot=0 container=") && event.detail.ends_with(" written=true");
            if exact_write {
                self.written = true;
            } else {
                self.failure = Some(("checkpoint_mismatch", "Kanmi ordinary Save wrong slot or write receipt"));
            }
        }
    }
}

#[derive(Debug)]
pub(super) struct Load {
    since: u64,
    terminal_group: usize,
    through_op: bool,
    slot: bool,
    sha256: Option<String>,
    press: Option<Press>,
    next: Option<NextPage>,
    failure: Option<&'static str>,
}

#[derive(Debug)]
struct NextPage {
    frame: u64,
    group: usize,
    tasks: (u32, u32, u32),
    origin: (u32, u64, u64),
    receipt: KeyboardPressReceipt,
}

impl Load {
    pub(super) fn new(terminal_group: usize, through_op: bool) -> Self {
        Self { since: 0, terminal_group, through_op, slot: false, sha256: None, press: None, next: None, failure: None }
    }

    pub(super) fn poll(&mut self, player: &mut HeadlessPlayer, frame: u64, idle: u64) -> Result<Decision> {
        if let Some(detail) = self.failure {
            return Ok(Decision::Stop("input_missed", format!("Kanmi ordinary Load {detail}")));
        }
        if self.sha256.is_none() {
            let Some(sha256) = player.host_neko_slot_sha256()? else {
                return Ok(Decision::Stop("checkpoint_mismatch", "Kanmi ordinary Load requires isolated slot 0".into()));
            };
            println!("replay frame={frame} kanmi_ordinary_load_open slot=0 sha256={sha256}");
            self.sha256 = Some(sha256);
        }
        if let Some(next) = &self.next {
            if next.receipt != KeyboardPressReceipt::Verified {
                return Ok(Decision::Stop("input_missed", "Kanmi ordinary Load next page Enter did not resume".into()));
            }
            if self.through_op && next.group > self.terminal_group {
                if let Some((branch_frame, branch, movie_frame)) = player.host_kanmi_sc02_branch_after(next.frame) {
                    if player.host_neko_slot_sha256()? != self.sha256 {
                        return Ok(Decision::Stop("checkpoint_mismatch", "Kanmi ordinary Load slot changed during the opening".into()));
                    }
                    println!("replay frame={frame} kanmi_ordinary_op_arrived branch={branch} branch_frame={branch_frame} natural_movie_frame={movie_frame} scene=sc03 group=0 record=7");
                    return Ok(Decision::Done);
                }
                return Ok(if frame.saturating_sub(self.since) < idle { Decision::Wait } else {
                    Decision::Stop("unrecognized_wait", "Kanmi did not confirm a registered branch, the natural OP end, and sc03 r7".into())
                });
            }
            if let Some((owner, region, receiver, context, load, body, epoch, source, prefix)) = player.host_kanmi_exact_body()
                && (context, load, epoch) != next.origin
            {
                let group = next.group;
                let record = HeadlessPlayer::host_kanmi_scene_waits("sc02")[group];
                if !at_page(player, group as i32, record)
                    || HeadlessPlayer::host_kanmi_body_record("sc02", group as i32, record) != Some(body)
                    || (owner, region, receiver) != next.tasks || (context, load) != (next.origin.0, next.origin.1)
                    || epoch <= next.origin.2 || source <= next.frame
                    || group == 101 && prefix != Some(555)
                {
                    return Ok(Decision::Stop("checkpoint_mismatch", format!("Kanmi ordinary Load successor sc02 g{group}/r{record} did not match")));
                }
                println!("replay frame={frame} state=page scene=sc02 group={group} record={record}");
                println!("replay frame={frame} kanmi_ordinary_load_advanced context={context} load={load} body={body} epoch={epoch} source_frame={source}");
                if let Some(prefix) = prefix {
                    println!("replay frame={frame} kanmi_ordinary_load_body_prefix record={prefix}");
                }
                if group == self.terminal_group && !self.through_op {
                    if player.host_neko_slot_sha256()? != self.sha256 {
                        return Ok(Decision::Stop("checkpoint_mismatch", "Kanmi ordinary Load slot changed during replay".into()));
                    }
                    return Ok(Decision::Done);
                }
                self.next = Some(NextPage { frame, group: group + 1, tasks: (owner, region, receiver),
                    origin: (context, load, epoch), receipt: KeyboardPressReceipt::Awaiting });
                self.since = frame;
                return Ok(Decision::Input(Action::Key(InputKey::Keyboard(13))));
            }
        } else if let Some(press) = &self.press {
            if !press.received && frame >= press.started + 3 {
                return Ok(Decision::Stop("input_missed", "Kanmi ordinary Load mouse receipt missing".into()));
            }
            if press.received && self.slot {
                if let Some((owner, region, receiver, context, load, body, epoch, source, _)) = player.host_kanmi_exact_body() {
                    if !at_page(player, 25, 147) || body != 146 || source <= press.started {
                        return Ok(Decision::Stop("checkpoint_mismatch", "Kanmi ordinary Load restored a different page".into()));
                    }
                    println!("replay frame={frame} state=page scene=sc02 group=25 record=147");
                    println!("replay frame={frame} kanmi_ordinary_load_restored context={context} load={load} body={body} epoch={epoch} source_frame={source}");
                    self.next = Some(NextPage { frame, group: 26, tasks: (owner, region, receiver), origin: (context, load, epoch),
                        receipt: KeyboardPressReceipt::Awaiting });
                    self.since = frame;
                    return Ok(Decision::Input(Action::Key(InputKey::Keyboard(13))));
                }
            } else if press.received && player.host_kanmi_load_point(true)?.is_some() {
                self.slot = true;
                self.press = None;
                self.since = frame;
            }
        }
        if self.next.is_none() && self.press.is_none() && let Some(([x, y], region, owner)) = player.host_kanmi_load_point(self.slot)? {
            if player.host_pointer_logical_position() != [x, y] {
                return Ok(Decision::Input(Action::Pointer { x, y, button: None }));
            }
            println!("replay frame={frame} kanmi_ordinary_load_control slot={} region={region} owner={owner}", self.slot);
            self.press = Some(Press { started: frame, region, owner, received: false });
            return Ok(Decision::Input(Action::Pointer { x, y, button: Some(0) }));
        }
        if frame.saturating_sub(self.since) >= idle {
            return Ok(Decision::Stop("unrecognized_wait", "Kanmi ordinary Load did not reach the next registered state".into()));
        }
        Ok(Decision::Wait)
    }

    pub(super) fn observe(&mut self, player: &HeadlessPlayer) {
        let frame = player.host_story_trace().0;
        if let Some(next) = &mut self.next {
            let expected = KeyboardPress { region: next.tasks.1, receiver: next.tasks.2, key: 2, group: 0, response: 0 };
            if let Some(hit) = next.receipt.observe(&expected, next.frame, player.host_input_route_receipts(), player.events()) {
                eprintln!("frame {frame}: Kanmi ordinary Load verified next page Enter: {hit:?}");
            }
        } else if let Some(press) = &mut self.press {
            match press.observe(player, frame) {
                Ok(Some(response)) => println!("replay frame={frame} kanmi_ordinary_load_received slot={} region={} owner={} response={response}",
                    self.slot, press.region, press.owner),
                Err(detail) => self.failure = Some(detail),
                Ok(None) => {}
            }
        }
    }
}

fn at_page(player: &HeadlessPlayer, group: i32, record: i32) -> bool {
    player.host_replay_kanmi_position().is_some_and(|(position, _)| position == ("page", "sc02", group, record))
}

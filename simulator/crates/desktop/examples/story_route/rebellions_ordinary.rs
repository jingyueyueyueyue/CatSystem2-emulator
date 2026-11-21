use super::{Action, Body, Decision, HeadlessPlayer, OPENING, root_wait};
use cs2_runtime::message_object::{ButtonSource, RoutedPhysicalButton};

#[derive(Clone, Copy, Debug)]
enum Control { Save, TitleLoad, Slot }

impl Control {
    fn binding(self) -> (&'static [u8], i32) {
        match self {
            Self::Save => (b"btn", 6),
            Self::TitleLoad => (b"btn1", 1),
            Self::Slot => (b"data_btn", 0),
        }
    }
}

struct Target {
    owner: u32,
    region: u32,
    point: [i32; 2],
    candidates: Vec<u32>,
}

enum Phase {
    Acquire(Control),
    Hover(Control, Target),
    Press(Control, Target),
    After(Control),
    Failed,
}

pub(super) struct Ordinary {
    phase: Phase,
    since: u64,
    retained: Option<Body>,
    panel: Option<u32>,
    expected: Option<String>,
}

impl Ordinary {
    pub(super) fn load() -> Self {
        Self { phase: Phase::Acquire(Control::TitleLoad), since: 0, retained: None,
            panel: None, expected: None }
    }

    pub(super) fn save(page: Body, frame: u64) -> Self {
        Self { phase: Phase::Acquire(Control::Save), since: frame, retained: Some(page),
            panel: None, expected: None }
    }

    pub(super) fn observe(&mut self, player: &HeadlessPlayer) {
        let Phase::Press(control, target) = &self.phase else { return; };
        let frame = player.host_story_trace().0;
        if frame != self.since + 1 {
            return;
        }
        let mut receipts = player.host_input_route_receipts().iter().filter(|r| r.frame == frame && r.pressed != 0);
        let physical = |r: &cs2_runtime::message_object::InputRouteReceipt| r.key == 0 && r.group == 0
            && r.pressed == 1 && !r.repeated && r.raw_mask == ButtonSource::Mouse.mask()
            && r.physical == [RoutedPhysicalButton { source: ButtonSource::Mouse, code: 1 }];
        let first = receipts.next();
        let hit = if first.is_some_and(|r| physical(r) && (r.region, r.receiver, r.response) == (3, 0, 1)) {
            receipts.next()
        } else {
            first
        };
        if let Some(hit) = hit.filter(|r| physical(r) && (r.region, r.receiver) == (target.region, target.owner)
            && (r.response == 0 || matches!(control, Control::Slot) && r.response == 1))
            && receipts.all(|r| r.response == 1 && r.receiver != target.owner)
            && root_wait(player, frame)
        {
            println!("frame={frame} rebellions-ordinary-click control={control:?} owner={} region={} response={}",
                target.owner, target.region, hit.response);
            self.phase = Phase::After(*control);
        } else {
            self.phase = Phase::Failed;
        }
    }

    fn panel(&self, player: &HeadlessPlayer, occupied: bool) -> Option<u32> {
        let retained = self.retained.map(|page| (page.scene, page.position, page.tasks,
            (page.context, page.load, page.epoch, page.source_frame, 0)));
        player.host_rebellions_ordinary_panel(retained, occupied)
    }

    fn owner(&self, player: &mut HeadlessPlayer, control: Control) -> Option<u32> {
        match control {
            Control::Save => {
                let page = self.retained?;
                player.host_rebellions_save_retained((page.scene, page.position, page.tasks,
                    (page.context, page.load, page.epoch, page.source_frame, 0))).then_some(page.tasks.0)
            }
            Control::TitleLoad => player.host_rebellions_entry_owner("title"),
            Control::Slot => self.panel(player, self.retained.is_none())
                .filter(|owner| self.panel == Some(*owner)),
        }
    }

    fn slot_unchanged(&self, player: &HeadlessPlayer) -> anyhow::Result<bool> {
        if self.retained.is_some() {
            Ok(player.host_neko_slot_zero_absent())
        } else {
            Ok(player.host_neko_slot_sha256()? == self.expected && self.expected.is_some())
        }
    }

    pub(super) fn restored(&self, player: &HeadlessPlayer) -> anyhow::Result<Option<Body>> {
        if !matches!(self.phase, Phase::After(Control::Slot)) || self.retained.is_some() {
            return Ok(None);
        }
        let Some(digest) = &self.expected else { return Ok(None); };
        let Some(page) = Body::observe(player) else { return Ok(None); };
        let mut opens = player.events().iter().filter(|e| e.frame > self.since
            && e.kind == "rebellions-slot-open");
        let Some(open) = opens.next() else { return Ok(None); };
        anyhow::ensure!(opens.next().is_none() && open.detail.ends_with(&format!(" sha256={digest}"))
            && self.slot_unchanged(player)? && page.scene == OPENING && page.position == (12, 61)
            && page.source_frame >= open.frame, "Rebellions restored slot or body differs");
        println!("frame={} rebellions-ordinary-restored group=12 record=61 open_frame={} sha256={digest}",
            player.host_story_trace().0, open.frame);
        Ok(Some(page))
    }


    pub(super) fn poll(&mut self, player: &mut HeadlessPlayer, frame: u64, idle: u64) -> anyhow::Result<Decision> {
        match &self.phase {
            Phase::Acquire(control) => {
                if matches!(control, Control::TitleLoad) && self.expected.is_none() {
                    self.expected = player.host_neko_slot_sha256()?;
                    anyhow::ensure!(self.expected.is_some(), "Rebellions cold reader needs the staged ordinary slot 0");
                }
                let (name, bank) = control.binding();
                if let Some(owner) = self.owner(player, *control)
                    && let Some((point, region)) = player.host_rebellions_ordinary_control(owner, name, bank, None)?
                {
                    anyhow::ensure!(self.slot_unchanged(player)?, "Rebellions ordinary slot changed or is occupied");
                    let candidates = player.mouse_press_candidate_regions(point)?;
                    let [x, y] = point;
                    self.phase = Phase::Hover(*control, Target { owner, region, point, candidates });
                    self.since = frame;
                    return Ok(Decision::Input(Action::Pointer { x, y, button: None }));
                }
            }
            Phase::Hover(control, target) => {
                let (name, bank) = control.binding();
                let owner = self.owner(player, *control);
                let binding = player.host_rebellions_ordinary_control(target.owner, name, bank, Some(target.point))?;
                let point = player.host_pointer_logical_position();
                let candidates = player.mouse_press_candidate_regions(target.point)?;
                let slot = self.slot_unchanged(player)?;
                if frame == self.since + 1 {
                    eprintln!("frame {frame}: Rebellions ordinary hover control={control:?} owner={owner:?} binding={binding:?} point={point:?} candidates={candidates:?} slot_unchanged={slot}");
                }
                if !slot || owner.is_some_and(|owner| owner != target.owner)
                    || binding.is_some_and(|binding| binding != (target.point, target.region))
                    || point != target.point
                {
                    return Ok(Decision::Stop("checkpoint_mismatch", format!(
                        "Rebellions ordinary hover changed: control={control:?} owner={owner:?} binding={binding:?} point={point:?} slot_unchanged={slot}")));
                }
                if owner.is_none() || binding.is_none() || candidates != target.candidates {
                    return Ok(if frame - self.since < idle { Decision::Wait } else {
                        Decision::Stop("unrecognized_wait", format!(
                            "Rebellions ordinary hover did not settle: control={control:?} owner={owner:?} binding={binding:?} candidates={candidates:?}"))
                    });
                }
                let [x, y] = target.point;
                let Phase::Hover(control, target) = std::mem::replace(&mut self.phase, Phase::Failed) else { unreachable!() };
                self.phase = Phase::Press(control, target);
                self.since = frame;
                return Ok(Decision::Input(Action::Pointer { x, y, button: Some(0) }));
            }
            Phase::After(Control::Save | Control::TitleLoad) => {
                if let Some(owner) = self.panel(player, self.retained.is_none()) {
                    anyhow::ensure!(self.slot_unchanged(player)?, "Rebellions ordinary panel changed slot 0");
                    self.panel = Some(owner);
                    if let Some((point, region)) = player.host_rebellions_ordinary_control(owner, b"data_btn", 0, None)? {
                        println!("frame={frame} rebellions-ordinary-panel owner={owner} slot=0 point={point:?} region={region}");
                        self.phase = Phase::Acquire(Control::Slot);
                        self.since = frame;
                    }
                }
            }
            Phase::After(Control::Slot) if self.retained.is_some() => {
                let mut writes = player.events().iter().filter(|e| e.frame > self.since && e.kind == "fes-datasave");
                if let Some(write) = writes.next() {
                    anyhow::ensure!(writes.next().is_none() && write.detail.starts_with("slot=0 container=")
                        && write.detail.ends_with(" written=true"), "Rebellions ordinary slot write differs");
                    if self.panel(player, true) == self.panel
                        && self.panel.is_some() && let Some(digest) = player.host_neko_slot_sha256()?
                    {
                        println!("frame={frame} rebellions-ordinary-written group=12 record=61 slot=0 write_frame={} sha256={digest}", write.frame);
                        return Ok(Decision::Done);
                    }
                }
            }
            Phase::After(Control::Slot) => {}
            Phase::Press(..) | Phase::Failed => {
                return Ok(Decision::Stop("input_missed", "Rebellions ordinary click did not reach its live control".into()));
            }
        }
        Ok(if frame - self.since < idle { Decision::Wait } else {
            Decision::Stop("unrecognized_wait", format!("Rebellions ordinary did not settle: {}", player.host_replay_wait_diagnostics()))
        })
    }
}

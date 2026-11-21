use super::{Action, Decision, KeyboardPress, KeyboardPressReceipt, NovelWindowLifetime};
use cs2_desktop::{HatsumiraPage, HeadlessPlayer, LogicEvent};
use cs2_runtime::InputKey;
use cs2_runtime::message_object::{ButtonSource, RoutedPhysicalButton};
use serde::Deserialize;

const START: [i32; 2] = [640, 443];
const TITLE_REGIONS: [u32; 4] = [3, 15, 37, 7];

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Mode {
    ObserveR69,
    EnterOnce,
    ObserveR92,
    ObserveR100,
    ObserveR195,
    ObserveR206,
    ObserveR222,
    ObserveR261,
    ObserveR619,
    ObservePostOpR42,
    ObservePostOpR90,
    OpeningSegment,
}

impl Mode {
    fn post_op(self) -> bool {
        matches!(self, Self::ObservePostOpR42 | Self::ObservePostOpR90)
    }
}

enum Phase {
    Title,
    Hover { owners: (u32, u32) },
    Click { owners: (u32, u32), started: u64 },
    FirstPage,
    Enter {
        group: i32,
        before: HatsumiraPage,
        started: u64,
        receipt: KeyboardPressReceipt,
        lifetimes: [NovelWindowLifetime; 3],
    },
    Failed(&'static str),
}

#[derive(Clone, Copy, Default)]
enum OpeningMovie {
    #[default]
    Awaiting,
    Loaded(u32),
    Playing(u32, u64),
    Released(u32, u64, u64),
    Invalid,
}

impl OpeningMovie {
    fn observe(&mut self, event: &LogicEvent) {
        if event.kind != "native-call" {
            return;
        }
        let detail = &event.detail;
        if let Some((_, tail)) = detail.split_once(" movie_load handle=")
            && let Some((handle, resource)) = tail.split_once(" resource=")
        {
            *self = match handle.parse() {
                Ok(handle) if matches!(self, Self::Awaiting) && handle != 0
                    && resource.eq_ignore_ascii_case(r#""movie.int/hatsuop.mpg""#) => Self::Loaded(handle),
                _ => Self::Invalid,
            };
        } else if let Some((_, handle)) = detail.split_once(" movie_hatsumira_op_started handle=") {
            *self = match (&*self, handle.parse::<u32>()) {
                (Self::Loaded(expected), Ok(handle)) if *expected == handle => Self::Playing(handle, event.frame),
                _ => Self::Invalid,
            };
        } else if let Some((_, handle)) = detail.split_once(" movie_hatsumira_op_released handle=") {
            *self = match (&*self, handle.parse::<u32>()) {
                (Self::Playing(expected, start), Ok(handle)) if *expected == handle && *start < event.frame =>
                    Self::Released(handle, *start, event.frame),
                _ => Self::Invalid,
            };
        }
    }
}

pub(super) struct HatsumiraRoute {
    mode: Mode,
    phase: Phase,
    since: u64,
    waiting_for: &'static str,
    movie: OpeningMovie,
    post_op: bool,
}

impl HatsumiraRoute {
    pub(super) fn new(mode: Mode) -> Self {
        Self { mode, phase: Phase::Title, since: 0, waiting_for: "default title",
            movie: OpeningMovie::Awaiting, post_op: false }
    }

    pub(super) fn observe_lifecycle_events(&mut self, events: &[LogicEvent], batch_start: usize) {
        if let Phase::Enter { group, before, started, lifetimes, .. } = &mut self.phase {
            if !self.post_op && *group == 61 && self.mode.post_op() {
                for event in events.get(batch_start..).into_iter().flatten().filter(|event| event.frame > *started) {
                    self.movie.observe(event);
                }
            }
            for (lifetime, id) in lifetimes.iter_mut().zip([before.owner, before.region, before.receiver]) {
                lifetime.observe(id, events, batch_start);
            }
        }
    }

    pub(super) fn observe_frame(&mut self, player: &HeadlessPlayer) {
        let frame = player.host_story_trace().0;
        match &mut self.phase {
            Phase::Click { owners, started } if frame == *started + 1 => {
                let (flow, title) = *owners;
                if title_resume(player, frame, flow, title) {
                    eprintln!("frame {frame}: Hatsumira title propagated flow={flow} title={title} and resumed root");
                    self.phase = Phase::FirstPage;
                    self.since = frame;
                } else {
                    self.phase = Phase::Failed("title propagation or root resume differed");
                }
            }
            Phase::Enter { before, started, receipt, .. } => {
                let expected = KeyboardPress {
                    region: before.region, receiver: before.receiver, key: 2, group: 0, response: 0,
                };
                let receipts = player.host_input_route_receipts();
                let verified = receipt.observe(&expected, *started, receipts, player.events());
                let consumers = receipts.iter().filter(|r| r.frame == frame && r.pressed != 0 && r.response == 0).count();
                if frame == *started + 1 {
                    if verified.is_none() || consumers != 1 || !root_wait(player, frame) {
                        self.phase = Phase::Failed("Enter did not uniquely consume and resume the waiting root");
                    } else {
                        eprintln!("frame {frame}: Hatsumira Enter uniquely consumed and resumed root");
                    }
                } else if consumers != 0 || *receipt == KeyboardPressReceipt::Rejected {
                    self.phase = Phase::Failed("extra input consumption after Enter");
                }
            }
            _ => {}
        }
    }

    pub(super) fn poll(&mut self, player: &mut HeadlessPlayer, frame: u64, idle: u64) -> anyhow::Result<Decision> {
        let [x, y] = START;
        match &self.phase {
            Phase::Title => {
                if let Some(owners) = player.host_hatsumira_12_title() {
                    self.phase = Phase::Hover { owners };
                    self.since = frame;
                    self.waiting_for = "live title after hover";
                    return Ok(Decision::Input(Action::Pointer { x, y, button: None }));
                }
            }
            Phase::Hover { owners } => {
                let Some(live) = player.host_hatsumira_12_title() else { return Ok(self.wait(frame, idle)); };
                let hit = player.masked_input_region_at(START)?;
                let candidates = player.mouse_press_candidate_regions(START)?;
                eprintln!("frame {frame}: Hatsumira title hover owners={live:?} hit={hit:?} candidates={candidates:?}");
                if live != *owners || player.host_pointer_logical_position() != START
                    || !hit.is_some_and(|words| words[0] == 15 && words[8] != 0)
                    || candidates.as_slice() != TITLE_REGIONS
                {
                    return Ok(Decision::Stop("checkpoint_mismatch", "Hatsumira live New Game target changed".into()));
                }
                self.phase = Phase::Click { owners: live, started: frame };
                return Ok(Decision::Input(Action::Pointer { x, y, button: Some(0) }));
            }
            Phase::Click { .. } => return Ok(Decision::Stop("input_missed", "Hatsumira title receipt absent".into())),
            Phase::FirstPage => {
                let page = match player.host_hatsumira_12_page(0, 69) {
                    Ok(page) => page,
                    Err(reason) => {
                        self.waiting_for = reason;
                        return Ok(self.wait(frame, idle));
                    }
                };
                log_page(frame, 0, 69, page);
                if matches!(self.mode, Mode::ObserveR69) {
                    return Ok(Decision::Done);
                }
                return Ok(self.press(player, frame, 0, page));
            }
            Phase::Enter { group, before, started, receipt, lifetimes } => {
                // poll runs only after the outer loop has confirmed device-scan release.
                if *receipt != KeyboardPressReceipt::Verified {
                    return Ok(Decision::Stop("input_missed", "Hatsumira Enter receipt absent".into()));
                }
                if !self.post_op && *group == 61 && self.mode.post_op() {
                    let OpeningMovie::Released(handle, played, released) = self.movie else {
                        if matches!(self.movie, OpeningMovie::Invalid) {
                            return Ok(Decision::Stop("checkpoint_mismatch", "Hatsumira OP movie lifecycle changed".into()));
                        }
                        self.waiting_for = "natural hatsuop release";
                        return Ok(self.wait(frame, idle));
                    };
                    let after = match player.host_hatsumira_12_post_op(0) {
                        Ok(page) => page,
                        Err(reason) => {
                            self.waiting_for = reason;
                            return Ok(self.wait(frame, idle));
                        }
                    };
                    if let [
                        NovelWindowLifetime::Recreated { frame: owner_created },
                        NovelWindowLifetime::Recreated { frame: region_created },
                        NovelWindowLifetime::Continuous,
                    ] = *lifetimes
                        && *started < played && played < released && released <= after.loaded
                        && after.loaded <= region_created && region_created < owner_created
                        && owner_created <= after.source_frame && after.load > before.load
                        && (after.context, after.owner, after.region, after.receiver)
                            == (before.context, before.owner, before.region, before.receiver)
                    {
                        println!("replay frame={frame} state=page scene=OPMOVIE_END group=0 record=42");
                        eprintln!("frame {frame}: Hatsumira post-op handle={handle} played={played} released={released} region_created={region_created} owner_created={owner_created} {after:?}");
                        if matches!(self.mode, Mode::ObservePostOpR42) {
                            return Ok(Decision::Done);
                        }
                        self.post_op = true;
                        return Ok(self.press(player, frame, 0, after));
                    }
                    return Ok(Decision::Stop("checkpoint_mismatch", format!(
                        "Hatsumira post-OP transition before={before:?} after={after:?} lifetimes={lifetimes:?} played={played} released={released}",
                    )));
                }
                if *lifetimes != [NovelWindowLifetime::Continuous; 3] {
                    return Ok(Decision::Stop("checkpoint_mismatch", format!("Hatsumira task lifetime changed: {lifetimes:?}")));
                }
                let group = *group + 1;
                let expected = if self.post_op { HeadlessPlayer::host_hatsumira_12_post_op_records(group) }
                    else { HeadlessPlayer::host_hatsumira_12_op001_records(group) };
                let Some((record, _)) = expected else {
                    return Ok(Decision::Stop("checkpoint_mismatch", "Hatsumira route exceeded the pinned waits".into()));
                };
                let result = if self.post_op { player.host_hatsumira_12_post_op(group) }
                    else { player.host_hatsumira_12_page(group, record) };
                let after = match result {
                    Ok(page) => page,
                    Err(reason) => {
                        self.waiting_for = reason;
                        return Ok(self.wait(frame, idle));
                    }
                };
                if (after.owner, after.region, after.receiver) != (before.owner, before.region, before.receiver)
                    || (after.context, after.load, after.loaded) != (before.context, before.load, before.loaded)
                    || after.epoch <= before.epoch || after.source_frame <= *started || after.marked <= *started
                {
                    return Ok(Decision::Stop("checkpoint_mismatch", "Hatsumira successor identity or generation changed".into()));
                }
                eprintln!("frame {frame}: Hatsumira Enter release observed");
                if self.post_op {
                    println!("replay frame={frame} state=page scene=OPMOVIE_END group={group} record={record}");
                    eprintln!("frame {frame}: Hatsumira post-op page group={group} record={record} {after:?}");
                    if record == 90 {
                        return Ok(Decision::Done);
                    }
                    return Ok(self.press(player, frame, group, after));
                }
                log_page(frame, group, record, after);
                let terminal = match self.mode {
                    Mode::ObserveR69 => 69,
                    Mode::EnterOnce => 72,
                    Mode::ObserveR92 => 92,
                    Mode::ObserveR100 => 100,
                    Mode::OpeningSegment => 179,
                    Mode::ObserveR195 => 195,
                    Mode::ObserveR206 => 206,
                    Mode::ObserveR222 => 222,
                    Mode::ObserveR261 => 261,
                    Mode::ObserveR619 => 619,
                    Mode::ObservePostOpR42 | Mode::ObservePostOpR90 => i32::MAX,
                };
                if record == terminal {
                    return Ok(Decision::Done);
                }
                return Ok(self.press(player, frame, group, after));
            }
            Phase::Failed(reason) => return Ok(Decision::Stop("input_missed", format!("Hatsumira {reason}"))),
        }
        Ok(self.wait(frame, idle))
    }

    fn press(&mut self, player: &HeadlessPlayer, frame: u64, group: i32, before: HatsumiraPage) -> Decision {
        let armed = NovelWindowLifetime::Armed { first_event: player.events().len() };
        self.phase = Phase::Enter {
            group, before, started: frame, receipt: KeyboardPressReceipt::Awaiting, lifetimes: [armed; 3],
        };
        self.since = frame;
        Decision::Input(Action::Key(InputKey::Keyboard(13)))
    }

    fn wait(&self, frame: u64, idle: u64) -> Decision {
        if frame - self.since < idle {
            return Decision::Wait;
        }
        let target = match self.phase {
            Phase::FirstPage => "g0/r69".into(),
            Phase::Enter { group, .. } if self.post_op => HeadlessPlayer::host_hatsumira_12_post_op_records(group + 1)
                .map_or_else(|| "end of guarded post-OP waits".into(),
                    |(record, _)| format!("OPMOVIE_END g{}/r{record}", group + 1)),
            Phase::Enter { group: 61, .. } => "OPMOVIE_END g0/r42".into(),
            Phase::Enter { group, .. } => HeadlessPlayer::host_hatsumira_12_op001_records(group + 1)
                .map_or_else(|| "end of op001".into(), |(record, _)| format!("g{}/r{record}", group + 1)),
            _ => "title".into(),
        };
        Decision::Stop("unrecognized_wait", format!("Hatsumira {target} did not settle: {}", self.waiting_for))
    }
}

fn root_wait(player: &HeadlessPlayer, frame: u64) -> bool {
    player.events().iter().rev().take_while(|event| event.frame == frame)
        .any(|event| event.kind == "input-wait" && event.detail == "native 71 address=0xc29")
}

fn title_resume(player: &HeadlessPlayer, frame: u64, flow: u32, title: u32) -> bool {
    let physical = RoutedPhysicalButton { source: ButtonSource::Mouse, code: 1 };
    let expected = [0, 2].into_iter().flat_map(|key| {
        TITLE_REGIONS.into_iter().zip([0, title, title, flow]).map(move |(region, receiver)| (key, region, receiver))
    });
    let receipts = player.host_input_route_receipts().iter().filter(|r| r.frame == frame && r.pressed != 0);
    let mut resumes = player.events().iter().rev().take_while(|event| event.frame == frame)
        .filter(|event| event.kind == "message-resume");
    root_wait(player, frame) && receipts.clone().count() == 8
        && receipts.zip(expected).all(|(r, (key, region, receiver))| {
            (r.key, r.region, r.receiver) == (key, region, receiver) && r.group == 0 && r.response == 1
                && r.pressed == 1 && !r.repeated && r.raw_mask == ButtonSource::Mouse.mask()
                && r.physical == [physical]
        })
        && resumes.next().is_some_and(|event| event.detail == "message=[2, 640, 443, 0]")
        && resumes.next().is_none()
}

fn log_page(frame: u64, group: i32, record: i32, page: HatsumiraPage) {
    println!("replay frame={frame} state=page scene=op001 group={group} record={record}");
    eprintln!("frame {frame}: Hatsumira page group={group} record={record} {page:?}");
}

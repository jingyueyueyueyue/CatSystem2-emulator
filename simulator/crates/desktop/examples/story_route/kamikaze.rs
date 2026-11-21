use super::{
    Action, Decision, KeyboardPress, KeyboardPressReceipt, LogicEvent, MousePress,
    NovelWindowLifetime,
};
use anyhow::Result;
use cs2_desktop::HeadlessPlayer;
use cs2_runtime::InputKey;
use serde::Deserialize;

const TITLE_POINT: [i32; 2] = [176, 256];
const TITLE_REGION: u32 = 20;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Contract {
    pub(super) keyboard_press: Option<KeyboardPress>,
    last_group: Option<i32>,
}

impl Contract {
    pub(super) fn valid(&self) -> bool {
        match &self.keyboard_press {
            None => self.last_group.is_none(),
            Some(press) => self.last_group.is_none_or(|group| {
                group > 0 && HeadlessPlayer::kamikaze_replay_page(group).is_some()
            })
                && press.region != 0 && press.receiver != 0
                && (press.key, press.group, press.response) == (2, 0, 0),
        }
    }
}

#[derive(Default)]
pub(crate) struct Opening {
    handle: Option<u32>,
    played: bool,
    released_at: Option<u64>,
    stopped: bool,
}

impl Opening {
    pub(crate) fn released_between(&self, pressed: u64, source: u64) -> bool {
        !self.stopped && self.released_at.is_some_and(|released| pressed < released && released <= source)
    }

    pub(crate) fn observe(&mut self, event: &LogicEvent) {
        if event.kind != "native-call" {
            return;
        }
        let detail = &event.detail;
        if let Some((_, tail)) = detail.split_once(" movie_load handle=")
            && let Some((handle, "\"movie.int/OPmovie.mpg\"")) = tail.split_once(" resource=")
            && let Ok(handle) = handle.parse::<u32>()
        {
            if self.handle.replace(handle).is_some() {
                self.stopped = true;
            }
        }
        let Some(handle) = self.handle else { return; };
        let native_handle = format!("{:08x}", handle.swap_bytes());
        if detail.starts_with("index=266 ") {
            if self.played || *detail != format!("index=266 arguments=8 bytes [{native_handle}01000000]") {
                self.stopped = true;
            }
            self.played = true;
        }
        if detail.starts_with("index=267 ") {
            self.stopped = true;
        }
        let release = format!("index=268 arguments=4 bytes [{native_handle}]");
        if let Some(tail) = detail.strip_prefix(release.as_str()) {
            if self.played && !self.stopped && self.released_at.is_none()
                && tail == format!(" movie_naturally_released handle={handle}")
            {
                eprintln!("frame {}: 07 OP naturally finished and released handle={handle}", event.frame);
                self.released_at = Some(event.frame);
            } else {
                self.stopped = true;
            }
        }
    }
}

struct BodyPress {
    frame: u64,
    from_group: i32,
    owner: u32,
    context: u32,
    load: u64,
    epoch: u64,
    lifetime: NovelWindowLifetime,
    receipt: KeyboardPressReceipt,
    root_resumes: usize,
    command_steps: usize,
}

impl BodyPress {
    fn arm(frame: u64, from_group: i32, (owner, context, load): (u32, u32, u64),
        epoch: u64, first_event: usize,
    ) -> Self {
        Self {
            frame, from_group, owner, context, load, epoch,
            lifetime: NovelWindowLifetime::Armed { first_event },
            receipt: KeyboardPressReceipt::Awaiting,
            root_resumes: 0,
            command_steps: 0,
        }
    }

    fn transition_commands(&self) -> &'static [&'static str] {
        match self.from_group {
            0 => &[
                "debug=\"[command] pl 1 fade 40 0 255\"",
                "debug=\"[command] pl 9 fade 40 255 0\"",
                "debug=\"[command] wait\"",
            ],
            1 => &[
                "debug=\"[command] bg 5 bg022a 0 0 0 1\"",
                "debug=\"[command] bg 5 scale 100% 100%\"",
                "debug=\"[command] rdraw 30\"",
                "debug=\"[command] wait\"",
            ],
            2 => &[
                "debug=\"[command] rdraw 30\"",
                "debug=\"[command] wait\"",
            ],
            _ => &[],
        }
    }
}

#[derive(Default)]
enum Phase {
    #[default]
    Title,
    TitleHover,
    TitlePress { frame: u64, owner: u32 },
    Opening,
    Body(BodyPress),
    Failed(String),
}

#[derive(Default)]
pub(super) struct Replay {
    phase: Phase,
    opening: Opening,
}

impl Replay {
    pub(super) fn observe_events(&mut self, events: &[LogicEvent], start: usize) {
        if let Phase::Body(body) = &mut self.phase {
            body.lifetime.observe(body.owner, events, start);
            let batch = events.get(start..).unwrap_or_default();
            let expected = body.transition_commands();
            for event in batch.iter().filter(|event| event.frame > body.frame) {
                if event.kind == "message-resume" {
                    body.root_resumes += 1;
                }
                if event.kind == "native-call"
                    && expected.get(body.command_steps).is_some_and(|command| event.detail.ends_with(*command))
                {
                    body.command_steps += 1;
                }
            }
        }
        if matches!(self.phase, Phase::Opening) {
            for event in events.get(start..).unwrap_or_default() {
                self.opening.observe(event);
            }
        }
    }

    pub(super) fn observe_frame(&mut self, player: &HeadlessPlayer, contract: &Contract) {
        match &mut self.phase {
            Phase::TitlePress { frame, owner } => {
                let tick = *frame + 1;
                let press = MousePress { candidates: vec![], region: TITLE_REGION,
                    key: 0, group: 0, response: 0 };
                let mut consumers = player.host_input_route_receipts().iter()
                    .filter(|route| route.frame == tick && route.pressed != 0 && route.response == 0);
                let mut resumes = player.events().iter().rev().take_while(|event| event.frame >= tick)
                    .filter(|event| event.frame == tick && event.kind == "message-resume");
                self.phase = if consumers.next().is_some_and(|receipt| press.matches_receipt(receipt, *frame, *owner))
                    && consumers.next().is_none()
                    && resumes.next().is_some_and(|event| event.detail == "message=[2, 176, 256, 0]")
                    && resumes.next().is_none()
                {
                    eprintln!("frame {tick}: verified 07 New Game region={TITLE_REGION} receiver={owner}");
                    Phase::Opening
                } else {
                    Phase::Failed("07 New Game consuming receipt or root resume missing".into())
                };
            }
            Phase::Body(body) => {
                if let Some(receipt) = body.receipt.observe(contract.keyboard_press.as_ref().unwrap(),
                    body.frame, player.host_input_route_receipts(), player.events())
                {
                    eprintln!("replay frame={}: verified 07 Enter and VM resume: {receipt:?}", receipt.frame);
                }
            }
            _ => {}
        }
    }

    pub(super) fn poll(&mut self, player: &mut HeadlessPlayer, frame: u64, contract: &Contract) -> Result<Decision> {
        if !player.host_kamikaze_replay_identity() {
            return Ok(Decision::Stop("checkpoint_mismatch", "07 runtime identity mismatch".into()));
        }
        if frame % 120 == 0 {
            println!("replay frame={frame} heartbeat");
        }
        match &self.phase {
            Phase::Title | Phase::TitleHover => {
                let owner = player.host_kamikaze_title_owner();
                // This query resolves both registered bitmaps and rectangular hit regions.
                if let Some(owner) = owner
                    && let Some(mask) = player.masked_input_region_at(TITLE_POINT)?
                    && mask[0] == TITLE_REGION && mask[8] != 0 && mask[9] != 0
                    && player.mouse_press_candidate_regions(TITLE_POINT)? == [3, 20, 31, 10, 7]
                    && player.input_button_state(TITLE_REGION).is_some_and(|button| {
                        button.receiver == Some(owner) && button.show != 0
                    })
                {
                    if mask[7] == 0 {
                        if matches!(self.phase, Phase::Title) {
                            self.phase = Phase::TitleHover;
                            return Ok(Decision::Input(Action::Pointer { x: TITLE_POINT[0],
                                y: TITLE_POINT[1], button: None }));
                        }
                        return Ok(Decision::Wait);
                    }
                    eprintln!("frame {frame}: guarded 07 New Game region={TITLE_REGION} receiver={owner}");
                    self.phase = Phase::TitlePress { frame, owner };
                    return Ok(Decision::Input(Action::Pointer { x: TITLE_POINT[0],
                        y: TITLE_POINT[1], button: Some(0) }));
                }
                if frame != 0 && frame % 600 == 0 {
                    let hit = player.masked_input_region_at(TITLE_POINT)?
                        .map(|words| (words[0], words[7], words[8], words[9], words[11]));
                    let candidates = player.mouse_press_candidate_regions(TITLE_POINT)?;
                    let button = player.input_button_state(TITLE_REGION)
                        .map(|state| (state.receiver, state.show));
                    eprintln!("frame {frame}: 07 title gate owner={owner:?} hit(id,hover,enabled,visible,bitmap)={hit:?} candidates={candidates:?} button(receiver,show)={button:?}");
                }
            }
            Phase::Opening => {
                if self.opening.stopped {
                    return Ok(Decision::Stop("checkpoint_mismatch", "07 OP stopped or changed instance".into()));
                }
                if let Some(released_at) = self.opening.released_at
                    && let Some((owner, region, receiver, context, load, 0, 57, epoch, source, complete)) =
                        player.host_kamikaze_body()
                    && complete > released_at
                {
                    eprintln!("frame {frame}: 07 com01 0/57 body=56 owner={owner} context={context} load={load} \
                        epoch={epoch} source={source} complete={complete} region={region} receiver={receiver} key=2 group=0");
                    eprintln!("frame {frame}: probe wait {}", player.host_replay_wait_diagnostics());
                    let Some(expected) = &contract.keyboard_press else {
                        return Ok(Decision::Done);
                    };
                    if (expected.region, expected.receiver) != (region, receiver) {
                        return Ok(Decision::Stop("checkpoint_mismatch", format!(
                            "07 expected Enter region={} receiver={}, live region={region} receiver={receiver}",
                            expected.region, expected.receiver)));
                    }
                    self.phase = Phase::Body(BodyPress::arm(
                        frame, 0, (owner, context, load), epoch, player.events().len(),
                    ));
                    return Ok(Decision::Input(Action::Key(InputKey::Keyboard(13))));
                }
            }
            Phase::Body(body) => {
                // The replay loop polls again only after the keyboard release was observed.
                if body.receipt != KeyboardPressReceipt::Verified || body.root_resumes != 1 {
                    return Ok(Decision::Stop("input_missed", "07 Enter did not yield one consuming receipt and root resume".into()));
                }
                if body.lifetime != NovelWindowLifetime::Continuous {
                    return Ok(Decision::Stop("checkpoint_mismatch", "07 meswnd was replaced".into()));
                }
                let next_group = body.from_group + 1;
                let Some((next_wait, next_body)) = HeadlessPlayer::kamikaze_replay_page(next_group) else {
                    return Ok(Decision::Stop("checkpoint_mismatch", "07 next page is outside the route".into()));
                };
                if let Some((owner, region, receiver, context, load, group, record, epoch, source, complete)) =
                    player.host_kamikaze_body()
                    && (group, record) == (next_group, next_wait)
                {
                    let expected = contract.keyboard_press.as_ref().unwrap();
                    if (owner, context, load) != (body.owner, body.context, body.load)
                        || (region, receiver) != (expected.region, expected.receiver)
                        || epoch <= body.epoch || source <= body.frame
                        || body.command_steps != body.transition_commands().len()
                    {
                        return Ok(Decision::Stop("checkpoint_mismatch", format!(
                            "07 r{record} body, load, receiver or natural transition did not match")));
                    }
                    eprintln!("frame {frame}: 07 com01 {group}/{record} body={next_body} owner={owner} context={context} load={load} \
                        epoch={epoch} source={source} complete={complete} region={region} receiver={receiver} key=2 group=0 release=observed");
                    eprintln!("frame {frame}: probe wait {}", player.host_replay_wait_diagnostics());
                    if group == contract.last_group.unwrap_or(1) {
                        return Ok(Decision::Done);
                    }
                    self.phase = Phase::Body(BodyPress::arm(
                        frame, group, (owner, context, load), epoch, player.events().len(),
                    ));
                    return Ok(Decision::Input(Action::Key(InputKey::Keyboard(13))));
                }
            }
            Phase::Failed(reason) => {
                return Ok(Decision::Stop("input_missed", reason.clone()));
            }
            _ => {}
        }
        Ok(Decision::Wait)
    }
}

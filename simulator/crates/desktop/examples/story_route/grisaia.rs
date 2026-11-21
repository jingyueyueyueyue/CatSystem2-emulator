use anyhow::Result;
use cs2_desktop::{Grisaia14Body, HeadlessPlayer, LogicEvent};
use cs2_runtime::InputKey;
use cs2_runtime::message_object::{ButtonSource, InputRouteReceipt, RoutedPhysicalButton};

use super::{Action, Decision, KeyboardPress, KeyboardPressReceipt, NovelWindowLifetime};

const WAITS: [i32; 9] = [6, 8, 78, 80, 82, 115, 124, 126, 130];
const POINT: [i32; 2] = [512, 354];
const TITLE_CANDIDATES: [u32; 6] = [3, 37, 19, 55, 10, 7];
const BOTTOM: (u32, u32) = (61, 8);

pub(super) enum GrisaiaRoute {
    Title {
        moved: bool,
    },
    TitlePress {
        frame: u64,
    },
    FirstBody {
        since: u64,
    },
    Successor {
        group: usize,
        started: u64,
        origin: Grisaia14Body,
        lifetimes: [NovelWindowLifetime; 3],
        candidates: Vec<(u32, u32)>,
        receipt: KeyboardPressReceipt,
    },
    Failed(&'static str),
}

impl GrisaiaRoute {
    pub(super) fn observe_lifetimes(&mut self, events: &[LogicEvent], batch_start: usize) {
        if let Self::Successor { lifetimes, .. } = self {
            for (id, lifetime) in [12, 61, 8].into_iter().zip(lifetimes) {
                lifetime.observe(id, events, batch_start);
            }
        }
    }

    pub(super) fn observe_frame(&mut self, player: &HeadlessPlayer) {
        let receipts = player.host_input_route_receipts();
        match self {
            Self::TitlePress { frame } => {
                let press_frame = *frame + 1;
                if !exact_press_chain(
                    receipts,
                    press_frame,
                    ButtonSource::Mouse,
                    1,
                    [(0, 3, 0, 1), (0, 37, 11, 0)],
                ) || !unique_resume(player.events(), press_frame, "message=[2, 512, 354, 0]")
                {
                    *self = Self::Failed("14 title receipt or root resume differed");
                    return;
                }
                eprintln!("frame {press_frame}: 14 verified title receipt and VM resume");
                *self = Self::FirstBody { since: press_frame };
            }
            Self::Successor {
                started, candidates, receipt, ..
            } => {
                if *receipt == KeyboardPressReceipt::Awaiting {
                    let press_frame = *started + 1;
                    let through_bottom = candidates.split_inclusive(|pair| *pair == BOTTOM)
                        .next().unwrap_or_default();
                    let expected = candidates
                        .iter()
                        .map(|&(region, receiver)| (0, region, receiver, 1))
                        .chain(through_bottom.iter().map(|&(region, receiver)| {
                            (2, region, receiver, u32::from((region, receiver) != BOTTOM))
                        }));
                    let consuming = KeyboardPress {
                        region: 61,
                        receiver: 8,
                        key: 2,
                        group: 0,
                        response: 0,
                    };
                    if !exact_press_chain(
                        receipts,
                        press_frame,
                        ButtonSource::Keyboard,
                        13,
                        expected,
                    ) || receipt
                        .observe(&consuming, *started, receipts, player.events())
                        .is_none()
                    {
                        *self = Self::Failed("14 Enter receipt or root resume differed");
                        return;
                    }
                    eprintln!(
                        "frame {press_frame}: 14 verified Enter key=2 group=0 region=61 receiver=8 and VM resume"
                    );
                } else if receipts.iter().any(|route| route.pressed != 0) {
                    *self = Self::Failed("14 received another press after Enter");
                }
            }
            _ => {}
        }
    }

    pub(super) fn poll(
        &mut self,
        player: &mut HeadlessPlayer,
        frame: u64,
        idle: u64,
        last_group: usize,
    ) -> Result<Decision> {
        if frame % 120 == 0 {
            println!("replay frame={frame} heartbeat");
        }
        match self {
            Self::Title { moved } => {
                if player.host_grisaia_14_title() {
                    let [x, y] = POINT;
                    if !*moved {
                        *moved = true;
                        return Ok(Decision::Input(Action::Pointer { x, y, button: None }));
                    }
                    if let Some(mask) = player.masked_input_region_at(POINT)?
                        && mask[0] == 37
                        && mask[7] != 0
                        && mask[8] != 0
                        && player.mouse_press_candidate_regions(POINT)? == TITLE_CANDIDATES
                    {
                        *self = Self::TitlePress { frame };
                        return Ok(Decision::Input(Action::Pointer {
                            x,
                            y,
                            button: Some(0),
                        }));
                    }
                }
                if frame >= idle {
                    return Ok(Decision::Stop(
                        "unrecognized_wait",
                        "14 title did not settle".into(),
                    ));
                }
            }
            Self::FirstBody { since } => {
                let reason = match player.host_grisaia_14_body(0, 6, *since) {
                    Ok(origin) => {
                        return self.arm(player, frame, 0, origin, frame);
                    }
                    Err(reason) => reason,
                };
                if frame - *since >= idle {
                    eprintln!("{}", player.host_grisaia_14_body_diagnostics());
                    return Ok(Decision::Stop(
                        "unrecognized_wait",
                        format!("14 OP g0/6 did not settle: {reason}"),
                    ));
                }
            }
            Self::Successor {
                group,
                started,
                origin,
                lifetimes,
                receipt,
                ..
            } => {
                // story_replay polls here only after the down/up and live scan release.
                if *receipt != KeyboardPressReceipt::Verified
                    || lifetimes
                        .iter()
                        .any(|state| *state != NovelWindowLifetime::Continuous)
                {
                    return Ok(Decision::Stop(
                        "checkpoint_mismatch",
                        "14 input or task lifetime changed".into(),
                    ));
                }
                let next_group = *group + 1;
                let wait = WAITS[next_group];
                let reason = match player.host_grisaia_14_body(next_group as i32, wait, *started + 1) {
                    Ok(next) => {
                        if (next.context, next.load, next.owner)
                            != (origin.context, origin.load, origin.owner)
                            || next.epoch <= origin.epoch
                            || next.source_frame <= *started
                            || next.marked <= *started
                        {
                            return Ok(Decision::Stop(
                                "checkpoint_mismatch",
                                "14 successor was not born after Enter in the same OP load".into(),
                            ));
                        }
                        if next_group == last_group {
                            eprintln!("frame {frame}: 14 settled OP g{next_group}/{wait} after release body={next:?}");
                            return Ok(Decision::Done);
                        }
                        let deadline = started.saturating_add(idle);
                        return self.arm(player, frame, next_group, next, deadline);
                    }
                    Err(reason) => reason,
                };
                if frame - *started >= idle {
                    eprintln!("{}", player.host_grisaia_14_body_diagnostics());
                    return Ok(Decision::Stop(
                        "unrecognized_wait",
                        format!("14 OP g{next_group}/{wait} did not settle: {reason}"),
                    ));
                }
            }
            Self::Failed(reason) => return Ok(Decision::Stop("input_missed", (*reason).into())),
            Self::TitlePress { .. } => {}
        }
        Ok(Decision::Wait)
    }

    fn arm(
        &mut self, player: &mut HeadlessPlayer, frame: u64, group: usize,
        origin: Grisaia14Body, deadline: u64,
    ) -> Result<Decision> {
        let wait = WAITS[group];
        let candidates = match player.host_grisaia_14_enter_binding(group)? {
            Ok(candidates) => candidates,
            Err(reason) if matches!(group, 6 | 7) && frame < deadline => {
                if frame % 120 == 0 {
                    eprintln!("frame {frame}: 14 waiting for OP g{group}/{wait} Enter binding: {reason}");
                }
                return Ok(Decision::Wait);
            }
            Err(reason) => {
                eprintln!("{}", player.host_grisaia_14_body_diagnostics());
                return Ok(Decision::Stop("checkpoint_mismatch",
                    format!("14 live Enter binding differs at OP g{group}/{wait}: {reason}")));
            }
        };
        if group != 0 {
            eprintln!("frame {frame}: 14 settled OP g{group}/{wait} after release body={origin:?}");
        }
        eprintln!("{}", player.host_grisaia_14_body_diagnostics());
        eprintln!("frame {frame}: 14 Enter candidates={candidates:?}");
        eprintln!("frame {frame}: 14 armed OP g{group}/{wait} region=61 receiver=8 body={origin:?}");
        *self = Self::Successor {
            group,
            started: frame,
            origin,
            lifetimes: [NovelWindowLifetime::Armed { first_event: player.events().len() }; 3],
            candidates,
            receipt: KeyboardPressReceipt::Awaiting,
        };
        Ok(Decision::Input(Action::Key(InputKey::Keyboard(13))))
    }
}

fn exact_press_chain(
    receipts: &[InputRouteReceipt],
    frame: u64,
    source: ButtonSource,
    code: u8,
    expected: impl IntoIterator<Item = (u32, u32, u32, u32)>,
) -> bool {
    let physical = [RoutedPhysicalButton { source, code }];
    let mut actual = receipts.iter().filter(|item| item.pressed != 0);
    expected.into_iter().all(|entry| {
        actual.next().is_some_and(|item| {
            item.frame == frame
                && item.group == 0
                && item.raw_mask == source.mask()
                && item.pressed == 1
                && !item.repeated
                && item.physical == physical
                && (item.key, item.region, item.receiver, item.response) == entry
        })
    }) && actual.next().is_none()
}

fn unique_resume(events: &[LogicEvent], frame: u64, packet: &str) -> bool {
    let mut resumes = events
        .iter()
        .rev()
        .take_while(|event| event.frame >= frame)
        .filter(|event| event.frame == frame && event.kind == "message-resume");
    resumes.next().is_some_and(|event| event.detail == packet) && resumes.next().is_none()
}

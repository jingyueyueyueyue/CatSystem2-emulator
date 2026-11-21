use super::{
    Action, Decision, HeadlessPlayer, InputKey, LogicEvent, MousePress, NovelWindowLifetime,
    consuming_mouse_resume,
};
use cs2_desktop::CampanellaWait;
use cs2_runtime::message_object::{ButtonSource, RoutedPhysicalButton};

const TITLE: [i32; 2] = [183, 434];

#[derive(Default)]
pub(super) enum Replay {
    #[default]
    Title,
    TitlePress {
        frame: u64,
        owner: u32,
    },
    First {
        title_owner: u32,
        since: u64,
        previous: Option<(CampanellaWait, i32)>,
    },
    Successor {
        title_owner: u32,
        group: usize,
        pressed: u64,
        first: CampanellaWait,
        bottom: (u32, u32),
        candidates: Vec<(u32, u32)>,
        lifetimes: [NovelWindowLifetime; 5],
        verified: bool,
        previous: Option<(CampanellaWait, i32)>,
    },
    Failed(&'static str),
}

impl Replay {
    pub(super) fn observe_lifetimes(&mut self, events: &[LogicEvent], batch_start: usize) {
        if let Self::Successor {
            first,
            bottom,
            lifetimes,
            ..
        } = self
        {
            for (lifetime, id) in lifetimes.iter_mut().zip([
                first.script,
                first.owner,
                first.plane,
                first.string,
                bottom.0,
            ]) {
                lifetime.observe(id, events, batch_start);
            }
        }
    }

    pub(super) fn observe_frame(&mut self, player: &HeadlessPlayer) {
        match self {
            Self::TitlePress { frame, owner } => {
                let frame = *frame + 1;
                let press = MousePress {
                    candidates: vec![],
                    region: 11,
                    key: 0,
                    group: 0,
                    response: 0,
                };
                if consuming_mouse_resume(player, &press, frame, *owner, TITLE, None) {
                    eprintln!("frame {frame}: 18 verified title click region=11 receiver={owner}");
                    *self = Self::First {
                        title_owner: *owner,
                        since: frame,
                        previous: None,
                    };
                } else {
                    *self = Self::Failed("18 title receipt or VM resume differed");
                }
            }
            Self::Successor {
                pressed,
                bottom,
                candidates,
                verified,
                ..
            } => {
                let receipts = player
                    .host_input_route_receipts()
                    .iter()
                    .filter(|receipt| receipt.pressed != 0);
                if *verified {
                    if receipts.count() != 0 {
                        *self = Self::Failed("18 received another press after Enter");
                    }
                    return;
                }
                let mut key = 0;
                let mut cursor = 0;
                let mut consumed = false;
                for receipt in receipts {
                    let target = (receipt.region, receipt.receiver);
                    if receipt.key != key && key == 0 && receipt.key == 2 {
                        key = 2;
                        cursor = 0;
                    }
                    let candidate = candidates[cursor..].iter().position(|&pair| pair == target);
                    if consumed
                        || receipt.frame != *pressed + 1
                        || receipt.key != key
                        || receipt.group != 0
                        || receipt.raw_mask != 1
                        || receipt.pressed != 1
                        || receipt.repeated
                        || candidate.is_none()
                        || receipt.physical
                            != [RoutedPhysicalButton {
                                source: ButtonSource::Keyboard,
                                code: 13,
                            }]
                    {
                        *self = Self::Failed("18 Enter receipt differed from the armed live chain");
                        return;
                    }
                    cursor += candidate.unwrap() + 1;
                    if receipt.response == 0 {
                        consumed = key == 2 && target == *bottom;
                        if !consumed {
                            *self = Self::Failed("18 Enter consumed by a different callback");
                            return;
                        }
                    }
                }
                if !consumed {
                    *self = Self::Failed("18 no consuming Enter receipt");
                    return;
                }
                *verified = true;
                eprintln!(
                    "frame {}: 18 verified Enter key=2 group=0 region={} receiver={}",
                    *pressed + 1,
                    bottom.0,
                    bottom.1
                );
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
    ) -> anyhow::Result<Decision> {
        if frame % 120 == 0 {
            println!("replay frame={frame} heartbeat");
        }
        match self {
            Self::Title => {
                if let Some(owner) = player.host_campanella_title()
                    && let Some(hit) = player.masked_input_region_at(TITLE)?
                    && hit[0] == 11
                    && hit[8] != 0
                    && player.mouse_press_candidate_regions(TITLE)? == [11, 29, 6]
                {
                    let [x, y] = TITLE;
                    *self = Self::TitlePress { frame, owner };
                    return Ok(Decision::Input(Action::Pointer {
                        x,
                        y,
                        button: Some(0),
                    }));
                }
                if frame >= idle {
                    return Ok(Decision::Stop(
                        "unrecognized_wait",
                        "18 title wait did not settle".into(),
                    ));
                }
            }
            Self::First { title_owner, since, .. } if last_group == 0 => {
                let result = player.host_campanella_localized_entry(*title_owner, *since);
                if let Ok((handle, loaded, marked)) = result {
                    eprintln!("frame {frame}: 18 localized entry g0/94 handle={handle} loaded={loaded} marked={marked}");
                    return Ok(Decision::Done);
                }
                if frame - *since >= idle {
                    eprintln!("{}", player.host_campanella_input_diagnostics(*title_owner));
                    return Ok(Decision::Stop("unrecognized_wait", format!(
                        "18 localized entry g0/94 did not settle: {}", result.unwrap_err())));
                }
            }
            Self::First { title_owner, since, previous } => {
                let title_owner = *title_owner;
                let result = player.host_campanella_wait(0, title_owner);
                let current = result.ok();
                if let Some((first, wait)) = current.filter(|_| current == *previous) {
                    return self.arm(player, frame, 0, wait, first, title_owner);
                }
                *previous = current;
                if frame - *since >= idle {
                    eprintln!("{}", player.host_campanella_input_diagnostics(title_owner));
                    let reason = result.err().unwrap_or("waiting for the second stable frame");
                    return Ok(Decision::Stop(
                        "unrecognized_wait",
                        format!("18 g0 body did not settle: {reason}"),
                    ));
                }
            }
            Self::Successor {
                title_owner,
                group,
                pressed,
                first,
                lifetimes,
                verified,
                previous,
                ..
            } => {
                let title_owner = *title_owner;
                if !*verified
                    || lifetimes
                        .iter()
                        .any(|lifetime| *lifetime != NovelWindowLifetime::Continuous)
                {
                    return Ok(Decision::Stop(
                        "checkpoint_mismatch",
                        "18 Enter or task lifetime differed".into(),
                    ));
                }
                let next_group = *group + 1;
                let result = player.host_campanella_wait(next_group as i32, title_owner);
                let current = result.ok();
                if let Some((next, wait)) = current.filter(|_| current == *previous) {
                    if !first.same_instance(next)
                        || next.generation <= first.generation
                        || next.marked <= *pressed
                    {
                        return Ok(Decision::Stop(
                            "checkpoint_mismatch",
                            format!("18 g{next_group}/{wait} did not settle in the same live load with a new body"),
                        ));
                    }
                    eprintln!("frame {frame}: 18 settled g{next_group}/{wait} after release body={next:?}");
                    if next_group == last_group {
                        return Ok(Decision::Done);
                    }
                    return self.arm(player, frame, next_group, wait, next, title_owner);
                }
                *previous = current;
                if frame - *pressed >= idle {
                    eprintln!("{}", player.host_campanella_input_diagnostics(title_owner));
                    return Ok(Decision::Stop(
                        "unrecognized_wait",
                        format!("18 g{next_group} body did not settle: {}",
                            result.err().unwrap_or("waiting for the second stable frame")),
                    ));
                }
            }
            Self::Failed(reason) => {
                return Ok(Decision::Stop("checkpoint_mismatch", (*reason).into()));
            }
            Self::TitlePress { .. } => {}
        }
        Ok(Decision::Wait)
    }

    fn arm(
        &mut self,
        player: &mut HeadlessPlayer,
        frame: u64,
        group: usize,
        wait: i32,
        first: CampanellaWait,
        title_owner: u32,
    ) -> anyhow::Result<Decision> {
        eprintln!("{}", player.host_campanella_input_diagnostics(title_owner));
        let Some((bottom, candidates)) = player.host_campanella_enter(first)? else {
            return Ok(Decision::Stop("checkpoint_mismatch",
                format!("18 g{group}/{wait} has an unverified live Enter binding")));
        };
        eprintln!("frame {frame}: 18 armed g{group}/{wait} body={first:?} bottom={bottom:?} candidates={candidates:?}");
        *self = Self::Successor {
            title_owner,
            group,
            pressed: frame,
            first,
            bottom,
            candidates,
            lifetimes: [NovelWindowLifetime::Armed { first_event: player.events().len() }; 5],
            verified: false,
            previous: None,
        };
        Ok(Decision::Input(Action::Key(InputKey::Keyboard(13))))
    }
}

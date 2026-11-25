use anyhow::{Context, Result, ensure};
use cs2_desktop::{HeadlessPlayer, HostSettingKind, HostSettingsEdit, HostWindowRequest};
use cs2_runtime::message_object::{ButtonSource, RoutedPhysicalButton};
use std::collections::VecDeque;
use super::Action;

pub(super) enum TitleAction {
    Wait,
    Move([i32; 2]),
    Click([i32; 2]),
    CaptureResult,
}

pub(super) struct Eye08Title {
    point: [i32; 2],
    region: u32,
    hovered: Option<(u64, u32)>,
    pressed: Option<u64>,
}

impl Eye08Title {
    pub fn new(button: &str) -> Result<Self> {
        let (point, region) = match button {
            "story" => ([634, 90], 13),
            "continue" => ([633, 239], 14),
            "exit" => ([639, 514], 16),
            _ => anyhow::bail!("unknown 08 title button: {button}"),
        };
        Ok(Self { point, region, hovered: None, pressed: None })
    }

    pub fn poll(&mut self, player: &mut HeadlessPlayer, frame: u64) -> Result<TitleAction> {
        if let Some(pressed) = self.pressed {
            if frame == pressed + 1 {
                let (_, owner) = self.hovered.context("missing 08 hover owner")?;
                let matched = player.host_input_route_receipts().iter().filter(|receipt| {
                    receipt.frame == frame && !receipt.repeated && receipt.pressed == 1
                        && receipt.response == 0 && receipt.raw_mask == ButtonSource::Mouse.mask()
                        && receipt.physical == [RoutedPhysicalButton { source: ButtonSource::Mouse, code: 1 }]
                        && (receipt.region, receipt.receiver) == (self.region, owner)
                }).count();
                ensure!(matched == 1, "08 title region {} lacks a unique mouse receipt at {frame}", self.region);
            }
            return Ok(if frame == pressed + 150 { TitleAction::CaptureResult } else { TitleAction::Wait });
        }
        let Some(owner) = player.host_replay_eye08_entry_owner(false) else {
            return Ok(TitleAction::Wait);
        };
        let Some((before, hovered_owner)) = self.hovered else {
            self.hovered = Some((frame, owner));
            return Ok(TitleAction::Move(self.point));
        };
        ensure!(hovered_owner == owner, "08 title owner changed after hover");
        if player.host_pointer_logical_position() == self.point
            && player.masked_input_region_at(self.point)?.is_some_and(|words| {
                words[0] == self.region && words[7] != 0 && words[8] != 0
            })
            && player.mouse_press_candidate_regions(self.point)?.contains(&self.region)
        {
            self.pressed = Some(frame);
            eprintln!("frame {frame}: live button before={before} region={}", self.region);
            return Ok(TitleAction::Click(self.point));
        }
        Ok(TitleAction::Wait)
    }
}

#[derive(Default)]
pub(super) struct EdenEntry {
    stage: usize,
    hovered: bool,
    pressed: Option<u64>,
}

impl EdenEntry {
    pub fn done(&self) -> bool {
        self.stage == 2
    }

    pub fn poll(&mut self, player: &mut HeadlessPlayer, frame: u64, busy: bool)
        -> Result<Option<Action>>
    {
        let ([x, y], region, key, response) = if self.stage == 0 {
            ([640, 441], 20, 2, 1)
        } else {
            ([640, 549], 55, 0, 0)
        };
        if let Some(pressed) = self.pressed {
            let hit = player.host_input_route_receipts().iter().any(|receipt| {
                receipt.frame == frame && !receipt.repeated && receipt.pressed == 1
                    && (receipt.region, receipt.key, receipt.response) == (region, key, response)
                    && receipt.physical == [RoutedPhysicalButton { source: ButtonSource::Mouse, code: 1 }]
            });
            let message = format!("message=[2, {x}, {y}, 0]");
            ensure!(frame == pressed + 1 && hit && player.events().iter().rev()
                .take_while(|event| event.frame == frame)
                .any(|event| event.kind == "message-resume" && event.detail == message),
                "13 entry region {region} did not produce the expected mouse receipt");
            self.stage += 1;
            self.hovered = false;
            self.pressed = None;
            eprintln!("frame {frame}: 13 entry region {region} verified");
            return Ok(None);
        }
        if busy || !player.host_replay_eden_entry_ready(self.stage == 0) {
            return Ok(None);
        }
        if !self.hovered {
            self.hovered = true;
            return Ok(Some(Action::Pointer { x, y, button: None }));
        }
        if player.masked_input_region_at([x, y])?.is_some_and(|hit| hit[0] == region && hit[8] != 0)
            && player.mouse_press_candidate_regions([x, y])?.contains(&region)
        {
            self.pressed = Some(frame);
            return Ok(Some(Action::Pointer { x, y, button: Some(0) }));
        }
        Ok(None)
    }
}

struct ReadyClick {
    region: u32,
    point: [i32; 2],
    require_root_resume: bool,
}

struct ReadyPress {
    frame: u64,
    receiver: u32,
    routed: bool,
    resumed: Option<u64>,
    retry_hovered: bool,
    retries: u8,
}

pub(super) fn choice_target_enabled(player: &HeadlessPlayer, receiver: u32, region: u32) -> bool {
    player.input_button_state(region).and_then(|button| button.receiver) == Some(receiver)
        && player.host_replay_input_regions().any(|hit| hit[0] == region && hit[8] != 0)
}

#[derive(Default)]
pub(super) struct ReadyEntry {
    clicks: VecDeque<ReadyClick>,
    hovered: bool,
    pressed: Option<ReadyPress>,
}

impl ReadyEntry {
    pub fn add(&mut self, value: &str) -> Result<()> {
        let (region, point) = value.split_once(':').context("expected ready-click:region:x,y")?;
        let (x, y) = point.split_once(',').context("expected ready-click:region:x,y")?;
        self.clicks.push_back(ReadyClick {
            region: region.parse()?, point: [x.parse()?, y.parse()?], require_root_resume: true,
        });
        Ok(())
    }

    pub fn add_choice(&mut self, region: u32, point: [i32; 2]) {
        self.clicks.push_back(ReadyClick { region, point, require_root_resume: false });
    }

    pub fn regions(&self) -> impl Iterator<Item = u32> + '_ {
        self.clicks.iter().map(|click| click.region)
    }

    pub fn done(&self) -> bool {
        self.clicks.is_empty()
    }

    pub fn waiting(&self) -> Option<String> {
        let click = self.clicks.front()?;
        Some(match &self.pressed {
            Some(press) => format!(
                "region={} point={:?} pressed={} receiver={} routed={} resumed={:?} retries={}",
                click.region, click.point, press.frame, press.receiver, press.routed, press.resumed,
                press.retries,
            ),
            None => format!("region={} point={:?} awaiting enabled button", click.region, click.point),
        })
    }

    pub fn poll(&mut self, player: &mut HeadlessPlayer, frame: u64, busy: bool)
        -> Result<Option<Action>>
    {
        let Some(&ReadyClick { region, point: [x, y], require_root_resume }) = self.clicks.front()
        else { return Ok(None); };
        if let Some(press) = self.pressed.as_mut() {
            let hits = player.host_input_route_receipts().iter().filter(|receipt| {
                receipt.frame == frame && !receipt.repeated && receipt.pressed == 1
                    && (receipt.region, receipt.receiver) == (region, press.receiver)
                    && receipt.raw_mask == ButtonSource::Mouse.mask()
                    && receipt.physical == [RoutedPhysicalButton { source: ButtonSource::Mouse, code: 1 }]
            });
            press.routed |= hits.filter(|receipt| require_root_resume || receipt.response == 0).count() == 1;
            let message = format!("message=[2, {x}, {y}, 0]");
            if player.events().iter().rev().take_while(|event| event.frame == frame)
                .any(|event| event.kind == "message-resume" && event.detail == message)
            {
                press.resumed = Some(frame);
            }
            let target_withdrawn = || !choice_target_enabled(player, press.receiver, region);
            // Legacy FES menus can resume the VM directly. Accept the exact
            // click message once its previously verified target withdraws.
            if !require_root_resume && (press.routed
                || (press.resumed.is_some() && target_withdrawn()))
            {
                eprintln!("frame {frame}: choice-click region={region} receiver={} routed={} resumed={:?}",
                    press.receiver, press.routed, press.resumed);
                self.clicks.pop_front();
                self.hovered = false;
                self.pressed = None;
                return Ok(None);
            }
            // Some menu callbacks resume the ordinary script without a typed press receipt.
            // In that path the same pointer target must withdraw after the script resumes.
            if require_root_resume && let Some(resumed) = press.resumed
                && (press.routed || target_withdrawn())
            {
                eprintln!("frame {frame}: ready-click region={region} receiver={} resumed={resumed} routed={}",
                    press.receiver, press.routed);
                self.clicks.pop_front();
                self.hovered = false;
                self.pressed = None;
                return Ok(None);
            }
            // Keep observing the outstanding press while re-hovering. The
            // callback can consume the original edge during that frame.
            if !require_root_resume && !busy && frame >= press.frame.saturating_add(12)
                && press.resumed.is_none() && press.retries < 2 && !target_withdrawn()
                && player.masked_input_region_at([x, y])?.is_some_and(|hit| hit[0] == region && hit[8] != 0)
                && player.mouse_press_candidate_regions([x, y])?.contains(&region)
            {
                if !press.retry_hovered {
                    press.retry_hovered = true;
                    return Ok(Some(Action::Pointer { x, y, button: None }));
                }
                if player.host_pointer_move_state().remaining <= 0 {
                    eprintln!("frame {frame}: retrying unconsumed choice-click region={region} receiver={}", press.receiver);
                    press.frame = frame;
                    press.retry_hovered = false;
                    press.retries += 1;
                    return Ok(Some(Action::Pointer { x, y, button: Some(0) }));
                }
            }
            return Ok(None);
        }
        // Choice input already waits for the replay's outstanding press and
        // Ctrl release. A script can leave unrelated logical key slots latched.
        if busy || (require_root_resume && player.device_key_scan_state().records()
            .iter().flatten().any(|key| key.mask & 3 != 0))
        {
            return Ok(None);
        }
        if !self.hovered {
            if player.input_button_state(region).is_some_and(|button| button.receiver.is_some()) {
                self.hovered = true;
                return Ok(Some(Action::Pointer { x, y, button: None }));
            }
            return Ok(None);
        }
        let point = [x, y];
        // A choice press moves to its verified point again before button-down.
        // Give an in-flight script cursor animation time to finish first.
        let pointer_ready = if require_root_resume {
            player.host_pointer_logical_position() == point
        } else {
            player.host_pointer_move_state().remaining <= 0
        };
        if pointer_ready
            && player.masked_input_region_at(point)?.is_some_and(|hit| hit[0] == region && hit[8] != 0)
            && player.mouse_press_candidate_regions(point)?.contains(&region)
            && let Some(receiver) = player.input_button_state(region).and_then(|button| button.receiver)
        {
            self.pressed = Some(ReadyPress {
                frame, receiver, routed: false, resumed: None, retry_hovered: false, retries: 0,
            });
            return Ok(Some(Action::Pointer { x, y, button: Some(0) }));
        }
        Ok(None)
    }
}

#[derive(Default)]
pub(super) struct Probe {
    pub enabled: bool,
    pub apply_settings: bool,
    installed_menu: Option<u64>,
    prior_setting: Option<(u32, u32, u32)>,
}

impl Probe {
    pub fn inspect(&self, player: &mut HeadlessPlayer, frame: u64) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }
        let [width, height] = player.logical_size().map(|value| value as i32);
        let regions: Vec<_> = player.host_replay_input_regions().collect();
        for region in regions {
            let [left, top, right, bottom] = [region[3], region[4], region[5], region[6]]
                .map(|value| value as i32);
            let (left, right) = (left.max(0), right.min(width));
            let (top, bottom) = (top.max(0), bottom.min(height));
            if left >= right || top >= bottom {
                continue;
            }
            let id = region[0];
            let mut point = None;
            let enabled = region[8] != 0 && region[2] & ButtonSource::Mouse.mask() != 0;
            if enabled {
                'points: for y in samples(top, bottom) {
                    for x in samples(left, right) {
                        if player.masked_input_region_at([x, y])?.is_some_and(|hit| hit[0] == id)
                            && player.mouse_press_candidate_regions([x, y])?.contains(&id)
                        {
                            point = Some([x, y]);
                            break 'points;
                        }
                    }
                }
            }
            let bounds = [left, top, right, bottom];
            eprintln!("frame {frame}: button region={id} bounds={bounds:?} enabled={enabled} point={point:?}");
        }
        Ok(())
    }

    pub fn menu_command(&self, player: &mut HeadlessPlayer, frame: u64, command: u32) -> Result<()> {
        let id = self.installed_menu.context("menu action before the game installed a menu")?;
        eprintln!("frame {frame}: menu command={command} id={id}");
        player.host_game_menu_command(id, command)
    }

    pub fn window_request(
        &mut self, player: &mut HeadlessPlayer, frame: u64, request: HostWindowRequest,
    ) -> Result<bool> {
        match request {
            HostWindowRequest::Menu => {
                let menu = player.host_game_menu()?;
                if self.enabled {
                    eprintln!("frame {frame}: menu installed {menu:?}");
                }
                player.complete_host_game_menu(menu.id)?;
                self.installed_menu = Some(menu.id);
            }
            HostWindowRequest::Settings => {
                let settings = player.host_effect_settings()?;
                let pages: Vec<_> = settings.pages.iter().map(|page| page.resource).collect();
                eprintln!("frame {frame}: settings opened pages={pages:?}");
                if self.apply_settings {
                    if let Some((resource, command, value)) = self.prior_setting {
                        let control = settings.pages.iter().find(|page| page.resource == resource)
                            .and_then(|page| page.controls.iter().find(|control| control.command == command));
                        if let Some(control) = control {
                            ensure!(control.value == value, "reopened setting {command} lost applied value");
                            eprintln!("frame {frame}: settings reopened resource={resource} command={command} value={value}");
                            self.prior_setting = None;
                        }
                    } else if let Some((page_index, control_index, value)) = settings.pages.iter()
                        .enumerate().find_map(|(page_index, page)| {
                            if page.input.is_some() || page.font.is_some() || page.sound.is_some() {
                                return None;
                            }
                            page.controls.iter().enumerate().find_map(|(control_index, control)| {
                                (!control.hidden && !control.disabled && control.enabled_by.is_none()
                                    && matches!(control.kind, HostSettingKind::Toggle))
                                    .then_some((page_index, control_index, u32::from(control.value == 0)))
                            })
                        })
                    {
                        let mut edits: Vec<_> = settings.pages.iter().map(|page| HostSettingsEdit {
                            reset: false, words: page.controls.iter().map(|item| item.value).collect(),
                        }).collect();
                        let page = &settings.pages[page_index];
                        let command = page.controls[control_index].command;
                        edits[page_index].words[control_index] = value;
                        player.apply_host_effect_settings(settings.id, &edits)?;
                        let applied = player.host_effect_settings()?;
                        ensure!(applied.pages[page_index].controls[control_index].value == value,
                            "settings Apply did not retain command {command}");
                        self.prior_setting = Some((page.resource, command, value));
                        eprintln!("frame {frame}: settings applied resource={} command={command} value={value}", page.resource);
                    }
                }
                player.close_host_effect_settings(settings.id)?;
                eprintln!("frame {frame}: settings closed");
            }
            HostWindowRequest::MenuConfirmation if self.enabled => {
                let confirmation = player.host_menu_confirmation()?;
                player.complete_host_menu_confirmation(confirmation.id, false)?;
                eprintln!("frame {frame}: return-to-title confirmation cancelled");
            }
            HostWindowRequest::SystemVolume if self.enabled => {
                let (id, _) = player.host_system_volume_request()?;
                player.complete_host_system_volume(id, Err("background replay has no system mixer".into()))?;
                eprintln!("frame {frame}: system mixer request observed");
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    pub fn complete_local_requests(&self, player: &mut HeadlessPlayer, frame: u64) -> Result<()> {
        if self.enabled {
            while let Some(dialog) = player.take_host_dialog_request() {
                eprintln!("frame {frame}: game dialog title={:?} text={:?}", dialog.title, dialog.text);
                player.complete_host_dialog(dialog.id, Ok(()))?;
            }
            while let Some(open) = player.take_host_system_open_request() {
                eprintln!("frame {frame}: external open request observed");
                player.complete_host_system_open(open.id, Err("background replay does not open external targets".into()))?;
            }
        }
        Ok(())
    }
}

fn samples(start: i32, end: i32) -> [i32; 5] {
    let span = end - start - 1;
    [2, 1, 3, 0, 4].map(|quarter| start + span * quarter / 4)
}

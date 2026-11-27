//! Posted window commands. Hosts consume real requests outside VM execution.
use std::collections::VecDeque;
use anyhow::{Result, bail};
use cs2_script::{NativeReturn, ScriptError};
use super::{HeadlessPlayer, LoggingHost, LogicEvent, native_state_error};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostWindowRequest {
    ToggleFullscreen,
    Minimize,
    Settings,
    Menu,
    MenuConfirmation,
    SystemVolume,
    MenuInfo,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PostedWindowCommand { ToggleFullscreen, SetWindowed(u32), Minimize, Close, Settings, GeneralSettings, Menu, Notify([u32; 4]), Open(u32), ConfirmTitle, ReturnToTitle, SystemVolume, Info(u32) }

#[derive(Default)]
pub(super) struct WindowControlState {
    pub(super) enabled: bool,
    pub(super) fes: super::fes_window::FesWindowState,
    commands: VecDeque<PostedWindowCommand>,
    requests: VecDeque<HostWindowRequest>,
    in_flight: Option<HostWindowRequest>,
    notifying_close: bool,
    notifying_menu: bool,
    opening_menu: bool,
    pub(super) ui: super::settings_menu_native::SettingsMenuState,
}

impl LoggingHost<'_> {
    pub(super) fn post_fes_window_menu(&mut self, id: u32) -> Result<()> {
        let command = match id {
            0 => PostedWindowCommand::ConfirmTitle,
            1 => PostedWindowCommand::Open(40045),
            2 => PostedWindowCommand::Close,
            10 => PostedWindowCommand::SetWindowed(0),
            11 => PostedWindowCommand::SetWindowed(1),
            12 => PostedWindowCommand::Minimize,
            13 => PostedWindowCommand::Notify([0, 20, 0, 0]),
            20 => PostedWindowCommand::Settings,
            21 => PostedWindowCommand::GeneralSettings,
            22 => PostedWindowCommand::SystemVolume,
            30..=32 => PostedWindowCommand::Info(id),
            _ => return Ok(()),
        };
        let state = &mut self.data_services.window_control;
        if !state.enabled && id != 13 { return Ok(()); }
        let notify = matches!(id, 0 | 22);
        if state.commands.len() + state.requests.len() + usize::from(notify) >= 4096 {
            self.events.push(LogicEvent { frame: self.frame, kind: "window-command-unavailable".into(),
                detail: format!("FES window menu {id}: posted command queue is full") });
            return Ok(());
        }
        if notify { state.commands.push_back(PostedWindowCommand::Notify([0, 159, 0, 1])); }
        state.commands.push_back(command);
        Ok(())
    }

    pub(super) fn post_title_return(&mut self) -> cs2_script::Result<()> {
        let state = &mut self.data_services.window_control;
        if state.commands.len() + state.requests.len() >= 4096 {
            self.events.push(LogicEvent { frame: self.frame, kind: "window-command-unavailable".into(),
                detail: "FES return to title: posted command queue is full".into() });
            return Ok(());
        }
        // The FES command posts the return request for delivery after the frame.
        state.commands.push_back(PostedWindowCommand::ReturnToTitle);
        Ok(())
    }

    pub(super) fn post_effect_settings(&mut self) -> cs2_script::Result<()> {
        self.window_control_native(457, &[])?;
        Ok(())
    }

    pub(super) fn window_control_native(
        &mut self, index: u32, arguments: &[u8],
    ) -> cs2_script::Result<NativeReturn> {
        let command = match index {
            457 => PostedWindowCommand::Settings,
            628 => PostedWindowCommand::Menu,
            845 => PostedWindowCommand::ToggleFullscreen,
            846 => PostedWindowCommand::Minimize,
            847 => PostedWindowCommand::Close,
            _ => return Err(ScriptError::UnimplementedNativeCall { index }),
        };
        if arguments.len() != if index == 628 { 4 } else { 0 } {
            return Err(native_state_error("window command argument byte count mismatch"));
        }
        let state = &mut self.data_services.window_control;
        if index == 628 {
            // The shared raw DWORD is written before PostMessage, including
            // failure to post. The receiver reads the latest value at delivery.
            state.ui.mode = u32::from_le_bytes(arguments.try_into().unwrap());
        }
        if !state.enabled {
            self.events.push(LogicEvent { frame: self.frame, kind: "window-command-unavailable".into(),
                detail: format!("native {index} needs an attached platform window") });
            return Ok(NativeReturn::Continue);
        }
        if state.commands.len() + state.requests.len() >= 4096 {
            self.events.push(LogicEvent { frame: self.frame, kind: "window-command-unavailable".into(),
                detail: format!("native {index}: posted command queue is full") });
            return Ok(NativeReturn::Continue);
        }
        state.commands.push_back(command);
        // The pinned wrappers post a message, ignore its BOOL and return 2.
        Ok(NativeReturn::Continue)
    }
}

#[test]
fn exhausted_window_queue_records_the_failed_posts_and_retains_queued_work() -> Result<()> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut player = HeadlessPlayer::open(root)?;
    player.with_host(|host| {
        let state = &mut host.data_services.window_control;
        state.enabled = true;
        state.commands.resize(4096, PostedWindowCommand::Minimize);

        assert!(host.window_control_native(628, &[]).is_err());
        assert_eq!(host.window_control_native(628, &5_u32.to_le_bytes())?, NativeReturn::Continue);
        host.post_fes_window_menu(0)?;
        host.post_title_return()?;

        let state = &host.data_services.window_control;
        assert_eq!(state.ui.mode, 5);
        assert_eq!(state.commands.len(), 4096);
        assert!(state.commands.iter().all(|command| *command == PostedWindowCommand::Minimize));
        assert_eq!(host.events.iter().filter(|event| event.kind == "window-command-unavailable").count(), 3);
        Ok(())
    })
}

impl HeadlessPlayer {
    pub(super) fn require_window_request(&self, request: HostWindowRequest) -> Result<()> {
        if self.data_services.window_control.in_flight != Some(request) {
            bail!("host UI operation does not match pending ordered window request");
        }
        Ok(())
    }

    pub(super) fn finish_window_ui_request(&mut self, request: HostWindowRequest) -> Result<()> {
        self.require_window_request(request)?;
        self.data_services.window_control.in_flight = None;
        Ok(())
    }

    pub(super) fn queue_menu_rebuild(&mut self) -> Result<()> {
        let state = &mut self.data_services.window_control;
        if state.commands.len() + state.requests.len() >= 4096 { bail!("window command queue is full"); }
        state.commands.push_back(PostedWindowCommand::Menu);
        Ok(())
    }

    pub(super) fn queue_menu_window_mode(&mut self, windowed: u32) -> Result<()> {
        let state = &mut self.data_services.window_control;
        if !state.enabled { bail!("menu display command needs an attached platform window"); }
        if state.commands.len() + state.requests.len() >= 4096 { bail!("window command queue is full"); }
        state.commands.push_back(PostedWindowCommand::SetWindowed(windowed));
        Ok(())
    }

    pub(super) fn queue_menu_general_settings(&mut self) -> Result<()> {
        let state = &mut self.data_services.window_control;
        if !state.enabled { bail!("configuration menu needs an attached platform window"); }
        if state.commands.len() + state.requests.len() >= 4096 { bail!("window command queue is full"); }
        state.commands.push_back(PostedWindowCommand::GeneralSettings);
        Ok(())
    }

    pub(super) fn queue_menu_task_message(&mut self, packet: [u32; 4]) -> Result<()> {
        let state = &mut self.data_services.window_control;
        if !state.enabled { bail!("menu task notification needs an attached platform window"); }
        if state.commands.len() + state.requests.len() >= 4096 { bail!("window command queue is full"); }
        state.commands.push_back(PostedWindowCommand::Notify(packet));
        Ok(())
    }
    pub(super) fn queue_menu_open(&mut self, command: u32) -> Result<()> {
        let state = &mut self.data_services.window_control;
        if !state.enabled { bail!("menu open needs an attached platform window"); }
        if state.commands.len() + state.requests.len() >= 4096 { bail!("window command queue is full"); }
        state.commands.push_back(PostedWindowCommand::Open(command));
        Ok(())
    }
    pub(super) fn queue_menu_title_return(&mut self) -> Result<()> {
        let state = &mut self.data_services.window_control;
        if !state.enabled { bail!("title-return confirmation needs an attached platform window"); }
        if state.commands.len() + state.requests.len() > 4094 { bail!("window command queue cannot hold title-return notification and confirmation"); }
        //49138c→491c60 sends159 before MessageBox, including a later No.
        state.commands.push_back(PostedWindowCommand::Notify([0, 159, 0, 1]));
        state.commands.push_back(PostedWindowCommand::ConfirmTitle);
        Ok(())
    }
    pub(super) fn accept_menu_title_return(&mut self) {
        // Continue at4913cc before commands posted while the modal UI was open.
        self.data_services.window_control.commands.push_front(PostedWindowCommand::ReturnToTitle);
    }
    pub(super) fn queue_menu_system_volume(&mut self) -> Result<()> {
        let state = &mut self.data_services.window_control;
        if !state.enabled { bail!("system-volume menu needs an attached platform window"); }
        if state.commands.len() + state.requests.len() > 4094 { bail!("window queue cannot hold system-volume notification and receiver"); }
        //49154f sends159 before523230 submits the OS mixer request.
        state.commands.push_back(PostedWindowCommand::Notify([0, 159, 0, 1]));
        state.commands.push_back(PostedWindowCommand::SystemVolume);
        Ok(())
    }
    pub fn enable_host_window_requests(&mut self) {
        self.data_services.window_control.enabled = true;
    }

    pub fn take_host_window_request(&mut self) -> Option<HostWindowRequest> {
        let state = &mut self.data_services.window_control;
        if state.in_flight.is_some() { return None; }
        let request = state.requests.pop_front()?;
        state.in_flight = Some(request);
        Some(request)
    }

    pub fn complete_host_window_request(&mut self, windowed: Option<u32>) -> Result<()> {
        let request = self.data_services.window_control.in_flight
            .ok_or_else(|| anyhow::anyhow!("no platform window request is awaiting completion"))?;
        match (request, windowed) {
            (HostWindowRequest::ToggleFullscreen, Some(mode @ 0..=1)) => self.set_host_windowed_mode(mode),
            (HostWindowRequest::Minimize, None) => {}
            _ => bail!("platform window result does not match its pending command"),
        }
        self.data_services.window_control.in_flight = None;
        Ok(())
    }

    /// A posted close first sends the original sender=0, code=159, data=(0,1)
    /// packet. Keep both the command and its VM dispatch across budget yields.
    pub(super) fn advance_posted_window_commands(&mut self, budget: usize) -> Result<bool> {
        if self.frame_in_progress() { return Ok(true); }
        if !self.data_services.window_control.requests.is_empty()
            || self.data_services.window_control.in_flight.is_some()
        { return Ok(false); }
        for _ in 0..4096 {
            let Some(command) = self.data_services.window_control.commands.front().copied()
                else { return Ok(true); };
            match command {
                PostedWindowCommand::Info(command) => {
                    self.prepare_fes_menu_info(command)?;
                    self.data_services.window_control.requests.push_back(HostWindowRequest::MenuInfo);
                    self.data_services.window_control.commands.pop_front();
                    return Ok(false);
                }
                PostedWindowCommand::SystemVolume => {
                    self.prepare_system_volume()?;
                    self.data_services.window_control.requests.push_back(HostWindowRequest::SystemVolume);
                    self.data_services.window_control.commands.pop_front();
                    return Ok(false);
                }
                PostedWindowCommand::ConfirmTitle => {
                    self.prepare_menu_confirmation()?;
                    self.data_services.window_control.requests.push_back(HostWindowRequest::MenuConfirmation);
                    self.data_services.window_control.commands.pop_front();
                    return Ok(false);
                }
                PostedWindowCommand::Open(command) => {
                    let done = if self.data_services.window_control.opening_menu { self.poll_window_system_open()? }
                        else { match self.prepare_menu_open(command) {
                            Ok(request) => self.begin_window_system_open(request, if command == 118 { 1 } else { 5 })?,
                            Err(error) => {
                                self.events.push(LogicEvent { frame: self.state.clock.frame, kind: "menu-open-failed".into(),
                                    detail: format!("command{command}: {error:#}; no platform open submitted") });
                                true
                            }
                        } };
                    self.data_services.window_control.opening_menu = !done;
                    if !done { return Ok(false); }
                }
                PostedWindowCommand::ReturnToTitle => {
                    if self.loaded.is_none() { bail!("title return requires the actual main task"); }
                    self.with_host(|host| Ok(host.broadcast_title_return()?))?;
                    self.events.push(LogicEvent {
                        frame: self.state.clock.frame, kind: "title-return".into(),
                        detail: "queued task command 21 throughout the main task tree".into(),
                    });
                }
                PostedWindowCommand::Notify(packet) => {
                    if self.loaded.is_none() && packet == [0, 20, 0, 0] {
                        self.data_services.window_control.commands.pop_front();
                        continue;
                    }
                    if self.loaded.is_none() { bail!("menu notification requires the actual main task"); }
                    if self.data_services.window_control.notifying_menu
                        && !self.service_pending_root_dispatch(budget)?
                    { return Ok(false); }
                    let root = self.current_task_id;
                    let done = self.with_host(|host| {
                        Ok(host.message_branch(root, packet, false, &mut 0)?)
                    })?;
                    if !done {
                        self.data_services.window_control.notifying_menu = true;
                        return Ok(false);
                    }
                    self.data_services.window_control.notifying_menu = false;
                    self.events.push(LogicEvent { frame: self.state.clock.frame,
                        kind: "menu-task-notification".into(),
                        detail: format!("main task {root}, sender={}, code={}, data=({}, {}) delivered", packet[0], packet[1], packet[2], packet[3]) });
                }
                PostedWindowCommand::SetWindowed(target) => {
                    let actual = self.data_services.fes_settings.windowed_mode
                        .ok_or_else(|| anyhow::anyhow!("menu display command has no actual window mode"))?;
                    if actual != target {
                        self.data_services.window_control.requests.push_back(HostWindowRequest::ToggleFullscreen);
                        self.data_services.window_control.commands.pop_front();
                        return Ok(false);
                    }
                }
                PostedWindowCommand::Settings | PostedWindowCommand::GeneralSettings | PostedWindowCommand::Menu => {
                    let request = if command == PostedWindowCommand::Settings {
                        if !self.prepare_effect_settings()? {
                            self.data_services.window_control.commands.pop_front();
                            continue;
                        }
                        HostWindowRequest::Settings
                    } else if command == PostedWindowCommand::GeneralSettings {
                        self.prepare_general_settings()?;
                        HostWindowRequest::Settings
                    } else {
                        self.prepare_game_menu()?;
                        HostWindowRequest::Menu
                    };
                    self.data_services.window_control.requests.push_back(request);
                    self.data_services.window_control.commands.pop_front();
                    return Ok(false);
                }
                PostedWindowCommand::Close => {
                    if self.loaded.is_some() {
                        if self.data_services.window_control.notifying_close
                            && !self.service_pending_root_dispatch(budget)?
                        { return Ok(false); }
                        let root = self.current_task_id;
                        let done = self.with_host(|host| {
                            Ok(host.message_branch(root, [0, 159, 0, 1], false, &mut 0)?)
                        })?;
                        if !done {
                            self.data_services.window_control.notifying_close = true;
                            return Ok(false);
                        }
                    }
                    self.data_services.window_control.notifying_close = false;
                    self.data_services.host_quit_requested = true;
                    self.events.push(LogicEvent {
                        frame: self.state.clock.frame, kind: "window-close-request".into(),
                        detail: "posted command: task notification completed".into(),
                    });
                }
                PostedWindowCommand::ToggleFullscreen => {
                    self.data_services.window_control.requests.push_back(HostWindowRequest::ToggleFullscreen);
                    self.data_services.window_control.commands.pop_front();
                    return Ok(false);
                }
                PostedWindowCommand::Minimize => {
                    self.data_services.window_control.requests.push_back(HostWindowRequest::Minimize);
                    self.data_services.window_control.commands.pop_front();
                    return Ok(false);
                }
            }
            self.data_services.window_control.commands.pop_front();
        }
        if self.data_services.window_control.commands.is_empty() { return Ok(true); }
        bail!("posted window command budget exhausted")
    }
}

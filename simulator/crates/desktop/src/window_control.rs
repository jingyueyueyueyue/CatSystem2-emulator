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
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PostedWindowCommand { ToggleFullscreen, SetWindowed(u32), Minimize, Close, Settings, GeneralSettings, Menu, Notify([u32; 4]), Open(u32), ConfirmTitle, ReturnToTitle, SystemVolume }

#[derive(Default)]
pub(super) struct WindowControlState {
    enabled: bool,
    commands: VecDeque<PostedWindowCommand>,
    requests: VecDeque<HostWindowRequest>,
    in_flight: Option<HostWindowRequest>,
    notifying_close: bool,
    notifying_menu: bool,
    opening_menu: bool,
    pub(super) ui: super::settings_menu_native::SettingsMenuState,
}

impl LoggingHost<'_> {
    pub(super) fn post_title_return(&mut self) -> cs2_script::Result<()> {
        let state = &mut self.data_services.window_control;
        if state.commands.len() + state.requests.len() >= 4096 {
            return Err(native_state_error("posted window command queue exceeds 4096 entries"));
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
            return Err(native_state_error("window command needs an attached platform window"));
        }
        if state.commands.len() + state.requests.len() >= 4096 {
            return Err(native_state_error("posted window command queue exceeds 4096 entries"));
        }
        state.commands.push_back(command);
        // The pinned wrappers post a message, ignore its BOOL and return 2.
        Ok(NativeReturn::Continue)
    }
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
                        self.prepare_effect_settings()?;
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

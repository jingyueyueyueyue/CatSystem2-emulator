//! Ordinary629: synchronous platform-open submission, with raw script bytes.
//! No process is launched while parsing or constructing the player.
use cs2_script::{NativeReturn, Result, Vm};
use std::collections::BTreeMap;
use std::path::PathBuf;

use super::{HeadlessPlayer, LoggingHost, ScriptTextEncoding, expand_kcs_format,
    native_state_error, read_u32, trim_nul};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SystemOpenRequest {
    pub task_id: u32,
    pub target: Vec<u8>,
    pub parameters: Vec<u8>,
    pub directory: Vec<u8>,
    pub encoding: ScriptTextEncoding,
}

/// Return only after the OS request has actually been submitted. An Android
/// shell can use an ACTION_VIEW/explicit activity bridge and diagnose failures.
pub type SystemOpenHandler = fn(&SystemOpenRequest) -> anyhow::Result<()>;

#[derive(Clone, Debug)]
pub struct PendingSystemOpen {
    pub id: u64,
    pub request: SystemOpenRequest,
    pub game_root: PathBuf,
}

struct QueuedOpen {
    request: PendingSystemOpen,
    delivered: bool,
    result: Option<std::result::Result<(), String>>,
}

#[derive(Default)]
pub(super) struct SystemOpenState {
    handler: Option<SystemOpenHandler>,
    queue_root: Option<PathBuf>,
    next_id: u64,
    pending: BTreeMap<(u32, usize, usize, usize), QueuedOpen>,
    // Window/menu requests have no VM instruction identity. Keep a real
    // independent receipt, using the same host consumer and identity counter.
    window: Option<QueuedOpen>,
}

impl HeadlessPlayer {
    pub fn set_system_open_handler(&mut self, handler: SystemOpenHandler) {
        self.data_services.system_open.handler = Some(handler);
    }

    pub fn enable_host_system_open_queue(&mut self) {
        self.data_services.system_open.queue_root = Some(self.root.path.clone());
    }

    pub fn take_host_system_open_request(&mut self) -> Option<PendingSystemOpen> {
        if let Some(pending) = self.data_services.system_open.window.as_mut().filter(|item| !item.delivered) {
            pending.delivered = true;
            return Some(pending.request.clone());
        }
        let pending = self.data_services.system_open.pending.values_mut()
            .find(|pending| !pending.delivered)?;
        pending.delivered = true;
        Some(pending.request.clone())
    }

    pub fn complete_host_system_open(
        &mut self, id: u64, result: std::result::Result<(), String>,
    ) -> anyhow::Result<()> {
        if let Some(pending) = self.data_services.system_open.window.as_mut().filter(|item| item.request.id == id) {
            if !pending.delivered || pending.result.is_some() { anyhow::bail!("window open is not awaiting a host result"); }
            pending.result = Some(result);
            return Ok(());
        }
        let pending = self.data_services.system_open.pending.values_mut()
            .find(|pending| pending.request.id == id)
            .ok_or_else(|| anyhow::anyhow!("system-open result has no matching request"))?;
        if !pending.delivered || pending.result.is_some() {
            anyhow::bail!("system-open request is not awaiting a host result");
        }
        pending.result = Some(result);
        Ok(())
    }

    pub(super) fn begin_window_system_open(&mut self, request: SystemOpenRequest, show: i32) -> anyhow::Result<bool> {
        let state = &mut self.data_services.system_open;
        if state.window.is_some() { anyhow::bail!("another menu open request is awaiting a host receipt"); }
        if let Some(root) = state.queue_root.clone() {
            state.next_id = state.next_id.checked_add(1).ok_or_else(|| anyhow::anyhow!("system-open identities exhausted"))?;
            state.window = Some(QueuedOpen { request: PendingSystemOpen { id: state.next_id, request, game_root: root },
                delivered: false, result: None });
            return Ok(false);
        }
        let result = if let Some(handler) = state.handler { handler(&request) }
            else { platform_open_window(&request, show) };
        self.record_window_open_result(result.map_err(|error| format!("{error:#}")));
        Ok(true)
    }

    pub(super) fn poll_window_system_open(&mut self) -> anyhow::Result<bool> {
        let state = &mut self.data_services.system_open;
        let Some(pending) = &mut state.window else { anyhow::bail!("menu open receipt disappeared"); };
        let Some(result) = pending.result.take() else { return Ok(false); };
        state.window = None;
        self.record_window_open_result(result);
        Ok(true)
    }

    fn record_window_open_result(&mut self, result: std::result::Result<(), String>) {
        // Both original menu paths discard ShellExecute's return. Report a
        // real host failure and finish this command; do not replay the launch.
        self.events.push(super::LogicEvent { frame: self.state.clock.frame,
            kind: if result.is_ok() { "menu-open-submitted" } else { "menu-open-failed" }.into(),
            detail: result.err().unwrap_or_else(|| "actual platform open accepted".into()) });
    }
}

#[cfg(windows)]
fn platform_open_window(request: &SystemOpenRequest, show: i32) -> anyhow::Result<()> {
    use std::ffi::c_void;
    #[link(name = "user32")]
    unsafe extern "system" { fn GetActiveWindow() -> *mut c_void; }
    #[link(name = "shell32")]
    unsafe extern "system" {
        fn ShellExecuteW(window: *mut c_void, operation: *const u16, target: *const u16,
            parameters: *const u16, directory: *const u16, show: i32) -> isize;
    }
    let wide = |bytes: &[u8]| request.encoding.decode(bytes).encode_utf16().chain(Some(0)).collect::<Vec<_>>();
    let target = wide(&request.target); let parameters = wide(&request.parameters); let directory = wide(&request.directory);
    let open = [b'o' as u16, b'p' as u16, b'e' as u16, b'n' as u16, 0];
    let result = unsafe { ShellExecuteW(GetActiveWindow(), open.as_ptr(), target.as_ptr(),
        parameters.as_ptr(), directory.as_ptr(), show) };
    if result <= 32 { anyhow::bail!("menu ShellExecuteW failed with code{result}"); }
    Ok(())
}
#[cfg(not(windows))]
fn platform_open_window(_: &SystemOpenRequest, _: i32) -> anyhow::Result<()> {
    anyhow::bail!("menu open requires an actual platform consumer")
}

impl LoggingHost<'_> {
    pub(super) fn system_open_native(&mut self, args: &[u8], vm: &Vm) -> Result<NativeReturn> {
        // An input handler can interrupt the same task's waiting body. Keep
        // its distinct native frame from consuming the body's host receipt.
        let invocation = (self.current_task_id, vm.instruction_pointer, vm.frame_base(), vm.stack_pointer());
        if let Some(pending) = self.data_services.system_open.pending.get_mut(&invocation) {
            let Some(result) = pending.result.take() else { return Ok(NativeReturn::Retry); };
            self.data_services.system_open.pending.remove(&invocation);
            return match result {
                Ok(()) => Ok(NativeReturn::Continue),
                Err(cause) => {
                    self.events.push(super::LogicEvent { frame: self.frame, kind: "system-open-error".into(),
                        detail: cause });
                    Err(native_state_error("native 629 platform open failed; see system-open-error"))
                }
            };
        }
        if args.len() != 12 {
            return Err(native_state_error("native 629 requires three address DWORDs"));
        }
        // Keep the original left-to-right evaluation and first-NUL boundary.
        let mut values = Vec::with_capacity(3);
        for offset in [0, 4, 8] {
            let bytes = expand_kcs_format(vm, &vm.read_string(read_u32(args, offset)?)?, 0)?;
            let text = trim_nul(&bytes);
            let capacity = if offset == 0 { 4096 } else { 260 };
            if text.len() >= capacity {
                return Err(native_state_error("native 629 formatted field exceeds its original buffer"));
            }
            values.push(text.to_vec());
        }
        let request = SystemOpenRequest {
            task_id: self.current_task_id, target: values.remove(0),
            parameters: values.remove(0), directory: values.remove(0), encoding: self.text_encoding,
        };
        let target = request.encoding.decode(&request.target).replace('\\', "/");
        if !target.contains("://")
            && target.rsplit('/').next().and_then(|name| name.rsplit_once('.'))
                .is_some_and(|(_, extension)| matches!(extension.to_ascii_lowercase().as_str(),
                    "exe" | "dll" | "com" | "bat" | "cmd" | "msi" | "scr" | "pif" | "lnk"))
        {
            return Err(native_state_error("native 629 cannot execute original Windows programs or launchers"));
        }
        if let Some(handler) = self.data_services.system_open.handler {
            handler(&request).map_err(|cause| {
                self.events.push(super::LogicEvent { frame: self.frame, kind: "system-open-error".into(),
                    detail: cause.to_string() });
                native_state_error("native 629 platform open failed; see system-open-error")
            })?;
        } else if let Some(root) = self.data_services.system_open.queue_root.clone() {
            let state = &mut self.data_services.system_open;
            if state.pending.len() >= 64 {
                return Err(native_state_error("system-open queue exceeds 64 requests"));
            }
            let id = state.next_id.checked_add(1)
                .ok_or_else(|| native_state_error("system-open request identities exhausted"))?;
            state.next_id = id;
            state.pending.insert(invocation, QueuedOpen {
                request: PendingSystemOpen { id, request, game_root: root },
                delivered: false, result: None,
            });
            return Ok(NativeReturn::Retry);
        } else {
            platform_open(&request)?;
        }
        Ok(NativeReturn::Continue)
    }
}

#[cfg(windows)]
fn platform_open(request: &SystemOpenRequest) -> Result<()> {
    use std::ffi::{CString, c_void};
    #[link(name = "user32")]
    unsafe extern "system" { fn GetActiveWindow() -> *mut c_void; }
    #[link(name = "shell32")]
    unsafe extern "system" {
        fn ShellExecuteA(window: *mut c_void, operation: *const u8, target: *const u8,
            parameters: *const u8, directory: *const u8, show: i32) -> *mut c_void;
    }
    let strings = [&request.target, &request.parameters, &request.directory].map(|bytes| {
        CString::new(bytes.as_slice()).map_err(|_| native_state_error("native 629 contains an inner NUL"))
    });
    let [target, parameters, directory] = strings;
    let (target, parameters, directory) = (target?, parameters?, directory?);
    // The baseline discards ShellExecuteA's success/error value. A configured
    // portable handler can instead return a service/protocol diagnostic.
    unsafe {
        ShellExecuteA(GetActiveWindow(), c"open".as_ptr().cast(), target.as_ptr().cast(),
            parameters.as_ptr().cast(), directory.as_ptr().cast(), 5);
    }
    Ok(())
}

#[cfg(not(windows))]
fn platform_open(_request: &SystemOpenRequest) -> Result<()> {
    Err(native_state_error("native 629 requires a synchronous platform open handler"))
}

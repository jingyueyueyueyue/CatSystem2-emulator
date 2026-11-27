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

#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum OpenInvocation {
    Kcs(u32, usize, usize, usize),
    Fes(u32, usize),
}

#[derive(Default)]
pub(super) struct SystemOpenState {
    handler: Option<SystemOpenHandler>,
    queue_root: Option<PathBuf>,
    next_id: u64,
    pending: BTreeMap<OpenInvocation, QueuedOpen>,
    // Window/menu requests have no VM instruction identity. Keep a real
    // independent receipt, using the same host consumer and identity counter.
    window: Option<QueuedOpen>,
}

impl SystemOpenState {
    fn enqueue(&mut self, invocation: OpenInvocation, request: SystemOpenRequest, root: PathBuf)
        -> std::result::Result<(), &'static str>
    {
        if self.pending.len() >= 64 {
            return Err("system-open queue exceeds 64 requests");
        }
        let id = self.next_id.checked_add(1).ok_or("system-open request identities exhausted")?;
        self.next_id = id;
        self.pending.insert(invocation, QueuedOpen {
            request: PendingSystemOpen { id, request, game_root: root },
            delivered: false, result: None,
        });
        Ok(())
    }

    pub(super) fn release_fes_owner(&mut self, owner: u32) {
        self.pending.retain(|key, _| !matches!(key, OpenInvocation::Fes(id, _) if *id == owner));
    }
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

fn fes_open_target(
    args: &[Vec<u8>], encode_query: bool, encoding: ScriptTextEncoding,
) -> std::result::Result<Vec<u8>, &'static str> {
    let [scheme, address, trailing @ ..] = args else {
        return Err("FES opensite requires a protocol and address");
    };
    if !matches!(scheme.as_slice(), b"http" | b"https")
        || address.is_empty()
        || address.iter().any(|byte| byte.is_ascii_control() || byte.is_ascii_whitespace())
    {
        return Err("FES opensite received an invalid HTTP address");
    }
    let mut target = [scheme.as_slice(), b"://", address.as_slice()].concat();
    if !encode_query {
        if !trailing.is_empty() {
            return Err("FES opensite requires a protocol and address");
        }
        return Ok(target);
    }
    let Some(query_start) = address.iter().position(|&byte| byte == b'?') else {
        if !trailing.is_empty() {
            return Err("FES opensiteenc has query arguments without a query delimiter");
        }
        return Ok(target);
    };
    let mut query = address[query_start + 1..].to_vec();
    for part in trailing {
        // FES scripts spell nested links as "http: example.org" to keep
        // the double slash out of unquoted command parameters.
        if query.ends_with(b"=http:") || query.ends_with(b"=https:") {
            query.extend_from_slice(b"//");
        } else if !query.is_empty() && !query.ends_with(b"&") {
            query.push(b' ');
        }
        query.extend_from_slice(part);
    }
    // The shipped encoded-link templates retain CP932 even in the GBK
    // resource archive. UTF-8 parameters also occur through string expansion.
    let query = std::str::from_utf8(&query).ok().map(std::borrow::Cow::Borrowed)
        .or_else(|| ScriptTextEncoding::Cp932.decode_strict(&query))
        .unwrap_or_else(|| std::borrow::Cow::Owned(encoding.decode(&query)));
    target.truncate(scheme.len() + 3 + query_start + 1);
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut in_value = false;
    for byte in query.bytes() {
        match byte {
            b'&' => {
                in_value = false;
                target.push(byte);
            }
            b'=' if !in_value => {
                in_value = true;
                target.push(byte);
            }
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => target.push(byte),
            _ => target.extend_from_slice(&[b'%', HEX[(byte >> 4) as usize], HEX[(byte & 15) as usize]]),
        }
    }
    Ok(target)
}

impl LoggingHost<'_> {
    fn record_system_open(&mut self, result: std::result::Result<(), String>) {
        if let Err(detail) = result {
            self.events.push(super::LogicEvent { frame: self.frame, kind: "system-open-error".into(), detail });
        }
    }

    fn finish_system_open(&mut self, result: std::result::Result<(), String>) -> NativeReturn {
        self.record_system_open(result);
        // Ordinary 629 has no script-visible return value.
        NativeReturn::Continue
    }

    pub(super) fn fes_open_site(&mut self, owner: u32, offset: usize, args: &[Vec<u8>], encode_query: bool) -> bool {
        let invocation = OpenInvocation::Fes(owner, offset);
        if let Some(pending) = self.data_services.system_open.pending.get_mut(&invocation) {
            let Some(result) = pending.result.take() else { return false; };
            self.data_services.system_open.pending.remove(&invocation);
            self.record_system_open(result);
            return true;
        }
        let target = match fes_open_target(args, encode_query, self.text_encoding) {
            Ok(target) => target,
            Err(error) => {
                self.record_system_open(Err(error.into()));
                return true;
            }
        };
        let request = SystemOpenRequest {
            task_id: owner, target, parameters: Vec::new(), directory: Vec::new(),
            encoding: self.text_encoding,
        };
        let state = &mut self.data_services.system_open;
        let result = if let Some(handler) = state.handler {
            handler(&request).map_err(|error| format!("{error:#}"))
        } else if let Some(root) = state.queue_root.clone() {
            match state.enqueue(invocation, request, root) {
                Ok(()) => return false,
                Err(error) => Err(error.to_owned()),
            }
        } else {
            platform_open_window(&request, 5).map_err(|error| format!("{error:#}"))
        };
        self.record_system_open(result);
        true
    }

    pub(super) fn system_open_native(&mut self, args: &[u8], vm: &Vm) -> Result<NativeReturn> {
        // An input handler can interrupt the same task's waiting body. Keep
        // its distinct native frame from consuming the body's host receipt.
        let invocation = OpenInvocation::Kcs(
            self.current_task_id, vm.instruction_pointer, vm.frame_base(), vm.stack_pointer(),
        );
        if let Some(pending) = self.data_services.system_open.pending.get_mut(&invocation) {
            let Some(result) = pending.result.take() else { return Ok(NativeReturn::Retry); };
            self.data_services.system_open.pending.remove(&invocation);
            return Ok(self.finish_system_open(result));
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
            return Ok(self.finish_system_open(Err("native 629 cannot execute original Windows programs or launchers".into())));
        }
        if let Some(handler) = self.data_services.system_open.handler {
            return Ok(self.finish_system_open(handler(&request).map_err(|cause| format!("{cause:#}"))));
        } else if let Some(root) = self.data_services.system_open.queue_root.clone() {
            return Ok(match self.data_services.system_open.enqueue(invocation, request, root) {
                Ok(()) => NativeReturn::Retry,
                Err(error) => self.finish_system_open(Err(error.to_owned())),
            });
        }
        Ok(self.finish_system_open(platform_open(&request).map_err(|cause| cause.to_string())))
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
    let result = unsafe {
        ShellExecuteA(GetActiveWindow(), c"open".as_ptr().cast(), target.as_ptr().cast(),
            parameters.as_ptr().cast(), directory.as_ptr().cast(), 5)
    } as isize;
    if result <= 32 {
        return Err(super::native_state_error_owned(format!("ShellExecuteA failed with code {result}")));
    }
    Ok(())
}

#[cfg(not(windows))]
fn platform_open(_request: &SystemOpenRequest) -> Result<()> {
    Err(native_state_error("native 629 requires a synchronous platform open handler"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_open_and_dialog_resume_after_rejected_host_receipts() -> anyhow::Result<()> {
        let path = std::env::temp_dir().join(format!("cs2-host-receipts-{}-{}", std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos()));
        std::fs::create_dir(&path)?;
        let player = HeadlessPlayer::open_with_save_root(&path, &path);
        std::fs::remove_dir(path)?;
        let mut player = player?;
        player.enable_host_system_open_queue();
        player.enable_host_dialog_queue();

        let program = cs2_script::KcsProgram::from_code(&[0x3b], Vec::new(), 0)?;
        let mut vm = Vm::from_program(program)?;
        let target = vm.allocate_region(b"https://example.invalid/\0")?;
        let empty = vm.allocate_region(b"\0")?;
        let text = vm.allocate_region(b"Host dialog\0")?;
        let open_args: Vec<_> = [target, empty, empty].into_iter().flat_map(u32::to_le_bytes).collect();
        let open = |player: &mut HeadlessPlayer| player.with_host(|host| Ok(host.system_open_native(&open_args, &vm)?));
        let dialog = |player: &mut HeadlessPlayer| player.with_host(|host| Ok(host.system_dialog_native(&text.to_le_bytes(), &vm)?));
        assert!(matches!(open(&mut player)?, NativeReturn::Retry));
        let request = player.take_host_system_open_request().unwrap();
        assert!(matches!(open(&mut player)?, NativeReturn::Retry));
        player.complete_host_system_open(request.id, Err("no browser".into()))?;
        assert!(matches!(open(&mut player)?, NativeReturn::Continue));
        assert!(player.take_host_system_open_request().is_none());
        assert!(player.events().iter().any(|event| event.kind == "system-open-error" && event.detail == "no browser"));
        assert!(matches!(dialog(&mut player)?, NativeReturn::Retry));
        let request = player.take_host_dialog_request().unwrap();
        player.complete_host_dialog(request.id, Err("dialog unavailable".into()))?;
        assert!(matches!(dialog(&mut player)?, NativeReturn::Continue));
        assert!(player.take_host_dialog_request().is_none());
        assert!(player.events().iter().any(|event| event.kind == "system-dialog-error" && event.detail == "dialog unavailable"));
        Ok(())
    }
}

//! Ordinary445: one owned system-error dialog, completed by a real host UI.
use std::collections::BTreeMap;
use cs2_script::{NativeReturn, Result, Vm};
use super::{HeadlessPlayer, LoggingHost, expand_kcs_format, native_state_error, read_u32};

#[derive(Clone, Debug)]
pub struct HostDialogRequest {
    pub id: u64,
    pub title: String,
    pub text: String,
}

struct PendingDialog {
    request: HostDialogRequest,
    delivered: bool,
    result: Option<std::result::Result<(), String>>,
}

#[derive(Default)]
pub(super) struct DialogState {
    queued: bool,
    next_id: u64,
    pending: BTreeMap<(u32, usize, usize, usize), PendingDialog>,
}

impl HeadlessPlayer {
    pub fn enable_host_dialog_queue(&mut self) { self.data_services.dialogs.queued = true; }

    pub fn take_host_dialog_request(&mut self) -> Option<HostDialogRequest> {
        let pending = self.data_services.dialogs.pending.values_mut().find(|item| !item.delivered)?;
        pending.delivered = true;
        Some(pending.request.clone())
    }

    pub fn complete_host_dialog(
        &mut self, id: u64, result: std::result::Result<(), String>,
    ) -> anyhow::Result<()> {
        let pending = self.data_services.dialogs.pending.values_mut()
            .find(|item| item.request.id == id)
            .ok_or_else(|| anyhow::anyhow!("dialog result has no matching request"))?;
        if !pending.delivered || pending.result.is_some() {
            anyhow::bail!("dialog is not awaiting a host result");
        }
        pending.result = Some(result);
        Ok(())
    }
}

impl LoggingHost<'_> {
    pub(super) fn system_dialog_native(&mut self, args: &[u8], vm: &Vm) -> Result<NativeReturn> {
        let key = (self.current_task_id, vm.instruction_pointer, vm.frame_base(), vm.stack_pointer());
        if let Some(pending) = self.data_services.dialogs.pending.get_mut(&key) {
            let Some(result) = pending.result.take() else { return Ok(NativeReturn::Retry); };
            self.data_services.dialogs.pending.remove(&key);
            return result.map(|()| NativeReturn::Continue)
                .map_err(|_| native_state_error("native 445 platform dialog failed"));
        }
        let text = expand_kcs_format(vm, &vm.read_string(read_u32(args, 0)?)?, 0)?;
        if text.len() >= 4096 {
            return Err(native_state_error("native 445 text exceeds the original format buffer"));
        }
        let text = self.text_encoding.decode(super::trim_nul(&text));
        if self.data_services.dialogs.queued {
            let state = &mut self.data_services.dialogs;
            if state.pending.len() >= 64 {
                return Err(native_state_error("platform dialog queue exceeds 64 requests"));
            }
            let id = state.next_id.checked_add(1)
                .ok_or_else(|| native_state_error("platform dialog identities exhausted"))?;
            state.next_id = id;
            state.pending.insert(key, PendingDialog {
                request: HostDialogRequest { id, title: "system error".into(), text },
                delivered: false, result: None,
            });
            return Ok(NativeReturn::Retry);
        }
        show_system_error(&text)?;
        Ok(NativeReturn::Continue)
    }
}

#[cfg(windows)]
fn show_system_error(text: &str) -> Result<()> {
    use std::ffi::c_void;
    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetActiveWindow() -> *mut c_void;
        fn MessageBoxW(owner: *mut c_void, text: *const u16, title: *const u16, flags: u32) -> i32;
    }
    let text: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
    let title: Vec<u16> = "system error".encode_utf16().chain(Some(0)).collect();
    // The original ignores both IDOK and the platform error return.
    unsafe { MessageBoxW(GetActiveWindow(), text.as_ptr(), title.as_ptr(), 0); }
    Ok(())
}

#[cfg(not(windows))]
fn show_system_error(_: &str) -> Result<()> {
    Err(native_state_error("native 445 needs a real platform dialog queue"))
}

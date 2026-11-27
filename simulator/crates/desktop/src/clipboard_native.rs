//! Ordinary10/1039 share newline expansion and actual platform submission.
//! The latter uses the old UTF-8 decoder and retains its first UTF-16 unit.
use std::collections::VecDeque;
use cs2_script::{NativeReturn, Result, Vm};
use super::{HeadlessPlayer, LoggingHost, LogicEvent, expand_kcs_format, native_state_error, read_u32};

#[derive(Default)]
pub(super) struct ClipboardState {
    pending: VecDeque<Vec<u16>>,
}

impl LoggingHost<'_> {
    /// Owned CP932 payload, already formatted by the M2 manager. Unlike10 this
    /// path performs no KCS formatting, newline expansion or4095-byte clamp.
    pub(super) fn publish_cp932_clipboard(&mut self, index: u32, bytes: &[u8]) {
        #[cfg(windows)]
        let result = windows_result(write_windows_clipboard(1, 1, bytes));
        #[cfg(target_os = "android")]
        let result = self.data_services.clipboard.queue(|| {
            Ok(super::ScriptTextEncoding::Cp932.decode(bytes).encode_utf16().collect())
        });
        #[cfg(not(any(windows, target_os = "android")))]
        let result = Err(native_state_error("clipboard requires a platform adapter"));
        self.note_clipboard_result(index, bytes.len(), result);
    }

    pub(super) fn publish_fes_clipboard(&mut self, bytes: &[u8]) {
        let text = self.text_encoding.decode(bytes);
        #[cfg(windows)]
        let result = {
            let wide: Vec<_> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
            windows_result(write_windows_clipboard(13, 2, &wide))
        };
        #[cfg(target_os = "android")]
        let result = self.data_services.clipboard.queue(|| Ok(text.encode_utf16().collect()));
        #[cfg(not(any(windows, target_os = "android")))]
        let result = Err(native_state_error("clipboard requires a platform adapter"));
        self.note_clipboard_result(0, bytes.len(), result);
    }

    pub(super) fn clipboard_native(&mut self, index: u32, args: &[u8], vm: &Vm) -> Result<NativeReturn> {
        let source = expand_kcs_format(vm, &vm.read_string(read_u32(args, 0)?)?, 0)?;
        let source = super::trim_nul(&source);
        if source.len() > 4095 {
            self.note_clipboard_result(index, source.len(),
                Err(native_state_error("clipboard format exceeds the original temporary buffer")));
            return Ok(NativeReturn::Continue);
        }
        let mut bytes = source.to_vec();
        let mut at = 0;
        while at + 1 < bytes.len() {
            if bytes[at..at + 2] == *b"\\n" {
                bytes[at..at + 2].copy_from_slice(b"\r\n");
                at += 2;
            } else { at += 1; }
        }
        #[cfg(windows)]
        let result = if index == 1039 {
            legacy_wide(&bytes).and_then(|units| {
                let wide: Vec<_> = units.into_iter().flat_map(u16::to_le_bytes).collect();
                windows_result(write_windows_clipboard(13, 2, &wide))
            })
        } else {
            windows_result(write_windows_clipboard(1, 1, &bytes))
        };
        #[cfg(target_os = "android")]
        let result = self.data_services.clipboard.queue(|| {
            if index == 1039 { legacy_wide(&bytes) }
            else { Ok(self.text_encoding.decode(&bytes).encode_utf16().collect()) }
        });
        #[cfg(not(any(windows, target_os = "android")))]
        let result = Err(native_state_error("clipboard requires a platform adapter"));
        self.note_clipboard_result(index, bytes.len(), result);
        Ok(NativeReturn::Continue)
    }

    fn note_clipboard_result(&mut self, index: u32, bytes: usize, result: Result<&str>) {
        let (kind, detail) = match result {
            Ok(kind) => (kind, format!("native={index} source_bytes={bytes}")),
            Err(error) => ("clipboard-write-failed", format!("native={index} source_bytes={bytes}: {error}")),
        };
        self.events.push(LogicEvent { frame: self.frame, kind: kind.into(), detail });
    }
}

#[cfg(target_os = "android")]
impl ClipboardState {
    fn queue(&mut self, payload: impl FnOnce() -> Result<Vec<u16>>) -> Result<&'static str> {
        if self.pending.len() >= 64 {
            return Err(native_state_error("Android clipboard host queue exceeds 64 requests"));
        }
        self.pending.push_back(payload()?);
        Ok("clipboard-host-request")
    }
}

impl HeadlessPlayer {
    /// Android presents this request through its actual ClipboardManager.
    /// Do not treat a queued request as evidence of a platform write.
    pub fn take_host_clipboard_request(&mut self) -> Option<String> {
        self.take_host_clipboard_units().map(|units| String::from_utf16_lossy(&units))
    }

    /// The native Android bridge uses raw UTF-16 so1039's isolated surrogate
    /// is not silently changed by an intermediate UTF-8 serialization.
    pub fn take_host_clipboard_units(&mut self) -> Option<Vec<u16>> {
        self.data_services.clipboard.pending.pop_front()
    }
}

fn legacy_wide(bytes: &[u8]) -> Result<Vec<u16>> {
    let mut units = Vec::new();
    let mut cursor = 0;
    while cursor < bytes.len() {
        if units.len() >= 2047 {
            return Err(native_state_error("Unicode clipboard exceeds the original 2048-unit buffer"));
        }
        if bytes.get(cursor..cursor + 2) == Some(&[0xff, 0xff]) {
            units.push(0xffff); cursor += 2;
            continue;
        }
        let width = super::utf8_byte_width(bytes, cursor);
        if width <= 0 { break; }
        let encoded = bytes.get(cursor..cursor + width as usize)
            .ok_or_else(|| native_state_error("Unicode clipboard has a truncated positive UTF-8 stride"))?;
        let text = String::from_utf8_lossy(encoded);
        let character = text.chars().next().unwrap_or('\u{fffd}');
        units.push(character.encode_utf16(&mut [0; 2])[0]);
        cursor += width as usize;
    }
    Ok(units)
}

#[cfg(windows)]
fn windows_result(written: bool) -> Result<&'static str> {
    if written { Ok("clipboard-written") }
    else { Err(native_state_error("Windows clipboard write failed")) }
}

#[cfg(windows)]
fn write_windows_clipboard(format: u32, terminators: usize, bytes: &[u8]) -> bool {
    use std::ffi::c_void;
    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetActiveWindow() -> *mut c_void;
        fn OpenClipboard(owner: *mut c_void) -> i32;
        fn EmptyClipboard() -> i32;
        fn CloseClipboard() -> i32;
        fn SetClipboardData(format: u32, memory: *mut c_void) -> *mut c_void;
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GlobalAlloc(flags: u32, bytes: usize) -> *mut c_void;
        fn GlobalLock(memory: *mut c_void) -> *mut c_void;
        fn GlobalUnlock(memory: *mut c_void) -> i32;
        fn GlobalFree(memory: *mut c_void) -> *mut c_void;
    }
    unsafe {
        if OpenClipboard(GetActiveWindow()) == 0 { return false; }
        if EmptyClipboard() == 0 {
            CloseClipboard();
            return false;
        }
        let memory = GlobalAlloc(0x42, bytes.len() + 8);
        let success = if memory.is_null() { false } else {
            let target = GlobalLock(memory).cast::<u8>();
            if target.is_null() {
                GlobalFree(memory);
                false
            } else {
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), target, bytes.len());
                std::ptr::write_bytes(target.add(bytes.len()), 0, terminators);
                GlobalUnlock(memory);
                if SetClipboardData(format, memory).is_null() {
                    GlobalFree(memory);
                    false
                } else { true }
            }
        };
        CloseClipboard();
        success
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oversized_optional_clipboard_payloads_do_not_stop_the_script() -> anyhow::Result<()> {
        let path = std::env::temp_dir().join(format!("cs2-clipboard-{}-{}", std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos()));
        std::fs::create_dir(&path)?;
        let player = HeadlessPlayer::open_with_save_root(&path, &path);
        std::fs::remove_dir(path)?;
        let mut player = player?;
        let program = cs2_script::KcsProgram::from_code(&[0x3b], Vec::new(), 0)?;
        let mut vm = Vm::from_program(program)?;
        let mut source = vec![b'a'; 4096];
        source.push(0);
        let long_bytes = vm.allocate_region(&source)?;
        source.truncate(2048);
        source.push(0);
        let long_wide = vm.allocate_region(&source)?;
        for (index, address) in [(10, long_bytes), (1039, long_wide)] {
            let result = player.with_host(|host| Ok(host.clipboard_native(index, &address.to_le_bytes(), &vm)?))?;
            assert!(matches!(result, NativeReturn::Continue));
        }
        assert_eq!(player.events().iter().filter(|event| event.kind == "clipboard-write-failed").count(), 2);
        assert!(player.take_host_clipboard_request().is_none());
        Ok(())
    }
}

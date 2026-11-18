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
    pub(super) fn publish_cp932_clipboard(&mut self, index: u32, bytes: &[u8]) -> Result<()> {
        #[cfg(windows)]
        let state = if write_windows_clipboard(1, 1, || Ok(bytes.to_vec()))? {
            "clipboard-written"
        } else { "clipboard-write-failed" };
        #[cfg(target_os = "android")]
        let state = {
            let pending = &mut self.data_services.clipboard.pending;
            if pending.len() >= 64 {
                return Err(native_state_error("Android clipboard host queue exceeds 64 requests"));
            }
            let text = super::ScriptTextEncoding::Cp932.decode(bytes);
            pending.push_back(text.encode_utf16().collect());
            "clipboard-host-request"
        };
        #[cfg(not(any(windows, target_os = "android")))]
        return Err(native_state_error("clipboard requires a platform adapter"));
        #[cfg(any(windows, target_os = "android"))]
        {
            self.events.push(LogicEvent { frame: self.frame, kind: state.into(),
                detail: format!("native={index} source_bytes={}", bytes.len()) });
            Ok(())
        }
    }

    pub(super) fn clipboard_native(&mut self, index: u32, args: &[u8], vm: &Vm) -> Result<NativeReturn> {
        let source = expand_kcs_format(vm, &vm.read_string(read_u32(args, 0)?)?, 0)?;
        let source = super::trim_nul(&source);
        if source.len() > 4095 {
            return Err(native_state_error("clipboard format exceeds the original temporary buffer"));
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
        let state = if write_windows_clipboard(
            if index == 1039 { 13 } else { 1 },
            if index == 1039 { 2 } else { 1 },
            || if index == 1039 {
                Ok(legacy_wide(&bytes)?.into_iter().flat_map(u16::to_le_bytes).collect())
            } else { Ok(bytes.clone()) },
        )? { "clipboard-written" } else { "clipboard-write-failed" };
        #[cfg(target_os = "android")]
        let state = {
            let pending = &mut self.data_services.clipboard.pending;
            if pending.len() >= 64 {
                return Err(native_state_error("Android clipboard host queue exceeds 64 requests"));
            }
            let units = if index == 1039 { legacy_wide(&bytes)? }
                else { self.text_encoding.decode(&bytes).encode_utf16().collect() };
            pending.push_back(units);
            "clipboard-host-request"
        };
        #[cfg(not(any(windows, target_os = "android")))]
        return Err(native_state_error("clipboard requires a platform adapter"));
        #[cfg(any(windows, target_os = "android"))]
        {
            self.events.push(LogicEvent { frame: self.frame, kind: state.into(),
                detail: format!("native={index} source_bytes={}", bytes.len()) });
            Ok(NativeReturn::Continue)
        }
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
fn write_windows_clipboard(
    format: u32, terminators: usize, payload: impl FnOnce() -> Result<Vec<u8>>,
) -> Result<bool> {
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
        if OpenClipboard(GetActiveWindow()) == 0 { return Ok(false); }
        EmptyClipboard();
        // Unicode conversion is AFTER Open/Empty, as in1039. On a decoding
        // diagnostic the clipboard must still be closed before propagating.
        let bytes = match payload() {
            Ok(bytes) => bytes,
            Err(error) => { CloseClipboard(); return Err(error); }
        };
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
        Ok(success)
    }
}

//! Actual Windows dialogs, browser authorization and WinHTTP HTTPS I/O.
//! The functions run only from the Tweet worker after native1021 starts it.
use anyhow::{Result, bail};

pub(super) struct Response { pub status: u32, pub bytes: Vec<u8> }
pub(super) enum EditorResult { Edited(String), Cancelled, Forget }

#[cfg(not(windows))]
pub(super) fn edit_tweet(_: &str, _: &str, _: &str) -> Result<EditorResult> {
    bail!("Tweet editor has no installed platform consumer on this target")
}

#[cfg(not(windows))]
pub(super) fn legacy_key() -> Option<[u8; 16]> { None }

#[cfg(not(windows))]
pub(super) fn request(_: &str, _: &str, _: &str, _: &str, _: &[u8]) -> Result<Response> {
    bail!("Tweet HTTPS has no installed platform consumer on this target")
}
#[cfg(not(windows))]
pub(super) fn edit(_: &str, _: &str, _: &str, _: usize) -> Result<Option<String>> {
    bail!("Tweet authorization/editor has no installed platform consumer on this target")
}
#[cfg(not(windows))]
pub(super) fn open_url(_: &str) -> Result<()> {
    bail!("Tweet authorization has no installed browser consumer on this target")
}
#[cfg(not(windows))]
pub(super) fn nonce() -> Result<String> {
    #[cfg(unix)] {
        use std::io::Read;
        let mut bytes = [0; 24];
        std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
        Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
    }
    #[cfg(not(unix))] { bail!("Tweet requires an OS random source") }
}
#[cfg(not(windows))]
pub(super) fn seal(_: &[u8]) -> Result<Vec<u8>> { bail!("Tweet private credential protection has no platform binding") }
#[cfg(not(windows))]
pub(super) fn unseal(_: &[u8]) -> Result<Vec<u8>> { bail!("Tweet private credential protection has no platform binding") }
#[cfg(all(not(windows), unix))]
pub(super) fn replace_file(from: &std::path::Path, to: &std::path::Path) -> Result<()> {
    // Same-directory rename: no process-global environment or public storage.
    std::fs::rename(from, to)?;
    if let Some(parent) = to.parent() { std::fs::File::open(parent)?.sync_all()?; }
    Ok(())
}
#[cfg(all(not(windows), not(unix)))]
pub(super) fn replace_file(_: &std::path::Path, _: &std::path::Path) -> Result<()> {
    bail!("Tweet credential replacement has no platform binding")
}

#[cfg(windows)]
mod windows {
    use super::*;
    use std::ffi::c_void;
    use std::ptr::{null, null_mut};
    type Handle = *mut c_void;
    fn wide(text: &str) -> Vec<u16> { text.encode_utf16().chain([0]).collect() }
    #[link(name = "winhttp")]
    unsafe extern "system" {
        fn WinHttpOpen(agent: *const u16, access: u32, proxy: *const u16, bypass: *const u16, flags: u32) -> Handle;
        fn WinHttpConnect(session: Handle, host: *const u16, port: u16, reserved: u32) -> Handle;
        fn WinHttpOpenRequest(connect: Handle, method: *const u16, path: *const u16, version: *const u16,
            referer: *const u16, accept: *const *const u16, flags: u32) -> Handle;
        fn WinHttpSetTimeouts(handle: Handle, resolve: i32, connect: i32, send: i32, receive: i32) -> i32;
        fn WinHttpSetOption(handle: Handle, option: u32, data: *const c_void, size: u32) -> i32;
        fn WinHttpSendRequest(handle: Handle, headers: *const u16, length: u32, data: *const c_void,
            data_length: u32, total: u32, context: usize) -> i32;
        fn WinHttpReceiveResponse(handle: Handle, reserved: Handle) -> i32;
        fn WinHttpQueryHeaders(handle: Handle, level: u32, name: *const u16, buffer: Handle,
            size: *mut u32, index: *mut u32) -> i32;
        fn WinHttpReadData(handle: Handle, output: Handle, size: u32, read: *mut u32) -> i32;
        fn WinHttpCloseHandle(handle: Handle) -> i32;
    }
    struct Http(Handle);
    impl Drop for Http { fn drop(&mut self) { unsafe { WinHttpCloseHandle(self.0); } } }
    fn handle(value: Handle) -> Result<Http> {
        if value.is_null() { return Err(std::io::Error::last_os_error().into()); }
        Ok(Http(value))
    }
    fn check(value: i32) -> Result<()> {
        if value == 0 { return Err(std::io::Error::last_os_error().into()); } Ok(())
    }
    pub(crate) fn request(method: &str, url: &str, authorization: &str, content: &str, bytes: &[u8]) -> Result<Response> {
        let tail = url.strip_prefix("https://").ok_or_else(|| anyhow::anyhow!("Tweet requires HTTPS"))?;
        let (host, path) = tail.split_once('/').ok_or_else(|| anyhow::anyhow!("Tweet URL has no path"))?;
        if !matches!(host, "api.twitter.com" | "upload.twitter.com") { bail!("unexpected Tweet API host"); }
        if authorization.contains(['\r','\n']) || content.contains(['\r','\n']) { bail!("invalid Tweet HTTP header"); }
        let length = u32::try_from(bytes.len())?;
        let agent = wide("CatSystem2-compatible-player/1.0"); let host = wide(host);
        let path = wide(&format!("/{path}")); let method = wide(method);
        let headers = wide(&format!("Authorization: {authorization}\r\nContent-Type: {content}\r\n"));
        let session = handle(unsafe { WinHttpOpen(agent.as_ptr(), 0, null(), null(), 0) })?;
        check(unsafe { WinHttpSetTimeouts(session.0, 30_000, 30_000, 60_000, 60_000) })?;
        let connection = handle(unsafe { WinHttpConnect(session.0, host.as_ptr(), 443, 0) })?;
        let request = handle(unsafe { WinHttpOpenRequest(connection.0, method.as_ptr(), path.as_ptr(),
            null(), null(), null(), 0x0080_0000) })?;
        // Do not forward signed authorization to a redirected host. A 3xx is
        // an actual API failure, not a successful request or browser result.
        let redirect_policy = 0u32;
        check(unsafe { WinHttpSetOption(request.0, 88, (&redirect_policy as *const u32).cast(), 4) })?;
        check(unsafe { WinHttpSendRequest(request.0, headers.as_ptr(), (headers.len()-1) as u32,
            bytes.as_ptr().cast(), length, length, 0) })?;
        check(unsafe { WinHttpReceiveResponse(request.0, null_mut()) })?;
        let mut status = 0u32; let mut size = 4u32;
        check(unsafe { WinHttpQueryHeaders(request.0, 19 | 0x2000_0000, null(),
            (&mut status as *mut u32).cast(), &mut size, null_mut()) })?;
        let mut body = Vec::new();
        loop {
            let mut buffer = [0; 8192]; let mut read = 0;
            check(unsafe { WinHttpReadData(request.0, buffer.as_mut_ptr().cast(), buffer.len() as u32, &mut read) })?;
            if read == 0 { break; }
            if body.len() + read as usize > 1024 * 1024 { bail!("Tweet API response exceeds 1 MiB"); }
            body.extend_from_slice(&buffer[..read as usize]);
        }
        Ok(Response { status, bytes: body })
    }

    #[link(name = "bcrypt")]
    unsafe extern "system" { fn BCryptGenRandom(algorithm: Handle, bytes: *mut u8, length: u32, flags: u32) -> i32; }
    pub(crate) fn nonce() -> Result<String> {
        let mut bytes = [0; 24];
        let status = unsafe { BCryptGenRandom(null_mut(), bytes.as_mut_ptr(), bytes.len() as u32, 2) };
        if status < 0 { bail!("Tweet OS random source failed: {status:#x}"); }
        Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
    }
    #[link(name = "shell32")]
    unsafe extern "system" { fn ShellExecuteW(owner: Handle, operation: *const u16, file: *const u16,
        parameters: *const u16, directory: *const u16, show: i32) -> isize; }
    pub(crate) fn open_url(url: &str) -> Result<()> {
        if !url.starts_with("https://api.twitter.com/oauth/authorize?oauth_token=") { bail!("invalid Tweet authorization URL"); }
        let action = wide("open"); let url = wide(url);
        let result = unsafe { ShellExecuteW(null_mut(), action.as_ptr(), url.as_ptr(), null(), null(), 1) };
        if result <= 32 { bail!("Tweet authorization browser failed: {result}"); } Ok(())
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        fn DialogBoxIndirectParamW(instance: Handle, template: *const c_void, owner: Handle,
            procedure: unsafe extern "system" fn(Handle, u32, usize, isize) -> isize, data: isize) -> isize;
        fn EndDialog(window: Handle, result: isize) -> i32;
        fn SetDlgItemTextW(window: Handle, id: i32, text: *const u16) -> i32;
        fn GetDlgItemTextW(window: Handle, id: i32, text: *mut u16, size: i32) -> u32;
        fn SendDlgItemMessageW(window: Handle, id: i32, message: u32, w: usize, l: isize) -> isize;
        #[cfg_attr(target_pointer_width = "32", link_name = "SetWindowLongW")]
        fn SetWindowLongPtrW(window: Handle, index: i32, value: isize) -> isize;
        #[cfg_attr(target_pointer_width = "32", link_name = "GetWindowLongW")]
        fn GetWindowLongPtrW(window: Handle, index: i32) -> isize;
        fn GetDlgItem(window: Handle, id: i32) -> Handle;
        fn EnableWindow(window: Handle, enable: i32) -> i32;
        fn MessageBoxW(window: Handle, text: *const u16, caption: *const u16, flags: u32) -> i32;
    }
    struct Edit { initial: Vec<u16>, value: Option<Vec<u16>>, limit: usize, suffix: Option<String> }
    unsafe fn count_editor(window: Handle, state: &Edit) {
        let Some(suffix) = &state.suffix else { return; };
        let mut bytes = vec![0u16; state.limit+1];
        let length = unsafe { GetDlgItemTextW(window, 100, bytes.as_mut_ptr(), bytes.len() as i32) };
        bytes.truncate(length as usize);
        let count = String::from_utf16(&bytes).ok().and_then(|text| super::super::tweet_host::tweet_character_count(&text, suffix).ok());
        let label = wide(&count.map_or_else(|| "Text is not CP932 encodable".into(), |count| format!("{}/140", 140i64-count as i64)));
        unsafe { SetDlgItemTextW(window, 102, label.as_ptr()); EnableWindow(GetDlgItem(window, 1), i32::from(count.is_some_and(|c| c <= 140))); }
    }
    unsafe extern "system" fn dialog(window: Handle, message: u32, w: usize, l: isize) -> isize {
        if message == 0x110 {
            unsafe {
                SetWindowLongPtrW(window, -21, l);
                let state = &*(l as *const Edit);
                SendDlgItemMessageW(window, 100, 0xc5, state.limit, 0);
                SetDlgItemTextW(window, 100, state.initial.as_ptr());
                count_editor(window, state);
            }
            return 1;
        }
        if message == 0x111 && w & 0xffff == 100 && w >> 16 == 0x300 {
            let state = unsafe { GetWindowLongPtrW(window, -21) as *const Edit };
            if !state.is_null() { unsafe { count_editor(window, &*state); } }
            return 1;
        }
        if message == 0x111 && w & 0xffff == 3 {
            let state = unsafe { GetWindowLongPtrW(window, -21) as *const Edit };
            if !state.is_null() && unsafe { (*state).suffix.is_some() } {
                let question = wide("Forget this application's saved authorization? No Tweet will be sent.");
                let title = wide("Forget Twitter authorization");
                if unsafe { MessageBoxW(window, question.as_ptr(), title.as_ptr(), 4) } == 6 {
                    unsafe { EndDialog(window, 3); }
                }
            }
            return 1;
        }
        if message == 0x10 || (message == 0x111 && w & 0xffff == 2) {
            unsafe { EndDialog(window, 2); } return 1;
        }
        if message == 0x111 && w & 0xffff == 1 {
            unsafe {
                let ptr = GetWindowLongPtrW(window, -21) as *mut Edit;
                if ptr.is_null() { EndDialog(window, 2); return 1; }
                let state = &mut *ptr;
                let mut text = vec![0; state.limit + 1];
                let count = GetDlgItemTextW(window, 100, text.as_mut_ptr(), text.len() as i32) as usize;
                text.truncate(count); state.value = Some(text); EndDialog(window, 1);
            }
            return 1;
        }
        0
    }
    fn word(out: &mut Vec<u8>, value: u16) { out.extend_from_slice(&value.to_le_bytes()); }
    fn dword(out: &mut Vec<u8>, value: u32) { out.extend_from_slice(&value.to_le_bytes()); }
    fn text(out: &mut Vec<u8>, value: &str) { for c in value.encode_utf16().chain([0]) { word(out, c); } }
    fn control(out: &mut Vec<u8>, style: u32, rectangle: [u16; 4], id: u16, class: u16, title: &str) {
        while out.len() % 4 != 0 { out.push(0); }
        dword(out, style | 0x5000_0000); dword(out, 0);
        for value in rectangle { word(out, value); }
        word(out, id); word(out, 0xffff); word(out, class); text(out, title); word(out, 0);
    }
    pub(crate) fn edit(caption: &str, prompt: &str, initial: &str, limit: usize) -> Result<Option<String>> {
        match editor(caption, prompt, initial, limit, None)? {
            EditorResult::Edited(text) => Ok(Some(text)), EditorResult::Cancelled => Ok(None),
            EditorResult::Forget => bail!("unexpected forget result in PIN dialog"),
        }
    }
    pub(crate) fn edit_tweet(prompt: &str, initial: &str, suffix: &str) -> Result<EditorResult> {
        editor("Tweet", &format!("{prompt}\n{suffix}"), initial, 1023, Some(suffix.to_owned()))
    }
    fn editor(caption: &str, prompt: &str, initial: &str, limit: usize, suffix: Option<String>) -> Result<EditorResult> {
        if limit == 0 || limit > 32767 || initial.encode_utf16().count() > limit { bail!("Tweet editor capacity exceeded"); }
        let mut bytes = Vec::new(); dword(&mut bytes, 0x80c8_0880); dword(&mut bytes, 0);
        word(&mut bytes, if suffix.is_some() {6} else {4});
        for value in [10,10,320,190] { word(&mut bytes, value); }
        word(&mut bytes, 0); word(&mut bytes, 0); text(&mut bytes, caption);
        control(&mut bytes, 0, [8,8,304,40], 101, 0x82, prompt);
        control(&mut bytes, 0x00a1_1004, [8,50,304,110], 100, 0x81, "");
        control(&mut bytes, 0x0001_0001, [200,166,52,16], 1, 0x80, "OK");
        control(&mut bytes, 0x0001_0000, [258,166,52,16], 2, 0x80, "Cancel");
        if suffix.is_some() {
            control(&mut bytes, 0x0001_0000, [8,166,110,16], 3, 0x80, "Forget authorization");
            control(&mut bytes, 0, [122,166,72,16], 102, 0x82, "");
        }
        while bytes.len() % 4 != 0 { bytes.push(0); }
        // Vec<u32> guarantees the DLGTEMPLATE alignment on both Windows ABIs.
        let template = bytes.chunks_exact(4).map(|b| u32::from_le_bytes(b.try_into().unwrap())).collect::<Vec<_>>();
        let mut state = Edit { initial: wide(initial), value: None, limit, suffix };
        let result = unsafe { DialogBoxIndirectParamW(null_mut(), template.as_ptr().cast(), null_mut(),
            dialog, (&mut state as *mut Edit) as isize) };
        match result {
            -1 => Err(std::io::Error::last_os_error().into()),
            1 => Ok(EditorResult::Edited(String::from_utf16(&state.value.ok_or_else(|| anyhow::anyhow!("Tweet editor returned no text"))?)?)),
            2 => Ok(EditorResult::Cancelled), 3 => Ok(EditorResult::Forget), _ => bail!("unexpected Tweet editor result {result}"),
        }
    }

    #[repr(C)]
    struct Blob { length: u32, data: *mut u8 }
    #[link(name = "crypt32")]
    unsafe extern "system" {
        fn CryptProtectData(input: *const Blob, description: *const u16, entropy: *const Blob,
            reserved: Handle, prompt: Handle, flags: u32, output: *mut Blob) -> i32;
        fn CryptUnprotectData(input: *const Blob, description: *mut *mut u16, entropy: *const Blob,
            reserved: Handle, prompt: Handle, flags: u32, output: *mut Blob) -> i32;
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LocalFree(value: Handle) -> Handle;
        fn MoveFileExW(from: *const u16, to: *const u16, flags: u32) -> i32;
        fn GetWindowsDirectoryW(path: *mut u16, length: u32) -> u32;
        fn GetVolumeInformationW(root: *const u16, name: *mut u16, name_length: u32,
            serial: *mut u32, component: *mut u32, flags: *mut u32, fs: *mut u16, fs_length: u32) -> i32;
    }
    pub(crate) fn legacy_key() -> Option<[u8; 16]> {
        let mut directory = [0u16; 260];
        let length = unsafe { GetWindowsDirectoryW(directory.as_mut_ptr(), 260) };
        let mut serial = 0;
        let root = [directory[0], b':' as u16, b'\\' as u16, 0];
        let valid = length != 0 && length < 260 && unsafe { GetVolumeInformationW(root.as_ptr(),
            null_mut(), 0, &mut serial, null_mut(), null_mut(), null_mut(), 0) } != 0;
        Some(cs2_formats::tweet_credentials::volume_key(valid.then_some(serial)))
    }
    fn protect(bytes: &[u8], seal: bool) -> Result<Vec<u8>> {
        let input = Blob { length: u32::try_from(bytes.len())?, data: bytes.as_ptr().cast_mut() };
        let mut output = Blob { length: 0, data: null_mut() };
        check(unsafe { if seal { CryptProtectData(&input, null(), null(), null_mut(), null_mut(), 1, &mut output) }
            else { CryptUnprotectData(&input, null_mut(), null(), null_mut(), null_mut(), 1, &mut output) } })?;
        let result = if output.length > 32768 || output.data.is_null() { Err(anyhow::anyhow!("invalid protected Tweet credential size")) }
            else { Ok(unsafe { std::slice::from_raw_parts(output.data, output.length as usize) }.to_vec()) };
        unsafe { LocalFree(output.data.cast()); } result
    }
    pub(crate) fn seal(bytes: &[u8]) -> Result<Vec<u8>> { protect(bytes, true) }
    pub(crate) fn unseal(bytes: &[u8]) -> Result<Vec<u8>> { protect(bytes, false) }
    pub(crate) fn replace_file(from: &std::path::Path, to: &std::path::Path) -> Result<()> {
        use std::os::windows::ffi::OsStrExt;
        let from = from.as_os_str().encode_wide().chain([0]).collect::<Vec<_>>();
        let to = to.as_os_str().encode_wide().chain([0]).collect::<Vec<_>>();
        check(unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), 1 | 8) })
    }
}
#[cfg(windows)]
pub(super) use windows::{request, edit, edit_tweet, open_url, nonce, seal, unseal, replace_file, legacy_key};

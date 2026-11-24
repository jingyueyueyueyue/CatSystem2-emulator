use std::ffi::c_void;
use std::ptr::null;

use anyhow::{Result, bail};

use super::HostTextEntry;

const TEXT_FIELD: i32 = 1163;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetModuleHandleW(name: *const u16) -> isize;
}

#[link(name = "user32")]
unsafe extern "system" {
    fn DialogBoxIndirectParamW(
        instance: isize,
        template: *const c_void,
        owner: isize,
        procedure: unsafe extern "system" fn(isize, u32, usize, isize) -> isize,
        parameter: isize,
    ) -> isize;
    fn SetWindowTextW(window: isize, text: *const u16) -> i32;
    fn SetDlgItemTextW(window: isize, control: i32, text: *const u16) -> i32;
    fn GetDlgItemTextW(window: isize, control: i32, text: *mut u16, limit: i32) -> u32;
    fn GetDlgItem(window: isize, control: i32) -> isize;
    fn EnableWindow(window: isize, enabled: i32) -> i32;
    fn EndDialog(window: isize, result: isize) -> i32;
    fn SetFocus(window: isize) -> isize;
    fn SendDlgItemMessageW(
        window: isize,
        control: i32,
        message: u32,
        first: usize,
        second: isize,
    ) -> isize;
    #[cfg_attr(target_pointer_width = "32", link_name = "SetWindowLongW")]
    fn SetWindowLongPtrW(window: isize, index: i32, value: isize) -> isize;
    #[cfg_attr(target_pointer_width = "32", link_name = "GetWindowLongW")]
    fn GetWindowLongPtrW(window: isize, index: i32) -> isize;
}

struct Dialog<'a> {
    request: &'a HostTextEntry,
    result: Option<String>,
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

unsafe extern "system" fn procedure(
    window: isize,
    message: u32,
    first: usize,
    second: isize,
) -> isize {
    const USER_DATA: i32 = -21;
    match message {
        0x110 => unsafe {
            SetWindowLongPtrW(window, USER_DATA, second);
            let request = (*(second as *const Dialog<'_>)).request;
            SetWindowTextW(window, wide(&request.title).as_ptr());
            SetDlgItemTextW(window, TEXT_FIELD, wide(&request.initial).as_ptr());
            SendDlgItemMessageW(window, TEXT_FIELD, 0xc5, request.limit, 0);
            EnableWindow(GetDlgItem(window, 2), i32::from(request.allow_cancel));
            SetFocus(GetDlgItem(window, TEXT_FIELD));
            SendDlgItemMessageW(window, TEXT_FIELD, 0xb1, 0, -1);
            0
        },
        0x111 | 0x10 => unsafe {
            let state = GetWindowLongPtrW(window, USER_DATA) as *mut Dialog<'_>;
            let Some(state) = state.as_mut() else {
                return 0;
            };
            if message == 0x111 && first as u16 == 1 {
                let mut text = vec![0; state.request.limit + 1];
                let length =
                    GetDlgItemTextW(window, TEXT_FIELD, text.as_mut_ptr(), text.len() as i32);
                state.result = Some(String::from_utf16_lossy(&text[..length as usize]));
                EndDialog(window, 1);
                1
            } else if message == 0x10 || first as u16 == 2 {
                if state.request.allow_cancel {
                    EndDialog(window, 2);
                }
                1
            } else {
                0
            }
        },
        _ => 0,
    }
}

pub(super) fn show(owner: isize, request: &HostTextEntry) -> Result<Option<String>> {
    // The dialog template uses DWORD alignment for both the top-level and item records.
    let mut template = vec![0x08c0u16, 0x80c8, 0, 0, 3, 0, 0, 270, 49, 0, 0, 0, 9];
    template.extend(wide("Segoe UI"));
    for (id, class, style, rect, label) in [
        (TEXT_FIELD as u16, 0x81, 0x5081_0080u32, [7, 7, 256, 14], ""),
        (1, 0x80, 0x5001_0001, [157, 27, 50, 14], "OK"),
        (2, 0x80, 0x5001_0000, [213, 27, 50, 14], "Cancel"),
    ] {
        if template.len() % 2 != 0 {
            template.push(0);
        }
        template.extend([style as u16, (style >> 16) as u16, 0, 0]);
        template.extend(rect);
        template.extend([id, 0xffff, class]);
        template.extend(wide(label));
        template.push(0);
    }
    if template.len() % 2 != 0 {
        template.push(0);
    }
    let template: Vec<u32> = template
        .chunks_exact(2)
        .map(|pair| u32::from(pair[0]) | u32::from(pair[1]) << 16)
        .collect();
    let mut dialog = Dialog {
        request,
        result: None,
    };
    let result = unsafe {
        DialogBoxIndirectParamW(
            GetModuleHandleW(null()),
            template.as_ptr().cast(),
            owner,
            procedure,
            (&mut dialog as *mut Dialog<'_>) as isize,
        )
    };
    if result == -1 {
        bail!(
            "Windows text-entry dialog failed: {}",
            std::io::Error::last_os_error()
        );
    }
    Ok(dialog.result)
}

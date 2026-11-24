//! Real property-sheet and menu consumers shared by both Windows presenters.
use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::ffi::c_void;
use std::ptr::{null, null_mut};
use anyhow::{Context as _, Result, bail};
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;
use crate::{HeadlessPlayer, HostEffectSettings, HostMenuItem};

#[path = "settings_menu_general_windows.rs"]
mod general;
pub(super) use general::{apply as apply_general_policy, camouflaged as general_camouflaged};
use super::settings_menu_native::{HostSettingKind, HostSettingsEdit, HostSettingsPage,
    settings_decimal_text, settings_decimal_value, settings_preset_edit, settings_sound_edit, input_view_checks,
    font_face_indices, font_select, font_reset};

type DialogProc = unsafe extern "system" fn(isize, u32, usize, isize) -> isize;
#[repr(C)]
struct Page {
    size: u32, flags: u32, instance: isize, template: *const c_void,
    icon: isize, title: *const u16, procedure: Option<DialogProc>, parameter: isize,
    callback: *const c_void, references: *mut u32,
}
#[repr(C)]
struct Sheet {
    size: u32, flags: u32, parent: isize, instance: isize, icon: isize,
    caption: *const u16, count: u32, start: usize, pages: *const isize,
    callback: *const c_void,
}
#[repr(C)]
struct Notification { window: isize, id: usize, code: u32 }
#[link(name = "comctl32")]
unsafe extern "system" {
    fn CreatePropertySheetPageW(page: *const Page) -> isize;
    fn DestroyPropertySheetPage(page: isize) -> i32;
    fn PropertySheetW(header: *const Sheet) -> isize;
}
#[link(name = "user32")]
unsafe extern "system" {
    fn GetParent(window: isize) -> isize;
    fn SendMessageW(window: isize, message: u32, wparam: usize, lparam: isize) -> isize;
    fn CheckDlgButton(window: isize, command: i32, checked: u32) -> i32;
    fn IsDlgButtonChecked(window: isize, command: i32) -> u32;
    fn SetDlgItemInt(window: isize, command: i32, value: u32, signed: i32) -> i32;
    fn GetDlgItemInt(window: isize, command: i32, translated: *mut i32, signed: i32) -> u32;
    fn SetDlgItemTextW(window: isize, command: i32, text: *const u16) -> i32;
    fn GetDlgItemTextW(window: isize, command: i32, text: *mut u16, maximum: i32) -> u32;
    fn GetDlgItem(window: isize, command: i32) -> isize;
    fn EnableWindow(window: isize, enabled: i32) -> i32;
    fn CreateMenu() -> isize;
    fn CreatePopupMenu() -> isize;
    fn AppendMenuW(menu: isize, flags: u32, command: usize, text: *const u16) -> i32;
    fn DestroyMenu(menu: isize) -> i32;
    fn SetMenu(window: isize, menu: isize) -> i32;
    fn DrawMenuBar(window: isize) -> i32;
    fn CallWindowProcW(procedure: Option<DialogProc>, window: isize, message: u32, wparam: usize, lparam: isize) -> isize;
    fn DefWindowProcW(window: isize, message: u32, wparam: usize, lparam: isize) -> isize;
    fn MessageBoxW(window: isize, text: *const u16, title: *const u16, flags: u32) -> i32;
    #[cfg(target_pointer_width = "64")]
    fn SetWindowLongPtrW(window: isize, index: i32, value: isize) -> isize;
    #[cfg(target_pointer_width = "64")]
    fn GetWindowLongPtrW(window: isize, index: i32) -> isize;
    #[cfg(target_pointer_width = "32")]
    fn SetWindowLongW(window: isize, index: i32, value: isize) -> isize;
    #[cfg(target_pointer_width = "32")]
    fn GetWindowLongW(window: isize, index: i32) -> isize;
}
unsafe fn set_data(window: isize, index: i32, value: isize) -> isize {
    #[cfg(target_pointer_width = "64")]
    { unsafe { SetWindowLongPtrW(window, index, value) } }
    #[cfg(target_pointer_width = "32")]
    { unsafe { SetWindowLongW(window, index, value) } }
}
unsafe fn get_long(window: isize, index: i32) -> isize {
    #[cfg(target_pointer_width = "64")]
    { unsafe { GetWindowLongPtrW(window, index) } }
    #[cfg(target_pointer_width = "32")]
    { unsafe { GetWindowLongW(window, index) } }
}
fn wide(text: &str) -> Vec<u16> { text.encode_utf16().chain(Some(0)).collect() }
pub(super) fn hwnd(window: &Window) -> Result<isize> {
    let handle = window.window_handle()?;
    let RawWindowHandle::Win32(handle) = handle.as_raw() else { bail!("host UI requires a Win32 window"); };
    Ok(handle.hwnd.get())
}

pub(super) fn show_fes_menu_info(player: &mut HeadlessPlayer, window: &Window) -> Result<()> {
    let info = player.host_fes_menu_info()?;
    let result = unsafe { MessageBoxW(hwnd(window)?, wide(&info.message).as_ptr(), wide(&info.title).as_ptr(), 0x40) };
    if result == 0 { bail!("menu information dialog failed: {}", std::io::Error::last_os_error()); }
    player.complete_host_fes_menu_info(info.id)
}
struct Context {
    player: *mut HeadlessPlayer, owner: isize, request: HostEffectSettings, values: Vec<HostSettingsEdit>,
    modified: bool, filling: bool, failure: Option<String>,
}
struct PageContext { shared: *mut Context, index: usize }
fn radio_id(field: usize, option: usize) -> i32 { 20000 + field as i32 * 32 + option as i32 }
unsafe fn fill_font_controls(window: isize, page: &HostSettingsPage, values: &[u32]) {
    let panel = page.font.as_ref().unwrap();
    let choices = font_face_indices(panel, values[4]);
    let combo = unsafe { GetDlgItem(window, 1121) };
    unsafe { SendMessageW(combo, 0x14b, 0, 0); }
    for &index in &choices {
        unsafe { SendMessageW(combo, 0x143, 0, wide(&panel.faces[index].0).as_ptr() as isize); }
    }
    let selected = choices.iter().position(|&index| index == values[0] as usize).unwrap_or(usize::MAX);
    unsafe { SendMessageW(combo, 0x14e, selected, 0); }
    for (control, &value) in page.controls[1..].iter().zip(&values[1..]) {
        unsafe { CheckDlgButton(window, control.command as i32, u32::from(value != 0)); }
    }
    unsafe { EnableWindow(GetDlgItem(window, 1306), i32::from(values[4] != 0)); }
}
#[link(name = "gdi32")]
unsafe extern "system" {
    fn CreateDIBSection(dc: isize, info: *const u32, usage: u32, bits: *mut *mut c_void, section: isize, offset: u32) -> isize;
    fn DeleteObject(object: isize) -> i32;
}
unsafe fn font_preview(window: isize, shared: *mut Context, values: &[u32]) {
    let (player, id) = unsafe { ((*shared).player, (*shared).request.id) };
    let result = (|| -> Result<isize> {
        let bytes = unsafe { &*player }.host_settings_font_preview(id, values[0], u32::from(values[3] != 0))?;
        let width = u32::from_le_bytes(bytes[..4].try_into().unwrap());
        let height = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
        let header = [40, width, (-(height as i32)) as u32, 0x00200001, 0, (bytes.len() - 8) as u32, 0, 0, 0, 0];
        let mut bits = null_mut();
        let bitmap = unsafe { CreateDIBSection(0, header.as_ptr(), 0, &mut bits, 0, 0) };
        if bitmap == 0 || bits.is_null() {
            if bitmap != 0 { unsafe { DeleteObject(bitmap); } }
            bail!("font preview DIB allocation failed: {}", std::io::Error::last_os_error());
        }
        let target = unsafe { std::slice::from_raw_parts_mut(bits.cast::<u8>(), bytes.len() - 8) };
        for (rgba, bgra) in bytes[8..].chunks_exact(4).zip(target.chunks_exact_mut(4)) {
            //Opaque BI_RGB avoids STATIC's alpha-bitmap copy/ownership branch.
            bgra.copy_from_slice(&[rgba[2], rgba[1], rgba[0], 0]);
        }
        Ok(bitmap)
    })();
    let (bitmap, error) = match result { Ok(bitmap) => (bitmap, String::new()), Err(error) => (0, format!("{error:#}")) };
    unsafe {
        let old = SendMessageW(GetDlgItem(window, 1125), 0x172, 0, bitmap);
        if old != 0 { DeleteObject(old); }
        SetDlgItemTextW(window, 1126, wide(&error).as_ptr());
    }
}
unsafe fn font_page_command(window: isize, shared: *mut Context, index: usize, page: &HostSettingsPage,
    command: u32, code: usize) -> bool {
    let panel = page.font.as_ref().unwrap();
    let mut values = unsafe { (&(*shared).values)[index].words.clone() };
    if command == 1145 { font_reset(page, &mut values); }
    else if command == 1121 && code == 1 {
        let selected = unsafe { SendMessageW(GetDlgItem(window, 1121), 0x147, 0, 0) };
        let Some(face) = usize::try_from(selected).ok().and_then(|index| font_face_indices(panel, values[4]).get(index).copied())
            else { return true; };
        values[0] = face as u32;
    } else if code == 0 {
        let Some(at) = page.controls.iter().position(|control| control.command == command && command != 1121) else { return false; };
        values[at] = u32::from(unsafe { IsDlgButtonChecked(window, command as i32) } != 0);
        if command == 1300 {
            let name = panel.faces.get(values[0] as usize).map_or("", |(name, _)| name.as_str());
            values[0] = font_select(panel, values[4], name);
        }
    } else { return false; }
    unsafe {
        (*shared).filling = true; fill_font_controls(window, page, &values); font_preview(window, shared, &values); (*shared).filling = false;
        (&mut (*shared).values)[index].words = values;
        if command == 1145 { (&mut (*shared).values)[index].reset = true; }
        (*shared).modified = true; SendMessageW(GetParent(window), 0x468, window as usize, 0);
    }
    true
}
unsafe fn fill_controls(window: isize, page: &HostSettingsPage, values: &mut [u32]) {
    if page.font.is_some() { unsafe { fill_font_controls(window, page, values); } return; }
    if let Some(input) = &page.input {
        for command in [1063, 1312] { unsafe { SendMessageW(GetDlgItem(window, command), 0x14b, 0, 0); } }
        for (_, label) in &input.keys {
            unsafe { SendMessageW(GetDlgItem(window, 1063), 0x143, 0, wide(label).as_ptr() as isize); }
        }
        for (label, _) in &input.devices {
            unsafe { SendMessageW(GetDlgItem(window, 1312), 0x143, 0, wide(label).as_ptr() as isize); }
        }
        unsafe { SendMessageW(GetDlgItem(window, 1063), 0x14e, 0, 0);
            SendMessageW(GetDlgItem(window, 1312), 0x14e, 0, 0);
            CheckDlgButton(window, 1327, u32::from(values[480] != 0));
            fill_input_bindings(window, input, values); }
        return;
    }
    for (index, (control, &value)) in page.controls.iter().zip(values.iter()).enumerate() {
        if page.sound.as_ref().is_some_and(|sound| index >= sound.first_role) { continue; }
        let id = control.command as i32;
        match &control.kind {
            HostSettingKind::Toggle => { unsafe { CheckDlgButton(window, id, u32::from(value != 0)); } }
            HostSettingKind::Choice(options) => {
                for option in 0..options.len() {
                    unsafe { CheckDlgButton(window, radio_id(index, option), u32::from(value == option as u32)); }
                }
            }
            HostSettingKind::Integer { .. } | HostSettingKind::AutoPreset(_) => { unsafe { SetDlgItemInt(window, id, value, 0); } }
            HostSettingKind::Decimal { scale, .. } => { unsafe { SetDlgItemTextW(window, id, wide(&settings_decimal_text(value, *scale)).as_ptr()); } }
        }
        if let Some((dependency, expected)) = control.enabled_by {
            let enabled = page.controls.iter().position(|field| field.command == dependency)
                .is_some_and(|index| (values[index] != 0) == expected);
            unsafe { EnableWindow(GetDlgItem(window, id), i32::from(enabled)); }
        }
        if control.disabled { unsafe { EnableWindow(GetDlgItem(window, id), 0); } }
    }
    if page.sound.is_some() { unsafe { fill_sound_roles(window, page, values); } }
}
unsafe fn fill_sound_roles(window: isize, page: &HostSettingsPage, values: &[u32]) {
    let sound = page.sound.as_ref().unwrap();
    let list = unsafe { GetDlgItem(window, 1114) };
    unsafe { SendMessageW(list, 0x184, 0, 0); }
    for (row, control) in page.controls[sound.first_role..].iter().enumerate() {
        unsafe { SendMessageW(list, 0x180, 0, wide(&control.label).as_ptr() as isize);
            SendMessageW(list, 0x185, usize::from(values[sound.first_role + row] != 0), row as isize); }
    }
}
unsafe fn input_selection(window: isize, input: &crate::HostInputPanel) -> Option<(usize, usize)> {
    let key = unsafe { SendMessageW(GetDlgItem(window, 1063), 0x147, 0, 0) };
    let device = unsafe { SendMessageW(GetDlgItem(window, 1312), 0x147, 0, 0) };
    let key = input.keys.get(usize::try_from(key).ok()?)?.0 as usize;
    let device = usize::try_from(device).ok()?;
    input.devices.get(device)?;
    Some((key, device))
}
unsafe fn fill_input_bindings(window: isize, input: &crate::HostInputPanel, values: &mut [u32]) {
    let selected = unsafe { input_selection(window, input) };
    let checks = selected.map(|(key, device)| input_view_checks(values, key, device));
    for slot in 0..4 {
        let combo = unsafe { GetDlgItem(window, 1313 + slot) };
        unsafe { SendMessageW(combo, 0x14b, 0, 0); }
        let Some((key, device)) = selected else {
            unsafe { EnableWindow(combo, 0); EnableWindow(GetDlgItem(window, 1317 + slot), 0); }
            continue;
        };
        for (_, label) in &input.devices[device].1 {
            unsafe { SendMessageW(combo, 0x143, 0, wide(label).as_ptr() as isize); }
        }
        let code = values[key * 6 + device * 2].to_le_bytes()[slot as usize];
        let index = input.devices[device].1.iter().position(|(candidate, _)| *candidate == u32::from(code));
        let enabled = device != 2 || values[480] != 0;
        unsafe { SendMessageW(combo, 0x14e, index.map_or(usize::MAX, |index| index), 0);
            CheckDlgButton(window, 1317 + slot, checks.unwrap()[slot as usize]);
            EnableWindow(combo, i32::from(enabled)); EnableWindow(GetDlgItem(window, 1317 + slot), i32::from(enabled)); }
        let mut repeat = values[key * 6 + device * 2 + 1].to_le_bytes();
        repeat[slot as usize] = unsafe { IsDlgButtonChecked(window, 1317 + slot) } as u8;
        values[key * 6 + device * 2 + 1] = u32::from_le_bytes(repeat);
    }
}
unsafe fn input_page_command(window: isize, shared: *mut Context, index: usize,
    page: &HostSettingsPage, command: i32, code: usize) -> bool {
    let Some(input) = &page.input else { return false; };
    if matches!(command, 1063 | 1312) && code == 1 {
        let mut values = unsafe { (&(*shared).values)[index].words.clone() };
        unsafe { (*shared).filling = true; fill_input_bindings(window, input, &mut values); (*shared).filling = false;
            (&mut (*shared).values)[index].words = values; }
        return true;
    }
    if command == 1145 {
        let mut values = page.controls.iter().map(|control| control.default).collect::<Vec<_>>();
        unsafe { (*shared).filling = true; CheckDlgButton(window, 1327, u32::from(values[480] != 0));
            fill_input_bindings(window, input, &mut values); (*shared).filling = false;
            (&mut (*shared).values)[index] = HostSettingsEdit { reset: true, words: values }; }
    } else if command == 1327 && code == 0 {
        let enabled = u32::from(unsafe { IsDlgButtonChecked(window, command) } != 0);
        unsafe { (&mut (*shared).values)[index].words[480] = enabled; }
        let mut values = unsafe { (&(*shared).values)[index].words.clone() };
        unsafe { (*shared).filling = true; fill_input_bindings(window, input, &mut values); (*shared).filling = false;
            (&mut (*shared).values)[index].words = values; }
    } else if ((1313..=1316).contains(&command) && code == 1) || ((1317..=1320).contains(&command) && code == 0) {
        let Some((key, device)) = (unsafe { input_selection(window, input) }) else { return true; };
        if device == 2 && unsafe { (&(*shared).values)[index].words[480] } == 0 { return true; }
        let repeat = command >= 1317;
        let slot = (command - if repeat { 1317 } else { 1313 }) as usize;
        let value = if repeat { u8::from(unsafe { IsDlgButtonChecked(window, command) } != 0) } else {
            let selected = unsafe { SendMessageW(GetDlgItem(window, command), 0x147, 0, 0) };
            let Some((code, _)) = usize::try_from(selected).ok().and_then(|index| input.devices[device].1.get(index)) else { return true; };
            *code as u8
        };
        let words = unsafe { &mut (&mut (*shared).values)[index].words };
        let at = key * 6 + device * 2 + usize::from(repeat);
        let mut bytes = words[at].to_le_bytes(); bytes[slot] = value; words[at] = u32::from_le_bytes(bytes);
    } else { return true; }
    unsafe { (*shared).modified = true; SendMessageW(GetParent(window), 0x468, window as usize, 0); }
    true
}
unsafe extern "system" fn page_proc(window: isize, message: u32, wparam: usize, lparam: isize) -> isize {
    if message == 0x110 {
        if lparam == 0 { return 0; }
        let page = unsafe { &*(lparam as *const Page) };
        unsafe { set_data(window, -21, page.parameter); }
        let page = unsafe { &*(page.parameter as *const PageContext) };
        let (request, mut values) = {
            let context = unsafe { &mut *page.shared };
            context.filling = true;
            let request = context.request.pages[page.index].clone();
            (request, context.values[page.index].words.clone())
        };
        // SetDlgItemInt can synchronously reenter this procedure. Keep no
        // reference to Context across any such Win32 call.
        unsafe { fill_controls(window, &request, &mut values); (*page.shared).filling = false;
            (&mut (*page.shared).values)[page.index].words = values; }
        if request.font.is_some() {
            let values = unsafe { (&(*page.shared).values)[page.index].words.clone() };
            unsafe { font_preview(window, page.shared, &values); }
        }
        return 1;
    }
    let pointer = unsafe { get_long(window, -21) } as *mut PageContext;
    if pointer.is_null() { return 0; }
    let page_context = unsafe { &*pointer };
    let shared = page_context.shared;
    let page_index = page_context.index;
    if unsafe { (*shared).filling } { return 0; }
    let page = unsafe { (&(*shared).request.pages)[page_index].clone() };
    if message == 2 && page.font.is_some() {
        unsafe { let old = SendMessageW(GetDlgItem(window, 1125), 0x173, 0, 0); if old != 0 { DeleteObject(old); } }
    }
    if message == 0x111 {
        let command = (wparam & 65535) as i32;
        if page.font.is_some() {
            return if unsafe { font_page_command(window, shared, page_index, &page, command as u32, wparam >> 16) } { 1 } else { 0 };
        }
        if page.input.is_some() {
            return if unsafe { input_page_command(window, shared, page_index, &page, command, wparam >> 16) } { 1 } else { 0 };
        }
        if let Some(sound) = &page.sound {
            if (command == 1114 && wparam >> 16 == 1) || matches!(command, 1116 | 1118) {
                let mut values = unsafe { (&(*shared).values)[page_index].words.clone() };
                if command == 1114 {
                    for (row, value) in values[sound.first_role..].iter_mut().enumerate() {
                        let selected = unsafe { SendMessageW(GetDlgItem(window, 1114), 0x187, row, 0) };
                        if selected >= 0 { *value = u32::from(selected != 0); }
                    }
                } else {
                    let reset = unsafe { (&(*shared).values)[page_index].reset };
                    for (index, value) in values.iter_mut().enumerate().skip(sound.first_role) {
                        if !reset || sound.reset_roles.contains(&index) { *value = u32::from(command == 1116); }
                    }
                    unsafe { (*shared).filling = true; fill_sound_roles(window, &page, &values); (*shared).filling = false; }
                }
                unsafe { (&mut (*shared).values)[page_index].words = values; (*shared).modified = true;
                    SendMessageW(GetParent(window), 0x468, window as usize, 0); }
                return 1;
            }
        }
        if command == 1145 {
            let mut defaults = page.controls.iter().map(|control| control.default).collect::<Vec<_>>();
            unsafe { (*shared).filling = true; fill_controls(window, &page, &mut defaults); (*shared).filling = false; }
            let context = unsafe { &mut *shared };
            context.values[page_index].reset = true;
            for (index, control) in page.controls.iter().enumerate() {
                if control.reset_changes_value { context.values[page_index].words[index] = control.default; }
            }
        } else if let Some((index, selected)) = page.controls.iter().enumerate().find_map(|(index, control)| {
            if let HostSettingKind::Choice(options) = &control.kind {
                (0..options.len()).find(|&option| radio_id(index, option) == command)
                    .map(|option| (index, Some(option as u32)))
            } else { (control.command == command as u32).then_some((index, None)) }
        }) {
            let control = &page.controls[index];
            if control.disabled { return 0; }
            let code = wparam >> 16;
            let value = match &control.kind {
                HostSettingKind::Toggle if code == 0 => u32::from(unsafe { IsDlgButtonChecked(window, command) } != 0),
                HostSettingKind::Choice(_) if code == 0 => selected.unwrap(),
                HostSettingKind::Integer { minimum, maximum } if code == 0x300 || code == 0x200 => {
                    let mut valid = 0;
                    let value = unsafe { GetDlgItemInt(window, command, &mut valid, 0) };
                    if valid == 0 { return 0; }
                    value.clamp(*minimum, *maximum)
                }
                HostSettingKind::AutoPreset(presets) if code == 0x300 || code == 0x200 => {
                    let mut valid = 0;
                    let value = unsafe { GetDlgItemInt(window, command, &mut valid, 0) };
                    if valid == 0 { return 0; }
                    value.min(presets.len() as u32 - 1)
                }
                HostSettingKind::Decimal { scale, minimum, maximum } if code == 0x300 || code == 0x200 => {
                    let mut text = [0u16; 256];
                    let length = unsafe { GetDlgItemTextW(window, command, text.as_mut_ptr(), text.len() as i32) } as usize;
                    let Some(value) = settings_decimal_value(&String::from_utf16_lossy(&text[..length]), *scale, *minimum, *maximum)
                        else { return 0; };
                    value
                }
                _ => return 0,
            };
            unsafe { (&mut *shared).values[page_index].words[index] = value; }
            let changed_pair = {
                let context = unsafe { &mut *shared };
                settings_preset_edit(&page, &mut context.values[page_index].words, control.command)
            };
            if changed_pair {
                for target in &page.controls {
                    if matches!(target.command, 1129 | 1080) {
                        let index = page.controls.iter().position(|field| field.command == target.command).unwrap();
                        let value = unsafe { (&(*shared).values)[page_index].words[index] };
                        unsafe { (*shared).filling = true; }
                        if let HostSettingKind::Decimal { scale, .. } = &target.kind {
                            unsafe { SetDlgItemTextW(window, target.command as i32, wide(&settings_decimal_text(value, *scale)).as_ptr()); }
                        } else { unsafe { SetDlgItemInt(window, target.command as i32, value, 0); } }
                        unsafe { (*shared).filling = false; }
                    }
                }
            }
            let linked_sound = {
                let context = unsafe { &mut *shared };
                settings_sound_edit(&page, &mut context.values[page_index].words, control.command)
            };
            if let Some((index, value)) = linked_sound {
                unsafe { (*shared).filling = true;
                    SetDlgItemInt(window, page.controls[index].command as i32, value, 0);
                    (*shared).filling = false; }
            }
            // Only dependent fields need refresh; rewriting a numeric edit on
            // each keystroke would prevent the user from entering two digits.
            for dependency in &page.controls {
                if let Some((command, expected)) = dependency.enabled_by.filter(|(command, _)| *command == control.command) {
                    let _ = command;
                    unsafe { EnableWindow(GetDlgItem(window, dependency.command as i32), i32::from(!dependency.disabled && (value != 0) == expected)); }
                }
            }
        } else { return 0; }
        {
            unsafe { (*shared).modified = true; }
            unsafe { SendMessageW(GetParent(window), 0x468, window as usize, 0); }
            return 1;
        }
    }
    if message == 0x4e && lparam != 0 {
        let notification = unsafe { &*(lparam as *const Notification) };
        if notification.code as i32 == -202 {
            if !unsafe { (*shared).modified } {
                unsafe { set_data(window, 0, 0); }
                return 1;
            }
            // PropertySheet runs synchronously on the engine/window thread.
            // No other Rust reference to player is used during this call.
            let (player, id, values, title) = {
                let context = unsafe { &*shared };
                (context.player, context.request.id, context.values.clone(), context.request.title.clone())
            };
            let result = unsafe { &mut *player }.apply_host_effect_settings(id, &values)
                .and_then(|()| general::apply_owner(unsafe { &mut *player }, unsafe { (*shared).owner }));
            let invalid = if let Err(error) = result {
                let error = format!("{error:#}");
                unsafe { (*shared).failure = Some(error.clone());
                    MessageBoxW(window, wide(&error).as_ptr(), wide(&title).as_ptr(), 0x10); }
                2
            } else {
                let context = unsafe { &mut *shared };
                context.failure = None; context.modified = false;
                for page in &mut context.values { page.reset = false; }
                0
            };
            unsafe { set_data(window, 0, invalid); }
            return 1;
        }
    }
    0
}
fn template(request: &HostSettingsPage) -> Vec<u32> {
    fn word(bytes: &mut Vec<u8>, value: u16) { bytes.extend_from_slice(&value.to_le_bytes()); }
    fn dword(bytes: &mut Vec<u8>, value: u32) { bytes.extend_from_slice(&value.to_le_bytes()); }
    fn text(bytes: &mut Vec<u8>, value: &str) { for unit in wide(value) { word(bytes, unit); } }
    let mut bytes = Vec::new();
    dword(&mut bytes, 0x40000440); dword(&mut bytes, 0);
    let rows = request.controls.iter().take(request.sound.as_ref().map_or(request.controls.len(), |sound| sound.first_role)).map(|control| match &control.kind {
        HostSettingKind::Toggle => 1, HostSettingKind::Choice(options) => options.len(),
        HostSettingKind::Integer { .. } | HostSettingKind::AutoPreset(_) | HostSettingKind::Decimal { .. } => 2,
    }).sum::<usize>();
    let count = if request.sound.is_some() { 20 } else if request.font.is_some() { 10 } else if request.input.is_some() { 18 } else { rows as u16 + 1 };
    let height = if request.sound.is_some() { 260 } else if request.input.is_some() || request.font.is_some() { 220 } else { rows as u16 * 16 + 28 };
    let width = if request.sound.is_some() || request.input.is_some() || request.font.is_some() { 330 } else { 270 };
    for value in [count, 0, 0, width, height, 0, 0] { word(&mut bytes, value); }
    text(&mut bytes, &request.title); word(&mut bytes, 9); text(&mut bytes, "MS UI Gothic");
    let mut item = |id, label: &str, style, rect: [u16; 4], class: &str| {
        while bytes.len() % 4 != 0 { bytes.push(0); }
        dword(&mut bytes, style); dword(&mut bytes, 0);
        for value in rect { word(&mut bytes, value); }
        word(&mut bytes, id); text(&mut bytes, class);
        text(&mut bytes, label); word(&mut bytes, 0);
    };
    if let Some(sound) = &request.sound {
        for row in 0..5u16 {
            let volume = &request.controls[row as usize * 2];
            let mute = &request.controls[row as usize * 2 + 1];
            let y = 4 + row * 34;
            item(0xffff, &volume.label, 0x50000000, [4, y, 160, 12], "STATIC");
            item(volume.command as u16, "", 0x50832080, [4, y + 14, 88, 14], "EDIT");
            item(mute.command as u16, &mute.label, 0x50010003, [98, y + 14, 66, 14], "BUTTON");
        }
        item(1120, &request.controls[10].label, 0x50010003, [4, 188, 320, 28], "BUTTON");
        item(1114, "", 0x50a10109, [178, 4, 148, 158], "LISTBOX");
        item(1116, &sound.all_on, 0x50010000, [178, 168, 70, 16], "BUTTON");
        item(1118, &sound.all_off, 0x50010000, [256, 168, 70, 16], "BUTTON");
        item(1145, &request.reset_label, 0x50010000, [256, 238, 70, 14], "BUTTON");
        while bytes.len() % 4 != 0 { bytes.push(0); }
        return bytes.chunks_exact(4).map(|bytes| u32::from_le_bytes(bytes.try_into().unwrap())).collect();
    }
    if request.font.is_some() {
        item(0xffff, &request.controls[0].label, 0x50000000, [4, 4, 320, 12], "STATIC");
        item(1121, "", 0x50210003, [4, 18, 320, 160], "COMBOBOX");
        item(1125, "", 0x5000000e, [4, 38, 320, 32], "STATIC");
        item(1126, "", 0x50000000, [4, 72, 320, 24], "STATIC");
        for (index, control) in request.controls[1..].iter().enumerate() {
            item(control.command as u16, &control.label, 0x50010003, [4, 100 + index as u16 * 18, 320, 16], "BUTTON");
        }
        item(1145, &request.reset_label, 0x50010000, [256, 202, 70, 14], "BUTTON");
        while bytes.len() % 4 != 0 { bytes.push(0); }
        return bytes.chunks_exact(4).map(|bytes| u32::from_le_bytes(bytes.try_into().unwrap())).collect();
    }
    if let Some(input) = &request.input {
        item(0xffff, &input.key_label, 0x50000000, [4, 4, 150, 12], "STATIC");
        item(1063, "", 0x50210003, [4, 18, 155, 160], "COMBOBOX");
        item(0xffff, &input.device_label, 0x50000000, [170, 4, 150, 12], "STATIC");
        item(1312, "", 0x50210003, [170, 18, 155, 160], "COMBOBOX");
        for slot in 0..4u16 {
            let y = 48 + slot * 30;
            item(0xffff, &input.binding_labels[slot as usize], 0x50000000, [4, y, 70, 12], "STATIC");
            item(1313 + slot, "", 0x50210003, [75, y - 2, 155, 160], "COMBOBOX");
            item(1317 + slot, &input.repeat_label, 0x50010003, [238, y, 88, 12], "BUTTON");
        }
        item(1327, &request.controls[480].label, 0x50010003, [4, 176, 260, 14], "BUTTON");
        item(1145, &request.reset_label, 0x50010000, [256, height - 18, 70, 14], "BUTTON");
        while bytes.len() % 4 != 0 { bytes.push(0); }
        return bytes.chunks_exact(4).map(|bytes| u32::from_le_bytes(bytes.try_into().unwrap())).collect();
    }
    let mut y = 4;
    for (index, control) in request.controls.iter().enumerate() {
        if request.sound.as_ref().is_some_and(|sound| index >= sound.first_role) { continue; }
        let visible = if control.hidden { 0 } else { 0x10000000 };
        match &control.kind {
            HostSettingKind::Toggle => {
                item(control.command as u16, &control.label, 0x40030003 | visible, [4, y, 260, 14], "BUTTON");
                y += 16;
            }
            HostSettingKind::Choice(options) => {
                for (option, label) in options.iter().enumerate() {
                    item(radio_id(index, option) as u16, label,
                        0x40010009 | visible | if option == 0 { 0x20000 } else { 0 }, [4, y, 260, 14], "BUTTON");
                    y += 16;
                }
            }
            HostSettingKind::Integer { .. } | HostSettingKind::AutoPreset(_) | HostSettingKind::Decimal { .. } => {
                item(0xffff, &control.label, 0x40000000 | visible, [4, y, 260, 12], "STATIC"); y += 16;
                let numeric = if matches!(control.kind, HostSettingKind::Decimal { .. }) { 0 } else { 0x2000 };
                item(control.command as u16, "", 0x40830080 | numeric | visible, [4, y, 260, 14], "EDIT"); y += 16;
            }
        }
    }
    if let Some(sound) = &request.sound {
        item(1114, "", 0x50a10109, [4, y, 260, 84], "LISTBOX");
        item(1116, &sound.all_on, 0x50010000, [4, y + 88, 126, 16], "BUTTON");
        item(1118, &sound.all_off, 0x50010000, [138, y + 88, 126, 16], "BUTTON");
    }
    item(1145, &request.reset_label, 0x50010000, [196, height - 18, 70, 14], "BUTTON");
    while bytes.len() % 4 != 0 { bytes.push(0); }
    bytes.chunks_exact(4).map(|bytes| u32::from_le_bytes(bytes.try_into().unwrap())).collect()
}
pub(super) fn show_settings(player: &mut HeadlessPlayer, window: &Window) -> Result<()> {
    let owner = hwnd(window)?;
    let request = player.host_effect_settings()?;
    let id = request.id;
    let title = wide(&request.title);
    let templates = request.pages.iter().map(template).collect::<Vec<_>>();
    let titles = request.pages.iter().map(|page| wide(&page.title)).collect::<Vec<_>>();
    let values = request.pages.iter().map(|page| HostSettingsEdit { reset: false,
        words: page.controls.iter().map(|control| control.value).collect() }).collect();
    let mut context = Context { player, owner, request, values, modified: false, filling: false, failure: None };
    let contexts = (0..templates.len()).map(|index| Box::new(PageContext { shared: &mut context, index })).collect::<Vec<_>>();
    let mut handles = Vec::new();
    for index in 0..templates.len() {
        let page = Page { size: std::mem::size_of::<Page>() as u32, flags: 1 | 8, instance: 0,
            template: templates[index].as_ptr().cast(), icon: 0, title: titles[index].as_ptr(), procedure: Some(page_proc),
            parameter: contexts[index].as_ref() as *const PageContext as isize, callback: null(), references: null_mut() };
        let handle = unsafe { CreatePropertySheetPageW(&page) };
        if handle == 0 {
            let error = std::io::Error::last_os_error();
            for handle in handles { unsafe { DestroyPropertySheetPage(handle); } }
            bail!("CreatePropertySheetPageW failed: {error}");
        }
        handles.push(handle);
    }
    let sheet = Sheet { size: std::mem::size_of::<Sheet>() as u32, flags: 0, parent: owner,
        instance: 0, icon: 0, caption: title.as_ptr(), count: handles.len() as u32, start: 0, pages: handles.as_ptr(), callback: null() };
    let result = unsafe { PropertySheetW(&sheet) };
    if result == -1 {
        bail!("PropertySheetW failed: {}", std::io::Error::last_os_error());
    }
    player.close_host_effect_settings(id)?;
    if let Some(error) = context.failure { bail!("settings Apply failed: {error}"); }
    Ok(())
}
pub(super) fn confirm_menu(player: &mut HeadlessPlayer, window: &Window) -> Result<()> {
    let request = player.host_menu_confirmation()?;
    // MB_YESNO only, matching491393. No synthetic affirmative result.
    let result = unsafe { MessageBoxW(hwnd(window)?, wide(&request.message).as_ptr(), wide(&request.title).as_ptr(), 4) };
    player.complete_host_menu_confirmation(request.id, result == 6)?;
    if result == 0 { bail!("menu MessageBoxW failed: {}", std::io::Error::last_os_error()); }
    Ok(())
}

pub(super) fn open_system_volume(player: &mut HeadlessPlayer, window: &Window) -> Result<()> {
    #[link(name = "kernel32")]
    unsafe extern "system" { fn GetSystemDirectoryW(path: *mut u16, capacity: u32) -> u32; }
    #[link(name = "shell32")]
    unsafe extern "system" {
        fn ShellExecuteW(window: isize, operation: *const u16, target: *const u16,
            parameters: *const u16, directory: *const u16, show: i32) -> isize;
    }
    let owner = hwnd(window)?;
    let (id, title) = player.host_system_volume_request()?;
    let result = (|| -> Result<()> {
        let mut system = [0u16; 32768];
        let length = unsafe { GetSystemDirectoryW(system.as_mut_ptr(), system.len() as u32) } as usize;
        if length == 0 || length >= system.len() { bail!("system mixer directory lookup failed: {}", std::io::Error::last_os_error()); }
        let mut last = 0;
        //523240..523280 tries these OS tools in this order with show=1.
        // Resolve under the actual system directory so the game root is never
        // searched for an executable with the same name.
        for name in ["sndvol32.exe", "sndvol.exe"] {
            let mut target = system[..length].to_vec();
            if !target.ends_with(&[b'\\' as u16]) { target.push(b'\\' as u16); }
            target.extend(name.encode_utf16()); target.push(0);
            last = unsafe { ShellExecuteW(owner, wide("open").as_ptr(), target.as_ptr(), null(), system.as_ptr(), 1) };
            // Preserve the original comparison, including its accepted32.
            if last >= 32 { return Ok(()); }
        }
        bail!("system mixer launch failed with ShellExecuteW code{last}")
    })();
    if let Err(error) = &result {
        unsafe { MessageBoxW(owner, wide(&format!("{error:#}")).as_ptr(), wide(&title).as_ptr(), 0x10); }
    }
    player.complete_host_system_volume(id, result.map_err(|error| format!("{error:#}")))
}

struct Menu { handle: isize, id: u64, original_proc: isize, attached: bool }
thread_local! {
    static MENUS: RefCell<BTreeMap<isize, Menu>> = RefCell::new(BTreeMap::new());
    static COMMANDS: RefCell<VecDeque<(isize, u64, u32)>> = RefCell::new(VecDeque::new());
}
pub(super) fn observe_menu_command(window: isize, message: u32, wparam: usize, lparam: isize) -> bool {
    if message != 0x111 || lparam != 0 || wparam >> 16 != 0 { return false; }
    let identity = MENUS.with_borrow(|menus| menus.get(&window).map(|menu| menu.id));
    let Some(identity) = identity else { return false; };
    COMMANDS.with_borrow_mut(|commands| commands.push_back((window, identity, wparam as u32 & 65535)));
    true
}
unsafe extern "system" fn menu_window_proc(window: isize, message: u32, wparam: usize, lparam: isize) -> isize {
    let original = MENUS.with_borrow(|menus| menus.get(&window).map(|menu| menu.original_proc));
    if observe_menu_command(window, message, wparam, lparam) { return 0; }
    // Menu selections are normally sent synchronously by Win32. They never
    // enter a queued-MSG hook; this subclass receives the actual WM_COMMAND.
    let result = if let Some(original) = original {
        let original: DialogProc = unsafe { std::mem::transmute(original) };
        unsafe { CallWindowProcW(Some(original), window, message, wparam, lparam) }
    } else { unsafe { DefWindowProcW(window, message, wparam, lparam) } };
    if message == 0x82 {
        let menu = MENUS.with_borrow_mut(|menus| menus.remove(&window));
        // Win32 destroys an attached menu with its window; a fullscreen
        // detached menu remains our property until explicitly released.
        if let Some(menu) = menu.filter(|menu| !menu.attached) { unsafe { DestroyMenu(menu.handle); } }
        COMMANDS.with_borrow_mut(|commands| commands.retain(|command| command.0 != window));
    }
    result
}
fn append(menu: isize, items: &[HostMenuItem]) -> Result<()> {
    for item in items {
        let label = wide(&item.label);
        let mut flags = if item.enabled { 0 } else { 3 } | if item.checked { 8 } else { 0 };
        let command = if !item.children.is_empty() {
            let child = unsafe { CreatePopupMenu() };
            if child == 0 { bail!("CreatePopupMenu failed"); }
            if let Err(error) = append(child, &item.children) { unsafe { DestroyMenu(child); } return Err(error); }
            flags |= 0x10; child as usize
        } else if item.separator { flags |= 0x800; 0 } else { item.command as usize };
        if unsafe { AppendMenuW(menu, flags, command, label.as_ptr()) } == 0 {
            if flags & 0x10 != 0 { unsafe { DestroyMenu(command as isize); } }
            bail!("AppendMenuW failed");
        }
    }
    Ok(())
}
pub(super) fn install_menu(player: &mut HeadlessPlayer, window: &Window) -> Result<()> {
    let request = player.host_game_menu()?;
    let owner = hwnd(window)?;
    let original_proc = MENUS.with_borrow(|menus| menus.get(&owner).map(|menu| menu.original_proc))
        .unwrap_or_else(|| unsafe { get_long(owner, -4) });
    if original_proc == 0 { bail!("the actual window procedure is unavailable"); }
    let handle = unsafe { CreateMenu() };
    if handle == 0 { bail!("CreateMenu failed"); }
    let result = append(handle, &request.items).and_then(|()| {
        if unsafe { SetMenu(owner, if request.visible { handle } else { 0 }) } == 0 { bail!("SetMenu failed"); }
        Ok(())
    });
    if let Err(error) = result { unsafe { DestroyMenu(handle); } return Err(error); }
    let previous = MENUS.with_borrow_mut(|menus| menus.insert(owner, Menu {
        handle, id: request.id, original_proc, attached: request.visible,
    }));
    if previous.is_none() {
        let replaced = unsafe { set_data(owner, -4, menu_window_proc as *const () as isize) };
        if replaced == 0 {
            MENUS.with_borrow_mut(|menus| { menus.remove(&owner); });
            unsafe { SetMenu(owner, 0); DestroyMenu(handle); }
            bail!("install actual menu window-procedure receiver failed");
        }
    }
    if let Some(previous) = previous { unsafe { DestroyMenu(previous.handle); } }
    if unsafe { DrawMenuBar(owner) } == 0 { bail!("DrawMenuBar failed"); }
    player.complete_host_game_menu(request.id)
}
pub(super) fn consume_menu_commands(player: &mut HeadlessPlayer, window: &Window) -> Result<()> {
    let owner = hwnd(window)?;
    let mut commands = COMMANDS.with_borrow_mut(std::mem::take);
    while let Some((target, id, command)) = commands.pop_front() {
        if target == owner { player.host_game_menu_command(id, command)?; }
    }
    Ok(())
}

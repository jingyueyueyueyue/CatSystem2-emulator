//! General107 policy on the actual host HWND, independent of menu creation.
use super::*;

const SUBCLASS: usize = 0x43533247;
type SubclassProc = unsafe extern "system" fn(isize, u32, usize, isize, usize, usize) -> isize;
#[link(name = "comctl32")]
unsafe extern "system" {
    fn SetWindowSubclass(window: isize, procedure: Option<SubclassProc>, id: usize, data: usize) -> i32;
    fn GetWindowSubclass(window: isize, procedure: Option<SubclassProc>, id: usize, data: *mut usize) -> i32;
    fn RemoveWindowSubclass(window: isize, procedure: Option<SubclassProc>, id: usize) -> i32;
    fn DefSubclassProc(window: isize, message: u32, wparam: usize, lparam: isize) -> isize;
}
#[link(name = "user32")]
unsafe extern "system" {
    fn IsIconic(window: isize) -> i32;
    fn GetWindowTextW(window: isize, text: *mut u16, count: i32) -> i32;
    fn SetWindowTextW(window: isize, text: *const u16) -> i32;
    fn DestroyIcon(icon: isize) -> i32;
}
#[link(name = "kernel32")]
unsafe extern "system" { fn GetWindowsDirectoryW(path: *mut u16, count: u32) -> u32; }
#[repr(C)]
struct FileInfo { icon: isize, index: i32, attributes: u32, display: [u16; 260], kind: [u16; 80] }
#[link(name = "shell32")]
unsafe extern "system" {
    fn SHGetFileInfoW(path: *const u16, attributes: u32, info: *mut FileInfo, size: u32, flags: u32) -> usize;
}
struct Policy {
    flags: u32, caption: Vec<u16>, icon: isize, old_icon: isize,
    active: bool, normal_title: Vec<u16>,
}
fn notepad_icon() -> Result<isize> {
    let mut path = vec![0u16; 32768];
    let length = unsafe { GetWindowsDirectoryW(path.as_mut_ptr(), path.len() as u32) } as usize;
    if length == 0 || length >= path.len() { bail!("camouflage Windows directory lookup failed"); }
    path.truncate(length);
    if path.last() != Some(&(b'\\' as u16)) { path.push(b'\\' as u16); }
    path.extend("notepad.exe".encode_utf16()); path.push(0);
    let mut info = FileInfo { icon: 0, index: 0, attributes: 0, display: [0; 260], kind: [0; 80] };
    if unsafe { SHGetFileInfoW(path.as_ptr(), 0, &mut info, std::mem::size_of::<FileInfo>() as u32, 0x101) } == 0 || info.icon == 0 {
        bail!("actual system Notepad icon is unavailable");
    }
    Ok(info.icon)
}
fn present(window: isize, policy: &mut Policy) {
    let active = policy.flags & 2 != 0 && unsafe { IsIconic(window) } != 0;
    if active && !policy.active {
        let mut title = vec![0u16; 32768];
        let length = unsafe { GetWindowTextW(window, title.as_mut_ptr(), title.len() as i32) }.max(0) as usize;
        title.truncate(length); title.push(0);
        policy.normal_title = title;
        policy.old_icon = unsafe { SendMessageW(window, 0x80, 0, policy.icon) };
        unsafe { SetWindowTextW(window, policy.caption.as_ptr()); }
    } else if !active && policy.active {
        unsafe { SendMessageW(window, 0x80, 0, policy.old_icon);
            SetWindowTextW(window, policy.normal_title.as_ptr()); }
    } else if active {
        unsafe { SetWindowTextW(window, policy.caption.as_ptr()); }
    }
    policy.active = active;
}
unsafe extern "system" fn procedure(
    window: isize, message: u32, wparam: usize, lparam: isize, _id: usize, data: usize,
) -> isize {
    let policy = data as *mut Policy;
    if message == 0x112 && wparam & 0xfff0 == 0xf170 && unsafe { (*policy).flags } & 1 != 0 {
        return 1; //492a48..492a6d: SC_MONITORPOWER, not a fabricated input.
    }
    let result = unsafe { DefSubclassProc(window, message, wparam, lparam) };
    if message == 5 { unsafe { present(window, &mut *policy); } }
    if message == 0x82 {
        unsafe { RemoveWindowSubclass(window, Some(procedure), SUBCLASS); }
        let policy = unsafe { Box::from_raw(policy) };
        if policy.icon != 0 { unsafe { DestroyIcon(policy.icon); } }
    }
    result
}
pub(crate) fn apply(player: &mut HeadlessPlayer, window: &Window) -> Result<()> {
    let owner = hwnd(window)?;
    apply_owner(player, owner)
}
pub(crate) fn apply_owner(player: &mut HeadlessPlayer, owner: isize) -> Result<()> {
    let flags = player.host_general_window_policy();
    let mut data = 0;
    let exists = unsafe { GetWindowSubclass(owner, Some(procedure), SUBCLASS, &mut data) } != 0;
    let caption = if flags & 2 != 0 { Some(wide(&player.host_general_camouflage_title()?)) } else { None };
    if exists {
        let policy = unsafe { &mut *(data as *mut Policy) };
        if flags & 2 != 0 && policy.icon == 0 { policy.icon = notepad_icon()?; }
        policy.flags = flags;
        if let Some(caption) = caption { policy.caption = caption; }
        present(owner, policy);
    } else {
        let icon = if flags & 2 != 0 { notepad_icon()? } else { 0 };
        let policy = Box::new(Policy { flags, caption: caption.unwrap_or_else(|| vec![0]),
            icon, old_icon: 0, active: false, normal_title: vec![0] });
        let data = Box::into_raw(policy);
        if unsafe { SetWindowSubclass(owner, Some(procedure), SUBCLASS, data as usize) } == 0 {
            let policy = unsafe { Box::from_raw(data) };
            if policy.icon != 0 { unsafe { DestroyIcon(policy.icon); } }
            bail!("install General settings HWND consumer failed");
        }
        unsafe { present(owner, &mut *data); }
    }
    Ok(())
}
pub(crate) fn camouflaged(window: &Window) -> bool {
    let Ok(owner) = hwnd(window) else { return false; };
    let mut data = 0;
    unsafe { GetWindowSubclass(owner, Some(procedure), SUBCLASS, &mut data) != 0
        && (*(data as *const Policy)).active }
}

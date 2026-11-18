//! Platform UI and wallpaper side effects. Android supplies explicit callbacks;
//! constructing or parsing a wallpaper request never touches the desktop.
use std::path::Path;
use anyhow::{Result, bail};
use super::{WallpaperExportRequest, WallpaperExportSelection};

#[cfg(not(windows))]
pub(super) fn select(_request: &WallpaperExportRequest) -> Result<Option<WallpaperExportSelection>> {
    bail!("wallpaper export needs a platform destination/placement prompt")
}

#[cfg(not(windows))]
pub(super) fn apply(_path: &Path, _placement: i32) -> Result<()> {
    bail!("wallpaper installation needs a platform wallpaper adapter")
}

#[cfg(windows)]
pub(super) fn select(request: &WallpaperExportRequest) -> Result<Option<WallpaperExportSelection>> {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    #[repr(C)]
    struct OpenFileName {
        size: u32, owner: *mut c_void, instance: *mut c_void,
        filter: *const u16, custom_filter: *mut u16, max_custom_filter: u32,
        filter_index: u32, file: *mut u16, max_file: u32,
        file_title: *mut u16, max_file_title: u32, initial_directory: *const u16,
        title: *const u16, flags: u32, file_offset: u16, file_extension: u16,
        default_extension: *const u16, custom_data: isize, hook: usize,
        template_name: *const u16, reserved: *mut c_void, reserved_dword: u32,
        flags_ex: u32,
    }
    #[link(name = "comdlg32")]
    unsafe extern "system" {
        fn GetSaveFileNameW(data: *mut OpenFileName) -> i32;
        fn CommDlgExtendedError() -> u32;
    }
    #[link(name = "user32")]
    unsafe extern "system" {
        fn MessageBoxW(owner: *mut c_void, text: *const u16, caption: *const u16, flags: u32) -> i32;
    }
    let initial: Vec<u16> = request.suggested_path.as_os_str().encode_wide().collect();
    if initial.len() >= 260 { bail!("suggested wallpaper path exceeds 259 UTF-16 units"); }
    if let Some(parent) = request.suggested_path.parent() { std::fs::create_dir_all(parent)?; }
    let mut file = [0u16; 260];
    file[..initial.len()].copy_from_slice(&initial);
    let filter: Vec<u16> = "Bitmap (*.bmp)\0*.bmp\0All files (*.*)\0*.*\0\0".encode_utf16().collect();
    let caption: Vec<u16> = "Export wallpaper\0".encode_utf16().collect();
    let extension: Vec<u16> = "bmp\0".encode_utf16().collect();
    // All zero/null representations in OPENFILENAMEW are valid optional
    // fields. The owned buffers outlive both synchronous Win32 calls.
    let mut data: OpenFileName = unsafe { std::mem::zeroed() };
    data.size = std::mem::size_of::<OpenFileName>() as u32;
    data.filter = filter.as_ptr();
    data.filter_index = 1;
    data.file = file.as_mut_ptr();
    data.max_file = file.len() as u32;
    data.title = caption.as_ptr();
    data.default_extension = extension.as_ptr();
    data.flags = 0x0008_0000 | 0x0000_0800 | 0x0000_0008 | 0x0000_0002;
    if unsafe { GetSaveFileNameW(&mut data) } == 0 {
        let code = unsafe { CommDlgExtendedError() };
        if code != 0 { bail!("wallpaper destination dialog failed: {code:#x}"); }
        return Ok(None);
    }
    let end = file.iter().position(|&unit| unit == 0).unwrap_or(file.len());
    let path = Path::new(&String::from_utf16_lossy(&file[..end])).to_owned();
    let question: Vec<u16> = "Set the exported image as desktop wallpaper?\0".encode_utf16().collect();
    let yes = unsafe { MessageBoxW(std::ptr::null_mut(), question.as_ptr(), caption.as_ptr(), 4) } == 6;
    Ok(Some(WallpaperExportSelection { path, placement: yes.then_some(-1) }))
}

#[cfg(windows)]
pub(super) fn apply(path: &Path, placement: i32) -> Result<()> {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "advapi32")]
    unsafe extern "system" {
        fn RegCreateKeyExW(key: isize, name: *const u16, reserved: u32, class: *mut u16,
            options: u32, access: u32, security: *mut c_void, result: *mut isize,
            disposition: *mut u32) -> i32;
        fn RegSetValueExW(key: isize, name: *const u16, reserved: u32, kind: u32,
            bytes: *const u8, length: u32) -> i32;
        fn RegCloseKey(key: isize) -> i32;
    }
    #[link(name = "user32")]
    unsafe extern "system" {
        fn SystemParametersInfoW(action: u32, parameter: u32, data: *mut c_void, flags: u32) -> i32;
    }
    if placement != -1 {
        let (tile, style) = match placement { 1 => ("0", "0"), 2 => ("1", "0"),
            3 => ("0", "2"), _ => bail!("unsupported wallpaper placement") };
        let key_name: Vec<u16> = "Control Panel\\Desktop\0".encode_utf16().collect();
        let mut key = 0isize;
        let mut disposition = 0u32;
        let code = unsafe { RegCreateKeyExW(-2_147_483_647isize, key_name.as_ptr(), 0,
            std::ptr::null_mut(), 0, 0x0002_0006, std::ptr::null_mut(),
            &mut key, &mut disposition) };
        if code != 0 { bail!("wallpaper registry open failed: {code}"); }
        let result: Result<()> = (|| {
            for (name, value) in [("TileWallpaper", tile), ("WallpaperStyle", style)] {
                let name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
                let value: Vec<u16> = value.encode_utf16().chain(Some(0)).collect();
                let code = unsafe { RegSetValueExW(key, name.as_ptr(), 0, 1,
                    value.as_ptr().cast(), (value.len() * 2) as u32) };
                if code != 0 { bail!("wallpaper registry value failed: {code}"); }
            }
            Ok(())
        })();
        unsafe { RegCloseKey(key); }
        result?;
    }
    let mut name: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    if unsafe { SystemParametersInfoW(20, 0, name.as_mut_ptr().cast(), 3) } == 0 {
        bail!("platform rejected the selected wallpaper")
    }
    Ok(())
}

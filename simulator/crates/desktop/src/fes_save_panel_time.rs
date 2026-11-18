//! Physical file modification date, in the host's local timezone.
use std::{fs, path::Path};
use anyhow::{Context, Result};
#[cfg(not(windows))]
use anyhow::bail;

pub(super) fn modified_date(root: &Path, name: &str) -> Result<Option<[u32; 6]>> {
    let path = crate::save_path(root, name)?;
    let metadata = match fs::metadata(&path) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(||
            format!("SAVEPANEL cannot query modification date of {}", path.display())),
    };
    if !metadata.is_file() { return Ok(None); }
    local_date(&metadata).map(Some)
}

#[cfg(windows)]
fn local_date(metadata: &fs::Metadata) -> Result<[u32; 6]> {
    use std::os::windows::fs::MetadataExt;
    #[repr(C)]
    #[derive(Default)]
    struct FileTime { low: u32, high: u32 }
    #[repr(C)]
    #[derive(Default)]
    struct SystemTime {
        year: u16, month: u16, weekday: u16, day: u16,
        hour: u16, minute: u16, second: u16, milliseconds: u16,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn FileTimeToLocalFileTime(input: *const FileTime, output: *mut FileTime) -> i32;
        fn FileTimeToSystemTime(input: *const FileTime, output: *mut SystemTime) -> i32;
    }
    let ticks = metadata.last_write_time();
    let source = FileTime { low: ticks as u32, high: (ticks >> 32) as u32 };
    let mut local = FileTime::default();
    let mut time = SystemTime::default();
    if unsafe { FileTimeToLocalFileTime(&source, &mut local) } == 0
        || unsafe { FileTimeToSystemTime(&local, &mut time) } == 0
    {
        return Err(std::io::Error::last_os_error()).context("SAVEPANEL local FILETIME conversion failed");
    }
    Ok([time.year, time.month, time.day, time.hour, time.minute, time.second].map(u32::from))
}

#[cfg(all(target_os = "android", target_pointer_width = "64"))]
fn local_date(metadata: &fs::Metadata) -> Result<[u32; 6]> {
    use std::{ffi::{c_char, c_long}, time::UNIX_EPOCH};
    // Same 64-bit Bionic ABI as calendar_native; do not substitute UTC or
    // frame time for the actual modification timestamp.
    #[repr(C)]
    struct Calendar {
        second: i32, minute: i32, hour: i32, day: i32, month: i32, year: i32,
        weekday: i32, yearday: i32, daylight: i32,
        offset: c_long, zone: *const c_char,
    }
    unsafe extern "C" {
        fn localtime_r(seconds: *const i64, output: *mut Calendar) -> *mut Calendar;
    }
    let seconds = match metadata.modified()?.duration_since(UNIX_EPOCH) {
        Ok(duration) => i64::try_from(duration.as_secs())
            .context("SAVEPANEL modification timestamp exceeds time_t")?,
        Err(error) => {
            let duration = error.duration();
            let seconds = i64::try_from(duration.as_secs())
                .context("SAVEPANEL modification timestamp precedes time_t")?;
            seconds.checked_neg().and_then(|value|
                value.checked_sub(i64::from(duration.subsec_nanos() != 0)))
                .context("SAVEPANEL negative modification timestamp exceeds time_t")?
        }
    };
    let mut local = std::mem::MaybeUninit::<Calendar>::uninit();
    if unsafe { localtime_r(&seconds, local.as_mut_ptr()) }.is_null() {
        bail!("SAVEPANEL local modification date conversion failed");
    }
    let local = unsafe { local.assume_init() };
    Ok([local.year.wrapping_add(1900) as u32, local.month.wrapping_add(1) as u32,
        local.day as u32, local.hour as u32, local.minute as u32, local.second as u32])
}

#[cfg(not(any(windows, all(target_os = "android", target_pointer_width = "64"))))]
fn local_date(_metadata: &fs::Metadata) -> Result<[u32; 6]> {
    bail!("SAVEPANEL local file date needs a platform timestamp consumer");
}

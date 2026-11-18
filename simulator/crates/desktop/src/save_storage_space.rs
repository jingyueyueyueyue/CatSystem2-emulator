//! Space on the actual private-save volume. Query before creating directories
//! or truncating an existing CSS file; host failures are never infinite space.
use std::{io, path::{Path, PathBuf}};

pub(super) fn require_available_space(directory: &Path, bytes: usize) -> io::Result<()> {
    let directory = existing_directory(directory)?;
    let available = available_bytes(&directory)?;
    // 6eae57..6eae7d compares a signed high DWORD, then the unsigned low
    // DWORD against the complete compressed CSS length, including its header.
    if available > i64::MAX as u64 || u128::from(available) < bytes as u128 {
        return Err(io::Error::other(format!(
            "insufficient save volume space: {available} available, {bytes} required",
        )));
    }
    Ok(())
}

fn existing_directory(directory: &Path) -> io::Result<PathBuf> {
    let mut directory = std::path::absolute(directory)?;
    loop {
        match std::fs::metadata(&directory) {
            Ok(metadata) if metadata.is_dir() => return Ok(directory),
            Ok(_) => return Err(io::Error::new(
                io::ErrorKind::InvalidInput, "save directory ancestor is not a directory",
            )),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                if !directory.pop() { return Err(error); }
            }
            Err(error) => return Err(error),
        }
    }
}

#[cfg(windows)]
fn available_bytes(directory: &Path) -> io::Result<u64> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetVolumePathNameW(path: *const u16, volume: *mut u16, capacity: u32) -> i32;
        fn GetDiskFreeSpaceW(path: *const u16, sectors: *mut u32, sector_bytes: *mut u32,
            free_clusters: *mut u32, total_clusters: *mut u32) -> i32;
        fn GetDiskFreeSpaceExW(path: *const u16, available: *mut u64,
            total: *mut u64, free: *mut u64) -> i32;
    }
    let mut path: Vec<u16> = directory.as_os_str().encode_wide().collect();
    if path.contains(&0) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "save volume path contains NUL"));
    }
    path.push(0);
    // The original process uses its current volume. The independent player
    // relocates saves, so resolve the volume owning that same private root.
    let mut volume = vec![0u16; 32_768];
    if unsafe { GetVolumePathNameW(path.as_ptr(), volume.as_mut_ptr(), volume.len() as u32) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut sectors = 0;
    let mut sector_bytes = 0;
    let mut free_clusters = 0;
    let mut total_clusters = 0;
    if unsafe { GetDiskFreeSpaceW(volume.as_ptr(), &mut sectors, &mut sector_bytes,
        &mut free_clusters, &mut total_clusters) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut available = 0;
    if unsafe { GetDiskFreeSpaceExW(volume.as_ptr(), &mut available,
        std::ptr::null_mut(), std::ptr::null_mut()) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(available)
}

#[cfg(unix)]
fn available_bytes(directory: &Path) -> io::Result<u64> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};
    let path = CString::new(directory.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "save volume path contains NUL"))?;
    let mut status = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    if unsafe { libc::statvfs(path.as_ptr(), status.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let status = unsafe { status.assume_init() };
    let unit = if status.f_frsize == 0 { status.f_bsize } else { status.f_frsize };
    let bytes = (status.f_bavail as u128).checked_mul(unit as u128)
        .and_then(|bytes| u64::try_from(bytes).ok())
        .ok_or_else(|| io::Error::other("save volume space exceeds the host size range"))?;
    Ok(bytes)
}

#[cfg(not(any(windows, unix)))]
fn available_bytes(_directory: &Path) -> io::Result<u64> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "save volume query has no host implementation"))
}

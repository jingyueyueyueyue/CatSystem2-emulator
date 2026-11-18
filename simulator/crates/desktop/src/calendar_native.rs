//! Ordinary11 returns a single sample of the physical local calendar.
//! Local time is independent of game/task time and is never synthesized from
//! elapsed frames. Outputs retain their sequential, overlapping write order.
use cs2_script::{NativeReturn, Result, Vm};
use super::{LoggingHost, native_state_error, read_u32};

impl LoggingHost<'_> {
    pub(super) fn weekday_native(&self, args: &[u8]) -> Result<NativeReturn> {
        Ok(NativeReturn::Value(cs2_runtime::calendar::weekday(
            read_u32(args, 0)?, read_u32(args, 4)?, read_u32(args, 8)?,
        )))
    }

    pub(super) fn calendar_native(&mut self, args: &[u8], vm: &mut Vm) -> Result<NativeReturn> {
        let mut addresses = [0; 7];
        for (index, address) in addresses.iter_mut().enumerate() {
            *address = read_u32(args, index * 4)?;
            vm.read(*address, 1)?;
        }
        let values = local_calendar()?;
        for (address, value) in addresses.into_iter().zip(values) {
            vm.write(address, &value.to_le_bytes())?;
        }
        Ok(NativeReturn::Continue)
    }
}

#[cfg(windows)]
fn local_calendar() -> Result<[u32; 7]> {
    #[repr(C)]
    #[derive(Default)]
    struct SystemTime {
        year: u16, month: u16, weekday: u16, day: u16,
        hour: u16, minute: u16, second: u16, milliseconds: u16,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetLocalTime(output: *mut SystemTime);
    }
    let mut sample = SystemTime::default();
    unsafe { GetLocalTime(&mut sample); }
    Ok([
        u32::from(sample.year), u32::from(sample.month), u32::from(sample.weekday),
        u32::from(sample.day), u32::from(sample.hour), u32::from(sample.minute),
        u32::from(sample.second),
    ])
}

#[cfg(all(target_os = "android", target_pointer_width = "64"))]
fn local_calendar() -> Result<[u32; 7]> {
    use std::ffi::{c_char, c_long};
    use std::time::{SystemTime, UNIX_EPOCH};
    // The installed NDK's 64-bit Bionic tm layout. The timezone extension is preserved
    // for ABI storage, not dereferenced or cached across timezone changes.
    #[repr(C)]
    struct Calendar {
        second: i32, minute: i32, hour: i32, day: i32, month: i32, year: i32,
        weekday: i32, yearday: i32, daylight: i32,
        offset: c_long, zone: *const c_char,
    }
    unsafe extern "C" {
        fn localtime_r(seconds: *const i64, output: *mut Calendar) -> *mut Calendar;
    }
    let seconds = match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => i64::try_from(duration.as_secs())
            .map_err(|_| native_state_error("local calendar time exceeds time_t"))?,
        Err(error) => {
            let duration = error.duration();
            let whole = i64::try_from(duration.as_secs())
                .map_err(|_| native_state_error("local calendar time precedes time_t"))?;
            -whole - i64::from(duration.subsec_nanos() != 0)
        }
    };
    let mut sample = std::mem::MaybeUninit::<Calendar>::uninit();
    if unsafe { localtime_r(&seconds, sample.as_mut_ptr()) }.is_null() {
        return Err(native_state_error("host local calendar conversion failed"));
    }
    let sample = unsafe { sample.assume_init() };
    Ok([
        sample.year.wrapping_add(1900) as u32, sample.month.wrapping_add(1) as u32,
        sample.weekday as u32, sample.day as u32, sample.hour as u32,
        sample.minute as u32, sample.second as u32,
    ])
}

#[cfg(not(any(windows, all(target_os = "android", target_pointer_width = "64"))))]
fn local_calendar() -> Result<[u32; 7]> {
    Err(native_state_error("local calendar requires a platform calendar adapter"))
}

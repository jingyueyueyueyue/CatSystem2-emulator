//! Android FFmpeg shared-library C API, loaded without changing the JNI shell.
//! AVIO owns only explicit callbacks into a bounded, replayable archive stream.
use std::ffi::{CStr, CString, c_char, c_void};
use std::io::Read;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::NonNull;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use cs2_formats::{MovieMetadata, MovieTimeBase};
use cs2_runtime::RuntimeResult;

use crate::media_process::{Cancellation, error};

const ABI: u32 = 1;
const VIDEO: u32 = 1;
const AUDIO: u32 = 2;
const SEEK_SIZE: i32 = 0x10000;
const SEEK_FORCE: i32 = 0x20000;
const IO_DEADLINE: Duration = Duration::from_secs(20);
const MAX_BYTES: u64 = crate::media_input::MAX_MOVIE_BYTES;

pub(super) type Resolver = Arc<dyn Fn(&str) -> RuntimeResult<Box<dyn Read + Send>> + Send + Sync>;

#[repr(C)]
struct MediaIO {
    opaque: *mut c_void,
    read: unsafe extern "C" fn(*mut c_void, *mut u8, i32) -> i32,
    seek: unsafe extern "C" fn(*mut c_void, i64, i32) -> i64,
    cancelled: unsafe extern "C" fn(*mut c_void) -> i32,
}

#[repr(C)]
#[derive(Default)]
struct MediaInfo {
    abi: u32,
    width: u32,
    height: u32,
    has_audio: u32,
    input_audio_rate: u32,
    input_audio_channels: u32,
    fps_numerator: u32,
    fps_denominator: u32,
    duration_us: i64,
}

#[repr(C)]
struct MediaFrame {
    kind: i32,
    flags: u32,
    pts_seconds: f64,
    width: u32,
    height: u32,
    sample_frames: u32,
    sample_rate: u32,
    channels: u32,
    reserved: u32,
    byte_length: u64,
    data: *const u8,
}

impl Default for MediaFrame {
    fn default() -> Self {
        Self {
            kind: 0,
            flags: 0,
            pts_seconds: 0.,
            width: 0,
            height: 0,
            sample_frames: 0,
            sample_rate: 0,
            channels: 0,
            reserved: 0,
            byte_length: 0,
            data: std::ptr::null(),
        }
    }
}

type Open = unsafe extern "C" fn(
    *const MediaIO,
    *const c_char,
    u32,
    *mut MediaInfo,
    *mut c_char,
    usize,
) -> *mut c_void;
type Decode = unsafe extern "C" fn(*mut c_void, *mut MediaFrame) -> i32;
type Close = unsafe extern "C" fn(*mut c_void);
type Diagnostic = unsafe extern "C" fn(*const c_void) -> *const c_char;

struct Api {
    _library: NonNull<c_void>,
    open: Open,
    read: Decode,
    close: Close,
    diagnostic: Diagnostic,
}

// dlopen handles and immutable function pointers remain valid for process
// lifetime. Individual decoder/context ownership remains one worker thread.
unsafe impl Send for Api {}
unsafe impl Sync for Api {}

#[link(name = "dl")]
unsafe extern "C" {
    fn dlopen(path: *const c_char, flags: i32) -> *mut c_void;
    fn dlsym(library: *mut c_void, symbol: *const c_char) -> *mut c_void;
    fn dlerror() -> *const c_char;
    fn dlclose(library: *mut c_void) -> i32;
}

static API: OnceLock<Result<Api, String>> = OnceLock::new();

fn api() -> RuntimeResult<&'static Api> {
    API.get_or_init(|| unsafe { load_api() })
        .as_ref()
        .map_err(|cause| error(cause))
}

pub(super) fn available() -> bool {
    api().is_ok()
}

unsafe fn load_api() -> Result<Api, String> {
    let library = NonNull::new(unsafe { dlopen(c"libcs2_media.so".as_ptr(), 2) })
        .ok_or_else(|| format!(
            "Android native movie backend is unavailable: {}; package arm64 libcs2_media and the pinned FFmpeg shared libraries. Windows FFmpeg executables cannot run in an APK",
            loader_error(),
        ))?;
    let result = (|| {
        macro_rules! symbol {
            ($name:literal, $ty:ty) => {{
                let pointer =
                    unsafe { dlsym(library.as_ptr(), concat!($name, "\0").as_ptr().cast()) };
                if pointer.is_null() {
                    return Err(format!(
                        "Android media ABI symbol {} missing: {}",
                        $name,
                        loader_error()
                    ));
                }
                unsafe { std::mem::transmute::<*mut c_void, $ty>(pointer) }
            }};
        }
        let version = symbol!("cs2_media_abi_version", unsafe extern "C" fn() -> u32);
        let info_size = symbol!("cs2_media_info_size", unsafe extern "C" fn() -> u32);
        let frame_size = symbol!("cs2_media_frame_size", unsafe extern "C" fn() -> u32);
        if unsafe { version() } != ABI
            || unsafe { info_size() } as usize != std::mem::size_of::<MediaInfo>()
            || unsafe { frame_size() } as usize != std::mem::size_of::<MediaFrame>()
        {
            return Err("Android media C/Rust ABI version or record size mismatch".into());
        }
        Ok(Api {
            _library: library,
            open: symbol!("cs2_media_open", Open),
            read: symbol!("cs2_media_read", Decode),
            close: symbol!("cs2_media_close", Close),
            diagnostic: symbol!("cs2_media_last_error", Diagnostic),
        })
    })();
    if result.is_err() {
        unsafe {
            dlclose(library.as_ptr());
        }
    }
    result
}

fn loader_error() -> String {
    let pointer = unsafe { dlerror() };
    if pointer.is_null() {
        "no dynamic-loader diagnostic".into()
    } else {
        unsafe { CStr::from_ptr(pointer) }
            .to_string_lossy()
            .into_owned()
    }
}

/// Runtime's decrypting archive reader is forward-only. AVIO backward seeks
/// reopen the same indexed resource and decrypt/discard to the requested byte.
/// No whole-movie allocation, cache file or guest process is needed.
struct ResourceIO {
    resolve: Resolver,
    name: String,
    reader: Box<dyn Read + Send>,
    position: u64,
    length: Option<u64>,
    cancel: Cancellation,
    deadline: Instant,
    diagnostic: Option<String>,
}

impl ResourceIO {
    fn read(&mut self, output: &mut [u8]) -> RuntimeResult<usize> {
        self.check()?;
        if self.length.is_some_and(|length| self.position == length) {
            return Ok(0);
        }
        let remaining = MAX_BYTES.saturating_sub(self.position);
        if remaining == 0 {
            let mut extra = [0_u8; 1];
            if self
                .reader
                .read(&mut extra)
                .map_err(|cause| error(format!("read media bound: {cause}")))?
                != 0
            {
                return Err(error("Android encoded movie exceeds 4 GiB"));
            }
            self.length = Some(self.position);
            return Ok(0);
        }
        let remaining = self.length.map_or(remaining, |length| {
            remaining.min(length.saturating_sub(self.position))
        });
        let count = output.len().min(remaining as usize);
        let read = loop {
            match self.reader.read(&mut output[..count]) {
                Err(cause) if cause.kind() == std::io::ErrorKind::Interrupted => {
                    self.check()?;
                }
                result => {
                    break result
                        .map_err(|cause| error(format!("read Android archive movie: {cause}")))?;
                }
            }
        };
        self.position += read as u64;
        if read == 0 {
            if self.length.is_some_and(|length| length != self.position) {
                return Err(error(
                    "Android archive movie ended before its indexed length",
                ));
            }
            self.length = Some(self.position);
        }
        Ok(read)
    }

    fn check(&self) -> RuntimeResult<()> {
        if self.cancel.cancelled() {
            return Err(error("Android movie read cancelled"));
        }
        if Instant::now() >= self.deadline {
            return Err(error("Android movie AVIO operation exceeded20s"));
        }
        Ok(())
    }

    fn seek(&mut self, offset: i64, whence: i32) -> RuntimeResult<Option<u64>> {
        self.check()?;
        let whence = whence & !SEEK_FORCE;
        if whence == SEEK_SIZE || (whence == 2 && self.length.is_none()) {
            if self.length.is_none() {
                let mut reader = (self.resolve)(&self.name)?;
                let mut length = 0_u64;
                let mut buffer = [0_u8; 64 * 1024];
                loop {
                    self.check()?;
                    let read = reader
                        .read(&mut buffer)
                        .map_err(|cause| error(format!("size Android archive movie: {cause}")))?;
                    if read == 0 {
                        break;
                    }
                    length = length
                        .checked_add(read as u64)
                        .filter(|value| *value <= MAX_BYTES)
                        .ok_or_else(|| error("Android encoded movie exceeds 4 GiB"))?;
                }
                self.length = Some(length);
            }
            if whence == SEEK_SIZE {
                return Ok(self.length);
            }
        }
        let target = match whence {
            0 => u64::try_from(offset).ok(),
            1 => self.position.checked_add_signed(offset),
            2 => self
                .length
                .and_then(|length| length.checked_add_signed(offset)),
            _ => None,
        };
        let Some(target) = target else {
            return Ok(None);
        };
        if target > MAX_BYTES || self.length.is_some_and(|length| target > length) {
            return Err(error("Android movie seek leaves its bounded resource"));
        }
        if target < self.position {
            self.reader = (self.resolve)(&self.name)?;
            self.position = 0;
        }
        let mut discard = [0_u8; 64 * 1024];
        while self.position < target {
            let count = (target - self.position).min(discard.len() as u64) as usize;
            if self.read(&mut discard[..count])? == 0 {
                return Err(error(
                    "Android movie seek reached EOF before its requested position",
                ));
            }
        }
        Ok(Some(self.position))
    }
}

unsafe extern "C" fn read_callback(opaque: *mut c_void, output: *mut u8, capacity: i32) -> i32 {
    if opaque.is_null() || output.is_null() || capacity <= 0 {
        return -1;
    }
    let io = unsafe { &mut *opaque.cast::<ResourceIO>() };
    let result = catch_unwind(AssertUnwindSafe(|| {
        io.read(unsafe { std::slice::from_raw_parts_mut(output, capacity as usize) })
    }));
    match result {
        Ok(Ok(read)) => read as i32,
        Ok(Err(cause)) => {
            io.diagnostic = Some(cause.to_string());
            if io.cancel.cancelled() { -2 } else { -1 }
        }
        Err(_) => {
            io.diagnostic = Some("panic in Android movie read callback".into());
            -1
        }
    }
}

unsafe extern "C" fn seek_callback(opaque: *mut c_void, offset: i64, whence: i32) -> i64 {
    if opaque.is_null() {
        return -1;
    }
    let io = unsafe { &mut *opaque.cast::<ResourceIO>() };
    match catch_unwind(AssertUnwindSafe(|| io.seek(offset, whence))) {
        Ok(Ok(Some(position))) => position as i64,
        Ok(Ok(None)) => -3,
        Ok(Err(cause)) => {
            io.diagnostic = Some(cause.to_string());
            if io.cancel.cancelled() { -2 } else { -1 }
        }
        Err(_) => {
            io.diagnostic = Some("panic in Android movie seek callback".into());
            -1
        }
    }
}

unsafe extern "C" fn cancelled_callback(opaque: *mut c_void) -> i32 {
    if opaque.is_null() {
        return 1;
    }
    let io = unsafe { &*opaque.cast::<ResourceIO>() };
    i32::from(io.cancel.cancelled() || Instant::now() >= io.deadline)
}

pub(super) enum Frame {
    Video {
        width: u32,
        height: u32,
        time: f64,
        duration: Option<f64>,
        rgba: Vec<u8>,
    },
    Audio {
        time: f64,
        samples: Vec<i16>,
    },
}

pub(super) struct Decoder {
    handle: NonNull<c_void>,
    api: &'static Api,
    io: Box<ResourceIO>,
}

impl Decoder {
    pub(super) fn open(
        resolve: Resolver,
        name: &str,
        length: Option<u64>,
        demuxer: &str,
        mask: u32,
        cancel: Cancellation,
    ) -> RuntimeResult<(Self, MovieMetadata, bool)> {
        let api = api()?;
        let reader = resolve(name)?;
        let mut io = Box::new(ResourceIO {
            resolve,
            name: name.to_owned(),
            reader,
            position: 0,
            length,
            cancel,
            deadline: Instant::now() + IO_DEADLINE,
            diagnostic: None,
        });
        let callbacks = MediaIO {
            opaque: (&mut *io as *mut ResourceIO).cast(),
            read: read_callback,
            seek: seek_callback,
            cancelled: cancelled_callback,
        };
        let mut info = MediaInfo::default();
        let mut diagnostic: [c_char; 1024] = [0; 1024];
        let demuxer = CString::new(demuxer).map_err(|_| error("invalid Android demuxer name"))?;
        let handle = NonNull::new(unsafe {
            (api.open)(
                &callbacks,
                demuxer.as_ptr(),
                mask,
                &mut info,
                diagnostic.as_mut_ptr(),
                diagnostic.len(),
            )
        })
        .ok_or_else(|| {
            error(format!(
                "Android movie open: {}; {}",
                unsafe { CStr::from_ptr(diagnostic.as_ptr()) }.to_string_lossy(),
                io.diagnostic.as_deref().unwrap_or("AVIO stream"),
            ))
        })?;
        let decoder = Self { handle, api, io };
        if let Some(cause) = &decoder.io.diagnostic {
            return Err(error(format!(
                "Android movie probe resource error: {cause}"
            )));
        }
        if info.abi != ABI
            || info.width == 0
            || info.height == 0
            || u64::from(info.width) * u64::from(info.height) > 16_777_216
        {
            return Err(error("Android decoder returned invalid video metadata"));
        }
        let width = u16::try_from(info.width)
            .map_err(|_| error("Android movie width exceeds core metadata"))?;
        let height = u16::try_from(info.height)
            .map_err(|_| error("Android movie height exceeds core metadata"))?;
        let metadata = MovieMetadata {
            width: Some(width),
            height: Some(height),
            time_base: (info.fps_numerator != 0 && info.fps_denominator != 0).then_some(
                MovieTimeBase {
                    numerator: info.fps_denominator,
                    denominator: info.fps_numerator,
                },
            ),
            duration_seconds: (info.duration_us >= 0)
                .then_some(info.duration_us as f64 / 1_000_000.),
            ..Default::default()
        };
        Ok((decoder, metadata, info.has_audio != 0))
    }

    pub(super) fn next(&mut self) -> RuntimeResult<Option<Frame>> {
        self.io.deadline = Instant::now() + IO_DEADLINE;
        self.io.diagnostic = None;
        let mut frame = MediaFrame::default();
        let result = unsafe { (self.api.read)(self.handle.as_ptr(), &mut frame) };
        if let Some(cause) = &self.io.diagnostic {
            return Err(error(format!(
                "Android movie decoder resource error: {cause}"
            )));
        }
        if result == 0 {
            return Ok(None);
        }
        if result < 0 {
            let text = unsafe { (self.api.diagnostic)(self.handle.as_ptr()) };
            let text = if text.is_null() {
                "no C decoder diagnostic".into()
            } else {
                unsafe { CStr::from_ptr(text) }
                    .to_string_lossy()
                    .into_owned()
            };
            return Err(error(format!(
                "Android movie decode: {text}; {}",
                self.io.diagnostic.as_deref().unwrap_or("AVIO stream")
            )));
        }
        if result != 1 || !frame.pts_seconds.is_finite() || frame.data.is_null() {
            return Err(error(
                "Android decoder returned invalid frame ownership/PTS",
            ));
        }
        match frame.kind {
            1 => {
                let expected = u64::from(frame.width) * u64::from(frame.height) * 4;
                if frame.width == 0
                    || frame.height == 0
                    || expected > 64 * 1024 * 1024
                    || frame.byte_length != expected
                {
                    return Err(error("Android decoded RGBA frame is invalid or oversized"));
                }
                let bytes = unsafe { std::slice::from_raw_parts(frame.data, expected as usize) };
                Ok(Some(Frame::Video {
                    width: frame.width,
                    height: frame.height,
                    time: frame.pts_seconds,
                    duration: (frame.flags & 8 != 0 && frame.reserved != 0)
                        .then_some(f64::from(frame.reserved) / 1_000_000.),
                    rgba: bytes.to_vec(),
                }))
            }
            2 => {
                let expected = u64::from(frame.sample_frames) * 4;
                if frame.channels != 2
                    || frame.sample_rate != 48_000
                    || frame.sample_frames == 0
                    || expected > 1024 * 1024
                    || frame.byte_length != expected
                {
                    return Err(error("Android decoded PCM frame is invalid or oversized"));
                }
                let bytes = unsafe { std::slice::from_raw_parts(frame.data, expected as usize) };
                let samples = bytes
                    .chunks_exact(2)
                    .map(|sample| i16::from_ne_bytes([sample[0], sample[1]]))
                    .collect();
                Ok(Some(Frame::Audio {
                    time: frame.pts_seconds,
                    samples,
                }))
            }
            _ => Err(error("Android decoder returned an unknown frame kind")),
        }
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        unsafe {
            (self.api.close)(self.handle.as_ptr());
        }
    }
}

pub(super) fn probe(
    resolve: Resolver,
    name: &str,
    length: Option<u64>,
    format: super::movie::MovieFormat,
) -> RuntimeResult<(MovieMetadata, bool)> {
    let (_, metadata, audio) = Decoder::open(
        resolve,
        name,
        length,
        format.demuxer_name(),
        VIDEO | AUDIO,
        Cancellation::in_process(),
    )?;
    Ok((metadata, audio))
}

pub(super) fn signature(resolve: &Resolver, name: &str) -> RuntimeResult<Vec<u8>> {
    let mut reader = resolve(name)?;
    let mut prefix = vec![0; 64];
    let mut read = 0;
    while read < prefix.len() {
        let count = reader
            .read(&mut prefix[read..])
            .map_err(|cause| error(format!("read Android movie signature: {cause}")))?;
        if count == 0 {
            break;
        }
        read += count;
    }
    prefix.truncate(read);
    Ok(prefix)
}

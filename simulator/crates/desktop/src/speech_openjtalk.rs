//! Independent, owned Open JTalk data/provider backend for Windows/Android.
//! Only our cs2_speech bridge is loaded; original game DLLs are never used.
use std::ffi::{CStr, CString, c_char, c_void};
use std::path::{Path, PathBuf};
use std::ptr::NonNull;
use std::sync::{Arc, Mutex, OnceLock};

use cs2_runtime::{RuntimeError, RuntimeResult};
use super::sound_speech::{
    SpeechConfiguration, SpeechConfigurationHost, SpeechPcm, SpeechSynthesisParameters,
};

static SERIAL: Mutex<()> = Mutex::new(());
static API: OnceLock<Arc<Api>> = OnceLock::new();

#[repr(C)]
struct Pcm {
    rate: u32,
    channels: u32,
    count: usize,
    samples: *mut i16,
}

struct Api {
    _library: NonNull<c_void>,
    open: unsafe extern "C" fn(*const c_char, *const c_char, *mut c_char, usize) -> *mut c_void,
    close: unsafe extern "C" fn(*mut c_void),
    set_parameters: unsafe extern "C" fn(*mut c_void, f64, f64, *mut c_char, usize) -> i32,
    synthesize: unsafe extern "C" fn(*mut c_void, *const c_char, usize, *mut Pcm, *mut c_char, usize) -> i32,
    free_pcm: unsafe extern "C" fn(*mut Pcm),
}
// Immutable API and permanently retained library; every engine call is
// serialized, including destroy, independently of the calling platform.
unsafe impl Send for Api {}
unsafe impl Sync for Api {}

pub(super) struct OpenJtalkProvider {
    game_root: PathBuf,
    data_root: PathBuf,
    instance: Option<(Arc<Api>, NonNull<c_void>)>,
}
unsafe impl Send for OpenJtalkProvider {}

fn error(message: impl Into<String>) -> RuntimeError {
    RuntimeError::Host(message.into())
}

impl OpenJtalkProvider {
    pub(super) fn new(game_root: PathBuf, data_root: PathBuf) -> Self {
        Self { game_root, data_root, instance: None }
    }

    fn paths(&self, configuration: &SpeechConfiguration) -> RuntimeResult<(PathBuf, PathBuf)> {
        let resolve = |value: &str| {
            let path = PathBuf::from(value.replace('\\', "/"));
            if path.is_absolute() { path } else { self.game_root.join(path) }
        };
        let paths = [&configuration.native_first, &configuration.native_second];
        let mut dictionary = None;
        let mut voice = None;
        for value in paths {
            if value.is_empty() { continue; }
            let path = resolve(value);
            if path.is_dir() {
                if dictionary.replace(path).is_some() {
                    return Err(error("sound228 supplied two dictionary directories"));
                }
            } else if path.is_file() {
                if voice.replace(path).is_some() {
                    return Err(error("sound228 supplied two voice files"));
                }
            } else {
                return Err(error(format!("sound228 data source does not exist: {}", path.display())));
            }
        }
        let dictionary = dictionary.unwrap_or_else(|| self.data_root.join("dictionary"));
        let voice = voice.unwrap_or_else(|| self.data_root.join("nitech_jp_atr503_m001.htsvoice"));
        for name in ["char.bin", "matrix.bin", "sys.dic", "unk.dic"] {
            let path = dictionary.join(name);
            let metadata = std::fs::metadata(&path)
                .map_err(|cause| error(format!("speech dictionary {}: {cause}", path.display())))?;
            if !metadata.is_file() || metadata.len() == 0 || metadata.len() > 256 * 1024 * 1024 {
                return Err(error("speech dictionary component is empty or exceeds256 MiB"));
            }
        }
        let metadata = std::fs::metadata(&voice)
            .map_err(|cause| error(format!("speech voice {}: {cause}", voice.display())))?;
        if !metadata.is_file() || metadata.len() == 0 || metadata.len() > 64 * 1024 * 1024 {
            return Err(error("speech HTS voice is empty or exceeds64 MiB"));
        }
        Ok((dictionary, voice))
    }
}

impl SpeechConfigurationHost for OpenJtalkProvider {
    fn configure(&mut self, configuration: &SpeechConfiguration) -> RuntimeResult<()> {
        let (dictionary, voice) = self.paths(configuration)?;
        let dictionary = platform::dictionary_path(&dictionary)?;
        let voice = path_string(&voice)?;
        let _guard = SERIAL.lock().map_err(|_| error("speech engine lock was poisoned"))?;
        let api = if let Some(api) = API.get() {
            api.clone()
        } else {
            let api = load_api().map_err(error)?;
            // SERIAL serializes all loaders as well. Failed loads are not
            // cached: a platform can finish mounting/installing the package.
            let _ = API.set(api.clone());
            api
        };
        let mut diagnostic: [c_char; 1024] = [0; 1024];
        let instance = NonNull::new(unsafe {
            (api.open)(dictionary.as_ptr(), voice.as_ptr(), diagnostic.as_mut_ptr(), diagnostic.len())
        }).ok_or_else(|| error(message(&diagnostic)))?;
        if let Some((old_api, old)) = self.instance.replace((api, instance)) {
            unsafe { (old_api.close)(old.as_ptr()); }
        }
        Ok(())
    }

    fn synthesize(&mut self, text: &str) -> RuntimeResult<SpeechPcm> {
        self.synthesize_with_parameters(text, SpeechSynthesisParameters::default())
    }

    fn synthesize_with_parameters(
        &mut self, text: &str, parameters: SpeechSynthesisParameters,
    ) -> RuntimeResult<SpeechPcm> {
        let parameters = parameters.validate()?;
        if text.is_empty() || text.len() > 4096 {
            return Err(error("speech text must contain1..4096 UTF-8 bytes"));
        }
        let input = CString::new(text).map_err(|_| error("speech text contains an innerNUL"))?;
        let (api, instance) = self.instance.as_ref()
            .ok_or_else(|| error("speech provider has not loaded dictionary and voice data"))?;
        let _guard = SERIAL.lock().map_err(|_| error("speech engine lock was poisoned"))?;
        let mut pcm = Pcm { rate: 0, channels: 0, count: 0, samples: std::ptr::null_mut() };
        let mut diagnostic: [c_char; 1024] = [0; 1024];
        if unsafe {
            (api.set_parameters)(instance.as_ptr(), f64::from(parameters.speed),
                                 f64::from(parameters.tone),
                                 diagnostic.as_mut_ptr(), diagnostic.len())
        } != 0 {
            return Err(error(message(&diagnostic)));
        }
        let status = unsafe {
            (api.synthesize)(instance.as_ptr(), input.as_ptr(), text.len(), &mut pcm,
                             diagnostic.as_mut_ptr(), diagnostic.len())
        };
        let result = (|| {
            if status != 0 { return Err(error(message(&diagnostic))); }
            if pcm.samples.is_null() || pcm.channels != 1 || pcm.rate == 0 || pcm.rate > 192_000
                || pcm.count == 0 || pcm.count > 32 * 1024 * 1024
                || pcm.count as u64 > u64::from(pcm.rate) * 60
            {
                return Err(error("speech bridge returned invalid PCM metadata"));
            }
            let samples = unsafe { std::slice::from_raw_parts(pcm.samples, pcm.count) }.to_vec();
            Ok(SpeechPcm { sample_rate: pcm.rate, channels: 1, samples: samples.into() })
        })();
        unsafe { (api.free_pcm)(&mut pcm); }
        result
    }
}

impl Drop for OpenJtalkProvider {
    fn drop(&mut self) {
        if let Some((api, instance)) = self.instance.take() {
            let _guard = SERIAL.lock().unwrap_or_else(|poison| poison.into_inner());
            unsafe { (api.close)(instance.as_ptr()); }
        }
    }
}

fn path_string(path: &Path) -> RuntimeResult<CString> {
    CString::new(path.to_str().ok_or_else(|| error("speech data path is not Unicode"))?)
        .map_err(|_| error("speech data path containsNUL"))
}

fn message(bytes: &[c_char; 1024]) -> String {
    let end = bytes.iter().position(|&byte| byte == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end].iter().map(|&byte| byte as u8).collect::<Vec<_>>()).into_owned()
}

fn load_api() -> std::result::Result<Arc<Api>, String> {
    let library = platform::load()?;
    macro_rules! symbol {
        ($name:literal, $ty:ty) => {{
            let pointer = platform::symbol(library, concat!($name, "\0").as_bytes())?;
            unsafe { std::mem::transmute::<*mut c_void, $ty>(pointer) }
        }};
    }
    let result = (|| {
        let abi = symbol!("cs2_speech_abi_version", unsafe extern "C" fn() -> u32);
        let size = symbol!("cs2_speech_pcm_size", unsafe extern "C" fn() -> u32);
        if unsafe { abi() } != 2 || unsafe { size() } as usize != std::mem::size_of::<Pcm>() {
            return Err("speech C/Rust ABI mismatch".into());
        }
        Ok(Arc::new(Api {
            _library: library,
            open: symbol!("cs2_speech_open", unsafe extern "C" fn(*const c_char, *const c_char, *mut c_char, usize) -> *mut c_void),
            close: symbol!("cs2_speech_close", unsafe extern "C" fn(*mut c_void)),
            set_parameters: symbol!("cs2_speech_set_parameters", unsafe extern "C" fn(*mut c_void, f64, f64, *mut c_char, usize) -> i32),
            synthesize: symbol!("cs2_speech_synthesize", unsafe extern "C" fn(*mut c_void, *const c_char, usize, *mut Pcm, *mut c_char, usize) -> i32),
            free_pcm: symbol!("cs2_speech_free_pcm", unsafe extern "C" fn(*mut Pcm)),
        }))
    })();
    if result.is_err() { platform::close(library); }
    result
}

#[cfg(windows)]
mod platform {
    use super::*;
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LoadLibraryExW(path: *const u16, file: *mut c_void, flags: u32) -> *mut c_void;
        fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
        fn FreeLibrary(module: *mut c_void) -> i32;
        fn GetACP() -> u32;
        fn GetShortPathNameW(path: *const u16, output: *mut u16, length: u32) -> u32;
        fn WideCharToMultiByte(page: u32, flags: u32, source: *const u16, length: i32,
            output: *mut u8, capacity: i32, default: *const u8, substituted: *mut i32) -> i32;
    }
    pub(super) fn dictionary_path(path: &Path) -> RuntimeResult<CString> {
        // Open JTalk's fixed MeCab fork uses CreateFileA and ANSI fstreams,
        // unlike HTS's independently wrapped UTF-8 fopen. Never pass it
        // lossy UTF-8 as an ACP path. Prefer an existing ASCII short path.
        if unsafe { GetACP() } == 65001 {
            return path_string(path);
        }
        let wide: Vec<_> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut short = vec![0u16; 32768];
        let count = unsafe { GetShortPathNameW(wide.as_ptr(), short.as_mut_ptr(), short.len() as u32) };
        if count != 0 && count < short.len() as u32
            && short[..count as usize].iter().all(|&unit| unit < 128)
        {
            return CString::new(short[..count as usize].iter().map(|&unit| unit as u8).collect::<Vec<_>>())
                .map_err(|_| error("speech dictionary short path containsNUL"));
        }
        let mut output = vec![0u8; 65536];
        let mut substituted = 0;
        let count = unsafe { WideCharToMultiByte(0, 0x400, wide.as_ptr(), -1,
            output.as_mut_ptr(), output.len() as i32, std::ptr::null(), &mut substituted) };
        if count == 0 || substituted != 0 {
            return Err(error("speech dictionary path needs an ASCII short path or exact Windows ACP representation"));
        }
        output.truncate(count as usize - 1);
        CString::new(output).map_err(|_| error("speech dictionary ACP path containsNUL"))
    }
    pub(super) fn load() -> std::result::Result<NonNull<c_void>, String> {
        let path = std::env::current_exe().map_err(|cause| cause.to_string())?
            .with_file_name("cs2_speech.dll");
        let path: Vec<_> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        NonNull::new(unsafe { LoadLibraryExW(path.as_ptr(), std::ptr::null_mut(), 0x1100) })
            .ok_or_else(|| format!("load independently built cs2_speech.dll: {}", std::io::Error::last_os_error()))
    }
    pub(super) fn symbol(library: NonNull<c_void>, name: &[u8]) -> std::result::Result<*mut c_void, String> {
        let pointer = unsafe { GetProcAddress(library.as_ptr(), name.as_ptr()) };
        if pointer.is_null() { Err("independent speech bridge symbol is missing".into()) } else { Ok(pointer) }
    }
    pub(super) fn close(library: NonNull<c_void>) { unsafe { FreeLibrary(library.as_ptr()); } }
}

#[cfg(target_os = "android")]
mod platform {
    use super::*;
    #[link(name = "dl")]
    unsafe extern "C" {
        fn dlopen(name: *const c_char, flags: i32) -> *mut c_void;
        fn dlsym(library: *mut c_void, name: *const c_char) -> *mut c_void;
        fn dlclose(library: *mut c_void) -> i32;
        fn dlerror() -> *const c_char;
    }
    fn diagnostic() -> String {
        let value = unsafe { dlerror() };
        if value.is_null() { "no loader diagnostic".into() }
        else { unsafe { CStr::from_ptr(value) }.to_string_lossy().into_owned() }
    }
    pub(super) fn dictionary_path(path: &Path) -> RuntimeResult<CString> { path_string(path) }
    pub(super) fn load() -> std::result::Result<NonNull<c_void>, String> {
        NonNull::new(unsafe { dlopen(c"libcs2_speech.so".as_ptr(), 2) })
            .ok_or_else(|| format!("load independent speech bridge: {}", diagnostic()))
    }
    pub(super) fn symbol(library: NonNull<c_void>, name: &[u8]) -> std::result::Result<*mut c_void, String> {
        let pointer = unsafe { dlsym(library.as_ptr(), name.as_ptr().cast()) };
        if pointer.is_null() { Err(format!("speech symbol: {}", diagnostic())) } else { Ok(pointer) }
    }
    pub(super) fn close(library: NonNull<c_void>) { unsafe { dlclose(library.as_ptr()); } }
}

#[cfg(not(any(windows, target_os = "android")))]
mod platform {
    use super::*;
    pub(super) fn load() -> std::result::Result<NonNull<c_void>, String> {
        Err("independent speech provider currently supports Windows/Android".into())
    }
    pub(super) fn dictionary_path(path: &Path) -> RuntimeResult<CString> { path_string(path) }
    pub(super) fn symbol(_: NonNull<c_void>, _: &[u8]) -> std::result::Result<*mut c_void, String> {
        Err("speech provider has no loader on this platform".into())
    }
    pub(super) fn close(_: NonNull<c_void>) {}
}

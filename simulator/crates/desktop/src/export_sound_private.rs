//! Application-private sound scheme storage and actual host PCM playback.
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;

use anyhow::{Context, Result, bail};
use cs2_runtime::export_sound_library::{ApplicationSound, ApplicationSoundLibrary};
use cs2_runtime::{AudioChannel, AudioHost, AudioRequest};

use super::PreparedScheme;

const FILENAME: &str = "sound-schemes.json";
const MAX_LIBRARY: u64 = 4 * 1024 * 1024;
const MAX_AUDIO: u64 = 64 * 1024 * 1024;

pub(super) fn load(root: &Path) -> Result<ApplicationSoundLibrary> {
    let path = root.join(FILENAME);
    let file = match File::open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ApplicationSoundLibrary::default());
        }
        Err(error) => return Err(error).with_context(|| format!("open {}", path.display())),
    };
    let mut bytes = Vec::new();
    file.take(MAX_LIBRARY + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_LIBRARY {
        bail!("application sound library exceeds 4 MiB");
    }
    Ok(ApplicationSoundLibrary::decode(&bytes)?)
}

fn save(root: &Path, library: &ApplicationSoundLibrary) -> Result<()> {
    let bytes = library.encode()?;
    std::fs::create_dir_all(root)?;
    let temporary = root.join("sound-schemes.pending");
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temporary)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    drop(file);
    replace(&temporary, &root.join(FILENAME))?;
    Ok(())
}

#[cfg(not(windows))]
fn replace(from: &Path, to: &Path) -> Result<()> {
    std::fs::rename(from, to)?;
    Ok(())
}
#[cfg(windows)]
fn replace(from: &Path, to: &Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(from: *const u16, to: *const u16, flags: u32) -> i32;
    }
    let from: Vec<_> = from.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<_> = to.as_os_str().encode_wide().chain(Some(0)).collect();
    if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), 1 | 8) } == 0 {
        return Err(std::io::Error::last_os_error()).context("replace application sound library");
    }
    Ok(())
}

pub(super) fn register(root: &Path, scheme: &PreparedScheme) -> Result<()> {
    let mut library = load(root)?;
    let events = scheme.events.iter().map(|(name, path)| {
        let sound = path.as_ref().map(|path| {
            if path.is_empty() {
                ApplicationSound::Mute
            } else {
                ApplicationSound::File(path.into())
            }
        });
        (name.clone(), sound)
    });
    library.register(scheme.theme.clone(), events)?;
    save(root, &library)
}

pub(super) fn select(root: &Path, theme: &str) -> Result<()> {
    let mut library = load(root)?;
    library.select(theme)?;
    save(root, &library)
}

pub(super) fn play(root: &Path, event: &str, audio: &mut dyn AudioHost) -> Result<Option<u64>> {
    let library = load(root)?;
    let Some(sound) = library.event(event) else {
        return Ok(None);
    };
    let ApplicationSound::File(path) = sound else {
        return Ok(None);
    };
    let file = File::open(path)
        .with_context(|| format!("open application event sound {}", path.display()))?;
    let mut bytes = Vec::new();
    file.take(MAX_AUDIO + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_AUDIO {
        bail!("application event sound exceeds 64 MiB");
    }
    // WAV/Vorbis decoding and PCM ownership are identical to sound-task
    // playback. Android's DesktopAudioHost consumes this PCM through AAudio.
    let prepared = crate::audio_decode::prepare_audio(bytes)?;
    let resource = path.to_string_lossy().into_owned();
    let playback = crate::audio::play_loaded_sound(
        audio,
        AudioRequest {
            channel: AudioChannel::SoundEffect,
            resource,
            volume: 1.,
            pan: 0.,
            looped: false,
        },
        Some(prepared),
        0,
        1,
        false,
    )?;
    Ok(Some(playback.handle))
}

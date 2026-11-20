//! Shared bounded PCM preparation for script loads and device playback.
use crate::media_process::error;
use cs2_runtime::RuntimeResult;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, OnceLock};

#[path = "audio_decode_state.rs"]
pub(crate) mod checkpoint;
#[path = "audio_decode_vorbis.rs"]
mod vorbis;
#[path = "audio_decode_stream.rs"]
mod vorbis_stream;

pub(super) const MAX_ENCODED: usize = 64 * 1024 * 1024;
const MAX_PCM: usize = 80 * 1024 * 1024;
const MAX_CACHE: usize = 128 * 1024 * 1024;
const MIN_STREAM_BYTES: usize = 256 * 1024;
const MIN_VOICE_STREAM_BYTES: usize = 32 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub enum AudioCodec {
    OggVorbis,
    WavPcm,
    WavMsAdpcm,
    WavFloat,
    SpeechPcm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub struct AudioProbe {
    pub codec: AudioCodec,
    pub sample_rate: u32,
    pub channels: u8,
    pub frames: u64,
}

#[derive(Debug, PartialEq)]
pub struct DecodedAudio {
    pub probe: AudioProbe,
    pub samples: Arc<[i16]>,
}

enum AudioSource {
    Pcm(Arc<[i16]>),
    Vorbis(vorbis_stream::IndexedVorbis),
}

pub(crate) struct PreparedAudio {
    pub probe: AudioProbe,
    source: AudioSource,
    // Checkpoints retain the existing exact PCM wire representation. Only
    // explicit checkpoint operations need to materialize an indexed source.
    checkpoint_pcm: OnceLock<Arc<[i16]>>,
}

impl std::fmt::Debug for PreparedAudio {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedAudio")
            .field("probe", &self.probe)
            .field("streaming", &matches!(self.source, AudioSource::Vorbis(_)))
            .finish()
    }
}

impl PartialEq for PreparedAudio {
    fn eq(&self, other: &Self) -> bool {
        self.matches_pcm(other).unwrap_or(false)
    }
}

impl PreparedAudio {
    pub(super) fn from_pcm(audio: DecodedAudio) -> Self {
        Self {
            probe: audio.probe,
            source: AudioSource::Pcm(audio.samples),
            checkpoint_pcm: OnceLock::new(),
        }
    }

    fn from_indexed_vorbis(source: vorbis_stream::IndexedVorbis) -> Self {
        Self {
            probe: AudioProbe {
                codec: AudioCodec::OggVorbis,
                sample_rate: source.metadata.sample_rate,
                channels: source.metadata.channels,
                frames: source.frames() as u64,
            },
            source: AudioSource::Vorbis(source),
            checkpoint_pcm: OnceLock::new(),
        }
    }

    pub(super) fn materialize(&self) -> RuntimeResult<Arc<[i16]>> {
        match &self.source {
            AudioSource::Pcm(samples) => Ok(samples.clone()),
            AudioSource::Vorbis(vorbis) => {
                if let Some(samples) = self.checkpoint_pcm.get() {
                    return Ok(samples.clone());
                }
                let (_, samples) = vorbis::decode(&vorbis.encoded)?;
                if samples.len() != self.sample_count() {
                    return Err(error("materialized Vorbis PCM differs from its indexed length"));
                }
                let samples: Arc<[i16]> = samples.into();
                let _ = self.checkpoint_pcm.set(samples);
                Ok(self.checkpoint_pcm.get().expect("initialized checkpoint PCM").clone())
            }
        }
    }

    pub(super) fn matches_pcm(&self, other: &Self) -> RuntimeResult<bool> {
        if std::ptr::eq(self, other) {
            return Ok(true);
        }
        if self.probe != other.probe {
            return Ok(false);
        }
        if let (AudioSource::Vorbis(left), AudioSource::Vorbis(right)) = (&self.source, &other.source)
            && left.encoded == right.encoded
        {
            return Ok(true);
        }
        Ok(self.materialize()? == other.materialize()?)
    }

    pub(super) fn sample_count(&self) -> usize {
        self.probe.frames as usize * usize::from(self.probe.channels)
    }

    #[cfg(any(test, windows, target_os = "android"))]
    pub(super) fn reader(&self) -> RuntimeResult<AudioReader<'_>> {
        let source = match &self.source {
            AudioSource::Pcm(samples) => PcmReader::Slice(samples),
            AudioSource::Vorbis(vorbis) => match self.checkpoint_pcm.get() {
                Some(samples) => PcmReader::Slice(samples),
                None => PcmReader::Stream(vorbis.reader()?),
            },
        };
        Ok(AudioReader { source, channels: usize::from(self.probe.channels) })
    }

    fn unshared_samples(&self) -> bool {
        match &self.source {
            AudioSource::Pcm(samples) => Arc::strong_count(samples) == 1,
            AudioSource::Vorbis(_) => self.checkpoint_pcm.get()
                .is_none_or(|samples| Arc::strong_count(samples) == 1),
        }
    }
}

#[cfg(any(test, windows, target_os = "android"))]
enum PcmReader<'a> {
    Slice(&'a [i16]),
    Stream(vorbis_stream::VorbisReader<'a>),
}

#[cfg(any(test, windows, target_os = "android"))]
pub(super) struct AudioReader<'a> {
    source: PcmReader<'a>,
    channels: usize,
}

#[cfg(any(test, windows, target_os = "android"))]
impl AudioReader<'_> {
    pub(super) fn read(&mut self, start: usize, end: usize) -> RuntimeResult<&[i16]> {
        match &mut self.source {
            PcmReader::Slice(samples) => Ok(&samples[start * self.channels..end * self.channels]),
            PcmReader::Stream(reader) => reader.read(start, end),
        }
    }
}

struct Entry {
    hash: u64,
    encoded: Arc<[u8]>,
    audio: Arc<PreparedAudio>,
    bytes: usize,
}

#[derive(Default)]
struct Cache {
    entries: VecDeque<Entry>,
    bytes: usize,
}

static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();

/// Cache hits require byte equality in addition to the hash; unrelated roots
/// and intentional hash collisions can never select another resource's PCM.
pub(super) fn prepare_audio(encoded: Vec<u8>) -> RuntimeResult<Arc<PreparedAudio>> {
    prepare_audio_with_minimum(encoded, MIN_STREAM_BYTES)
}

pub(super) fn prepare_named_audio(encoded: Vec<u8>, resource: &str) -> RuntimeResult<Arc<PreparedAudio>> {
    // Traditional dialogue resides in pcm_* archives. This classification
    // controls only preparation, leaving script channel routing untouched.
    let voice = resource.split(['/', '\\']).any(|part| {
        part.eq_ignore_ascii_case("voice")
            || part.get(..4).is_some_and(|prefix| prefix.eq_ignore_ascii_case("pcm_"))
    });
    prepare_audio_with_minimum(encoded, if voice { MIN_VOICE_STREAM_BYTES } else { MIN_STREAM_BYTES })
}

fn prepare_audio_with_minimum(encoded: Vec<u8>, min_stream_bytes: usize) -> RuntimeResult<Arc<PreparedAudio>> {
    if encoded.is_empty() || encoded.len() > MAX_ENCODED {
        return Err(error("encoded sound must contain 1 byte to 64 MiB"));
    }
    let hash = encoded.iter().fold(0xcbf29ce484222325_u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    });
    let mut cache = CACHE
        .get_or_init(|| Mutex::new(Cache::default()))
        .lock()
        .map_err(|_| error("decoded audio cache was poisoned"))?;
    if let Some(index) = cache
        .entries
        .iter()
        .position(|entry| entry.hash == hash && entry.encoded.as_ref() == encoded.as_slice())
    {
        let entry = cache.entries.remove(index).expect("selected cache entry");
        let audio = entry.audio.clone();
        cache.entries.push_back(entry);
        return Ok(audio);
    }
    let encoded: Arc<[u8]> = encoded.into();
    let pinned_bytes = || cache.entries.iter()
        .filter(|entry| !entry.evictable())
        .map(|entry| entry.bytes)
        .sum::<usize>();
    let indexed = if encoded.len() >= min_stream_bytes && encoded.starts_with(b"OggS") {
        vorbis_stream::IndexedVorbis::new(encoded.clone())?.filter(|source| {
            // Reserve the optional checkpoint PCM and fall back to ordinary
            // PCM when the index would prevent a previously admissible load.
            pinned_bytes() + encoded.len() + source.retained_bytes()
                + source.frames() * usize::from(source.metadata.channels) * size_of::<i16>()
                <= MAX_CACHE
        })
    } else {
        None
    };
    let (audio, retained) = if let Some(source) = indexed {
        let retained = source.retained_bytes();
        (PreparedAudio::from_indexed_vorbis(source), retained)
    } else {
        (decode_pcm(&encoded)?, 0)
    };
    let AudioProbe { sample_rate, channels, frames, .. } = audio.probe;
    if !matches!(channels, 1 | 2) || !(1..=384_000).contains(&sample_rate) || frames == 0 {
        return Err(error(
            "decoded sound has an unsupported rate, channel count or sample alignment",
        ));
    }
    let bytes = encoded.len() + retained + audio.sample_count() * size_of::<i16>();
    if bytes > MAX_CACHE {
        return Err(error("sound cache entry exceeds 128 MiB"));
    }
    while cache.bytes + bytes > MAX_CACHE || cache.entries.len() >= 128 {
        let index = cache.entries.iter().position(Entry::evictable)
            .ok_or_else(|| error("128 MiB decoded sound cache is pinned by loaded/playing channels"))?;
        let evicted = cache.entries.remove(index).expect("selected unpinned cache entry");
        cache.bytes -= evicted.bytes;
    }
    let audio = Arc::new(audio);
    cache.bytes += bytes;
    cache.entries.push_back(Entry {
        hash,
        encoded,
        audio: audio.clone(),
        bytes,
    });
    Ok(audio)
}

impl Entry {
    fn evictable(&self) -> bool {
        Arc::strong_count(&self.audio) == 1 && self.audio.unshared_samples()
    }
}

fn decode_pcm(encoded: &[u8]) -> RuntimeResult<PreparedAudio> {
    let (codec, sample_rate, channels, samples) = if encoded.starts_with(b"OggS") {
        let (metadata, samples) = vorbis::decode(encoded)?;
        (AudioCodec::OggVorbis, metadata.sample_rate, metadata.channels, samples)
    } else if encoded.starts_with(b"RIFF") && encoded.get(8..12) == Some(b"WAVE") {
        crate::wav_decode::decode(encoded)?
    } else {
        return Err(error(
            "sound is neither Ogg/Vorbis nor a supported RIFF/WAVE codec",
        ));
    };
    if !matches!(channels, 1 | 2)
        || !(1..=384_000).contains(&sample_rate)
        || samples.is_empty()
        || samples.len() % usize::from(channels) != 0
    {
        return Err(error(
            "decoded sound has an unsupported rate, channel count or sample alignment",
        ));
    }
    Ok(PreparedAudio::from_pcm(DecodedAudio {
        probe: AudioProbe {
            codec,
            sample_rate,
            channels,
            frames: (samples.len() / usize::from(channels)) as u64,
        },
        samples: samples.into(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cs2_formats::OggVorbis;
    use cs2_runtime::GameRoot;
    use std::path::PathBuf;

    #[test]
    #[ignore = "requires the local sample 05 archives"]
    fn decodes_sample_05_title_music_and_voice_from_original_archives() -> anyhow::Result<()> {
        let root = match std::env::var_os("CS2_SAMPLE_ROOT").map(PathBuf::from) {
            Some(root) => {
                let catalog = root.join("games");
                if catalog.is_dir() { catalog } else { root }
            }
            None => PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../games"),
        };
        let sample = root.join("\u{3046}\u{305f}\u{3066}\u{3081}\u{3050}\u{308a}");
        let scan = GameRoot::open(sample)?.scan()?;
        let music = scan.resources.read("bgm.int/BGM50.ogg")?;
        assert_eq!(music.len(), 9_942_144);
        assert!(matches!(OggVorbis::metadata(&music), Err(cs2_formats::OggVorbisError::Truncated)));
        let title = prepare_audio(music)?;
        assert_eq!(title.probe, AudioProbe {
            codec: AudioCodec::OggVorbis, sample_rate: 44_100, channels: 2, frames: 18_598_464,
        });
        assert!(title.materialize()?.iter().any(|&sample| sample != 0));

        let voice = scan.resources.read("pcm_a.int/A_00_01_001.ogg")?;
        assert!(OggVorbis::metadata(&voice).is_ok());
        let spoken = prepare_audio(voice)?;
        assert_eq!(spoken.probe, AudioProbe {
            codec: AudioCodec::OggVorbis, sample_rate: 44_100, channels: 1, frames: 189_954,
        });
        assert!(spoken.materialize()?.iter().any(|&sample| sample != 0));
        Ok(())
    }
}

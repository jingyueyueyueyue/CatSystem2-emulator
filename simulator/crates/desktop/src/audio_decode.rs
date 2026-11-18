//! Shared bounded PCM preparation for script loads and device playback.
use crate::media_process::error;
use cs2_formats::OggVorbis;
use cs2_runtime::RuntimeResult;
use std::collections::VecDeque;
use std::io::Cursor;
use std::sync::{Arc, Mutex, OnceLock};

#[path = "audio_decode_state.rs"]
pub(crate) mod checkpoint;

const MAX_ENCODED: usize = 64 * 1024 * 1024;
const MAX_PCM: usize = 80 * 1024 * 1024;
const MAX_CACHE: usize = 128 * 1024 * 1024;

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

struct Entry {
    hash: u64,
    encoded: Arc<[u8]>,
    audio: Arc<DecodedAudio>,
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
pub fn prepare_audio(encoded: Vec<u8>) -> RuntimeResult<Arc<DecodedAudio>> {
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
    let (codec, sample_rate, channels, samples) = if encoded.starts_with(b"OggS") {
        // Validate pages/CRC using the format service, then collect packets
        // with an explicit allocation ceiling instead of an unbounded decode.
        let metadata = OggVorbis::metadata_at_entry_boundary(&encoded)
            .map_err(|cause| error(format!("probe Ogg/Vorbis: {cause}")))?;
        let mut stream = lewton::inside_ogg::OggStreamReader::new(Cursor::new(encoded.as_slice()))
            .map_err(|cause| error(format!("open Ogg/Vorbis decoder: {cause}")))?;
        let mut samples = Vec::new();
        while let Some(packet) = stream
            .read_dec_packet_itl()
            .map_err(|cause| error(format!("decode Ogg/Vorbis packet: {cause}")))?
        {
            let length = samples
                .len()
                .checked_add(packet.len())
                .filter(|samples| *samples <= MAX_PCM / 2)
                .ok_or_else(|| error("decoded audio exceeds the 80 MiB PCM limit"))?;
            if length > samples.capacity() {
                let capacity = length.max((samples.capacity() * 2).min(MAX_PCM / 2));
                samples.try_reserve_exact(capacity - samples.len())
                    .map_err(|_| error("could not allocate bounded sound PCM"))?;
            }
            samples.extend(packet);
        }
        (
            AudioCodec::OggVorbis,
            metadata.sample_rate,
            metadata.channels,
            samples,
        )
    } else if encoded.starts_with(b"RIFF") && encoded.get(8..12) == Some(b"WAVE") {
        crate::wav_decode::decode(&encoded)?
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
    let bytes = encoded
        .len()
        .checked_add(samples.len() * 2)
        .filter(|bytes| *bytes <= MAX_CACHE)
        .ok_or_else(|| error("sound cache entry exceeds 128 MiB"))?;
    while cache.bytes + bytes > MAX_CACHE || cache.entries.len() >= 128 {
        let index = cache
            .entries
            .iter()
            .position(|entry| {
                Arc::strong_count(&entry.audio) == 1 && Arc::strong_count(&entry.audio.samples) == 1
            })
            .ok_or_else(|| {
                error("128 MiB decoded sound cache is pinned by loaded/playing channels")
            })?;
        let evicted = cache
            .entries
            .remove(index)
            .expect("selected unpinned cache entry");
        cache.bytes -= evicted.bytes;
    }
    let audio = Arc::new(DecodedAudio {
        probe: AudioProbe {
            codec,
            sample_rate,
            channels,
            frames: (samples.len() / usize::from(channels)) as u64,
        },
        samples: samples.into(),
    });
    cache.bytes += bytes;
    cache.entries.push_back(Entry {
        hash,
        encoded: encoded.into(),
        audio: audio.clone(),
        bytes,
    });
    Ok(audio)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cs2_runtime::GameRoot;
    use std::path::PathBuf;

    #[test]
    #[ignore = "requires the local sample 05 archives"]
    fn decodes_sample_05_title_music_and_voice_from_original_archives() -> anyhow::Result<()> {
        let root = std::env::var_os("CS2_SAMPLE_ROOT").map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.."));
        let sample = root.join("\u{3046}\u{305f}\u{3066}\u{3081}\u{3050}\u{308a}");
        let scan = GameRoot::open(sample)?.scan()?;
        let music = scan.resources.read("bgm.int/BGM50.ogg")?;
        assert_eq!(music.len(), 9_942_144);
        assert!(matches!(OggVorbis::metadata(&music), Err(cs2_formats::OggVorbisError::Truncated)));
        let title = prepare_audio(music)?;
        assert_eq!(title.probe, AudioProbe {
            codec: AudioCodec::OggVorbis, sample_rate: 44_100, channels: 2, frames: 18_598_464,
        });
        assert!(title.samples.iter().any(|&sample| sample != 0));

        let voice = scan.resources.read("pcm_a.int/A_00_01_001.ogg")?;
        assert!(OggVorbis::metadata(&voice).is_ok());
        let spoken = prepare_audio(voice)?;
        assert_eq!(spoken.probe, AudioProbe {
            codec: AudioCodec::OggVorbis, sample_rate: 44_100, channels: 1, frames: 189_954,
        });
        assert!(spoken.samples.iter().any(|&sample| sample != 0));
        Ok(())
    }
}

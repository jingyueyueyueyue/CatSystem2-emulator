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
const MAX_SAMPLE_RATE: u32 = 384_000;
const PCM_READ_FRAMES: usize = MAX_SAMPLE_RATE as usize / 100;
const MIN_STREAM_BYTES: usize = 256 * 1024;
const MIN_VOICE_STREAM_BYTES: usize = 32 * 1024;

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    cs2_runtime::graph_serde::Serialize,
    cs2_runtime::graph_serde::Deserialize,
)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub enum AudioCodec {
    OggVorbis,
    WavPcm,
    WavMsAdpcm,
    WavFloat,
    SpeechPcm,
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    cs2_runtime::graph_serde::Serialize,
    cs2_runtime::graph_serde::Deserialize,
)]
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
    Wav {
        wave: crate::wav_decode::Wave,
        encoded: Arc<[u8]>,
    },
}

pub(crate) struct PreparedAudio {
    pub probe: AudioProbe,
    source: AudioSource,
}

impl std::fmt::Debug for PreparedAudio {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedAudio")
            .field("probe", &self.probe)
            .field("streaming", &!matches!(self.source, AudioSource::Pcm(_)))
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
        }
    }

    pub(super) fn checkpoint_samples(&self) -> RuntimeResult<Vec<i16>> {
        match &self.source {
            AudioSource::Pcm(samples) => Ok(samples.to_vec()),
            AudioSource::Vorbis(vorbis) => vorbis.checkpoint_samples(),
            AudioSource::Wav { wave, encoded } => {
                if self.sample_count() > MAX_PCM / size_of::<i16>() {
                    return Err(error("checkpoint audio exceeds the 80 MiB PCM limit"));
                }
                let mut samples = Vec::new();
                wave.read(encoded, 0, self.probe.frames as usize, &mut samples)?;
                Ok(samples)
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
        if let (AudioSource::Vorbis(left), AudioSource::Vorbis(right)) =
            (&self.source, &other.source)
            && left.encoded == right.encoded
        {
            return Ok(true);
        }
        match (&self.source, &other.source) {
            (AudioSource::Pcm(samples), _) => other.matches_samples(samples),
            (_, AudioSource::Pcm(samples)) => self.matches_samples(samples),
            _ => {
                let mut left = self.reader()?;
                let mut right = other.reader()?;
                for start in (0..self.probe.frames as usize).step_by(PCM_READ_FRAMES) {
                    let end = (start + PCM_READ_FRAMES).min(self.probe.frames as usize);
                    if left.read(start, end)? != right.read(start, end)? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
        }
    }

    pub(super) fn matches_samples(&self, samples: &[i16]) -> RuntimeResult<bool> {
        if samples.len() != self.sample_count() {
            return Ok(false);
        }
        if let AudioSource::Pcm(actual) = &self.source {
            return Ok(actual.as_ref() == samples);
        }
        let mut reader = self.reader()?;
        let channels = usize::from(self.probe.channels);
        for start in (0..self.probe.frames as usize).step_by(PCM_READ_FRAMES) {
            let end = (start + PCM_READ_FRAMES).min(self.probe.frames as usize);
            if reader.read(start, end)? != &samples[start * channels..end * channels] {
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub(super) fn sample_count(&self) -> usize {
        self.probe.frames as usize * usize::from(self.probe.channels)
    }

    fn resident_bytes(&self) -> usize {
        match &self.source {
            AudioSource::Pcm(samples) => std::mem::size_of_val(samples.as_ref()),
            AudioSource::Vorbis(vorbis) => vorbis.index_bytes(),
            AudioSource::Wav { wave, .. } => wave.extra_bytes(),
        }
    }

    pub(super) fn reader(&self) -> RuntimeResult<AudioReader<'_>> {
        let source = match &self.source {
            AudioSource::Pcm(samples) => PcmReader::Slice(samples),
            AudioSource::Vorbis(vorbis) => PcmReader::Stream(vorbis.reader()?),
            AudioSource::Wav { wave, encoded } => PcmReader::Wav {
                wave,
                encoded,
                samples: Vec::new(),
            },
        };
        Ok(AudioReader {
            source,
            channels: usize::from(self.probe.channels),
        })
    }
}

enum PcmReader<'a> {
    Slice(&'a [i16]),
    Stream(vorbis_stream::VorbisReader<'a>),
    Wav {
        wave: &'a crate::wav_decode::Wave,
        encoded: &'a [u8],
        samples: Vec<i16>,
    },
}

pub(super) struct AudioReader<'a> {
    source: PcmReader<'a>,
    channels: usize,
}

impl AudioReader<'_> {
    pub(super) fn read(&mut self, start: usize, end: usize) -> RuntimeResult<&[i16]> {
        match &mut self.source {
            PcmReader::Slice(samples) => {
                if start > end || end > samples.len() / self.channels {
                    return Err(error("requested frames beyond the PCM source"));
                }
                Ok(&samples[start * self.channels..end * self.channels])
            }
            PcmReader::Stream(reader) => reader.read(start, end),
            PcmReader::Wav {
                wave,
                encoded,
                samples,
            } => {
                if end.saturating_sub(start) > PCM_READ_FRAMES {
                    return Err(error("requested PCM exceeds the WAV streaming window"));
                }
                wave.read(encoded, start, end, samples)?;
                Ok(samples)
            }
        }
    }
}

struct Entry {
    hash: u64,
    encoded: Arc<[u8]>,
    audio: Arc<PreparedAudio>,
    // Encoded data is shared by the cache key and any indexed source.
    bytes: usize,
}

#[derive(Default)]
struct Cache {
    entries: VecDeque<Entry>,
    bytes: usize,
}

impl Cache {
    fn find(&mut self, hash: u64, encoded: &[u8]) -> Option<Arc<PreparedAudio>> {
        let index = self
            .entries
            .iter()
            .position(|entry| entry.hash == hash && entry.encoded.as_ref() == encoded)?;
        let entry = self.entries.remove(index).expect("selected cache entry");
        let audio = entry.audio.clone();
        self.entries.push_back(entry);
        Some(audio)
    }
}

static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
// Bound concurrent cold preparations without growing per-resource lock state.
static PREPARATION: [Mutex<()>; 2] = [Mutex::new(()), Mutex::new(())];

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PreparationEvent {
    CacheMiss,
    PreparationStart,
}

#[cfg(test)]
static PREPARATION_TEST_HOOK: Mutex<Option<Arc<dyn Fn(PreparationEvent) + Send + Sync>>> =
    Mutex::new(None);

#[cfg(test)]
fn notify_preparation_test(event: PreparationEvent) {
    let hook = PREPARATION_TEST_HOOK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    if let Some(hook) = hook {
        hook(event);
    }
}

fn content_hash(encoded: &[u8]) -> u64 {
    encoded.iter().fold(0xcbf29ce484222325_u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    })
}

/// Cache hits require byte equality in addition to the hash; unrelated roots
/// and intentional hash collisions can never select another resource's PCM.
pub(super) fn prepare_audio(encoded: Vec<u8>) -> RuntimeResult<Arc<PreparedAudio>> {
    prepare_audio_with_minimum(encoded, MIN_STREAM_BYTES)
}

pub(super) fn prepare_named_audio(
    encoded: Vec<u8>,
    resource: &str,
) -> RuntimeResult<Arc<PreparedAudio>> {
    // Traditional dialogue resides in pcm_* archives. This classification
    // controls only preparation, leaving script channel routing untouched.
    let voice = resource.split(['/', '\\']).any(|part| {
        part.eq_ignore_ascii_case("voice")
            || part
                .get(..4)
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("pcm_"))
    });
    prepare_audio_with_minimum(
        encoded,
        if voice {
            MIN_VOICE_STREAM_BYTES
        } else {
            MIN_STREAM_BYTES
        },
    )
}

fn prepare_audio_with_minimum(
    encoded: Vec<u8>,
    min_stream_bytes: usize,
) -> RuntimeResult<Arc<PreparedAudio>> {
    if encoded.is_empty() || encoded.len() > MAX_ENCODED {
        return Err(error("encoded sound must contain 1 byte to 64 MiB"));
    }
    let hash = content_hash(&encoded);
    let shared_cache = CACHE.get_or_init(|| Mutex::new(Cache::default()));
    let lock_cache = || {
        shared_cache
            .lock()
            .map_err(|_| error("decoded audio cache was poisoned"))
    };
    if let Some(audio) = lock_cache()?.find(hash, &encoded) {
        return Ok(audio);
    }
    #[cfg(test)]
    notify_preparation_test(PreparationEvent::CacheMiss);
    let _preparation = PREPARATION[(hash >> 63) as usize]
        .lock()
        .map_err(|_| error("audio preparation lock was poisoned"))?;
    if let Some(audio) = lock_cache()?.find(hash, &encoded) {
        return Ok(audio);
    }
    #[cfg(test)]
    notify_preparation_test(PreparationEvent::PreparationStart);
    let encoded: Arc<[u8]> = encoded.into();
    let audio = prepare_source(&encoded, min_stream_bytes)?;
    let AudioProbe {
        sample_rate,
        channels,
        frames,
        ..
    } = audio.probe;
    if !matches!(channels, 1 | 2) || !(1..=MAX_SAMPLE_RATE).contains(&sample_rate) || frames == 0 {
        return Err(error(
            "decoded sound has an unsupported rate, channel count or sample alignment",
        ));
    }
    let bytes = encoded.len() + audio.resident_bytes();
    if bytes > MAX_CACHE {
        return Err(error("sound cache entry exceeds 128 MiB"));
    }
    let mut cache = lock_cache()?;
    while cache.bytes + bytes > MAX_CACHE || cache.entries.len() >= 128 {
        let index = cache
            .entries
            .iter()
            .position(Entry::evictable)
            .ok_or_else(|| error("128 MiB sound cache is pinned by loaded/playing channels"))?;
        let evicted = cache
            .entries
            .remove(index)
            .expect("selected unpinned cache entry");
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
        Arc::strong_count(&self.audio) == 1
    }
}

fn prepare_source(encoded: &Arc<[u8]>, min_stream_bytes: usize) -> RuntimeResult<PreparedAudio> {
    if encoded.starts_with(b"OggS") {
        let index =
            || vorbis_stream::IndexedVorbis::new(encoded.clone(), MAX_CACHE - encoded.len());
        let prefer_stream = encoded.len() >= min_stream_bytes;
        if prefer_stream && let Some(source) = index()? {
            return Ok(PreparedAudio::from_indexed_vorbis(source));
        }
        let (metadata, samples) = match vorbis::decode(encoded) {
            Ok(decoded) => decoded,
            Err(vorbis::DecodeError::Invalid(cause)) => return Err(cause),
            Err(vorbis::DecodeError::PcmLimit) => {
                if !prefer_stream && let Some(source) = index()? {
                    return Ok(PreparedAudio::from_indexed_vorbis(source));
                }
                return Err(error(
                    "long Vorbis source exceeds the streaming index budget",
                ));
            }
        };
        return prepare_pcm(
            AudioCodec::OggVorbis,
            metadata.sample_rate,
            metadata.channels,
            samples,
        );
    }
    if encoded.starts_with(b"RIFF") && encoded.get(8..12) == Some(b"WAVE") {
        let wave = crate::wav_decode::Wave::parse(encoded)?;
        let probe = wave.probe;
        let size = probe.frames * u64::from(probe.channels) * size_of::<i16>() as u64;
        if size > (64 * 1024 * 1024).min(MAX_CACHE - encoded.len()) as u64 {
            return Ok(PreparedAudio {
                probe,
                source: AudioSource::Wav {
                    wave,
                    encoded: encoded.clone(),
                },
            });
        }
        let mut samples = Vec::new();
        wave.read(encoded, 0, probe.frames as usize, &mut samples)?;
        return prepare_pcm(probe.codec, probe.sample_rate, probe.channels, samples);
    }
    Err(error(
        "sound is neither Ogg/Vorbis nor a supported RIFF/WAVE codec",
    ))
}

fn prepare_pcm(
    codec: AudioCodec,
    sample_rate: u32,
    channels: u8,
    samples: Vec<i16>,
) -> RuntimeResult<PreparedAudio> {
    if !matches!(channels, 1 | 2)
        || !(1..=MAX_SAMPLE_RATE).contains(&sample_rate)
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
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Barrier, Condvar, MutexGuard, mpsc};
    use std::thread;
    use std::time::Duration;

    static TEST_SERIAL: Mutex<()> = Mutex::new(());

    struct TestState {
        _serial: MutexGuard<'static, ()>,
    }

    impl TestState {
        fn new() -> Self {
            let serial = TEST_SERIAL
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            Self::reset_cache();
            *PREPARATION_TEST_HOOK
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
            Self { _serial: serial }
        }

        fn reset_cache() {
            let mut cache = CACHE
                .get_or_init(|| Mutex::new(Cache::default()))
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *cache = Cache::default();
        }

        fn set_hook(&self, hook: impl Fn(PreparationEvent) + Send + Sync + 'static) {
            *PREPARATION_TEST_HOOK
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::new(hook));
        }
    }

    impl Drop for TestState {
        fn drop(&mut self) {
            *PREPARATION_TEST_HOOK
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
            Self::reset_cache();
        }
    }

    struct ColdPreparations {
        events: mpsc::Receiver<PreparationEvent>,
        starts: Arc<AtomicUsize>,
        released: Arc<(Mutex<bool>, Condvar)>,
    }

    impl ColdPreparations {
        fn install(state: &TestState) -> Self {
            let (event_sender, events) = mpsc::channel();
            let starts = Arc::new(AtomicUsize::new(0));
            let released = Arc::new((Mutex::new(false), Condvar::new()));
            let hook_starts = starts.clone();
            let hook_released = released.clone();
            state.set_hook(move |event| {
                let _ = event_sender.send(event);
                if event == PreparationEvent::PreparationStart {
                    hook_starts.fetch_add(1, Ordering::SeqCst);
                    let (lock, condition) = &*hook_released;
                    let mut released = lock
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    while !*released {
                        released = condition
                            .wait(released)
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                    }
                }
            });
            Self {
                events,
                starts,
                released,
            }
        }

        fn receive(&self) -> Option<PreparationEvent> {
            self.events.recv_timeout(Duration::from_secs(10)).ok()
        }

        fn starts(&self) -> usize {
            self.starts.load(Ordering::SeqCst)
        }

        fn release(&self) {
            let (lock, condition) = &*self.released;
            *lock
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
            condition.notify_all();
        }
    }

    impl Drop for ColdPreparations {
        fn drop(&mut self) {
            self.release();
        }
    }

    fn pcm_wav(samples: &[i16]) -> Vec<u8> {
        let data_bytes = (samples.len() * size_of::<i16>()) as u32;
        let mut encoded = Vec::with_capacity(44 + data_bytes as usize);
        encoded.extend_from_slice(b"RIFF");
        encoded.extend_from_slice(&(36 + data_bytes).to_le_bytes());
        encoded.extend_from_slice(b"WAVEfmt ");
        encoded.extend_from_slice(&16_u32.to_le_bytes());
        encoded.extend_from_slice(&1_u16.to_le_bytes());
        encoded.extend_from_slice(&1_u16.to_le_bytes());
        encoded.extend_from_slice(&8_000_u32.to_le_bytes());
        encoded.extend_from_slice(&16_000_u32.to_le_bytes());
        encoded.extend_from_slice(&2_u16.to_le_bytes());
        encoded.extend_from_slice(&16_u16.to_le_bytes());
        encoded.extend_from_slice(b"data");
        encoded.extend_from_slice(&data_bytes.to_le_bytes());
        for sample in samples {
            encoded.extend_from_slice(&sample.to_le_bytes());
        }
        encoded
    }

    fn spawn_preparation(
        start: Arc<Barrier>,
        encoded: Vec<u8>,
    ) -> thread::JoinHandle<RuntimeResult<Arc<PreparedAudio>>> {
        thread::spawn(move || {
            start.wait();
            prepare_audio(encoded)
        })
    }

    #[test]
    fn same_resource_cold_preparation_is_singleflight() {
        let state = TestState::new();
        let encoded = pcm_wav(&[0]);
        let cold = ColdPreparations::install(&state);
        let start = Arc::new(Barrier::new(3));
        let first = spawn_preparation(start.clone(), encoded.clone());
        let second = spawn_preparation(start.clone(), encoded);
        start.wait();

        let events = [cold.receive(), cold.receive(), cold.receive()];
        cold.release();
        let first = first
            .join()
            .expect("first preparation thread")
            .expect("first audio");
        let second = second
            .join()
            .expect("second preparation thread")
            .expect("second audio");

        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(cold.starts(), 1);
        assert_eq!(
            events
                .iter()
                .filter(|&&event| event == Some(PreparationEvent::CacheMiss))
                .count(),
            2
        );
        assert_eq!(
            events
                .iter()
                .filter(|&&event| event == Some(PreparationEvent::PreparationStart))
                .count(),
            1
        );
    }

    #[test]
    fn different_resources_on_different_shards_prepare_in_parallel() {
        let state = TestState::new();
        let first = pcm_wav(&[0]);
        let second = pcm_wav(&[0, 1, 2, 3, 4, 5]);
        assert_ne!(content_hash(&first) >> 63, content_hash(&second) >> 63);
        let cold = ColdPreparations::install(&state);
        let start = Arc::new(Barrier::new(3));
        let first_thread = spawn_preparation(start.clone(), first);
        let second_thread = spawn_preparation(start.clone(), second);
        start.wait();

        let events = [
            cold.receive(),
            cold.receive(),
            cold.receive(),
            cold.receive(),
        ];
        cold.release();
        first_thread
            .join()
            .expect("first preparation thread")
            .expect("first audio");
        second_thread
            .join()
            .expect("second preparation thread")
            .expect("second audio");

        assert_eq!(cold.starts(), 2);
        assert_eq!(
            events
                .iter()
                .filter(|&&event| event == Some(PreparationEvent::CacheMiss))
                .count(),
            2
        );
        assert_eq!(
            events
                .iter()
                .filter(|&&event| event == Some(PreparationEvent::PreparationStart))
                .count(),
            2
        );
    }

    #[test]
    fn cache_hit_completes_while_cold_preparation_is_blocked() {
        let state = TestState::new();
        let warm = pcm_wav(&[0]);
        let cold_encoded = pcm_wav(&[0, 1, 2, 3, 4, 5]);
        let cached = prepare_audio(warm.clone()).expect("warm audio");
        let cold = ColdPreparations::install(&state);
        let start = Arc::new(Barrier::new(2));
        let cold_thread = spawn_preparation(start.clone(), cold_encoded);
        start.wait();
        let events = [cold.receive(), cold.receive()];

        let (hit_sender, hit_receiver) = mpsc::channel();
        let hit_thread = thread::spawn(move || {
            let _ = hit_sender.send(prepare_audio(warm));
        });
        let hit = hit_receiver.recv_timeout(Duration::from_secs(10)).ok();
        cold.release();
        cold_thread
            .join()
            .expect("cold preparation thread")
            .expect("cold audio");
        hit_thread.join().expect("cache hit thread");

        assert_eq!(
            events,
            [
                Some(PreparationEvent::CacheMiss),
                Some(PreparationEvent::PreparationStart)
            ]
        );
        let hit = hit
            .expect("cache hit while cold preparation was blocked")
            .expect("cached audio");
        assert!(Arc::ptr_eq(&cached, &hit));
    }

    #[test]
    fn streams_adpcm_beyond_the_eager_pcm_budget() -> anyhow::Result<()> {
        let _state = TestState::new();
        let blocks = 17_000;
        let data_bytes = blocks * 1024;
        let mut encoded = Vec::with_capacity(data_bytes + 54);
        encoded.extend_from_slice(b"RIFF");
        encoded.extend_from_slice(&((data_bytes + 46) as u32).to_le_bytes());
        encoded.extend_from_slice(b"WAVEfmt \x1a\0\0\0");
        for value in [2_u16, 1, 44_100, 0, 22_180, 0, 1024, 4, 8, 2036, 1, 256, 0] {
            encoded.extend_from_slice(&value.to_le_bytes());
        }
        encoded.extend_from_slice(b"data");
        encoded.extend_from_slice(&(data_bytes as u32).to_le_bytes());
        let mut block = [0; 1024];
        block[1..3].copy_from_slice(&16_u16.to_le_bytes());
        block[3..5].copy_from_slice(&1200_i16.to_le_bytes());
        block[5..7].copy_from_slice(&(-500_i16).to_le_bytes());
        for _ in 0..blocks {
            encoded.extend_from_slice(&block);
        }

        let audio = prepare_audio(encoded)?;
        assert!(matches!(audio.source, AudioSource::Wav { .. }));
        assert_eq!(audio.probe.frames, (blocks * 2036) as u64);
        assert!(audio.sample_count() * size_of::<i16>() > 64 * 1024 * 1024);
        let mut reader = audio.reader()?;
        let last_block = (blocks - 1) * 2036;
        assert_eq!(
            reader.read(last_block - 1, last_block + 3)?,
            [1200, -500, 1200, 1200]
        );
        assert_eq!(reader.read(0, 3)?, [-500, 1200, 1200]);
        Ok(())
    }

    #[test]
    #[ignore = "requires the original game archives"]
    fn streams_original_bgm1_12_beyond_the_old_pcm_limit() -> anyhow::Result<()> {
        let _state = TestState::new();
        let root = std::env::var_os("CS2_SAMPLE_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../games"));
        let catalog = if root.join("games").is_dir() {
            root.join("games")
        } else {
            root
        };
        let game = catalog.join("\u{767d}\u{663c}\u{5922}\u{306e}\u{9752}\u{5199}\u{771f}");
        let scan = GameRoot::open(game)?.scan()?;
        let resource = "bgm.int/bgm1_12.ogg";
        let audio = prepare_named_audio(scan.resources.read(resource)?, resource)?;
        let AudioSource::Vorbis(source) = &audio.source else {
            anyhow::bail!("long Vorbis source must stream");
        };
        let channels = usize::from(audio.probe.channels);
        let old_frame_limit = MAX_PCM / size_of::<i16>() / channels;
        let end = audio.probe.frames as usize;
        assert!(end > old_frame_limit + PCM_READ_FRAMES);
        println!(
            "{resource} {:?}, {} PCM bytes, {} cache bytes",
            audio.probe,
            audio.sample_count() * size_of::<i16>(),
            source.encoded.len() + audio.resident_bytes()
        );

        let mut reader = audio.reader()?;
        let starts = [0, old_frame_limit, end - PCM_READ_FRAMES];
        let windows = starts.map(|start| {
            reader
                .read(start, start + PCM_READ_FRAMES)
                .map(<[i16]>::to_vec)
        });
        let windows = windows.into_iter().collect::<RuntimeResult<Vec<_>>>()?;
        let mut reference = lewton::inside_ogg::OggStreamReader::new(std::io::Cursor::new(
            source.encoded.as_ref(),
        ))?;
        let mut position = 0;
        let mut audible_after_limit = false;
        while let Some(packet) = reference.read_dec_packet_itl()? {
            let next = position + packet.len() / channels;
            for (start, window) in starts.into_iter().zip(&windows) {
                let from = start.max(position);
                let to = (start + PCM_READ_FRAMES).min(next);
                if from < to {
                    assert_eq!(
                        &window[(from - start) * channels..(to - start) * channels],
                        &packet[(from - position) * channels..(to - position) * channels],
                        "mismatched samples at frame {from}"
                    );
                }
            }
            if position >= old_frame_limit && packet.iter().any(|&sample| sample != 0) {
                audible_after_limit = true;
            }
            position = next;
        }
        assert_eq!(position, end);
        assert!(audible_after_limit);
        assert_eq!(reader.read(0, PCM_READ_FRAMES)?, windows[0]);
        Ok(())
    }

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
        assert!(matches!(
            OggVorbis::metadata(&music),
            Err(cs2_formats::OggVorbisError::Truncated)
        ));
        let title = prepare_audio(music)?;
        assert_eq!(
            title.probe,
            AudioProbe {
                codec: AudioCodec::OggVorbis,
                sample_rate: 44_100,
                channels: 2,
                frames: 18_598_464,
            }
        );
        assert!(
            title
                .checkpoint_samples()?
                .iter()
                .any(|&sample| sample != 0)
        );

        let voice = scan.resources.read("pcm_a.int/A_00_01_001.ogg")?;
        assert!(OggVorbis::metadata(&voice).is_ok());
        let spoken = prepare_audio(voice)?;
        assert_eq!(
            spoken.probe,
            AudioProbe {
                codec: AudioCodec::OggVorbis,
                sample_rate: 44_100,
                channels: 1,
                frames: 189_954,
            }
        );
        assert!(
            spoken
                .checkpoint_samples()?
                .iter()
                .any(|&sample| sample != 0)
        );
        Ok(())
    }
}

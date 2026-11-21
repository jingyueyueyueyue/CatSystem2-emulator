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
}

pub(crate) struct PreparedAudio {
    pub probe: AudioProbe,
    source: AudioSource,
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
            (AudioSource::Vorbis(_), AudioSource::Vorbis(_)) => {
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

    pub(super) fn reader(&self) -> RuntimeResult<AudioReader<'_>> {
        let source = match &self.source {
            AudioSource::Pcm(samples) => PcmReader::Slice(samples),
            AudioSource::Vorbis(vorbis) => PcmReader::Stream(vorbis.reader()?),
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
}

pub(super) struct AudioReader<'a> {
    source: PcmReader<'a>,
    channels: usize,
}

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
    // The original encoded-plus-PCM charge bounds either resident source.
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
    let indexed = if encoded.len() >= min_stream_bytes && encoded.starts_with(b"OggS") {
        let index_limit = (MAX_CACHE - encoded.len()).min(MAX_PCM);
        vorbis_stream::IndexedVorbis::new(encoded.clone(), index_limit)?
    } else {
        None
    };
    let audio = if let Some(source) = indexed {
        PreparedAudio::from_indexed_vorbis(source)
    } else {
        decode_pcm(&encoded)?
    };
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
    let bytes = encoded.len() + audio.sample_count() * size_of::<i16>();
    if bytes > MAX_CACHE {
        return Err(error("sound cache entry exceeds 128 MiB"));
    }
    let mut cache = lock_cache()?;
    while cache.bytes + bytes > MAX_CACHE || cache.entries.len() >= 128 {
        let index = cache
            .entries
            .iter()
            .position(Entry::evictable)
            .ok_or_else(|| {
                error("128 MiB decoded sound cache is pinned by loaded/playing channels")
            })?;
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

fn decode_pcm(encoded: &[u8]) -> RuntimeResult<PreparedAudio> {
    let (codec, sample_rate, channels, samples) = if encoded.starts_with(b"OggS") {
        let (metadata, samples) = vorbis::decode(encoded)?;
        (
            AudioCodec::OggVorbis,
            metadata.sample_rate,
            metadata.channels,
            samples,
        )
    } else if encoded.starts_with(b"RIFF") && encoded.get(8..12) == Some(b"WAVE") {
        crate::wav_decode::decode(encoded)?
    } else {
        return Err(error(
            "sound is neither Ogg/Vorbis nor a supported RIFF/WAVE codec",
        ));
    };
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

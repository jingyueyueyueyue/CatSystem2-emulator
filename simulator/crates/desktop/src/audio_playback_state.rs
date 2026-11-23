//! Real physical PCM checkpoint preparation. Device queues retain exactly the
//! accepted, unconsumed samples; source cursors are never inferred from ticks.
use super::*;
use crate::audio_control::{PcmCursor, PlaybackRepeats, SignalMeter, AudioLoop};
use crate::audio_control::checkpoint::{ParametersState, RegistryLease};
use crate::audio_decode::checkpoint::PcmState;
use crate::media_process::error;
use cs2_runtime::graph_serde::{Serialize, Deserialize};
use cs2_runtime::object_graph as wire;
use std::collections::{BTreeMap, VecDeque};

#[derive(Clone, Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(crate) struct QueueState {
    consumed: u64,
    submitted: u64,
    chunks: VecDeque<Vec<i16>>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(crate) struct WorkerState {
    pub(crate) cursor: PcmCursor,
    pub(crate) repeats: PlaybackRepeats,
    pub(crate) signal: SignalMeter,
    pub(crate) parameters: ParametersState,
    pub(crate) loop_spec: AudioLoop,
    pub(crate) initial_silence: u64,
    pub(crate) silent_frames: u64,
    pub(crate) pass_marked: bool,
    pub(crate) paused: bool,
    pub(crate) queue: QueueState,
}
impl WorkerState {
    fn validate(&mut self, source: &PreparedAudio, parameters: &AudioParameters) -> cs2_runtime::RuntimeResult<()> {
        self.cursor.validate_checkpoint(source.probe.frames as usize, self.loop_spec)?;
        self.cursor.end(parameters)?;
        self.repeats.validate_checkpoint(parameters)?;
        let frames = self.queue.validate(source.probe.channels)?;
        self.signal.validate_checkpoint(source.probe.sample_rate, self.queue.consumed, frames)?;
        self.parameters.validate_consumed(self.queue.consumed)?;
        if self.silent_frames > self.initial_silence || self.initial_silence > u64::from(source.probe.sample_rate) * u64::from(u32::MAX) / 1000 {
            return Err(error("SOUND checkpoint has invalid front-silence cursor"));
        }
        Ok(())
    }
}
impl QueueState {
    pub(crate) fn consumed_frames(&self) -> u64 { self.consumed }
    fn validate(&self, channels: u8) -> cs2_runtime::RuntimeResult<u64> {
        let mut samples = 0usize;
        for chunk in &self.chunks {
            samples = samples.checked_add(chunk.len()).ok_or_else(|| error("PCM queue overflow"))?;
            if chunk.is_empty() || chunk.len() > 48_000 || chunk.len() % usize::from(channels) != 0 {
                return Err(error("SOUND checkpoint device queue is not frame aligned"));
            }
        }
        let frames = (samples / usize::from(channels)) as u64;
        if self.chunks.len() > 1024 || samples > 4 * 1024 * 1024
            || self.consumed.checked_add(frames) != Some(self.submitted) {
            return Err(error("SOUND checkpoint accepted/consumed PCM queue differs"));
        }
        Ok(frames)
    }
}

#[cfg(any(windows, target_os = "android"))]
pub(crate) struct TrackedOutput {
    output: Box<dyn crate::movie_audio::MovieAudioOutput>,
    queue: QueueState,
    base: u64,
    rate: u32,
    channels: u8,
    unavailable: Option<String>,
}
#[cfg(any(windows, target_os = "android"))]
impl TrackedOutput {
    pub(crate) fn open(rate: u32, channels: u8, saved: Option<&QueueState>) -> cs2_runtime::RuntimeResult<Self> {
        Self::from_output(rate, channels, saved, crate::movie_audio::open_pcm(rate, channels)?)
    }
    pub(crate) fn from_output(
        rate: u32,
        channels: u8,
        saved: Option<&QueueState>,
        mut output: Box<dyn crate::movie_audio::MovieAudioOutput>,
    ) -> cs2_runtime::RuntimeResult<Self> {
        output.pause(true)?;
        let queue = if let Some(saved) = saved {
            saved.validate(channels)?;
            for chunk in &saved.chunks {
                if !output.submit(chunk)? { return Err(error("restored SOUND queue exceeds the actual paused device capacity")); }
            }
            saved.clone()
        } else { QueueState { consumed: 0, submitted: 0, chunks: VecDeque::new() } };
        Ok(Self { base: queue.consumed, output, queue, rate, channels, unavailable: None })
    }
    fn retire(&mut self, frames: u64) -> cs2_runtime::RuntimeResult<()> {
        let consumed = self.base.checked_add(frames).ok_or_else(|| error("SOUND device timeline overflow"))?;
        if consumed < self.queue.consumed || consumed > self.queue.submitted { return Err(error("SOUND device frame counter left its submitted PCM timeline")); }
        let mut remaining = (consumed - self.queue.consumed) as usize;
        while remaining != 0 {
            let front = self.queue.chunks.front_mut().ok_or_else(|| error("SOUND device queue lost accepted PCM"))?;
            let count = remaining.min(front.len() / usize::from(self.channels));
            front.drain(..count * usize::from(self.channels));
            remaining -= count;
            if front.is_empty() { self.queue.chunks.pop_front(); }
        }
        self.queue.consumed = consumed;
        Ok(())
    }
    pub(crate) fn capture(&mut self) -> cs2_runtime::RuntimeResult<QueueState> {
        if let Some(reason) = &self.unavailable { return Err(error(reason)); }
        let frames = self.output.checkpoint_position_frames()?;
        self.retire(frames)?;
        self.queue.validate(self.channels)?;
        Ok(self.queue.clone())
    }
}
#[cfg(any(windows, target_os = "android"))]
impl crate::movie_audio::MovieAudioOutput for TrackedOutput {
    fn submit(&mut self, samples: &[i16]) -> cs2_runtime::RuntimeResult<bool> {
        let submitted = self.queue.submitted.checked_add((samples.len() / usize::from(self.channels)) as u64)
            .ok_or_else(|| error("PCM submission timeline overflow"))?;
        if !self.output.submit(samples)? { return Ok(false); }
        self.queue.submitted = submitted;
        if self.unavailable.is_none() {
            self.queue.chunks.push_back(samples.to_vec());
            if self.queue.chunks.len() > 1024 || self.queue.chunks.iter().map(Vec::len).sum::<usize>() > 4 * 1024 * 1024 {
                self.unavailable = Some("SOUND device queue exceeded checkpoint tracking bound".into());
                self.queue.chunks.clear();
            }
        }
        Ok(true)
    }
    fn position_seconds(&mut self) -> cs2_runtime::RuntimeResult<f64> {
        let seconds = self.output.position_seconds()?;
        if self.unavailable.is_none() {
            let result = self.output.checkpoint_position_frames().and_then(|frames| self.retire(frames));
            if let Err(cause) = result {
                self.unavailable = Some(cause.to_string());
                self.queue.chunks.clear();
            }
        }
        Ok(self.base as f64 / f64::from(self.rate) + seconds)
    }
    fn checkpoint_position_frames(&mut self) -> cs2_runtime::RuntimeResult<u64> { Ok(self.capture()?.consumed) }
    fn drained(&mut self) -> cs2_runtime::RuntimeResult<bool> { self.output.drained() }
    fn pause(&mut self, value: bool) -> cs2_runtime::RuntimeResult<()> {
        let _timing = crate::perf_detail::audio::Stage::Activate.begin();
        self.output.pause(value)
    }
    fn start_with_acknowledgement(&mut self, acknowledge: &mut dyn FnMut() -> bool) -> cs2_runtime::RuntimeResult<bool> {
        let _timing = crate::perf_detail::audio::Stage::Activate.begin();
        self.output.start_with_acknowledgement(acknowledge)
    }
    fn stop(&mut self) -> cs2_runtime::RuntimeResult<()> { self.output.stop() }
}

#[derive(Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct Playback {
    channel: AudioChannel,
    resource: String,
    source: PcmState,
    worker: WorkerState,
    ogg: bool,
    #[serde(skip)]
    captured_source: Option<Arc<PreparedAudio>>,
}
#[derive(Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct Snapshot { version: u32, playbacks: BTreeMap<u64, Playback> }

/// Keeps all captured workers frozen through the enclosing physical-state
/// serialization. Drop restores their existing pause/lifecycle policy.
pub(crate) struct CapturedAudioState {
    snapshot: Snapshot,
    release: Vec<mpsc::Sender<WavWorkerCommand>>,
}
impl Drop for CapturedAudioState {
    fn drop(&mut self) { for command in &self.release { let _ = command.send(WavWorkerCommand::ReleaseCapture); } }
}
pub(crate) struct PreparedAudioState { host: DesktopAudioHost, registry: RegistryLease }
impl CapturedAudioState {
    pub(crate) fn matches_reference(&self, handle: u64, resource: &str, paused: bool, channel: AudioChannel) -> bool {
        self.snapshot.playbacks.get(&handle).is_some_and(|p| p.resource == resource && p.worker.paused == paused && p.channel == channel)
    }
    pub(crate) fn encode(&self) -> cs2_runtime::RuntimeResult<Vec<u8>> { wire::encode_component(&self.snapshot) }
    pub(crate) fn matches(&self, handle: u64, resource: &str, source: &PreparedAudio, signal: &Arc<audio_control::AudioSignal>, paused: bool) -> bool {
        self.snapshot.playbacks.get(&handle).is_some_and(|saved|
            saved.resource == resource && saved.source_matches(source)
                && saved.worker.parameters_signal_matches(signal) && saved.worker.paused == paused)
    }
    pub(crate) fn handles(&self) -> impl Iterator<Item = &u64> { self.snapshot.playbacks.keys() }
}
impl Playback {
    fn source_matches(&self, source: &PreparedAudio) -> bool {
        self.captured_source.as_deref().is_some_and(|captured| std::ptr::eq(captured, source))
            || self.source.matches_source(source).unwrap_or(false)
    }
}
impl WorkerState {
    fn parameters_signal_matches(&self, signal: &audio_control::AudioSignal) -> bool { self.parameters.signal_matches(signal) }
}
impl PreparedAudioState {
    pub(crate) fn matches_reference(&self, handle: u64, resource: &str, paused: bool, channel: AudioChannel) -> bool {
        self.host.playback(handle).is_some_and(|p| p.resource == resource && p.paused == paused && p.channel == channel)
    }
    pub(crate) fn signal(&self, handle: u64) -> Option<Arc<audio_control::AudioSignal>> {
        self.host.parameters(handle).map(|p| p.signal_arc())
    }
    pub(crate) fn matches(&self, handle: u64, resource: &str, source: &PreparedAudio, signal: &audio_control::AudioSignal, paused: bool) -> cs2_runtime::RuntimeResult<bool> {
        let Some(playback) = self.host.playback(handle) else { return Ok(false); };
        Ok(playback.resource == resource && playback.paused == paused
            && playback._parameters.signal_arc().checkpoint() == signal.checkpoint()
            && playback.prepared.matches_pcm(source)?)
    }
    pub(crate) fn handles(&self) -> impl Iterator<Item = &u64> { self.host.handles.keys() }
}
impl DesktopAudioHost {
    pub(crate) fn checkpoint_owns_signal(&self, handle: u64, signal: &Arc<audio_control::AudioSignal>) -> bool {
        self.parameters(handle).is_some_and(|parameters| Arc::ptr_eq(&parameters.signal_arc(), signal))
    }
    fn playback(&self, id: u64) -> Option<&ActiveWavPlayback> {
        match self.handles.get(&id)? {
            AudioBackendHandle::Ogg(inner, _) => self.ogg.active.get(inner),
            AudioBackendHandle::Wav(inner, _) => self.wav.active.get(inner),
        }
    }
    fn parameters(&self, id: u64) -> Option<&Arc<AudioParameters>> { self.playback(id).map(|p| &p._parameters) }
    pub(crate) fn freeze_checkpoint(&self) -> cs2_runtime::RuntimeResult<CapturedAudioState> {
        if self.handles.len() > 64 || self.ogg.active.len() + self.wav.active.len() != self.handles.len() {
            return Err(error("SOUND checkpoint host/backend ownership differs"));
        }
        let mut saved = CapturedAudioState { snapshot: Snapshot { version: 1, playbacks: BTreeMap::new() }, release: Vec::new() };
        let mut checkpoint_pcm_bytes = 0usize;
        for (&id, backend) in &self.handles {
            let playback = self.playback(id).ok_or_else(|| error("SOUND checkpoint handle lost its actual backend"))?;
            let (inner, category) = match backend {
                AudioBackendHandle::Ogg(inner, category) | AudioBackendHandle::Wav(inner, category) => (*inner, *category),
            };
            if inner != id || category != playback.channel {
                return Err(error("SOUND checkpoint public/backend channel identity differs"));
            }
            let (tx, rx) = mpsc::sync_channel(1);
            playback.commands.send(WavWorkerCommand::Capture(tx)).map_err(|_| error("SOUND worker completed before checkpoint; poll its real result first"))?;
            saved.release.push(playback.commands.clone());
            let mut worker = rx.recv_timeout(Duration::from_secs(5)).map_err(|_| error("SOUND worker did not acknowledge checkpoint"))??;
            if worker.paused != playback.paused { return Err(error("SOUND worker/host script pause differs")); }
            worker.validate(&playback.prepared, &playback._parameters)?;
            checkpoint_pcm_bytes += playback.prepared.sample_count() * size_of::<i16>();
            if checkpoint_pcm_bytes > wire::MAX_GRAPH_BYTES {
                return Err(error("SOUND checkpoint playback PCM exceeds the portable graph byte bound"));
            }
            let source = PcmState::capture(&playback.prepared)
                .map_err(|cause| error(format!("checkpoint SOUND resource {:?}: {cause}", playback.resource)))?;
            saved.snapshot.playbacks.insert(id, Playback { channel: playback.channel.clone(), resource: playback.resource.clone(), source, worker,
                ogg: matches!(backend, AudioBackendHandle::Ogg(_, _)), captured_source: Some(playback.prepared.clone()) });
        }
        Ok(saved)
    }
    pub(crate) fn prepare_checkpoint(&self, bytes: &[u8],
        retained_handles: &std::collections::BTreeSet<u64>,
        loaded_source: impl Fn(u64, &str) -> Option<Arc<PreparedAudio>>,
    ) -> cs2_runtime::RuntimeResult<PreparedAudioState> {
        let saved: Snapshot = wire::decode_component(bytes)?;
        if saved.version != 1 || saved.playbacks.len() > 64 { return Err(error("unsupported physical audio checkpoint")); }
        let ogg_resolve = self.resolve.clone(); let wav_resolve = self.resolve.clone();
        let mut host = DesktopAudioHost { resolve: self.resolve.clone(), ogg: WavAudioHost::new(move |name| ogg_resolve(name)),
            wav: WavAudioHost::new(move |name| wav_resolve(name)), handles: HashMap::new() };
        host.ogg.suspended = self.ogg.suspended; host.wav.suspended = self.wav.suspended;
        for (id, mut playback) in saved.playbacks {
            if id == 0 || id == u64::MAX || playback.resource.is_empty() || playback.resource.len() > 4096 || playback.resource.contains('\0') {
                return Err(error("invalid saved physical SOUND handle/resource"));
            }
            let source = playback.source.restore()?;
            // Scoped native loads use LooseFirst, whereas ordinary runtime
            // play uses this host's resolver. Reuse the channel's already
            // verified real resource when it owns this precise handle.
            let actual = match loaded_source(id, &playback.resource) {
                Some(source) => source,
                None => prepare_audio((self.resolve)(&playback.resource)?)?,
            };
            if !source.matches_pcm(&actual)? || playback.ogg != (source.probe.codec == AudioCodec::OggVorbis) {
                return Err(error("saved SOUND PCM differs from its actual resource/decoder"));
            }
            let parameters = playback.worker.parameters.restore()?;
            playback.worker.validate(&source, &parameters)?;
            let backend = if playback.ogg { &mut host.ogg } else { &mut host.wav };
            backend.restore_playback(id, playback.channel.clone(), playback.resource, actual, parameters, playback.worker)?;
            host.handles.insert(id, if playback.ogg { AudioBackendHandle::Ogg(id, playback.channel) } else { AudioBackendHandle::Wav(id, playback.channel) });
        }
        let parameters = host.handles.keys().map(|&id| (id, host.parameters(id).unwrap().clone())).collect();
        let registry = audio_control::checkpoint::prepare_registry(parameters, self.handles.keys().copied().collect(), retained_handles)?;
        Ok(PreparedAudioState { host, registry })
    }
    pub(crate) fn commit_checkpoint(&mut self, prepared: PreparedAudioState) {
        let PreparedAudioState { host, registry } = prepared;
        let old = std::mem::replace(self, host);
        // Stop old real devices before opening the start gate of their
        // prepared replacements. Cleanup uses the existing worker destructor.
        drop(old);
        registry.commit();
        for playback in self.ogg.active.values().chain(self.wav.active.values()) {
            // A late device failure remains on finished and is surfaced by
            // the normal poll path; commit itself performs no device calls.
            let _ = playback.commands.send(WavWorkerCommand::CommitRestore);
        }
    }
}

impl WavAudioHost {
    fn restore_playback(&mut self, id: u64, channel: AudioChannel, resource: String,
        prepared: Arc<PreparedAudio>, parameters: Arc<AudioParameters>, saved: WorkerState,
    ) -> cs2_runtime::RuntimeResult<()> {
        #[cfg(not(any(windows, target_os = "android")))]
        { let _ = (id, channel, resource, prepared, parameters, saved); return Err(error("physical SOUND restore requires Windows/Android PCM output")); }
        #[cfg(any(windows, target_os = "android"))]
        {
            let paused = saved.paused;
            let (commands, rx) = mpsc::channel(); let (started, ready) = mpsc::sync_channel(1);
            let (done, finished) = mpsc::channel();
            let source = prepared.clone(); let controls = parameters.clone(); let suspended = self.suspended;
            let worker_resource = resource.clone();
            let worker = thread::Builder::new().name(format!("cs2-pcm-restore-{id}")).spawn(move || {
                let startup_error = started.clone();
                let result = super::pcm_worker::run_playback(
                    source, &worker_resource, controls, 0, saved.loop_spec, suspended, rx, started, Some(saved), false,
                    #[cfg(target_os = "android")]
                    None,
                );
                if let Err(cause) = &result { let _ = startup_error.try_send(Err(error(cause))); }
                let _ = done.send(result);
            }).map_err(|cause| error(format!("prepare SOUND worker: {cause}")))?;
            let active = ActiveWavPlayback { channel, paused, resource, prepared, commands, finished,
                thread: Some(worker), _parameters: parameters };
            ready.recv_timeout(Duration::from_secs(10)).map_err(|_| error("restored SOUND worker did not acknowledge paused preparation"))??;
            self.active.insert(id, active);
            Ok(())
        }
    }
}

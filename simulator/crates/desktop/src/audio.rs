use super::read_entry_with_fallback;
use crate::audio_control::{self, AudioParameters};
use crate::audio_decode::{AudioCodec, AudioProbe, PreparedAudio, prepare_audio};
use crate::perf_detail::audio::{self as audio_profile, Stage};
use cs2_runtime::{
    AudioChannel, AudioControl, AudioHost, AudioRequest, AudioStart, PlaybackState, ResourceIndex, RuntimeError,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

#[path = "audio_start.rs"]
mod start;

#[path = "audio_playback_state.rs"]
pub(crate) mod checkpoint;

struct PreparedRequest {
    resource: String,
    audio: Arc<PreparedAudio>,
    silence_millis: u32,
    play_count: u32,
    signal_enabled: bool,
    loop_spec: audio_control::AudioLoop,
}
thread_local! {
    static PREPARED_REQUEST: RefCell<Option<PreparedRequest>> = const { RefCell::new(None) };
}

struct PreparedRequestGuard(Option<PreparedRequest>);
impl Drop for PreparedRequestGuard {
    fn drop(&mut self) {
        PREPARED_REQUEST.with(|slot| *slot.borrow_mut() = self.0.take());
    }
}

/// Passes prepared PCM through the unchanged runtime AudioHost ABI. Custom
/// hosts still receive the ordinary AudioRequest; desktop consumes the scoped
/// preparation synchronously, then owns it in its device worker.
pub(super) fn play_loaded_audio(
    host: &mut dyn AudioHost,
    request: AudioRequest,
    prepared: Option<Arc<PreparedAudio>>,
    silence_millis: u32,
) -> cs2_runtime::RuntimeResult<PlaybackState> {
    let count = if request.looped { 0 } else { 1 };
    play_loaded_sound(host, request, prepared, silence_millis, count, false)
}

pub(super) fn play_loaded_sound(
    host: &mut dyn AudioHost,
    request: AudioRequest,
    prepared: Option<Arc<PreparedAudio>>,
    silence_millis: u32,
    play_count: u32,
    signal_enabled: bool,
) -> cs2_runtime::RuntimeResult<PlaybackState> {
    play_loaded_sound_range(
        host,
        request,
        prepared,
        silence_millis,
        play_count,
        signal_enabled,
        audio_control::AudioLoop {
            range: [0, -1],
            whole: true,
        },
    )
}

pub(super) fn play_loaded_sound_range(
    host: &mut dyn AudioHost,
    request: AudioRequest,
    prepared: Option<Arc<PreparedAudio>>,
    silence_millis: u32,
    play_count: u32,
    signal_enabled: bool,
    loop_spec: audio_control::AudioLoop,
) -> cs2_runtime::RuntimeResult<PlaybackState> {
    let previous = PREPARED_REQUEST.with(|slot| slot.borrow_mut().take());
    let _guard = PreparedRequestGuard(previous);
    PREPARED_REQUEST.with(|slot| {
        *slot.borrow_mut() = prepared.map(|audio| PreparedRequest {
            resource: request.resource.clone(),
            audio,
            silence_millis,
            play_count,
            signal_enabled,
            loop_spec,
        })
    });
    host.play(request)
}


/// Retains the prepared decoder and loop policy before returning a pending
/// backend ticket; the device worker never depends on this thread-local scope.
pub(super) fn begin_loaded_sound_range(
    host: &mut dyn AudioHost,
    request: AudioRequest,
    prepared: Option<Arc<PreparedAudio>>,
    silence_millis: u32,
    play_count: u32,
    signal_enabled: bool,
    loop_spec: audio_control::AudioLoop,
) -> cs2_runtime::RuntimeResult<AudioStart> {
    let previous = PREPARED_REQUEST.with(|slot| slot.borrow_mut().take());
    let _guard = PreparedRequestGuard(previous);
    PREPARED_REQUEST.with(|slot| {
        *slot.borrow_mut() = prepared.map(|audio| PreparedRequest {
            resource: request.resource.clone(), audio, silence_millis,
            play_count, signal_enabled, loop_spec,
        });
    });
    host.begin_play(request)
}

fn join_worker(worker: &mut Option<JoinHandle<()>>) {
    if let Some(worker) = worker.take() {
        let _ = worker.join();
    }
}

pub struct DesktopAudioHost {
    resolve: Arc<dyn Fn(&str) -> cs2_runtime::RuntimeResult<Vec<u8>> + Send + Sync>,
    ogg: WavAudioHost,
    wav: WavAudioHost,
    handles: HashMap<u64, AudioBackendHandle>,
    pending: HashMap<u64, bool>,
    controls: HashMap<u64, bool>,
}

#[derive(Clone, Copy)]
enum AudioBackendHandle {
    Ogg(u64, AudioChannel),
    Wav(u64, AudioChannel),
}

impl DesktopAudioHost {
    /// App lifecycle pause is independent of script-native pause. Existing
    /// source cursors and desired script pause bits survive resume; sources
    /// opened while suspended are primed but do not start the device.
    pub fn set_suspended(&mut self, suspended: bool) -> cs2_runtime::RuntimeResult<()> {
        let ogg = self.ogg.set_suspended(suspended);
        let wav = self.wav.set_suspended(suspended);
        ogg.and(wav)
    }

    pub fn probe(&mut self, resource: &str) -> cs2_runtime::RuntimeResult<AudioProbe> {
        prepare_audio((self.resolve)(resource)?).map(|prepared| prepared.probe)
    }

    pub fn new(
        resolve: impl Fn(&str) -> cs2_runtime::RuntimeResult<Vec<u8>> + Send + Sync + 'static,
    ) -> Self {
        let resolver: Arc<
            dyn Fn(&str) -> cs2_runtime::RuntimeResult<Vec<u8>> + Send + Sync + 'static,
        > = Arc::new(resolve);
        let ogg_resolver = resolver.clone();
        let wav_resolver = resolver.clone();
        Self {
            resolve: resolver,
            ogg: WavAudioHost::new(move |name| ogg_resolver(name)),
            wav: WavAudioHost::new(move |name| wav_resolver(name)),
            handles: HashMap::new(),
            pending: HashMap::new(),
            controls: HashMap::new(),
        }
    }

    pub fn is_active(&mut self, handle: u64) -> cs2_runtime::RuntimeResult<bool> {
        let Some(backend) = self.handles.get(&handle).copied() else {
            return Ok(false);
        };
        let active = match backend {
            AudioBackendHandle::Ogg(backend_handle, _) => self.ogg.is_active(backend_handle)?,
            AudioBackendHandle::Wav(backend_handle, _) => self.wav.is_active(backend_handle)?,
        };
        if !active {
            self.handles.remove(&handle);
        }
        Ok(active)
    }

    /// Reap completed channels even when the script never queries status.
    pub fn poll(&mut self) -> cs2_runtime::RuntimeResult<()> {
        self.ogg.reap_retiring();
        self.wav.reap_retiring();
        let handles = self.handles.keys().copied().collect::<Vec<_>>();
        let mut first_error = None;
        for handle in handles {
            if let Err(error) = self.is_active(handle) {
                first_error.get_or_insert(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }


    /// Gates only device startup; tickets, controls, cancellation, promotion,
    /// and reaping use the real backend paths in engine scheduling tests.
    #[cfg(test)]
    pub(crate) fn begin_test_pending_start(
        &mut self,
        request: AudioRequest,
        completion: mpsc::Receiver<cs2_runtime::RuntimeResult<()>>,
    ) -> cs2_runtime::RuntimeResult<AudioStart> {
        self.begin_test_pending_start_with_controls(request, completion, None)
    }

    #[cfg(test)]
    pub(crate) fn begin_test_pending_start_with_controls(
        &mut self,
        request: AudioRequest,
        completion: mpsc::Receiver<cs2_runtime::RuntimeResult<()>>,
        controls: Option<mpsc::Receiver<cs2_runtime::RuntimeResult<()>>>,
    ) -> cs2_runtime::RuntimeResult<AudioStart> {
        self.ogg.reap_retiring(); self.wav.reap_retiring();
        if self.ogg.worker_count() + self.wav.worker_count() >= MAX_PCM_WORKERS {
            return Err(RuntimeError::Host("too many live or retiring PCM workers".into()));
        }
        let handle = audio_control::allocate_handle()?;
        let parameters = AudioParameters::new(request.volume, request.pan)?;
        audio_control::register(handle, &parameters)?;
        let (commands, command_receiver) = mpsc::channel();
        let (started, receiver) = mpsc::sync_channel(1);
        let (done, finished) = mpsc::channel();
        let worker = thread::spawn(move || {
            let result = completion.recv_timeout(Duration::from_secs(5)).unwrap_or_else(|_| Err(RuntimeError::Host("test driver disconnected or timed out".into())));
            let acknowledgement = result.as_ref().map(|_| ()).map_err(|cause| match cause {
                RuntimeError::AudioStartTimeout(message) => RuntimeError::AudioStartTimeout(message.clone()),
                _ => RuntimeError::Host(cause.to_string()),
            });
            let accepted = started.send(acknowledgement).is_ok();
            let result = if accepted && result.is_ok() {
                let mut result = Ok(());
                while let Ok(command) = command_receiver.recv() {
                    let response = controls.as_ref().map_or(Ok(()), |gate| gate.recv_timeout(Duration::from_secs(5))
                        .unwrap_or_else(|_| Err(RuntimeError::Host("test control driver timed out".into()))));
                    match command {
                        WavWorkerCommand::Stop(_) => { result = response; break; }
                        WavWorkerCommand::Pause(_, reply, _) | WavWorkerCommand::Suspend(_, reply) => { let _ = reply.send(response); }
                        _ => {}
                    }
                }
                result
            } else { result };
            let _ = done.send(result);
        });
        let prepared = Arc::new(PreparedAudio::from_pcm(crate::audio_decode::DecodedAudio {
            probe: AudioProbe { codec: AudioCodec::WavPcm, sample_rate: 44_100, channels: 1, frames: 0 },
            samples: Vec::new().into(),
        }));
        let channel = request.channel;
        let resource = request.resource.clone();
        self.wav.install_start(handle, request, ActiveWavPlayback {
            channel, paused: false, commands, finished,
            thread: Some(worker), _parameters: parameters, resource, prepared,
        }, receiver);
        self.pending.insert(handle, true);
        Ok(AudioStart::Pending(handle))
    }

    fn play_backend(
        &mut self,
        request: AudioRequest,
        prepared: Arc<PreparedAudio>,
        silence_millis: u32,
        play_count: u32,
        signal_enabled: bool,
    ) -> cs2_runtime::RuntimeResult<PlaybackState> {
        self.ogg.reap_retiring();
        self.wav.reap_retiring();
        if self.ogg.worker_count() + self.wav.worker_count() >= MAX_PCM_WORKERS {
            return Err(RuntimeError::Host("too many live or retiring PCM workers".into()));
        }
        let wav = prepared.probe.codec != AudioCodec::OggVorbis;
        let playback = if wav {
            self.wav.play_prepared(
                request.clone(),
                prepared,
                silence_millis,
                play_count,
                signal_enabled,
            )?
        } else {
            self.ogg.play_prepared(
                request.clone(),
                prepared,
                silence_millis,
                play_count,
                signal_enabled,
            )?
        };
        let public_handle = playback.handle;
        self.handles.insert(
            public_handle,
            if wav {
                AudioBackendHandle::Wav(playback.handle, request.channel)
            } else {
                AudioBackendHandle::Ogg(playback.handle, request.channel)
            },
        );
        Ok(PlaybackState {
            handle: public_handle,
            resource: playback.resource,
            looped: playback.looped,
            paused: playback.paused,
        })
    }
}

impl AudioHost for DesktopAudioHost {
    fn play(&mut self, request: AudioRequest) -> cs2_runtime::RuntimeResult<PlaybackState> {
        self.poll()?;
        let loaded = PREPARED_REQUEST.with(|slot| {
            slot.borrow()
                .as_ref()
                .filter(|loaded| loaded.resource == request.resource)
                .map(|loaded| {
                    (
                        loaded.audio.clone(),
                        loaded.silence_millis,
                        loaded.play_count,
                        loaded.signal_enabled,
                    )
                })
        });
        let (prepared, silence_millis, play_count, signal_enabled) = match loaded {
            Some(loaded) => loaded,
            None => (
                prepare_audio((self.resolve)(&request.resource)?)?,
                0,
                if request.looped { 0 } else { 1 },
                false,
            ),
        };
        self.play_backend(
            request,
            prepared,
            silence_millis,
            play_count,
            signal_enabled,
        )
    }

    fn begin_play(&mut self, request: AudioRequest) -> cs2_runtime::RuntimeResult<AudioStart> {
        self.poll()?;
        let loaded = PREPARED_REQUEST.with(|slot| {
            slot.borrow()
                .as_ref()
                .filter(|loaded| loaded.resource == request.resource)
                .map(|loaded| {
                    (
                        loaded.audio.clone(),
                        loaded.silence_millis,
                        loaded.play_count,
                        loaded.signal_enabled,
                    )
                })
        });
        let (prepared, silence_millis, play_count, signal_enabled) = match loaded {
            Some(loaded) => loaded,
            None => (
                prepare_audio((self.resolve)(&request.resource)?)?,
                0,
                if request.looped { 0 } else { 1 },
                false,
            ),
        };
        if self.ogg.worker_count() + self.wav.worker_count() >= MAX_PCM_WORKERS {
            return Err(RuntimeError::Host("too many live or retiring PCM workers".into()));
        }
        let wav = prepared.probe.codec != AudioCodec::OggVorbis;
        let backend = if wav { &mut self.wav } else { &mut self.ogg };
        let ticket = backend.begin_prepared(request, prepared, silence_millis, play_count, signal_enabled, true)?;
        self.pending.insert(ticket, wav);
        Ok(AudioStart::Pending(ticket))
    }

    fn poll_play(&mut self, ticket: u64) -> cs2_runtime::RuntimeResult<Option<PlaybackState>> {
        let wav = *self.pending.get(&ticket).ok_or_else(|| RuntimeError::Host(format!("audio start ticket {ticket} is not pending")))?;
        let backend = if wav { &mut self.wav } else { &mut self.ogg };
        match backend.poll_play(ticket) {
            Ok(None) => Ok(None),
            Ok(Some(playback)) => {
                self.pending.remove(&ticket);
                let channel = backend.active.get(&playback.handle).unwrap().channel;
                self.handles.insert(playback.handle, if wav {
                    AudioBackendHandle::Wav(playback.handle, channel)
                } else {
                    AudioBackendHandle::Ogg(playback.handle, channel)
                });
                Ok(Some(playback))
            }
            Err(cause) => { self.pending.remove(&ticket); Err(cause) }
        }
    }

    fn cancel_play(&mut self, ticket: u64) -> cs2_runtime::RuntimeResult<()> {
        if let Some(wav) = self.pending.remove(&ticket) {
            if wav { self.wav.cancel_play(ticket) } else { self.ogg.cancel_play(ticket) }
        } else { Ok(()) }
    }

    fn begin_stop(&mut self, handle: u64) -> cs2_runtime::RuntimeResult<AudioControl> {
        let Some(backend) = self.handles.get(&handle).copied() else { return Ok(AudioControl::Ready); };
        if self.controls.len() >= MAX_PCM_WORKERS { return Err(RuntimeError::Host("too many pending audio controls".into())); }
        let (wav, result) = match backend {
            AudioBackendHandle::Wav(inner, _) => (true, self.wav.begin_stop(inner)?),
            AudioBackendHandle::Ogg(inner, _) => (false, self.ogg.begin_stop(inner)?),
        };
        self.handles.remove(&handle);
        if let AudioControl::Pending(ticket) = result { self.controls.insert(ticket, wav); }
        Ok(result)
    }

    fn begin_pause(&mut self, handle: u64, paused: bool) -> cs2_runtime::RuntimeResult<AudioControl> {
        if self.controls.len() >= MAX_PCM_WORKERS { return Err(RuntimeError::Host("too many pending audio controls".into())); }
        let (wav, result) = match self.handles.get(&handle) {
            Some(AudioBackendHandle::Wav(inner, _)) => (true, self.wav.begin_pause(*inner, paused)?),
            Some(AudioBackendHandle::Ogg(inner, _)) => (false, self.ogg.begin_pause(*inner, paused)?),
            None => return Err(RuntimeError::Host(format!("audio playback handle {handle} is not active"))),
        };
        if let AudioControl::Pending(ticket) = result { self.controls.insert(ticket, wav); }
        Ok(result)
    }

    fn poll_control(&mut self, ticket: u64) -> cs2_runtime::RuntimeResult<Option<()>> {
        let wav = *self.controls.get(&ticket).ok_or_else(|| RuntimeError::Host(format!("audio control ticket {ticket} is not pending")))?;
        let result = if wav { self.wav.poll_control(ticket) } else { self.ogg.poll_control(ticket) };
        if !matches!(result, Ok(None)) { self.controls.remove(&ticket); }
        result
    }

    fn cancel_control(&mut self, ticket: u64) -> cs2_runtime::RuntimeResult<()> {
        if let Some(wav) = self.controls.remove(&ticket) {
            if wav { self.wav.cancel_control(ticket) } else { self.ogg.cancel_control(ticket) }
        } else { Ok(()) }
    }

    fn stop(&mut self, handle: u64) -> cs2_runtime::RuntimeResult<()> {
        let Some(backend) = self.handles.remove(&handle) else {
            return Ok(());
        };
        match backend {
            AudioBackendHandle::Ogg(handle, _) => self.ogg.stop(handle),
            AudioBackendHandle::Wav(handle, _) => self.wav.stop(handle),
        }
    }

    fn pause(&mut self, handle: u64, paused: bool) -> cs2_runtime::RuntimeResult<()> {
        match self.handles.get(&handle) {
            Some(AudioBackendHandle::Ogg(handle, _)) => self.ogg.pause(*handle, paused),
            Some(AudioBackendHandle::Wav(handle, _)) => self.wav.pause(*handle, paused),
            None => Err(RuntimeError::Host(format!(
                "audio playback handle {handle} is not active"
            ))),
        }
    }

    fn stop_all(&mut self, channel: AudioChannel) -> cs2_runtime::RuntimeResult<()> {
        let ogg_result = self.ogg.stop_all(channel);
        let wav_result = self.wav.stop_all(channel);
        self.pending.retain(|ticket, wav| if *wav { self.wav.pending.contains_key(ticket) } else { self.ogg.pending.contains_key(ticket) });
        self.controls.retain(|ticket, wav| if *wav { self.wav.controls.contains_key(ticket) } else { self.ogg.controls.contains_key(ticket) });
        self.handles.retain(|_, backend| match backend {
            AudioBackendHandle::Ogg(_, active_channel)
            | AudioBackendHandle::Wav(_, active_channel) => *active_channel != channel,
        });
        ogg_result.and(wav_result)
    }

    fn is_active(&mut self, handle: u64) -> cs2_runtime::RuntimeResult<bool> {
        DesktopAudioHost::is_active(self, handle)
    }
}

impl Drop for DesktopAudioHost {
    fn drop(&mut self) {
        for channel in [
            AudioChannel::Bgm,
            AudioChannel::Voice,
            AudioChannel::SoundEffect,
        ] {
            let _ = self.stop_all(channel);
        }
    }
}

pub fn audio_host_for_resources(resources: &ResourceIndex) -> DesktopAudioHost {
    let resources = resources.clone();
    DesktopAudioHost::new(move |name| {
        read_entry_with_fallback(&resources, name)
            .map_err(|error| RuntimeError::Host(error.to_string()))
    })
}

enum WavWorkerCommand {
    Pause(bool, mpsc::SyncSender<cs2_runtime::RuntimeResult<()>>, Option<audio_profile::Shared>),
    Suspend(bool, mpsc::SyncSender<cs2_runtime::RuntimeResult<()>>),
    Stop(Option<audio_profile::Shared>),
    Capture(mpsc::SyncSender<cs2_runtime::RuntimeResult<checkpoint::WorkerState>>),
    ReleaseCapture,
    CommitRestore,
}

struct ActiveWavPlayback {
    channel: AudioChannel,
    paused: bool,
    commands: mpsc::Sender<WavWorkerCommand>,
    finished: mpsc::Receiver<cs2_runtime::RuntimeResult<()>>,
    thread: Option<JoinHandle<()>>,
    _parameters: Arc<AudioParameters>,
    resource: String,
    prepared: Arc<PreparedAudio>,
}

impl Drop for ActiveWavPlayback {
    fn drop(&mut self) {
        if self.thread.is_some() {
            let _ = self.commands.send(WavWorkerCommand::Stop(None));
            join_worker(&mut self.thread);
        }
    }
}

const MAX_PCM_WORKERS: usize = 64;

struct PendingWavPlayback {
    acknowledgement: start::StartAcknowledgement<RuntimeError>,
    playback: PlaybackState,
    active: ActiveWavPlayback,
}

enum PendingWavControl {
    Pause { handle: u64, channel: AudioChannel, paused: bool, acknowledgement: start::StartAcknowledgement<RuntimeError> },
    Stop { active: ActiveWavPlayback, deadline: Instant },
}

/// PCM output for RIFF/WAVE and Ogg/Vorbis, using bounded WinMM/AAudio buffers.
/// The historical `WavAudioHost` name remains available to existing callers.
pub struct WavAudioHost {
    resolve: Arc<dyn Fn(&str) -> cs2_runtime::RuntimeResult<Vec<u8>> + Send + Sync + 'static>,
    active: HashMap<u64, ActiveWavPlayback>,
    pending: HashMap<u64, PendingWavPlayback>,
    controls: HashMap<u64, PendingWavControl>,
    retiring: Vec<ActiveWavPlayback>,
    suspended: bool,
    #[cfg(target_os = "android")]
    standby: crate::audio_android::StandbyPool,
}

impl WavAudioHost {
    pub fn new(
        resolve: impl Fn(&str) -> cs2_runtime::RuntimeResult<Vec<u8>> + Send + Sync + 'static,
    ) -> Self {
        Self {
            resolve: Arc::new(resolve),
            active: HashMap::new(),
            pending: HashMap::new(),
            controls: HashMap::new(),
            retiring: Vec::new(),
            suspended: false,
            #[cfg(target_os = "android")]
            standby: crate::audio_android::StandbyPool::default(),
        }
    }

    pub fn from_resources(resources: &ResourceIndex) -> Self {
        let resources = resources.clone();
        Self::new(move |name| {
            read_entry_with_fallback(&resources, name)
                .map_err(|error| RuntimeError::Host(error.to_string()))
        })
    }

    pub fn is_active(&mut self, handle: u64) -> cs2_runtime::RuntimeResult<bool> {
        self.reap_finished()?;
        Ok(self.active.contains_key(&handle))
    }

    pub fn set_suspended(&mut self, suspended: bool) -> cs2_runtime::RuntimeResult<()> {
        self.suspended = suspended;
        #[cfg(target_os = "android")]
        if suspended {
            self.standby = crate::audio_android::StandbyPool::default();
        }
        self.reap_finished()?;
        let mut first_error = None;
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut responses = Vec::with_capacity(self.active.len());
        for playback in self.active.values().chain(self.pending.values().map(|p| &p.active)) {
            let (acknowledge, response) = mpsc::sync_channel(1);
            if playback
                .commands
                .send(WavWorkerCommand::Suspend(suspended, acknowledge))
                .is_err()
            {
                first_error.get_or_insert_with(|| {
                    RuntimeError::Host("PCM worker stopped during app suspend/resume".into())
                });
            } else {
                responses.push(response);
            }
        }
        // Signal all streams before waiting. Activity pause must not serialize
        // up to64 per-device timeouts on the engine thread.
        for response in responses {
            let result = response
                .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
                .map_err(|_| {
                    RuntimeError::Host("PCM worker did not acknowledge app suspend/resume".into())
                })
                .and_then(|result| result);
            if let Err(cause) = result {
                first_error.get_or_insert(cause);
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    fn worker_count(&self) -> usize {
        self.active.len() + self.pending.len() + self.retiring.len()
            + self.controls.values().filter(|c| matches!(c, PendingWavControl::Stop { .. })).count()
    }

    fn reap_retiring(&mut self) {
        self.retiring.retain_mut(|playback| {
            if playback.thread.as_ref().is_some_and(JoinHandle::is_finished) {
                join_worker(&mut playback.thread);
                false
            } else { true }
        });
    }

    fn retire(&mut self, playback: ActiveWavPlayback) {
        let _ = playback.commands.send(WavWorkerCommand::Stop(None));
        self.retiring.push(playback);
        self.reap_retiring();
    }

    fn reap_finished(&mut self) -> cs2_runtime::RuntimeResult<()> {
        self.reap_retiring();
        let completed = self
            .active
            .iter()
            .filter_map(|(&handle, playback)| match playback.finished.try_recv() {
                Ok(result) => Some((handle, result)),
                Err(mpsc::TryRecvError::Disconnected) => Some((
                    handle,
                    Err(RuntimeError::Host(
                        "WAV playback worker exited without a result".to_string(),
                    )),
                )),
                Err(mpsc::TryRecvError::Empty) => None,
            })
            .collect::<Vec<_>>();
        let mut first_error = None;
        for (handle, result) in completed {
            if let Some(mut playback) = self.active.remove(&handle) {
                join_worker(&mut playback.thread);
            }
            if let Err(error) = result {
                first_error.get_or_insert(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    fn stop_playback(&mut self, handle: u64) -> cs2_runtime::RuntimeResult<()> {
        let Some(mut playback) = self.active.remove(&handle) else {
            return Ok(());
        };
        let _timing = Stage::Ack.begin();
        let _ = playback.commands.send(WavWorkerCommand::Stop(audio_profile::active()));
        let worker_result = playback.finished.recv_timeout(Duration::from_secs(5));
        let worker_result = match worker_result {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                Err(RuntimeError::Host("WAV worker dropped result".to_string()))
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                return Err(RuntimeError::Host("WAV worker stop timed out".to_string()));
            }
        };
        join_worker(&mut playback.thread);
        worker_result
    }
}

impl AudioHost for WavAudioHost {
    fn play(&mut self, request: AudioRequest) -> cs2_runtime::RuntimeResult<PlaybackState> {
        self.reap_finished()?;
        if !request.volume.is_finite() || !request.pan.is_finite() {
            return Err(RuntimeError::Host(
                "audio volume and pan must be finite".to_string(),
            ));
        }
        let loaded = PREPARED_REQUEST.with(|slot| {
            slot.borrow()
                .as_ref()
                .filter(|loaded| loaded.resource == request.resource)
                .map(|loaded| {
                    (
                        loaded.audio.clone(),
                        loaded.silence_millis,
                        loaded.play_count,
                        loaded.signal_enabled,
                    )
                })
        });
        let (prepared, silence, count, signal) = match loaded {
            Some(loaded) => loaded,
            None => (
                prepare_audio((self.resolve)(&request.resource)?)?,
                0,
                if request.looped { 0 } else { 1 },
                false,
            ),
        };
        self.play_prepared(request, prepared, silence, count, signal)
    }

    fn begin_play(&mut self, request: AudioRequest) -> cs2_runtime::RuntimeResult<AudioStart> {
        self.reap_finished()?;
        if !request.volume.is_finite() || !request.pan.is_finite() {
            return Err(RuntimeError::Host(
                "audio volume and pan must be finite".to_string(),
            ));
        }
        let loaded = PREPARED_REQUEST.with(|slot| {
            slot.borrow()
                .as_ref()
                .filter(|loaded| loaded.resource == request.resource)
                .map(|loaded| {
                    (
                        loaded.audio.clone(),
                        loaded.silence_millis,
                        loaded.play_count,
                        loaded.signal_enabled,
                    )
                })
        });
        let (prepared, silence, count, signal) = match loaded {
            Some(loaded) => loaded,
            None => (
                prepare_audio((self.resolve)(&request.resource)?)?,
                0,
                if request.looped { 0 } else { 1 },
                false,
            ),
        };
        self.begin_prepared(request, prepared, silence, count, signal, true).map(AudioStart::Pending)
    }

    fn poll_play(&mut self, ticket: u64) -> cs2_runtime::RuntimeResult<Option<PlaybackState>> {
        self.reap_retiring();
        let pending = self.pending.get_mut(&ticket).ok_or_else(|| RuntimeError::Host(format!("audio start ticket {ticket} is not pending")))?;
        let Some(result) = pending.acknowledgement.poll(Instant::now(), || RuntimeError::Host(
            "WAV worker did not acknowledge opening the device".into()
        )) else { return Ok(None); };
        let pending = self.pending.remove(&ticket).unwrap();
        match result {
            Ok(()) => {
                #[cfg(target_os = "android")]
                self.warm_device_for(pending.active.prepared.probe);
                self.active.insert(ticket, pending.active);
                Ok(Some(pending.playback))
            }
            Err(cause) => { self.retire(pending.active); Err(cause) }
        }
    }

    fn cancel_play(&mut self, ticket: u64) -> cs2_runtime::RuntimeResult<()> {
        if let Some(pending) = self.pending.remove(&ticket) {
            // A delayed worker cannot publish this canceled start.
            drop(pending.acknowledgement);
            self.retire(pending.active);
        }
        Ok(())
    }

    fn begin_stop(&mut self, handle: u64) -> cs2_runtime::RuntimeResult<AudioControl> {
        self.reap_retiring();
        if !self.active.contains_key(&handle) { return Ok(AudioControl::Ready); }
        if self.controls.len() >= MAX_PCM_WORKERS { return Err(RuntimeError::Host("too many pending audio controls".into())); }
        let ticket = audio_control::allocate_handle()?;
        let active = self.active.remove(&handle).unwrap();
        // A concurrent natural finish already owns its result in `finished`.
        let _ = active.commands.send(WavWorkerCommand::Stop(audio_profile::active()));
        self.controls.insert(ticket, PendingWavControl::Stop { active, deadline: Instant::now() + Duration::from_secs(5) });
        Ok(AudioControl::Pending(ticket))
    }

    fn begin_pause(&mut self, handle: u64, paused: bool) -> cs2_runtime::RuntimeResult<AudioControl> {
        self.reap_finished()?;
        if self.controls.len() >= MAX_PCM_WORKERS { return Err(RuntimeError::Host("too many pending audio controls".into())); }
        let active = self.active.get(&handle).ok_or_else(|| RuntimeError::Host(format!("WAV playback handle {handle} is not active")))?;
        let ticket = audio_control::allocate_handle()?;
        let (acknowledge, response) = mpsc::sync_channel(1);
        active.commands.send(WavWorkerCommand::Pause(paused, acknowledge, audio_profile::active()))
            .map_err(|_| RuntimeError::Host("WAV playback worker stopped".into()))?;
        self.controls.insert(ticket, PendingWavControl::Pause {
            handle, channel: active.channel, paused,
            acknowledgement: start::StartAcknowledgement::new(response, Instant::now() + Duration::from_secs(5)),
        });
        Ok(AudioControl::Pending(ticket))
    }

    fn poll_control(&mut self, ticket: u64) -> cs2_runtime::RuntimeResult<Option<()>> {
        self.reap_retiring();
        let pending = self.controls.get_mut(&ticket).ok_or_else(|| RuntimeError::Host(format!("audio control ticket {ticket} is not pending")))?;
        let result = match pending {
            PendingWavControl::Pause { acknowledgement, .. } => acknowledgement.poll(Instant::now(), || RuntimeError::Host("WAV worker dropped pause response".into())),
            PendingWavControl::Stop { active, deadline } => match active.finished.try_recv() {
                Ok(result) => Some(result),
                Err(mpsc::TryRecvError::Empty) if Instant::now() < *deadline => None,
                Err(mpsc::TryRecvError::Empty) => Some(Err(RuntimeError::Host("WAV worker stop timed out".into()))),
                Err(mpsc::TryRecvError::Disconnected) => Some(Err(RuntimeError::Host("WAV worker dropped result".into()))),
            },
        };
        let Some(result) = result else { return Ok(None); };
        match self.controls.remove(&ticket).unwrap() {
            PendingWavControl::Pause { handle, paused, .. } => {
                if result.is_ok() && let Some(active) = self.active.get_mut(&handle) { active.paused = paused; }
            }
            PendingWavControl::Stop { active, .. } => {
                #[cfg(target_os = "android")]
                if result.is_ok() { self.warm_device_for(active.prepared.probe); }
                self.retire(active);
            }
        }
        result.map(|_| Some(()))
    }

    fn cancel_control(&mut self, ticket: u64) -> cs2_runtime::RuntimeResult<()> {
        if let Some(PendingWavControl::Stop { active, .. }) = self.controls.remove(&ticket) { self.retire(active); }
        Ok(())
    }

    fn stop(&mut self, handle: u64) -> cs2_runtime::RuntimeResult<()> {
        #[cfg(target_os = "android")]
        let source = self.active.get(&handle).map(|playback| playback.prepared.probe);
        let result = self.stop_playback(handle);
        #[cfg(target_os = "android")]
        if result.is_ok() && let Some(source) = source {
            self.warm_device_for(source);
        }
        result
    }

    fn pause(&mut self, handle: u64, paused: bool) -> cs2_runtime::RuntimeResult<()> {
        self.reap_finished()?;
        let playback = self.active.get_mut(&handle).ok_or_else(|| {
            RuntimeError::Host(format!("WAV playback handle {handle} is not active"))
        })?;
        let (acknowledge, response) = mpsc::sync_channel(1);
        let _timing = Stage::Ack.begin();
        playback
            .commands
            .send(WavWorkerCommand::Pause(paused, acknowledge, audio_profile::active()))
            .map_err(|_| RuntimeError::Host("WAV playback worker stopped".to_string()))?;
        response
            .recv_timeout(Duration::from_secs(5))
            .map_err(|_| RuntimeError::Host("WAV worker dropped pause response".to_string()))??;
        playback.paused = paused;
        Ok(())
    }

    fn stop_all(&mut self, channel: AudioChannel) -> cs2_runtime::RuntimeResult<()> {
        let controls = self.controls.iter().filter_map(|(&ticket, control)| {
            let active_channel = match control { PendingWavControl::Pause { channel, .. } => *channel, PendingWavControl::Stop { active, .. } => active.channel };
            (active_channel == channel).then_some(ticket)
        }).collect::<Vec<_>>();
        for ticket in controls { self.cancel_control(ticket)?; }
        let pending = self.pending.iter().filter_map(|(&ticket, p)| (p.active.channel == channel).then_some(ticket)).collect::<Vec<_>>();
        for ticket in pending { self.cancel_play(ticket)?; }
        let handles = self
            .active
            .iter()
            .filter_map(|(&handle, playback)| (playback.channel == channel).then_some(handle))
            .collect::<Vec<_>>();
        let mut first_error = None;
        for handle in handles {
            if let Err(error) = self.stop_playback(handle) {
                first_error.get_or_insert(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    fn is_active(&mut self, handle: u64) -> cs2_runtime::RuntimeResult<bool> {
        WavAudioHost::is_active(self, handle)
    }
}

impl WavAudioHost {
    #[cfg(target_os = "android")]
    fn warm_device_for(&mut self, source: AudioProbe) {
        if !self.suspended && source.codec == AudioCodec::OggVorbis {
            self.standby.prepare(source.sample_rate, source.channels);
        }
    }

    fn play_prepared(
        &mut self,
        request: AudioRequest,
        prepared: Arc<PreparedAudio>,
        silence_millis: u32,
        play_count: u32,
        signal_enabled: bool,
    ) -> cs2_runtime::RuntimeResult<PlaybackState> {
        let ticket = self.begin_prepared(request, prepared, silence_millis, play_count, signal_enabled, true)?;
        let _timing = Stage::Ack.begin();
        loop {
            if let Some(playback) = self.poll_play(ticket)? { return Ok(playback); }
            thread::sleep(Duration::from_millis(1));
        }
    }

    fn install_start(
        &mut self, handle: u64, request: AudioRequest, active: ActiveWavPlayback,
        receiver: mpsc::Receiver<cs2_runtime::RuntimeResult<()>>,
    ) {
        self.pending.insert(handle, PendingWavPlayback {
            acknowledgement: start::StartAcknowledgement::new(receiver, Instant::now() + Duration::from_secs(10)),
            playback: PlaybackState { handle, resource: request.resource, looped: request.looped, paused: false },
            active,
        });
    }

    fn begin_prepared(
        &mut self,
        request: AudioRequest,
        prepared: Arc<PreparedAudio>,
        silence_millis: u32,
        play_count: u32,
        signal_enabled: bool,
        compatible_early_ack: bool,
    ) -> cs2_runtime::RuntimeResult<u64> {
        self.reap_finished()?;
        if self.worker_count() >= MAX_PCM_WORKERS {
            return Err(RuntimeError::Host("too many live or retiring PCM workers".into()));
        }
        let parameters = {
            let _timing = Stage::Register.begin();
            let parameters = AudioParameters::new(request.volume, request.pan)?;
            parameters.reset_repeat_count(play_count)?;
            parameters.enable_signal(signal_enabled);
            parameters
        };
        let loop_spec = PREPARED_REQUEST
            .with(|slot| {
                slot.borrow()
                    .as_ref()
                    .filter(|loaded| loaded.resource == request.resource)
                    .map(|loaded| loaded.loop_spec)
            })
            .unwrap_or(audio_control::AudioLoop {
                range: [0, -1],
                whole: true,
            });
        let initial_suspended = self.suspended;
        #[cfg(not(any(windows, target_os = "android")))]
        {
            let _ = (
                prepared,
                request,
                parameters,
                silence_millis,
                loop_spec,
                initial_suspended,
                compatible_early_ack,
            );
            Err(RuntimeError::Host(
                "system PCM audio output is implemented for Windows and Android only".to_string(),
            ))
        }

        #[cfg(any(windows, target_os = "android"))]
        {
            #[cfg(target_os = "android")]
            let standby = if !initial_suspended {
                self.standby.take(prepared.probe.sample_rate, prepared.probe.channels)
            } else {
                None
            };
            let handle = {
                let _timing = Stage::Register.begin();
                let handle = audio_control::allocate_handle()?;
                audio_control::register(handle, &parameters)?;
                handle
            };
            let (commands, command_receiver) = mpsc::channel();
            let (started_sender, started_receiver) = mpsc::sync_channel(1);
            let (finished_sender, finished) = mpsc::channel();
            let retained_prepared = prepared.clone();
            let worker_timing = audio_profile::active();
            // Native157 effects retain confirmed-start/nonfatal-timeout
            // semantics. BGM/voice acknowledge accepted requests as before;
            // subsequent device failures remain on the live worker result.
            let acknowledge_start_request = compatible_early_ack && cfg!(target_os = "android")
                && matches!(request.channel, AudioChannel::Bgm | AudioChannel::Voice);
            let worker = {
                let _timing = Stage::Spawn.begin();
                thread::Builder::new()
                    .name(format!("cs2-pcm-{handle}"))
                    // Keep a control owner even if a very short worker finishes
                    // before its handle is returned to the script thread.
                    .spawn({
                        let parameters = parameters.clone();
                        let resource = request.resource.clone();
                        move || {
                            let startup_error = started_sender.clone();
                            let result = audio_profile::with_worker(worker_timing, || {
                                pcm_worker::run_playback(
                                    prepared,
                                    &resource,
                                    parameters,
                                    silence_millis,
                                    loop_spec,
                                    initial_suspended,
                                    command_receiver,
                                    started_sender,
                                    None,
                                    acknowledge_start_request,
                                    #[cfg(target_os = "android")]
                                    standby,
                                )
                            });
                            if let Err(cause) = &result {
                                let _ = startup_error.try_send(Err(pcm_worker::clone_error(cause)));
                            }
                            let _ = finished_sender.send(result);
                        }
                    })
                    .map_err(|error| RuntimeError::Host(format!("start WAV worker: {error}")))?
            };
            let channel = request.channel;
            let resource = request.resource.clone();
            self.install_start(handle, request, ActiveWavPlayback {
                channel, paused: false, commands, finished,
                thread: Some(worker), _parameters: parameters,
                resource, prepared: retained_prepared,
            }, started_receiver);
            Ok(handle)
        }
    }
}

impl Drop for WavAudioHost {
    fn drop(&mut self) {
        let controls = self.controls.keys().copied().collect::<Vec<_>>();
        for ticket in controls { let _ = self.cancel_control(ticket); }
        let tickets = self.pending.keys().copied().collect::<Vec<_>>();
        for ticket in tickets { let _ = self.cancel_play(ticket); }
        let handles = self.active.keys().copied().collect::<Vec<_>>();
        for handle in handles {
            let _ = self.stop_playback(handle);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> AudioRequest {
        AudioRequest { channel: AudioChannel::SoundEffect, resource: "effect.wav".into(), volume: 1.0, pan: 0.0, looped: false }
    }

    fn poll_until_ready(host: &mut DesktopAudioHost, ticket: u64) -> cs2_runtime::RuntimeResult<PlaybackState> {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(playback) = host.poll_play(ticket)? { return Ok(playback); }
            assert!(Instant::now() < deadline, "test driver failed to complete");
            thread::yield_now();
        }
    }

    #[test]
    fn delayed_start_returns_pending_then_promotes_the_real_handle() {
        let mut host = DesktopAudioHost::new(|_| unreachable!());
        let (completion, startup) = mpsc::channel();
        let AudioStart::Pending(ticket) = host.begin_test_pending_start(request(), startup).unwrap() else { panic!("pending start required") };
        assert!(host.poll_play(ticket).unwrap().is_none());
        assert!(!host.handles.contains_key(&ticket));
        completion.send(Ok(())).unwrap();
        let playback = poll_until_ready(&mut host, ticket).unwrap();
        assert_eq!(playback.handle, ticket);
        assert_eq!(playback.resource, "effect.wav");
        assert!(host.is_active(ticket).unwrap());
        host.stop(ticket).unwrap();
    }

    #[test]
    fn cancellation_returns_while_driver_is_still_blocked_and_does_not_promote_it() {
        let mut host = DesktopAudioHost::new(|_| unreachable!());
        let (completion, startup) = mpsc::channel();
        let AudioStart::Pending(ticket) = host.begin_test_pending_start(request(), startup).unwrap() else { panic!("pending start required") };
        host.cancel_play(ticket).unwrap();
        assert_eq!(host.wav.retiring.len(), 1);
        assert!(!host.wav.retiring[0].thread.as_ref().unwrap().is_finished());
        completion.send(Ok(())).unwrap();
        assert!(host.poll_play(ticket).is_err());
        assert!(!host.is_active(ticket).unwrap());
    }

    #[test]
    fn deferred_se_start_timeout_keeps_the_native157_error_kind() {
        let mut host = DesktopAudioHost::new(|_| unreachable!());
        let (completion, startup) = mpsc::channel();
        let AudioStart::Pending(ticket) = host.begin_test_pending_start(request(), startup).unwrap() else { panic!("pending start required") };
        completion.send(Err(RuntimeError::AudioStartTimeout("device still starting".into()))).unwrap();
        assert!(matches!(poll_until_ready(&mut host, ticket), Err(RuntimeError::AudioStartTimeout(_))));
        host.poll().unwrap();
        assert!(!host.is_active(ticket).unwrap());
    }

    #[test]
    fn expired_start_can_retire_without_joining_a_blocked_driver() {
        let mut host = DesktopAudioHost::new(|_| unreachable!());
        let (completion, startup) = mpsc::channel();
        let AudioStart::Pending(ticket) = host.begin_test_pending_start(request(), startup).unwrap() else { panic!("pending start required") };
        let (_sender, receiver) = mpsc::sync_channel(1);
        host.wav.pending.get_mut(&ticket).unwrap().acknowledgement = start::StartAcknowledgement::new(receiver, Instant::now());
        assert!(host.poll_play(ticket).is_err());
        assert_eq!(host.wav.retiring.len(), 1);
        completion.send(Ok(())).unwrap();
        host.poll().unwrap();
    }

    fn delayed_control_host() -> (DesktopAudioHost, u64, mpsc::Sender<cs2_runtime::RuntimeResult<()>>) {
        let mut host = DesktopAudioHost::new(|_| unreachable!());
        let handle = audio_control::allocate_handle().unwrap();
        let parameters = AudioParameters::new(1.0, 0.0).unwrap();
        let (commands, receiver) = mpsc::channel();
        let (done, finished) = mpsc::channel();
        let (gate, driver) = mpsc::channel();
        let worker = thread::spawn(move || {
            while let Ok(command) = receiver.recv() {
                let result = driver.recv_timeout(Duration::from_secs(5)).unwrap_or_else(|_| Err(RuntimeError::Host("test driver timed out".into())));
                match command {
                    WavWorkerCommand::Pause(_, acknowledge, _) => { let _ = acknowledge.send(result); }
                    WavWorkerCommand::Stop(_) => { let _ = done.send(result); return; }
                    _ => panic!("unexpected test driver command"),
                }
            }
        });
        host.wav.active.insert(handle, ActiveWavPlayback {
            channel: AudioChannel::SoundEffect, paused: false, commands, finished, thread: Some(worker),
            _parameters: parameters, resource: "effect.wav".into(),
            prepared: Arc::new(PreparedAudio::from_pcm(crate::audio_decode::DecodedAudio {
                probe: AudioProbe { codec: AudioCodec::WavPcm, sample_rate: 44_100, channels: 1, frames: 0 },
                samples: Vec::new().into(),
            })),
        });
        host.handles.insert(handle, AudioBackendHandle::Wav(handle, AudioChannel::SoundEffect));
        (host, handle, gate)
    }

    fn poll_control_until_ready(host: &mut DesktopAudioHost, ticket: u64) -> cs2_runtime::RuntimeResult<()> {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if host.poll_control(ticket)?.is_some() { return Ok(()); }
            assert!(Instant::now() < deadline, "test driver control failed to complete");
            thread::yield_now();
        }
    }

    #[test]
    fn asynchronous_pause_keeps_frames_free_until_the_driver_acknowledges() {
        let (mut host, handle, gate) = delayed_control_host();
        let AudioControl::Pending(ticket) = host.begin_pause(handle, true).unwrap() else { panic!("pending pause required") };
        assert!(host.poll_control(ticket).unwrap().is_none());
        assert!(!host.wav.active[&handle].paused);
        gate.send(Ok(())).unwrap();
        poll_control_until_ready(&mut host, ticket).unwrap();
        assert!(host.wav.active[&handle].paused);
        let AudioControl::Pending(ticket) = host.begin_stop(handle).unwrap() else { panic!("pending stop required") };
        gate.send(Ok(())).unwrap();
        poll_control_until_ready(&mut host, ticket).unwrap();
    }

    #[test]
    fn asynchronous_stop_keeps_cleanup_errors_without_waiting_for_the_driver() {
        let (mut host, handle, gate) = delayed_control_host();
        let AudioControl::Pending(ticket) = host.begin_stop(handle).unwrap() else { panic!("pending stop required") };
        assert!(host.poll_control(ticket).unwrap().is_none());
        assert!(!host.is_active(handle).unwrap());
        gate.send(Err(RuntimeError::Host("device cleanup failed".into()))).unwrap();
        assert!(matches!(poll_control_until_ready(&mut host, ticket), Err(RuntimeError::Host(message)) if message == "device cleanup failed"));
        host.poll().unwrap();
    }

    #[test]
    fn an_expired_stop_retires_the_driver_without_joining_it() {
        let (mut host, handle, gate) = delayed_control_host();
        let AudioControl::Pending(ticket) = host.begin_stop(handle).unwrap() else { panic!("pending stop required") };
        let PendingWavControl::Stop { deadline, .. } = host.wav.controls.get_mut(&ticket).unwrap() else { panic!("stop ownership required") };
        *deadline = Instant::now();
        assert!(host.poll_control(ticket).is_err());
        assert_eq!(host.wav.retiring.len(), 1);
        assert!(!host.wav.retiring[0].thread.as_ref().unwrap().is_finished());
        gate.send(Ok(())).unwrap();
    }

    #[test]
    fn canceling_an_issued_pause_never_waits_for_its_response() {
        let (mut host, handle, gate) = delayed_control_host();
        let AudioControl::Pending(ticket) = host.begin_pause(handle, true).unwrap() else { panic!("pending pause required") };
        host.cancel_control(ticket).unwrap();
        gate.send(Ok(())).unwrap();
        let AudioControl::Pending(ticket) = host.begin_stop(handle).unwrap() else { panic!("pending stop required") };
        gate.send(Ok(())).unwrap();
        poll_control_until_ready(&mut host, ticket).unwrap();
    }

    #[test]
    fn stop_uses_the_result_of_a_worker_finishing_concurrently() {
        let mut host = WavAudioHost::new(|_| unreachable!());
        for (handle, result) in [
            (1, Ok(())),
            (2, Err(RuntimeError::Host("device cleanup failed".into()))),
        ] {
            let (commands, pending) = mpsc::channel();
            let (finished_sender, finished) = mpsc::channel();
            let worker = thread::spawn(move || {
                pending.recv().unwrap();
                drop(pending);
                finished_sender.send(result).unwrap();
            });
            host.active.insert(
                handle,
                ActiveWavPlayback {
                    channel: AudioChannel::SoundEffect,
                    paused: false,
                    commands,
                    finished,
                    thread: Some(worker),
                    _parameters: AudioParameters::new(1.0, 0.0).unwrap(),
                    resource: String::new(),
                    prepared: Arc::new(PreparedAudio::from_pcm(crate::audio_decode::DecodedAudio {
                        probe: AudioProbe {
                            codec: AudioCodec::WavPcm,
                            sample_rate: 44_100,
                            channels: 1,
                            frames: 0,
                        },
                        samples: Arc::from([]),
                    })),
                },
            );
        }
        host.stop(1).unwrap();
        assert!(matches!(
            host.stop(2),
            Err(RuntimeError::Host(message)) if message == "device cleanup failed"
        ));
        assert!(host.active.is_empty());
    }

    #[test]
    fn pcm_volume_pan_and_reused_silence_preserve_chunks() {
        let chunk = apply_pcm_volume_pan(vec![1000, 2000, -1000, -2000], 2, 0.5, 0.5);
        assert_eq!(chunk, [250, 1000, -250, -1000]);
        let chunk = apply_pcm_volume_pan(vec![1000, -1000], 1, 1.0, 0.0);
        assert_eq!(chunk, [1000, -1000]);
        let mut silence = Vec::with_capacity(8);
        silence.resize(4, 0);
        assert_eq!(silence, [0; 4]);
        silence.clear();
        silence.resize(2, 0);
        assert_eq!(silence, [0; 2]);
    }
}

fn apply_pcm_volume_pan(mut samples: Vec<i16>, channels: u8, volume: f32, pan: f32) -> Vec<i16> {
    let volume = volume.clamp(0.0, 1.0);
    let pan = pan.clamp(-1.0, 1.0);
    for frame in samples.chunks_exact_mut(usize::from(channels)) {
        let left_gain = volume
            * if channels == 2 && pan > 0.0 {
                1.0 - pan
            } else {
                1.0
            };
        frame[0] = (f32::from(frame[0]) * left_gain)
            .round()
            .clamp(-32768.0, 32767.0) as i16;
        if channels == 2 {
            let right_gain = volume * if pan < 0.0 { 1.0 + pan } else { 1.0 };
            frame[1] = (f32::from(frame[1]) * right_gain)
                .round()
                .clamp(-32768.0, 32767.0) as i16;
        }
    }
    samples
}

#[cfg(any(windows, target_os = "android"))]
mod pcm_worker {
    use super::{
        AudioParameters, PreparedAudio, RuntimeError, WavWorkerCommand, apply_pcm_volume_pan,
    };
    use crate::audio_control::{AudioLoop, PcmCursor, PlaybackRepeats, SignalMeter};
    use crate::movie_audio::MovieAudioOutput;
    use crate::perf_detail::audio::{self as audio_profile, Stage};
    use std::sync::Arc;
    use std::sync::mpsc::{self, RecvTimeoutError};
    use std::time::Duration;

    fn activate_and_acknowledge(
        output: &mut dyn MovieAudioOutput,
        paused: bool,
        acknowledge_start_request: bool,
        started: &mpsc::SyncSender<cs2_runtime::RuntimeResult<()>>,
    ) -> cs2_runtime::RuntimeResult<bool> {
        let mut acknowledge = || started.send(Ok(())).is_ok();
        if acknowledge_start_request && !paused {
            output.start_with_acknowledgement(&mut acknowledge)
        } else {
            output.pause(paused)?;
            Ok(acknowledge())
        }
    }

    struct StartupPolicy {
        paused: bool,
        suspended: bool,
        deferred: std::collections::VecDeque<WavWorkerCommand>,
    }

    /// Open/prime may wait inside the driver. Honor lifecycle and cancellation
    /// that arrived during that work before any program PCM starts playing.
    fn activate_pending_output(
        output: &mut dyn MovieAudioOutput,
        mut paused: bool,
        mut suspended: bool,
        restore_hold: bool,
        acknowledge_start_request: bool,
        started: &mpsc::SyncSender<cs2_runtime::RuntimeResult<()>>,
        commands: &mpsc::Receiver<WavWorkerCommand>,
    ) -> cs2_runtime::RuntimeResult<Option<StartupPolicy>> {
        let mut deferred = std::collections::VecDeque::new();
        let mut acknowledgements = Vec::new();
        let mut stop = false;
        let mut stop_timing = None;
        loop {
            match commands.try_recv() {
                Ok(WavWorkerCommand::Stop(timing)) => { stop = true; stop_timing = timing; break; }
                Ok(WavWorkerCommand::Suspend(value, reply)) => { suspended = value; acknowledgements.push(reply); }
                Ok(WavWorkerCommand::Pause(value, reply, _)) => { paused = value; acknowledgements.push(reply); }
                Ok(other) => deferred.push_back(other),
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => { stop = true; break; }
            }
        }
        let result = if stop {
            audio_profile::with_worker(stop_timing, || output.stop()).map(|_| false)
        } else {
            activate_and_acknowledge(output, paused || suspended || restore_hold, acknowledge_start_request, started)
        };
        for reply in acknowledgements { let _ = reply.send(result.as_ref().map(|_| ()).map_err(clone_error)); }
        let acknowledged = result?;
        if stop { return Ok(None); }
        if !acknowledged { output.stop()?; return Ok(None); }
        Ok(Some(StartupPolicy { paused, suspended, deferred }))
    }

    pub(super) fn run_playback(
        prepared: Arc<PreparedAudio>,
        resource: &str,
        parameters: Arc<AudioParameters>,
        silence_millis: u32,
        loop_spec: AudioLoop,
        initial_suspended: bool,
        commands: mpsc::Receiver<WavWorkerCommand>,
        started: mpsc::SyncSender<cs2_runtime::RuntimeResult<()>>,
        saved: Option<super::checkpoint::WorkerState>,
        acknowledge_start_request: bool,
        #[cfg(target_os = "android")] standby: Option<crate::audio_android::Standby>,
    ) -> cs2_runtime::RuntimeResult<()> {
        let sample_rate = prepared.probe.sample_rate;
        let channels = prepared.probe.channels;
        if !matches!(channels, 1 | 2) {
            let error = RuntimeError::Host("unsupported PCM source channel count".into());
            let _ = started.send(Err(clone_error(&error)));
            return Err(error);
        }
        let decode_error = |frame, cause| {
            RuntimeError::Host(format!("decode SOUND resource {resource:?} at frame {frame}: {cause}"))
        };
        let mut source = {
            let _timing = Stage::Reader.begin();
            let position = saved.as_ref().map_or(0, |state| state.cursor.position);
            prepared.reader().map_err(|cause| decode_error(position, cause))?
        };
        let restoring = saved.is_some();
        let mut cursor = match saved.as_ref().map(|state| Ok(state.cursor.clone())).unwrap_or_else(|| PcmCursor::new(
            prepared.probe.frames as usize,
            loop_spec,
        )).and_then(|cursor| {
            cursor.end(&parameters)?;
            Ok(cursor)
        }) {
            Ok(cursor) => cursor,
            Err(error) => {
                let _ = started.send(Err(clone_error(&error)));
                return Err(error);
            }
        };
        let opened = {
            let _timing = Stage::Open.begin();
            let saved_queue = saved.as_ref().map(|s| &s.queue);
            #[cfg(target_os = "android")]
            let warmed = standby.and_then(|standby| {
                let _timing = Stage::Standby.begin();
                standby.claim()
            });
            #[cfg(target_os = "android")]
            let output = if let Some(warmed) = warmed {
                super::checkpoint::TrackedOutput::from_output(
                    sample_rate, channels, saved_queue, Box::new(warmed),
                )
            } else {
                super::checkpoint::TrackedOutput::open(sample_rate, channels, saved_queue)
            };
            #[cfg(windows)]
            let output = super::checkpoint::TrackedOutput::open(sample_rate, channels, saved_queue);
            output
        };
        let mut output = match opened {
            Ok(output) => output,
            Err(error) => {
                let _ = started.send(Err(clone_error(&error)));
                return Err(error);
            }
        };
        output.pause(true)?;
        let chunk_frames = (sample_rate as usize / 100).max(1);
        let mut silence = Vec::new();
        let mut signal = saved.as_ref().map_or_else(|| SignalMeter::new(sample_rate), |s| s.signal.clone());
        let mut pass_marked = saved.as_ref().is_some_and(|s| s.pass_marked);
        let initial_silence = saved.as_ref().map_or(u64::from(sample_rate) * u64::from(silence_millis) / 1000, |s| s.initial_silence);
        let mut silent_frames = saved.as_ref().map_or(initial_silence, |s| s.silent_frames);
        // Prime a bounded 80 ms queue before starting the hardware clock.
        let priming = Stage::Prime.begin();
        while !restoring && (cursor.position < cursor.end(&parameters)? || silent_frames != 0) {
            if silent_frames != 0 {
                let frames = silent_frames.min(chunk_frames as u64) as usize;
                silence.clear();
                silence.resize(frames * usize::from(channels), 0);
                if !output.submit(&silence)? {
                    break;
                }
                silent_frames -= frames as u64;
                signal.submitted(&silence, channels, &parameters);
                continue;
            }
            let end = (cursor.position + chunk_frames).min(cursor.end(&parameters)?);
            let samples = source.read(cursor.position, end)
                .map_err(|cause| decode_error(cursor.position, cause))?;
            let (volume, pan) = parameters.values();
            let chunk = apply_pcm_volume_pan(samples.to_vec(), channels, volume, pan);
            if !output.submit(&chunk)? {
                break;
            }
            cursor.position = end;
            signal.submitted(samples, channels, &parameters);
        }
        drop(priming);
        let paused = saved.as_ref().is_some_and(|s| s.paused);
        let mut restore_hold = restoring;
        let mut capture_hold = false;
        let Some(policy) = activate_pending_output(
            &mut output, paused, initial_suspended, restore_hold,
            acknowledge_start_request, &started, &commands,
        )? else { return Ok(()); };
        let mut paused = policy.paused;
        let mut suspended = policy.suspended;
        let mut deferred = policy.deferred;
        audio_profile::clear();
        let mut repeats = saved.map_or_else(|| PlaybackRepeats::new(&parameters), |s| s.repeats);
        loop {
            if !restore_hold && !capture_hold { repeats.refresh(&parameters); }
            match deferred.pop_front().map(Ok).unwrap_or_else(|| commands.recv_timeout(Duration::from_millis(2))) {
                Ok(WavWorkerCommand::Stop(timing)) => {
                    return audio_profile::with_worker(timing, || output.stop());
                }
                Ok(WavWorkerCommand::Pause(value, acknowledge, timing)) => {
                    let result = audio_profile::with_worker(timing, || {
                        output.pause(value || suspended || restore_hold || capture_hold)
                    });
                    let _ = acknowledge.send(result.as_ref().map(|_| ()).map_err(clone_error));
                    result?;
                    paused = value;
                }
                Ok(WavWorkerCommand::Suspend(value, acknowledge)) => {
                    let result = output.pause(paused || value || restore_hold || capture_hold);
                    let _ = acknowledge.send(result.as_ref().map(|_| ()).map_err(clone_error));
                    result?;
                    suspended = value;
                }
                Ok(WavWorkerCommand::Capture(acknowledge)) => {
                    capture_hold = true;
                    let result = (|| -> cs2_runtime::RuntimeResult<super::checkpoint::WorkerState> {
                        output.pause(true)?;
                        let queue = output.capture()?;
                        signal.consumed_frames(queue.consumed_frames(), &parameters)?;
                        Ok(super::checkpoint::WorkerState { cursor: cursor.clone(), repeats: repeats.clone(),
                            signal: signal.clone(), parameters: parameters.checkpoint(), loop_spec,
                            initial_silence, silent_frames, pass_marked, paused, queue })
                    })();
                    let _ = acknowledge.send(result);
                }
                Ok(WavWorkerCommand::ReleaseCapture) => {
                    capture_hold = false;
                    output.pause(paused || suspended || restore_hold)?;
                }
                Ok(WavWorkerCommand::CommitRestore) => {
                    restore_hold = false;
                    output.pause(paused || suspended || capture_hold)?;
                }
                Err(RecvTimeoutError::Disconnected) => return output.stop(),
                Err(RecvTimeoutError::Timeout) => {}
            }
            if capture_hold || restore_hold { continue; }
            if paused || suspended {
                // Device disconnection must still surface while backgrounded;
                // this query never advances the source cursor.
                signal.consumed(output.position_seconds()?, &parameters)?;
                continue;
            }
            signal.consumed(output.position_seconds()?, &parameters)?;
            loop {
                // Updates can arrive while filling the bounded device queue,
                // including while an earlier finite policy is draining.
                repeats.refresh(&parameters);
                if silent_frames != 0 {
                    let frames = silent_frames.min(chunk_frames as u64) as usize;
                    silence.clear();
                    silence.resize(frames * usize::from(channels), 0);
                    if !output.submit(&silence)? {
                        break;
                    }
                    silent_frames -= frames as u64;
                    signal.submitted(&silence, channels, &parameters);
                    continue;
                }
                if cursor.position == cursor.end(&parameters)? {
                    if loop_spec.whole && !pass_marked {
                        signal.source_pass_end(&parameters)?;
                        pass_marked = true;
                    }
                    if repeats.repeat_at_end() {
                        cursor.rewind();
                        pass_marked = false;
                        if loop_spec.whole {
                            silent_frames = initial_silence;
                        }
                        continue;
                    } else {
                        break;
                    }
                }
                let end = (cursor.position + chunk_frames).min(cursor.end(&parameters)?);
                let samples = source.read(cursor.position, end)
                    .map_err(|cause| decode_error(cursor.position, cause))?;
                let (volume, pan) = parameters.values();
                let chunk = apply_pcm_volume_pan(samples.to_vec(), channels, volume, pan);
                if !output.submit(&chunk)? {
                    break;
                }
                cursor.position = end;
                signal.submitted(samples, channels, &parameters);
            }
            if repeats.exhausted()
                && cursor.position == cursor.end(&parameters)?
                && output.drained()?
            {
                // Recheck after polling the device. Claiming completion with
                // CAS prevents closing over a newly accepted repeat policy.
                if repeats.refresh(&parameters) {
                    continue;
                }
                if !repeats.claim_completion(&parameters) {
                    continue;
                }
                signal.finish(&parameters);
                signal.consumed(output.position_seconds()?, &parameters)?;
                return output.stop();
            }
        }
    }

    pub(super) fn clone_error(error: &RuntimeError) -> RuntimeError {
        match error {
            RuntimeError::AudioStartTimeout(cause) => RuntimeError::AudioStartTimeout(cause.clone()),
            _ => RuntimeError::Host(error.to_string()),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use cs2_runtime::RuntimeResult;

        struct TestOutput {
            paused: Option<bool>,
            startup: mpsc::Receiver<RuntimeResult<()>>,
            reject_request: bool,
        }

        impl MovieAudioOutput for TestOutput {
            fn submit(&mut self, _: &[i16]) -> RuntimeResult<bool> { unreachable!() }
            fn position_seconds(&mut self) -> RuntimeResult<f64> { unreachable!() }
            fn drained(&mut self) -> RuntimeResult<bool> { unreachable!() }
            fn stop(&mut self) -> RuntimeResult<()> { unreachable!() }

            fn pause(&mut self, paused: bool) -> RuntimeResult<()> {
                assert!(matches!(self.startup.try_recv(), Err(mpsc::TryRecvError::Empty)));
                self.paused = Some(paused);
                Ok(())
            }

            fn start_with_acknowledgement(&mut self, acknowledge: &mut dyn FnMut() -> bool) -> RuntimeResult<bool> {
                if self.reject_request {
                    return Err(RuntimeError::Host("request rejected".into()));
                }
                assert!(acknowledge());
                self.startup.try_recv().expect("acknowledge before confirming device")?;
                Err(RuntimeError::AudioStartTimeout("device never confirmed".into()))
            }
        }

        struct StartupOutput { pause_calls: Vec<bool>, stopped: bool }
        impl MovieAudioOutput for StartupOutput {
            fn submit(&mut self, _: &[i16]) -> RuntimeResult<bool> { unreachable!() }
            fn position_seconds(&mut self) -> RuntimeResult<f64> { unreachable!() }
            fn drained(&mut self) -> RuntimeResult<bool> { unreachable!() }
            fn pause(&mut self, paused: bool) -> RuntimeResult<()> { self.pause_calls.push(paused); Ok(()) }
            fn stop(&mut self) -> RuntimeResult<()> { self.stopped = true; Ok(()) }
        }

        #[test]
        fn startup_cancellation_queued_while_opening_never_activates_device() {
            let mut output = StartupOutput { pause_calls: Vec::new(), stopped: false };
            let (commands, pending) = mpsc::channel();
            commands.send(WavWorkerCommand::Stop(None)).unwrap();
            let (started, startup) = mpsc::sync_channel(1);
            assert!(activate_pending_output(&mut output, false, false, false, false, &started, &pending).unwrap().is_none());
            assert!(output.stopped);
            assert!(output.pause_calls.is_empty());
            assert!(startup.try_recv().is_err());
        }

        #[test]
        fn lifecycle_suspend_queued_while_opening_keeps_first_activation_paused() {
            let mut output = StartupOutput { pause_calls: Vec::new(), stopped: false };
            let (commands, pending) = mpsc::channel();
            let (reply, acknowledged) = mpsc::sync_channel(1);
            commands.send(WavWorkerCommand::Suspend(true, reply)).unwrap();
            let (started, startup) = mpsc::sync_channel(1);
            let policy = activate_pending_output(&mut output, false, false, false, false, &started, &pending).unwrap().unwrap();
            assert!(policy.suspended);
            assert_eq!(output.pause_calls, [true]);
            assert!(startup.try_recv().unwrap().is_ok());
            assert!(acknowledged.try_recv().unwrap().is_ok());
        }

        #[test]
        fn startup_drain_preserves_restore_commands_for_the_worker_loop() {
            let mut output = StartupOutput { pause_calls: Vec::new(), stopped: false };
            let (commands, pending) = mpsc::channel();
            commands.send(WavWorkerCommand::CommitRestore).unwrap();
            let (started, _startup) = mpsc::sync_channel(1);
            let policy = activate_pending_output(&mut output, false, false, true, false, &started, &pending).unwrap().unwrap();
            assert_eq!(output.pause_calls, [true]);
            assert!(matches!(policy.deferred.front(), Some(WavWorkerCommand::CommitRestore)));
        }

        #[test]
        fn startup_acknowledges_accepted_request_and_keeps_device_error() {
            for reject_request in [false, true] {
                let (started, startup) = mpsc::sync_channel(1);
                let mut output = TestOutput { paused: None, startup, reject_request };
                let result = activate_and_acknowledge(&mut output, false, true, &started);
                if reject_request {
                    assert!(matches!(result, Err(RuntimeError::Host(cause)) if cause == "request rejected"));
                    assert!(matches!(output.startup.try_recv(), Err(mpsc::TryRecvError::Empty)));
                } else {
                    assert!(matches!(result, Err(RuntimeError::AudioStartTimeout(cause)) if cause == "device never confirmed"));
                }
                assert_eq!(output.paused, None);
            }
        }

        #[test]
        fn startup_preserves_suspended_and_synchronous_paths() {
            for (paused, early_ack) in [(true, true), (false, false)] {
                let (started, startup) = mpsc::sync_channel(1);
                let mut output = TestOutput { paused: None, startup, reject_request: false };
                assert!(activate_and_acknowledge(&mut output, paused, early_ack, &started).unwrap());
                output.startup.try_recv().unwrap().unwrap();
                assert_eq!(output.paused, Some(paused));
            }
        }
    }
}

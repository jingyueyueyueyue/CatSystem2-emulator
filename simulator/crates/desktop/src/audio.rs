use super::read_entry_with_fallback;
use crate::audio_control::{self, AudioParameters};
use crate::audio_decode::{AudioCodec, AudioProbe, PreparedAudio, prepare_audio};
use crate::perf_detail::audio::{self as audio_profile, Stage};
use cs2_runtime::{
    AudioChannel, AudioHost, AudioRequest, PlaybackState, ResourceIndex, RuntimeError,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

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
        let handles = self.handles.keys().copied().collect::<Vec<_>>();
        let mut first_error = None;
        for handle in handles {
            if let Err(error) = self.is_active(handle) {
                first_error.get_or_insert(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    fn play_backend(
        &mut self,
        request: AudioRequest,
        prepared: Arc<PreparedAudio>,
        silence_millis: u32,
        play_count: u32,
        signal_enabled: bool,
    ) -> cs2_runtime::RuntimeResult<PlaybackState> {
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

/// PCM output for RIFF/WAVE and Ogg/Vorbis, using bounded WinMM/AAudio buffers.
/// The historical `WavAudioHost` name remains available to existing callers.
pub struct WavAudioHost {
    resolve: Arc<dyn Fn(&str) -> cs2_runtime::RuntimeResult<Vec<u8>> + Send + Sync + 'static>,
    active: HashMap<u64, ActiveWavPlayback>,
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
        for playback in self.active.values() {
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

    fn reap_finished(&mut self) -> cs2_runtime::RuntimeResult<()> {
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
        self.reap_finished()?;
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
            let worker = {
                let _timing = Stage::Spawn.begin();
                thread::Builder::new()
                    .name(format!("cs2-pcm-{handle}"))
                    // Keep a control owner even if a very short worker finishes
                    // before its handle is returned to the script thread.
                    .spawn({
                        let parameters = parameters.clone();
                        move || {
                            let startup_error = started_sender.clone();
                            let result = audio_profile::with_worker(worker_timing, || {
                                pcm_worker::run_playback(
                                    prepared,
                                    parameters,
                                    silence_millis,
                                    loop_spec,
                                    initial_suspended,
                                    command_receiver,
                                    started_sender,
                                    None,
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
            let ready = {
                let _timing = Stage::Ack.begin();
                started_receiver.recv_timeout(Duration::from_secs(10))
            };
            match ready {
                Ok(Ok(())) => {
                    #[cfg(target_os = "android")]
                    self.warm_device_for(retained_prepared.probe);
                    self.active.insert(
                        handle,
                        ActiveWavPlayback {
                            channel: request.channel.clone(),
                            paused: false,
                            commands,
                            finished,
                            thread: Some(worker),
                            _parameters: parameters,
                            resource: request.resource.clone(),
                            prepared: retained_prepared,
                        },
                    );
                    Ok(PlaybackState {
                        handle,
                        resource: request.resource,
                        looped: request.looped,
                        paused: false,
                    })
                }
                Ok(Err(error)) => {
                    let _ = worker.join();
                    Err(error)
                }
                Err(_) => {
                    drop(commands);
                    drop(started_receiver);
                    let _ = worker.join();
                    Err(RuntimeError::Host(
                        "WAV worker did not acknowledge opening the device".to_string(),
                    ))
                }
            }
        }
    }
}

impl Drop for WavAudioHost {
    fn drop(&mut self) {
        let handles = self.active.keys().copied().collect::<Vec<_>>();
        for handle in handles {
            let _ = self.stop_playback(handle);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    pub(super) fn run_playback(
        prepared: Arc<PreparedAudio>,
        parameters: Arc<AudioParameters>,
        silence_millis: u32,
        loop_spec: AudioLoop,
        initial_suspended: bool,
        commands: mpsc::Receiver<WavWorkerCommand>,
        started: mpsc::SyncSender<cs2_runtime::RuntimeResult<()>>,
        saved: Option<super::checkpoint::WorkerState>,
        #[cfg(target_os = "android")] standby: Option<crate::audio_android::Standby>,
    ) -> cs2_runtime::RuntimeResult<()> {
        let sample_rate = prepared.probe.sample_rate;
        let channels = prepared.probe.channels;
        if !matches!(channels, 1 | 2) {
            let error = RuntimeError::Host("unsupported PCM source channel count".into());
            let _ = started.send(Err(clone_error(&error)));
            return Err(error);
        }
        let mut source = {
            let _timing = Stage::Reader.begin();
            prepared.reader()?
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
        let mut signal = saved.as_ref().map_or_else(|| SignalMeter::new(sample_rate), |s| s.signal.clone());
        let mut pass_marked = saved.as_ref().is_some_and(|s| s.pass_marked);
        let initial_silence = saved.as_ref().map_or(u64::from(sample_rate) * u64::from(silence_millis) / 1000, |s| s.initial_silence);
        let mut silent_frames = saved.as_ref().map_or(initial_silence, |s| s.silent_frames);
        // Prime a bounded 80 ms queue before starting the hardware clock.
        let priming = Stage::Prime.begin();
        while !restoring && (cursor.position < cursor.end(&parameters)? || silent_frames != 0) {
            if silent_frames != 0 {
                let frames = silent_frames.min(chunk_frames as u64) as usize;
                if !output.submit(&vec![0; frames * usize::from(channels)])? {
                    break;
                }
                silent_frames -= frames as u64;
                signal.submitted(
                    &vec![0; frames * usize::from(channels)],
                    channels,
                    &parameters,
                );
                continue;
            }
            let end = (cursor.position + chunk_frames).min(cursor.end(&parameters)?);
            let samples = source.read(cursor.position, end)?;
            let (volume, pan) = parameters.values();
            let chunk = apply_pcm_volume_pan(samples.to_vec(), channels, volume, pan);
            if !output.submit(&chunk)? {
                break;
            }
            cursor.position = end;
            signal.submitted(samples, channels, &parameters);
        }
        drop(priming);
        let mut paused = saved.as_ref().is_some_and(|s| s.paused);
        let mut restore_hold = restoring;
        let mut capture_hold = false;
        output.pause(initial_suspended || paused || restore_hold)?;
        audio_profile::clear();
        if started.send(Ok(())).is_err() {
            return output.stop();
        }
        let mut suspended = initial_suspended;
        let mut repeats = saved.map_or_else(|| PlaybackRepeats::new(&parameters), |s| s.repeats);
        loop {
            if !restore_hold && !capture_hold { repeats.refresh(&parameters); }
            match commands.recv_timeout(Duration::from_millis(2)) {
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
                    if !output.submit(&vec![0; frames * usize::from(channels)])? {
                        break;
                    }
                    silent_frames -= frames as u64;
                    signal.submitted(
                        &vec![0; frames * usize::from(channels)],
                        channels,
                        &parameters,
                    );
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
                let samples = source.read(cursor.position, end)?;
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
}

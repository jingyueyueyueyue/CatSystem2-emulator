use crate::{AudioChannel, AudioHost, AudioRequest, PlaybackState, RuntimeError, RuntimeResult};
use cs2_formats::{AudioMetadata, OggVorbis};
use std::{
    collections::HashMap,
    sync::{Arc, mpsc},
    thread::{self, JoinHandle},
    time::Duration,
};

type ResourceResolver = dyn Fn(&str) -> RuntimeResult<Vec<u8>> + Send + Sync + 'static;

/// Ogg/Vorbis `AudioHost`; Windows builds output PCM through WinMM waveOut.
///
/// The resolver should read the requested resource from the game's directory or
/// archive. Decoding and device I/O run on one worker thread per playback.
pub struct OggAudioHost {
    resolve: Arc<ResourceResolver>,
    next_handle: u64,
    active: HashMap<u64, ActivePlayback>,
}

struct ActivePlayback {
    channel: AudioChannel,
    commands: mpsc::Sender<WorkerCommand>,
    finished: mpsc::Receiver<RuntimeResult<()>>,
    thread: Option<JoinHandle<()>>,
}

enum WorkerCommand {
    Pause(bool, mpsc::SyncSender<RuntimeResult<()>>),
    Stop(mpsc::SyncSender<RuntimeResult<()>>),
}

impl OggAudioHost {
    pub fn new(resolve: impl Fn(&str) -> RuntimeResult<Vec<u8>> + Send + Sync + 'static) -> Self {
        Self {
            resolve: Arc::new(resolve),
            next_handle: 1,
            active: HashMap::new(),
        }
    }

    pub fn is_active(&mut self, handle: u64) -> RuntimeResult<bool> {
        self.reap_finished()?;
        Ok(self.active.contains_key(&handle))
    }

    fn reap_finished(&mut self) -> RuntimeResult<()> {
        let finished: Vec<_> = self
            .active
            .iter()
            .filter_map(|(&handle, playback)| match playback.finished.try_recv() {
                Ok(result) => Some((handle, result)),
                Err(mpsc::TryRecvError::Disconnected) => Some((
                    handle,
                    Err(RuntimeError::Host(
                        "audio worker exited without a result".to_string(),
                    )),
                )),
                Err(mpsc::TryRecvError::Empty) => None,
            })
            .collect();
        let mut first_error = None;
        for (handle, result) in finished {
            if let Some(mut playback) = self.active.remove(&handle) {
                join_worker(&mut playback.thread);
            }
            if let Err(error) = result {
                first_error.get_or_insert(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    fn stop_playback(&mut self, handle: u64) -> RuntimeResult<()> {
        let Some(mut playback) = self.active.remove(&handle) else {
            return Ok(());
        };
        if let Ok(result) = playback.finished.try_recv() {
            join_worker(&mut playback.thread);
            return result;
        }
        let (acknowledge, response) = mpsc::sync_channel(1);
        if playback
            .commands
            .send(WorkerCommand::Stop(acknowledge))
            .is_err()
        {
            join_worker(&mut playback.thread);
            return playback.finished.recv().unwrap_or_else(|_| {
                Err(RuntimeError::Host(
                    "audio worker stopped unexpectedly".to_string(),
                ))
            });
        }
        let device_result = response.recv().unwrap_or_else(|_| {
            Err(RuntimeError::Host(
                "audio worker dropped stop response".to_string(),
            ))
        });
        join_worker(&mut playback.thread);
        let worker_result = playback.finished.recv().unwrap_or_else(|_| {
            Err(RuntimeError::Host(
                "audio worker dropped result".to_string(),
            ))
        });
        device_result.and(worker_result)
    }
}

impl AudioHost for OggAudioHost {
    fn play(&mut self, request: AudioRequest) -> RuntimeResult<PlaybackState> {
        self.reap_finished()?;
        if !request.volume.is_finite() || !request.pan.is_finite() {
            return Err(RuntimeError::Host(
                "audio volume and pan must be finite".to_string(),
            ));
        }
        let encoded = (self.resolve)(&request.resource)?;
        let decoded = OggVorbis::decode_i16(&encoded)
            .map_err(|error| RuntimeError::Host(format!("decode {}: {error}", request.resource)))?;
        validate_audio(decoded.metadata, decoded.frames, decoded.samples.len())?;
        let samples = apply_volume_pan(
            decoded.samples,
            decoded.metadata,
            request.volume,
            request.pan,
        );

        #[cfg(not(windows))]
        {
            let _ = samples;
            Err(RuntimeError::Host(
                "WinMM audio output is only available on Windows".to_string(),
            ))
        }

        #[cfg(windows)]
        {
            let handle = self.next_handle;
            if handle == 0 || self.active.contains_key(&handle) {
                return Err(RuntimeError::Host(
                    "audio playback handle space is exhausted".to_string(),
                ));
            }
            self.next_handle = handle.checked_add(1).unwrap_or(0);
            let (commands, command_receiver) = mpsc::channel();
            let (started_sender, started_receiver) = mpsc::sync_channel(1);
            let (finished_sender, finished) = mpsc::channel();
            let metadata = decoded.metadata;
            let looped = request.looped;
            let worker = thread::Builder::new()
                .name(format!("cs2-audio-{handle}"))
                .spawn(move || {
                    let result = winmm::run_playback(
                        metadata,
                        samples,
                        looped,
                        command_receiver,
                        started_sender,
                    );
                    let _ = finished_sender.send(result);
                })
                .map_err(|error| RuntimeError::Host(format!("start audio worker: {error}")))?;

            match started_receiver.recv() {
                Ok(Ok(())) => {
                    self.active.insert(
                        handle,
                        ActivePlayback {
                            channel: request.channel.clone(),
                            commands,
                            finished,
                            thread: Some(worker),
                        },
                    );
                    Ok(PlaybackState {
                        handle,
                        resource: request.resource,
                        looped,
                        paused: false,
                    })
                }
                Ok(Err(error)) => {
                    let _ = worker.join();
                    Err(error)
                }
                Err(_) => {
                    let _ = worker.join();
                    Err(RuntimeError::Host(
                        "audio worker exited before opening the device".to_string(),
                    ))
                }
            }
        }
    }

    fn stop(&mut self, handle: u64) -> RuntimeResult<()> {
        self.stop_playback(handle)
    }

    fn pause(&mut self, handle: u64, paused: bool) -> RuntimeResult<()> {
        self.reap_finished()?;
        let playback = self.active.get(&handle).ok_or_else(|| {
            RuntimeError::Host(format!("audio playback handle {handle} is not active"))
        })?;
        let (acknowledge, response) = mpsc::sync_channel(1);
        playback
            .commands
            .send(WorkerCommand::Pause(paused, acknowledge))
            .map_err(|_| RuntimeError::Host("audio worker stopped unexpectedly".to_string()))?;
        response
            .recv()
            .map_err(|_| RuntimeError::Host("audio worker dropped pause response".to_string()))?
    }

    fn stop_all(&mut self, channel: AudioChannel) -> RuntimeResult<()> {
        let handles: Vec<_> = self
            .active
            .iter()
            .filter_map(|(&handle, playback)| (playback.channel == channel).then_some(handle))
            .collect();
        let mut first_error = None;
        for handle in handles {
            if let Err(error) = self.stop_playback(handle) {
                first_error.get_or_insert(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    fn is_active(&mut self, handle: u64) -> RuntimeResult<bool> {
        OggAudioHost::is_active(self, handle)
    }
}

impl Drop for OggAudioHost {
    fn drop(&mut self) {
        let handles: Vec<_> = self.active.keys().copied().collect();
        for handle in handles {
            let _ = self.stop_playback(handle);
        }
    }
}

fn join_worker(worker: &mut Option<JoinHandle<()>>) {
    if let Some(worker) = worker.take() {
        let _ = worker.join();
    }
}

fn validate_audio(metadata: AudioMetadata, frames: usize, samples: usize) -> RuntimeResult<()> {
    if metadata.sample_rate == 0 || !matches!(metadata.channels, 1 | 2) {
        return Err(RuntimeError::Host(format!(
            "unsupported PCM format: {} Hz, {} channel(s)",
            metadata.sample_rate, metadata.channels
        )));
    }
    if frames == 0 || samples != frames.saturating_mul(usize::from(metadata.channels)) {
        return Err(RuntimeError::Host(
            "decoded PCM buffer is empty or inconsistent".to_string(),
        ));
    }
    Ok(())
}

fn apply_volume_pan(
    mut samples: Vec<i16>,
    metadata: AudioMetadata,
    volume: f32,
    pan: f32,
) -> Vec<i16> {
    let volume = volume.clamp(0.0, 1.0);
    let pan = pan.clamp(-1.0, 1.0);
    for frame in samples.chunks_exact_mut(usize::from(metadata.channels)) {
        let left_gain = volume
            * if metadata.channels == 2 && pan > 0.0 {
                1.0 - pan
            } else {
                1.0
            };
        frame[0] = scale_sample(frame[0], left_gain);
        if metadata.channels == 2 {
            let right_gain = volume * if pan < 0.0 { 1.0 + pan } else { 1.0 };
            frame[1] = scale_sample(frame[1], right_gain);
        }
    }
    samples
}

fn scale_sample(sample: i16, gain: f32) -> i16 {
    (f32::from(sample) * gain).round().clamp(-32768.0, 32767.0) as i16
}

fn host_error(message: impl std::fmt::Display) -> RuntimeError {
    RuntimeError::Host(message.to_string())
}

#[cfg(windows)]
mod winmm {
    use super::{Duration, RuntimeError, RuntimeResult, WorkerCommand, host_error};
    use cs2_formats::AudioMetadata;
    use std::{
        mem::size_of,
        ptr,
        sync::{mpsc, mpsc::RecvTimeoutError},
        thread,
    };
    use windows_sys::Win32::Media::Audio::{
        HWAVEOUT, WAVE_FORMAT_PCM, WAVE_MAPPER, WAVEFORMATEX, WAVEHDR, WHDR_BEGINLOOP, WHDR_DONE,
        WHDR_ENDLOOP, waveOutClose, waveOutOpen, waveOutPause, waveOutPrepareHeader, waveOutReset,
        waveOutRestart, waveOutUnprepareHeader, waveOutWrite,
    };

    pub(super) fn run_playback(
        metadata: AudioMetadata,
        samples: Vec<i16>,
        looped: bool,
        commands: mpsc::Receiver<WorkerCommand>,
        started: mpsc::SyncSender<RuntimeResult<()>>,
    ) -> RuntimeResult<()> {
        let mut format = WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_PCM as u16,
            nChannels: u16::from(metadata.channels),
            nSamplesPerSec: metadata.sample_rate,
            nAvgBytesPerSec: metadata.sample_rate * u32::from(metadata.channels) * 2,
            nBlockAlign: u16::from(metadata.channels) * 2,
            wBitsPerSample: 16,
            cbSize: 0,
        };
        let mut output: HWAVEOUT = 0;
        let open = unsafe { waveOutOpen(&mut output, WAVE_MAPPER, &mut format, 0, 0, 0) };
        if open != 0 {
            let error = host_error(format!("waveOutOpen failed with MMRESULT {open}"));
            let _ = started.send(Err(host_error(error.to_string())));
            return Err(error);
        }

        let mut pcm = samples
            .into_iter()
            .flat_map(i16::to_le_bytes)
            .collect::<Vec<_>>();
        let byte_length = u32::try_from(pcm.len())
            .map_err(|_| host_error("decoded audio exceeds WinMM buffer size"));
        let byte_length = match byte_length {
            Ok(length) => length,
            Err(error) => {
                unsafe {
                    waveOutClose(output);
                }
                let _ = started.send(Err(host_error(error.to_string())));
                return Err(error);
            }
        };
        let mut header = WAVEHDR {
            lpData: pcm.as_mut_ptr().cast(),
            dwBufferLength: byte_length,
            dwBytesRecorded: 0,
            dwUser: 0,
            dwFlags: 0,
            dwLoops: if looped { u32::MAX } else { 0 },
            lpNext: ptr::null_mut(),
            reserved: 0,
        };

        let prepare =
            unsafe { waveOutPrepareHeader(output, &mut header, size_of::<WAVEHDR>() as u32) };
        if prepare != 0 {
            let error = host_error(format!(
                "waveOutPrepareHeader failed with MMRESULT {prepare}"
            ));
            unsafe {
                waveOutClose(output);
            }
            let _ = started.send(Err(host_error(error.to_string())));
            return Err(error);
        }
        if looped {
            header.dwFlags |= WHDR_BEGINLOOP | WHDR_ENDLOOP;
        }
        let write = unsafe { waveOutWrite(output, &mut header, size_of::<WAVEHDR>() as u32) };
        if write != 0 {
            let error = host_error(format!("waveOutWrite failed with MMRESULT {write}"));
            unsafe {
                waveOutUnprepareHeader(output, &mut header, size_of::<WAVEHDR>() as u32);
                waveOutClose(output);
            }
            let _ = started.send(Err(host_error(error.to_string())));
            return Err(error);
        }
        if started.send(Ok(())).is_err() {
            unsafe {
                waveOutReset(output);
                waveOutUnprepareHeader(output, &mut header, size_of::<WAVEHDR>() as u32);
                waveOutClose(output);
            }
            return Ok(());
        }

        let result = loop {
            if header.dwFlags & WHDR_DONE != 0 {
                break Ok(());
            }
            match commands.recv_timeout(Duration::from_millis(8)) {
                Ok(WorkerCommand::Pause(paused, acknowledge)) => {
                    let code = unsafe {
                        if paused {
                            waveOutPause(output)
                        } else {
                            waveOutRestart(output)
                        }
                    };
                    let result = mmresult("pause/restart", code);
                    match result {
                        Ok(()) => {
                            let _ = acknowledge.send(Ok(()));
                        }
                        Err(error) => {
                            let _ = acknowledge.send(Err(host_error(error.to_string())));
                            break Err(error);
                        }
                    }
                }
                Ok(WorkerCommand::Stop(acknowledge)) => {
                    let reset = unsafe { waveOutReset(output) };
                    let result = mmresult("reset", reset);
                    let cleanup = cleanup(output, &mut header);
                    let final_result = result.and(cleanup);
                    match final_result {
                        Ok(()) => {
                            let _ = acknowledge.send(Ok(()));
                            return Ok(());
                        }
                        Err(error) => {
                            let _ = acknowledge.send(Err(host_error(error.to_string())));
                            return Err(error);
                        }
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break Ok(()),
            }
            thread::yield_now();
        };

        let cleanup = cleanup(output, &mut header);
        result.and(cleanup)
    }

    fn cleanup(output: HWAVEOUT, header: &mut WAVEHDR) -> RuntimeResult<()> {
        let reset = unsafe { waveOutReset(output) };
        let unprepare =
            unsafe { waveOutUnprepareHeader(output, header, size_of::<WAVEHDR>() as u32) };
        let close = unsafe { waveOutClose(output) };
        mmresult("reset", reset)?;
        mmresult("unprepare header", unprepare)?;
        mmresult("close", close)
    }

    fn mmresult(operation: &str, code: u32) -> RuntimeResult<()> {
        if code == 0 {
            Ok(())
        } else {
            Err(RuntimeError::Host(format!(
                "waveOut {operation} failed with MMRESULT {code}"
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_and_pan_are_clamped_and_applied_per_channel() {
        let stereo = AudioMetadata {
            sample_rate: 48_000,
            channels: 2,
        };
        assert_eq!(
            apply_volume_pan(vec![10_000, -10_000, 20_000, -20_000], stereo, 0.5, 1.0),
            [0, -5_000, 0, -10_000]
        );
        assert_eq!(
            apply_volume_pan(vec![10_000, -10_000], stereo, 4.0, -4.0),
            [10_000, 0]
        );
    }

    #[test]
    fn mono_volume_is_applied_without_pan_expansion() {
        let mono = AudioMetadata {
            sample_rate: 22_050,
            channels: 1,
        };
        assert_eq!(
            apply_volume_pan(vec![i16::MIN, 12_000], mono, 0.5, 0.8),
            [-16_384, 6_000]
        );
    }

    #[test]
    fn rejects_empty_inconsistent_and_unsupported_pcm() {
        assert!(
            validate_audio(
                AudioMetadata {
                    sample_rate: 0,
                    channels: 2
                },
                2,
                4
            )
            .is_err()
        );
        assert!(
            validate_audio(
                AudioMetadata {
                    sample_rate: 44_100,
                    channels: 6
                },
                2,
                12
            )
            .is_err()
        );
        assert!(
            validate_audio(
                AudioMetadata {
                    sample_rate: 44_100,
                    channels: 1
                },
                0,
                0
            )
            .is_err()
        );
    }
}

//! Streaming movie PCM output. The portable movie scheduler owns this interface.
use cs2_runtime::RuntimeResult;

pub trait MovieAudioOutput: Send {
    /// Interleaved PCM16 in the rate/channel layout selected by open_pcm.
    /// MovieAudioFactory selects stereo at 48 kHz. False applies backpressure
    /// and means none of this call's samples were accepted.
    fn submit(&mut self, samples: &[i16]) -> RuntimeResult<bool>;
    /// Seconds actually consumed by the output device, excluding pauses.
    fn position_seconds(&mut self) -> RuntimeResult<f64>;
    /// Exact consumed PCM frames for physical SOUND checkpoints. A provider
    /// with only a millisecond clock must reject instead of guessing a seek.
    fn checkpoint_position_frames(&mut self) -> RuntimeResult<u64> {
        Err(super::media_process::error("audio output has no exact checkpoint frame counter"))
    }
    fn drained(&mut self) -> RuntimeResult<bool>;
    fn pause(&mut self, paused: bool) -> RuntimeResult<()>;
    /// A worker may acknowledge a start once its primed device accepts the
    /// request. Device confirmation and any subsequent error stay with the
    /// worker. False means the caller is gone and the worker can stop.
    fn start_with_acknowledgement(&mut self, acknowledge: &mut dyn FnMut() -> bool) -> RuntimeResult<bool> {
        self.pause(false)?;
        Ok(acknowledge())
    }
    fn stop(&mut self) -> RuntimeResult<()>;
}

pub trait MovieAudioFactory: Send + Sync {
    fn open(&self) -> RuntimeResult<Box<dyn MovieAudioOutput>>;
}

pub(crate) struct SystemMovieAudio;

#[cfg(windows)]
pub(crate) fn open_pcm(sample_rate: u32, channels: u8) -> RuntimeResult<Box<dyn MovieAudioOutput>> {
    Ok(Box::new(winmm::Output::open_pcm(sample_rate, channels)?))
}

#[cfg(target_os = "android")]
pub(crate) fn open_pcm(sample_rate: u32, channels: u8) -> RuntimeResult<Box<dyn MovieAudioOutput>> {
    Ok(Box::new(crate::audio_android::Output::open_pcm(
        sample_rate,
        channels,
    )?))
}

impl MovieAudioFactory for SystemMovieAudio {
    fn open(&self) -> RuntimeResult<Box<dyn MovieAudioOutput>> {
        #[cfg(any(windows, target_os = "android"))]
        {
            open_pcm(48_000, 2)
        }
        #[cfg(not(any(windows, target_os = "android")))]
        {
            Err(super::media_process::error(
                "movie audio requires a MovieAudioFactory on this platform",
            ))
        }
    }
}

#[cfg(windows)]
mod winmm {
    use super::MovieAudioOutput;
    use crate::media_process::error;
    use cs2_runtime::RuntimeResult;
    use std::collections::VecDeque;
    use std::mem::size_of;
    use windows_sys::Win32::Media::Audio::{
        HWAVEOUT, WAVE_FORMAT_PCM, WAVE_MAPPER, WAVEFORMATEX, WAVEHDR, WHDR_DONE, waveOutClose,
        waveOutOpen, waveOutPause, waveOutPrepareHeader, waveOutReset, waveOutRestart,
        waveOutUnprepareHeader, waveOutWrite,
    };

    // MMTIME is a WinMM ABI record (DWORD type + 8-byte union). Keeping this
    // declaration local avoids making a platform record part of the core API.
    #[repr(C)]
    struct Position {
        kind: u32,
        value: [u32; 2],
    }
    #[link(name = "winmm")]
    unsafe extern "system" {
        fn waveOutGetPosition(output: HWAVEOUT, position: *mut Position, size: u32) -> u32;
    }

    struct Buffer {
        header: Box<AlignedHeader>,
        _samples: Vec<i16>,
    }

    #[repr(C, align(8))]
    struct AlignedHeader(WAVEHDR);

    pub(super) struct Output {
        output: HWAVEOUT,
        buffers: VecDeque<Buffer>,
        last_units: u32,
        elapsed_units: u64,
        position_kind: Option<u32>,
        closed: bool,
        sample_rate: u32,
        channels: u8,
    }

    // Output is created/used/destroyed by one movie worker. Heap-owned PCM and
    // boxed headers stay at fixed addresses while the driver borrows them.
    unsafe impl Send for Output {}

    impl Output {
        pub(super) fn open_pcm(sample_rate: u32, channels: u8) -> RuntimeResult<Self> {
            if sample_rate == 0 || sample_rate > 384_000 || !matches!(channels, 1 | 2) {
                return Err(error("unsupported PCM output sample rate/channel count"));
            }
            let block_align = u16::from(channels) * 2;
            let format = WAVEFORMATEX {
                wFormatTag: WAVE_FORMAT_PCM as u16,
                nChannels: channels.into(),
                nSamplesPerSec: sample_rate,
                nAvgBytesPerSec: sample_rate * u32::from(block_align),
                nBlockAlign: block_align,
                wBitsPerSample: 16,
                cbSize: 0,
            };
            let mut output = 0;
            check("open movie device", unsafe {
                waveOutOpen(&mut output, WAVE_MAPPER, &format, 0, 0, 0)
            })?;
            Ok(Self {
                output,
                buffers: VecDeque::new(),
                last_units: 0,
                elapsed_units: 0,
                position_kind: None,
                closed: false,
                sample_rate,
                channels,
            })
        }

        fn reap(&mut self) -> RuntimeResult<()> {
            while self.buffers.front().is_some_and(|buffer| {
                // WinMM updates dwFlags asynchronously; ordinary cached loads
                // must not be used to decide whether borrowed memory is free.
                // The SDK type is packed; AlignedHeader ensures that dwFlags
                // (a multiple-of-four offset on Win32/Win64) is aligned.
                unsafe {
                    std::ptr::read_volatile(std::ptr::addr_of!(buffer.header.0.dwFlags)) & WHDR_DONE
                        != 0
                }
            }) {
                let buffer = self.buffers.front_mut().expect("front exists");
                check("unprepare movie buffer", unsafe {
                    waveOutUnprepareHeader(
                        self.output,
                        &mut buffer.header.0,
                        size_of::<WAVEHDR>() as u32,
                    )
                })?;
                self.buffers.pop_front();
            }
            Ok(())
        }
    }

    impl MovieAudioOutput for Output {
        fn submit(&mut self, samples: &[i16]) -> RuntimeResult<bool> {
            self.reap()?;
            if self.closed {
                return Err(error("movie audio device is closed"));
            }
            if self.buffers.len() >= 8 {
                return Ok(false);
            }
            if samples.is_empty()
                || samples.len() % usize::from(self.channels) != 0
                || samples.len() > 48_000
            {
                return Err(error(
                    "PCM buffer must contain bounded, aligned sample frames",
                ));
            }
            let mut samples = samples.to_vec();
            let mut header = Box::new(AlignedHeader(WAVEHDR {
                lpData: samples.as_mut_ptr().cast(),
                dwBufferLength: (samples.len() * 2) as u32,
                dwBytesRecorded: 0,
                dwUser: 0,
                dwFlags: 0,
                dwLoops: 0,
                lpNext: std::ptr::null_mut(),
                reserved: 0,
            }));
            check("prepare movie buffer", unsafe {
                waveOutPrepareHeader(self.output, &mut header.0, size_of::<WAVEHDR>() as u32)
            })?;
            // Retain ownership before write: an error must not free memory
            // while a buggy/late device still holds a prepared header.
            self.buffers.push_back(Buffer {
                header,
                _samples: samples,
            });
            let buffer = self.buffers.back_mut().expect("buffer inserted");
            check("write movie buffer", unsafe {
                waveOutWrite(
                    self.output,
                    &mut buffer.header.0,
                    size_of::<WAVEHDR>() as u32,
                )
            })?;
            Ok(true)
        }

        fn position_seconds(&mut self) -> RuntimeResult<f64> {
            let mut position = Position {
                kind: 2,
                value: [0; 2],
            }; // TIME_SAMPLES
            check("query movie clock", unsafe {
                waveOutGetPosition(self.output, &mut position, size_of::<Position>() as u32)
            })?;
            let divisor = match position.kind {
                2 => f64::from(self.sample_rate),
                4 => f64::from(self.sample_rate) * f64::from(self.channels) * 2.0, // TIME_BYTES
                1 => 1_000.0,                                                      // TIME_MS
                _ => return Err(error("WinMM returned an unsupported movie clock unit")),
            };
            if self.position_kind.is_some_and(|kind| kind != position.kind) {
                return Err(error("WinMM changed movie clock units during playback"));
            }
            self.position_kind = Some(position.kind);
            let units = position.value[0];
            self.elapsed_units += u64::from(units.wrapping_sub(self.last_units));
            self.last_units = units;
            Ok(self.elapsed_units as f64 / divisor)
        }

        fn checkpoint_position_frames(&mut self) -> RuntimeResult<u64> {
            self.position_seconds()?;
            match self.position_kind {
                Some(2) => Ok(self.elapsed_units),
                Some(4) if self.elapsed_units % (u64::from(self.channels) * 2) == 0 =>
                    Ok(self.elapsed_units / (u64::from(self.channels) * 2)),
                _ => Err(error("WinMM SOUND checkpoint requires a sample-aligned frame/byte counter")),
            }
        }

        fn drained(&mut self) -> RuntimeResult<bool> {
            self.reap()?;
            Ok(self.buffers.is_empty())
        }

        fn pause(&mut self, paused: bool) -> RuntimeResult<()> {
            check("pause/restart movie audio", unsafe {
                if paused {
                    waveOutPause(self.output)
                } else {
                    waveOutRestart(self.output)
                }
            })
        }

        fn stop(&mut self) -> RuntimeResult<()> {
            if self.closed {
                return Ok(());
            }
            check("reset movie audio", unsafe { waveOutReset(self.output) })?;
            while let Some(buffer) = self.buffers.front_mut() {
                check("release movie buffer", unsafe {
                    waveOutUnprepareHeader(
                        self.output,
                        &mut buffer.header.0,
                        size_of::<WAVEHDR>() as u32,
                    )
                })?;
                self.buffers.pop_front();
            }
            check("close movie audio", unsafe { waveOutClose(self.output) })?;
            self.closed = true;
            Ok(())
        }
    }

    impl Drop for Output {
        fn drop(&mut self) {
            if self.stop().is_err() {
                // Preserve driver-borrowed memory on an exceptional reset /
                // unprepare failure. Freeing it would be a use-after-free.
                for buffer in self.buffers.drain(..) {
                    std::mem::forget(buffer);
                }
            }
        }
    }

    fn check(operation: &str, result: u32) -> RuntimeResult<()> {
        if result == 0 {
            Ok(())
        } else {
            Err(error(format!(
                "WinMM {operation} failed with MMRESULT {result}"
            )))
        }
    }
}

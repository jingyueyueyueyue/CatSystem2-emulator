//! Android system PCM output through AAudio (API26+). No JNI, decoder plugin,
//! original-game code, callback thread or external crate is involved.
//! FFI declarations follow the installed Android NDK28.2 public AAudio.h.
use std::collections::VecDeque;
use std::ffi::{CStr, c_char, c_void};
use std::ptr::NonNull;
use std::time::{Duration, Instant};

use cs2_runtime::RuntimeResult;

use crate::media_process::error;
use crate::movie_audio::MovieAudioOutput;

const OK: i32 = 0;
const OUTPUT: i32 = 0;
const PCM_I16: i32 = 1;
const SHARED: i32 = 1;
const LOW_LATENCY: i32 = 12;
const PERFORMANCE_NONE: i32 = 10;
const OPEN: i32 = 2;
const STARTED: i32 = 4;
const PAUSED: i32 = 6;
const STOPPED: i32 = 10;
const DISCONNECTED: i32 = 13;
const WOULD_BLOCK: i32 = -884;
const CONTROL_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_CHUNK_SAMPLES: usize = 48_000;
const MAX_CHUNKS: usize = 8;

#[repr(C)]
struct AAudioStreamBuilder {
    _private: [u8; 0],
}
#[repr(C)]
struct AAudioStream {
    _private: [u8; 0],
}

#[link(name = "aaudio")]
unsafe extern "C" {
    fn AAudio_createStreamBuilder(builder: *mut *mut AAudioStreamBuilder) -> i32;
    fn AAudioStreamBuilder_delete(builder: *mut AAudioStreamBuilder) -> i32;
    fn AAudioStreamBuilder_setDirection(builder: *mut AAudioStreamBuilder, direction: i32);
    fn AAudioStreamBuilder_setSharingMode(builder: *mut AAudioStreamBuilder, mode: i32);
    fn AAudioStreamBuilder_setPerformanceMode(builder: *mut AAudioStreamBuilder, mode: i32);
    fn AAudioStreamBuilder_setFormat(builder: *mut AAudioStreamBuilder, format: i32);
    fn AAudioStreamBuilder_setSampleRate(builder: *mut AAudioStreamBuilder, rate: i32);
    fn AAudioStreamBuilder_setChannelCount(builder: *mut AAudioStreamBuilder, channels: i32);
    fn AAudioStreamBuilder_setBufferCapacityInFrames(
        builder: *mut AAudioStreamBuilder,
        frames: i32,
    );
    fn AAudioStreamBuilder_openStream(
        builder: *mut AAudioStreamBuilder,
        stream: *mut *mut AAudioStream,
    ) -> i32;
    fn AAudioStream_getSampleRate(stream: *mut AAudioStream) -> i32;
    fn AAudioStream_getChannelCount(stream: *mut AAudioStream) -> i32;
    fn AAudioStream_getFormat(stream: *mut AAudioStream) -> i32;
    fn AAudioStream_getState(stream: *mut AAudioStream) -> i32;
    fn AAudioStream_waitForStateChange(
        stream: *mut AAudioStream,
        input: i32,
        next: *mut i32,
        nanos: i64,
    ) -> i32;
    fn AAudioStream_requestStart(stream: *mut AAudioStream) -> i32;
    fn AAudioStream_requestPause(stream: *mut AAudioStream) -> i32;
    fn AAudioStream_requestStop(stream: *mut AAudioStream) -> i32;
    fn AAudioStream_setBufferSizeInFrames(stream: *mut AAudioStream, frames: i32) -> i32;
    fn AAudioStream_getBufferCapacityInFrames(stream: *mut AAudioStream) -> i32;
    fn AAudioStream_getFramesRead(stream: *mut AAudioStream) -> i64;
    fn AAudioStream_getFramesWritten(stream: *mut AAudioStream) -> i64;
    fn AAudioStream_write(
        stream: *mut AAudioStream,
        data: *const c_void,
        frames: i32,
        nanos: i64,
    ) -> i32;
    fn AAudioStream_close(stream: *mut AAudioStream) -> i32;
    fn AAudio_convertResultToText(result: i32) -> *const c_char;
}

struct Builder(NonNull<AAudioStreamBuilder>);
impl Drop for Builder {
    fn drop(&mut self) {
        // The builder retains no borrowed PCM and is independent of the stream.
        unsafe {
            AAudioStreamBuilder_delete(self.0.as_ptr());
        }
    }
}

struct Pending {
    samples: Vec<i16>,
    offset: usize,
}

pub(crate) struct Output {
    stream: Option<NonNull<AAudioStream>>,
    pending: VecDeque<Pending>,
    rate: u32,
    channels: u8,
    max_queued_frames: u64,
    pending_frames: u64,
    accepted_frames: u64,
    last_consumed: u64,
    paused: bool,
    started: bool,
    failed: Option<String>,
}

// One PCM worker owns every method call and the final close. No data callback
// or second thread can access this stream; pending PCM is copied by write().
unsafe impl Send for Output {}

impl Output {
    pub(crate) fn open_pcm(rate: u32, channels: u8) -> RuntimeResult<Self> {
        if rate == 0 || rate > 384_000 || !matches!(channels, 1 | 2) {
            return Err(error(
                "AAudio requires PCM16 mono/stereo at a supported sample rate",
            ));
        }
        let mut builder = std::ptr::null_mut();
        check("create builder", unsafe {
            AAudio_createStreamBuilder(&mut builder)
        })?;
        let builder = Builder(
            NonNull::new(builder)
                .ok_or_else(|| error("AAudio returned a null builder after success"))?,
        );
        unsafe {
            AAudioStreamBuilder_setDirection(builder.0.as_ptr(), OUTPUT);
            AAudioStreamBuilder_setSharingMode(builder.0.as_ptr(), SHARED);
            AAudioStreamBuilder_setPerformanceMode(builder.0.as_ptr(), LOW_LATENCY);
            AAudioStreamBuilder_setFormat(builder.0.as_ptr(), PCM_I16);
            AAudioStreamBuilder_setSampleRate(builder.0.as_ptr(), rate as i32);
            AAudioStreamBuilder_setChannelCount(builder.0.as_ptr(), i32::from(channels));
            AAudioStreamBuilder_setBufferCapacityInFrames(
                builder.0.as_ptr(),
                (rate / 10).max(1) as i32,
            );
        }
        let mut stream = std::ptr::null_mut();
        let mut opened = unsafe { AAudioStreamBuilder_openStream(builder.0.as_ptr(), &mut stream) };
        if opened < 0 {
            // The low-latency path is a request, not a format requirement.
            // A shared normal stream still uses the exact declared PCM layout.
            unsafe {
                AAudioStreamBuilder_setPerformanceMode(builder.0.as_ptr(), PERFORMANCE_NONE);
            }
            stream = std::ptr::null_mut();
            opened = unsafe { AAudioStreamBuilder_openStream(builder.0.as_ptr(), &mut stream) };
        }
        check("open shared output stream", opened)?;
        let stream = NonNull::new(stream)
            .ok_or_else(|| error("AAudio returned a null output stream after success"))?;
        let mut output = Self {
            stream: Some(stream),
            pending: VecDeque::new(),
            rate,
            channels,
            max_queued_frames: u64::from(rate / 5).max(1),
            pending_frames: 0,
            accepted_frames: 0,
            last_consumed: 0,
            paused: true,
            started: false,
            failed: None,
        };
        let actual = unsafe {
            (
                AAudioStream_getSampleRate(stream.as_ptr()),
                AAudioStream_getChannelCount(stream.as_ptr()),
                AAudioStream_getFormat(stream.as_ptr()),
            )
        };
        if actual != (rate as i32, i32::from(channels), PCM_I16) {
            return Err(error(format!(
                "AAudio opened rate/channels/format {actual:?}, expected ({rate}, {channels}, PCM_I16); no implicit reinterpretation is allowed",
            )));
        }
        let capacity = unsafe { AAudioStream_getBufferCapacityInFrames(stream.as_ptr()) };
        if capacity <= 0 || capacity as u32 > rate.saturating_mul(2) {
            return Err(error(format!(
                "AAudio returned an invalid/oversized buffer capacity: {capacity}"
            )));
        }
        let size = unsafe {
            AAudioStream_setBufferSizeInFrames(stream.as_ptr(), (rate / 25).max(1) as i32)
        };
        if size <= 0 {
            return Err(output.device_error("configure buffer size", size));
        }
        // A driver may choose a minimum buffer larger than requested. Include
        // that known capacity in the bound while retaining a finite PCM queue.
        output.max_queued_frames = output
            .max_queued_frames
            .max(size as u64 + u64::from(rate / 10));
        Ok(output)
    }

    fn stream(&self) -> RuntimeResult<NonNull<AAudioStream>> {
        if let Some(cause) = &self.failed {
            return Err(error(cause));
        }
        self.stream.ok_or_else(|| error("AAudio output is closed"))
    }

    fn device_error(&mut self, operation: &str, result: i32) -> cs2_runtime::RuntimeError {
        let cause = format!("AAudio {operation}: {}", result_text(result));
        self.failed = Some(cause.clone());
        error(cause)
    }

    fn validate_device(&mut self) -> RuntimeResult<NonNull<AAudioStream>> {
        let stream = self.stream()?;
        let state = unsafe { AAudioStream_getState(stream.as_ptr()) };
        if state == DISCONNECTED {
            return Err(self.device_error("output device disconnected", -899));
        }
        Ok(stream)
    }

    /// Nonblocking writes may consume only a prefix. Keep that exact cursor;
    /// never report false after accepting samples or repeat a written prefix.
    fn pump(&mut self) -> RuntimeResult<()> {
        let stream = self.validate_device()?;
        while let Some(pending) = self.pending.front_mut() {
            let channels = usize::from(self.channels);
            let frames = (pending.samples.len() - pending.offset) / channels;
            let result = unsafe {
                AAudioStream_write(
                    stream.as_ptr(),
                    pending.samples[pending.offset..].as_ptr().cast(),
                    frames as i32,
                    0,
                )
            };
            if result == WOULD_BLOCK {
                break;
            }
            if result < 0 {
                return Err(self.device_error("write PCM", result));
            }
            if result == 0 {
                break;
            }
            if result as usize > frames {
                return Err(self.device_error("write returned more frames than supplied", -896));
            }
            pending.offset += result as usize * channels;
            self.pending_frames -= result as u64;
            if pending.offset == pending.samples.len() {
                self.pending.pop_front();
            }
        }
        Ok(())
    }

    fn counters(&mut self) -> RuntimeResult<(u64, u64)> {
        let stream = self.validate_device()?;
        let read = unsafe { AAudioStream_getFramesRead(stream.as_ptr()) };
        let written = unsafe { AAudioStream_getFramesWritten(stream.as_ptr()) };
        if read < 0 || written < 0 || (read as u64) < self.last_consumed {
            return Err(error(
                "AAudio returned invalid or regressing device frame counters",
            ));
        }
        // Endpoint counters are the actual audio consumer, not the game frame
        // clock. Clamp underrun silence to samples the app submitted.
        let consumed = (read as u64).min(written as u64).min(self.accepted_frames);
        self.last_consumed = self.last_consumed.max(consumed);
        Ok((self.last_consumed, written as u64))
    }

    fn wait_state(&mut self, expected: i32) -> RuntimeResult<()> {
        let stream = self.validate_device()?;
        let deadline = Instant::now() + CONTROL_TIMEOUT;
        loop {
            let state = unsafe { AAudioStream_getState(stream.as_ptr()) };
            if state == expected {
                return Ok(());
            }
            if state == DISCONNECTED {
                return Err(self.device_error("device disconnected during control", -899));
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(self.device_error("control state acknowledgement timed out", -885));
            }
            let mut next = state;
            let result = unsafe {
                AAudioStream_waitForStateChange(
                    stream.as_ptr(),
                    state,
                    &mut next,
                    left.min(Duration::from_millis(20)).as_nanos() as i64,
                )
            };
            if result < 0 && next == state {
                // A bounded wait timeout is expected while an async request
                // is still transitioning; other results retain diagnostics.
                if result != -885 {
                    return Err(self.device_error("wait for output state", result));
                }
            }
        }
    }
}

impl MovieAudioOutput for Output {
    fn submit(&mut self, samples: &[i16]) -> RuntimeResult<bool> {
        if samples.is_empty()
            || samples.len() > MAX_CHUNK_SAMPLES
            || samples.len() % usize::from(self.channels) != 0
        {
            return Err(error(
                "AAudio PCM chunk must be bounded, nonempty and frame-aligned",
            ));
        }
        self.pump()?;
        let (consumed, _) = self.counters()?;
        let frames = (samples.len() / usize::from(self.channels)) as u64;
        if frames > self.max_queued_frames {
            return Err(error("AAudio PCM chunk exceeds its bounded device queue"));
        }
        if self.pending.len() >= MAX_CHUNKS
            || self.accepted_frames.saturating_sub(consumed) + frames > self.max_queued_frames
        {
            return Ok(false);
        }
        self.accepted_frames = self
            .accepted_frames
            .checked_add(frames)
            .ok_or_else(|| error("AAudio PCM frame count exhausted"))?;
        self.pending_frames += frames;
        self.pending.push_back(Pending {
            samples: samples.to_vec(),
            offset: 0,
        });
        self.pump()?;
        Ok(true)
    }

    fn position_seconds(&mut self) -> RuntimeResult<f64> {
        self.pump()?;
        let (consumed, _) = self.counters()?;
        Ok(consumed as f64 / f64::from(self.rate))
    }

    fn checkpoint_position_frames(&mut self) -> RuntimeResult<u64> {
        self.pump()?;
        self.counters().map(|(consumed, _)| consumed)
    }

    fn drained(&mut self) -> RuntimeResult<bool> {
        self.pump()?;
        let (consumed, _) = self.counters()?;
        Ok(self.pending.is_empty() && consumed >= self.accepted_frames)
    }

    fn pause(&mut self, paused: bool) -> RuntimeResult<()> {
        let stream = self.validate_device()?;
        if paused == self.paused {
            return Ok(());
        }
        if paused {
            check("request pause", unsafe {
                AAudioStream_requestPause(stream.as_ptr())
            })?;
            self.wait_state(PAUSED)?;
        } else {
            self.pump()?;
            check("request start", unsafe {
                AAudioStream_requestStart(stream.as_ptr())
            })?;
            self.wait_state(STARTED)?;
            self.started = true;
        }
        self.paused = paused;
        Ok(())
    }

    fn stop(&mut self) -> RuntimeResult<()> {
        let Some(stream) = self.stream else {
            return Ok(());
        };
        let mut result = self
            .failed
            .as_ref()
            .map_or(Ok(()), |cause| Err(error(cause)));
        let state = unsafe { AAudioStream_getState(stream.as_ptr()) };
        if self.started && !matches!(state, OPEN | STOPPED | DISCONNECTED) {
            let requested = check("request stop", unsafe {
                AAudioStream_requestStop(stream.as_ptr())
            });
            if requested.is_ok() && self.failed.is_none() {
                result = result.and(self.wait_state(STOPPED));
            } else {
                result = result.and(requested);
            }
        }
        // No callback or blocking writer can retain this stream. Close once
        // even after disconnection/stop failure; never fake a successful close.
        self.stream = None;
        let closed = check("close output stream", unsafe {
            AAudioStream_close(stream.as_ptr())
        });
        self.pending.clear();
        self.pending_frames = 0;
        result.and(closed)
    }
}

impl Drop for Output {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

fn check(operation: &str, result: i32) -> RuntimeResult<()> {
    if result == OK {
        Ok(())
    } else {
        Err(error(format!(
            "AAudio {operation}: {}",
            result_text(result)
        )))
    }
}

fn result_text(result: i32) -> String {
    let text = unsafe { AAudio_convertResultToText(result) };
    if text.is_null() {
        return format!("status {result}");
    }
    format!(
        "{} ({result})",
        unsafe { CStr::from_ptr(text) }.to_string_lossy()
    )
}

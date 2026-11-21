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
use crate::perf_detail::audio::Stage;

#[path = "audio_android_standby.rs"]
mod standby;
pub(crate) use standby::{Standby, StandbyPool};

const OK: i32 = 0;
const OUTPUT: i32 = 0;
const PCM_I16: i32 = 1;
const SHARED: i32 = 1;
const LOW_LATENCY: i32 = 12;
const PERFORMANCE_NONE: i32 = 10;
const OPEN: i32 = 2;
const STARTING: i32 = 3;
const STARTED: i32 = 4;
const PAUSED: i32 = 6;
const STOPPED: i32 = 10;
const DISCONNECTED: i32 = 13;
const WOULD_BLOCK: i32 = -884;
const CONTROL_TIMEOUT: Duration = Duration::from_secs(2);
const START_BUFFER_RETRY_DELAY: Duration = Duration::from_millis(100);
const STATE_WAIT_SLICE: Duration = Duration::from_millis(20);
const START_PROGRESS_SLICE: Duration = Duration::from_millis(5);
const INITIAL_PROGRESS_WINDOW: Duration = Duration::from_millis(8);
const INITIAL_PROGRESS_SLICE: Duration = Duration::from_millis(1);
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
    fn AAudioStream_getXRunCount(stream: *mut AAudioStream) -> i32;
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

enum StagedDevice {
    Open,
    Started(i32),
}

pub(crate) struct Output {
    stream: Option<NonNull<AAudioStream>>,
    pending: VecDeque<Pending>,
    rate: u32,
    channels: u8,
    device_buffer_capacity: i32,
    device_buffer_size: i32,
    max_queued_frames: u64,
    pending_frames: u64,
    accepted_frames: u64,
    last_consumed: u64,
    frame_origin: u64,
    // The first positive program-frame counter may arrive after PLAY's ack.
    first_consumption: Option<(crate::perf_detail::audio::Shared, u64, &'static str)>,
    staged_device: Option<StagedDevice>,
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
            device_buffer_capacity: 0,
            device_buffer_size: 0,
            max_queued_frames: u64::from(rate / 5).max(1),
            pending_frames: 0,
            accepted_frames: 0,
            last_consumed: 0,
            frame_origin: 0,
            first_consumption: None,
            staged_device: None,
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
        output.device_buffer_capacity = capacity;
        output.device_buffer_size = size;
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
        if self.staged_device.is_some() {
            // Program PCM remains private until activation validates the
            // silent device's origin and allows the first real write.
            return Ok(());
        }
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
        if self.staged_device.is_some() {
            return Ok((0, 0));
        }
        let stream = self.validate_device()?;
        let read = unsafe { AAudioStream_getFramesRead(stream.as_ptr()) };
        let written = unsafe { AAudioStream_getFramesWritten(stream.as_ptr()) };
        if read < 0 || written < 0 {
            return Err(error(
                "AAudio returned invalid or regressing device frame counters",
            ));
        }
        let program_read = (read as u64).saturating_sub(self.frame_origin);
        let program_written = (written as u64).saturating_sub(self.frame_origin);
        if program_read < self.last_consumed {
            return Err(error("AAudio returned invalid or regressing device frame counters"));
        }
        // Endpoint counters are the actual audio consumer, not the game frame
        // clock. Clamp underrun silence to samples the app submitted.
        let consumed = program_read.min(program_written).min(self.accepted_frames);
        self.last_consumed = self.last_consumed.max(consumed);
        if self.first_consumption.as_ref().is_some_and(|(_, position, _)| consumed > *position)
            && let Some((command, position, route)) = self.first_consumption.take()
        {
            let elapsed_us = command.elapsed().as_micros();
            let state = unsafe { AAudioStream_getState(stream.as_ptr()) };
            let xruns = unsafe { AAudioStream_getXRunCount(stream.as_ptr()) };
            report_audio(&format!(
                "audio_first_consume audio_id={} channel={} name={} route={route} rate={} channels={} elapsed_us={elapsed_us} state={state} position={position} consumed={consumed} device_read={read} device_written={written} accepted={} pending={} origin={} xruns={xruns}",
                command.id, command.channel, command.name(), self.rate, self.channels,
                self.accepted_frames, self.pending_frames, self.frame_origin,
            ));
        }
        Ok((self.last_consumed, program_written))
    }

    fn observe_start(&mut self, position: u64, route: &'static str) {
        self.first_consumption = crate::perf_detail::audio::active().map(|command| {
            report_audio(&format!(
                "audio_start_probe audio_id={} channel={} name={} route={route} rate={} channels={} elapsed_us={} position={position} accepted={} pending={} origin={}",
                command.id, command.channel, command.name(), self.rate, self.channels,
                command.elapsed().as_micros(), self.accepted_frames, self.pending_frames, self.frame_origin,
            ));
            (command, position, route)
        });
    }

    fn wait_state(&mut self, expected: i32, start_position: Option<u64>) -> RuntimeResult<()> {
        let _timing = Stage::Confirm.begin();
        let stream = self.validate_device()?;
        let start = Instant::now();
        let deadline = start + CONTROL_TIMEOUT;
        let mut can_expand_start_buffer = expected == STARTED
            && self.device_buffer_size < self.device_buffer_capacity;
        loop {
            let state = unsafe { AAudioStream_getState(stream.as_ptr()) };
            if state == expected {
                return Ok(());
            }
            if state == DISCONNECTED {
                return Err(self.device_error("device disconnected during control", -899));
            }
            // Some shared AudioTrack streams consume submitted PCM while
            // still reporting STARTING. Measured playback is enough to proceed.
            if state == STARTING && let Some(position) = start_position
                && self.counters()?.0 > position
            {
                return Ok(());
            }
            if expected == STARTED {
                // A shared AudioTrack may still be waiting for enough PCM to
                // start. Feed the queued tail while its request is in flight.
                if can_expand_start_buffer && !self.pending.is_empty()
                    && start.elapsed() >= START_BUFFER_RETRY_DELAY
                {
                    can_expand_start_buffer = false;
                    let size = {
                        let _timing = Stage::ConfirmExpand.begin();
                        unsafe { AAudioStream_setBufferSizeInFrames(stream.as_ptr(), self.device_buffer_capacity) }
                    };
                    if size > self.device_buffer_size && size <= self.device_buffer_capacity {
                        self.device_buffer_size = size;
                        self.max_queued_frames = self.max_queued_frames
                            .max(size as u64 + u64::from(self.rate / 10));
                    }
                }
                let _timing = Stage::ConfirmPump.begin();
                self.pump()?;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                if let Some(position) = start_position {
                    let (consumed, written) = self.counters()?;
                    let cause = format!("AAudio state={state}, start_position={position}, consumed={consumed}, written={written}: {}", result_text(-885));
                    self.failed = Some(cause.clone());
                    return Err(cs2_runtime::RuntimeError::AudioStartTimeout(cause));
                }
                return Err(self.device_error("control state acknowledgement timed out", -885));
            }
            let mut next = state;
            // Legacy shared AudioTrack may keep reporting STARTING even
            // while it consumes real PCM. Bound the first progress probes
            // more tightly during the first device period. Return to the
            // normal control cadence if startup stalls.
            let slice = if state == STARTING
                && start_position.is_some()
                && start.elapsed() < START_BUFFER_RETRY_DELAY
            {
                if start.elapsed() < INITIAL_PROGRESS_WINDOW {
                    INITIAL_PROGRESS_SLICE
                } else {
                    START_PROGRESS_SLICE
                }
            } else {
                STATE_WAIT_SLICE
            };
            let result = {
                let _timing = Stage::StateWait.begin();
                unsafe { AAudioStream_waitForStateChange(
                    stream.as_ptr(),
                    state,
                    &mut next,
                    left.min(slice).as_nanos() as i64,
                ) }
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
        if paused {
            self.first_consumption = None;
        }
        if paused && self.staged_device.is_some() {
            return Ok(());
        }
        let mut route = if self.started { "resume" } else { "cold" };
        if let Some(staged) = self.staged_device.take() {
            if let StagedDevice::Started(xruns) = staged
                && self.can_activate_standby(xruns)
            {
                self.observe_start(0, "started");
                self.pump()?;
                let stream = self.validate_device()?;
                if unsafe { AAudioStream_getXRunCount(stream.as_ptr()) } != xruns {
                    // Programming PCM has already been offered. Restarting
                    // it could duplicate an audible prefix, and the lost
                    // device origin can no longer prove exact progress.
                    let cause = "AAudio standby lost its device clock during activation".to_owned();
                    self.failed = Some(cause.clone());
                    return Err(cs2_runtime::RuntimeError::AudioStartTimeout(cause));
                }
                self.wait_state(STARTED, Some(0))?;
                self.paused = false;
                return Ok(());
            }
            if !matches!(staged, StagedDevice::Open) || !self.can_activate_opened() {
                // No program PCM has reached the expired or disconnected
                // standby. Transfer the exact staged PCM to a fresh device.
                let mut fresh = Self::open_pcm(self.rate, self.channels)?;
                fresh.pending = std::mem::take(&mut self.pending);
                fresh.pending_frames = self.pending_frames;
                fresh.accepted_frames = self.accepted_frames;
                drop(std::mem::replace(self, fresh));
                route = "reopened";
            } else {
                route = "opened";
            }
        }
        let stream = self.validate_device()?;
        if paused == self.paused {
            return Ok(());
        }
        if paused {
            {
                let _timing = Stage::Request.begin();
                check("request pause", unsafe { AAudioStream_requestPause(stream.as_ptr()) })?;
            }
            self.wait_state(PAUSED, None)?;
        } else {
            self.pump()?;
            let (position, _) = self.counters()?;
            self.observe_start(position, route);
            {
                let _timing = Stage::Request.begin();
                check("request start", unsafe { AAudioStream_requestStart(stream.as_ptr()) })?;
            }
            self.wait_state(STARTED, Some(position))?;
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
            let requested = {
                let _timing = Stage::Request.begin();
                check("request stop", unsafe { AAudioStream_requestStop(stream.as_ptr()) })
            };
            if requested.is_ok() && self.failed.is_none() {
                result = result.and(self.wait_state(STOPPED, None));
            } else {
                result = result.and(requested);
            }
        }
        // No callback or blocking writer can retain this stream. Close once
        // even after disconnection/stop failure; never fake a successful close.
        self.stream = None;
        let closed = {
            let _timing = Stage::Close.begin();
            check("close output stream", unsafe { AAudioStream_close(stream.as_ptr()) })
        };
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

fn report_audio(message: &str) {
    #[link(name = "log")]
    unsafe extern "C" {
        fn __android_log_write(priority: i32, tag: *const c_char, text: *const c_char) -> i32;
    }
    if let Ok(message) = std::ffi::CString::new(message) {
        unsafe { __android_log_write(4, c"CS2Audio".as_ptr(), message.as_ptr()); }
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

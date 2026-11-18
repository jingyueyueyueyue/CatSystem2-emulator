//! Owned external decoders. Only encoded bytes cross the process boundary.
use crate::media_input::MediaInput;
use cs2_runtime::{RuntimeError, RuntimeResult};
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

pub(crate) fn error(message: impl std::fmt::Display) -> RuntimeError {
    RuntimeError::Host(message.to_string())
}

#[derive(Clone)]
pub(crate) struct Cancellation {
    flag: Arc<AtomicBool>,
    child: Arc<Mutex<Option<Child>>>,
}

impl Cancellation {
    #[cfg(target_os = "android")]
    pub(crate) fn in_process() -> Self {
        Self {
            flag: Arc::new(AtomicBool::new(false)),
            child: Arc::new(Mutex::new(None)),
        }
    }

    pub(crate) fn cancel(&self) {
        self.flag.store(true, Ordering::Release);
        if let Ok(mut slot) = self.child.lock()
            && let Some(child) = slot.as_mut()
        {
            let _ = child.kill();
        }
    }

    pub(crate) fn cancelled(&self) -> bool {
        self.flag.load(Ordering::Acquire)
    }
}

pub(crate) struct DecoderProcess {
    pub(crate) cancellation: Cancellation,
    pub(crate) timestamps: mpsc::Receiver<RuntimeResult<VideoTiming>>,
    stderr: Arc<Mutex<VecDeque<String>>>,
    workers: Vec<JoinHandle<()>>,
    _input: Arc<MediaInput>,
}

pub(crate) struct VideoTiming {
    pub(crate) time: f64,
    pub(crate) duration: Option<f64>,
}

pub(crate) struct DecoderDeadline {
    stop: mpsc::SyncSender<()>,
    worker: Option<JoinHandle<()>>,
}

impl DecoderDeadline {
    pub(crate) fn arm(cancellation: Cancellation, timeout: Duration) -> RuntimeResult<Self> {
        let (stop, receiver) = mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("cs2-media-deadline".into())
            .spawn(move || {
                if matches!(
                    receiver.recv_timeout(timeout),
                    Err(mpsc::RecvTimeoutError::Timeout)
                ) {
                    cancellation.cancel();
                }
            })
            .map_err(|cause| error(format!("start media probe deadline: {cause}")))?;
        Ok(Self {
            stop,
            worker: Some(worker),
        })
    }
}

impl Drop for DecoderDeadline {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl DecoderProcess {
    pub(crate) fn spawn(
        executable: &Path,
        input: Arc<MediaInput>,
        input_format: &str,
        output_arguments: &[String],
        video_timestamps: bool,
    ) -> RuntimeResult<(Self, ChildStdout)> {
        let mut command = Command::new(executable);
        command.args([
            "-hide_banner",
            "-nostdin",
            "-nostats",
            "-loglevel",
            if video_timestamps { "info" } else { "error" },
            "-copyts",
            "-start_at_zero",
            "-f",
            input_format,
        ]);
        command.arg("-i").arg(input.path());
        command.args(output_arguments);
        Self::launch(command, input, video_timestamps)
    }

    pub(crate) fn probe_streams(
        executable: &Path,
        input: Arc<MediaInput>,
        input_format: &str,
    ) -> RuntimeResult<(Self, ChildStdout)> {
        let mut command = Command::new(executable);
        command.args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-show_entries",
            "stream=codec_type,width,height,sample_aspect_ratio,avg_frame_rate,r_frame_rate,time_base,duration,duration_ts,nb_frames,sample_rate,channels:stream_tags=DURATION:stream_side_data=rotation:format=duration",
            "-of",
            "default=noprint_wrappers=0:nokey=0",
            "-f",
            input_format,
        ]);
        command.arg("-i").arg(input.path());
        Self::launch(command, input, false)
    }

    fn launch(
        mut command: Command,
        input: Arc<MediaInput>,
        video_timestamps: bool,
    ) -> RuntimeResult<(Self, ChildStdout)> {
        let tool_name = Path::new(command.get_program())
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if !matches!(
            tool_name.as_str(),
            "ffmpeg" | "ffmpeg.exe" | "ffprobe" | "ffprobe.exe"
        ) {
            return Err(error(
                "media tool must be named ffmpeg or ffprobe; guest executables are never launched",
            ));
        }
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let mut child = command.spawn().map_err(|cause| {
            error(format!(
                "start media tool `{:?}`: {cause}",
                command.get_program()
            ))
        })?;
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let (Some(stdout), Some(stderr)) = (stdout, stderr) else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error("FFmpeg did not provide its output/diagnostic pipes"));
        };
        let cancellation = Cancellation {
            flag: Arc::new(AtomicBool::new(false)),
            child: Arc::new(Mutex::new(Some(child))),
        };
        let (time_sender, timestamps) = mpsc::channel();
        let log = Arc::new(Mutex::new(VecDeque::new()));
        let mut process = Self {
            cancellation,
            timestamps,
            stderr: log.clone(),
            workers: Vec::new(),
            _input: input,
        };
        process.workers.push(
            thread::Builder::new()
                .name("cs2-media-log".into())
                .spawn(move || {
                    // Bound each line and the diagnostic tail even for a broken tool.
                    let mut reader = BufReader::new(stderr);
                    loop {
                        let mut line = Vec::new();
                        let read = reader.by_ref().take(8192).read_until(b'\n', &mut line);
                        match read {
                            Ok(0) => break,
                            Err(cause) => {
                                let _ = time_sender
                                    .send(Err(error(format!("read FFmpeg log: {cause}"))));
                                break;
                            }
                            Ok(_) => {}
                        }
                        let line = String::from_utf8_lossy(&line).into_owned();
                        if video_timestamps && line.contains("showinfo") && line.contains(" n:") {
                            let timestamp = line
                                .split("pts_time:")
                                .nth(1)
                                .and_then(|tail| tail.split_whitespace().next())
                                .and_then(|value| value.parse::<f64>().ok())
                                .filter(|value| value.is_finite() && *value >= -0.001)
                                .map(|value| VideoTiming {
                                    time: value.max(0.0),
                                    duration: line
                                        .split("duration_time:")
                                        .nth(1)
                                        .and_then(|tail| tail.split_whitespace().next())
                                        .and_then(|text| text.parse::<f64>().ok())
                                        .filter(|value| {
                                            value.is_finite() && *value > 0. && *value <= 3600.
                                        }),
                                })
                                .ok_or_else(|| {
                                    error(format!(
                                        "invalid decoded video timestamp: {}",
                                        line.trim()
                                    ))
                                });
                            let _ = time_sender.send(timestamp);
                        } else if let Ok(mut tail) = log.lock() {
                            if tail.len() == 24 {
                                tail.pop_front();
                            }
                            tail.push_back(line.trim().to_owned());
                        }
                    }
                })
                .map_err(|cause| error(format!("start FFmpeg log worker: {cause}")))?,
        );
        Ok((process, stdout))
    }

    pub(crate) fn diagnostic(&self) -> String {
        self.stderr
            .lock()
            .map(|lines| lines.iter().cloned().collect::<Vec<_>>().join("\n"))
            .unwrap_or_else(|_| "FFmpeg diagnostic lock was poisoned".into())
    }

    pub(crate) fn finish(&mut self) -> RuntimeResult<()> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let status = self
                .cancellation
                .child
                .lock()
                .map_err(|_| error("FFmpeg process lock was poisoned"))?
                .as_mut()
                .ok_or_else(|| error("FFmpeg process is missing"))?
                .try_wait()
                .map_err(|cause| error(format!("wait for FFmpeg: {cause}")))?;
            if let Some(status) = status {
                self.join_workers();
                return if status.success() {
                    Ok(())
                } else {
                    Err(error(format!(
                        "FFmpeg exited with {status}: {}",
                        self.diagnostic()
                    )))
                };
            }
            if self.cancellation.cancelled() {
                return Err(error("FFmpeg decoding was cancelled"));
            }
            if Instant::now() >= deadline {
                return Err(error("FFmpeg did not exit after closing its output"));
            }
            thread::sleep(Duration::from_millis(2));
        }
    }

    fn join_workers(&mut self) {
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

impl Drop for DecoderProcess {
    fn drop(&mut self) {
        self.cancellation.cancel();
        if let Ok(mut child) = self.cancellation.child.lock()
            && let Some(mut child) = child.take()
        {
            let _ = child.wait();
        }
        self.join_workers();
    }
}

pub(crate) fn send_bounded<T>(
    sender: &mpsc::SyncSender<T>,
    mut value: T,
    cancellation: &Cancellation,
) -> bool {
    loop {
        if cancellation.cancelled() {
            return false;
        }
        match sender.try_send(value) {
            Ok(()) => return true,
            Err(mpsc::TrySendError::Disconnected(_)) => return false,
            Err(mpsc::TrySendError::Full(pending)) => {
                value = pending;
                thread::sleep(Duration::from_millis(2));
            }
        }
    }
}

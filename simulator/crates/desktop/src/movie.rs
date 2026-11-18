//! FFmpeg decoding and a portable PTS scheduler; no guest EXE/DLL is loaded.
use crate::media_input::MediaInput;
use crate::media_process::{Cancellation, DecoderDeadline, DecoderProcess, error, send_bounded};
pub use crate::movie_audio::{MovieAudioFactory, MovieAudioOutput};
use cs2_formats::{MovieMetadata, MovieTimeBase};
use cs2_runtime::resource_router::ResourceMode;
use cs2_runtime::{
    MovieHost, MovieLoadResult, MovieRequest, PlaybackState, ResourceIndex, RuntimeError,
    RuntimeResult,
};
use std::collections::HashMap;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const START_TIMEOUT: Duration = Duration::from_secs(20);
const CONTROL_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MovieFormat {
    MpegProgramStream,
    Webm,
    Avi,
    Mp4,
    WindowsMedia,
    MpegVideo,
}

impl MovieFormat {
    #[cfg(target_os = "android")]
    pub(super) fn demuxer_name(self) -> &'static str {
        self.demuxer()
    }

    fn demuxer(self) -> &'static str {
        match self {
            Self::MpegProgramStream => "mpeg",
            Self::Webm => "webm",
            Self::Avi => "avi",
            Self::Mp4 => "mov",
            Self::WindowsMedia => "asf",
            Self::MpegVideo => "mpegvideo",
        }
    }

    fn detect(prefix: &[u8]) -> RuntimeResult<Self> {
        if prefix.starts_with(&[0x1a, 0x45, 0xdf, 0xa3]) {
            return Ok(Self::Webm);
        }
        if prefix.starts_with(&[0, 0, 1, 0xba]) {
            return Ok(Self::MpegProgramStream);
        }
        if prefix.starts_with(&[0, 0, 1, 0xb3]) {
            return Ok(Self::MpegVideo);
        }
        if prefix.starts_with(b"RIFF") && prefix.get(8..12) == Some(b"AVI ") {
            return Ok(Self::Avi);
        }
        if prefix.get(4..8).is_some_and(|tag| {
            matches!(
                tag,
                b"ftyp" | b"moov" | b"mdat" | b"wide" | b"free" | b"skip"
            )
        }) {
            return Ok(Self::Mp4);
        }
        if prefix.starts_with(&[
            0x30, 0x26, 0xb2, 0x75, 0x8e, 0x66, 0xcf, 0x11, 0xa6, 0xd9, 0, 0xaa, 0, 0x62, 0xce,
            0x6c,
        ]) {
            return Ok(Self::WindowsMedia);
        }
        Err(error(
            "movie signature is not MPEG, WebM, AVI, MP4 or ASF/WMV",
        ))
    }
}

#[derive(Debug, Clone)]
pub struct MovieFrame {
    pub width: u32,
    pub height: u32,
    pub presentation_time: f64,
    pub rgba: Arc<[u8]>,
}

pub trait MovieFramePresenter: Send + Sync {
    fn present(&self, handle: u64, frame: Arc<MovieFrame>) -> RuntimeResult<()>;
}

pub struct MovieFrameSurface {
    latest: Mutex<Option<(u64, Arc<MovieFrame>)>>,
    presented: Mutex<HashMap<u64, f64>>,
    visible: AtomicBool,
    game_drawing: AtomicBool,
    rectangle: Mutex<Option<[i32; 4]>>,
}

impl Default for MovieFrameSurface {
    fn default() -> Self {
        Self {
            latest: Mutex::new(None),
            presented: Mutex::new(HashMap::new()),
            visible: AtomicBool::new(false),
            game_drawing: AtomicBool::new(false),
            rectangle: Mutex::new(None),
        }
    }
}

impl MovieFrameSurface {
    pub fn display_rectangle(&self) -> RuntimeResult<Option<[i32; 4]>> {
        self.rectangle
            .lock()
            .map(|rectangle| *rectangle)
            .map_err(|_| error("movie display rectangle was poisoned"))
    }

    pub fn displaying_movie(&self) -> bool {
        self.visible.load(Ordering::Acquire)
    }

    pub fn latest(&self) -> RuntimeResult<Option<(u64, Arc<MovieFrame>)>> {
        if !self.visible.load(Ordering::Acquire) {
            return Ok(None);
        }
        self.retained_frame()
    }

    fn retained_frame(&self) -> RuntimeResult<Option<(u64, Arc<MovieFrame>)>> {
        self.latest
            .lock()
            .map(|frame| frame.clone())
            .map_err(|_| error("movie frame surface was poisoned"))
    }

    pub fn take_latest(&self) -> RuntimeResult<Option<(u64, Arc<MovieFrame>)>> {
        self.latest
            .lock()
            .map(|mut frame| frame.take())
            .map_err(|_| error("movie frame surface was poisoned"))
    }

    /// Call only after the window/backend has successfully presented this frame.
    pub fn mark_presented(&self, handle: u64, time: f64) -> RuntimeResult<()> {
        if !time.is_finite() || time < 0.0 {
            return Err(error("invalid presented movie timestamp"));
        }
        let latest = self
            .latest
            .lock()
            .map_err(|_| error("movie frame surface was poisoned"))?;
        if latest.as_ref().is_none_or(|(owner, _)| *owner != handle) {
            // A queued UI frame may finish after synchronous source release.
            // Its receipt must not resurrect a released presentation clock.
            return Ok(());
        }
        self.presented
            .lock()
            .map_err(|_| error("movie presentation clock was poisoned"))?
            .entry(handle)
            .and_modify(|last| *last = (*last).max(time))
            .or_insert(time);
        Ok(())
    }

    fn presented_time(&self, handle: u64) -> RuntimeResult<Option<f64>> {
        Ok(self
            .presented
            .lock()
            .map_err(|_| error("movie presentation clock was poisoned"))?
            .get(&handle)
            .copied())
    }

    fn clear(&self, handle: u64) -> RuntimeResult<()> {
        let mut latest = self
            .latest
            .lock()
            .map_err(|_| error("movie frame surface was poisoned"))?;
        if latest.as_ref().is_some_and(|(owner, _)| *owner == handle) {
            *latest = None;
        }
        self.presented
            .lock()
            .map_err(|_| error("movie presentation clock was poisoned"))?
            .remove(&handle);
        Ok(())
    }
}

impl MovieFramePresenter for MovieFrameSurface {
    fn present(&self, handle: u64, frame: Arc<MovieFrame>) -> RuntimeResult<()> {
        *self
            .latest
            .lock()
            .map_err(|_| error("movie frame surface was poisoned"))? = Some((handle, frame));
        Ok(())
    }
}

pub struct CallbackMoviePresenter<F>
where
    F: Fn(u64, Arc<MovieFrame>) -> RuntimeResult<()> + Send + Sync,
{
    callback: F,
}

impl<F> CallbackMoviePresenter<F>
where
    F: Fn(u64, Arc<MovieFrame>) -> RuntimeResult<()> + Send + Sync,
{
    pub fn new(callback: F) -> Self {
        Self { callback }
    }
}

impl<F> MovieFramePresenter for CallbackMoviePresenter<F>
where
    F: Fn(u64, Arc<MovieFrame>) -> RuntimeResult<()> + Send + Sync,
{
    fn present(&self, handle: u64, frame: Arc<MovieFrame>) -> RuntimeResult<()> {
        (self.callback)(handle, frame)
    }
}

#[derive(Clone)]
struct LoadedMovie {
    resource: String,
    #[cfg(not(target_os = "android"))]
    input: Arc<MediaInput>,
    #[cfg(target_os = "android")]
    resolve: crate::movie_android::Resolver,
    #[cfg(target_os = "android")]
    length: Option<u64>,
    format: MovieFormat,
    metadata: MovieMetadata,
    width: u32,
    height: u32,
    has_audio: bool,
}

enum MovieCommand {
    Pause(bool, mpsc::SyncSender<RuntimeResult<()>>),
    HostPause(mpsc::SyncSender<RuntimeResult<()>>),
    Stop,
}

#[derive(Default)]
struct DecoderGroup {
    cancelled: AtomicBool,
    processes: Mutex<Vec<Cancellation>>,
}

impl DecoderGroup {
    fn register(&self, cancellation: Cancellation) -> RuntimeResult<()> {
        let mut processes = self
            .processes
            .lock()
            .map_err(|_| error("movie process list was poisoned"))?;
        processes.retain(|process| !process.cancelled());
        if self.cancelled.load(Ordering::Acquire) {
            cancellation.cancel();
            return Err(error("movie startup was cancelled"));
        }
        processes.push(cancellation);
        Ok(())
    }

    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        if let Ok(processes) = self.processes.lock() {
            for process in processes.iter() {
                process.cancel();
            }
        }
    }
}

struct ActiveMovie {
    commands: mpsc::Sender<MovieCommand>,
    decoders: Arc<DecoderGroup>,
    finished: mpsc::Receiver<RuntimeResult<()>>,
    last_time: Arc<AtomicU64>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for ActiveMovie {
    fn drop(&mut self) {
        let _ = self.commands.send(MovieCommand::Stop);
        self.decoders.cancel();
        if let Some(worker) = self.thread.take() {
            let _ = worker.join();
        }
    }
}

struct Completion {
    result: Result<(), String>,
    last_time: Option<f64>,
}

pub struct MpegMovieHost {
    resolve: Arc<dyn Fn(&str) -> RuntimeResult<Box<dyn Read + Send>> + Send + Sync>,
    loaded: Option<LoadedMovie>,
    ffmpeg: PathBuf,
    next_handle: u64,
    active: HashMap<u64, ActiveMovie>,
    completed: HashMap<u64, Completion>,
    surface: Arc<MovieFrameSurface>,
    presenter: Option<Arc<dyn MovieFramePresenter>>,
    audio: Arc<dyn MovieAudioFactory>,
    enabled: bool,
    volume: Arc<AtomicU32>,
    host_paused: Arc<AtomicBool>,
    #[cfg(target_os = "android")]
    android_resources: Option<Arc<ResourceIndex>>,
    pub(super) planes: crate::movie_plane::MoviePlanes,
}

impl MpegMovieHost {
    pub fn new(resolve: impl Fn(&str) -> RuntimeResult<Vec<u8>> + Send + Sync + 'static) -> Self {
        Self::with_ffmpeg(
            resolve,
            discover_ffmpeg().unwrap_or_else(|| PathBuf::from("ffmpeg")),
        )
    }

    pub fn with_ffmpeg(
        resolve: impl Fn(&str) -> RuntimeResult<Vec<u8>> + Send + Sync + 'static,
        ffmpeg: impl Into<PathBuf>,
    ) -> Self {
        Self::with_reader_and_ffmpeg(
            move |name| {
                resolve(name).map(|bytes| Box::new(Cursor::new(bytes)) as Box<dyn Read + Send>)
            },
            ffmpeg,
        )
    }

    /// A streaming resource resolver avoids allocating an entire movie in RAM.
    /// Input is copied in 64 KiB chunks into an owned seekable host cache.
    pub fn with_reader(
        resolve: impl Fn(&str) -> RuntimeResult<Box<dyn Read + Send>> + Send + Sync + 'static,
    ) -> Self {
        Self::with_reader_and_ffmpeg(
            resolve,
            discover_ffmpeg().unwrap_or_else(|| PathBuf::from("ffmpeg")),
        )
    }

    pub fn with_reader_and_ffmpeg(
        resolve: impl Fn(&str) -> RuntimeResult<Box<dyn Read + Send>> + Send + Sync + 'static,
        ffmpeg: impl Into<PathBuf>,
    ) -> Self {
        Self {
            resolve: Arc::new(resolve),
            loaded: None,
            ffmpeg: ffmpeg.into(),
            next_handle: 1,
            active: HashMap::new(),
            completed: HashMap::new(),
            surface: Arc::new(MovieFrameSurface::default()),
            presenter: None,
            audio: Arc::new(crate::movie_audio::SystemMovieAudio),
            enabled: true,
            volume: Arc::new(AtomicU32::new(100)),
            host_paused: Arc::new(AtomicBool::new(false)),
            #[cfg(target_os = "android")]
            android_resources: None,
            planes: crate::movie_plane::MoviePlanes::default(),
        }
    }

    pub fn from_resources(resources: &ResourceIndex) -> Self {
        let resources = Arc::new(resources.clone());
        #[cfg(target_os = "android")]
        let indexed = resources.clone();
        let host = Self::with_reader(move |name| {
            let candidate = resources.resolve_mode_utf16(
                &name.encode_utf16().collect::<Vec<_>>(), ResourceMode::LooseFirst,
            )?.ok_or_else(|| RuntimeError::ResourceNotFound(name.to_owned()))?;
            resources.open_media_candidate(candidate.value, crate::media_input::MAX_MOVIE_BYTES)
                .map(|reader| Box::new(reader) as Box<dyn Read + Send>)
        });
        #[cfg(target_os = "android")]
        {
            let mut host = host;
            host.android_resources = Some(indexed);
            host
        }
        #[cfg(not(target_os = "android"))]
        {
            host
        }
    }

    pub fn set_presenter(&mut self, presenter: Arc<dyn MovieFramePresenter>) {
        self.presenter = Some(presenter);
    }

    pub fn set_audio_factory(&mut self, audio: Arc<dyn MovieAudioFactory>) {
        self.audio = audio;
    }

    /// App suspension preserves the script's own pause state and every PTS
    /// cursor. Signal all independent movie planes before awaiting controls.
    pub fn set_host_paused(&mut self, paused: bool) -> RuntimeResult<()> {
        self.host_paused.store(paused, Ordering::Release);
        self.poll()?;
        let mut first = None;
        let deadline = Instant::now() + CONTROL_TIMEOUT;
        let mut responses = Vec::new();
        self.queue_host_pause(paused, &mut responses, &mut first);
        for receiver in responses {
            let result = receiver
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .map_err(|cause| error(format!("movie host pause acknowledgement: {cause}")))
                .and_then(|result| result);
            if let Err(cause) = result {
                first.get_or_insert(cause);
            }
        }
        first.map_or(Ok(()), Err)
    }

    fn queue_host_pause(
        &mut self,
        paused: bool,
        responses: &mut Vec<mpsc::Receiver<RuntimeResult<()>>>,
        first: &mut Option<RuntimeError>,
    ) {
        self.host_paused.store(paused, Ordering::Release);
        for movie in self.planes.movies.values_mut() {
            movie.player.queue_host_pause(paused, responses, first);
        }
        for playback in self.active.values() {
            let (sender, receiver) = mpsc::sync_channel(1);
            if playback
                .commands
                .send(MovieCommand::HostPause(sender))
                .is_ok()
            {
                responses.push(receiver);
            } else {
                first.get_or_insert_with(|| error("movie worker stopped during host pause"));
            }
        }
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Gain is applied to PCM before device submission. Zero gain still
    /// consumes samples and advances the real device clock.
    pub fn set_volume(&mut self, percent: i32) {
        self.volume
            .store(percent.clamp(0, 100) as u32, Ordering::Release);
    }

    pub fn volume(&self) -> u32 {
        self.volume.load(Ordering::Acquire)
    }

    pub fn set_display_rectangle(&mut self, rectangle: [i32; 4]) -> RuntimeResult<()> {
        *self
            .surface
            .rectangle
            .lock()
            .map_err(|_| error("movie display rectangle was poisoned"))? = Some(rectangle);
        Ok(())
    }

    pub fn set_game_drawing(&mut self, enabled: bool) {
        self.surface.game_drawing.store(enabled, Ordering::Release);
        self.surface.visible.store(!enabled, Ordering::Release);
    }

    pub fn duration_millis(&self) -> RuntimeResult<u32> {
        let seconds = self
            .loaded
            .as_ref()
            .and_then(|movie| movie.metadata.duration_seconds)
            .filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
            .ok_or_else(|| error("movie duration is unavailable from container metadata"))?;
        let millis = (seconds * 1000.0).trunc();
        if millis >= u64::MAX as f64 {
            return Err(error(
                "movie duration exceeds the supported DWORD conversion range",
            ));
        }
        Ok(millis as u64 as u32)
    }

    /// Native 270 targets this host's current desktop window/surface.
    pub fn show_on_window(&self) {
        self.surface.visible.store(
            !self.surface.game_drawing.load(Ordering::Acquire),
            Ordering::Release,
        );
    }

    pub fn stop_preserving_frame(&mut self, handle: u64) -> RuntimeResult<()> {
        let time = self.presentation_time(handle)?;
        let frame = self
            .surface
            .retained_frame()?
            .filter(|(owner, _)| *owner == handle);
        let result = self.release(handle);
        if let Some((owner, frame)) = frame {
            self.surface.present(owner, frame)?;
            self.show_on_window();
        }
        self.completed.insert(
            handle,
            Completion {
                result: result.as_ref().map(|_| ()).map_err(ToString::to_string),
                last_time: time,
            },
        );
        result
    }

    pub fn frame_surface(&self) -> Arc<MovieFrameSurface> {
        self.surface.clone()
    }

    pub fn decoder_path(&self) -> &Path {
        &self.ffmpeg
    }

    pub fn native_android_decoder_available() -> bool {
        #[cfg(target_os = "android")]
        {
            crate::movie_android::available()
        }
        #[cfg(not(target_os = "android"))]
        {
            false
        }
    }

    pub fn probe_path(&self) -> PathBuf {
        probe_path(&self.ffmpeg)
    }

    pub fn loaded_metadata(&self) -> Option<(&str, &MovieMetadata)> {
        self.loaded
            .as_ref()
            .map(|movie| (movie.resource.as_str(), &movie.metadata))
    }

    pub fn loaded_has_audio(&self) -> Option<bool> {
        self.loaded.as_ref().map(|movie| movie.has_audio)
    }

    pub fn loaded_format(&self) -> Option<MovieFormat> {
        self.loaded.as_ref().map(|movie| movie.format)
    }

    /// Poll from the formal event loop even when no script asks `finished`.
    pub fn poll(&mut self) -> RuntimeResult<()> {
        for movie in self.planes.movies.values_mut() {
            movie.player.poll()?;
        }
        let completed = self
            .active
            .iter()
            .filter_map(|(&handle, playback)| match playback.finished.try_recv() {
                Ok(result) => Some((handle, result)),
                Err(mpsc::TryRecvError::Disconnected) => Some((
                    handle,
                    Err(error("movie worker exited without a completion result")),
                )),
                Err(mpsc::TryRecvError::Empty) => None,
            })
            .collect::<Vec<_>>();
        let mut first_error = None;
        for (handle, result) in completed {
            let last_time = self.presentation_time(handle)?;
            self.active.remove(&handle); // Drop joins and cancels any decoder.
            if let Err(cause) = &result {
                first_error.get_or_insert_with(|| cause.to_string());
            }
            self.completed.insert(
                handle,
                Completion {
                    result: result.map_err(|cause| cause.to_string()),
                    last_time,
                },
            );
        }
        first_error.map_or(Ok(()), |cause| Err(error(cause)))
    }

    pub fn release(&mut self, handle: u64) -> RuntimeResult<()> {
        let current_surface = self
            .surface
            .retained_frame()?
            .is_some_and(|(owner, _)| owner == handle);
        let mut result = Ok(());
        if let Some(mut playback) = self.active.remove(&handle) {
            let mut completion = playback.finished.try_recv().ok();
            if completion.is_none() {
                let _ = playback.commands.send(MovieCommand::Stop);
                completion = match playback.finished.recv_timeout(CONTROL_TIMEOUT) {
                    Ok(completion) => Some(completion),
                    Err(cause) => {
                        playback.decoders.cancel();
                        Some(Err(error(format!("movie stop did not complete: {cause}"))))
                    }
                };
            }
            if let Some(worker) = playback.thread.take()
                && worker.join().is_err()
            {
                result = Err(error("movie worker panicked during release"));
            }
            // In particular, a device reset/close failure is not hidden by
            // an intentional stop request.
            if let Some(completion) = completion {
                result = result.and(completion);
            }
        }
        if let Some(completed) = self.completed.remove(&handle) {
            result = result.and(completed.result.map_err(error));
        }
        let surface_result = self.surface.clear(handle);
        if self
            .planes
            .exclusive_snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.playback == handle)
        {
            self.planes.exclusive_snapshot = None;
        }
        if current_surface {
            self.surface.visible.store(false, Ordering::Release);
        }
        result.and(surface_result)
    }

    pub fn unload(&mut self) -> RuntimeResult<()> {
        let handles = self
            .active
            .keys()
            .chain(self.completed.keys())
            .copied()
            .collect::<Vec<_>>();
        let mut first = None;
        for handle in handles {
            if let Err(cause) = self.release(handle) {
                first.get_or_insert(cause);
            }
        }
        self.loaded = None;
        self.planes.exclusive_snapshot = None;
        self.surface.visible.store(false, Ordering::Release);
        self.surface.game_drawing.store(false, Ordering::Release);
        *self
            .surface
            .rectangle
            .lock()
            .map_err(|_| error("movie display rectangle was poisoned"))? = None;
        self.volume.store(100, Ordering::Release);
        first.map_or(Ok(()), Err)
    }

    pub(super) fn fork_movie_player(&self) -> Self {
        let resolver = self.resolve.clone();
        let mut player =
            Self::with_reader_and_ffmpeg(move |name| resolver(name), self.ffmpeg.clone());
        player.audio = self.audio.clone();
        player
            .host_paused
            .store(self.host_paused.load(Ordering::Acquire), Ordering::Release);
        #[cfg(target_os = "android")]
        {
            player.android_resources = self.android_resources.clone();
        }
        player
    }

    fn presentation_time(&self, handle: u64) -> RuntimeResult<Option<f64>> {
        if let Some(time) = self.surface.presented_time(handle)? {
            return Ok(Some(time));
        }
        Ok(self
            .active
            .get(&handle)
            .and_then(|playback| {
                let value = f64::from_bits(playback.last_time.load(Ordering::Acquire));
                value.is_finite().then_some(value)
            })
            .or_else(|| {
                self.completed
                    .get(&handle)
                    .and_then(|completion| completion.last_time)
            }))
    }
}

impl MpegMovieHost {
    pub fn play_count(
        &mut self,
        request: MovieRequest,
        count: u32,
    ) -> RuntimeResult<PlaybackState> {
        self.poll()?;
        if self
            .loaded
            .as_ref()
            .is_none_or(|movie| movie.resource != request.resource)
        {
            match self.load(request.clone())? {
                MovieLoadResult::Ready(_) => {}
                MovieLoadResult::Missing => {
                    return Err(RuntimeError::ResourceNotFound(request.resource));
                }
                MovieLoadResult::Disabled => {
                    return Err(error("movie has no supported video dimensions"));
                }
            }
        }
        // This surface is one movie plane. Replace the previous playback
        // explicitly instead of allowing two decoders to race to overwrite it.
        let handles = self.active.keys().copied().collect::<Vec<_>>();
        for handle in handles {
            self.release(handle)?;
        }
        let movie = self.loaded.clone().expect("loaded above");
        self.show_on_window();
        let handle = self.next_handle;
        self.next_handle = self
            .next_handle
            .checked_add(1)
            .ok_or_else(|| error("movie handle space exhausted"))?;
        let decoders = Arc::new(DecoderGroup::default());
        let worker_decoders = decoders.clone();
        let (commands, command_receiver) = mpsc::channel();
        let (started_sender, started_receiver) = mpsc::sync_channel(1);
        let (finished_sender, finished) = mpsc::channel();
        let last_time = Arc::new(AtomicU64::new(f64::NAN.to_bits()));
        let worker_last_time = last_time.clone();
        let ffmpeg = self.ffmpeg.clone();
        let surface = self.surface.clone();
        let presenter = self.presenter.clone();
        let audio = self.audio.clone();
        let volume = self.volume.clone();
        let host_paused = self.host_paused.clone();
        let looped = count == 0 || count == u32::MAX;
        let muted = request.muted;
        let worker = thread::Builder::new()
            .name(format!("cs2-movie-{handle}"))
            .spawn(move || {
                let mut started = Some(started_sender);
                let result = (|| {
                    let mut remaining = count;
                    loop {
                        let keep_playing = run_iteration(
                            handle,
                            &movie,
                            &ffmpeg,
                            muted,
                            &audio,
                            &volume,
                            &host_paused,
                            &command_receiver,
                            &worker_decoders,
                            &surface,
                            presenter.as_deref(),
                            &worker_last_time,
                            &mut started,
                        )?;
                        if !keep_playing {
                            return Ok(());
                        }
                        if !looped {
                            remaining = remaining.saturating_sub(1);
                            if remaining == 0 {
                                return Ok(());
                            }
                        }
                    }
                })();
                if let Some(started) = started {
                    let message = result
                        .as_ref()
                        .err()
                        .map(ToString::to_string)
                        .unwrap_or_else(|| "movie ended before decoding its first frame".into());
                    let _ = started.send(Err(error(message)));
                }
                let _ = finished_sender.send(result);
            })
            .map_err(|cause| error(format!("start movie worker: {cause}")))?;
        let playback = ActiveMovie {
            commands,
            decoders,
            finished,
            last_time,
            thread: Some(worker),
        };
        match started_receiver.recv_timeout(START_TIMEOUT) {
            Ok(Ok(())) => {
                self.active.insert(handle, playback);
                self.show_on_window();
                Ok(PlaybackState {
                    handle,
                    resource: request.resource,
                    looped,
                    paused: false,
                })
            }
            Ok(Err(cause)) => {
                drop(playback);
                self.surface.clear(handle)?;
                Err(cause)
            }
            Err(cause) => {
                drop(playback);
                self.surface.clear(handle)?;
                Err(error(format!(
                    "movie did not decode/publish its first frame: {cause}"
                )))
            }
        }
    }
}

impl Drop for MpegMovieHost {
    fn drop(&mut self) {
        let _ = self.unload();
    }
}

impl MovieHost for MpegMovieHost {
    fn load(&mut self, request: MovieRequest) -> RuntimeResult<MovieLoadResult> {
        // Never reuse the previous asset after a failed load.
        self.loaded = None;
        if !self.enabled {
            return Ok(MovieLoadResult::Disabled);
        }
        #[cfg(not(target_os = "android"))]
        let reader = match (self.resolve)(&request.resource) {
            Ok(reader) => reader,
            Err(RuntimeError::ResourceNotFound(_)) => return Ok(MovieLoadResult::Missing),
            Err(cause) => return Err(cause),
        };
        #[cfg(not(target_os = "android"))]
        let input = MediaInput::from_reader(reader)?;
        #[cfg(not(target_os = "android"))]
        let format = MovieFormat::detect(&input.prefix()?)?;
        #[cfg(not(target_os = "android"))]
        let (metadata, has_audio) = probe_metadata(input.clone(), format, &self.ffmpeg)?;
        #[cfg(target_os = "android")]
        let format = match crate::movie_android::signature(&self.resolve, &request.resource) {
            Ok(prefix) => MovieFormat::detect(&prefix)?,
            Err(RuntimeError::ResourceNotFound(_)) => return Ok(MovieLoadResult::Missing),
            Err(cause) => return Err(cause),
        };
        #[cfg(target_os = "android")]
        let source_length = match &self.android_resources {
            Some(resources) => resources.resolve_mode_utf16(
                &request.resource.encode_utf16().collect::<Vec<_>>(), ResourceMode::LooseFirst,
            )?.map(|candidate| candidate.value.size),
            None => None,
        };
        #[cfg(target_os = "android")]
        let (metadata, has_audio) = crate::movie_android::probe(
            self.resolve.clone(),
            &request.resource,
            source_length,
            format,
        )?;
        let (Some(width), Some(height)) = (metadata.width, metadata.height) else {
            return Err(error(
                "movie metadata probe has no supported frame dimensions",
            ));
        };
        if width == 0 || height == 0 || u64::from(width) * u64::from(height) > 16_777_216 {
            return Err(error(
                "movie dimensions exceed the 64 MiB decoded frame limit",
            ));
        }
        self.loaded = Some(LoadedMovie {
            resource: request.resource,
            #[cfg(not(target_os = "android"))]
            input,
            #[cfg(target_os = "android")]
            resolve: self.resolve.clone(),
            #[cfg(target_os = "android")]
            length: source_length,
            format,
            metadata: metadata.clone(),
            width: width.into(),
            height: height.into(),
            has_audio,
        });
        Ok(MovieLoadResult::Ready(metadata))
    }

    fn play(&mut self, request: MovieRequest) -> RuntimeResult<PlaybackState> {
        let count = if request.looped { 0 } else { 1 };
        self.play_count(request, count)
    }

    fn stop(&mut self, handle: u64) -> RuntimeResult<()> {
        self.stop_preserving_frame(handle)
    }

    fn pause(&mut self, handle: u64, paused: bool) -> RuntimeResult<()> {
        self.poll()?;
        let playback = self
            .active
            .get(&handle)
            .ok_or_else(|| error(format!("movie handle {handle} is not active")))?;
        let (acknowledge, response) = mpsc::sync_channel(1);
        playback
            .commands
            .send(MovieCommand::Pause(paused, acknowledge))
            .map_err(|_| error("movie worker stopped before pause"))?;
        response
            .recv_timeout(CONTROL_TIMEOUT)
            .map_err(|cause| error(format!("movie pause was not acknowledged: {cause}")))?
    }

    fn finished(&mut self, handle: u64) -> RuntimeResult<bool> {
        self.poll()?;
        if let Some(completion) = self.completed.get(&handle) {
            completion.result.as_ref().map_err(error)?;
        }
        Ok(!self.active.contains_key(&handle))
    }

    fn last_frame_time(&mut self, handle: u64) -> RuntimeResult<Option<f64>> {
        self.poll()?;
        self.presentation_time(handle)
    }
}

/// Locate configured/PATH/winget FFmpeg without executing it or guessing a game DLL.
pub fn discover_ffmpeg() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("CS2_FFMPEG").map(PathBuf::from) {
        // A bad explicit setting must fail at spawn; do not silently override it.
        return Some(path);
    }
    let filename = if cfg!(windows) {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    };
    if let Some(paths) = std::env::var_os("PATH") {
        for directory in std::env::split_paths(&paths) {
            let candidate = directory.join(filename);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    #[cfg(windows)]
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        let local = PathBuf::from(local);
        let installed = local.join("Programs/CS2MediaTools/ffmpeg/bin/ffmpeg.exe");
        if installed.is_file() {
            return Some(installed);
        }
        let winget = local.join("Microsoft/WinGet");
        let linked = winget.join("Links/ffmpeg.exe");
        if linked.is_file() {
            return Some(linked);
        }
        let mut packages = std::fs::read_dir(winget.join("Packages"))
            .ok()?
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("Gyan.FFmpeg_")
            })
            .map(|entry| entry.path())
            .collect::<Vec<_>>();
        packages.sort();
        for package in packages.into_iter().rev() {
            let Ok(children) = std::fs::read_dir(package) else {
                continue;
            };
            for child in children.filter_map(Result::ok) {
                let candidate = child.path().join("bin/ffmpeg.exe");
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    None
}

pub fn discover_ffprobe() -> Option<PathBuf> {
    let decoder = discover_ffmpeg().unwrap_or_else(|| PathBuf::from("ffmpeg"));
    let probe = probe_path(&decoder);
    probe.is_file().then_some(probe)
}

struct DecodedStream<T> {
    packets: mpsc::Receiver<RuntimeResult<Option<T>>>,
    cancellation: Cancellation,
    worker: Option<JoinHandle<()>>,
}

// Decoder timing belongs to the internal scheduler. Keep public MovieFrame
// unchanged for both native presenters and external callback consumers.
struct DecodedVideo {
    frame: Arc<MovieFrame>,
    duration: Option<f64>,
}

impl<T> Drop for DecodedStream<T> {
    fn drop(&mut self) {
        self.cancellation.cancel();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(target_os = "android")]
fn video_stream(
    movie: &LoadedMovie,
    _ffmpeg: &Path,
    group: &DecoderGroup,
) -> RuntimeResult<DecodedStream<DecodedVideo>> {
    let cancellation = Cancellation::in_process();
    group.register(cancellation.clone())?;
    let cancel = cancellation.clone();
    let (sender, packets) = mpsc::sync_channel(2);
    let movie = movie.clone();
    let worker = thread::Builder::new()
        .name("cs2-android-video".into())
        .spawn(move || {
            let result = (|| {
                let (mut decoder, _, _) = crate::movie_android::Decoder::open(
                    movie.resolve.clone(),
                    &movie.resource,
                    movie.length,
                    movie.format.demuxer(),
                    1,
                    cancel.clone(),
                )?;
                let mut count = 0_u64;
                let mut previous = 0.;
                while let Some(frame) = decoder.next()? {
                    match frame {
                        crate::movie_android::Frame::Video {
                            width,
                            height,
                            time,
                            duration,
                            rgba,
                        } => {
                            let time = time.max(0.);
                            if count != 0 && time < previous {
                                return Err(error(
                                    "Android decoded video timestamps went backwards",
                                ));
                            }
                            previous = time;
                            count += 1;
                            if !send_bounded(
                                &sender,
                                Ok(Some(DecodedVideo {
                                    frame: Arc::new(MovieFrame {
                                        width,
                                        height,
                                        presentation_time: time,
                                        rgba: rgba.into(),
                                    }),
                                    duration,
                                })),
                                &cancel,
                            ) {
                                return Ok(());
                            }
                        }
                        _ => {
                            return Err(error(
                                "Android video-only decoder returned a non-video event",
                            ));
                        }
                    }
                }
                if count == 0 {
                    return Err(error("Android FFmpeg decoded no video frames"));
                }
                let _ = send_bounded(&sender, Ok(None), &cancel);
                Ok(())
            })();
            if !cancel.cancelled()
                && let Err(cause) = result
            {
                let _ = send_bounded(&sender, Err(cause), &cancel);
            }
        })
        .map_err(|cause| error(format!("start Android video reader: {cause}")))?;
    Ok(DecodedStream {
        packets,
        cancellation,
        worker: Some(worker),
    })
}

#[cfg(target_os = "android")]
fn audio_stream(
    movie: &LoadedMovie,
    _ffmpeg: &Path,
    group: &DecoderGroup,
) -> RuntimeResult<DecodedStream<Vec<i16>>> {
    let cancellation = Cancellation::in_process();
    group.register(cancellation.clone())?;
    let cancel = cancellation.clone();
    let (sender, packets) = mpsc::sync_channel(4);
    let movie = movie.clone();
    let worker = thread::Builder::new()
        .name("cs2-android-movie-audio".into())
        .spawn(move || {
            let result = (|| {
                let (mut decoder, _, _) = crate::movie_android::Decoder::open(
                    movie.resolve.clone(),
                    &movie.resource,
                    movie.length,
                    movie.format.demuxer(),
                    2,
                    cancel.clone(),
                )?;
                let mut output_frames = 0_u64;
                let mut decoded_frames = 0_u64;
                while let Some(frame) = decoder.next()? {
                    let crate::movie_android::Frame::Audio { time, samples } = frame else {
                        return Err(error(
                            "Android audio-only decoder returned a non-audio event",
                        ));
                    };
                    let target = (time * 48_000.).round();
                    if target <= i64::MIN as f64 || target >= i64::MAX as f64 {
                        return Err(error("Android audio timestamp exceeds its frame clock"));
                    }
                    let target = target as i64;
                    let trim = if target < 0 {
                        target.unsigned_abs()
                    } else {
                        output_frames.saturating_sub(target as u64)
                    }
                    .min((samples.len() / 2) as u64) as usize
                        * 2;
                    let target = target.max(0) as u64;
                    while output_frames < target {
                        let frames = (target - output_frames).min(480);
                        if !send_bounded(&sender, Ok(Some(vec![0; frames as usize * 2])), &cancel) {
                            return Ok(());
                        }
                        output_frames += frames;
                    }
                    decoded_frames += (samples.len() / 2) as u64;
                    for chunk in samples[trim..].chunks(960) {
                        if !send_bounded(&sender, Ok(Some(chunk.to_vec())), &cancel) {
                            return Ok(());
                        }
                        output_frames += (chunk.len() / 2) as u64;
                    }
                }
                if decoded_frames == 0 {
                    return Err(error("Android movie audio track decoded no samples"));
                }
                let _ = send_bounded(&sender, Ok(None), &cancel);
                Ok(())
            })();
            if !cancel.cancelled()
                && let Err(cause) = result
            {
                let _ = send_bounded(&sender, Err(cause), &cancel);
            }
        })
        .map_err(|cause| error(format!("start Android movie audio reader: {cause}")))?;
    Ok(DecodedStream {
        packets,
        cancellation,
        worker: Some(worker),
    })
}

#[cfg(not(target_os = "android"))]
fn video_stream(
    movie: &LoadedMovie,
    ffmpeg: &Path,
    group: &DecoderGroup,
) -> RuntimeResult<DecodedStream<DecodedVideo>> {
    let arguments = [
        "-map".into(),
        "0:v:0".into(),
        "-an".into(),
        "-sn".into(),
        "-dn".into(),
        "-vf".into(),
        format!(
            "scale={}:{},format=rgba,showinfo",
            movie.width, movie.height
        ),
        "-fps_mode".into(),
        "passthrough".into(),
        "-f".into(),
        "rawvideo".into(),
        "-pix_fmt".into(),
        "rgba".into(),
        "pipe:1".into(),
    ];
    let (mut process, mut stdout) = DecoderProcess::spawn(
        ffmpeg,
        movie.input.clone(),
        movie.format.demuxer(),
        &arguments,
        true,
    )?;
    let cancellation = process.cancellation.clone();
    group.register(cancellation.clone())?;
    let cancel = cancellation.clone();
    let (sender, packets) = mpsc::sync_channel(2);
    let (width, height) = (movie.width, movie.height);
    let worker = thread::Builder::new()
        .name("cs2-movie-video".into())
        .spawn(move || {
            let result =
                (|| {
                    let frame_length = width as usize * height as usize * 4;
                    let mut frames = 0_u64;
                    let mut previous = 0.0;
                    loop {
                        let mut rgba = vec![0; frame_length];
                        if !read_packet(&mut stdout, &mut rgba)? {
                            break;
                        }
                        let timing = process.timestamps.recv_timeout(CONTROL_TIMEOUT).map_err(
                            |cause| {
                                error(format!(
                                    "missing decoded frame PTS: {cause}; {}",
                                    process.diagnostic()
                                ))
                            },
                        )??;
                        let time = timing.time;
                        if frames > 0 && time < previous {
                            return Err(error("decoded movie timestamps went backwards"));
                        }
                        previous = time;
                        frames += 1;
                        if !send_bounded(
                            &sender,
                            Ok(Some(DecodedVideo {
                                frame: Arc::new(MovieFrame {
                                    width,
                                    height,
                                    presentation_time: time,
                                    rgba: rgba.into(),
                                }),
                                duration: timing.duration,
                            })),
                            &cancel,
                        ) {
                            return Ok(());
                        }
                    }
                    process.finish()?;
                    if frames == 0 {
                        return Err(error("FFmpeg decoded no video frames"));
                    }
                    let _ = send_bounded(&sender, Ok(None), &cancel);
                    Ok(())
                })();
            if !cancel.cancelled()
                && let Err(cause) = result
            {
                let _ = send_bounded(
                    &sender,
                    Err(error(format!("{cause}; {}", process.diagnostic()))),
                    &cancel,
                );
            }
        })
        .map_err(|cause| error(format!("start decoded video reader: {cause}")))?;
    Ok(DecodedStream {
        packets,
        cancellation,
        worker: Some(worker),
    })
}

#[cfg(not(target_os = "android"))]
fn audio_stream(
    movie: &LoadedMovie,
    ffmpeg: &Path,
    group: &DecoderGroup,
) -> RuntimeResult<DecodedStream<Vec<i16>>> {
    let arguments = [
        "-map",
        "0:a:0",
        "-vn",
        "-sn",
        "-dn",
        "-af",
        "aresample=48000:async=1:first_pts=0",
        "-ac",
        "2",
        "-ar",
        "48000",
        "-f",
        "s16le",
        "-acodec",
        "pcm_s16le",
        "pipe:1",
    ]
    .map(str::to_owned);
    let (mut process, mut stdout) = DecoderProcess::spawn(
        ffmpeg,
        movie.input.clone(),
        movie.format.demuxer(),
        &arguments,
        false,
    )?;
    let cancellation = process.cancellation.clone();
    group.register(cancellation.clone())?;
    let cancel = cancellation.clone();
    let (sender, packets) = mpsc::sync_channel(4);
    let worker = thread::Builder::new()
        .name("cs2-movie-audio".into())
        .spawn(move || {
            let result = (|| {
                let mut total = 0;
                loop {
                    let mut pcm = vec![0_u8; 1920]; // 10 ms, bounded decoder/device queues
                    let mut filled = 0;
                    while filled < pcm.len() {
                        match stdout.read(&mut pcm[filled..]) {
                            Ok(0) => break,
                            Ok(count) => filled += count,
                            Err(cause) if cause.kind() == std::io::ErrorKind::Interrupted => {}
                            Err(cause) => return Err(error(format!("read movie PCM: {cause}"))),
                        }
                    }
                    if filled == 0 {
                        break;
                    }
                    if filled % 4 != 0 {
                        return Err(error("decoded movie PCM ended inside a stereo frame"));
                    }
                    total += filled;
                    let samples = pcm[..filled]
                        .chunks_exact(2)
                        .map(|sample| i16::from_le_bytes([sample[0], sample[1]]))
                        .collect();
                    if !send_bounded(&sender, Ok(Some(samples)), &cancel) {
                        return Ok(());
                    }
                }
                process.finish()?;
                if total == 0 {
                    return Err(error("movie audio track decoded no samples"));
                }
                let _ = send_bounded(&sender, Ok(None), &cancel);
                Ok(())
            })();
            if !cancel.cancelled()
                && let Err(cause) = result
            {
                let _ = send_bounded(
                    &sender,
                    Err(error(format!("{cause}; {}", process.diagnostic()))),
                    &cancel,
                );
            }
        })
        .map_err(|cause| error(format!("start decoded audio reader: {cause}")))?;
    Ok(DecodedStream {
        packets,
        cancellation,
        worker: Some(worker),
    })
}

#[cfg(not(target_os = "android"))]
fn probe_metadata(
    input: Arc<MediaInput>,
    format: MovieFormat,
    ffmpeg: &Path,
) -> RuntimeResult<(MovieMetadata, bool)> {
    let ffprobe = probe_path(ffmpeg);
    let (mut process, stdout) = DecoderProcess::probe_streams(&ffprobe, input, format.demuxer())?;
    let _deadline = DecoderDeadline::arm(process.cancellation.clone(), START_TIMEOUT)?;
    let mut text = String::new();
    stdout
        .take(65_537)
        .read_to_string(&mut text)
        .map_err(|cause| error(format!("read movie metadata: {cause}")))?;
    if text.len() > 65_536 {
        return Err(error("movie stream inventory exceeds 64 KiB"));
    }
    process.finish()?;
    parse_metadata(&text)
}

fn probe_path(ffmpeg: &Path) -> PathBuf {
    std::env::var_os("CS2_FFPROBE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let filename = if cfg!(windows) {
                "ffprobe.exe"
            } else {
                "ffprobe"
            };
            if ffmpeg.components().count() > 1 {
                let sibling = ffmpeg.with_file_name(filename);
                if sibling.is_file() {
                    return sibling;
                }
            }
            if let Some(paths) = std::env::var_os("PATH") {
                for directory in std::env::split_paths(&paths) {
                    let candidate = directory.join(filename);
                    if candidate.is_file() {
                        return candidate;
                    }
                }
            }
            PathBuf::from(filename)
        })
}

fn parse_metadata(text: &str) -> RuntimeResult<(MovieMetadata, bool)> {
    let mut metadata = MovieMetadata::default();
    let mut fields = HashMap::new();
    let mut section = "";
    let mut has_audio = false;
    let mut video_found = false;
    let mut container_duration = None;
    for line in text.lines().map(str::trim) {
        match line {
            "[STREAM]" => {
                fields.clear();
                section = "stream";
            }
            "[/STREAM]" => {
                match fields.get("codec_type").copied() {
                    Some("audio") => has_audio = true,
                    Some("video") if !video_found => {
                        video_found = true;
                        let width = fields.get("width").and_then(|value| value.parse().ok());
                        let height = fields.get("height").and_then(|value| value.parse().ok());
                        if let (Some(width), Some(height)) = (width, height) {
                            let size = movie_display_size(
                                width,
                                height,
                                fields.get("sample_aspect_ratio").copied(),
                                fields.get("rotation").copied(),
                            )?;
                            metadata.width = Some(size.0);
                            metadata.height = Some(size.1);
                        }
                        metadata.time_base = fields
                            .get("avg_frame_rate")
                            .and_then(|value| parse_rate(value))
                            .or_else(|| {
                                fields
                                    .get("r_frame_rate")
                                    .and_then(|value| parse_rate(value))
                            });
                        // Native998 asks for video duration. A longer audio
                        // stream must not replace a valid video-stream value.
                        metadata.duration_seconds = fields
                            .get("duration")
                            .and_then(|value| parse_seconds(value))
                            .or_else(|| {
                                let ticks = fields.get("duration_ts")?.parse::<u64>().ok()?;
                                let (numerator, denominator) =
                                    fields.get("time_base")?.split_once('/')?;
                                let numerator = numerator.parse::<u64>().ok()?;
                                let denominator = denominator.parse::<u64>().ok()?;
                                (denominator != 0)
                                    .then_some(ticks as f64 * numerator as f64 / denominator as f64)
                            })
                            .or_else(|| {
                                fields
                                    .get("TAG:DURATION")
                                    .and_then(|value| parse_clock_duration(value))
                            })
                            .or_else(|| {
                                let frames = fields.get("nb_frames")?.parse::<u64>().ok()?;
                                let base = metadata.time_base?;
                                Some(
                                    frames as f64 * f64::from(base.numerator)
                                        / f64::from(base.denominator),
                                )
                            })
                            .filter(|seconds| seconds.is_finite() && *seconds >= 0.);
                    }
                    _ => {}
                }
                section = "";
            }
            "[FORMAT]" => section = "format",
            "[/FORMAT]" => section = "",
            "[SIDE_DATA]" => {}
            "[/SIDE_DATA]" => {}
            _ => {
                if let Some((key, value)) = line.split_once('=') {
                    if section == "stream" {
                        fields.insert(key, value);
                    }
                    if section == "format" && key == "duration" {
                        container_duration = parse_seconds(value);
                    }
                }
            }
        }
    }
    if !video_found {
        return Err(error("ffprobe found no video stream"));
    }
    // Formats without a per-stream duration can only supply a container
    // estimate. Keep it as fallback, never override a video-specific value.
    metadata.duration_seconds = metadata.duration_seconds.or(container_duration);
    Ok((metadata, has_audio))
}

fn parse_seconds(value: &str) -> Option<f64> {
    value
        .parse::<f64>()
        .ok()
        .filter(|seconds| seconds.is_finite() && *seconds >= 0.)
}

fn movie_display_size(
    width: u32,
    height: u32,
    aspect: Option<&str>,
    rotation: Option<&str>,
) -> RuntimeResult<(u16, u16)> {
    let ratio = aspect
        .and_then(|text| text.split_once(':'))
        .and_then(|(numerator, denominator)| {
            let numerator = numerator.parse::<u32>().ok()?;
            let denominator = denominator.parse::<u32>().ok()?;
            (numerator != 0 && denominator != 0)
                .then_some(f64::from(numerator) / f64::from(denominator))
        })
        .unwrap_or(1.);
    let rotation = rotation
        .map(|text| {
            text.parse::<f64>()
                .map_err(|_| error("movie display rotation is not numeric"))
        })
        .transpose()?
        .unwrap_or(0.);
    if !rotation.is_finite() {
        return Err(error("movie display rotation is not finite"));
    }
    // FFmpeg's input autorotation supplies the pixels; output scale must use
    // the corresponding display boundary, not squeeze it back to encoded SAR.
    let radians = rotation.to_radians();
    let display_width = f64::from(width) * ratio;
    let display_height = f64::from(height);
    let output_width =
        (display_width * radians.cos().abs() + display_height * radians.sin().abs() - 1e-9).ceil();
    let output_height =
        (display_width * radians.sin().abs() + display_height * radians.cos().abs() - 1e-9).ceil();
    if !output_width.is_finite()
        || !output_height.is_finite()
        || output_width < 1.
        || output_height < 1.
        || output_width > f64::from(u16::MAX)
        || output_height > f64::from(u16::MAX)
        || output_width * output_height > 16_777_216.
    {
        return Err(error(
            "movie display dimensions exceed the decoded frame limit",
        ));
    }
    Ok((output_width as u16, output_height as u16))
}

fn parse_clock_duration(value: &str) -> Option<f64> {
    let mut parts = value.split(':');
    let hours = parts.next()?.parse::<u64>().ok()?;
    let minutes = parts.next()?.parse::<u64>().ok()?;
    let seconds = parse_seconds(parts.next()?)?;
    if parts.next().is_some() || minutes >= 60 || seconds >= 60. {
        return None;
    }
    Some(hours as f64 * 3600. + minutes as f64 * 60. + seconds)
}

fn parse_rate(value: &str) -> Option<MovieTimeBase> {
    let (frames, seconds) = value.split_once('/')?;
    let frames = frames.parse::<u32>().ok()?;
    let seconds = seconds.parse::<u32>().ok()?;
    if frames == 0 || seconds == 0 {
        return None;
    }
    Some(MovieTimeBase {
        numerator: seconds,
        denominator: frames,
    })
}

fn read_packet(reader: &mut impl Read, packet: &mut [u8]) -> RuntimeResult<bool> {
    let mut offset = 0;
    while offset < packet.len() {
        match reader.read(&mut packet[offset..]) {
            Ok(0) if offset == 0 => return Ok(false),
            Ok(0) => return Err(error("FFmpeg ended inside a decoded video frame")),
            Ok(count) => offset += count,
            Err(cause) if cause.kind() == std::io::ErrorKind::Interrupted => {}
            Err(cause) => return Err(error(format!("read decoded movie frame: {cause}"))),
        }
    }
    Ok(true)
}

struct WallClock {
    origin: Instant,
    paused_at: Option<Instant>,
}

impl WallClock {
    fn new() -> Self {
        Self {
            origin: Instant::now(),
            paused_at: None,
        }
    }
    fn seconds(&self) -> f64 {
        self.paused_at
            .unwrap_or_else(Instant::now)
            .duration_since(self.origin)
            .as_secs_f64()
    }
    fn pause(&mut self, value: bool) {
        match (value, self.paused_at) {
            (true, None) => self.paused_at = Some(Instant::now()),
            (false, Some(start)) => {
                self.origin += start.elapsed();
                self.paused_at = None;
            }
            _ => {}
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn run_iteration(
    handle: u64,
    movie: &LoadedMovie,
    ffmpeg: &Path,
    muted: bool,
    audio_factory: &Arc<dyn MovieAudioFactory>,
    volume: &AtomicU32,
    host_paused: &AtomicBool,
    commands: &mpsc::Receiver<MovieCommand>,
    group: &DecoderGroup,
    surface: &MovieFrameSurface,
    presenter: Option<&dyn MovieFramePresenter>,
    last_time: &AtomicU64,
    started: &mut Option<mpsc::SyncSender<RuntimeResult<()>>>,
) -> RuntimeResult<bool> {
    let video = video_stream(movie, ffmpeg, group)?;
    let audio_stream = if movie.has_audio && !muted {
        Some(audio_stream(movie, ffmpeg, group)?)
    } else {
        None
    };
    let mut audio = if audio_stream.is_some() {
        Some(audio_factory.open()?)
    } else {
        None
    };
    if let Some(output) = audio.as_mut() {
        output.pause(true)?;
    }
    let mut clock = WallClock::new();
    clock.pause(true);
    let mut running = false;
    let mut paused = false;
    let mut next_video = None;
    let mut pending_pcm = None;
    let mut video_eof = false;
    let mut audio_eof = audio.is_none();
    let mut submitted_audio = false;
    let mut audio_tail = None;
    let mut last_video_time = None;
    let nominal_frame = movie
        .metadata
        .time_base
        .filter(|base| base.numerator != 0 && base.denominator != 0)
        .map(
            |MovieTimeBase {
                 numerator,
                 denominator,
             }| f64::from(numerator) / f64::from(denominator),
        )
        .unwrap_or(1.0 / 30.0);
    let mut last_interval = nominal_frame;
    loop {
        if group.cancelled.load(Ordering::Acquire) {
            if let Some(output) = audio.as_mut() {
                output.stop()?;
            }
            return Ok(false);
        }
        // Process control before inspecting potentially full decoder queues.
        while let Ok(command) = commands.try_recv() {
            match command {
                MovieCommand::Stop => {
                    if let Some(output) = audio.as_mut() {
                        output.stop()?;
                    }
                    return Ok(false);
                }
                MovieCommand::Pause(value, acknowledge) => {
                    let suspended = host_paused.load(Ordering::Acquire);
                    let result = if let Some(output) = audio.as_mut() {
                        output.pause(value || suspended || !running)
                    } else {
                        Ok(())
                    };
                    let _ = acknowledge.send(result.as_ref().map(|_| ()).map_err(error));
                    result?;
                    paused = value;
                    clock.pause(value || suspended || !running);
                }
                MovieCommand::HostPause(acknowledge) => {
                    let suspended = host_paused.load(Ordering::Acquire);
                    let result = if let Some(output) = audio.as_mut() {
                        output.pause(paused || suspended || !running)
                    } else {
                        Ok(())
                    };
                    clock.pause(paused || suspended || !running);
                    let _ = acknowledge.send(result.as_ref().map(|_| ()).map_err(error));
                    result?;
                }
            }
        }
        if next_video.is_none() && !video_eof {
            match video.packets.try_recv() {
                Ok(frame) => match frame? {
                    Some(frame) => next_video = Some(frame),
                    None => video_eof = true,
                },
                Err(mpsc::TryRecvError::Disconnected) => {
                    return Err(error("video reader exited without a validated EOF"));
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if pending_pcm.is_none()
            && !audio_eof
            && let Some(stream) = audio_stream.as_ref()
        {
            match stream.packets.try_recv() {
                Ok(samples) => match samples? {
                    Some(samples) => pending_pcm = Some(samples),
                    None => audio_eof = true,
                },
                Err(mpsc::TryRecvError::Disconnected) => {
                    return Err(error("audio reader exited without a validated EOF"));
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        let suspended = host_paused.load(Ordering::Acquire);
        if !running
            && suspended
            && let Some(frame) = next_video.as_ref()
        {
            surface.present(handle, frame.frame.clone())?;
            if let Some(started) = started.take() {
                let _ = started.send(Ok(()));
            }
        }
        if !paused
            && !suspended
            && let (Some(output), Some(samples)) = (audio.as_mut(), pending_pcm.as_ref())
        {
            let gain = volume.load(Ordering::Acquire).min(100) as i32;
            let adjusted = samples
                .iter()
                .map(|sample| (i32::from(*sample) * gain / 100) as i16)
                .collect::<Vec<_>>();
            if output.submit(&adjusted)? {
                submitted_audio = true;
                pending_pcm = None;
            }
        }
        if !running && next_video.is_some() && (audio.is_none() || submitted_audio) {
            running = true;
            clock = WallClock::new();
            clock.pause(paused || suspended);
            if let Some(output) = audio.as_mut() {
                output.pause(paused || suspended)?;
            }
        }
        let mut now = if let Some(output) = audio.as_mut() {
            output.position_seconds()?
        } else {
            clock.seconds()
        };
        // When a shorter audio track drains, continue video on a wall clock
        // anchored to the last real device position.
        if running
            && audio_eof
            && pending_pcm.is_none()
            && let Some(output) = audio.as_mut()
            && output.drained()?
        {
            let tail = audio_tail.get_or_insert_with(|| (now, WallClock::new()));
            tail.1.pause(paused || suspended);
            now = tail.0 + tail.1.seconds();
        }
        if running
            && !paused
            && !suspended
            && next_video
                .as_ref()
                .is_some_and(|video| video.frame.presentation_time <= now + 0.001)
        {
            let video = next_video.take().expect("checked above");
            let frame = video.frame;
            if let Some(previous) = last_video_time
                && frame.presentation_time > previous
            {
                last_interval = frame.presentation_time - previous;
            }
            if let Some(duration) = video.duration {
                last_interval = duration;
            }
            last_video_time = Some(frame.presentation_time);
            surface.present(handle, frame.clone())?;
            if let Some(presenter) = presenter {
                presenter.present(handle, frame.clone())?;
                last_time.store(frame.presentation_time.to_bits(), Ordering::Release);
            }
            if let Some(started) = started.take() {
                let _ = started.send(Ok(()));
            }
        }
        if video_eof
            && next_video.is_none()
            && audio_eof
            && pending_pcm.is_none()
            && !paused
            && !suspended
        {
            let drained = match audio.as_mut() {
                Some(output) => output.drained()?,
                None => true,
            };
            if drained && last_video_time.is_some_and(|time| now >= time + last_interval) {
                if let Some(output) = audio.as_mut() {
                    output.stop()?;
                }
                return Ok(true);
            }
        }
        thread::sleep(Duration::from_millis(2));
    }
}

use crate::host::{
    AudioChannel, AudioHost, AudioRequest, AudioState, InputEvent, InputState, MovieHost,
    MovieLoadResult, MovieRequest, PlaybackState,
};
use crate::scheduler::{ObjectHandle, ObjectHandleTable, TaskId, TaskScheduler};
use crate::{RuntimeError, RuntimeResult};
use cs2_formats::MovieMetadata;
use cs2_script::{Value as ScriptValue, Vm};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum RuntimeValue {
    U32(u32),
    I32(i32),
    F32(f32),
    Address(u32),
}

impl RuntimeValue {
    pub fn to_script_value(&self) -> ScriptValue {
        match self {
            Self::U32(value) => ScriptValue::U32(*value),
            Self::I32(value) => ScriptValue::I32(*value),
            Self::F32(value) => ScriptValue::F32(*value),
            Self::Address(value) => ScriptValue::Address(*value),
        }
    }

    pub fn from_script_value(value: &ScriptValue) -> Self {
        match value {
            ScriptValue::U32(value) => Self::U32(*value),
            ScriptValue::I32(value) => Self::I32(*value),
            ScriptValue::F32(value) => Self::F32(*value),
            ScriptValue::Address(value) => Self::Address(*value),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ScriptState {
    pub current_script: Option<String>,
    pub instruction_pointer: usize,
    pub stack: Vec<RuntimeValue>,
    pub stack_bits: Vec<u32>,
    pub stack_pointer: usize,
    pub frame_base: usize,
    pub frame_top: usize,
    pub frames: Vec<ScriptCallFrame>,
    pub program: Option<ScriptProgramState>,
    pub memory: Option<Vec<u8>>,
    /// Lossless execution/continuation state. Legacy saves retain None and
    /// cannot be resumed by inventing frame or shared-memory state.
    #[serde(default)]
    pub execution_group: Option<cs2_script::VmSnapshotGroup>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScriptCallFrame {
    pub return_pc: usize,
    pub base: usize,
    pub top: usize,
    pub parameter_bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScriptProgramState {
    pub code: Vec<u8>,
    pub data: Vec<u8>,
    pub stack_size: usize,
}

impl ScriptState {
    pub fn from_vm(vm: &Vm) -> Self {
        let program = vm.program().map(|program| ScriptProgramState {
            code: program.code.clone(),
            data: program.data.clone(),
            stack_size: program.stack_size,
        });
        let memory = vm.memory_snapshot();

        Self {
            current_script: None,
            instruction_pointer: vm.instruction_pointer,
            stack: vm
                .stack
                .iter()
                .map(RuntimeValue::from_script_value)
                .collect(),
            stack_bits: vm.stack_bits().collect(),
            stack_pointer: vm.stack_pointer(),
            frame_base: vm.frame_base(),
            frame_top: vm.frame_top(),
            frames: vm
                .frames()
                .iter()
                .map(|frame| ScriptCallFrame {
                    return_pc: frame.return_pc,
                    base: frame.base,
                    top: frame.top,
                    parameter_bytes: frame.parameter_bytes,
                })
                .collect(),
            program,
            memory,
            execution_group: None,
        }
    }

    pub fn capture_vm(vm: &Vm) -> RuntimeResult<Self> {
        let mut state = Self::from_vm(vm);
        state.execution_group = Some(cs2_script::VmSnapshotGroup::capture([(0, vm)])?);
        Ok(state)
    }

    pub fn restore_vm(&self) -> RuntimeResult<Vm> {
        let group = self.execution_group.as_ref().ok_or_else(|| RuntimeError::InvalidSave(
            "legacy script snapshot has no execution/continuation state".into(),
        ))?;
        let mut programs = group.restore()?;
        if programs.len() != 1 {
            return Err(RuntimeError::InvalidSave(
                "single-script snapshot contains a different VM group".into(),
            ));
        }
        programs.remove(&0).ok_or_else(|| RuntimeError::InvalidSave(
            "single-script snapshot is missing VM identity zero".into(),
        ))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct SceneState {
    pub current_script: Option<String>,
    pub current_label: Option<String>,
    pub return_script: Option<String>,
    pub line: u32,
}

impl SceneState {
    pub fn enter_script(&mut self, script: impl Into<String>, label: Option<String>) {
        self.current_script = Some(script.into());
        self.current_label = label;
        self.return_script = None;
        self.line = 0;
    }

    pub fn set_label(&mut self, label: impl Into<String>) {
        self.current_label = Some(label.into());
        self.line = 0;
    }

    pub fn advance_line(&mut self) {
        self.line = self.line.saturating_add(1);
    }

    pub fn return_to(&mut self, script: impl Into<String>, label: Option<String>) {
        self.return_script = self.current_script.take();
        self.current_script = Some(script.into());
        self.current_label = label;
        self.line = 0;
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct MovieState {
    #[serde(default)]
    pub current: Option<PlaybackState>,
    #[serde(default)]
    pub request: Option<MovieRequest>,
    #[serde(default)]
    pub resource_status: MovieResourceStatus,
    #[serde(default)]
    pub playback_status: MoviePlaybackStatus,
    #[serde(default)]
    pub metadata: Option<MovieMetadataState>,
    #[serde(default)]
    pub position_seconds: f64,
    #[serde(default)]
    pub last_frame_time: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct MovieMetadataState {
    pub video_stream_ids: Vec<u8>,
    pub audio_stream_ids: Vec<u8>,
    pub pes_start_offsets: Vec<usize>,
    pub width: Option<u16>,
    pub height: Option<u16>,
    pub time_base: Option<MovieTimeBaseState>,
    pub duration_seconds: Option<f64>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct MovieTimeBaseState {
    pub numerator: u32,
    pub denominator: u32,
}

impl From<MovieMetadata> for MovieMetadataState {
    fn from(metadata: MovieMetadata) -> Self {
        Self {
            video_stream_ids: metadata.video_stream_ids,
            audio_stream_ids: metadata.audio_stream_ids,
            pes_start_offsets: metadata.pes_start_offsets,
            width: metadata.width,
            height: metadata.height,
            time_base: metadata.time_base.map(|time_base| MovieTimeBaseState {
                numerator: time_base.numerator,
                denominator: time_base.denominator,
            }),
            duration_seconds: metadata.duration_seconds,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum MovieResourceStatus {
    #[default]
    Unloaded,
    Ready,
    Disabled,
    Missing,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum MoviePlaybackStatus {
    #[default]
    Idle,
    Playing,
    Paused,
    Stopped,
    Finished,
}

impl MovieState {
    pub fn load<H: MovieHost>(
        &mut self,
        host: &mut H,
        request: MovieRequest,
    ) -> RuntimeResult<MovieResourceStatus> {
        let loaded = host.load(request.clone())?;
        if let Some(previous) = self.current.as_ref() {
            host.stop(previous.handle)?;
        }
        *self = Self {
            request: Some(request),
            ..Self::default()
        };
        match loaded {
            MovieLoadResult::Ready(metadata) => {
                self.resource_status = MovieResourceStatus::Ready;
                self.metadata = Some(metadata.into());
            }
            MovieLoadResult::Disabled => {
                self.resource_status = MovieResourceStatus::Disabled;
            }
            MovieLoadResult::Missing => {
                self.resource_status = MovieResourceStatus::Missing;
            }
        }
        Ok(self.resource_status)
    }

    pub fn play<H: MovieHost>(&mut self, host: &mut H) -> RuntimeResult<bool> {
        if self.resource_status != MovieResourceStatus::Ready {
            return Ok(false);
        }
        if matches!(
            self.playback_status,
            MoviePlaybackStatus::Playing | MoviePlaybackStatus::Paused
        ) {
            return Ok(false);
        }
        let Some(request) = self.request.clone() else {
            return Ok(false);
        };
        let playback = host.play(request)?;
        if playback.handle == 0 {
            return Err(RuntimeError::Host(
                "movie host returned a zero playback handle".to_string(),
            ));
        }
        self.position_seconds = 0.0;
        self.last_frame_time = None;
        self.current = Some(playback);
        self.playback_status = MoviePlaybackStatus::Playing;
        Ok(true)
    }

    pub fn pause<H: MovieHost>(&mut self, host: &mut H, paused: bool) -> RuntimeResult<bool> {
        let Some(playback) = self.current.as_mut() else {
            return Ok(false);
        };
        if !matches!(
            self.playback_status,
            MoviePlaybackStatus::Playing | MoviePlaybackStatus::Paused
        ) {
            return Ok(false);
        }
        host.pause(playback.handle, paused)?;
        playback.paused = paused;
        self.playback_status = if paused {
            MoviePlaybackStatus::Paused
        } else {
            MoviePlaybackStatus::Playing
        };
        Ok(true)
    }

    pub fn stop<H: MovieHost>(&mut self, host: &mut H) -> RuntimeResult<bool> {
        let was_playing = matches!(
            self.playback_status,
            MoviePlaybackStatus::Playing | MoviePlaybackStatus::Paused
        );
        if let Some(playback) = &self.current {
            host.stop(playback.handle)?;
        }
        self.current = None;
        self.position_seconds = 0.0;
        self.last_frame_time = None;
        self.playback_status = MoviePlaybackStatus::Stopped;
        Ok(was_playing)
    }

    pub fn finished<H: MovieHost>(&mut self, host: &mut H) -> RuntimeResult<bool> {
        if self.playback_status == MoviePlaybackStatus::Finished {
            return Ok(true);
        }
        let Some(playback) = &self.current else {
            return Ok(false);
        };
        if host.finished(playback.handle)? {
            self.playback_status = MoviePlaybackStatus::Finished;
            if let Some(playback) = &mut self.current {
                playback.paused = false;
            }
            return Ok(true);
        }
        Ok(false)
    }

    /// Advances the host clock and records only timestamps reported by a
    /// decoder-backed host. It never synthesizes a frame.
    pub fn advance<H: MovieHost>(&mut self, host: &mut H, seconds: f64) -> RuntimeResult<()> {
        if !seconds.is_finite() || seconds < 0.0 {
            return Err(RuntimeError::Host(
                "movie time increment must be finite and non-negative".to_string(),
            ));
        }
        if self.playback_status != MoviePlaybackStatus::Playing {
            return Ok(());
        }
        let Some(handle) = self.current.as_ref().map(|playback| playback.handle) else {
            return Ok(());
        };
        if let Some(time) = host.last_frame_time(handle)? {
            if !time.is_finite() || time < 0.0 {
                return Err(RuntimeError::Host(
                    "movie host reported an invalid frame time".to_string(),
                ));
            }
            if self.last_frame_time.is_none_or(|previous| time >= previous) {
                self.last_frame_time = Some(time);
            }
        }
        let next_position = self.position_seconds + seconds;
        if !next_position.is_finite() {
            return Err(RuntimeError::Host(
                "movie playback position overflowed".to_string(),
            ));
        }
        self.position_seconds = next_position;
        self.finished(host)?;
        Ok(())
    }

    pub fn last_frame_time(&self) -> Option<f64> {
        self.last_frame_time
    }

    pub fn recover_after_restart(&mut self) {
        self.current = None;
        self.resource_status = MovieResourceStatus::Unloaded;
        self.playback_status = MoviePlaybackStatus::Idle;
        self.metadata = None;
        self.position_seconds = 0.0;
        self.last_frame_time = None;
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct GameClock {
    pub frame: u64,
    pub elapsed_ticks: u64,
    pub paused: bool,
    #[serde(default)]
    pub last_delta_ticks: u64,
}

impl GameClock {
    pub fn advance(&mut self, ticks: u64) -> bool {
        if self.paused {
            self.last_delta_ticks = 0;
            return false;
        }
        self.frame = self.frame.saturating_add(1);
        self.elapsed_ticks = self.elapsed_ticks.saturating_add(ticks);
        self.last_delta_ticks = ticks;
        true
    }

    pub fn advance_u32(&mut self, ticks: u32) -> bool {
        self.advance(u64::from(ticks))
    }

    pub fn frame_u32(&self) -> u32 {
        self.frame as u32
    }

    pub fn elapsed_ticks_u32(&self) -> u32 {
        self.elapsed_ticks as u32
    }

    pub fn last_delta_ticks_u32(&self) -> u32 {
        self.last_delta_ticks as u32
    }

    pub fn advance_frame(&mut self) -> bool {
        self.advance(0)
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
        if paused {
            self.last_delta_ticks = 0;
        }
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RuntimeState {
    pub schema_version: u32,
    pub scene: SceneState,
    pub script: ScriptState,
    pub variables: BTreeMap<String, RuntimeValue>,
    pub flags: BTreeMap<String, bool>,
    pub input: InputState,
    pub audio: AudioState,
    pub movie: MovieState,
    pub clock: GameClock,
    pub tasks: TaskScheduler,
    #[serde(default)]
    pub object_handles: ObjectHandleTable,
}

impl Default for RuntimeState {
    fn default() -> Self {
        Self {
            schema_version: 1,
            scene: SceneState::default(),
            script: ScriptState::default(),
            variables: BTreeMap::new(),
            flags: BTreeMap::new(),
            input: InputState::default(),
            audio: AudioState::default(),
            movie: MovieState::default(),
            clock: GameClock::default(),
            tasks: TaskScheduler::default(),
            object_handles: ObjectHandleTable::default(),
        }
    }
}

impl RuntimeState {
    pub fn apply_input(&mut self, event: InputEvent) {
        self.input.apply(event);
    }

    pub fn apply_input_all<I>(&mut self, events: I)
    where
        I: IntoIterator<Item = InputEvent>,
    {
        self.input.apply_all(events);
    }

    pub fn commit_input(&mut self) {
        self.input.commit();
    }

    pub fn clear_input(&mut self) {
        self.input.clear();
    }

    pub fn tick(&mut self, elapsed_ticks: u64) -> bool {
        self.clock.advance(elapsed_ticks)
    }

    pub fn tick_u32(&mut self, elapsed_ticks: u32) -> bool {
        self.clock.advance_u32(elapsed_ticks)
    }

    pub fn advance_frame_u32<I>(&mut self, elapsed_ticks: u32, events: I) -> Vec<TaskId>
    where
        I: IntoIterator<Item = InputEvent>,
    {
        self.apply_input_all(events);
        if !self.clock.advance_u32(elapsed_ticks) {
            return Vec::new();
        }
        self.tasks.poll_ready_u32(self.clock.frame_u32())
    }

    pub fn tick_and_poll_tasks(&mut self, elapsed_ticks: u64) -> Vec<TaskId> {
        if !self.clock.advance(elapsed_ticks) {
            return Vec::new();
        }
        self.tasks.poll_ready(self.clock.frame)
    }

    pub fn advance_frame<I>(&mut self, elapsed_ticks: u64, events: I) -> Vec<TaskId>
    where
        I: IntoIterator<Item = InputEvent>,
    {
        self.apply_input_all(events);
        self.tick_and_poll_tasks(elapsed_ticks)
    }

    pub fn end_frame(&mut self) {
        self.commit_input();
        self.tasks.retain_active();
    }

    pub fn poll_tasks(&mut self) -> Vec<TaskId> {
        self.tasks.poll_ready(self.clock.frame)
    }

    pub fn allocate_object_handle(&mut self) -> RuntimeResult<ObjectHandle> {
        self.object_handles.allocate()
    }

    pub fn release_object_handle(&mut self, handle: ObjectHandle) -> bool {
        self.object_handles.release(handle)
    }

    pub fn play_audio<H: AudioHost>(
        &mut self,
        host: &mut H,
        request: AudioRequest,
    ) -> RuntimeResult<PlaybackState> {
        if !request.volume.is_finite() || !request.pan.is_finite() {
            return Err(RuntimeError::Host(
                "audio volume and pan must be finite".to_string(),
            ));
        }
        let channel = request.channel;
        let playback = host.play(request)?;
        if playback.handle == 0 {
            return Err(RuntimeError::Host(
                "audio host returned a zero playback handle".to_string(),
            ));
        }
        let previous = self.audio.replace(channel, playback.clone());
        if let Some(previous) = previous {
            if let Err(error) = host.stop(previous.handle) {
                self.audio.remove_handle(playback.handle);
                self.audio.replace(channel, previous);
                let _ = host.stop(playback.handle);
                return Err(error);
            }
        }
        Ok(playback)
    }

    pub fn pause_audio<H: AudioHost>(
        &mut self,
        host: &mut H,
        handle: u64,
        paused: bool,
    ) -> RuntimeResult<bool> {
        if !self
            .audio
            .bgm
            .iter()
            .chain(self.audio.voice.iter())
            .chain(self.audio.sound_effects.iter())
            .any(|playback| playback.handle == handle)
        {
            return Ok(false);
        }
        host.pause(handle, paused)?;
        if let Some(playback) = self
            .audio
            .bgm
            .iter_mut()
            .chain(self.audio.voice.iter_mut())
            .chain(self.audio.sound_effects.iter_mut())
            .find(|playback| playback.handle == handle)
        {
            playback.paused = paused;
        }
        Ok(true)
    }

    pub fn stop_audio<H: AudioHost>(&mut self, host: &mut H, handle: u64) -> RuntimeResult<bool> {
        let active = self.audio.handles().any(|active| active == handle);
        if !active {
            return Ok(false);
        }
        host.stop(handle)?;
        self.audio.remove_handle(handle);
        Ok(true)
    }

    pub fn stop_audio_channel<H: AudioHost>(
        &mut self,
        host: &mut H,
        channel: AudioChannel,
    ) -> RuntimeResult<bool> {
        let had_playback = self.audio.playback(channel).is_some();
        if !had_playback {
            return Ok(false);
        }
        host.stop_all(channel)?;
        self.audio.remove_channel(channel);
        Ok(true)
    }

    pub fn recover_after_restart(&mut self) -> RuntimeResult<()> {
        self.validate()?;
        self.input.clear();
        self.audio.clear_external_handles();
        self.movie.recover_after_restart();
        self.tasks.recover_after_restart(self.clock.frame)?;
        self.object_handles.clear();
        self.clock.paused = false;
        self.clock.last_delta_ticks = 0;
        self.validate()
    }

    pub fn validate(&self) -> RuntimeResult<()> {
        if self.schema_version == 0 {
            return Err(RuntimeError::InvalidSave(
                "runtime state schema version is zero".to_string(),
            ));
        }
        if let Some(group) = &self.script.execution_group {
            group.restore().map_err(|error| RuntimeError::InvalidSave(format!(
                "invalid script execution snapshot: {error}",
            )))?;
        }
        self.audio
            .validate()
            .map_err(|error| RuntimeError::InvalidSave(error.to_string()))?;
        self.tasks
            .validate()
            .map_err(|error| RuntimeError::InvalidSave(error.to_string()))?;
        self.object_handles
            .validate()
            .map_err(|error| RuntimeError::InvalidSave(error.to_string()))?;
        if self.clock.paused && self.clock.last_delta_ticks != 0 {
            return Err(RuntimeError::InvalidSave(
                "paused clock has a nonzero frame delta".to_string(),
            ));
        }
        if !self.movie.position_seconds.is_finite() || self.movie.position_seconds < 0.0 {
            return Err(RuntimeError::InvalidSave(
                "movie position is invalid".to_string(),
            ));
        }
        if self
            .movie
            .last_frame_time
            .is_some_and(|time| !time.is_finite() || time < 0.0)
        {
            return Err(RuntimeError::InvalidSave(
                "movie frame time is invalid".to_string(),
            ));
        }
        if self.movie.playback_status == MoviePlaybackStatus::Playing
            && self.movie.current.is_none()
        {
            return Err(RuntimeError::InvalidSave(
                "playing movie state has no host handle".to_string(),
            ));
        }
        if self.movie.current.as_ref().is_some_and(|playback| {
            playback.handle == 0
                || self
                    .audio
                    .handles()
                    .any(|audio_handle| audio_handle == playback.handle)
        }) {
            return Err(RuntimeError::InvalidSave(
                "movie state contains an invalid or duplicate playback handle".to_string(),
            ));
        }
        Ok(())
    }

    pub fn load_movie<H: MovieHost>(
        &mut self,
        host: &mut H,
        request: MovieRequest,
    ) -> RuntimeResult<MovieResourceStatus> {
        self.movie.load(host, request)
    }

    pub fn play_movie<H: MovieHost>(&mut self, host: &mut H) -> RuntimeResult<bool> {
        self.movie.play(host)
    }

    pub fn pause_movie<H: MovieHost>(&mut self, host: &mut H, paused: bool) -> RuntimeResult<bool> {
        self.movie.pause(host, paused)
    }

    pub fn stop_movie<H: MovieHost>(&mut self, host: &mut H) -> RuntimeResult<bool> {
        self.movie.stop(host)
    }

    pub fn movie_finished<H: MovieHost>(&mut self, host: &mut H) -> RuntimeResult<bool> {
        self.movie.finished(host)
    }

    pub fn advance_movie<H: MovieHost>(
        &mut self,
        host: &mut H,
        elapsed_seconds: f64,
    ) -> RuntimeResult<()> {
        self.movie.advance(host, elapsed_seconds)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{InputKey, RuntimeState};
    use crate::{MovieHost, MovieLoadResult, MovieRequest};

    #[derive(Default)]
    struct TestMovieHost {
        load_result: Option<MovieLoadResult>,
        finished: bool,
        last_frame_time: Option<f64>,
        next_handle: u64,
        paused: Vec<bool>,
        stopped: Vec<u64>,
    }

    impl MovieHost for TestMovieHost {
        fn load(&mut self, _request: MovieRequest) -> RuntimeResult<MovieLoadResult> {
            Ok(self.load_result.clone().unwrap_or(MovieLoadResult::Missing))
        }

        fn play(&mut self, request: MovieRequest) -> RuntimeResult<PlaybackState> {
            self.next_handle += 1;
            Ok(PlaybackState {
                handle: self.next_handle,
                resource: request.resource,
                looped: request.looped,
                paused: false,
            })
        }

        fn stop(&mut self, handle: u64) -> RuntimeResult<()> {
            self.stopped.push(handle);
            Ok(())
        }

        fn pause(&mut self, _handle: u64, paused: bool) -> RuntimeResult<()> {
            self.paused.push(paused);
            Ok(())
        }

        fn finished(&mut self, _handle: u64) -> RuntimeResult<bool> {
            Ok(self.finished)
        }

        fn last_frame_time(&mut self, _handle: u64) -> RuntimeResult<Option<f64>> {
            Ok(self.last_frame_time)
        }
    }

    #[test]
    fn input_focus_and_clock_follow_frame_semantics() {
        let mut state = RuntimeState::default();
        state.apply_input(InputEvent::KeyDown(InputKey::Keyboard(13)));
        state.apply_input(InputEvent::KeyDown(InputKey::Keyboard(13)));
        assert_eq!(state.input.pressed.len(), 1);
        assert!(state.input.key_pressed(13));
        assert!(state.input.key_was_pressed(13));
        assert!(state.tick(16));
        state.clock.set_paused(true);
        assert!(!state.tick(16));
        state.apply_input(InputEvent::FocusChanged(false));
        assert!(state.input.pressed.is_empty());
        assert!(state.input.key_was_released(13));
    }

    #[test]
    fn commit_input_preserves_held_state_and_clears_frame_edges() {
        let mut state = RuntimeState::default();
        state.apply_input(InputEvent::KeyDown(InputKey::Keyboard(32)));
        state.apply_input(InputEvent::KeyDown(InputKey::MouseWheel(120)));

        state.commit_input();

        assert!(state.input.key_pressed(32));
        assert!(!state.input.key_was_pressed(32));
        assert_eq!(state.input.wheel_delta(), 0);
        assert!(state.input.wheel_events().is_empty());
    }

    #[test]
    fn clear_input_resets_held_and_transient_input_without_resetting_clock() {
        let mut state = RuntimeState::default();
        state.apply_input(InputEvent::KeyDown(InputKey::Keyboard(32)));
        state.apply_input(InputEvent::MouseMove { x: 17, y: 23 });
        state.apply_input(InputEvent::QuitRequested);
        assert!(state.tick(16));

        state.clear_input();

        assert_eq!(state.input, InputState::default());
        assert_eq!(state.clock.frame, 1);
        assert_eq!(state.clock.elapsed_ticks, 16);
    }

    #[test]
    fn scene_transitions_reset_line_and_record_return_script() {
        let mut state = RuntimeState::default();
        state.scene.enter_script("main.cst", Some("start".into()));
        state.scene.advance_line();
        state.scene.return_to("menu.cst", None);
        assert_eq!(state.scene.current_script.as_deref(), Some("menu.cst"));
        assert_eq!(state.scene.return_script.as_deref(), Some("main.cst"));
        assert_eq!(state.scene.line, 0);
    }

    #[test]
    fn movie_load_statuses_distinguish_disabled_missing_and_ready() {
        let request = MovieRequest {
            resource: "opening.mpg".into(),
            looped: false,
            muted: false,
        };
        let mut state = RuntimeState::default();
        let mut host = TestMovieHost {
            load_result: Some(MovieLoadResult::Disabled),
            ..TestMovieHost::default()
        };
        assert_eq!(
            state.load_movie(&mut host, request.clone()).unwrap(),
            MovieResourceStatus::Disabled
        );
        assert!(!state.play_movie(&mut host).unwrap());

        host.load_result = Some(MovieLoadResult::Missing);
        assert_eq!(
            state.load_movie(&mut host, request.clone()).unwrap(),
            MovieResourceStatus::Missing
        );

        host.load_result = Some(MovieLoadResult::Ready(MovieMetadata {
            video_stream_ids: vec![0xe0],
            audio_stream_ids: vec![],
            pes_start_offsets: vec![12],
            width: Some(320),
            height: Some(240),
            time_base: None,
            duration_seconds: Some(4.0),
        }));
        assert_eq!(
            state.load_movie(&mut host, request).unwrap(),
            MovieResourceStatus::Ready
        );
        assert!(state.movie.metadata.is_some());
    }

    #[test]
    fn movie_lifecycle_tracks_time_pause_finish_and_host_frames() {
        let mut state = RuntimeState::default();
        let mut host = TestMovieHost {
            load_result: Some(MovieLoadResult::Ready(MovieMetadata::default())),
            last_frame_time: Some(0.04),
            ..TestMovieHost::default()
        };
        state
            .load_movie(
                &mut host,
                MovieRequest {
                    resource: "opening.mpg".into(),
                    looped: false,
                    muted: false,
                },
            )
            .unwrap();
        assert!(state.play_movie(&mut host).unwrap());
        assert!(state.pause_movie(&mut host, true).unwrap());
        state.advance_movie(&mut host, 0.5).unwrap();
        assert_eq!(state.movie.position_seconds, 0.0);
        assert_eq!(state.movie.last_frame_time, None);

        assert!(state.pause_movie(&mut host, false).unwrap());
        state.advance_movie(&mut host, 0.5).unwrap();
        assert_eq!(state.movie.position_seconds, 0.5);
        assert_eq!(state.movie.last_frame_time, Some(0.04));

        host.finished = true;
        assert!(state.movie_finished(&mut host).unwrap());
        assert_eq!(state.movie.playback_status, MoviePlaybackStatus::Finished);
        assert!(!state.stop_movie(&mut host).unwrap());
        assert_eq!(host.stopped, [1]);
    }

    #[test]
    fn old_movie_state_deserializes_and_time_inputs_are_validated() {
        let movie: MovieState = serde_json::from_str(r#"{"current":null}"#).unwrap();
        assert_eq!(movie, MovieState::default());

        let mut state = RuntimeState::default();
        let mut host = TestMovieHost::default();
        assert!(matches!(
            state.advance_movie(&mut host, f64::NAN),
            Err(RuntimeError::Host(_))
        ));
    }

    #[test]
    fn clock_u32_views_and_task_polling_follow_runtime_frames() {
        let mut state = RuntimeState::default();
        let task = state
            .tasks
            .schedule_after("frame task", 0, 2, None)
            .unwrap();
        state.clock.set_paused(true);
        assert!(!state.tick_u32(16));
        assert_eq!(state.clock.frame_u32(), 0);
        state.clock.set_paused(false);
        assert!(state.tick_u32(16));
        assert_eq!(state.clock.frame_u32(), 1);
        assert_eq!(state.clock.elapsed_ticks_u32(), 16);
        assert_eq!(state.clock.last_delta_ticks_u32(), 16);
        assert!(state.poll_tasks().is_empty());
        assert_eq!(state.tick_and_poll_tasks(16), vec![task]);
        assert_eq!(state.clock.frame_u32(), 2);
        assert!(state.poll_tasks().is_empty());
    }

    #[test]
    fn restart_releases_external_state_and_rearms_running_tasks() {
        let mut state = RuntimeState::default();
        let object = state.allocate_object_handle().unwrap();
        let task = state.tasks.schedule("restart", 1, None).unwrap();
        state.apply_input(InputEvent::KeyDown(InputKey::Keyboard(13)));
        state.tick(1);
        assert_eq!(state.poll_tasks(), vec![task]);
        assert!(state.object_handles.contains(object));
        assert!(state.input.key_pressed(13));

        state.recover_after_restart().unwrap();

        assert!(!state.object_handles.contains(object));
        assert!(state.input.pressed.is_empty());
        assert_eq!(state.tasks.status(task).unwrap(), crate::TaskStatus::Ready);
        assert!(!state.clock.is_paused());
        assert_eq!(state.clock.last_delta_ticks, 0);
    }

    #[test]
    fn invalid_restored_runtime_state_is_rejected_before_restart() {
        let mut state = RuntimeState::default();
        state.schema_version = 0;
        assert!(matches!(
            state.recover_after_restart(),
            Err(RuntimeError::InvalidSave(message))
                if message == "runtime state schema version is zero"
        ));

        let mut state = RuntimeState::default();
        state.movie.position_seconds = f64::NAN;
        assert!(matches!(
            state.validate(),
            Err(RuntimeError::InvalidSave(message))
                if message == "movie position is invalid"
        ));
    }
}

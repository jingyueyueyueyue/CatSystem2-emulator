use crate::{RuntimeResult, RuntimeState};
use cs2_formats::MovieMetadata;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum InputKey {
    Keyboard(u32),
    MouseButton(u8),
    MouseWheel(i32),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum InputEvent {
    KeyDown(InputKey),
    KeyUp(InputKey),
    MouseMove { x: i32, y: i32 },
    Text(String),
    FocusChanged(bool),
    QuitRequested,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct InputState {
    pub pressed: Vec<InputKey>,
    pub cursor_x: i32,
    pub cursor_y: i32,
    pub focused: bool,
    pub quit_requested: bool,
    #[serde(default)]
    pub just_pressed: Vec<InputKey>,
    #[serde(default)]
    pub just_released: Vec<InputKey>,
    #[serde(default)]
    pub wheel_events: Vec<i32>,
    #[serde(default)]
    pub wheel_delta: i32,
    #[serde(default)]
    pub text_events: Vec<String>,
}

impl InputState {
    pub fn apply_all<I>(&mut self, events: I)
    where
        I: IntoIterator<Item = InputEvent>,
    {
        for event in events {
            self.apply(event);
        }
    }

    pub fn apply(&mut self, event: InputEvent) {
        match event {
            InputEvent::KeyDown(key) => {
                if let InputKey::MouseWheel(delta) = key {
                    if delta != 0 {
                        self.wheel_events.push(delta);
                        self.wheel_delta = self.wheel_delta.saturating_add(delta);
                    }
                    return;
                }
                if !self.pressed.contains(&key) {
                    self.pressed.push(key.clone());
                    if !self.just_pressed.contains(&key) {
                        self.just_pressed.push(key);
                    }
                }
            }
            InputEvent::KeyUp(key) => {
                if matches!(key, InputKey::MouseWheel(_)) {
                    return;
                }
                let was_pressed = self.pressed.iter().any(|pressed| *pressed == key);
                self.pressed.retain(|pressed| *pressed != key);
                if was_pressed && !self.just_released.contains(&key) {
                    self.just_released.push(key);
                }
            }
            InputEvent::MouseMove { x, y } => {
                self.cursor_x = x;
                self.cursor_y = y;
            }
            InputEvent::FocusChanged(focused) => {
                self.focused = focused;
                if !focused {
                    let released = std::mem::take(&mut self.pressed);
                    for key in released {
                        if !self.just_released.contains(&key) {
                            self.just_released.push(key);
                        }
                    }
                }
            }
            InputEvent::QuitRequested => self.quit_requested = true,
            InputEvent::Text(text) => self.text_events.push(text),
        }
    }

    pub fn is_pressed(&self, key: &InputKey) -> bool {
        !matches!(key, InputKey::MouseWheel(_)) && self.pressed.contains(key)
    }

    pub fn key_pressed(&self, key: u32) -> bool {
        self.is_pressed(&InputKey::Keyboard(key))
    }

    pub fn mouse_button_pressed(&self, button: u8) -> bool {
        self.is_pressed(&InputKey::MouseButton(button))
    }

    pub fn was_pressed(&self, key: &InputKey) -> bool {
        !matches!(key, InputKey::MouseWheel(_)) && self.just_pressed.contains(key)
    }

    pub fn key_was_pressed(&self, key: u32) -> bool {
        self.was_pressed(&InputKey::Keyboard(key))
    }

    pub fn mouse_button_was_pressed(&self, button: u8) -> bool {
        self.was_pressed(&InputKey::MouseButton(button))
    }

    pub fn was_released(&self, key: &InputKey) -> bool {
        !matches!(key, InputKey::MouseWheel(_)) && self.just_released.contains(key)
    }

    pub fn key_was_released(&self, key: u32) -> bool {
        self.was_released(&InputKey::Keyboard(key))
    }

    pub fn mouse_button_was_released(&self, button: u8) -> bool {
        self.was_released(&InputKey::MouseButton(button))
    }

    pub fn wheel_delta(&self) -> i32 {
        self.wheel_delta
    }

    pub fn wheel_events(&self) -> &[i32] {
        &self.wheel_events
    }

    pub fn text_events(&self) -> &[String] {
        &self.text_events
    }

    pub fn wheel_up(&self) -> bool {
        self.wheel_delta > 0
    }

    pub fn wheel_down(&self) -> bool {
        self.wheel_delta < 0
    }

    pub fn commit(&mut self) {
        self.just_pressed.clear();
        self.just_released.clear();
        self.wheel_events.clear();
        self.wheel_delta = 0;
        self.text_events.clear();
    }

    pub fn transient_is_clear(&self) -> bool {
        self.just_pressed.is_empty()
            && self.just_released.is_empty()
            && self.wheel_events.is_empty()
            && self.wheel_delta == 0
            && self.text_events.is_empty()
            && !self.quit_requested
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    pub fn clear_quit_request(&mut self) {
        self.quit_requested = false;
    }
}

pub trait InputHost {
    fn poll_events(&mut self) -> RuntimeResult<Vec<InputEvent>>;

    fn set_cursor_visible(&mut self, visible: bool) -> RuntimeResult<()> {
        Err(crate::RuntimeError::Host(format!(
            "input host does not support cursor visibility changes to {visible}"
        )))
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum AudioChannel {
    Bgm,
    Voice,
    SoundEffect,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AudioRequest {
    pub channel: AudioChannel,
    pub resource: String,
    pub volume: f32,
    pub pan: f32,
    pub looped: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlaybackState {
    pub handle: u64,
    pub resource: String,
    pub looped: bool,
    pub paused: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct AudioState {
    pub bgm: Option<PlaybackState>,
    pub voice: Option<PlaybackState>,
    pub sound_effects: Vec<PlaybackState>,
}

impl AudioState {
    pub fn playback(&self, channel: AudioChannel) -> Option<&PlaybackState> {
        match channel {
            AudioChannel::Bgm => self.bgm.as_ref(),
            AudioChannel::Voice => self.voice.as_ref(),
            AudioChannel::SoundEffect => self.sound_effects.last(),
        }
    }

    pub fn handles(&self) -> impl Iterator<Item = u64> + '_ {
        self.bgm
            .iter()
            .chain(self.voice.iter())
            .chain(self.sound_effects.iter())
            .map(|playback| playback.handle)
    }

    pub fn replace(
        &mut self,
        channel: AudioChannel,
        playback: PlaybackState,
    ) -> Option<PlaybackState> {
        match channel {
            AudioChannel::Bgm => self.bgm.replace(playback),
            AudioChannel::Voice => self.voice.replace(playback),
            AudioChannel::SoundEffect => {
                self.sound_effects.push(playback);
                None
            }
        }
    }

    pub fn remove_handle(&mut self, handle: u64) -> Option<PlaybackState> {
        if self
            .bgm
            .as_ref()
            .is_some_and(|playback| playback.handle == handle)
        {
            return self.bgm.take();
        }
        if self
            .voice
            .as_ref()
            .is_some_and(|playback| playback.handle == handle)
        {
            return self.voice.take();
        }
        let index = self
            .sound_effects
            .iter()
            .position(|playback| playback.handle == handle)?;
        Some(self.sound_effects.remove(index))
    }

    pub fn remove_channel(&mut self, channel: AudioChannel) -> Vec<PlaybackState> {
        match channel {
            AudioChannel::Bgm => self.bgm.take().into_iter().collect(),
            AudioChannel::Voice => self.voice.take().into_iter().collect(),
            AudioChannel::SoundEffect => std::mem::take(&mut self.sound_effects),
        }
    }

    pub fn clear_external_handles(&mut self) {
        *self = Self::default();
    }

    pub fn validate(&self) -> RuntimeResult<()> {
        let mut seen = std::collections::BTreeSet::new();
        for playback in self
            .bgm
            .iter()
            .chain(self.voice.iter())
            .chain(self.sound_effects.iter())
        {
            if playback.handle == 0 || !seen.insert(playback.handle) {
                return Err(crate::RuntimeError::InvalidSave(
                    "audio state contains an invalid or duplicate playback handle".to_string(),
                ));
            }
        }
        Ok(())
    }
}

pub trait AudioHost {
    fn play(&mut self, request: AudioRequest) -> RuntimeResult<PlaybackState>;
    fn stop(&mut self, handle: u64) -> RuntimeResult<()>;
    fn pause(&mut self, handle: u64, paused: bool) -> RuntimeResult<()>;
    fn stop_all(&mut self, channel: AudioChannel) -> RuntimeResult<()>;
    fn is_active(&mut self, handle: u64) -> RuntimeResult<bool> {
        if handle == 0 {
            return Ok(false);
        }
        Err(crate::RuntimeError::Host(
            "audio host does not provide playback status".to_string(),
        ))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MovieRequest {
    pub resource: String,
    pub looped: bool,
    pub muted: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MovieLoadResult {
    Ready(MovieMetadata),
    Disabled,
    Missing,
}

pub trait MovieHost {
    fn load(&mut self, request: MovieRequest) -> RuntimeResult<MovieLoadResult> {
        Err(crate::RuntimeError::Host(format!(
            "movie host does not implement loading for {:?}",
            request.resource
        )))
    }

    fn play(&mut self, request: MovieRequest) -> RuntimeResult<PlaybackState>;
    fn stop(&mut self, handle: u64) -> RuntimeResult<()>;
    fn pause(&mut self, handle: u64, paused: bool) -> RuntimeResult<()>;

    fn finished(&mut self, handle: u64) -> RuntimeResult<bool> {
        Err(crate::RuntimeError::Host(format!(
            "movie host does not provide completion status for handle {handle}"
        )))
    }

    /// Reports the time of the latest frame actually presented by the host.
    /// None means the host has not produced a frame; this runtime does not
    /// infer decoded frames from playback time.
    fn last_frame_time(&mut self, _handle: u64) -> RuntimeResult<Option<f64>> {
        Ok(None)
    }
}

pub trait SaveHost {
    fn save(&mut self, slot: u32, state: &RuntimeState) -> RuntimeResult<()>;
    fn load(&mut self, slot: u32) -> RuntimeResult<Option<RuntimeState>>;
    fn delete(&mut self, slot: u32) -> RuntimeResult<()>;
    fn list_slots(&mut self) -> RuntimeResult<Vec<u32>>;
}

#[cfg(test)]
mod tests {
    use super::{InputEvent, InputKey, InputState};

    #[test]
    fn key_and_mouse_queries_track_edges_without_duplicate_down_events() {
        let mut input = InputState::default();
        input.apply(InputEvent::KeyDown(InputKey::Keyboard(13)));
        input.apply(InputEvent::KeyDown(InputKey::Keyboard(13)));
        input.apply(InputEvent::KeyDown(InputKey::MouseButton(1)));

        assert!(input.key_pressed(13));
        assert!(input.mouse_button_pressed(1));
        assert!(input.key_was_pressed(13));
        assert!(input.mouse_button_was_pressed(1));
        assert_eq!(input.just_pressed.len(), 2);

        input.apply(InputEvent::KeyUp(InputKey::Keyboard(13)));
        assert!(!input.key_pressed(13));
        assert!(input.key_was_released(13));
        assert!(!input.mouse_button_was_released(1));
    }

    #[test]
    fn wheel_events_are_transient_and_accumulate_until_commit() {
        let mut input = InputState::default();
        input.apply(InputEvent::KeyDown(InputKey::MouseWheel(120)));
        input.apply(InputEvent::KeyDown(InputKey::MouseWheel(-40)));
        input.apply(InputEvent::Text("hello".into()));

        assert_eq!(input.wheel_events(), &[120, -40]);
        assert_eq!(input.wheel_delta(), 80);
        assert_eq!(input.text_events(), &["hello"]);
        assert!(input.wheel_up());
        assert!(!input.wheel_down());
        assert!(input.pressed.is_empty());

        input.commit();
        assert!(input.wheel_events().is_empty());
        assert_eq!(input.wheel_delta(), 0);
        assert!(input.text_events().is_empty());
        assert!(!input.wheel_up());
        assert!(!input.wheel_down());
    }

    #[test]
    fn repeated_transitions_in_one_frame_keep_edge_queries_boolean() {
        let mut input = InputState::default();
        let key = InputKey::Keyboard(65);
        input.apply(InputEvent::KeyDown(key.clone()));
        input.apply(InputEvent::KeyUp(key.clone()));
        input.apply(InputEvent::KeyDown(key));

        assert!(input.key_pressed(65));
        assert!(input.key_was_pressed(65));
        assert!(input.key_was_released(65));
        assert_eq!(input.just_pressed.len(), 1);
        assert_eq!(input.just_released.len(), 1);
    }

    #[test]
    fn old_serialized_input_state_defaults_new_frame_fields() {
        let input: InputState = serde_json::from_str(
            r#"{"pressed":[{"Keyboard":13}],"cursor_x":4,"cursor_y":9,"focused":true,"quit_requested":false}"#,
        )
        .unwrap();

        assert!(input.key_pressed(13));
        assert!(input.just_pressed.is_empty());
        assert!(input.just_released.is_empty());
        assert!(input.wheel_events.is_empty());
        assert_eq!(input.wheel_delta(), 0);
        assert!(input.text_events().is_empty());
    }

    #[test]
    fn losing_focus_releases_pressed_inputs_and_clear_resets_state() {
        let mut input = InputState::default();
        input.focused = true;
        input.apply(InputEvent::KeyDown(InputKey::Keyboard(65)));
        input.apply(InputEvent::KeyDown(InputKey::MouseButton(0)));
        input.apply(InputEvent::FocusChanged(false));

        assert!(input.pressed.is_empty());
        assert!(input.key_was_released(65));
        assert!(input.mouse_button_was_released(0));

        input.apply(InputEvent::QuitRequested);
        input.clear();
        assert_eq!(input, InputState::default());
    }
}

use anyhow::{Context, Result, bail};
use cs2_formats::{
    AnmFile, ConfigXml, CssFile, CstlTable, HgFile,
};
use cs2_renderer::FrameSnapshot;
use cs2_renderer::{
    Canvas, Color, FrameFilter, FrameMetadata, HgFrameAttributes, ImageDrawOptions,
    ParticlePoint, Point, Rect, RenderCommand, RenderNode, RenderQueue, RgbaFrame,
};
use cs2_runtime::{
    AudioChannel, AudioHost, AudioRequest, FileSaveStore, GameRoot, GameScan, InputEvent,
    InputHost, InputKey, OggAudioHost,
    ResourceIndex, RuntimeError, RuntimeState, SaveHost, ScanDiagnosticLevel, ScriptState,
};
use cs2_script::{
    CstDocument, DisplayState, ExpressionState, FesDocument, Host, ImageAnimation, KcsProgram,
    NativeReturn, Step, Vm, eval_expression_fallback, eval_expression_parameter,
};
use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

mod text;
use text::{load_font, draw_message_text};
mod audio;
mod audio_decode;
mod wav_decode;
mod audio_control;
#[cfg(target_os = "android")]
mod audio_android;
pub use audio_decode::{AudioCodec, AudioProbe, DecodedAudio};
pub use audio_control::{set_playback_properties, set_playback_repeat_count};
mod sound_native;
mod sound_control;
mod sound_task;
mod reserved_native;
mod sound_legacy;
mod sound_loop_table;
mod sound_media;
mod sound_sidecar;
mod sound_signal;
mod sound_queries;
mod sound_speech;
mod sound_speech_load;
mod speech_openjtalk;
mod speech_table;
mod media_wrapper_native;
pub use sound_speech::{
    SpeechConfiguration, SpeechConfigurationHost, SpeechPcm, SpeechSynthesisParameters,
};
mod resource_category_native;
mod resource_direct_native;
pub use resource_direct_native::ImageRecordPolicy;
mod media_clock;
mod media_process;
mod media_input;
mod movie;
#[cfg(target_os = "android")]
mod movie_android;
mod movie_audio;
mod movie_native;
mod movie_plane;
mod movie_plane_native;
mod movie_plane_render;
pub use movie_plane_render::MoviePlaneProjection;
#[cfg(windows)]
mod window_media;
mod data_services;
mod language_native;
mod language_platform;
mod language_adapters;
mod message_native;
mod message_backend;
mod message_platform;
mod input_history_native;
mod legacy_text_native;
mod ordinary_native;
mod ordinary_pair_table;
mod resource_preload;
mod name_table_format;
mod image_native;
mod image_export_native;
mod image_manager;
mod image_pixels;
mod image_blit;
mod image_transform_native;
mod image_resample;
mod database_native;
mod image_global_blit;
mod emote_native;
mod effekseer_native;
mod emocon_native;
mod string_native;
mod file_native;
mod portable_host;
pub use portable_host::HostFrameTiming;
use portable_host::{HostPhase, measure_host_phase, measure_script_detail};
mod vm_context_native;
mod achievement_native;
pub use achievement_native::LocalAchievement;
mod window_control;
pub use window_control::HostWindowRequest;
mod ui_resources;
mod settings_menu_native;
#[cfg(windows)]
mod settings_menu_windows;
pub use settings_menu_native::{HostEffectSettings, HostGameMenu, HostInputPanel, HostMenuConfirmation, HostMenuItem, HostSettingControl, HostSettingKind, HostSettingsEdit, HostSettingsPage};
mod plane_native;
mod native_tasks;
use native_tasks::OwnedTaskBuffers;
pub use native_tasks::MessageListView;
pub use native_tasks::SceneToolView;
mod javascript_native;
mod host_adapter;
mod fes_driver;
mod fes_tasks;
mod fes_task_platform;
mod fes_media;
mod fes_motion;
mod fes_controls;
mod fes_text;
mod fes_sound;
mod fes_globals;
mod fes_settings;
mod configuration;
mod save_native;
mod css_task_native;
mod css_dae;
mod css_model;
mod css_model_render;
mod css_model_shader;
mod backlog_native;
mod cst_context_native;
mod cst_window_native;
mod cst_state;
mod system_open_native;
mod dialog_native;
pub use dialog_native::HostDialogRequest;
pub use system_open_native::{PendingSystemOpen, SystemOpenHandler, SystemOpenRequest};
mod wallpaper_native;
mod tweet_native;
mod tweet_worker;
mod tweet_protocol;
mod tweet_platform;
mod tweet_host;
pub use tweet_host::tweet_character_count;
pub use wallpaper_native::{WallpaperApplyHandler, WallpaperExportPrompt,
    WallpaperExportRequest, WallpaperExportSelection};
mod calendar_native;
mod clipboard_native;
mod frame_capture;
mod scene_compositor;
mod dictionary_native;
mod task_wrapper_native;
mod top_task_native;
mod coordinate_native;
mod word_pair_native;
mod fes_draw;
mod list_native;
mod canvas_native;
mod transition_native;
mod fes_services;
mod fes_particle;
mod fes_save_panel;
mod motion_vm;
mod particle_vm;
mod particle_plugin;
mod particle_plugin_digest;
mod particle_rain_field;
mod particle_burst;
mod particle_native;
mod particle_external;
mod particle_canvas;
mod particle_canvas_native;
mod particle_render;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScriptTextEncoding {
    #[default]
    Cp932,
    Gbk,
    Utf8,
}

impl ScriptTextEncoding {
    pub fn decode(self, bytes: &[u8]) -> String {
        match self {
            Self::Cp932 => encoding_rs::SHIFT_JIS.decode(bytes).0.into_owned(),
            Self::Gbk => encoding_rs::GBK.decode(bytes).0.into_owned(),
            Self::Utf8 => String::from_utf8_lossy(bytes).into_owned(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogicEvent {
    pub frame: u64,
    pub kind: String,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedEntry {
    pub name: String,
    pub kind: String,
    pub byte_length: usize,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SceneNode {
    pub id: u32,
    pub kind: u32,
    pub parent: u32,
    pub name: String,
    pub size: [f32; 2],
    pub position: [f32; 3],
    pub scale: [f32; 3],
    pub color: u32,
    pub overlay_color: u32,
    pub visible: bool,
    pub initialized: bool,
    fes_block_name: Vec<u8>,
    fes_object_prefix: Vec<u8>,
    fes_bank: u32,
    fes_parameters: [i32; 512],
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ParticleSnapshot {
    pub frame: u64,
    pub objects: usize,
    pub points: Vec<ParticlePoint>,
}

#[derive(Debug)]
struct ParticleObjectState {
    program_name: String,
    image_name: String,
    grid_count: u32,
    position: Point,
    color: Color,
    depth_bits: u32,
    opacity: u32,
    enabled: bool,
    parameters: [u8; 128],
    transform_source: u32,
    coordinate_group: u32,
    elapsed: u32,
    points: Vec<ParticlePoint>,
    runner: Option<particle_vm::ParticleVm>,
    plugin: Option<particle_plugin::ParticlePlugin>,
    texture: Option<RgbaFrame>,
    rendered: Option<particle_vm::ParticleRenderSnapshot>,
    external_draw: u32,
    creation_config: Vec<u8>,
    external_extent_bound: bool,
}

impl ParticleObjectState {
    fn new(
        program_name: String,
        image_name: String,
        grid_count: u32,
        parameters: [u8; 128],
    ) -> Self {
        Self {
            program_name,
            image_name,
            grid_count,
            position: Point::default(),
            color: Color::rgba(255, 255, 255, 255),
            depth_bits: 0,
            opacity: 256,
            enabled: false,
            parameters,
            transform_source: u32::MAX,
            coordinate_group: 0,
            elapsed: 0,
            points: Vec::new(),
            runner: None,
            plugin: None,
            texture: None,
            rendered: None,
            external_draw: 0,
            creation_config: Vec::new(),
            external_extent_bound: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct SoundChannelState {
    resource: Option<String>,
    handle: Option<u64>,
    looped: bool,
    paused: bool,
    selector: u32,
    duration_frames: u32,
    repeat_remaining: u32,
    volume: u32,
    pan: i32,
    volume_fade: Option<audio_control::AudioFade>,
    pan_fade: Option<audio_control::AudioFade>,
    fade_clock: u32,
    #[serde(with = "audio_decode::checkpoint::optional")]
    prepared_audio: Option<Arc<audio_decode::DecodedAudio>>,
    task: sound_task::SoundTaskChannel,
}

#[derive(Debug, Clone, PartialEq, Eq, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct SoundGroup {
    key: u32,
    id: u32,
    label: Vec<u8>,
    range: [i32; 2],
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct DesktopSoundPart {
    xml_key: Vec<u8>,
    name: Vec<u8>,
    type_name: Vec<u8>,
    file_prefix: Vec<u8>,
    archive: Vec<u8>,
    id: i32,
    same_id: i32,
    master_volume: i32,
    volume: i32,
    voice_volume: i32,
    skip_volume: i32,
    system_voice: i32,
    play_enabled: i32,
    mute: i32,
    voice_mute: i32,
    children: Vec<DesktopSoundPart>,
}

#[derive(Debug, Clone, Default)]
struct DesktopSoundConfig {
    source_loaded: bool,
    parts: Vec<DesktopSoundPart>,
    master_volume: Option<i32>,
    master_play_enabled: Option<i32>,
    master_system_voice: Option<i32>,
    master_voice_volume: Option<i32>,
    master_skip_volume: Option<i32>,
    master_mute: i32,
    master_voice_mute: i32,
    user_overrides: BTreeMap<Vec<u8>, DesktopSoundOverride>,
}

#[derive(Debug, Clone, Copy, Default)]
struct DesktopSoundOverride {
    volume: Option<i32>,
    voice_volume: Option<i32>,
    mute: Option<i32>,
}

#[derive(Debug, Clone)]
struct SoundXmlNode {
    name: Vec<u8>,
    text: Vec<u8>,
    children: Vec<SoundXmlNode>,
}

#[derive(Debug, Clone)]
struct DesktopImageResource {
    image: RgbaFrame,
    geometry: image_manager::ResourceGeometry,
    owner: Option<u32>,
    references: usize,
    has_source_layer: bool,
    updated: bool,
    uses_alpha: bool,
    auxiliary: u32,
    attributes: BTreeMap<u32, [u32; 5]>,
}

use cs2_runtime::backlog::Backlog as DesktopBacklog;

#[derive(Debug, Clone)]
struct DesktopRecordStack {
    stride: u32,
    count: u32,
    growth: u32,
    capacity: u32,
    bytes: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct InputRegionState {
    words: [u32; 15],
}

impl InputRegionState {
    fn id(self) -> u32 {
        self.words[0]
    }

    fn bounds(self) -> [i32; 4] {
        self.words[3..7]
            .iter()
            .map(|value| *value as i32)
            .collect::<Vec<_>>()
            .try_into()
            .expect("input region bounds contain four values")
    }

    fn contains(self, point: [i32; 2]) -> bool {
        let [left, top, right, bottom] = self.bounds();
        left <= point[0] && point[0] < right && top <= point[1] && point[1] < bottom
    }
}

#[derive(Debug, Clone)]
struct DesktopInputTask {
    owner_id: Option<u32>,
    alive: bool,
    regions: Vec<InputRegionState>,
    key_masks: [[u32; 4]; 80],
    key_enabled: [bool; 80],
    pointer_position: [i32; 2],
    cursor_requested: u32,
    auto_hide: u32,
    idle_limit: u32,
    wheel_repeat: u32,
    selected_region: Option<u32>,
}

impl Default for DesktopInputTask {
    fn default() -> Self {
        Self {
            owner_id: None,
            alive: true,
            regions: Vec::new(),
            key_masks: [[0; 4]; 80],
            key_enabled: [true; 80],
            pointer_position: [0; 2],
            cursor_requested: 1,
            auto_hide: 0,
            idle_limit: 0,
            wheel_repeat: 0,
            selected_region: None,
        }
    }
}

impl Default for SoundChannelState {
    fn default() -> Self {
        Self {
            resource: None,
            handle: None,
            looped: false,
            paused: false,
            selector: 1,
            duration_frames: 0,
            repeat_remaining: 0,
            volume: 100,
            pan: 0,
            volume_fade: None,
            pan_fade: None,
            fade_clock: 0,
            prepared_audio: None,
            task: sound_task::SoundTaskChannel::default(),
        }
    }
}

impl DesktopSoundConfig {
    fn system_voice_enabled(&self, path: &[i32]) -> i32 {
        self.path_value(path, 1, |part| part.system_voice)
    }

    fn play_enabled(&self, path: &[i32]) -> i32 {
        self.path_value(path, 1, |part| part.play_enabled)
    }

    fn volume(&self, path: &[i32]) -> i32 {
        self.path_value(path, 100, |part| part.volume)
    }

    fn group_part(&self, name: &[u8]) -> Option<&DesktopSoundPart> {
        self.find_path(name, SoundMatch::Name).last().copied()
    }

    fn type_part(&self, name: &[u8]) -> Option<&DesktopSoundPart> {
        self.find_path(name, SoundMatch::Type).last().copied()
    }

    fn file_part(&self, name: &[u8]) -> Option<&DesktopSoundPart> {
        self.find_path(name, SoundMatch::Prefix).last().copied()
    }

    fn group_volume(&self, name: &[u8]) -> i32 {
        self.group_part(name).map_or(100, |part| part.volume)
    }

    fn group_voice_volume(&self, name: &[u8]) -> i32 {
        self.group_part(name).map_or(100, |part| part.voice_volume)
    }

    fn effective_file_volume(&self, name: &[u8], voice: bool) -> Option<i32> {
        if self.master_mute != 0 && self.master_mute != -1 {
            return Some(0);
        }
        let mut volume = 100_i32;
        for part in self.find_path(name, SoundMatch::Prefix).iter().rev() {
            volume = if part.mute == 0 && (!voice || part.voice_mute == 0) {
                let value = if voice {
                    part.voice_volume
                } else {
                    part.volume
                };
                value.wrapping_mul(volume) / 100
            } else {
                0
            };
            volume = part.master_volume.wrapping_mul(volume) / 100;
        }
        let volume = self
            .user_overrides
            .get(&normalize_sound_name(name))
            .and_then(|override_value| {
                if voice {
                    override_value.voice_volume
                } else {
                    override_value.volume
                }
            })
            .unwrap_or(volume);
        self.master_volume
            .map(|master| master.wrapping_mul(volume) / 100)
    }

    fn override_value(&self, name: &[u8], field: u32) -> Option<i32> {
        let value = self.user_overrides.get(&normalize_sound_name(name))?;
        match field {
            0 => value.volume,
            1 => value.voice_volume,
            2 => value.mute,
            _ => None,
        }
    }

    fn set_override(&mut self, name: &[u8], field: u32, value: i32) {
        let entry = self
            .user_overrides
            .entry(normalize_sound_name(name))
            .or_default();
        match field {
            0 => entry.volume = Some(value),
            1 => entry.voice_volume = Some(value),
            2 => entry.mute = Some(value),
            _ => {}
        }
    }

    fn load_overrides(&mut self, root: &Path) {
        let Ok(bytes) = fs::read(root.join("soundconf.override")) else {
            return;
        };
        if bytes.get(..8) != Some(b"CS2SOVR\0") || bytes.len() < 12 {
            return;
        }
        let count = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
        let mut cursor = 12usize;
        for _ in 0..count {
            let Some(length_bytes) = bytes.get(cursor..cursor + 4) else {
                break;
            };
            let length = u32::from_le_bytes(length_bytes.try_into().unwrap()) as usize;
            cursor += 4;
            let Some(name) = bytes.get(cursor..cursor + length) else {
                break;
            };
            cursor += length;
            let Some(fields) = bytes.get(cursor..cursor + 12) else {
                break;
            };
            cursor += 12;
            let mut override_value = DesktopSoundOverride::default();
            for (slot, target) in [
                (0, &mut override_value.volume),
                (1, &mut override_value.voice_volume),
                (2, &mut override_value.mute),
            ] {
                let value = i32::from_le_bytes(
                    fields[slot * 4..slot * 4 + 4]
                        .try_into()
                        .expect("sound override field has four bytes"),
                );
                if value != i32::MIN {
                    *target = Some(value);
                }
            }
            self.user_overrides
                .insert(normalize_sound_name(name), override_value);
        }
    }

    fn save_overrides(&self, root: &Path) -> Result<()> {
        let mut bytes = Vec::from(&b"CS2SOVR\0"[..]);
        bytes.extend_from_slice(
            &u32::try_from(self.user_overrides.len())
                .map_err(|_| anyhow::anyhow!("sound override count exceeds DWORD"))?
                .to_le_bytes(),
        );
        for (name, value) in &self.user_overrides {
            bytes.extend_from_slice(
                &u32::try_from(name.len())
                    .map_err(|_| anyhow::anyhow!("sound override name exceeds DWORD"))?
                    .to_le_bytes(),
            );
            bytes.extend_from_slice(name);
            for field in [value.volume, value.voice_volume, value.mute] {
                bytes.extend_from_slice(&field.unwrap_or(i32::MIN).to_le_bytes());
            }
        }
        fs::create_dir_all(root)?;
        fs::write(root.join("soundconf.override"), bytes)?;
        Ok(())
    }

    fn path_value(&self, path: &[i32], default: i32, value: fn(&DesktopSoundPart) -> i32) -> i32 {
        let mut parts = self.parts.as_slice();
        let mut result = default;
        for id in path {
            result = default;
            if let Some(part) = parts.iter().find(|part| part.id == *id) {
                result = value(part);
                parts = &part.children;
            }
        }
        result
    }

    fn find_path(&self, name: &[u8], mode: SoundMatch) -> Vec<&DesktopSoundPart> {
        let name = trim_nul(name);
        let name = normalize_sound_name(name);
        let mut stack = self
            .parts
            .iter()
            .rev()
            .map(|part| (part, 0usize))
            .collect::<Vec<_>>();
        let mut path = Vec::new();
        while let Some((part, depth)) = stack.pop() {
            path.truncate(depth);
            path.push(part);
            let key = normalize_sound_name(match mode {
                SoundMatch::Name => &part.name,
                SoundMatch::Type => &part.type_name,
                SoundMatch::Prefix => &part.file_prefix,
            });
            let matched = if matches!(mode, SoundMatch::Prefix) {
                !key.is_empty() && name.starts_with(&key)
            } else {
                name == key
            };
            if matched {
                return path;
            }
            stack.extend(part.children.iter().rev().map(|part| (part, depth + 1)));
        }
        Vec::new()
    }
}

#[derive(Debug, Clone, Copy)]
enum SoundMatch {
    Name,
    Type,
    Prefix,
}

impl SoundChannelState {
    fn is_playing(&self) -> bool {
        self.handle.is_some() && !self.paused
    }

    fn release(&mut self) {
        self.handle = None;
        self.paused = false;
        self.resource = None;
        self.looped = false;
        self.duration_frames = 0;
        self.repeat_remaining = 0;
        self.volume_fade = None;
        self.pan_fade = None;
        self.prepared_audio = None;
        let requested_volume = self.task.requested_volume;
        self.task = sound_task::SoundTaskChannel::default();
        self.task.requested_volume = requested_volume;
    }

    fn request_volume(&self) -> f32 {
        (self.volume as i32 as f32 / 100.0).clamp(0.0, 1.0)
    }

    fn request_pan(&self) -> f32 {
        (self.pan as f32 / 100.0).clamp(-1.0, 1.0)
    }
}

fn native_play_looped(count: u32) -> bool {
    count == 0
}

fn should_resume_playback(state: &SoundChannelState) -> bool {
    state.paused && state.handle.is_some()
}

#[derive(Debug, Clone)]
struct FesRuntime {
    driver: Option<fes_driver::FesScriptDriver>,
    document: FesDocument,
    section: usize,
    line: usize,
    wait_frames: u32,
    exit_code: Option<i32>,
    values: BTreeMap<i32, i32>,
    named: BTreeMap<String, i32>,
    input_mode: bool,
    continuous_input: bool,
    media: BTreeMap<String, FesMediaLayer>,
    planes: BTreeMap<String, FesPlaneLayer>,
    animation_random_state: u32,
    focused_object: Option<String>,
    event_return: Option<(usize, usize, u32)>,
    event_target: Option<String>,
    event_bank: Option<i32>,
}

#[derive(Debug, Clone)]
struct FesMediaLayer {
    navigation: fes_driver::main_flow::ImageNavigation,
    metadata: fes_driver::main_flow::GeometryMetadata,
    motion: Option<[u32; 36]>,
    name: String,
    kind: String,
    file: Option<String>,
    source: fes_media::ImageSourceState,
    frame_ids: Vec<u32>,
    selected_id: usize,
    plane: Option<String>,
    position: Point,
    priority: i32,
    clip: Option<Rect>,
    color: Color,
    visible: bool,
    enabled: bool,
    opacity: u8,
    image: Option<RgbaFrame>,
    image_offset: [i32; 2],
    image_base: [i32; 2],
    animation: Option<ImageAnimation>,
    animation_request: fes_media::ImageAnimationRequest,
    animation_display: Option<DisplayState>,
    fade: Option<(i32, i32, i32, i32)>,
}

impl FesMediaLayer {
    fn is_drawable(&self) -> bool {
        self.file.is_some() || self.color.a != 0
    }

    fn draw_position(&self, planes: &BTreeMap<String, FesPlaneLayer>) -> Point {
        let (plane_position, plane_vram) = self
            .plane
            .as_ref()
            .and_then(|name| planes.get(name))
            .map(|plane| (plane.position, plane.vram))
            .unwrap_or_default();
        let animation_position = self
            .animation_display
            .map(|display| Point::new(display.x, display.y))
            .unwrap_or_default();
        Point::new(
            plane_position
                .x
                .saturating_add(self.position.x)
                .saturating_add(animation_position.x)
                .saturating_add(self.image_offset[0])
                .saturating_sub(self.image_base[0])
                .saturating_sub(plane_vram[0]),
            plane_position
                .y
                .saturating_add(self.position.y)
                .saturating_add(animation_position.y)
                .saturating_add(self.image_offset[1])
                .saturating_sub(self.image_base[1])
                .saturating_sub(plane_vram[1]),
        )
    }
}

#[derive(Debug, Clone)]
struct FesPlaneLayer {
    expose: u32,
    metadata: fes_driver::main_flow::GeometryMetadata,
    motion: Option<[u32; 36]>,
    priority: i32,
    position: Point,
    size: [u32; 2],
    vram: [i32; 2],
    visible: bool,
    opacity: u8,
}

impl Default for FesPlaneLayer {
    fn default() -> Self {
        Self {
            expose: 0,
            metadata: Default::default(),
            motion: None,
            priority: 0,
            position: Point::default(),
            size: [0; 2],
            vram: [0; 2],
            visible: false,
            opacity: 255,
        }
    }
}

fn parse_fes_media(
    document: &FesDocument,
) -> (
    BTreeMap<String, FesMediaLayer>,
    BTreeMap<String, FesPlaneLayer>,
) {
    let mut declarations = BTreeMap::new();
    for section in &document.sections {
        if !section.header.eq_ignore_ascii_case(b"DEFINE") {
            continue;
        }
        for line in &section.lines {
            let text = String::from_utf8_lossy(&line.bytes);
            let mut parts = text.split_ascii_whitespace();
            let Some(kind) = parts.next() else {
                continue;
            };
            let Some(declaration) = parts.next() else {
                continue;
            };
            let (name, count) = declaration
                .strip_suffix(']')
                .and_then(|value| value.rsplit_once('['))
                .and_then(|(name, count)| count.parse::<usize>().ok().map(|count| (name, count)))
                .unwrap_or((declaration, 1));
            for index in 0..count {
                let object_name = if count == 1 {
                    name.to_owned()
                } else {
                    format!("{name}[{index}]")
                };
                declarations.insert(object_name, kind.to_ascii_uppercase());
            }
        }
    }

    let mut media = BTreeMap::new();
    let mut planes = BTreeMap::new();
    for (name, kind) in &declarations {
        if matches!(kind.as_str(), "PLANE" | "CPLANE") {
            planes.insert(name.clone(), FesPlaneLayer::default());
        } else {
            media.insert(
                name.clone(),
                FesMediaLayer {
                    navigation: Default::default(),
                    metadata: Default::default(),
                    motion: None,
                    name: name.clone(),
                    kind: kind.clone(),
                    file: None,
                    source: Default::default(),
                    frame_ids: Vec::new(),
                    selected_id: 0,
                    plane: None,
                    position: Point::default(),
                    priority: 0,
                    clip: None,
                    color: Color::TRANSPARENT,
                    visible: false,
                    enabled: true,
                    opacity: 255,
                    image: None,
                    image_offset: [0; 2],
                    image_base: [0; 2],
                    animation: None,
                    animation_request: Default::default(),
                    animation_display: None,
                    fade: None,
                },
            );
        }
    }

    for section in &document.sections {
        let Some(columns) = section.header.strip_prefix(b"OBJECT ").map(|bytes| {
            String::from_utf8_lossy(bytes)
                .split_ascii_whitespace()
                .map(str::to_owned)
                .collect::<Vec<_>>()
        }) else {
            continue;
        };
        if columns.is_empty() {
            continue;
        }
        for line in &section.lines {
            let text = String::from_utf8_lossy(&line.bytes);
            let mut values = text.split_ascii_whitespace();
            let Some(name) = values.next() else {
                continue;
            };
            if !declarations.contains_key(name) {
                continue;
            }
            let fields = columns
                .iter()
                .zip(values)
                .map(|(key, value)| (key.as_str(), value))
                .collect::<BTreeMap<_, _>>();
            if let Some(plane) = planes.get_mut(name) {
                if let Some(size) = fields.get("SIZE").and_then(|value| parse_fes_pair(value)) {
                    plane.size = [size[0].max(0) as u32, size[1].max(0) as u32];
                }
                if let Some(vram) = fields.get("VRAM").and_then(|value| parse_fes_pair(value)) {
                    plane.vram = vram;
                }
                plane.visible = fields
                    .get("DISP")
                    .and_then(|value| value.parse::<i32>().ok())
                    .is_some_and(|value| value != 0);
            }
            if let Some(layer) = media.get_mut(name) {
                if let Some(file) = fields.get("FILE") {
                    layer.file = Some((*file).to_owned());
                }
                if let Some(plane) = fields.get("PL") {
                    layer.plane = Some((*plane).to_owned());
                }
                if let Some(position) = fields.get("POS").and_then(|value| parse_fes_pair(value)) {
                    layer.position = Point::new(position[0], position[1]);
                }
                if let Some(size) = fields.get("SIZE").and_then(|value| parse_fes_pair(value)) {
                    layer.clip = Some(Rect::new(
                        0,
                        0,
                        size[0].max(0) as u32,
                        size[1].max(0) as u32,
                    ));
                    layer.navigation.set_size(size);
                }
                if let Some(priority) = fields.get("PRI").and_then(|value| value.parse().ok()) {
                    layer.priority = priority;
                }
                if let Some(color) = fields.get("COLOR").and_then(|value| parse_fes_color(value)) {
                    layer.color = color;
                }
                if let Some(display) = fields
                    .get("DISP")
                    .and_then(|value| value.parse::<i32>().ok())
                {
                    layer.visible = display != 0;
                }
                if let Some(enabled) = fields
                    .get("ENABLE")
                    .and_then(|value| value.parse::<i32>().ok())
                {
                    layer.enabled = enabled != 0;
                }
                if let Some(ids) = fields
                    .iter()
                    .filter_map(|(key, value)| {
                        key.strip_prefix("ID.")
                            .and_then(|index| index.parse::<usize>().ok())
                            .and_then(|index| value.parse::<u32>().ok().map(|id| (index, id)))
                    })
                    .max_by_key(|(index, _)| *index)
                {
                    layer.frame_ids.resize(ids.0 + 1, 0);
                    for (key, value) in &fields {
                        if let Some(index) = key
                            .strip_prefix("ID.")
                            .and_then(|index| index.parse::<usize>().ok())
                        {
                            if let Ok(id) = value.parse::<u32>() {
                                if let Some(frame_id) = layer.frame_ids.get_mut(index) {
                                    *frame_id = id;
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    (media, planes)
}

fn parse_fes_pair(value: &str) -> Option<[i32; 2]> {
    let (left, right) = value.split_once(',')?;
    Some([left.parse().ok()?, right.parse().ok()?])
}

fn parse_fes_color(value: &str) -> Option<Color> {
    let value = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))?;
    let value = u32::from_str_radix(value, 16).ok()?;
    Some(Color::rgba(
        (value >> 16) as u8,
        (value >> 8) as u8,
        value as u8,
        (value >> 24) as u8,
    ))
}

fn parse_fes_position(value: Option<&&str>, previous: i32) -> i32 {
    let Some(value) = value else {
        return 0;
    };
    if *value == "@" {
        return previous;
    }
    if let Some((base, delta)) = value.split_once('+') {
        if base == "@" {
            return previous.saturating_add(delta.parse::<i32>().unwrap_or(0));
        }
    }
    if let Some((base, delta)) = value.split_once('-') {
        if base == "@" {
            return previous.saturating_sub(delta.parse::<i32>().unwrap_or(0));
        }
    }
    value.parse().unwrap_or(0)
}

fn split_fes_object_bank(value: &str) -> (&str, i32) {
    let Some((name, suffix)) = value.rsplit_once('[') else {
        return (value, 0);
    };
    let Some(bank) = suffix
        .strip_suffix(']')
        .and_then(|value| value.parse().ok())
    else {
        return (value, 0);
    };
    (name, bank)
}

fn solid_frame(width: u32, height: u32, color: Color) -> Option<RgbaFrame> {
    if width == 0 || height == 0 {
        return None;
    }
    let pixel_count = usize::try_from(width)
        .ok()?
        .checked_mul(usize::try_from(height).ok()?)?;
    let mut pixels = Vec::new();
    pixels.try_reserve_exact(pixel_count.checked_mul(4)?).ok()?;
    pixels.resize(pixel_count.checked_mul(4)?, 0);
    for pixel in pixels.chunks_exact_mut(4) {
        pixel.copy_from_slice(&[color.r, color.g, color.b, color.a]);
    }
    RgbaFrame::from_rgba8(width, height, pixels).ok()
}

fn load_fes_hg_image(
    resources: &ResourceIndex,
    resource: &str,
    frame_id: u32,
) -> Result<(RgbaFrame, [i32; 2], [i32; 2])> {
    let (_, bytes) = read_fes_media_asset(resources, resource, "hg3")
        .context("FES image resource was not found")?;
    let file = HgFile::parse(&bytes).context("parse FES HG image")?;
    let frame = file
        .frames
        .iter()
        .find(|frame| frame.id == frame_id)
        .with_context(|| format!("FES HG image has no frame with ID {frame_id}"))?;
    let pixels = frame
        .decode_pixels_rgba8(&bytes)
        .map_err(|error| anyhow::anyhow!(error))?;
    let image = RgbaFrame::from_rgba8(frame.width, frame.height, pixels)
        .map_err(|error| anyhow::anyhow!(error))?;
    Ok((image, frame.offset, frame.base))
}

fn load_fes_media_image(layer: &mut FesMediaLayer, resources: &ResourceIndex) -> Result<()> {
    let frame_id = layer
        .animation_display
        .filter(|display| display.frame_id >= 0)
        .map(|display| display.frame_id as u32)
        .or_else(|| layer.frame_ids.get(layer.selected_id).copied())
        .unwrap_or(layer.selected_id as u32);
    load_fes_media_frame(layer, resources, frame_id)
}

fn load_fes_media_frame(
    layer: &mut FesMediaLayer,
    resources: &ResourceIndex,
    frame_id: u32,
) -> Result<()> {
    let resource = layer
        .file
        .as_deref()
        .context("FES image has no FILE resource")?;
    let (image, offset, base) = load_fes_hg_image(resources, resource, frame_id)?;
    let size = layer.clip.map(|clip| [clip.width as i32, clip.height as i32])
        .unwrap_or([image.width() as i32, image.height() as i32]);
    layer.navigation.set_size(size);
    layer.image = Some(image);
    layer.image_offset = offset;
    layer.image_base = base;
    Ok(())
}

fn read_fes_media_asset(
    resources: &ResourceIndex,
    resource: &str,
    extension: &str,
) -> Result<(String, Vec<u8>)> {
    let resource = resource.replace('\\', "/");
    let suffix = format!(".{extension}");
    let candidates = if resource.to_ascii_lowercase().ends_with(&suffix) {
        vec![resource.clone()]
    } else {
        let mut candidates = vec![
            format!("system/image/{resource}{suffix}"),
            format!("image/{resource}{suffix}"),
            format!("{resource}{suffix}"),
        ];
        if extension.eq_ignore_ascii_case("hg3") {
            candidates.push(format!("{resource}.hg2"));
        }
        candidates
    };
    let mut errors = Vec::new();
    for candidate in candidates {
        match read_entry_with_fallback(resources, &candidate) {
            Ok(bytes) => return Ok((candidate, bytes)),
            Err(error) => errors.push(format!("{candidate}: {error}")),
        }
    }
    bail!(
        "FES {extension} resource {resource:?} was not found ({})",
        errors.join("; ")
    )
}

fn load_fes_animation(
    resources: &ResourceIndex,
    animation_resource: &str,
    image_resource: &str,
) -> Result<ImageAnimation> {
    let (_, animation_bytes) = read_fes_media_asset(resources, animation_resource, "anm")?;
    let parsed_animation = AnmFile::parse(&animation_bytes).context("parse FES ANM animation")?;
    let max_image_id = read_fes_media_asset(resources, image_resource, "hg3")
        .ok()
        .and_then(|(_, bytes)| HgFile::parse(&bytes).ok())
        .and_then(|file| file.frames.iter().map(|frame| frame.id).max())
        .or_else(|| {
            parsed_animation
                .instructions
                .iter()
                .filter(|instruction| instruction.opcode == 0)
                .map(|instruction| instruction.operands[0].value)
                .max()
        })
        .unwrap_or(0);
    let mut animation = ImageAnimation::default();
    animation
        .load(&animation_bytes, max_image_id)
        .context("initialize FES image animation")?;
    animation.reset_time_on_update = true;
    Ok(animation)
}

#[derive(Debug, Clone)]
struct CstRuntime {
    document: CstDocument,
    scene_name: String,
    input: usize,
    record: usize,
    waiting: bool,
    advance_input_on_resume: bool,
    finished: bool,
    wait_frames: u32,
    choices: Option<Vec<CstChoice>>,
    pending_choice: Option<usize>,
    pending_scene: Option<String>,
    speaker: Vec<u8>,
    text: Vec<u8>,
    encoding: ScriptTextEncoding,
    call_stack: Vec<CstCallFrame>,
    strings: BTreeMap<i32, Vec<u8>>,
    localization: Option<CstlTable>,
    language: String,
    message_fragment: usize,
}

#[derive(Debug, Clone)]
struct CstCallFrame {
    document: CstDocument,
    scene_name: String,
    input: usize,
    record: usize,
    speaker: Vec<u8>,
    text: Vec<u8>,
    localization: Option<CstlTable>,
    language: String,
    message_fragment: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CstChoice {
    id: i32,
    target: String,
    text: String,
    source_text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum CstImageKind {
    Background,
    Cg,
    Effect,
    Foreground,
    FrontWindow,
    Particle,
}

impl CstImageKind {
    fn parse(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "bg" => Some(Self::Background),
            "cg" => Some(Self::Cg),
            "eg" => Some(Self::Effect),
            "fg" => Some(Self::Foreground),
            "fw" => Some(Self::FrontWindow),
            "pl" => Some(Self::Particle),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Background => "bg",
            Self::Cg => "cg",
            Self::Effect => "eg",
            Self::Foreground => "fg",
            Self::FrontWindow => "fw",
            Self::Particle => "pl",
        }
    }
}

#[derive(Debug, Clone)]
struct CstImageLayer {
    image: RgbaFrame,
    position: Point,
    offset: [i32; 2],
    base: [i32; 2],
    canvas_size: [u32; 2],
    opacity: u8,
}

impl CstImageLayer {
    fn draw_position(&self) -> Point {
        Point::new(
            self.position
                .x
                .saturating_add(self.offset[0])
                .saturating_sub(self.base[0]),
            self.position
                .y
                .saturating_add(self.offset[1])
                .saturating_sub(self.base[1]),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CstRegistration {
    name: [u8; 32],
    value: u32,
    attachments: Vec<[u8; 72]>,
    data: Vec<u8>,
}

#[derive(Debug, Clone, Default)]
struct CstContextState {
    document: Option<CstDocument>,
    position: [i32; 2],
    registrations: Vec<CstRegistration>,
    history: Vec<CstRegistration>,
    languages: Vec<Vec<u8>>,
    localization: Option<cs2_formats::CstlTable>,
    encoding: ScriptTextEncoding,
    source_name: Vec<u8>,
    source_line: u32,
}

impl CstContextState {
    fn table(&self, index: u32) -> &[CstRegistration] {
        if index == 0 {
            &self.registrations
        } else {
            &self.history
        }
    }

    fn replace_table(&mut self, index: u32, entries: Vec<CstRegistration>) {
        if index == 0 {
            self.registrations = entries;
        } else {
            self.history = entries;
        }
    }
}

struct MotionTaskState {
    name: Vec<u8>,
    group: i32,
    parameters: [u32; 256],
    saved_data: Vec<u8>,
    snapshot_flags: u32,
    external_value: u32,
    origin: u32,
    elapsed: u32,
    running: i32,
    finishing: i32,
    runner: motion_vm::MotionRunner,
}

struct MotionControllerUpdate {
    now: u32,
    skip_unstarted: bool,
    finishing: bool,
    ids: Vec<u32>,
    cursor: usize,
    updated: bool,
    speed_bits: u32,
    step_done: bool,
}

struct MotionControllerState {
    base_properties: [u32; 36],
    properties: [u32; 36],
    speed_bits: u32,
    external_value: u32,
    next_task_id: u32,
    tasks: BTreeMap<u32, MotionTaskState>,
    pending: Option<MotionControllerUpdate>,
}

impl Default for MotionControllerState {
    fn default() -> Self {
        let mut properties = [0_u32; 36];
        for index in [9, 14, 19, 24, 25, 26, 27] {
            properties[index] = 1.0_f32.to_bits();
        }
        properties[28] = u32::MAX;
        properties[35] = 1;
        Self {
            base_properties: properties,
            properties,
            speed_bits: 1.0_f32.to_bits(),
            external_value: 0,
            next_task_id: 1,
            tasks: BTreeMap::new(),
            pending: None,
        }
    }
}

#[derive(Debug, Clone)]
struct MotionSnapshotState {
    controller: u32,
    storage: Vec<u8>,
    size: usize,
}

impl CstRuntime {
    fn new(
        document: CstDocument,
        scene_name: String,
        encoding: ScriptTextEncoding,
        localization: Option<CstlTable>,
    ) -> Self {
        let record = document.inputs.first().map_or(0, |range| range.start);
        Self {
            document,
            scene_name,
            input: 0,
            record,
            waiting: false,
            advance_input_on_resume: false,
            finished: false,
            wait_frames: 0,
            choices: None,
            pending_choice: None,
            pending_scene: None,
            speaker: Vec::new(),
            text: Vec::new(),
            encoding,
            call_stack: Vec::new(),
            strings: BTreeMap::new(),
            language: localization
                .as_ref()
                .and_then(|table| table.languages.first())
                .cloned()
                .unwrap_or_default(),
            localization,
            message_fragment: 0,
        }
    }

    fn resume(&mut self) {
        if self.waiting {
            self.waiting = false;
            if let Some(choice) = self.pending_choice.take() {
                if let Some(choices) = self.choices.take() {
                    if let Some(choice) = choices.get(choice) {
                        self.pending_scene = Some(choice.target.clone());
                    }
                }
            }
            if self.advance_input_on_resume {
                self.input = self.input.saturating_add(1);
                self.record = self
                    .document
                    .inputs
                    .get(self.input)
                    .map_or(self.document.records.len(), |range| range.start);
            }
            self.advance_input_on_resume = false;
            self.message_fragment = 0;
        }
    }

    fn advance(
        &mut self,
        frame: u64,
        resources: &ResourceIndex,
        shared_values: &mut Vec<u32>,
        images: &mut BTreeMap<(CstImageKind, i32), Vec<CstImageLayer>>,
        events: &mut Vec<LogicEvent>,
    ) -> Result<()> {
        if self.waiting || self.finished {
            return Ok(());
        }
        if self.wait_frames > 0 {
            self.wait_frames -= 1;
            return Ok(());
        }
        loop {
            if let Some(target) = self.pending_scene.take() {
                let target = self.resolve_scene_target(&target, resources)?;
                let bytes = read_entry_with_fallback(resources, &target)?;
                self.document = CstDocument::parse(&bytes)?;
                self.scene_name = target.clone();
                self.input = 0;
                self.record = self.document.inputs.first().map_or(0, |range| range.start);
                self.finished = false;
                self.speaker.clear();
                self.text.clear();
                events.push(LogicEvent {
                    frame,
                    kind: "cst-choice-transfer".into(),
                    detail: target,
                });
            }
            let Some(range) = self.document.inputs.get(self.input) else {
                if self.restore_call() {
                    continue;
                }
                self.finished = true;
                events.push(LogicEvent {
                    frame,
                    kind: "cst-finished".into(),
                    detail: format!("inputs={}", self.document.inputs.len()),
                });
                return Ok(());
            };
            if self.record >= range.end {
                if self.input + 1 < self.document.inputs.len() {
                    self.waiting = true;
                    self.advance_input_on_resume = true;
                    events.push(LogicEvent {
                        frame,
                        kind: "cst-wait".into(),
                        detail: format!("input={}", self.input),
                    });
                } else {
                    if self.restore_call() {
                        continue;
                    }
                    self.finished = true;
                    events.push(LogicEvent {
                        frame,
                        kind: "cst-finished".into(),
                        detail: format!("inputs={}", self.document.inputs.len()),
                    });
                }
                return Ok(());
            }
            let record = self.document.records[self.record].clone();
            self.record += 1;
            match record.kind {
                0x02 | 0x03 => {
                    self.waiting = true;
                    self.advance_input_on_resume = true;
                    events.push(LogicEvent {
                        frame,
                        kind: "cst-wait".into(),
                        detail: format!("input={} kind={:#04x}", self.input, record.kind),
                    });
                    return Ok(());
                }
                0x20 => {
                    self.text = self.expand_strings(&record.text);
                    self.message_fragment = self.message_fragment.saturating_add(1);
                    let (speaker, text) = self.display_text();
                    events.push(LogicEvent {
                        frame,
                        kind: "cst-message".into(),
                        detail: format!(
                            "speaker={:?} text={:?}",
                            speaker,
                            text
                        ),
                    });
                }
                0x21 => {
                    self.speaker = self.expand_strings(&record.text);
                    let (speaker, _) = self.display_text();
                    events.push(LogicEvent {
                        frame,
                        kind: "cst-speaker".into(),
                        detail: format!(
                            "{:?}",
                            speaker
                        ),
                    });
                }
                0x30 => {
                    let command = self.expand_strings(&record.text);
                    if self.execute_command(
                        &command,
                        resources,
                        shared_values,
                        images,
                        frame,
                        events,
                    )? {
                        continue;
                    }
                    events.push(LogicEvent {
                        frame,
                        kind: "cst-command".into(),
                        detail: self.encoding.decode(&command),
                    });
                }
                0xf0 | 0xf1 => {
                    events.push(LogicEvent {
                        frame,
                        kind: "cst-debug".into(),
                        detail: format!(
                            "kind={:#04x} text={:?}",
                            record.kind,
                            String::from_utf8_lossy(&record.text)
                        ),
                    });
                }
                kind => {
                    events.push(LogicEvent {
                        frame,
                        kind: "cst-record".into(),
                        detail: format!(
                            "kind={kind:#04x} text={:?}",
                            self.encoding.decode(&record.text)
                        ),
                    });
                }
            }
        }
    }

    fn display_text(&self) -> (String, String) {
        if let Some(table) = self.bound_display_localization() {
            // A bound row/fragment which is empty or absent stays empty. Only
            // an absent language binding falls back to original scene bytes.
            let speaker = table.message(&self.language, self.input)
                .map_or("", |message| message.name.as_str()).to_owned();
            let text = if self.message_fragment == 0 {
                self.encoding.decode(&self.text)
            } else {
                table.segment(&self.language, self.input, self.message_fragment - 1)
                    .unwrap_or("").to_owned()
            };
            return (speaker, text);
        }
        (
            self.encoding.decode(&self.speaker),
            self.encoding.decode(&self.text),
        )
    }

    fn bound_display_localization(&self) -> Option<&CstlTable> {
        self.localization.as_ref().filter(|table|
            table.languages.iter().any(|language| language == &self.language))
    }

    fn display_encoding(&self) -> ScriptTextEncoding {
        if self.bound_display_localization().is_some() { ScriptTextEncoding::Utf8 }
        else { self.encoding }
    }

    fn execute_command(
        &mut self,
        bytes: &[u8],
        resources: &ResourceIndex,
        shared_values: &mut Vec<u32>,
        images: &mut BTreeMap<(CstImageKind, i32), Vec<CstImageLayer>>,
        frame: u64,
        events: &mut Vec<LogicEvent>,
    ) -> Result<bool> {
        let bytes = trim_ascii(bytes);
        if bytes.starts_with(b"if") {
            if let Some((condition, command)) = split_cst_condition(bytes)? {
                if eval_cst_condition(condition, shared_values)? {
                    return self.execute_command(
                        command,
                        resources,
                        shared_values,
                        images,
                        frame,
                        events,
                    );
                }
                events.push(LogicEvent {
                    frame,
                    kind: "cst-condition".into(),
                    detail: format!("false: {}", self.encoding.decode(condition)),
                });
                return Ok(true);
            }
        }
        if bytes.first() == Some(&b'#') {
            let equal = bytes
                .iter()
                .position(|&byte| byte == b'=')
                .context("CST assignment is missing '='")?;
            let variable = &bytes[..equal];
            let value = &bytes[equal + 1..];
            let index = parse_cst_integer(trim_ascii(variable).get(1..).unwrap_or_default())
                .context("CST assignment has an invalid shared variable index")?;
            if index < 0 || index as usize >= 1_000_000 {
                bail!("CST shared variable index is out of range: {index}");
            }
            let value = parse_cst_integer(trim_ascii(value))
                .context("CST assignment has an invalid integer value")?;
            let index = index as usize;
            if shared_values.len() <= index {
                shared_values.resize(index + 1, 0);
            }
            shared_values[index] = value as u32;
            events.push(LogicEvent {
                frame,
                kind: "cst-assignment".into(),
                detail: format!("#{index}={value}"),
            });
            return Ok(true);
        }
        let command = String::from_utf8_lossy(bytes);
        let mut parts = command.split_ascii_whitespace();
        let head = parts.next().unwrap_or_default();
        match head.to_ascii_lowercase().as_str() {
            "bg" | "cg" | "fg" | "eg" | "fw" | "pl" => {
                let kind = CstImageKind::parse(head).expect("matched CST image command");
                let arguments = parts.collect::<Vec<_>>();
                let Some(bank) = arguments
                    .first()
                    .and_then(|value| value.parse::<i32>().ok())
                else {
                    images.retain(|(image_kind, _), _| *image_kind != kind);
                    events.push(LogicEvent {
                        frame,
                        kind: "cst-image-clear".into(),
                        detail: format!("type={} all", kind.as_str()),
                    });
                    return Ok(true);
                };
                let key = (kind, bank);
                let asset = arguments.get(1).copied().unwrap_or_default();
                if asset.is_empty() || asset == "0" || asset.eq_ignore_ascii_case("clear") {
                    images.remove(&key);
                    events.push(LogicEvent {
                        frame,
                        kind: "cst-image-clear".into(),
                        detail: format!("type={} bank={bank}", kind.as_str()),
                    });
                    return Ok(true);
                }
                if asset.eq_ignore_ascii_case("blend") {
                    let opacity = arguments
                        .get(2)
                        .and_then(|value| parse_cst_integer(value.as_bytes()))
                        .context("CST image blend has an invalid opacity")?
                        .clamp(0, 255) as u8;
                    if let Some(image_layers) = images.get_mut(&key) {
                        for image in image_layers {
                            image.opacity = opacity;
                        }
                    }
                    return Ok(true);
                }
                if kind == CstImageKind::Particle {
                    let previous_position = images
                        .get(&key)
                        .and_then(|layers| layers.first())
                        .map(|image| image.position)
                        .unwrap_or_default();
                    let position = Point::new(
                        parse_cst_position(arguments.get(2).copied(), previous_position.x)?,
                        parse_cst_position(arguments.get(3).copied(), previous_position.y)?,
                    );
                    events.push(LogicEvent {
                        frame,
                        kind: "cst-particle-snapshot".into(),
                        detail: format!(
                            "bank={bank} resource={asset:?} position={position:?} opacity=255 points=0"
                        ),
                    });
                    return Ok(true);
                }
                let previous_position = images
                    .get(&key)
                    .and_then(|layers| layers.first())
                    .map(|image| image.position)
                    .unwrap_or_default();
                let position = Point::new(
                    parse_cst_position(arguments.get(2).copied(), previous_position.x)?,
                    parse_cst_position(arguments.get(3).copied(), previous_position.y)?,
                );
                let (resource_names, image_layers) =
                    load_cst_images(resources, kind, asset, position, self.encoding)?;
                let layer_count = image_layers.len();
                let geometry = image_layers
                    .iter()
                    .map(|image| {
                        format!(
                            "{}x{} canvas={:?} pos={:?} offset={:?} base={:?}",
                            image.image.width(),
                            image.image.height(),
                            image.canvas_size,
                            image.position,
                            image.offset,
                            image.base,
                        )
                    })
                    .collect::<Vec<_>>();
                images.insert(key, image_layers);
                events.push(LogicEvent {
                    frame,
                    kind: "cst-image".into(),
                    detail: format!(
                        "type={} bank={bank} resources={resource_names:?} layers={layer_count} geometry={geometry:?}",
                        kind.as_str(),
                    ),
                });
                return Ok(true);
            }
            "next" | "call" => {
                let target = parts
                    .next()
                    .context("CST scene transfer is missing a target")?;
                let target = self.resolve_scene_target(target, resources)?;
                let bytes = read_entry_with_fallback(resources, &target)
                    .with_context(|| format!("read CST scene target {target}"))?;
                let document = CstDocument::parse(&bytes)
                    .with_context(|| format!("parse CST scene target {target}"))?;
                if head.eq_ignore_ascii_case("call") {
                    if self.call_stack.len() >= 128 {
                        bail!("CST scene call depth exceeds 128");
                    }
                    self.call_stack.push(CstCallFrame {
                        document: self.document.clone(),
                        scene_name: self.scene_name.clone(),
                        input: self.input,
                        record: self.record,
                        speaker: self.speaker.clone(),
                        text: self.text.clone(),
                        localization: self.localization.clone(),
                        language: self.language.clone(),
                        message_fragment: self.message_fragment,
                    });
                } else {
                    self.call_stack.clear();
                }
                let record = document.inputs.first().map_or(0, |range| range.start);
                self.document = document;
                self.scene_name = target.clone();
                self.localization = load_cstl(resources, &target);
                self.language = self
                    .localization
                    .as_ref()
                    .and_then(|table| table.languages.first())
                    .cloned()
                    .unwrap_or_default();
                self.input = 0;
                self.record = record;
                self.waiting = false;
                self.advance_input_on_resume = false;
                self.finished = false;
                self.speaker.clear();
                self.text.clear();
                self.message_fragment = 0;
                events.push(LogicEvent {
                    frame,
                    kind: "cst-scene-transfer".into(),
                    detail: format!("command={} target={target}", head.to_ascii_lowercase()),
                });
                Ok(true)
            }
            "wait" => {
                let argument = parts.next();
                match argument {
                    Some(value) if value.bytes().all(|byte| byte.is_ascii_digit()) => {
                        self.wait_frames = value
                            .parse::<u32>()
                            .context("invalid CST wait frame count")?;
                        events.push(LogicEvent {
                            frame,
                            kind: "cst-time-wait".into(),
                            detail: format!("frames={}", self.wait_frames),
                        });
                    }
                    Some("key") | None => {
                        self.waiting = true;
                        self.advance_input_on_resume = false;
                        events.push(LogicEvent {
                            frame,
                            kind: "cst-wait".into(),
                            detail: "command=wait key".into(),
                        });
                    }
                    Some(kind) => bail!("unsupported CST wait kind: {kind}"),
                }
                Ok(true)
            }
            "str" | "strn" => {
                let index = parts
                    .next()
                    .and_then(|value| value.parse::<i32>().ok())
                    .context("CST string command has an invalid index")?;
                let value_start = bytes
                    .iter()
                    .enumerate()
                    .filter(|(_, byte)| byte.is_ascii_whitespace())
                    .nth(1)
                    .map(|(index, _)| index + 1)
                    .unwrap_or(bytes.len());
                let value = trim_ascii(&bytes[value_start..]);
                let value = if head.eq_ignore_ascii_case("strn") {
                    parse_cst_integer(value)
                        .context("CST strn value is not an integer")?
                        .to_string()
                        .into_bytes()
                } else {
                    value.to_vec()
                };
                self.strings.insert(index, value.clone());
                events.push(LogicEvent {
                    frame,
                    kind: "cst-string".into(),
                    detail: format!("index={index} value={:?}", self.encoding.decode(&value)),
                });
                Ok(true)
            }
            "fselect" => {
                let interface = parts.next().unwrap_or_default();
                let choices = self.collect_choices(interface, shared_values)?;
                if choices.is_empty() {
                    return Ok(true);
                }
                self.choices = Some(choices.clone());
                self.waiting = true;
                self.advance_input_on_resume = false;
                events.push(LogicEvent {
                    frame,
                    kind: "cst-choices".into(),
                    detail: choices
                        .iter()
                        .map(|choice| format!("{}:{}:{}", choice.id, choice.target, choice.text))
                        .collect::<Vec<_>>()
                        .join("|"),
                });
                Ok(true)
            }
            "return" => {
                if !self.restore_call() {
                    self.finished = true;
                    events.push(LogicEvent {
                        frame,
                        kind: "cst-finished".into(),
                        detail: "return at root scene".into(),
                    });
                } else {
                    events.push(LogicEvent {
                        frame,
                        kind: "cst-return".into(),
                        detail: self.scene_name.clone(),
                    });
                }
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    fn resolve_scene_target(&self, target: &str, resources: &ResourceIndex) -> Result<String> {
        let target = target.replace('\\', "/");
        let mut candidates = vec![target.clone()];
        if !Path::new(&target).extension().is_some() {
            candidates.push(format!("{target}.cst"));
        }
        if let Some(parent) = Path::new(&self.scene_name).parent() {
            candidates.push(parent.join(&target).to_string_lossy().replace('\\', "/"));
            if !Path::new(&target).extension().is_some() {
                candidates.push(
                    parent
                        .join(format!("{target}.cst"))
                        .to_string_lossy()
                        .replace('\\', "/"),
                );
            }
        }
        candidates.push(format!("scene/{target}"));
        if !Path::new(&target).extension().is_some() {
            candidates.push(format!("scene/{target}.cst"));
        }
        candidates
            .into_iter()
            .find(|candidate| read_entry_with_fallback(resources, candidate).is_ok())
            .context("CST scene target was not found")
    }

    fn restore_call(&mut self) -> bool {
        let Some(frame) = self.call_stack.pop() else {
            return false;
        };
        self.document = frame.document;
        self.scene_name = frame.scene_name;
        self.input = frame.input;
        self.record = frame.record;
        self.speaker = frame.speaker;
        self.text = frame.text;
        self.localization = frame.localization;
        self.language = frame.language;
        self.message_fragment = frame.message_fragment;
        self.waiting = false;
        self.advance_input_on_resume = false;
        self.finished = false;
        true
    }

    fn collect_choices(
        &mut self,
        _interface: &str,
        shared_values: &[u32],
    ) -> Result<Vec<CstChoice>> {
        let mut choices = Vec::new();
        let mut cursor = self.record;
        while let Some(record) = self.document.records.get(cursor) {
            if record.kind != 0x30 {
                break;
            }
            if choices.len() + cursor.saturating_sub(self.record) >= 4096 {
                bail!("CST choice rows exceed 4096 records");
            }
            let expanded = self.expand_strings(&record.text);
            let (command, hidden) = match split_cst_condition(&expanded)? {
                Some((condition, command)) => {
                    (command, !eval_cst_condition(condition, shared_values)?)
                }
                None => (expanded.as_slice(), false),
            };
            if hidden {
                cursor += 1;
                continue;
            }
            let (id_bytes, tail) = split_ascii_word(command);
            let Some(id) = parse_cst_integer(id_bytes) else {
                break;
            };
            let (target_bytes, text_bytes) = split_ascii_word(tail);
            if target_bytes.is_empty() || text_bytes.is_empty() {
                break;
            }
            let choice_index = choices.len();
            let target = self.encoding.decode(target_bytes);
            let base_text = self.encoding.decode(trim_ascii(text_bytes));
            let text = self
                .localization
                .as_ref()
                .and_then(|table| {
                    table
                        .option(&self.language, self.document.inputs.len(), choice_index)
                        .filter(|text| !text.is_empty())
                })
                .unwrap_or_else(|| base_text.clone());
            choices.push(CstChoice { id, target, text, source_text: base_text });
            cursor += 1;
        }
        self.record = cursor;
        Ok(choices)
    }

    fn expand_strings(&self, input: &[u8]) -> Vec<u8> {
        let mut output = Vec::with_capacity(input.len());
        let mut cursor = 0;
        while cursor < input.len() {
            if input[cursor..].starts_with(b"$str") {
                let start = cursor + 4;
                let mut end = start;
                let mut index = 0i32;
                while input.get(end).is_some_and(u8::is_ascii_digit) {
                    index = index
                        .saturating_mul(10)
                        .saturating_add(i32::from(input[end] - b'0'));
                    end += 1;
                }
                if end > start {
                    if let Some(value) = self.strings.get(&index) {
                        output.extend_from_slice(value);
                    } else {
                        output.extend_from_slice(&input[cursor..end]);
                    }
                    cursor = end;
                    continue;
                }
            }
            output.push(input[cursor]);
            cursor += 1;
        }
        output
    }
}

fn trim_ascii(mut bytes: &[u8]) -> &[u8] {
    while bytes.first().is_some_and(u8::is_ascii_whitespace) {
        bytes = &bytes[1..];
    }
    while bytes.last().is_some_and(u8::is_ascii_whitespace) {
        bytes = &bytes[..bytes.len() - 1];
    }
    bytes
}

fn parse_cst_integer(bytes: &[u8]) -> Option<i32> {
    let bytes = trim_ascii(bytes);
    let text = std::str::from_utf8(bytes).ok()?;
    if let Some(hex) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        u32::from_str_radix(hex, 16).ok().map(|value| value as i32)
    } else {
        text.parse().ok()
    }
}

fn split_ascii_word(bytes: &[u8]) -> (&[u8], &[u8]) {
    let bytes = trim_ascii(bytes);
    let end = bytes
        .iter()
        .position(u8::is_ascii_whitespace)
        .unwrap_or(bytes.len());
    (&bytes[..end], trim_ascii(&bytes[end..]))
}

fn split_cst_condition(bytes: &[u8]) -> Result<Option<(&[u8], &[u8])>> {
    if !bytes.starts_with(b"if") || !bytes.get(2).is_some_and(u8::is_ascii_whitespace) {
        return Ok(None);
    }
    let rest = trim_ascii(&bytes[2..]);
    if rest.first() != Some(&b'(') {
        bail!("CST if command is missing '('");
    }
    let mut depth = 0usize;
    for (index, byte) in rest.iter().copied().enumerate() {
        match byte {
            b'(' => depth += 1,
            b')' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Ok(Some((&rest[1..index], trim_ascii(&rest[index + 1..]))));
                }
            }
            _ => {}
        }
    }
    bail!("CST if command is missing ')'");
}

fn eval_cst_condition(expression: &[u8], shared_values: &[u32]) -> Result<bool> {
    let expression = trim_ascii(expression);
    if let Some((left, right)) = split_cst_operator(expression, b"||") {
        return Ok(
            eval_cst_condition(left, shared_values)? || eval_cst_condition(right, shared_values)?
        );
    }
    if let Some((left, right)) = split_cst_operator(expression, b"&&") {
        return Ok(
            eval_cst_condition(left, shared_values)? && eval_cst_condition(right, shared_values)?
        );
    }
    for operator in [b"==".as_slice(), b"!="] {
        if let Some((left, right)) = split_cst_operator(expression, operator) {
            let equal =
                eval_cst_value(left, shared_values)? == eval_cst_value(right, shared_values)?;
            return Ok(if operator == b"==" { equal } else { !equal });
        }
    }
    for operator in [b">=".as_slice(), b"<=", b">", b"<"] {
        if let Some((left, right)) = split_cst_operator(expression, operator) {
            let left = eval_cst_value(left, shared_values)?;
            let right = eval_cst_value(right, shared_values)?;
            return Ok(match operator {
                b">=" => left >= right,
                b"<=" => left <= right,
                b">" => left > right,
                _ => left < right,
            });
        }
    }
    Ok(eval_cst_value(expression, shared_values)? != 0)
}

fn eval_cst_value(expression: &[u8], shared_values: &[u32]) -> Result<i32> {
    let expression = trim_ascii(expression);
    if expression.starts_with(b"#") {
        let index = parse_cst_integer(&expression[1..])
            .context("CST condition has an invalid shared variable index")?;
        if index < 0 {
            bail!("CST condition shared variable index is negative");
        }
        return Ok(shared_values.get(index as usize).copied().unwrap_or(0) as i32);
    }
    parse_cst_integer(expression).context("unsupported CST condition expression")
}

fn split_cst_operator<'a>(expression: &'a [u8], operator: &[u8]) -> Option<(&'a [u8], &'a [u8])> {
    let mut depth = 0usize;
    let mut index = 0usize;
    while index + operator.len() <= expression.len() {
        match expression[index] {
            b'(' => depth += 1,
            b')' => depth = depth.saturating_sub(1),
            _ if depth == 0 && expression[index..].starts_with(operator) => {
                return Some((
                    trim_ascii(&expression[..index]),
                    trim_ascii(&expression[index + operator.len()..]),
                ));
            }
            _ => {}
        }
        index += 1;
    }
    None
}

impl FesRuntime {
    fn new(document: FesDocument) -> Self {
        let section = document
            .sections
            .iter()
            .position(|section| section.header.eq_ignore_ascii_case(b"START"))
            .unwrap_or(0);
        let (media, planes) = parse_fes_media(&document);
        Self {
            driver: None,
            document,
            section,
            line: 0,
            wait_frames: 0,
            exit_code: None,
            values: BTreeMap::new(),
            named: BTreeMap::new(),
            input_mode: false,
            continuous_input: false,
            media,
            planes,
            animation_random_state: 0,
            focused_object: None,
            event_return: None,
            event_target: None,
            event_bank: None,
        }
    }

    fn record_input(&mut self, event: u32, pointer: Option<Point>) {
        let (name, value) = match event {
            2 | 19 => ("_CLICK_L_", -1),
            3 => ("_CLICK_R_", -1),
            4 => ("_CLICK_L_", 0),
            5 => ("_CLICK_R_", 0),
            6 => ("_SCRL_U_", -1),
            7 => ("_SCRL_D_", -1),
            8 => ("_SCRL_P_", -1),
            9 => ("_PAGE_U_", -1),
            10 => ("_PAGE_D_", -1),
            11 => ("_PUSH_UP_", -1),
            12 => ("_PUSH_DOWN_", -1),
            13 => ("_PUSH_LEFT_", -1),
            14 => ("_PUSH_RIGHT_", -1),
            15 => ("_RELEASE_UP_", -1),
            16 => ("_RELEASE_DOWN_", -1),
            17 => ("_RELEASE_LEFT_", -1),
            18 => ("_RELEASE_RIGHT_", -1),
            20 => ("_CLICK_L_", 0),
            21 => ("_TOUCH_", -1),
            _ => return,
        };
        self.named.insert(name.to_string(), value);
        match event {
            19 => self.input_mode = true,
            20 => self.input_mode = false,
            _ => {}
        }
        if matches!(event, 2..=5 | 11..=18) {
            if event == 2
                && let Some(target) = pointer.and_then(|point| self.hit_test_button(point))
            {
                if self.focused_object.as_deref() != Some(target.as_str()) {
                    self.focused_object = Some(target.clone());
                    self.start_object_event("FOCUS");
                    return;
                }
                self.focused_object = Some(target);
            }
            let name = match event {
                2 => "PUSH_L",
                3 => "PUSH_R",
                4 => "RELEASE_L",
                5 => "RELEASE_R",
                11 => "PUSH_UP",
                12 => "PUSH_DOWN",
                13 => "PUSH_LEFT",
                14 => "PUSH_RIGHT",
                15 => "RELEASE_UP",
                16 => "RELEASE_DOWN",
                17 => "RELEASE_LEFT",
                18 => "RELEASE_RIGHT",
                _ => return,
            };
            self.start_object_event(name);
        }
    }

    fn hit_test_button(&self, point: Point) -> Option<String> {
        if let Some(focused) = self.focused_object.as_deref()
            && let Some(layer) = self.media.get(focused)
            && layer.kind == "BUTTON"
            && layer.visible
            && layer.enabled
            && self.button_contains(layer, point)
        {
            return Some(focused.to_owned());
        }
        self.media
            .values()
            .filter(|layer| layer.kind == "BUTTON" && layer.visible && layer.enabled)
            .filter_map(|layer| {
                let image = layer.image.as_ref()?;
                let plane = layer.plane.as_ref().and_then(|name| self.planes.get(name));
                if plane.is_some_and(|plane| {
                    !plane.visible
                        || point.x < plane.position.x
                        || point.y < plane.position.y
                        || i64::from(point.x)
                            >= i64::from(plane.position.x) + i64::from(plane.size[0])
                        || i64::from(point.y)
                            >= i64::from(plane.position.y) + i64::from(plane.size[1])
                }) {
                    return None;
                }
                let _ = image;
                self.button_contains(layer, point)
                    .then(|| (layer.priority, layer.name.clone()))
            })
            .max_by_key(|(priority, _)| *priority)
            .map(|(_, name)| name)
    }

    fn button_contains(&self, layer: &FesMediaLayer, point: Point) -> bool {
        let Some(image) = layer.image.as_ref() else {
            return false;
        };
        let position = layer.draw_position(&self.planes);
        i64::from(point.x) >= i64::from(position.x)
            && i64::from(point.y) >= i64::from(position.y)
            && i64::from(point.x) < i64::from(position.x) + i64::from(image.width())
            && i64::from(point.y) < i64::from(position.y) + i64::from(image.height())
    }

    fn start_object_event(&mut self, event: &str) {
        let Some(target) = self.focused_object.clone() else {
            return;
        };
        if self.event_return.is_some() {
            return;
        }
        let (base, bank) = split_fes_object_bank(&target);
        let header = format!("{base}.{event}");
        let Some(section) = self
            .document
            .sections
            .iter()
            .position(|section| section.header.eq_ignore_ascii_case(header.as_bytes()))
        else {
            return;
        };
        self.event_return = Some((self.section, self.line, self.wait_frames));
        self.event_target = Some(target);
        self.event_bank = Some(bank);
        self.section = section;
        self.line = 0;
        self.wait_frames = 0;
    }

    fn finish_input(&mut self) {
        if !self.input_mode && !self.continuous_input {
            self.named.insert("_CLICK_L_".to_string(), 0);
        }
        for name in [
            "_CLICK_R_",
            "_SCRL_U_",
            "_SCRL_D_",
            "_SCRL_P_",
            "_PAGE_U_",
            "_PAGE_D_",
            "_PUSH_UP_",
            "_PUSH_DOWN_",
            "_PUSH_LEFT_",
            "_PUSH_RIGHT_",
            "_RELEASE_UP_",
            "_RELEASE_DOWN_",
            "_RELEASE_LEFT_",
            "_RELEASE_RIGHT_",
            "_TOUCH_",
        ] {
            self.named.insert(name.to_string(), 0);
        }
    }

    fn advance(
        &mut self,
        frame: u64,
        resources: &ResourceIndex,
        events: &mut Vec<LogicEvent>,
        nodes: &mut BTreeMap<u32, SceneNode>,
    ) {
        if self.exit_code.is_some() {
            return;
        }
        if self.wait_frames > 0 {
            self.wait_frames -= 1;
            return;
        }
        for _ in 0..256 {
            let Some(section) = self.document.sections.get(self.section) else {
                if let Some((section, line, wait_frames)) = self.event_return.take() {
                    self.section = section;
                    self.line = line;
                    self.wait_frames = wait_frames;
                    self.event_target = None;
                    self.event_bank = None;
                    return;
                }
                self.exit_code = Some(0);
                return;
            };
            let Some(line) = section.lines.get(self.line) else {
                if let Some((section, line, wait_frames)) = self.event_return.take() {
                    self.section = section;
                    self.line = line;
                    self.wait_frames = wait_frames;
                    self.event_target = None;
                    self.event_bank = None;
                    return;
                }
                self.line = 0;
                self.wait_frames = 1;
                return;
            };
            self.line += 1;
            let text = String::from_utf8_lossy(&line.bytes).trim().to_string();
            if text.is_empty() {
                continue;
            }
            let mut parts = text.split_whitespace();
            let command = parts.next().unwrap_or_default();
            match command.to_ascii_lowercase().as_str() {
                "if" => {
                    let condition = text
                        .strip_prefix(command)
                        .map(str::trim)
                        .unwrap_or_default();
                    if !self.eval_condition(condition) {
                        self.skip_to_endif();
                    }
                    continue;
                }
                "endif" => continue,
                "wait" => {
                    self.wait_frames = parts
                        .next()
                        .and_then(|value| value.parse::<u32>().ok())
                        .unwrap_or(1);
                    return;
                }
                "next" => {
                    let target = parts.next().unwrap_or_default().as_bytes();
                    if let Some(index) = self
                        .document
                        .sections
                        .iter()
                        .position(|section| section.header.eq_ignore_ascii_case(target))
                    {
                        self.event_return = None;
                        self.event_target = None;
                        self.event_bank = None;
                        self.section = index;
                        self.line = 0;
                    } else {
                        self.exit_code = Some(-1);
                    }
                    return;
                }
                "exit" => {
                    self.exit_code = Some(
                        parts
                            .next()
                            .and_then(|value| value.parse::<i32>().ok())
                            .unwrap_or(0),
                    );
                    return;
                }
                _ if command.starts_with('$') => {
                    if let Some((name, value)) = text.split_once('=') {
                        let name = name.trim().trim_start_matches('$');
                        if let Ok(value) = value.trim().parse::<i32>() {
                            if let Ok(index) = name.parse::<i32>() {
                                self.values.insert(index, value);
                            }
                        }
                    }
                    continue;
                }
                "@" if self.event_target.is_some() => {
                    if let Some(event_target) = self.event_target.clone() {
                        let target_command = parts.next().unwrap_or_default();
                        let banked_target = format!(
                            "{event_target} {target_command} {}",
                            parts.collect::<Vec<_>>().join(" ")
                        );
                        self.apply_media_command(
                            &event_target,
                            &banked_target,
                            resources,
                            frame,
                            events,
                        );
                    }
                    events.push(LogicEvent {
                        frame,
                        kind: "fes-event-command".into(),
                        detail: text,
                    });
                }
                _ => {
                    let object_name = command
                        .split_once('[')
                        .map(|(name, _)| name)
                        .unwrap_or(command);
                    if command.eq_ignore_ascii_case("automove") {
                        if let Some(target) = parts.next() {
                            self.focused_object = Some(target.to_owned());
                            self.start_object_event("FOCUS");
                        }
                    }
                    self.apply_media_command(command, &text, resources, frame, events);
                    let node_id = 0x4000_0000_u32
                        | object_name.bytes().fold(0x811c_9dc5_u32, |hash, byte| {
                            (hash ^ u32::from(byte)).wrapping_mul(16_777_619)
                        });
                    let node = nodes
                        .entry(node_id)
                        .or_insert_with(|| SceneNode::new(node_id, 0, 0, object_name.to_string()));
                    if text.to_ascii_lowercase().contains("disp 0")
                        || text.to_ascii_lowercase().contains("noact")
                    {
                        node.visible = false;
                    } else if text.to_ascii_lowercase().contains("disp 1")
                        || text.to_ascii_lowercase().contains("enable 1")
                    {
                        node.visible = true;
                        node.initialized = true;
                        if let Some(layer) = self.media.get(command) {
                            if let Some(image) = &layer.image {
                                node.size = [image.width() as f32, image.height() as f32];
                            }
                            node.position[0] = layer.position.x as f32;
                            node.position[1] = layer.position.y as f32;
                            node.color = (u32::from(layer.color.a) << 24)
                                | (u32::from(layer.color.r) << 16)
                                | (u32::from(layer.color.g) << 8)
                                | u32::from(layer.color.b);
                        }
                    }
                    events.push(LogicEvent {
                        frame,
                        kind: "fes-command".into(),
                        detail: text,
                    });
                }
            }
        }
    }

    fn apply_media_command(
        &mut self,
        command: &str,
        text: &str,
        resources: &ResourceIndex,
        frame: u64,
        events: &mut Vec<LogicEvent>,
    ) {
        let args = text.split_ascii_whitespace().collect::<Vec<_>>();
        let subcommand = args
            .get(1)
            .copied()
            .unwrap_or_default()
            .to_ascii_lowercase();
        if command.eq_ignore_ascii_case("keywait") {
            self.apply_keywait_command(&args, text, resources, frame, events);
            return;
        }
        if command.eq_ignore_ascii_case("baseimg") {
            let Some(source) = args.get(1) else {
                return;
            };
            for layer in self.media.values_mut() {
                if layer.name == *source {
                    layer.visible = true;
                }
            }
            return;
        }

        if let Some(plane) = self.planes.get_mut(command) {
            match subcommand.as_str() {
                "disp" => {
                    plane.visible = args
                        .get(2)
                        .and_then(|value| value.parse().ok())
                        .unwrap_or(1)
                        != 0;
                }
                "pos" => {
                    let previous = plane.position;
                    plane.position = Point::new(
                        parse_fes_position(args.get(2), previous.x),
                        parse_fes_position(args.get(3), previous.y),
                    );
                }
                "fade" => {
                    plane.visible = true;
                    plane.opacity = args
                        .get(4)
                        .and_then(|value| value.parse::<i32>().ok())
                        .unwrap_or(255)
                        .clamp(0, 255) as u8;
                }
                _ => {}
            }
            events.push(LogicEvent {
                frame,
                kind: "fes-media".into(),
                detail: format!("plane={command} command={subcommand}"),
            });
            return;
        }

        let targets = self
            .media
            .keys()
            .filter(|name| *name == command || name.starts_with(&format!("{command}[")))
            .cloned()
            .collect::<Vec<_>>();
        let has_targets = !targets.is_empty();
        for target in targets {
            let Some(layer) = self.media.get_mut(&target) else {
                continue;
            };
            match subcommand.as_str() {
                "disp" => {
                    layer.visible = args
                        .get(2)
                        .and_then(|value| value.parse().ok())
                        .unwrap_or(1)
                        != 0;
                    if layer.visible && layer.image.is_none() {
                        if layer.file.is_some() {
                            if let Err(error) = load_fes_media_image(layer, resources) {
                                events.push(LogicEvent {
                                    frame,
                                    kind: "fes-media-error".into(),
                                    detail: format!("object={target} {error:#}"),
                                });
                            }
                        } else if layer.color.a != 0 && layer.image.is_none() {
                            let size = layer
                                .clip
                                .map(|clip| [clip.width, clip.height])
                                .unwrap_or([800, 600]);
                            layer.image = solid_frame(size[0], size[1], layer.color);
                        }
                    }
                }
                "setid" => {
                    if let Some(id) = args.get(2).and_then(|value| value.parse::<usize>().ok()) {
                        layer.selected_id = id;
                        layer.image = None;
                        if layer.visible && layer.file.is_some() {
                            if let Err(error) = load_fes_media_image(layer, resources) {
                                events.push(LogicEvent {
                                    frame,
                                    kind: "fes-media-error".into(),
                                    detail: format!("object={target} {error:#}"),
                                });
                            }
                        }
                    }
                }
                "fade" => {
                    layer.opacity = args
                        .get(4)
                        .and_then(|value| value.parse::<i32>().ok())
                        .unwrap_or(255)
                        .clamp(0, 255) as u8;
                    layer.visible = true;
                }
                "pos" => {
                    let previous = layer.position;
                    layer.position = Point::new(
                        parse_fes_position(args.get(2), previous.x),
                        parse_fes_position(args.get(3), previous.y),
                    );
                }
                "enable" => {
                    layer.enabled = args
                        .get(2)
                        .and_then(|value| value.parse::<i32>().ok())
                        .unwrap_or(1)
                        != 0;
                }
                "noact" => {}
                _ => {}
            }
        }
        if has_targets {
            events.push(LogicEvent {
                frame,
                kind: "fes-media".into(),
                detail: format!("object={command} command={subcommand}"),
            });
        }
    }

    fn apply_keywait_command(
        &mut self,
        args: &[&str],
        text: &str,
        resources: &ResourceIndex,
        frame: u64,
        events: &mut Vec<LogicEvent>,
    ) {
        let error = |events: &mut Vec<LogicEvent>, detail: String| {
            events.push(LogicEvent {
                frame,
                kind: "fes-media-error".into(),
                detail: format!("object=keywait command={text:?} {detail}"),
            });
        };
        let Some(subcommand) = args.get(1).map(|value| value.to_ascii_lowercase()) else {
            error(events, "missing subcommand".into());
            return;
        };
        let Some(layer) = self.media.get("keywait") else {
            error(events, "object is not defined".into());
            return;
        };

        match subcommand.as_str() {
            "setanm" => {
                if args.len() != 3 || args[2].is_empty() {
                    error(
                        events,
                        "setanm requires exactly one animation resource".into(),
                    );
                    return;
                }
                let Some(image_resource) = layer.file.clone() else {
                    error(events, "setanm has no companion FILE image".into());
                    return;
                };
                match load_fes_animation(resources, args[2], &image_resource) {
                    Ok(animation) => {
                        let display = animation.display();
                        if let Some(layer) = self.media.get_mut("keywait") {
                            layer.animation = Some(animation);
                            layer.animation_display = Some(display);
                            layer.image = None;
                        }
                        events.push(LogicEvent {
                            frame,
                            kind: "fes-media".into(),
                            detail: format!("object=keywait command=setanm resource={:?}", args[2]),
                        });
                    }
                    Err(load_error) => error(events, load_error.to_string()),
                }
            }
            "anm" => {
                if args.len() != 3 {
                    error(events, "anm requires exactly one numeric state".into());
                    return;
                }
                let Ok(value) = args[2].parse::<i32>() else {
                    error(
                        events,
                        format!("anm state is not an integer: {:?}", args[2]),
                    );
                    return;
                };
                let Some(layer) = self.media.get_mut("keywait") else {
                    return;
                };
                let Some(animation) = layer.animation.as_mut() else {
                    error(events, "anm has no loaded animation".into());
                    return;
                };
                animation.playing = i32::from(value != 0);
                events.push(LogicEvent {
                    frame,
                    kind: "fes-media".into(),
                    detail: format!("object=keywait command=anm value={}", animation.playing),
                });
            }
            "disp" => {
                if args.len() != 3 {
                    error(events, "disp requires exactly one numeric state".into());
                    return;
                }
                let Ok(value) = args[2].parse::<i32>() else {
                    error(
                        events,
                        format!("disp state is not an integer: {:?}", args[2]),
                    );
                    return;
                };
                let Some(layer) = self.media.get_mut("keywait") else {
                    return;
                };
                layer.visible = value != 0;
                if layer.visible && layer.image.is_none() {
                    if let Err(load_error) = load_fes_media_image(layer, resources) {
                        error(events, load_error.to_string());
                    }
                }
                events.push(LogicEvent {
                    frame,
                    kind: "fes-media".into(),
                    detail: format!(
                        "object=keywait command=disp value={}",
                        i32::from(layer.visible)
                    ),
                });
            }
            "fade" => {
                if args.len() != 5 {
                    error(
                        events,
                        "fade requires duration, start, and end values".into(),
                    );
                    return;
                }
                let Ok(duration) = args[2].parse::<i32>() else {
                    error(
                        events,
                        format!("fade duration is not an integer: {:?}", args[2]),
                    );
                    return;
                };
                let Ok(from) = args[3].parse::<i32>() else {
                    error(
                        events,
                        format!("fade start is not an integer: {:?}", args[3]),
                    );
                    return;
                };
                let Ok(to) = args[4].parse::<i32>() else {
                    error(events, format!("fade end is not an integer: {:?}", args[4]));
                    return;
                };
                let Some(layer) = self.media.get_mut("keywait") else {
                    return;
                };
                let from = from.clamp(0, 255);
                let to = to.clamp(0, 255);
                layer.fade = Some((duration.max(0), from, to, 0));
                layer.opacity = from as u8;
                layer.visible = true;
                events.push(LogicEvent {
                    frame,
                    kind: "fes-media".into(),
                    detail: format!(
                        "object=keywait command=fade duration={duration} from={from} to={to}"
                    ),
                });
            }
            _ => error(
                events,
                format!("unsupported keywait subcommand {:?}", args[1]),
            ),
        }
    }

    fn update_animations(
        &mut self,
        frame: u64,
        budget: usize,
        resources: &ResourceIndex,
        events: &mut Vec<LogicEvent>,
    ) {
        let now = frame.min(u64::from(u32::MAX)) as u32;
        let random_state = &mut self.animation_random_state;
        for layer in self.media.values_mut() {
            if let Some((duration, from, to, elapsed)) = layer.fade {
                let elapsed = elapsed.saturating_add(1);
                if duration <= 0 {
                    layer.opacity = to.clamp(0, 255) as u8;
                    layer.fade = None;
                } else {
                    let progress = (elapsed as f32 / duration as f32).clamp(0.0, 1.0);
                    let value = from as f32 + (to - from) as f32 * progress;
                    layer.opacity = value.round().clamp(0.0, 255.0) as u8;
                    layer.fade = (elapsed < duration).then_some((duration, from, to, elapsed));
                }
            }
            let Some(animation) = layer.animation.as_mut() else {
                continue;
            };
            let mut random = || {
                *random_state = random_state
                    .wrapping_mul(1_664_525)
                    .wrapping_add(1_013_904_223);
                *random_state
            };
            match animation.update_with_random(now, budget, false, &mut random) {
                Ok(_) => {
                    layer.animation_display = Some(animation.display());
                    if layer.source.has_raw_source() {
                        continue;
                    }
                    if animation.frame_change().is_some() {
                        layer.image = None;
                    }
                    if layer.visible && layer.image.is_none() {
                        if let Err(error) = load_fes_media_image(layer, resources) {
                            events.push(LogicEvent {
                                frame,
                                kind: "fes-media-error".into(),
                                detail: format!("object={} animation frame: {error}", layer.name),
                            });
                        }
                    }
                }
                Err(error) => events.push(LogicEvent {
                    frame,
                    kind: "fes-media-error".into(),
                    detail: format!("object={} animation update: {error}", layer.name),
                }),
            }
        }
    }

    fn eval_condition(&self, condition: &str) -> bool {
        let condition = condition
            .trim()
            .trim_start_matches('(')
            .trim_end_matches(')');
        if let Some((left, right)) = condition.split_once("||") {
            return self.eval_condition(left) || self.eval_condition(right);
        }
        if let Some((left, right)) = condition.split_once("&&") {
            return self.eval_condition(left) && self.eval_condition(right);
        }
        for operator in ["!=", "=="] {
            if let Some((left, right)) = condition.split_once(operator) {
                let left = self.eval_value(left.trim());
                let right = self.eval_value(right.trim());
                return if operator == "==" {
                    left == right
                } else {
                    left != right
                };
            }
        }
        self.eval_value(condition) != 0
    }

    fn eval_value(&self, value: &str) -> i32 {
        let value = value.trim().trim_start_matches('(').trim_end_matches(')');
        if value == "@" {
            return self.event_bank.unwrap_or_default();
        }
        if let Some(index) = value.strip_prefix('$').and_then(|value| value.parse().ok()) {
            return *self.values.get(&index).unwrap_or(&0);
        }
        if let Some(value) = self.named.get(value) {
            return *value;
        }
        value.parse::<i32>().unwrap_or(0)
    }

    fn skip_to_endif(&mut self) {
        let mut depth = 1usize;
        while let Some(section) = self.document.sections.get(self.section) {
            let Some(line) = section.lines.get(self.line) else {
                self.exit_code = Some(-1);
                return;
            };
            self.line += 1;
            let text = String::from_utf8_lossy(&line.bytes);
            let command = text.split_whitespace().next().unwrap_or_default();
            if command.eq_ignore_ascii_case("if") {
                depth += 1;
            } else if command.eq_ignore_ascii_case("endif") {
                depth -= 1;
                if depth == 0 {
                    return;
                }
            }
        }
        self.exit_code = Some(-1);
    }
}

impl SceneNode {
    fn new(id: u32, kind: u32, parent: u32, name: String) -> Self {
        Self {
            id,
            kind,
            parent,
            name,
            size: [0.0; 2],
            position: [0.0; 3],
            scale: [1.0; 3],
            color: 0xffff_ffff,
            overlay_color: 0,
            visible: false,
            initialized: false,
            fes_block_name: Vec::new(),
            fes_object_prefix: Vec::new(),
            fes_bank: 0,
            fes_parameters: [0; 512],
        }
    }
}

#[derive(Debug, Clone)]
pub struct HeadlessReport {
    pub root: PathBuf,
    pub scan: GameScan,
    pub compatibility: CompatibilityReport,
    pub loaded: Option<LoadedEntry>,
    pub state: RuntimeState,
    pub current_task_id: u32,
    pub scene_nodes: Vec<SceneNode>,
    pub particle_snapshot: ParticleSnapshot,
    pub menu_item_requests: Vec<(u32, u32)>,
    pub debug_mode: u32,
    pub sound_config_enabled: u32,
    pub frame_hash: u64,
    pub frame_size: [u32; 2],
    pub frame_in_progress: bool,
    pub frame_rgba: Vec<u8>,
    pub fes_exit_code: Option<i32>,
    pub events: Vec<LogicEvent>,
    pub log: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostCapability {
    pub status: String,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompatibilityReport {
    pub engine_family: String,
    pub detected_variant: String,
    pub primary_executable: Option<String>,
    pub pe_architecture: Option<String>,
    pub imported_modules: Vec<String>,
    pub runtime_profiles: Vec<String>,
    pub resource_extensions: BTreeMap<String, usize>,
    pub script_formats: Vec<String>,
    pub audio_formats: Vec<String>,
    pub video_formats: Vec<String>,
    pub save_formats: Vec<String>,
    pub archive_count: usize,
    pub indexed_archives: usize,
    pub encrypted_archives: usize,
    pub malformed_archives: usize,
    pub configuration_resources: Vec<String>,
    pub window: HostCapability,
    pub input: HostCapability,
    pub audio: HostCapability,
    pub video: HostCapability,
    pub saves: HostCapability,
    pub boundaries: Vec<String>,
}

impl CompatibilityReport {
    pub fn from_scan(scan: &GameScan) -> Self {
        let mut resource_extensions = BTreeMap::new();
        let mut binary_names = Vec::new();
        let mut audio_formats = BTreeMap::new();
        let mut video_formats = BTreeMap::new();
        let mut save_formats = BTreeMap::new();
        let mut script_formats = BTreeMap::new();
        let mut all_resource_names = Vec::new();

        for file in scan.resources.files() {
            let name = file.relative_path.to_string_lossy().replace('\\', "/");
            if is_binary_path(Path::new(&name)) {
                binary_names.push(name.clone());
            }
            all_resource_names.push(name);
        }
        for archive in scan.resources.archives() {
            for entry in &archive.entries {
                all_resource_names.push(entry.name.replace('\\', "/"));
            }
        }
        for name in &all_resource_names {
            let Some(extension) = resource_extension(name) else {
                continue;
            };
            *resource_extensions.entry(extension.clone()).or_insert(0) += 1;
            let target = match extension.as_str() {
                "kcs" | "cst" | "fes" => Some(&mut script_formats),
                "ogg" | "wav" | "mp3" | "wma" | "mid" | "midi" | "opus" | "flac" => {
                    Some(&mut audio_formats)
                }
                "mpg" | "mpeg" | "m1v" | "mpv" | "m2v" | "webm" | "avi" | "mp4" | "mov" | "wmv" | "asf" => {
                    Some(&mut video_formats)
                }
                "css" | "cs2save" | "sav" | "s00" | "s01" => Some(&mut save_formats),
                _ => None,
            };
            if let Some(target) = target {
                *target.entry(extension).or_insert(0) += 1;
            }
        }

        let primary_executable = select_host_executable(scan).or_else(|| {
            scan.engine
                .primary_executable
                .as_ref()
                .map(|binary| binary.relative_path.to_string_lossy().replace('\\', "/"))
        });
        let pe_summary = primary_executable
            .as_deref()
            .map(|relative| read_pe_summary(&scan.root.join(relative)));
        let pe_architecture = pe_summary
            .as_ref()
            .and_then(|summary| summary.as_ref().ok())
            .map(|summary| summary.architecture.clone());
        let imported_modules = pe_summary
            .as_ref()
            .and_then(|summary| summary.as_ref().ok())
            .map(|summary| summary.imports.clone())
            .unwrap_or_default();

        let lower_binary_names = binary_names
            .iter()
            .map(|name| name.to_ascii_lowercase())
            .collect::<Vec<_>>();
        let lower_imports = imported_modules
            .iter()
            .map(|name| name.to_ascii_lowercase())
            .collect::<Vec<_>>();
        let has_module = |needle: &str| {
            lower_binary_names
                .iter()
                .any(|name| name.rsplit('/').next().is_some_and(|name| name == needle))
        };
        let imports_module = |needle: &str| {
            lower_imports
                .iter()
                .any(|name| name == needle || name.starts_with(&format!("{needle}.")))
        };
        let mut runtime_profiles = Vec::new();
        if has_module("cs2conf.dll") {
            runtime_profiles.push("cs2conf legacy configuration ABI".to_string());
        }
        if has_module("cs2confx.dll") {
            runtime_profiles.push("cs2confx extended/Steam configuration ABI".to_string());
        }
        if has_module("hook.dll") {
            runtime_profiles.push("hook.dll host-hook ABI".to_string());
        }
        if has_module("emotedriver.dll") {
            runtime_profiles.push("emotedriver plugin ABI".to_string());
        }
        if imports_module("dsound.dll") || imports_module("d3d8.dll") || imports_module("d3d9.dll")
        {
            runtime_profiles.push("DirectSound/legacy Direct3D host".to_string());
        }
        if imports_module("msvfw32.dll") {
            runtime_profiles.push("Video for Windows media path".to_string());
        }
        if lower_binary_names
            .iter()
            .any(|name| name.contains("webmsplit") || name.contains("vp8decoder"))
        {
            runtime_profiles.push("DirectShow WebM/VP8 filter chain".to_string());
        }
        if lower_binary_names
            .iter()
            .any(|name| name.contains("node") || name.contains("v8"))
            || all_resource_names.iter().any(|name| {
                matches!(
                    resource_extension(name).as_deref(),
                    Some("js" | "html" | "htm" | "mhtml")
                )
            })
        {
            runtime_profiles.push("embedded web/JavaScript host markers".to_string());
        }
        if imports_module("winmm.dll") {
            runtime_profiles.push("WinMM multimedia ABI".to_string());
        }

        let indexed_archives = scan
            .resources
            .archives()
            .iter()
            .filter(|archive| matches!(&archive.status, cs2_runtime::ArchiveStatus::Indexed))
            .count();
        let encrypted_archives = scan
            .resources
            .archives()
            .iter()
            .filter(|archive| {
                matches!(
                    &archive.status,
                    cs2_runtime::ArchiveStatus::Encrypted { .. }
                )
            })
            .count();
        let malformed_archives = scan
            .resources
            .archives()
            .iter()
            .filter(|archive| {
                matches!(
                    &archive.status,
                    cs2_runtime::ArchiveStatus::Malformed { .. }
                )
            })
            .count();

        let audio_extensions = audio_formats.keys().cloned().collect::<Vec<_>>();
        let video_extensions = video_formats.keys().cloned().collect::<Vec<_>>();
        let movie_decoder = movie::discover_ffmpeg().filter(|path| path.is_file());
        let movie_probe = movie::discover_ffprobe();
        let android_movie_decoder = MpegMovieHost::native_android_decoder_available();
        let script_formats = script_formats.into_keys().collect::<Vec<_>>();
        let save_formats = save_formats.into_keys().collect::<Vec<_>>();
        let mut boundaries = vec![
            "Only the simulator process is used; runtime EXE and DLL files are fingerprinted, never loaded or launched.".to_string(),
            "Hook, emotedriver, Steam, and other game-specific DLL side effects are reported as ABI markers but are not invoked.".to_string(),
        ];
        if pe_summary.as_ref().is_some_and(|summary| summary.is_err()) {
            boundaries.push("The primary executable could not be parsed as a PE image; architecture and import ABI are unknown.".to_string());
        }
        if pe_architecture
            .as_deref()
            .is_some_and(|architecture| architecture != "x86")
        {
            boundaries.push("Only x86 guest ABI behavior is currently modeled; this sample reports a different PE architecture.".to_string());
        }
        if !video_extensions.is_empty() {
            boundaries.push(if cfg!(target_os = "android") {
                if android_movie_decoder {
                    "Android libcs2_media ABI was loaded. The native FFmpeg AVIO/RGBA/PCM implementation has not been verified by this resource scan; library presence is not codec/game acceptance.".into()
                } else {
                    "Android native FFmpeg AVIO/RGBA/PCM implementation requires packaged libcs2_media and its pinned FFmpeg shared libraries. They were not loaded; movie load reports this explicitly. Windows FFmpeg executables cannot run on Android.".into()
                }
            } else { match &movie_decoder {
                Some(path) => format!(
                    "FFmpeg executable located at {}. MPEG/WebM/AVI/MP4/ASF-WMV decoding, ffprobe metadata, PTS scheduling and window presentation are implemented; the scan did not execute the decoder or verify any game movie.",
                    path.display()
                ),
                None => "MPEG/WebM/AVI/MP4/ASF-WMV playback requires installed FFmpeg and ffprobe tools (CS2_FFMPEG/CS2_FFPROBE, PATH, user tools or winget). No executable was located; play reports an error rather than simulating playback.".to_string(),
            } });
            if movie_probe.is_none() && !cfg!(target_os = "android") {
                boundaries.push("ffprobe is missing: movie loading fails with an explicit metadata-tool diagnostic; set CS2_FFPROBE or install the matching tool.".into());
            }
            boundaries.push("PCM output has WinMM on Windows and AAudio on Android; other platforms require a MovieAudioFactory. A PCM output backend does not establish video decoder availability or codec support. Seekable movie input is bounded to 4 GiB; indexed archives use the runtime bounded media stream with incremental payload decryption.".into());
        }
        if audio_extensions.iter().any(|extension| !matches!(extension.as_str(), "ogg" | "wav")) {
            boundaries.push("Standalone audio implements Ogg/Vorbis and RIFF/WAVE integer PCM, IEEE float and Microsoft ADPCM; other detected audio codecs fail explicitly. Decoder source presence is not sample acceptance.".to_string());
        }
        if !audio_extensions.is_empty() {
            boundaries.push("Kind13 sound commands share decoded PCM and physical desktop channels, including queued input, live controls, partial loop endpoints and alternate-channel transitions. PCM is bounded to 64 MiB per source; the desktop queue differs from the original four-second prefetch ring. Undocumented sound commands fail explicitly. Scan results do not verify fades, endpoint timing, device output or game audio.".to_string());
        }
        if runtime_profiles
            .iter()
            .any(|profile| profile.contains("hook.dll") || profile.contains("emotedriver"))
        {
            boundaries.push("Game-specific hook/plugin calls may alter behavior beyond the portable script and resource interfaces.".to_string());
        }
        if script_formats
            .iter()
            .any(|format| format == "cst" || format == "fes")
            || all_resource_names
                .iter()
                .any(|name| resource_extension(name).as_deref() == Some("kcs"))
        {
            boundaries.push("A detected script format does not imply complete scene, native-call, or menu coverage.".to_string());
        }

        let window = HostCapability {
            status: if cfg!(windows) {
                "supported".to_string()
            } else {
                "windows-only".to_string()
            },
            detail: "Windows resizable window with optional wgpu presentation or Win32 GDI fallback; configured logical canvas, aspect-fit presentation, timed frame scheduling and decoded movie surface. Source implementation is not game acceptance evidence.".to_string(),
        };
        let input = HostCapability {
            status: if cfg!(windows) {
                "partial".to_string()
            } else {
                "windows-only".to_string()
            },
            detail: "Keyboard, mouse, wheel, focus and committed IME events are translated; per-game remapping and text-widget semantics are not inferred.".to_string(),
        };
        let audio = if audio_extensions.is_empty() {
            HostCapability {
                status: "not-detected".to_string(),
                detail: "No indexed Ogg/Vorbis or other recognized audio resource was found."
                    .to_string(),
            }
        } else if audio_extensions.iter().all(|extension| matches!(extension.as_str(), "ogg" | "wav")) {
            HostCapability {
                status: if cfg!(any(windows, target_os = "android")) { "implemented-unverified" } else { "output-unavailable" }.to_string(),
                detail:
                    "Ogg/Vorbis and RIFF/WAVE PCM8/16/24/32, IEEE float32/64 and Microsoft ADPCM decoding share bounded PCM, channel controls and looping, using WinMM on Windows or AAudio API26+ on Android. Codec output, device operation, app lifecycle and game playback were not verified by this scan."
                        .to_string(),
            }
        } else {
            HostCapability {
                status: "partial".to_string(),
                detail: format!(
                    "Ogg/Vorbis and RIFF/WAVE PCM, IEEE float and Microsoft ADPCM have Windows/Android output implementations; detected formats: {}. This scan does not test codec output or devices.",
                    audio_extensions.join(", ")
                ),
            }
        };
        let video = if video_extensions.is_empty() {
            HostCapability {
                status: "not-detected".to_string(),
                detail: "No indexed video resource with a recognized extension was found."
                    .to_string(),
            }
        } else {
            HostCapability {
                status: if cfg!(target_os = "android") {
                    if android_movie_decoder { "native-library-found-unverified" } else { "decoder-unavailable" }
                } else if movie_decoder.is_none() || movie_probe.is_none() {
                    "missing-decoder"
                } else if video_extensions.iter().any(|extension|
                    !matches!(extension.as_str(), "mpg" | "mpeg" | "m1v" | "mpv" | "m2v" | "webm" | "wmv" | "asf" | "avi" | "mp4" | "mov"))
                {
                    "partial"
                } else { "decoder-found-unverified" }.to_string(),
                detail: if cfg!(target_os = "android") {
                    format!("Detected {}. Android uses native libcs2_media/FFmpeg C APIs with archive AVIO, RGBA frames and AAudio PCM. Libraries loaded: {android_movie_decoder}. No decoder, APK or game playback was exercised by this scan.", video_extensions.join(", "))
                } else { format!(
                    "Detected {}. MPEG program/elementary streams, WebM, AVI, MP4 and ASF/WMV use external FFmpeg/ffprobe, decoded RGBA/PCM, PTS clocks and the desktop event loop. Executable discovery is not a codec or game acceptance test.",
                    video_extensions.join(", ")
                ) },
            }
        };
        let saves = HostCapability {
            status: "partial".to_string(),
            detail: "CSS container and file operations have portable host support; complete title-menu save/load behavior and game-specific side effects are not established.".to_string(),
        };
        let configuration_resources = scan
            .config_candidates
            .iter()
            .map(|candidate| candidate.requested_name.clone())
            .collect::<Vec<_>>();

        Self {
            engine_family: format!("{:?}", scan.engine.family),
            detected_variant: format!("{:?}", scan.engine.variant),
            primary_executable,
            pe_architecture,
            imported_modules,
            runtime_profiles,
            resource_extensions,
            script_formats,
            audio_formats: audio_extensions,
            video_formats: video_extensions,
            save_formats,
            archive_count: scan.resources.archives().len(),
            indexed_archives,
            encrypted_archives,
            malformed_archives,
            configuration_resources,
            window,
            input,
            audio,
            video,
            saves,
            boundaries,
        }
    }
}

#[derive(Debug, Clone)]
struct PeSummary {
    architecture: String,
    imports: Vec<String>,
}

fn is_binary_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("exe") || extension.eq_ignore_ascii_case("dll")
        })
}

fn select_host_executable(scan: &GameScan) -> Option<String> {
    scan.resources
        .files()
        .iter()
        .filter(|file| {
            file.relative_path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
        })
        .filter_map(|file| {
            let name = file
                .relative_path
                .file_name()?
                .to_string_lossy()
                .to_ascii_lowercase();
            let excluded = [
                "uninst",
                "install",
                "setup",
                "boot",
                "tool",
                "patch",
                "boxloader",
                "msquestion",
                "edpsf",
                "start.exe",
                "launcher",
                "fes.exe",
                "wsd.exe",
                "pcmtag.exe",
                "ztpack.exe",
                "stview.exe",
            ];
            if excluded.iter().any(|needle| name.contains(needle)) {
                return None;
            }
            let preferred = if matches!(
                name.as_str(),
                "cs2.exe" | "cs2_open.exe" | "nekoparavol3.exe"
            ) {
                3
            } else if name.contains("grisaia") || name.contains("hatsumira") {
                2
            } else {
                1
            };
            Some((
                preferred,
                file.size,
                file.relative_path.to_string_lossy().replace('\\', "/"),
            ))
        })
        .max_by_key(|(preferred, size, path)| (*preferred, *size, path.clone()))
        .map(|(_, _, path)| path)
}

fn resource_extension(name: &str) -> Option<String> {
    Path::new(&name.replace('\\', "/"))
        .extension()
        .and_then(|extension| extension.to_str())
        .filter(|extension| !extension.is_empty())
        .map(str::to_ascii_lowercase)
}

fn read_pe_summary(path: &Path) -> std::result::Result<PeSummary, String> {
    let bytes = fs::read(path).map_err(|error| format!("read PE: {error}"))?;
    let read_u16 = |offset: usize| {
        bytes
            .get(offset..offset.checked_add(2)?)
            .map(|value| u16::from_le_bytes([value[0], value[1]]))
    };
    let read_u32 = |offset: usize| {
        bytes
            .get(offset..offset.checked_add(4)?)
            .map(|value| u32::from_le_bytes([value[0], value[1], value[2], value[3]]))
    };
    if bytes.get(..2) != Some(b"MZ") {
        return Err("missing DOS signature".to_string());
    }
    let pe_offset = read_u32(0x3c).ok_or_else(|| "truncated DOS header".to_string())? as usize;
    if bytes.get(pe_offset..pe_offset.saturating_add(4)) != Some(b"PE\0\0") {
        return Err("missing PE signature".to_string());
    }
    let machine = read_u16(pe_offset + 4).ok_or_else(|| "truncated PE header".to_string())?;
    let section_count =
        read_u16(pe_offset + 6).ok_or_else(|| "truncated PE section count".to_string())?;
    let optional_size = read_u16(pe_offset + 20)
        .ok_or_else(|| "truncated PE optional header size".to_string())?
        as usize;
    let optional_offset = pe_offset
        .checked_add(24)
        .ok_or_else(|| "PE header offset overflow".to_string())?;
    let optional_end = optional_offset
        .checked_add(optional_size)
        .filter(|end| *end <= bytes.len())
        .ok_or_else(|| "PE optional header is truncated".to_string())?;
    let magic =
        read_u16(optional_offset).ok_or_else(|| "missing PE optional header".to_string())?;
    let directory_offset = match magic {
        0x10b => optional_offset + 96,
        0x20b => optional_offset + 112,
        _ => return Err(format!("unsupported PE optional-header magic {magic:#x}")),
    };
    let machine_name = match machine {
        0x014c => "x86",
        0x8664 => "x86_64",
        0xaa64 => "arm64",
        _ => return Err(format!("unknown PE machine {machine:#06x}")),
    };
    let section_table = optional_end;
    let mut sections = Vec::new();
    for index in 0..usize::from(section_count).min(512) {
        let offset = section_table
            .checked_add(
                index
                    .checked_mul(40)
                    .ok_or_else(|| "section table overflow".to_string())?,
            )
            .ok_or_else(|| "section table overflow".to_string())?;
        let virtual_size =
            read_u32(offset + 8).ok_or_else(|| "truncated PE section".to_string())?;
        let virtual_address =
            read_u32(offset + 12).ok_or_else(|| "truncated PE section".to_string())?;
        let raw_size = read_u32(offset + 16).ok_or_else(|| "truncated PE section".to_string())?;
        let raw_offset = read_u32(offset + 20).ok_or_else(|| "truncated PE section".to_string())?;
        sections.push((virtual_address, virtual_size.max(raw_size), raw_offset));
    }
    let map_rva = |rva: u32| -> Option<usize> {
        sections.iter().find_map(|(start, size, raw)| {
            let relative = rva.checked_sub(*start)?;
            if relative >= *size {
                return None;
            }
            usize::try_from(raw.checked_add(relative)?).ok()
        })
    };
    let import_rva = read_u32(directory_offset + 8).unwrap_or(0);
    let mut imports = Vec::new();
    if import_rva != 0 {
        let mut descriptor =
            map_rva(import_rva).ok_or_else(|| "import table RVA is unmapped".to_string())?;
        for _ in 0..4096 {
            let original_thunk =
                read_u32(descriptor).ok_or_else(|| "truncated import descriptor".to_string())?;
            let timestamp = read_u32(descriptor + 4)
                .ok_or_else(|| "truncated import descriptor".to_string())?;
            let forwarder = read_u32(descriptor + 8)
                .ok_or_else(|| "truncated import descriptor".to_string())?;
            let name_rva = read_u32(descriptor + 12)
                .ok_or_else(|| "truncated import descriptor".to_string())?;
            let first_thunk = read_u32(descriptor + 16)
                .ok_or_else(|| "truncated import descriptor".to_string())?;
            if original_thunk == 0
                && timestamp == 0
                && forwarder == 0
                && name_rva == 0
                && first_thunk == 0
            {
                break;
            }
            let name_offset =
                map_rva(name_rva).ok_or_else(|| "import name RVA is unmapped".to_string())?;
            let tail = bytes
                .get(name_offset..)
                .ok_or_else(|| "import name is outside the file".to_string())?;
            let length = tail
                .iter()
                .position(|byte| *byte == 0)
                .unwrap_or(tail.len())
                .min(512);
            imports.push(String::from_utf8_lossy(&tail[..length]).to_ascii_lowercase());
            descriptor = descriptor
                .checked_add(20)
                .ok_or_else(|| "import descriptor offset overflow".to_string())?;
        }
        imports.sort();
        imports.dedup();
    }
    Ok(PeSummary {
        architecture: machine_name.to_string(),
        imports,
    })
}

/// Queued desktop input that can be consumed by a runtime scheduler.
///
/// The Windows window loop uses the same event shape directly, while this
/// adapter is useful to hosts that poll input outside of winit.
#[derive(Debug, Default)]
pub struct DesktopInputHost {
    events: VecDeque<InputEvent>,
    cursor_visible: bool,
}

impl DesktopInputHost {
    pub fn new() -> Self {
        Self {
            events: VecDeque::new(),
            cursor_visible: true,
        }
    }

    pub fn push(&mut self, event: InputEvent) {
        self.events.push_back(event);
    }

    pub fn extend(&mut self, events: impl IntoIterator<Item = InputEvent>) {
        self.events.extend(events);
    }

    pub fn cursor_visible(&self) -> bool {
        self.cursor_visible
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}

impl InputHost for DesktopInputHost {
    fn poll_events(&mut self) -> cs2_runtime::RuntimeResult<Vec<InputEvent>> {
        Ok(self.events.drain(..).collect())
    }

    fn set_cursor_visible(&mut self, visible: bool) -> cs2_runtime::RuntimeResult<()> {
        self.cursor_visible = visible;
        Ok(())
    }
}

pub use audio::{DesktopAudioHost, WavAudioHost, audio_host_for_resources};
pub use movie::{CallbackMoviePresenter, MovieAudioFactory, MovieAudioOutput, MovieFormat, MovieFrame, MovieFramePresenter, MovieFrameSurface, MpegMovieHost};

pub struct DesktopSaveHost {
    store: FileSaveStore,
}

impl DesktopSaveHost {
    pub fn open(root: impl AsRef<Path>) -> cs2_runtime::RuntimeResult<Self> {
        Ok(Self {
            store: FileSaveStore::open(root)?,
        })
    }

    pub fn root(&self) -> &Path {
        self.store.root()
    }

    pub fn save_kind(
        &mut self,
        kind: cs2_runtime::SaveKind,
        state: &RuntimeState,
    ) -> cs2_runtime::RuntimeResult<()> {
        self.store.save(kind, state)
    }

    pub fn load_kind(
        &mut self,
        kind: cs2_runtime::SaveKind,
    ) -> cs2_runtime::RuntimeResult<Option<RuntimeState>> {
        self.store.load(kind)
    }

    pub fn load_kind_for_restart(
        &mut self,
        kind: cs2_runtime::SaveKind,
    ) -> cs2_runtime::RuntimeResult<Option<RuntimeState>> {
        self.store.load_for_restart(kind)
    }

    pub fn save_ordinary(
        &mut self,
        slot: u32,
        state: &RuntimeState,
    ) -> cs2_runtime::RuntimeResult<()> {
        self.store
            .save(cs2_runtime::SaveKind::Ordinary(slot), state)
    }

    pub fn load_ordinary(&mut self, slot: u32) -> cs2_runtime::RuntimeResult<Option<RuntimeState>> {
        self.store.load(cs2_runtime::SaveKind::Ordinary(slot))
    }

    pub fn delete_ordinary(&mut self, slot: u32) -> cs2_runtime::RuntimeResult<()> {
        self.store.delete(cs2_runtime::SaveKind::Ordinary(slot))
    }

    pub fn ordinary_slots(&mut self) -> cs2_runtime::RuntimeResult<Vec<u32>> {
        self.store.list_slots()
    }

    pub fn delete_kind(&mut self, kind: cs2_runtime::SaveKind) -> cs2_runtime::RuntimeResult<()> {
        self.store.delete(kind)
    }

    pub fn list_ordinary_slots(&mut self) -> cs2_runtime::RuntimeResult<Vec<u32>> {
        self.store.list_slots()
    }
}

impl SaveHost for DesktopSaveHost {
    fn save(&mut self, slot: u32, state: &RuntimeState) -> cs2_runtime::RuntimeResult<()> {
        self.store
            .save(cs2_runtime::SaveKind::Ordinary(slot), state)
    }

    fn load(&mut self, slot: u32) -> cs2_runtime::RuntimeResult<Option<RuntimeState>> {
        self.store.load(cs2_runtime::SaveKind::Ordinary(slot))
    }

    fn delete(&mut self, slot: u32) -> cs2_runtime::RuntimeResult<()> {
        self.store.delete(cs2_runtime::SaveKind::Ordinary(slot))
    }

    fn list_slots(&mut self) -> cs2_runtime::RuntimeResult<Vec<u32>> {
        self.store.list_slots()
    }
}

struct LoggingHost<'a> {
    events: &'a mut Vec<LogicEvent>,
    frame: u64,
    resources: &'a ResourceIndex,
    xml_handles: &'a mut BTreeMap<u32, ConfigXml>,
    next_xml_handle: &'a mut u32,
    configured_shared_slots: usize,
    text_encoding: ScriptTextEncoding,
    shared_values: &'a mut Vec<u32>,
    current_parameter: &'a mut u32,
    current_task_id: u32,
    pointer_position: [i32; 2],
    task_time: u32,
    task_origin: u32,
    auto_select: u32,
    scene_animation_enabled: u32,
    fullscreen: u32,
    fullscreen_mw_pos: u32,
    script_replacements: &'a BTreeMap<Vec<u8>, Vec<u8>>,
    string_values: &'a mut BTreeMap<u32, Vec<u8>>,
    next_task_id: &'a mut u32,
    task_parents: &'a mut BTreeMap<u32, u32>,
    scene_nodes: &'a mut BTreeMap<u32, SceneNode>,
    image_handles: &'a mut BTreeMap<u32, RgbaFrame>,
    image_resources: &'a mut BTreeMap<u32, DesktopImageResource>,
    backlogs: &'a mut BTreeMap<u32, DesktopBacklog>,
    canvas: Option<&'a mut Canvas>,
    back_buffer: Option<&'a Canvas>,
    particles: &'a mut BTreeMap<u32, ParticleObjectState>,
    record_stacks: &'a mut BTreeMap<u32, DesktopRecordStack>,
    input_task: &'a mut DesktopInputTask,
    task_vms: &'a mut BTreeMap<u32, Vm>,
    system_config_blocks: &'a mut [Vec<u8>; 4],
    css_handles: &'a mut BTreeMap<u32, CssFile>,
    integer_maps: &'a mut BTreeMap<u32, BTreeMap<Vec<u8>, u32>>,
    data_services: &'a mut data_services::DataServices,
    cst_contexts: &'a mut BTreeMap<u32, CstContextState>,
    motion_controllers: &'a mut BTreeMap<u32, MotionControllerState>,
    motion_snapshots: &'a mut BTreeMap<u32, MotionSnapshotState>,
    save_root: &'a Path,
    message_wait_address: &'a mut Option<u32>,
    audio_host: &'a mut dyn AudioHost,
    movie_host: Option<&'a mut MpegMovieHost>,
    movie_playbacks: &'a mut BTreeMap<u32, u64>,
    movie_resources: &'a mut BTreeMap<u32, String>,
    sound_channels: &'a mut BTreeMap<u32, SoundChannelState>,
    sound_groups: &'a [SoundGroup],
    sound_config: &'a DesktopSoundConfig,
    script_random: &'a mut ScriptRandom,
    menu_item_requests: &'a mut Vec<(u32, u32)>,
    debug_mode: u32,
    sound_config_enabled: u32,
}

#[derive(Clone)]
struct ScriptRandom {
    integer: RandomStream,
    floating: RandomStream,
}

#[derive(Clone)]
struct RandomStream {
    values: [u32; 624],
    index: usize,
}

impl Default for ScriptRandom {
    fn default() -> Self {
        Self::seeded(4357)
    }
}

impl ScriptRandom {
    fn seeded(seed: u32) -> Self {
        Self {
            integer: RandomStream::seeded(seed),
            floating: RandomStream::seeded(seed),
        }
    }

    fn next_integer(&mut self) -> u32 {
        self.integer.next()
    }

    fn next_float_bits(&mut self) -> u32 {
        ((self.floating.next() as f64 * (1.0 / 4_294_967_295.0)) as f32).to_bits()
    }
}

impl particle_vm::ParticleRandomSource for ScriptRandom {
    fn next_integer(&mut self) -> u32 { ScriptRandom::next_integer(self) }
    fn next_float_bits(&mut self) -> u32 { ScriptRandom::next_float_bits(self) }
}

impl RandomStream {
    fn seeded(mut seed: u32) -> Self {
        let mut values = [0_u32; 624];
        for value in &mut values {
            let high = seed & 0xffff_0000;
            seed = seed.wrapping_mul(69069).wrapping_add(1);
            *value = high | (seed >> 16);
            seed = seed.wrapping_mul(69069).wrapping_add(1);
        }
        Self { values, index: 624 }
    }

    fn next(&mut self) -> u32 {
        if self.index == 624 {
            for index in 0..624 {
                let y = (self.values[index] & 0x8000_0000)
                    | (self.values[(index + 1) % 624] & 0x7fff_ffff);
                self.values[index] = self.values[(index + 397) % 624]
                    ^ (y >> 1)
                    ^ if y & 1 != 0 { 0x9908_b0df } else { 0 };
            }
            self.index = 0;
        }
        let mut value = self.values[self.index];
        self.index += 1;
        value ^= value >> 11;
        value ^= (value << 7) & 0x9d2c_5680;
        value ^= (value << 15) & 0xefc6_0000;
        value ^ (value >> 18)
    }
}

impl Host for LoggingHost<'_> {
    fn call(
        &mut self,
        index: u32,
        arguments: &[u8],
        vm: &mut Vm,
    ) -> cs2_script::Result<NativeReturn> {
        let argument_hex = arguments
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<Vec<_>>()
            .join("");
        self.events.push(LogicEvent {
            frame: self.frame,
            kind: "native-call".to_string(),
            detail: format!(
                "index={index} arguments={} bytes [{argument_hex}]",
                arguments.len()
            ),
        });
        let word = |offset: usize| -> Option<u32> {
            arguments
                .get(offset..offset + 4)
                .map(|bytes| u32::from_le_bytes(bytes.try_into().unwrap()))
        };
        if self.data_services.particle_external.legacy_kc_view() && matches!(index, 543 | 547 | 578 | 579 | 580 | 652) {
            return self.particle_canvas_native(index, arguments, vm);
        }
        if self.data_services.particle_external.legacy_kc_view() && matches!(index, 552 | 554 | 555) {
            return self.particle_canvas_motion_native(index, arguments, vm);
        }
        if javascript_native::JavaScriptStore::supports(index) && arguments.len() >= 4 {
            let requested = read_u32(arguments, 0)?;
            let target = if requested == 0 && matches!(index, 976 | 1006) {
                self.task_parents.get(&self.current_task_id).copied().unwrap_or(0)
            } else { requested };
            if self.data_services.tasks.message_task_state(target)
                .is_some_and(|(kind, _, _)| kind == 55)
                && !self.data_services.native_profile.and_then(|profile| profile.feature(index))
                    .is_some_and(|feature| matches!(feature.kind,
                        cs2_runtime::native_profile::NativeFeatureKind::ExactConstant(_)))
            {
                return self.javascript_native(index, arguments, vm)
                    .map_err(|cause| self.task_error(target, cause));
            }
        }
        if emote_native::EmoteNativeServices::recognizes(index)
            && self.data_services.emote.state.backend_ready()
        {
            return self.emote_native(index, arguments, vm);
        }
        if effekseer_native::supports(index)
            && effekseer_native::identified(self.data_services.native_profile)
        {
            return self.effekseer_native(index, arguments, vm);
        }
        if let Some(result) = self.emote_geometry_native(index, arguments)? {
            return Ok(result);
        }
        if native_tasks::RenderTaskStore::supports(index)
            && !self.data_services.native_profile.and_then(|profile| profile.feature(index))
                .is_some_and(|feature| matches!(feature.kind,
                    cs2_runtime::native_profile::NativeFeatureKind::ExactConstant(_)))
        {
            return self.render_tasks_native(index, arguments, vm);
        }
        if canvas_native::CanvasNativeState::supports(index)
            && !self.data_services.native_profile.and_then(|profile| profile.feature(index))
                .is_some_and(|feature| matches!(feature.kind,
                    cs2_runtime::native_profile::NativeFeatureKind::ExactConstant(_)))
        {
            return self.canvas_native(index, arguments, vm);
        }
        if native_tasks::ExportTaskStore::supports(index) && arguments.len() >= 4
            && self.data_services.tasks.message_task_state(read_u32(arguments, 0)?)
                .is_some_and(|(kind, deletion, _)| matches!(kind, 45 | 46) && deletion == 0)
            && !self.data_services.native_profile.and_then(|profile| profile.feature(index))
                .is_some_and(|feature| matches!(feature.kind,
                    cs2_runtime::native_profile::NativeFeatureKind::ExactConstant(_)))
        {
            return self.export_task_native(index, arguments, vm);
        }
        if index == 576 {
            return self.restore_css_task_native(arguments);
        }
        if let Some(result) = self.css_model_native(index, arguments, vm)? {
            return Ok(result);
        }
        if let Some(result) = self.css_owner_lifecycle_native(index, arguments)? {
            return Ok(result);
        }
        if achievement_native::AchievementState::supports(index)
            && !self.data_services.native_profile.and_then(|profile| profile.feature(index))
                .is_some_and(|feature| matches!(feature.kind,
                    cs2_runtime::native_profile::NativeFeatureKind::ExactConstant(_)))
        {
            return self.achievement_native(index, arguments, vm);
        }
        if matches!(index, 1018..=1023)
            && !self.data_services.native_profile.and_then(|profile| profile.feature(index))
                .is_some_and(|feature| matches!(feature.kind,
                    cs2_runtime::native_profile::NativeFeatureKind::ExactConstant(_)))
        {
            return self.tweet_native(index, arguments, vm);
        }
        if let Some(feature) = self.data_services.native_profile.and_then(|profile| profile.feature(index)) {
            match feature.kind {
                cs2_runtime::native_profile::NativeFeatureKind::ExactConstant(result) => {
                    return Ok(result.native_return());
                }
                cs2_runtime::native_profile::NativeFeatureKind::RequiresImplementation => {
                    return Err(cs2_script::ScriptError::UnimplementedNativeCall { index });
                }
            }
        }
        if matches!(index, 906..=933 | 965 | 966 | 1025 | 1027..=1030 | 1047 | 1048) {
            return Err(cs2_script::ScriptError::UnimplementedNativeCall { index });
        }
        if matches!(index, 430 | 460 | 681 | 682) {
            return self.image_manager_native(index, arguments, vm);
        }
        if native_tasks::NativeTaskStore::supports(index) {
            return self.task_values_native(index, arguments);
        }
        if native_tasks::ObjectGraphReceiver::supports(index) {
            return self.portable_object_graph_native(index, arguments);
        }
        if native_tasks::HgWindowNative::supports(index) {
            return self.hg_window_native(arguments);
        }
        if message_native::MessageNativeServices::supports(index) {
            return self.call_message_native(index, arguments, vm);
        }
        if language_native::LanguageNativeServices::supports(index) {
            return self.data_services.language.call(index, arguments, vm, self.current_task_id);
        }
        if data_services::DataServices::supports(index) {
            return self.data_services.call(index, arguments, vm, self.resources,
                self.css_handles, self.integer_maps, self.next_xml_handle,
                self.text_encoding == ScriptTextEncoding::Utf8, self.text_encoding);
        }
        if let Some(result) = self.effekseer_rotation_native(index, arguments)? {
            return Ok(result);
        }
        if let Some(result) = self.movie_plane_native(index, arguments, vm)? {
            return Ok(result);
        }
        if let Some(result) = self.particle_camera_native(index, arguments)? {
            return Ok(result);
        }
        if index == 116 && let Some(result) = self.image_wrapper_copy_native116(arguments)? {
            return Ok(result);
        }
        if let Some(result) = self.plane_native(index, arguments, vm)? {
            return Ok(result);
        }
        match index {
            741 | 852 | 999 | 1050 => self.settings_option_native(index),
            1077..=1079 => self.settings_control_native(index, arguments),
            29..=32 => self.file_native(index, arguments, vm),
            11 => self.calendar_native(arguments, vm),
            786 => self.weekday_native(arguments),
            10 | 1039 => self.clipboard_native(index, arguments, vm),
            441 => self.frame_capture_native(arguments, vm),
            1052 => self.dictionary_prefix_native(arguments, vm),
            651 => self.reset_task_property_native(arguments),
            739 => self.doubled_device_offset_native(arguments, vm),
            787 => self.display_rectangle_native(arguments, vm),
            874 => self.pack_words_native(arguments),
            875 => self.unpack_words_native(arguments, vm),
            49 => self.find_string_native(arguments, vm),
            196 | 197 | 198 | 200 | 203 => self.transition_native(index, arguments, vm),
            111 | 112 | 114 | 117 => self.image_pixels_native(index, arguments, vm),
            1009 => self.image_export_native1009(arguments, vm),
            257..=259 | 263 | 339 | 455 | 462 | 463 | 464 | 622..=625 | 657 | 663 | 732 | 862 | 879..=881 =>
                self.list_batch_native(index, arguments, vm),
            113 | 118 | 119 | 121 | 241 | 251 | 362..=364 | 369..=376 | 536..=538 | 558 | 641 | 1026 => {
                self.image_wrapper_native(index, arguments, vm)
            }
            367 => self.image_global_blit_native(arguments),
            468 | 564 => self.image_transform_native(index, arguments),
            869..=873 | 877 | 878 => self.database_native(index, arguments, vm),
            1055..=1061 => self.emocon_native(index, arguments, vm),
            447..=449 => self.fes_native(index, arguments, vm),
            647 => self.fes_pause_native(arguments),
            256 | 260 | 261 | 262 | 340..=342 | 534 => self.fes_list_native(index, arguments),
            0 | 2 | 3 => Ok(NativeReturn::Continue),
            457 | 628 | 845..=847 => self.window_control_native(index, arguments),
            549 => self.vm_context_native(vm),
            577 => self.top_clock_native(arguments),
            7 => Err(cs2_script::ScriptError::UnimplementedNativeCall { index }),
            4 => Ok(NativeReturn::Value(self.host_millis())),
            5 if arguments.is_empty() => Ok(NativeReturn::Value(
                self.task_time.wrapping_sub(self.task_origin),
            )),
            5 => Err(native_state_error("native 5 requires zero argument bytes")),
            9 if arguments.len() == 4 => {
                let notify = u32::from_le_bytes(arguments.try_into().unwrap()) == 0;
                self.data_services.host_quit_requested = true;
                self.events.push(LogicEvent {
                    frame: self.frame,
                    kind: "window-close-request".into(),
                    detail: format!("notify={notify}"),
                });
                Ok(NativeReturn::Continue)
            }
            9 => Err(native_state_error(
                "native 9 requires exactly 4 argument bytes",
            )),
            429 | 439 | 543..=548 | 550..=556 | 559..=563 | 573 | 575
            | 578..=581 | 626 | 627 | 642..=644 | 652 | 660 | 661
            | 670..=673 | 686 | 687 => reserved_native::call(
                self.data_services.native_profile.map_or("", |profile| profile.pe_sha256), index,
            ),
            141
            | 169
            | 170
            | 172
            | 173
            | 180
            | 182
            | 184
            | 185
            | 187..=191
            | 574
            | 656
            | 906..=908
            | 910..=924
            | 926..=933
            | 965
            | 966
            | 974..=976
            | 1006
            | 1025
            | 1047
            | 1048 => Ok(NativeReturn::Continue),
            65 => Ok(NativeReturn::YieldVoid),
            171
            | 174..=179
            | 181
            | 183
            | 186
            | 236
            | 432
            | 909
            | 925
            | 1027..=1030 => Ok(NativeReturn::Value(0)),
            33 if arguments.is_empty() => {
                Ok(NativeReturn::Value(self.script_random.next_integer()))
            }
            34 if arguments.is_empty() => {
                Ok(NativeReturn::Value(self.script_random.next_float_bits()))
            }
            6 if arguments.len() == 4 => Ok(NativeReturn::Value(word(0).unwrap())),
            6 => Err(native_state_error(
                "native 6 requires exactly 4 argument bytes",
            )),
            35 if arguments.len() == 12 => {
                let target = word(0).unwrap();
                let source = word(4).unwrap();
                let length = usize::try_from(word(8).unwrap()).unwrap_or(0);
                cs2_runtime::native_copy_memory(
                    vm, target, source, length, self.data_services.memory_copy_mode,
                )?;
                Ok(NativeReturn::Continue)
            }
            35 => Err(native_state_error(
                "native 35 requires exactly 12 argument bytes",
            )),
            1 | 1040 if arguments.len() >= 4 => {
                let source = vm.read_string(word(0).unwrap())?;
                let text = expand_kcs_format_inner(vm, &source, 0, true)?;
                if let Some(event) = self.events.last_mut() {
                    event.detail.push_str(&format!(" debug={:?}", String::from_utf8_lossy(&text)));
                }
                Ok(NativeReturn::Continue)
            }
            8 if arguments.len() == 8 => {
                let [width, height] = self.canvas.as_ref()
                    .map(|canvas| [canvas.width(), canvas.height()])
                    .unwrap_or([0, 0]);
                vm.write(word(0).unwrap(), &width.to_le_bytes())?;
                vm.write(word(4).unwrap(), &height.to_le_bytes())?;
                Ok(NativeReturn::Continue)
            }
            8 => Err(native_state_error(
                "native 8 requires exactly 8 argument bytes",
            )),
            13..=16 => self.string_copy_native(index, arguments, vm),
            17 if arguments.len() == 4 => Ok(NativeReturn::Value(
                vm.read_string(word(0).unwrap())?.len() as u32,
            )),
            17 => Err(native_state_error(
                "native 17 requires exactly 4 argument bytes",
            )),
            18 if arguments.len() == 4 => {
                let source = vm.read_string(word(0).unwrap())?;
                Ok(NativeReturn::Value(
                    expand_kcs_format(vm, &source, 0)?.len() as u32,
                ))
            }
            18 => Err(native_state_error(
                "native 18 requires exactly 4 argument bytes",
            )),
            19..=24 => self.string_scalar_native(index, arguments, vm),
            25 | 46 => self.string_number_native(index, arguments, vm),
            26 if arguments.len() == 4 => {
                let source = expand_kcs_format(vm, &vm.read_string(word(0).unwrap())?, 0)?;
                Ok(NativeReturn::Value(parse_kcs_float_prefix(&source)))
            }
            26 => Err(native_state_error(
                "native 26 requires exactly 4 argument bytes",
            )),
            38 | 40 => self.string_token_native(index, arguments, vm),
            41..=45 => self.string_table_native(index, arguments, vm),
            47 | 48 if arguments.len() >= 4 => {
                let source = vm.read_string(word(0).unwrap())?;
                let source = if index == 48 {
                    expand_kcs_format(vm, &source, 0)?
                } else {
                    source
                };
                Ok(NativeReturn::Value(cp932_char_count(&source) as u32))
            }
            519 => self.string_argument_native519(arguments, vm),
            539 if arguments.len() == 12 => {
                let source_address = word(0).unwrap();
                let target = word(4).unwrap();
                let maximum = word(8).unwrap() as i32;
                Ok(NativeReturn::Value(self.integer_sequence_native(
                    vm,
                    source_address,
                    target,
                    maximum,
                )?))
            }
            539 => Err(native_state_error(
                "native 539 requires exactly 12 argument bytes",
            )),
            540 if arguments.len() == 8 => Ok(NativeReturn::Value(self.word_offset_native(
                vm,
                word(0).unwrap(),
                word(4).unwrap(),
            )?)),
            540 => Err(native_state_error(
                "native 540 requires exactly 8 argument bytes",
            )),
            598 => self.string_resource_name_native598(arguments, vm),
            397 if arguments.len() >= 8 => {
                let y = f32::from_bits(word(0).unwrap());
                let x = f32::from_bits(word(4).unwrap());
                Ok(NativeReturn::Value(y.atan2(x).to_bits()))
            }
            461 => {
                let bits = read_u32(arguments, 0)?;
                let value = f32::from_bits(bits);
                // Original promotes f32 to the double CRT log10 entry and
                // rounds once on return. Preserve quieted NaN payloads.
                let output = if value.is_nan() {
                    bits | 0x0040_0000
                } else if value < 0.0 {
                    0xffc0_0000
                } else {
                    (f64::from(value).log10() as f32).to_bits()
                };
                Ok(NativeReturn::Value(output))
            }
            50..=64
                if (matches!(index, 59 | 61) && arguments.len() == 8)
                    || (!matches!(index, 59 | 61) && arguments.len() == 4) =>
            {
                let first = word(0).unwrap();
                let second = word(4).unwrap_or(0);
                Ok(NativeReturn::Value(native_scalar(index, first, second)))
            }
            50..=64 => Err(native_state_error(
                "scalar native argument byte count is invalid",
            )),
            213 if arguments.len() >= 4 => {
                let source = vm.read_string(word(0).unwrap())?;
                let value = self.eval_script_expression(&source, false)?;
                Ok(NativeReturn::Value(value as u32))
            }
            318 if arguments.len() == 20 => {
                let from = f32::from_bits(word(0).unwrap());
                let to = f32::from_bits(word(4).unwrap());
                let start = f32::from_bits(word(8).unwrap());
                let end = f32::from_bits(word(12).unwrap());
                let now = f32::from_bits(word(16).unwrap());
                let value = if start >= now {
                    from
                } else if now >= end {
                    to
                } else {
                    let elapsed = now - start;
                    let duration = end - start;
                    from + ((to - from) * (elapsed / duration))
                };
                Ok(NativeReturn::Value(value.to_bits()))
            }
            318 => Err(native_state_error(
                "native 318 requires exactly 20 argument bytes",
            )),
            319 if arguments.len() == 4 => {
                let value = f32::from_bits(word(0).unwrap());
                let rounded = (value + 0.5).floor();
                Ok(NativeReturn::Value(float_to_i32(rounded) as u32))
            }
            319 => Err(native_state_error(
                "native 319 requires exactly 4 argument bytes",
            )),
            320 if arguments.len() == 8 => {
                prepare_spline(vm, word(0).unwrap(), word(4).unwrap())?;
                Ok(NativeReturn::Continue)
            }
            320 => Err(native_state_error(
                "native 320 requires exactly 8 argument bytes",
            )),
            630 => self.fes_named_block_native(arguments, vm),
            629 => self.system_open_native(arguments, vm),
            445 => self.system_dialog_native(arguments, vm),
            600 | 601 => self.wallpaper_native(index, arguments, vm),
            284 => self.sound_exists_native284(arguments, vm),
            514 => self.motion_contains_native514(arguments, vm),
            703 | 704 => self.image_records_native(index, arguments, vm),
            705 => self.image_resize_native705(arguments),
            706 | 709 => self.list_entry_record_native(index, arguments, vm),
            707 | 708 => self.list_contract_native(index, arguments),
            294..=317 | 388 | 440 | 535 | 586 => self.motion_native(index, arguments, vm),
            387 => self.append_data_object_native(arguments),
            390 => self.clone_data_container_native(arguments),
            325..=338 | 608 | 982 | 1033 | 1037 | 1038 => self.cst_native(index, arguments, vm),
            991 | 992 | 994 => self.cst_snapshot_native(index, arguments, vm),
            978 | 979 | 980 | 993 => self.cst_container_native(index, arguments),
            981 => self.copy_cst_attached_data_native981(arguments),
            990 | 1031 => self.save_file_native(index, arguments, vm),
            1064 => self.cst_option_native1064(arguments, vm),
            379..=384 => self.cst_window_native(index, arguments, vm),
            567 => self.write_text_file_native567(arguments, vm),
            934 => self.cst_alias_native(arguments, vm),
            451 | 645 | 646 | 649 | 650 | 882 | 883 => self.fes_data_native(index, arguments, vm),
            1045 | 1046 | 1051 => utf8_text_native(index, arguments, vm),
            696..=700 | 785 | 850 | 851 | 876 | 977 | 1034 => {
                self.backlog_native(index, arguments, vm)
            }
            1063 => cp932_utf8_native(vm, arguments),
            265 => self.load_movie_native(arguments, vm),
            201 | 214 | 215 | 903..=905 => self.media_family_native(index, arguments, vm),
            674 => self.resource_category_native674(arguments, vm),
            266..=270 => self.control_movie_native(index, arguments),
            417 | 788 | 997 | 998 => self.movie_options_native(index, arguments),
            271 | 669 | 693 => self.load_particle_native(index, arguments, vm),
            272 | 273 | 274 | 275 | 426 | 427 => self.control_particle_native(index, arguments, vm),
            710..=737 => self.image_resource_native(index, arguments, vm),
            863..=868 => self.record_stack_native(index, arguments, vm),
            145 | 146 | 147 | 148 | 154 | 285 | 286 | 401 | 403 | 404 | 937 => {
                self.input_native(index, arguments, vm)
            }
            36 if arguments.len() == 12 => {
                let target = word(0).unwrap();
                let value = word(4).unwrap() as u8;
                let count = usize::try_from(word(8).unwrap()).unwrap_or(0);
                if count > 64 * 1024 * 1024 {
                    return Err(native_state_error("native 36 fill exceeds 64 MiB"));
                }
                if count != 0 {
                    vm.write(target, &vec![value; count])?;
                }
                Ok(NativeReturn::Continue)
            }
            36 => Err(native_state_error(
                "native 36 requires exactly 12 argument bytes",
            )),
            37 if arguments.len() == 12 => {
                let target = word(0).unwrap();
                let value = word(4).unwrap() as i32;
                let show_sign = word(8).unwrap() != 0;
                let text = format_grouped_integer(value, show_sign);
                write_kcs_terminated(vm, target, text.as_bytes(), 2)?;
                Ok(NativeReturn::Continue)
            }
            37 => Err(native_state_error(
                "native 37 requires exactly 12 argument bytes",
            )),
            66 => self.create_task_native(arguments, vm),
            143 => self.load_ordinary_native(arguments, vm),
            452 | 453 => self.task_property_native(index, arguments),
            454 => self.task_context_native(arguments, vm),
            570..=572 => self.message_list_native(index, arguments),
            279..=283 | 287 | 288 => self.scene_tool_native(index, arguments, vm),
            450 | 666 => self.fes_control_native(index, arguments, vm),
            115 => self.image_blit_native(arguments),
            67 if arguments.len() == 4 => self.delete_task_native(arguments),
            67 => Err(native_state_error(
                "native 67 requires exactly 4 argument bytes",
            )),
            72 | 73 if arguments.len() == 16 => self.cross_task_memory_native(index, arguments, vm),
            72 | 73 => Err(native_state_error(
                "cross-task memory native requires exactly 16 argument bytes",
            )),
            69 | 70 if arguments.len() == 16 => self.task_message_native(index, arguments, vm),
            69 | 70 => Err(native_state_error(
                "task message native requires exactly 16 argument bytes",
            )),
            75 if arguments.len() == 4 => {
                let requested = word(0).unwrap() as usize;
                let mut descendants = self
                    .task_parents
                    .iter()
                    .filter_map(|(&id, &parent)| {
                        if id == self.current_task_id
                            || !self.data_services.tasks.task_is_registered(id)
                        {
                            return None;
                        }
                        let mut current = parent;
                        for _ in 0..1024 {
                            if current == self.current_task_id {
                                return Some(id);
                            }
                            if current == 0 {
                                break;
                            }
                            current = *self.task_parents.get(&current)?;
                        }
                        None
                    })
                    .collect::<Vec<_>>();
                descendants.sort_by_key(|id| *id as i32);
                Ok(NativeReturn::Value(
                    descendants.get(requested).copied().unwrap_or(u32::MAX),
                ))
            }
            75 => Err(native_state_error(
                "native 75 requires exactly 4 argument bytes",
            )),
            108 if arguments.len() == 8 => {
                let target = word(0).unwrap();
                let value = word(4).unwrap();
                self.events.push(LogicEvent {
                    frame: self.frame,
                    kind: "task-response".into(),
                    detail: format!("target={target} value={value}"),
                });
                Ok(NativeReturn::Continue)
            }
            108 => Err(native_state_error(
                "native 108 requires exactly 8 argument bytes",
            )),
            109 if arguments.len() == 4 => {
                let target = word(0).unwrap();
                Ok(NativeReturn::Value(u32::from(
                    target == 0
                        || self.task_parents.contains_key(&target)
                        || self.scene_nodes.contains_key(&target),
                )))
            }
            109 => Err(native_state_error(
                "native 109 requires exactly 4 argument bytes",
            )),
            110 if arguments.len() == 8 => {
                let target = word(0).unwrap();
                let value = word(4).unwrap();
                self.events.push(LogicEvent {
                    frame: self.frame,
                    kind: "task-value".into(),
                    detail: format!("target={target} value={value}"),
                });
                Ok(NativeReturn::Continue)
            }
            110 => Err(native_state_error(
                "native 110 requires exactly 8 argument bytes",
            )),
            79 if arguments.len() >= 16 => {
                let id = word(0).unwrap();
                let node = self.scene_nodes.get_mut(&id).ok_or(
                    cs2_script::ScriptError::InvalidStructuredText {
                        offset: 0,
                        message: "native 79 target task is missing",
                    },
                )?;
                node.size = [
                    f32::from_bits(word(4).unwrap()),
                    f32::from_bits(word(8).unwrap()),
                ];
                node.color = word(12).unwrap();
                node.initialized = true;
                self.data_services.planes.observe_existing(index, arguments, self.scene_nodes)?;
                Ok(NativeReturn::Continue)
            }
            80 if arguments.len() >= 4 => {
                let id = word(0).unwrap();
                let node = self.scene_nodes.get_mut(&id).ok_or(
                    cs2_script::ScriptError::InvalidStructuredText {
                        offset: 0,
                        message: "native 80 target task is missing",
                    },
                )?;
                node.initialized = true;
                self.data_services.planes.observe_existing(index, arguments, self.scene_nodes)?;
                Ok(NativeReturn::Continue)
            }
            83 if arguments.len() >= 4 => {
                let id = word(0).unwrap();
                let node = self.scene_nodes.get_mut(&id).ok_or(
                    cs2_script::ScriptError::InvalidStructuredText {
                        offset: 0,
                        message: "native 83 target task is missing",
                    },
                )?;
                node.initialized = false;
                node.visible = false;
                self.particles.remove(&id);
                self.data_services.particle_external.release(id);
                self.data_services.particle_external.release_canvas(id);
                self.data_services.planes.observe_existing(index, arguments, self.scene_nodes)?;
                Ok(NativeReturn::Continue)
            }
            84 if arguments.len() >= 20 => {
                let id = word(0).unwrap();
                let node = self.scene_nodes.get_mut(&id).ok_or(
                    cs2_script::ScriptError::InvalidStructuredText {
                        offset: 0,
                        message: "native 84 target task is missing",
                    },
                )?;
                node.position = [
                    f32::from_bits(word(4).unwrap()),
                    f32::from_bits(word(8).unwrap()),
                    f32::from_bits(word(12).unwrap()),
                ];
                self.data_services.planes.observe_existing(index, arguments, self.scene_nodes)?;
                Ok(NativeReturn::Continue)
            }
            218 if arguments.len() == 20 => {
                let target = word(0).unwrap();
                let current = vm.read(target, 16)?;
                let mut rectangle = [0_i32; 4];
                for (slot, bytes) in rectangle.iter_mut().zip(current.chunks_exact(4)) {
                    *slot = i32::from_le_bytes(bytes.try_into().unwrap());
                }
                let clip = [
                    word(4).unwrap() as i32,
                    word(8).unwrap() as i32,
                    word(12).unwrap() as i32,
                    word(16).unwrap() as i32,
                ];
                let left = rectangle[0].max(clip[0]);
                let top = rectangle[1].max(clip[1]);
                let right = rectangle[2].min(clip[2]);
                let bottom = rectangle[3].min(clip[3]);
                let result = if left < right && top < bottom {
                    [left, top, right, bottom]
                } else {
                    [0; 4]
                };
                let mut output = Vec::with_capacity(16);
                for value in result {
                    output.extend_from_slice(&value.to_le_bytes());
                }
                vm.write(target, &output)?;
                Ok(NativeReturn::Continue)
            }
            218 => Err(native_state_error(
                "native 218 requires exactly 20 argument bytes",
            )),
            352 if arguments.len() >= 4 => {
                let address = word(0).unwrap();
                let mut vector = read_f32_vector(vm, address)?;
                normalize_f32_vector(&mut vector);
                write_f32_vector(vm, address, vector)?;
                Ok(NativeReturn::Continue)
            }
            353 | 354 if arguments.len() >= 8 => {
                let target = word(0).unwrap();
                let source = word(4).unwrap();
                let left = read_f32_vector(vm, target)?;
                let right = read_f32_vector(vm, source)?;
                let vector = if index == 353 {
                    std::array::from_fn(|slot| left[slot] + right[slot])
                } else {
                    std::array::from_fn(|slot| left[slot] - right[slot])
                };
                write_f32_vector(vm, target, vector)?;
                Ok(NativeReturn::Continue)
            }
            355 | 356 if arguments.len() >= 4 => {
                let vector = read_f32_vector(vm, word(0).unwrap())?;
                let length_squared = vector
                    .into_iter()
                    .fold(0.0_f32, |sum, component| sum + component * component);
                let value = if index == 355 {
                    length_squared.sqrt()
                } else {
                    length_squared
                };
                Ok(NativeReturn::Value(value.to_bits()))
            }
            357 if arguments.len() >= 8 => {
                let target = word(0).unwrap();
                let scale = f32::from_bits(vm.read_u32(word(4).unwrap())?);
                let vector = read_f32_vector(vm, target)?;
                write_f32_vector(vm, target, vector.map(|component| component * scale))?;
                Ok(NativeReturn::Continue)
            }
            358 if arguments.len() >= 8 => {
                let target = word(0).unwrap();
                let matrix = read_f32_matrix(vm, word(4).unwrap())?;
                let vector = read_f32_vector(vm, target)?;
                let mut output = [0.0_f32; 4];
                for row in 0..4 {
                    output[row] = (0..4)
                        .map(|column| matrix[row * 4 + column] * vector[column])
                        .fold(0.0, |sum, value| sum + value);
                }
                write_f32_vector(vm, target, output)?;
                Ok(NativeReturn::Continue)
            }
            88 if arguments.len() >= 8 => {
                let id = word(0).unwrap();
                let node = self.scene_nodes.get_mut(&id).ok_or(
                    cs2_script::ScriptError::InvalidStructuredText {
                        offset: 0,
                        message: "native 88 target task is missing",
                    },
                )?;
                node.position[2] = f32::from_bits(word(4).unwrap());
                self.data_services.planes.observe_existing(index, arguments, self.scene_nodes)?;
                Ok(NativeReturn::Continue)
            }
            92 if arguments.len() >= 8 => {
                let id = word(0).unwrap();
                let node = self.scene_nodes.get_mut(&id).ok_or(
                    cs2_script::ScriptError::InvalidStructuredText {
                        offset: 0,
                        message: "native 92 target task is missing",
                    },
                )?;
                node.visible = word(4).unwrap() != 0;
                self.data_services.planes.observe_existing(index, arguments, self.scene_nodes)?;
                Ok(NativeReturn::Continue)
            }
            100 if arguments.len() >= 12 => {
                let id = word(0).unwrap();
                let node = self.scene_nodes.get_mut(&id).ok_or(
                    cs2_script::ScriptError::InvalidStructuredText {
                        offset: 0,
                        message: "native 100 target task is missing",
                    },
                )?;
                node.overlay_color = if word(8).unwrap() == 0 {
                    0
                } else {
                    word(4).unwrap()
                };
                self.data_services.planes.observe_existing(index, arguments, self.scene_nodes)?;
                Ok(NativeReturn::Continue)
            }
            101 if arguments.len() >= 20 => {
                let id = word(0).unwrap();
                let node = self.scene_nodes.get_mut(&id).ok_or(
                    cs2_script::ScriptError::InvalidStructuredText {
                        offset: 0,
                        message: "native 101 target task is missing",
                    },
                )?;
                node.scale = [
                    f32::from_bits(word(4).unwrap()),
                    f32::from_bits(word(8).unwrap()),
                    f32::from_bits(word(12).unwrap()),
                ];
                self.data_services.planes.observe_existing(index, arguments, self.scene_nodes)?;
                Ok(NativeReturn::Continue)
            }
            104 if arguments.len() >= 28 => {
                let id = word(0).unwrap();
                let node = self.scene_nodes.get_mut(&id).ok_or(
                    cs2_script::ScriptError::InvalidStructuredText {
                        offset: 0,
                        message: "native 104 target task is missing",
                    },
                )?;
                node.position = [
                    word(4).unwrap() as i32 as f32,
                    word(8).unwrap() as i32 as f32,
                    word(12).unwrap() as i32 as f32,
                ];
                node.size = [word(16).unwrap() as f32, word(20).unwrap() as f32];
                node.visible = word(24).unwrap() != 0;
                node.initialized = true;
                Ok(NativeReturn::Continue)
            }
            105 if arguments.len() >= 8 => {
                let id = word(0).unwrap();
                let enabled = word(4).unwrap() != 0;
                if let Some(node) = self.scene_nodes.get_mut(&id) {
                    node.visible = enabled;
                }
                Ok(NativeReturn::Continue)
            }
            848 | 849 if arguments.len() >= 8 => {
                let id = word(0).unwrap();
                let enabled = word(4).unwrap() != 0;
                if let Some(node) = self.scene_nodes.get_mut(&id) {
                    node.initialized = enabled;
                }
                Ok(NativeReturn::Continue)
            }
            68 if arguments.is_empty() => Ok(NativeReturn::Value(self.current_task_id)),
            68 => Err(native_state_error("native 68 requires zero argument bytes")),
            76 if arguments.is_empty() => Ok(NativeReturn::Value(
                self.task_parents
                    .get(&self.current_task_id)
                    .copied()
                    .unwrap_or(0),
            )),
            76 => Err(native_state_error("native 76 requires zero argument bytes")),
            149 if arguments.len() == 8 => {
                vm.write(word(0).unwrap(), &self.pointer_position[0].to_le_bytes())?;
                vm.write(word(4).unwrap(), &self.pointer_position[1].to_le_bytes())?;
                Ok(NativeReturn::Continue)
            }
            149 => Err(native_state_error(
                "native 149 requires exactly 8 argument bytes",
            )),
            71 if arguments.len() == 4 => {
                let address = word(0).unwrap();
                vm.wait_for_message(address)?;
                *self.message_wait_address = Some(address);
                Ok(NativeReturn::YieldVoid)
            }
            400 => self.load_sound_loop_native(arguments, vm),
            935 => self.load_sound_native(arguments, vm),
            936 => self.load_sound_group_native(arguments, vm),
            156 => self.control_sound_native(156, arguments, vm),
            157 => self.control_sound_native(157, arguments, vm),
            158 => self.control_sound_native(158, arguments, vm),
            159 => self.control_sound_native(159, arguments, vm),
            160 => self.control_sound_native(160, arguments, vm),
            161 => self.control_sound_native(161, arguments, vm),
            162 => self.control_sound_native(162, arguments, vm),
            163 => self.control_sound_native(163, arguments, vm),
            164 => self.control_sound_native(164, arguments, vm),
            165 => self.control_sound_native(165, arguments, vm),
            166..=168 | 347..=349 => self.control_sound_native(index, arguments, vm),
            400 if arguments.len() >= 8 => {
                let slot = word(0).unwrap();
                let name = String::from_utf8_lossy(&vm.read_string(word(4).unwrap())?).to_string();
                if let Some(event) = self.events.last_mut() {
                    event
                        .detail
                        .push_str(&format!(" sound_loop slot={slot} name={name:?}"));
                }
                Ok(NativeReturn::Continue)
            }
            207 if arguments.len() == 4 => {
                let mut matrix = [0.0_f32; 16];
                for index in [0, 5, 10, 15] {
                    matrix[index] = 1.0;
                }
                let mut bytes = Vec::with_capacity(64);
                for value in matrix {
                    bytes.extend_from_slice(&value.to_bits().to_le_bytes());
                }
                vm.write(word(0).unwrap(), &bytes)?;
                Ok(NativeReturn::Continue)
            }
            207 => Err(native_state_error(
                "native 207 requires exactly 4 argument bytes",
            )),
            243 if arguments.len() >= 8 => {
                let handle = word(0).unwrap();
                if handle == 0 { return Ok(NativeReturn::Value(0)); }
                let record_id = word(4).unwrap();
                if self.string_values.len() > 1_000_000 {
                    return Err(native_state_error("shared string count exceeds storage limit"));
                }
                let length = self.string_values.values().try_fold(12usize, |length, value| {
                    length.checked_add(5)?.checked_add(value.len())
                        .filter(|&length| length <= 64 * 1024 * 1024)
                }).ok_or_else(|| native_state_error("shared strings save exceeds 64 MiB"))?;
                let mut data = Vec::with_capacity(length);
                data.extend_from_slice(&(self.string_values.len() as u32).to_le_bytes());
                for (&index, value) in self.string_values.iter() {
                    data.extend_from_slice(&index.to_le_bytes());
                    let span = value.len() + 1;
                    let start = data.len();
                    data.resize(start + span, 0);
                    let text = trim_nul(value);
                    data[start..start + text.len()].copy_from_slice(text);
                }
                data.extend_from_slice(&[0; 8]);
                self.css_handles
                    .get_mut(&handle)
                    .ok_or_else(|| audio_script_error("CSS container handle is invalid"))?
                    .append(record_id, &data)
                    .map_err(|_| audio_script_error("shared strings save failed"))?;
                Ok(NativeReturn::Value(1))
            }
            515 | 675 | 885 | 894 if arguments.is_empty() => {
                let handle = self.allocate_handle();
                self.integer_maps.insert(handle, BTreeMap::new());
                Ok(NativeReturn::Value(handle))
            }
            516 | 676 | 886 | 895 if arguments.len() >= 4 => {
                let handle = word(0).unwrap();
                if handle != 0 && self.integer_maps.remove(&handle).is_none() {
                    return Err(audio_script_error("integer map handle is invalid"));
                }
                Ok(NativeReturn::Continue)
            }
            517 if arguments.len() >= 12 => {
                let handle = word(0).unwrap();
                if handle == 0 {
                    return Ok(NativeReturn::Continue);
                }
                let key = vm.read_string(word(4).unwrap())?;
                let map = self
                    .integer_maps
                    .get_mut(&handle)
                    .ok_or_else(|| audio_script_error("integer map handle is invalid"))?;
                if map.len() >= 1_000_000 && !map.contains_key(&key) {
                    return Err(audio_script_error("integer map entry limit exceeded"));
                }
                map.entry(key).or_insert(word(8).unwrap());
                Ok(NativeReturn::Continue)
            }
            518 if arguments.len() >= 8 => {
                let handle = word(0).unwrap();
                if handle == 0 {
                    return Ok(NativeReturn::Value(u32::MAX));
                }
                let key = vm.read_string(word(4).unwrap())?;
                let map = self
                    .integer_maps
                    .get(&handle)
                    .ok_or_else(|| audio_script_error("integer map handle is invalid"))?;
                Ok(NativeReturn::Value(
                    map.get(&key).copied().unwrap_or(u32::MAX),
                ))
            }
            887 | 896 if arguments.len() >= 12 => {
                let handle = word(0).unwrap();
                if handle == 0 {
                    return Ok(NativeReturn::Continue);
                }
                let key = expand_kcs_format(vm, &vm.read_string(word(4).unwrap())?, 0)?;
                let key = trim_nul(&key).to_vec();
                let map = self
                    .integer_maps
                    .get_mut(&handle)
                    .ok_or_else(|| audio_script_error("integer map handle is invalid"))?;
                if !map.contains_key(&key) {
                    let span = key.len().checked_add(4).map(|length| length & !3)
                        .and_then(|length| length.checked_add(4))
                        .ok_or_else(|| native_state_error("ordered map key size overflows"))?;
                    if map.len() >= 1_000_000 || (data_services::DataServices::dictionary_size(map)? as usize)
                        .checked_add(span).is_none_or(|length| length > 64 * 1024 * 1024)
                    {
                        return Err(native_state_error("ordered map storage/count limit exceeded"));
                    }
                }
                // Float887 moves the original four value bytes without any
                // arithmetic; preserve NaN payloads and negative zero.
                map.insert(key, word(8).unwrap());
                Ok(NativeReturn::Continue)
            }
            888 | 897 if arguments.len() >= 8 => {
                let handle = word(0).unwrap();
                if handle == 0 {
                    return Ok(NativeReturn::Value(0));
                }
                let key = expand_kcs_format(vm, &vm.read_string(word(4).unwrap())?, 0)?;
                let key = trim_nul(&key);
                let map = self
                    .integer_maps
                    .get(&handle)
                    .ok_or_else(|| audio_script_error("integer map handle is invalid"))?;
                Ok(NativeReturn::Value(map.get(key).copied().unwrap_or(0)))
            }
            889 | 898 if arguments.len() >= 4 => {
                let handle = word(0).unwrap();
                if handle == 0 {
                    return Ok(NativeReturn::Continue);
                }
                self.integer_maps
                    .get_mut(&handle)
                    .ok_or_else(|| audio_script_error("integer map handle is invalid"))?
                    .clear();
                Ok(NativeReturn::Continue)
            }
            890 | 899 if arguments.len() >= 4 => {
                let handle = word(0).unwrap();
                if handle == 0 {
                    return Ok(NativeReturn::Value(0));
                }
                let map = self
                    .integer_maps
                    .get(&handle)
                    .ok_or_else(|| audio_script_error("integer map handle is invalid"))?;
                let size = map.keys().try_fold(4_u32, |size, key| {
                    let key_size = u32::try_from(key.len().checked_add(4)? & !3).ok()?;
                    size.checked_add(key_size)?.checked_add(4)
                });
                Ok(NativeReturn::Value(size.unwrap_or(u32::MAX)))
            }
            893 | 902 if arguments.len() >= 12 => {
                let handle = word(0).unwrap();
                if handle == 0 {
                    return Ok(NativeReturn::Value(0));
                }
                let index = word(4).unwrap() as usize;
                let target = word(8).unwrap();
                let map = self
                    .integer_maps
                    .get(&handle)
                    .ok_or_else(|| audio_script_error("integer map handle is invalid"))?;
                if let Some(key) = map.keys().nth(index) {
                    write_kcs_terminated(vm, target, key, 1)?;
                    Ok(NativeReturn::Value(1))
                } else {
                    Ok(NativeReturn::Value(0))
                }
            }
            253 if arguments.len() >= 8 => {
                let handle = word(0).unwrap();
                if handle == 0 { return Ok(NativeReturn::Value(0)); }
                let record_id = word(4).unwrap();
                let Some(data) = self
                    .css_handles
                    .get(&handle)
                    .ok_or_else(|| audio_script_error("CSS container handle is invalid"))?
                    .try_record(record_id)
                    .map_err(|_| audio_script_error("shared strings CSS record is malformed"))?
                    .map(ToOwned::to_owned)
                else {
                    return Ok(NativeReturn::Value(0));
                };
                let count = read_u32(&data, 0)
                    .map_err(|_| audio_script_error("shared strings record is malformed"))?
                    as usize;
                if count > 1_000_000 {
                    return Err(native_state_error("shared strings record count exceeds limit"));
                }
                let mut cursor = 4usize;
                let mut restored = BTreeMap::new();
                for _ in 0..count {
                    let index = read_u32(&data, cursor)
                        .map_err(|_| audio_script_error("shared strings record is truncated"))?;
                    cursor += 4;
                    let end = data[cursor..]
                        .iter()
                        .position(|byte| *byte == 0)
                        .map(|offset| cursor + offset)
                        .ok_or_else(|| audio_script_error("shared string is unterminated"))?;
                    restored.entry(index).or_insert_with(|| data[cursor..end].to_vec());
                    cursor = end + 1;
                }
                *self.string_values = restored;
                Ok(NativeReturn::Value(1))
            }
            323 => {
                let handle = self.allocate_handle();
                let mut languages = self.data_services.language.languages.prefixes.iter()
                    .map(|name| trim_nul(name).to_vec()).collect::<Vec<_>>();
                languages.sort();
                languages.dedup();
                self.cst_contexts.insert(handle, CstContextState {
                    languages, ..CstContextState::default()
                });
                Ok(NativeReturn::Value(handle))
            }
            324 if arguments.len() >= 4 => {
                let handle = word(0).unwrap();
                if handle == 0 {
                    return Ok(NativeReturn::Continue);
                }
                self.cst_contexts
                    .remove(&handle)
                    .ok_or_else(|| audio_script_error("CST context handle is invalid"))?;
                Ok(NativeReturn::Continue)
            }
            385 if arguments.len() == 16 || arguments.len() >= 20 => {
                let container_handle = word(0).unwrap();
                let record_id = word(4).unwrap();
                let context_handle = word(8).unwrap();
                if container_handle == 0 || context_handle == 0 {
                    return Ok(NativeReturn::Value(0));
                }
                let table_index = word(12).unwrap();
                let layout = match word(16) {
                    Some(include_data) => CstTableLayout::Current {
                        include_data: include_data != 0,
                    },
                    None => CstTableLayout::Legacy,
                };
                let entries = self
                    .cst_contexts
                    .get(&context_handle)
                    .ok_or_else(|| audio_script_error("CST context handle is invalid"))?
                    .table(table_index);
                let bytes = serialize_cst_table_with_layout(entries, layout)?;
                let container = self
                    .css_handles
                    .get_mut(&container_handle)
                    .ok_or_else(|| audio_script_error("CSS container handle is invalid"))?;
                container
                    .append(record_id, &bytes)
                    .map_err(|_| audio_script_error("CST table append failed"))?;
                Ok(NativeReturn::Value(1))
            }
            386 if arguments.len() >= 16 => {
                let container_handle = word(0).unwrap();
                let record_id = word(4).unwrap();
                let context_handle = word(8).unwrap();
                if container_handle == 0 || context_handle == 0 {
                    return Ok(NativeReturn::Value(0));
                }
                let table_index = word(12).unwrap();
                if !self.cst_contexts.contains_key(&context_handle) {
                    return Err(audio_script_error("CST context handle is invalid"));
                }
                let bytes = self
                    .css_handles
                    .get(&container_handle)
                    .ok_or_else(|| audio_script_error("CSS container handle is invalid"))?
                    .try_record(record_id)
                    .map_err(|_| save_script_error("CST CSS record storage is malformed"))?
                    .map(ToOwned::to_owned);
                let Some(bytes) = bytes else {
                    return Ok(NativeReturn::Value(0));
                };
                let entries = if bytes.is_empty() {
                    Vec::new()
                } else {
                    parse_cst_table(&bytes)?
                };
                if table_index == 0 && entries.iter().any(|entry| !entry.data.is_empty()) {
                    return Err(save_script_error(
                        "CST registration data restore needs the absent second input block in the original ABI",
                    ));
                }
                self.cst_contexts
                    .get_mut(&context_handle)
                    .ok_or_else(|| audio_script_error("CST context handle is invalid"))?
                    .replace_table(table_index, entries);
                Ok(NativeReturn::Value(1))
            }
            276 if arguments.len() >= 8 => {
                let index = word(0).unwrap();
                let value = vm.read_string(word(4).unwrap())?;
                self.string_values.insert(index, value);
                Ok(NativeReturn::Continue)
            }
            277 if arguments.len() >= 8 => self.read_shared_string_native277(arguments, vm),
            278 if arguments.len() >= 4 => self.expand_shared_string_native278(arguments, vm),
            242 | 243 | 244 | 245 | 248 | 252 | 253 | 254 | 389 if word(0) == Some(0) =>
                Ok(NativeReturn::Value(0)),
            246 | 469 if word(0) == Some(0) => Ok(NativeReturn::Continue),
            238 if arguments.len() >= 4 => {
                let handle = self.allocate_handle();
                self.css_handles
                    .insert(handle, CssFile::new(word(0).unwrap()));
                Ok(NativeReturn::Value(handle))
            }
            246 if arguments.len() >= 4 => {
                self.css_handles.remove(&word(0).unwrap())
                    .ok_or_else(|| audio_script_error("CSS handle is invalid or already released"))?;
                Ok(NativeReturn::Continue)
            }
            247 if arguments.len() >= 4 => {
                let filename_source = vm.read_string(word(0).unwrap())?;
                let filename = expand_kcs_format(vm, &filename_source, 0)?;
                let filename = self.text_encoding.decode(trim_nul(&filename));
                let file = match read_save_css(self.save_root, &filename) {
                    Ok(bytes) => CssFile::from_css(&bytes)
                        .map_err(|_| audio_script_error("CSS file structure or capacity is invalid"))?,
                    Err(_) => None,
                };
                let handle = file
                    .map(|file| {
                        let handle = self.allocate_handle();
                        self.css_handles.insert(handle, file);
                        handle
                    })
                    .unwrap_or(0);
                Ok(NativeReturn::Value(handle))
            }
            244 if arguments.len() >= 16 => {
                let handle = word(0).unwrap();
                if handle == 0 {
                    return Ok(NativeReturn::Value(0));
                }
                let record_id = word(4).unwrap();
                let length = usize::try_from(word(8).unwrap()).unwrap_or(0);
                let source = word(12).unwrap();
                if vm.read(source, 1).is_err() { return Ok(NativeReturn::Value(0)); }
                let bytes = vm.read(source, length)?;
                let Some(file) = self.css_handles.get_mut(&handle) else {
                    return Err(audio_script_error("CSS handle is invalid"));
                };
                file.append(record_id, &bytes)
                    .map_err(|_| audio_script_error("CSS record append failed"))?;
                Ok(NativeReturn::Value(1))
            }
            245 if arguments.len() >= 8 => {
                let handle = word(0).unwrap();
                if handle == 0 {
                    return Ok(NativeReturn::Value(0));
                }
                let Some(file) = self.css_handles.get(&handle) else {
                    return Err(audio_script_error("CSS handle is invalid"));
                };
                let filename_source = vm.read_string(word(4).unwrap())?;
                let expanded = expand_kcs_format(vm, &filename_source, 0)?;
                let filename = self.text_encoding.decode(trim_nul(&expanded));
                let data = file
                    .to_css()
                    .map_err(|_| audio_script_error("CSS serialization failed"))?;
                if write_save_css(self.save_root, &filename, &data).is_ok() {
                    Ok(NativeReturn::Value(1))
                } else {
                    Ok(NativeReturn::Value(0))
                }
            }
            242 if arguments.len() >= 12 => {
                let target_handle = word(0).unwrap();
                if target_handle == 0 {
                    return Ok(NativeReturn::Value(0));
                }
                let record_id = word(4).unwrap();
                let source_handle = word(8).unwrap();
                if source_handle == 0 {
                    return Ok(NativeReturn::Value(0));
                }
                let snapshot = self
                    .css_handles
                    .get(&source_handle)
                    .ok_or_else(|| audio_script_error("CSS source handle is invalid"))?
                    .snapshot()
                    .map_err(|_| audio_script_error("CSS snapshot serialization failed"))?;
                let target = self
                    .css_handles
                    .get_mut(&target_handle)
                    .ok_or_else(|| audio_script_error("CSS target handle is invalid"))?;
                target
                    .append(record_id, &snapshot)
                    .map_err(|_| audio_script_error("CSS snapshot append failed"))?;
                Ok(NativeReturn::Value(1))
            }
            252 if arguments.len() >= 8 => {
                let handle = word(0).unwrap();
                if handle == 0 {
                    return Ok(NativeReturn::Value(0));
                }
                let record_id = word(4).unwrap();
                let Some(snapshot) = self
                    .css_handles
                    .get(&handle)
                    .ok_or_else(|| audio_script_error("CSS handle is invalid"))?
                    .try_record(record_id)
                    .map_err(|_| audio_script_error("CSS nested record query failed"))?
                    .map(ToOwned::to_owned)
                else {
                    return Ok(NativeReturn::Value(0));
                };
                let file = CssFile::from_snapshot(&snapshot)
                    .map_err(|_| audio_script_error("CSS snapshot restoration failed"))?;
                let new_handle = self.allocate_handle();
                self.css_handles.insert(new_handle, file);
                Ok(NativeReturn::Value(new_handle))
            }
            248 if arguments.len() >= 4 => {
                let handle = word(0).unwrap();
                if handle == 0 {
                    return Ok(NativeReturn::Value(0));
                }
                let tag = self
                    .css_handles
                    .get(&handle)
                    .ok_or_else(|| audio_script_error("CSS handle is invalid"))?
                    .tag;
                Ok(NativeReturn::Value(tag))
            }
            254 if arguments.len() >= 16 => {
                let handle = word(0).unwrap();
                if handle == 0 {
                    return Ok(NativeReturn::Value(0));
                }
                let file = self.css_handles.get(&handle)
                    .ok_or_else(|| audio_script_error("CSS handle is invalid"))?;
                let record_id = word(4).unwrap();
                let target = word(8).unwrap();
                let capacity = word(12).unwrap() as usize;
                if vm.read(target, 1).is_err() { return Ok(NativeReturn::Value(0)); }
                let Some(record) = file.try_record(record_id)
                    .map_err(|_| audio_script_error("CSS record query failed"))?
                else {
                    return Ok(NativeReturn::Value(0));
                };
                let count = record.len().min(capacity);
                vm.write(target, &record[..count])?;
                Ok(NativeReturn::Value(count as u32))
            }
            389 if arguments.len() >= 4 => {
                let handle = word(0).unwrap();
                if handle == 0 {
                    return Ok(NativeReturn::Value(0));
                }
                let file = self
                    .css_handles
                    .get_mut(&handle)
                    .ok_or_else(|| audio_script_error("CSS handle is invalid"))?;
                file.clear();
                Ok(NativeReturn::Value(1))
            }
            469 if arguments.len() >= 8 => {
                let handle = word(0).unwrap();
                if handle == 0 {
                    return Ok(NativeReturn::Continue);
                }
                let file = self.css_handles.get_mut(&handle)
                    .ok_or_else(|| audio_script_error("CSS handle is invalid"))?;
                let record_id = word(4).unwrap();
                file.try_remove(record_id)
                    .map_err(|_| audio_script_error("CSS record removal failed"))?;
                Ok(NativeReturn::Continue)
            }
            393 if arguments.len() == 4 => {
                let value = word(0).unwrap();
                self.menu_item_requests.push((self.current_task_id, value));
                Ok(NativeReturn::Continue)
            }
            393 => Err(native_state_error(
                "native 393 requires exactly 4 argument bytes",
            )),
            465 if arguments.is_empty() => Ok(NativeReturn::Value(self.debug_mode)),
            465 => Err(native_state_error(
                "native 465 requires zero argument bytes",
            )),
            565 => Ok(NativeReturn::Value(self.data_services.frame_capture.enabled)),
            566 => Ok(NativeReturn::Value(self.data_services.frame_capture.next_index)),
            789 if arguments.is_empty() => {
                Ok(NativeReturn::Value(self.configured_shared_slots as u32))
            }
            789 => Err(native_state_error(
                "native 789 requires zero argument bytes",
            )),
            790 if arguments.len() == 16 => {
                self.flag_range_native(arguments, vm)
            }
            790 => Err(native_state_error(
                "native 790 requires exactly 16 argument bytes",
            )),
            938 if arguments.is_empty() => Ok(NativeReturn::Value(self.sound_config_enabled)),
            938 => Err(native_state_error(
                "native 938 requires zero argument bytes",
            )),
            939..=942 if arguments.is_empty() => {
                let value = match index {
                    939 => self.sound_config.master_volume,
                    940 => self.sound_config.master_play_enabled,
                    941 => self.sound_config.master_system_voice,
                    942 => Some(if self.sound_config.master_mute == -1 {
                        0
                    } else {
                        self.sound_config.master_mute
                    }),
                    _ => unreachable!(),
                }
                .ok_or_else(|| native_state_error("sound master setting is absent"))?;
                Ok(NativeReturn::Value(value as u32))
            }
            939..=942 => Err(native_state_error(
                "sound master query requires zero argument bytes",
            )),
            943..=964 | 995 | 1000 | 1001 if arguments.len() == 4 => {
                let source = vm.read_string(word(0).unwrap())?;
                let name = expand_kcs_format(vm, &source, 0)?;
                let name = trim_nul(&name);
                let value = if index == 964 || index == 995 {
                    self.sound_config
                        .effective_file_volume(name, index == 995)
                        .ok_or_else(|| {
                            native_state_error("sound master volume is not configured")
                        })?
                } else if index == 1000 {
                    self.sound_config
                        .file_part(name)
                        .map_or(0, |part| part.same_id)
                } else if index == 1001 {
                    self.sound_config.file_part(name).map_or(-1, |part| part.id)
                } else {
                    let mode = (index - 943) / 7;
                    let field = (index - 943) % 7;
                    let part = match mode {
                        0 => self.sound_config.file_part(name),
                        1 => self.sound_config.type_part(name),
                        _ => self.sound_config.group_part(name),
                    };
                    match field {
                        0 => part.map_or(100, |part| part.volume),
                        1 => part.map_or(i32::from(mode != 1), |part| part.play_enabled),
                        2 => part.map_or(i32::from(mode != 1), |part| part.system_voice),
                        3 => part.map_or(100, |part| part.master_volume),
                        4 => part.map_or(100, |part| part.voice_volume),
                        5 => part.map_or(-1, |part| part.skip_volume),
                        6 => part.map_or(if mode == 2 { 0 } else { 100 }, |part| {
                            if part.mute == -1 { 0 } else { part.mute }
                        }),
                        _ => unreachable!(),
                    }
                };
                Ok(NativeReturn::Value(value as u32))
            }
            943..=964 | 995 | 1000 | 1001 => Err(native_state_error(
                "sound named query requires a format string address",
            )),
            1002 if arguments.len() == 8 => {
                let target = word(0).unwrap();
                let source = expand_kcs_format(vm, &vm.read_string(word(4).unwrap())?, 0)?;
                let mut output = source;
                for (find, replacement) in self.script_replacements {
                    if find.is_empty() {
                        return Err(native_state_error("native 1002 replacement key is empty"));
                    }
                    output = replace_all_non_overlapping(&output, find, replacement);
                }
                write_kcs_terminated(vm, target, &output, 1)?;
                Ok(NativeReturn::Continue)
            }
            1002 => Err(native_state_error(
                "native 1002 requires exactly 8 argument bytes",
            )),
            605..=607 | 844 => self.legacy_sound_native(index, arguments, vm),
            359 if arguments.len() == 16 => {
                self.ensure_legacy_sound_settings()?;
                for (index, block) in self.system_config_blocks.iter().enumerate() {
                    let address = word(index * 4).unwrap();
                    if vm.read(address, 1).is_ok() {
                        vm.write(address, block)?;
                    }
                }
                Ok(NativeReturn::Continue)
            }
            360 if arguments.len() == 12 => {
                self.ensure_legacy_sound_settings()?;
                for (index, block) in self.system_config_blocks.iter_mut().take(3).enumerate() {
                    let address = word(index * 4).unwrap();
                    if vm.read(address, 1).is_ok() {
                        let bytes = vm.read(address, block.len())?;
                        block.copy_from_slice(&bytes);
                    }
                }
                Ok(NativeReturn::Continue)
            }
            359 | 360 => Err(cs2_script::ScriptError::InvalidStructuredText {
                offset: 0,
                message: "system native argument byte count is invalid",
            }),
            361 => self.save_configuration_native(),
            590 if arguments.len() >= 4 => {
                let expanded = expand_kcs_format(vm, &vm.read_string(word(0).unwrap())?, 0)?;
                let name = self.text_encoding.decode(trim_nul(&expanded));
                if let Some(event) = self.events.last_mut() {
                    event.detail.push_str(&format!(" xml_resource={name:?}"));
                }
                let bytes = match self.resources.read(&name) {
                    Ok(bytes) => bytes,
                    Err(RuntimeError::ResourceNotFound(_)) => return Ok(NativeReturn::Value(0)),
                    Err(_) => return Err(native_state_error("XML resource read failed")),
                };
                let Ok(document) = ConfigXml::parse_runtime(&bytes) else {
                    return Ok(NativeReturn::Value(0));
                };
                let handle = *self.next_xml_handle;
                *self.next_xml_handle = self.next_xml_handle.saturating_add(1).max(1);
                self.xml_handles.insert(handle, document);
                Ok(NativeReturn::Value(handle))
            }
            591 if arguments.len() >= 4 => {
                let handle = word(0).unwrap();
                if handle != 0 && self.xml_handles.remove(&handle).is_none() {
                    return Err(native_state_error("XML handle is invalid or has a different type"));
                }
                Ok(NativeReturn::Continue)
            }
            592 if arguments.len() >= 8 && word(0) == Some(0) => {
                Ok(NativeReturn::Value(word(4).unwrap()))
            }
            592 if arguments.len() >= 12 => {
                let handle = word(0).unwrap();
                let default = word(4).unwrap();
                if handle == 0 { return Ok(NativeReturn::Value(default)); }
                let path_source = vm.read_string(word(8).unwrap())?;
                let path = expand_kcs_format(vm, &path_source, 0)?;
                if let Some(event) = self.events.last_mut() {
                    event
                        .detail
                        .push_str(&format!(" xml_path={:?} default={default}", String::from_utf8_lossy(trim_nul(&path))));
                }
                let document = self.xml_handles.get(&handle)
                    .ok_or_else(|| native_state_error("XML handle is invalid or has a different type"))?;
                let path = trim_nul(&path);
                let mut parts: Vec<_> = path.split(|&byte| byte == b'/').filter(|part| !part.is_empty()).collect();
                if parts.is_empty() { return Ok(NativeReturn::Value(default)); }
                if parts.iter().any(|part| part.len() > 511) {
                    return Err(native_state_error("XML query path segment exceeds 511 bytes"));
                }
                if document.root == b"document" && parts[0] != b"document" {
                    parts.insert(0, b"document");
                }
                Ok(NativeReturn::Value(document.integer(&parts, default as i32) as u32))
            }
            593 if arguments.len() >= 4 && word(0) == Some(0) => Ok(NativeReturn::Continue),
            593 if arguments.len() >= 12 => {
                let handle = word(0).unwrap();
                if handle == 0 { return Ok(NativeReturn::Continue); }
                let target = word(4).unwrap();
                let path_source = vm.read_string(word(8).unwrap())?;
                let path = expand_kcs_format(vm, &path_source, 0)?;
                let document = self.xml_handles.get(&handle)
                    .ok_or_else(|| native_state_error("XML handle is invalid or has a different type"))?;
                {
                    let mut parts: Vec<_> = trim_nul(&path).split(|&byte| byte == b'/')
                        .filter(|part| !part.is_empty()).collect();
                    if parts.iter().any(|part| part.len() > 511) {
                        return Err(native_state_error("XML query path segment exceeds 511 bytes"));
                    }
                    if parts.is_empty() { return Ok(NativeReturn::Continue); }
                    if document.root == b"document" && parts[0] != b"document" {
                        parts.insert(0, b"document");
                    }
                    if let Some(text) = document.text(&parts) {
                        let mut output = text.to_vec();
                        output.push(0);
                        vm.write(target, &output)?;
                    }
                }
                Ok(NativeReturn::Continue)
            }
            _ => Err(cs2_script::ScriptError::UnimplementedNativeCall { index }),
        }
    }
}

impl LoggingHost<'_> {
    fn cross_task_memory_native(
        &mut self,
        index: u32,
        arguments: &[u8],
        vm: &mut Vm,
    ) -> cs2_script::Result<NativeReturn> {
        let target_task = read_u32(arguments, 0)?;
        let target_address = read_u32(arguments, 4)?;
        let count = usize::try_from(read_u32(arguments, 8)?)
            .map_err(|_| native_state_error("cross-task memory length is not representable"))?;
        if count > 64 * 1024 * 1024 {
            return Err(native_state_error(
                "cross-task memory request exceeds the 64 MiB limit",
            ));
        }
        let current_address = read_u32(arguments, 12)?;
        if target_task == self.current_task_id {
            let (destination, source) = if index == 72 {
                (current_address, target_address)
            } else {
                (target_address, current_address)
            };
            cs2_runtime::native_copy_memory(
                vm, destination, source, count, self.data_services.memory_copy_mode,
            )?;
            return Ok(NativeReturn::Continue);
        }
        let other = self
            .task_vms
            .get_mut(&target_task)
            .ok_or_else(|| native_state_error("cross-task VM owner is unavailable"))?;
        if index == 72 {
            cs2_runtime::native_copy_between_vms(
                vm, current_address, other, target_address, count, self.data_services.memory_copy_mode,
            )?;
        } else {
            cs2_runtime::native_copy_between_vms(
                other, target_address, vm, current_address, count, self.data_services.memory_copy_mode,
            )?;
        }
        Ok(NativeReturn::Continue)
    }

    fn record_stack_native(
        &mut self,
        index: u32,
        arguments: &[u8],
        vm: &mut Vm,
    ) -> cs2_script::Result<NativeReturn> {
        let handle = read_u32(arguments, 0)?;
        let required = if matches!(index, 864 | 867) { 4 } else { 8 };
        if arguments.len() != required && !(index != 863 && handle == 0 && arguments.len() == 4) {
            return Err(native_state_error(
                "record stack argument byte count is invalid",
            ));
        }
        if index == 863 {
            let stride = handle;
            let growth = read_u32(arguments, 4)?;
            let capacity = usize::try_from(stride)
                .ok()
                .and_then(|stride| {
                    usize::try_from(growth)
                        .ok()
                        .and_then(|growth| stride.checked_mul(growth))
                })
                .ok_or_else(|| native_state_error("record stack allocation overflows"))?;
            if capacity > 64 * 1024 * 1024 {
                return Err(native_state_error("record stack allocation exceeds 64 MiB"));
            }
            let handle = self.allocate_handle();
            self.record_stacks.insert(
                handle,
                DesktopRecordStack {
                    stride,
                    count: 0,
                    growth,
                    capacity: growth,
                    bytes: vec![0; capacity],
                },
            );
            return Ok(NativeReturn::Value(handle));
        }
        if handle == 0 {
            return Ok(if matches!(index, 864 | 868) {
                NativeReturn::Continue
            } else {
                NativeReturn::Value(0)
            });
        }
        if index == 864 {
            if self.record_stacks.remove(&handle).is_none() {
                return Err(native_state_error("record stack handle is invalid"));
            }
            return Ok(NativeReturn::Continue);
        }
        if index == 868 {
            let source = read_u32(arguments, 4)?;
            if source == 0 {
                return Ok(NativeReturn::Continue);
            }
            let source = self
                .record_stacks
                .get(&source)
                .cloned()
                .ok_or_else(|| native_state_error("record stack source handle is invalid"))?;
            let target = self
                .record_stacks
                .get_mut(&handle)
                .ok_or_else(|| native_state_error("record stack target handle is invalid"))?;
            *target = source;
            return Ok(NativeReturn::Continue);
        }
        let stack = self
            .record_stacks
            .get_mut(&handle)
            .ok_or_else(|| native_state_error("record stack handle is invalid"))?;
        if index == 867 {
            return Ok(NativeReturn::Value(stack.count));
        }
        let address = read_u32(arguments, 4)?;
        if index == 865 {
            if stack.count as i32 >= stack.capacity as i32 {
                let capacity = stack
                    .capacity
                    .checked_add(stack.growth)
                    .ok_or_else(|| native_state_error("record stack capacity overflows"))?;
                let length = usize::try_from(stack.stride)
                    .ok()
                    .and_then(|stride| {
                        usize::try_from(capacity)
                            .ok()
                            .and_then(|capacity| stride.checked_mul(capacity))
                    })
                    .ok_or_else(|| native_state_error("record stack allocation overflows"))?;
                if length > 64 * 1024 * 1024 {
                    return Err(native_state_error("record stack allocation exceeds 64 MiB"));
                }
                stack.bytes.resize(length, 0);
                stack.capacity = capacity;
            }
            if vm.read(address, 1).is_err() {
                return Ok(NativeReturn::Value(0));
            }
            let stride = usize::try_from(stack.stride)
                .map_err(|_| native_state_error("record stack stride is invalid"))?;
            let start = usize::try_from(stack.count)
                .ok()
                .and_then(|count| count.checked_mul(stride))
                .ok_or_else(|| native_state_error("record stack offset overflows"))?;
            let bytes = vm.read(address, stride)?;
            stack
                .bytes
                .get_mut(start..start + stride)
                .ok_or_else(|| native_state_error("record stack capacity is insufficient"))?
                .copy_from_slice(&bytes);
            stack.count = stack.count.wrapping_add(1);
        } else {
            if stack.count as i32 <= 0 || vm.read(address, 1).is_err() {
                return Ok(NativeReturn::Value(0));
            }
            let stride = usize::try_from(stack.stride)
                .map_err(|_| native_state_error("record stack stride is invalid"))?;
            let count = stack.count - 1;
            let start = usize::try_from(count)
                .ok()
                .and_then(|count| count.checked_mul(stride))
                .ok_or_else(|| native_state_error("record stack offset overflows"))?;
            vm.write(address, &stack.bytes[start..start + stride])?;
            stack.count = count;
        }
        Ok(NativeReturn::Value(1))
    }

    fn input_native(
        &mut self,
        index: u32,
        arguments: &[u8],
        vm: &mut Vm,
    ) -> cs2_script::Result<NativeReturn> {
        let required = match index {
            145 | 286 | 404 | 937 => 0,
            146 | 147 | 148 | 154 | 403 => 8,
            285 | 401 => 4,
            _ => return Err(native_state_error("input native index is unsupported")),
        };
        if arguments.len() != required {
            return Err(native_state_error(
                "input native argument byte count is invalid",
            ));
        }
        let word = |offset| read_u32(arguments, offset);
        match index {
            145 => {
                let id = self.current_task_id;
                self.input_task.regions.retain(|region| region.id() != id);
            }
            146 => {
                let id = word(0)?;
                let priority = word(4)? as i32;
                if let Some(region) = self
                    .input_task
                    .regions
                    .iter_mut()
                    .find(|region| region.id() == id)
                {
                    region.words[1] = priority as u32;
                }
            }
            147 => {
                let id = word(0)?;
                let address = word(4)?;
                let bounds = if address == 0 {
                    [0; 4]
                } else {
                    let bytes = vm.read(address, 16)?;
                    std::array::from_fn(|index| {
                        i32::from_le_bytes(bytes[index * 4..index * 4 + 4].try_into().unwrap())
                            as u32
                    })
                };
                if let Some(region) = self
                    .input_task
                    .regions
                    .iter_mut()
                    .find(|region| region.id() == id)
                {
                    region.words[3..7].copy_from_slice(&bounds);
                }
            }
            148 => {
                let id = word(0)?;
                let mask = word(4)?;
                if let Some(region) = self
                    .input_task
                    .regions
                    .iter_mut()
                    .find(|region| region.id() == id)
                {
                    region.words[2] = mask;
                }
            }
            154 => {
                self.input_task.auto_hide = word(0)?;
                self.input_task.idle_limit = word(4)?;
            }
            285 => {
                let key = usize::try_from(word(0)?)
                    .map_err(|_| native_state_error("input key index is invalid"))?;
                if key >= self.input_task.key_masks.len() {
                    return Err(native_state_error("input key index exceeds 0..79"));
                }
                let mut value = 0;
                for mask in self.input_task.key_masks[key] {
                    value |= mask;
                }
                return Ok(NativeReturn::Value(value));
            }
            286 => {
                for region in &mut self.input_task.regions {
                    region.words[7] = 0;
                }
                self.input_task.selected_region = None;
                self.input_task.pointer_position = [-1; 2];
            }
            401 => self.input_task.cursor_requested = word(0)?,
            403 => {
                let key = usize::try_from(word(0)?)
                    .map_err(|_| native_state_error("input key index is invalid"))?;
                if key >= self.input_task.key_enabled.len() {
                    return Err(native_state_error("input key index exceeds 0..79"));
                }
                self.input_task.key_enabled[key] = word(4)? != 0;
            }
            404 => {
                self.input_task.key_enabled.fill(true);
            }
            937 => self.input_task.wheel_repeat = 0,
            _ => unreachable!(),
        }
        Ok(NativeReturn::Continue)
    }

    fn image_resource_native(
        &mut self,
        index: u32,
        arguments: &[u8],
        vm: &mut Vm,
    ) -> cs2_script::Result<NativeReturn> {
        if index == 736 {
            if let Some(value) = self.image_record_fill_native736(arguments)? {
                return Ok(value);
            }
        }
        if index == 710 {
            return self.load_image_resource_native710(arguments, vm);
        }
        if index == 735 {
            return self.image_resource_copy_native735(arguments);
        }
        if index == 733 {
            if arguments.len() != 8 {
                return Err(native_state_error(
                    "native 733 requires exactly 8 argument bytes",
                ));
            }
            let width = read_u32(arguments, 0)?;
            let height = read_u32(arguments, 4)?;
            if width > i32::MAX as u32 || height > i32::MAX as u32 {
                return Err(native_state_error("native 733 image dimensions exceed signed coordinates"));
            }
            let length = (width as usize).checked_mul(height as usize)
                .and_then(|length| length.checked_mul(4))
                .filter(|&length| length <= 128 * 1024 * 1024)
                .ok_or_else(|| native_state_error("native 733 image exceeds 128 MiB"))?;
            let mut pixels = Vec::new();
            pixels.try_reserve_exact(length)
                .map_err(|_| native_state_error("native 733 pixel allocation failed"))?;
            pixels.resize(length, 0);
            let image = RgbaFrame::from_native_rgba8(width, height, pixels,
                cs2_renderer::FrameMetadata::raster([0; 2]))
                .map_err(|_| native_state_error("native 733 image dimensions are invalid"))?;
            let handle = self.allocate_handle();
            self.image_resources.insert(
                handle,
                DesktopImageResource {
                    geometry: image_manager::ResourceGeometry::from_frame(&image),
                    image,
                    owner: Some(self.current_task_id),
                    references: 1,
                    has_source_layer: false,
                    updated: true,
                    uses_alpha: false,
                    auxiliary: 0,
                    attributes: BTreeMap::new(),
                },
            );
            return Ok(NativeReturn::Value(handle));
        }
        let handle = read_u32(arguments, 0)?;
        if index == 736 && handle == 0 {
            if arguments.len() != 24 {
                return Err(native_state_error(
                    "native 736 requires exactly 24 argument bytes",
                ));
            }
            let x = read_u32(arguments, 4)? as i32;
            let y = read_u32(arguments, 8)? as i32;
            let width = read_u32(arguments, 12)? as i32;
            let height = read_u32(arguments, 16)? as i32;
            let color = packed_color(read_u32(arguments, 20)?);
            let Some(canvas) = self.canvas.as_deref_mut() else {
                return Err(native_state_error(
                    "native 736 canvas target is unavailable",
                ));
            };
            canvas
                .fill_rect(
                    Rect::new(x, y, width.max(0) as u32, height.max(0) as u32),
                    color,
                )
                .map_err(|_| native_state_error("native 736 canvas clear failed"))?;
            return Ok(NativeReturn::Continue);
        }
        if handle == 0 {
            return Ok(if matches!(index, 717..=720 | 724) {
                NativeReturn::Value(1)
            } else {
                NativeReturn::Continue
            });
        }
        if index == 711 {
            let resource = self
                .image_resources
                .get_mut(&handle)
                .ok_or_else(|| native_state_error("native 711 image resource handle is invalid"))?;
            if resource.owner == Some(self.current_task_id) {
                resource.owner = None;
            }
            resource.references = resource.references.saturating_sub(1);
            if resource.references == 0 {
                self.image_resources.remove(&handle);
                self.release_image_resource_layers(handle);
            }
            return Ok(NativeReturn::Continue);
        }
        let required = match index {
            712 | 714 | 715 | 717 | 718 | 719 | 720 | 726 | 730 => 12,
            713 => 16,
            721 => 4,
            722 | 723 | 727 | 728 | 737 => 8,
            724 => 28,
            735 => 40,
            736 => 24,
            _ => {
                return Err(native_state_error(
                    "native image resource index is outside the supported table",
                ));
            }
        };
        if arguments.len() != required {
            return Err(native_state_error(
                "native image resource argument byte count is invalid",
            ));
        }
        // Native no-op guards precede even a nonzero resource lookup.
        if (index == 713 && [read_u32(arguments, 4)?, read_u32(arguments, 8)?,
                read_u32(arguments, 12)?] == [100; 3])
            || (index == 737 && !matches!(read_u32(arguments, 4)?, 1..=3)) {
            return Ok(NativeReturn::Continue);
        }
        if index == 736 {
            let x = read_u32(arguments, 4)? as i32;
            let y = read_u32(arguments, 8)? as i32;
            let width = read_u32(arguments, 12)? as i32;
            let height = read_u32(arguments, 16)? as i32;
            let color = packed_color(read_u32(arguments, 20)?);
            let resource = self
                .image_resources
                .get(&handle)
                .ok_or_else(|| native_state_error("native 736 image resource is invalid"))?
                .clone();
            let top = (resource.image.height() as i32)
                .wrapping_sub(y)
                .wrapping_sub(height);
            let image = if resource.image.is_empty() { resource.image.clone() } else { fill_frame_rect(
                &resource.image,
                Rect::new(x, top, width.max(0) as u32, height.max(0) as u32),
                color,
            )? }
            .with_raw_metadata(resource.image.metadata())
            .with_hg_attributes(resource.image.hg_attributes().clone());
            let resource = self.image_resources.get_mut(&handle).unwrap();
            resource.image = image;
            resource.updated = true;
            self.synchronize_list_resource(handle)
                .map_err(|_| native_state_error("resource fill list synchronization failed"))?;
            return Ok(NativeReturn::Continue);
        }
        let original = self
            .image_resources
            .get(&handle)
            .ok_or_else(|| native_state_error("native image resource handle is invalid"))?
            .clone();
        if matches!(index, 713 | 723 | 726 | 727 | 730) && !original.has_source_layer {
            return Err(native_state_error(
                "image operation requires a resource with a source layer",
            ));
        }
        if self.transform_image_resource_layers(handle, index, arguments)? {
            return Ok(NativeReturn::Continue);
        }
        if index == 721 {
            return Ok(NativeReturn::Value(u32::from(original.uses_alpha)));
        }
        if index == 722 {
            let resource = self.image_resources.get_mut(&handle).unwrap();
            resource.uses_alpha = read_u32(arguments, 4)? != 0;
            resource.updated = true;
            return Ok(NativeReturn::Continue);
        }
        if index == 724 {
            let values = original.attributes.get(&read_u32(arguments, 4)?).copied();
            let Some(values) = values else {
                return Ok(NativeReturn::Value(0));
            };
            for (address, value) in arguments[8..].chunks_exact(4).zip(values) {
                vm.write(
                    u32::from_le_bytes(address.try_into().unwrap()),
                    &value.to_le_bytes(),
                )?;
            }
            return Ok(NativeReturn::Value(1));
        }
        if index == 728 {
            self.image_resources.get_mut(&handle).unwrap().auxiliary = read_u32(arguments, 4)?;
            return Ok(NativeReturn::Continue);
        }
        let mut image = original.image.clone();
        let mut metadata = image.metadata();
        match index {
            712 => {
                vm.write(read_u32(arguments, 4)?, &image.width().to_le_bytes())?;
                vm.write(read_u32(arguments, 8)?, &image.height().to_le_bytes())?;
                return Ok(NativeReturn::Continue);
            }
            713 => {
                image = adjust_rgb_frame(
                    &image,
                    [
                        read_u32(arguments, 4)?,
                        read_u32(arguments, 8)?,
                        read_u32(arguments, 12)?,
                    ],
                )?;
            }
            714 => {
                let canvas = image_manager::native_canvas_dimensions(&image);
                vm.write(
                    read_u32(arguments, 4)?,
                    &canvas[0].to_le_bytes(),
                )?;
                vm.write(
                    read_u32(arguments, 8)?,
                    &canvas[1].to_le_bytes(),
                )?;
                return Ok(NativeReturn::Continue);
            }
            715 => {
                vm.write(
                    read_u32(arguments, 4)?,
                    &(metadata.offset[0] as u32).to_le_bytes(),
                )?;
                vm.write(
                    read_u32(arguments, 8)?,
                    &(metadata.offset[1] as u32).to_le_bytes(),
                )?;
                return Ok(NativeReturn::Continue);
            }
            717 => {
                vm.write(
                    read_u32(arguments, 4)?,
                    &(original.geometry.base[0] as u32).to_le_bytes(),
                )?;
                vm.write(
                    read_u32(arguments, 8)?,
                    &(original.geometry.base[1] as u32).to_le_bytes(),
                )?;
                return Ok(NativeReturn::Value(1));
            }
            718 => metadata.canvas_size = [read_u32(arguments, 4)?, read_u32(arguments, 8)?],
            719 => {
                metadata.offset = [
                    read_u32(arguments, 4)? as i32,
                    read_u32(arguments, 8)? as i32,
                ]
            }
            720 => {
                metadata.base = [
                    read_u32(arguments, 4)? as i32,
                    read_u32(arguments, 8)? as i32,
                ]
            }
            723 => {
                let degrees = read_u32(arguments, 4)? as i32;
                image = image
                    .rotate_right_angle(degrees)
                    .map_err(|_| native_state_error("native 723 image rotation failed"))?
                    .ok_or_else(|| {
                        native_state_error("native 723 rotation angle is unsupported")
                    })?;
            }
            726 => image = blur_frame(&image, read_u32(arguments, 4)?, read_u32(arguments, 8)?)?,
            727 => {
                // docs/graphics.md:140: nonzero horizontal, zero vertical.
                image = if read_u32(arguments, 4)? != 0 {
                    image
                        .flip_horizontal()
                        .map_err(|_| native_state_error("native 727 image flip failed"))?
                } else {
                    image
                        .flip_vertical()
                        .map_err(|_| native_state_error("native 727 image flip failed"))?
                };
            }
            730 => {
                image = self.resize_image_canvas(
                    &image, [read_u32(arguments, 4)?, read_u32(arguments, 8)?],
                )?;
            }
            737 => {
                let filter = match read_u32(arguments, 4)? {
                    1 => FrameFilter::Mono,
                    2 => FrameFilter::Sepia,
                    3 => FrameFilter::Negative,
                    _ => return Ok(NativeReturn::Continue),
                };
                image = image
                    .apply_filter(filter)
                    .map_err(|_| native_state_error("native 737 image filter failed"))?
                    .with_raw_metadata(metadata);
            }
            _ => {}
        }
        if matches!(index, 718..=720) {
            // These setters store raw DWORDs. In particular a zero canvas
            // axis remains zero until a native canvas getter resolves it.
            image = image.with_raw_metadata(metadata);
        }
        let resource = self.image_resources.get_mut(&handle).unwrap();
        resource.image = image;
        resource.updated = true;
        resource.uses_alpha = if matches!(index, 718..=720 | 730 | 737) {
            // Resampling preserves the native Alpha classification cache.
            original.uses_alpha
        } else { resource.image.has_alpha() };
        self.synchronize_list_resource(handle)
            .map_err(|_| native_state_error("resource list pixel synchronization failed"))?;
        Ok(if matches!(index, 718..=720) {
            NativeReturn::Value(1)
        } else {
            NativeReturn::Continue
        })
    }

    fn motion_native(
        &mut self,
        index: u32,
        arguments: &[u8],
        vm: &mut Vm,
    ) -> cs2_script::Result<NativeReturn> {
        if index == 294 {
            let handle = self.allocate_handle();
            self.motion_controllers
                .insert(handle, MotionControllerState::default());
            return Ok(NativeReturn::Value(handle));
        }

        let expected = match index {
            295 | 296 | 303 | 311 | 314 | 317 => 4,
            297 | 298 | 305 | 308 | 309 | 310 | 312 | 313 => 8,
            306 => 16,
            299..=302 => 40,
            304 => 28,
            388 => 16,
            315 | 316 if matches!(arguments.len(), 4 | 8) => arguments.len(),
            315 | 316 => {
                return Err(native_state_error(
                    "motion native argument byte count is invalid",
                ));
            }
            307 => 12,
            440 => 4,
            535 => 32,
            586 => 8,
            _ => {
                return Err(native_state_error(
                    "motion native index is outside the supported table",
                ));
            }
        };
        if arguments.len() != expected {
            return Err(native_state_error(
                "motion native argument byte count is invalid",
            ));
        }

        if matches!(index, 304 | 535) {
            return self.motion_start_native(index, arguments, vm);
        }
        if index == 388 {
            return self.motion_restore_native(arguments);
        }
        if matches!(index, 315..=317) {
            return self.motion_snapshot_native(index, arguments);
        }

        let handle = read_u32(arguments, 0)?;
        let query = matches!(index, 307 | 309..=311);
        if handle == 0 {
            return Ok(if query {
                NativeReturn::Value(0)
            } else {
                NativeReturn::Continue
            });
        }

        if index == 295 {
            if self.motion_controllers.remove(&handle).is_none() {
                return Err(native_state_error("motion controller handle is invalid"));
            }
            return Ok(NativeReturn::Continue);
        }

        let controller = self
            .motion_controllers
            .get_mut(&handle)
            .ok_or_else(|| native_state_error("motion controller handle is invalid"))?;
        let mut args = [0_u32; 10];
        for (target, bytes) in args.iter_mut().zip(arguments.chunks_exact(4)) {
            *target = u32::from_le_bytes(bytes.try_into().unwrap());
        }

        match index {
            296 => controller.tasks.clear(),
            297 => {
                controller.tasks.remove(&args[1]);
            }
            298 => {
                let group = args[1] as i32;
                controller.tasks.retain(|_, task| task.group != group);
            }
            299 | 301 => {
                let properties = read_motion_properties(vm, &args[1..])?;
                if index == 299 {
                    controller.base_properties = properties;
                } else {
                    controller.properties = properties;
                }
            }
            300 | 302 => {
                let properties = if index == 300 {
                    controller.base_properties
                } else {
                    controller.properties
                };
                let mut outputs = Vec::new();
                for (group, (start, length)) in motion_property_groups().into_iter().enumerate() {
                    let address = args[group + 1];
                    if index == 302 && group == 8 && vm.read(address, 1).is_err() {
                        continue;
                    }
                    let bytes = properties[start..start + length]
                        .iter()
                        .flat_map(|value| value.to_le_bytes())
                        .collect::<Vec<_>>();
                    outputs.push((address, bytes));
                }
                for (address, bytes) in outputs {
                    vm.write(address, &bytes)?;
                }
            }
            303 => controller.base_properties = controller.properties,
            305 => {
                let task_id = args[1];
                let origin = args[2];
                for (id, task) in &mut controller.tasks {
                    if task_id == 0 || task_id == *id {
                        task.origin = origin;
                    }
                }
            }
            306 => {
                if let Some(task) = controller.tasks.get_mut(&args[1]) {
                    let count = usize::try_from(args[3]).unwrap_or(0).min(256);
                    let bytes = if count == 0 {
                        Vec::new()
                    } else {
                        vm.read(args[2], count * 4)?
                    };
                    task.parameters = [0; 256];
                    for (target, bytes) in task.parameters.iter_mut().zip(bytes.chunks_exact(4)) {
                        *target = u32::from_le_bytes(bytes.try_into().unwrap());
                    }
                }
            }
            307 => {
                let mut services = DesktopMotionServices {
                    random: self.script_random,
                    events: self.events,
                    frame: self.frame,
                };
                return advance_motion_controller(controller, args[1], args[2] != 0,
                    false, &mut services, 10_000);
            }
            440 => {
                let mut services = DesktopMotionServices {
                    random: self.script_random,
                    events: self.events,
                    frame: self.frame,
                };
                return advance_motion_controller(controller, u32::MAX, false,
                    true, &mut services, 10_000);
            }
            308 => controller.speed_bits = args[1],
            586 => controller.external_value = args[1],
            309 => {
                return Ok(NativeReturn::Value(
                    controller
                        .tasks
                        .get(&args[1])
                        .map_or(0, |task| task.running as u32),
                ));
            }
            310 | 311 => {
                let group = (index == 310).then_some(args[1] as i32);
                return Ok(NativeReturn::Value(u32::from(
                    controller.tasks.values().any(|task| {
                        group.is_none_or(|group| group == task.group) && task.running != 0
                    }),
                )));
            }
            312 => {
                if let Some(task) = controller.tasks.get_mut(&args[1]) {
                    task.finishing = 1;
                }
            }
            313 | 314 => {
                let group = (index == 313).then_some(args[1] as i32);
                for task in controller.tasks.values_mut() {
                    if group.is_none_or(|group| group == task.group) {
                        task.finishing = 1;
                    }
                }
            }
            _ => unreachable!("motion index was filtered above"),
        }
        Ok(NativeReturn::Continue)
    }

    fn eval_script_expression(&mut self, source: &[u8], fallback: bool) -> cs2_script::Result<i32> {
        // The expression scratch borrows the current task's actual numeric
        // capacity. Growing it to 2048 silently changed the shared identity's
        // allocation and invalidated 609/610's fixed backup byte count.
        let mut state = ExpressionState::new(self.shared_values.len());
        if let Some(locals) = self.current_local_values() {
            for (target, &source) in state.local.iter_mut().zip(locals) { *target = source as i32; }
        }
        for (destination, source) in state.shared.iter_mut().zip(self.shared_values.iter()) {
            *destination = *source as i32;
        }
        state.current = *self.current_parameter as i32;
        state.strings.clone_from(self.string_values);
        let value = if fallback {
            eval_expression_fallback(source, &mut state)
        } else {
            eval_expression_parameter(source, &mut state)
        };
        for (destination, source) in self.shared_values.iter_mut().zip(state.shared) {
            *destination = source as u32;
        }
        *self.current_parameter = state.current as u32;
        self.commit_expression_locals(&state.local);
        *self.string_values = state.strings;
        value
    }

    fn integer_sequence_native(
        &mut self,
        vm: &mut Vm,
        source_address: u32,
        destination: u32,
        maximum: i32,
    ) -> cs2_script::Result<u32> {
        if vm.read(source_address, 1).is_err() || vm.read(destination, 1).is_err() {
            return Ok(0);
        }
        if vm.read(source_address, 1)?[0] != b'[' {
            if maximum <= 0 {
                return Ok(0);
            }
            let source = vm.read_string(source_address)?;
            if !source.is_ascii() {
                return Err(native_state_error(
                    "native 539 expression requires ASCII input",
                ));
            }
            let value = self.eval_script_expression(&source, true)?;
            vm.write(destination, &(value as u32).to_le_bytes())?;
            return Ok(1);
        }

        let mut scan = source_address.wrapping_add(1);
        loop {
            let byte = vm.read(scan, 1)?[0];
            if matches!(byte, 0 | b',') {
                break;
            }
            scan = scan.wrapping_add(1);
        }
        let mut cursor = source_address.wrapping_add(1);
        let mut written = 0_u32;
        if vm.read(scan, 1)?[0] == b',' {
            while (written as i32) < maximum {
                let next = vm.read(cursor, 1)?[0];
                if matches!(next, 0 | b']') {
                    break;
                }
                let expression = read_native_expression(vm, &mut cursor, b",]")?;
                if !expression.is_ascii() {
                    return Err(native_state_error(
                        "native 539 expression requires ASCII input",
                    ));
                }
                let value = self.eval_script_expression(&expression, true)?;
                vm.write(
                    destination.wrapping_add(written.wrapping_mul(4)),
                    &(value as u32).to_le_bytes(),
                )?;
                written = written.wrapping_add(1);
                if vm.read(cursor, 1)?[0] == b',' {
                    cursor = cursor.wrapping_add(1);
                } else {
                    break;
                }
            }
        } else {
            let first = read_native_expression(vm, &mut cursor, b"-]")?;
            if !first.is_ascii() {
                return Err(native_state_error(
                    "native 539 range expression requires ASCII input",
                ));
            }
            let mut value = self.eval_script_expression(&first, true)?;
            if vm.read(cursor, 1)?[0] == b'-' {
                cursor = cursor.wrapping_add(1);
            }
            let last = read_native_expression(vm, &mut cursor, b"]")?;
            if !last.is_ascii() {
                return Err(native_state_error(
                    "native 539 range expression requires ASCII input",
                ));
            }
            let end = self.eval_script_expression(&last, true)?;
            while value <= end && (written as i32) < maximum {
                vm.write(
                    destination.wrapping_add(written.wrapping_mul(4)),
                    &(value as u32).to_le_bytes(),
                )?;
                written = written.wrapping_add(1);
                value = value.wrapping_add(1);
            }
        }
        Ok(written)
    }

    fn word_offset_native(&mut self, vm: &Vm, address: u32, count: u32) -> cs2_script::Result<u32> {
        if count == 0 {
            return Ok(0);
        }
        let mut offset = 0_u32;
        for _ in 0..count {
            loop {
                let byte = vm.read(address.wrapping_add(offset), 1)?[0];
                if byte == 0 {
                    return Ok(u32::MAX);
                }
                if byte == b' ' {
                    break;
                }
                offset = offset.wrapping_add(1);
            }
            while vm.read(address.wrapping_add(offset), 1)?[0] == b' ' {
                offset = offset.wrapping_add(1);
            }
        }
        Ok(offset)
    }

    fn motion_start_native(
        &mut self,
        index: u32,
        arguments: &[u8],
        vm: &mut Vm,
    ) -> cs2_script::Result<NativeReturn> {
        let controller_handle = read_u32(arguments, 0)?;
        if controller_handle == 0 {
            return Ok(NativeReturn::Value(0));
        }
        if !self.motion_controllers.contains_key(&controller_handle) {
            return Err(native_state_error("motion controller handle is invalid"));
        }

        let name_address = read_u32(arguments, 4)?;
        let mut name = expand_kcs_format(vm, &vm.read_string(name_address)?, 0)?;
        name.truncate(
            name.iter()
                .position(|&byte| byte == 0)
                .unwrap_or(name.len()),
        );
        if name.is_empty() {
            return Ok(NativeReturn::Value(0));
        }
        let decoded = ScriptTextEncoding::Cp932.decode(&name);
        let resolved_name = self.resources.category_names(b"mot", &decoded)
            .map_err(|_| native_state_error("motion resource category resolution failed"))?
            .into_iter().next().ok_or_else(|| native_state_error("motion category resolved no name"))?;
        let Some(bytes) = self.resources.read_mode(&resolved_name, cs2_runtime::resource_router::ResourceMode::LooseFirst)
            .map_err(|_| native_state_error("motion resource routing/read failed"))? else {
            return Ok(NativeReturn::Value(0));
        };
        let program = KcsProgram::parse(&bytes)
            .map_err(|_| native_state_error("motion resource is not a valid KCS program"))?;

        let parameter_count = usize::try_from(read_u32(arguments, 12)?)
            .unwrap_or(0)
            .min(256);
        let parameter_bytes = if parameter_count == 0 {
            Vec::new()
        } else {
            vm.read(read_u32(arguments, 8)?, parameter_count * 4)?
        };
        let mut parameters = [0_u32; 256];
        for (target, bytes) in parameters.iter_mut().zip(parameter_bytes.chunks_exact(4)) {
            *target = u32::from_le_bytes(bytes.try_into().unwrap());
        }

        let saved_data =
            if read_u32(arguments, 20)? != 0 && vm.read(read_u32(arguments, 16)?, 1).is_ok() {
                vm.read(
                    read_u32(arguments, 16)?,
                    usize::try_from(read_u32(arguments, 20)?).unwrap_or(0),
                )?
            } else {
                Vec::new()
            };
        let basename = resolved_name
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(&resolved_name);
        let (name, _, invalid) = encoding_rs::SHIFT_JIS.encode(basename);
        if invalid { return Err(native_state_error("motion task basename is not representable in CP932")); }
        let name = name.into_owned();
        let group = read_u32(arguments, 24)? as i32;
        let flags = if index == 535 {
            read_u32(arguments, 28)?
        } else {
            0
        };
        let external_value = self
            .motion_controllers
            .get(&controller_handle)
            .map_or(0, |controller| controller.external_value);
        let properties = self.motion_controllers[&controller_handle].properties;
        let runner = create_motion_runner(program, &name, &parameters, &saved_data,
            external_value, properties, flags)?;
        let controller = self
            .motion_controllers
            .get_mut(&controller_handle)
            .ok_or_else(|| native_state_error("motion controller handle is invalid"))?;
        let id = controller.next_task_id;
        controller.next_task_id = controller.next_task_id.saturating_add(1).max(1);
        controller.tasks.insert(
            id,
            MotionTaskState {
                name,
                group,
                parameters,
                saved_data,
                snapshot_flags: flags,
                external_value,
                origin: 0,
                elapsed: 0,
                running: 1,
                finishing: 0,
                runner,
            },
        );
        Ok(NativeReturn::Value(id))
    }

    fn motion_restore_native(&mut self, arguments: &[u8]) -> cs2_script::Result<NativeReturn> {
        let container = read_u32(arguments, 0)?;
        if container == 0 {
            return Ok(NativeReturn::Value(0));
        }
        if arguments.len() != 16 {
            return Err(native_state_error(
                "native 388 requires exactly 16 argument bytes",
            ));
        }
        let record = read_u32(arguments, 4)?;
        let controller_handle = read_u32(arguments, 8)?;
        let now = read_u32(arguments, 12)?;
        let bytes = self
            .css_handles
            .get(&container)
            .ok_or_else(|| native_state_error("native 388 CSS container handle is invalid"))?
            .record(record)
            .map(ToOwned::to_owned);
        let Some(bytes) = bytes else {
            return Ok(NativeReturn::Value(0));
        };
        if bytes.len() < 8 {
            return Err(native_state_error(
                "native 388 motion snapshot is truncated",
            ));
        }
        let total_size = snapshot_word(&bytes, 0)?;
        let count = snapshot_word(&bytes, 4)?;
        if total_size as usize > bytes.len() || total_size < 8 {
            return Err(native_state_error(
                "native 388 motion snapshot size is invalid",
            ));
        }
        if count > 1_000_000 {
            return Err(native_state_error(
                "native 388 motion snapshot record count exceeds the limit",
            ));
        }
        if controller_handle != 0 {
            if !self.motion_controllers.contains_key(&controller_handle) {
                return Err(native_state_error("motion controller handle is invalid"));
            }
            self.motion_controllers
                .get_mut(&controller_handle)
                .unwrap()
                .tasks
                .clear();
        }

        let snapshot_handle = self.allocate_handle();
        self.motion_snapshots.insert(
            snapshot_handle,
            MotionSnapshotState {
                controller: controller_handle,
                size: bytes.len(),
                storage: bytes.clone(),
            },
        );

        let mut offset = 8usize;
        for _ in 0..count {
            let record_size = snapshot_word(&bytes, offset)? as usize;
            if record_size == 0 {
                offset = offset
                    .checked_add(0x454)
                    .ok_or_else(|| native_state_error("native 388 record offset overflows"))?;
                continue;
            }
            if record_size < 0x454 {
                return Err(native_state_error(
                    "native 388 motion snapshot record is too small",
                ));
            }
            let end = offset
                .checked_add(record_size)
                .ok_or_else(|| native_state_error("native 388 record size overflows"))?;
            if end > total_size as usize || end > bytes.len() {
                return Err(native_state_error(
                    "native 388 motion snapshot record is truncated",
                ));
            }
            if controller_handle != 0 {
                let name = decode_motion_snapshot_name(&bytes[offset..end])?;
                if !name.is_empty() {
                    let group = snapshot_word(&bytes, offset + 0x44)? as i32;
                    let mut parameters = [0_u32; 256];
                    for (target, chunk) in parameters
                        .iter_mut()
                        .zip(bytes[offset + 0x48..offset + 0x448].chunks_exact(4))
                    {
                        *target = u32::from_le_bytes(chunk.try_into().unwrap());
                    }
                    let saved_length = snapshot_word(&bytes, offset + 0x448)? as usize;
                    let saved_start = offset + 0x454;
                    let saved_end = saved_start.checked_add(saved_length).ok_or_else(|| {
                        native_state_error("native 388 saved data size overflows")
                    })?;
                    if saved_end > end {
                        return Err(native_state_error(
                            "native 388 saved data exceeds record bounds",
                        ));
                    }
                    let snapshot_flags = snapshot_word(&bytes, offset + 0x450)?;
                    let delta = snapshot_word(&bytes, offset + 0x44c)?;
                    if let Some(program) = load_motion_program(self.resources, &name)? {
                        let basename = name
                            .rsplit(|byte| matches!(*byte, b'/' | b'\\'))
                            .next()
                            .unwrap_or(&name)
                            .to_vec();
                        let controller =
                            self.motion_controllers.get_mut(&controller_handle).unwrap();
                        let external_value = controller.external_value;
                        let saved_data = bytes[saved_start..saved_end].to_vec();
                        let runner = create_motion_runner(program, &basename, &parameters,
                            &saved_data, external_value, controller.properties, snapshot_flags)?;
                        let id = controller.next_task_id;
                        controller.next_task_id = controller.next_task_id.saturating_add(1).max(1);
                        controller.tasks.insert(
                            id,
                            MotionTaskState {
                                name: basename,
                                group,
                                parameters,
                                saved_data,
                                snapshot_flags,
                                external_value,
                                origin: now.wrapping_sub(delta),
                                elapsed: 0,
                                running: 1,
                                finishing: 0,
                                runner,
                            },
                        );
                    }
                }
            }
            offset = end;
        }
        Ok(NativeReturn::Value(snapshot_handle))
    }

    fn motion_snapshot_native(
        &mut self,
        index: u32,
        arguments: &[u8],
    ) -> cs2_script::Result<NativeReturn> {
        let handle = read_u32(arguments, 0)?;
        if index == 317 {
            if handle != 0 && self.motion_snapshots.remove(&handle).is_none() {
                return Err(native_state_error("motion snapshot handle is invalid"));
            }
            return Ok(NativeReturn::Continue);
        }
        if handle == 0 {
            return Ok(NativeReturn::Value(0));
        }
        if index == 315 {
            let controller = self
                .motion_controllers
                .get(&handle)
                .ok_or_else(|| native_state_error("motion controller handle is invalid"))?;
            let now = arguments
                .get(4..8)
                .map_or(0, |bytes| u32::from_le_bytes(bytes.try_into().unwrap()));
            let bytes = serialize_motion_snapshot(controller, now)?;
            let snapshot_handle = self.allocate_handle();
            self.motion_snapshots.insert(
                snapshot_handle,
                MotionSnapshotState {
                    controller: handle,
                    size: bytes.len(),
                    storage: bytes,
                },
            );
            return Ok(NativeReturn::Value(snapshot_handle));
        }

        let (controller_handle, previous_capacity) = {
            let snapshot = self
                .motion_snapshots
                .get(&handle)
                .ok_or_else(|| native_state_error("motion snapshot handle is invalid"))?;
            (snapshot.controller, snapshot.storage.len())
        };
        let controller = self
            .motion_controllers
            .get(&controller_handle)
            .ok_or_else(|| native_state_error("motion controller handle is invalid"))?;
        let now = arguments
            .get(4..8)
            .map_or(0, |bytes| u32::from_le_bytes(bytes.try_into().unwrap()));
        let bytes = serialize_motion_snapshot(controller, now)?;
        let snapshot = self.motion_snapshots.get_mut(&handle).unwrap();
        if bytes.len() > previous_capacity {
            snapshot.storage.resize(bytes.len(), 0);
        }
        snapshot.storage[..bytes.len()].copy_from_slice(&bytes);
        snapshot.size = bytes.len();
        Ok(NativeReturn::Value(1))
    }

    fn cst_native(
        &mut self,
        index: u32,
        arguments: &[u8],
        vm: &mut Vm,
    ) -> cs2_script::Result<NativeReturn> {
        let handle = read_u32(arguments, 0)?;
        if handle == 0 {
            return Ok(match index {
                330 | 331 | 336 | 337 => NativeReturn::Continue,
                329 | 982 => NativeReturn::Value(u32::MAX),
                _ => NativeReturn::Value(0),
            });
        }
        let expected = match index {
            // Ordinary 332 queries by (context, name), with no value DWORD.
            // All three reference native-call TSVs record eight bytes.
            325 | 332 | 338 => 8,
            326 | 328 | 329 => 12,
            327 | 1037 | 336 | 337 => 16,
            1033 | 1038 => 20,
            330 | 331 | 608 | 982 => 4,
            333 => 16,
            334 | 335 => 20,
            _ => {
                return Err(native_state_error(
                    "CST native index is outside the supported table",
                ));
            }
        };
        if arguments.len() != expected {
            return Err(native_state_error(
                "CST native argument byte count is invalid",
            ));
        }
        if !self.cst_contexts.contains_key(&handle) {
            return Err(native_state_error("CST context handle is invalid"));
        }
        if index == 325 {
            return self.load_cst_context(handle, read_u32(arguments, 4)?, vm);
        }
        if index == 338 {
            let context = &self.cst_contexts[&handle];
            write_kcs_terminated(vm, read_u32(arguments, 4)?, &context.source_name, 1)?;
            return Ok(NativeReturn::Value(context.source_line));
        }
        if matches!(index, 327 | 1033 | 1037 | 1038) {
            return self.read_cst_record(index, arguments, vm);
        }
        if index == 982 {
            return Ok(NativeReturn::Value(
                self.cst_contexts[&handle].history.len().wrapping_sub(1) as u32,
            ));
        }
        if index == 608 {
            return Ok(NativeReturn::Value(
                self.cst_contexts[&handle].document.as_ref()
                    .map_or(u32::MAX, |document| document.inputs.len() as u32),
            ));
        }
        let context = self
            .cst_contexts
            .get_mut(&handle)
            .ok_or_else(|| native_state_error("CST context handle is invalid"))?;
        match index {
            326 => {
                let input = read_u32(arguments, 4)? as i32;
                let record = read_u32(arguments, 8)? as i32;
                let value = context.document.as_ref().map_or(u32::MAX, |document| {
                    if input < 0 {
                        document.inputs.len() as u32
                    } else if let Some(range) = document.inputs.get(input as usize) {
                        if record < 0 {
                            range.len() as u32
                        } else {
                            range
                                .start
                                .checked_add(record as usize)
                                .filter(|index| *index < range.end)
                                .and_then(|index| document.records.get(index))
                                .map_or(u32::MAX, |record| u32::from(record.kind))
                        }
                    } else {
                        u32::MAX
                    }
                });
                Ok(NativeReturn::Value(value))
            }
            328 | 329 | 332 => {
                let mut name = expand_kcs_format(vm, &vm.read_string(read_u32(arguments, 4)?)?, 0)?;
                normalize_cst_name(&mut name);
                if index != 332 && name.len() >= 32 {
                    return Err(native_state_error("CST registration name exceeds 31 bytes"));
                }
                if index == 332 {
                    let value = context
                        .registrations
                        .iter()
                        .find(|entry| cst_name_matches(&entry.name, &name))
                        .map_or(0, |entry| entry.value);
                    return Ok(NativeReturn::Value(value));
                }
                if name.is_empty() {
                    return Ok(NativeReturn::Value(if index == 328 { 1 } else { u32::MAX }));
                }
                let mut fixed_name = [0_u8; 32];
                fixed_name[..name.len()].copy_from_slice(&name);
                let value = read_u32(arguments, 8)?;
                if index == 328 {
                    if let Some(entry) = context
                        .registrations
                        .iter_mut()
                        .find(|entry| cst_name_matches(&entry.name, &name))
                    {
                        entry.value = entry.value.max(value);
                    } else {
                        context.registrations.push(CstRegistration {
                            name: fixed_name,
                            value,
                            attachments: Vec::new(),
                            data: Vec::new(),
                        });
                    }
                    Ok(NativeReturn::Value(1))
                } else {
                    if let Some(entry) = context
                        .history
                        .last_mut()
                        .filter(|entry| cst_name_matches(&entry.name, &name))
                    {
                        entry.value = entry.value.max(value);
                    } else {
                        context.history.push(CstRegistration {
                            name: fixed_name,
                            value,
                            attachments: Vec::new(),
                            data: Vec::new(),
                        });
                    }
                    Ok(NativeReturn::Value(context.history.len() as u32 - 1))
                }
            }
            330 => {
                context.registrations.clear();
                Ok(NativeReturn::Continue)
            }
            331 => {
                context.history.clear();
                Ok(NativeReturn::Continue)
            }
            333 => {
                let requested = read_u32(arguments, 4)? as usize;
                let selected = context
                    .history
                    .is_empty()
                    .then_some(0)
                    .or_else(|| Some(requested.min(context.history.len() - 1)));
                let (name, value) = selected
                    .and_then(|index| context.history.get(index))
                    .map_or((&[][..], u32::MAX), |entry| {
                        (trim_nul(&entry.name), entry.value)
                    });
                write_kcs_terminated(vm, read_u32(arguments, 8)?, name, 1)?;
                vm.write(read_u32(arguments, 12)?, &value.to_le_bytes())?;
                Ok(NativeReturn::Value(
                    selected
                        .filter(|index| context.history.get(*index).is_some())
                        .unwrap_or(u32::MAX as usize) as u32,
                ))
            }
            334 | 335 => {
                let requested = read_u32(arguments, 4)? as usize;
                let position = read_u32(arguments, 8)?;
                let id = read_u32(arguments, 12)?;
                let address = read_u32(arguments, 16)?;
                let selected = requested.min(context.history.len().saturating_sub(1));
                if index == 334 {
                    let value = vm.read_string(address)?;
                    let Some(entry) = context.history.get_mut(selected) else {
                        return Ok(NativeReturn::Value(0));
                    };
                    cst_detail_set(entry, position, id, &value);
                    Ok(NativeReturn::Value(1))
                } else {
                    let Some(entry) = context.history.get(selected) else {
                        return Ok(NativeReturn::Value(0));
                    };
                    if entry.attachments.is_empty() {
                        return Ok(NativeReturn::Value(0));
                    }
                    let value = cst_detail_get(entry, position, id)?;
                    write_kcs_terminated(vm, address, value.as_deref().unwrap_or_default(), 1)?;
                    Ok(NativeReturn::Value(u32::from(value.is_some())))
                }
            }
            336 | 337 => {
                let requested = read_u32(arguments, 4)? as usize;
                let position = read_u32(arguments, 8)?;
                let address = read_u32(arguments, 12)?;
                let input = vm.read_string(address)?;
                let mut output = Vec::new();
                let mut cursor = 0;
                while cursor < input.len() {
                    if let Some((consumed, id)) = cst_string_marker(&input[cursor..]) {
                        if index == 336 {
                            let selected = requested.min(context.history.len().saturating_sub(1));
                            if let (Some(entry), Some(value)) = (
                                context.history.get_mut(selected),
                                self.string_values.get(&id),
                            ) {
                                cst_detail_set(entry, position, id, value);
                            }
                        } else {
                            let selected = requested.min(context.history.len().saturating_sub(1));
                            if let Some(entry) = context.history.get(selected) {
                                if let Some(value) = cst_detail_get(entry, position, id)? {
                                    output.extend_from_slice(&value);
                                }
                            }
                        }
                        cursor += consumed;
                    } else {
                        if index == 337 {
                            output.push(input[cursor]);
                        }
                        cursor += 1;
                    }
                    if output.len() > 64 * 1024 * 1024 {
                        return Err(native_state_error("CST detail expansion exceeds 64 MiB"));
                    }
                }
                if index == 337 {
                    write_kcs_terminated(vm, address, &output, 2)?;
                }
                Ok(NativeReturn::Continue)
            }
            _ => unreachable!("CST index was filtered above"),
        }
    }

}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum PlayerFramePhase {
    #[default]
    Start,
    Input,
    Children,
    Root,
    Fes,
    Particles,
    Messages,
    Configuration,
    Render,
}

#[derive(Clone)]
struct PendingFesInput {
    code: u32,
    physical: Option<InputEvent>,
}

pub struct HeadlessPlayer {
    root: GameRoot,
    scan: GameScan,
    compatibility: OnceLock<CompatibilityReport>,
    current_task_id: u32,
    save_root: PathBuf,
    default_entry: Option<String>,
    frame_phase: PlayerFramePhase,
    particle_frame_cursor: Option<i32>,
    external_size: usize,
    configured_shared_slots: usize,
    shared_values: Vec<u32>,
    current_parameter: u32,
    menu_item_requests: Vec<(u32, u32)>,
    debug_mode: u32,
    sound_config_enabled: u32,
    auto_select: u32,
    string_values: BTreeMap<u32, Vec<u8>>,
    next_task_id: u32,
    task_parents: BTreeMap<u32, u32>,
    scene_nodes: BTreeMap<u32, SceneNode>,
    image_handles: BTreeMap<u32, RgbaFrame>,
    image_resources: BTreeMap<u32, DesktopImageResource>,
    backlogs: BTreeMap<u32, DesktopBacklog>,
    script_replacements: BTreeMap<Vec<u8>, Vec<u8>>,
    particles: BTreeMap<u32, ParticleObjectState>,
    record_stacks: BTreeMap<u32, DesktopRecordStack>,
    input_task: DesktopInputTask,
    task_vms: BTreeMap<u32, Vm>,
    system_config_blocks: [Vec<u8>; 4],
    css_handles: BTreeMap<u32, CssFile>,
    integer_maps: BTreeMap<u32, BTreeMap<Vec<u8>, u32>>,
    data_services: data_services::DataServices,
    cst_contexts: BTreeMap<u32, CstContextState>,
    motion_controllers: BTreeMap<u32, MotionControllerState>,
    motion_snapshots: BTreeMap<u32, MotionSnapshotState>,
    message_wait_address: Option<u32>,
    pending_messages: VecDeque<[u32; 4]>,
    pending_fes_events: VecDeque<PendingFesInput>,
    canvas: Canvas,
    graphics_canvas: Canvas,
    scene_compositor: scene_compositor::SceneCompositor,
    image_frame: Option<RgbaFrame>,
    cst_images: BTreeMap<(CstImageKind, i32), Vec<CstImageLayer>>,
    fes: Option<FesRuntime>,
    cst: Option<CstRuntime>,
    xml_handles: BTreeMap<u32, ConfigXml>,
    next_xml_handle: u32,
    state: RuntimeState,
    capture_script_evidence: bool,
    loaded: Option<LoadedEntry>,
    vm: Option<Vm>,
    audio_host: DesktopAudioHost,
    movie_host: MpegMovieHost,
    movie_playbacks: BTreeMap<u32, u64>,
    movie_resources: BTreeMap<u32, String>,
    sound_channels: BTreeMap<u32, SoundChannelState>,
    sound_groups: Vec<SoundGroup>,
    sound_config: DesktopSoundConfig,
    script_random: ScriptRandom,
    script_encoding: ScriptTextEncoding,
    font: Vec<cs2_renderer::FontRasterizer>,
    events: Vec<LogicEvent>,
    log: Vec<String>,
    particle_snapshot: ParticleSnapshot,
}

impl HeadlessPlayer {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_internal(path.as_ref(), None)
    }

    /// Use a caller-owned save directory for this player only. Android hosts
    /// can pass their private files directory without changing process-wide
    /// environment state or reading another player's settings first.
    pub fn open_with_save_root(
        path: impl AsRef<Path>, save_root: impl AsRef<Path>,
    ) -> Result<Self> {
        Self::open_internal(path.as_ref(), Some(save_root.as_ref().to_path_buf()))
    }

    fn open_internal(path: &Path, save_root: Option<PathBuf>) -> Result<Self> {
        let root = GameRoot::open(path).context("open game directory")?;
        let scan = root.scan().context("scan game directory")?;
        let (sound_groups, discovered_sound_config_enabled) =
            discover_sound_groups(&scan.resources);
        let sound_config = discover_sound_config(&scan.resources);
        let sound_config_enabled = if !sound_config.source_loaded {
            discovered_sound_config_enabled
        } else {
            1
        };
        let save_root = save_root.unwrap_or_else(|| save_root_for(&scan.engine.signature));
        let mut sound_config = sound_config;
        sound_config.load_overrides(&save_root);
        let audio_host = audio_host_for_resources(&scan.resources);
        let movie_host = MpegMovieHost::from_resources(&scan.resources);
        let font = load_font(&scan.resources);
        let [canvas_width, canvas_height] = discover_screen_size(&scan.resources)?;
        let canvas = Canvas::new(canvas_width, canvas_height)
            .map_err(|error| anyhow::anyhow!(error))
            .context("create game logical canvas")?;
        let graphics_canvas = Canvas::new(canvas_width, canvas_height)
            .map_err(|error| anyhow::anyhow!(error))
            .context("create persistent graphics canvas")?;
        let mut player = Self {
            root,
            scan,
            compatibility: OnceLock::new(),
            current_task_id: 0,
            save_root,
            default_entry: None,
            frame_phase: PlayerFramePhase::Start,
            particle_frame_cursor: None,
            external_size: 0,
            configured_shared_slots: 512,
            shared_values: Vec::new(),
            current_parameter: 0,
            menu_item_requests: Vec::new(),
            debug_mode: 0,
            sound_config_enabled,
            auto_select: 0,
            string_values: BTreeMap::new(),
            next_task_id: 1,
            task_parents: BTreeMap::new(),
            scene_nodes: BTreeMap::new(),
            image_handles: BTreeMap::new(),
            image_resources: BTreeMap::new(),
            backlogs: BTreeMap::new(),
            script_replacements: BTreeMap::new(),
            particles: BTreeMap::new(),
            record_stacks: BTreeMap::new(),
            input_task: DesktopInputTask::default(),
            task_vms: BTreeMap::new(),
            system_config_blocks: [vec![0; 5628], vec![0; 72], vec![0; 32], vec![0; 12]],
            css_handles: BTreeMap::new(),
            integer_maps: BTreeMap::new(),
            data_services: data_services::DataServices::default(),
            cst_contexts: BTreeMap::new(),
            motion_controllers: BTreeMap::new(),
            motion_snapshots: BTreeMap::new(),
            message_wait_address: None,
            pending_messages: VecDeque::new(),
            pending_fes_events: VecDeque::new(),
            canvas,
            graphics_canvas,
            scene_compositor: scene_compositor::SceneCompositor::default(),
            image_frame: None,
            cst_images: BTreeMap::new(),
            fes: None,
            cst: None,
            xml_handles: BTreeMap::new(),
            next_xml_handle: 1,
            state: RuntimeState::default(),
            capture_script_evidence: true,
            loaded: None,
            vm: None,
            audio_host,
            movie_host,
            movie_playbacks: BTreeMap::new(),
            movie_resources: BTreeMap::new(),
            sound_channels: BTreeMap::new(),
            sound_groups,
            sound_config,
            script_random: ScriptRandom::default(),
            script_encoding: ScriptTextEncoding::default(),
            font,
            events: Vec::new(),
            log: Vec::new(),
            particle_snapshot: ParticleSnapshot::default(),
        };
        player.log.push(format!(
            "opened {}: {} files, {} archives",
            player.root.path.display(),
            player.scan.resources.files().len(),
            player.scan.resources.archives().len()
        ));
        player.log.push(format!(
            "message font: {}",
            if !player.font.is_empty() {
                "loaded"
            } else {
                "unavailable"
            }
        ));
        player.log.push(format!("logical canvas: {canvas_width}x{canvas_height}"));
        for diagnostic in &player.scan.diagnostics {
            player.log.push(format_diagnostic(diagnostic));
        }
        player.default_entry = discover_start_entry(&player.scan.resources);
        player.external_size = discover_heap_size(&player.scan.resources);
        player.configured_shared_slots =
            fes_tasks::startup_shared_slots(&player.scan, player.default_entry.as_deref());
        player.auto_select = discover_auto_select(&player.scan.resources);
        player.data_services.native_profile = player.scan.engine.primary_executable.as_ref()
            .and_then(|binary| cs2_runtime::native_profile::profile_for_pe_sha256(&binary.sha256, None));
        if let Some(binary) = player.scan.engine.primary_executable.as_ref() {
            player.data_services.tasks.message_lists.identify(&binary.sha256);
            player.data_services.ordinary.identify(&binary.sha256);
            player.data_services.css_task_pe_sha256 = css_task_native::identified_pe_sha256(&binary.sha256);
            player.data_services.particle_external.identify(&binary.sha256);
            player.data_services.emote.initialize(&player.scan.resources, &binary.sha256)
                .context("initialize identified E-mote CPU backend")?;
        }
        player.data_services.tasks.fes.install_cpu_hooks();
        player.data_services.tasks.install_buffer_receiver(15, list_native::receive);
        player.data_services.tasks.install_buffer_receiver(54, emote_native::receive);
        player.data_services.tasks.install_buffer_receiver(61, effekseer_native::receive);
        player.data_services.tasks.fes.platform.extra_effect = Some(fes_services::apply_effect);
        player.data_services.tasks.fes.platform.finish_extension = Some(fes_services::finish);
        player.data_services.tasks.fes.platform.release_extension = Some(fes_services::release);
        player.initialize_particle_message_profile().context("initialize identified particle protocol")?;
        player.initialize_languages().context("initialize language resources")?;
        player.initialize_local_achievements().context("initialize local achievements")?;
        // Platform events enter in explicit game-logical coordinates. Their
        // viewport transform is performed by the window/touch host first.
        player.data_services.host_input_coordinates = Some(cs2_runtime::input_coordinates::InputCoordinateMap {
            logical_size: [canvas_width as i32, canvas_height as i32],
            ..Default::default()
        });
        player.install_default_language_adapters();
        player.set_fes_button_adapters(
            Some(fes_task_platform::button_target_lookup),
            Some(fes_tasks::fes_button_media_hook),
            Some(fes_tasks::fes_button_named_hook),
        );
        player.set_fes_button_record_lookup(Some(fes_task_platform::button_record_lookup));
        player.set_button_binding_lookup(Some(fes_task_platform::button_binding_lookup));
        player.set_root_task_number_lookup(Some(task_wrapper_native::root_task_number_lookup));
        player.set_kx_plane_view_hook(Some(plane_native::kx_plane_view));
        player.set_kx_plane_install_hook(Some(plane_native::install_kx_plane));
        player.initialize_message_timing().context("initialize message settings")?;
        player.apply_media_settings().context("initialize movie settings")?;
        if let Some(entry) = &player.default_entry {
            player.log.push(format!("startup entry: {entry}"));
        }
        player
            .log
            .push(format!("startup heap: {} bytes", player.external_size));
        player.with_host(|host| host.ensure_fes_settings())?;
        player.initialize_configuration_host()?;
        player.with_host(|host| Ok(host.initialize_root_sound_service()?))?;
        player.initialize_enabled_speech().context("initialize enabled speech provider")?;
        Ok(player)
    }

    pub fn root(&self) -> &Path {
        &self.root.path
    }
    pub fn save_root(&self) -> &Path {
        &self.save_root
    }
    pub fn scan(&self) -> &GameScan {
        &self.scan
    }
    pub fn resources(&self) -> &ResourceIndex {
        &self.scan.resources
    }
    pub fn state(&self) -> &RuntimeState {
        &self.state
    }
    pub fn logical_size(&self) -> [u32; 2] {
        [self.canvas.width(), self.canvas.height()]
    }
    pub fn events(&self) -> &[LogicEvent] {
        &self.events
    }
    pub fn log(&self) -> &[String] {
        &self.log
    }

    pub fn register_task_vm(&mut self, task_id: u32, vm: Vm) -> Result<()> {
        if task_id == 0 || task_id == self.current_task_id {
            bail!("a registered task VM needs a nonzero, non-current task id")
        }
        if !self.task_parents.contains_key(&task_id) && !self.scene_nodes.contains_key(&task_id) {
            bail!("task {task_id} has no live task record")
        }
        if self.task_vms.contains_key(&task_id) {
            bail!("task {task_id} already owns a VM")
        }
        self.task_vms.insert(task_id, vm);
        Ok(())
    }

    pub fn unregister_task_vm(&mut self, task_id: u32) -> Option<Vm> {
        self.task_vms.remove(&task_id)
    }

    pub fn set_input_key_masks(&mut self, key: usize, groups: [u32; 4]) -> Result<()> {
        let Some(target) = self.input_task.key_masks.get_mut(key) else {
            bail!("input key slot must be in 0..79")
        };
        *target = groups;
        Ok(())
    }

    pub fn register_input_region(&mut self, words: [u32; 15]) -> Result<()> {
        if self.input_task.regions.len() >= 1_000_000 {
            bail!("input region count exceeds the runtime limit")
        }
        let mut words = words;
        words[1] = words[1] as i32 as u32;
        words[7] = 0;
        words[8] = 1;
        words[9] = 1;
        words[11..14].fill(0);
        words[14] = 1;
        self.input_task.regions.push(InputRegionState { words });
        self.input_task
            .regions
            .sort_by_key(|region| std::cmp::Reverse(region.words[1] as i32));
        Ok(())
    }

    pub fn input_region_at(&self, point: [i32; 2]) -> Option<[u32; 15]> {
        self.input_task
            .regions
            .iter()
            .find(|region| region.words[8] != 0 && region.contains(point))
            .map(|region| region.words)
    }

    pub fn set_particle_points(&mut self, task_id: u32, points: Vec<ParticlePoint>) -> Result<()> {
        let particle = self
            .particles
            .get_mut(&task_id)
            .ok_or_else(|| anyhow::anyhow!("particle task {task_id} is not registered"))?;
        particle.points = points;
        Ok(())
    }

    pub fn image_resource(&self, handle: u32) -> Option<&RgbaFrame> {
        self.image_resources
            .get(&handle)
            .map(|resource| &resource.image)
    }

    pub fn retain_image_resource(&mut self, handle: u32) -> Result<()> {
        let resource = self
            .image_resources
            .get_mut(&handle)
            .ok_or_else(|| anyhow::anyhow!("image resource handle {handle} is invalid"))?;
        resource.references = resource
            .references
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("image resource reference count overflow"))?;
        Ok(())
    }

    pub fn release_image_resource_reference(&mut self, handle: u32) -> Result<()> {
        if handle == 0 {
            return Ok(());
        }
        let resource = self
            .image_resources
            .get_mut(&handle)
            .ok_or_else(|| anyhow::anyhow!("image resource handle {handle} is invalid"))?;
        resource.references = resource.references.saturating_sub(1);
        if resource.references == 0 {
            self.image_resources.remove(&handle);
            self.data_services.image_manager.release_resource_layers(handle);
        }
        Ok(())
    }

    pub fn image_resource_updated(&mut self, handle: u32, clear: bool) -> Result<bool> {
        let resource = self
            .image_resources
            .get_mut(&handle)
            .ok_or_else(|| anyhow::anyhow!("image resource handle {handle} is invalid"))?;
        let updated = resource.updated;
        if clear {
            resource.updated = false;
        }
        Ok(updated)
    }

    pub fn default_entry(&self) -> Option<&str> {
        self.default_entry.as_deref()
    }

    /// Enter the configured game program. Opening a directory only indexes
    /// resources; it must not manufacture a title screen or a successful run.
    pub fn load_startup(&mut self) -> Result<&LoadedEntry> {
        let entry = self.default_entry.clone().context(
            "no startup program: expected SCRIPT/start in startup.xml or a main/start KCS resource",
        )?;
        self.load_entry(&entry)
            .with_context(|| format!("load configured startup program `{entry}`"))
    }

    pub fn set_script_encoding(&mut self, encoding: ScriptTextEncoding) {
        self.script_encoding = encoding;
    }

    pub fn frame_snapshot(&self) -> FrameSnapshot {
        self.canvas.snapshot()
    }

    pub fn compatibility_report(&self) -> CompatibilityReport {
        self.compatibility
            .get_or_init(|| CompatibilityReport::from_scan(&self.scan))
            .clone()
    }

    pub fn queue_message(&mut self, message: [u32; 4]) {
        self.pending_messages.push_back(message);
    }

    pub fn queue_fes_event(&mut self, event: u32) {
        self.pending_fes_events.push_back(PendingFesInput { code: event, physical: None });
    }

    pub(crate) fn queue_host_fes_event(&mut self, code: u32, physical: &InputEvent) {
        self.pending_fes_events.push_back(PendingFesInput {
            code, physical: Some(physical.clone()),
        });
    }

    pub fn load_entry(&mut self, name: impl AsRef<str>) -> Result<&LoadedEntry> {
        let name = name.as_ref();
        let bytes = read_entry_with_fallback(&self.scan.resources, name)
            .with_context(|| format!("read script/resource entry {name}"))?;
        let loaded = if bytes.starts_with(b"KCS\0") {
            let program = KcsProgram::parse(&bytes).context("parse KCS entry")?;
            self.retire_direct_fes_for_entry()?;
            let mut vm = Vm::new();
            vm.load_program(program.clone(), vec![0; self.external_size])
                .context("create KCS VM")?;
            self.data_services.tasks.set_factory_external(vm.external_binding());
            self.data_services.tasks.register_external_task(
                self.current_task_id, 0, &self.data_services.language.languages,
            ).context("register root script task")?;
            self.vm = Some(vm);
            self.cst = None;
            self.state.scene.current_script = Some(name.to_string());
            self.state.script.current_script = Some(name.to_string());
            self.events.push(LogicEvent {
                frame: self.state.clock.frame,
                kind: "scene-entry".to_string(),
                detail: format!("KCS {} instructions", program.instructions.len()),
            });
            LoadedEntry {
                name: name.to_string(),
                kind: "KCS".to_string(),
                byte_length: bytes.len(),
                detail: format!(
                    "code={} data={} stack={}",
                    program.code.len(),
                    program.data.len(),
                    program.stack_size
                ),
            }
        } else if bytes.starts_with(b"CatScene") {
            let document = CstDocument::parse(&bytes).context("parse CST entry")?;
            self.retire_direct_fes_for_entry()?;
            self.vm = None;
            self.fes = None;
            self.cst = Some(CstRuntime::new(
                document.clone(),
                name.to_string(),
                self.script_encoding,
                load_cstl(&self.scan.resources, name),
            ));
            self.state.scene.current_script = Some(name.to_string());
            self.events.push(LogicEvent {
                frame: self.state.clock.frame,
                kind: "scene-entry".to_string(),
                detail: format!(
                    "CST {} records, {} inputs",
                    document.records.len(),
                    document.inputs.len()
                ),
            });
            LoadedEntry {
                name: name.to_string(),
                kind: "CST".to_string(),
                byte_length: bytes.len(),
                detail: format!(
                    "records={} inputs={}",
                    document.records.len(),
                    document.inputs.len()
                ),
            }
        } else if bytes.starts_with(b"FES\0") {
            let document = FesDocument::parse(&bytes).context("parse FES entry")?;
            let sections = document.sections.len();
            let lines = document.sections.iter().map(|section| section.lines.len()).sum::<usize>();
            if self.frame_phase != PlayerFramePhase::Start {
                bail!("finish the current logical frame before installing a direct FES entry");
            }
            self.retire_direct_fes_for_entry()?;
            let root = self.current_task_id;
            let external = cs2_script::SharedExternalStorage::new(vec![0; self.external_size])?;
            self.with_host(|host| Ok(host.begin_direct_fes_entry(root, document, external)?))?;
            self.fes = None;
            self.vm = None;
            self.cst = None;
            self.image_frame = None;
            self.message_wait_address = None;
            self.pending_messages.clear();
            self.pending_fes_events.clear();
            self.state.scene.current_script = Some(name.to_string());
            self.state.script = ScriptState::default();
            self.state.script.current_script = Some(name.to_string());
            self.events.push(LogicEvent {
                frame: self.state.clock.frame,
                kind: "fes-entry".to_string(),
                detail: format!("FES {sections} sections; full type-34 task driver"),
            });
            LoadedEntry {
                name: name.to_string(),
                kind: "FES".to_string(),
                byte_length: bytes.len(),
                detail: format!(
                    "sections={} lines={}",
                    sections,
                    lines
                ),
            }
        } else if bytes.starts_with(b"HG-3") || bytes.starts_with(b"HG-2") {
            let file = HgFile::parse(&bytes).context("parse HG image entry")?;
            let frame = file
                .frames
                .first()
                .ok_or_else(|| anyhow::anyhow!("HG image contains no frames"))?;
            let pixels = frame
                .decode_pixels_rgba8(&bytes)
                .map_err(|error| anyhow::anyhow!(error))?;
            let _frame = RgbaFrame::from_rgba8(frame.width, frame.height, pixels)
                .map_err(|error| anyhow::anyhow!(error))?;
            self.retire_direct_fes_for_entry()?;
            self.image_frame = Some(_frame.clone());
            self.events.push(LogicEvent {
                frame: self.state.clock.frame,
                kind: "image-frame".to_string(),
                detail: format!(
                    "{}x{} {} frame(s)",
                    frame.width,
                    frame.height,
                    file.frames.len()
                ),
            });
            LoadedEntry {
                name: name.to_string(),
                kind: "HG".to_string(),
                byte_length: bytes.len(),
                detail: format!(
                    "frames={} first={}x{} bit_depth={}",
                    file.frames.len(),
                    frame.width,
                    frame.height,
                    frame.bit_depth
                ),
            }
        } else {
            bail!("entry {name} is not a supported KCS/CST/FES document")
        };
        self.log.push(format!(
            "loaded {} {} ({} bytes): {}",
            loaded.kind, loaded.name, loaded.byte_length, loaded.detail
        ));
        self.frame_phase = PlayerFramePhase::Start;
        self.particle_frame_cursor = None;
        self.loaded = Some(loaded);
        Ok(self
            .loaded
            .as_ref()
            .expect("loaded entry was just assigned"))
    }

    pub fn run_frames(&mut self, frames: u64, instruction_budget: usize) -> Result<()> {
        if instruction_budget == 0 {
            bail!("instruction budget must be greater than zero")
        }
        if self.data_services.host_pause.paused {
            // Keep an interrupted frame/VM continuation and the game's pause
            // state intact. Background audio-device failures still propagate.
            self.audio_host.poll().context("poll host-paused audio")?;
            return Ok(());
        }
        let mut completed_frames = 0;
        for _ in 0..frames {
            // A graph native yields with its VM/task/frame continuation intact.
            // All LoggingHost and VM borrows have returned at this safe point.
            self.service_object_graph_request().context("service object graph transaction")?;
            if self.frame_phase == PlayerFramePhase::Start {
            self.apply_pending_host_input().context("consume queued platform input")?;
            self.particle_frame_cursor = None;
            self.state.clock.frame = self.state.clock.frame.saturating_add(1);
            if self.data_services.top_clock.sampled == 0 {
                self.state.clock.elapsed_ticks = self.state.clock.elapsed_ticks.saturating_add(1);
            }
            let frame = self.state.clock.frame;
            self.begin_host_device_keys_frame().context("sample configured platform keys")?;
            self.advance_audio_repeats()?;
            self.events.push(LogicEvent {
                frame,
                kind: "logic-frame".into(),
                detail: "headless tick".into(),
            });
            self.frame_phase = PlayerFramePhase::Input;
            }
            if self.frame_phase == PlayerFramePhase::Input {
            let _timing = measure_host_phase(HostPhase::ScriptInput);
            if !self.advance_host_device_keys_scan(instruction_budget)
                .context("scan configured keys and finish input callbacks")?
            { continue; }
            let frame = self.state.clock.frame;
            self.dispatch_pending_task_input().context("route input to FES tasks")?;
            if let Some(cst) = self.cst.as_mut() {
                if cst.waiting {
                    if let Some(message) = self.pending_messages.pop_front() {
                        if cst.choices.is_some() {
                            cst.pending_choice = Some(message[1] as usize);
                        }
                        cst.resume();
                        self.events.push(LogicEvent {
                            frame,
                            kind: "cst-input".into(),
                            detail: format!("input={} message={message:?}", cst.input),
                        });
                    }
                }
                cst.advance(
                    frame,
                    &self.scan.resources,
                    &mut self.shared_values,
                    &mut self.cst_images,
                    &mut self.events,
                )?;
            }
            self.frame_phase = PlayerFramePhase::Children;
            }
            let frame = self.state.clock.frame;
            let waiting_for_input = self.message_wait_address.is_some();
            if waiting_for_input {
                self.events.push(LogicEvent {
                    frame,
                    kind: "input-wait".into(),
                    detail: format!(
                        "native 71 address={:#x}",
                        self.message_wait_address.unwrap()
                    ),
                });
            }
            if matches!(self.frame_phase, PlayerFramePhase::Children | PlayerFramePhase::Root) {
            let _timing = measure_host_phase(HostPhase::ScriptRoot);
            if self.vm.is_none() && self.data_services.tasks.fes.is_direct_entry(self.current_task_id)
                && self.frame_phase == PlayerFramePhase::Children
            {
                let root = self.current_task_id;
                let now = self.state.clock.elapsed_ticks as u32;
                if !self.with_host(|host| Ok(host.update_ordinary_children(root, now, 1, instruction_budget)?))? {
                    continue;
                }
                self.frame_phase = PlayerFramePhase::Fes;
            }
            if let Some(vm) = self.vm.as_mut() {
                let mut message = self.message_wait_address
                    .and_then(|_| self.pending_messages.front().copied());
                let mut consumed_message = false;
                let mut host = LoggingHost {
                    events: &mut self.events,
                    frame,
                    resources: &self.scan.resources,
                    xml_handles: &mut self.xml_handles,
                    next_xml_handle: &mut self.next_xml_handle,
                    configured_shared_slots: self.configured_shared_slots,
                    text_encoding: self.script_encoding,
                    shared_values: &mut self.shared_values,
                    current_parameter: &mut self.current_parameter,
                    current_task_id: self.current_task_id,
                    pointer_position: [self.state.input.cursor_x, self.state.input.cursor_y],
                    task_time: self.state.clock.elapsed_ticks as u32,
                    task_origin: 0,
                    auto_select: self.auto_select,
                    scene_animation_enabled: 1,
                    fullscreen: 0,
                    fullscreen_mw_pos: 0,
                    string_values: &mut self.string_values,
                    next_task_id: &mut self.next_task_id,
                    task_parents: &mut self.task_parents,
                    scene_nodes: &mut self.scene_nodes,
                    image_handles: &mut self.image_handles,
                    image_resources: &mut self.image_resources,
                    backlogs: &mut self.backlogs,
                    canvas: Some(&mut self.graphics_canvas),
                    back_buffer: Some(&self.canvas),
                    particles: &mut self.particles,
                    record_stacks: &mut self.record_stacks,
                    input_task: &mut self.input_task,
                    task_vms: &mut self.task_vms,
                    script_replacements: &self.script_replacements,
                    system_config_blocks: &mut self.system_config_blocks,
                    css_handles: &mut self.css_handles,
                    integer_maps: &mut self.integer_maps,
                    data_services: &mut self.data_services,
                    cst_contexts: &mut self.cst_contexts,
                    motion_controllers: &mut self.motion_controllers,
                    motion_snapshots: &mut self.motion_snapshots,
                    save_root: &self.save_root,
                    message_wait_address: &mut self.message_wait_address,
                    audio_host: &mut self.audio_host,
                    movie_host: Some(&mut self.movie_host),
                    movie_playbacks: &mut self.movie_playbacks,
                    movie_resources: &mut self.movie_resources,
                    sound_channels: &mut self.sound_channels,
                    sound_groups: &self.sound_groups,
                    sound_config: &self.sound_config,
                    script_random: &mut self.script_random,
                    menu_item_requests: &mut self.menu_item_requests,
                    debug_mode: self.debug_mode,
                    sound_config_enabled: self.sound_config_enabled,
                };
                let execution = (|| -> cs2_script::Result<Option<cs2_script::ResumeStep>> {
                    // Child clocks update before child bodies, then the root
                    // body resumes. Continuations reuse this frame's time.
                    if self.frame_phase == PlayerFramePhase::Children {
                        if let Some((target, mut packet)) = host.data_services.tasks.pending_root_dispatch() {
                            if target != host.current_task_id {
                                return Err(native_state_error("pending root message targets another external task"));
                            }
                            let original = packet;
                            match host.dispatch_external_task_message(vm, &mut packet, instruction_budget)? {
                                Some(cs2_script::ResumeStep::Yield) => return Ok(None),
                                step => host.data_services.tasks.complete_root_dispatch_step(target, original, packet, step)?,
                            }
                        }
                        let root = host.current_task_id;
                        let now = host.task_time;
                        if !host.update_ordinary_children(root, now, 1, instruction_budget)? {
                            return Ok(None);
                        }
                        self.frame_phase = PlayerFramePhase::Root;
                    }
                    self.pending_messages.extend(
                        host.drain_root_task_messages_with_vm(host.current_task_id, vm)?,
                    );
                    if message.is_none() && host.message_wait_address.is_some() {
                        message = self.pending_messages.front().copied();
                    }
                    if let Some(packet) = message.as_mut() {
                        if let Some(result) = host.dispatch_external_task_message(vm, packet, instruction_budget)? {
                            consumed_message = result != cs2_script::ResumeStep::Yield;
                            return Ok(Some(result));
                        }
                        consumed_message = true;
                    }
                    if vm.execution_mode() != 0 {
                        *host.message_wait_address = None;
                    }
                    host.resume_external_task(vm, instruction_budget).map(Some)
                })();
                if execution.is_ok() && self.data_services.tasks.object_graph_request_pending() {
                    // Retry is a native event, not an instruction-budget
                    // result. Freeze this exact Root stage before consuming
                    // messages, advancing media or completing the frame.
                    continue;
                }
                if matches!(&execution, Ok(None)) {
                    self.events.push(LogicEvent {
                        frame, kind: "task-budget-yield".into(),
                        detail: format!("budget={instruction_budget}; retain frame time"),
                    });
                    continue;
                }
                let root_budget_yield = matches!(&execution, Ok(Some(cs2_script::ResumeStep::Yield)));
                if consumed_message {
                    self.pending_messages.pop_front();
                    if let Some(packet) = message {
                        self.events.push(LogicEvent { frame, kind: "message-resume".into(),
                            detail: format!("message={packet:?}") });
                        if self.pending_fes_events.is_empty()
                            && let Some(fes) = self.fes.as_mut()
                        {
                            fes.record_input(packet[0], Some(Point::new(
                                self.state.input.cursor_x, self.state.input.cursor_y)));
                        }
                    }
                }
                if !vm.waiting_for_message() && !vm.has_pending_execution() {
                    self.message_wait_address = None;
                }
                let result = match execution {
                    Ok(Some(cs2_script::ResumeStep::Yield | cs2_script::ResumeStep::Paused)) => Step::Yield,
                    Ok(Some(cs2_script::ResumeStep::Completed { event, .. })) =>
                        event.map_or(Step::Halted, Step::Event),
                    Ok(None) => unreachable!("child budget yield handled before root dispatch"),
                    Err(error) => {
                        let instruction = vm
                            .program()
                            .and_then(|program| program.instruction_at(vm.instruction_pointer))
                            .map(|instruction| format!("opcode={:#04x}", instruction.opcode))
                            .unwrap_or_else(|| "opcode=unknown".to_string());
                        let previous = vm.program().and_then(|program| {
                            program
                                .instructions
                                .iter()
                                .rev()
                                .find(|instruction| instruction.offset < vm.instruction_pointer)
                        });
                        self.events.push(LogicEvent {
                            frame,
                            kind: "vm-error".into(),
                            detail: format!(
                                "pc={:#x} {instruction} prev={:?} sp={} base={} top={} frames={} {error}",
                                vm.instruction_pointer,
                                previous.map(|instruction| {
                                    (instruction.offset, instruction.opcode, instruction.operands.len())
                                }),
                                vm.stack_pointer(),
                                vm.frame_base(),
                                vm.frame_top(),
                                vm.frames().len()
                            ),
                        });
                        self.log.push(format!("KCS stopped: {error}"));
                        return Err(anyhow::anyhow!(error).context("execute KCS game task"));
                    }
                };
                match result {
                    Step::Yield => self.events.push(LogicEvent {
                        frame,
                        kind: "vm-yield".into(),
                        detail: format!("budget={instruction_budget}"),
                    }),
                    Step::Halted => self.events.push(LogicEvent {
                        frame,
                        kind: "vm-halted".into(),
                        detail: format!("pc={}", vm.instruction_pointer),
                    }),
                    Step::Event(event) => self.events.push(LogicEvent {
                        frame,
                        kind: "vm-event".into(),
                        detail: format!(
                            "native_index={} status={}",
                            event.native_index, event.status
                        ),
                    }),
                }
                if self.capture_script_evidence {
                    self.state.script = ScriptState::from_vm(vm);
                    self.state.script.current_script = self.state.scene.current_script.clone();
                }
                if root_budget_yield {
                    continue;
                }
            }
            self.frame_phase = PlayerFramePhase::Fes;
            }
            if self.frame_phase == PlayerFramePhase::Fes {
            let _timing = measure_host_phase(HostPhase::ScriptFes);
            if self.data_services.tasks.fes.is_direct_entry(self.current_task_id) {
                let root = self.current_task_id;
                let now = self.state.clock.elapsed_ticks as u32;
                if !self.with_host(|host| Ok(host.advance_direct_fes_entry(root, now, instruction_budget)?))? {
                    continue;
                }
            } else if self.fes.is_some() {
                bail!("legacy FES entry is not installed in the full task driver");
            }
            self.frame_phase = PlayerFramePhase::Particles;
            }
            if self.frame_phase == PlayerFramePhase::Particles {
            if !self.update_particles(self.state.clock.elapsed_ticks, instruction_budget)? {
                continue;
            }
            for released in self.data_services.tasks.take_released_task_ids() {
                self.data_services.messages.release(released);
                self.data_services.message_device.release(released as i32);
                self.release_plane_state(released);
                let wrappers = self.data_services.image_wrapper_owners.iter()
                    .filter_map(|(&handle, &owner)| (owner == released).then_some(handle))
                    .collect::<Vec<_>>();
                for handle in wrappers {
                    self.image_handles.remove(&handle);
                    self.data_services.image_wrapper_owners.remove(&handle);
                    self.data_services.release_image_pixel_identity(handle);
                }
            }
            self.frame_phase = PlayerFramePhase::Messages;
            }
            if self.frame_phase == PlayerFramePhase::Messages {
            self.advance_message_objects().context("update message objects")?;
            self.frame_phase = PlayerFramePhase::Configuration;
            }
            if self.frame_phase == PlayerFramePhase::Configuration {
            {
                let _timing = measure_host_phase(HostPhase::Script);
                if !self.service_pending_root_dispatch(instruction_budget)? {
                    continue;
                }
            }
            if !self.finish_language_frame().context("refresh language and configuration")? {
                continue;
            }
            self.frame_phase = PlayerFramePhase::Render;
            }
            self.render_scene().context("render logical scene")?;
            self.capture_completed_frame();
            self.commit_host_device_keys_frame();
            self.data_services.top_clock.sampled = self.data_services.top_clock.requested;
            self.frame_phase = PlayerFramePhase::Start;
            completed_frames += 1;
        }
        self.log.push(format!("ran {completed_frames} logical frame(s)"));
        Ok(())
    }

    fn advance_audio_repeats(&mut self) -> Result<()> {
        self.advance_sound_fades()?;
        let channels = self.sound_channels.keys().copied().collect::<Vec<_>>();
        for channel in channels {
            let Some(mut state) = self.sound_channels.remove(&channel) else {
                continue;
            };
            let active = match state.handle {
                Some(handle) => match self.audio_host.is_active(handle) {
                    Ok(active) => active,
                    Err(error) => {
                        self.sound_channels.insert(channel, state);
                        return Err(error.into());
                    }
                },
                None => false,
            };
            if !active {
                state.handle = None;
                state.paused = false;
                if state.repeat_remaining > 0 {
                    if let Some(resource) = state.resource.clone() {
                        match audio::play_loaded_audio(&mut self.audio_host, AudioRequest {
                            channel: sound_audio_channel(&resource),
                            resource,
                            volume: state.request_volume(),
                            pan: state.request_pan(),
                            looped: false,
                        }, state.prepared_audio.clone(), 0) {
                            Ok(playback) => {
                                state.handle = Some(playback.handle);
                                state.paused = false;
                                state.repeat_remaining -= 1;
                            }
                            Err(error) => {
                                self.sound_channels.insert(channel, state);
                                return Err(error.into());
                            }
                        }
                    } else {
                        state.repeat_remaining = 0;
                    }
                }
            }
            self.sound_channels.insert(channel, state);
        }
        Ok(())
    }

    fn dispatch_pending_task_input(&mut self) -> Result<()> {
        self.flush_control_inputs()?;
        self.synchronize_message_input_geometry();
        let point = Point::new(self.state.input.cursor_x, self.state.input.cursor_y);
        self.input_task.pointer_position = [point.x, point.y];
        if !self.host_pointer_motion_routed() {
            self.queue_control_pointer_motion(point)?;
        }
        let selected = self.with_host(|host| {
            Ok(host.input_task.regions.iter()
                .find(|region| region.words[8] != 0 && region.contains([point.x, point.y])
                    && host.host_input_region_ready(region.id()))
                .map(|region| region.id()))
        })?;
        if !self.host_input_selection_routed() && selected != self.input_task.selected_region {
            if let Some(previous) = self.input_task.selected_region {
                if !self.queue_control_region_focus(previous, false)? {
                    self.data_services.tasks.fes.queue_region_focus(previous, false)?;
                }
            }
            if let Some(current) = selected {
                if !self.queue_control_region_focus(current, true)? {
                    self.data_services.tasks.fes.queue_region_focus(current, true)?;
                }
            }
            self.synchronize_plane_pointer_selection(selected);
            self.input_task.selected_region = selected;
        }
        let count = self.pending_fes_events.len();
        for _ in 0..count {
            let Some(pending) = self.pending_fes_events.front().cloned() else { break };
            if pending.physical.as_ref().is_some_and(|event| {
                self.host_input_fallback_disposition(event)
                    != cs2_runtime::message_object::InputFallbackDisposition::Unrouted
            }) {
                self.pending_fes_events.pop_front();
                continue;
            }
            let event = pending.code;
            let target = if event == 4 {
                self.data_services.fes_controls.captured_region().or(selected)
            } else { selected };
            let routed = match target {
                Some(region) if self.queue_control_region_input(region, event, point)? => true,
                Some(region) => self.data_services.tasks.fes.queue_region_input(region, event, Some(point))?,
                None => false,
            };
            self.pending_fes_events.pop_front();
            if !routed && self.fes.is_some() {
                self.pending_fes_events.push_back(pending);
            }
        }
        self.flush_control_inputs()?;
        Ok(())
    }

    fn apply_media_settings(&mut self) -> Result<()> {
        let path = self.save_root.join("setup.xml");
        let disabled = match fs::read(&path) {
            Ok(bytes) => ConfigXml::parse_runtime(&bytes)
                .with_context(|| format!("parse movie settings {}", path.display()))?
                .integer(&[b"document", b"ETC", b"disablemovie"], 0) != 0,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
        };
        self.movie_host.set_enabled(!disabled);
        Ok(())
    }

    /// Configuration hosts apply an already parsed user document. A missing
    /// disablemovie field resets the flag according to the platform contract.
    pub fn apply_platform_configuration(&mut self, document: &ConfigXml) -> Result<()> {
        let disabled = xml_integer(document, "document/ETC/disablemovie").unwrap_or(0) != 0;
        self.movie_host.set_enabled(!disabled);
        self.apply_message_settings_xml(document)?;
        self.with_host(|host| Ok(host.apply_legacy_sound_settings_xml(document)?))?;
        self.with_host(|host| host.apply_fes_user_document(document))?;
        self.data_services.configuration.apply_user(document);
        self.data_services.language.apply_user_config_xml(document);
        self.initialize_enabled_speech().context("apply speech provider configuration")?;
        Ok(())
    }

    /// Equality token for the latest completed, provably static composition.
    /// `None` requires normal presentation. Surface exposure still needs a redraw.
    pub fn stable_frame_revision(&self) -> Option<u64> {
        self.scene_compositor.revision
    }

    fn render_scene(&mut self) -> Result<()> {
        let _timing = measure_host_phase(HostPhase::Compose);
        let previous_revision = self.scene_compositor.revision.take();
        let mut graphics_snapshot = None;
        let stale_movies = self
            .movie_host
            .begin_movie_plane_frame(&self.scene_nodes)
            .context("prepare scene movie planes")?;
        for handle in stale_movies {
            self.movie_playbacks.remove(&handle);
            self.movie_resources.remove(&handle);
        }
        if self.data_services.tasks.render.has_composition() {
            self.scene_compositor.clear_prefix();
            if self.image_frame.is_some() || self.fes.is_some() || self.cst.is_some() || !self.cst_images.is_empty() {
                bail!("render effect needs the legacy scene's actual drawable depth bindings");
            }
            let mut output = Canvas::new(self.canvas.width(), self.canvas.height())?;
            output.clear(Color::rgba(0, 0, 0, 255));
            let mut effects = std::mem::take(&mut self.data_services.tasks.render);
            let result = effects.compose(&mut output, &mut |target, lower, upper| {
                std::mem::swap(&mut self.canvas, target);
                let result = self.render_scene_depth_range(lower, upper, false, &mut graphics_snapshot);
                std::mem::swap(&mut self.canvas, target);
                result
            });
            self.data_services.tasks.render = effects;
            result.context("compose render-task feedback and scene depth ranges")?;
            self.canvas = output;
        } else {
            let prefix_cached = self.fes.is_none() && (self.image_frame.is_some() || !self.cst_images.is_empty());
            let static_prefix = prefix_cached
                && self.scene_nodes.is_empty()
                && (self.cst.is_none() || self.font.is_empty())
                && fes_text::novel_projections(&self.data_services, f32::NEG_INFINITY, f32::INFINITY).is_empty();
            let unchanged = if prefix_cached {
                let layers = self.scene_image_prefix();
                self.scene_compositor.compose_prefix(&mut self.canvas, layers, static_prefix)?
            } else {
                self.scene_compositor.clear_prefix();
                self.canvas.clear(Color::rgba(0, 0, 0, 255));
                false
            };
            if static_prefix {
                let movie = self.movie_host.draw_exclusive_movie(&mut self.canvas)
                    .context("compose exclusive movie surface")?;
                if !movie {
                    self.scene_compositor.record_static(previous_revision, unchanged);
                }
                return Ok(());
            }
            self.render_scene_depth_range(f32::NEG_INFINITY, f32::INFINITY, prefix_cached, &mut graphics_snapshot)?;
        }
        self.movie_host.draw_exclusive_movie(&mut self.canvas)
            .context("compose exclusive movie surface")?;
        Ok(())
    }

    fn scene_image_prefix(&self) -> Vec<scene_compositor::SceneImage> {
        use scene_compositor::SceneImage;
        let mut layers: Vec<_> = self.cst_images.iter().flat_map(|(&(kind, bank), images)| {
            images.iter().map(move |image| (image_priority(kind, bank), SceneImage {
                frame: image.image.clone(), position: image.draw_position(), opacity: image.opacity,
            }))
        }).collect();
        layers.sort_by_key(|(priority, _)| *priority);
        self.image_frame.iter().map(|frame| SceneImage {
            frame: frame.clone(), position: Point::new(0, 0), opacity: 255,
        }).chain(layers.into_iter().map(|(_, image)| image)).collect()
    }

    /// Draw a normal scene slice into the already selected back/work buffer.
    /// Persistent logical VRAM is sampled separately and never cleared here.
    fn render_scene_depth_range(
        &mut self, lower: f32, upper: f32, prefix_cached: bool,
        graphics_snapshot: &mut Option<RgbaFrame>,
    ) -> Result<()> {
        if !prefix_cached && let Some(frame) = &self.image_frame {
            self.canvas
                .draw_frame(frame, &ImageDrawOptions::new(Point::new(0, 0)))
                .map_err(|error| anyhow::anyhow!(error))?;
        }
        if let Some(fes) = &self.fes {
            let mut fes_queue = RenderQueue::new();
            for layer in fes.media.values() {
                let animation_visible = layer
                    .animation_display
                    .is_none_or(|display| display.visible != 0);
                if !layer.visible || !animation_visible || !layer.is_drawable() {
                    continue;
                }
                let opacity = layer
                    .animation_display
                    .map(|display| {
                        display
                            .compose((0.0, 0.0), i32::from(layer.opacity), 1)
                            .blend
                            .clamp(0, 255) as u8
                    })
                    .unwrap_or(layer.opacity);
                let Some(plane) = layer.plane.as_ref().and_then(|name| fes.planes.get(name)) else {
                    if layer.plane.is_some() {
                        continue;
                    }
                    let Some(image) = layer.image.as_ref() else {
                        continue;
                    };
                    let mut options = ImageDrawOptions::new(layer.draw_position(&fes.planes));
                    options.opacity = opacity;
                    fes_queue.push(RenderNode::new(
                        layer.priority,
                        RenderCommand::Frame {
                            frame: image.clone(),
                            options,
                        },
                    ));
                    continue;
                };
                if !plane.visible {
                    continue;
                }
                let Some(image) = layer.image.as_ref() else {
                    continue;
                };
                let mut options = ImageDrawOptions::new(layer.draw_position(&fes.planes));
                options.opacity =
                    ((u16::from(opacity) * u16::from(plane.opacity) + 127) / 255) as u8;
                options.clip = Some(Rect::new(
                    plane.position.x,
                    plane.position.y,
                    plane.size[0],
                    plane.size[1],
                ));
                fes_queue.push(RenderNode::new(
                    layer.priority,
                    RenderCommand::Frame {
                        frame: image.clone(),
                        options,
                    },
                ));
            }
            self.canvas
                .render_queue(&fes_queue)
                .map_err(|error| anyhow::anyhow!(error))?;
        }
        if !prefix_cached {
            let mut image_queue = RenderQueue::new();
            for ((kind, bank), layers) in &self.cst_images {
                for image in layers {
                    let mut options = ImageDrawOptions::new(image.draw_position());
                    options.opacity = image.opacity;
                    image_queue.push(RenderNode::new(
                        image_priority(*kind, *bank),
                        RenderCommand::Frame {
                            frame: image.image.clone(),
                            options,
                        },
                    ));
                }
            }
            self.canvas
                .render_queue(&image_queue)
                .map_err(|error| anyhow::anyhow!(error))?;
        }
        if let Some(cst) = &self.cst
            && !self.font.is_empty()
        {
            let (speaker, text) = cst.display_text();
            draw_message_text(&mut self.canvas, &self.font, &speaker, &text, cst.display_encoding())?;
        }
        let mut movie_depths = BTreeMap::new();
        for node in self.scene_nodes.values() {
            if self.movie_host.plane_target_handle(node.id).is_none() {
                continue;
            }
            let mut depth = self
                .movie_host
                .movie_plane_depth(node.id)
                .unwrap_or(5000.);
            if let Some(properties) = self.movie_host.planes.properties.get(&node.id)
                && let Some(offset) = properties.parent_offset
                && let Some(parent) = self.scene_nodes.get(&node.parent)
            {
                let parent_base = self
                    .movie_host
                    .planes
                    .properties
                    .get(&parent.id)
                    .map_or(5000., |properties| properties.base[2]);
                depth = parent_base + offset[2];
            }
            movie_depths.insert(node.id, depth);
        }
        let nodes = self
            .data_services
            .planes
            .draw_order_with_depths(&self.scene_nodes, &movie_depths);
        let mut novel = fes_text::novel_projections(&self.data_services, lower, upper)
            .into_iter().peekable();
        for id in nodes {
            let Some(node) = self.scene_nodes.get(&id) else {
                continue;
            };
            if !node.visible || !node.initialized {
                continue;
            }
            if matches!(node.kind, 35..=39) { continue; }
            let depth = movie_depths.get(&id).copied().unwrap_or_else(|| {
                if node.kind == 2 { plane_native::plane_depth(node, &self.data_services.planes) }
                else { node.position[2] }
            });
            if !(lower <= depth && depth < upper) { continue; }
            while novel.peek().is_some_and(|layer| (layer.priority as f32) < depth) {
                let layer = novel.next().unwrap();
                self.canvas.draw_frame(&layer.frame, &layer.options)?;
            }
            if particle_native::draw_ordinary_task(
                id, &mut self.canvas, &self.particles,
                &self.movie_host.planes.projection, &self.movie_host.planes.cameras,
                lower, upper,
            )? { continue; }
            if self.data_services.css_models.draw(id, &mut self.canvas, &self.scan.resources, self.script_encoding)? { continue; }
            if self.data_services.particle_external.draw(id, &mut self.canvas, &self.particles)? { continue; }
            if self.kx_geometry(id).is_some() {
                let source = scene_compositor::graphics_source(&self.graphics_canvas, graphics_snapshot)?;
                if self.draw_kx_geometry(id, source).context("compose live KX plane mesh")? {
                    continue;
                }
            }
            let Some(node) = self.scene_nodes.get(&id) else { continue; };
            if self.data_services.emote.draw_scene_task(node, &mut self.canvas)
                .context("compose strict E-mote CPU task")?
            {
                continue;
            }
            if self.data_services.effekseer.draw_scene_task(node, &mut self.canvas)
                .context("compose type61 Effekseer CPU sprites")?
            {
                continue;
            }
            if self.data_services.tasks.javascript.render_plane(&mut self.canvas, node)
                .context("compose JavaScript-owned image plane")?
            {
                continue;
            }
            if self
                .movie_host
                .draw_movie_plane(&mut self.canvas, node, &self.scene_nodes)
                .context("draw scene movie plane")?
            {
                continue;
            }
            let canvas_source = if node.kind == 2 && self.data_services.planes.state(id)
                .is_some_and(|state| state.shape == plane_native::PlaneShape::Canvas)
            {
                Some(scene_compositor::graphics_source(&self.graphics_canvas, graphics_snapshot)?)
            } else { None };
            if plane_native::draw_plane_with_view(
                &mut self.canvas,
                node,
                &self.data_services.planes,
                canvas_source,
                &self.movie_host.planes.cameras,
                &self.movie_host.planes.projection,
            )? {
                continue;
            }
            if matches!(node.kind,
                0 | 1 | 6 | 8 | 11..=13 | 15 | 16 | 32..=34 | 40..=49 | 55 | 62 | 63)
            {
                // Script/input/list/text/media owners submit through their
                // actual services. A metadata node is not a color rectangle.
                continue;
            }
            bail!("task {} type {} has no concrete drawable renderer", node.id, node.kind);
        }
        for layer in novel {
            self.canvas.draw_frame(&layer.frame, &layer.options)?;
        }
        self.render_message_objects_range(lower, upper).context("compose message object depth range")?;
        Ok(())
    }

    fn update_particles(&mut self, frame: u64, _budget: usize) -> Result<bool> {
        // Execution belongs to kind16's task-tree body. This final pass only
        // publishes already committed frames and never resumes a particle VM.
        self.particle_snapshot = ParticleSnapshot {
            frame,
            objects: self.particles.len(),
            points: self
                .particles
                .values()
                .filter(|particle| particle.enabled)
                .flat_map(|particle| particle.points.iter().copied())
                .collect(),
        };
        Ok(true)
    }

    pub fn report(&self) -> HeadlessReport {
        HeadlessReport {
            root: self.root.path.clone(),
            scan: self.scan.clone(),
            compatibility: self.compatibility_report(),
            loaded: self.loaded.clone(),
            state: self.state.clone(),
            current_task_id: self.current_task_id,
            scene_nodes: self.scene_nodes.values().cloned().collect(),
            particle_snapshot: self.particle_snapshot.clone(),
            menu_item_requests: self.menu_item_requests.clone(),
            debug_mode: self.debug_mode,
            sound_config_enabled: self.sound_config_enabled,
            frame_hash: self.canvas.hash(),
            frame_size: self.logical_size(),
            frame_in_progress: self.frame_in_progress(),
            frame_rgba: self.frame_rgba().to_vec(),
            fes_exit_code: self.data_services.tasks.fes.direct_exit_code(self.current_task_id)
                .or_else(|| self.fes.as_ref().and_then(|fes| fes.exit_code)),
            events: self.events.clone(),
            log: self.log.clone(),
        }
    }
}

fn image_priority(kind: CstImageKind, bank: i32) -> i32 {
    let type_priority = match kind {
        CstImageKind::Background => 0,
        CstImageKind::Cg => 1,
        CstImageKind::Effect => 2,
        CstImageKind::Foreground => 3,
        CstImageKind::FrontWindow => 4,
        CstImageKind::Particle => 5,
    };
    type_priority * 1000 + bank
}

pub fn run_headless(
    path: impl AsRef<Path>,
    entry: Option<&str>,
    frames: u64,
    instruction_budget: usize,
) -> Result<HeadlessReport> {
    run_headless_with_message(path, entry, frames, instruction_budget, None)
}

pub fn run_headless_with_message(
    path: impl AsRef<Path>,
    entry: Option<&str>,
    frames: u64,
    instruction_budget: usize,
    message: Option<[u32; 4]>,
) -> Result<HeadlessReport> {
    run_headless_with_messages(path, entry, frames, instruction_budget, message)
}

pub fn run_headless_with_messages(
    path: impl AsRef<Path>,
    entry: Option<&str>,
    frames: u64,
    instruction_budget: usize,
    messages: impl IntoIterator<Item = [u32; 4]>,
) -> Result<HeadlessReport> {
    run_headless_with_encoding(
        path,
        entry,
        frames,
        instruction_budget,
        ScriptTextEncoding::default(),
        messages,
    )
}

pub fn run_headless_with_encoding(
    path: impl AsRef<Path>,
    entry: Option<&str>,
    frames: u64,
    instruction_budget: usize,
    encoding: ScriptTextEncoding,
    messages: impl IntoIterator<Item = [u32; 4]>,
) -> Result<HeadlessReport> {
    let mut player = HeadlessPlayer::open(path)?;
    player.set_script_encoding(encoding);
    if let Some(entry) = entry {
        player.load_entry(entry)?;
    } else {
        player.load_startup()?;
    }
    for message in messages {
        player.queue_message(message);
    }
    player.run_frames(frames, instruction_budget)?;
    Ok(player.report())
}

pub fn run() -> Result<()> {
    let report = run_headless(".", None, 1, 10_000)?;
    println!(
        "headless desktop host: {} files, {} archives, frame {}",
        report.scan.resources.files().len(),
        report.scan.resources.archives().len(),
        report.state.clock.frame
    );
    Ok(())
}

#[cfg(windows)]
fn open_window_player(path: &Path) -> Result<HeadlessPlayer> {
    let mut player = HeadlessPlayer::open(path)
        .with_context(|| format!("open game directory `{}`", path.display()))?;
    player.capture_script_evidence = false;
    player.enable_host_window_requests();
    player.initialize_general_window_host(1)?;
    // Both Windows presenters consume one logical input tick at 60 Hz.
    player.set_host_input_update_rate(60);
    player.load_startup()?;
    Ok(player)
}

#[cfg(any(windows, test))]
#[derive(Default)]
struct WindowDiagnostics {
    checked_events: usize,
}

#[cfg(any(windows, test))]
impl WindowDiagnostics {
    fn check_and_limit(&mut self, player: &mut HeadlessPlayer) -> Result<()> {
        const NOT_IMPLEMENTED: &[u8] = b"is not implemented";
        let new_events = &player.events()[self.checked_events..];
        let failure = new_events
            .iter()
            .rev()
            .find(|event| {
                event.kind == "vm-error"
                    || event.kind == "native-call"
                        && event.detail.as_bytes().windows(NOT_IMPLEMENTED.len())
                            .any(|text| text.eq_ignore_ascii_case(NOT_IMPLEMENTED))
            })
            .map(|event| anyhow::anyhow!(
                "runtime reported a failure at frame {}: {} {}",
                event.frame, event.kind, event.detail
            ));
        player.limit_host_diagnostics(10_000, 1_000);
        self.checked_events = player.events().len();
        failure.map_or(Ok(()), Err)
    }
}

#[cfg(windows)]
fn advance_window_frame(
    player: &mut HeadlessPlayer,
    window: &winit::window::Window,
    diagnostics: &mut WindowDiagnostics,
) -> Result<bool> {
    input_history_native::publish(player, window)?;
    let completed = player.advance_host_frame_with_retries(10_000)?;
    diagnostics.check_and_limit(player)?;
    Ok(completed)
}

#[cfg(windows)]
fn update_window_title(window: &winit::window::Window, player: &HeadlessPlayer, last: &mut String) {
    if settings_menu_windows::general_camouflaged(window) {
        last.clear();
    } else if let Some(title) = player.resolved_language_title() {
        if last.as_str() != title {
            window.set_title(title);
            last.clear();
            last.push_str(title);
        }
    }
}

#[cfg(windows)]
#[derive(Debug, Default)]
struct WindowInputState {
    cursor_position: Option<[f64; 2]>,
    ime_enabled: bool,
    ime_preedit: String,
}

#[cfg(windows)]
fn apply_window_pointer_request(
    player: &mut HeadlessPlayer,
    window: &winit::window::Window,
    input: &mut WindowInputState,
) -> Result<()> {
    if let Some(visible) = player.take_host_cursor_visibility_request() {
        window.set_cursor_visible(visible);
    }
    let Some([x, y]) = player.take_host_pointer_request() else { return Ok(()); };
    let size = window.inner_size();
    let [width, height] = player.logical_size();
    let viewport = cs2_renderer::Viewport::new(
        size.width, size.height, width, height, cs2_renderer::ViewportMode::Fit,
    ).map_err(|error| anyhow::anyhow!(error))?;
    let [left, top, _, _] = viewport.destination();
    let [scale_x, scale_y] = viewport.scale();
    let physical = [left + f64::from(x) * scale_x, top + f64::from(y) * scale_y];
    window.set_cursor_position(winit::dpi::PhysicalPosition::new(physical[0], physical[1]))
        .context("apply game pointer position to the window")?;
    input.cursor_position = Some(physical);
    Ok(())
}

#[cfg(windows)]
fn apply_window_control_requests(
    player: &mut HeadlessPlayer, window: &winit::window::Window, fullscreen: &mut bool,
) -> Result<()> {
    settings_menu_windows::apply_general_policy(player, window)?;
    settings_menu_windows::consume_menu_commands(player, window)?;
    while let Some(request) = player.take_host_window_request() {
        match request {
            HostWindowRequest::ToggleFullscreen => {
                *fullscreen = !*fullscreen;
                window.set_fullscreen(if *fullscreen {
                    Some(winit::window::Fullscreen::Borderless(None))
                } else { None });
                player.complete_host_window_request(Some(u32::from(!*fullscreen)))?;
            }
            HostWindowRequest::Minimize => {
                window.set_minimized(true);
                player.complete_host_window_request(None)?;
            }
            HostWindowRequest::Settings => settings_menu_windows::show_settings(player, window)?,
            HostWindowRequest::Menu => settings_menu_windows::install_menu(player, window)?,
            HostWindowRequest::MenuConfirmation => settings_menu_windows::confirm_menu(player, window)?,
            HostWindowRequest::SystemVolume => settings_menu_windows::open_system_volume(player, window)?,
        }
    }
    Ok(())
}

#[cfg(windows)]
fn handle_window_input(
    input: &mut WindowInputState,
    player: &mut HeadlessPlayer,
    event: &winit::event::WindowEvent,
    window_width: u32,
    window_height: u32,
) -> Result<bool> {
    use winit::event::{ElementState, Ime, MouseScrollDelta, WindowEvent};
    use cs2_runtime::message_object::MessageTextInputEvent;

    match event {
        WindowEvent::CloseRequested => {
            player.apply_host_input(InputEvent::QuitRequested)?;
            return Ok(true);
        }
        WindowEvent::Focused(focused) => {
            if !focused {
                input.ime_preedit.clear();
                input.cursor_position = None;
            }
            player.apply_host_text_input(MessageTextInputEvent::Focus(*focused))?;
        }
        WindowEvent::CursorLeft { .. } => {
            input.cursor_position = None;
        }
        WindowEvent::CursorMoved { position, .. } => {
            input.cursor_position = Some([position.x, position.y]);
            let [logical_width, logical_height] = player.logical_size();
            if let Some(point) =
                map_window_surface_point([position.x, position.y], window_width, window_height,
                    logical_width, logical_height)
            {
                player.apply_host_input(InputEvent::MouseMove {
                    x: point.x,
                    y: point.y,
                })?;
            }
        }
        WindowEvent::MouseInput { state, button, .. } => {
            let Some(button_code) = mouse_button_code(*button) else {
                return Ok(false);
            };
            let key = InputKey::MouseButton(button_code);
            if *state == ElementState::Pressed
                && input.cursor_position.and_then(|position| {
                    let [width, height] = player.logical_size();
                    map_window_surface_point(position, window_width, window_height, width, height)
                }).is_none()
            {
                return Ok(false);
            }
            player.apply_host_input(match state {
                ElementState::Pressed => InputEvent::KeyDown(key),
                ElementState::Released => InputEvent::KeyUp(key),
            })?;
        }
        WindowEvent::MouseWheel { delta, .. } => {
            let delta = match delta {
                MouseScrollDelta::LineDelta(_, y) => (*y * 120.0).round() as i32,
                MouseScrollDelta::PixelDelta(position) => position.y.round() as i32,
            };
            if delta != 0 {
                player
                    .apply_host_input(InputEvent::KeyDown(InputKey::MouseWheel(delta)))?;
            }
        }
        WindowEvent::Ime(Ime::Enabled) => {
            input.ime_enabled = true;
            player.apply_host_text_input(MessageTextInputEvent::ImeEnabled(true))?;
        }
        WindowEvent::Ime(Ime::Disabled) => {
            input.ime_enabled = false;
            input.ime_preedit.clear();
            player.apply_host_text_input(MessageTextInputEvent::ImeEnabled(false))?;
        }
        WindowEvent::Ime(Ime::Preedit(text, cursor)) => {
            input.ime_preedit = text.clone();
            let selection = cursor.map(|(start, end)| -> Result<[u32; 2]> {
                let start = text.get(..start).context("IME start splits its UTF-8 source")?
                    .encode_utf16().count();
                let end = text.get(..end).context("IME end splits its UTF-8 source")?
                    .encode_utf16().count();
                Ok([u32::try_from(start)?, u32::try_from(end)?])
            }).transpose()?;
            player.apply_host_text_input(MessageTextInputEvent::Preedit {
                text: text.clone(), selection,
            })?;
        }
        WindowEvent::Ime(Ime::Commit(text)) => {
            input.ime_preedit.clear();
            if !text.is_empty() {
                player.apply_host_text_input(MessageTextInputEvent::Commit(text.clone()))?;
            }
        }
        WindowEvent::KeyboardInput { event, .. } => {
            if event.state == ElementState::Pressed && !input.ime_enabled
                && let Some(text) = event.text.as_ref()
            {
                let committed = text.chars().filter(|character| !character.is_control()).collect::<String>();
                if !committed.is_empty() {
                    player.apply_host_text_input(MessageTextInputEvent::HardwareText(committed))?;
                }
            }
            let Some(key_code) = window_key_code(&event.logical_key) else {
                return Ok(false);
            };
            player.apply_host_text_key(key_code, event.state == ElementState::Pressed)?;
        }
        _ => {}
    }
    Ok(false)
}

#[cfg(windows)]
fn is_fullscreen_toggle(event: &winit::event::WindowEvent) -> bool {
    use winit::event::{ElementState, WindowEvent};
    use winit::keyboard::{Key, NamedKey};

    matches!(
        event,
        WindowEvent::KeyboardInput { event, .. }
            if event.state == ElementState::Pressed
                && !event.repeat
                && matches!(&event.logical_key, Key::Named(NamedKey::F11))
    )
}

#[cfg(windows)]
fn mouse_button_code(button: winit::event::MouseButton) -> Option<u8> {
    match button {
        winit::event::MouseButton::Left => Some(0),
        winit::event::MouseButton::Right => Some(1),
        winit::event::MouseButton::Middle => Some(2),
        winit::event::MouseButton::Back => Some(3),
        winit::event::MouseButton::Forward => Some(4),
        winit::event::MouseButton::Other(value) => u8::try_from(value).ok(),
    }
}

#[cfg(windows)]
fn window_key_code(key: &winit::keyboard::Key) -> Option<u32> {
    use winit::keyboard::{Key, NamedKey};

    match key {
        Key::Character(text) => text
            .chars()
            .next()
            .map(|character| character.to_ascii_uppercase() as u32),
        Key::Named(key) => match key {
            NamedKey::Backspace => Some(0x08),
            NamedKey::Tab => Some(0x09),
            NamedKey::Enter => Some(0x0d),
            NamedKey::Shift => Some(0x10),
            NamedKey::Control => Some(0x11),
            NamedKey::Alt => Some(0x12),
            NamedKey::Pause => Some(0x13),
            NamedKey::CapsLock => Some(0x14),
            NamedKey::Escape => Some(0x1b),
            NamedKey::Space => Some(0x20),
            NamedKey::PageUp => Some(0x21),
            NamedKey::PageDown => Some(0x22),
            NamedKey::End => Some(0x23),
            NamedKey::Home => Some(0x24),
            NamedKey::ArrowLeft => Some(0x25),
            NamedKey::ArrowUp => Some(0x26),
            NamedKey::ArrowRight => Some(0x27),
            NamedKey::ArrowDown => Some(0x28),
            NamedKey::Insert => Some(0x2d),
            NamedKey::Delete => Some(0x2e),
            NamedKey::F1 => Some(0x70),
            NamedKey::F2 => Some(0x71),
            NamedKey::F3 => Some(0x72),
            NamedKey::F4 => Some(0x73),
            NamedKey::F5 => Some(0x74),
            NamedKey::F6 => Some(0x75),
            NamedKey::F7 => Some(0x76),
            NamedKey::F8 => Some(0x77),
            NamedKey::F9 => Some(0x78),
            NamedKey::F10 => Some(0x79),
            NamedKey::F11 => Some(0x7a),
            NamedKey::F12 => Some(0x7b),
            _ => None,
        },
        _ => None,
    }
}

#[cfg(windows)]
fn arrow_event_code(key: &winit::keyboard::Key) -> Option<u32> {
    use winit::keyboard::{Key, NamedKey};
    match key {
        Key::Named(NamedKey::ArrowUp) => Some(11),
        Key::Named(NamedKey::ArrowDown) => Some(12),
        Key::Named(NamedKey::ArrowLeft) => Some(13),
        Key::Named(NamedKey::ArrowRight) => Some(14),
        _ => None,
    }
}

#[cfg(windows)]
fn arrow_release_event_code(key: &winit::keyboard::Key) -> Option<u32> {
    arrow_event_code(key).map(|event| event + 4)
}

#[cfg(all(windows, feature = "gpu"))]
struct WindowGpu {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
    pipeline: wgpu::RenderPipeline,
    frame_size: [u32; 2],
}

#[cfg(all(windows, feature = "gpu"))]
impl WindowGpu {
    fn new(window: std::sync::Arc<winit::window::Window>, frame_size: [u32; 2]) -> Result<Self> {
        let [width, height] = frame_size;
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let surface = instance
            .create_surface(Arc::clone(&window))
            .context("create wgpu surface")?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        }))
        .map_err(|error| anyhow::anyhow!("request wgpu adapter: {error}"))?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("cs2-device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::downlevel_defaults(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .context("request wgpu device")?;
        let mut config = surface
            .get_default_config(&adapter, window.inner_size().width.max(1), window.inner_size().height.max(1))
            .context("surface has no supported configuration")?;
        let supported = surface.get_capabilities(&adapter).present_modes;
        config.present_mode = match std::env::var("CS2_WINDOW_PRESENT_MODE").as_deref() {
            Ok("no-vsync") => wgpu::PresentMode::AutoNoVsync,
            Ok("fifo") => wgpu::PresentMode::Fifo,
            _ if supported.contains(&wgpu::PresentMode::Mailbox) => wgpu::PresentMode::Mailbox,
            _ => wgpu::PresentMode::Fifo,
        };
        config.desired_maximum_frame_latency = 2;
        surface.configure(&device, &config);

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("cs2-frame-texture"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let texture_view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("cs2-frame-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("cs2-frame-layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("cs2-frame-bind-group"),
            layout: &bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&texture_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("cs2-frame-shader"),
            source: wgpu::ShaderSource::Wgsl(std::borrow::Cow::Borrowed(
                r#"
@group(0) @binding(0) var frame_texture: texture_2d<f32>;
@group(0) @binding(1) var frame_sampler: sampler;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VertexOutput {
    var positions = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 1.0, -1.0),
        vec2<f32>(-1.0,  1.0),
        vec2<f32>(-1.0,  1.0),
        vec2<f32>( 1.0, -1.0),
        vec2<f32>( 1.0,  1.0),
    );
    var uvs = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 0.0),
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(1.0, 0.0),
    );
    var output: VertexOutput;
    output.position = vec4<f32>(positions[index], 0.0, 1.0);
    output.uv = uvs[index];
    return output;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    return textureSample(frame_texture, frame_sampler, input.uv);
}
"#,
            )),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("cs2-frame-pipeline-layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("cs2-frame-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview: None,
            cache: None,
        });
        Ok(Self {
            surface,
            device,
            queue,
            config,
            texture,
            bind_group,
            pipeline,
            frame_size,
        })
    }

    fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
    }

    fn draw(&mut self, rgba: &[u8]) -> Result<bool> {
        let [width, height] = self.frame_size;
        if rgba.len() != width as usize * height as usize * 4 {
            bail!("GPU frame pixel count does not match logical canvas dimensions")
        }
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * width),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        let frame = match self.surface.get_current_texture() {
            Ok(frame) => frame,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                self.surface.configure(&self.device, &self.config);
                self.surface
                    .get_current_texture()
                    .context("reacquire window frame after surface reconfiguration")?
            }
            Err(wgpu::SurfaceError::Timeout) => return Ok(false),
            Err(error) => return Err(anyhow::anyhow!("acquire window frame: {error}")),
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("cs2-frame-encoder"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("cs2-frame-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            let viewport = cs2_renderer::Viewport::new(
                self.config.width, self.config.height, width, height,
                cs2_renderer::ViewportMode::Fit,
            ).map_err(|error| anyhow::anyhow!(error))?;
            let [left, top, view_width, view_height] = viewport.destination();
            pass.set_viewport(left as f32, top as f32, view_width as f32, view_height as f32, 0.0, 1.0);
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.draw(0..6, 0..1);
        }
        self.queue.submit(Some(encoder.finish()));
        frame.present();
        Ok(true)
    }
}

#[cfg(all(windows, feature = "gpu"))]
pub fn run_windowed(path: impl AsRef<Path>) -> Result<()> {
    use std::time::{Duration, Instant};
    use winit::application::ApplicationHandler;
    use winit::event::WindowEvent;
    use winit::event_loop::{ActiveEventLoop, ControlFlow};
    use winit::window::{Window, WindowAttributes};

    struct WindowApp {
        player: Option<HeadlessPlayer>,
        path: PathBuf,
        window: Option<std::sync::Arc<Window>>,
        gpu: Option<WindowGpu>,
        input: WindowInputState,
        pacer: window_media::WindowPacer,
        diagnostics: WindowDiagnostics,
        applied_title: String,
        fullscreen: bool,
        error: Option<String>,
    }

    impl ApplicationHandler for WindowApp {
        fn resumed(&mut self, event_loop: &ActiveEventLoop) {
            if self.window.is_some() {
                return;
            }
            let player = match open_window_player(&self.path) {
                Ok(player) => player,
                Err(error) => {
                    self.error = Some(format!("{error:#}"));
                    event_loop.exit();
                    return;
                }
            };
            let [width, height] = player.logical_size();
            let title = match player.language_window_title() {
                Ok(title) => title.unwrap_or_else(|| "CatSystem2 Simulator".into()),
                Err(error) => {
                    self.error = Some(format!("resolve window title: {error:#}"));
                    event_loop.exit();
                    return;
                }
            };
            let attributes = WindowAttributes::default()
                .with_title(title)
                .with_inner_size(winit::dpi::LogicalSize::new(f64::from(width), f64::from(height)));
            let window = match event_loop.create_window(attributes) {
                Ok(window) => std::sync::Arc::new(window),
                Err(error) => {
                    self.error = Some(format!("create window: {error}"));
                    event_loop.exit();
                    return;
                }
            };
            let gpu = match WindowGpu::new(window.clone(), [width, height]) {
                Ok(gpu) => gpu,
                Err(error) => {
                    self.error = Some(format!("initialize GPU host: {error:#}"));
                    event_loop.exit();
                    return;
                }
            };
            window.set_ime_allowed(true);
            self.player = Some(player);
            self.gpu = Some(gpu);
            self.window = Some(window);
        }

        fn window_event(
            &mut self,
            event_loop: &ActiveEventLoop,
            _window_id: winit::window::WindowId,
            event: WindowEvent,
        ) {
            let Some(window) = self.window.as_ref() else {
                return;
            };
            let size = window.inner_size();
            if self.input.ime_enabled {
                if let WindowEvent::CursorMoved { position, .. } = &event {
                    window.set_ime_cursor_area(*position, winit::dpi::PhysicalSize::new(1u32, 24u32));
                }
            }
            if is_fullscreen_toggle(&event) {
                self.fullscreen = !self.fullscreen;
                window.set_fullscreen(if self.fullscreen {
                    Some(winit::window::Fullscreen::Borderless(None))
                } else {
                    None
                });
                if let Some(player) = self.player.as_mut() {
                    player.set_host_windowed_mode(u32::from(!self.fullscreen));
                }
                return;
            }
            if let Some(player) = self.player.as_mut() {
                match handle_window_input(&mut self.input, player, &event, size.width, size.height) {
                    Ok(true) => { event_loop.exit(); return; }
                    Err(error) => {
                        self.error = Some(format!("window input: {error:#}"));
                        event_loop.exit();
                        return;
                    }
                    Ok(false) => {}
                }
            }
            match event {
                WindowEvent::CloseRequested => event_loop.exit(),
                WindowEvent::Resized(size) => {
                    if let Some(gpu) = self.gpu.as_mut() {
                        gpu.resize(size.width, size.height);
                    }
                }
                WindowEvent::RedrawRequested => {
                    if size.width == 0 || size.height == 0 { return; }
                    if Instant::now() < self.pacer.next { return; }
                    if let Some(player) = self.player.as_mut() {
                        match advance_window_frame(player, window, &mut self.diagnostics) {
                            Ok(true) => self.pacer.completed(Instant::now()),
                            Ok(false) => self.pacer.retry(Instant::now(), player.frame_in_progress()),
                            Err(error) => {
                                self.error = Some(format!("{error:#}"));
                                event_loop.exit();
                                return;
                            }
                        }
                        if player.quit_requested() {
                            event_loop.exit();
                            return;
                        }
                        let presentation = (|| -> Result<()> {
                            apply_window_control_requests(player, window, &mut self.fullscreen)?;
                            apply_window_pointer_request(player, window, &mut self.input)?;
                            let frame = window_media::prepare(player);
                            if let Some(gpu) = self.gpu.as_mut() {
                                if gpu.draw(frame.rgba)? { frame.acknowledge()?; }
                            }
                            Ok(())
                        })();
                        if let Err(error) = presentation {
                            self.error = Some(format!("present frame: {error:#}"));
                            event_loop.exit();
                            return;
                        }
                        update_window_title(window, player, &mut self.applied_title);
                    }
                }
                _ => {}
            }
        }

        fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
            event_loop.set_control_flow(ControlFlow::WaitUntil(self.pacer.next));
            if let Some(window) = self.window.as_ref() {
                if Instant::now() >= self.pacer.next {
                    let size = window.inner_size();
                    if size.width == 0 || size.height == 0 {
                        self.pacer.suspend(Instant::now());
                    } else { window.request_redraw(); }
                }
            }
        }
    }

    let event_loop = input_history_native::event_loop()?;
    let mut app = WindowApp {
        player: None,
        path: path.as_ref().to_path_buf(),
        window: None,
        gpu: None,
        input: WindowInputState::default(),
        pacer: window_media::WindowPacer::default(),
        diagnostics: WindowDiagnostics::default(),
        applied_title: String::new(),
        fullscreen: false,
        error: None,
    };
    event_loop
        .run_app(&mut app)
        .context("run Windows event loop")?;
    if let Some(error) = app.error {
        bail!("{error}");
    }
    Ok(())
}

#[cfg(all(windows, not(feature = "gpu")))]
pub fn run_windowed(path: impl AsRef<Path>) -> Result<()> {
    use std::time::{Duration, Instant};
    use winit::application::ApplicationHandler;
    use winit::event::WindowEvent;
    use winit::event_loop::{ActiveEventLoop, ControlFlow};
    use winit::window::{Window, WindowAttributes};

    struct WindowApp {
        player: Option<HeadlessPlayer>,
        path: PathBuf,
        window: Option<Window>,
        input: WindowInputState,
        pacer: window_media::WindowPacer,
        diagnostics: WindowDiagnostics,
        applied_title: String,
        fullscreen: bool,
        error: Option<String>,
    }

    impl ApplicationHandler for WindowApp {
        fn resumed(&mut self, event_loop: &ActiveEventLoop) {
            if self.window.is_some() {
                return;
            }
            let player = match open_window_player(&self.path) {
                Ok(player) => player,
                Err(error) => {
                    self.error = Some(format!("{error:#}"));
                    event_loop.exit();
                    return;
                }
            };
            let [width, height] = player.logical_size();
            let title = match player.language_window_title() {
                Ok(title) => title.unwrap_or_else(|| "CatSystem2 Simulator".into()),
                Err(error) => {
                    self.error = Some(format!("resolve window title: {error:#}"));
                    event_loop.exit();
                    return;
                }
            };
            let attributes = WindowAttributes::default()
                .with_title(title)
                .with_inner_size(winit::dpi::LogicalSize::new(f64::from(width), f64::from(height)));
            match event_loop.create_window(attributes) {
                Ok(window) => {
                    window.set_ime_allowed(true);
                    self.window = Some(window);
                    self.player = Some(player);
                }
                Err(error) => {
                    self.error = Some(format!("create window: {error}"));
                    event_loop.exit();
                }
            }
        }

        fn window_event(
            &mut self,
            event_loop: &ActiveEventLoop,
            _window_id: winit::window::WindowId,
            event: WindowEvent,
        ) {
            let Some(window) = self.window.as_ref() else {
                return;
            };
            let size = window.inner_size();
            if self.input.ime_enabled {
                if let WindowEvent::CursorMoved { position, .. } = &event {
                    window.set_ime_cursor_area(*position, winit::dpi::PhysicalSize::new(1u32, 24u32));
                }
            }
            if is_fullscreen_toggle(&event) {
                self.fullscreen = !self.fullscreen;
                window.set_fullscreen(if self.fullscreen {
                    Some(winit::window::Fullscreen::Borderless(None))
                } else {
                    None
                });
                if let Some(player) = self.player.as_mut() {
                    player.set_host_windowed_mode(u32::from(!self.fullscreen));
                }
                return;
            }
            if let Some(player) = self.player.as_mut() {
                match handle_window_input(&mut self.input, player, &event, size.width, size.height) {
                    Ok(true) => { event_loop.exit(); return; }
                    Err(error) => {
                        self.error = Some(format!("window input: {error:#}"));
                        event_loop.exit();
                        return;
                    }
                    Ok(false) => {}
                }
            }
            match event {
                WindowEvent::CloseRequested => event_loop.exit(),
                WindowEvent::Resized(_) => window.request_redraw(),
                WindowEvent::RedrawRequested => {
                    if size.width == 0 || size.height == 0 { return; }
                    if Instant::now() < self.pacer.next { return; }
                    if let Some(player) = self.player.as_mut() {
                        match advance_window_frame(player, window, &mut self.diagnostics) {
                            Ok(true) => self.pacer.completed(Instant::now()),
                            Ok(false) => self.pacer.retry(Instant::now(), player.frame_in_progress()),
                            Err(error) => {
                                self.error = Some(format!("{error:#}"));
                                event_loop.exit();
                                return;
                            }
                        }
                        if player.quit_requested() {
                            event_loop.exit();
                            return;
                        }
                        let presentation = (|| -> Result<()> {
                            apply_window_control_requests(player, window, &mut self.fullscreen)?;
                            apply_window_pointer_request(player, window, &mut self.input)?;
                            let frame = window_media::prepare(player);
                            blit_rgba_to_window(window, frame.rgba, player.logical_size())?;
                            frame.acknowledge()
                        })();
                        if let Err(error) = presentation {
                            self.error = Some(format!("present frame: {error:#}"));
                            event_loop.exit();
                            return;
                        }
                        update_window_title(window, player, &mut self.applied_title);
                    }
                }
                _ => {}
            }
        }

        fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
            event_loop.set_control_flow(ControlFlow::WaitUntil(self.pacer.next));
            if let Some(window) = self.window.as_ref() {
                if Instant::now() >= self.pacer.next {
                    let size = window.inner_size();
                    if size.width == 0 || size.height == 0 {
                        self.pacer.suspend(Instant::now());
                    } else { window.request_redraw(); }
                }
            }
        }
    }

    let event_loop = input_history_native::event_loop()?;
    let mut app = WindowApp {
        player: None,
        path: path.as_ref().to_path_buf(),
        window: None,
        input: WindowInputState::default(),
        pacer: window_media::WindowPacer::default(),
        diagnostics: WindowDiagnostics::default(),
        applied_title: String::new(),
        fullscreen: false,
        error: None,
    };
    event_loop
        .run_app(&mut app)
        .context("run Windows event loop")?;
    if let Some(error) = app.error {
        bail!("{error}");
    }
    Ok(())
}

fn map_window_point(position: [f64; 2], width: u32, height: u32) -> Option<Point> {
    map_window_surface_point(position, width, height, 800, 600)
}

fn map_window_surface_point(
    position: [f64; 2], width: u32, height: u32, logical_width: u32, logical_height: u32,
) -> Option<Point> {
    cs2_renderer::Viewport::new(width, height, logical_width, logical_height,
        cs2_renderer::ViewportMode::Fit).ok()?.map_point(position)
}

fn pointer_message(
    event: u32,
    position: Option<[f64; 2]>,
    width: u32,
    height: u32,
) -> Option<[u32; 4]> {
    pointer_message_for_surface(event, position, width, height, [800, 600])
}

fn pointer_message_for_surface(
    event: u32,
    position: Option<[f64; 2]>,
    width: u32,
    height: u32,
    logical_size: [u32; 2],
) -> Option<[u32; 4]> {
    let point = map_window_surface_point(position?, width, height, logical_size[0], logical_size[1])?;
    Some([event, point.x as u32, point.y as u32, 0])
}

#[cfg(all(windows, not(feature = "gpu")))]
fn blit_rgba_to_window(window: &winit::window::Window, rgba: &[u8], logical_size: [u32; 2]) -> Result<()> {
    use windows_sys::Win32::Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, GetDC, ReleaseDC, SRCCOPY,
        StretchDIBits, PatBlt, BLACKNESS,
    };
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let size = window.inner_size();
    let [width, height] = logical_size;
    if size.width == 0 || size.height == 0 || width == 0 || height == 0 {
        return Ok(());
    }
    if rgba.len() != width as usize * height as usize * 4 {
        bail!("frame pixel count does not match logical canvas dimensions")
    }
    let handle = window.window_handle().context("get HWND")?.as_raw();
    let RawWindowHandle::Win32(handle) = handle else {
        bail!("winit did not provide a Win32 HWND")
    };
    let hwnd = handle.hwnd.get() as windows_sys::Win32::Foundation::HWND;
    let hdc = unsafe { GetDC(hwnd) };
    if hdc == 0 {
        bail!("GetDC failed")
    }

    let mut bgra = Vec::with_capacity(rgba.len());
    for pixel in rgba.chunks_exact(4) {
        bgra.extend_from_slice(&[pixel[2], pixel[1], pixel[0], 255]);
    }
    let bitmap = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width as i32,
            biHeight: -(height as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            biSizeImage: bgra.len() as u32,
            biXPelsPerMeter: 0,
            biYPelsPerMeter: 0,
            biClrUsed: 0,
            biClrImportant: 0,
        },
        bmiColors: [unsafe { std::mem::zeroed() }],
    };
    let viewport = cs2_renderer::Viewport::new(size.width, size.height, width, height,
        cs2_renderer::ViewportMode::Fit).map_err(|error| anyhow::anyhow!(error))?;
    let [left, top, view_width, view_height] = viewport.destination();
    let result = unsafe {
        PatBlt(hdc, 0, 0, size.width as i32, size.height as i32, BLACKNESS);
        StretchDIBits(
            hdc,
            left.round() as i32,
            top.round() as i32,
            view_width.round() as i32,
            view_height.round() as i32,
            0,
            0,
            width as i32,
            height as i32,
            bgra.as_ptr().cast(),
            &bitmap,
            DIB_RGB_COLORS,
            SRCCOPY,
        )
    };
    unsafe { ReleaseDC(hwnd, hdc) };
    if result == 0 || result == -1 {
        bail!("StretchDIBits failed")
    }
    Ok(())
}

#[cfg(windows)]
pub fn select_game_directory() -> Result<Option<PathBuf>> {
    use std::ffi::c_void;
    use std::ptr::null_mut;

    #[repr(C)]
    struct BrowseInfoW {
        hwnd_owner: *mut c_void,
        pidl_root: *mut c_void,
        psz_display_name: *mut u16,
        lpsz_title: *const u16,
        ul_flags: u32,
        lpfn: Option<unsafe extern "system" fn(*mut c_void, u32, isize, isize) -> i32>,
        lparam: isize,
        i_image: i32,
    }

    unsafe extern "system" {
        fn SHBrowseForFolderW(info: *const BrowseInfoW) -> *mut c_void;
        fn SHGetPathFromIDListW(item: *const c_void, path: *mut u16) -> i32;
        fn CoTaskMemFree(value: *const c_void);
    }

    let title: Vec<u16> = "Select a CatSystem2 game directory\0"
        .encode_utf16()
        .collect();
    let mut display_name = [0_u16; 260];
    let info = BrowseInfoW {
        hwnd_owner: null_mut(),
        pidl_root: null_mut(),
        psz_display_name: display_name.as_mut_ptr(),
        lpsz_title: title.as_ptr(),
        ul_flags: 1 | 64,
        lpfn: None,
        lparam: 0,
        i_image: 0,
    };
    let item = unsafe { SHBrowseForFolderW(&info) };
    if item.is_null() {
        return Ok(None);
    }

    let mut path = [0_u16; 32768];
    let success = unsafe { SHGetPathFromIDListW(item.cast(), path.as_mut_ptr()) != 0 };
    unsafe { CoTaskMemFree(item.cast()) };
    if !success {
        bail!("Windows folder picker returned a path that could not be decoded");
    }
    let length = path
        .iter()
        .position(|value| *value == 0)
        .unwrap_or(path.len());
    if length == 0 {
        return Ok(None);
    }
    let path = String::from_utf16(&path[..length]).context("decode Windows folder picker path")?;
    Ok(Some(PathBuf::from(path)))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DesktopWindowState {
    pub width: u32,
    pub height: u32,
    pub focused: bool,
    pub fullscreen: bool,
    pub close_requested: bool,
}

pub trait WindowHost {
    fn state(&self) -> DesktopWindowState;
    fn set_title(&mut self, title: &str) -> Result<()>;
    fn set_fullscreen(&mut self, fullscreen: bool) -> Result<()>;
    fn request_redraw(&self) -> Result<()>;
}

#[cfg(windows)]
pub struct WinitWindowHost {
    window: Arc<winit::window::Window>,
    state: DesktopWindowState,
}

#[cfg(windows)]
impl WinitWindowHost {
    pub fn new(window: Arc<winit::window::Window>) -> Self {
        let size = window.inner_size();
        Self {
            window,
            state: DesktopWindowState {
                width: size.width,
                height: size.height,
                focused: true,
                fullscreen: false,
                close_requested: false,
            },
        }
    }

    pub fn window(&self) -> &winit::window::Window {
        &self.window
    }

    pub fn apply_event(&mut self, event: &winit::event::WindowEvent) {
        use winit::event::WindowEvent;
        match event {
            WindowEvent::CloseRequested => self.state.close_requested = true,
            WindowEvent::Focused(focused) => self.state.focused = *focused,
            WindowEvent::Resized(size) => {
                self.state.width = size.width;
                self.state.height = size.height;
            }
            _ => {}
        }
    }
}

#[cfg(windows)]
impl WindowHost for WinitWindowHost {
    fn state(&self) -> DesktopWindowState {
        self.state
    }

    fn set_title(&mut self, title: &str) -> Result<()> {
        self.window.set_title(title);
        Ok(())
    }

    fn set_fullscreen(&mut self, fullscreen: bool) -> Result<()> {
        self.window.set_fullscreen(if fullscreen {
            Some(winit::window::Fullscreen::Borderless(None))
        } else {
            None
        });
        self.state.fullscreen = fullscreen;
        Ok(())
    }

    fn request_redraw(&self) -> Result<()> {
        self.window.request_redraw();
        Ok(())
    }
}

#[cfg(windows)]
pub fn select_media_file(initial_dir: Option<&Path>) -> Result<Option<PathBuf>> {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use std::ptr::null_mut;

    #[repr(C)]
    struct OpenFileNameW {
        l_struct_size: u32,
        hwnd_owner: *mut c_void,
        h_instance: *mut c_void,
        lpstr_filter: *const u16,
        lpstr_custom_filter: *mut u16,
        n_max_cust_filter: u32,
        n_filter_index: u32,
        lpstr_file: *mut u16,
        n_max_file: u32,
        lpstr_file_title: *mut u16,
        n_max_file_title: u32,
        lpstr_initial_dir: *const u16,
        lpstr_title: *const u16,
        flags: u32,
        n_file_offset: u16,
        n_file_extension: u16,
        lpstr_def_ext: *const u16,
        l_cust_data: isize,
        lpfn_hook: *mut c_void,
        lp_template_name: *const u16,
        pv_reserved: *mut c_void,
        dw_reserved: u32,
        flags_ex: u32,
    }

    #[link(name = "comdlg32")]
    unsafe extern "system" {
        fn GetOpenFileNameW(file_name: *mut OpenFileNameW) -> i32;
    }

    let filter: Vec<u16> =
        "CatSystem2 media\0*.wav;*.ogg;*.mpg;*.mpeg;*.m1v;*.mpv;*.webm\0All files\0*.*\0\0"
            .encode_utf16()
            .collect();
    let title: Vec<u16> = "Select CatSystem2 media\0".encode_utf16().collect();
    let initial = initial_dir
        .map(|path| {
            path.as_os_str()
                .encode_wide()
                .chain(std::iter::once(0))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let mut selected = vec![0_u16; 32_768];
    let mut dialog = OpenFileNameW {
        l_struct_size: std::mem::size_of::<OpenFileNameW>() as u32,
        hwnd_owner: null_mut(),
        h_instance: null_mut(),
        lpstr_filter: filter.as_ptr(),
        lpstr_custom_filter: null_mut(),
        n_max_cust_filter: 0,
        n_filter_index: 1,
        lpstr_file: selected.as_mut_ptr(),
        n_max_file: selected.len() as u32,
        lpstr_file_title: null_mut(),
        n_max_file_title: 0,
        lpstr_initial_dir: if initial.is_empty() {
            std::ptr::null()
        } else {
            initial.as_ptr()
        },
        lpstr_title: title.as_ptr(),
        flags: 0x0000_1000 | 0x0000_0800,
        n_file_offset: 0,
        n_file_extension: 0,
        lpstr_def_ext: std::ptr::null(),
        l_cust_data: 0,
        lpfn_hook: null_mut(),
        lp_template_name: std::ptr::null(),
        pv_reserved: null_mut(),
        dw_reserved: 0,
        flags_ex: 0,
    };
    if unsafe { GetOpenFileNameW(&mut dialog) } == 0 {
        return Ok(None);
    }
    let length = selected
        .iter()
        .position(|value| *value == 0)
        .unwrap_or(selected.len());
    if length == 0 {
        return Ok(None);
    }
    Ok(Some(PathBuf::from(
        String::from_utf16(&selected[..length]).context("decode media file path")?,
    )))
}

#[cfg(not(windows))]
pub fn select_game_directory() -> Result<Option<PathBuf>> {
    bail!("the Windows folder picker is only available on Windows")
}

#[cfg(not(windows))]
pub fn select_media_file(_initial_dir: Option<&Path>) -> Result<Option<PathBuf>> {
    bail!("the Windows media file picker is only available on Windows")
}

#[cfg(not(windows))]
pub fn run_windowed(_path: impl AsRef<Path>) -> Result<()> {
    bail!("windowed desktop host is only available on Windows")
}

fn format_diagnostic(diagnostic: &cs2_runtime::ScanDiagnostic) -> String {
    let level = match diagnostic.level {
        ScanDiagnosticLevel::Warning => "warning",
        ScanDiagnosticLevel::Error => "error",
    };
    format!(
        "{level}: {}: {}",
        diagnostic.path.display(),
        diagnostic.message
    )
}

fn read_entry_with_fallback(resources: &ResourceIndex, name: &str) -> Result<Vec<u8>> {
    // A mounted virtual prefix already describes all permitted fallbacks.
    // Stripping it could load an unrelated same-named script or image.
    if let Some((prefix, _)) = name.split_once('/')
        && resources.resource_router().is_some_and(|router| {
            router.slots.iter().any(|slot| String::from_utf16_lossy(&slot.prefix).eq_ignore_ascii_case(prefix))
                || router.groups.iter().any(|group| String::from_utf16_lossy(&group.prefix).eq_ignore_ascii_case(prefix))
        })
    {
        return resources.read(name).map_err(anyhow::Error::new);
    }
    let mut candidates = vec![name.to_string()];
    if !name.starts_with("system/") && !name.starts_with("system\\") {
        candidates.push(format!("system/{name}"));
    }
    let fallback = name.rsplit(['/', '\\']).next().unwrap_or(name);
    if fallback != name {
        candidates.push(fallback.to_string());
        candidates.push(format!("system/{fallback}"));
        candidates.push(format!("system/config/{fallback}"));
    }
    if name
        .split(['/', '\\'])
        .next()
        .is_some_and(|component| component.to_ascii_lowercase().ends_with(".int"))
    {
        candidates.push(fallback.to_string());
    }
    let mut first_error = None;
    for candidate in candidates {
        match resources.read(&candidate) {
            Ok(bytes) => return Ok(bytes),
            Err(error @ RuntimeError::ResourceNotFound(_)) => first_error = Some(error),
            Err(error) => return Err(anyhow::Error::new(error)),
        }
    }
    Err(first_error
        .map(anyhow::Error::msg)
        .unwrap_or_else(|| anyhow::anyhow!("resource is unavailable")))
}

fn load_cstl(resources: &ResourceIndex, scene_name: &str) -> Option<CstlTable> {
    let path = Path::new(scene_name);
    let candidate = path
        .with_extension("cstl")
        .to_string_lossy()
        .replace('\\', "/");
    let bytes = read_entry_with_fallback(resources, &candidate).ok()?;
    CstlTable::parse(&bytes).ok()
}

fn parse_hg_frame_id(value: &str) -> Option<usize> {
    if value.len() == 1 && value.as_bytes()[0].is_ascii_hexdigit() {
        usize::from_str_radix(value, 16).ok()
    } else {
        value.parse().ok()
    }
}

fn parse_cst_position(value: Option<&str>, previous: i32) -> Result<i32> {
    let Some(value) = value else {
        return Ok(0);
    };
    if value == "@" {
        return Ok(previous);
    }
    if let Some(parsed) = parse_cst_integer(value.as_bytes()) {
        return Ok(parsed);
    }
    if value.starts_with("#(") && value.ends_with(')') {
        return Ok(previous);
    }
    bail!("CST image position is not a supported integer: {value}")
}

fn load_cst_images(
    resources: &ResourceIndex,
    kind: CstImageKind,
    asset: &str,
    position: Point,
    encoding: ScriptTextEncoding,
) -> Result<(Vec<String>, Vec<CstImageLayer>)> {
    let arguments = asset.split(',').map(str::trim).collect::<Vec<_>>();
    let resource = arguments.first().copied().unwrap_or_default();
    let variant = arguments.get(1).copied().unwrap_or_default();
    if resource.is_empty() {
        bail!("CST image resource is empty");
    }

    let names = if kind == CstImageKind::Cg && arguments.len() >= 5 {
        let zero_pad = |value: &str, width: usize| format!("{:0>width$}", value, width = width);
        vec![
            format!("{}_{}", resource, arguments[1]),
            format!("{}_{}", resource, zero_pad(arguments[3], 3)),
            format!("{}_{}", resource, zero_pad(arguments[4], 4)),
        ]
    } else {
        vec![resource.to_owned()]
    };

    let mut resource_names = Vec::with_capacity(names.len());
    let mut layers = Vec::with_capacity(names.len());
    for name in &names {
        let layer_variant = if names.len() == 1 { variant } else { "" };
        let (resource_name, image, offset, base, canvas_size, _) =
            load_cst_image(resources, name, layer_variant, encoding)?;
        resource_names.push(resource_name);
        layers.push(CstImageLayer {
            image,
            position,
            offset,
            base,
            canvas_size,
            opacity: 255,
        });
    }
    Ok((resource_names, layers))
}

fn load_cst_image(
    resources: &ResourceIndex,
    resource: &str,
    variant: &str,
    _encoding: ScriptTextEncoding,
) -> Result<(
    String,
    RgbaFrame,
    [i32; 2],
    [i32; 2],
    [u32; 2],
    BTreeMap<u32, [u32; 5]>,
)> {
    let resource = resource.replace('\\', "/");
    let mut candidates = Vec::new();
    if !variant.is_empty() {
        candidates.push(format!("{resource}_{variant}.hg3"));
        candidates.push(format!("{resource}{variant}.hg3"));
    }
    if resource.to_ascii_lowercase().ends_with(".hg3") {
        candidates.push(resource.clone());
    } else {
        candidates.push(format!("{resource}.hg3"));
    }
    if !variant.is_empty() {
        candidates.push(format!("{resource}s.hg3"));
    }
    let (name, bytes) = candidates
        .into_iter()
        .find_map(|candidate| {
            read_entry_with_fallback(resources, &candidate)
                .ok()
                .map(|bytes| (candidate, bytes))
        })
        .context("CST image resource was not found")?;
    let file = HgFile::parse(&bytes).context("parse CST HG image")?;
    let frame_index = if variant.is_empty() {
        0
    } else {
        parse_hg_frame_id(variant).unwrap_or(0)
    };
    let frame = file
        .frames
        .get(frame_index)
        .or_else(|| file.frames.first())
        .context("CST HG image has no frames")?;
    let pixels = frame
        .decode_pixels_rgba8(&bytes)
        .map_err(|error| anyhow::anyhow!(error))?;
    let image = RgbaFrame::from_hg_rgba8_with_attributes(
        frame.width,
        frame.height,
        &pixels,
        FrameMetadata {
            offset: frame.offset,
            base: frame.base,
            canvas_size: frame.canvas_size,
            alpha_flag: frame.is_transparent,
            ..FrameMetadata::raster([frame.width, frame.height])
        },
        HgFrameAttributes::new(
            frame
                .attributes
                .iter()
                .filter_map(|attribute| {
                    u16::try_from(attribute.id)
                        .ok()
                        .map(|id| (id, attribute.values))
                })
                .collect(),
        ),
    )
    .map_err(|error| anyhow::anyhow!(error))?;
    let attributes = frame
        .attributes
        .iter()
        .map(|attribute| (attribute.id, attribute.values))
        .collect();
    Ok((
        name,
        image,
        frame.offset,
        frame.base,
        frame.canvas_size,
        attributes,
    ))
}

fn save_root_for(signature: &str) -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("CatSystem2Simulator")
        .join(signature)
        .join("save")
}

fn save_path(root: &Path, name: &str) -> Result<PathBuf> {
    let name = name.replace('\\', "/");
    let relative = Path::new(&name);
    if name.is_empty() || name.contains('\0') || name.contains(':')
        || relative.is_absolute()
        || relative.components().any(|part| !matches!(part, std::path::Component::Normal(_)))
    {
        bail!("invalid save file name");
    }
    let path = root.join(relative);
    // Existing parent components may be links created outside the player.
    // Reject those before a write; keep the caller's own save-root binding.
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => bail!("save path contains a symbolic link"),
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => return Err(error).context("inspect save path"),
        }
    }
    Ok(path)
}

fn read_save_css(root: &Path, name: &str) -> Result<Vec<u8>> {
    let path = save_path(root, name)?;
    fs::read(&path).with_context(|| format!("read save file {}", path.display()))
}

fn write_save_css(root: &Path, name: &str, data: &[u8]) -> Result<()> {
    let path = save_path(root, name)?;
    fs::create_dir_all(root)
        .with_context(|| format!("create save directory {}", root.display()))?;
    fs::write(&path, data).with_context(|| format!("write save file {}", path.display()))
}

fn read_u32(bytes: &[u8], offset: usize) -> cs2_script::Result<u32> {
    bytes
        .get(offset..offset.saturating_add(4))
        .and_then(|slice| slice.try_into().ok())
        .map(u32::from_le_bytes)
        .ok_or_else(|| audio_script_error("audio native argument is truncated"))
}

fn require_particle_args(
    arguments: &[u8],
    required: usize,
    message: &'static str,
) -> cs2_script::Result<()> {
    if arguments.len() < required {
        return Err(particle_script_error(message));
    }
    Ok(())
}

fn particle_script_error(message: &'static str) -> cs2_script::ScriptError {
    cs2_script::ScriptError::InvalidStructuredText { offset: 0, message }
}

fn require_audio_args(
    arguments: &[u8],
    required: usize,
    message: &'static str,
) -> cs2_script::Result<()> {
    if arguments.len() < required {
        return Err(audio_script_error(message));
    }
    Ok(())
}

fn audio_script_error(message: &'static str) -> cs2_script::ScriptError {
    cs2_script::ScriptError::InvalidStructuredText { offset: 0, message }
}

fn save_script_error(message: &'static str) -> cs2_script::ScriptError {
    cs2_script::ScriptError::InvalidStructuredText { offset: 0, message }
}

enum CstTableLayout {
    Legacy,
    Current { include_data: bool },
}

fn serialize_cst_table(
    entries: &[CstRegistration],
    include_data: bool,
) -> cs2_script::Result<Vec<u8>> {
    serialize_cst_table_with_layout(entries, CstTableLayout::Current { include_data })
}

fn serialize_cst_table_with_layout(
    entries: &[CstRegistration],
    layout: CstTableLayout,
) -> cs2_script::Result<Vec<u8>> {
    if entries.len() > 1_000_000 {
        return Err(save_script_error("CST table exceeds one million entries"));
    }
    let (mut output, header_length, include_data) = match layout {
        CstTableLayout::Legacy => (Vec::new(), 40, false),
        CstTableLayout::Current { include_data } => (vec![0, 0, 0, 0], 48, include_data),
    };
    for entry in entries {
        if !entry.name.contains(&0) {
            return Err(save_script_error("CST table name has no terminator"));
        }
        let attachment_count = u32::try_from(entry.attachments.len())
            .map_err(|_| save_script_error("CST attachment count exceeds DWORD"))?;
        let attachment_bytes = entry
            .attachments
            .len()
            .checked_mul(72)
            .ok_or_else(|| save_script_error("CST attachment size overflows"))?;
        let data_offset = if include_data && !entry.data.is_empty() {
            48usize
                .checked_add(attachment_bytes)
                .ok_or_else(|| save_script_error("CST data offset overflows"))?
        } else {
            0
        };
        let mut header = [0u8; 48];
        output.len().checked_add(header_length)
            .and_then(|length| length.checked_add(attachment_bytes))
            .and_then(|length| length.checked_add(if include_data { entry.data.len() } else { 0 }))
            .filter(|&length| length <= 64 * 1024 * 1024)
            .ok_or_else(|| save_script_error("CST table exceeds 64 MiB"))?;
        header[0..32].copy_from_slice(&entry.name);
        header[32..36].copy_from_slice(&entry.value.to_le_bytes());
        header[36..40].copy_from_slice(&attachment_count.to_le_bytes());
        let data_len = if include_data {
            u32::try_from(entry.data.len())
                .map_err(|_| save_script_error("CST data length exceeds DWORD"))?
        } else {
            0
        };
        header[40..44].copy_from_slice(&data_len.to_le_bytes());
        header[44..48].copy_from_slice(
            &u32::try_from(data_offset)
                .map_err(|_| save_script_error("CST data offset exceeds DWORD"))?
                .to_le_bytes(),
        );
        output.extend_from_slice(&header[..header_length]);
        for attachment in &entry.attachments {
            output.extend_from_slice(attachment);
        }
        if include_data && !entry.data.is_empty() {
            output.extend_from_slice(&entry.data);
        }
    }
    Ok(output)
}

fn parse_cst_table(bytes: &[u8]) -> cs2_script::Result<Vec<CstRegistration>> {
    if bytes.len() > 64 * 1024 * 1024 {
        return Err(save_script_error("CST table exceeds 64 MiB"));
    }
    if bytes.len() < 4 {
        return Err(save_script_error("CST table header is truncated"));
    }
    let current_format = bytes[0..4] == [0, 0, 0, 0];
    let mut cursor = if current_format { 4usize } else { 0usize };
    let mut entries = Vec::new();
    while cursor < bytes.len() {
        if entries.len() >= 1_000_000 {
            return Err(save_script_error("CST table exceeds one million entries"));
        }
        let header_length = if current_format { 48 } else { 40 };
        let header_end = cursor
            .checked_add(header_length)
            .ok_or_else(|| save_script_error("CST table header overflows"))?;
        let header = bytes
            .get(cursor..header_end)
            .ok_or_else(|| save_script_error("CST table header is truncated"))?;
        let mut name = [0u8; 32];
        name.copy_from_slice(&header[..32]);
        if !name.contains(&0) {
            return Err(save_script_error("CST table name has no terminator"));
        }
        let value = u32::from_le_bytes(header[32..36].try_into().unwrap());
        let attachment_count = u32::from_le_bytes(header[36..40].try_into().unwrap()) as usize;
        let (data_len, data_offset) = if current_format {
            (
                u32::from_le_bytes(header[40..44].try_into().unwrap()) as usize,
                u32::from_le_bytes(header[44..48].try_into().unwrap()) as usize,
            )
        } else {
            (0, 0)
        };
        cursor = header_end;
        let attachment_bytes = attachment_count
            .checked_mul(72)
            .ok_or_else(|| save_script_error("CST attachment count overflows"))?;
        let attachments_end = cursor
            .checked_add(attachment_bytes)
            .ok_or_else(|| save_script_error("CST attachment range overflows"))?;
        if attachments_end > bytes.len() {
            return Err(save_script_error("CST attachments are truncated"));
        }
        let mut attachments = Vec::with_capacity(attachment_count);
        for chunk in bytes[cursor..attachments_end].chunks_exact(72) {
            let mut attachment = [0u8; 72];
            attachment.copy_from_slice(chunk);
            attachments.push(attachment);
        }
        cursor = attachments_end;
        // 0x68b309..0x68b31a advances by physical header + records + data
        // length. The source pointer for data is independently item-relative;
        // it may point into another in-bounds part of the container.
        let next = cursor.checked_add(data_len).filter(|&next| next <= bytes.len())
            .ok_or_else(|| save_script_error("CST table physical data span is truncated"))?;
        let data = if data_len == 0 {
            Vec::new()
        } else {
            if !current_format || data_offset > bytes.len() {
                return Err(save_script_error("CST data offset is invalid"));
            }
            let start = header_end
                .checked_sub(header_length)
                .and_then(|start| start.checked_add(data_offset))
                .ok_or_else(|| save_script_error("CST data range overflows"))?;
            let end = start
                .checked_add(data_len)
                .ok_or_else(|| save_script_error("CST data range overflows"))?;
            let data = bytes
                .get(start..end)
                .ok_or_else(|| save_script_error("CST data is truncated"))?
                .to_vec();
            data
        };
        cursor = next;
        entries.push(CstRegistration {
            name,
            value,
            attachments,
            data,
        });
    }
    Ok(entries)
}

fn trim_nul(bytes: &[u8]) -> &[u8] {
    &bytes[..bytes
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(bytes.len())]
}

fn has_extension(bytes: &[u8]) -> bool {
    bytes
        .rsplit(|&byte| byte == b'/' || byte == b'\\')
        .next()
        .is_some_and(|name| name.contains(&b'.'))
}

fn sound_audio_channel(resource: &str) -> AudioChannel {
    let lower = resource.to_ascii_lowercase();
    if lower.contains("/bgm") || lower.starts_with("bgm") {
        AudioChannel::Bgm
    } else if lower.contains("voice") || lower.contains("sysvoice") {
        AudioChannel::Voice
    } else {
        AudioChannel::SoundEffect
    }
}

fn discover_start_entry(resources: &ResourceIndex) -> Option<String> {
    for name in [
        "config/startup.xml",
        "startup.xml",
        "system/config/startup.xml",
        "system/startup.xml",
    ] {
        let Ok(bytes) = resources.read(name) else {
            continue;
        };
        let Ok(config) = ConfigXml::parse_runtime(&bytes) else {
            continue;
        };
        let Some(start) = config.text(&[b"document", b"SCRIPT", b"start"]) else {
            continue;
        };
        let start = String::from_utf8_lossy(start).replace('\\', "/");
        if !start.is_empty() {
            return Some(start);
        }
    }
    for fallback in [
        "main.kcs",
        "system/main.kcs",
        "start.kcs",
        "system/start.kcs",
    ] {
        if resources.find(fallback).is_some() {
            return Some(fallback.to_string());
        }
    }
    None
}

fn discover_screen_size(resources: &ResourceIndex) -> Result<[u32; 2]> {
    for name in [
        "config/startup.xml", "startup.xml", "system/config/startup.xml", "system/startup.xml",
    ] {
        let Ok(bytes) = resources.read(name) else { continue };
        let Ok(config) = ConfigXml::parse_runtime(&bytes) else { continue };
        let width = config.integer(&[b"document", b"SCREEN", b"width"], 800);
        let height = config.integer(&[b"document", b"SCREEN", b"height"], 600);
        let width = config.integer(&[b"document", b"SCREEN", b"v_width"], width);
        let height = config.integer(&[b"document", b"SCREEN", b"v_height"], height);
        if width <= 0 || height <= 0 ||
            (width as u64) * (height as u64) * 4 > 64 * 1024 * 1024 {
            bail!("invalid logical canvas dimensions in {name}: {width}x{height}");
        }
        return Ok([width as u32, height as u32]);
    }
    Ok([800, 600])
}

fn discover_heap_size(resources: &ResourceIndex) -> usize {
    for name in [
        "config/startup.xml",
        "startup.xml",
        "system/config/startup.xml",
        "system/startup.xml",
    ] {
        let Ok(bytes) = resources.read(name) else {
            continue;
        };
        let Ok(config) = ConfigXml::parse_runtime(&bytes) else {
            continue;
        };
        let value = config.integer(&[b"document", b"ETC", b"heap"], 0);
        if value > 0 {
            return usize::try_from(value).unwrap_or(0);
        }
    }
    0
}

fn discover_shared_slots(resources: &ResourceIndex) -> Option<usize> {
    for name in [
        "config/startup.xml",
        "startup.xml",
        "system/config/startup.xml",
        "system/startup.xml",
    ] {
        let Ok(bytes) = resources.read(name) else { continue };
        let Ok(config) = ConfigXml::parse_runtime(&bytes) else { continue };
        let value = config.integer(&[b"document", b"GAME", b"flag", b"max"], -1);
        if value >= 0 {
            return usize::try_from(value).ok();
        }
    }
    None
}

fn discover_auto_select(resources: &ResourceIndex) -> u32 {
    for name in [
        "config/startup.xml",
        "startup.xml",
        "system/config/startup.xml",
        "system/startup.xml",
    ] {
        let Ok(bytes) = resources.read(name) else {
            continue;
        };
        let Ok(config) = ConfigXml::parse_runtime(&bytes) else {
            continue;
        };
        let value = config.integer(&[b"document", b"GAME", b"autoselect"], i32::MIN);
        if value != i32::MIN {
            return value as u32;
        }
    }
    0
}

fn discover_sound_groups(resources: &ResourceIndex) -> (Vec<SoundGroup>, u32) {
    let mut sound_conf_candidates = vec![
        "config/soundconf.xml".to_owned(),
        "soundconf.xml".to_owned(),
    ];
    for startup in ["config/startup.xml", "startup.xml"] {
        let Ok(bytes) = resources.read(startup) else {
            continue;
        };
        let Ok(config) = ConfigXml::parse_runtime(&bytes) else {
            continue;
        };
        let Some(name) = config.text(&[b"document", b"SOUNDCONFIG"]) else {
            continue;
        };
        let name = String::from_utf8_lossy(name).replace('\\', "/");
        if name.is_empty() {
            continue;
        }
        sound_conf_candidates.push(name.clone());
        if !name.contains('/') {
            let parent = Path::new(startup).parent().unwrap_or_else(|| Path::new(""));
            sound_conf_candidates.push(parent.join(&name).to_string_lossy().replace('\\', "/"));
        }
    }
    sound_conf_candidates.sort();
    sound_conf_candidates.dedup();

    for name in sound_conf_candidates {
        let Ok(bytes) = read_entry_with_fallback(resources, &name) else {
            continue;
        };
        let Ok(config) = ConfigXml::parse_runtime(&bytes) else {
            continue;
        };
        let groups = parse_sound_groups(&config, true);
        if !groups.is_empty() {
            return (groups, 1);
        }
    }

    for startup in ["config/startup.xml", "startup.xml"] {
        let Ok(bytes) = resources.read(startup) else {
            continue;
        };
        let Ok(config) = ConfigXml::parse_runtime(&bytes) else {
            continue;
        };
        let groups = parse_sound_groups(&config, false);
        if !groups.is_empty() {
            return (groups, 0);
        }
    }
    (Vec::new(), 0)
}

fn discover_sound_config(resources: &ResourceIndex) -> DesktopSoundConfig {
    let mut candidates = vec![
        "config/soundconf.xml".to_owned(),
        "soundconf.xml".to_owned(),
    ];
    for startup in ["config/startup.xml", "startup.xml"] {
        let Ok(bytes) = resources.read(startup) else {
            continue;
        };
        let Ok(config) = ConfigXml::parse_runtime(&bytes) else {
            continue;
        };
        if let Some(name) = config.text(&[b"document", b"SOUNDCONFIG"]) {
            let name = String::from_utf8_lossy(name).replace('\\', "/");
            if !name.is_empty() {
                candidates.push(name.clone());
                if !name.contains('/') {
                    let parent = Path::new(startup).parent().unwrap_or_else(|| Path::new(""));
                    candidates.push(parent.join(&name).to_string_lossy().replace('\\', "/"));
                }
            }
        }
    }
    candidates.sort();
    candidates.dedup();
    for name in candidates {
        let Ok(bytes) = read_entry_with_fallback(resources, &name) else {
            continue;
        };
        if let Ok(config) = parse_desktop_sound_config(&bytes) {
            return config;
        }
    }
    DesktopSoundConfig::default()
}

fn parse_desktop_sound_config(bytes: &[u8]) -> Result<DesktopSoundConfig> {
    let config = ConfigXml::parse_runtime(bytes).map_err(|_| anyhow::anyhow!("sound XML is malformed"))?;
    let mut root = SoundXmlNode {
        name: b"document".to_vec(),
        text: Vec::new(),
        children: Vec::new(),
    };
    for field in &config.fields {
        if field.path.first().map(Vec::as_slice) != Some(b"document") {
            continue;
        }
        let mut node = &mut root;
        for segment in field.path.iter().skip(1) {
            let index = node
                .children
                .iter()
                .position(|child| child.name.as_slice() == segment.as_slice())
                .unwrap_or_else(|| {
                    node.children.push(SoundXmlNode {
                        name: segment.clone(),
                        text: Vec::new(),
                        children: Vec::new(),
                    });
                    node.children.len() - 1
                });
            node = &mut node.children[index];
        }
        node.text = field.text.clone();
    }
    let master = sound_child(&root, b"MASTER");
    let parts = sound_child(&root, b"PART")
        .map(|part| {
            part.children
                .iter()
                .rev()
                .map(|node| parse_desktop_sound_part(node, 0, None))
                .collect::<Result<Vec<_>>>()
        })
        .transpose()?
        .unwrap_or_default();
    Ok(DesktopSoundConfig {
        source_loaded: true,
        parts,
        master_volume: master.and_then(|node| sound_integer(sound_child(node, b"volume"))),
        master_play_enabled: master.and_then(|node| sound_integer(sound_child(node, b"play"))),
        master_system_voice: master.and_then(|node| sound_integer(sound_child(node, b"sysplay"))),
        master_voice_volume: master.and_then(|node| sound_integer(sound_child(node, b"voicevol"))),
        master_skip_volume: master.and_then(|node| sound_integer(sound_child(node, b"skipvol"))),
        master_mute: master
            .and_then(|node| sound_integer(sound_child(node, b"mute")))
            .unwrap_or(0),
        master_voice_mute: master
            .and_then(|node| sound_integer(sound_child(node, b"voicemute")))
            .unwrap_or(0),
        user_overrides: BTreeMap::new(),
    })
}

fn parse_desktop_sound_part(
    node: &SoundXmlNode,
    depth: usize,
    parent: Option<&DesktopSoundPart>,
) -> Result<DesktopSoundPart> {
    if depth >= 32 {
        bail!("sound configuration nesting exceeds 32 levels")
    }
    let mut part = DesktopSoundPart {
        xml_key: normalize_sound_name(&node.name),
        name: sound_bytes(sound_child(node, b"name")),
        type_name: sound_bytes(sound_child(node, b"type")),
        file_prefix: sound_bytes(sound_child(node, b"head")),
        archive: sound_bytes(sound_child(node, b"int")),
        id: sound_integer(sound_child(node, b"id")).unwrap_or(-1),
        same_id: sound_integer(sound_child(node, b"sameid")).unwrap_or(0),
        master_volume: sound_integer(sound_child(node, b"mastervol")).unwrap_or(-1),
        volume: sound_integer(sound_child(node, b"volume")).unwrap_or(-1),
        voice_volume: sound_integer(sound_child(node, b"voicevol")).unwrap_or(-1),
        skip_volume: sound_integer(sound_child(node, b"skipvol")).unwrap_or(-1),
        system_voice: sound_integer(sound_child(node, b"sysplay")).unwrap_or(-1),
        play_enabled: sound_integer(sound_child(node, b"play")).unwrap_or(-1),
        mute: sound_integer(sound_child(node, b"mute")).unwrap_or(0),
        voice_mute: sound_integer(sound_child(node, b"voicemute")).unwrap_or(0),
        children: Vec::new(),
    };
    if let Some(parent) = parent {
        for (value, inherited) in [
            (&mut part.name, &parent.name),
            (&mut part.type_name, &parent.type_name),
            (&mut part.file_prefix, &parent.file_prefix),
            (&mut part.archive, &parent.archive),
        ] {
            if value.is_empty() {
                value.clone_from(inherited);
            }
        }
        for (value, inherited) in [
            (&mut part.id, parent.id),
            (&mut part.master_volume, parent.master_volume),
            (&mut part.volume, parent.volume),
            (&mut part.voice_volume, parent.voice_volume),
            (&mut part.skip_volume, parent.skip_volume),
            (&mut part.system_voice, parent.system_voice),
            (&mut part.play_enabled, parent.play_enabled),
        ] {
            if *value == -1 {
                *value = inherited;
            }
        }
    }
    if let Some(children) = sound_child(node, b"part") {
        part.children = children
            .children
            .iter()
            .rev()
            .map(|child| parse_desktop_sound_part(child, depth + 1, Some(&part)))
            .collect::<Result<Vec<_>>>()?;
    }
    Ok(part)
}

fn sound_child<'a>(node: &'a SoundXmlNode, name: &[u8]) -> Option<&'a SoundXmlNode> {
    node.children.iter().find(|child| child.name == name)
}

fn sound_bytes(node: Option<&SoundXmlNode>) -> Vec<u8> {
    node.map_or_else(Vec::new, |node| trim_ascii(&node.text).to_vec())
}

fn sound_integer(node: Option<&SoundXmlNode>) -> Option<i32> {
    let bytes = trim_ascii(&node?.text);
    if bytes.is_empty() {
        return Some(0);
    }
    let negative = bytes.first() == Some(&b'-');
    let digits = if matches!(bytes.first(), Some(b'-' | b'+')) {
        &bytes[1..]
    } else {
        bytes
    };
    let (digits, radix) = if digits.starts_with(b"0x") || digits.starts_with(b"0X") {
        (&digits[2..], 16_u32)
    } else {
        (digits, 10_u32)
    };
    let value = digits
        .iter()
        .take_while(|byte| {
            if radix == 16 {
                byte.is_ascii_hexdigit()
            } else {
                byte.is_ascii_digit()
            }
        })
        .fold(0_u32, |value, byte| {
            let digit = if byte.is_ascii_digit() {
                u32::from(byte - b'0')
            } else {
                u32::from(byte.to_ascii_lowercase() - b'a' + 10)
            };
            value.wrapping_mul(radix).wrapping_add(digit)
        });
    Some(if negative {
        value.wrapping_neg() as i32
    } else {
        value as i32
    })
}

fn normalize_sound_name(name: &[u8]) -> Vec<u8> {
    let name = trim_nul(name);
    let mut normalized = name.to_vec();
    let mut cursor = 0;
    while cursor < normalized.len() {
        if matches!(normalized[cursor], 0x81..=0x9f | 0xe0..=0xfc) && cursor + 1 < normalized.len()
        {
            cursor += 2;
        } else {
            normalized[cursor].make_ascii_uppercase();
            cursor += 1;
        }
    }
    normalized
}

fn parse_sound_groups(config: &ConfigXml, keyed_by_id: bool) -> Vec<SoundGroup> {
    let mut groups = Vec::new();
    for position in 0..32 {
        let index = if keyed_by_id { 31 - position } else { position };
        let entry = format!("group{index:02}");
        let prefix = format!("document/SOUNDGROUP/{entry}");
        let field = |name: &str| format!("{prefix}/{name}");
        let Some(id) = xml_integer(config, &field("id")) else {
            continue;
        };
        if keyed_by_id && groups.iter().any(|group: &SoundGroup| group.key == id) {
            continue;
        }
        let label = xml_text(config, &field("label"))
            .or_else(|| xml_text(config, &field("type")))
            .unwrap_or_default()
            .to_vec();
        let min = xml_integer(config, &field("min")).unwrap_or(u32::MAX) as i32;
        let max = xml_integer(config, &field("max")).unwrap_or(u32::MAX) as i32;
        groups.push(SoundGroup {
            key: if keyed_by_id { id } else { index as u32 },
            id,
            label,
            range: [min, max],
        });
    }
    groups
}

fn xml_path(path: &str) -> Vec<&[u8]> {
    path.trim_start_matches('/')
        .split('/')
        .filter(|part| !part.is_empty())
        .map(str::as_bytes)
        .collect()
}

fn xml_text<'a>(document: &'a ConfigXml, path: &str) -> Option<&'a [u8]> {
    let path = xml_path(path);
    document
        .fields
        .iter()
        .find(|field| {
            let skip_root = document.root == b"document"
                && path.first().is_none_or(|part| *part != b"document");
            field.path.iter().skip(usize::from(skip_root)).map(Vec::as_slice)
                .eq(path.iter().copied())
        })
        .map(|field| field.text.as_slice())
}

fn xml_integer(document: &ConfigXml, path: &str) -> Option<u32> {
    let text = xml_text(document, path)?;
    Some(xml_dword(text))
}

fn xml_dword(text: &[u8]) -> u32 {
    let text = text.trim_ascii();
    let negative = text.first() == Some(&b'-');
    let digits = if matches!(text.first(), Some(b'-' | b'+')) {
        &text[1..]
    } else {
        text
    };
    let (digits, radix) = if digits.starts_with(b"0x") || digits.starts_with(b"0X") {
        (&digits[2..], 16_u32)
    } else {
        (digits, 10_u32)
    };
    let value = digits
        .iter()
        .take_while(|byte| match radix {
            16 => byte.is_ascii_hexdigit(),
            _ => byte.is_ascii_digit(),
        })
        .fold(0_u32, |value, byte| {
            let byte = *byte;
            let digit = if byte.is_ascii_digit() {
                u32::from(byte - b'0')
            } else {
                u32::from(byte.to_ascii_lowercase() - b'a' + 10)
            };
            value.wrapping_mul(radix).wrapping_add(digit)
        });
    if negative {
        value.wrapping_neg()
    } else {
        value
    }
}

fn expand_kcs_format(vm: &Vm, input: &[u8], depth: usize) -> cs2_script::Result<Vec<u8>> {
    expand_kcs_format_inner(vm, input, depth, false)
}

#[test]
fn kcs_debug_format_keeps_literal_percent_without_hiding_address_errors() {
    let vm = Vm::new();
    let input = b"diagnostic %name (100%%, 92%)";
    assert_eq!(
        expand_kcs_format_inner(&vm, input, 0, true).unwrap(),
        b"diagnostic %name (100%, 92%)",
    );
    assert!(expand_kcs_format(&vm, input, 0).is_err());
    assert!(expand_kcs_format_inner(&vm, b"%s[G42", 0, true).is_err());
}

fn expand_kcs_format_inner(
    vm: &Vm, input: &[u8], depth: usize, allow_literal_percent: bool,
) -> cs2_script::Result<Vec<u8>> {
    if depth >= 32 {
        return Err(cs2_script::ScriptError::InvalidStructuredText {
            offset: 0,
            message: "KCS format recursion exceeds limit",
        });
    }
    let mut output = Vec::new();
    let mut cursor = 0;
    while cursor < input.len() {
        if input[cursor] != b'%' {
            output.push(input[cursor]);
            cursor += 1;
            continue;
        }
        let percent_start = cursor;
        cursor += 1;
        if input.get(cursor) == Some(&b'%') {
            output.push(b'%');
            cursor += 1;
            continue;
        }
        let zero_pad = input.get(cursor) == Some(&b'0');
        if zero_pad {
            cursor += 1;
        }
        let width_start = cursor;
        while input.get(cursor).is_some_and(u8::is_ascii_digit) {
            cursor += 1;
        }
        let width = parse_decimal(&input[width_start..cursor]).unwrap_or(0);
        let precision = if input.get(cursor) == Some(&b'.') {
            cursor += 1;
            let start = cursor;
            while input.get(cursor).is_some_and(u8::is_ascii_digit) {
                cursor += 1;
            }
            Some(parse_decimal(&input[start..cursor]).unwrap_or(0))
        } else {
            None
        };
        if allow_literal_percent && input.get(cursor + 1) != Some(&b'[') {
            output.push(b'%');
            cursor = percent_start + 1;
            continue;
        }
        let kind = *input
            .get(cursor)
            .ok_or(cs2_script::ScriptError::InvalidStructuredText {
                offset: cursor,
                message: "KCS format conversion is missing",
            })?;
        cursor += 1;
        if input.get(cursor) != Some(&b'[') {
            return Err(cs2_script::ScriptError::InvalidStructuredText {
                offset: cursor,
                message: "KCS format address is missing",
            });
        }
        cursor += 1;
        let domain = *input
            .get(cursor)
            .ok_or(cs2_script::ScriptError::InvalidStructuredText {
                offset: cursor,
                message: "KCS format address domain is missing",
            })?;
        cursor += 1;
        let address_start = cursor;
        while input.get(cursor).is_some_and(u8::is_ascii_digit) {
            cursor += 1;
        }
        if input.get(cursor) != Some(&b']') {
            return Err(cs2_script::ScriptError::InvalidStructuredText {
                offset: cursor,
                message: "KCS format address is malformed",
            });
        }
        let relative = parse_decimal(&input[address_start..cursor]).unwrap_or(0);
        cursor += 1;
        let relative = relative as u32;
        let address = match domain {
            b'G' => relative,
            b'L' => (vm.frame_base() as u32).wrapping_add(relative),
            b'H' => 0x8000_0000_u32.wrapping_add(relative),
            _ => {
                return Err(cs2_script::ScriptError::InvalidStructuredText {
                    offset: address_start,
                    message: "unsupported KCS format address domain",
                });
            }
        };
        let rendered = match kind {
            b's' => {
                let source = vm.read_string(address)?;
                let mut text = expand_kcs_format_inner(vm, &source, depth + 1, allow_literal_percent)?;
                if let Some(limit) = precision {
                    text.truncate(limit);
                }
                text
            }
            b'c' => vec![vm.read(address, 1)?[0]],
            b'f' => {
                let value = f32::from_bits(vm.read_u32(address)?);
                let text = match precision {
                    Some(precision) => format!("{value:.precision$}"),
                    None => format!("{value:.6}"),
                };
                text.into_bytes()
            }
            b'd' | b'i' => {
                let value = vm.read_u32(address)? as i32;
                format!("{value}").into_bytes()
            }
            b'u' => format!("{}", vm.read_u32(address)?).into_bytes(),
            b'x' => format!("{:x}", vm.read_u32(address)?).into_bytes(),
            b'X' => format!("{:X}", vm.read_u32(address)?).into_bytes(),
            _ => {
                return Err(cs2_script::ScriptError::InvalidStructuredText {
                    offset: cursor,
                    message: "unsupported KCS format conversion",
                });
            }
        };
        append_width(&mut output, &rendered, width, zero_pad);
        if output.len() > 64 * 1024 {
            return Err(cs2_script::ScriptError::InvalidStructuredText {
                offset: cursor,
                message: "KCS formatted output exceeds limit",
            });
        }
    }
    Ok(output)
}

fn parse_decimal(bytes: &[u8]) -> Option<usize> {
    if bytes.is_empty() {
        return Some(0);
    }
    bytes.iter().try_fold(0_usize, |value, byte| {
        value.checked_mul(10)?.checked_add(usize::from(byte - b'0'))
    })
}

fn append_width(output: &mut Vec<u8>, value: &[u8], width: usize, zero_pad: bool) {
    let padding = width.saturating_sub(value.len()).min(4096);
    if zero_pad && value.first() == Some(&b'-') {
        output.push(b'-');
        output.extend(std::iter::repeat_n(b'0', padding));
        output.extend_from_slice(&value[1..]);
    } else if zero_pad {
        output.extend(std::iter::repeat_n(b'0', padding));
        output.extend_from_slice(value);
    } else {
        output.extend(std::iter::repeat_n(b' ', padding));
        output.extend_from_slice(value);
    }
}

fn compare_kcs_strings(left: &[u8], right: &[u8]) -> i32 {
    for index in 0..=left.len().min(right.len()) {
        let left_byte = left.get(index).copied().unwrap_or(0);
        let right_byte = right.get(index).copied().unwrap_or(0);
        if left_byte != right_byte {
            return left_byte as i8 as i32 - right_byte as i8 as i32;
        }
    }
    0
}

fn compare_kcs_strings_ascii_insensitive(left: &[u8], right: &[u8]) -> i32 {
    for index in 0..=left.len().min(right.len()) {
        let left_byte = left.get(index).copied().unwrap_or(0).to_ascii_lowercase();
        let right_byte = right.get(index).copied().unwrap_or(0).to_ascii_lowercase();
        if left_byte != right_byte {
            return i32::from(left_byte) - i32::from(right_byte);
        }
    }
    0
}

fn write_kcs_terminated(
    vm: &mut Vm,
    target: u32,
    value: &[u8],
    terminators: usize,
) -> cs2_script::Result<()> {
    let mut output = Vec::with_capacity(value.len() + terminators);
    output.extend_from_slice(value);
    output.resize(output.len() + terminators, 0);
    vm.write(target, &output)
}

fn native_state_error(message: &'static str) -> cs2_script::ScriptError {
    cs2_script::ScriptError::InvalidStructuredText { offset: 0, message }
}

fn native_state_error_owned(message: impl Into<String>) -> cs2_script::ScriptError {
    cs2_script::ScriptError::HostCallFailed { message: message.into() }
}

fn motion_property_groups() -> [(usize, usize); 9] {
    [
        (0, 3),
        (3, 3),
        (6, 3),
        (9, 16),
        (25, 3),
        (28, 1),
        (29, 3),
        (32, 3),
        (35, 1),
    ]
}

fn read_motion_properties(vm: &Vm, arguments: &[u32]) -> cs2_script::Result<[u32; 36]> {
    let mut properties = [0_u32; 36];
    for (group, (start, length)) in motion_property_groups().into_iter().enumerate() {
        if group == 5 || group == 8 {
            properties[start] = arguments[group];
            continue;
        }
        let bytes = vm.read(arguments[group], length * 4)?;
        for (target, bytes) in properties[start..start + length]
            .iter_mut()
            .zip(bytes.chunks_exact(4))
        {
            *target = u32::from_le_bytes(bytes.try_into().unwrap());
        }
    }
    Ok(properties)
}

fn serialize_motion_snapshot(
    controller: &MotionControllerState,
    now: u32,
) -> cs2_script::Result<Vec<u8>> {
    let mut output = vec![0_u8; 8];
    for task in controller.tasks.values() {
        let name = encoding_rs::SHIFT_JIS.decode(&task.name).0;
        let name = name.encode_utf16().collect::<Vec<_>>();
        if name.len() >= 32 {
            return Err(native_state_error(
                "motion snapshot name exceeds 31 UTF-16 units",
            ));
        }
        let size = 0x454usize
            .checked_add(task.saved_data.len())
            .ok_or_else(|| native_state_error("motion snapshot size overflows"))?;
        let end = output
            .len()
            .checked_add(size)
            .filter(|end| *end <= 64 * 1024 * 1024)
            .ok_or_else(|| native_state_error("motion snapshot exceeds storage limit"))?;
        let start = output.len();
        output.resize(end, 0);
        let record = &mut output[start..end];
        record[..4].copy_from_slice(&(size as u32).to_le_bytes());
        for (index, unit) in name.iter().enumerate() {
            record[4 + index * 2..6 + index * 2].copy_from_slice(&unit.to_le_bytes());
        }
        record[0x44..0x48].copy_from_slice(&(task.group as u32).to_le_bytes());
        for (target, value) in record[0x48..0x448].chunks_exact_mut(4).zip(task.parameters) {
            target.copy_from_slice(&value.to_le_bytes());
        }
        record[0x448..0x44c].copy_from_slice(&(task.saved_data.len() as u32).to_le_bytes());
        record[0x44c..0x450].copy_from_slice(&now.wrapping_sub(task.origin).to_le_bytes());
        record[0x450..0x454].copy_from_slice(&task.snapshot_flags.to_le_bytes());
        record[0x454..].copy_from_slice(&task.saved_data);
    }
    let output_size = output.len() as u32;
    output[..4].copy_from_slice(&output_size.to_le_bytes());
    output[4..8].copy_from_slice(&(controller.tasks.len() as u32).to_le_bytes());
    Ok(output)
}

fn snapshot_word(bytes: &[u8], offset: usize) -> cs2_script::Result<u32> {
    bytes
        .get(offset..offset.saturating_add(4))
        .and_then(|value| value.try_into().ok())
        .map(u32::from_le_bytes)
        .ok_or_else(|| native_state_error("motion snapshot DWORD is truncated"))
}

fn decode_motion_snapshot_name(record: &[u8]) -> cs2_script::Result<Vec<u8>> {
    let mut units = Vec::new();
    for chunk in record
        .get(4..0x44)
        .ok_or_else(|| native_state_error("motion snapshot name field is truncated"))?
        .chunks_exact(2)
    {
        let unit = u16::from_le_bytes(chunk.try_into().unwrap());
        if unit == 0 {
            break;
        }
        units.push(unit);
    }
    if units.len() == 32 {
        return Err(native_state_error(
            "motion snapshot name has no terminating UTF-16 unit",
        ));
    }
    let name = String::from_utf16(&units)
        .map_err(|_| native_state_error("motion snapshot name is invalid UTF-16"))?;
    let (encoded, _, had_errors) = encoding_rs::SHIFT_JIS.encode(&name);
    if had_errors {
        return Err(native_state_error(
            "motion snapshot name cannot be encoded as CP932",
        ));
    }
    Ok(encoded.into_owned())
}

fn load_motion_program(
    resources: &ResourceIndex,
    name: &[u8],
) -> cs2_script::Result<Option<KcsProgram>> {
    let name = ScriptTextEncoding::Cp932.decode(trim_nul(name));
    if name.is_empty() {
        return Ok(None);
    }
    let resolved = resources.category_names(b"mot", &name)
        .map_err(|_| native_state_error("motion restore category resolution failed"))?
        .into_iter().next().ok_or_else(|| native_state_error("motion restore category resolved no name"))?;
    let Some(bytes) = resources.read_mode(&resolved, cs2_runtime::resource_router::ResourceMode::LooseFirst)
        .map_err(|_| native_state_error("motion restore resource routing/read failed"))? else {
        return Ok(None);
    };
    KcsProgram::parse(&bytes)
        .map(Some)
        .map_err(|_| native_state_error("motion resource is not a valid KCS program"))
}

fn create_motion_runner(
    program: KcsProgram,
    name: &[u8],
    parameters: &[u32; 256],
    saved_data: &[u8],
    external_value: u32,
    properties: [u32; 36],
    snapshot_flags: u32,
) -> cs2_script::Result<motion_vm::MotionRunner> {
    motion_vm::MotionRunner::from_program(
        motion_vm::MotionProgram::from_program(program)?,
        motion_vm::MotionStart {
            name,
            parameters: *parameters,
            external_value,
            properties,
            base_time: 0,
            saved_data,
            snapshot_flags,
        },
    )
}

struct DesktopMotionServices<'a> {
    random: &'a mut ScriptRandom,
    events: &'a mut Vec<LogicEvent>,
    frame: u64,
}

impl motion_vm::MotionServices for DesktopMotionServices<'_> {
    fn random_integer(&mut self) -> u32 { self.random.next_integer() }
    fn random_float_bits(&mut self) -> u32 { self.random.next_float_bits() }
    fn debug(&mut self, text: &[u8]) -> cs2_script::Result<()> {
        self.events.push(LogicEvent {
            frame: self.frame,
            kind: "motion-debug".into(),
            detail: encoding_rs::SHIFT_JIS.decode(text).0.into_owned(),
        });
        Ok(())
    }
}

fn advance_motion_controller(
    controller: &mut MotionControllerState,
    now: u32,
    skip_unstarted: bool,
    finishing: bool,
    services: &mut impl motion_vm::MotionServices,
    budget: usize,
) -> cs2_script::Result<NativeReturn> {
    if controller.pending.as_ref().is_some_and(|pending| pending.finishing != finishing) {
        return Err(native_state_error("motion controller has a different pending invocation"));
    }
    let mut update = controller.pending.take().unwrap_or_else(|| {
        controller.properties = controller.base_properties;
        MotionControllerUpdate {
            now, skip_unstarted, finishing,
            ids: controller.tasks.keys().copied().collect(),
            cursor: 0, updated: false, speed_bits: controller.speed_bits, step_done: false,
        }
    });
    while let Some(&id) = update.ids.get(update.cursor) {
        let Some(task) = controller.tasks.get_mut(&id) else {
            update.cursor += 1;
            continue;
        };
        if !update.step_done {
        let progress = (|| -> cs2_script::Result<Option<motion_vm::MotionProgress>> {
        if !task.runner.is_pending() {
            if !update.finishing && update.now != u32::MAX && task.origin == 0 {
                if update.skip_unstarted {
                    update.cursor += 1;
                    return Ok(None);
                }
                task.origin = update.now;
            }
            task.runner.set_properties(&controller.properties)?;
            task.runner.set_parameters(&task.parameters)?;
            if task.finishing != 0 { task.runner.request_finish()?; }
        }
        let time = if update.now == u32::MAX { u32::MAX } else {
            motion_vm::controller_time(update.now, task.origin, update.speed_bits)
        };
        let progress = if update.finishing {
            task.runner.finish(services, budget)?
        } else {
            task.runner.update(time, services, budget)?
        };
        Ok(Some(progress))
        })();
        let progress = match progress {
            Ok(Some(progress)) => progress,
            Ok(None) => continue,
            Err(error) => {
                controller.pending = Some(update);
                return Err(error);
            }
        };
        if progress == motion_vm::MotionProgress::Yield {
            controller.pending = Some(update);
            return Ok(NativeReturn::Retry);
        }
        if progress == motion_vm::MotionProgress::NotStarted {
            update.cursor += 1;
            continue;
        }
        update.step_done = true;
        }
        let commit = (|| -> cs2_script::Result<()> {
            let state = task.runner.state()?;
            controller.properties = state.properties;
            task.parameters = state.parameters;
            task.elapsed = state.elapsed;
            task.running = state.running as i32;
            task.finishing = state.finishing as i32;
            task.external_value = state.external_value;
            let time = if update.now == u32::MAX { u32::MAX } else {
                motion_vm::controller_time(update.now, task.origin, update.speed_bits)
            };
            task.saved_data = task.runner.snapshot(time, task.group)?.saved_data;
            update.updated = true;
            Ok(())
        })();
        if let Err(error) = commit {
            controller.pending = Some(update);
            return Err(error);
        }
        update.cursor += 1;
        update.step_done = false;
    }
    Ok(NativeReturn::Value(u32::from(update.updated)))
}

fn signed_cst_index(value: u32, current: i32) -> i32 {
    if (value as i32) < 0 {
        current
    } else {
        value as i32
    }
}

fn normalize_cst_name(name: &mut Vec<u8>) {
    name.truncate(
        name.iter()
            .position(|&byte| byte == 0)
            .unwrap_or(name.len()),
    );
    if let Some(dot) = name.iter().rposition(|byte| *byte == b'.').filter(|index| {
        name[..*index]
            .iter()
            .rposition(|byte| matches!(*byte, b'/' | b'\\'))
            .is_none_or(|slash| slash < *index)
    }) {
        name.truncate(dot);
    }
}

fn cst_name_matches(stored: &[u8; 32], requested: &[u8]) -> bool {
    let stored = trim_nul(stored);
    stored.is_empty() || stored.eq_ignore_ascii_case(requested)
}

fn cst_detail_set(entry: &mut CstRegistration, position: u32, id: u32, value: &[u8]) {
    let offset = entry.attachments.iter().position(|record| {
        u32::from_le_bytes(record[..4].try_into().unwrap()) == position
            && u32::from_le_bytes(record[4..8].try_into().unwrap()) == id
    });
    let index = offset.unwrap_or_else(|| {
        entry.attachments.push([0; 72]);
        entry.attachments.len() - 1
    });
    let record = &mut entry.attachments[index];
    record[..4].copy_from_slice(&position.to_le_bytes());
    record[4..8].copy_from_slice(&id.to_le_bytes());
    let value = trim_nul(value);
    let length = value.len().min(62);
    record[8..8 + length].copy_from_slice(&value[..length]);
    record[8 + length] = 0;
    if value.len() > 62 {
        record[8 + length + 1] = 0;
    }
}

fn cst_detail_get(
    entry: &CstRegistration, position: u32, id: u32,
) -> cs2_script::Result<Option<Vec<u8>>> {
    let exact = entry.attachments.iter().find(|record| {
        u32::from_le_bytes(record[..4].try_into().unwrap()) == position
            && u32::from_le_bytes(record[4..8].try_into().unwrap()) == id
    });
    let record = exact.or_else(|| {
        let previous = entry.attachments.iter()
            .map(|record| u32::from_le_bytes(record[..4].try_into().unwrap()))
            .filter(|&stored| stored < position).max().unwrap_or(0);
        entry
            .attachments
            .iter()
            .find(|record| {
                u32::from_le_bytes(record[4..8].try_into().unwrap()) == id
                    && u32::from_le_bytes(record[..4].try_into().unwrap()) == previous
            })
    });
    let Some(record) = record else { return Ok(None); };
    let length = record[8..].iter().position(|&byte| byte == 0)
        .ok_or_else(|| native_state_error("CST detail record has no string terminator"))?;
    Ok(Some(record[8..8 + length].to_vec()))
}

fn cst_string_marker(bytes: &[u8]) -> Option<(usize, u32)> {
    if !bytes.starts_with(b"$str") {
        return None;
    }
    // The optional semicolon is literal source text in the original helpers.
    cst_context_native::integer_prefix(&bytes[4..]).map(|(length, id)| (length + 4, id))
}

fn expand_cst_strings(
    input: &[u8],
    strings: &BTreeMap<u32, Vec<u8>>,
) -> cs2_script::Result<Vec<u8>> {
    let mut output = Vec::new();
    let mut cursor = 0;
    while cursor < input.len() {
        if let Some((consumed, id)) = cst_string_marker(&input[cursor..]) {
            if let Some(value) = strings.get(&id) {
                output.extend_from_slice(value);
            }
            cursor += consumed;
        } else {
            output.push(input[cursor]);
            cursor += 1;
        }
        if output.len() > 64 * 1024 * 1024 {
            return Err(native_state_error(
                "CST string expansion exceeds storage limit",
            ));
        }
    }
    Ok(output)
}

fn cp932_char_width(bytes: &[u8]) -> usize {
    if bytes.len() >= 2
        && matches!(bytes[0], 0x81..=0x9f | 0xe0..=0xef)
        && matches!(bytes[1], 0x40..=0x7e | 0x80..=0xfc)
    {
        2
    } else {
        1
    }
}

fn cp932_char_count(bytes: &[u8]) -> usize {
    let mut offset = 0;
    let mut count = 0;
    while offset < bytes.len() {
        offset += cp932_char_width(&bytes[offset..]);
        count += 1;
    }
    count
}

fn cp932_truncate_bytes(bytes: &[u8], capacity: usize) -> usize {
    let mut offset = 0;
    while offset < bytes.len() {
        let width = cp932_char_width(&bytes[offset..]);
        if offset.saturating_add(width) >= capacity {
            break;
        }
        offset += width;
    }
    offset
}

fn cp932_prefix_chars(bytes: &[u8], count: usize) -> usize {
    let mut offset = 0;
    for _ in 0..count {
        if offset >= bytes.len() {
            break;
        }
        offset += cp932_char_width(&bytes[offset..]);
    }
    offset
}

fn cp932_to_utf8(bytes: &[u8]) -> Vec<u8> {
    let mut output = String::new();
    let mut position = 0;
    while let Some(&byte) = bytes.get(position) {
        match byte {
            0x00..=0x80 => output.push(char::from(byte)),
            0xa0 => output.push('\u{f8f0}'),
            0xa1..=0xdf => {
                output.push(char::from_u32(0xff61 + u32::from(byte - 0xa1)).unwrap());
            }
            0xfd..=0xff => {
                output.push(char::from_u32(0xf8f1 + u32::from(byte - 0xfd)).unwrap());
            }
            _ => {
                let end = (position + 2).min(bytes.len());
                if let Some(decoded) = encoding_rs::SHIFT_JIS
                    .decode_without_bom_handling_and_without_replacement(&bytes[position..end])
                {
                    output.push_str(&decoded);
                } else {
                    output.push('\u{30fb}');
                }
                position = end;
                continue;
            }
        }
        position += 1;
    }
    output.into_bytes()
}

fn cp932_utf8_native(vm: &mut Vm, arguments: &[u8]) -> cs2_script::Result<NativeReturn> {
    if arguments.len() != 8 {
        return Err(native_state_error(
            "native 1063 requires exactly 8 argument bytes",
        ));
    }
    let target = read_u32(arguments, 0)?;
    let source = vm.read_string(read_u32(arguments, 4)?)?;
    let mut converted = cp932_to_utf8(&source);
    if converted.len() < 256 {
        converted.push(0);
        vm.write(target, &converted)?;
    }
    Ok(NativeReturn::Continue)
}

fn utf8_byte_width(bytes: &[u8], offset: usize) -> i32 {
    let Some(&byte) = bytes.get(offset) else {
        return 1;
    };
    let leading = byte.leading_ones();
    match leading {
        0 | 6.. => 1,
        1 => -2,
        _ => {
            for index in 1..leading.saturating_sub(1) as usize {
                if bytes
                    .get(offset.saturating_add(index))
                    .is_none_or(|value| value & 0xc0 != 0x80)
                {
                    return -3;
                }
            }
            leading as i32
        }
    }
}

fn utf8_truncate_native(
    vm: &mut Vm,
    address: u32,
    limit: u32,
    by_bytes: bool,
) -> cs2_script::Result<u32> {
    let bytes = vm.read_string(address)?;
    let mut position = 0_u32;
    let mut count = 0_u32;
    let mut visited = BTreeMap::new();
    while (by_bytes || count < limit)
        && bytes
            .get(position as usize)
            .is_some_and(|&value| value != 0)
    {
        if let Some(previous) = visited.insert(position, count) {
            if by_bytes {
                return Err(native_state_error(
                    "UTF-8 byte truncation entered a repeated position",
                ));
            }
            let period = count.wrapping_sub(previous);
            if period == 0 {
                return Err(native_state_error("UTF-8 truncation made no progress"));
            }
            count = count.saturating_add((limit - count) / period * period);
            if count == limit {
                break;
            }
        }
        if visited.len() > 1_000_000 {
            return Err(native_state_error("UTF-8 truncation exceeded the item limit"));
        }
        let width = utf8_byte_width(&bytes, position as usize);
        let next = position.wrapping_add(width as u32);
        if by_bytes && next >= limit {
            break;
        }
        position = next;
        count = count.saturating_add(1);
    }
    vm.write(address.wrapping_add(position), &[0, 0])?;
    Ok(position)
}

fn utf8_text_native(
    index: u32,
    arguments: &[u8],
    vm: &mut Vm,
) -> cs2_script::Result<NativeReturn> {
    match index {
        1045 | 1046 if arguments.len() == 8 => {
            let address = read_u32(arguments, 0)?;
            let limit = read_u32(arguments, 4)?;
            Ok(NativeReturn::Value(utf8_truncate_native(
                vm,
                address,
                limit,
                index == 1045,
            )?))
        }
        1045 | 1046 => Err(native_state_error(
            "native 1045/1046 require exactly 8 argument bytes",
        )),
        1051 if arguments.len() == 8 => {
            let source_address = read_u32(arguments, 0)?;
            let query_address = read_u32(arguments, 4)?;
            let source = vm.read_string(source_address)?;
            let query_source = vm.read_string(query_address)?;
            let query = if query_source.len() > 4_000 {
                Vec::new()
            } else {
                let query = cp932_to_utf8(&query_source);
                if String::from_utf8_lossy(&query).encode_utf16().count() >= 260 {
                    return Err(native_state_error(
                        "native 1051 query exceeds the conversion buffer limit",
                    ));
                }
                query
            };
            let mut position = 0_usize;
            while source.get(position).copied().unwrap_or(0) != 0 {
                if source
                    .get(position..position.saturating_add(query.len()))
                    .is_some_and(|value| value == query.as_slice())
                {
                    return Ok(NativeReturn::Value(position as u32));
                }
                let width = utf8_byte_width(&source, position);
                if width <= 0 {
                    break;
                }
                position = position.saturating_add(width as usize);
                if position > 64 * 1024 * 1024 {
                    return Err(native_state_error("native 1051 search exceeded the limit"));
                }
            }
            Ok(NativeReturn::Value(source.len() as u32))
        }
        1051 => Err(native_state_error(
            "native 1051 requires exactly 8 argument bytes",
        )),
        _ => Err(native_state_error("unsupported UTF-8 text native")),
    }
}

fn parse_kcs_integer_prefix(bytes: &[u8]) -> Option<u32> {
    let bytes = trim_ascii(bytes);
    let mut cursor = usize::from(matches!(bytes.first(), Some(b'+' | b'-')));
    let negative = bytes.first() == Some(&b'-');
    if !bytes.get(cursor).is_some_and(u8::is_ascii_digit) {
        return None;
    }
    let radix = if bytes
        .get(cursor..)
        .is_some_and(|value| value.starts_with(b"0x") || value.starts_with(b"0X"))
    {
        cursor += 2;
        16
    } else {
        10
    };
    let start = cursor;
    while bytes.get(cursor).is_some_and(|value| {
        if radix == 16 {
            value.is_ascii_hexdigit()
        } else {
            value.is_ascii_digit()
        }
    }) {
        cursor += 1;
    }
    if start == cursor || radix == 10 && bytes.get(cursor) == Some(&b'.') {
        return None;
    }
    let mut value =
        u32::from_str_radix(std::str::from_utf8(&bytes[start..cursor]).ok()?, radix).ok()?;
    if negative {
        value = value.wrapping_neg();
    }
    Some(value)
}

fn parse_kcs_float_prefix(bytes: &[u8]) -> u32 {
    // Reference docs/kcs.md, ordinary 26: CRT numeric prefix, converted through
    // double before float. Work on ASCII bytes so trailing CP932 is not decoded
    // or included in the numeric token.
    let mut cursor = 0;
    while bytes
        .get(cursor)
        .is_some_and(|&byte| matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c))
    {
        cursor += 1;
    }
    let start = cursor;
    let negative = bytes.get(cursor) == Some(&b'-');
    if matches!(bytes.get(cursor), Some(b'+' | b'-')) {
        cursor += 1;
    }
    if bytes
        .get(cursor..cursor.saturating_add(3))
        .is_some_and(|word| word.eq_ignore_ascii_case(b"nan"))
    {
        return if negative { 0xffff_ffff } else { 0x7fff_ffff };
    }
    if bytes
        .get(cursor..cursor.saturating_add(3))
        .is_some_and(|word| word.eq_ignore_ascii_case(b"inf"))
    {
        return if negative {
            f32::NEG_INFINITY.to_bits()
        } else {
            f32::INFINITY.to_bits()
        };
    }
    if bytes.get(cursor) == Some(&b'0')
        && matches!(bytes.get(cursor + 1), Some(b'x' | b'X'))
    {
        if let Some(value) = parse_kcs_hex_float_prefix(&bytes[cursor + 2..], negative) {
            return (value as f32).to_bits();
        }
        // With no hex digit, the leading decimal zero remains a valid prefix.
    }

    let mut digits = 0;
    while bytes.get(cursor).is_some_and(u8::is_ascii_digit) {
        cursor += 1;
        digits += 1;
    }
    if bytes.get(cursor) == Some(&b'.') {
        cursor += 1;
        while bytes.get(cursor).is_some_and(u8::is_ascii_digit) {
            cursor += 1;
            digits += 1;
        }
    }
    if digits == 0 {
        return 0;
    }
    let mut end = cursor;
    if matches!(bytes.get(cursor), Some(b'e' | b'E')) {
        cursor += 1;
        if matches!(bytes.get(cursor), Some(b'+' | b'-')) {
            cursor += 1;
        }
        let exponent_start = cursor;
        while bytes.get(cursor).is_some_and(u8::is_ascii_digit) {
            cursor += 1;
        }
        if cursor != exponent_start {
            end = cursor;
        }
    }
    std::str::from_utf8(&bytes[start..end])
        .ok()
        .and_then(|text| text.parse::<f64>().ok())
        .map_or(0, |value| (value as f32).to_bits())
}

fn parse_kcs_hex_float_prefix(bytes: &[u8], negative: bool) -> Option<f64> {
    let mut cursor = 0;
    let mut fraction = false;
    let mut digits = 0;
    let mut fractional_digits = 0_i64;
    let mut significant_bits = 0_i64;
    let mut kept_bits = 0_u32;
    let mut significand = 0_u64;
    let mut sticky = false;
    loop {
        let Some(&byte) = bytes.get(cursor) else {
            break;
        };
        if byte == b'.' && !fraction {
            fraction = true;
            cursor += 1;
            continue;
        }
        let digit = match byte {
            b'0'..=b'9' => u32::from(byte - b'0'),
            b'a'..=b'f' => u32::from(byte - b'a') + 10,
            b'A'..=b'F' => u32::from(byte - b'A') + 10,
            _ => break,
        };
        cursor += 1;
        digits += 1;
        fractional_digits += if fraction { 1 } else { 0 };
        if significant_bits == 0 && digit == 0 {
            continue;
        }
        let bits = if significant_bits == 0 {
            u32::BITS - digit.leading_zeros()
        } else {
            4
        };
        significant_bits += i64::from(bits);
        let take = bits.min(64 - kept_bits);
        if take != 0 {
            significand = (significand << take) | u64::from(digit >> (bits - take));
            kept_bits += take;
        }
        let discarded = bits - take;
        if discarded != 0 && digit & ((1_u32 << discarded) - 1) != 0 {
            sticky = true;
        }
    }
    if digits == 0 {
        return None;
    }
    let mut exponent = 0_i64;
    if matches!(bytes.get(cursor), Some(b'p' | b'P')) {
        cursor += 1;
        let exponent_negative = bytes.get(cursor) == Some(&b'-');
        if matches!(bytes.get(cursor), Some(b'+' | b'-')) {
            cursor += 1;
        }
        while let Some(byte) = bytes.get(cursor).filter(|byte| byte.is_ascii_digit()) {
            exponent = exponent
                .saturating_mul(10)
                .saturating_add(i64::from(*byte - b'0'));
            cursor += 1;
        }
        if exponent_negative {
            exponent = -exponent;
        }
    }
    let sign = if negative { 1_u64 << 63 } else { 0 };
    if significant_bits == 0 {
        return Some(f64::from_bits(sign));
    }
    let mut exponent = (significant_bits - 1)
        .saturating_add(exponent)
        .saturating_sub(fractional_digits * 4);
    if exponent > 1023 {
        return Some(f64::from_bits(sign | f64::INFINITY.to_bits()));
    }
    if exponent < -1075 {
        return Some(f64::from_bits(sign));
    }
    let precision = if exponent >= -1022 {
        53
    } else {
        (exponent + 1075) as u32
    };
    // Keep guard/round/sticky information when first rounding to binary64,
    // including subnormal ties; native 26 then rounds binary64 to binary32.
    let mut rounded = if kept_bits <= precision {
        significand << (precision - kept_bits)
    } else {
        let discard = kept_bits - precision;
        let value = u128::from(significand);
        let remainder = value & ((1_u128 << discard) - 1);
        let halfway = 1_u128 << (discard - 1);
        let upper = value >> discard;
        let round_up =
            remainder > halfway || remainder == halfway && (sticky || upper & 1 != 0);
        (upper + if round_up { 1 } else { 0 }) as u64
    };
    let magnitude = if exponent < -1022 {
        rounded
    } else {
        if rounded == 1_u64 << 53 {
            rounded >>= 1;
            exponent += 1;
        }
        if exponent > 1023 {
            f64::INFINITY.to_bits()
        } else {
            (((exponent + 1023) as u64) << 52) | (rounded & ((1_u64 << 52) - 1))
        }
    };
    Some(f64::from_bits(sign | magnitude))
}

fn parse_percent_integer(bytes: &[u8]) -> Option<i32> {
    let bytes = trim_ascii(bytes);
    if bytes.last() != Some(&b'%') {
        return None;
    }
    parse_kcs_integer_prefix(&bytes[..bytes.len() - 1]).map(|value| value as i32)
}

fn float_to_i32(value: f32) -> i32 {
    if !value.is_finite() || value >= 2_147_483_648.0 || value < -2_147_483_648.0 {
        i32::MIN
    } else {
        value.trunc() as i32
    }
}

fn native_scalar(index: u32, first: u32, second: u32) -> u32 {
    // Ordinary 64 returns an integer DWORD, including wrapping i32::MIN.
    // It must bypass the floating-point result encoding below.
    if index == 64 {
        return (first as i32).wrapping_abs() as u32;
    }
    let a = f32::from_bits(first);
    let b = f32::from_bits(second);
    let value = match index {
        50 => a.sin(),
        51 => a.cos(),
        52 => a.tan(),
        53 => a.asin(),
        54 => a.acos(),
        55 => a.atan(),
        56 => a.ceil(),
        57 => a.abs(),
        58 => a.floor(),
        59 => a % b,
        60 => a.sqrt(),
        61 => a.powf(b),
        62 => a * std::f32::consts::PI / 180.0,
        63 => a * 180.0 / std::f32::consts::PI,
        _ => 0.0,
    };
    if value.is_nan() {
        0xffc0_0000
    } else {
        value.to_bits()
    }
}

fn parse_separators(input: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(input.len());
    let mut index = 0;
    while index < input.len() {
        if input[index] == b'\\' && input.get(index + 1) == Some(&b't') {
            output.push(b'\t');
            index += 2;
        } else {
            output.push(input[index]);
            index += 1;
        }
    }
    output
}

fn split_bytes(input: &[u8], separators: &[u8]) -> Vec<Vec<u8>> {
    let mut tokens = Vec::new();
    let mut start = 0;
    for (index, byte) in input.iter().enumerate() {
        if separators.contains(byte) {
            tokens.push(input[start..index].to_vec());
            start = index + 1;
        }
    }
    tokens.push(input[start..].to_vec());
    tokens
}

fn read_native_expression(
    vm: &Vm,
    cursor: &mut u32,
    separators: &[u8],
) -> cs2_script::Result<Vec<u8>> {
    let mut expression = Vec::new();
    for _ in 0..260 {
        let byte = vm.read(*cursor, 1)?[0];
        if byte == 0 || separators.contains(&byte) {
            return Ok(expression);
        }
        expression.push(byte);
        *cursor = cursor.wrapping_add(1);
    }
    Err(native_state_error(
        "native 539 expression exceeds 259 bytes",
    ))
}

fn format_grouped_integer(value: i32, show_sign: bool) -> String {
    let negative = value < 0;
    let magnitude = if negative {
        (value as i64).unsigned_abs()
    } else {
        value as u32 as u64
    };
    let digits = magnitude.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3 + 1);
    for (index, byte) in digits.bytes().enumerate() {
        if index != 0 && (digits.len() - index) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(byte as char);
    }
    if negative {
        format!("-{grouped}")
    } else if show_sign {
        format!("+{grouped}")
    } else {
        grouped
    }
}

fn replace_all_non_overlapping(input: &[u8], find: &[u8], replacement: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(input.len());
    let mut cursor = 0;
    while cursor < input.len() {
        if input
            .get(cursor..cursor.saturating_add(find.len()))
            .is_some_and(|value| value == find)
        {
            output.extend_from_slice(replacement);
            cursor += find.len();
        } else {
            output.push(input[cursor]);
            cursor += 1;
        }
    }
    output
}

fn split_kcs_parenthesized(input: &[u8], separators: &[u8]) -> Vec<u8> {
    let mut output = Vec::new();
    let mut token = Vec::new();
    let mut cursor = 0;
    let mut depth = 0usize;
    while cursor < input.len() {
        let byte = input[cursor];
        if depth == 0 && separators.contains(&byte) {
            if !token.is_empty() {
                output.extend_from_slice(&token);
                output.push(0);
                token.clear();
            }
            cursor += 1;
            continue;
        }
        if byte == b'(' {
            if depth == 0 && !token.is_empty() && token.last().is_some_and(u8::is_ascii_alphabetic)
            {
                output.extend_from_slice(&token);
                output.push(0);
                token.clear();
            }
            depth += 1;
            if input.get(cursor + 1) == Some(&b':') {
                cursor += 2;
                let start = cursor;
                let mut nested = 1usize;
                while cursor < input.len() {
                    match input[cursor] {
                        b'(' => nested += 1,
                        b')' => {
                            nested = nested.saturating_sub(1);
                            if nested == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                    cursor += 1;
                }
                token.extend_from_slice(&input[start..cursor.min(input.len())]);
                depth = depth.saturating_sub(1);
                cursor = cursor.saturating_add(1);
                continue;
            }
            token.push(byte);
            cursor += 1;
            continue;
        }
        if byte == b')' && depth > 0 {
            depth -= 1;
        }
        token.push(byte);
        cursor += 1;
    }
    if !token.is_empty() {
        output.extend_from_slice(&token);
        output.push(0);
    }
    output.push(0);
    output
}

fn split_kcs_string_table(input: &[u8], separators: &[u8]) -> Vec<u8> {
    let mut output = Vec::new();
    let mut position = 0;
    while position < input.len() {
        if separators.contains(&input[position]) {
            if output.last().is_some_and(|byte| *byte != 0) {
                output.push(0);
            }
            position += 1;
        } else {
            let width = cp932_char_width(&input[position..]);
            output.extend_from_slice(&input[position..position + width]);
            position += width;
        }
    }
    output.extend_from_slice(&[0, 0]);
    output
}

fn encode_string_table(tokens: &[Vec<u8>]) -> Vec<u8> {
    let mut output = Vec::new();
    for token in tokens {
        output.extend_from_slice(token);
        output.push(0);
    }
    output.push(0);
    output
}

fn read_string_table(vm: &Vm, address: u32) -> cs2_script::Result<Vec<Vec<u8>>> {
    let mut tokens = Vec::new();
    let mut cursor = address;
    for _ in 0..1_000_000 {
        let token = vm.read_string(cursor)?;
        if token.is_empty() {
            return Ok(tokens);
        }
        cursor = cursor.wrapping_add(token.len() as u32 + 1);
        tokens.push(token);
    }
    Err(cs2_script::ScriptError::InvalidStructuredText {
        offset: address as usize,
        message: "KCS string table exceeds entry limit",
    })
}

fn read_f32_vector(vm: &Vm, address: u32) -> cs2_script::Result<[f32; 4]> {
    let bytes = vm.read(address, 16)?;
    let mut vector = [0.0_f32; 4];
    for (target, bytes) in vector.iter_mut().zip(bytes.chunks_exact(4)) {
        *target = f32::from_bits(u32::from_le_bytes(bytes.try_into().unwrap()));
    }
    Ok(vector)
}

fn write_f32_vector(vm: &mut Vm, address: u32, vector: [f32; 4]) -> cs2_script::Result<()> {
    let mut bytes = Vec::with_capacity(16);
    for value in vector {
        bytes.extend_from_slice(&value.to_bits().to_le_bytes());
    }
    vm.write(address, &bytes)
}

fn read_f32_matrix(vm: &Vm, address: u32) -> cs2_script::Result<[f32; 16]> {
    let bytes = vm.read(address, 64)?;
    let mut matrix = [0.0_f32; 16];
    for (target, bytes) in matrix.iter_mut().zip(bytes.chunks_exact(4)) {
        *target = f32::from_bits(u32::from_le_bytes(bytes.try_into().unwrap()));
    }
    Ok(matrix)
}

fn normalize_f32_vector(vector: &mut [f32; 4]) {
    let length_squared = vector
        .iter()
        .copied()
        .fold(0.0_f32, |sum, component| sum + component * component);
    if length_squared == 0.0 {
        *vector = [f32::from_bits(0xffc0_0000); 4];
        return;
    }
    let inverse = length_squared.sqrt().recip();
    for component in vector {
        *component *= inverse;
    }
}

fn prepare_spline(vm: &mut Vm, address: u32, count: u32) -> cs2_script::Result<()> {
    if (count.wrapping_sub(1) as i32) < 2 {
        return Ok(());
    }
    let count = usize::try_from(count)
        .map_err(|_| native_state_error("native 320 point count is invalid"))?;
    let byte_length = count
        .checked_mul(20)
        .filter(|length| *length <= 64 * 1024 * 1024)
        .ok_or_else(|| native_state_error("native 320 control points exceed storage limit"))?;
    let mut points = vm
        .read(address, byte_length)?
        .chunks_exact(20)
        .map(|chunk| {
            std::array::from_fn(|index| {
                u32::from_le_bytes(chunk[index * 4..index * 4 + 4].try_into().unwrap())
            })
        })
        .collect::<Vec<[u32; 5]>>();
    let n = count - 1;
    points[0][4] = spline_float(2, points[1][0], points[0][0]);
    points[1][3] = spline_float(4, spline_float(2, points[1][1], points[0][1]), points[0][4]);
    for index in 1..n {
        let interval = spline_float(2, points[index + 1][0], points[index][0]);
        points[index][4] = interval;
        let diagonal = spline_float(1, points[index - 1][4], interval);
        points[index][2] = spline_float(1, diagonal, diagonal);
        points[index + 1][3] = spline_float(
            4,
            spline_float(2, points[index + 1][1], points[index][1]),
            interval,
        );
        points[index][3] = spline_float(2, points[index + 1][3], points[index][3]);
    }
    points[0][2] = points[0][4] ^ 0x8000_0000;
    points[n][2] = points[n - 1][4] ^ 0x8000_0000;
    if count == 3 {
        points[0][3] = 0;
        points[n][3] = 0;
    } else {
        points[0][3] = spline_float(
            2,
            spline_float(4, points[2][3], spline_float(2, points[3][0], points[1][0])),
            spline_float(4, points[1][3], spline_float(2, points[2][0], points[0][0])),
        );
        points[n][3] = spline_float(
            2,
            spline_float(
                4,
                points[n - 1][3],
                spline_float(2, points[n][0], points[n - 2][0]),
            ),
            spline_float(
                4,
                points[n - 2][3],
                spline_float(2, points[n - 1][0], points[n - 3][0]),
            ),
        );
        let square = spline_pow(points[0][4], 2.0_f32.to_bits());
        points[0][3] = spline_float(
            4,
            spline_float(3, square, points[0][3]),
            spline_float(2, points[3][0], points[1][0]),
        );
        let square = spline_pow(points[n - 1][4], 2.0_f32.to_bits());
        points[n][3] = spline_float(
            4,
            spline_float(3, square, points[n][3]),
            spline_float(2, points[n][0], points[n - 3][0]),
        ) ^ 0x8000_0000;
    }
    for index in 1..=n {
        let ratio = spline_float(4, points[index - 1][4], points[index - 1][2]);
        points[index][2] = spline_float(
            2,
            points[index][2],
            spline_float(3, points[index - 1][4], ratio),
        );
        points[index][3] = spline_float(
            2,
            points[index][3],
            spline_float(3, points[index - 1][3], ratio),
        );
    }
    points[n][3] = spline_float(4, points[n][3], points[n][2]);
    for index in (0..n).rev() {
        points[index][3] = spline_float(
            4,
            spline_float(
                2,
                points[index][3],
                spline_float(3, points[index + 1][3], points[index][4]),
            ),
            points[index][2],
        );
    }
    points[n][2] = spline_float(
        1,
        spline_float(
            3,
            spline_float(
                1,
                spline_float(1, points[n][3], points[n][3]),
                points[n - 1][3],
            ),
            points[n - 1][4],
        ),
        spline_float(
            4,
            spline_float(2, points[n][1], points[n - 1][1]),
            points[n - 1][4],
        ),
    );
    for index in 0..n {
        let current = points[index][3];
        let next = points[index + 1][3];
        let interval = points[index][4];
        let slope = spline_float(
            4,
            spline_float(2, points[index + 1][1], points[index][1]),
            interval,
        );
        points[index][2] = spline_float(
            2,
            slope,
            spline_float(
                3,
                spline_float(1, spline_float(1, current, current), next),
                interval,
            ),
        );
        points[index][4] = spline_float(4, spline_float(2, next, current), interval);
        points[index][3] = spline_float(3, current, 3.0_f32.to_bits());
    }
    points[n][4] = points[n - 1][4];
    let bytes = points
        .into_iter()
        .flatten()
        .flat_map(u32::to_le_bytes)
        .collect::<Vec<_>>();
    vm.write(address, &bytes)
}

fn spline_float(operation: u8, left: u32, right: u32) -> u32 {
    let a = f32::from_bits(left);
    let b = f32::from_bits(right);
    if a.is_nan() {
        return left | 0x0040_0000;
    }
    if b.is_nan() {
        return right | 0x0040_0000;
    }
    let value = match operation {
        1 => a + b,
        2 => a - b,
        3 => a * b,
        4 => a / b,
        _ => unreachable!("spline uses only IEEE add/subtract/multiply/divide"),
    };
    if value.is_nan() {
        0xffc0_0000
    } else {
        value.to_bits()
    }
}

fn spline_pow(value: u32, exponent: u32) -> u32 {
    let base = f32::from_bits(value);
    let exponent = f32::from_bits(exponent);
    if base.is_nan() {
        return value | 0x0040_0000;
    }
    if exponent.is_nan() {
        return exponent.to_bits() | 0x0040_0000;
    }
    let result = (base as f64).powf(exponent as f64) as f32;
    if result.is_nan() {
        0xffc0_0000
    } else {
        result.to_bits()
    }
}

fn packed_color(value: u32) -> Color {
    Color::rgba(
        (value >> 16) as u8,
        (value >> 8) as u8,
        value as u8,
        (value >> 24) as u8,
    )
}

fn fill_frame_rect(frame: &RgbaFrame, rect: Rect, color: Color) -> cs2_script::Result<RgbaFrame> {
    let mut pixels = frame.pixels().to_vec();
    let width = frame.width() as i32;
    let height = frame.height() as i32;
    for y in 0..rect.height {
        for x in 0..rect.width {
            let px = rect.x.saturating_add(x as i32);
            let py = rect.y.saturating_add(y as i32);
            if px < 0 || py < 0 || px >= width || py >= height {
                continue;
            }
            let offset = (py as usize * width as usize + px as usize) * 4;
            pixels[offset..offset + 4].copy_from_slice(&[color.r, color.g, color.b, color.a]);
        }
    }
    RgbaFrame::from_rgba8_with_metadata(frame.width(), frame.height(), pixels, frame.metadata())
        .map_err(|_| native_state_error("image rectangle fill failed"))
}

fn adjust_rgb_frame(frame: &RgbaFrame, percentages: [u32; 3]) -> cs2_script::Result<RgbaFrame> {
    let mut pixels = frame.pixels().to_vec();
    for pixel in pixels.chunks_exact_mut(4) {
        for channel in 0..3 {
            let value = u32::from(pixel[channel])
                .saturating_mul(percentages[channel])
                .saturating_add(50)
                / 100;
            pixel[channel] = value.min(255) as u8;
        }
    }
    RgbaFrame::from_rgba8_with_metadata(frame.width(), frame.height(), pixels, frame.metadata())
        .map_err(|_| native_state_error("image RGB adjustment failed"))
}

fn blur_frame(frame: &RgbaFrame, radius_x: u32, radius_y: u32) -> cs2_script::Result<RgbaFrame> {
    let radius_x = radius_x.min(255) as i32;
    let radius_y = radius_y.min(255) as i32;
    if radius_x == 0 && radius_y == 0 {
        return Ok(frame.clone());
    }
    let width = frame.width() as i32;
    let height = frame.height() as i32;
    let mut horizontal = frame.pixels().to_vec();
    if radius_x > 0 {
        for y in 0..height {
            for x in 0..width {
                let left = (x - radius_x).max(0);
                let right = (x + radius_x).min(width - 1);
                let count = (right - left + 1) as u32;
                let destination = (y as usize * width as usize + x as usize) * 4;
                let mut sums = [0_u32; 4];
                for sample_x in left..=right {
                    let source = (y as usize * width as usize + sample_x as usize) * 4;
                    for channel in 0..4 {
                        sums[channel] = sums[channel]
                            .saturating_add(u32::from(frame.pixels()[source + channel]));
                    }
                }
                for channel in 0..4 {
                    horizontal[destination + channel] = (sums[channel] / count) as u8;
                }
            }
        }
    }
    let source = if radius_x > 0 {
        &horizontal
    } else {
        frame.pixels()
    };
    let mut output = source.to_vec();
    if radius_y > 0 {
        for y in 0..height {
            for x in 0..width {
                let top = (y - radius_y).max(0);
                let bottom = (y + radius_y).min(height - 1);
                let count = (bottom - top + 1) as u32;
                let destination = (y as usize * width as usize + x as usize) * 4;
                let mut sums = [0_u32; 4];
                for sample_y in top..=bottom {
                    let source_offset = (sample_y as usize * width as usize + x as usize) * 4;
                    for channel in 0..4 {
                        sums[channel] = sums[channel]
                            .saturating_add(u32::from(source[source_offset + channel]));
                    }
                }
                for channel in 0..4 {
                    output[destination + channel] = (sums[channel] / count) as u8;
                }
            }
        }
    }
    RgbaFrame::from_rgba8_with_metadata(frame.width(), frame.height(), output, frame.metadata())
        .map_err(|_| native_state_error("image blur failed"))
}

impl LoggingHost<'_> {
    fn apply_particle_parameters(&mut self, target: u32, parameters: [u8; 128]) -> cs2_script::Result<()> {
        if let Some(particle) = self.particles.get_mut(&target) {
            particle.parameters = parameters;
            if let Some(runner) = particle.runner.as_mut() { runner.set_parameters(parameters)?; }
        }
        Ok(())
    }

    fn apply_particle_command(
        &mut self,
        target: u32,
        command: u32,
        value: Option<u32>,
    ) -> cs2_script::Result<Option<u32>> {
        if command == 323 {
            self.release_particle_program(target);
            return Ok(None);
        }
        let Some(particle) = self.particles.get_mut(&target) else { return Ok(None) };
        match command {
            321 => {
                if particle.external_draw != 0 && !particle.external_extent_bound {
                    return Err(particle_script_error("external particle has no real extent binding"));
                }
                if let Some(plugin) = particle.plugin.as_mut() {
                    let canvas = self.canvas.as_ref().ok_or_else(|| particle_script_error("particle plugin start has no real canvas"))?;
                    let viewport = [canvas.width(), canvas.height()];
                    plugin.start(&particle.parameters, viewport, self.resources, self.script_random)?;
                    particle.rendered = Some(plugin.snapshot(particle.opacity));
                } else {
                    let runner = particle.runner.as_mut().ok_or_else(||
                        particle_script_error("particle has no executable KCS/plugin program"))?;
                    runner.start()?;
                    particle.rendered = Some(runner.render_snapshot());
                }
                particle.enabled = true;
                particle.elapsed = 0;
                particle.points.clear();
            }
            322 => {
                if let Some(runner) = particle.runner.as_mut() { runner.stop(); }
                if let Some(plugin) = particle.plugin.as_mut() { plugin.stop(); }
                particle.enabled = false;
                particle.points.clear();
                particle.rendered = None;
            }
            324 => return Ok(Some(u32::from(particle.enabled))),
            326 => {
                let value = value.unwrap_or_default();
                particle.opacity = if (value as i32) < 255 { value } else { 256 };
                if let Some(runner) = particle.runner.as_mut() { runner.set_opacity(value); }
                if let Some(snapshot) = particle.rendered.as_mut() { snapshot.opacity = particle.opacity; }
            }
            327 => {
                let value = value.unwrap_or_default();
                particle.transform_source = if value == 0 { u32::MAX } else { value };
            }
            591 => particle.coordinate_group = value.unwrap_or_default(),
            592 => particle.coordinate_group = 0,
            _ => {}
        }
        Ok(None)
    }

    fn allocate_handle(&mut self) -> u32 {
        let handle = *self.next_xml_handle;
        *self.next_xml_handle = self.next_xml_handle.saturating_add(1).max(1);
        handle
    }

    pub(crate) fn select_sound_group_channel(
        &mut self,
        group_key: u32,
        by_key: bool,
    ) -> cs2_script::Result<Option<u32>> {
        if self.sound_groups.is_empty() {
            return Err(audio_script_error(
                "native 936 sound groups are not configured",
            ));
        }
        let group = if by_key {
            self.sound_groups
                .iter()
                .find(|group| group.key == group_key)
        } else {
            self.sound_groups
                .iter()
                .filter(|group| group.id == 0)
                .min_by_key(|group| group.key)
        };
        let Some(group) = group else {
            return Ok(None);
        };
        let first = group.range[0].clamp(0, 64) as u32;
        let last = group.range[1].clamp(0, 64) as u32;
        if first > last || first >= 64 {
            return Ok(None);
        }
        let mut first_channel = None;
        for channel in first..=last.min(63) {
            first_channel.get_or_insert(channel);
            let active = match self
                .sound_channels
                .get(&channel)
                .and_then(|state| state.handle)
            {
                Some(handle) => self
                    .audio_host
                    .is_active(handle)
                    .map_err(|_| audio_script_error("native 936 channel status query failed"))?,
                None => false,
            };
            if !active {
                if let Some(state) = self.sound_channels.get_mut(&channel) {
                    state.handle = None;
                    state.paused = false;
                }
                return Ok(Some(channel));
            }
        }
        Ok(first_channel)
    }


}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TempDir(PathBuf);
    impl TempDir {
        fn new() -> Self {
            let suffix = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!("cs2-desktop-{suffix}"));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn report_perf_compatibility_is_lazy_and_scoped_to_a_player() {
        let temp = TempDir::new();
        let executable = temp.0.join("cs2.exe");
        fs::write(&executable, b"not a PE").unwrap();
        let player = HeadlessPlayer::open_with_save_root(&temp.0, temp.0.join("save")).unwrap();
        assert!(player.compatibility.get().is_none());

        let mut pe = [0; 0x5a];
        pe[..2].copy_from_slice(b"MZ");
        pe[0x3c] = 0x40;
        pe[0x40..0x44].copy_from_slice(b"PE\0\0");
        pe[0x44..0x46].copy_from_slice(&0x014c_u16.to_le_bytes());
        pe[0x54] = 2;
        pe[0x58..0x5a].copy_from_slice(&0x010b_u16.to_le_bytes());
        fs::write(&executable, pe).unwrap();

        let report = player.compatibility_report();
        assert_eq!(report.pe_architecture.as_deref(), Some("x86"));
        fs::write(&executable, b"not a PE").unwrap();
        assert_eq!(player.report().compatibility, report);
        let reopened = HeadlessPlayer::open_with_save_root(&temp.0, temp.0.join("save")).unwrap();
        assert_eq!(reopened.compatibility_report().pe_architecture, None);
    }

    #[test]
    fn report_perf_partial_frame_keeps_root_checkpoint_and_completed_pixels() {
        let temp = TempDir::new();
        for capture_script_evidence in [true, false] {
            let mut player =
                HeadlessPlayer::open_with_save_root(&temp.0, temp.0.join("save")).unwrap();
            player.capture_script_evidence = capture_script_evidence;
            let vm = Vm::from_program(
                KcsProgram::from_code(&[0x00, 0x00, 0x3b], vec![0; 8], 64).unwrap(),
            ).unwrap();
            player.data_services.tasks.set_factory_external(vm.external_binding());
            player.data_services.tasks.register_external_task(
                player.current_task_id, 0, &player.data_services.language.languages,
            ).unwrap();
            player.vm = Some(vm);
            player.state.scene.current_script = Some("main.kcs".into());
            player.state.script.current_script = Some("main.kcs".into());
            let completed = player.frame_snapshot();
            assert!(player.state().script.memory.is_none());

            player.frame_phase = PlayerFramePhase::Root;
            player.run_frames(1, 1).unwrap();
            assert!(player.frame_in_progress());
            assert_eq!(player.vm.as_ref().unwrap().instruction_pointer, 1);
            assert_eq!(player.frame_rgba(), completed.rgba());
            if capture_script_evidence {
                let mut expected = ScriptState::from_vm(player.vm.as_ref().unwrap());
                expected.current_script = Some("main.kcs".into());
                player.vm.as_mut().unwrap().write(0, &[0x73]).unwrap();
                let report = player.report();
                assert_eq!(report.state.script, expected);
                assert!(report.frame_in_progress);
                assert_eq!(report.frame_hash, completed.hash());
                assert_eq!(report.frame_rgba, completed.rgba());
            } else {
                assert!(player.state().script.memory.is_none());
                assert!(player.compatibility.get().is_none());
            }
        }
    }

    #[test]
    fn report_perf_window_checks_new_failures_before_trimming_diagnostics() {
        let temp = TempDir::new();
        let mut player = HeadlessPlayer::open_with_save_root(&temp.0, temp.0.join("save")).unwrap();
        player.events.push(LogicEvent {
            frame: 1,
            kind: "native-call".into(),
            detail: "native Is Not Implemented".into(),
        });
        player.events.extend((2..=20_001).map(|frame| LogicEvent {
            frame,
            kind: "logic-frame".into(),
            detail: String::new(),
        }));
        player.log.resize(2_001, "retained".into());
        let report = player.report();
        assert_eq!(report.events.len(), 20_001);
        assert_eq!(report.log.len(), 2_001);

        let mut diagnostics = WindowDiagnostics::default();
        let failure = diagnostics.check_and_limit(&mut player).unwrap_err();
        assert!(failure.to_string().contains("failure at frame 1"));
        assert!(player.events().len() <= 10_000);
        assert!(player.log().len() <= 1_000);
        assert_eq!(player.events().last().unwrap().frame, 20_001);
        diagnostics.check_and_limit(&mut player).unwrap();

        player.events.push(LogicEvent {
            frame: 20_002,
            kind: "vm-error".into(),
            detail: "new failure".into(),
        });
        assert!(diagnostics.check_and_limit(&mut player).unwrap_err().to_string()
            .contains("vm-error new failure"));
    }

    fn packed_cst(inputs: &[Vec<(u8, &[u8])>]) -> Vec<u8> {
        let records = inputs.iter().flatten().collect::<Vec<_>>();
        let record_table = inputs.len() * 8;
        let string_table = record_table + records.len() * 4;
        let mut strings = Vec::new();
        let mut offsets = Vec::with_capacity(records.len());
        for (kind, text) in records {
            offsets.push(strings.len() as u32);
            strings.extend_from_slice(&[1, *kind]);
            strings.extend_from_slice(text);
            strings.push(0);
        }

        let mut body = vec![0; 16 + string_table + strings.len()];
        let payload_length = (body.len() - 16) as u32;
        body[0..4].copy_from_slice(&payload_length.to_le_bytes());
        body[4..8].copy_from_slice(&(inputs.len() as u32).to_le_bytes());
        body[8..12].copy_from_slice(&(record_table as u32).to_le_bytes());
        body[12..16].copy_from_slice(&(string_table as u32).to_le_bytes());
        let mut input_start = 0u32;
        for (index, input) in inputs.iter().enumerate() {
            let offset = 16 + index * 8;
            body[offset..offset + 4].copy_from_slice(&(input.len() as u32).to_le_bytes());
            body[offset + 4..offset + 8].copy_from_slice(&input_start.to_le_bytes());
            input_start += input.len() as u32;
        }
        for (index, offset) in offsets.into_iter().enumerate() {
            let at = 16 + record_table + index * 4;
            body[at..at + 4].copy_from_slice(&offset.to_le_bytes());
        }
        body[16 + string_table..].copy_from_slice(&strings);

        let mut file = Vec::with_capacity(16 + body.len());
        file.extend_from_slice(b"CatScene");
        file.extend_from_slice(&0u32.to_le_bytes());
        file.extend_from_slice(&(body.len() as u32).to_le_bytes());
        file.extend_from_slice(&body);
        file
    }

    fn packed_cstl(name: &str, content: &str) -> Vec<u8> {
        fn ff(value: usize, output: &mut Vec<u8>) {
            let mut value = value;
            while value >= 255 {
                output.push(255);
                value -= 255;
            }
            output.push(value as u8);
        }
        fn string(value: &str, output: &mut Vec<u8>) {
            ff(value.len(), output);
            output.extend_from_slice(value.as_bytes());
        }

        let mut data = b"CSTL".to_vec();
        data.extend_from_slice(&0u32.to_le_bytes());
        ff(1, &mut data);
        string("zh", &mut data);
        ff(1, &mut data);
        string(name, &mut data);
        string(content, &mut data);
        data
    }

    #[test]
    fn cst_table_current_and_legacy_formats_round_trip() {
        let mut name = [0; 32];
        name[..4].copy_from_slice(b"slot");
        let entry = CstRegistration {
            name,
            value: 0xfedc_ba98,
            attachments: vec![[0x5a; 72]],
            data: b"extra-state".to_vec(),
        };
        let current = serialize_cst_table(std::slice::from_ref(&entry), true).unwrap();
        assert_eq!(parse_cst_table(&current).unwrap(), [entry.clone()]);

        let without_data = serialize_cst_table(std::slice::from_ref(&entry), false).unwrap();
        let mut expected = entry.clone();
        expected.data.clear();
        assert_eq!(parse_cst_table(&without_data).unwrap(), [expected]);

        let mut legacy = Vec::new();
        legacy.extend_from_slice(&entry.name);
        legacy.extend_from_slice(&entry.value.to_le_bytes());
        legacy.extend_from_slice(&(entry.attachments.len() as u32).to_le_bytes());
        legacy.extend_from_slice(&entry.attachments[0]);
        let parsed_legacy = parse_cst_table(&legacy).unwrap();
        assert_eq!(parsed_legacy[0].name, entry.name);
        assert_eq!(parsed_legacy[0].value, entry.value);
        assert_eq!(parsed_legacy[0].attachments, entry.attachments);
        assert!(parsed_legacy[0].data.is_empty());
    }

    #[test]
    fn css_save_files_round_trip_outside_game_root() {
        let temp = TempDir::new();
        let save_root = temp.0.join("profile").join("save");
        let mut file = CssFile::new(0x1234);
        file.append(0x55, b"saved payload").unwrap();
        write_save_css(&save_root, "save0001.dat", &file.to_css().unwrap()).unwrap();
        let loaded = CssFile::parse(&read_save_css(&save_root, "save0001.dat").unwrap()).unwrap();
        assert_eq!(loaded.record(0x55), Some(b"saved payload".as_slice()));
        assert!(save_path(&save_root, "../outside.dat").is_err());
    }

    #[test]
    fn cst_context_natives_save_and_restore_history_data() {
        let temp = TempDir::new();
        let resources = ResourceIndex::scan(&temp.0).unwrap();
        let mut events = Vec::new();
        let mut xml_handles = BTreeMap::new();
        let mut next_xml_handle = 1;
        let mut shared_values = Vec::new();
        let mut current_parameter = 0;
        let mut string_values = BTreeMap::new();
        let mut next_task_id = 1;
        let mut task_parents = BTreeMap::new();
        let mut scene_nodes = BTreeMap::new();
        let mut image_handles = BTreeMap::new();
        let script_replacements = BTreeMap::new();
        let mut particles = BTreeMap::new();
        let mut css_handles = BTreeMap::new();
        let mut integer_maps = BTreeMap::new();
        let mut cst_contexts = BTreeMap::new();
        let mut motion_controllers = BTreeMap::new();
        let mut motion_snapshots = BTreeMap::new();
        let mut message_wait_address = None;
        let mut audio_host =
            OggAudioHost::new(|_| Err(RuntimeError::Host("unexpected audio resolution".into())));
        let mut sound_channels = BTreeMap::new();
        let mut script_random = ScriptRandom::default();
        let mut vm = Vm::new();
        let mut host = LoggingHost {
            events: &mut events,
            frame: 0,
            resources: &resources,
            xml_handles: &mut xml_handles,
            next_xml_handle: &mut next_xml_handle,
            configured_shared_slots: 0,
            text_encoding: ScriptTextEncoding::Cp932,
            shared_values: &mut shared_values,
            current_parameter: &mut current_parameter,
            current_task_id: 0,
            pointer_position: [0, 0],
            task_time: 0,
            task_origin: 0,
            auto_select: 0,
            scene_animation_enabled: 1,
            fullscreen: 0,
            fullscreen_mw_pos: 0,
            string_values: &mut string_values,
            next_task_id: &mut next_task_id,
            task_parents: &mut task_parents,
            scene_nodes: &mut scene_nodes,
            image_handles: &mut image_handles,
            image_resources: &mut BTreeMap::new(),
            backlogs: &mut BTreeMap::new(),
            canvas: None,
            back_buffer: None,
            particles: &mut particles,
            record_stacks: &mut BTreeMap::new(),
            input_task: &mut DesktopInputTask::default(),
            task_vms: &mut BTreeMap::new(),
            script_replacements: &script_replacements,
            system_config_blocks: &mut [vec![0; 5628], vec![0; 72], vec![0; 32], vec![0; 12]],
            css_handles: &mut css_handles,
            integer_maps: &mut integer_maps,
            data_services: &mut data_services::DataServices::default(),
            cst_contexts: &mut cst_contexts,
            motion_controllers: &mut motion_controllers,
            motion_snapshots: &mut motion_snapshots,
            save_root: &temp.0.join("save"),
            message_wait_address: &mut message_wait_address,
            audio_host: &mut audio_host,
            movie_host: None,
            movie_playbacks: &mut BTreeMap::new(),
            movie_resources: &mut BTreeMap::new(),
            sound_channels: &mut sound_channels,
            sound_groups: &[],
            sound_config: &DesktopSoundConfig::default(),
            script_random: &mut script_random,
            menu_item_requests: &mut Vec::new(),
            debug_mode: 0,
            sound_config_enabled: 0,
        };

        let context = match host.call(323, &[], &mut vm).unwrap() {
            NativeReturn::Value(handle) => handle,
            result => panic!("unexpected CST context creation result: {result:?}"),
        };
        let container = match host.call(238, &7u32.to_le_bytes(), &mut vm).unwrap() {
            NativeReturn::Value(handle) => handle,
            result => panic!("unexpected CSS creation result: {result:?}"),
        };
        let mut name = [0; 32];
        name[..4].copy_from_slice(b"test");
        host.cst_contexts
            .get_mut(&context)
            .unwrap()
            .registrations
            .push(CstRegistration {
                name,
                value: 42,
                attachments: vec![[0x33; 72]],
                data: b"payload".to_vec(),
            });

        let mut save_args = Vec::new();
        for word in [container, 99, context, 0, 1] {
            save_args.extend_from_slice(&word.to_le_bytes());
        }
        assert_eq!(
            host.call(385, &save_args, &mut vm).unwrap(),
            NativeReturn::Value(1)
        );
        host.cst_contexts
            .get_mut(&context)
            .unwrap()
            .registrations
            .clear();
        let mut load_args = Vec::new();
        for word in [container, 99, context, 0] {
            load_args.extend_from_slice(&word.to_le_bytes());
        }
        // Registration data restoration dereferences the absent second
        // original input block. Diagnose it without changing the table.
        assert!(host.call(386, &load_args, &mut vm).is_err());
        assert!(host.cst_contexts[&context].registrations.is_empty());
        load_args[12..16].copy_from_slice(&1u32.to_le_bytes());
        assert_eq!(
            host.call(386, &load_args, &mut vm).unwrap(),
            NativeReturn::Value(1)
        );
        let restored = &host.cst_contexts.get(&context).unwrap().history[0];
        assert_eq!(restored.value, 42);
        assert_eq!(restored.attachments, vec![[0x33; 72]]);
        assert_eq!(restored.data, b"payload");

        let legacy_args: Vec<u8> = [container, 100, context, 1]
            .into_iter().flat_map(u32::to_le_bytes).collect();
        assert_eq!(host.call(385, &legacy_args, &mut vm).unwrap(), NativeReturn::Value(1));
        let legacy = host.css_handles[&container].record(100).unwrap();
        assert_eq!(legacy.len(), 40 + 72);
        assert_eq!(&legacy[32..40], &[42, 0, 0, 0, 1, 0, 0, 0]);
        assert_eq!(&legacy[40..], &[0x33; 72]);
        assert_eq!(host.call(386, &legacy_args, &mut vm).unwrap(), NativeReturn::Value(1));
        assert!(host.cst_contexts[&context].history[0].data.is_empty());

        let empty_args: Vec<u8> = [container, 101, context, 0]
            .into_iter().flat_map(u32::to_le_bytes).collect();
        assert_eq!(host.call(385, &empty_args, &mut vm).unwrap(), NativeReturn::Value(1));
        assert_eq!(host.css_handles[&container].record(101), Some(&[][..]));
        let mut clear_history_args = empty_args;
        clear_history_args[12..16].copy_from_slice(&1u32.to_le_bytes());
        assert_eq!(host.call(386, &clear_history_args, &mut vm).unwrap(), NativeReturn::Value(1));
        assert!(host.cst_contexts[&context].history.is_empty());
    }

    #[test]
    fn pointer_messages_follow_the_scaled_logical_canvas_and_reject_invalid_viewports() {
        assert_eq!(
            pointer_message(2, Some([400.0, 300.0]), 800, 600),
            Some([2, 400, 300, 0])
        );
        assert_eq!(
            pointer_message(3, Some([800.0, 450.0]), 1600, 900),
            Some([3, 400, 300, 0])
        );
        assert_eq!(
            pointer_message(2, Some([1599.0, 899.0]), 1600, 900),
            None
        );
        assert_eq!(pointer_message(2, Some([1399.0, 899.0]), 1600, 900),
            Some([2, 799, 599, 0]));
        assert_eq!(pointer_message(2, None, 800, 600), None);
        assert_eq!(pointer_message(2, Some([0.0, 0.0]), 0, 600), None);
        assert_eq!(pointer_message(2, Some([800.0, 10.0]), 800, 600), None);
        assert_eq!(pointer_message(2, Some([f64::NAN, 10.0]), 800, 600), None);
    }

    #[test]
    fn kcs_string_compare_uses_signed_bytes_and_case_sensitive_ordering() {
        assert_eq!(compare_kcs_strings(b"apple", b"apple"), 0);
        assert_eq!(compare_kcs_strings(b"apple", b"Apple"), 32);
        assert_eq!(compare_kcs_strings(b"ab", b"abc"), -99);
        assert_eq!(compare_kcs_strings(&[0x80], &[0x7f]), -255);
    }

    #[test]
    fn native_1063_maps_cp932_extensions_and_preserves_output_when_too_large() {
        let program = KcsProgram::from_code(&[0x3b], vec![0; 4096], 0).unwrap();
        let mut vm = Vm::from_program(program).unwrap();
        vm.write(64, &[0x80, 0xa0, 0xfd, 0xfe, 0xff, 0]).unwrap();
        vm.write(96, &[0x82, 0]).unwrap();
        let mut short_source = vec![b'A'; 255];
        short_source.push(0);
        vm.write(128, &short_source).unwrap();
        let mut long_source = vec![b'A'; 256];
        long_source.push(0);
        vm.write(2400, &long_source).unwrap();
        assert_eq!(vm.read_string(128).unwrap().len(), 255);
        assert_eq!(vm.read_string(2400).unwrap().len(), 256);
        vm.write(1024, &[0x5a; 256]).unwrap();
        vm.write(1600, &[0x5a; 256]).unwrap();

        let make_arguments =
            |target: u32, source: u32| [target.to_le_bytes(), source.to_le_bytes()].concat();
        assert_eq!(
            cp932_utf8_native(&mut vm, &make_arguments(512, 64)).unwrap(),
            NativeReturn::Continue
        );
        assert_eq!(
            vm.read_string(512).unwrap(),
            "\u{80}\u{f8f0}\u{f8f1}\u{f8f2}\u{f8f3}".as_bytes()
        );
        assert_eq!(
            cp932_utf8_native(&mut vm, &make_arguments(768, 96)).unwrap(),
            NativeReturn::Continue
        );
        assert_eq!(vm.read_string(768).unwrap(), "\u{30fb}".as_bytes());

        assert_eq!(
            cp932_utf8_native(&mut vm, &make_arguments(1024, 128)).unwrap(),
            NativeReturn::Continue
        );
        let mut expected = vec![b'A'; 255];
        expected.push(0);
        assert_eq!(vm.read(1024, 256).unwrap(), expected);

        assert_eq!(vm.read(1600, 4).unwrap(), vec![0x5a; 4]);
        assert_eq!(cp932_to_utf8(&vm.read_string(2400).unwrap()).len(), 256);
        assert_eq!(
            cp932_utf8_native(&mut vm, &make_arguments(1600, 2400)).unwrap(),
            NativeReturn::Continue
        );
        assert_eq!(vm.read(1600, 256).unwrap(), vec![0x5a; 256]);
        assert!(
            cp932_utf8_native(&mut vm, &make_arguments(0, 64)[..7]).is_err(),
            "native 1063 requires exactly two DWORD arguments"
        );
    }

    #[test]
    fn cst_next_transfers_to_a_resource_scene() {
        let temp = TempDir::new();
        fs::write(
            temp.0.join("main.cst"),
            packed_cst(&[vec![(0x30, b"next second.cst")]]),
        )
        .unwrap();
        fs::write(
            temp.0.join("second.cst"),
            packed_cst(&[vec![(0x21, b"Speaker"), (0x20, b"Message")]]),
        )
        .unwrap();

        let mut player = HeadlessPlayer::open(&temp.0).unwrap();
        player.load_entry("main.cst").unwrap();
        player.run_frames(2, 100).unwrap();
        assert!(player.events().iter().any(|event| {
            event.kind == "cst-scene-transfer" && event.detail.contains("second.cst")
        }));
        assert!(
            player
                .events()
                .iter()
                .any(|event| event.kind == "cst-speaker" && event.detail.contains("Speaker"))
        );
        assert!(
            player
                .events()
                .iter()
                .any(|event| event.kind == "cst-message" && event.detail.contains("Message"))
        );
        assert!(
            player
                .frame_snapshot()
                .rgba()
                .chunks_exact(4)
                .any(|pixel| pixel[..3] != [0, 0, 0])
        );
    }

    #[test]
    fn cst_call_and_return_restore_the_calling_scene() {
        let temp = TempDir::new();
        fs::write(
            temp.0.join("main.cst"),
            packed_cst(&[vec![(0x30, b"call child.cst"), (0x20, b"After return")]]),
        )
        .unwrap();
        fs::write(
            temp.0.join("child.cst"),
            packed_cst(&[vec![(0x30, b"return")]]),
        )
        .unwrap();

        let mut player = HeadlessPlayer::open(&temp.0).unwrap();
        player.load_entry("main.cst").unwrap();
        player.run_frames(1, 100).unwrap();
        assert!(
            player.events().iter().any(|event| {
                event.kind == "cst-scene-transfer" && event.detail.contains("call")
            })
        );
        assert!(
            player
                .events()
                .iter()
                .any(|event| event.kind == "cst-return")
        );
        assert!(
            player.events().iter().any(|event| {
                event.kind == "cst-message" && event.detail.contains("After return")
            })
        );
    }

    #[test]
    fn cst_assignments_conditions_and_string_references_execute() {
        let temp = TempDir::new();
        let resources = ResourceIndex::scan(&temp.0).unwrap();
        let document = CstDocument {
            records: vec![
                cs2_script::CstRecord {
                    kind: 0x30,
                    text: b"#4 = 1".to_vec(),
                },
                cs2_script::CstRecord {
                    kind: 0x30,
                    text: b"if (#4==1) str 2 hello world".to_vec(),
                },
                cs2_script::CstRecord {
                    kind: 0x20,
                    text: b"Hi $str2".to_vec(),
                },
                cs2_script::CstRecord {
                    kind: 0x02,
                    text: Vec::new(),
                },
            ],
            inputs: vec![0..4],
        };
        let mut cst = CstRuntime::new(
            document,
            "memory.cst".into(),
            ScriptTextEncoding::Utf8,
            None,
        );
        let mut shared = Vec::new();
        let mut images = BTreeMap::new();
        let mut events = Vec::new();
        cst.advance(1, &resources, &mut shared, &mut images, &mut events)
            .unwrap();

        assert_eq!(shared[4], 1);
        assert!(
            events
                .iter()
                .any(|event| event.kind == "cst-string" && event.detail.contains("hello world"))
        );
        assert!(events.iter().any(|event| {
            event.kind == "cst-message" && event.detail.contains("Hi hello world")
        }));
        assert!(cst.waiting);
    }

    #[test]
    fn cstl_replaces_name_and_message_with_language_text() {
        let temp = TempDir::new();
        fs::write(
            temp.0.join("main.cst"),
            packed_cst(&[vec![(0x21, b"Base"), (0x20, b"Base text"), (0x02, b"")]]),
        )
        .unwrap();
        fs::write(
            temp.0.join("main.cstl"),
            packed_cstl("Localized name", "Localized text"),
        )
        .unwrap();

        let mut player = HeadlessPlayer::open(&temp.0).unwrap();
        player.set_script_encoding(ScriptTextEncoding::Utf8);
        player.load_entry("main.cst").unwrap();
        player.run_frames(1, 100).unwrap();
        assert!(player
            .events()
            .iter()
            .any(|event| event.kind == "cst-speaker" && event.detail.contains("Localized name")));
        assert!(player
            .events()
            .iter()
            .any(|event| event.kind == "cst-message" && event.detail.contains("Localized text")));
    }

    #[test]
    fn cst_fselect_collects_choices_and_transfers_on_indexed_input() {
        let temp = TempDir::new();
        fs::write(
            temp.0.join("main.cst"),
            packed_cst(&[vec![
                (0x30, b"fselect menu"),
                (0x30, b"1 left.cst Left"),
                (0x30, b"2 right.cst Right"),
                (0x02, b""),
            ]]),
        )
        .unwrap();
        fs::write(
            temp.0.join("right.cst"),
            packed_cst(&[vec![(0x20, b"Right route")]]),
        )
        .unwrap();

        let mut player = HeadlessPlayer::open(&temp.0).unwrap();
        player.load_entry("main.cst").unwrap();
        player.run_frames(1, 100).unwrap();
        assert!(
            player
                .events()
                .iter()
                .any(|event| event.kind == "cst-choices" && event.detail.contains("Right"))
        );
        player.queue_message([2, 1, 0, 0]);
        player.run_frames(1, 100).unwrap();
        assert!(player.events().iter().any(|event| {
            event.kind == "cst-choice-transfer" && event.detail.contains("right.cst")
        }));
        assert!(
            player
                .events()
                .iter()
                .any(|event| event.kind == "cst-message" && event.detail.contains("Right route"))
        );
    }

    #[test]
    fn cst_fselect_filters_conditional_rows_before_cstl_indexing() {
        let temp = TempDir::new();
        fs::write(
            temp.0.join("main.cst"),
            packed_cst(&[vec![
                (0x30, b"fselect menu"),
                (0x30, b"if (0) 1 left.cst Hidden"),
                (0x30, b"2 right.cst Visible"),
                (0x02, b""),
            ]]),
        )
        .unwrap();

        let mut cstl = b"CSTL".to_vec();
        cstl.extend_from_slice(&0u32.to_le_bytes());
        fn ff(value: usize, output: &mut Vec<u8>) {
            let mut value = value;
            while value >= 255 {
                output.push(255);
                value -= 255;
            }
            output.push(value as u8);
        }
        fn string(value: &str, output: &mut Vec<u8>) {
            ff(value.len(), output);
            output.extend_from_slice(value.as_bytes());
        }
        ff(1, &mut cstl);
        string("zh", &mut cstl);
        ff(1, &mut cstl);
        string("", &mut cstl);
        string("本地化选项", &mut cstl);
        fs::write(temp.0.join("main.cstl"), cstl).unwrap();

        let mut player = HeadlessPlayer::open(&temp.0).unwrap();
        player.set_script_encoding(ScriptTextEncoding::Utf8);
        player.load_entry("main.cst").unwrap();
        player.run_frames(1, 100).unwrap();
        assert!(player.events().iter().any(|event| {
            event.kind == "cst-choices" && event.detail.contains("本地化选项")
        }));
    }

    #[test]
    fn native_157_repeat_counts_select_single_finite_or_continuous_playback() {
        assert!(native_play_looped(0));
        assert!(!native_play_looped(1));
        assert!(!native_play_looped(2));

        let mut state = SoundChannelState {
            resource: Some("se001.ogg".to_string()),
            handle: Some(41),
            paused: true,
            ..SoundChannelState::default()
        };
        assert!(should_resume_playback(&state));
        state.paused = false;
        assert!(!should_resume_playback(&state));
    }

    #[test]
    fn system_natives_preserve_configuration_menu_and_status_values() {
        let temp = TempDir::new();
        let resources = ResourceIndex::scan(&temp.0).unwrap();
        let mut events = Vec::new();
        let mut xml_handles = BTreeMap::new();
        let mut next_xml_handle = 1;
        let mut shared_values = Vec::new();
        let mut current_parameter = 0;
        let mut string_values = BTreeMap::new();
        let mut next_task_id = 1;
        let mut task_parents = BTreeMap::new();
        let mut scene_nodes = BTreeMap::new();
        let mut image_handles = BTreeMap::new();
        let script_replacements = BTreeMap::new();
        let mut particles = BTreeMap::new();
        let mut css_handles = BTreeMap::new();
        let mut integer_maps = BTreeMap::new();
        let mut cst_contexts = BTreeMap::new();
        let mut motion_controllers = BTreeMap::new();
        let mut motion_snapshots = BTreeMap::new();
        let mut message_wait_address = None;
        let mut audio_host =
            OggAudioHost::new(|_| Err(RuntimeError::Host("unexpected audio resolution".into())));
        let mut sound_channels = BTreeMap::new();
        let mut script_random = ScriptRandom::default();
        let mut vm =
            Vm::from_program(KcsProgram::from_code(&[0x3b], vec![0; 7000], 0).unwrap()).unwrap();
        let mut system_config_blocks = [
            vec![0x11; 5628],
            vec![0x22; 72],
            vec![0x33; 32],
            vec![0x44; 12],
        ];
        let mut menu_item_requests = Vec::new();
        let mut host = LoggingHost {
            events: &mut events,
            frame: 0,
            resources: &resources,
            xml_handles: &mut xml_handles,
            next_xml_handle: &mut next_xml_handle,
            configured_shared_slots: 0,
            text_encoding: ScriptTextEncoding::Cp932,
            shared_values: &mut shared_values,
            current_parameter: &mut current_parameter,
            current_task_id: 0x8123_4567,
            pointer_position: [0, 0],
            task_time: 0,
            task_origin: 0,
            auto_select: 0,
            scene_animation_enabled: 1,
            fullscreen: 0,
            fullscreen_mw_pos: 0,
            string_values: &mut string_values,
            next_task_id: &mut next_task_id,
            task_parents: &mut task_parents,
            scene_nodes: &mut scene_nodes,
            image_handles: &mut image_handles,
            image_resources: &mut BTreeMap::new(),
            backlogs: &mut BTreeMap::new(),
            canvas: None,
            back_buffer: None,
            particles: &mut particles,
            record_stacks: &mut BTreeMap::new(),
            input_task: &mut DesktopInputTask::default(),
            task_vms: &mut BTreeMap::new(),
            script_replacements: &script_replacements,
            system_config_blocks: &mut system_config_blocks,
            css_handles: &mut css_handles,
            integer_maps: &mut integer_maps,
            data_services: &mut data_services::DataServices::default(),
            cst_contexts: &mut cst_contexts,
            motion_controllers: &mut motion_controllers,
            motion_snapshots: &mut motion_snapshots,
            save_root: &temp.0.join("save"),
            message_wait_address: &mut message_wait_address,
            audio_host: &mut audio_host,
            movie_host: None,
            movie_playbacks: &mut BTreeMap::new(),
            movie_resources: &mut BTreeMap::new(),
            sound_channels: &mut sound_channels,
            sound_groups: &[],
            sound_config: &DesktopSoundConfig::default(),
            script_random: &mut script_random,
            menu_item_requests: &mut menu_item_requests,
            debug_mode: 0x1234_5678,
            sound_config_enabled: 0x8765_4321,
        };

        assert_eq!(
            host.call(6, &0xdead_beefu32.to_le_bytes(), &mut vm)
                .unwrap(),
            NativeReturn::Value(0xdead_beef)
        );
        vm.write(200, b"copy").unwrap();
        let mut copy_arguments = Vec::new();
        for word in [300_u32, 200, 4] {
            copy_arguments.extend_from_slice(&word.to_le_bytes());
        }
        assert_eq!(
            host.call(35, &copy_arguments, &mut vm).unwrap(),
            NativeReturn::Continue
        );
        assert_eq!(vm.read(300, 4).unwrap(), b"copy");
        vm.write(100, b"missing.ogg\0").unwrap();
        let mut grouped_load = Vec::new();
        for word in [0_u32, 0, 100, 0, 1, 0] {
            grouped_load.extend_from_slice(&word.to_le_bytes());
        }
        assert!(matches!(
            host.call(936, &grouped_load, &mut vm),
            Err(cs2_script::ScriptError::InvalidStructuredText {
                message: "native 936 sound groups are not configured",
                ..
            })
        ));
        assert!(matches!(
            host.call(937, &[], &mut vm),
            Ok(NativeReturn::Continue)
        ));
        assert_eq!(
            host.call(68, &[], &mut vm).unwrap(),
            NativeReturn::Value(0x8123_4567)
        );
        for debug_mode in [0, 0x1234_5678, u32::MAX] {
            host.debug_mode = debug_mode;
            assert_eq!(
                host.call(465, &[], &mut vm).unwrap(),
                NativeReturn::Value(debug_mode)
            );
        }
        assert_eq!(
            host.call(938, &[], &mut vm).unwrap(),
            NativeReturn::Value(0x8765_4321)
        );
        vm.write(64, b"alpha\0").unwrap();
        let map_handle = match host.call(515, &[], &mut vm).unwrap() {
            NativeReturn::Value(value) => value,
            other => panic!("unexpected integer-map handle result: {other:?}"),
        };
        let mut map_set = Vec::new();
        for value in [map_handle, 64, 42] {
            map_set.extend_from_slice(&value.to_le_bytes());
        }
        assert_eq!(
            host.call(517, &map_set, &mut vm).unwrap(),
            NativeReturn::Continue
        );
        let mut map_get = Vec::new();
        for value in [map_handle, 64] {
            map_get.extend_from_slice(&value.to_le_bytes());
        }
        assert_eq!(
            host.call(518, &map_get, &mut vm).unwrap(),
            NativeReturn::Value(42)
        );
        assert_eq!(
            host.call(516, &map_handle.to_le_bytes(), &mut vm).unwrap(),
            NativeReturn::Continue
        );
        assert_eq!(
            host.call(393, &7u32.to_le_bytes(), &mut vm).unwrap(),
            NativeReturn::Continue
        );
        assert_eq!(host.menu_item_requests.as_slice(), &[(0x8123_4567, 7)]);
        let mut addresses = Vec::new();
        for address in [0_u32, 6000, 6100, 6200] {
            addresses.extend_from_slice(&address.to_le_bytes());
        }
        assert_eq!(
            host.call(359, &addresses, &mut vm).unwrap(),
            NativeReturn::Continue
        );
        assert_eq!(vm.read(0, 5628).unwrap(), vec![0x11; 5628]);
        assert_eq!(vm.read(6000, 72).unwrap(), vec![0x22; 72]);
        assert_eq!(vm.read(6100, 32).unwrap(), vec![0x33; 32]);
        assert_eq!(vm.read(6200, 12).unwrap(), vec![0x44; 12]);

        vm.write(0, &vec![0xa1; 5628]).unwrap();
        vm.write(6000, &vec![0xb2; 72]).unwrap();
        vm.write(6100, &vec![0xc3; 32]).unwrap();
        let mut writeback_addresses = Vec::new();
        for address in [0_u32, 6000, 6100] {
            writeback_addresses.extend_from_slice(&address.to_le_bytes());
        }
        assert_eq!(
            host.call(360, &writeback_addresses, &mut vm).unwrap(),
            NativeReturn::Continue
        );
        assert_eq!(host.system_config_blocks[0], vec![0xa1; 5628]);
        assert_eq!(host.system_config_blocks[1], vec![0xb2; 72]);
        assert_eq!(host.system_config_blocks[2], vec![0xc3; 32]);
        assert_eq!(host.system_config_blocks[3], vec![0x44; 12]);
        assert_eq!(
            host.events
                .iter()
                .filter(|event| event.kind == "native-call")
                .count(),
            16
        );
    }

    #[test]
    fn particle_natives_cover_create_control_query_release_and_unknown_plugin() {
        let temp = TempDir::new();
        let resources = ResourceIndex::scan(&temp.0).unwrap();
        let mut events = Vec::new();
        let mut xml_handles = BTreeMap::new();
        let mut next_xml_handle = 1;
        let mut shared_values = Vec::new();
        let mut current_parameter = 0;
        let mut string_values = BTreeMap::new();
        let mut next_task_id = 1;
        let mut task_parents = BTreeMap::new();
        let mut scene_nodes = BTreeMap::new();
        let mut image_handles = BTreeMap::new();
        let script_replacements = BTreeMap::new();
        let mut particles = BTreeMap::new();
        let mut css_handles = BTreeMap::new();
        let mut integer_maps = BTreeMap::new();
        let mut cst_contexts = BTreeMap::new();
        let mut motion_controllers = BTreeMap::new();
        let mut motion_snapshots = BTreeMap::new();
        let mut message_wait_address = None;
        let mut audio_host =
            OggAudioHost::new(|_| Err(RuntimeError::Host("unexpected audio resolution".into())));
        let mut sound_channels = BTreeMap::new();
        let mut script_random = ScriptRandom::default();
        let mut system_config_blocks = [vec![0; 5628], vec![0; 72], vec![0; 32], vec![0; 12]];
        let mut menu_item_requests = Vec::new();
        let mut host = LoggingHost {
            events: &mut events,
            frame: 3,
            resources: &resources,
            xml_handles: &mut xml_handles,
            next_xml_handle: &mut next_xml_handle,
            configured_shared_slots: 0,
            text_encoding: ScriptTextEncoding::Cp932,
            shared_values: &mut shared_values,
            current_parameter: &mut current_parameter,
            current_task_id: 7,
            pointer_position: [0, 0],
            task_time: 0,
            task_origin: 0,
            auto_select: 0,
            scene_animation_enabled: 1,
            fullscreen: 0,
            fullscreen_mw_pos: 0,
            string_values: &mut string_values,
            next_task_id: &mut next_task_id,
            task_parents: &mut task_parents,
            scene_nodes: &mut scene_nodes,
            image_handles: &mut image_handles,
            image_resources: &mut BTreeMap::new(),
            backlogs: &mut BTreeMap::new(),
            canvas: None,
            back_buffer: None,
            particles: &mut particles,
            record_stacks: &mut BTreeMap::new(),
            input_task: &mut DesktopInputTask::default(),
            task_vms: &mut BTreeMap::new(),
            script_replacements: &script_replacements,
            system_config_blocks: &mut system_config_blocks,
            css_handles: &mut css_handles,
            integer_maps: &mut integer_maps,
            data_services: &mut data_services::DataServices::default(),
            cst_contexts: &mut cst_contexts,
            motion_controllers: &mut motion_controllers,
            motion_snapshots: &mut motion_snapshots,
            save_root: &temp.0.join("save"),
            message_wait_address: &mut message_wait_address,
            audio_host: &mut audio_host,
            movie_host: None,
            movie_playbacks: &mut BTreeMap::new(),
            movie_resources: &mut BTreeMap::new(),
            sound_channels: &mut sound_channels,
            sound_groups: &[],
            sound_config: &DesktopSoundConfig::default(),
            script_random: &mut script_random,
            menu_item_requests: &mut menu_item_requests,
            debug_mode: 0,
            sound_config_enabled: 0,
        };
        let program = cs2_script::KcsProgram::from_code(&[0x3b], vec![0; 1024], 512).unwrap();
        let mut vm = Vm::from_program(program).unwrap();
        vm.write(64, b"particle.kcs\0").unwrap();
        vm.write(96, b"particle.hg3\0").unwrap();
        vm.write(512, &[0x5a; 128]).unwrap();

        let mut load = Vec::new();
        for word in [42_u32, 64, 96, 3, 0x4120_0000, 512] {
            load.extend_from_slice(&word.to_le_bytes());
        }
        assert_eq!(
            host.call(271, &load, &mut vm).unwrap(),
            NativeReturn::Value(1)
        );
        assert_eq!(host.particles[&42].program_name, "particle.kcs");
        assert_eq!(host.particles[&42].image_name, "particle.hg3");
        assert_eq!(host.particles[&42].depth_bits, 0x4120_0000);
        assert_eq!(host.particles[&42].parameters, [0x5a; 128]);

        assert_eq!(
            host.call(274, &42_u32.to_le_bytes(), &mut vm).unwrap(),
            NativeReturn::Value(0)
        );
        assert_eq!(
            host.call(272, &42_u32.to_le_bytes(), &mut vm).unwrap(),
            NativeReturn::Continue
        );
        assert_eq!(
            host.call(274, &42_u32.to_le_bytes(), &mut vm).unwrap(),
            NativeReturn::Value(1)
        );
        assert_eq!(
            host.call(273, &42_u32.to_le_bytes(), &mut vm).unwrap(),
            NativeReturn::Continue
        );
        assert_eq!(
            host.call(274, &42_u32.to_le_bytes(), &mut vm).unwrap(),
            NativeReturn::Value(0)
        );

        let mut set_parameters = 42_u32.to_le_bytes().to_vec();
        set_parameters.extend_from_slice(&512_u32.to_le_bytes());
        vm.write(512, &[0xa5; 128]).unwrap();
        host.call(275, &set_parameters, &mut vm).unwrap();
        assert_eq!(host.particles[&42].parameters, [0xa5; 128]);

        let mut opacity = 42_u32.to_le_bytes().to_vec();
        opacity.extend_from_slice(&127_u32.to_le_bytes());
        host.call(426, &opacity, &mut vm).unwrap();
        assert_eq!(host.particles[&42].opacity, 127);
        let mut source = 42_u32.to_le_bytes().to_vec();
        source.extend_from_slice(&99_u32.to_le_bytes());
        host.call(427, &source, &mut vm).unwrap();
        assert_eq!(host.particles[&42].transform_source, 99);
        host.apply_particle_command(42, 591, Some(4));
        assert_eq!(host.particles[&42].coordinate_group, 4);
        host.apply_particle_command(42, 592, None);
        assert_eq!(host.particles[&42].coordinate_group, 0);
        host.apply_particle_command(42, 323, None);
        assert!(!host.particles.contains_key(&42));

        vm.write(64, b"unknown.ppi\0").unwrap();
        let error = host.call(693, &load, &mut vm).unwrap_err();
        assert!(matches!(
            error,
            cs2_script::ScriptError::InvalidStructuredText {
                message: "particle plugin type or version is unknown",
                ..
            }
        ));
        assert!(host.events.iter().any(|event| {
            event.kind == "particle-error" && event.detail.contains("unknown.ppi")
        }));
    }

    #[test]
    fn headless_player_scans_and_advances_without_an_executable() {
        let temp = TempDir::new();
        fs::write(temp.0.join("readme.txt"), b"fixture").unwrap();
        let mut player = HeadlessPlayer::open(&temp.0).unwrap();
        player.run_frames(3, 8).unwrap();
        assert_eq!(player.state().clock.frame, 3);
        assert_eq!(player.report().current_task_id, 0);
        assert!(
            player
                .events()
                .iter()
                .any(|event| event.kind == "logic-frame")
        );
        assert!(player.vm.is_none());
    }

    #[test]
    fn fes_input_routes_unbound_focus_during_script_wait() {
        let temp = TempDir::new();
        let mut player = HeadlessPlayer::open_with_save_root(&temp.0, temp.0.join("save")).unwrap();
        let owner = player.current_task_id;
        let document = FesDocument {
            sections: vec![cs2_script::FesSection {
                header: b"START".to_vec(),
                lines: vec![cs2_script::FesLine { offset: 0, bytes: b"wait 20".to_vec() }],
            }],
        };
        player
            .with_host(|host| Ok(host.begin_direct_fes_entry(owner, document, Default::default())?))
            .unwrap();
        player.run_frames(1, 10_000).unwrap();
        assert!(matches!(player.frame_phase, PlayerFramePhase::Start));
        let region = player.input_task.regions.iter().find(|region| {
            player.scene_nodes[&region.id()].name == "low button"
        }).unwrap().id();
        assert!(player.input_button_state(region).is_none());
        let driver = player.data_services.tasks.fes.runtime(owner).unwrap().driver.as_ref().unwrap();
        assert!(matches!(driver.interpreter.pending(), Some(cs2_script::FesEvent::Wait(20))));
        assert!(!driver.initialization_pending());

        player.state.apply_input(cs2_runtime::InputEvent::MouseMove { x: 10, y: 10 });
        player.observe_host_device_activity(1);
        player.run_frames(1, 10_000).unwrap();
        assert_eq!(player.frame_phase, PlayerFramePhase::Start);
        assert_eq!(player.input_task.selected_region, Some(region));
        assert!(player.host_input_selection_routed());
        let driver = player.data_services.tasks.fes.runtime(owner).unwrap().driver.as_ref().unwrap();
        assert!(matches!(driver.interpreter.pending(), Some(cs2_script::FesEvent::Wait(20))));
    }

    #[test]
    fn fes_input_routes_bank_handles_through_button_and_auxiliary_regions() {
        let temp = TempDir::new();
        let mut player = HeadlessPlayer::open_with_save_root(&temp.0, temp.0.join("save")).unwrap();
        let owner = player.current_task_id;
        let mut offset = 0;
        let mut section = |header: &[u8], lines: &[&[u8]]| cs2_script::FesSection {
            header: header.to_vec(),
            lines: lines.iter().map(|bytes| {
                offset += 1;
                cs2_script::FesLine { offset, bytes: bytes.to_vec() }
            }).collect(),
        };
        let document = FesDocument {
            sections: vec![
                section(b"DEFINE", &[b"BUTTON button[2]"]),
                section(b"START", &[
                    b"if (_CLICK_L_==button[1])",
                    b"button[1] delete",
                    b"if (button[1]==0 && button[0]!=0)",
                    b"exit 42",
                    b"else",
                    b"exit 99",
                    b"endif",
                    b"endif",
                ]),
            ],
        };
        player.with_host(|host| {
            Ok(host.begin_direct_fes_entry(owner, document, Default::default())?)
        }).unwrap();
        player.run_frames(1, 10_000).unwrap();
        assert_eq!(player.data_services.tasks.fes.direct_exit_code(owner), None);

        let auxiliary = player.data_services.tasks.fes.platform.auxiliary_button(owner).unwrap();
        player.with_host(|host| {
            let (target, _, _) = fes_task_platform::navigation_binding(host, owner, b"button", 1)
                .expect("Start bank has a live region");
            let region = target.value as u32;
            let mut send = |sender: u32, code, key| {
                let command = cs2_runtime::TaskCommand {
                    target: owner as i32, sender: sender as i32, code, data: [sender, key, 0],
                };
                assert_eq!(host.receive_fes_button_owner(command, 10_000).unwrap(), Some(true));
            };
            send(region, 64, 1);
            for key in [0, 2] {
                send(region, 9, key);
                send(auxiliary, 9, key);
            }
            let driver = host.data_services.tasks.fes.runtime(owner).unwrap().driver.as_ref().unwrap();
            assert_eq!(driver.interpreter.named["_CLICK_L_"][&0], region as i32);
            assert_eq!(driver.interpreter.named["button"][&1], region as i32);
            Ok(())
        }).unwrap();

        player.run_frames(1, 10_000).unwrap();
        assert_eq!(player.data_services.tasks.fes.direct_exit_code(owner), Some(42));
    }

    #[test]
    fn acceptance_startup_waits_for_real_input_and_executes_native_21() {
        let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(3)
            .expect("desktop crate should be inside the simulator workspace")
            .to_path_buf();
        let v401 = workspace_root.join("cs2_full_v401");
        let newton = workspace_root.join("牛顿与苹果树");
        let neko = workspace_root
            .join("[170526][NEKO WORKs] ネコぱら vol.3 steam版 (files)/nekopara_vol3");
        if !v401.is_dir() || !newton.is_dir() || !neko.is_dir() {
            return;
        }

        let mut player = HeadlessPlayer::open(&v401).unwrap();
        let entry = player.default_entry().unwrap().to_owned();
        player.load_entry(entry).unwrap();
        player.run_frames(12, 10_000).unwrap();
        assert!(player.message_wait_address.is_some());
        assert!(
            player
                .events()
                .iter()
                .any(|event| event.kind == "input-wait")
        );
        assert!(
            !player
                .events()
                .iter()
                .any(|event| event.kind == "message-resume")
        );

        player.queue_message([2, 0, 0, 0]);
        player.queue_message([2, 0, 0, 0]);
        player.run_frames(2, 10_000).unwrap();
        assert_eq!(
            player
                .events()
                .iter()
                .filter(|event| event.kind == "message-resume")
                .count(),
            2
        );
        assert!(player.message_wait_address.is_some());

        let mut title = HeadlessPlayer::open(&v401).unwrap();
        title.load_entry("system/fes/title.fes").unwrap();
        title.run_frames(7, 10_000).unwrap();
        let title_hash_before_image = title.report().frame_hash;
        title.run_frames(3, 10_000).unwrap();
        let title_hash_after_image = title.report().frame_hash;
        assert_ne!(
            title_hash_before_image, title_hash_after_image,
            "FES image command should change the final RGBA frame"
        );
        title.run_frames(4, 10_000).unwrap();
        let title_fes = title.fes.as_ref().expect("title FES should be active");
        assert!(
            title_fes
                .media
                .get("img_bg")
                .and_then(|layer| layer.image.as_ref())
                .is_some(),
            "title FES should decode sys_title.hg3 for img_bg"
        );
        assert!(
            title.events().iter().any(|event| {
                event.kind == "fes-media" && event.detail.contains("object=img_bg")
            })
        );
        assert!(title.fes.as_ref().unwrap().exit_code.is_none());
        for _ in 0..60 {
            if title.fes.as_ref().is_some_and(|fes| {
                fes.media
                    .get("btn1[0]")
                    .is_some_and(|button| button.enabled)
            }) {
                break;
            }
            title.run_frames(1, 10_000).unwrap();
        }
        assert!(
            title.fes.as_ref().unwrap().exit_code.is_none(),
            "title wait section should remain active across frame boundaries"
        );
        let button_point = {
            let fes = title.fes.as_ref().unwrap();
            let button = fes.media.get("btn1[0]").unwrap();
            let image = button.image.as_ref().expect("visible title button image");
            let position = button.draw_position(&fes.planes);
            Point::new(
                position.x + image.width() as i32 / 2,
                position.y + image.height() as i32 / 2,
            )
        };
        assert!(
            title
                .fes
                .as_ref()
                .unwrap()
                .media
                .get("btn1[0]")
                .unwrap()
                .enabled
        );
        assert_eq!(
            title.fes.as_ref().unwrap().hit_test_button(button_point),
            Some("btn1[0]".to_string())
        );
        title.state.apply_input(InputEvent::MouseMove {
            x: button_point.x,
            y: button_point.y,
        });
        title.queue_fes_event(2);
        title.run_frames(1, 10_000).unwrap();
        let title_section = title
            .fes
            .as_ref()
            .and_then(|fes| fes.document.sections.get(fes.section))
            .map(|section| String::from_utf8_lossy(&section.header).into_owned());
        assert_eq!(title_section.as_deref(), Some("GO_STORY"));
        assert!(
            title.events().iter().any(|event| {
                event.kind == "fes-event-command" && event.detail.contains("setid")
            })
        );
        assert_eq!(
            title
                .events()
                .iter()
                .filter(|event| {
                    event.kind == "fes-event-command" && event.detail == "@ setid 2"
                })
                .count(),
            1,
            "one queued click should dispatch one FES PUSH_L handler"
        );

        let mut newton = HeadlessPlayer::open(&newton).unwrap();
        assert!(newton.sound_groups.len() >= 4);
        assert_eq!(newton.sound_config_enabled, 1);
        let entry = newton.default_entry().unwrap().to_owned();
        newton.load_entry(entry).unwrap();
        newton.run_frames(12, 10_000).unwrap();
        assert!(newton.message_wait_address.is_some());
        assert!(!newton.events().iter().any(|event| {
            event.kind == "vm-error"
                || event.kind == "native-call" && event.detail.contains("is not implemented")
        }));

        let mut group_vm =
            Vm::from_program(KcsProgram::from_code(&[0x3b], vec![0; 256], 0).unwrap()).unwrap();
        group_vm.write(64, b"B_lav_004_004.ogg\0").unwrap();
        let mut group_arguments = Vec::new();
        for word in [0_u32, 0, 64, 0, 1, 60] {
            group_arguments.extend_from_slice(&word.to_le_bytes());
        }
        {
            let mut group_host = LoggingHost {
                events: &mut newton.events,
                frame: 0,
                resources: &newton.scan.resources,
                xml_handles: &mut newton.xml_handles,
                next_xml_handle: &mut newton.next_xml_handle,
                configured_shared_slots: newton.configured_shared_slots,
                text_encoding: newton.script_encoding,
                shared_values: &mut newton.shared_values,
                current_parameter: &mut newton.current_parameter,
                current_task_id: 0,
                pointer_position: [0, 0],
                task_time: 0,
                task_origin: 0,
                auto_select: 0,
                scene_animation_enabled: 1,
                fullscreen: 0,
                fullscreen_mw_pos: 0,
                string_values: &mut newton.string_values,
                next_task_id: &mut newton.next_task_id,
                task_parents: &mut newton.task_parents,
                scene_nodes: &mut newton.scene_nodes,
                image_handles: &mut newton.image_handles,
                image_resources: &mut newton.image_resources,
                backlogs: &mut newton.backlogs,
                canvas: Some(&mut newton.graphics_canvas),
                back_buffer: Some(&newton.canvas),
                particles: &mut newton.particles,
                record_stacks: &mut newton.record_stacks,
                input_task: &mut newton.input_task,
                task_vms: &mut newton.task_vms,
                script_replacements: &newton.script_replacements,
                system_config_blocks: &mut newton.system_config_blocks,
                css_handles: &mut newton.css_handles,
                integer_maps: &mut newton.integer_maps,
                data_services: &mut newton.data_services,
                cst_contexts: &mut newton.cst_contexts,
                motion_controllers: &mut newton.motion_controllers,
                motion_snapshots: &mut newton.motion_snapshots,
                save_root: &newton.save_root,
                message_wait_address: &mut newton.message_wait_address,
                audio_host: &mut newton.audio_host,
                movie_host: Some(&mut newton.movie_host),
                movie_playbacks: &mut newton.movie_playbacks,
                movie_resources: &mut newton.movie_resources,
                sound_channels: &mut newton.sound_channels,
                sound_groups: &newton.sound_groups,
                sound_config: &newton.sound_config,
                script_random: &mut newton.script_random,
                menu_item_requests: &mut newton.menu_item_requests,
                debug_mode: newton.debug_mode,
                sound_config_enabled: newton.sound_config_enabled,
            };
            assert_eq!(
                group_host
                    .call(936, &group_arguments, &mut group_vm)
                    .unwrap(),
                NativeReturn::Value(0)
            );
            let mut keyed_arguments = Vec::new();
            for word in [2_u32, 0, 64, 1, 2, 60] {
                keyed_arguments.extend_from_slice(&word.to_le_bytes());
            }
            assert_eq!(
                group_host
                    .call(936, &keyed_arguments, &mut group_vm)
                    .unwrap(),
                NativeReturn::Value(15)
            );
        }
        assert_eq!(
            newton
                .sound_channels
                .get(&0)
                .and_then(|state| state.resource.as_deref()),
            Some("B_lav_004_004.ogg")
        );
        assert_eq!(
            newton
                .sound_channels
                .get(&15)
                .and_then(|state| state.resource.as_deref()),
            Some("B_lav_004_004.ogg")
        );

        let mut newton_scene = HeadlessPlayer::open(&newton.root.path).unwrap();
        newton_scene.set_script_encoding(ScriptTextEncoding::Gbk);
        newton_scene.load_entry("scene/ali_001A.cst").unwrap();
        newton_scene.run_frames(1, 10_000).unwrap();
        let first_message = newton_scene
            .events()
            .iter()
            .find(|event| event.kind == "cst-message")
            .expect("first CST block should execute a message");
        assert!(first_message.detail.contains("艾米小姐"));
        assert!(!first_message.detail.contains('\u{fffd}'));
        let frame_before_image = newton_scene.report().frame_rgba;
        newton_scene.queue_message([2, 0, 0, 0]);
        newton_scene.run_frames(1, 10_000).unwrap();
        assert!(
            newton_scene
                .events()
                .iter()
                .any(|event| event.kind == "cst-input")
        );
        assert!(newton_scene.cst_images.contains_key(&(CstImageKind::Cg, 0)));
        let frame_after_image = newton_scene.report().frame_rgba;
        assert_ne!(
            frame_before_image, frame_after_image,
            "CST image command must affect the final RGBA frame"
        );
        assert!(
            newton_scene
                .events()
                .iter()
                .any(|event| { event.kind == "cst-image" && event.detail.contains("be01_a.hg3") })
        );

        let mut neko = HeadlessPlayer::open(&neko).unwrap();
        let entry = neko.default_entry().unwrap().to_owned();
        neko.load_entry(entry).unwrap();
        neko.run_frames(2, 10_000).unwrap();
        assert!(
            neko.events().iter().any(|event| {
                event.kind == "native-call" && event.detail.starts_with("index=21 ")
            })
        );
        assert!(!neko.events().iter().any(|event| event.kind == "vm-error"));
    }

    #[test]
    fn unsupported_entry_has_an_actionable_error() {
        let temp = TempDir::new();
        fs::write(temp.0.join("data.bin"), b"not a script").unwrap();
        let mut player = HeadlessPlayer::open(&temp.0).unwrap();
        let error = player.load_entry("data.bin").unwrap_err().to_string();
        assert!(error.contains("not a supported KCS/CST/FES"));
    }
}

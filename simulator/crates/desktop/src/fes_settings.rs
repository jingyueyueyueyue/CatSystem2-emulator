//! FES getconf/setconf over the same raw option blocks used by 359/360.
//! Layout offsets are serialized ABI facts; values remain raw DWORDs. The
//! command cursor prevents replay of successful expression side effects.
use std::collections::BTreeMap;
use anyhow::{Context, Result, bail};
use cs2_formats::ConfigXml;
use cs2_runtime::language::{LanguageChangeRequest, LanguageTarget};
use cs2_runtime::message_object::MessageDefaults;
use cs2_script::ExpressionState;
use super::fes_driver::{FesDriverCommand, FesDriverEffect, FesScriptDriver};
use super::fes_tasks::FesTaskView;
use super::{FesRuntime, LoggingHost, LogicEvent, ScriptTextEncoding, trim_nul};

struct Field {
    name: &'static [u8],
    block: usize,
    offset: usize,
    xml: &'static str,
}
const FIELDS: &[Field] = &[
    field(b"messpd", 1, 0, "MESSAGE/speed"),
    field(b"autospd", 1, 4, "MESSAGE/auto_p"),
    field(b"autoind", 1, 8, "MESSAGE/ind"),
    field(b"autospd_b", 1, 12, "MESSAGE/base"),
    field(b"autospd_c", 1, 16, "MESSAGE/CHAR"),
    field(b"stillread", 1, 20, "MESSAGE/still"),
    field(b"contauto", 1, 24, "MESSAGE/cont"),
    field(b"waitpcm", 1, 28, "MESSAGE/waitpcm"),
    field(b"fullskip", 1, 32, "MESSAGE/skip_full"),
    field(b"", 1, 36, "MESSAGE/log_limit"),
    field(b"enable_readcol", 1, 40, "MESSAGE/enable_read"),
    field(b"readcol", 1, 44, "MESSAGE/read_color"),
    field(b"skip_speed", 1, 48, "MESSAGE/skipspeed"),
    field(b"contskip", 1, 52, "MESSAGE/contskip"),
    field(b"strscroll", 1, 56, "MESSAGE/wipe"),
    field(b"ctrlmode", 1, 60, "MESSAGE/ctrlmode"),
    field(b"autoleft", 1, 64, "MESSAGE/autoleft"),
    field(b"enable_readselcol", 1, 68, "MESSAGE/enable_read_sel"),
    field(b"effspd", 2, 0, "EFFECT/speed"),
    field(b"effcut", 2, 4, "EFFECT/skipdraw"),
    field(b"", 2, 8, "EFFECT/help"),
    field(b"autocur", 2, 12, "EFFECT/auto_c"),
    field(b"autodel", 2, 16, "EFFECT/auto_cd"),
    field(b"", 2, 20, "EFFECT/auto_cds"),
    field(b"dorama", 2, 24, "EFFECT/dramatic"),
    field(b"motcut", 2, 28, "EFFECT/skipmot"),
    field(b"wndcolor", 3, 0, "GAME/wndcolor"),
    field(b"dragwnd", 3, 4, "GAME/enabledrag"),
    field(b"wndframe", 3, 8, "GAME/drawwnd"),
];
const fn field(name: &'static [u8], block: usize, offset: usize, xml: &'static str) -> Field {
    Field { name, block, offset, xml }
}

#[derive(cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(super) struct FesSettingsStore {
    initialized: bool,
    pub(super) windowed_mode: Option<u32>,
    pub(super) scene_animation: u32,
    pub(super) auto_select: u32,
    active: u32,
    scene_skip: u32,
    jtalk: u32,
    fullscreen: u32,
    fullscreen_position: u32,
    fonts: [u32; 5],
    fonts_specified: [bool; 2],
    flag_ranges: [[[u32; 2]; 8]; 2],
    backlog_static: u32,
    #[serde(with = "crate::fes_tasks::state::words48")]
    system_voice: [u32; 48],
    pub(super) presets: [[u32; 2]; 32],
    pub(super) message_panel_defaults: [u32; 18],
    pub(super) effect_panel_defaults: [u32; 8],
    pub(super) window_panel_defaults: [u32; 3],
    pending: BTreeMap<(u32, usize), Vec<i32>>,
}
impl Default for FesSettingsStore {
    fn default() -> Self {
        let mut presets = [[0; 2]; 32];
        presets[..10].copy_from_slice(&[[5,7],[7,8],[8,8],[12,7],[15,5],[16,9],[17,12],[21,11],[25,10],[29,10]]);
        let mut flag_ranges = [[[u32::MAX; 2]; 8]; 2];
        flag_ranges[0][0] = [0, 512];
        flag_ranges[1][0] = [512, 512];
        Self {
            initialized: false, windowed_mode: None, scene_animation: 1, auto_select: 0,
            active: 1, scene_skip: 0, jtalk: 0, fullscreen: 0, fullscreen_position: 0,
            fonts: [1, 1, 0, 1, 0], system_voice: [1; 48], presets,
            fonts_specified: [false; 2],
            effect_panel_defaults: [0, 0, 1, 1, 1, 5, 0, 0],
            window_panel_defaults: [u32::MAX, 1, 1],
            message_panel_defaults: [4,4,0,15,5,1,0,1,0,100,0,u32::MAX,0,0,0,0,0,0],
            flag_ranges, backlog_static: 0,
            pending: BTreeMap::new(),
        }
    }
}

impl FesSettingsStore {
    pub(crate) fn capture_checkpoint(&self, kinds: &BTreeMap<u32, u32>) -> Result<Vec<u8>> {
        self.validate_checkpoint(kinds)?;
        Ok(cs2_runtime::object_graph::encode_component(self)?)
    }
    pub(crate) fn prepare_checkpoint(bytes: &[u8], kinds: &BTreeMap<u32, u32>) -> Result<Self> {
        let saved: Self = cs2_runtime::object_graph::decode_component(bytes)?;
        saved.validate_checkpoint(kinds)?;
        Ok(saved)
    }
    fn validate_checkpoint(&self, kinds: &BTreeMap<u32, u32>) -> Result<()> {
        if self.pending.len() > 65_536 || self.pending.iter()
            .any(|((owner, _), values)| kinds.get(owner) != Some(&34) || values.len() > 1_000_000)
        { bail!("FES settings checkpoint has invalid owners or expression cursors"); }
        Ok(())
    }

    pub(super) fn set_presented_fullscreen(&mut self, value: u32) {
        self.fullscreen = value;
    }

    pub(super) fn message_window_position(&self) -> u32 {
        self.fullscreen_position
    }

    pub(super) fn set_message_window_position(&mut self, value: u32) -> Result<()> {
        if value > 2 { bail!("message-window position is outside the inspected0..2 domain"); }
        self.fullscreen_position = value;
        Ok(())
    }

    pub(super) fn cst_local_flag_ranges(&self) -> [[u32; 2]; 8] {
        self.flag_ranges[0]
    }
}

impl LoggingHost<'_> {
    fn fes_message_defaults(&mut self) -> &mut MessageDefaults {
        let controls = self.data_services.fes_settings.fonts;
        self.data_services.messages.defaults.get_or_insert_with(|| MessageDefaults {
            font_name: Vec::new(), bold: controls[2] != 0, shadow: controls[1], outline: controls[0],
            glyph_parameter: Some(if controls[3] == 0 { 1 } else { controls[4] }), gradual: false,
        })
    }

    pub(super) fn apply_settings_font(&mut self, name: &str, controls: [u32; 5]) -> Result<()> {
        let units: Vec<_> = name.encode_utf16().collect();
        if units.len() > 63 || units.contains(&0) { bail!("font page face exceeds the original bounded field"); }
        let defaults = self.fes_message_defaults();
        defaults.font_name = units;
        defaults.outline = controls[0]; defaults.shadow = controls[1]; defaults.bold = controls[2] != 0;
        defaults.glyph_parameter = Some(if controls[3] == 0 { 1 } else { controls[4] });
        self.data_services.fes_settings.fonts = controls;
        self.data_services.fes_settings.fonts_specified[0] = true;
        self.data_services.message_device.set_raw_font_controls(controls);
        // Existing dirty dispatch delivers command19 to real FES/STRING/sound
        // consumers with its own continuation; do not recreate text objects.
        self.data_services.language.system_settings_dirty = 1;
        Ok(())
    }
    pub(super) fn ensure_fes_settings(&mut self) -> Result<()> {
        if self.data_services.fes_settings.initialized {
            if let Some(controls) = self.data_services.message_device.raw_font_controls() {
                self.data_services.fes_settings.fonts = controls;
            }
            return Ok(());
        }
        self.ensure_legacy_sound_settings()?;
        let defaults: &[&[u32]] = &[
            &[4,4,0,15,5,1,0,1,0,100,0,u32::MAX,0,0,0,0,0,0],
            &[0,0,1,1,1,5,0,0], &[u32::MAX,1,1],
        ];
        for (index, words) in defaults.iter().enumerate() {
            for (slot, &value) in words.iter().enumerate() {
                put(&mut self.system_config_blocks[index + 1], slot * 4, value);
            }
        }
        let mut documents = Vec::new();
        if let Some(startup) = self.read_legacy_sound_xml("startup.xml")? {
            if let Some(value) = xml_integer(&startup, "document/GAME/use_jtalk") {
                self.data_services.fes_settings.jtalk = value;
            }
            for (group, name) in ["local", "global"].iter().enumerate() {
                for index in 0..8 {
                    for (component, field) in ["start", "count"].iter().enumerate() {
                        if let Some(value) = xml_integer(&startup,
                            &format!("document/GAME/flag/{name}{index}/{field}"))
                        {
                            self.data_services.fes_settings.flag_ranges[group][index][component] = value;
                        }
                    }
                }
            }
            self.data_services.fes_settings.backlog_static =
                xml_integer(&startup, "document/GAME/log/static").unwrap_or(0);
            self.data_services.fes_settings.fullscreen_position =
                xml_integer(&startup, "document/SCREEN/mw_pos").unwrap_or(0);
            self.data_services.fes_settings.auto_select =
                xml_integer(&startup, "document/GAME/autoselect").unwrap_or(self.auto_select);
            let mut color = [255_u32; 4];
            for (component, name) in color.iter_mut().zip(["a", "r", "g", "b"]) {
                if let Some(value) = xml_integer(&startup, &format!("document/GAME/wndcolor/{name}")) {
                    *component = value;
                }
            }
            if xml_integer(&startup, "document/CONFIG_B/wnd_alpha").unwrap_or(1) != 0 {
                color[0] = (color[0] as i32).wrapping_mul(100).wrapping_div(255) as u32;
            }
            let packed = color.into_iter().fold(0_u32, |packed, value| packed.wrapping_shl(8) | value);
            put(&mut self.system_config_blocks[3], 0, packed);
            for index in 0..32 {
                for (field, key) in ["base", "char"].iter().enumerate() {
                    if let Some(value) = xml_integer(&startup, &format!("document/GAME/autopriset/p{index:02}/{key}")) {
                        self.data_services.fes_settings.presets[index][field] = value;
                    }
                }
            }
        }
        let preset = word(&self.system_config_blocks[1], 4) as usize;
        if preset >= 32 { bail!("automatic wait preset is outside 0..31"); }
        let values = self.data_services.fes_settings.presets[preset];
        put(&mut self.system_config_blocks[1], 12, values[0]);
        put(&mut self.system_config_blocks[1], 16, values[1]);
        for name in ["gamedef.xml", "gamedef"] {
            if let Some(xml) = self.read_legacy_sound_xml(name)? { documents.push((xml, false)); break; }
        }
        let user = self.save_root.join("setup.xml");
        if user.is_file() {
            let bytes = std::fs::read(user)?;
            documents.push((ConfigXml::parse_runtime(&bytes)?, true));
        }
        self.data_services.fes_settings.window_panel_defaults = [0, 4, 8]
            .map(|offset| word(&self.system_config_blocks[3], offset));
        self.data_services.fes_settings.message_panel_defaults = std::array::from_fn(|index|
            word(&self.system_config_blocks[1], index * 4));
        for (document, user_document) in &documents {
            for field in FIELDS {
                if let Some(value) = xml_integer(document, &format!("document/{}", field.xml)) {
                    put(&mut self.system_config_blocks[field.block], field.offset, value);
                }
            }
            if !*user_document {
                self.data_services.fes_settings.effect_panel_defaults = [0, 4, 8, 12, 16, 20, 24, 28]
                    .map(|offset| word(&self.system_config_blocks[2], offset));
                self.data_services.fes_settings.window_panel_defaults = [0, 4, 8]
                    .map(|offset| word(&self.system_config_blocks[3], offset));
                self.data_services.fes_settings.message_panel_defaults = std::array::from_fn(|index|
                    word(&self.system_config_blocks[1], index * 4));
            }
            let state = &mut self.data_services.fes_settings;
            for (target, key) in [
                (&mut state.scene_animation, "GAME/enable_scene_anm"),
                (&mut state.active, "GAME/always_active"),
                (&mut state.scene_skip, "GAME/use_scene_skip"),
                (&mut state.jtalk, "GAME/use_jtalk"),
                (&mut state.auto_select, "GAME/autoselect"),
            ] {
                if let Some(value) = xml_integer(document, &format!("document/{key}")) { *target = value; }
            }
            if *user_document {
                if let Some(value) = xml_integer(document, "document/WINDOW/fullscreen") {
                    state.fullscreen = value;
                }
                if let Some(value) = xml_integer(document, "document/FULL_SCREEN/mw_pos") {
                    state.fullscreen_position = value;
                }
            }
            for index in 0..48 {
                if let Some(value) = xml_integer(document, &format!("document/SYSTEMVOICE/play{index:02}")) {
                    state.system_voice[index] = value;
                }
            }
        }
        if let Some(controls) = self.data_services.message_device.raw_font_controls() {
            self.data_services.fes_settings.fonts = controls;
        } else if let Some(defaults) = &self.data_services.messages.defaults {
            self.data_services.fes_settings.fonts = [
                defaults.outline, defaults.shadow, u32::from(defaults.bold), 1,
                defaults.glyph_parameter.unwrap_or(0),
            ];
        }
        for (document, user_document) in &documents {
            if *user_document { self.apply_fes_user_document(document)?; }
        }
        self.data_services.fes_settings.initialized = true;
        Ok(())
    }

    pub(super) fn apply_fes_user_document(&mut self, document: &ConfigXml) -> Result<()> {
        // Validate both bounded names before changing any setting.
        let mut names: [Option<Vec<u16>>; 2] = [None, None];
        for (index, key) in ["face", "faceruby"].iter().enumerate() {
            if let Some(bytes) = super::xml_text(document, &format!("document/FONT/{key}")) {
                names[index] = Some(font_units(bytes)?);
            }
        }
        for field in FIELDS {
            if let Some(value) = xml_integer(document, &format!("document/{}", field.xml)) {
                put(&mut self.system_config_blocks[field.block], field.offset, value);
            }
        }
        let state = &mut self.data_services.fes_settings;
        for (target, key) in [
            (&mut state.auto_select, "GAME/autoselect"),
            (&mut state.scene_animation, "GAME/enable_scene_anm"),
            (&mut state.active, "GAME/always_active"),
            (&mut state.scene_skip, "GAME/use_scene_skip"),
            (&mut state.jtalk, "GAME/use_jtalk"),
            (&mut state.fullscreen, "WINDOW/fullscreen"),
            (&mut state.fullscreen_position, "FULL_SCREEN/mw_pos"),
        ] {
            if let Some(value) = xml_integer(document, &format!("document/{key}")) { *target = value; }
        }
        for (index, key) in ["shadow/around", "shadow/rightdown", "bold", "proportional"].iter().enumerate() {
            if let Some(value) = xml_integer(document, &format!("document/FONT/{key}")) { state.fonts[index] = value; }
        }
        state.fonts[4] = xml_integer(document, "document/FONT/fixed").unwrap_or(state.fonts[3]);
        for (index, value) in state.system_voice.iter_mut().enumerate() {
            if let Some(raw) = xml_integer(document, &format!("document/SYSTEMVOICE/play{index:02}")) { *value = raw; }
        }
        for (index, name) in names.into_iter().enumerate() {
            let Some(name) = name else { continue; };
            self.data_services.fes_settings.fonts_specified[index] = true;
            if index == 0 {
                self.fes_message_defaults().font_name = name;
            } else { self.data_services.message_device.ruby_font_name = name; }
        }
        let controls = self.data_services.fes_settings.fonts;
        if let Some(defaults) = &mut self.data_services.messages.defaults {
            defaults.outline = controls[0];
            defaults.shadow = controls[1];
            defaults.bold = controls[2] != 0;
            defaults.glyph_parameter = Some(if controls[3] == 0 { 1 } else { controls[4] });
        }
        self.data_services.message_device.set_raw_font_controls(controls);
        Ok(())
    }

    pub(super) fn settings_option_native(&mut self, index: u32) -> cs2_script::Result<cs2_script::NativeReturn> {
        if let Err(error) = self.ensure_fes_settings() {
            self.events.push(super::LogicEvent {
                frame: self.frame, kind: "settings-error".into(), detail: format!("{error:#}"),
            });
            return Err(super::native_state_error("settings initialization failed; see settings-error"));
        }
        Ok(cs2_script::NativeReturn::Value(match index {
            852 => self.data_services.fes_settings.backlog_static,
            741 => if self.data_services.fes_settings.fullscreen != 0 {
                self.data_services.fes_settings.fullscreen_position
            } else { 0 },
            999 => self.data_services.fes_settings.auto_select,
            1050 => self.data_services.fes_settings.scene_animation,
            _ => return Err(cs2_script::ScriptError::UnimplementedNativeCall { index }),
        }))
    }

    /// These wrappers read/write the same fields as FES getconf/setconf.
    /// They do not enqueue language changes or a configuration broadcast.
    pub(super) fn settings_control_native(
        &mut self, index: u32, arguments: &[u8],
    ) -> cs2_script::Result<cs2_script::NativeReturn> {
        use cs2_script::{NativeReturn, ScriptError};
        match index {
            1077 | 1078 => {
                self.ensure_fes_settings().map_err(|error| super::native_state_error_owned(
                    format!("scene skip settings initialization failed: {error:#}"),
                ))?;
                if index == 1077 {
                    let value = super::read_u32(arguments, 0)?;
                    self.data_services.fes_settings.scene_skip = value;
                    Ok(NativeReturn::Continue)
                } else {
                    Ok(NativeReturn::Value(self.data_services.fes_settings.scene_skip))
                }
            }
            1079 => {
                let mode = super::read_u32(arguments, 0)?;
                let languages = &self.data_services.language.languages;
                Ok(NativeReturn::Value(match mode {
                    0 => languages.primary_index,
                    1 => languages.message_index,
                    2 => languages.subtitle_index,
                    _ => 0,
                }))
            }
            _ => Err(ScriptError::UnimplementedNativeCall { index }),
        }
    }

    pub(super) fn flag_range_native(
        &mut self, arguments: &[u8], vm: &mut cs2_script::Vm,
    ) -> cs2_script::Result<cs2_script::NativeReturn> {
        let group = super::read_u32(arguments, 0)?;
        let index = super::read_u32(arguments, 4)? as usize;
        if index >= 8 { return Ok(cs2_script::NativeReturn::Value(0)); }
        let start_address = super::read_u32(arguments, 8)?;
        let count_address = super::read_u32(arguments, 12)?;
        self.ensure_fes_settings().map_err(|error| super::native_state_error_owned(
            format!("flag range settings initialization failed: {error:#}"),
        ))?;
        let ranges = &self.data_services.fes_settings.flag_ranges[usize::from(group != 0)];
        let start = ranges[index][0];
        vm.write(start_address, &start.to_le_bytes())?;
        // The original re-reads the popped native argument after this first
        // write. Preserve aliases into that VM region; explicit Host::call
        // without a live argument area uses its original argument slice.
        let current_index = match vm.native_arguments() {
            Some(bytes) => super::read_u32(bytes, 4)? as usize,
            None => index,
        };
        let count = ranges.get(current_index)
            .ok_or_else(|| super::native_state_error("flag range output changed its native index beyond seven"))?[1];
        vm.write(count_address, &count.to_le_bytes())?;
        Ok(cs2_script::NativeReturn::Value(1))
    }

    pub(super) fn write_fes_settings(
        &mut self, document: &mut cs2_runtime::configuration::ConfigurationDocument,
    ) -> Result<()> {
        self.ensure_fes_settings()?;
        for field in FIELDS {
            let path: Vec<_> = field.xml.split('/').map(str::as_bytes).collect();
            document.integer(&path, word(&self.system_config_blocks[field.block], field.offset))?;
        }
        let state = &self.data_services.fes_settings;
        for (key, value) in [
            ("WINDOW/fullscreen", state.fullscreen),
            ("FULL_SCREEN/mw_pos", state.fullscreen_position),
            ("GAME/autoselect", state.auto_select),
            ("GAME/always_active", state.active),
            ("GAME/enable_scene_anm", state.scene_animation),
            ("GAME/use_scene_skip", state.scene_skip),
            ("GAME/use_jtalk", state.jtalk),
        ] {
            document.integer(&key.split('/').map(str::as_bytes).collect::<Vec<_>>(), value)?;
        }
        for (path, value) in [
            ("FONT/shadow/around", state.fonts[0]),
            ("FONT/shadow/rightdown", state.fonts[1]),
            ("FONT/bold", state.fonts[2]),
            ("FONT/proportional", state.fonts[3]),
            ("FONT/fixed", state.fonts[4]),
        ] {
            document.integer(&path.split('/').map(str::as_bytes).collect::<Vec<_>>(), value)?;
        }
        if state.fonts_specified[0] {
            let defaults = self.data_services.messages.defaults.as_ref()
                .context("configured main font defaults are unavailable")?;
            document.wide(&[b"FONT", b"face"], &defaults.font_name)?;
        }
        if state.fonts_specified[1] {
            document.wide(&[b"FONT", b"faceruby"], &self.data_services.message_device.ruby_font_name)?;
        }
        for (index, &value) in state.system_voice.iter().enumerate() {
            document.integer(&[b"SYSTEMVOICE", format!("play{index:02}").as_bytes()], value)?;
        }
        let languages = &self.data_services.language.languages;
        for (name, value) in [
            (b"UI".as_slice(), languages.primary_index),
            (b"main".as_slice(), languages.message_index),
            (b"sub".as_slice(), languages.subtitle_index),
        ] { document.integer(&[b"language", b"id", name], value)?; }
        Ok(())
    }

    fn get_fes_setting(&mut self, name: &[u8], index: i32, extra: i32) -> Result<Option<u32>> {
        if let Some(field) = FIELDS.iter().find(|field| !field.name.is_empty() && field.name == name) {
            return Ok(Some(word(&self.system_config_blocks[field.block], field.offset)));
        }
        let channel = index.wrapping_add(extra) as u32;
        if let Some(value) = self.legacy_sound_conf(name, channel)? { return Ok(Some(value)); }
        let state = &self.data_services.fes_settings;
        let languages = &self.data_services.language.languages;
        Ok(Some(match name {
            b"lang_ui" => languages.primary_index,
            b"lang_text_main" => languages.message_index,
            b"lang_text_sub" => languages.subtitle_index,
            b"enable_sceneanm" => state.scene_animation,
            b"activewindow" => state.active,
            b"use_sceneskip" => state.scene_skip,
            b"use_jtalk" => state.jtalk,
            b"screen" => state.fullscreen,
            b"iswindow" => state.windowed_mode.unwrap_or_else(|| u32::from(state.fullscreen == 0)),
            b"fshadow" => state.fonts[shadow_index(index)?],
            b"fontweight" => state.fonts[2],
            b"fontprop" => state.fonts[3],
            b"fontfix" => state.fonts[4],
            b"sysplay" if channel >= 48 => 0,
            b"sysplay" => *state.system_voice.get(index as usize)
                .context("sysplay checked sum is valid but its actual index is outside 0..47")?,
            _ => return Ok(None),
        }))
    }

    fn set_fes_setting(&mut self, name: &[u8], value: u32, index: i32, extra: i32) -> Result<()> {
        if let Some(field) = FIELDS.iter().find(|field| !field.name.is_empty() && field.name == name) {
            if name == b"autospd" {
                let preset = self.data_services.fes_settings.presets.get(value as usize)
                    .context("automatic wait preset is outside 0..31")?.to_owned();
                put(&mut self.system_config_blocks[1], 12, preset[0]);
                put(&mut self.system_config_blocks[1], 16, preset[1]);
            }
            put(&mut self.system_config_blocks[field.block], field.offset, value);
            return Ok(());
        }
        let channel = index.wrapping_add(extra) as u32;
        if self.set_legacy_sound_conf(name, channel, value)? { return Ok(()); }
        if matches!(name, b"lang_ui" | b"lang_text_main" | b"lang_text_sub") {
            let target = match name {
                b"lang_ui" => LanguageTarget::Interface,
                b"lang_text_main" => LanguageTarget::MainText,
                _ => LanguageTarget::Subtitle,
            };
            let languages = &mut self.data_services.language.languages;
            let valid = (value as usize) < languages.prefixes.len();
            let notify = match target {
                LanguageTarget::Interface if valid => {
                    let changed = languages.primary_index != value;
                    languages.primary_index = value;
                    changed
                }
                LanguageTarget::MainText if valid => { languages.message_index = value; true }
                LanguageTarget::Subtitle => { languages.subtitle_index = if valid { value } else { u32::MAX }; false }
                _ => false,
            };
            if notify {
                self.data_services.language.enqueue_language_change(LanguageChangeRequest { task_id: self.current_task_id, target })?;
            }
            return Ok(());
        }
        if matches!(name, b"screen" | b"iswindow") {
            let windowed = if name == b"screen" { value == 0 } else { value != 0 };
            if self.data_services.fes_settings.windowed_mode.is_some() {
                return self.post_fes_window_menu(if windowed { 11 } else { 10 });
            }
            self.data_services.fes_settings.set_presented_fullscreen(u32::from(!windowed));
            return Ok(());
        }
        let state = &mut self.data_services.fes_settings;
        match name {
            b"enable_sceneanm" => state.scene_animation = value,
            b"activewindow" => state.active = value,
            b"use_sceneskip" => state.scene_skip = value,
            b"use_jtalk" => {
                state.jtalk = value;
                self.data_services.legacy_sound.jtalk = value != 0;
            }
            b"sysplay" if channel >= 48 => {}
            b"sysplay" => *state.system_voice.get_mut(index as usize)
                .context("sysplay actual index is outside 0..47")? = value,
            b"fshadow" => state.fonts[shadow_index(index)?] = value,
            b"fontweight" => state.fonts[2] = value,
            b"fontprop" => state.fonts[3] = value,
            b"fontfix" => state.fonts[4] = value,
            _ => return Ok(()),
        }
        if matches!(name, b"fshadow" | b"fontweight" | b"fontprop" | b"fontfix") {
            let controls = state.fonts;
            let defaults = self.fes_message_defaults();
            defaults.outline = controls[0];
            defaults.shadow = controls[1];
            defaults.bold = controls[2] != 0;
            defaults.glyph_parameter = Some(if controls[3] == 0 { 1 } else { controls[4] });
            self.data_services.message_device.set_raw_font_controls(controls);
        }
        if name == b"use_jtalk" && value != 0 {
            self.initialize_sound_speech()?;
        }
        Ok(())
    }
}

pub(super) fn apply_effect(
    host: &mut LoggingHost<'_>, view: &FesTaskView, _runtime: &mut FesRuntime,
    driver: &mut FesScriptDriver, effect: &FesDriverEffect, shared: &mut ExpressionState,
) -> Result<Option<bool>> {
    let FesDriverEffect::Command(command) = effect else { return Ok(None); };
    let Some(verb) = command.parameters.first().map(Vec::as_slice) else { return Ok(None); };
    if verb == b"config_open" {
        host.post_effect_settings()?;
        return Ok(Some(true));
    }
    if verb == b"fontcom" {
        return apply_font_command(host, driver, command, shared);
    }
    if !matches!(verb, b"getconf" | b"setconf") { return Ok(None); }
    host.ensure_fes_settings()?;
    let args = &command.parameters[1..];
    if args.is_empty() { return Ok(Some(true)); }
    let stamp = (view.id, command.offset);
    let mut values = host.data_services.fes_settings.pending.remove(&stamp).unwrap_or_default();
    let result = (|| -> Result<()> {
        if values.is_empty() { values.push(driver.evaluate_parameter(&args[0], 0, shared)?); }
        let Some(name) = args.get(1).map(|name| trim_nul(name)) else { return Ok(()); };
        for index in 0..2 {
            if values.len() <= index + 1 {
                values.push(args.get(index + 2).map(|source| driver.evaluate_parameter(source, 0, shared)).transpose()?.unwrap_or(0));
            }
        }
        if matches!(name, b"font" | b"ruby") {
            if verb == b"setconf" {
                let bytes = shared.strings.get(&(values[0] as u32)).cloned().unwrap_or_default();
                let units = font_units(&bytes)?;
                if name == b"font" {
                    host.fes_message_defaults().font_name = units;
                } else { host.data_services.message_device.ruby_font_name = units; }
                host.data_services.fes_settings.fonts_specified[usize::from(name == b"ruby")] = true;
            } else {
                let units = if name == b"font" {
                    host.data_services.messages.defaults.as_ref().map_or(&[][..], |defaults| defaults.font_name.as_slice())
                } else { host.data_services.message_device.ruby_font_name.as_slice() };
                let destination = values[0] as u32;
                let bytes = font_bytes(units);
                host.data_services.cst_text_origins.set_shared_encoding(
                    destination, &bytes, ScriptTextEncoding::Utf8,
                );
                shared.strings.insert(destination, bytes);
            }
        } else if verb == b"setconf" {
            host.set_fes_setting(name, values[0] as u32, values[1], values[2])?;
        } else if let Some(value) = host.get_fes_setting(name, values[1], values[2])? {
            if let Some(target) = usize::try_from(values[0]).ok().and_then(|index| shared.shared.get_mut(index)) {
                *target = value as i32;
            }
        }
        if verb == b"setconf" {
            host.refresh_legacy_sound_volumes()?;
            host.data_services.language.system_settings_dirty = 1;
        }
        Ok(())
    })();
    if result.is_err() { host.data_services.fes_settings.pending.insert(stamp, values); }
    result.map(|_| Some(true))
}

fn apply_font_command(
    host: &mut LoggingHost<'_>, driver: &mut FesScriptDriver, command: &FesDriverCommand,
    shared: &mut ExpressionState,
) -> Result<Option<bool>> {
    let args = &command.parameters[1..];
    let Some(operation) = args.first().map(Vec::as_slice) else { return Ok(None); };
    if operation == b"init" {
        host.data_services.message_device.fes_font_faces(host.resources);
        return Ok(Some(true));
    }
    if !matches!(operation, b"count" | b"search" | b"default" | b"get") { return Ok(None); }
    let destination = args.get(if operation == b"get" { 2 } else { 1 })
        .context("fontcom destination is missing")?;
    let index = if operation == b"get" {
        Some(driver.evaluate_parameter(&args[1], 0, shared)?)
    } else { None };
    let destination = driver.evaluate_parameter(destination, 0, shared)?;
    let faces = host.data_services.message_device.fes_font_faces(host.resources);
    let find_face = |name: &str| {
        let name = name.to_lowercase();
        faces.iter().position(|face| face.to_lowercase() == name)
    };
    if matches!(operation, b"count" | b"search") {
        let value = if operation == b"count" {
            faces.len() as i32
        } else {
            let name = args[2..].join(&b' ');
            std::str::from_utf8(&name).ok().and_then(find_face).map_or(-1, |index| index as i32)
        };
        if let Some(target) = usize::try_from(destination).ok().and_then(|index| shared.shared.get_mut(index)) {
            *target = value;
        }
        if operation == b"count" {
            host.events.push(LogicEvent { frame: host.frame, kind: "fes-font-catalog".into(),
                detail: format!("count={value} destination={destination}") });
        }
    } else {
        let name = if let Some(index) = index {
            usize::try_from(index).ok().and_then(|index| faces.get(index))
        } else {
            host.data_services.messages.font_panel_defaults.as_ref()
                .and_then(|defaults| String::from_utf16(&defaults.font_name).ok())
                .and_then(|name| find_face(&name)).and_then(|index| faces.get(index))
                .or_else(|| faces.first())
        };
        let bytes = name.map_or_else(Vec::new, |name| name.as_bytes().to_vec());
        if let Some(index) = index {
            host.events.push(LogicEvent { frame: host.frame, kind: "fes-font-catalog".into(),
                detail: format!("index={index} destination={destination} bytes={}", bytes.len()) });
        }
        let destination = destination as u32;
        host.data_services.cst_text_origins.set_shared_encoding(destination, &bytes, ScriptTextEncoding::Utf8);
        shared.strings.insert(destination, bytes);
    }
    Ok(Some(true))
}

pub(super) fn release_owner(host: &mut LoggingHost<'_>, owner: u32) {
    host.data_services.fes_settings.pending.retain(|(id, _), _| *id != owner);
}
fn shadow_index(index: i32) -> Result<usize> {
    if index < 0 { bail!("negative fshadow index precedes the font configuration"); }
    Ok(index.min(2) as usize)
}
fn word(bytes: &[u8], at: usize) -> u32 { u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) }
fn put(bytes: &mut [u8], at: usize, value: u32) { bytes[at..at + 4].copy_from_slice(&value.to_le_bytes()); }
fn xml_integer(document: &ConfigXml, path: &str) -> Option<u32> {
    let bytes = super::xml_text(document, path)?;
    let (input, negative) = match bytes.first() {
        Some(b'-') => (&bytes[1..], true),
        Some(b'+') => (&bytes[1..], false),
        _ => (bytes, false),
    };
    let hexadecimal = input.starts_with(b"0x") || input.starts_with(b"0X");
    let input = if hexadecimal { &input[2..] } else { input };
    let count = input.iter().take_while(|byte|
        if hexadecimal { byte.is_ascii_hexdigit() } else { byte.is_ascii_digit() }).count();
    if !hexadecimal && input.get(count) == Some(&b'.') { return Some(0); }
    let radix = if hexadecimal { 16 } else { 10 };
    let value = input[..count].iter().fold(0_u32, |value, byte| {
        let digit = if byte.is_ascii_digit() { u32::from(*byte - b'0') }
            else { u32::from(byte.to_ascii_lowercase() - b'a' + 10) };
        value.wrapping_mul(radix).wrapping_add(digit)
    });
    Some(if negative { value.wrapping_neg() } else { value })
}
fn font_bytes(units: &[u16]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for &unit in units.iter().take_while(|&&unit| unit != 0) {
        if unit == 0xffff { bytes.extend_from_slice(&[0xff, 0xff]); }
        else {
            let mut buffer = [0; 4];
            bytes.extend_from_slice(char::from_u32(u32::from(unit)).unwrap_or(char::REPLACEMENT_CHARACTER)
                .encode_utf8(&mut buffer).as_bytes());
        }
    }
    bytes
}
fn font_units(input: &[u8]) -> Result<Vec<u16>> {
    let input = trim_nul(input);
    let mut units = Vec::new();
    let mut cursor = 0;
    while cursor < input.len() {
        if input.get(cursor..cursor + 2) == Some(&[0xff, 0xff]) {
            units.push(0xffff); cursor += 2;
        } else {
            let first = input[cursor];
            let length = match first { 0..=0x7f => 1, 0xc0..=0xdf => 2, 0xe0..=0xef => 3, 0xf0..=0xf7 => 4, _ => break };
            let encoded = input.get(cursor..cursor + length).context("truncated UTF-8 font name")?;
            if encoded[1..].iter().any(|byte| byte & 0xc0 != 0x80) { break; }
            let mask = if length == 1 { 0x7f } else { (1_u32 << (7 - length)) - 1 };
            let code = encoded[1..].iter().fold(u32::from(first) & mask, |value, byte| (value << 6) | u32::from(byte & 63));
            let unit = if code >= 0x10000 { 0xd800 | ((code.wrapping_sub(0x10000) >> 10) & 0x3ff) } else { code };
            units.push(unit as u16);
            cursor += length;
        }
        if units.len() > 63 { bail!("font name exceeds 63 UTF-16 units"); }
    }
    Ok(units)
}

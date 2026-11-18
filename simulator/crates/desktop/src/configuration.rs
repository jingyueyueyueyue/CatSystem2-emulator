//! Host-owned configuration persistence. Paths always refer to the player's
//! private save directory; original game files are only read as definitions.
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use anyhow::{Context, Result, bail};
use cs2_formats::ConfigXml;
use cs2_runtime::configuration::ConfigurationDocument;
use cs2_script::NativeReturn;
use super::{DesktopSoundConfig, DesktopSoundPart, HeadlessPlayer, LoggingHost, LogicEvent, native_state_error, xml_integer};

// Public serialized field identities and initial values, not renderer feature
// claims. Actual device application stays with the corresponding host.
const PLATFORM: &[(&str, u32)] = &[
    ("WINDOW/fps",60), ("WINDOW/mode",0), ("WINDOW/bpp",u32::MAX),
    ("WINDOW/full_wide",0), ("WINDOW/limit",1), ("WINDOW/cursor_mode",1),
    ("WINDOW/screensaver",0), ("WINDOW/camouflage",0), ("WINDOW/out_info",1),
    ("WINDOW/setmode",0), ("TEXTURE/bpp",32), ("TEXTURE/min",0),
    ("TEXTURE/mag",0), ("TEXTURE/dither",1), ("SOUND/ds3d",0),
    ("SOUND/sticky",1), ("SOUND/stereo",1), ("ETC/cache",1),
    ("ETC/imgcache",32), ("ETC/dualcore",1), ("ETC/disablemovie",0),
    ("ETC/disablepixelshader",0), ("ETC/waitd3dcommand",0),
    ("DEPEND/gdi_cap",0), ("DEPEND/z32",0), ("DEPEND/use_z",1),
    ("D3D/backbuffer",1), ("D3D/hardTnL",1), ("D3D/light",1),
    ("D3D/vsync",1), ("D3D/antialias",0), ("D3D/fogtype",1),
    ("D3D/tex_size",0), ("D3D/tex_comp",0), ("FULL_SCREEN/full_size",0),
    ("FULL_SCREEN/full_pos",0), ("FULL_SCREEN/wall",1), ("FULL_SCREEN/no_full_res",0),
    ("CONFIG/type",0), ("KEY/enable_joypad",0),
];
const STARTUP: &[(&str, &str)] = &[
    ("SCREEN/fps","WINDOW/fps"), ("SCREEN/limit","WINDOW/limit"),
    ("GENERAL/screensaver","WINDOW/screensaver"), ("GENERAL/camouflage","WINDOW/camouflage"),
    ("CURSOR/mode","WINDOW/cursor_mode"), ("TEXTURE/bpp","TEXTURE/bpp"),
    ("TEXTURE/filter_min","TEXTURE/min"), ("TEXTURE/filter_mag","TEXTURE/mag"),
    ("TEXTURE/dither","TEXTURE/dither"), ("SOUND/use3d","SOUND/ds3d"),
    ("SOUND/sticky","SOUND/sticky"), ("SOUND/stereo","SOUND/stereo"),
    ("APP/imgcache","ETC/imgcache"), ("APP/dualcore","ETC/dualcore"),
    ("SCREEN/full_size","FULL_SCREEN/full_size"), ("SCREEN/full_pos","FULL_SCREEN/full_pos"),
    ("SCREEN/wall","FULL_SCREEN/wall"), ("SCREEN/no_full_res","FULL_SCREEN/no_full_res"),
    ("ETC/cache","ETC/cache"),
];

pub(super) struct ConfigurationState {
    initialized: bool,
    values: BTreeMap<&'static str, u32>,
    sizes: [u16; 4],
    user: Option<ConfigXml>,
    sound_file: PathBuf,
    general_defaults: [u32; 4],
    plugin_sound_defaults: [u32; 2],
    // Actual device-creation policy, distinct from a next-start user edit.
    sound_sticky_active: Option<bool>,
}

impl Default for ConfigurationState {
    fn default() -> Self {
        Self {
            initialized: false, values: PLATFORM.iter().copied().collect(),
            sizes: [800,600,800,600], user: None,
            sound_file: PathBuf::from("soundconf.xml"),
            general_defaults: [0, 0, 0, 1],
            plugin_sound_defaults: [1, 1], sound_sticky_active: None,
        }
    }
}

impl ConfigurationState {
    pub(super) fn screen_dimensions(&self) -> [u16; 4] { self.sizes }

    /// In-memory shared configuration, including a speech table's accepted
    /// default-row fallback. No game file or save file is written here.
    pub(super) fn synchronize_speech(
        &mut self, id: &str, parameters: super::SpeechSynthesisParameters,
    ) {
        let user = self.user.get_or_insert_with(|| ConfigXml {
            root: b"document".to_vec(), fields: Vec::new(),
        });
        for (key, value) in [
            (b"id".as_slice(), id.as_bytes().to_vec()),
            (b"speed".as_slice(), format!("{:.6}", parameters.speed).into_bytes()),
            (b"tone".as_slice(), format!("{:.6}", parameters.tone).into_bytes()),
        ] {
            let path = vec![b"document".to_vec(), b"TTS".to_vec(), key.to_vec()];
            if let Some(field) = user.fields.iter_mut().find(|field| field.path == path) {
                field.text = value;
            } else {
                user.fields.push(cs2_formats::XmlField { path, text: value });
            }
        }
    }
}

impl LoggingHost<'_> {
    pub(super) fn write_general_information(&mut self, bytes: &[u8]) {
        self.write_configuration_file(Path::new("info.txt"), bytes);
    }
    pub(super) fn ensure_configuration_state(&mut self) -> Result<()> {
        if self.data_services.configuration.initialized { return Ok(()); }
        let mut state = ConfigurationState::default();
        if let Some(startup) = self.read_legacy_sound_xml("startup.xml")? {
            if let Some(name) = super::xml_text(&startup, "document/SOUNDCONFIG") {
                let name = std::str::from_utf8(super::trim_nul(name))?.trim().replace('\\', "/");
                if !name.is_empty() {
                    let candidate = PathBuf::from(name);
                    if candidate.is_absolute() || candidate.components().any(|part| {
                        !matches!(part, Component::Normal(_))
                    }) || candidate.to_string_lossy().contains(':') {
                        bail!("SOUNDCONFIG is not a private relative file path");
                    }
                    state.sound_file = candidate;
                }
            }
            for &(source, target) in STARTUP {
                if let Some(value) = xml_integer(&startup, &format!("document/{source}")) {
                    state.values.insert(target, value);
                }
            }
            for (index, name) in ["width","height","full_width","full_height"].iter().enumerate() {
                let fallback = state.sizes[if index < 2 { index } else { index - 2 }];
                state.sizes[index] = xml_integer(&startup, &format!("document/SCREEN/{name}"))
                    .map_or(fallback, |value| value as u16);
            }
        }
        state.general_defaults = [0, state.values["WINDOW/screensaver"], state.values["WINDOW/camouflage"], 1];
        //109 Reset copies sticky from the pre-user definition, forces stereo1.
        state.plugin_sound_defaults = [state.values["SOUND/sticky"], 1];
        let path = self.save_root.join("setup.xml");
        match std::fs::read(&path) {
            Ok(bytes) => {
                let document = ConfigXml::parse_runtime(&bytes).context("parse private setup.xml")?;
                state.apply_user(&document);
                self.data_services.messages.device_keys.apply_user_configuration(&document);
                self.data_services.language.apply_user_config_xml(&document);
                self.apply_legacy_sound_settings_xml(&document)?;
                state.user = Some(document);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
        }
        state.initialized = true;
        self.data_services.configuration = state;
        // Applying setup.xml above may have loaded a default voice row and
        // corrected TTS/id. Publish that accepted selection after replacing
        // the configuration snapshot, so its original id cannot overwrite it.
        self.synchronize_sound_speech_configuration();
        Ok(())
    }

    pub(super) fn save_configuration_native(&mut self) -> cs2_script::Result<NativeReturn> {
        self.persist_configuration().map_err(|error| {
            self.events.push(LogicEvent {
                frame: self.frame, kind: "configuration-error".into(), detail: format!("{error:#}"),
            });
            native_state_error("configuration persistence failed; see configuration-error")
        })?;
        Ok(NativeReturn::Continue)
    }

    pub(super) fn persist_configuration(&mut self) -> Result<()> {
        self.ensure_fes_settings()?;
        self.ensure_configuration_state()?;
        let state = &self.data_services.configuration;
        let mut document = match &state.user {
            Some(user) => ConfigurationDocument::from_config(user)?,
            None => ConfigurationDocument::default(),
        };
        state.write(&mut document)?;
        self.write_sound_speech_configuration(&mut document)?;
        write_keys(&self.data_services.messages.device_keys, &mut document)?;
        self.write_fes_settings(&mut document)?;
        self.write_legacy_sound_configuration(&mut document)?;
        let bytes = document.to_bytes()?;
        self.write_configuration_file(Path::new("setup.xml"), &bytes);
        if self.sound_config_enabled != 0 {
            if !self.sound_config.source_loaded {
                bail!("native 361 has no independent sound configuration");
            }
            let sound = sound_document(self.sound_config)?.to_bytes()?;
            let filename = self.data_services.configuration.sound_file.clone();
            // An ordinary main-file failure must not suppress the sound-file
            // attempt. A serialization/integration error remains a diagnostic.
            self.write_configuration_file(&filename, &sound);
            if let Err(error) = self.sound_config.save_overrides(self.save_root) {
                self.events.push(LogicEvent {
                    frame: self.frame, kind: "configuration-write-failed".into(),
                    detail: format!("soundconf.override: {error}"),
                });
            }
        }
        Ok(())
    }

    fn write_configuration_file(&mut self, filename: &Path, bytes: &[u8]) {
        let result = (|| -> std::io::Result<()> {
            let path = self.save_root.join(filename);
            std::fs::create_dir_all(path.parent().unwrap_or(self.save_root))?;
            let mut file = std::fs::File::create(path)?;
            file.write_all(bytes)?;
            file.flush()
        })();
        self.events.push(LogicEvent {
            frame: self.frame,
            kind: if result.is_ok() { "configuration-written" } else { "configuration-write-failed" }.into(),
            detail: match result {
                Ok(()) => format!("{}: {} bytes", filename.display(), bytes.len()),
                Err(error) => format!("{}: {error}", filename.display()),
            },
        });
    }

    fn write_legacy_sound_configuration(&mut self, document: &mut ConfigurationDocument) -> Result<()> {
        let block = &self.system_config_blocks[0];
        for (name, base) in [("vol",0x15d4),("mute",0x15e8)] {
            for (index, class) in ["master","bgm","se","voice","play"].iter().enumerate() {
                let value = u32::from_le_bytes(block[base + index * 4..base + index * 4 + 4].try_into()?);
                document.integer(&[b"VOICE", format!("{name}_{class}").as_bytes()], value)?;
            }
        }
        document.integer(&[b"VOICE",b"cont_voice"], u32::from_le_bytes(block[16..20].try_into()?))?;
        for channel in 0..48 {
            let at = 20 + channel * 116;
            document.integer(&[b"VOICE",format!("play{channel:02}").as_bytes()],
                u32::from_le_bytes(block[at..at + 4].try_into()?))?;
        }
        Ok(())
    }
}

impl HeadlessPlayer {
    pub(super) fn initialize_configuration_host(&mut self) -> Result<()> {
        self.with_host(|host| host.ensure_configuration_state())?;
        // Own the mutable definition here; LoggingHost's live scripts borrow
        // an immutable view of it during dispatch.
        if self.sound_config_enabled != 0 {
            let path = self.save_root.join(&self.data_services.configuration.sound_file);
            match std::fs::read(&path) {
                Ok(bytes) => {
                    if !self.sound_config.source_loaded {
                        bail!("sound user configuration requires its source definition");
                    }
                    let document = ConfigXml::parse_runtime(&bytes).context("parse private sound configuration")?;
                    apply_sound_user(&mut self.sound_config, &document);
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
            }
        }
        Ok(())
    }
}

impl ConfigurationState {
    pub(super) fn platform_value(&self,path:&str)->Option<u32> {
        self.values.get(path).copied()
    }
    pub(super) fn general_defaults(&self) -> [u32; 4] { self.general_defaults }
    pub(super) fn plugin_sound_defaults(&self) -> [u32; 2] { self.plugin_sound_defaults }
    pub(super) fn set_plugin_sound_value(&mut self, field: &'static str, value: u32) -> Result<()> {
        if !matches!(field, "SOUND/sticky" | "SOUND/stereo") || value > 1 {
            bail!("uninspected plugin sound field or value");
        }
        self.values.insert(field, value);
        Ok(())
    }
    pub(super) fn set_plugin_backbuffer(&mut self, value: u32) -> Result<()> {
        if value > 1 { bail!("plugin backbuffer checkbox is outside its inspected domain"); }
        // Existing858 and retained-frame capture receivers read this map.
        // No renderer, image store or configuration copy is created here.
        self.values.insert("D3D/backbuffer", value);
        Ok(())
    }
    pub(super) fn initialize_sound_focus_policy(&mut self) {
        self.sound_sticky_active.get_or_insert(self.values["SOUND/sticky"] != 0);
    }
    pub(super) fn window_sound_suspended(&self, focused: bool) -> bool {
        !focused && self.sound_sticky_active == Some(false)
    }
    pub(super) fn set_general_values(&mut self, words: [u32; 4]) {
        for (field, value) in ["WINDOW/setmode", "WINDOW/screensaver", "WINDOW/camouflage", "WINDOW/out_info"].into_iter().zip(words) {
            self.values.insert(field, value);
        }
    }
    pub(super) fn set_actual_window_mode(&mut self, windowed: u32) {
        self.values.insert("WINDOW/mode", u32::from(windowed == 0));
    }
    pub(super) fn set_input_joypad_enabled(&mut self, value: u32) {
        self.values.insert("KEY/enable_joypad", value);
    }

    pub(super) fn apply_user(&mut self, document: &ConfigXml) {
        for &(path, _) in PLATFORM {
            if matches!(path, "WINDOW/full_wide" | "ETC/cache" | "DEPEND/z32" | "DEPEND/use_z") { continue; }
            if let Some(value) = xml_integer(document, &format!("document/{path}")) {
                self.values.insert(path, value);
            }
        }
        self.values.insert("ETC/disablemovie", xml_integer(document, "document/ETC/disablemovie").unwrap_or(0));
        for (index, name) in ["width","height","full_width","full_height"].iter().enumerate() {
            let fallback = self.sizes[if index < 2 { index } else { index - 2 }];
            self.sizes[index] = xml_integer(document, &format!("document/WINDOW/{name}"))
                .map_or(fallback, |value| value as u16);
        }
        self.user = Some(document.clone());
    }

    fn write(&self, document: &mut ConfigurationDocument) -> Result<()> {
        for &(path, _) in PLATFORM {
            document.integer(&path.split('/').map(str::as_bytes).collect::<Vec<_>>(), self.values[path])?;
        }
        for (index, name) in ["width","height","full_width","full_height"].iter().enumerate() {
            document.integer(&[b"WINDOW",name.as_bytes()], u32::from(self.sizes[index]))?;
        }
        for (name, fallback) in [(b"id".as_slice(), b"".as_slice()), (b"speed", b"1.000000"), (b"tone", b"0.000000")] {
            let existing = self.user.as_ref().and_then(|user| user.text(&[b"document",b"TTS",name]));
            document.write(&[b"TTS",name], existing.unwrap_or(fallback))?;
        }
        Ok(())
    }
}

fn write_keys(
    keys: &cs2_runtime::message_object::DeviceKeys,
    document: &mut ConfigurationDocument,
) -> Result<()> {
    for (index, key) in keys.bindings().iter().enumerate().filter(|(_, key)| key.enabled) {
        for (device, name) in ["kb","ms","jp"].iter().enumerate() {
            for group in 0..4 {
                for (suffix, value) in [("",u32::from(key.codes[device][group])),("r",key.repeat[device][group] & 255)] {
                    document.integer(&[b"KEY",format!("{name}{}{suffix}_{index:02}", group + 1).as_bytes()], value)?;
                }
            }
        }
    }
    Ok(())
}

fn sound_document(sound: &DesktopSoundConfig) -> Result<ConfigurationDocument> {
    let mut document = ConfigurationDocument::default();
    for (name, value) in [
        ("volume",sound.master_volume), ("play",sound.master_play_enabled),
        ("sysplay",sound.master_system_voice), ("voicevol",sound.master_voice_volume),
        ("skipvol",sound.master_skip_volume), ("mute",Some(sound.master_mute)),
        ("voicemute",Some(sound.master_voice_mute)),
    ] {
        if let Some(value) = value { document.integer(&[b"MASTER",name.as_bytes()], value as u32)?; }
    }
    fn parts(document: &mut ConfigurationDocument, parent: &[Vec<u8>], entries: &[DesktopSoundPart]) -> Result<()> {
        for entry in entries {
            let mut path = parent.to_vec();
            path.push(entry.xml_key.clone());
            for (name, value) in [
                ("volume",entry.volume), ("play",entry.play_enabled), ("sysplay",entry.system_voice),
                ("voicevol",entry.voice_volume), ("skipvol",entry.skip_volume),
                ("mute",entry.mute), ("voicemute",entry.voice_mute),
            ] {
                let mut field = path.clone(); field.push(name.as_bytes().to_vec());
                document.integer(&field.iter().map(Vec::as_slice).collect::<Vec<_>>(), value as u32)?;
            }
            if !entry.children.is_empty() {
                path.push(b"part".to_vec());
                parts(document, &path, &entry.children)?;
            }
        }
        Ok(())
    }
    parts(&mut document, &[b"PART".to_vec()], &sound.parts)?;
    Ok(document)
}

fn apply_sound_user(sound: &mut DesktopSoundConfig, document: &ConfigXml) {
    let fallback = sound.master_system_voice.unwrap_or(0);
    for (name, value) in [
        ("volume",&mut sound.master_volume), ("play",&mut sound.master_play_enabled),
        ("sysplay",&mut sound.master_system_voice), ("voicevol",&mut sound.master_voice_volume),
        ("skipvol",&mut sound.master_skip_volume),
    ] {
        if let Some(raw) = xml_integer(document, &format!("document/MASTER/{name}")) { *value = Some(raw as i32); }
    }
    sound.master_mute = xml_integer(document, "document/MASTER/mute").map_or(fallback, |value| value as i32);
    sound.master_voice_mute = xml_integer(document, "document/MASTER/voicemute").map_or(fallback, |value| value as i32);
    fn parts(entries: &mut [DesktopSoundPart], parent: &[Vec<u8>], document: &ConfigXml) {
        fn field_value(document: &ConfigXml, path: &[Vec<u8>], name: &[u8]) -> Option<u32> {
            let mut fields: Vec<_> = path.iter().map(Vec::as_slice).collect();
            fields.push(name);
            document.text(&fields).map(super::xml_dword)
        }
        for entry in entries {
            let mut path = parent.to_vec();
            path.push(entry.xml_key.clone());
            let fallback = entry.system_voice;
            for (name, value) in [
                ("volume",&mut entry.volume), ("play",&mut entry.play_enabled),
                ("sysplay",&mut entry.system_voice), ("voicevol",&mut entry.voice_volume),
                ("skipvol",&mut entry.skip_volume),
            ] {
                if let Some(raw) = field_value(document, &path, name.as_bytes()) { *value = raw as i32; }
            }
            entry.mute = field_value(document, &path, b"mute").map_or(fallback, |value| value as i32);
            entry.voice_mute = field_value(document, &path, b"voicemute").map_or(fallback, |value| value as i32);
            path.push(b"part".to_vec());
            parts(&mut entry.children, &path, document);
        }
    }
    parts(&mut sound.parts, &[b"document".to_vec(),b"PART".to_vec()], document);
}

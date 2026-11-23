//! Host-owned configuration persistence. Paths always refer to the player's
//! private save directory; original game files are only read as definitions.
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use anyhow::{Context, Result, bail};
use cs2_formats::ConfigXml;
use cs2_runtime::configuration::ConfigurationDocument;
use cs2_script::{NativeReturn, Vm};
use super::particle_external::CAMPANELLA_SHA;
use super::{DesktopSoundConfig, DesktopSoundPart, HeadlessPlayer, LoggingHost, LogicEvent, native_state_error, xml_integer};

pub(super) const CAMPANELLA_BOOT_PATH: &[&[u8]] = &[b"document", b"sysstr", b"boot"];
const CAMPANELLA_ADV_SHA256: &str = "5e55b26f8c9637054486a3fe303e25f863a29ae5deda96554e2dc9517e320b22";
const CAMPANELLA_CONFIG_LENGTHS: [usize; 4] = [0xb28, 0x20, 0x1c, 0x0c];

fn campanella_sound_block(modern: &[u8]) -> cs2_script::Result<[u8; 0xb28]> {
    let raw_volumes = modern.get(0x15d4..0x15e8)
        .ok_or_else(|| native_state_error("Campanella sound configuration source is truncated"))?;
    let mut legacy = [0; 0xb28];
    legacy[..20].copy_from_slice(&modern[..20]);
    for channel in 0..32 {
        let source = 20 + 116 * channel;
        let target = 20 + 0x58 * channel;
        legacy[target..target + 40].copy_from_slice(&modern[source..source + 40]);
        let head = &modern[source + 40..source + 80];
        let mut length = 0;
        while length < 15 && head[length] != 0 {
            let width = if matches!(head[length], 0x81..=0x9f | 0xe0..=0xfc) { 2 } else { 1 };
            if length + width > 15 { break; }
            length += width;
        }
        legacy[target + 40..target + 40 + length].copy_from_slice(&head[..length]);
        legacy[target + 56..target + 0x58].copy_from_slice(&modern[source + 84..source + 116]);
    }
    legacy[0xb14..].copy_from_slice(raw_volumes);
    Ok(legacy)
}

fn write_campanella_configuration(
    blocks: &[Vec<u8>; 4], addresses: [u32; 4], vm: &mut Vm,
) -> cs2_script::Result<()> {
    let sound = campanella_sound_block(&blocks[0])?;
    for (index, address) in addresses.into_iter().enumerate() {
        let block = if index == 0 { sound.as_slice() } else { &blocks[index] };
        if vm.read(address, 1).is_ok() {
            let bytes = block.get(..CAMPANELLA_CONFIG_LENGTHS[index])
                .ok_or_else(|| native_state_error("Campanella configuration source is truncated"))?;
            vm.write(address, bytes)?;
        }
    }
    Ok(())
}

#[cfg(test)]
#[test]
fn campanella_native359_preserves_motion_slots_and_copy_order() {
    let blocks = [vec![0x11; 5628], vec![0x22; 72], vec![0x33; 32], vec![0x44; 12]];
    let mut vm = Vm::new();
    vm.bind_external(vec![0xa5; 0x16a0]).unwrap();
    vm.write(0x8000_0e44, &0x15c_u32.to_le_bytes()).unwrap();
    vm.write(0x8000_1168, &0x15f_u32.to_le_bytes()).unwrap();
    let untouched = vm.read(0x8000_0bc8, 0x16a0 - 0xbc8).unwrap();

    write_campanella_configuration(&blocks, [0x8000_0098, 0x8000_0b80, 0x8000_0ba0, 0x8000_0bbc], &mut vm).unwrap();

    assert_eq!(vm.read(0x8000_0097, 2).unwrap(), [0xa5, 0x11]);
    assert_eq!(vm.read(0x8000_0b80, 0x20).unwrap(), [0x22; 0x20]);
    assert_eq!(vm.read(0x8000_0ba0, 0x1c).unwrap(), [0x33; 0x1c]);
    assert_eq!(vm.read(0x8000_0bbc, 0x0c).unwrap(), [0x44; 0x0c]);
    assert_eq!(vm.read(0x8000_0bc8, untouched.len()).unwrap(), untouched);
}

#[cfg(test)]
#[test]
fn campanella_native359_maps_sound_fields_to_legacy_offsets() {
    let mut modern = vec![0; 5628];
    modern[..20].fill(0x12);
    let channel = 20 + 116 * 31;
    modern[channel..channel + 40].fill(0x34);
    modern[channel + 40..channel + 54].fill(b'a');
    modern[channel + 54..channel + 56].copy_from_slice(&[0x82, 0xa0]);
    modern[channel + 80..channel + 84].fill(0x56);
    modern[channel + 84..channel + 116].fill(0x78);
    modern[0x15d4..0x15e8].fill(0x9a);
    modern[0x15e8..].fill(0xbc);

    let legacy = campanella_sound_block(&modern).unwrap();
    let channel = 20 + 0x58 * 31;
    assert_eq!(&legacy[..20], &[0x12; 20]);
    assert_eq!(&legacy[channel..channel + 40], &[0x34; 40]);
    assert_eq!(&legacy[channel + 40..channel + 54], &[b'a'; 14]);
    assert_eq!(&legacy[channel + 54..channel + 56], &[0; 2]);
    assert_eq!(&legacy[channel + 56..channel + 0x58], &[0x78; 32]);
    assert_eq!(&legacy[0xb14..], &[0x9a; 20]);
}

pub(super) fn campanella_boot_first_path(
    pe_sha: Option<&str>,
    name: &str,
    member_sha256: impl FnOnce() -> String,
) -> Option<&'static [&'static [u8]]> {
    (pe_sha == Some(CAMPANELLA_SHA)
        && name.eq_ignore_ascii_case("config.int/adv.xml")
        && member_sha256() == CAMPANELLA_ADV_SHA256)
        .then_some(CAMPANELLA_BOOT_PATH)
}

#[cfg(test)]
#[test]
fn campanella_boot_first_path_checks_game_and_member_identity() {
    let adv = "config.int/adv.xml";
    let verified = || CAMPANELLA_ADV_SHA256.to_owned();
    assert_eq!(
        campanella_boot_first_path(Some(CAMPANELLA_SHA), adv, verified),
        Some(CAMPANELLA_BOOT_PATH)
    );
    assert_eq!(campanella_boot_first_path(None, adv, || panic!("unexpected hash")), None);
    assert_eq!(
        campanella_boot_first_path(Some(CAMPANELLA_SHA), "adv.xml", || panic!("unexpected hash")),
        None
    );
    assert_eq!(
        campanella_boot_first_path(Some(CAMPANELLA_SHA), adv, || "changed".to_owned()),
        None
    );
}

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
    pub(super) fn campanella_configuration_export(
        &mut self, arguments: &[u8], vm: &mut Vm,
    ) -> cs2_script::Result<NativeReturn> {
        self.ensure_legacy_sound_settings()?;
        let mut addresses = [0; 4];
        for (index, address) in addresses.iter_mut().enumerate() {
            *address = super::read_u32(arguments, index * 4)?;
        }
        write_campanella_configuration(self.system_config_blocks, addresses, vm)?;
        self.events.push(LogicEvent {
            frame: self.frame,
            kind: "campanella-config-export".into(),
            detail: format!("native=359 addresses={addresses:#x?} lengths={CAMPANELLA_CONFIG_LENGTHS:#x?}"),
        });
        Ok(NativeReturn::Continue)
    }

    pub(super) fn write_general_information(&mut self, bytes: &[u8]) {
        let _ = self.write_configuration_file(Path::new("info.txt"), bytes);
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
        self.persist_configuration_files(false)
    }

    fn persist_configuration_files(&mut self, report_io_errors: bool) -> Result<()> {
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
        let setup = self.write_configuration_file(Path::new("setup.xml"), &bytes);
        if self.sound_config_enabled != 0 {
            if !self.sound_config.source_loaded {
                bail!("native 361 has no independent sound configuration");
            }
            let sound = sound_document(self.sound_config)?.to_bytes()?;
            let filename = self.data_services.configuration.sound_file.clone();
            // An ordinary main-file failure must not suppress the sound-file
            // attempt. A serialization/integration error remains a diagnostic.
            let sound = self.write_configuration_file(&filename, &sound);
            let overrides = self.sound_config.save_overrides(self.save_root);
            if let Err(error) = &overrides {
                self.events.push(LogicEvent {
                    frame: self.frame, kind: "configuration-write-failed".into(),
                    detail: format!("soundconf.override: {error}"),
                });
            }
            if report_io_errors {
                setup?;
                sound?;
                overrides.context("write soundconf.override")?;
            }
        } else if report_io_errors {
            setup?;
        }
        Ok(())
    }

    fn write_configuration_file(&mut self, filename: &Path, bytes: &[u8]) -> Result<()> {
        let result = (|| -> Result<()> {
            let path = self.save_root.join(filename);
            std::fs::create_dir_all(path.parent().unwrap_or(self.save_root))?;
            cs2_runtime::write_private_file(&path, bytes)?;
            Ok(())
        })().with_context(|| format!("write {}", filename.display()));
        self.events.push(LogicEvent {
            frame: self.frame,
            kind: if result.is_ok() { "configuration-written" } else { "configuration-write-failed" }.into(),
            detail: match &result {
                Ok(()) => format!("{}: {} bytes", filename.display(), bytes.len()),
                Err(error) => format!("{error:#}"),
            },
        });
        result
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
    pub fn persist_host_configuration(&mut self) -> Result<()> {
        self.with_host(|host| host.persist_configuration_files(true))
    }

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

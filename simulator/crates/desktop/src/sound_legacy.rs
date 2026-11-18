//! Legacy VOICE settings and 605/606/607/844. Layout constants are ABI facts
//! from docs/settings.md and public settings-layout assertions, not copied code.
use cs2_formats::ConfigXml;
use cs2_script::{NativeReturn, Result, Vm};

use super::{
    LoggingHost, audio_script_error, expand_kcs_format, read_u32, trim_nul, xml_integer, xml_text,
};

const CHANNELS: usize = 48;
const RECORD: usize = 116;
const RECORD_START: usize = 20;
const RAW_VOLUME: usize = 0x15d4;
const RAW_MUTE: usize = 0x15e8;
const BLOCK_BYTES: usize = 5628;

#[derive(Clone, Default, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(super) struct LegacySoundSettings {
    initialized: bool,
    pub(super) stereo: bool,
    pub(super) jtalk: bool,
    /// Original definition before private user overrides; not another live store.
    pub(super) panel_defaults: Option<Vec<u8>>,
}

impl LoggingHost<'_> {
    pub(super) fn ensure_legacy_sound_settings(&mut self) -> Result<()> {
        if self.data_services.legacy_sound.initialized {
            return Ok(());
        }
        if self.system_config_blocks[0].len() != BLOCK_BYTES {
            return Err(audio_script_error(
                "legacy sound settings block has invalid size",
            ));
        }
        // An explicit host/359/360 configuration must remain authoritative.
        if self.system_config_blocks[0].iter().any(|&byte| byte != 0) {
            self.data_services.legacy_sound.initialized = true;
            self.data_services.legacy_sound.stereo = true;
            return Ok(());
        }
        let mut block = vec![0; BLOCK_BYTES];
        for index in 0..CHANNELS {
            let base = RECORD_START + RECORD * index;
            write_word(&mut block, base, 1);
            write_word(&mut block, base + 36, 100);
            write_word(&mut block, base + 80, 100);
        }
        for index in 0..5 {
            write_word(&mut block, RAW_VOLUME + index * 4, 100);
        }
        let mut stereo = true;
        let mut jtalk = false;
        if let Some(startup) = self.read_legacy_sound_xml("startup.xml")? {
            apply_definitions(&mut block, &startup)?;
            self.apply_sound_speech_parameters_xml(&startup)?;
            stereo = xml_integer(&startup, "document/SOUND/stereo").unwrap_or(1) != 0;
            jtalk = xml_integer(&startup, "document/GAME/use_jtalk").unwrap_or(0) != 0;
        }
        for name in ["gamedef.xml", "settings.xml", "config.xml"] {
            if let Some(xml) = self.read_legacy_sound_xml(name)? {
                apply_values(&mut block, &xml);
                self.apply_sound_speech_parameters_xml(&xml)?;
                stereo =
                    xml_integer(&xml, "document/SOUND/stereo").map_or(stereo, |value| value != 0);
                jtalk =
                    xml_integer(&xml, "document/GAME/use_jtalk").map_or(jtalk, |value| value != 0);
            }
        }
        refresh(&mut block);
        let panel_defaults = Some(block.clone());
        for name in ["settings.xml", "config.xml"] {
            let path = self.save_root.join(name);
            match std::fs::read(path) {
                Ok(bytes) => {
                    let xml = ConfigXml::parse_runtime(&bytes).map_err(|_| {
                        audio_script_error("legacy user sound settings XML is malformed")
                    })?;
                    apply_values(&mut block, &xml);
                    self.apply_sound_speech_parameters_xml(&xml)?;
                    stereo = xml_integer(&xml, "document/SOUND/stereo")
                        .map_or(stereo, |value| value != 0);
                    jtalk = xml_integer(&xml, "document/GAME/use_jtalk")
                        .map_or(jtalk, |value| value != 0);
                }
                Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => {}
                Err(cause) => return Err(self.sound_host_error(cause)),
            }
        }
        refresh(&mut block);
        self.system_config_blocks[0] = block;
        self.data_services.legacy_sound = LegacySoundSettings {
            initialized: true,
            stereo,
            jtalk,
            panel_defaults,
        };
        Ok(())
    }

    pub(super) fn read_legacy_sound_xml(&mut self, name: &str) -> Result<Option<ConfigXml>> {
        if name == "startup.xml" {
            return self.resources.startup_configuration().map_err(|cause| self.sound_host_error(cause));
        }
        let names = match self.resources.category_names(b"config", name) {
            Ok(names) => names,
            Err(cs2_runtime::RuntimeError::InvalidResourceName(_)) => {
                // Startup bootstrap without a configured config category is
                // a host policy. An existing category's miss must not escape
                // its exact registered loose/archive chain.
                ["", "config/", "system/config/", "system/"]
                    .into_iter().map(|prefix| format!("{prefix}{name}")).collect()
            }
            Err(cause) => return Err(self.sound_host_error(cause)),
        };
        let resources = self.resources;
        for name in names {
            let resource = resources.resolve(&name).map_err(|cause| self.sound_host_error(cause))?;
            let Some(resource) = resource else { continue; };
            let bytes = resources.read_candidate(resource).map_err(|cause| self.sound_host_error(cause))?;
            return ConfigXml::parse_runtime(&bytes).map(Some)
                .map_err(|_| audio_script_error("legacy sound settings XML is malformed"));
        }
        Ok(None)
    }

    pub(super) fn legacy_sound_native(
        &mut self,
        index: u32,
        args: &[u8],
        vm: &mut Vm,
    ) -> Result<NativeReturn> {
        let name = expand_kcs_format(vm, &vm.read_string(read_u32(args, 0)?)?, 0)?;
        let value = self.legacy_sound_file_value(index, &name)?;
        Ok(NativeReturn::Value(value))
    }

    /// 674's old-configuration branch shares the 844 selector, then reads
    /// that definition's INT name (+84), not its group or archive filename.
    pub(super) fn legacy_sound_archive_name(&mut self, name: &[u8]) -> Result<Option<Vec<u8>>> {
        let selected = self.legacy_sound_file_value(844, name)?;
        if selected == u32::MAX {
            return Ok(None);
        }
        let start = RECORD_START + selected as usize * RECORD + 84;
        Ok(Some(
            trim_nul(&self.system_config_blocks[0][start..start + 32]).to_vec(),
        ))
    }

    /// 605 is the cached category volume, 606 play, 607 per-file mastervol,
    /// 844 definition index. Names are original declared script bytes.
    pub(super) fn legacy_sound_file_value(&mut self, index: u32, name: &[u8]) -> Result<u32> {
        self.ensure_legacy_sound_settings()?;
        let block = &self.system_config_blocks[0];
        if index == 605 && word(block, RAW_MUTE) != 0 {
            return Ok(0);
        }
        let name = checked_name(name)?;
        let selected = directory_index(block, name).or_else(|| {
            let source = if index == 605 {
                // This query's original basename scan uses raw separators.
                name.rsplit(|&byte| byte == b'/' || byte == b'\\')
                    .next()
                    .unwrap_or_default()
            } else {
                name
            };
            prefix_index(block, source)
        });
        match index {
            606 | 607 => Ok(selected.map_or(0, |channel| {
                word(
                    block,
                    RECORD_START + channel * RECORD + if index == 606 { 0 } else { 36 },
                )
            })),
            844 => Ok(selected.map_or(u32::MAX, |channel| channel as u32)),
            605 => {
                let Some(channel) = selected else {
                    return Ok(100);
                };
                let group = word(block, RECORD_START + channel * RECORD + 80);
                let Some(group) = self.sound_groups.iter().find(|entry| entry.id == group) else {
                    return Ok(100);
                };
                let class = if group.label.eq_ignore_ascii_case(b"bgm") {
                    0
                } else if group.label.eq_ignore_ascii_case(b"se") {
                    1
                } else if group.label.eq_ignore_ascii_case(b"voice")
                    || group.label.eq_ignore_ascii_case(b"sysvoice")
                {
                    2
                } else {
                    return Ok(100);
                };
                Ok(if word(block, RAW_MUTE + (class + 1) * 4) != 0 {
                    0
                } else {
                    word(block, class * 4)
                })
            }
            _ => Err(audio_script_error("unsupported legacy sound query")),
        }
    }

    /// Media TYPE=0 with a nonempty GROUP calls a label query directly, not
    /// filename matching (PE RVA 0x26ce50). Unknown groups return 100.
    pub(super) fn legacy_sound_group_volume(&mut self, group: &[u8]) -> Result<u32> {
        self.ensure_legacy_sound_settings()?;
        let block = &self.system_config_blocks[0];
        if word(block, RAW_MUTE) != 0 {
            return Ok(0);
        }
        let Some(class) = legacy_group_class(trim_nul(group)) else {
            return Ok(100);
        };
        Ok(if word(block, RAW_MUTE + (class + 1) * 4) != 0 {
            0
        } else {
            word(block, class * 4)
        })
    }

    /// Media TYPE!=0 keeps the object's raw volume, applying only this mute
    /// query. Even this path observes master mute (PE RVA 0x26cf00).
    pub(super) fn legacy_sound_group_muted(&mut self, group: &[u8]) -> Result<bool> {
        self.ensure_legacy_sound_settings()?;
        let block = &self.system_config_blocks[0];
        Ok(word(block, RAW_MUTE) != 0
            || legacy_group_class(trim_nul(group))
                .is_some_and(|class| word(block, RAW_MUTE + (class + 1) * 4) != 0))
    }

    /// Shared FES getconf/setconf backend. A missing key is left to the main
    /// FES configuration service. Raw writes refresh caches only on request.
    pub(super) fn legacy_sound_conf(&mut self, name: &[u8], channel: u32) -> Result<Option<u32>> {
        self.ensure_legacy_sound_settings()?;
        if name == b"play" && channel >= 48 {
            return Ok(Some(0));
        }
        Ok(conf_offset(name, channel).map(|offset| word(&self.system_config_blocks[0], offset)))
    }

    pub(super) fn set_legacy_sound_conf(
        &mut self,
        name: &[u8],
        channel: u32,
        value: u32,
    ) -> Result<bool> {
        self.ensure_legacy_sound_settings()?;
        if name == b"play" && channel >= 48 {
            self.refresh_legacy_sound_volumes()?;
            self.data_services.language.system_settings_dirty = 1;
            return Ok(true);
        }
        let Some(offset) = conf_offset(name, channel) else {
            return Ok(false);
        };
        write_word(&mut self.system_config_blocks[0], offset, value);
        self.refresh_legacy_sound_volumes()?;
        self.data_services.language.system_settings_dirty = 1;
        Ok(true)
    }

    pub(super) fn refresh_legacy_sound_volumes(&mut self) -> Result<()> {
        self.ensure_legacy_sound_settings()?;
        refresh(&mut self.system_config_blocks[0]);
        Ok(())
    }

    pub(super) fn apply_legacy_sound_settings_xml(&mut self, xml: &ConfigXml) -> Result<()> {
        self.ensure_legacy_sound_settings()?;
        self.apply_sound_speech_parameters_xml(xml)?;
        apply_values(&mut self.system_config_blocks[0], xml);
        if let Some(value) = xml_integer(xml, "document/SOUND/stereo") {
            self.data_services.legacy_sound.stereo = value != 0;
        }
        if let Some(value) = xml_integer(xml, "document/GAME/use_jtalk") {
            self.data_services.legacy_sound.jtalk = value != 0;
        }
        self.refresh_legacy_sound_volumes()?;
        self.refresh_sound_task_options();
        if self.data_services.legacy_sound.jtalk {
            self.initialize_sound_speech()?;
        }
        Ok(())
    }
}

fn apply_definitions(block: &mut [u8], xml: &ConfigXml) -> Result<()> {
    for index in 0..CHANNELS {
        let base = RECORD_START + index * RECORD;
        write_word(block, base, 1);
        for (field, offset, capacity) in [("voice", 4, 32), ("head", 40, 40), ("int", 84, 32)] {
            if let Some(bytes) = xml_text(xml, &format!("document/VOICE/v{index:02}/{field}")) {
                let bytes = trim_nul(bytes);
                if bytes.len() >= capacity {
                    return Err(audio_script_error(
                        "legacy sound definition exceeds its field capacity",
                    ));
                }
                block[base + offset..base + offset + capacity].fill(0);
                block[base + offset..base + offset + bytes.len()].copy_from_slice(bytes);
            }
        }
        for (field, offset) in [("mastervol", 36), ("group", 80)] {
            if let Some(value) = xml_integer(xml, &format!("document/VOICE/v{index:02}/{field}")) {
                write_word(block, base + offset, value);
            }
        }
        let lowered = lower_cp932(&block[base + 84..base + 116]);
        block[base + 84..base + 116].copy_from_slice(&lowered);
    }
    Ok(())
}

fn apply_values(block: &mut [u8], xml: &ConfigXml) {
    if let Some(value) = xml_integer(xml, "document/VOICE/cont_voice") {
        write_word(block, 16, value);
    }
    for (index, name) in ["master", "bgm", "se", "voice", "play"].iter().enumerate() {
        for (prefix, offset) in [("vol_", RAW_VOLUME), ("mute_", RAW_MUTE)] {
            if let Some(value) = xml_integer(xml, &format!("document/VOICE/{prefix}{name}")) {
                write_word(block, offset + index * 4, value);
            }
        }
    }
    for index in 0..CHANNELS {
        if let Some(value) = xml_integer(xml, &format!("document/VOICE/play{index:02}")) {
            write_word(block, RECORD_START + RECORD * index, value);
        }
    }
}

fn refresh(block: &mut [u8]) {
    let master = word(block, RAW_VOLUME);
    for index in 0..4 {
        let value = master.wrapping_mul(word(block, RAW_VOLUME + 4 + index * 4)) / 100;
        write_word(block, index * 4, value);
    }
}

fn conf_offset(name: &[u8], channel: u32) -> Option<usize> {
    if name == b"contvoice" {
        return Some(16);
    }
    if name == b"play" {
        return (channel < 48).then_some(RECORD_START + channel as usize * RECORD);
    }
    for (prefix, base) in [
        (b"vol_".as_slice(), RAW_VOLUME),
        (b"mute_".as_slice(), RAW_MUTE),
    ] {
        if let Some(name) = name.strip_prefix(prefix) {
            return [b"master".as_slice(), b"bgm", b"se", b"voice", b"play"]
                .iter()
                .position(|&class| class == name)
                .map(|index| base + index * 4);
        }
    }
    None
}

fn checked_name(name: &[u8]) -> Result<&[u8]> {
    let name = trim_nul(name);
    if name.len() >= 260 {
        return Err(audio_script_error(
            "legacy sound filename exceeds 259 bytes",
        ));
    }
    Ok(name)
}

fn legacy_group_class(group: &[u8]) -> Option<usize> {
    if group.eq_ignore_ascii_case(b"bgm") {
        Some(0)
    } else if group.eq_ignore_ascii_case(b"se") {
        Some(1)
    } else if group.eq_ignore_ascii_case(b"voice") || group.eq_ignore_ascii_case(b"sysvoice") {
        Some(2)
    } else {
        None
    }
}

fn directory_index(block: &[u8], name: &[u8]) -> Option<usize> {
    let mut cursor = 0;
    let mut last_separator = None;
    while cursor < name.len() {
        let byte = name[cursor];
        if cp932_lead(byte) && cursor + 1 < name.len() {
            cursor += 2;
            continue;
        }
        if byte == b'/' || byte == b'\\' {
            last_separator = Some(cursor);
        }
        cursor += 1;
    }
    let directory = &name[..last_separator?];
    let mut cursor = 0;
    let mut extension = None;
    while cursor < directory.len() {
        if cp932_lead(directory[cursor]) && cursor + 1 < directory.len() {
            cursor += 2;
            continue;
        }
        match directory[cursor] {
            b'/' | b'\\' => extension = None,
            b'.' => extension = Some(cursor),
            _ => {}
        }
        cursor += 1;
    }
    let extension = extension.unwrap_or(directory.len());
    let directory = lower_cp932(&directory[..extension]);
    (0..CHANNELS).find(|index| {
        let start = RECORD_START + index * RECORD + 84;
        trim_nul(&block[start..start + 32]) == directory && !directory.is_empty()
    })
}

fn prefix_index(block: &[u8], name: &[u8]) -> Option<usize> {
    let name = lower_cp932(name);
    (0..CHANNELS).find(|index| {
        let start = RECORD_START + index * RECORD + 40;
        let prefix = trim_nul(&block[start..start + 40]);
        !prefix.is_empty() && name.starts_with(prefix)
    })
}

fn lower_cp932(bytes: &[u8]) -> Vec<u8> {
    let mut result = bytes.to_vec();
    let mut cursor = 0;
    while cursor < result.len() {
        if cp932_lead(result[cursor]) && cursor + 1 < result.len() {
            cursor += 2;
        } else {
            result[cursor].make_ascii_lowercase();
            cursor += 1;
        }
    }
    result
}

fn cp932_lead(byte: u8) -> bool {
    matches!(byte, 0x81..=0x9f | 0xe0..=0xfc)
}
fn word(block: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(block[offset..offset + 4].try_into().unwrap())
}
fn write_word(block: &mut [u8], offset: usize, value: u32) {
    block[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

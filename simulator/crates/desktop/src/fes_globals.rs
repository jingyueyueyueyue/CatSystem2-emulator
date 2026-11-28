//! FES global commands. Every entry shares the driver's expression
//! scope and the player's random stream. Retry cursors retain successful
//! evaluations so an error cannot replay earlier assignment/random effects.
use std::collections::BTreeMap;

use anyhow::{Result, bail};
use cs2_runtime::resource_router::ResourceMode;
use cs2_script::ExpressionState;

use super::fes_driver::{FesDriverCommand, FesDriverEffect, FesScriptDriver};
use super::fes_tasks::FesTaskView;
use super::{DesktopSoundConfig, DesktopSoundPart, FesRuntime, LoggingHost, normalize_sound_name, trim_nul};

#[derive(Default, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(super) struct FesGlobalStore {
    pending: BTreeMap<(u32, usize), QueryProgress>,
    #[serde(skip)]
    warned_sound_defaults: bool,
    #[serde(skip)]
    legacy_voice_channels: Option<Vec<i32>>,
}

#[derive(Default, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct QueryProgress {
    output: Option<i32>,
    parameter: usize,
    component: usize,
    paths: Vec<Vec<i32>>,
    values: Vec<i32>,
}

impl FesGlobalStore {
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
        if self.pending.len() > 65_536 || self.pending.iter().any(|((owner, _), progress)|
            kinds.get(owner) != Some(&34) || progress.parameter > 1_000_000
                || progress.component > 1_000_000 || progress.paths.len() > 1_000_000
                || progress.values.len() > 1_000_000
                || progress.paths.iter().any(|path| path.len() > 1_000_000))
        { bail!("FES global query checkpoint has invalid owners or cursors"); }
        Ok(())
    }
}

pub(super) fn apply_effect(
    host: &mut LoggingHost<'_>, view: &FesTaskView, _runtime: &mut FesRuntime,
    driver: &mut FesScriptDriver, effect: &FesDriverEffect, shared: &mut ExpressionState,
) -> Result<Option<bool>> {
    let FesDriverEffect::Command(command) = effect else { return Ok(None); };
    let Some(raw_verb) = command.parameters.first().map(Vec::as_slice) else { return Ok(None); };
    let lower_verb = raw_verb.to_ascii_lowercase();
    let verb = lower_verb.as_slice();
    if !matches!(
        verb,
        b"sysvoice"
            | b"sysvoiceex"
            | b"getsndconfex"
            | b"setsndconfex"
            | b"rand"
            | b"bitcount"
            | b"fileexist"
            | b"steam_isenable"
            | b"steam_isbroadcast"
            | b"steam_enablescreenshot"
            | b"steam_achievement"
            | b"gettime"
            | b"clipboard"
            | b"messagebox"
            | b"dbgstr"
            | b"dbgstru8"
            | b"cursormove"
            | b"automoveex"
            | b"btnsimulate"
            | b"touch_enable"
            | b"touch_kill"
            | b"rdraw"
            | b"fw_init"
            | b"fw_auth"
            | b"fw_hash"
            | b"fw_limitpos"
            | b"fw_activate"
            | b"fw_status"
            | b"fw_actcode"
            | b"fw_uniquecode"
    ) {
        return Ok(None);
    }
    let args = &command.parameters[1..];
    if args.is_empty() && verb != b"clipboard" { return Ok(Some(true)); }
    // The FES parser already expands the NAME token. The original wrapper
    // only passes it on when a Steam service is present.
    if verb == b"steam_achievement" { return Ok(Some(true)); }
    if matches!(verb, b"fw_init" | b"fw_auth" | b"fw_hash" | b"fw_activate" | b"touch_kill") {
        return Ok(Some(true));
    }
    let query = args.get(1).map(Vec::as_slice);
    let needs_config = match verb {
        b"sysvoiceex" => host.sound_config_enabled != 0,
        b"getsndconfex" => query.is_some(),
        _ => false,
    };
    let missing_master = verb == b"getsndconfex" && query == Some(b"volume")
        && args.get(2).is_some_and(|name| trim_nul(name) == b"master")
        && host.sound_config.master_volume.is_none();
    if (needs_config && !host.sound_config.source_loaded || missing_master)
        && !host.data_services.fes_globals.warned_sound_defaults
    {
        host.data_services.fes_globals.warned_sound_defaults = true;
        host.events.push(super::LogicEvent { frame: host.frame, kind: "settings-sound-defaults".into(),
            detail: "FES sound query has no configured value; using sound defaults".into() });
    }
    if verb == b"getsndconfex" {
        if let Some(query) = query {
            if !matches!(query, b"volume_id" | b"sysplay_id" | b"play_id"
                | b"volume" | b"voicebgm" | b"volume_type" | b"play_type")
            { bail!("getsndconfex query {:?} has no documented host contract", String::from_utf8_lossy(query)); }
        }
    }
    let key = (view.id, command.offset);
    let mut progress = host.data_services.fes_globals.pending.remove(&key).unwrap_or_default();
    let result = (|| -> Result<()> {
        if verb == b"fileexist" {
            let Some(destination) = args.get(1) else { return Ok(()); };
            let path = host.text_encoding.decode(trim_nul(&args[0]));
            let output = match progress.output {
                Some(output) => output,
                None => {
                    let output = driver.evaluate_parameter(destination, command.context.current, shared)?;
                    progress.output = Some(output);
                    output
                }
            };
            let present = host.resources.resolve_mode_candidate(&path, ResourceMode::LooseFirst)?.is_some();
            write(shared, output, i32::from(present));
            return Ok(());
        }
        if verb == b"steam_enablescreenshot" {
            // The offline wrapper still evaluates the optional enable flag.
            driver.evaluate_parameter(&args[0], command.context.current, shared)?;
            return Ok(());
        }
        if apply_platform_utility(host, view, driver, command, verb, args, &mut progress, shared)? {
            return Ok(());
        }
        let output = match progress.output {
            Some(output) => output,
            None => {
                let current = if matches!(verb, b"sysvoice" | b"sysvoiceex" | b"getsndconfex" | b"setsndconfex") {
                    0
                } else {
                    command.context.current
                };
                let output = driver.evaluate_parameter(&args[0], current, shared)?;
                progress.output = Some(output);
                output
            }
        };
        if matches!(verb, b"steam_isenable" | b"steam_isbroadcast") {
            write(shared, output, 0);
            return Ok(());
        }
        if verb == b"setsndconfex" {
            let Some((setting, selector)) = query.and_then(SoundSetting::parse) else { return Ok(()); };
            if selector == SoundSelector::Id {
                for source in args.iter().skip(2 + progress.values.len()) {
                    progress.values.push(driver.evaluate_parameter(source, 0, shared)?);
                }
                if let Some(part) = sound_part_by_id(&mut host.sound_config.parts, &progress.values) {
                    setting.apply_part(part, output, true);
                }
            } else {
                let Some(raw_name) = args.get(2).map(|name| trim_nul(name)) else { return Ok(()); };
                let name = normalize_sound_name(raw_name);
                if selector == SoundSelector::Name {
                    if raw_name == b"master" && setting.apply_master(host.sound_config, output) {
                        host.data_services.language.system_settings_dirty = 1;
                        return Ok(());
                    }
                    if let Some(part) = sound_part_by_name(&mut host.sound_config.parts, &name) {
                        setting.apply_part(part, output, false);
                        if let Some(overrides) = host.sound_config.user_overrides.get_mut(&name) {
                            match setting {
                                SoundSetting::Volume => overrides.volume = None,
                                SoundSetting::VoiceBgm => overrides.voice_volume = None,
                                SoundSetting::Mute => overrides.mute = None,
                                SoundSetting::Reset => *overrides = Default::default(),
                                SoundSetting::Play | SoundSetting::SystemPlay => {}
                            }
                        }
                    }
                } else {
                    setting.apply_type(&mut host.sound_config.parts, &name, output);
                }
            }
            host.data_services.language.system_settings_dirty = 1;
            return Ok(());
        }
        if matches!(verb, b"rand" | b"bitcount") {
            let count = if verb == b"rand" { 2 } else { 1 };
            for source in args.iter().take(1 + count).skip(1 + progress.values.len()) {
                progress.values.push(driver.evaluate_parameter(source, command.context.current, shared)?);
            }
            let value = match progress.values.as_slice() {
                [value] if verb == b"bitcount" => value.count_ones() as i32,
                [first, last] if verb == b"rand" => {
                    let width = (*last as u32).wrapping_sub(*first as u32).wrapping_add(1);
                    if width == 0 { bail!("FES rand range has zero width"); }
                    first.wrapping_add((host.script_random.next_integer() % width) as i32)
                }
                _ => return Ok(()),
            };
            write(shared, output, value);
            return Ok(());
        }
        if verb == b"sysvoice" {
            host.ensure_fes_settings()?;
            if host.data_services.fes_globals.legacy_voice_channels.is_none() {
                let startup = host.resources.startup_configuration()?;
                let channels = (0..48).filter(|index| {
                    startup.as_ref().is_some_and(|document| {
                        super::xml_text(document, &format!("document/SYSTEMVOICE/ch{index:02}/name"))
                            .is_some_and(|name| !trim_nul(name).is_empty())
                    })
                }).collect();
                host.data_services.fes_globals.legacy_voice_channels = Some(channels);
            }
            while progress.values.len() < args.len() - 1 {
                let source = &args[1 + progress.values.len()];
                progress.values.push(driver.evaluate_parameter(source, 0, shared)?);
            }
            let channels = host.data_services.fes_globals.legacy_voice_channels.as_deref().unwrap_or_default();
            let requested = if args.len() == 1 { channels } else { &progress.values };
            let candidates: Vec<_> = requested.iter().copied().filter(|id| {
                channels.contains(id) && host.data_services.fes_settings.system_voice_enabled(*id)
            }).collect();
            let selected = if candidates.is_empty() { -1 } else {
                candidates[host.script_random.next_integer() as usize % candidates.len()]
            };
            write(shared, output, selected);
            return Ok(());
        }
        if verb == b"sysvoiceex" {
            if host.sound_config_enabled == 0 {
                write(shared, output, -1);
                return Ok(());
            }
            while progress.parameter < args.len().saturating_sub(1) {
                let source = &args[1 + progress.parameter];
                let components: Vec<_> = source.split(|&byte| byte == b',').collect();
                if progress.paths.len() <= progress.parameter { progress.paths.push(Vec::new()); }
                while progress.component < components.len() {
                    let value = driver.evaluate_parameter(components[progress.component], 0, shared)?;
                    progress.paths[progress.parameter].push(value);
                    progress.component += 1;
                }
                progress.parameter += 1;
                progress.component = 0;
            }
            let candidates = voice_candidates(&host.sound_config.parts, &progress.paths);
            let selected = if candidates.is_empty() { -1 } else {
                let index = host.script_random.next_integer() as usize % candidates.len();
                candidates[index]
            };
            write(shared, output, selected);
            return Ok(());
        }
        let Some(query) = query else { return Ok(()); };
        let value = if matches!(query, b"volume_id" | b"sysplay_id" | b"play_id") {
            while progress.parameter < args.len().saturating_sub(2) {
                let value = driver.evaluate_parameter(&args[2 + progress.parameter], 0, shared)?;
                progress.values.push(value);
                progress.parameter += 1;
            }
            match query {
                b"volume_id" => host.sound_config.volume(&progress.values),
                b"sysplay_id" => host.sound_config.system_voice_enabled(&progress.values),
                _ => host.sound_config.play_enabled(&progress.values),
            }
        } else {
            let Some(name) = args.get(2).map(|name| trim_nul(name)) else { return Ok(()); };
            if name.is_empty() { return Ok(()); }
            match query {
                b"volume" if name == b"master" =>
                    host.sound_config.master_volume.unwrap_or(100),
                b"volume" => host.sound_config.override_value(name, 0)
                    .unwrap_or_else(|| host.sound_config.group_volume(name)),
                b"voicebgm" => host.sound_config.override_value(name, 1)
                    .unwrap_or_else(|| host.sound_config.group_voice_volume(name)),
                b"volume_type" => host.sound_config.type_part(name).map_or(100, |part| part.volume),
                b"play_type" => host.sound_config.type_part(name).map_or(0, |part| part.play_enabled),
                _ => unreachable!(),
            }
        };
        write(shared, output, value);
        Ok(())
    })();
    if result.is_err() { host.data_services.fes_globals.pending.insert(key, progress); }
    result.map(|_| Some(true))
}

#[derive(Clone, Copy)]
enum SoundSetting { Volume, Play, SystemPlay, Mute, VoiceBgm, Reset }

#[derive(PartialEq, Eq)]
enum SoundSelector { Name, Type, Id }

impl SoundSetting {
    fn parse(query: &[u8]) -> Option<(Self, SoundSelector)> {
        let (field, selector) = if let Some(field) = query.strip_suffix(b"_id") {
            (field, SoundSelector::Id)
        } else if let Some(field) = query.strip_suffix(b"_type") {
            (field, SoundSelector::Type)
        } else {
            (query, SoundSelector::Name)
        };
        let setting = match field {
            b"volume" => Self::Volume,
            b"play" => Self::Play,
            b"sysplay" => Self::SystemPlay,
            b"mute" => Self::Mute,
            b"voicebgm" => Self::VoiceBgm,
            b"reset" => Self::Reset,
            _ => return None,
        };
        Some((setting, selector))
    }

    fn apply_master(self, config: &mut DesktopSoundConfig, value: i32) -> bool {
        let defaults = config.defaults;
        match self {
            Self::Volume => config.master_volume = Some(value),
            Self::Play if defaults.play.is_some_and(|play| play != -1) => config.master_play_enabled = Some(value),
            Self::Mute if defaults.mute.is_some_and(|mute| mute != -1) => config.master_mute = value,
            Self::Play | Self::Mute => {}
            Self::Reset => {
                config.master_volume = defaults.volume;
                config.master_play_enabled = defaults.play;
                config.master_system_voice = defaults.system_voice;
                config.master_voice_volume = defaults.voice_volume;
                config.master_mute = defaults.mute.unwrap_or(0);
            }
            Self::SystemPlay | Self::VoiceBgm => return false,
        }
        true
    }

    fn apply_part(self, part: &mut DesktopSoundPart, value: i32, reset_minus_one: bool) {
        let defaults = part.defaults;
        let (destination, default) = match self {
            Self::Volume => (&mut part.volume, defaults.volume),
            Self::Play if defaults.play != Some(-1) => (&mut part.play_enabled, defaults.play),
            Self::SystemPlay if defaults.system_voice != Some(-1) => (&mut part.system_voice, defaults.system_voice),
            Self::Mute if defaults.mute != Some(-1) => (&mut part.mute, defaults.mute),
            Self::VoiceBgm => (&mut part.voice_volume, defaults.voice_volume),
            Self::Reset => {
                part.volume = defaults.volume.unwrap_or(part.volume);
                part.play_enabled = defaults.play.unwrap_or(part.play_enabled);
                part.system_voice = defaults.system_voice.unwrap_or(part.system_voice);
                part.voice_volume = defaults.voice_volume.unwrap_or(part.voice_volume);
                part.mute = defaults.mute.unwrap_or(part.mute);
                return;
            }
            Self::Play | Self::SystemPlay | Self::Mute => return,
        };
        *destination = if reset_minus_one && value == -1 { default.unwrap_or(*destination) } else { value };
    }

    fn apply_type(self, parts: &mut [DesktopSoundPart], type_name: &[u8], value: i32) {
        for part in parts {
            if normalize_sound_name(&part.type_name) == type_name {
                self.apply_part(part, value, true);
            }
            self.apply_type(&mut part.children, type_name, value);
        }
    }
}

fn sound_part_by_name<'a>(parts: &'a mut [DesktopSoundPart], name: &[u8]) -> Option<&'a mut DesktopSoundPart> {
    if name.is_empty() { return None; }
    for part in parts {
        if normalize_sound_name(&part.name) == name { return Some(part); }
        if let Some(child) = sound_part_by_name(&mut part.children, name) { return Some(child); }
    }
    None
}

fn sound_part_by_id<'a>(mut parts: &'a mut [DesktopSoundPart], path: &[i32]) -> Option<&'a mut DesktopSoundPart> {
    let (last, parents) = path.split_last()?;
    for id in parents {
        if let Some(index) = parts.iter().position(|part| part.id == *id) {
            parts = &mut parts[index].children;
        }
    }
    parts.iter_mut().find(|part| part.id == *last)
}

fn join_text_parameters(args: &[Vec<u8>]) -> Vec<u8> {
    let mut joined = Vec::new();
    for (index, part) in args.iter().enumerate() {
        if index > 0 {
            joined.push(b' ');
        }
        joined.extend_from_slice(trim_nul(part));
    }
    joined
}

fn apply_platform_utility(
    host: &mut LoggingHost<'_>,
    view: &FesTaskView,
    driver: &mut FesScriptDriver,
    command: &FesDriverCommand,
    verb: &[u8],
    args: &[Vec<u8>],
    progress: &mut QueryProgress,
    shared: &mut ExpressionState,
) -> Result<bool> {
    match verb {
        b"gettime" => {
            let count = args.len().min(8);
            while progress.values.len() < count {
                let dest = driver.evaluate_parameter(&args[progress.values.len()], -1, shared)?;
                progress.values.push(dest);
            }
            let [year, month, weekday, day, hour, minute, second, millis] =
                super::calendar_native::local_calendar()?;
            let fields = [year, month, day, weekday, hour, minute, second, millis];
            for (&dest, value) in progress.values.iter().zip(fields) {
                write(shared, dest, value as i32);
            }
            Ok(true)
        }
        b"clipboard" => {
            let payload = join_text_parameters(args);
            host.publish_fes_clipboard(&payload);
            Ok(true)
        }
        b"messagebox" => {
            let output = match progress.output {
                Some(output) => output,
                None => {
                    let output = driver.evaluate_parameter(&args[0], -1, shared)?;
                    progress.output = Some(output);
                    output
                }
            };
            let text_start = if args.len() >= 3 {
                if progress.values.is_empty() {
                    let flags = driver.evaluate_parameter(&args[1], 0, shared)?;
                    progress.values.push(flags);
                }
                2
            } else {
                1
            };
            let text = if args.len() > text_start {
                host.text_encoding.decode(&join_text_parameters(&args[text_start..]))
            } else {
                String::new()
            };
            host.events.push(super::LogicEvent {
                frame: host.frame,
                kind: "fes-messagebox".into(),
                detail: format!("output={output} text={text:?}"),
            });
            write(shared, output, 1);
            Ok(true)
        }
        b"dbgstr" | b"dbgstru8" => {
            let raw = join_text_parameters(args);
            let encoding = if verb == b"dbgstru8" {
                super::ScriptTextEncoding::Utf8
            } else {
                host.text_encoding
            };
            let text = encoding.decode(&raw);
            host.events.push(super::LogicEvent {
                frame: host.frame,
                kind: "fes-dbgstr".into(),
                detail: text,
            });
            Ok(true)
        }
        b"cursormove" | b"automoveex" => {
            let expected = if verb == b"automoveex" { args.len().min(3) } else { args.len().min(2) };
            while progress.values.len() < expected {
                let value = driver.evaluate_parameter(&args[progress.values.len()], 0, shared)?;
                progress.values.push(value);
            }
            let x = progress.values.first().copied().unwrap_or(0);
            let y = progress.values.get(1).copied().unwrap_or(0);
            let frames = if verb == b"automoveex" {
                progress.values.get(2).copied().unwrap_or(1).clamp(1, 600) as u32
            } else {
                1
            };
            if let Err(error) = host.move_fes_pointer(view.id, [x, y], frames) {
                let detail = error.to_string();
                if detail.contains("navigation input") {
                    host.events.push(super::LogicEvent {
                        frame: host.frame,
                        kind: "fes-pointer-ignored".into(),
                        detail: format!(
                            "command={} point=({x},{y}) frames={frames}: {detail}",
                            String::from_utf8_lossy(verb)
                        ),
                    });
                } else {
                    return Err(error.into());
                }
            }
            Ok(true)
        }
        b"btnsimulate" | b"fw_limitpos" | b"touch_enable" | b"rdraw" => {
            let limit = if verb == b"btnsimulate" { args.len() } else { args.len().min(1) };
            let default = i32::from(matches!(verb, b"touch_enable" | b"rdraw"));
            while progress.values.len() < limit {
                let value = driver.evaluate_parameter(&args[progress.values.len()], default, shared)?;
                progress.values.push(value);
            }
            Ok(true)
        }
        b"fw_status" => {
            let output = match progress.output {
                Some(output) => output,
                None => {
                    let output = driver.evaluate_parameter(&args[0], command.context.current, shared)?;
                    progress.output = Some(output);
                    output
                }
            };
            write(shared, output, 2);
            Ok(true)
        }
        b"fw_actcode" | b"fw_uniquecode" => {
            let slot = match progress.output {
                Some(slot) => slot,
                None => {
                    let slot = driver.evaluate_parameter(&args[0], -1, shared)?;
                    progress.output = Some(slot);
                    slot
                }
            };
            if slot >= 0 {
                let code: &[u8] = if verb == b"fw_actcode" {
                    b"CS2-OFFLINE-ACTCODE"
                } else {
                    b"CS2-OFFLINE-UID"
                };
                shared.strings.insert(slot as u32, code.to_vec());
            }
            Ok(true)
        }
        _ => Ok(false),
    }
}

pub(super) fn release_owner(host: &mut LoggingHost<'_>, owner: u32) {
    host.data_services.fes_globals.pending.retain(|(id, _), _| *id != owner);
}

fn write(shared: &mut ExpressionState, output: i32, value: i32) {
    if output >= 0 {
        if let Some(slot) = shared.shared.get_mut(output as usize) { *slot = value; }
    }
}

fn voice_candidates(parts: &[DesktopSoundPart], paths: &[Vec<i32>]) -> Vec<i32> {
    if paths.is_empty() {
        let mut stack: Vec<_> = parts.iter().rev().collect();
        let mut candidates = Vec::new();
        while let Some(part) = stack.pop() {
            if part.system_voice == 1 { candidates.push(part.id); }
            stack.extend(part.children.iter().rev());
        }
        candidates
    } else {
        paths.iter().filter_map(|path| {
            let mut level = parts;
            let mut enabled = 1;
            for id in path {
                enabled = 1;
                if let Some(part) = level.iter().find(|part| part.id == *id) {
                    enabled = part.system_voice;
                    level = &part.children;
                }
            }
            path.last().copied().filter(|_| enabled != 0)
        }).collect()
    }
}

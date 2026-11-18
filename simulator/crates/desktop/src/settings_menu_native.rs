//! Partial 457/628 receivers. Original UI resources are data, never loaded code.
//! Only inspected pages are exposed; missing pages remain explicit gaps.
use anyhow::{Context, Result, bail};
use super::{HeadlessPlayer, HostWindowRequest, LogicEvent};
use super::ui_resources::{Reader, UiResources};

#[path = "settings_menu_general.rs"]
mod general;
#[path = "settings_menu_plugin_sound.rs"]
mod plugin_sound;
#[path = "settings_menu_plugin_image.rs"]
mod plugin_image;
#[path = "settings_menu_plugin_position.rs"]
mod plugin_position;

#[derive(Clone, Debug)]
pub struct HostEffectSettings {
    pub id: u64,
    pub title: String,
    pub pages: Vec<HostSettingsPage>,
}
#[derive(Clone, Debug)]
pub struct HostSettingsPage {
    pub resource: u32,
    pub title: String,
    pub reset_label: String,
    pub controls: Vec<HostSettingControl>,
    pub input: Option<HostInputPanel>,
    pub font: Option<HostFontPanel>,
    pub sound: Option<HostSoundPanel>,
}
#[derive(Clone, Debug)]
pub struct HostSoundPanel {
    pub all_on: String,
    pub all_off: String,
    pub first_role: usize,
    /// Original Apply searches a raw byte-name pointer with LB_FINDSTRINGEXACT W.
    /// Keep actual misses distinct from translated display-name aggregation.
    pub lookups: Vec<Option<usize>>,
    pub reset_lookups: Vec<Option<usize>>,
    pub reset_roles: Vec<usize>,
    pub named: Vec<bool>,
}
#[derive(Clone, Debug)]
pub struct HostFontPanel {
    pub faces: Vec<(String, bool)>,
    pub sample: String,
    pub default_face: String,
}
#[derive(Clone, Debug)]
pub struct HostInputPanel {
    pub key_label: String,
    pub device_label: String,
    pub keys: Vec<(u32, String)>,
    pub devices: Vec<(String, Vec<(u32, String)>)>,
    pub binding_labels: Vec<String>,
    pub repeat_label: String,
}
#[derive(Clone, Debug)]
pub struct HostSettingsEdit {
    pub reset: bool,
    pub words: Vec<u32>,
}
#[derive(Clone, Debug)]
pub enum HostSettingKind {
    Toggle,
    Choice(Vec<String>),
    Integer { minimum: u32, maximum: u32 },
    /// Stored integer ticks; original dialog displays decimal seconds.
    Decimal { scale: u32, minimum: u32, maximum: u32 },
    /// Original automatic-wait slider selects a real base/character pair.
    AutoPreset(Vec<[u32; 2]>),
}
#[derive(Clone, Debug)]
pub struct HostSettingControl {
    pub command: u32,
    pub label: String,
    pub kind: HostSettingKind,
    pub value: u32,
    pub default: u32,
    pub hidden: bool,
    pub disabled: bool,
    pub enabled_by: Option<(u32, bool)>,
    // Page484 Reset only checks radio1334; it does not write scratch a923e4.
    pub reset_changes_value: bool,
}
#[derive(Clone, Debug)]
pub struct HostMenuItem {
    pub command: u32,
    pub label: String,
    pub enabled: bool,
    pub checked: bool,
    pub separator: bool,
    pub children: Vec<HostMenuItem>,
}
#[derive(Clone, Debug)]
pub struct HostGameMenu { pub id: u64, pub resource: u32, pub visible: bool, pub items: Vec<HostMenuItem> }
#[derive(Clone, Debug)]
pub struct HostMenuConfirmation { pub id: u64, pub title: String, pub message: String }

#[derive(Default)]
pub(super) struct SettingsMenuState {
    pub(super) mode: u32,
    next_id: u64,
    effects: Option<HostEffectSettings>,
    effects_applied: bool,
    general: bool,
    camouflage_title: Option<String>,
    pending_menu: Option<HostGameMenu>,
    installed_menu: Option<HostGameMenu>,
    confirmation: Option<HostMenuConfirmation>,
    system_volume: Option<(u64, String)>,
    input_defaults: Option<[cs2_runtime::message_object::DeviceBinding; 80]>,
}

fn menu_items(r: &mut Reader<'_>, depth: usize, count: &mut usize) -> Result<Vec<HostMenuItem>> {
    if depth > 16 { bail!("original UI menu nesting exceeds 16"); }
    let mut items = Vec::new();
    loop {
        *count += 1;
        if *count > 4096 { bail!("original UI menu exceeds 4096 items"); }
        let flags = r.word()?;
        let command = if flags & 0x10 == 0 { u32::from(r.word()?) } else { 0 };
        let label = String::from_utf16(&r.text()?)?;
        let children = if flags & 0x10 != 0 { menu_items(r, depth + 1, count)? } else { Vec::new() };
        items.push(HostMenuItem { command, separator: children.is_empty() && command == 0 && label.is_empty(),
            label, enabled: flags & 3 == 0, checked: flags & 8 != 0, children });
        if flags & 0x80 != 0 { return Ok(items); }
    }
}
fn find_menu(items: &[HostMenuItem], command: u32) -> Option<&HostMenuItem> {
    for item in items {
        if item.command == command && item.children.is_empty() { return Some(item); }
        if let Some(found) = find_menu(&item.children, command) { return Some(found); }
    }
    None
}
fn remove_menu(items: &mut Vec<HostMenuItem>, command: u32) {
    items.retain(|item| item.command != command);
    for item in items { remove_menu(&mut item.children, command); }
}
pub(super) fn settings_decimal_text(value: u32, scale: u32) -> String {
    let places = if scale == 10 { 1 } else { 2 };
    format!("{}.{:0places$}", value / scale, value % scale)
}
pub(super) fn settings_decimal_value(text: &str, scale: u32, minimum: u32, maximum: u32) -> Option<u32> {
    let text = text.trim();
    if text.is_empty() { return None; }
    let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
    let whole = if whole.is_empty() { 0 } else { whole.parse::<i64>().ok()? };
    let places = if scale == 10 { 1 } else { 2 };
    let fraction = &fraction[..fraction.char_indices().nth(places).map_or(fraction.len(), |(at, _)| at)];
    let fraction = if fraction.is_empty() { 0 } else { fraction.parse::<i64>().ok()? };
    //4892c0 consumes at most one/two fractional digits without padding them.
    Some(whole.saturating_mul(i64::from(scale)).saturating_add(fraction)
        .clamp(i64::from(minimum), i64::from(maximum)) as u32)
}
pub(super) fn settings_preset_edit(page: &HostSettingsPage, words: &mut [u32], command: u32) -> bool {
    let Some((index, presets)) = page.controls.iter().enumerate().find_map(|(index, control)|
        if let HostSettingKind::AutoPreset(presets) = &control.kind { Some((index, presets)) } else { None }) else { return false; };
    if command != page.controls[index].command && command != 1127 { return false; }
    let Some(pair) = presets.get(words[index] as usize) else { return false; };
    //4864ca copies the selected pair on either edge of the detailed checkbox.
    for (command, value) in [1129, 1080].into_iter().zip(*pair) {
        if let Some(index) = page.controls.iter().position(|control| control.command == command) { words[index] = value; }
    }
    true
}
pub(super) fn settings_sound_edit(page: &HostSettingsPage, words: &mut [u32], command: u32) -> Option<(usize, u32)> {
    if !matches!(page.resource, 0x418 | 0x41e) { return None; }
    let bgm = page.controls.iter().position(|control| control.command == 1078)?;
    let during_voice = page.controls.iter().position(|control| control.command == 1095)?;
    //4851dd/485222: editing either BGM value pulls the other within its bound.
    let changed = if command == 1078 && words[during_voice] > words[bgm] { during_voice }
        else if command == 1095 && words[bgm] < words[during_voice] { bgm }
        else { return None; };
    words[changed] = words[if changed == bgm { during_voice } else { bgm }];
    Some((changed, words[changed]))
}
pub(super) fn input_view_checks(words: &[u32], key: usize, device: usize) -> [u32; 4] {
    let at = key * 6 + device * 2;
    let codes = words[at].to_le_bytes();
    let repeat = words[at + 1].to_le_bytes();
    // Preserve the original489dd0 operands, including its mouse/joypad quirks:
    // keyboard uses repeat bytes, mouse uses codes, joypad gates on code2.
    // Windows passes these raw values to CheckDlgButton and reads them back.
    std::array::from_fn(|slot| u32::from(match device {
        0 => if codes[slot] == 0 { 0 } else { repeat[slot] },
        1 => codes[slot],
        _ => if codes[1] == 0 { 0 } else { codes[slot] },
    }))
}
pub(super) fn font_face_indices(panel: &HostFontPanel, proportional: u32) -> Vec<usize> {
    panel.faces.iter().enumerate().filter_map(|(index, (_, mono))|
        (proportional != 0 || *mono).then_some(index)).collect()
}
pub(super) fn font_select(panel: &HostFontPanel, proportional: u32, name: &str) -> u32 {
    let choices = font_face_indices(panel, proportional);
    choices.iter().copied().find(|&index| panel.faces[index].0.eq_ignore_ascii_case(name))
        .or_else(|| choices.first().copied()).map_or(u32::MAX, |index| index as u32)
}
pub(super) fn font_reset(page: &HostSettingsPage, words: &mut [u32]) {
    let Some(panel) = &page.font else { return; };
    //1145 selector enters487a39, checks1300 and rebuilds the list before
    //falling through487b8c. Bold and1306 are zero; Apply stores fixed=!1306.
    words[4] = 1;
    words[0] = font_select(panel, words[4], &panel.default_face);
    for index in [1, 2, 3, 5] { words[index] = page.controls[index].default; }
}
impl HeadlessPlayer {
    fn prepare_font_settings_page(&mut self, source: &UiResources, detailed: bool) -> Result<Option<HostSettingsPage>> {
        let startup = self.with_host(|host| Ok(host.read_legacy_sound_xml("startup.xml")?))?;
        if startup.as_ref().and_then(|xml| super::xml_integer(xml, "document/CONFIG_B/font")).unwrap_or(1) == 0 { return Ok(None); }
        self.ensure_device_keys()?;
        self.data_services.message_device.load_fonts(&self.scan.resources);
        let faces = self.data_services.message_device.settings_font_faces();
        if faces.is_empty() {
            self.events.push(LogicEvent { frame: self.state.clock.frame, kind: "settings-page-unavailable".into(),
                detail: "457 font page has no actual Japanese horizontal font supported by the shared message device".into() });
            return Ok(None);
        }
        let defaults = self.data_services.messages.font_panel_defaults.as_ref()
            .context("font page has no pre-user default snapshot")?;
        let current = self.data_services.messages.defaults.as_ref().context("font page needs live message defaults")?;
        let raw = self.data_services.message_device.raw_font_controls().context("font controls have not initialized")?;
        let resource = if detailed { 0x41b } else { 0x421 };
        let (title, controls) = source.dialog(resource)?;
        let label = |id| -> Result<String> { Ok(self.resolve_ui_label(&controls.iter().find(|(command, _)| *command == id)
            .context("font resource lacks an inspected control")?.1)) };
        let face_label = controls.iter().find(|(_, text)| String::from_utf16_lossy(text)
            .starts_with(if detailed { "$0024-0005$" } else { "$0025-0005$" }))
            .map(|(_, text)| self.resolve_ui_label(text)).context("font resource has no face caption")?;
        let panel = HostFontPanel { faces, sample: label(1125)?, default_face: String::from_utf16(&defaults.font_name)? };
        let selected = font_select(&panel, raw[3], &String::from_utf16(&current.font_name)?);
        let mut fields = vec![HostSettingControl { command: 1121, label: face_label,
            kind: HostSettingKind::Integer { minimum: 0, maximum: panel.faces.len() as u32 - 1 },
            value: selected, default: selected, hidden: false, disabled: false, enabled_by: None, reset_changes_value: true }];
        for (command, value, default) in [(1132, raw[0], defaults.outline), (1130, raw[1], defaults.shadow),
            (1123, raw[2], 0), (1300, raw[3], 1), (1306, u32::from(raw[4] == 0), 0)] {
            fields.push(HostSettingControl { command, label: label(command)?, kind: HostSettingKind::Toggle,
                value, default, hidden: false, disabled: false,
                enabled_by: (command == 1306).then_some((1300, true)), reset_changes_value: true });
        }
        Ok(Some(HostSettingsPage { resource, title: self.resolve_ui_label(&title), reset_label: label(1145)?,
            controls: fields, input: None, font: Some(panel), sound: None }))
    }
    /// Real font bytes, style selection and CPU rasterizer shared with messages.
    /// The bounded packet is width/height followed by RGBA; no live setting changes.
    pub fn host_settings_font_preview(&self, id: u64, face: u32, bold: u32) -> Result<Vec<u8>> {
        let snapshot = self.host_effect_settings()?;
        if snapshot.id != id || bold > 1 { bail!("stale or invalid font preview request"); }
        let panel = snapshot.pages.iter().find_map(|page| page.font.as_ref()).context("font page is absent")?;
        let (name, _) = panel.faces.get(face as usize).context("font preview selection is absent")?;
        let canvas = self.data_services.message_device.settings_font_preview(name, bold != 0, &panel.sample)?;
        let mut bytes = Vec::with_capacity(8 + canvas.pixels().len());
        bytes.extend_from_slice(&canvas.width().to_le_bytes()); bytes.extend_from_slice(&canvas.height().to_le_bytes());
        bytes.extend_from_slice(canvas.pixels());
        Ok(bytes)
    }
    fn prepare_sound_volume_page(&mut self, source: &UiResources, detailed: bool) -> Result<Option<HostSettingsPage>> {
        let (enabled, current, defaults, independent) = self.with_host(|host| {
            let startup = host.read_legacy_sound_xml("startup.xml")?;
            host.ensure_legacy_sound_settings()?;
            Ok((startup.as_ref().and_then(|xml| super::xml_integer(xml, "document/CONFIG_B/snd")).unwrap_or(1) != 0,
                host.system_config_blocks[0].clone(), host.data_services.legacy_sound.panel_defaults.clone(), host.sound_config_enabled != 0))
        })?;
        if !enabled { return Ok(None); }
        let Some(defaults) = defaults.filter(|block| block.len() == 0x15fc && current.len() == 0x15fc && !independent) else {
            self.events.push(LogicEvent { frame: self.state.clock.frame, kind: "settings-page-unavailable".into(),
                detail: "457 legacy sound volume page needs its actual default snapshot and legacy consumer; independent SOUNDCONFIG UI remains unrecovered".into() });
            return Ok(None);
        };
        let resource = if detailed { 0x418 } else { 0x41e };
        let (title, controls) = source.dialog(resource)?;
        let label = |id| -> Result<String> { Ok(self.resolve_ui_label(&controls.iter().find(|(command, _)| *command == id)
            .context("sound resource lacks an inspected control")?.1)) };
        let caption = |token: &str| -> Result<String> { Ok(self.resolve_ui_label(&controls.iter()
            .find(|(_, text)| String::from_utf16_lossy(text).starts_with(token))
            .context("sound resource lacks an inspected caption")?.1)) };
        let captions = [caption("$0030-0012$")?, caption("$0030-0003$")?, caption("$0030-0004$")?,
            caption("$0030-0005$")?, format!("{} {}", caption("$0030-0003$")?, caption("$0030-0006$")?)];
        let word = |block: &[u8], at| u32::from_le_bytes(block[at..at + 4].try_into().unwrap());
        let mut fields = Vec::new();
        for (index, command) in [1087, 1078, 1083, 1091, 1095].into_iter().enumerate() {
            let at = 0x15d4 + index * 4;
            fields.push(HostSettingControl { command, label: captions[index].clone(),
                kind: HostSettingKind::Integer { minimum: 0, maximum: 100 }, value: word(&current, at), default: word(&defaults, at),
                hidden: false, disabled: false, enabled_by: None, reset_changes_value: true });
            let command = 1328 + index as u32;
            let at = 0x15e8 + index * 4;
            fields.push(HostSettingControl { command, label: format!("{} {}", captions[index], label(command)?),
                kind: HostSettingKind::Toggle, value: word(&current, at), default: word(&defaults, at),
                hidden: false, disabled: false, enabled_by: None, reset_changes_value: true });
        }
        fields.push(HostSettingControl { command: 1120, label: label(1120)?, kind: HostSettingKind::Toggle,
            value: word(&current, 16), default: word(&defaults, 16), hidden: false, disabled: false,
            enabled_by: None, reset_changes_value: true });
        let first_role = fields.len();
        let mut names = Vec::<String>::new();
        let mut selected = Vec::<u32>::new();
        let mut named = Vec::new();
        //484e10 builds a unique list of translated UTF8 names in record order;
        //485686 overwrites a duplicate row's selection for each later record.
        for channel in 0..48 {
            let at = 20 + channel * 116;
            let bytes = super::trim_nul(&current[at + 4..at + 36]);
            named.push(!bytes.is_empty());
            if bytes.is_empty() { continue; }
            let text = std::str::from_utf8(bytes).context("original role name is not runtime UTF8")?;
            let text = self.resolve_ui_label(&text.encode_utf16().collect::<Vec<_>>());
            let index = names.iter().position(|name| name.eq_ignore_ascii_case(&text)).unwrap_or_else(|| {
                names.push(text); selected.push(0); names.len() - 1
            });
            selected[index] = u32::from(word(&current, at) != 0);
        }
        let mut reset_selected = selected.clone();
        let mut reset_roles = Vec::new();
        for channel in 0..48 {
            let at = 20 + channel * 116;
            let bytes = super::trim_nul(&defaults[at + 4..at + 36]);
            if bytes.is_empty() { continue; }
            let text = std::str::from_utf8(bytes).context("default role name is not runtime UTF8")?;
            let text = self.resolve_ui_label(&text.encode_utf16().collect::<Vec<_>>());
            if let Some(index) = names.iter().position(|name| name.eq_ignore_ascii_case(&text)) {
                reset_selected[index] = u32::from(word(&defaults, at) != 0);
                if !reset_roles.contains(&(first_role + index)) { reset_roles.push(first_role + index); }
            }
        }
        let raw_lookups = |block: &[u8]| (0..48).map(|channel| {
            let at = 20 + channel * 116 + 4;
            // The W receiver scans UTF16 units from this byte pointer, not the
            // UTF8 display string. Bound it to the actual contiguous block.
            let units: Vec<_> = block[at..].chunks_exact(2).map(|bytes| u16::from_le_bytes([bytes[0], bytes[1]]))
                .take_while(|&unit| unit != 0).collect();
            let row = String::from_utf16(&units).ok().and_then(|name| names.iter().position(|candidate| candidate.eq_ignore_ascii_case(&name)));
            row.map(|row| first_role + row)
        }).collect::<Vec<_>>();
        let lookups = raw_lookups(&current);
        let reset_lookups = raw_lookups(&defaults);
        for (index, name) in names.into_iter().enumerate() {
            fields.push(HostSettingControl { command: 1400 + index as u32, label: name, kind: HostSettingKind::Toggle,
                value: selected[index], default: reset_selected[index], hidden: false, disabled: false,
                enabled_by: None, reset_changes_value: true });
        }
        let sound = HostSoundPanel { all_on: label(1116)?, all_off: label(1118)?, first_role, lookups, reset_lookups, reset_roles, named };
        Ok(Some(HostSettingsPage { resource, title: self.resolve_ui_label(&title), reset_label: label(1145)?, controls: fields,
            input: None, font: None, sound: Some(sound) }))
    }
    fn prepare_input_settings_page(&mut self, source: &UiResources) -> Result<Option<HostSettingsPage>> {
        let startup = self.with_host(|host| Ok(host.read_legacy_sound_xml("startup.xml")?))?
            .context("457 input settings require original startup definitions")?;
        if super::xml_integer(&startup, "document/CONFIG_B/key").unwrap_or(1) == 0 { return Ok(None); }
        self.ensure_device_keys()?;
        let defaults = *cs2_runtime::message_object::DeviceKeys::from_configuration(&startup).bindings();
        let current = *self.device_key_bindings();
        let (title, controls) = source.dialog(0x429)?;
        let label = |id| -> Result<String> { Ok(self.resolve_ui_label(&controls.iter().find(|(command, _)| *command == id)
            .context("input resource lacks an inspected control")?.1)) };
        let caption = |key: &str| -> Result<String> { Ok(self.resolve_ui_label(&controls.iter()
            .find(|(_, text)| String::from_utf16_lossy(text).starts_with(key))
            .context("input resource lacks an inspected caption")?.1)) };
        let mut keys = Vec::new();
        for (index, binding) in current.iter().enumerate() {
            let prefix = format!("document/KEYCUSTOMIZE/key{index:02}");
            if !binding.enabled || super::xml_integer(&startup, &format!("{prefix}/hide")).unwrap_or(0) != 0 { continue; }
            let text = super::xml_text(&startup, &format!("{prefix}/name")).context("enabled input slot has no original name")?;
            let text = std::str::from_utf8(super::trim_nul(text)).context("input definition name is not runtime XML UTF8")?;
            keys.push((index as u32, self.resolve_ui_label(&text.encode_utf16().collect::<Vec<_>>())));
        }
        let mut devices = Vec::new();
        for (label_token, code_token) in [("$0000-0050$", "$0010-0001$"),
            ("$0000-0051$", "$0010-0107$"), ("$0000-0052$", "$0010-0115$")] {
            let names = source.input_code_labels(code_token)?.into_iter()
                .map(|(code, text)| (code, self.resolve_ui_label(&text))).collect();
            devices.push((self.resolve_ui_label(&source.wide_label(label_token)?), names));
        }
        let input = HostInputPanel { key_label: caption("$0019-0002$")?, device_label: caption("$0019-0009$")?, keys, devices,
            binding_labels: (3..=6).map(|index| caption(&format!("$0019-{index:04}$"))).collect::<Result<_>>()?,
            repeat_label: caption("$0019-0008$")? };
        let mut fields = Vec::with_capacity(481);
        for (index, binding) in current.iter().enumerate() {
            for device in 0..3 {
                for repeat in [false, true] {
                    let packed = |binding: &cs2_runtime::message_object::DeviceBinding| {
                        if repeat { u32::from_le_bytes(binding.repeat[device].map(|value| value as u8)) }
                        else { u32::from_le_bytes(binding.codes[device]) }
                    };
                    fields.push(HostSettingControl { command: 4096 + fields.len() as u32, label: String::new(),
                        kind: HostSettingKind::Integer { minimum: 0, maximum: u32::MAX }, value: packed(binding),
                        default: packed(&defaults[index]), hidden: true, disabled: false, enabled_by: None, reset_changes_value: true });
                }
            }
        }
        fields.push(HostSettingControl { command: 1327, label: label(1327)?, kind: HostSettingKind::Toggle,
            value: self.data_services.configuration.platform_value("KEY/enable_joypad").context("input configuration is absent")?,
            //67b155 constructs the default shared flag as zero.
            default: 0, hidden: false, disabled: false, enabled_by: None, reset_changes_value: true });
        let title = self.resolve_ui_label(&title);
        let reset_label = label(1145)?;
        self.data_services.window_control.ui.input_defaults = Some(defaults);
        Ok(Some(HostSettingsPage { resource: 0x429, title, reset_label,
            controls: fields, input: Some(input), font: None, sound: None }))
    }
    fn validate_input_settings_page(&self, page: &HostSettingsPage, input: &HostInputPanel, edit: &HostSettingsEdit) -> Result<()> {
        if edit.words.len() != 481 || page.controls.len() != 481 || input.devices.len() != 3
            || self.data_services.window_control.ui.input_defaults.is_none() { bail!("input settings snapshot layout is invalid"); }
        for (index, (&value, control)) in edit.words[..480].iter().zip(&page.controls).enumerate() {
            if value == control.value || (edit.reset && value == control.default) { continue; }
            let key = (index / 6) as u32;
            if !input.keys.iter().any(|(id, _)| *id == key) { bail!("input setting attempts to edit a hidden original binding"); }
            let device = index % 6 / 2;
            for (old, new) in control.value.to_le_bytes().into_iter().zip(value.to_le_bytes()) {
                if old == new { continue; }
                if index % 2 == 0 {
                    if !input.devices[device].1.iter().any(|(code, _)| *code == u32::from(new)) {
                        bail!("input code is absent from the original device menu");
                    }
                } else if new > 1 { bail!("input repeat edit is not Boolean"); }
            }
        }
        Ok(())
    }
    fn apply_input_settings_page(&mut self, edit: &HostSettingsEdit) -> Result<()> {
        let mut bindings = if edit.reset { self.data_services.window_control.ui.input_defaults.context("input Reset has no original defaults")? }
            else { *self.device_key_bindings() };
        for (key, binding) in bindings.iter_mut().enumerate() {
            for device in 0..3 {
                let at = key * 6 + device * 2;
                binding.codes[device] = edit.words[at].to_le_bytes();
                binding.repeat[device] = edit.words[at + 1].to_le_bytes().map(u32::from);
            }
        }
        // Original489470 rebuilds every binding through669420 on Apply.
        // Use the live DeviceKeys setter, including its per-slot state reset.
        for (key, binding) in bindings.into_iter().enumerate() { self.set_device_key_binding(key as u32, binding)?; }
        self.set_host_joypad_enabled(edit.words[480]);
        Ok(())
    }
    fn original_ui_resources(&self) -> Result<UiResources> {
        let binary = self.scan.engine.primary_executable.as_ref()
            .context("settings/menu receiver needs the original PE UI resources")?;
        if binary.relative_path.is_absolute() || binary.relative_path.components().any(|part|
            !matches!(part, std::path::Component::Normal(_))) { bail!("UI source escapes game root"); }
        UiResources::read(&self.scan.root.join(&binary.relative_path))
    }
    fn resolve_ui_label(&self, text: &[u16]) -> String {
        String::from_utf16_lossy(&self.resolve_language_text(text))
    }
    fn next_ui_id(&mut self) -> Result<u64> {
        let state = &mut self.data_services.window_control.ui;
        state.next_id = state.next_id.checked_add(1).context("host UI identities exhausted")?;
        Ok(state.next_id)
    }
    pub(super) fn prepare_effect_settings(&mut self) -> Result<()> {
        self.data_services.window_control.ui.general = false;
        self.with_host(|host| { host.ensure_configuration_state()?; host.ensure_fes_settings() })?;
        let (effects, automatic, window, startup_drag, simple_alpha, alpha_only) = self.with_host(|host| {
            let startup = host.read_legacy_sound_xml("startup.xml")?;
            let enabled = |name| startup.as_ref().and_then(|xml|
                super::xml_integer(xml, name)).unwrap_or(1) != 0;
            // CONFIG_B/autoselect defaults to zero in the original parser.
            let flag = |name, default| startup.as_ref().and_then(|xml| super::xml_integer(xml, name)).unwrap_or(default) != 0;
            Ok((enabled("document/CONFIG_B/eff"), flag("document/CONFIG_B/autoselect", 0),
                enabled("document/CONFIG_B/wnd"), flag("document/GAME/enabledrag", 1),
                flag("document/CONFIG_B/smp_wndalpha", 0), flag("document/CONFIG_B/wnd_alpha", 1)))
        })?;
        let source = self.original_ui_resources()?;
        let detailed = self.data_services.configuration.platform_value("CONFIG/type").unwrap_or(0) != 0;
        let mut pages = Vec::new();
        if detailed {
            if let Some(page) = self.prepare_input_settings_page(&source)? { pages.push(page); }
        }
        if let Some(page) = self.prepare_sound_volume_page(&source, detailed)? { pages.push(page); }
        if let Some(page) = self.prepare_font_settings_page(&source, detailed)? { pages.push(page); }
        {
            let message = self.with_host(|host| {
                let startup = host.read_legacy_sound_xml("startup.xml")?;
                let value = |key, default| startup.as_ref().and_then(|xml| super::xml_integer(xml, key)).unwrap_or(default);
                Ok((value("document/CONFIG_B/mes", 1) != 0, value("document/GAME/strframe_count", 0),
                    value("document/GAME/autoframe_count", 0), value("document/GAME/enablefullskip", 0) != 0,
                    value("document/GAME/log/type", 1) != 0))
            })?;
            let bounded = (1..=65536).contains(&message.1) && (1..=32).contains(&message.2);
            if message.0 && !bounded {
                self.events.push(LogicEvent { frame: self.state.clock.frame, kind: "settings-page-unavailable".into(),
                    detail: format!("457 message page: strframe_count={}, autoframe_count={}; missing bounded original slider definitions; other recovered pages remain available", message.1, message.2) });
            }
            if message.0 && bounded {
                let resource = match (detailed, message.3) { (true, true) => 0x483, (true, false) => 0x419,
                    (false, true) => 0x482, (false, false) => 0x41f };
                let (title, controls) = source.dialog(resource)?;
                let label = |command| -> Result<String> {
                    Ok(self.resolve_ui_label(&controls.iter().find(|(id, _)| *id == command)
                        .context("message resource lacks an inspected control")?.1))
                };
                let caption = |key: &str| -> Result<String> {
                    Ok(self.resolve_ui_label(&controls.iter().find(|(_, text)| String::from_utf16_lossy(text).starts_with(key))
                        .context("message resource lacks an inspected caption")?.1))
                };
                let defaults = self.data_services.fes_settings.message_panel_defaults;
                let mut fields = Vec::new();
                for (command, offset, kind, label_text, hidden) in [
                    (1122, 0, HostSettingKind::Integer { minimum: 0, maximum: message.1 - 1 }, caption("$0021-0002$")?, false),
                    (if detailed { 1124 } else { 1150 }, 4, HostSettingKind::AutoPreset(self.data_services.fes_settings.presets[..message.2 as usize].to_vec()),
                        caption("$0021-0009$")?, false),
                    // Existing base/char words survive opening the simple page;
                    // a real preset edit replaces them together, like48b998.
                    (1129, 12, if detailed { HostSettingKind::Decimal { scale: 10, minimum: 0, maximum: 100 } }
                        else { HostSettingKind::Integer { minimum: 0, maximum: u32::MAX } },
                        if detailed { caption("$0021-0013$")? } else { String::new() }, !detailed),
                    (1080, 16, if detailed { HostSettingKind::Decimal { scale: 100, minimum: 0, maximum: 100 } }
                        else { HostSettingKind::Integer { minimum: 0, maximum: u32::MAX } },
                        if detailed { caption("$0021-0014$")? } else { String::new() }, !detailed),
                ] {
                    fields.push(HostSettingControl { command, label: label_text, kind,
                        value: u32::from_le_bytes(self.system_config_blocks[1][offset..offset + 4].try_into().unwrap()),
                        default: defaults[offset / 4], hidden, disabled: false,
                        enabled_by: if detailed { match command { 1124 => Some((1127, false)),
                            1129 | 1080 => Some((1127, true)), _ => None } } else { None }, reset_changes_value: true });
                }
                if detailed {
                    fields.push(HostSettingControl { command: 1127, label: label(1127)?, kind: HostSettingKind::Toggle,
                        value: u32::from_le_bytes(self.system_config_blocks[1][8..12].try_into().unwrap()), default: defaults[2],
                        hidden: false, disabled: false, enabled_by: None, reset_changes_value: true });
                    fields.push(HostSettingControl { command: 1322, label: caption("$0021-0017$")?,
                        kind: HostSettingKind::Integer { minimum: 10, maximum: 10000 },
                        value: u32::from_le_bytes(self.system_config_blocks[1][36..40].try_into().unwrap()), default: defaults[9],
                        hidden: !message.4, disabled: false, enabled_by: None, reset_changes_value: true });
                }
                for (command, offset) in if message.3 { vec![(1084, 20), (1086, 24), (1089, 28), (1153, 32)] }
                    else { vec![(1084, 20), (1086, 24), (1089, 28)] } {
                    fields.push(HostSettingControl { command, label: label(command)?, kind: HostSettingKind::Toggle,
                        value: u32::from_le_bytes(self.system_config_blocks[1][offset..offset + 4].try_into().unwrap()),
                        default: defaults[offset / 4], hidden: false, disabled: false, enabled_by: None, reset_changes_value: true });
                }
                pages.push(HostSettingsPage { resource, title: self.resolve_ui_label(&title), reset_label: label(1145)?, controls: fields, input: None, font: None, sound: None });
            }
        }
        if effects {
            let resource = if detailed { 0x41a } else { 0x420 };
            let (title, controls) = source.dialog(resource)?;
            let label = |command| -> Result<String> {
                Ok(self.resolve_ui_label(&controls.iter().find(|(id, _)| *id == command)
                    .context("effects resource lacks an inspected control")?.1))
            };
            let mut fields = Vec::new();
            if detailed {
                fields.push(HostSettingControl { command: 1098, label: String::new(),
                    kind: HostSettingKind::Choice([1098, 1099, 1100, 1101, 1110].into_iter()
                        .map(label).collect::<Result<_>>()?), value: 0, default: 0,
                    hidden: false, disabled: false, enabled_by: None, reset_changes_value: true });
            }
            for (command, offset, hidden, enabled_by) in if detailed {
                vec![(1309, 4, false, None), (1096, 8, true, None), (1111, 12, false, None),
                    (1113, 16, false, None), (1115, 20, false, Some((1113, true))), (1119, 24, false, None)]
            } else { vec![(1096, 8, true, None), (1111, 12, false, None), (1119, 24, false, None)] } {
                let value = u32::from_le_bytes(self.system_config_blocks[2][offset..offset + 4].try_into().unwrap());
                // Delay is an edit/spin field; its original static label has no numeric ID.
                let caption = if command == 1115 {
                    controls.iter().find(|(_, text)| String::from_utf16_lossy(text).starts_with("$0022-0006$"))
                        .map(|(_, text)| self.resolve_ui_label(text)).context("effects delay caption absent")?
                } else { label(command)? };
                fields.push(HostSettingControl { command, label: caption,
                    kind: if command == 1115 { HostSettingKind::Integer { minimum: 1, maximum: 99 } }
                        else { HostSettingKind::Toggle }, value,
                    default: self.data_services.fes_settings.effect_panel_defaults[offset / 4],
                    hidden, disabled: false, enabled_by, reset_changes_value: true });
            }
            if detailed {
                fields[0].value = u32::from_le_bytes(self.system_config_blocks[2][..4].try_into().unwrap());
                fields[0].default = self.data_services.fes_settings.effect_panel_defaults[0];
            }
            pages.push(HostSettingsPage { resource, title: self.resolve_ui_label(&title),
                reset_label: label(1145)?, controls: fields, input: None, font: None, sound: None });
        }
        if window && (detailed || startup_drag) {
            let resource = if detailed { if alpha_only { 0x6b } else { 0x41c } }
                else if simple_alpha { 0x428 } else { 0x422 };
            let (title, controls) = source.dialog(resource)?;
            let label = |command| -> Result<String> {
                Ok(self.resolve_ui_label(&controls.iter().find(|(id, _)| *id == command)
                    .context("window settings resource lacks an inspected control")?.1))
            };
            let caption = |key: &str| -> Result<String> {
                Ok(self.resolve_ui_label(&controls.iter().find(|(_, text)| String::from_utf16_lossy(text).starts_with(key))
                    .context("window settings resource lacks an inspected caption")?.1))
            };
            let packed = u32::from_le_bytes(self.system_config_blocks[3][..4].try_into().unwrap());
            let defaults = self.data_services.fes_settings.window_panel_defaults;
            let mut fields = Vec::new();
            if resource == 0x41c {
                for (command, shift, key) in [(1134, 16, "$0026-0004$"), (1136, 8, "$0026-0005$"),
                    (1138, 0, "$0026-0006$"), (1140, 24, "$0026-0007$")] {
                    fields.push(HostSettingControl { command, label: caption(key)?,
                        kind: HostSettingKind::Integer { minimum: 0, maximum: 255 }, value: (packed >> shift) & 255,
                        default: (defaults[0] >> shift) & 255, hidden: false, disabled: false, enabled_by: None, reset_changes_value: true });
                }
            } else if matches!(resource, 0x6b | 0x428) {
                fields.push(HostSettingControl { command: 1140, label: caption("$0028-0006$")?,
                    kind: HostSettingKind::Integer { minimum: 0, maximum: 100 }, value: packed >> 24,
                    default: defaults[0] >> 24, hidden: false, disabled: false, enabled_by: None, reset_changes_value: true });
            }
            for (command, offset) in if detailed { vec![(1143, 4), (1144, 8)] } else { vec![(1143, 4)] } {
                let value = u32::from_le_bytes(self.system_config_blocks[3][offset..offset + 4].try_into().unwrap());
                let disabled = command == 1143 && value == 0;
                fields.push(HostSettingControl { command, label: label(command)?, kind: HostSettingKind::Toggle,
                    value, default: if disabled { 0 } else { defaults[offset / 4] },
                    hidden: command == 1143 && !startup_drag, disabled,
                    enabled_by: None, reset_changes_value: true });
            }
            pages.push(HostSettingsPage { resource, title: self.resolve_ui_label(&title), reset_label: label(1145)?, controls: fields, input: None, font: None, sound: None });
        }
        if automatic {
            let (title, controls) = source.dialog(0x484)?;
            let label = |command| -> Result<String> {
                Ok(self.resolve_ui_label(&controls.iter().find(|(id, _)| *id == command)
                    .context("auto-selection resource lacks an inspected control")?.1))
            };
            pages.push(HostSettingsPage { resource: 0x484, title: self.resolve_ui_label(&title),
                input: None, font: None, sound: None,
                reset_label: label(1145)?, controls: vec![HostSettingControl {
                    command: 1334, label: String::new(), kind: HostSettingKind::Choice(
                        [1334, 1335, 1336].into_iter().map(label).collect::<Result<_>>()?),
                    value: match self.data_services.fes_settings.auto_select { value @ 0..=2 => value, _ => 0 }, default: 0,
                    hidden: false, disabled: false, enabled_by: None, reset_changes_value: false }] });
        }
        if pages.is_empty() { bail!("457: all recovered settings pages are disabled; other configured pages still lack receivers"); }
        let title = pages[0].title.clone();
        let request = HostEffectSettings { id: self.next_ui_id()?, title, pages };
        self.events.push(LogicEvent { frame: self.state.clock.frame,
            kind: "settings-partial-panel".into(),
            detail: format!("457: inspected pages {:?}; other enabled pages remain unimplemented",
                request.pages.iter().map(|page| page.resource).collect::<Vec<_>>()) });
        self.data_services.window_control.ui.effects = Some(request);
        self.data_services.window_control.ui.effects_applied = false;
        Ok(())
    }
    pub fn host_effect_settings(&self) -> Result<HostEffectSettings> {
        self.require_window_request(HostWindowRequest::Settings)?;
        self.data_services.window_control.ui.effects.clone().context("effects request has no snapshot")
    }
    /// UI owns edits until Apply/OK. Cancel never rewinds earlier accepted Apply.
    pub fn apply_host_effect_settings(&mut self, id: u64, values: &[HostSettingsEdit]) -> Result<()> {
        let snapshot = self.host_effect_settings()?;
        if snapshot.id != id { bail!("stale settings request identity"); }
        if values.len() != snapshot.pages.len() { bail!("settings page count mismatch"); }
        for (page, values) in snapshot.pages.iter().zip(values) {
            if values.words.len() != page.controls.len() { bail!("settings control count mismatch"); }
            for (control, &value) in page.controls.iter().zip(&values.words) {
                if value == control.value { continue; }
                if control.disabled { bail!("settings control{} is disabled by original configuration", control.command); }
                let valid = match &control.kind {
                    HostSettingKind::Toggle => value <= 1,
                    HostSettingKind::Choice(options) => (value as usize) < options.len(),
                    HostSettingKind::Integer { minimum, maximum } | HostSettingKind::Decimal { minimum, maximum, .. } => (*minimum..=*maximum).contains(&value),
                    HostSettingKind::AutoPreset(presets) => (value as usize) < presets.len(),
                };
                if !valid { bail!("settings control{} value is outside its inspected domain", control.command); }
            }
            if let Some(input) = &page.input { self.validate_input_settings_page(page, input, values)?; }
            if let Some(font) = &page.font {
                if values.words.len() != 6 || !font_face_indices(font, values.words[4]).contains(&(values.words[0] as usize)) {
                    bail!("font page has no real face in its selected proportional filter");
                }
                if font.faces[values.words[0] as usize].0.encode_utf16().count() > 63 { bail!("font name exceeds original capacity"); }
            }
            if values.reset && matches!(page.resource, 0x418 | 0x41e) &&
                self.data_services.legacy_sound.panel_defaults.as_ref().is_none_or(|block| block.len() != 0x15fc) {
                bail!("sound Reset has no original default snapshot");
            }
        }
        for (page, values) in snapshot.pages.iter().zip(values) {
            if page.input.is_some() { self.apply_input_settings_page(values)?; }
        }
        if self.data_services.window_control.ui.general {
            self.apply_configuration_plugin_settings(&snapshot.pages, values)?;
            if let Some(snapshot) = &mut self.data_services.window_control.ui.effects {
                for (page, values) in snapshot.pages.iter_mut().zip(values) {
                    for (control, &value) in page.controls.iter_mut().zip(&values.words) { control.value = value; }
                }
            }
            self.data_services.window_control.ui.effects_applied = true;
            return Ok(());
        }
        // The host only submits an edited page. Reset/click-back-to-the-same
        // value still sets the original edit dirty marker and is saved after
        // close; an unmodified OK never submits this operation.
        self.with_host(|host| {
            host.ensure_fes_settings()?;
            for (page, values) in snapshot.pages.iter().zip(values) {
                if page.input.is_some() { continue; }
                if let Some(font) = &page.font {
                    let words = &values.words;
                    host.apply_settings_font(&font.faces[words[0] as usize].0,
                        [words[1], words[2], words[3], words[4], u32::from(words[5] == 0)])?;
                    continue;
                }
                //484eb4 commits inspected fields into live a900b4, then copies
                //live back to scratch. Reset's whole scratch block is not a
                //whole-block live write; definitions/mastervol/group survive.
                if let Some(sound) = &page.sound {
                    let lookups = if values.reset { &sound.reset_lookups } else { &sound.lookups };
                    for channel in 0..48 {
                        let value = if !sound.named[channel] { Some(1) }
                            else { lookups[channel].map(|index| values.words[index]) };
                        if let Some(value) = value {
                            let at = 20 + channel * 116;
                            host.system_config_blocks[0][at..at + 4].copy_from_slice(&value.to_le_bytes());
                        }
                    }
                    host.events.push(LogicEvent { frame: host.frame, kind: "settings-role-lookup".into(),
                        detail: format!("457 original W lookup: named={}, matched={}, missed={}; misses preserve live play, empty names write1",
                            sound.named.iter().filter(|&&named| named).count(),
                            sound.named.iter().zip(lookups).filter(|(named, lookup)| **named && lookup.is_some()).count(),
                            sound.named.iter().zip(lookups).filter(|(named, lookup)| **named && lookup.is_none()).count()) });
                }
                // Effects reset32 bytes into a page-local copy, then only
                // their inspected controls write to the shared edit block.
                // Window pages instead copy their complete12-byte local block.
                if values.reset && matches!(page.resource, 0x6b | 0x41c | 0x422 | 0x428) {
                    for (index, value) in host.data_services.fes_settings.window_panel_defaults.into_iter().enumerate() {
                        host.system_config_blocks[3][index * 4..index * 4 + 4].copy_from_slice(&value.to_le_bytes());
                    }
                }
                for (control, &value) in page.controls.iter().zip(&values.words) {
                    if matches!(page.resource, 0x418 | 0x41e) {
                        if control.command >= 1400 { continue; }
                        let offset = match control.command {
                            1087 => 0x15d4, 1078 => 0x15d8, 1083 => 0x15dc, 1091 => 0x15e0, 1095 => 0x15e4,
                            1328..=1332 => 0x15e8 + (control.command as usize - 1328) * 4,
                            1120 => 16, _ => bail!("sound volume control has no inspected field"),
                        };
                        host.system_config_blocks[0][offset..offset + 4].copy_from_slice(&value.to_le_bytes());
                    } else if matches!(page.resource, 0x419 | 0x483 | 0x41f | 0x482) {
                        let offset = match control.command {
                            1122 => 0, 1124 | 1150 => 4, 1127 => 8, 1129 => 12, 1080 => 16,
                            1084 => 20, 1086 => 24, 1089 => 28, 1153 => 32, 1322 => 36,
                            _ => bail!("message setting has no inspected field"),
                        };
                        host.system_config_blocks[1][offset..offset + 4].copy_from_slice(&value.to_le_bytes());
                        // The simple receiver explicitly clears detailed mode.
                        if matches!(page.resource, 0x41f | 0x482) {
                            host.system_config_blocks[1][8..12].copy_from_slice(&0u32.to_le_bytes());
                        }
                    } else if page.resource == 0x484 {
                        host.data_services.fes_settings.auto_select = value;
                    } else if matches!(page.resource, 0x6b | 0x41c | 0x422 | 0x428) {
                        match control.command {
                            1134 | 1136 | 1138 | 1140 => {
                                let shift = match control.command { 1134 => 16, 1136 => 8, 1138 => 0, _ => 24 };
                                let old = u32::from_le_bytes(host.system_config_blocks[3][..4].try_into().unwrap());
                                let packed = (old & !(255 << shift)) | ((value & 255) << shift);
                                host.system_config_blocks[3][..4].copy_from_slice(&packed.to_le_bytes());
                            }
                            1143 | 1144 => {
                                let offset = if control.command == 1143 { 4 } else { 8 };
                                host.system_config_blocks[3][offset..offset + 4].copy_from_slice(&value.to_le_bytes());
                            }
                            _ => bail!("window setting has no inspected field"),
                        }
                    } else {
                        let offset = match control.command {
                            1098 => 0, 1309 => 4, 1096 => 8, 1111 => 12, 1113 => 16, 1115 => 20, 1119 => 24,
                            _ => bail!("settings effect control has no inspected field"),
                        };
                        host.system_config_blocks[2][offset..offset + 4].copy_from_slice(&value.to_le_bytes());
                    }
                }
                if matches!(page.resource, 0x418 | 0x41e) { host.refresh_legacy_sound_volumes()?; }
            }
            host.data_services.language.system_settings_dirty = 1;
            Ok(())
        })?;
        if let Some(snapshot) = &mut self.data_services.window_control.ui.effects {
            for (page, values) in snapshot.pages.iter_mut().zip(values) {
                for (control, &value) in page.controls.iter_mut().zip(&values.words) { control.value = value; }
            }
        }
        self.data_services.window_control.ui.effects_applied = true;
        self.events.push(LogicEvent { frame: self.state.clock.frame, kind: "settings-applied".into(),
            detail: format!("457 inspected page values {values:?}; shared FES fields and configuration dirty") });
        Ok(())
    }
    /// Bounded Android Apply record: page count, then resource/count/reset/words.
    /// Validate the entire packet before mutating shared configuration.
    pub fn apply_host_settings_payload(&mut self, id: u64, bytes: &[u8]) -> Result<()> {
        if bytes.len() > 16 * 1024 { bail!("settings Apply record exceeds limit"); }
        let snapshot = self.host_effect_settings()?;
        let mut r = Reader::new(bytes);
        if r.dword()? as usize != snapshot.pages.len() { bail!("settings Apply page count mismatch"); }
        let mut pages = Vec::new();
        for page in &snapshot.pages {
            if r.dword()? != page.resource || r.dword()? as usize != page.controls.len() {
                bail!("settings Apply page identity/count mismatch");
            }
            let reset = match r.dword()? { 0 => false, 1 => true, _ => bail!("settings Reset marker is not Boolean") };
            pages.push(HostSettingsEdit { reset,
                words: (0..page.controls.len()).map(|_| r.dword()).collect::<Result<Vec<_>>>()? });
        }
        if r.at != bytes.len() { bail!("settings Apply record has trailing bytes"); }
        self.apply_host_effect_settings(id, &pages)
    }
    pub fn close_host_effect_settings(&mut self, id: u64) -> Result<()> {
        if self.host_effect_settings()?.id != id { bail!("stale settings close identity"); }
        // 4839f9 checks the applied marker, then66c2e0 saves only after the
        // sheet closes. A later Cancel preserves earlier Apply and saves it.
        if self.data_services.window_control.ui.effects_applied && !self.data_services.window_control.ui.general {
            self.with_host(|host| host.persist_configuration())?;
        }
        self.finish_window_ui_request(HostWindowRequest::Settings)?;
        self.data_services.window_control.ui.effects = None;
        self.data_services.window_control.ui.effects_applied = false;
        self.data_services.window_control.ui.input_defaults = None;
        self.data_services.window_control.ui.general = false;
        Ok(())
    }
    pub(super) fn prepare_game_menu(&mut self) -> Result<()> {
        self.with_host(|host| host.ensure_configuration_state())?;
        let resource = if self.data_services.window_control.ui.mode == 0 { 0x68 } else { 0x69 };
        let source = self.original_ui_resources()?;
        let mut reader = Reader::new(source.resource(4, resource)?);
        let version = reader.word()?;
        if version != 0 { bail!("uninspected extended original menu template"); }
        let extra = reader.word()?; reader.take(usize::from(extra))?;
        let mut items = menu_items(&mut reader, 0, &mut 0)?;
        fn translate(player: &HeadlessPlayer, items: &mut [HostMenuItem]) {
            for item in items {
                item.label = player.resolve_ui_label(&item.label.encode_utf16().collect::<Vec<_>>());
                translate(player, &mut item.children);
            }
        }
        translate(self, &mut items);
        // 66c300 returns owner+418. The startup parser writes owner+bf50
        // (MANUAL existence) and owner+c78c (GAME/enabledrag, default1),
        // which the menu receiver reads at global+bb38/c374 respectively.
        let (manual, drag) = self.with_host(|host| {
            let startup = host.read_legacy_sound_xml("startup.xml")?;
            let manual = startup.as_ref().and_then(|xml| super::xml_text(xml, "document/MANUAL/path"))
                .map(|bytes| host.text_encoding.decode(super::trim_nul(bytes)).replace('\\', "/"));
            let drag = startup.as_ref().and_then(|xml| super::xml_integer(xml, "document/GAME/enabledrag")).unwrap_or(1);
            Ok((manual, drag))
        })?;
        let manual = manual.map(std::path::PathBuf::from).filter(|path|
            !path.is_absolute() && path.components().all(|part| matches!(part, std::path::Component::Normal(_))))
            .is_some_and(|path| self.scan.root.join(path).is_file());
        if !manual { remove_menu(&mut items, 118); }
        if drag == 0 { remove_menu(&mut items, 40034); }
        let visible = self.data_services.configuration.platform_value("WINDOW/mode") != Some(1);
        // The original display-format fallback lock (window+2c) still has no
        // corresponding host provider. Do not infer it from fullscreen mode.
        let request = HostGameMenu { id: self.next_ui_id()?, resource, visible, items };
        self.data_services.window_control.ui.pending_menu = Some(request);
        Ok(())
    }
    pub fn host_game_menu(&self) -> Result<HostGameMenu> {
        self.require_window_request(HostWindowRequest::Menu)?;
        self.data_services.window_control.ui.pending_menu.clone().context("menu request has no snapshot")
    }
    pub fn complete_host_game_menu(&mut self, id: u64) -> Result<()> {
        if self.host_game_menu()?.id != id { bail!("stale menu rebuild identity"); }
        self.finish_window_ui_request(HostWindowRequest::Menu)?;
        self.data_services.window_control.ui.installed_menu = self.data_services.window_control.ui.pending_menu.take();
        Ok(())
    }
    pub fn host_game_menu_command(&mut self, id: u64, command: u32) -> Result<()> {
        let menu = self.data_services.window_control.ui.installed_menu.as_ref().context("menu is not installed")?;
        if menu.id != id { bail!("stale installed menu identity"); }
        let item = find_menu(&menu.items, command).context("command is absent from installed original menu")?;
        if !item.enabled { bail!("original menu command is disabled"); }
        if command == 119 { return self.queue_menu_title_return(); }
        if command == 40011 { return self.queue_menu_system_volume(); }
        if command == 40057 { return self.queue_menu_general_settings(); }
        if matches!(command, 118 | 40045) { return self.queue_menu_open(command); }
        let packet = match command {
            40034 => Some([0, 20, 0, 0]),
            40047 => Some([0, 449, 0, 1]),
            40052 => Some([0, 335, 0, 1]),
            _ => None,
        };
        if let Some(packet) = packet { return self.queue_menu_task_message(packet); }
        let request = match command {
            40002 => 847,
            40021 => 846,
            40027 => 457,
            40008 | 40009 => {
                // Resolve explicit enter/leave at delivery, after preceding
                // requests have actual receipts. Two clicks must not toggle
                // twice merely because their first request is still in flight.
                return self.queue_menu_window_mode(u32::from(command == 40009));
            }
            _ => bail!("original menu command {command:#x} has no recovered host receiver"),
        };
        self.with_host(|host| { host.window_control_native(request, &[])?; Ok(()) })
    }
    pub(super) fn prepare_menu_open(&mut self, command: u32) -> Result<super::SystemOpenRequest> {
        let target = if command == 118 {
            let text = self.with_host(|host| {
                let startup = host.read_legacy_sound_xml("startup.xml")?.context("manual has no startup definition")?;
                let path = super::xml_text(&startup, "document/MANUAL/path").context("manual has no defined path")?;
                Ok(host.text_encoding.decode(super::trim_nul(path)).replace('\\', "/"))
            })?;
            let path = std::path::PathBuf::from(&text);
            if path.is_absolute() || text.contains(':') || path.components().any(|part|
                !matches!(part, std::path::Component::Normal(_))) { bail!("manual path escapes original read-only root"); }
            let extension = path.extension().and_then(|value| value.to_str()).unwrap_or("").to_ascii_lowercase();
            if matches!(extension.as_str(), "exe" | "dll" | "com" | "bat" | "cmd" | "msi" | "scr" | "pif" | "lnk") {
                bail!("manual cannot execute an original Windows program or launcher");
            }
            self.scan.root.join(path)
        } else if command == 40045 {
            // Original global+b70 is the active save directory, not the game
            // installation. The portable owner keeps that directory private.
            self.save_root.clone()
        } else { bail!("menu open command has no inspected path producer"); };
        let target = std::fs::canonicalize(target).context("resolve actual menu document/directory")?;
        let allowed = if command == 118 { &self.scan.root } else { &self.save_root };
        let allowed = std::fs::canonicalize(allowed)?;
        if !target.starts_with(&allowed) { bail!("menu open path escapes its actual resource boundary"); }
        Ok(super::SystemOpenRequest { task_id: self.current_task_id,
            target: target.to_str().context("menu open target is not Unicode")?.as_bytes().to_vec(), parameters: Vec::new(),
            directory: self.scan.root.to_str().context("menu game root is not Unicode")?.as_bytes().to_vec(),
            encoding: super::ScriptTextEncoding::Utf8 })
    }
    pub(super) fn prepare_menu_confirmation(&mut self) -> Result<()> {
        let source = self.original_ui_resources()?;
        let title = self.resolve_ui_label(&source.wide_label("$0000-0035$")?);
        let message = self.resolve_ui_label(&source.wide_label("$0000-0036$")?);
        self.data_services.window_control.ui.confirmation = Some(HostMenuConfirmation { id: self.next_ui_id()?, title, message });
        Ok(())
    }
    pub(super) fn prepare_system_volume(&mut self) -> Result<()> {
        let title = self.data_services.window_control.ui.installed_menu.as_ref()
            .and_then(|menu| find_menu(&menu.items, 40011)).context("system-volume command has no installed original menu")?.label.clone();
        let id = self.next_ui_id()?;
        self.data_services.window_control.ui.system_volume = Some((id, title));
        Ok(())
    }
    pub fn host_system_volume_request(&self) -> Result<(u64, String)> {
        self.require_window_request(HostWindowRequest::SystemVolume)?;
        self.data_services.window_control.ui.system_volume.clone().context("system-volume request has no identity")
    }
    pub fn complete_host_system_volume(&mut self, id: u64, result: std::result::Result<(), String>) -> Result<()> {
        if self.host_system_volume_request()?.0 != id { bail!("stale system-volume request identity"); }
        self.finish_window_ui_request(HostWindowRequest::SystemVolume)?;
        self.data_services.window_control.ui.system_volume = None;
        self.events.push(LogicEvent { frame: self.state.clock.frame,
            kind: if result.is_ok() { "menu-volume-accepted" } else { "menu-volume-failed" }.into(),
            detail: result.err().unwrap_or_else(|| "40011 host result accepted; prior159 delivered; Windows preserves original >=32 completion condition".into()) });
        Ok(())
    }
    pub fn host_menu_confirmation(&self) -> Result<HostMenuConfirmation> {
        self.require_window_request(HostWindowRequest::MenuConfirmation)?;
        self.data_services.window_control.ui.confirmation.clone().context("menu confirmation has no snapshot")
    }
    pub fn complete_host_menu_confirmation(&mut self, id: u64, accepted: bool) -> Result<()> {
        if self.host_menu_confirmation()?.id != id { bail!("stale menu confirmation identity"); }
        self.finish_window_ui_request(HostWindowRequest::MenuConfirmation)?;
        self.data_services.window_control.ui.confirmation = None;
        if accepted { self.accept_menu_title_return(); }
        self.events.push(LogicEvent { frame: self.state.clock.frame, kind: "menu-title-confirmation".into(),
            detail: format!("119 accepted={accepted}; prior159 already delivered; code21 queued only on Yes") });
        Ok(())
    }
    pub(super) fn request_language_menu_refresh(&mut self) -> Result<()> {
        if self.data_services.window_control.ui.installed_menu.is_some() {
            self.queue_menu_rebuild()?;
        }
        Ok(())
    }
    /// Shared little-endian payload used by the Android UI thread. Only owned
    /// strings and values cross threads, never a Rust object or VM reference.
    pub fn host_window_ui_payload(&self) -> Result<Vec<u8>> {
        fn word(out: &mut Vec<u8>, value: u32) { out.extend_from_slice(&value.to_le_bytes()); }
        fn text(out: &mut Vec<u8>, value: &str) {
            word(out, value.len() as u32); out.extend_from_slice(value.as_bytes());
        }
        fn items(out: &mut Vec<u8>, values: &[HostMenuItem]) {
            word(out, values.len() as u32);
            for item in values {
                word(out, item.command); word(out, u32::from(item.enabled));
                word(out, u32::from(item.checked)); word(out, u32::from(item.separator));
                text(out, &item.label); items(out, &item.children);
            }
        }
        let mut out = Vec::new();
        if let Ok((id, title)) = self.host_system_volume_request() {
            word(&mut out, 6); out.extend_from_slice(&id.to_le_bytes()); text(&mut out, &title);
        } else if let Ok(request) = self.host_menu_confirmation() {
            word(&mut out, 5); out.extend_from_slice(&request.id.to_le_bytes());
            text(&mut out, &request.title); text(&mut out, &request.message);
        } else if let Ok(request) = self.host_effect_settings() {
            word(&mut out, 3); out.extend_from_slice(&request.id.to_le_bytes());
            text(&mut out, &request.title);
            word(&mut out, request.pages.len() as u32);
            for page in &request.pages {
                word(&mut out, page.resource); text(&mut out, &page.title); text(&mut out, &page.reset_label);
                word(&mut out, u32::from(page.input.is_some()));
                if let Some(input) = &page.input {
                    text(&mut out, &input.key_label); text(&mut out, &input.device_label);
                    word(&mut out, input.keys.len() as u32);
                    for (key, label) in &input.keys { word(&mut out, *key); text(&mut out, label); }
                    word(&mut out, input.devices.len() as u32);
                    for (label, codes) in &input.devices {
                        text(&mut out, label); word(&mut out, codes.len() as u32);
                        for (code, label) in codes { word(&mut out, *code); text(&mut out, label); }
                    }
                    word(&mut out, input.binding_labels.len() as u32);
                    for label in &input.binding_labels { text(&mut out, label); }
                    text(&mut out, &input.repeat_label);
                }
                word(&mut out, u32::from(page.font.is_some()));
                if let Some(font) = &page.font {
                    text(&mut out, &font.sample); text(&mut out, &font.default_face);
                    word(&mut out, font.faces.len() as u32);
                    for (name, mono) in &font.faces { text(&mut out, name); word(&mut out, u32::from(*mono)); }
                }
                word(&mut out, u32::from(page.sound.is_some()));
                if let Some(sound) = &page.sound {
                    text(&mut out, &sound.all_on); text(&mut out, &sound.all_off);
                    word(&mut out, sound.first_role as u32);
                    word(&mut out, sound.reset_roles.len() as u32);
                    for &index in &sound.reset_roles { word(&mut out, index as u32); }
                }
                word(&mut out, page.controls.len() as u32);
                for control in &page.controls {
                    word(&mut out, control.command); text(&mut out, &control.label);
                    word(&mut out, control.value); word(&mut out, control.default);
                    word(&mut out, u32::from(control.hidden)); word(&mut out, u32::from(control.disabled));
                    let (dependency, enabled_value) = control.enabled_by.unwrap_or((0, true));
                    word(&mut out, dependency); word(&mut out, u32::from(enabled_value));
                    word(&mut out, u32::from(control.reset_changes_value));
                    match &control.kind {
                        HostSettingKind::Toggle => word(&mut out, 0),
                        HostSettingKind::Choice(options) => {
                            word(&mut out, 1); word(&mut out, options.len() as u32);
                            for option in options { text(&mut out, option); }
                        }
                        HostSettingKind::Integer { minimum, maximum } => {
                            word(&mut out, 2); word(&mut out, *minimum); word(&mut out, *maximum);
                        }
                        HostSettingKind::AutoPreset(presets) => {
                            word(&mut out, 3); word(&mut out, presets.len() as u32);
                            for pair in presets { for &value in pair { word(&mut out, value); } }
                        }
                        HostSettingKind::Decimal { scale, minimum, maximum } => {
                            word(&mut out, 4); word(&mut out, *scale); word(&mut out, *minimum); word(&mut out, *maximum);
                        }
                    }
                }
            }
        } else {
            let request = self.host_game_menu()?;
            word(&mut out, 4); out.extend_from_slice(&request.id.to_le_bytes());
            word(&mut out, request.resource); word(&mut out, u32::from(request.visible));
            items(&mut out, &request.items);
        }
        Ok(out)
    }
}

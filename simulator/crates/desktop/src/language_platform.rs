//! Resource-backed language tables and in-place FES image refresh.
//!
//! Window/menu, current CST text, button-region and task-specific configuration
//! effects are explicit adapters. Missing effects retain their pending request.

use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use cs2_formats::ConfigXml;
use cs2_runtime::language::{LanguageChangeRequest, LanguageState, LanguageTarget};
use cs2_runtime::resource_router::{GameLanguage, ResourceMode};
use cs2_runtime::{ResourceIndex, ResourceLocation, RuntimeError};

use super::{HeadlessPlayer, load_fes_media_image};

const MAX_TABLE_BYTES: u64 = 64 * 1024 * 1024;

pub(super) type InterfaceRefresh = fn(&mut HeadlessPlayer) -> Result<()>;
pub(super) type TextRefresh = fn(&mut HeadlessPlayer, LanguageTarget) -> Result<()>;
pub(super) type ButtonRefresh = fn(&mut HeadlessPlayer, &str) -> Result<()>;
pub(super) type ConfigurationRefresh = fn(&mut HeadlessPlayer) -> Result<bool>;
pub(super) type ResourceName = fn(&[u8]) -> Result<String>;

#[derive(Debug, Clone, Default)]
pub(super) struct LanguagePlatformState {
    configuration: TableConfiguration,
    tables: BTreeMap<u32, BTreeMap<Vec<u16>, Vec<u16>>>,
    media: BTreeMap<String, MediaSource>,
    prepared_request: Option<(LanguageChangeRequest, LanguageState)>,
    pub(super) interface_refresh: Option<InterfaceRefresh>,
    pub(super) text_refresh: Option<TextRefresh>,
    pub(super) button_refresh: Option<ButtonRefresh>,
    pub(super) configuration_refresh: Option<ConfigurationRefresh>,
    pub(super) resource_name: Option<ResourceName>,
    pub(super) adapters: super::language_adapters::LanguageAdapterState,
}

#[derive(Debug, Clone, Default)]
struct TableConfiguration {
    prefixes: Vec<Vec<u16>>,
    game_table: Vec<u16>,
    title: Option<Vec<u16>>,
}

#[derive(Debug, Clone)]
struct MediaSource {
    requested: String,
    resolved: String,
}

impl LanguagePlatformState {
    /// The same declared prefix used by the original configuration-name
    /// lookup. No selection fallback and no CSTL positional interpretation.
    pub(super) fn declared_cstl_name(&self, index: u32) -> Result<Option<String>> {
        self.configuration.prefixes.get(index as usize)
            .map(|name| String::from_utf16(name).context("declared language prefix is invalid UTF-16"))
            .transpose()
    }

    pub(crate) fn graph_has_continuation(&self) -> bool {
        self.prepared_request.is_some() || !self.media.is_empty()
            || self.adapters.has_configuration_continuation()
    }
}

struct EncodedConfiguration {
    document: ConfigXml,
    codec: &'static encoding_rs::Encoding,
}

impl HeadlessPlayer {
    /// Loads actual resources through the config category. language.xml is an
    /// optional declaration source; startup supplies APP/title. gamedef is
    /// parsed but is not treated as a user-language override without a contract.
    pub(super) fn initialize_languages(&mut self) -> Result<()> {
        let story_prefixes = self.data_services.ordinary_native_table.as_ref()
            .is_some_and(|table| table.language_query_uses_story_prefixes());
        self.data_services.language.configure_prefix_query(story_prefixes);
        let startup = optional_config(&self.scan.resources, "startup.xml")?;
        let language = optional_config(&self.scan.resources, "language.xml")?;
        let _game_defaults = optional_config(&self.scan.resources, "gamedef.xml")?;
        let declaration = language
            .as_ref()
            .filter(|xml| has_language(xml))
            .or_else(|| startup.as_ref().filter(|xml| has_language(xml)));
        let mut from_setup = false;
        if let Some(declaration) = declaration {
            self.data_services.language.languages =
                LanguageState::from_config_xml(&declaration.document);
            self.data_services.language.platform.configuration =
                TableConfiguration::from_xml(declaration, startup.as_ref())?;
            let path = self.save_root.join("setup.xml");
            match std::fs::read(&path) {
                Ok(bytes) => {
                    let document = ConfigXml::parse_runtime(&bytes).context("parse private setup.xml")?;
                    self.data_services.language.apply_user_config_xml(&document);
                    from_setup = true;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
            }
            let languages = &mut self.data_services.language.languages;
            let preferred = match self.scan.resources.game_language() {
                GameLanguage::Automatic => None,
                GameLanguage::Japanese => languages.prefixes.iter().position(|prefix| {
                    prefix.is_empty() || [b"ja".as_slice(), b"jp"].iter().any(|name| prefix.eq_ignore_ascii_case(name))
                }),
                GameLanguage::Localized => languages.prefixes.iter().position(|prefix| {
                    [b"cn".as_slice(), b"zh", b"chs", b"zh-cn", b"zh-hans"].iter()
                        .any(|name| prefix.eq_ignore_ascii_case(name))
                }),
            };
            if let Some(index) = preferred {
                languages.primary_index = index as u32;
                languages.message_index = index as u32;
            }
        } else if let Some(startup) = &startup {
            self.data_services.language.platform.configuration.title =
                wide_xml(startup, &[b"APP", b"title"])?;
        }
        let languages = self.data_services.language.languages.clone();
        let platform = &mut self.data_services.language.platform;
        let [interface_table, game_table] =
            platform.reload_tables(&self.scan.resources, LanguageTarget::Interface, &languages)?;
        if declaration.is_some() {
            let ui = languages.prefix(0).map(String::from_utf8_lossy);
            let main = languages.prefix(1).map(String::from_utf8_lossy);
            self.log.push(format!(
                "language-bootstrap from_setup={from_setup} ui_index={} ui={ui:?} main_index={} main={main:?} interface_table={interface_table:?} game_table={game_table:?}",
                languages.primary_index, languages.message_index,
            ));
        }
        self.install_default_language_adapters();
        self.data_services.language.platform.adapters.resolved_title =
            self.language_window_title()?;
        Ok(())
    }

    /// Call before the next media update. Incomplete work returns a diagnostic
    /// while retaining the queue head, dirty flag and broadcast cursor.
    pub(super) fn finish_language_frame(&mut self) -> Result<bool> {
        if self
            .data_services
            .language
            .has_pending_configuration_broadcast()
        {
            return self.finish_language_configuration();
        }
        self.refresh_language_media()?;
        loop {
            let request = self
                .data_services
                .language
                .pending_language_requests()
                .next()
                .copied();
            let Some(request) = request else { break };
            let languages = self.data_services.language.languages.clone();
            let valid_interface = languages.contains_index(languages.primary_index);
            if request.target == LanguageTarget::Interface && !valid_interface {
                self.data_services
                    .language
                    .acknowledge_language_request(request)?;
                self.data_services.language.platform.prepared_request = None;
                continue;
            }
            let prepared = self
                .data_services
                .language
                .platform
                .prepared_request
                .as_ref()
                .is_some_and(|(previous, snapshot)| *previous == request && snapshot == &languages);
            if !prepared {
                self.data_services.language.platform.reload_tables(
                    &self.scan.resources,
                    request.target,
                    &languages,
                )?;
                self.data_services.language.platform.prepared_request = Some((request, languages));
            }
            match request.target {
                LanguageTarget::Interface => {
                    let refresh = self.data_services.language.platform.interface_refresh
                        .context("interface language request retained: window/menu refresh adapter is absent")?;
                    refresh(self)?;
                }
                LanguageTarget::MainText | LanguageTarget::Subtitle => {
                    if let Some(refresh) = self.data_services.language.platform.text_refresh {
                        refresh(self, request.target)?;
                    } else if self.cst.is_some()
                        || self.fes.as_ref().is_some_and(|fes| {
                            fes.media.values().any(|layer| {
                                matches!(layer.kind.as_str(), "STRING" | "STRING8" | "NOVEL")
                            })
                        })
                    {
                        bail!(
                            "text language request retained: current text refresh adapter is absent"
                        );
                    }
                }
            }
            self.data_services
                .language
                .acknowledge_language_request(request)?;
            self.data_services.language.platform.prepared_request = None;
        }
        self.finish_language_configuration()
    }

    /// Query a loaded table using literal UTF-16 keys. Lower selector wins.
    /// A missing delimited key returns its suffix; ordinary text is unchanged.
    pub(super) fn resolve_language_text(&self, text: &[u16]) -> Vec<u16> {
        self.data_services.language.platform.resolve_text(text)
    }

    pub(super) fn language_window_title(&self) -> Result<Option<String>> {
        self.data_services
            .language
            .platform
            .configuration
            .title
            .as_ref()
            .map(|title| {
                String::from_utf16(&self.resolve_language_text(title)).context(
                    "language window title contains an unsupported isolated UTF-16 surrogate",
                )
            })
            .transpose()
    }

    /// Exact byte-name lookup only. The result is a CSTL name, never a table
    /// position inferred from a configuration index. No active text is changed.
    pub(super) fn current_cstl_language_name(&self) -> Option<&str> {
        let prefix = self.data_services.language.languages.prefix(1)?;
        self.cst
            .as_ref()?
            .localization
            .as_ref()?
            .languages
            .iter()
            .find(|name| name.as_bytes() == prefix)
            .map(String::as_str)
    }

    pub(super) fn set_language_interface_refresh(&mut self, callback: InterfaceRefresh) {
        self.data_services.language.platform.interface_refresh = Some(callback);
    }

    pub(super) fn set_language_text_refresh(&mut self, callback: TextRefresh) {
        self.data_services.language.platform.text_refresh = Some(callback);
    }

    pub(super) fn set_language_button_refresh(&mut self, callback: ButtonRefresh) {
        self.data_services.language.platform.button_refresh = Some(callback);
    }

    /// Callback owns complete tree traversal and resumable command-19 delivery;
    /// use LanguageNativeServices::consume_configuration_change for its cursor.
    pub(super) fn set_language_configuration_refresh(&mut self, callback: ConfigurationRefresh) {
        self.data_services.language.platform.configuration_refresh = Some(callback);
    }

    pub(super) fn set_language_resource_name(&mut self, callback: ResourceName) {
        self.data_services.language.platform.resource_name = Some(callback);
    }

    pub(super) fn refresh_language_media(&mut self) -> Result<()> {
        let Some(fes) = &self.fes else {
            self.data_services.language.platform.media.clear();
            return Ok(());
        };
        let names: Vec<_> = fes.media.keys().cloned().collect();
        self.data_services
            .language
            .platform
            .media
            .retain(|name, _| fes.media.contains_key(name));
        for name in names {
            let Some(layer) = self.fes.as_ref().and_then(|fes| fes.media.get(&name)) else {
                continue;
            };
            if !matches!(layer.kind.as_str(), "IMAGE" | "BUTTON") {
                continue;
            }
            let Some(current) = layer.file.as_ref() else {
                continue;
            };
            let previous = self.data_services.language.platform.media.get(&name);
            if current.is_empty() {
                if let Some(previous) = previous {
                    let old = previous.resolved.clone();
                    if let Some(layer) = self.fes.as_mut().and_then(|fes| fes.media.get_mut(&name))
                    {
                        layer.file = Some(old);
                    }
                }
                continue;
            }
            let requested = previous
                .filter(|source| source.resolved == *current)
                .map(|source| source.requested.clone())
                .unwrap_or_else(|| current.clone());
            let expanded = self
                .data_services
                .language
                .languages
                .expand_image(requested.as_bytes())?;
            let resolved = match self.data_services.language.platform.resource_name {
                _ if expanded == requested.as_bytes() => requested.clone(),
                Some(convert) => convert(&expanded)?,
                None if expanded.is_ascii() => String::from_utf8(expanded)
                    .context("ASCII FES language resource name is invalid")?,
                None => {
                    bail!("FES language resource bytes need an explicit filename encoding adapter")
                }
            };
            let changed = *current != resolved;
            if changed {
                let button = layer.kind == "BUTTON";
                let button_refresh = self.data_services.language.platform.button_refresh;
                if button && button_refresh.is_none() {
                    bail!(
                        "FES BUTTON {name:?} language source retained: input-region refresh adapter is absent"
                    );
                }
                let mut replacement = layer.clone();
                replacement.file = Some(resolved.clone());
                load_fes_media_image(&mut replacement, &self.scan.resources)
                    .with_context(|| format!("reload FES language image {name:?}"))?;
                // Commit pixels and source in the existing wrapper. Animation,
                // position, selected frame, visibility and object identity remain.
                let old = self
                    .fes
                    .as_mut()
                    .and_then(|fes| fes.media.get_mut(&name))
                    .context("FES language image disappeared during refresh")?;
                std::mem::swap(old, &mut replacement);
                if let Some(refresh) = button_refresh.filter(|_| button) {
                    if let Err(error) = refresh(self, &name) {
                        if let Some(old) =
                            self.fes.as_mut().and_then(|fes| fes.media.get_mut(&name))
                        {
                            *old = replacement;
                        }
                        return Err(error);
                    }
                }
            }
            self.data_services.language.platform.media.insert(
                name,
                MediaSource {
                    requested,
                    resolved,
                },
            );
        }
        Ok(())
    }

    fn finish_language_configuration(&mut self) -> Result<bool> {
        if !self
            .data_services
            .language
            .has_pending_configuration_change()
        {
            return Ok(true);
        }
        let refresh = self
            .data_services
            .language
            .platform
            .configuration_refresh
            .context("configuration change retained: task/FES command-19 adapter is absent")?;
        if !refresh(self)? {
            return Ok(false);
        }
        if self
            .data_services
            .language
            .has_pending_configuration_change()
        {
            bail!("configuration notification remains pending after the task/FES adapter");
        }
        Ok(true)
    }
}

impl LanguagePlatformState {
    fn resolve_text(&self, text: &[u16]) -> Vec<u16> {
        let text = wide_nul(text);
        let Some(&delimiter) = text.first()
            .filter(|&&unit| unit == u16::from(b'$') || unit == u16::from(b'#'))
        else { return text.to_vec(); };
        let Some(end) = text[1..].iter().position(|&unit| unit == delimiter).map(|i| i + 1)
        else { return text.to_vec(); };
        let key = &text[..=end];
        for table in self.tables.values() {
            if let Some(value) = table.get(key) { return value.clone(); }
        }
        text[end + 1..].to_vec()
    }
}

/// Read actual APP/FOLDER fields through the existing encoded config loader;
/// the same loaded language tables resolve APP text for window and path users.
pub(super) fn image_export_path_fields(
    resources: &ResourceIndex, language: &LanguagePlatformState,
) -> Result<[Vec<u16>; 3]> {
    let Some(startup) = optional_config(resources, "startup.xml")? else { return Ok(Default::default()); };
    let mut fields = [Vec::new(), Vec::new(), Vec::new()];
    for (index, path) in [
        [b"APP".as_slice(), b"brand".as_slice()],
        [b"APP".as_slice(), b"title".as_slice()],
        [b"FOLDER".as_slice(), b"imglib".as_slice()],
    ].iter().enumerate() {
        fields[index] = wide_xml(&startup, path)?.unwrap_or_default();
        if index < 2 { fields[index] = language.resolve_text(&fields[index]); }
    }
    Ok(fields)
}

impl TableConfiguration {
    fn from_xml(
        document: &EncodedConfiguration,
        startup: Option<&EncodedConfiguration>,
    ) -> Result<Self> {
        let mut prefixes = Vec::new();
        for index in 0..document.document.fields.len() {
            let key = format!("L{index:02}");
            let Some(prefix) = wide_xml(document, &[b"language", key.as_bytes(), b"prefix"])?
            else {
                break;
            };
            prefixes.push(prefix);
        }
        Ok(Self {
            prefixes,
            game_table: wide_xml(document, &[b"language", b"OPTTABLE"])?.unwrap_or_default(),
            title: startup
                .map(|xml| wide_xml(xml, &[b"APP", b"title"]))
                .transpose()?
                .flatten(),
        })
    }

    fn game_path(&self, languages: &LanguageState) -> Result<Vec<u16>> {
        let mut name = self.game_table.clone();
        for (index, marker) in [
            (languages.primary_index, "%LANG%"),
            (languages.message_index, "%LANG_M%"),
        ] {
            if let Some(prefix) = self
                .prefixes
                .get(index as usize)
                .or_else(|| self.prefixes.get(languages.default_index as usize))
            {
                name = expand_wide(&name, &marker.encode_utf16().collect::<Vec<_>>(), prefix)?;
            }
        }
        if name.len() > 2047 {
            bail!("language game-table path exceeds 2047 UTF-16 units");
        }
        Ok(name)
    }
}

impl LanguagePlatformState {
    fn reload_tables(
        &mut self,
        resources: &ResourceIndex,
        target: LanguageTarget,
        languages: &LanguageState,
    ) -> Result<[Option<String>; 2]> {
        match target {
            LanguageTarget::Interface => {
                let Some(prefix) = self
                    .configuration
                    .prefixes
                    .get(languages.primary_index as usize)
                else {
                    return Ok([None, None]);
                };
                let mut name: Vec<_> = "language_".encode_utf16().collect();
                name.extend_from_slice(prefix);
                name.extend(".txt".encode_utf16());
                if name.len() > 1023 {
                    bail!("language interface-table path exceeds 1023 UTF-16 units");
                }
                let interface = self.reload_table(resources, 0, &name)?;
                let name = self.configuration.game_path(languages)?;
                let game = self.reload_table(resources, 1, &name)?;
                Ok([interface, game])
            }
            LanguageTarget::MainText => {
                let name = self.configuration.game_path(languages)?;
                let mut game = self.reload_table(resources, 1, &name)?;
                if languages.contains_index(languages.message_index) {
                    game = self.reload_table(resources, 1, &name)?;
                }
                Ok([None, game])
            }
            LanguageTarget::Subtitle => Ok([None, None]),
        }
    }

    fn reload_table(
        &mut self,
        resources: &ResourceIndex,
        selector: u32,
        name: &[u16],
    ) -> Result<Option<String>> {
        self.tables.remove(&selector);
        let name = String::from_utf16(name)
            .context("language table filename contains an unsupported isolated UTF-16 surrogate")?;
        if let Some((bytes, source)) = optional_resource_mode(resources, &name, ResourceMode::LooseFirst)? {
            self.tables.insert(selector, parse_string_table(&bytes)
                .with_context(|| format!("parse language table {name:?} from {source:?}"))?);
            return Ok(Some(source));
        }
        Ok(None)
    }
}

fn optional_config(resources: &ResourceIndex, name: &str) -> Result<Option<EncodedConfiguration>> {
    if name == "startup.xml" {
        return Ok(resources.startup_configuration()?.map(|document| EncodedConfiguration {
            document,
            codec: encoding_rs::UTF_8,
        }));
    }
    let (names, mode) = match resources.category_names(b"config", name) {
        Ok(names) => (names, ResourceMode::LooseFirst),
        // Explicit bootstrap policy for an unconfigured portable host only.
        // A configured category never falls back to an unrelated loose path.
        Err(RuntimeError::InvalidResourceName(_)) => (vec![
            format!("config/{name}"),
            name.to_owned(),
            format!("system/config/{name}"),
            format!("system/{name}"),
        ], ResourceMode::File),
        Err(error) => return Err(error.into()),
    };
    for candidate in names {
        if let Some((bytes, _)) = optional_resource_mode(resources, &candidate, mode)? {
            let document = ConfigXml::parse_runtime(&bytes)
                .with_context(|| format!("parse language configuration resource {candidate:?}"))?;
            return Ok(Some(EncodedConfiguration { document, codec: encoding_rs::UTF_8 }));
        }
    }
    Ok(None)
}

fn optional_resource_mode(
    resources: &ResourceIndex, name: &str, mode: ResourceMode,
) -> Result<Option<(Vec<u8>, String)>> {
    if name.is_empty() {
        return Ok(None);
    }
    let Some(resource) = resources.resolve_mode_utf16(&name.encode_utf16().collect::<Vec<_>>(), mode)? else {
        return Ok(None);
    };
    let candidate = resource.value;
    if candidate.size > MAX_TABLE_BYTES {
        bail!("language resource exceeds 64 MiB");
    }
    let bytes = resources.read_candidate(candidate)?;
    if bytes.len() as u64 > MAX_TABLE_BYTES {
        bail!("language resource exceeds 64 MiB");
    }
    let source = match &candidate.location {
        ResourceLocation::File { .. } => candidate.name.replace('\\', "/"),
        ResourceLocation::Archive { path, .. } => format!(
            "{}::{}", path.file_name().unwrap_or(path.as_os_str()).to_string_lossy(), candidate.name,
        ),
    };
    Ok(Some((bytes, source)))
}

fn has_language(xml: &EncodedConfiguration) -> bool {
    xml_text(&xml.document, &[b"language", b"default"]).is_some()
        || xml_text(&xml.document, &[b"language", b"L00", b"prefix"]).is_some()
}

fn xml_text<'a>(xml: &'a ConfigXml, path: &[&[u8]]) -> Option<&'a [u8]> {
    let mut rooted = Vec::new();
    if xml.root == b"document" {
        rooted.push(b"document".as_slice());
    }
    rooted.extend_from_slice(path);
    xml.text(&rooted)
}

fn wide_xml(xml: &EncodedConfiguration, path: &[&[u8]]) -> Result<Option<Vec<u16>>> {
    let Some(bytes) = xml_text(&xml.document, path) else {
        return Ok(None);
    };
    let (text, failed) = xml.codec.decode_without_bom_handling(nul(bytes));
    if failed {
        bail!(
            "wide language XML field is invalid for its declared {} encoding",
            xml.codec.name()
        );
    }
    let text = text.replace("\r\n", " ").replace(['\n', '\t'], " ");
    let units: Vec<_> = text
        .chars()
        .map(|character| {
            let mut buffer = [0; 2];
            character.encode_utf16(&mut buffer)[0]
        })
        .collect();
    if units.len() > 4095 {
        bail!("wide language XML field exceeds 4095 UTF-16 units");
    }
    Ok(Some(units))
}

fn expand_wide(input: &[u16], marker: &[u16], prefix: &[u16]) -> Result<Vec<u16>> {
    let mut output = Vec::new();
    let mut offset = 0;
    while offset < input.len() {
        let units = if input[offset..].starts_with(marker) {
            offset += marker.len();
            prefix
        } else {
            let start = offset;
            offset += 1;
            &input[start..offset]
        };
        if output.len() + units.len() > 2047 {
            bail!("expanded language game-table path exceeds 2047 UTF-16 units");
        }
        output.extend_from_slice(units);
    }
    Ok(output)
}

fn parse_string_table(bytes: &[u8]) -> Result<BTreeMap<Vec<u16>, Vec<u16>>> {
    let bytes = nul(bytes);
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    let mut cleaned = Vec::new();
    let mut offset = 0;
    while offset < bytes.len() {
        if bytes[offset..].starts_with(b"//") {
            offset += 2;
            while offset < bytes.len() && !matches!(bytes[offset], b'\r' | b'\n') {
                offset += 1;
            }
        } else if bytes[offset..].starts_with(b"/*") {
            offset += 2;
            while offset < bytes.len() && !bytes[offset..].starts_with(b"*/") {
                if matches!(bytes[offset], b'\r' | b'\n') {
                    cleaned.push(bytes[offset]);
                }
                offset += 1;
            }
            if offset < bytes.len() {
                offset += 2;
            }
        } else {
            cleaned.push(if bytes[offset] == b'\t' {
                b' '
            } else {
                bytes[offset]
            });
            offset += 1;
        }
    }
    let text = std::str::from_utf8(&cleaned)
        .context("language string table contains unsupported malformed UTF-8")?;
    let lines: Vec<_> = text
        .split(['\r', '\n'])
        .filter_map(|line| {
            let line = line.strip_prefix(' ').unwrap_or(line);
            let line = line.strip_suffix(' ').unwrap_or(line);
            (!line.is_empty() && !line.starts_with(' ')).then_some(line)
        })
        .collect();
    let mut entries = BTreeMap::new();
    for pair in lines.chunks_exact(2) {
        if pair[1] == "$nr$" {
            continue;
        }
        let value = if pair[1] == "$empty$" { "" } else { pair[1] };
        entries
            .entry(pair[0].encode_utf16().collect())
            .or_insert_with(|| value.encode_utf16().collect());
    }
    Ok(entries)
}

fn nul(bytes: &[u8]) -> &[u8] {
    &bytes[..bytes
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(bytes.len())]
}

fn wide_nul(units: &[u16]) -> &[u16] {
    &units[..units
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(units.len())]
}

#[cfg(test)]
mod tests {
    use super::*;
    use cs2_script::{Host, KcsProgram, NativeReturn, Vm};
    use std::{fs, path::PathBuf, time::{SystemTime, UNIX_EPOCH}};

    #[test]
    #[ignore = "requires the installed NEKOPARA Vol. 3 and an isolated save root"]
    fn native1036_uses_the_language_layout_from_the_opened_game() -> Result<()> {
        let game = PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"), "/../../../games/",
            "[170526][NEKO WORKs] \u{30cd}\u{30b3}\u{3071}\u{3089} vol.3 steam\u{7248} (files)",
            "/nekopara_vol3",
        ));
        let saves = std::env::var_os("CS2_SIDECAR9_PROFILE").expect("isolated profile required");
        let mut player = HeadlessPlayer::open_with_save_root(game, PathBuf::from(saves).join("native1036"))?;
        let assert_query = |player: &mut HeadlessPlayer, first: &[u8], second: &[u8]| {
            player.data_services.language.languages = LanguageState {
                prefixes: vec![b"en".to_vec(), b"jp".to_vec(), b"cn".to_vec()],
                default_index: 0,
                primary_index: 0,
                message_index: 2,
                subtitle_index: 1,
            };
            player.with_host(|host| {
                let program = KcsProgram::from_code(&[0x3b], vec![0; 64], 0)?;
                let mut vm = Vm::from_program(program)?;
                let args = [4u32, 16].map(u32::to_le_bytes).concat();
                assert_eq!(host.call(1036, &args, &mut vm)?, NativeReturn::Continue);
                assert_eq!(vm.read(4, 3)?, first);
                assert_eq!(vm.read(16, 3)?, second);
                Ok(())
            })
        };
        assert_query(&mut player, b"cn\0", b"jp\0")?;

        player.data_services.ordinary_native_table = None;
        player.initialize_languages()?;
        assert_query(&mut player, b"en\0", b"cn\0")
    }

    #[test]
    fn cold_open_loads_private_ui_and_main_tables_independently() {
        struct Scratch(PathBuf);
        impl Drop for Scratch {
            fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
        }
        let suffix = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let root = std::env::temp_dir().join(format!("cs2-language-{}-{suffix}", std::process::id()));
        fs::create_dir(&root).unwrap();
        let scratch = Scratch(root);
        let game = scratch.0.join("game");
        let profile = scratch.0.join("profile");
        fs::create_dir_all(game.join("config")).unwrap();
        fs::create_dir(&profile).unwrap();
        fs::write(game.join("config/startup.xml"), br#"<?xml version="1.0" encoding="UTF-8"?>
        <document>
          <APP><title>$ui$Default title</title></APP>
          <language><default>0</default><L00><prefix>en</prefix></L00>
            <L01><prefix>jp</prefix></L01><L02><prefix>cn</prefix></L02>
            <OPTTABLE>config.int/gamelangtbl_%LANG_M%.txt</OPTTABLE></language>
          <INT><int00><name>config</name><int>config.int</int><ext>xml</ext><dir>config</dir></int00></INT>
        </document>"#).unwrap();
        for (name, table) in [
            ("language_en.txt", "$ui$\nEnglish UI\n"),
            ("language_cn.txt", "$ui$\nChinese UI\n"),
            ("config/gamelangtbl_en.txt", "$body$\nEnglish text\n"),
            ("config/gamelangtbl_cn.txt", "$body$\nChinese text\n"),
        ] {
            fs::write(game.join(name), table).unwrap();
        }
        for (setup, selections, ui, body, source) in [
            (None, [0, 0, u32::MAX], "English UI", "English text", "config/gamelangtbl_en.txt"),
            (Some((0, 2)), [0, 2, u32::MAX], "English UI", "Chinese text", "config/gamelangtbl_cn.txt"),
            (Some((1, 1)), [1, 1, u32::MAX], "Default title", "Original text", ""),
        ] {
            if let Some((interface, main)) = setup {
                fs::write(profile.join("setup.xml"), format!(
                    "<?xml version=\"1.0\" encoding=\"UTF-8\"?><document><language><id><UI>{interface}</UI><main>{main}</main></id></language></document>"
                )).unwrap();
            }
            let player = HeadlessPlayer::open_with_save_root(&game, &profile).unwrap();
            assert_eq!(player.data_services.language.languages.selections(), selections);
            assert_eq!(player.language_window_title().unwrap().as_deref(), Some(ui));
            assert_eq!(player.data_services.language.platform.adapters.resolved_title.as_deref(), Some(ui));
            let key: Vec<u16> = "$body$Original text".encode_utf16().collect();
            assert_eq!(String::from_utf16(&player.resolve_language_text(&key)).unwrap(), body);
            let receipt = player.log().iter().find(|line| line.starts_with("language-bootstrap ")).unwrap();
            assert!(receipt.contains(&format!("from_setup={}", setup.is_some())));
            let game_table = if source.is_empty() { "game_table=None".to_owned() }
                else { format!("game_table=Some({source:?})") };
            assert!(receipt.contains(&game_table), "{receipt}");
        }
    }
}

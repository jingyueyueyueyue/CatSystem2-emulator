//! Byte-oriented language selections and configuration fields.
//!
//! Prefix encoding is supplied by the host. XML loading normalizes XML field
//! whitespace but does not infer a code page from an index or declaration.

use cs2_formats::{ConfigError, ConfigXml, XmlField};
use cs2_script::{Result, ScriptError};

pub const MAX_LANGUAGE_PREFIX_BYTES: usize = 255;
pub const MAX_LANGUAGE_EXPANSION_BYTES: usize = 2047;
pub const MAX_LANGUAGE_QUERY_PREFIX_BYTES: usize = 4095;
pub const MAX_LANGUAGE_CHANGE_REQUESTS: usize = 1_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LanguageTarget {
    Interface,
    MainText,
    Subtitle,
}

impl LanguageTarget {
    pub fn from_mode(mode: u32) -> Option<Self> {
        match mode {
            0 => Some(Self::Interface),
            1 => Some(Self::MainText),
            2 => Some(Self::Subtitle),
            _ => None,
        }
    }
}

/// Selection numbers are intentionally absent: delivery reads current state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LanguageChangeRequest {
    pub task_id: u32,
    pub target: LanguageTarget,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LanguageState {
    pub prefixes: Vec<Vec<u8>>,
    pub default_index: u32,
    pub primary_index: u32,
    pub message_index: u32,
    pub subtitle_index: u32,
}

impl Default for LanguageState {
    fn default() -> Self {
        Self {
            prefixes: Vec::new(),
            default_index: 0,
            primary_index: 0,
            message_index: 0,
            subtitle_index: u32::MAX,
        }
    }
}

impl LanguageState {
    pub fn from_xml(bytes: &[u8]) -> std::result::Result<Self, ConfigError> {
        Ok(Self::from_config_xml(&ConfigXml::parse_runtime(nul_text(bytes))?))
    }

    /// Reads language/default and contiguous language/L%02d/prefix fields.
    /// An existing empty prefix is an entry; only a missing field stops scanning.
    pub fn from_config_xml(document: &ConfigXml) -> Self {
        let default_index = xml_dword(document, &[b"language", b"default"], 0);
        let mut prefixes = Vec::new();
        // Every prefix needs a distinct field, so the field count is a bound.
        for index in 0..document.fields.len() {
            let key = format!("L{index:02}");
            let Some(text) = xml_text(document, &[b"language", key.as_bytes(), b"prefix"]) else {
                break;
            };
            prefixes.push(xml_field_text(text));
        }
        Self {
            prefixes,
            default_index,
            primary_index: default_index,
            message_index: default_index,
            subtitle_index: u32::MAX,
        }
    }

    pub fn apply_user_xml(&mut self, bytes: &[u8]) -> std::result::Result<(), ConfigError> {
        let document = ConfigXml::parse_runtime(nul_text(bytes))?;
        self.apply_user_config_xml(&document);
        Ok(())
    }

    /// Missing choices retain their value before the range check. The fallback
    /// is the raw default even when that default is itself outside the table.
    pub fn apply_user_config_xml(&mut self, document: &ConfigXml) {
        let primary = xml_dword(document, &[b"language", b"id", b"UI"], self.primary_index);
        let message = xml_dword(document, &[b"language", b"id", b"main"], self.message_index);
        let subtitle = xml_dword(document, &[b"language", b"id", b"sub"], self.subtitle_index);
        self.primary_index = self.user_selection(primary);
        self.message_index = self.user_selection(message);
        self.subtitle_index = if subtitle == u32::MAX {
            subtitle
        } else {
            self.user_selection(subtitle)
        };
    }

    pub fn contains_index(&self, index: u32) -> bool {
        self.prefixes.get(index as usize).is_some()
    }

    pub fn selection(&self, target: LanguageTarget) -> u32 {
        match target {
            LanguageTarget::Interface => self.primary_index,
            LanguageTarget::MainText => self.message_index,
            LanguageTarget::Subtitle => self.subtitle_index,
        }
    }

    pub fn selections(&self) -> [u32; 3] {
        [self.primary_index, self.message_index, self.subtitle_index]
    }

    pub fn config_value(&self, name: &[u8]) -> Option<u32> {
        match name {
            b"lang_ui" => Some(self.primary_index),
            b"lang_text_main" => Some(self.message_index),
            b"lang_text_sub" => Some(self.subtitle_index),
            _ => None,
        }
    }

    /// Ordinary 1053 accepts raw DWORDs without validating the language table.
    /// None means an invalid mode or an unchanged choice, including main text.
    pub fn select_ordinary(&mut self, mode: u32, index: u32) -> Option<LanguageTarget> {
        let target = LanguageTarget::from_mode(mode)?;
        let destination = match target {
            LanguageTarget::Interface => &mut self.primary_index,
            LanguageTarget::MainText => &mut self.message_index,
            LanguageTarget::Subtitle => &mut self.subtitle_index,
        };
        if *destination == index {
            return None;
        }
        *destination = index;
        Some(target)
    }

    /// Mode 0 selects the interface prefix; mode 1 selects main text.
    /// Some(empty) is distinct from the absence of both selection and fallback.
    pub fn prefix(&self, mode: u32) -> Option<&[u8]> {
        let index = match mode {
            0 => self.primary_index,
            1 => self.message_index,
            _ => return None,
        };
        self.prefixes
            .get(index as usize)
            .or_else(|| self.prefixes.get(self.default_index as usize))
            .map(|bytes| nul_text(bytes))
    }

    pub fn query_prefix(&self, mode: u32) -> Result<&[u8]> {
        let prefix = self.prefix(mode).unwrap_or_default();
        if prefix.len() > MAX_LANGUAGE_QUERY_PREFIX_BYTES {
            return Err(language_error("language query prefix exceeds 4095 bytes"));
        }
        Ok(prefix)
    }

    /// Expansion is a single, case-sensitive byte scan. Inserted prefixes are
    /// not rescanned during this pass. All sizes exclude the terminating zero.
    pub fn expand(&self, input: &[u8], mode: u32) -> Result<Vec<u8>> {
        let input = nul_text(input);
        let marker: &[u8] = match mode {
            0 => b"%LANG%",
            1 => b"%LANG_M%",
            _ => return Ok(input.to_vec()),
        };
        let Some(prefix) = self.prefix(mode) else {
            return Ok(input.to_vec());
        };
        if prefix.len() > MAX_LANGUAGE_PREFIX_BYTES {
            return Err(language_error(
                "language expansion prefix exceeds 255 bytes",
            ));
        }

        let mut output = Vec::new();
        let mut offset = 0;
        while offset < input.len() {
            let bytes = if input[offset..].starts_with(marker) {
                offset += marker.len();
                prefix
            } else {
                let start = offset;
                offset += 1;
                &input[start..offset]
            };
            if output.len() + bytes.len() > MAX_LANGUAGE_EXPANSION_BYTES {
                return Err(language_error("language expansion exceeds 2047 bytes"));
            }
            output.extend_from_slice(bytes);
        }
        Ok(output)
    }

    /// The second pass also sees markers introduced by the first prefix.
    /// Resource category resolution and preloading do not implicitly call this.
    pub fn expand_image(&self, input: &[u8]) -> Result<Vec<u8>> {
        self.expand(&self.expand(input, 0)?, 1)
    }

    /// Called after a successful platform refresh. Interface/main retain raw
    /// indices; subtitle closes any selection not present in the table.
    pub fn finish_platform_refresh(&mut self, target: LanguageTarget) -> bool {
        match target {
            LanguageTarget::Interface => self.contains_index(self.primary_index),
            LanguageTarget::MainText => self.contains_index(self.message_index),
            LanguageTarget::Subtitle => {
                if !self.contains_index(self.subtitle_index) {
                    self.subtitle_index = u32::MAX;
                }
                true
            }
        }
    }

    /// Upserts user selections in a shared ConfigXml without touching startup
    /// prefixes or other settings. Numeric text uses signed DWORD formatting.
    pub fn write_xml(&self, document: &mut ConfigXml) -> Result<()> {
        let language_path = match document.root.as_slice() {
            b"document" => vec![b"document".to_vec(), b"language".to_vec()],
            b"language" => vec![b"language".to_vec()],
            _ => {
                return Err(language_error(
                    "language XML export requires a document or language root",
                ));
            }
        };
        ensure_xml_field(document, language_path.clone());
        let mut id_path = language_path;
        id_path.push(b"id".to_vec());
        ensure_xml_field(document, id_path.clone());
        for (name, value) in [
            (b"UI".as_slice(), self.primary_index),
            (b"main".as_slice(), self.message_index),
            (b"sub".as_slice(), self.subtitle_index),
        ] {
            let mut path = id_path.clone();
            path.push(name.to_vec());
            let text = (value as i32).to_string().into_bytes();
            match document.fields.iter_mut().find(|field| field.path == path) {
                Some(field) => field.text = text,
                None => document.fields.push(XmlField { path, text }),
            }
        }
        Ok(())
    }

    /// A language-only user document; a complete settings save should merge
    /// write_xml into its shared model instead of replacing other settings.
    pub fn to_user_xml(&self) -> Vec<u8> {
        format!(
            concat!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\r\n",
                "<document>\r\n",
                "  <language>\r\n",
                "    <id>\r\n",
                "      <UI>{}</UI>\r\n",
                "      <main>{}</main>\r\n",
                "      <sub>{}</sub>\r\n",
                "    </id>\r\n",
                "  </language>\r\n",
                "</document>\r\n",
            ),
            self.primary_index as i32, self.message_index as i32, self.subtitle_index as i32,
        )
        .into_bytes()
    }

    fn user_selection(&self, index: u32) -> u32 {
        if self.contains_index(index) {
            index
        } else {
            self.default_index
        }
    }
}

/// Keep one cache per ordinary task for its entire lifetime, including periods
/// without a program. Enqueue returned messages without executing that VM.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LanguageNotificationCache {
    selections: [u32; 3],
}

impl LanguageNotificationCache {
    pub fn new(languages: &LanguageState) -> Self {
        Self {
            selections: languages.selections(),
        }
    }

    pub fn selections(&self) -> [u32; 3] {
        self.selections
    }

    pub fn configuration_messages(
        &mut self,
        sender: u32,
        languages: &LanguageState,
        has_program: bool,
    ) -> Vec<[u32; 4]> {
        let current = languages.selections();
        let changed = current != self.selections;
        self.selections = current;
        let mut messages = Vec::new();
        if has_program {
            messages.push([sender, 0xffff_0011, 0, 0]);
            if changed {
                messages.push([sender, 0xffff_001a, 0, 0]);
            }
        }
        messages
    }
}

fn nul_text(bytes: &[u8]) -> &[u8] {
    &bytes[..bytes
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(bytes.len())]
}

fn language_error(message: &'static str) -> ScriptError {
    ScriptError::InvalidStructuredText { offset: 0, message }
}

fn xml_text<'a>(document: &'a ConfigXml, path: &[&[u8]]) -> Option<&'a [u8]> {
    let mut rooted = Vec::with_capacity(path.len() + 1);
    if document.root == b"document" {
        rooted.push(b"document".as_slice());
    }
    rooted.extend_from_slice(path);
    document.text(&rooted)
}

fn xml_field_text(bytes: &[u8]) -> Vec<u8> {
    let bytes = nul_text(bytes);
    let mut result = Vec::with_capacity(bytes.len());
    let mut offset = 0;
    while offset < bytes.len() {
        match bytes[offset] {
            b'\r' if bytes.get(offset + 1) == Some(&b'\n') => {
                result.push(b' ');
                offset += 2;
            }
            b'\n' | b'\t' => {
                result.push(b' ');
                offset += 1;
            }
            byte => {
                result.push(byte);
                offset += 1;
            }
        }
    }
    result
}

fn xml_dword(document: &ConfigXml, path: &[&[u8]], default: u32) -> u32 {
    let Some(text) = xml_text(document, path) else {
        return default;
    };
    let text = xml_field_text(text);
    let mut digits = text.as_slice();
    let negative = digits.first() == Some(&b'-');
    if matches!(digits.first(), Some(b'-' | b'+')) {
        digits = &digits[1..];
    }
    let radix = if digits.starts_with(b"0x") || digits.starts_with(b"0X") {
        digits = &digits[2..];
        16
    } else {
        10
    };
    let mut value = 0_u32;
    let mut consumed = 0;
    for &byte in digits {
        let digit = match byte {
            b'0'..=b'9' => u32::from(byte - b'0'),
            b'a'..=b'f' if radix == 16 => u32::from(byte - b'a') + 10,
            b'A'..=b'F' if radix == 16 => u32::from(byte - b'A') + 10,
            _ => break,
        };
        value = value.wrapping_mul(radix).wrapping_add(digit);
        consumed += 1;
    }
    if radix == 10 && consumed != 0 && digits.get(consumed) == Some(&b'.') {
        return 0;
    }
    if negative {
        value.wrapping_neg()
    } else {
        value
    }
}

fn ensure_xml_field(document: &mut ConfigXml, path: Vec<Vec<u8>>) {
    if !document.fields.iter().any(|field| field.path == path) {
        document.fields.push(XmlField {
            path,
            text: Vec::new(),
        });
    }
}

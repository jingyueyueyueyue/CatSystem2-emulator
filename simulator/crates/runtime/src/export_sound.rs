//! Sound-scheme definition consumed after a type46 ZT export.
//! The scheme keeps original XML value bytes until a platform adapter converts
//! them for the registry. Empty/#no_change fields inherit the current sound.
use cs2_formats::ConfigXml;

use crate::{RuntimeError, RuntimeResult};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportSoundScheme {
    pub theme: Vec<u8>,
    pub events: Vec<ExportSoundEvent>,
    pub encoding: ExportSoundEncoding,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportSoundEncoding {
    Script,
    Cp932,
    Gbk,
    Utf8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportSoundEvent {
    pub name: Vec<u8>,
    pub file: ExportSoundFile,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExportSoundFile {
    KeepCurrent,
    Mute,
    File(Vec<u8>),
}

impl ExportSoundScheme {
    pub fn decode(bytes: &[u8]) -> RuntimeResult<Self> {
        let encoding = declared_encoding(bytes)?;
        let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
        let xml = ConfigXml::parse_runtime(bytes).map_err(|cause| invalid(cause.to_string()))?;
        if xml.root != b"document" {
            return Err(invalid(
                "sound-set definition requires its document root".into(),
            ));
        }
        let theme = xml
            .text(&[b"document", b"GENERAL", b"theme"])
            .ok_or_else(|| invalid("sound-set GENERAL/theme is absent".into()))?;
        let theme = field(theme)?;
        if theme.is_empty() {
            return Err(invalid("sound-set theme is empty".into()));
        }
        let mut events = Vec::new();
        // Both identified constructors enumerate 0..98, rather than stopping
        // at the first missing event or accepting arbitrary XML child names.
        for index in 0..99 {
            let event = format!("event{index:02}");
            let Some(name) = xml.text(&[b"document", b"SOUND", event.as_bytes(), b"name"]) else {
                continue;
            };
            let name = field(name)?;
            if name.is_empty() {
                continue;
            }
            let file = xml
                .text(&[b"document", b"SOUND", event.as_bytes(), b"file"])
                .map(field)
                .transpose()?
                .unwrap_or_else(|| b"#no_change".to_vec());
            let file = match file.as_slice() {
                b"" | b"#no_change" => ExportSoundFile::KeepCurrent,
                b"#mute" => ExportSoundFile::Mute,
                _ => ExportSoundFile::File(file),
            };
            events.push(ExportSoundEvent { name, file });
        }
        Ok(Self {
            theme,
            events,
            encoding,
        })
    }
}

fn declared_encoding(bytes: &[u8]) -> RuntimeResult<ExportSoundEncoding> {
    if bytes.starts_with(&[0xff, 0xfe]) || bytes.starts_with(&[0xfe, 0xff]) {
        return Err(invalid(
            "UTF-16 sound-set XML decoding is not recovered".into(),
        ));
    }
    let header = &bytes[..bytes.len().min(1024)];
    let Some(begin) = header.windows(5).position(|part| part == b"<?xml") else {
        return Ok(ExportSoundEncoding::Script);
    };
    let rest = &header[begin + 5..];
    let end = rest
        .windows(2)
        .position(|part| part == b"?>")
        .ok_or_else(|| invalid("sound-set XML declaration is unterminated".into()))?;
    let declaration = &rest[..end];
    let Some(at) = declaration
        .windows(8)
        .position(|part| part.eq_ignore_ascii_case(b"encoding"))
    else {
        return Ok(ExportSoundEncoding::Script);
    };
    let rest = declaration[at + 8..].trim_ascii_start();
    let rest = rest
        .strip_prefix(b"=")
        .ok_or_else(|| invalid("sound-set XML encoding is malformed".into()))?
        .trim_ascii_start();
    let quote = rest
        .first()
        .filter(|&&v| matches!(v, b'\'' | b'"'))
        .ok_or_else(|| invalid("sound-set XML encoding lacks quotes".into()))?;
    let end = rest[1..]
        .iter()
        .position(|v| v == quote)
        .ok_or_else(|| invalid("sound-set XML encoding is unterminated".into()))?;
    match rest[1..1 + end].to_ascii_lowercase().as_slice() {
        b"shift_jis" | b"shift-jis" | b"cp932" | b"windows-31j" => Ok(ExportSoundEncoding::Cp932),
        b"utf-8" | b"utf8" => Ok(ExportSoundEncoding::Utf8),
        b"gbk" | b"gb2312" => Ok(ExportSoundEncoding::Gbk),
        _ => Err(invalid(
            "sound-set XML encoding has no recovered decoder".into(),
        )),
    }
}

fn field(bytes: &[u8]) -> RuntimeResult<Vec<u8>> {
    let end = bytes
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(bytes.len());
    let bytes = &bytes[..end];
    if bytes.len() > 511 {
        return Err(invalid("sound-set field exceeds its native buffer".into()));
    }
    Ok(bytes.to_vec())
}
fn invalid(message: String) -> RuntimeError {
    RuntimeError::Host(message)
}

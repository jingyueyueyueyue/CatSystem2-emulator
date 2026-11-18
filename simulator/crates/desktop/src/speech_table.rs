//! Owned APP/ttstable selection, independently recovered from the fixed PE.
//! XML/data only: no original speech DLL is loaded or called.
use cs2_formats::ConfigXml;
use cs2_runtime::{ResourceIndex, RuntimeError, RuntimeResult};

use super::sound_speech::SpeechConfiguration;

const MAX_TABLE_BYTES: u64 = 1024 * 1024;
const MAX_VOICES: usize = 256;

pub(super) struct SpeechTableSelection {
    pub(super) configuration: SpeechConfiguration,
    pub(super) id: String,
}

fn error(message: impl Into<String>) -> RuntimeError {
    RuntimeError::Host(message.into())
}

pub(super) fn xml_string(xml: &ConfigXml, key: &str, capacity: usize) -> RuntimeResult<Option<String>> {
    super::xml_text(xml, key).map(|bytes| {
        let text = std::str::from_utf8(super::trim_nul(bytes))
            .map_err(|_| error(format!("speech XML {key} is not normalized UTF-8")))?;
        if text.encode_utf16().count() >= capacity {
            return Err(error(format!("speech XML {key} exceeds its UTF-16 field capacity")));
        }
        Ok(text.to_owned())
    }).transpose()
}

pub(super) fn load(
    resources: &ResourceIndex, filename: &str, selected_id: &str,
) -> RuntimeResult<SpeechTableSelection> {
    // 66e552 -> 67f630 -> 682800 -> 52e8a0 opens the configured wide
    // filename with CreateFileW, not category or patch resource routing.
    let resource = resources.resolve_plain_file(filename)?
        .ok_or_else(|| error(format!("configured speech table is unavailable: {filename}")))?;
    if resource.size > MAX_TABLE_BYTES {
        return Err(error("speech voice table exceeds the host1 MiB bound"));
    }
    let bytes = resources.read_candidate(resource)?;
    let xml = ConfigXml::parse_runtime(&bytes)
        .map_err(|cause| error(format!("speech voice table XML: {cause}")))?;
    let dictionary = xml_string(&xml, "document/dic", 260)?.unwrap_or_default();
    if dictionary.is_empty() {
        return Err(error("configured speech table has no dictionary path"));
    }
    // PE0x26e5ef..0x26e614 reads root volume (default f32 1), stores
    // double, and0x129fc1 passes it to the original provider's vtable+2C.
    // Its units/conversion to HTS have not been established. Do not guess
    // linear gain or decibels, or silently synthesize at a different volume.
    if let Some(value) = super::xml_text(&xml, "document/volume") {
        let value = std::str::from_utf8(super::trim_nul(value)).ok()
            .and_then(|text| text.trim().parse::<f32>().ok())
            .ok_or_else(|| error("speech table volume is not a float"))?;
        if value != 1. {
            return Err(error("nondefault speech table volume requires the original provider unit mapping"));
        }
    }
    let default = super::xml_integer(&xml, "document/default").unwrap_or(0) as usize;
    let mut voices = Vec::new();
    for index in 0..=MAX_VOICES {
        let prefix = format!("document/tbl{index:02}");
        let Some(id) = xml_string(&xml, &format!("{prefix}/id"), 64)? else { break; };
        if id.is_empty() { break; }
        if index == MAX_VOICES {
            return Err(error("speech voice table exceeds the host256-entry bound"));
        }
        let voice = xml_string(&xml, &format!("{prefix}/voice"), 260)?.unwrap_or_default();
        voices.push((id, voice));
    }
    // Exact UTF-16 id comparison in0x26e850..0x26e893; first match wins.
    // If absent, the unsigned default index selects a row. Neither a row
    // number nor its UI name is a model filename.
    let selected = voices.iter().find(|(id, _)| id == selected_id)
        .or_else(|| voices.get(default))
        .ok_or_else(|| error("speech voice table has no matching id or valid default row"))?;
    if selected.1.is_empty() {
        return Err(error("selected speech table row has no voice model path"));
    }
    Ok(SpeechTableSelection {
        configuration: SpeechConfiguration {
            native_first: dictionary,
            native_second: selected.1.clone(),
        },
        id: selected.0.clone(),
    })
}

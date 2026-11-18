//! Ordinary 674: filename/category formatting and the startup INT slot query.
//! Independently recovered from Yukikoi RVA 0x191670 and 0x26c920.
use cs2_formats::ConfigXml;
use cs2_script::{NativeReturn, Result, Vm};

use super::{
    LoggingHost, expand_kcs_format, native_state_error, normalize_sound_name, read_u32, trim_nul,
};

impl LoggingHost<'_> {
    pub(super) fn resource_category_native674(
        &mut self,
        args: &[u8],
        vm: &Vm,
    ) -> Result<NativeReturn> {
        if args.len() != 8 {
            return Err(native_state_error(
                "native 674 requires exactly 8 argument bytes",
            ));
        }
        // The wrapper expands arg0 into buffer0, arg1 into buffer1, then
        // pushes buffer0 followed by buffer1. Thus the helper's category
        // (its first argument, compared with "sound") is wrapper arg1.
        let file = formatted(vm, read_u32(args, 0)?)?;
        let category = formatted(vm, read_u32(args, 4)?)?;
        Ok(NativeReturn::Value(
            self.resource_category_slot_name(&category, &file)?,
        ))
    }

    /// Names remain encoded bytes. The result identifies an INT *name*,
    /// independently of resource existence or the slot's archive filename.
    pub(super) fn resource_category_slot_name(
        &mut self,
        category: &[u8],
        file: &[u8],
    ) -> Result<u32> {
        let category = trim_nul(category);
        let selected = if normalize_sound_name(category) == b"SOUND" {
            if self.sound_config_enabled != 0 {
                let Some(part) = self.sound_config.file_part(trim_nul(file)) else {
                    return Ok(u32::MAX);
                };
                trim_nul(&part.archive).to_vec()
            } else {
                let Some(archive) = self.legacy_sound_archive_name(file)? else {
                    return Ok(u32::MAX);
                };
                archive
            }
        } else {
            category.to_vec()
        };
        let startup = self.read_legacy_sound_xml("startup.xml")?;
        startup_slot(startup.as_ref(), &selected)
    }

    /// Sound loading shares674's real INT-name selection, including an
    /// explicitly empty identifier. The shared router selects per-slot order.
    pub(super) fn media_category_candidates(
        &mut self,
        category: &[u8],
        source: &str,
    ) -> Result<Option<Vec<String>>> {
        let source = source.split('\0').next().unwrap_or_default().to_owned();
        if source.rfind(['/', '\\']).is_some_and(|offset| offset > 0) {
            return Ok(Some(vec![source]));
        }
        // 52b810 calls 66c920, whose "sound" branch selects an INT name
        // from soundconf/VOICE using the file name. "sound" need not be a
        // literal INT name; reuse674's existing selector and live settings.
        let file = if normalize_sound_name(trim_nul(category)) == b"SOUND" {
            super::sound_native::encode_name(self.text_encoding, &source)?
        } else {
            Vec::new()
        };
        let slot = self.resource_category_slot_name(category, &file)?;
        if slot == u32::MAX {
            return Ok(None);
        }
        self.resources.category_slot_name(slot, &source).map(|name| Some(vec![name]))
            .map_err(|cause| self.sound_host_error(cause))
    }
}

fn formatted(vm: &Vm, address: u32) -> Result<Vec<u8>> {
    let expanded = expand_kcs_format(vm, &vm.read_string(address)?, 0)?;
    let name = trim_nul(&expanded);
    if name.len() >= 260 {
        return Err(native_state_error(
            "native 674 formatted filename/category exceeds 259 bytes",
        ));
    }
    Ok(name.to_vec())
}

fn startup_slot(config: Option<&ConfigXml>, category: &[u8]) -> Result<u32> {
    let category = normalize_sound_name(category);
    let mut selected = None;
    for slot in 0..128u32 {
        let key = format!("int{slot:02}");
        let path = [b"document".as_slice(), b"INT", key.as_bytes(), b"name"];
        // Missing fields are the original zero-initialized empty slots.
        // Later occurrences overwrite the same field without discarding
        // other fields from an earlier INT/slot element.
        let name = config
            .and_then(|xml| last_field(xml, &path))
            .map_or(b"".as_slice(), trim_nul);
        if name.len() >= 32 {
            return Err(native_state_error(
                "native 674 startup INT name exceeds 31 bytes",
            ));
        }
        // This folding skips both bytes of CP932 double-byte characters;
        // an ASCII-range trail byte must not be folded independently.
        if selected.is_none() && normalize_sound_name(name) == category {
            selected = Some(slot);
        }
    }
    Ok(selected.unwrap_or(u32::MAX))
}

fn last_field<'a>(config: &'a ConfigXml, path: &[&[u8]]) -> Option<&'a [u8]> {
    config.fields.iter().rev().find(|field| {
        field.path.iter().map(Vec::as_slice).eq(path.iter().copied())
    }).map(|field| field.text.as_slice())
}

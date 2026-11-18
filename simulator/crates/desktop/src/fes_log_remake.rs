//! Resource producer for the Unicode LOGSTRING remake receiver.
//! Recovered from Yukikoi 63de0e14: 2d87e0 -> 2bed60 -> 28d390.
//! This is not the clear/append history-import protocol of ordinary1034.
use super::*;
use cs2_formats::{CstFile, CstlTable};

struct Scene {
    document: CstFile,
    localization: Option<CstlTable>,
}

/// Called with the live text store taken out of DataServices. Bank and language
/// expressions are evaluated once, in receiver order, including broadcasts.
pub(crate) fn apply(
    host: &mut LoggingHost<'_>, store: &mut FesTextStore, owner: u32,
    command: &FesDriverCommand, driver: &mut FesScriptDriver,
    shared: &mut ExpressionState,
) -> Result<bool> {
    if command.parameters.get(1).map(Vec::as_slice) != Some(b"remake".as_slice()) {
        return Ok(false);
    }
    let Some(target) = command.parameters.first() else { return Ok(false) };
    let (name, selection) = split_target(target)?;
    let all = store.keys(owner, name, None);
    if !all.iter().any(|key| store.banks[key].log.is_some()) { return Ok(false); }
    let selection = selection.map(|source|
        driver.evaluate_parameter(source, command.context.current, shared)).transpose()?;
    let keys = store.keys(owner, name, selection);
    if keys.is_empty() {
        driver.fallback_command(command, shared)?;
        return Ok(true);
    }
    for key in keys {
        let bank = &store.banks[&key];
        // The recovered callback checks kind38 before fetching either argument.
        // CP932 LOGSTRING and unrelated banks have no remake side effect.
        if bank.log.is_none() || bank.message.encoding() != MessageEncoding::Unicode { continue; }
        let handle = bank.log.as_ref().unwrap().handle;
        let main = language_argument(host, command.parameters.get(2), driver, shared)?;
        let subtitle = language_argument(host, command.parameters.get(3), driver, shared)?;
        if subtitle.is_some() {
            bail!("LOGSTRING remake subtitle requires the original independent subtitle row surfaces; history is unchanged");
        }
        if handle == 0 { continue; }
        remake_backlog(host, handle, main.as_deref())?;
        // Every observer of the same unique Backlog sees the new bytes. Keep
        // row ids, scroll, drawing, palette selection and voice/path metadata.
        for bank in store.banks.values_mut() {
            if let Some(log) = &mut bank.log {
                if log.handle == handle { log.redraw = true; }
            }
        }
    }
    Ok(true)
}

fn language_argument(
    host: &LoggingHost<'_>, source: Option<&Vec<u8>>,
    driver: &mut FesScriptDriver, shared: &mut ExpressionState,
) -> Result<Option<String>> {
    let Some(source) = source else { return Ok(None) };
    let value = driver.evaluate_parameter(source, 0, shared)? as u32;
    // The original lookup uses the configured wide prefix, then CP932, not a
    // CSTL table index. Invalid (also -1) is a null language, with no fallback.
    let Some(name) = host.data_services.language.platform.declared_cstl_name(value)? else {
        return Ok(None);
    };
    let (bytes, _, errors) = encoding_rs::SHIFT_JIS.encode(&name);
    if errors || bytes.len() >= 64 || bytes.contains(&0) {
        bail!("LOGSTRING remake language name cannot fit its original CP932 name buffer");
    }
    Ok(Some(name))
}

fn remake_backlog(host: &mut LoggingHost<'_>, handle: u32, language: Option<&str>) -> Result<()> {
    let log = host.backlogs.get(&handle)
        .context("LOGSTRING remake requires its original live Backlog handle")?;
    let mut rows = vec![None; log.lines().len()];
    let mut names = BTreeMap::new();
    // Only resource identities/indices are copied for borrowing, never a second
    // Backlog or a new append history. Original cumulative indices are retained.
    let sources = log.sources().to_vec();
    let segments = log.segments().iter().map(|segment|
        (segment.source, segment.input)).collect::<Vec<_>>();
    let segment_start = log.segment_start();
    let references = log.lines().iter().map(|line| line.segment).collect::<Vec<_>>();
    let mut scenes = BTreeMap::new();
    let mut translated = BTreeMap::<usize, (Vec<u8>, Option<Vec<u8>>)>::new();
    let mut total = 0usize;
    for (index, absolute_segment) in references.into_iter().enumerate() {
        if absolute_segment == u32::MAX { continue; } // Existing spacer.
        let segment = absolute_segment.wrapping_sub(segment_start) as usize;
        let &(source, input) = segments.get(segment)
            .context("LOGSTRING remake row has no retained segment")?;
        if !translated.contains_key(&segment) {
            let translated_row = if let Some(language) = language {
                if !scenes.contains_key(&source) {
                    let source_name = sources.get(source as usize)
                        .context("LOGSTRING remake segment has no retained source")?;
                    scenes.insert(source, load_scene(host, source_name)?);
                }
                read_input(&scenes[&source], input as usize, language)?
            } else {
                // Null main clears the existing main row. No scene-language
                // index or current global language is substituted for it.
                (Vec::new(), None)
            };
            translated.insert(segment, translated_row);
        }
        let (text, name) = &translated[&segment];
        total = total.checked_add(text.len()).filter(|&n| n <= MAX_TEXT_BYTES)
            .context("LOGSTRING remade rows exceed 64 MiB")?;
        rows[index] = Some(text.clone());
        if let Some(name) = name { names.insert(segment, name.clone()); }
    }
    host.backlogs.get_mut(&handle).unwrap().replace_remade_rows(rows, names.into_iter().collect())?;
    Ok(())
}

fn load_scene(host: &LoggingHost<'_>, source: &[u8]) -> Result<Scene> {
    let (resolved, bytes) = host.backlog_resource(b"scene", source, false)?
        .context("LOGSTRING remake source CST is unavailable; retained history is unchanged")?;
    let document = CstFile::parse(&bytes).context("LOGSTRING remake source CST is malformed")?;
    let mut adjacent = resolved;
    if let Some(dot) = adjacent.rfind('.').filter(|&dot|
        adjacent[..dot].rfind(['/', '\\']).is_none_or(|slash| slash < dot)) {
        adjacent.truncate(dot);
    }
    adjacent.push_str(".cstl");
    let localization = host.backlog_read(&adjacent, false)?
        .map(|bytes| CstlTable::parse_prefix(&bytes).map(|(table, _)| table))
        .transpose().context("LOGSTRING remake adjacent CSTL is malformed")?;
    Ok(Scene { document, localization })
}

fn read_input(scene: &Scene, input: usize, language: &str) -> Result<(Vec<u8>, Option<Vec<u8>>)> {
    let Some(range) = scene.document.inputs.get(input) else { return Ok((Vec::new(), None)) };
    // A missing binding uses base CST; an existing binding with empty/missing
    // name, message or fragment stays empty. Do not use choice-menu fallback.
    let bound = scene.localization.as_ref().filter(|table|
        table.languages.iter().any(|name| name == language));
    let mut text = Vec::new();
    let mut name = None;
    let mut fragment = 0;
    for record in &scene.document.records[range.clone()] {
        let bytes = match record.kind {
            0x20 => {
                let bytes = if let Some(table) = bound {
                    table.segment(language, input, fragment).unwrap_or("").as_bytes().to_vec()
                } else { base_utf8(&record.text)? };
                fragment += 1;
                bytes
            }
            0x21 => {
                let bytes = if let Some(table) = bound {
                    table.message(language, input).map_or("", |message| &message.name).as_bytes().to_vec()
                } else { base_utf8(&record.text)? };
                check_record(&bytes)?;
                name = Some(bytes);
                continue;
            }
            _ => continue,
        };
        check_record(&bytes)?;
        // The actual body drops repeated leading literal \\r from each
        // message, then concatenates. It neither rewraps nor changes line ids.
        let mut bytes = bytes.as_slice();
        while bytes.starts_with(b"\\r") { bytes = &bytes[2..]; }
        if text.len().checked_add(bytes.len()).is_none_or(|n| n > MAX_TEXT_BYTES) {
            bail!("LOGSTRING remade input exceeds 64 MiB");
        }
        text.extend_from_slice(bytes);
    }
    Ok((text, name))
}

fn check_record(bytes: &[u8]) -> Result<()> {
    if bytes.len() >= 2048 || bytes.contains(&0) {
        bail!("LOGSTRING remake record exceeds its original UTF-8 buffer or contains a zero byte");
    }
    Ok(())
}

fn base_utf8(bytes: &[u8]) -> Result<Vec<u8>> {
    let bytes = &bytes[..bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len())];
    // The recovered scene getter explicitly converts base CST CP932 to UTF-8;
    // it does not infer an encoding from the selected localization language.
    let (text, _, errors) = encoding_rs::SHIFT_JIS.decode(bytes);
    if errors { bail!("LOGSTRING remake base CST contains invalid CP932"); }
    Ok(text.as_bytes().to_vec())
}

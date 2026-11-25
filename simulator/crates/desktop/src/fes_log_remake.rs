//! Resource producer for the Unicode LOGSTRING remake receiver.
//! Recovered from Yukikoi 63de0e14: 2d87e0 -> 2bed60 -> 28d390.
//! This is not the clear/append history-import protocol of ordinary1034.
use super::*;
use cs2_formats::CstlTable;
use cs2_script::CstDocument;
use crate::cst_context_native::repair_routed_scene_text;

struct Scene {
    document: CstDocument,
    localization: Option<CstlTable>,
    encoding: ScriptTextEncoding,
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
                read_input_with_recovery(&scenes[&source], input as usize, language, |record, codec, bad_at| {
                    host.data_services.note_text_decode(host.events, host.frame, format!(
                        "LOGSTRING remake source={:?} input={input} record={record} codec={codec:?} bad_at={bad_at}",
                        sources.get(source as usize).map(|name| String::from_utf8_lossy(name)),
                    ));
                })?
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
    let (resolved, resource) = host.cst_resource_with_source(b"scene", source)?
        .context("LOGSTRING remake source CST is unavailable; retained history is unchanged")?;
    let encoding = host.cst_scene_encoding_override(&resource).unwrap_or(host.text_encoding);
    let mut document = CstDocument::parse(&resource.bytes).context("LOGSTRING remake source CST is malformed")?;
    repair_routed_scene_text(host.resources, &resource, &mut document);
    let mut adjacent = resolved;
    if let Some(dot) = adjacent.rfind('.').filter(|&dot|
        adjacent[..dot].rfind(['/', '\\']).is_none_or(|slash| slash < dot)) {
        adjacent.truncate(dot);
    }
    adjacent.push_str(".cstl");
    let localization = host.backlog_read(&adjacent, false)?
        .map(|bytes| CstlTable::parse_prefix(&bytes).map(|(table, _)| table))
        .transpose().context("LOGSTRING remake adjacent CSTL is malformed")?;
    Ok(Scene { document, localization, encoding })
}

fn read_input_with_recovery(
    scene: &Scene, input: usize, language: &str,
    mut warn: impl FnMut(usize, ScriptTextEncoding, usize),
) -> Result<(Vec<u8>, Option<Vec<u8>>)> {
    let Some(range) = scene.document.inputs.get(input) else { return Ok((Vec::new(), None)) };
    // A missing binding uses base CST; an existing binding with empty/missing
    // name, message or fragment stays empty. Do not use choice-menu fallback.
    let bound = scene.localization.as_ref().filter(|table|
        table.languages.iter().any(|name| name == language));
    let mut text = Vec::new();
    let mut name = None;
    let mut fragment = 0;
    for (ordinal, record) in scene.document.records[range.clone()].iter().enumerate() {
        let mut base_utf8 = || {
            let (text, bad_at) = scene.encoding.decode_message(crate::trim_nul(&record.text));
            if let Some(bad_at) = bad_at { warn(ordinal, scene.encoding, bad_at); }
            text.into_bytes()
        };
        let bytes = match record.kind {
            0x20 => {
                let bytes = if let Some(table) = bound {
                    table.segment(language, input, fragment).unwrap_or("").as_bytes().to_vec()
                } else { base_utf8() };
                fragment += 1;
                bytes
            }
            0x21 => {
                let bytes = if let Some(table) = bound {
                    table.message(language, input).map_or("", |message| &message.name).as_bytes().to_vec()
                } else { base_utf8() };
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

#[cfg(test)]
fn read_input(scene: &Scene, input: usize, language: &str) -> Result<(Vec<u8>, Option<Vec<u8>>)> {
    read_input_with_recovery(scene, input, language, |_, _, _| {})
}

#[test]
fn log_remake_keeps_declared_base_codec_and_bound_utf8_overrides() {
    use cs2_formats::CstlMessage;
    use cs2_script::CstRecord;

    let mut scene = Scene {
        document: CstDocument {
            records: vec![
                CstRecord { kind: 0x21, text: b"\xc2\xa9".to_vec() },
                CstRecord { kind: 0x20, text: b"\\r\xc2\xa9".to_vec() },
                CstRecord { kind: 0x20, text: b"\\r!".to_vec() },
            ],
            inputs: std::iter::once(0..3).collect(),
        },
        localization: Some(CstlTable {
            languages: vec!["bound".into()],
            messages: vec![vec![CstlMessage { name: String::new(), content: "\u{65e5}\u{1f}".into() }]],
        }),
        encoding: ScriptTextEncoding::Cp932,
    };
    for (codec, expected) in [
        (ScriptTextEncoding::Cp932, "\u{ff82}\u{ff69}"),
        (ScriptTextEncoding::Gbk, "\u{6f0f}"),
        (ScriptTextEncoding::Utf8, "\u{a9}"),
    ] {
        scene.encoding = codec;
        assert_eq!(read_input(&scene, 0, "unbound").unwrap(),
            (format!("{expected}!").into_bytes(), Some(expected.as_bytes().to_vec())));
        assert_eq!(read_input(&scene, 0, "bound").unwrap(),
            ("\u{65e5}".as_bytes().to_vec(), Some(Vec::new())));
    }
    scene.document.records[1].text = b"\x81".to_vec();
    assert_eq!(read_input(&scene, 0, "unbound").unwrap().0, "\u{fffd}!".as_bytes());
}

#[test]
#[ignore = "requires the installed Grisaia Eden game and an isolated profile"]
fn grisaia_eden_mixed_archive_text_backlog_and_cst_reopen() -> Result<()> {
    use crate::particle_plugin_digest::sha256;
    use cs2_runtime::object_graph::{decode_component, encode_component};
    use cs2_script::{Host, NativeReturn, Vm};

    let root = std::env::var_os("CS2_EDEN_TEXT_TEST_ROOT").context("installed game root required")?;
    let profile = std::path::PathBuf::from(std::env::var_os("CS2_EDEN_TEXT_TEST_PROFILE")
        .context("isolated profile required")?);
    std::fs::create_dir_all(&profile).context("create isolated CST checkpoint directory")?;
    let mut player = crate::HeadlessPlayer::open_with_save_root(&root, &profile)?;
    let mut vm = Vm::new();
    vm.bind_external(vec![0; 4096])?;
    let scene_name = 0x8000_0100;
    let output = 0x8000_0400;
    let args = |words: &[u32]| words.iter().flat_map(|word| word.to_le_bytes()).collect::<Vec<_>>();
    let body_hash = "69d0b63f3c4b50667d993303eb8e5772e087455965f97a81f6fea41c8d34ac72";
    let speaker_hash = "3398206e602e12d0af463cd54c3b90086cc6762e60a56aadba859012c37f78c6";
    let handle = player.with_host(|host| {
        let NativeReturn::Value(handle) = host.call(323, &[], &mut vm)? else {
            bail!("no CST context handle");
        };
        let japanese = load_scene(host, b"bla018G_A.cst")?;
        assert_eq!(japanese.encoding, ScriptTextEncoding::Cp932);
        let (row, _) = read_input(&japanese, 0, "unbound")?;
        assert_eq!(sha256(&row), "34bb1de6f71e0f4b6cb596b22768d8d5ff95cbf5a19714b69d74d7f95d694516");

        vm.write(scene_name, b"aft_002.cst\0")?;
        assert_eq!(host.call(325, &args(&[handle, scene_name]), &mut vm)?, NativeReturn::Value(1));
        host.read_cst_record(327, &args(&[handle, output, 0, 48]), &mut vm)?;
        host.call(276, &args(&[1000, output]), &mut vm)?;
        let raw = vm.read_string(output)?;
        let origin = host.data_services.cst_text_origins.shared(1000, &raw);
        assert_eq!(origin, Some(ScriptTextEncoding::Gbk));
        let decoded = super::decode_sourced_text(b"$str1000", &raw, ScriptTextEncoding::Cp932,
            ScriptTextEncoding::Cp932, false, |_| (&raw, origin))?.context("missing FES text")?;
        assert_eq!(sha256(decoded.as_bytes()), body_hash);

        let chinese = load_scene(host, b"aft_002.cst")?;
        let (row, speaker) = read_input(&chinese, 0, "unbound")?;
        assert_eq!(sha256(&row), body_hash);
        assert_eq!(sha256(&speaker.context("missing history speaker")?), speaker_hash);
        Ok(handle)
    })?;

    let checkpoint = profile.join("cst-checkpoint.bin");
    std::fs::write(&checkpoint, encode_component(&player.cst_context_snapshots()?)?)
        .context("write isolated CST checkpoint")?;
    drop(player);
    let mut reopened = crate::HeadlessPlayer::open_with_save_root(&root, &profile)?;
    let snapshot = decode_component(&std::fs::read(checkpoint).context("read isolated CST checkpoint")?)?;
    reopened.commit_cst_context_restore(crate::HeadlessPlayer::prepare_cst_context_restore(&snapshot)?);
    reopened.with_host(|host| {
        host.read_cst_record(1037, &args(&[handle, output, 0, 47]), &mut vm)?;
        assert_eq!(sha256(&vm.read_string(output)?), speaker_hash);
        host.read_cst_record(1037, &args(&[handle, output, 0, 48]), &mut vm)?;
        assert_eq!(sha256(&vm.read_string(output)?), body_hash);
        Ok(())
    })
}

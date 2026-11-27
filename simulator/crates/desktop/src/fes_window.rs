//! FES window commands and the result of a platform text-entry request.
use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use cs2_runtime::resource_router::ResourceMode;
use cs2_script::ExpressionState;

use super::fes_driver::{FesDriverEffect, FesScriptDriver};
use super::fes_tasks::FesTaskView;
use super::{HeadlessPlayer, LoggingHost, ScriptTextEncoding, trim_nul};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostCursorImage {
    pub width: u16,
    pub height: u16,
    pub hotspot: [u16; 2],
    pub rgba: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct HostTextEntry {
    pub id: u64,
    pub title: String,
    pub initial: String,
    pub limit: usize,
    pub allow_cancel: bool,
}

struct PendingEntry {
    request: HostTextEntry,
    output: usize,
    text: u32,
    encoding: ScriptTextEncoding,
    delivered: bool,
    result: Option<Option<String>>,
}

#[derive(Default)]
pub(super) struct FesWindowState {
    next_entry: u64,
    entries: BTreeMap<(u32, usize), PendingEntry>,
    cursors: BTreeMap<(String, Option<u16>, Option<u16>), Option<HostCursorImage>>,
    cursor: Option<Option<HostCursorImage>>,
}

fn set_result(shared: &mut ExpressionState, destination: usize, accepted: bool) {
    if let Some(value) = shared.shared.get_mut(destination) {
        *value = i32::from(accepted);
    }
}

fn decode_string(
    host: &mut LoggingHost<'_>, shared: &ExpressionState, id: u32,
) -> (String, ScriptTextEncoding) {
    let bytes = shared.strings.get(&id).map_or(&[][..], Vec::as_slice);
    let raw = trim_nul(bytes);
    let origins = &host.data_services.cst_text_origins;
    let encoding = origins.shared(id, bytes).unwrap_or_else(|| {
        if !raw.is_ascii() && std::str::from_utf8(raw).is_ok() {
            ScriptTextEncoding::Utf8
        } else {
            host.text_encoding
        }
    });
    if let Some(display) = origins.shared_display(id, bytes) {
        return (trim_display(display).to_owned(), encoding);
    }
    let (display, bad_at) = encoding.decode_message(raw);
    if let Some(offset) = bad_at {
        host.data_services.note_text_decode(host.events, host.frame,
            format!("input_text string={id} encoding={encoding:?} offset={offset}"));
    }
    (display, encoding)
}

fn trim_display(display: &str) -> &str {
    display.split('\0').next().unwrap_or_default()
}

fn encode_string(input: &str, encoding: ScriptTextEncoding) -> (Vec<u8>, ScriptTextEncoding) {
    let codec = match encoding {
        ScriptTextEncoding::Cp932 => encoding_rs::SHIFT_JIS,
        ScriptTextEncoding::Gbk => encoding_rs::GBK,
        ScriptTextEncoding::Utf8 => return (input.as_bytes().to_vec(), encoding),
    };
    let (bytes, _, had_errors) = codec.encode(input);
    if !had_errors && encoding.decode_strict(&bytes).as_deref() == Some(input) {
        (bytes.into_owned(), encoding)
    } else {
        (input.as_bytes().to_vec(), ScriptTextEncoding::Utf8)
    }
}

pub(super) fn apply_effect(
    host: &mut LoggingHost<'_>,
    view: &FesTaskView,
    driver: &mut FesScriptDriver,
    effect: &FesDriverEffect,
    shared: &mut ExpressionState,
) -> Result<Option<bool>> {
    let FesDriverEffect::Command(command) = effect else {
        return Ok(None);
    };
    let Some((verb, args)) = command.parameters.split_first() else {
        return Ok(None);
    };
    let mut number =
        |value: &[u8]| driver.evaluate_parameter(value, command.context.current, shared);
    match verb.as_slice() {
        b"opensite" | b"opensiteenc" => return Ok(Some(
            host.fes_open_site(view.id, command.offset, args, verb == b"opensiteenc"),
        )),
        b"wndmenu" => {
            if let Some(id) = args.first() {
                host.post_fes_window_menu(number(id)? as u32)?;
            }
        }
        b"xchgcursor" => {
            let name = args
                .first()
                .map(|name| host.text_encoding.decode(trim_nul(name)))
                .unwrap_or_else(|| "default".to_owned());
            let x = args
                .get(1)
                .map(|value| number(value))
                .transpose()?
                .map(|value| value as u16);
            let y = args
                .get(2)
                .map(|value| number(value))
                .transpose()?
                .map(|value| value as u16);
            let key = (name, x, y);
            let state = &mut host.data_services.window_control.fes;
            if !state.cursors.contains_key(&key) {
                let cursor = load_cursor(host.resources, &key.0, x, y)?;
                if state.cursors.len() >= 64 {
                    state.cursors.clear();
                }
                state.cursors.insert(key.clone(), cursor);
            }
            state.cursor = state.cursors.get(&key).cloned();
        }
        b"input_text" => {
            let [output, title, text, optional @ ..] = args else {
                return Ok(Some(true));
            };
            let key = (view.id, command.offset);
            if let Some(pending) = host.data_services.window_control.fes.entries.get(&key) {
                if pending.result.is_none() {
                    return Ok(Some(false));
                }
                let pending = host
                    .data_services
                    .window_control
                    .fes
                    .entries
                    .remove(&key)
                    .context("completed FES text entry disappeared")?;
                let result = pending.result.context("completed FES text entry lacks its result")?;
                set_result(shared, pending.output, result.is_some());
                if let Some(value) = result.filter(|value| *value != pending.request.initial) {
                    let (bytes, encoding) = encode_string(&value, pending.encoding);
                    host.data_services.cst_text_origins.set_shared_encoding(pending.text, &bytes, encoding);
                    shared.strings.insert(pending.text, bytes);
                }
                return Ok(Some(true));
            }
            let output = number(output)? as usize;
            let title = number(title)? as u32;
            let text = number(text)? as u32;
            let limit = optional
                .first()
                .map(|value| number(value))
                .transpose()?
                .unwrap_or(128);
            let allow_cancel = optional
                .get(1)
                .map(|value| number(value))
                .transpose()?
                .unwrap_or(1)
                != 0;
            if !host.data_services.window_control.enabled {
                set_result(shared, output, false);
                return Ok(Some(true));
            }
            let (title, _) = decode_string(host, shared, title);
            let (initial, encoding) = decode_string(host, shared, text);
            let state = &mut host.data_services.window_control.fes;
            state.next_entry = state
                .next_entry
                .checked_add(1)
                .context("text-entry identities exhausted")?;
            let limit = if limit == 0 {
                1023
            } else {
                limit.clamp(1, 1023) as usize
            };
            state.entries.insert(
                key,
                PendingEntry {
                    request: HostTextEntry {
                        id: state.next_entry,
                        title,
                        initial,
                        limit,
                        allow_cancel,
                    },
                    output,
                    text,
                    encoding,
                    delivered: false,
                    result: None,
                },
            );
            return Ok(Some(false));
        }
        _ => return Ok(None),
    }
    Ok(Some(true))
}

pub(super) fn release_owner(host: &mut LoggingHost<'_>, owner: u32) {
    host.data_services.system_open.release_fes_owner(owner);
    host.data_services
        .window_control
        .fes
        .entries
        .retain(|(id, _), _| *id != owner);
}

impl HeadlessPlayer {
    pub fn take_host_cursor_image(&mut self) -> Option<Option<HostCursorImage>> {
        self.data_services.window_control.fes.cursor.take()
    }

    pub fn take_host_text_entry(&mut self) -> Option<HostTextEntry> {
        let entries = &mut self.data_services.window_control.fes.entries;
        if entries
            .values()
            .any(|entry| entry.delivered && entry.result.is_none())
        {
            return None;
        }
        let pending = entries
            .values_mut()
            .filter(|entry| !entry.delivered)
            .min_by_key(|entry| entry.request.id)?;
        pending.delivered = true;
        let mut request = pending.request.clone();
        request.title = String::from_utf16_lossy(
            &self.resolve_language_text(&request.title.encode_utf16().collect::<Vec<_>>()),
        );
        Some(request)
    }

    /// Returns false when the host answers a request that has already finished or lost its owner.
    pub fn complete_host_text_entry(&mut self, id: u64, value: Option<String>) -> bool {
        let Some(pending) = self
            .data_services
            .window_control
            .fes
            .entries
            .values_mut()
            .find(|entry| entry.request.id == id && entry.delivered && entry.result.is_none())
        else {
            return false;
        };
        let normalized = value.map(|value| {
            let limit = pending.request.limit;
            let mut remaining = limit;
            value
                .chars()
                .take_while(|ch| {
                    if *ch == '\0' || ch.len_utf16() > remaining {
                        return false;
                    }
                    remaining -= ch.len_utf16();
                    true
                })
                .collect()
        });
        pending.result = Some(normalized);
        true
    }

    #[cfg(any(windows, test))]
    pub(super) fn finish_host_text_entry(&mut self, id: u64, result: Result<Option<String>>) {
        let value = result.unwrap_or_else(|error| {
            self.events.push(crate::LogicEvent {
                frame: self.state.clock.frame,
                kind: "text-entry-error".into(),
                detail: format!("request={id}: {error:#}"),
            });
            None
        });
        self.complete_host_text_entry(id, value);
    }
}

fn load_cursor(
    resources: &cs2_runtime::ResourceIndex,
    name: &str,
    x: Option<u16>,
    y: Option<u16>,
) -> Result<Option<HostCursorImage>> {
    if !name
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        return Ok(None);
    }
    let stem = name.strip_suffix(".cur").unwrap_or(name);
    let alternatives = match stem {
        "default" => ["cursor", "default"],
        "btndown" => ["btndown", "cur_over"],
        other => [other, other],
    };
    for stem in alternatives {
        for path in [
            format!("cursor/{stem}.cur"),
            format!("system/cursor/{stem}.cur"),
        ] {
            let Some(candidate) =
                resources.resolve_mode_candidate(&path, ResourceMode::LooseFirst)?
            else {
                continue;
            };
            let bytes = resources.read_candidate_bounded(&candidate, 4 * 1024 * 1024)?;
            if let Some(bytes) = bytes
                && let Ok(cursor) = decode_cursor(&bytes, x, y)
            {
                return Ok(Some(cursor));
            }
        }
    }
    Ok(None)
}

fn decode_cursor(bytes: &[u8], x: Option<u16>, y: Option<u16>) -> Result<HostCursorImage> {
    let word = |at| {
        Some(u16::from_le_bytes(*bytes.get(at..)?.first_chunk::<2>()?))
    };
    let dword = |at| {
        Some(u32::from_le_bytes(*bytes.get(at..)?.first_chunk::<4>()?))
    };
    if word(0) != Some(0) || word(2) != Some(2) || word(4).unwrap_or(0) == 0 {
        bail!("invalid CUR directory");
    }
    let offset = dword(18).context("CUR image offset is absent")? as usize;
    let length = dword(14).context("CUR image length is absent")? as usize;
    let data = bytes
        .get(offset..offset.checked_add(length).context("CUR image overflows")?)
        .context("CUR image is truncated")?;
    let (width, height, rgba) = if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        let dimension = |at| {
            data.get(at..).and_then(<[u8]>::first_chunk::<4>)
                .map_or(0, |v| u32::from_be_bytes(*v))
        };
        if !(1..=256).contains(&dimension(16)) || !(1..=256).contains(&dimension(20)) {
            bail!("invalid CUR PNG dimensions");
        }
        let image =
            image::load_from_memory_with_format(data, image::ImageFormat::Png)?.into_rgba8();
        (
            image.width() as usize,
            image.height() as usize,
            image.into_raw(),
        )
    } else {
        let header = dword(offset).unwrap_or(0) as usize;
        let width = dword(offset + 4).unwrap_or(0) as usize;
        let height = dword(offset + 8).unwrap_or(0) as usize / 2;
        if header < 40
            || word(offset + 14) != Some(32)
            || !(1..=256).contains(&width)
            || !(1..=256).contains(&height)
            || dword(offset + 16) != Some(0)
        {
            bail!("unsupported CUR bitmap header");
        }
        let color = data
            .get(header..header + width * height * 4)
            .context("CUR pixels are truncated")?;
        let mask = &data[header + color.len()..];
        let mask_stride = width.div_ceil(32) * 4;
        let uses_alpha = color.chunks_exact(4).any(|pixel| pixel[3] != 0);
        let mut rgba = Vec::with_capacity(color.len());
        for row in (0..height).rev() {
            for col in 0..width {
                let pixel = &color[(row * width + col) * 4..][..4];
                let masked = mask
                    .get(row * mask_stride + col / 8)
                    .is_some_and(|byte| byte & (0x80 >> (col % 8)) != 0);
                let alpha = if masked {
                    0
                } else if uses_alpha {
                    pixel[3]
                } else {
                    255
                };
                rgba.extend_from_slice(&[pixel[2], pixel[1], pixel[0], alpha]);
            }
        }
        (width, height, rgba)
    };
    if !(1..=256).contains(&width) || !(1..=256).contains(&height) {
        bail!("invalid CUR dimensions");
    }
    let hotspot = [
        x.or_else(|| word(10)).unwrap_or(0).min(width as u16 - 1),
        y.or_else(|| word(12)).unwrap_or(0).min(height as u16 - 1),
    ];
    Ok(HostCursorImage {
        width: width as u16,
        height: height as u16,
        hotspot,
        rgba,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HostWindowRequest;
    use crate::fes_driver::FesDriverCommand;
    use cs2_script::FesDocument;

    fn command(
        driver: &FesScriptDriver,
        shared: &ExpressionState,
        args: &[&[u8]],
        offset: usize,
    ) -> FesDriverEffect {
        FesDriverEffect::Command(FesDriverCommand {
            parameters: args.iter().map(|arg| arg.to_vec()).collect(),
            source: args.join(&b' '),
            offset,
            context: driver.interpreter.context(shared),
            object_event: None,
        })
    }

    fn setup() -> Result<(
        HeadlessPlayer,
        FesTaskView,
        FesScriptDriver,
        ExpressionState,
    )> {
        let path = std::env::temp_dir().join(format!(
            "cs2-fes-window-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos()
        ));
        std::fs::create_dir(&path)?;
        let mut player = HeadlessPlayer::open_with_save_root(&path, path.join("save"))?;
        std::fs::remove_dir_all(&path)?;
        player.enable_host_window_requests();
        let view = FesTaskView {
            id: 1,
            parent: 0,
            now: 0,
            paused: 0,
            context: Vec::new(),
            list: None,
        };
        let driver = FesScriptDriver::new(&FesDocument {
            sections: Vec::new(),
        })?;
        Ok((player, view, driver, ExpressionState::new(1000)))
    }

    #[test]
    fn fes_open_site_completes_after_host_rejection() -> Result<()> {
        let (mut player, view, mut driver, mut shared) = setup()?;
        player.enable_host_system_open_queue();
        let open = command(&driver, &shared, &[b"opensite", b"https", b"example.invalid/manual"], 72);
        let mut apply = |player: &mut HeadlessPlayer| {
            player.with_host(|host| apply_effect(host, &view, &mut driver, &open, &mut shared))
        };
        assert_eq!(apply(&mut player)?, Some(false));
        let request = player.take_host_system_open_request().unwrap();
        assert_eq!(request.request.task_id, view.id);
        assert_eq!(request.request.target, b"https://example.invalid/manual");
        assert_eq!(apply(&mut player)?, Some(false));
        assert!(player.take_host_system_open_request().is_none());
        player.complete_host_system_open(request.id, Err("no browser".into()))?;
        assert_eq!(apply(&mut player)?, Some(true));
        assert!(player.take_host_system_open_request().is_none());
        assert!(player.events().iter().any(|event| {
            event.kind == "system-open-error" && event.detail == "no browser"
        }));
        Ok(())
    }

    #[test]
    fn fes_open_site_encoded_preserves_query_fields_and_resumes_after_rejection() -> Result<()> {
        let (mut player, view, mut driver, mut shared) = setup()?;
        player.enable_host_system_open_queue();
        let (line, _, _) = encoding_rs::SHIFT_JIS.encode(concat!(
            "opensiteenc https example.invalid/intent? original_referer=http: ",
            "example.invalid/&text=\u{4f53}\u{9a13}\u{7248} 42&url=https: ",
            "example.invalid/game/&hashtags=\u{65e5}\u{672c}\u{8a9e}",
        ));
        let parameters = driver.interpreter.program.command_parameters(&line, &shared.strings)?;
        let arguments: Vec<&[u8]> = parameters.iter().map(Vec::as_slice).collect();
        let open = command(&driver, &shared, &arguments, 73);
        let mut apply = |player: &mut HeadlessPlayer| {
            player.with_host(|host| apply_effect(host, &view, &mut driver, &open, &mut shared))
        };
        assert_eq!(apply(&mut player)?, Some(false));
        let request = player.take_host_system_open_request().unwrap();
        assert_eq!(request.request.target, concat!(
            "https://example.invalid/intent?original_referer=http%3A%2F%2Fexample.invalid%2F",
            "&text=%E4%BD%93%E9%A8%93%E7%89%88%2042&url=https%3A%2F%2Fexample.invalid%2Fgame%2F",
            "&hashtags=%E6%97%A5%E6%9C%AC%E8%AA%9E",
        ).as_bytes());
        player.complete_host_system_open(request.id, Err("no browser".into()))?;
        assert_eq!(apply(&mut player)?, Some(true));
        assert!(player.take_host_system_open_request().is_none());
        assert!(player.events().iter().any(|event| {
            event.kind == "system-open-error" && event.detail == "no browser"
        }));
        Ok(())
    }

    #[test]
    fn fes_window_text_entry_retries_then_writes_only_accepted_text() -> Result<()> {
        let (mut player, view, mut driver, mut shared) = setup()?;
        shared
            .strings
            .insert(481, "\u{7de8}\u{96c6}".as_bytes().to_vec());
        let original = b"\x82\xa0\x82";
        shared.strings.insert(480, original.to_vec());
        player.data_services.cst_text_origins.set_shared_encoding(480, original, ScriptTextEncoding::Cp932);
        let command = command(
            &driver,
            &shared,
            &[b"input_text", b"850", b"481", b"480", b"3", b"1"],
            1267,
        );
        let apply = |player: &mut HeadlessPlayer,
                     driver: &mut FesScriptDriver,
                     shared: &mut ExpressionState| {
            player.with_host(|host| apply_effect(host, &view, driver, &command, shared))
        };
        assert_eq!(apply(&mut player, &mut driver, &mut shared)?, Some(false));
        let first = player.take_host_text_entry().unwrap();
        assert_eq!(first.initial, "\u{3042}\u{fffd}");
        assert!(player.events().iter().any(|event| event.kind == "text-decode-recovered"
            && event.detail.contains("input_text string=480 encoding=Cp932 offset=2")));
        assert_eq!(first.limit, 3);
        assert!(first.allow_cancel);
        assert_eq!(apply(&mut player, &mut driver, &mut shared)?, Some(false));
        assert!(player.take_host_text_entry().is_none());
        assert!(player.complete_host_text_entry(first.id, Some("a\u{1f30d}b".into())));
        assert!(!player.complete_host_text_entry(first.id, None));
        assert_eq!(apply(&mut player, &mut driver, &mut shared)?, Some(true));
        assert_eq!(shared.shared[850], 1);
        assert_eq!(shared.strings[&480], "a\u{1f30d}".as_bytes());
        assert_eq!(player.data_services.cst_text_origins.shared(480, &shared.strings[&480]), Some(ScriptTextEncoding::Utf8));
        assert_eq!(apply(&mut player, &mut driver, &mut shared)?, Some(false));
        let second = player.take_host_text_entry().unwrap();
        assert_ne!(first.id, second.id);
        assert_eq!(second.initial, "a\u{1f30d}");
        assert!(!player.complete_host_text_entry(first.id, None));
        player.finish_host_text_entry(second.id, Err(anyhow::anyhow!("unable to open native dialog")));
        assert_eq!(apply(&mut player, &mut driver, &mut shared)?, Some(true));
        assert_eq!(shared.shared[850], 0);
        assert_eq!(shared.strings[&480], "a\u{1f30d}".as_bytes());
        assert!(player.events().iter().any(|event| event.kind == "text-entry-error"
            && event.detail.contains("unable to open native dialog")));
        Ok(())
    }

    #[test]
    fn fes_window_menu_repeated_fullscreen_is_idempotent() -> Result<()> {
        let (mut player, view, mut driver, mut shared) = setup()?;
        player.set_host_windowed_mode(1);
        let fullscreen = command(&driver, &shared, &[b"wndmenu", b"10"], 1004);
        let window = command(&driver, &shared, &[b"wndmenu", b"11"], 1006);
        for command in [&fullscreen, &fullscreen, &window] {
            player
                .with_host(|host| apply_effect(host, &view, &mut driver, command, &mut shared))?;
        }
        assert!(!player.advance_posted_window_commands(4096)?);
        assert_eq!(
            player.take_host_window_request(),
            Some(HostWindowRequest::ToggleFullscreen)
        );
        player.complete_host_window_request(Some(0))?;
        assert!(!player.advance_posted_window_commands(4096)?);
        assert_eq!(
            player.take_host_window_request(),
            Some(HostWindowRequest::ToggleFullscreen)
        );
        player.complete_host_window_request(Some(1))?;
        assert!(player.advance_posted_window_commands(4096)?);
        assert!(player.take_host_window_request().is_none());
        Ok(())
    }

    #[test]
    fn fes_window_cursor_decodes_neko_resource_pair_when_present() -> Result<()> {
        let games = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../games");
        let Ok(entries) = std::fs::read_dir(games) else {
            return Ok(());
        };
        let Some(cursor) = entries
            .filter_map(std::result::Result::ok)
            .map(|entry| entry.path().join("nekopara_vol3/cursor"))
            .find(|path| path.join("cursor.cur").is_file())
        else {
            return Ok(());
        };
        let regular = decode_cursor(&std::fs::read(cursor.join("cursor.cur"))?, None, None)?;
        let pressed = decode_cursor(
            &std::fs::read(cursor.join("cur_over.cur"))?,
            Some(1),
            Some(4),
        )?;
        assert_eq!([regular.width, regular.height], [32, 32]);
        assert_eq!(regular.hotspot, [15, 15]);
        assert_eq!(pressed.hotspot, [1, 4]);
        assert_ne!(regular.rgba, pressed.rgba);
        Ok(())
    }
}

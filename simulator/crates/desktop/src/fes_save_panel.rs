//! SAVEPANEL's CSS region and dedicated IMAGE-backed cell consumer.
//! Shared service hooks pass the one store explicitly; no shadow scene/map.
use anyhow::{Context, Result};
use cs2_formats::CssFile;
use cs2_script::ExpressionState;

use crate::{LoggingHost, ScriptTextEncoding, fes_driver::{FesDriverCommand, FesScriptDriver}};

// The ordinary save script can write its CP932 "[choice]" literal in place of
// dialogue. A localized CST does not change the compiled script literal.
const KCS_CHOICE_PREVIEW: &[u8] = b"[\x91I\x91\xf0\x8e\x88]";
const STANDARD_TEXT_RECORD: u32 = 0x200;

#[path = "fes_save_panel_time.rs"]
mod file_date;
#[path = "fes_save_panel_bank.rs"]
mod bank;
#[path = "fes_save_panel_cell.rs"]
mod cell;
#[path = "fes_save_panel_pixels.rs"]
mod pixels;
#[path = "fes_save_panel_state.rs"]
mod state;
#[path = "fes_save_panel_host.rs"]
mod host;
pub(crate) use bank::FesSavePanelStore;
pub(crate) use state::PreparedSavePanels;
pub(crate) use host::{apply_effect, update_owner, release_bank, release_owner};

/// Retain the prepared request with the original pending command. In
/// particular, applying cell sources after SetSave must not evaluate it again.
#[derive(Clone, Copy, Debug, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(crate) enum PanelDataRequest {
    Skipped,
    SetSave { slot: i32, record: u32, start: i32 },
    GetFlag { index: i32, destination: i32 },
    /// The source service has completed; retry only cell application, without
    /// rereading a potentially changed file or repeating argument expressions.
    ApplyCellSources,
}

impl PanelDataRequest {
    /// Call after declaration-kind/bank resolution, before checking whether
    /// the live bank exists. An absent bank still evaluates these arguments,
    /// then the caller discards the prepared request without writing output.
    /// `bank` is the already-evaluated explicit bank, including @ event identity;
    /// None/-1 means the original no-bank early return, before any arguments.
    /// None as the result means an unsupported verb, with no argument effects.
    pub(crate) fn prepare(
        command: &FesDriverCommand, bank: Option<i32>,
        driver: &mut FesScriptDriver, shared: &mut ExpressionState,
    ) -> Result<Option<Self>> {
        let verb = command.parameters.get(1).map(Vec::as_slice);
        let count = match verb {
            Some(b"setsave") => 3,
            Some(b"getflag") => 2,
            _ => return Ok(None),
        };
        if bank.is_none_or(|bank| bank == -1) {
            return Ok(Some(Self::Skipped));
        }
        let mut values = [0; 3];
        for (index, value) in values[..count].iter_mut().enumerate() {
            let Some(source) = command.parameters.get(index + 2) else {
                // Do not undo earlier expressions or inspect a suffix.
                return Ok(Some(Self::Skipped));
            };
            if !source.is_empty() {
                *value = driver.evaluate_parameter(source, command.context.current, shared)?;
            }
        }
        Ok(Some(if count == 3 {
            Self::SetSave { slot: values[0], record: values[1] as u32, start: values[2] }
        } else {
            Self::GetFlag { index: values[0], destination: values[1] }
        }))
    }
}

#[derive(Clone, Debug, Default, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(crate) struct SavePanelData {
    save_name: Vec<u8>,
    record_id: u32,
    // No image, history, VM container handle, or second global slot map.
    // The CSS object is the last successfully loaded panel-owned source.
    #[serde(with = "state::css")]
    data: Option<CssFile>,
    flags: FlagRegion,
    date: [u32; 6],
}

#[derive(Clone, Debug, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct FlagRegion(Vec<i32>);

impl Default for FlagRegion {
    fn default() -> Self { Self(vec![0; 1024]) }
}

/// SetSave always needs the panel's cell-source consumer, even after a missing
/// file: old CSS/flags remain available while DATE_* becomes zero. The host
/// must not turn this outcome into command completion without that consumer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PanelDataOutcome {
    Complete,
    ApplyCellSources,
}

impl SavePanelData {
    pub(crate) fn save_name(&self) -> &[u8] { &self.save_name }
    pub(crate) fn record_id(&self) -> u32 { self.record_id }
    pub(crate) fn date(&self) -> [u32; 6] { self.date }
    pub(crate) fn flag(&self, index: i32) -> i32 {
        self.flags.0.get(index as usize).copied().unwrap_or(0)
    }

    /// Cell consumers share this exact last-successful CSS object, including
    /// its native checked record scan. A missing file never invents a source.
    pub(crate) fn record(&self, id: u32) -> Result<Option<&[u8]>> {
        self.data.as_ref().map(|data| data.try_record(id)).transpose()
            .context("SAVEPANEL CSS source record is invalid").map(Option::flatten)
    }

    fn text_encoding(&self, host: &LoggingHost<'_>, record: u32, text: &[u8]) -> Result<ScriptTextEncoding> {
        save_record_text_encoding(host, self.data.as_ref(), record, text)
    }

    pub(crate) fn apply(
        &mut self, request: &mut PanelDataRequest, host: &mut LoggingHost<'_>,
        shared: &mut ExpressionState,
    ) -> Result<PanelDataOutcome> {
        match *request {
            PanelDataRequest::Skipped => Ok(PanelDataOutcome::Complete),
            PanelDataRequest::ApplyCellSources => Ok(PanelDataOutcome::ApplyCellSources),
            PanelDataRequest::GetFlag { index, destination } => {
                if let Some(output) = shared.shared.get_mut(destination as usize) {
                    *output = self.flag(index);
                }
                *request = PanelDataRequest::Skipped;
                Ok(PanelDataOutcome::Complete)
            }
            PanelDataRequest::SetSave { slot, record, start } => {
                // Filename/record change before attempting the real service.
                // Keep signed formatting identical to FES datasave/saveexist.
                let name = format!("save{slot:04}.dat");
                self.save_name = name.as_bytes().to_vec();
                self.record_id = record;
                self.date = [0; 6];
                let result = host.read_fes_css_save(&name).and_then(|loaded| {
                    let Some(data) = loaded else { return Ok(()) };
                    let source = data.try_record(record)
                        .context("SAVEPANEL numeric CSS record is invalid")?;
                    if let (Some(source), Some(target)) =
                        (source, self.flags.0.get_mut(start as usize..))
                    {
                        // Preserve unwritten words, truncate at the region
                        // end, and ignore a source's incomplete final DWORD.
                        for (target, bytes) in target.iter_mut().zip(source.as_chunks::<4>().0) {
                            *target = i32::from_le_bytes(*bytes);
                        }
                    }
                    self.data = Some(data);
                    self.date = file_date::modified_date(host.save_root, &name)?.unwrap_or([0; 6]);
                    Ok(())
                });
                if let Err(error) = result {
                    host.events.push(crate::LogicEvent { frame: host.frame, kind: "save-panel-source-error".into(),
                        detail: format!("file={name:?} record={record:#x}: {error:#}") });
                }
                *request = PanelDataRequest::ApplyCellSources;
                Ok(PanelDataOutcome::ApplyCellSources)
            }
        }
    }
}

pub(super) fn save_record_text_encoding(
    host: &LoggingHost<'_>, file: Option<&CssFile>, record: u32, text: &[u8],
) -> Result<ScriptTextEncoding> {
    if std::str::from_utf8(text).is_ok() {
        return Ok(ScriptTextEncoding::Utf8);
    }
    let encoding = host.text_encoding;
    let Some(file) = file else { return Ok(encoding) };
    // Script-defined preview records can copy scene bytes or hold legacy
    // strings already expressed in the host encoding.
    if record == u32::MAX || (record != STANDARD_TEXT_RECORD && (encoding != ScriptTextEncoding::Cp932
        || encoding_rs::SHIFT_JIS.decode_without_bom_handling_and_without_replacement(text).is_some()))
        || file.try_record(record)?.map(crate::trim_nul) != Some(text)
    {
        return Ok(encoding);
    }
    if record == STANDARD_TEXT_RECORD && text == KCS_CHOICE_PREVIEW {
        return Ok(ScriptTextEncoding::Cp932);
    }
    // A legacy CST snapshot can copy the preview from the recorded scene.
    let Some(scene) = file.try_record(5)?.and_then(|header| header.get(4..))
        .map(crate::trim_nul).filter(|scene| !scene.is_empty()) else { return Ok(encoding) };
    if ScriptTextEncoding::Gbk.decode_strict(text).is_some()
        && host.cst_resource_with_source(b"scene", scene)?.is_some_and(|(_, source)|
            host.cst_scene_encoding_override(&source) == Some(ScriptTextEncoding::Gbk))
    {
        return Ok(ScriptTextEncoding::Gbk);
    }
    Ok(encoding)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_panel_loads_legacy_image_formats_and_preserves_cells_on_corruption() -> Result<()> {
        let root = std::env::temp_dir().join(format!("cs2-save-panel-{}-{}", std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos()));
        std::fs::create_dir(&root)?;
        let result = (|| -> Result<()> {
            let mut player = crate::HeadlessPlayer::open_with_save_root(&root, &root)?;
            let mut panel = bank::Bank::default();
            let mut css = CssFile::new(0);
            let pixel = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(2, 2, image::Rgb([11, 31, 71])));
            for (index, format) in [image::ImageFormat::Bmp, image::ImageFormat::Png, image::ImageFormat::Jpeg]
                .into_iter().enumerate()
            {
                let id = index as u32 + 1;
                let mut encoded = std::io::Cursor::new(Vec::new());
                pixel.write_to(&mut encoded, format)?;
                css.append(id, encoded.get_ref())?;
                let mut cell = cell::Cell::default();
                cell.put(0, 1);
                cell.put(4, 1);
                cell.put(0x10, id);
                panel.cells.insert(id as i32, cell);
            }
            let path = root.join("save0001.dat");
            std::fs::write(&path, css.to_css()?)?;
            let mut request = PanelDataRequest::SetSave { slot: 1, record: 0x400, start: 0 };
            player.with_host(|host| {
                panel.data.apply(&mut request, host, &mut ExpressionState::default())?;
                cell::apply_sources(&mut panel, host)
            })?;
            let frames = panel.contents.values().map(|content| content.image.as_ref().unwrap()).collect::<Vec<_>>();
            assert_eq!(frames.len(), 3);
            assert!(frames.iter().all(|frame| frame.width() == 2 && frame.height() == 2));
            assert_eq!(&frames[0].pixels()[..4], &[11, 31, 71, 255]);
            assert_eq!(&frames[1].pixels()[..4], &[11, 31, 71, 255]);
            css.try_remove(2)?;
            css.append(2, b"\x89PNG invalid thumbnail")?;
            panel.data.data = Some(css);
            player.with_host(|host| cell::apply_sources(&mut panel, host))?;
            assert_eq!(&panel.contents[&2].image.as_ref().unwrap().pixels()[..4], &[11, 31, 71, 255]);
            assert!(player.events().iter().any(|event| event.kind == "save-panel-image-error"));
            std::fs::write(&path, b"CSS\0 truncated")?;
            let mut request = PanelDataRequest::SetSave { slot: 1, record: 0x400, start: 0 };
            player.with_host(|host| panel.data.apply(&mut request, host, &mut ExpressionState::default()))?;
            assert!(player.events().iter().any(|event| event.kind == "save-panel-source-error"));
            Ok(())
        })();
        let path = root.join("save0001.dat");
        if path.exists() { std::fs::remove_file(path)?; }
        std::fs::remove_dir(root)?;
        result
    }

    #[test]
    #[ignore = "requires the installed ISLAND Chinese patch and an isolated copy of com05_selB save0000.dat"]
    fn island_save_panel_preserves_kcs_choice_and_gbk_dialogue() -> Result<()> {
        let game = std::env::var_os("CS2_ISLAND_TEST_GAME").context("installed game root required")?;
        let saves = std::env::var_os("CS2_ISLAND_TEST_SAVES").context("isolated save root required")?;
        let mut player = crate::HeadlessPlayer::open_with_save_root(game, saves)?;
        player.with_host(|host| {
            let mut panel = SavePanelData::default();
            let mut request = PanelDataRequest::SetSave { slot: 0, record: 0x400, start: 0 };
            assert_eq!(panel.apply(&mut request, host, &mut ExpressionState::default())?,
                PanelDataOutcome::ApplyCellSources);
            let scene = panel.record(5)?.and_then(|header| header.get(4..)).map(crate::trim_nul);
            assert_eq!(scene, Some(b"com05_selB".as_slice()));
            let (_, source) = host.cst_resource_with_source(b"scene", b"com05_selB")?
                .context("localized com05_selB scene required")?;
            assert_eq!(host.cst_scene_encoding_override(&source), Some(ScriptTextEncoding::Gbk));

            let choice = crate::trim_nul(panel.record(STANDARD_TEXT_RECORD)?.context("missing choice preview")?);
            let encoding = panel.text_encoding(host, STANDARD_TEXT_RECORD, choice)?;
            assert_eq!(encoding, ScriptTextEncoding::Cp932);
            assert_eq!(encoding.decode(choice), "[\u{9078}\u{629e}\u{80a2}]");

            let dialogue = crate::trim_nul(panel.record(0x80000000)?.context("missing snapshot dialogue")?)
                .to_vec();
            let encoding = panel.text_encoding(host, 0x80000000, &dialogue)?;
            assert_eq!(encoding, ScriptTextEncoding::Gbk);
            assert_eq!(encoding.decode(&dialogue),
                "\u{5582}\u{ff0c}\u{4e5f}\u{5e26}\u{6211}\u{53bb}\u{561b}\u{ff01}\u{53bb}\u{6d77}\u{8fb9}\u{ff01}");

            let css = panel.data.as_mut().context("missing CSS container")?;
            assert!(css.remove(STANDARD_TEXT_RECORD));
            let mut preview = dialogue.clone();
            preview.push(0);
            css.append(STANDARD_TEXT_RECORD, &preview)?;
            assert_eq!(panel.text_encoding(host, STANDARD_TEXT_RECORD, &dialogue)?, ScriptTextEncoding::Gbk);
            Ok(())
        })
    }
}

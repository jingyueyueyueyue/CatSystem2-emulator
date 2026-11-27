//! Panel top-coordinate composition over the existing HG, CSS and message
//! glyph consumers. Only the final frame belongs to runtime.media/list state.
use super::*;
use std::collections::BTreeMap;
use anyhow::{bail, ensure};
use cs2_formats::HgFile;
use cs2_renderer::{Canvas, FrameMetadata, GlyphEffects, ImageDrawOptions, Point, RgbaFrame};
use cs2_runtime::message_object::{MessageConfiguration, MessageDefaults, MessageEncoding};
use crate::{FesRuntime, ScriptTextEncoding};

const MAX_PIXELS: u64 = 64 * 1024 * 1024;
const MAX_SOURCES: usize = 128 * 1024 * 1024;

pub(super) fn checked_size(size: [u32; 2]) -> Result<()> {
    ensure!(size.into_iter().all(|axis| axis != 0 && axis <= i32::MAX as u32)
        && u64::from(size[0]) * u64::from(size[1]) * 4 <= MAX_PIXELS,
        "SAVEPANEL pixel canvas exceeds its supported allocation");
    Ok(())
}

fn source<'a>(host: &mut LoggingHost<'_>, sources: &'a mut BTreeMap<Vec<u8>, Vec<u8>>,
    name: &str) -> Result<Option<&'a [u8]>> {
    if name.is_empty() { return Ok(None); }
    for candidate in host.resources.optional_category_names(b"image", name)?.into_iter().flatten() {
        let key = candidate.as_bytes().to_vec();
        if !sources.contains_key(&key) {
            let Some(bytes) = host.read_managed_raw_image_bytes(&candidate)? else { continue };
            let total = sources.values().try_fold(bytes.len(), |sum, bytes| sum.checked_add(bytes.len()))
                .context("SAVEPANEL retained source length overflow")?;
            ensure!(total <= MAX_SOURCES, "SAVEPANEL retained resources exceed 128 MiB");
            sources.insert(key.clone(), bytes);
        }
        return Ok(sources.get(&key).map(Vec::as_slice));
    }
    Ok(None)
}

fn image_name(host: &LoggingHost<'_>, raw: &[u8]) -> Result<String> {
    Ok(host.text_encoding.decode(&host.data_services.language.languages.expand_image(raw)?))
}

fn blend_resource_image(target: &mut Canvas, host: &mut LoggingHost<'_>, bytes: &[u8],
    id: u32, position: [i32; 2]) -> Result<()> {
    // Reuse the real image decoder/patch hook. Do not substitute thumbnails
    // from ordinary IMAGE.current-frame for the requested cell frame.
    let Some(image) = host.decode_managed_image_frame(bytes, id)? else { return Ok(()); };
    blend_image(target, &image, position)
}

pub(super) fn update(panel: &mut bank::Bank, sources: &mut BTreeMap<Vec<u8>, Vec<u8>>,
    host: &mut LoggingHost<'_>, runtime: &mut FesRuntime) -> Result<()> {
    let expanded = host.data_services.language.languages.expand_image(&panel.file)?;
    let name = host.text_encoding.decode(&expanded);
    let layer = runtime.media.get_mut(&panel.layer).context("SAVEPANEL composition layer is absent")?;
    ensure!(layer.kind == "SAVEPANEL", "SAVEPANEL composition layer has a different owner kind");
    if layer.metadata.size_dirty { panel.dirty = true; }
    let size_source = layer.metadata.size_source.clone();
    if let Some(reference) = layer.metadata.position_source.clone() {
        let position = crate::fes_controls::read_metadata(host, &reference)?.map_or([0; 2], |(position, _)| position);
        layer.position = Point::new(position[0], position[1]);
        if let Some(words) = &mut layer.motion {
            words[6] = (position[0] as f32).to_bits(); words[7] = (position[1] as f32).to_bits();
        }
        layer.metadata.position_source = None;
    }
    if panel.expanded_file.as_ref() != Some(&expanded) { panel.dirty = true; }
    // A referenced IMAGE file/table can change independently of the panel.
    // Detect that real source identity without rebuilding every idle frame.
    let mut references = Vec::new();
    for content in panel.contents.values() {
        if matches!(content.record.word(0), 2 | 5) {
            if let Some(image) = reference_image(runtime, host, content) {
                references.push((image.file.clone(), image.frame_ids.clone()));
            } else { references.push((None, Vec::new())); }
        }
    }
    if panel.reference_sources != references { panel.dirty = true; }
    if !panel.dirty { return Ok(()); }
    let Some(bytes) = source(host, sources, &name)? else {
        let layer = runtime.media.get_mut(&panel.layer).context("SAVEPANEL composition layer is absent")?;
        layer.image = None; layer.file = (!name.is_empty()).then_some(name);
        layer.metadata.size_dirty = false;
        panel.expanded_file = Some(expanded); panel.reference_sources = references;
        panel.dirty = false; return Ok(());
    };
    let directory = HgFile::parse(bytes)?;
    let Some(first) = directory.frames.first() else { return Ok(()) };
    let size = first.canvas_size;
    checked_size(size)?;
    // Own the main bytes only during composition so loading a digit/userfont
    // into the same source cache never holds a mutable alias of that cache.
    let bytes = bytes.to_vec();
    let mut canvas = Canvas::new(size[0], size[1])?;
    host.data_services.message_device.load_fonts(host.resources);
    for &id in &panel.enabled {
        if id == -1 { break; }
        let Some(content) = panel.contents.get(&id) else { continue };
        let position = directory.frames.iter().find(|info| info.id == id as u32)
            .map_or([0; 2], |info| info.offset);
        match content.record.word(0) {
            0 => blend_resource_image(&mut canvas, host, &bytes, id as u32, [0; 2])?,
            1 | 6 => if let Some(image) = &content.image {
                blend_image(&mut canvas, image, position)?;
            },
            2 | 5 => {
                let Some(reference) = reference_image(runtime, host, content) else { continue };
                let Some(name) = reference.file.clone() else { continue };
                let mut selected = content.record.word(0x40) as i32;
                let selector = content.record.word(0x44) as i32;
                if selector != 0 {
                    let divisor = selector.wrapping_mul(10);
                    ensure!(divisor != 0 && !(selected == i32::MIN && divisor == -1),
                        "SAVEPANEL digit selector divides outside i32");
                    selected = (selected % divisor).checked_div(selector)
                        .context("SAVEPANEL digit selector division overflow")?;
                }
                if content.record.word(0x48) == 0 && !reference.frame_ids.is_empty() {
                    selected = reference.frame_ids.get(selected as usize).copied().unwrap_or(0) as i32;
                }
                if let Some(bytes) = source(host, sources, &name)? {
                    blend_resource_image(&mut canvas, host, bytes, selected as u32, position)?;
                }
            }
            3 | 4 => text(&mut canvas, host, sources, panel, id, content, position)?,
            7 => if let Some(image) = &content.image {
                if let Some(image) = crop_image(image, &content.record)? {
                    blend_image(&mut canvas, &image, position)?;
                }
            },
            _ => bail!("SAVEPANEL applied content kind is invalid"),
        }
    }
    let mut output = RgbaFrame::from_rgba8(size[0], size[1], canvas.pixels().to_vec())?
        .with_raw_metadata(FrameMetadata::raster(size));
    // The assembled cells use the HG source coordinates. Resize the complete
    // layer so their positions and pixels follow the same mapping as the base.
    if let Some(reference) = &size_source
        && let Some((_, [width, height])) = crate::fes_controls::read_metadata(host, reference)?
        && width > 0 && height > 0
    {
        let requested = [width as u32, height as u32];
        checked_size(requested)?;
        if requested != size {
            output = crate::image_resample::resize_canvas(&output, requested)?;
        }
    }
    let size = [output.width() as i32, output.height() as i32];
    let layer = runtime.media.get_mut(&panel.layer).context("SAVEPANEL composition layer is absent")?;
    layer.image = Some(output); layer.image_offset = [0; 2]; layer.image_base = [0; 2];
    layer.file = Some(name); layer.navigation.set_size(size);
    layer.metadata.size_dirty = false;
    panel.expanded_file = Some(expanded); panel.reference_sources = references; panel.dirty = false;
    Ok(())
}

fn reference_image<'a>(runtime: &'a FesRuntime, host: &LoggingHost<'_>, content: &cell::Content)
    -> Option<&'a crate::FesMediaLayer> {
    let name = host.text_encoding.decode(content.record.name(4, 0x24));
    let bank = content.record.word(0x24) as i32;
    let banked = format!("{name}[{bank}]");
    runtime.media.get(&banked).or_else(|| (bank == 0).then(|| runtime.media.get(&name)).flatten())
        .filter(|image| matches!(image.kind.as_str(), "IMAGE" | "BUTTON"))
}

fn blend_image(target: &mut Canvas, image: &RgbaFrame, at: [i32; 2]) -> Result<()> {
    let metadata = image.metadata();
    let at = [at[0].wrapping_add(metadata.offset[0]), at[1].wrapping_add(metadata.offset[1])];
    let pixels = image.pixels();
    // Preserve the original panel clip: negative placement clamps the target
    // origin while reading from source pixel zero, instead of skipping it.
    let start = [at[0].max(0), at[1].max(0)];
    let end = [(i64::from(at[0]) + i64::from(target.width())).min(i64::from(target.width())),
        (i64::from(at[1]) + i64::from(target.height())).min(i64::from(target.height()))];
    if end[0] <= i64::from(start[0]) || end[1] <= i64::from(start[1]) { return Ok(()); }
    let width = image.width().min((end[0] - i64::from(start[0])) as u32);
    let height = image.height().min((end[1] - i64::from(start[1])) as u32);
    if width == 0 || height == 0 { return Ok(()); }
    let mut clipped = vec![0; width as usize * height as usize * 4];
    for y in 0..height as usize {
        let input = y * image.width() as usize * 4;
        let output = y * width as usize * 4;
        clipped[output..output + width as usize * 4].copy_from_slice(&pixels[input..input + width as usize * 4]);
    }
    let image = RgbaFrame::from_rgba8(width, height, clipped)?;
    target.draw_frame(&image, &ImageDrawOptions::new(Point::new(start[0], start[1])))?;
    Ok(())
}

fn crop_image(image: &RgbaFrame, record: &cell::Cell) -> Result<Option<RgbaFrame>> {
    let size = [record.word(0xa4), record.word(0xa8)]; checked_size(size)?;
    let image = crate::image_resample::resize_canvas(image, size)?;
    let size = [image.width(), image.height()];
    let crop = [0xac, 0xb0, 0xb4, 0xb8].map(|at| record.word(at) as i32);
    ensure!(crop.into_iter().all(|v| v >= 0), "SAVEPANEL crop has a negative extent or origin");
    ensure!(crop[0] <= size[0] as i32 && crop[1] <= size[1] as i32,
        "SAVEPANEL crop starts outside the resized image");
    let out = [(crop[2] as u32).min(size[0] - crop[0] as u32),
        (crop[3] as u32).min(size[1] - crop[1] as u32)];
    if out.contains(&0) { return Ok(None); }
    checked_size(out)?;
    let mut pixels = vec![0; out[0] as usize * out[1] as usize * 4];
    for y in 0..out[1] {
        for x in 0..out[0] {
            let sx = x + crop[0] as u32;
            let sy = y + crop[1] as u32;
            let from = (sy as usize * image.width() as usize + sx as usize) * 4;
            let to = (y as usize * out[0] as usize + x as usize) * 4;
            pixels[to..to + 4].copy_from_slice(&image.pixels()[from..from + 4]);
        }
    }
    Ok(Some(RgbaFrame::from_rgba8(out[0], out[1], pixels)?))
}

fn text(target: &mut Canvas, host: &mut LoggingHost<'_>, sources: &mut BTreeMap<Vec<u8>, Vec<u8>>,
    panel: &bank::Bank, id: i32, content: &cell::Content, mut position: [i32; 2]) -> Result<()> {
    if content.text.is_empty() { return Ok(()); }
    let record = &content.record;
    let size = record.word(0x34);
    ensure!(size <= 120, "SAVEPANEL text exceeds the shared 128px glyph work surface");
    let font = record.name(0x64, 188);
    let font_name = if font.is_empty() {
        host.data_services.messages.defaults.as_ref().map_or_else(Vec::new, |value| value.font_name.clone())
    } else { host.text_encoding.decode(font).encode_utf16().collect() };
    ensure!(font_name.len() < 64, "SAVEPANEL font exceeds the shared UTF16 field");
    let defaults = MessageDefaults { font_name, bold: false, shadow: 0, outline: 0,
        glyph_parameter: Some(1), gradual: false };
    let mut config = defaults.configuration(MessageEncoding::Unicode)?.bytes().to_vec();
    config[0x104..0x108].copy_from_slice(&400u32.to_le_bytes());
    let config = MessageConfiguration::from_bytes(&config, MessageEncoding::Unicode)?;
    let source_record = panel.cells.get(&id).filter(|cell| cell.word(0) == 3 && cell.word(4) == 2)
        .map_or(u32::MAX, |cell| cell.word(0x10));
    let encoding = panel.data.text_encoding(host, source_record, &content.text)?;
    let (text, bad_at) = encoding.decode_message(&content.text);
    if let Some(offset) = bad_at {
        host.data_services.note_text_decode(host.events, host.frame, format!(
            "SAVEPANEL file={:?} record={source_record:#x} cell={id} encoding={encoding:?} offset={offset}",
            String::from_utf8_lossy(panel.data.save_name()),
        ));
    }
    let size_i = size as i32;
    let limit = (record.word(0x3c) as i32).wrapping_mul(size_i).wrapping_sub(size_i / 2);
    let start = position[0]; let (mut width, mut line) = (0i32, 0i32);
    let effects = GlyphEffects { outline: record.word(0x4c) != 0, shadow: record.word(0x50) != 0,
        ..Default::default() };
    for character in text.chars() {
        if width > limit {
            line = line.wrapping_add(1);
            if line >= record.word(0x5c) as i32 { break; }
            position[0] = start; position[1] = position[1].wrapping_add(size_i).wrapping_add(record.word(0x60) as i32);
            width = 0;
        }
        let mut glyph_canvas = Canvas::new(size + 8, size + 8)?;
        // 5b629f..5b6411 registers the appended entries in ascending order;
        // 5e7b54..5e7b94 removes an existing character before replacing it.
        // Keep the original vector for checkpoint/release, but consume its
        // final registration (V401 repeats 0x8744 with frames 5 through 9).
        let mapping = panel.user_fonts.get(&id).and_then(|fonts| fonts.iter().rev().find(|entry|
            user_character(entry.character) == Some(character)));
        let advance = if let Some(mapping) = mapping {
            let name = image_name(host, &mapping.file)?;
            let bytes = source(host, sources, &name)?.context("SAVEPANEL userfont resource is absent")?;
            if let Some(glyph) = crate::message_native::decode_user_glyph(bytes, mapping.frame)? {
                crate::message_backend::paint_message_user_glyph(&mut glyph_canvas, &glyph, [0; 2], size,
                    record.word(0x54), effects)?;
                size_i
            } else { 0 }
        } else {
            let glyph = host.data_services.message_device.legacy_mask(&config, character, size)?;
            crate::message_backend::paint_message_mask(&mut glyph_canvas, &glyph, [0; 2], size,
                record.word(0x38), record.word(0x54), effects)?;
            glyph.advance()
        };
        let image = RgbaFrame::from_rgba8(size + 8, size + 8, glyph_canvas.pixels().to_vec())?;
        blend_image(target, &image, position)?;
        position[0] = position[0].wrapping_add(if advance == 0 { size_i / 2 } else { advance });
        width = width.wrapping_add(if advance == 0 { size_i } else { advance });
        if effects.outline || effects.shadow {
            position[0] = position[0].wrapping_sub(2); width = width.wrapping_sub(2);
        }
    }
    Ok(())
}

fn user_character(code: u16) -> Option<char> {
    let bytes = if code > 255 { vec![(code >> 8) as u8, code as u8] } else { vec![code as u8] };
    let (text, _, errors) = encoding_rs::SHIFT_JIS.decode(&bytes);
    if errors { return None; }
    text.chars().next()
}

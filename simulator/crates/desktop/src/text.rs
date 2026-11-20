use super::{ScriptTextEncoding, read_entry_with_fallback};
use anyhow::Result;
use cs2_renderer::{Canvas, Color, FontRasterizer, Point};
use cs2_runtime::ResourceIndex;
use std::collections::BTreeSet;
use std::path::Path;
#[cfg(any(windows, target_os = "android"))]
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Debug, PartialEq, Eq)]
pub(super) enum FontOrigin {
    Resource(String),
    #[cfg(any(windows, target_os = "android"))]
    System(PathBuf),
}

#[derive(Debug)]
pub(super) struct LoadedFont {
    pub origin: Arc<FontOrigin>,
    pub index: u32,
    pub rasterizer: Arc<FontRasterizer>,
}

pub(super) fn load_font(resources: &ResourceIndex) -> Vec<LoadedFont> {
    let mut fonts = Vec::new();
    let names = resources
        .files()
        .iter()
        .map(|file| file.relative_path.to_string_lossy().replace('\\', "/"))
        .chain(resources.archives().iter().flat_map(|archive| {
            archive
                .entries
                .iter()
                .map(|entry| entry.name.replace('\\', "/"))
        }));
    let mut seen = BTreeSet::new();
    for name in names {
        let extension = Path::new(&name)
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default();
        if !["ttf", "ttc", "otf"]
            .iter()
            .any(|known| extension.eq_ignore_ascii_case(known))
        {
            continue;
        }
        if !seen.insert(name.to_ascii_lowercase()) {
            continue;
        }
        if let Ok(bytes) = read_entry_with_fallback(resources, &name) {
            append_faces(&mut fonts, FontOrigin::Resource(name), &bytes);
        }
    }

    #[cfg(windows)]
    {
        let windows = std::env::var_os("WINDIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from(r"C:\Windows"));
        for name in [
            "msgothic.ttc",
            "msyh.ttc",
            "simsun.ttc",
            "simsunb.ttf",
            "arial.ttf",
        ] {
            let path = windows.join("Fonts").join(name);
            if let Ok(bytes) = std::fs::read(&path) {
                append_faces(&mut fonts, FontOrigin::System(path), &bytes);
            }
        }
    }
    #[cfg(target_os = "android")]
    {
        // Private game fonts retain precedence. Use a small real glyph
        // fallback set rather than decoding every installed CJK collection.
        let mut system_faces = 0;
        let mut cjk_loaded = false;
        for path in android_font_paths() {
            let name = path.file_name().and_then(|name| name.to_str()).unwrap_or_default();
            let cjk = android_font_priority(name) <= 1;
            if (cjk && cjk_loaded) || system_faces == 4 || fonts.len() >= 64 {
                continue;
            }
            if let Ok(bytes) = std::fs::read(&path) {
                if let Ok(font) = FontRasterizer::from_bytes(&bytes, 0) {
                    fonts.push(LoadedFont {
                        origin: Arc::new(FontOrigin::System(path)),
                        index: 0,
                        rasterizer: Arc::new(font),
                    });
                    system_faces += 1;
                    cjk_loaded |= cjk;
                }
            }
        }
    }
    fonts
}

/// Real installed paths, in deterministic fallback order. The CPU message
/// device can reuse these for SFNT name discovery and lazy face loading.
#[cfg(target_os = "android")]
pub(super) fn android_font_paths() -> Vec<std::path::PathBuf> {
    let mut paths = Vec::new();
    if let Ok(entries) = std::fs::read_dir("/system/fonts") {
        for entry in entries.flatten() {
            let path = entry.path();
            let known = path.extension().and_then(|extension| extension.to_str())
                .is_some_and(|extension| ["ttf", "ttc", "otf"].iter()
                    .any(|known| extension.eq_ignore_ascii_case(known)));
            if known && path.is_file() {
                paths.push(path);
            }
        }
    }
    paths.sort_by(|left, right| {
        let a = left.file_name().and_then(|name| name.to_str()).unwrap_or_default();
        let b = right.file_name().and_then(|name| name.to_str()).unwrap_or_default();
        android_font_priority(a).cmp(&android_font_priority(b)).then_with(|| left.cmp(right))
    });
    paths
}

#[cfg(target_os = "android")]
fn android_font_priority(name: &str) -> u8 {
    let name = name.to_ascii_lowercase();
    if name.contains("cjk") && name.contains("regular") {
        0
    } else if name == "droidsansfallback.ttf" {
        1
    } else if name == "roboto-regular.ttf" {
        2
    } else if name == "notosans-regular.ttf" {
        3
    } else if (name.contains("sansjp") || name.contains("sanssc")
        || name.contains("sanstc") || name.contains("sanskr")) && name.contains("regular")
    {
        1
    } else if name.contains("symbols") && name.contains("regular") {
        4
    } else if name.contains("regular") {
        5
    } else {
        6
    }
}

fn append_faces(fonts: &mut Vec<LoadedFont>, origin: FontOrigin, bytes: &[u8]) {
    let origin = Arc::new(origin);
    let count = if bytes.starts_with(b"ttcf") {
        bytes
            .get(8..12)
            .map(|count| u32::from_be_bytes(count.try_into().unwrap()))
            .unwrap_or(0)
            .min(16)
    } else {
        1
    };
    for face in 0..count {
        if fonts.len() >= 64 {
            break;
        }
        if let Ok(font) = FontRasterizer::from_bytes(bytes, face) {
            fonts.push(LoadedFont {
                origin: Arc::clone(&origin),
                index: face,
                rasterizer: Arc::new(font),
            });
        }
    }
}

pub(super) fn draw_message_text(
    canvas: &mut Canvas,
    fonts: &[LoadedFont],
    speaker: &str,
    text: &str,
    encoding: ScriptTextEncoding,
) -> Result<()> {
    let margin = (canvas.width() / 20).max(1) as i32;
    let max_width = (canvas.width() as i32 - 2 * margin).max(1);
    let speaker_y = (canvas.height() as i32 - 170).max(24);
    let text_y = (canvas.height() as i32 - 130).max(52);
    draw_text_block(
        canvas,
        fonts,
        speaker,
        Point::new(margin, speaker_y),
        24,
        Color::rgb(255, 220, 120),
        max_width,
        encoding,
    )?;
    draw_text_block(
        canvas,
        fonts,
        text,
        Point::new(margin, text_y),
        26,
        Color::rgb(255, 255, 255),
        max_width,
        encoding,
    )?;
    Ok(())
}

fn draw_text_block(
    canvas: &mut Canvas,
    fonts: &[LoadedFont],
    text: &str,
    origin: Point,
    size: u32,
    color: Color,
    max_width: i32,
    encoding: ScriptTextEncoding,
) -> Result<()> {
    let style = cs2_renderer::TextStyle {
        color,
        ruby_color: color,
        size,
        ruby_size: (size / 2).max(1),
        ..Default::default()
    };
    let encoding = if encoding == ScriptTextEncoding::Utf8 {
        cs2_renderer::TextEncoding::Unicode
    } else {
        cs2_renderer::TextEncoding::Cp932
    };
    let markup = cs2_renderer::TextMarkup::parse_with_encoding(text, style, encoding)
        .map_err(|error| anyhow::anyhow!(error))?;
    let run = markup
        .layout_in_width(
            origin,
            max_width,
            size as i32 + 8,
            0,
            |character, size, bold, italic| {
                let font = fonts
                    .iter()
                    .map(|face| &face.rasterizer)
                    .find(|font| font.has_character(character))
                    .ok_or_else(|| cs2_renderer::RendererError::Decode {
                        message: format!(
                            "no available message font contains U+{:04X}",
                            character as u32
                        ),
                    })?;
                font.rasterize_styled(character, size, bold, italic)
            },
        )
        .map_err(|error| anyhow::anyhow!(error))?;
    let top = origin.y.saturating_sub((size / 2) as i32).max(0);
    let clip = cs2_renderer::Rect::new(
        origin.x.max(0),
        top,
        max_width.max(0) as u32,
        canvas.height().saturating_sub(top as u32),
    );
    canvas
        .draw_text_run(
            &run,
            255,
            Some(clip),
            None,
            cs2_renderer::BlendMode::SourceOver,
            cs2_renderer::GlyphEffects::default(),
        )
        .map_err(|error| anyhow::anyhow!(error))
}

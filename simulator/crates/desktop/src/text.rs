use super::ScriptTextEncoding;
use super::perf_detail::{TextStage, TextTimer};
use anyhow::Result;
use cs2_renderer::{Canvas, Color, FontRasterizer, Point};
use cs2_runtime::ResourceIndex;
use std::collections::BTreeSet;
use std::io::{self, Read};
use std::path::Path;
#[cfg(any(windows, target_os = "android"))]
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

#[path = "font_cache.rs"]
mod font_cache;
pub(super) use font_cache::{
    FontVersion, prepare_font_face, resource_font_version, shared_candidate_font, shared_font_face,
    shared_font_file, shared_resource_font,
};

#[path = "font_metadata.rs"]
mod font_metadata;
pub(super) use font_metadata::system_font_metadata;

pub(super) fn system_font_paths() -> Vec<std::path::PathBuf> {
    #[cfg(target_os = "android")]
    { return android_font_paths(); }
    #[cfg(windows)]
    {
        let windows = std::env::var_os("WINDIR").map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from(r"C:\Windows"));
        let mut folders = vec![windows.join("Fonts")];
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            folders.push(std::path::PathBuf::from(local).join("Microsoft/Windows/Fonts"));
        }
        let mut paths = Vec::new();
        for folder in folders {
            if let Ok(entries) = std::fs::read_dir(folder) {
                let mut files: Vec<_> = entries.flatten().map(|entry| entry.path()).filter(|path| {
                    path.extension().and_then(|extension| extension.to_str()).is_some_and(|extension| {
                        ["ttf", "ttc", "otf"].iter().any(|known| extension.eq_ignore_ascii_case(known))
                    })
                }).collect();
                files.sort();
                paths.extend(files);
            }
        }
        return paths;
    }
    #[cfg(not(any(windows, target_os = "android")))]
    { Vec::new() }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum FontOrigin {
    Resource(String),
    #[cfg(any(windows, target_os = "android"))]
    System(PathBuf),
}

#[cfg(all(test, windows))]
mod font_cache_tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct Fixture(std::path::PathBuf);
    impl Fixture {
        fn new() -> Self {
            let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
            let root = std::env::temp_dir().join(format!("cs2-shared-font-{nonce}"));
            fs::create_dir_all(root.join("font")).unwrap();
            let windows = std::path::PathBuf::from(std::env::var_os("WINDIR").unwrap());
            fs::copy(windows.join("Fonts/arial.ttf"), root.join("font/face.ttf")).unwrap();
            Self(root)
        }
        fn scan(&self) -> ResourceIndex { ResourceIndex::scan(&self.0).unwrap() }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_file(self.0.join("font/face.ttf"));
            let _ = fs::remove_dir(self.0.join("font"));
            let _ = fs::remove_dir(&self.0);
        }
    }
    fn game_face(fonts: &[LoadedFont]) -> &LoadedFont {
        fonts.iter().find(|font| matches!(font.origin.as_ref(), FontOrigin::Resource(_))).unwrap()
    }

    #[test]
    fn startup_font_face_is_ready_before_first_character() {
        let fixture = Fixture::new();
        let fonts = load_font(&fixture.scan());
        let face = game_face(&fonts);
        assert!(face.rasterizer.get().is_some(), "startup must decode the font face before text is requested");
        assert!(face.for_character('A').is_some());
    }

    #[test]
    fn repeated_font_load_shares_bytes_and_decoded_face() {
        let fixture = Fixture::new();
        let index = fixture.scan();
        let first = load_font(&index);
        let second = load_font(&index);
        let first = game_face(&first);
        let second = game_face(&second);
        assert!(Arc::ptr_eq(&first.bytes, &second.bytes), "same version must be read once across entry points");
        assert!(Arc::ptr_eq(&first.rasterizer, &second.rasterizer), "same face must decode once across entry points");
    }

    #[test]
    fn replacement_font_invalidates_bytes_without_changing_old_readers() {
        let fixture = Fixture::new();
        let index = fixture.scan();
        let first = load_font(&index);
        let original = game_face(&first);
        let original_length = original.bytes.len();
        let mut replacement = original.bytes.as_ref().clone();
        replacement.extend_from_slice(&[0, 0, 0, 0]);
        fs::write(fixture.0.join("font/face.ttf"), &replacement).unwrap();
        let second = load_font(&index);
        let replaced = game_face(&second);
        assert!(!Arc::ptr_eq(&original.bytes, &replaced.bytes));
        assert!(!Arc::ptr_eq(&original.rasterizer, &replaced.rasterizer));
        assert_eq!(original.bytes.len(), original_length);
        assert_eq!(replaced.bytes.len(), original_length + 4);
        assert!(original.for_character('A').is_some());
        assert!(replaced.for_character('A').is_some());
    }

    #[test]
    fn retained_rasterizer_survives_temporary_face_slot_drop() {
        let fixture = Fixture::new();
        let bytes = shared_font_file(&fixture.0.join("font/face.ttf")).unwrap();
        let first = shared_font_face(&bytes, 0);
        let prepared = prepare_font_face(&bytes, 0, &first).unwrap();
        drop(first);
        let second = shared_font_face(&bytes, 0);
        let reused = prepare_font_face(&bytes, 0, &second).unwrap();
        assert!(Arc::ptr_eq(&prepared, &reused), "filename backend retains the rasterizer without a permanent slot");
    }

    #[test]
    fn collection_members_share_bytes_but_keep_distinct_face_identity() {
        let fixture = Fixture::new();
        let member = fs::read(fixture.0.join("font/face.ttf")).unwrap();
        // Two complete real SFNT members; TTC table offsets are absolute.
        let mut collection = b"ttcf\0\x01\0\0\0\0\0\x02\0\0\0\0\0\0\0\0".to_vec();
        for index in 0..2 {
            while collection.len() % 4 != 0 { collection.push(0); }
            let base = collection.len();
            collection[12 + index * 4..16 + index * 4].copy_from_slice(&(base as u32).to_be_bytes());
            collection.extend_from_slice(&member);
            let tables = u16::from_be_bytes(member[4..6].try_into().unwrap());
            for table in 0..usize::from(tables) {
                let offset = base + 12 + table * 16 + 8;
                let old = u32::from_be_bytes(collection[offset..offset + 4].try_into().unwrap());
                collection[offset..offset + 4].copy_from_slice(&(old + base as u32).to_be_bytes());
            }
        }
        fs::write(fixture.0.join("font/face.ttf"), collection).unwrap();
        let index = fixture.scan();
        let first = load_font(&index);
        let members: Vec<_> = first.iter().filter(|font| matches!(font.origin.as_ref(), FontOrigin::Resource(_))).collect();
        assert_eq!(members.len(), 2);
        assert_eq!([members[0].index, members[1].index], [0, 1]);
        assert!(Arc::ptr_eq(&members[0].bytes, &members[1].bytes));
        assert!(!Arc::ptr_eq(&members[0].rasterizer, &members[1].rasterizer));
        let second = load_font(&index);
        let reloaded = second.iter().find(|font| matches!(font.origin.as_ref(), FontOrigin::Resource(_)) && font.index == 1).unwrap();
        assert!(Arc::ptr_eq(&members[1].rasterizer, &reloaded.rasterizer));
    }
}

impl FontOrigin {
    pub(super) fn name(&self) -> &str {
        match self {
            Self::Resource(name) => name,
            #[cfg(any(windows, target_os = "android"))]
            Self::System(path) => path.to_str().unwrap_or("<unnamed system font>"),
        }
    }
}

#[derive(Debug)]
pub(super) struct LoadedFont {
    pub origin: Arc<FontOrigin>,
    pub index: u32,
    pub bytes: Arc<Vec<u8>>,
    pub rasterizer: Arc<OnceLock<Option<Arc<FontRasterizer>>>>,
}

impl LoadedFont {
    pub(super) fn for_character(&self, character: char) -> Option<&FontRasterizer> {
        if let Some(font) = self.rasterizer.get() {
            return font.as_deref().filter(|font| font.has_character(character));
        }
        if ttf_parser::Face::parse(&self.bytes, self.index).ok()
                .and_then(|face| face.tables().cmap)
                .is_some_and(|cmap| !cmap.subtables.into_iter()
                    .any(|table| table.glyph_index(character as u32).is_some()))
        {
            return None;
        }
        let name = match self.origin.as_ref() {
            FontOrigin::Resource(name) => name.as_bytes(),
            #[cfg(any(windows, target_os = "android"))]
            FontOrigin::System(path) => path.file_name().map_or(&[][..], std::ffi::OsStr::as_encoded_bytes),
        };
        let _load = TextTimer::new(TextStage::FaceLoad).face(name, self.index);
        self.rasterizer.get_or_init(|| {
            let _decode = TextTimer::new(TextStage::FaceDecode);
            FontRasterizer::from_shared_bytes(Arc::clone(&self.bytes), self.index).ok().map(Arc::new)
        })
            .as_deref().filter(|font| font.has_character(character))
    }
}

pub(super) fn read_font_file(path: &Path) -> io::Result<Vec<u8>> {
    let file = std::fs::File::open(path)?;
    let limit = FontRasterizer::MAX_FONT_BYTES;
    if file.metadata()?.len() > limit as u64 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "font data exceeds the 64 MiB limit"));
    }

    let mut bytes = Vec::new();
    file.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "font data exceeds the 64 MiB limit"));
    }
    Ok(bytes)
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
        if !use_resource_font(resources, &name) { continue; }
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
        if let Ok(bytes) = shared_resource_font(resources, &name) {
            append_faces(&mut fonts, FontOrigin::Resource(name), bytes);
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
            if let Ok(bytes) = shared_font_file(&path) {
                append_faces(&mut fonts, FontOrigin::System(path), bytes);
            }
        }
    }
    #[cfg(target_os = "android")]
    {
        // Private game fonts retain precedence. Use a small real glyph
        // fallback set rather than decoding every installed CJK collection.
        let mut system_files = 0;
        let mut cjk_loaded = false;
        for path in android_font_paths() {
            let name = path.file_name().and_then(|name| name.to_str()).unwrap_or_default();
            let cjk = android_font_priority(name) <= 1;
            if (cjk && cjk_loaded) || system_files == 4 || fonts.len() >= 64 {
                continue;
            }
            if let Ok(bytes) = shared_font_file(&path) {
                let before = fonts.len();
                // Preserve real collection indices and named regional styles
                // from the selected CJK collection, rather than scanning all
                // installed families again in the message backend.
                append_faces(&mut fonts, FontOrigin::System(path), bytes);
                if fonts.len() != before {
                    system_files += 1;
                    cjk_loaded |= cjk;
                }
            }
        }
    }
    fonts
}

pub(super) fn use_resource_font(resources: &ResourceIndex, name: &str) -> bool {
    if resources.game_language() != cs2_runtime::resource_router::GameLanguage::Japanese {
        return true;
    }
    let Some(name) = Path::new(name).file_name().and_then(|name| name.to_str()) else { return true; };
    !["WenQuanYi_msgothic.ttf", "WenQuanYi_msgothic.otf", "WenQuanYi_CNJP.ttf",
        "UtateMeguriSimplifiedChinese.otf", "HatsumiraSimplifiedChinese.ttf", "simhei_CNJP.ttf"]
        .iter().any(|translation| name.eq_ignore_ascii_case(translation))
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

fn append_faces(fonts: &mut Vec<LoadedFont>, origin: FontOrigin, bytes: Arc<Vec<u8>>) {
    if bytes.len() > FontRasterizer::MAX_FONT_BYTES {
        return;
    }
    let origin = Arc::new(origin);
    let count = if bytes.starts_with(b"ttcf") {
        bytes
            .get(8..12)
            .map(|count| u32::from_be_bytes(count.try_into().unwrap()))
            .unwrap_or(0)
            .min(64)
    } else {
        1
    };
    for face in 0..count {
        if fonts.len() >= 64 {
            break;
        }
        let rasterizer = shared_font_face(&bytes, face);
        let _load = TextTimer::new(TextStage::FaceLoad).face(origin.name().as_bytes(), face);
        // Prepare the cmap/metrics face once in the startup phase. A warm
        // shared face needs neither another SFNT parse nor another decode.
        // Outlines and bounded glyph/text caches fill for requested glyphs.
        if prepare_font_face(&bytes, face, &rasterizer).is_some() {
            fonts.push(LoadedFont {
                origin: Arc::clone(&origin),
                index: face,
                bytes: Arc::clone(&bytes),
                rasterizer,
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
                    .find_map(|face| face.for_character(character))
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

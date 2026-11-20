//! CPU message layout and persistent real pixels. This is an independent
//! implementation over the project's FontRasterizer/GlyphMask/Canvas APIs.
//! Layout, glyph effects and ordinary plane transforms share this CPU path.
//!
//! Batch priorities came from static sample payloads (including archive/update
//! versions): 193 XML and 728 FES entries, with indent=1 at 111 layout sites,
//! Unicode profmode=1 at 13, rsize=0 at 32, and fontfix values 0/1 at 22/4.
//! No sample, VM, compiler or test was executed to derive those counts.
//! Effect arithmetic follows docs/message.md, not the generic Canvas maximum
//! outline. Exact font hinting and full custom break-character sets remain
//! separate from this portable pixel/layout implementation.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use cs2_renderer::{
    Canvas, Color, FontRasterizer, GlyphEffects, GlyphMask, GlyphMetrics, ImageDrawOptions, Point,
    Rect, RendererError, RgbaFrame,
};
use cs2_runtime::{ResourceIndex, ResourceLocation};
use cs2_runtime::message_object::{
    MessageAtom, MessageAtomKind, MessageBackend, MessageBreakCharacters, MessageConfiguration,
    MessageCopy, MessageEncoding, MessageFontSubstitution, MessageGlyphState, MessageLayout,
    MessageLayoutEntry, MessageObject, MessagePaintFailure, MessagePaintResult,
    MessagePixelSnapshot, MessageRenderRequest, MessageSurfaceChange, MessageUserGlyph,
};
use cs2_script::{Result, ScriptError};

use super::message_native::{decode_inline_image, decode_user_glyph, scale_user_glyph};
use super::text::{FontOrigin, LoadedFont};

const MAX_SURFACE_BYTES: usize = 64 * 1024 * 1024;

#[cfg(any(windows, target_os = "android"))]
fn is_east_asian_message_character(character: char) -> bool {
    matches!(character as u32,
        0x2e80..=0x9fff | 0xac00..=0xd7af | 0xf900..=0xfaff | 0xfe30..=0xffef
            | 0x20000..=0x2ffff | 0x30000..=0x323af)
}
const MAX_DEVICE_BYTES: usize = 256 * 1024 * 1024;
const GLYPH_SIDE: usize = 128;

#[cfg(windows)]
fn installed_settings_fonts() -> Vec<(String, bool)> {
    #[repr(C)]
    struct LogFont { metrics: [i32; 5], flags: [u8; 8], face: [u16; 32] }
    #[link(name = "gdi32")]
    unsafe extern "system" {
        fn EnumFontFamiliesExW(dc: isize, font: *const LogFont,
            callback: unsafe extern "system" fn(*const LogFont, *const std::ffi::c_void, u32, isize) -> i32,
            parameter: isize, flags: u32) -> i32;
    }
    #[link(name = "user32")]
    unsafe extern "system" { fn GetDC(window: isize) -> isize; fn ReleaseDC(window: isize, dc: isize) -> i32; }
    unsafe extern "system" fn collect(font: *const LogFont, _: *const std::ffi::c_void, _: u32, parameter: isize) -> i32 {
        let font = unsafe { &*font };
        let rows = unsafe { &mut *(parameter as *mut Vec<(String, bool)>) };
        if font.flags[3] == 128 && font.face[0] != 0x40 && rows.len() < 4096 {
            let end = font.face.iter().position(|&unit| unit == 0).unwrap_or(32);
            if let Ok(name) = String::from_utf16(&font.face[..end]) {
                rows.push((name, font.flags[7] & 15 == 1));
            }
        }
        1
    }
    let mut rows = Vec::new();
    let dc = unsafe { GetDC(0) };
    if dc != 0 {
        let mut font = LogFont { metrics: [0; 5], flags: [0; 8], face: [0; 32] };
        font.flags[3] = 128;
        unsafe { EnumFontFamiliesExW(dc, &font, collect, &mut rows as *mut _ as isize, 0); ReleaseDC(0, dc); }
    }
    rows
}

fn invalid(message: &'static str) -> ScriptError {
    ScriptError::InvalidStructuredText { offset: 0, message }
}

fn message_surface_error(width: u32, height: u32, error: RendererError) -> ScriptError {
    ScriptError::HostCallFailed {
        message: format!("message pixel surface {width}x{height}: {error}"),
    }
}

#[derive(Debug)]
pub(super) struct DesktopMessageDevice {
    fonts: Arc<[LoadedFont]>,
    /// SFNT aliases share one lazy face. System font paths are retained instead
    /// of eagerly decoding/copying every installed font into every alias.
    named_fonts: BTreeMap<String, Vec<Arc<NamedFace>>>,
    resource_font_files: BTreeMap<String, GameFontFile>,
    fallback_faces: Vec<Arc<NamedFace>>,
    platform_substitutions: RefCell<BTreeSet<MessageFontSubstitution>>,
    fonts_loaded: bool,
    surfaces: BTreeMap<i32, MessageSurface>,
    pixel_less_message: Option<i32>,
    pub(super) time_scale: Option<u32>,
    pub(super) drawing_enabled: bool,
    pub(super) settings_loaded: bool,
    pub(super) ruby_font_name: Vec<u16>,
    raw_font_controls: Option<[u32; 5]>,
    cursor_visible: bool,
    cursor_visibility_request: Option<bool>,
}

#[derive(Debug)]
enum FontSource {
    Bytes(Arc<Vec<u8>>),
    File(std::path::PathBuf),
}

#[derive(Debug)]
struct GameFontFile {
    path: PathBuf,
    face: OnceLock<std::result::Result<GameFontFace, String>>,
}

#[derive(Debug)]
struct GameFontFace {
    font: FontRasterizer,
    bold: bool,
    italic: bool,
}

impl GameFontFile {
    fn load(&self) -> std::result::Result<GameFontFace, String> {
        let mut bytes = Vec::new();
        std::fs::File::open(&self.path)
            .and_then(|file| file.take(MAX_SURFACE_BYTES as u64 + 1).read_to_end(&mut bytes))
            .map_err(|error| format!("could not be read: {error}"))?;
        let font = FontRasterizer::from_bytes(&bytes, 0)
            .map_err(|error| format!("could not be decoded: {error}"))?;
        let offset = if bytes.starts_with(b"ttcf") {
            be32(&bytes, 12).unwrap_or(0) as usize
        } else {
            0
        };
        Ok(GameFontFace {
            font,
            bold: sfnt_weight(&bytes, offset) == Some(700),
            italic: sfnt_italic(&bytes, offset),
        })
    }
}

#[derive(Debug)]
struct NamedFace {
    source: Arc<FontSource>,
    index: u32,
    weight: u16,
    italic: bool,
    last_resort: bool,
    private: bool,
    face_name: String,
    cjk: bool,
    serif: bool,
    monospace: bool,
    font: OnceLock<Option<Arc<FontRasterizer>>>,
    control_font: OnceLock<Option<fontdue::Font>>,
}

impl NamedFace {
    fn get(&self) -> Option<&FontRasterizer> {
        self.font
            .get_or_init(|| {
                let font = match self.source.as_ref() {
                    FontSource::Bytes(bytes) => FontRasterizer::from_bytes(bytes, self.index).ok()?,
                    FontSource::File(path) => {
                        let bytes = std::fs::read(path).ok()?;
                        FontRasterizer::from_bytes(&bytes, self.index).ok()?
                    }
                };
                Some(Arc::new(font))
            })
            .as_deref()
    }

    fn control_font(&self) -> Option<&fontdue::Font> {
        self.control_font
            .get_or_init(|| {
                let bytes = match self.source.as_ref() {
                    FontSource::Bytes(bytes) => bytes.as_ref().clone(),
                    FontSource::File(path) => std::fs::read(path).ok()?,
                };
                fontdue::Font::from_bytes(
                    bytes,
                    fontdue::FontSettings {
                        collection_index: self.index,
                        load_substitutions: false,
                        ..Default::default()
                    },
                )
                .ok()
            })
            .as_ref()
    }
}

impl Default for DesktopMessageDevice {
    fn default() -> Self {
        Self {
            fonts: Arc::default(),
            named_fonts: BTreeMap::new(),
            resource_font_files: BTreeMap::new(),
            fallback_faces: Vec::new(),
            platform_substitutions: RefCell::new(BTreeSet::new()),
            fonts_loaded: false,
            surfaces: BTreeMap::new(),
            pixel_less_message: None,
            time_scale: None,
            drawing_enabled: true,
            settings_loaded: false,
            ruby_font_name: Vec::new(),
            raw_font_controls: None,
            cursor_visible: true,
            cursor_visibility_request: None,
        }
    }
}

#[derive(Debug)]
struct MessageSurface {
    generation: u64,
    visible: Canvas,
    source: Option<Canvas>,
    /// Drawing-context texture: changes only after real full/dirty copies.
    submitted: Canvas,
    dirty: Option<Rect>,
    /// Local ink bounds and reveal starts survive clock reversal and failures.
    glyphs: BTreeMap<usize, RevealGlyph>,
    restore_pending: bool,
}

#[derive(Clone, Copy, Debug)]
struct RevealGlyph {
    position: [i32; 2],
    bounds: [i32; 4],
    start: i32,
}

pub(super) struct MessageSurfaceView<'a> {
    pub(super) generation: u64,
    pub(super) visible: &'a Canvas,
    pub(super) source: Option<&'a Canvas>,
    pub(super) submitted: &'a Canvas,
    pub(super) dirty: Option<Rect>,
}

impl DesktopMessageDevice {
    /// Commit E's already validated, negative-ID FES staging surfaces. Keep
    /// the live font device/cache and ordinary nonnegative message surfaces.
    pub(crate) fn commit_fes_surfaces(&mut self, mut prepared: Self) {
        self.surfaces.retain(|id, _| *id >= 0);
        self.surfaces.append(&mut prepared.surfaces);
    }

    /// Real retained state for the software input device. Native presenters
    /// consume this device's latest visibility request before presenting.
    pub(super) fn show_device_cursor(&mut self, requested: u32) {
        self.cursor_visible = requested != 0;
        self.cursor_visibility_request = Some(self.cursor_visible);
    }

    pub(super) fn device_cursor_visible(&self) -> u32 {
        u32::from(self.cursor_visible)
    }

    pub(super) fn take_cursor_visibility_request(&mut self) -> Option<bool> {
        self.cursor_visibility_request.take()
    }

    pub(super) fn legacy_mask(
        &self,
        configuration: &MessageConfiguration,
        character: char,
        size: u32,
    ) -> Result<GlyphMask> {
        self.mask(
            configuration,
            character,
            size,
            configuration.font_weight() == 700,
            false,
        )
    }

    pub(super) fn load_fonts(&mut self, resources: &ResourceIndex) {
        if !self.fonts_loaded {
            self.initialize_fonts(resources, super::load_font(resources).into());
        }
    }

    pub(super) fn initialize_fonts(&mut self, resources: &ResourceIndex, fonts: Arc<[LoadedFont]>) {
        if !self.fonts_loaded {
            self.fonts = fonts;
            self.load_font_names(resources);
            self.load_game_font_files(resources);
            self.fonts_loaded = true;
        }
    }

    /// Explicit face aliases are case-insensitive and remain local to this
    /// device. The caller can use a game's font setting or SFNT metadata.
    pub(super) fn bind_font(&mut self, name: &str, bytes: &[u8], face: u32) -> Result<()> {
        let font = FontRasterizer::from_bytes(bytes, face)
            .map_err(|_| invalid("message named font could not be decoded"))?;
        let cached = OnceLock::new();
        let _ = cached.set(Some(Arc::new(font)));
        let offset = if bytes.starts_with(b"ttcf") {
            be32(bytes, 12 + face as usize * 4).unwrap_or(0) as usize
        } else {
            0
        };
        let names = sfnt_names(bytes, offset);
        let face_name = names
            .iter()
            .find(|name| name.is_ascii())
            .or_else(|| names.iter().next())
            .cloned()
            .unwrap_or_else(|| String::from("unnamed SFNT face"));
        let face = Arc::new(NamedFace {
            source: Arc::new(FontSource::Bytes(Arc::new(bytes.to_vec()))),
            index: face,
            weight: sfnt_weight(bytes, offset).unwrap_or(400),
            italic: sfnt_italic(bytes, offset),
            last_resort: sfnt_last_resort(bytes, offset),
            private: true,
            face_name,
            cjk: names
                .iter()
                .any(|name| name.to_ascii_lowercase().contains("cjk")),
            serif: sfnt_serif(bytes, offset),
            monospace: sfnt_monospace(bytes, offset),
            font: cached,
            control_font: OnceLock::new(),
        });
        self.named_fonts
            .entry(name.to_lowercase())
            .or_default()
            .push(face.clone());
        self.fallback_faces.push(face);
        Ok(())
    }

    pub(super) fn has_font_name(&self, name: &str) -> bool {
        self.named_fonts.contains_key(&name.to_lowercase())
    }
    pub(super) fn settings_font_faces(&self) -> Vec<(String, bool)> {
        #[cfg(windows)]
        let candidates = installed_settings_fonts();
        #[cfg(not(windows))]
        let candidates: Vec<_> = self.fallback_faces.iter().filter(|face| !face.last_resort && !face.italic)
            .filter(|face| face.get().is_some_and(|font| "ABC\u{3042}\u{3044}\u{3046}\u{3048}\u{304a}".chars().all(|c| font.has_character(c))))
            .map(|face| (face.face_name.clone(), face.monospace)).collect();
        let mut seen = BTreeSet::new();
        candidates.into_iter().filter(|(name, _)| !name.starts_with('@') && name.encode_utf16().count() <= 63
            && self.has_font_name(name) && seen.insert(name.to_lowercase())).take(4096).collect()
    }
    pub(super) fn settings_font_preview(&self, name: &str, bold: bool, sample: &str) -> Result<Canvas> {
        if sample.chars().count() > 512 { return Err(invalid("font preview text exceeds limit")); }
        let configuration = cs2_runtime::message_object::MessageDefaults {
            font_name: name.encode_utf16().collect(), bold, shadow: 0, outline: 0,
            glyph_parameter: Some(128), gradual: false,
        }.configuration(MessageEncoding::Unicode)?;
        let mut canvas = Canvas::new(640, 48).map_err(|_| invalid("font preview allocation failed"))?;
        canvas.clear(Color::WHITE);
        let mut x = 4i32;
        for character in sample.chars() {
            let glyph = self.mask(&configuration, character, 32, bold, false)?;
            canvas.draw_glyph(&glyph, Point::new(x, 4), Color::BLACK, 255, None, None)
                .map_err(|_| invalid("font preview rasterization failed"))?;
            x = x.saturating_add(glyph.advance());
            if x >= 640 { break; }
        }
        Ok(canvas)
    }
    /// [outline, shadow, bold, proportional, fixed], before conversion into
    /// MessageDefaults booleans/glyph parameters.
    pub(super) fn raw_font_controls(&self) -> Option<[u32; 5]> {
        self.raw_font_controls
    }
    /// Configuration adapters that already update defaults/notifications also
    /// publish the same raw values here for subsequent XML/FES reads.
    pub(super) fn set_raw_font_controls(&mut self, controls: [u32; 5]) {
        self.raw_font_controls = Some(controls);
    }

    pub(crate) fn restore_raw_font_controls(&mut self, controls: Option<[u32; 5]>) {
        self.raw_font_controls = controls;
    }
    pub(super) fn font_substitutions(&self) -> Vec<MessageFontSubstitution> {
        self.platform_substitutions
            .borrow()
            .iter()
            .cloned()
            .collect()
    }

    fn load_game_font_files(&mut self, resources: &ResourceIndex) {
        for file in resources.files() {
            let relative = file.relative_path.to_string_lossy().replace('\\', "/");
            let lower = relative.to_ascii_lowercase();
            let indexed_path = resources.root().join(&file.relative_path);
            for (offset, _) in lower.match_indices("font/") {
                if offset != 0 && lower.as_bytes()[offset - 1] != b'/' {
                    continue;
                }
                let Ok(Some(candidate)) = resources.resolve_plain_file(&relative[offset..]) else {
                    continue;
                };
                if let ResourceLocation::File { path } = &candidate.location
                    && *path == indexed_path
                {
                    self.resource_font_files.insert(
                        lower[offset + "font/".len()..].to_owned(),
                        GameFontFile {
                            path: path.clone(),
                            face: OnceLock::new(),
                        },
                    );
                    break;
                }
            }
        }
    }

    fn load_font_names(&mut self, resources: &ResourceIndex) {
        let names: BTreeSet<_> = resources
            .files()
            .iter()
            .map(|file| file.relative_path.to_string_lossy().replace('\\', "/"))
            .chain(resources.archives().iter().flat_map(|archive| {
                archive
                    .entries
                    .iter()
                    .map(|entry| entry.name.replace('\\', "/"))
            }))
            .filter(|name| {
                std::path::Path::new(name)
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| {
                        ["ttf", "ttc", "otf"]
                            .iter()
                            .any(|known| extension.eq_ignore_ascii_case(known))
                    })
            })
            .collect();
        for name in names {
            if let Ok(bytes) = super::read_entry_with_fallback(resources, &name) {
                let bytes = Arc::new(bytes);
                let source = Arc::new(FontSource::Bytes(Arc::clone(&bytes)));
                self.register_sfnt(&bytes, source, &FontOrigin::Resource(name));
            }
        }
        #[cfg(windows)]
        {
            // Enumerating actual installed font files also finds localized
            // family/full/PostScript names. SFNT names are authoritative; a
            // filename or registry display label is never used as a face alias.
            let windows = std::env::var_os("WINDIR")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| std::path::PathBuf::from(r"C:\Windows"));
            let mut folders = vec![windows.join("Fonts")];
            if let Some(local) = std::env::var_os("LOCALAPPDATA") {
                folders.push(std::path::PathBuf::from(local).join("Microsoft/Windows/Fonts"));
            }
            for folder in folders {
                if let Ok(files) = std::fs::read_dir(folder) {
                    let mut paths: Vec<_> = files
                        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
                        .collect();
                    paths.sort();
                    for path in paths {
                        if path
                            .extension()
                            .and_then(|extension| extension.to_str())
                            .is_some_and(|extension| {
                                ["ttf", "ttc", "otf"]
                                    .iter()
                                    .any(|known| extension.eq_ignore_ascii_case(known))
                            })
                        {
                            if let Ok(bytes) = std::fs::read(&path) {
                                self.register_system_sfnt(&bytes, path);
                            }
                        }
                    }
                }
            }
        }
        #[cfg(target_os = "android")]
        {
            // Use the same discovered real files as the plain-text backend,
            // but register every SFNT collection member and style lazily.
            for path in super::text::android_font_paths() {
                if let Ok(bytes) = std::fs::read(&path) {
                    self.register_system_sfnt(&bytes, path);
                }
            }
        }
    }

    #[cfg(any(windows, target_os = "android"))]
    fn register_system_sfnt(&mut self, bytes: &[u8], path: PathBuf) {
        let origin = FontOrigin::System(path.clone());
        self.register_sfnt(bytes, Arc::new(FontSource::File(path)), &origin);
    }

    fn register_sfnt(&mut self, bytes: &[u8], source: Arc<FontSource>, origin: &FontOrigin) {
        if bytes.len() > MAX_SURFACE_BYTES {
            return;
        }
        let count = if bytes.starts_with(b"ttcf") {
            be32(bytes, 8).unwrap_or(0).min(64)
        } else {
            1
        };
        for face in 0..count {
            let offset = if bytes.starts_with(b"ttcf") {
                let Some(offset) = be32(bytes, 12 + face as usize * 4) else {
                    continue;
                };
                offset as usize
            } else {
                0
            };
            let aliases = sfnt_names(bytes, offset);
            if aliases.is_empty() {
                continue;
            }
            let weight = sfnt_weight(bytes, offset).unwrap_or(400);
            let face_name = aliases
                .iter()
                .find(|name| name.is_ascii())
                .or_else(|| aliases.iter().next())
                .cloned()
                .unwrap();
            let cjk = aliases.iter().any(|name| {
                let name = name.to_ascii_lowercase();
                name.contains("cjk")
                    || name.contains("sansjp")
                    || name.contains("serifjp")
                    || name.contains("sanssc")
                    || name.contains("serifsc")
                    || name.contains("sanstc")
                    || name.contains("seriftc")
                    || name.contains("microsoft yahei")
                    || name.contains("simsun")
                    || name.contains("simhei")
                    || name.contains("droidsansfallback")
            });
            let serif = sfnt_serif(bytes, offset)
                || aliases.iter().any(|name| {
                    let name = name.to_lowercase();
                    name.contains("mincho")
                        || name.contains("明朝")
                        || name.contains("serif") && !name.contains("sans")
                });
            let cached = OnceLock::new();
            if let Some(loaded) = self.fonts.iter().find(|loaded| {
                loaded.origin.as_ref() == origin && loaded.index == face
            }) {
                let _ = cached.set(Some(Arc::clone(&loaded.rasterizer)));
            }
            let font = Arc::new(NamedFace {
                source: source.clone(),
                index: face,
                weight,
                italic: sfnt_italic(bytes, offset),
                last_resort: sfnt_last_resort(bytes, offset),
                private: matches!(source.as_ref(), FontSource::Bytes(_)),
                face_name,
                cjk,
                serif,
                monospace: sfnt_monospace(bytes, offset),
                font: cached,
                control_font: OnceLock::new(),
            });
            self.fallback_faces.push(font.clone());
            for name in aliases {
                let fonts = self.named_fonts.entry(name.to_lowercase()).or_default();
                // Keep every real style in discovery order; selection sorts
                // private resources and requested style without losing faces.
                fonts.push(font.clone());
            }
        }
    }

    pub(super) fn surface(&self, id: i32) -> Option<MessageSurfaceView<'_>> {
        self.surfaces.get(&id).map(|surface| MessageSurfaceView {
            generation: surface.generation,
            visible: &surface.visible,
            source: surface.source.as_ref(),
            submitted: &surface.submitted,
            dirty: surface.dirty,
        })
    }

    pub(super) fn snapshot_pixels(&self, id: i32) -> Option<MessagePixelSnapshot> {
        self.surfaces.get(&id).map(|surface| MessagePixelSnapshot {
            version: 1,
            object_id: id,
            surface_generation: surface.generation,
            size: [surface.visible.width(), surface.visible.height()],
            visible_rgba: surface.visible.pixels().to_vec(),
            source_rgba: surface
                .source
                .as_ref()
                .map(|source| source.pixels().to_vec()),
            submitted_rgba: surface.submitted.pixels().to_vec(),
            dirty: surface
                .dirty
                .map(|rect| [rect.x, rect.y, rect.width as i32, rect.height as i32]),
            glyphs: surface
                .glyphs
                .iter()
                .map(|(&index, glyph)| (index, glyph.position, glyph.bounds, glyph.start))
                .collect(),
        })
    }

    pub(super) fn restore_pixels(
        &mut self,
        object: &MessageObject,
        snapshot: MessagePixelSnapshot,
    ) -> Result<()> {
        let configuration = object
            .configuration()
            .ok_or_else(|| invalid("message pixel restore requires a configured object"))?;
        if snapshot.version != 1
            || snapshot.object_id != object.id()
            || snapshot.size != configuration.size()
            || snapshot.source_rgba.is_some() != (configuration.reveal_width() != 0)
        {
            return Err(invalid(
                "message pixel snapshot identity, size, or mode differs",
            ));
        }
        let [width, height] = snapshot.size;
        let mut glyphs = BTreeMap::new();
        for (index, position, bounds, start) in snapshot.glyphs {
            if index >= object.glyph_states().len() || glyphs.contains_key(&index) {
                return Err(invalid(
                    "message pixel snapshot glyph index is invalid or repeated",
                ));
            }
            glyphs.insert(
                index,
                RevealGlyph {
                    position,
                    bounds,
                    start,
                },
            );
        }
        if object
            .glyph_states()
            .iter()
            .enumerate()
            .any(|(index, state)| {
                *state == MessageGlyphState::Revealing && !glyphs.contains_key(&index)
            })
        {
            return Err(invalid(
                "message pixel snapshot omitted a revealing glyph's bounds",
            ));
        }
        let dirty = snapshot
            .dirty
            .map(|bounds| {
                if bounds[0] < 0
                    || bounds[1] < 0
                    || bounds[2] <= 0
                    || bounds[3] <= 0
                    || i64::from(bounds[0]) + i64::from(bounds[2]) > i64::from(width)
                    || i64::from(bounds[1]) + i64::from(bounds[3]) > i64::from(height)
                {
                    return Err(invalid("message pixel snapshot dirty rectangle is invalid"));
                }
                Ok(Rect::new(
                    bounds[0],
                    bounds[1],
                    bounds[2] as u32,
                    bounds[3] as u32,
                ))
            })
            .transpose()?;
        self.ensure_surface(object, configuration)?;
        let canvas = |pixels| {
            Canvas::from_rgba8(width, height, pixels)
                .map_err(|_| invalid("message pixel snapshot RGBA capacity is invalid"))
        };
        let surface = MessageSurface {
            generation: object.surface_generation(),
            visible: canvas(snapshot.visible_rgba)?,
            source: snapshot.source_rgba.map(canvas).transpose()?,
            submitted: canvas(snapshot.submitted_rgba)?,
            dirty,
            glyphs,
            restore_pending: false,
        };
        self.surfaces.insert(object.id(), surface);
        Ok(())
    }

    pub(super) fn release(&mut self, id: i32) {
        self.surfaces.remove(&id);
    }

    pub(super) fn without_message_pixels<T>(
        &mut self,
        id: i32,
        update: impl FnOnce(&mut Self) -> T,
    ) -> T {
        let previous = self.pixel_less_message.replace(id);
        self.release(id);
        let result = update(self);
        self.pixel_less_message = previous;
        result
    }

    /// Clear/recreate also affect inactive objects, so frame/render integration
    /// must synchronize every retained object, not only active playback.
    pub(super) fn sync_object(&mut self, object: &MessageObject) -> Result<()> {
        let Some(configuration) = object.configuration() else {
            self.release(object.id());
            return Ok(());
        };
        self.ensure_surface(object, configuration)?;
        #[cfg(not(target_os = "android"))]
        if object.active() && object.reveal_trace_enabled() && !object.source().is_empty() {
            eprintln!(
                "message-reveal sync id={} gen={} playback={}",
                object.id(), object.surface_generation(), object.playback_time(),
            );
        }
        Ok(())
    }

    fn ensure_surface(
        &mut self,
        object: &MessageObject,
        configuration: &MessageConfiguration,
    ) -> Result<()> {
        if self
            .surfaces
            .get(&object.id())
            .is_some_and(|surface| surface.generation == object.surface_generation())
        {
            return Ok(());
        }
        if object.surface_change() == MessageSurfaceChange::Clear {
            if let Some(surface) = self.surfaces.get_mut(&object.id()) {
                surface.visible.clear(Color::rgba(0, 0, 0, 0));
                if let Some(source) = &mut surface.source {
                    source.clear(Color::rgba(0, 0, 0, 0));
                }
                surface.glyphs.clear();
                surface.restore_pending = false;
                surface.generation = object.surface_generation();
                // clear() neither creates nor discards a dirty rectangle.
                // A drawing-context texture changes only on actual submission.
                return Ok(());
            }
        }
        let [width, height] = configuration.size();
        if width == 0 || height == 0 {
            return Err(message_surface_error(
                width,
                height,
                RendererError::InvalidDimensions { width, height },
            ));
        }
        let length = (width as usize)
            .checked_mul(height as usize)
            .and_then(|n| n.checked_mul(4))
            .ok_or_else(|| message_surface_error(width, height, RendererError::ScaledDimensionsOverflow))?;
        if length > MAX_SURFACE_BYTES {
            return Err(invalid("message surface exceeds 64 MiB"));
        }
        let copies = if configuration.reveal_width() == 0 {
            2
        } else {
            3
        };
        let other = self
            .surfaces
            .iter()
            .filter(|(id, _)| **id != object.id())
            .map(|(_, surface)| {
                surface.visible.pixels().len()
                    + surface.submitted.pixels().len()
                    + surface
                        .source
                        .as_ref()
                        .map_or(0, |source| source.pixels().len())
            })
            .sum::<usize>();
        if length.saturating_mul(copies).saturating_add(other) > MAX_DEVICE_BYTES {
            return Err(invalid("message device surfaces exceed 256 MiB"));
        }
        let canvas = || {
            Canvas::new(width, height).map_err(|error| message_surface_error(width, height, error))
        };
        let source = if configuration.reveal_width() != 0 {
            Some(canvas()?)
        } else {
            None
        };
        self.surfaces.insert(
            object.id(),
            MessageSurface {
                generation: object.surface_generation(),
                visible: canvas()?,
                source,
                submitted: canvas()?,
                dirty: None,
                glyphs: BTreeMap::new(),
                restore_pending: object
                    .glyph_states()
                    .iter()
                    .any(|state| *state != MessageGlyphState::Hidden),
            },
        );
        Ok(())
    }

    fn font(
        &self,
        configuration: &MessageConfiguration,
        character: char,
        bold: bool,
        italic: bool,
        override_name: Option<&[u16]>,
    ) -> Result<(&FontRasterizer, bool, bool)> {
        if configuration.font_backend() != 0 {
            let units = override_name.map_or_else(|| configuration.font_name(), |name| name.to_vec());
            let name = String::from_utf16(&units)
                .map_err(|_| invalid("message font filename has an isolated UTF-16 surrogate"))?;
            let key = name.replace('\\', "/").to_ascii_lowercase();
            let file = self.resource_font_files.get(&key).ok_or_else(|| ScriptError::HostCallFailed {
                message: format!("message font resource font/{name} not found"),
            })?;
            let face = file.face.get_or_init(|| file.load()).as_ref().map_err(|error| {
                ScriptError::HostCallFailed { message: format!("message font resource font/{name} {error}") }
            })?;
            if !face.font.has_character(character) {
                return Err(ScriptError::HostCallFailed {
                    message: format!("message font resource font/{name} could not supply U+{:04X}", character as u32),
                });
            }
            return Ok((&face.font, face.bold, face.italic));
        }
        // The extra DWORD is a Windows character-set selector in its LOW
        // byte, not a glyph-size multiplier or a strict zero/one flag.
        // Text is already Unicode; recognized selectors affect font selection
        // only and never reinterpret a decoded character's bytes.
        if !matches!(
            configuration.glyph_parameter() as u8,
            0 | 1
                | 2
                | 77
                | 128
                | 129
                | 130
                | 134
                | 136
                | 161
                | 162
                | 163
                | 177
                | 178
                | 186
                | 204
                | 222
                | 238
                | 255
        ) {
            return Err(invalid(
                "message font extra parameter contains an unknown Windows character set",
            ));
        }
        let units = override_name.map_or_else(|| configuration.font_name(), |name| name.to_vec());
        if units.is_empty() {
            let requested_weight = if bold { 700 } else { 400 };
            let mut candidates: Vec<_> = self
                .fallback_faces
                .iter()
                .filter(|face| !face.last_resort)
                .collect();
            candidates.sort_by_key(|face| {
                (
                    !face.private,
                    face.italic != italic,
                    (i32::from(face.weight) - requested_weight).abs(),
                )
            });
            for face in candidates {
                if let Some(font) = face.get().filter(|font| font.has_character(character)) {
                    return Ok((font, face.weight == 700, face.italic));
                }
            }
            // Font data without usable SFNT names can still be a valid
            // explicitly loaded glyph source.
            return self
                .fonts
                .iter()
                .filter(|_| self.fallback_faces.is_empty())
                .map(|face| face.rasterizer.as_ref())
                .find(|font| font.has_character(character))
                .map(|font| (font, false, false))
                .ok_or_else(|| ScriptError::HostCallFailed {
                    message: format!("no available message font covers U+{:04X} for the default family", character as u32),
                });
        }
        let name = String::from_utf16(&units)
            .map_err(|_| invalid("message font name has an isolated UTF-16 surrogate"))?;
        if name.starts_with('@') {
            return Err(invalid("vertical message font is unsupported"));
        }
        let fonts = self.named_fonts.get(&name.to_lowercase());
        let mut candidates: Vec<_> = fonts.into_iter().flatten().collect();
        let requested_weight = if bold { 700 } else { 400 };
        candidates.sort_by_key(|face| {
            (
                !face.private,
                face.italic != italic,
                (i32::from(face.weight) - requested_weight).abs(),
            )
        });
        for face in candidates {
            if let Some(font) = face.get().filter(|font| font.has_character(character)) {
                return Ok((font, face.weight == 700, face.italic));
            }
        }
        #[cfg(any(windows, target_os = "android"))]
        {
            return self.platform_substitute(&units, configuration, character, bold, italic);
        }
        #[cfg(not(any(windows, target_os = "android")))]
        Err(ScriptError::HostCallFailed {
            message: format!("no configured message font covers U+{:04X} for {name:?}", character as u32),
        })
    }

    #[cfg(any(windows, target_os = "android"))]
    fn platform_substitute(
        &self,
        requested: &[u16],
        configuration: &MessageConfiguration,
        character: char,
        bold: bool,
        italic: bool,
    ) -> Result<(&FontRasterizer, bool, bool)> {
        let requested_name = String::from_utf16_lossy(requested).to_lowercase();
        let serif = requested_name.contains("mincho")
            || requested_name.contains("明朝")
            || requested_name.contains("serif") && !requested_name.contains("sans");
        let mono = requested_name.contains("mono")
            || requested_name.contains("ＭＳ ゴシック".to_lowercase().as_str())
            || requested_name == "ms gothic";
        let cjk = is_east_asian_message_character(character);
        let japanese = configuration.glyph_parameter() as u8 == 128
            || requested_name.contains("gothic")
            || requested_name.contains("mincho")
            || requested_name.contains("ゴシック")
            || requested_name.contains("明朝");
        let weight = if bold { 700 } else { 400 };
        let mut candidates: Vec<_> = self
            .fallback_faces
            .iter()
            .filter(|face| !face.last_resort)
            .collect();
        candidates.sort_by_key(|face| {
            let family = face.face_name.to_ascii_lowercase();
            let japanese_face = family.contains("cjkjp")
                || family.contains("cjk jp")
                || family.contains("sansjp")
                || family.contains("serifjp");
            (
                !face.private,
                face.cjk != cjk,
                face.serif != serif,
                mono && !face.monospace,
                face.italic != italic,
                (i32::from(face.weight) - weight).abs(),
                cjk && japanese && !japanese_face,
            )
        });
        for face in candidates {
            let Some(font) = face.get().filter(|font| font.has_character(character)) else {
                continue;
            };
            let source = match face.source.as_ref() {
                FontSource::File(path) => path.to_string_lossy().into_owned(),
                FontSource::Bytes(_) => String::from("private SFNT bytes"),
            };
            let substitution = MessageFontSubstitution {
                requested_name: requested.to_vec(),
                selected_name: face.face_name.clone(),
                source,
                collection_index: face.index,
                weight: face.weight,
                italic: face.italic,
                synthesized_bold: bold && face.weight != 700,
                synthesized_italic: italic && !face.italic,
            };
            let mut substitutions = self.platform_substitutions.borrow_mut();
            if substitutions.len() < 256 {
                substitutions.insert(substitution);
            }
            return Ok((font, face.weight == 700, face.italic));
        }
        Err(ScriptError::HostCallFailed {
            message: format!("no real message font covers U+{:04X} for {requested_name:?}", character as u32),
        })
    }

    fn measure(
        &self,
        configuration: &MessageConfiguration,
        character: char,
        size: u32,
    ) -> Result<i32> {
        if size == 0 {
            return Ok(0);
        }
        if size > i32::MAX as u32 {
            return Err(invalid("message font size exceeds i32"));
        }
        let bold = configuration.font_weight() == 700;
        let (font, selected_bold, _) = self.font(configuration, character, bold, false, None)?;
        let measured = font
            .measure_styled(character, size, bold && !selected_bold, false)
            .map_err(|_| invalid("message glyph measurement failed"))?;
        if configuration.measure_parameter() == 0 {
            return Ok(measured);
        }
        let mut buffer = [0; 4];
        let (bytes, _, errors) = encoding_rs::SHIFT_JIS.encode(character.encode_utf8(&mut buffer));
        let fixed = if !errors && bytes.len() == 2 {
            size
        } else {
            size.div_ceil(2)
        };
        Ok(measured.max(fixed as i32))
    }

    fn measure_inline_image(&self, configuration: &MessageConfiguration, size: u32) -> Result<i32> {
        if size == 0 {
            return Ok(0);
        }
        if size > i32::MAX as u32 {
            return Err(invalid("inline image nominal text size exceeds i32"));
        }
        let bold = configuration.font_weight() == 700;
        // Original ui measures code ten through the ordinary selected font,
        // independently of the decoded image's width. SFNT's actual cmap (or
        // glyph zero) supplies this metric even when LF has no visible glyph.
        let (font, selected_bold, _) = self.font(configuration, ' ', bold, false, None)?;
        let measured = if font.has_character('\n') {
            font.measure_styled('\n', size, bold && !selected_bold, false)
                .map_err(|_| invalid("inline image control metric failed"))?
        } else {
            let face = self
                .fallback_faces
                .iter()
                .find(|face| {
                    face.font
                        .get()
                        .and_then(Option::as_deref)
                        .is_some_and(|loaded| std::ptr::eq(loaded, font))
                })
                .ok_or_else(|| invalid("inline image selected font has no retained SFNT source"))?;
            let font = face
                .control_font()
                .ok_or_else(|| invalid("inline image SFNT metric load failed"))?;
            let advance = font.metrics('\n', size as f32).advance_width.round();
            if !advance.is_finite()
                || f64::from(advance) > f64::from(i32::MAX)
                || advance < i32::MIN as f32
            {
                return Err(invalid("inline image font metric is outside i32"));
            }
            (advance as i32).saturating_add((bold && !selected_bold) as i32)
        };
        Ok(if configuration.measure_parameter() == 0 {
            measured
        } else {
            measured.max(size.div_ceil(2) as i32)
        })
    }

    fn mask(
        &self,
        configuration: &MessageConfiguration,
        character: char,
        size: u32,
        bold: bool,
        italic: bool,
    ) -> Result<GlyphMask> {
        self.mask_with_name(configuration, character, size, bold, italic, None)
    }

    fn mask_with_name(
        &self,
        configuration: &MessageConfiguration,
        character: char,
        size: u32,
        bold: bool,
        italic: bool,
        override_name: Option<&[u16]>,
    ) -> Result<GlyphMask> {
        if size == 0 {
            return GlyphMask::from_alpha_with_metrics(0, 0, Vec::new(), GlyphMetrics::default())
                .map_err(|_| invalid("empty message glyph construction failed"));
        }
        let (font, selected_bold, selected_italic) =
            self.font(configuration, character, bold, italic, override_name)?;
        let glyph = font
            .rasterize_styled(
                character,
                size,
                bold && !selected_bold,
                italic && !selected_italic,
            )
            .map_err(|_| invalid("message font rasterization failed"))?;
        if configuration.font_weight() != 700 {
            return Ok(glyph);
        }
        let alpha = glyph
            .alpha()
            .iter()
            .map(|&alpha| alpha.saturating_mul(2))
            .collect();
        GlyphMask::from_alpha_with_metrics(glyph.width(), glyph.height(), alpha, glyph.metrics())
            .map_err(|_| invalid("message doubled coverage construction failed"))
    }

    fn paint_entry(
        &self,
        target: &mut Canvas,
        request: &MessageRenderRequest<'_>,
        index: usize,
    ) -> Result<[i32; 4]> {
        let entry = &request.layout.entries[index];
        let atom = entry
            .atom
            .and_then(|index| request.object.atoms().get(index))
            .ok_or_else(|| invalid("message painter entry has no source atom"))?;
        if let Some(frame_id) = atom.inline_image {
            let Some(source) = request.object.inline_image_source() else {
                return Ok([0; 4]);
            };
            let Some(frame) = decode_inline_image(source, frame_id)? else {
                return Ok([0; 4]);
            };
            let width = i32::try_from(frame.width())
                .map_err(|_| invalid("inline image width exceeds i32"))?;
            let height = i32::try_from(frame.height())
                .map_err(|_| invalid("inline image height exceeds i32"))?;
            let [x, y] = entry.position;
            target
                .draw_frame(&frame, &ImageDrawOptions::new(Point::new(x, y)))
                .map_err(|_| invalid("inline message image pixel composition failed"))?;
            return Ok([0, 0, width, height]);
        }
        let MessageAtomKind::Glyph {
            character,
            encoding,
            style,
            ruby,
            ..
        } = &atom.kind
        else {
            return Err(invalid(
                "message painter was asked to rasterize a control record",
            ));
        };
        // Off-surface records still become complete, without querying fonts.
        let [x, y] = entry.position;
        if i64::from(x) + 128 <= 0
            || i64::from(y) + 128 <= 0
            || i64::from(x) >= i64::from(target.width())
            || i64::from(y) >= i64::from(target.height())
        {
            return Ok([0; 4]);
        }
        let size = if *ruby {
            style.ruby_size
        } else {
            style.text_size
        };
        let tail = if request.configuration.encoding() == MessageEncoding::Cp932 {
            0xc4
        } else {
            0x104
        };
        let effects = GlyphEffects {
            outline: request.configuration.word(tail + 8)? != 0,
            shadow: request.configuration.word(tail + 4)? != 0,
            shadow_offset: [1, 1],
        };
        match request.object.user_glyph(*character, *encoding) {
            Some(None) => Ok([0; 4]),
            Some(Some(mapped)) => {
                paint_message_user_glyph(target, mapped, [x, y], size, style.shadow_color, effects)
            }
            None => {
                let name = request.object.atom_font_name(entry.atom.unwrap());
                let glyph = self.mask_with_name(
                    request.configuration,
                    *character,
                    size,
                    style.bold,
                    style.italic,
                    (!name.is_empty()).then_some(name),
                )?;
                let color = if *ruby {
                    style.ruby_color
                } else {
                    style.text_color
                };
                paint_message_mask(
                    target,
                    &glyph,
                    [x, y],
                    size,
                    color,
                    style.shadow_color,
                    effects,
                )
            }
        }
    }
}

impl MessageBackend for DesktopMessageDevice {
    fn layout(
        &mut self,
        configuration: &MessageConfiguration,
        atoms: &[MessageAtom],
        previous: Option<&MessageLayout>,
    ) -> Result<MessageLayout> {
        layout_atoms(
            self,
            configuration,
            atoms,
            previous,
            &MessageBreakCharacters::for_encoding(configuration.encoding()),
        )
    }

    fn layout_with_break_characters(
        &mut self,
        configuration: &MessageConfiguration,
        atoms: &[MessageAtom],
        previous: Option<&MessageLayout>,
        characters: &MessageBreakCharacters,
    ) -> Result<MessageLayout> {
        layout_atoms(self, configuration, atoms, previous, characters)
    }

    fn redraw_message(&mut self, object: &MessageObject) -> Result<bool> {
        let Some(configuration) = object.configuration() else {
            return Ok(false);
        };
        self.ensure_surface(object, configuration)?;
        let mut surface = self
            .surfaces
            .remove(&object.id())
            .ok_or_else(|| invalid("message redraw has no allocated surface"))?;
        surface.visible.clear(Color::rgba(0, 0, 0, 0));
        if let Some(source) = &mut surface.source {
            source.clear(Color::rgba(0, 0, 0, 0));
        }
        let empty = MessageLayout::default();
        let layout = object.layout().unwrap_or(&empty);
        let request = MessageRenderRequest {
            object,
            configuration,
            layout,
            prepare: &[],
            process: &[],
            reveal_phase: None,
            force: false,
            copy: MessageCopy::Full,
            surface_generation: object.surface_generation(),
        };
        let result = (|| {
            for (index, state) in object.glyph_states().iter().enumerate() {
                if *state == MessageGlyphState::Complete {
                    self.paint_entry(&mut surface.visible, &request, index)?;
                }
            }
            surface.dirty = Some(Rect::new(
                0,
                0,
                surface.visible.width(),
                surface.visible.height(),
            ));
            let submit = configuration.ordinary_submit() && self.drawing_enabled;
            if submit {
                surface
                    .submitted
                    .pixels_mut()
                    .copy_from_slice(surface.visible.pixels());
                surface.dirty = None;
            }
            surface.restore_pending = false;
            Ok(submit)
        })();
        self.surfaces.insert(object.id(), surface);
        result
    }

    fn clear_message(&mut self, object: &MessageObject) -> Result<()> {
        let Some(configuration) = object.configuration() else {
            return Ok(());
        };
        self.ensure_surface(object, configuration)?;
        let surface = self
            .surfaces
            .get_mut(&object.id())
            .ok_or_else(|| invalid("message clear has no allocated surface"))?;
        surface.visible.clear(Color::rgba(0, 0, 0, 0));
        if let Some(source) = &mut surface.source {
            source.clear(Color::rgba(0, 0, 0, 0));
        }
        if configuration.ordinary_submit() && self.drawing_enabled {
            surface
                .submitted
                .pixels_mut()
                .copy_from_slice(surface.visible.pixels());
        }
        // Full clear helper neither consumes dirty nor refreshes completion.
        surface.glyphs.clear();
        surface.restore_pending = false;
        Ok(())
    }

    fn decode_user_glyph(&mut self, source: &[u8], frame: u32) -> Result<Option<MessageUserGlyph>> {
        decode_user_glyph(source, frame)
    }

    fn paint(&mut self, request: MessageRenderRequest<'_>) -> Result<MessagePaintResult> {
        if self.pixel_less_message == Some(request.object.id())
            && request.configuration.size() == [0, 0]
            && request.reveal_phase.is_none()
        {
            return Ok(MessagePaintResult {
                prepared: request.prepare.to_vec(),
                completed: request.process.to_vec(),
                ..MessagePaintResult::default()
            });
        }
        self.ensure_surface(request.object, request.configuration)?;
        let id = request.object.id();
        // Taking the surface out avoids self-referential mutable/immutable
        // borrows while font lookup and a persistent surface are both needed.
        let mut surface = self
            .surfaces
            .remove(&id)
            .ok_or_else(|| invalid("message surface allocation did not register its result"))?;
        let result = self.paint_surface(&mut surface, &request);
        self.surfaces.insert(id, surface);
        result
    }
}

impl DesktopMessageDevice {
    fn paint_surface(
        &self,
        surface: &mut MessageSurface,
        request: &MessageRenderRequest<'_>,
    ) -> Result<MessagePaintResult> {
        let mut result = MessagePaintResult::default();
        let starts = reveal_starts(request)?;
        if surface.restore_pending {
            // Ordinary completed glyphs are reconstructible from the snapshot.
            // Partial Alpha needs a saved device surface; do not invent it.
            if request
                .object
                .glyph_states()
                .contains(&MessageGlyphState::Revealing)
            {
                return Err(invalid(
                    "restoring a partial gradual message needs its device pixel snapshot",
                ));
            }
            for (index, state) in request.object.glyph_states().iter().enumerate() {
                if *state == MessageGlyphState::Complete {
                    self.paint_entry(&mut surface.visible, request, index)?;
                }
            }
            surface.restore_pending = false;
        }
        for &index in request.prepare {
            let target = surface.source.as_mut().unwrap_or(&mut surface.visible);
            match self.paint_entry(target, request, index) {
                Ok(bounds) => {
                    surface.glyphs.insert(
                        index,
                        RevealGlyph {
                            position: request.layout.entries[index].position,
                            bounds,
                            start: starts[index].unwrap_or(0),
                        },
                    );
                    add_dirty(
                        &mut surface.dirty,
                        clipped_bounds(target, request.layout.entries[index].position, bounds),
                    );
                    result.prepared.push(index);
                    if request.reveal_phase.is_none() {
                        result.completed.push(index);
                    }
                }
                Err(error) => {
                    result.failure = Some(paint_failure(error));
                    break;
                }
            }
        }
        if let Some(source) = &surface.source {
            // Even when a prefix failed, preserve pre-generated RGB for retry.
            for (target, original) in surface
                .visible
                .pixels_mut()
                .chunks_exact_mut(4)
                .zip(source.pixels().chunks_exact(4))
            {
                target[..3].copy_from_slice(&original[..3]);
            }
        }
        if result.failure.is_none() {
            if let (Some(phase), Some(source)) = (request.reveal_phase, &surface.source) {
                let width = request.configuration.reveal_width();
                for &index in request.process {
                    let Some(mut glyph) = surface.glyphs.get(&index).copied() else {
                        return Err(invalid(
                            "gradual message glyph has no persistent raster bounds",
                        ));
                    };
                    // Re-layout retains ink but future reveal uses new positions.
                    glyph.position = request.layout.entries[index].position;
                    glyph.start = starts[index].unwrap_or(glyph.start);
                    surface.glyphs.insert(index, glyph);
                    let (complete, active) = apply_reveal(
                        &mut surface.visible,
                        source,
                        glyph,
                        phase,
                        width,
                        request.force,
                    );
                    if active {
                        add_dirty(
                            &mut surface.dirty,
                            clipped_bounds(source, glyph.position, glyph.bounds),
                        );
                    }
                    if complete {
                        result.completed.push(index);
                    }
                }
            }
        }
        #[cfg(not(target_os = "android"))]
        let trace = request.object.reveal_trace_enabled()
            && (!request.process.is_empty() || surface.dirty.is_some());
        #[cfg(not(target_os = "android"))]
        let trace_dirty = trace.then_some(surface.dirty).flatten();
        #[cfg(not(target_os = "android"))]
        let changed = trace_dirty.map(|rect| differing_message_pixels(surface, rect));
        match request.copy {
            MessageCopy::None => {}
            MessageCopy::Full => {
                surface
                    .submitted
                    .pixels_mut()
                    .copy_from_slice(surface.visible.pixels());
                surface.dirty = None;
            }
            MessageCopy::Dirty => {
                if let Some(rectangle) = surface.dirty.take() {
                    copy_rectangle(&mut surface.submitted, &surface.visible, rectangle);
                    result.copied_dirty = true;
                }
            }
        }
        #[cfg(not(target_os = "android"))]
        if trace {
            eprintln!(
                "message-reveal paint id={} gen={} copy={:?} prepared={} completed={} dirty={:?} changed={:?} copied_dirty={} failure={:?}",
                request.object.id(), request.surface_generation, request.copy, result.prepared.len(),
                result.completed.len(), trace_dirty, changed, result.copied_dirty, result.failure,
            );
        }
        Ok(result)
    }
}

#[cfg(not(target_os = "android"))]
fn differing_message_pixels(surface: &MessageSurface, rect: Rect) -> usize {
    let width = surface.visible.width() as usize;
    (rect.y as usize..rect.y as usize + rect.height as usize)
        .map(|row| {
            let start = (row * width + rect.x as usize) * 4;
            let end = start + rect.width as usize * 4;
            surface.visible.pixels()[start..end]
                .chunks_exact(4)
                .zip(surface.submitted.pixels()[start..end].chunks_exact(4))
                .filter(|(visible, submitted)| visible != submitted)
                .count()
        })
        .sum()
}

fn paint_failure(error: ScriptError) -> MessagePaintFailure {
    match error {
        ScriptError::InvalidStructuredText { offset, message } => {
            MessagePaintFailure { offset, message }
        }
        _ => MessagePaintFailure {
            offset: 0,
            message: "message CPU painter failed after a rasterized prefix",
        },
    }
}

fn layout_atoms(
    device: &DesktopMessageDevice,
    configuration: &MessageConfiguration,
    atoms: &[MessageAtom],
    previous: Option<&MessageLayout>,
    characters: &MessageBreakCharacters,
) -> Result<MessageLayout> {
    let width = configuration.size()[0];
    if width > i32::MAX as u32 {
        return Err(invalid("message CPU layout width exceeds i32"));
    }
    if atoms.iter().any(|atom| {
        matches!(
            atom.kind,
            MessageAtomKind::RawX(_) | MessageAtomKind::RawY(_)
        )
    }) {
        return Err(invalid(
            "message raw X/Y record-category layout is not verified for this CPU backend",
        ));
    }
    let left = configuration.word(0x44)? as i32;
    let top = configuration.word(0x48)? as i32;
    let right = (width as i32).wrapping_sub(configuration.word(0x4c)? as i32);
    let spacing = configuration.word(0x54)? as i32;
    let line_spacing = configuration.word(0x58)? as i32;
    let normal = configuration.text_sizes()[2];
    if normal > i32::MAX as u32 {
        return Err(invalid("message normal font size exceeds i32"));
    }
    let mut sizes = Vec::with_capacity(atoms.len());
    for atom in atoms {
        sizes.push(match &atom.kind {
            MessageAtomKind::Glyph {
                character,
                style,
                ruby,
                ..
            } => {
                let size = if *ruby {
                    style.ruby_size
                } else {
                    style.text_size
                };
                let width = if atom.inline_image.is_some() {
                    device.measure_inline_image(configuration, size)?
                } else {
                    device.measure(configuration, *character, size)?
                };
                [
                    width,
                    i32::try_from(size)
                        .map_err(|_| invalid("message layout glyph size exceeds i32"))?,
                ]
            }
            _ => [0; 2],
        });
    }
    // Retain automatic breaks from earlier preparation. A wider later window
    // must not erase those breaks; explicit reconfigure passes no old layout.
    let mut forced = BTreeSet::new();
    if let Some(previous) = previous {
        for (index, entry) in previous.entries.iter().enumerate() {
            if entry.atom.is_none() {
                if let Some(atom) = previous.entries[index + 1..]
                    .iter()
                    .find_map(|entry| entry.atom)
                {
                    forced.insert(atom);
                }
            }
        }
    }
    let mut layout = MessageLayout::default();
    let mut x = left;
    let mut y = top;
    let mut line = 0u32;
    let mut height = 0i32;
    let mut line_start = 0usize;
    let mut alignment = 0u32;
    let mut previous_body = None;
    let mut previous_group = None;
    let breaking = configuration.word(0x6c)? != 0;
    let word_wrap = breaking
        && configuration.encoding() == MessageEncoding::Unicode
        && configuration.word(0x11c)? != 0;
    let hanging = configuration.word(0x70)? != 0;
    let indent_width = if hanging {
        atoms
            .iter()
            .enumerate()
            .find_map(|(index, atom)| match atom.kind {
                MessageAtomKind::Glyph {
                    character,
                    ruby: false,
                    ..
                } => Some(if characters.indent_open.contains(&character) {
                    sizes[index][0].wrapping_add(spacing)
                } else {
                    0
                }),
                _ => None,
            })
            .unwrap_or(0)
    } else {
        0
    };
    let mut indent_active = true;
    let mut line_left = left;
    for (atom_index, atom) in atoms.iter().enumerate() {
        let mut automatic = forced.contains(&atom_index);
        if let MessageAtomKind::Glyph {
            character,
            ruby: false,
            group,
            ..
        } = &atom.kind
        {
            let mut required = sizes[atom_index][0];
            if breaking
                && characters.repeated.contains(character)
                && previous_body != Some(*character)
            {
                let repeated_width: i64 = atoms[atom_index..]
                    .iter()
                    .zip(&sizes[atom_index..])
                    .take_while(|(next, _)| {
                        matches!(&next.kind,
                        MessageAtomKind::Glyph { character: next_character, ruby: false, .. }
                            if next_character == character)
                    })
                    .map(|(_, size)| i64::from(size[0]) + i64::from(spacing))
                    .sum();
                if repeated_width <= i64::from(right) - i64::from(line_left) {
                    required = required
                        .max(repeated_width.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32);
                }
            }
            if word_wrap
                && *character != ' '
                && previous_body.is_none_or(|previous| previous == ' ')
            {
                let mut word_width = 0i64;
                for (next, size) in atoms[atom_index..].iter().zip(&sizes[atom_index..]) {
                    match next.kind {
                        MessageAtomKind::Glyph {
                            character: ' ',
                            ruby: false,
                            ..
                        } => break,
                        MessageAtomKind::Glyph { ruby: false, .. } => {
                            word_width += i64::from(size[0]) + i64::from(spacing);
                        }
                        MessageAtomKind::Glyph { ruby: true, .. } => {}
                        _ => break,
                    }
                }
                if word_width <= i64::from(right) - i64::from(left) - i64::from(indent_width) {
                    required = required
                        .max(word_width.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32);
                }
            }
            if group.is_some() && *group != previous_group {
                let group_width: i64 = atoms[atom_index..]
                    .iter()
                    .zip(&sizes[atom_index..])
                    .take_while(|(atom, _)| {
                        matches!(&atom.kind,
                        MessageAtomKind::Glyph { group: next, .. } if next == group)
                    })
                    .filter(|(atom, _)| {
                        matches!(atom.kind, MessageAtomKind::Glyph { ruby: false, .. })
                    })
                    .map(|(_, size)| i64::from(size[0]) + i64::from(spacing))
                    .sum();
                if breaking && group_width <= i64::from(right) - i64::from(left) {
                    required = required
                        .max(group_width.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32);
                }
            }
            if x != line_left
                && i64::from(x) + i64::from(required) > i64::from(right)
                && !(breaking
                    && (characters.no_start.contains(character)
                        || previous_body
                            .is_some_and(|previous| characters.no_end.contains(&previous))))
            {
                automatic = true;
            }
        }
        if automatic {
            align_line(&mut layout.entries[line_start..], x, right, alignment);
            layout.entries.push(MessageLayoutEntry {
                atom: None,
                position: [0; 2],
                size: [0; 2],
                line,
                display_anchor: None,
            });
            line = line.wrapping_add(1);
            y = y
                .wrapping_add(if height > 0 { height } else { normal as i32 })
                .wrapping_add(line_spacing);
            indent_active = next_indent_state(atoms, atom_index, indent_active, characters);
            line_left = if indent_active {
                left.wrapping_add(indent_width)
            } else {
                left
            };
            x = line_left;
            line_start = layout.entries.len();
            height = 0;
            previous_body = None;
            previous_group = None;
        }
        let mut size = sizes[atom_index];
        if configuration.encoding() == MessageEncoding::Cp932
            && x == line_left
            && y != top
            && matches!(
                atom.kind,
                MessageAtomKind::Glyph {
                    character: '\u{3000}',
                    ruby: false,
                    ..
                }
            )
        {
            size[0] = 0;
        }
        layout.entries.push(MessageLayoutEntry {
            atom: Some(atom_index),
            position: [x, y],
            size,
            line,
            display_anchor: None,
        });
        match &atom.kind {
            MessageAtomKind::Glyph {
                character,
                ruby: false,
                group,
                ..
            } => {
                layout.cursor = [x.wrapping_add(size[0]), y];
                layout.bottom = layout.bottom.max(y.wrapping_add(size[1]));
                x = x.wrapping_add(size[0]).wrapping_add(spacing);
                height = height.max(size[1]);
                previous_body = Some(*character);
                previous_group = *group;
            }
            MessageAtomKind::Newline | MessageAtomKind::Control(8)
                if matches!(atom.kind, MessageAtomKind::Newline)
                    || configuration.encoding() == MessageEncoding::Unicode =>
            {
                align_line(&mut layout.entries[line_start..], x, right, alignment);
                indent_active = next_indent_state(atoms, atom_index + 1, indent_active, characters);
                line_left = if indent_active {
                    left.wrapping_add(indent_width)
                } else {
                    left
                };
                x = line_left;
                y = y
                    .wrapping_add(if height > 0 { height } else { normal as i32 })
                    .wrapping_add(line_spacing);
                line = line.wrapping_add(1);
                height = 0;
                line_start = layout.entries.len();
                previous_body = None;
                previous_group = None;
            }
            MessageAtomKind::Align(next) => alignment = *next,
            MessageAtomKind::Control(6) => {
                alignment = 0;
                previous_group = None;
            }
            _ => {}
        }
    }
    align_line(&mut layout.entries[line_start..], x, right, alignment);
    layout.line_count = layout
        .entries
        .iter()
        .filter(|entry| {
            entry.atom.is_some_and(|atom| {
                matches!(atoms[atom].kind, MessageAtomKind::Glyph { ruby: false, .. })
            })
        })
        .map(|entry| entry.line.wrapping_add(1))
        .max()
        .unwrap_or(0);
    vertical_align(&mut layout, atoms, configuration.word(0x60)?);
    position_ruby(
        &mut layout,
        atoms,
        left,
        right,
        configuration.word(0x5c)? as i32,
    )?;
    layout.validate(atoms)?;
    Ok(layout)
}

fn next_indent_state(
    atoms: &[MessageAtom],
    start: usize,
    previous: bool,
    characters: &MessageBreakCharacters,
) -> bool {
    let character = atoms[start..].iter().find_map(|atom| match atom.kind {
        MessageAtomKind::Glyph {
            character,
            ruby: false,
            ..
        } => Some(character),
        _ => None,
    });
    match character {
        Some(character) if characters.indent_open.contains(&character) => false,
        Some(character) if characters.indent_keep.contains(&character) => previous,
        _ => true,
    }
}
fn align_line(entries: &mut [MessageLayoutEntry], end: i32, right: i32, alignment: u32) {
    let spare = right.wrapping_sub(end);
    let shift = match alignment {
        1 => spare / 2,
        2 => spare,
        _ => 0,
    };
    for entry in entries {
        entry.position[0] = entry.position[0].wrapping_add(shift);
    }
}
fn vertical_align(layout: &mut MessageLayout, atoms: &[MessageAtom], mode: u32) {
    if !matches!(mode, 1 | 2) {
        return;
    }
    let mut lines: BTreeMap<u32, (i32, i32)> = BTreeMap::new();
    for entry in &layout.entries {
        if entry.atom.is_some_and(|atom| {
            matches!(atoms[atom].kind, MessageAtomKind::Glyph { ruby: false, .. })
        }) {
            let bounds = lines
                .entry(entry.line)
                .or_insert((entry.position[1], entry.position[1]));
            bounds.0 = bounds.0.min(entry.position[1]);
            bounds.1 = bounds.1.max(entry.position[1].wrapping_add(entry.size[1]));
        }
    }
    for entry in &mut layout.entries {
        if entry.atom.is_some_and(|atom| {
            matches!(atoms[atom].kind, MessageAtomKind::Glyph { ruby: false, .. })
        }) {
            if let Some(&(top, bottom)) = lines.get(&entry.line) {
                entry.position[1] = if mode == 1 {
                    top
                } else {
                    bottom.wrapping_sub(entry.size[1])
                };
            }
        }
    }
}
fn position_ruby(
    layout: &mut MessageLayout,
    atoms: &[MessageAtom],
    left: i32,
    right: i32,
    offset: i32,
) -> Result<()> {
    let mut groups: BTreeMap<(u32, u32), (Vec<usize>, Vec<usize>)> = BTreeMap::new();
    let mut ruby_groups: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
    for (index, entry) in layout.entries.iter().enumerate() {
        if let Some(MessageAtom {
            kind:
                MessageAtomKind::Glyph {
                    group: Some(group),
                    ruby,
                    ..
                },
            ..
        }) = entry.atom.and_then(|atom| atoms.get(atom))
        {
            if *ruby {
                ruby_groups.entry(*group).or_default().push(index);
            } else {
                groups
                    .entry((*group, entry.line))
                    .or_default()
                    .0
                    .push(index);
            }
        }
    }
    for (group, rubies) in ruby_groups {
        let segments: Vec<_> = groups
            .iter()
            .filter(|((id, _), _)| *id == group)
            .map(|(_, (body, _))| body.clone())
            .collect();
        if segments.is_empty() {
            return Err(invalid("message ruby group has no earlier body glyph"));
        }
        let total: usize = segments.iter().map(Vec::len).sum();
        let mut consumed_body = 0usize;
        let mut consumed_ruby = 0usize;
        for body in segments {
            consumed_body += body.len();
            let end = consumed_body * rubies.len() / total;
            let reading = &rubies[consumed_ruby..end];
            consumed_ruby = end;
            if reading.is_empty() {
                continue;
            }
            let body_width: i32 = body.iter().fold(0i32, |value, &index| {
                value.wrapping_add(layout.entries[index].size[0])
            });
            let ruby_width: i32 = reading.iter().fold(0i32, |value, &index| {
                value.wrapping_add(layout.entries[index].size[0])
            });
            let body_left = layout.entries[body[0]].position[0];
            let mut x = body_left;
            let mut spare = body_width.wrapping_sub(ruby_width);
            if spare < 0 {
                x = x
                    .wrapping_add(spare / 2)
                    .max(left)
                    .min(right.wrapping_sub(ruby_width));
            }
            let top = body
                .iter()
                .map(|&index| layout.entries[index].position[1])
                .min()
                .unwrap();
            let height = reading
                .iter()
                .map(|&index| layout.entries[index].size[1])
                .max()
                .unwrap();
            for (position, &index) in reading.iter().enumerate() {
                let anchor = body[(position * body.len() / reading.len()).min(body.len() - 1)];
                if anchor >= index {
                    return Err(invalid(
                        "message ruby layout anchor is not an earlier record",
                    ));
                }
                let anchor_line = layout.entries[anchor].line;
                let entry = &mut layout.entries[index];
                entry.position = [x, top.wrapping_sub(height).wrapping_add(offset)];
                entry.line = anchor_line;
                entry.display_anchor = Some(anchor);
                x = x.wrapping_add(entry.size[0]);
                if spare >= 0 {
                    let interval = spare / (reading.len() - position) as i32;
                    x = x.wrapping_add(interval);
                    spare -= interval;
                }
            }
        }
    }
    Ok(())
}

fn reveal_starts(request: &MessageRenderRequest<'_>) -> Result<Vec<Option<i32>>> {
    let mut lines: BTreeMap<u32, u32> = BTreeMap::new();
    for entry in &request.layout.entries {
        if entry.atom.is_some_and(|atom| {
            matches!(
                request.object.atoms()[atom].kind,
                MessageAtomKind::Glyph { .. }
            )
        }) {
            lines
                .entry(entry.line)
                .and_modify(|x| *x = (*x).min(entry.position[0] as u32))
                .or_insert(entry.position[0] as u32);
        }
    }
    let mut result = vec![None; request.layout.entries.len()];
    let mut current_line = None;
    let mut accumulated = 0i32;
    let mut last_edge = 0i32;
    for (index, entry) in request.layout.entries.iter().enumerate() {
        let kind = entry.atom.map(|atom| &request.object.atoms()[atom].kind);
        if matches!(
            kind,
            Some(MessageAtomKind::Control(9) | MessageAtomKind::ResetWait)
        ) {
            accumulated = 0;
        }
        if !matches!(kind, Some(MessageAtomKind::Glyph { .. })) {
            continue;
        }
        if current_line.is_some_and(|line| line != entry.line) {
            accumulated = accumulated.wrapping_add(last_edge);
        }
        current_line = Some(entry.line);
        let start = lines.get(&entry.line).copied().unwrap_or(0) as i32;
        let relative = entry.position[0].wrapping_sub(start);
        result[index] = Some(accumulated.wrapping_add(relative));
        last_edge = relative.wrapping_add(entry.size[0]);
    }
    Ok(result)
}

fn apply_reveal(
    target: &mut Canvas,
    source: &Canvas,
    glyph: RevealGlyph,
    phase: i32,
    width: u32,
    force: bool,
) -> (bool, bool) {
    let elapsed = phase.wrapping_sub(glyph.start);
    let complete = force || elapsed.wrapping_sub(width as i32) >= glyph.bounds[2];
    if !complete && elapsed < glyph.bounds[0] {
        return (false, false);
    }
    let endpoint = |value: i32, position: i32, limit: u32| {
        (value.wrapping_add(position) as u32).min(limit.saturating_sub(1))
    };
    let left = endpoint(glyph.bounds[0], glyph.position[0], target.width());
    let top = endpoint(glyph.bounds[1], glyph.position[1], target.height());
    let right = endpoint(glyph.bounds[2], glyph.position[0], target.width());
    let bottom = endpoint(glyph.bounds[3], glyph.position[1], target.height());
    for y in top..bottom {
        for x in left..right {
            let at = ((y as usize * source.width() as usize) + x as usize) * 4 + 3;
            let local = (x as i32).wrapping_sub(glyph.position[0]);
            let distance = elapsed.wrapping_sub(local);
            let alpha = source.pixels()[at];
            if complete || distance >= width as i32 {
                target.pixels_mut()[at] = alpha;
            } else if distance >= 0 {
                let gradient_index = width.wrapping_sub(distance as u32).min(width - 1);
                let gradient = 255u32.wrapping_sub(gradient_index.wrapping_mul(255) / width) as u8;
                let next = (((u32::from(gradient) + 1) * u32::from(alpha)) >> 8) as u8;
                let alpha = target.pixels()[at].max(next);
                target.pixels_mut()[at] = alpha;
            }
        }
    }
    (complete, true)
}

fn clipped_bounds(canvas: &Canvas, position: [i32; 2], bounds: [i32; 4]) -> Option<Rect> {
    let left = (i64::from(position[0]) + i64::from(bounds[0]))
        .max(0)
        .min(i64::from(canvas.width()));
    let top = (i64::from(position[1]) + i64::from(bounds[1]))
        .max(0)
        .min(i64::from(canvas.height()));
    let right = (i64::from(position[0]) + i64::from(bounds[2]))
        .max(0)
        .min(i64::from(canvas.width()));
    let bottom = (i64::from(position[1]) + i64::from(bounds[3]))
        .max(0)
        .min(i64::from(canvas.height()));
    (right > left && bottom > top).then_some(Rect::new(
        left as i32,
        top as i32,
        (right - left) as u32,
        (bottom - top) as u32,
    ))
}
fn add_dirty(target: &mut Option<Rect>, next: Option<Rect>) {
    let Some(next) = next else {
        return;
    };
    *target = Some(match *target {
        None => next,
        Some(old) => {
            let left = old.x.min(next.x);
            let top = old.y.min(next.y);
            let right = (i64::from(old.x) + i64::from(old.width))
                .max(i64::from(next.x) + i64::from(next.width));
            let bottom = (i64::from(old.y) + i64::from(old.height))
                .max(i64::from(next.y) + i64::from(next.height));
            Rect::new(
                left,
                top,
                (right - i64::from(left)) as u32,
                (bottom - i64::from(top)) as u32,
            )
        }
    });
}
fn copy_rectangle(target: &mut Canvas, source: &Canvas, rectangle: Rect) {
    for y in rectangle.y as u32..rectangle.y as u32 + rectangle.height {
        let start = ((y as usize * source.width() as usize) + rectangle.x as usize) * 4;
        let end = start + rectangle.width as usize * 4;
        target.pixels_mut()[start..end].copy_from_slice(&source.pixels()[start..end]);
    }
}

fn be16(bytes: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_be_bytes(
        bytes.get(offset..offset.checked_add(2)?)?.try_into().ok()?,
    ))
}
fn be32(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_be_bytes(
        bytes.get(offset..offset.checked_add(4)?)?.try_into().ok()?,
    ))
}
fn sfnt_table(bytes: &[u8], face: usize, tag: &[u8; 4]) -> Option<usize> {
    let count = usize::from(be16(bytes, face.checked_add(4)?)?).min(4096);
    for index in 0..count {
        let record = face.checked_add(12)?.checked_add(index * 16)?;
        if bytes.get(record..record + 4) == Some(tag.as_slice()) {
            return Some(be32(bytes, record + 8)? as usize);
        }
    }
    None
}
fn sfnt_weight(bytes: &[u8], face: usize) -> Option<u16> {
    be16(bytes, sfnt_table(bytes, face, b"OS/2")?.checked_add(4)?)
}
fn sfnt_italic(bytes: &[u8], face: usize) -> bool {
    sfnt_table(bytes, face, b"OS/2")
        .and_then(|table| be16(bytes, table + 62))
        .is_some_and(|selection| selection & 1 != 0)
        || sfnt_table(bytes, face, b"head")
            .and_then(|table| be16(bytes, table + 44))
            .is_some_and(|style| style & 2 != 0)
}
fn sfnt_last_resort(bytes: &[u8], face: usize) -> bool {
    sfnt_table(bytes, face, b"head")
        .and_then(|table| be16(bytes, table + 16))
        .is_some_and(|flags| flags & (1 << 14) != 0)
}
fn sfnt_monospace(bytes: &[u8], face: usize) -> bool {
    sfnt_table(bytes, face, b"post")
        .and_then(|table| be32(bytes, table + 12))
        .is_some_and(|pitch| pitch != 0)
}
fn sfnt_serif(bytes: &[u8], face: usize) -> bool {
    sfnt_table(bytes, face, b"OS/2")
        .and_then(|table| be16(bytes, table + 30))
        .is_some_and(|class| matches!(class >> 8, 1..=7))
}
fn sfnt_names(bytes: &[u8], face: usize) -> BTreeSet<String> {
    let mut result = BTreeSet::new();
    let Some(table) = sfnt_table(bytes, face, b"name") else {
        return result;
    };
    let Some(count) = be16(bytes, table + 2) else {
        return result;
    };
    let Some(storage) = be16(bytes, table + 4).map(|offset| table + usize::from(offset)) else {
        return result;
    };
    for index in 0..usize::from(count).min(4096) {
        let record = table + 6 + index * 12;
        let (Some(platform), Some(encoding), Some(name), Some(length), Some(offset)) = (
            be16(bytes, record),
            be16(bytes, record + 2),
            be16(bytes, record + 6),
            be16(bytes, record + 8),
            be16(bytes, record + 10),
        ) else {
            continue;
        };
        if !matches!(name, 1 | 4 | 6 | 16) || length > 4096 {
            continue;
        }
        let Some(payload) = bytes.get(
            storage + usize::from(offset)..storage + usize::from(offset) + usize::from(length),
        ) else {
            continue;
        };
        let text = if platform == 0 || (platform == 3 && matches!(encoding, 0 | 1 | 10)) {
            if payload.len() % 2 != 0 {
                continue;
            }
            let units: Vec<_> = payload
                .chunks_exact(2)
                .map(|unit| u16::from_be_bytes(unit.try_into().unwrap()))
                .collect();
            String::from_utf16(&units).ok()
        } else if (platform == 3 && encoding == 2) || (platform == 1 && encoding == 1) {
            let (text, _, errors) = encoding_rs::SHIFT_JIS.decode(payload);
            (!errors).then(|| text.into_owned())
        } else if payload.is_ascii() {
            String::from_utf8(payload.to_vec()).ok()
        } else {
            None
        };
        if let Some(text) = text {
            let text = text.trim_matches('\0').trim();
            if !text.is_empty() {
                result.insert(text.to_owned());
            }
        }
    }
    result
}

#[cfg(all(test, windows))]
mod game_font_tests {
    use super::*;
    use cs2_runtime::message_object::MessageDefaults;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct FontFixture(PathBuf);

    impl Drop for FontFixture {
        fn drop(&mut self) {
            for relative in [
                "inner/config/startup.xml",
                "inner/font/face.ttf",
                "inner/font/broken.ttf",
                "inner/other/outside.ttf",
                "font/face.ttf",
                "font/outside.ttf",
            ] {
                let _ = fs::remove_file(self.0.join(relative));
            }
            for relative in ["inner/config", "inner/font", "inner/other", "inner", "font"] {
                let _ = fs::remove_dir(self.0.join(relative));
            }
            let _ = fs::remove_dir(&self.0);
        }
    }

    fn configuration(name: &str, backend: u32) -> MessageConfiguration {
        let encoding = MessageEncoding::Unicode;
        let defaults = MessageDefaults {
            font_name: name.encode_utf16().collect(),
            bold: false,
            shadow: 0,
            outline: 0,
            glyph_parameter: Some(128),
            gradual: false,
        };
        let mut bytes = defaults.configuration(encoding).unwrap().bytes().to_vec();
        bytes[0x118..0x11c].copy_from_slice(&backend.to_le_bytes());
        MessageConfiguration::from_bytes(&bytes, encoding).unwrap()
    }

    #[test]
    fn message_reveal_submits_new_pixels_only_when_enabled() {
        use cs2_runtime::message_object::MessagePlaneDisplay;

        let windows = PathBuf::from(std::env::var_os("WINDIR").unwrap());
        let font = fs::read(windows.join("Fonts/arial.ttf")).unwrap();
        let mut device = DesktopMessageDevice::default();
        device.bind_font("Arial", &font, 0).unwrap();

        let scale = Some(5);
        let mut bytes = configuration("Arial", 0).bytes().to_vec();
        bytes[8..12].copy_from_slice(&128u32.to_le_bytes());
        bytes[12..16].copy_from_slice(&64u32.to_le_bytes());
        bytes[0x74..0x78].copy_from_slice(&7u32.to_le_bytes());
        let mut skipped_pixels = Vec::new();
        for (id, submit, force) in [(-1, true, true), (-2, true, false), (-3, false, false)] {
            bytes[0x128..0x12c].copy_from_slice(&u32::from(submit).to_le_bytes());
            let mut message = MessageObject::new(id, MessageEncoding::Unicode);
            message.recreate(&bytes, scale, &mut device).unwrap();
            message.append(b"ABC").unwrap();
            message.start(0, MessagePlaneDisplay::Visible, 100, scale, &mut device).unwrap();
            let start = device.snapshot_pixels(id).unwrap().submitted_rgba;
            assert!(start.chunks_exact(4).any(|pixel| pixel[3] != 0));
            if force {
                message.force_complete(MessagePlaneDisplay::Visible, scale, &mut device).unwrap();
                skipped_pixels = device.snapshot_pixels(id).unwrap().submitted_rgba;
                assert_ne!(skipped_pixels, start);
                continue;
            }
            message.frame_update(103, scale, &mut device).unwrap();
            assert_eq!(device.surface(id).unwrap().submitted.pixels(), start);
            message.frame_update(104, scale, &mut device).unwrap();
            if submit {
                assert_ne!(device.surface(id).unwrap().submitted.pixels(), start);
                assert_ne!(device.surface(id).unwrap().submitted.pixels(), skipped_pixels);
            }
            message.frame_update(107, scale, &mut device).unwrap();
            message.frame_update(108, scale, &mut device).unwrap();
            assert_eq!(message.remaining_all().unwrap(), 0);
            let surface = device.surface(id).unwrap();
            assert_eq!(surface.visible.pixels(), skipped_pixels);
            if submit {
                assert_eq!(surface.submitted.pixels(), skipped_pixels);
                assert!(surface.dirty.is_none());
            } else {
                assert_eq!(surface.submitted.pixels(), start);
                assert!(surface.dirty.is_some());
            }
        }
    }

    #[test]
    fn registered_face_reuses_only_the_matching_preloaded_rasterizer() {
        let windows = PathBuf::from(std::env::var_os("WINDIR").unwrap());
        let bytes = Arc::new(fs::read(windows.join("Fonts/arial.ttf")).unwrap());
        let rasterizer = Arc::new(FontRasterizer::from_bytes(&bytes, 0).unwrap());
        let origin = Arc::new(FontOrigin::Resource("font/face.ttf".into()));
        let mut device = DesktopMessageDevice::default();
        device.fonts = vec![LoadedFont {
            origin: Arc::clone(&origin),
            index: 0,
            rasterizer: Arc::clone(&rasterizer),
        }].into();

        let source = Arc::new(FontSource::Bytes(Arc::clone(&bytes)));
        device.register_sfnt(&bytes, Arc::clone(&source), &origin);
        device.register_sfnt(&bytes, source, &FontOrigin::Resource("other/face.ttf".into()));
        let selected = device.font(&configuration("Arial", 0), 'A', false, false, None).unwrap().0;
        assert!(std::ptr::eq(selected, rasterizer.as_ref()));
        assert!(device.fallback_faces[1].font.get().is_none());
    }

    #[test]
    fn message_family_substitutes_a_real_face_for_punctuation_outside_cjk_blocks() {
        let windows = PathBuf::from(std::env::var_os("WINDIR").unwrap());
        let font = fs::read(windows.join("Fonts/arial.ttf")).unwrap();
        let mut device = DesktopMessageDevice::default();
        device.bind_font("Fixture fallback", &font, 0).unwrap();
        let requested = configuration("Uninstalled game family", 0);
        let glyph = device.mask(&requested, '\u{2014}', 24, false, false).unwrap();
        assert!(glyph.alpha().iter().any(|&alpha| alpha != 0));
        assert!(device.font_substitutions()[0].selected_name.to_lowercase().contains("arial"));
        let error = device.mask(&requested, '\u{10ffff}', 24, false, false).unwrap_err().to_string();
        assert!(error.contains("U+10FFFF") && error.contains("uninstalled game family"));
    }

    #[test]
    fn message_backend1_routes_only_the_active_loose_game_fonts() {
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let fixture = FontFixture(std::env::temp_dir().join(format!("cs2-font-route-{nonce}")));
        for relative in ["inner/config", "inner/font", "inner/other", "font"] {
            fs::create_dir_all(fixture.0.join(relative)).unwrap();
        }
        fs::write(
            fixture.0.join("inner/config/startup.xml"),
            br#"<?xml version="1.0" encoding="UTF-8"?><document/>"#,
        ).unwrap();
        let windows = PathBuf::from(std::env::var_os("WINDIR").unwrap());
        let font = fs::read(windows.join("Fonts/arial.ttf")).unwrap();
        fs::write(fixture.0.join("inner/font/face.ttf"), &font).unwrap();
        fs::write(fixture.0.join("inner/font/broken.ttf"), b"bad font").unwrap();
        fs::write(fixture.0.join("inner/other/outside.ttf"), &font).unwrap();
        fs::write(fixture.0.join("font/face.ttf"), b"outer decoy").unwrap();
        fs::write(fixture.0.join("font/outside.ttf"), &font).unwrap();

        let index = ResourceIndex::scan(&fixture.0).unwrap();
        assert!(index.startup_configuration().unwrap().is_some());
        let mut device = DesktopMessageDevice::default();
        device.load_game_font_files(&index);
        device.bind_font("Test family", &font, 0).unwrap();

        let by_file = device.mask(&configuration("FACE.TTF", 1), 'A', 24, false, false).unwrap();
        let by_family = device.mask(&configuration("TEST FAMILY", 0), 'A', 24, false, false).unwrap();
        assert!(by_file.alpha().iter().any(|&alpha| alpha != 0));
        assert_eq!(by_file.alpha(), by_family.alpha());
        let error = |name, character| {
            device.mask(&configuration(name, 1), character, 24, false, false).unwrap_err().to_string()
        };
        assert!(error("outside.ttf", 'A').contains("not found"));
        assert!(error("Test family", 'A').contains("not found"));
        assert!(error("broken.ttf", 'A').contains("could not be decoded"));
        assert!(error("face.ttf", '\u{10ffff}').contains("could not supply U+10FFFF"));
    }
}

/// Message-specific effect path. Canvas's generic outline uses a maximum;
/// this documented legacy path accumulates three horizontal and then three
/// vertical Alpha samples, saturating after each pass.
pub(super) fn paint_message_user_glyph(
    target: &mut Canvas,
    mapped: &MessageUserGlyph,
    position: [i32; 2],
    size: u32,
    shadow_color: u32,
    effects: GlyphEffects,
) -> Result<[i32; 4]> {
    if size == 0 {
        return Ok([0; 4]);
    }
    let glyph = scale_user_glyph(mapped, size)?;
    let mut foreground = vec![0; GLYPH_SIDE * GLYPH_SIDE * 4];
    for y in 0..size.min(GLYPH_SIDE as u32 - 1) {
        for x in 0..size.min(GLYPH_SIDE as u32 - 1) {
            let from = (y as usize * size as usize + x as usize) * 4;
            let to = ((y as usize + 1) * GLYPH_SIDE + x as usize + 1) * 4;
            let pixel = &glyph.bgra[from..from + 4];
            foreground[to..to + 4].copy_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
        }
    }
    paint_glyph_pixels(
        target,
        foreground,
        position,
        [1, 1, size as i32, size as i32],
        size,
        shadow_color,
        effects,
    )
}

pub(super) fn paint_message_mask(
    target: &mut Canvas,
    glyph: &GlyphMask,
    position: [i32; 2],
    size: u32,
    color: u32,
    shadow_color: u32,
    effects: GlyphEffects,
) -> Result<[i32; 4]> {
    if glyph.width() == 0 || glyph.height() == 0 {
        return Ok([0; 4]);
    }
    let mut pixels = vec![0; GLYPH_SIDE * GLYPH_SIDE * 4];
    let rgb = [color as u8, (color >> 8) as u8, (color >> 16) as u8];
    for pixel in pixels.chunks_exact_mut(4) {
        pixel[..3].copy_from_slice(&rgb);
    }
    let offset = [glyph.metrics().bearing_x, glyph.metrics().bearing_y];
    for y in 0..glyph.height() {
        let dy = i64::from(offset[1]) + i64::from(y);
        if dy < 0 || dy >= GLYPH_SIDE as i64 {
            continue;
        }
        for x in 0..glyph.width() {
            let dx = i64::from(offset[0]) + i64::from(x);
            if dx < 0 || dx >= GLYPH_SIDE as i64 {
                continue;
            }
            let source = y as usize * glyph.width() as usize + x as usize;
            pixels[(dy as usize * GLYPH_SIDE + dx as usize) * 4 + 3] = glyph.alpha()[source];
        }
    }
    paint_glyph_pixels(
        target,
        pixels,
        position,
        [
            offset[0],
            offset[1],
            glyph.width() as i32,
            glyph.height() as i32,
        ],
        size,
        shadow_color,
        effects,
    )
}

fn paint_glyph_pixels(
    target: &mut Canvas,
    foreground: Vec<u8>,
    position: [i32; 2],
    extent: [i32; 4],
    font_size: u32,
    shadow_color: u32,
    effects: GlyphEffects,
) -> Result<[i32; 4]> {
    let side = GLYPH_SIDE;
    let enabled = effects.outline || effects.shadow;
    let mut result = vec![0; side * side * 4];
    if enabled {
        // AABBGGRR's low bytes enter BGRA without swapping for the effect
        // plane. In RGBA storage this deliberately reverses shadow red/blue.
        let shadow_rgb = [
            (shadow_color >> 16) as u8,
            (shadow_color >> 8) as u8,
            shadow_color as u8,
        ];
        for pixel in result.chunks_exact_mut(4) {
            pixel[..3].copy_from_slice(&shadow_rgb);
        }
        if effects.outline {
            let mut horizontal = vec![0u8; side * side];
            for y in 0..side {
                for x in 0..side {
                    let alpha = |column: usize| u16::from(foreground[(y * side + column) * 4 + 3]);
                    let sum = alpha(x)
                        + if x > 0 { alpha(x - 1) } else { 0 }
                        + if x + 1 < side { alpha(x + 1) } else { 0 };
                    horizontal[y * side + x] = sum.min(255) as u8;
                }
            }
            for y in 0..side {
                for x in 0..side {
                    let alpha = |row: usize| u16::from(horizontal[row * side + x]);
                    let sum = alpha(y)
                        + if y > 0 { alpha(y - 1) } else { 0 }
                        + if y + 1 < side { alpha(y + 1) } else { 0 };
                    result[(y * side + x) * 4 + 3] = sum.min(255) as u8;
                }
            }
        }
        if effects.shadow {
            let offset = effects.shadow_offset.map(|value| {
                if font_size < 20 {
                    value.wrapping_add(1) >> 1
                } else {
                    value
                }
            });
            if offset.iter().any(|&value| value < 0) {
                return Err(invalid(
                    "message shadow offset would access before its glyph surface",
                ));
            }
            let [dx, dy] = [offset[0] as usize, offset[1] as usize];
            if dx < side && dy < side {
                for y in (0..side - dy).rev() {
                    for x in 0..side - dx {
                        let from = (y * side + x) * 4 + 3;
                        let to = ((y + dy) * side + x + dx) * 4 + 3;
                        // Reads result[from] at this instant: horizontal
                        // overlap intentionally observes earlier writes.
                        result[to] = (u16::from(result[to])
                            + u16::from(result[from])
                            + u16::from(foreground[from]))
                        .min(255) as u8;
                    }
                }
            }
        }
        let foreground = RgbaFrame::from_rgba8(side as u32, side as u32, foreground)
            .map_err(|_| invalid("message foreground surface construction failed"))?;
        let mut effects_canvas = Canvas::from_rgba8(side as u32, side as u32, result)
            .map_err(|_| invalid("message effect surface construction failed"))?;
        effects_canvas
            .draw_frame(&foreground, &ImageDrawOptions::new(Point::default()))
            .map_err(|_| invalid("message foreground/effect composition failed"))?;
        result = effects_canvas.pixels().to_vec();
    } else {
        result = foreground;
    }
    // The original paint bounds use RAW shadow offsets even when disabled or
    // when small-font shadow processing used the halved effective offsets.
    let right = extent[0]
        .wrapping_add(extent[2])
        .wrapping_add(effects.shadow_offset[0])
        .wrapping_add(2);
    let bottom = extent[1]
        .wrapping_add(extent[3])
        .wrapping_add(effects.shadow_offset[1])
        .wrapping_add(2);
    let bounds = [0, 0, right, bottom];
    let frame = RgbaFrame::from_rgba8(side as u32, side as u32, result)
        .map_err(|_| invalid("message composited glyph frame construction failed"))?;
    let mut options = ImageDrawOptions::new(Point::new(position[0], position[1]));
    options.clip = clipped_bounds(target, position, bounds);
    if options.clip.is_some() {
        target
            .draw_frame(&frame, &options)
            .map_err(|_| invalid("message composited glyph pixel submission failed"))?;
    }
    Ok(bounds)
}

/// Shared by old and new text planes. ARGB plane color modulates sampled RGBA;
/// the optional overlay is the ordinary command59 vertex-color override, not
/// an additive text recoloring operation. The retained source stays untouched.
pub(super) fn draw_message_plane(
    target: &mut Canvas,
    frame: RgbaFrame,
    node: &super::SceneNode,
    offset: [i32; 2],
    registered_size: [u32; 2],
) -> Result<()> {
    if !node.visible {
        return Ok(());
    }
    if node
        .position
        .iter()
        .chain(node.scale.iter())
        .chain(node.size.iter())
        .any(|value| !value.is_finite())
    {
        return Err(invalid(
            "message plane transform contains nonfinite coordinates",
        ));
    }
    if frame.width() == 0 || frame.height() == 0 || node.scale[0] == 0.0 || node.scale[1] == 0.0 {
        // Degenerate geometry draws no pixels but retains text/reveal state.
        return Ok(());
    }
    let geometry = if node.initialized {
        node.size
    } else {
        [registered_size[0] as f32, registered_size[1] as f32]
    };
    if geometry[0] == 0.0 || geometry[1] == 0.0 {
        return Ok(());
    }
    let color = if node.overlay_color != 0 {
        node.overlay_color
    } else {
        node.color
    };
    let channels = [
        (color >> 16) as u8,
        (color >> 8) as u8,
        color as u8,
        (color >> 24) as u8,
    ];
    let mut pixels = frame.pixels().to_vec();
    if color != u32::MAX {
        for pixel in pixels.chunks_exact_mut(4) {
            for component in 0..4 {
                pixel[component] = ((u16::from(pixel[component]) * u16::from(channels[component])
                    + 127)
                    / 255) as u8;
            }
        }
    }
    let tinted = RgbaFrame::from_rgba8(frame.width(), frame.height(), pixels)
        .map_err(|_| invalid("message plane tinted frame allocation failed"))?;
    let scale = [
        node.scale[0] * geometry[0] / frame.width() as f32,
        node.scale[1] * geometry[1] / frame.height() as f32,
    ];
    let mut options = ImageDrawOptions::new(Point::new(
        (node.position[0] as i32).wrapping_add(offset[0]),
        (node.position[1] as i32).wrapping_add(offset[1]),
    ));
    options.scale_x = scale[0];
    options.scale_y = scale[1];
    // Canvas already supports signed/mirrored scales and clips to the viewport.
    // An unscaled registration rectangle would incorrectly cut enlarged text.
    target
        .draw_frame(&tinted, &options)
        .map_err(|_| invalid("message plane scaled/color pixel composition failed"))
}

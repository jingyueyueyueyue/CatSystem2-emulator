//! Ordinary1009: mutable source conversion, encoded memory, then private I/O.
//! Yukikoi SHA-bound entry197a30 ->26c3d0 ->1e2940. No resource is writable.
use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use cs2_renderer::RgbaFrame;
use cs2_script::{NativeReturn, Result, Vm};
use image::{ExtendedColorType, ImageEncoder};

use super::{LoggingHost, LogicEvent, native_state_error, read_u32, trim_nul};

/// Original underlying image+58, shared by aliases, not by decoded cache copies.
/// Pixels remain in the renderer's fixed RGBA interpretation. Setting this bit
/// exchanges their R/B channels; JPEG consumes the corresponding raw RGB bytes.
#[derive(Debug, Clone, Default)]
pub(super) struct ImageExportState {
    pub(super) rgb_pixels: BTreeSet<u32>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind { Bmp, Png, Jpeg }

impl Kind {
    fn filename(name: &[u16]) -> Self {
        let extension = name.iter().rposition(|&c| c == u16::from(b'.')
            || c == u16::from(b'/') || c == u16::from(b'\\'))
            .filter(|&i| name[i] == u16::from(b'.'))
            .map_or(&[][..], |i| &name[i + 1..]);
        let eq = |bytes: &[u8]| extension.len() == bytes.len()
            && extension.iter().zip(bytes).all(|(&a, &b)| a <= 127 && (a as u8).eq_ignore_ascii_case(&b));
        if eq(b"bmp") { Self::Bmp } else if eq(b"png") { Self::Png } else { Self::Jpeg }
    }
}

impl LoggingHost<'_> {
    /// Shared original 66c3d0/66c700 filename-marker contract. Tweet1018
    /// uses signed narrow literal widening; Tweet1019 uses proper wide text.
    pub(super) fn expand_tweet_application_path(&self, input: &[u8], wide_input: bool) -> Result<Vec<u16>> {
        let application = super::language_platform::image_export_path_fields(
            self.resources, &self.data_services.language.platform,
        ).map_err(|_| native_state_error("Tweet application path fields could not be read"))?;
        let fields = PathFields { application, save: self.save_root };
        if !wide_input { return fields.expand(input); }
        fields.expand_wide(&super::ScriptTextEncoding::Cp932.decode(trim_nul(input)).encode_utf16().collect::<Vec<_>>())
    }

    pub(super) fn image_export_native1009(&mut self, args: &[u8], vm: &Vm) -> Result<NativeReturn> {
        if args.len() != 8 {
            return Err(native_state_error("native 1009 requires exactly two DWORDs"));
        }
        let handle = read_u32(args, 0)?;
        // A zero wrapper never reads the filename address or its VM contents.
        if handle == 0 { return Ok(NativeReturn::Value(0)); }
        let formatted = format_filename(vm, &vm.read_string(read_u32(args, 4)?)?, 0)?;
        let fields = super::language_platform::image_export_path_fields(
            self.resources, &self.data_services.language.platform,
        ).map_err(|_| native_state_error("native 1009 application path fields could not be read"))?;
        let paths = PathFields { application: fields, save: self.save_root };
        let name = paths.expand(&formatted)?;
        let kind = Kind::filename(&name);
        let source = self.image_handles.get(&handle).cloned()
            .ok_or_else(|| native_state_error("native 1009 invalid/wrong-type image wrapper"))?;
        let pixel_id = self.data_services.image_wrapper_pixels.get(&handle).copied()
            .ok_or_else(|| native_state_error("native 1009 image wrapper has no underlying pixel identity"))?;
        let swap = kind == Kind::Jpeg && !self.data_services.image_export.rgb_pixels.contains(&pixel_id);
        let opaque = kind != Kind::Png && source.metadata().bit_depth == 32;
        let frame = if swap || opaque {
            let mut pixels = Vec::new();
            pixels.try_reserve_exact(source.pixels().len())
                .map_err(|_| native_state_error("native 1009 source conversion allocation failed"))?;
            pixels.extend_from_slice(source.pixels());
            for pixel in pixels.chunks_exact_mut(4) {
                if swap { pixel.swap(0, 2); }
                if opaque {
                    // 1e3c80/99/b2 IMUL; SAR8. Even alpha255 darkens by 1/256.
                    let alpha = u16::from(pixel[3]);
                    for channel in &mut pixel[..3] { *channel = ((u16::from(*channel) * alpha) >> 8) as u8; }
                    pixel[3] = 255;
                }
            }
            let mut metadata = source.metadata();
            if opaque { metadata.bit_depth = 24; }
            let frame = RgbaFrame::from_rgba8(source.width(), source.height(), pixels)
                .map_err(|_| native_state_error("native 1009 source dimensions are invalid"))?
                .with_raw_metadata(metadata).with_hg_attributes(source.hg_attributes().clone());
            self.replace_image_wrapper_pixels(handle, frame.clone())?;
            if swap { self.data_services.image_export.rgb_pixels.insert(pixel_id); }
            frame
        } else { source };

        // The source commits before codec creation, path validation and open.
        let encoded = match encode(&frame, kind) {
            Ok(bytes) => bytes,
            Err(error) => { self.image_export_event(handle, "encode-failed", error); return Ok(NativeReturn::Value(0)); }
        };
        let name = match String::from_utf16(&name) {
            Ok(name) => name,
            Err(error) => { self.image_export_event(handle, "path-failed", error); return Ok(NativeReturn::Value(0)); }
        };
        let path = match private_path(self.save_root, &name) {
            Ok(path) => path,
            Err(error) => { self.image_export_event(handle, "path-failed", error); return Ok(NativeReturn::Value(0)); }
        };
        if let Err(error) = fs::create_dir_all(self.save_root) {
            self.image_export_event(handle, "open-failed", error);
            return Ok(NativeReturn::Value(0));
        }
        // 0x402: CREATE_NEW, then TRUNCATE_EXISTING. No parent creation,
        // append, replacement rename, rollback or retry of the WriteFile call.
        let mut create = OpenOptions::new();
        create.write(true).create_new(true);
        let mut truncate = OpenOptions::new();
        truncate.write(true).truncate(true);
        #[cfg(windows)] {
            use std::os::windows::fs::OpenOptionsExt;
            create.share_mode(0);
            truncate.share_mode(0);
        }
        let mut file = match create.open(&path).or_else(|_| truncate.open(&path)) {
            Ok(file) => file,
            Err(error) => { self.image_export_event(handle, "open-failed", error); return Ok(NativeReturn::Value(0)); }
        };
        let value = match file.write(&encoded) {
            Ok(count) => {
                self.image_export_event(handle, "write", format!("bytes={count}/{} path={}", encoded.len(), path.display()));
                u32::from(count != 0)
            }
            Err(error) => {
                // 12eb32 returns DWORD(-1); 1e2b8e tests only nonzero. This
                // original bug returns1 on a failed write after a valid open.
                self.image_export_event(handle, "write-error-native-result-1", error);
                1
            }
        };
        Ok(NativeReturn::Value(value))
    }

    fn image_export_event(&mut self, handle: u32, stage: &str, detail: impl std::fmt::Display) {
        self.events.push(LogicEvent { frame: self.frame, kind: "image-export".into(),
            detail: format!("native=1009 image={handle} stage={stage} {detail}") });
    }
}

fn encode(frame: &RgbaFrame, kind: Kind) -> std::result::Result<Vec<u8>, String> {
    if frame.width() == 0 || frame.height() == 0 { return Err("empty source surface".into()); }
    if !matches!(frame.metadata().bit_depth, 24 | 32) { return Err("unsupported source bit depth".into()); }
    let mut output = Vec::new();
    let alpha = kind == Kind::Png && frame.metadata().bit_depth == 32;
    let mut rgb = Vec::new();
    let pixels = if alpha { frame.pixels() } else {
        rgb.try_reserve_exact(frame.pixels().len() / 4 * 3).map_err(|error| error.to_string())?;
        for pixel in frame.pixels().chunks_exact(4) {
            // JPEG accepts raw RGB after the one-time source swap. BMP and
            // PNG interpret the original BGR(A) surface, as the renderer does.
            if kind == Kind::Jpeg { rgb.extend_from_slice(&[pixel[2], pixel[1], pixel[0]]); }
            else { rgb.extend_from_slice(&pixel[..3]); }
        }
        &rgb
    };
    let color = if alpha { ExtendedColorType::Rgba8 } else { ExtendedColorType::Rgb8 };
    match kind {
        Kind::Bmp => image::codecs::bmp::BmpEncoder::new(&mut output)
            .encode(pixels, frame.width(), frame.height(), color),
        Kind::Png => image::codecs::png::PngEncoder::new(&mut output)
            .write_image(pixels, frame.width(), frame.height(), color),
        Kind::Jpeg => image::codecs::jpeg::JpegEncoder::new_with_quality(&mut output, 90)
            .encode(pixels, frame.width(), frame.height(), color),
    }.map_err(|error| error.to_string())?;
    Ok(output)
}

fn private_path(root: &Path, name: &str) -> anyhow::Result<PathBuf> {
    let normalized = name.replace('\\', "/");
    let path = Path::new(&normalized);
    // %save% is the real private binding, which may itself be absolute.
    // Other absolute/special-folder paths retain their identity and fail the
    // same private boundary; they must never become game-root writes.
    let binding = if root.is_absolute() { root.to_path_buf() } else { std::env::current_dir()?.join(root) };
    let private_root = binding.to_string_lossy().replace('\\', "/");
    let relative = if path.is_absolute() { path.strip_prefix(Path::new(&private_root))? } else { path };
    super::save_path(root, relative.to_str().ok_or_else(|| anyhow::anyhow!("non-Unicode output path"))?)
}

struct PathFields<'a> { application: [Vec<u16>; 3], save: &'a Path }

impl PathFields<'_> {
    fn expand(&self, input: &[u8]) -> Result<Vec<u16>> {
        let input = trim_nul(input);
        let mut output = Vec::new();
        let mut offset = 0;
        while offset < input.len() {
            if input[offset] != b'%' {
                // 26c6b2 CBW: literal bytes are NOT decoded as a whole CP932
                // filename. Bytes >=128 become UTF16 ff80..ffff.
                output.push(input[offset] as i8 as i16 as u16);
                offset += 1;
            } else {
                let end = input[offset + 1..].iter().position(|&b| b == b'%')
                    .map_or(input.len(), |i| offset + i + 2);
                let token = &input[offset..end];
                if token.len() >= 260 { return Err(native_state_error("native 1009 path marker exceeds 259 bytes")); }
                let replacement = match token {
                    b"%brand%" => self.application[0].clone(),
                    b"%title%" => self.application[1].clone(),
                    b"%save%" => self.private_save()?,
                    b"%imglib%" => self.library_path()?,
                    b"%USERPROFILE%" => special_folder(false)?,
                    b"%ProgramData%" | b"%ALLUSERSPROFILE%" => special_folder(true)?,
                    // The native does not perform LANG/category/resource lookup
                    // here. Unknown percent markers remain literal via CP932.
                    _ => super::ScriptTextEncoding::Cp932.decode(token).encode_utf16().collect(),
                };
                output.extend(replacement);
                offset = end;
            }
            if output.len() > 2047 { return Err(native_state_error("native 1009 expanded path exceeds 2047 UTF16 units")); }
        }
        Ok(output)
    }

    fn private_save(&self) -> Result<Vec<u16>> {
        let root = if self.save.is_absolute() { self.save.to_path_buf() } else {
            std::env::current_dir().map_err(|_| native_state_error("native 1009 private binding has no current directory"))?
                .join(self.save)
        };
        Ok(root.to_string_lossy().encode_utf16().collect())
    }

    fn library_path(&self) -> Result<Vec<u16>> {
        // FOLDER/imglib is already expanded once by the original wide helper
        // during startup. Inserted values are not scanned again in that pass.
        let output = self.expand_wide(&self.application[2])?;
        if output.len() > 259 { return Err(native_state_error("native 1009 image-library folder exceeds 259 UTF16 units")); }
        Ok(output)
    }

    fn expand_wide(&self, input: &[u16]) -> Result<Vec<u16>> {
        let mut output = Vec::new();
        let mut offset = 0;
        while offset < input.len() {
            if input[offset] != u16::from(b'%') { output.push(input[offset]); offset += 1; }
            else {
                let end = input[offset + 1..].iter().position(|&c| c == u16::from(b'%'))
                    .map_or(input.len(), |i| offset + i + 2);
                if end - offset >= 260 { return Err(native_state_error("native 1009 wide folder marker exceeds capacity")); }
                let marker = String::from_utf16(&input[offset..end])
                    .map_err(|_| native_state_error("native 1009 folder marker has invalid UTF16"))?;
                let value = match marker.as_str() {
                    "%brand%" => self.application[0].clone(),
                    "%title%" => self.application[1].clone(),
                    "%save%" => self.private_save()?,
                    "%imglib%" if std::ptr::eq(input, self.application[2].as_slice()) => return Err(native_state_error("native 1009 self-referential image-library initialization has no established binding")),
                    "%imglib%" => self.library_path()?,
                    "%USERPROFILE%" => special_folder(false)?,
                    "%ProgramData%" | "%ALLUSERSPROFILE%" => special_folder(true)?,
                    _ => input[offset..end].to_vec(),
                };
                output.extend(value);
                offset = end;
            }
            if output.len() > 2047 { return Err(native_state_error("wide application path exceeds 2047 UTF16 units")); }
        }
        Ok(output)
    }
}

fn special_folder(common: bool) -> Result<Vec<u16>> {
    #[cfg(windows)] {
        // Original helper modes8/9 map to COMMON_APPDATA(35)/APPDATA(26).
        #[link(name = "shell32")]
        unsafe extern "system" { fn SHGetFolderPathW(window: *mut std::ffi::c_void, folder: i32,
            token: *mut std::ffi::c_void, flags: u32, path: *mut u16) -> i32; }
        let mut path = [0u16; 260];
        let status = unsafe { SHGetFolderPathW(std::ptr::null_mut(), if common { 35 } else { 26 },
            std::ptr::null_mut(), 0, path.as_mut_ptr()) };
        if status < 0 { return Err(native_state_error("native 1009 system folder lookup failed")); }
        return Ok(path[..path.iter().position(|&c| c == 0).unwrap_or(path.len())].to_vec());
    }
    #[cfg(not(windows))] {
        let _ = common;
        Err(native_state_error("native 1009 Windows special folder has no host binding"))
    }
}

/// 492200 leaves a percent byte literal when it is not a complete VM format.
/// Keep folder markers (including markers in recursive %s values) intact;
/// delegate actual numeric formatting to the existing KCS implementation.
fn format_filename(vm: &Vm, input: &[u8], depth: usize) -> Result<Vec<u8>> {
    if depth >= 32 { return Err(native_state_error("native 1009 filename recursion exceeds 32")); }
    let input = trim_nul(input);
    let mut output = Vec::new();
    let mut cursor = 0;
    while cursor < input.len() {
        let start = cursor;
        if input[cursor] != b'%' { output.push(input[cursor]); cursor += 1; continue; }
        if input.get(cursor + 1) == Some(&b'%') { output.push(b'%'); cursor += 2; continue; }
        let mut end = cursor + 1;
        while input.get(end).is_some_and(|b| b.is_ascii_digit() || *b == b'.') { end += 1; }
        let kind = input.get(end).copied().unwrap_or(0);
        let address_start = end + 2;
        let domain = input.get(address_start).copied().unwrap_or(0);
        let mut close = address_start + 1;
        while input.get(close).is_some_and(u8::is_ascii_digit) { close += 1; }
        if !matches!(kind, b's' | b'S' | b'c' | b'f' | b'F' | b'd' | b'i' | b'u' | b'x' | b'X')
            || input.get(end + 1) != Some(&b'[') || !matches!(domain, b'G' | b'L' | b'H')
            || input.get(close) != Some(&b']') {
            output.push(b'%'); cursor += 1; continue;
        }
        if end - start >= 64 { return Err(native_state_error("native 1009 VM format exceeds 63 bytes")); }
        if matches!(kind, b's' | b'S') {
            let relative = super::parse_decimal(&input[address_start + 1..close])
                .ok_or_else(|| native_state_error("native 1009 format address overflow"))? as u32;
            let address = match domain {
                b'L' => (vm.frame_base() as u32).wrapping_add(relative),
                b'H' => 0x8000_0000u32.wrapping_add(relative),
                _ => relative,
            };
            let mut value = format_filename(vm, &vm.read_string(address)?, depth + 1)?;
            let specification = &input[start + 1..end];
            let dot = specification.iter().position(|&b| b == b'.');
            if let Some(dot) = dot {
                let precision = super::parse_decimal(&specification[dot + 1..])
                    .ok_or_else(|| native_state_error("native 1009 format precision overflow"))?;
                value.truncate(precision);
            }
            let width = super::parse_decimal(&specification[..dot.unwrap_or(specification.len())])
                .ok_or_else(|| native_state_error("native 1009 format width overflow"))?;
            super::append_width(&mut output, &value, width, specification.first() == Some(&b'0'));
        } else {
            let mut conversion = input[start..=close].to_vec();
            if kind == b'F' { conversion[end - start] = b'f'; }
            let mut rendered = super::expand_kcs_format(vm, &conversion, depth)?;
            if kind == b'F' { rendered.make_ascii_uppercase(); }
            output.extend(rendered);
        }
        cursor = close + 1;
        if output.len() > 64 * 1024 { return Err(native_state_error("native 1009 formatted filename exceeds 64 KiB")); }
    }
    Ok(output)
}

//! ISLAND's identified translation DLL HG-3 receiver, independently expressed
//! from its fixed byte evidence. No DLL loading, PNG/CRC name aliases, global
//! filename overrides, or retained replacement pixel store.
use cs2_formats::{HgFile, HgFrame};
use cs2_renderer::RgbaFrame;
use cs2_runtime::{ResourceIndex, RuntimeError};
use cs2_script::Result;

use crate::native_state_error;

const GAME: &str = "474cc11d875d7be9df9581f3a55d64698ccf4e760c44013df403cced85e505f1";
const GAME_CHS: &str = "7bd9f6dcc7d62868d4ad9cc68d5580452e71bd907540ec4f993d960d051d2c45";
const DLL: &str = "782674f1eac817b1842f6d0f0d416c17e86fadfb8fb640d335a1da3ce6cabb27";
const PACK: &str = "45e3f1840c6ef15826d85446f6f6f8142be3fd52ddc5c1e87389f625973162a3";
const MAX_BYTES: usize = 64 * 1024 * 1024;
const TO_RGBA: [usize; 4] = [2, 1, 0, 3];

/// This is a compatibility profile for three exact on-disk identities. The
/// plain-file API deliberately bypasses resource aliases and patch mounts.
pub(super) fn identify(resources: &ResourceIndex) -> Result<bool> {
    let mut game = false;
    for (name, expected) in [("ISLAND.exe", GAME), ("ISLAND_CHS.exe", GAME_CHS)] {
        if let Some(bytes) = plain_file(resources, name)?
            && crate::particle_plugin_digest::sha256(&bytes) == expected {
            game = true;
            break;
        }
    }
    if !game { return Ok(false); }
    for (name, expected) in [("Accelerator.dll", DLL), ("cnimage.int", PACK)] {
        let Some(bytes) = plain_file(resources, name)? else { return Ok(false); };
        if crate::particle_plugin_digest::sha256(&bytes) != expected { return Ok(false); }
    }
    let Some(archive) = resources.archive_index("cnimage.int") else {
        return Err(native_state_error("identified ISLAND cnimage.int is absent from archive inventory"));
    };
    if !archive.is_readable() || archive.entries.len() != 458 || archive.is_encrypted() {
        return Err(native_state_error("identified ISLAND FUYINPAK directory is not readable"));
    }
    Ok(true)
}

fn plain_file(resources: &ResourceIndex, name: &str) -> Result<Option<Vec<u8>>> {
    let candidate = resources.resolve_plain_file(name)
        .map_err(|_| native_state_error("ISLAND literal source lookup failed"))?;
    let Some(candidate) = candidate else { return Ok(None); };
    match resources.read_candidate(candidate) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(RuntimeError::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(native_state_error("ISLAND literal source read failed")),
    }
}

/// Original 548f0b invokes the receiver once for each imgNNNN slice, after
/// inverse predictors, before metadata publication/cache insertion. The native
/// image uses bottom-up BGR(A); renderer storage is top-down RGBA.
pub(super) fn apply(resources: &ResourceIndex, encoded: &[u8], id: u32, source: Option<&HgFrame>,
    frame: RgbaFrame, warning: &mut impl FnMut(String)) -> Result<RgbaFrame> {
    let file;
    let source = match source {
        Some(source) => source,
        None => {
            file = HgFile::parse(encoded)
                .map_err(|_| native_state_error("ISLAND HG-3 directory changed after decoding"))?;
            file.frames.iter().find(|source| source.id == id)
                .ok_or_else(|| native_state_error("ISLAND selected HG-3 frame is absent"))?
        }
    };
    // The patched instruction is in the native imgNNNN branch, not img_jpg,
    // img_al or a generic bitmap decoder. Do not apply it to other encodings.
    if source.encoded_image.is_some() { return Ok(frame); }
    let channels = match source.bit_depth as u16 as i16 {
        24 => 3usize, 32 => 4,
        _ => return Err(native_state_error("ISLAND native HG depth is unsupported")),
    };
    let stride = (source.width as usize).checked_mul(channels)
        .ok_or_else(|| native_state_error("ISLAND native HG stride overflows"))?;
    let slices = cs2_formats::hg::native_image_slices(source, encoded)
        .map_err(|_| native_state_error("ISLAND native HG slice directory is invalid"))?;
    let mut pixels = None::<Vec<u8>>;
    for slice in slices {
        // Hook: edx=stride; ecx += edx; edx >>= 2; receiver multiplies
        // edx*slice_rows*4. Preserve the low-bit truncation even for BGR24.
        let length = (stride & !3).checked_mul(slice.row_count as usize)
            .filter(|&n| n <= MAX_BYTES)
            .ok_or_else(|| native_state_error("ISLAND slice receiver exceeds 64 MiB"))?;
        if length == 0 { continue; }
        let original = pixels.as_deref().unwrap_or_else(|| frame.pixels());
        let address = |byte: usize| -> Result<usize> {
            let row = byte / stride;
            let lane = byte % stride;
            let y = (slice.start_row as usize).checked_add(slice.row_count as usize)
                .and_then(|end| end.checked_sub(row + 1))
                .ok_or_else(|| native_state_error("ISLAND slice row escapes its source"))?;
            y.checked_mul(source.width as usize).and_then(|n| n.checked_add(lane / channels))
                .and_then(|n| n.checked_mul(4)).and_then(|n| n.checked_add(TO_RGBA[lane % channels]))
                .filter(|&n| n < frame.pixels().len())
                .ok_or_else(|| native_state_error("ISLAND native byte escapes decoded pixels"))
        };
        let mut crc = !0u32;
        for byte in 0..length {
            crc = (crc >> 8) ^ CRC_TABLE[((crc as u8) ^ original[address(byte)?]) as usize];
        }
        let name = format!("{:08X}.png", !crc);
        let Some(replacement) = replacement(resources, &name, warning)? else { continue; };
        if replacement.pixels().len() < length {
            // The DLL memcpy reads beyond the PNG allocation in this case.
            // Expose that boundary instead of padding or resizing the source.
            warning(format!("{name}: decoded PNG is shorter than original memcpy range {length}"));
            return Err(native_state_error("ISLAND PNG is shorter than the original memcpy range"));
        }
        let destination = pixels.get_or_insert_with(|| frame.pixels().to_vec());
        for byte in 0..length {
            // libpng rows become BGRA in their existing order, then memcpy
            // copies exactly the original slice length. PNG geometry never
            // replaces native HG geometry; a larger PNG contributes a prefix.
            destination[address(byte)?] = replacement.pixels()[byte / 4 * 4 + TO_RGBA[byte % 4]];
        }
    }
    match pixels {
        None => Ok(frame),
        Some(pixels) => RgbaFrame::from_native_rgba8(frame.width(), frame.height(), pixels, frame.metadata())
            .map(|next| next.with_raw_metadata(frame.metadata()).with_hg_attributes(frame.hg_attributes().clone()))
            .map_err(|_| native_state_error("ISLAND translated frame construction failed")),
    }
}

fn replacement(resources: &ResourceIndex, name: &str, warning: &mut impl FnMut(String))
    -> Result<Option<RgbaFrame>> {
    // DLL 10008223..100082b2 attempts literal cnimage\\<CRC>.png first.
    // A failed PNG decode falls through to the exact byte-key archive lookup.
    if let Some(bytes) = plain_file(resources, &format!("cnimage/{name}"))? {
        match png(&bytes) {
            Ok(frame) => return Ok(Some(frame)),
            Err(error) => warning(format!("loose cnimage/{name} rejected: {error}; trying identified pack")),
        }
    }
    let archive = resources.archive_index("cnimage.int")
        .ok_or_else(|| native_state_error("identified ISLAND image archive was removed"))?;
    // 100093d0 stores the original name, 10009c20/10009640 hash those exact
    // bytes with FNV-1a and 10008780 compares raw bytes + length. No unmasking.
    // In particular $RWG4MTC.png is NOT an alias for a hexadecimal CRC key.
    let Some(entry) = archive.entries.iter().find(|entry| entry.raw_name == name.as_bytes()) else {
        return Ok(None);
    };
    let bytes = resources.read_archive_entry_by_index(archive, entry)
        .map_err(|_| native_state_error("ISLAND identified pack entry read failed"))?;
    match png(&bytes) {
        Ok(frame) => Ok(Some(frame)),
        Err(error) => { warning(format!("pack {name}: {error}")); Err(error) }
    }
}

fn png(bytes: &[u8]) -> Result<RgbaFrame> {
    if bytes.len() > MAX_BYTES || bytes.get(..8) != Some(b"\x89PNG\r\n\x1a\n")
        || bytes.get(8..16) != Some(b"\0\0\0\rIHDR") || bytes.len() < 33 {
        return Err(native_state_error("ISLAND replacement is not a bounded PNG"));
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().unwrap());
    let height = u32::from_be_bytes(bytes[20..24].try_into().unwrap());
    if width == 0 || height == 0 || width.checked_mul(height).and_then(|n| n.checked_mul(4))
        .is_none_or(|n| n as u64 > MAX_BYTES as u64)
        || bytes[24] != 8 || bytes[25] != 6 {
        return Err(native_state_error("ISLAND PNG receiver requires bounded RGBA8 (all observed pack entries)"));
    }
    RgbaFrame::decode(bytes).map_err(|_| native_state_error("ISLAND PNG decode failed"))
}

const CRC_TABLE: [u32; 256] = crc_table();
const fn crc_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut index = 0usize;
    while index < 256 {
        let mut lane = index as u32;
        let mut bit = 0;
        while bit < 8 {
            lane = (lane >> 1) ^ if lane & 1 != 0 { 0xedb8_8320 } else { 0 };
            bit += 1;
        }
        table[index] = lane;
        index += 1;
    }
    table
}

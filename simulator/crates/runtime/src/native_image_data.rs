//! Metadata-only ordinary-native 368 parser. It deliberately does not decode
//! pixels or infer metadata from an already rendered image.
use cs2_script::{Result, ScriptError};

pub fn native_image_metadata(bytes: &[u8], frame_id: u32) -> Result<Option<[u32; 9]>> {
    if bytes.starts_with(b"HG-2") {
        return hg2_metadata(bytes, frame_id);
    }
    if bytes.starts_with(&[0xff, 0xd8]) {
        // 0x5fbd00 ignores the requested frame ID and returns JPEG header
        // geometry, offset/base zero and a constant three-byte depth slot.
        let metadata = cs2_formats::JpegMetadata::parse(bytes)
            .map_err(|_| error("native 368 malformed/unsupported JPEG header"))?;
        return Ok(Some([0, 0, metadata.width, metadata.height,
            metadata.width, metadata.height, 3, 0, 0]));
    }
    if !bytes.starts_with(b"HG-3") {
        return Ok(None);
    }
    // The metadata helper always starts at +12; unlike the pixel loader it
    // does not dispatch on HeaderSize/version. Ordinary375 also calls it
    // with only the first 1024 resource bytes.
    let mut frame = 12usize;
    for _ in 0..1_000_000 {
        let next = word(bytes, frame)? as usize;
        let id = word(bytes, frame + 4)?;
        let end = if next == 0 {
            bytes.len()
        } else {
            if next < 8 {
                return Err(error("native 368 invalid HG-3 frame link"));
            }
            frame
                .checked_add(next)
                .ok_or_else(|| error("native 368 HG-3 frame offset overflows"))?
        };
        if id == frame_id {
            // A next-frame link may be beyond this metadata-only prefix.
            let end = end.min(bytes.len());
            let mut tag = frame + 8;
            for _ in 0..1_000_000 {
                let header = range(bytes, tag, 16, end)?;
                let next_tag = word(header, 8)? as usize;
                let length = word(header, 12)? as usize;
                let payload = range(bytes, tag + 16, length, end)?;
                if header[..8].split(|&byte| byte == 0).next() == Some(b"stdinfo".as_slice()) {
                    let depth = i32::from(word(payload, 8)? as u16 as i16) / 8;
                    return Ok(Some([
                        word(payload, 12)?,
                        word(payload, 16)?,
                        word(payload, 0)?,
                        word(payload, 4)?,
                        word(payload, 20)?,
                        word(payload, 24)?,
                        depth as u32,
                        word(payload, 32)?,
                        word(payload, 36)?,
                    ]));
                }
                if next_tag == 0 {
                    return Ok(None);
                }
                if next_tag < 16 || next_tag < 16usize.saturating_add(length) {
                    return Err(error("native 368 invalid HG-3 tag link"));
                }
                tag = tag
                    .checked_add(next_tag)
                    .ok_or_else(|| error("native 368 HG-3 tag offset overflows"))?;
            }
            return Err(error("native 368 HG-3 tag limit exceeded"));
        }
        if next == 0 {
            return Ok(None);
        }
        frame = end;
    }
    Err(error("native 368 HG-3 frame limit exceeded"))
}

fn hg2_metadata(bytes: &[u8], frame_id: u32) -> Result<Option<[u32; 9]>> {
    // 0x5fa140 always starts at file + 12, independent of HeaderSize.
    // It only reads frame fields; compressed payload need not be decoded.
    let version = word(bytes, 8)? as u16;
    if !matches!(version, 0x10 | 0x20 | 0x25) {
        return Err(error("native 368 unsupported HG-2 version"));
    }
    let mut frame = 12usize;
    if version == 0x10 {
        // This branch never reads the requested frame ID.
        let width = word(bytes, frame)?;
        let height = word(bytes, frame + 4)?;
        let depth = i32::from((word(bytes, frame + 8)? >> 16) as u16 as i16) / 8;
        return Ok(Some([0, 0, width, height, width, height, depth as u32, 0, 0]));
    }
    let header_size = if version == 0x25 { 76 } else { 68 };
    for _ in 0..1_000_000 {
        let header = range(bytes, frame, header_size, bytes.len())?;
        if word(header, 40)? == frame_id {
            // The baseline metadata helper uses frame + 0x0a, while the
            // actual pixel loader uses frame + 8. Keep this observed ABI
            // instead of normalizing metadata to rendered pixel bit depth.
            let depth = i32::from((word(header, 8)? >> 16) as u16 as i16) / 8;
            return Ok(Some([
                word(header, 52)?,
                word(header, 56)?,
                word(header, 0)?,
                word(header, 4)?,
                word(header, 44)?,
                word(header, 48)?,
                depth as u32,
                if version == 0x25 { word(header, 68)? } else { 0 },
                if version == 0x25 { word(header, 72)? } else { 0 },
            ]));
        }
        let next = word(header, 64)? as usize;
        if next == 0 { return Ok(None); }
        if next < header_size {
            return Err(error("native 368 HG-2 frame link overlaps its header"));
        }
        frame = frame.checked_add(next).filter(|&next| next < bytes.len())
            .ok_or_else(|| error("native 368 truncated HG-2 next frame"))?;
    }
    Err(error("native 368 HG-2 frame limit exceeded"))
}

fn word(bytes: &[u8], offset: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(
        range(bytes, offset, 4, bytes.len())?.try_into().unwrap(),
    ))
}

fn range(bytes: &[u8], offset: usize, length: usize, limit: usize) -> Result<&[u8]> {
    let end = offset
        .checked_add(length)
        .filter(|&end| end <= limit)
        .ok_or_else(|| error("native 368 image field crosses its frame"))?;
    bytes
        .get(offset..end)
        .ok_or_else(|| error("native 368 truncated image field"))
}

fn error(message: &'static str) -> ScriptError {
    ScriptError::InvalidStructuredText { offset: 0, message }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_keeps_raw_signed_fields_and_selects_first_duplicate_frame() {
        let mut bytes = b"HG-3".to_vec();
        for word in [12u32, 0x300, 64, u32::MAX] {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        bytes.extend_from_slice(b"stdinfo\0");
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&40u32.to_le_bytes());
        for word in [7u32, 9, 0xffe8, 0xffff_fff0, 3, 640, 480, 1, 4, 5] {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        let mut duplicate = bytes[12..].to_vec();
        duplicate[..4].copy_from_slice(&0u32.to_le_bytes());
        duplicate[24..28].copy_from_slice(&777u32.to_le_bytes());
        bytes.extend_from_slice(&duplicate);
        assert_eq!(
            native_image_metadata(&bytes, u32::MAX).unwrap(),
            Some([0xffff_fff0, 3, 7, 9, 640, 480, (-3i32) as u32, 4, 5])
        );
        assert_eq!(native_image_metadata(&bytes, 8).unwrap(), None);
        assert!(native_image_metadata(&bytes[..78], 8).is_err());
        assert!(native_image_metadata(b"HG-2", 0).is_err());
        assert_eq!(native_image_metadata(b"unknown", 0).unwrap(), None);
    }
}

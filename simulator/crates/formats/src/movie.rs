use thiserror::Error;

const PROBE_LIMIT: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MovieTimeBase {
    /// Seconds per nominal video frame, represented as numerator / denominator.
    pub numerator: u32,
    pub denominator: u32,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct MovieMetadata {
    pub video_stream_ids: Vec<u8>,
    pub audio_stream_ids: Vec<u8>,
    pub pes_start_offsets: Vec<usize>,
    pub width: Option<u16>,
    pub height: Option<u16>,
    pub time_base: Option<MovieTimeBase>,
    /// Video PTS span only when the probe reaches the end of the program.
    /// A bounded prefix of a longer movie cannot establish its duration.
    pub duration_seconds: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WebmTrack {
    pub track_type: u64,
    pub codec_id: Vec<u8>,
    pub pixel_width: Option<u64>,
    pub pixel_height: Option<u64>,
    pub audio_sample_rate: Option<f64>,
    pub audio_channels: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct WebmMetadata {
    pub timecode_scale: u64,
    pub duration_seconds: Option<f64>,
    pub tracks: Vec<WebmTrack>,
}

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum MovieError {
    #[error("MPEG program stream input is truncated at byte {offset} (need {required} bytes)")]
    Truncated { offset: usize, required: usize },
    #[error("input does not begin with an MPEG program stream pack header")]
    InvalidSignature,
    #[error("invalid program stream boundary at byte {offset}")]
    InvalidBoundary { offset: usize },
    #[error("unknown program stream start code {code:#04x} at byte {offset}")]
    UnknownStartCode { offset: usize, code: u8 },
    #[error("invalid MPEG pack header at byte {offset}")]
    InvalidPackHeader { offset: usize },
    #[error("invalid packet length at byte {offset}")]
    InvalidPacketLength { offset: usize },
    #[error("invalid PES header at byte {offset}")]
    InvalidPesHeader { offset: usize },
    #[error("no MPEG-1 video PES stream was found in the probe window")]
    MissingVideoStream,
    #[error("invalid MPEG-1 video dimensions at byte {offset}")]
    InvalidDimensions { offset: usize },
    #[error("unknown MPEG-1 frame-rate code {code} at byte {offset}")]
    UnknownFrameRate { offset: usize, code: u8 },
}

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum WebmError {
    #[error("WebM input is truncated")]
    Truncated,
    #[error("invalid EBML/WebM signature")]
    InvalidSignature,
    #[error("invalid EBML element boundary")]
    InvalidBoundary,
    #[error("unsupported EBML integer or float width")]
    InvalidValue,
    #[error("WebM contains no track entries")]
    MissingTracks,
}

pub struct Webm;

impl Webm {
    pub fn probe(data: &[u8]) -> Result<WebmMetadata, WebmError> {
        if data.len() < 4 || data.get(..4) != Some(&[0x1a, 0x45, 0xdf, 0xa3]) {
            return Err(WebmError::InvalidSignature);
        }
        let mut metadata = WebmMetadata {
            timecode_scale: 1_000_000,
            ..WebmMetadata::default()
        };
        parse_ebml_children(data, 0, data.len(), None, &mut metadata, 0)?;
        if metadata.tracks.is_empty() {
            return Err(WebmError::MissingTracks);
        }
        if let Some(duration) = metadata.duration_seconds {
            metadata.duration_seconds =
                Some(duration * metadata.timecode_scale as f64 / 1_000_000_000.0);
        }
        Ok(metadata)
    }
}

fn parse_ebml_children(
    data: &[u8],
    mut cursor: usize,
    end: usize,
    active_track: Option<usize>,
    metadata: &mut WebmMetadata,
    depth: usize,
) -> Result<(), WebmError> {
    if depth > 32 {
        return Err(WebmError::InvalidBoundary);
    }
    while cursor < end {
        let (id, id_length) = read_ebml_id(data, cursor)?;
        cursor = cursor
            .checked_add(id_length)
            .ok_or(WebmError::InvalidBoundary)?;
        let (size, size_length, unknown_size) = read_ebml_size(data, cursor)?;
        cursor = cursor
            .checked_add(size_length)
            .ok_or(WebmError::InvalidBoundary)?;
        let body_end = if unknown_size {
            end
        } else {
            cursor
                .checked_add(usize::try_from(size).map_err(|_| WebmError::InvalidBoundary)?)
                .ok_or(WebmError::InvalidBoundary)?
        };
        if body_end > end || body_end > data.len() {
            return Err(WebmError::Truncated);
        }

        match id {
            0x1853_8067 | 0x1549_a966 | 0x1654_ae6b | 0xae | 0xe0 | 0xe1 => {
                let next_track = if id == 0xae {
                    metadata.tracks.push(WebmTrack {
                        track_type: 0,
                        codec_id: Vec::new(),
                        pixel_width: None,
                        pixel_height: None,
                        audio_sample_rate: None,
                        audio_channels: None,
                    });
                    Some(metadata.tracks.len() - 1)
                } else {
                    active_track
                };
                parse_ebml_children(data, cursor, body_end, next_track, metadata, depth + 1)?;
            }
            0x2a_d7_b1 => {
                metadata.timecode_scale = read_ebml_uint(data, cursor, body_end)?;
            }
            0x44_89 => {
                metadata.duration_seconds = Some(read_ebml_float(data, cursor, body_end)?);
            }
            0x83 => {
                if let Some(index) = active_track {
                    metadata.tracks[index].track_type = read_ebml_uint(data, cursor, body_end)?;
                }
            }
            0x86 => {
                if let Some(index) = active_track {
                    metadata.tracks[index].codec_id = data
                        .get(cursor..body_end)
                        .ok_or(WebmError::Truncated)?
                        .to_vec();
                }
            }
            0xb0 => {
                if let Some(index) = active_track {
                    metadata.tracks[index].pixel_width =
                        Some(read_ebml_uint(data, cursor, body_end)?);
                }
            }
            0xba => {
                if let Some(index) = active_track {
                    metadata.tracks[index].pixel_height =
                        Some(read_ebml_uint(data, cursor, body_end)?);
                }
            }
            0xb5 => {
                if let Some(index) = active_track {
                    metadata.tracks[index].audio_sample_rate =
                        Some(read_ebml_float(data, cursor, body_end)?);
                }
            }
            0x9f => {
                if let Some(index) = active_track {
                    metadata.tracks[index].audio_channels =
                        Some(read_ebml_uint(data, cursor, body_end)?);
                }
            }
            _ => {}
        }
        cursor = body_end;
    }
    Ok(())
}

fn read_ebml_id(data: &[u8], offset: usize) -> Result<(u32, usize), WebmError> {
    let first = *data.get(offset).ok_or(WebmError::Truncated)?;
    let length = ebml_vint_length(first).ok_or(WebmError::InvalidValue)?;
    let bytes = data
        .get(offset..offset + length)
        .ok_or(WebmError::Truncated)?;
    let mut value = 0u32;
    for &byte in bytes {
        value = value.checked_shl(8).ok_or(WebmError::InvalidValue)? | u32::from(byte);
    }
    Ok((value, length))
}

fn read_ebml_size(data: &[u8], offset: usize) -> Result<(u64, usize, bool), WebmError> {
    let first = *data.get(offset).ok_or(WebmError::Truncated)?;
    let length = ebml_vint_length(first).ok_or(WebmError::InvalidValue)?;
    let bytes = data
        .get(offset..offset + length)
        .ok_or(WebmError::Truncated)?;
    let mut value = u64::from(first & (0xff >> length));
    for &byte in &bytes[1..] {
        value = value.checked_shl(8).ok_or(WebmError::InvalidValue)? | u64::from(byte);
    }
    let unknown = value == (1u64 << (7 * length)) - 1;
    Ok((value, length, unknown))
}

fn ebml_vint_length(first: u8) -> Option<usize> {
    (1..=8).find(|length| first & (0x80 >> (length - 1)) != 0)
}

fn read_ebml_uint(data: &[u8], start: usize, end: usize) -> Result<u64, WebmError> {
    let bytes = data.get(start..end).ok_or(WebmError::Truncated)?;
    if bytes.is_empty() || bytes.len() > 8 {
        return Err(WebmError::InvalidValue);
    }
    let mut value = 0u64;
    for &byte in bytes {
        value = value.checked_shl(8).ok_or(WebmError::InvalidValue)? | u64::from(byte);
    }
    Ok(value)
}

fn read_ebml_float(data: &[u8], start: usize, end: usize) -> Result<f64, WebmError> {
    match end.checked_sub(start) {
        Some(4) => Ok(f32::from_be_bytes(
            data[start..end]
                .try_into()
                .map_err(|_| WebmError::InvalidValue)?,
        ) as f64),
        Some(8) => Ok(f64::from_be_bytes(
            data[start..end]
                .try_into()
                .map_err(|_| WebmError::InvalidValue)?,
        )),
        _ => Err(WebmError::InvalidValue),
    }
}

/// Bounded structural probe for MPEG-1 video in an MPEG Program Stream.
///
/// This inspects container and sequence-header metadata only. It does not
/// decode video or audio packets.
pub struct MpegProgramStream;

impl MpegProgramStream {
    pub fn probe(data: &[u8]) -> Result<MovieMetadata, MovieError> {
        if data.len() < 4 {
            if b"\0\0\x01\xba".starts_with(data) {
                return Err(MovieError::Truncated {
                    offset: 0,
                    required: 4,
                });
            }
            return Err(MovieError::InvalidSignature);
        }
        if data.get(..4) != Some(b"\0\0\x01\xba") {
            return Err(MovieError::InvalidSignature);
        }

        let limit = data.len().min(PROBE_LIMIT);
        let mut metadata = MovieMetadata::default();
        let mut first_video_pts = None;
        let mut last_video_pts = None;
        let mut offset = 0usize;
        let mut saw_pack = false;
        let mut reached_program_end = false;

        while offset < limit {
            if limit - offset < 4 {
                if limit == data.len() {
                    return Err(MovieError::Truncated {
                        offset,
                        required: 4,
                    });
                }
                break;
            }
            if data.get(offset..offset + 3) != Some(b"\0\0\x01") {
                return Err(MovieError::InvalidBoundary { offset });
            }
            let code = data[offset + 3];
            match code {
                0xba => {
                    match parse_pack_header(data, offset, limit) {
                        Ok(next) => offset = next,
                        Err(MovieError::Truncated { .. }) if limit < data.len() => break,
                        Err(error) => return Err(error),
                    }
                    saw_pack = true;
                }
                0xb9 => {
                    reached_program_end = true;
                    break;
                }
                0xbb => match skip_length_packet(data, offset, limit) {
                    Ok(next) => offset = next,
                    Err(MovieError::Truncated { .. }) if limit < data.len() => break,
                    Err(error) => return Err(error),
                },
                0xbc | 0xbe | 0xbf | 0xf0..=0xff => {
                    match skip_special_packet(data, offset, limit) {
                        Ok(next) => offset = next,
                        Err(MovieError::Truncated { .. }) if limit < data.len() => break,
                        Err(error) => return Err(error),
                    }
                }
                0xc0..=0xdf | 0xe0..=0xef => {
                    let packet = match parse_pes_packet(data, offset, limit) {
                        Ok(packet) => packet,
                        Err(MovieError::Truncated { .. }) if limit < data.len() => break,
                        Err(error) => return Err(error),
                    };
                    metadata.pes_start_offsets.push(offset);
                    if (0xe0..=0xef).contains(&code) {
                        push_unique(&mut metadata.video_stream_ids, code);
                        if let Some((width, height, time_base, header_offset)) = sequence_header(
                            &data[packet.payload_start..packet.payload_end],
                            packet.payload_start,
                        )? {
                            if width == 0 || height == 0 {
                                return Err(MovieError::InvalidDimensions {
                                    offset: header_offset,
                                });
                            }
                            metadata.width.get_or_insert(width);
                            metadata.height.get_or_insert(height);
                            metadata.time_base.get_or_insert(time_base);
                        }
                        if let Some(pts) = packet.pts {
                            first_video_pts.get_or_insert(pts);
                            last_video_pts = Some(pts);
                        }
                    } else {
                        push_unique(&mut metadata.audio_stream_ids, code);
                    }
                    offset = packet.next_offset;
                }
                _ => return Err(MovieError::UnknownStartCode { offset, code }),
            }
        }

        if !saw_pack {
            return Err(MovieError::InvalidSignature);
        }
        if metadata.video_stream_ids.is_empty() {
            if limit == data.len() {
                return Err(MovieError::MissingVideoStream);
            }
            return Err(MovieError::MissingVideoStream);
        }
        if reached_program_end || offset == data.len() {
            metadata.duration_seconds = first_video_pts
                .zip(last_video_pts)
                .filter(|(first, last)| last >= first && last > first)
                .map(|(first, last)| (last - first) as f64 / 90_000.0);
        }
        Ok(metadata)
    }
}

struct PesPacket {
    payload_start: usize,
    payload_end: usize,
    next_offset: usize,
    pts: Option<u64>,
}

fn parse_pack_header(data: &[u8], offset: usize, limit: usize) -> Result<usize, MovieError> {
    let payload = offset + 4;
    require(data, payload, 1, limit)?;
    let first = data[payload];
    if first >> 6 == 0b01 {
        require(data, payload, 10, limit)?;
        let stuffing = usize::from(data[payload + 9] & 0x07);
        require(data, payload, 10 + stuffing, limit)?;
        Ok(payload + 10 + stuffing)
    } else if first >> 4 == 0b0010 {
        require(data, payload, 8, limit)?;
        Ok(payload + 8)
    } else {
        Err(MovieError::InvalidPackHeader { offset })
    }
}

fn skip_length_packet(data: &[u8], offset: usize, limit: usize) -> Result<usize, MovieError> {
    let length = packet_length(data, offset, limit)?;
    let end = offset
        .checked_add(6)
        .and_then(|start| start.checked_add(length))
        .ok_or(MovieError::InvalidPacketLength { offset })?;
    require_end(data, end, limit)?;
    Ok(end)
}

fn skip_special_packet(data: &[u8], offset: usize, limit: usize) -> Result<usize, MovieError> {
    skip_length_packet(data, offset, limit)
}

fn parse_pes_packet(data: &[u8], offset: usize, limit: usize) -> Result<PesPacket, MovieError> {
    let length = packet_length(data, offset, limit)?;
    let body_start = offset + 6;
    if length == 0 {
        let end = find_next_packet_boundary(data, body_start, limit).unwrap_or(limit);
        let pts = pes_pts(data, body_start, end)?;
        return Ok(PesPacket {
            payload_start: pes_payload_start(data, body_start, end)?,
            payload_end: end,
            next_offset: end,
            pts,
        });
    }

    let end = body_start
        .checked_add(length)
        .ok_or(MovieError::InvalidPacketLength { offset })?;
    require_end(data, end, limit)?;
    let payload_start = pes_payload_start(data, body_start, end)?;
    let pts = pes_pts(data, body_start, end)?;
    Ok(PesPacket {
        payload_start,
        payload_end: end,
        next_offset: end,
        pts,
    })
}

fn pes_payload_start(data: &[u8], start: usize, end: usize) -> Result<usize, MovieError> {
    let first = *data
        .get(start)
        .filter(|_| start < end)
        .ok_or(MovieError::Truncated {
            offset: start,
            required: 1,
        })?;
    if first & 0xc0 == 0x80 {
        if end - start < 3 {
            return Err(MovieError::Truncated {
                offset: start,
                required: 3,
            });
        }
        let header_length = usize::from(data[start + 2]);
        let header_end = start
            .checked_add(3 + header_length)
            .ok_or(MovieError::InvalidPesHeader { offset: start })?;
        if header_end > end {
            return Err(MovieError::Truncated {
                offset: start,
                required: 3 + header_length,
            });
        }
        let pts_dts_flags = (data[start + 1] >> 6) & 0x03;
        if (pts_dts_flags == 0x02 && header_length < 5)
            || (pts_dts_flags == 0x03 && header_length < 10)
            || pts_dts_flags == 0x01
        {
            return Err(MovieError::InvalidPesHeader { offset: start });
        }
        return Ok(header_end);
    }

    let mut cursor = start;
    while cursor < end && data[cursor] == 0xff {
        cursor += 1;
    }
    if cursor + 2 <= end && data[cursor] & 0xc0 == 0x40 {
        cursor += 2;
    }
    if cursor >= end {
        return Err(MovieError::Truncated {
            offset: cursor,
            required: 1,
        });
    }
    match data[cursor] >> 4 {
        0x02 => cursor += 5,
        0x03 => cursor += 10,
        _ if data[cursor] == 0x0f => cursor += 1,
        _ => return Err(MovieError::InvalidPesHeader { offset: cursor }),
    }
    if cursor > end {
        return Err(MovieError::Truncated {
            offset: start,
            required: cursor - start,
        });
    }
    Ok(cursor)
}

fn pes_pts(data: &[u8], start: usize, end: usize) -> Result<Option<u64>, MovieError> {
    let first = *data
        .get(start)
        .filter(|_| start < end)
        .ok_or(MovieError::Truncated {
            offset: start,
            required: 1,
        })?;
    if first & 0xc0 == 0x80 {
        if end - start < 3 {
            return Err(MovieError::Truncated {
                offset: start,
                required: 3,
            });
        }
        let flags = (data[start + 1] >> 6) & 0x03;
        if flags == 0 {
            return Ok(None);
        }
        if flags == 1 || data[start + 2] < 5 {
            return Err(MovieError::InvalidPesHeader { offset: start });
        }
        return Ok(Some(decode_timestamp(data, start + 3)?));
    }

    let mut cursor = start;
    while cursor < end && data[cursor] == 0xff {
        cursor += 1;
    }
    if cursor + 2 <= end && data[cursor] & 0xc0 == 0x40 {
        cursor += 2;
    }
    if cursor >= end {
        return Err(MovieError::Truncated {
            offset: cursor,
            required: 1,
        });
    }
    match data[cursor] >> 4 {
        0x02 | 0x03 => Ok(Some(decode_timestamp(data, cursor)?)),
        0x00 if data[cursor] == 0x0f => Ok(None),
        _ => Err(MovieError::InvalidPesHeader { offset: cursor }),
    }
}

fn decode_timestamp(data: &[u8], offset: usize) -> Result<u64, MovieError> {
    let bytes = data.get(offset..offset + 5).ok_or(MovieError::Truncated {
        offset,
        required: 5,
    })?;
    if bytes[0] & 1 == 0 || bytes[2] & 1 == 0 || bytes[4] & 1 == 0 {
        return Err(MovieError::InvalidPesHeader { offset });
    }
    Ok((u64::from((bytes[0] >> 1) & 0x07) << 30)
        | (u64::from(bytes[1]) << 22)
        | (u64::from(bytes[2] >> 1) << 15)
        | (u64::from(bytes[3]) << 7)
        | u64::from(bytes[4] >> 1))
}

fn sequence_header(
    payload: &[u8],
    payload_offset: usize,
) -> Result<Option<(u16, u16, MovieTimeBase, usize)>, MovieError> {
    let Some(relative) = payload
        .windows(4)
        .position(|window| window == b"\0\0\x01\xb3")
    else {
        return Ok(None);
    };
    let relative_header_offset = relative + 4;
    let header_offset = payload_offset + relative_header_offset;
    let header = payload
        .get(relative_header_offset..relative_header_offset + 8)
        .ok_or(MovieError::Truncated {
            offset: header_offset,
            required: 8,
        })?;
    let width = (u16::from(header[0]) << 4) | u16::from(header[1] >> 4);
    let height = (u16::from(header[1] & 0x0f) << 8) | u16::from(header[2]);
    let frame_rate_code = header[3] & 0x0f;
    let time_base = match frame_rate_code {
        1 => MovieTimeBase {
            numerator: 1001,
            denominator: 24_000,
        },
        2 => MovieTimeBase {
            numerator: 1,
            denominator: 24,
        },
        3 => MovieTimeBase {
            numerator: 1,
            denominator: 25,
        },
        4 => MovieTimeBase {
            numerator: 1001,
            denominator: 30_000,
        },
        5 => MovieTimeBase {
            numerator: 1,
            denominator: 30,
        },
        6 => MovieTimeBase {
            numerator: 1,
            denominator: 50,
        },
        7 => MovieTimeBase {
            numerator: 1001,
            denominator: 60_000,
        },
        8 => MovieTimeBase {
            numerator: 1,
            denominator: 60,
        },
        code => {
            return Err(MovieError::UnknownFrameRate {
                offset: header_offset + 3,
                code,
            });
        }
    };
    Ok(Some((width, height, time_base, header_offset)))
}

fn find_next_packet_boundary(data: &[u8], start: usize, limit: usize) -> Option<usize> {
    (start..limit.saturating_sub(3)).find(|&offset| {
        data.get(offset..offset + 3) == Some(b"\0\0\x01")
            && data
                .get(offset + 3)
                .is_some_and(|code| matches!(*code, 0xb9 | 0xba | 0xbb | 0xbc..=0xff))
    })
}

fn packet_length(data: &[u8], offset: usize, limit: usize) -> Result<usize, MovieError> {
    require(data, offset, 6, limit)?;
    Ok(usize::from(u16::from_be_bytes([
        data[offset + 4],
        data[offset + 5],
    ])))
}

fn require(data: &[u8], offset: usize, length: usize, limit: usize) -> Result<(), MovieError> {
    let end = offset
        .checked_add(length)
        .ok_or(MovieError::InvalidBoundary { offset })?;
    if end > limit {
        if limit == data.len() {
            return Err(MovieError::Truncated {
                offset,
                required: length,
            });
        }
        return Err(MovieError::Truncated {
            offset,
            required: length,
        });
    }
    Ok(())
}

fn require_end(data: &[u8], end: usize, limit: usize) -> Result<(), MovieError> {
    if end > limit {
        return Err(MovieError::Truncated {
            offset: limit,
            required: end - limit,
        });
    }
    if end > data.len() {
        return Err(MovieError::Truncated {
            offset: data.len(),
            required: end - data.len(),
        });
    }
    Ok(())
}

fn push_unique(streams: &mut Vec<u8>, id: u8) {
    if !streams.contains(&id) {
        streams.push(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pack_header() -> Vec<u8> {
        vec![0, 0, 1, 0xba, 0x21, 0, 1, 0, 1, 0x80, 0, 0]
    }

    fn pts(value: u64, prefix: u8) -> [u8; 5] {
        [
            (prefix << 4) | (((value >> 30) as u8 & 7) << 1) | 1,
            (value >> 22) as u8,
            (((value >> 15) as u8 & 0x7f) << 1) | 1,
            (value >> 7) as u8,
            ((value as u8 & 0x7f) << 1) | 1,
        ]
    }

    fn pes(stream_id: u8, pts_value: u64, payload: &[u8]) -> Vec<u8> {
        let mut body = vec![0x80, 0x80, 5];
        body.extend_from_slice(&pts(pts_value, 0x02));
        body.extend_from_slice(payload);
        let mut packet = vec![0, 0, 1, stream_id];
        packet.extend_from_slice(&(body.len() as u16).to_be_bytes());
        packet.extend_from_slice(&body);
        packet
    }

    fn video_payload(frame_rate: u8) -> Vec<u8> {
        vec![
            0,
            0,
            1,
            0xb3,
            0x02,
            0x00,
            0x20,
            0x10 | frame_rate,
            0,
            0,
            0,
            0,
        ]
    }

    #[test]
    fn probes_stream_ids_sequence_metadata_and_pts_duration() {
        let mut bytes = pack_header();
        bytes.extend(pes(0xe0, 90_000, &video_payload(3)));
        bytes.extend(pes(0xc1, 90_000, b"audio"));
        bytes.extend(pes(0xe0, 180_000, b"\0\0\x01\0"));

        let metadata = MpegProgramStream::probe(&bytes).unwrap();

        assert_eq!(metadata.video_stream_ids, [0xe0]);
        assert_eq!(metadata.audio_stream_ids, [0xc1]);
        assert_eq!((metadata.width, metadata.height), (Some(32), Some(32)));
        assert_eq!(
            metadata.time_base,
            Some(MovieTimeBase {
                numerator: 1,
                denominator: 25
            })
        );
        assert_eq!(metadata.duration_seconds, Some(1.0));
    }

    #[test]
    fn reports_signature_unknown_code_and_truncated_boundaries() {
        assert_eq!(
            MpegProgramStream::probe(b"not a stream"),
            Err(MovieError::InvalidSignature)
        );
        assert!(matches!(
            MpegProgramStream::probe(&[0, 0, 1]),
            Err(MovieError::Truncated { .. })
        ));

        let mut unknown = pack_header();
        unknown.extend_from_slice(&[0, 0, 1, 0xa0]);
        assert!(matches!(
            MpegProgramStream::probe(&unknown),
            Err(MovieError::UnknownStartCode {
                code: 0xa0,
                offset: 12
            })
        ));

        let mut truncated = pack_header();
        truncated.extend_from_slice(&[0, 0, 1, 0xe0, 0, 20, 0x80]);
        assert!(matches!(
            MpegProgramStream::probe(&truncated),
            Err(MovieError::Truncated { .. })
        ));
    }

    #[test]
    fn rejects_invalid_frame_rate_and_missing_video() {
        let mut invalid_rate = pack_header();
        invalid_rate.extend(pes(0xe0, 0, &video_payload(0)));
        assert!(matches!(
            MpegProgramStream::probe(&invalid_rate),
            Err(MovieError::UnknownFrameRate { code: 0, .. })
        ));

        let mut audio_only = pack_header();
        audio_only.extend(pes(0xc0, 0, b"audio"));
        assert_eq!(
            MpegProgramStream::probe(&audio_only),
            Err(MovieError::MissingVideoStream)
        );
    }
}

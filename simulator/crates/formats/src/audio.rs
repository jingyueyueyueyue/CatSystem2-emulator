use std::io::Cursor;

use lewton::inside_ogg::OggStreamReader;
use thiserror::Error;

use crate::util::MAX_BUFFER;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioMetadata {
    pub sample_rate: u32,
    pub channels: u8,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PcmAudio<T> {
    pub metadata: AudioMetadata,
    /// Number of sample frames per channel.
    pub frames: usize,
    /// Interleaved samples in frame-major, channel-minor order.
    pub samples: Vec<T>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WavPcmMetadata {
    pub audio_format: u16,
    pub channels: u16,
    pub sample_rate: u32,
    pub byte_rate: u32,
    pub block_align: u16,
    pub bits_per_sample: u16,
    pub data_offset: usize,
    pub data_length: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WavPcm {
    pub metadata: WavPcmMetadata,
    pub samples: Vec<i16>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum WavPcmError {
    #[error("WAV input is truncated")]
    Truncated,
    #[error("invalid RIFF/WAVE signature")]
    InvalidSignature,
    #[error("WAV chunk boundary is invalid")]
    InvalidChunk,
    #[error("unsupported WAV format")]
    UnsupportedFormat,
    #[error("WAV sample data is not aligned")]
    InvalidSampleAlignment,
}

#[derive(Debug, Error)]
pub enum OggVorbisError {
    #[error("Ogg/Vorbis input is truncated")]
    Truncated,
    #[error("invalid Ogg page structure")]
    InvalidOgg,
    #[error("Ogg input exceeds the format reader limit")]
    TooLarge,
    #[error(
        "Ogg page {page} has invalid CRC (expected {expected:#010x}, calculated {actual:#010x})"
    )]
    PageChecksum {
        page: usize,
        expected: u32,
        actual: u32,
    },
    #[error("Ogg page {page} sequence is {actual}, expected {expected}")]
    PageSequence {
        page: usize,
        expected: u32,
        actual: u32,
    },
    #[error("Ogg page {page} has invalid stream boundary flags")]
    InvalidPageFlags { page: usize },
    #[error("Ogg page {page} granule position moved backwards")]
    InvalidGranulePosition { page: usize },
    #[error("invalid Vorbis header: {0}")]
    InvalidVorbisHeader(#[source] lewton::header::HeaderReadError),
    #[error("invalid Vorbis audio packet: {0}")]
    InvalidAudioPacket(#[source] lewton::audio::AudioReadError),
    #[error("Ogg decoding failed: {0}")]
    OggDecode(#[source] lewton::OggReadError),
    #[error("decoded PCM length is not divisible by the channel count")]
    InvalidPcmLength,
    #[error("invalid Vorbis stream metadata: {sample_rate} Hz, {channels} channels")]
    InvalidMetadata { sample_rate: u32, channels: u8 },
}

/// Portable in-memory Ogg/Vorbis metadata and PCM decoder.
pub struct OggVorbis;

impl WavPcm {
    pub fn metadata(data: &[u8]) -> Result<WavPcmMetadata, WavPcmError> {
        parse_wav_pcm_metadata(data)
    }

    pub fn parse(data: &[u8]) -> Result<Self, WavPcmError> {
        let metadata = parse_wav_pcm_metadata(data)?;
        let bytes = data
            .get(metadata.data_offset..metadata.data_offset + metadata.data_length)
            .ok_or(WavPcmError::Truncated)?;
        if bytes.len() % 2 != 0 {
            return Err(WavPcmError::InvalidSampleAlignment);
        }
        let samples = bytes
            .chunks_exact(2)
            .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]))
            .collect();
        Ok(Self { metadata, samples })
    }
}

fn parse_wav_pcm_metadata(data: &[u8]) -> Result<WavPcmMetadata, WavPcmError> {
    if data.len() < 12 {
        return Err(WavPcmError::Truncated);
    }
    if &data[..4] != b"RIFF" || &data[8..12] != b"WAVE" {
        return Err(WavPcmError::InvalidSignature);
    }
    let riff_length = u32::from_le_bytes(data[4..8].try_into().unwrap()) as usize;
    let riff_end = 8usize
        .checked_add(riff_length)
        .ok_or(WavPcmError::InvalidChunk)?;
    if riff_end > data.len() {
        return Err(WavPcmError::Truncated);
    }

    let mut cursor = 12usize;
    let mut format = None;
    let mut data_chunk = None;
    while cursor < riff_end {
        let header_end = cursor.checked_add(8).ok_or(WavPcmError::InvalidChunk)?;
        let header = data.get(cursor..header_end).ok_or(WavPcmError::Truncated)?;
        let length = u32::from_le_bytes(header[4..8].try_into().unwrap()) as usize;
        let body_start = header_end;
        let body_end = body_start
            .checked_add(length)
            .ok_or(WavPcmError::InvalidChunk)?;
        let padded_end = body_end
            .checked_add(length & 1)
            .ok_or(WavPcmError::InvalidChunk)?;
        if padded_end > riff_end || body_end > data.len() {
            return Err(WavPcmError::Truncated);
        }
        match &header[..4] {
            b"fmt " if length >= 16 => {
                let body = &data[body_start..body_end];
                format = Some((
                    u16::from_le_bytes(body[0..2].try_into().unwrap()),
                    u16::from_le_bytes(body[2..4].try_into().unwrap()),
                    u32::from_le_bytes(body[4..8].try_into().unwrap()),
                    u32::from_le_bytes(body[8..12].try_into().unwrap()),
                    u16::from_le_bytes(body[12..14].try_into().unwrap()),
                    u16::from_le_bytes(body[14..16].try_into().unwrap()),
                ));
            }
            b"fmt " => return Err(WavPcmError::InvalidChunk),
            b"data" if data_chunk.is_none() => data_chunk = Some((body_start, length)),
            _ => {}
        }
        cursor = padded_end;
    }

    let (audio_format, channels, sample_rate, byte_rate, block_align, bits_per_sample) =
        format.ok_or(WavPcmError::InvalidChunk)?;
    let (data_offset, data_length) = data_chunk.ok_or(WavPcmError::InvalidChunk)?;
    if audio_format != 1
        || channels == 0
        || sample_rate == 0
        || bits_per_sample != 16
        || block_align != channels.saturating_mul(2)
        || byte_rate != sample_rate.saturating_mul(u32::from(block_align))
    {
        return Err(WavPcmError::UnsupportedFormat);
    }
    if data_length % usize::from(block_align) != 0 {
        return Err(WavPcmError::InvalidSampleAlignment);
    }
    Ok(WavPcmMetadata {
        audio_format,
        channels,
        sample_rate,
        byte_rate,
        block_align,
        bits_per_sample,
        data_offset,
        data_length,
    })
}

impl OggVorbis {
    pub fn metadata(data: &[u8]) -> Result<AudioMetadata, OggVorbisError> {
        Self::probe(data, true)
    }

    pub fn metadata_at_entry_boundary(data: &[u8]) -> Result<AudioMetadata, OggVorbisError> {
        Self::probe(data, false)
    }

    fn probe(data: &[u8], require_eos: bool) -> Result<AudioMetadata, OggVorbisError> {
        validate_pages(data, require_eos)?;
        let reader = OggStreamReader::new(Cursor::new(data)).map_err(map_decode_error)?;
        let metadata = AudioMetadata {
            sample_rate: reader.ident_hdr.audio_sample_rate,
            channels: reader.ident_hdr.audio_channels,
        };
        validate_metadata(metadata)?;
        Ok(metadata)
    }

    pub fn decode_i16(data: &[u8]) -> Result<PcmAudio<i16>, OggVorbisError> {
        let mut reader = open_reader(data)?;
        let metadata = AudioMetadata {
            sample_rate: reader.ident_hdr.audio_sample_rate,
            channels: reader.ident_hdr.audio_channels,
        };
        validate_metadata(metadata)?;
        let mut samples = Vec::new();
        while let Some(mut packet) = reader.read_dec_packet_itl().map_err(map_decode_error)? {
            samples.append(&mut packet);
        }
        let frames = frame_count(samples.len(), metadata.channels)?;
        Ok(PcmAudio {
            metadata,
            frames,
            samples,
        })
    }

    pub fn decode_f32(data: &[u8]) -> Result<PcmAudio<f32>, OggVorbisError> {
        let mut reader = open_reader(data)?;
        let metadata = AudioMetadata {
            sample_rate: reader.ident_hdr.audio_sample_rate,
            channels: reader.ident_hdr.audio_channels,
        };
        validate_metadata(metadata)?;
        let mut samples = Vec::new();
        while let Some(mut packet) = reader
            .read_dec_packet_generic::<lewton::samples::InterleavedSamples<f32>>()
            .map_err(map_decode_error)?
        {
            samples.append(&mut packet.samples);
        }
        let frames = frame_count(samples.len(), metadata.channels)?;
        Ok(PcmAudio {
            metadata,
            frames,
            samples,
        })
    }
}

fn open_reader(data: &[u8]) -> Result<OggStreamReader<Cursor<&[u8]>>, OggVorbisError> {
    validate_pages(data, true)?;
    Ok(OggStreamReader::new(Cursor::new(data)).map_err(map_decode_error)?)
}

fn map_decode_error(error: lewton::VorbisError) -> OggVorbisError {
    match error {
        lewton::VorbisError::BadHeader(source) => OggVorbisError::InvalidVorbisHeader(source),
        lewton::VorbisError::BadAudio(source) => OggVorbisError::InvalidAudioPacket(source),
        lewton::VorbisError::OggError(source) => {
            if matches!(
                &source,
                lewton::OggReadError::ReadError(error)
                    if error.kind() == std::io::ErrorKind::UnexpectedEof
            ) {
                OggVorbisError::Truncated
            } else {
                OggVorbisError::OggDecode(source)
            }
        }
    }
}

fn frame_count(sample_count: usize, channels: u8) -> Result<usize, OggVorbisError> {
    let channels = usize::from(channels);
    if channels == 0 || sample_count % channels != 0 {
        return Err(OggVorbisError::InvalidPcmLength);
    }
    Ok(sample_count / channels)
}

fn validate_metadata(metadata: AudioMetadata) -> Result<(), OggVorbisError> {
    if metadata.sample_rate == 0 || !matches!(metadata.channels, 1 | 2) {
        return Err(OggVorbisError::InvalidMetadata {
            sample_rate: metadata.sample_rate,
            channels: metadata.channels,
        });
    }
    Ok(())
}

fn validate_pages(data: &[u8], require_eos: bool) -> Result<(), OggVorbisError> {
    if data.len() < 27 {
        return Err(OggVorbisError::Truncated);
    }
    if data.len() > MAX_BUFFER {
        return Err(OggVorbisError::TooLarge);
    }

    let mut offset = 0usize;
    let mut page_index = 0usize;
    let mut stream_serial = None;
    let mut expected_sequence = None;
    let mut previous_continued = false;
    let mut previous_granule = None;
    let mut saw_end = false;
    let mut complete_timed_packet = false;
    while offset < data.len() {
        if saw_end {
            return Err(OggVorbisError::InvalidOgg);
        }
        let header_end = offset.checked_add(27).ok_or(OggVorbisError::InvalidOgg)?;
        let header = data
            .get(offset..header_end)
            .ok_or(OggVorbisError::Truncated)?;
        if &header[..4] != b"OggS" || header[4] != 0 {
            return Err(OggVorbisError::InvalidOgg);
        }
        let flags = header[5];
        if flags & !0x07 != 0
            || (flags & 0x01 != 0) != previous_continued
            || (page_index == 0 && (flags & 0x02 == 0 || flags & 0x01 != 0))
            || (page_index != 0 && flags & 0x02 != 0)
        {
            return Err(OggVorbisError::InvalidPageFlags { page: page_index });
        }
        let serial = u32::from_le_bytes(header[14..18].try_into().unwrap());
        if stream_serial.is_some_and(|expected| expected != serial) {
            return Err(OggVorbisError::InvalidOgg);
        }
        stream_serial = Some(serial);
        let sequence = u32::from_le_bytes(header[18..22].try_into().unwrap());
        if let Some(expected) = expected_sequence
            && sequence != expected
        {
            return Err(OggVorbisError::PageSequence {
                page: page_index,
                expected,
                actual: sequence,
            });
        }
        expected_sequence = Some(sequence.wrapping_add(1));

        let segment_count = usize::from(header[26]);
        let table_end = header_end
            .checked_add(segment_count)
            .ok_or(OggVorbisError::InvalidOgg)?;
        let lacing = data
            .get(header_end..table_end)
            .ok_or(OggVorbisError::Truncated)?;
        let body_len = lacing.iter().try_fold(0usize, |sum, &lace| {
            sum.checked_add(usize::from(lace))
                .ok_or(OggVorbisError::InvalidOgg)
        })?;
        offset = table_end
            .checked_add(body_len)
            .ok_or(OggVorbisError::InvalidOgg)?;
        if offset > data.len() {
            return Err(OggVorbisError::Truncated);
        }
        previous_continued = lacing.last().is_some_and(|&lace| lace == 255);
        if flags & 0x04 != 0 && previous_continued {
            return Err(OggVorbisError::InvalidPageFlags { page: page_index });
        }

        let granule = u64::from_le_bytes(header[6..14].try_into().unwrap());
        complete_timed_packet = granule != u64::MAX
            && granule != 0
            && lacing.iter().any(|&lace| lace != 255);
        if granule != u64::MAX {
            if previous_granule.is_some_and(|previous| granule < previous) {
                return Err(OggVorbisError::InvalidGranulePosition { page: page_index });
            }
            previous_granule = Some(granule);
        }

        let expected_crc = u32::from_le_bytes(header[22..26].try_into().unwrap());
        let actual_crc = ogg_crc32_page(&data[offset - (27 + segment_count + body_len)..offset]);
        if actual_crc != expected_crc {
            return Err(OggVorbisError::PageChecksum {
                page: page_index,
                expected: expected_crc,
                actual: actual_crc,
            });
        }
        saw_end = flags & 0x04 != 0;
        page_index += 1;
    }

    if !saw_end && (require_eos || !complete_timed_packet) {
        return Err(OggVorbisError::Truncated);
    }
    Ok(())
}

const fn ogg_crc_table() -> [u32; 256] {
    let mut table = [0; 256];
    let mut index = 0;
    while index < table.len() {
        let mut value = (index as u32) << 24;
        let mut bit = 0;
        while bit < 8 {
            value = if value & 0x8000_0000 != 0 {
                (value << 1) ^ 0x04c1_1db7
            } else {
                value << 1
            };
            bit += 1;
        }
        table[index] = value;
        index += 1;
    }
    table
}

const OGG_CRC_TABLE: [u32; 256] = ogg_crc_table();

fn ogg_crc32_page(page: &[u8]) -> u32 {
    page.iter().enumerate().fold(0, |crc, (index, &byte)| {
        let byte = if (22..26).contains(&index) { 0 } else { byte };
        (crc << 8) ^ OGG_CRC_TABLE[((crc >> 24) as u8 ^ byte) as usize]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ogg_page(flags: u8, sequence: u32, granule: u64, lacing: &[u8], body: &[u8]) -> Vec<u8> {
        let mut page = b"OggS".to_vec();
        page.push(0);
        page.push(flags);
        page.extend_from_slice(&granule.to_le_bytes());
        page.extend_from_slice(&1u32.to_le_bytes());
        page.extend_from_slice(&sequence.to_le_bytes());
        page.extend_from_slice(&[0; 4]);
        page.push(lacing.len() as u8);
        page.extend_from_slice(lacing);
        page.extend_from_slice(body);
        let crc = ogg_crc32_page(&page);
        page[22..26].copy_from_slice(&crc.to_le_bytes());
        page
    }

    #[test]
    fn categorizes_invalid_vorbis_headers_explicitly() {
        let error = map_decode_error(lewton::VorbisError::BadHeader(
            lewton::header::HeaderReadError::NotVorbisHeader,
        ));
        assert!(matches!(
            error,
            OggVorbisError::InvalidVorbisHeader(lewton::header::HeaderReadError::NotVorbisHeader)
        ));
    }

    #[test]
    fn validates_ogg_page_crc_sequence_continuations_and_end_boundary() {
        let first = ogg_page(0x02, 0, 0, &[2], b"ab");
        let last = ogg_page(0x04, 1, 4, &[2], b"cd");
        let valid = [first.as_slice(), last.as_slice()].concat();
        assert!(validate_pages(&valid, true).is_ok());

        let mut bad_crc = valid.clone();
        *bad_crc.last_mut().unwrap() ^= 1;
        assert!(matches!(
            validate_pages(&bad_crc, true),
            Err(OggVorbisError::PageChecksum { page: 1, .. })
        ));

        let skipped_sequence = [
            first.as_slice(),
            ogg_page(0x04, 3, 4, &[2], b"cd").as_slice(),
        ]
        .concat();
        assert!(matches!(
            validate_pages(&skipped_sequence, true),
            Err(OggVorbisError::PageSequence { page: 1, .. })
        ));

        let unpaired_continuation = [
            first.as_slice(),
            ogg_page(0x05, 1, 4, &[2], b"cd").as_slice(),
        ]
        .concat();
        assert!(matches!(
            validate_pages(&unpaired_continuation, true),
            Err(OggVorbisError::InvalidPageFlags { page: 1 })
        ));

        let missing_end = ogg_page(0x02, 0, 0, &[2], b"ab");
        assert!(matches!(
            validate_pages(&missing_end, true),
            Err(OggVorbisError::Truncated)
        ));
    }

    #[test]
    fn bounded_ogg_entry_may_end_after_a_complete_timed_packet_without_eos() {
        let first = ogg_page(0x02, 0, 0, &[2], b"ab");
        let complete = [first.as_slice(), &ogg_page(0, 1, 4, &[2], b"cd")].concat();
        assert!(matches!(validate_pages(&complete, true), Err(OggVorbisError::Truncated)));
        assert!(validate_pages(&complete, false).is_ok());
        assert!(matches!(validate_pages(&complete[..complete.len() - 1], false), Err(OggVorbisError::Truncated)));
        let continuing = [first.as_slice(), &ogg_page(0, 1, 4, &[255], &[0; 255])].concat();
        assert!(matches!(validate_pages(&continuing, false), Err(OggVorbisError::Truncated)));
        let timed_with_tail = [first.as_slice(), &ogg_page(0, 1, 4, &[2, 255], &[0; 257])].concat();
        assert!(validate_pages(&timed_with_tail, false).is_ok());
        assert!(matches!(validate_pages(&timed_with_tail, true), Err(OggVorbisError::Truncated)));
        assert!(matches!(validate_pages(&timed_with_tail[..timed_with_tail.len() - 1], false), Err(OggVorbisError::Truncated)));
        let invalid_end = [first.as_slice(), &ogg_page(4, 1, 4, &[2, 255], &[0; 257])].concat();
        assert!(matches!(validate_pages(&invalid_end, false), Err(OggVorbisError::InvalidPageFlags { page: 1 })));
        let mut corrupt = complete;
        *corrupt.last_mut().unwrap() ^= 1;
        assert!(matches!(validate_pages(&corrupt, false), Err(OggVorbisError::PageChecksum { page: 1, .. })));
    }
}

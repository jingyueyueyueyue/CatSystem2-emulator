//! Independently implemented RIFF/WAVE PCM, IEEE float and Microsoft ADPCM.
//! Format facts: Microsoft RIFF/WAVE Multimedia Standards, ADPCMWAVEFORMAT.
//! No codec implementation or reference-engine source has been copied.
use cs2_runtime::RuntimeResult;

use crate::audio_decode::AudioCodec;
use crate::media_process::error;

const MAX_SAMPLES: usize = 32 * 1024 * 1024;
const ADAPTATION: [i64; 16] = [
    230, 230, 230, 230, 307, 409, 512, 614, 768, 614, 512, 409, 307, 230, 230, 230,
];

pub(super) fn decode(bytes: &[u8]) -> RuntimeResult<(AudioCodec, u32, u8, Vec<i16>)> {
    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(error("WAV source has no RIFF/WAVE header"));
    }
    let end = (word32(bytes, 4)? as usize)
        .checked_add(8)
        .filter(|&end| end <= bytes.len() && end >= 12)
        .ok_or_else(|| error("WAV RIFF range is truncated"))?;
    let mut cursor = 12;
    let mut format = None;
    let mut data = None;
    let mut fact = None;
    while cursor < end {
        let header_end = cursor
            .checked_add(8)
            .filter(|&end| end <= bytes.len())
            .ok_or_else(|| error("WAV chunk header is truncated"))?;
        if header_end > end {
            return Err(error("WAV chunk header exceeds RIFF"));
        }
        let size = word32(bytes, cursor + 4)? as usize;
        let body_end = header_end
            .checked_add(size)
            .filter(|&value| value <= end)
            .ok_or_else(|| error("WAV chunk exceeds RIFF"))?;
        let body = &bytes[header_end..body_end];
        match &bytes[cursor..cursor + 4] {
            b"fmt " if format.is_none() => format = Some(body),
            b"data" if data.is_none() => data = Some(body),
            b"fact" if fact.is_none() => fact = Some(word32(body, 0)? as usize),
            _ => {}
        }
        cursor = body_end
            .checked_add(size & 1)
            .filter(|&value| value <= end)
            .ok_or_else(|| error("WAV chunk padding exceeds RIFF"))?;
    }
    let format = format.ok_or_else(|| error("WAV fmt chunk is absent"))?;
    let data = data.ok_or_else(|| error("WAV data chunk is absent"))?;
    if format.len() < 16 {
        return Err(error("WAV fmt chunk is shorter than16 bytes"));
    }
    let mut tag = word16(format, 0)?;
    let channels = word16(format, 2)?;
    let rate = word32(format, 4)?;
    let average = word32(format, 8)?;
    let align = usize::from(word16(format, 12)?);
    let bits = word16(format, 14)?;
    if !matches!(channels, 1 | 2) || !(1..=384_000).contains(&rate) || align == 0 {
        return Err(error(
            "WAV sample rate/channel count/block alignment is unsupported",
        ));
    }
    let channels = channels as u8;
    let mut valid_bits = bits;
    if tag == 0xfffe {
        if format.len() < 40 || word16(format, 16)? < 22 {
            return Err(error("WAV extensible format is truncated"));
        }
        // KSDATAFORMAT_SUBTYPE_PCM / IEEE_FLOAT share the standard GUID tail.
        if format[26..40] != [0, 0, 0, 0, 0x10, 0, 0x80, 0, 0, 0xaa, 0, 0x38, 0x9b, 0x71] {
            return Err(error("WAV extensible subformat GUID is unsupported"));
        }
        tag = word16(format, 24)?;
        valid_bits = word16(format, 18)?;
        if valid_bits == 0 {
            valid_bits = bits;
        }
        if valid_bits > bits || valid_bits == 0 {
            return Err(error(
                "WAV extensible valid bits exceed their sample container",
            ));
        }
        let mask = word32(format, 20)?;
        if mask != 0 && mask != if channels == 1 { 4 } else { 3 } {
            return Err(error(
                "WAV extensible channel mask has no supported mono/stereo mapping",
            ));
        }
    }
    if tag == 2 {
        return decode_adpcm(format, data, fact, rate, channels, align, bits);
    }
    if !(tag == 1 && matches!(bits, 8 | 16 | 24 | 32) || tag == 3 && matches!(bits, 32 | 64)) {
        return Err(error(format!(
            "unsupported WAV encoding tag={tag} bits={bits}"
        )));
    }
    let width = usize::from(bits / 8);
    if align != width * usize::from(channels)
        || u64::from(average) != u64::from(rate) * align as u64
        || data.len() % align != 0
    {
        return Err(error(
            "WAV uncompressed block alignment/byte rate is invalid",
        ));
    }
    if tag == 3 && valid_bits != bits {
        return Err(error(
            "WAV floating-point valid bits differ from their container",
        ));
    }
    let available = data.len() / align;
    let frames = fact.unwrap_or(available);
    if frames > available {
        return Err(error("WAV fact declares samples beyond data"));
    }
    let count = frames
        .checked_mul(usize::from(channels))
        .filter(|&count| count <= MAX_SAMPLES)
        .ok_or_else(|| error("WAV decoded samples exceed64 MiB"))?;
    let mut output = Vec::new();
    output
        .try_reserve_exact(count)
        .map_err(|_| error("allocate bounded WAV PCM"))?;
    for sample in data[..frames * align].chunks_exact(width) {
        let value = if tag == 3 {
            let value = if width == 4 {
                f64::from(f32::from_le_bytes(sample.try_into().unwrap()))
            } else {
                f64::from_le_bytes(sample.try_into().unwrap())
            };
            if !value.is_finite() {
                return Err(error("WAV floating-point sample is not finite"));
            }
            (value.clamp(-1., 1.) * 32768.)
                .round()
                .clamp(-32768., 32767.) as i16
        } else {
            // Valid bits in WAVE_FORMAT_EXTENSIBLE are left aligned. Taking
            // the high16 bits also preserves24-valid-in32 precision correctly.
            match width {
                1 => ((i16::from(sample[0]) - 128) << 8) as i16,
                2 => i16::from_le_bytes(sample.try_into().unwrap()),
                3 => {
                    let word = i32::from(sample[0])
                        | i32::from(sample[1]) << 8
                        | i32::from(sample[2]) << 16;
                    ((word << 8) >> 16) as i16
                }
                4 => (i32::from_le_bytes(sample.try_into().unwrap()) >> 16) as i16,
                _ => unreachable!(),
            }
        };
        output.push(value);
    }
    Ok((
        if tag == 3 {
            AudioCodec::WavFloat
        } else {
            AudioCodec::WavPcm
        },
        rate,
        channels,
        output,
    ))
}

fn decode_adpcm(
    format: &[u8],
    data: &[u8],
    fact: Option<usize>,
    rate: u32,
    channels: u8,
    align: usize,
    bits: u16,
) -> RuntimeResult<(AudioCodec, u32, u8, Vec<i16>)> {
    let channels = usize::from(channels);
    if bits != 4 || format.len() < 22 {
        return Err(error("Microsoft ADPCM format extension is absent"));
    }
    let extra = usize::from(word16(format, 16)?);
    if extra < 4 || extra > format.len() - 18 {
        return Err(error("Microsoft ADPCM extension size is invalid"));
    }
    let frames_per_block = usize::from(word16(format, 18)?);
    let coefficients = usize::from(word16(format, 20)?);
    if coefficients == 0 || coefficients > 256 || 4 + coefficients * 4 > extra {
        return Err(error("Microsoft ADPCM coefficient table is truncated"));
    }
    let header = 7 * channels;
    if align < header
        || frames_per_block < 2
        || frames_per_block > 2 + (align - header) * 2 / channels
    {
        return Err(error("Microsoft ADPCM block geometry is invalid"));
    }
    let mut pairs = Vec::with_capacity(coefficients);
    for coefficient in 0..coefficients {
        pairs.push((
            word16(format, 22 + coefficient * 4)? as i16,
            word16(format, 24 + coefficient * 4)? as i16,
        ));
    }
    let full = data.len() / align;
    let tail = data.len() % align;
    if tail != 0 && tail < header {
        return Err(error("Microsoft ADPCM final block header is truncated"));
    }
    let maximum = full
        .checked_mul(frames_per_block)
        .and_then(|n| {
            n.checked_add(if tail == 0 {
                0
            } else {
                frames_per_block.min(2 + (tail - header) * 2 / channels)
            })
        })
        .ok_or_else(|| error("Microsoft ADPCM frame count overflow"))?;
    let frames = match fact {
        None => maximum,
        Some(declared) if declared <= maximum => declared,
        Some(declared)
            if full > 0
                && declared % frames_per_block == 0
                && declared - maximum <= frames_per_block =>
        {
            maximum
        }
        Some(_) => return Err(error("Microsoft ADPCM fact exceeds encoded blocks")),
    };
    let count = frames
        .checked_mul(channels)
        .filter(|&value| value <= MAX_SAMPLES)
        .ok_or_else(|| error("Microsoft ADPCM decoded samples exceed64 MiB"))?;
    let mut output = Vec::new();
    output
        .try_reserve_exact(count)
        .map_err(|_| error("allocate bounded ADPCM PCM"))?;
    for block in data.chunks(align) {
        let block_frames = frames_per_block
            .min(2 + (block.len() - header) * 2 / channels)
            .min((count - output.len()) / channels);
        if block_frames == 0 {
            break;
        }
        let mut states = Vec::with_capacity(channels);
        for channel in 0..channels {
            let predictor = usize::from(block[channel]);
            let &(first, second) = pairs
                .get(predictor)
                .ok_or_else(|| error("Microsoft ADPCM predictor exceeds coefficient table"))?;
            states.push(AdpcmState {
                first: i64::from(first),
                second: i64::from(second),
                delta: i64::from(word16(block, channels + channel * 2)?).max(16),
                recent: i64::from(word16(block, 3 * channels + channel * 2)? as i16),
                previous: i64::from(word16(block, 5 * channels + channel * 2)? as i16),
            });
        }
        for state in &states {
            output.push(state.previous as i16);
        }
        if block_frames == 1 {
            continue;
        }
        for state in &states {
            output.push(state.recent as i16);
        }
        let mut frame = 2;
        for &byte in &block[header..] {
            if frame >= block_frames {
                break;
            }
            if channels == 2 {
                output.push(states[0].expand(byte >> 4));
                output.push(states[1].expand(byte & 15));
                frame += 1;
            } else {
                output.push(states[0].expand(byte >> 4));
                frame += 1;
                if frame < block_frames {
                    output.push(states[0].expand(byte & 15));
                    frame += 1;
                }
            }
        }
    }
    if output.len() != count {
        return Err(error("Microsoft ADPCM did not produce its declared frames"));
    }
    Ok((AudioCodec::WavMsAdpcm, rate, channels as u8, output))
}

struct AdpcmState {
    first: i64,
    second: i64,
    delta: i64,
    recent: i64,
    previous: i64,
}

impl AdpcmState {
    fn expand(&mut self, nibble: u8) -> i16 {
        let difference = if nibble & 8 != 0 {
            i64::from(nibble) - 16
        } else {
            i64::from(nibble)
        };
        let prediction = (self.recent * self.first + self.previous * self.second) / 256;
        let value = (prediction + difference * self.delta).clamp(-32768, 32767);
        self.previous = self.recent;
        self.recent = value;
        self.delta =
            (self.delta * ADAPTATION[usize::from(nibble)] / 256).clamp(16, i32::MAX as i64);
        value as i16
    }
}

fn word16(bytes: &[u8], offset: usize) -> RuntimeResult<u16> {
    bytes
        .get(offset..offset + 2)
        .map(|bytes| u16::from_le_bytes(bytes.try_into().unwrap()))
        .ok_or_else(|| error("WAV word is truncated"))
}

fn word32(bytes: &[u8], offset: usize) -> RuntimeResult<u32> {
    bytes
        .get(offset..offset + 4)
        .map(|bytes| u32::from_le_bytes(bytes.try_into().unwrap()))
        .ok_or_else(|| error("WAV DWORD is truncated"))
}

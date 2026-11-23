//! Independently implemented RIFF/WAVE PCM, IEEE float and Microsoft ADPCM.
//! Format facts: Microsoft RIFF/WAVE Multimedia Standards, ADPCMWAVEFORMAT.
//! No codec implementation or reference-engine source has been copied.
use cs2_runtime::RuntimeResult;

use crate::audio_decode::{AudioCodec, AudioProbe};
use crate::media_process::error;
use std::ops::Range;

const ADAPTATION: [i64; 16] = [
    230, 230, 230, 230, 307, 409, 512, 614, 768, 614, 512, 409, 307, 230, 230, 230,
];

pub(super) struct Wave {
    pub(super) probe: AudioProbe,
    data: Range<usize>,
    align: usize,
    encoding: Encoding,
}

enum Encoding {
    Pcm {
        width: usize,
        float: bool,
    },
    Adpcm {
        frames_per_block: usize,
        pairs: Vec<(i16, i16)>,
    },
}

impl Wave {
    pub(super) fn parse(bytes: &[u8]) -> RuntimeResult<Self> {
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
                b"data" if data.is_none() => data = Some(header_end..body_end),
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
            let (frames, encoding) =
                parse_adpcm(format, &bytes[data.clone()], fact, channels, align, bits)?;
            return Ok(Self {
                probe: AudioProbe {
                    codec: AudioCodec::WavMsAdpcm,
                    sample_rate: rate,
                    channels,
                    frames: frames as u64,
                },
                data,
                align,
                encoding,
            });
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
        let float = tag == 3;
        Ok(Self {
            probe: AudioProbe {
                codec: if float {
                    AudioCodec::WavFloat
                } else {
                    AudioCodec::WavPcm
                },
                sample_rate: rate,
                channels,
                frames: frames as u64,
            },
            data,
            align,
            encoding: Encoding::Pcm { width, float },
        })
    }

    pub(super) fn extra_bytes(&self) -> usize {
        match &self.encoding {
            Encoding::Pcm { .. } => 0,
            Encoding::Adpcm { pairs, .. } => pairs.capacity() * size_of::<(i16, i16)>(),
        }
    }

    pub(super) fn read(
        &self,
        bytes: &[u8],
        start: usize,
        end: usize,
        output: &mut Vec<i16>,
    ) -> RuntimeResult<()> {
        if start > end || end > self.probe.frames as usize {
            return Err(error("requested frames beyond the WAV source"));
        }
        let channels = usize::from(self.probe.channels);
        output.clear();
        output
            .try_reserve_exact((end - start) * channels)
            .map_err(|_| error("allocate bounded WAV PCM"))?;
        let data = &bytes[self.data.clone()];
        match &self.encoding {
            Encoding::Pcm { width, float } => decode_pcm(
                &data[start * self.align..end * self.align],
                *width,
                *float,
                output,
            ),
            Encoding::Adpcm {
                frames_per_block,
                pairs,
            } => {
                let mut position = start;
                while position < end {
                    let block_no = position / frames_per_block;
                    let block_frame = block_no * frames_per_block;
                    let block_start = block_no * self.align;
                    let block_end = (block_start + self.align).min(data.len());
                    let next = (block_frame + frames_per_block).min(end);
                    decode_adpcm_block(
                        &data[block_start..block_end],
                        channels,
                        pairs,
                        position - block_frame,
                        next - block_frame,
                        output,
                    )?;
                    position = next;
                }
                Ok(())
            }
        }
    }
}

fn decode_pcm(data: &[u8], width: usize, float: bool, output: &mut Vec<i16>) -> RuntimeResult<()> {
    for sample in data.chunks_exact(width) {
        let value = if float {
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
    Ok(())
}

fn parse_adpcm(
    format: &[u8],
    data: &[u8],
    fact: Option<usize>,
    channels: u8,
    align: usize,
    bits: u16,
) -> RuntimeResult<(usize, Encoding)> {
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
        // Some legacy mono effects overcount a short final block by one sample.
        Some(declared) if channels == 1 && tail != 0 && declared - maximum == 1 => maximum,
        Some(declared)
            if full > 0
                && declared % frames_per_block == 0
                && declared - maximum <= frames_per_block =>
        {
            maximum
        }
        Some(_) => return Err(error("Microsoft ADPCM fact exceeds encoded blocks")),
    };
    if data
        .chunks(align)
        .take(frames.div_ceil(frames_per_block))
        .any(|block| {
            block[..channels]
                .iter()
                .any(|&predictor| usize::from(predictor) >= pairs.len())
        })
    {
        return Err(error("Microsoft ADPCM predictor exceeds coefficient table"));
    }
    Ok((
        frames,
        Encoding::Adpcm {
            frames_per_block,
            pairs,
        },
    ))
}

fn decode_adpcm_block(
    block: &[u8],
    channels: usize,
    pairs: &[(i16, i16)],
    start: usize,
    end: usize,
    output: &mut Vec<i16>,
) -> RuntimeResult<()> {
    let state = |channel: usize| -> RuntimeResult<AdpcmState> {
        let (first, second) = pairs[usize::from(block[channel])];
        Ok(AdpcmState {
            first: i64::from(first),
            second: i64::from(second),
            delta: i64::from(word16(block, channels + channel * 2)?).max(16),
            recent: i64::from(word16(block, 3 * channels + channel * 2)? as i16),
            previous: i64::from(word16(block, 5 * channels + channel * 2)? as i16),
        })
    };
    let mut left = state(0)?;
    let mut right = (channels == 2).then(|| state(1)).transpose()?;
    let data = &block[7 * channels..];
    for frame in 0..end {
        let (first, second) = match frame {
            0 => (
                left.previous as i16,
                right.as_ref().map(|s| s.previous as i16),
            ),
            1 => (left.recent as i16, right.as_ref().map(|s| s.recent as i16)),
            _ => match &mut right {
                Some(right) => {
                    let byte = data[frame - 2];
                    (left.expand(byte >> 4), Some(right.expand(byte & 15)))
                }
                None => {
                    let nibble = frame - 2;
                    let byte = data[nibble / 2];
                    (
                        left.expand(if nibble % 2 == 0 {
                            byte >> 4
                        } else {
                            byte & 15
                        }),
                        None,
                    )
                }
            },
        };
        if frame >= start {
            output.push(first);
            output.extend(second);
        }
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::parse_adpcm;

    #[test]
    fn mono_adpcm_accepts_one_sample_overcount_only_with_short_final_block() {
        let mut format = [0; 26];
        for (offset, word) in [(16, 8_u16), (18, 2036), (20, 1), (22, 256)] {
            format[offset..offset + 2].copy_from_slice(&word.to_le_bytes());
        }
        let data = vec![0; 19_059];
        let decode = |data: &[u8], fact| parse_adpcm(&format, data, Some(fact), 1, 1024, 4);

        let (frames, _) = decode(&data, 37_891).unwrap();
        assert_eq!(frames, 37_890);
        assert!(decode(&data, 37_892).is_err());
        assert!(decode(&data[..1024], 2037).is_err());
        assert!(decode(&data[..1029], 2037).is_err());
    }
}

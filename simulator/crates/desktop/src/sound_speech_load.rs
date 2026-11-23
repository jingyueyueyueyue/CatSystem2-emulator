//! Real generated PCM ownership for kind13 speech loads. No synthetic VM
//! pointers, encoded placeholder files or decode-success without samples.
use std::sync::Arc;
use cs2_script::{NativeReturn, Result, Vm};
use super::{LoggingHost, audio_script_error, read_u32};
use super::audio_decode::{AudioCodec, AudioProbe, DecodedAudio, PreparedAudio};

pub(super) const RECORD_BYTES: usize = 292;
pub(super) const MAX_TEXT: usize = 4096;
pub(super) const MAX_RECORD: usize = RECORD_BYTES + MAX_TEXT + 1;
const MAX_LIVE_PCM: usize = 128 * 1024 * 1024;

impl LoggingHost<'_> {
    /// Prepares an actual source but does not start its output device. The
    /// normal183/157 path consumes this exact Arc through PREPARED_REQUEST.
    pub(super) fn load_sound_speech_pcm(
        &mut self, channel: u32, text: &[u8], silence_millis: u32,
    ) -> Result<NativeReturn> {
        if channel >= 64 {
            return Err(audio_script_error("speech physical channel is outside0..63"));
        }
        let pcm = self.synthesize_sound_speech(text)?;
        if !matches!(pcm.channels, 1 | 2) || pcm.sample_rate == 0 || pcm.sample_rate > 192_000
            || pcm.samples.is_empty() || pcm.samples.len() % usize::from(pcm.channels) != 0
            || pcm.samples.len() > 32 * 1024 * 1024
        {
            return Err(audio_script_error("speech provider produced invalid/unbounded PCM"));
        }
        let used = self.sound_channels.iter()
            .filter(|(id, state)| **id != channel && state.task.is_speech)
            .filter_map(|(_, state)| state.prepared_audio.as_ref())
            .try_fold(0usize, |total, audio| total.checked_add(audio.sample_count() * 2))
            .ok_or_else(|| audio_script_error("speech PCM ownership accounting overflow"))?;
        if used.checked_add(pcm.samples.len() * 2).is_none_or(|total| total > MAX_LIVE_PCM) {
            return Err(audio_script_error("live generated speech PCM exceeds128 MiB"));
        }
        let prepared = Arc::new(PreparedAudio::from_pcm(DecodedAudio {
            probe: AudioProbe { codec: AudioCodec::SpeechPcm, sample_rate: pcm.sample_rate,
                channels: pcm.channels,
                frames: (pcm.samples.len() / usize::from(pcm.channels)) as u64 },
            samples: pcm.samples,
        }));
        let identity = super::sound_native::next_sound_resource_identity()?;
        self.cancel_audio_channel_start(channel)?;
        let mut state = self.sound_channels.remove(&channel).unwrap_or_default();
        if let Some(handle) = state.handle {
            if let Err(cause) = self.audio_host.stop(handle) {
                self.sound_channels.insert(channel, state);
                return Err(self.sound_host_error(cause));
            }
        }
        state.release();
        // This is a generated-source identity. It is never passed to file IO;
        // speech playback requires the actual prepared-PCM desktop controls.
        state.resource = Some(format!("speech-pcm:{identity}"));
        state.task.resource_id = identity;
        state.task.is_speech = true;
        state.task.owner = self.single_sound_task_owner();
        state.source_query_loaded();
        state.selector = 1;
        state.duration_frames = silence_millis;
        state.volume = 100;
        state.pan = 0;
        state.task.whole_buffer = prepared.probe.frames
            + u64::from(prepared.probe.sample_rate) * u64::from(silence_millis) / 1000
            <= u64::from(prepared.probe.sample_rate) * 4;
        state.prepared_audio = Some(prepared);
        self.sound_channels.insert(channel, state);
        Ok(NativeReturn::Value(1))
    }
}

/// Native69/70 copy the nested text at+288 immediately, replacing its guest
/// address with a presence flag. Explicit producers supply this same owned
/// [292-byte prefix][declared-encoding text includingNUL] representation.
pub(super) fn snapshot_speech_record(vm: &Vm, address: u32) -> Result<Vec<u8>> {
    let mut bytes = vm.read(address, RECORD_BYTES)?;
    let text_address = read_u32(&bytes, 288)?;
    if text_address == 0 {
        return Err(audio_script_error("speech load text pointer is null"));
    }
    bytes[288..292].copy_from_slice(&1u32.to_le_bytes());
    for offset in 0..=MAX_TEXT {
        let at = text_address.checked_add(offset as u32)
            .ok_or_else(|| audio_script_error("speech text VM address overflows"))?;
        let byte = vm.read(at, 1)?[0];
        bytes.push(byte);
        if byte == 0 { return Ok(bytes); }
    }
    Err(audio_script_error("speech text exceeds4096 encoded bytes"))
}

pub(super) fn speech_record_text(bytes: &[u8]) -> Result<&[u8]> {
    if bytes.len() > MAX_RECORD || read_u32(bytes, 288)? == 0 {
        return Err(audio_script_error("speech load record/presence is invalid"));
    }
    let text = bytes.get(RECORD_BYTES..)
        .ok_or_else(|| audio_script_error("speech load record has no owned text"))?;
    if text.is_empty() || text.last() != Some(&0) || text[..text.len() - 1].contains(&0) {
        return Err(audio_script_error("speech load text must end with exactly oneNUL"));
    }
    Ok(&text[..text.len() - 1])
}

/// Ordinary1070..1073 build the same owned record as native69/70. In the
/// verified PE the text is format-expanded, never resolved as a filename;
/// +272 remains zero and only1072/1073 write front silence at+276.
pub(super) fn prepare_native_record(index: u32, args: &[u8], vm: &Vm) -> Result<Vec<u8>> {
    let grouped = matches!(index, 1071 | 1073);
    let words = match index {
        1070 => 3,
        1071 => 4,
        1072 => 5,
        1073 => 6,
        _ => return Err(audio_script_error("unsupported speech preparation wrapper")),
    };
    if args.len() < words * 4 {
        return Err(audio_script_error("speech preparation arguments are truncated"));
    }
    let source = vm.read_string(read_u32(args, 8)?)?;
    let expanded = super::expand_kcs_format(vm, &source, 0)?;
    let text = super::trim_nul(&expanded);
    if text.len() > MAX_TEXT {
        return Err(audio_script_error("expanded speech text exceeds4096 encoded bytes"));
    }
    let mut record = vec![0u8; RECORD_BYTES];
    let key = read_u32(args, 0)?;
    let offset = if grouped { 4 } else { 0 };
    record[offset..offset + 4].copy_from_slice(&key.to_le_bytes());
    if matches!(index, 1072 | 1073) {
        // MUL and unsigned 64/64 divide, then store EAX: preserve the
        // resulting DWORD wrap, rather than saturating a large frame count.
        let frames = read_u32(args, if grouped { 20 } else { 16 })?;
        let millis = (u64::from(frames) * 1000 / 60) as u32;
        record[276..280].copy_from_slice(&millis.to_le_bytes());
    }
    record[288..292].copy_from_slice(&1u32.to_le_bytes());
    record.extend_from_slice(text);
    record.push(0);
    Ok(record)
}

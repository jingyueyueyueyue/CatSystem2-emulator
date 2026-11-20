//! Frame indexing and bounded reads for long, single-stream Vorbis sources.
use super::{MAX_PCM, error};
use cs2_formats::{AudioMetadata, OggVorbis};
use cs2_runtime::RuntimeResult;
use lewton::audio::{PreviousWindowRight, get_decoded_sample_count, read_audio_packet_generic};
use lewton::header::{IdentHeader, SetupHeader};
use lewton::inside_ogg::read_headers;
use lewton::samples::InterleavedSamples;
use ogg::PacketReader;
use std::io::Cursor;
use std::sync::Arc;

const MAX_PACKETS: usize = 65_536;

struct IndexedPacket {
    encoded: Vec<u8>,
    end: usize,
    decoded_frames: usize,
}

pub(super) struct IndexedVorbis {
    pub(super) metadata: AudioMetadata,
    pub(super) encoded: Arc<[u8]>,
    packets: Vec<IndexedPacket>,
}

impl IndexedVorbis {
    pub(super) fn new(encoded: Arc<[u8]>) -> RuntimeResult<Option<Self>> {
        let metadata = OggVorbis::metadata_at_entry_boundary(&encoded)
            .map_err(|cause| error(format!("probe Ogg/Vorbis: {cause}")))?;
        let mut stream = PacketReader::new(Cursor::new(encoded.as_ref()));
        let ((ident, _, setup), _) = read_headers(&mut stream).map_err(packet_error)?;
        let channels = usize::from(metadata.channels);
        let frame_limit = MAX_PCM / size_of::<i16>() / channels;
        let mut packets: Vec<IndexedPacket> = Vec::new();
        let mut frames = 0usize;
        let mut granule: Option<u64> = None;
        while let Some(packet) = stream.read_packet().map_err(packet_error)? {
            if packets.len() == MAX_PACKETS {
                return Ok(None);
            }
            // The checked header supplies the sample count directly. Decode
            // packet bodies as the playback cursor reaches them. The first
            // packet emits nothing but must be valid to prime the overlap.
            let count = get_decoded_sample_count(&ident, &setup, &packet.data)
                .map_err(packet_error)?;
            let decoded_frames = if packets.is_empty() {
                decode_packet(&ident, &setup, &packet.data, &mut PreviousWindowRight::new())?;
                0
            } else {
                count
            };
            let mut emitted = decoded_frames;
            if let (Some(position), true) = (granule, packet.last_in_stream()) {
                emitted = emitted.min(packet.absgp_page().saturating_sub(position) as usize);
                if emitted == 0 && let Some(previous) = packets.last() {
                    // The playback cursor never visits an entirely trimmed
                    // final packet, so validate it here with its real overlap.
                    let mut window = PreviousWindowRight::new();
                    decode_packet(&ident, &setup, &previous.encoded, &mut window)?;
                    decode_packet(&ident, &setup, &packet.data, &mut window)?;
                }
            }
            if packet.last_in_page() {
                granule = Some(packet.absgp_page());
            } else if let Some(position) = &mut granule {
                *position = position.checked_add(emitted as u64)
                    .ok_or_else(|| packet_error("granule position overflow"))?;
            }
            frames = frames.checked_add(emitted)
                .filter(|&frames| frames <= frame_limit)
                .ok_or_else(|| error("decoded audio exceeds the 80 MiB PCM limit"))?;
            packets.push(IndexedPacket { encoded: packet.data, end: frames, decoded_frames });
        }
        drop(stream);
        Ok(Some(Self { metadata, encoded, packets }))
    }

    pub(super) fn frames(&self) -> usize {
        self.packets.last().map_or(0, |packet| packet.end)
    }

    pub(super) fn retained_bytes(&self) -> usize {
        let packets = self.packets.capacity() * size_of::<IndexedPacket>();
        packets + self.packets.iter().map(|packet| packet.encoded.capacity()).sum::<usize>()
    }

    pub(super) fn reader(&self) -> RuntimeResult<VorbisReader<'_>> {
        let mut stream = PacketReader::new(Cursor::new(self.encoded.as_ref()));
        let ((ident, _, setup), _) = read_headers(&mut stream).map_err(packet_error)?;
        Ok(VorbisReader {
            source: self,
            ident,
            setup,
            window: PreviousWindowRight::new(),
            packet_index: None,
            packet_pcm: Vec::new(),
            recent: Vec::new(),
            recent_start: 0,
        })
    }
}

pub(super) struct VorbisReader<'a> {
    source: &'a IndexedVorbis,
    ident: IdentHeader,
    setup: SetupHeader,
    window: PreviousWindowRight,
    packet_index: Option<usize>,
    packet_pcm: Vec<i16>,
    recent: Vec<i16>,
    recent_start: usize,
}

impl VorbisReader<'_> {
    pub(super) fn read(&mut self, start: usize, end: usize) -> RuntimeResult<&[i16]> {
        if start > end || end > self.source.frames() {
            return Err(packet_error("requested frames beyond the indexed source"));
        }
        let channels = usize::from(self.source.metadata.channels);
        let recent_end = self.recent_start + self.recent.len() / channels;
        // The output queue can reject an offered chunk while full. Replay
        // the same unscaled samples without resetting the decoder window.
        if self.recent_start <= start && end <= recent_end {
            let offset = self.recent_start;
            return Ok(&self.recent[(start - offset) * channels..(end - offset) * channels]);
        }
        let mut index = self.source.packets.partition_point(|packet| packet.end <= start);
        let mut copied = start;
        self.recent.clear();
        while copied < end {
            self.decode(index)?;
            let packet_start = self.start_of(index);
            let copy_end = end.min(self.source.packets[index].end);
            self.recent.extend_from_slice(
                &self.packet_pcm[(copied - packet_start) * channels..(copy_end - packet_start) * channels],
            );
            copied = copy_end;
            index += 1;
        }
        self.recent_start = start;
        Ok(&self.recent)
    }

    fn start_of(&self, index: usize) -> usize {
        if index == 0 { 0 } else { self.source.packets[index - 1].end }
    }

    fn decode(&mut self, index: usize) -> RuntimeResult<()> {
        if self.packet_index == Some(index) {
            return Ok(());
        }
        if self.packet_index.and_then(|previous| previous.checked_add(1)) != Some(index) {
            // For a loop or restored checkpoint, prime from the predecessor
            // instead of replaying the source up to the requested position.
            self.window = PreviousWindowRight::new();
            if let Some(previous) = index.checked_sub(1) {
                decode_packet(&self.ident, &self.setup, &self.source.packets[previous].encoded, &mut self.window)?;
            }
        }
        let packet = &self.source.packets[index];
        let mut pcm = decode_packet(&self.ident, &self.setup, &packet.encoded, &mut self.window)?;
        let channels = usize::from(self.source.metadata.channels);
        if pcm.len() != packet.decoded_frames * channels {
            return Err(packet_error("decoded length differs from indexed frames"));
        }
        pcm.truncate((packet.end - self.start_of(index)) * channels);
        self.packet_index = Some(index);
        self.packet_pcm = pcm;
        Ok(())
    }
}

fn decode_packet(
    ident: &IdentHeader,
    setup: &SetupHeader,
    packet: &[u8],
    window: &mut PreviousWindowRight,
) -> RuntimeResult<Vec<i16>> {
    let samples: InterleavedSamples<i16> =
        read_audio_packet_generic(ident, setup, packet, window).map_err(packet_error)?;
    Ok(samples.samples)
}

fn packet_error(cause: impl std::fmt::Display) -> cs2_runtime::RuntimeError {
    error(format!("decode Ogg/Vorbis packet: {cause}"))
}

//! Frame indexing and bounded reads for long, single-stream Vorbis sources.
use super::{MAX_PCM, PCM_READ_FRAMES, error};
use cs2_formats::{AudioMetadata, OggVorbis};
use cs2_runtime::RuntimeResult;
use lewton::audio::{PreviousWindowRight, get_decoded_sample_count, read_audio_packet_generic};
use lewton::header::{IdentHeader, SetupHeader};
use lewton::inside_ogg::read_headers;
use lewton::samples::InterleavedSamples;
use ogg::PacketReader;
use std::io::Cursor;
use std::sync::Arc;
use std::thread;

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
    pub(super) fn new(encoded: Arc<[u8]>, index_budget: usize) -> RuntimeResult<Option<Self>> {
        let metadata = OggVorbis::metadata_at_entry_boundary(&encoded)
            .map_err(|cause| error(format!("probe Ogg/Vorbis: {cause}")))?;
        let mut stream = PacketReader::new(Cursor::new(encoded.as_ref()));
        let ((ident, _, setup), _) = read_headers(&mut stream).map_err(packet_error)?;
        let channels = usize::from(metadata.channels);
        let frame_limit = usize::MAX / size_of::<i16>() / channels;
        let mut packets: Vec<IndexedPacket> = Vec::new();
        let mut frames = 0usize;
        let mut packet_data_bytes = 0usize;
        let mut granule: Option<u64> = None;
        while let Some(packet) = stream.read_packet().map_err(packet_error)? {
            // The checked header supplies the sample count directly. Decode
            // packet bodies as the playback cursor reaches them. The first
            // packet emits nothing but must be valid to prime the overlap.
            let count =
                get_decoded_sample_count(&ident, &setup, &packet.data).map_err(packet_error)?;
            let decoded_frames = if packets.is_empty() {
                decode_packet(
                    &ident,
                    &setup,
                    &packet.data,
                    &mut PreviousWindowRight::new(),
                )?;
                0
            } else {
                count
            };
            let mut emitted = decoded_frames;
            if let (Some(position), true) = (granule, packet.last_in_stream()) {
                emitted = emitted.min(
                    usize::try_from(packet.absgp_page().saturating_sub(position))
                        .unwrap_or(usize::MAX),
                );
                if emitted == 0
                    && let Some(previous) = packets.last()
                {
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
                *position = position
                    .checked_add(emitted as u64)
                    .ok_or_else(|| packet_error("granule position overflow"))?;
            }
            frames = frames
                .checked_add(emitted)
                .filter(|&frames| frames <= frame_limit)
                .ok_or_else(|| packet_error("frame count overflow"))?;
            packet_data_bytes += packet.data.capacity();
            packets.push(IndexedPacket {
                encoded: packet.data,
                end: frames,
                decoded_frames,
            });
            if packet_data_bytes + packets.capacity() * size_of::<IndexedPacket>() > index_budget {
                return Ok(None);
            }
        }
        drop(stream);
        let retained_bytes = packet_data_bytes + packets.capacity() * size_of::<IndexedPacket>();
        // Prefer PCM when it is smaller than the packet index.
        if retained_bytes > frames * channels * size_of::<i16>() {
            return Ok(None);
        }
        Ok(Some(Self {
            metadata,
            encoded,
            packets,
        }))
    }

    pub(super) fn frames(&self) -> usize {
        self.packets.last().map_or(0, |packet| packet.end)
    }

    pub(super) fn index_bytes(&self) -> usize {
        self.packets.capacity() * size_of::<IndexedPacket>()
            + self
                .packets
                .iter()
                .map(|packet| packet.encoded.capacity())
                .sum::<usize>()
    }

    pub(super) fn checkpoint_samples(&self) -> RuntimeResult<Vec<i16>> {
        let frames = self.frames();
        let channels = usize::from(self.metadata.channels);
        let sample_count = frames * channels;
        if sample_count > MAX_PCM / size_of::<i16>() {
            return Err(error("checkpoint audio exceeds the 80 MiB PCM limit"));
        }
        let mut samples = Vec::new();
        samples
            .try_reserve_exact(sample_count)
            .map_err(|_| error("could not allocate bounded checkpoint PCM"))?;
        samples.resize(sample_count, 0);

        let lanes = if self.encoded.len() >= super::vorbis::MIN_PARALLEL_BYTES {
            thread::available_parallelism().map_or(1, |threads| threads.get().min(3))
        } else {
            1
        };
        if lanes > 1 && frames > PCM_READ_FRAMES {
            let frames_per_lane = frames.div_ceil(lanes);
            let decoded = thread::scope(|scope| {
                let mut chunks = samples.chunks_mut(frames_per_lane * channels);
                let first = chunks.next().expect("nonempty checkpoint PCM");
                let mut workers = Vec::new();
                for (lane, chunk) in chunks.enumerate() {
                    let start = (lane + 1) * frames_per_lane;
                    let worker = thread::Builder::new()
                        .spawn_scoped(scope, move || self.fill_pcm(start, chunk));
                    let Ok(worker) = worker else {
                        // The scope joins started lanes before a serial retry.
                        return Ok(false);
                    };
                    workers.push(worker);
                }
                let mut result = self.fill_pcm(0, first);
                for worker in workers {
                    let lane_result = worker
                        .join()
                        .unwrap_or_else(|_| Err(error("Vorbis checkpoint worker panicked")));
                    result = result.and(lane_result);
                }
                result.map(|()| true)
            })?;
            if decoded {
                return Ok(samples);
            }
        }
        self.fill_pcm(0, &mut samples)?;
        Ok(samples)
    }

    fn fill_pcm(&self, mut start: usize, samples: &mut [i16]) -> RuntimeResult<()> {
        let channels = usize::from(self.metadata.channels);
        let mut reader = self.reader()?;
        for chunk in samples.chunks_mut(PCM_READ_FRAMES * channels) {
            let end = start + chunk.len() / channels;
            chunk.copy_from_slice(reader.read(start, end)?);
            start = end;
        }
        Ok(())
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
        if end - start > PCM_READ_FRAMES {
            return Err(packet_error("requested PCM exceeds the streaming window"));
        }
        let channels = usize::from(self.source.metadata.channels);
        let recent_end = self.recent_start + self.recent.len() / channels;
        // The output queue can reject an offered chunk while full. Replay
        // the same unscaled samples without resetting the decoder window.
        if self.recent_start <= start && end <= recent_end {
            let offset = self.recent_start;
            return Ok(&self.recent[(start - offset) * channels..(end - offset) * channels]);
        }
        if start == end {
            return Ok(&[]);
        }
        let mut index = self
            .source
            .packets
            .partition_point(|packet| packet.end <= start);
        let mut copied = start;
        self.recent.clear();
        self.recent
            .try_reserve_exact((end - start) * channels)
            .map_err(|_| packet_error("could not allocate the streaming window"))?;
        self.recent_start = start;
        while copied < end {
            self.decode(index)?;
            let packet_start = self.start_of(index);
            let copy_end = end.min(self.source.packets[index].end);
            self.recent.extend_from_slice(
                &self.packet_pcm
                    [(copied - packet_start) * channels..(copy_end - packet_start) * channels],
            );
            copied = copy_end;
            index += 1;
        }
        Ok(&self.recent)
    }

    fn start_of(&self, index: usize) -> usize {
        if index == 0 {
            0
        } else {
            self.source.packets[index - 1].end
        }
    }

    fn decode(&mut self, index: usize) -> RuntimeResult<()> {
        if self.packet_index == Some(index) {
            return Ok(());
        }
        if self
            .packet_index
            .and_then(|previous| previous.checked_add(1))
            != Some(index)
        {
            // For a loop or restored checkpoint, prime from the predecessor
            // instead of replaying the source up to the requested position.
            self.window = PreviousWindowRight::new();
            if let Some(previous) = index.checked_sub(1) {
                decode_packet(
                    &self.ident,
                    &self.setup,
                    &self.source.packets[previous].encoded,
                    &mut self.window,
                )?;
            }
        }
        let packet = &self.source.packets[index];
        self.packet_index = None;
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

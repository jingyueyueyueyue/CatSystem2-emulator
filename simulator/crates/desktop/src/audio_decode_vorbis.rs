use super::{MAX_PCM, error};
use cs2_formats::{AudioMetadata, OggVorbis};
use cs2_runtime::RuntimeResult;
use lewton::audio::{PreviousWindowRight, read_audio_packet_generic};
use lewton::header::{IdentHeader, SetupHeader};
use lewton::inside_ogg::{OggStreamReader, read_headers};
use lewton::samples::{InterleavedSamples, Samples};
use ogg::{Packet, PacketReader};
use std::io::Cursor;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

const MAX_SAMPLES: usize = MAX_PCM / size_of::<i16>();
const MIN_PARALLEL_BYTES: usize = 256 * 1024;
const MAX_PARALLEL_PACKETS: usize = 65_536;

pub(super) fn decode(encoded: &[u8]) -> RuntimeResult<(AudioMetadata, Vec<i16>)> {
    // Retain full validation, including archives that end at a completed
    // page without the Ogg end-of-stream flag.
    let metadata = OggVorbis::metadata_at_entry_boundary(encoded)
        .map_err(|cause| error(format!("probe Ogg/Vorbis: {cause}")))?;
    if encoded.len() >= MIN_PARALLEL_BYTES {
        let lanes = thread::available_parallelism().map_or(1, |threads| threads.get().min(3));
        if lanes > 1 && let Some(samples) = decode_parallel(encoded, lanes)? {
            return Ok((metadata, samples));
        }
    }
    let mut stream = OggStreamReader::new(Cursor::new(encoded))
        .map_err(|cause| error(format!("open Ogg/Vorbis decoder: {cause}")))?;
    let mut samples = Vec::new();
    while let Some(packet) = stream.read_dec_packet_itl().map_err(packet_error)? {
        append_samples(&mut samples, &packet)?;
    }
    Ok((metadata, samples))
}

pub(super) fn decode_parallel(encoded: &[u8], lanes: usize) -> RuntimeResult<Option<Vec<i16>>> {
    let mut reader = PacketReader::new(Cursor::new(encoded));
    let ((ident, _, setup), _) = read_headers(&mut reader)
        .map_err(|cause| error(format!("open Ogg/Vorbis decoder: {cause}")))?;
    let mut packets = Vec::new();
    while let Some(packet) = reader.read_packet().map_err(packet_error)? {
        if packets.len() == MAX_PARALLEL_PACKETS {
            return Ok(None);
        }
        packets.push(packet);
    }

    // Split after full pages, where the decoder's absolute granule position
    // is known exactly. Vorbis overlap only needs the preceding packet.
    let mut starts = vec![0];
    let stride = packets.len() / lanes;
    for (index, packet) in packets
        .iter()
        .enumerate()
        .take(packets.len().saturating_sub(1))
    {
        if index + 1 >= stride * starts.len() && packet.last_in_page() {
            starts.push(index + 1);
            if starts.len() == lanes {
                break;
            }
        }
    }
    if starts.len() == 1 {
        return Ok(None);
    }
    starts.push(packets.len());

    let total = AtomicUsize::new(0);
    thread::scope(|scope| {
        let mut workers = Vec::new();
        for range in starts[1..].windows(2) {
            let (ident, setup, total) = (&ident, &setup, &total);
            let previous = &packets[range[0] - 1];
            let packets = &packets[range[0]..range[1]];
            let worker = thread::Builder::new().spawn_scoped(scope, move || {
                decode_range(ident, setup, Some(previous), packets, total)
            });
            let Ok(worker) = worker else {
                return Ok(None);
            };
            workers.push(worker);
        }
        let first = decode_range(&ident, &setup, None, &packets[..starts[1]], &total);
        // Join every lane before propagating errors or assembling the PCM.
        let pieces = std::iter::once(first)
            .chain(workers.into_iter().map(|worker| {
                worker
                    .join()
                    .unwrap_or_else(|_| Err(error("Vorbis decoder worker panicked")))
            }))
            .collect::<Vec<_>>();
        let mut pieces = pieces.into_iter().collect::<RuntimeResult<Vec<_>>>()?.into_iter();
        let mut result = pieces.next().expect("first Vorbis lane");
        result
            .try_reserve_exact(total.load(Ordering::Relaxed) - result.len())
            .map_err(|_| error("could not allocate bounded sound PCM"))?;
        for mut piece in pieces {
            result.append(&mut piece);
        }
        Ok(Some(result))
    })
}

fn decode_range(
    ident: &IdentHeader,
    setup: &SetupHeader,
    previous: Option<&Packet>,
    packets: &[Packet],
    total: &AtomicUsize,
) -> RuntimeResult<Vec<i16>> {
    let mut window = PreviousWindowRight::new();
    let mut granule = None;
    if let Some(previous) = previous {
        let _: InterleavedSamples<i16> =
            read_audio_packet_generic(ident, setup, &previous.data, &mut window)
                .map_err(packet_error)?;
        granule = Some(previous.absgp_page());
    }
    let mut samples = Vec::new();
    for packet in packets {
        let mut pcm: InterleavedSamples<i16> =
            read_audio_packet_generic(ident, setup, &packet.data, &mut window)
                .map_err(packet_error)?;
        // Apply Lewton's final-packet trimming using the same page position
        // and intermediate sample counts as OggStreamReader.
        if let (Some(position), true) = (granule, packet.last_in_stream()) {
            pcm.truncate(packet.absgp_page().saturating_sub(position) as usize);
        }
        if packet.last_in_page() {
            granule = Some(packet.absgp_page());
        } else if let Some(position) = &mut granule {
            *position = position
                .checked_add(pcm.num_samples() as u64)
                .ok_or_else(|| error("Vorbis granule position overflow"))?;
        }
        if total.fetch_add(pcm.samples.len(), Ordering::Relaxed) + pcm.samples.len() > MAX_SAMPLES {
            return Err(error("decoded audio exceeds the 80 MiB PCM limit"));
        }
        append_samples(&mut samples, &pcm.samples)?;
    }
    Ok(samples)
}

fn append_samples(samples: &mut Vec<i16>, packet: &[i16]) -> RuntimeResult<()> {
    let length = samples.len() + packet.len();
    if length > MAX_SAMPLES {
        return Err(error("decoded audio exceeds the 80 MiB PCM limit"));
    }
    if length > samples.capacity() {
        let capacity = length.max((samples.capacity() * 2).min(MAX_SAMPLES));
        samples
            .try_reserve_exact(capacity - samples.len())
            .map_err(|_| error("could not allocate bounded sound PCM"))?;
    }
    samples.extend_from_slice(packet);
    Ok(())
}

fn packet_error(cause: impl std::fmt::Display) -> cs2_runtime::RuntimeError {
    error(format!("decode Ogg/Vorbis packet: {cause}"))
}

//! Bounded material workers produce immutable audio/sidecars; only the frame
//! thread owns channels, task records and native completion.
use std::sync::{Arc, Mutex, Weak, mpsc, atomic::{AtomicUsize, Ordering}};
use std::time::{Duration, Instant};
use cs2_runtime::{ResourceCandidate, ResourceIndex, RuntimeResult};
use crate::audio_decode::{MAX_ENCODED, PreparedAudio, prepare_named_audio};
use crate::sound_sidecar::{SidecarRequest, SoundSidecars};

const MAX_WORKERS: usize = 2;
static ACTIVE_WORKERS: AtomicUsize = AtomicUsize::new(0);

pub(super) const TIMEOUT: Duration = Duration::from_secs(30);

struct WorkerSlot;
impl WorkerSlot {
    fn acquire() -> Option<Self> {
        ACTIVE_WORKERS.fetch_update(Ordering::AcqRel, Ordering::Acquire,
            |active| (active < MAX_WORKERS).then_some(active + 1)).ok().map(|_| Self)
    }
}
impl Drop for WorkerSlot {
    fn drop(&mut self) { ACTIVE_WORKERS.fetch_sub(1, Ordering::AcqRel); }
}

#[derive(Clone)]
pub(super) struct Source {
    pub(super) resource: String,
    pub(super) candidate: ResourceCandidate,
    pub(super) sidecars: Result<SidecarRequest, String>,
    pub(super) version: cs2_runtime::emote::EmoteArchiveVersion,
}

pub(super) struct Ready {
    pub(super) prepared: Arc<PreparedAudio>,
    pub(super) sidecars: Result<SoundSidecars, String>,
    pub(super) read_us: u128,
    pub(super) prepare_us: u128,
    pub(super) sidecar_us: u128,
    pub(super) background_wall_us: u128,
}

type Receiver = Mutex<mpsc::Receiver<RuntimeResult<Ready>>>;
pub(super) struct Ticket { receiver: Arc<Receiver>, _slot: Option<Arc<WorkerSlot>> }
impl Ticket {
    pub(super) fn poll(&self) -> Option<RuntimeResult<Ready>> {
        match self.receiver.lock().unwrap_or_else(|error| error.into_inner()).try_recv() {
            Ok(result) => Some(result),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => Some(Err(crate::media_process::error("sound material worker disconnected"))),
        }
    }
}

/// No pending job owns copied payload bytes until a worker slot is available.
/// At most two jobs can read/decode concurrently across all players.
pub(super) fn begin(
    resources: Arc<ResourceIndex>, source: Arc<Source>, cached: Option<&[u8]>,
) -> RuntimeResult<Option<Arc<Ticket>>> {
    let Some(slot) = WorkerSlot::acquire().map(Arc::new) else { return Ok(None); };
    let cached = cached.map(<[u8]>::to_vec);
    let (sender, receiver) = mpsc::channel();
    let receiver = Arc::new(Mutex::new(receiver));
    let owner = Arc::downgrade(&receiver);
    let worker_slot = slot.clone();
    std::thread::Builder::new().name("cs2-sound-material".into()).spawn(move || {
        let _slot = worker_slot;
        if owner.strong_count() == 0 { return; }
        let result = prepare(&resources, &source, cached, &owner);
        if owner.strong_count() != 0 { let _ = sender.send(result); }
    }).map_err(|cause| cs2_runtime::RuntimeError::Host(format!("spawn sound material worker: {cause}")))?;
    Ok(Some(Arc::new(Ticket { receiver, _slot: Some(slot) })))
}

/// Non-resumable JavaScript calls cannot wait for another caller to consume
/// its ready ticket on this frame. They retain the old synchronous contract.
pub(super) fn prepare_compat(resources: &ResourceIndex, source: &Source, cached: Option<&[u8]>) -> Arc<Ticket> {
    let (sender, receiver) = mpsc::channel();
    let receiver = Arc::new(Mutex::new(receiver));
    let result = prepare(resources, source, cached.map(<[u8]>::to_vec), &Arc::downgrade(&receiver));
    let _ = sender.send(result);
    Arc::new(Ticket { receiver, _slot: None })
}

fn prepare(resources: &ResourceIndex, source: &Source, cached: Option<Vec<u8>>, owner: &Weak<Receiver>) -> RuntimeResult<Ready> {
    let started = Instant::now();
    #[cfg(test)]
    if let cs2_runtime::ResourceLocation::File { path } = &source.candidate.location {
        super::async_load_tests::delay_material(&path.to_string_lossy());
    }
    if owner.strong_count() == 0 { return Err(crate::media_process::error("sound material load cancelled")); }
    if cs2_runtime::emote::EmoteArchiveVersion::observe(&source.candidate).as_ref() != Some(&source.version) {
        return Err(crate::media_process::error("sound material source changed before preparation"));
    }
    let read = Instant::now();
    let encoded = if let Some(cached) = cached { cached }
        else { resources.read_candidate_bounded(&source.candidate, MAX_ENCODED)?
            .ok_or_else(|| crate::media_process::error("encoded sound exceeds 64 MiB"))? };
    let read_us = read.elapsed().as_micros();
    if owner.strong_count() == 0 { return Err(crate::media_process::error("sound material load cancelled")); }
    let decoding = Instant::now();
    let prepared = prepare_named_audio(encoded, &source.resource)?;
    let prepare_us = decoding.elapsed().as_micros();
    let sidecars_started = Instant::now();
    let sidecars = if owner.strong_count() == 0 { Err("sound material load cancelled".into()) }
        else { source.sidecars.as_ref().map_err(Clone::clone)
            .and_then(|request| request.prepare(resources).map_err(|cause| cause.to_string())) };
    if cs2_runtime::emote::EmoteArchiveVersion::observe(&source.candidate).as_ref() != Some(&source.version) {
        return Err(crate::media_process::error("sound material source changed during preparation"));
    }
    Ok(Ready { prepared, sidecars, read_us, prepare_us,
        sidecar_us: sidecars_started.elapsed().as_micros(), background_wall_us: started.elapsed().as_micros() })
}

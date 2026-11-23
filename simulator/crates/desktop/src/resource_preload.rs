//! Ordinary 434..438's explicit resource queue and cached bytes.
//! A portable synchronous worker completes a requested batch at 436. It keeps
//! real resource failures distinct from a completed, missing-file attempt.
use cs2_runtime::{ResourceCandidate, ResourceIndex, RuntimeError, resource_router::ResourceMode};
use cs2_runtime::emote::{CpuMotionModel, EmoteArchiveVersion, EmoteResource};
use cs2_script::{NativeReturn, Result, Vm};
use std::sync::{Arc, Mutex, Weak, mpsc};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::{cp932_to_utf8, expand_kcs_format, native_state_error, read_u32};

#[path = "image_predecode.rs"]
mod images;
#[path = "resource_read_ahead.rs"]
mod read_ahead;
pub(super) use read_ahead::AheadRequest;

const MAX_LIVE: usize = 256 * 1024 * 1024;
const MAX_ENTRIES: usize = 1_000_000;
const MAX_MODEL_BYTES: usize = 256 * 1024 * 1024;
const MAX_MODEL_POOL_BYTES: usize = 512 * 1024 * 1024;
const MAX_PACKED_MODEL_BYTES: usize = 32 * 1024 * 1024;
const MAX_MODELS: usize = 4;

type ModelResult = Option<Arc<CpuMotionModel>>;
type ModelReceiver = Mutex<mpsc::Receiver<std::result::Result<ReadyModel, &'static str>>>;

#[derive(Debug, Default)]
struct ModelBudget(AtomicUsize);
impl ModelBudget {
    fn reserve(budget: &Arc<Self>, maximum: usize) -> Option<ModelCharge> {
        let mut used = budget.0.load(Ordering::Acquire);
        loop {
            let allowed = MAX_MODEL_POOL_BYTES.saturating_sub(used).min(maximum);
            if allowed == 0 { return None; }
            match budget.0.compare_exchange_weak(used,used+allowed,Ordering::AcqRel,Ordering::Acquire) {
                Ok(_) => return Some(ModelCharge { budget: budget.clone(), bytes: allowed }),
                Err(current) => used = current,
            }
        }
    }
}
#[derive(Debug)]
struct ModelCharge { budget: Arc<ModelBudget>, bytes: usize }
impl Drop for ModelCharge {
    fn drop(&mut self) { self.budget.0.fetch_sub(self.bytes, Ordering::AcqRel); }
}
#[derive(Debug, Clone)]
struct ReadyModel { model: Arc<CpuMotionModel>, _charge: Arc<ModelCharge> }


#[derive(Debug)]
struct ModelJob {
    bytes: Vec<u8>,
    receiver: Weak<ModelReceiver>,
    result: mpsc::Sender<std::result::Result<ReadyModel, &'static str>>,
    budget: Arc<ModelBudget>,
}

#[derive(Debug, Clone)]
struct ModelPreload {
    name: String,
    version: EmoteArchiveVersion,
    receiver: Arc<ModelReceiver>,
    ready: Option<ReadyModel>,
    finished: bool,
}

fn start_model_worker() -> Option<mpsc::SyncSender<ModelJob>> {
    let (sender, jobs) = mpsc::sync_channel::<ModelJob>(MAX_MODELS);
    std::thread::Builder::new().name("cs2-model-preload".into())
        .spawn(move || {
            while let Ok(job) = jobs.recv() {
                if job.receiver.strong_count() == 0 {
                    continue;
                }
                let Some(mut charge) = ModelBudget::reserve(&job.budget, MAX_MODEL_BYTES) else {
                    let _ = job.result.send(Err("model-pool-budget")); continue;
                };
                let available = charge.bytes;
                let expanded = job.bytes.get(8..12).map(|bytes| u32::from_le_bytes(bytes.try_into().unwrap()) as usize).unwrap_or(usize::MAX);
                if expanded > available {
                    let _ = job.result.send(Err("expanded-model/pool-budget"));
                    continue;
                }
                // Compare-exchange reservations remain bounded even if a cloned
                // preload graph owns a separately started background worker.
                let model = EmoteResource::decode(&job.bytes).map_err(|_| "PSZ-decode")
                    .and_then(|resource| CpuMotionModel::load_for_preload(&resource, available)
                        .map_err(|_| "model-decode/unsupported-or-capacity"));
                let model = model.and_then(|model| {
                    let actual = model.estimated_storage_bytes();
                    if actual > available { return Err("final-model/pool-budget"); }
                    job.budget.0.fetch_sub(available.saturating_sub(actual), Ordering::AcqRel);
                    charge.bytes = actual;
                    Ok(ReadyModel { model: Arc::new(model), _charge: Arc::new(charge) })
                });
                let _ = job.result.send(model);
            }
        }).ok()?;
    Some(sender)
}

fn submit_model(
    worker: &mut Option<mpsc::SyncSender<ModelJob>>, name: &str,
    version: EmoteArchiveVersion, bytes: &[u8], budget: &Arc<ModelBudget>,
) -> Option<ModelPreload> {
    if bytes.len() > MAX_PACKED_MODEL_BYTES || !bytes.starts_with(b"PSZ\0") {
        return None;
    }
    let expanded = u32::from_le_bytes(bytes.get(8..12)?.try_into().ok()?) as usize;
    if !(8..=MAX_MODEL_BYTES).contains(&expanded) {
        return None;
    }
    if worker.is_none() {
        *worker = start_model_worker();
    }
    let (result, receiver) = mpsc::channel();
    let receiver = Arc::new(Mutex::new(receiver));
    let job = ModelJob { bytes: bytes.to_vec(), receiver: Arc::downgrade(&receiver), result, budget: budget.clone() };
    match worker.as_ref()?.try_send(job) {
        Ok(()) => Some(ModelPreload { name: name.into(), version, receiver, ready: None, finished: false }),
        Err(mpsc::TrySendError::Full(_)) => None,
        Err(mpsc::TrySendError::Disconnected(_)) => {
            *worker = None;
            None
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct SourcedResource {
    pub source: ResourceCandidate,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone)]
struct PreparedResource {
    resource: SourcedResource,
    name: String,
    stamp: read_ahead::ResourceStamp,
    version: Option<EmoteArchiveVersion>,
}
impl std::ops::Deref for PreparedResource {
    type Target = SourcedResource;
    fn deref(&self) -> &Self::Target { &self.resource }
}
impl PreparedResource {
    pub(super) fn is_current(&self) -> bool {
        read_ahead::ResourceStamp::observe(&self.source).as_ref() == Some(&self.stamp)
    }
}

#[derive(Debug, Clone)]
struct Request {
    name: String,
    priority: i32,
    age: i32,
    attempted: bool,
    resource: Option<PreparedResource>,
}

#[derive(Debug, Clone, Default)]
pub(super) struct ResourcePreloads {
    enabled: Option<bool>,
    requests: Vec<Request>,
    completed_through: i32,
    bytes: usize,
    images: images::ImagePredecode,
    model_worker: Option<mpsc::SyncSender<ModelJob>>,
    models: VecDeque<ModelPreload>,
    staging_cursor: usize,
    ahead: read_ahead::ReadAhead,
    diagnostics_remaining: u8,
    notes: Vec<String>,
    seen_reasons: Vec<&'static str>,
    previous_stats: (usize, usize, usize, usize, usize),
    model_budget: Arc<ModelBudget>,
}

impl ResourcePreloads {
    pub(crate) fn graph_request_count(&self) -> usize { self.requests.len() }
    pub(super) fn cached(&self, name: &str) -> Option<&[u8]> {
        self.cached_with_source(name).map(|resource| resource.bytes.as_slice())
    }

    pub(super) fn cached_with_source(&self, name: &str) -> Option<&SourcedResource> {
        self.requests.iter().find(|request| request.name.eq_ignore_ascii_case(name))
            .and_then(|request| request.resource.as_ref()).filter(|resource| resource.is_current())
            .or_else(|| self.ahead.cached(name)).map(|resource| &resource.resource)
    }

    #[cfg(test)]
    fn enqueue_model(&mut self, name: &str, version: EmoteArchiveVersion, bytes: &[u8]) {
        if self.models.iter().any(|model| model.version == version) { return; }
        if let Some(model) = submit_model(&mut self.model_worker, name, version, bytes, &self.model_budget) {
            if self.models.len() >= MAX_MODELS { self.models.pop_front(); }
            self.models.push_back(model);
        }
    }

    fn poll_models(&mut self) {
        for model in &mut self.models {
            if model.finished { continue; }
            let result = match model.receiver.lock() {
                Ok(receiver) => receiver.try_recv(),
                Err(_) => { model.finished = true; continue; }
            };
            match result {
                Ok(ready) => {
                    match ready {
                        Ok(ready) => model.ready = Some(ready),
                        Err(reason) => if self.diagnostics_remaining < 64 {
                            self.diagnostics_remaining += 1;
                            self.notes.push(format!("prefetch model rejected name={} reason={reason}", model.name));
                        },
                    }
                    model.finished = true;
                }
                Err(mpsc::TryRecvError::Empty) => {},
                Err(mpsc::TryRecvError::Disconnected) => model.finished = true,
            }
        }
    }

    pub(super) fn ready_model(&mut self) -> Option<&Arc<CpuMotionModel>> {
        self.poll_models();
        let count = self.models.len();
        for _ in 0..count {
            let index = self.staging_cursor % count;
            self.staging_cursor = (index + 1) % count;
            if self.models[index].ready.is_some() { return self.models[index].ready.as_ref().map(|ready| &ready.model); }
        }
        None
    }

    pub(super) fn take_model(&mut self, version: &EmoteArchiveVersion) -> ModelResult {
        let index = self.models.iter().position(|model| &model.version == version)?;
        let model = self.models.remove(index)?;
        // Demand never waits behind an unbounded decode queue. Give a finishing
        // job at most 2ms, then use the normal synchronous reference loader.
        let ready = model.ready.or_else(|| model.receiver.lock().ok()?
            .recv_timeout(std::time::Duration::from_millis(2)).ok()?.ok());
        if ready.is_none() { self.note("demand-model-not-ready (wait capped at 2ms)"); }
        ready.map(|ready| ready.model)
    }

    pub(super) fn update_prefetch(&mut self, resources: &ResourceIndex,
        requests: &[AheadRequest], boundaries: &[&'static str]) {
        self.ahead.update_requests(resources, requests);
        self.poll_models();
        self.models.retain(|model|
            self.requests.iter().filter_map(|r| r.resource.as_ref()).any(|resource|
                resource.version.as_ref() == Some(&model.version) && resource.is_current())
            || self.ahead.resources().any(|resource| resource.version.as_ref() == Some(&model.version)));
        // Each source version generates one warm event. Never compare/copy HG3
        // sources again just because another logical frame elapsed.
        for resource in self.ahead.newly_ready() {
            self.images.submit(&resource.bytes);
            if let Some(version) = resource.version.clone() {
                if self.models.len() >= MAX_MODELS || self.models.iter().any(|model| model.version == version) { continue; }
                if let Some(model) = submit_model(&mut self.model_worker, &resource.name, version, &resource.bytes, &self.model_budget) {
                    self.models.push_back(model);
                } else if self.diagnostics_remaining < 64 {
                    self.diagnostics_remaining += 1;
                    self.notes.push(format!("prefetch model rejected name={} reason=packed/header/queue-budget",resource.name));
                }
            }
        }
        let stats = self.ahead.stats();
        let current = (stats.0, stats.1, stats.2, self.models.len(), self.model_budget.0.load(Ordering::Acquire));
        if current != self.previous_stats && self.diagnostics_remaining < 64 {
            self.previous_stats = current;
            self.diagnostics_remaining += 1;
            self.notes.push(format!("prefetch scan_limit=20 candidates={} ready={} reserved_bytes={} models={} model_bytes={} model_pool_limit={}",
                current.0,current.1,current.2,current.3,current.4,MAX_MODEL_POOL_BYTES));
        }
        for note in self.ahead.drain_notes() { self.note(note); }
        for reason in boundaries.iter().copied() {
            if !matches!(reason, "block-end" | "command-limit") { self.note(reason); }
        }
    }

    fn note(&mut self, reason: &'static str) {
        if self.diagnostics_remaining >= 64 || self.seen_reasons.contains(&reason) { return; }
        self.diagnostics_remaining += 1;
        self.seen_reasons.push(reason);
        self.notes.push(format!("prefetch reason={reason}; FES scan<=20, KCS stops at first native"));
    }
    pub(super) fn drain_notes(&mut self) -> Vec<String> { std::mem::take(&mut self.notes) }

    pub(super) fn predecode_image(&mut self, bytes: &[u8]) { self.images.submit(bytes); }

    pub(super) fn image_predecode_snapshot(&self) -> Vec<(Arc<[u8]>, cs2_renderer::RgbaFrame)> {
        self.images.snapshot()
    }

    pub(super) fn take_predecoded_image(&mut self, bytes: &[u8], id: u32) -> Option<cs2_renderer::RgbaFrame> {
        self.images.take(bytes, id)
    }

    pub(super) fn call(
        &mut self, index: u32, args: &[u8], vm: &Vm, resources: &ResourceIndex,
    ) -> Result<NativeReturn> {
        self.initialize(resources)?;
        match index {
            434 => {
                if self.enabled == Some(true) {
                    self.requests.clear();
                    self.images.clear();
                    self.models.clear();
                    self.bytes = 0;
                    self.completed_through = i32::MAX;
                }
                Ok(NativeReturn::Continue)
            }
            435 => {
                // The wrapper formats category before filename, even when
                // caching is disabled. Both are bounded stack buffers.
                let category = formatted(vm, read_u32(args, 4)?)?;
                let name = formatted(vm, read_u32(args, 0)?)?;
                let priority = read_u32(args, 8)? as i32;
                let name = String::from_utf8(cp932_to_utf8(&name))
                    .expect("CP932 conversion emits UTF-8");
                let candidates = resources.category_names(&category, &name)
                    .map_err(|_| native_state_error("native 435 category resolution failed"))?;
                // Registration completes the name without opening it. The
                // batch worker performs the exact per-slot read later.
                let resolved = candidates.first()
                    .ok_or_else(|| native_state_error("native 435 category resolved no names"))?;
                if resolved.encode_utf16().count() >= 260 {
                    return Err(native_state_error("native 435 resolved name exceeds 259 UTF-16 units"));
                }
                if self.enabled == Some(true) {
                    if let Some(existing) = self.requests.iter_mut()
                        .find(|request| request.name.eq_ignore_ascii_case(resolved))
                    {
                        // Re-registration refreshes priority only; the
                        // completed buffer/attempt flag and age remain.
                        existing.priority = priority;
                    } else {
                        if self.requests.len() >= MAX_ENTRIES {
                            return Err(native_state_error("native 435 resource queue exceeds one million entries"));
                        }
                        self.requests.push(Request {
                            name: resolved.clone(), priority, age: priority,
                            attempted: false, resource: None,
                        });
                    }
                }
                Ok(NativeReturn::Continue)
            }
            436 => {
                if self.enabled == Some(true) {
                    self.ready_model();
                    // The list sort uses +0x218's age, not refreshed
                    // +0x214 priority (0x52ac20..0x52ac2c).
                    self.requests.sort_by_key(|request| request.age);
                    self.completed_through = 0;
                    for request in &mut self.requests {
                        if request.attempted { continue; }
                        let remaining = MAX_LIVE - self.bytes;
                        let (resource, version) = if let Some(prepared) = self.ahead.cached(&request.name) {
                            if prepared.bytes.capacity() <= remaining {
                                (Some(prepared.clone()), prepared.version.clone())
                            } else { (None,None) }
                        } else { read_optional(resources, &request.name, remaining)? };
                        let total = self.bytes + resource.as_ref().map_or(0, |resource| resource.bytes.capacity());
                        if let Some(resource) = &resource {
                            self.images.submit(&resource.bytes);
                            if let Some(version) = version
                                && !self.models.iter().any(|model| model.version == version)
                                && let Some(model) = submit_model(&mut self.model_worker, &request.name, version, &resource.bytes, &self.model_budget) {
                                if self.models.len() >= MAX_MODELS { self.models.pop_front(); }
                                self.models.push_back(model);
                            }
                        }
                        request.resource = resource;
                        request.attempted = true;
                        self.bytes = total;
                        self.completed_through = request.priority;
                    }
                    self.completed_through = i32::MAX;
                }
                Ok(NativeReturn::Continue)
            }
            437 => {
                let priority = read_u32(args, 0)? as i32;
                Ok(NativeReturn::Value(u32::from(
                    self.enabled != Some(true) || priority <= self.completed_through,
                )))
            }
            438 => {
                if self.enabled == Some(true) {
                    for request in &mut self.requests {
                        request.priority = request.priority.wrapping_sub(1);
                        if request.priority >= 0 { request.age = request.age.wrapping_sub(1); }
                    }
                    let mut reclaimed = 0;
                    self.requests.retain(|request| {
                        if request.priority < 0 {
                            reclaimed += request.resource.as_ref().map_or(0, |resource| resource.bytes.capacity());
                            false
                        } else { true }
                    });
                    self.bytes -= reclaimed;
                    self.models.retain(|model| self.requests.iter().any(|request| request.name == model.name)
                        || self.ahead.resources().any(|resource| resource.name == model.name));
                    self.completed_through = self.completed_through.wrapping_sub(1);
                }
                Ok(NativeReturn::Continue)
            }
            _ => Err(cs2_script::ScriptError::UnimplementedNativeCall { index }),
        }
    }

    fn initialize(&mut self, resources: &ResourceIndex) -> Result<()> {
        if self.enabled.is_some() { return Ok(()); }
        if let Some(config) = resources.startup_configuration()
            .map_err(|_| native_state_error("resource preload startup configuration failed"))? {
            self.enabled = Some(config.integer(&[b"document", b"ETC", b"cache"], 1) != 0);
            return Ok(());
        }
        self.enabled = Some(true);
        Ok(())
    }
}

fn formatted(vm: &Vm, address: u32) -> Result<Vec<u8>> {
    let expanded = expand_kcs_format(vm, &vm.read_string(address)?, 0)?;
    let bytes = &expanded[..expanded.iter().position(|&byte| byte == 0).unwrap_or(expanded.len())];
    if bytes.len() >= 260 {
        return Err(native_state_error("native 435 filename/category exceeds 259 bytes"));
    }
    Ok(bytes.to_vec())
}

fn read_optional(
    resources: &ResourceIndex, name: &str, remaining: usize,
) -> Result<(Option<PreparedResource>, Option<EmoteArchiveVersion>)> {
    let Some(candidate) = resources.resolve_mode_candidate(name, ResourceMode::LooseFirst)
        .map_err(|_| native_state_error("native 436 resource routing failed"))? else { return Ok((None, None)); };
    let Some(stamp) = read_ahead::ResourceStamp::observe(candidate) else { return Ok((None, None)); };
    let version = name.rsplit_once('.').filter(|(_, ext)| ext.eq_ignore_ascii_case("psz"))
        .and_then(|_| EmoteArchiveVersion::observe(candidate));
    match resources.read_candidate_bounded(candidate, remaining) {
        Ok(bytes) => {
            let version = version.filter(|before| Some(before) == EmoteArchiveVersion::observe(candidate).as_ref());
            if read_ahead::ResourceStamp::observe(candidate).as_ref() != Some(&stamp) { return Ok((None, None)); }
            Ok((bytes.map(|bytes| PreparedResource { resource: SourcedResource { source: candidate.clone(), bytes }, name: name.into(), stamp,
                version: version.clone() }), version))
        }
        Err(RuntimeError::ResourceNotFound(_)) => Ok((None, None)),
        Err(RuntimeError::Io { source, .. }) if matches!(source.kind(),
            std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied
        ) => Ok((None, None)),
        Err(_) => Err(native_state_error("native 436 resource read failed")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_preload_keeps_multiple_versions_in_a_bounded_queue() {
        let root = std::env::temp_dir().join(format!("cs2-model-queue-{}", std::process::id()));
        std::fs::write(&root, vec![0; 64]).unwrap();
        let mut bytes = b"PSZ\0".to_vec();
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&16u32.to_le_bytes());
        let mut cache = ResourcePreloads::default();
        for i in 0..2 {
            let source = ResourceCandidate { name: format!("model{i}.psz"), size: 16, priority: 0,
                location: cs2_runtime::ResourceLocation::Archive { path: root.clone(), offset: i * 16,
                    size: 16, name_seed: None, payload_key: None } };
            cache.enqueue_model(&source.name, EmoteArchiveVersion::observe(&source).unwrap(), &bytes);
        }
        assert_eq!(cache.models.len(), 2, "preparing one model must not discard the next model");
        std::fs::remove_file(root).unwrap();
    }

    #[test]
    fn model_budget_parallel_reservations_never_exceed_aggregate_limit() {
        let budget = Arc::new(ModelBudget::default());
        let (sender,receiver) = mpsc::channel();
        let mut threads = Vec::new();
        for _ in 0..8 {
            let budget = budget.clone(); let sender = sender.clone();
            threads.push(std::thread::spawn(move || {
                sender.send(ModelBudget::reserve(&budget,MAX_MODEL_BYTES)).unwrap();
            }));
        }
        drop(sender);
        let leases = receiver.into_iter().flatten().collect::<Vec<_>>();
        for thread in threads { thread.join().unwrap(); }
        let total = leases.iter().map(|lease|lease.bytes).sum::<usize>();
        assert_eq!(total,MAX_MODEL_POOL_BYTES);
        assert_eq!(budget.0.load(Ordering::Acquire),total);
        drop(leases);
        assert_eq!(budget.0.load(Ordering::Acquire),0);
    }

    #[test]
    fn model_budget_leases_release_only_after_the_last_queue_clone() {
        let budget = Arc::new(ModelBudget::default());
        budget.0.store(1024, Ordering::Release);
        let lease = Arc::new(ModelCharge { budget: budget.clone(), bytes: 1024 });
        let clone = lease.clone();
        drop(lease);
        assert_eq!(budget.0.load(Ordering::Acquire), 1024);
        drop(clone);
        assert_eq!(budget.0.load(Ordering::Acquire), 0);
    }

    #[test]
    fn explicit_cached_bytes_reject_replaced_file_identity() {
        let root = std::env::temp_dir().join(format!("cs2-preload-version-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("resource.bin"), b"old").unwrap();
        let resources = ResourceIndex::scan(&root).unwrap();
        let resource = read_optional(&resources, "resource.bin", 100).unwrap().0;
        let mut cache = ResourcePreloads::default();
        cache.requests.push(Request { name: "resource.bin".into(), priority: 0, age: 0,
            attempted: true, resource });
        assert_eq!(cache.cached("resource.bin"), Some(b"old".as_slice()));
        std::fs::write(root.join("resource.bin"), b"new content").unwrap();
        assert!(cache.cached("resource.bin").is_none(), "stale bytes must never be paired with a new model version");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[ignore = "requires the local PSZ fixture"]
    fn model_worker_reuses_the_exact_archived_source() -> anyhow::Result<()> {
        let root = std::env::var_os("CS2_EMOTE_FIXTURE_ROOT").unwrap();
        let resources = ResourceIndex::scan_with_name_seed(root, Some(0x0F6BB0AE))?;
        let name = "ca01_l_body_1.psz";
        let candidate = resources.resolve_mode_candidate(&format!("psz.int/{name}"), ResourceMode::LooseFirst)?.unwrap();
        let version = EmoteArchiveVersion::observe(candidate).unwrap();
        let packed = resources.read_candidate(candidate)?;
        let mut preloads = ResourcePreloads::default();
        preloads.enqueue_model(name, version.clone(), &packed);
        eprintln!("prefetch fixture packed={} expanded={} limit={}",packed.len(),u32::from_le_bytes(packed[8..12].try_into().unwrap()),MAX_MODEL_BYTES);
        for _ in 0..1000 {
            if preloads.ready_model().is_some() { break; }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let model = preloads.take_model(&version).expect("worker prepared the registered model");
        eprintln!("prefetch fixture resident_estimate={}",model.estimated_storage_bytes());
        assert!(!model.textures.is_empty());
        assert!(model.estimated_storage_bytes() <= MAX_MODEL_BYTES);
        assert!(preloads.take_model(&version).is_none());
        Ok(())
    }

    #[test]
    fn resource_preload_budget_miss_leaves_demand_read_available() {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let root = std::env::temp_dir().join(format!("cs2-preload-budget-{stamp}"));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("resource.bin"), b"x").unwrap();
        let resources = ResourceIndex::scan(&root).unwrap();
        std::fs::write(root.join("resource.bin"), b"grown").unwrap();

        assert!(read_optional(&resources, "resource.bin", 4).unwrap().0.is_none());
        assert!(read_optional(&resources, "missing.bin", 5).unwrap().0.is_none());
        assert_eq!(read_optional(&resources, "resource.bin", 5).unwrap().0.unwrap().bytes, b"grown");
        assert_eq!(resources.read("resource.bin").unwrap(), b"grown");
        std::fs::remove_dir_all(root).unwrap();
    }
}

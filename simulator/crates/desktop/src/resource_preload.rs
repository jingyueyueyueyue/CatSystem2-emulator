//! Ordinary 434..438's explicit resource queue and cached bytes.
//! A portable synchronous worker completes a requested batch at 436. It keeps
//! real resource failures distinct from a completed, missing-file attempt.
use cs2_runtime::{ResourceCandidate, ResourceIndex, RuntimeError, resource_router::ResourceMode};
use cs2_runtime::emote::{CpuMotionModel, EmoteArchiveVersion, EmoteResource};
use cs2_script::{NativeReturn, Result, Vm};
use std::sync::{Arc, Mutex, Weak, mpsc};

use super::{cp932_to_utf8, expand_kcs_format, native_state_error, read_u32};

#[path = "image_predecode.rs"]
mod images;

const MAX_LIVE: usize = 256 * 1024 * 1024;
const MAX_ENTRIES: usize = 1_000_000;
const MAX_MODEL_BYTES: usize = 96 * 1024 * 1024;
const MAX_PACKED_MODEL_BYTES: usize = 8 * 1024 * 1024;

type ModelResult = Option<Arc<CpuMotionModel>>;
type ModelReceiver = Mutex<mpsc::Receiver<ModelResult>>;

#[derive(Debug)]
struct ModelJob {
    bytes: Vec<u8>,
    receiver: Weak<ModelReceiver>,
    result: mpsc::Sender<ModelResult>,
}

#[derive(Debug, Clone)]
struct ModelPreload {
    name: String,
    version: EmoteArchiveVersion,
    receiver: Arc<ModelReceiver>,
    ready: ModelResult,
}

fn start_model_worker() -> Option<mpsc::SyncSender<ModelJob>> {
    let (sender, jobs) = mpsc::sync_channel::<ModelJob>(1);
    std::thread::Builder::new().name("cs2-model-preload".into())
        .spawn(move || {
            while let Ok(job) = jobs.recv() {
                if job.receiver.strong_count() == 0 {
                    continue;
                }
                let model = EmoteResource::decode(&job.bytes).ok()
                    .and_then(|resource| CpuMotionModel::load_for_preload(&resource, MAX_MODEL_BYTES).ok())
                    .map(Arc::new);
                let _ = job.result.send(model);
            }
        }).ok()?;
    Some(sender)
}

fn submit_model(
    worker: &mut Option<mpsc::SyncSender<ModelJob>>, name: &str,
    version: EmoteArchiveVersion, bytes: &[u8],
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
    let job = ModelJob { bytes: bytes.to_vec(), receiver: Arc::downgrade(&receiver), result };
    match worker.as_ref()?.try_send(job) {
        Ok(()) => Some(ModelPreload { name: name.into(), version, receiver, ready: None }),
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
struct Request {
    name: String,
    priority: i32,
    age: i32,
    attempted: bool,
    resource: Option<SourcedResource>,
}

#[derive(Debug, Clone, Default)]
pub(super) struct ResourcePreloads {
    enabled: Option<bool>,
    requests: Vec<Request>,
    completed_through: i32,
    bytes: usize,
    images: images::ImagePredecode,
    model_worker: Option<mpsc::SyncSender<ModelJob>>,
    model: Option<ModelPreload>,
}

impl ResourcePreloads {
    pub(crate) fn graph_request_count(&self) -> usize { self.requests.len() }
    pub(super) fn cached(&self, name: &str) -> Option<&[u8]> {
        self.cached_with_source(name).map(|resource| resource.bytes.as_slice())
    }

    pub(super) fn cached_with_source(&self, name: &str) -> Option<&SourcedResource> {
        self.requests.iter().find(|request| request.name.eq_ignore_ascii_case(name))
            .and_then(|request| request.resource.as_ref())
    }

    pub(super) fn ready_model(&mut self) -> Option<&Arc<CpuMotionModel>> {
        let model = self.model.as_mut()?;
        if model.ready.is_none() {
            let result = model.receiver.lock().ok()?.try_recv();
            match result {
                Ok(Some(ready)) => model.ready = Some(ready),
                Err(mpsc::TryRecvError::Empty) => return None,
                Ok(None) | Err(mpsc::TryRecvError::Disconnected) => {
                    self.model = None;
                    return None;
                }
            }
        }
        self.model.as_ref()?.ready.as_ref()
    }

    pub(super) fn take_model(&mut self, version: &EmoteArchiveVersion) -> ModelResult {
        if self.model.as_ref().is_some_and(|model| &model.version == version) {
            let model = self.model.take()?;
            model.ready.or_else(|| model.receiver.lock().ok()?.recv().ok().flatten())
        } else {
            None
        }
    }

    pub(super) fn predecode_image(&mut self, bytes: &[u8]) { self.images.submit(bytes); }

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
                    self.model = None;
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
                        let (resource, version) =
                            read_optional(resources, &request.name, MAX_LIVE - self.bytes)?;
                        let total = self.bytes + resource.as_ref().map_or(0, |resource| resource.bytes.capacity());
                        if let Some(resource) = &resource {
                            self.images.submit(&resource.bytes);
                            if self.model.is_none() && let Some(version) = version {
                                self.model = submit_model(&mut self.model_worker, &request.name, version, &resource.bytes);
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
                    if self.model.as_ref().is_some_and(|model|
                        !self.requests.iter().any(|request| request.name == model.name))
                    {
                        self.model = None;
                    }
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
) -> Result<(Option<SourcedResource>, Option<EmoteArchiveVersion>)> {
    let Some(candidate) = resources.resolve_mode_candidate(name, ResourceMode::LooseFirst)
        .map_err(|_| native_state_error("native 436 resource routing failed"))? else { return Ok((None, None)); };
    let version = name.rsplit_once('.').filter(|(_, ext)| ext.eq_ignore_ascii_case("psz"))
        .and_then(|_| EmoteArchiveVersion::observe(candidate));
    match resources.read_candidate_bounded(candidate, remaining) {
        Ok(bytes) => {
            let version = version.filter(|before| Some(before) == EmoteArchiveVersion::observe(candidate).as_ref());
            Ok((bytes.map(|bytes| SourcedResource { source: candidate.clone(), bytes }), version))
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
    #[ignore = "requires the local PSZ fixture"]
    fn model_worker_reuses_the_exact_archived_source() -> anyhow::Result<()> {
        let root = std::env::var_os("CS2_EMOTE_FIXTURE_ROOT").unwrap();
        let resources = ResourceIndex::scan_with_name_seed(root, Some(0x0F6BB0AE))?;
        let name = "ca01_l_body_1.psz";
        let candidate = resources.category_names(b"m2psb", name)?.iter()
            .find_map(|name| resources.resolve_mode_candidate(name, ResourceMode::LooseFirst).transpose())
            .transpose()?.unwrap();
        let version = EmoteArchiveVersion::observe(candidate).unwrap();
        let packed = resources.read_candidate(candidate)?;
        let mut preloads = ResourcePreloads::default();
        preloads.model = submit_model(&mut preloads.model_worker, name, version.clone(), &packed);
        let model = preloads.take_model(&version).expect("worker prepared the registered model");
        assert!(model.textures.values().all(|texture| texture.is_bgra()));
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

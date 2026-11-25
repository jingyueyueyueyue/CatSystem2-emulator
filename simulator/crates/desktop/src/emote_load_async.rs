//! Bounded cold M2 preparation. Workers own resources only; scene/VM publication
//! happens exclusively when the original caller polls its retained request.
use super::*;
use crate::resource_preload::ModelSubscription;
use cs2_runtime::{ResourceCandidate, emote::CpuMotionModel};
use std::{collections::BTreeSet, sync::{Mutex, Weak, mpsc, atomic::{AtomicUsize, Ordering}}, time::{Duration, Instant}};

const MAX_PENDING: usize = 16;
const MAX_PACKED: usize = 64 * 1024 * 1024;
const MAX_MODEL: usize = 256 * 1024 * 1024;
const MAX_POOL: usize = 512 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(30);
type PreparedResult = std::result::Result<Prepared, EmoteError>;

pub(super) struct Prepared {
    pub(super) plan: EmoteM2ResourcePlan,
    pub(super) model: Arc<CpuMotionModel>,
    pub(super) version: EmoteArchiveVersion,
    identities: Vec<(ResourceCandidate, EmoteArchiveVersion)>,
    pub(super) phases_us: [u128; 4],
    pub(super) preload_wait_us: Option<u128>,
    _charge: Option<Charge>,
}
pub(super) enum CachedModel {
    Current(Arc<CpuMotionModel>),
    Preload(ModelSubscription),
}
struct PlanWork {
    plan: EmoteM2ResourcePlan,
    source: ResourceCandidate,
    version: EmoteArchiveVersion,
    identities: Vec<(ResourceCandidate, EmoteArchiveVersion)>,
    config_us: u128,
}
impl PlanWork {
    fn reuse(self, model: Arc<CpuMotionModel>, preload_wait_us: Option<u128>) -> Prepared {
        Prepared {
            plan: self.plan, model, version: self.version, identities: self.identities,
            phases_us: [self.config_us, 0, 0, 0], preload_wait_us, _charge: None,
        }
    }
}

struct WaitingPreload {
    work: PlanWork,
    subscription: ModelSubscription,
    started: Instant,
}
enum Work { Plan(Vec<u8>), Model(PlanWork) }
enum Stage { Plan(PlanWork), Ready(Prepared) }
type StageResult = std::result::Result<Stage, EmoteError>;
struct Charge { pool: Arc<AtomicUsize>, bytes: usize }
impl Drop for Charge { fn drop(&mut self) { self.pool.fetch_sub(self.bytes, Ordering::AcqRel); } }
impl Charge {
    fn reserve(pool: Arc<AtomicUsize>) -> std::result::Result<Self, EmoteError> {
        let mut used = pool.load(Ordering::Acquire);
        loop {
            let available = MAX_POOL.saturating_sub(used).min(MAX_MODEL);
            if available == 0 { return Err(EmoteError::Capacity); }
            let next = used + available;
            match pool.compare_exchange_weak(used, next, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => return Ok(Self { pool, bytes: available }),
                Err(current) => used = current,
            }
        }
    }
    fn shrink(&mut self, actual: usize) {
        self.pool.fetch_sub(self.bytes.saturating_sub(actual), Ordering::AcqRel);
        self.bytes = actual;
    }
}
struct Pending {
    target: u32,
    spec: Vec<u8>,
    mode: u32,
    filter: u32,
    imports: u32,
    started: Instant,
    result: Arc<Mutex<Option<StageResult>>>,
    next: Option<Work>,
    waiting: Option<WaitingPreload>,
}
struct Job {
    resources: Arc<ResourceIndex>,
    work: Work,
    filter: u32,
    result: Weak<Mutex<Option<StageResult>>>,
    budget: Arc<AtomicUsize>,
    #[cfg(test)]
    gate: Option<Arc<Mutex<mpsc::Receiver<()>>>>,
}
#[derive(Default)]
pub(super) struct Loads {
    resources: Option<Arc<ResourceIndex>>,
    worker: Option<mpsc::SyncSender<Job>>,
    pending: BTreeMap<u32, Pending>,
    cancelled: BTreeSet<u32>,
    budget: Arc<AtomicUsize>,
    #[cfg(test)]
    gate: Option<Arc<Mutex<mpsc::Receiver<()>>>>,
}

fn rejected(reason: &'static str) -> EmoteError { EmoteError::InvalidResource(reason) }

impl Loads {
    pub(super) fn set_resources(&mut self, resources: Arc<ResourceIndex>) {
        if self.resources.is_none() { self.resources = Some(resources); }
    }
    pub(super) fn owners(&self) -> impl Iterator<Item = u32> + '_ {
        self.pending.keys().chain(self.cancelled.iter()).copied()
    }
    pub(super) fn has_pending(&self) -> bool { !self.pending.is_empty() || !self.cancelled.is_empty() }
    pub(super) fn saved(&self, owner: u32) -> Option<(u32, Vec<u8>, u32)> {
        self.pending.get(&owner).map(|pending| (pending.target, pending.spec.clone(), pending.mode))
    }
    pub(super) fn check_cancelled(&mut self, owner: u32) -> std::result::Result<(), EmoteError> {
        if self.cancelled.remove(&owner) { return Err(rejected("M2 preparation target was released")); }
        Ok(())
    }
    pub(super) fn cancel(&mut self, id: u32) {
        self.cancelled.remove(&id);
        self.pending.remove(&id);
        self.cancel_target(id);
    }
    pub(super) fn cancel_target(&mut self, id: u32) {
        self.pending.retain(|owner, pending| {
            if pending.target == id { self.cancelled.insert(*owner); return false; }
            true
        });
    }
    pub(super) fn poll(&mut self, resources: &ResourceIndex, owner: u32, target: u32,
        spec: &[u8], mode: u32, filter: u32, imports: u32,
        mut cached: impl FnMut(&EmoteM2ResourcePlan, &EmoteArchiveVersion) -> Option<CachedModel>,
    ) -> std::result::Result<Option<Prepared>, EmoteError> {
        self.check_cancelled(owner)?;
        if !self.pending.contains_key(&owner) {
            if spec.len() > MAX_FORMAT_BYTES + 1 { return Err(EmoteError::Capacity); }
            if self.pending.len() >= MAX_PENDING { return Err(EmoteError::Capacity); }
            self.pending.insert(owner, Pending { target, spec: spec.to_vec(), mode, filter, imports,
                started: Instant::now(), result: Arc::new(Mutex::new(None)), next: Some(Work::Plan(spec.to_vec())), waiting: None });
        }
        let pending = self.pending.get_mut(&owner).unwrap();
        let mut result = pending.result.try_lock().ok().and_then(|mut slot| slot.take());
        if let Some(status) = pending.waiting.as_ref().and_then(|waiting| waiting.subscription.poll_model()) {
            let WaitingPreload { work, started, .. } = pending.waiting.take().unwrap();
            match status {
                Ok(model) => result = Some(Ok(Stage::Ready(work.reuse(model, Some(started.elapsed().as_micros()))))),
                Err(_) => pending.next = Some(Work::Model(work)),
            }
        }
        // A completed model remains usable after a paused application resumes;
        // the deadline applies while preparation is still unfinished.
        if pending.started.elapsed() >= TIMEOUT && !matches!(&result, Some(Ok(Stage::Ready(_)))) {
            self.pending.remove(&owner);
            return Err(rejected("M2 preparation timed out"));
        }
        if pending.target != target || pending.filter != filter || pending.imports != imports {
            self.pending.remove(&owner);
            return Err(rejected("M2 preparation target changed"));
        }
        if let Some(work) = pending.next.take() {
            if self.resources.is_none() { self.resources = Some(Arc::new(resources.clone())); }
            if self.worker.is_none() {
                match start_worker() {
                    Ok(worker) => self.worker = Some(worker),
                    Err(cause) => { self.pending.remove(&owner); return Err(cause); }
                }
            }
            let job = Job { resources: self.resources.as_ref().unwrap().clone(), work,
                filter: pending.filter, result: Arc::downgrade(&pending.result), budget: self.budget.clone(),
                #[cfg(test)] gate: self.gate.clone(),
            };
            match self.worker.as_ref().unwrap().try_send(job) {
                Ok(()) => {},
                Err(mpsc::TrySendError::Full(job)) => pending.next = Some(job.work),
                Err(mpsc::TrySendError::Disconnected(_)) => {
                    self.worker = None;
                    self.pending.remove(&owner);
                    return Err(rejected("M2 preparation worker disconnected"));
                }
            }
            return Ok(None);
        }
        let Some(result) = result else { return Ok(None); };
        let stage = match result { Ok(stage) => stage, Err(cause) => { self.pending.remove(&owner); return Err(cause); } };
        let prepared = match stage {
            Stage::Plan(work) => {
                if work.identities.iter().any(|(candidate, version)| EmoteArchiveVersion::observe(candidate).as_ref() != Some(version)) {
                    self.pending.remove(&owner);
                    return Err(rejected("M2 preparation configuration changed"));
                }
                match cached(&work.plan, &work.version) {
                    Some(CachedModel::Current(model)) => work.reuse(model, None),
                    Some(CachedModel::Preload(subscription)) => match subscription.poll_model() {
                        Some(Ok(model)) => work.reuse(model, Some(0)),
                        Some(Err(_)) => {
                            pending.next = Some(Work::Model(work));
                            return Ok(None);
                        }
                        None => {
                            pending.waiting = Some(WaitingPreload { work, subscription, started: Instant::now() });
                            return Ok(None);
                        }
                    },
                    None => {
                        pending.next = Some(Work::Model(work));
                        return Ok(None);
                    }
                }
            }
            Stage::Ready(prepared) => prepared,
        };
        self.pending.remove(&owner);
        if prepared.identities.iter().any(|(candidate, version)|
            EmoteArchiveVersion::observe(candidate).as_ref() != Some(version)) {
            return Err(rejected("M2 preparation resource changed"));
        }
        Ok(Some(prepared))
    }
}

fn start_worker() -> std::result::Result<mpsc::SyncSender<Job>, EmoteError> {
    // Jobs and completed result slots share an explicit 512MiB reservation pool.
    let (sender, receiver) = mpsc::sync_channel::<Job>(1);
    std::thread::Builder::new().name("cs2-m2-cold-prepare".into()).spawn(move || {
        while let Ok(job) = receiver.recv() {
            if job.result.strong_count() == 0 { continue; }
            #[cfg(test)]
            if matches!(&job.work, Work::Model(_)) {
                if let Some(gate) = &job.gate { let _ = gate.lock().unwrap().recv(); }
            }
            if job.result.strong_count() == 0 { continue; }
            let result = match job.work {
                Work::Plan(spec) => plan(&job.resources, &spec).map(Stage::Plan),
                Work::Model(work) => prepare(&job.resources, work, job.filter, job.budget).map(Stage::Ready),
            };
            if let Some(slot) = job.result.upgrade() {
                if let Ok(mut slot) = slot.lock() { *slot = Some(result); }
            }
        }
    }).map_err(|cause| EmoteError::Backend(cause.to_string()))?;
    Ok(sender)
}

fn plan(resources: &ResourceIndex, spec: &[u8]) -> std::result::Result<PlanWork, EmoteError> {
    let started = Instant::now();
    let mut identities = Vec::new();
    let plan = load_m2_resource_plan_impl(resources, b"m2config", b"m2view.cnf", spec, None, Some(&mut identities))?;
    let (name, _, invalid) = encoding_rs::SHIFT_JIS.decode(&plan.resource_name);
    if invalid || plan.resource_name.is_empty() || plan.resource_name.contains(&0) {
        return Err(rejected("M2 resource name"));
    }
    let mut source = None;
    for name in resources.category_names(&plan.resource_category, &name).map_err(|cause| EmoteError::Backend(cause.to_string()))? {
        if let Some(candidate) = resources.resolve_mode_candidate(&name, ResourceMode::LooseFirst)
            .map_err(|cause| EmoteError::Backend(cause.to_string()))? { source = Some(candidate); break; }
    }
    let source = source.ok_or(rejected("M2 resource not found"))?;
    let version = EmoteArchiveVersion::observe(source).ok_or(rejected("M2 resource version unavailable"))?;
    identities.push((source.clone(), version.clone()));
    Ok(PlanWork { plan, source: source.clone(), version, identities, config_us: started.elapsed().as_micros() })
}

fn prepare(resources: &ResourceIndex, work: PlanWork, filter: u32, budget: Arc<AtomicUsize>) -> PreparedResult {
    let mut charge = Charge::reserve(budget)?;
    let PlanWork { plan, source, version, identities, config_us } = work;
    let started = Instant::now();
    let packed = resources.read_candidate_bounded(&source, MAX_PACKED)
        .map_err(|cause| EmoteError::Backend(cause.to_string()))?.ok_or(EmoteError::Capacity)?;
    if packed.starts_with(b"PSZ\0") && packed.get(8..12)
        .map(|bytes| u32::from_le_bytes(bytes.try_into().unwrap()) as usize).is_none_or(|size| size > charge.bytes) {
        return Err(EmoteError::Capacity);
    }
    let read_us = started.elapsed().as_micros();
    let started = Instant::now();
    let resource = EmoteResource::decode(&packed)?;
    let decode_us = started.elapsed().as_micros();
    let started = Instant::now();
    let model = Arc::new(CpuMotionModel::load_for_preload_with_filter(&resource, filter, charge.bytes)?);
    let load_us = started.elapsed().as_micros();
    let actual = model.estimated_storage_bytes();
    if actual > charge.bytes { return Err(EmoteError::Capacity); }
    charge.shrink(actual);
    if identities.iter().any(|(candidate, version)| EmoteArchiveVersion::observe(candidate).as_ref() != Some(version)) {
        return Err(rejected("M2 resource changed during preparation"));
    }
    Ok(Prepared { plan, model, version, identities, phases_us: [config_us, read_us, decode_us, load_us],
        preload_wait_us: None, _charge: Some(charge) })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_player() -> anyhow::Result<(std::path::PathBuf, crate::HeadlessPlayer, u32, mpsc::Sender<()>)> {
        let root = std::env::var_os("CS2_EMOTE_FIXTURE_ROOT").ok_or_else(|| anyhow::anyhow!("CS2_EMOTE_FIXTURE_ROOT required"))?;
        let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos();
        let save = std::env::temp_dir().join(format!("cs2-m2-native906-{}-{nonce}", std::process::id()));
        std::fs::create_dir_all(&save)?;
        let mut player = crate::HeadlessPlayer::open_with_save_root(&root, &save)?;
        player.scan.resources = ResourceIndex::scan_with_name_seed(root, Some(0x0F6BB0AE))?;
        player.scan.resources.initialize_resource_router()?;
        // Select a real profile row for the known body payload, rather than
        // replacing the native's model/config decoder with a fake backend.
        let resources = &player.scan.resources;
        let view = CpuM2Config::parse_cp932(&resources.read_category(b"m2config", "m2view.cnf")?)?;
        let category = &view.records(b"profilepath")[0][0];
        let mut spec = None;
        for row in view.records(b"profile") {
            let (filename, _, _) = encoding_rs::SHIFT_JIS.decode(&row[1]);
            let profile = CpuM2Config::parse_cp932(&resources.read_category(category, &filename)?)?;
            if let Some(chara) = profile.records(b"charaname").iter().find(|chara| chara.get(2).is_some_and(|name| name == b"ca01_l_body_1")) {
                let motion = profile.records(b"motname").iter().find(|motion| motion[0] == chara[0]).unwrap();
                let mut value = row[0].clone(); value.push(b','); value.extend_from_slice(&chara[0]);
                value.push(b':'); value.extend_from_slice(&motion[1]); value.push(0);
                spec = Some(value); break;
            }
        }
        let spec = spec.ok_or_else(|| anyhow::anyhow!("body fixture profile absent"))?;
        let (completion, gate) = mpsc::channel();
        let target = player.with_host(|host| {
            host.data_services.emote.initialize(host.resources)?;
            host.data_services.emote.loads.resources = None;
            host.data_services.emote.loads.gate = Some(Arc::new(Mutex::new(gate)));
            Ok(host.create_task_object(54, host.current_task_id, b"m2plane", b"")?)
        })?;
        let mut code = Vec::new();
        for (opcode, operands) in [(0x25, vec![target]), (0x25, vec![0]), (0x25, vec![0]), (0x2b, vec![906, 12])] {
            code.push(opcode);
            for value in operands { let operand = cs2_script::Operand::immediate(value);
                code.push(operand.flags.bits()); code.extend_from_slice(&operand.payload.to_le_bytes()); }
        }
        code.push(0x3b);
        let mut data = vec![0; 256]; data[..spec.len()].copy_from_slice(&spec);
        let vm = Vm::from_program(cs2_script::KcsProgram::from_code(&code, data, 64)?)?;
        player.data_services.tasks.set_factory_external(vm.external_binding());
        player.data_services.tasks.register_external_task(player.current_task_id, 0, &player.data_services.language.languages)?;
        player.vm = Some(vm);
        Ok((save, player, target, completion))
    }

    #[test]
    #[ignore = "requires local NEKOPARA fixture archives"]
    fn native906_real_cold_worker_keeps_frames_and_script_order() -> anyhow::Result<()> {
        let (save, mut player, target, completion) = fixture_player()?;
        let frame = player.state.clock.frame;
        for _ in 0..8 {
            assert!(player.advance_host_frame_with_retries(1000)?);
            assert_eq!(player.vm.as_ref().unwrap().instruction_pointer, 18);
            assert_eq!(player.data_services.emote.state.state(target).unwrap().successful_resource_imports, 0);
        }
        assert_eq!(player.state.clock.frame, frame + 8);
        assert!(player.capture_object_graph().unwrap_err().to_string().contains("asynchronous E-mote"));
        let original_spec = player.vm.as_ref().unwrap().read_string(0)?;
        player.vm.as_mut().unwrap().write(0, b"bad,99\0")?;
        completion.send(())?;
        let started = Instant::now();
        let mut frames = 0;
        let mut max_frame = Duration::ZERO;
        while player.data_services.emote.has_pending_loads() {
            let tick = Instant::now();
            let (advanced, details) = crate::perf_detail::HostPerfDetails::capture(false, || player.advance_host_frame_with_retries(1000));
            assert!(advanced?);
            if !player.data_services.emote.has_pending_loads() { eprintln!("{}", details.rows(0, 0, 0)); }
            max_frame = max_frame.max(tick.elapsed()); frames += 1;
            assert!(started.elapsed() < Duration::from_secs(20));
            std::thread::yield_now();
        }
        assert_eq!(player.data_services.emote.state.state(target).unwrap().successful_resource_imports, 1);
        assert!(player.vm.as_ref().unwrap().instruction_pointer > 18);
        eprintln!("native906 real cold frames_while_pending={frames} wall_us={} max_frame_us={}", started.elapsed().as_micros(), max_frame.as_micros());
        // The second load must reuse the exact current model before dispatching
        // a model job: its test gate has no second completion token.
        let mut vm = Vm::from_program(cs2_script::KcsProgram::from_code(&[0x3b], vec![0; 256], 64)?)?;
        vm.write(0, &original_spec)?;
        let args = [target, 0, 0].into_iter().flat_map(u32::to_le_bytes).collect::<Vec<_>>();
        let started = Instant::now();
        loop {
            let result = player.with_host(|host| Ok(host.emote_native(906, &args, &mut vm)?))?;
            if result != NativeReturn::Retry { assert_eq!(result, NativeReturn::Continue); break; }
            assert!(started.elapsed() < Duration::from_secs(5), "current model reuse must bypass the blocked cold decoder");
            std::thread::yield_now();
        }
        drop(player); std::fs::remove_dir_all(save)?;
        Ok(())
    }

    #[test]
    #[ignore = "requires local NEKOPARA fixture archives"]
    fn native906_real_cold_cancellation_and_timeout_reject_late_completion() -> anyhow::Result<()> {
        for timeout in [false, true] {
            let (save, mut player, target, completion) = fixture_player()?;
            assert!(player.advance_host_frame_with_retries(1000)?);
            let owner = player.current_task_id;
            let started = Instant::now();
            loop {
                if matches!(player.data_services.emote.loads.pending[&owner].next, Some(Work::Model(_))) {
                    assert!(player.advance_host_frame_with_retries(1000)?);
                    break;
                }
                assert!(player.advance_host_frame_with_retries(1000)?);
                assert!(started.elapsed() < Duration::from_secs(10));
                std::thread::yield_now();
            }
            if timeout {
                player.data_services.emote.loads.pending.get_mut(&owner).unwrap().started = Instant::now() - TIMEOUT;
            } else {
                player.with_host(|host| Ok(host.emote_release_task_hook(target)?))?;
            }
            let failure = format!("{:#}", player.advance_host_frame_with_retries(1000).unwrap_err());
            assert!(failure.contains(if timeout { "timed out" } else { "released" }), "{failure}");
            assert!(!player.data_services.emote.has_pending_loads());
            let _ = completion.send(());
            assert!(player.data_services.emote.state.state(target).is_none_or(|state| state.successful_resource_imports == 0));
            drop(player); std::fs::remove_dir_all(save)?;
        }
        Ok(())
    }

    #[test]
    fn completed_model_reservations_are_bounded_until_results_are_dropped() {
        let pool = Arc::new(AtomicUsize::new(0));
        let first = Charge::reserve(pool.clone()).unwrap();
        let second = Charge::reserve(pool.clone()).unwrap();
        assert!(Charge::reserve(pool.clone()).is_err());
        drop(first);
        assert!(Charge::reserve(pool.clone()).is_ok());
        drop(second);
        assert_eq!(pool.load(Ordering::Acquire), 0);
    }

    #[test]
    fn pending_model_preparation_rejects_changed_targets_and_keeps_the_original_deadline() -> anyhow::Result<()> {
        let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos();
        let root = std::env::temp_dir().join(format!("cs2-m2-target-{}-{nonce}", std::process::id()));
        std::fs::create_dir(&root)?;
        let resources = ResourceIndex::scan(&root)?;
        let mut loads = Loads::default();
        assert!(loads.poll(&resources, 1, 2, b"actor", 7, 0, 0, |_, _| None)?.is_none());
        assert_eq!(loads.saved(1), Some((2, b"actor".to_vec(), 7)));
        assert!(loads.poll(&resources, 1, 2, b"changed", 0, 1, 0, |_, _| None).is_err());
        assert!(!loads.has_pending());
        assert!(loads.poll(&resources, 1, 2, b"actor", 7, 0, 0, |_, _| None)?.is_none());
        loads.pending.get_mut(&1).unwrap().started = Instant::now() - TIMEOUT;
        assert!(loads.poll(&resources, 1, 2, b"changed", 0, 0, 0, |_, _| None).err().unwrap().to_string().contains("timed out"));
        assert!(!loads.has_pending());
        assert!(loads.poll(&resources, 1, 2, b"actor", 7, 0, 0, |_, _| None)?.is_none());
        loads.cancel_target(2);
        assert!(loads.has_pending(), "a surviving caller must consume its cancelled native before graph capture");
        assert!(loads.poll(&resources, 1, 2, b"actor", 7, 0, 0, |_, _| None).is_err());
        assert!(!loads.has_pending());
        drop(loads); std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn model_preparation_read_failure_clears_the_retained_request() -> anyhow::Result<()> {
        let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos();
        let root = std::env::temp_dir().join(format!("cs2-m2-failed-{}-{nonce}", std::process::id()));
        std::fs::create_dir(&root)?;
        let resources = ResourceIndex::scan(&root)?;
        let mut loads = Loads::default();
        assert!(loads.poll(&resources, 1, 2, b"actor", 0, 0, 0, |_, _| None)?.is_none());
        let started = Instant::now();
        loop {
            match loads.poll(&resources, 1, 2, b"actor", 0, 0, 0, |_, _| None) {
                Err(_) => break,
                Ok(None) => std::thread::yield_now(),
                Ok(Some(_)) => panic!("an absent configuration cannot report a committed model"),
            }
            assert!(started.elapsed() < Duration::from_secs(2));
        }
        assert!(!loads.has_pending());
        drop(loads); std::fs::remove_dir_all(root)?;
        Ok(())
    }
}

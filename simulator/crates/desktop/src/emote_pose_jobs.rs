//! Interactive E-mote scene sampling. The pool owns only CPU work, while the
//! session validates and installs results on its existing render thread.

use super::{PreparedPose, PREPARED, RASTER, REQUIRE_RESIDENT_ATLASES, collect_pose_until_cancelled,
    mesh, own_draws, report, INDEXED_PREP_GEOMETRY_BYTES, MAX_INDEXED_PREP_POSES};
use cs2_runtime::emote::{CpuPlayer, CpuPlayerRenderState, CpuViewport, EmoteRasterFrame};
use std::collections::{BTreeMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock, atomic::{AtomicBool, AtomicUsize, Ordering}, mpsc};
use std::time::{Duration, Instant};

const INPUT_BYTES: usize = 8 * 1024 * 1024;
const DISPLAY_BYTES: usize = 128 * 1024 * 1024;
const POSE_TIMEOUT: Duration = Duration::from_secs(2);
const CONGESTION_RETRY: Duration = Duration::from_millis(16);
const WARN_AGE: Duration = Duration::from_millis(50);

static INPUT_USED: AtomicUsize = AtomicUsize::new(0);
static GEOMETRY_USED: AtomicUsize = AtomicUsize::new(0);
static DISPLAY_USED: AtomicUsize = AtomicUsize::new(0);

struct Reservation {
    used: &'static AtomicUsize,
    bytes: usize,
}

impl Reservation {
    fn acquire(used: &'static AtomicUsize, limit: usize, bytes: usize) -> Option<Self> {
        used.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |used| {
            used.checked_add(bytes).filter(|total| *total <= limit)
        }).ok()?;
        Some(Self { used, bytes })
    }

    fn shrink(&mut self, bytes: usize) {
        if bytes < self.bytes {
            self.used.fetch_sub(self.bytes - bytes, Ordering::Relaxed);
            self.bytes = bytes;
        }
    }

    fn resize(&mut self, limit: usize, bytes: usize) -> bool {
        if bytes <= self.bytes {
            self.shrink(bytes);
            return true;
        }
        let additional = bytes - self.bytes;
        if self.used.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |used| {
            used.checked_add(additional).filter(|total| *total <= limit)
        }).is_err() {
            return false;
        }
        self.bytes = bytes;
        true
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        self.used.fetch_sub(self.bytes, Ordering::Relaxed);
    }
}

struct Sample {
    tick: u64,
    revision: u64,
    scheduled: Instant,
    player: CpuPlayer,
    viewport: CpuViewport,
    variables: Arc<BTreeMap<String, f64>>,
    state: Arc<CpuPlayerRenderState>,
    _input: Reservation,
}

struct ActorGeometry {
    cache: mesh::GeometryCache,
    reservation: Option<Arc<Reservation>>,
}

impl Default for ActorGeometry {
    fn default() -> Self {
        Self { cache: mesh::GeometryCache::default(), reservation: None }
    }
}

struct Work {
    task: u32,
    generation: u64,
    sample: Arc<Sample>,
    cancelled: Arc<AtomicBool>,
    cache: ActorGeometry,
    done: mpsc::Sender<Completion>,
    #[cfg(test)]
    gate: Option<Arc<PoseTestGate>>,
}

#[cfg(test)]
#[derive(Default)]
pub(in crate) struct PoseTestGate {
    pub(in crate) started: AtomicBool,
    pub(in crate) released: AtomicBool,
}

struct Completion {
    task: u32,
    generation: u64,
    cache: ActorGeometry,
    result: WorkResult,
}

enum WorkResult {
    Ready(ReadyPose),
    Congested(Arc<Sample>),
    Unsupported,
}

pub(in crate) struct ReadyPose {
    sample: Arc<Sample>,
    pose: PreparedPose,
    atlas: PoseAtlas,
    atlas_names: Vec<String>,
    _geometry: Arc<Reservation>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PoseAtlas {
    Pending,
    Resident,
}

pub(in crate) struct InstalledReadyPose {
    player: usize,
    old_atlas_mode: bool,
}

impl Drop for InstalledReadyPose {
    fn drop(&mut self) {
        PREPARED.with_borrow_mut(|slots| slots.retain(|pose| pose.player != self.player));
        REQUIRE_RESIDENT_ATLASES.with(|resident| resident.set(self.old_atlas_mode));
    }
}

impl ReadyPose {
    pub(in crate) fn revision(&self) -> u64 {
        self.sample.revision
    }

    pub(in crate) fn sample(&self) -> (&CpuPlayer, &BTreeMap<String, f64>) {
        (&self.sample.player, &self.sample.variables)
    }

    pub(in crate) fn install(&self) -> InstalledReadyPose {
        let mut pose = self.pose.clone();
        // This address is only a same-call lookup on the renderer thread.
        pose.player = &self.sample.player as *const _ as usize;
        let player = pose.player;
        PREPARED.with_borrow_mut(|slots| slots.push(pose));
        let old_atlas_mode = REQUIRE_RESIDENT_ATLASES.with(|resident| resident.replace(true));
        InstalledReadyPose { player, old_atlas_mode }
    }
}

struct Pool {
    sender: mpsc::SyncSender<Work>,
}

impl Pool {
    fn get() -> Option<&'static Self> {
        static POOL: OnceLock<Option<Pool>> = OnceLock::new();
        POOL.get_or_init(|| {
            let workers = super::prep_worker_limit().min(if cfg!(target_os = "android") { 2 } else { 4 });
            let (sender, receive) = mpsc::sync_channel::<Work>(workers);
            let receive = Arc::new(Mutex::new(receive));
            let mut started = 0;
            for worker in 0..workers {
                let receive = Arc::clone(&receive);
                if std::thread::Builder::new().name(format!("cs2-emote-{worker}")).spawn(move || {
                    loop {
                        let Ok(work) = receive.lock().expect("E-mote receive lock").recv() else {
                            break;
                        };
                        work.run(worker);
                    }
                }).is_ok() {
                    started += 1;
                }
            }
            report(&format!("emote_pose_workers workers={started}"));
            (started != 0).then_some(Self { sender })
        }).as_ref()
    }
}

impl Work {
    fn run(mut self, worker: usize) {
        #[cfg(test)]
        if let Some(gate) = &self.gate {
            gate.started.store(true, Ordering::Release);
            while !gate.released.load(Ordering::Acquire) && !self.cancelled.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        if self.cancelled.load(Ordering::Relaxed) {
            return;
        }
        let start = Instant::now();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.evaluate(worker, start)))
            .unwrap_or_else(|_| {
                self.cache = ActorGeometry::default();
                WorkResult::Congested(Arc::clone(&self.sample))
            });
        if !self.cancelled.load(Ordering::Relaxed) {
            let _ = self.done.send(Completion {
                task: self.task,
                generation: self.generation,
                cache: self.cache,
                result,
            });
        }
    }

    fn evaluate(&mut self, worker: usize, start: Instant) -> WorkResult {
        let sample = &self.sample;
        let pose = match collect_pose_until_cancelled(&sample.player, sample.viewport, &sample.variables, true,
            Some(&self.cancelled)) {
            Ok(pose) => pose,
            Err(_) => return WorkResult::Unsupported,
        };
        let evaluated = Instant::now();
        if self.cancelled.load(Ordering::Relaxed) || mesh::compatible(&pose.draws).is_err() {
            return WorkResult::Unsupported;
        }
        // Reserve working space and existing retained data before replacing
        // the previous packet. The old charge can shrink independently.
        let bytes = self.cache.cache.reservation_bytes(&pose.draws);
        if bytes > INDEXED_PREP_GEOMETRY_BYTES {
            return WorkResult::Unsupported;
        }
        let Some(mut geometry) = Reservation::acquire(&GEOMETRY_USED, INDEXED_PREP_GEOMETRY_BYTES, bytes) else {
            // Drop this actor's retained topology before a retry so a group of
            // busy actors cannot hold the quota while waiting for each other.
            self.cache = ActorGeometry::default();
            return WorkResult::Congested(Arc::clone(sample));
        };
        let indexed = match self.cache.cache.prepare(&pose.draws, sample.viewport) {
            Ok(indexed) => indexed,
            Err(error) => {
                self.cache = ActorGeometry::default();
                report(&format!("emote_pose_fallback task={} reason={error:#}", self.task));
                return WorkResult::Unsupported;
            }
        };
        let draws_bytes = pose.draws.iter().map(|draw| {
            size_of::<super::OwnedDraw>() + draw.mesh.source.len() + 64
                + draw.mesh.vertices.capacity() * size_of::<[f64; 4]>()
        }).fold(0usize, usize::saturating_add);
        let mut unique = HashSet::new();
        let atlas_names: Vec<_> = indexed.active_draws().map(|index| pose.draws[index].mesh.source)
            .filter(|name| unique.insert(*name)).map(str::to_owned).collect();
        if atlas_names.iter().any(|name| !pose.model.textures.contains_key(name)) {
            return WorkResult::Unsupported;
        }
        let atlas_bytes = atlas_names.capacity() * size_of::<String>()
            + atlas_names.iter().map(String::capacity).sum::<usize>();
        let prepared = PreparedPose {
            player: 0,
            state: Arc::clone(&sample.state),
            draws: own_draws(pose.draws).into(),
            words: Vec::new(),
            phases: [Duration::ZERO; 2],
            indexed: Some(indexed),
            has_warnings: pose.has_warnings,
        };
        geometry.shrink(self.cache.cache.retained_bytes().saturating_add(draws_bytes).saturating_add(atlas_bytes));
        let geometry = Arc::new(geometry);
        self.cache.reservation = Some(Arc::clone(&geometry));
        if sample.revision != 0 && (sample.tick.is_multiple_of(120) || start.elapsed() > WARN_AGE) {
            report(&format!(
                "emote_pose_ready task={} generation={} tick={} worker={worker} queue_us={} evaluate_us={} geometry_us={} bytes={}",
                self.task, self.generation, sample.tick, start.duration_since(sample.scheduled).as_micros(),
                (evaluated - start).as_micros(), evaluated.elapsed().as_micros(), geometry.bytes,
            ));
        }
        WorkResult::Ready(ReadyPose {
            sample: Arc::clone(sample), pose: prepared, atlas: PoseAtlas::Pending, atlas_names,
            _geometry: geometry,
        })
    }
}

pub(in crate) enum ScenePose {
    Exact,
    Pending,
    Ready(ReadyPose),
    Reuse(EmoteRasterFrame),
}

struct Displayed {
    sample: Arc<Sample>,
    frame: EmoteRasterFrame,
    _pixels: Reservation,
}

struct Actor {
    generation: u64,
    context: ScenePoseContext,
    waiting_since: Instant,
    retry_at: Option<Instant>,
    exact_wait: Option<(u64, Instant)>,
    cancelled: Arc<AtomicBool>,
    running: Option<Arc<Sample>>,
    dispatched_at: Option<Instant>,
    pending: Option<Arc<Sample>>,
    ready: Option<ReadyPose>,
    displayed: Option<Displayed>,
    cache: ActorGeometry,
    unsupported: bool,
    warned_age: bool,
}

impl Actor {
    fn latest(&self) -> Option<&Sample> {
        self.pending.as_deref().or(self.running.as_deref())
            .or(self.ready.as_ref().map(|pose| pose.sample.as_ref()))
            .or(self.displayed.as_ref().map(|pose| pose.sample.as_ref()))
    }
}

impl Drop for Actor {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

pub(in crate) struct PoseScheduler {
    actors: BTreeMap<u32, Actor>,
    next_generation: u64,
    next_job: usize,
    sender: mpsc::Sender<Completion>,
    receiver: mpsc::Receiver<Completion>,
    gpu_has_room: bool,
    #[cfg(test)]
    gate: Option<(u32, Arc<PoseTestGate>)>,
}

impl Default for PoseScheduler {
    fn default() -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            actors: BTreeMap::new(), next_generation: 0, next_job: 0, sender, receiver, gpu_has_room: true,
            #[cfg(test)]
            gate: None,
        }
    }
}

impl PoseScheduler {
    #[cfg(test)]
    pub(in crate) fn set_gate(&mut self, task: u32, gate: Arc<PoseTestGate>) {
        self.gate = Some((task, gate));
    }

    #[cfg(test)]
    pub(in crate) fn displayed_tick(&self, task: u32) -> Option<u64> {
        Some(self.actors.get(&task)?.displayed.as_ref()?.sample.tick)
    }

    pub(in crate) fn retain(&mut self, tasks: &[u32]) {
        self.actors.retain(|task, _| tasks.contains(task));
        while let Ok(completion) = self.receiver.try_recv() {
            let Some(actor) = self.actors.get_mut(&completion.task) else { continue; };
            if actor.generation != completion.generation { continue; }
            actor.running = None;
            actor.dispatched_at = None;
            actor.cache = completion.cache;
            match completion.result {
                WorkResult::Ready(pose) => actor.ready = Some(pose),
                WorkResult::Congested(sample) => {
                    actor.pending.get_or_insert(sample);
                    actor.retry_at = Some(Instant::now() + CONGESTION_RETRY);
                }
                WorkResult::Unsupported => {
                    report(&format!("emote_pose_fallback task={} reason=unsupported_actor", completion.task));
                    actor.unsupported = true;
                    actor.pending = None;
                }
            }
        }
    }

    pub(in crate) fn cancel(&mut self, task: u32) {
        self.actors.remove(&task);
    }

    pub(in crate) fn request(
        &mut self, task: u32, tick: u64, revision: u64, context: ScenePoseContext, player: &CpuPlayer,
        viewport: CpuViewport, exact: bool,
    ) {
        let Ok(variables) = player.effective_variables() else {
            self.cancel(task);
            return;
        };
        let incompatible = self.actors.get(&task).is_some_and(|actor| {
            actor.context != context || actor.latest().is_some_and(|sample| {
                !sample.state.same_display_context(player, viewport, &variables)
            })
        });
        if incompatible {
            self.cancel(task);
        }
        let actor = self.actors.entry(task).or_insert_with(|| {
            self.next_generation = self.next_generation.wrapping_add(1);
            Actor {
                generation: self.next_generation, context, waiting_since: Instant::now(), retry_at: None,
                cancelled: Arc::new(AtomicBool::new(false)),
                exact_wait: None, running: None, dispatched_at: None, pending: None, ready: None, displayed: None,
                cache: ActorGeometry::default(),
                unsupported: false, warned_age: false,
            }
        });
        if actor.unsupported { return; }
        if actor.displayed.take_if(|pose| !pose.frame.is_valid()).is_some() {
            actor.waiting_since = Instant::now();
        }
        if exact {
            if actor.exact_wait.is_none_or(|(previous, _)| previous != tick) {
                actor.exact_wait = Some((tick, Instant::now()));
            }
            // The last composed canvas remains available while an exact
            // capture or feedback scene waits for the requested input.
            let stale = |sample: &Sample| !sample.state.matches(player, viewport, &variables);
            actor.ready.take_if(|pose| stale(&pose.sample));
            actor.displayed.take_if(|pose| stale(&pose.sample));
        } else {
            actor.exact_wait = None;
        }
        if actor.latest().is_some_and(|sample| sample.state.matches(player, viewport, &variables)) {
            return;
        }
        let bytes = player.estimate_draw_snapshot_bytes(&variables).max(size_of::<Sample>());
        if bytes > INPUT_BYTES {
            actor.unsupported = true;
            actor.cancelled.store(true, Ordering::Relaxed);
            actor.running = None;
            actor.dispatched_at = None;
            actor.pending = None;
            report(&format!("emote_pose_fallback task={task} reason=unsupported_actor input_bytes={bytes}"));
            return;
        }
        // Coalescing frees the previous input charge before requesting another.
        actor.pending = None;
        if let Some(input) = Reservation::acquire(&INPUT_USED, INPUT_BYTES, bytes) {
            let variables = Arc::new(variables);
            actor.pending = Some(Arc::new(Sample {
                tick, revision, scheduled: Instant::now(), player: player.snapshot_for_draw(),
                viewport, state: Arc::new(player.render_state_snapshot_shared(viewport, variables.clone())),
                variables, _input: input,
            }));
        }
    }

    pub(in crate) fn dispatch(&mut self, tasks: &[u32]) {
        self.gpu_has_room = tasks.is_empty() || RASTER.with_borrow(|slot| {
            slot.as_ref().is_some_and(|raster| raster.emote.as_ref().and_then(|emote| emote.mesh.as_ref())
                .is_some_and(|mesh| mesh.submission.can_start_batch(&raster.device)))
        });
        if self.gpu_has_room {
            self.stage_atlases(tasks);
        } else {
            crate::perf_detail::emote_fallback("queue_busy");
        }
        let Some(pool) = Pool::get() else {
            for actor in self.actors.values_mut() { actor.unsupported = true; }
            return;
        };
        let count = tasks.len().min(MAX_INDEXED_PREP_POSES);
        if count == 0 { return; }
        self.next_job %= count;
        for _ in 0..count {
            let next = self.next_job;
            self.next_job = (next + 1) % count;
            let task = tasks[next];
            let Some(actor) = self.actors.get_mut(&task) else { continue; };
            if actor.dispatched_at.is_some_and(|start| start.elapsed() >= POSE_TIMEOUT) {
                actor.cancelled.store(true, Ordering::Relaxed);
                self.next_generation = self.next_generation.wrapping_add(1);
                actor.generation = self.next_generation;
                actor.cancelled = Arc::new(AtomicBool::new(false));
                actor.retry_at = Some(Instant::now() + CONGESTION_RETRY);
                actor.dispatched_at = None;
                if let Some(expired) = actor.running.take() {
                    actor.pending.get_or_insert(expired);
                }
            }
            if actor.running.is_some() || actor.unsupported || actor.retry_at.is_some_and(|at| Instant::now() < at) {
                continue;
            }
            let Some(sample) = actor.pending.take() else { continue; };
            let work = Work {
                task, generation: actor.generation, sample: Arc::clone(&sample),
                cancelled: Arc::clone(&actor.cancelled), cache: std::mem::take(&mut actor.cache),
                done: self.sender.clone(),
                #[cfg(test)]
                gate: self.gate.as_ref().filter(|(id, _)| *id == task).map(|(_, gate)| gate.clone()),
            };
            match pool.sender.try_send(work) {
                Ok(()) => {
                    actor.running = Some(sample);
                    actor.dispatched_at = Some(Instant::now());
                }
                Err(mpsc::TrySendError::Full(work)) => {
                    actor.cache = work.cache;
                    actor.pending = Some(sample);
                    self.next_job = next;
                    break;
                }
                Err(mpsc::TrySendError::Disconnected(_)) => {
                    actor.unsupported = true;
                }
            }
        }
    }

    fn stage_atlases(&mut self, tasks: &[u32]) {
        let ready: Vec<_> = tasks.iter().filter_map(|task| {
            let actor = self.actors.get(task)?;
            let pose = actor.ready.as_ref()?;
            Some((*task, pose.sample.player.shared_motion_model(), &pose.atlas_names))
        }).collect();
        let evaluating: Vec<_> = tasks.iter().filter_map(|task| {
            let actor = self.actors.get(task)?;
            if actor.ready.is_some() || actor.unsupported || actor.displayed.is_some() { return None; }
            let model = actor.running.as_ref().or(actor.pending.as_ref())?.player.shared_motion_model();
            // The only texture in the model can be warmed while geometry runs.
            (model.textures.len() == 1).then(|| (model, model.textures.keys().next().unwrap()))
        }).collect();
        if ready.is_empty() && evaluating.is_empty() { return; }

        let statuses = RASTER.with_borrow_mut(|slot| {
            let Some(raster) = slot.as_mut() else { return Vec::new() };
            let Some(emote) = raster.emote.as_mut() else { return Vec::new() };
            emote.poll_atlas_upload();
            let start = Instant::now();
            #[cfg(windows)]
            let background = super::mesh::atlas_upload_enabled();
            #[cfg(not(windows))]
            let background = false;
            emote.atlas_upload_window.refresh(start);
            let available = emote.atlas_transfers.allowance(&raster.device,
                emote.atlas_upload_window.scene_remaining, Duration::ZERO);
            let mut remaining = available;
            // Protect every dependency already allocated before admitting any
            // new texture, including dependencies of later scene actors.
            let needed: HashSet<_> = ready.iter()
                .flat_map(|(_, model, names)| names.iter().map(move |name| &model.textures[name]))
                .chain(evaluating.iter().map(|(model, name)| &model.textures[*name]))
                .map(|texture| std::ptr::from_ref(texture) as usize).collect();
            emote.begin_atlas_batch();
            for atlas in &mut emote.atlases {
                atlas.used_by_draw = atlas.owner.strong_count() != 0 && needed.contains(&atlas.identity);
            }

            let statuses = ready.into_iter().map(|(task, model, names)| {
                let mut status = PoseAtlas::Resident;
                for name in names {
                    if start.elapsed() >= Duration::from_millis(2) {
                        if emote.scene_atlas_tail(model, &model.textures[name]) != 0 { status = PoseAtlas::Pending; }
                        continue;
                    }
                    match emote.stage_scene_atlas(&raster.device, &raster.queue, model, name, &mut remaining, background) {
                        Ok(Some(false)) => status = PoseAtlas::Pending,
                        Ok(Some(true)) => {}
                        // Unadmitted resources use the existing CPU fallback
                        // under the residency guard.
                        Ok(None) | Err(_) => return (task, PoseAtlas::Resident),
                    }
                }
                (task, status)
            }).collect();
            for (model, name) in evaluating {
                if remaining == 0 || start.elapsed() >= Duration::from_millis(2) { break; }
                let _ = emote.stage_scene_atlas(&raster.device, &raster.queue, model, name, &mut remaining, background);
            }
            let bytes = available - remaining;
            if bytes != 0 {
                emote.atlas_upload_window.scene_remaining -= bytes;
                if background {
                    report(&format!("emote_atlas_scene_enqueued_bytes={bytes} stage_us={}", start.elapsed().as_micros()));
                } else {
                    let submitted = super::super::submit_gpu_commands(&raster.queue, []);
                    emote.atlas_transfers.submitted(submitted, bytes);
                    report(&format!("emote_atlas_scene_stage_bytes={bytes} submit_us={}", start.elapsed().as_micros()));
                }
            }
            statuses
        });
        for (task, status) in statuses {
            if let Some(pose) = self.actors.get_mut(&task).and_then(|actor| actor.ready.as_mut()) {
                pose.atlas = status;
            }
        }
    }

    pub(in crate) fn scene_ready(&self, task: u32) -> bool {
        let Some(actor) = self.actors.get(&task) else { return true; };
        if actor.unsupported || actor.displayed.as_ref().is_some_and(|pose| pose.frame.is_valid()) {
            return true;
        }
        if self.gpu_has_room && actor.ready.as_ref().is_some_and(|pose| pose.atlas != PoseAtlas::Pending) {
            return true;
        }
        actor.exact_wait.map_or(actor.waiting_since, |(_, start)| start).elapsed() >= POSE_TIMEOUT
    }

    pub(in crate) fn take(&mut self, task: u32, tick: u64) -> ScenePose {
        let Some(actor) = self.actors.get_mut(&task) else { return ScenePose::Exact; };
        if actor.unsupported {
            crate::perf_detail::emote_fallback("unsupported_actor");
            return ScenePose::Exact;
        }
        if self.gpu_has_room && actor.ready.as_ref().is_some_and(|pose| pose.atlas != PoseAtlas::Pending) {
            return ScenePose::Ready(actor.ready.take().unwrap());
        }
        if let Some(displayed) = actor.displayed.as_ref().filter(|pose| pose.frame.is_valid()) {
            let age = displayed.sample.scheduled.elapsed();
            let lag = tick.saturating_sub(displayed.sample.tick);
            let changing = actor.running.is_some() || actor.pending.is_some() || actor.ready.is_some();
            crate::perf_detail::emote_scene_pose(false, changing, lag, age);
            // Static input is permitted to stay resident indefinitely.
            if (lag > 2 || age > WARN_AGE) && actor.running.is_some() && !actor.warned_age
                && crate::perf_detail::enabled()
            {
                actor.warned_age = true;
                report(&format!("emote_pose_age task={task} generation={} requested_tick={tick} displayed_tick={} age_us={}",
                    actor.generation, displayed.sample.tick, age.as_micros()));
            }
            return ScenePose::Reuse(displayed.frame.clone());
        }
        if actor.exact_wait.map_or(actor.waiting_since, |(_, start)| start).elapsed() >= POSE_TIMEOUT {
            let step = if actor.ready.is_some() { "atlas" } else if actor.exact_wait.is_some() { "exact_pose" } else { "first_pose" };
            report(&format!("emote_pose_timeout task={task} generation={} step={step}", actor.generation));
            let now = Instant::now();
            actor.waiting_since = now;
            if let Some((_, start)) = &mut actor.exact_wait { *start = now; }
            return ScenePose::Exact;
        }
        ScenePose::Pending
    }

    pub(in crate) fn record_render(&mut self, task: u32, tick: u64, pose: ReadyPose, frame: &EmoteRasterFrame) {
        let Some(actor) = self.actors.get_mut(&task) else { return; };
        if crate::perf_detail::enabled() {
            crate::perf_detail::emote_scene_pose(true, true, tick.saturating_sub(pose.sample.tick), pose.sample.scheduled.elapsed());
        }
        let bytes = frame.width() as usize * frame.height() as usize * 4;
        if let Some(displayed) = &mut actor.displayed {
            if !displayed._pixels.resize(DISPLAY_BYTES, bytes) { return; }
            displayed.sample = pose.sample;
            displayed.frame = frame.clone();
        } else if let Some(pixels) = Reservation::acquire(&DISPLAY_USED, DISPLAY_BYTES, bytes) {
            actor.displayed = Some(Displayed { sample: pose.sample, frame: frame.clone(), _pixels: pixels });
        } else {
            return;
        }
        actor.waiting_since = Instant::now();
        actor.warned_age = false;
        let sample = &actor.displayed.as_ref().unwrap().sample;
        if crate::perf_detail::enabled() && sample.tick.is_multiple_of(120) {
            report(&format!("emote_pose_consumed task={task} generation={} tick={} age_us={}",
                actor.generation, sample.tick, sample.scheduled.elapsed().as_micros()));
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate) struct ScenePoseContext {
    pub(in crate) imports: u32,
    pub(in crate) base_color: u32,
    pub(in crate) color_percentages: Option<[u32; 3]>,
    pub(in crate) blend: cs2_renderer::BlendMode,
}

pub(in crate) fn pose_jobs_available() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    let enabled = *ENABLED.get_or_init(|| {
        #[cfg(windows)]
        return std::env::var_os("CS2_EMOTE_POSE_JOBS").is_none_or(|value| value != "0");
        #[cfg(target_os = "android")]
        return !super::super::android_property(c"debug.cs2.emote_pose_jobs", |value| value == b"0").unwrap_or(false);
    });
    enabled && mesh::enabled() && RASTER.with_borrow(|state| state.as_ref().is_some_and(|raster| {
        raster.direct_output_enabled() && !raster.failed.load(Ordering::Relaxed)
            && raster.emote.as_ref().is_some_and(|emote| emote.mesh.is_some())
    }))
}

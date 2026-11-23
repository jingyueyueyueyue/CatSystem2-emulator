//! Native710 demand preparation. Jobs own immutable resource snapshots only;
//! the original invocation alone publishes handles, cache entries and layers.
use super::*;
use cs2_runtime::{ResourceIndex, ResourceLocation, emote::EmoteArchiveVersion, resource_router::ResourceMode};
use std::{path::PathBuf, sync::{Mutex, Weak, mpsc, atomic::{AtomicUsize, Ordering}}, time::{Duration, Instant}};

const MAX_PENDING: usize = 16;
const MAX_SOURCE: usize = 128 * 1024 * 1024;
const MAX_DECODED: usize = 256 * 1024 * 1024;
const MAX_JOB: usize = MAX_SOURCE + MAX_DECODED;
const MAX_POOL: usize = MAX_JOB * 2;
const TIMEOUT: Duration = Duration::from_secs(30);
static USED: AtomicUsize = AtomicUsize::new(0);

struct Charge(usize);
impl Charge {
    fn reserve() -> Option<Self> {
        USED.fetch_update(Ordering::AcqRel, Ordering::Acquire,
            |used| used.checked_add(MAX_JOB).filter(|&next| next <= MAX_POOL)).ok().map(|_| Self(MAX_JOB))
    }
    fn shrink(&mut self, bytes: usize) { USED.fetch_sub(self.0 - bytes, Ordering::AcqRel); self.0 = bytes; }
}
impl Drop for Charge { fn drop(&mut self) { USED.fetch_sub(self.0, Ordering::AcqRel); } }

struct Asset {
    name: String,
    source: Option<(ResourceCandidate, EmoteArchiveVersion)>,
    loaded: bool,
    bytes: Option<Arc<Vec<u8>>>,
    frames: BTreeMap<u32, Result<Option<RgbaFrame>>>,
}
pub(super) struct Ready {
    pub(super) name: Option<(String, u32)>,
    assets: Vec<Asset>,
    pub(super) aliases: Option<cs2_formats::animation_alias::AnimationAliases>,
    pub(super) configuration: Option<(bool, usize)>,
    setup: Option<(ResourceCandidate, Option<EmoteArchiveVersion>)>,
    pub(super) read_us: u128,
    pub(super) decode_us: u128,
    pub(super) wall_us: u128,
    pub(super) source_bytes: usize,
    pub(super) decoded_bytes: usize,
    pub(super) warm_hits: usize,
    _charge: Charge,
}
impl Ready {
    pub(super) fn asset(&self, name: &str, raw: bool) -> Option<Option<(String, Arc<Vec<u8>>)>> {
        self.assets.iter().find(|asset| asset.name.eq_ignore_ascii_case(name)).map(|asset|
            asset.bytes.as_ref().filter(|bytes| raw || !bytes.is_empty()).map(|bytes|
                (asset.source.as_ref().map_or_else(|| name.to_owned(), |(source, _)| source.name.clone()), bytes.clone())))
    }
    pub(super) fn exists(&self, name: &str) -> Option<bool> {
        self.assets.iter().find(|asset| asset.name.eq_ignore_ascii_case(name)).map(|asset| asset.source.is_some())
    }
    pub(super) fn frame(&self, bytes: &[u8], id: u32) -> Option<Result<Option<RgbaFrame>>> {
        self.assets.iter().find(|asset| asset.bytes.as_ref().is_some_and(|source| source.as_slice() == bytes))?
            .frames.get(&id).cloned()
    }
    fn validate(&self, resources: &ResourceIndex) -> Result<()> {
        for asset in &self.assets {
            let current = resources.resolve_mode_candidate(&asset.name, ResourceMode::LooseFirst)
                .map_err(|_| native_state_error("image material source routing changed"))?;
            let current = current.and_then(|source| EmoteArchiveVersion::observe(source).map(|stamp| (source.clone(), stamp)));
            if current != asset.source { return Err(native_state_error("image material source changed during preparation")); }
        }
        if let Some((source, version)) = &self.setup
            && EmoteArchiveVersion::observe(source) != *version {
            return Err(native_state_error("image material setup.xml changed during preparation"));
        }
        Ok(())
    }
}

struct Request {
    name: String,
    save_root: PathBuf,
    aliases: Option<cs2_formats::animation_alias::AnimationAliases>,
    initialized: bool,
    cached: std::collections::BTreeSet<String>,
    warm: Vec<(Arc<[u8]>, RgbaFrame)>,
}
type Slot = Mutex<Option<Result<Ready>>>;
struct Pending { started: Instant, request: Option<Request>, slot: Arc<Slot> }
struct Job { resources: Arc<ResourceIndex>, request: Request, slot: Weak<Slot>, charge: Charge,
    #[cfg(test)] gate: Option<Arc<Mutex<mpsc::Receiver<()>>>>,
}
#[derive(Default)]
pub(super) struct Loads {
    pending: BTreeMap<u32, Pending>,
    worker: Option<mpsc::SyncSender<Job>>,
    #[cfg(test)] pub(super) gate: Option<Arc<Mutex<mpsc::Receiver<()>>>>,
}
impl Loads {
    #[cfg(test)]
    pub(super) fn result_ready(&self, owner: u32) -> bool {
        self.pending.get(&owner).is_some_and(|pending| pending.slot.try_lock().ok().is_some_and(|slot| slot.is_some()))
    }
    #[cfg(test)]
    pub(super) fn expire(&mut self, owner: u32) { self.pending.get_mut(&owner).unwrap().started = Instant::now() - TIMEOUT; }
    pub(super) fn owners(&self) -> impl Iterator<Item = u32> + '_ { self.pending.keys().copied() }
    pub(super) fn has_pending(&self) -> bool { !self.pending.is_empty() }
    pub(super) fn cancel(&mut self, owner: u32) { self.pending.remove(&owner); }
    pub(super) fn saved(&self, owner: u32) -> bool { self.pending.contains_key(&owner) }
    pub(super) fn begin(&mut self, owner: u32, name: String, save_root: PathBuf,
        aliases: Option<cs2_formats::animation_alias::AnimationAliases>, initialized: bool,
        cached: std::collections::BTreeSet<String>, warm: Vec<(Arc<[u8]>, RgbaFrame)>) -> Result<()> {
        if self.pending.len() >= MAX_PENDING { return Err(native_state_error("too many pending image materials")); }
        self.pending.insert(owner, Pending { started: Instant::now(), slot: Arc::new(Mutex::new(None)),
            request: Some(Request { name, save_root, aliases, initialized, cached, warm }) });
        Ok(())
    }
    pub(super) fn poll(&mut self, owner: u32, resources: Arc<ResourceIndex>) -> Result<Option<Ready>> {
        let pending = self.pending.get_mut(&owner).expect("retained image invocation");
        let result = pending.slot.try_lock().ok().and_then(|mut slot| slot.take());
        if let Some(result) = result {
            self.pending.remove(&owner);
            let ready = result?;
            ready.validate(&resources)?;
            return Ok(Some(ready));
        }
        if pending.started.elapsed() >= TIMEOUT {
            self.pending.remove(&owner);
            return Err(native_state_error("image material preparation timed out"));
        }
        if pending.request.is_none() { return Ok(None); }
        let Some(charge) = Charge::reserve() else { return Ok(None); };
        if self.worker.is_none() {
            match start_worker() {
                Ok(worker) => self.worker = Some(worker),
                Err(cause) => { self.pending.remove(&owner); return Err(cause); }
            }
        }
        let job = Job { resources, request: pending.request.take().unwrap(), slot: Arc::downgrade(&pending.slot), charge,
            #[cfg(test)] gate: self.gate.clone(),
        };
        match self.worker.as_ref().unwrap().try_send(job) {
            Ok(()) => {},
            Err(mpsc::TrySendError::Full(job)) => pending.request = Some(job.request),
            Err(mpsc::TrySendError::Disconnected(_)) => {
                self.worker = None; self.pending.remove(&owner);
                return Err(native_state_error("image material worker disconnected"));
            }
        }
        Ok(None)
    }
}

fn start_worker() -> Result<mpsc::SyncSender<Job>> {
    let (sender, receiver) = mpsc::sync_channel::<Job>(1);
    std::thread::Builder::new().name("cs2-image-material".into()).spawn(move || {
        while let Ok(job) = receiver.recv() {
            if job.slot.strong_count() == 0 { continue; }
            #[cfg(test)] if let Some(gate) = &job.gate { let _ = gate.lock().unwrap().recv(); }
            if job.slot.strong_count() == 0 { continue; }
            let result = prepare(&job.resources, job.request, job.charge, &job.slot);
            if let Some(slot) = job.slot.upgrade() && let Ok(mut slot) = slot.lock() { *slot = Some(result); }
        }
    }).map_err(|cause| native_state_error_owned(format!("image material worker spawn failed: {cause}")))?;
    Ok(sender)
}

struct Builder<'a> { resources: &'a ResourceIndex, ready: Ready, owner: &'a Weak<Slot>, warm: Vec<(Arc<[u8]>, RgbaFrame)> }
impl Builder<'_> {
    // Existence queries must not open the companion payload: the original
    // returns a missing image before trying to read or parse its ANM file.
    fn probe(&mut self, name: &str) -> Result<bool> {
        if let Some(asset) = self.ready.assets.iter().find(|asset| asset.name.eq_ignore_ascii_case(name)) { return Ok(asset.source.is_some()); }
        let candidate = self.resources.resolve_mode_candidate(name, ResourceMode::LooseFirst)
            .map_err(|_| native_state_error("image animation resource routing failed"))?;
        let source = candidate.map(|candidate| EmoteArchiveVersion::observe(candidate)
            .map(|stamp| (candidate.clone(), stamp)).ok_or_else(|| native_state_error("image material source version unavailable"))).transpose()?;
        let exists = source.is_some();
        self.ready.assets.push(Asset { name: name.to_owned(), source, loaded: false, bytes: None, frames: BTreeMap::new() });
        Ok(exists)
    }
    fn read(&mut self, name: &str, cst: bool) -> Result<Option<Arc<Vec<u8>>>> {
        if let Some(asset) = self.ready.assets.iter().find(|asset| asset.loaded && asset.name.eq_ignore_ascii_case(name)) { return Ok(asset.bytes.clone()); }
        if self.owner.strong_count() == 0 { return Err(native_state_error("image material cancelled")); }
        if name.encode_utf16().count() >= 260 { return Err(native_state_error("image manager name exceeds 259 UTF-16 units")); }
        let started = Instant::now();
        let candidate = self.resources.resolve_mode_candidate(name, ResourceMode::LooseFirst)
            .map_err(|_| native_state_error(if cst { "CST resource routing failed" } else { "image resource routing failed" }))?.cloned();
        let mut asset = Asset { name: name.to_owned(), source: None, loaded: true, bytes: None, frames: BTreeMap::new() };
        if let Some(candidate) = candidate {
            if candidate.size > MAX_FILE as u64 { return Err(native_state_error(if cst { "CST resource exceeds 64 MiB" } else { "image manager resource exceeds 64 MiB" })); }
            let version = EmoteArchiveVersion::observe(&candidate).ok_or_else(|| native_state_error("image material source version unavailable"))?;
            asset.source = Some((candidate.clone(), version));
            let remaining = MAX_SOURCE.saturating_sub(self.ready.source_bytes).min(MAX_FILE);
            let bytes = match self.resources.read_candidate_bounded(&candidate, remaining) {
                Ok(Some(bytes)) => Some(bytes),
                Ok(None) => return Err(native_state_error("image material source bytes exceed bounded preparation pool")),
                Err(RuntimeError::ResourceNotFound(_)) if !cst => None,
                Err(RuntimeError::Io { source, .. }) if !cst && (source.kind() == std::io::ErrorKind::NotFound || source.kind() == std::io::ErrorKind::PermissionDenied) => None,
                Err(_) => return Err(native_state_error(if cst { "CST resource read failed" } else { "image manager resource read failed" })),
            };
            if let Some(bytes) = bytes { self.ready.source_bytes += bytes.len(); asset.bytes = Some(Arc::new(bytes)); }
        }
        self.ready.read_us += started.elapsed().as_micros();
        let bytes = asset.bytes.clone();
        if let Some(previous) = self.ready.assets.iter_mut().find(|previous| previous.name.eq_ignore_ascii_case(name)) { *previous = asset; }
        else { self.ready.assets.push(asset); }
        Ok(bytes)
    }
    fn aliases(&mut self) -> Result<&cs2_formats::animation_alias::AnimationAliases> {
        if self.ready.aliases.is_none() {
            let names = self.resources.category_names(b"image", "anmtable.csv").map_err(|_| native_state_error("CST resource category resolution failed"))?;
            let mut aliases = Default::default();
            for name in names {
                if let Some(bytes) = self.read(&name, true)? {
                    aliases = cs2_formats::animation_alias::AnimationAliases::parse(&bytes)
                        .map_err(|_| native_state_error("image manager anmtable.csv is malformed or oversized"))?; break;
                }
            }
            self.ready.aliases = Some(aliases);
        }
        Ok(self.ready.aliases.as_ref().unwrap())
    }
    fn animation(&mut self, name: &str) -> Result<Option<String>> {
        let anm = with_extension(name, Some("anm"));
        if self.probe(&anm)? { return Ok(Some(anm)); }
        let start = name.rfind(['/', '\\']).filter(|&at| at > 0).map_or(0, |at| at + 1);
        let key = with_extension(&name[start..], None);
        let (key, _, invalid) = encoding_rs::SHIFT_JIS.encode(&key);
        if invalid || key.len() >= 256 { return Err(native_state_error("image animation alias key exceeds CP932 capacity")); }
        let alias = self.aliases()?.resolve(&key).map(<[u8]>::to_vec);
        let Some(alias) = alias else { return Ok(None); };
        let alias = crate::ScriptTextEncoding::Cp932.decode(&alias);
        let names = self.resources.category_names(b"image", &alias).map_err(|_| native_state_error("image animation alias category resolution failed"))?;
        for name in names { let anm = with_extension(&name, Some("anm")); if self.probe(&anm)? { return Ok(Some(anm)); } }
        Ok(None)
    }
    fn decode(&mut self, name: &str, id: u32, animated: bool) -> Result<bool> {
        let Some(bytes) = self.read(name, false)? else { return Ok(false); };
        if bytes.is_empty() { return Ok(false); }
        let mut any_frame = false;
        let mut all_frames_valid = true;
        let ids = if animated && bytes.starts_with(b"HG-3") {
            HgFile::parse(&bytes).map(|file| file.frames.iter().map(|frame| frame.id).collect::<std::collections::BTreeSet<_>>().into_iter().collect())
                .unwrap_or_else(|_| vec![id])
        } else { vec![id] };
        for id in ids {
            if self.owner.strong_count() == 0 { return Err(native_state_error("image material cancelled")); }
            let started = Instant::now();
            let warm = (id == 0).then(|| self.warm.iter().find(|(source, _)| source.as_ref() == bytes.as_slice()).map(|(_, frame)| frame.clone())).flatten();
            let result = if let Some(frame) = warm { self.ready.warm_hits += 1; Ok(Some(frame)) }
                else { check_decode_budget(&bytes, id, MAX_DECODED.saturating_sub(self.ready.decoded_bytes))?; decode_image_frame(&bytes, id) };
            all_frames_valid &= result.is_ok();
            if let Ok(Some(frame)) = &result {
                any_frame = true;
                self.ready.decoded_bytes = self.ready.decoded_bytes.checked_add(frame.pixels().len()).filter(|&bytes| bytes <= MAX_DECODED)
                    .ok_or_else(|| native_state_error("image material decoded frames exceed 256 MiB"))?;
            }
            self.ready.decode_us += started.elapsed().as_micros();
            self.ready.assets.iter_mut().find(|asset| asset.name.eq_ignore_ascii_case(name)).unwrap().frames.insert(id, result);
        }
        Ok(any_frame && all_frames_valid)
    }
    fn configuration(&mut self, save_root: &std::path::Path) -> Result<()> {
        self.aliases()?;
        let mut megabytes = self.resources.startup_configuration().map_err(|_| native_state_error("image cache startup XML read/parse failed"))?
            .map_or(32, |startup| startup.integer(&[b"document", b"APP", b"imgcache"], 32) as u32);
        let path = save_root.join("setup.xml");
        let source = ResourceCandidate { name: "setup.xml".into(), location: ResourceLocation::File { path: path.clone() }, size: 0, priority: 0 };
        self.ready.setup = Some((source.clone(), EmoteArchiveVersion::observe(&source)));
        match std::fs::File::open(path) {
            Ok(file) => {
                use std::io::Read;
                let mut bytes = Vec::new(); file.take((MAX_FILE + 1) as u64).read_to_end(&mut bytes).map_err(|_| native_state_error("image cache private setup.xml read failed"))?;
                if bytes.len() > MAX_FILE { return Err(native_state_error("image cache private setup.xml exceeds 64 MiB")); }
                let user = ConfigXml::parse_runtime(&bytes).map_err(|_| native_state_error("image cache private setup.xml is malformed"))?;
                megabytes = user.integer(&[b"document", b"ETC", b"imgcache"], megabytes as i32) as u32;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
            Err(_) => return Err(native_state_error("image cache private setup.xml read failed")),
        }
        let capacity = megabytes.wrapping_shl(20) as usize;
        if capacity > MAX_CACHE { return Err(native_state_error("configured image cache exceeds portable 256 MiB limit")); }
        self.ready.configuration = Some((megabytes != 0, capacity)); Ok(())
    }
}

fn prepare(resources: &ResourceIndex, request: Request, charge: Charge, owner: &Weak<Slot>) -> Result<Ready> {
    let started = Instant::now();
    let names = resources.category_names(b"image", &request.name).map_err(|_| native_state_error("native 710 image category resolution failed"))?;
    let mut builder = Builder { resources, owner, warm: request.warm, ready: Ready { name: None, assets: vec![], aliases: request.aliases,
        configuration: None, setup: None, read_us: 0, decode_us: 0, wall_us: 0, source_bytes: 0, decoded_bytes: 0, warm_hits: 0, _charge: charge } };
    if let Some(name) = names.first() {
        let (encoded, _, invalid) = encoding_rs::SHIFT_JIS.encode(name);
        if invalid { return Err(native_state_error("native 710 completed name is not CP932")); }
        let (name, id) = crate::image_pixels::image_reference(&encoded)?;
        let name = crate::sound_native::decode_name(crate::ScriptTextEncoding::Cp932, &name)?;
        let (parts, extension) = image_plan_components(&name);
        if parts.len() > 65 { return Err(native_state_error("native 710 exceeds 64 layer selectors")); }
        let mut physical = Vec::new();
        if parts.len() <= 1 { physical.push((name.clone(), id)); }
        else {
            for (index, selector) in parts[1..].iter().enumerate() {
                let (encoded, _, invalid) = encoding_rs::SHIFT_JIS.encode(selector);
                let first = encoded[0];
                if invalid || !first.is_ascii() { return Err(native_state_error("native 710 layer selector is not an ASCII first byte")); }
                if first != b'0' { physical.push((format!("{}_{}{}.{}", parts[0], "0".repeat(index), char::from(first), extension), 0)); }
            }
        }
        for (physical, id) in physical {
            let animation = builder.animation(&physical)?;
            let animated = animation.is_some();
            if !animated && !request.initialized && builder.ready.configuration.is_none() { builder.configuration(&request.save_root)?; }
            if animated || !request.cached.contains(&ImageManager::key(&physical, id)) {
                let has_frame = builder.decode(&physical, id, animated)?;
                if has_frame && let Some(animation) = animation { builder.read(&animation, false)?; }
            }
        }
        builder.ready.name = Some((name, id));
    }
    builder.ready.validate(resources)?;
    builder.ready.wall_us = started.elapsed().as_micros();
    builder.ready._charge.shrink(builder.ready.source_bytes + builder.ready.decoded_bytes);
    Ok(builder.ready)
}

fn check_decode_budget(bytes: &[u8], id: u32, remaining: usize) -> Result<()> {
    if bytes.starts_with(b"HG-") {
        if let Ok(file) = HgFile::parse(bytes) {
            let selected = file.frames.iter().find(|frame| frame.id == id).or_else(|| file.frames.first());
            if let Some(frame) = selected {
                let size = u64::from(frame.width).checked_mul(u64::from(frame.height)).and_then(|size| size.checked_mul(4)).unwrap_or(u64::MAX);
                let inflated = match &frame.encoded_image {
                    Some(image) => u64::from(image.alpha_decompressed_length.unwrap_or(0)),
                    None => cs2_formats::hg::native_image_slices(frame, bytes).ok().map(|slices| slices.iter().map(|slice|
                        u64::from(slice.image.decompressed_data_length) + u64::from(slice.image.decompressed_command_length)).sum()).unwrap_or(0),
                };
                if size > remaining as u64 || inflated > MAX_DECODED as u64 { return Err(native_state_error("image material decode exceeds bounded preparation pool")); }
            }
        }
    } else if remaining < MAX_PIXELS {
        let pixel_bytes = |width: u32, height: u32| u64::from(width).checked_mul(u64::from(height)).and_then(|size| size.checked_mul(4)).unwrap_or(u64::MAX);
        let size = if bytes.starts_with(b"BM") { bytes.get(18..26).map(|v| pixel_bytes(u32::from_le_bytes(v[..4].try_into().unwrap()), u32::from_le_bytes(v[4..].try_into().unwrap()))) }
            else { cs2_formats::jpeg::JpegMetadata::parse(bytes).ok().map(|m| pixel_bytes(m.width, m.height)) };
        if size.is_some_and(|size| size > remaining as u64) { return Err(native_state_error("image material decode exceeds bounded preparation pool")); }
    }
    Ok(())
}

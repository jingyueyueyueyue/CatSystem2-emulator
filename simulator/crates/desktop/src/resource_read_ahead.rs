//! Bounded cancellable background resource reads. Dropping a window revokes
//! queued jobs/results; an already-started read may finish within its byte cap.
use super::*;
use cs2_runtime::ResourceLocation;
use std::time::SystemTime;

const MAX_AHEAD: usize = 20;
const MAX_RESOURCE: usize = 32 * 1024 * 1024;
const MAX_AHEAD_BYTES: usize = 128 * 1024 * 1024;
const MAX_CONFIG: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResourceStamp {
    location: ResourceLocation,
    len: u64,
    modified: SystemTime,
    created: Option<SystemTime>,
    #[cfg(unix)]
    inode: (u64,u64,i64,i64),
}
impl ResourceStamp {
    pub(super) fn observe(candidate: &ResourceCandidate) -> Option<Self> {
        let path = match &candidate.location { ResourceLocation::File { path }
            | ResourceLocation::Archive { path, .. } => path };
        let metadata = std::fs::metadata(path).ok()?;
        if !metadata.is_file() { return None; }
        if let ResourceLocation::Archive { offset, size, .. } = &candidate.location
            && offset.checked_add(*size)? > metadata.len() { return None; }
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        Some(Self { location: candidate.location.clone(), len: metadata.len(),
            modified: metadata.modified().ok()?, created: metadata.created().ok(),
            #[cfg(unix)]
            inode: (metadata.dev(),metadata.ino(),metadata.ctime(),metadata.ctime_nsec()),
        })
    }
    fn payload_size(&self) -> u64 {
        match &self.location { ResourceLocation::File { .. } => self.len,
            ResourceLocation::Archive { size, .. } => *size }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AheadRequest { Named(String), M2Spec(Vec<u8>) }
impl AheadRequest {
    fn matches(&self, other: &Self) -> bool {
        match (self,other) { (Self::Named(a),Self::Named(b)) => a.eq_ignore_ascii_case(b),
            _ => self == other }
    }
}
#[derive(Debug)]
enum ReadResult { Pending, Done(Option<PreparedResource>) }
#[derive(Debug, Clone)]
struct Entry {
    request: AheadRequest,
    stamp: Option<ResourceStamp>,
    reserved: usize,
    result: Arc<Mutex<ReadResult>>,
    ready: Option<PreparedResource>,
    submitted: bool,
    finished: bool,
    warmed: bool,
}
#[derive(Debug)]
struct Job {
    resources: Arc<ResourceIndex>,
    request: AheadRequest,
    result: Weak<Mutex<ReadResult>>,
    limit: usize,
}
#[derive(Debug, Clone, Default)]
pub(super) struct ReadAhead {
    index: Option<Arc<ResourceIndex>>,
    worker: Option<mpsc::SyncSender<Job>>,
    entries: Vec<Entry>,
    diagnostics: Vec<&'static str>,
    notes: Vec<&'static str>,
}
impl ReadAhead {
    #[cfg(test)]
    pub(super) fn update(&mut self, resources: &ResourceIndex, names: &[String]) {
        self.update_requests(resources, &names.iter().cloned().map(AheadRequest::Named).collect::<Vec<_>>());
    }
    pub(super) fn update_requests(&mut self, resources: &ResourceIndex, requests: &[AheadRequest]) {
        let mut keep = Vec::new();
        for request in requests.iter().take(MAX_AHEAD) {
            if keep.iter().any(|other: &AheadRequest| other.matches(request)) { continue; }
            keep.push(request.clone());
        }
        let previous_count = self.entries.len();
        self.entries.retain(|entry| keep.iter().any(|request| entry.request.matches(request)));
        if self.entries.len() < previous_count { self.note("window-cancelled"); }
        let mut reserved = self.entries.iter().map(|entry| entry.reserved).sum::<usize>();
        for request in keep {
            let stamp = match &request {
                AheadRequest::Named(name) => resources.resolve_mode_candidate(name, ResourceMode::LooseFirst)
                    .ok().flatten().and_then(ResourceStamp::observe),
                AheadRequest::M2Spec(_) => None,
            };
            if let Some(index) = self.entries.iter().position(|entry| entry.request.matches(&request)) {
                let old = &self.entries[index];
                let changed_spec = matches!(request, AheadRequest::M2Spec(_))
                    && old.ready.as_ref().is_some_and(|resource| !resource.is_current());
                if old.stamp == stamp && !changed_spec { continue; }
                reserved -= self.entries.remove(index).reserved;
            }
            let bytes = match &request {
                AheadRequest::Named(_) => match stamp.as_ref().and_then(|s| usize::try_from(s.payload_size()).ok()) {
                    Some(size) => size, None => continue,
                },
                AheadRequest::M2Spec(_) => MAX_RESOURCE,
            };
            if bytes > MAX_RESOURCE || reserved.saturating_add(bytes) > MAX_AHEAD_BYTES {
                self.note("byte-budget"); continue;
            }
            reserved += bytes;
            self.entries.push(Entry { request, stamp, reserved: bytes,
                result: Arc::new(Mutex::new(ReadResult::Pending)), ready: None, submitted: false, finished: false, warmed: false });
        }
        if self.entries.is_empty() { return; }
        if self.index.is_none() { self.index = Some(Arc::new(resources.clone())); }
        if self.worker.is_none() { self.worker = start_worker(); }
        let mut ready = 0;
        let mut failed = 0;
        for entry in &mut self.entries {
            if !entry.finished {
                if let Ok(mut slot) = entry.result.try_lock()
                    && let ReadResult::Done(result) = &mut *slot {
                    entry.ready = result.take();
                    entry.finished = true;
                    ready += usize::from(entry.ready.is_some());
                    failed += usize::from(entry.ready.is_none());
                }
            }
            if entry.submitted { continue; }
            let Some(worker) = &self.worker else { break; };
            let job = Job { resources: self.index.as_ref().unwrap().clone(), request: entry.request.clone(),
                result: Arc::downgrade(&entry.result), limit: entry.reserved };
            match worker.try_send(job) {
                Ok(()) => entry.submitted = true,
                Err(mpsc::TrySendError::Full(_)) => break,
                Err(mpsc::TrySendError::Disconnected(_)) => { self.worker = None; break; }
            }
        }
        if ready != 0 { self.note("read-ready (images/audio/model consumers enabled)"); }
        if failed != 0 { self.note("read-miss/routing/budget"); }
    }
    fn note(&mut self, reason: &'static str) {
        if self.diagnostics.len() < 16 && !self.diagnostics.contains(&reason) {
            self.diagnostics.push(reason); self.notes.push(reason);
        }
    }
    pub(super) fn drain_notes(&mut self) -> Vec<&'static str> { std::mem::take(&mut self.notes) }
    pub(super) fn stats(&self) -> (usize, usize, usize) {
        (self.entries.len(),self.resources().count(),self.entries.iter().map(|entry|entry.reserved).sum())
    }
    pub(super) fn newly_ready(&mut self) -> impl Iterator<Item=&PreparedResource> {
        self.entries.iter_mut().filter_map(|entry| {
            if entry.warmed { return None; }
            let resource = entry.ready.as_ref()?;
            entry.warmed = true;
            Some(resource)
        })
    }
    pub(super) fn cached(&self, name: &str) -> Option<&PreparedResource> {
        self.resources().find(|resource| resource.name.eq_ignore_ascii_case(name) && resource.is_current())
    }
    pub(super) fn resources(&self) -> impl Iterator<Item=&PreparedResource> {
        self.entries.iter().filter_map(|entry| entry.ready.as_ref())
    }
    #[cfg(test)]
    pub(super) fn len(&self) -> usize { self.entries.len() }
}
fn start_worker() -> Option<mpsc::SyncSender<Job>> {
    let (sender, receiver) = mpsc::sync_channel::<Job>(MAX_AHEAD);
    std::thread::Builder::new().name("cs2-resource-ahead".into()).spawn(move || {
        while let Ok(job) = receiver.recv() {
            if job.result.strong_count() == 0 { continue; }
            let name = match &job.request {
                AheadRequest::Named(name) => Some(name.clone()),
                AheadRequest::M2Spec(spec) => resolve_m2_name(&job.resources, spec),
            };
            if job.result.strong_count() == 0 { continue; }
            let resource = name.and_then(|name| read_optional(&job.resources, &name, job.limit).ok()?.0);
            if job.result.strong_count() == 0 { continue; }
            if let Some(resource) = &resource {
                // Reuses the actual bounded prepared-audio cache; no channel,
                // clock, callback or playback operation is touched here.
                if resource.bytes.starts_with(b"OggS") || resource.bytes.starts_with(b"RIFF") {
                    let _ = crate::audio_decode::prepare_named_audio(resource.bytes.clone(), &resource.name);
                }
            }
            if let Some(result) = job.result.upgrade() && let Ok(mut slot) = result.lock() {
                *slot = ReadResult::Done(resource);
            }
        }
    }).ok()?;
    Some(sender)
}

/// Resolve only the immutable profile selection required to name the PSZ.
/// Controller construction, random seeds and difference files are demand work.
fn resolve_m2_name(resources: &ResourceIndex, spec: &[u8]) -> Option<String> {
    use cs2_runtime::emote::{CpuM2Config, CpuM2LoadSpec};
    let read = |category: &[u8], name: &[u8]| -> Option<Vec<u8>> {
        let name=String::from_utf8(crate::cp932_to_utf8(name)).ok()?;
        for name in resources.category_names(category,&name).ok()? {
            let Some(candidate)=resources.resolve_mode_candidate(&name,ResourceMode::LooseFirst).ok()? else { continue; };
            return resources.read_candidate_bounded(candidate,MAX_CONFIG).ok()?;
        }
        None
    };
    let single = |config: &CpuM2Config, section: &[u8]| -> Option<Vec<u8>> {
        let rows=config.records(section);
        (rows.len()==1 && rows[0].len()==1).then(||rows[0][0].clone())
    };
    let view=CpuM2Config::parse_cp932(&read(b"m2config",b"m2view.cnf")?).ok()?;
    let profile_category=single(&view,b"profilepath")?;
    let data_category=single(&view,b"datapath")?;
    let extension=single(&view,b"dataext")?;
    let spec=CpuM2LoadSpec::parse_unquoted(spec).ok()?;
    let filename=view.profile_file(&spec.profile_alias_cp932).ok()?;
    let profile=CpuM2Config::parse_cp932(&read(&profile_category,filename)?).ok()?;
    let selection=profile.select_profile(spec.chara_id,spec.motion_id).ok()?;
    let mut name=selection.resource_stem_cp932;
    name.push(b'.'); name.extend_from_slice(&extension);
    let name=String::from_utf8(crate::cp932_to_utf8(&name)).ok()?;
    let category=if data_category.eq_ignore_ascii_case(b"psz")
        && resources.optional_category_names(b"m2psb","probe").ok()?.is_some() { b"m2psb".as_slice() } else { &data_category };
    resources.category_names(category,&name).ok()?.into_iter().find(|name|
        resources.resolve_mode_candidate(name,ResourceMode::LooseFirst).ok().flatten().is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (std::path::PathBuf, ResourceIndex) {
        let stamp=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let root=std::env::temp_dir().join(format!("cs2-ahead-{stamp}"));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("pic.hg3"),b"prepared image").unwrap();
        std::fs::write(root.join("voice.ogg"),b"prepared audio").unwrap();
        let resources=ResourceIndex::scan(&root).unwrap();
        (root,resources)
    }
    #[test]
    fn read_ahead_deduplicates_cancels_and_exposes_real_cached_bytes() {
        let (root,resources)=fixture();
        let names=vec!["pic.hg3".to_owned(),"PIC.HG3".to_owned(),"voice.ogg".to_owned()];
        let mut cache=ReadAhead::default();
        cache.update(&resources,&names);
        assert_eq!(cache.len(),2);
        for _ in 0..200 {
            cache.update(&resources,&names);
            if cache.cached("voice.ogg").is_some() { break; }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert_eq!(cache.cached("pic.hg3").unwrap().bytes,b"prepared image");
        assert_eq!(cache.cached("voice.ogg").unwrap().bytes,b"prepared audio");
        cache.update(&resources,&[]);
        assert_eq!(cache.len(),0);
        assert!(cache.cached("voice.ogg").is_none());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn read_ahead_emits_one_warm_event_per_source_version() {
        let (root,resources) = fixture();
        let names = vec!["pic.hg3".to_owned()];
        let mut cache = ReadAhead::default();
        for _ in 0..200 {
            cache.update(&resources,&names);
            if cache.cached("pic.hg3").is_some() { break; }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert_eq!(cache.newly_ready().count(),1);
        for _ in 0..20 {
            cache.update(&resources,&names);
            assert_eq!(cache.newly_ready().count(),0,"a frame tick must not resubmit HG3 decode");
        }
        std::fs::write(root.join("pic.hg3"),b"new resource version").unwrap();
        for _ in 0..200 {
            cache.update(&resources,&names);
            if cache.cached("pic.hg3").is_some() { break; }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert_eq!(cache.newly_ready().count(),1);
        cache.update(&resources,&[]);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn read_ahead_rejects_oversized_files_before_submitting_io() {
        let (root,_) = fixture();
        std::fs::File::create(root.join("huge.bin")).unwrap().set_len((MAX_RESOURCE+1) as u64).unwrap();
        let resources = ResourceIndex::scan(&root).unwrap();
        let mut cache = ReadAhead::default();
        cache.update(&resources,&["huge.bin".into()]);
        assert_eq!(cache.len(),0);
        assert!(cache.worker.is_none());
        assert!(cache.drain_notes().contains(&"byte-budget"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn read_ahead_rejects_stale_sources_and_limits_candidate_count() {
        let (root,resources)=fixture();
        let names=vec!["pic.hg3".to_owned()];
        let mut cache=ReadAhead::default();
        for _ in 0..200 {
            cache.update(&resources,&names);
            if cache.cached("pic.hg3").is_some() { break; }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(cache.cached("pic.hg3").is_some());
        std::fs::write(root.join("pic.hg3"),b"changed file identity").unwrap();
        assert!(cache.cached("pic.hg3").is_none());
        cache.update(&resources,&(0..100).map(|i|format!("resource{i}")).collect::<Vec<_>>());
        assert!(cache.len() <= 20);
        cache.update(&resources,&[]);
        std::fs::remove_dir_all(root).unwrap();
    }
}

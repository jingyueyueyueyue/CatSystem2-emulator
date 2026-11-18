use crate::{RuntimeError, RuntimeResult};
use blowfish::{
    BlowfishLE,
    cipher::{BlockDecrypt, KeyInit},
};
use cs2_formats::{Archive, ArchiveEntry};
use encoding_rs::SHIFT_JIS;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{Cursor, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use crate::resource_router::{ResourceMode, ResourceRouter, RoutedResource, directory_key, wide_query_key};

const KIF_HEADER_SIZE: u64 = 8;
const KIF_STANDARD_ENTRY_SIZE: u64 = 72;
const KIF_LEGACY_ENTRY_SIZE: u64 = 40;
const MAX_KIF_ENTRIES: u32 = 1_000_000;
const MAX_RESOURCE_BYTES: u64 = 64 * 1024 * 1024;
const CONFIG_BOOTSTRAP_PREFIXES: [&str; 4] = ["config/", "", "system/config/", "system/"];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DirectoryFile {
    pub relative_path: PathBuf,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ArchiveStatus {
    Indexed,
    Encrypted { raw_entry_count: u32 },
    NotAnArchive,
    Malformed { reason: String },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ArchiveLayout {
    Standard64,
    Legacy32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArchiveIndex {
    pub path: PathBuf,
    pub entry_count: u32,
    pub entries: Vec<ArchiveResourceEntry>,
    pub status: ArchiveStatus,
    #[serde(default)]
    pub layout: Option<ArchiveLayout>,
    payload_key: Option<u32>,
    pub name_seed: Option<u32>,
    pub name_seed_score: Option<i64>,
}

impl ArchiveIndex {
    pub fn is_encrypted(&self) -> bool {
        matches!(self.status, ArchiveStatus::Encrypted { .. })
            || self.payload_key.is_some()
            || self.name_seed.is_some()
    }

    pub fn is_readable(&self) -> bool {
        matches!(self.status, ArchiveStatus::Indexed)
    }

    pub fn raw_entry_count(&self) -> u32 {
        match self.status {
            ArchiveStatus::Encrypted { raw_entry_count } => raw_entry_count,
            _ => self.entry_count,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArchiveResourceEntry {
    pub name: String,
    pub raw_name: Vec<u8>,
    pub offset: u64,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ResourceLocation {
    File {
        path: PathBuf,
    },
    Archive {
        path: PathBuf,
        offset: u64,
        size: u64,
        name_seed: Option<u32>,
        payload_key: Option<u32>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResourceCandidate {
    pub name: String,
    pub location: ResourceLocation,
    pub size: u64,
    pub priority: i32,
}

pub type ResourceLeaseId = u64;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResourceLease {
    pub id: ResourceLeaseId,
    pub name: String,
    pub size: u64,
}

struct CachedResource {
    lease: ResourceLease,
    bytes: Arc<Vec<u8>>,
    references: u32,
    source: String,
}

pub struct ResourceCache {
    next_id: ResourceLeaseId,
    entries: BTreeMap<ResourceLeaseId, CachedResource>,
    by_source: BTreeMap<String, ResourceLeaseId>,
}

impl Default for ResourceCache {
    fn default() -> Self {
        Self::new()
    }
}

impl ResourceCache {
    pub fn new() -> Self {
        Self {
            next_id: 1,
            entries: BTreeMap::new(),
            by_source: BTreeMap::new(),
        }
    }

    pub fn acquire(
        &mut self,
        resources: &ResourceIndex,
        name: impl AsRef<str>,
    ) -> RuntimeResult<ResourceLease> {
        let name = name.as_ref();
        if name.is_empty() {
            return Err(RuntimeError::InvalidResourceName(name.to_owned()));
        }
        let candidate = resources
            .resolve(name)?
            .ok_or_else(|| RuntimeError::ResourceNotFound(name.to_owned()))?;
        self.acquire_candidate(resources, candidate)
    }

    pub fn acquire_candidate(
        &mut self,
        resources: &ResourceIndex,
        candidate: &ResourceCandidate,
    ) -> RuntimeResult<ResourceLease> {
        let key = normalize_name(&candidate.name);
        if key.is_empty() {
            return Err(RuntimeError::InvalidResourceName(candidate.name.clone()));
        }
        let source = resource_source_key(candidate);
        if let Some(&id) = self.by_source.get(&source) {
            let cached = self.entries.get_mut(&id).ok_or_else(|| {
                RuntimeError::Host("resource cache index is inconsistent".to_string())
            })?;
            cached.references = cached.references.checked_add(1).ok_or_else(|| {
                RuntimeError::Host("resource lease reference count overflowed".to_string())
            })?;
            return Ok(cached.lease.clone());
        }
        let bytes = resources.read_candidate(candidate)?;
        self.insert_loaded(key, source, bytes)
    }

    pub fn acquire_all_candidates(
        &mut self,
        resources: &ResourceIndex,
        name: impl AsRef<str>,
    ) -> Vec<(ResourceCandidate, RuntimeResult<ResourceLease>)> {
        resources
            .candidates(name)
            .iter()
            .cloned()
            .map(|candidate| {
                let result = self.acquire_candidate(resources, &candidate);
                (candidate, result)
            })
            .collect()
    }

    fn insert_loaded(
        &mut self,
        key: String,
        source: String,
        bytes: Vec<u8>,
    ) -> RuntimeResult<ResourceLease> {
        if self.next_id == 0 {
            return Err(RuntimeError::Host("resource lease id is zero".to_string()));
        }
        let id = self.next_id;
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or_else(|| RuntimeError::Host("resource lease id space exhausted".to_string()))?;
        let lease = ResourceLease {
            id,
            name: key.clone(),
            size: bytes.len() as u64,
        };
        self.entries.insert(
            id,
            CachedResource {
                lease: lease.clone(),
                bytes: Arc::new(bytes),
                references: 1,
                source: source.clone(),
            },
        );
        self.by_source.insert(source, id);
        Ok(lease)
    }

    pub fn bytes(&self, lease: ResourceLeaseId) -> RuntimeResult<Arc<Vec<u8>>> {
        self.entries
            .get(&lease)
            .map(|cached| Arc::clone(&cached.bytes))
            .ok_or_else(|| RuntimeError::ResourceNotFound(format!("resource lease {lease}")))
    }

    pub fn lease(&self, lease: ResourceLeaseId) -> Option<&ResourceLease> {
        self.entries.get(&lease).map(|cached| &cached.lease)
    }

    pub fn references(&self, lease: ResourceLeaseId) -> Option<u32> {
        self.entries.get(&lease).map(|cached| cached.references)
    }

    pub fn release(&mut self, lease: ResourceLeaseId) -> RuntimeResult<bool> {
        let cached = self
            .entries
            .get_mut(&lease)
            .ok_or_else(|| RuntimeError::ResourceNotFound(format!("resource lease {lease}")))?;
        if cached.references == 0 {
            return Err(RuntimeError::Host(format!(
                "resource lease {lease} has already been released"
            )));
        }
        cached.references -= 1;
        Ok(cached.references == 0)
    }

    pub fn evict_unused(&mut self) -> usize {
        let unused = self
            .entries
            .iter()
            .filter_map(|(&id, cached)| (cached.references == 0).then_some(id))
            .collect::<Vec<_>>();
        for id in &unused {
            if let Some(cached) = self.entries.remove(id) {
                self.by_source.remove(&cached.source);
            }
        }
        unused.len()
    }

    pub fn clear(&mut self) -> usize {
        let count = self.entries.len();
        self.entries.clear();
        self.by_source.clear();
        count
    }

    pub fn active_leases(&self) -> impl Iterator<Item = &ResourceLease> {
        self.entries.values().map(|cached| &cached.lease)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn validate(&self) -> RuntimeResult<()> {
        if self.next_id == 0 {
            return Err(RuntimeError::Host(
                "resource cache next lease id is zero".to_string(),
            ));
        }
        for (source, id) in &self.by_source {
            let cached = self.entries.get(id).ok_or_else(|| {
                RuntimeError::Host("resource cache source index is inconsistent".to_string())
            })?;
            if &cached.source != source || cached.lease.id != *id {
                return Err(RuntimeError::Host(
                    "resource cache contains an invalid source entry".to_string(),
                ));
            }
        }
        for (id, cached) in &self.entries {
            if *id == 0 || cached.lease.id != *id {
                return Err(RuntimeError::Host(
                    "resource cache contains an invalid lease".to_string(),
                ));
            }
            if self.by_source.get(&cached.source) != Some(id) {
                return Err(RuntimeError::Host(
                    "resource cache lease is missing from the source index".to_string(),
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ScanDiagnosticLevel {
    Warning,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScanDiagnostic {
    pub level: ScanDiagnosticLevel,
    pub path: PathBuf,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceIndex {
    root: PathBuf,
    files: Vec<DirectoryFile>,
    archives: Vec<ArchiveIndex>,
    candidates: BTreeMap<String, Vec<ResourceCandidate>>,
    diagnostics: Vec<ScanDiagnostic>,
    #[serde(default)]
    archive_name_seed: Option<u32>,
    #[serde(default)]
    archive_name_seeds: Vec<u32>,
    #[serde(default)]
    routing: Option<ResourceRouter>,
    #[serde(default)]
    routing_base: PathBuf,
    #[serde(default)]
    routing_error: Option<String>,
    /// The startup document used to prepare the mounts. Category lookup is
    /// based on this startup state, not on a later search for a loose alias.
    #[serde(default)]
    routing_startup: Option<Vec<u8>>,
    #[serde(skip)]
    directory_lookups: OnceLock<Vec<OnceLock<BTreeMap<Vec<u8>, usize>>>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfigMatch {
    Startup,
    Sound,
    Language,
    GameDefinition,
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ConfigKind {
    Startup,
    Sound,
    Language,
    GameDefinition,
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConfigCandidate {
    pub kind: ConfigKind,
    pub requested_name: String,
    pub resource: ResourceCandidate,
}

pub enum ResourceReader {
    File(File),
    Archive(Cursor<Vec<u8>>),
}

pub struct BoundedReader {
    file: File,
    start: u64,
    length: u64,
    position: u64,
}

/// A bounded encoded-media reader. Encrypted entries are decrypted a block at
/// a time; the final 0..7 bytes retain the archive's original clear tail.
pub struct MediaResourceReader {
    source: BoundedReader,
    cipher: Option<BlowfishLE>,
    block: [u8; 8],
    block_start: usize,
    block_length: usize,
}

impl ResourceIndex {
    /// Media decoding uses a stream rather than the small resource Vec API.
    /// The caller supplies a finite encoded-size ceiling (for example 4 GiB).
    pub fn open_media(
        &self, name: impl AsRef<str>, max_bytes: u64,
    ) -> RuntimeResult<MediaResourceReader> {
        let name = name.as_ref();
        let candidate = self.resolve(name)?
            .ok_or_else(|| RuntimeError::ResourceNotFound(name.to_owned()))?;
        self.open_media_candidate(candidate, max_bytes)
    }

    pub fn open_media_candidate(
        &self, candidate: &ResourceCandidate, max_bytes: u64,
    ) -> RuntimeResult<MediaResourceReader> {
        self.ensure_indexed_candidate(candidate)?;
        if max_bytes == 0 || candidate.size > max_bytes {
            return Err(RuntimeError::Archive(format!(
                "encoded media size {} exceeds caller limit {max_bytes}", candidate.size,
            )));
        }
        let (path, start, length, key) = match &candidate.location {
            ResourceLocation::File { path } => (path, 0, candidate.size, None),
            ResourceLocation::Archive { path, offset, size, payload_key, .. } => {
                if *size != candidate.size {
                    return Err(RuntimeError::Archive("media entry size differs from its index".into()));
                }
                (path, *offset, *size, *payload_key)
            }
        };
        let mut file = File::open(path).map_err(|source| RuntimeError::Io {
            path: path.clone(), source,
        })?;
        let file_length = file.metadata().map_err(|source| RuntimeError::Io {
            path: path.clone(), source,
        })?.len();
        if start.checked_add(length).is_none_or(|end| end > file_length) {
            return Err(RuntimeError::Archive("media entry points outside its current file".into()));
        }
        file.seek(SeekFrom::Start(start)).map_err(|source| RuntimeError::Io {
            path: path.clone(), source,
        })?;
        let cipher = key.map(|key| BlowfishLE::new_from_slice(&key.to_le_bytes()))
            .transpose()
            .map_err(|_| RuntimeError::Archive("invalid media payload key".into()))?;
        Ok(MediaResourceReader {
            source: BoundedReader { file, start, length, position: 0 },
            cipher, block: [0; 8], block_start: 0, block_length: 0,
        })
    }

    pub fn scan(root: impl AsRef<Path>) -> RuntimeResult<Self> {
        let root = root.as_ref().to_path_buf();
        if !root.is_dir() {
            return Err(RuntimeError::MissingGameDirectory(root));
        }
        let mut paths = Vec::new();
        collect_files(&root, &mut paths)?;
        paths.sort_by_key(|path| path_sort_key(path));
        let name_seeds = find_archive_name_seeds(&root, &paths);
        Self::scan_with_name_seeds(root, &name_seeds)
    }

    pub fn scan_with_name_seed(
        root: impl AsRef<Path>,
        name_seed: Option<u32>,
    ) -> RuntimeResult<Self> {
        let seeds = name_seed.into_iter().collect::<Vec<_>>();
        Self::scan_with_name_seeds(root, &seeds)
    }

    /// Scan a game directory with an ordered set of executable-derived archive
    /// name seeds. Each encrypted archive may select the first seed that
    /// produces a valid directory; this supports coexisting localized/runtime
    /// executable variants without changing the explicit single-seed API.
    pub fn scan_with_name_seeds(root: impl AsRef<Path>, name_seeds: &[u32]) -> RuntimeResult<Self> {
        let root = root.as_ref().to_path_buf();
        if !root.is_dir() {
            return Err(RuntimeError::MissingGameDirectory(root));
        }
        let name_seeds = name_seeds
            .iter()
            .copied()
            .fold(Vec::new(), |mut seeds, seed| {
                if !seeds.contains(&seed) {
                    seeds.push(seed);
                }
                seeds
            });

        let mut paths = Vec::new();
        collect_files(&root, &mut paths)?;
        paths.sort_by_key(|path| path_sort_key(path));

        let mut index = Self {
            root,
            files: Vec::new(),
            archives: Vec::new(),
            candidates: BTreeMap::new(),
            diagnostics: Vec::new(),
            archive_name_seed: name_seeds.first().copied(),
            archive_name_seeds: name_seeds.clone(),
            routing: None,
            routing_base: PathBuf::new(),
            routing_startup: None,
            routing_error: None,
            directory_lookups: OnceLock::new(),
        };

        for path in paths {
            let metadata = fs::metadata(&path).map_err(|source| RuntimeError::Io {
                path: path.clone(),
                source,
            })?;
            let relative_path = path
                .strip_prefix(&index.root)
                .map_err(|_| RuntimeError::InvalidResourceName(path.display().to_string()))?
                .to_path_buf();
            let size = metadata.len();
            index.files.push(DirectoryFile {
                relative_path: relative_path.clone(),
                size,
            });

            let priority = directory_priority(&relative_path);
            index.add_candidate(
                relative_path.to_string_lossy().as_ref(),
                ResourceCandidate {
                    name: relative_path.to_string_lossy().to_string(),
                    location: ResourceLocation::File { path: path.clone() },
                    size,
                    priority,
                },
            )?;

            if let Some(file_name) = relative_path.file_name().and_then(|name| name.to_str()) {
                if relative_path.components().count() > 1 {
                    index.add_candidate(
                        file_name,
                        ResourceCandidate {
                            name: relative_path.to_string_lossy().to_string(),
                            location: ResourceLocation::File { path: path.clone() },
                            size,
                            priority: priority - 1,
                        },
                    )?;
                }
            }

            if path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("int"))
            {
                let archive = parse_archive(&path, relative_path, &name_seeds)?;
                for entry in &archive.entries {
                    let archive_priority = archive_priority(&archive.path);
                    let candidate = ResourceCandidate {
                            name: entry.name.clone(),
                            location: ResourceLocation::Archive {
                                path: path.clone(),
                                offset: entry.offset,
                                size: entry.size,
                                name_seed: archive.name_seed,
                                payload_key: archive.payload_key,
                            },
                            size: entry.size,
                            priority: archive_priority,
                        };
                    index.add_candidate(&entry.name, candidate.clone())?;
                    // A virtual resource contains the archive identity. Keeping
                    // it avoids taking a same-named entry from another mount.
                    let qualified = format!(
                        "{}/{}",
                        archive.path.to_string_lossy().replace('\\', "/"),
                        entry.name
                    );
                    index.add_candidate(&qualified, candidate.clone())?;
                    if let Some(file_name) = archive.path.file_name().and_then(|value| value.to_str()) {
                        let qualified = format!("{file_name}/{}", entry.name);
                        index.add_candidate(&qualified, candidate)?;
                    }
                }
                let diagnostic = match &archive.status {
                    ArchiveStatus::Encrypted { .. } => {
                        Some("archive is encrypted and needs the executable name seed".to_string())
                    }
                    ArchiveStatus::Malformed { reason } => Some(reason.clone()),
                    ArchiveStatus::Indexed | ArchiveStatus::NotAnArchive => None,
                };
                if let Some(message) = diagnostic {
                    index.diagnostics.push(ScanDiagnostic {
                        level: ScanDiagnosticLevel::Warning,
                        path: archive.path.clone(),
                        message,
                    });
                }
                index.archives.push(archive);
            }
        }

        for candidates in index.candidates.values_mut() {
            candidates.sort_by(compare_candidates);
            candidates.dedup();
        }

        // Keep inventory usable when startup data is corrupt, but never let
        // production reads silently fall back to the inventory's global sort.
        if let Err(cause) = index.initialize_resource_router() {
            index.routing_error = Some(cause.to_string());
            index.diagnostics.push(ScanDiagnostic {
                level: ScanDiagnosticLevel::Error, path: PathBuf::from("startup.xml"),
                message: format!("resource routing initialization: {cause}"),
            });
        }

        Ok(index)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn files(&self) -> &[DirectoryFile] {
        &self.files
    }

    pub fn archives(&self) -> &[ArchiveIndex] {
        &self.archives
    }

    pub fn diagnostics(&self) -> &[ScanDiagnostic] {
        &self.diagnostics
    }

    pub fn archive_name_seed(&self) -> Option<u32> {
        self.archive_name_seed
    }

    pub fn archive_name_seeds(&self) -> &[u32] {
        &self.archive_name_seeds
    }

    pub fn archive_payload_key(&self, path: impl AsRef<Path>) -> Option<u32> {
        self.archive_index(path)
            .and_then(|archive| archive.payload_key)
    }

    pub fn candidates(&self, name: impl AsRef<str>) -> &[ResourceCandidate] {
        let key = normalize_name(name.as_ref());
        self.candidates
            .get(&key)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    pub fn archive_candidates(
        &self,
        name: impl AsRef<str>,
    ) -> impl Iterator<Item = &ResourceCandidate> {
        self.candidates(name)
            .iter()
            .filter(|candidate| matches!(&candidate.location, ResourceLocation::Archive { .. }))
    }

    pub fn archive_index(&self, path: impl AsRef<Path>) -> Option<&ArchiveIndex> {
        let requested = normalize_name(&path.as_ref().to_string_lossy());
        self.archives
            .iter()
            .find(|archive| normalize_name(&archive.path.to_string_lossy()) == requested)
    }

    pub fn find(&self, name: impl AsRef<str>) -> Option<&ResourceCandidate> {
        // Compatibility presence query cannot express diagnostics. Actual
        // reads/open/cache use resolve(), which propagates routing errors.
        self.resolve(name.as_ref()).ok().flatten()
    }

    pub fn resource_router(&self) -> Option<&ResourceRouter> { self.routing.as_ref() }

    /// Explicit hosts can install their actual dynamic registration state.
    pub fn set_resource_router(&mut self, router: ResourceRouter) -> RuntimeResult<()> {
        if router.slots.len() != 256 { return Err(RuntimeError::Host("resource router requires 256 slots".into())); }
        if router.slots.iter().filter_map(|s| s.archive.as_ref()).chain(router.patches.iter())
            .any(|mount| mount.archive >= self.archives.len()) {
            return Err(RuntimeError::Host("resource mount refers to an absent archive".into()));
        }
        self.routing = Some(router);
        self.routing_error = None;
        Ok(())
    }

    /// Existing API: bare names use the empty-prefix LooseFirst search;
    /// registered virtual names use LooseFirst; explicit host file paths use
    /// File. All three share the same patch-first routing core.
    pub fn resolve(&self, name: &str) -> RuntimeResult<Option<&ResourceCandidate>> {
        let wide = name.encode_utf16().collect::<Vec<_>>();
        let (prefix, _) = crate::resource_router::split_name(crate::resource_router::terminated(&wide));
        let mut virtual_name = prefix.is_empty() || crate::resource_router::list_prefix(prefix);
        if !virtual_name {
            if let Some(router) = &self.routing {
                for slot in &router.slots {
                    if crate::resource_router::prefix_equal(prefix, &slot.prefix)? {
                        virtual_name = true;
                        break;
                    }
                }
            }
        }
        self.resolve_mode_utf16(&wide, if virtual_name { ResourceMode::LooseFirst } else { ResourceMode::File })
            .map(|found| found.map(|found| found.value))
    }

    pub fn resolve_mode_utf16(&self, name: &[u16], mode: ResourceMode)
        -> RuntimeResult<Option<RoutedResource<&ResourceCandidate>>> {
        if let Some(cause) = &self.routing_error { return Err(RuntimeError::Host(cause.clone())); }
        let Some(router) = &self.routing else {
            return Err(RuntimeError::Host("resource routing has not been initialized".into()));
        };
        router.route_utf16(name, mode,
            |path| self.routed_file_candidate(path),
            |archive, internal| self.routed_archive_candidate(archive, internal))
    }

    pub fn read_mode(&self, name: &str, mode: ResourceMode) -> RuntimeResult<Option<Vec<u8>>> {
        self.read_mode_utf16(&name.encode_utf16().collect::<Vec<_>>(), mode)
    }

    pub fn read_mode_utf16(&self, name: &[u16], mode: ResourceMode) -> RuntimeResult<Option<Vec<u8>>> {
        self.resolve_mode_utf16(name, mode)?.map(|found| self.read_candidate(found.value)).transpose()
    }

    /// A literal file opened by an engine file object, independently of the
    /// resource service. In particular this does not apply patch archives or
    /// category completion. Access remains inside the portable game's root.
    pub fn resolve_plain_file(&self, name: &str) -> RuntimeResult<Option<&ResourceCandidate>> {
        let wide = name.encode_utf16().collect::<Vec<_>>();
        let wide = crate::resource_router::terminated(&wide);
        if wide.len() > 259 {
            return Err(RuntimeError::InvalidResourceName("plain file name exceeds 259 UTF-16 units".into()));
        }
        self.routed_file_candidate(wide)
    }

    fn routed_file_candidate(&self, name: &[u16]) -> RuntimeResult<Option<&ResourceCandidate>> {
        let name = String::from_utf16(name).map_err(|_| RuntimeError::InvalidResourceName("invalid UTF-16 file name".into()))?;
        let relative = PathBuf::from(name.replace('\\', "/"));
        // The portable host keeps original game access inside its read-only
        // root. Do not turn a leading backslash into a relative path.
        if relative.is_absolute() || relative.components().any(|part| matches!(part,
            std::path::Component::ParentDir | std::path::Component::Prefix(_) | std::path::Component::RootDir)) {
            return Ok(None);
        }
        let relative = if !self.routing_base.as_os_str().is_empty() && relative.starts_with(&self.routing_base) {
            relative
        } else { self.routing_base.join(relative) };
        let key = relative.to_string_lossy();
        let candidate = self.candidates(key.as_ref()).iter().find(|candidate| match &candidate.location {
            ResourceLocation::File { path } => normalize_name(&path.to_string_lossy()) == normalize_name(&self.root.join(&relative).to_string_lossy()),
            _ => false,
        });
        candidate.map(probe_candidate).transpose().map(Option::flatten)
    }

    fn routed_archive_candidate(&self, index: usize, name: &[u16]) -> RuntimeResult<Option<&ResourceCandidate>> {
        let archive = self.archives.get(index).ok_or_else(|| RuntimeError::Host("resource mount index is outside archive inventory".into()))?;
        if !archive.is_readable() { return Ok(None); }
        let lookups = self.directory_lookups.get_or_init(|| {
            (0..self.archives.len()).map(|_| OnceLock::new()).collect()
        });
        let lookup = lookups[index].get_or_init(|| {
            let mut lookup = BTreeMap::new();
            for (i, entry) in archive.entries.iter().enumerate() {
                lookup.entry(directory_key(&entry.raw_name)).or_insert(i);
            }
            lookup
        });
        let Some(&entry) = lookup.get(&wide_query_key(name)?) else { return Ok(None); };
        let entry = &archive.entries[entry];
        let path = self.root.join(&archive.path);
        let candidate = self.candidates(&entry.name).iter().find(|candidate| {
            matches!(&candidate.location, ResourceLocation::Archive { path: stored, offset, size, .. }
                if stored == &path && *offset == entry.offset && *size == entry.size)
        }).ok_or_else(|| RuntimeError::Archive("archive directory has no matching indexed payload".into()))?;
        probe_candidate(candidate)
    }

    pub fn initialize_resource_router(&mut self) -> RuntimeResult<()> {
        self.routing_startup = None;
        let mut startup = None;
        for prefix in CONFIG_BOOTSTRAP_PREFIXES {
            if let Some(candidate) = self.candidates(format!("{prefix}startup.xml")).first() {
                startup = Some(candidate.clone());
                break;
            }
        }
        let (config, base) = if let Some(candidate) = startup {
            let bytes = self.read_candidate(&candidate)?;
            let config = cs2_formats::ConfigXml::parse_runtime(&bytes)
                .map_err(|cause| RuntimeError::Host(format!("startup runtime XML: {cause}")))?;
            self.routing_startup = Some(bytes);
            let parent = match &candidate.location {
                ResourceLocation::File { path } => {
                    let parent = path.parent().unwrap_or(&self.root);
                    if parent.file_name().is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case("config")) {
                        parent.parent().unwrap_or(&self.root)
                    } else { parent }
                },
                ResourceLocation::Archive { path, .. } => path.parent().unwrap_or(&self.root),
            };
            (Some(config), parent.strip_prefix(&self.root).unwrap_or(Path::new("")).to_path_buf())
        } else { (None, PathBuf::new()) };
        let router = ResourceRouter::from_index(self, config.as_ref(), &base)?;
        self.routing_base = base;
        self.set_resource_router(router)
    }

    pub fn startup_configuration(&self) -> RuntimeResult<Option<cs2_formats::ConfigXml>> {
        if let Some(cause) = &self.routing_error { return Err(RuntimeError::Host(cause.clone())); }
        self.routing_startup.as_deref().map(|bytes| {
            cs2_formats::ConfigXml::parse_runtime(bytes)
                .map_err(|cause| RuntimeError::Host(format!("startup runtime XML: {cause}")))
        }).transpose()
    }

    /// Load a named configuration through its declared category, with file
    /// paths for legacy games that have no config category.
    pub fn read_optional_configuration(
        &self,
        name: &str,
    ) -> RuntimeResult<Option<cs2_formats::ConfigXml>> {
        if name.eq_ignore_ascii_case("startup.xml") {
            return self.startup_configuration();
        }
        let (names, mode) = match self.optional_category_names(b"config", name)? {
            Some(names) => (names, ResourceMode::LooseFirst),
            None => (
                CONFIG_BOOTSTRAP_PREFIXES
                    .iter()
                    .map(|prefix| format!("{prefix}{name}"))
                    .collect(),
                ResourceMode::File,
            ),
        };
        for name in names {
            if let Some(bytes) = self.read_mode(&name, mode)? {
                return cs2_formats::ConfigXml::parse_runtime(&bytes)
                    .map(Some)
                    .map_err(|cause| RuntimeError::Host(format!("configuration {name:?}: {cause}")));
            }
        }
        Ok(None)
    }

    /// Resolve one virtual name. The router applies the corresponding slot's
    /// loose/archive order; returning separate loose names would reorder it.
    pub fn category_names(&self, category: &[u8], source: &str) -> RuntimeResult<Vec<String>> {
        self.optional_category_names(category, source)?.ok_or_else(|| {
            RuntimeError::InvalidResourceName(format!(
                "resource category is absent: {}", String::from_utf8_lossy(category)
            ))
        })
    }

    pub fn optional_category_names(&self, category: &[u8], source: &str)
        -> RuntimeResult<Option<Vec<String>>> {
        let source = source.split('\0').next().unwrap_or_default().to_owned();
        if crate::resource_router::has_category_directory(&source)? {
            return Ok(Some(vec![source]));
        }
        if let Some(config) = self.startup_configuration()? {
            for slot in 0..128 {
                let key = format!("int{slot:02}");
                let path = [b"document".as_slice(), b"INT", key.as_bytes(), b"name"];
                let name = config.text(&path).unwrap_or_default();
                if name.len() >= 32 { return Err(RuntimeError::Host("INT name exceeds original capacity".into())); }
                if !name.eq_ignore_ascii_case(category) { continue; }
                return crate::resource_router::complete_category(&config, slot, &source)
                    .map(|name| Some(vec![name]));
            }
        }
        Ok(None)
    }

    pub fn read_category(&self, category: &[u8], name: &str) -> RuntimeResult<Vec<u8>> {
        let names = self.category_names(category, name)?;
        for candidate in &names {
            if let Some(bytes) = self.read_mode(candidate, ResourceMode::LooseFirst)? {
                return Ok(bytes);
            }
        }
        Err(RuntimeError::ResourceNotFound(names.join(" | ")))
    }

    pub fn category_slot_name(&self, slot: u32, source: &str) -> RuntimeResult<String> {
        if slot >= 128 { return Err(RuntimeError::InvalidResourceName("INT category slot is outside 0..127".into())); }
        let source = source.split('\0').next().unwrap_or_default();
        if crate::resource_router::has_category_directory(source)? { return Ok(source.to_owned()); }
        if let Some(config) = self.startup_configuration()? {
            return crate::resource_router::complete_category(&config, slot, source);
        }
        Err(RuntimeError::ResourceNotFound("startup.xml".into()))
    }

    pub fn open_candidate(&self, candidate: &ResourceCandidate) -> RuntimeResult<ResourceReader> {
        self.ensure_indexed_candidate(candidate)?;
        if candidate.size > MAX_RESOURCE_BYTES {
            return Err(RuntimeError::Archive(format!(
                "resource exceeds the supported {} MiB limit",
                MAX_RESOURCE_BYTES / (1024 * 1024)
            )));
        }
        ResourceReader::open(candidate)
    }

    pub fn read_candidate(&self, candidate: &ResourceCandidate) -> RuntimeResult<Vec<u8>> {
        self.ensure_indexed_candidate(candidate)?;
        if candidate.size > MAX_RESOURCE_BYTES {
            return Err(RuntimeError::Archive(format!(
                "resource exceeds the supported {} MiB limit",
                MAX_RESOURCE_BYTES / (1024 * 1024)
            )));
        }
        let mut reader = ResourceReader::open(candidate)?;
        let mut bytes = Vec::with_capacity(
            usize::try_from(candidate.size)
                .map_err(|_| RuntimeError::Archive("resource is too large".to_string()))?,
        );
        reader
            .read_to_end(&mut bytes)
            .map_err(|source| RuntimeError::Io {
                path: self.root.clone(),
                source,
            })?;
        Ok(bytes)
    }

    fn ensure_indexed_candidate(&self, candidate: &ResourceCandidate) -> RuntimeResult<()> {
        let same_source = |indexed: &ResourceCandidate| {
            indexed.location == candidate.location && indexed.size == candidate.size
        };
        // Candidate.name retains the original archive entry or loose path even
        // when it was found through a virtual alias. Avoid a full archive scan
        // for each image/audio packet lookup.
        if self.candidates(&candidate.name).iter().any(same_source) {
            return Ok(());
        }
        let indexed = self.candidates.values().any(|candidates| {
            candidates.iter().any(|indexed| {
                indexed.location == candidate.location && indexed.size == candidate.size
            })
        });
        if indexed {
            return Ok(());
        }
        Err(RuntimeError::InvalidResourceName(candidate.name.clone()))
    }

    pub fn read_candidates(
        &self,
        name: impl AsRef<str>,
    ) -> Vec<(ResourceCandidate, RuntimeResult<Vec<u8>>)> {
        self.candidates(name)
            .iter()
            .cloned()
            .map(|candidate| {
                let result = self.read_candidate(&candidate);
                (candidate, result)
            })
            .collect()
    }

    pub fn read_all_archive_candidates(
        &self,
        name: impl AsRef<str>,
    ) -> Vec<(ResourceCandidate, RuntimeResult<Vec<u8>>)> {
        self.archive_candidates(name)
            .cloned()
            .map(|candidate| {
                let result = self.read_candidate(&candidate);
                (candidate, result)
            })
            .collect()
    }

    pub fn read_all_candidates(
        &self,
        name: impl AsRef<str>,
    ) -> Vec<(ResourceCandidate, RuntimeResult<Vec<u8>>)> {
        self.candidates(name)
            .iter()
            .cloned()
            .map(|candidate| {
                let result = self.read_candidate(&candidate);
                (candidate, result)
            })
            .collect()
    }

    pub fn open(&self, name: impl AsRef<str>) -> RuntimeResult<ResourceReader> {
        let name = name.as_ref();
        let candidate = self
            .resolve(name)?
            .ok_or_else(|| RuntimeError::ResourceNotFound(name.to_string()))?;
        self.open_candidate(candidate)
    }

    pub fn read(&self, name: impl AsRef<str>) -> RuntimeResult<Vec<u8>> {
        let name = name.as_ref().to_string();
        let candidate = self
            .resolve(&name)?
            .ok_or_else(|| RuntimeError::ResourceNotFound(name.clone()))?;
        self.read_candidate(candidate)
    }

    /// Reads an entry from an encrypted KIF archive using the seed supplied by
    /// the runtime fingerprint/native configuration layer.
    ///
    /// The seed is data passed into the simulator. It is never obtained by
    /// launching or loading the original game executable.
    pub fn read_archive_with_name_seed(
        &self,
        archive_name: impl AsRef<str>,
        entry_name: impl AsRef<[u8]>,
        name_seed: u32,
    ) -> RuntimeResult<Vec<u8>> {
        let requested = normalize_name(&archive_name.as_ref().to_string());
        let archive = self
            .archives
            .iter()
            .find(|archive| normalize_name(&archive.path.to_string_lossy()) == requested)
            .ok_or_else(|| RuntimeError::ResourceNotFound(archive_name.as_ref().to_string()))?;
        let path = self.root.join(&archive.path);
        let mut file = File::open(&path).map_err(|source| RuntimeError::Io {
            path: path.clone(),
            source,
        })?;
        let archive_length = file
            .metadata()
            .map_err(|source| RuntimeError::Io {
                path: path.clone(),
                source,
            })?
            .len();
        let (directory, entry_count, layout) =
            read_archive_directory(&mut file, &path, archive_length)?;
        let directory = normalize_kif_table(&directory, entry_count, layout)?;
        let parsed = Archive::parse_directory(&directory, archive_length, Some(name_seed))
            .map_err(|error| RuntimeError::Archive(error.to_string()))?;
        let entry = parsed.find(entry_name.as_ref()).ok_or_else(|| {
            RuntimeError::ResourceNotFound(String::from_utf8_lossy(entry_name.as_ref()).to_string())
        })?;
        read_archive_entry(&mut file, &path, &parsed, entry)
    }

    /// Read one entry from a specific indexed archive using the seed selected
    /// for that archive during directory scanning.
    pub fn read_archive_entry(
        &self,
        archive_name: impl AsRef<Path>,
        entry_name: impl AsRef<[u8]>,
    ) -> RuntimeResult<Vec<u8>> {
        let archive_name = archive_name.as_ref();
        let requested = normalize_name(&archive_name.to_string_lossy());
        let archive = self
            .archives
            .iter()
            .position(|archive| normalize_name(&archive.path.to_string_lossy()) == requested)
            .ok_or_else(|| {
                RuntimeError::ResourceNotFound(archive_name.to_string_lossy().to_string())
            })?;
        let entry = self.archives[archive]
            .entries
            .iter()
            .position(|entry| names_equal(&entry.raw_name, entry_name.as_ref()))
            .ok_or_else(|| {
                RuntimeError::ResourceNotFound(
                    String::from_utf8_lossy(entry_name.as_ref()).to_string(),
                )
            })?;
        self.read_archive_entry_at(archive, entry)
    }

    pub fn read_archive_entry_by_index(
        &self,
        archive: &ArchiveIndex,
        entry: &ArchiveResourceEntry,
    ) -> RuntimeResult<Vec<u8>> {
        let archive_index = self
            .archives
            .iter()
            .position(|candidate| candidate.path == archive.path)
            .ok_or_else(|| RuntimeError::ResourceNotFound(archive.path.display().to_string()))?;
        let entry_index = self.archives[archive_index]
            .entries
            .iter()
            .position(|candidate| candidate == entry)
            .ok_or_else(|| RuntimeError::ResourceNotFound(entry.name.clone()))?;
        self.read_archive_entry_at(archive_index, entry_index)
    }

    pub fn read_archive_candidates_by_index(
        &self,
        archive_index: usize,
    ) -> Vec<(ArchiveResourceEntry, RuntimeResult<Vec<u8>>)> {
        let Some(archive) = self.archives.get(archive_index) else {
            return Vec::new();
        };
        archive
            .entries
            .iter()
            .cloned()
            .enumerate()
            .map(|(entry_index, entry)| {
                let result = self.read_archive_entry_at(archive_index, entry_index);
                (entry, result)
            })
            .collect()
    }

    /// Read an entry by its indexes in the scan result. The archive directory
    /// has already been decrypted and range-checked, so this avoids reparsing
    /// a large KIF table for every script during a workspace audit.
    pub fn read_archive_entry_at(
        &self,
        archive_index: usize,
        entry_index: usize,
    ) -> RuntimeResult<Vec<u8>> {
        let archive = self
            .archives
            .get(archive_index)
            .ok_or_else(|| RuntimeError::Archive("archive index is out of range".to_string()))?;
        let entry = archive.entries.get(entry_index).ok_or_else(|| {
            RuntimeError::Archive("archive entry index is out of range".to_string())
        })?;
        let path = self.root.join(&archive.path);
        let mut file = File::open(&path).map_err(|source| RuntimeError::Io {
            path: path.clone(),
            source,
        })?;
        let file_length = file
            .metadata()
            .map_err(|source| RuntimeError::Io {
                path: path.clone(),
                source,
            })?
            .len();
        if entry
            .offset
            .checked_add(entry.size)
            .is_none_or(|end| end > file_length)
        {
            return Err(RuntimeError::Archive(
                "resource points outside the archive".to_string(),
            ));
        }
        let size = usize::try_from(entry.size)
            .map_err(|_| RuntimeError::Archive("resource is too large".to_string()))?;
        if entry.size > MAX_RESOURCE_BYTES {
            return Err(RuntimeError::Archive(format!(
                "resource exceeds the supported {} MiB limit",
                MAX_RESOURCE_BYTES / (1024 * 1024)
            )));
        }
        let mut bytes = vec![0_u8; size];
        file.seek(SeekFrom::Start(entry.offset))
            .and_then(|_| file.read_exact(&mut bytes))
            .map_err(|source| RuntimeError::Io {
                path: path.clone(),
                source,
            })?;
        decrypt_payload_with_key(&mut bytes, archive.payload_key)?;
        Ok(bytes)
    }

    pub fn config_candidates(&self) -> Vec<ConfigCandidate> {
        let mut result = Vec::new();
        for requested_name in [
            "startup.xml",
            "config/startup.xml",
            "soundconf.xml",
            "config/soundconf.xml",
            "adv.xml",
            "gamedef.xml",
            "config/gamedef.xml",
            "language_cn.txt",
            "language_en.txt",
            "config/gamelangtbl_cn.txt",
            "config/gamelangtbl_en.txt",
            "boot.dfn",
        ] {
            let kind = config_kind(config_match(requested_name));
            for resource in self.candidates(requested_name) {
                result.push(ConfigCandidate {
                    kind: kind.clone(),
                    requested_name: requested_name.to_string(),
                    resource: resource.clone(),
                });
            }
        }
        result
    }

    fn add_candidate(&mut self, name: &str, candidate: ResourceCandidate) -> RuntimeResult<()> {
        let key = normalize_name(name);
        if key.is_empty() {
            return Err(RuntimeError::InvalidResourceName(name.to_string()));
        }
        self.candidates.entry(key).or_default().push(candidate);
        Ok(())
    }
}

impl ResourceReader {
    fn open(candidate: &ResourceCandidate) -> RuntimeResult<Self> {
        match &candidate.location {
            ResourceLocation::File { path } => {
                File::open(path)
                    .map(ResourceReader::File)
                    .map_err(|source| RuntimeError::Io {
                        path: path.clone(),
                        source,
                    })
            }
            ResourceLocation::Archive {
                path,
                offset,
                size,
                payload_key,
                ..
            } => {
                let mut file = File::open(path).map_err(|source| RuntimeError::Io {
                    path: path.clone(),
                    source,
                })?;
                let mut payload = read_file_range(&mut file, path, *offset, *size)?;
                decrypt_payload_with_key(&mut payload, *payload_key)?;
                Ok(ResourceReader::Archive(Cursor::new(payload)))
            }
        }
    }
}

impl Read for ResourceReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::File(file) => file.read(buffer),
            Self::Archive(reader) => reader.read(buffer),
        }
    }
}

impl Seek for ResourceReader {
    fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
        match self {
            Self::File(file) => file.seek(position),
            Self::Archive(reader) => reader.seek(position),
        }
    }
}

impl Read for BoundedReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if self.position >= self.length || buffer.is_empty() {
            return Ok(0);
        }
        let remaining = self.length - self.position;
        let count = buffer
            .len()
            .min(usize::try_from(remaining).unwrap_or(usize::MAX));
        let read = self.file.read(&mut buffer[..count])?;
        if read == 0 && remaining != 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof, "indexed resource was truncated",
            ));
        }
        self.position += read as u64;
        Ok(read)
    }
}

impl Read for MediaResourceReader {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        let Some(cipher) = &self.cipher else {
            return self.source.read(output);
        };
        let mut written = 0;
        while written < output.len() {
            if self.block_start == self.block_length {
                let remaining = self.source.length.saturating_sub(self.source.position);
                if remaining == 0 {
                    break;
                }
                let count = remaining.min(8) as usize;
                self.source.read_exact(&mut self.block[..count])?;
                if count == 8 {
                    cipher.decrypt_block((&mut self.block).into());
                }
                self.block_start = 0;
                self.block_length = count;
            }
            let count = (self.block_length - self.block_start).min(output.len() - written);
            output[written..written + count].copy_from_slice(
                &self.block[self.block_start..self.block_start + count],
            );
            self.block_start += count;
            written += count;
        }
        Ok(written)
    }
}

impl Seek for BoundedReader {
    fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
        let next = match position {
            SeekFrom::Start(offset) => offset as i128,
            SeekFrom::Current(offset) => self.position as i128 + offset as i128,
            SeekFrom::End(offset) => self.length as i128 + offset as i128,
        };
        if !(0..=self.length as i128).contains(&next) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "resource seek outside bounded archive entry",
            ));
        }
        self.file.seek(SeekFrom::Start(self.start + next as u64))?;
        self.position = next as u64;
        Ok(self.position)
    }
}

fn collect_files(directory: &Path, output: &mut Vec<PathBuf>) -> RuntimeResult<()> {
    let entries = fs::read_dir(directory).map_err(|source| RuntimeError::Io {
        path: directory.to_path_buf(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| RuntimeError::Io {
            path: directory.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|source| RuntimeError::Io {
            path: path.clone(),
            source,
        })?;
        if file_type.is_dir() {
            if !is_ignored_directory(&path) {
                collect_files(&path, output)?;
            }
        } else if file_type.is_file() {
            output.push(path);
        }
    }
    Ok(())
}

fn is_ignored_directory(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| {
            let lower = name.to_ascii_lowercase();
            lower == ".git"
                || lower == "target"
                || lower.contains("backup")
                || name.contains("备份")
                || name.contains("補丁")
                || name.contains("补丁")
        })
        .unwrap_or(false)
}

fn probe_candidate(candidate: &ResourceCandidate) -> RuntimeResult<Option<&ResourceCandidate>> {
    let path = match &candidate.location {
        ResourceLocation::File { path } | ResourceLocation::Archive { path, .. } => path,
    };
    match File::open(path) {
        Ok(file) => {
            let metadata = file.metadata().map_err(|source| RuntimeError::Io { path: path.clone(), source })?;
            Ok(metadata.is_file().then_some(candidate))
        }
        Err(source) if matches!(source.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied) => Ok(None),
        Err(source) => Err(RuntimeError::Io { path: path.clone(), source }),
    }
}

fn path_sort_key(path: &Path) -> String {
    path.to_string_lossy().to_ascii_lowercase()
}

fn normalize_name(name: &str) -> String {
    name.replace('\\', "/")
        .trim_start_matches("./")
        .trim_start_matches('/')
        .to_ascii_lowercase()
}

fn resource_source_key(candidate: &ResourceCandidate) -> String {
    match &candidate.location {
        ResourceLocation::File { path } => {
            format!("file:{}", path.to_string_lossy().to_ascii_lowercase())
        }
        ResourceLocation::Archive {
            path,
            offset,
            size,
            name_seed,
            payload_key,
        } => format!(
            "archive:{}:{offset}:{size}:{name_seed:?}:{payload_key:?}",
            path.to_string_lossy().to_ascii_lowercase()
        ),
    }
}

fn directory_priority(path: &Path) -> i32 {
    1_000 - path.components().count() as i32 * 10
}

fn archive_priority(path: &Path) -> i32 {
    let file_name = path
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let update_number = file_name
        .strip_prefix("update")
        .and_then(|suffix| suffix.parse::<i32>().ok())
        .unwrap_or(-1);
    if is_update_archive(path) {
        2_000 + update_number.max(0)
    } else {
        500
    }
}

fn is_update_archive(path: &Path) -> bool {
    path.file_stem().and_then(|value| value.to_str()).is_some_and(|name| {
        name.get(..6).is_some_and(|prefix| prefix.eq_ignore_ascii_case("update"))
            && name.get(6..).is_some_and(|suffix| {
                suffix.len() == 2 && suffix.bytes().all(|value| value.is_ascii_digit())
            })
    })
}

fn find_archive_name_seeds(root: &Path, paths: &[PathBuf]) -> Vec<u32> {
    let mut executables = paths
        .iter()
        .filter(|path| {
            path.extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
                && is_runtime_executable(root, path)
                && !is_utility_executable(path)
        })
        .collect::<Vec<_>>();
    executables.sort_by_key(|path| executable_seed_rank(path));
    let mut seeds = Vec::new();
    for path in executables {
        if let Some(seed) = crate::pe_resources::archive_name_seed(&root.join(path))
            .ok()
            .flatten()
            && !seeds.contains(&seed)
        {
            seeds.push(seed);
        }
    }
    seeds
}

fn is_runtime_executable(root: &Path, path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if !name.ends_with(".exe") || is_utility_executable(path) {
        return false;
    }
    let known_runtime = name == "cs2_open.exe"
        || name == "cs2.exe"
        || name.starts_with("nekopara")
        || name.starts_with("grisaia");
    let is_root_executable = path.parent().is_some_and(|parent| parent == root);
    known_runtime
        || (is_root_executable
            && !path.components().any(|component| {
                let component = component.as_os_str().to_string_lossy().to_ascii_lowercase();
                component == "system"
                    || component == "tool"
                    || component == "tools"
                    || component == "directx"
                    || component.contains("patch")
                    || component.contains("backup")
                    || component.contains("补丁")
                    || component.contains("备份")
            }))
}

fn is_utility_executable(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    name.contains("install")
        || name.contains("uninstall")
        || name.contains("boot")
        || name.contains("setup")
}

fn executable_seed_rank(path: &Path) -> (u8, usize, String) {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let rank = if name == "cs2_open.exe" {
        0
    } else if name == "cs2.exe" {
        1
    } else if name.starts_with("nekopara") {
        2
    } else {
        3
    };
    (rank, path.components().count(), name)
}

fn compare_candidates(left: &ResourceCandidate, right: &ResourceCandidate) -> Ordering {
    right
        .priority
        .cmp(&left.priority)
        .then_with(|| left.name.cmp(&right.name))
}

fn parse_archive(
    path: &Path,
    relative_path: PathBuf,
    name_seeds: &[u32],
) -> RuntimeResult<ArchiveIndex> {
    let mut file = File::open(path).map_err(|source| RuntimeError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let file_length = file
        .metadata()
        .map_err(|source| RuntimeError::Io {
            path: path.to_path_buf(),
            source,
        })?
        .len();
    if file_length >= 8 {
        let mut signature = [0u8; 8];
        file.read_exact(&mut signature).map_err(|source| RuntimeError::Io {
            path: path.to_path_buf(), source,
        })?;
        if &signature == cs2_formats::archive::fuyin::SIGNATURE {
            return parse_fuyin_archive(&mut file, path, relative_path, file_length);
        }
    }
    let (table, entry_count, layout) = match read_archive_directory(&mut file, path, file_length) {
        Ok(value) => value,
        Err(RuntimeError::Archive(reason)) if reason == "invalid KIF signature" => {
            return Ok(ArchiveIndex {
                path: relative_path,
                entry_count: 0,
                entries: Vec::new(),
                status: ArchiveStatus::NotAnArchive,
                layout: None,
                payload_key: None,
                name_seed: None,
                name_seed_score: None,
            });
        }
        Err(RuntimeError::Archive(reason)) => {
            return Ok(malformed_archive(relative_path, 0, reason));
        }
        Err(error) => return Err(error),
    };
    let parse_table = match normalize_kif_table(&table, entry_count, layout) {
        Ok(table) => table,
        Err(error) => {
            return Ok(malformed_archive(
                relative_path,
                entry_count,
                error.to_string(),
            ));
        }
    };
    let encrypted = kif_table_is_encrypted(&table);
    let (parsed, selected_name_seed, selected_seed_score) = if encrypted {
        let mut last_error = None;
        let mut selected = None;
        for &name_seed in name_seeds {
            match Archive::parse_directory(&parse_table, file_length, Some(name_seed)) {
                Ok(archive) => {
                    let score = archive_name_score(&archive.entries);
                    if selected
                        .as_ref()
                        .is_none_or(|(_, _, selected_score)| score > *selected_score)
                    {
                        selected = Some((archive, name_seed, score));
                    }
                }
                Err(error) => last_error = Some(error),
            }
        }
        match selected {
            Some((archive, name_seed, score)) => (archive, Some(name_seed), Some(score)),
            None if name_seeds.is_empty() => {
                return Ok(ArchiveIndex {
                    path: relative_path,
                    entry_count,
                    entries: Vec::new(),
                    status: ArchiveStatus::Encrypted {
                        raw_entry_count: entry_count,
                    },
                    layout: Some(layout),
                    payload_key: None,
                    name_seed: None,
                    name_seed_score: None,
                });
            }
            None => {
                return Ok(malformed_archive(
                    relative_path,
                    entry_count,
                    last_error
                        .map(|error| error.to_string())
                        .unwrap_or_else(|| {
                            "no candidate archive name seed decoded the directory".to_string()
                        }),
                ));
            }
        }
    } else {
        match Archive::parse_directory(&parse_table, file_length, None) {
            Ok(archive) => (archive, None, None),
            Err(error) => {
                return Ok(malformed_archive(
                    relative_path,
                    entry_count,
                    error.to_string(),
                ));
            }
        }
    };
    let entries = parsed
        .entries
        .iter()
        .cloned()
        .map(|entry| {
            let name = decode_archive_name(&entry.name);
            ArchiveResourceEntry {
                name,
                raw_name: entry.name,
                offset: entry.offset,
                size: entry.size,
            }
        })
        .collect();
    Ok(ArchiveIndex {
        path: relative_path,
        entry_count,
        entries,
        status: ArchiveStatus::Indexed,
        layout: Some(layout),
        payload_key: parsed.payload_key(),
        name_seed: selected_name_seed,
        name_seed_score: selected_seed_score,
    })
}

fn parse_fuyin_archive(
    file: &mut File,
    path: &Path,
    relative_path: PathBuf,
    file_length: u64,
) -> RuntimeResult<ArchiveIndex> {
    use cs2_formats::archive::fuyin::{FuyinArchive, FuyinError};
    let parsed = match FuyinArchive::read_directory(file, file_length) {
        Ok(parsed) => parsed,
        Err(FuyinError::Io(source)) => return Err(RuntimeError::Io {
            path: path.to_path_buf(), source,
        }),
        Err(error) => return Ok(malformed_archive(relative_path, 0, error.to_string())),
    };
    let entries = parsed.entries.into_iter().map(|entry| ArchiveResourceEntry {
        name: decode_archive_name(&entry.name), raw_name: entry.name,
        offset: entry.offset, size: entry.size,
    }).collect::<Vec<_>>();
    Ok(ArchiveIndex {
        path: relative_path, entry_count: entries.len() as u32, entries,
        status: ArchiveStatus::Indexed,
        // This field describes a KIF name layout; FUYINPAK has length-prefixed
        // external names, and deliberately does not masquerade as either KIF.
        layout: None, payload_key: None, name_seed: None, name_seed_score: None,
    })
}

fn decode_archive_name(bytes: &[u8]) -> String {
    SHIFT_JIS.decode_without_bom_handling(bytes).0.into_owned()
}

fn archive_name_score(entries: &[ArchiveEntry]) -> i64 {
    const KNOWN_EXTENSIONS: &[&[u8]] = &[
        b"anm", b"bmp", b"cst", b"cstl", b"dat", b"dfn", b"fes", b"font", b"gif", b"hg2", b"hg3",
        b"ini", b"jpg", b"jpeg", b"kcs", b"ogg", b"png", b"ptc", b"ptcl", b"qdb", b"txt", b"wav",
        b"webm", b"wmv", b"wss", b"xml", b"mpg", b"mp4", b"m1v",
    ];
    let mut score = 0_i64;
    let mut scored = 0_i64;
    for entry in entries.iter().take(8192) {
        if entry.name.is_empty() {
            score -= 8;
            scored += 1;
            continue;
        }
        let mut position = 0;
        let mut valid = true;
        while position < entry.name.len() {
            let byte = entry.name[position];
            if byte.is_ascii() {
                if byte.is_ascii_control() {
                    valid = false;
                    break;
                }
                position += 1;
            } else if (0x81..=0x9f).contains(&byte) || (0xe0..=0xfc).contains(&byte) {
                let Some(&trail) = entry.name.get(position + 1) else {
                    valid = false;
                    break;
                };
                if !((0x40..=0x7e).contains(&trail) || (0x80..=0xfc).contains(&trail)) {
                    valid = false;
                    break;
                }
                position += 2;
            } else if (0xa1..=0xdf).contains(&byte) {
                position += 1;
            } else {
                valid = false;
                break;
            }
        }
        if valid {
            let (_, had_errors) = SHIFT_JIS.decode_without_bom_handling(&entry.name);
            valid = !had_errors;
        }
        if valid {
            score += 1;
        } else {
            score -= 12;
        }

        let basename = entry
            .name
            .rsplit(|byte| matches!(*byte, b'/' | b'\\'))
            .next()
            .unwrap_or(&entry.name);
        if let Some(dot) = basename.iter().rposition(|byte| *byte == b'.') {
            let extension = &basename[dot + 1..];
            if KNOWN_EXTENSIONS
                .iter()
                .any(|known| extension.eq_ignore_ascii_case(known))
            {
                score += 24;
            } else if !extension.is_empty()
                && extension.len() <= 8
                && extension.iter().all(u8::is_ascii_alphanumeric)
            {
                score += 2;
            } else {
                score -= 2;
            }
        }
        scored += 1;
    }
    if scored == 0 { i64::MIN } else { score }
}

fn kif_table_is_encrypted(table: &[u8]) -> bool {
    for name_size in [32usize, 64] {
        if let Some(name) = table
            .get(KIF_HEADER_SIZE as usize..KIF_HEADER_SIZE as usize + name_size)
            .and_then(|name| name.split(|byte| *byte == 0).next())
            && name.eq_ignore_ascii_case(b"__key__.dat")
        {
            return true;
        }
    }
    false
}

fn malformed_archive(path: PathBuf, entry_count: u32, reason: impl Into<String>) -> ArchiveIndex {
    ArchiveIndex {
        path,
        entry_count,
        entries: Vec::new(),
        status: ArchiveStatus::Malformed {
            reason: reason.into(),
        },
        layout: None,
        payload_key: None,
        name_seed: None,
        name_seed_score: None,
    }
}

fn read_archive_directory(
    file: &mut File,
    path: &Path,
    archive_length: u64,
) -> RuntimeResult<(Vec<u8>, u32, ArchiveLayout)> {
    let mut header = [0_u8; KIF_HEADER_SIZE as usize];
    file.seek(SeekFrom::Start(0))
        .and_then(|_| file.read_exact(&mut header))
        .map_err(|source| RuntimeError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    if &header[..4] != b"KIF\0" {
        return Err(RuntimeError::Archive("invalid KIF signature".to_string()));
    }
    let entry_count = u32::from_le_bytes(header[4..8].try_into().expect("fixed header"));
    if entry_count > MAX_KIF_ENTRIES {
        return Err(RuntimeError::Archive(format!(
            "entry count exceeds the supported limit of {MAX_KIF_ENTRIES}"
        )));
    }
    let probe_length = usize::try_from(
        archive_length
            .saturating_sub(KIF_HEADER_SIZE)
            .min(KIF_STANDARD_ENTRY_SIZE),
    )
    .map_err(|_| RuntimeError::Archive("KIF header probe is too large".to_string()))?;
    let mut probe = vec![0_u8; KIF_HEADER_SIZE as usize + probe_length];
    probe[..header.len()].copy_from_slice(&header);
    file.read_exact(&mut probe[header.len()..])
        .map_err(|source| RuntimeError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    let legacy_hint = probe
        .get(KIF_HEADER_SIZE as usize..KIF_HEADER_SIZE as usize + 32)
        .is_some_and(|name| {
            name.iter().position(|byte| *byte == 0).is_some()
                && probe
                    .get(40..48)
                    .is_some_and(|metadata| metadata.iter().any(|&byte| byte != 0))
        });
    let encrypted = kif_table_is_encrypted(&probe);
    let layouts = if legacy_hint {
        [
            (KIF_LEGACY_ENTRY_SIZE, ArchiveLayout::Legacy32),
            (KIF_STANDARD_ENTRY_SIZE, ArchiveLayout::Standard64),
        ]
    } else {
        [
            (KIF_STANDARD_ENTRY_SIZE, ArchiveLayout::Standard64),
            (KIF_LEGACY_ENTRY_SIZE, ArchiveLayout::Legacy32),
        ]
    };

    let mut candidates = Vec::new();
    let mut last_error = None;
    for (layout_index, (entry_size, layout)) in layouts.into_iter().enumerate() {
        if encrypted && layout_index != 0 {
            break;
        }
        let table_length = KIF_HEADER_SIZE
            .checked_add(
                (entry_count as u64)
                    .checked_mul(entry_size)
                    .ok_or_else(|| {
                        RuntimeError::Archive("entry table size overflows".to_string())
                    })?,
            )
            .ok_or_else(|| RuntimeError::Archive("entry table size overflows".to_string()))?;
        if table_length > archive_length {
            last_error = Some("entry table extends beyond the archive".to_string());
            continue;
        }
        let table_length = usize::try_from(table_length)
            .map_err(|_| RuntimeError::Archive("entry table is too large".to_string()))?;
        let mut table = vec![0_u8; table_length];
        file.seek(SeekFrom::Start(0))
            .and_then(|_| file.read_exact(&mut table))
            .map_err(|source| RuntimeError::Io {
                path: path.to_path_buf(),
                source,
            })?;
        let parsed = parse_kif_layout(&table, archive_length, layout);
        match parsed {
            Ok(archive) => {
                if !encrypted
                    && archive
                        .entries
                        .iter()
                        .any(|entry| entry.size > 0 && entry.offset < table_length as u64)
                {
                    last_error = Some("KIF resource overlaps its entry table".to_string());
                    continue;
                }
                let score = if encrypted {
                    0
                } else {
                    archive_name_score(&archive.entries)
                };
                candidates.push((score, table, layout));
            }
            Err(error) => last_error = Some(error.to_string()),
        }
    }

    if encrypted {
        return candidates
            .pop()
            .map(|(_, table, layout)| (table, entry_count, layout))
            .ok_or_else(|| {
                RuntimeError::Archive(
                    last_error
                        .unwrap_or_else(|| "hinted encrypted KIF layout is invalid".to_string()),
                )
            });
    }

    candidates.sort_by(|left, right| right.0.cmp(&left.0));
    candidates
        .into_iter()
        .next()
        .map(|(_, table, layout)| (table, entry_count, layout))
        .ok_or_else(|| {
            RuntimeError::Archive(
                last_error.unwrap_or_else(|| "KIF entry table is invalid".to_string()),
            )
        })
}

fn parse_kif_layout(
    table: &[u8],
    archive_length: u64,
    layout: ArchiveLayout,
) -> Result<Archive, cs2_formats::ArchiveError> {
    let encrypted = kif_table_is_encrypted(table);
    let table = match layout {
        ArchiveLayout::Standard64 => table.to_vec(),
        ArchiveLayout::Legacy32 => expand_legacy_kif_table(table)?,
    };
    if encrypted {
        return Archive::parse_raw(&table);
    }
    match layout {
        ArchiveLayout::Standard64 => Archive::parse_directory(&table, archive_length, None),
        ArchiveLayout::Legacy32 => Archive::parse_directory(&table, archive_length, None),
    }
}

fn normalize_kif_table(
    table: &[u8],
    entry_count: u32,
    layout: ArchiveLayout,
) -> RuntimeResult<Vec<u8>> {
    match layout {
        ArchiveLayout::Standard64 => Ok(table.to_vec()),
        ArchiveLayout::Legacy32 => expand_legacy_kif_table_with_count(table, entry_count),
    }
}

fn expand_legacy_kif_table(table: &[u8]) -> Result<Vec<u8>, cs2_formats::ArchiveError> {
    let entry_count = u32::from_le_bytes(
        table
            .get(4..8)
            .ok_or(cs2_formats::ArchiveError::Truncated)?
            .try_into()
            .map_err(|_| cs2_formats::ArchiveError::Truncated)?,
    );
    expand_legacy_kif_table_with_count(table, entry_count)
        .map_err(|_| cs2_formats::ArchiveError::InvalidTable)
}

fn expand_legacy_kif_table_with_count(table: &[u8], entry_count: u32) -> RuntimeResult<Vec<u8>> {
    let expected = KIF_HEADER_SIZE
        .checked_add(u64::from(entry_count).saturating_mul(KIF_LEGACY_ENTRY_SIZE))
        .ok_or_else(|| RuntimeError::Archive("legacy KIF table size overflows".to_string()))?;
    if table.len() as u64 != expected {
        return Err(RuntimeError::Archive(
            "legacy KIF table length does not match its entry count".to_string(),
        ));
    }
    let standard_size = KIF_HEADER_SIZE
        .checked_add(u64::from(entry_count).saturating_mul(KIF_STANDARD_ENTRY_SIZE))
        .ok_or_else(|| RuntimeError::Archive("KIF table size overflows".to_string()))?;
    let standard_size = usize::try_from(standard_size)
        .map_err(|_| RuntimeError::Archive("KIF table is too large".to_string()))?;
    let mut expanded = vec![0_u8; standard_size];
    expanded[..8].copy_from_slice(&table[..8]);
    for index in 0..entry_count as usize {
        let source = 8 + index * KIF_LEGACY_ENTRY_SIZE as usize;
        let destination = 8 + index * KIF_STANDARD_ENTRY_SIZE as usize;
        expanded[destination..destination + 32].copy_from_slice(&table[source..source + 32]);
        expanded[destination + 64..destination + 72]
            .copy_from_slice(&table[source + 32..source + 40]);
    }
    Ok(expanded)
}

fn read_archive_entry(
    file: &mut File,
    path: &Path,
    archive: &Archive,
    entry: &ArchiveEntry,
) -> RuntimeResult<Vec<u8>> {
    let file_length = file
        .metadata()
        .map_err(|source| RuntimeError::Io {
            path: path.to_path_buf(),
            source,
        })?
        .len();
    let end = entry
        .offset
        .checked_add(entry.size)
        .ok_or_else(|| RuntimeError::Archive("resource range overflows".to_string()))?;
    if end > file_length {
        return Err(RuntimeError::Archive(
            "resource points outside the archive".to_string(),
        ));
    }
    let size = usize::try_from(entry.size)
        .map_err(|_| RuntimeError::Archive("resource is too large".to_string()))?;
    if entry.size > MAX_RESOURCE_BYTES {
        return Err(RuntimeError::Archive(format!(
            "resource exceeds the supported {} MiB limit",
            MAX_RESOURCE_BYTES / (1024 * 1024)
        )));
    }
    let mut payload = vec![0_u8; size];
    file.seek(SeekFrom::Start(entry.offset))
        .and_then(|_| file.read_exact(&mut payload))
        .map_err(|source| RuntimeError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    archive.decrypt_payload(&mut payload);
    Ok(payload)
}

fn read_file_range(file: &mut File, path: &Path, offset: u64, size: u64) -> RuntimeResult<Vec<u8>> {
    let file_length = file
        .metadata()
        .map_err(|source| RuntimeError::Io {
            path: path.to_path_buf(),
            source,
        })?
        .len();
    let end = offset
        .checked_add(size)
        .ok_or_else(|| RuntimeError::Archive("resource range overflows".to_string()))?;
    if end > file_length {
        return Err(RuntimeError::Archive(
            "resource points outside the archive".to_string(),
        ));
    }
    let size = usize::try_from(size)
        .map_err(|_| RuntimeError::Archive("resource is too large".to_string()))?;
    if size as u64 > MAX_RESOURCE_BYTES {
        return Err(RuntimeError::Archive(format!(
            "resource exceeds the supported {} MiB limit",
            MAX_RESOURCE_BYTES / (1024 * 1024)
        )));
    }
    let mut payload = vec![0_u8; size];
    file.seek(SeekFrom::Start(offset))
        .and_then(|_| file.read_exact(&mut payload))
        .map_err(|source| RuntimeError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    Ok(payload)
}

fn decrypt_payload_with_key(bytes: &mut [u8], key: Option<u32>) -> RuntimeResult<()> {
    let Some(key) = key else {
        return Ok(());
    };
    let cipher = BlowfishLE::new_from_slice(&key.to_le_bytes())
        .map_err(|_| RuntimeError::Archive("invalid KIF payload key".to_string()))?;
    for block in bytes.chunks_exact_mut(8) {
        cipher.decrypt_block(block.into());
    }
    Ok(())
}

fn names_equal(left: &[u8], right: &[u8]) -> bool {
    let mut left = left;
    let mut right = right;
    while !left.is_empty() && !right.is_empty() {
        let left_lead = matches!(left[0], 0x81..=0x9f | 0xe0..=0xef);
        let right_lead = matches!(right[0], 0x81..=0x9f | 0xe0..=0xef);
        if left_lead || right_lead {
            if left.len() < 2 || right.len() < 2 || left[..2] != right[..2] {
                return false;
            }
            left = &left[2..];
            right = &right[2..];
        } else {
            if !left[0].eq_ignore_ascii_case(&right[0]) {
                return false;
            }
            left = &left[1..];
            right = &right[1..];
        }
    }
    left.is_empty() && right.is_empty()
}

fn config_match(name: &str) -> ConfigMatch {
    let lower = name.to_ascii_lowercase();
    if lower.ends_with("startup.xml") {
        ConfigMatch::Startup
    } else if lower.ends_with("soundconf.xml") {
        ConfigMatch::Sound
    } else if lower.contains("language") || lower.contains("gamelangtbl") {
        ConfigMatch::Language
    } else if lower.ends_with("gamedef.xml") {
        ConfigMatch::GameDefinition
    } else {
        ConfigMatch::Other
    }
}

fn config_kind(kind: ConfigMatch) -> ConfigKind {
    match kind {
        ConfigMatch::Startup => ConfigKind::Startup,
        ConfigMatch::Sound => ConfigKind::Sound,
        ConfigMatch::Language => ConfigKind::Language,
        ConfigMatch::GameDefinition => ConfigKind::GameDefinition,
        ConfigMatch::Other => ConfigKind::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn archive_seed_adapter_reads_an_entry_without_launching_a_host() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("cs2-runtime-resource-{suffix}"));
        fs::create_dir_all(&root).unwrap();
        let archive_path = root.join("scene.int");
        let table_size = 8 + 72;
        let mut bytes = vec![0; table_size + 4];
        bytes[..4].copy_from_slice(b"KIF\0");
        bytes[4..8].copy_from_slice(&1_u32.to_le_bytes());
        bytes[8..8 + 10].copy_from_slice(b"start.cst\0");
        bytes[8 + 64..8 + 68].copy_from_slice(&(table_size as u32).to_le_bytes());
        bytes[8 + 68..8 + 72].copy_from_slice(&4_u32.to_le_bytes());
        bytes[table_size..].copy_from_slice(b"test");
        fs::write(&archive_path, bytes).unwrap();

        let index = ResourceIndex::scan(&root).unwrap();
        assert_eq!(index.read("start.cst").unwrap(), b"test");
        assert_eq!(
            index
                .read_archive_with_name_seed("scene.int", b"START.CST", 0)
                .unwrap(),
            b"test"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn missing_seed_keeps_encrypted_archive_diagnostic_and_scanning_continues() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("cs2-runtime-encrypted-{suffix}"));
        fs::create_dir_all(&root).unwrap();
        let mut bytes = vec![0_u8; 8 + 72 + 8];
        bytes[..4].copy_from_slice(b"KIF\0");
        bytes[4..8].copy_from_slice(&1_u32.to_le_bytes());
        bytes[8..8 + 11].copy_from_slice(b"__key__.dat");
        bytes[8 + 64..8 + 68].copy_from_slice(&(8_u32 + 72).to_le_bytes());
        bytes[8 + 68..8 + 72].copy_from_slice(&8_u32.to_le_bytes());
        fs::write(root.join("encrypted.int"), bytes).unwrap();
        fs::write(root.join("loose.cst"), b"loose script").unwrap();

        let index = ResourceIndex::scan_with_name_seed(&root, None).unwrap();

        assert!(matches!(
            index.archives()[0].status,
            ArchiveStatus::Encrypted { raw_entry_count: 1 }
        ));
        assert!(
            index
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.message.contains("name seed"))
        );
        assert_eq!(index.read("loose.cst").unwrap(), b"loose script");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn legacy_kif_layout_is_indexed_and_all_archive_candidates_are_readable() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("cs2-runtime-legacy-kif-{suffix}"));
        fs::create_dir_all(&root).unwrap();

        let payload = b"legacy payload";
        let table_size = 8 + 40;
        let mut archive = vec![0_u8; table_size + payload.len()];
        archive[..4].copy_from_slice(b"KIF\0");
        archive[4..8].copy_from_slice(&1_u32.to_le_bytes());
        archive[8..8 + 10].copy_from_slice(b"scene.kcs\0");
        archive[8 + 32..8 + 36].copy_from_slice(&(table_size as u32).to_le_bytes());
        archive[8 + 36..8 + 40].copy_from_slice(&(payload.len() as u32).to_le_bytes());
        archive[table_size..].copy_from_slice(payload);
        fs::write(root.join("legacy.int"), archive).unwrap();
        fs::write(root.join("scene.kcs"), b"loose payload").unwrap();

        let index = ResourceIndex::scan(&root).unwrap();
        assert_eq!(index.archives()[0].layout, Some(ArchiveLayout::Legacy32));
        assert_eq!(index.read("scene.kcs").unwrap(), b"loose payload");
        let archive_results = index.read_all_archive_candidates("scene.kcs");
        assert_eq!(archive_results.len(), 1);
        assert_eq!(archive_results[0].1.as_ref().unwrap(), payload);
        assert_eq!(index.archive_index("legacy.int").unwrap().entry_count, 1);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn candidate_reads_preserve_priority_order_and_report_each_source() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("cs2-runtime-candidates-{suffix}"));
        fs::create_dir_all(root.join("config")).unwrap();
        fs::write(root.join("config").join("same.bin"), b"nested").unwrap();
        fs::write(root.join("same.bin"), b"root").unwrap();

        let index = ResourceIndex::scan(&root).unwrap();
        let results = index.read_candidates("same.bin");
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].1.as_ref().unwrap(), b"root");
        assert_eq!(results[1].1.as_ref().unwrap(), b"nested");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn resource_cache_tracks_leases_and_evicts_only_unused_entries() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("cs2-runtime-cache-{suffix}"));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("scene.cst"), b"scene").unwrap();
        let index = ResourceIndex::scan(&root).unwrap();
        let mut cache = ResourceCache::new();

        let first = cache.acquire(&index, "scene.cst").unwrap();
        let second = cache.acquire(&index, "SCENE.CST").unwrap();
        assert_eq!(first.id, second.id);
        assert_eq!(cache.references(first.id), Some(2));
        assert_eq!(cache.bytes(first.id).unwrap().as_slice(), b"scene");

        assert!(!cache.release(first.id).unwrap());
        assert_eq!(cache.evict_unused(), 0);
        assert!(cache.release(second.id).unwrap());
        assert_eq!(cache.evict_unused(), 1);
        assert!(cache.bytes(first.id).is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn decrypts_real_newton_and_nekopara_script_entries() {
        let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(3)
            .expect("runtime crate should be inside the workspace")
            .to_path_buf();
        let samples = [
            workspace_root.join("牛顿与苹果树"),
            workspace_root
                .join("[170526][NEKO WORKs] ネコぱら vol.3 steam版 (files)")
                .join("nekopara_vol3"),
        ];

        for root in samples {
            if !root.is_dir() {
                continue;
            }
            let index = ResourceIndex::scan(&root).expect("sample scan should succeed");
            let name_seed = index
                .archive_name_seed
                .expect("sample runtime executable should provide a name seed");
            let mut checked_script = false;
            for archive in index.archives() {
                if !matches!(archive.status, ArchiveStatus::Indexed) {
                    continue;
                }
                if let Some(script) = archive
                    .entries
                    .iter()
                    .find(|entry| entry.name.to_ascii_lowercase().ends_with(".cst"))
                {
                    let archive_seed = archive.name_seed.unwrap_or(name_seed);
                    let bytes = index
                        .read_archive_with_name_seed(
                            archive.path.to_string_lossy(),
                            &script.raw_name,
                            archive_seed,
                        )
                        .expect("encrypted script entry should decrypt");
                    assert!(!bytes.is_empty());
                    assert!(bytes.starts_with(b"CatScene"));
                    checked_script = true;
                    break;
                }
            }
            assert!(checked_script, "sample should contain an indexed CST entry");
        }
    }
}

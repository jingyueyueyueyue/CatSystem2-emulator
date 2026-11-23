//! Shared immutable font data. The registry owns only weak references: closing
//! the last player releases font bytes, decoded faces, glyphs and text runs.
use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::SystemTime;

use anyhow::Result;
use cs2_renderer::FontRasterizer;
use cs2_runtime::{ResourceCandidate, ResourceIndex, ResourceLocation};

use super::super::perf_detail::{TextStage, TextTimer};

pub(crate) type SharedFace = Arc<OnceLock<Option<Arc<FontRasterizer>>>>;
const MAX_SOURCES: usize = 256;
const MAX_FACES_PER_SOURCE: u32 = 64;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct FontVersion {
    path: PathBuf,
    length: u64,
    modified: Option<SystemTime>,
    created: Option<SystemTime>,
    archive: Option<(u64, u64, Option<u32>, Option<u32>)>,
}

impl FontVersion {
    pub(crate) fn file(path: &Path) -> io::Result<Self> {
        let metadata = path.metadata()?;
        let path = path.canonicalize()?;
        #[cfg(windows)]
        let path = PathBuf::from(path.to_string_lossy().to_lowercase());
        Ok(Self {
            path, length: metadata.len(), modified: metadata.modified().ok(),
            created: metadata.created().ok(), archive: None,
        })
    }

    pub(crate) fn candidate(candidate: &ResourceCandidate) -> io::Result<Self> {
        match &candidate.location {
            ResourceLocation::File { path } => Self::file(path),
            ResourceLocation::Archive { path, offset, size, name_seed, payload_key } => {
                let mut version = Self::file(path)?;
                version.archive = Some((*offset, *size, *name_seed, *payload_key));
                Ok(version)
            }
        }
    }

    pub(crate) fn unchanged(&self, requested_path: &Path) -> io::Result<()> {
        let mut current = Self::file(requested_path)?;
        current.archive = self.archive;
        if current == *self { Ok(()) } else {
            Err(io::Error::other("font source changed while being read"))
        }
    }
}

#[derive(Default)]
struct FaceEntry {
    slot: Weak<OnceLock<Option<Arc<FontRasterizer>>>>,
    decoded: Weak<FontRasterizer>,
}

#[derive(Default)]
struct Entry {
    bytes: Weak<Vec<u8>>,
    faces: BTreeMap<u32, FaceEntry>,
}

#[derive(Default)]
struct Registry {
    sources: BTreeMap<FontVersion, Entry>,
}

fn registry() -> &'static Mutex<Registry> {
    static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
    REGISTRY.get_or_init(Mutex::default)
}

fn cached_bytes<E>(version: FontVersion, read: impl FnOnce() -> std::result::Result<Vec<u8>, E>)
    -> std::result::Result<Arc<Vec<u8>>, E>
{
    // Reads are single-flight across the two text backends and concurrent game
    // opens. Face compilation uses its own per-face OnceLock outside this lock.
    let mut registry = registry().lock().unwrap_or_else(|error| error.into_inner());
    if let Some(bytes) = registry.sources.get(&version).and_then(|entry| entry.bytes.upgrade()) {
        return Ok(bytes);
    }
    registry.sources.retain(|_, entry| entry.bytes.strong_count() != 0);
    let bytes = Arc::new(read()?);
    // No strong byte cache and at most 256 small identity records. An unusually
    // large active font set still loads correctly without growing the registry.
    if registry.sources.len() < MAX_SOURCES {
        registry.sources.insert(version, Entry { bytes: Arc::downgrade(&bytes), ..Entry::default() });
    }
    Ok(bytes)
}

pub(crate) fn shared_font_file(path: &Path) -> io::Result<Arc<Vec<u8>>> {
    let version = FontVersion::file(path)?;
    cached_bytes(version.clone(), || {
        let _read = TextTimer::new(TextStage::FaceRead);
        let bytes = super::read_font_file(path)?;
        version.unchanged(path)?;
        Ok(bytes)
    })
}

pub(crate) fn shared_resource_font(resources: &ResourceIndex, name: &str) -> Result<Arc<Vec<u8>>> {
    let candidate = resolve_candidate(resources, name)?;
    let version = FontVersion::candidate(candidate)?;
    let path = match &candidate.location {
        ResourceLocation::File { path } | ResourceLocation::Archive { path, .. } => path,
    };
    cached_bytes(version.clone(), || -> Result<_> {
        let _read = TextTimer::new(TextStage::FaceRead);
        let bytes = resources.read_candidate_bounded(candidate, FontRasterizer::MAX_FONT_BYTES)?
            .ok_or_else(|| anyhow::anyhow!("font data exceeds the 64 MiB limit"))?;
        version.unchanged(path)?;
        Ok(bytes)
    })
}

pub(crate) fn resource_font_version(resources: &ResourceIndex, name: &str) -> Result<FontVersion> {
    Ok(FontVersion::candidate(resolve_candidate(resources, name)?)?)
}

fn resolve_candidate<'a>(resources: &'a ResourceIndex, name: &str) -> Result<&'a ResourceCandidate> {
    // Match read_entry_with_fallback's routing, including the prohibition on
    // stripping a mounted virtual prefix. Cache by the resolved source, never
    // by the alias spelling or the ResourceIndex allocation's address.
    let mounted = name.split_once('/').is_some_and(|(prefix, _)| {
        resources.resource_router().is_some_and(|router| {
            router.slots.iter().any(|slot| String::from_utf16_lossy(&slot.prefix).eq_ignore_ascii_case(prefix))
                || router.groups.iter().any(|group| String::from_utf16_lossy(&group.prefix).eq_ignore_ascii_case(prefix))
        })
    });
    let mut names = vec![name.to_string()];
    if !mounted {
        if !name.starts_with("system/") && !name.starts_with("system\\") {
            names.push(format!("system/{name}"));
        }
        let basename = name.rsplit(['/', '\\']).next().unwrap_or(name);
        if basename != name {
            names.extend([basename.to_owned(), format!("system/{basename}"), format!("system/config/{basename}")]);
        }
    }
    for name in names {
        if let Some(candidate) = resources.resolve(&name)? { return Ok(candidate); }
    }
    Err(anyhow::anyhow!("font resource is unavailable: {name}"))
}

pub(crate) fn shared_font_face(bytes: &Arc<Vec<u8>>, index: u32) -> SharedFace {
    let mut registry = registry().lock().unwrap_or_else(|error| error.into_inner());
    if index < MAX_FACES_PER_SOURCE {
        if let Some(entry) = registry.sources.values_mut()
            .find(|entry| std::ptr::eq(entry.bytes.as_ptr(), Arc::as_ptr(bytes)))
        {
            let cached = entry.faces.entry(index).or_default();
            if let Some(face) = cached.slot.upgrade() { return face; }
            let face = Arc::new(OnceLock::new());
            if let Some(decoded) = cached.decoded.upgrade() { let _ = face.set(Some(decoded)); }
            cached.slot = Arc::downgrade(&face);
            return face;
        }
    }
    Arc::new(OnceLock::new())
}

pub(crate) fn prepare_font_face(bytes: &Arc<Vec<u8>>, index: u32, face: &SharedFace) -> Option<Arc<FontRasterizer>> {
    let prepared = face.get_or_init(|| {
        let _decode = TextTimer::new(TextStage::FaceDecode);
        FontRasterizer::from_shared_bytes(Arc::clone(bytes), index).ok().map(Arc::new)
    }).clone();
    if let Some(font) = &prepared {
        let mut registry = registry().lock().unwrap_or_else(|error| error.into_inner());
        if let Some(entry) = registry.sources.values_mut()
            .find(|entry| std::ptr::eq(entry.bytes.as_ptr(), Arc::as_ptr(bytes)))
        {
            if let Some(cached) = entry.faces.get_mut(&index) { cached.decoded = Arc::downgrade(font); }
        }
    }
    prepared
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::UNIX_EPOCH;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
            let path = std::env::temp_dir().join(format!("cs2-font-version-{nonce}"));
            std::fs::write(&path, b"not a font").unwrap();
            Self(path)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) { let _ = std::fs::remove_file(&self.0); }
    }

    #[test]
    fn archive_range_and_decryption_settings_are_part_of_font_identity() {
        let fixture = Fixture::new();
        let candidate = |offset, size, name_seed, payload_key| ResourceCandidate {
            name: "font/face.ttf".into(), size, priority: 0,
            location: ResourceLocation::Archive { path: fixture.0.clone(), offset, size, name_seed, payload_key },
        };
        let base = FontVersion::candidate(&candidate(0, 8, Some(1), Some(2))).unwrap();
        for other in [candidate(1, 8, Some(1), Some(2)), candidate(0, 9, Some(1), Some(2)),
            candidate(0, 8, Some(3), Some(2)), candidate(0, 8, Some(1), Some(4))]
        {
            assert_ne!(base, FontVersion::candidate(&other).unwrap());
        }
        assert_ne!(base, FontVersion::file(&fixture.0).unwrap());
    }

    #[test]
    fn failed_face_is_shared_and_registry_does_not_keep_font_data_alive() {
        let fixture = Fixture::new();
        let bytes = shared_font_file(&fixture.0).unwrap();
        let first = shared_font_face(&bytes, 0);
        assert!(prepare_font_face(&bytes, 0, &first).is_none());
        let second_bytes = shared_font_file(&fixture.0).unwrap();
        let second = shared_font_face(&second_bytes, 0);
        assert!(Arc::ptr_eq(&bytes, &second_bytes));
        assert!(Arc::ptr_eq(&first, &second));
        assert!(second.get().is_some_and(Option::is_none));
        let released = Arc::downgrade(&bytes);
        drop((first, second, bytes, second_bytes));
        assert!(released.upgrade().is_none(), "weak registry must release bytes with the last user");
    }

    #[test]
    fn timestamp_change_invalidates_same_length_font_data() {
        let fixture = Fixture::new();
        let first = shared_font_file(&fixture.0).unwrap();
        let file = std::fs::OpenOptions::new().write(true).open(&fixture.0).unwrap();
        let changed = file.metadata().unwrap().modified().unwrap() + std::time::Duration::from_secs(2);
        file.set_modified(changed).unwrap();
        let second = shared_font_file(&fixture.0).unwrap();
        assert_eq!(first.as_slice(), second.as_slice());
        assert!(!Arc::ptr_eq(&first, &second));
    }
}

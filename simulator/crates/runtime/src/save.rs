use crate::host::SaveHost;
use crate::{RuntimeError, RuntimeResult, RuntimeState};
use serde::{Deserialize, Serialize};
use std::fs::{self, FileTimes, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

const SAVE_MAGIC: &str = "CS2RUNTIME";
const SAVE_VERSION: u32 = 1;
const MAX_SLOT: u32 = 999;
const SAVE_FILE_EXTENSION: &str = "cs2save";
const TEMP_SAVE_MARKER: &str = ".tmp-";
const MAX_SAVE_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum SaveKind {
    Ordinary(u32),
    Quick,
}

pub trait SaveRepository {
    fn write(&mut self, kind: SaveKind, state: &RuntimeState) -> RuntimeResult<()>;
    fn read(&mut self, kind: SaveKind) -> RuntimeResult<Option<RuntimeState>>;
    fn delete_kind(&mut self, kind: SaveKind) -> RuntimeResult<()>;
    fn list(&mut self) -> RuntimeResult<Vec<u32>>;

    fn read_for_restart(&mut self, kind: SaveKind) -> RuntimeResult<Option<RuntimeState>> {
        let Some(mut state) = self.read(kind)? else {
            return Ok(None);
        };
        state.recover_after_restart()?;
        Ok(Some(state))
    }
}

#[derive(Debug, Clone)]
pub struct FileSaveStore {
    root: PathBuf,
}

impl FileSaveStore {
    pub fn open(root: impl AsRef<Path>) -> RuntimeResult<Self> {
        let root = root.as_ref().to_path_buf();
        fs::create_dir_all(&root).map_err(|source| RuntimeError::Io {
            path: root.clone(),
            source,
        })?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn save(&mut self, kind: SaveKind, state: &RuntimeState) -> RuntimeResult<()> {
        self.write(kind, state)
    }

    pub fn load(&mut self, kind: SaveKind) -> RuntimeResult<Option<RuntimeState>> {
        self.read(kind)
    }

    pub fn load_for_restart(&mut self, kind: SaveKind) -> RuntimeResult<Option<RuntimeState>> {
        SaveRepository::read_for_restart(self, kind)
    }

    pub fn delete(&mut self, kind: SaveKind) -> RuntimeResult<()> {
        self.delete_kind(kind)
    }

    fn path_for(&self, kind: SaveKind) -> RuntimeResult<PathBuf> {
        match kind {
            SaveKind::Ordinary(slot) if slot <= MAX_SLOT => {
                Ok(self.root.join(ordinary_filename(slot)))
            }
            SaveKind::Ordinary(slot) => Err(RuntimeError::InvalidSaveSlot(slot)),
            SaveKind::Quick => Ok(self.root.join(format!("quicksave.{SAVE_FILE_EXTENSION}"))),
        }
    }
}

impl SaveRepository for FileSaveStore {
    fn write(&mut self, kind: SaveKind, state: &RuntimeState) -> RuntimeResult<()> {
        state.validate()?;
        let payload = serde_json::to_vec(state)
            .map_err(|error| RuntimeError::SaveSerialization(error.to_string()))?;
        let envelope = SaveEnvelope {
            magic: SAVE_MAGIC.to_string(),
            version: SAVE_VERSION,
            kind,
            checksum: checksum(&payload),
            payload,
        };
        let bytes = serde_json::to_vec(&envelope)
            .map_err(|error| RuntimeError::SaveSerialization(error.to_string()))?;
        if bytes.len() as u64 > MAX_SAVE_BYTES {
            return Err(RuntimeError::InvalidSave("serialized save exceeds 512 MiB".into()));
        }
        let path = self.path_for(kind)?;
        write_private_file(&path, &bytes)
    }

    fn read(&mut self, kind: SaveKind) -> RuntimeResult<Option<RuntimeState>> {
        let path = self.path_for(kind)?;
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(RuntimeError::Io { path, source });
            }
        };
        if metadata.file_type().is_symlink() {
            return Err(RuntimeError::InvalidSave(
                "save path must not be a symbolic link".to_string(),
            ));
        }
        if !metadata.is_file() || metadata.len() > MAX_SAVE_BYTES {
            return Err(RuntimeError::InvalidSave("save is not a bounded regular file".into()));
        }
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(RuntimeError::Io { path, source });
            }
        };
        let envelope: SaveEnvelope = serde_json::from_slice(&bytes)
            .map_err(|error| RuntimeError::InvalidSave(format!("invalid envelope: {error}")))?;
        if envelope.magic != SAVE_MAGIC {
            return Err(RuntimeError::InvalidSave("invalid save magic".to_string()));
        }
        if envelope.version != SAVE_VERSION {
            return Err(RuntimeError::InvalidSave(format!(
                "unsupported save version {}",
                envelope.version
            )));
        }
        if envelope.kind != kind {
            return Err(RuntimeError::InvalidSave(
                "save kind does not match requested slot".to_string(),
            ));
        }
        if checksum(&envelope.payload) != envelope.checksum {
            return Err(RuntimeError::InvalidSave(
                "save checksum mismatch".to_string(),
            ));
        }
        let state: RuntimeState = serde_json::from_slice(&envelope.payload)
            .map_err(|error| RuntimeError::InvalidSave(format!("invalid state: {error}")))?;
        state.validate()?;
        Ok(Some(state))
    }

    fn delete_kind(&mut self, kind: SaveKind) -> RuntimeResult<()> {
        let path = self.path_for(kind)?;
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(RuntimeError::Io { path, source }),
        }
    }

    fn list(&mut self) -> RuntimeResult<Vec<u32>> {
        let mut slots = Vec::new();
        let entries = fs::read_dir(&self.root).map_err(|source| RuntimeError::Io {
            path: self.root.clone(),
            source,
        })?;
        for entry in entries {
            let entry = entry.map_err(|source| RuntimeError::Io {
                path: self.root.clone(),
                source,
            })?;
            if !entry
                .file_type()
                .map_err(|source| RuntimeError::Io {
                    path: entry.path(),
                    source,
                })?
                .is_file()
            {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            let Some(slot) = name
                .strip_prefix("slot_")
                .and_then(|name| name.strip_suffix(&format!(".{SAVE_FILE_EXTENSION}")))
                .and_then(|name| name.parse::<u32>().ok())
            else {
                continue;
            };
            if slot > MAX_SLOT || name != ordinary_filename(slot) {
                continue;
            }
            match self.read(SaveKind::Ordinary(slot)) {
                Ok(Some(_)) => slots.push(slot),
                Ok(None) | Err(RuntimeError::InvalidSave(_)) => {}
                Err(error) => return Err(error),
            }
        }
        slots.sort_unstable();
        Ok(slots)
    }
}

impl SaveHost for FileSaveStore {
    fn save(&mut self, slot: u32, state: &RuntimeState) -> RuntimeResult<()> {
        self.write(SaveKind::Ordinary(slot), state)
    }

    fn load(&mut self, slot: u32) -> RuntimeResult<Option<RuntimeState>> {
        self.read(SaveKind::Ordinary(slot))
    }

    fn delete(&mut self, slot: u32) -> RuntimeResult<()> {
        self.delete_kind(SaveKind::Ordinary(slot))
    }

    fn list_slots(&mut self) -> RuntimeResult<Vec<u32>> {
        self.list()
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct SaveEnvelope {
    magic: String,
    version: u32,
    kind: SaveKind,
    checksum: u64,
    payload: Vec<u8>,
}

fn ordinary_filename(slot: u32) -> String {
    format!("slot_{slot:03}.{SAVE_FILE_EXTENSION}")
}

/// Replace a private file only after its new contents have been written.
pub fn write_private_file(path: &Path, bytes: &[u8]) -> RuntimeResult<()> {
    replace_private_file(path, bytes, None)
}

/// Replace a file while retaining its previous modification time.
pub fn write_private_file_with_modified_time(
    path: &Path, bytes: &[u8], modified: SystemTime,
) -> RuntimeResult<()> {
    replace_private_file(path, bytes, Some(modified))
}

fn replace_private_file(
    path: &Path, bytes: &[u8], modified: Option<SystemTime>,
) -> RuntimeResult<()> {
    let temporary = temporary_path(path);
    write_temporary(&temporary, bytes, modified).map_err(|source| RuntimeError::Io {
        path: temporary.clone(),
        source,
    })?;
    if let Err(source) = replace_file(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(RuntimeError::Io { path: path.to_path_buf(), source });
    }
    Ok(())
}

fn temporary_path(path: &Path) -> PathBuf {
    static TEMPORARY_COUNTER: AtomicU64 = AtomicU64::new(0);

    let sequence = TEMPORARY_COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut filename = path
        .file_name()
        .expect("save paths always have a file name")
        .to_os_string();
    filename.push(format!(
        "{TEMP_SAVE_MARKER}{}-{sequence}",
        std::process::id()
    ));
    path.with_file_name(filename)
}

fn write_temporary(
    path: &Path, bytes: &[u8], modified: Option<SystemTime>,
) -> std::io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    let result = (|| {
        file.write_all(bytes)?;
        if let Some(time) = modified {
            file.set_times(FileTimes::new().set_modified(time))?;
        }
        file.sync_all()
    })();
    drop(file);
    if result.is_err() {
        let _ = fs::remove_file(path);
    }
    result
}

fn checksum(bytes: &[u8]) -> u64 {
    let mut value = 0xcbf29ce484222325_u64;
    for byte in bytes {
        value ^= u64::from(*byte);
        value = value.wrapping_mul(0x100000001b3);
    }
    value
}

#[cfg(not(windows))]
fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    fs::rename(source, destination)
}

#[cfg(windows)]
fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;

    const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(existing: *const u16, new: *const u16, flags: u32) -> i32;
    }

    let source = source
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let result = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TempDirectory {
        path: PathBuf,
    }

    impl TempDirectory {
        fn new() -> Self {
            let suffix = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock must be after epoch")
                .as_nanos();
            let path = std::env::temp_dir()
                .join(format!("cs2-runtime-save-{}-{suffix}", std::process::id()));
            fs::create_dir_all(&path).expect("temporary save directory should be created");
            Self { path }
        }

        fn store(&self) -> FileSaveStore {
            FileSaveStore::open(&self.path).expect("temporary save store should open")
        }
    }

    impl Drop for TempDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn state_with_label(label: &str, frame: u64) -> RuntimeState {
        let mut state = RuntimeState::default();
        state.scene.current_label = Some(label.to_string());
        state.clock.frame = frame;
        state
    }

    fn temporary_files(root: &Path) -> Vec<PathBuf> {
        fs::read_dir(root)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.contains(TEMP_SAVE_MARKER))
            })
            .collect()
    }

    #[test]
    fn ordinary_and_quick_saves_round_trip_across_restart() {
        let temp = TempDirectory::new();
        let mut store = temp.store();
        let ordinary = state_with_label("ordinary", 2);
        let quick = state_with_label("quick", 3);

        store.write(SaveKind::Ordinary(2), &ordinary).unwrap();
        store.write(SaveKind::Quick, &quick).unwrap();
        assert_eq!(store.list().unwrap(), vec![2]);

        let overwritten = state_with_label("overwritten", 4);
        store.write(SaveKind::Ordinary(2), &overwritten).unwrap();
        assert_eq!(
            store.read(SaveKind::Ordinary(2)).unwrap(),
            Some(overwritten.clone())
        );
        assert!(temporary_files(&temp.path).is_empty());

        drop(store);
        let mut restarted = temp.store();
        assert_eq!(
            restarted.read(SaveKind::Ordinary(2)).unwrap(),
            Some(overwritten)
        );
        assert_eq!(restarted.read(SaveKind::Quick).unwrap(), Some(quick));

        restarted.delete(SaveKind::Ordinary(2)).unwrap();
        restarted.delete(SaveKind::Ordinary(2)).unwrap();
        restarted.delete(SaveKind::Quick).unwrap();
        assert_eq!(restarted.list().unwrap(), Vec::<u32>::new());
        assert_eq!(restarted.read(SaveKind::Ordinary(2)).unwrap(), None);
        assert_eq!(restarted.read(SaveKind::Quick).unwrap(), None);
    }

    #[test]
    fn load_for_restart_clears_transient_state_and_rearms_tasks() {
        let temp = TempDirectory::new();
        let mut store = temp.store();
        let mut state = state_with_label("restartable", 4);
        let task = state.tasks.schedule("resume", 4, None).unwrap();
        assert_eq!(state.tasks.poll_ready(4), vec![task]);
        state.apply_input(crate::InputEvent::KeyDown(crate::InputKey::Keyboard(13)));
        let object = state.allocate_object_handle().unwrap();

        store.write(SaveKind::Quick, &state).unwrap();
        let restored = store.load_for_restart(SaveKind::Quick).unwrap().unwrap();

        assert!(!restored.input.key_pressed(13));
        assert!(!restored.object_handles.contains(object));
        assert_eq!(
            restored.tasks.status(task).unwrap(),
            crate::TaskStatus::Ready
        );
        assert_eq!(restored.scene.current_label.as_deref(), Some("restartable"));
    }

    #[test]
    fn list_reports_only_valid_canonical_ordinary_slots() {
        let temp = TempDirectory::new();
        let mut store = temp.store();
        store
            .write(SaveKind::Ordinary(1), &state_with_label("one", 1))
            .unwrap();
        store
            .write(SaveKind::Ordinary(7), &state_with_label("seven", 7))
            .unwrap();

        fs::write(temp.path.join("slot_002.cs2save"), b"corrupt").unwrap();
        fs::write(temp.path.join("slot_1000.cs2save"), b"out of range").unwrap();
        fs::write(temp.path.join("slot_1.cs2save"), b"alias").unwrap();
        fs::create_dir(temp.path.join("slot_003.cs2save")).unwrap();

        assert_eq!(store.list().unwrap(), vec![1, 7]);
    }

    #[test]
    fn checksum_corruption_is_rejected() {
        let temp = TempDirectory::new();
        let mut store = temp.store();
        store
            .write(SaveKind::Ordinary(1), &RuntimeState::default())
            .unwrap();
        let path = store.path_for(SaveKind::Ordinary(1)).unwrap();
        let mut envelope: SaveEnvelope = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        envelope.payload[0] ^= 1;
        fs::write(&path, serde_json::to_vec(&envelope).unwrap()).unwrap();

        assert!(matches!(
            store.read(SaveKind::Ordinary(1)),
            Err(RuntimeError::InvalidSave(message)) if message == "save checksum mismatch"
        ));
    }

    #[test]
    fn unsupported_version_and_mismatched_kind_are_rejected() {
        let temp = TempDirectory::new();
        let mut store = temp.store();
        let state = state_with_label("versioned", 1);
        let payload = serde_json::to_vec(&state).unwrap();
        let path = store.path_for(SaveKind::Ordinary(1)).unwrap();

        let mut envelope = SaveEnvelope {
            magic: SAVE_MAGIC.to_string(),
            version: SAVE_VERSION + 1,
            kind: SaveKind::Ordinary(1),
            checksum: checksum(&payload),
            payload: payload.clone(),
        };
        fs::write(&path, serde_json::to_vec(&envelope).unwrap()).unwrap();
        assert!(matches!(
            store.read(SaveKind::Ordinary(1)),
            Err(RuntimeError::InvalidSave(message))
                if message == format!("unsupported save version {}", SAVE_VERSION + 1)
        ));

        envelope.version = SAVE_VERSION;
        envelope.kind = SaveKind::Quick;
        fs::write(&path, serde_json::to_vec(&envelope).unwrap()).unwrap();
        assert!(matches!(
            store.read(SaveKind::Ordinary(1)),
            Err(RuntimeError::InvalidSave(message))
                if message == "save kind does not match requested slot"
        ));
    }

    #[test]
    fn invalid_magic_and_state_payload_are_rejected() {
        let temp = TempDirectory::new();
        let mut store = temp.store();
        let path = store.path_for(SaveKind::Ordinary(3)).unwrap();
        let mut envelope = SaveEnvelope {
            magic: "OTHER".to_string(),
            version: SAVE_VERSION,
            kind: SaveKind::Ordinary(3),
            checksum: checksum(b"{}"),
            payload: b"{}".to_vec(),
        };
        fs::write(&path, serde_json::to_vec(&envelope).unwrap()).unwrap();
        assert!(matches!(
            store.read(SaveKind::Ordinary(3)),
            Err(RuntimeError::InvalidSave(message)) if message == "invalid save magic"
        ));

        envelope.magic = SAVE_MAGIC.to_string();
        envelope.payload = b"not a runtime state".to_vec();
        envelope.checksum = checksum(&envelope.payload);
        fs::write(&path, serde_json::to_vec(&envelope).unwrap()).unwrap();
        assert!(matches!(
            store.read(SaveKind::Ordinary(3)),
            Err(RuntimeError::InvalidSave(message)) if message.starts_with("invalid state:")
        ));
    }

    #[test]
    fn nonzero_runtime_schema_versions_round_trip_without_schema_changes() {
        let temp = TempDirectory::new();
        let mut store = temp.store();
        let mut state = state_with_label("future schema", 5);
        state.schema_version = 2;

        store.write(SaveKind::Ordinary(8), &state).unwrap();

        assert_eq!(store.read(SaveKind::Ordinary(8)).unwrap(), Some(state));
    }

    #[test]
    fn zero_schema_is_rejected_without_replacing_existing_save() {
        let temp = TempDirectory::new();
        let mut store = temp.store();
        let path = store.path_for(SaveKind::Ordinary(4)).unwrap();
        let existing = state_with_label("existing", 4);
        store.write(SaveKind::Ordinary(4), &existing).unwrap();

        let mut invalid = existing.clone();
        invalid.schema_version = 0;
        assert!(matches!(
            store.write(SaveKind::Ordinary(4), &invalid),
            Err(RuntimeError::InvalidSave(message))
                if message == "runtime state schema version is zero"
        ));
        assert_eq!(store.read(SaveKind::Ordinary(4)).unwrap(), Some(existing));
        assert!(path.exists());
        assert!(temporary_files(&temp.path).is_empty());
    }

    #[test]
    fn ordinary_slot_paths_are_bounded_and_canonical() {
        let temp = TempDirectory::new();
        let store = temp.store();
        let path = store.path_for(SaveKind::Ordinary(42)).unwrap();

        assert_eq!(path, temp.path.join("slot_042.cs2save"));
        assert_eq!(path.parent(), Some(temp.path.as_path()));
        assert!(matches!(
            store.path_for(SaveKind::Ordinary(MAX_SLOT + 1)),
            Err(RuntimeError::InvalidSaveSlot(slot)) if slot == MAX_SLOT + 1
        ));
        assert!(matches!(
            store.path_for(SaveKind::Ordinary(u32::MAX)),
            Err(RuntimeError::InvalidSaveSlot(slot)) if slot == u32::MAX
        ));
    }

    #[test]
    fn failed_replacement_preserves_destination_and_cleans_temporary_file() {
        let temp = TempDirectory::new();
        let mut store = temp.store();
        let path = store.path_for(SaveKind::Ordinary(6)).unwrap();
        fs::create_dir(&path).unwrap();

        assert!(matches!(
            store.write(SaveKind::Ordinary(6), &RuntimeState::default()),
            Err(RuntimeError::Io { .. })
        ));
        assert!(path.is_dir());
        assert!(temporary_files(&temp.path).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_save_paths_are_rejected() {
        use std::os::unix::fs::symlink;

        let temp = TempDirectory::new();
        let outside = temp
            .path
            .parent()
            .unwrap()
            .join(format!("cs2-runtime-save-outside-{}", std::process::id()));
        fs::write(&outside, b"outside").unwrap();
        let path = temp.path.join("slot_005.cs2save");
        symlink(&outside, &path).unwrap();

        let mut store = temp.store();
        assert!(matches!(
            store.read(SaveKind::Ordinary(5)),
            Err(RuntimeError::InvalidSave(message))
                if message == "save path must not be a symbolic link"
        ));
        fs::remove_file(outside).unwrap();
    }
}

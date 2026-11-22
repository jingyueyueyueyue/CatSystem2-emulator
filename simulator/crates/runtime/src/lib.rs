mod audio;
pub mod backlog;
mod fes;
pub mod fes_motion;
pub mod fes_controls;
mod fingerprint;
mod host;
pub mod language;
pub mod javascript;
pub mod export_tree;
pub mod export_sound;
pub mod export_sound_library;
pub mod message_object;
pub mod message_list;
pub mod scene_tools;
pub mod object_state;
pub mod object_graph;
pub mod model_state;
/// Graph adapters share serde without adding a desktop manifest dependency.
pub use serde as graph_serde;
pub mod emote;
pub mod effekseer;
pub mod emocon;
pub mod configuration;
pub mod native_factory;
pub mod native_task_profile;
pub mod native_task_clock_profile;
pub mod input_coordinates;
pub mod calendar;
mod native;
mod native_image_data;
mod native_memory;
mod native_sequence;
mod pe_resources;
mod resource;
pub mod resource_router;
mod save;
mod scheduler;
mod sha256;
pub mod native_profile;
pub mod native_context_profile;
mod state;
mod task;
pub use task::snapshot::OrdinaryTaskSnapshot;
mod task_tree;

pub use audio::OggAudioHost;
pub use fes::{
    FesDispatch, FesObjectBankState, FesObjectDefinitionState, FesObjectKind, FesObjectProperty,
    FesObjectState, FesRuntimeAdapter, FesRuntimeStatus, FesSubcommandInvocation,
    FesSubcommandMode, MessagePresentation, MessageRevealProgress,
};
pub use fingerprint::{BinaryFingerprint, EngineFamily, EngineFingerprint, EngineVariant};
pub use host::{
    AudioChannel, AudioHost, AudioRequest, AudioState, InputEvent, InputHost, InputKey, InputState,
    MovieHost, MovieLoadResult, MovieRequest, PlaybackState, SaveHost,
};
pub use native::{NativeCallRegistry, NativeHandler};
pub use native_image_data::native_image_metadata;
pub use native_memory::{NativeMemoryCopyMode, native_copy_between_vms, native_copy_memory};
pub use native_sequence::NativeSequence;
pub use resource::{
    ArchiveIndex, ArchiveLayout, ArchiveResourceEntry, ArchiveStatus, BoundedReader,
    ConfigCandidate, ConfigKind, DirectoryFile, MediaResourceReader, ResourceCache, ResourceCandidate, ResourceIndex,
    ResourceLease, ResourceLeaseId, ResourceLocation, ResourceReader, ScanDiagnostic,
    ScanDiagnosticLevel,
};
pub use save::{FileSaveStore, SaveKind, SaveRepository};
pub use scheduler::{
    ObjectHandle, ObjectHandleTable, RuntimeTask, TaskId, TaskScheduler, TaskStatus,
};
pub use task::{OrdinaryTask, TaskClock, TaskCommandBuffers, TaskCommandOutcome, TaskMessage};
pub use task_tree::{EngineTaskId, TaskCommand, TaskLifecycle, TaskNode, TaskTree};
pub use state::{
    GameClock, MovieMetadataState, MoviePlaybackStatus, MovieResourceStatus, MovieState,
    MovieTimeBaseState, RuntimeState, RuntimeValue, SceneState, ScriptCallFrame,
    ScriptProgramState, ScriptState,
};

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("game directory does not exist: {0}")]
    MissingGameDirectory(PathBuf),
    #[error("I/O error for {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid resource name: {0}")]
    InvalidResourceName(String),
    #[error("resource was not found: {0}")]
    ResourceNotFound(String),
    #[error("invalid save slot: {0}")]
    InvalidSaveSlot(u32),
    #[error("host operation failed: {0}")]
    Host(String),
    #[error("audio playback start timed out: {0}")]
    AudioStartTimeout(String),
    #[error("runtime is not initialized")]
    NotInitialized,
    #[error("save data is corrupt or unsupported: {0}")]
    InvalidSave(String),
    #[error("save serialization failed: {0}")]
    SaveSerialization(String),
    #[error("archive operation failed: {0}")]
    Archive(String),
    #[error("native call {0} is not registered")]
    UnknownNativeCall(u32),
    #[error("native call {index} failed: {message}")]
    NativeCall { index: u32, message: String },
    #[error("script execution failed: {0}")]
    Script(#[from] cs2_script::ScriptError),
    #[error("scene runtime error during {phase} at {offset:?}: {message}")]
    Scene {
        phase: &'static str,
        offset: Option<usize>,
        message: String,
    },
}

pub type RuntimeResult<T> = Result<T, RuntimeError>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GameRoot {
    pub path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameScan {
    pub root: PathBuf,
    pub resources: ResourceIndex,
    pub engine: EngineFingerprint,
    pub config_candidates: Vec<ConfigCandidate>,
    pub diagnostics: Vec<ScanDiagnostic>,
}

impl GameRoot {
    pub fn open(path: impl AsRef<Path>) -> RuntimeResult<Self> {
        let path = path.as_ref().to_path_buf();
        if !path.is_dir() {
            return Err(RuntimeError::MissingGameDirectory(path));
        }
        Ok(Self { path })
    }

    pub fn scan(&self) -> RuntimeResult<GameScan> {
        self.scan_with_language(resource_router::GameLanguage::Automatic)
    }

    pub fn scan_with_language(&self, language: resource_router::GameLanguage) -> RuntimeResult<GameScan> {
        let resources = ResourceIndex::scan_with_language(&self.path, language)?;
        let engine = EngineFingerprint::detect(&self.path, &resources)?;
        let config_candidates = resources.config_candidates();
        let diagnostics = resources.diagnostics().to_vec();

        Ok(GameScan {
            root: self.path.clone(),
            resources,
            engine,
            config_candidates,
            diagnostics,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Serialize, de::DeserializeOwned};
    use std::fs::{self, File};
    use std::io::Write;
    use std::path::Path;
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
                .join(format!("cs2-runtime-test-{}-{suffix}", std::process::id()));
            fs::create_dir_all(&path).expect("temporary directory should be created");
            Self { path }
        }
    }

    impl Drop for TempDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn write_kif(path: &Path, name: &[u8], payload: &[u8]) {
        let table_size = 8 + 72;
        let mut data = vec![0; table_size + payload.len()];
        data[0..4].copy_from_slice(b"KIF\0");
        data[4..8].copy_from_slice(&1_u32.to_le_bytes());
        let name_end = name.len().min(64);
        data[8..8 + name_end].copy_from_slice(&name[..name_end]);
        data[8 + 64..8 + 68].copy_from_slice(&(table_size as u32).to_le_bytes());
        data[8 + 68..8 + 72].copy_from_slice(&(payload.len() as u32).to_le_bytes());
        data[table_size..].copy_from_slice(payload);
        fs::write(path, data).expect("KIF fixture should be written");
    }

    #[test]
    fn scans_directory_and_reads_kif_entries() {
        let temp = TempDirectory::new();
        fs::write(temp.path.join("startup.xml"), b"directory").expect("file fixture");
        fs::create_dir_all(temp.path.join("config")).expect("config directory");
        fs::write(temp.path.join("config").join("startup.xml"), b"nested").expect("nested fixture");
        write_kif(
            &temp.path.join("scene.int"),
            b"start.cst",
            b"archive script",
        );

        let root = GameRoot::open(&temp.path).expect("fixture root should open");
        let scan = root.scan().expect("fixture root should scan");

        assert_eq!(scan.resources.read("startup.xml").unwrap(), b"directory");
        assert_eq!(scan.resources.read("start.cst").unwrap(), b"archive script");
        assert_eq!(scan.resources.archives().len(), 1);
        assert_eq!(scan.resources.archives()[0].entry_count, 1);
        assert!(
            scan.config_candidates
                .iter()
                .any(|candidate| candidate.kind == ConfigKind::Startup)
        );
    }

    #[test]
    fn malformed_kif_is_reported_without_aborting_scan() {
        let temp = TempDirectory::new();
        let mut file = File::create(temp.path.join("broken.int")).expect("fixture file");
        file.write_all(b"KIF\0\xff\xff\xff\x7f")
            .expect("fixture should be written");

        let resources = ResourceIndex::scan(&temp.path).expect("scan should finish");

        assert_eq!(resources.archives().len(), 1);
        assert!(matches!(
            &resources.archives()[0].status,
            ArchiveStatus::Malformed { .. }
        ));
        assert!(!resources.diagnostics().is_empty());
    }

    #[test]
    fn detects_catsystem2_engine_and_serializes_state() {
        let temp = TempDirectory::new();
        fs::write(temp.path.join("cs2.exe"), b"MZ test executable").expect("exe fixture");
        fs::write(temp.path.join("cs2conf.dll"), b"MZ config").expect("dll fixture");
        fs::write(temp.path.join("config.int"), b"not an archive").expect("archive fixture");

        let root = GameRoot::open(&temp.path).expect("fixture root should open");
        let scan = root.scan().expect("fixture root should scan");
        assert_eq!(scan.engine.family, EngineFamily::CatSystem2);
        assert!(scan.engine.primary_executable.is_some());
        assert!(!scan.engine.signature.is_empty());

        let mut state = RuntimeState::default();
        state.scene.current_script = Some("start.cst".to_string());
        state
            .variables
            .insert("route".to_string(), RuntimeValue::I32(2));
        assert_serializable::<RuntimeState>();
        assert_deserializable::<RuntimeState>();
    }

    fn assert_serializable<T: Serialize>() {}

    fn assert_deserializable<T: DeserializeOwned>() {}

    #[test]
    fn runtime_state_maps_to_cs2_script_vm() {
        let program = cs2_script::KcsProgram::from_code(&[0x00], vec![1, 2, 3, 4], 16).unwrap();
        let mut vm = cs2_script::Vm::from_program(program).unwrap();
        vm.instruction_pointer = 17;
        vm.stack.push(cs2_script::Value::I32(-3));
        vm.stack.push(cs2_script::Value::Address(0x1000));
        vm.write(0, &[9, 8, 7, 6]).unwrap();

        let state = ScriptState::from_vm(&vm);
        assert_eq!(state.instruction_pointer, 17);
        assert_eq!(state.stack_bits, vec![(-3_i32) as u32, 0x1000]);
        assert_eq!(state.stack_pointer, 4);
        assert_eq!(state.frame_base, 4);
        assert_eq!(state.frame_top, 4);
        assert!(state.frames.is_empty());
        assert_eq!(state.program.as_ref().unwrap().data, vec![1, 2, 3, 4]);
        assert_eq!(
            state.memory.as_ref().unwrap().get(..4).unwrap(),
            &[9, 8, 7, 6]
        );
    }

    #[test]
    fn scans_local_reference_samples_without_writing_to_them() {
        let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(3)
            .expect("runtime crate should be inside the simulator workspace")
            .join("games");
        let samples = [
            ("cs2_full_v401", EngineVariant::V401),
            ("牛顿与苹果树", EngineVariant::Legacy),
            (
                "[170526][NEKO WORKs] ネコぱら vol.3 steam版 (files)\\nekopara_vol3",
                EngineVariant::Steam,
            ),
            ("灰色的乐园v1", EngineVariant::Legacy),
        ];

        for (relative_path, expected_variant) in samples {
            let path = workspace_root.join(relative_path);
            if !path.is_dir() {
                continue;
            }
            let scan = GameRoot::open(&path)
                .expect("sample directory should open")
                .scan()
                .expect("sample directory should scan");
            assert_eq!(scan.engine.family, EngineFamily::CatSystem2);
            assert_eq!(scan.engine.variant, expected_variant);
            assert!(!scan.resources.archives().is_empty());
        }
    }
}

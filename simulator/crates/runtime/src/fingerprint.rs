use crate::RuntimeResult;
use crate::resource::{DirectoryFile, ResourceIndex};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum EngineFamily {
    CatSystem2,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum EngineVariant {
    V401,
    Legacy,
    Steam,
    Extended,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BinaryFingerprint {
    pub relative_path: PathBuf,
    pub size: u64,
    pub digest: String,
    #[serde(default)]
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EngineFingerprint {
    pub family: EngineFamily,
    pub variant: EngineVariant,
    pub primary_executable: Option<BinaryFingerprint>,
    pub engine_binaries: Vec<BinaryFingerprint>,
    pub signature: String,
    #[serde(skip)]
    pub legacy_save_signature: Option<String>,
}

impl EngineFingerprint {
    pub(crate) fn detect(root: &Path, resources: &ResourceIndex) -> RuntimeResult<Self> {
        let mut executables = resources
            .files()
            .iter()
            .filter(|file| {
                file.relative_path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
            })
            .collect::<Vec<_>>();
        executables.sort_by_key(|file| executable_rank(&file.relative_path));

        let mut engine_files = resources
            .files()
            .iter()
            .filter(|file| is_engine_binary(&file.relative_path))
            .collect::<Vec<_>>();
        engine_files.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));

        let previous_primary = executables.first().copied();
        let primary_file = previous_primary.and_then(|file| {
            if is_configuration_executable(&file.relative_path) {
                executables
                    .iter()
                    .copied()
                    .find(|candidate| {
                        executable_rank(&candidate.relative_path).0 < 2
                            && !is_configuration_executable(&candidate.relative_path)
                    })
                    .or(Some(file))
            } else {
                Some(file)
            }
        });
        let primary_executable = primary_file
            .map(|file| binary_fingerprint(root, file))
            .transpose()?;
        let engine_binaries = engine_files
            .iter()
            .map(|file| binary_fingerprint(root, file))
            .collect::<RuntimeResult<Vec<_>>>()?;

        let family = if resources
            .files()
            .iter()
            .any(|file| is_catsystem2_marker(&file.relative_path))
            || resources.archives().iter().any(|archive| {
                archive
                    .path
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case("kcs.int"))
            }) {
            EngineFamily::CatSystem2
        } else {
            EngineFamily::Unknown
        };
        let variant = detect_variant(resources.files(), primary_executable.as_ref());
        let legacy_save_signature = previous_primary
            .filter(|file| primary_file.is_some_and(|primary| primary.relative_path != file.relative_path))
            .map(|file| binary_fingerprint(root, file))
            .transpose()?
            .map(|previous| signature(&family, &variant, &Some(previous), &engine_binaries));
        let signature = signature(&family, &variant, &primary_executable, &engine_binaries);

        Ok(Self {
            family,
            variant,
            primary_executable,
            engine_binaries,
            signature,
            legacy_save_signature,
        })
    }
}

fn binary_fingerprint(root: &Path, file: &DirectoryFile) -> RuntimeResult<BinaryFingerprint> {
    let path = root.join(&file.relative_path);
    let (digest, sha256) = digest_file(&path).map_err(|source| crate::RuntimeError::Io {
        path: path.clone(),
        source,
    })?;
    Ok(BinaryFingerprint {
        relative_path: file.relative_path.clone(),
        size: file.size,
        digest,
        sha256,
    })
}

fn digest_file(path: &Path) -> std::io::Result<(String, String)> {
    let mut file = File::open(path)?;
    let mut state = [
        0xcbf29ce484222325_u64,
        0x84222325cbf29ce4_u64,
        0x9e3779b185ebca87_u64,
        0x517cc1b727220a95_u64,
    ];
    let mut buffer = [0_u8; 64 * 1024];
    let mut position = 0_u64;
    let mut sha256 = crate::sha256::Sha256::default();
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        sha256.update(&buffer[..read]);
        for byte in &buffer[..read] {
            for (lane, lane_state) in state.iter_mut().enumerate() {
                let mixed = (*byte as u64)
                    .wrapping_add(position)
                    .wrapping_add((lane as u64) << 8);
                *lane_state ^= mixed;
                *lane_state = lane_state
                    .wrapping_mul(0x100000001b3)
                    .rotate_left((lane * 7 + 1) as u32);
            }
            position = position.wrapping_add(1);
        }
    }
    Ok((state
        .iter()
        .map(|lane| format!("{lane:016x}"))
        .collect::<String>(), sha256.finish()))
}

fn executable_rank(path: &Path) -> (u8, usize, String) {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let excluded = name.contains("uninst")
        || name.contains("install")
        || name.contains("boot")
        || name.contains("setup")
        || name.contains("tool");
    let preferred = if excluded {
        2
    } else if name == "cs2.exe"
        || name == "cs2_open.exe"
        || name.contains("nekopara")
        || name.contains("grisaia")
    {
        0
    } else {
        1
    };
    (preferred, path.components().count(), name)
}

fn is_configuration_executable(path: &Path) -> bool {
    path.file_name().is_some_and(|name| {
        name.eq_ignore_ascii_case("cs2conf.exe") || name.eq_ignore_ascii_case("cs2confx.exe")
    })
}

fn is_engine_binary(path: &Path) -> bool {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    name == "cs2.exe"
        || name == "cs2_open.exe"
        || name == "cs2conf.dll"
        || name == "cs2confx.dll"
        || name == "hook.dll"
        || name == "emotedriver.dll"
}

fn is_catsystem2_marker(path: &Path) -> bool {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    name == "cs2.exe"
        || name == "cs2_open.exe"
        || name == "cs2conf.dll"
        || name == "cs2confx.dll"
        || name == "kcs.int"
        || name == "fes.int"
}

fn detect_variant(files: &[DirectoryFile], _primary: Option<&BinaryFingerprint>) -> EngineVariant {
    let names = files
        .iter()
        .map(|file| file.relative_path.to_string_lossy().to_ascii_lowercase())
        .collect::<Vec<_>>();
    if names.iter().any(|name| name.ends_with("cs2_open.exe")) {
        EngineVariant::V401
    } else if names.iter().any(|name| name.ends_with("cs2confx.dll"))
        || names.iter().any(|name| name.contains("nekopara"))
    {
        EngineVariant::Steam
    } else if names.iter().any(|name| name.ends_with(".exe")) {
        EngineVariant::Legacy
    } else {
        EngineVariant::Unknown
    }
}

fn signature(
    family: &EngineFamily,
    variant: &EngineVariant,
    primary: &Option<BinaryFingerprint>,
    engine_binaries: &[BinaryFingerprint],
) -> String {
    let mut value = format!("{family:?}:{variant:?}");
    if let Some(primary) = primary {
        value.push_str(&primary.digest);
    }
    for binary in engine_binaries {
        value.push_str(&binary.relative_path.to_string_lossy());
        value.push_str(&binary.digest);
    }
    let mut state = 0xcbf29ce484222325_u64;
    for byte in value.as_bytes() {
        state ^= *byte as u64;
        state = state.wrapping_mul(0x100000001b3);
    }
    format!("{state:016x}")
}

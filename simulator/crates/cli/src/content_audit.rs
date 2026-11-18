use anyhow::{Context, Result};
use cs2_formats::{AnmFile, HgFile, MpegProgramStream, OggVorbis};
use cs2_runtime::{ArchiveStatus, GameRoot, ResourceIndex};
use cs2_script::{CstDocument, FesProgram, KcsProgram};
use serde::Serialize;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Serialize)]
pub struct WorkspaceAudit {
    pub schema_version: &'static str,
    pub status: &'static str,
    pub game_count: usize,
    pub games: Vec<GameAudit>,
}

#[derive(Debug, Serialize)]
pub struct GameAudit {
    pub root: PathBuf,
    pub variant: String,
    pub fingerprint: String,
    pub file_count: usize,
    pub archives: ArchiveAudit,
    pub resources: BTreeMap<String, u64>,
    pub scripts: ScriptAudit,
    pub samples: BTreeMap<String, Vec<FormatProbe>>,
    pub diagnostics: Vec<AuditDiagnostic>,
}

#[derive(Debug, Default, Serialize)]
pub struct ArchiveAudit {
    pub total: usize,
    pub indexed: usize,
    pub encrypted_without_seed: usize,
    pub non_kif: usize,
    pub malformed: usize,
    pub indexed_entries: u64,
    pub entries_by_extension: BTreeMap<String, u64>,
    pub selected_name_seeds: BTreeMap<String, String>,
}

#[derive(Debug, Default, Serialize)]
pub struct ScriptAudit {
    pub kcs_files: u64,
    pub cst_files: u64,
    pub fes_files: u64,
    pub anm_files: u64,
    pub parse_failures: u64,
    pub opcodes: BTreeMap<String, u64>,
    pub native_calls: BTreeMap<String, NativeCallCount>,
    /// Keep program identity and instruction offsets: numeric slots alone
    /// cannot distinguish ordinary, motion and particle callback tables.
    pub kcs_programs: Vec<KcsProgramAudit>,
}

#[derive(Debug, Serialize)]
pub struct KcsProgramAudit {
    pub resource: String,
    pub runtime_scope: bool,
    pub program_version_raw: u32,
    pub program_version: String,
    pub host_table_hint: &'static str,
    pub host_table_evidence: &'static str,
    pub instruction_count: usize,
    pub native_calls: Vec<NativeCallSite>,
}

#[derive(Debug, Serialize)]
pub struct NativeCallSite {
    pub instruction_offset: usize,
    pub index: Option<u32>,
    pub argument_bytes: Option<u32>,
    pub index_flags: u8,
    pub index_payload: u32,
    pub argument_flags: u8,
    pub argument_payload: u32,
}

#[derive(Debug, Default, Serialize)]
pub struct NativeCallCount {
    pub call_sites: u64,
    pub argument_bytes: BTreeMap<String, u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FormatProbe {
    pub resource: String,
    pub status: String,
    pub detail: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AuditDiagnostic {
    pub resource: String,
    pub message: String,
}

const MAX_PROBES_PER_FORMAT: usize = 4;
const PATCH_COMPONENTS: &[&str] = &[
    "backup",
    "directx",
    "patch",
    "patches",
    "tool",
    "tools",
    "原版备份",
    "补丁",
    "补丁＆附件",
    "補丁",
];

pub fn audit_workspace(paths: &[PathBuf]) -> Result<WorkspaceAudit> {
    let mut games = Vec::with_capacity(paths.len());
    for path in paths {
        games.push(audit_game(path)?);
    }
    Ok(WorkspaceAudit {
        schema_version: "1.0.0",
        status: if games.iter().all(|game| game.diagnostics.is_empty()) {
            "scan-complete"
        } else {
            "scan-complete-with-diagnostics"
        },
        game_count: games.len(),
        games,
    })
}

pub fn audit_game(path: &Path) -> Result<GameAudit> {
    let root =
        GameRoot::open(path).with_context(|| format!("open game directory {}", path.display()))?;
    let scan = root.scan().context("scan game resources")?;
    let resources = &scan.resources;
    let mut archive_audit = ArchiveAudit {
        total: resources.archives().len(),
        ..ArchiveAudit::default()
    };
    let mut resource_counts = BTreeMap::new();
    let mut scripts = ScriptAudit::default();
    let mut samples = BTreeMap::<String, Vec<FormatProbe>>::new();
    let mut diagnostics = Vec::new();

    for file in resources.files() {
        if excluded_path(&file.relative_path) {
            continue;
        }
        let extension =
            extension_bytes(file.relative_path.as_os_str().to_string_lossy().as_bytes());
        if let Some(extension) = extension {
            *resource_counts.entry(extension.clone()).or_insert(0) += 1;
            if matches!(extension.as_str(), "kcs" | "cst" | "fes" | "anm") {
                let name = file.relative_path.to_string_lossy().to_string();
                match resources.read(&name) {
                    Ok(bytes) => {
                        audit_script(&bytes, &extension, &name, &mut scripts, &mut diagnostics)
                    }
                    Err(error) => diagnostics.push(AuditDiagnostic {
                        resource: name,
                        message: format!("read loose script: {error}"),
                    }),
                }
            }
            if is_probe_extension(&extension) {
                if should_probe(&samples, &extension) {
                    let name = file.relative_path.to_string_lossy().to_string();
                    let probe = probe_loose(resources, &file.relative_path, &extension, &name);
                    record_probe(&mut samples, &extension, probe);
                }
            }
        }
    }

    for (archive_index, archive) in resources.archives().iter().enumerate() {
        if excluded_path(&archive.path) {
            continue;
        }
        match &archive.status {
            ArchiveStatus::Indexed => {
                archive_audit.indexed += 1;
                archive_audit.indexed_entries += archive.entries.len() as u64;
                if let Some(seed) = archive.name_seed {
                    archive_audit.selected_name_seeds.insert(
                        archive.path.to_string_lossy().to_string(),
                        format!(
                            "{seed:#010x} score={}",
                            archive.name_seed_score.unwrap_or_default()
                        ),
                    );
                }
            }
            ArchiveStatus::Encrypted { .. } => {
                archive_audit.encrypted_without_seed += 1;
                continue;
            }
            ArchiveStatus::NotAnArchive => {
                archive_audit.non_kif += 1;
                continue;
            }
            ArchiveStatus::Malformed { reason } => {
                archive_audit.malformed += 1;
                diagnostics.push(AuditDiagnostic {
                    resource: archive.path.to_string_lossy().to_string(),
                    message: format!("malformed archive: {reason}"),
                });
                continue;
            }
        }
        for (entry_index, entry) in archive.entries.iter().enumerate() {
            if excluded_resource_name(entry.raw_name.as_slice()) {
                continue;
            }
            let Some(extension) = extension_bytes(&entry.raw_name) else {
                continue;
            };
            *archive_audit
                .entries_by_extension
                .entry(extension.clone())
                .or_insert(0) += 1;
            *resource_counts.entry(extension.clone()).or_insert(0) += 1;
            let name = format!(
                "{}::{}",
                archive.path.to_string_lossy(),
                String::from_utf8_lossy(&entry.raw_name)
            );
            match extension.as_str() {
                "kcs" | "cst" | "fes" | "anm" => {
                    match resources.read_archive_entry_at(archive_index, entry_index) {
                        Ok(bytes) => {
                            audit_script(&bytes, &extension, &name, &mut scripts, &mut diagnostics)
                        }
                        Err(error) => diagnostics.push(AuditDiagnostic {
                            resource: name,
                            message: format!("read script entry: {error}"),
                        }),
                    }
                }
                _ if is_probe_extension(&extension) => {
                    if should_probe(&samples, &extension) {
                        match resources.read_archive_entry_at(archive_index, entry_index) {
                            Ok(bytes) => {
                                let probe = probe_payload(&bytes, &extension, &name);
                                record_probe(&mut samples, &extension, probe);
                            }
                            Err(error) => {
                                record_probe(
                                    &mut samples,
                                    &extension,
                                    FormatProbe {
                                        resource: name,
                                        status: "read-error".to_string(),
                                        detail: Some(error.to_string()),
                                    },
                                );
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }

    for diagnostic in resources.diagnostics() {
        diagnostics.push(AuditDiagnostic {
            resource: diagnostic.path.to_string_lossy().to_string(),
            message: diagnostic.message.clone(),
        });
    }

    Ok(GameAudit {
        root: scan.root,
        variant: format!("{:?}", scan.engine.variant),
        fingerprint: scan.engine.signature,
        file_count: resources.files().len(),
        archives: archive_audit,
        resources: resource_counts,
        scripts,
        samples,
        diagnostics,
    })
}

fn audit_script(
    bytes: &[u8],
    extension: &str,
    name: &str,
    audit: &mut ScriptAudit,
    diagnostics: &mut Vec<AuditDiagnostic>,
) {
    let result: std::result::Result<(), String> = match extension {
        "kcs" => {
            audit.kcs_files += 1;
            match KcsProgram::parse(bytes) {
                Ok(program) => {
                    let version = program.header.program_version;
                    let mut identity = KcsProgramAudit {
                        resource: name.to_owned(),
                        runtime_scope: name.replace('\\', "/").to_ascii_lowercase() != "inst/inst.kcs",
                        program_version_raw: version,
                        program_version: format!("{}.{}", version & 0xffff, version >> 16),
                        host_table_hint: host_table_hint(name),
                        host_table_evidence: "mount/path category hint; runtime registration not verified",
                        instruction_count: program.instructions.len(),
                        native_calls: Vec::new(),
                    };
                    for instruction in program.instructions {
                        if identity.runtime_scope {
                            *audit.opcodes.entry(format!("{:#04x}", instruction.opcode)).or_insert(0) += 1;
                        }
                        if instruction.opcode == 0x2b
                            && instruction.operands.len() == 2
                        {
                            let index_operand = instruction.operands[0];
                            let argument_operand = instruction.operands[1];
                            let index = matches!(index_operand.flags.bits(), 3 | 0x23)
                                .then_some(index_operand.payload);
                            let argument_bytes = matches!(argument_operand.flags.bits(), 3 | 0x23)
                                .then_some(argument_operand.payload);
                            identity.native_calls.push(NativeCallSite {
                                instruction_offset: instruction.offset,
                                index,
                                argument_bytes,
                                index_flags: index_operand.flags.bits(),
                                index_payload: index_operand.payload,
                                argument_flags: argument_operand.flags.bits(),
                                argument_payload: argument_operand.payload,
                            });
                            if let (Some(native), Some(argument_bytes)) = (index, argument_bytes)
                                && identity.runtime_scope
                            {
                                let count = audit.native_calls.entry(native.to_string()).or_default();
                                count.call_sites += 1;
                                *count.argument_bytes.entry(argument_bytes.to_string()).or_insert(0) += 1;
                            }
                        }
                    }
                    audit.kcs_programs.push(identity);
                    Ok(())
                }
                Err(error) => Err(error.to_string()),
            }
        }
        "cst" => {
            audit.cst_files += 1;
            CstDocument::parse(bytes)
                .map(|_| ())
                .map_err(|error| error.to_string())
        }
        "fes" => {
            audit.fes_files += 1;
            FesProgram::parse(bytes)
                .map(|_| ())
                .map_err(|error| error.to_string())
        }
        "anm" => {
            audit.anm_files += 1;
            AnmFile::parse(bytes)
                .map(|_| ())
                .map_err(|error| error.to_string())
        }
        _ => Ok(()),
    };
    if let Err(message) = result {
        audit.parse_failures += 1;
        diagnostics.push(AuditDiagnostic {
            resource: name.to_string(),
            message: format!("{extension} parse failed: {message}"),
        });
    }
}

fn host_table_hint(resource: &str) -> &'static str {
    let normalized = resource.replace('\\', "/").to_ascii_lowercase();
    for component in normalized.split(['/', ':']).filter(|part| !part.is_empty()) {
        match component {
            "mot" | "mot.int" | "motion" => return "motion",
            "ptcl" | "ptcl.int" | "particle" => return "particle",
            "kcs" | "kcs.int" => return "ordinary",
            _ => {}
        }
    }
    "unknown"
}

fn probe_loose(
    resources: &ResourceIndex,
    relative: &Path,
    extension: &str,
    name: &str,
) -> FormatProbe {
    let path = resources.root().join(relative);
    let result = File::open(&path)
        .and_then(|mut file| {
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes)?;
            Ok(bytes)
        })
        .map(|bytes| probe_payload(&bytes, extension, name))
        .unwrap_or_else(|error| FormatProbe {
            resource: name.to_string(),
            status: "read-error".to_string(),
            detail: Some(error.to_string()),
        });
    result
}

fn probe_payload(bytes: &[u8], extension: &str, name: &str) -> FormatProbe {
    let result = match extension {
        "hg2" | "hg3" => HgFile::parse(bytes)
            .map(|file| format!("frames={} version={:#x}", file.frames.len(), file.version))
            .map_err(|error| error.to_string()),
        "ogg" => OggVorbis::metadata(bytes)
            .map(|metadata| {
                format!(
                    "sample_rate={} channels={}",
                    metadata.sample_rate, metadata.channels
                )
            })
            .map_err(|error| error.to_string()),
        "mpg" | "mpeg" | "m1v" | "mp4" | "webm" | "avi" | "wmv" => MpegProgramStream::probe(bytes)
            .map(|stream| {
                format!(
                    "video_streams={} audio_streams={} duration_seconds={:?}",
                    stream.video_stream_ids.len(),
                    stream.audio_stream_ids.len(),
                    stream.duration_seconds
                )
            })
            .map_err(|error| error.to_string()),
        _ => Ok("recognized by extension only".to_string()),
    };
    match result {
        Ok(detail) => FormatProbe {
            resource: name.to_string(),
            status: "parsed".to_string(),
            detail: Some(detail),
        },
        Err(error) => FormatProbe {
            resource: name.to_string(),
            status: "unsupported-or-invalid".to_string(),
            detail: Some(error),
        },
    }
}

fn record_probe(
    samples: &mut BTreeMap<String, Vec<FormatProbe>>,
    extension: &str,
    probe: FormatProbe,
) {
    let entries = samples.entry(extension.to_string()).or_default();
    if entries.len() < MAX_PROBES_PER_FORMAT {
        entries.push(probe);
    }
}

fn should_probe(samples: &BTreeMap<String, Vec<FormatProbe>>, extension: &str) -> bool {
    samples
        .get(extension)
        .is_none_or(|entries| entries.len() < MAX_PROBES_PER_FORMAT)
}

fn is_probe_extension(extension: &str) -> bool {
    matches!(
        extension,
        "hg2" | "hg3" | "ogg" | "mpg" | "mpeg" | "m1v" | "mp4" | "webm" | "avi" | "wmv"
    )
}

fn excluded_path(path: &Path) -> bool {
    path.components().any(|component| match component {
        Component::Normal(value) => PATCH_COMPONENTS
            .iter()
            .any(|excluded| value.to_string_lossy().eq_ignore_ascii_case(excluded)),
        _ => false,
    })
}

fn excluded_resource_name(name: &[u8]) -> bool {
    name.split(|byte| matches!(*byte, b'/' | b'\\'))
        .any(|part| {
            PATCH_COMPONENTS
                .iter()
                .any(|excluded| part.eq_ignore_ascii_case(excluded.as_bytes()))
        })
}

fn extension_bytes(name: &[u8]) -> Option<String> {
    let basename = name
        .rsplit(|byte| matches!(*byte, b'/' | b'\\'))
        .next()
        .unwrap_or(name);
    let dot = basename.iter().rposition(|byte| *byte == b'.')?;
    let extension = basename.get(dot + 1..)?;
    if extension.is_empty() || !extension.is_ascii() {
        return None;
    }
    Some(std::str::from_utf8(extension).ok()?.to_ascii_lowercase())
}

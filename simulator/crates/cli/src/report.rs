use crate::content_audit::{self, AuditDiagnostic, GameAudit, WorkspaceAudit};
use anyhow::{Context, Result};
use cs2_desktop::{CompatibilityReport, HeadlessReport, LogicEvent};
use cs2_runtime::{ArchiveIndex, ArchiveStatus, GameScan, ScanDiagnostic, ScanDiagnosticLevel};
use serde::Serialize;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

pub const SCHEMA_VERSION: &str = "cs2-cli-report/v1";

pub fn scan_json(scan: &GameScan) -> Result<Value> {
    Ok(json!({
        "schema_version": SCHEMA_VERSION,
        "kind": "scan",
        "status": scan_status(&scan.diagnostics),
        "root": stable_path(&scan.root),
        "engine": engine_json(scan)?,
        "resources": resource_summary(scan)?,
        "configuration": {
            "candidates": scan.config_candidates,
            "count": scan.config_candidates.len(),
        },
        "diagnostics": diagnostics_json(&scan.diagnostics),
    }))
}

pub fn compatibility_json(report: &CompatibilityReport, scan: Option<&GameScan>) -> Value {
    let mut value = json!({
        "schema_version": SCHEMA_VERSION,
        "kind": "compatibility",
        "status": compatibility_status(report),
        "verification": {
            "scope": "static-resource-and-host-capability-detection",
            "gameplay_executed": false,
            "complete_game_compatibility_verified": false,
        },
        "engine_family": report.engine_family,
        "detected_variant": report.detected_variant,
        "primary_executable": report.primary_executable,
        "pe_architecture": report.pe_architecture,
        "imported_modules": report.imported_modules,
        "runtime_profiles": report.runtime_profiles,
        "resource_extensions": report.resource_extensions,
        "script_formats": report.script_formats,
        "audio_formats": report.audio_formats,
        "video_formats": report.video_formats,
        "save_formats": report.save_formats,
        "archive_count": report.archive_count,
        "indexed_archives": report.indexed_archives,
        "encrypted_archives": report.encrypted_archives,
        "malformed_archives": report.malformed_archives,
        "configuration_resources": report.configuration_resources,
        "capabilities": {
            "window": capability_json(&report.window),
            "input": capability_json(&report.input),
            "audio": capability_json(&report.audio),
            "video": capability_json(&report.video),
            "saves": capability_json(&report.saves),
        },
        "boundaries": report.boundaries,
    });
    if let Some(scan) = scan {
        if let Some(object) = value.as_object_mut() {
            object.insert("root".to_string(), Value::String(stable_path(&scan.root)));
            object.insert(
                "engine_signature".to_string(),
                Value::String(scan.engine.signature.clone()),
            );
            object.insert(
                "resource_fingerprint".to_string(),
                Value::String(
                    resource_manifest_digest(scan).unwrap_or_else(|_| "error".to_string()),
                ),
            );
        }
    }
    value
}

pub fn audit_json(report: &GameAudit) -> Result<Value> {
    Ok(json!({
        "schema_version": SCHEMA_VERSION,
        "kind": "content-audit",
        "status": if report.diagnostics.is_empty() {
            "scan-complete"
        } else {
            "scan-complete-with-diagnostics"
        },
        "root": stable_path(&report.root),
        "audit": report,
    }))
}

pub fn workspace_audit_json(report: &WorkspaceAudit) -> Result<Value> {
    Ok(json!({
        "schema_version": SCHEMA_VERSION,
        "kind": "content-audit-batch",
        "status": report.status,
        "game_count": report.game_count,
        "games": report.games,
    }))
}

pub fn game_report_batch(paths: &[PathBuf]) -> Result<Value> {
    let mut samples = Vec::with_capacity(paths.len());
    let mut failures = 0usize;
    for path in paths {
        match cs2_runtime::GameRoot::open(path)
            .and_then(|root| root.scan())
            .map_err(|error| anyhow::anyhow!(error.to_string()))
            .and_then(|scan| game_report(path, &scan))
        {
            Ok(report) => samples.push(report),
            Err(error) => {
                failures += 1;
                samples.push(json!({
                    "schema_version": SCHEMA_VERSION,
                    "kind": "game-report",
                    "status": "error",
                    "root": stable_path(path),
                    "error": format!("{error:#}"),
                }));
            }
        }
    }
    Ok(json!({
        "schema_version": SCHEMA_VERSION,
        "kind": "game-report-batch",
        "status": if failures == 0 { "complete" } else { "complete-with-errors" },
        "sample_count": paths.len(),
        "failure_count": failures,
        "samples": samples,
    }))
}

pub fn game_report(path: &Path, scan: &GameScan) -> Result<Value> {
    let audit = content_audit::audit_game(path)?;
    let pes = pe_reports(scan)?;
    let call_table = call_table(&audit);
    let function_ledger = function_ledger(&pes, scan, &call_table);
    let mut diagnostics = diagnostics_json(&scan.diagnostics);
    diagnostics.extend(
        audit
            .diagnostics
            .iter()
            .map(audit_diagnostic_json)
            .collect::<Vec<_>>(),
    );
    diagnostics.extend(
        pes.iter()
            .flat_map(|pe| {
                pe.diagnostics.iter().map(|message| {
                    json!({
                        "level": "warning",
                        "path": pe.path,
                        "message": message,
                    })
                })
            })
            .collect::<Vec<_>>(),
    );
    Ok(json!({
        "schema_version": SCHEMA_VERSION,
        "kind": "game-report",
        "status": if diagnostics.is_empty() { "complete" } else { "complete-with-diagnostics" },
        "root": stable_path(path),
        "engine": engine_json(scan)?,
        "resources": resource_summary(scan)?,
        "fingerprints": {
            "engine": scan.engine.signature,
            "resources": resource_manifest_digest(scan)?,
            "combined": combined_fingerprint(scan)?,
        },
        "pe": pes,
        "function_ledger": function_ledger,
        "call_table": call_table,
        "compatibility": compatibility_json(&CompatibilityReport::from_scan(scan), Some(scan)),
        "content_audit": audit,
        "diagnostics": diagnostics,
    }))
}

pub fn headless_json(report: &HeadlessReport, mode: &str) -> Value {
    json!({
        "schema_version": SCHEMA_VERSION,
        "kind": mode,
        "verification": {
            "scope": "requested-bounded-run-only",
            "complete_game_compatibility_verified": false,
        },
        "status": if has_runtime_failures(report) {
            "runtime-failed"
        } else if report.frame_in_progress {
            "budget-exhausted"
        } else {
            "complete"
        },
        "root": stable_path(&report.root),
        "frames": report.state.clock.frame,
        "engine": engine_json(&report.scan).unwrap_or_else(|_| json!({
            "family": format!("{:?}", report.scan.engine.family),
            "variant": format!("{:?}", report.scan.engine.variant),
            "signature": report.scan.engine.signature,
        })),
        "compatibility": compatibility_json(&report.compatibility, Some(&report.scan)),
        "loaded": report.loaded.as_ref().map(|loaded| json!({
            "name": loaded.name,
            "kind": loaded.kind,
            "byte_length": loaded.byte_length,
            "detail": loaded.detail,
        })),
        "runtime": {
            "current_task_id": report.current_task_id,
            "state": report.state,
            "scene_nodes": report.scene_nodes.iter().map(scene_node_json).collect::<Vec<_>>(),
            "particle_snapshot": particle_snapshot_json(&report.particle_snapshot),
            "menu_item_requests": report.menu_item_requests,
            "debug_mode": report.debug_mode,
            "sound_config_enabled": report.sound_config_enabled,
            "fes_exit_code": report.fes_exit_code,
            "frame_hash": format!("{:016x}", report.frame_hash),
            "frame_size": report.frame_size,
            "frame_in_progress": report.frame_in_progress,
            "frame_rgba_hash": format!("{:016x}", stable_bytes_hash(&report.frame_rgba)),
        },
        "events": report.events.iter().map(event_json).collect::<Vec<_>>(),
        "log": report.log,
        "diagnostics": diagnostics_json(&report.scan.diagnostics),
    })
}

pub fn diff_json(left: &HeadlessReport, right: &HeadlessReport, differences: &[String]) -> Value {
    json!({
        "schema_version": SCHEMA_VERSION,
        "kind": "diff",
        "status": if differences.is_empty() { "equal" } else { "different" },
        "left": headless_json(left, "run"),
        "right": headless_json(right, "run"),
        "differences": differences,
    })
}

pub fn smoke_json(results: &[Value], failure_count: usize, frames: u64) -> Value {
    json!({
        "schema_version": SCHEMA_VERSION,
        "kind": "smoke",
        "verification": {
            "scope": "requested-bounded-run-only",
            "complete_game_compatibility_verified": false,
        },
        "status": if failure_count == 0 { "passed" } else { "failed" },
        "sample_count": results.len(),
        "failure_count": failure_count,
        "frames": frames,
        "samples": results,
    })
}

pub fn window_preflight_json(scan: &GameScan) -> Result<Value> {
    Ok(json!({
        "schema_version": SCHEMA_VERSION,
        "kind": "window-preflight",
        "status": "ready",
        "verification": {
            "scope": "static-preflight-only",
            "window_created": false,
            "gameplay_executed": false,
            "complete_game_compatibility_verified": false,
        },
        "root": stable_path(&scan.root),
        "engine": engine_json(scan)?,
        "compatibility": compatibility_json(&CompatibilityReport::from_scan(scan), Some(scan)),
        "original_executable_policy": "never-started",
    }))
}

pub fn write_json(path: &Path, value: &Value) -> Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
        && !parent.is_dir()
    {
        anyhow::bail!("report output parent does not exist: {}", parent.display());
    }
    let text = serde_json::to_string_pretty(value)?;
    std::fs::write(path, format!("{text}\n"))
        .with_context(|| format!("write report `{}`", path.display()))?;
    Ok(())
}

fn engine_json(scan: &GameScan) -> Result<Value> {
    Ok(json!({
        "family": format!("{:?}", scan.engine.family),
        "variant": format!("{:?}", scan.engine.variant),
        "signature": scan.engine.signature,
        "primary_executable": scan.engine.primary_executable.as_ref().map(binary_json),
        "engine_binaries": scan.engine.engine_binaries.iter().map(binary_json).collect::<Vec<_>>(),
    }))
}

fn binary_json(binary: &cs2_runtime::BinaryFingerprint) -> Value {
    json!({
        "path": stable_path(&binary.relative_path),
        "size": binary.size,
        "digest": binary.digest,
        "sha256": binary.sha256,
    })
}

fn resource_summary(scan: &GameScan) -> Result<Value> {
    let mut extensions = BTreeMap::<String, u64>::new();
    let mut total_bytes = 0_u64;
    let mut files = scan.resources.files().iter().collect::<Vec<_>>();
    files.sort_by_key(|file| stable_path_key(&file.relative_path));
    let file_rows = files
        .iter()
        .map(|file| {
            total_bytes = total_bytes.saturating_add(file.size);
            if let Some(extension) = file
                .relative_path
                .extension()
                .and_then(|value| value.to_str())
            {
                *extensions
                    .entry(extension.to_ascii_lowercase())
                    .or_default() += 1;
            }
            json!({
                "path": stable_path(&file.relative_path),
                "size": file.size,
                "digest": digest_path(&scan.root.join(&file.relative_path)).unwrap_or_else(|_| "error".to_string()),
            })
        })
        .collect::<Vec<_>>();
    let archives = scan
        .resources
        .archives()
        .iter()
        .map(archive_json)
        .collect::<Vec<_>>();
    Ok(json!({
        "file_count": files.len(),
        "total_bytes": total_bytes,
        "extensions": extensions,
        "files": file_rows,
        "archive_count": archives.len(),
        "archives": archives,
        "manifest_digest": resource_manifest_digest(scan)?,
    }))
}

fn archive_json(archive: &ArchiveIndex) -> Value {
    let status = match &archive.status {
        ArchiveStatus::Indexed => json!({"kind": "indexed"}),
        ArchiveStatus::Encrypted { raw_entry_count } => {
            json!({"kind": "encrypted", "raw_entry_count": raw_entry_count})
        }
        ArchiveStatus::NotAnArchive => json!({"kind": "not-an-archive"}),
        ArchiveStatus::Malformed { reason } => json!({"kind": "malformed", "reason": reason}),
    };
    json!({
        "path": stable_path(&archive.path),
        "entry_count": archive.entry_count,
        "status": status,
        "name_seed": archive.name_seed.map(|seed| format!("{seed:08x}")),
        "name_seed_score": archive.name_seed_score,
        "entries": archive.entries.iter().map(|entry| json!({
            "name": entry.name,
            "raw_name_hex": hex(&entry.raw_name),
            "offset": entry.offset,
            "size": entry.size,
        })).collect::<Vec<_>>(),
    })
}

fn call_table(audit: &GameAudit) -> Value {
    let mut calls = Vec::new();
    for (index, count) in &audit.scripts.native_calls {
        calls.push(json!({
            "index": index.parse::<u32>().unwrap_or_default(),
            "call_sites": count.call_sites,
            "argument_bytes": count.argument_bytes,
            "status": "static-observation",
        }));
    }
    calls.sort_by_key(|entry| {
        entry
            .get("index")
            .and_then(Value::as_u64)
            .unwrap_or_default()
    });
    json!({
        "source": "KCS opcode 0x2b",
        "native_call_count": calls.len(),
        "total_call_sites": calls.iter().map(|entry| entry.get("call_sites").and_then(Value::as_u64).unwrap_or_default()).sum::<u64>(),
        "calls": calls,
        "opcodes": audit.scripts.opcodes,
    })
}

fn function_ledger(pes: &[PeReport], scan: &GameScan, call_table: &Value) -> Value {
    let binaries = pes
        .iter()
        .map(|pe| {
            json!({
                "path": pe.path,
                "architecture": pe.architecture,
                "format": pe.format,
                "entrypoint_rva": pe.entrypoint_rva,
                "function_count": pe.functions.len(),
                "functions": pe.functions,
                "imports": pe.imports,
                "exports": pe.exports,
                "sections": pe.sections,
                "analysis_status": "static-pe-symbols-and-imports",
            })
        })
        .collect::<Vec<_>>();
    json!({
        "schema_version": "cs2-function-ledger/v1",
        "status": "static-evidence-only",
        "semantic_implementation_claim": false,
        "game_root": stable_path(&scan.root),
        "binary_count": binaries.len(),
        "binary_function_count": binaries.iter().map(|binary| binary.get("function_count").and_then(Value::as_u64).unwrap_or_default()).sum::<u64>(),
        "native_call_sites": call_table.get("total_call_sites").cloned().unwrap_or(Value::from(0)),
        "binaries": binaries,
    })
}

fn diagnostics_json(diagnostics: &[ScanDiagnostic]) -> Vec<Value> {
    diagnostics
        .iter()
        .map(|diagnostic| {
            json!({
                "level": diagnostic_level(&diagnostic.level),
                "path": stable_path(&diagnostic.path),
                "message": diagnostic.message,
            })
        })
        .collect()
}

fn audit_diagnostic_json(diagnostic: &AuditDiagnostic) -> Value {
    json!({
        "level": "warning",
        "path": diagnostic.resource.replace('\\', "/"),
        "message": diagnostic.message,
    })
}

fn capability_json(capability: &cs2_desktop::HostCapability) -> Value {
    json!({
        "status": capability.status,
        "detail": capability.detail,
    })
}

fn event_json(event: &LogicEvent) -> Value {
    json!({
        "frame": event.frame,
        "kind": event.kind,
        "detail": event.detail,
    })
}

fn scene_node_json(node: &cs2_desktop::SceneNode) -> Value {
    json!({
        "id": node.id,
        "kind": node.kind,
        "parent": node.parent,
        "name": node.name,
        "size": node.size,
        "position": node.position,
        "scale": node.scale,
        "color": format!("{:08x}", node.color),
        "overlay_color": format!("{:08x}", node.overlay_color),
        "visible": node.visible,
        "initialized": node.initialized,
    })
}

fn particle_snapshot_json(snapshot: &cs2_desktop::ParticleSnapshot) -> Value {
    json!({
        "frame": snapshot.frame,
        "objects": snapshot.objects,
        "points": snapshot.points.iter().map(|point| json!({
            "position": {"x": point.position.x, "y": point.position.y},
            "color": {
                "r": point.color.r,
                "g": point.color.g,
                "b": point.color.b,
                "a": point.color.a,
            },
        })).collect::<Vec<_>>(),
    })
}

fn has_runtime_failures(report: &HeadlessReport) -> bool {
    report.events.iter().any(|event| {
        event.kind == "vm-error"
            || event.kind == "task-error"
            || (event.kind == "native-call"
                && event
                    .detail
                    .to_ascii_lowercase()
                    .contains("is not implemented"))
    })
}

fn compatibility_status(report: &CompatibilityReport) -> &'static str {
    if report.boundaries.is_empty()
        && report.window.status == "supported"
        && report.input.status == "supported"
    {
        "compatible"
    } else {
        "compatible-with-boundaries"
    }
}

fn scan_status(diagnostics: &[ScanDiagnostic]) -> &'static str {
    if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.level == ScanDiagnosticLevel::Error)
    {
        "scan-complete-with-errors"
    } else if diagnostics.is_empty() {
        "scan-complete"
    } else {
        "scan-complete-with-diagnostics"
    }
}

fn resource_manifest_digest(scan: &GameScan) -> Result<String> {
    let mut rows = Vec::new();
    let mut files = scan.resources.files().iter().collect::<Vec<_>>();
    files.sort_by_key(|file| stable_path_key(&file.relative_path));
    for file in files {
        rows.push(format!(
            "file|{}|{}|{}",
            stable_path(&file.relative_path),
            file.size,
            digest_path(&scan.root.join(&file.relative_path))?
        ));
    }
    for archive in scan.resources.archives() {
        rows.push(format!(
            "archive|{}|{}|{}",
            stable_path(&archive.path),
            archive.entry_count,
            archive
                .entries
                .iter()
                .map(|entry| format!("{}:{}:{}", entry.name, entry.offset, entry.size))
                .collect::<Vec<_>>()
                .join(",")
        ));
    }
    Ok(stable_text_hash(&rows.join("\n")))
}

fn combined_fingerprint(scan: &GameScan) -> Result<String> {
    Ok(stable_text_hash(&format!(
        "{}|{}",
        scan.engine.signature,
        resource_manifest_digest(scan)?
    )))
}

fn digest_path(path: &Path) -> Result<String> {
    let mut file =
        File::open(path).with_context(|| format!("open fingerprint input `{}`", path.display()))?;
    let mut state = [
        0xcbf29ce484222325_u64,
        0x84222325cbf29ce4_u64,
        0x9e3779b185ebca87_u64,
        0x517cc1b727220a95_u64,
    ];
    let mut buffer = [0_u8; 64 * 1024];
    let mut position = 0_u64;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        for byte in &buffer[..read] {
            for (lane, lane_state) in state.iter_mut().enumerate() {
                *lane_state ^= u64::from(*byte)
                    .wrapping_add(position)
                    .wrapping_add((lane as u64) << 8);
                *lane_state = lane_state
                    .wrapping_mul(0x100000001b3)
                    .rotate_left((lane * 7 + 1) as u32);
            }
            position = position.wrapping_add(1);
        }
    }
    Ok(state
        .iter()
        .map(|lane| format!("{lane:016x}"))
        .collect::<String>())
}

fn stable_text_hash(text: &str) -> String {
    let mut state = 0xcbf29ce484222325_u64;
    for byte in text.as_bytes() {
        state ^= u64::from(*byte);
        state = state.wrapping_mul(0x100000001b3);
    }
    format!("{state:016x}")
}

fn stable_bytes_hash(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    hash
}

fn diagnostic_level(level: &ScanDiagnosticLevel) -> &'static str {
    match level {
        ScanDiagnosticLevel::Warning => "warning",
        ScanDiagnosticLevel::Error => "error",
    }
}

fn stable_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn stable_path_key(path: &Path) -> (String, String) {
    let path = stable_path(path);
    (path.to_ascii_lowercase(), path)
}

fn hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push_str(&format!("{byte:02x}"));
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let suffix = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock should be after epoch")
                .as_nanos();
            let path = std::env::temp_dir().join(format!("cs2-report-{suffix}"));
            fs::create_dir_all(&path).expect("temporary directory should be created");
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn stable_hash_and_hex_are_deterministic() {
        assert_eq!(stable_text_hash("abc"), stable_text_hash("abc"));
        assert_ne!(stable_text_hash("abc"), stable_text_hash("abd"));
        assert_eq!(hex(&[0, 0xab, 0xff]), "00abff");
    }

    #[test]
    fn parses_minimal_pe_and_records_entrypoint_evidence() {
        let temp = TempDir::new();
        let path = temp.0.join("sample.exe");
        let mut bytes = vec![0_u8; 0x240];
        bytes[..2].copy_from_slice(b"MZ");
        bytes[0x3c..0x40].copy_from_slice(&0x80_u32.to_le_bytes());
        bytes[0x80..0x84].copy_from_slice(b"PE\0\0");
        bytes[0x84..0x86].copy_from_slice(&0x14c_u16.to_le_bytes());
        bytes[0x86..0x88].copy_from_slice(&1_u16.to_le_bytes());
        bytes[0x94..0x96].copy_from_slice(&224_u16.to_le_bytes());
        bytes[0x98..0x9a].copy_from_slice(&0x10b_u16.to_le_bytes());
        bytes[0x98 + 16..0x98 + 20].copy_from_slice(&0x1000_u32.to_le_bytes());
        bytes[0x98 + 28..0x98 + 32].copy_from_slice(&0x400000_u32.to_le_bytes());
        let section = 0x178;
        bytes[section..section + 5].copy_from_slice(b".text");
        bytes[section + 8..section + 12].copy_from_slice(&16_u32.to_le_bytes());
        bytes[section + 12..section + 16].copy_from_slice(&0x1000_u32.to_le_bytes());
        bytes[section + 16..section + 20].copy_from_slice(&16_u32.to_le_bytes());
        bytes[section + 20..section + 24].copy_from_slice(&0x200_u32.to_le_bytes());
        bytes[section + 36..section + 40].copy_from_slice(&0x60000020_u32.to_le_bytes());
        fs::write(&path, bytes).expect("PE fixture should be written");

        let report = parse_pe(&temp.0, Path::new("sample.exe")).expect("PE should parse");
        assert_eq!(report.format, "PE32");
        assert_eq!(report.architecture, "x86");
        assert_eq!(report.entrypoint_rva, Some(0x1000));
        assert_eq!(report.functions.len(), 1);
        assert_eq!(report.sections[0]["name"], ".text");
    }

    #[test]
    fn game_report_contains_stable_fingerprint_and_compatibility_sections() {
        let temp = TempDir::new();
        fs::write(temp.0.join("sample.txt"), b"fixture").expect("fixture should be written");
        let scan = cs2_runtime::GameRoot::open(&temp.0)
            .expect("root should open")
            .scan()
            .expect("scan should finish");
        let report = game_report(&temp.0, &scan).expect("report should finish");
        assert_eq!(report["schema_version"], SCHEMA_VERSION);
        assert_eq!(report["kind"], "game-report");
        assert!(report["fingerprints"]["engine"].is_string());
        assert!(report["fingerprints"]["resources"].is_string());
        assert!(report["compatibility"]["capabilities"].is_object());
        assert!(report["function_ledger"]["binaries"].is_array());
        assert!(report["call_table"]["calls"].is_array());
    }
}

#[derive(Debug, Clone)]
struct PeReport {
    path: String,
    format: String,
    architecture: String,
    entrypoint_rva: Option<u32>,
    image_base: Option<u64>,
    sections: Vec<Value>,
    imports: Vec<String>,
    exports: Vec<String>,
    functions: Vec<Value>,
    diagnostics: Vec<String>,
}

impl serde::Serialize for PeReport {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut object = Map::new();
        object.insert("path".to_string(), Value::String(self.path.clone()));
        object.insert("format".to_string(), Value::String(self.format.clone()));
        object.insert(
            "architecture".to_string(),
            Value::String(self.architecture.clone()),
        );
        object.insert(
            "entrypoint_rva".to_string(),
            self.entrypoint_rva.map(Value::from).unwrap_or(Value::Null),
        );
        object.insert(
            "image_base".to_string(),
            self.image_base.map(Value::from).unwrap_or(Value::Null),
        );
        object.insert("sections".to_string(), json!(self.sections));
        object.insert("imports".to_string(), json!(self.imports));
        object.insert("exports".to_string(), json!(self.exports));
        object.insert("functions".to_string(), json!(self.functions));
        object.insert("diagnostics".to_string(), json!(self.diagnostics));
        object.serialize(serializer)
    }
}

fn pe_reports(scan: &GameScan) -> Result<Vec<PeReport>> {
    let mut paths = scan
        .resources
        .files()
        .iter()
        .filter(|file| {
            file.relative_path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| {
                    extension.eq_ignore_ascii_case("exe") || extension.eq_ignore_ascii_case("dll")
                })
        })
        .map(|file| file.relative_path.clone())
        .collect::<Vec<_>>();
    paths.sort_by_key(|path| stable_path_key(path));
    Ok(paths
        .into_iter()
        .map(|path| match parse_pe(&scan.root, &path) {
            Ok(report) => report,
            Err(error) => PeReport {
                path: stable_path(&path),
                format: "unreadable".to_string(),
                architecture: "unknown".to_string(),
                entrypoint_rva: None,
                image_base: None,
                sections: Vec::new(),
                imports: Vec::new(),
                exports: Vec::new(),
                functions: Vec::new(),
                diagnostics: vec![error.to_string()],
            },
        })
        .collect())
}

fn parse_pe(root: &Path, relative: &Path) -> Result<PeReport> {
    let path = root.join(relative);
    let mut bytes = Vec::new();
    File::open(&path)
        .with_context(|| format!("open PE candidate `{}`", path.display()))?
        .read_to_end(&mut bytes)?;
    let mut report = PeReport {
        path: stable_path(relative),
        format: "not-pe".to_string(),
        architecture: "unknown".to_string(),
        entrypoint_rva: None,
        image_base: None,
        sections: Vec::new(),
        imports: Vec::new(),
        exports: Vec::new(),
        functions: Vec::new(),
        diagnostics: Vec::new(),
    };
    if bytes.get(..2) != Some(b"MZ") {
        report.diagnostics.push("missing MZ signature".to_string());
        return Ok(report);
    }
    let pe_offset = read_u32(&bytes, 0x3c)
        .map(|value| value as usize)
        .context("PE DOS header is truncated")?;
    if bytes.get(pe_offset..pe_offset + 4) != Some(b"PE\0\0") {
        report.diagnostics.push("missing PE signature".to_string());
        return Ok(report);
    }
    let coff = pe_offset + 4;
    let machine = read_u16(&bytes, coff)?;
    let section_count = read_u16(&bytes, coff + 2)? as usize;
    let optional_size = read_u16(&bytes, coff + 16)? as usize;
    let optional = coff + 20;
    let magic = read_u16(&bytes, optional)?;
    let (format, architecture, directory_base, image_base) = match (magic, machine) {
        (0x10b, 0x14c) => (
            "PE32".to_string(),
            "x86".to_string(),
            optional + 96,
            read_u32(&bytes, optional + 28)? as u64,
        ),
        (0x20b, 0x8664) => (
            "PE32+".to_string(),
            "x86_64".to_string(),
            optional + 112,
            read_u64(&bytes, optional + 24)?,
        ),
        _ => (
            format!("PE({magic:#x})"),
            format!("machine-{machine:#x}"),
            optional + 96,
            0,
        ),
    };
    report.format = format;
    report.architecture = architecture;
    report.entrypoint_rva = Some(read_u32(&bytes, optional + 16)?);
    report.image_base = Some(image_base);
    let section_table = optional + optional_size;
    for index in 0..section_count {
        let offset = section_table + index * 40;
        let name = bytes
            .get(offset..offset + 8)
            .map(|value| {
                String::from_utf8_lossy(value)
                    .trim_matches('\0')
                    .to_string()
            })
            .unwrap_or_else(|| format!("section-{index}"));
        let virtual_size = read_u32(&bytes, offset + 8).unwrap_or_default();
        let virtual_address = read_u32(&bytes, offset + 12).unwrap_or_default();
        let raw_size = read_u32(&bytes, offset + 16).unwrap_or_default();
        let raw_offset = read_u32(&bytes, offset + 20).unwrap_or_default();
        let characteristics = read_u32(&bytes, offset + 36).unwrap_or_default();
        report.sections.push(json!({
            "name": name,
            "rva": virtual_address,
            "virtual_size": virtual_size,
            "raw_offset": raw_offset,
            "raw_size": raw_size,
            "characteristics": format!("{characteristics:#010x}"),
            "executable": characteristics & 0x20000000 != 0,
            "readable": characteristics & 0x40000000 != 0,
            "writable": characteristics & 0x80000000 != 0,
        }));
    }
    let import_directory = directory(&bytes, directory_base, 1);
    let export_directory = directory(&bytes, directory_base, 0);
    report.imports = parse_imports(&bytes, &report.sections, import_directory);
    report.exports = parse_exports(&bytes, &report.sections, export_directory);
    report.functions = report
        .exports
        .iter()
        .enumerate()
        .map(|(index, name)| {
            json!({
                "id": format!("export-{index:04}"),
                "name": name,
                "kind": "export",
                "status": "candidate",
            })
        })
        .collect();
    if report.functions.is_empty() {
        if let Some(entrypoint_rva) = report.entrypoint_rva.filter(|value| *value != 0) {
            report.functions.push(json!({
                "id": "entrypoint",
                "name": "entrypoint",
                "rva": entrypoint_rva,
                "kind": "entrypoint",
                "status": "candidate",
            }));
        }
    }
    Ok(report)
}

fn parse_imports(bytes: &[u8], sections: &[Value], directory: Option<(u32, u32)>) -> Vec<String> {
    let Some((rva, _size)) = directory else {
        return Vec::new();
    };
    let Some(mut offset) = rva_to_offset(bytes, sections, rva) else {
        return Vec::new();
    };
    let mut imports = BTreeSet::new();
    for _ in 0..4096 {
        let original_thunk = read_u32(bytes, offset).unwrap_or_default();
        let name_rva = read_u32(bytes, offset + 12).unwrap_or_default();
        let first_thunk = read_u32(bytes, offset + 16).unwrap_or_default();
        if original_thunk == 0 && name_rva == 0 && first_thunk == 0 {
            break;
        }
        let dll = rva_to_offset(bytes, sections, name_rva)
            .and_then(|name| c_string(bytes, name))
            .unwrap_or_else(|| "unknown.dll".to_string());
        let thunk_rva = if original_thunk != 0 {
            original_thunk
        } else {
            first_thunk
        };
        if let Some(mut thunk) = rva_to_offset(bytes, sections, thunk_rva) {
            for ordinal in 0..4096_u32 {
                let value = read_u32(bytes, thunk).unwrap_or_default();
                if value == 0 {
                    break;
                }
                let name = if value & 0x8000_0000 != 0 {
                    format!("ordinal-{}", value & 0xffff)
                } else {
                    rva_to_offset(bytes, sections, value + 2)
                        .and_then(|name| c_string(bytes, name))
                        .unwrap_or_else(|| format!("hint-name-rva-{value:#x}"))
                };
                imports.insert(format!("{dll}!{name}"));
                thunk += 4;
                if ordinal == 4095 {
                    break;
                }
            }
        }
        offset += 20;
    }
    imports.into_iter().collect()
}

fn parse_exports(bytes: &[u8], sections: &[Value], directory: Option<(u32, u32)>) -> Vec<String> {
    let Some((rva, _size)) = directory else {
        return Vec::new();
    };
    let Some(offset) = rva_to_offset(bytes, sections, rva) else {
        return Vec::new();
    };
    let function_count = read_u32(bytes, offset + 20).unwrap_or_default();
    let name_count = read_u32(bytes, offset + 24).unwrap_or_default();
    let names_rva = read_u32(bytes, offset + 32).unwrap_or_default();
    let mut names = BTreeSet::new();
    if let Some(names_offset) = rva_to_offset(bytes, sections, names_rva) {
        for index in 0..name_count.min(65536) {
            let name_rva = read_u32(bytes, names_offset + index as usize * 4).unwrap_or_default();
            if let Some(name_offset) = rva_to_offset(bytes, sections, name_rva)
                && let Some(name) = c_string(bytes, name_offset)
            {
                names.insert(name);
            }
        }
    }
    if names.is_empty() && function_count > 0 {
        names.insert(format!("ordinal-only-count-{function_count}"));
    }
    names.into_iter().collect()
}

fn directory(bytes: &[u8], base: usize, index: usize) -> Option<(u32, u32)> {
    let offset = base.checked_add(index.checked_mul(8)?)?;
    let rva = read_u32(bytes, offset).ok()?;
    let size = read_u32(bytes, offset + 4).ok()?;
    (rva != 0).then_some((rva, size))
}

fn rva_to_offset(bytes: &[u8], sections: &[Value], rva: u32) -> Option<usize> {
    for section in sections {
        let start = section.get("rva")?.as_u64()? as u32;
        let virtual_size = section.get("virtual_size")?.as_u64()? as u32;
        let raw_size = section.get("raw_size")?.as_u64()? as u32;
        let span = virtual_size.max(raw_size);
        if rva >= start && rva < start.saturating_add(span) {
            let delta = rva - start;
            if delta >= raw_size {
                return None;
            }
            let raw_offset = section.get("raw_offset")?.as_u64()? as usize;
            let offset = raw_offset.checked_add(delta as usize)?;
            return (offset < bytes.len()).then_some(offset);
        }
    }
    ((rva as usize) < bytes.len()).then_some(rva as usize)
}

fn c_string(bytes: &[u8], offset: usize) -> Option<String> {
    let end = bytes.get(offset..)?.iter().position(|byte| *byte == 0)?;
    Some(String::from_utf8_lossy(&bytes[offset..offset + end]).to_string())
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16> {
    let value = bytes
        .get(offset..offset + 2)
        .context("binary is truncated")?;
    Ok(u16::from_le_bytes([value[0], value[1]]))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32> {
    let value = bytes
        .get(offset..offset + 4)
        .context("binary is truncated")?;
    Ok(u32::from_le_bytes([value[0], value[1], value[2], value[3]]))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64> {
    let value = bytes
        .get(offset..offset + 8)
        .context("binary is truncated")?;
    Ok(u64::from_le_bytes([
        value[0], value[1], value[2], value[3], value[4], value[5], value[6], value[7],
    ]))
}

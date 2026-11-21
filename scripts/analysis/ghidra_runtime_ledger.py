"""Run and merge Ghidra headless function ledgers for a static runtime ledger.

This script imports PE files into Ghidra for analysis only. It never launches
the imported executable. The existing Capstone ledger remains the source of
selection and byte-coverage evidence; Ghidra data is attached as independent
function-discovery evidence and is never promoted to semantic implementation.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any
from urllib.parse import unquote, urlparse

from recover_functions import select_workspace_runtime


WORKSPACE = Path(__file__).resolve().parents[2]
DEFAULT_LEDGER = WORKSPACE / "reports" / "runtime_function_coverage_all.json"
DEFAULT_INVENTORY = WORKSPACE / "reports" / "sample_inventory.json"
DEFAULT_GHIDRA_OUTPUT = (
    WORKSPACE / "archive" / "2026-11" / "runtime_function_ghidra"
)
DEFAULT_OUTPUT_JSON = WORKSPACE / "reports" / "runtime_function_coverage_all_ghidra.json"
DEFAULT_OUTPUT_MD = WORKSPACE / "reports" / "runtime_function_coverage_all_ghidra.md"
EXPORT_SCRIPT = Path(__file__).with_name("ExportGhidraLedger.java")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Merge Ghidra headless function evidence into a runtime ledger."
    )
    parser.add_argument("--ledger", type=Path, default=DEFAULT_LEDGER)
    parser.add_argument("--inventory", type=Path, default=DEFAULT_INVENTORY)
    parser.add_argument(
        "--ghidra-home",
        type=Path,
        default=Path(
            os.environ.get(
                "GHIDRA_HOME",
                r"C:\Users\jingyue\AppData\Local\ghidra_11.0.3_PUBLIC",
            )
        ),
    )
    parser.add_argument("--ghidra-dir", type=Path, default=DEFAULT_GHIDRA_OUTPUT)
    parser.add_argument(
        "--ghidra-export-dir",
        type=Path,
        default=WORKSPACE / "reports",
        help="Directory containing previously exported Ghidra JSON files.",
    )
    parser.add_argument(
        "--ghidra-export",
        action="append",
        type=Path,
        default=[],
        help="Add an existing one-program Ghidra JSON export for merging.",
    )
    parser.add_argument("--output-json", type=Path, default=DEFAULT_OUTPUT_JSON)
    parser.add_argument("--output-md", type=Path, default=DEFAULT_OUTPUT_MD)
    parser.add_argument(
        "--run-headless",
        action="store_true",
        help="Run Ghidra headless for every ledger binary before merging.",
    )
    parser.add_argument(
        "--timeout",
        type=int,
        default=600,
        help="Per-binary Ghidra analysis timeout in seconds.",
    )
    parser.add_argument(
        "--ghidra-analysis-mode",
        choices=("full-analysis", "import-only", "unknown"),
        default="import-only",
        help="Analysis level represented by supplied Ghidra exports.",
    )
    parser.add_argument(
        "--binary-sha256",
        action="append",
        default=[],
        help="Restrict headless processing to a selected binary SHA-256; repeatable.",
    )
    parser.add_argument(
        "--headless-input-dir",
        type=Path,
        help=(
            "Optional directory of byte-identical analysis copies. Files are "
            "selected by SHA-256; source game folders remain read-only."
        ),
    )
    return parser.parse_args()


def workspace_relative(path: Path) -> str:
    try:
        return str(path.resolve().relative_to(WORKSPACE)).replace("\\", "/")
    except ValueError:
        return str(path.resolve())


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def safe_key(binary: dict[str, Any]) -> str:
    digest = hashlib.sha256(
        f"{binary['sample_id']}:{binary['binary_path']}:{binary['sha256']}".encode(
            "utf-8"
        )
    ).hexdigest()[:16]
    name = Path(binary["binary_path"]).name
    return f"{digest}_{name}"


def find_ghidra_export(directory: Path, binary: dict[str, Any]) -> Path | None:
    expected = directory / f"{Path(binary['binary_path']).name}.json"
    if expected.is_file():
        return expected
    candidates = sorted(directory.glob("*.json"))
    if len(candidates) == 1:
        return candidates[0]
    return None


def load_selection(inventory_path: Path) -> tuple[dict[str, dict[str, Any]], list[dict[str, Any]]]:
    inventory = json.loads(inventory_path.read_text(encoding="utf-8"))
    selected_by_path: dict[str, dict[str, Any]] = {}
    audit = []
    for sample in inventory.get("samples", []):
        selected, sample_audit = select_workspace_runtime(sample)
        audit.extend(sample_audit)
        for binary in selected:
            selected_by_path[binary["path"]] = {
                "sample_id": sample["sample_id"],
                "sample_name": sample["name"],
                **binary,
            }
    return selected_by_path, audit


def load_external_exports(
    paths: list[Path],
) -> tuple[dict[str, dict[str, Any]], dict[str, dict[str, Any]]]:
    result = {}
    by_sha = {}
    for path in paths:
        try:
            if path.stat().st_size > 1_000_000:
                continue
        except OSError:
            continue
        try:
            data = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, ValueError, json.JSONDecodeError):
            continue
        executable_path = data.get("executable_path")
        if executable_path:
            parsed = urlparse(executable_path)
            source_path = (
                Path(unquote(parsed.path.lstrip("/")))
                if parsed.scheme == "file"
                else Path(executable_path)
            )
            if parsed.scheme == "file" and parsed.netloc:
                source_path = Path(
                    "\\\\" + parsed.netloc + unquote(parsed.path).replace("/", "\\")
                )
            elif (
                parsed.scheme == ""
                and re.match(r"^/[A-Za-z]:/", executable_path)
            ):
                source_path = Path(executable_path[1:])
            if source_path.is_file():
                by_sha[sha256_file(source_path)] = {
                    **data,
                    "_export_path": str(path),
                }
            result[os.path.normcase(os.path.abspath(str(source_path)))] = {
                **data,
                "_export_path": str(path),
            }
    return result, by_sha


def run_headless_for_binary(
    binary: dict[str, Any],
    ghidra_home: Path,
    output_dir: Path,
    timeout: int,
    input_by_sha256: dict[str, Path] | None = None,
) -> dict[str, Any]:
    path = WORKSPACE / Path(binary["binary_path"].replace("/", os.sep))
    result: dict[str, Any] = {
        "status": "not-run",
        "binary_path": binary["binary_path"],
        "sha256": binary["sha256"],
        "command": [],
        "diagnostics": [],
        "export_path": None,
    }
    if input_by_sha256 and binary["sha256"] in input_by_sha256:
        path = input_by_sha256[binary["sha256"]]
    if not path.is_file():
        result["status"] = "error"
        result["diagnostics"].append("binary path does not exist")
        return result
    actual_sha = sha256_file(path)
    if actual_sha != binary["sha256"]:
        result["status"] = "error"
        result["diagnostics"].append(
            f"SHA-256 mismatch: inventory={binary['sha256']} actual={actual_sha}"
        )
        return result

    analyze = ghidra_home / "support" / "analyzeHeadless.bat"
    if not analyze.is_file():
        result["status"] = "unavailable"
        result["diagnostics"].append(f"analyzeHeadless.bat not found: {analyze}")
        return result
    if not EXPORT_SCRIPT.is_file():
        result["status"] = "error"
        result["diagnostics"].append(f"export script not found: {EXPORT_SCRIPT}")
        return result

    binary_key = safe_key(binary)
    result_dir = output_dir / binary_key
    result_dir.mkdir(parents=True, exist_ok=True)
    project_dir = Path(tempfile.mkdtemp(prefix="ghidra_", dir=str(output_dir)))
    project_name = "RuntimeLedger"
    command = [
        str(analyze),
        str(project_dir),
        project_name,
        "-import",
        str(path),
        "-scriptPath",
        str(EXPORT_SCRIPT.parent),
        "-postScript",
        EXPORT_SCRIPT.name,
        str(result_dir),
        "-analysisTimeoutPerFile",
        str(timeout),
        "-overwrite",
        "-deleteProject",
    ]
    result["command"] = command
    try:
        completed = subprocess.run(
            command,
            cwd=str(WORKSPACE),
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=timeout + 120,
            check=False,
        )
    except subprocess.TimeoutExpired as exc:
        result["status"] = "timeout"
        result["diagnostics"].append(f"Ghidra timeout after {timeout + 120}s")
        result["stdout_tail"] = (exc.stdout or "")[-4000:]
        result["stderr_tail"] = (exc.stderr or "")[-4000:]
        return result
    finally:
        shutil.rmtree(project_dir, ignore_errors=True)

    result["returncode"] = completed.returncode
    result["stdout_tail"] = completed.stdout[-4000:]
    result["stderr_tail"] = completed.stderr[-4000:]
    exported = find_ghidra_export(result_dir, binary)
    if completed.returncode != 0:
        result["status"] = "error"
        result["diagnostics"].append(
            f"analyzeHeadless exited with code {completed.returncode}"
        )
    elif exported is None:
        result["status"] = "error"
        result["diagnostics"].append("postScript completed without a JSON export")
    else:
        result["status"] = "ok"
        result["export_path"] = workspace_relative(exported)
    return result


def parse_ghidra_address(value: str, image_base: int) -> int | None:
    match = re.search(r"([0-9A-Fa-f]+)$", value or "")
    if match is None:
        return None
    try:
        address = int(match.group(1), 16)
    except ValueError:
        return None
    return address - image_base if address >= image_base else address


def merge_binary(
    binary: dict[str, Any],
    execution: dict[str, Any],
) -> dict[str, Any]:
    merged = {
        **binary,
        "sample_id": binary.get("sample_id"),
        "sample_name": binary.get("sample_name"),
        "binary_path": binary["binary_path"],
        "sha256": binary["sha256"],
        "capstone_status": binary.get("capstone_status", "available"),
        "ghidra_status": execution["status"],
        "ghidra_diagnostics": execution.get("diagnostics", []),
        "ghidra_export_path": execution.get("export_path"),
        "ghidra_function_count": 0,
        "ghidra_named_function_count": 0,
        "ghidra_decompile_count": 0,
        "ghidra_functions": [],
    }
    export_path = execution.get("export_path")
    if not export_path:
        return merged
    export_file = WORKSPACE / export_path
    try:
        data = json.loads(export_file.read_text(encoding="utf-8"))
    except (OSError, ValueError, json.JSONDecodeError) as exc:
        merged["ghidra_status"] = "invalid-export"
        merged["ghidra_diagnostics"].append(f"cannot read export: {exc}")
        return merged

    image_base = int(binary.get("pe_metadata", {}).get("image_base", 0))
    functions = []
    for function in data.get("functions", []):
        entry_rva = parse_ghidra_address(function.get("entry", ""), image_base)
        end_rva = parse_ghidra_address(function.get("end", ""), image_base)
        functions.append(
            {
                **function,
                "entry_rva": entry_rva,
                "end_rva": end_rva,
            }
        )
    merged["ghidra_program"] = data.get("program")
    merged["ghidra_executable_path"] = data.get("executable_path")
    merged["ghidra_language"] = data.get("language")
    merged["ghidra_compiler"] = data.get("compiler")
    merged["ghidra_function_count"] = len(functions)
    merged["ghidra_named_function_count"] = sum(
        not str(function.get("name", "")).casefold().startswith("fun_")
        for function in functions
    )
    merged["ghidra_decompile_count"] = sum(
        bool(function.get("decompile")) for function in functions
    )
    merged["ghidra_functions"] = functions
    candidate_rvas = {
        int(function["rva"])
        for function in binary.get("function_candidates", [])
    }
    ghidra_rvas = {
        int(function["entry_rva"])
        for function in functions
        if function.get("entry_rva") is not None
    }
    merged["candidate_entry_matches"] = len(candidate_rvas & ghidra_rvas)
    merged["candidate_entry_match_percent"] = round(
        len(candidate_rvas & ghidra_rvas) * 100 / len(candidate_rvas),
        4,
    ) if candidate_rvas else 0.0
    merged["ghidra_only_entry_count"] = len(ghidra_rvas - candidate_rvas)
    return merged


def render_markdown(payload: dict[str, Any]) -> str:
    scope = payload["scope"]
    lines = [
        "# Runtime Function Ghidra Integration",
        "",
        "This report merges Ghidra headless function discovery with the existing "
        "Capstone candidate ledger. Ghidra functions remain static evidence and "
        "are not semantic implementations.",
        "",
        f"- Source ledger: `{payload['source_ledger']}`",
        f"- Binary count: `{scope['binary_count']}`",
        f"- Ghidra succeeded: `{scope['ghidra_ok_count']}`",
        f"- Ghidra failed or unavailable: `{scope['ghidra_failed_count']}`",
        f"- Ghidra analysis mode: `{payload['method']['ghidra_analysis_mode']}`",
        f"- Capstone records available: `{scope['capstone_analyzed_binary_count']}`",
        f"- Selected binaries missing Capstone records: `{scope['capstone_missing_binary_count']}`",
        f"- Capstone candidates: `{scope['capstone_candidate_count']}`",
        f"- Semantic implementations claimed: `0`",
        "",
        "## Per-Binary Results",
        "",
        "| Binary | Ghidra | Ghidra functions | Named | Decompilations | Candidate entry match |",
        "| --- | --- | ---: | ---: | ---: | ---: |",
    ]
    for binary in payload["binaries"]:
        lines.append(
            f"| `{binary['binary_path']}` | `{binary['ghidra_status']}` | "
            f"{binary.get('ghidra_function_count', 0)} | "
            f"{binary.get('ghidra_named_function_count', 0)} | "
            f"{binary.get('ghidra_decompile_count', 0)} | "
            f"{binary.get('candidate_entry_match_percent', 0)}% |"
        )
    lines.extend(
        [
            "",
            "Ghidra function entry RVAs and Capstone candidate entry RVAs are "
            "stored in the JSON for independent review. A matching entry address "
            "does not confirm a boundary or function semantics.",
        ]
    )
    return "\n".join(lines) + "\n"


def main() -> int:
    args = parse_args()
    ledger_path = args.ledger.resolve()
    payload = json.loads(ledger_path.read_text(encoding="utf-8"))
    ghidra_dir = args.ghidra_dir.resolve()
    ghidra_dir.mkdir(parents=True, exist_ok=True)
    selected_by_path, selection_audit = load_selection(args.inventory.resolve())
    source_by_path = {
        binary["binary_path"]: binary
        for binary in payload.get("binaries", [])
    }
    selected_records = []
    for path, selected in selected_by_path.items():
        source = source_by_path.get(path)
        pe_metadata = {}
        if source is None:
            inventory_pe = selected.get("pe", {})
            image_base = inventory_pe.get("image_base", 0)
            entrypoint_rva = inventory_pe.get("entrypoint_rva", 0)
            pe_metadata = {
                "image_base": image_base,
                "entrypoint_rva": entrypoint_rva,
                "entrypoint_va": image_base + entrypoint_rva,
                "architecture": inventory_pe.get("architecture"),
                "machine": inventory_pe.get("machine"),
            }
        record = {
            **selected,
            **(source or {}),
            "binary_path": path,
            "sample_id": selected["sample_id"],
            "sample_name": selected["sample_name"],
            "capstone_status": (
                "available" if source is not None else "missing-from-source-report"
            ),
            "pe_metadata": (
                source.get("pe_metadata", {})
                if source is not None
                else pe_metadata
            ),
            "selection_class": selected.get("selection_class"),
            "selection_reason": selected.get("selection_reason"),
        }
        selected_records.append(record)

    if args.binary_sha256:
        requested_hashes = {value.lower() for value in args.binary_sha256}
        selected_records = [
            binary
            for binary in selected_records
            if str(binary.get("sha256", "")).lower() in requested_hashes
        ]
        found_hashes = {str(binary.get("sha256", "")).lower() for binary in selected_records}
        missing_hashes = sorted(requested_hashes - found_hashes)
        if missing_hashes:
            raise ValueError(
                "requested binary SHA-256 not present in selected runtime ledger: "
                + ", ".join(missing_hashes)
            )

    input_by_sha256: dict[str, Path] | None = None
    if args.headless_input_dir:
        input_dir = args.headless_input_dir.resolve()
        if not input_dir.is_dir():
            raise ValueError(f"headless input directory does not exist: {input_dir}")
        wanted = {str(binary["sha256"]).lower() for binary in selected_records}
        input_by_sha256 = {}
        for candidate in input_dir.rglob("*"):
            if not candidate.is_file():
                continue
            try:
                digest = sha256_file(candidate)
            except OSError:
                continue
            if digest.lower() in wanted:
                input_by_sha256[digest.lower()] = candidate
        absent_inputs = sorted(wanted - set(input_by_sha256))
        if absent_inputs:
            raise ValueError(
                "byte-identical headless input copy not found for SHA-256: "
                + ", ".join(absent_inputs)
            )

    export_paths = [path.resolve() for path in args.ghidra_export]
    if args.ghidra_export_dir.is_dir():
        export_paths.extend(args.ghidra_export_dir.resolve().glob("*.json"))
    external_exports, external_exports_by_sha = load_external_exports(export_paths)
    executions = []
    for binary in selected_records:
        if args.run_headless:
            execution = run_headless_for_binary(
                binary,
                args.ghidra_home.resolve(),
                ghidra_dir,
                args.timeout,
                input_by_sha256,
            )
        else:
            export = find_ghidra_export(
                ghidra_dir / safe_key(binary),
                binary,
            )
            execution = {
                "status": "ok" if export else "not-run",
                "binary_path": binary["binary_path"],
                "sha256": binary["sha256"],
                "export_path": workspace_relative(export) if export else None,
                "diagnostics": [],
            }
            normalized_path = os.path.normcase(
                os.path.abspath(
                    WORKSPACE
                    / Path(binary["binary_path"].replace("/", os.sep))
                )
            )
            external = external_exports.get(
                normalized_path
            ) or external_exports_by_sha.get(binary["sha256"])
            if external is not None:
                execution["status"] = "ok"
                execution["external_export"] = external
                export_source = Path(external["_export_path"]).resolve()
                execution["external_export_path"] = workspace_relative(export_source)
                execution["export_path"] = workspace_relative(export_source)
        executions.append(execution)

    execution_by_path = {
        execution["binary_path"]: execution for execution in executions
    }
    merged_binaries = [
        merge_binary(binary, execution_by_path[binary["binary_path"]])
        for binary in selected_records
    ]
    for binary in merged_binaries:
        execution = execution_by_path[binary["binary_path"]]
        external = execution.get("external_export")
        if external is not None and not binary["ghidra_functions"]:
            binary.update(merge_binary(binary, execution))
            binary["ghidra_export_path"] = execution.get("external_export_path")
        binary["ghidra_analysis_mode"] = args.ghidra_analysis_mode

    candidate_count = sum(
        len(binary.get("function_candidates", [])) for binary in merged_binaries
    )
    raw_bytes = sum(
        binary.get("static_coverage", {}).get("executable_raw_bytes", 0)
        for binary in merged_binaries
    )
    decoded_bytes = sum(
        binary.get("static_coverage", {}).get("decoded_instruction_bytes", 0)
        for binary in merged_binaries
    )
    candidate_bytes = sum(
        binary.get("static_coverage", {}).get("candidate_instruction_bytes", 0)
        for binary in merged_binaries
    )
    result = {
        "schema_version": "1.0.0",
        "status": "static-evidence-only",
        "source_ledger": workspace_relative(ledger_path),
        "ghidra_home": workspace_relative(args.ghidra_home.resolve()),
        "ghidra_export_dir": workspace_relative(ghidra_dir),
        "scope": {
            "binary_count": len(merged_binaries),
            "sample_count": len(
                {item["sample_id"] for item in merged_binaries}
            ),
            "game_sample_count": len(
                {
                    item["sample_id"]
                    for item in merged_binaries
                    if item["sample_id"] != "cs2_full_v401"
                }
            ),
            "reference_sample_count": int(
                any(
                    item["sample_id"] == "cs2_full_v401"
                    for item in merged_binaries
                )
            ),
            "selection_audit_count": len(selection_audit),
            "selection_excluded_count": sum(
                not item["selected"] for item in selection_audit
            ),
            "capstone_candidate_count": candidate_count,
            "capstone_analyzed_binary_count": sum(
                item["capstone_status"] == "available"
                for item in merged_binaries
            ),
            "capstone_missing_binary_count": sum(
                item["capstone_status"] != "available"
                for item in merged_binaries
            ),
            "executable_raw_bytes": raw_bytes,
            "decoded_instruction_bytes": decoded_bytes,
            "decoded_instruction_coverage_percent": round(
                decoded_bytes * 100 / raw_bytes, 4
            )
            if raw_bytes
            else 0.0,
            "candidate_instruction_bytes": candidate_bytes,
            "candidate_instruction_coverage_percent": round(
                candidate_bytes * 100 / raw_bytes, 4
            )
            if raw_bytes
            else 0.0,
            "ghidra_ok_count": sum(
                item["ghidra_status"] == "ok" for item in merged_binaries
            ),
            "ghidra_failed_count": sum(
                item["ghidra_status"] != "ok" for item in merged_binaries
            ),
            "semantic_implementation_count": 0,
            "semantic_implementation_claim": False,
        },
        "selection": {
            "mode": "all-runtime-reviewed",
            "selected_binary_count": len(selected_records),
            "selected_by_class": {
                selection_class: sum(
                    item.get("selection_class") == selection_class
                    for item in selected_records
                )
                for selection_class in sorted(
                    {
                        item.get("selection_class", "unspecified")
                        for item in selected_records
                    }
                )
            },
            "audit": selection_audit,
        },
        "method": {
            "ghidra": (
                "Ghidra headless "
                + args.ghidra_analysis_mode
                + " run and ExportGhidraLedger postScript"
            ),
            "ghidra_analysis_mode": args.ghidra_analysis_mode,
            "capstone_source": workspace_relative(ledger_path),
            "merge_key": "sample_id + binary_path + sha256",
            "interpretation": (
                "Ghidra function discovery and decompilation excerpts are "
                "independent static evidence; no semantic implementation status "
                "is inferred."
            ),
        },
        "binaries": merged_binaries,
    }
    output_json = args.output_json.resolve()
    output_md = args.output_md.resolve()
    output_json.parent.mkdir(parents=True, exist_ok=True)
    output_md.parent.mkdir(parents=True, exist_ok=True)
    output_json.write_text(
        json.dumps(result, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
        newline="\n",
    )
    output_md.write_text(
        render_markdown(result),
        encoding="utf-8",
        newline="\n",
    )
    print(f"Wrote {output_json}")
    print(f"Wrote {output_md}")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, ValueError, KeyError, json.JSONDecodeError) as exc:
        print(f"ghidra_runtime_ledger.py: {exc}", file=sys.stderr)
        raise SystemExit(2)

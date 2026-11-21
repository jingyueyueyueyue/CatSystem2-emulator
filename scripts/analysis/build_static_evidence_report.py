"""Build stable second-batch static evidence reports for all workspace samples.

This report generator reads existing read-only evidence:

* reports/all_sample_content_audit.json
* reports/sample_inventory.json
* reports/runtime_function_coverage_all_ghidra.json

It does not inspect or start a game executable and does not modify simulator
source.  Function boundaries, call edges, Ghidra output, opcode sightings,
and native sightings retain evidence-only status in the generated schema.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
from collections import Counter, defaultdict
from datetime import datetime, timezone
from pathlib import Path
from typing import Any


WORKSPACE = Path(__file__).resolve().parents[2]
DEFAULT_CONTENT = WORKSPACE / "reports" / "all_sample_content_audit.json"
DEFAULT_INVENTORY = WORKSPACE / "reports" / "sample_inventory.json"
DEFAULT_LEDGER = WORKSPACE / "reports" / "runtime_function_coverage_all_ghidra.json"
DEFAULT_OUTPUT = WORKSPACE / "reports" / "static_evidence_report.json"
DEFAULT_MARKDOWN = WORKSPACE / "reports" / "static_evidence_report.md"
DEFAULT_VERSION_OUTPUT = WORKSPACE / "reports" / "version_diff_report.json"
DEFAULT_VERSION_MARKDOWN = WORKSPACE / "reports" / "version_diff_report.md"
ARCHIVE_ROOT = (WORKSPACE / "archive" / "2026-11").resolve()


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Build all-sample static evidence and version-difference reports."
    )
    parser.add_argument("--content", type=Path, default=DEFAULT_CONTENT)
    parser.add_argument("--inventory", type=Path, default=DEFAULT_INVENTORY)
    parser.add_argument("--ledger", type=Path, default=DEFAULT_LEDGER)
    parser.add_argument(
        "--ghidra-overlay",
        action="append",
        type=Path,
        default=[],
        help="Overlay a newer Ghidra merge report, matching binaries by SHA-256.",
    )
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    parser.add_argument("--markdown", type=Path, default=DEFAULT_MARKDOWN)
    parser.add_argument("--version-output", type=Path, default=DEFAULT_VERSION_OUTPUT)
    parser.add_argument(
        "--version-markdown", type=Path, default=DEFAULT_VERSION_MARKDOWN
    )
    parser.add_argument(
        "--archive-status",
        type=Path,
        default=ARCHIVE_ROOT / "20261116_static_evidence_status.json",
    )
    return parser.parse_args()


def read_json(path: Path) -> dict[str, Any]:
    return json.loads(path.read_text(encoding="utf-8"))


def write_json(path: Path, value: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        json.dumps(value, ensure_ascii=False, indent=2, sort_keys=False) + "\n",
        encoding="utf-8",
        newline="\n",
    )


def write_text(path: Path, value: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(value, encoding="utf-8", newline="\n")


def workspace_relative(path: Path) -> str:
    try:
        return str(path.resolve().relative_to(WORKSPACE)).replace("\\", "/")
    except ValueError:
        return str(path.resolve())


def canonical_path(value: str | None) -> str:
    if not value:
        return ""
    return str(Path(value.replace("/", "\\"))).casefold()


def resolved_inventory_root(value: str, workspace: Path) -> str:
    path = Path(value)
    if not path.is_absolute():
        path = workspace / path
    return canonical_path(str(path.resolve()))


def stable_sample_id(sample: dict[str, Any]) -> str:
    value = sample.get("sample_id")
    if value:
        return str(value)
    root = str(sample.get("root", ""))
    return hashlib.sha256(root.encode("utf-8")).hexdigest()[:16]


def relative_root(root: str, workspace: Path) -> str:
    try:
        return str(Path(root).resolve().relative_to(workspace.resolve())).replace(
            "\\", "/"
        )
    except ValueError:
        return root.replace("\\", "/")


def parse_hex_key(value: str) -> str:
    match = re.search(r"0x[0-9a-fA-F]+", str(value))
    return match.group(0).lower() if match else str(value)


def primary_binary(
    sample: dict[str, Any], ledger_by_sample: dict[str, list[dict[str, Any]]]
) -> dict[str, Any] | None:
    binaries = ledger_by_sample.get(stable_sample_id(sample), [])
    preferred_roles = (
        "primary-runtime-executable",
        "runtime-variant-executable",
    )
    for role in preferred_roles:
        candidates = [item for item in binaries if item.get("runtime_role") == role]
        if candidates:
            return sorted(candidates, key=lambda item: item["binary_path"])[0]
    return sorted(binaries, key=lambda item: item["binary_path"])[0] if binaries else None


def inventory_binary(
    sample: dict[str, Any], binary_path: str | None
) -> dict[str, Any] | None:
    if not binary_path:
        return None
    wanted = canonical_path(binary_path)
    for binary in sample.get("binaries", []):
        if canonical_path(binary.get("path")) == wanted:
            return binary
    return None


def runtime_fingerprint(
    sample: dict[str, Any],
    primary: dict[str, Any] | None,
    inventory: dict[str, Any] | None,
    code_identity: dict[str, Any] | None = None,
) -> dict[str, Any]:
    pe = (inventory or {}).get("pe", {})
    metadata = (primary or {}).get("pe_metadata", {})
    fingerprint: dict[str, Any] = {
        "status": "inventory-and-static-ledger",
        "evidence_level": "static",
        "binary_path": (primary or {}).get("binary_path"),
        "sha256": (primary or {}).get("sha256")
        or (inventory or {}).get("sha256"),
        "size": (inventory or {}).get("size"),
        "machine": pe.get("machine") or metadata.get("machine"),
        "architecture": pe.get("architecture") or metadata.get("architecture"),
        "image_base": pe.get("image_base") or metadata.get("image_base"),
        "entrypoint_rva": pe.get("entrypoint_rva") or metadata.get("entrypoint_rva"),
        "entrypoint_virtual_address": pe.get("entrypoint_virtual_address")
        or metadata.get("entrypoint_va"),
        "size_of_image": metadata.get("size_of_image"),
        "ghidra_status": (primary or {}).get("ghidra_status", "not-selected"),
        "ghidra_analysis_mode": (primary or {}).get(
            "ghidra_analysis_mode", "not-available"
        ),
        "candidate_status": "static-candidate-only",
    }
    if code_identity:
        fingerprint["code_identity"] = code_identity
    if not fingerprint["sha256"]:
        fingerprint["status"] = "missing-primary-runtime"
    return fingerprint


def normalize_counts(value: dict[str, Any] | None) -> dict[str, int]:
    return {
        str(key): int(item)
        for key, item in (value or {}).items()
        if isinstance(item, (int, float))
    }


def diagnostic_categories(diagnostics: list[dict[str, Any]]) -> dict[str, int]:
    categories: Counter[str] = Counter()
    for diagnostic in diagnostics:
        message = str(diagnostic.get("message", ""))
        lowered = message.casefold()
        if "deflate" in lowered or "compressed" in lowered:
            category = "compressed-payload"
        elif "range" in lowered or "record" in lowered:
            category = "structured-range"
        elif "anm" in lowered or "animation" in lowered:
            category = "animation-layout"
        elif "fes" in lowered or "macro" in lowered:
            category = "fes-syntax"
        else:
            category = "other"
        categories[category] += 1
    return dict(sorted(categories.items()))


def diagnostic_format(message: str) -> str:
    resource = message.rsplit("::", 1)[-1]
    match = re.search(r"\.([A-Za-z0-9]+)$", resource)
    return match.group(1).lower() if match else "unclassified"


def format_rows(
    games: list[dict[str, Any]],
) -> tuple[list[dict[str, Any]], dict[str, dict[str, int]]]:
    by_format: dict[str, dict[str, int]] = defaultdict(
        lambda: {
            "games_observed": 0,
            "loose_files": 0,
            "archive_entries": 0,
            "parse_failures": 0,
            "diagnostics": 0,
        }
    )
    game_rows = []
    for game in games:
        format_counts = game.get("format_counts", {})
        diagnostic_counts = Counter(
            diagnostic_format(str(item.get("resource", "")))
            for item in game.get("diagnostics", [])
        )
        formats = sorted(format_counts)
        for extension in formats:
            row = by_format[extension]
            row["games_observed"] += 1
            row["loose_files"] += int(
                format_counts[extension].get("loose_files", 0)
            )
            row["archive_entries"] += int(
                format_counts[extension].get("archive_entries", 0)
            )
            row["parse_failures"] += diagnostic_counts.get(extension, 0)
            row["diagnostics"] += diagnostic_counts.get(extension, 0)
        game_rows.append(
            {
                "sample_id": game["sample_id"],
                "formats": [
                    {
                        "extension": extension,
                        "loose_files": int(
                            format_counts[extension].get("loose_files", 0)
                        ),
                        "archive_entries": int(
                            format_counts[extension].get("archive_entries", 0)
                        ),
                        "observed_total": int(
                            format_counts[extension].get("observed_total", 0)
                        ),
                    }
                    for extension in formats
                ],
            }
        )
    return game_rows, {key: dict(value) for key, value in sorted(by_format.items())}


def summarize_opcodes(
    games: list[dict[str, Any]],
) -> tuple[list[dict[str, Any]], list[dict[str, Any]]]:
    aggregate: dict[str, dict[str, Any]] = {}
    per_game = []
    for game in games:
        counts = normalize_counts(game.get("opcode_counts"))
        per_game.append(
            {
                "sample_id": game["sample_id"],
                "distinct_count": len(counts),
                "occurrence_count": sum(counts.values()),
                "observed_opcodes": sorted(counts),
                "evidence_status": "sample-observed-static-script",
            }
        )
        for opcode, count in counts.items():
            item = aggregate.setdefault(
                opcode,
                {
                    "opcode": opcode,
                    "sample_count": 0,
                    "occurrence_count": 0,
                    "samples": [],
                    "evidence_status": "sample-observed-static-script",
                },
            )
            item["sample_count"] += 1
            item["occurrence_count"] += count
            item["samples"].append(game["sample_id"])
    for item in aggregate.values():
        item["samples"].sort()
    return per_game, [aggregate[key] for key in sorted(aggregate)]


def summarize_natives(
    games: list[dict[str, Any]],
) -> tuple[list[dict[str, Any]], list[dict[str, Any]]]:
    aggregate: dict[str, dict[str, Any]] = {}
    per_game = []
    for game in games:
        raw = game.get("native_details", {})
        native_ids = sorted(str(key) for key in raw)
        per_game.append(
            {
                "sample_id": game["sample_id"],
                "distinct_count": len(native_ids),
                "occurrence_count": sum(
                    int(raw[key].get("call_sites", 0)) for key in raw
                ),
                "observed_native_indices": native_ids,
                "evidence_status": "sample-observed-static-script",
            }
        )
        for native_id in native_ids:
            value = raw[native_id]
            item = aggregate.setdefault(
                native_id,
                {
                    "native_index": int(native_id),
                    "sample_count": 0,
                    "call_site_count": 0,
                    "argument_byte_counts": Counter(),
                    "samples": [],
                    "evidence_status": "sample-observed-static-script",
                    "semantic_status": "unverified",
                },
            )
            item["sample_count"] += 1
            item["call_site_count"] += int(value.get("call_sites", 0))
            for width, count in (value.get("argument_bytes") or {}).items():
                item["argument_byte_counts"][str(width)] += int(count)
            item["samples"].append(game["sample_id"])
    rows = []
    for key in sorted(aggregate, key=lambda item: int(item)):
        item = aggregate[key]
        rows.append(
            {
                **item,
                "argument_byte_counts": dict(
                    sorted(item["argument_byte_counts"].items(), key=lambda pair: int(pair[0]))
                ),
            }
        )
    return per_game, rows


def ledger_rows(
    samples: list[dict[str, Any]],
    ledger_by_sample: dict[str, list[dict[str, Any]]],
) -> tuple[list[dict[str, Any]], dict[str, int]]:
    per_game = []
    totals = Counter()
    for sample in samples:
        binaries = ledger_by_sample.get(sample["sample_id"], [])
        rows = []
        for binary in sorted(binaries, key=lambda item: item["binary_path"]):
            summary = binary.get("candidate_summary", {})
            row = {
                "binary_path": binary["binary_path"],
                "sha256": binary.get("sha256"),
                "runtime_role": binary.get("runtime_role"),
                "selection_class": binary.get("selection_class"),
                "ghidra_status": binary.get("ghidra_status"),
                "ghidra_analysis_mode": binary.get("ghidra_analysis_mode"),
                "candidate_count": int(summary.get("candidate_count", 0)),
                "entrypoint_reachable_count": int(
                    summary.get("entrypoint_reachable_count", 0)
                ),
                "direct_call_edge_count": int(
                    summary.get("direct_call_edge_count", 0)
                ),
                "resolved_call_edge_count": int(
                    summary.get("resolved_call_edge_count", 0)
                ),
                "ghidra_function_count": int(binary.get("ghidra_function_count", 0)),
                "candidate_entry_matches": int(
                    binary.get("candidate_entry_matches", 0)
                ),
                "candidate_entry_match_percent": float(
                    binary.get("candidate_entry_match_percent", 0.0)
                ),
                "evidence_status": "static-candidate-only",
                "semantic_status": "unverified",
            }
            rows.append(row)
            totals["candidate_count"] += row["candidate_count"]
            totals["entrypoint_reachable_count"] += row["entrypoint_reachable_count"]
            totals["direct_call_edge_count"] += row["direct_call_edge_count"]
            totals["resolved_call_edge_count"] += row["resolved_call_edge_count"]
            totals["ghidra_ok_binary_count"] += row["ghidra_status"] == "ok"
        per_game.append({"sample_id": sample["sample_id"], "binaries": rows})
    return per_game, dict(totals)


def build_version_diffs(
    samples: list[dict[str, Any]],
    primary_by_sample: dict[str, dict[str, Any] | None],
    game_rows: dict[str, dict[str, Any]],
    ledger_by_sample: dict[str, list[dict[str, Any]]],
    baseline_id: str,
    binary_fingerprints: dict[str, dict[str, Any]],
) -> dict[str, Any]:
    baseline = game_rows[baseline_id]
    diffs = []
    for sample in samples:
        sample_id = sample["sample_id"]
        if sample_id == baseline_id:
            continue
        current = game_rows[sample_id]
        primary = primary_by_sample.get(sample_id) or {}
        baseline_primary = primary_by_sample.get(baseline_id) or {}
        baseline_opcodes = set(baseline["opcode_counts"])
        current_opcodes = set(current["opcode_counts"])
        baseline_natives = set(baseline["native_counts"])
        current_natives = set(current["native_counts"])
        baseline_formats = set(baseline["format_counts"])
        current_formats = set(current["format_counts"])
        primary_summary = primary.get("candidate_summary", {})
        baseline_summary = baseline_primary.get("candidate_summary", {})
        diffs.append(
            {
                "sample_id": sample_id,
                "baseline_sample_id": baseline_id,
                "variant": current["variant"],
                "content_fingerprint": current["content_fingerprint"],
                "runtime_sha256": current["runtime_fingerprint"].get("sha256"),
                "runtime_identity": {
                    "same_sha256_as_baseline": current["runtime_fingerprint"].get("sha256")
                    == baseline["runtime_fingerprint"].get("sha256"),
                    "same_image_base": current["runtime_fingerprint"].get("image_base")
                    == baseline["runtime_fingerprint"].get("image_base"),
                    "same_entrypoint_rva": current["runtime_fingerprint"].get(
                        "entrypoint_rva"
                    )
                    == baseline["runtime_fingerprint"].get("entrypoint_rva"),
                    "comparison_status": "static-pe-metadata-only",
                },
                "format_delta": {
                    "added": sorted(current_formats - baseline_formats),
                    "removed": sorted(baseline_formats - current_formats),
                    "count_delta": {
                        key: current["format_counts"].get(key, 0)
                        - baseline["format_counts"].get(key, 0)
                        for key in sorted(current_formats | baseline_formats)
                        if current["format_counts"].get(key, 0)
                        != baseline["format_counts"].get(key, 0)
                    },
                },
                "opcode_delta": {
                    "added": sorted(current_opcodes - baseline_opcodes),
                    "removed": sorted(baseline_opcodes - current_opcodes),
                    "occurrence_delta": {
                        key: current["opcode_counts"].get(key, 0)
                        - baseline["opcode_counts"].get(key, 0)
                        for key in sorted(current_opcodes | baseline_opcodes)
                        if current["opcode_counts"].get(key, 0)
                        != baseline["opcode_counts"].get(key, 0)
                    },
                },
                "native_delta": {
                    "added": sorted(
                        int(item) for item in current_natives - baseline_natives
                    ),
                    "removed": sorted(
                        int(item) for item in baseline_natives - current_natives
                    ),
                    "call_site_delta": {
                        key: current["native_counts"].get(key, 0)
                        - baseline["native_counts"].get(key, 0)
                        for key in sorted(
                            current_natives | baseline_natives,
                            key=lambda item: int(item),
                        )
                        if current["native_counts"].get(key, 0)
                        != baseline["native_counts"].get(key, 0)
                    },
                },
                "diagnostics": {
                    "count": current["diagnostic_count"],
                    "categories": current["diagnostic_categories"],
                },
                "static_function_delta": {
                    "candidate_count": int(primary_summary.get("candidate_count", 0))
                    - int(baseline_summary.get("candidate_count", 0)),
                    "entrypoint_reachable_count": int(
                        primary_summary.get("entrypoint_reachable_count", 0)
                    )
                    - int(baseline_summary.get("entrypoint_reachable_count", 0)),
                    "direct_call_edge_count": int(
                        primary_summary.get("direct_call_edge_count", 0)
                    )
                    - int(baseline_summary.get("direct_call_edge_count", 0)),
                    "resolved_call_edge_count": int(
                        primary_summary.get("resolved_call_edge_count", 0)
                    )
                    - int(baseline_summary.get("resolved_call_edge_count", 0)),
                    "evidence_status": "static-candidate-only",
                    "semantic_status": "unverified",
                },
            }
        )

    sha_groups: dict[str, list[str]] = defaultdict(list)
    for sample in samples:
        sha = game_rows[sample["sample_id"]]["runtime_fingerprint"].get("sha256")
        if sha:
            sha_groups[sha].append(sample["sample_id"])
    binary_identity_clusters = [
        {
            "sha256": sha,
            "sample_ids": sorted(sample_ids),
            "identity_status": "exact-file-hash-match",
        }
        for sha, sample_ids in sorted(sha_groups.items())
        if len(sample_ids) > 1
    ]
    baseline_binaries = ledger_by_sample.get(baseline_id, [])
    runtime_binary_comparisons = []
    for sample in samples:
        sample_id = sample["sample_id"]
        if sample_id == baseline_id:
            continue
        current_binaries = ledger_by_sample.get(sample_id, [])
        for current_binary in current_binaries:
            role = current_binary.get("runtime_role")
            if role not in {
                "primary-runtime-executable",
                "runtime-variant-executable",
                "engine-configuration-plugin",
                "runtime-support-library",
                "runtime-dependent-library",
            }:
                continue
            baseline_candidates = [
                item
                for item in baseline_binaries
                if item.get("runtime_role") == role
            ]
            if not baseline_candidates:
                runtime_binary_comparisons.append(
                    {
                        "sample_id": sample_id,
                        "binary_path": current_binary["binary_path"],
                        "runtime_role": role,
                        "baseline_binary_path": None,
                        "status": "no-same-role-v401-reference",
                        "evidence_status": "static-evidence-only",
                    }
                )
                continue
            baseline_binary = sorted(
                baseline_candidates, key=lambda item: item["binary_path"]
            )[0]
            current_functions = {
                int(item["rva"]): item
                for item in current_binary.get("function_candidates", [])
            }
            baseline_functions = {
                int(item["rva"]): item
                for item in baseline_binary.get("function_candidates", [])
            }
            current_edges = {
                (
                    int(item["caller_rva"]),
                    int(item["call_site_rva"]),
                    int(item["target_rva"]),
                    bool(item.get("resolved_candidate")),
                )
                for item in current_binary.get("call_edges", [])
            }
            baseline_edges = {
                (
                    int(item["caller_rva"]),
                    int(item["call_site_rva"]),
                    int(item["target_rva"]),
                    bool(item.get("resolved_candidate")),
                )
                for item in baseline_binary.get("call_edges", [])
            }
            current_imports = {
                str(item.get("name", item))
                if isinstance(item, dict)
                else str(item)
                for item in current_binary.get("imports", [])
            }
            baseline_imports = {
                str(item.get("name", item))
                if isinstance(item, dict)
                else str(item)
                for item in baseline_binary.get("imports", [])
            }
            current_identity = binary_fingerprints.get(current_binary["binary_path"], {})
            baseline_identity = binary_fingerprints.get(
                baseline_binary["binary_path"], {}
            )
            common_rvas = set(current_functions) & set(baseline_functions)
            current_summary = current_binary.get("candidate_summary", {})
            baseline_summary = baseline_binary.get("candidate_summary", {})
            runtime_binary_comparisons.append(
                {
                    "sample_id": sample_id,
                    "binary_path": current_binary["binary_path"],
                    "sha256": current_binary.get("sha256"),
                    "runtime_role": role,
                    "baseline_sample_id": baseline_id,
                    "baseline_binary_path": baseline_binary["binary_path"],
                    "baseline_sha256": baseline_binary.get("sha256"),
                    "status": "compared",
                    "code_identity": {
                        "sha256": current_identity.get("code_sha256"),
                        "baseline_sha256": baseline_identity.get("code_sha256"),
                        "exact_code_match": (
                            current_identity.get("code_sha256") is not None
                            and current_identity.get("code_sha256")
                            == baseline_identity.get("code_sha256")
                        ),
                        "fingerprint_status": current_identity.get(
                            "status", "unavailable"
                        ),
                    },
                    "candidate_function_delta": {
                        "current_count": len(current_functions),
                        "baseline_count": len(baseline_functions),
                        "added_rvas": sorted(set(current_functions) - set(baseline_functions)),
                        "removed_rvas": sorted(set(baseline_functions) - set(current_functions)),
                        "shared_rvas_with_size_changes": [
                            {
                                "rva": rva,
                                "current_size": int(current_functions[rva].get("size", 0)),
                                "baseline_size": int(baseline_functions[rva].get("size", 0)),
                            }
                            for rva in sorted(common_rvas)
                            if int(current_functions[rva].get("size", 0))
                            != int(baseline_functions[rva].get("size", 0))
                        ],
                        "entrypoint_reachable_delta": int(
                            current_summary.get("entrypoint_reachable_count", 0)
                        )
                        - int(baseline_summary.get("entrypoint_reachable_count", 0)),
                        "evidence_status": "static-candidate-only",
                        "semantic_status": "unverified",
                    },
                    "direct_call_graph_delta": {
                        "current_edge_count": len(current_edges),
                        "baseline_edge_count": len(baseline_edges),
                        "added_edges": [
                            {
                                "caller_rva": edge[0],
                                "call_site_rva": edge[1],
                                "target_rva": edge[2],
                                "resolved_candidate": edge[3],
                            }
                            for edge in sorted(current_edges - baseline_edges)
                        ],
                        "removed_edges": [
                            {
                                "caller_rva": edge[0],
                                "call_site_rva": edge[1],
                                "target_rva": edge[2],
                                "resolved_candidate": edge[3],
                            }
                            for edge in sorted(baseline_edges - current_edges)
                        ],
                        "evidence_status": "static-direct-call-evidence",
                    },
                    "imports_delta": {
                        "added": sorted(current_imports - baseline_imports),
                        "removed": sorted(baseline_imports - current_imports),
                    },
                    "ghidra_evidence": {
                        "status": current_binary.get("ghidra_status"),
                        "analysis_mode": current_binary.get("ghidra_analysis_mode"),
                        "function_count": int(
                            current_binary.get("ghidra_function_count", 0)
                        ),
                        "semantic_status": "unverified",
                    },
                }
            )

    code_groups: dict[str, list[dict[str, str]]] = defaultdict(list)
    for binary_path, identity in binary_fingerprints.items():
        code_sha = identity.get("code_sha256")
        if code_sha:
            binary = next(
                (
                    item
                    for values in ledger_by_sample.values()
                    for item in values
                    if item["binary_path"] == binary_path
                ),
                None,
            )
            if binary:
                code_groups[code_sha].append(
                    {
                        "sample_id": str(binary["sample_id"]),
                        "binary_path": binary_path,
                    }
                )
    code_identity_clusters = [
        {
            "code_sha256": code_sha,
            "binaries": sorted(items, key=lambda item: (item["sample_id"], item["binary_path"])),
            "identity_status": "exact-executable-section-bytes",
        }
        for code_sha, items in sorted(code_groups.items())
        if len(items) > 1
    ]

    return {
        "schema_version": "cs2-version-diff/v1",
        "status": "static-evidence-only",
        "baseline_sample_id": baseline_id,
        "comparison_count": len(diffs),
        "comparisons": diffs,
        "binary_identity_clusters": binary_identity_clusters,
        "runtime_binary_comparison_count": len(runtime_binary_comparisons),
        "runtime_binary_comparisons": runtime_binary_comparisons,
        "code_identity_clusters": code_identity_clusters,
        "interpretation": {
            "runtime_sha256": "Exact file identity only; it does not prove equivalent behavior.",
            "opcode_delta": "Static script occurrence difference; it does not prove execution coverage.",
            "native_delta": "Static immediate native index difference; ABI and side effects remain unverified.",
            "function_delta": "Capstone candidate and direct-call graph difference; function boundaries and semantics remain unverified.",
        },
    }


def build_report(
    content: dict[str, Any],
    inventory: dict[str, Any],
    ledger: dict[str, Any],
    overlays: list[dict[str, Any]] | None = None,
) -> tuple[dict[str, Any], dict[str, Any]]:
    inventory_by_id = {
        stable_sample_id(sample): sample for sample in inventory.get("samples", [])
    }
    ledger_by_sample: dict[str, list[dict[str, Any]]] = defaultdict(list)
    binary_by_path: dict[str, dict[str, Any]] = {}
    for binary in ledger.get("binaries", []):
        ledger_by_sample[str(binary.get("sample_id"))].append(binary)
        binary_by_path[binary["binary_path"]] = binary
    overlay_by_sha: dict[str, dict[str, Any]] = {}
    overlay_sources = []
    for overlay in overlays or []:
        overlay_sources.append(overlay.get("_source_path"))
        for binary in overlay.get("binaries", []):
            sha = str(binary.get("sha256", "")).lower()
            if not sha:
                continue
            previous = overlay_by_sha.get(sha)
            if previous is None or (
                binary.get("ghidra_status") == "ok"
                and previous.get("ghidra_status") != "ok"
            ):
                overlay_by_sha[sha] = binary
    for binary in binary_by_path.values():
        overlay = overlay_by_sha.get(str(binary.get("sha256", "")).lower())
        if overlay is None:
            continue
        binary["ghidra_overlay"] = {
            "source": next(
                (
                    overlay.get("_source_path")
                    for item in overlays or []
                    if any(
                        candidate is overlay
                        for candidate in item.get("binaries", [])
                    )
                ),
                None,
            ),
            "status": overlay.get("ghidra_status"),
            "analysis_mode": overlay.get("ghidra_analysis_mode"),
            "function_count": int(overlay.get("ghidra_function_count", 0)),
            "named_function_count": int(
                overlay.get("ghidra_named_function_count", 0)
            ),
            "decompile_count": int(overlay.get("ghidra_decompile_count", 0)),
        }
        if overlay.get("ghidra_status") == "ok":
            binary["ghidra_status"] = overlay.get("ghidra_status")
            binary["ghidra_analysis_mode"] = overlay.get("ghidra_analysis_mode")
            binary["ghidra_function_count"] = int(
                overlay.get("ghidra_function_count", 0)
            )
            binary["ghidra_named_function_count"] = int(
                overlay.get("ghidra_named_function_count", 0)
            )
            binary["ghidra_decompile_count"] = int(
                overlay.get("ghidra_decompile_count", 0)
            )
            binary["ghidra_functions"] = overlay.get("ghidra_functions", [])
    binary_fingerprints: dict[str, dict[str, Any]] = {}
    for binary_path, binary in binary_by_path.items():
        path = WORKSPACE / Path(binary_path.replace("/", "\\"))
        identity: dict[str, Any] = {
            "status": "static-ledger-only",
            "file_sha256": binary.get("sha256"),
            "code_sha256": None,
            "executable_sections": [],
        }
        try:
            raw = path.read_bytes()
            actual_sha = hashlib.sha256(raw).hexdigest()
            if actual_sha != binary.get("sha256"):
                identity["status"] = "source-hash-mismatch"
            else:
                section_hashes = []
                code_digest = hashlib.sha256()
                for section in binary.get("executable_sections", []):
                    offset = int(section["raw_offset"])
                    length = int(section["raw_size"])
                    data = raw[offset : offset + length]
                    if len(data) != length:
                        raise ValueError(
                            f"executable section truncated: {binary_path}:{section.get('name')}"
                        )
                    section_hash = hashlib.sha256(data).hexdigest()
                    section_hashes.append(
                        {
                            "name": section.get("name"),
                            "rva": int(section.get("rva", 0)),
                            "raw_size": length,
                            "sha256": section_hash,
                        }
                    )
                    code_digest.update(
                        str(section.get("name", "")).encode("ascii", errors="replace")
                    )
                    code_digest.update(b"\0")
                    code_digest.update(length.to_bytes(8, "little"))
                    code_digest.update(data)
                identity.update(
                    {
                        "status": "sha256-verified",
                        "code_sha256": code_digest.hexdigest()
                        if section_hashes
                        else None,
                        "executable_sections": section_hashes,
                    }
                )
        except OSError as error:
            identity["status"] = f"binary-unavailable: {error.__class__.__name__}"
        binary_fingerprints[binary_path] = identity

    games = []
    for content_game in content.get("games", []):
        root = str(content_game.get("root", ""))
        sample = next(
            (
                item
                for item in inventory.get("samples", [])
                if resolved_inventory_root(str(item.get("root", "")), WORKSPACE)
                == resolved_inventory_root(root, WORKSPACE)
            ),
            None,
        )
        if sample is None:
            sample = {
                "sample_id": hashlib.sha256(root.encode("utf-8")).hexdigest()[:16],
                "name": root,
                "root": root,
                "binaries": [],
            }
        sample_id = stable_sample_id(sample)
        binaries = ledger_by_sample.get(sample_id, [])
        primary = primary_binary(sample, ledger_by_sample)
        inventory_primary = inventory_binary(sample, (primary or {}).get("binary_path"))
        script = content_game.get("scripts", {})
        resources = normalize_counts(content_game.get("resources"))
        archive_entries = normalize_counts(
            content_game.get("archives", {}).get("entries_by_extension")
        )
        diagnostic_list = content_game.get("diagnostics", [])
        game = {
            "sample_id": sample_id,
            "name": sample.get("name") or content_game.get("variant") or sample_id,
            "root": relative_root(root, WORKSPACE),
            "variant": content_game.get("variant"),
            "content_fingerprint": content_game.get("fingerprint"),
            "runtime_fingerprint": runtime_fingerprint(
                sample,
                primary,
                inventory_primary,
                binary_fingerprints.get((primary or {}).get("binary_path", "")),
            ),
            "archive_summary": {
                key: content_game.get("archives", {}).get(key)
                for key in (
                    "total",
                    "indexed",
                    "encrypted_without_seed",
                    "non_kif",
                    "malformed",
                    "indexed_entries",
                )
                if key in content_game.get("archives", {})
            },
            "format_counts": {
                key: {
                    "loose_files": resources.get(key, 0),
                    "archive_entries": archive_entries.get(key, 0),
                    "observed_total": resources.get(key, 0) + archive_entries.get(key, 0),
                }
                for key in sorted(set(resources) | set(archive_entries))
            },
            "script_summary": {
                "kcs_files": int(script.get("kcs_files", 0)),
                "cst_files": int(script.get("cst_files", 0)),
                "fes_files": int(script.get("fes_files", 0)),
                "anm_files": int(script.get("anm_files", 0)),
                "parse_failures": int(script.get("parse_failures", 0)),
                "evidence_status": "static-content-audit",
            },
            "opcode_counts": normalize_counts(script.get("opcodes")),
            "native_counts": {
                str(key): int(value.get("call_sites", 0))
                for key, value in (script.get("native_calls") or {}).items()
            },
            "native_details": script.get("native_calls") or {},
            "diagnostic_count": len(diagnostic_list),
            "diagnostic_categories": diagnostic_categories(diagnostic_list),
            "function_ledger": {
                "binary_count": len(binaries),
                "binaries": [],
                "semantic_implementation_count": 0,
                "semantic_implementation_claim": False,
                "evidence_status": "static-candidate-only",
            },
        }
        for binary in sorted(binaries, key=lambda item: item["binary_path"]):
            summary = binary.get("candidate_summary", {})
            game["function_ledger"]["binaries"].append(
                {
                    "binary_path": binary["binary_path"],
                    "runtime_role": binary.get("runtime_role"),
                    "sha256": binary.get("sha256"),
                    "candidate_count": int(summary.get("candidate_count", 0)),
                    "entrypoint_reachable_count": int(
                        summary.get("entrypoint_reachable_count", 0)
                    ),
                    "direct_call_edge_count": int(
                        summary.get("direct_call_edge_count", 0)
                    ),
                    "resolved_call_edge_count": int(
                        summary.get("resolved_call_edge_count", 0)
                    ),
                    "ghidra_status": binary.get("ghidra_status"),
                    "ghidra_function_count": int(
                        binary.get("ghidra_function_count", 0)
                    ),
                    "candidate_entry_matches": int(
                        binary.get("candidate_entry_matches", 0)
                    ),
                    "candidate_entry_match_percent": float(
                        binary.get("candidate_entry_match_percent", 0.0)
                    ),
                    "code_identity": binary_fingerprints.get(
                        binary["binary_path"], {}
                    ),
                    "ghidra_overlay": binary.get("ghidra_overlay"),
                    "evidence_status": "static-candidate-only",
                    "semantic_status": "unverified",
                }
            )
        games.append(game)

    games.sort(key=lambda item: item["sample_id"])
    format_game_rows, format_totals = format_rows(games)
    opcode_by_game, opcode_rows = summarize_opcodes(games)
    native_by_game, native_rows = summarize_natives(games)
    ledger_game_rows, ledger_totals = ledger_rows(
        games, ledger_by_sample
    )
    game_lookup = {item["sample_id"]: item for item in games}
    primary_by_sample = {
        item["sample_id"]: primary_binary(
            inventory_by_id.get(item["sample_id"], {}),
            ledger_by_sample,
        )
        for item in games
    }
    version_games = {}
    for item in games:
        version_games[item["sample_id"]] = {
            **item,
            "format_counts": {
                key: value["observed_total"]
                for key, value in item["format_counts"].items()
            },
            "opcode_counts": item["opcode_counts"],
            "native_counts": item["native_counts"],
        }
    baseline = "cs2_full_v401" if "cs2_full_v401" in version_games else games[0]["sample_id"]
    version_report = build_version_diffs(
        games,
        primary_by_sample,
        version_games,
        ledger_by_sample,
        baseline,
        binary_fingerprints,
    )

    primary_sha256s = sorted(
        {
            item["runtime_fingerprint"]["sha256"]
            for item in games
            if item["runtime_fingerprint"].get("sha256")
        }
    )
    report = {
        "schema_version": "cs2-content-audit/v1",
        "status": "static-evidence-only",
        "generated_at_utc": datetime.now(timezone.utc).replace(microsecond=0).isoformat(),
        "sources": {
            "content_audit": content.get("_source_path", workspace_relative(DEFAULT_CONTENT)),
            "sample_inventory": inventory.get("_source_path", workspace_relative(DEFAULT_INVENTORY)),
            "runtime_function_ledger": ledger.get("_source_path", workspace_relative(DEFAULT_LEDGER)),
            "ghidra_overlays": overlay_sources,
            "ghidra_status": ledger.get("method", {}).get("ghidra_analysis_mode"),
        },
        "scope": {
            "sample_count": len(games),
            "game_count": sum(item["sample_id"] != "cs2_full_v401" for item in games),
            "primary_runtime_sha256_count": len(primary_sha256s),
            "archive_count": sum(
                int(item["archive_summary"].get("total", 0)) for item in games
            ),
            "indexed_archive_count": sum(
                int(item["archive_summary"].get("indexed", 0)) for item in games
            ),
            "semantic_implementation_count": 0,
            "semantic_implementation_claim": False,
        },
        "evidence_policy": {
            "static_evidence_only": True,
            "game_executables_started": False,
            "simulator_source_read": False,
            "candidate_functions_status": "static-candidate-only",
            "call_edges_status": "static-direct-call-evidence",
            "ghidra_status": "function-discovery-only",
            "opcode_status": "sample-observed-static-script",
            "native_status": "sample-observed-static-script",
            "unverified_behavior_must_not_be_called_implemented": True,
        },
        "games": games,
        "format_summary": {
            "by_format": format_totals,
            "by_game": format_game_rows,
        },
        "opcode_summary": {
            "by_opcode": opcode_rows,
            "by_game": opcode_by_game,
        },
        "native_summary": {
            "by_native_index": native_rows,
            "by_game": native_by_game,
        },
        "function_and_call_graph_summary": {
            "by_game": ledger_game_rows,
            "totals": ledger_totals,
            "semantic_implementation_count": 0,
            "semantic_implementation_claim": False,
        },
        "version_differences": {
            "report": "reports/version_diff_report.json",
            "baseline_sample_id": baseline,
            "comparison_status": "static-content-and-pe-metadata",
        },
    }
    return report, version_report


def render_markdown(report: dict[str, Any], version_report: dict[str, Any]) -> str:
    lines = [
        "# CatSystem2 Static Evidence Report",
        "",
        "This is a read-only static evidence snapshot. It does not start game "
        "executables. Candidate functions, call edges, Ghidra functions, opcode "
        "sightings, and native sightings are not semantic implementation claims.",
        "",
        f"- Schema: `{report['schema_version']}`",
        f"- Generated: `{report['generated_at_utc']}`",
        f"- Samples: `{report['scope']['sample_count']}` "
        f"({report['scope']['game_count']} game roots plus the V401 reference)",
        f"- Primary runtime hashes: `{report['scope']['primary_runtime_sha256_count']}`",
        f"- Candidate functions: `{report['function_and_call_graph_summary']['totals'].get('candidate_count', 0)}`",
        f"- Direct call edges: `{report['function_and_call_graph_summary']['totals'].get('direct_call_edge_count', 0)}`",
        f"- Ghidra successes: `{report['function_and_call_graph_summary']['totals'].get('ghidra_ok_binary_count', 0)}`",
        f"- Semantic implementations claimed: `0`",
        "",
        "## Game Summary",
        "",
        "| Sample | Variant | Formats | KCS | CST | FES | ANM | Opcodes | Natives | Diagnostics | Primary runtime |",
        "| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |",
    ]
    for game in report["games"]:
        script = game["script_summary"]
        primary = game["runtime_fingerprint"]
        lines.append(
            f"| `{game['sample_id']}` | `{game.get('variant')}` | "
            f"{len(game['format_counts'])} | {script['kcs_files']} | "
            f"{script['cst_files']} | {script['fes_files']} | {script['anm_files']} | "
            f"{len(game['opcode_counts'])} | {len(game['native_counts'])} | "
            f"{game['diagnostic_count']} | `{primary.get('sha256', 'missing')}` |"
        )
    lines.extend(
        [
            "",
            "## Format Totals",
            "",
            "| Extension | Games | Loose files | Archive entries | Parse failures |",
            "| --- | ---: | ---: | ---: | ---: |",
        ]
    )
    for extension, row in report["format_summary"]["by_format"].items():
        lines.append(
            f"| `{extension}` | {row['games_observed']} | {row['loose_files']} | "
            f"{row['archive_entries']} | {row['parse_failures']} |"
        )
    lines.extend(
        [
            "",
            "## Opcode Summary",
            "",
            "| Opcode | Samples | Occurrences | Evidence |",
            "| --- | ---: | ---: | --- |",
        ]
    )
    for row in report["opcode_summary"]["by_opcode"]:
        lines.append(
            f"| `{row['opcode']}` | {row['sample_count']} | "
            f"{row['occurrence_count']} | `{row['evidence_status']}` |"
        )
    lines.extend(
        [
            "",
            "## Native Summary",
            "",
            "| Native | Samples | Call sites | Argument byte widths | Semantic status |",
            "| ---: | ---: | ---: | --- | --- |",
        ]
    )
    for row in report["native_summary"]["by_native_index"]:
        widths = ", ".join(
            f"{key}:{value}" for key, value in row["argument_byte_counts"].items()
        )
        lines.append(
            f"| `{row['native_index']}` | {row['sample_count']} | "
            f"{row['call_site_count']} | `{widths}` | `{row['semantic_status']}` |"
        )
    lines.extend(
        [
            "",
            "## Function And Call Graph Evidence",
            "",
            "| Sample | Binary | Candidates | Reachable | Direct edges | Resolved edges | Ghidra |",
            "| --- | --- | ---: | ---: | ---: | ---: | --- |",
        ]
    )
    for game in report["function_and_call_graph_summary"]["by_game"]:
        for binary in game["binaries"]:
            lines.append(
                f"| `{game['sample_id']}` | `{binary['binary_path']}` | "
                f"{binary['candidate_count']} | {binary['entrypoint_reachable_count']} | "
                f"{binary['direct_call_edge_count']} | "
                f"{binary['resolved_call_edge_count']} | "
                f"`{binary['ghidra_status']}` |"
            )
    lines.extend(
        [
            "",
            "## Version Differences",
            "",
            f"Baseline: `{version_report['baseline_sample_id']}`. "
            "Differences are static content, PE metadata, and candidate-graph "
            "differences; they do not establish runtime behavior.",
            "",
            "| Sample | Added opcodes | Removed opcodes | Added natives | Removed natives | Diagnostics |",
            "| --- | ---: | ---: | ---: | ---: | ---: |",
        ]
    )
    for comparison in version_report["comparisons"]:
        lines.append(
            f"| `{comparison['sample_id']}` | "
            f"{len(comparison['opcode_delta']['added'])} | "
            f"{len(comparison['opcode_delta']['removed'])} | "
            f"{len(comparison['native_delta']['added'])} | "
            f"{len(comparison['native_delta']['removed'])} | "
            f"{comparison['diagnostics']['count']} |"
        )
    lines.extend(
        [
            "",
            "The JSON report retains per-game format counts, opcode/native "
            "occurrences, PE fingerprints, Ghidra status, candidate function "
            "counts, direct call graph counts, and all evidence classifications.",
            "",
        ]
    )
    return "\n".join(lines)


def render_version_markdown(version_report: dict[str, Any]) -> str:
    lines = [
        "# CatSystem2 Version Difference Report",
        "",
        f"Baseline: `{version_report['baseline_sample_id']}`.",
        "",
        "All comparisons are static evidence. Exact hashes identify identical "
        "files; they do not prove identical runtime behavior.",
        "",
        "| Sample | Variant | Runtime hash same | Formats added/removed | Opcodes added/removed | Natives added/removed | Diagnostics |",
        "| --- | --- | --- | --- | --- | --- | ---: |",
    ]
    for row in version_report["comparisons"]:
        lines.append(
            f"| `{row['sample_id']}` | `{row['variant']}` | "
            f"{row['runtime_identity']['same_sha256_as_baseline']} | "
            f"{len(row['format_delta']['added'])}/"
            f"{len(row['format_delta']['removed'])} | "
            f"{len(row['opcode_delta']['added'])}/"
            f"{len(row['opcode_delta']['removed'])} | "
            f"{len(row['native_delta']['added'])}/"
            f"{len(row['native_delta']['removed'])} | "
            f"{row['diagnostics']['count']} |"
        )
    lines.extend(
        [
            "",
            "The JSON contains occurrence deltas and candidate function/call-edge "
            "deltas for each comparison.",
            "",
        ]
    )
    return "\n".join(lines)


def main() -> int:
    args = parse_args()
    content_path = args.content.resolve()
    inventory_path = args.inventory.resolve()
    ledger_path = args.ledger.resolve()
    content = read_json(content_path)
    inventory = read_json(inventory_path)
    ledger = read_json(ledger_path)
    content["_source_path"] = workspace_relative(content_path)
    inventory["_source_path"] = workspace_relative(inventory_path)
    ledger["_source_path"] = workspace_relative(ledger_path)
    overlays = []
    for overlay_path in args.ghidra_overlay:
        overlay = read_json(overlay_path.resolve())
        overlay["_source_path"] = workspace_relative(overlay_path.resolve())
        overlays.append(overlay)
    report, version_report = build_report(content, inventory, ledger, overlays)
    write_json(args.output.resolve(), report)
    write_text(args.markdown.resolve(), render_markdown(report, version_report))
    write_json(args.version_output.resolve(), version_report)
    write_text(
        args.version_markdown.resolve(), render_version_markdown(version_report)
    )
    status = {
        "schema_version": "cs2-content-audit/v1",
        "status": "generated",
        "generated_at_utc": report["generated_at_utc"],
        "outputs": {
            "static_evidence_json": workspace_relative(args.output.resolve()),
            "static_evidence_markdown": workspace_relative(args.markdown.resolve()),
            "version_diff_json": workspace_relative(args.version_output.resolve()),
            "version_diff_markdown": workspace_relative(
                args.version_markdown.resolve()
            ),
        },
        "scope": report["scope"],
        "evidence_policy": report["evidence_policy"],
        "source_hashes": {
            "content_audit": hashlib.sha256(content_path.read_bytes()).hexdigest(),
            "sample_inventory": hashlib.sha256(inventory_path.read_bytes()).hexdigest(),
            "runtime_function_ledger": hashlib.sha256(ledger_path.read_bytes()).hexdigest(),
        },
        "archive_policy": {
            "temporary_outputs": "archive/YYYY-MM/",
            "game_directories_modified": False,
            "original_executables_started": False,
            "simulator_source_modified": False,
        },
    }
    archive_path = args.archive_status.resolve()
    if ARCHIVE_ROOT not in archive_path.parents:
        raise ValueError("archive status must be inside archive/2026-11/")
    write_json(archive_path, status)
    print(f"Wrote {args.output.resolve()}")
    print(f"Wrote {args.version_output.resolve()}")
    print(f"Wrote {archive_path}")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, ValueError, KeyError, json.JSONDecodeError) as error:
        print(f"build_static_evidence_report.py: {error}", file=sys.stderr)
        raise SystemExit(2)

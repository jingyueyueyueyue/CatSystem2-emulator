"""Inventory CatSystem2 game samples and seed the runtime function ledger.

The scanner is intentionally read-only with respect to game samples. It writes
only the two requested JSON reports under ``reports/`` unless another output
directory is supplied.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import sys
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Iterable

import capstone
import pefile
import unicorn
from unicorn import Uc, UC_ARCH_X86, UC_MODE_32, UC_MODE_64


SCRIPT_PATH = Path(__file__).resolve()
DEFAULT_WORKSPACE = SCRIPT_PATH.parents[2]
RESOURCE_EXTENSIONS = (".int", ".cst", ".fes", ".kcs", ".hg3", ".anm", ".wss")
PE_EXTENSIONS = (".exe", ".dll")
MACHINE_NAMES = {
    0x014C: ("IMAGE_FILE_MACHINE_I386", "x86"),
    0x8664: ("IMAGE_FILE_MACHINE_AMD64", "x64"),
    0x01C0: ("IMAGE_FILE_MACHINE_ARM", "arm"),
    0x01C4: ("IMAGE_FILE_MACHINE_ARMNT", "arm"),
    0xAA64: ("IMAGE_FILE_MACHINE_ARM64", "arm64"),
}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Scan CatSystem2 samples and write inventory/function-ledger JSON reports."
    )
    parser.add_argument(
        "--workspace",
        type=Path,
        default=DEFAULT_WORKSPACE,
        help="CS2 workspace root. Defaults to the repository containing this script.",
    )
    parser.add_argument(
        "--sample-root",
        dest="sample_roots",
        action="append",
        type=Path,
        help="Sample directory to scan. Repeat for multiple roots. Defaults to discovery.",
    )
    parser.add_argument(
        "--output-dir",
        type=Path,
        help="Report directory. Defaults to <workspace>/reports.",
    )
    return parser.parse_args()


def utc_timestamp() -> str:
    return datetime.now(timezone.utc).replace(microsecond=0).isoformat().replace("+00:00", "Z")


def path_for_json(path: Path, workspace: Path) -> str:
    try:
        relative = path.resolve().relative_to(workspace.resolve())
        return relative.as_posix()
    except ValueError:
        return path.resolve().as_posix()


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def iter_files(root: Path) -> Iterable[Path]:
    for directory, directory_names, file_names in os.walk(root, followlinks=False):
        directory_names.sort()
        file_names.sort()
        for file_name in file_names:
            yield Path(directory) / file_name


def has_sample_files(root: Path) -> bool:
    for path in iter_files(root):
        if path.suffix.lower() in PE_EXTENSIONS + RESOURCE_EXTENSIONS:
            return True
    return False


def discover_sample_roots(workspace: Path) -> list[Path]:
    catalog = workspace / "games"
    if not catalog.is_dir():
        return []
    roots = [sample for sample in catalog.iterdir() if sample.is_dir() and has_sample_files(sample)]

    roots.sort(
        key=lambda item: (
            0 if item.name.casefold() == "cs2_full_v401" else 1,
            item.name.casefold(),
        )
    )
    return roots


def normalize_sample_roots(workspace: Path, requested: list[Path] | None) -> list[Path]:
    roots = requested if requested else discover_sample_roots(workspace)
    normalized: list[Path] = []
    seen: set[str] = set()
    for root in roots:
        resolved = root if root.is_absolute() else workspace / root
        resolved = resolved.resolve()
        key = os.path.normcase(str(resolved))
        if not resolved.is_dir():
            raise FileNotFoundError(f"Sample directory does not exist: {resolved}")
        if key not in seen:
            normalized.append(resolved)
            seen.add(key)
    return normalized


def classify_binary(path: Path, sample_root: Path) -> str:
    relative_parts = [part.casefold() for part in path.relative_to(sample_root).parts]
    patch_markers = ("patch", "backup", "原版备份", "补丁", "修正")
    if any(marker in part for part in relative_parts for marker in patch_markers):
        return "patch_or_backup"
    if any(part in {"tool", "tools", "system"} for part in relative_parts[:-1]):
        return "tool_or_system"
    return "sample_root"


def machine_info(machine: int) -> tuple[str, str]:
    return MACHINE_NAMES.get(machine, (f"0x{machine:04x}", "unknown"))


def section_name(section: Any) -> str:
    return section.Name.rstrip(b"\x00").decode("ascii", errors="replace")


def section_record(section: Any) -> dict[str, Any]:
    return {
        "name": section_name(section),
        "virtual_address_rva": int(section.VirtualAddress),
        "virtual_size": int(section.Misc_VirtualSize),
        "raw_pointer": int(section.PointerToRawData),
        "raw_size": int(section.SizeOfRawData),
        "characteristics": f"0x{int(section.Characteristics):08x}",
    }


def import_dlls(pe: pefile.PE) -> list[str]:
    names: set[str] = set()
    for entry in getattr(pe, "DIRECTORY_ENTRY_IMPORT", []):
        if entry.dll:
            names.add(entry.dll.decode("ascii", errors="replace"))
    return sorted(names, key=str.casefold)


def unicorn_probe(machine: int) -> dict[str, Any]:
    _, architecture = machine_info(machine)
    if architecture == "x86":
        mode = UC_MODE_32
    elif architecture == "x64":
        mode = UC_MODE_64
    else:
        return {
            "available": False,
            "architecture": architecture,
            "reason": "Unicorn probe supports x86 and x64 PE samples only.",
        }

    try:
        Uc(UC_ARCH_X86, mode)
    except Exception as exc:  # pragma: no cover - depends on local Unicorn build
        return {
            "available": False,
            "architecture": architecture,
            "reason": f"{type(exc).__name__}: {exc}",
        }
    return {
        "available": True,
        "architecture": architecture,
        "mode": "32-bit" if mode == UC_MODE_32 else "64-bit",
    }


def entrypoint_disassembly(
    pe: pefile.PE,
    data: bytes,
    machine: int,
    image_base: int,
    entrypoint_rva: int,
) -> list[dict[str, Any]]:
    _, architecture = machine_info(machine)
    if architecture not in {"x86", "x64"}:
        return []

    try:
        raw_offset = pe.get_offset_from_rva(entrypoint_rva)
    except (pefile.PEFormatError, ValueError):
        return []
    if raw_offset < 0 or raw_offset >= len(data):
        return []

    mode = capstone.CS_MODE_32 if architecture == "x86" else capstone.CS_MODE_64
    decoder = capstone.Cs(capstone.CS_ARCH_X86, mode)
    decoder.detail = False
    instructions: list[dict[str, Any]] = []
    code = data[raw_offset : raw_offset + 32]
    for instruction in decoder.disasm(code, image_base + entrypoint_rva, count=8):
        instructions.append(
            {
                "address": int(instruction.address),
                "offset_from_entrypoint": int(instruction.address - (image_base + entrypoint_rva)),
                "bytes": instruction.bytes.hex(),
                "mnemonic": instruction.mnemonic,
                "operands": instruction.op_str,
                "size": int(instruction.size),
            }
        )
    return instructions


def parse_pe(path: Path, workspace: Path, sample_root: Path) -> dict[str, Any]:
    binary: dict[str, Any] = {
        "path": path_for_json(path, workspace),
        "name": path.name,
        "extension": path.suffix.lower(),
        "size": path.stat().st_size,
        "sha256": sha256_file(path),
        "scope": classify_binary(path, sample_root),
        "pe": {
            "parse_status": "unparsed",
            "machine": None,
            "architecture": None,
            "entrypoint_rva": None,
            "entrypoint_virtual_address": None,
            "image_base": None,
            "sections": [],
            "import_dlls": [],
            "entrypoint_instructions": [],
            "unicorn_probe": None,
        },
    }

    try:
        data = path.read_bytes()
        parsed = pefile.PE(data=data, fast_load=False)
        parsed.parse_data_directories(
            directories=[pefile.DIRECTORY_ENTRY["IMAGE_DIRECTORY_ENTRY_IMPORT"]]
        )
        machine_value = int(parsed.FILE_HEADER.Machine)
        machine_name, architecture = machine_info(machine_value)
        entrypoint_rva = int(parsed.OPTIONAL_HEADER.AddressOfEntryPoint)
        image_base = int(parsed.OPTIONAL_HEADER.ImageBase)
        binary["pe"] = {
            "parse_status": "ok",
            "machine": machine_name,
            "machine_value": machine_value,
            "architecture": architecture,
            "entrypoint_rva": entrypoint_rva,
            "entrypoint_virtual_address": image_base + entrypoint_rva,
            "image_base": image_base,
            "subsystem": int(parsed.OPTIONAL_HEADER.Subsystem),
            "sections": [section_record(section) for section in parsed.sections],
            "import_dlls": import_dlls(parsed),
            "entrypoint_instructions": entrypoint_disassembly(
                parsed,
                data,
                machine_value,
                image_base,
                entrypoint_rva,
            ),
            "unicorn_probe": unicorn_probe(machine_value),
        }
    except (OSError, pefile.PEFormatError, ValueError, IndexError) as exc:
        binary["pe"] = {
            "parse_status": "error",
            "error": f"{type(exc).__name__}: {exc}",
            "machine": None,
            "architecture": None,
            "entrypoint_rva": None,
            "entrypoint_virtual_address": None,
            "image_base": None,
            "sections": [],
            "import_dlls": [],
            "entrypoint_instructions": [],
            "unicorn_probe": None,
        }
    return binary


def resource_counts(files: Iterable[Path]) -> dict[str, int]:
    counts = {extension[1:]: 0 for extension in RESOURCE_EXTENSIONS}
    for path in files:
        extension = path.suffix.lower()
        if extension in RESOURCE_EXTENSIONS:
            counts[extension[1:]] += 1
    return counts


def sample_id(sample_root: Path) -> str:
    value = sample_root.name.casefold()
    cleaned = "".join(character if character.isalnum() else "_" for character in value)
    return cleaned.strip("_") or "sample"


def scan_sample(sample_root: Path, workspace: Path) -> dict[str, Any]:
    files = list(iter_files(sample_root))
    binaries = [
        parse_pe(path, workspace, sample_root)
        for path in files
        if path.suffix.lower() in PE_EXTENSIONS
    ]
    parse_errors = [
        {
            "path": binary["path"],
            "error": binary["pe"].get("error", "unknown PE parse error"),
        }
        for binary in binaries
        if binary["pe"]["parse_status"] == "error"
    ]
    return {
        "sample_id": sample_id(sample_root),
        "name": sample_root.name,
        "root": path_for_json(sample_root, workspace),
        "scan_status": "ok" if not parse_errors else "ok_with_diagnostics",
        "file_count": len(files),
        "binary_count": len(binaries),
        "resource_counts": resource_counts(files),
        "binaries": binaries,
        "diagnostics": {
            "pe_parse_errors": parse_errors,
        },
    }


def initial_function_entry(binary: dict[str, Any], sample: dict[str, Any]) -> dict[str, Any]:
    pe_info = binary["pe"]
    entrypoint_rva = pe_info.get("entrypoint_rva")
    return {
        "function_id": "entrypoint",
        "name": None,
        "rva": entrypoint_rva,
        "virtual_address": pe_info.get("entrypoint_virtual_address"),
        "size": None,
        "status": "unreviewed",
        "confidence": "seed",
        "source": "PE optional-header AddressOfEntryPoint",
        "disassembly": pe_info.get("entrypoint_instructions", []),
        "notes": [
            "Initial seed only; this is not a complete function recovery result."
        ],
        "sample_id": sample["sample_id"],
        "binary_path": binary["path"],
    }


def build_function_ledger(
    inventory: dict[str, Any],
    toolchain: dict[str, Any],
) -> dict[str, Any]:
    binaries: list[dict[str, Any]] = []
    for sample in inventory["samples"]:
        for binary in sample["binaries"]:
            pe_info = binary["pe"]
            functions = (
                [initial_function_entry(binary, sample)]
                if pe_info["parse_status"] == "ok"
                else []
            )
            binaries.append(
                {
                    "sample_id": sample["sample_id"],
                    "sample_name": sample["name"],
                    "binary_path": binary["path"],
                    "sha256": binary["sha256"],
                    "scope": binary["scope"],
                    "machine": pe_info.get("machine"),
                    "architecture": pe_info.get("architecture"),
                    "entrypoint_rva": pe_info.get("entrypoint_rva"),
                    "entrypoint_virtual_address": pe_info.get(
                        "entrypoint_virtual_address"
                    ),
                    "analysis_status": (
                        "seeded" if pe_info["parse_status"] == "ok" else "unparsed"
                    ),
                    "functions": functions,
                    "review": {
                        "status": "unreviewed",
                        "analyst": None,
                        "last_reviewed": None,
                        "notes": [],
                    },
                }
            )

    return {
        "schema_version": "1.0.0",
        "status": "initial",
        "generated_at_utc": inventory["generated_at_utc"],
        "source_inventory": "reports/sample_inventory.json",
        "toolchain": toolchain,
        "scope": {
            "description": (
                "Seed ledger for PE entry points. Populate recovered functions "
                "after disassembly and behavior validation."
            ),
            "sample_count": len(inventory["samples"]),
            "binary_count": len(binaries),
            "seed_function_count": sum(len(binary["functions"]) for binary in binaries),
        },
        "function_statuses": [
            "unreviewed",
            "needs_review",
            "confirmed",
            "implemented",
            "unreachable",
        ],
        "binaries": binaries,
        "next_steps": [
            "Recover function boundaries for each executable and DLL.",
            "Record calling convention, parameters, side effects, and version differences.",
            "Validate recovered behavior against the matching original runtime.",
        ],
    }


def toolchain_info() -> dict[str, Any]:
    return {
        "python": platform.python_version(),
        "pefile": getattr(pefile, "__version__", "unknown"),
        "capstone": getattr(capstone, "__version__", "unknown"),
        "unicorn": getattr(unicorn, "__version__", "unknown"),
        "platform": platform.platform(),
        "script": path_for_json(SCRIPT_PATH, DEFAULT_WORKSPACE),
    }


def build_inventory(workspace: Path, sample_roots: list[Path]) -> dict[str, Any]:
    generated_at = utc_timestamp()
    toolchain = toolchain_info()
    samples = [scan_sample(root, workspace) for root in sample_roots]
    binary_count = sum(sample["binary_count"] for sample in samples)
    resource_totals = {
        extension: sum(
            sample["resource_counts"][extension]
            for sample in samples
        )
        for extension in (extension[1:] for extension in RESOURCE_EXTENSIONS)
    }
    return {
        "schema_version": "1.0.0",
        "generated_at_utc": generated_at,
        "workspace": path_for_json(workspace, workspace),
        "toolchain": toolchain,
        "summary": {
            "sample_count": len(samples),
            "binary_count": binary_count,
            "resource_totals": resource_totals,
        },
        "samples": samples,
    }


def write_json(path: Path, payload: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8", newline="\n") as handle:
        json.dump(payload, handle, ensure_ascii=False, indent=2)
        handle.write("\n")


def main() -> int:
    args = parse_args()
    workspace = args.workspace.resolve()
    output_dir = (args.output_dir or workspace / "reports").resolve()
    sample_roots = normalize_sample_roots(workspace, args.sample_roots)
    if not sample_roots:
        raise RuntimeError(
            "No sample directories found. Use --sample-root to select sample directories."
        )

    inventory = build_inventory(workspace, sample_roots)
    ledger = build_function_ledger(inventory, inventory["toolchain"])
    inventory_path = output_dir / "sample_inventory.json"
    ledger_path = output_dir / "runtime_function_ledger.json"
    write_json(inventory_path, inventory)
    write_json(ledger_path, ledger)

    print(
        f"Scanned {inventory['summary']['sample_count']} samples, "
        f"{inventory['summary']['binary_count']} PE files."
    )
    for sample in inventory["samples"]:
        print(
            f"- {sample['name']}: {sample['binary_count']} PE files, "
            f"{sum(sample['resource_counts'].values())} tracked resources, "
            f"{sample['scan_status']}"
        )
    print(f"Wrote {inventory_path}")
    print(f"Wrote {ledger_path}")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (FileNotFoundError, RuntimeError, ValueError) as exc:
        print(f"inventory_samples.py: {exc}", file=sys.stderr)
        raise SystemExit(2)

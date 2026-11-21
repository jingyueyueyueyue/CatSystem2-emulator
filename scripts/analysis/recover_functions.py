"""Recover candidate x86 functions from CatSystem2 PE code sections.

This is a static recovery pass, not a semantic decompiler. It records
functions reached from PE entry points, exports, and direct call targets,
then follows basic blocks until returns or unconditional control flow. The
report keeps confidence and evidence explicit so later runtime validation can
promote candidates to confirmed implementations.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import sys
from collections import deque
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

import capstone
import pefile


WORKSPACE = Path(__file__).resolve().parents[2]
DEFAULT_INVENTORY = WORKSPACE / "reports" / "sample_inventory.json"
DEFAULT_OUTPUT = WORKSPACE / "reports" / "runtime_function_ledger_recovered.json"
MAX_FUNCTION_BYTES = 64 * 1024
MAX_BLOCKS_PER_FUNCTION = 512
MAX_SEED_BYTES = 4 * 1024
MAX_CALL_SEED_BYTES = 512
MAX_SECONDARY_SEEDS = 2048
RUNTIME_EXE_EXCLUSIONS = {
    "ac.exe",
    "bootmenu.exe",
    "boxloader.exe",
    "cs2conf.exe",
    "cs2_crack.exe",
    "dsetup32.exe",
    "dxsetup.exe",
    "edpsf.exe",
    "fes.exe",
    "install.exe",
    "makecglist.exe",
    "makeint.exe",
    "mc.exe",
    "msquestion.exe",
    "pcmtag.exe",
    "radio_cht_201125.exe",
    "setup.exe",
    "start.exe",
    "stview.exe",
    "uninst.exe",
    "uninst_cn.exe",
    "wgc.exe",
    "wsd.exe",
    "ztpack.exe",
    "kamikaze_cht_150324.exe",
}
RUNTIME_DLL_EXCLUSIONS = {
    "dsetup.dll",
    "dsetup32.dll",
    "libpng16.dll",
    "ssefirewall.dll",
    "ssefirewall64.dll",
    "sseoverlay.dll",
}
RUNTIME_PATH_EXCLUSIONS = {
    "backup",
    "patch",
    "patches",
    "tool",
    "tools",
    "directx",
    "master",
    "マスター作成",
    "原版备份",
    "补丁",
    "補丁",
    "补丁＆附件",
}
RUNTIME_EXE_NAME_TOKENS = (
    "update",
    "patch",
    "setup",
    "install",
    "uninstall",
    "uninst",
    "bootmenu",
    "boxloader",
    "msquestion",
    "crack",
    "汉化升级",
    "启动游戏",
)
RUNTIME_TRANSLATION_EXE_TOKENS = (
    "_cht_",
    "cht_",
    "_cn.exe",
    "_chs.exe",
)
KNOWN_RUNTIME_DLLS = {
    "accelerator.dll",
    "edp4.dll",
    "edpsf.dll",
    "emotedriver.dll",
    "hook.dll",
    "iconv.dll",
    "steam_api.dll",
    "vp8decoder.dll",
    "webmsplit.dll",
    "webmvorbisdecoder.dll",
}
KNOWN_ENGINE_DLL_PREFIXES = ("cs2conf",)
ENGINE_RUNTIME_IMPORTS = {
    "imm32.dll",
    "msvfw32.dll",
    "winmm.dll",
}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Recover candidate x86 functions from an existing sample inventory."
    )
    parser.add_argument("--inventory", type=Path, default=DEFAULT_INVENTORY)
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    parser.add_argument(
        "--sample-id",
        action="append",
        dest="sample_ids",
        help="Restrict analysis to one or more sample ids.",
    )
    parser.add_argument(
        "--binary-name",
        action="append",
        dest="binary_names",
        help="Restrict analysis to PE file names, case-insensitive.",
    )
    parser.add_argument(
        "--acceptance-runtime",
        action="store_true",
        help=(
            "Analyze only runtime EXE/DLL files for cs2_full_v401, "
            "Newton and NEKOPARA vol.3."
        ),
    )
    parser.add_argument(
        "--all-runtime",
        action="store_true",
        help=(
            "Analyze runtime executables and host/support DLLs for every sample "
            "in the inventory, excluding tools, installers, patches and backups."
        ),
    )
    parser.add_argument(
        "--print-selection",
        action="store_true",
        help="Print the selected runtime binaries and exit before PE analysis.",
    )
    parser.add_argument(
        "--coverage-json",
        type=Path,
        help="Write a separate static PE/function-candidate coverage report.",
    )
    parser.add_argument(
        "--coverage-markdown",
        type=Path,
        help="Write a human-readable companion to --coverage-json.",
    )
    parser.add_argument(
        "--coverage-only",
        action="store_true",
        help="Build coverage reports without writing the candidate ledger.",
    )
    parser.add_argument(
        "--ghidra-status",
        default=(
            "unavailable: the configured MCP request to "
            "http://127.0.0.1:8080/list_functions was refused; no local "
            "Ghidra/analyzeHeadless command was available"
        ),
        help="Record the Ghidra availability result in the coverage report.",
    )
    return parser.parse_args()


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def rva_to_offset(pe: pefile.PE, rva: int) -> int | None:
    try:
        offset = int(pe.get_offset_from_rva(rva))
    except (pefile.PEFormatError, ValueError):
        return None
    if offset < 0:
        return None
    return offset


def executable_sections(pe: pefile.PE, data: bytes) -> list[dict[str, Any]]:
    sections: list[dict[str, Any]] = []
    for section in pe.sections:
        characteristics = int(section.Characteristics)
        if not characteristics & 0x20000000:
            continue
        start = int(section.VirtualAddress)
        size = max(int(section.Misc_VirtualSize), int(section.SizeOfRawData))
        raw_start = int(section.PointerToRawData)
        declared_raw_size = int(section.SizeOfRawData)
        if raw_start >= len(data) or declared_raw_size <= 0:
            continue
        raw_end = min(len(data), raw_start + declared_raw_size)
        sections.append(
            {
                "name": section.Name.rstrip(b"\0").decode("ascii", errors="replace"),
                "rva": start,
                "virtual_size": size,
                "raw_offset": raw_start,
                "raw_size": raw_end - raw_start,
                "declared_raw_size": declared_raw_size,
                "data": data[raw_start:raw_end],
            }
        )
    return sections


def decode_pe_name(value: bytes | str | None) -> str | None:
    if value is None:
        return None
    if isinstance(value, str):
        return value
    return value.decode("utf-8", errors="replace")


def section_characteristics(value: int) -> list[str]:
    flags = (
        (0x00000020, "code"),
        (0x00000040, "initialized-data"),
        (0x00000080, "uninitialized-data"),
        (0x20000000, "execute"),
        (0x40000000, "read"),
        (0x80000000, "write"),
        (0x02000000, "discardable"),
        (0x10000000, "shared"),
    )
    return [name for bit, name in flags if value & bit]


def pe_section_inventory(pe: pefile.PE, file_size: int) -> list[dict[str, Any]]:
    sections = []
    for section in pe.sections:
        raw_offset = int(section.PointerToRawData)
        declared_raw_size = int(section.SizeOfRawData)
        raw_size = max(0, min(file_size, raw_offset + declared_raw_size) - raw_offset)
        virtual_size = int(section.Misc_VirtualSize)
        mapped_size = max(virtual_size, declared_raw_size)
        characteristics = int(section.Characteristics)
        sections.append(
            {
                "name": section.Name.rstrip(b"\0").decode("ascii", errors="replace"),
                "rva_start": int(section.VirtualAddress),
                "rva_virtual_end_exclusive": int(section.VirtualAddress) + virtual_size,
                "rva_mapped_end_exclusive": int(section.VirtualAddress) + mapped_size,
                "virtual_size": virtual_size,
                "raw_offset": raw_offset,
                "raw_end_exclusive": min(file_size, raw_offset + declared_raw_size),
                "raw_size_declared": declared_raw_size,
                "raw_size_available": raw_size,
                "characteristics": f"0x{characteristics:08x}",
                "flags": section_characteristics(characteristics),
                "executable": bool(characteristics & 0x20000000),
            }
        )
    return sections


def import_inventory(pe: pefile.PE, image_base: int) -> list[dict[str, Any]]:
    imports = []
    for descriptor in getattr(pe, "DIRECTORY_ENTRY_IMPORT", []):
        symbols = []
        for item in descriptor.imports:
            address = int(item.address)
            symbols.append(
                {
                    "name": decode_pe_name(item.name),
                    "ordinal": (
                        int(item.ordinal)
                        if item.name is None and item.ordinal is not None
                        else None
                    ),
                    "hint": (
                        int(item.hint)
                        if item.name is not None and item.hint is not None
                        else None
                    ),
                    "iat_rva": address - image_base if address >= image_base else None,
                }
            )
        imports.append(
            {
                "module": decode_pe_name(descriptor.dll),
                "symbols": symbols,
            }
        )
    return imports


def export_inventory(pe: pefile.PE) -> list[dict[str, Any]]:
    exports = []
    directory = getattr(pe, "DIRECTORY_ENTRY_EXPORT", None)
    if directory is None:
        return exports
    for item in directory.symbols:
        exports.append(
            {
                "name": decode_pe_name(item.name),
                "ordinal": int(item.ordinal),
                "rva": int(item.address) if item.address is not None else None,
                "forwarder": decode_pe_name(item.forwarder),
            }
        )
    return exports


def merge_ranges(ranges: list[tuple[int, int]]) -> list[tuple[int, int]]:
    merged: list[tuple[int, int]] = []
    for start, end in sorted((start, end) for start, end in ranges if end > start):
        if merged and start <= merged[-1][1]:
            merged[-1] = (merged[-1][0], max(merged[-1][1], end))
        else:
            merged.append((start, end))
    return merged


def complement_ranges(start: int, end: int, occupied: list[tuple[int, int]]) -> list[tuple[int, int]]:
    result = []
    cursor = start
    for left, right in merge_ranges(
        [(max(start, left), min(end, right)) for left, right in occupied]
    ):
        if cursor < left:
            result.append((cursor, left))
        cursor = max(cursor, right)
    if cursor < end:
        result.append((cursor, end))
    return result


def instruction_ranges(instructions: list[tuple[int, int]]) -> list[dict[str, int]]:
    return [
        {"start_rva": start, "end_rva_exclusive": end, "byte_count": end - start}
        for start, end in merge_ranges(
            [(rva, rva + size) for rva, size in instructions]
        )
    ]


def intersect_ranges(
    left_ranges: list[tuple[int, int]],
    right_ranges: list[tuple[int, int]],
) -> list[tuple[int, int]]:
    left = merge_ranges(left_ranges)
    right = merge_ranges(right_ranges)
    result = []
    left_index = 0
    right_index = 0
    while left_index < len(left) and right_index < len(right):
        start = max(left[left_index][0], right[right_index][0])
        end = min(left[left_index][1], right[right_index][1])
        if end > start:
            result.append((start, end))
        if left[left_index][1] <= right[right_index][1]:
            left_index += 1
        else:
            right_index += 1
    return result


def capstone_skipdata_sweep(
    decoder: capstone.Cs,
    sections: list[dict[str, Any]],
    image_base: int,
) -> dict[int, dict[str, Any]]:
    result = {}
    prior_skipdata = decoder.skipdata
    decoder.skipdata = True
    try:
        for section in sections:
            start = section["rva"]
            decoded = []
            skipped = []
            for instruction in decoder.disasm(
                section["data"],
                image_base + start,
            ):
                rva = int(instruction.address - image_base)
                span = (rva, rva + int(instruction.size))
                if instruction.id == 0:
                    skipped.append(span)
                else:
                    decoded.append(span)
            result[start] = {
                "decoded_ranges": merge_ranges(decoded),
                "undecoded_ranges": merge_ranges(skipped),
            }
    finally:
        decoder.skipdata = prior_skipdata
    return result


def file_unmapped_ranges(
    file_size: int,
    size_of_headers: int,
    sections: list[dict[str, Any]],
) -> list[dict[str, int]]:
    mapped = [(0, min(file_size, max(0, size_of_headers)))]
    mapped.extend(
        (section["raw_offset"], section["raw_end_exclusive"])
        for section in sections
        if section["raw_size_available"] > 0
    )
    return [
        {
            "file_offset_start": start,
            "file_offset_end_exclusive": end,
            "byte_count": end - start,
        }
        for start, end in complement_ranges(0, file_size, mapped)
    ]


def section_for_rva(sections: list[dict[str, Any]], rva: int) -> dict[str, Any] | None:
    for section in sections:
        if section["rva"] <= rva < section["rva"] + section["virtual_size"]:
            return section
    return None


def direct_call_target(instruction: capstone.CsInsn, image_base: int) -> int | None:
    if instruction.mnemonic.lower() != "call" or len(instruction.operands) != 1:
        return None
    operand = instruction.operands[0]
    if operand.type != capstone.x86.X86_OP_IMM:
        return None
    return int(operand.imm - image_base)


def branch_target(instruction: capstone.CsInsn, image_base: int) -> int | None:
    if instruction.mnemonic.lower() == "ret":
        return None
    if instruction.mnemonic.lower().startswith("j") or instruction.mnemonic.lower() == "loop":
        if len(instruction.operands) == 1:
            operand = instruction.operands[0]
            if operand.type == capstone.x86.X86_OP_IMM:
                return int(operand.imm - image_base)
    return None


def instruction_map(
    decoder: capstone.Cs,
    sections: list[dict[str, Any]],
    image_base: int,
) -> tuple[dict[int, capstone.CsInsn], set[int], dict[int, set[int]]]:
    instructions: dict[int, capstone.CsInsn] = {}
    call_targets: set[int] = set()
    branch_targets: dict[int, set[int]] = {}
    for section in sections:
        address = image_base + section["rva"]
        for instruction in decoder.disasm(section["data"], address):
            rva = int(instruction.address - image_base)
            instructions[rva] = instruction
            target = direct_call_target(instruction, image_base)
            if target is not None and section_for_rva(sections, target) is not None:
                call_targets.add(target)
            target = branch_target(instruction, image_base)
            if target is not None and section_for_rva(sections, target) is not None:
                branch_targets.setdefault(rva, set()).add(target)
    return instructions, call_targets, branch_targets


def augment_from_seeds(
    decoder: capstone.Cs,
    sections: list[dict[str, Any]],
    image_base: int,
    seeds: set[int],
    instructions: dict[int, capstone.CsInsn],
    max_seed_bytes: int,
) -> None:
    for seed in sorted(seeds):
        section = section_for_rva(sections, seed)
        if section is None:
            continue
        offset = seed - section["rva"]
        if offset < 0 or offset >= len(section["data"]):
            continue
        code = section["data"][offset : offset + max_seed_bytes]
        for instruction in decoder.disasm(code, image_base + seed):
            instructions.setdefault(
                int(instruction.address - image_base),
                instruction,
            )


def rebuild_edges(
    instructions: dict[int, capstone.CsInsn],
    sections: list[dict[str, Any]],
    image_base: int,
) -> tuple[set[int], dict[int, set[int]]]:
    call_targets: set[int] = set()
    branch_targets: dict[int, set[int]] = {}
    for rva, instruction in instructions.items():
        target = direct_call_target(instruction, image_base)
        if target is not None and section_for_rva(sections, target) is not None:
            call_targets.add(target)
        target = branch_target(instruction, image_base)
        if target is not None and section_for_rva(sections, target) is not None:
            branch_targets.setdefault(rva, set()).add(target)
    return call_targets, branch_targets


def exported_rvas(pe: pefile.PE) -> set[int]:
    result: set[int] = set()
    directory = getattr(pe, "DIRECTORY_ENTRY_EXPORT", None)
    if directory is None:
        return result
    for symbol in directory.symbols:
        if symbol.address is not None:
            result.add(int(symbol.address))
    return result


def follow_function(
    start: int,
    instructions: dict[int, capstone.CsInsn],
    branch_targets: dict[int, set[int]],
    image_base: int,
) -> dict[str, Any] | None:
    if start not in instructions:
        return None

    pending = deque([start])
    visited: set[int] = set()
    processed_blocks: set[int] = set()
    block_starts: set[int] = {start}
    block_ends: set[int] = set()
    calls: set[int] = set()
    call_sites: list[dict[str, int]] = []
    total_bytes = 0
    minimum = start
    maximum = start

    while pending and len(block_starts) <= MAX_BLOCKS_PER_FUNCTION:
        block = pending.popleft()
        if block in processed_blocks or block not in instructions:
            continue
        processed_blocks.add(block)
        cursor = block
        while cursor in instructions and cursor not in visited:
            visited.add(cursor)
            instruction = instructions[cursor]
            size = int(instruction.size)
            total_bytes += size
            minimum = min(minimum, cursor)
            maximum = max(maximum, cursor + size)
            if total_bytes > MAX_FUNCTION_BYTES:
                break
            target = direct_call_target(instruction, image_base)
            if target is not None:
                calls.add(target)
                call_sites.append({"rva": cursor, "target_rva": target})
            mnemonic = instruction.mnemonic.lower()
            next_rva = cursor + size
            if mnemonic == "ret":
                block_ends.add(cursor + size)
                break
            targets = branch_targets.get(cursor, set())
            if targets:
                for target in targets:
                    if target not in block_starts:
                        block_starts.add(target)
                        pending.append(target)
                if mnemonic.startswith("j") and mnemonic not in {"jmp"}:
                    if next_rva in instructions:
                        block_starts.add(next_rva)
                        pending.append(next_rva)
                block_ends.add(cursor + size)
                break
            cursor = next_rva
        if total_bytes > MAX_FUNCTION_BYTES:
            break

    if not visited:
        return None
    end = max(maximum, max(block_ends, default=maximum))
    reached_ranges = instruction_ranges(
        [(rva, int(instructions[rva].size)) for rva in visited]
    )
    return {
        "rva": start,
        "size": end - start,
        "range_end_rva_exclusive": end,
        "instruction_bytes": sum(item["byte_count"] for item in reached_ranges),
        "instruction_ranges_rva": reached_ranges,
        "boundary_interpretation": (
            "candidate CFG extent only; the envelope may include gaps or overlap "
            "another candidate and is not a confirmed function boundary"
        ),
        "status": "needs_review",
        "confidence": "static-control-flow",
        "blocks": len(block_starts),
        "instruction_count": len(visited),
        "call_targets": sorted(target for target in calls if target in instructions),
        "call_sites": sorted(call_sites, key=lambda call: (call["rva"], call["target_rva"])),
    }


def is_acceptance_runtime(sample: dict[str, Any], binary: dict[str, Any]) -> bool:
    sample_root = Path(sample["root"])
    relative = Path(binary["path"]).relative_to(sample_root)
    root_name = sample_root.name.casefold()
    relative_parts = [part.casefold() for part in relative.parts]
    name = relative.name.casefold()

    if root_name == "cs2_full_v401":
        return (
            relative_parts == ["system", "cs2_open.exe"]
            or relative_parts == ["system", "cs2conf.dll"]
        )
    if root_name == "\u725b\u987f\u4e0e\u82f9\u679c\u6811":
        return (
            name in {
                "\u725b\u987f\u4e0e\u82f9\u679c\u6811.exe",
                "cs2conf.dll",
                "hook.dll",
            }
            and len(relative_parts) == 1
        )
    if root_name.startswith("[170526]") and "nekopara_vol3" in relative_parts:
        return name in {
            "nekoparavol3.exe",
            "cs2confx.dll",
            "emotedriver.dll",
        }
    return False


def binary_relative_path(
    sample: dict[str, Any],
    binary: dict[str, Any],
) -> Path | None:
    try:
        return Path(binary["path"]).relative_to(Path(sample["root"]))
    except (KeyError, ValueError):
        return None


def runtime_path_exclusion_reason(relative: Path) -> str | None:
    excluded_parts = {value.casefold() for value in RUNTIME_PATH_EXCLUSIONS}
    for part in relative.parts[:-1]:
        if part.casefold() in excluded_parts:
            return f"path component `{part}` is a tool/patch/backup directory"
    return None


def runtime_executable_decision(
    binary: dict[str, Any],
    relative: Path,
) -> tuple[bool, str, str]:
    name = relative.name.casefold()
    if name in RUNTIME_EXE_EXCLUSIONS:
        return (
            False,
            "excluded-tool",
            f"executable `{relative.name}` is a known tool, installer, or patch helper",
        )
    if any(token in name for token in RUNTIME_TRANSLATION_EXE_TOKENS):
        return (
            False,
            "excluded-translation-patch",
            f"executable `{relative.name}` is a translation patch or launcher",
        )
    if any(token in name for token in RUNTIME_EXE_NAME_TOKENS):
        return (
            False,
            "excluded-tool",
            f"executable `{relative.name}` matches a tool, installer, or launcher name",
        )
    if name in {"cs2_open.exe"}:
        return True, "runtime-executable", "CatSystem2 reference entrypoint"
    if name == "nekoparavol3.exe":
        return True, "runtime-executable", "NEKOPARA vol.3 game entrypoint"
    if name.endswith(("_cn.exe", "_chs.exe")):
        return (
            True,
            "runtime-executable-variant",
            "root-level localized runtime executable",
        )
    if len(relative.parts) != 1:
        return (
            False,
            "excluded-nonroot-executable",
            "nested executable is not a selected game runtime entrypoint",
        )

    imported = {
        item.casefold()
        for item in binary.get("pe", {}).get("import_dlls", [])
    }
    if name == "cs2.exe":
        return True, "runtime-executable", "root-level CatSystem2 engine name"
    if imported & ENGINE_RUNTIME_IMPORTS:
        return (
            True,
            "runtime-executable-variant",
            "root-level executable imports CatSystem2 multimedia/input runtime DLLs",
        )
    return (
        False,
        "excluded-unconfirmed-executable",
        "root-level executable lacks the CatSystem2 runtime import signature",
    )


def dynamic_local_dll_references(
    sample: dict[str, Any],
    executable: dict[str, Any],
    dll_names: set[str],
) -> set[str]:
    path = WORKSPACE / Path(executable["path"].replace("/", os.sep))
    try:
        data = path.read_bytes().lower()
    except OSError:
        return set()
    references = set()
    for name in dll_names:
        encoded = name.encode("ascii", errors="ignore")
        if encoded and (
            encoded in data
            or name.encode("utf-16le", errors="ignore").lower() in data
        ):
            references.add(name)
    return references


def select_workspace_runtime(
    sample: dict[str, Any],
) -> tuple[list[dict[str, Any]], list[dict[str, Any]]]:
    """Select runtime files and retain an auditable decision for every PE."""
    candidates: list[tuple[dict[str, Any], Path]] = []
    audit: list[dict[str, Any]] = []
    for binary in sample.get("binaries", []):
        relative = binary_relative_path(sample, binary)
        record = {
            "sample_id": sample["sample_id"],
            "sample_name": sample["name"],
            "binary_path": binary.get("path"),
            "name": binary.get("name"),
            "scope": binary.get("scope"),
            "selected": False,
            "selection_class": "excluded",
            "reason": None,
        }
        if binary.get("scope") == "patch_or_backup":
            record["reason"] = "inventory scope is patch_or_backup"
            audit.append(record)
            continue
        if relative is None:
            record["reason"] = "binary path is outside the sample root"
            audit.append(record)
            continue
        path_reason = runtime_path_exclusion_reason(relative)
        if path_reason:
            record["reason"] = path_reason
            audit.append(record)
            continue
        suffix = relative.suffix.casefold()
        if suffix not in {".exe", ".dll"}:
            record["reason"] = "not a PE runtime extension"
            audit.append(record)
            continue
        candidates.append((binary, relative))
        audit.append(record)

    executable_entries: list[tuple[dict[str, Any], Path, str, str]] = []
    for binary, relative in candidates:
        if relative.suffix.casefold() != ".exe":
            continue
        include, selection_class, reason = runtime_executable_decision(
            binary,
            relative,
        )
        if not include:
            for record in audit:
                if record["binary_path"] == binary["path"]:
                    record["selection_class"] = selection_class
                    record["reason"] = reason
                    break
            continue
        executable_entries.append((binary, relative, selection_class, reason))

    explicit_primaries = [
        item
        for item in executable_entries
        if item[2] == "runtime-executable"
    ]
    if explicit_primaries:
        primary = max(explicit_primaries, key=lambda item: int(item[0].get("size", 0)))
    elif executable_entries:
        primary = max(executable_entries, key=lambda item: int(item[0].get("size", 0)))
    else:
        primary = None

    executable_entries = [
        (
            binary,
            relative,
            "runtime-executable"
            if primary is not None and binary["path"] == primary[0]["path"]
            else "runtime-executable-variant",
            reason,
        )
        for binary, relative, _, reason in executable_entries
    ]

    dll_candidates = [
        (binary, relative)
        for binary, relative in candidates
        if relative.suffix.casefold() == ".dll"
        and relative.name.casefold() not in RUNTIME_DLL_EXCLUSIONS
    ]
    dlls_by_name: dict[str, list[tuple[dict[str, Any], Path]]] = {}
    for binary, relative in dll_candidates:
        dlls_by_name.setdefault(relative.name.casefold(), []).append((binary, relative))
    for binary, relative in candidates:
        if (
            relative.suffix.casefold() == ".dll"
            and relative.name.casefold() in RUNTIME_DLL_EXCLUSIONS
        ):
            reason = (
                "only linked from the excluded Grisaia2 translation patch DLL"
                if relative.name.casefold() == "libpng16.dll"
                else "known non-runtime installer or distribution component"
            )
            for record in audit:
                if record["binary_path"] == binary["path"]:
                    record["selection_class"] = "excluded-nonruntime-dll"
                    record["reason"] = reason
                    break

    selected_dll_paths: dict[str, tuple[str, str]] = {}
    discovery_reasons: dict[str, str] = {}
    pending_dll_names = []
    for binary, _, _, _ in executable_entries:
        for imported in binary.get("pe", {}).get("import_dlls", []):
            name = imported.casefold()
            if name.endswith(".dll"):
                pending_dll_names.append(name)
                discovery_reasons.setdefault(
                    name,
                    "listed in the import table of a selected runtime executable",
                )
    dll_names = set(dlls_by_name)
    for binary, _, _, _ in executable_entries:
        for name in dynamic_local_dll_references(sample, binary, dll_names):
            pending_dll_names.append(name)
            discovery_reasons.setdefault(
                name,
                "DLL filename referenced in selected runtime executable bytes; "
                "possible dynamic load",
            )
    known_names = set(KNOWN_RUNTIME_DLLS)
    known_names.update(
        relative.name.casefold()
        for binary, relative in dll_candidates
        if (
            relative.name.casefold().startswith(KNOWN_ENGINE_DLL_PREFIXES)
            or relative.name.casefold().endswith(("_cn.dll", "_chs.dll"))
        )
    )
    pending_dll_names.extend(sorted(known_names))
    for name in known_names:
        discovery_reasons.setdefault(
            name,
            "recognized CatSystem2 or game-specific runtime support DLL",
        )
    visited_dll_names: set[str] = set()
    while pending_dll_names:
        name = pending_dll_names.pop()
        if name in visited_dll_names:
            continue
        visited_dll_names.add(name)
        matches = dlls_by_name.get(name, [])
        for binary, relative in matches:
            path = binary["path"]
            if name.startswith(KNOWN_ENGINE_DLL_PREFIXES):
                selection_class = "runtime-engine-dll"
                reason = "known CatSystem2 engine configuration DLL"
            elif name in KNOWN_RUNTIME_DLLS:
                selection_class = "runtime-support-dll"
                reason = discovery_reasons.get(
                    name,
                    "known game host, media, audio, or version-specific runtime DLL",
                )
            else:
                selection_class = "runtime-dependency-dll"
                reason = discovery_reasons.get(
                    name,
                    "transitive local DLL dependency of a selected runtime binary",
                )
            selected_dll_paths[path] = (selection_class, reason)
            pending_dll_names.extend(
                imported.casefold()
                for imported in binary.get("pe", {}).get("import_dlls", [])
                if imported.casefold().endswith(".dll")
            )
            for imported in binary.get("pe", {}).get("import_dlls", []):
                name = imported.casefold()
                if name.endswith(".dll"):
                    discovery_reasons.setdefault(
                        name,
                        f"imported by selected runtime DLL `{relative.name}`",
                    )

    selected: list[dict[str, Any]] = []
    for binary, relative, selection_class, reason in executable_entries:
        selected.append(
            {
                **binary,
                "selection_class": selection_class,
                "selection_reason": reason,
            }
        )

    for binary, relative in dll_candidates:
        decision = selected_dll_paths.get(binary["path"])
        if decision is None:
            continue
        selection_class, reason = decision
        selected.append(
            {
                **binary,
                "selection_class": selection_class,
                "selection_reason": reason,
            }
        )
    for record in audit:
        selected_binary = next(
            (item for item in selected if item["path"] == record["binary_path"]),
            None,
        )
        if selected_binary is not None:
            record["selected"] = True
            record["selection_class"] = selected_binary["selection_class"]
            record["reason"] = selected_binary["selection_reason"]
        elif record["reason"] is None:
            record["reason"] = "not selected by the runtime executable/DLL rules"
    return selected, audit


def is_workspace_runtime(sample: dict[str, Any], binary: dict[str, Any]) -> bool:
    """Compatibility wrapper for callers that ask about one binary."""
    selected, _ = select_workspace_runtime(sample)
    return any(item["path"] == binary.get("path") for item in selected)


def recover_binary(
    binary: dict[str, Any],
    sample: dict[str, Any],
) -> dict[str, Any]:
    root = WORKSPACE
    path = root / Path(binary["path"].replace("/", os.sep))
    result = {
        "binary_path": binary["path"],
        "sha256": binary["sha256"],
        "scope": binary["scope"],
        "runtime_role": binary.get("runtime_role", "unclassified"),
        "analysis_status": "error",
        "sections": [],
        "pe_sections": [],
        "imports": [],
        "exports": [],
        "file_layout": {},
        "static_coverage": {},
        "functions": [],
        "diagnostics": [],
    }
    if not path.is_file():
        result["diagnostics"].append("binary path does not exist")
        return result
    if sha256_file(path) != binary["sha256"]:
        result["diagnostics"].append("binary changed since inventory")
        return result

    try:
        data = path.read_bytes()
        pe = pefile.PE(data=data, fast_load=False)
        machine = int(pe.FILE_HEADER.Machine)
        if machine != 0x014C:
            result["diagnostics"].append(f"unsupported machine 0x{machine:04x}")
            return result
        image_base = int(pe.OPTIONAL_HEADER.ImageBase)
        all_sections = pe_section_inventory(pe, len(data))
        imports = import_inventory(pe, image_base)
        exports = export_inventory(pe)
        result["pe_metadata"] = {
            "machine": f"0x{machine:04x}",
            "architecture": "x86",
            "image_base": image_base,
            "entrypoint_rva": int(pe.OPTIONAL_HEADER.AddressOfEntryPoint),
            "entrypoint_va": image_base + int(pe.OPTIONAL_HEADER.AddressOfEntryPoint),
            "subsystem": int(pe.OPTIONAL_HEADER.Subsystem),
            "file_size": len(data),
            "size_of_headers": int(pe.OPTIONAL_HEADER.SizeOfHeaders),
            "size_of_image": int(pe.OPTIONAL_HEADER.SizeOfImage),
        }
        result["pe_sections"] = all_sections
        result["imports"] = imports
        result["exports"] = exports
        result["file_layout"] = {
            "unmapped_or_overlay_ranges": file_unmapped_ranges(
                len(data),
                int(pe.OPTIONAL_HEADER.SizeOfHeaders),
                all_sections,
            )
        }
        sections = executable_sections(pe, data)
        result["sections"] = [
            {
                "name": section["name"],
                "rva": section["rva"],
                "virtual_size": section["virtual_size"],
                "raw_offset": section["raw_offset"],
                "raw_size": section["raw_size"],
                "declared_raw_size": section["declared_raw_size"],
            }
            for section in sections
        ]
        decoder = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_32)
        decoder.detail = True
        skipdata_sweep = capstone_skipdata_sweep(
            decoder,
            sections,
            int(pe.OPTIONAL_HEADER.ImageBase),
        )
        instructions, call_targets, branch_targets = instruction_map(
            decoder,
            sections,
            int(pe.OPTIONAL_HEADER.ImageBase),
        )
        root_seeds = exported_rvas(pe)
        entrypoint_rva = int(pe.OPTIONAL_HEADER.AddressOfEntryPoint)
        root_seeds.add(entrypoint_rva)
        augment_from_seeds(
            decoder,
            sections,
            image_base,
            root_seeds,
            instructions,
            MAX_SEED_BYTES,
        )
        call_targets, branch_targets = rebuild_edges(
            instructions,
            sections,
            image_base,
        )
        secondary_seeds = set(sorted(call_targets)[:MAX_SECONDARY_SEEDS])
        augment_from_seeds(
            decoder,
            sections,
            image_base,
            secondary_seeds,
            instructions,
            MAX_CALL_SEED_BYTES,
        )
        call_targets, branch_targets = rebuild_edges(
            instructions,
            sections,
            image_base,
        )
        seeds = root_seeds | call_targets
        functions = []
        for seed in sorted(seeds):
            candidate = follow_function(
                seed,
                instructions,
                branch_targets,
                image_base,
            )
            if candidate is not None:
                evidence = []
                if seed == entrypoint_rva:
                    evidence.append("entrypoint")
                if seed in exported_rvas(pe):
                    evidence.append("export")
                if seed in call_targets:
                    evidence.append("direct-call-target")
                candidate["evidence"] = evidence
                candidate["confidence"] = (
                    "entrypoint"
                    if "entrypoint" in evidence
                    else "export"
                    if "export" in evidence
                    else "direct-call-target"
                )
                candidate["virtual_address"] = image_base + seed
                functions.append(candidate)
        candidate_rvas = {function["rva"] for function in functions}
        called_by: dict[int, list[int]] = {rva: [] for rva in candidate_rvas}
        call_edges = []
        for function in functions:
            retained_targets = set()
            for call in function["call_sites"]:
                target = call["target_rva"]
                call["resolved_candidate"] = target in candidate_rvas
                edge = {
                    "caller_rva": function["rva"],
                    "call_site_rva": call["rva"],
                    "target_rva": target,
                    "resolved_candidate": target in candidate_rvas,
                }
                call_edges.append(edge)
                if target in called_by:
                    called_by[target].append(function["rva"])
                    retained_targets.add(target)
            function["call_targets"] = sorted(retained_targets)
        for function in functions:
            function["called_by"] = sorted(set(called_by[function["rva"]]))
        sorted_functions = sorted(functions, key=lambda item: item["rva"])
        active: list[dict[str, Any]] = []
        overlapping_rvas: set[int] = set()
        for function in sorted_functions:
            active = [
                prior
                for prior in active
                if prior["range_end_rva_exclusive"] > function["rva"]
            ]
            for prior in active:
                overlapping_rvas.add(prior["rva"])
                overlapping_rvas.add(function["rva"])
            active.append(function)
        for function in functions:
            function["overlaps_other_candidate_envelope"] = (
                function["rva"] in overlapping_rvas
            )

        by_rva = {function["rva"]: function for function in functions}
        reachable = set()
        pending = deque([entrypoint_rva] if entrypoint_rva in by_rva else [])
        while pending:
            current = pending.popleft()
            if current in reachable:
                continue
            reachable.add(current)
            pending.extend(
                target
                for target in by_rva[current]["call_targets"]
                if target not in reachable
            )
        for function in functions:
            function["reachable_from_entrypoint"] = function["rva"] in reachable

        section_coverage = []
        for section in sections:
            start = section["rva"]
            raw_end = start + len(section["data"])
            seeded_instruction_ranges = merge_ranges(
                [
                    (rva, rva + int(instruction.size))
                    for rva, instruction in instructions.items()
                    if start <= rva < raw_end
                ]
            )
            sweep_data = skipdata_sweep[start]
            section_instruction_ranges = [
                (max(start, begin), min(raw_end, end))
                for begin, end in sweep_data["decoded_ranges"]
                if begin < raw_end and end > start
            ]
            section_instruction_ranges = merge_ranges(section_instruction_ranges)
            candidate_ranges = merge_ranges(
                [
                    (
                        max(start, entry["start_rva"]),
                        min(raw_end, entry["end_rva_exclusive"]),
                    )
                    for function in functions
                    for entry in function["instruction_ranges_rva"]
                    if start <= entry["start_rva"] < raw_end
                    and entry["end_rva_exclusive"] > start
                ]
            )
            decoded_bytes = sum(end - begin for begin, end in section_instruction_ranges)
            candidate_bytes = sum(end - begin for begin, end in candidate_ranges)
            candidate_bytes_decoded_by_sweep = sum(
                end - begin
                for begin, end in intersect_ranges(
                    section_instruction_ranges,
                    candidate_ranges,
                )
            )
            unknown_ranges = [
                (max(start, begin), min(raw_end, end))
                for begin, end in sweep_data["undecoded_ranges"]
                if begin < raw_end and end > start
            ]
            section_coverage.append(
                {
                    "name": section["name"],
                    "rva_start": start,
                    "rva_raw_end_exclusive": raw_end,
                    "rva_mapped_end_exclusive": start + section["virtual_size"],
                    "file_offset_start": section["raw_offset"],
                    "file_offset_end_exclusive": section["raw_offset"] + len(section["data"]),
                    "raw_bytes": len(section["data"]),
                    "decoded_instruction_bytes": decoded_bytes,
                    "decoded_instruction_coverage_percent": round(
                        decoded_bytes * 100 / len(section["data"]), 4
                    )
                    if section["data"]
                    else 0.0,
                    "candidate_instruction_bytes": candidate_bytes,
                    "candidate_instruction_coverage_percent": round(
                        candidate_bytes * 100 / len(section["data"]), 4
                    )
                    if section["data"]
                    else 0.0,
                    "seed_augmented_instruction_bytes": sum(
                        end - begin for begin, end in seeded_instruction_ranges
                    ),
                    "decoded_not_candidate_bytes": max(
                        0,
                        decoded_bytes - candidate_bytes_decoded_by_sweep,
                    ),
                    "undecoded_ranges": [
                        {
                            "rva_start": begin,
                            "rva_end_exclusive": end,
                            "file_offset_start": section["raw_offset"] + begin - start,
                            "byte_count": end - begin,
                        }
                        for begin, end in unknown_ranges
                    ],
                }
            )
        total_raw_bytes = sum(section["raw_bytes"] for section in section_coverage)
        total_decoded_bytes = sum(
            section["decoded_instruction_bytes"] for section in section_coverage
        )
        total_candidate_bytes = sum(
            section["candidate_instruction_bytes"] for section in section_coverage
        )
        result["static_coverage"] = {
            "method": (
                "Full-section Capstone x86 skip-data linear sweep; data pseudo-"
                "instructions are excluded from decoded bytes. Candidate recovery "
                "uses the separate entrypoint/export/direct-call seed map."
            ),
            "executable_raw_bytes": total_raw_bytes,
            "decoded_instruction_bytes": total_decoded_bytes,
            "decoded_instruction_coverage_percent": round(
                total_decoded_bytes * 100 / total_raw_bytes, 4
            )
            if total_raw_bytes
            else 0.0,
            "candidate_instruction_bytes": total_candidate_bytes,
            "candidate_instruction_coverage_percent": round(
                total_candidate_bytes * 100 / total_raw_bytes, 4
            )
            if total_raw_bytes
            else 0.0,
            "sections": section_coverage,
        }

        result["analysis_status"] = "candidate-recovery"
        result["functions"] = functions
        result["entrypoint_rva"] = entrypoint_rva
        result["root_seeds"] = [
            {
                "rva": seed,
                "kind": (
                    "entrypoint"
                    if seed == entrypoint_rva
                    else "export"
                    if seed in exported_rvas(pe)
                    else "direct-call-target"
                ),
            }
            for seed in sorted(root_seeds)
        ]
        result["call_edges"] = sorted(
            call_edges,
            key=lambda edge: (
                edge["caller_rva"],
                edge["call_site_rva"],
                edge["target_rva"],
            ),
        )
        result["summary"] = {
            "candidate_count": len(functions),
            "entrypoint_reachable_count": len(reachable),
            "direct_call_edge_count": len(call_edges),
            "resolved_call_edge_count": sum(
                edge["resolved_candidate"] for edge in call_edges
            ),
            "root_seed_count": len(root_seeds),
        }
    except (OSError, pefile.PEFormatError, ValueError, IndexError) as exc:
        result["diagnostics"].append(f"{type(exc).__name__}: {exc}")
    return result


def assign_runtime_roles(binaries: list[dict[str, Any]]) -> None:
    groups: dict[str, list[dict[str, Any]]] = {}
    for binary in binaries:
        groups.setdefault(binary["sample_id"], []).append(binary)
    for group in groups.values():
        for binary in group:
            name = Path(binary["binary_path"]).name.casefold()
            selection_class = binary.get("selection_class", "")
            if selection_class == "runtime-engine-dll":
                role = "engine-configuration-plugin"
            elif selection_class == "runtime-support-dll":
                role = "runtime-support-library"
            elif selection_class == "runtime-dependency-dll":
                role = "runtime-dependent-library"
            elif selection_class == "runtime-variant-dll":
                role = "runtime-variant-library"
            elif selection_class == "runtime-executable":
                role = "primary-runtime-executable"
            elif selection_class == "runtime-executable-variant":
                role = "runtime-variant-executable"
            elif name in {"cs2conf.dll", "cs2confx.dll"}:
                role = "engine-configuration-plugin"
            else:
                role = "unclassified"
            binary["runtime_role"] = role


def workspace_relative(path: Path) -> str:
    try:
        return str(path.resolve().relative_to(WORKSPACE)).replace("\\", "/")
    except ValueError:
        return str(path.resolve())


def review_existing_acceptance_report(
    path: Path,
    binaries: list[dict[str, Any]],
) -> dict[str, Any]:
    review: dict[str, Any] = {
        "path": workspace_relative(path),
        "read_status": "missing",
        "legacy_status": None,
        "legacy_scope": None,
        "findings": [],
        "binaries": [],
    }
    if not path.is_file():
        review["findings"].append("existing acceptance ledger is missing")
        return review
    try:
        legacy = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError, json.JSONDecodeError) as exc:
        review["read_status"] = "error"
        review["findings"].append(f"cannot read existing acceptance ledger: {exc}")
        return review

    review["read_status"] = "ok"
    review["legacy_status"] = legacy.get("status")
    review["legacy_scope"] = legacy.get("scope")
    scope_description = str(legacy.get("scope", {}).get("description", ""))
    if "candidate" in scope_description.lower():
        review["findings"].append(
            "legacy ledger explicitly describes candidates, not semantic implementations"
        )
    legacy_by_path = {
        item.get("binary_path"): item for item in legacy.get("binaries", [])
    }
    for binary in binaries:
        legacy_binary = legacy_by_path.get(binary["binary_path"])
        item = {
            "binary_path": binary["binary_path"],
            "legacy_present": legacy_binary is not None,
            "legacy_candidate_count": (
                legacy_binary.get("summary", {}).get("candidate_count")
                if legacy_binary
                else None
            ),
            "legacy_fields_missing_for_static_review": [],
        }
        if legacy_binary is not None:
            for field in ("pe_sections", "imports", "exports", "static_coverage"):
                if field not in legacy_binary:
                    item["legacy_fields_missing_for_static_review"].append(field)
            if any(item["legacy_fields_missing_for_static_review"]):
                review["findings"].append(
                    f"{binary['binary_path']}: legacy ledger lacks "
                    "one or more PE/static coverage fields"
                )
        else:
            review["findings"].append(
                f"{binary['binary_path']}: legacy ledger has no matching binary entry"
            )
        review["binaries"].append(item)
    return review


def build_coverage_payload(
    inventory_path: Path,
    binaries: list[dict[str, Any]],
    acceptance_path: Path,
    ghidra_status: str,
    selection_audit: list[dict[str, Any]] | None = None,
) -> dict[str, Any]:
    total_candidates = sum(len(binary["functions"]) for binary in binaries)
    total_reachable = sum(
        binary.get("summary", {}).get("entrypoint_reachable_count", 0)
        for binary in binaries
    )
    total_raw = sum(
        binary.get("static_coverage", {}).get("executable_raw_bytes", 0)
        for binary in binaries
    )
    total_decoded = sum(
        binary.get("static_coverage", {}).get("decoded_instruction_bytes", 0)
        for binary in binaries
    )
    total_candidate_bytes = sum(
        binary.get("static_coverage", {}).get("candidate_instruction_bytes", 0)
        for binary in binaries
    )
    total_call_edges = sum(
        len(binary.get("call_edges", [])) for binary in binaries
    )
    total_resolved_call_edges = sum(
        sum(
            bool(edge.get("resolved_candidate"))
            for edge in binary.get("call_edges", [])
        )
        for binary in binaries
    )
    selection_audit = selection_audit or []
    selected_classes: dict[str, int] = {}
    for binary in binaries:
        selection_class = binary.get("selection_class", "unspecified")
        selected_classes[selection_class] = (
            selected_classes.get(selection_class, 0) + 1
        )
    audited_sample_ids = {record["sample_id"] for record in selection_audit}
    reference_sample_ids = {"cs2_full_v401"}
    return {
        "schema_version": "1.0.0",
        "status": "static-evidence-only",
        "generated_at_utc": datetime.now(timezone.utc)
        .replace(microsecond=0)
        .isoformat()
        .replace("+00:00", "Z"),
        "source_inventory": workspace_relative(inventory_path),
        "scope": {
            "sample_ids": sorted({binary["sample_id"] for binary in binaries}),
            "binary_count": len(binaries),
            "sample_count": len({binary["sample_id"] for binary in binaries}),
            "game_sample_count": len(
                audited_sample_ids - reference_sample_ids
            ),
            "reference_sample_count": len(
                audited_sample_ids & reference_sample_ids
            ),
            "candidate_count": total_candidates,
            "entrypoint_reachable_candidate_count": total_reachable,
            "executable_raw_bytes": total_raw,
            "decoded_instruction_bytes": total_decoded,
            "decoded_instruction_coverage_percent": round(
                total_decoded * 100 / total_raw, 4
            )
            if total_raw
            else 0.0,
            "candidate_instruction_bytes": total_candidate_bytes,
            "candidate_instruction_coverage_percent": round(
                total_candidate_bytes * 100 / total_raw, 4
            )
            if total_raw
            else 0.0,
            "direct_call_edge_count": total_call_edges,
            "resolved_call_edge_count": total_resolved_call_edges,
            "semantic_implementation_count": 0,
            "semantic_implementation_claim": False,
        },
        "selection": {
            "mode": "all-runtime" if selection_audit else "explicit-runtime",
            "selected_binary_count": len(binaries),
            "excluded_binary_count": sum(
                not record["selected"] for record in selection_audit
            ),
            "selected_by_class": selected_classes,
            "audit": selection_audit,
        },
        "method": {
            "decoder": "Capstone",
            "pe_parser": "pefile",
            "architecture": "x86 32-bit",
            "candidate_seed_sources": [
                "PE entrypoint",
                "PE exports",
                "direct immediate call targets in decoded executable sections",
            ],
            "boundary_rule": (
                "candidate envelope from reachable decoded basic blocks; "
                "instruction_ranges_rva is the auditable byte union. The envelope "
                "is not a confirmed function boundary."
            ),
            "unknown_range_rule": (
                "unknown ranges are executable raw section bytes not covered by "
                "Capstone-decoded instruction ranges; they include possible data, "
                "padding, invalid/undecodable bytes, and missed code."
            ),
            "coverage_interpretation": (
                "decoded_instruction_coverage_percent is syntactic linear-sweep "
                "decodability, not reachable-code coverage. "
                "candidate_instruction_coverage_percent is the candidate "
                "instruction-byte union divided by executable raw bytes. Neither "
                "metric represents semantic implementation coverage."
            ),
        },
        "ghidra": {
            "status": ghidra_status,
            "service_used": not ghidra_status.casefold().startswith("unavailable"),
            "analysis_contribution": (
                "status recorded; Ghidra function exports are merged by "
                "ghidra_runtime_ledger.py"
            ),
        },
        "existing_acceptance_ledger_review": review_existing_acceptance_report(
            acceptance_path,
            binaries,
        ),
        "binaries": [
            {
                "sample_id": binary["sample_id"],
                "sample_name": binary["sample_name"],
                "binary_path": binary["binary_path"],
                "sha256": binary["sha256"],
                "scope": binary["scope"],
                "runtime_role": binary["runtime_role"],
                "selection_class": binary.get("selection_class"),
                "selection_reason": binary.get("selection_reason"),
                "analysis_status": binary["analysis_status"],
                "diagnostics": binary["diagnostics"],
                "pe_metadata": binary.get("pe_metadata", {}),
                "pe_sections": binary.get("pe_sections", []),
                "executable_sections": binary.get("sections", []),
                "imports": binary.get("imports", []),
                "exports": binary.get("exports", []),
                "file_layout": binary.get("file_layout", {}),
                "static_coverage": binary.get("static_coverage", {}),
                "candidate_summary": binary.get("summary", {}),
                "function_candidates": binary.get("functions", []),
                "call_edges": binary.get("call_edges", []),
            }
            for binary in binaries
        ],
        "interpretation": [
            "All function records are static candidates requiring review.",
            "No candidate is treated as a recovered semantic implementation.",
            "No default return value or stub behavior is inferred from absence of evidence.",
            "Ghidra function exports are independent evidence and must not be "
            "read as semantic implementation status.",
        ],
    }


def format_rva(value: int | None) -> str:
    return "n/a" if value is None else f"0x{value:08X}"


def render_coverage_markdown(payload: dict[str, Any]) -> str:
    scope = payload["scope"]
    selection_mode = payload.get("selection", {}).get("mode", "unspecified")
    scope_description = (
        "the selected runtime sample set"
        if selection_mode == "all-runtime"
        else "the configured acceptance sample set"
    )
    lines = [
        "# Runtime Function Static Coverage",
        "",
        f"This report is static evidence for {scope_description}. Function "
        "records are candidates only; they are not semantic implementations.",
        "",
        f"- Generated (UTC): `{payload['generated_at_utc']}`",
        f"- Binary count: `{scope['binary_count']}`",
        f"- Candidate count: `{scope['candidate_count']}`",
        f"- Entrypoint-reachable candidates: `{scope['entrypoint_reachable_candidate_count']}`",
        f"- Executable raw bytes: `{scope['executable_raw_bytes']}`",
        f"- Capstone-decoded bytes: `{scope['decoded_instruction_bytes']}` "
        f"(`{scope['decoded_instruction_coverage_percent']}%`)",
        f"- Candidate instruction bytes: `{scope['candidate_instruction_bytes']}` "
        f"(`{scope['candidate_instruction_coverage_percent']}%`)",
        f"- Direct call edges: `{scope['direct_call_edge_count']}` "
        f"(resolved candidate targets: `{scope['resolved_call_edge_count']}`)",
        "- Semantic implementations claimed: `0`",
        "- Coverage meaning: linear-sweep decodability and candidate-byte ratios; "
        "neither is semantic implementation coverage",
        "",
        "## Runtime Selection",
        "",
        f"- Mode: `{payload.get('selection', {}).get('mode', 'unspecified')}`",
        f"- Selected binaries: `{payload.get('selection', {}).get('selected_binary_count', scope['binary_count'])}`",
        f"- Excluded inventory entries: `{payload.get('selection', {}).get('excluded_binary_count', 0)}`",
        f"- Game samples: `{scope.get('game_sample_count', scope.get('sample_count', 0))}`",
        f"- Reference samples: `{scope.get('reference_sample_count', 0)}`",
        f"- Selection classes: `{payload.get('selection', {}).get('selected_by_class', {})}`",
        "",
        "## Tool Status",
        "",
        f"- Ghidra: `{payload['ghidra']['status']}`",
        "- Static fallback: `Capstone 5.x + pefile`",
        "",
        "## Existing Ledger Review",
        "",
        f"- File: `{payload['existing_acceptance_ledger_review']['path']}`",
        f"- Read status: `{payload['existing_acceptance_ledger_review']['read_status']}`",
        f"- Legacy status: `{payload['existing_acceptance_ledger_review']['legacy_status']}`",
    ]
    for finding in payload["existing_acceptance_ledger_review"]["findings"]:
        lines.append(f"- Finding: {finding}")

    for binary in payload["binaries"]:
        metadata = binary["pe_metadata"]
        coverage = binary["static_coverage"]
        summary = binary["candidate_summary"]
        lines.extend(
            [
                "",
                f"## `{binary['binary_path']}`",
                "",
                f"- SHA-256: `{binary['sha256']}`",
                f"- Image base: `{format_rva(metadata.get('image_base'))}`",
                f"- Entry point RVA: `{format_rva(metadata.get('entrypoint_rva'))}`",
                f"- Candidate functions: `{summary.get('candidate_count', 0)}`",
                f"- Entrypoint-reachable candidates: "
                f"`{summary.get('entrypoint_reachable_count', 0)}`",
                f"- Imports: `{sum(len(item['symbols']) for item in binary['imports'])}` "
                f"from `{len(binary['imports'])}` modules",
                f"- Exports: `{len(binary['exports'])}`",
                f"- Decoded executable bytes: "
                f"`{coverage.get('decoded_instruction_bytes', 0)}` "
                f"(`{coverage.get('decoded_instruction_coverage_percent', 0)}%`)",
                f"- Candidate instruction bytes: "
                f"`{coverage.get('candidate_instruction_bytes', 0)}` "
                f"(`{coverage.get('candidate_instruction_coverage_percent', 0)}%`)",
                "",
                "### PE Sections",
                "",
                "| Section | RVA range | File range | Raw bytes | Flags |",
                "| --- | --- | --- | ---: | --- |",
            ]
        )
        for section in binary["pe_sections"]:
            lines.append(
                f"| `{section['name']}` | "
                f"`{format_rva(section['rva_start'])}-{format_rva(section['rva_mapped_end_exclusive'])}` | "
                f"`0x{section['raw_offset']:08X}-0x{section['raw_end_exclusive']:08X}` | "
                f"{section['raw_size_available']} | {', '.join(section['flags'])} |"
            )
        lines.extend(
            [
                "",
                "### Imports and Exports",
                "",
                f"- Import modules: "
                f"{', '.join('`' + str(item['module']) + '`' for item in binary['imports']) or '`none`'}",
            ]
        )
        export_names = [
            item["name"] or f"ordinal:{item['ordinal']}"
            for item in binary["exports"]
        ]
        lines.append(
            f"- Export symbols: "
            f"{', '.join(f'`{name}`' for name in export_names) or '`none`'}"
        )
        lines.extend(
            [
                "",
                "### Executable Section Coverage",
                "",
                "| Section | Decoded bytes | Decoded % | Candidate bytes | Candidate % | Unknown ranges |",
                "| --- | ---: | ---: | ---: | ---: | ---: |",
            ]
        )
        for section in coverage.get("sections", []):
            lines.append(
                f"| `{section['name']}` | {section['decoded_instruction_bytes']} | "
                f"{section['decoded_instruction_coverage_percent']}% | "
                f"{section['candidate_instruction_bytes']} | "
                f"{section['candidate_instruction_coverage_percent']}% | "
                f"{len(section['undecoded_ranges'])} |"
            )
        lines.extend(
            [
                "",
                "### Candidate Boundaries",
                "",
                "| Start RVA | Envelope end | Envelope bytes | Instruction bytes | Confidence | Evidence | Overlap |",
                "| ---: | ---: | ---: | ---: | --- | --- | --- |",
            ]
        )
        for candidate in binary["function_candidates"]:
            lines.append(
                f"| `{format_rva(candidate['rva'])}` | "
                f"`{format_rva(candidate.get('range_end_rva_exclusive'))}` | "
                f"{candidate.get('size', 0)} | {candidate.get('instruction_bytes', 0)} | "
                f"{candidate.get('confidence', 'unknown')} | "
                f"{', '.join(candidate.get('evidence', [])) or 'none'} | "
                f"{'yes' if candidate.get('overlaps_other_candidate_envelope') else 'no'} |"
            )
        lines.extend(
            [
                "",
                "Candidate boundary rows above are mechanically auditable against the "
                "JSON `instruction_ranges_rva` and PE section ranges. Unknown intervals "
                "are retained in JSON under `static_coverage.sections[].undecoded_ranges`.",
            ]
        )
    return "\n".join(lines) + "\n"


def write_coverage_reports(
    payload: dict[str, Any],
    json_path: Path,
    markdown_path: Path,
) -> None:
    json_path.parent.mkdir(parents=True, exist_ok=True)
    markdown_path.parent.mkdir(parents=True, exist_ok=True)
    json_path.write_text(
        json.dumps(payload, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
        newline="\n",
    )
    markdown_path.write_text(
        render_coverage_markdown(payload),
        encoding="utf-8",
        newline="\n",
    )


def main() -> int:
    args = parse_args()
    if args.acceptance_runtime and args.all_runtime:
        raise ValueError("--acceptance-runtime and --all-runtime are mutually exclusive")
    coverage_requested = bool(
        args.coverage_only or args.coverage_json or args.coverage_markdown
    )
    if coverage_requested and not (args.acceptance_runtime or args.all_runtime):
        raise ValueError(
            "coverage mode requires --acceptance-runtime or --all-runtime"
        )
    inventory_path = args.inventory.resolve()
    output_path = args.output.resolve()
    inventory = json.loads(inventory_path.read_text(encoding="utf-8"))
    selected_ids = set(args.sample_ids or [])
    selected_names = {name.casefold() for name in args.binary_names or []}
    binaries: list[dict[str, Any]] = []
    selection_audit: list[dict[str, Any]] = []
    selected_samples = []
    for sample in inventory["samples"]:
        if selected_ids and sample["sample_id"] not in selected_ids:
            continue
        selected_samples.append(sample)
        if args.all_runtime:
            sample_binaries, sample_audit = select_workspace_runtime(sample)
            selection_audit.extend(sample_audit)
        else:
            sample_binaries = [
                {**binary, "selection_class": "explicit-runtime"}
                for binary in sample["binaries"]
            ]
        for binary in sample_binaries:
            if selected_names and binary["name"].casefold() not in selected_names:
                continue
            if binary["scope"] == "patch_or_backup":
                continue
            if args.acceptance_runtime and not is_acceptance_runtime(sample, binary):
                continue
            if args.print_selection:
                binaries.append(
                    {
                        "sample_id": sample["sample_id"],
                        "sample_name": sample["name"],
                        "binary_path": binary["path"],
                        "scope": binary["scope"],
                        "selection_class": binary.get(
                            "selection_class", "explicit-runtime"
                        ),
                        "selection_reason": binary.get(
                            "selection_reason", "explicit runtime selection"
                        ),
                        "runtime_role": "unclassified",
                    }
                )
            else:
                recovered = recover_binary(binary, sample)
                recovered["selection_class"] = binary.get(
                    "selection_class", "explicit-runtime"
                )
                recovered["selection_reason"] = binary.get(
                    "selection_reason", "explicit runtime selection"
                )
                binaries.append(
                    {
                        "sample_id": sample["sample_id"],
                        "sample_name": sample["name"],
                        **recovered,
                    }
                )
    assign_runtime_roles(binaries)

    if args.print_selection:
        for binary in binaries:
            print(
                f"{binary['sample_id']}\t{binary['binary_path']}\t"
                f"{binary.get('runtime_role', 'unclassified')}\t"
                f"{binary.get('selection_class', 'unspecified')}\t"
                f"{binary.get('selection_reason', '')}"
            )
        print(f"Selected {len(binaries)} runtime binaries.", file=sys.stderr)
        return 0

    if coverage_requested:
        coverage_payload = build_coverage_payload(
            inventory_path,
            binaries,
            (
                WORKSPACE / "reports" / "runtime_function_ledger_acceptance.json"
                if args.acceptance_runtime
                else WORKSPACE / "reports" / "runtime_function_ledger.json"
            ),
            args.ghidra_status,
            selection_audit,
        )
        coverage_json_path = (
            args.coverage_json
            or (
                WORKSPACE / "reports" / "runtime_function_coverage.json"
                if args.acceptance_runtime
                else WORKSPACE / "reports" / "runtime_function_coverage_all.json"
            )
        ).resolve()
        coverage_markdown_path = (
            args.coverage_markdown
            or (
                WORKSPACE / "reports" / "runtime_function_coverage.md"
                if args.acceptance_runtime
                else WORKSPACE / "reports" / "runtime_function_coverage_all.md"
            )
        ).resolve()
        write_coverage_reports(
            coverage_payload,
            coverage_json_path,
            coverage_markdown_path,
        )
        print(
            f"Wrote static coverage for {len(binaries)} binaries and "
            f"{coverage_payload['scope']['candidate_count']} candidate functions."
        )
        print(f"Wrote {coverage_json_path}")
        print(f"Wrote {coverage_markdown_path}")
        if args.coverage_only:
            return 0

    function_count = sum(len(binary["functions"]) for binary in binaries)
    call_edge_count = sum(len(binary.get("call_edges", [])) for binary in binaries)
    reachable_count = sum(
        binary.get("summary", {}).get("entrypoint_reachable_count", 0)
        for binary in binaries
    )

    payload = {
        "schema_version": "1.1.0",
        "status": "candidate-recovery",
        "generated_at_utc": inventory["generated_at_utc"],
        "source_inventory": str(inventory_path.relative_to(WORKSPACE)).replace("\\", "/"),
        "scope": {
            "description": (
                "Static x86 candidate functions recovered from executable sections. "
                "Call sites and entrypoint reachability are candidate evidence; "
                "boundaries and semantics require review and runtime validation."
            ),
            "binary_count": len(binaries),
            "function_count": function_count,
            "call_edge_count": call_edge_count,
            "entrypoint_reachable_candidate_count": reachable_count,
            "sample_count": len({binary["sample_id"] for binary in binaries}),
            "acceptance_runtime_only": args.acceptance_runtime,
            "all_runtime": args.all_runtime,
        },
        "binaries": binaries,
        "next_steps": [
            "Review overlapping and tail-call candidates before treating boundaries as functions.",
            "Compare candidates with import/export and string-reference evidence.",
            "Validate behavior before marking a candidate confirmed or implemented.",
        ],
    }
    output_path.parent.mkdir(parents=True, exist_ok=True)
    output_path.write_text(
        json.dumps(payload, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
        newline="\n",
    )
    print(
        f"Recovered {function_count} candidate functions and {call_edge_count} "
        f"call-site edges ({reachable_count} entrypoint-reachable) from "
        f"{len(binaries)} binaries."
    )
    print(f"Wrote {output_path}")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, ValueError, KeyError, json.JSONDecodeError) as exc:
        print(f"recover_functions.py: {exc}", file=sys.stderr)
        raise SystemExit(2)

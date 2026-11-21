"""Audit sample KCS opcode/native-call evidence against the static host routes.

The script reads the three acceptance sample trees and existing JSON ledgers.
It does not launch game executables or write sample files. By default, JSON is
printed to stdout; an output file is accepted only beneath archive/2026-11/.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import struct
import sys
import zlib
from collections import Counter
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import pefile
from Crypto.Cipher import Blowfish


WORKSPACE = Path(__file__).resolve().parents[2]
INVENTORY_PATH = WORKSPACE / "reports" / "sample_inventory.json"
FUNCTION_LEDGER_PATH = WORKSPACE / "reports" / "runtime_function_ledger_acceptance.json"
DESKTOP_SOURCE_PATH = WORKSPACE / "simulator" / "crates" / "desktop" / "src" / "lib.rs"
SCRIPT_SOURCE_PATH = WORKSPACE / "simulator" / "crates" / "script" / "src" / "lib.rs"
ARCHIVE_ROOT = (WORKSPACE / "archive" / "2026-11").resolve()


@dataclass(frozen=True)
class Sample:
    sample_id: str
    name: str
    runtime_root: str
    seed_pe: str


SAMPLES = (
    Sample(
        "cs2_full_v401",
        "cs2_full_v401",
        "games/cs2_full_v401",
        "games/cs2_full_v401/system/cs2_open.exe",
    ),
    Sample(
        "\u725b\u987f\u4e0e\u82f9\u679c\u6811",
        "\u725b\u987f\u4e0e\u82f9\u679c\u6811",
        "games/\u725b\u987f\u4e0e\u82f9\u679c\u6811",
        "games/\u725b\u987f\u4e0e\u82f9\u679c\u6811/cs2.exe",
    ),
    Sample(
        "170526__neko_works__\u30cd\u30b3\u3071\u3089_vol_3_steam\u7248__files",
        "NEKOPARA vol.3",
        "games/[170526][NEKO WORKs] \u30cd\u30b3\u3071\u3089 vol.3 steam\u7248 (files)/nekopara_vol3",
        "games/[170526][NEKO WORKs] \u30cd\u30b3\u3071\u3089 vol.3 steam\u7248 (files)/nekopara_vol3/NEKOPARAvol3.exe",
    ),
)

REFERENCE_TRACE_EVIDENCE = {
    "cs2_full_v401": {
        "indices": [71],
        "basis": "12-frame startup asserts message wait registration and input-wait.",
    },
    "\u725b\u987f\u4e0e\u82f9\u679c\u6811": {
        "indices": [71],
        "basis": "12-frame startup asserts message wait registration.",
    },
    "170526__neko_works__\u30cd\u30b3\u3071\u3089_vol_3_steam\u7248__files": {
        "indices": [21],
        "basis": "2-frame archived startup asserts native-call event index=21.",
    },
}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Audit KCS opcode and native-call evidence for three samples."
    )
    parser.add_argument(
        "--workspace",
        type=Path,
        default=WORKSPACE,
        help="Workspace root; defaults to the root containing this script.",
    )
    parser.add_argument(
        "--output",
        type=Path,
        help="Optional JSON output, restricted to archive/2026-11/.",
    )
    return parser.parse_args()


def read_json(path: Path) -> dict[str, Any]:
    return json.loads(path.read_text(encoding="utf-8"))


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def resource_name(entry: Any) -> str:
    return str(entry.name) if entry.name is not None else str(entry.id)


def named_resource(pe: pefile.PE, type_name: str, item_name: str) -> bytes | None:
    directory = getattr(pe, "DIRECTORY_ENTRY_RESOURCE", None)
    if directory is None:
        return None
    for type_entry in directory.entries:
        if resource_name(type_entry) != type_name or not hasattr(type_entry, "directory"):
            continue
        for item_entry in type_entry.directory.entries:
            if resource_name(item_entry) != item_name or not hasattr(item_entry, "directory"):
                continue
            for language_entry in item_entry.directory.entries:
                if not hasattr(language_entry, "data"):
                    continue
                data = language_entry.data.struct
                return pe.get_data(int(data.OffsetToData), int(data.Size))
    return None


def blowfish_le_decrypt(cipher: Blowfish, data: bytes) -> bytes:
    full_length = len(data) - len(data) % 8
    output = bytearray(data)
    for offset in range(0, full_length, 8):
        block = data[offset : offset + 8]
        be_words = block[0:4][::-1] + block[4:8][::-1]
        decrypted = cipher.decrypt(be_words)
        output[offset : offset + 8] = (
            decrypted[0:4][::-1] + decrypted[4:8][::-1]
        )
    return bytes(output)


def verification_seed(code: bytes) -> int:
    seed = 0xFFFFFFFF
    for byte in code.split(b"\0", 1)[0]:
        seed ^= byte << 24
        for _ in range(8):
            seed = ((seed << 1) & 0xFFFFFFFF) ^ (
                0x04C11DB7 if seed & 0x80000000 else 0
            )
        seed = (~seed) & 0xFFFFFFFF
    return seed


def runtime_fingerprint(path: Path) -> dict[str, Any]:
    data = path.read_bytes()
    pe = pefile.PE(data=data, fast_load=False)
    encrypted_vcode = named_resource(pe, "V_CODE2", "DATA")
    if encrypted_vcode is None:
        encrypted_vcode = named_resource(pe, "V_CODE", "DATA")
    if encrypted_vcode is None or len(encrypted_vcode) % 8:
        raise ValueError(f"{path}: missing or invalid V_CODE resource")
    key = named_resource(pe, "KEY_CODE", "KEY")
    key = bytes(byte ^ 0xCD for byte in key) if key else b"windmill"
    vcode = blowfish_le_decrypt(Blowfish.new(key, Blowfish.MODE_ECB), encrypted_vcode)
    vcode = vcode.rstrip(b"\0")
    machine = int(pe.FILE_HEADER.Machine)
    return {
        "sha256": hashlib.sha256(data).hexdigest(),
        "size": len(data),
        "machine": f"0x{machine:04x}",
        "timestamp": f"0x{int(pe.FILE_HEADER.TimeDateStamp):08x}",
        "image_base": f"0x{int(pe.OPTIONAL_HEADER.ImageBase):08x}",
        "entrypoint_rva": f"0x{int(pe.OPTIONAL_HEADER.AddressOfEntryPoint):08x}",
        "size_of_image": int(pe.OPTIONAL_HEADER.SizeOfImage),
        "checksum": f"0x{int(pe.OPTIONAL_HEADER.CheckSum):08x}",
        "v_code": vcode.decode("ascii", errors="replace"),
        "archive_name_seed": f"0x{verification_seed(vcode):08x}",
    }


class LegacyRandom:
    def __init__(self, seed: int) -> None:
        values = []
        for _ in range(624):
            high = seed & 0xFFFF0000
            seed = (seed * 69069 + 1) & 0xFFFFFFFF
            values.append(high | (seed >> 16))
            seed = (seed * 69069 + 1) & 0xFFFFFFFF
        self.values = values
        self.index = 624

    def next(self) -> int:
        if self.index == 624:
            for index in range(624):
                y = (self.values[index] & 0x80000000) | (
                    self.values[(index + 1) % 624] & 0x7FFFFFFF
                )
                self.values[index] = (
                    self.values[(index + 397) % 624]
                    ^ (y >> 1)
                    ^ (0x9908B0DF if y & 1 else 0)
                )
            self.index = 0
        value = self.values[self.index]
        self.index += 1
        value ^= value >> 11
        value ^= (value << 7) & 0x9D2C5680
        value ^= (value << 15) & 0xEFC60000
        return (value ^ (value >> 18)) & 0xFFFFFFFF


def unobfuscate_name(name: bytes, seed: int) -> bytes:
    forward = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz"
    reverse = b"zyxwvutsrqponmlkjihgfedcbaZYXWVUTSRQPONMLKJIHGFEDCBA"
    result = bytearray(name)
    random = LegacyRandom(seed)
    key = random.next()
    shift = ((key >> 24) + (key >> 16) + (key >> 8) + key) & 0xFF
    for position, byte in enumerate(result):
        if not (65 <= byte <= 90 or 97 <= byte <= 122):
            shift += 1
            continue
        index = 0
        cursor = shift
        while True:
            if reverse[cursor % 52] == byte:
                break
            if reverse[(shift + index + 1) % 52] == byte:
                index += 1
                break
            if reverse[(shift + index + 2) % 52] == byte:
                index += 2
                break
            if reverse[(shift + index + 3) % 52] == byte:
                index += 3
                break
            index += 4
            cursor += 4
            if index >= 52:
                break
        if index < 52:
            result[position] = forward[index]
        shift += 1
    return bytes(result)


def kif_entries(
    path: Path, name_seed: int
) -> tuple[list[dict[str, Any]], str, Blowfish | None]:
    length = path.stat().st_size
    with path.open("rb") as handle:
        header = handle.read(8)
        if len(header) != 8 or header[:4] != b"KIF\0":
            raise ValueError("not a KIF archive")
        count = struct.unpack_from("<I", header, 4)[0]
        if count > 1_000_000:
            raise ValueError(f"entry count {count} exceeds limit")
        table_length = 8 + count * 72
        if table_length > length:
            raise ValueError("KIF directory extends beyond archive")
        table = header + handle.read(table_length - 8)
        if len(table) != table_length:
            raise ValueError("truncated KIF directory")
        entries = []
        for index in range(count):
            offset = 8 + index * 72
            record = table[offset : offset + 72]
            name_end = record[:64].find(b"\0")
            if name_end < 0:
                raise ValueError(f"entry {index} has no NUL-terminated name")
            entries.append(
                {
                    "name": record[:name_end],
                    "offset": struct.unpack_from("<I", record, 64)[0],
                    "size": struct.unpack_from("<I", record, 68)[0],
                }
            )

        encrypted = bool(entries and entries[0]["name"].lower() == b"__key__.dat")
        cipher = None
        if encrypted:
            payload_key = LegacyRandom(entries[0]["size"]).next()
            cipher = Blowfish.new(struct.pack("<I", payload_key), Blowfish.MODE_ECB)
            for index, entry in enumerate(entries[1:], start=1):
                metadata = struct.pack(
                    "<II", (entry["offset"] + index) & 0xFFFFFFFF, entry["size"]
                )
                decoded = blowfish_le_decrypt(cipher, metadata)
                entry["offset"], entry["size"] = struct.unpack("<II", decoded)
                entry["name"] = unobfuscate_name(
                    entry["name"], (name_seed + index) & 0xFFFFFFFF
                )
            entries = entries[1:]

        for entry in entries:
            start = entry["offset"]
            end = start + entry["size"]
            if end > length:
                raise ValueError(f"entry {entry['name']!r} is outside archive")
        return entries, "encrypted" if encrypted else "plain", cipher


def operand_count(opcode: int) -> int | None:
    if opcode in (0x00, 0x2D, 0x33, 0x3A, 0x3B, 0x3C):
        return 0
    if (
        opcode == 0x08
        or opcode == 0x0C
        or 0x16 <= opcode <= 0x1A
        or 0x25 <= opcode <= 0x29
        or 0x2E <= opcode <= 0x2F
        or 0x31 <= opcode <= 0x32
        or 0x36 <= opcode <= 0x39
        or opcode in (0x3D, 0x42, 0x44)
    ):
        return 1
    if (
        0x01 <= opcode <= 0x07
        or 0x09 <= opcode <= 0x0B
        or 0x0D <= opcode <= 0x15
        or 0x1B <= opcode <= 0x24
        or 0x2A <= opcode <= 0x2C
        or opcode in (0x30, 0x34, 0x3E, 0x41, 0x43)
    ):
        return 2
    if opcode == 0x35:
        return 3
    return None


def parse_kcs(data: bytes) -> dict[str, Any]:
    if len(data) < 52 or data[:4] != b"KCS\0":
        raise ValueError("invalid KCS signature or truncated header")
    version, program_version, header_length, code_length = struct.unpack_from(
        "<IIII", data, 4
    )
    init_length = struct.unpack_from("<I", data, 28)[0]
    capacity = struct.unpack_from("<I", data, 48)[0]
    if version != 2 or header_length != 52 or code_length + init_length > capacity:
        raise ValueError("unsupported or inconsistent KCS header")
    body = zlib.decompress(data[52:])
    if len(body) != code_length + init_length:
        raise ValueError("decompressed KCS body length mismatch")
    code = body[:code_length]
    opcodes: Counter[int] = Counter()
    native_calls: Counter[int] = Counter()
    dynamic_native_calls = 0
    cursor = 0
    while cursor < len(code):
        offset = cursor
        opcode = code[cursor]
        cursor += 1
        count = operand_count(opcode)
        if count is None:
            raise ValueError(f"unknown opcode 0x{opcode:02x} at 0x{offset:x}")
        operand_bytes = count * 5
        if cursor + operand_bytes > len(code):
            raise ValueError(f"truncated instruction at 0x{offset:x}")
        operands = [
            code[cursor + index * 5 : cursor + (index + 1) * 5]
            for index in range(count)
        ]
        cursor += operand_bytes
        opcodes[opcode] += 1
        if opcode == 0x2B:
            flags = operands[0][0]
            if flags & 0x50:
                dynamic_native_calls += 1
            else:
                native_calls[struct.unpack_from("<I", operands[0], 1)[0]] += 1
    return {
        "program_version": program_version,
        "instruction_count": sum(opcodes.values()),
        "opcodes": opcodes,
        "native_calls": native_calls,
        "dynamic_native_calls": dynamic_native_calls,
    }


def collect_kcs(root: Path, name_seed: int) -> dict[str, Any]:
    files = sorted(root.rglob("*.kcs"), key=lambda path: str(path).casefold())
    archives = sorted(root.rglob("*.int"), key=lambda path: str(path).casefold())
    totals: Counter[int] = Counter()
    native_calls: Counter[int] = Counter()
    versions: Counter[int] = Counter()
    diagnostics: list[str] = []
    parsed_count = 0
    dynamic_calls = 0
    archive_count = 0
    archive_entry_count = 0
    for path in files:
        try:
            parsed = parse_kcs(path.read_bytes())
            parsed_count += 1
            totals.update(parsed["opcodes"])
            native_calls.update(parsed["native_calls"])
            versions[parsed["program_version"]] += 1
            dynamic_calls += parsed["dynamic_native_calls"]
        except (OSError, ValueError, zlib.error, struct.error) as error:
            diagnostics.append(f"{path.relative_to(root).as_posix()}: {error}")

    for path in archives:
        try:
            entries, _archive_kind, payload_cipher = kif_entries(path, name_seed)
            archive_count += 1
        except (OSError, ValueError, struct.error) as error:
            if path.read_bytes()[:4] == b"KIF\0":
                diagnostics.append(f"{path.relative_to(root).as_posix()}: {error}")
            continue
        with path.open("rb") as handle:
            for entry in entries:
                name = entry["name"].lower()
                if not name.endswith(b".kcs"):
                    continue
                archive_entry_count += 1
                handle.seek(entry["offset"])
                payload = handle.read(entry["size"])
                if len(payload) != entry["size"]:
                    diagnostics.append(
                        f"{path.relative_to(root).as_posix()}::{entry['name']!r}: truncated"
                    )
                    continue
                if payload_cipher is not None:
                    payload = blowfish_le_decrypt(payload_cipher, payload)
                try:
                    parsed = parse_kcs(payload)
                except (ValueError, zlib.error, struct.error) as error:
                    diagnostics.append(
                        f"{path.relative_to(root).as_posix()}::{entry['name']!r}: {error}"
                    )
                    continue
                parsed_count += 1
                totals.update(parsed["opcodes"])
                native_calls.update(parsed["native_calls"])
                versions[parsed["program_version"]] += 1
                dynamic_calls += parsed["dynamic_native_calls"]

    return {
        "loose_kcs_files": len(files),
        "kif_archives": archive_count,
        "kif_kcs_entries": archive_entry_count,
        "parsed_kcs_programs": parsed_count,
        "distinct_opcodes": sorted(totals),
        "opcode_counts": {f"0x{key:02x}": totals[key] for key in sorted(totals)},
        "program_versions": {
            f"0x{key:08x}": versions[key] for key in sorted(versions)
        },
        "static_native_call_indices": sorted(native_calls),
        "native_call_occurrences": {
            str(key): native_calls[key] for key in sorted(native_calls)
        },
        "dynamic_native_call_sites": dynamic_calls,
        "diagnostics": diagnostics,
    }


def parse_implemented_native_indices(source: str) -> list[int]:
    start = source.index("impl Host for LoggingHost<'_>")
    end = source.index("pub struct HeadlessPlayer", start)
    host = source[start:end]
    match_start = host.index("match index {")
    match_end = host.index("\n        }\n    }\n}", match_start)
    arms = host[match_start:match_end]
    indices: set[int] = set()
    for line in arms.splitlines():
        line = line.strip()
        if "=>" not in line:
            continue
        pattern = line.split("=>", 1)[0].strip()
        pattern = pattern.split(" if ", 1)[0].strip()
        if not pattern or pattern == "_":
            continue
        for token in pattern.split("|"):
            token = token.strip()
            range_match = re.fullmatch(r"(\d+)\.\.=([0-9]+)", token)
            if range_match:
                indices.update(
                    range(int(range_match.group(1)), int(range_match.group(2)) + 1)
                )
            elif token.isdecimal():
                indices.add(int(token))
    return sorted(indices)


def reference_opcode_set(source: str) -> list[int]:
    start = source.index("pub const ACTUAL_OPCODES")
    end = source.index("];", start)
    body = source[start:end]
    return sorted(
        {
            int(token, 16)
            for token in re.findall(r"0x([0-9a-fA-F]{2})", body)
        }
    )


def candidate_ledger(ledger: dict[str, Any], sample_id: str) -> list[dict[str, Any]]:
    rows = []
    for binary in ledger["binaries"]:
        if binary["sample_id"] != sample_id:
            continue
        summary = binary.get("summary", {})
        rows.append(
            {
                "path": binary["binary_path"],
                "sha256": binary["sha256"],
                "runtime_role": binary["runtime_role"],
                "candidate_functions": summary.get("candidate_count", 0),
                "entrypoint_reachable_candidates": summary.get(
                    "entrypoint_reachable_count", 0
                ),
                "direct_call_edges": summary.get("direct_call_edge_count", 0),
                "resolved_call_edges": summary.get("resolved_call_edge_count", 0),
                "status": "static-candidate-only",
            }
        )
    return rows


def analyze(workspace: Path) -> dict[str, Any]:
    inventory = read_json(workspace / "reports" / "sample_inventory.json")
    ledger = read_json(workspace / "reports" / "runtime_function_ledger_acceptance.json")
    desktop_source = (workspace / DESKTOP_SOURCE_PATH.relative_to(WORKSPACE)).read_text(
        encoding="utf-8"
    )
    script_source = (workspace / SCRIPT_SOURCE_PATH.relative_to(WORKSPACE)).read_text(
        encoding="utf-8"
    )
    implemented_native = parse_implemented_native_indices(desktop_source)
    reference_opcodes = reference_opcode_set(script_source)
    implemented_set = set(implemented_native)
    reference_set = set(reference_opcodes)
    inventory_samples = {sample["sample_id"]: sample for sample in inventory["samples"]}
    output_samples = []
    union_opcodes: set[int] = set()
    union_native: set[int] = set()

    for spec in SAMPLES:
        inventory_sample = inventory_samples.get(spec.sample_id)
        if inventory_sample is None:
            raise ValueError(f"sample missing from inventory: {spec.sample_id}")
        runtime_root = workspace / spec.runtime_root
        seed_pe_path = workspace / spec.seed_pe
        if not runtime_root.is_dir() or not seed_pe_path.is_file():
            raise FileNotFoundError(f"sample runtime evidence missing: {spec.name}")
        fingerprint = runtime_fingerprint(seed_pe_path)
        recorded_binary = next(
            (
                binary
                for sample in inventory["samples"]
                for binary in sample["binaries"]
                if binary["path"] == spec.seed_pe.replace("\\", "/")
            ),
            None,
        )
        fingerprint["inventory_sha256_match"] = (
            recorded_binary is not None
            and fingerprint["sha256"] == recorded_binary["sha256"]
        )
        if not fingerprint["inventory_sha256_match"]:
            raise ValueError(f"runtime PE hash differs from inventory: {spec.seed_pe}")

        kcs = collect_kcs(runtime_root, int(fingerprint["archive_name_seed"], 16))
        sample_opcodes = set(kcs["distinct_opcodes"])
        sample_native = set(kcs["static_native_call_indices"])
        union_opcodes.update(sample_opcodes)
        union_native.update(sample_native)
        trace = REFERENCE_TRACE_EVIDENCE[spec.sample_id]
        output_samples.append(
            {
                "sample_id": spec.sample_id,
                "name": spec.name,
                "runtime_root": spec.runtime_root,
                "seed_pe_path": spec.seed_pe,
                "fingerprint": fingerprint,
                "function_candidates": candidate_ledger(ledger, spec.sample_id),
                "kcs": kcs,
                "native_route_status": {
                    "sample_present_indices": sorted(sample_native),
                    "implemented_indices": sorted(sample_native & implemented_set),
                    "trace_observed_indices": sorted(set(trace["indices"])),
                    "unmatched_sample_indices": sorted(sample_native - implemented_set),
                    "sample_present_but_not_trace_observed": sorted(
                        sample_native - set(trace["indices"])
                    ),
                    "trace_evidence_basis": trace["basis"],
                },
                "opcode_status": {
                    "sample_distinct_count": len(sample_opcodes),
                    "implemented_by_reference_vm": sorted(sample_opcodes & reference_set),
                    "sample_opcodes_without_reference_vm_path": sorted(
                        sample_opcodes - reference_set
                    ),
                    "reference_opcodes_absent_from_sample": sorted(
                        reference_set - sample_opcodes
                    ),
                },
            }
        )

    return {
        "schema_version": "1.0.0",
        "audit_date": "2026-11-16",
        "scope": {
            "sample_count": len(output_samples),
            "function_ledger_status": "static candidates; no semantic PE recovery claim",
            "runtime_trace_scope": "bounded acceptance startup paths only",
            "complete_game_coverage_claimed": False,
            "static_native_call_denominator_claimed": False,
        },
        "opcode_reference": {
            "constant": "ACTUAL_OPCODES",
            "count": len(reference_opcodes),
            "opcodes": reference_opcodes,
            "sample_union_count": len(union_opcodes),
            "sample_union": sorted(union_opcodes),
            "sample_union_without_reference_vm_path": sorted(
                union_opcodes - reference_set
            ),
        },
        "native_call_routes": {
            "source": "simulator/crates/desktop/src/lib.rs: LoggingHost::call",
            "implemented_static_indices": implemented_native,
            "sample_union_indices": sorted(union_native),
            "sample_union_implemented_indices": sorted(
                union_native & implemented_set
            ),
            "sample_union_unmatched_indices": sorted(
                union_native - implemented_set
            ),
        },
        "samples": output_samples,
    }


def main() -> int:
    args = parse_args()
    workspace = args.workspace.resolve()
    result = analyze(workspace)
    serialized = json.dumps(result, ensure_ascii=False, indent=2) + "\n"
    if args.output:
        output = args.output if args.output.is_absolute() else workspace / args.output
        output = output.resolve()
        try:
            output.relative_to((workspace / "archive" / "2026-11").resolve())
        except ValueError as error:
            raise ValueError("output must be inside archive/2026-11/") from error
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text(serialized, encoding="utf-8", newline="\n")
        print(f"Wrote {output}")
    else:
        sys.stdout.write(serialized)
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, ValueError, KeyError, pefile.PEFormatError) as error:
        print(f"audit_runtime_evidence.py: {error}", file=sys.stderr)
        raise SystemExit(2)

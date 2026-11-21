"""SHA-bound read-only PE evidence and optional source grammar for ordinary1009.

Never executes a recovered function, native entry, VM, game, test or compiler.
"""
from __future__ import annotations

import argparse
import ast
import hashlib
import json
from pathlib import Path
import struct
import subprocess
import tomllib

from recovery_A_20261116 import ROOT, Image
from recovery_A_receiver_contracts_20261116 import block


SOURCES = [
    "simulator/crates/desktop/src/image_export_native.rs",
    "simulator/crates/desktop/src/image_pixels.rs",
    "simulator/crates/desktop/src/data_services.rs",
    "simulator/crates/desktop/src/language_platform.rs",
    "simulator/crates/desktop/src/lib.rs",
]


def syntax() -> None:
    rows = []
    rustfmt = Path.home() / ".cargo/bin/rustfmt.exe"
    for relative in SOURCES:
        raw = (ROOT / relative).read_bytes()
        result = subprocess.run(
            [str(rustfmt), "--edition", "2024", "--emit", "stdout",
             "--config", "skip_children=true"], input=raw,
            stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, check=False,
        )
        rows.append({"source": relative, "sha256": hashlib.sha256(raw).hexdigest(),
                     "exit_code": result.returncode,
                     "diagnostics": result.stderr.decode("utf-8", "replace")})
    for relative in ["simulator/crates/desktop/Cargo.toml", "simulator/Cargo.lock"]:
        raw = (ROOT / relative).read_bytes()
        tomllib.loads(raw.decode("utf-8"))
        rows.append({"source": relative, "sha256": hashlib.sha256(raw).hexdigest(),
                     "exit_code": 0, "grammar": "TOML only"})
    ast.parse(Path(__file__).read_text(encoding="utf-8"))
    output = ROOT / "archive/2026-11/20261117_image_export_E_source_syntax.json"
    output.write_text(json.dumps({
        "status": "grammar-only-not-type-link-or-behavior-validation",
        "compiled": False, "tests_run": False, "engine_functions_executed": False,
        "vm_game_apk_executed": False, "sources": rows,
    }, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"report": output.relative_to(ROOT).as_posix(),
                      "failures": sum(row["exit_code"] != 0 for row in rows)}))
    if any(row["exit_code"] != 0 for row in rows):
        raise SystemExit(1)


def evidence() -> None:
    image = Image(ROOT / "Yukikoi Melt/YukikoiMelt.exe",
                  "63de0e14cd8ce11ad2d68e7ed6076641ff5e5b73d822603cc52764009d812976")
    ranges = [
        ("ordinary1009", 0x197A30, 0x104),
        ("vm_format_literal_fallback", 0x492200, 0x2FE),
        ("wide_path", 0x26C3D0, 0x31E),
        ("wide_startup_path", 0x26C700, 0x217),
        ("startup_folder_fields", 0x2797E1, 0x76),
        ("extension", 0x200560, 0x54),
        ("encode_open_write", 0x1E2940, 0x2E9),
        ("source_copy_does_not_copy_order_flag", 0x1E3530, 0xCF),
        ("source_opaque_black_sar8", 0x1E3BC0, 0x153),
        ("source_rgb_to_bgr", 0x1E5B90, 0x4F),
        ("source_bgr_to_rgb", 0x1E5BE0, 0x4F),
        ("jpeg_quality90", 0x1FB810, 0x1F),
        ("jpeg_writer", 0x1FB870, 0x270),
        ("png_writer", 0x1FC1A0, 0x25E),
        ("png_bgr_transform_flag", 0x445C00, 0x10),
        ("file_open_create_new_then_truncate", 0x12E8A0, 0x128),
        ("file_write_count_or_minus1", 0x12EAD0, 0x79),
        ("special_folder_dispatch", 0x2010B0, 0x180),
    ]
    literals = {}
    for address in [0x50CEC8, 0x50CED8, 0x50CEE8, 0x50CEFC,
                    0x50CF04, 0x50CF0C, 0x50CF14]:
        literals[hex(address)] = image.get(address, 40).split(b"\0", 1)[0].decode("ascii")
    inverse_calls = []
    for section in image.sections:
        if not section["executable"]:
            continue
        raw = section["data"]
        for offset in range(len(raw) - 4):
            if raw[offset] == 0xE8 and section["rva"] + offset + 5 + struct.unpack_from("<i", raw, offset + 1)[0] == 0x1E5B90:
                inverse_calls.append(hex(section["rva"] + offset))
    output = ROOT / "reports/image_export_E_contracts_20261117.json"
    output.write_text(json.dumps({
        "status": "static-instruction-evidence-not-runtime-validation",
        "pe": "Yukikoi Melt/YukikoiMelt.exe", "sha256": image.digest,
        "base": image.base, "engine_functions_executed": False,
        "slot": 1009, "v401_slot_present": False, "path_literals": literals,
        "rgb_to_bgr_direct_calls": inverse_calls,
        "kind": {"1": "BMP", "5": "JPEG, every other extension, quality90", "7": "PNG"},
        "observations": [
            "zero wrapper returns0 without evaluating filename",
            "case-insensitive extension after final dot, stopping at slash/backslash",
            "JPEG swaps source first/third byte once using underlying image+58 flag",
            "BMP/JPEG32bit source becomes24bit via (channel*alpha)>>8 before any encoding/open",
            "shared wrappers retain source mutations even on encode/open/write failure",
            "0x402 opens CREATE_NEW then TRUNCATE_EXISTING, no directory creation",
            "one WriteFile; outer success test is !=0, so short nonzero and DWORD(-1) return1",
            "USERPROFILE marker is APPDATA(0x1a), ProgramData/ALLUSERSPROFILE COMMON_APPDATA(0x23)",
            "wide path helper does not expand LANG or category names; unmarked bytes use signed CBW",
            "native codec output buffers may include8byte allocation padding; portable library bytes need not match",
        ],
        "snippets": {name: block(image, start, size) for name, start, size in ranges},
    }, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"report": output.relative_to(ROOT).as_posix(), "snippets": len(ranges),
                      "engine_execution": False}))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--syntax", action="store_true")
    args = parser.parse_args()
    syntax() if args.syntax else evidence()

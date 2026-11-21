"""Static bytes for the layered image-record producer and its consumers.

Disassembles immutable PE bytes; never loads or invokes original functions.
"""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import subprocess

from native_batch_contracts import analyze
from recovery_A_20261116 import Image, ROOT, instruction_row

RANGES = {
    "records703": (0x5CA010, 0x4B0),
    "selector_parser": (0x6EFFA0, 0xC9),
    "wrapper705": (0x5979D0, 0x90),
    "wrapper706": (0x596E80, 0xC0),
    "wrapper707": (0x59DE60, 0xD0),
    "wrapper708": (0x59E930, 0xD0),
    "wrapper709": (0x59E250, 0x70),
    "entry_constructor": (0x69F8B0, 0x196),
    "entry_attach_hg": (0x69FEA0, 0x46E),
    "entry_release": (0x69FBD0, 0x60),
    "frame_by_ordinal": (0x6A0380, 0x60),
    "select_frame": (0x6A03E0, 0xE2),
    "entry_geometry": (0x6A0EE0, 0x16A),
    "entry_animation_load": (0x6A1050, 0x160),
    "select_frame_reference": (0x6A1E90, 0x68),
    "pixel_geometry_scale": (0x5E4F10, 0x328),
    "pixel_resample": (0x5E5240, 0x420),
    "resample_dispatch": (0x5DD1F0, 0x900),
    "resample_interpolation": (0x5DDA30, 0x730),
    "resample_neighbours": (0x5DCE00, 0x200),
    "wrapper_pixels": (0x4D0DE0, 0x10),
    "memset_zero": (0x600E50, 0x40),
}


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--show", nargs="*")
    parser.add_argument("--wrappers", action="store_true")
    parser.add_argument("--syntax-only", action="store_true")
    args = parser.parse_args()
    if args.syntax_only:
        formatter = Path.home() / ".rustup/toolchains/stable-x86_64-pc-windows-gnu/bin/rustfmt.exe"
        checks = []
        for relative in ["resource_direct_native.rs", "image_manager.rs", "list_native.rs", "image_resample.rs", "lib.rs"]:
            path = ROOT / "simulator/crates/desktop/src" / relative
            raw = path.read_bytes()
            result = subprocess.run([str(formatter), "--edition", "2024", "--config",
                                     "skip_children=true", "--emit", "stdout"],
                                    input=raw, capture_output=True, check=False)
            checks.append({"path": path.relative_to(ROOT).as_posix(),
                           "sha256": hashlib.sha256(raw).hexdigest(),
                           "parse": result.returncode == 0,
                           "unchanged": raw == path.read_bytes(),
                           "diagnostic": result.stderr.decode("utf8", errors="replace")})
        output = ROOT / "archive/2026-11/20261117_format_B_layer_records_syntax.json"
        output.write_text(json.dumps({"compiler_executed": False, "type_checked": False,
                                      "files": checks}, indent=2) + "\n", encoding="utf8")
        print(json.dumps(checks, indent=2))
        return
    summary = json.loads((ROOT / "reports/recovery_A_20261116_summary.json").read_text(encoding="utf8"))
    row = next(row for row in summary["perPE"] if row["binary_path"] == "Yukikoi Melt/YukikoiMelt.exe")
    image = Image(ROOT / row["binary_path"], row["sha256"])
    routines = {
        name: [instruction_row(item, image.base)
               for item in image.md.disasm(image.get(va - image.base, size), va)]
        for name, (va, size) in RANGES.items()
    }
    output = ROOT / "archive/2026-11/20261117_format_B_layer_records_contracts.json"
    wrappers = analyze([703, 704, 705, 706, 707, 708, 709], 180)
    output.write_text(json.dumps({
        "original_code_executed": False, "path": row["binary_path"], "sha256": row["sha256"],
        "routines": routines, "wrappers": wrappers,
        "strings": {hex(va): image.get(va - image.base, 32).hex() for va in
                    (0x8F06C8, 0x8F06D0, 0x8E8584, 0x8F07D0, 0x8EDF7C)},
    }, indent=2) + "\n", encoding="utf8")
    if args.show is not None:
        for name in args.show:
            print(name)
            for item in routines[name]:
                print(f'{item["rva"] + image.base:08x} {item["mnemonic"]:8s} {item["operands"]}')
    else:
        print(str(output.relative_to(ROOT)))


if __name__ == "__main__":
    main()

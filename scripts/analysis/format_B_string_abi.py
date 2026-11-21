"""Read only fixed PE string-wrapper bytes; never invokes target code."""
from __future__ import annotations

import argparse
import json
from pathlib import Path
import struct

from recovery_A_20261116 import Image, instruction_row

ROOT = Path(__file__).resolve().parents[2]
PROFILES = {
    "yukikoi": ("Yukikoi Melt/YukikoiMelt.exe",
                "63de0e14cd8ce11ad2d68e7ed6076641ff5e5b73d822603cc52764009d812976",
                0x6270E8),
    "v401": ("cs2_full_v401/system/cs2_open.exe",
             "0e1dde51e86e9152d8e1d27b82efb15916cb9f82d23dd30990212fafd96c8b9f",
             7588096),
}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--profile", choices=PROFILES, default="yukikoi")
    parser.add_argument("--slots", nargs="*", type=int, default=[])
    parser.add_argument("--range", action="append", default=[], metavar="VA:LENGTH")
    args = parser.parse_args()
    path, expected, table = PROFILES[args.profile]
    image = Image(ROOT / path, expected)
    ranges = []
    for slot in args.slots:
        if not 0 <= slot < 1100:
            parser.error("slot outside fixed ordinary table")
        va, = struct.unpack("<I", image.get(table + slot * 4, 4))
        ranges.append((f"ordinary{slot}", va, 0x240, True))
    for value in args.range:
        va, length = (int(part, 0) for part in value.split(":"))
        if not 0 < length <= 0x1800:
            parser.error("range must be bounded")
        ranges.append((f"range_{va:x}_{length:x}", va, length, False))
    routines = []
    for name, va, length, stop_return in ranges:
        rows = []
        for insn in image.md.disasm(image.get(va - image.base, length), va):
            rows.append(instruction_row(insn, image.base))
            if stop_return and insn.mnemonic.startswith("ret"):
                break
        routines.append({"name": name, "va": va, "length": length,
                         "stopped_at_first_linear_return": stop_return,
                         "instructions": rows})
    report = {"schema": "cs2-string-abi-static/v1", "path": path,
              "sha256": expected, "table_rva": table, "routines": routines,
              "original_code_executed": False}
    suffix = "_".join(str(slot) for slot in args.slots) or "helpers"
    output = ROOT / f"archive/2026-11/20261117_format_B_string_abi_{args.profile}_{suffix}.json"
    if output.exists() and not args.slots:
        old = json.loads(output.read_text(encoding="utf-8"))
        by_name = {row["name"]: row for row in old["routines"]}
        by_name.update({row["name"]: row for row in routines})
        report["routines"] = list(by_name.values())
    output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    for routine in routines:
        print(routine["name"], hex(routine["va"]))
        for row in routine["instructions"]:
            print(f"{row['rva'] + image.base:08x} {row['mnemonic']:8} {row['operands']}")
    print(output.relative_to(ROOT).as_posix())


if __name__ == "__main__":
    main()

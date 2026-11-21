"""Disassemble a bounded byte range from an independently hashed PE.

This is navigation evidence, not a function-boundary or execution claim.
"""
import argparse
import hashlib
import json
from pathlib import Path
import re

import capstone
import pefile

ROOT = Path(__file__).resolve().parents[2]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sha-prefix", required=True)
    parser.add_argument("--rva", type=lambda s: int(s, 0), action="append", required=True)
    parser.add_argument("--size", type=lambda s: int(s, 0), default=256)
    parser.add_argument("--name", required=True)
    args = parser.parse_args()
    if not re.fullmatch(r"[a-zA-Z0-9_-]+", args.name) or not 1 <= args.size <= 65536:
        parser.error("invalid output label or slice size")
    rows = json.loads((ROOT / "reports/recovery_A_20261116_summary.json").read_text(encoding="utf-8"))["perPE"]
    selected = [row for row in rows if row["sha256"].startswith(args.sha_prefix)]
    if len(selected) != 1:
        parser.error("SHA prefix must select exactly one fixed image")
    row = selected[0]
    data = (ROOT / row["binary_path"]).read_bytes()
    if hashlib.sha256(data).hexdigest() != row["sha256"]:
        raise ValueError("PE content identity changed")
    pe = pefile.PE(data=data, fast_load=True)
    base = pe.OPTIONAL_HEADER.ImageBase
    decoder = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_32)
    slices = []
    for rva in args.rva:
        raw = pe.get_data(rva, args.size)
        instructions = [{"rva": i.address-base, "bytes": i.bytes.hex(),
                         "op": i.mnemonic, "args": i.op_str}
                        for i in decoder.disasm(raw, base+rva)]
        slices.append({"rva": rva, "bytes": raw.hex(), "instructions": instructions})
        print("RVA", hex(rva))
        for i in instructions:
            print(f"{i['rva']:08x} {i['op']:8s} {i['args']}")
    output = ROOT / "archive/2026-11" / f"20261117_C_{args.name}.json"
    output.write_text(json.dumps({"sha256": row["sha256"], "original_code_executed": False,
                                 "slices": slices}, indent=2)+"\n", encoding="utf-8")


if __name__ == "__main__":
    main()

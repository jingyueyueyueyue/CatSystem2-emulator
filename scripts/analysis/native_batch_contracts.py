"""Read-only, bounded PE evidence for high-frequency ordinary-native gaps.

Reuses the byte-verified registration scanner. It never executes a game or
decodes a whole giant function ledger. Evidence remains tied to PE SHA256/RVA.
"""
from __future__ import annotations

import argparse
import json
import re
from datetime import datetime, timezone
from pathlib import Path

from recovery_A_20261116 import (
    Image, ROOT, classify_tables, instruction_row, registrations,
)


def analyze(slots: list[int], instruction_limit: int) -> dict:
    summary = json.loads((ROOT / "reports/recovery_A_20261116_summary.json").read_text(encoding="utf-8"))
    payload = {
        "schema": "cs2-native-batch-contracts/v1",
        "generated_at": datetime.now(timezone.utc).isoformat(),
        "status": "bounded-static-instructions-not-semantic-validation",
        "slots": slots, "instruction_limit": instruction_limit, "binaries": [],
        "original_code_executed": False,
    }
    selected = [row for row in summary["perPE"]
                if row["binary_path"] in {
                    "Yukikoi Melt/YukikoiMelt.exe",
                    "cs2_full_v401/system/cs2_open.exe",
                }]
    for row in selected:
        image = Image(ROOT / row["binary_path"], row["sha256"])
        _, observed = registrations(image)
        tables = [table for table in classify_tables(image, observed)
                  if table["table_hint"] == "ordinary"]
        entries = []
        for table in tables:
            for slot in slots:
                if slot >= len(table["slots"]):
                    continue
                entry = table["slots"][slot]
                rva = entry["entry_rva"] if isinstance(entry, dict) else entry
                instructions = []
                for insn in image.md.disasm(image.get(rva, instruction_limit * 15),
                                           image.base + rva, count=instruction_limit):
                    instructions.append(insn)
                    if insn.mnemonic.startswith("ret"):
                        break
                # Stop on the first linear return. Alternate basic blocks and
                # helper semantics remain separate evidence obligations.
                calls = sorted({target for insn in instructions
                                if insn.mnemonic == "call"
                                and (target := image.direct(insn)) is not None})
                branch_tails = []
                prefix_end = instructions[-1].address - image.base + instructions[-1].size if instructions else rva
                for insn in instructions:
                    target = image.direct(insn)
                    if insn.mnemonic.startswith("j") and target is not None and prefix_end <= target < prefix_end + 128:
                        tail = []
                        for item in image.md.disasm(image.get(target, 96), image.base + target, count=24):
                            tail.append(instruction_row(item, image.base))
                            if item.mnemonic.startswith("ret"):
                                break
                        branch_tails.append({"rva": target, "instructions": tail})
                entries.append({
                    "slot": slot, "table_rva": table["table_rva"], "entry_rva": rva,
                    "instructions": [instruction_row(insn, image.base) for insn in instructions],
                    "direct_calls": calls,
                    "branch_tails": branch_tails,
                    "first_linear_return_seen": bool(instructions and instructions[-1].mnemonic.startswith("ret")),
                })
        payload["binaries"].append({
            "path": row["binary_path"], "sha256": row["sha256"],
            "file_version": row["file_version"], "entries": entries,
        })
    return payload


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--slots", nargs="+", type=int, default=[29, 30, 31, 32, 49, 364, 367, 435, 685])
    parser.add_argument("--instructions", type=int, default=160)
    parser.add_argument("--name", default="contracts",
                        help="Stable report group name, e.g. image-list or scalar.")
    args = parser.parse_args()
    if not 1 <= args.instructions <= 1024 or any(not 0 <= slot < 1400 for slot in args.slots):
        parser.error("slots/instruction budget out of bounds")
    if not re.fullmatch(r"[a-z0-9][a-z0-9_-]{0,47}", args.name):
        parser.error("report group must be lowercase ASCII letters, digits, underscore or hyphen")
    report = analyze(sorted(set(args.slots)), args.instructions)
    output = ROOT / "reports" / f"native_batch_{args.name}_20261117.json"
    output.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({
        "report": output.relative_to(ROOT).as_posix(),
        "binaries": len(report["binaries"]),
        "entries": sum(len(row["entries"]) for row in report["binaries"]),
        "execution": False,
    }, ensure_ascii=True))


if __name__ == "__main__":
    main()

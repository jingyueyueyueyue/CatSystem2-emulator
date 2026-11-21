"""Hash-pinned read-only call sites and bounded helper instruction context."""
from __future__ import annotations
import argparse
import json
import re
import struct
from recovery_A_20261116 import Image, ROOT, instruction_row

def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--pe", required=True, help="Workspace-relative PE path")
    parser.add_argument("--sha256", required=True)
    parser.add_argument("--callee", type=lambda value: int(value, 0), nargs="+", required=True)
    parser.add_argument("--name", required=True)
    parser.add_argument("--radius", type=int, default=96)
    args = parser.parse_args()
    if not re.fullmatch(r"[a-z0-9][a-z0-9_-]{0,47}", args.name) or not 16 <= args.radius <= 4096:
        parser.error("invalid report group/radius")
    path = (ROOT / args.pe).resolve()
    if not path.is_relative_to(ROOT.resolve()):
        parser.error("PE path escapes workspace")
    image = Image(path, args.sha256)
    sites = []
    targets = set(args.callee)
    for section in image.sections:
        if not section["executable"]:
            continue
        code = section["data"]
        for found in re.finditer(b"\xe8", code):
            offset = found.start()
            if offset + 5 > len(code):
                continue
            rva = section["rva"] + offset
            target = image.base + rva + 5 + struct.unpack_from("<i", code, offset + 1)[0]
            if target not in targets:
                continue
            # Raw E8 bytes can occur inside an operand. Keep that caveat,
            # plus the shortest nearby suffix that decodes through this call.
            prefix = []
            for distance in range(16, args.radius + 1):
                start = max(section["rva"], rva - distance)
                decoded = image.insns(start, rva + 5 - start)
                if decoded and decoded[-1].address == image.base + rva \
                    and decoded[-1].mnemonic == "call":
                    prefix = decoded
                    break
            after = image.insns(rva + 5, args.radius)
            sites.append({
                "callee_va": target, "call_rva": rva,
                "candidate_status": "static-relative-call-byte-match-not-function-boundary-proof",
                "decoded_suffix_ends_at_call": bool(prefix),
                "before": [instruction_row(ins, image.base) for ins in prefix],
                "after": [instruction_row(ins, image.base) for ins in after],
            })
    report = {
        "pe": path.relative_to(ROOT).as_posix(), "sha256": image.digest,
        "image_base": image.base,
        "status": "read-only-call-context-not-engine-execution-or-semantic-validation",
        "callees": [{"va": address, "instructions": [
            instruction_row(ins, image.base) for ins in image.insns(address - image.base, args.radius * 2)
        ]} for address in sorted(targets)],
        "sites": sites, "original_code_executed": False,
    }
    output = ROOT / "archive/2026-11" / f"20261117_pe_calls_{args.name}.json"
    output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({
        "report": output.relative_to(ROOT).as_posix(), "sites": [
            {"callee": hex(site["callee_va"]), "call": hex(site["call_rva"] + image.base),
             "decoded_suffix": site["decoded_suffix_ends_at_call"]} for site in sites
        ], "original_code_executed": False,
    }, indent=2))

if __name__ == "__main__":
    main()

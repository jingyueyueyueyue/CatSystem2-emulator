"""Recover each original703 body/selector imports without invoking target code."""
from __future__ import annotations
import argparse
import hashlib
import json
import re
from pathlib import Path
from recovery_A_20261116 import Image, ROOT, instruction_row, registrations, classify_tables

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--show", type=int)
    args = parser.parse_args()
    output = ROOT / "archive/2026-11/20261117_format_B_layer_profiles.json"
    if args.show is not None and output.exists():
        record = json.loads(output.read_text(encoding="utf8"))["profiles"][args.show]
        print(record["path"], record["sha256"], record["entry_va"])
        for row in record.get("helper_instructions", record["instructions"]):
            print(f'{row["rva"]+0x400000:08x} {row["mnemonic"]:8s} {row["operands"]}')
        print(json.dumps(record["references"], indent=2)); return
    summary = json.loads((ROOT / "reports/recovery_A_20261116_summary.json").read_text(encoding="utf8"))
    rows = []
    for row in summary["perPE"]:
        if Path(row["binary_path"]).suffix.lower() != ".exe": continue
        image = Image(ROOT / row["binary_path"], row["sha256"])
        _, observed = registrations(image)
        tables = [t for t in classify_tables(image, observed) if t["table_hint"] == "ordinary" and len(t["slots"])>704]
        for table in tables:
            slot = table["slots"][703]
            rva = slot["entry_rva"] if isinstance(slot, dict) else slot
            instructions = []
            for instruction in image.md.disasm(image.get(rva, 0x1000), image.base+rva):
                instructions.append(instruction)
                if instruction.mnemonic.startswith("ret"): break
            refs = {}
            imports = {}
            helpers = [i for index,i in enumerate(instructions[:-1]) if i.mnemonic == "call"
                       and i.op_str.startswith("0x") and instructions[index+1].mnemonic == "mov"
                       and instructions[index+1].op_str == "dword ptr [ebx], eax"]
            helper = int(helpers[-1].op_str,16) if helpers else None
            body = []
            if helper:
                for instruction in image.md.disasm(image.get(helper-image.base, 0x1000), helper):
                    body.append(instruction)
                    if instruction.mnemonic.startswith("ret"): break
            for instruction in instructions + body:
                for value in re.findall(r"0x[0-9a-f]+", instruction.op_str):
                    va = int(value, 16)
                    if va-image.base in image.imports: imports[value] = image.imports[va-image.base]
                    section = image.section(va-image.base)
                    if section and not section["executable"]:
                        raw = image.get(va-image.base, 32)
                        refs[value] = raw.hex()
            calls = [int(i.op_str,16) for i in body if i.mnemonic == "call" and i.op_str.startswith("0x")]
            selector = calls[4 if imports else 2]
            selector_instructions = list(image.md.disasm(image.get(selector-image.base, 0xd0), selector))
            addresses = {i.address: index for index,i in enumerate(body)}
            def normalized(instruction):
                def address(match):
                    value = int(match.group(),16)
                    if value in addresses and instruction.mnemonic.startswith("j"): return "branch"+str(addresses[value])
                    return "address" if value >= image.base else match.group()
                return instruction.mnemonic + " " + re.sub(r"0x[0-9a-f]+", address, instruction.op_str)
            shape = "\n".join(normalized(i) for i in body)
            rows.append({"path": row["binary_path"], "sha256": row["sha256"], "entry_va": hex(rva+image.base),
                "body_sha256": hashlib.sha256(b"".join(bytes(i.bytes) for i in instructions)).hexdigest(),
                "instructions": [instruction_row(i, image.base) for i in instructions],
                "helper_va": hex(helper) if helper else None,
                "helper_sha256": hashlib.sha256(b"".join(bytes(i.bytes) for i in body)).hexdigest(),
                "helper_instructions": [instruction_row(i, image.base) for i in body],
                "normalized_body_sha256": hashlib.sha256(shape.encode("ascii")).hexdigest(),
                "selector_va": hex(selector),
                "selector_instructions": [instruction_row(i,image.base) for i in selector_instructions],
                "imports": imports, "references": refs})
    output.write_text(json.dumps({"target_executed": False, "profiles": rows}, indent=2)+"\n", encoding="utf8")
    print(json.dumps([{ "index": i, "path": r["path"], "sha256": r["sha256"], "entry": r["entry_va"],
        "helper": r["helper_va"], "selector": r["selector_va"], "shape": r["normalized_body_sha256"],
        "count": len(r["helper_instructions"])} for i,r in enumerate(rows)], indent=2))

if __name__ == "__main__": main()

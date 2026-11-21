"""Bounded static factory/688/689 receiver navigation; never executes PE code."""
from pathlib import Path
import hashlib
import json
import struct
import capstone
import pefile

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "archive/2026-11/20261117_C_object_graph_receivers.json"

def analyze(row):
    path = ROOT / row["binary_path"]
    data = path.read_bytes()
    digest = hashlib.sha256(data).hexdigest()
    if digest != row["sha256"]:
        raise ValueError("fixed PE changed")
    pe = pefile.PE(data=data, fast_load=True)
    base = pe.OPTIONAL_HEADER.ImageBase
    md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_32)
    md.detail = True
    def ins(address, size=96):
        return [{"rva": i.address-base, "bytes": i.bytes.hex(), "mnemonic": i.mnemonic,
                 "operands": i.op_str} for i in md.disasm(pe.get_data(address-base, size), address)]
    factories = []
    command_hits = []
    for section in pe.sections:
        if not section.Characteristics & 0x20000000:
            continue
        code = section.get_data()
        cursor = 0
        while (cursor := code.find(b"\xff\x24", cursor)) >= 0:
            if cursor+7 > len(code) or code[cursor+2] & 0xc7 != 0x85:
                cursor += 2
                continue
            va = base + section.VirtualAddress + cursor
            jump = next(md.disasm(code[cursor:cursor+7], va, count=1), None)
            if not jump or jump.operands[0].type != capstone.x86.X86_OP_MEM:
                cursor += 2
                continue
            index = jump.operands[0].mem.index
            prefix = None
            for distance in range(7, 30):
                start = max(0, cursor-distance)
                seq = list(md.disasm(code[start:cursor+7], base+section.VirtualAddress+start))
                compares = [i for i in seq[:-1] if i.mnemonic == "cmp" and len(i.operands) == 2
                    and i.operands[0].type == capstone.x86.X86_OP_REG and i.operands[0].reg == index
                    and i.operands[1].type == capstone.x86.X86_OP_IMM and 43 <= i.operands[1].imm <= 80]
                if seq and seq[-1].address == va and compares and any(i.mnemonic == "ja" for i in seq):
                    prefix = seq
                    limit = compares[-1].operands[1].imm
                    break
            if prefix:
                table = jump.operands[0].mem.disp
                try:
                    pointers = struct.unpack(f"<{limit+1}I", pe.get_data(table-base, (limit+1)*4))
                except (struct.error, pefile.PEFormatError):
                    cursor += 2
                    continue
                if all(any(s.Characteristics & 0x20000000 and s.VirtualAddress <= p-base < s.VirtualAddress+s.SizeOfRawData
                           for s in pe.sections) for p in pointers):
                    factories.append({"jump_rva": va-base, "table_rva": table-base, "last_kind": limit,
                        "prefix": ins(prefix[0].address, va+7-prefix[0].address),
                        "kind0": ins(pointers[0]), "kind34": ins(pointers[34]),
                        "kind43": ins(pointers[43]), "candidate_only": True})
            cursor += 2
        for command in (426, 435, 437, 445, 446, 447):
            cursor = 0
            needle = struct.pack("<I", command)
            while (cursor := code.find(needle, cursor)) >= 0:
                # Only report instructions which actually end at the literal,
                # rather than disassembling arbitrary bytes as instructions.
                for distance in range(1, 7):
                    start = cursor-distance
                    if start < 0:
                        continue
                    item = next(md.disasm(code[start:cursor+4], base+section.VirtualAddress+start, count=1), None)
                    if item and item.size == distance+4 and item.mnemonic in {"cmp", "sub", "add", "push", "mov"}:
                        command_hits.append({"command": command, "instruction": ins(item.address, item.size)[0]})
                cursor += 4
    return {"path": row["binary_path"], "sha256": digest, "factories": factories, "command_hits": command_hits}

def main():
    summary = json.loads((ROOT/"reports/recovery_A_20261116_summary.json").read_text(encoding="utf-8"))
    images = [analyze(row) for row in summary["perPE"] if row["binary_path"].lower().endswith(".exe")]
    OUT.write_text(json.dumps({"schema": "cs2-state-receiver-navigation/v1", "images": images,
        "original_code_executed": False}, ensure_ascii=False, indent=2)+"\n", encoding="utf-8")
    print(json.dumps({"evidence": OUT.relative_to(ROOT).as_posix(), "images": [
        {"sha256": image["sha256"], "factories": [{"table": hex(f["table_rva"]),
        "43": f["kind43"][:10]} for f in image["factories"]],
        "445_446_nonpush": [h for h in image["command_hits"] if h["command"] in (445,446)
            and h["instruction"]["mnemonic"] != "push"]} for image in images]}, ensure_ascii=True))

if __name__ == "__main__":
    main()

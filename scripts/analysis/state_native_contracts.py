"""Bounded read-only evidence for state-tree and export task protocols.

Candidates are immediate-byte matches, not verified function boundaries.
No original code, VM, compiler or acceptance test is executed.
"""
from __future__ import annotations
import hashlib
import json
from pathlib import Path
import struct
import capstone
import pefile

ROOT = Path(__file__).resolve().parents[2]
BINARY = ROOT / "Yukikoi Melt/YukikoiMelt.exe"
EXPECTED = "63de0e14cd8ce11ad2d68e7ed6076641ff5e5b73d822603cc52764009d812976"
HELPERS = {
    "tree_constructor": (0x6838E0, 0x50),
    "tree_destructor": (0x683930, 0x100),
    "tree_read": (0x683A90, 0xC0),
    "tree_write": (0x683B50, 0x180),
    "tree_insert": (0x683980, 0x80),
}

def main() -> None:
    data = BINARY.read_bytes()
    digest = hashlib.sha256(data).hexdigest()
    if digest != EXPECTED:
        raise ValueError("state protocol source hash changed")
    image = pefile.PE(data=data, fast_load=True)
    base = image.OPTIONAL_HEADER.ImageBase
    decoder = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_32)
    decoder.detail = True
    def decode(address: int, size: int) -> list[dict]:
        return [
            {"va": instruction.address, "bytes": instruction.bytes.hex(),
             "mnemonic": instruction.mnemonic, "operands": instruction.op_str}
            for instruction in decoder.disasm(image.get_data(address - base, size), address)
        ]
    candidates = []
    calls = []
    switches = []
    for section in image.sections:
        if not section.Characteristics & 0x20000000:
            continue
        code = section.get_data()
        cursor = 0
        while (cursor := code.find(b"\xff\x24", cursor)) >= 0:
            if cursor + 7 <= len(code) and code[cursor + 2] & 0xc7 == 0x85:
                address = base + section.VirtualAddress + cursor
                table = struct.unpack_from("<I", code, cursor + 3)[0]
                if table >= base:
                    prefix = None
                    for distance in range(7, 65):
                        start = max(0, cursor - distance)
                        sequence = list(decoder.disasm(code[start:cursor + 7],
                            base + section.VirtualAddress + start))
                        if not sequence or sequence[-1].address != address:
                            continue
                        bounds = [
                            int(operand.imm) for ins in sequence if ins.mnemonic == "cmp"
                            for operand in ins.operands if operand.type == capstone.x86.X86_OP_IMM
                            and 445 <= operand.imm <= 4096
                        ]
                        if bounds:
                            prefix = (sequence, max(bounds))
                            break
                    if prefix:
                        try:
                            target = struct.unpack("<I", image.get_data(table - base + 445 * 4, 4))[0]
                        except (struct.error, pefile.PEFormatError):
                            target = 0
                        if base <= target < base + image.OPTIONAL_HEADER.SizeOfImage:
                            switches.append({
                                "jump_va": address, "table_va": table, "max_index_candidate": prefix[1],
                                "unadjusted_index445_target_va": target,
                                "warning": "Index445 may not be command445: prove the index normalization first.",
                                "prefix": decode(prefix[0][0].address, address + 7 - prefix[0][0].address),
                                "case_instructions": decode(target, 512),
                            })
            cursor += 2
        cursor = 0
        while (cursor := code.find(b"\xe8", cursor)) >= 0:
            if cursor + 5 <= len(code):
                va = base + section.VirtualAddress + cursor
                target = va + 5 + struct.unpack_from("<i", code, cursor + 1)[0]
                selected = next((name for name, (address, _) in HELPERS.items() if address == target), None)
                if selected:
                    boundary = code.rfind(b"\xcc\xcc", max(0, cursor - 8192), cursor)
                    begin = boundary + 2 if boundary >= 0 else max(0, cursor - 160)
                    while begin < cursor and code[begin] == 0xcc:
                        begin += 1
                    start = base + section.VirtualAddress + begin
                    calls.append({
                        "helper": selected, "call_va": va, "candidate_start_va": start,
                        "instructions": decode(start, min(cursor - begin + 512, 8192)),
                    })
            cursor += 1
        numbers = [*range(400, 449), *range(510, 518)]
        for number in numbers + [-value for value in numbers]:
            pattern = struct.pack("<i", number)
            cursor = 0
            while (cursor := code.find(pattern, cursor)) >= 0:
                start = max(0, cursor - 7)
                hits = []
                for offset in range(start, cursor):
                    instructions = list(decoder.disasm(
                        code[offset:cursor + 4], base + section.VirtualAddress + offset, count=1))
                    if not instructions:
                        continue
                    ins = instructions[0]
                    if len(ins.operands) != 2 or ins.operands[0].type != capstone.x86.X86_OP_REG:
                        continue
                    if ins.reg_name(ins.operands[0].reg) in {"esp", "ebp"}:
                        continue
                    operand = ins.operands[1]
                    matches = (
                        operand.type == capstone.x86.X86_OP_IMM and operand.imm == number
                        or operand.type == capstone.x86.X86_OP_MEM and operand.mem.disp == number
                        and ins.reg_name(operand.mem.base) not in {"esp", "ebp"}
                    )
                    if ins.mnemonic in {"cmp", "sub", "add", "lea"} and matches and ins.address + ins.size == base + section.VirtualAddress + cursor + 4:
                        hits.append(offset)
                if hits:
                    offset = min(hits)
                    boundary = code.rfind(b"\xcc\xcc", max(0, offset - 4096), offset)
                    begin = boundary + 2 if boundary >= 0 else max(0, offset - 128)
                    while begin < offset and code[begin] == 0xcc:
                        begin += 1
                    address = base + section.VirtualAddress + begin
                    candidates.append({
                        "command": number, "cmp_va": base + section.VirtualAddress + offset,
                        "candidate_start_va": address,
                        "matched_instruction": decode(base + section.VirtualAddress + offset, cursor + 4 - offset),
                        "instructions": decode(address, min(offset - begin + 1024, 8192)),
                    })
                cursor += 4
    report = {
        "status": "static-byte-candidates-not-function-or-behavior-proof",
        "binary": BINARY.relative_to(ROOT).as_posix(), "sha256": digest,
        "candidates": candidates,
        "helper_callers": calls,
        "large_switch_candidates": switches,
        "helpers": {name: {"va": address, "instructions": decode(address, size)}
                    for name, (address, size) in HELPERS.items()},
        "executed_original_code": False,
    }
    output = ROOT / "archive/2026-11/20261117_state_native_contracts.json"
    output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({
        "report": output.relative_to(ROOT).as_posix(),
        "candidate_count": len(candidates),
        "large_switch_count": len(switches),
        "helpers": list(HELPERS), "executed_original_code": False,
        "helper_callers": [{"helper": row["helper"], "call_va": hex(row["call_va"]),
                            "start": hex(row["candidate_start_va"])} for row in calls],
    }, indent=2))

if __name__ == "__main__":
    main()

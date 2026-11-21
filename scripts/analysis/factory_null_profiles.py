"""Read runtime task-factory switches and prove direct null-return cases.

No original code executes. A profile is emitted only for the independently
identified V401 factory; other PE switches remain candidates for review.
"""
from __future__ import annotations
import hashlib
import json
from pathlib import Path
import struct
import capstone
import pefile

ROOT = Path(__file__).resolve().parents[2]
V401_SHA = "0e1dde51e86e9152d8e1d27b82efb15916cb9f82d23dd30990212fafd96c8b9f"
YUKIKOI_SHA = "63de0e14cd8ce11ad2d68e7ed6076641ff5e5b73d822603cc52764009d812976"
NEWTON_SHAS = {
    "3b359fc2d737d5ceed454d6b65e6e5b5e6dfb4a24cf88ee361df86e454203f18",
    "6f75ae3877e018de1982a68f1089c127870d895d748c6290dd2ee85cd8daa4de",
}

def main() -> None:
    summary = json.loads((ROOT / "reports/recovery_A_20261116_summary.json").read_text(encoding="utf-8"))
    selected = [row for row in summary["perPE"] if row["binary_path"].lower().endswith(".exe")
                and any(name in row["binary_path"] for name in [
                    "cs2_full_v401/system/cs2_open.exe", "Yukikoi Melt/YukikoiMelt.exe",
                    "牛顿与苹果树", "ネコぱら",
                ])]
    images = []
    for row in selected:
        path = ROOT / row["binary_path"]
        data = path.read_bytes()
        digest = hashlib.sha256(data).hexdigest()
        if digest != row["sha256"]:
            raise ValueError("factory PE source changed")
        pe = pefile.PE(data=data, fast_load=True)
        if pe.OPTIONAL_HEADER.Magic != 0x10b:
            continue
        base = pe.OPTIONAL_HEADER.ImageBase
        decoder = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_32)
        decoder.detail = True
        def instructions(address: int, size: int) -> list:
            return list(decoder.disasm(pe.get_data(address - base, size), address))
        def encode(items: list) -> list[dict]:
            return [{"rva": item.address - base, "bytes": item.bytes.hex(),
                     "mnemonic": item.mnemonic, "operands": item.op_str} for item in items]
        candidates = []
        for section in pe.sections:
            if not section.Characteristics & 0x20000000:
                continue
            code = section.get_data()
            cursor = 0
            while (cursor := code.find(b"\xff\x24", cursor)) >= 0:
                if cursor + 7 > len(code) or code[cursor + 2] & 0xc7 != 0x85:
                    cursor += 2
                    continue
                va = base + section.VirtualAddress + cursor
                jump = next(decoder.disasm(code[cursor:cursor + 7], va, count=1), None)
                prefix = None
                if jump and jump.operands[0].type == capstone.x86.X86_OP_MEM:
                    index = jump.operands[0].mem.index
                    for distance in range(7, 30):
                        start = max(0, cursor - distance)
                        seq = list(decoder.disasm(code[start:cursor + 7],
                            base + section.VirtualAddress + start))
                        if seq and seq[-1].address == va and any(
                            item.mnemonic == "cmp" and len(item.operands) == 2
                            and item.operands[0].type == capstone.x86.X86_OP_REG
                            and item.operands[0].reg == index
                            and item.operands[1].type == capstone.x86.X86_OP_IMM
                            and 59 <= item.operands[1].imm <= 66
                            for item in seq[:-1]
                        ) and any(item.mnemonic == "ja" for item in seq[:-1]):
                            prefix = seq
                            break
                if prefix:
                    table = jump.operands[0].mem.disp
                    limit = next(item.operands[1].imm for item in prefix if item.mnemonic == "cmp")
                    try:
                        pointers = struct.unpack(f"<{limit + 1}I", pe.get_data(table - base, (limit + 1) * 4))
                    except (struct.error, pefile.PEFormatError):
                        cursor += 2
                        continue
                    if any(not any(s.Characteristics & 0x20000000
                                   and s.VirtualAddress <= pointer - base < s.VirtualAddress + s.SizeOfRawData
                                   for s in pe.sections) for pointer in pointers):
                        cursor += 2
                        continue
                    nulls = []
                    targets = {}
                    for kind, target in enumerate(pointers):
                        items = instructions(target, 64)
                        end = next((index + 1 for index, item in enumerate(items)
                                    if item.mnemonic.startswith("ret")), 0)
                        items = items[:end]
                        pure_null = items and all(item.mnemonic in {"pop", "xor", "ret"} for item in items)
                        # MSVC SEH epilogue of the independently identified
                        # Yukikoi factory. These MOVs restore only its frame
                        # and exception chain; no engine object is written.
                        seh_null = (
                            digest == YUKIKOI_SHA and target == 0x536412
                            and pe.get_data(target - base, 22).hex()
                                == "33c08b4df464890d00000000595f5e5b8be55dc20c00"
                        )
                        if items and (pure_null or seh_null) and [
                            item.op_str for item in items if item.mnemonic == "xor"
                        ] == ["eax, eax"] and items[-1].op_str == "0xc":
                            nulls.append(kind)
                            targets[str(kind)] = {"entry_rva": target - base, "instructions": encode(items)}
                    if nulls:
                        markers = {}
                        if digest in NEWTON_SHAS:
                            for kind in [0, 1, 2, 6, 11, 12, 13, 34]:
                                items = instructions(pointers[kind], 96)
                                end = next((index + 1 for index, item in enumerate(items)
                                            if item.mnemonic == "jmp"), len(items))
                                markers[str(kind)] = {
                                    "entry_rva": pointers[kind] - base,
                                    "instructions": encode(items[:end]),
                                }
                        candidates.append({
                            "table_rva": table - base, "jump_rva": va - base,
                            "prefix": encode(prefix), "last_kind": limit,
                            "direct_null_kinds": nulls, "null_entries": targets,
                            "constructor_markers": markers,
                            "identified_factory": (
                                digest == V401_SHA and table == 0x4BAA90
                                or digest == YUKIKOI_SHA and table == 0x536428
                                or digest in NEWTON_SHAS and table == base + 0x1949F0
                            ),
                        })
                cursor += 2
        images.append({"path": row["binary_path"], "sha256": digest,
                       "file_version": row["file_version"], "candidates": candidates})
    report = {
        "schema": "cs2-runtime-factory-null-profile/v1",
        "status": "static-local-instruction-proof-not-global-unreachability",
        "images": images, "original_code_executed": False,
        "limits": "A direct null factory case has no created task. This alone does not prove every numeric native referencing that task is globally unreachable.",
    }
    output = ROOT / "reports/factory_null_profiles_20261117.json"
    output.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"report": output.relative_to(ROOT).as_posix(), "images": [
        {"sha256": image["sha256"], "candidates": [{
            "table_rva": hex(table["table_rva"]), "null_kinds": table["direct_null_kinds"],
            "identified_factory": table["identified_factory"],
        } for table in image["candidates"]]} for image in images],
    }, ensure_ascii=True, indent=2))

if __name__ == "__main__":
    main()

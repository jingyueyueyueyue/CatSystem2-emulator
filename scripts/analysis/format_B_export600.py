"""Read-only wallpaper/export and system-open ABI evidence, including old 8-byte600."""
from __future__ import annotations
import hashlib
import json
import re
import struct
from pathlib import Path

import capstone
import pefile

ROOT = Path(__file__).resolve().parents[2]
OUTPUT = ROOT / "archive/2026-11/20261117_format_B_export600_helpers.json"


def main() -> None:
    summary = json.loads((ROOT / "reports/recovery_A_20261116_summary.json").read_text(encoding="utf-8"))
    old = next(row for row in summary["perPE"]
               if row["sha256"] == "72c9c1e7335a92bc2f1c731275a8da34d638d1730b9d5003fb6a8e85fe5b15c8")
    paths = [(ROOT / "Yukikoi Melt/YukikoiMelt.exe",
              "63de0e14cd8ce11ad2d68e7ed6076641ff5e5b73d822603cc52764009d812976",
              {"type44_constructor": (0x585DE0, 0x6A),
               "type44_receiver": (0x5860C0, 0xE0),
               "type44_worker": (0x5861A0, 0x340),
               "package_load": (0x6F1520, 0x20),
               "package_lookup": (0x6F1540, 0x170),
               "wallpaper_apply": (0x600F00, 0x240),
               "resolved_name_conversion": (0x5FFC90, 0x80)}),
             (ROOT / old["binary_path"], old["sha256"], {})]
    decoder = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_32)
    result = []
    for path, expected, ranges in paths:
        raw = path.read_bytes()
        digest = hashlib.sha256(raw).hexdigest()
        if digest != expected:
            raise SystemExit(f"Unexpected hash {digest}")
        pe = pefile.PE(data=raw, fast_load=True)
        base = pe.OPTIONAL_HEADER.ImageBase
        runs = []
        executable = [(base + section.VirtualAddress,
                       base + section.VirtualAddress + section.SizeOfRawData)
                      for section in pe.sections if section.Characteristics & 0x20000000]
        if digest == old["sha256"]:
            for section in pe.sections:
                if section.Characteristics & 0x20000000:
                    continue
                data = section.get_data()
                words = list(struct.unpack("<" + "I" * (len(data) // 4), data[:len(data) // 4 * 4]))
                start = None
                for index, value in enumerate(words + [0]):
                    if any(left <= value < right for left, right in executable):
                        if start is None:
                            start = index
                    elif start is not None:
                        if index - start >= 40:
                            address = base + section.VirtualAddress + start * 4
                            runs.append({"va": address, "length": index - start})
                            if index - start >= 630:
                                for slot in (0, 1, 66, 323, 600, 601, 629):
                                    ranges[f"candidate_{address:x}_slot{slot}"] = (words[start + slot], 240)
                        start = None
            for section in pe.sections:
                if not section.Characteristics & 0x20000000:
                    continue
                data = section.get_data()
                for hit in re.finditer(re.escape(bytes.fromhex("5c010000")), data):
                    start = data.rfind(b"\xcc\xcc", max(0, hit.start() - 180), hit.start())
                    if start >= 0:
                        va = base + section.VirtualAddress + start + 2
                        decoded = list(decoder.disasm(pe.get_data(va - base, hit.start() - start + 8), va))
                        if any(i.mnemonic in ("cmp", "sub") and i.op_str.endswith(", 0x15c")
                               and not i.op_str.startswith("esp,") for i in decoded):
                            ranges[f"old_wallpaper_receiver_{hit.start():x}"] = (va, 900)
            for section in pe.sections:
                data = section.get_data()
                for hit in list(re.finditer(re.escape("\u58c1\u7d19".encode("cp932")), data))[:12]:
                    address = base + section.VirtualAddress + hit.start()
                    for code_section in pe.sections:
                        if not code_section.Characteristics & 0x20000000:
                            continue
                        code = code_section.get_data()
                        for reference in re.finditer(re.escape(struct.pack("<I", address)), code):
                            if reference.start() and code[reference.start() - 1] == 0x68:
                                start = code.rfind(b"\xcc\xcc", max(0, reference.start() - 1100), reference.start())
                                if start >= 0:
                                    va = base + code_section.VirtualAddress + start + 2
                                    ranges[f"old_wallpaper_literal_{address:x}_{reference.start():x}"] = (va, 1200)
        string_hits = []
        for section in pe.sections:
            data = section.get_data()
            for hit in re.finditer(b"export\x00", data, re.IGNORECASE):
                address = base + section.VirtualAddress + hit.start()
                string_hits.append(address)
                immediate = re.escape(struct.pack("<I", address))
                for code_section in pe.sections:
                    if not code_section.Characteristics & 0x20000000:
                        continue
                    code = code_section.get_data()
                    for reference in re.finditer(immediate, code):
                        offset = reference.start()
                        if offset and code[offset - 1] == 0x68:
                            # Preserve the literal surrounding bytes too. A
                            # backward INT3 boundary is a seed, not a claim.
                            start = code.rfind(b"\xcc\xcc", max(0, offset - 500), offset)
                            start = start + 2 if start >= 0 else max(0, offset - 80)
                            va = base + code_section.VirtualAddress + start
                            ranges[f"export_name_xref_{address:x}_{offset:x}"] = (
                                va, min(offset - start + 340, 900))
        routines = {name: [
            {"va": i.address, "bytes": i.bytes.hex(), "mnemonic": i.mnemonic, "operands": i.op_str}
            for i in decoder.disasm(pe.get_data(va - base, length), va)
        ] for name, (va, length) in ranges.items()}
        result.append({"path": path.relative_to(ROOT).as_posix(), "sha256": digest,
                       "pointer_runs": runs, "export_literals": string_hits, "routines": routines})
    OUTPUT.write_text(json.dumps({"original_code_executed": False, "binaries": result}, indent=2)
                      + "\n", encoding="utf-8")
    print(OUTPUT.relative_to(ROOT).as_posix())


if __name__ == "__main__":
    main()

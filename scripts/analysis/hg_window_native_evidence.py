"""Read fixed PE bytes to locate the ordinary HG window constructor.

This only disassembles data. It never loads a native image or executes code.
"""
from pathlib import Path
import hashlib
import json
import struct

import capstone
import pefile

ROOT = Path(__file__).resolve().parents[2]
OUTPUT = ROOT / "archive/2026-11/20261117_C_hg_window_native.json"
SHAS = {
    "63de0e14cd8ce11ad2d68e7ed6076641ff5e5b73d822603cc52764009d812976",
    "659d5f069f3c97f69a61e12449cf1510b45e48a10649b9ac6977c2b93d6d28ad",
}


def analyze(row):
    data = (ROOT / row["binary_path"]).read_bytes()
    assert hashlib.sha256(data).hexdigest() == row["sha256"]
    pe = pefile.PE(data=data, fast_load=True)
    base = pe.OPTIONAL_HEADER.ImageBase
    decoder = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_32)
    decoder.detail = True

    def code(va, length):
        return [{"rva": i.address - base, "bytes": i.bytes.hex(),
                 "op": i.mnemonic, "args": i.op_str}
                for i in decoder.disasm(pe.get_data(va - base, length), va)]

    pointer_refs = {}
    for section in pe.sections:
        if section.Characteristics & 0x20000000:
            continue
        buf = section.get_data()
        for at in range(0, len(buf)-3, 4):
            value = struct.unpack_from("<I", buf, at)[0]
            if any(s.Characteristics & 0x20000000
                   and s.VirtualAddress <= value-base < s.VirtualAddress+s.SizeOfRawData
                   for s in pe.sections):
                pointer_refs.setdefault(value, []).append(section.VirtualAddress+at)
    allocations = []
    runs = []
    previous = -8
    for slot in sorted(a for refs in pointer_refs.values() for a in refs):
        if slot == previous + 4:
            runs[-1][1] += 1
        else:
            runs.append([slot, 1])
        previous = slot
    long_runs = []
    for start, count in runs:
        if count < 100:
            continue
        pointers = struct.unpack(f"<{count}I", pe.get_data(start, count*4))
        long_runs.append({"table_rva": start, "count": count,
            "entries": {str(n): code(pointers[n], 180)
                for n in (0, 2, 3, 66, 119, 364, 365, 648) if n < count}})
    for section in pe.sections:
        if not section.Characteristics & 0x20000000:
            continue
        buf = section.get_data()
        cursor = 0
        while (cursor := buf.find(bytes.fromhex("6818020000"), cursor)) >= 0:
            va = base + section.VirtualAddress + cursor
            # Native wrappers have a function-pointer entry in the ordinary
            # table within 256 bytes of this allocation site. Keep candidates
            # and all table witnesses rather than guessing the slot identity.
            tables = []
            for distance in range(1, 257):
                entry = va - distance
                for slot in pointer_refs.get(entry, []):
                    table_rva = slot - 648 * 4
                    try:
                        pointers = struct.unpack("<700I", pe.get_data(table_rva, 700 * 4))
                    except (pefile.PEFormatError, struct.error):
                        continue
                    executable = sum(any(s.Characteristics & 0x20000000
                        and s.VirtualAddress <= p - base < s.VirtualAddress + s.SizeOfRawData
                        for s in pe.sections) for p in pointers)
                    if executable >= 680:
                        tables.append({"table_rva": table_rva, "entry_rva": entry-base,
                            "executable_slots": executable,
                            "entries": {str(n): code(pointers[n], 160)
                                for n in (66, 119, 364, 365, 648)}})
            allocations.append({"allocation_rva": va-base, "code": code(va-24, 200), "tables": tables})
            cursor += 5
    extra = {}
    if row["sha256"].startswith("63de"):
        for address, length in ((0x596900, 200), (0x5e14b0, 2300), (0x5dd0b0, 1100),
                                (0x5df410, 600), (0x5df670, 350)):
            extra[hex(address)] = code(address, length)
    else:
        for address, length in ((0x459e00, 145), (0x4a3800, 3000)):
            extra[hex(address)] = code(address, length)
    return {"sha256": row["sha256"], "version": row["file_version"],
            "allocations": allocations, "long_runs": long_runs, "known_functions": extra}


def main():
    rows = json.loads((ROOT / "reports/recovery_A_20261116_summary.json").read_text(encoding="utf-8"))["perPE"]
    result = {"original_code_executed": False,
              "images": [analyze(row) for row in rows if row["sha256"] in SHAS]}
    OUTPUT.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    for image in result["images"]:
        print(json.dumps({"sha256": image["sha256"], "long_runs": [
            (hex(t["table_rva"]), t["count"]) for t in image["long_runs"]], "candidates": [
            {"allocation": hex(a["allocation_rva"]), "tables": [
                {"table": hex(t["table_rva"]), "entry": hex(t["entry_rva"])} for t in a["tables"]]}
            for a in image["allocations"] if a["tables"]]}))


if __name__ == "__main__":
    main()

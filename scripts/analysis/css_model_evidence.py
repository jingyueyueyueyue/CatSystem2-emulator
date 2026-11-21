"""Read-only SHA-pinned legacy DAE/material evidence; never runs game code."""
import argparse
import hashlib
import json
from pathlib import Path
import struct

import capstone
import pefile

ROOT = Path(__file__).resolve().parents[2]
SHA = "72c9c1e7335a92bc2f1c731275a8da34d638d1730b9d5003fb6a8e85fe5b15c8"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--va", nargs="*", default=[])
    parser.add_argument("--length", type=lambda v: int(v, 0), default=256)
    parser.add_argument("--strings", nargs="*", default=[])
    parser.add_argument("--imports", action="store_true")
    parser.add_argument("--calls-to", nargs="*", default=[])
    parser.add_argument("--slots", nargs="*", type=int, default=[])
    args = parser.parse_args()
    ledger = json.loads((ROOT / "reports/css_task_576_F_contracts_20261117.json").read_text(encoding="utf-8"))
    path = ROOT / ledger["legacy_material_receiver"]["path"]
    data = path.read_bytes()
    if hashlib.sha256(data).hexdigest() != SHA:
        raise ValueError("legacy model PE identity changed")
    pe = pefile.PE(data=data)
    base = pe.OPTIONAL_HEADER.ImageBase
    md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_32)
    for slot in args.slots:
        va = struct.unpack("<I", pe.get_data(0x834f80-base+slot*4, 4))[0]
        print("slot", slot, hex(va))
        print("\n".join(f"{i.address:08x} {i.mnemonic} {i.op_str}"
                        for i in md.disasm(pe.get_data(va-base, args.length), va)))
    if args.imports:
        for descriptor in pe.DIRECTORY_ENTRY_IMPORT:
            for symbol in descriptor.imports:
                print(hex(symbol.address), descriptor.dll.decode("ascii"), symbol.name)
    for target in args.calls_to:
        target = int(target, 16)
        for section in pe.sections:
            if not section.Characteristics & 0x20000000:
                continue
            raw = section.get_data()
            section_va = base + section.VirtualAddress
            for at in range(len(raw) - 4):
                if raw[at] == 0xe8 and section_va + at + 5 + struct.unpack_from("<i", raw, at+1)[0] == target:
                    print("call", hex(section_va + at), "->", hex(target))
    for text in args.va:
        va = int(text, 16)
        print("\n".join(f"{i.address:08x} {i.mnemonic} {i.op_str}"
                        for i in md.disasm(pe.get_data(va-base, args.length), va)))
    for text in args.strings:
        va = int(text, 16)
        print(hex(va), repr(pe.get_data(va-base, args.length).split(b"\0", 1)[0]))


if __name__ == "__main__":
    main()

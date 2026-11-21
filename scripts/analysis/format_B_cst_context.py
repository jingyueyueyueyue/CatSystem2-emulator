"""Read-only CST context/save instruction evidence, pinned to the original PE."""
from __future__ import annotations

import hashlib
import json
import struct
from pathlib import Path

import capstone
import pefile

ROOT = Path(__file__).resolve().parents[2]
BINARY = ROOT / "Yukikoi Melt" / "YukikoiMelt.exe"
EXPECTED = "63de0e14cd8ce11ad2d68e7ed6076641ff5e5b73d822603cc52764009d812976"
OUTPUT = ROOT / "archive/2026-11/20261117_format_B_cst_context_static.json"
SLOTS = (323, 325, 327, 333, 334, 335, 336, 337, 385, 386, 608, 609, 610,
         978, 979, 980, 982, 991, 992, 993, 994, 1033, 1037, 1038)


def main() -> None:
    raw = BINARY.read_bytes()
    digest = hashlib.sha256(raw).hexdigest()
    if digest != EXPECTED:
        raise SystemExit(f"Unexpected SHA256 {digest}")
    pe = pefile.PE(data=raw, fast_load=True)
    base = pe.OPTIONAL_HEADER.ImageBase
    decoder = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_32)
    entries = {slot: struct.unpack("<I", pe.get_data(0x6270E8 + slot * 4, 4))[0]
               for slot in SLOTS}
    ranges = {f"ordinary{slot}": (va, 0x500 if slot in (991, 992, 994) else 0x180)
              for slot, va in entries.items()}
    ranges.update({
        "constructor": (0x688D60, 0x1F0),
        "load": (0x689090, 0x2D0),
        "raw_record": (0x689550, 0x240),
        "localized_record": (0x689790, 0x260),
        "snapshot_config": (0x508BA0, 0x270),
        "snapshot_save": (0x508E30, 0x1F0),
        "snapshot_restore": (0x509020, 0x1D0),
        "snapshot_trim": (0x5091F0, 0x170),
        "table_serialize": (0x68ADC0, 0x200),
        "table_restore": (0x68AFC0, 0x470),
        "detail_set": (0x692B50, 0x140),
        "detail_get": (0x692C90, 0x180),
        "scene_query": (0x689A50, 0x60),
        "task_context609": (0x686000, 0x20),
        "task_context610": (0x686020, 0x40),
        "task_context609610_nearby": (0x685F60, 0x200),
        "shared_numeric_constructor": (0x6853A0, 0x120),
        "shared_numeric_destructor": (0x6854C0, 0x120),
        "shared_numeric_read_write": (0x685830, 0x80),
    })
    # Direct call targets of the snapshot wrappers expose the chain helpers
    # without executing the PE or consulting an implementation repository.
    for slot in (991, 992, 993, 994):
        va = entries[slot]
        for instruction in decoder.disasm(pe.get_data(va - base, 0x500), va):
            if instruction.mnemonic == "call" and instruction.op_str.startswith("0x"):
                target = int(instruction.op_str, 16)
                if 0x688000 <= target < 0x690000:
                    ranges[f"snapshot_helper_{target:x}"] = (target, 0x650)
    routines = {name: [
        {"va": ins.address, "mnemonic": ins.mnemonic, "operands": ins.op_str}
        for ins in decoder.disasm(pe.get_data(va - base, length), va)
    ] for name, (va, length) in ranges.items()}
    OUTPUT.write_text(json.dumps({
        "method": "read-only static instructions; no original code executed",
        "binary": BINARY.relative_to(ROOT).as_posix(), "sha256": digest,
        "ordinary_entries": entries, "routines": routines,
    }, indent=2) + "\n", encoding="utf-8")
    for slot, va in entries.items():
        print(f"{slot}: {va:#x}")
    print(OUTPUT.relative_to(ROOT).as_posix())


if __name__ == "__main__":
    main()

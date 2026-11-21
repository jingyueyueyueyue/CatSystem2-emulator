"""KCDB/database static instruction and CP932-key facts. Never executes a PE."""
from __future__ import annotations
import hashlib
import json
import struct
from pathlib import Path
import capstone
import pefile

ROOT = Path(__file__).resolve().parents[2]
BINARY = ROOT / "Yukikoi Melt/YukikoiMelt.exe"
EXPECTED = "63de0e14cd8ce11ad2d68e7ed6076641ff5e5b73d822603cc52764009d812976"
OUTPUT = ROOT / "archive/2026-11/20261117_format_B_database_static.json"
RANGES = {
    "constructor": (0x810810, 0xE0), "destructor": (0x8108F0, 0xF0),
    "set_bit": (0x810BB0, 0x160), "normalize_key": (0x810E90, 0xC0),
    "load_rows": (0x810F50, 0x470), "export_state": (0x8113C0, 0xA8),
    "import_state": (0x811FA0, 0x1B8), "load_resource": (0x812160, 0xB0),
    "set_separators": (0x812210, 0x70), "field_query": (0x6FA3C0, 0x60),
    "container_decode": (0x6FA420, 0x240), "field_name_lookup": (0x6FA670, 0x40),
}


def main() -> None:
    raw = BINARY.read_bytes()
    digest = hashlib.sha256(raw).hexdigest()
    if digest != EXPECTED:
        raise SystemExit(f"Unexpected PE SHA256 {digest}")
    pe = pefile.PE(data=raw, fast_load=True)
    base = pe.OPTIONAL_HEADER.ImageBase
    decoder = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_32)
    routines = {name: [{"va": i.address, "bytes": i.bytes.hex(),
                        "mnemonic": i.mnemonic, "operands": i.op_str}
                       for i in decoder.disasm(pe.get_data(va - base, length), va)]
                for name, (va, length) in RANGES.items()}
    pairs = []
    for index in range(256):
        left, right = struct.unpack("<II", pe.get_data(0x62F640 + index * 8, 8))
        if not left:
            break
        pairs.append([pe.get_data(left - base, 2).hex(), pe.get_data(right - base, 2).hex()])
    OUTPUT.write_text(json.dumps({"original_code_executed": False, "sha256": digest,
                                 "key_pairs": pairs, "routines": routines}, indent=2) + "\n",
                      encoding="utf-8")
    print(OUTPUT.relative_to(ROOT).as_posix())


if __name__ == "__main__":
    main()

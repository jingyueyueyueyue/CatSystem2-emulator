"""Read-only bounded helper evidence for native338/468/564/658/659."""
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
OUTPUT = ROOT / "archive/2026-11/20261117_format_B_scalar_tail_static.json"
RANGES = {
    "local_numeric_set": (0x685F40, 0x20),
    "local_numeric_get": (0x685F60, 0x30),
    "local_numeric_capture": (0x685F90, 0x70),
    "local_numeric_lifetime": (0x685600, 0x160),
    "scene_string": (0x689AB0, 0x40),
    "scene_load_name": (0x689090, 0x2D0),
    "frame_selector": (0x4D0DE0, 0x70),
    "image468": (0x5E4AC0, 0x470),
    "image564": (0x5E5540, 0x10),
}


def main() -> None:
    raw = BINARY.read_bytes()
    digest = hashlib.sha256(raw).hexdigest()
    if digest != EXPECTED:
        raise SystemExit(f"Unexpected PE SHA256 {digest}")
    pe = pefile.PE(data=raw, fast_load=True)
    decoder = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_32)
    base = pe.OPTIONAL_HEADER.ImageBase
    ranges = dict(RANGES)
    for slot in (338, 468, 564, 658, 659):
        entry = struct.unpack("<I", pe.get_data(0x6270E8 + slot * 4, 4))[0]
        ranges[f"ordinary{slot}"] = (entry, 0x60)
    routines = {name: [{"va": i.address, "bytes": i.bytes.hex(),
                        "mnemonic": i.mnemonic, "operands": i.op_str}
                       for i in decoder.disasm(pe.get_data(va - base, length), va)]
                for name, (va, length) in ranges.items()}
    OUTPUT.write_text(json.dumps({"original_code_executed": False, "sha256": digest,
                                 "routines": routines}, indent=2) + "\n", encoding="utf-8")
    print(OUTPUT.relative_to(ROOT).as_posix())


if __name__ == "__main__":
    main()

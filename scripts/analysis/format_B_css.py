"""Read-only, hash-pinned PE evidence for CSS capacity and storage recovery."""
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
OUTPUT = ROOT / "archive" / "2026-11" / "20261117_format_B_css_static.json"
RANGES = {
    "constructor": (0x6EAAA0, 0x50),
    "clear": (0x6EAB20, 0x40),
    "remove": (0x6EAB60, 0xB0),
    "append_record": (0x6EAC10, 0x40),
    "find_record": (0x6EAC50, 0x50),
    "snapshot": (0x6EACA0, 0x40),
    "from_snapshot": (0x6EACE0, 0x70),
    "to_css": (0x6EAD50, 0x270),
    "from_css": (0x6EAFC0, 0x190),
    "clone": (0x6EB150, 0x80),
    "append_bytes": (0x6EB850, 0x90),
    "deflate_helper": (0x806090, 0x190),
    "inflate_helper": (0x806220, 0x156),
}


def main() -> None:
    raw = BINARY.read_bytes()
    digest = hashlib.sha256(raw).hexdigest()
    if digest != EXPECTED:
        raise SystemExit(f"Unexpected PE SHA256: {digest}")
    image = pefile.PE(data=raw, fast_load=True)
    base = image.OPTIONAL_HEADER.ImageBase
    decoder = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_32)
    result = {
        "status": "read-only-static-instruction-evidence",
        "binary": BINARY.relative_to(ROOT).as_posix(),
        "sha256": digest,
        "image_base": base,
        "ordinary_table_rva": 0x6270E8,
        "entries": {
            str(slot): struct.unpack("<I", image.get_data(0x6270E8 + slot * 4, 4))[0]
            for slot in (238, 242, 245, 247, 252, 254, 389, 390, 469)
        },
        "routines": {
            name: [
                {
                    "va": instruction.address,
                    "bytes": instruction.bytes.hex(),
                    "mnemonic": instruction.mnemonic,
                    "operands": instruction.op_str,
                }
                for instruction in decoder.disasm(image.get_data(address - base, length), address)
            ]
            for name, (address, length) in RANGES.items()
        },
    }
    OUTPUT.write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(OUTPUT.relative_to(ROOT).as_posix())


if __name__ == "__main__":
    main()

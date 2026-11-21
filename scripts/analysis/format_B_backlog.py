"""Hash-pinned, read-only backlog wrapper and byte-layout instruction evidence."""
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
OUTPUT = ROOT / "archive" / "2026-11" / "20261117_format_B_backlog_static.json"
RANGES = {
    "ordinary696": (0x5A7F10, 0xE0),
    "ordinary698": (0x5A7C80, 0x130),
    "ordinary977": (0x5A7DB0, 0x140),
    "ordinary699": (0x5A80C0, 0x490),
    "ordinary1034": (0x5A8550, 0x4E0),
    "ordinary700": (0x5A8020, 0xA0),
    "ordinary243": (0x59DB60, 0x80),
    "ordinary253": (0x59D0A0, 0xC0),
    "shared_strings_serialize": (0x699DD0, 0x1B0),
    "shared_strings_restore": (0x699F80, 0x1C0),
    "continuous_command": (0x5A8A50, 0xD0),
    "sound_command": (0x5A8B20, 0x140),
    "temporary_scene_load": (0x689090, 0x2D0),
    "temporary_scene_container": (0x689D50, 0xE4),
    "constructor": (0x68BB80, 0x4A0),
    "helpers": (0x68C020, 0x530),
    "append": (0x68C550, 0x5F0),
    "clear_query_trim": (0x68CB40, 0xD0),
    "serialize": (0x68CC10, 0x290),
    "restore": (0x68CEA0, 0x4F0),
    "layout_prefix": (0x68D390, 0x4E0),
    "preprocess": (0x68D870, 0x1C0),
    "escape": (0x68DA30, 0x194),
    "numeric_span": (0x68DD40, 0x40),
    "layout": (0x68DD80, 0x1170),
    "trim": (0x68F3E0, 0x1C0),
    "filename_register": (0x68EEF0, 0x1C0),
    "name_register": (0x68F0B0, 0x200),
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
        "method": "read-only-static-instructions; no original code executed",
        "binary": BINARY.relative_to(ROOT).as_posix(),
        "sha256": digest,
        "image_base": base,
        "ordinary_entries": {
            str(slot): struct.unpack("<I", image.get_data(0x6270E8 + slot * 4, 4))[0]
            for slot in (696, 697, 698, 699, 700, 785, 850, 851, 876, 977, 1034)
        },
        "routines": {
            name: [
                {
                    "va": instruction.address, "bytes": instruction.bytes.hex(),
                    "mnemonic": instruction.mnemonic, "operands": instruction.op_str,
                }
                for instruction in decoder.disasm(image.get_data(address - base, length), address)
            ]
            for name, (address, length) in RANGES.items()
        },
        "tables": {
            name: {"va": address, "bytes": image.get_data(address - base, length).hex()}
            for name, (address, length) in {
                "escape_primary_jumps": (0x68DBC4, 0x1C),
                "escape_primary_selectors": (0x68DBE0, 0x3A),
                "escape_color_jumps": (0x68DC1C, 0xC),
                "escape_color_selectors": (0x68DC28, 0x74),
                "escape_size_jumps": (0x68DC9C, 0x14),
                "escape_size_selectors": (0x68DCB0, 0x49),
                "escape_position_jumps": (0x68DCFC, 8),
                "escape_position_selectors": (0x68DD04, 0x30),
                "prohibited_line_start_1": (0x90FAC8, 0x48),
                "prohibited_line_start_2": (0x90FB10, 0x30),
                "prohibited_line_start_3": (0x90FB40, 0x2C),
                "opening_characters": (0x90FB80, 0xC),
                "indent_space": (0x90FB94, 4),
            }.items()
        },
    }
    OUTPUT.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    print(OUTPUT.relative_to(ROOT).as_posix())


if __name__ == "__main__":
    main()

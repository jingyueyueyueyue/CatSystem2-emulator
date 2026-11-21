"""Read-only PE evidence for CST history windows and adjacent native gaps."""
from __future__ import annotations
import hashlib
import json
from pathlib import Path
import capstone
import pefile
from native_batch_contracts import analyze

ROOT = Path(__file__).resolve().parents[2]
EXPECTED = "63de0e14cd8ce11ad2d68e7ed6076641ff5e5b73d822603cc52764009d812976"
RANGES = {
    "read_slot": (0x5A2C40, 0x190),
    "shift": (0x5A2DD0, 0x370),
    "load_slot": (0x5A3140, 0x130),
    "rotate_slots": (0x5A3270, 0xB0),
    "create": (0x5A3690, 0x1F0),
    "read_body_name": (0x5A3560, 0x80),
    "read_command": (0x5A3380, 0x160),
    "read_position": (0x5A34E0, 0x80),
    "source_filename": (0x6005C0, 0xA0),
    "name_compare": (0x86628A, 0x30),
    "database_dirty_query": (0x4D4800, 0x10),
    "database_clear_dirty": (0x810D10, 0x08),
    "image_header_query": (0x5E1E10, 0x2D0),
    "image_header_hg3": (0x5FB360, 0xC0),
    "image_header_hg2": (0x5FA140, 0x180),
    "image_header_jpeg": (0x5FBD00, 0x140),
    "image_cache_query430": (0x5CB7B0, 0x1F0),
    "file_open567": (0x52E8A0, 0x1E0),
    "file_size567": (0x52E7A0, 0x50),
    "file_seek567": (0x52EA90, 0x40),
    "file_write567": (0x52EAD0, 0x80),
    "image_manager_alias_init": (0x5C91D6, 0xE2),
    "animation_alias_decode": (0x684010, 0x2B0),
    "animation_alias_query": (0x6842C0, 0x1D0),
    "animation_table_rows": (0x684980, 0x1E0),
    "animation_table_field": (0x685110, 0x220),
    "animation_table_defaults": (0x6848B0, 0x50),
    "animation_alias_uppercase": (0x600CD0, 0x60),
    "replace_extension": (0x5FFEE0, 0x70),
}


def main() -> None:
    raw = (ROOT / "Yukikoi Melt/YukikoiMelt.exe").read_bytes()
    if hashlib.sha256(raw).hexdigest() != EXPECTED:
        raise SystemExit("Unexpected Yukikoi PE SHA256")
    pe = pefile.PE(data=raw, fast_load=True)
    base = pe.OPTIONAL_HEADER.ImageBase
    decoder = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_32)
    routines = {key: [{"va": item.address, "bytes": item.bytes.hex(),
                       "mnemonic": item.mnemonic, "operands": item.op_str}
                      for item in decoder.disasm(pe.get_data(va - base, size), va)]
                for key, (va, size) in RANGES.items()}
    output = ROOT / "archive/2026-11/20261117_format_B_history_window_static.json"
    literals = {}
    for key, va in {"animation_extension": 0x8F07F8, "animation_alias_table": 0x8F06AC}.items():
        encoded = pe.get_data(va - base, 512)
        end = next(index for index in range(0, len(encoded) - 1, 2)
                   if encoded[index:index + 2] == b"\0\0")
        literals[key] = {"va": va, "utf16le": encoded[:end].hex(),
                         "text": encoded[:end].decode("utf-16le")}
    literals["text_file_line_end"] = {"va": 0x9F8E58,
                                      "bytes": pe.get_data(0x5F8E58, 2).hex()}
    output.write_text(json.dumps({"sha256": EXPECTED, "original_code_executed": False,
                                  "routines": routines, "literals": literals}, indent=2) + "\n",
                      encoding="utf-8")
    wrappers = analyze([375, 379, 380, 381, 382, 383, 384, 430, 567, 877, 878], 256)
    wrapper_output = ROOT / "archive/2026-11/20261117_format_B_history_window_wrappers.json"
    wrapper_output.write_text(json.dumps(wrappers, indent=2) + "\n", encoding="utf-8")
    print(output.relative_to(ROOT).as_posix())
    print(wrapper_output.relative_to(ROOT).as_posix())


if __name__ == "__main__":
    main()

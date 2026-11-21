"""Fixed ISLAND image receiver navigation; no decode/CRC/VM/PE execution."""
from pathlib import Path
import hashlib
import json
import re
import struct

import capstone
import pefile

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "archive/2026-11/20261117_format_B_island_image_patch.json"
DLL_SHA = "782674f1eac817b1842f6d0f0d416c17e86fadfb8fb640d335a1da3ce6cabb27"
GAMES = {
    "ISLAND.exe": "474cc11d875d7be9df9581f3a55d64698ccf4e760c44013df403cced85e505f1",
    "ISLAND_CHS.exe": "7bd9f6dcc7d62868d4ad9cc68d5580452e71bd907540ec4f993d960d051d2c45",
}


def disassemble(raw, ranges):
    pe = pefile.PE(data=raw, fast_load=True)
    md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_32)
    base = pe.OPTIONAL_HEADER.ImageBase
    return {label: {"va": hex(va), "bytes_sha256": hashlib.sha256(pe.get_data(va-base, size)).hexdigest(),
        "instructions": [{"va": hex(i.address), "bytes": i.bytes.hex(),
                          "mnemonic": i.mnemonic, "operands": i.op_str}
                         for i in md.disasm(pe.get_data(va-base, size), va)]}
        for label, va, size in ranges}


def main():
    audit = json.loads((ROOT / "reports/all_sample_content_audit.json").read_text(encoding="utf8"))
    game = next(g for g in audit["games"] if Path(g["root"]).name == "ISLAND")
    root = Path(game["root"])
    prior_path = ROOT / "archive/2026-11/20261117_format_B_unindexed_archives.json"
    prior = json.loads(prior_path.read_text(encoding="utf8"))
    pack = next(f for f in prior["files"] if f.get("entry_count") == 458)
    names = [row["name"] for row in pack["entries"]]
    dll = (root / "Accelerator.dll").read_bytes()
    if hashlib.sha256(dll).hexdigest() != DLL_SHA:
        raise ValueError("DLL identity changed")
    pe = pefile.PE(data=dll, fast_load=True)
    result = {
        "compiled": False, "type_checked": False, "tests_run": False,
        "image_decode_or_crc_executed": False, "target_code_executed": False,
        "pack_evidence": prior_path.relative_to(ROOT).as_posix(),
        "pack_sha256": pack["sha256"], "pack_count": len(names),
        "hex_crc_names": sum(re.fullmatch(r"[0-9A-F]{8}\.png", n) is not None for n in names),
        "non_crc_names": [n for n in names if re.fullmatch(r"[0-9A-F]{8}\.png", n) is None],
        "png_bit_depth_color_type": [[8, 6]],
        "actual_kcs_calls": {str(n): game["scripts"]["native_calls"].get(str(n)) for n in (111, 364, 681, 710)},
        "dll": {"path": (root/"Accelerator.dll").relative_to(ROOT).as_posix(), "sha256": DLL_SHA,
            "hook_pointer": hex(struct.unpack("<I", pe.get_data(0x42b60, 4))[0]),
            "crc_table_first8": [hex(x) for x in struct.unpack("<8I", pe.get_data(0x312c0, 32))],
            "format_literal": pe.get_data(0x39c20, 12).split(b"\0")[0].decode("ascii"),
            "routines": disassemble(dll, [
                ("slice_receiver", 0x10008150, 0x228),
                ("detour_arguments", 0x100083a0, 0x25),
                ("crc_entry_and_kernel", 0x10002d70, 0x2ea),
                ("png_receiver", 0x10009140, 0x140),
                ("png_rgba_to_bgra", 0x10009280, 0xab),
                ("directory_loader", 0x100093d0, 0x1d0),
                ("raw_name_hash_lookup", 0x10009640, 0x11f),
                ("raw_name_insert", 0x10009c20, 0x280),
                ("raw_byte_comparison", 0x10008780, 0x85),
            ])},
        "games": [],
        "semantic_limits": [
            "No current-source pixel CRC was executed, so matching counts are not runtime proof.",
            "$RWG4MTC.png is stored verbatim; generated %08X.png cannot reach it in this receiver.",
            "CRC visits each native slice in bottom-up BGR(A), not the full top-down RGBA frame.",
            "PNG rows become BGRA and memcpy uses original slice size; geometry remains original.",
            "Short replacement allocations are explicit diagnostics instead of original out-of-bounds reads.",
        ],
    }
    for name, expected in GAMES.items():
        raw = (root/name).read_bytes()
        if hashlib.sha256(raw).hexdigest() != expected:
            raise ValueError("game identity changed")
        result["games"].append({"path": (root/name).relative_to(ROOT).as_posix(), "sha256": expected,
            "routines": disassemble(raw, [
                ("native_slice_setup", 0x548b3c, 0x90),
                ("bottom_up_predictor_and_hook", 0x548e5e, 0xad),
                ("slice_continuation", 0x548f0b, 0x53),
                ("top_origin_pointer", 0x53b0f0, 0x1c),
            ])})
    OUT.write_text(json.dumps(result, ensure_ascii=False, indent=2)+"\n", encoding="utf8")
    print(json.dumps({"output": OUT.relative_to(ROOT).as_posix(), "pack_entries": len(names),
                      "hex_crc_names": result["hex_crc_names"], "non_crc_names": result["non_crc_names"]}))


if __name__ == "__main__":
    main()

"""Fixed PE source-layer transformation loops; no original code is executed."""
from pathlib import Path
import hashlib
import json
import struct
import sys

sys.dont_write_bytecode = True
from recovery_A_20261116 import Image, ROOT, instruction_row
from native_batch_contracts import analyze

SHA = "63de0e14cd8ce11ad2d68e7ed6076641ff5e5b73d822603cc52764009d812976"
RANGES = {
    "rgb_all_sources": (0x69C260, 0x76),
    "rotate_all_sources": (0x69C0C0, 0x5B),
    "resize_all_sources": (0x69C120, 0x66),
    "blur_all_sources": (0x69A900, 0xBF),
    "flip_all_sources": (0x69B240, 0x59),
    "mono_all_sources": (0x69B140, 0x7A),
    "rotate_directory": (0x6A11D0, 0xA0),
    "resize_directory": (0x6A1100, 0xC2),
    "blur_directory": (0x6A16F0, 0xC0),
    "rebuild_resource": (0x69A9D0, 0x30E),
    "raw_border_expansion": (0x5E5A20, 0x16E),
    "blur_base_adjustment": (0x5E33B0, 0x51),
    "blur_clamped_second_border": (0x5E6130, 0x5E),
    "cached_base_query": (0x69B2A0, 0x17),
    "cached_offset_query": (0x69B2E0, 0x17),
    "cached_canvas_query": (0x69B300, 0x17),
    "cached_size_query": (0x69B320, 0x17),
    "resource_source_geometry_binding": (0x69FDB0, 0xEC),
    "resource_copy735_wrapper": (0x5A8FD0, 0x2B3),
    "list_invalidate_retained_tiles": (0x69D520, 0x77),
    "list_compose_retained_output": (0x69D640, 0x710),
    "list_allocate_tiles": (0x69C340, 0x294),
    "list_invalidate_bounds": (0x69EC90, 0x6F),
    "selected_frame_refreshes_registration": (0x6A03E0, 0xFC),
    "list_local_source_position": (0x6A1080, 0x35),
    "list_draw_nested_resource": (0x6A1050, 0x24),
    "list_bounds_and_clip": (0x6A18B0, 0x4A8),
    "blur_bgr24": (0x5E5C30, 0x4F5),
    "native_scratch_allocation": (0x5FDA40, 0x47),
    "native_canvas_getter": (0x5E4770, 0x40),
    "raw_metadata_setters": (0x5E54D0, 0xD0),
    "resource_directory_geometry": (0x69ACE0, 0x1F6),
    "whole_image_resize": (0x5E4F10, 0x322),
    "whole_image_copy_record": (0x5E5240, 0xCB),
    "image_copy_dispatch": (0x5DD1F0, 0x1E2),
    "image_copy_equal_rectangles": (0x5DD3E0, 0x21D),
    "image_copy_bgra32_rows": (0x5DCCE0, 0xA6),
    "image_copy_sample_edges": (0x5DCE00, 0x1F6),
    "image_copy_fixed_windows": (0x5DD860, 0x1C1),
    "image_copy_interpolation": (0x5DDA30, 0x725),
    "copy_bgr24_to_bgra32_rows": (0x5DE560, 0xE0),
    "copy_bgra32_to_bgr24_rows": (0x5DE9D0, 0xD6),
}

def main():
    image = Image(ROOT / "Yukikoi Melt/YukikoiMelt.exe", SHA)
    output = ROOT / "archive/2026-11/20261117_format_B_image_transform_helpers.json"
    routines = {}
    for name, (address, size) in RANGES.items():
        raw = image.get(address - image.base, size)
        routines[name] = {"va": address, "size": size,
            "bytes_sha256": hashlib.sha256(raw).hexdigest(),
            "instructions": [instruction_row(i, image.base) for i in image.md.disasm(raw, address)]}
    imports = [{"dll": block.dll.decode("ascii"), "name": entry.name.decode("ascii") if entry.name else None,
        "iat_va": entry.address} for block in image.pe.DIRECTORY_ENTRY_IMPORT for entry in block.imports
        if entry.address == 0x8E2348]
    legacy_sha = "0e1dde51e86e9152d8e1d27b82efb15916cb9f82d23dd30990212fafd96c8b9f"
    legacy = Image(ROOT / "cs2_full_v401/system/cs2_open.exe", legacy_sha)
    copy_table = list(struct.unpack("<21I", image.get(0xA2869C - image.base, 84)))
    legacy_table = list(struct.unpack("<21I", legacy.get(0xBD03DC - legacy.base, 84)))
    legacy_ranges = {}
    for name, address, size in [("copy_integer_windows_24_32", 0x55EFB0, 0x31A),
        ("copy_bgr24_to_bgra32_rows", legacy_table[16], 0xE8),
        ("copy_bgra32_to_bgr24_rows", legacy_table[19], 0xE8)]:
        raw = legacy.get(address - legacy.base, size)
        legacy_ranges[name] = {"va": address, "size": size,
            "bytes_sha256": hashlib.sha256(raw).hexdigest(),
            "instructions": [instruction_row(i, legacy.base) for i in legacy.md.disasm(raw, address)]}
    output.write_text(json.dumps({"path": "Yukikoi Melt/YukikoiMelt.exe", "sha256": SHA,
        "scratch_allocator_import": imports,
        "copy_format_table": {"va": 0xA2869C, "index": "target_bpp+source_bpp*4", "pointers": copy_table},
        "v401": {"path": "cs2_full_v401/system/cs2_open.exe", "sha256": legacy_sha,
            "copy_format_table": {"va": 0xBD03DC, "index": "target_bpp+source_bpp*4", "pointers": legacy_table},
            "routines": legacy_ranges},
        "original_code_executed": False, "routines": routines}, indent=2) + "\n", encoding="utf8")
    if "--helpers-only" not in sys.argv:
        wrappers = analyze([713, 723, 726, 727, 730, 737], 90)
        (output.parent / "20261117_format_B_image_transform_wrappers.json").write_text(
            json.dumps(wrappers, ensure_ascii=True, indent=2) + "\n", encoding="utf8")
    print(json.dumps({"output": output.relative_to(ROOT).as_posix(), "routines": len(routines),
        "compiled": False, "original_code_executed": False}))

if __name__ == "__main__":
    main()

"""Read-only PE instruction evidence for ordinary image wrapper recovery."""
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
OUTPUT = ROOT / "archive" / "2026-11" / "20261117_format_B_image_batch_static.json"

# Explicit ranges retain branch tails beyond the first RET.
RANGES = {
    "ordinary363": (0x5971B0, 0x60),
    "ordinary364": (0x597920, 0xB0),
    "ordinary365": (0x596730, 0x150),
    "ordinary367": (0x5963F0, 0x2C0),
    "ordinary369": (0x596B40, 0x20),
    "ordinary370": (0x597CD0, 0xE0),
    "ordinary371": (0x596CE0, 0x60),
    "ordinary372": (0x597C20, 0x40),
    "ordinary373": (0x597BE0, 0x40),
    "ordinary374": (0x597B70, 0x40),
    "ordinary376": (0x595BD0, 0x60),
    "ordinary121": (0x597C60, 0x40),
    "ordinary460": (0x5976D0, 0xB0),
    "ordinary431": (0x5970F0, 0x60),
    "ordinary537": (0x595AB0, 0x50),
    "ordinary558": (0x596BD0, 0x20),
    "ordinary681": (0x595B30, 0xA0),
    "ordinary682": (0x596B20, 0x20),
    "ordinary1026": (0x597250, 0x30),
    "manager_constructor": (0x5C8D30, 0x9D0),
    "manager_destructor": (0x5C9300, 0x270),
    "manager_cache_evict": (0x5C9570, 0x1B0),
    "manager_preload": (0x5C9720, 0x30),
    "manager_layer_plan": (0x5C9850, 0x7C0),
    "manager_load_single": (0x5CA590, 0xD0),
    "manager_cache_get": (0x5CB360, 0x110),
    "manager_cache_peek": (0x5CB470, 0x110),
    "manager_cache_touch": (0x5CB9A0, 0x310),
    "manager_cache_put": (0x5CBCE0, 0x390),
    "manager_force_cache": (0x5CB120, 0x10),
    "wrapper_deep_copy": (0x5E1200, 0x70),
    "pixel_deep_copy": (0x5E3530, 0xD0),
    "pixel_merge_attributes": (0x5E3220, 0x180),
    "pixel_blur_dispatch": (0x5E33B0, 0x60),
    "ordinary538": (0x595B00, 0x30),
    "pixel_blur_bgr24": (0x5E5C30, 0x500),
    "pixel_blur_bgra32": (0x5E6130, 0x8A0),
    "pixel_expand_border": (0x5E5A20, 0x210),
    "pixel_flip": (0x5E4320, 0x1C0),
    "pixel_rgb_percent": (0x5E5590, 0x490),
    "pixel_global_argb": (0x5E47A0, 0xB0),
    "layer_selectors": (0x6EFFA0, 0xD0),
    "aggregate_metadata_dispatch": (0x5E1EA0, 0x80),
    "frame_metadata_dispatch": (0x5E1E10, 0x90),
    "wrapper_allocation_size": (0x5E1F30, 0x50),
    "pixel_dimensions": (0x5E48A0, 0x20),
    "pixel_canvas_size": (0x5E4770, 0x30),
    "pixel_allocate": (0x5E3910, 0x1A0),
    "jpeg_metadata_probe": (0x5FBD00, 0x140),
    "jpeg_constructor_and_decode": (0x5FB810, 0x4F0),
    "hg3_aggregate_metadata": (0x5FB420, 0x180),
    "hg2_aggregate_metadata": (0x5FA2C0, 0x280),
    "hg2_decode": (0x5F9370, 0x700),
    "hg2_frame_by_index": (0x5FA090, 0x88),
    "hg2_frame_metadata": (0x5FA140, 0x17B),
    "image_modulate": (0x5E3D20, 0x110),
    "image_monochrome": (0x5E3E30, 0x180),
    "image_negative": (0x5E3FB0, 0x50),
    "image_sepia": (0x5E4000, 0x1D0),
    "image_metadata_setters": (0x5E54D0, 0xC0),
    "image_base_query": (0x5E4540, 0x20),
    "image_bottom_pixel": (0x5E4870, 0x30),
    "ordinary685": (0x5A79F0, 0x100),
    "wrapper_pixels": (0x4D0DE0, 0x10),
    "pixel_offset": (0x5E4750, 0x30),
    "manager_raw_load": (0x5CA700, 0xC0),
    "wrapper_load": (0x5E2310, 0x450),
    "hg3_signature": (0x5FB5A0, 0x30),
    "hg2_signature": (0x5FA540, 0x30),
    "hg3_load": (0x5FA7F0, 0xA50),
    "convert_bgr24": (0x5E3030, 0x180),
    "ordinary683": (0x5A78C0, 0x130),
    "ordinary684": (0x5A7890, 0x30),
    "object683_load": (0x6E7970, 0x460),
    "pair_table_lines": (0x6E67D0, 0x190),
    "pair_table_comments": (0x6E6EA0, 0x150),
    "pair_table_spaces": (0x6E6FF0, 0x370),
    "pair_table_split": (0x6E8040, 0x270),
    "resource_open": (0x52C5D0, 0x1C0),
    "vm_format_string": (0x892B60, 0x30),
    "bmp_signature": (0x5F82C0, 0x30),
    "bmp_constructor": (0x5F7F10, 0x60),
    "bmp_decode": (0x5F80E0, 0x1E0),
    "manager367_canvas": (0x5CC7F0, 0x200),
    "pixel_global_clip": (0x5E3490, 0x90),
    "pixel_constructor": (0x5E2EF0, 0xA0),
    "pixel_mask": (0x534EE0, 0x10),
    "hg3_wrapper_load": (0x5FB240, 0x120),
    "blend_dispatch": (0x5DD0B0, 0x140),
    "blend_clip": (0x5DD3E0, 0x310),
    "blend_rgba32": (0x5DF410, 0x460),
    "object685": (0x6E7DD0, 0x250),
}
DATA_RANGES = {
    "blend_function_tables": (0xA2861C, 0x120),
    "bmp_vtable": (0x8F1174, 0x10),
    "hg3_vtable": (0x8F1308, 0x10),
    "hg3_magic": (0x8F1358, 4),
    "hg2_magic": (0x8F1224, 4),
    "tab_replacement": (0x9138D8, 2),
    "image_grayscale_f32_coefficients": (0x8F0FD4, 12),
    "image_cache_key_format": (0x8F0800, 8),
    "image_layer_name_format": (0x8F06C8, 8),
    "image_layer_leading_zero": (0x8F06D0, 4),
    "image_layer_delimiter": (0x913A8C, 2),
}


def main() -> None:
    data = BINARY.read_bytes()
    digest = hashlib.sha256(data).hexdigest()
    if digest != EXPECTED:
        raise SystemExit(f"Unexpected PE SHA256: {digest}")
    image = pefile.PE(data=data, fast_load=True)
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
            for slot in (121, 363, 364, 365, 367, 369, 370, 371, 372, 373, 374, 376,
                         431, 460, 537, 538, 558, 681, 682, 683, 684, 685, 1026)
        },
        "routines": {},
        "data_ranges": {
            name: {"va": address, "bytes": image.get_data(address - base, length).hex()}
            for name, (address, length) in DATA_RANGES.items()
        },
    }
    for name, (address, length) in RANGES.items():
        result["routines"][name] = [
            {
                "va": instruction.address,
                "bytes": instruction.bytes.hex(),
                "mnemonic": instruction.mnemonic,
                "operands": instruction.op_str,
            }
            for instruction in decoder.disasm(image.get_data(address - base, length), address)
        ]
    OUTPUT.write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(OUTPUT.relative_to(ROOT).as_posix())


if __name__ == "__main__":
    main()

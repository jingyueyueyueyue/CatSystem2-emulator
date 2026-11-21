"""Pin software mosaic resampling evidence to two exact, read-only PE images.

This reads executable bytes and decodes bounded instruction ranges. It never
loads an image as a library or executes a recovered pixel/VM/game function.
"""
from __future__ import annotations

import hashlib
import json
from pathlib import Path
import struct

import capstone
import pefile

ROOT = Path(__file__).resolve().parents[2]
OUTPUT = ROOT / "reports/frame_effect_resample_F_contracts_20261117.json"

PROFILES = [
    {
        "name": "v401",
        "path": "cs2_full_v401/system/cs2_open.exe",
        "sha256": "0e1dde51e86e9152d8e1d27b82efb15916cb9f82d23dd30990212fafd96c8b9f",
        "copy_table_va": 0xBD03DC,
        "copy_va": 0x55DEF0,
        "fragments": [
            ("wmosaic_soft_call", 0x4D60D4, 5),
            ("fmosaic_soft_call", 0x4D7B32, 5),
            ("dispatch", 0x55E3F0, 0x1E2),
            ("format4_row_copy", 0x55DEF0, 0xA9),
            ("format4_sample_edges", 0x55E010, 0xF0),
            ("format4_bilinear", 0x55EA10, 0x2D3),
            ("format4_integer_box", 0x55EFB0, 0x1C2),
            ("auxiliary_pool_gates_growth_caps_and_limit", 0x546E10, 0x152),
            ("device_npot_capability", 0x547FE0, 0x83),
            ("wmosaic_hard_format32_write_order", 0x4D620E, 0x341),
            ("fmosaic_hard_format32_write_order", 0x4D7C6C, 0x344),
            ("wmosaic_native_level_change_and_dimensions", 0x4D58F6, 0x60),
            ("fmosaic_native_level_change_and_dimensions", 0x4D726A, 0xBB),
            ("fmosaic_alpha_at_first_changed_block", 0x4D7449, 0x70),
        ],
        "strong_shrink": {
            "boundaries": "signed integer floor((index+1)*source/destination)",
            "kernel": "difference from preceding boundary, minimum one per axis",
            "sum": "four independent wrapping DWORD channel sums",
            "division": "signed IDIV with actual rectangular sample count",
        },
    },
    {
        "name": "yukikoi",
        "path": "Yukikoi Melt/YukikoiMelt.exe",
        "sha256": "63de0e14cd8ce11ad2d68e7ed6076641ff5e5b73d822603cc52764009d812976",
        "copy_table_va": 0xA2869C,
        "copy_va": 0x5DCCE0,
        "fragments": [
            ("wmosaic_level_scale_call", 0x553F23, 5),
            ("wmosaic_retained_previous_level_source", 0x553E67, 0xC1),
            ("dispatch", 0x5DD1F0, 0x1E2),
            ("format4_row_copy", 0x5DCCE0, 0xA5),
            ("format4_sample_edges", 0x5DCE00, 0xF3),
            ("format4_bilinear", 0x5DDA30, 0x2D6),
            ("float_origin_fixed_box", 0x5DD860, 0x1C1),
            ("fmosaic_hard_format32_write_order", 0x55667B, 0x379),
            ("wmosaic_hard_format32_write_order", 0x554DAD, 0x36D),
        ],
        "strong_shrink": {
            "ratio": "CVTDQ2PS/DIVSS source/destination, independent f32 axes",
            "origins": "CVTTSS2SI(f32(index)*ratio)",
            "kernel": "max(1,CVTTSS2SI(ratio)) fixed across destination",
            "sum": "independent wrapping DWORD channel sums",
            "division": "unsigned DIV with actual sample count",
        },
    },
]


def fragment(pe, label, va, length):
    base = pe.OPTIONAL_HEADER.ImageBase
    data = pe.get_data(va - base, length)
    decoder = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_32)
    return {
        "label": label, "va": va, "rva": va - base, "bytes": data.hex(),
        "instructions": [
            {"va": ins.address, "bytes": ins.bytes.hex(),
             "mnemonic": ins.mnemonic, "operands": ins.op_str}
            for ins in decoder.disasm(data, va)
        ],
    }


def main():
    profiles = []
    for description in PROFILES:
        path = ROOT / description["path"]
        if hashlib.sha256(path.read_bytes()).hexdigest() != description["sha256"]:
            raise ValueError("fixed executable changed")
        pe = pefile.PE(str(path))
        copy_pointer = struct.unpack("<I", pe.get_data(
            description["copy_table_va"] - pe.OPTIONAL_HEADER.ImageBase + (4 + 4*4)*4, 4))[0]
        if copy_pointer != description["copy_va"]:
            raise ValueError("format4-to-format4 dispatch identity changed")
        row = dict(description)
        row["fragments"] = [fragment(pe, *anchor) for anchor in description["fragments"]]
        profiles.append(row)
    result = {
        "schema": "cs2-frame-effect-format4-resample-static/v1", "profiles": profiles,
        "shared_observations": [
            "Equal extents use format4 row copy, including transparent source pixels.",
            "Either source axis greater than four times destination selects the box path.",
            "Other unequal dimensions use signed 15-bit steps, without half-pixel offset.",
            "Each two-dimensional weight truncates before the final channel truncation.",
            "Zero combined sample alpha skips the existing destination; all255 forces alpha255.",
            "Missing fourth corner repeats p00 whenever either axis has no next texel.",
            "Hard format32 expansion emits leading floor(block/2), middle source rows guarded by block*row+offset, then the actual tail cursor.",
            "Hard expansion sets output alpha255 and must not clamp an unbacked source tail.",
            "Native level generation keeps only the first timestamp of each changed truncated f32 block and the alpha calculated at that timestamp.",
        ],
        "limits": [
            "Only the CPU full-rectangle format4 path is implemented here.",
            "The two strong-shrink receivers differ; no version alias is inferred.",
            "No device-resolution ratio, GPU filtering, reset or 16-bit format is inferred.",
            "This is static recovery, not an execution or bitwise output comparison.",
        ],
        "next_device_findings": {
            "v401_npot_caps": "547fe0 reads D3DCAPS9+0x3c, masks0x102; NPOT allowed only when mask is0 or0x102.",
            "v401_auxiliary": "546e10 reads live surface+0x144/+0x148; absent NPOT pads each axis to power-of-two BEFORE configured tex_size check.",
            "integration_limit": "Current CPU sampler has a fixed padded texture model. No genuine host D3DCAPS/reset producer is bound; do not infer it from a window/backbuffer ratio.",
        },
        "compiler_invoked": False, "tests_run": False,
        "original_functions_executed": False, "vm_executed": False,
    }
    OUTPUT.write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"report": OUTPUT.relative_to(ROOT).as_posix(),
                      "profiles": len(profiles), "original_functions_executed": False}))


if __name__ == "__main__":
    main()

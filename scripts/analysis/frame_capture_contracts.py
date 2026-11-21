"""Record fixed-PE continuous-capture state, pixel conversion and BMP evidence.

Only immutable executable bytes are read. No engine, VM, compiler, capture
writer or graphics driver is invoked.
"""
from __future__ import annotations
import json
from datetime import datetime, timezone
from recovery_A_20261116 import Image, ROOT, instruction_row

PROFILES = [
    {
        "pe": "cs2_full_v401/system/cs2_open.exe",
        "sha256": "0e1dde51e86e9152d8e1d27b82efb15916cb9f82d23dd30990212fafd96c8b9f",
        "string_encoding": "cp932",
        "format_va": 0x99A0CC,
        "state_offsets": {"enabled": 0x84, "index": 0x88, "name": 0x8C},
        "regions": [
            ("set", 0x54E170, 0x2E), ("get_enabled", 0x54D8C0, 7),
            ("get_index", 0x54D200, 7), ("consumer", 0x54D050, 0x9B),
            ("readback", 0x54CA70, 0x570), ("flip_rows", 0x563AA0, 0x90),
            ("flatten_white", 0x562BD0, 0x160), ("writer", 0x566A00, 0x300),
        ],
    },
    {
        "pe": "Yukikoi Melt/YukikoiMelt.exe",
        "sha256": "63de0e14cd8ce11ad2d68e7ed6076641ff5e5b73d822603cc52764009d812976",
        "string_encoding": "utf-16-le",
        "format_va": 0x8F07D8,
        "state_offsets": {"enabled": 0xA4, "index": 0xA8, "name": 0xAC},
        "regions": [
            ("set", 0x5CC1E0, 0x30), ("get_enabled", 0x5CB7A0, 7),
            ("get_index", 0x5CB1A0, 7), ("consumer", 0x5CAF90, 0xFC),
            ("readback", 0x5CA960, 0x3BA), ("force_alpha", 0x5E41E0, 0xC8),
            ("flip_rows", 0x5E4A30, 0x8F), ("flatten_white", 0x5E3BC0, 0x151),
            ("writer", 0x5E2940, 0x2EC), ("bmp_constructor", 0x5F7F10, 0x5B),
            ("bmp_encode", 0x5F7FA0, 0x138),
        ],
    },
]

def main() -> None:
    rows = []
    for profile in PROFILES:
        image = Image(ROOT / profile["pe"], profile["sha256"])
        data = image.get(profile["format_va"] - image.base, 96)
        if profile["string_encoding"] == "utf-16-le":
            end = next(offset for offset in range(0, len(data) - 1, 2)
                       if data[offset:offset + 2] == b"\0\0")
        else:
            end = data.index(0)
        value = data[:end].decode(profile["string_encoding"])
        if value != r"%s\%s_%010d.bmp":
            raise ValueError("capture format identity changed")
        rows.append({
            key: value for key, value in profile.items() if key != "regions"
        } | {
            "format_string": value, "image_base": image.base,
            "instruction_regions": [
                {"role": role, "va": address, "byte_limit": length,
                 "instructions": [instruction_row(item, image.base)
                                  for item in image.insns(address - image.base, length)]}
                for role, address, length in profile["regions"]
            ],
        })
    report = {
        "schema": "cs2-frame-capture-contracts/v1",
        "generated_at": datetime.now(timezone.utc).isoformat(),
        "status": "static-fixed-PE-evidence-and-source-wiring-not-executed-equivalence",
        "profiles": rows,
        "contract": {
            "natives": [441, 565, 566], "enabled_is_raw_dword": True,
            "index_is_wrapping_dword": True, "filename_index_is_signed_decimal": True,
            "filename_format_reuses_name_twice": True, "increment_before_writer": True,
            "writer_result_does_not_disable_capture": True,
            "file_root": "independent private game save profile",
            "row_stride": "align4(width*3)", "output_allocation": "align8(54+stride*height)",
            "bfSize_excludes_final_allocation_padding": True,
            "readback_backend": "actual completed independent CPU logical surface",
            "driver_specific_readback_equivalence_verified": False,
        },
        "source_paths": [
            "simulator/crates/desktop/src/frame_capture.rs",
            "simulator/crates/renderer/src/capture.rs",
            "simulator/crates/desktop/src/data_services.rs",
            "simulator/crates/desktop/src/lib.rs",
        ],
        "compiled": False, "tests_run": False, "game_or_DLL_execution": False,
    }
    output = ROOT / "reports/frame_capture_contracts_20261117.json"
    output.write_text(json.dumps(report, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print(json.dumps({"report": output.relative_to(ROOT).as_posix(), "profiles": len(rows),
                      "original_code_executed": False}))

if __name__ == "__main__":
    main()

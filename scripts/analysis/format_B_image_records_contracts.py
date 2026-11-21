"""Read-only record ABI and list-object evidence; never calls PE code."""
from __future__ import annotations

import json
import hashlib
from pathlib import Path
import pefile

from recovery_A_20261116 import Image, ROOT, classify_tables, instruction_row, registrations


def main() -> None:
    summary = json.loads((ROOT / "reports/recovery_A_20261116_summary.json").read_text(encoding="utf8"))
    binaries = []
    for row in summary["perPE"]:
        if Path(row["binary_path"]).suffix.lower() != ".exe":
            continue
        image = Image(ROOT / row["binary_path"], row["sha256"])
        _, observed = registrations(image)
        tables = [table for table in classify_tables(image, observed) if table["table_hint"] == "ordinary"]
        for table in tables:
            if len(table["slots"]) <= 704:
                continue
            slot = table["slots"][704]
            rva = slot["entry_rva"] if isinstance(slot, dict) else slot
            instructions = []
            for item in image.md.disasm(image.get(rva, 1024), image.base + rva, count=128):
                instructions.append(item)
                if item.mnemonic.startswith("ret"):
                    break
            strides = [int(item.op_str.split(", ")[1], 16) for item in instructions
                       if item.mnemonic == "add" and item.op_str in {"esi, 0x48", "esi, 0x88"}]
            binaries.append({"path": row["binary_path"], "sha256": row["sha256"],
                             "entry_rva": rva, "record_stride": strides[0] if len(strides) == 1 else None,
                             "instructions": [instruction_row(item, image.base) for item in instructions]})
    yuki = next(row for row in summary["perPE"] if row["binary_path"] == "Yukikoi Melt/YukikoiMelt.exe")
    image = Image(ROOT / yuki["binary_path"], yuki["sha256"])
    ranges = {"entry_constructor": (0x69F8B0, 0x196), "entry_attach_hg": (0x69FEA0, 0x46E),
              "entry_release": (0x69FBD0, 0x60), "read_image_raw": (0x5CA7A0, 0xB4),
              "records703": (0x5CA010, 0x4B0), "enumerate_image_ids": (0x5E1D50, 0xA6),
              "selector_parser": (0x6EFFA0, 0xC9), "to_cp932": (0x5FFC70, 0x20),
              "cp932_conversion": (0x5FFC10, 0x3C)}
    routines = {name: [instruction_row(item, image.base)
                       for item in image.md.disasm(image.get(va - image.base, size), va)]
                for name, (va, size) in ranges.items()}
    pe = pefile.PE(str(ROOT / yuki["binary_path"]))
    imported_strings = [{"address": item.address, "dll": descriptor.dll.decode("ascii"),
                         "name": item.name.decode("ascii") if item.name else None}
                        for descriptor in pe.DIRECTORY_ENTRY_IMPORT for item in descriptor.imports
                        if item.address in {0x8E2508, 0x8E2168, 0x8E2260}]
    v401 = next(row for row in summary["perPE"] if row["binary_path"] == "cs2_full_v401/system/cs2_open.exe")
    old = Image(ROOT / v401["binary_path"], v401["sha256"])
    old_body = [instruction_row(item, old.base)
                for item in old.md.disasm(old.get(0x54C0F0 - old.base, 0x5F0), 0x54C0F0)]
    path = ROOT / "archive/2026-11/20261117_format_B_image_records_abi.json"
    path.write_text(json.dumps({"original_code_executed": False, "binaries": binaries,
                                "helper_sha256": yuki["sha256"], "routines": routines,
                                "string_imports": imported_strings,
                                "wide_format_hex": image.get(0x9DC598 - image.base, 8).hex(),
                                "v401_sha256": v401["sha256"], "v401_records703": old_body}, indent=2) + "\n", encoding="utf8")
    compatibility = {
        "original_code_executed": False,
        "default_policy": "pe-compatibility", "explicit_repair_policy": "portable-static-runs",
        "profiles": [
            {"sha256": v401["sha256"], "path": v401["binary_path"], "entry_va": "54c0f0",
             "stride": 72, "layer_behavior": "narrow-pe-state-machine",
             "body_sha256": hashlib.sha256(old.get(0x54c0f0-old.base, 0x5e2)).hexdigest(),
             "animation_extension_appended_twice": True, "flush_gated_by_previous_animation": True,
             "static_flush_excludes_current_selector": True, "final_flush": False,
             "static_record_tail_source": "current physical name, not pending loaded name",
             "prior_outputs_retained_on_error": True},
            {"sha256": yuki["sha256"], "path": yuki["binary_path"], "entry_va": "5ca010",
             "stride": 136, "layer_behavior": "diagnose-narrow-as-wide-uninitialized-stack",
             "body_sha256": hashlib.sha256(image.get(0x5ca010-image.base, 0x49d)).hexdigest(),
             "first_mixed_encoding_call_va": "5ca1b1", "prefix_narrow_copy_va": "6f0007",
             "buffer_terminated_bytes": 1, "consumer_terminator_bytes": 2},
        ],
        "other_known_stride_profiles_have_no_inferred_layer_contract": True,
        "v401_string_bytes": {hex(va): old.get(va-old.base, 12).hex() for va in
            (0xae75d8, 0x99a0c4, 0x99a0c8, 0xb006ac, 0xb003d8, 0xb000d4)},
        "yukikoi_string_imports": imported_strings,
    }
    (ROOT / "archive/2026-11/20261117_format_B_image_records_compat.json").write_text(
        json.dumps(compatibility, indent=2)+"\n", encoding="utf8")
    print(json.dumps([{key: row[key] for key in ("path", "sha256", "record_stride")}
                      for row in binaries], ensure_ascii=True, indent=2))


if __name__ == "__main__":
    main()

"""Bounded game-call/PE evidence for the actually used resource chain.

Only parses stored inventories, immutable PE/container bytes and Rust grammar.
No original instruction, simulator, compiler or test is executed.
"""
from __future__ import annotations
import hashlib
import json
from pathlib import Path
import subprocess
import sys

sys.dont_write_bytecode = True
from recovery_A_20261116 import Image, ROOT, instruction_row

OUTPUT = ROOT / "archive/2026-11/20261117_format_B_resource_game_chain.json"
RANGES = {
    "UTATEMEGURI.exe": {
        "ordinary_table_inline_registration": (0x43074A, 0x43),
        "ordinary703_pointer": (0x45B240, 0x7B),
        "ordinary150_pointer": (0x45AD70, 0x5F),
        "input128_129_switch": (0x43A520, 0x31),
        "input128_129_body": (0x43AB1F, 0x7C),
        "input_coordinate_conversion": (0x489C50, 0x100),
        "ordinary624": (0x453600, 0xAF),
    },
    "YukikoiMelt.exe": {
        "ordinary710": (0x5A9740, 0x180),
        "resource_load710": (0x69B420, 0xB80),
        "entry_attach_frame_directory": (0x69FEA0, 0x46E),
        "resource_companion_bytes": (0x69B360, 0xB6),
        "resource_internal_composition": (0x69A9D0, 0x50E),
        "resource_final_release": (0x69AEE0, 0xAB),
        "resource_forward_playing": (0x69BFA0, 0x46),
        "resource_forward_restart": (0x69C070, 0x44),
        "resource_forward_variable": (0x69C1F0, 0x60),
        "list_update_nested_resource": (0x6A09A0, 0x177),
        "list_recursive_restart": (0x6A0B40, 0x17B),
        "list_forward_variable": (0x6A0CD0, 0x2B),
    },
}
OLD_ANIMATION = {
    "a9f8b08713aac278671853b2293a13b2623a122ede9341cb921901b09601cb52":
        (0x69DE80, 0x453600, 0x5AAB6C, 0x5AB840),
    "72c9c1e7335a92bc2f1c731275a8da34d638d1730b9d5003fb6a8e85fe5b15c8":
        (0x834F80, 0x44B490, 0x6F7828, 0x6F7EB0),
    "659d5f069f3c97f69a61e12449cf1510b45e48a10649b9ac6977c2b93d6d28ad":
        (0x6E6100, 0x44FE10, 0x5D80A4, 0x5D8730),
}
FILES = ["resource_direct_native.rs", "message_platform.rs", "image_manager.rs",
         "image_pixels.rs", "list_native.rs", "native_tasks.rs", "data_services.rs", "lib.rs"]


def main():
    inventory_path = ROOT / "reports/kcs_call_inventory_20261116.json"
    inventory = json.loads(inventory_path.read_text(encoding="utf8"))
    calls = [{"game": game["root"], "resource": program["resource"],
              "sha256": program["sha256"], "table_hint": program["table_hint"], **call}
             for game in inventory["games"] for program in game["programs"]
             for call in program["calls"] if call["index"] in (703, 710, 624)]
    summary = json.loads((ROOT / "reports/recovery_A_20261116_summary.json").read_text(encoding="utf8"))
    binaries = []
    for row in summary["perPE"]:
        spans = dict(RANGES.get(Path(row["binary_path"]).name, {}))
        old_animation = OLD_ANIMATION.get(row["sha256"])
        if old_animation:
            spans["ordinary624"] = (old_animation[1], 0xAF)
        if not spans:
            continue
        image = Image(ROOT / row["binary_path"], row["sha256"])
        routines = {name: {"va": va, "bytes_sha256": hashlib.sha256(image.get(va-image.base, size)).hexdigest(),
                           "instructions": [instruction_row(x, image.base)
                                            for x in image.md.disasm(image.get(va-image.base, size), va)]}
                    for name, (va, size) in spans.items()}
        record = {"path": row["binary_path"], "sha256": row["sha256"], "routines": routines}
        if old_animation:
            table, entry, category, extension = old_animation
            assert image.u32(table-image.base+624*4) == entry
            assert image.get(category-image.base, 6) == b"image\0"
            assert image.get(extension-image.base, 4) == b"anm\0"
            record["old624"] = {"table_va": table, "entry_va": entry,
                                "category_va": category, "extension_va": extension}
        if Path(row["binary_path"]).name == "UTATEMEGURI.exe":
            record["ordinary_table"] = {"va": 0x69DE80, "slot703": image.u32(0x69DE80-image.base+703*4)}
            record["input_switch"] = {
                str(code): image.u32(0x43AF84-image.base+4*image.get(0x43B004-image.base+code-3, 1)[0])
                for code in (128, 129)}
            record["input_rtti"] = image.get(0x6C8E58-image.base, 17).hex()
            record["input_vtable"] = image.get(0x5AB450-image.base, 32).hex()
            record["624_category_literal"] = image.get(0x5AAB6C-image.base, 6).hex()
        binaries.append(record)
    formatter = Path.home() / ".rustup/toolchains/stable-x86_64-pc-windows-gnu/bin/rustfmt.exe"
    syntax = []
    for name in FILES:
        path = ROOT / "simulator/crates/desktop/src" / name
        raw = path.read_bytes()
        result = subprocess.run([str(formatter), "--edition", "2024", "--config", "skip_children=true",
                                 "--emit", "stdout"], input=raw, capture_output=True, check=False)
        syntax.append({"path": path.relative_to(ROOT).as_posix(), "sha256": hashlib.sha256(raw).hexdigest(),
                       "parsed": result.returncode == 0, "unchanged": raw == path.read_bytes(),
                       "diagnostic": result.stderr.decode("utf8", errors="replace")})
    OUTPUT.write_text(json.dumps({"original_code_executed": False, "compiled": False, "tests_run": False,
                                 "inventory_sha256": hashlib.sha256(inventory_path.read_bytes()).hexdigest(),
                                 "calls": calls, "binaries": binaries, "syntax": syntax},
                                ensure_ascii=True, indent=2)+"\n", encoding="utf8")
    print(json.dumps({"output": OUTPUT.relative_to(ROOT).as_posix(), "calls": len(calls),
                      "grammar_passed": all(x["parsed"] and x["unchanged"] for x in syntax)}))


if __name__ == "__main__":
    main()

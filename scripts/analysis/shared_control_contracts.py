"""Pinned clipboard and global-control bytes; no engine code is executed."""
from __future__ import annotations
import hashlib
import json
from native_batch_contracts import analyze
from recovery_A_20261116 import Image, ROOT, instruction_row


def main() -> None:
    image = Image(ROOT / "Yukikoi Melt/YukikoiMelt.exe",
                  "63de0e14cd8ce11ad2d68e7ed6076641ff5e5b73d822603cc52764009d812976")
    regions = [
        ("unicode_decode", 0x5FF280, 0xD0),
        ("legacy_utf8_stride", 0x5FF350, 0x70),
        ("language_query_all_branches", 0x5A1F70, 0x70),
        ("fes_language_ui", 0x6DCA99, 0x44),
        ("fes_language_main", 0x6DCADD, 0x44),
        ("fes_language_subtitle", 0x6DCB21, 0x48),
        ("fes_scene_skip_read", 0x6DCD09, 0x41),
        ("fes_scene_skip_write", 0x6DDDE1, 0x45),
    ]
    names = [0x9130F4, 0x9130FC, 0x91310C, 0x913170, 0x9133B0, 0x913180]
    sources = [
        "simulator/crates/desktop/src/clipboard_native.rs",
        "simulator/crates/desktop/src/fes_settings.rs",
        "simulator/crates/desktop/src/language_native.rs",
        "simulator/crates/android/src/lib.rs",
        "simulator/android/app/src/main/cpp/bridge.c",
        "simulator/android/app/src/main/java/org/catsystem2/player/PlayerActivity.java",
    ]
    report = {
        "schema": "cs2-shared-control-contracts/v1",
        "status": "static-wrapper-and-field-identity-not-executed-equivalence",
        "slots": [10, 1039, 1077, 1078, 1079],
        "wrappers": analyze([10, 1039, 1077, 1078, 1079], 220)["binaries"],
        "helper_pe": {"path": image.path.relative_to(ROOT).as_posix(), "sha256": image.digest},
        "regions": [{"role": name, "va": va, "instructions": [
            instruction_row(ins, image.base) for ins in image.insns(va - image.base, size)
        ]} for name, va, size in regions],
        "field_names": [{"va": va, "bytes": image.get(va - image.base, 64)
                         .split(b"\0", 1)[0].hex()} for va in names],
        "source_sha256": {name: hashlib.sha256((ROOT / name).read_bytes()).hexdigest()
                          for name in sources},
        "compiled": False, "tests_run": False, "device_modified": False,
        "original_code_executed": False,
    }
    output = ROOT / "reports/shared_control_contracts_20261117.json"
    output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"report": output.relative_to(ROOT).as_posix(),
                      "original_code_executed": False}))


if __name__ == "__main__":
    main()

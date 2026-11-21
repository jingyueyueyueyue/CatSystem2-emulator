"""Pinned posted-window wrappers and their actual receivers, bytes only."""
from __future__ import annotations
import hashlib
import json
import struct
from native_batch_contracts import analyze
from recovery_A_20261116 import Image, ROOT, instruction_row


def main() -> None:
    image = Image(ROOT / "Yukikoi Melt/YukikoiMelt.exe",
                  "63de0e14cd8ce11ad2d68e7ed6076641ff5e5b73d822603cc52764009d812976")
    regions = [
        ("window_message_switch", 0x492AD5, 0x20),
        ("window_menu_reload", 0x493507, 0xBD),
        ("posted_commands", 0x49360F, 0x4E),
        ("command_switch", 0x49145B, 0x1D),
        ("toggle_command", 0x4914D4, 0x29),
        ("enter_fullscreen", 0x491AC0, 0xDE),
        ("leave_fullscreen", 0x491BA0, 0xB7),
        ("minimize_command", 0x49165E, 0x18),
        ("close_command", 0x49142B, 0x30),
        ("host_task_message", 0x533FF0, 0x86),
        ("settings_property_sheet", 0x483880, 0x19F),
    ]
    source_names = [
        "simulator/crates/desktop/src/window_control.rs",
        "simulator/crates/desktop/src/lib.rs",
        "simulator/crates/desktop/src/portable_host.rs",
        "simulator/crates/desktop/src/host_adapter.rs",
        "simulator/crates/desktop/src/native_tasks.rs",
        "simulator/crates/desktop/src/fes_settings.rs",
        "simulator/crates/android/src/lib.rs",
        "simulator/android/app/src/main/cpp/bridge.c",
        "simulator/android/app/src/main/java/org/catsystem2/player/PlayerActivity.java",
    ]
    selectors = []
    for command in [0x9C4C, 0x9C55]:
        selector = image.get(0x491A60 - image.base + command - 0x9C48, 1)[0]
        target = struct.unpack("<I", image.get(0x491A00 - image.base + selector * 4, 4))[0]
        selectors.append({"command": command, "selector": selector, "target_va": target})
    report = {
        "schema": "cs2-window-control-contracts/v1",
        "status": "static-command-chain-and-source-not-executed-equivalence",
        "slots_implemented_in_this_batch": [845, 846, 847],
        "slots_still_requiring_settings_and_menu_receivers": [457, 628],
        "wrappers": analyze([457, 628, 845, 846, 847], 160)["binaries"],
        "pe": {"path": image.path.relative_to(ROOT).as_posix(), "sha256": image.digest},
        "command_selectors": selectors,
        "imports": [image.imports[va - image.base] for va in [0x8E24B0, 0x8E2068, 0x8E2060]],
        "regions": [{"role": role, "va": va, "instructions": [
            instruction_row(ins, image.base) for ins in image.insns(va - image.base, size)
        ]} for role, va, size in regions],
        "source_sha256": {name: hashlib.sha256((ROOT / name).read_bytes()).hexdigest()
                          for name in source_names},
        "known_limits": [
            "Original fullscreen lock and monitor mode/palette/UI refresh side effects still need full mapping.",
            "Android uses immersive system bars and moveTaskToBack as host operations.",
            "Not type checked, linked, built or executed on a host or device.",
        ],
        "compiler_invoked": False, "tests_run": False, "original_code_executed": False,
        "device_modified": False,
    }
    output = ROOT / "reports/window_control_contracts_20261117.json"
    output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"report": output.relative_to(ROOT).as_posix(),
                      "implemented_slots": [845, 846, 847], "original_code_executed": False}))


if __name__ == "__main__":
    main()

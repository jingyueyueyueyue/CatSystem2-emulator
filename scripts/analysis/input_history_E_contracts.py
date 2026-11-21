"""Read-only native device-history producer evidence; optional source grammar.

No engine functions, Windows messages, VM, tests, compiler or game execute.
"""
from __future__ import annotations

import argparse
import ast
import hashlib
import json
from pathlib import Path
import subprocess

from recovery_A_20261116 import ROOT, Image
from recovery_A_receiver_contracts_20261116 import block


def evidence() -> None:
    image = Image(ROOT / "Yukikoi Melt/YukikoiMelt.exe",
                  "63de0e14cd8ce11ad2d68e7ed6076641ff5e5b73d822603cc52764009d812976")
    cases = {}
    for message in [0x6, 0x100, 0x101, 0x102, 0x104, 0x105,
                    *range(0x201, 0x20F), 0x240]:
        if message <= 0x102:
            selector = image.get(0x26B08C + message - 6, 1)[0]
            target = image.u32(0x26B078 + selector * 4) - image.base
        elif 0x201 <= message <= 0x240:
            selector = image.get(0x26B1BC + message - 0x201, 1)[0]
            target = image.u32(0x26B18C + selector * 4) - image.base
        else:
            selector, target = None, 0x26AFD9
        cases[hex(message)] = {"selector": selector, "target_rva": hex(target)}
    ranges = [
        ("zero_initialization", 0x26991D, 0x7A),
        ("device_reset", 0x269CD0, 0xE5),
        ("raw_history_getter", 0x26A3C0, 0x52),
        ("native_wrapper", 0x268F70, 0xE),
        ("message_dispatch_keyboard", 0x26A590, 0x21D),
        ("message_dispatch_mouse", 0x26A8E5, 0x333),
        ("touch_source_and_suppression", 0x26AC1E, 0x3BB),
        ("input_complete_history_copy", 0x13EC4F, 0x34),
        ("button_display_dispatch", 0x136B30, 0x23),
        ("button47_sends_input135", 0x136DCB, 0xEC),
        ("button48_sends_input135", 0x136EB7, 0x51),
    ]
    output = ROOT / "reports/input_history_E_contracts_20261117.json"
    output.write_text(json.dumps({
        "status": "static-producer-evidence-not-runtime-validation",
        "pe": "Yukikoi Melt/YukikoiMelt.exe", "sha256": image.digest,
        "engine_functions_executed": False,
        "receiver_command": 160,
        "layout": {"device_offset": "0x628", "bytes": 32, "bits": 256,
                   "index": "low8 virtual key, DWORD index=key>>5, bit=key&31",
                   "byte_view": "byte=key>>3, bit=key&7, little-endian DWORDs"},
        "message_cases": cases,
        "observations": [
            "WM_KEYDOWN100 BTS and WM_KEYUP101 BTR run even outside normal key callback gates",
            "WM_SYSKEYDOWN104 and UP105 do not update the bitset",
            "WM_LBUTTON201/202 uses bit1 unless device+618 touch suppression is active",
            "actual selector maps WM_MBUTTON207/208 to bit2 and WM_RBUTTON204/205 to bit3",
            "XBUTTON20b/20c checks wParam highword20/40 and updatesbit4/5; standard1/2 is not substituted",
            "doubleclick203/206/209/20d cases use default handling and do not write this bitset",
            "focus/wheel/pointer changes are not a bitset reset",
            "constructor and explicit device reset initialize all32bytes to zero",
            "Windows provider observes queued MSG bytes before winit translation; InputState is never read",
            "WM_TOUCH makes provider unavailable until its exact suppression/reset lifetime is bound",
        ],
        "snippets": {name: block(image, start, size) for name, start, size in ranges},
    }, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"report": output.relative_to(ROOT).as_posix(), "execution": False}))


def syntax() -> None:
    rustfmt = Path.home() / ".cargo/bin/rustfmt.exe"
    rows = []
    for relative in [
        "simulator/crates/desktop/src/input_history_native.rs",
        "simulator/crates/desktop/src/lib.rs",
        "simulator/crates/desktop/src/message_platform.rs",
        "simulator/crates/desktop/src/native_object_state.rs",
        "simulator/crates/runtime/src/object_state.rs",
    ]:
        raw = (ROOT / relative).read_bytes()
        result = subprocess.run([str(rustfmt), "--edition", "2024", "--emit", "stdout",
                                 "--config", "skip_children=true"], input=raw,
                                stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, check=False)
        rows.append({"source": relative, "sha256": hashlib.sha256(raw).hexdigest(),
                     "exit_code": result.returncode,
                     "diagnostics": result.stderr.decode("utf-8", "replace")})
    ast.parse(Path(__file__).read_text(encoding="utf-8"))
    output = ROOT / "archive/2026-11/20261117_input_history_E_source_syntax.json"
    output.write_text(json.dumps({"status": "grammar-only-not-types-linking-or-runtime",
                                  "compiled": False, "tests_run": False,
                                  "engine_vm_game_apk_executed": False, "sources": rows}, indent=2)
                      + "\n", encoding="utf-8")
    failures = sum(row["exit_code"] != 0 for row in rows)
    print(json.dumps({"report": output.relative_to(ROOT).as_posix(), "failures": failures}))
    if failures:
        raise SystemExit(1)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--syntax", action="store_true")
    args = parser.parse_args()
    syntax() if args.syntax else evidence()

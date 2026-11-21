"""Read-only, SHA-bound message/body and input receiver evidence.

Disassembles bytes only; no recovered function, VM, game or compiler runs.
"""
from __future__ import annotations

import argparse
import ast
import hashlib
import json
from pathlib import Path
import subprocess

from recovery_A_20261116 import ROOT, Image
from recovery_A_receiver_contracts_20261116 import block, rtti_vtable


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--syntax", action="store_true",
                        help="Parse source only instead of collecting PE evidence")
    if parser.parse_args().syntax:
        syntax()
        return
    image = Image(
        ROOT / "Yukikoi Melt/YukikoiMelt.exe",
        "63de0e14cd8ce11ad2d68e7ed6076641ff5e5b73d822603cc52764009d812976",
    )
    classes = rtti_vtable(image, "kcTaskInput")
    if len(classes) != 1 or classes[0]["command_handler_RVA"] != 0x13EEF0:
        raise ValueError("Input receiver identity changed")
    cases = {}
    for code in (130, 135, 136, 143, 149, 150, 159, 160):
        selector = image.get(0x13F84C + code - 3, 1)[0]
        target = image.u32(0x13F7B0 + selector * 4) - image.base
        cases[str(code)] = block(image, target, 224)
    report = {
        "status": "static-instruction-evidence-not-semantic-validation",
        "pe": "Yukikoi Melt/YukikoiMelt.exe",
        "sha256": image.digest,
        "image_base": image.base,
        "original_code_executed": False,
        "rtti": classes,
        "input_commands": cases,
        "ordinary1009": {
            "entry_rva": 0x197A30,
            "classification": "image-encode-and-write-not-text-or-KX-resource-load",
            "extension_literals": {"0x4ee150": "bmp", "0x4ef9c4": "png"},
            "snippets": {
                name: block(image, start, size)
                for name, start, size in (
                    ("wrapper", 0x197A30, 0x101),
                    ("wide_path_expansion", 0x26C3D0, 0x330),
                    ("wide_extension_lookup", 0x200560, 0x54),
                    ("encode_and_file_write", 0x1E2940, 0x2E9),
                    ("default_source_channel_conversion", 0x1E5BE0, 0x4F),
                    ("opaque_source_conversion", 0x1E3BC0, 0x14F),
                )
            },
        },
        "snippets": {
            name: block(image, start, size)
            for name, start, size in (
                ("input_update", 0x13E5F0, 0x693),
                ("device_history_wrapper", 0x268F70, 0x10),
                ("device_history_copy", 0x26A3C0, 0x52),
                ("device_finish", 0x268D00, 0x27),
                ("device_commit", 0x269400, 0x1C),
                ("cp932_export", 0x1EB960, 0x160),
                ("unicode_export", 0x1F1A60, 0x180),
                ("unicode_export_conversion", 0x1FF010, 0xCE),
            )
        },
    }
    output = ROOT / "reports/message_input_E_contracts_20261117.json"
    output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"report": output.relative_to(ROOT).as_posix(),
                      "commands": list(cases), "execution": False}))


def syntax() -> None:
    rustfmt = Path.home() / ".cargo/bin/rustfmt.exe"
    sources = [
        "simulator/crates/desktop/src/lib.rs",
        "simulator/crates/desktop/src/message_native.rs",
        "simulator/crates/desktop/src/message_platform.rs",
        "simulator/crates/desktop/src/message_backend.rs",
        "simulator/crates/runtime/src/message_object.rs",
    ]
    rows = []
    for relative in sources:
        raw = (ROOT / relative).read_bytes()
        result = subprocess.run(
            [str(rustfmt), "--edition", "2024", "--emit", "stdout",
             "--config", "skip_children=true", str(ROOT / relative)],
            stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
            check=False,
        )
        rows.append({"source": relative, "sha256": hashlib.sha256(raw).hexdigest(),
                     "exit_code": result.returncode,
                     "diagnostics": result.stderr.decode("utf-8", "replace")})
    ast.parse(Path(__file__).read_text(encoding="utf-8"))
    output = ROOT / "archive/2026-11/20261117_message_input_E_source_syntax.json"
    output.write_text(json.dumps({
        "status": "source-syntax-only-not-types-linking-or-runtime-validation",
        "compiler_invoked": False, "tests_run": False,
        "original_engine_functions_vm_game_apk_executed": False,
        "sources": rows,
    }, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"report": output.relative_to(ROOT).as_posix(),
                      "source_count": len(rows),
                      "failures": sum(row["exit_code"] != 0 for row in rows)}))
    if any(row["exit_code"] != 0 for row in rows):
        raise SystemExit(1)


if __name__ == "__main__":
    main()

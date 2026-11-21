"""Pinned byte evidence for list registration/ANM and root clock control.

Reads PE bytes and source only. It does not compile, run a VM, or execute any
original function. Wrapper and receiver command numbers remain separate.
"""
from __future__ import annotations
import hashlib
import json
from native_batch_contracts import analyze
from recovery_A_20261116 import Image, ROOT, instruction_row
from recovery_A_receiver_contracts_20261116 import rtti_vtable


def region(image: Image, role: str, rva: int, length: int) -> dict:
    raw = image.get(rva, length)
    return {"role": role, "rva": rva, "length": len(raw),
            "sha256": hashlib.sha256(raw).hexdigest(),
            "instructions": [instruction_row(ins, image.base) for ins in image.insns(rva, length)]}


def main() -> None:
    images = [
        Image(ROOT / "cs2_full_v401/system/cs2_open.exe",
              "0e1dde51e86e9152d8e1d27b82efb15916cb9f82d23dd30990212fafd96c8b9f"),
        Image(ROOT / "Yukikoi Melt/YukikoiMelt.exe",
              "63de0e14cd8ce11ad2d68e7ed6076641ff5e5b73d822603cc52764009d812976"),
    ]
    roots = []
    for image, command, byte_table, jump_table, expected in [
        (images[0], 38, 0xBAF98, 0xBAF64, 0xBAF30),
        (images[1], 40, 0x136668, 0x13663C, 0x13662C),
    ]:
        classes = rtti_vtable(image, "kcTaskTop")
        selector = image.get(byte_table + command - 1, 1)[0]
        target = image.u32(jump_table + selector * 4) - image.base
        if len(classes) != 1 or target != expected:
            raise ValueError("Root clock receiver identity changed")
        roots.append({"path": image.path.relative_to(ROOT).as_posix(), "sha256": image.digest,
                      "class": classes[0], "command": command, "selector": selector,
                      "writer": region(image, "root-clock-request", target, 6 if command == 38 else 10)})
    current = images[1]
    sources = [
        "simulator/crates/script/src/lib.rs", "simulator/crates/desktop/src/list_native.rs",
        "simulator/crates/desktop/src/image_manager.rs", "simulator/crates/desktop/src/top_task_native.rs",
        "simulator/crates/desktop/src/task_commands.rs", "simulator/crates/desktop/src/lib.rs",
        "simulator/crates/desktop/src/data_services.rs",
        "simulator/crates/desktop/src/root_clock_profile.rs",
    ]
    output = ROOT / "reports/residual_task_contracts_20261117.json"
    payload = {
        "schema": "cs2-residual-task-contracts/v1", "slots_added": [577, 622, 623, 624],
        "wrappers": analyze([577, 622, 623, 624], 420), "root_receivers": roots,
        "current_regions": [region(current, *row) for row in [
            ("list-register-switch", 0x14CD8F, 0x15),
            ("list-register-sources", 0x14CDBF, 0x116),
            ("image-manager-anm-alias-lookup", 0x1CB7B0, 0x104),
            ("root-request-getter", 0x136530, 7),
            ("post-update-sample", 0x133F72, 0x1E),
            ("manager-sample-getter", 0xF4B30, 4),
            ("platform-consumes-sample", 0x133848, 0x56),
            ("timer-hold-branch", 0x133CAA, 0x25),
        ]],
        "source_sha256": {name: hashlib.sha256((ROOT / name).read_bytes()).hexdigest() for name in sources},
        "limits": [
            "Initial root anchors are two receivers; root_clock_profiles_20261117.json separately proves the twelve-profile extension.",
            "622/623 read the real 52-byte parameter storage even for observed declared pop-count32.",
            "The shared list/image decoding and composition retain their separately documented boundaries.",
            "577 keeps platform frames and dispatch active; the next frame holds game time. No audio pause is inferred.",
            "Native688/689 full-state transaction must preserve both requested and sampled root clock DWORDs.",
            "No type checking, build, behavior tests or device acceptance performed.",
        ],
        "original_code_executed": False, "compiler_invoked": False, "tests_run": False,
    }
    output.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"report": output.relative_to(ROOT).as_posix(), "slots_added": payload["slots_added"]}))


if __name__ == "__main__":
    main()

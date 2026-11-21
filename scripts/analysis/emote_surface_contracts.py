"""Record the fixed NEKO surface owner, clock and pixel kernels, without execution."""
from __future__ import annotations

import hashlib
import json
import struct
import subprocess
import sys
from pathlib import Path

from recovery_A_20261116 import ROOT, Image
from root_clock_profiles import snippet

PE = "c71c5b85e6eca660474f156d96b0021a53a5f05690dd077d7861d72ff5b11adc"


def source_syntax(sources: list[str]) -> None:
    """Read changed Rust grammar from stdin; never write or type-check Rust."""
    rows = []
    for name in sources:
        raw = (ROOT / name).read_bytes()
        run = subprocess.run(
            [str(Path.home() / ".cargo/bin/rustfmt.exe"), "--edition", "2024",
             "--config", "skip_children=true", "--emit", "stdout"],
            input=raw, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, check=False,
        )
        rows.append({"path": name, "sha256": hashlib.sha256(raw).hexdigest(),
                     "syntax_exit": run.returncode,
                     "diagnostic": run.stderr.decode("utf-8", errors="replace")})
    report = {"grammar_only": True, "compiled": False, "type_checked": False,
              "tests_run": False, "sources": rows,
              "script_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest()}
    (ROOT / "archive/2026-11/20261117_emote_surface_source_syntax.json").write_text(
        json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    if any(row["syntax_exit"] for row in rows):
        raise ValueError("Rust grammar read failed; see archived diagnostics")


def main() -> None:
    prior = json.loads((ROOT / "reports/recovery_A_20261116_emote.json").read_text(encoding="utf-8"))
    image = Image(ROOT / prior["binary_path"], PE)
    ranges = {
        "native931_wrapper": (0x24CCA0, 0x30),
        "command545_receiver": (0x239120, 0x28),
        "player_pending_setter": (0x1743C0, 0x18),
        "config_constructor_default": (0x19F550, 0x20),
        "fade_config_consumer": (0x1A0538, 0x15),
        "manager_duration_input": (0x17D1C5, 0x2E),
        "manager_and_player_duration": (0x17A6AB, 0x0A),
        "player_duration_input": (0x17A927, 0x1C),
        "player_duration_store": (0x170E7A, 0x0F),
        "manager_clock": (0x17ADD5, 0x80),
        "manager_held_return": (0x17AECF, 0x1A),
        "manager_pending_gate": (0x17AEB0, 0xC7),
        "manager_disable_controller": (0x17B0E0, 0x79),
        "manager_restore_controller": (0x17B193, 0x96),
        "manager_begin_and_rearm": (0x17B7A3, 0x36),
        "primary_empty_endpoint": (0x172AA6, 0x2B),
        "primary_chain_fade": (0x172B89, 0x48),
        "resource_replace_capture_request": (0x171A8D, 0x32),
        "resource_replace_driver_release": (0x17592E, 0x43),
        "resource_replace_duration_reset": (0x1725AA, 0x0E),
        "manager_resource_selection": (0x17A9BA, 0x68),
        "manager_surface_before_new_update": (0x17AFD5, 0x17),
        "surface_fraction_select": (0x19B70D, 0xC6),
        "rgba_integer_blend": (0x19C6B7, 0x126),
        "alpha_only_blend": (0x19C841, 0x9E),
        "transparent_rgb_fill": (0x19C949, 0x97),
        "float_to_integer": (0x3A0320, 0x18B),
        "shared_integer_parser": (0x2A4DB0, 0xD4),
    }
    evidence = {name: snippet(image, start, size) for name, (start, size) in ranges.items()}
    if struct.unpack("<f", image.get(0x5E4D98, 4))[0] != 256.0:
        raise ValueError("surface blend multiplier changed")
    required = {
        "player_pending_setter": [("mov", "dword ptr [ecx + 0xa0], eax"),
                                  ("mov", "dword ptr [ecx + 0x9c], 1")],
        "manager_clock": [("sub", "eax, dword ptr [edi + 0x30]"),
                          ("cmova", "eax, ecx"), ("divss", "xmm1, xmm0")],
        "primary_chain_fade": [("mov", "dword ptr [ebx + 0x9c], 1"),
                               ("mov", "dword ptr [ebx + 0xa0], eax")],
        "resource_replace_capture_request": [("mov", "dword ptr [esi + 0x9c], 1")],
        "resource_replace_duration_reset": [("mov", "eax, dword ptr [ebx + 0xa4]"),
                                            ("mov", "dword ptr [ebx + 0xa0], eax")],
        "rgba_integer_blend": [("mov", "ebx, 0x100"), ("shr", "ecx, 8")],
        "alpha_only_blend": [("imul", "eax, dword ptr [ebp - 4]"),
                             ("mov", "byte ptr [edx - 1], al")],
    }
    for name, operations in required.items():
        found = {(op["mnemonic"], op["operands"]) for op in evidence[name]["instructions"]}
        if not set(operations).issubset(found):
            raise ValueError(f"fixed-PE source contract changed: {name}")
    sources = ["simulator/crates/runtime/src/" + name for name in (
        "emote_surface.rs", "emote_cpu.rs", "emote.rs", "emote_dispatch.rs")]
    sources += ["simulator/crates/desktop/src/emote_native.rs",
                "simulator/crates/formats/src/config.rs"]
    if "--syntax" in sys.argv:
        source_syntax(sources[:2])
    result = {
        "schema": "cs2-emote-surface-contracts/v1", "pe_sha256": PE,
        "binary_path": prior["binary_path"], "evidence": evidence,
        "source_sha256": {name: hashlib.sha256((ROOT / name).read_bytes()).hexdigest() for name in sources},
        "implemented_consumers": [
            "native931/owned545 -> CpuTask.surface pending -> actual PSB frame -> owned old/new RGBA -> Canvas submission",
            "same-resource same-profile primary Fade chain -> last Fade dimension duration -> same surface owner",
            "different-resource import -> capture owned old RGBA -> load actual new PSB and owner -> constructor duration -> shared pixel composition -> release old player",
            "active manager -> raw DWORD/f32 time -> no driver/bank/clock/random/PCM update while held",
            "first following update reanchors start; retirement continues the same host tick",
            "transparent RGB prefill; 256-level RGBA interpolation or old-alpha-only decay",
            "task tint is applied after composition, once; release drops all owned surfaces",
            "m2view #fade and native931 share the real duration field; existing shared integer parser",
        ],
        "remaining": [
            "nonzero same-resource secondary/profile transition retains an explicit guard",
            "failed resource reopen preserves previous live CPU owner; original destructive-failure side effects remain incomplete",
            "native931 before a loaded PSB/live owner remains a diagnostic",
            "other PE profiles and driver kinds", "complete object-graph save/restore",
            "original-engine function equivalence and all-game acceptance",
        ],
        "ordinary_routes_added": 0, "full_function_equivalence_added": 0,
        "compiled": False, "type_checked": False, "tests_run": False,
        "original_code_executed": False,
    }
    path = ROOT / "reports/emote_surface_contracts_20261117.json"
    path.write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"fixed_pe_regions": len(evidence), "source_files": len(sources),
                      "ordinary_routes_added": 0, "report": path.relative_to(ROOT).as_posix()}))


if __name__ == "__main__":
    main()

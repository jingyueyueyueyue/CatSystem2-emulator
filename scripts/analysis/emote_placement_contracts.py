"""Record exact NEKOPARA player-placement and final-quad source contracts.

Only immutable PE bytes and Rust source text are read. No compiler, VM,
original executable/DLL, game, network submission or device is invoked.
"""
from __future__ import annotations

import hashlib
import json
import struct

from recovery_A_20261116 import ROOT, Image, classify_tables, registrations
from root_clock_profiles import snippet

PE = "c71c5b85e6eca660474f156d96b0021a53a5f05690dd077d7861d72ff5b11adc"
DRIVER = "85eb459e94e74ab608f649eb7fb440f2526e90aaa1af7276ed01dc10018903ce"


def main() -> None:
    earlier = json.loads((ROOT / "reports/recovery_A_20261116_emote.json").read_text(encoding="utf-8"))
    image = Image(ROOT / earlier["binary_path"], PE)
    _, registered = registrations(image)
    tables = [row for row in classify_tables(image, registered) if row["table_hint"] == "ordinary"]
    if len(tables) != 1:
        raise ValueError("ambiguous registered ordinary table")
    table = tables[0]
    wrappers = []
    for slot, command in [(84, 51), (93, 55), (101, 66)]:
        value = table["slots"][slot]
        entry = value["entry_rva"] if isinstance(value, dict) else value
        row = snippet(image, entry, 0x90)
        operations = row["instructions"]
        if not any(op["mnemonic"] == "push" and int(op["operands"], 0) == command
                   for op in operations if op["operands"].startswith("0x") or op["operands"].isdigit()):
            raise ValueError("wrapper no longer carries the expected command")
        wrappers.append({"slot": slot, "command": command, "registered_argument_bytes": 20,
                         "table_rva": table["table_rva"], **row})
    targets = {}
    for command, target, setter in [(51, 0x2385FF, 0x1743E0), (55, 0x2386E9, 0x174450), (66, 0x238695, 0x174420)]:
        selected = image.u32(0x2393E8 + 4 * image.get(0x239414 + command - 3, 1)[0]) - image.base
        if selected != target:
            raise ValueError("M2 command switch changed")
        branch = snippet(image, target, 0x55)
        if not any(op.mnemonic == "call" and image.direct(op) == setter
                   for op in image.insns(target, 0x55)):
            raise ValueError("M2 command lost the recovered player setter")
        targets[command] = {"branch": branch, "setter": snippet(image, setter, 0x30)}
    update = snippet(image, 0x174088, 0xEE)
    instructions = [(row["mnemonic"], row["operands"]) for row in update["instructions"]]
    for required in [("subss", "xmm2, dword ptr [esi + 0xc4]"),
                     ("addss", "xmm1, dword ptr [esi + 0xac]"),
                     ("addss", "xmm1, dword ptr [esi + 0xb0]"),
                     ("mulss", "xmm0, dword ptr [esi + 0xc4]"),
                     ("call", "dword ptr [eax + 0x54]"),
                     ("call", "dword ptr [eax + 0x5c]"),
                     ("call", "dword ptr [eax + 0x64]")]:
        if required not in instructions:
            raise ValueError("M2 placement arithmetic changed")
    sources = ["simulator/crates/desktop/src/" + name for name in (
        "emote_geometry.rs", "emote_native.rs", "task_commands.rs", "native_tasks.rs", "lib.rs")]
    sources.extend("simulator/crates/runtime/src/" + name for name in (
        "emote_driver_transform.rs", "emote.rs", "emote_cpu.rs", "emote_timeline.rs"))
    driver = Image((ROOT / earlier["binary_path"]).with_name("emotedriver.dll"), DRIVER)
    if driver.u32(0x7C9F8 + 0x64) != driver.base + 0x2820:
        raise ValueError("concrete player rotation vtable changed")
    if struct.unpack("<f", driver.get(0x7DFF4, 4))[0] != struct.unpack("<f", bytes.fromhex("db0fc940"))[0]:
        raise ValueError("driver f32 TAU changed")
    if struct.unpack("<f", driver.get(0x7E020, 4))[0] != 360.0:
        raise ValueError("driver degree conversion changed")
    driver_overrides = snippet(driver, 0x3E9EB, 0xD8)
    for operation in [("addss", "xmm0, dword ptr [esi + 0x270]"),
                      ("mulss", "xmm0, dword ptr [esi + 0x274]"),
                      ("addss", "xmm0, dword ptr [esi + 0x264]")]:
        if not any((op["mnemonic"], op["operands"]) == operation for op in driver_overrides["instructions"]):
            raise ValueError("driver overrides lost their local-property consumers")
    report = {
        "schema": "cs2-emote-placement-contracts/v2", "pe_sha256": PE,
        "path": earlier["binary_path"], "wrappers": wrappers, "receivers": targets,
        "profile_field_import": snippet(image, 0x174251, 0x49),
        "player_driver_update": update, "final_quad": snippet(image, 0x238090, 0x190),
        "unloaded_command_tail": snippet(image, 0x2393A0, 4),
        "driver": {
            "path": driver.path.relative_to(ROOT).as_posix(), "sha256": DRIVER,
            "concrete_player_vtable_rva": 0x7C9F8, "rotation_slot": 0x64,
            "rotation_method": snippet(driver, 0x2820, 0x69),
            "immediate_normalization": snippet(driver, 0xCB9C, 0x6D),
            "root_rotation_producer": snippet(driver, 0x1BD5F, 0xF8),
            "local_property_consumer": driver_overrides,
            "tau_f32_bits": "40c90fdb", "root_angle_unit": "degrees",
        },
        "source_sha256": {name: hashlib.sha256((ROOT / name).read_bytes()).hexdigest() for name in sources},
        "facts": {
            "position_xy": "(requested_position + profile_position) + scaleoffset * (1 - requested_scale_x)",
            "driver_uniform_scale": "profile_scale_x * requested_scale_x",
            "retained_but_unused_in_kind1_update": ["requested_scale_y", "profile_scale_y"],
            "ignored_input": "position/scale third float and wrapper selector",
            "rotation_input": "only the third float of command55; wrapper selector and first two floats ignored",
            "root_property_application": "sampled root coord += driver position; root angle += normalized driver degrees; root zx/zy *= driver scale, before transformOrder",
            "previous_placement_implementation": "parent-matrix implementation superseded; it incorrectly transformed the root's local coordinates",
            "arithmetic": "SSE f32 multiply/add order, widened only for the CPU evaluator",
            "unloaded_receiver": "consumes command without reading pointer or mutating placement",
            "final_quad": "full logical viewport; task placement belongs to the model driver",
        },
        "remaining": ["old-model fade/replacement", "other driver types", "alternate root coordinate modes",
                      "other PE profiles", "nondefault host display context", "whole engine/game equivalence"],
        "ordinary_routes_added": 0, "full_function_equivalence_added": 0,
        "compiled": False, "tests_run": False, "original_code_executed": False,
    }
    destination = ROOT / "reports/emote_placement_contracts_20261117.json"
    destination.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"wrappers": len(wrappers), "receivers": len(targets), "routes_added": 0,
                      "report": destination.relative_to(ROOT).as_posix()}))


if __name__ == "__main__":
    main()

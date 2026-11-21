"""Recover sound creation-context rejection/forwarding from immutable PE bytes.

Native66 sends the caller context after construction. kcTaskSound does not
interpret it as an audio option packet: both context commands reach the sound
default and then the base default. This describes only that input contract.
"""
from __future__ import annotations

import json
import re

from recovery_A_20261116 import ROOT, Image, recover
from recovery_A_receiver_contracts_20261116 import rtti_vtable
from root_clock_profiles import snippet


def switch(image: Image, entry: int) -> dict:
    prefix = image.insns(entry, 0x100)
    load = next((x for x in prefix if x.mnemonic == "movzx" and re.fullmatch(
        r"e[a-z]{2}, byte ptr \[e[a-z]{2} \+ 0x[0-9a-f]+\]", x.op_str)), None)
    jump = next((x for x in prefix if x.mnemonic == "jmp" and re.fullmatch(
        r"dword ptr \[e[a-z]{2}\*4 \+ 0x[0-9a-f]+\]", x.op_str)), None)
    if load is None or jump is None:
        raise ValueError("unrecognized receiver switch")
    before = [x for x in prefix if x.address < load.address]
    lower = next((x for x in reversed(before) if x.mnemonic == "add"
                  and re.fullmatch(r"e[a-z]{2}, -(?:0x[0-9a-f]+|\d+)", x.op_str)), None)
    bound = next((x for x in reversed(before) if x.mnemonic == "ja"), None)
    if lower is None or bound is None:
        raise ValueError("unrecognized receiver switch bound")
    first = -int(lower.op_str.split(", ")[1], 0)
    byte_table = int(re.search(r"0x[0-9a-f]+", load.op_str)[0], 0) - image.base
    jump_table = int(re.search(r"0x[0-9a-f]+", jump.op_str)[0], 0) - image.base
    default = image.direct(bound)
    selected = {}
    for command in (25, 26):
        selector = image.get(byte_table + command - first, 1)[0]
        selected[command] = image.u32(jump_table + selector * 4) - image.base
    return {"entry_rva": entry, "first_command": first, "byte_table_rva": byte_table,
            "jump_table_rva": jump_table, "default_rva": default,
            "context_command_targets": selected, "prefix": snippet(image, entry, jump.address - image.base + jump.size - entry)}


def contract(image: Image) -> dict:
    classes = rtti_vtable(image, "kcTaskSound")
    if len(classes) != 1:
        return {"accepted": False, "reason": "sound RTTI is missing or ambiguous"}
    identity = classes[0]
    sound = switch(image, identity["command_handler_RVA"])
    forward = recover(image, identity["first_8_virtual_method_RVAs"][6], set())
    direct = [x["target_rva"] for x in forward["calls"] if x["kind"] == "direct"]
    if len(direct) != 1 or not forward["all_observed_paths_terminated"] or forward["exit_edges"]:
        return {"accepted": False, "reason": "unrecognized sound/base forwarding", "forwarding": forward}
    base = switch(image, direct[0])
    sound_prefix = image.insns(sound["default_rva"], 24)
    sound_default = next((x for x in sound_prefix if x.mnemonic != "pop"), None)
    base_default = recover(image, base["default_rva"], set())
    # Base default may only restore its own registers/frame and return one.
    # It cannot access a context pointer or write a task/audio field.
    operations = base_default["disassembly"]
    base_pure = all(x["mnemonic"] in {"pop", "ret"}
                    or (x["mnemonic"] == "mov" and x["operands"] == "eax, 1")
                    or (x["mnemonic"] == "add" and x["operands"].startswith("esp, "))
                    for x in operations)
    accepted = bool(
        all(x == sound["default_rva"] for x in sound["context_command_targets"].values())
        and all(x == base["default_rva"] for x in base["context_command_targets"].values())
        and sound_default and (sound_default.mnemonic, sound_default.op_str) == ("mov", "eax, 1")
        and base_pure and any(x["mnemonic"] == "mov" and x["operands"] == "eax, 1" for x in operations)
        and not base_default["calls"] and not base_default["exit_edges"]
        and base_default["all_observed_paths_terminated"])
    return {"accepted": accepted, "reason": "context input is ignored by both real receivers" if accepted
            else "receiver contract not completely recognized", "sound_class": identity,
            "sound_switch": sound, "sound_default": snippet(image, sound["default_rva"], 24),
            "forwarding": forward, "base_switch": base, "base_default": base_default,
            "scope": "nonempty creation-context input only; not full sound-function equivalence"}


def main() -> None:
    evidence = json.loads((ROOT / "reports/recovery_A_20261116_native.json").read_text(encoding="utf-8"))
    rows = []
    for pe in evidence["native_evidence"]:
        image = Image(ROOT / pe["binary_path"], pe["sha256"])
        try:
            result = contract(image)
        except (ValueError, IndexError, TypeError) as error:
            result = {"accepted": False, "reason": str(error)}
        rows.append({"path": pe["binary_path"], "sha256": pe["sha256"], **result})
    report = ROOT / "reports/sound_context_contracts_20261117.json"
    report.write_text(json.dumps({"schema": "cs2-sound-context-contracts/v1", "binaries": rows,
                                  "original_code_executed": False, "compiler_invoked": False}, indent=2) + "\n", encoding="utf-8")
    module = ["//! Exact sound/base receiver evidence for inherited context input.",
              "//! Generated by scripts/analysis/sound_context_contracts.py; no copied engine code.",
              "pub(super) fn ignores_creation_context(sha256: &str) -> bool {", "    matches!(sha256,"]
    accepted = [row for row in rows if row["accepted"]]
    for n, row in enumerate(accepted):
        module.append(('        ' if n == 0 else '        | ') + '"' + row["sha256"] + '"')
    module += ["    )", "}", ""]
    if not accepted:
        module = ["pub(super) fn ignores_creation_context(_: &str) -> bool { false }", ""]
    (ROOT / "simulator/crates/desktop/src/sound_context_profile.rs").write_text("\n".join(module), encoding="utf-8")
    print(json.dumps({"accepted": len(accepted), "unrecognized": [row["path"] for row in rows if not row["accepted"]]}))


if __name__ == "__main__":
    main()

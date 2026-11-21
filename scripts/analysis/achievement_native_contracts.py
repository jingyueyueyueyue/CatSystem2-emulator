"""Read the registered achievement family without loading any original code.

Only complete wrapper/receiver patterns authorize a local achievement profile.
This does not claim that an independent profile synchronizes a Steam account.
"""
from __future__ import annotations

import json
import re

from recovery_A_20261116 import Image, ROOT
from root_clock_profiles import snippet


def receiver_body(image: Image, entry: int, size: int = 0xE0) -> tuple[int, list, list]:
    """Follow only the two identified platform-presence thunks, not arbitrary jumps."""
    prefix = list(image.insns(entry, 35))
    gate = {(ins.mnemonic, ins.op_str) for ins in prefix}
    if ("mov", "ecx, dword ptr [ecx + 0x2004]") in gate and ("test", "ecx, ecx") in gate:
        jump = next((ins for ins in prefix if ins.mnemonic in {"jmp", "jne"}
                     and image.direct(ins) is not None and image.direct(ins) > entry + 35), None)
        if jump:
            target = image.direct(jump)
            return target, list(image.insns(target, size)), [snippet(image, entry, jump.address - image.base - entry + jump.size)]
    return entry, list(image.insns(entry, size)), []


def family(image: Image, table: int) -> dict:
    entries = {index: image.u32(table + index * 4) for index in range(1041, 1045)}
    if any(value is None or not image.code(value - image.base) for value in entries.values()):
        return {"accepted": False, "reason": "family absent from registered table"}
    entries = {index: value - image.base for index, value in entries.items()}
    wrappers = {index: list(image.insns(entry, 100)) for index, entry in entries.items()}
    result = {"entries": entries, "wrappers": {index: snippet(image, entry, 100)
                                                for index, entry in entries.items()}}
    unlock = wrappers[1042]
    end = next((n for n, ins in enumerate(unlock) if ins.mnemonic == "ret"), len(unlock))
    unlock = unlock[:end + 1]
    calls = [image.direct(ins) for ins in unlock if ins.mnemonic == "call"]
    global_load = next((ins for ins in unlock if ins.mnemonic == "mov"
                        and re.fullmatch(r"ecx, dword ptr \[0x[0-9a-f]+\]", ins.op_str)), None)
    if len(calls) != 2 or None in calls or global_load is None:
        return {**result, "accepted": False, "reason": "unrecognized unlock wrapper"}
    receiver, body, thunks = receiver_body(image, calls[1])
    required = {
        ("mov", "byte ptr [eax + 0x40], 1"),
        ("call", "dword ptr [eax + 0x1c]"),
        ("call", "dword ptr [eax + 0x28]"),
    }
    if not required <= {(ins.mnemonic, ins.op_str) for ins in body}:
        return {**result, "accepted": False, "reason": "unrecognized achievement consumer",
                "receiver": snippet(image, receiver, 0xE0)}
    companions = {}
    for index, expected in [(1041, None), (1043, 0x18), (1044, 0x54)]:
        wrapper = wrappers[index]
        stop = next((n for n, ins in enumerate(wrapper) if ins.mnemonic == "ret"), len(wrapper))
        wrapper = wrapper[:stop + 1]
        direct = [image.direct(ins) for ins in wrapper if ins.mnemonic == "call"]
        if not direct or direct[-1] is None:
            return {**result, "accepted": False, "reason": f"missing companion {index}"}
        target, data, companion_thunks = receiver_body(image, direct[-1], 0xC0 if index != 1041 else 0x110)
        stop = next((n for n, ins in enumerate(data) if ins.mnemonic == "ret"), len(data))
        data = data[:stop + 1]
        companions[index] = {"receiver_rva": target, "region": snippet(image, target, 0xC0), "thunks": companion_thunks}
        if index == 1041:
            imported = [image.import_target(ins) for ins in data if ins.mnemonic == "call"]
            if not any(row and row["name"] == "SteamAPI_Init" for row in imported):
                constructors = []
                for ins in data:
                    if ins.mnemonic != "call" or (callee := image.direct(ins)) is None:
                        continue
                    child = image.insns(callee, 0x110)
                    stop = next((n for n, call in enumerate(child) if call.mnemonic == "ret"), len(child))
                    child = child[:stop + 1]
                    if any((imported := image.import_target(call)) and imported["name"] == "SteamAPI_Init"
                           for call in child if call.mnemonic == "call"):
                        constructors.append({"call_rva": ins.address - image.base, "callee_rva": callee,
                                             "region": snippet(image, callee, 0x110)})
                if len(constructors) != 1:
                    return {**result, "accepted": False, "reason": "init import not established"}
                companions[index]["constructors"] = constructors
        elif index == 1043:
            if not any(ins.mnemonic == "mov" and ins.op_str == "al, byte ptr [eax + 0x40]" for ins in data):
                return {**result, "accepted": False, "reason": "query cache read absent"}
        elif not any(ins.mnemonic == "call" and ins.op_str == f"dword ptr [eax + {hex(expected)}]"
                     for ins in data):
            return {**result, "accepted": False, "reason": "reset host consumer absent"}
    return {**result, "accepted": True, "reason": "Steam init/cache/unlock/reset receiver family",
            "receiver": snippet(image, receiver, 0xE0), "receiver_thunks": thunks, "companions": companions,
            "format_helper_rva": calls[0], "state_global": global_load.op_str}


def main() -> None:
    document = json.loads((ROOT / "reports/recovery_A_20261116_native.json").read_text(encoding="utf-8"))
    rows = []
    for pe in document["native_evidence"]:
        image = Image(ROOT / pe["binary_path"], pe["sha256"])
        tables = sorted({row["table_rva"] for row in pe["focus"] if row["table_hint"] == "ordinary"})
        contracts = [family(image, table) for table in tables]
        rows.append({"path": pe["binary_path"], "sha256": pe["sha256"], "tables": contracts,
                     "accepted": bool(contracts) and all(row["accepted"] for row in contracts)})
    path = ROOT / "reports/achievement_native_contracts_20261117.json"
    path.write_text(json.dumps({"schema": "cs2-achievement-contracts/v1", "binaries": rows,
                                "original_code_executed": False, "compiler_invoked": False,
                                "equivalence": "local-profile-only; Steam-account synchronization absent"},
                               indent=2) + "\n", encoding="utf-8")
    module = ["//! Exact PE achievement receiver identities; independent local profile only.",
              "//! Generated by scripts/analysis/achievement_native_contracts.py.",
              "pub(super) fn recognized(sha256: &str) -> bool {", "    matches!(sha256,"]
    accepted = [row for row in rows if row["accepted"]]
    if accepted:
        module += [('        ' if n == 0 else '        | ') + '"' + row["sha256"] + '"'
                   for n, row in enumerate(accepted)]
        module += ["    )", "}", ""]
        (ROOT / "simulator/crates/desktop/src/achievement_profile.rs").write_text("\n".join(module), encoding="utf-8")
    print(json.dumps({"accepted": len(accepted), "unrecognized": [row["path"] for row in rows if not row["accepted"]]}))


if __name__ == "__main__":
    main()

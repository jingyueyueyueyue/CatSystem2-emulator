"""Recover exact-PE root clock command/writer profiles without execution.

The report keeps wrapper, RTTI, command selection and clock-consumer evidence
separate. A matching wrapper alone never authorizes a runtime profile.
"""
from __future__ import annotations

import hashlib
import json
import re
import struct

from recovery_A_20261116 import Image, ROOT, instruction_row, recover
from recovery_A_receiver_contracts_20261116 import rtti_vtable


def snippet(image: Image, start: int, size: int) -> dict:
    raw = image.get(start, size)
    return {"rva": start, "byte_length": len(raw), "sha256": hashlib.sha256(raw).hexdigest(),
            "bytes": raw.hex(), "instructions": [instruction_row(ins, image.base)
                                                    for ins in image.insns(start, size)]}


def byte_sites(image: Image, pattern: bytes) -> list[int]:
    return [section["rva"] + match.start() for section in image.sections if section["executable"]
            for match in re.finditer(re.escape(pattern), section["data"])]


def callers(image: Image, target: int) -> list[int]:
    sites = []
    for section in image.sections:
        if not section["executable"]:
            continue
        data = section["data"]
        for match in re.finditer(b"\xe8", data):
            offset = match.start()
            if offset + 5 <= len(data):
                site = section["rva"] + offset
                if site + 5 + struct.unpack_from("<i", data, offset + 1)[0] == target:
                    sites.append(site)
    return sites


def argument_register(image: Image, entry: int, stop: int, register: str, argument: int) -> bool:
    """Track entry-ESP offsets through the receiver's straight-line prefix."""
    stack = 0
    origin = {}
    for ins in image.insns(entry, stop - entry):
        if ins.mnemonic == "push":
            stack -= 4
        elif ins.mnemonic == "pop":
            stack += 4
        elif ins.mnemonic in {"sub", "add"} and ins.op_str.startswith("esp, "):
            amount = int(ins.op_str.split(", ")[1], 0)
            stack += amount if ins.mnemonic == "add" else -amount
        elif ins.mnemonic == "mov":
            found = re.fullmatch(r"(e[a-z]{2}), dword ptr \[esp \+ (0x[0-9a-f]+|\d+)\]", ins.op_str)
            if found:
                origin[found[1]] = stack + int(found[2], 0)
            elif re.fullmatch(r"e[a-z]{2}, e[a-z]{2}", ins.op_str):
                dest, source = ins.op_str.split(", ")
                origin[dest] = origin.get(source)
            elif re.match(r"e[a-z]{2},", ins.op_str):
                origin[ins.op_str.split(",")[0]] = None
    return origin.get(register) == argument


def timer_contract(image: Image, consumer: dict) -> dict:
    instructions = list(image.insns(consumer["call_rva"] + 5, 100))
    hold = next((ins for ins in instructions[:4] if ins.mnemonic == "or"
                 and ins.op_str.endswith(", dword ptr [esi + 0x50]")), None)
    if hold is None:
        return {"accepted": False, "reason": "missing platform hold producer"}
    register = hold.op_str.split(",")[0]
    pushed = next((n for n, ins in enumerate(instructions) if ins.mnemonic == "push" and ins.op_str == register), None)
    committed = next((n for n, ins in enumerate(instructions)
                      if ins.mnemonic == "mov" and ins.op_str == "dword ptr [esi + 0x28], eax"), None)
    if pushed is None or committed is None:
        return {"accepted": False, "reason": "missing timer input/output"}
    targets = [image.direct(ins) for ins in instructions[pushed:committed]
               if ins.mnemonic == "call" and image.direct(ins) is not None]
    if len(targets) != 2 or len(set(targets)) != 1:
        return {"accepted": False, "reason": "unrecognized timer invocation pair"}
    timer = targets[0]
    body = list(image.insns(timer, 0x150))
    gate = next((n for n, ins in enumerate(body) if ins.mnemonic == "cmp"
                 and ins.op_str == "dword ptr [esp + 0x1c], 0"), None)
    if gate is None:
        return {"accepted": False, "reason": "unrecognized timer hold condition"}
    tail = body[gate:gate + 18]
    branch = next((ins for ins in tail[:3] if ins.mnemonic in {"jne", "jnz"}), None)
    write = next((ins for ins in tail if
                  (ins.mnemonic == "mov" and ins.op_str == "dword ptr [esi + 0x14], ecx")
                  or (ins.mnemonic == "add" and ins.op_str == "dword ptr [esi + 0x14], eax")), None)
    target = image.direct(branch) if branch else None
    accepted = bool(branch and write and target == write.address - image.base + write.size)
    # Verify that the only stack parameter is still entry ESP+4 at the gate.
    stack = 0
    register_imports = {}
    cleanups = []
    for ins in body[:gate]:
        if ins.mnemonic == "push": stack -= 4
        elif ins.mnemonic == "pop": stack += 4
        elif ins.mnemonic == "sub" and ins.op_str.startswith("esp, "): stack -= int(ins.op_str.split(", ")[1], 0)
        elif ins.mnemonic == "add" and ins.op_str.startswith("esp, "): stack += int(ins.op_str.split(", ")[1], 0)
        elif ins.mnemonic == "mov" and (found := re.fullmatch(r"(e[a-z]{2}), dword ptr \[(0x[0-9a-f]+)\]", ins.op_str)):
            register_imports[found[1]] = image.imports.get(int(found[2], 16) - image.base, {}).get("name")
        elif ins.mnemonic == "call":
            direct = image.direct(ins)
            if direct is not None:
                recovered = recover(image, direct, set())
                returns = {ret["pop_bytes"] for ret in recovered["returns"]}
                if returns != {16} or recovered["exit_edges"] or not recovered["all_observed_paths_terminated"]:
                    return {"accepted": False, "reason": "unproven timer helper stack cleanup"}
                cleanup, proof = 16, {"target_rva": direct, "cfg": recovered}
            else:
                imported = image.import_target(ins)
                name = imported.get("name") if imported else register_imports.get(ins.op_str)
                if name not in {"Sleep", "GetTickCount", "timeGetTime"}:
                    return {"accepted": False, "reason": "unrecognized timer host import", "call": instruction_row(ins, image.base)}
                cleanup, proof = (4 if name == "Sleep" else 0), {"import": name}
            stack += cleanup
            cleanups.append({"site_rva": ins.address - image.base, "pop_bytes": cleanup, **proof})
    accepted = accepted and stack + 0x1C == 4
    return {"accepted": accepted, "timer_rva": timer, "hold_register": register,
            "hold_parameter_entry_esp_offset": stack + 0x1C,
            "call_stack_cleanups": cleanups,
            "gate": snippet(image, body[gate].address - image.base, 42),
            "prefix": snippet(image, timer, body[gate].address - image.base - timer)}


def contract(image: Image, wrappers: list[dict], classes: list[dict]) -> dict:
    commands = set()
    templates = [re.compile(re.escape(bytes.fromhex(prefix)) + b"(.)" + re.escape(bytes.fromhex(suffix)), re.DOTALL)
                 for prefix, suffix in [
                     ("8b4c24088d442408508b4424106a008b11ff306a006a", "6a00ff7104ff5210b802000000c21000"),
                     ("8b4c24088b018b40108d542408528b5424108b126a00528b51046a006a", "6a0052ffd0b802000000c21000"),
                 ]]
    for row in wrappers:
        found = next((found for template in templates if (found := template.match(image.get(row["rva"], 0x35)))), None)
        if not found:
            return {"accepted": False, "reason": "unrecognized native577 wrapper"}
        commands.add(found[1][0])
    if len(commands) != 1 or len(classes) != 1:
        return {"accepted": False, "reason": "ambiguous wrapper/root class"}
    command = next(iter(commands))
    handler = classes[0]["command_handler_RVA"]
    prefix = list(image.insns(handler, 0x100))
    table_load = next((ins for ins in prefix if ins.mnemonic == "movzx"
                       and re.fullmatch(r"eax, byte ptr \[eax \+ 0x[0-9a-f]+\]", ins.op_str)), None)
    table_jump = next((ins for ins in prefix if ins.mnemonic == "jmp"
                       and re.fullmatch(r"dword ptr \[eax\*4 \+ 0x[0-9a-f]+\]", ins.op_str)), None)
    if table_load is None or table_jump is None:
        return {"accepted": False, "reason": "unrecognized root command switch"}
    byte_table = int(re.search(r"0x([0-9a-f]+)", table_load.op_str)[1], 16) - image.base
    jump_table = int(re.search(r"0x([0-9a-f]+)", table_jump.op_str)[1], 16) - image.base
    selector = image.get(byte_table + command - 1, 1)[0]
    writer = image.u32(jump_table + selector * 4) - image.base
    raw = image.get(writer, 24)
    current = re.match(re.escape(bytes.fromhex("8b4424108981")) + b"(.{4})" + re.escape(bytes.fromhex("33c0c21800")), raw, re.DOTALL)
    old = re.match(b"\x89[\xbe\x9e]" + b"(.{4})", raw, re.DOTALL)
    field = struct.unpack("<I", (current or old)[1])[0] if current or old else None
    old_register = "edi" if raw[:2] == bytes.fromhex("89be") else "ebx"
    old_uses_this = any(ins.mnemonic == "mov" and ins.op_str == "esi, ecx"
                        for ins in prefix if ins.address < table_load.address)
    writer_argument = bool(current or (old and old_uses_this
                                       and argument_register(image, handler, table_jump.address - image.base,
                                                             old_register, 16)))
    getters = byte_sites(image, bytes.fromhex("8b81") + struct.pack("<I", field) + b"\xc3") if field else []
    samples = []
    for getter in getters:
        for site in callers(image, getter):
            if image.get(site - 3, 3) == bytes.fromhex("8b4e08") and image.get(site + 5, 3) == bytes.fromhex("894624"):
                samples.append({"getter_rva": getter, "call_rva": site, "region": snippet(image, site - 12, 35)})
    consumers = []
    for getter in byte_sites(image, bytes.fromhex("8b4124c3")):
        for site in callers(image, getter):
            insns = list(image.insns(site + 5, 95))
            if any(ins.mnemonic == "or" and ins.op_str in {"ebx, dword ptr [esi + 0x50]", "ebp, dword ptr [esi + 0x50]"}
                   for ins in insns[:4]):
                consumers.append({"sample_getter_rva": getter, "call_rva": site,
                                  "region": snippet(image, site, 100)})
    timers = [timer_contract(image, consumer) for consumer in consumers]
    switch_prefix = [ins for ins in prefix if ins.address < table_load.address]
    decrement = next((n for n, ins in enumerate(switch_prefix) if ins.mnemonic == "dec" and ins.op_str == "eax"), None)
    bound = next((n for n, ins in enumerate(switch_prefix) if ins.mnemonic == "cmp"
                  and ins.op_str == f"eax, {hex(command - 1)}"), None)
    switch_valid = bool(decrement is not None and bound is not None and decrement < bound
                        and bound + 1 < len(switch_prefix) and switch_prefix[bound + 1].mnemonic == "ja"
                        and argument_register(image, handler, switch_prefix[decrement].address - image.base, "eax", 8))
    accepted = bool(writer_argument and switch_valid and len(samples) == 1 and len(consumers) == 1
                    and timers and all(timer["accepted"] for timer in timers))
    return {"accepted": accepted, "reason": "exact wrapper/receiver/sample/timer chain" if accepted else "unrecognized clock chain",
            "command": command, "writer_uses_data1": writer_argument, "switch_valid": switch_valid,
            "byte_table_rva": byte_table, "jump_table_rva": jump_table, "selector": selector,
            "writer": snippet(image, writer, 15 if current else 12), "request_field": field,
            "sampling_candidates": samples, "consumer_candidates": consumers, "timer_contracts": timers}


def main() -> None:
    evidence = json.loads((ROOT / "reports/recovery_A_20261116_native.json").read_text(encoding="utf-8"))
    rows = []
    for pe in evidence["native_evidence"]:
        image = Image(ROOT / pe["binary_path"], pe["sha256"])
        tables = sorted({item["table_rva"] for item in pe["focus"] if item["table_hint"] == "ordinary"})
        wrappers = []
        for table in tables:
            entry = image.u32(table + 577 * 4) - image.base
            wrappers.append({"table_rva": table, **snippet(image, entry, 0x35)})
        classes = rtti_vtable(image, "kcTaskTop")
        rows.append({"path": pe["binary_path"], "sha256": pe["sha256"], "wrappers": wrappers,
                     "root_classes": [{**row, "handler": snippet(image, row["command_handler_RVA"], 0x100)}
                                      for row in classes], **contract(image, wrappers, classes)})
    output = ROOT / "reports/root_clock_profiles_20261117.json"
    output.write_text(json.dumps({"schema": "cs2-root-clock-profiles/v1", "binaries": rows,
                                  "original_code_executed": False, "compiler_invoked": False}, indent=2) + "\n",
                      encoding="utf-8")
    module = ["//! SHA-bound native577 root-command/sample/timer evidence.",
              "//! Generated by scripts/analysis/root_clock_profiles.py; no original code.",
              "pub(super) fn command_for(sha256: &str) -> Option<u32> {", "    match sha256 {"]
    for row in rows:
        if row["accepted"]:
            module.append('        "' + row["sha256"] + '" => Some(' + str(row["command"]) + '),')
    module += ["        _ => None,", "    }", "}", ""]
    (ROOT / "simulator/crates/desktop/src/root_clock_profile.rs").write_text("\n".join(module), encoding="utf-8")
    print(json.dumps({"report": output.relative_to(ROOT).as_posix(),
                      "accepted": sum(row["accepted"] for row in rows),
                      "unrecognized": [row["path"] for row in rows if not row["accepted"]]}))


if __name__ == "__main__":
    main()

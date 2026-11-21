"""Generate exact-PE task command descriptors from registered ordinary tables.

Only immutable PE bytes are decoded. No game, VM, compiler, tests, or recovered
functions are executed. Registration provenance is reused instead of rescanned.
"""
from __future__ import annotations

import hashlib
import json
from pathlib import Path
import struct

import capstone
import pefile
from capstone.x86 import X86_OP_IMM, X86_OP_MEM, X86_OP_REG

ROOT = Path(__file__).resolve().parents[2]
ARCHIVE = ROOT / "archive/2026-11"
OUTPUT = ROOT / "simulator/crates/runtime/src/native_task_profile.rs"
SLOTS = (279, 280, 281, 282, 283, 287, 288, 450, 570, 571, 572, 666, 690, 691, 692)


def wrapper(image: pefile.PE, entry: int, allow_multiple: bool = False) -> dict:
    base = image.OPTIONAL_HEADER.ImageBase
    decoder = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_32)
    decoder.detail = True
    pending = [entry]
    visited: dict[int, object] = {}
    calls = []
    returns = []
    while pending:
        at = pending.pop()
        if at in visited:
            continue
        if not entry <= at < entry + 1024:
            raise ValueError(f"wrapper branches outside its bounded envelope: {entry:x}->{at:x}")
        while at not in visited:
            instruction = next(decoder.disasm(image.get_data(at, 16), base + at, count=1), None)
            if instruction is None:
                raise ValueError(f"undecodable wrapper instruction {at:x}")
            visited[at] = instruction
            if len(visited) > 250:
                raise ValueError("task wrapper exceeded 250 instructions")
            at += instruction.size
            if instruction.mnemonic.startswith("ret"):
                returns.append(instruction)
                break
            if instruction.mnemonic == "call":
                calls.append(instruction)
            if instruction.mnemonic.startswith("j"):
                if not instruction.operands or instruction.operands[0].type != X86_OP_IMM:
                    raise ValueError("task wrapper contains an unresolved jump")
                target = instruction.operands[0].imm - base
                pending.append(target)
                if instruction.mnemonic == "jmp":
                    break
    ordered = sorted(visited.values(), key=lambda instruction: instruction.address)
    commands = set()
    proof_sites = []
    # A task dispatch ends in a vtable+10 call (or a register loaded from
    # vtable+10). Its seven stack arguments include exactly one command > 255.
    # Ordinary VM helpers are direct calls and never selected as dispatches.
    for call in calls:
        if call.operands[0].type not in (X86_OP_MEM, X86_OP_REG):
            continue
        position = ordered.index(call)
        preceding = []
        for instruction in reversed(ordered[max(0, position-18):position]):
            if instruction.mnemonic in ("call", "ret", "jmp"):
                break
            preceding.append(instruction)
        candidates = [
            instruction for instruction in preceding
            if instruction.mnemonic == "push"
            and instruction.operands[0].type == X86_OP_IMM
            and 256 <= instruction.operands[0].imm <= 700
        ]
        if len(candidates) == 1:
            commands.add(candidates[0].operands[0].imm)
            proof_sites.append(candidates[0].address-base)
    if (len(commands) != 1 and not (allow_multiple and len(commands) == 2)) or not returns:
        raise ValueError(f"ambiguous task wrapper {entry:x}: {commands}")
    if any(not instruction.operands or instruction.operands[0].imm != 16 for instruction in returns):
        raise ValueError("wrapper does not return stdcall16")
    ordered_commands = [
        visited[site].operands[0].imm for site in sorted(set(proof_sites))
    ]
    return {
        "entry_rva": entry, "command": ordered_commands[0],
        "commands": ordered_commands, "command_sites_rva": sorted(set(proof_sites)),
        "instructions": [
            {"rva": instruction.address-base, "bytes": instruction.bytes.hex(),
             "mnemonic": instruction.mnemonic, "operands": instruction.op_str}
            for instruction in ordered
        ],
    }


def main() -> None:
    ledger = json.loads((ROOT / "reports/recovery_A_20261116.json").read_text(encoding="utf-8"))
    descriptors = []
    rejected = []
    for digest, record in sorted(ledger["images"].items()):
        tables = [table for table in record["tables"]
                  if table["table_hint"] == "ordinary" and table["registration_chain_verified"]]
        paths = [row["binary_path"] for row in ledger["binaries"]
                 if row["sha256"] == digest and row["binary_path"].lower().endswith(".exe")]
        if not paths:
            continue
        if len(tables) != 1:
            rejected.append({"sha256": digest, "paths": paths, "reason": "no unique registered ordinary table"})
            continue
        data = (ROOT / paths[0]).read_bytes()
        if hashlib.sha256(data).hexdigest() != digest:
            raise ValueError(f"source identity changed: {paths[0]}")
        image = pefile.PE(data=data)
        table = tables[0]
        rows = {}
        for slot in SLOTS:
            entry = table["slots"][slot]["entry_rva"]
            pointer = struct.unpack("<I", image.get_data(table["table_rva"]+slot*4, 4))[0]
            if pointer != image.OPTIONAL_HEADER.ImageBase+entry:
                raise ValueError("registered ordinary pointer changed")
            try:
                rows[str(slot)] = wrapper(image, entry, allow_multiple=slot == 281)
            except ValueError as cause:
                rows[str(slot)] = {"entry_rva": entry, "unresolved": str(cause)}
        descriptors.append({
            "sha256": digest, "paths": paths, "file_version": record["identity"]["file_version"],
            "table_rva": table["table_rva"],
            "registration_sites": table["registration_sites"], "wrappers": rows,
        })
    evidence = {
        "method": "exact SHA and reused registered table; bounded wrapper CFG; immutable reads only",
        "slots": SLOTS, "descriptors": descriptors, "unresolved_images": rejected,
        "original_code_executed": False,
    }
    (ARCHIVE / "20261117_C_task_native_descriptors.json").write_text(
        json.dumps(evidence, ensure_ascii=False, indent=2), encoding="utf-8")
    lines = [
        "//! Exact-PE task messages generated from registered ordinary wrappers.",
        "//! Unknown identities have no default command numbering.",
        "// Generated by scripts/analysis/task_native_descriptors.py.",
        "#[derive(Clone, Copy, Debug)]",
        "pub struct TaskNativeProfile {",
        "    pub message_list: Option<[u32; 3]>, // native570,571,572",
        "    pub export: Option<[u32; 3]>, // native690,691,692",
        "    pub fes_control: Option<[u32; 2]>, // native450,666",
        "    pub scene_debug: Option<[u32; 5]>, // native279..283",
        "    pub scene_list: Option<[u32; 2]>, // native287,288",
        "    pub scene_debug_refresh: Option<u32>, // native281 second dispatch",
        "}",
        "pub fn exact_task_native_profile(sha256: &str) -> Option<TaskNativeProfile> {",
        "    Some(match sha256 {",
    ]
    for descriptor in descriptors:
        rows = descriptor["wrappers"]
        values = []
        for slots in ((570, 571, 572), (690, 691, 692), (450, 666),
                      (279, 280, 281, 282, 283), (287, 288)):
            if all("command" in rows[str(slot)] for slot in slots):
                commands = [rows[str(slot)]["command"] for slot in slots]
                values.append("Some(["+", ".join(map(str, commands))+"])")
            else:
                values.append("None")
        lines.append(f'        "{descriptor["sha256"]}" => TaskNativeProfile {{ '
                     f'message_list: {values[0]}, export: {values[1]}, fes_control: {values[2]}, '
                     f'scene_debug: {values[3]}, scene_list: {values[4]}, '
                     f'scene_debug_refresh: '
                     + (f'Some({rows["281"]["commands"][1]})'
                        if len(rows["281"].get("commands", [])) == 2 else "None")
                     + " },")
    lines += ["        _ => return None,", "    })", "}", ""]
    OUTPUT.write_text("\n".join(lines), encoding="utf-8")
    print(json.dumps({
        "fixed_images": len(descriptors), "unregistered_images": len(rejected),
        "resolved_wrappers": sum("command" in wrapper for row in descriptors for wrapper in row["wrappers"].values()),
        "unresolved": [{row["sha256"][:12]: {slot: value["unresolved"]
                        for slot, value in row["wrappers"].items() if "unresolved" in value}}
                       for row in descriptors if any("unresolved" in value for value in row["wrappers"].values())],
    }, indent=2))


if __name__ == "__main__":
    main()

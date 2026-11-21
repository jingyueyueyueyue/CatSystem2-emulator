"""Static CSS576 receiver/body inventory. Never executes a recovered function.

The instruction hits are navigation, not proof of ownership or consumption.
All PE images are selected by the existing SHA-pinned wrapper ledger.
"""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import re
import struct

import capstone
import pefile

from css_task_576_contracts import primary_vtables

ROOT = Path(__file__).resolve().parents[2]


def images():
    source = json.loads((ROOT / "reports/css_task_576_F_contracts_20261117.json").read_text(encoding="utf-8"))
    rows = source["profiles"] + source["supplemental_wrappers"] + source["legacy_receivers"]
    for row in rows:
        path = ROOT / row["path"]
        if hashlib.sha256(path.read_bytes()).hexdigest() != row["sha256"]:
            raise ValueError("CSS receiver PE identity changed")
        yield row, pefile.PE(str(path))


def fragment(pe, va, length):
    md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_32)
    return [f"{ins.address:08x} {ins.mnemonic} {ins.op_str}"
            for ins in md.disasm(pe.get_data(va - pe.OPTIONAL_HEADER.ImageBase, length), va)]


def contracts():
    """Fixed leaves plus their actual factory/dispatch identities, not x86 execution."""
    records = []
    material = None
    source = json.loads((ROOT / "reports/css_task_576_F_contracts_20261117.json").read_text(encoding="utf-8"))
    modern = {row["sha256"] for row in source["profiles"]+source["supplemental_wrappers"][:3]}
    md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_32)
    for row, pe in images():
        sha = row["sha256"]
        if sha.startswith("72c9c1e7"):
            base = pe.OPTIONAL_HEADER.ImageBase
            strings = {hex(va): pe.get_data(va-base, 40).split(b"\0", 1)[0].decode("ascii")
                       for va in (0x6fb2c8, 0x6f7ec8, 0x6fb43c, 0x6fb440)}
            if list(strings.values()) != ["shd", "kxs", "dae", "file:/%s"]:
                raise ValueError("legacy341 model resource provenance changed")
            material = {"sha256": sha, "path": row["path"], "implemented": False,
                "resource_strings": strings,
                "producer": "315 configures texture manager;316 reloads the shd model and writes+68;317 loads/selects another scene",
                "limit": "This is the DAE scene/material producer, not the MDL locator/camera producer. No empty owner or unused binding map is installed.",
                "fragments": {hex(va): fragment(pe, va, length) for va, length in (
                    (0x41e6d6, 0x126), (0x485630, 0x130), (0x489960, 3),
                    (0x491060, 0x300), (0x4913b0, 0xa0))}}
        if sha not in modern and not sha.startswith("659d5f06"):
            continue
        base = pe.OPTIONAL_HEADER.ImageBase
        wrapper = row["restore576"]
        entry = base + wrapper["entry_rva"]
        raw = pe.get_data(entry - base, 0x98)
        # At entry ESP+12 is the valid native parameter pointer. There are
        # exactly three pushes before lea edi,[esp+18h]; no caller argument
        # slot was overwritten. The final cmp reads the same out slot.
        if raw[:2] != b"\x51\x56" or b"\x57\x8d\x7c\x24\x18\x57" not in raw:
            raise ValueError("native576 output-slot alias changed")
        if b"\x83\x7c\x24\x18\x00" not in raw:
            raise ValueError("native576 no longer tests the retained output word")
        result = {"sha256": sha, "path": row["path"],
                  "command": wrapper["command"], "wrapper": fragment(pe, entry, 0x98),
                  "output_initial_value": "valid nonzero native argument pointer at entry ESP+12",
                  "data": [0, "raw CSS body pointer", 0],
                  "body_length_passed": False}
        if sha in modern:
            tables = primary_vtables(pe)
            result["receivers"] = []
            base_table, = (item for item in tables if item["class"] == ".?AVkcTaskBase@@")
            dispatch = base+base_table["methods_rva"][6]
            dispatch_ins = list(md.disasm(pe.get_data(dispatch-base, 90), dispatch))
            generic_call = next(ins for ins in dispatch_ins if ins.mnemonic == "call" and ins.op_str.startswith("0x"))
            generic = int(generic_call.op_str, 16)
            generic_ins = list(md.disasm(pe.get_data(generic-base, 64), generic))
            generic_guard = next(ins for ins in generic_ins if ins.mnemonic == "ja")
            generic_default = int(generic_guard.op_str, 16)
            if not any(ins.mnemonic == "cmp" and ins.op_str in ("eax, 0x1a", "eax, 0x1b") for ins in generic_ins):
                raise ValueError("generic base command guard changed")
            generic_default_ins = list(md.disasm(pe.get_data(generic_default-base, 28), generic_default))
            if not any(ins.mnemonic == "mov" and ins.op_str == "eax, 1" for ins in generic_default_ins):
                raise ValueError("generic base no-output return changed")
            result["generic_base"] = {"dispatch": fragment(pe, dispatch, generic_call.address+generic_call.size-dispatch),
                "guard": fragment(pe, generic, generic_guard.address+generic_guard.size-generic),
                "default": fragment(pe, generic_default, 28)}
            for name in (".?AVkcTaskTop@@", ".?AVkcTaskScript@@"):
                table, = (item for item in tables if item["class"] == name)
                va = base + table["methods_rva"][7]
                prefix = list(md.disasm(pe.get_data(va - base, 200), va))
                branch = next(ins for ins in prefix if ins.mnemonic ==
                              ("ja" if name == ".?AVkcTaskTop@@" else "jg"))
                destination = int(branch.op_str, 16)
                if name == ".?AVkcTaskScript@@":
                    high = list(md.disasm(pe.get_data(destination - base, 16), destination))
                    if (high[0].mnemonic != "cmp" or high[1].mnemonic != "jne"
                        or int(high[0].op_str.split(", ")[1], 0) == wrapper["command"]):
                        raise ValueError("ordinary CSS command does not take the reviewed default")
                    default = int(high[1].op_str, 16)
                    if pe.get_data(default-base, 5) != b"\xb8\x01\x00\x00\x00":
                        raise ValueError("ordinary default is not the reviewed output-preserving return")
                else:
                    default = destination
                result["receivers"].append({"class": name, "vtable_va": hex(base+table["vtable_rva"]),
                    "handler_va": hex(va), "prefix": fragment(pe, va, branch.address+branch.size-va),
                    "out_of_range": fragment(pe, destination, 0x48),
                    "default_va": hex(default), "default": fragment(pe, default, 0x40),
                    "body_effect": "none", "output_effect": "preserve"})
            result["fallback_routes"] = [{"class": table["class"],
                "instructions": fragment(pe, base+table["methods_rva"][5], 0x90)}
                for table in tables if table["class"] in (".?AVkcTaskTop@@", ".?AVkcTaskBase@@")]
            result["implemented"] = "Top/ordinary output-preserving receiver and exact wrapper boolean; other concrete classes remain explicit boundaries"
        else:
            # Real D owner, not the old report's stale 'no host producer'.
            if pe.get_data(0x41fc6e-base, 9) != bytes.fromhex("8b451cc70000000000"):
                raise ValueError("Campanella concrete366 zero leaf changed")
            result.update({"kind": 42, "constructor": fragment(pe, 0x41ed00, 0xe0),
                "receiver_prefix": fragment(pe, 0x41ef20, 0x60),
                "receiver_leaf": fragment(pe, 0x41fc6e, 0x23),
                "native83_owner_preservation": {"wrapper": fragment(pe, 0x44e900, 0x31),
                    "message": 61, "concrete_default": fragment(pe, 0x41fd1d, 0x1b),
                    "base_dispatch": fragment(pe, 0x4199c0, 0x50),
                    "base_guard": fragment(pe, 0x419680, 0x40),
                    "effect": "no-op for this class; do not run generic plane clear/release_canvas"},
                "owner_lifecycle": {"producer": "native_tasks::create_task_object -> particle_external.canvases",
                    "clear_model": "particle_external::clear_canvas_graph retains constructed manager",
                    "release": "native_tasks unregister -> release_canvas; removes graph, particle dependents and shared binding"},
                "implemented": "available concrete kind42 rejects restore with output0, including before model load; no body read"})
        records.append(result)
    output = ROOT / "reports/css_task_receiver_F_contracts_20261117.json"
    output.write_text(json.dumps({"schema": "cs2-css-task-receivers/v1", "records": records,
        "legacy341_model_producer": material,
        "body_restore_consumers_newly_completed": 0,
        "limit": "No modern437/426 3D/material body consumer is inferred. Legacy341 model is the shd/kxs/DAE chain, not Campanella MDL."},
        ensure_ascii=False, indent=2)+"\n", encoding="utf-8")
    print(json.dumps({"report": output.relative_to(ROOT).as_posix(), "profiles": len(records)}))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sha", help="exact SHA prefix from pinned ledger")
    parser.add_argument("--va", type=lambda value: int(value, 16))
    parser.add_argument("--length", type=lambda value: int(value, 0), default=256)
    parser.add_argument("--contracts", action="store_true")
    args = parser.parse_args()
    if args.contracts:
        contracts()
        return
    md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_32)
    md.detail = True
    for row, pe in images():
        if args.sha and not row["sha256"].startswith(args.sha):
            continue
        if args.va is not None:
            print(row["sha256"])
            print("\n".join(fragment(pe, args.va, args.length)))
            continue
        hits = []
        for section in pe.sections:
            if not section.Characteristics & 0x20000000:
                continue
            va = pe.OPTIONAL_HEADER.ImageBase + section.VirtualAddress
            raw = section.get_data()
            for value in (0x824, 437, 426):
                for match in re.finditer(re.escape(struct.pack("<I", value)), raw):
                    end = match.start() + 4
                    for start in range(max(0, end - 11), match.start()):
                        ins = next(md.disasm(raw[start:end], va + start, 1), None)
                        if (ins is not None and ins.address + ins.size == va + end
                            and ins.mnemonic in ("cmp", "push", "mov")
                            and any(op.type == capstone.x86.X86_OP_IMM and op.imm == value
                                    for op in ins.operands)):
                            hits.append({"va": hex(ins.address), "instruction": f"{ins.mnemonic} {ins.op_str}"})
        print(json.dumps({"sha256": row["sha256"], "hits": hits}), flush=True)


if __name__ == "__main__":
    main()

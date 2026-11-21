"""Prove native549's unwritten return slot aliases the VM input parameter.

Only exact registered tables and a complete recognized dispatcher prefix are
accepted. This reads bytes; it does not run native functions or a VM.
"""
from __future__ import annotations
import json
import struct
from recovery_A_20261116 import Image, ROOT, instruction_row, recover


def operand_cleanup(image: Image, entry: int) -> dict:
    body = recover(image, entry, set())
    cases = []
    resolved = set()
    for edge in body["exit_edges"]:
        site = edge["site_rva"]
        jump = image.get(site, 7)
        # CMP EAX,3; JA default; JMP DWORD PTR [EAX*4+table]. Every table
        # entry is bounded by that unsigned check, including width case4.
        bounds = image.get(site - 5, 4)
        matched_index = ((bounds == bytes.fromhex("83f80377") and jump[:3] == bytes.fromhex("ff2485"))
                         or (bounds == bytes.fromhex("83f90377") and jump[:3] == bytes.fromhex("ff248d")))
        if edge["kind"] == "unresolved-indirect-jump" and matched_index:
            table = struct.unpack_from("<I", jump, 3)[0] - image.base
            targets = [struct.unpack("<I", image.get(table + index * 4, 4))[0] - image.base
                       for index in range(4)]
            if all(entry <= target < entry + 0x400 for target in targets):
                rows = [recover(image, target, set()) for target in sorted(set(targets))]
                if all(row["returns"] and row["all_observed_paths_terminated"]
                       and not row["exit_edges"]
                       and all(ret["pop_bytes"] == 8 for ret in row["returns"]) for row in rows):
                    cases.append({"site_rva": site, "table_rva": table, "case_cfgs": rows})
                    resolved.add(site)
    cleanup = bool(body["returns"] and all(ret["pop_bytes"] == 8 for ret in body["returns"])
                   and all(edge["site_rva"] in resolved for edge in body["exit_edges"])
                   and all(issue["kind"] == "unresolved-indirect-jump" and issue["site_rva"] in resolved
                           for issue in body["issues"]))
    return {"entry_rva": entry, "ret8_proven": cleanup, "cfg": body, "bounded_switches": cases}


def main() -> None:
    evidence = json.loads((ROOT / "reports/recovery_A_20261116_native.json").read_text(encoding="utf-8"))
    rows = []
    for pe in evidence["native_evidence"]:
        image = Image(ROOT / pe["binary_path"], pe["sha256"])
        tables = sorted({item["table_rva"] for item in pe["focus"] if item["table_hint"] == "ordinary"})
        slots = []
        for table in tables:
            pointer = image.get(table + 549 * 4, 4)
            entry = struct.unpack("<I", pointer)[0] - image.base
            raw = image.get(entry, 8)
            slots.append({"table_rva": table, "entry_rva": entry, "bytes": raw.hex(),
                          "exact_status1_unwritten": raw == bytes.fromhex("b801000000c21000")})
        handlers = []
        for dispatcher in pe["dispatchers"]:
            call = dispatcher["indirect_call_rva"]
            # Original input lives at entry ESP+4. SUB16 and three PUSHes put
            # it at ESP+32. Both operand helpers have two stdcall parameters.
            templates = [bytes.fromhex(
                "83ec105356578b7c24208d442410508d4424188bcf508b5f1033f64b"
                "e8000000008d44240c8bcf508d44241c50e8000000008b44240c294718"
                "8d4424208b4f38508b4714034718508b442418ff773c578b0481ffd0"
            ), bytes.fromhex(
                "83ec1053568b74241c578b7e108d442410508d4c2418518bce33db4f"
                "e8000000008d54240c528d44241c508bcee8000000008b4c240c294e18"
                "8b46188b56388d4c2420518b4e1403c88b463c518b4c24188b148a5056ffd2"
            )]
            matched = False
            start, prefix = call, b""
            for template in templates:
                start = call + 2 - len(template)
                prefix = image.get(start, len(template))
                normalized = bytearray(prefix)
                for ins in image.insns(start, len(template)):
                    if ins.mnemonic == "call" and ins.bytes[0] == 0xE8:
                        offset = ins.address - image.base - start
                        normalized[offset + 1:offset + 5] = bytes(4)
                if bytes(normalized) == template:
                    matched = True
                    break
            jump = image.get(call + 7, 7)
            target = None
            helper_proofs = []
            if matched:
                helper_targets = {image.direct(ins) for ins in image.insns(start, len(prefix))
                                  if ins.mnemonic == "call" and ins.bytes[0] == 0xE8}
                for helper in sorted(helper_targets):
                    helper_proofs.append(operand_cleanup(image, helper))
                matched = bool(helper_proofs and all(item["ret8_proven"] for item in helper_proofs))
            if matched and jump[:3] == bytes.fromhex("ff2485"):
                table_va = struct.unpack_from("<I", jump, 3)[0]
                target = struct.unpack("<I", image.get(table_va - image.base + 4, 4))[0]
                value_push = image.get(target - image.base, 5)
                matched = (value_push[:4] == bytes.fromhex("ff742420")
                           or value_push == bytes.fromhex("8b44242050"))
            else:
                matched = False
            handlers.append({"start_rva": start, "call_rva": call,
                             "operand_helpers": helper_proofs,
                             "status1_target_va": target, "recognized_parameter_alias": matched,
                             "instructions": [instruction_row(ins, image.base)
                                              for ins in image.insns(start, len(prefix))] if matched else [],
                             "status1_instructions": [instruction_row(ins, image.base)
                                                       for ins in image.insns(target - image.base, 0x1B)] if matched else []})
        accepted = bool(slots and handlers and all(item["exact_status1_unwritten"] for item in slots)
                        and all(item["recognized_parameter_alias"] for item in handlers))
        rows.append({"path": pe["binary_path"], "sha256": pe["sha256"],
                     "slots": slots, "handlers": handlers, "accepted": accepted})
    report = {
        "schema": "cs2-native-context-return/v1", "status": "fixed-byte-and-stack-alias-evidence",
        "slot": 549, "binaries": rows, "original_code_executed": False, "compiler_invoked": False,
        "boundary": "Opaque VM object identity; no host address or invented constant value.",
    }
    output = ROOT / "reports/native_context_contracts_20261117.json"
    output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    accepted = list({row["sha256"]: row for row in rows if row["accepted"]}.values())
    module = [
        "//! Exact PE identities whose native549 return aliases its VM parameter.",
        "//! Generated by scripts/analysis/native_context_contracts.py; no original code.",
        "pub fn returns_vm_identity(pe_sha256: &str) -> bool {", "    matches!(pe_sha256,",
        "\n        | ".join('        "' + row["sha256"] + '"' if index == 0 else '"' + row["sha256"] + '"'
                              for index, row in enumerate(accepted)),
        "    )", "}", "",
    ]
    if not accepted:
        module = ["//! No PE currently has a complete native549 context-return proof.",
                  "pub fn returns_vm_identity(_: &str) -> bool { false }", ""]
    (ROOT / "simulator/crates/runtime/src/native_context_profile.rs").write_text("\n".join(module), encoding="utf-8")
    print(json.dumps({"report": output.relative_to(ROOT).as_posix(), "accepted": len(accepted),
                      "unrecognized": [row["path"] for row in rows if not row["accepted"]]}))


if __name__ == "__main__":
    main()

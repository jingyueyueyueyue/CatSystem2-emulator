"""Read only inventoried NEKO factory/message producers, without executing KCS."""
import json
import struct
import zlib
from collections import Counter

from audit_runtime_evidence import blowfish_le_decrypt, kif_entries
from recovery_A_20261116 import ROOT, read_json, sha
from recovery_A_emote_save_state_20261118 import decode


OUTPUT = ROOT / "archive/2026-11/20261118_A_save_route_reachability.json"


def main():
    identity = read_json(ROOT / "reports/recovery_A_20261116_emote.json")
    inventory = read_json(ROOT / "reports/kcs_call_inventory_20261116.json")
    game = next(row for row in inventory["games"] if "NEKO" in row["root"])
    selected = [program for program in game["programs"] if any(
        call["index"] in (66, 69, 70) for call in program["calls"])]
    path = (ROOT / identity["binary_path"]).parent / "kcs.int"
    entries, _, cipher = kif_entries(path, 0x0F6BB0AE)
    entries = {entry["name"].decode("ascii").lower(): entry for entry in entries}
    evidence = []
    with path.open("rb") as handle:
        for program in selected:
            name = program["resource"].split("::", 1)[1]
            entry = entries[name.lower()]
            handle.seek(entry["offset"])
            raw = handle.read(entry["size"])
            raw = blowfish_le_decrypt(cipher, raw) if cipher else raw
            if sha(raw) != program["sha256"]:
                raise ValueError("inventoried KCS changed: " + name)
            length, = struct.unpack_from("<I", raw, 16)
            rows = decode(zlib.decompress(raw[52:])[:length])
            positions = {row["offset"]: index for index, row in enumerate(rows)}
            for call in program["calls"]:
                if call["index"] not in (66, 69, 70):
                    continue
                at = positions[call["offset"]]
                row = rows[at]
                if row["opcode"] != 43 or row["operands"][0] != {
                        "flags": 3, "value": call["index"]}:
                    raise ValueError("inventoried call boundary changed")
                start = at
                # This is a lexical statement, not a VM stack simulation.
                while start and rows[start - 1]["opcode"] not in (60, 44):
                    start -= 1
                context = rows[start:at + 1]
                literals = [op["value"] for item in context if item["opcode"] == 37
                            for op in item["operands"] if op["flags"] == 3]
                evidence.append({
                    "resource": program["resource"], "KCS_sha256": program["sha256"],
                    "native": call["index"], "offset": call["offset"],
                    "immediate_pushes": literals, "instructions": context,
                    "literal37_in_message_statement": call["index"] in (69, 70) and 37 in literals,
                    "literal11_in_factory_statement": call["index"] == 66 and 11 in literals,
                    "factory_first_instruction_kind": context[0]["operands"][0]["value"]
                        if call["index"] == 66 and context[0]["opcode"] == 37
                        and context[0]["operands"][0]["flags"] == 3 else None,
                })
    summary = {
        "scope": "only named producer-bearing KCS from existing NEKO inventory; no other game/resource search",
        "programs": len(selected), "sites": dict(Counter(row["native"] for row in evidence)),
        "literal37_message_candidates": [row["offset"] for row in evidence
                                         if row["literal37_in_message_statement"]],
        "literal11_factory_candidates": [row["offset"] for row in evidence
                                         if row["literal11_in_factory_statement"]],
        "direct_factory_kinds": dict(Counter(str(row["factory_first_instruction_kind"])
            for row in evidence if row["native"] == 66)),
        "dynamic_values_evaluated": False, "original_or_VM_executed": False,
        "not_a_global_unreachability_proof": True,
    }
    OUTPUT.write_text(json.dumps({"summary": summary, "sites": evidence},
                                 ensure_ascii=True, indent=2) + "\n", encoding="utf8")
    print(json.dumps(summary, ensure_ascii=True))


if __name__ == "__main__":
    main()

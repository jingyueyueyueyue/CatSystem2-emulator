"""Enrich agent A's PE ledger with native dispatch and workload evidence."""
from __future__ import annotations

import argparse
from collections import Counter, defaultdict
from datetime import datetime, timezone
import json
from pathlib import Path
import re
import sys

from capstone.x86 import X86_OP_IMM, X86_OP_MEM

from recovery_A_20261116 import (ROOT, FOCUS, Image, call_provenance,
                                 instruction_row, read_json, recover, sha)


def dispatcher_evidence(image, setters):
    result = []
    for setter in setters:
        section = image.section(setter)
        start = max(section["rva"], setter-0x100)
        stop = min(section["rva"]+len(section["data"]), setter+0x11000)
        raw = image.get(start, stop-start)
        # Decode at the actual instruction, then require both the VM table
        # load and a scale-four indexed memory operand followed by a call.
        for match in re.finditer(rb"\x8b[\x40-\x7f]\x38", raw):
            site = start + match.start()
            ins = image.insns(site, 110)
            if not ins or len(ins[0].operands) != 2:
                continue
            mem = ins[0].operands[1]
            if mem.type != X86_OP_MEM or mem.mem.disp != 0x38:
                continue
            indexed = [(n, i) for n, i in enumerate(ins[:22])
                       if any(o.type == X86_OP_MEM and o.mem.scale == 4
                              and o.mem.index for o in i.operands)]
            if not indexed:
                continue
            calls = [(n, i) for n, i in enumerate(ins[:25])
                     if i.mnemonic == "call" and image.direct(i) is None]
            if not calls:
                continue
            end = calls[0][0] + 8
            result.append({"table_load_rva": site,
                           "indirect_call_rva": calls[0][1].address-image.base,
                           "instructions": [instruction_row(i, image.base)
                                            for i in ins[:end]],
                           "status": "instruction-verified-VM+0x38-scale4-indirect-call"})
    # Other table-like users can reside nearby; preserve evidence rather than
    # assigning opcode numbers from proximity.
    return result


def full_function(image, functions, rva):
    seeds = set(functions)
    record = recover(image, rva, seeds)
    return record


def markdown(payload):
    q = payload["workload"]
    lines = ["# A native ABI / workload handoff", "",
             "Generated: " + payload["generated_at_utc"], "",
             "## Immediately usable quantities", "",
             f"- {q['PE_paths']} PE paths / {q['PE_SHA256_identities']} content identities.",
             f"- {q['legacy_unique_entries']} legacy SHA256:RVA candidate identities -> {q['new_unique_entries']} recovered identities.",
             f"- {q['new_registered_entries']} registered table entries absent from legacy (unique-image weighting).",
             f"- Candidate-byte coverage: {q['legacy_candidate_bytes_percent']}% -> {q['candidate_bytes_percent']}% (path-weighted; not implementation coverage).",
             f"- Some static semantic evidence: {q['with_some_semantic_evidence']}; unknown: {q['unknown']}; exact local instruction facts: {q['exact_local_facts']}.",
             f"- Ordinary KCS IDs: {q['ordinary_KCS_slots']}; motion: {q['motion_KCS_slots']}; particle: {q['particle_KCS_slots']}.",
             f"- Ordinary source route snapshot classifications: `{q['ordinary_source_route_snapshot']}`.",
             "- Reference contract attachment is provisional across differing PE versions. Unknown includes functions that may already have equivalent Rust behavior without an original-address mapping.",
             "- All-engine completion cannot be calculated by subtracting native routes from PE candidates. The latter also include CRT, codecs and other dependencies.", "",
             "## Original dispatch chain", "",
             "A decoded registration call passes the table to a setter which stores it at VM+0x38. The independent snippets below load VM+0x38, index at scale four, and call the selected pointer.",
             "Function pointer membership is verified as static bytes. Task-factory table classification and a full control-flow proof to opcode 0x2b remain separate obligations.", "",
             "| PE | File version | Setter RVA | Indexed indirect call RVA |",
             "|---|---|---|---|"]
    for b in payload["native_evidence"]:
        lines.append(f"| `{b['binary_path']}` | {b['file_version']} | " +
                     ", ".join(f"`0x{x:08x}`" for x in b["setters"]) + " | " +
                     ", ".join(f"`0x{x['indirect_call_rva']:08x}`" for x in b["dispatchers"]) + " |")
    lines += ["", "## Focused addresses", "",
              "| PE | Table hint | Slot | Entry RVA | Return cleanup | Immediate status evidence |",
              "|---|---|---:|---|---|---|"]
    for b in payload["native_evidence"]:
        for f in b["focus"]:
            if f["index"] in {7, 1063, 124, 934, 125, 647, 925}:
                lines.append(f"| `{b['binary_path']}` | {f['table_hint']} | {f['index']} | "
                             f"`0x{f['entry_rva']:08x}` | {f['ret_pop_bytes']} | {f['status_immediates']} |")
    lines += ["", "## Confirmed details and remaining obligations", "",
              "- Yukikoi ordinary/7, RVA 0x191560: reads task+0x48 to the native result slot, returns status1, ret16, no script-argument read. The field's abstract meaning is still unresolved. It is unrelated to motion/7 atan2 (8 argument bytes) or particle/7 cos (4). No ordinary/7 KCS use appears in the provenance.",
              "- Yukikoi ordinary/1063, RVA 0x1921c0: resolves destination and source VM addresses, calls helper RVA 0x1fef30 with capacity256, returns status2 and preserves result slot.",
              "- Conversion helper 0x1fef30 directly calls MultiByteToWideChar with codepage932 (0x3a4) and WideCharToMultiByte with codepage65001 (0xfde9). Required output length is compared against capacity before the destination conversion call. The <=256-byte contract includes the terminator.",
              "- The helper first queries UTF16 length, rejects lengths >4096, then supplies capacity260 to its intermediate MultiByteToWideChar call. This mismatch means arbitrary large inputs need a diagnostic/explicit policy; do not extrapolate reference support beyond its documented boundaries.",
              "- Yukikoi ordinary/934, RVA 0x1a1910: null context/context+4 guard, formats two names, calls method0x28b550. That method looks up source and destination, only proceeds when source exists and destination is absent, copies source value from node+0x28, and zeroes three appended-state fields before insertion. Fixed reference runtime/cst.rs:286 supplies the portable alias contract.",
              "- Yukikoi ordinary/124, RVA 0x1984c0: format args[1]; send target args[0], command104 (0x68), data=[0,text,0]; status2, ret16. Targeted follow-up identifies old kcTaskStringPlane RTTI, handler0x13c660, block0x13cae1, parser/append0x13db70. See recovery_A_20261116_contracts for append/prepare/render evidence and remaining parser obligations.",
              "- Yukikoi ordinary/125, RVA 0x197e10: target args[0], command105 (0x69), all three data items zero; status2, ret16. Old StringPlane block0x13cb1d clears records/pixels and resets playback state. New StringPlane2 has a separate455-series command protocol.",
              "- Ordinary/647 is documented FES pause/wake command348. PE wrapper evidence and other high-frequency addresses are retained in JSON; it still requires the destination object's real service.",
              "", "## Function identity and version repetitions", "",
              f"- Exact CFG hash groups: {payload['identity_summary']['exact_CFG_hash_groups']}; normalized shape groups: {payload['identity_summary']['normalized_shape_hash_groups']}.",
              f"- Normalized groups occurring in more than one PE content identity: {payload['identity_summary']['cross_PE_normalized_groups']}.",
              "- Cross-image hash groups are review candidates, not permission to assume equal ABI or semantics. Address masking can hide changes in callees and globals.",
              "- The companion JSON lists full members, PE versions, reverse direct-call edges and candidate validation results.", "",
              "## Reproduction / organization", "",
              "`python -B scripts/analysis/recovery_A_20261116.py`",
              "",
              "`python -B scripts/analysis/recovery_A_native_evidence_20261116.py`",
              "",
              "Functional scripts are in scripts/analysis. Pilot outputs are already archived as archive/2026-11/20261116_recovery_A_pilot.json/md. Final reports use recovery_A_20261116*. No runtime source, root README, localization, original game files or backups were modified."]
    return "\n".join(lines) + "\n"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ledger", type=Path, default=ROOT/"reports/recovery_A_20261116.json")
    parser.add_argument("--output-prefix", type=Path, default=ROOT/"reports/recovery_A_20261116_native")
    args = parser.parse_args()
    payload = read_json(args.ledger)
    native_path = ROOT/"reports/native_coverage_20261116.json"
    native = read_json(native_path)
    provenance_path = ROOT/"reports/kcs_call_inventory_20261116.json"
    provenance = read_json(provenance_path)
    calls, counts = call_provenance(provenance)
    ordinary = {key[1] for key in calls if key[0] == "ordinary"}
    routes = {n["index"]: n["status"] for n in native["natives"]}
    p = payload["summary"]
    workload = {
        "PE_paths": p["binary_path_count"], "PE_SHA256_identities": p["unique_image_count"],
        "legacy_unique_entries": p["legacy_unique_candidate_count"],
        "new_unique_entries": p["new_unique_candidate_count"],
        "new_registered_entries": p["new_registered_entries_missing_legacy_unique_images"],
        "legacy_candidate_bytes_percent": p["legacy_path_byte_coverage_percent"],
        "candidate_bytes_percent": p["new_path_byte_coverage_percent"],
        "with_some_semantic_evidence": p["semantic_evidence_unique_candidates"],
        "unknown": p["unknown_unique_candidates"],
        "exact_local_facts": p["exact_local_facts_unique_candidates"],
        "ordinary_KCS_slots": len(ordinary),
        "motion_KCS_slots": len({k[1] for k in calls if k[0] == "motion"}),
        "particle_KCS_slots": len({k[1] for k in calls if k[0] == "particle"}),
        "ordinary_source_route_snapshot": dict(Counter(routes.get(n, "absent-from-route-report")
                                                       for n in ordinary)),
        "source_snapshot_sha256": sha(native_path.read_bytes()),
        "ordinary_7_local_calls": len(calls.get(("ordinary", 7), [])),
    }
    exact_groups, shape_groups, reverse = defaultdict(list), defaultdict(list), []
    native_evidence = []
    validation = Counter()
    paths = {}
    for binary in payload["binaries"]:
        paths.setdefault(binary["sha256"], binary["binary_path"])
    for digest, img in payload["images"].items():
        functions = {f["entry_rva"]: f for f in img["functions"]}
        for f in functions.values():
            assert f["function_identity"] == f"{digest}:{f['entry_rva']:08x}"
            spans = f["instruction_ranges_rva"]
            assert sum(b-a for a, b in spans) == f["instruction_bytes"]
            assert all(a < b for a, b in spans)
            assert all(spans[n][1] < spans[n+1][0] for n in range(len(spans)-1))
            assert all(any(a <= c["site_rva"] < b for a, b in spans) for c in f["calls"])
            validation["candidate_records_checked"] += 1
            member = {"PE_sha256": digest, "rva": f["entry_rva"],
                      "file_version": img["identity"]["file_version"]}
            if f["instruction_bytes"]:
                exact_groups[f["exact_CFG_hash"]].append(member)
                shape_groups[f["normalized_shape_hash"]].append(member)
            for c in f["calls"]:
                if c["kind"] == "direct" and c["target_rva"] in functions:
                    reverse.append({"PE_sha256": digest, "caller_rva": f["entry_rva"],
                                    "site_rva": c["site_rva"], "callee_rva": c["target_rva"]})
        if not img["tables"]:
            continue
        print("Native evidence", paths[digest].encode("ascii", "backslashreplace").decode(),
              flush=True)
        image = Image(ROOT/paths[digest], digest)
        focus = []
        helpers = {}
        for table in img["tables"]:
            for slot in table["slots"]:
                assert image.u32(table["table_rva"]+4*slot["index"]) == image.base+slot["entry_rva"]
                validation["registered_pointer_slots_checked"] += 1
                if slot["index"] not in FOCUS:
                    continue
                record = functions[slot["entry_rva"]]
                focus.append({"table_hint": table["table_hint"],
                              "table_rva": table["table_rva"], "index": slot["index"],
                              "entry_rva": slot["entry_rva"],
                              "ret_pop_bytes": sorted({r["pop_bytes"] for r in record["returns"]}),
                              "status_immediates": record["observed_immediate_EAX_before_returns"],
                              "reference_contract": slot["reference_contract"],
                              "provenance_argument_bytes_counts": dict(counts.get((table["table_hint"], slot["index"]), {})),
                              "function": record})
                if table["table_hint"] == "ordinary" and slot["index"] in {1063, 934, 124}:
                    for c in record["calls"]:
                        if c["kind"] == "direct" and image.code(c["target_rva"]):
                            target = c["target_rva"]
                            helpers[f"0x{target:08x}"] = full_function(image, functions, target)
        native_evidence.append({"binary_path": paths[digest], "sha256": digest,
                                "file_version": img["identity"]["file_version"],
                                "setters": img["VM_setters"],
                                "dispatchers": dispatcher_evidence(image, img["VM_setters"]),
                                "focus": focus, "helpers": helpers})
    # Exact hashes exclude entry RVA but include CFG-relative offsets and bytes.
    # Shape groups mask addresses and must never be treated as proven equivalence.
    cross = {h: group for h, group in shape_groups.items()
             if len({m["PE_sha256"] for m in group}) > 1}
    out = {"schema": "cs2-recovery-A-native/v1", "status": "static-evidence-only",
           "generated_at_utc": datetime.now(timezone.utc).isoformat(),
           "input_sha256": {"ledger": sha(args.ledger.read_bytes()),
                            "native_coverage": sha(native_path.read_bytes()),
                            "provenance": sha(provenance_path.read_bytes())},
           "workload": workload, "validation": dict(validation),
           "identity_summary": {
               "exact_CFG_hash_groups": len(exact_groups),
               "normalized_shape_hash_groups": len(shape_groups),
               "cross_PE_normalized_groups": len(cross)},
           "native_evidence": native_evidence,
           "reverse_direct_call_edges": reverse,
           "repeated_exact_CFG_groups": {h: group for h, group in exact_groups.items()
                                          if len(group) > 1},
           "cross_PE_normalized_shape_groups": cross}
    args.output_prefix.with_suffix(".json").write_text(
        json.dumps(out, ensure_ascii=False, indent=2)+"\n", encoding="utf-8")
    args.output_prefix.with_suffix(".md").write_text(markdown(out), encoding="utf-8")
    print(json.dumps({"workload": workload, "validation": dict(validation)}, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())

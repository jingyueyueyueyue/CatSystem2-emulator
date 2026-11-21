"""Read-only PE/native recovery with content identities and bounded CFGs.

No game, compiler, emulator, or runtime is invoked. Inputs are immutable bytes.
Only this agent's reports are written. Run with python -B to avoid pycache.
"""
from __future__ import annotations

import argparse
from bisect import bisect_right
from collections import Counter, defaultdict, deque
from copy import deepcopy
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import re
import struct
import sys

import capstone
from capstone.x86 import X86_OP_IMM, X86_OP_MEM, X86_OP_REG
import pefile

ROOT = Path(__file__).resolve().parents[2]
PREFIX = "recovery_A_20261116"
MAX_INSNS = 8192
MAX_BLOCKS = 512
SETTER = bytes.fromhex("8b442404894138b801000000c20800")
FOCUS = {7, 26, 64, 78, 85, 94, 116, 124, 125, 258, 262, 332, 339,
         340, 342, 364, 367, 435, 447, 448, 449, 452, 453, 647, 685,
         727, 732, 925, 934, 1063}


def sha(data):
    return hashlib.sha256(data).hexdigest()


def read_json(path):
    return json.loads(path.read_text(encoding="utf-8"))


def ranges(values):
    result = []
    for left, right in sorted(values):
        if result and left <= result[-1][1]:
            result[-1][1] = max(result[-1][1], right)
        else:
            result.append([left, right])
    return result


def byte_count(values):
    return sum(b - a for a, b in ranges(values))


def instruction_row(i, base):
    return {"rva": i.address - base, "bytes": i.bytes.hex(),
            "mnemonic": i.mnemonic, "operands": i.op_str}


class Image:
    def __init__(self, path, expected):
        self.path = path
        self.data = path.read_bytes()
        self.digest = sha(self.data)
        if self.digest != expected:
            raise ValueError("Stale source identity: " + str(path))
        self.pe = pefile.PE(data=self.data)
        self.base = int(self.pe.OPTIONAL_HEADER.ImageBase)
        self.bits = 64 if self.pe.OPTIONAL_HEADER.Magic == 0x20B else 32
        self.md = capstone.Cs(capstone.CS_ARCH_X86,
                             capstone.CS_MODE_64 if self.bits == 64
                             else capstone.CS_MODE_32)
        self.md.detail = True
        self.sections = []
        for s in self.pe.sections:
            self.sections.append({
                "name": s.Name.rstrip(b"\0").decode("ascii", "replace"),
                "rva": int(s.VirtualAddress), "data": s.get_data(),
                "raw_offset": int(s.PointerToRawData),
                "executable": bool(s.Characteristics & 0x20000000),
                "flags": int(s.Characteristics),
            })
        self.imports = {}
        self.import_descriptors = []
        for field, kind in (("DIRECTORY_ENTRY_IMPORT", "normal"),
                            ("DIRECTORY_ENTRY_DELAY_IMPORT", "delay")):
            for d in getattr(self.pe, field, []):
                module = d.dll.decode("ascii", "replace")
                symbols = []
                for imp in d.imports:
                    name = imp.name.decode("ascii", "replace") if imp.name else None
                    rva = int(imp.address) - self.base
                    row = {"module": module, "name": name,
                           "ordinal": imp.ordinal if name is None else None,
                           "iat_rva": rva, "kind": kind}
                    self.imports[rva] = row
                    symbols.append(row)
                self.import_descriptors.append({"module": module, "kind": kind,
                                                "symbols": symbols})
        self.relocations = []
        for block in getattr(self.pe, "DIRECTORY_ENTRY_BASERELOC", []):
            for entry in block.entries:
                width = {3: 4, 10: 8}.get(entry.type)
                if width:
                    self.relocations.append((entry.rva, entry.rva + width))
        self.relocations.sort()
        self.relocation_starts = [a for a, _ in self.relocations]
        self.cache = {}

    def section(self, rva, code=False):
        return next((s for s in self.sections
                     if s["rva"] <= rva < s["rva"] + len(s["data"])
                     and (not code or s["executable"])), None)

    def get(self, rva, length):
        section = self.section(rva)
        if section is None:
            return b""
        off = rva - section["rva"]
        return section["data"][off:off + length]

    def code(self, rva):
        return self.section(rva, True) is not None

    def decode(self, rva):
        if rva not in self.cache:
            self.cache[rva] = next(self.md.disasm(self.get(rva, 15),
                                                 self.base + rva, count=1), None)
        return self.cache[rva]

    def u32(self, rva):
        value = self.get(rva, 4)
        return struct.unpack("<I", value)[0] if len(value) == 4 else None

    def insns(self, rva, limit=512):
        return list(self.md.disasm(self.get(rva, limit), self.base + rva))

    def import_target(self, i):
        if len(i.operands) == 1 and i.operands[0].type == X86_OP_MEM:
            mem = i.operands[0].mem
            if not mem.base and not mem.index:
                return self.imports.get(mem.disp - self.base)
        return None

    def direct(self, i):
        if i.operands and i.operands[0].type == X86_OP_IMM:
            return int(i.operands[0].imm) - self.base
        return None

    def identity(self):
        fixed = getattr(self.pe, "VS_FIXEDFILEINFO", [])
        version = None
        if fixed:
            ms, ls = fixed[0].FileVersionMS, fixed[0].FileVersionLS
            version = ".".join(map(str, [ms >> 16, ms & 65535, ls >> 16, ls & 65535]))
        strings = {}
        for group in getattr(self.pe, "FileInfo", []):
            for item in group:
                for table in getattr(item, "StringTable", []):
                    for key, value in table.entries.items():
                        strings[key.decode("utf-8", "replace")] = value.decode("utf-8", "replace")
        return {"sha256": self.digest, "file_size": len(self.data),
                "machine": int(self.pe.FILE_HEADER.Machine), "bits": self.bits,
                "image_base": self.base,
                "entrypoint_rva": int(self.pe.OPTIONAL_HEADER.AddressOfEntryPoint),
                "timestamp_raw": int(self.pe.FILE_HEADER.TimeDateStamp),
                "checksum_raw": int(self.pe.OPTIONAL_HEADER.CheckSum),
                "file_version": version, "version_strings": strings,
                "sections": [{k: v for k, v in s.items() if k != "data"} |
                             {"raw_bytes": len(s["data"]), "sha256": sha(s["data"])}
                             for s in self.sections]}

    def normalized_hash(self, instructions, entry):
        digest = hashlib.sha256()
        for i in instructions:
            rva = i.address - self.base
            value = bytearray(i.bytes)
            loc = max(0, bisect_right(self.relocation_starts, rva) - 1)
            for left, right in self.relocations[loc:loc+5]:
                if rva < right and rva + i.size > left:
                    for x in range(max(0, left-rva), min(i.size, right-rva)):
                        value[x] = 0
            if i.mnemonic in {"call", "jmp"} or i.group(capstone.CS_GRP_JUMP):
                if i.imm_size:
                    value[i.imm_offset:i.imm_offset+i.imm_size] = b"\0" * i.imm_size
            for op in i.operands:
                if op.type == X86_OP_IMM and self.section(op.imm - self.base):
                    if i.imm_size:
                        value[i.imm_offset:i.imm_offset+i.imm_size] = b"\0" * i.imm_size
                if op.type == X86_OP_MEM and not op.mem.base and not op.mem.index:
                    if self.section(op.mem.disp - self.base) and i.disp_size:
                        value[i.disp_offset:i.disp_offset+i.disp_size] = b"\0" * i.disp_size
            digest.update(struct.pack("<qI", rva-entry, len(value)))
            digest.update(value)
        return digest.hexdigest()


def registrations(image):
    """Decode call sites targeting a byte-verified VM table setter."""
    if image.bits != 32:
        return [], []
    setters = []
    for s in image.sections:
        if s["executable"]:
            setters += [s["rva"] + m.start()
                        for m in re.finditer(re.escape(SETTER), s["data"])]
    rows = []
    for s in image.sections:
        if not s["executable"]:
            continue
        for match in re.finditer(b"\xe8", s["data"]):
            off = match.start()
            if off + 5 > len(s["data"]):
                continue
            site = s["rva"] + off
            target = site + 5 + struct.unpack_from("<i", s["data"], off+1)[0]
            if target not in setters:
                continue
            # A bounded suffix must decode exactly through the observed call.
            # Search last PUSH imm32, excluding bytes inside other instructions
            # by requiring an uninterrupted suffix ending at the E8 call.
            for distance in range(5, 25):
                start = site - distance
                if image.get(start, 1) != b"\x68":
                    continue
                decoded = image.insns(start, distance + 5)
                if not decoded or decoded[-1].address - image.base != site:
                    continue
                if decoded[0].mnemonic != "push":
                    continue
                if any(i.mnemonic in {"jmp", "ret", "call"} for i in decoded[:-1]):
                    continue
                table = image.u32(start + 1)
                if table is None:
                    continue
                table -= image.base
                if image.section(table) is None:
                    continue
                ptr = image.u32(table)
                if ptr is None or not image.code(ptr - image.base):
                    continue
                rows.append({"setter_rva": target, "call_site_rva": site,
                             "table_rva": table, "push_rva": start,
                             "instructions": [instruction_row(i, image.base) for i in decoded],
                             "status": "decoded-table-registration-to-VM+0x38"})
                break
    return setters, rows


def classify_tables(image, registrations_):
    by_table = defaultdict(list)
    for row in registrations_:
        by_table[row["table_rva"]].append(row)
    result = []
    for table, refs in sorted(by_table.items()):
        slots = []
        for n in range(1400):
            ptr = image.u32(table + 4*n)
            if ptr is None or not image.code(ptr-image.base):
                break
            slots.append(ptr-image.base)
        if len(slots) < 12:
            continue
        alias = len(slots) > 3 and slots[0] == slots[2] == slots[3]
        if alias and len(slots) > 500:
            hint = "ordinary"
        elif len(slots) == 45:
            hint = "motion"
        elif len(slots) in {44, 51}:
            hint = "particle"
        else:
            hint = "unclassified"
        result.append({"table_rva": table, "table_hint": hint,
                       "table_identity_evidence": "registration plus pointer-run shape; no task factory proof",
                       "contiguous_code_pointer_count": len(slots),
                       "bounds_status": "first non-code pointer; not a proven VM index bound",
                       "registration_chain_verified": True,
                       "registration_sites": refs,
                       "slots": [{"index": n, "entry_rva": ptr} for n, ptr in enumerate(slots)]})
    return result


def recover(image, entry, seeds):
    todo = deque([entry])
    visited = {}
    blocks = set()
    calls, exits, returns, issues = [], [], [], []
    while todo and len(blocks) < MAX_BLOCKS and len(visited) < MAX_INSNS:
        pc = todo.popleft()
        if pc in blocks:
            continue
        blocks.add(pc)
        while pc not in visited and len(visited) < MAX_INSNS:
            if pc != entry and pc in seeds:
                exits.append({"site_rva": pc, "kind": "other-known-entry", "target_rva": pc})
                break
            if not image.code(pc):
                issues.append({"site_rva": pc, "kind": "outside-file-backed-code"})
                break
            i = image.decode(pc)
            if i is None:
                issues.append({"site_rva": pc, "kind": "decode-failed"})
                break
            if i.mnemonic in {"int3", "ud2", "hlt", "int", "iret", "iretd", "iretq"}:
                issues.append({"site_rva": pc, "kind": "trap-or-system-transfer"})
                break
            visited[pc] = i
            nxt = pc + i.size
            if i.mnemonic.startswith("ret"):
                returns.append({"site_rva": pc, "pop_bytes": (i.operands[0].imm
                                                            if i.operands else 0)})
                break
            if i.mnemonic == "call":
                target = image.direct(i)
                imported = image.import_target(i)
                calls.append({"site_rva": pc, "kind": "import" if imported else
                              "direct" if target is not None else "indirect",
                              "target_rva": target,
                              "target_import": imported,
                              "instruction": instruction_row(i, image.base)})
            if i.group(capstone.CS_GRP_JUMP) or i.mnemonic.startswith("loop"):
                target = image.direct(i)
                if target is None:
                    imported = image.import_target(i)
                    exits.append({"site_rva": pc, "kind": "import-thunk" if imported
                                  else "unresolved-indirect-jump", "import": imported})
                    if imported is None:
                        issues.append({"site_rva": pc, "kind": "unresolved-indirect-jump"})
                elif target != entry and target in seeds:
                    exits.append({"site_rva": pc, "kind": "tail-to-known-entry",
                                  "target_rva": target})
                elif image.code(target):
                    todo.append(target)
                else:
                    issues.append({"site_rva": pc, "kind": "jump-outside-code",
                                   "target_rva": target})
                if i.mnemonic not in {"jmp", "ljmp"}:
                    todo.append(nxt)
                break
            pc = nxt
    if todo or len(visited) >= MAX_INSNS:
        issues.append({"kind": "analysis-budget-limit"})
    ordered = [visited[key] for key in sorted(visited)]
    spans = ranges([(i.address-image.base, i.address-image.base+i.size) for i in ordered])
    raw = b"".join(struct.pack("<qI", i.address-image.base-entry, i.size) + i.bytes
                   for i in ordered)
    statuses = set()
    for n, i in enumerate(ordered):
        if i.mnemonic.startswith("ret"):
            # Deliberately narrow: immediate EAX in the preceding six straight-line
            # instructions. This is evidence, not general dataflow inference.
            previous = ordered[max(0, n-6):n]
            for prior in reversed(previous):
                if prior.mnemonic == "call":
                    break
                if prior.mnemonic == "mov" and len(prior.operands) == 2:
                    if prior.op_str.startswith("eax, ") and prior.operands[1].type == X86_OP_IMM:
                        statuses.add(prior.operands[1].imm & 0xffffffff)
                        break
    return {"entry_rva": entry, "function_identity": f"{image.digest}:{entry:08x}",
            "boundary_status": "bounded-static-CFG-not-confirmed-function",
            "instruction_ranges_rva": spans,
            "instruction_bytes": sum(b-a for a, b in spans),
            "envelope_end_exclusive": max((b for _, b in spans), default=entry),
            "instruction_count": len(ordered), "block_count": len(blocks),
            "returns": returns, "exit_edges": exits, "issues": issues,
            "all_observed_paths_terminated": not issues,
            "observed_immediate_EAX_before_returns": sorted(statuses),
            "calls": calls, "exact_CFG_hash": sha(raw),
            "normalized_shape_hash": image.normalized_hash(ordered, entry),
            "semantic_status": "unknown", "semantic_evidence": [],
            "disassembly": [instruction_row(i, image.base) for i in ordered]}


def literal_facts(image, record):
    raw = image.get(record["entry_rva"], 32)
    if re.match(rb"\xb8....\xc2\x10\x00", raw, re.S):
        return {"kind": "constant-status", "status": struct.unpack_from("<I", raw, 1)[0],
                "writes_return_slot": False, "argument_reads": False,
                "evidence_bytes": raw[:8].hex()}
    # This is the actual ordinary/7 wrapper in multiple builds.
    if raw.startswith(bytes.fromhex("8b4424088b48488b4424108908b801000000c21000")):
        return {"kind": "task-field-read", "task_offset": 0x48,
                "writes_return_slot": True, "return_status": 1, "ret_pop_bytes": 16,
                "argument_reads": False, "evidence_bytes": raw[:21].hex(),
                "semantic_limit": "field meaning must be established from task writes"}
    return None


def call_provenance(payload):
    result = defaultdict(list)
    counts = defaultdict(Counter)
    for game in payload.get("games", []):
        for program in game["programs"]:
            table = program.get("table_hint", "unknown")
            resource = program["resource"]
            if resource.lower().replace("\\", "/").endswith("inst/inst.kcs"):
                table = "installer"
            for call in program.get("calls", []):
                index = call.get("index")
                if not isinstance(index, int):
                    continue
                key = (table, index)
                counts[key][str(call.get("argument_bytes"))] += 1
                result[key].append({"root": game["root"], "resource": resource,
                                    "program_sha256": program["sha256"],
                                    "program_version": program["program_version"],
                                    "offset": call["offset"],
                                    "argument_bytes": call.get("argument_bytes"),
                                    "table_evidence": program.get("table_evidence")})
    return result, counts


def analyze_image(image, binary, native_by_index, provenance, args):
    old = {f["rva"]: f for f in binary.get("function_candidates", [])}
    setters, regs = registrations(image)
    tables = classify_tables(image, regs)
    seed_sources = defaultdict(set)
    for rva in old:
        seed_sources[rva].add("legacy-candidate")
    for rva in setters:
        seed_sources[rva].add("byte-verified-VM-table-setter")
    for table in tables:
        for slot in table["slots"]:
            seed_sources[slot["entry_rva"]].add("registered-pointer-table")
    entry = int(image.pe.OPTIONAL_HEADER.AddressOfEntryPoint)
    seed_sources[entry].add("PE-entrypoint")
    exports = []
    for e in getattr(getattr(image.pe, "DIRECTORY_ENTRY_EXPORT", None), "symbols", []):
        exports.append({"name": e.name.decode("ascii", "replace") if e.name else None,
                        "ordinal": int(e.ordinal), "rva": int(e.address),
                        "forwarder": e.forwarder.decode("ascii", "replace") if e.forwarder else None})
        if not e.forwarder and image.code(e.address):
            seed_sources[e.address].add("PE-export")
    seeds = {rva for rva in seed_sources if image.code(rva)}
    pending = deque(sorted(seeds))
    functions = {}
    while pending:
        rva = pending.popleft()
        if rva in functions:
            continue
        record = recover(image, rva, seeds)
        record["seed_sources"] = sorted(seed_sources[rva])
        functions[rva] = record
        for call in record["calls"]:
            target = call["target_rva"]
            if call["kind"] == "direct" and image.code(target) and target not in seeds:
                seeds.add(target)
                seed_sources[target].add("recursive-direct-call-target")
                pending.append(target)
    # Re-run only CFGs that now cross a newly discovered entry. This avoids
    # treating a prologue-to-next-function fallthrough as a broad boundary.
    ordered_seeds = sorted(seeds)
    for rva, record in list(functions.items()):
        crossed = False
        for left, right in record["instruction_ranges_rva"]:
            pos = bisect_right(ordered_seeds, left)
            if pos < len(ordered_seeds) and ordered_seeds[pos] < right:
                crossed = True
                break
        if crossed:
            revised = recover(image, rva, seeds)
            revised["seed_sources"] = record["seed_sources"]
            functions[rva] = revised
    for rva, record in functions.items():
        fact = literal_facts(image, record)
        if fact:
            record["semantic_status"] = "exact-local-instruction-fact"
            record["semantic_evidence"].append(fact)
        if rva in setters:
            record["semantic_status"] = "exact-local-instruction-fact"
            record["semantic_evidence"].append({
                "kind": "VM-native-table-setter", "VM_field_offset": 0x38,
                "source": "first stack argument", "EAX_status": 1, "ret_pop_bytes": 8,
                "ignored_second_argument": True, "evidence_bytes": SETTER.hex()})
        for call in record["calls"]:
            target = call["target_rva"]
            if target in functions:
                # Preserve import identities for direct calls through JMP IAT thunks.
                thunk = functions[target]
                if len(thunk["exit_edges"]) == 1:
                    ex = thunk["exit_edges"][0]
                    if ex["kind"] == "import-thunk":
                        call["resolved_import_thunk"] = ex["import"]
    for table in tables:
        hint = table["table_hint"]
        aliases = defaultdict(list)
        for slot in table["slots"]:
            aliases[slot["entry_rva"]].append(slot["index"])
        table["distinct_entries"] = len(aliases)
        for slot in table["slots"]:
            index, rva = slot["index"], slot["entry_rva"]
            f = functions.get(rva)
            docs = [d for d in native_by_index.get(index, {}).get("semantic_documentation", [])
                    if d.get("table") == hint]
            if f and docs:
                f["semantic_evidence"].extend(docs)
                if f["semantic_status"] == "unknown":
                    f["semantic_status"] = "fixed-reference-contract-for-registered-slot"
            slot.update({"function_identity": f"{image.digest}:{rva:08x}",
                         "shared_slots": aliases[rva],
                         "reference_contract": docs,
                         "provenance_call_sites": len(provenance.get((hint, index), [])),
                         "local_literal_fact": literal_facts(image, f) if f else None,
                         "implemented_claim": False})
    focused_entries = {slot["entry_rva"] for table in tables
                       for slot in table["slots"] if slot["index"] in FOCUS}
    for rva, record in functions.items():
        if rva not in focused_entries and rva not in setters:
            record["disassembly_truncated"] = len(record["disassembly"]) > 12
            record["disassembly"] = record["disassembly"][:12]
    recovered_ranges = [r for f in functions.values() for r in f["instruction_ranges_rva"]]
    old_ranges = [(r["start_rva"], r["end_rva_exclusive"])
                  for f in old.values() for r in f.get("instruction_ranges_rva", [])]
    raw_bytes = sum(len(s["data"]) for s in image.sections if s["executable"])
    new_bytes = byte_count(recovered_ranges)
    return {"identity": image.identity(), "imports": image.import_descriptors,
            "exports": exports, "VM_setters": setters, "registrations": regs,
            "tables": tables, "functions": list(functions.values()),
            "summary": {
                "legacy_candidate_count": len(old),
                "registered_table_entry_count": sum(len(t["slots"]) for t in tables),
                "registered_distinct_entry_count": len({s["entry_rva"] for t in tables for s in t["slots"]}),
                "registered_entries_missing_legacy": len({s["entry_rva"] for t in tables
                                                         for s in t["slots"]} - set(old)),
                "recovered_candidate_count": len(functions),
                "candidates_with_decoded_instructions": sum(bool(f["instruction_count"]) for f in functions.values()),
                "zero_instruction_rejected_seeds": sum(not f["instruction_count"] for f in functions.values()),
                "new_candidate_count": len(set(functions)-set(old)),
                "legacy_candidates_absent_from_code": len(set(old)-set(functions)),
                "executable_raw_bytes": raw_bytes,
                "legacy_candidate_instruction_bytes": byte_count(old_ranges),
                "legacy_candidate_coverage_percent": round(100*byte_count(old_ranges)/raw_bytes, 5) if raw_bytes else 0,
                "recovered_candidate_instruction_bytes": new_bytes,
                "recovered_candidate_coverage_percent": round(100*new_bytes/raw_bytes, 5) if raw_bytes else 0,
                "unknown_candidate_count": sum(f["semantic_status"] == "unknown" for f in functions.values()),
                "candidates_with_some_semantic_evidence": sum(f["semantic_status"] != "unknown" for f in functions.values()),
                "exact_local_instruction_facts": sum(f["semantic_status"] == "exact-local-instruction-fact"
                                                    for f in functions.values()),
                "CFGs_with_unresolved_paths": sum(bool(f["issues"]) for f in functions.values()),
                "call_site_count": sum(len(f["calls"]) for f in functions.values()),
                "semantic_implementations_claimed": 0}}


def render(payload):
    s = payload["summary"]
    lines = ["# PE/runtime recovery A (2026-11-16 task, regenerated " +
             payload["generated_at_utc"][:10] + ")", "",
             "Read-only static recovery. No compilation, tests of the runtime, game execution, or source implementation.",
             "Registration and static CFG evidence do not establish behavioral completion.", "",
             "## Quantified remaining work", "",
             f"- PE paths: {s['binary_path_count']}; byte-distinct PE images: {s['unique_image_count']}.",
             f"- Legacy path-weighted candidates: {s['legacy_path_candidate_count']}; SHA256:RVA identities: {s['legacy_unique_candidate_count']}.",
             f"- New path-weighted candidates: {s['new_path_candidate_count']}; SHA256:RVA identities: {s['new_unique_candidate_count']}.",
             f"- Added unique candidate entries: {s['added_unique_candidates']}; newly registered table entries missing in legacy: {s['new_registered_entries_missing_legacy_unique_images']}.",
             f"- Old candidate byte coverage: {s['legacy_path_byte_coverage_percent']}%; recovered CFG coverage: {s['new_path_byte_coverage_percent']}%.",
             f"- Candidate identities with some semantic evidence: {s['semantic_evidence_unique_candidates']}; without: {s['unknown_unique_candidates']}.",
             f"- Candidates with decoded instructions: {s['candidates_with_decoded_instructions']}; zero-instruction rejected seeds retained for audit: {s['zero_instruction_rejected_seeds']}.",
             f"- Exact local instruction facts: {s['exact_local_facts_unique_candidates']}. These include small stubs, setters and field reads, not full subsystem implementations.",
             f"- Runtime source snapshot categories (separate denominator): `{payload['runtime_source_snapshot']['status_counts']}`.",
             "- Zero semantic implementation claims is an analysis policy. It does NOT mean zero existing Rust source behavior.",
             "- An all-functions completion percentage or reliable hours estimate cannot be calculated: candidates are not a complete function universe; OS/CRT/codec functions and game engine functions need different scope.",
             "- Ordinary native denominator comes from the new KCS provenance (table identity retained); the previous merged 685 numeric IDs must not be used.", "",
             "## Per-PE quantities", "",
             "| PE | File version | Old/new candidates | Old/new code bytes % | Some semantics / unknown | Registered new entries |",
             "|---|---|---:|---:|---:|---:|"]
    for row in payload["binaries"]:
        q = row["summary"]
        lines.append(f"| `{row['binary_path']}` | {row['file_version']} | "
                     f"{q['legacy_candidate_count']}/{q['recovered_candidate_count']} | "
                     f"{q['legacy_candidate_coverage_percent']}/{q['recovered_candidate_coverage_percent']} | "
                     f"{q['candidates_with_some_semantic_evidence']}/{q['unknown_candidate_count']} | "
                     f"{q['registered_entries_missing_legacy']} |")
    lines += ["", "## Registered pointer tables", "",
              "The setter is instruction-verified (`VM+0x38`). Table category is an explicit shape hint; task factory and opcode-dispatch proof are separate.",
              "Pointer-run length ends at the first non-code pointer; it is NOT a VM bounds proof. No missing slot receives fabricated semantics.", "",
              "| PE | Table RVA | Kind hint | Contiguous pointers / distinct entries | Registration call RVA |",
              "|---|---|---|---:|---|"]
    for b in payload["binaries"]:
        image = payload["images"][b["sha256"]]
        for t in image["tables"]:
            lines.append(f"| `{b['binary_path']}` | `0x{t['table_rva']:08x}` | {t['table_hint']} | "
                         f"{t['contiguous_code_pointer_count']}/{t['distinct_entries']} | " +
                         ", ".join(f"`0x{x['call_site_rva']:08x}`" for x in t["registration_sites"]) + " |")
    lines += ["", "## Concrete handoff evidence", ""]
    for finding in payload["handoff"]:
        lines += [f"- **{finding['topic']}**: {finding['conclusion']}",
                  f"  Evidence: {finding['evidence']}"]
    lines += ["", "## Identity / duplicates", "",
              f"- Unique exact CFG byte hashes: {s['exact_CFG_hash_groups']}; normalized shape groups: {s['normalized_shape_hash_groups']}.",
              "- The canonical identity is PE SHA256 + RVA. Same bytes at another RVA are a duplication hint, not proof of shared semantic identity.",
              "- Normalized shape hashes mask relocation bytes and selected address immediates; they can merge different callees. They are review hints, never ABI equivalence.",
              "- PE version resources, timestamps and section hashes are retained per byte-distinct image. Matching version text alone never merges binaries.",
              "- Import descriptors include normal/delay IAT identities; direct calls through one-instruction import thunks are resolved separately from indirect virtual calls.",
              "", "## Reproduction / limitations", "",
              "`python -B scripts/analysis/recovery_A_20261116.py`",
              "", "The companion JSON stores all disjoint CFG ranges, returns, exits, unresolved jumps, import call edges, native slots, file identities and KCS resource/version/offset joins.",
              "Direct-call seeds are recursively expanded. Trap bytes, undecodable bytes, indirect jumps and known other entries terminate a path; unresolved paths remain explicit.",
              "Reference contracts attached to registered slot hypotheses are evidence, not behavioral verification. Static source route snapshots may change while other agents work.",
              "Scripts are classified under scripts/analysis; final reports under reports. No temporary products or game backups are created or modified; no root README or localization edits."]
    return "\n".join(lines) + "\n"


def write_summary(payload, output_prefix):
    """Small stable workload input; deliberately excludes all CFG/edge arrays."""
    path = output_prefix.parent / (output_prefix.name + "_summary.json")
    summary = dict(payload["summary"])
    summary["generated_at_utc"] = payload["generated_at_utc"]
    summary["source_ledger_sha256"] = payload["source_hashes"]
    summary["runtime_source_status_counts"] = payload["runtime_source_snapshot"]["status_counts"]
    summary["status"] = "static-candidates-not-implementation-percentage"
    path.write_text(json.dumps({"summary": summary, "perPE": payload["binaries"]},
                               ensure_ascii=False, indent=2)+"\n", encoding="utf-8")
    return path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ledger", type=Path, default=ROOT/"reports/runtime_function_coverage_all.json")
    parser.add_argument("--native-coverage", type=Path, default=ROOT/"reports/native_coverage_20261116.json")
    parser.add_argument("--provenance", type=Path, default=ROOT/"reports/kcs_call_inventory_20261116.json")
    parser.add_argument("--output-prefix", type=Path, default=ROOT/"reports"/PREFIX)
    parser.add_argument("--max-images", type=int)
    args = parser.parse_args()
    inputs = [args.ledger, args.native_coverage, args.provenance]
    source_hashes = {str(p.relative_to(ROOT)): sha(p.read_bytes()) for p in inputs}
    ledger = read_json(args.ledger)
    native = read_json(args.native_coverage)
    provenance_payload = read_json(args.provenance)
    provenance, usage_counts = call_provenance(provenance_payload)
    by_index = {n["index"]: n for n in native["natives"]}
    images, binaries = {}, []
    for binary in ledger["binaries"]:
        digest = binary["sha256"]
        if digest not in images:
            if args.max_images and len(images) >= args.max_images:
                break
            print("Analyzing", binary["binary_path"].encode("ascii", "backslashreplace").decode(), flush=True)
            image = Image(ROOT/binary["binary_path"], digest)
            images[digest] = analyze_image(image, binary, by_index, provenance, args)
            q = images[digest]["summary"]
            print("  candidates", q["legacy_candidate_count"], "->", q["recovered_candidate_count"],
                  "registered", q["registered_distinct_entry_count"], flush=True)
        recovered = images[digest]
        binaries.append({"binary_path": binary["binary_path"], "sample_id": binary["sample_id"],
                         "runtime_role": binary.get("runtime_role"), "sha256": digest,
                         "file_version": recovered["identity"]["file_version"],
                         "summary": deepcopy(recovered["summary"]),
                         "legacy_linear_decode_percent": binary.get("static_coverage", {}).get("decoded_instruction_coverage_percent")})
    fs = [f for image in images.values() for f in image["functions"]]
    old_unique = {(b["sha256"], f["rva"]) for b in ledger["binaries"]
                  if b["sha256"] in images for f in b["function_candidates"]}
    def sum_paths(key):
        return sum(b["summary"][key] for b in binaries)
    total_code = sum_paths("executable_raw_bytes")
    summary = {
        "binary_path_count": len(binaries), "unique_image_count": len(images),
        "legacy_path_candidate_count": sum_paths("legacy_candidate_count"),
        "legacy_unique_candidate_count": len(old_unique),
        "new_path_candidate_count": sum_paths("recovered_candidate_count"),
        "new_unique_candidate_count": len(fs),
        "added_unique_candidates": sum((image["identity"]["sha256"], f["entry_rva"]) not in old_unique
                                       for image in images.values() for f in image["functions"]),
        "new_registered_entries_missing_legacy_unique_images": sum(
            image["summary"]["registered_entries_missing_legacy"] for image in images.values()),
        "legacy_path_byte_coverage_percent": round(100*sum_paths("legacy_candidate_instruction_bytes")/total_code, 5),
        "new_path_byte_coverage_percent": round(100*sum_paths("recovered_candidate_instruction_bytes")/total_code, 5),
        "semantic_evidence_unique_candidates": sum(f["semantic_status"] != "unknown" for f in fs),
        "unknown_unique_candidates": sum(f["semantic_status"] == "unknown" for f in fs),
        "exact_local_facts_unique_candidates": sum(f["semantic_status"] == "exact-local-instruction-fact" for f in fs),
        "ordinary_KCS_slots": len({k[1] for k in provenance if k[0] == "ordinary"}),
        "motion_KCS_slots": len({k[1] for k in provenance if k[0] == "motion"}),
        "particle_KCS_slots": len({k[1] for k in provenance if k[0] == "particle"}),
        "ordinary_native_source_status_counts": dict(Counter(
            by_index.get(k[1], {}).get("status", "absent-from-route-report")
            for k in provenance if k[0] == "ordinary")),
        "candidates_with_decoded_instructions": sum(bool(f["instruction_count"]) for f in fs),
        "zero_instruction_rejected_seeds": sum(not f["instruction_count"] for f in fs),
        "exact_CFG_hash_groups": len({f["exact_CFG_hash"] for f in fs if f["instruction_count"]}),
        "normalized_shape_hash_groups": len({f["normalized_shape_hash"] for f in fs if f["instruction_count"]}),
        "semantic_implementations_claimed": 0}
    handoff = [
        {"topic": "ordinary/7 differs from motion/particle",
         "conclusion": "The Yukikoi registered ordinary table slot 7 reads DWORD task+0x48 into *return_value, EAX=1, ret 16; it reads no script arguments. Local KCS provenance has no ordinary/7 calls. Motion/7 has 8-byte atan2(y,x); particle/7 has 4-byte cos. These identities must remain separate.",
         "evidence": "Yukikoi ordinary table RVA 0x6270e8; slot RVA 0x191560; setter RVA 0x493090. Fixed reference docs/kcs.md motion/particle sections."},
        {"topic": "1063 concrete ABI",
         "conclusion": "Yukikoi wrapper resolves args[0] destination and args[1] source through VM resolver; calls helper(source,destination,256); EAX=2; ret 16, no result-slot write. Fixed reference specifies CP932->UTF8, one terminator, >255 output bytes preserves destination, aliasing supported, Windows extension mappings.",
         "evidence": "Yukikoi RVA 0x1921c0; resolver 0x492f20; conversion helper 0x1fef30; reference docs/kcs.md UTF-8 string section."},
        {"topic": "124 high-frequency gap",
         "conclusion": "Yukikoi wrapper formats args[1], dispatches target=args[0], command104, data=[0,text,0], EAX2. Targeted RTTI recovery identifies old kcTaskStringPlane receiver: command104 appends text records then conditionally prepares/renders. Full parser/configuration remains partial; see recovery_A_20261116_contracts.",
         "evidence": "Wrapper0x1984c0; old StringPlane vtable0x4e8eb8, handler0x13c660, command104 block0x13cae1, append helper0x13db70."},
        {"topic": "934 high-frequency gap",
         "conclusion": "CST registration alias: args=(context,source-name-format,new-name-format), state2. Original wrapper guards null context and context+4 before formatting two names into 128-byte buffers; invokes context method with formatted names. Fixed reference source finds an existing source, requires destination absent, appends new name with copied value and empty details/saved data.",
         "evidence": "Yukikoi RVA 0x1a1910; method 0x28b550. resources/reference/KatzeSystem2/src/runtime/cst.rs:286-314 (fixed source contract; no copied code)."},
        {"topic": "125 adjacent command",
         "conclusion": "One DWORD target; sends command105, three zero data items, EAX2, result slot preserved. Old StringPlane command105 clears text/pixels and resets playback state; newer StringPlane2 uses a separate protocol.",
         "evidence": "Wrapper0x197e10; RTTI old StringPlane handler0x13c660; command105 block0x13cb1d. See receiver-contract report."}]
    source_files = ["resources/reference/README.md",
                    "resources/reference/KatzeSystem2/docs/kcs.md",
                    "resources/reference/KatzeSystem2/src/runtime/cst.rs",
                    "D:/MajiroWorkspace/scripts/analysis/audit_remajiro.py",
                    "D:/MajiroWorkspace/reports/remajiro_functions.json"]
    payload = {"schema": "cs2-recovery-A/v1", "status": "read-only-static-evidence",
               "generated_at_utc": datetime.now(timezone.utc).isoformat(),
               "source_hashes": source_hashes,
               "reference_sources": [{"path": p, "sha256": sha((ROOT/p).read_bytes())}
                                     for p in source_files if (ROOT/p).is_file()],
               "reference_policy": "fixed local snapshot contracts; no network, runtime invocation or implementation code copying",
               "Majiro_method": "SHA-bound original dispatch + table-specific usage + source provider evidence; registration never equals semantics",
               "summary": summary, "binaries": binaries, "images": images,
               "handoff": handoff,
               "supplementary_reports": ["reports/recovery_A_20261116_native.json",
                                         "reports/recovery_A_20261116_contracts.json",
                                         "reports/recovery_A_20261116_loading_contracts.json",
                                         "reports/recovery_A_20261116_summary.json"],
               "semantic_evidence_count_scope": "ledger snapshot, excluding targeted receiver follow-up; lower bound, not implementation count",
               "runtime_source_snapshot": {"source": str(args.native_coverage.relative_to(ROOT)),
                                           "status_counts": dict(Counter(n["status"] for n in native["natives"])),
                                           "interpretation": "existing source routing evidence; separate from PE candidate semantics"},
               "kcs_provenance_source": {"sha256": source_hashes[str(args.provenance.relative_to(ROOT))],
                                         "summary": provenance_payload["summary"]},
               "focused_KCS_calls": [{"table_hint": key[0], "index": key[1],
                                     "argument_bytes_counts": dict(usage_counts[key]),
                                     "calls": calls}
                                    for key, calls in provenance.items() if key[1] in FOCUS],
               "identity_groups": {
                   "byte_identical_PE_paths": [list(group) for group in (
                       [b["binary_path"] for b in binaries if b["sha256"] == digest]
                       for digest in images) if len(group) > 1],
                   "normalized_shape_policy": "diagnostic similarity only; masking addresses can merge semantically different functions"},
               "limitations": ["Native table categories are shape hypotheses until each task factory is traced.",
                               "All function boundaries remain static candidates; unresolved indirect jumps explicit.",
                               "No exact denominator for all native engine functions or semantic completion percentage.",
                               "Some semantic evidence is not complete semantic evidence.",
                               "Static candidate code coverage is not source implementation coverage."]}
    out_json = args.output_prefix.with_suffix(".json")
    out_md = args.output_prefix.with_suffix(".md")
    out_json.write_text(json.dumps(payload, ensure_ascii=False, separators=(",", ":"))+"\n", encoding="utf-8")
    out_md.write_text(render(payload), encoding="utf-8")
    write_summary(payload, args.output_prefix)
    print(json.dumps(summary, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())

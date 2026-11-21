"""Static evidence for the CSS-to-task restore wrapper, ordinary576.

Do not stop at the first return: older images place the nonzero-handle path
after their zero-handle return. No recovered function or VM is executed.
"""
from __future__ import annotations

import hashlib
import json
from pathlib import Path
import re
import struct
import zlib

import capstone
import pefile

from task_native_descriptors import wrapper
from audit_runtime_evidence import kif_entries, operand_count

ROOT = Path(__file__).resolve().parents[2]
OUTPUT = ROOT / "reports/css_task_576_F_contracts_20261117.json"


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def code_fragment(pe, rva, length):
    raw = pe.get_data(rva, length)
    decoder = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_32)
    return {"rva": rva, "bytes": raw.hex(), "instructions": [
        {"rva": ins.address - pe.OPTIONAL_HEADER.ImageBase,
         "bytes": ins.bytes.hex(), "mnemonic": ins.mnemonic, "operands": ins.op_str}
        for ins in decoder.disasm(raw, pe.OPTIONAL_HEADER.ImageBase + rva)
    ]}


def primary_vtables(pe):
    """Report RTTI-backed task entrypoints, without inferring message support."""
    image = pe.get_memory_mapped_image()
    base = pe.OPTIONAL_HEADER.ImageBase
    u32 = lambda off: struct.unpack_from("<I", image, off)[0]
    results = []
    for name in re.finditer(rb"\.\?AVkcTask[^\0]*\0", image):
        td = base + name.start() - 8
        for ref in re.finditer(re.escape(struct.pack("<I", td)), image):
            col = ref.start() - 12
            if col < 0 or any(u32(col + offset) != 0 for offset in (0, 4, 8)):
                continue
            for vt_ref in re.finditer(re.escape(struct.pack("<I", base + col)), image):
                vt = vt_ref.start() + 4
                pointers = [u32(vt + index * 4) for index in range(8)]
                if not all(any(section.Characteristics & 0x20000000
                               and section.VirtualAddress <= value - base
                               < section.VirtualAddress + section.Misc_VirtualSize
                               for section in pe.sections) for value in pointers):
                    continue
                results.append({"class": name.group()[:-1].decode("ascii"),
                                "vtable_rva": vt,
                                "methods_rva": [value - base for value in pointers]})
    return results


def null_factory43(pe, tables):
    """Follow the Top's type-one receiver; never select a numeric switch globally."""
    base = pe.OPTIONAL_HEADER.ImageBase
    top = [table for table in tables if table["class"] == ".?AVkcTaskTop@@"]
    if len(top) != 1:
        raise ValueError("Top RTTI identity is ambiguous")
    route = top[0]["methods_rva"][5]
    decoder = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_32)
    route_items = list(decoder.disasm(pe.get_data(route, 80), base + route))
    first_call = next(item for item in route_items if item.mnemonic == "call")
    factory = int(first_call.op_str, 16) - base
    factory_items = list(decoder.disasm(pe.get_data(factory, 180), base + factory))
    jump = next(item for item in factory_items if item.mnemonic == "jmp"
                and "*4 + " in item.op_str)
    table_va = int(jump.op_str.split("+ ")[1].rstrip("]"), 16)
    target = struct.unpack("<I", pe.get_data(table_va - base + 43 * 4, 4))[0] - base
    tail = []
    for item in decoder.disasm(pe.get_data(target, 40), base + target):
        tail.append(item)
        if item.mnemonic == "ret":
            break
    # Four independently reviewed compiler epilogues. Exact bytes prevent
    # treating arbitrary writes, callbacks or later branches as a null case.
    encoded = b"".join(item.bytes for item in tail).hex()
    if encoded not in {
        "5f5e5d33c05b59c20c00",
        "5f5e33c05b8be55dc20c00",
        "33c08b4df464890d00000000595f5e5b8be55dc20c00",
        "33c08b4c241864890d00000000595f5e5d5b83c410c20c00",
    }:
        raise ValueError("kind43 is not a reviewed pure-null epilogue")
    return {
        "top_vtable_rva": top[0]["vtable_rva"],
        "top_type1_route": code_fragment(pe, route, first_call.address + first_call.size - base - route),
        "factory_prefix": code_fragment(pe, factory, jump.address + jump.size - base - factory),
        "factory_table_rva": table_va - base,
        "kind43_null": code_fragment(pe, target, sum(item.size for item in tail)),
        "interpretation": "This exact native66 factory creates no kind43 object; not a constant576 proof.",
    }


def script_provenance():
    """Decode literal callsites only; do not evaluate script instructions."""
    path = ROOT / "cs2_full_v401/system/kcs.int"
    entries, _, cipher = kif_entries(path, 0x16580A97)
    if cipher is not None:
        raise ValueError("the fixed script anchor is no longer a plain archive")
    entry = next(item for item in entries if item["name"].lower() == b"sscript.kcs")
    with path.open("rb") as stream:
        stream.seek(entry["offset"])
        data = stream.read(entry["size"])
    body = zlib.decompress(data[52:])
    code_length = struct.unpack_from("<I", data, 16)[0]
    rows = []
    at = 0
    while at < code_length:
        opcode = body[at]
        count = operand_count(opcode)
        if count is None:
            raise ValueError("unknown static KCS opcode")
        rows.append({"offset": at, "opcode": opcode, "operands": [
            {"flags": body[at + 1 + index * 5],
             "value": struct.unpack_from("<I", body, at + 2 + index * 5)[0]}
            for index in range(count)]})
        at += 1 + count * 5
    windows = []
    for anchor, before, after in [(692745, 8, 1), (715604, 7, 1), (745683, 3, 3)]:
        index = next(index for index, row in enumerate(rows) if row["offset"] == anchor)
        windows.append({"anchor": anchor, "instructions": rows[index-before:index+after+1]})
    return {"resource": "cs2_full_v401/system/kcs.int::sscript.kcs",
            "sha256": hashlib.sha256(data).hexdigest(), "callsites": windows,
            "interpretation": "Native66 kind43 -> field171164+4; both575/576 use that field as target and record13. CSS is the local argument, not field171164+4."}


def legacy_receivers(ledger):
    anchors = [
        {"sha256": "659d5f069f3c97f69a61e12449cf1510b45e48a10649b9ac6977c2b93d6d28ad",
         "table_va": 0x6E6100, "registration_va": 0x430178,
         "wrapper_va": 0x454520, "command": 366, "kind": 42,
         "factory_table_va": 0x418E50, "constructor_va": 0x41ED00,
         "vtable_va": 0x5D7FD0, "handler_va": 0x41EF20,
         "handler_slot": 6, "available_offset": 0x270,
         "first_command": 332, "message_table_va": 0x41FD3C,
         "zero_leaf_va": 0x41FC6E, "default_va": 0x41FD1D},
        {"sha256": "a9f8b08713aac278671853b2293a13b2623a122ede9341cb921901b09601cb52",
         "table_va": 0x69DE80, "registration_va": 0x430768,
         "wrapper_va": 0x457EF0, "command": 372, "kind": 42,
         "factory_table_va": 0x418944, "constructor_va": 0x41E840,
         "vtable_va": None, "handler_va": 0x41EA60,
         "handler_slot": 7, "available_offset": 0x27C,
         "first_command": 338, "message_table_va": 0x41F880,
         "zero_leaf_va": 0x41F7B1, "default_va": 0x41F860},
    ]
    results = []
    for anchor in anchors:
        binary = next(row for row in ledger["binaries"] if row["sha256"] == anchor["sha256"])
        path = ROOT / binary["binary_path"]
        if digest(path) != anchor["sha256"]:
            raise ValueError("legacy receiver image changed")
        pe = pefile.PE(str(path))
        base = pe.OPTIONAL_HEADER.ImageBase
        read = lambda va: struct.unpack("<I", pe.get_data(va - base, 4))[0]
        if read(anchor["table_va"] + 576*4) != anchor["wrapper_va"]:
            raise ValueError("legacy576 table entry changed")
        if pe.get_data(anchor["registration_va"] - base, 7) != b"\xc7\x40\x38" + struct.pack("<I", anchor["table_va"]):
            raise ValueError("legacy ordinary VM table publication changed")
        restored = wrapper(pe, anchor["wrapper_va"] - base)
        if restored["command"] != anchor["command"]:
            raise ValueError("legacy576 wrapper command changed")
        leaf = read(anchor["message_table_va"] + (anchor["command"] - anchor["first_command"])*4)
        if leaf != anchor["zero_leaf_va"] or pe.get_data(leaf - base, 9) != bytes.fromhex("8b451cc70000000000"):
            raise ValueError("legacy576 explicit zero-output leaf changed")
        if anchor["vtable_va"] is None:
            table = next(row for row in primary_vtables(pe) if row["class"] == ".?AVkcTask3DView@@")
            anchor["vtable_va"] = base + table["vtable_rva"]
        if read(anchor["vtable_va"] + anchor["handler_slot"]*4) != anchor["handler_va"]:
            raise ValueError("legacy3D concrete virtual receiver changed")
        factory_case = read(anchor["factory_table_va"] + anchor["kind"]*4)
        result = dict(anchor, path=binary["binary_path"], restore576=restored)
        result["fragments"] = [code_fragment(pe, va-base, size) for va, size in [
            (anchor["registration_va"], 7), (factory_case, 0x36),
            (anchor["constructor_va"], 0x100), (anchor["handler_va"], 0x5D),
            (leaf, 0x23), (anchor["default_va"], 0x1B),
        ]]
        result["interpretation"] = (
            "Available concrete kind42 explicitly rejects this restore with output0, without reading body. "
            "Unavailable object takes generic propagation and does not write output. "
            "Neither is a configuration parser or evidence for modern437/426; the original3D availability producer must also be recovered."
        )
        results.append(result)
    return results


def supplemental_wrappers(ledger):
    """Older VM table publications are independent of the 12-table catalog."""
    anchors = [
        ("f6af7437475b8c066be973245aa79688bea510ca1192264add4f1914ebee4c0e", 0x75C6F0, 0x4532C9, 0x484D90, 393),
        ("012c71635932c241cde4df986377516279a50b907166757e89ea7b86ff3f813f", 0x761A78, 0x453989, 0x4857C0, 395),
        ("9f3a5ad0ed22074420685f1e2a3bc92436d68289c7bac854d5f7d7331dc3ef67", 0x73E578, 0x453439, 0x484DE0, 389),
        ("72c9c1e7335a92bc2f1c731275a8da34d638d1730b9d5003fb6a8e85fe5b15c8", 0x834F80, 0x42EC98, 0x44F740, 341),
    ]
    results = []
    for sha, table, registration, entry, command in anchors:
        binary = next(row for row in ledger["binaries"] if row["sha256"] == sha)
        path = ROOT / binary["binary_path"]
        if digest(path) != sha:
            raise ValueError("supplemental576 image changed")
        pe = pefile.PE(str(path))
        base = pe.OPTIONAL_HEADER.ImageBase
        if pe.get_data(registration-base, 7) != b"\xc7\x40\x38" + struct.pack("<I", table):
            raise ValueError("supplemental ordinary VM publication changed")
        if struct.unpack("<I", pe.get_data(table-base+576*4, 4))[0] != entry:
            raise ValueError("supplemental576 pointer changed")
        restored = wrapper(pe, entry-base)
        if restored["command"] != command:
            raise ValueError("supplemental576 command changed")
        row = {"sha256": sha, "path": binary["binary_path"], "table_va": table,
               "registration": code_fragment(pe, registration-base, 7), "restore576": restored}
        if sha != anchors[-1][0]:
            row["kind43_factory"] = null_factory43(pe, primary_vtables(pe))
        else:
            row["kind43_factory"] = None
            row["limit"] = "RTTI-free legacy identity; no kind number or factory result inferred."
        results.append(row)
    return results


def legacy_material_receiver(ledger):
    """Pin the RTTI-free kind42 body reader without extending its ABI to437."""
    sha = "72c9c1e7335a92bc2f1c731275a8da34d638d1730b9d5003fb6a8e85fe5b15c8"
    binary = next(row for row in ledger["binaries"] if row["sha256"] == sha)
    path = ROOT / binary["binary_path"]
    if digest(path) != sha:
        raise ValueError("legacy material image changed")
    pe = pefile.PE(str(path))
    base = pe.OPTIONAL_HEADER.ImageBase
    for table, index, target in [
        (0x418C10, 42, 0x418AF0),
        (0x6F7760, 6, 0x41E680),
        (0x41EF54, 341 - 315, 0x41EED1),
        (0x41EF54, 342 - 315, 0x41EF04),
    ]:
        if struct.unpack("<I", pe.get_data(table - base + index * 4, 4))[0] != target:
            raise ValueError("legacy material dispatch identity changed")
    return {
        "sha256": sha, "path": binary["binary_path"], "kind": 42,
        "factory_va": 0x418290, "factory_table_va": 0x418C10,
        "constructor_va": 0x41E4A0, "vtable_va": 0x6F7760,
        "handler_va": 0x41E680, "handler_slot": 6,
        "available_offset": 0x70, "material_owner_offset": 0x68,
        "restore_command": 341, "capture_command": 342,
        "record_size": 0x824, "base_name_offset": 4,
        "binding_offset": 0x24, "binding_count": 32, "binding_stride": 0x40,
        "name_slot_bytes": 32, "image_slot_bytes": 32,
        "fragments": [code_fragment(pe, va - base, length) for va, length in [
            (0x418290, 0x4C), (0x418AF0, 0x31), (0x41E4A0, 0xEE),
            (0x41E680, 0x55), (0x41EED1, 0x67),
            (0x486A80, 0x17F), (0x486C00, 0x183),
            (0x486460, 0x240), (0x486710, 0x15A),
        ]],
        "observations": [
            "Constructor available requires a real graphics scene and registration; no CPU substitute is inferred.",
            "Receiver explicitly clears out before testing material owner at+0x68; no owner returns zero.",
            "Body consumer clears the current base texture and all named bindings BEFORE testing size DWORD==0x824.",
            "Size mismatch returns zero after those clears; it is not a pure validation failure.",
            "Base name starts+4. Each of32 slots starts+0x24+index*0x40; empty name skips that slot only.",
            "Nonempty slots pass raw name and image strings through the real binding loader; no codepage is inferred.",
            "Capture zero-fills0x824, writes its size, base name and at most32 name/image pairs.",
            "This legacy reader is evidence for341 only, not for modern437/426 or the366/372 reject leaves.",
        ],
        "body_reader_recovered": True, "host_consumer_implemented": False,
        "remaining": "No genuine kind42 scene/model/material owner is installed in the host. Do not create one just to consume this record.",
    }


def main():
    ledger = json.loads((ROOT / "reports/recovery_A_20261116.json").read_text(encoding="utf-8"))
    seen = set()
    profiles = []
    for binary in ledger["binaries"]:
        sha = binary["sha256"]
        if sha in seen:
            continue
        tables = [table for table in ledger["images"][sha]["tables"]
                  if table["table_hint"] == "ordinary" and len(table["slots"]) > 576]
        if not tables:
            continue
        if len(tables) != 1:
            raise ValueError("ordinary table identity is ambiguous")
        seen.add(sha)
        path = ROOT / binary["binary_path"]
        if digest(path) != sha:
            raise ValueError("registered PE changed")
        pe = pefile.PE(str(path))
        table = tables[0]
        slot_rva = table["slots"][576]["entry_rva"]
        pointer = struct.unpack("<I", pe.get_data(table["table_rva"] + 576 * 4, 4))[0]
        if pointer != pe.OPTIONAL_HEADER.ImageBase + slot_rva:
            raise ValueError("ordinary576 registration changed")
        restore = wrapper(pe, slot_rva)
        # Record575 is evidence of the retained zero producer only; pairing
        #575 and576 or assuming that a CSS record can never exist is not proved.
        producer = code_fragment(pe, table["slots"][575]["entry_rva"], 18)
        tables = primary_vtables(pe)
        profiles.append({"path": binary["binary_path"], "sha256": sha,
                         "file_version": binary["file_version"],
                         "table_rva": table["table_rva"],
                         "restore576": restore, "neighbor575": producer,
                         "kind43_factory": null_factory43(pe, tables)})
    plugins = []
    seen.clear()
    for path in sorted(ROOT.rglob("cs2conf*.dll")):
        sha = digest(path)
        if sha in seen:
            continue
        seen.add(sha)
        pe = pefile.PE(str(path))
        image = pe.get_memory_mapped_image()
        plugins.append({"path": path.relative_to(ROOT).as_posix(), "sha256": sha,
                        "exports": [item.name.decode("ascii") for item in
                                    pe.DIRECTORY_ENTRY_EXPORT.symbols if item.name],
                        "relevant_rtti": [match.group()[:-1].decode("ascii", "replace")
                                          for match in re.finditer(rb"\.\?AV[^\0]{2,100}\0", image)
                                          if any(word in match.group().lower() for word in
                                                 (b"task", b"conf", b"option"))],
                        "interpretation": "inventory only; no receiver absence inferred from RTTI"})
    current = next(row for row in profiles if row["sha256"] ==
                   "63de0e14cd8ce11ad2d68e7ed6076641ff5e5b73d822603cc52764009d812976")
    pe = pefile.PE(str(ROOT / current["path"]))
    payload = {"schema": "cs2-css-task576-static/v1", "profiles": profiles,
               "script_provenance": script_provenance(),
               "legacy_receivers": legacy_receivers(ledger),
               "legacy_material_receiver": legacy_material_receiver(ledger),
               "supplemental_wrappers": supplemental_wrappers(ledger),
               "current_task_vtables": primary_vtables(pe), "configuration_plugins": plugins,
               "current_reviewed_fragments": [code_fragment(pe, rva, length) for rva, length in [
                   (0x134F00, 8), (0x136540, 0x1C), (0x136636, 5),
                   (0x1405FF, 0x28), (0x141088, 0x15),
                   (0x17E7FD, 0x2F), (0x17F7A1, 0x16),
                   (0x17A5E0, 0x1F), (0x2EAC50, 0x90),
                   (0x1352B0, 0xDE), (0x135178, 0x21), (0x134E90, 0x40),
                   (0x134BE0, 0x29), (0x134E26, 0x1B), (0x136700, 0x47),
               ]],
               "current_output_alias": {
                   "wrapper_va": 0x59C810,
                   "record_length_slot": "entryESP-4, passed to6eac50; not passed to task",
                   "task_output_slot": "entryESP+12, aliases the native argument-pointer stack slot",
                   "task_output_address_instruction_va": 0x59C832,
                   "check_output_instruction_va": 0x59C84C,
                   "interpretation": "No zero initialization before dispatch; generic routing/propagate result is not this output DWORD. Do not invent a zero output.",
               },
               "limits": [
                   "576 has a nonzero CSS-handle path in all twelve registered images reviewed.",
                   "Message codes are wrapper identities, not evidence of receiving object or record ABI.",
                   "No modern437/426 body-reading receiver was recovered; a separate exact legacy341 material reader is now pinned, while366/372 explicitly reject.",
                   "All twelve identified native66 factories have a pure-null kind43 case. This does not prove576 constant for arbitrary targets or externally supplied records.",
                   "RTTI inventories and immediate searches do not prove the absence of an external receiver.",
                   "No XML field layout, record encoding or script message fallback is inferred.",
               ], "compiler_invoked": False, "tests_run": False,
               "original_functions_executed": False, "vm_executed": False}
    OUTPUT.write_text(json.dumps(payload, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"report": OUTPUT.relative_to(ROOT).as_posix(),
                      "profiles": len(profiles), "plugins": len(plugins),
                      "supplemental_wrappers": len(payload["supplemental_wrappers"]),
                      "legacy_explicit_reject_receivers": len(payload["legacy_receivers"]),
                      "body_reading_receiver_recovered": True,
                      "modern_body_reading_receiver_recovered": False}))


if __name__ == "__main__":
    main()

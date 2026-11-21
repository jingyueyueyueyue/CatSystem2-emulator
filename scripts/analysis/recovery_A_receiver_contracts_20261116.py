"""Bounded, read-only receiver evidence for ordinary 647, 934 and 124.

Addresses below are evidence anchors for the SHA-bound Yukikoi 2.6.1.90 image,
not portable symbols. The companion JSON carries the original bytes, imports,
decoded basic blocks, source hashes, and KCS call provenance.
"""
from __future__ import annotations

from datetime import datetime, timezone
import json
from pathlib import Path
import re
import struct
import sys
import zlib

from recovery_A_20261116 import ROOT, Image, instruction_row, read_json, sha
from audit_runtime_evidence import kif_entries, operand_count


def block(image, start, size):
    instructions = image.insns(start, size)
    # A snippet is never presented as a whole recovered function.
    return {"start_rva": start, "byte_length_requested": size,
            "raw_sha256": sha(image.get(start, size)),
            "bytes": image.get(start, size).hex(),
            "status": "bounded-instruction-snippet",
            "instructions": [instruction_row(i, image.base) for i in instructions]}


def source_excerpt(relative, first, last):
    path = ROOT/relative
    text = path.read_text(encoding="utf-8")
    return {"path": relative, "sha256": sha(path.read_bytes()),
            "first_line": first, "last_line": last,
            "text": "\n".join(text.splitlines()[first-1:last])}


def rtti_vtable(image, class_name):
    """Follow MSVC x86 TypeDescriptor -> COL -> primary vftable."""
    needle = (".?AV" + class_name + "@@\0").encode("ascii")
    results = []
    for s in image.sections:
        for found in re.finditer(re.escape(needle), s["data"]):
            name_rva = s["rva"] + found.start()
            descriptor = name_rva - 8
            ptr = struct.pack("<I", image.base+descriptor)
            for rs in image.sections:
                if rs["executable"]:
                    continue
                for ref in re.finditer(re.escape(ptr), rs["data"]):
                    ref_rva = rs["rva"] + ref.start()
                    col = ref_rva - 12
                    if image.u32(col) != 0 or image.u32(col+4) != 0:
                        continue
                    col_ptr = struct.pack("<I", image.base+col)
                    for vs in image.sections:
                        if vs["executable"]:
                            continue
                        for vref in re.finditer(re.escape(col_ptr), vs["data"]):
                            vt = vs["rva"] + vref.start() + 4
                            pointers = [image.u32(vt+4*n) for n in range(8)]
                            if all(value is not None and image.code(value-image.base)
                                   for value in pointers):
                                results.append({
                                    "class_name": class_name, "decorated_name_RVA": name_rva,
                                    "TypeDescriptor_RVA": descriptor, "COL_RVA": col,
                                    "vtable_RVA": vt,
                                    "first_8_virtual_method_RVAs": [v-image.base for v in pointers],
                                    "command_handler_RVA": pointers[7]-image.base,
                                    "status": "MSVC-x86-primary-RTTI-pointer-chain"})
    return results


def script_context():
    """Read one plain archive member; retain only bounded callsite context."""
    path = ROOT/"cs2_full_v401/system/kcs.int"
    entries, kind, cipher = kif_entries(path, 0x16580A97)
    if cipher:
        raise ValueError("The v401 evidence anchor unexpectedly became encrypted")
    entry = next(e for e in entries if e["name"].lower() == b"sscript.kcs")
    with path.open("rb") as handle:
        handle.seek(entry["offset"])
        data = handle.read(entry["size"])
    body = zlib.decompress(data[52:])
    length = struct.unpack_from("<I", data, 16)[0]
    n = struct.unpack_from("<I", data, 24)[0]
    init = body[length:]
    strings = {}
    for k in range(n):
        src, dst = struct.unpack_from("<II", init, k*8)
        strings[dst] = init[src:].split(b"\0", 1)[0]
    code, rows, pc = body[:length], [], 0
    while pc < len(code):
        off, op = pc, code[pc]
        count = operand_count(op)
        if count is None:
            raise ValueError(f"Unsupported opcode at {off:x}")
        operands = []
        for k in range(count):
            flags, value = struct.unpack_from("<BI", code, pc+1+5*k)
            literal = strings.get(value) if flags == 3 else None
            operands.append({"flags": flags, "value": value,
                             "literal_hex": literal.hex() if literal else None,
                             "literal_cp932": literal.decode("cp932", "replace") if literal else None})
        pc += 1+5*count
        rows.append({"offset": off, "opcode": op, "operands": operands})
    calls = []
    for index, row in enumerate(rows):
        if row["opcode"] == 0x2B and row["operands"][0] == {
                "flags": 3, "value": 124, "literal_hex": None, "literal_cp932": None}:
            calls.append({"offset": row["offset"],
                          "prior_instructions": rows[max(0, index-8):index+1]})
    return {"resource": "system/kcs.int::sscript.kcs", "program_sha256": sha(data),
            "call_count": len(calls), "first_calls": calls[:8],
            "related_string_literals": [
                {"VM_address": a, "bytes": value.hex(), "text": value.decode("cp932", "replace")}
                for a, value in strings.items() if value in (b"Image", b"Plane", b"M2Plane")
                or b"%s[L52]" in value][:8],
            "interpretation": "These callers format Image/Plane/M2Plane labels; receiver identity is separately proved by RTTI and the command104 jump-table chain."}


def loading_and_construction_contracts(image, binary):
    """Fixed anchors only: no additional global function recovery or CFG sweep."""
    factory = 0x136428
    assert image.u32(factory+8*4)-image.base == 0x13588D
    assert image.u32(factory+34*4)-image.base == 0x135FBA
    assert image.u32(factory+48*4)-image.base == 0x136179
    assert image.u32(factory+63*4)-image.base == 0x136367
    classes = []
    for name in ["kcTaskStringPlane", "kcTaskFEScript", "kcTaskStringPlane2",
                 "kcTaskStringPlaneUTF8"]:
        classes.extend(rtti_vtable(image, name))
    commands = {}
    for code in [337, 338, 339, 340, 343, 346, 347, 348, 357, 358]:
        if code == 337:
            ptr_rva, target = None, 0x17F687
        else:
            ptr_rva = 0x17FE5C+(code-338)*4
            target = image.u32(ptr_rva)-image.base
        commands[code] = {"code": code, "pointer_slot_RVA": ptr_rva,
                          "receiver_block_RVA": target,
                          "snippet": block(image, target, 290 if code in {337, 340} else 95)}
    assert commands[338]["receiver_block_RVA"] == 0x17F7B7
    assert commands[339]["receiver_block_RVA"] == 0x17F80E
    assert commands[340]["receiver_block_RVA"] == 0x17F857
    # Exact old-text command102 mapping from the secondary byte table.
    old_index = image.get(0x13D028+102-3, 1)[0]
    old_target = image.u32(0x13CFC0+old_index*4)-image.base
    assert old_target == 0x13C6B0
    old_fields = [
        (0x00, "i32", "canvas_position_x", "converted to f32 in plane command51"),
        (0x04, "i32", "canvas_position_y", "converted to f32 in plane command51"),
        (0x08, "i32", "pixel_width", "image allocation and plane rectangle command41"),
        (0x0C, "i32", "pixel_height", "image allocation and plane rectangle command41"),
        (0x10, "u32", "image_create_parameter_raw", "third argument to image create helper; semantic name unproved"),
        (0x14, "u32", "submit_pixels_raw", "nonzero gates canvas submission"),
        (0x18, "u32", "base_character_size", "copied into five per-style size fields"),
        (0x1C, "u32", "raw_1c", "preserve raw; not fully named"),
        (0x20, "u32", "raw_20", "used with base size in indentation branch"),
        (0x24, "u32", "character_spacing_raw", "added to base size by record writer"),
        (0x28, "u32", "raw_28", "preserve raw; not fully named"),
        (0x2C, "u32", "initial_cursor_x_raw", "clear command restores object+0x730 from this field"),
        (0x30, "u32", "initial_cursor_y_raw", "clear command restores object+0x734 from this field"),
        (0x34, "u32", "raw_34", "preserve raw; not fully named"),
        (0x38, "u32", "raw_38", "preserve raw; not fully named"),
        (0x3C, "u32", "character_record_capacity", "must be nonzero; allocate40 bytes per record plus16-byte auxiliary records"),
        (0x40, "u32", "plane_property_raw", "copied to old plane object+0xbc; abstract property name unproved"),
        (0x44, "u32", "playback_mode_raw", "one defers immediate prepare/render after append"),
        (0x48, "u32", "limit_or_timing_raw", "command107 writes its second scalar here"),
        (0x4C, "u32", "post_parse_control_raw", "passed to old parser finalizer0x13bc90"),
        (0x50, "u32", "text_markup_enabled_raw", "gates bracket/backslash parsing in0x13db70"),
        (0x54, "u32", "style_mode_raw", "0..3 select font/style mode branches"),
        (0x58, "u32", "raw_58", "preserve raw; not fully named"),
        (0x5C, "u32", "raw_5c", "preserve raw; not fully named"),
    ]
    native_contracts = [
        {"index": 66, "argument_bytes": 12,
         "words": ["task_type", "parent_task", "task_name_format_VM_address"],
         "return_status": 1,
         "behavior": ["Expand name, send command1 to parent with data0=type and data1=name.",
                      "Convert the created original pointer to a portable task id; zero creation stays failure.",
                      "For a created task send command26 to inherit caller creation-context bytes.",
                      "Type34 selects FES; type8 selects old StringPlane. Construction does not load or start either object."]},
        {"index": 447, "argument_bytes": 8,
         "words": ["FES_target", "name_format_VM_address"], "return_status": 2,
         "preserve_result_slot": True,
         "behavior": ["Expand name from VM, resolve through fes resource category, retain first-NUL complete name (max259 bytes). Empty name still resolves.",
                      "Send337 first with caller shared expression context; portable runtime uses its implicit context instead of a numeric host pointer.",
                      "Then send338 with resolved name in data1 buffer. Both commands use data0=0,data2=0 and target0 still routes.",
                      "337 builds an empty controller with512 local values and registers task callbacks;337 does not read a script.",
                      "338 reads the complete name LooseFirst, parses declarations/properties and prepares child media. It is separate from start.",
                      "Missing file writes target property -2 and keeps the initialized empty context and parameters. Source original return values are ignored by native447.",
                      "On service error keep a resumable operation stage so init/create side effects are not repeated.",
                      "Original formatting failure can leave a global scratch name stale; portable code must diagnose failure, not replay stale global bytes."]},
        {"index": 448, "argument_bytes": 4, "words": ["FES_target"], "return_status": 2,
         "preserve_result_slot": True, "command": {"code": 339, "data": [0, 0, 0]},
         "behavior": ["Require initialized FES controller. Set started flag at object+0x188.",
                      "Obtain parent task elapsed (time-origin), invoke own update with that time and increment0.",
                      "This immediately executes the FES body after updating its clock/children; it does not merely set a bool.",
                      "An empty initialized or failed-load context can start. Preserve pending/resume stage to avoid restarting."]},
        {"index": 449, "argument_bytes": 8, "words": ["FES_target", "image_list_task"],
         "return_status": 2, "preserve_result_slot": True,
         "command": {"code": 340, "data": [0, "image_list_task", 0]},
         "behavior": ["Save list id, query list command291 for the image wrapper and its dimensions.",
                      "Then query command290 for list offsets, save returned offsets; target must be a real reachable type15 image list.",
                      "Binding can precede script load. A child FES may reuse its parent's list. Preserve clipping dimensions and offsets independently."]},
        {"index": 118, "argument_bytes": 4, "words": ["image_wrapper_handle"],
         "return_status": 1,
         "category": "image-ownership-not-old-text-configuration",
         "behavior": ["Zero input writes return0 and skips allocation.",
                      "Allocate new0x218 image wrapper, construct at0x1e1100, invoke clone/copy helper0x1e1200 with source wrapper.",
                      "Write the new wrapper handle and register ownership in caller list at+0xdc.",
                      "The wrapper does not test the clone helper return; helper failure is distinct from a null input.",
                      "This is an image object handle. Do not treat it as task id, text config, or font handle."],
         "remaining": "Pixel/metadata deep-copy and failure-state equivalence are not fully recovered."},
        {"index": 119, "argument_bytes": 12,
         "words": ["image_wrapper_handle", "width_output_VM_address", "height_output_VM_address"],
         "return_status": 2, "preserve_result_slot": True,
         "category": "image-dimensions-not-old-text-configuration",
         "behavior": ["Resolve both output VM addresses, write zero width then zero height before checking image handle.",
                      "Nonzero image uses0x1e1f30->0x1e48a0 to write integer pixel dimensions.",
                      "Zero image still accesses/zeros both outputs. Same output address receives the last height write.",
                      "Typed invalid handle and invalid VM address must diagnose; do not dereference portable numeric handles."]},
        {"index": 123, "argument_bytes": 12,
         "words": ["old_StringPlane_target", "config96_VM_address", "character_table_CP932_VM_address"],
         "return_status": 2, "preserve_result_slot": True,
         "category": "old-type8-configuration-command102",
         "behavior": ["Resolve and copy exactly96 bytes from config address; this old record has no leading size tag.",
                      "Zero a0x468-byte command record, copy config96 to offset0.",
                      "Parse the third argument as raw CP932 character codes, not Unicode: valid two-byte code is stored high-byte-first as u16; a one-byte value is sign-extended to u16.",
                      "Write character count at offset0x60; u16 character codes and terminal0 begin at0x64. Record0x264 is explicitly zero.",
                      "Send command102 with data1 pointing to that record, data0=0,data2=0. No automatic remapping to new commands456/477.",
                      "Receiver rejects null record or zero capacity field at0x3c before replacing existing configuration.",
                      "On valid config it removes old drawing registration, copies0x468 bytes, allocates capacity*40 character records and capacity*16 auxiliary records, resets text count, rebuilds its image surface and fonts.",
                      "Then sends plane rectangle command41 and position command51 (X/Y converted from config integers), optionally submits pixels when offset0x14 is nonzero.",
                      "Native123 performs no explicit character-table bounds check; portable adapter must reject >256 entries. For exactly256 entries the terminal u16 occupies the explicitly zero DWORD at0x264; preserve that zero rather than writing beyond the first table."],
         "record_fields": [{"offset": o, "storage": typ, "name": name, "evidence": why}
                           for o, typ, name, why in old_fields]},
    ]
    snapshot = read_json(ROOT/"reports/kcs_call_inventory_20261116.json")
    observed = {c["index"]: {} for c in native_contracts}
    for game in snapshot["games"]:
        for program in game["programs"]:
            if program["table_hint"] != "ordinary":
                continue
            for call in program["calls"]:
                if call["index"] in observed:
                    values = observed[call["index"]]
                    length = str(call["argument_bytes"])
                    values[length] = values.get(length, 0)+1
    anchors = {
        "factory_dispatch": (0x135750, 100),
        "factory_type8": (0x13588D, 55), "factory_type34": (0x135FBA, 71),
        "factory_type48": (0x136179, 55), "factory_type63": (0x136367, 48),
        "factory_finalization": (0x1363C6, 38),
        "type8_constructor": (0x13BA30, 352), "type34_constructor": (0x17A9A0, 604),
        "native66": (0x193F70, 112),
        "native447": (0x1A4790, 176), "native448": (0x1A4A00, 42),
        "native449": (0x1A4940, 42), "native118": (0x195C30, 208),
        "native119": (0x197290, 72), "native123": (0x1980C0, 320),
        "old_command102_receiver": (0x13C6B0, 1009),
        "image_clone_helper": (0x1E1200, 106), "image_dimensions_helper": (0x1E1F30, 15)}
    out = {"schema": "cs2-recovery-A-loading-construction/v1",
           "generated_at_utc": datetime.now(timezone.utc).isoformat(),
           "status": "read-only-bounded-static-evidence",
           "binary": {"path": binary["binary_path"], "sha256": image.digest,
                      "file_version": "2.6.1.90", "image_base": image.base},
           "RTTI": classes, "factory": {
               "handler_RVA": 0x135750, "type_jump_table_RVA": factory,
               "types": [{"type": 8, "class": "kcTaskStringPlane", "branch_RVA": 0x13588D,
                          "allocation_bytes": 0x774, "constructor_RVA": 0x13BA30},
                         {"type": 34, "class": "kcTaskFEScript", "branch_RVA": 0x135FBA,
                          "allocation_bytes": 0x4B4, "constructor_RVA": 0x17A9A0},
                         {"type": 48, "class": "kcTaskStringPlane2", "branch_RVA": 0x136179,
                          "allocation_bytes": 0x3B0, "constructor_RVA": 0x186ED0},
                         {"type": 63, "class": "kcTaskStringPlaneUTF8", "branch_RVA": 0x136367,
                          "allocation_bytes": 0x40C, "constructor_RVA": 0x187B50}],
               "FES_constructor_raw_parameters": [
                   "allocated_id", "name_bytes", "parent_field48", "parent_field74",
                   "parent_field78", "parent_field6c", "parent_field70", "address_of_parent_field84"],
               "FES_constructor_field_stores": {
                   "0x70": "arg6", "0x74": "arg7", "0x1bc": "arg4", "0x1c0": "arg5",
                   "0x230": "copy string fromarg8", "0x6c": "zero controller",
                   "0x188": "zero started", "0x454": "zero pause"},
               "finalization": "write original task type to object+0x40; set clock using creating parent elapsed; raw fields are not portable pointers"},
           "contracts": native_contracts, "ordinary_KCS_argument_bytes_counts": observed,
           "FES_receiver_commands": list(commands.values()),
           "old_text_config_command102": {"secondary_index": old_index,
                                          "receiver_RVA": old_target,
                                          "record_bytes": 0x468,
                                          "VM_config_bytes": 0x60,
                                          "count_offset": 0x60,
                                          "u16_table_offset": 0x64,
                                          "u16_table_end_exclusive": 0x264,
                                          "zero_flag_offset": 0x264,
                                          "fields": native_contracts[-1]["record_fields"]},
           "snippets": {k: block(image, r, n) for k, (r, n) in anchors.items()},
           "reference_excerpts": [
               source_excerpt("resources/reference/KatzeSystem2/src/runtime/fes/natives.rs", 14, 59),
               source_excerpt("resources/reference/KatzeSystem2/docs/graphics.md", 374, 402),
               source_excerpt("resources/reference/KatzeSystem2/docs/message.md", 328, 344)],
           "limits": ["Type and protocol evidence is bound to the stated PE hash.",
                      "Old96-byte config and new232/300-byte config layouts must remain separate.",
                      "Unnamed raw fields remain unnamed; no successful placeholder implementation.",
                      "No compiler, runtime, game, or emulator was invoked."]}
    prefix = ROOT/"reports/recovery_A_20261116_loading_contracts"
    prefix.with_suffix(".json").write_text(json.dumps(out, ensure_ascii=False, indent=2)+"\n",
                                          encoding="utf-8")
    lines = ["# A: FES loading / old StringPlane construction contracts", "",
             "Generated: " + out["generated_at_utc"], "",
             f"SHA-bound original evidence: `{binary['binary_path']}` / `{image.digest}` / version2.6.1.90.",
             "No compilation, game execution, runtime edits or global CFG expansion.", "",
             "## Real types", "",
             "| Type | RTTI class | Allocation | Factory branch RVA | Constructor RVA |",
             "|---:|---|---:|---|---|"]
    for t in out["factory"]["types"]:
        lines.append(f"| {t['type']} | {t['class']} | `0x{t['allocation_bytes']:x}` | "
                     f"`0x{t['branch_RVA']:08x}` | `0x{t['constructor_RVA']:08x}` |")
    lines += ["", "Factory0x135750 indexes the type table0x136428. Constructors leave FES unloaded/unstarted and old text unconfigured; construction is separate from native447/448 and123.",
              "", "## Native contracts", ""]
    for c in native_contracts:
        lines += [f"### Ordinary {c['index']}", "",
                  f"Words: `{c['words']}`; {c['argument_bytes']} bytes; native status{c['return_status']}.",
                  "Observed ordinary KCS length: `" + str(observed[c["index"]]) + "`.", ""]
        lines += ["- " + text for text in c["behavior"]]
        lines += [""]
    lines += ["## Old configuration96-byte prefix", "",
              "Used by native123/type8 command102. Offsets refer to the caller config, not the newer size-tagged message config.", "",
              "| Offset | Storage | Field / evidence |", "|---|---|---|"]
    for o, typ, name, why in old_fields:
        lines.append(f"| `0x{o:02x}` | {typ} | {name}: {why} |")
    lines += ["", "The wrapper builds0x468 bytes: first0x60 raw config, count at0x60, u16 table at0x64, terminal0, explicit zero flag at0x264. No all-fields meaning is claimed.",
              "", "## FES command receiver RVAs", "",
              "| Command | Receiver block RVA |", "|---:|---|"]
    for c in commands.values():
        lines.append(f"| {c['code']} | `0x{c['receiver_block_RVA']:08x}` |")
    lines += ["", "337 allocates controller0x898;338 reads the completed name;339 starts immediately using parent elapsed;340 queries list291 then290. Their return is a command-routing result, separate from native status2.",
              "", "## Integration handoff", "",
              "- C: real type34/type8 factory registration, initialization and command routing; typed ownership, parent clock and context inheritance.",
              "- B: FES337/338/339/340 staging, declarations/properties/media, immediate start, list clipping/offsets, retries and initialized failed-load context.",
              "- E: type8 command102 record, old123/124/125 path and conditional prepare/render; keep type48/63 newer protocol separate.",
              "- 118/119 belong to image ownership/dimensions. 118 allocates a new wrapper in the caller's owned list;119 resolves and zeros both outputs even for a null source.",
              "", "Script increment stays in scripts/analysis/recovery_A_receiver_contracts_20261116.py; this report and JSON are under reports. No new temporary output or game backups were created or moved."]
    prefix.with_suffix(".md").write_text("\n".join(lines)+"\n", encoding="utf-8")
    print("Wrote", str(prefix).encode("ascii", "backslashreplace").decode())


def main():
    summary = read_json(ROOT/"reports/recovery_A_20261116_summary.json")
    binary = next(b for b in summary["perPE"] if b["binary_path"].startswith("Yukikoi Melt/")
                  and b["binary_path"].endswith(".exe"))
    image = Image(ROOT/binary["binary_path"], binary["sha256"])
    assert image.identity()["file_version"] == "2.6.1.90"
    assert image.get(0x17F7A1, 6) == bytes.fromhex("81c2aefeffff")
    assert image.get(0x17F7B0, 7) == bytes.fromhex("ff24955cfe5700")
    table_rva, code, subtract = 0x17FE5C, 348, 338
    target = image.u32(table_rva + (code-subtract)*4) - image.base
    assert target == 0x17F9F4
    assert image.get(target, 13) == bytes.fromhex("39b754040000740e568bcf89b7")
    old_text_rtti = rtti_vtable(image, "kcTaskStringPlane")
    new_text_rtti = rtti_vtable(image, "kcTaskStringPlane2")
    assert old_text_rtti[0]["command_handler_RVA"] == 0x13C660
    assert new_text_rtti[0]["command_handler_RVA"] == 0x1871C0
    old_command_map = {}
    for command in [104, 105, 106, 107, 108, 109, 110]:
        secondary_index = image.get(0x13D028+command-3, 1)[0]
        dest = image.u32(0x13CFC0+secondary_index*4)-image.base
        old_command_map[command] = {"command": command,
                                    "byte_index_RVA": 0x13D028+command-3,
                                    "secondary_index": secondary_index,
                                    "pointer_slot_RVA": 0x13CFC0+secondary_index*4,
                                    "receiver_block_RVA": dest}
    assert old_command_map[104]["receiver_block_RVA"] == 0x13CAE1
    assert old_command_map[105]["receiver_block_RVA"] == 0x13CB1D
    snippets = {
        "647_native_wrapper": block(image, 0x1A49D0, 43),
        "FES_command_entry": block(image, 0x17E7B0, 126),
        "FES_high_command_dispatch": block(image, 0x17F7A1, 22),
        "348_receiver": block(image, 0x17F9F4, 37),
        "FES_handler_success_epilogue": block(image, 0x17E8B9, 34),
        "FES_sleep_wake_helper_start": block(image, 0x180820, 54),
        "FES_wake_branch": block(image, 0x180AAE, 35),
        "934_native_wrapper": block(image, 0x1A1910, 142),
        "934_alias_method": block(image, 0x28B550, 200),
        "934_name_lookup": block(image, 0x28BA60, 93),
        "124_native_wrapper": block(image, 0x1984C0, 61),
        "125_native_wrapper": block(image, 0x197E10, 39),
        "old_StringPlane_command_entry": block(image, 0x13C660, 80),
        "old_StringPlane_command104": block(image, 0x13CAE1, 60),
        "old_StringPlane_command105": block(image, 0x13CB1D, 248),
        "old_StringPlane_text_parser": block(image, 0x13DB70, 915),
        "old_StringPlane_record_append": block(image, 0x13D0C0, 300),
        "old_StringPlane_prepare_start": block(image, 0x13C070, 140),
        "old_StringPlane_render_start": block(image, 0x13C310, 140),
        "1063_native_wrapper": block(image, 0x1921C0, 56),
        "1063_conversion_helper": block(image, 0x1FEF30, 218),
    }
    source = [
        source_excerpt("resources/reference/KatzeSystem2/src/runtime/cst.rs", 219, 231),
        source_excerpt("resources/reference/KatzeSystem2/src/runtime/cst.rs", 264, 315),
        source_excerpt("resources/reference/KatzeSystem2/src/runtime/fes/task.rs", 1229, 1254),
        source_excerpt("resources/reference/KatzeSystem2/src/runtime/fes/natives.rs", 58, 65),
        source_excerpt("resources/reference/KatzeSystem2/docs/graphics.md", 419, 430),
    ]
    contracts = [
        {"index": 934, "table": "ordinary",
         "parameter_words": ["CST_context_handle", "source_name_format_VM_address",
                             "destination_name_format_VM_address"],
         "argument_bytes": 12, "return_status": 2, "preserve_result_slot": True,
         "category": "receiver-behavior-evidence-not-runtime-implementation",
         "behavior": [
             "A zero context skips both format strings and returns status2. The original also skips context+4==0.",
             "An invalid nonzero portable handle must use the host's existing typed-handle error; never dereference the numeric handle.",
             "Format source then destination independently; truncate each at first NUL. Wrapper buffers are128 bytes.",
             "Search in registration order, with ASCII case-insensitive comparison; an existing entry whose name is empty matches before scanning later entries.",
             "Look up source and destination using that same rule before mutation.",
             "If source is missing or destination already matches, do nothing.",
             "Otherwise append a new registration, copy source unsigned DWORD value, preserve supplied destination spelling, initialize details and saved-data empty.",
             "Do not modify existing source/destination, history table, script, position, language binding, or source metadata.",
             "A successful new name must fit31 bytes plus terminator; reject overflow rather than reproducing native stack overwrite."],
         "important_edge_cases": ["same name in another ASCII case => destination exists => no-op",
                                  "empty-name registration may match both lookups => no-op",
                                  "zero handle => skip malformed format/output addresses",
                                  "overlapping VM source strings are only read; neither string is written",
                                  "source unsigned value must not be recomputed from details/history"],
         "original_addresses_RVA": {"wrapper": 0x1A1910, "alias_method": 0x28B550,
                                    "lookup": 0x28BA60, "case_compare": 0x46628A}},
        {"index": 647, "table": "ordinary",
         "parameter_words": ["target_FES_task", "paused_raw_DWORD"],
         "argument_bytes": 8, "return_status": 2, "preserve_result_slot": True,
         "command": {"code": 348, "data": [0, "paused_raw_DWORD", 0]},
         "category": "receiver-command-chain-and-fixed-reference-contract",
         "behavior": [
             "Always route the full command, including target0; native wrapper preserves result slot and returns2.",
             "FES receiver compares the entire raw DWORD, not its bool conversion. Equal value skips system block/media changes.",
             "If changed, store the raw value first. Nonzero selects _FES_SLEEP_; zero selects _FES_WAKE_. 1->2 is a fresh sleep.",
             "The original helper obtains task elapsed before dispatching the system block; afterward it refreshes objects and applies sleep/wake media state.",
             "Portable receiver must support pending/resume: completed system block effects must not repeat if media setup fails or budget expires.",
             "Refresh in declaration/bank order; hide IMAGE/CIMAGE/BUTTON list entries and PLANE/CPLANE tasks on sleep.",
             "BUTTON enable is sent with command93 before image visibility; wake uses current ENABLE and current DISP, not a snapshot.",
             "SLIDER enable/display use367/366 with propagation1; SCROLL2 has its separate375/379 path.",
             "Force list submission after wake. Do not equate this with the base task clock pause.",
             "Paused body skips first-button creation, input/script/object update; tree clocks and child tasks continue.",
             "Receiver acknowledges success while preserving command output; normal task-tree third-data propagation remains applicable."],
         "original_addresses_RVA": {"wrapper": 0x1A49D0, "FES_handler": 0x17E7B0,
                                    "command_jump_table": table_rva,
                                    "348_branch": target, "sleep_wake_helper": 0x180820,
                                    "pause_raw_field": 0x454, "effects_dirty_field": 0x4AC},
         "dispatch_proof": {"command": code, "table_index": code-subtract,
                            "pointer_slot_RVA": table_rva+(code-subtract)*4,
                            "decoded_target_RVA": target}},
        {"index": 124, "table": "ordinary",
         "parameter_words": ["target_task", "text_format_VM_address"],
         "argument_bytes": 8, "return_status": 2, "preserve_result_slot": True,
         "command": {"code": 104, "data": [0, "formatted_NUL_terminated_bytes", 0]},
         "category": "RTTI-identified-old-StringPlane-receiver-and-parser-evidence",
         "behavior": ["Expand second word using VM format helper, then send command104 to target.",
                      "Full target0 path still formats and routes. No script-return value.",
                      "RTTI identifies kcTaskStringPlane handler0x13c660; its command104 jump-table slot selects0x13cae1.",
                      "Receiver calls text parser/append helper0x13db70 on data1. Existing source text is retained; this is an append operation, not a replacement.",
                      "No input pointer or no allocated record array returns false from the parser. Empty text appends no record. The helper tracks whether any append succeeded.",
                      "The parser preserves CP932 two-byte handling and existing bracket/backslash control grammar; it writes40-byte character records. A plain Rust String append is insufficient for full semantics.",
                      "After successful append, skip immediate prepare/render when object+0x30c==1. Otherwise prepare with count u32::MAX through0x13c070; render through0x13c310 only when preparation succeeds.",
                      "Native125 sends command105 with all-zero data and no text; on this receiver it clears text/pixels and resets counts, playback indices and caches while restoring configured cursor.",
                      "This is an older object protocol. New kcTaskStringPlane2 is a different RTTI identity, with handler0x1871c0 and455-series commands.",
                      "Keep unrecognized/wrong object types diagnostic until their receiver protocol is known; never fabricate success with state2 alone."],
         "receiver_semantics": "append formatted CP932 text to old StringPlane, conditionally prepare/render",
         "original_addresses_RVA": {"wrapper": 0x1984C0, "formatter": 0x492B60,
                                    "native125_wrapper": 0x197E10,
                                    "old_StringPlane_handler": 0x13C660,
                                    "command104_branch": 0x13CAE1,
                                    "command105_branch": 0x13CB1D,
                                    "text_parser_append": 0x13DB70,
                                    "character_record_append": 0x13D0C0,
                                    "prepare": 0x13C070, "render": 0x13C310},
         "dispatch_proof": old_command_map[104],
         "related_command_map": list(old_command_map.values()),
         "remaining": ["full old-object configuration ABI and ownership",
                       "all text-control parser branches and partial append at record-capacity exhaustion",
                       "cross-version old/new object selection and caller resource provenance"]},
    ]
    payload = {"schema": "cs2-recovery-A-receiver-contracts/v1",
               "status": "read-only-static-evidence",
               "generated_at_utc": datetime.now(timezone.utc).isoformat(),
               "binary": {"path": binary["binary_path"], "sha256": image.digest,
                          "file_version": "2.6.1.90", "image_base": image.base},
               "contracts": contracts, "original_snippets": snippets,
               "RTTI_class_identities": old_text_rtti + new_text_rtti,
               "reference_excerpts": source, "ordinary124_KCS_context": script_context(),
               "limits": ["No execution, compilation, runtime tests, source changes or copied reference implementation.",
                          "Raw addresses are version-bound and not portable host pointers.",
                          "Old StringPlane receiver identity and append/clear effects are established; full parser/configuration semantics remain partial."]}
    prefix = ROOT/"reports/recovery_A_20261116_contracts"
    prefix.with_suffix(".json").write_text(json.dumps(payload, ensure_ascii=False, indent=2)+"\n",
                                          encoding="utf-8")
    lines = ["# A receiver contracts for C", "",
             "Read-only evidence; regenerated " + payload["generated_at_utc"] + ".",
             f"Addresses are RVAs for `{binary['binary_path']}`, SHA256 `{image.digest}`, file version2.6.1.90.", ""]
    for c in contracts:
        lines += [f"## Ordinary {c['index']}", "",
                  f"Arguments: {c['parameter_words']} ({c['argument_bytes']} bytes); state2; preserve result slot.",
                  f"Evidence status: `{c['category']}`.", ""]
        lines += ["- " + text for text in c["behavior"]]
        if c.get("important_edge_cases"):
            lines += [""] + ["- Edge: " + text for text in c["important_edge_cases"]]
        lines += ["", "Original RVAs: " + ", ".join(f"{key}=`0x{value:08x}`"
                                                  for key, value in c["original_addresses_RVA"].items()), ""]
    lines += ["## Most material discoveries", "",
              "- 647/348 receiver is now byte-linked: FES handler0x17e7b0 subtracts338, jump-table slot10 at0x17fe884 points to0x17f9f4. It compares/stores raw pause at object+0x454, calls helper0x180820, then writes effects-dirty at+0x4ac. Helper chooses literal _FES_SLEEP_/_FES_WAKE_ and updates objects afterward.",
              "- 934 lookup treats an empty-name entry as a match and compares other names without ASCII case. A normal map insert is not an equivalent implementation.",
              "- 124/125 receiver identity is now established through MSVC RTTI and both jump-table stages: kcTaskStringPlane handler0x13c660, command104->0x13cae1 append, command105->0x13cb1d clear. This belongs to E's message work; do not route it to a generic ordinary no-op or to the newer455-series object without an adapter.",
              "", "## Classification / archive", "",
              "Script is under scripts/analysis; final evidence under reports/recovery_A_20261116_contracts.*. No temporary output, backup changes, localization changes, root README or runtime source modifications."]
    prefix.with_suffix(".md").write_text("\n".join(lines)+"\n", encoding="utf-8")
    print("Wrote", str(prefix).encode("ascii", "backslashreplace").decode())
    loading_and_construction_contracts(image, binary)
    return 0


if __name__ == "__main__":
    sys.exit(main())

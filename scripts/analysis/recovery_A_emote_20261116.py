"""Targeted NEKOPARA E-mote ABI/resource evidence; no original execution.

Reads the 36 nonconstant wrappers, type54 M2Plane command receiver, selected
helper snippets, and two PSZ resources. No global CFG sweep or file extraction.
"""
from __future__ import annotations

from collections import Counter
from datetime import datetime, timezone
import json
from pathlib import Path
import struct
import zlib

from audit_runtime_evidence import kif_entries, blowfish_le_decrypt
from recovery_A_20261116 import ROOT, Image, read_json, recover, sha
from recovery_A_receiver_contracts_20261116 import block, rtti_vtable

SHA = "c71c5b85e6eca660474f156d96b0021a53a5f05690dd077d7861d72ff5b11adc"
COMMANDS = {
    906: [524], 907: [526], 908: [527], 909: [528], 910: [529],
    911: [530], 912: [530], 913: [530], 914: [532], 915: [532], 916: [532],
    917: [533], 918: [535], 919: [536], 920: [538], 921: [539, 538],
    922: [], 923: [], 924: [540], 926: [534], 927: [541], 928: [542],
    929: [543], 930: [544], 931: [545], 932: [], 933: [], 965: [547],
    966: [531], 1025: [548, 549], 1027: [551], 1028: [553],
    1029: [554], 1030: [554], 1047: [552], 1048: [550],
}
FACTS = {
    906: ("resource-load", ["target", "formatted_load_spec_VM", "mode_raw"],
          "524 data0=arg2,data1=formatted arg1. M2Plane invokes manager load0x17d270; it parses colon/comma syntax. Full load-spec-to-resource semantics remain partial."),
    907: ("player-enable", ["target", "enable_raw"],
          "526->manager0x17e3f0->player wrapper0x1744a0. Stores raw request at+0x10c, updates object and controller enables; scene policy can override effective enable."),
    908: ("player-control", ["target"],
          "527->manager0x17e4d0. Full finish/stop semantics require helper recovery."),
    909: ("player-query", ["target"],
          "528 initializes output0; returns1 if manager active flag+0x20, pending child+0x60/+8, or driver virtual slot0xf0 reports active. Otherwise0. Native status1."),
    910: ("vector-query", ["target", "selector_raw", "output_x_VM", "output_y_VM", "output_z_VM"],
          "529 copies three raw DWORDs from helper0x174510 selected vector into resolved VM outputs. Do not assign geometry meaning from length alone."),
    911: ("timeline-scalar", ["target", "scalar_f32_bits"],
          "530 with null selector applies scalar through0x16d980 to all stored records. No guessed time scale or unit conversion."),
    912: ("timeline-scalar", ["target", "selector_format_VM", "scalar_f32_bits"],
          "530 with formatted named selector and float record."),
    913: ("timeline-scalar-batch", ["target", "name_float_table_handle"],
          "Iterates original tree in key order: name at node+0x10,value+0x28; sends530 once per pair. Typed portable table ownership still needs integration."),
    914: ("parameter-scalar", ["target", "scalar_f32_bits"],
          "532 null selector uses0x16dd10, preserves raw f32 and marks dirty when ordered/NaN comparison differs."),
    915: ("parameter-scalar", ["target", "selector_format_VM", "scalar_f32_bits"],
          "532 formatted selector plus float."),
    916: ("parameter-scalar-batch", ["target", "name_float_table_handle"],
          "Iterates original ordered name/value tree and sends532 once per pair."),
    917: ("player-specification", ["target", "formatted_spec_VM", "mode_raw"],
          "533 data0=arg2,data1=formatted arg1; manager0x17db90 then M2Plane refresh. Exact timeline/play interpretation remains partial."),
    918: ("player-raw-property", ["target", "value_raw"],
          "535->0x1745d0 stores DWORD at player+0x134, no clamp. Field name unresolved."),
    919: ("color-percentages", ["target", "red_raw", "green_raw", "blue_raw"],
          "536 copies three DWORDs to M2Plane+0x3a4/+0x3a8/+0x3ac then0x239640 combines percentages with ARGB channels and clips to255."),
    920: ("named-value", ["target", "name_format_VM", "value_raw"],
          "538 data0=formatted name,data1=raw arg2; helper0x1745e0. Detailed value rules unresolved."),
    921: ("named-value-batch", ["target", "name_value_table_handle"],
          "Sends539 clear/reset then538 per ordered pair. Not a host pointer."),
    922: ("global-service", [], "Calls global+0xb9bd98 method0x1807c0 conditionally, status2. Service meaning unresolved."),
    923: ("global-service", [], "Calls global+0xb9bd98 method0x180b30 conditionally, status2. Service meaning unresolved."),
    924: ("cross-object-link", ["target", "data1_raw", "data0_raw"],
          "540 stores data1 at+0x38c and data0 at+0x390; uses sound-root query35 then sound commands211/220/221/222 and controller metadata. Not a plain bool."),
    926: ("record-transfer", ["target", "record_VM_address"],
          "VM resolver, not formatter; sends534 data1=resolved record to manager0x17dd00. Record layout unresolved."),
    927: ("automatic-controller-A", ["target", "enable_raw"],
          "541->0x17ece0->0x174c20 stores requested raw+0x14c and applies to controller vector+0x84..88. Zero invokes reset0x184d70. Commonly blink; semantic name is not yet independently bound."),
    928: ("automatic-controller-B", ["target", "enable_raw"],
          "542->0x17ed50->0x174cb0 stores requested raw+0x150 and applies vector+0x90..94. Zero invokes reset0x184cd0. Commonly lip; name binding remains a hypothesis."),
    929: ("label-control", ["target", "label_format_VM"],
          "543->0x174d40 copies string into optional record+0xfc/+0xc; sets flags+0x24/+0x28=1,+0x2c/+0x30=0."),
    930: ("label-control", ["target"], "544->0x174d90. Exact phase/clear meaning remains partial."),
    931: ("player-raw-property", ["target", "value_raw"],
          "545->0x1743c0 stores raw player+0xa0 and sets+0x9c=1; later update consumption unresolved."),
    932: ("global-dirty", [], "Manager0x17edc0 iterates all players and calls0x174dc0 recomputing effective controller state. Not a constant."),
    933: ("global-save-state", [], "Manager0x17eea0 exports a variable-size blob then persists it with caller state. Binary serialization unresolved."),
    965: ("label-parameters", ["target", "label_format_VM", "word2_raw", "word3_raw"],
          "547 carries zeroed80-byte record with formatted name at0 and two raw DWORDs at72/76; manager guard required."),
    966: ("timeline-scalar-batch", ["target", "name_table_handle"],
          "531 per name in ordered table, then0x16da50. Values ignored by wrapper."),
    1025: ("refresh-enable", ["target", "enable_raw"],
           "Global guard then548 if nonzero or549 if zero. Uses0x175620 and manager0x17e150; full resource/refresh behavior remains partial."),
    1027: ("label-query", ["target", "label_format_VM"],
           "551 returns optional helper0x175880 result through initialized0 result slot; status1."),
    1028: ("label-query", ["target"],
           "553 returns0x1758c0 count-like result. Observed4-byte arguments, but wrapper still formats args+4 then discards it: original out-of-argument read; portable path must explicitly diagnose until resolved."),
    1029: ("label-enumeration", ["target", "selector0_raw", "selector1_raw"],
           "554 obtains descriptor; helper0x17fff0 returns count-like DWORD, status1. Descriptor layout unresolved."),
    1030: ("label-enumeration", ["target", "selector0_raw", "selector1_raw", "item_raw", "output0_VM", "output1_VM"],
           "554 obtains descriptor;0x17ff90 queries item fields0/1, copies name strings; result starts0, becomes1 after first successful name."),
    1047: ("label-control", ["target"], "552->0x1758a0, initialized temporary ignored, status2."),
    1048: ("label-control", ["target"], "550->0x175830 trims active range and clears a list; full meaning unresolved."),
}


def psz_samples(game):
    path = game/"psz.int"
    entries, kind, cipher = kif_entries(path, 0x0F6BB0AE)
    rows, decoded_count = [], 0
    for number, entry in enumerate(entries):
        with path.open("rb") as handle:
            handle.seek(entry["offset"])
            encrypted = handle.read(entry["size"] if number < 2 else min(64, entry["size"]))
        raw = blowfish_le_decrypt(cipher, encrypted) if cipher else encrypted
        row = {"name": entry["name"].decode("cp932", "replace"),
               "archive_offset": entry["offset"], "entry_bytes": entry["size"],
               "head16": raw[:16].hex()}
        if len(raw) >= 16 and raw[:4] == b"PSZ\0":
            compressed, expanded, reserved = struct.unpack_from("<III", raw, 4)
            row.update(compressed_bytes=compressed, expanded_bytes=expanded, reserved=reserved,
                       payload_length_matches=compressed+16 == entry["size"])
            if number < 2:
                assert expanded < 256*1024*1024
                z = zlib.decompressobj()
                psb = z.decompress(raw[16:], expanded+1)
                assert len(psb) == expanded and z.eof and not z.unused_data
                assert psb[:4] == b"PSB\0"
                version, flags = struct.unpack_from("<HH", psb, 4)
                row.update(psb_version=version, psb_flags=flags,
                           psb_head64=psb[:64].hex(), psb_sha256=sha(psb),
                           psb_header_status="encrypted" if flags else "plaintext",
                           ready_for_timeline_rendering=False)
                decoded_count += 1
        rows.append(row)
    return {"archive": str(path.relative_to(ROOT)), "archive_kind": kind,
            "entry_count": len(entries), "fully_decoded_envelopes": decoded_count,
            "entries": rows,
            "scope": "full PSZ decompression for first2 only; all135 headers inspected; no assets exported"}


def main():
    profiles = read_json(ROOT/"reports/recovery_A_20261116_feature_profiles.json")
    p = next(p for p in profiles["profiles"] if p["sha256"] == SHA)
    binary = ROOT/p["binary_paths"][0]
    image = Image(binary, SHA)
    rtti = rtti_vtable(image, "kcTaskM2Plane")[0]
    assert rtti["command_handler_RVA"] == 0x2384C0
    assert image.u32(0x1DE9A4+54*4)-image.base == 0x1DE791
    targets = {r["entry_rva"] for r in p["slots"] if r["entry_rva"] is not None}
    target_rows = []
    for row in p["slots"]:
        index = row["index"]
        if index == 925:
            continue
        record = recover(image, row["entry_rva"], targets)
        instruction_pushes = {int(i["operands"], 0) for i in record["disassembly"]
                              if i["mnemonic"] == "push"
                              and (i["operands"].startswith("0x") or i["operands"].isdigit())}
        assert set(COMMANDS[index]).issubset(instruction_pushes), (index, instruction_pushes)
        target_rows.append({"index": index, "entry_rva": row["entry_rva"],
                            "callsites": sum(row["ordinary_local_argument_byte_counts"].values()),
                            "argument_bytes_counts": row["ordinary_local_argument_byte_counts"],
                            "group": FACTS[index][0], "words": FACTS[index][1],
                            "contract": FACTS[index][2], "commands": COMMANDS[index],
                            "native_return_status": 1 if index in {909,1027,1028,1029,1030} else 2,
                            "implemented_and_integrated": False,
                            "wrapper_CFG": record})
    receivers = {}
    for command in range(525, 555):
        target = image.u32(0x239458+(command-525)*4)-image.base
        receivers[command] = {"pointer_slot_RVA": 0x239458+(command-525)*4,
                              "target_RVA": target, "snippet": block(image, target, 220)}
    receivers[524] = {"target_RVA": 0x2387AD, "snippet": block(image, 0x2387AD, 507)}
    helpers = {f"0x{r:08x}": block(image, r, size) for r, size in [
        (0x237CF0, 275), (0x1DE791, 60), (0x17ECE0, 108), (0x17ED50, 108),
        (0x174C20, 129), (0x174CB0, 129), (0x1744A0, 110), (0x1743C0, 24),
        (0x1745D0, 14), (0x174D40, 65), (0x17BFA0, 136), (0x2394D0, 359),
        (0x239640, 240), (0x16D980, 191), (0x16DD10, 223), (0x17EDC0, 106),
        (0x2395D2, 106), (0x237F90, 255)]}
    resource = psz_samples(binary.parent)
    ordered = sorted(target_rows, key=lambda r: (-r["callsites"], r["index"]))
    out = {"schema": "cs2-recovery-A-emote/v1", "generated_at_utc": datetime.now(timezone.utc).isoformat(),
           "binary_path": p["binary_paths"][0], "PE_sha256": SHA,
           "file_version": p["file_version"],
           "summary": {"nonconstant_slots": len(target_rows),
                       "ordinary_static_callsites": sum(r["callsites"] for r in target_rows),
                       "task_type": 54, "rendering_compatibility_claim": False,
                       "integrated_semantic_native_count": 0},
           "priority_contracts": ordered, "RTTI": rtti,
           "factory": {"type": 54, "table_RVA": 0x1DE9A4, "branch_RVA": 0x1DE791,
                       "allocation_bytes": 0x4B8, "constructor_RVA": 0x237CF0},
           "command_receiver": {"RVA": 0x2384C0, "table_RVA": 0x239458,
                                "commands": receivers},
           "helpers": helpers, "resource_evidence": resource,
           "limits": ["emocon.md covers ordinary1055-1061; not this player API.",
                      "Controller A/B semantic names and timeline scalar units are not guessed.",
                      "NEKO PSZ samples decode to encrypted-header PSB4; no pixel/NEKO compatibility claim.",
                      "All native/profile/backend/rendering integration remains explicit and separate."]}
    prefix = ROOT/"reports/recovery_A_20261116_emote"
    prefix.with_suffix(".json").write_text(json.dumps(out, ensure_ascii=False, indent=2)+"\n", encoding="utf-8")
    lines = ["# A: NEKOPARA E-mote / type54 contracts", "",
             "Original-byte static evidence; no original code execution or builds.",
             f"PE SHA256 `{SHA}`; version `{p['file_version']}`.", "",
             "## Confirmed architecture", "",
             "- Factory slot54 ->0x1de791, allocate0x4b8 ->constructor0x237cf0 ->RTTI kcTaskM2Plane.",
             "- Object+0x380 holds manager; commands524..554 ->receiver0x2384c0, jump table0x239458 for525..554.",
             "- Drawing callback0x2394d0 has two D3D draw paths: source texture available uses virtual slot0x14c,primitive5,count2,stride0x1c; missing texture uses stride0x14,object+0x328. The source geometry/texture path and its evaluation still need recovery.",
             "- emocon.md is a separate1055..1061 graphics-label configuration. No LGPL source was copied.",
             "", "## Priority and ABI", "",
             "| Native | Static calls | Argument bytes | Group | Commands |",
             "|---:|---:|---|---|---|"]
    for r in ordered:
        lines.append(f"| {r['index']} | {r['callsites']} | {r['argument_bytes_counts']} | {r['group']} | {r['commands']} |")
    lines += [""]
    for r in ordered:
        lines += [f"### Ordinary {r['index']}: `{r['words']}`", "",
                  f"RVA `0x{r['entry_rva']:08x}`; native status{r['native_return_status']}.",
                  r["contract"], ""]
    lines += ["## Resources and rendering limits", "",
              f"`psz.int`: {resource['entry_count']} entries; first2 fully decompressed in memory, all headers read.",
              "PSZ: magic PSZ+NUL; LE compressed length@4, expanded length@8, reserved@12, zlib@16. Both samples decode to PSB v4 with flags1 (encrypted header).",
              "These decoded envelopes are not playable timelines. Backend must decrypt/validate PSB, evaluate timeline and produce a real frame before draw can succeed.",
              "No static portrait, blank/zero pixel buffer or silent no-op may count as E-mote rendering.",
              "", "## Module scope", "",
              "runtime/emote.rs supplies bounded PSZ decode, PSB header identification, persistent type54 identities and an explicit backend protocol. desktop/emote_native.rs supplies bounded ABI planning and genuine CPU RGBA submission. Backend absence/encrypted payload/unknown commands remain errors.",
              "The parent must attach state to DataServices, resolve tasks with its real tree, expose modules and place the supported-native guard before the profile guard. No host constructors or lib.rs were edited here.",
              "runtime/Cargo.toml adds the existing workspace flate2 dependency for actual in-process PSZ decoding. No original DLL is loaded or executed.",
              "Source includes synthetic decoder/backend-rejection and CPU clipping/pixel tests, but cargo test and compilation were not run.",
              "", "## Organization", "",
              "Analysis script classified in scripts/analysis; evidence in reports/recovery_A_20261116_emote.*. Assets are read in memory and not exported. No temporary products or backups created."]
    prefix.with_suffix(".md").write_text("\n".join(lines)+"\n", encoding="utf-8")
    print(json.dumps(out["summary"], indent=2))


if __name__ == "__main__":
    main()

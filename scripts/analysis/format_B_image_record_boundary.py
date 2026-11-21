"""Bounded703 buffer ancestry and one evidenced resource fallback; no target execution."""
from pathlib import Path
import argparse
import hashlib
import json
import struct
import subprocess
import sys

sys.dont_write_bytecode = True
from recovery_A_20261116 import Image, ROOT, instruction_row

SOURCE = ROOT / "archive/2026-11/20261117_format_B_layer_profiles.json"
OUTPUT = ROOT / "archive/2026-11/20261117_format_B_image_record_boundary.json"
WIDE_HELPERS = {0x5CA010, 0x674F40, 0x63B460, 0x5C95E0}


def resource_names(profile):
    """Read identified image name tables only, without their image bodies."""
    from startup_inventory import entries_at, normalized
    audit_path = ROOT / "reports/all_sample_content_audit.json"
    audit = json.loads(audit_path.read_text(encoding="utf8"))
    executable = (ROOT / profile["path"]).resolve()
    game = max((g for g in audit["games"]
                if executable.is_relative_to(Path(g["root"]).resolve())),
               key=lambda g: len(g["root"]))
    aliases = game["archives"]["selected_name_seeds"]
    archives = [k for k in aliases if normalized(k).split("/")[-1] == "image.int"
                or normalized(k).split("/")[-1].startswith("image_")]
    if not archives:
        raise ValueError("No previously identified image archive")
    results = []
    for archive in archives:
        path = Path(game["root"]) / archive
        entries, layout, _ = entries_at(path, int(aliases[archive].split()[0], 0))
        examples = [e for e in entries if e["name"].lower().endswith(b".anm")][:4]
        examples += [e for e in entries if b"_" in e["name"] and
                     e["name"].lower().endswith((b".hg3", b".hg2"))][:8]
        results.append({"path": path.relative_to(ROOT).as_posix(), "layout": layout,
            "name_seed": aliases[archive].split()[0], "entry_count": len(entries),
            "decoded_names_sha256": hashlib.sha256(b"\0".join(e["name"] for e in entries)).hexdigest(),
            "encoding": "archive entry raw bytes; CP932 labels are display only",
            "examples": [{"raw_hex": e["name"].hex(),
                          "cp932": e["name"].decode("cp932", "backslashreplace"),
                          "offset": e["offset"], "size": e["size"]} for e in examples]})
    return results


def routine(image, va, length):
    raw = image.get(va - image.base, length)
    return {"va": hex(va), "length": length, "bytes_sha256": hashlib.sha256(raw).hexdigest(),
            "instructions": [instruction_row(i, image.base) for i in image.md.disasm(raw, va)]}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--dump", nargs="+")
    parser.add_argument("--resource-wrapper", type=int)
    parser.add_argument("--syntax", action="store_true")
    args = parser.parse_args()
    if args.syntax:
        formatter = Path.home() / ".rustup/toolchains/stable-x86_64-pc-windows-gnu/bin/rustfmt.exe"
        rows = []
        for name in ("resource_direct_native.rs", "image_record_fill.rs"):
            path = ROOT / "simulator/crates/desktop/src" / name
            raw = path.read_bytes()
            parsed = subprocess.run([str(formatter), "--edition", "2024", "--config",
                "skip_children=true", "--emit", "stdout"], input=raw, capture_output=True)
            rows.append({"path": path.relative_to(ROOT).as_posix(),
                "sha256": hashlib.sha256(raw).hexdigest(),
                "grammar_read": parsed.returncode == 0, "unchanged": raw == path.read_bytes(),
                "diagnostic": parsed.stderr.decode("utf8", errors="replace")})
        output = OUTPUT.with_name("20261117_format_B_image_record_boundary_syntax.json")
        output.write_text(json.dumps({"compiled": False, "type_checked": False,
            "tests_run": False, "original_code_executed": False, "files": rows}, indent=2) + "\n", encoding="utf8")
        print(json.dumps({"output": output.relative_to(ROOT).as_posix(),
            "grammar_read": all(r["grammar_read"] for r in rows),
            "source_unchanged": all(r["unchanged"] for r in rows)}))
        return
    old = json.loads(SOURCE.read_text(encoding="utf8"))
    profiles = [r for r in old["profiles"] if int(r["helper_va"], 0) in WIDE_HELPERS]
    yuki = next(r for r in profiles if int(r["helper_va"], 0) == 0x5CA010)
    if args.dump or args.resource_wrapper is not None:
        image = Image(ROOT / yuki["path"], yuki["sha256"])
        spans = list(args.dump or [])
        if args.resource_wrapper is not None:
            table = 0xA270E8
            va = image.u32(table - image.base + 4 * args.resource_wrapper)
            print("TABLE", hex(table), args.resource_wrapper, hex(va))
            spans.append(f"{va:#x}:0x180")
        for text in spans:
            va, length = (int(x, 0) for x in text.split(":"))
            for i in image.md.disasm(image.get(va - image.base, length), va):
                print(f"{i.address:x}: {i.mnemonic} {i.op_str}")
        return
    rows = []
    for profile in profiles:
        image = Image(ROOT / profile["path"], profile["sha256"])
        wrapper = int(profile["entry_va"], 0)
        helper = int(profile["helper_va"], 0)
        selector = int(profile["selector_va"], 0)
        # Recover the two concrete conversion calls immediately before the
        # known selector; no registry/table rediscovery or other PE is read.
        before = [i for i in profile["helper_instructions"]
                  if i["rva"] + image.base < helper + 0xD8]
        calls = [int(i["operands"], 16) for i in before
                 if i["mnemonic"] == "call" and i["operands"].startswith("0x")]
        conversions = []
        for va in calls:
            if va == selector:
                continue
            conversions.append(routine(image, va, 0x50))
        rows.append({"path": profile["path"], "sha256": profile["sha256"],
            "wrapper": routine(image, wrapper, 0xC4),
            "helper": routine(image, helper, len(bytes.fromhex("".join(i["bytes"] for i in profile["helper_instructions"])))),
            "selector": routine(image, selector, 0xC9),
            "upstream_conversions": conversions,
            "wide_string_imports": profile["imports"], "literal_bytes": profile["references"]})
        rows[-1]["actual_image_names"] = resource_names(profile)
        if helper == 0x5CA010:
            rows[-1]["conversion_functions"] = {
                "cp932_to_wide": routine(image, 0x5FFC30, 0x1C),
                "wide_to_cp932": routine(image, 0x5FFC10, 0x20),
                "copy_without_extension": routine(image, 0x5FFB60, 0x70),
                "tokenizer": routine(image, 0x5FF580, 0xD0),
                "vm_string_pointer": routine(image, 0x892F20, 0x84),
                "vm_format_entry": routine(image, 0x892B60, 0x21),
                "vm_format_byte_writes": routine(image, 0x892200, 0x310),
            }
            rows[-1]["conversion_imports"] = {hex(image.base + rva): item
                for rva, item in image.imports.items() if image.base + rva in (0x8E2330, 0x8E2334)}
            rows[-1]["prefix_range"] = {"base_relative_to_ebp": "0x404", "capacity": 256,
                "written_by_selector": "bytes through first single NUL only",
                "prior_zero_fill": False, "consumer_format_hex": image.get(0x9DC598 - image.base, 8).hex()}
            # A fallback selected from the existing static call inventory,
            # bounded to this exact PE and these known receiver addresses.
            rows[-1]["resource736"] = {
                "table_va": "0xa270e8", "slot_va": hex(image.u32(0xA270E8 - image.base + 736 * 4)),
                "wrapper": routine(image, 0x5A93F0, 0x5A),
                "current_wrapper": routine(image, 0x4E3E40, 4),
                "current_pixels": routine(image, 0x4D0DE0, 3),
                "fill_record": routine(image, 0x5E42B0, 0x6F),
                "clip_dispatch": routine(image, 0x5DE170, 0xEA),
                "format_table_hex": image.get(0xA2872C - image.base, 20).hex(),
                "format_table": [hex(x) for x in struct.unpack("<5I", image.get(0xA2872C - image.base, 20))],
                "bgra32": routine(image, 0x5E0D20, 0xBB),
                "bgr24": routine(image, 0x5E0DE0, 0x195),
                "canvas_gate": routine(image, 0x5C9750, 0x42),
                "canvas_tile_clear": routine(image, 0x5D5D80, 0x1E8),
            }
    inventory_path = ROOT / "reports/kcs_call_inventory_20261116.json"
    inventory = json.loads(inventory_path.read_text(encoding="utf8"))
    calls = [{"root": g["root"], "resource": p["resource"], "sha256": p["sha256"], **c}
             for g in inventory["games"] for p in g["programs"] if p["table_hint"] == "ordinary"
             for c in p["calls"] if c["index"] in (703, 710, 736)]
    OUTPUT.write_text(json.dumps({"original_code_executed": False, "vm_executed": False,
        "source_profiles_sha256": hashlib.sha256(SOURCE.read_bytes()).hexdigest(),
        "inventory_sha256": hashlib.sha256(inventory_path.read_bytes()).hexdigest(),
        "profiles": rows, "actual_static_calls": calls}, ensure_ascii=True, indent=2) + "\n", encoding="utf8")
    print(json.dumps({"output": OUTPUT.relative_to(ROOT).as_posix(), "profiles": len(rows),
        "calls": {str(n): sum(c["index"] == n for c in calls) for n in (703, 710, 736)}}))


if __name__ == "__main__":
    main()

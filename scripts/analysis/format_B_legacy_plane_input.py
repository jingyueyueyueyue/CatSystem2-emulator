"""Bounded Utate702 byte/receiver evidence. Never execute PE, KCS or Rust."""
from pathlib import Path
import hashlib
import json
import re
import struct
import subprocess
import sys
import zlib

sys.dont_write_bytecode = True
from recovery_A_20261116 import Image, ROOT, instruction_row
from audit_runtime_evidence import operand_count, blowfish_le_decrypt
from startup_inventory import entries_at, normalized

SHA = "a9f8b08713aac278671853b2293a13b2623a122ede9341cb921901b09601cb52"
NAV = ROOT / "archive/2026-11/20261117_movie_plane_legacy701702_navigation.json"
OUT = ROOT / "archive/2026-11/20261117_format_B_legacy_plane_input.json"


def references(image, value):
    needle = struct.pack("<I", value)
    return [image.base + s["rva"] + m.start() for s in image.sections
            for m in re.finditer(re.escape(needle), s["data"])]


def routine(image, address, length):
    raw = image.get(address - image.base, length)
    return {"va": hex(address), "length": length,
            "bytes_sha256": hashlib.sha256(raw).hexdigest(),
            "instructions": [instruction_row(i, image.base)
                             for i in image.md.disasm(raw, address)]}


def word(image, address):
    return struct.unpack("<I", image.get(address - image.base, 4))[0]


def switch_case(image, code, byte_map, jump_table):
    index = image.get(byte_map + code - 3 - image.base, 1)[0]
    return {"code": code, "byte_map_va": hex(byte_map), "index": index,
            "jump_table_va": hex(jump_table),
            "case_va": hex(word(image, jump_table + 4 * index))}


def main():
    if "--syntax" in sys.argv:
        formatter = Path.home() / ".rustup/toolchains/stable-x86_64-pc-windows-gnu/bin/rustfmt.exe"
        rows = []
        for name in ("legacy_plane_input.rs", "message_platform.rs"):
            path = ROOT / "simulator/crates/desktop/src" / name
            raw = path.read_bytes()
            parsed = subprocess.run([str(formatter), "--edition", "2024", "--config",
                "skip_children=true", "--emit", "stdout"], input=raw, capture_output=True)
            rows.append({"path": path.relative_to(ROOT).as_posix(),
                         "sha256": hashlib.sha256(raw).hexdigest(),
                         "grammar_read": parsed.returncode == 0,
                         "unchanged": raw == path.read_bytes(),
                         "diagnostic": parsed.stderr.decode("utf8", errors="replace")})
        output = OUT.with_name("20261117_format_B_legacy_plane_input_syntax.json")
        output.write_text(json.dumps({"compiled": False, "type_checked": False,
            "tests_run": False, "original_code_executed": False, "files": rows}, indent=2) + "\n",
            encoding="utf8")
        print(json.dumps({"output": output.relative_to(ROOT).as_posix(),
                          "grammar_read": all(row["grammar_read"] for row in rows),
                          "source_unchanged": all(row["unchanged"] for row in rows)}))
        return
    nav = json.loads(NAV.read_text(encoding="utf8"))
    image = Image(ROOT / nav["path"], SHA)
    if "--rtti" in sys.argv:
        rows = []
        for s in image.sections:
            for m in re.finditer(rb"\.\?AV[^\x00]*(?:Plane|2D|Input|Button)[^\x00]*\x00", s["data"]):
                address = image.base + s["rva"] + m.start()
                rows.append({"name": m.group()[:-1].decode("ascii"), "va": hex(address),
                             "descriptor_refs": [hex(x) for x in references(image, address - 8)]})
        print(json.dumps(rows))
        return
    if "--dump" in sys.argv:
        start = sys.argv.index("--dump") + 1
        for text in sys.argv[start:]:
            address, length = (int(x, 0) for x in text.split(":"))
            for i in image.md.disasm(image.get(address - image.base, length), address):
                print(f"{i.address:x}: {i.mnemonic} {i.op_str}")
        return
    if "--refs" in sys.argv:
        for text in sys.argv[sys.argv.index("--refs") + 1:]:
            value = int(text, 0)
            print(hex(value), [hex(x) for x in references(image, value)])
        return
    if "--words" in sys.argv:
        for text in sys.argv[sys.argv.index("--words") + 1:]:
            address, length = (int(x, 0) for x in text.split(":"))
            raw = image.get(address - image.base, length)
            for at in range(0, len(raw) - 3, 4):
                print(f"{address+at:x}: {struct.unpack_from('<I', raw, at)[0]:08x}")
        return
    if "--motion-reads" in sys.argv:
        instructions = image.insns(0x43B0A0 - image.base, 0x1030)
        indices = {n for n, i in enumerate(instructions)
                   if any(o.type == 3 and o.mem.disp == 0x34 for o in i.operands)}
        keep = {n for at in indices for n in range(max(0, at-5), min(len(instructions), at+7))}
        for n in sorted(keep):
            i = instructions[n]
            print(f"{i.address:x}: {i.mnemonic} {i.op_str}")
        return
    inventory_path = ROOT / "reports/kcs_call_inventory_20261116.json"
    inventory = json.loads(inventory_path.read_text(encoding="utf8"))
    root = (ROOT / nav["path"]).parent
    game = next(g for g in inventory["games"] if Path(g["root"]) == root)
    program = next(p for p in game["programs"] if any(c["index"] == 702 for c in p["calls"]))
    resource = program["resource"]
    if "::" in resource:
        archive, name = resource.split("::", 1)
        audit = json.loads((ROOT / "reports/all_sample_content_audit.json").read_text(encoding="utf8"))
        facts = next(g for g in audit["games"] if Path(g["root"]) == root)
        seeds = {normalized(k): int(v.split()[0], 0)
                 for k, v in facts["archives"]["selected_name_seeds"].items()}
        path = root / archive
        entries, _, cipher = entries_at(path, seeds.get(normalized(archive), 0))
        entry = next(e for e in entries if e["name"].decode("cp932") == name)
        with path.open("rb") as stream:
            stream.seek(entry["offset"])
            data = stream.read(entry["size"])
        if cipher:
            data = blowfish_le_decrypt(cipher, data)
    else:
        data = (root / resource).read_bytes()
    if hashlib.sha256(data).hexdigest() != program["sha256"]:
        raise ValueError("KCS inventory bytes changed")
    header = struct.unpack_from("<12I", data, 4)
    body = zlib.decompress(data[52:])
    code_length = header[3]
    code = body[:code_length]
    rows = []
    at = 0
    while at < len(code):
        offset = at
        opcode = code[at]
        count = operand_count(opcode)
        if count is None:
            raise ValueError("unknown KCS instruction")
        at += 1
        operands = []
        for _ in range(count):
            flags, payload = struct.unpack_from("<BI", code, at)
            operands.append({"flags": flags, "payload": payload, "hex": hex(payload)})
            at += 5
        if 1980 <= offset <= 2270:
            rows.append({"offset": offset, "opcode": hex(opcode), "operands": operands})
    button91 = switch_case(image, 91, 0x42E6E0, 0x42E68C)
    input143 = switch_case(image, 143, 0x43B004, 0x43AF84)
    if button91["case_va"] != "0x42e49c" or input143["case_va"] != "0x43ae98":
        raise ValueError("fixed receiver switch bytes differ")
    if word(image, 0x69DE80 + 702 * 4) != 0x45DB20:
        raise ValueError("fixed ordinary702 table entry differs")
    objects = []
    for name, descriptor, locator, vtable, receiver in [
        ("kcTaskButton", 0x6C8A10, 0x5B4200, 0x5AAE78, 0x42E0E0),
        ("kcTaskInput", 0x6C8E50, 0x5B4794, 0x5AB450, 0x43A520),
    ]:
        rtti = image.get(descriptor + 8 - image.base, len(name) + 7).split(b"\0", 1)[0]
        if rtti != (".?AV" + name + "@@").encode("ascii"):
            raise ValueError("fixed RTTI name differs")
        if (word(image, vtable - 4), word(image, locator + 12), word(image, vtable + 0x1C)) != (
            locator, descriptor, receiver
        ):
            raise ValueError("fixed RTTI/receiver binding differs")
        objects.append({"name": name, "descriptor_va": hex(descriptor),
                        "locator_va": hex(locator), "vtable_va": hex(vtable),
                        "receiver_va": hex(receiver), "rtti_ascii": rtti.decode("ascii")})
    result = {"pe_path": nav["path"], "pe_sha256": SHA,
              "original_code_executed": False, "vm_executed": False,
              "inventory_sha256": hashlib.sha256(inventory_path.read_bytes()).hexdigest(),
              "program": {"resource": program["resource"], "sha256": program["sha256"],
                          "calls": [c for c in program["calls"] if 1980 <= c["offset"] <= 2270]},
              "header": list(header), "call_neighborhood": rows,
              "data_prefix": body[code_length:code_length+512].hex(),
              "ordinary_table": {"va": "0x69de80", "slot": 702, "entry_va": "0x45db20"},
              "receiver_rtti": objects, "switch_cases": [button91, input143],
              "resolved_call": {"offset": 2225, "argument_bytes": 8,
                                "target_kind": 6, "target_data_offset": "0x70", "arg1": 0},
              "effect": {"button_command": 91, "input_command": 143,
                         "legacy_region_field": "0x34", "canonical_region_word": 14,
                         "meaning": "raw DWORD movement notification enable",
                         "region_removal": False, "binding_removal": False,
                         "mask_removal": False, "selection_reset": False},
              "routines": {name: routine(image, address, length) for name, address, length in [
                  ("wrapper66", 0x449900, 0x80),
                  ("wrapper702", 0x45DB20, 0x2F),
                  ("button_constructor", 0x42DF90, 0x51),
                  ("button_switch", 0x42E0E0, 0x27),
                  ("button91", 0x42E49C, 0x39),
                  ("root_input_query", 0x42E740, 0x1F),
                  ("input_switch", 0x43A520, 0x31),
                  ("input143", 0x43AE98, 0x6D),
                  ("region_copy", 0x43B0A0, 0x53),
                  ("motion_consumer", 0x43B690, 0x1D8),
              ]}}
    OUT.write_text(json.dumps(result, ensure_ascii=True, indent=2) + "\n", encoding="utf8")
    print(json.dumps({"output": OUT.relative_to(ROOT).as_posix(), "resource": resource,
                      "caller": result["resolved_call"], "switch_cases": result["switch_cases"],
                      "routines": len(result["routines"])}))


if __name__ == "__main__":
    main()

"""Static Yukikoi task-directory producer/consumer evidence, never target execution.

Only reads a fixed PE and existing inventory. Output belongs to archive/2026-11.
"""
from __future__ import annotations

import argparse
from collections import deque
import hashlib
import json
from pathlib import Path
import struct
import xml.etree.ElementTree as ET

import capstone
import pefile

ROOT = Path(__file__).resolve().parents[2]
EXPECTED = "63de0e14cd8ce11ad2d68e7ed6076641ff5e5b73d822603cc52764009d812976"


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--range", action="append", default=[], help="VA:length, hex accepted")
    parser.add_argument("--xref", action="append", default=[], type=lambda value: int(value, 0))
    parser.add_argument("--field", action="store_true", help="Find literal task+0x104 uses for navigation")
    parser.add_argument("--displacement", action="append", default=[], type=lambda value: int(value, 0))
    parser.add_argument("--push", action="append", default=[], type=lambda value: int(value, 0))
    parser.add_argument("--producer", action="store_true", help="Read the recovered complete directory chain")
    args = parser.parse_args()
    raw = (ROOT / "Yukikoi Melt/YukikoiMelt.exe").read_bytes()
    if hashlib.sha256(raw).hexdigest() != EXPECTED:
        raise ValueError("Unexpected Yukikoi PE identity")
    pe = pefile.PE(data=raw)
    base = pe.OPTIONAL_HEADER.ImageBase
    decoder = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_32)
    imports = {symbol.address: (symbol.name or b"ordinal").decode("ascii")
               for library in pe.DIRECTORY_ENTRY_IMPORT for symbol in library.imports}
    if args.producer:
        args.range.extend([
            "0x66e97e:0x1c", "0x6787fa:0x41", "0x6797e1:0x3e",
            "0x66c700:0x217", "0x492d58:0x19", "0x534080:0x6d",
            "0x536540:0x1c0", "0x535580:0x88", "0x53596c:0x42",
            "0x53ffd0:0x23e", "0x59d190:0xe0", "0x59d2c0:0xb5",
            "0x59c750:0xb5", "0x59dbe0:0xb5", "0x5ffc30:0x1c",
            "0x5ffc90:0x16", "0x6ead50:0xe5", "0x6eafc0:0xad",
            "0x6eb3c0:0xaf", "0x6010b0:0x1e0",
        ])

    def row(item):
        indirect = item.op_str.removeprefix("dword ptr [").removesuffix("]")
        slot = (int(indirect, 16) if indirect.startswith("0x")
                and all(char in "0123456789abcdefABCDEF" for char in indirect[2:]) else None)
        return {"va": hex(item.address), "bytes": item.bytes.hex(),
                "op": item.mnemonic, "args": item.op_str,
                "import": imports.get(slot)}

    slices = {}
    for spec in args.range:
        start, length = (int(value, 0) for value in spec.split(":"))
        if not 0 < length <= 0x10000:
            raise ValueError("Range must be bounded")
        slices[hex(start)] = [row(item) for item in decoder.disasm(pe.get_data(start - base, length), start)]
    navigation = []
    if args.field or args.xref or args.displacement or args.push:
        for section in pe.sections:
            if not section.Characteristics & 0x20000000:
                continue
            previous = deque(maxlen=10)
            # skipdata is navigation only; slices above always retain raw bytes.
            decoder.skipdata = True
            for item in decoder.disasm(section.get_data(), base + section.VirtualAddress):
                offsets = ([0x104] if args.field else []) + args.displacement
                field = any(f" + {hex(offset)}]" in item.op_str for offset in offsets)
                target = item.mnemonic in ("call", "jmp") and item.op_str.startswith("0x")
                pushed = item.mnemonic == "push" and item.op_str in {hex(value) for value in args.push}
                if field or pushed or (target and int(item.op_str, 16) in args.xref):
                    navigation.append({"at": hex(item.address), "context": [*previous, row(item)]})
                previous.append(row(item))
    result = {"pe_sha256": EXPECTED, "original_code_executed": False,
              "scope": "bounded static instructions; field matches are navigation, not task identity proof",
              "slices": slices, "navigation": navigation}
    if args.producer:
        # Existing selected startup, read once; comments are not another save
        # setting. Do not run the player or rescan game resources/callers.
        source = ROOT / "Yukikoi Melt/config/startup.xml"
        xml_bytes = source.read_bytes()
        xml = ET.fromstring(xml_bytes)
        save = xml.find("FOLDER/save")
        jumps = struct.unpack("<11I", pe.get_data(0x53663C - base, 44))
        indices = pe.get_data(0x536668 - base, 40)
        ordinary = {str(slot): hex(struct.unpack("<I", pe.get_data(
            0xA270E8 - base + slot * 4, 4))[0]) for slot in (245, 247, 990, 1031)}
        cached = json.loads((ROOT / "archive/2026-11/20261118_save_storage_contracts.json")
                            .read_text(encoding="utf-8"))["ordinary_inventory"]
        result["producer_contract"] = {
            "selected_startup": {"path": source.relative_to(ROOT).as_posix(),
                "sha256": hashlib.sha256(xml_bytes).hexdigest(),
                "folder_save": None if save is None else (save.text or ""),
                "original_config_read_only": True},
            "configuration_alias": "66e984: config = application + 0x418; +0xb70 = application+0xf88",
            "startup_set": "492d58..492d6c: manager target0 command37 data0=0 data1=config+0xb70",
            "manager_directory": "+0x84, initial literal dot; command37 replaces; command38 copies out",
            "manager_directory_command_targets": {str(i + 1): hex(jumps[index])
                for i, index in enumerate(indices) if jumps[index] in (0x5365D5, 0x5365EB)},
            "script_task": "kind12 -> 53596c -> 53ffd0 arg8 -> lstrcpyW(task+0x104)",
            "ordinary_wrappers": ordinary,
            "filename_encoding": "5ffc90/5ffc30: MultiByteToWideChar(CP932=0x3a4, flags=1)",
            "path": "wide directory + literal backslash + formatted/converted filename; no INT lookup",
            "host_policy": "explicit private-root relocation; no game-root write or per-task numeric subdirectory",
            "cached_1031_scope": {"games": cached["game_count"], "sites": cached["site_count"],
                "Yukikoi_sites": sum("Yukikoi Melt" in item["game"] for item in cached["sites"]),
                "source": "archive/2026-11/20261118_save_storage_contracts.json",
                "not_cross_version_semantic_proof": True},
        }
    label = "field" if args.field else "_".join(hex(int(spec.split(':')[0], 0))[2:] for spec in args.range)
    if args.producer:
        label = "producer"
    if args.xref:
        label += "_xref_" + "_".join(f"{value:x}" for value in args.xref)
    if args.displacement:
        label += "_disp_" + "_".join(f"{value:x}" for value in args.displacement)
    if args.push:
        label += "_push_" + "_".join(f"{value:x}" for value in args.push)
    path = ROOT / "archive/2026-11" / f"20261118_F_save_task_directory_{label}.json"
    path.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"output": str(path.relative_to(ROOT)), "slices": len(slices),
                      "navigation_matches": len(navigation)}))


if __name__ == "__main__":
    main()

"""Read fixed CSS-write/disk-query code and existing ordinary call inventory.

Never executes original code, the player, disk queries or save operations.
"""
from __future__ import annotations

from collections import Counter
import hashlib
import json
from pathlib import Path

import capstone
import pefile

ROOT = Path(__file__).resolve().parents[2]
EXPECTED = "63de0e14cd8ce11ad2d68e7ed6076641ff5e5b73d822603cc52764009d812976"
RANGES = {
    "ordinary1031": (0x59DBE0, 0x80),
    "save_compress_query_write": (0x6EAD50, 0x265),
    "logical_drives_constructor": (0x6FD300, 0x0F),
    "space_query": (0x6FD3F0, 0xDD),
}


def main() -> None:
    raw = (ROOT / "Yukikoi Melt/YukikoiMelt.exe").read_bytes()
    if hashlib.sha256(raw).hexdigest() != EXPECTED:
        raise SystemExit("Unexpected PE identity")
    pe = pefile.PE(data=raw)
    imports = {
        symbol.address: symbol.name.decode("ascii", "replace") if symbol.name else str(symbol.ordinal)
        for library in pe.DIRECTORY_ENTRY_IMPORT for symbol in library.imports
    }
    decoder = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_32)
    routines = {}
    for name, (address, length) in RANGES.items():
        routines[name] = [{
            "va": item.address, "bytes": item.bytes.hex(),
            "mnemonic": item.mnemonic, "operands": item.op_str,
            "import": next((name for slot, name in imports.items()
                            if hex(slot) in item.op_str), None),
        } for item in decoder.disasm(pe.get_data(address - pe.OPTIONAL_HEADER.ImageBase, length), address)]

    inventory_path = ROOT / "reports/kcs_call_inventory_20261116.json"
    inventory_bytes = inventory_path.read_bytes()
    inventory = json.loads(inventory_bytes)
    sites = []
    for game in inventory["games"]:
        for program in game["programs"]:
            if program.get("role") != "runtime-candidate" or program.get("table_hint") != "ordinary":
                continue
            for call in program.get("calls", []):
                if call.get("index") == 1031:
                    sites.append({"game": game["root"], "resource": program["resource"],
                                  "program_sha256": program["sha256"], **call})
    sources = {}
    for name in ["simulator/crates/desktop/src/save_native.rs",
                 "simulator/crates/desktop/src/save_storage_space.rs",
                 "simulator/crates/desktop/Cargo.toml", "simulator/Cargo.lock"]:
        sources[name] = hashlib.sha256((ROOT / name).read_bytes()).hexdigest()
    report = {
        "schema": "cs2-save-storage-contract-v1", "pe_sha256": EXPECTED,
        "original_code_executed": False, "host_space_query_executed": False,
        "save_operation_executed": False, "compiled": False, "tests_run": False,
        "routines": routines, "source_sha256": sources,
        "ordinary_inventory": {"source": str(inventory_path.relative_to(ROOT)),
            "sha256": hashlib.sha256(inventory_bytes).hexdigest(),
            "table_scope": "runtime-candidate + ordinary path classification; not execution proof",
            "game_count": len({item["game"] for item in sites}), "site_count": len(sites),
            "argument_bytes": dict(Counter(str(item["argument_bytes"]) for item in sites)),
            "sites": sites},
        "contract": {
            "required_bytes": "complete serialized compressed CSS including its 16-byte header",
            "query_before_existing_file_open_or_truncate": True,
            "compare": "signed high DWORD positive, or high zero and unsigned low >= required",
            "query_failure_or_insufficient_space": "ordinary1031 returns 0 without creating/truncating output",
            "host_volume": "independent private-save destination; nearest existing directory if root is absent",
            "windows": "GetVolumePathNameW, GetDiskFreeSpaceW, GetDiskFreeSpaceExW available-to-caller",
            "unix": "libc statvfs f_bavail * fragment size, including Android",
            "remaining": ["original per-task +104 directory producer",
                          "cross-version complete equivalence",
                          "original ExW failure ignores its result and may consume uninitialized memory; host diagnoses it"],
        },
    }
    output = ROOT / "archive/2026-11/20261118_save_storage_contracts.json"
    output.write_text(json.dumps(report, ensure_ascii=True, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"output": str(output.relative_to(ROOT)),
                      "ordinary_games": report["ordinary_inventory"]["game_count"],
                      "ordinary_sites": len(sites),
                      "argument_bytes": report["ordinary_inventory"]["argument_bytes"]}))


if __name__ == "__main__":
    main()

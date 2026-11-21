"""Read-only, hash-pinned PE contracts for ordinary string/table batches."""
from __future__ import annotations

import hashlib
import json
import struct
from pathlib import Path

import capstone
import pefile


ROOT = Path(__file__).resolve().parents[2]
BINARY = ROOT / "Yukikoi Melt" / "YukikoiMelt.exe"
EXPECTED = "63de0e14cd8ce11ad2d68e7ed6076641ff5e5b73d822603cc52764009d812976"
SLOTS = (434, 435, 436, 437, 438, 633, 637, 638, 639, 640, 641, 675, 676, 677, 678, 679, 680, 681, 682, 688, 689, 690, 691, 692)
OUTPUT = ROOT / "archive" / "2026-11" / "20261117_format_B_table_batch_static.json"
HELPERS: dict[str, tuple[int, int]] = {
    "name_additional_field": (0x810170, 0x80),
    "name_byte_template": (0x5A7260, 0x160),
    "name_file_pattern": (0x5FFFC0, 0x80),
    "name_extension": (0x6004F0, 0x70),
    "name_wildcard_segment": (0x5FFCC0, 0xD0),
    "image_alpha_percent": (0x5E5310, 0x1C0),
    "map_remove_iterator": (0x5A7800, 0x90),
    "resource_request": (0x52B490, 0x1B0),
    "resource_cache_constructor": (0x52ADE0, 0x270),
    "resource_cache_clear": (0x52BAB0, 0xA0),
    "resource_cache_age": (0x52C250, 0xA0),
    "resource_cache_worker": (0x52CE50, 0xE0),
    "resource_cache_start": (0x52DF40, 0x70),
    "resource_cache_sort": (0x52AB90, 0x100),
    "resource_cache_query": (0x52C570, 0x20),
    "image_preload": (0x5C9720, 0x30),
    "image_cache_mode": (0x5CB120, 0x10),
}


def main() -> None:
    data = BINARY.read_bytes()
    digest = hashlib.sha256(data).hexdigest()
    if digest != EXPECTED:
        raise SystemExit(f"Unexpected PE SHA256: {digest}")
    image = pefile.PE(data=data, fast_load=True)
    base = image.OPTIONAL_HEADER.ImageBase
    decoder = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_32)
    pointers = struct.unpack("<1100I", image.get_data(0x6270E8, 1100 * 4))
    all_entries = sorted(set(pointers))

    def instructions(address: int, length: int) -> list[dict]:
        return [
            {"va": ins.address, "bytes": ins.bytes.hex(),
             "mnemonic": ins.mnemonic, "operands": ins.op_str}
            for ins in decoder.disasm(image.get_data(address - base, length), address)
        ]

    coverage = json.loads((ROOT / "reports/native_coverage_20261116.json").read_text(encoding="utf-8"))
    selected = [
        {key: row[key] for key in ("index", "status", "games_count", "call_sites", "argument_bytes")}
        for row in coverage["natives"] if row["index"] in SLOTS
    ]
    entries = {}
    for slot in SLOTS:
        address = pointers[slot]
        following = next((other for other in all_entries if other > address), address + 0x500)
        entries[str(slot)] = {
            "va": address, "rva": address - base,
            "instructions": instructions(address, min(following - address, 0x500)),
        }
    result = {
        "status": "read-only-static-instruction-evidence-not-runtime-validation",
        "binary": BINARY.relative_to(ROOT).as_posix(), "sha256": digest,
        "image_base": base, "ordinary_table_rva": 0x6270E8,
        "coverage_snapshot": coverage["generated_at_utc"],
        "coverage_warning": "Routing classifications are historical; inspect current source before wiring.",
        "coverage": selected, "entries": entries,
        "source_snapshots": {
            path: hashlib.sha256((ROOT / path).read_bytes()).hexdigest()
            for path in (
                "simulator/crates/desktop/src/data_services.rs",
                "simulator/crates/desktop/src/name_table_format.rs",
                "simulator/crates/desktop/src/image_native.rs",
                "simulator/crates/desktop/src/ordinary_native.rs",
                "simulator/crates/desktop/src/resource_preload.rs",
                "simulator/crates/desktop/src/lib.rs",
            )
        },
        "data_ranges": {
            "name_template_jump_table": {
                "va": 0x5A73C0,
                "bytes": image.get_data(0x5A73C0 - base, 0x29).hex(),
            },
            "wildcard_all": {
                "va": 0x8E9AC8,
                "bytes": image.get_data(0x8E9AC8 - base, 2).hex(),
            },
        },
        "helpers": {
            name: {"va": address, "instructions": instructions(address, length)}
            for name, (address, length) in HELPERS.items()
        },
    }
    other_path = ROOT / "cs2_full_v401" / "system" / "cs2_open.exe"
    other_data = other_path.read_bytes()
    other_digest = hashlib.sha256(other_data).hexdigest()
    if other_digest != "0e1dde51e86e9152d8e1d27b82efb15916cb9f82d23dd30990212fafd96c8b9f":
        raise SystemExit(f"Unexpected V401 SHA256: {other_digest}")
    other = pefile.PE(data=other_data, fast_load=True)
    other_base = other.OPTIONAL_HEADER.ImageBase
    other_pointers = struct.unpack("<1100I", other.get_data(0x73C900, 1100 * 4))
    other_entries = sorted(set(other_pointers))
    corroboration = {}
    for slot in (*SLOTS, 369, 370, 371, 372, 373, 374, 376):
        address = other_pointers[slot]
        following = next((value for value in other_entries if value > address), address + 0x500)
        corroboration[str(slot)] = {
            "va": address,
            "instructions": [
                {"va": ins.address, "bytes": ins.bytes.hex(),
                 "mnemonic": ins.mnemonic, "operands": ins.op_str}
                for ins in decoder.disasm(
                    other.get_data(address - other_base, min(following - address, 0x500)), address,
                )
            ],
        }
    result["version_corroboration"] = {
        "path": other_path.relative_to(ROOT).as_posix(),
        "sha256": other_digest, "ordinary_table_rva": 0x73C900,
        "status": "full-bounded-wrapper-evidence-only-not-complete-helper-equivalence",
        "entries": corroboration,
    }
    OUTPUT.write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(OUTPUT.relative_to(ROOT).as_posix())


if __name__ == "__main__":
    main()

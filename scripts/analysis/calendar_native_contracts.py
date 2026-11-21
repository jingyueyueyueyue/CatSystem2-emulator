"""Record both pinned native786 helpers and actual double constants."""
from __future__ import annotations
import json
import struct
from recovery_A_20261116 import Image, ROOT, instruction_row

PROFILES = [
    ("cs2_full_v401/system/cs2_open.exe",
     "0e1dde51e86e9152d8e1d27b82efb15916cb9f82d23dd30990212fafd96c8b9f",
     0x51BB10, 0x577350, 0x99AFA8, 0x99AFB0),
    ("Yukikoi Melt/YukikoiMelt.exe",
     "63de0e14cd8ce11ad2d68e7ed6076641ff5e5b73d822603cc52764009d812976",
     0x591A10, 0x6008B0, 0x8F1910, 0x8F1918),
]

def main() -> None:
    rows = []
    for name, digest, wrapper, helper, year_constant, month_constant in PROFILES:
        image = Image(ROOT / name, digest)
        constants = [
            {"va": address, "bytes": image.get(address - image.base, 8).hex(),
             "double": struct.unpack("<d", image.get(address - image.base, 8))[0]}
            for address in [year_constant, month_constant]
        ]
        if [row["double"] for row in constants] != [365.25, -30.6]:
            raise ValueError("weekday constants changed")
        rows.append({
            "pe": name, "sha256": image.digest, "image_base": image.base,
            "wrapper": [instruction_row(item, image.base)
                        for item in image.insns(wrapper - image.base, 32)],
            "helper": [instruction_row(item, image.base)
                       for item in image.insns(helper - image.base, 224)],
            "constants": constants,
        })
    report = {
        "schema": "cs2-calendar-native786/v1", "profiles": rows,
        "contract": "raw DWORD year/month/day; signed month selection; wrapping x86 integer arithmetic; double CVTT conversion; two signed remainders",
        "source": "simulator/crates/runtime/src/calendar.rs",
        "status": "fixed-PE-source-recovery-not-executed-equivalence",
        "compiled": False, "tests_run": False, "game_or_DLL_execution": False,
    }
    output = ROOT / "reports/calendar_native_contracts_20261117.json"
    output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"report": output.relative_to(ROOT).as_posix(), "profiles": len(rows)}))

if __name__ == "__main__":
    main()

"""Pinned wrappers and helper bytes for the independent emocon service."""
from __future__ import annotations
import hashlib
import json
from native_batch_contracts import analyze
from recovery_A_20261116 import Image, ROOT, instruction_row

def main() -> None:
    wrappers = analyze(list(range(1055, 1062)), 180)
    image = Image(ROOT / "Yukikoi Melt/YukikoiMelt.exe",
                  "63de0e14cd8ce11ad2d68e7ed6076641ff5e5b73d822603cc52764009d812976")
    references = json.loads((ROOT / "resources/reference/manifest.json").read_text(encoding="utf-8"))
    reference = next(row for row in references["sources"] if row["name"] == "KatzeSystem2")
    doc = ROOT / "resources/reference/KatzeSystem2/docs/emocon.md"
    paths = [
        "simulator/crates/runtime/src/emocon.rs",
        "simulator/crates/desktop/src/emocon_native.rs",
        "simulator/crates/formats/src/config.rs",
    ]
    report = {
        "schema": "cs2-emocon-native-contracts/v1", "slots": list(range(1055, 1062)),
        "status": "fixed-PE-and-document-behavior-recovery-not-compiled-or-executed",
        "wrappers": wrappers["binaries"],
        "helper_pe": {"path": image.path.relative_to(ROOT).as_posix(), "sha256": image.digest},
        "helpers": [
            {"role": role, "va": address, "instructions": [
                instruction_row(item, image.base)
                for item in image.insns(address - image.base, length)
            ]} for role, address, length in [
                ("count", 0x5AE260, 0x1C), ("label", 0x5AE280, 0x40),
                ("name", 0x5AE2C0, 0x40), ("stop", 0x5AE300, 0x0C),
                ("placement", 0x5AE310, 0x1B3), ("pose", 0x5AE4D0, 0xE0),
                ("reload", 0x5AE5B0, 0xB0), ("xml_open", 0x5AE660, 0x110),
            ]
        ],
        "reference": {
            "commit": reference["commit"], "license": reference["license"],
            "documentation": doc.relative_to(ROOT).as_posix(),
            "document_sha256": hashlib.sha256(doc.read_bytes()).hexdigest(),
            "reference_implementation_code_copied": False,
            "is_runtime_dependency": False,
        },
        "source_sha256": {
            path: hashlib.sha256((ROOT / path).read_bytes()).hexdigest() for path in paths
        },
        "remaining_boundaries": [
            "per-PE equivalence beyond pinned wrappers is not an executed acceptance result",
            "complete resource-router per-slot precedence remains a common host integration issue",
            "full host object-graph save must consume emocon snapshot APIs",
        ],
        "compiled": False, "tests_run": False, "original_code_executed": False,
    }
    output = ROOT / "reports/emocon_native_20261117.json"
    output.write_text(json.dumps(report, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print(json.dumps({"report": output.relative_to(ROOT).as_posix(),
                      "slots": 7, "runtime_executed": False}))

if __name__ == "__main__":
    main()

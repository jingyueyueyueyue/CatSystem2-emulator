"""Static resource-consumer evidence and read-only Rust syntax inspection.

Never calls original code, a compiler, a test or a simulator. All outputs are
dated archive files; rustfmt output is captured in memory without writing Rust.
"""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import subprocess

from native_batch_contracts import analyze
from recovery_A_20261116 import Image, ROOT, classify_tables, instruction_row, registrations


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--syntax-only", action="store_true")
    parser.add_argument("--evidence-only", action="store_true")
    args = parser.parse_args()
    output = ROOT / "archive/2026-11/20261117_format_B_resource_consumers_followup_PE.json"
    if not args.syntax_only:
        write_evidence(output)
    if args.evidence_only:
        print(output.relative_to(ROOT).as_posix())
        return
    paths = ["runtime/src/resource.rs", "desktop/src/sound_loop_table.rs",
             "desktop/src/language_platform.rs", "desktop/src/sound_sidecar.rs",
             "desktop/src/movie_native.rs", "desktop/src/movie_plane_native.rs", "desktop/src/movie.rs",
             "desktop/src/speech_table.rs"]
    formatter = Path.home() / ".rustup/toolchains/stable-x86_64-pc-windows-gnu/bin/rustfmt.exe"
    checks = []
    for relative in paths:
        path = ROOT / "simulator/crates" / relative
        raw = path.read_bytes()
        run = subprocess.run([str(formatter), "--edition", "2024", "--config",
                              "skip_children=true", "--emit", "stdout"],
                             input=raw, capture_output=True, check=False)
        checks.append({"path": path.relative_to(ROOT).as_posix(),
                       "sha256": hashlib.sha256(raw).hexdigest(),
                       "parse": run.returncode == 0,
                       "unchanged_after_read": raw == path.read_bytes(),
                       "diagnostic": run.stderr.decode("utf8", errors="replace")})
    syntax = output.with_name("20261117_format_B_resource_consumers_followup_syntax.json")
    syntax.write_text(json.dumps({"compiler_executed": False, "original_code_executed": False,
                                  "type_checked": False, "files": checks}, indent=2) + "\n", encoding="utf8")
    print(json.dumps({"evidence": output.relative_to(ROOT).as_posix(), "syntax": checks}, indent=2))


def write_evidence(output: Path) -> None:
    evidence = analyze([400], 512)
    for row in evidence["binaries"]:
        image = Image(ROOT / row["path"], row["sha256"])
        row["helpers"] = [
            {"rva": rva, "instructions": [instruction_row(item, image.base)
                for item in image.md.disasm(image.get(rva, 2048), image.base + rva)]}
            for rva in sorted({rva for entry in row["entries"] for rva in entry["direct_calls"]})
        ]
    summary = json.loads((ROOT / "reports/recovery_A_20261116_summary.json").read_text(encoding="utf8"))
    evidence["loop_profiles"] = []
    for row in summary["perPE"]:
        if Path(row["binary_path"]).suffix.lower() != ".exe":
            continue
        image = Image(ROOT / row["binary_path"], row["sha256"])
        _, observed = registrations(image)
        for table in classify_tables(image, observed):
            if table["table_hint"] != "ordinary" or len(table["slots"]) <= 400:
                continue
            slot = table["slots"][400]
            rva = slot["entry_rva"] if isinstance(slot, dict) else slot
            instructions = []
            literals = []
            for item in image.md.disasm(image.get(rva, 2048), image.base + rva):
                instructions.append(instruction_row(item, image.base))
                value = image.direct(item)
                if item.mnemonic == "push" and value is not None:
                    raw = image.get(value, 12)
                    if raw.startswith(b"sound\0") or raw.startswith("sound\0".encode("utf-16le")):
                        literals.append({"instruction_rva": item.address - image.base,
                                         "value_rva": value, "hex": raw.hex()})
                if item.mnemonic.startswith("ret"):
                    break
            evidence["loop_profiles"].append({"path": row["binary_path"], "sha256": row["sha256"],
                "entry_rva": rva, "sound_literals": literals, "instructions": instructions})
    row = next(row for row in evidence["binaries"] if row["path"] == "Yukikoi Melt/YukikoiMelt.exe")
    image = Image(ROOT / row["path"], row["sha256"])
    evidence["sidecar_helpers"] = {"sha256": row["sha256"], "routines": {
        name: [instruction_row(item, image.base)
               for item in image.md.disasm(image.get(va - image.base, size), va)]
        for name, (va, size) in {
            "lip": (0x542F80, 0x23A), "signal": (0x5431C0, 0x174),
            "tag": (0x546160, 0x268), "basename": (0x6005C0, 0x60),
            "extension": (0x5FFE20, 0xD0),
            "xml_file_load": (0x67F630, 0x2A0),
            "xml_file_source": (0x682800, 0x200),
            "plain_file_open": (0x52E8A0, 0x100),
            "movie_load": (0x656AE0, 0x200),
        }.items()}, "category_and_extension_hex": image.get(0x8E8FA0 - image.base, 0x1C).hex(),
        "plain_file_import": image.imports.get(0x8E22A8 - image.base)}
    output.write_text(json.dumps(evidence, ensure_ascii=False, indent=2) + "\n", encoding="utf8")


if __name__ == "__main__":
    main()

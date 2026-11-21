"""Read fixed M2 hittest contracts and resources without executing runtime code."""
from __future__ import annotations

import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys

from audit_runtime_evidence import blowfish_le_decrypt, kif_entries
from recovery_A_20261116 import ROOT, Image
from root_clock_profiles import snippet


def main() -> None:
    prior = json.loads((ROOT / "reports/recovery_A_20261116_emote.json").read_text(encoding="utf-8"))
    image = Image(ROOT / prior["binary_path"], prior["PE_sha256"])
    spans = {
        "native_1025_boolean_command": (0x24D6D0, 0x4F),
        "owned_548": (0x2391ED, 0x42),
        "owned_549": (0x23922F, 0x2B),
        "profile_hittest_first_tokens": (0x18BFF0, 0x166),
        "hittest_write_and_driver_fc_one": (0x175620, 0x205),
        "baseline_write_and_initial_driver_capture": (0x183260, 0x1F1),
        "device_manager_refresh": (0x17E150, 0xB0),
        "wrapper_driver_resource_release": (0x175920, 0x290),
    }
    entries, _, cipher = kif_entries(image.path.parent / "m2config.int", 0x0F6BB0AE)
    rows = []
    program_directives = 0
    for entry in entries:
        with (image.path.parent / "m2config.int").open("rb") as handle:
            handle.seek(entry["offset"])
            raw = handle.read(entry["size"])
        raw = blowfish_le_decrypt(cipher, raw) if cipher else raw
        clean = re.sub(rb"/\*.*?\*/", b"", raw, flags=re.S)
        active = False
        names = []
        for line in clean.splitlines():
            words = line.split(b"//", 1)[0].split()
            if not words:
                continue
            if words[0].startswith((b"#", b"$")):
                active = words[0] == b"#hittest"
            elif active:
                names.append(words[0].decode("cp932"))
            program_directives += words[0] in (b"%time", b"%set")
        if names:
            rows.append({"resource": entry["name"].decode("cp932"), "names": names,
                         "decoded_sha256": hashlib.sha256(raw).hexdigest()})
    sources = ["simulator/crates/runtime/src/" + name for name in (
        "emote_m2banks.rs", "emote_m2force.rs", "emote_cpu.rs", "emote_dispatch.rs",
    )] + ["simulator/crates/desktop/src/emote_native.rs"]
    report = {
        "schema": "cs2-emote-hittest-contracts/v1", "pe_sha256": image.digest,
        "binary_path": prior["binary_path"],
        "evidence": {name: snippet(image, start, size) for name, (start, size) in spans.items()},
        "section_bytes": image.get(0x9E4344 - image.base, 8).hex(),
        "resources": rows, "config_entries_read": len(entries),
        "program_time_set_directives": program_directives,
        "source_sha256": {name: hashlib.sha256((ROOT / name).read_bytes()).hexdigest() for name in sources},
        "chain": ["1025 and548/549 share one semantic operation",
                  "profile+194 #hittest -> value table absolute write plus actual aggregator baseline",
                  "nonzero enable writes0.0; zero writes1.0",
                  "explicit value-table mode0 -> driverfc(1.0) -> reference clocks -> lazy CPU pixels",
                  "no owner/blink/lip/action/spring clock is invented by this command"],
        "limits": ["fixed PE CPU renderer path; no claim of complete original device-loss equivalence",
                   "965 procedural resource family remains separate and lower priority without resource hits",
                   "multi-player selection, M2 serialization and all-version equivalence remain open"],
        "compiled": False, "type_checked": False, "tests_run": False,
        "runtime_or_original_executed": False, "full_function_equivalence_added": 0,
    }
    (ROOT / "reports/emote_hittest_contracts_20261117.json").write_text(
        json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    if "--syntax" in sys.argv:
        results = []
        for name in sources:
            raw = (ROOT / name).read_bytes()
            run = subprocess.run([str(Path.home() / ".cargo/bin/rustfmt.exe"), "--edition", "2024",
                                  "--config", "skip_children=true", "--emit", "stdout"],
                                 input=raw, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, check=False)
            results.append({"path": name, "sha256": hashlib.sha256(raw).hexdigest(),
                            "syntax_exit": run.returncode, "diagnostic": run.stderr.decode("utf-8", errors="replace")})
        (ROOT / "archive/2026-11/20261117_emote_hittest_source_syntax.json").write_text(
            json.dumps({"grammar_only": True, "compiled": False, "type_checked": False,
                        "tests_run": False, "sources": results}, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"hittest_files": len(rows), "hittest_rows": sum(len(row["names"]) for row in rows),
                      "program_time_set_directives": program_directives,
                      "syntax_failures": sum(row["syntax_exit"] != 0 for row in results) if "--syntax" in sys.argv else None}))


if __name__ == "__main__":
    main()

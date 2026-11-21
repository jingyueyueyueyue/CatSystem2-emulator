"""Fixed-PE M2 spline/config/accumulator evidence; never execute a curve.

--syntax feeds source bytes to rustfmt stdin and discards formatted stdout.
No build, type check, test, VM, original executable or plugin execution.
"""
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

SOURCES = ["simulator/crates/runtime/src/" + name for name in (
    "emote.rs", "emote_m2config.rs", "emote_m2spline.rs", "emote_m2force.rs",
    "emote_m2live.rs", "emote_dispatch.rs", "emote_cpu.rs",
)] + ["simulator/crates/desktop/src/emote_native.rs"]


def main() -> None:
    prior = json.loads((ROOT / "reports/recovery_A_20261116_emote.json").read_text(encoding="utf-8"))
    image = Image(ROOT / prior["binary_path"],
                  "c71c5b85e6eca660474f156d96b0021a53a5f05690dd077d7861d72ff5b11adc")
    spans = {
        "native929_formatted_label": (0x24D120, 0x40),
        "native930_stop": (0x24D760, 0x30),
        "command543_lookup_and_label": (image.u32(0x239458 + 4 * (543 - 525)) - image.base, 0x62),
        "command544_lookup_and_stop": (image.u32(0x239458 + 4 * (544 - 525)) - image.base, 0x3B),
        "startup_spline_resource_to_config98": (0x1A0BB0, 0x101),
        "manager_config98_constructor_argument": (0x17D1B7, 0x3C),
        "manager_to_wrapper_spline_argument": (0x17A8D0, 0x73),
        "wrapper_fc_three_real_dependencies": (0x170DEC, 0x60),
        "label_start_and_stop": (0x174D40, 0x77),
        "update_spring_then_optional_spline_then_accumulator": (0x173EF7, 0xBD),
        "spline_tick_material_scale_and_accumulator": (0x185BA0, 0x153),
        "material_query_returns_node44": (0x1944A0, 0x5D),
        "material_base_maximum54_to_current_b4_distinct_from_output_scale5c": (0x193D5D, 0x2D),
        "accumulator_registered_name_add": (0x183580, 0x6B),
        "curve_file_parser": (0x1A1F60, 0x94C),
        "curve_sample_loop_and_cubic": (0x1A2890, 0x1B5),
        "curve_coefficient_builder": (0x1A2A50, 0x472),
    }
    archive = image.path.parent / "m2config.int"
    entries, _, cipher = kif_entries(archive, 0x0F6BB0AE)
    entry = next(row for row in entries if row["name"] == b"spline.inf")
    with archive.open("rb") as handle:
        handle.seek(entry["offset"])
        raw = handle.read(entry["size"])
    raw = blowfish_le_decrypt(cipher, raw) if cipher else raw
    text = re.sub(r"/\*.*?\*/", "", raw.decode("cp932"), flags=re.S)
    rows = [line.split("//", 1)[0].split() for line in text.splitlines()]
    rows = [row for row in rows if row]
    calls = {str(row["index"]): row["callsites"] for row in prior["priority_contracts"]
             if row["index"] in (929, 930)}
    report = {
        "schema": "cs2-emote-spline-contracts/v1", "pe_sha256": image.digest,
        "binary_path": prior["binary_path"],
        "evidence": {name: snippet(image, start, size) for name, (start, size) in spans.items()},
        "actual_resource": {"name": "spline.inf", "decoded_sha256": hashlib.sha256(raw).hexdigest(),
                            "bytes": len(raw), "lexical_rows": rows, "curve_evaluated": False},
        "prior_static_kcs_callsites": calls,
        "source_sha256": {name: hashlib.sha256((ROOT / name).read_bytes()).hexdigest() for name in SOURCES},
        "chain": [
            "startup #spline -> real resource bytes -> shared config98 -> manager3c -> wrapper+fc",
            "929/543 start copies raw label and resets first/origin/wrote; 930/544 clears label/enabled only",
            "first eligible update captures raw task tick; elapsed uses wrapping DWORD subtraction",
            "1a1f60 cumulative time points and duplicated loop ->1a2a50 endpoint/tridiagonal coefficients",
            "1a2890 ordered curve sample ->1944a0 node+44 ->fixed output_scale at+18 (node5c)",
            "183580 adds to registered accumulator after spring, before1837a0 driver targets ->PSB pixels",
            "same-owner reselection retains actual spline state; new wrapper/task release uses normal ownership",
        ],
        "limits": [
            "Malformed/nonfinite/degenerate curve data has explicit diagnostics; no IEEE exceptional-domain equivalence claim",
            "965/1048/1027/1047/1028 action-label family is distinct and not completed by this spline family",
            "full M2 object graph persistence and multi-player manager coverage remain separate",
            "static source chain is not full original-function equivalence or game acceptance",
        ],
        "ordinary_routes_added": 0, "full_function_equivalence_added": 0,
        "compiled": False, "type_checked": False, "tests_run": False,
        "curve_or_original_code_executed": False,
    }
    (ROOT / "reports/emote_spline_contracts_20261117.json").write_text(
        json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    if "--syntax" in sys.argv:
        syntax = []
        for name in SOURCES:
            raw_source = (ROOT / name).read_bytes()
            run = subprocess.run([str(Path.home() / ".cargo/bin/rustfmt.exe"), "--edition", "2024",
                                  "--config", "skip_children=true", "--emit", "stdout"],
                                 input=raw_source, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, check=False)
            syntax.append({"path": name, "sha256": hashlib.sha256(raw_source).hexdigest(),
                           "syntax_exit": run.returncode, "diagnostic": run.stderr.decode("utf-8", errors="replace")})
        (ROOT / "archive/2026-11/20261117_emote_spline_source_syntax.json").write_text(
            json.dumps({"grammar_only": True, "compiled": False, "type_checked": False,
                        "tests_run": False, "sources": syntax}, indent=2) + "\n", encoding="utf-8")
        if any(row["syntax_exit"] for row in syntax):
            raise ValueError("Rust grammar read failed; see archived diagnostics")
    print(json.dumps({"native": [929, 930], "command": [543, 544], "source_chain_added": True,
                      "routes_added": 0, "full_equivalence_added": 0, "compiled": False}))


if __name__ == "__main__":
    main()

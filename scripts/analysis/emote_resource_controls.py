"""Read fixed-PE attribute/filter contracts without executing engine code.

--syntax reads Rust grammar through rustfmt stdin, discarding stdout.
No compilation, type checking, tests, VM, filter algorithm or game execution.
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
    "emote_texture.rs", "emote_m2banks.rs", "emote_dispatch.rs", "emote_cpu.rs",
)] + ["simulator/crates/desktop/src/emote_native.rs"]


def main() -> None:
    prior = json.loads((ROOT / "reports/recovery_A_20261116_emote.json").read_text(encoding="utf-8"))
    image = Image(ROOT / prior["binary_path"],
                  "c71c5b85e6eca660474f156d96b0021a53a5f05690dd077d7861d72ff5b11adc")
    driver = Image(image.path.parent / "emotedriver.dll",
                   "85eb459e94e74ab608f649eb7fb440f2526e90aaa1af7276ed01dc10018903ce")
    spans = {
        "native910_three_initialized_outputs": (0x24CCD0, 0x80),
        "native918_raw_setter": (0x24D280, 0x30),
        "command529_query": (image.u32(0x239458 + 4 * (529 - 525)) - image.base, 0x59),
        "command535_setter": (image.u32(0x239458 + 4 * (535 - 525)) - image.base, 0x3E),
        "wrapper_query_two_word_clear": (0x174510, 0x3F),
        "config_query_signed_exact_map": (0x187760, 0x72),
        "attribute_record_defaults_and_three_words": (0x1898A0, 0x20F),
        "attribute_unique_insert": (0x18F010, 0x104),
        "wrapper_filter_default_zero": (0x17107A, 0xA),
        "wrapper_filter_raw_write": (0x1745D0, 0x10),
        "rebuild_starts_with_raw_sources": (0x175900, 0xF5),
        "rebuild_filter_before_driver_creation": (0x175AC8, 0xB1),
        "filter_dispatch": (0x1778F0, 0x80),
        "filter_grayscale_tables": (0x196BB0, 0xF0),
        "filter_negative_rgb_only": (0x196CA0, 0x36),
        "filter_sepia_tables_and_saturation": (0x196CE0, 0x117),
    }
    entries, _, cipher = kif_entries(image.path.parent / "m2config.int", 0x0F6BB0AE)
    attributes = []
    resource_files = 0
    with (image.path.parent / "m2config.int").open("rb") as handle:
        for entry in entries:
            if not entry["name"].endswith((b".dif", b".inc")):
                continue
            resource_files += 1
            handle.seek(entry["offset"])
            raw = handle.read(entry["size"])
            raw = blowfish_le_decrypt(cipher, raw) if cipher else raw
            text = re.sub(r"/\*.*?\*/", "", raw.decode("cp932"), flags=re.S)
            active = False
            rows = []
            for line in text.splitlines():
                tokens = line.split("//", 1)[0].split()
                if not tokens:
                    continue
                if tokens[0].startswith(("#", "$")):
                    active = tokens[0] == "#attribute"
                    continue
                if active:
                    rows.append(tokens)
            if rows:
                attributes.append({"resource": entry["name"].decode("cp932"),
                                   "decoded_sha256": hashlib.sha256(raw).hexdigest(), "rows": rows})
    report = {
        "schema": "cs2-emote-resource-controls/v1", "binary_path": prior["binary_path"],
        "pe_sha256": image.digest, "driver_sha256": driver.digest,
        "evidence": {name: snippet(image, start, size) for name, (start, size) in spans.items()},
        "driver_filter_export": snippet(driver, 0x3790, 0x540),
        "filter_import": image.get(image.u32(0x4C6758), 43).hex(),
        "filter_coefficient_bits_rgb": [image.u32(a) for a in (0x5E4B04, 0x5E4B1C, 0x5E4AF0)],
        "attribute_resource_files_read": resource_files, "actual_attribute_rows": attributes,
        "prior_static_kcs_callsites": {str(r["index"]): r["callsites"]
                                       for r in prior["priority_contracts"] if r["index"] in (910, 918)},
        "source_sha256": {name: hashlib.sha256((ROOT / name).read_bytes()).hexdigest() for name in SOURCES},
        "chains": [
            "910/529 -> primary difference #attribute -> signed exact key -> three raw DWORD outputs",
            "918/535 -> persistent wrapper134 raw word -> next actual resource rebuild -> texture decode/filter -> shared model -> PSB pixels",
            "gray/sepia truncate each byte-table contribution separately; negative preserves alpha",
            "filter participates in shared model cache identity; same-resource reuse is not a rebuild",
            "ordinary VM and owned-buffer receivers share the actual semantic operation",
        ],
        "limits": [
            "original atoi malformed/overflow text domain is not claimed equivalent; parser diagnoses invalid signed DWORDs",
            "other M2 program/action owners, full multi-player manager and complete persistence remain separate",
            "source implementation is not original full-function equivalence or gameplay acceptance",
        ],
        "ordinary_routes_added": 0, "full_function_equivalence_added": 0,
        "compiled": False, "type_checked": False, "tests_run": False,
        "algorithm_or_original_code_executed": False,
    }
    (ROOT / "reports/emote_resource_controls_20261117.json").write_text(
        json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    if "--syntax" in sys.argv:
        syntax = []
        for name in SOURCES:
            raw = (ROOT / name).read_bytes()
            run = subprocess.run([str(Path.home() / ".cargo/bin/rustfmt.exe"), "--edition", "2024",
                                  "--config", "skip_children=true", "--emit", "stdout"],
                                 input=raw, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, check=False)
            syntax.append({"path": name, "sha256": hashlib.sha256(raw).hexdigest(),
                           "syntax_exit": run.returncode, "diagnostic": run.stderr.decode("utf-8", errors="replace")})
        (ROOT / "archive/2026-11/20261117_emote_resource_controls_source_syntax.json").write_text(
            json.dumps({"grammar_only": True, "compiled": False, "type_checked": False,
                        "tests_run": False, "sources": syntax}, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"attribute_files": len(attributes), "attribute_rows": sum(len(r["rows"]) for r in attributes),
                      "source_files": len(SOURCES), "executed_algorithms": False}))


if __name__ == "__main__":
    main()

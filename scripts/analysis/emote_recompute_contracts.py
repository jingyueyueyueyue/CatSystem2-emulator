"""Fixed M2 manager recompute evidence; never execute the receiver or game."""
from __future__ import annotations

import hashlib
import json
from pathlib import Path
import subprocess
import sys

from recovery_A_20261116 import ROOT, Image
from root_clock_profiles import snippet

SOURCES = ["simulator/crates/runtime/src/" + name for name in (
    "emote.rs", "emote_dispatch.rs", "emote_control.rs", "emote_m2live.rs", "emote_cpu.rs",
)] + ["simulator/crates/desktop/src/emote_native.rs"]


def main() -> None:
    prior = json.loads((ROOT / "reports/recovery_A_20261116_emote.json").read_text(encoding="utf-8"))
    image = Image(ROOT / prior["binary_path"],
                  "c71c5b85e6eca660474f156d96b0021a53a5f05690dd077d7861d72ff5b11adc")
    spans = {
        "native932_no_task_or_output": (0x24CB50, 0x17),
        "manager_all_task_groups": (0x17EDC0, 0x6A),
        "group_all_player_wrappers": (0x17C0D0, 0x53),
        "wrapper_scene_mask_material_and_banks": (0x174DC0, 0x354),
        "transition_reset_on_effective_word_change": (0x193400, 0x3C1),
        "blink_request_retained_before_scene_mask": (0x174C20, 0x81),
        "lip_request_retained_before_scene_mask": (0x174CB0, 0x81),
    }
    report = {
        "schema": "cs2-emote-recompute-contracts/v1", "binary_path": prior["binary_path"],
        "pe_sha256": image.digest,
        "evidence": {name: snippet(image, start, size) for name, (start, size) in spans.items()},
        "prior_static_kcs_callsites": next(r["callsites"] for r in prior["priority_contracts"] if r["index"] == 932),
        "source_sha256": {name: hashlib.sha256((ROOT / name).read_bytes()).hexdigest() for name in SOURCES},
        "chain": [
            "932 direct manager API -> all actual loaded task/player owners; no invented command or target",
            "wrapper requested10c -> independent scene active/engine/transition mask -> shared193400 reset",
            "global material level/enable map -> same loaded material groups -> spring/history consumers",
            "requested14c/150 retained separately -> scene blink/lip mask -> same active bank and sidecar consumers",
            "no unconditional907 bank restoration, no timeline clock advance, no forced PSB reload",
        ],
        "limits": [
            "general multi-player manager selection is still separate; this traverses all currently represented live owners",
            "full M2 checkpoint and other procedural/action owners are not completed here",
            "static source integration is not full original-function equivalence or gameplay acceptance",
        ],
        "ordinary_routes_added": 0, "full_function_equivalence_added": 0,
        "compiled": False, "type_checked": False, "tests_run": False, "receiver_or_game_executed": False,
    }
    (ROOT / "reports/emote_recompute_contracts_20261117.json").write_text(
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
        (ROOT / "archive/2026-11/20261117_emote_recompute_source_syntax.json").write_text(
            json.dumps({"grammar_only": True, "compiled": False, "type_checked": False,
                        "tests_run": False, "sources": syntax}, indent=2) + "\n", encoding="utf-8")
        print(json.dumps({"sources": len(syntax), "grammar_failures": sum(row["syntax_exit"] != 0 for row in syntax)}))


if __name__ == "__main__":
    main()

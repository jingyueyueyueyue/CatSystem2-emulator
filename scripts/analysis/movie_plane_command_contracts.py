"""Summarize existing movie wrapper evidence without executing game code.

Reads prior PE navigation and KCS provenance; no resource rescan, Rust build,
VM, plugin or original binary execution is performed.
"""
from __future__ import annotations

from collections import Counter
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
OUTPUT = ROOT / "reports/movie_plane_commands_20261117.json"


def build_report() -> dict:
    inventory = json.loads((ROOT / "reports/kcs_call_inventory_20261116.json").read_text(encoding="utf-8"))
    evidence = "archive/2026-11/20261117_movie_plane_legacy701702_navigation.json"
    legacy = json.loads((ROOT / evidence).read_text(encoding="utf-8"))
    wrappers = []
    for slot in (701, 702, 988, 989):
        calls = []
        for game in inventory["games"]:
            for program in game["programs"]:
                if program["table_hint"] != "ordinary" or program["role"] == "excluded-installer-script":
                    continue
                for call in program["calls"]:
                    if call.get("index") == slot:
                        calls.append({"root": game["root"], "resource": program["resource"],
                                      "offset": call["offset"], "argument_bytes": call["argument_bytes"]})
        wrappers.append({"native": slot, "games": len({call["root"] for call in calls}),
                         "static_calls": len(calls),
                         "argument_bytes": dict(sorted(Counter(str(call["argument_bytes"]) for call in calls).items())),
                         "callsites": calls})
    sources = ["simulator/crates/desktop/src/" + name for name in (
        "movie_plane_commands.rs", "movie_plane_native.rs", "legacy_plane_input.rs",
        "message_platform.rs", "task_commands.rs", "task_commands2.rs", "native_tasks.rs", "plane_native.rs")]
    return {
        "schema": "cs2-movie-plane-commands/static-v1",
        "scope": "Existing static PE/KCS evidence and source identities; no compile, typecheck, test or execution.",
        "wrappers": wrappers,
        "legacy": {"sha256": legacy["sha256"], "path": legacy["path"],
                   "table_va": legacy["table_va"], "evidence": evidence,
                   "entries_rva": {str(row["slot"]): row["entry_rva"] for row in legacy["records"]},
                   "native701": "16 bytes x/y/width/height; context+AC exclusive movie display rectangle; status2",
                   "native702": "8 bytes target/value -> owned command91 -> old type6 BUTTON -> input143 -> existing region words[14]; status2; ordinary/typed/owned/raw paths integrated",
                   "native702_evidence": "archive/2026-11/20261117_format_B_legacy_plane_input.json"},
        "consumer": {
            "load": "ordinary701/988 -> owned XYZ+NUL name -> route_task_command_buffers -> movie PLANE receiver -> real host",
            "raw": "69 -> same typed receiver;70 copies fixed inputs/name now -> existing Body-owned queue",
            "plane_codes": [43, 44, 47, 48, 49, 51, 53, 54, 55, 56, 57, 58, 59, 61, 62, 63, 66, 67, 68, 75, 76, 81, 82, 84, 85, 86, 87, 88, 89, 591, 592],
            "camera_codes": [51, 57, 66, 589, 590],
            "query": "87/88 write handle to data1; receiver output0; ordinary702/989 return actual owned handle; raw sync writes caller VM",
            "native90": "16-byte movie target+XYZ -> same owned command53 -> existing position/render consumer; pending owned inputs survive retry",
            "geometry_queries": "ordinary86/87/89/343/345 and raw69 use the same owned61/62/75/76 receiver; writable first/second outputs -> ordered VM writes; typed61/62 supports the single mutable second buffer; two-output75/76 requires owned buffers; nonzero selector preserves raw destinations and wrapper zero-initialized outputs",
            "deletion": "Only supported real movie/camera mutators are ignored; ordinary geometry queries retain initialized scratch and status/value/writeback; raw deleted receivers are consumed before payload or VM destination access; unrelated object services remain reachable",
            "release": "Existing task queue cleanup and movie host release; no original process pointers retained",
        },
        "remaining": [
            "Queued VM output destinations need retained leases; explicit diagnostic remains",
            "Other drawable replacements and matrix propagation retain their independent recovery boundaries",
            "Original CSS movie checkpoints are not enabled by this command integration",
            "Full per-PE behavior, pixels, playback and whole-game acceptance remain unvalidated",
        ],
        "sources": [{"path": path, "sha256": hashlib.sha256((ROOT / path).read_bytes()).hexdigest()} for path in sources],
        "ordinary_routes_added": 0,
        "completed_native_equivalence_added": 0,
        "compiled": False, "typechecked": False, "tests_run": False, "original_executed": False,
    }


def main() -> None:
    report = build_report()
    OUTPUT.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"wrappers": [{k: row[k] for k in ("native", "games", "static_calls", "argument_bytes")} for row in report["wrappers"]],
                      "ordinary_routes_added": 0, "completed_native_equivalence_added": 0}, ensure_ascii=True))


if __name__ == "__main__":
    main()

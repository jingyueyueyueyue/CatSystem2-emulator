"""Read fixed action bytes/resources and source hashes; never run an action."""
from __future__ import annotations

from collections import Counter
import hashlib
import json
import re
import sys

from audit_runtime_evidence import blowfish_le_decrypt, kif_entries
from recovery_A_20261116 import ROOT, Image
from root_clock_profiles import snippet


def main() -> None:
    prior = json.loads((ROOT / "reports/recovery_A_20261116_emote.json").read_text(encoding="utf-8"))
    image = Image(ROOT / prior["binary_path"], prior["PE_sha256"])
    spans = {
        "action_ctor": (0x1914C0, 0xA8),
        "action_select_and_saved_name_union": (0x1915F0, 0x1F4),
        "skip_until_restore_flag": (0x1917F0, 0x117),
        "capture_gate_wait_and_one_step": (0x191910, 0x203),
        "clear_saved_without_cancelling_action": (0x175830, 0x44),
        "wrapper_start": (0x175880, 0x19),
        "wrapper_skip": (0x1758A0, 0x12),
        "wrapper_busy": (0x1758C0, 0x13),
        "action_before_driver_tick": (0x174040, 0x162),
        "parse_action_sections_and_names": (0x1A99F0, 0x299),
        "parse_steps_flush_reset_and_directives": (0x1A9D70, 0x45F),
        "lookup_step_bounds": (0x1A9900, 0x89),
        "lookup_saved_name_union": (0x1A9990, 0x58),
    }
    for code, size in ((550, 0x31), (551, 0x62), (552, 0x35), (553, 0x38)):
        spans[f"owned_command_{code}"] = (image.u32(0x239458 + 4 * (code - 525)) - image.base, size)
    entries, _, cipher = kif_entries(image.path.parent / "m2config.int", 0x0F6BB0AE)
    row = next(row for row in entries if row["name"] == b"action.def")
    with (image.path.parent / "m2config.int").open("rb") as handle:
        handle.seek(row["offset"])
        raw = handle.read(row["size"])
    raw = blowfish_le_decrypt(cipher, raw) if cipher else raw
    text = re.sub(r"/\*.*?\*/", "", raw.decode("cp932"), flags=re.S)
    rows = [line.split("//", 1)[0].split() for line in text.splitlines()]
    rows = [row for row in rows if row]
    sources = ["simulator/crates/runtime/src/" + name for name in (
        "emote.rs", "emote_m2action.rs", "emote_m2live.rs", "emote_cpu.rs", "emote_dispatch.rs",
    )] + ["simulator/crates/desktop/src/emote_native.rs"]
    report = {
        "schema": "cs2-emote-action-contracts/v1", "pe_sha256": image.digest,
        "binary_path": prior["binary_path"],
        "evidence": {name: snippet(image, rva, size) for name, (rva, size) in spans.items()},
        "resource": {"name": "m2config.int::action.def", "bytes": len(raw),
                     "sha256": hashlib.sha256(raw).hexdigest(),
                     "labels": sum(row[0].startswith("$") for row in rows),
                     "directives": dict(Counter(row[0] for row in rows if not row[0].startswith(("#", "$")))),
                     "missing_var_operand_rows": [row for row in rows if row[0] == "var" and len(row) < 3]},
        "prior_callsites": {str(row["index"]): row["callsites"] for row in prior["priority_contracts"]
                            if row["index"] in (1027, 1047, 1048, 1028)},
        "source_sha256": {name: hashlib.sha256((ROOT / name).read_bytes()).hexdigest() for name in sources},
        "chain": ["#action resource -> shared config -> persistent wrapper180 action owner",
                  "1027/551 select+saved-name union; wrapper returns actual zero",
                  "chain count and driverf0 gate -> one step before driverfc -> existing timed targets",
                  "1047/552 skips ordinary steps until restore flag; current wait is cleared",
                  "1048/550 clears saved values; action/cursor continue",
                  "owned553 queries actual current step through typed backend API; native1028 remains blocked",
                  "wait compares original u32 deadline; restore uses captured actual driver values"],
        "limits": ["1028 native4-byte ABI/args+4 formatting conflict remains explicitly blocked",
                   "965 wrapper100 procedural owner is distinct and still separate",
                   "full multi-player, binary checkpoint and full native equivalence remain separate"],
        "compiled": False, "type_checked": False, "tests_run": False,
        "action_or_original_executed": False, "full_function_equivalence_added": 0,
    }
    (ROOT / "reports/emote_action_contracts_20261117.json").write_text(
        json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"resource": report["resource"], "prior_callsites": report["prior_callsites"]}, ensure_ascii=True))


if __name__ == "__main__":
    main()

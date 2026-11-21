"""Read the fixed secondary/scalar ABI and source bytes, without execution.

The optional grammar pass feeds Rust bytes to rustfmt and discards its output;
it does not compile, type-check, load an engine or execute its algorithms.
"""
from __future__ import annotations

import hashlib
import json
from pathlib import Path
import subprocess
import sys

from audit_runtime_evidence import blowfish_le_decrypt, kif_entries
from recovery_A_20261116 import ROOT, Image
from recovery_A_emote_host_20261117 import sections
from root_clock_profiles import snippet

PE = "c71c5b85e6eca660474f156d96b0021a53a5f05690dd077d7861d72ff5b11adc"
SOURCES = ["simulator/crates/runtime/src/" + name for name in (
    "emote_m2config.rs", "emote_m2live.rs", "emote_m2banks.rs",
    "emote_m2values.rs", "emote_m2timed.rs", "emote_m2physics.rs", "emote_surface.rs", "emote_variable_timeline.rs", "emote_dispatch.rs", "emote_cpu.rs",
)] + ["simulator/crates/desktop/src/emote_native.rs"]


def main() -> None:
    prior = json.loads((ROOT / "reports/recovery_A_20261116_emote.json").read_text(encoding="utf-8"))
    image = Image(ROOT / prior["binary_path"], PE)
    driver = Image(image.path.parent / "emotedriver.dll",
                   "85eb459e94e74ab608f649eb7fb440f2526e90aaa1af7276ed01dc10018903ce")
    target = image.u32(0x239458 + 4 * (533 - 0x20D)) - image.base
    spans = {
        "native917_wrapper": (0x24D230, 0x41),
        "command533_switch_receiver": (target, 0x9D),
        "manager_secondary_lookup": (0x17DB90, 0x16D),
        "secondary_spec_parser": (0x17E950, 0x21D),
        "secondary_prepass": (0x173570, 0x24A),
        "secondary_shared_chain_and_selection_commit": (0x173BC0, 0x182),
        "shared_chain_owner_submission": (0x177168, 0x2B),
        "one_sided_endpoint_first_entry": (0x176239, 0x57),
        "one_sided_endpoint_advances_every_entry": (0x176290, 0x45),
        "shared_planner_advances_every_entry": (0x176A8D, 0x43),
        "command530_speed_receiver": (image.u32(0x239458 + 4 * (530 - 0x20D)) - image.base, 0x73),
        "command532_weight_receiver": (image.u32(0x239458 + 4 * (532 - 0x20D)) - image.base, 0x73),
        "native911_speed_all": (0x24D350, 0x40),
        "native912_speed_named": (0x24D300, 0x50),
        "native913_speed_table": (0x24D390, 0xC0),
        "native914_weight_all": (0x24D570, 0x40),
        "native915_weight_named": (0x24D520, 0x50),
        "native916_weight_table": (0x24D5B0, 0xC0),
        "speed_named_value28_chain5c": (0x16D800, 0x179),
        "speed_all_current_records": (0x16D980, 0xC0),
        "weight_named_value2c_chain60": (0x16DB40, 0x1CC),
        "weight_all_current_records": (0x16DD10, 0xE0),
        "scalar_default_one_and_registered_lookup": (0x16F010, 0x71),
        "chain_constructor_scalar_lookup": (0x16CF70, 0x49),
        "chain_count58_speed5c_weight60_distinct": (0x16D17B, 0xA5),
        "chain_duration_divides_speed_and_target_multiplies_weight": (0x16D411, 0xE2),
        "value_weight_rescales_owned_samples": (0x16E630, 0x140),
        "owner_value_constructor_defaults": (0x16CB00, 0x110),
        "native920_material_hold_named": (0x24D030, 0x50),
        "native921_material_hold_reset_and_batch": (0x24D450, 0xD0),
        "command538_material_hold": (image.u32(0x239458 + 4 * (538 - 0x20D)) - image.base, 0x63),
        "command539_material_hold_reset": (image.u32(0x239458 + 4 * (539 - 0x20D)) - image.base, 0x3D),
        "player_optional_force_owner_hold": (0x1745E0, 0x2F),
        "named_hold_raw_compare_and_reset": (0x1937D0, 0x83),
        "reset_nonzero_holds": (0x193860, 0x8E),
        "spring_enable_hold_integration_and_disabled_reset": (0x194730, 0x18F),
        "native966_name_table_sends531": (0x24D160, 0xAF),
        "command531_ignores_name_resets_whole_owner": (image.u32(0x239458 + 4 * (531 - 0x20D)) - image.base, 0x4C),
        "reset_all_value2c_and_chain60_to_one": (0x16DA50, 0xEE),
        "native909_busy_query": (0x24CF10, 0x60),
        "command528_output_from_real_manager": (image.u32(0x239458 + 4 * (528 - 0x20D)) - image.base, 0x36),
        "manager_active_chain_count_and_virtualf0": (0x17BFA0, 0x89),
        "native908_finish": (0x24D730, 0x30),
        "command527_manager_finish": (image.u32(0x239458 + 4 * (527 - 0x20D)) - image.base, 0x35),
        "task_manager_finish_lookup": (0x17E4D0, 0x55),
        "finish_stage_driver_update_chain_release_and_blend": (0x17C1B0, 0x10E),
    }
    evidence = {name: snippet(image, start, length) for name, (start, length) in spans.items()}
    if not any(row["mnemonic"] == "call" and row["operands"] == "0x57db90"
               for row in evidence["command533_switch_receiver"]["instructions"]):
        raise ValueError("identified M2 secondary manager receiver changed")
    archive = image.path.parent / "m2config.int"
    entries, _, cipher = kif_entries(archive, 0x0F6BB0AE)
    entry = next(entry for entry in entries if entry["name"] == b"body.dif")
    with archive.open("rb") as handle:
        handle.seek(entry["offset"])
        raw = handle.read(entry["size"])
    raw = blowfish_le_decrypt(cipher, raw) if cipher else raw
    config = sections(raw)
    report = {
        "schema": "cs2-emote-secondary-contracts/v1",
        "pe_sha256": PE, "binary_path": prior["binary_path"], "evidence": evidence,
        "driver_evidence": {"sha256": driver.digest,
            "virtualf0_receiver_timers_nonloop_timeline_and_exclusions": snippet(driver, 0x11DC0, 0x5A0),
            "vtable_f0_f4_f8_fc": {"rva": 0x7CAE8,
                "targets_rva": [driver.u32(0x7CAE8 + offset) - driver.base for offset in range(0, 16, 4)]},
            "bridge_f4_and_fc": snippet(driver, 0x3520, 0x31),
            "finish_all_driver_receivers": snippet(driver, 0x12DB0, 0x85),
            "finish_nonloop_timeline_remove_loop_scalar_complete": snippet(driver, 0x1FBF0, 0x177),
            "scalar_finish_latest_endpoint": snippet(driver, 0xD920, 0xE3),
            "driver_one_frame_ordinary_update": snippet(driver, 0x12BB0, 0x1FA)},
        "resource": {"name": "body.dif", "sha256": hashlib.sha256(raw).hexdigest(),
                     "dimension_rows": len(config.get("dif", [])),
                     "pattern_rows": len(config.get("pattern", [])),
                     "chain_rows": len(config.get("chain", []))},
        "source_sha256": {name: hashlib.sha256((ROOT / name).read_bytes()).hexdigest() for name in SOURCES},
        "chain": [
            "native917 and owned command533 -> separate comma dimensions/colon existing-player key",
            "actual secondary config/selection retained by the real M2 owner",
            "173570 prepass -> last Fade duration -> owned old RGBA surface",
            "1766b0 shared sample planner -> shared same-name value/chain owner -> real driver timers",
            "ordinary PSB advance/strict renderer and existing resource/task release",
        ],
        "scalar_chain": [
            "911/912/913 -> command530 -> all/named/ordered-table FloatBits -> shared registered value/chain speed",
            "914/915/916 -> command532 -> all/named/ordered-table FloatBits -> shared registered value/chain weight",
            "owned task buffers use the same command receiver; unknown names do not create values",
            "new chains inherit actual registered defaults; duration=frames/speed5c, target=actual_target80*weight60+offset34",
            "initialized non-Fix chain queries the real driver without inventing a restart",
            "966 emits531 per actual table entry; receiver ignores each name and resets all current weights to1",
        ],
        "material_hold_chain": [
            "920 named raw / 921 reset539 followed by ordered538 -> actual optional material owner",
            "1937d0 stores complete raw d0; only a changed raw word resets displacement/velocity/normalized",
            "193860 resets nonzero d0 to zero; unknown names never allocate materials",
            "194730: disabled resets history, held skips spring integration, unheld uses existing integer-frame spring and driver output",
            "same resource reselection clones the real material owner; task/resource release drops its hold word",
        ],
        "busy_query_chain": [
            "909 / owned command528 -> manager active surface blend, actual owned chain count, real driver busy",
            "driver busy uses receiver timers except timeline-owned names and nonlooping timeline lifetime",
            "looping timelines alone do not report busy; pending surface request is distinct from active manager blend",
            "query returns the actual word to VM or owned message output without zeroing it afterwards",
        ],
        "finish_chain": [
            "908/527 ->17e4d0 task lookup ->17c1b0 actual pending chain consumer",
            "stage real M2 owner values -> virtualf4 receiver finish -> virtualfc update1 -> consume chain output",
            "nonloop timeline schedules lastTime and retires; looping labels remain and finish current scalar endpoint",
            "release pending chain records while retaining registered values and consume once again",
            "active blend finishes at fraction1, releasing old RGBA; original M2 clock/blink/lip are not advanced",
        ],
        "corrected_prior_interpretation": {
            "sample_count_minus_one_field": "58",
            "duration_divisor_field": "5c (actual speed, default 1)",
            "target_weight_field": "60 (actual weight, default 1)",
            "count_is_duration_divisor": False,
            "older_intermediate_report_superseded": True,
        },
        "remaining": [
            "nonzero same-resource profile replacement remains a separate explicit guard",
            "quoted/escaped specification grammar",
            "distinct value/chain offset setter and other unrecovered M2 operations",
            "nonfinite scalar inputs retain an explicit diagnostic; not a full IEEE exceptional-domain claim",
            "full function equivalence and actual game acceptance",
        ],
        "ordinary_routes_added": 0, "full_function_equivalence_added": 0,
        "compiled": False, "type_checked": False, "tests_run": False,
        "game_or_original_code_executed": False,
    }
    destination = ROOT / "reports/emote_secondary_contracts_20261117.json"
    destination.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    if "--syntax" in sys.argv:
        rows = []
        for name in SOURCES:
            raw = (ROOT / name).read_bytes()
            run = subprocess.run(
                [str(Path.home() / ".cargo/bin/rustfmt.exe"), "--edition", "2024",
                 "--config", "skip_children=true", "--emit", "stdout"],
                input=raw, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, check=False,
            )
            rows.append({"path": name, "sha256": hashlib.sha256(raw).hexdigest(),
                         "syntax_exit": run.returncode,
                         "diagnostic": run.stderr.decode("utf-8", errors="replace")})
        destination = ROOT / "archive/2026-11/20261117_emote_secondary_source_syntax.json"
        destination.write_text(json.dumps({"grammar_only": True, "compiled": False,
            "type_checked": False, "tests_run": False, "sources": rows}, indent=2) + "\n", encoding="utf-8")
        if any(row["syntax_exit"] for row in rows):
            raise ValueError("Rust grammar read failed; see archived diagnostics")
    print(json.dumps({"native": [908, 909, 911, 912, 913, 914, 915, 916, 917, 920, 921, 966], "command": [527, 528, 530, 531, 532, 533, 538, 539], "source_chain_added": True,
                      "routes_added": 0, "full_equivalence_added": 0, "compiled": False}))


if __name__ == "__main__":
    main()

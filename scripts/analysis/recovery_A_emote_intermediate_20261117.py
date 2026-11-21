"""Bounded SHA-bound intermediate/compound source handoff, no runtime execution.

Only reads fixed PE snippets, prior declared counters and current source bytes.
Does not compile, test, execute Rust/game code or rescan the function ledger.
"""
from datetime import datetime, timezone
import json

from recovery_A_20261116 import ROOT, Image, read_json, sha
from recovery_A_receiver_contracts_20261116 import block


def main():
    identity = read_json(ROOT / "reports/recovery_A_20261116_emote.json")
    exe = Image(ROOT / identity["binary_path"], identity["PE_sha256"])
    driver = Image(exe.path.parent / "emotedriver.dll",
                   "85eb459e94e74ab608f649eb7fb440f2526e90aaa1af7276ed01dc10018903ce")
    prior = read_json(ROOT / "reports/recovery_A_20261117_emote_shared_bank_summary.json")
    remaining = [f for f in prior["remaining_implementation_families"]
                 if f["id"] not in {"intermediate_chain", "compound_dimensions"}]
    sources = ["simulator/crates/runtime/src/emote_" + stem + ".rs" for stem in (
        "m2chain", "m2banks", "m2live", "m2values", "m2timed", "cpu")]
    sources.append("scripts/analysis/recovery_A_emote_intermediate_20261117.py")
    summary = {
        "primary_exact_pairs_inherited_not_rescanned": 102,
        "primary_source_blocked_pairs_remaining": 0,
        "intermediate_primary_samples_and_native_duration_source": True,
        "alias_exact_then_reversed_then_fade_fix_default": True,
        "NoMove_reverse_pair_excluded": True,
        "all_alias_intermediate_labels_preserved": True,
        "same_start_smooth_profile_maps_source": True,
        "primary_shared_name_dimension_last_write_wins": True,
        "relative_force_offsets_are_separate_remaining_scope": True,
        "every_enabled_entry_introduces_output_name": True,
        "all_matching_entries_preserve_duplicates_in_samples": True,
        "missing_pattern_entries_do_not_insert_zero_samples": True,
        "actual_target_saved_before_sample_corrections": True,
        "duration_is_frames_divided_by_actual_speed_5c": True,
        "sample_count_minus_one_is_distinct_field_58": True,
        "positive_infinite_duration_preserves_native_zero_rate": True,
        "positive_duration_equal_scalar_keeps_live_receiver": True,
        "invented_equal_time_segment_queue": False,
        "remaining_implementation_family_count_before": 11,
        "remaining_implementation_family_count": len(remaining),
        "remaining_family_count_is_declared_scope_not_all_function_count": True,
        "timed_labels_pending_final_combined_runtime_validation": 60,
        "pending_runtime_validation_is_not_missing_source": True,
        "full_NEKO_strict_pixel_claim": False,
        "completed_native_count_added": 0,
        "compiled": False, "tests_run": False, "Rust_execution": False,
        "game_or_DLL_executed": False,
    }
    spans = {
        "alias_fade_set_parser": (0x1973A5, 0xDF),
        "alias_fix_set_parser": (0x1974D7, 0xC7),
        "alias_exact_pair_sequence": (0x197790, 0x170),
        "alias_reverse_and_NoMove_exception": (0x197932, 0x21A),
        "alias_fade_then_fix_then_default": (0x197B8D, 0x2D7),
        "enabled_entries_per_later_pattern": (0x176876, 0x211),
        "per_entry_loop_reset_and_full_vector_advance": (0x176A8D, 0x43),
        "initial_from_does_not_introduce_output": (0x1769CB, 0xBA),
        "append_all_matching_enabled_entries": (0x176D54, 0x220),
        "target_saved_before_sample_rewrite": (0x176F80, 0x29),
        "smooth_uses_actual_driver_base": (0x17709C, 0x62),
        "same_start_maps_then_owned_record_call": (0x1770FE, 0x95),
        "same_config_import": (0x189489, 0x60),
        "start_config_import": (0x18965A, 0x49),
        "smooth_config_import": (0x1897F2, 0x1B),
        "same_map_nearest_and_lower_tie": (0x19665E, 0xC3),
        "start_map_first_exact_replacement": (0x18268E, 0x53),
        "sample_count_minus_one": (0x16D066, 0x47),
        "same_name_record_replaced_not_added": (0x16D17B, 0xB4),
        "one_target_duration_divisor": (0x16D411, 0x5A),
        "initialized_nonfix_does_not_submit_next_sample": (0x16D2D2, 0x99),
        "actual_target_plus_distinct_offset": (0x16D577, 0x61),
        "relative_offset_is_separate_setter": (0x16DE00, 0x91),
    }
    payload = {
        "schema": "cs2-recovery-A-emote-intermediate/v1", "task_date": "2026-11-17",
        "generated_at_utc": datetime.now(timezone.utc).isoformat(),
        "summary": summary,
        "EXE": {"path": identity["binary_path"], "sha256": exe.digest},
        "driver": {"path": str(driver.path.relative_to(ROOT)), "sha256": driver.digest},
        "APIs": {
            "host_entry_unchanged": "EmoteServices::import_m2_resource",
            "selection": "CpuM2PatternChains::select",
            "samples": "CpuM2ControllerConfig::chain_requests (internal)",
            "request": "CpuM2TimedRequest{ name,target,frames,kind,samples }::driver_duration",
            "shared_owner": "CpuM2ValueTable::queue_pattern/begin_update",
        },
        "contracts": [
            "197760 exact pair first; reverse pair excludes stored NoMove6 and reverses intermediate labels.",
            "Without a pair, either endpoint in fade set wins, then fix set, otherwise alias default; use alias frames.",
            "1766b0 introduces names from every enabled entry in each sequence pattern after initial from;176aca loops through176851 for each0x28 entry.",
            "Collect every enabled matching entry in sequence order; repeated names append repeatedly; absent entries append nothing.",
            "Save actual last target80 before #same/#start/#smooth correction of samples.",
            "#same exact members stay, otherwise choose closest-to-neighbor including original; equal distance favors lower value.",
            "#start replaces only first exact sample match; #smooth replaces first sample by actual controller base and skips other maps.",
            "16cf70 replaces same-name chain records; later primary dimensions win, NoMove skips replacement.",
            "16d250 submits actual target once with frames/speed5c; count-1 is a distinct field58. Initialized non-fix records query the driver, not a next-sample queue.",
            "Primary chain replacement is not relative/inertia offset accumulation; secondary_weights/material_force remain.",
        ],
        "EXE_evidence": {key: block(exe, *span) for key, span in spans.items()},
        "driver_evidence": {"scalar_positive_duration_reciprocal_and_live_state": block(driver, 0xDA10, 0x105)},
        "remaining_implementation_families": remaining,
        "sources": [{"path": p, "sha256": sha((ROOT / p).read_bytes())} for p in sources],
        "classification": "scripts/analysis + reports; no temporary artifacts, game asset outputs or new directories",
    }
    prefix = ROOT / "reports/recovery_A_20261117_emote_intermediate"
    payload["modified_paths"] = sources + [str(prefix.with_suffix(ext).relative_to(ROOT))
                                              for ext in (".json", ".md")] + [
        "reports/recovery_A_20261117_emote_intermediate_summary.json",
        "reports/recovery_A_20261117_emote_current_summary.json"]
    prefix.with_suffix(".json").write_text(json.dumps(payload, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    small = {key: payload[key] for key in ("schema", "task_date", "summary", "APIs",
             "remaining_implementation_families", "classification")}
    prefix.with_name(prefix.name + "_summary.json").write_text(json.dumps(small, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    prefix.with_suffix(".md").write_text(
        "# A / intermediate 与 primary compound（2026-11-17）\n\n"
        "实际源码已接入现有906→CPU owner，不需要新增宿主接线。alias按 exact、反向pair、fade集合、fix集合、default顺序选择；反向NoMove禁用。\n\n"
        "1766b0从from之后每个pattern的全部有效entry引入输出变量名，再按完整sequence顺序收集全部同名有效值；重复entry保留，缺项不补零。"
        "actual target在样本修正前保存；新增#same近邻/低值平局、#start首样本替换、#smooth真实driver base修正。"
        "16cf70对同名chain直接覆盖，后dimension写入胜出；相对offset仍属于secondary/force族。\n\n"
        "16d42f实际提交duration=frames/speed5c与原末值；count−1另存58；initialized非fix只查driver，没有按intermediate逐段等时提交。"
        "源码保留真实raw/scaled向量，未凭名称发明队列。正duration同值scalar仍有live状态；正无穷duration按原版倒数产生rate0，NaN保留明确诊断。\n\n"
        "本批声明实现族11→9，102/102沿用前批静态路径计数，没有重扫、没有新增完整native完成计数。"
        "60 timed标签待最终统一运行验证，未计作缺源码；不宣称完整NEKO像素兼容。\n\n"
        "剩余：surface混合、secondary权重、material/force、LPD自动preset、sidecar切换、非默认quiet、graph非有限域、mesh层级、stencil调度。"
        "脚本归scripts/analysis，报告归reports；无临时产物、无游戏资产输出。仅rustfmt语法读取，未编译、测试或执行运行时。\n",
        encoding="utf-8")
    current_path = ROOT / "reports/recovery_A_20261117_emote_current_summary.json"
    current = read_json(current_path)
    current["intermediate_batch"] = small
    current["summary"]["remaining_implementation_family_count"] = len(remaining)
    current["summary"]["remaining_strict_blockers"] = [f["reason"] for f in remaining]
    current["summary"]["backend_native_branches"]["906"] = (
        "primary timed graph/scalar/selector, shared bank, NoMove/Fade, intermediate sample/divisor and last-write compound source; secondary/model-surface remain")
    current["source_hashes_current"] = payload["sources"]
    report = str(prefix.with_suffix(".md").relative_to(ROOT)).replace("\\", "/")
    if report not in current.setdefault("reports", []): current["reports"].append(report)
    current_path.write_text(json.dumps(current, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(summary, ensure_ascii=True))


if __name__ == "__main__":
    main()

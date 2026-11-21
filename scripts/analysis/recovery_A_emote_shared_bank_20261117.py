"""Fixed-PE primary shared value/bank ownership, static source handoff.

No Rust execution, tests, builds, game/DLL execution, CFG sweep or asset
output. Counts below describe actual resource conditions and source branches.
"""
from __future__ import annotations

from datetime import datetime, timezone
import json

from recovery_A_20261116 import ROOT, Image, read_json, sha
from recovery_A_receiver_contracts_20261116 import block


def main():
    identity = read_json(ROOT / "reports/recovery_A_20261116_emote.json")
    exe = Image(ROOT / identity["binary_path"], identity["PE_sha256"])
    driver = Image(exe.path.parent / "emotedriver.dll",
                   "85eb459e94e74ab608f649eb7fb440f2526e90aaa1af7276ed01dc10018903ce")
    graph = read_json(ROOT / "reports/recovery_A_20261117_emote_graph_selector.json")
    pairs = []
    for old in graph["configured_static_pairs"]:
        record = dict(old)
        record["previous_static_blockers"] = old["remaining_static_blockers"]
        record["remaining_static_blockers"] = [
            reason for reason in old["remaining_static_blockers"]
            if not reason.startswith("selected blink bank shares the timed variable")
        ]
        record["fits_family_endpoint_subset"] = not record["remaining_static_blockers"]
        record["shared_bank_ownership_source"] = (
            bool(record["previous_static_blockers"])
            and not record["remaining_static_blockers"])
        pairs.append(record)
    remaining_families = [
        {"id": "compound_dimensions", "reason": "same-name primary pattern dimensions need compound ownership"},
        {"id": "intermediate_chain", "reason": "intermediate pattern sample scheduling and special per-pattern fix/fade alias maps"},
        {"id": "model_surface_fade", "reason": "old-model replacement surface blending; distinct from implemented primary chain Fade enum5"},
        {"id": "secondary_weights", "reason": "secondary smooth/weighted ownership and live offset production"},
        {"id": "material_force", "reason": "complete material/inertia force owner"},
        {"id": "LPD_preset", "reason": "automatic exact LPD origin/weights preset"},
        {"id": "sidecar_switch", "reason": "active external lip sidecar to waveform lifetime switching"},
        {"id": "waveform_nondefault_quiet", "reason": "nondefault record78 quiet-scan state"},
        {"id": "graph_nonfinite_domain", "reason": "original nonfinite inter-segment pow path keeps diagnostic"},
        {"id": "mesh_hierarchy", "reason": "generic additive/inherited mesh/skin hierarchy"},
        {"id": "stencil_scheduler", "reason": "joined/inherited stencil schedule and mode1 fallback"},
    ]
    summary = {
        "primary_shared_absolute_value_bank_source_implemented": True,
        "bank_outputs_are_not_added_to_timed_values": True,
        "bank_outputs_do_not_directly_replace_render_driver_values": True,
        "bank_absolute_writes_deferred_to_next_chain_pass": True,
        "raw_and_scaled_last_samples_updated": True,
        "initialized_nonfix_chain_queries_without_restarting": True,
        "fixed_chain_reissues_real_absolute_target": True,
        "native_f0_lifetime_and_active_timeline_exclusions_in_source": True,
        "blink_lip_907_927_928_and_waveform_reset_use_shared_owner": True,
        "same_variable_timed_timeline_blanket_guard_removed": True,
        "NoMove_keeps_existing_values_chains_and_selects_banks": True,
        "NoMove_alone_skips_global_bank_restore": True,
        "same_owner_selection_preserves_live_waveform_flow_and_phase": True,
        "bank_selection_clears_timestamps_without_reseeding_flow": True,
        "primary_Fade_enum5_actual_zero_duration_target_in_source": True,
        "primary_Fade_retains_nonfix_owner_lifetime": True,
        "primary_Fade_not_counted_as_model_surface_blending": True,
        "primary_exact_pairs": len(pairs),
        "primary_explicit_pair_NoMove_count": sum(p["kind"] == "nomove" for p in pairs),
        "primary_explicit_pair_Fade_count": sum(p["kind"] == "fade" for p in pairs),
        "previous_source_blocked_pairs": sum(bool(p["previous_static_blockers"]) for p in pairs),
        "source_blocked_pairs_removed": sum(p["shared_bank_ownership_source"] for p in pairs),
        "pairs_fitting_implemented_primary_source": sum(p["fits_family_endpoint_subset"] for p in pairs),
        "source_blocked_pairs_remaining": sum(not p["fits_family_endpoint_subset"] for p in pairs),
        "remaining_implementation_family_count": len(remaining_families),
        "remaining_family_count_is_declared_scope_not_all_function_count": True,
        "first_model_controller_family_labels": 115,
        "timed_labels_pending_final_combined_runtime_validation": 60,
        "pending_runtime_validation_is_not_missing_source": True,
        "full_NEKO_strict_pixel_claim": False,
        "completed_native_count_added": 0,
        "compiled": False, "tests_run": False, "Rust_execution": False,
        "game_or_DLL_executed": False,
    }
    sources = [
        "simulator/crates/runtime/src/emote.rs",
        "simulator/crates/runtime/src/emote_m2values.rs",
        "simulator/crates/runtime/src/emote_m2live.rs",
        "simulator/crates/runtime/src/emote_m2timed.rs",
        "simulator/crates/runtime/src/emote_variable_timeline.rs",
        "simulator/crates/runtime/src/emote_cpu.rs",
        "simulator/crates/runtime/src/emote_m2wave.rs",
        "scripts/analysis/recovery_A_emote_shared_bank_20261117.py",
    ]
    apis = {
        "host_unchanged": "EmoteServices::import_m2_resource/play_timeline/advance",
        "owner": "CpuM2ValueTable::queue_pattern/write_absolute/begin_update",
        "lifetime": "CpuVariableTimelines::owner_busy_inputs + CpuM2TimedVariables::has_active_except",
        "inspection": "playback_info().m2_owner.{owner_value_records,owner_chain_records,deferred_bank_writes}",
        "controller_query": "CpuPlayer::controller_variables; driver virtual94=2b40->133e0 excludes independently added timeline diffs",
    }
    exe_spans = {
        "original_chain_before_bank_before_driver": (0x173F13, 0x11),
        "original_blink_pass": (0x173FD0, 0x27),
        "original_driver_advance": (0x174156, 0x46),
        "absolute_value_compare_and_dirty": (0x16E00D, 0x3F),
        "absolute_last_target_and_raw_scaled_samples": (0x16E164, 0x86),
        "dirty_value_stage_before_chain_pass": (0x16D28F, 0x2C),
        "refresh_existing_last_sample_suppresses_new_chain": (0x16E878, 0x55),
        "create_absolute_fix_pair_from_real_driver": (0x16E969, 0x75),
        "initialized_nonfix_actual_driver_query": (0x16D2D2, 0x99),
        "fixed_or_first_timed_setter": (0x16D577, 0x61),
        "global_animation_query_and_chain_release": (0x16D787, 0x3B),
        "record_default_weight_and_dirty": (0x16E063, 0x3E),
        "NoMove_enum6_skips_chain_write": (0x17329A, 0x4B),
        "common_blink_selection_after_NoMove": (0x1730EA, 0x34),
        "common_lip_selection_after_NoMove": (0x173150, 0x3A),
        "only_non_NoMove_sets_global_bank_reset": (0x1733CA, 0x8C),
        "blink_selection_only_clears_active_and_timestamps": (0x184AE0, 0x3A),
        "lip_selection_keeps_live_phase_6c_and_flow": (0x184BD0, 0x3A),
        "Fade_enum5_zero_duration_target": (0x16D36B, 0x56),
        "Fade_enum5_jump_table": (0x16D7E8, 24),
        "Fade_named_token_to_enum5": (0x197F0A, 0x1D),
        "intermediate_actual_last_sample_and_target": (0x176F80, 0x29),
        "intermediate_endpoint_adjustment_and_chain_call": (0x1770FE, 0x97),
        "intermediate_count_to_owned_record": (0x16D038, 0x76),
    }
    driver_spans = {
        "f0_vtable_pointer": (0x7C9F8 + 0xF0, 4),
        "f0_bridge": (0x3510, 8),
        "active_timeline_names_and_nonloop_busy": (0x11E86, 0xE0),
        "active_eye_receiver_and_timeline_name_exclusion": (0x120DA, 0x52),
        "excluded_set_range_count": (0x25EF0, 0x68),
        "virtual94_base_value_query_bridge": (0x2B40, 0x19),
        "base_query_map_not_render_additive": (0x134E1, 0xA),
        "render_additive_application_separate_from_query": (0x13560, 0x1D),
    }
    payload = {
        "schema": "cs2-recovery-A-emote-shared-bank/v1", "task_date": "2026-11-17",
        "generated_at_utc": datetime.now(timezone.utc).isoformat(),
        "summary": summary, "APIs": apis, "remaining_implementation_families": remaining_families,
        "EXE": {"path": identity["binary_path"], "sha256": exe.digest},
        "driver": {"path": str(driver.path.relative_to(ROOT)), "sha256": driver.digest},
        "configured_primary_pairs": pairs,
        "EXE_evidence": {key: block(exe, *span) for key, span in exe_spans.items()},
        "driver_evidence": {key: block(driver, *span) for key, span in driver_spans.items()},
        "sources": [{"path": p, "sha256": sha((ROOT / p).read_bytes())} for p in sources],
        "batch_deltas": [
            {"batch": "shared absolute value/bank owner", "blocked_pairs_before": 20, "blocked_pairs_after": 0,
             "remaining_declared_implementation_families_after": 12},
            {"batch": "NoMove selection and primary Fade enum5", "blocked_pairs_before": 0, "blocked_pairs_after": 0,
             "remaining_declared_implementation_families_after": len(remaining_families),
             "scope_clarification": "primary Fade implemented; separate old-model surface blend still absent"},
        ],
        "next_intermediate_contract": {
            "callee_RVA": "1766b0",
            "argument_order": ["dimension", "kind_enum", "frames", "CP932_pattern_sequence", "actual_profile"],
            "owned_record_builder_RVA": "16cf70",
            "chain_record_fields": {
                "40": "raw float sample vector", "4c": "scaled float sample vector",
                "5c": "float(sample_count-1)", "68": "start task clock",
                "6c": "end task clock", "70": "kind enum", "80": "actual last target",
                "84": "initialized target flag",
            },
            "still_unrecovered": "full live intermediate scheduling/initialized continuation; do not replace with a guessed equal-time target queue",
            "facts_not_implementation_status": True,
        },
        "classification": "scripts/analysis and reports; no temporary artifacts/assets or new directories",
    }
    prefix = ROOT / "reports/recovery_A_20261117_emote_shared_bank"
    payload["modified_paths"] = sources + [
        str(prefix.with_suffix(ext).relative_to(ROOT)) for ext in (".json", ".md")
    ] + [
        str(prefix.with_name(prefix.name + "_summary.json").relative_to(ROOT)),
        "reports/recovery_A_20261117_emote_current_summary.json",
    ]
    prefix.with_suffix(".json").write_text(
        json.dumps(payload, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    small = {key: payload[key] for key in (
        "schema", "task_date", "summary", "APIs", "remaining_implementation_families", "batch_deltas", "classification")}
    prefix.with_name(prefix.name + "_summary.json").write_text(
        json.dumps(small, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    prefix.with_suffix(".md").write_text(
        "# A / primary shared bank所有权（2026-11-17）\n\n"
        "实际源码新增emote_m2values.rs，接入M2 live owner和CpuEmoteBackend。"
        "173f1f处理chain，173fde/17401e才推进blink/lip，174196最后推进driver；"
        "因此bank输出走16df30式absolute值表，下一次chain pass消费，不能直接覆盖渲染driver值，"
        "也不能和timed数值相加。\n\n"
        "16e770/16df30更新已有chain末端raw/scaled样本；initialized84且kind非fix时"
        "16d2d2仅查询当前driver值回填owner30，不重发target，不重新延长动画。"
        "无既有chain的dirty absolute写入生成真实current→target、kind0/duration0 pair。"
        "fix记录按原版每次重发target；其他timed记录保持首次提交状态。\n\n"
        "16d787的virtualf0绑定driver3510→11dc0：收集活动timeline变量名，"
        "排除这些变量的receiver忙状态；非loop timeline自身保持busy，loop自身不强制busy。"
        "所有实际receiver空闲或engine=0才清chain，绝不是每帧清owner。"
        "virtual94=2b40→133e0查询base map54；13530/20170单独添加timeline差分用于渲染，"
        "本批没有把差分反馈到bank绝对当前值。\n\n"
        "906 reselect、blink/lip更新、907/927/928重置及waveform停播恢复均走同一owner。"
        "已删除原20条selected-blink blanket guard以及timed timeline同名blanket guard，"
        "用实际值表/chain生命周期接替。构造种子未被计为live更新。\n\n"
        "继续批次已实现NoMove：17329a的enum6跳过1766b0，保留当前driver/value/chain，"
        "仍走1730ea/173150的实际bank选择；仅NoMove变化不设置changed90，"
        "因此不执行1733d8的整组恢复。184ae0/184bd0选择会清active/60/64，"
        "但保持phase6c及flow94..b0。重选保留实际waveform/phase，仅重置原timestamp，"
        "其他非NoMove变化才执行bank恢复与finish request。\n\n"
        "同批实现primary Fade：197f0a映射enum5，16d7e8第五分支指16d3ae，"
        "duration/easing沿用16d36b清零值，实际提交末值；kind5仍按非fix owner查询/释放，"
        "不能每帧重发fix。该primary chain行为与旧模型surface混合分别统计。\n\n"
        f"primary102显式pair现在{summary['pairs_fitting_implemented_primary_source']}/102"
        f"具备对应源码路径，明确pair阻断{summary['source_blocked_pairs_remaining']}，本批解除20。"
        "115个标签均有控制器族路径；60个timed标签等待用户指定的最终统一运行验证，"
        "待验证不是缺源码，也不等同于完整NEKO像素兼容。\n\n"
        f"当前另列{len(remaining_families)}个尚未实现的分支族，JSON逐条列出："
        "compound、intermediate、旧模型surface混合、secondary weights、material force、"
        "LPD preset、sidecar switching、nondefault quiet、graph非有限域、mesh hierarchy、"
        "stencil scheduler。该分支族数量不是全PE函数剩余数。\n\n"
        "宿主API保持import_m2_resource/play_timeline/advance；"
        "playback_info().m2_owner新增owner_value_records、owner_chain_records、deferred_bank_writes。"
        "不碰1055–1061/emocon_native、main render_scene、C工厂hooks。\n\n"
        "未编译、未测试、未执行Rust/游戏/DLL/SDK，未构建APK。"
        "只做原始PE静态分析和rustfmt语法读取。新增完整native完成数0。"
        "脚本归scripts/analysis，报告归reports，无临时产物需归档。\n",
        encoding="utf-8")
    current_path = ROOT / "reports/recovery_A_20261117_emote_current_summary.json"
    current = read_json(current_path)
    current["shared_bank_batch"] = small
    current["source_hashes_current"] = payload["sources"]
    current["summary"].update({
        "configured_primary_pair_family_subset": summary["pairs_fitting_implemented_primary_source"],
        "configured_primary_pair_blink_blocked": summary["source_blocked_pairs_remaining"],
        "first_model_timed_labels_pending_final_runtime_validation": 60,
        "first_model_timed_target_labels_remaining": 0,
        "first_model_timed_target_labels_remaining_scope": "missing controller-family source paths;60 pending final runtime validations are recorded separately",
        "pending_runtime_validation_is_not_missing_source": True,
        "remaining_implementation_family_count": len(remaining_families),
    })
    current["summary"]["backend_native_branches"]["906"] = (
        "primary timed graph/scalar/selector + shared bank + actual NoMove/Fade source; intermediate/compound/secondary/model-surface remain")
    current["summary"]["remaining_strict_blockers"] = [f["reason"] for f in remaining_families]
    for p in sources[:-1]:
        if p not in current["new_source_paths"] and p not in current["modified_source_paths"]:
            current["new_source_paths"].append(p)
    report_path = str(prefix.with_suffix(".md").relative_to(ROOT))
    if report_path not in current["reports"]:
        current["reports"].append(report_path)
    current_path.write_text(json.dumps(current, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(summary, ensure_ascii=True))


if __name__ == "__main__":
    main()

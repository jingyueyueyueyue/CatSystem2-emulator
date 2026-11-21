"""Fixed-PE M2 evaluator retention and targeted surface contracts; no execution."""
from datetime import datetime, timezone
import json

from recovery_A_20261116 import ROOT, Image, read_json, sha
from recovery_A_receiver_contracts_20261116 import block


def main():
    identity = read_json(ROOT / "reports/recovery_A_20261116_emote.json")
    exe = Image(ROOT / identity["binary_path"], identity["PE_sha256"])
    prior = read_json(ROOT / "reports/recovery_A_20261117_emote_presets_summary.json")
    remaining = prior["remaining_implementation_families"]
    reasons = {
        "LPD_preset": "exact/default declared presets implemented; missing default reads uninitialized profile bytes, including an empty table; explicit diagnostic remains",
        "sidecar_switch": "retained virtual receiver, parameters/origin and sidecar-to-waveform live ownership implemented; failed LPD reopen clears the original evaluator but currently returns a transactional diagnostic",
        "waveform_nondefault_quiet": "record78 consumer/clear and sticky quiet-tail implemented; external nonzero record78 producer identity remains unbound",
    }
    for family in remaining:
        if family["id"] in reasons:
            family["reason"] = reasons[family["id"]]
    paths = [
        "simulator/crates/runtime/src/emote_m2lip.rs",
        "simulator/crates/runtime/src/emote_m2presets.rs",
        "simulator/crates/runtime/src/emote_m2live.rs",
        "simulator/crates/runtime/src/emote_m2wave.rs",
        "simulator/crates/desktop/src/emote_native.rs",
        "scripts/analysis/recovery_A_emote_evaluator_20261117.py",
    ]
    spans = {
        "clipper_allocate40_only_map_initialized_default32_indeterminate": (0x186CF9, 0x45),
        "clipper_miss_returns_profile_start": (0x186089, 0x10),
        "LPD_construct_only_if_null_then_existing_virtual_open": (0x184EE0, 0x4B),
        "LV_construct_only_if_null_then_existing_virtual_open": (0x1754A1, 0xB2),
        "LPD_vtable": (0x5E4460, 28),
        "LV_vtable": (0x5E4480, 28),
        "LPD_open_clears_then_parses_and_clears_on_failure": (0x1A1A90, 0x67),
        "LV_open_retains_parameters_and_smooths_before_return": (0x1A1DF0, 0x104),
        "LPD_after_open_weights_origin_radius": (0x1753E1, 0x36),
        "weights_write_dirty": (0x1AB3B0, 0x32),
        "LPD_sampler_consumes_dirty": (0x1A1B00, 0x57),
        "LV_sampler_ignores_dirty_and_uses_retained_origin": (0x1A1F00, 0x26),
        "window_observe_has_no_sidecar_null_check": (0x174684, 0x93),
        "loud_sets_phase_clears78_but_not_quiet_b0": (0x17468E, 0x2A),
        "nonzero78_skips_quiet_scan": (0x1746B8, 0x10),
        "external_sampler_precedes_waveform_branch": (0x184150, 0xE),
        "retained_external_sampler_actual_sound_time": (0x18431C, 0x25),
        "bank20_commit_after_evaluator_update": (0x184410, 6),
        "finish_request_keeps_external_b4": (0x1747D8, 0x31),
        "nonplaying_resets_real_base_and_phase_including_b4": (0x174984, 0x47),
        "flow_reset_does_not_clear_b0": (0x184F95, 0x2A),
        "completed_interval_clears_b0": (0x1841BB, 0x6D),
        "manager_capture_before_resource_replace": (0x17D6B8, 0x47),
        "ordered_driver_render_to_shared_target": (0x19B4FF, 0x73),
        "surface_projection_setup": (0x19C050, 0x235),
        "surface_D3D_state_setup": (0x19C290, 0x263),
        "surface_image_copy_not_time_fade": (0x19CA42, 0x132),
    }
    summary = {
        "primary_pair_source_paths_inherited": 102,
        "primary_pair_explicit_source_blocked": 0,
        "remaining_implementation_family_count_before": 9,
        "remaining_implementation_family_count": len(remaining),
        "newly_closed_entire_families": [],
        "implemented_receiver_branches_this_batch": [
            "retain b4 class when new sound sidecar type changes",
            "LV reopens using its old radius; later LPD setters change metadata without second smoothing",
            "LV command keeps existing preset and origin; LPD command applies declared profile overrides",
            "sidecar-to-waveform uses actual new SOUND time/window with retained external evaluator",
            "nonplaying SOUND restores true base and clears phase for external as well as waveform records",
            "sample1 bank20 is committed after evaluation",
            "nonzero78 quiet-scan gate; loud clears78, quiet b0 stays latched until completion",
        ],
        "LPD_implicit_profile_default_is_uninitialized_not_constructor_default": True,
        "LPD_empty_table_is_also_an_indeterminate_miss": True,
        "failed_LPD_reopen_mutation_matches_original": False,
        "nonzero_record78_producer_bound": False,
        "previous_surface_fade_weight_recovered": False,
        "no_new_main_hooks_required": True,
        "completed_native_count_added": 0,
        "timed_labels_pending_final_combined_runtime_validation": 60,
        "pending_runtime_validation_is_not_missing_source": True,
        "compiled": False, "tests_run": False, "Rust_execution": False,
        "game_or_DLL_executed": False, "full_NEKO_strict_pixel_claim": False,
    }
    apis = {
        "host_entry_unchanged": "native924 -> EmoteServices::bind_m2_profile_lip_sidecar / update_m2_waveform / control_m2_waveform",
        "retained_parameters": "CpuM2LipCurve holds actual preset, origin and virtual class; m2presets::bind_sidecar_records reopens that class",
        "sound_switch": "EmoteNativeServices.sound_links keeps actual new SOUND channel/time; no release/replace of b4 is invented",
        "quiet_receiver": "CpuM2Waveform::quiet_scan_inhibited represents actual node78; observe/sample implement the gate and sticky b0",
        "strict_render": "existing render_rgba strict diagnostics retained; this batch adds no pixel-compatibility claim",
    }
    missing = [
        {"family": "sidecar_switch", "receiver": "m2presets::bind_sidecar_records", "PE_RVA": ["1a1a90", "1a1d20"],
         "missing": "original failed retained LPD open clears old arrays and leaves the class present; current error keeps the live owner unchanged rather than silently succeeding"},
        {"family": "LPD_preset", "receiver": "CpuM2ClipperProfile::select", "PE_RVA": ["186cf9", "185fe0"],
         "missing": "undeclared default is uninitialized allocation state, not a recoverable fixed constant; explicit None/diagnostic required"},
        {"family": "waveform_nondefault_quiet", "receiver": "CpuM2Waveform::quiet_scan_inhibited", "PE_RVA": ["1746be", "1746af"],
         "missing": "consumer implemented; identity of any external nonzero writer still required before automatically injecting such a flag"},
        {"family": "model_surface_fade", "receiver": "emote_cpu::import_m2_resource/render_rgba", "PE_RVA": ["17b820", "19b4b0", "19c9f0"],
         "missing": "ordered capture/projection/state/image copy confirmed; old/new time weight, retirement and multi-player composition still not identified"},
        {"family": "mesh_hierarchy", "receiver": "RenderContext::layer", "missing": "meshCombine/skin hierarchy and inherited subdivision"},
        {"family": "stencil_scheduler", "receiver": "literal_stencil_clip/stencil_source_motion", "missing": "joined/inherited/nested source ordering and non-type5/mode1"},
    ]
    payload = {"schema": "cs2-recovery-A-emote-evaluator/v1", "task_date": "2026-11-17",
        "generated_at_utc": datetime.now(timezone.utc).isoformat(), "summary": summary, "APIs": apis,
        "EXE": {"path": identity["binary_path"], "sha256": exe.digest, "address_kind": "RVA", "image_base": exe.base},
        "remaining_implementation_families": remaining, "specific_missing_receivers": missing,
        "EXE_evidence": {name: block(exe, *span) for name, span in spans.items()},
        "sources": [{"path": p, "sha256": sha((ROOT / p).read_bytes())} for p in paths],
        "classification": "scripts/analysis + reports; no temporary products, new directories, root README/main render_scene/C factory edits"}
    prefix = ROOT / "reports/recovery_A_20261117_emote_evaluator"
    payload["modified_paths"] = paths + ["reports/" + prefix.name + ext for ext in (".json", ".md", "_summary.json")] + [
        "reports/recovery_A_20261117_emote_current_summary.json"]
    prefix.with_suffix(".json").write_text(json.dumps(payload, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    small = {k: payload[k] for k in ("schema", "task_date", "summary", "APIs", "remaining_implementation_families", "specific_missing_receivers", "sources", "classification")}
    prefix.with_name(prefix.name + "_summary.json").write_text(json.dumps(small, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    prefix.with_suffix(".md").write_text(
        "# A / 实际 evaluator 复用与 SOUND 切换（2026-11-17）\n\n"
        "本批源码已落盘：emote_m2lip/presets/live/wave.rs、desktop/emote_native.rs。接口不变，主代理不需新增hook。"
        "primary沿用102/102源码路径、0明确pair阻断；整族仍9，未用子分支改进虚减分母。60 timed标签待最终统一验证，不算缺源码。\n\n"
        "184ee0/1754a1只在b4为空时构造；新SOUND宣称的LPD/LV类型不会替换已存在虚表。"
        "现按保留的class解析，LV reopen先用旧radius平滑；175400的后置preset setter只改LV参数/origin，"
        "1a1f00不消费dirty，因此不会重复平滑。LPD按真实声明preset重建曲线；LV命令保留既存preset/origin。\n\n"
        "sidecar→waveform现在保留b4，以实际新SOUND时间和window继续驱动；174610阈值分支不检查b4，"
        "184150仍优先外部sampler。停声174870对包括sidecar在内的active记录恢复实际base并清phase。"
        "bank20改到184410的evaluator pass后提交；quiet b0不再被每次loud或184f90错误清除，"
        "node78非零时跳过quiet scan，loud只清78，完成interval才清b0。非零78自动生产者尚未绑定。\n\n"
        "重要纠正：186cf9..186d38申请40字节，只初始化map+20/+24；profile default前32字节未初始化。"
        "185fe0 miss返回该区域，空clipper表同样如此。已移除原源码将空表误用evaluator默认值的分支，"
        "现在明确诊断；这不是一个可以从PE恢复固定默认常量的接收者。\n\n"
        "仍缺：失败LPD reopen在原版先清旧数组、保留class；当前实现返回明确错误并保留旧live owner，"
        "未伪装原版成功。surface19c050=投影、19c290=D3D状态、19c9f0=图像copy；"
        "19b4b0按顺序调用driver virtual104。尚未确定old/new时长权重和退休策略，resource replacement guard保留。"
        "其余secondary/material、meshCombine/skin和joined/inherited/nested stencil继续单列。\n\n"
        "JSON包含固定NEKO PE SHA256、RVA原字节片段、修改路径/哈希；不含EXE或游戏资源。"
        "仅rustfmt语法读取及Python只读PE/源码报告；不编译、不测试、不运行Rust/VM/游戏/DLL/APK。"
        "脚本归scripts/analysis，报告归reports，无临时产物或新增目录。\n", encoding="utf-8")
    current_path = ROOT / "reports/recovery_A_20261117_emote_current_summary.json"
    current = read_json(current_path)
    current["evaluator_batch"] = small
    current["summary"]["remaining_implementation_family_count"] = len(remaining)
    current["summary"]["remaining_strict_blockers"] = [f["reason"] for f in remaining]
    current["summary"]["backend_native_branches"]["924"] = "retained LV/LPD virtual receiver, actual preset/origin, live SOUND/window switch and base restore; uninitialized profile misses and failed-open mutation remain diagnostic"
    current["source_hashes_current"] = payload["sources"]
    report = "reports/" + prefix.name + ".md"
    if report not in current.setdefault("reports", []):
        current["reports"].append(report)
    current_path.write_text(json.dumps(current, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(summary, ensure_ascii=True))


if __name__ == "__main__":
    main()

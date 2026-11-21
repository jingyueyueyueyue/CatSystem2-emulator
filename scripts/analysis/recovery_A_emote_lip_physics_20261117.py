"""Bounded exact-PE LPD/physics contract; no original code or Rust execution."""
from __future__ import annotations
from datetime import datetime, timezone
import json
from recovery_A_20261116 import ROOT, Image, read_json, sha
from recovery_A_receiver_contracts_20261116 import block


def main():
    prior = read_json(ROOT / "reports/recovery_A_20261116_emote.json")
    exe = Image(ROOT / prior["binary_path"], prior["PE_sha256"])
    spans = {
        "LPD_signature_recordsize_count": (0x1AB266, 0x2E),
        "LPD_record_predecessor": (0x1AB2E7, 0x53),
        "LPD_driver_defaults": (0x1A19E0, 0x6A),
        "LPD_level_and_phoneme_curve": (0x1A1C37, 0x5E),
        "LPD_symmetric_window_average": (0x1A1CCD, 0x4C),
        "LPD_actual_time_sample": (0x1A1B18, 0x3F),
        "external_lip_to_driver_variable": (0x18431C, 0x82),
        "physics_force_step": (0x194787, 0xA8),
        "physics_response_scale": (0x1948E0, 0x25),
    }
    sources = [
        "simulator/crates/runtime/src/emote_m2lip.rs",
        "simulator/crates/runtime/src/emote_m2physics.rs",
        "simulator/crates/runtime/src/emote_m2selector.rs",
        "simulator/crates/runtime/src/emote_m2live.rs",
        "simulator/crates/runtime/src/emote.rs",
        "simulator/crates/runtime/src/emote_cpu.rs",
        "scripts/analysis/recovery_A_emote_lip_physics_20261117.py",
    ]
    summary = {
        "LPD_parser_and_curve_source_implemented": True,
        "LPD_signature": "LPD\\0",
        "LPD_header_bytes": 20,
        "LPD_record_bytes": 32,
        "LPD_default_phoneme_weights": [0, 1, 0.4, 0.2, 0.8, 0.6],
        "LPD_default_smoothing_radius": 1,
        "LPD_bound_owner_tick_writes_real_sampler_variables": True,
        "LPD_all_target_atomic_batch_backend": True,
        "LPD_shared_Arc_curve_storage": True,
        "native924_sound_task_sidecar_route_complete": False,
        "native924_actual_legacy_LV_branch_connected": True,
        "LPD_actual_workspace_resource_execution": False,
        "material_integrator_source_implemented": True,
        "material_full_force_schedule_integrated": False,
        "immediate_selector_option_values_feed_actual_sampler": True,
        "timed_selector_and_instant_timeline_targets_complete": False,
        "compiled": False, "tests_run": False, "Rust_execution": False,
        "original_game_or_DLL_executed": False,
        "full_NEKO_strict_frame_claim": False,
        "complete_native_count_added": 0,
    }
    payload = {
        "schema": "cs2-recovery-A-emote-lip-physics/v1",
        "task_date": "2026-11-17",
        "generated_at_utc": datetime.now(timezone.utc).isoformat(),
        "summary": summary,
        "APIs": {
            "LPD": "CpuM2Lpd::parse(bytes)",
            "actual_curve": "CpuM2LipCurve::default_from_lpd(&lpd,actual_origin_tick,actual_begin,actual_end)",
            "custom_curve": "CpuM2Lpd::curve(origin,begin,end,actual_phoneme_weights,actual_radius)",
            "sampler_link": "EmoteServices::bind_m2_lip_curve(task,actual_dimension_zero_based,actual_pattern_cp932,curve)",
            "batch_source": "EmoteServices::bind_m2_lpd(task,actual_bytes,actual_origin,&[CpuM2LipBinding])",
            "time_update": "advance_with_random -> live_owner.advance_lip -> same dictionary consumed by CPU parameterize",
            "material": "CpuM2Material::from_record/inject_variable_delta/step/variable_offset",
            "selector": "evaluate_emote_immediate_selectors(root,&mut effective_variables); only zero-duration M2 assignments",
        },
        "format_details": [
            "1ab250 validates DWORD0=0x0044504c,record-size32 at8,nonzero count at12;threshold f32 at16;records follow20.",
            "Record word0 is integer time,word1 selects phoneme weight,word7 is f32 dB;unrecovered phoneme indices are explicit errors.",
            "Curve count=last record frame+1;1ab2d0 searches record1 onward using predecessor0 and returns null after last boundary.",
            "Clamp -record.dB between20 and -header.threshold;ratio=(upper-20-(strength-20))/(upper-20);curve=(ratio*.4+.6)*phoneme_weight.",
            "1a1b60 smooths interior samples by the original sequential f32 sum over2*radius+1;endpoints remain unchanged.",
            "Sample is begin+(end-begin)*curve[now-origin],or begin outside range;caller must supply actual sidecar/task time mapping.",
            "Material194730 clamps displacement,then velocity+=((-spring*position-resistance*velocity)/mass)*microstep,position+=microstep*velocity,normalizes by maximum.",
            "Remaining graph/host force injection,material_min/max/inertia toggles and complete strict renderer are not inferred from these operators.",
        ],
        "sources": [{"path": path, "sha256": sha((ROOT/path).read_bytes())} for path in sources],
        "snippets": {name: block(exe, *span) for name, span in spans.items()},
        "classification": "scripts/analysis + reports; no temporary asset extraction or publishing",
    }
    out = ROOT / "reports/recovery_A_20261117_emote_lip_physics"
    out.with_suffix(".json").write_text(json.dumps(payload, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    small = {k: v for k, v in payload.items() if k not in {"sources", "snippets"}}
    out.with_name(out.name + "_summary.json").write_text(json.dumps(small, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    out.with_suffix(".md").write_text(
        "# A / M2 LPD 与 material 算子批次（2026-11-17）\n\n"
        "新增emote_m2lip.rs：独立LPD格式解析、真实phoneme/dB曲线、对称窗口平滑和任务时间采样。"
        "EmoteServices::bind_m2_lip_curve把实际曲线绑定到真实dimension/pattern的运行中lip owner，"
        "advance会写入真实sampler变量；没有绑定sidecar就不会制造静态成功。\n\n"
        "LPD头20字节、记录32字节、count在12、threshold在16；记录time在0、phoneme在4、dB在28。"
        "默认phoneme权重0/1/.4/.2/.8/.6，平滑半径1。曲线及精确f32运算顺序见JSON和原字节片段。"
        "调用方必须提供真实音频origin和begin/end，不能把宿主毫秒直接当模型帧。"
        "后续chain_lpd批已接实际LV的自动924分支、独立声音tick和直接f32采样；"
        "自动LPD profile override及waveform路径仍缺，不能记924全部完成。\n\n"
        "emote_m2physics.rs已按194730/1948c0实现真实material clamp、弹簧阻尼积分和缩放输出，"
        "并由配置导入真实记录。完整force调度、动态min/max、宿主运动注入仍缺，"
        "strict诊断保留；依赖未知force状态的907 reset也不再返回成功。\n\n"
        "继续批次新增emote_m2selector.rs：依据driverd260→d060，真实arm_type整数选择"
        "会把对应option的onValue及其他option的offValue写入sampler变量。"
        "只覆盖零duration M2赋值；timed selector和63个instant目标仍保持未恢复，不以即时赋值冒充过渡。\n\n"
        "未编译、未测试、未运行Rust或原游戏/DLL；未输出游戏资产。"
        "本批脚本已归scripts/analysis、报告归reports，无临时产物。\n",
        encoding="utf-8",
    )
    current_path = ROOT / "reports/recovery_A_20261117_emote_current_summary.json"
    current = read_json(current_path)
    current["lip_physics_batch"] = small
    current["source_hashes_current"] = payload["sources"]
    current_path.write_text(json.dumps(current, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(summary, ensure_ascii=True))


if __name__ == "__main__":
    main()

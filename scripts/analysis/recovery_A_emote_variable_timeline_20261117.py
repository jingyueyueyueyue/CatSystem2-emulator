"""Bounded original timelineControl evidence and small actual-model inventory."""
from __future__ import annotations
from collections import Counter
from datetime import datetime, timezone
import json
from recovery_A_20261116 import ROOT, Image, read_json, sha
from recovery_A_receiver_contracts_20261116 import block
from recovery_A_psb_20261117 import Psb, load_sample


def main():
    driver = Image(next(ROOT.glob("*NEKO*/nekopara_vol3/emotedriver.dll")),
                   "85eb459e94e74ab608f649eb7fb440f2526e90aaa1af7276ed01dc10018903ce")
    name, data = load_sample(0)
    psb = Psb(data)
    metadata = psb.root["metadata"]
    instant = set(metadata.get("instantVariableList", []))
    timelines = metadata["timelineControl"]
    selectors = {v["label"] for v in metadata.get("selectorControl", []) if v.get("enabled", 1)}
    mouths = {v["label"] for v in metadata.get("mouthControl", []) if v.get("enabled", 1)}
    direct_controls = selectors | mouths | {
        v["label"] for section in ("eyeControl", "eyebrowControl")
        for v in metadata.get(section, [])
        if v.get("enabled", 1) and (section != "eyeControl" or not v.get("blinkEnabled", 0))
    }
    records = []
    for timeline in timelines:
        tracks = [v for v in timeline["variableList"] if v["frameList"]]
        nonlinear = sorted({float(f["content"]["easing"]) for v in tracks
                            for f in v["frameList"] if f.get("content") and f["content"]["easing"] != 0})
        instant_targets = [v["label"] for v in tracks if v["label"] in instant]
        immediate_selector_tracks = [
            v for v in tracks if v["label"] in instant and v["label"] in selectors
            and all(
                i + 1 < len(v["frameList"])
                and v["frameList"][i + 1]["time"] - frame["time"] - 1 <= 0
                for i, frame in enumerate(v["frameList"]) if frame.get("content")
            )
        ]
        fits_immediate = set(instant_targets) <= {v["label"] for v in immediate_selector_tracks}
        actual_immediate_tracks = [
            v for v in tracks if v["label"] in instant and v["label"] in direct_controls
            and (v["label"] in mouths or all(
                i + 1 < len(v["frameList"])
                and v["frameList"][i + 1]["time"] - frame["time"] - 1 <= 0
                for i, frame in enumerate(v["frameList"]) if frame.get("content")
            ))
        ]
        fits_controls = set(instant_targets) <= {v["label"] for v in actual_immediate_tracks}
        records.append({
            "label": timeline["label"], "diff": timeline["diff"],
            "loopBegin": timeline["loopBegin"], "loopEnd": timeline["loopEnd"],
            "lastTime": timeline["lastTime"], "tracks": len(tracks),
            "frames": sum(len(v["frameList"]) for v in tracks),
            "nonlinear_easing": nonlinear, "instant_targets": instant_targets,
            "fits_current_additive_subset": not instant_targets,
            "fits_linear_only_subset": not nonlinear and not instant_targets,
            "fits_additive_plus_immediate_selector_subset": fits_immediate,
            "zero_duration_instant_selector_targets": [v["label"] for v in immediate_selector_tracks],
            "fits_recovered_immediate_controls_subset": fits_controls,
            "recovered_immediate_control_targets": [v["label"] for v in actual_immediate_tracks],
        })
    spans = {
        "start_stop_others_unless_flags1": (0x1D78B, 0x53),
        "start_zero_seek": (0x1D83B, 0x24),
        "target_time_minus1": (0x1F401, 0x29),
        "flags2_difference_flags4_skip_instant": (0x1F601, 0x6A),
        "instant_selector_integer_and_options": (0xD29B, 0x38),
        "selector_actual_option_on_off": (0xD060, 0x120),
        "eye_zero_duration_actual_write": (0x6788, 0x4D),
        "eyebrow_zero_duration_actual_write": (0x91D8, 0x4A),
        "mouth_selector_always_integer_ignores_duration": (0x132E2, 0x28),
        "update_boundary_comparison": (0x1F685, 0x7B),
        "loop_wrap_remaining_delta": (0x1FB0A, 0x86),
        "active_additive_accumulation": (0x2035D, 0x16),
        "linear_tween_duration_and_epsilon": (0xDC01, 0x4D),
        "linear_tween_interpolation": (0xDD10, 0x2A),
        "same_endpoint_remaining_duration": (0xDA94, 0x78),
        "metadata_parse_lastTime_maximum": (0x1EF5E, 0x13),
        "signed_easing_to_power": (0x131D5, 0x4D),
        "double_pow_to_f32_interpolation": (0xDCF2, 0x48),
        "SSE_math_fallback": (0x7055B, 0x1C),
        "pow_x87_log2": (0x7110D, 0x1B),
        "pow_x87_exp2": (0x6D090, 0x15),
    }
    paths = [
        "simulator/crates/runtime/src/emote.rs",
        "simulator/crates/runtime/src/emote_cpu.rs",
        "simulator/crates/runtime/src/emote_variable_timeline.rs",
        "simulator/crates/runtime/src/emote_m2selector.rs",
        "scripts/analysis/recovery_A_emote_variable_timeline_20261117.py",
    ]
    payload = {
        "schema": "cs2-recovery-A-emote-variable-timeline/v1",
        "task_date": "2026-11-17",
        "generated_at_utc": datetime.now(timezone.utc).isoformat(),
        "summary": {
            "actual_model_timeline_labels": len(records),
            "actual_model_frame_counts": dict(Counter(str(f["type"]) for t in timelines
                for v in t["variableList"] for f in v["frameList"])),
            "labels_fitting_additive_source_subset": sum(r["fits_current_additive_subset"] for r in records),
            "labels_fitting_linear_only_subset": sum(r["fits_linear_only_subset"] for r in records),
            "labels_fitting_additive_plus_immediate_subset": sum(r["fits_additive_plus_immediate_selector_subset"] for r in records),
            "labels_fitting_all_recovered_immediate_subset": sum(r["fits_recovered_immediate_controls_subset"] for r in records),
            "instant_target_labels_remaining": sum(not r["fits_recovered_immediate_controls_subset"] for r in records),
            "zero_duration_selector_labels_added": sum(r["fits_additive_plus_immediate_selector_subset"] and not r["fits_current_additive_subset"] for r in records),
            "walking_loop_source_supported": True,
            "troubled_and_initialize_labels_source_supported": True,
            "timed_eye_eyebrow_and_timed_selector_complete": False,
            "selected_waiting_loop_supported_in_source": next(r["fits_current_additive_subset"] for r in records
                if r["label"] == "\u5dee\u5206\u7528_waiting_loop"),
            "selected_waiting_loop2_supported_in_source": True,
            "selected_waiting_loop3_supported_in_source": next(r["fits_current_additive_subset"] for r in records
                if r["label"] == "\u5dee\u5206\u7528_waiting_loop3"),
            "variables_consumed_by_real_CPU_sampler": True,
            "nonlinear_math_helper_identified": True,
            "nonlinear_helper": "6fa30->70566->710c4,math-error-label80f60='pow',7111a+6d090",
            "pow_cross_platform_bit_equivalence_verified": False,
            "compiled": False, "tests_run": False, "original_code_executed": False,
            "Rust_pixel_execution": False, "full_NEKO_strict_frame_claim": False,
            "integrated_completed_native_count": 0,
        },
        "resource": {"name": name, "PSB_sha256": sha(data), "values": psb.nodes},
        "driver": {"path": str(driver.path.relative_to(ROOT)), "sha256": driver.digest},
        "APIs": {
            "play": "EmoteServices::play_timeline(task,label,2); flags&1 preserves other labels,flags&4 skips instant targets",
            "query": "playback_info(task).active_timeline_labels; CpuPlayer::is_timeline_playing(label)",
            "stop": "EmoteServices::stop_timeline(task,label); empty label stops all",
            "advance": "EmoteServices::advance -> CpuPlayer::advance_frames -> CpuVariableTimelines::advance",
            "actual_geometry_input": "CpuPlayer::effective_variables -> RenderContext.variables -> layer.parameterize",
            "instant_selector": "zero-duration targets drain into the real owner dictionary, then selectorControl on/off options; no additive fake value",
            "selector_difference_order": "evaluate selector option base first,then add independent timeline differences; retain actual fade option differences",
            "direct_controls": "zero-duration eye/eyebrow preserve f32; mouth primary label uses CVTTSS2SI and ignores duration; timed graphs remain errors",
        },
        "records": records,
        "snippets": {key: block(driver, *span) for key, span in spans.items()},
        "sources": [{"path": p, "sha256": sha((ROOT / p).read_bytes())} for p in paths],
        "remaining": [
            "eye/eyebrow/mouth/timed-selector instant target receivers",
            "enabled metadata controllers,host PFL controller banks",
            "full stencil/mesh schedule and strict full-model pixels",
        ],
    }
    out = ROOT / "reports/recovery_A_20261117_emote_variable_timeline"
    out.with_suffix(".json").write_text(json.dumps(payload, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    short = {k: v for k, v in payload.items() if k not in {"records", "snippets", "sources"}}
    out.with_name(out.name + "_summary.json").write_text(json.dumps(short, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    out.with_suffix(".md").write_text(
        "# A / metadata.timelineControl 批次（2026-11-17）\n\n"
        f"首个真实模型有 {len(records)} 个时间线标签、4905 个 type2 值帧、1551 个 type0 停用帧。"
        f"{payload['summary']['labels_fitting_additive_source_subset']} 个标签满足本批加法子集的静态条件；不是运行验收数量。\n\n"
        f"继续批次新增零duration的instant selector真实写入；"
        f"当前加法+即时selector子集为{payload['summary']['labels_fitting_additive_plus_immediate_subset']}/115，"
        f"再加本批有据零duration eye/eyebrow与整数mouth后为{payload['summary']['labels_fitting_all_recovered_immediate_subset']}/115，"
        f"剩{payload['summary']['instant_target_labels_remaining']}个含未恢复控制器的标签。"
        "新增可导入「走行ループ」的arm_type单次选择：owner写值后实际selector option会改变fade_a..e，"
        "停止timeline保留该即时选择；没有把instant值加进difference sum。"
        "因此额外可导入「困り」「初始化」。非零duration selector仍报缺option tween queue，"
        "timed eye/eyebrow图和完整控制器自动行为仍未恢复；mouth talk浮点变化与整数主选择保持分别处理。\n\n"
        "`EmoteServices::play_timeline(task,label,2)` 从真实 PSB 取轨道，"
        "`advance` 以原始 f32 时间推进，`effective_variables` 把结果送入 CPU 图层 parameterize，"
        "`stop_timeline` 停止差分而保留基值。PFL waiting_loop、waiting_loop2、waiting_loop3 均可导入这一子集。\n\n"
        "driver1d720 flags&1 控制是否先停止其他标签，flags&2 使用加法轨道、flags&4 跳过 instant 目标。"
        "1f1c0/1f5c0 目标时间是 next_time-now-1，1f950 处理循环后余量，"
        "20170 将独立轨道值加到基值。停止加法轨道不能把末帧差分反复累加进基值。\n\n"
        "easing=0→指数1，正值e→1+e，负值e→1/(1-e)。"
        "6fa30 的70566回退到710c4；数学错误标识80f60是 pow，"
        "7111a 的 fyl2x 与6d090 的 f2xm1/fscale 确认幂函数。"
        "实现先扩展 f32 到 f64 求幂，结果收窄再插值；尚未验证跨平台逐位等价。\n\n"
        "新增模块 emote_variable_timeline.rs，连接 emote.rs 与 emote_cpu.rs。"
        "启用的未实现 metadata 控制器会产生 strict 警告，防止静态姿势冒充完整 native 像素。\n\n"
        "未编译、未测试、未运行 Rust/游戏；原函数只静态读取。完整 NEKO strict 帧及新增已接通 native 计数为0。"
        "脚本归 scripts/analysis，报告归 reports；未输出临时资源。\n",
        encoding="utf-8",
    )
    current_path = ROOT / "reports/recovery_A_20261117_emote_current_summary.json"
    current = read_json(current_path)
    current["variable_timeline_batch"] = short
    current["source_hashes_current"] = payload["sources"]
    current_path.write_text(json.dumps(current, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(payload["summary"], ensure_ascii=True))


if __name__ == "__main__":
    main()

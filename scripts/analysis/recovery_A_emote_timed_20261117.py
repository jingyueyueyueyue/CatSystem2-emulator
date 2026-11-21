"""SHA-bound M2 timed scalar/same-edge baseline, static only.

Reads one actual PSB, actual primary difference config and bounded original
instruction spans. No candidate sweep, binary execution, Rust build or test.
"""
from __future__ import annotations

from collections import Counter
from datetime import datetime, timezone
import json

from recovery_A_20261116 import ROOT, Image, read_json, sha
from recovery_A_receiver_contracts_20261116 import block
from recovery_A_emote_host_20261117 import sections
from recovery_A_psb_20261117 import load_sample, Psb
from audit_runtime_evidence import kif_entries, blowfish_le_decrypt


def main():
    prior = read_json(ROOT / "reports/recovery_A_20261116_emote.json")
    exe = Image(ROOT/prior["binary_path"], prior["PE_sha256"])
    driver = Image(exe.path.parent/"emotedriver.dll",
                   "85eb459e94e74ab608f649eb7fb440f2526e90aaa1af7276ed01dc10018903ce")
    resource_name, psb_bytes = load_sample(0)
    psb = Psb(psb_bytes)
    metadata = psb.root["metadata"]
    controls = {
        entry["label"]: (section, entry)
        for section in ("eyeControl", "eyebrowControl", "mouthControl", "selectorControl")
        for entry in metadata.get(section, []) if entry.get("enabled", 1)
    }
    archive = exe.path.parent/"m2config.int"
    entries, _, cipher = kif_entries(archive, 0x0F6BB0AE)
    entry = next(e for e in entries if e["name"] == b"ca01.inc")
    with archive.open("rb") as handle:
        handle.seek(entry["offset"])
        raw = handle.read(entry["size"])
    raw = blowfish_le_decrypt(cipher, raw) if cipher else raw
    rows = sections(raw)
    patterns, owner = {}, None
    for row in rows["pattern"]:
        if row[0].startswith("%"):
            owner = (int(row[0][1:])-1, row[1])
            patterns.setdefault(owner, {})
        else:
            patterns[owner][row[0]] = float(row[1])
    active_blinks = {(int(row[0])-1, row[1]) for row in rows["blink"]}
    aliases = {}
    alias = None
    for row in rows["chain"]:
        if row[0].startswith("$"):
            alias = row[0]
        elif alias and len(row) >= 4:
            aliases.setdefault(alias, []).append(row)
    dimensions = {"$eye": 2, "$blow": 4}

    def edge_for(control, value):
        return next((index for index, edge in enumerate(control["edge"])
                     if edge[0] <= value <= edge[1]), None)

    pair_records = []
    for alias, pairs in aliases.items():
        dimension = dimensions[alias]
        for row in pairs:
            before, after = patterns[(dimension, row[0])], patterns[(dimension, row[1])]
            reasons = []
            if (dimension, row[1]) in active_blinks:
                reasons.append("selected blink bank shares the timed variable; merge remains unrecovered")
            for name, target in after.items():
                section, control = controls.get(name, (None, None))
                if section in ("eyeControl", "eyebrowControl"):
                    a, b = edge_for(control, before.get(name, 0)), edge_for(control, target)
                    if a is None or b != a:
                        reasons.append("cross-edge graph route:" + name)
                elif section == "selectorControl":
                    reasons.append("timed selector option queues:" + name)
            pair_records.append({
                "dimension_zero_based": dimension, "from": row[0], "to": row[1],
                "kind": row[2], "frames": int(row[3]), "pattern_values_from": before,
                "pattern_values_to": after, "static_endpoint_subset": not reasons,
                "blocked_reasons": reasons,
                "not_live_call_or_current_state": True,
            })
    timeline = read_json(ROOT/"reports/recovery_A_20261117_emote_variable_timeline.json")
    remaining = [
        row["label"] for row in timeline["records"]
        if not row["fits_recovered_immediate_controls_subset"]
    ]
    summary = {
        "snapshot_scope": "historical scalar/same-edge endpoint baseline; current graph/selector progress is in graph_selector_summary",
        "current_progress_summary": "reports/recovery_A_20261117_emote_graph_selector_summary.json",
        "same_profile_timed_chain_scalar_and_same_edge_source": True,
        "chain_driver_easing": {"linear": 1.0, "sin_fs": -1.2, "sin_sf": 1.2, "sin_ss": 1.0},
        "chain_easing_not_inferred_from_names": True,
        "timed_from_actual_current_driver_value": True,
        "timed_state_advances_original_f32_task_delta": True,
        "timed_timeline_eye_eyebrow_targets_submit_actual_driver": True,
        "cross_edge_missing_route_blanket_diagnostic_superseded": True,
        "nonfinite_inter_segment_math_remains_diagnostic": True,
        "timed_equal_endpoint_longer_duration_retains_existing_target": True,
        "play_and_advance_controller_transactional_in_source": True,
        "native907_engine_zero_finishes_pending_timed_source": True,
        "live_bank_and_force_history_retained_on_reselection": True,
        "strict_blink_timeline_collision_diagnostics": True,
        "configured_primary_exact_pair_count": len(pair_records),
        "configured_static_endpoint_pairs_in_subset": sum(row["static_endpoint_subset"] for row in pair_records),
        "configured_static_endpoint_pairs_blocked": sum(not row["static_endpoint_subset"] for row in pair_records),
        "pair_counts_are_not_live_or_native_completion": True,
        "first_model_timeline_labels_source_subset": 55,
        "first_model_timed_target_labels_remaining": len(remaining),
        "timed_cross_edge_graph_complete": False,
        "timed_chain_complete_all_branches": False,
        "full_NEKO_strict_frame_claim": False,
        "complete_native_count_added": 0,
        "compiled": False, "tests_run": False, "Rust_execution": False,
        "original_game_or_DLL_executed": False,
    }
    APIs = {
        "unchanged_load": "EmoteServices::import_m2_resource -> CpuM2LiveOwner::reselect -> pending_timed",
        "driver": "CpuM2TimedVariables::validate/submit/advance/finish_all",
        "timed_timeline": "CpuEmoteControllerTarget{name,value,duration,easing}; actual next_time-now-1 -> submit_targets",
        "packet": "CpuM2TimedRequest{name,target,frames,kind}; real config values only",
        "inspection": "playback_info().active_m2_timed_variables and m2_owner.pending_timed_targets",
        "907": "actual engine flag0 finishes pending/active timed targets on next original update",
        "time": "EmoteTick.task_elapsed -> CpuOriginalTaskClock delta; lip_elapsed remains independent sound clock",
    }
    remaining_branches = [
        "same-name variables shared by changed dimensions require original compound blend ownership",
        "active timeline difference and timed pattern same-variable ownership",
        "selected blink bank overriding the same timed variable",
        "nonfinite inter-segment graph pow domain; ordered graph implementation is in later graph-selector batch",
        "append selector queues, Fade/NoMove object effects and intermediate pattern segments",
        "secondary smooth/group transitions and old-model surface fade",
        "native907 actual complete material/global force owner",
        "LPD automatic preset; concrete PCM waveform/flow exists in later waveform batch",
        "generic mesh/stencil and full-model strict pixels",
    ]
    exe_spans = {
        "timed_linear_duration_and_raw_easing1": (0x16D411, 0x5A),
        "timed_sin_fs_duration_and_raw_easing_minus1p2": (0x16D46B, 0x5A),
        "timed_sin_sf_duration_and_raw_easing_plus1p2": (0x16D4C5, 0x5A),
        "M2_driver_vtable90_target_call": (0x16D5A4, 0x36),
        "bank_reset_after_selection": (0x1733D8, 0x7E),
        "engine_enable_zero_immediate_endpoint": (0x16D36B, 0x56),
    }
    driver_spans = {
        "bridge_virtual90_arguments": (0x2B10, 0x27),
        "target_dispatch": (0x131D5, 0x4D),
        "generic_scalar_tween_route": (0x13331, 0x2F),
        "eye_queue_or_zero": (0x6788, 0x4D),
        "eye_graph_prepare": (0x6B0C, 0x4E),
        "graph_current_and_target_same_edge": (0x9F9D, 0x56),
        "graph_first_inclusive_edge_lookup": (0x9A20, 0x3A),
        "graph_direct_pair_distance": (0xA5B6, 0x41),
        "eye_power_distance_advance": (0x6910, 0x8A),
        "eye_positive_negative_epsilon": (0x6996, 0x4A),
        "eyebrow_power_distance_advance": (0x934E, 0x77),
        "generic_power_scalar_advance": (0xDCF2, 0x48),
        "eye_same_endpoint_retention": (0x67DF, 0x6B),
        "eyebrow_same_endpoint_retention": (0x922C, 0x65),
        "timeline_actual_instant_target_duration": (0x1F708, 0xAE),
        "mouth_label_integer_no_tween": (0x132E2, 0x28),
    }
    paths = [
        "simulator/crates/runtime/src/emote.rs",
        "simulator/crates/runtime/src/emote_cpu.rs",
        "simulator/crates/runtime/src/emote_m2live.rs",
        "simulator/crates/runtime/src/emote_m2timed.rs",
        "simulator/crates/runtime/src/emote_variable_timeline.rs",
        "scripts/analysis/recovery_A_emote_timed_20261117.py",
    ]
    payload = {
        "schema": "cs2-recovery-A-emote-timed/v1", "task_date": "2026-11-17",
        "generated_at_utc": datetime.now(timezone.utc).isoformat(),
        "binary": {"path": prior["binary_path"], "sha256": exe.digest},
        "driver_sha256": driver.digest,
        "resource": {"name": resource_name, "PSB_sha256": sha(psb_bytes),
                     "config": "m2config.int::ca01.inc", "config_sha256": sha(raw)},
        "summary": summary, "APIs": APIs,
        "remaining_branches": remaining_branches, "remaining_labels": remaining,
        "configured_static_endpoint_pairs": pair_records,
        "EXE_evidence": {key: block(exe, *span) for key, span in exe_spans.items()},
        "driver_evidence": {key: block(driver, *span) for key, span in driver_spans.items()},
        "sources": [{"path": path, "sha256": sha((ROOT/path).read_bytes())} for path in paths],
        "classification": "scripts/analysis and reports; no temporary artifacts/assets; no render_scene/main441 changes",
    }
    out = ROOT/"reports/recovery_A_20261117_emote_timed"
    out.with_suffix(".json").write_text(json.dumps(payload, ensure_ascii=False, indent=2)+"\n", encoding="utf-8")
    small = {key: payload[key] for key in (
        "schema", "task_date", "summary", "APIs", "remaining_branches", "remaining_labels", "classification"
    )}
    out.with_name(out.name+"_summary.json").write_text(json.dumps(small, ensure_ascii=False, indent=2)+"\n", encoding="utf-8")
    out.with_suffix(".md").write_text(
        "# A / M2 timed chain首批基线（2026-11-17）\n\n"
        "本报告的22/102、55/115及跨edge描述保留首批基线口径；"
        "当前已继续实现ordered graph、selector及SinSs，请使用"
        "recovery_A_20261117_emote_graph_selector_summary.json的当前统计。"
        "它不把控制器族条件通过当作播放/像素验收。\n\n"
        "新增emote_m2timed.rs，接906同profile实际chain请求，保留当前controller值作为起点，"
        "下一次原任务更新提交目标，再按原f32时钟差推进；907 engine=0在更新时取真实endpoint。"
        "普通scalar与同edge eye/eyebrow已具体实现，不是只存pending状态。\n\n"
        "easing参数来自EXE16d411/46b/4c5：linear传1、sin_fs传-1.2、sin_sf传1.2；"
        "driver2b10→13090→13160再转成signed easing power，不能凭名称猜sin函数或统一线性。"
        "eye/eyebrow按9710/9da0选择第一个包含端点的edge；同edge经a5b6直接记录pair和绝对路径长度。"
        "68d0/9310用pow反推phase、增加实际delta再映射到距离，保留f32及±0.0001终点比较。"
        "跨edge与自动blink仍保留明诊断。\n\n"
        "交付后继续完成的第二批：timeline中的timed eye/eyebrow目标已直接进入相同driver状态，"
        "不再以duration>0一概拒绝；真正跨edge仍在实际current值可得时明确报错。"
        "play/advance克隆状态验证后提交。da94、67df、922c的相同endpoint规则也已实现："
        "新duration比原剩余更长时保留原目标，不重新延长。"
        "外部即时写入会取消同名timed状态，差分加法仍在controller输出之后独立合成。\n\n"
        f"primary的102条显式pair中，{summary['configured_static_endpoint_pairs_in_subset']}条满足静态端点子集，"
        f"{summary['configured_static_endpoint_pairs_blocked']}条被跨edge/选中blink冲突等阻断。"
        "该数字只检查配置端点，实际current、timeline、材料历史仍要在load/update处验证，"
        "不等同于已执行调用、native完成或像素验收。"
        f"首模型timeline仍55/115，剩{len(remaining)}个标签；小摘要列出全部剩余标签与分支。\n\n"
        "接口：CpuM2TimedVariables::validate/submit/advance/finish_all；"
        "playback_info().active_m2_timed_variables及owner.pending_timed_targets可检查实际状态。"
        "同名多维、timeline差分同名、选中blink同名、intermediate、SinSs/Fade/NoMove、"
        "secondary smooth、旧surface fade、完整force/mesh/stencil继续待恢复。\n\n"
        "未编译、未测试、未运行Rust/原游戏/DLL；仅静态原字节、实际资源解析及rustfmt语法读取。"
        "新增完整native计数0。脚本归scripts/analysis，报告归reports；无临时文件或资产输出。\n",
        encoding="utf-8",
    )
    path = ROOT/"reports/recovery_A_20261117_emote_current_summary.json"
    current = read_json(path)
    current["timed_chain_batch"] = small
    current["summary"]["backend_native_branches"]["906"] = (
        "load/reuse/fix plus actual scalar/same-edge timed source; graph/bank/compound/fade incomplete"
    )
    current["source_hashes_current"] = payload["sources"]
    path.write_text(json.dumps(current, ensure_ascii=False, indent=2)+"\n", encoding="utf-8")
    print(json.dumps(summary, ensure_ascii=True))


if __name__ == "__main__":
    main()

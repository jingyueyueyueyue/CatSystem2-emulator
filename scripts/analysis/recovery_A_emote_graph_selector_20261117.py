"""Bounded PE evidence and actual-model timed controller family inventory.

Reads a fixed driver and one actual PSB. The route calculations describe
configuration endpoints only: they are neither Rust tests nor live playback.
No game/driver/SDK execution, candidate sweep, build or asset output.
"""
from __future__ import annotations

from collections import Counter
from datetime import datetime, timezone
import json
import struct

from recovery_A_20261116 import ROOT, Image, read_json, sha
from recovery_A_receiver_contracts_20261116 import block
from recovery_A_psb_20261117 import Psb, load_sample


def f32(value):
    return struct.unpack("<f", struct.pack("<f", value))[0]


def route_inventory(control, source, target):
    """Original ordered search, used only for static endpoint descriptions."""
    edges = control["edge"]
    nodes = control["node"]
    edge = lambda v: next((i for i, (a, b) in enumerate(edges) if a <= v <= b), None)
    node = lambda v: next((i for i, values in enumerate(nodes) if v in values), None)
    visited, candidates = [], []
    steps = 0

    def unused(index):
        value = next((v for v in nodes[index] if v not in visited), None)
        return None if value == -1 else value

    for _ in range(10):
        if edge(source) is None or edge(target) is None:
            break
        current, forward, state = source, True, 1
        segments, previous, cost = [], [], 0.0
        succeeded = False
        while True:
            steps += 1
            if steps > 100000 or edge(current) is None:
                return {"diagnostic": "capacity or invalid node endpoint"}
            low, high = edges[edge(current)]
            if state == 1:
                if edge(current) == edge(target):
                    visited = [v for v in visited if v not in (low, high)]
                    segments.append([current, target])
                    cost = f32(cost + abs(f32(target - current)))
                    succeeded = True
                    break
                state = 2 if forward else 3
            endpoint = high if state == 2 else low
            group = node(endpoint)
            next_value = None
            if group is not None:
                visited.append(endpoint)
                next_value = unused(group)
            if next_value is not None:
                segments.append([current, endpoint])
                cost = f32(cost + abs(f32(endpoint - current)))
                visited.append(next_value)
                previous.append(current)
                current, state = next_value, 1
                continue
            if previous:
                current = previous.pop()
                a, b = segments.pop()
                cost = f32(cost - abs(f32(b - a)))
                continue
            if state == 2:
                forward, state = False, 3
                continue
            group = next((i for i, values in enumerate(nodes)
                          if any(low <= v <= high for v in values)), None)
            if group is None:
                break
            endpoint = unused(group)
            endpoint = -1.0 if endpoint is None else endpoint
            visited.append(endpoint)
            next_value = unused(group)
            if next_value is None:
                break
            segments.append([current, endpoint])
            cost = f32(cost + abs(f32(endpoint - current)))
            visited.append(next_value)
            previous.append(current)
            current, state = next_value, 1
        if not succeeded:
            break
        if not any(old[0] == cost for old in candidates):
            candidates.append((cost, segments))
        if not previous:
            break
    if not candidates:
        return {"segments": [[target, target]], "length": 0.0,
                "native_no_route_branch": True, "nonzero_segments": 0}
    cost, segments = min(candidates, key=lambda row: row[0])
    return {"segments": segments, "length": cost, "native_no_route_branch": False,
            "nonzero_segments": sum(a != b for a, b in segments)}


def main():
    prior = read_json(ROOT / "reports/recovery_A_20261116_emote.json")
    driver = Image((ROOT / prior["binary_path"]).parent / "emotedriver.dll",
                   "85eb459e94e74ab608f649eb7fb440f2526e90aaa1af7276ed01dc10018903ce")
    exe = Image(ROOT / prior["binary_path"], prior["PE_sha256"])
    name, data = load_sample(0)
    psb = Psb(data)
    metadata = psb.root["metadata"]
    instant = set(metadata["instantVariableList"])
    controls = {
        entry["label"]: (section, entry)
        for section in ("eyeControl", "eyebrowControl", "mouthControl", "selectorControl")
        for entry in metadata.get(section, [])
        if entry.get("enabled", 1) and (
            section != "eyeControl" or not entry.get("blinkEnabled", 0))
    }
    labels, target_families = [], Counter()
    graph_targets, selector_targets = [], []
    for timeline in metadata["timelineControl"]:
        missing, timed = [], set()
        for variable in timeline["variableList"]:
            label, frames = variable["label"], variable["frameList"]
            if label not in instant or not frames:
                continue
            family, control = controls.get(label, (None, None))
            if family is None:
                missing.append(label)
                continue
            for index, frame in enumerate(frames):
                if not frame.get("content"):
                    continue
                if index + 1 == len(frames):
                    missing.append(label + ": no inactive terminal")
                    continue
                duration = max(0, frames[index + 1]["time"] - frame["time"] - 1)
                target_families[family] += 1
                if duration > 0:
                    timed.add(family)
                record = {"timeline": timeline["label"], "variable": label,
                          "time": frame["time"], "duration": duration,
                          "value": frame["content"]["value"],
                          "easing": frame["content"]["easing"]}
                if family == "selectorControl":
                    record["option_labels"] = [v["label"] for v in control["optionList"]]
                    record["degenerate_ranges"] = [
                        v["label"] for v in control["optionList"]
                        if v["onValue"] == v["offValue"]]
                    selector_targets.append(record)
                elif family in ("eyeControl", "eyebrowControl"):
                    #Source is live controller state, never inferred from the
                    #previous key. Record destinations without pretending it
                    #is a concrete runtime route.
                    graph_targets.append(record)
        labels.append({"label": timeline["label"], "missing_controller_families": missing,
                       "timed_families": sorted(timed),
                       "fits_controller_family_source": not missing,
                       "live_composition_and_pixels_verified": False})

    old_pairs = read_json(ROOT / "reports/recovery_A_20261117_emote_timed.json")[
        "configured_static_endpoint_pairs"]
    pairs = []
    for old in old_pairs:
        record = {key: old[key] for key in (
            "dimension_zero_based", "from", "to", "kind", "frames")}
        routes = {}
        reasons = [r for r in old["blocked_reasons"]
                   if not r.startswith(("cross-edge graph route:", "timed selector option queues:"))]
        for label, target in old["pattern_values_to"].items():
            family, control = controls.get(label, (None, None))
            if family in ("eyeControl", "eyebrowControl"):
                routes[label] = route_inventory(
                    control, old["pattern_values_from"].get(label, 0), target)
                if "diagnostic" in routes[label]:
                    reasons.append(label + ": static route diagnostic")
        record.update({"routes": routes, "remaining_static_blockers": reasons,
                       "fits_family_endpoint_subset": not reasons,
                       "actual_current_state_not_known": True})
        pairs.append(record)
    remaining = [r["label"] for r in labels if not r["fits_controller_family_source"]]
    conditional_timed = [r["label"] for r in labels if r["timed_families"]]
    route_modes = Counter(
        "diagnostic" if "diagnostic" in r else
        "native_no_route_snap" if r["native_no_route_branch"] else
        "all_zero_length_node_jumps" if r["nonzero_segments"] == 0 else
        "one_nonzero_segment" if r["nonzero_segments"] == 1 else "multiple_nonzero_segments"
        for pair in pairs for r in pair["routes"].values())
    summary = {
        "graph_search_source_connected_to_chain_and_timeline": True,
        "ordered_node_jumps_and_native_no_route_snap": True,
        "selector_integer_plus_distance_scaled_option_tweens": True,
        "controller_queries_exclude_additive_timeline_values": True,
        "selector_outputs_applied_before_additive_timelines": True,
        "SinSs_enum4_and_Linear_enum1_same_original_driver_branch": True,
        "SinSs_raw_driver_easing": 1.0,
        "player_owner_clock_commit_after_success": True,
        "timed_timeline_selected_blink_owner_conflicts_are_diagnostics": True,
        "configured_primary_pairs": len(pairs),
        "configured_endpoint_pairs_in_family_subset": sum(p["fits_family_endpoint_subset"] for p in pairs),
        "configured_endpoint_pairs_still_blocked": sum(not p["fits_family_endpoint_subset"] for p in pairs),
        "static_route_modes": dict(route_modes),
        "first_model_labels": len(labels),
        "previous_immediate_only_label_subset": 55,
        "labels_fitting_controller_family_source": sum(r["fits_controller_family_source"] for r in labels),
        "labels_missing_controller_families": len(remaining),
        "labels_requiring_actual_timed_state_validation": len(conditional_timed),
        "configured_pairs_requiring_inter_segment_validation": sum(
            any(r.get("nonzero_segments", 0) > 1 for r in p["routes"].values()) for p in pairs),
        "instant_targets_by_family": dict(target_families),
        "timed_selector_targets": sum(r["duration"] > 0 for r in selector_targets),
        "selector_targets_with_degenerate_option_range": sum(bool(r["degenerate_ranges"]) for r in selector_targets),
        "static_family_counts_are_not_playback_or_pixel_completion": True,
        "full_NEKO_strict_frame_claim": False, "complete_native_count_added": 0,
        "compiled": False, "tests_run": False, "Rust_execution": False,
        "original_game_or_DLL_executed": False,
    }
    remaining_branches = [
        "multi-segment graph updates leaving finite pow domain remain explicit diagnostics; no invented residual-time interpolation",
        "generic queued append targets and selector wait queues beyond current timeline/chain replace mode",
        "same-name compound dimension, selected blink and timeline ownership guards remain",
        "intermediate chain labels, Fade/NoMove object effects, secondary smoothing and complete force owner",
        "automatic LPD preset and nondefault waveform record78/quiet-scan states",
        "generic mesh/stencil and full-model strict pixels",
    ]
    spans = {
        "graph_search_state_reset_and_first_edges": (0x9ECC, 0x131),
        "graph_upper_node_and_visited_successor": (0xA028, 0x107),
        "graph_lower_node_and_backtrack": (0xA1F3, 0xEA),
        "graph_interior_node_fallback": (0xA2DD, 0x101),
        "graph_final_edge_visited_removal": (0xA49C, 0x11A),
        "graph_cost_dedup_and_ten_pass_limit": (0xA5B6, 0x195),
        "graph_minimum_strict_lower": (0x97B0, 0x58),
        "graph_native_no_route_snap": (0x99DB, 0x22),
        "graph_eye_segment_queue_and_distance_correction": (0x69BB, 0xFA),
        "graph_brow_distance_correction": (0x93F9, 0x25),
        "selector_integer_and_per_option_receiver": (0xD07E, 0x4A),
        "selector_actual_current_duration_scaling": (0xD0C7, 0x83),
        "selector_queued_target_and_wait_clock": (0xD386, 0xCF),
        "selector_zero_duration_clear_and_assign": (0xD29B, 0x38),
    }
    paths = [
        "simulator/crates/runtime/src/emote.rs",
        "simulator/crates/runtime/src/emote_m2graph.rs",
        "simulator/crates/runtime/src/emote_m2timed.rs",
        "simulator/crates/runtime/src/emote_variable_timeline.rs",
        "simulator/crates/runtime/src/emote_cpu.rs",
        "scripts/analysis/recovery_A_emote_graph_selector_20261117.py",
    ]
    apis = {
        "public_unchanged": "EmoteServices::import_m2_resource/play_timeline/advance",
        "shared_driver": "CpuM2TimedVariables::submit/submit_targets/advance/finish_all",
        "graph_internal": "m2graph::plan(control,current,target) -> Route{segments,length}",
        "selector_composition": "CpuPlayer::controller_variables -> option receiver bases -> independent additive timeline values",
        "diagnostic": "EmoteError::Unsupported on nonfinite inter-segment math, unsupported ownership or degenerate active option range",
    }
    payload = {
        "schema": "cs2-recovery-A-emote-graph-selector/v1", "task_date": "2026-11-17",
        "generated_at_utc": datetime.now(timezone.utc).isoformat(),
        "summary": summary, "APIs": apis, "remaining_branches": remaining_branches,
        "remaining_labels_missing_controller_families": remaining,
        "conditional_timed_labels_not_playback_verified": conditional_timed,
        "driver": {"path": str(driver.path.relative_to(ROOT)), "sha256": driver.digest},
        "resource": {"name": name, "PSB_sha256": sha(data)},
        "labels": labels, "graph_targets": graph_targets, "selector_targets": selector_targets,
        "configured_static_pairs": pairs,
        "PE_evidence": {key: block(driver, *span) for key, span in spans.items()},
        "EXE_identity": {"path": prior["binary_path"], "sha256": exe.digest},
        "EXE_SinSs_evidence": {
            "named_kind_to_enum": block(exe, 0x197EED, 0x1D),
            "SinSs_token_bytes": block(exe, 0x5E4268, 7)["bytes"],
            "enabled_kind_switch": block(exe, 0x16D36B, 0x36),
            "enum_jump_table": block(exe, 0x16D7E8, 24),
            "enum_jump_table_decoded_VA": [
                hex(v) for v in struct.unpack("<6I", bytes.fromhex(block(exe, 0x16D7E8, 24)["bytes"]))
            ],
            "raw_easing1_branch": block(exe, 0x16D411, 0x5A),
        },
        "EXE_live_value_owner_evidence": {
            "absolute_current_value_and_changed_state": block(exe, 0x16E00D, 0x3F),
            "live_value_updates_pattern_target": block(exe, 0x16E164, 0x28),
            "live_value_updates_last_raw_and_scaled_chain_sample": block(exe, 0x16E18C, 0x5E),
        },
        "sources": [{"path": p, "sha256": sha((ROOT / p).read_bytes())} for p in paths],
        "modified_paths": paths[:-1] + [
            paths[-1],
            "scripts/analysis/recovery_A_emote_timed_20261117.py",
            "reports/recovery_A_20261117_emote_timed.json",
            "reports/recovery_A_20261117_emote_timed.md",
            "reports/recovery_A_20261117_emote_timed_summary.json",
            "reports/recovery_A_20261117_emote_waveform.json",
            "reports/recovery_A_20261117_emote_waveform.md",
            "reports/recovery_A_20261117_emote_waveform_summary.json",
            "reports/recovery_A_20261117_emote_graph_selector.json",
            "reports/recovery_A_20261117_emote_graph_selector.md",
            "reports/recovery_A_20261117_emote_graph_selector_summary.json",
            "reports/recovery_A_20261117_emote_current_summary.json",
        ],
        "classification": "scripts/analysis and reports; no assets or temporary outputs; no main render_scene edits",
    }
    out = ROOT / "reports/recovery_A_20261117_emote_graph_selector"
    out.with_suffix(".json").write_text(
        json.dumps(payload, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    small = {key: payload[key] for key in (
        "schema", "task_date", "summary", "APIs", "remaining_branches",
        "remaining_labels_missing_controller_families",
        "conditional_timed_labels_not_playback_verified", "classification")}
    out.with_name(out.name + "_summary.json").write_text(
        json.dumps(small, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    out.with_suffix(".md").write_text(
        "# A / timed graph与selector控制器族（2026-11-17）\n\n"
        "本批已写实际源码，906 chain与PSB timeline共用同一timed bank。"
        "新emote_m2graph按driver9da0/9710恢复首inclusive edge、实际node列表顺序、"
        "visited/backtrack、内部node补路、最多10轮、按f32累计长度去重及严格最小值选路。"
        "node组之间跳转不计距离；原驱动99db的无有效路线分支就是target→target、长度0，"
        "该原生分支有独立字节证据。没有使用通用Dijkstra或猜测线性跨边。\n\n"
        "69e0/941e消费有向pair并真实跳到其起点，零长度段直接消费；"
        "68d0/9310按pow映射原始任务delta与累计路径距离。69bb/93f9跨段距离修正保持原字节算术，"
        "若下一段进入非有限pow域仍明确诊断，不以自拟余量插值兜底。\n\n"
        "当前保留的20条blink端点冲突有额外证据：EXE16df30的16e02d更新绝对当前值，"
        "16e164更新pattern target，16e18c还改末端raw/scaled chain样本。"
        "因此不能把blink和timed值简单相加；活动timeline timed目标与选中blink同名"
        "也在load/update处明确拒绝，避免新控制器路径静默覆盖银行输出。\n\n"
        "第二族selector已同步落地：d060立即写整数selected，逐option查询真实scalar receiver，"
        "选中项取onValue，其余取offValue；duration乘abs((current-target)/(on-off))。"
        "保留活动tween的同终点剩余时间规则，停稳差值门限为7df78的f32 EPSILON。"
        "option实际输出在加法timeline之前合成，不能每帧被即时on/off规则覆盖，"
        "也不能把加法差分反馈成新tween起点。owner/player/clock只在本次推进成功后提交。\n\n"
        "继续批次：EXE197eed把sin_ss映射枚举4；16d395分发表16d7e8中enum1和enum4"
        "都指向16d411，因此本样本sin_ss与linear都传raw easing=1。已具体启用SinSs，"
        "不按名称猜正弦；Fade/NoMove仍缺对象副作用而保留诊断。\n\n"
        f"首模型115标签中，旧即时子集55；当前{summary['labels_fitting_controller_family_source']}个"
        f"通过控制器家族静态条件，缺控制器族标签{len(remaining)}。"
        f"这只是资源字段/源码路径条件，{len(conditional_timed)}个timed标签仍须验证实际状态、组合和像素，"
        "不等于60个已验收标签或NEKO全模型兼容。\n\n"
        f"primary102显式pair中{summary['configured_endpoint_pairs_in_family_subset']}条"
        f"符合本批端点条件，{summary['configured_endpoint_pairs_still_blocked']}条仍受blink等条件阻断；"
        f"静态路线类型{dict(route_modes)}。不是live调用数量。\n\n"
        "仍缺：跨段非有限域、append/wait队列通用路径、同名维度与blink/timeline合成、"
        "intermediate/Fade/NoMove、secondary/完整force、LPD自动配置及mesh/stencil全模型。"
        "原有主宿主入口/API和C生命周期hooks继续复用；没有改main render_scene或441。\n\n"
        "未编译、未测试、未执行Rust/原EXE/DLL/SDK。新增完整native计数0。"
        "脚本已归scripts/analysis，报告归reports，无临时文件或资源需要归档。\n",
        encoding="utf-8")
    current_path = ROOT / "reports/recovery_A_20261117_emote_current_summary.json"
    current = read_json(current_path)
    current["graph_selector_batch"] = small
    current["summary"]["backend_native_branches"]["906"] = (
        "actual scalar/ordered-graph/selector-option timed source; compound/blink/intermediate/fade incomplete")
    current["summary"]["first_model_timeline_source_subset"] = summary["labels_fitting_controller_family_source"]
    current["summary"]["first_model_timeline_source_subset_scope"] = (
        "controller-family source paths only; not live state, composition, timing or pixel acceptance")
    current["summary"]["first_model_timed_target_labels_remaining"] = len(conditional_timed)
    current["summary"]["first_model_labels_missing_controller_families"] = len(remaining)
    current["summary"]["configured_primary_pair_family_subset"] = summary["configured_endpoint_pairs_in_family_subset"]
    current["summary"]["configured_primary_pair_blink_blocked"] = summary["configured_endpoint_pairs_still_blocked"]
    current["summary"]["remaining_strict_blockers"] = [
        "timed M2 compound ownership and inter-segment finite-domain qualification; model fade and secondary smooth transitions",
        "native924 automatic LPD preset, old-sidecar switching and nondefault quiet-scan state; complete force/global controls",
        "selected blink shared value-table/chain-endpoint synchronization",
        "full stencil/mesh scheduling and nonidentity final display mapping",
        "live-controller save/restore and full-model strict pixel acceptance",
    ]
    for entry in [
        "ordered eye/eyebrow graph routes and native no-route branch",
        "integer selector plus actual distance-scaled scalar option tweens",
        "SinSs and Linear verified same fixed-PE driver branch",
    ]:
        if entry not in current["summary"]["new_CPU_batches"]:
            current["summary"]["new_CPU_batches"].append(entry)
    for entry in paths[:5]:
        if entry not in current["modified_source_paths"] and entry not in current["new_source_paths"]:
            current["new_source_paths"].append(entry)
    if str(out.relative_to(ROOT)) + ".md" not in current["reports"]:
        current["reports"].append(str(out.relative_to(ROOT)) + ".md")
    current["source_hashes_current"] = payload["sources"]
    current_path.write_text(json.dumps(current, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(summary, ensure_ascii=True))


if __name__ == "__main__":
    main()

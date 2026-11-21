"""Targeted raw-BP ancestry and inherited grid evidence; PE/resource reads only."""
from collections import Counter
from datetime import datetime, timezone
import json

from recovery_A_20261116 import ROOT, Image, read_json, sha
from recovery_A_receiver_contracts_20261116 import block
from recovery_A_psb_20261117 import Psb, load_sample

DRIVER_SHA = "85eb459e94e74ab608f649eb7fb440f2526e90aaa1af7276ed01dc10018903ce"


def main():
    identity = read_json(ROOT / "reports/recovery_A_20261116_emote.json")
    driver = Image((ROOT / identity["binary_path"]).parent / "emotedriver.dll", DRIVER_SHA)
    sample_name, data = load_sample(0)
    model = Psb(data)
    counts = Counter()

    def walk(layer, chain):
        counts["layers"] += 1
        has_bp = any(frame.get("content", {}).get("mesh", {}).get("bp") for frame in layer["frameList"])
        combine = bool(layer.get("meshTransform", 0) and layer.get("meshCombine", 0))
        if has_bp:
            counts["layers_with_some_BP_frame"] += 1
            if combine:
                counts["combine_layers_with_some_BP_frame"] += 1
                counts["structural_raw_ancestor_contributions"] += len(chain)
        if layer.get("meshSyncChildMask", 0):
            counts["nonzero_meshSyncChildMask_layers"] += 1
        if any(frame.get("content", {}).get("mesh", {}).get("cc") for frame in layer["frameList"]):
            counts["layers_with_nonempty_cc_frame"] += 1
        if layer.get("type") == 0 and not layer.get("meshTransform", 0) and chain:
            counts["structural_inherited_grid_sprite_layers"] += 1
        next_chain = ([(layer.get("label"),)] if has_bp else []) + (chain if combine else [])
        for child in layer["children"]:
            walk(child, next_chain)

    for obj in model.root["object"].values():
        for motion in obj["motion"].values():
            for layer in motion["layer"]:
                walk(layer, [])
    prior = read_json(ROOT / "reports/recovery_A_20261117_emote_evaluator_summary.json")
    remaining = prior["remaining_implementation_families"]
    for family in remaining:
        if family["id"] == "mesh_hierarchy":
            family["reason"] = "raw BP additive ancestry and consumed-parent retirement plus inherited grid implemented; cc/skin, meshSyncChildMask, referenced-motion joins and inheritMask anchor correction remain diagnostic"
    paths = ["simulator/crates/runtime/src/emote_cpu.rs", "simulator/crates/runtime/src/emote_mesh.rs",
             "scripts/analysis/recovery_A_emote_mesh_hierarchy_20261117.py"]
    summary = {
        "local_raw_BP_ancestor_chain_in_real_CPU_sampler": True,
        "inactive_noncombine_ancestor_stops_chain": True,
        "raw_ancestor_BP_not_already_combined_BP_is_added": True,
        "consumed_active_ancestors_removed_from_geometry_chain": True,
        "prevents_double_sequential_warp_of_consumed_ancestors": True,
        "inherited_mesh_grid_uses_actual_nearest_active_ancestor": True,
        "inherited_grid_truncates_quality_once_then_unsigned_integer_ratios": True,
        "meshSyncChildMask_and_inheritMask_anchor_gaps_keep_explicit_diagnostics": True,
        "actual_first_model_structure": dict(counts),
        "structure_counts_are_not_frame_execution_or_compatibility": True,
        "primary_pair_source_paths_inherited": 102, "primary_pair_explicit_source_blocked": 0,
        "remaining_implementation_family_count": len(remaining), "newly_closed_entire_families": [],
        "timed_labels_pending_final_combined_runtime_validation": 60,
        "completed_native_count_added": 0, "compiled": False, "tests_run": False,
        "Rust_execution": False, "game_or_DLL_executed": False, "full_NEKO_strict_pixel_claim": False,
    }
    spans = {
        "actual_meshTransform_name_and_field": (0x35EB5, 0x48),
        "actual_meshSyncChildMask_name": (0x35F50, 0x4C),
        "actual_meshDivision_name_and_field": (0x35F9C, 0x46),
        "actual_meshCombine_name_and_byte": (0x35FE2, 0x48),
        "physical_parent_vs_effective_mesh_parent": (0x3F5B1, 0x5E),
        "inheritMask_compensation_flag": (0x3F69E, 0x20),
        "active_BP_field_gate": (0x3F6BE, 0x47),
        "copy_raw_own_BP_then_collect_raw_ancestors_and_rewire": (0x3F849, 0x83),
        "source_minus_identity_added_to_destination": (0x2BDB0, 0x76),
        "combined_normalized_points_are_converted_to_own_basis": (0x3FA75, 0x74),
        "inherited_nearest_active_ancestor_and_integer_grid": (0x3FEF2, 0xEE),
        "parent_transform_stop_on_inheritMask": (0x400C3, 0x1A),
        "skipped_ancestor_anchor_delta_applied_to_all_vertices": (0x401C0, 0x51),
    }
    apis = {
        "new_public_grid": "CpuMeshGrid::from_inherited_division(width,height,ancestor_width,ancestor_height,ancestor_division,quality)",
        "actual_backend_path": "RenderContext::layer -> PatchAncestor raw chain -> CpuBezierPatch::combine_offsets -> sprite/raster; consumed parents are removed before drawing",
        "all_layers_tracked": "meshCombine==0 cuts the raw chain even if that layer has no active BP; meshCombine==1 carries it through inactive nodes",
        "host_integration": "existing CpuEmoteBackend::render_rgba; no new main/native/C hooks",
    }
    missing = [
        {"receiver": "RenderContext::layer", "missing": "meshSyncChildMask field propagation and cc/skin ownership; meshTransform variants other than one"},
        {"receiver": "RenderContext::motion", "missing": "referenced-motion join mesh ownership; explicit warning when incoming warps exist"},
        {"receiver": "RenderContext::layer", "PE_RVA": ["400dd", "401c0"], "missing": "inheritMask disabled geometry anchor compensation; explicit warning when skipping real ancestors"},
        {"receiver": "literal_stencil_clip/stencil_source_motion", "missing": "joined/inherited/nested source pass scheduler, non-type5 and mode1"},
    ]
    payload = {"schema": "cs2-recovery-A-emote-mesh-hierarchy/v1", "task_date": "2026-11-17",
        "generated_at_utc": datetime.now(timezone.utc).isoformat(), "summary": summary, "APIs": apis,
        "driver": {"sha256": driver.digest, "address_kind": "RVA", "image_base": driver.base},
        "sample": {"resource_name": sample_name, "decoded_sha256": sha(data), "no_assets_exported": True},
        "remaining_implementation_families": remaining, "specific_missing_receivers": missing,
        "driver_evidence": {name: block(driver, *span) for name, span in spans.items()},
        "sources": [{"path": p, "sha256": sha((ROOT / p).read_bytes())} for p in paths],
        "classification": "scripts/analysis + reports; no temporary products/new directories/root README or main render_scene edits"}
    prefix = ROOT / "reports/recovery_A_20261117_emote_mesh_hierarchy"
    payload["modified_paths"] = paths + ["reports/" + prefix.name + ext for ext in (".json", ".md", "_summary.json")] + [
        "reports/recovery_A_20261117_emote_current_summary.json"]
    prefix.with_suffix(".json").write_text(json.dumps(payload, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    small = {k: payload[k] for k in ("schema", "task_date", "summary", "APIs", "remaining_implementation_families", "specific_missing_receivers", "sources", "classification")}
    prefix.with_name(prefix.name + "_summary.json").write_text(json.dumps(small, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    prefix.with_suffix(".md").write_text(
        "# A / 实际 raw BP 祖先链与继承网格（2026-11-17）\n\n"
        "已修改emote_cpu.rs/emote_mesh.rs，直接进入既有CPU sprite/raster路径，无需宿主新增hook。"
        "3f876复制当前raw BP，3f880逐物理祖先添加raw BP减identity，经过第一个meshCombine=0祖先后停止，"
        "即使该祖先没有活动BP也要停止。独立PatchAncestor链记录这条物理来源；与effective warp链分开。"
        "3f889..3f897跳过已消费的活动祖先，现同步从几何链移除，避免相同BP同时加法合并又顺序变形。\n\n"
        "新增CpuMeshGrid::from_inherited_division。3ff31按最近活动祖先的division乘quality并截断一次，"
        "再乘当前(width+height)/祖先(width+height)做DWORD整数除法，最后按当前宽高分配并各加1。"
        "已替换无own meshTransform时的固定16×16 preview网格，进入真实三角形绘制入口。\n\n"
        "实际首模型结构（只读解析，不执行帧）：" + json.dumps(dict(counts), ensure_ascii=False) + "。"
        "这些是结构涉及量，不是已运行标签/完整像素兼容数量。\n\n"
        "mesh整族仍未闭合：cc/skin、meshSyncChildMask（实际大量为8）、跨referenced motion的join所有权、"
        "inheritMask禁用时400dd/401c0的anchor补偿均明确诊断；其他meshTransform值继续诊断。"
        "9个声明族保持不变，primary102/102/0阻断沿用，完整native新增0。strict仍拒绝未恢复模型分支，"
        "未把diagnostic preview计作成功；60timed标签只待最终统一验证。\n\n"
        "固定driver SHA256=" + driver.digest + "；JSON保存RVA原字节、源码哈希与具体接收者。"
        "不编译、不测试、不运行Rust/VM/游戏/DLL/APK；仅rustfmt语法读取及Python只读PE/资源解析。"
        "脚本已归scripts/analysis，报告归reports；无临时产物、资产发布或新目录。\n", encoding="utf-8")
    current_path = ROOT / "reports/recovery_A_20261117_emote_current_summary.json"
    current = read_json(current_path)
    current["mesh_hierarchy_batch"] = small
    current["summary"]["remaining_implementation_family_count"] = len(remaining)
    current["summary"]["remaining_strict_blockers"] = [f["reason"] for f in remaining]
    hashes = {row["path"]: row for row in current.get("source_hashes_current", [])}
    hashes.update({row["path"]: row for row in payload["sources"]})
    current["source_hashes_current"] = list(hashes.values())
    report = "reports/" + prefix.name + ".md"
    if report not in current.setdefault("reports", []):
        current["reports"].append(report)
    current_path.write_text(json.dumps(current, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(summary, ensure_ascii=True))


if __name__ == "__main__":
    main()

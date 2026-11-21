"""Targeted c1 mesh barrier and anchor compensation; immutable PE/PSB inputs."""
from collections import Counter
from datetime import datetime, timezone
import json

from recovery_A_20261116 import ROOT, Image, read_json, sha
from recovery_A_receiver_contracts_20261116 import block
from recovery_A_psb_20261117 import Psb, load_sample


def main():
    identity = read_json(ROOT / "reports/recovery_A_20261116_emote.json")
    driver = Image((ROOT / identity["binary_path"]).parent / "emotedriver.dll",
                   "85eb459e94e74ab608f649eb7fb440f2526e90aaa1af7276ed01dc10018903ce")
    name, data = load_sample(0)
    root = Psb(data).root
    counts, barriers = Counter(), []

    def walk(layer, below, obj_name, motion_name):
        disabled = not layer.get("inheritMask", 0x2000000) & 0x2000000
        if disabled:
            counts["structural_inheritMask_barriers"] += 1
            barriers.append({"object": obj_name, "motion": motion_name, "label": layer.get("label"),
                             "type": layer.get("type"), "inheritMask": layer.get("inheritMask"),
                             "meshSyncChildMask": layer.get("meshSyncChildMask", 0)})
        if below:
            counts["physical_descendants_below_barrier"] += 1
            if layer.get("type") == 0:
                counts["sprite_descendants_below_barrier"] += 1
        for child in layer.get("children", []):
            walk(child, below or disabled, obj_name, motion_name)

    for obj_name, obj in root["object"].items():
        for motion_name, motion in obj["motion"].items():
            for layer in motion["layer"]:
                walk(layer, False, obj_name, motion_name)
    current_path = ROOT / "reports/recovery_A_20261117_emote_current_summary.json"
    current = read_json(current_path)
    remaining = current["mesh_sync_batch"]["remaining_implementation_families"]
    for row in remaining:
        if row["id"] == "mesh_hierarchy":
            row["reason"] = "raw BP ancestry, active-parent retirement, inherited grids, bit8 propagation and c1 geometry-stop/anchor delta implemented; other mask bits, cc/skin, meshTransform variants and referenced-motion mesh owner transfer remain diagnostic"
    summary = {
        "inheritMask_c1_barrier_retained_without_active_own_BP": True,
        "current_and_parent_c1_stop_full_geometry_walk": True,
        "skipped_ancestors_deform_anchor_only": True,
        "skipped_anchor_delta_added_to_all_actual_vertices": True,
        "own_BP_does_not_deform_the_pre_mesh_anchor": True,
        "combine_retires_only_active_consumed_parents": True,
        "grid_finds_nearest_active_patch_through_inactive_barriers": True,
        "same_mesh_plan_used_by_color_and_stencil_primitives": True,
        "first_model_structure": dict(counts), "structure_is_not_runtime_coverage": True,
        "primary_pair_source_paths_inherited": 102, "primary_pair_explicit_source_blocked": 0,
        "remaining_implementation_family_count": len(remaining), "newly_closed_entire_families": [],
        "timed_labels_pending_final_combined_runtime_validation": 60,
        "completed_native_count_added": 0, "compiled": False, "tests_run": False,
        "Rust_execution": False, "game_or_DLL_executed": False, "full_NEKO_strict_pixel_claim": False,
    }
    apis = {
        "effective_parent": "MeshAncestor{owner,warp:Option<Warp>,stops_geometry} preserves inactive c1 nodes",
        "real_sampler": "mesh_raster_plan(chain,current_owner,world_anchor) -> full geometry warps + uniform anchor delta",
        "render": "sprite applies that plan to every real quad/mesh vertex before color/stencil triangle raster",
        "host": "unchanged CpuEmoteBackend::render_rgba; no main render_scene/native_tasks hooks",
    }
    missing = [
        {"receiver": "RenderContext::motion / stencil_source_motion", "driver_RVA": ["41afd", "43cbd"],
         "missing": "mesh owner/effective parent transfer across referenced motion roots; generic join remains diagnostic"},
        {"receiver": "RenderContext::layer / mesh payload", "missing": "cc/skin, meshTransform other than one, meshSyncChildMask bits other than8"},
        {"receiver": "stencil scheduler", "missing": "joined local/caller chains, nested masks, source queue color lifetime, blank/group source variants and mode1"},
    ]
    spans = {
        "c1_gate_is_actual_inheritMask_bit02000000": (0x3F69E, 0x20),
        "anchor_initialized_before_own_BP": (0x3F8FE, 0x34),
        "current_c1_stops_before_parent_geometry": (0x3FFDD, 0x23),
        "full_geometry_and_anchor_deformed_together": (0x40000, 0xC3),
        "parent_c1_stops_after_that_parent": (0x400C3, 0x37),
        "remaining_ancestors_update_anchor_only": (0x40100, 0xB6),
        "uniform_anchor_delta_to_every_vertex": (0x401B6, 0x5B),
        "combined_raw_offsets_and_active_parent_rewire": (0x3F880, 0x4C),
    }
    paths = ["simulator/crates/runtime/src/emote_cpu.rs", "scripts/analysis/recovery_A_emote_mesh_anchor_20261117.py"]
    payload = {"schema": "cs2-recovery-A-emote-mesh-anchor/v1", "task_date": "2026-11-17",
        "generated_at_utc": datetime.now(timezone.utc).isoformat(), "summary": summary, "APIs": apis,
        "remaining_implementation_families": remaining, "specific_missing_receivers": missing,
        "driver": {"sha256": driver.digest, "image_base": driver.base, "address_kind": "RVA"},
        "sample": {"resource": name, "decoded_sha256": sha(data), "barriers": barriers, "no_assets_exported": True},
        "driver_evidence": {key: block(driver, *span) for key, span in spans.items()},
        "sources": [{"path": p, "sha256": sha((ROOT / p).read_bytes())} for p in paths],
        "classification": "scripts/analysis + reports; no temporary products/new directories/root README/main render_scene/native_tasks edits"}
    prefix = ROOT / "reports/recovery_A_20261117_emote_mesh_anchor"
    payload["modified_paths"] = paths + ["reports/" + prefix.name + ext for ext in (".json", ".md", "_summary.json")] + ["reports/recovery_A_20261117_emote_current_summary.json"]
    prefix.with_suffix(".json").write_text(json.dumps(payload, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    small = {key: payload[key] for key in ("schema", "task_date", "summary", "APIs", "remaining_implementation_families", "specific_missing_receivers", "sources", "classification")}
    prefix.with_name(prefix.name + "_summary.json").write_text(json.dumps(small, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    prefix.with_suffix(".md").write_text(
        "# A / inheritMask c1及真实anchor补偿（2026-11-17）\n\n"
        "本批直接修改emote_cpu.rs。3f6a6以inheritMask bit02000000设置c1；3ffdd在当前c1处跳过父geometry，"
        "400c3在parent.c1处停止后续geometry变形。400dd保存停止点anchor，40100继续遍历剩余活动BP，"
        "仅变形该anchor；401d2..4020f把after-before统一加到每个顶点。不能简单丢弃全部祖先。\n\n"
        "现在MeshAncestor分开可空活动patch与c1，保留无活动BP的barrier。mesh_raster_plan生成真正geometry warp序列"
        "和uniform anchor delta；本层BP只变形几何，不提前变形3f90e/3f92d初始化的anchor。"
        "同一plan接入color和stencil的逐纹理图元raster。meshCombine仅退休实际活动且被raw加法消费的parent；"
        "无活动BP的c1节点仍保留，grid穿过它寻找最近活动patch。二维affine basis沿用既有采样器；3D等旧诊断保留。\n\n"
        "首模型结构=" + json.dumps(dict(counts), ensure_ascii=False) + "；具体barrier资源身份见JSON。"
        "这些不是执行帧覆盖或像素验收。仍缺跨motion mesh owner转移、cc/skin、其他mask及meshTransform分支；"
        "stencil双链/nested/source color/mode1等不因本批消失。声明9族不变，primary102/102/0沿用，"
        "60timed待最终统一验证，完整native新增0。\n\n"
        "driver SHA256=" + driver.digest + "；证据保留原RVA字节、指令及源码哈希。"
        "未编译、测试、执行Rust/VM/游戏/DLL/APK；只进行rustfmt语法读取与Python只读PE/PSB分析。"
        "脚本分类scripts/analysis，报告reports；无临时产物、资产发布或新增目录。\n", encoding="utf-8")
    current["mesh_anchor_batch"] = small
    current["summary"]["remaining_implementation_family_count"] = len(remaining)
    current["summary"]["remaining_strict_blockers"] = [row["reason"] for row in remaining]
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

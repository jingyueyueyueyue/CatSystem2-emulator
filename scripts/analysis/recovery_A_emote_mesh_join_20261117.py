"""Read-only fixed-driver evidence for effective mesh parents across motions."""
from collections import Counter
import json

from recovery_A_20261116 import ROOT, Image, read_json, sha
from recovery_A_receiver_contracts_20261116 import block
from recovery_A_psb_20261117 import Psb, load_sample


def main():
    identity = read_json(ROOT / "reports/recovery_A_20261116_emote.json")
    driver = Image((ROOT / identity["binary_path"]).parent / "emotedriver.dll",
                   "85eb459e94e74ab608f649eb7fb440f2526e90aaa1af7276ed01dc10018903ce")
    sample, data = load_sample(0)
    root = Psb(data).root
    counts, rows = Counter(), []

    def walk(layer, ancestors, obj, motion):
        if layer.get("type") == 3:
            counts["motion_reference_layers"] += 1
            if ancestors:
                counts["references_below_structural_active_mesh_parent"] += 1
            rows.append({"object": obj, "motion": motion, "label": layer.get("label"),
                         "ancestor_mesh_labels": ancestors,
                         "meshTransform": layer.get("meshTransform"),
                         "inheritMask": layer.get("inheritMask")})
        own = layer.get("meshTransform") == 1 and layer.get("meshSyncChildMask", 0) & 8
        for child in layer.get("children", []):
            walk(child, ancestors + ([layer.get("label")] if own else []), obj, motion)

    for obj, value in root["object"].items():
        for motion, value in value["motion"].items():
            for layer in value["layer"]:
                walk(layer, [], obj, motion)
    current_path = ROOT / "reports/recovery_A_20261117_emote_current_summary.json"
    current = read_json(current_path)
    remaining = current["mesh_anchor_batch"]["remaining_implementation_families"]
    for row in remaining:
        if row["id"] == "mesh_hierarchy":
            row["reason"] = "effective2c4 mesh parent now transfers across ordinary and stencil-source motion roots with original owner/world transform and independent physical raw BP ancestry; cc/skin, meshSyncChildMask bits other than8 and meshTransform variants remain diagnostic"
    summary = {
        "effective_parent_2c4_transferred_to_referenced_motion_root": True,
        "referencing_layer_own_BP_and_c1_excluded_from_transfer": True,
        "consumed_parent_rewiring_retained_before_transfer": True,
        "original_parent_owner_and_world_transform_retained": True,
        "referenced_motion_physical_raw_BP_tree_independent": True,
        "ordinary_color_and_stencil_source_motion_share_transfer": True,
        "blanket_cross_motion_mesh_warning_removed": True,
        "first_model_structure": dict(counts), "structure_is_not_runtime_coverage": True,
        "remaining_implementation_family_count": len(remaining),
        "closed_subbranches": ["mesh_hierarchy/effective_parent_motion_join"],
        "newly_closed_entire_families": [], "primary_pair_source_paths_inherited": 102,
        "primary_pair_explicit_source_blocked": 0,
        "timed_labels_pending_final_combined_runtime_validation": 60,
        "completed_native_count_added": 0, "compiled": False, "tests_run": False,
        "target_execution": False, "full_NEKO_strict_pixel_claim": False,
    }
    apis = {"internal": "DrawLayer::motion_parent_warps stores rewritten effective parent before own BP/c1",
            "receivers": "RenderContext::motion + stencil_source_motion + literal_stencil_clip(kind3)",
            "pixels": "same sprite/mesh_raster_plan -> actual color and mask triangle raster",
            "host": "CpuEmoteBackend::render_rgba unchanged; no new main render_scene/native_tasks hook"}
    missing = [
        {"receiver": "RenderContext::layer", "missing": "cc/skin, other sync-mask bits, non-one meshTransform"},
        {"receiver": "stencil scheduler", "missing": "joined local/caller chains; nested masks; source2cc ordinary color/range ownership; blank/group variants; mode1 targets/composition"},
    ]
    spans = {
        "motion_root_copies_effective_2c4_and_stencil_2d4": (0x41af0, 0x56),
        "secondary_reference_path_copies_same_root_fields": (0x43ca2, 0x4e),
        "local_physical_parent_selects_c0_c1_or_effective_2c4": (0x3f5bb, 0x65),
        "raw_combination_walk_stays_in_current_motion_node_array": (0x3f880, 0x4c),
    }
    paths = ["simulator/crates/runtime/src/emote_cpu.rs", "scripts/analysis/recovery_A_emote_mesh_join_20261117.py"]
    payload = {"schema": "cs2-recovery-A-emote-mesh-join/v1", "task_date": "2026-11-17",
               "summary": summary, "APIs": apis, "remaining_implementation_families": remaining,
               "specific_missing_receivers": missing,
               "driver": {"sha256": driver.digest, "image_base": driver.base, "address_kind": "RVA"},
               "sample": {"resource": sample, "decoded_sha256": sha(data), "references": rows,
                          "no_assets_exported": True},
               "driver_evidence": {name: block(driver, *span) for name, span in spans.items()},
               "sources": [{"path": p, "sha256": sha((ROOT / p).read_bytes())} for p in paths],
               "classification": "scripts/analysis + reports; no temporary products/new directories/root README/main render_scene/native_tasks edits"}
    prefix = ROOT / "reports/recovery_A_20261117_emote_mesh_join"
    prefix.with_suffix(".json").write_text(json.dumps(payload, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    small = {key: payload[key] for key in ("schema", "task_date", "summary", "APIs", "remaining_implementation_families", "specific_missing_receivers", "sources", "classification")}
    prefix.with_name(prefix.name + "_summary.json").write_text(json.dumps(small, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    prefix.with_suffix(".md").write_text(
        "# A / 跨motion的实际mesh父链接收\n\n"
        "已修改emote_cpu.rs：41afd/43cbd直接把引用层的effective2c4复制到新motion root，"
        "没有复制该层自身c0/BP/c1。DrawLayer::motion_parent_warps现在在combined父链重写之后、"
        "本层BP/c1加入之前保存该指针对应链。原owner和world transform沿用；新motion的physical/raw BP树独立，"
        "3f8ba..3f8c4的raw加法仍只索引当前motion节点数组，不能把外部raw BP接入。\n\n"
        "实际接收链：RenderContext::motion(kind3)或literal_stencil_clip(kind3) → "
        "stencil_source_motion/子motion.layer → inherited subdivision → mesh_raster_plan → "
        "真实color/mask triangle raster。删除已被本批闭合的跨motion blanket warning，其他未恢复分支仍严格诊断。\n\n"
        "首模型只读结构=" + json.dumps(dict(counts), ensure_ascii=False) + "。不是运行/像素覆盖。"
        "仍缺cc/skin、其他sync bits、meshTransform变体；stencil双链/nested/source2cc颜色range/blank/group/mode1。"
        "剩余声明9族，primary102/102、pair明确阻断0沿用，60timed待最终统一验证，完整native新增0。\n\n"
        "固定driver SHA256=" + driver.digest + "；JSON含RVA原字节、指令和当前源码SHA256。"
        "未编译、测试或执行目标；仅rustfmt语法读取及Python静态PE/PSB分析。脚本归scripts/analysis，"
        "正式报告归reports；无临时产物、新目录、资产发布、main render_scene或task hooks修改。\n", encoding="utf-8")
    current["mesh_join_batch"] = small
    current["summary"]["remaining_strict_blockers"] = [row["reason"] for row in remaining]
    hashes = {row["path"]: row for row in current.get("source_hashes_current", [])}
    hashes.update({row["path"]: row for row in payload["sources"]})
    current["source_hashes_current"] = list(hashes.values())
    current.setdefault("reports", []).append("reports/" + prefix.name + ".md")
    current_path.write_text(json.dumps(current, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(summary))


if __name__ == "__main__":
    main()

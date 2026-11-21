"""Targeted meshSyncChildMask bit8 evidence and source handoff; static only."""
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
    masks, counts = Counter(), Counter()

    def walk(layer):
        mask = layer.get("meshSyncChildMask", 0)
        masks[str(mask)] += 1
        has_bp = any(frame.get("content", {}).get("mesh", {}).get("bp") for frame in layer.get("frameList", []))
        if has_bp:
            counts["layers_with_some_BP_frame"] += 1
            counts["BP_layers_with_child_bit8" if mask & 8 else "BP_layers_without_child_bit8"] += 1
        if mask & ~8:
            counts["layers_with_unrecovered_mask_bits"] += 1
        for child in layer.get("children", []):
            walk(child)

    for obj in root["object"].values():
        for motion in obj["motion"].values():
            for layer in motion["layer"]:
                walk(layer)
    current_path = ROOT / "reports/recovery_A_20261117_emote_current_summary.json"
    current = read_json(current_path)
    remaining = current["stencil_own_batch"]["remaining_implementation_families"]
    for row in remaining:
        if row["id"] == "mesh_hierarchy":
            row["reason"] = "raw BP ancestry, consumed-parent retirement, inherited subdivision and meshSyncChildMask bit8 child propagation implemented; other mask bits, cc/skin, referenced-motion mesh joins and inheritMask anchor compensation remain diagnostic"
    paths = ["simulator/crates/runtime/src/emote_cpu.rs", "scripts/analysis/recovery_A_emote_mesh_sync_20261117.py"]
    summary = {
        "meshSyncChildMask_bit8_parent_activity_gate_implemented": True,
        "own_BP_draw_is_separate_from_child_BP_propagation": True,
        "mask_zero_children_keep_effective_parent_chain_without_own_BP": True,
        "raw_meshCombine_ancestry_independent_of_bit8": True,
        "other_mask_bits_explicitly_diagnostic": True,
        "unproven_fixed16_grid_fallback_removed": True,
        "first_model_mask_distribution": dict(masks),
        "first_model_structural_counts": dict(counts),
        "structure_is_not_active_frame_or_pixel_verification": True,
        "primary_pair_source_paths_inherited": 102, "primary_pair_explicit_source_blocked": 0,
        "remaining_implementation_family_count": len(remaining), "newly_closed_entire_families": [],
        "timed_labels_pending_final_combined_runtime_validation": 60,
        "completed_native_count_added": 0, "compiled": False, "tests_run": False,
        "Rust_execution": False, "game_or_DLL_executed": False, "full_NEKO_strict_pixel_claim": False,
    }
    apis = {
        "actual_receiver": "RenderContext::layer separates draw_warps and child_warps; Warp::propagate_to_children is actual mask bit8",
        "raw_ancestry": "PatchAncestor raw entries still carry inactive BP for the separately recovered additive meshCombine operation",
        "grid": "sprite requires its actual CpuMeshGrid when any warp is present; missing grid is Err",
        "host": "unchanged CpuEmoteBackend::render_rgba; no new native_tasks/main render_scene hooks",
    }
    missing = [
        {"receiver": "RenderContext::layer", "driver_RVA": ["3f6a6", "400c3", "401c0"],
         "missing": "inheritMask c1 stop/anchor compensation and exact retained anchor basis, including descendants crossing the barrier"},
        {"receiver": "RenderContext::motion", "missing": "referenced-motion mesh/cc/skin owner transfer; current mesh join diagnostic retained"},
        {"receiver": "RenderContext::layer", "missing": "cc/skin coefficients, meshTransform values other than one, and mask bits other than8 remain explicit"},
        {"receiver": "RenderContext stencil scheduling", "missing": "joined local/caller owners, nested source chains, source queue color lifetime, blank/group variants and mode1"},
    ]
    spans = {
        "parse_meshSyncChildMask_to_mesh_record_word0": (0x35F50, 0x4C),
        "physical_parent_c0_c1_selects_effective_mesh_chain": (0x3F5DB, 0x2E),
        "inheritMask_c1_is_separate_from_child_c0": (0x3F69E, 0x20),
        "active_BP_and_bit8_gate_for_c0": (0x3F6BE, 0x47),
        "own_BP_generation_not_gated_by_child_c0": (0x3F816, 0x3F),
        "child_geometry_walk_uses_only_parent_c0": (0x3FFF3, 0x37),
        "inheritMask_anchor_delta_remains_separate": (0x401C0, 0x51),
    }
    payload = {
        "schema": "cs2-recovery-A-emote-mesh-sync/v1", "task_date": "2026-11-17",
        "generated_at_utc": datetime.now(timezone.utc).isoformat(), "summary": summary, "APIs": apis,
        "remaining_implementation_families": remaining, "specific_missing_receivers": missing,
        "driver": {"sha256": driver.digest, "image_base": driver.base, "address_kind": "RVA"},
        "sample": {"resource": name, "decoded_sha256": sha(data), "no_assets_exported": True},
        "driver_evidence": {key: block(driver, *span) for key, span in spans.items()},
        "sources": [{"path": p, "sha256": sha((ROOT / p).read_bytes())} for p in paths],
        "classification": "scripts/analysis + reports; no temporary products/new directories/root README/main render_scene/native_tasks edits",
    }
    prefix = ROOT / "reports/recovery_A_20261117_emote_mesh_sync"
    payload["modified_paths"] = paths + ["reports/" + prefix.name + ext for ext in (".json", ".md", "_summary.json")] + ["reports/recovery_A_20261117_emote_current_summary.json"]
    prefix.with_suffix(".json").write_text(json.dumps(payload, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    small = {key: payload[key] for key in ("schema", "task_date", "summary", "APIs", "remaining_implementation_families", "specific_missing_receivers", "sources", "classification")}
    prefix.with_name(prefix.name + "_summary.json").write_text(json.dumps(small, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    prefix.with_suffix(".md").write_text(
        "# A / meshSyncChildMask bit8真实传播（2026-11-17）\n\n"
        "本批修改emote_cpu.rs。35f5b读取meshSyncChildMask到mesh record word0；3f6f0测试bit8，"
        "在存在实际BP等前提下设置c0。3f5db据parent.c0/c1选择effective2c4；3fff3只让活动parent.c0变形子几何。"
        "相对地，3f816生成本层BP的入口不以本层c0为门控。因此bit8决定子层传播，不能当作本层是否绘制BP的开关。\n\n"
        "现在draw_warps保留本层实际patch，child_warps仅在bit8启用时传递该patch；mask0子层继续使用有效祖先链。"
        "原始BP的meshCombine物理祖先加法仍独立，不因c0关闭丢失raw贡献。"
        "未知mask bits继续诊断；sprite有warp却无真实grid时返回Err，移除固定16x16替代。\n\n"
        "首模型meshSyncChildMask分布=" + json.dumps(dict(masks), ensure_ascii=False)
        + "，BP相关结构=" + json.dumps(dict(counts), ensure_ascii=False) + "。仅为只读结构，非活动帧覆盖率。\n\n"
        "仍缺inheritMask c1/barrier的anchor补偿、cc/skin、跨motion mesh所有权、其他mask/meshTransform分支。"
        "stencil双链/nested/source color queue/mode1也继续诊断。声明剩余9族，primary102/102/0沿用，"
        "60timed留最终统一验证，完整native新增0，不声明完整NEKO像素兼容。\n\n"
        "driver SHA256=" + driver.digest + "，RVA字节与源码哈希见JSON。未编译、测试或执行Rust/VM/游戏/DLL/APK；"
        "仅rustfmt语法读取及Python只读PE/资源分析。脚本已分类scripts/analysis，报告reports；无临时产物或新增目录。\n", encoding="utf-8")
    current["mesh_sync_batch"] = small
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

"""Bounded own-geometry and referenced-motion stencil contracts, static only."""
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
    sample, data = load_sample(0)
    root = Psb(data).root
    counts, kinds = Counter(), Counter()

    def walk(layer, parents):
        counts["physical_layers"] += 1
        kind = layer.get("stencilType", 0)
        kinds[str(kind)] += 1
        if layer.get("type") == 3:
            counts["referenced_motion_layers"] += 1
            if parents:
                counts["referenced_motion_layers_below_structural_stencil_owner"] += 1
            if kind:
                counts["referenced_motion_layers_with_own_stencil_type"] += 1
        if kind in (1, 2) and layer.get("type") == 0:
            counts["own_sprite_type1_type2_layers"] += 1
        for child in layer.get("children", []):
            walk(child, parents + (1 if kind else 0))

    for obj in root["object"].values():
        for motion in obj["motion"].values():
            for layer in motion["layer"]:
                walk(layer, 0)
    current_path = ROOT / "reports/recovery_A_20261117_emote_current_summary.json"
    current = read_json(current_path)
    remaining = current["destructive_open_batch"]["remaining_implementation_families"]
    for row in remaining:
        if row["id"] == "stencil_scheduler":
            row["reason"] = "local type1/2 own-sprite and type5/6 composite passes, plus caller-only EQUAL clip across motion references, implemented; joined local/caller owner chains, nested mask sources, source color queue ownership, blank/group variants and mode1 remain diagnostic"
    paths = ["simulator/crates/runtime/src/emote_cpu.rs", "simulator/crates/runtime/src/emote_stencil.rs",
             "scripts/analysis/recovery_A_emote_stencil_own_20261117.py"]
    summary = {
        "local_type1_type2_owner_textured_geometry_implemented": True,
        "local_type5_type6_composite_geometry_inherited": True,
        "ordinary_referenced_motion_carries_caller_equal_plane_and_reference": True,
        "local_plus_caller_join_explicitly_rejected": True,
        "referenced_motion_itself_stencil_owner_explicitly_rejected": True,
        "general_strict_stencil_diagnostic_preserved": True,
        "structure_only_not_frame_or_native_acceptance": True,
        "first_model_counts": dict(counts), "first_model_stencil_types": dict(kinds),
        "primary_pair_source_paths_inherited": 102, "primary_pair_explicit_source_blocked": 0,
        "remaining_implementation_family_count": len(remaining), "newly_closed_entire_families": [],
        "timed_labels_pending_final_combined_runtime_validation": 60,
        "completed_native_count_added": 0, "compiled": False, "tests_run": False,
        "Rust_execution": False, "game_or_DLL_executed": False, "full_NEKO_strict_pixel_claim": False,
    }
    apis = {
        "binding": "resolve_emote_stencil_bindings adds type1/2 owner_index as its own geometry source; non-sprite variants remain diagnostic",
        "internal_plan": "stencil::local_stencil_plan replaces internal local_composite_plan; supports local1/2/5/6 with real clear/reference/operation",
        "actual_renderer": "RenderContext::motion receives inherited_clip; kind3 passes caller EQUAL plane/ref to actual child sprite raster",
        "host": "unchanged CpuEmoteBackend::render_rgba; no render_scene or native task hooks",
    }
    missing = [
        {"receiver": "RenderContext::motion / literal_stencil_clip", "driver_RVA": ["402cd", "41b12", "43cd2"],
         "missing": "joining local and caller linked owners, and a referenced motion which is itself a stencil owner; explicit Err rather than inferred independent buffer intersection"},
        {"receiver": "RenderContext::stencil_source_motion", "missing": "nested source masks and their joined owner schedule"},
        {"receiver": "RenderContext::motion / primitive collection", "driver_RVA": ["44ae6", "44b0c", "45000"],
         "missing": "source2cc ordinary color emission/range lifetime; cannot interpret2cc as skipping the whole source layer"},
        {"receiver": "RenderContext::literal_stencil_clip", "driver_RVA": ["540b"],
         "missing": "blank sprites and non-textured/group owner variants; no guessed opaque bounds"},
        {"receiver": "stencil target backend", "driver_RVA": ["57600", "578e0", "57ae0", "57c30"],
         "missing": "mode1 independent target, projection, blending and final composition"},
    ]
    spans = {
        "noncomposite_dispatch_uses_owner_primitive": (0x540B, 9),
        "owner_quad_geometry_to_actual_draw": (0x558B, 0x4A),
        "owner_mesh_branch_and_projection": (0x5641, 0x2A),
        "nearest_active_stencil_owner_chain": (0x402B0, 0xA9),
        "reference_motion_root_inherits_actual_caller2d4": (0x41B09, 0x15),
        "second_reference_path_inherits_actual_caller2d4": (0x43CC9, 0x15),
        "primitive_parent_chain_drives_stencil_rebuild": (0x49BB, 0x29),
        "same_include_exclude_chain_and_final_reference": (0x55D5, 0x32),
    }
    payload = {
        "schema": "cs2-recovery-A-emote-stencil-own/v1", "task_date": "2026-11-17",
        "generated_at_utc": datetime.now(timezone.utc).isoformat(), "summary": summary, "APIs": apis,
        "remaining_implementation_families": remaining, "specific_missing_receivers": missing,
        "driver": {"sha256": driver.digest, "image_base": driver.base, "address_kind": "RVA"},
        "sample": {"resource": sample, "decoded_sha256": sha(data), "no_assets_exported": True},
        "driver_evidence": {name: block(driver, *span) for name, span in spans.items()},
        "sources": [{"path": p, "sha256": sha((ROOT / p).read_bytes())} for p in paths],
        "classification": "scripts/analysis + reports; no temporary products/new directories/root README/main render_scene/native_tasks edits",
    }
    prefix = ROOT / "reports/recovery_A_20261117_emote_stencil_own"
    payload["modified_paths"] = paths + ["reports/" + prefix.name + ext for ext in (".json", ".md", "_summary.json")] + ["reports/recovery_A_20261117_emote_current_summary.json"]
    prefix.with_suffix(".json").write_text(json.dumps(payload, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    small = {key: payload[key] for key in ("schema", "task_date", "summary", "APIs", "remaining_implementation_families", "specific_missing_receivers", "sources", "classification")}
    prefix.with_name(prefix.name + "_summary.json").write_text(json.dumps(small, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    prefix.with_suffix(".md").write_text(
        "# A / stencil自身几何与caller-only继承（2026-11-17）\n\n"
        "本批直接修改emote_cpu.rs与emote_stencil.rs。driver540b在非composite分支使用owner自己的纹理primitive，"
        "558b..55d0提交其真实quad；有mesh则5641进入相应几何路径。type1/2纹理sprite现在以owner_index自身作source，"
        "沿既有真实纹理/网格triangle采样器写入mask，不再要求composite名单，不使用opaque bounds。"
        "内部调度改名local_stencil_plan，local1/2/5/6共用已恢复的两pass及最终EQUAL引用值。\n\n"
        "41b12及43cd2把referencing layer实际2d4写入新motion root；49bb按primitive.parent链重建stencil。"
        "现在kind3普通motion引用携带caller的实际EQUAL plane/ref到子sprite绘制。仅支持子层没有自己stencil owner的继承分支；"
        "同时有local/caller链，或kind3自身成为stencil owner时明确Err，不擅自按两个buffer交集替代linked scheduler。\n\n"
        "仍缺nested mask源、source2cc的ordinary color队列/范围生命周期、blank/group owner及mode1独立target/混合。"
        "通用stencil严格警告保留；因此上述真实像素代码仍不代表完整NEKO或native成功。"
        "声明9族不变，primary102/102/0沿用，60timed待最后统一验证，完整native新增0。\n\n"
        "首模型只读结构统计：" + json.dumps(dict(counts), ensure_ascii=False) + "；stencilType分布="
        + json.dumps(dict(kinds), ensure_ascii=False) + "。这些是资源结构数量，不是帧运行覆盖率。\n\n"
        "driver SHA256=" + driver.digest + "；各RVA原字节及源码哈希见JSON。"
        "仅rustfmt语法读取、Python只读PE/资源对象解析；没有编译、测试或执行Rust/VM/游戏/DLL/APK。"
        "脚本分类scripts/analysis，报告reports；无临时产物、资产发布或新增目录。\n", encoding="utf-8")
    current["stencil_own_batch"] = small
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

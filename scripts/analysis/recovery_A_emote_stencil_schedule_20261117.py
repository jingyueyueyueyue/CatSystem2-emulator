"""Targeted mode0 include/exclude pass contract; static bytes/source only."""
from datetime import datetime, timezone
import json

from recovery_A_20261116 import ROOT, Image, read_json, sha
from recovery_A_receiver_contracts_20261116 import block

DRIVER_SHA = "85eb459e94e74ab608f649eb7fb440f2526e90aaa1af7276ed01dc10018903ce"


def main():
    identity = read_json(ROOT / "reports/recovery_A_20261116_emote.json")
    driver = Image((ROOT / identity["binary_path"]).parent / "emotedriver.dll", DRIVER_SHA)
    order = [driver.u32(0x7CBC8), driver.u32(0x7CBCC)]
    if order != [1, 2]:
        raise ValueError("fixed stencil pass identity differs")
    prior = read_json(ROOT / "reports/recovery_A_20261117_emote_mesh_hierarchy_summary.json")
    remaining = prior["remaining_implementation_families"]
    for family in remaining:
        if family["id"] == "stencil_scheduler":
            family["reason"] = "local mode0 type5/type6 two-pass include/exclude in real raster source; joined/nested source chains, noncomposite geometry, mask-source color suppression and mode1 remain diagnostic"
    paths = ["simulator/crates/runtime/src/emote_cpu.rs", "simulator/crates/runtime/src/emote_stencil.rs",
             "scripts/analysis/recovery_A_emote_stencil_schedule_20261117.py"]
    summary = {
        "fixed_original_pass_table": order,
        "local_mode0_type5_type6_pass_planner_implemented": True,
        "clear_is_one_iff_chain_contains_exclusion": True,
        "positive_pass_advances_reference_once_per_owner": True,
        "negative_pass_all_owners_share_final_reference": True,
        "color_pass_keeps_positive_final_reference": True,
        "real_sprite_triangle_alpha_test_and_increment_decrement_connected": True,
        "source_motion_recursion_carries_actual_pass_operation": True,
        "strict_general_stencil_warning_preserved": True,
        "joined_nested_noncomposite_and_mode1_complete": False,
        "primary_pair_source_paths_inherited": 102, "primary_pair_explicit_source_blocked": 0,
        "remaining_implementation_family_count": len(remaining), "newly_closed_entire_families": [],
        "timed_labels_pending_final_combined_runtime_validation": 60,
        "completed_native_count_added": 0, "compiled": False, "tests_run": False,
        "Rust_execution": False, "game_or_DLL_executed": False, "full_NEKO_strict_pixel_claim": False,
    }
    spans = {
        "fixed_include_then_exclude_pass_table": (0x7CBC8, 8),
        "chain_scan_for_exclusion_and_initial_clear": (0x4FE5, 0x37),
        "two_pass_loop_and_stencil_reference": (0x5022, 0x66),
        "composite_list_branch": (0x5088, 0x4D),
        "noncomposite_owner_geometry_branch": (0x540B, 0x73),
        "chain_next_and_color_final_reference": (0x55D5, 0x32),
        "mode0_clear_value_and_enable": (0x577EF, 0x53),
        "mode0_alpha_GREATER64": (0x578A1, 0x30),
        "mode0_include_EQUAL_reference_wrappingINCR": (0x57A96, 0x44),
        "mode0_exclude_EQUAL_reference_wrappingDECR": (0x57BDE, 0x45),
        "mode0_color_EQUAL_KEEP": (0x57CCD, 0x93),
        "source_layer_marked_as_mask_member": (0x3A6BD, 0xD),
    }
    apis = {
        "internal_plan": "stencil::local_composite_plan(nearest_first_owner_indices,bindings) -> clear,color_reference,(owner,reference,operation)[]",
        "actual_backend": "RenderContext::literal_stencil_clip -> plan -> SpritePass::Mask(operation) -> textured triangle raster",
        "actual_source_motion": "stencil_source_motion(...,reference,operation) carries increment/decrement to every real primitive",
        "host_integration": "unchanged CpuEmoteBackend::render_rgba; strict warning prevents incomplete models reaching native success",
    }
    missing = [
        {"receiver": "RenderContext::motion/stencil_source_motion", "missing": "global linked owner propagation across referenced motions and nested mask sources; color descendants of kind3 must inherit the caller's actual chain"},
        {"receiver": "RenderContext::literal_stencil_clip", "PE_RVA": ["540b"], "missing": "noncomposite type1/type2 owner's actual own geometry; currently planner explicitly refuses"},
        {"receiver": "RenderContext::motion", "PE_RVA": ["3a6c3"], "missing": "mask source layer2cc lifetime and suppression in ordinary color emission; no silent strict promotion"},
        {"receiver": "stencil backend target lifecycle", "PE_RVA": ["57600", "578e0", "57ae0", "57c30"], "missing": "mode1 separate target allocation, size/projection, reverse/subtract blend and final texture composition"},
    ]
    payload = {"schema": "cs2-recovery-A-emote-stencil-schedule/v1", "task_date": "2026-11-17",
        "generated_at_utc": datetime.now(timezone.utc).isoformat(), "summary": summary, "APIs": apis,
        "driver": {"sha256": driver.digest, "address_kind": "RVA", "image_base": driver.base},
        "remaining_implementation_families": remaining, "specific_missing_receivers": missing,
        "driver_evidence": {name: block(driver, *span) for name, span in spans.items()},
        "sources": [{"path": p, "sha256": sha((ROOT / p).read_bytes())} for p in paths],
        "classification": "scripts/analysis + reports; no temporary products, new directories or main render_scene/C hooks/root README edits"}
    prefix = ROOT / "reports/recovery_A_20261117_emote_stencil_schedule"
    payload["modified_paths"] = paths + ["reports/" + prefix.name + ext for ext in (".json", ".md", "_summary.json")] + [
        "reports/recovery_A_20261117_emote_current_summary.json"]
    prefix.with_suffix(".json").write_text(json.dumps(payload, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    small = {k: payload[k] for k in ("schema", "task_date", "summary", "APIs", "remaining_implementation_families", "specific_missing_receivers", "sources", "classification")}
    prefix.with_name(prefix.name + "_summary.json").write_text(json.dumps(small, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    prefix.with_suffix(".md").write_text(
        "# A / mode0实际包含与排除 stencil pass（2026-11-17）\n\n"
        "本批修改emote_stencil.rs/emote_cpu.rs，直接接既有逐三角形mask raster。"
        "原7cbc8表为[1,2]；4ff0扫描整链，若存在low2==2则57600把stencil clear为1，否则0。"
        "第一pass按当前owner到parent顺序处理low2==1，每个owner用当前ref做INCR并增加期望ref；"
        "第二pass处理low2==2，所有owner都用同一个最终ref做DECR。color最终仍EQUAL该ref。"
        "相同owner的多个源图元由EQUAL限制只改一次，保留原alpha GREATER64。\n\n"
        "现在local_composite_plan输出真实clear/ref/operation，type5包含及type6排除均走"
        "SpritePass::Mask与同一纹理triangle sampler；source motion递归沿用实际INCR/DECR操作。"
        "没有把多个半透明图元先source-over成一个mask。\n\n"
        "整族未闭合：跨motion的owner链、nested源、非composite type1/2 own geometry、"
        "源层2cc普通color emission抑制和mode1独立目标/混合仍缺；通用stencil严格警告保留。"
        "因此不会把本地type5/6 preview当作完整NEKO或native成功。声明9族不变，primary102/102/0沿用，"
        "完整native新增0，60timed标签保留最终统一验证。\n\n"
        "JSON提供固定driver SHA256=" + driver.digest + "、RVA原字节、源码哈希及具体接收者。"
        "不编译、不测试、不执行Rust/VM/游戏/DLL/APK；仅rustfmt语法读取与Python只读PE分析。"
        "脚本分类scripts/analysis，报告reports；无临时产物、资产发布或新目录。\n", encoding="utf-8")
    current_path = ROOT / "reports/recovery_A_20261117_emote_current_summary.json"
    current = read_json(current_path)
    current["stencil_schedule_batch"] = small
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

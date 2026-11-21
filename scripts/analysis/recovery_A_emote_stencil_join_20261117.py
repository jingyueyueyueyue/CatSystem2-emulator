"""Fixed-driver local/caller linked stencil schedule evidence, static only."""
import json

from recovery_A_20261116 import ROOT, Image, read_json, sha
from recovery_A_receiver_contracts_20261116 import block


def main():
    identity = read_json(ROOT / "reports/recovery_A_20261116_emote.json")
    driver = Image((ROOT / identity["binary_path"]).parent / "emotedriver.dll",
                   "85eb459e94e74ab608f649eb7fb440f2526e90aaa1af7276ed01dc10018903ce")
    current_path = ROOT / "reports/recovery_A_20261117_emote_current_summary.json"
    current = read_json(current_path)
    remaining = current["mesh_join_batch"]["remaining_implementation_families"]
    for row in remaining:
        if row["id"] == "stencil_scheduler":
            row["reason"] = "local and caller type1/2/5/6 owners now form one nearest-first linked clear/increment/decrement/EQUAL plane across ordinary motion references; nested source masks, kind3 itself owning a mask, source2cc color/range ownership, blank/group variants and mode1 remain diagnostic"
    summary = {
        "local_and_caller_owner_scopes_joined_in_actual_link_order": True,
        "owner_geometry_retains_original_motion_scope": True,
        "single_clear_positive_negative_equal_schedule": True,
        "independent_clip_intersection_not_substituted": True,
        "global_chain_reference_accounts_for_all_positive_and_negative_owners": True,
        "real_alpha_tested_textured_mesh_triangles_feed_linked_plane": True,
        "kind3_own_stencil_owner_still_explicit_error": True,
        "nested_mask_source_still_diagnostic": True,
        "general_source_color_range_warning_preserved": True,
        "remaining_implementation_family_count": len(remaining),
        "closed_subbranches": ["stencil_scheduler/local_caller_linked_owners"],
        "newly_closed_entire_families": [], "primary_pair_source_paths_inherited": 102,
        "primary_pair_explicit_source_blocked": 0,
        "timed_labels_pending_final_combined_runtime_validation": 60,
        "completed_native_count_added": 0, "compiled": False, "tests_run": False,
        "target_execution": False, "full_NEKO_strict_pixel_claim": False,
    }
    apis = {"owner_scope": "StencilScope{owners,pending,bindings}; current motion nearest first, caller tails retained",
            "receiver": "RenderContext::motion passes scoped chain to child motion; literal_stencil_clip resolves each scope's actual source primitives",
            "plan": "stencil::linked_stencil_plan(kinds) -> one clear, ordered owner-indexed passes and final EQUAL reference",
            "pixels": "sprite -> SpritePass::Mask triangle raster -> CpuStencilBuffer -> SpritePass::Color EQUAL triangle raster",
            "host": "CpuEmoteBackend::render_rgba unchanged; no extra integration hook"}
    missing = [
        {"receiver": "RenderContext::stencil_source_motion", "missing": "nested source mask schedule, source2cc actual emitted color/range lifecycle"},
        {"receiver": "RenderContext::motion / stencil owner primitive", "missing": "referencing motion which is itself a stencil owner; blank/group primitive geometry"},
        {"receiver": "stencil target backend", "missing": "mode1 target/projection/blend/composition"},
    ]
    spans = {
        "local_parent_uses_active_owner_else_effective_tail": (0x402b0, 0x32),
        "caller_tail_transferred_to_new_motion_root": (0x41b09, 0x15),
        "second_reference_path_tail_transfer": (0x43cc9, 0x15),
        "primitive_link_from_effective_owner": (0x44b12, 0x27),
        "ordinary_primitive_link_from_same_effective_owner": (0x44de1, 0x20),
        "scan_full_chain_for_negative_owner_then_one_clear": (0x4fe5, 0x37),
        "positive_and_negative_pass_selectors": (0x5022, 0x72),
        "follow_owner_next_then_finish_equal_reference": (0x55d5, 0x32),
    }
    paths = ["simulator/crates/runtime/src/emote_cpu.rs", "simulator/crates/runtime/src/emote_stencil.rs",
             "scripts/analysis/recovery_A_emote_stencil_join_20261117.py"]
    payload = {"schema": "cs2-recovery-A-emote-stencil-join/v1", "task_date": "2026-11-17",
               "summary": summary, "APIs": apis, "remaining_implementation_families": remaining,
               "specific_missing_receivers": missing,
               "driver": {"sha256": driver.digest, "image_base": driver.base, "address_kind": "RVA"},
               "driver_evidence": {name: block(driver, *span) for name, span in spans.items()},
               "sources": [{"path": p, "sha256": sha((ROOT / p).read_bytes())} for p in paths],
               "classification": "scripts/analysis + reports; no temporary products/new directories/root README/main render_scene/native_tasks edits"}
    prefix = ROOT / "reports/recovery_A_20261117_emote_stencil_join"
    prefix.with_suffix(".json").write_text(json.dumps(payload, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    small = {key: payload[key] for key in ("schema", "task_date", "summary", "APIs", "remaining_implementation_families", "specific_missing_receivers", "sources", "classification")}
    prefix.with_name(prefix.name + "_summary.json").write_text(json.dumps(small, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    prefix.with_suffix(".md").write_text(
        "# A / local与caller的真实stencil链\n\n"
        "源码emote_cpu.rs/emote_stencil.rs已接通：StencilScope保存各motion自己的owner索引、pending图元与binding。"
        "普通kind3进入新motion时保留caller tail；local owners最近在先，再连接caller scopes。"
        "literal_stencil_clip不再把两张独立plane相交，而是从各scope取实际纹理/mesh源，送入同一"
        "linked_stencil_plan，按完整链计算一次clear、positive递增、negative递减及最终EQUAL引用值。"
        "颜色和mask仍用真实CPU triangle raster。\n\n"
        "PE链：402cd选择parent active owner或2d4 tail；41b12/43cd2传新motion root；"
        "44b22/44dea将effective owner.2dc写入primitive+78；4ff0扫描整个+78链是否有negative，"
        "5022/7cbc8=[1,2]分两pass，55d9遍历同一链，5602提交最终EQUAL。\n\n"
        "仍明确缺：nested mask source、kind3自身是owner的range、source2cc普通color/range生命周期、"
        "blank/group图元、mode1 target/projection/blend/composition。通用source color/range警告保留，"
        "不会把diagnostic preview冒充native成功。剩余声明9族，primary102/102、pair阻断0沿用，"
        "60timed最终统一验证；完整native新增0。\n\n"
        "driver SHA256=" + driver.digest + "；JSON保存RVA原字节/指令和源码SHA256。"
        "未编译、测试或执行Rust/VM/游戏/DLL/APK；仅rustfmt语法读取与静态PE分析。"
        "脚本已分类scripts/analysis，报告归reports；无临时文件、新目录、资产发布或主图形域修改。\n", encoding="utf-8")
    current["stencil_join_batch"] = small
    current["summary"]["remaining_strict_blockers"] = [row["reason"] for row in remaining]
    hashes = {row["path"]: row for row in current.get("source_hashes_current", [])}
    hashes.update({row["path"]: row for row in payload["sources"]})
    current["source_hashes_current"] = list(hashes.values())
    report = "reports/" + prefix.name + ".md"
    if report not in current.setdefault("reports", []):
        current["reports"].append(report)
    current_path.write_text(json.dumps(current, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(summary))


if __name__ == "__main__":
    main()

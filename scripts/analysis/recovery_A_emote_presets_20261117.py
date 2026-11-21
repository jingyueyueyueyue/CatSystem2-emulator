"""Fixed-PE clipper preset/source binding handoff; static analysis only."""
from datetime import datetime, timezone
import json

from recovery_A_20261116 import ROOT, Image, read_json, sha
from recovery_A_receiver_contracts_20261116 import block
from recovery_A_emote_host_20261117 import sections
from audit_runtime_evidence import kif_entries, blowfish_le_decrypt


def main():
    identity = read_json(ROOT / "reports/recovery_A_20261116_emote.json")
    exe = Image(ROOT / identity["binary_path"], identity["PE_sha256"])
    prior = read_json(ROOT / "reports/recovery_A_20261117_emote_intermediate_summary.json")
    remaining = list(prior["remaining_implementation_families"])
    for family in remaining:
        if family["id"] == "LPD_preset":
            family["reason"] = "explicit #clipper presets and automatic LPD receiver implemented; table without declared default and unmatched key remains diagnostic"
        if family["id"] == "sidecar_switch":
            family["reason"] = "existing LV/LPD evaluator-class reuse and external-sidecar to waveform lifetime remain diagnostic"
    config_inventory = []
    archive = exe.path.parent / "m2config.int"
    entries, _, cipher = kif_entries(archive, 0x0F6BB0AE)
    with archive.open("rb") as handle:
        for member in (b"ca01.inc", b"body.dif"):
            entry = next(e for e in entries if e["name"] == member)
            handle.seek(entry["offset"])
            data = handle.read(entry["size"])
            if cipher: data = blowfish_le_decrypt(cipher, data)
            rows = sections(data)
            config_inventory.append({"member": member.decode("ascii"), "decoded_sha256": sha(data),
                "clipper_records": len(rows.get("clipper", [])),
                "clipper_default_records": len(rows.get("clipper_default", [])),
                "LPD_calls_executed": False})
    summary = {
        "primary_pairs_inherited_not_rescanned": 102, "primary_blocked_pairs": 0,
        "closed_implementation_families_this_combined_delivery": ["intermediate_chain", "compound_dimensions"],
        "remaining_implementation_family_count": len(remaining),
        "remaining_family_count_scope": "prior declared implementation families, not all native/functions or game completion",
        "LPD_preset_family_partially_implemented_not_closed": True,
        "correct_PE_section_names": ["clipper_default", "clipper"],
        "previous_lipper_report_spelling_corrected_here": True,
        "profile_default_last_assignment_and_exact_override_first_insertion": True,
        "automatic_native924_LPD_binding_source_connected": True,
        "each_dimension_pattern_owns_actual_origin_and_radius": True,
        "LV_ignores_clipper_uses_constructor_origin_zero": True,
        "all_curves_validate_before_live_owner_publish": True,
        "curve_sharing_identity_includes_preset_and_origin": True,
        "implicit_profile_default_on_explicit_table_miss_unrecovered": True,
        "existing_LV_LPD_class_switch_explicit_diagnostic": True,
        "existing_sidecar_to_waveform_explicit_diagnostic": True,
        "timed_labels_pending_final_combined_runtime_validation": 60,
        "pending_runtime_validation_is_not_missing_source": True,
        "full_NEKO_strict_pixel_claim": False, "completed_native_count_added": 0,
        "compiled": False, "tests_run": False, "Rust_execution": False, "game_or_DLL_executed": False,
    }
    paths = ["simulator/crates/runtime/src/emote.rs"] + [
        "simulator/crates/runtime/src/emote_" + stem + ".rs" for stem in (
            "m2presets", "m2chain", "m2banks", "m2live", "m2values", "m2timed", "m2lip", "cpu")]
    paths += ["simulator/crates/desktop/src/emote_native.rs",
              "scripts/analysis/recovery_A_emote_intermediate_20261117.py",
              "scripts/analysis/recovery_A_emote_presets_20261117.py"]
    spans = {
        "actual_default_section_name": (0x18BB43, 7),
        "default_six_weights_two_integer_fields": (0x18BBEC, 0xE7),
        "actual_override_section_name": (0x18BD58, 7),
        "override_dimension_pattern_six_weights_origin_radius": (0x18BDFB, 0x133),
        "override_first_insert_not_overwrite": (0x185F2C, 0x85),
        "exact_key_or_existing_default_record": (0x186042, 0x57),
        "LPD_all_records_then_real_profile_preset": (0x1752BB, 0x30),
        "LPD_apply_weights_origin_radius": (0x1753E1, 0x36),
        "LPD_existing_evaluator_not_reconstructed": (0x184EE0, 0x4B),
        "LPD_actual_constructor_fields": (0x1A19E0, 0x72),
        "LV_constructor_and_open_without_profile_overrides": (0x1754A1, 0xB2),
        "weights_virtual10": (0x1AB3B0, 0x32),
        "origin_virtual14": (0x1AB3F0, 0x12),
        "radius_virtual18": (0x1AB410, 0x12),
        "new_resource_releases_driver_before_import": (0x17592E, 0x36),
        "profile_change_sets_previous_draw_flag": (0x171A8D, 0x21),
        "driver_update_clears_previous_draw_flag": (0x1741DC, 0x1E),
    }
    apis = {
        "unchanged_906_entry": "EmoteServices::import_m2_resource",
        "profile_sidecar_entry": "EmoteServices::bind_m2_profile_lip_sidecar(task,bytes,CpuM2LipSidecarKind) -> bound_count",
        "actual_host_route": "native924 / command540 -> LoggingHost::emote_link_legacy_sound_lip -> profile sidecar entry ->928(1)",
        "time_source": "existing EmoteTick.lip_elapsed from actual selected SOUND task219; per-record origin subtracted by curve sampler",
        "manual_explicit_entry_preserved": "EmoteServices::bind_m2_lip_sidecar(task,bytes,kind,origin,bindings)",
        "independent_parser": "emote_m2presets.rs::CpuM2ClipperProfile::from_expanded/select",
        "transaction": "m2presets::bind_sidecar_records -> cloned live owner -> publish after full success",
    }
    payload = {"schema": "cs2-recovery-A-emote-presets/v1", "task_date": "2026-11-17",
        "generated_at_utc": datetime.now(timezone.utc).isoformat(), "summary": summary, "APIs": apis,
        "EXE": {"path": identity["binary_path"], "sha256": exe.digest},
        "actual_config_inventory": config_inventory,
        "remaining_implementation_families": remaining,
        "specific_missing_receivers": [
            {"family": "model_surface_fade", "source": "emote_cpu.rs::import_m2_resource/render_rgba", "PE": ["171a00", "175900", "174040"],
             "missing": "previous rendered surface ownership, exact blending weight/duration and multi-player retirement; capture flags alone are not blending semantics"},
            {"family": "LPD_preset", "source": "emote_m2presets.rs::CpuM2ClipperProfile::select", "PE": ["185fe0"],
             "missing": "implicit default record initialization when explicit clipper table misses and no clipper_default is declared"},
            {"family": "sidecar_switch", "source": "emote_m2live.rs::bind_lip_curve; desktop/emote_native.rs::emote_link_legacy_sound_lip", "PE": ["184ee0", "1754a1"],
             "missing": "existing evaluator virtual class retention/release policy; new sidecar class is not silently substituted"},
            {"family": "mesh_hierarchy", "source": "emote_cpu.rs::RenderContext::layer", "missing": "meshCombine hierarchy, cc/skin coefficient ownership, inherited subdivision without own transform"},
            {"family": "stencil_scheduler", "source": "emote_cpu.rs::literal_stencil_clip/stencil_source_motion", "missing": "joined/inherited and nested-source stencil pass order, non-type5 and mode1 branches"}],
        "EXE_evidence": {key: block(exe, *span) for key, span in spans.items()},
        "sources": [{"path": p, "sha256": sha((ROOT / p).read_bytes())} for p in paths],
        "classification": "scripts/analysis + reports; no temporary products/game assets/new directories; no root README/lib/render_scene edits"}
    prefix = ROOT / "reports/recovery_A_20261117_emote_presets"
    payload["modified_paths"] = paths + [str(prefix.with_suffix(ext).relative_to(ROOT)) for ext in (".json", ".md")] + [
        "reports/recovery_A_20261117_emote_presets_summary.json", "reports/recovery_A_20261117_emote_current_summary.json"]
    prefix.with_suffix(".json").write_text(json.dumps(payload, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    small = {k: payload[k] for k in ("schema", "task_date", "summary", "APIs", "remaining_implementation_families", "specific_missing_receivers", "classification")}
    prefix.with_name(prefix.name + "_summary.json").write_text(json.dumps(small, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    prefix.with_suffix(".md").write_text(
        "# A / 交付审查：intermediate、compound 与 LPD preset（2026-11-17）\n\n"
        "已闭合前批声明的intermediate_chain/compound_dimensions源码分支，11→9族；102/102静态pair沿用，明确pair阻断0。"
        "60 timed标签只待最终统一验证，不算缺源码；新增完整native完成计数0。\n\n"
        "新增emote_m2presets.rs，实际读取#clipper_default（6个f32权重、origin、radius）及#clipper（dimension、pattern、同8项）；"
        "185ec0首次exact插入保留，185fe0按真实dimension/pattern查找，否则返回已有default record。旧报告lipper名称已在此更正，历史报告未伪造重写。\n\n"
        "新增EmoteServices::bind_m2_profile_lip_sidecar；desktop native924已直接接入，LPD按每个live bank的真实preset/origin建立曲线，"
        "全部验证后原子发布owner。共享Arc曲线的身份同时含preset和origin，避免不同origin误共享。LV保持175460构造参数/origin0，不套用LPD覆盖项。"
        "现有实际SOUND tick→curve→shared absolute value→timed receiver→CPU sampler链保留。\n\n"
        "尚缺的具体接收者：显式clipper表未声明default且查找落空时，185fe0返回的隐式profile default初始化仍未恢复；"
        "该分支明确诊断。184ee0/1754a1仅在b4为空时构造，已有LV/LPD类别切换及sidecar→waveform的复用/释放仍诊断。"
        "因此LPD_preset族本次仅部分补齐，剩余仍为9。实际ca01.inc/body.dif的preset行数见JSON，只读资源计数不是运行验收。\n\n"
        "surface：171a8d设置9c旧draw标志，17592e释放driver再导入，1741dc清标志；目前不足以确定真实blend权重/时长/退休策略，"
        "没有据此制造线性交叉淡化。具体缺口仍在emote_cpu::import_m2_resource/render_rgba。"
        "mesh缺meshCombine/cc层级，stencil缺joined/inherited/nested源调度；这些严格像素诊断继续保留。\n\n"
        "修改路径、固定PE SHA256与RVA片段、当前源码哈希均在JSON。未改main render_scene、根README、C工厂/hooks，未重复Emocon/窗口native。"
        "源码仅rustfmt语法读取；不编译、不测试、不运行Rust/VM/游戏/DLL/APK。脚本归scripts/analysis，报告归reports；无临时产物和资产发布。\n",
        encoding="utf-8")
    current_path = ROOT / "reports/recovery_A_20261117_emote_current_summary.json"
    current = read_json(current_path)
    current["presets_batch"] = small
    current["summary"]["remaining_implementation_family_count"] = len(remaining)
    current["summary"]["remaining_strict_blockers"] = [f["reason"] for f in remaining]
    current["summary"]["backend_native_branches"]["924"] = (
        "owned LV and actual PCM/PVL waveform-flow plus explicit clipper LPD profile receiver; implicit profile default/table miss and evaluator-class/sidecar switching remain")
    current["source_hashes_current"] = payload["sources"]
    report = str(prefix.with_suffix(".md").relative_to(ROOT)).replace("\\", "/")
    if report not in current.setdefault("reports", []): current["reports"].append(report)
    current_path.write_text(json.dumps(current, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(summary, ensure_ascii=True))


if __name__ == "__main__": main()

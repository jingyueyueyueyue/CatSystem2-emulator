"""Fixed PE destructive lip reopen evidence; no Rust/runtime execution."""
from datetime import datetime, timezone
import json

from recovery_A_20261116 import ROOT, Image, read_json, sha
from recovery_A_receiver_contracts_20261116 import block


def main():
    identity = read_json(ROOT / "reports/recovery_A_20261116_emote.json")
    exe = Image(ROOT / identity["binary_path"], identity["PE_sha256"])
    driver_sha = "85eb459e94e74ab608f649eb7fb440f2526e90aaa1af7276ed01dc10018903ce"
    driver = Image((ROOT / identity["binary_path"]).parent / "emotedriver.dll", driver_sha)
    current_path = ROOT / "reports/recovery_A_20261117_emote_current_summary.json"
    current = read_json(current_path)
    remaining = current["stencil_schedule_batch"]["remaining_implementation_families"]
    for row in remaining:
        if row["id"] == "sidecar_switch":
            row["reason"] = "destructive failed reopen now clears old arrays, retains virtual class/parameters and phase timestamps, and persists a strict diagnostic; native continuation after rejected or unsafe input remains deliberately unclaimed"
    paths = [
        "simulator/crates/runtime/src/emote_m2lip.rs",
        "simulator/crates/runtime/src/emote_m2presets.rs",
        "simulator/crates/runtime/src/emote_m2live.rs",
        "simulator/crates/runtime/src/emote_cpu.rs",
        "scripts/analysis/recovery_A_emote_destructive_open_20261117.py",
    ]
    summary = {
        "failed_reopen_old_arrays_cleared_in_source": True,
        "retained_virtual_receiver_class_and_post_open_parameters": True,
        "failed_open_preserves_phase_and_flow_timestamps": True,
        "batch_continues_other_targets_and_returns_first_error": True,
        "failed_receiver_diagnostic_persists_in_tick_and_strict_render": True,
        "successful_reopen_replaces_failure_diagnostic": True,
        "new_LV_receiver_uses_constructor_radius_before_post_open_setter": True,
        "native_unsafe_input_or_failure_continuation_equivalence_claim": False,
        "primary_pair_source_paths_inherited": 102,
        "primary_pair_explicit_source_blocked": 0,
        "remaining_implementation_family_count": len(remaining),
        "newly_closed_entire_families": [],
        "timed_labels_pending_final_combined_runtime_validation": 60,
        "completed_native_count_added": 0,
        "compiled": False, "tests_run": False, "Rust_execution": False,
        "game_or_DLL_executed": False, "full_NEKO_strict_pixel_claim": False,
    }
    apis = {
        "unchanged_import": "EmoteServices::bind_m2_lip_sidecar/bind_m2_profile_lip_sidecar",
        "lifetime": "m2presets::bind_sidecar_records publishes destructive mutations even when returning Err",
        "inspection": "playback_info().m2_owner.failed_sidecar_evaluators; CpuM2LipCurve::reopen_error()",
        "strict_tick": "CpuM2LiveOwner::advance_lip -> CpuM2LipCurve::sample_checked",
        "strict_pixels": "CpuEmoteBackend::render_preview carries owner.sidecar_diagnostics; render_rgba refuses them",
        "integration": "existing host entry; no main/native_tasks/render_scene hooks added",
    }
    missing = [
        {"receiver": "CpuM2ClipperProfile::select", "PE_RVA": ["186cf9", "186089"],
         "missing": "profile miss returns uninitialized default bytes; no deterministic preset can be invented"},
        {"receiver": "CpuM2Lpd::parse / bind_sidecar_records", "PE_RVA": ["1ab283", "184f1e", "1755bd"],
         "missing": "native only compares length against wrapped count*32, omits the20-byte header in this guard, and ignores per-record open failures at outer return; portable parser rejects unsafe/nonfinite/unsupported data and keeps an explicit persistent diagnostic"},
        {"receiver": "RenderContext::motion / draw queue", "driver_RVA": ["3a6c3", "44ae6", "44b0c", "45000"],
         "missing": "2cc is not a direct skip-whole-source test: it changes referenced-motion range capture and composite primitive collection; downstream ordinary color suppression/join ownership still requires recovery"},
    ]
    spans = {
        "LPD_constructor": (0x1A19E0, 0x72),
        "LPD_destructive_open_and_second_clear_on_failure": (0x1A1A90, 0x67),
        "LPD_clear_leaves_class_and_parameters": (0x1A1D20, 0x70),
        "LPD_header_reader_and_unsafe_length_guard": (0x1AB250, 0x73),
        "LPD_empty_sampler_returns_begin": (0x1A1B00, 0x57),
        "LV_clear_before_import": (0x1A1DF0, 0x46),
        "LV_empty_sampler_returns_begin": (0x1A1F00, 0x26),
        "failed_open_skips_flow_reset_then_continues_map": (0x184F08, 0x7D),
        "LPD_post_open_parameter_setters": (0x1753E1, 0x36),
        "LV_outer_returns_one_after_per_record_failures": (0x175538, 0x8F),
    }
    driver_spans = {
        "mark_source_layer2cc": (0x3A6BD, 0xD),
        "kind3_mask_member_changes_range_collection": (0x44AC1, 0xB0),
        "composite_collects_only_emitted_sources": (0x45000, 0x42),
    }
    prefix = ROOT / "reports/recovery_A_20261117_emote_destructive_open"
    payload = {
        "schema": "cs2-recovery-A-emote-destructive-open/v1", "task_date": "2026-11-17",
        "generated_at_utc": datetime.now(timezone.utc).isoformat(),
        "summary": summary, "APIs": apis,
        "remaining_implementation_families": remaining,
        "specific_missing_receivers": missing,
        "EXE": {"path": identity["binary_path"], "sha256": exe.digest, "image_base": exe.base, "address_kind": "RVA"},
        "driver": {"sha256": driver.digest, "image_base": driver.base, "address_kind": "RVA"},
        "EXE_evidence": {name: block(exe, *span) for name, span in spans.items()},
        "driver_evidence": {name: block(driver, *span) for name, span in driver_spans.items()},
        "sources": [{"path": p, "sha256": sha((ROOT / p).read_bytes())} for p in paths],
        "classification": "scripts/analysis + reports; no temporary products, new directories, asset publication, main render_scene/native_tasks/root README edits",
    }
    payload["modified_paths"] = paths + ["reports/" + prefix.name + ext for ext in (".json", ".md", "_summary.json")] + ["reports/recovery_A_20261117_emote_current_summary.json"]
    prefix.with_suffix(".json").write_text(json.dumps(payload, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    small = {key: payload[key] for key in ("schema", "task_date", "summary", "APIs", "remaining_implementation_families", "specific_missing_receivers", "sources", "classification")}
    prefix.with_name(prefix.name + "_summary.json").write_text(json.dumps(small, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    prefix.with_suffix(".md").write_text(
        "# A / lip破坏性reopen接收者（2026-11-17）\n\n"
        "实际源码修改emote_m2lip.rs、emote_m2presets.rs、emote_m2live.rs、emote_cpu.rs。"
        "原1a1a90在解析前调用1a1d20清空旧LPD及曲线数组，失败后再次清空；虚表、参数、origin保留。"
        "184f1e失败跳过184f90流重置，随后继续map。1753e1仍会执行LPD命令的后置参数setter。\n\n"
        "本批不再因事务回滚保留旧声音的口型。每个失败目标保存空数组、原receiver class和新后置参数，"
        "保留phase/flow时间戳；其他目标继续，最终返回首个诊断。诊断保存在实际owner，后续tick及strict render均可见，"
        "直到该目标成功reopen。新LV receiver先使用真实构造radius1平滑，再应用后置参数。"
        "接口沿用bind_m2_lip_sidecar/bind_m2_profile_lip_sidecar；新增failed_sidecar_evaluators及reopen_error检查。\n\n"
        "边界：原1ab283只比较长度与DWORD回绕count*32，未加20字节header；外层又会忽略单条open失败而返回1。"
        "本实现继续拒绝越界、非有限值及未支持输入，并持久报告错误，不声称复现原程序的越界读取或错误后静默继续。"
        "未声明profile default仍是未初始化字节，不能用构造默认值填补。9个声明族不变，native新增0，"
        "102/102 primary源码路径及60timed最终统一验证沿用。\n\n"
        "下一stencil调查已确认：3a6c3设置的source2cc在44ae6/44b0c改变kind3的队列range收集，"
        "不能直接推成跳过整层。45000只收集已emitted源；下游color/join消费者仍待定向恢复。\n\n"
        "固定EXE SHA256=" + exe.digest + "；driver SHA256=" + driver.digest + "。"
        "完整JSON包含各RVA原字节/指令/哈希及源码哈希。仅静态读取与rustfmt语法读取；未编译、测试或执行目标代码。"
        "脚本已归scripts/analysis，报告归reports；本批无临时产物或新增目录。\n", encoding="utf-8")
    current["destructive_open_batch"] = small
    current["summary"]["remaining_implementation_family_count"] = len(remaining)
    current["summary"]["remaining_strict_blockers"] = [row["reason"] for row in remaining]
    current["summary"]["backend_native_branches"]["924"] = "retained virtual receiver, actual presets/origin, live SOUND and destructive reopen mutations; rejected imports retain explicit diagnostics"
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

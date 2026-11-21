"""Small Emote source-review handoff. Reads summaries/source bytes, never CFGs."""
from datetime import datetime, timezone
import json

from recovery_A_20261116 import ROOT, read_json, sha


def main():
    identity = read_json(ROOT / "reports/recovery_A_20261116_emote.json")
    batches = [
        ("evaluator", ["emote_m2lip.rs", "emote_m2presets.rs", "emote_m2live.rs", "emote_m2wave.rs"],
         ["EXE184ee0", "EXE1754a1", "EXE174610", "EXE174984"],
         "retained LV/LPD virtual receiver, actual SOUND window/clock switch, stop/base restore and sticky quiet-tail"),
        ("mesh_hierarchy", ["emote_cpu.rs", "emote_mesh.rs"],
         ["driver3f876", "driver3f880", "driver3fef2"],
         "raw BP addition, consumed-parent retirement and real inherited subdivision"),
        ("stencil_schedule", ["emote_cpu.rs", "emote_stencil.rs"],
         ["driver7cbc8", "driver4ff0", "driver5022", "driver55d5", "driver57a96", "driver57bde"],
         "mode0 type5/6 include/exclude passes on actual textured triangle masks"),
        ("destructive_open", ["emote_m2lip.rs", "emote_m2presets.rs", "emote_m2live.rs", "emote_cpu.rs"],
         ["EXE1a1a90", "EXE1a1d20", "EXE184f1e", "EXE1753e1"],
         "failed reopen clears stale curves, preserves receiver/phase, persists explicit diagnostic; successful retry replaces fault"),
        ("stencil_own", ["emote_cpu.rs", "emote_stencil.rs"],
         ["driver540b", "driver558b", "driver41b12", "driver43cd2", "driver49bb"],
         "type1/2 own-sprite geometry; caller-only EQUAL clip passes to referenced motion child sprites"),
        ("mesh_sync", ["emote_cpu.rs"],
         ["driver35f5b", "driver3f6f0", "driver3f5db"],
         "meshSyncChildMask bit8 gates child propagation independently of own BP; no fixed16 grid fallback"),
        ("mesh_anchor", ["emote_cpu.rs"],
         ["driver3f90e", "driver3ffdd", "driver400c3", "driver40100", "driver401d2"],
         "c1 barrier keeps skipped ancestors for anchor-only deformation; uniform delta enters real color/stencil vertices"),
        ("mesh_join", ["emote_cpu.rs"],
         ["driver41afd", "driver43cbd", "driver3f5db", "driver3f8ba"],
         "effective mesh parent transfers across ordinary and stencil-source motion roots; own BP/c1 and physical raw BP trees remain separate"),
        ("stencil_join", ["emote_cpu.rs", "emote_stencil.rs"],
         ["driver402cd", "driver41b12", "driver44b22", "driver4ff0", "driver55d9"],
         "local/caller owner scopes enter one linked clear/increment/decrement/EQUAL triangle-mask schedule"),
    ]
    rows = []
    for suffix, sources, anchors, behavior in batches:
        prefix = "reports/recovery_A_20261117_emote_" + suffix
        small = read_json(ROOT / (prefix + "_summary.json"))
        rows.append({"batch": suffix, "summary_path": prefix + "_summary.json", "report_path": prefix + ".md",
                     "bounded_PE_evidence_path": prefix + ".json", "PE_RVA_anchors": anchors,
                     "actual_behavior_in_source": behavior, "runtime_source_names": sources,
                     "compiled": small["summary"].get("compiled", False),
                     "tests_run": small["summary"].get("tests_run", False)})
    final = read_json(ROOT / "reports/recovery_A_20261117_emote_stencil_join_summary.json")
    runtime_names = sorted({p for _, sources, _, _ in batches for p in sources})
    paths = ["simulator/crates/runtime/src/" + p for p in runtime_names]
    paths.append("simulator/crates/desktop/src/emote_native.rs")
    paths.extend("scripts/analysis/recovery_A_emote_" + suffix + "_20261117.py" for suffix, *_ in batches)
    paths.append("scripts/analysis/recovery_A_emote_handoff_20261117.py")
    missing = [
        {"family": "model_surface_fade", "receiver": "CpuEmoteBackend::import_m2_resource/render_rgba",
         "missing": "old-model surface ownership, old/new weight, retirement and full manager composition; capture/projection alone is not a recovered time fade"},
        {"family": "secondary_weights", "receiver": "CpuM2LiveOwner secondary smooth/weighted owner",
         "missing": "actual secondary live offsets and weighted history"},
        {"family": "material_force", "receiver": "CpuM2LiveOwner material/inertia force update",
         "missing": "complete active material/force production; retained configuration is not evaluation"},
        {"family": "LPD_preset", "receiver": "CpuM2ClipperProfile::select", "kind": "indeterminate_original_state",
         "missing": "undeclared default is uninitialized original allocation; explicit diagnostic, not an invented constant"},
        {"family": "sidecar_switch", "receiver": "bind_sidecar_records / strict sidecar importer", "kind": "intentional_strict_boundary",
         "missing": "native ignores rejected open and has an unsafe length check; cleared-state side effects implemented, unsafe/native failure continuation not claimed"},
        {"family": "waveform_nondefault_quiet", "receiver": "CpuM2Waveform::quiet_scan_inhibited",
         "missing": "external nonzero node78 producer; consumer/clear/sticky tail implemented"},
        {"family": "graph_nonfinite_domain", "receiver": "M2 inter-segment graph pow/error path",
         "missing": "nonfinite domain and original CRT error outputs; finite graph path implemented"},
        {"family": "mesh_hierarchy", "receiver": "RenderContext::motion/stencil_source_motion; RenderContext::layer",
         "missing": "cc/skin, other mask bits and meshTransform variants; cross-motion effective parent transfer is now in source"},
        {"family": "stencil_scheduler", "receiver": "RenderContext::motion/literal_stencil_clip/stencil_source_motion and target backend",
         "missing": "nested source masks, kind3 itself owning a mask, source2cc ordinary color/range lifetime, blank/group variants and mode1 target/projection/blend/composition; ordinary local/caller chain now in source"},
    ]
    payload = {
        "schema": "cs2-recovery-A-emote-source-review/v1", "task_date": "2026-11-17",
        "generated_at_utc": datetime.now(timezone.utc).isoformat(),
        "summary": {"primary_pair_source_paths_inherited": 102, "primary_explicit_pair_blocked": 0,
                    "remaining_declared_implementation_families": len(missing),
                    "timed_labels_pending_final_runtime_validation": 60,
                    "pending_validation_is_not_missing_source": True, "integrated_completed_native_count_added": 0,
                    "full_NEKO_strict_pixel_claim": False, "compiled": False, "tests_run": False,
                    "Rust_VM_game_DLL_APK_execution": False,
                    "new_hooks_required_for_these_batches": False,
                    "source_hashes_are_current_bytes_not_compilation_or_semantic_verification": True},
        "PE_identity": {"EXE_sha256": identity["PE_sha256"], "file_version": "2.6.1.76",
                        "driver_sha256": "85eb459e94e74ab608f649eb7fb440f2526e90aaa1af7276ed01dc10018903ce",
                        "all_addresses_RVA": True},
        "batches": rows, "remaining_receivers": missing,
        "latest_remaining_family_descriptions": final["remaining_implementation_families"],
        "host_APIs": ["EmoteServices::import_m2_resource/play_timeline/advance_with_random",
                      "EmoteServices::bind_m2_profile_lip_sidecar/bind_m2_lip_sidecar",
                      "playback_info().m2_owner.failed_sidecar_evaluators", "CpuM2LipCurve::reopen_error()",
                      "CpuEmoteBackend::render_rgba (strict); render_preview is diagnostic only"],
        "current_source_and_script_hashes": [{"path": p, "sha256": sha((ROOT / p).read_bytes())} for p in paths],
        "classification": "functional scripts in scripts/analysis, reports in reports; no temporary products/new directories or root README/main render_scene/native_tasks edits",
        "historical_report_note": "Earlier batch hashes are stage snapshots; current hashes above do not retroactively turn earlier evidence into runtime verification.",
    }
    prefix = ROOT / "reports/recovery_A_20261117_emote_review_summary"
    prefix.with_suffix(".json").write_text(json.dumps(payload, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    lines = ["# A / 本轮Emote源码审查交付（2026-11-17）", "",
             "已落盘实际接收者与逐批固定PE证据如下；主宿主沿用已有API，无需增加本批hook。未编译、测试或执行目标代码。", "",
             "| 批次 | 实际源码行为 | 可核对RVA | 报告 |", "| --- | --- | --- | --- |"]
    for row in rows:
        lines.append("| " + row["batch"] + " | " + row["actual_behavior_in_source"] + " | " + ", ".join(row["PE_RVA_anchors"]) + " | " + row["report_path"] + " |")
    lines.extend(["", "原版身份：EXE SHA256=" + identity["PE_sha256"] + "，file_version2.6.1.76；driver SHA256=" + payload["PE_identity"]["driver_sha256"] + "。均为RVA。", "",
                  "源码路径（本轮累计；哈希见小JSON）：", ""])
    lines.extend("- `" + path + "`" for path in paths)
    lines.extend(["", "真实仍缺接收者：", ""])
    lines.extend("- " + row["family"] + " / `" + row["receiver"] + "`：" + row["missing"] for row in missing)
    lines.extend(["", "计数：primary102/102源码路径、明确pair阻断0沿用；声明剩余9族。60timed标签只待最终统一运行验证，不算缺源码；完整native新增0，未声明完整NEKO像素兼容。", "",
                  "LPD失败reopen的实际清空副作用及持久诊断已实现，不能再沿用此前“事务错误保留旧曲线”的描述；仍明确保留原版unsafe/failure continuation边界。", "",
                  "检查入口：bind_m2_profile_lip_sidecar/bind_m2_lip_sidecar；playback_info().m2_owner.failed_sidecar_evaluators；CpuM2LipCurve::reopen_error()。实际绘制仍走render_rgba，diagnostic preview不作为native成功。", "",
                  "前批报告哈希是阶段快照，不能反向当作当前源码/运行验证；本小JSON重新记录当前源码字节。脚本已分类scripts/analysis，报告归reports；无临时产物、新目录、资源发布或主代理图形域改动。", ""])
    prefix.with_suffix(".md").write_text("\n".join(lines), encoding="utf-8")
    print(json.dumps({"report": str(prefix.relative_to(ROOT)) + ".json", "summary": payload["summary"], "source_and_script_count": len(paths)}, ensure_ascii=True))


if __name__ == "__main__":
    main()

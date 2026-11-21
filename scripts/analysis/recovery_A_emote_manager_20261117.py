"""Targeted M2 manager/live-bank recovery; never builds or executes the engine."""
from __future__ import annotations
from datetime import datetime, timezone
import json
from recovery_A_20261116 import ROOT, Image, read_json, sha
from recovery_A_receiver_contracts_20261116 import block
from recovery_A_emote_host_20261117 import sections
from audit_runtime_evidence import kif_entries, blowfish_le_decrypt


def main():
    prior = read_json(ROOT / "reports/recovery_A_20261116_emote.json")
    exe = Image(ROOT / prior["binary_path"], prior["PE_sha256"])
    driver = Image(exe.path.parent / "emotedriver.dll",
                   "85eb459e94e74ab608f649eb7fb440f2526e90aaa1af7276ed01dc10018903ce")
    sources = [
        "simulator/crates/runtime/src/emote.rs",
        "simulator/crates/runtime/src/emote_cpu.rs",
        "simulator/crates/runtime/src/emote_m2banks.rs",
        "simulator/crates/runtime/src/emote_m2chain.rs",
        "simulator/crates/runtime/src/emote_m2live.rs",
        "simulator/crates/runtime/src/emote_m2physics.rs",
        "simulator/crates/desktop/src/emote_native.rs",
        "simulator/crates/desktop/src/lib.rs",
        "scripts/analysis/recovery_A_emote_manager_20261117.py",
    ]
    task_source = (ROOT / "simulator/crates/desktop/src/native_tasks.rs").read_text(encoding="utf-8")
    hooks = {name: f"{name}(" in task_source for name in [
        "emote_register_task_hook", "emote_update_task_hook", "emote_release_task_hook"
    ]}
    spans = {
        "ordinary906_12byte_wrapper": (0x24D2B0, 0x3F),
        "M2Plane906_fixed_internal_selector0": (0x23880E, 0x18),
        "task_id_manager_key": (0x17D425, 0x26),
        "inner_selector_vector_lookup": (0x17C2D0, 0x5B),
        "inner_player_ctor_defaults": (0x170F4E, 0x36),
        "resource_name_profile_lookup": (0x17D36D, 0x49),
        "default_variables_and_bank_rebuild": (0x171E10, 0x6C),
        "A_deactivate_then_selected_key": (0x184AF2, 0x19),
        "A_activate_selected_key": (0x184BAE, 0x17),
        "B_activate_selected_key": (0x184C9E, 0x1E),
        "blink_rng_schedule": (0x183F60, 0x34),
        "blink_uint_time_and_triangle": (0x184004, 0x7F),
        "default_scene_flags": (0x1803D1, 0x31),
        "material_integrator": (0x194787, 0xA8),
        "material_scaled_output": (0x1948E0, 0x25),
        "controller927_zero_real_reset": (0x174C61, 0x25),
        "907_reset_actual_banks": (0x1744CE, 0x40),
        "driver_current_missing_is_zero": (0x18365C, 0x29),
    }
    dll_spans = {"driver_variable_missing_default": (0x3396B, 0x21)}
    archive = exe.path.parent / "m2config.int"
    entries, _, cipher = kif_entries(archive, 0x0F6BB0AE)
    entry = next(e for e in entries if e["name"] == b"ca01.inc")
    with archive.open("rb") as handle:
        handle.seek(entry["offset"])
        bytes_ = handle.read(entry["size"])
    bytes_ = blowfish_le_decrypt(cipher, bytes_) if cipher else bytes_
    rows = sections(bytes_)
    patterns = {}
    current_pattern = None
    for row in rows["pattern"]:
        if row[0].startswith("%"):
            current_pattern = (int(row[0][1:]) - 1, row[1])
            patterns.setdefault(current_pattern, [])
        else:
            patterns[current_pattern].append((row[0], float(row[1])))
    control_inventory = {
        "resource": "m2config.int::ca01.inc",
        "decoded_sha256": sha(bytes_),
        "pattern_keys": len(patterns),
        "blink_dimension_count": len({int(r[0]) for r in rows["blink"]}),
        "blink_record_count": len(rows["blink"]),
        "lip_dimension_count": len({int(r[0]) for r in rows["lip_table"]}),
        "lip_record_count": len(rows["lip_table"]),
        "material_count": len(rows["material"]),
        "configured_blink_pattern_keys": [
            {"dimension_zero_based": int(r[0])-1, "pattern": r[1],
             "pattern_first_variable": patterns[(int(r[0])-1, r[1])][0][0],
             "initial_value": patterns[(int(r[0])-1, r[1])][0][1],
             "begin": float(r[2]), "end": float(r[3]), "duration": int(r[4]),
             "wait_min": int(r[5]), "wait_max": int(r[6])}
            for r in rows["blink"]
        ],
        "inventory_is_not_executed_call_inputs": True,
    }
    summary = {
        "task_hooks_observed": hooks,
        "manager_identity": "(actual_task_id, internal_selector); M2Plane906 fixes selector0",
        "native906_first_real_PFL_PSZ_load_commits": True,
        "native906_exact_same_selection_reuses_live_state": True,
        "native906_zero_transition_pattern_change_commits": True,
        "native906_nonzero_reused_pattern_chain_complete": False,
        "native906_nonzero_fix_branch_in_source": True,
        "native906_timeline_only_reuse_preserves_live_owner": True,
        "native906_replacing_loaded_resource_fade_complete": False,
        "native906_complete_all_branches": False,
        "native907_auto_constructed_selected_live_banks": True,
        "native907_actual_reset_writes_sampler_variables": True,
        "blink_clock_random_variable_execution_in_source": True,
        "lip_selected_owner_constructed": True,
        "lip_audio_phase_driver_attached": False,
        "lip_audio_phase_driver_attached_scope": "all-source completion; LV partial source wiring is recorded separately, no runtime execution",
        "lip_legacy_LV_owner_and_sound_clock_in_source": True,
        "native927_928_live_owner_toggle_and_zero_reset_in_source": True,
        "type54_owned_buffer_receiver_installed": True,
        "material_inertia_evaluator_complete": False,
        "material_operator_implemented_and_real_records_loaded": True,
        "material_operator_connected_to_complete_force_schedule": False,
        "907_transition_reset_after_unresolved_force_update_diagnosed": True,
        "strict_reports_missing_material_and_model_features": True,
        "model_texture_storage_shared_with_Arc": True,
        "model_cache_identity": "exact resource name plus expanded PSB SHA256; weak cache does not retain released assets",
        "compiled": False, "tests_run": False, "Rust_execution": False,
        "original_game_or_DLL_executed": False, "full_NEKO_strict_frame_claim": False,
        "complete_native_count_added": 0,
    }
    payload = {
        "schema": "cs2-recovery-A-emote-manager/v1",
        "task_date": "2026-11-17",
        "generated_at_utc": datetime.now(timezone.utc).isoformat(),
        "summary": summary,
        "actual_primary_controller_inventory": control_inventory,
        "APIs": {
            "actual_load": "EmoteServices::import_m2_resource(task,name,Option<&EmoteResource>,CpuM2Load)",
            "typed_owner": "CpuM2LiveOwner::new(config,selectors,&mut variables)",
            "native907": "CpuM2LiveOwner::apply_907(raw,&mut base_variables,&effective_variables)",
            "real_blink": "advance_with_random(task,EmoteTick,&mut actual_shared_integer_rng)",
            "inspection": "EmotePlaybackInfo.m2_owner:CpuM2OwnerInfo with selected/active bank and phase counts",
            "unchanged_hooks": "C register/update/release calls are consumed; only update hook implementation now supplies host RNG",
            "task_messages": "emote_native::receive installed as type54 OwnedTaskBuffers receiver; supported524/526/536/541/542",
            "material_operator": "CpuM2Material::from_record/inject_variable_delta/step/variable_offset; real owner retains parsed records",
        },
        "semantic_details": [
            "Actual primary/secondary DIF default variables and selected pattern values are imported before load commit.",
            "Default timelineControl stays separate from metadata.base object/motion; actual waiting_loop labels evaluate into geometry variables.",
            "Missing default timeline label follows original stop-all/no-match behavior without substituting an object motion.",
            "Identical task/resource/profile/timeline/pattern inputs preserve model,clock,active timelines and blink timestamps.",
            "The M2 wrapper clock survives resource replacement; raw PSB import remains a distinct new-player API.",
            "Repeated same-name resources skip PSZ reading/inflation; CPU model/texture storage is shared by Arc when cloning player state.",
            "184ae0/184bd0 deactivate other pattern records and activate the exact selected byte-name key.",
            "183f10 consumes a shared MT integer only at equal blink timestamps; unsigned scheduling and two-segment f32 blink values write the real sampler dictionary.",
            "907 uses actual current variables for reset baselines and reinitializes selected A/B records; no empty-bank fallback.",
            "927 restores active blink variables on zero;928 restores active lip base values and clears phase-enable state.",
            "Actual LV native924 path now binds owned legacy bytes and separate sound elapsed ticks; waveform/LPD automatic paths still diagnose.",
            "Identified fix transitions now reuse the real owner/history; timed/nomove chains and replacement fade remain checked errors.",
            "Timeline-only changes preserve the entire live owner; unchanged patterns do not reimport constructor defaults.",
            "Changing transition enable after an unresolved force update is explicitly diagnosed; no guessed current inertia baseline is returned as native907 success.",
        ],
        "source_hashes": [{"path": path, "sha256": sha((ROOT / path).read_bytes())} for path in sources],
        "EXE_snippets": {key: block(exe, *value) for key, value in spans.items()},
        "driver_snippets": {key: block(driver, *value) for key, value in dll_spans.items()},
        "classification": "scripts/analysis + reports; no temporary asset outputs",
    }
    out = ROOT / "reports/recovery_A_20261117_emote_manager"
    out.with_suffix(".json").write_text(json.dumps(payload, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    compact = {key: payload[key] for key in [
        "schema", "task_date", "summary", "APIs", "semantic_details", "actual_primary_controller_inventory",
    ]}
    out.with_name(out.name + "_summary.json").write_text(json.dumps(compact, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    out.with_suffix(".md").write_text(
        "# A / M2 manager 与运行中银行批次（2026-11-17）\n\n"
        "C三处register/update/release hook已实际存在；本批只改A的hook实现、未覆盖任务工厂。\n\n"
        "906已能将实际CNF/PFL/DIF/include/PSZ提交到CPU player：主次默认变量、pattern选择、"
        "root transform、默认timeline均进入实际状态。manager按真实任务ID分离player；"
        "M2Plane的内部selector固定0。完全相同的重复装载保留时钟、timeline和blink时间戳；"
        "零transition的同资源pattern变更会构造并激活新银行。\n\n"
        "新增emote_m2live.rs的CpuM2LiveOwner，不再只交付构造种子。184ae0/184bd0只激活当前"
        "pattern标签对应记录；907重置这些记录并写入实际sampler变量。blink按183f10使用共享整数随机流、"
        "无符号时间戳及f32双段曲线执行；927/928的零值恢复和开关也作用于实际银行。\n\n"
        "通过playback_info(task).m2_owner可读取实际selected_patterns、A/B记录数、active数量、"
        "运行中lip phase数量、requested_enable与pending_effects。PSB纹理改用Arc共享，复用player不复制整张纹理。\n\n"
        "后续chain批已允许有据fix非零切换，并在只改timeline时保留整个live owner；"
        "边界仍明确：timed/nomove复用切换缺chain evaluator，替换已装载资源缺旧surface/fade owner；暂时报错。"
        "lip银行已按当前pattern激活；后续chain_lpd批已接实际LV、独立sound时钟和924分支，"
        "waveform/自动LPD分支及material/inertia仍有明确缺口。"
        "后续批已新增emote_m2physics.rs，按194730的f32顺序实现clamp/弹簧阻尼积分、"
        "配置真实material记录和1948c0缩放输出；尚未将部分算子冒充完整inertia调度。"
        "完整63个instant timeline目标及通用mesh/stencil继续下一批。完整NEKO strict帧、新增全部分支完成native计数仍为0。\n\n"
        "未编译、未测试、未运行Rust或原游戏/DLL；只读原始字节并进行源码语法解析。"
        "脚本归scripts/analysis，报告归reports；本批没有临时资产。\n",
        encoding="utf-8",
    )
    current_path = ROOT / "reports/recovery_A_20261117_emote_current_summary.json"
    current = read_json(current_path)
    current["manager_batch"] = compact
    current["summary"]["native907_automatic_profile_bank_import_complete"] = False
    current["summary"]["native907_automatic_selected_live_owner_in_source"] = True
    current["summary"]["backend_native_branches"]["906"] = "real PFL/PSZ commit and identity reuse; nonzero chain/replacement fade incomplete"
    current["summary"]["backend_native_branches"]["907"] = "auto selected live owner banks; actual reset writes; material/lip audio evaluator still incomplete"
    current["source_hashes_current"] = payload["source_hashes"]
    if "host_integration" in current:
        current["host_integration"]["summary"]["task_hooks_observed_in_native_tasks"] = hooks
    current_path.write_text(json.dumps(current, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(summary, ensure_ascii=True))


if __name__ == "__main__":
    main()

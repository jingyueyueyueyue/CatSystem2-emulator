"""Small Emote integration ledger plus bounded M2 owner-construction evidence."""
from __future__ import annotations
from datetime import datetime, timezone
import json
from recovery_A_20261116 import ROOT, Image, read_json, sha
from recovery_A_receiver_contracts_20261116 import block
from recovery_A_emote_host_20261117 import sections
from audit_runtime_evidence import kif_entries, blowfish_le_decrypt


def main():
    prior = read_json(ROOT / "reports/recovery_A_20261116_emote.json")
    image = Image(ROOT / prior["binary_path"], prior["PE_sha256"])
    paths = [
        "simulator/crates/runtime/src/emote.rs",
        "simulator/crates/runtime/src/emote_cpu.rs",
        "simulator/crates/runtime/src/emote_variable_timeline.rs",
        "simulator/crates/runtime/src/emote_m2banks.rs",
        "simulator/crates/desktop/src/emote_native.rs",
        "simulator/crates/desktop/src/data_services.rs",
        "simulator/crates/desktop/src/lib.rs",
        "scripts/analysis/recovery_A_emote_integration_20261117.py",
    ]
    task_source = (ROOT / "simulator/crates/desktop/src/native_tasks.rs").read_text(encoding="utf-8")
    calls = {name: f"{name}(" in task_source for name in [
        "emote_register_task_hook", "emote_update_task_hook", "emote_release_task_hook"
    ]}
    archive = image.path.parent / "m2config.int"
    entries, _, cipher = kif_entries(archive, 0x0F6BB0AE)
    ca01 = next(entry for entry in entries if entry["name"] == b"ca01.inc")
    with archive.open("rb") as handle:
        handle.seek(ca01["offset"])
        data = handle.read(ca01["size"])
    data = blowfish_le_decrypt(cipher, data) if cipher else data
    actual = sections(data)
    config_counts = {key: len(actual.get(key, [])) for key in [
        "defval", "pattern", "blink", "lip_table", "lip_variable", "lip_flow",
    ]}
    spans = {
        "A_owner_from_profile_map50": (0x177C6F, 0xA7),
        "A_initial_value_from_first_pattern_variable": (0x177E8E, 0x89),
        "A_record_parameters": (0x183BA7, 0x45),
        "B_owner_from_profile_map58": (0x1781F7, 0x92),
        "B_initial_phase_list": (0x183DE1, 0x39),
        "B_lip_flow_tuple_decoder": (0x18A940, 0xBE),
        "transition_constructor_empty_vectors": (0x191B67, 0x6C),
        "profile_default_variable_application_then_rebuild": (0x171E10, 0x6C),
    }
    payload = {
        "schema": "cs2-recovery-A-emote-integration/v1",
        "task_date": "2026-11-17",
        "generated_at_utc": datetime.now(timezone.utc).isoformat(),
        "summary": {
            "exact_PE_initialization_hook_connected": True,
            "DataServices_owned_state_connected": True,
            "descriptor_dispatch_before_RequiresImplementation": True,
            "strict_scene_draw_hook_connected": True,
            "natural_display_only": True,
            "type54_solid_primitive_fallback_allowed": False,
            "task_hooks_observed_in_native_tasks": calls,
            "native906_complete": False,
            "native906_actual_first_load_and_zero_transition_commit": True,
            "native907_automatic_owner_import_complete": False,
            "native907_selected_live_banks_and_blink_execution_in_source": True,
            "automatic_M2_controller_construction_input_import_in_source": True,
            "automatic_constructor_seed_import_is_live907_state": False,
            "actual_primary_include_control_rows": config_counts,
            "native906_does_not_bypass_M2_parser_with_raw_PSZ": True,
            "variable_timeline_labels_fitting_actual_first_model": 52,
            "variable_timeline_labels_with_immediate_selector_subset": 53,
            "variable_timeline_labels_with_recovered_immediate_controls": 55,
            "instant_target_labels_remaining": 60,
            "first_model_timeline_total": 115,
            "recursive_type5_motion_source_mask_rasterization_connected": True,
            "general_type5_scheduler_complete": False,
            "full_NEKO_strict_frame_claim": False,
            "compiled": False, "tests_run": False, "Rust_pixel_execution": False,
            "original_game_or_DLL_executed": False,
            "integrated_completed_native_count": 0,
        },
        "bank_identity_correction": {
            "A_player84": "#blink via primary difference profile+50,177c10/1877e0; not #chain",
            "B_player90": "#lip_table/#lip_flow via primary profile+58,178190",
            "selected_variable_source": "first variable/value in matched #pattern entry,177e8e and1783ef",
            "A_constructor": "record stores first pattern value,begin/end,speed,min/max wait; active field1,183ac0",
            "B_constructor": "first lip_table scalar raw bits become record40 and initial phase; lip_flow tuples are integer,float,integer",
            "transition_owner": "191b20 initializes empty vectors; later material/key/controller population and live baselines must be recovered",
            "current_limit": "live blink and actual legacy LV/sound tick implemented; automatic LPD/waveform and material/inertia remain incomplete",
            "new_parser": "CpuM2ControllerConfig::from_expanded/default_variables/reset_bank_seeds; real config supplies every value",
        },
        "host_parameters": {
            "default_object_motion": "metadata.base.chara/motion,initial frame0",
            "actual_first_model_default": ["all_parts", "\u30bf\u30a4\u30e0\u30e9\u30a4\u30f3\u69cb\u9020"],
            "PFL_waiting_labels": "metadata.timelineControl,flags2; play_timeline(task,label,2)",
            "original_tick": "raw task clock elapsed; first delta1,units/frame1",
            "actual_target": "read m2config/m2view.cnf wndsize;1280x720 -> origin639.5,359.5,scale1",
            "raw_import_new_player_clock": "raw PSB API resets clock; M2 wrapper identity reuses its task clock across model configuration",
            "viewport_after_boxing": "EmoteServices::set_viewport",
        },
        "APIs": {
            "register": "LoggingHost::emote_register_task_hook(id,kind)->script::Result<()>; before publishing type54",
            "update": "LoggingHost::emote_update_task_hook(id,clock.elapsed())->script::Result<()>; once per task body",
            "release": "LoggingHost::emote_release_task_hook(id)->script::Result<()>",
            "startup": "EmoteNativeServices::initialize(ResourceIndex,actual_PE_sha256)",
            "ordinary": "LoggingHost::emote_native -> shared36 descriptor -> real state or diagnostic",
            "draw": "EmoteNativeServices::draw_scene_task -> strict render -> real Canvas -> count successful pixels",
            "shutdown": "EmoteServices::release_all/shutdown; destruction works after fingerprint mismatch",
            "inspection": "status/task_ids/playback_info; includes resource_generation and active timeline labels",
            "controller_import": "load_m2_resource_plan now expands primary DIF include records and produces controller_config/reset_bank_seeds",
        },
        "remaining_order": [
            "timed pattern chains, old-surface fade and secondary smooth transitions",
            "LPD automatic presets, waveform/flow and material/global controls",
            "shared task-command target0/raw69/70 receiver; explicit target is currently required",
            "full stencil join/reference inheritance scheduling and meshCombine/meshSyncChildMask",
            "metadata controllers,instant targets,3D/nonlocal/layer1 and color/blend modes",
            "nonidentity final display quad and complete save/restore of live controllers",
        ],
        "snippets": {name: block(image, *span) for name, span in spans.items()},
        "sources": [{"path": p, "sha256": sha((ROOT / p).read_bytes())} for p in paths],
        "classification": "analysis scripts classified in scripts/analysis; reports in reports; no temporary outputs or assets",
    }
    out = ROOT / "reports/recovery_A_20261117_emote_integration"
    out.with_suffix(".json").write_text(json.dumps(payload, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    short = {k: v for k, v in payload.items() if k not in {"snippets", "sources"}}
    out.with_name(out.name + "_summary.json").write_text(json.dumps(short, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    out.with_suffix(".md").write_text(
        "# A / Emote 主宿主接入批次（2026-11-17）\n\n"
        "本批已改 DataServices.emote、Headless exact-SHA 初始化、profile guard 前 descriptor 分派、"
        "render_scene strict CPU draw；未覆盖 C 的 create_task_object。"
        f"C 文件中实际观察到的 hook：{json.dumps(calls, ensure_ascii=False)}。\n\n"
        "## 可集成接口\n\n"
        "- register：emote_register_task_hook(id,kind)，工厂发布 type54 前调用。\n"
        "- update：emote_update_task_hook(id,clock.elapsed())，实际任务 body 更新一次。\n"
        "- release：emote_release_task_hook(id)，允许指纹失配后清理。\n"
        "- inspection：state.status/task_ids/playback_info；release_all/shutdown 用于退出和换游戏。\n"
        "- timeline：state.play_timeline(task,label,2)，支持实际 waiting_loop/loop2/loop3 的幂 easing 加法轨道；"
        "首模型115标签中52满足该源代码子集，不能算52个已验收函数。\n\n"
        "## 真实906/907边界\n\n"
        "906现在经过真实CNF→PFL→DIF/include→PSZ并可提交首次装载和零transition变更；"
        "按真实任务ID分别拥有player，内部selector0有原版证据。实际选中pattern的A/B银行自动建立，"
        "907和blink更新作用于这些银行及sampler变量。非零复用chain、旧surface/fade、"
        "后续chain_lpd批已接实际legacy LV和独立声音时钟；自动LPD/waveform、material/inertia仍有明确缺口。"
        "详见emote_manager与emote_chain_lpd报告。\n\n"
        "重要纠正：bank84是 #blink（177c10/1877e0、profile+50），不是 #chain。"
        "bank90是 #lip_table/#lip_flow（178190、profile+58）。"
        "两者变量/初始值来自命中 #pattern 的首个变量；不能用 PSB 全变量列表替代。"
        "lip_flow 由18a940解成整数/浮点/整数三元组，首 lip_table scalar 的原始f32字节参与phase初值。\n\n"
        "新增 emote_m2banks.rs：CpuM2ControllerConfig::from_expanded/default_variables/reset_bank_seeds，"
        f"真实ca01.inc含{config_counts['defval']}个默认值、{config_counts['blink']}条blink、"
        f"{config_counts['lip_table']}条lip_table、{config_counts['lip_flow']}条lip_flow。"
        "load_m2_resource_plan自动按主DIF/include导入这些构造输入；"
        "其后由emote_m2live.rs执行pattern激活、blink计时及907重置；"
        "构造数据与live行为已分开，完整907/全模型等价仍不宣称完成。\n\n"
        "## 像素与缺口\n\n"
        "新接递归motion-mask逐图元alpha测试，保留真实priority、parameterize和warp；"
        "不先合成preview再当mask。一般join/inheritance/mesh调度仍有strict警告。"
        "仅自然尺寸、零位移、单位缩放显示路径已接，其他宿主映射明确拒绝；type54不再掉进纯色矩形替代。\n\n"
        "未编译、未测试、未执行Rust像素或原游戏/DLL。完整 NEKO strict 帧、新增已完成native计数仍为0。"
        "新增脚本归scripts/analysis，报告归reports；本批没有临时资源或发布资产。\n",
        encoding="utf-8",
    )
    current_path = ROOT / "reports/recovery_A_20261117_emote_current_summary.json"
    current = read_json(current_path)
    current["summary"]["status"] = "source integration connected; M2 manager/controller and full strict frame incomplete"
    current["summary"]["remaining_strict_blockers"] = payload["remaining_order"]
    current["scope"] = "A emote modules and additive DataServices/lib hooks; C owns task factory lifecycle insertion"
    current["host_integration"] = short
    current["source_hashes_current"] = payload["sources"]
    current_path.write_text(json.dumps(current, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(payload["summary"], ensure_ascii=True))


if __name__ == "__main__":
    main()

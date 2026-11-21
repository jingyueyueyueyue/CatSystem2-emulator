"""Small source/integration contract for the independently written Emote backend.

Reads source and previous byte evidence. No build, tests, engine or asset export.
"""
from __future__ import annotations

from datetime import datetime, timezone
import json

from recovery_A_20261116 import ROOT, read_json, sha


def main():
    paths = [
        "simulator/crates/runtime/src/emote.rs",
        "simulator/crates/runtime/src/emote_cpu.rs",
        "simulator/crates/runtime/src/emote_host.rs",
        "simulator/crates/runtime/src/emote_control.rs",
        "simulator/crates/runtime/src/emote_m2config.rs",
        "simulator/crates/desktop/src/emote_native.rs",
        "scripts/analysis/recovery_A_emote_lifecycle_20261117.py",
    ]
    payload = {
        "schema": "cs2-recovery-A-emote-lifecycle/v1",
        "task_date": "2026-11-17",
        "generated_at_utc": datetime.now(timezone.utc).isoformat(),
        "summary": {
            "source_implemented": True,
            "compiled": False, "tests_run": False, "game_or_DLL_executed": False,
            "full_NEKO_strict_frame_claim": False,
            "integrated_completed_native_count": 0,
            "new_lifecycle": [
                "status/task_ids/playback_info expose actual owned state without claiming rendering compatibility",
                "release_task/release_all/shutdown remain usable after profile mismatch",
                "resource replacement increments generation and invalidates907 banks only after complete default selection",
                "invalid metadata.base preserves the old player",
                "set_viewport reaches a boxed backend and preserves original clock scale1",
            ],
            "default_motion": {
                "source": "PSB metadata.base.chara/motion; driver RVA fdea..ff87",
                "first_model_object": "all_parts",
                "first_model_motion": "\u30bf\u30a4\u30e0\u30e9\u30a4\u30f3\u69cb\u9020",
                "frame": 0,
                "PFL_timeline_label_is_object_motion": False,
            },
            "native907": {
                "source": "EXE1744a0,174a60,174790,193400",
                "raw_DWORD_preserved": True,
                "actual_owner_banks_required": True,
                "automatic_PFL_bank_import_complete": False,
                "is_visibility_switch": False,
                "safe_to_route_all_real_NEKO907_calls": False,
            },
            "viewport_time": {
                "actual_target": "m2view.cnf wndsize1280x720",
                "internal_origin": [639.5, 359.5],
                "internal_scale": 1.0,
                "tick_input": "raw task64-task58, not wall-clock milliseconds",
                "driver_units_per_frame": 1.0,
                "first_tick_delta": 1,
                "display": "logical*host_scale+host_offset-0.5; actual host context required",
            },
        },
        "APIs": {
            "attach": [
                "EmoteServices::bind_pe_sha256(actual_sha)",
                "CpuViewport::original_neko_target(actual_target_width,actual_target_height)",
                "CpuEmoteBackend::new_original_task_clock(viewport)",
                "CpuEmoteBackend::set_resources(actual_provider)",
                "EmoteServices::set_backend(Box::new(backend))",
                "EmoteServices::register_task(actual_id,54)",
            ],
            "load": "import_psz(task,exact_resource_name,PSZ_or_PSB); read playback_info for generation/default selection",
            "907": "bind_native907_state(task,actual_controller_owner_state) after every successful import; dispatch907 uses raw input",
            "tick": "advance(task,EmoteTick{task_elapsed:raw_task64_minus58,increment:0})",
            "mapping": "set_root_affine(task,actual_resolved_affine); set_viewport(actual_offscreen_viewport)",
            "draw": "render_rgba->strict validation->Canvas submission->record_pixel_submission; never promote render_preview",
            "inspect": "status(),task_ids(),state(task),playback_info(task)",
            "destroy": "release_task(task) or release_all(); shutdown() also drops backend/resource provider and profile",
        },
        "remaining": [
            "PFL/DIF/INC commands into actual907 controller reset owners",
            "timelineControl variable tracks/loops/easing and controller evaluation",
            "general joined/reference stencil,meshSyncChildMask,additive BP scheduling",
            "nonlinear layer curves,3D and nonlocal coordinates,unsupported layer1,color/blend modes",
        ],
        "evidence_reports": [
            "reports/recovery_A_20261117_emote_host.json",
            "reports/recovery_A_20261117_emote_dispatch.json",
            "reports/recovery_A_20261117_emote_texture_color.json",
        ],
        "sources": [{"path": p, "sha256": sha((ROOT / p).read_bytes())} for p in paths],
        "classification": "scripts/analysis + reports; no temporary files generated",
    }
    out = ROOT / "reports/recovery_A_20261117_emote_lifecycle"
    out.with_suffix(".json").write_text(json.dumps(payload, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    out.with_suffix(".md").write_text(
        "# A / Emote 宿主生命周期交付（2026-11-17）\n\n"
        "本批只改 A 的独立模块；未编译、未测试、未执行原版，完整 NEKO strict 帧及新增已接通 native 计数仍为 0。\n\n"
        "## 可绑定 API\n\n"
        "- 先 `bind_pe_sha256`，再 `set_backend(Box::new(CpuEmoteBackend::new_original_task_clock(viewport)?))`；"
        "`register_task(id,54)` / `import_psz` / `advance` / `render_rgba` / `release_task`。\n"
        "- `status/task_ids/playback_info` 提供真实任务数、资源名及代数、默认/当前 object-motion、时间、907 表绑定状态。"
        "不含 PSB 或图像，不能直接当作完整存档。\n"
        "- `set_viewport` 可在后端装箱后更新实际离屏目标。默认 1280×720 的 origin=(639.5,359.5)、scale=1；"
        "最终显示四边形另用 `CpuOriginalDisplayMapping`，不能再重复居中。\n"
        "- `release_task/release_all/shutdown` 在 profile 匹配失效后也可清理；失败保留未释放对象以便重试。"
        "`shutdown` 成功后销毁 provider、backend 并解除指纹。\n\n"
        "## 默认选择与 907\n\n"
        "原始 PSB 默认从 metadata.base 选择 object/motion，首个实际模型为 all_parts / タイムライン構造、frame0。"
        "PFL 的 waiting_loop 是 metadata.timelineControl 标签，不可直接传给 select_motion。"
        "导入在替换旧对象前验证默认 motion 存在；成功才增加 resource_generation 并使旧 907 表失效。\n\n"
        "907 的原始 DWORD、场景覆盖、过渡与 A/B 表重置已有有据接收器，但还依赖宿主提供真实 owner 表。"
        "缺表时报错；不能接成 enabled=false，也不能用空表假称已实现所有 NEKO 调用。"
        "Tick 使用 raw(task64-task58)，第一步1，后续无 ms→fps 转换。\n\n"
        "后续批继续 timelineControl 与实际模型 stencil/mesh；所有 strict 警告均阻止 native 像素成功。"
        "分类：新脚本置 scripts/analysis，报告置 reports，本批无临时输出。\n",
        encoding="utf-8",
    )
    current_path = ROOT / "reports/recovery_A_20261117_emote_current_summary.json"
    current = read_json(current_path)
    current["lifecycle_contract"] = str(out.with_suffix(".json").relative_to(ROOT)).replace("\\", "/")
    current["lifecycle_APIs"] = payload["APIs"]
    current["source_hashes_current"] = payload["sources"]
    current_path.write_text(json.dumps(current, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"report": str(out.relative_to(ROOT)), "summary": payload["summary"]}, ensure_ascii=True))


if __name__ == "__main__":
    main()

"""Actual lip_flow/window/finish delivery; bounded original bytes, static only."""
from __future__ import annotations
from datetime import datetime, timezone
import json

from recovery_A_20261116 import ROOT, Image, read_json, sha
from recovery_A_receiver_contracts_20261116 import block
from recovery_A_emote_host_20261117 import sections
from audit_runtime_evidence import kif_entries, blowfish_le_decrypt


def main():
    prior = read_json(ROOT/"reports/recovery_A_20261116_emote.json")
    exe = Image(ROOT/prior["binary_path"], prior["PE_sha256"])
    archive = exe.path.parent/"m2config.int"
    entries, _, cipher = kif_entries(archive, 0x0F6BB0AE)
    entry = next(row for row in entries if row["name"] == b"ca01.inc")
    with archive.open("rb") as handle:
        handle.seek(entry["offset"])
        raw = handle.read(entry["size"])
    raw = blowfish_le_decrypt(cipher, raw) if cipher else raw
    rows = sections(raw)
    summary = {
        "concrete_waveform_lip_flow_backend_source": True,
        "native924_no_sidecar_monitor_branch_in_source": True,
        "host_consumes_D_actual_sound_signal_window_API": True,
        "host_calls_raw_version_shifted_sound_commands": False,
        "scalar_level_repeated_as_waveform": False,
        "actual_threshold_and_flow_configuration": True,
        "one_flow_boundary_per_original_tick": True,
        "independent_actual_sound_elapsed": True,
        "native907_requests_actual_waveform_finish": True,
        "explicit_disable_finishes_last_interval": True,
        "source_not_playing_restores_actual_base": True,
        "sidecar_to_waveform_release_complete": False,
        "automatic_LPD_preset_complete": False,
        "actual_first_profile_lip_records": len(rows["lip_table"]),
        "actual_first_profile_flow_records": len(rows["lip_flow"]),
        "full_NEKO_strict_frame_claim": False,
        "complete_native_count_added": 0,
        "compiled": False, "tests_run": False, "Rust_execution": False,
        "original_game_or_DLL_executed": False,
    }
    APIs = {
        "waveform_object": "CpuM2Waveform::new/observe/sample/request_finish/disable",
        "owned_window_receiver": "EmoteServices::update_m2_waveform(task,actual_DWORD_samples,original_extent)",
        "source_finish_or_stop": "EmoteServices::control_m2_waveform(task,immediate_stop)",
        "actual_tick": "advance_with_random -> owner.advance_lip(lip_elapsed) -> real sampler variables",
        "host_input": "LoggingHost::sound_signal_capacity/sound_signal_window supplied by D, read only here",
        "monitor": "actual selected SoundTaskChannel.signal_enabled and AudioSignal.enable",
        "inspection": "playback_info().m2_owner waveform_evaluators,bound_sidecar_evaluators,phase counts",
    }
    semantics = [
        "174610 consumes sample1 only after raw bank20 differs from new bank24;184410 commits new bank20.",
        "definition+34 is the configured unsigned threshold; first loud sample activates/reset actual flow.",
        "quiet-window scan uses indices1..<raw_extent/4 and requires floor(raw_extent/10) loud entries; no invented PCM meter expansion.",
        "184f90 preloads flow0/flow1 values,frames,finish-check words from actual lip_table/lip_flow; out-of-range pattern index falls back to table0.",
        "185080 increments/wraps phase and shifts preloaded from/to/duration/check fields.",
        "184130 crosses at most one phase boundary per update and sets start=now; does not consume guessed leftover time.",
        "quiet tail plus finish-check sets target=table0 for a final interval; next boundary restores true base and disables phase.",
        "174790/native907 ask active waveform to finish;174870 on nonplaying audio restores actual base immediately.",
        "Original NEKO213 corresponds to D's typed signal window behavior, not D's raw213 scalar query; no raw command forwarding.",
        "Switching a still-bound sidecar evaluator to waveform is explicitly rejected until original lifetime/release ownership is recovered.",
    ]
    spans = {
        "M2Plane_no_sidecar_monitor_enable": (0x238E65, 0x66),
        "M2Plane_tick_query_playing": (0x238365, 0x61),
        "M2Plane_signal_query_then_owned_copy": (0x2383D3, 0xD3),
        "window_current_threshold": (0x174668, 0x50),
        "window_quiet_scan_extent_div4_div10": (0x1746B8, 0x5F),
        "window_previous_commit": (0x184410, 0x6),
        "flow_preload": (0x184F90, 0xE5),
        "waveform_time_finish": (0x184170, 0xEF),
        "waveform_linear_actual_sampler_write": (0x184260, 0xA2),
        "flow_advance_and_wrap": (0x185080, 0x114),
        "flow_finish_to_table0": (0x1851A0, 0x2A),
        "native907_active_B_finish": (0x1747D8, 0x31),
        "nonplaying_actual_base_reset": (0x174984, 0x47),
        "actual_PVL_window_source": (0x1EB0D0, 0x96),
    }
    paths = [
        "simulator/crates/runtime/src/emote.rs",
        "simulator/crates/runtime/src/emote_cpu.rs",
        "simulator/crates/runtime/src/emote_m2live.rs",
        "simulator/crates/runtime/src/emote_m2wave.rs",
        "simulator/crates/desktop/src/emote_native.rs",
        "scripts/analysis/recovery_A_emote_waveform_20261117.py",
    ]
    dependencies = [
        "simulator/crates/desktop/src/sound_signal.rs",
        "simulator/crates/desktop/src/sound_sidecar.rs",
        "simulator/crates/desktop/src/audio_control.rs",
    ]
    payload = {
        "schema": "cs2-recovery-A-emote-waveform/v1",
        "task_date": "2026-11-17", "generated_at_utc": datetime.now(timezone.utc).isoformat(),
        "binary": {"path": prior["binary_path"], "sha256": exe.digest},
        "summary": summary, "APIs": APIs, "semantic_details": semantics,
        "actual_config": {"identity": "m2config.int::ca01.inc", "sha256": sha(raw),
                         "lip_table": rows["lip_table"], "lip_flow": rows["lip_flow"]},
        "EXE_evidence": {key: block(exe, *span) for key, span in spans.items()},
        "sources": [{"path": path, "sha256": sha((ROOT/path).read_bytes())} for path in paths],
        "D_dependencies_read_only": [{"path": path, "sha256": sha((ROOT/path).read_bytes())} for path in dependencies],
        "remaining": [
            "sidecar->waveform lifetime/release,LPD automatic profile presets",
            "complete timed controller graphs and same-variable blend/offset ownership",
            "complete material/global force scheduler",
            "strict full model mesh/stencil and final viewport/save integration",
        ],
        "classification": "scripts/analysis and reports; no temporary outputs/assets; no D/audio or main render_scene/441 changes",
    }
    out = ROOT/"reports/recovery_A_20261117_emote_waveform"
    out.with_suffix(".json").write_text(json.dumps(payload, ensure_ascii=False, indent=2)+"\n", encoding="utf-8")
    small = {key: payload[key] for key in ("schema", "task_date", "summary", "APIs", "semantic_details", "remaining", "classification")}
    out.with_name(out.name+"_summary.json").write_text(json.dumps(small, ensure_ascii=False, indent=2)+"\n", encoding="utf-8")
    out.with_suffix(".md").write_text(
        "# A / actual waveform与lip_flow批次（2026-11-17）\n\n"
        "新增emote_m2wave.rs并接live owner：实际sample1阈值、按raw extent的quiet lookahead、"
        "真实flow表预装/推进/回绕、fix/linear插值、finish-check收口与基值恢复。"
        "advance_lip用独立sound tick写入真实sampler变量。907同时对实际wave对象请求收口，"
        "不只更新银行的展示字段。\n\n"
        "desktop的924/540无sidecar分支现打开D的实际PCM monitor，保存typed sound link；"
        "任务更新先查真实playing，再通过sound_signal_capacity/window读取真正owned窗口。"
        "非playing立即恢复base；显式关闭停止monitor、保留声音时钟供最后flow interval结束。"
        "该路径没有调用另一版本的213/214号命令，没有复制标量音量伪造窗口，未改D的声音文件。\n\n"
        "证据：174610使用sample1，quiet扫描extent/4并要求extent/10的loud条数；"
        "184f90来自真实lip_flow，185080推进，1851a0置finish到table0，"
        "184130每次最多过一个phase，重设start=now。具体原字节见JSON。\n\n"
        "仍拒绝带旧sidecar转waveform的未恢复释放策略；自动LPD配置覆盖、完整力学、"
        "跨edge图与mesh/stencil继续待恢复。没有声称924全部分支完成或NEKO像素通过。\n\n"
        "未编译、未测试、未执行Rust/游戏/DLL。只读原字节/资源及rustfmt语法读取。"
        "脚本归scripts/analysis，报告归reports；无临时文件、游戏资产输出、新目录或main441改动。\n",
        encoding="utf-8",
    )
    path = ROOT/"reports/recovery_A_20261117_emote_current_summary.json"
    current = read_json(path)
    current["waveform_batch"] = small
    current["summary"]["backend_native_branches"]["924"] = (
        "owned LV plus actual PCM/PVL waveform-flow source route; old-sidecar switching and automatic LPD presets remain incomplete"
    )
    current["source_hashes_current"] = payload["sources"]
    path.write_text(json.dumps(current, ensure_ascii=False, indent=2)+"\n", encoding="utf-8")
    print(json.dumps(summary, ensure_ascii=True))


if __name__ == "__main__":
    main()

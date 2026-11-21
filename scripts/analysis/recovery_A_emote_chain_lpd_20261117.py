"""Small SHA-bound M2 chain/LPD delivery, not a new candidate CFG sweep.

Only static original bytes, config records and source hashes are read.
Never imports game code, invokes Rust, builds, tests, or writes assets.
"""
from __future__ import annotations

from collections import Counter
from datetime import datetime, timezone
import json
import math
import struct

from recovery_A_20261116 import ROOT, Image, read_json, sha
from recovery_A_receiver_contracts_20261116 import block, rtti_vtable
from recovery_A_emote_host_20261117 import sections
from audit_runtime_evidence import kif_entries, blowfish_le_decrypt


def main():
    prior = read_json(ROOT / "reports/recovery_A_20261116_emote.json")
    exe = Image(ROOT / prior["binary_path"], prior["PE_sha256"])
    archive = exe.path.parent / "m2config.int"
    entries, _, cipher = kif_entries(archive, 0x0F6BB0AE)
    inventories = []
    with archive.open("rb") as handle:
        for name in (b"ca01.inc", b"body.dif"):
            entry = next(e for e in entries if e["name"] == name)
            handle.seek(entry["offset"])
            data = handle.read(entry["size"])
            data = blowfish_le_decrypt(cipher, data) if cipher else data
            rows = sections(data)
            direct, aliases, pairs = [], [], []
            alias = None
            for row in rows.get("chain", []):
                if row[0].startswith("$"):
                    alias = row[0]
                    aliases.append(alias)
                elif alias is None:
                    direct.append(row)
                elif len(row) >= 4:
                    pairs.append({"alias": alias, "from": row[0], "to": row[1],
                                  "kind": row[2], "frames": int(row[3])})
            inventories.append({
                "member": name.decode("ascii"), "decoded_sha256": sha(data),
                "direct_dimension_rows": direct, "aliases": aliases,
                "explicit_pair_count": len(pairs),
                "explicit_pair_kind_counts": dict(Counter(p["kind"] for p in pairs)),
                "explicit_pair_frame_counts": dict(Counter(str(p["frames"]) for p in pairs)),
                "pairs": pairs,
                "lip_table_records": len(rows.get("lip_table", [])),
                "lipper_records": len(rows.get("lipper", [])),
                "lipper_default_records": len(rows.get("lipper_default", [])),
                "is_live_call_input": False,
            })
    sound_rtti = rtti_vtable(exe, "kcTaskSound")
    if len(sound_rtti) != 1 or sound_rtti[0]["command_handler_RVA"] != 0x1EB670:
        raise ValueError("Original NEKO sound receiver identity changed")
    sound_commands = {}
    for code in (185, 200, 211, 213, 219, 220, 221, 222):
        tag = exe.get(0x1ED780 + code - 19, 1)[0]
        target = exe.u32(0x1ED6C4 + tag * 4) - exe.base
        sound_commands[str(code)] = {"case_rva": target, "switch_byte": tag}
    sound_inventory = []
    legacy_sample = None
    names = set()
    for sound_path in sorted(exe.path.parent.glob("pcm_*.int")) + [exe.path.parent / "update00.int"]:
        archive_name = sound_path.name
        sound_entries, _, sound_cipher = kif_entries(sound_path, 0x0F6BB0AE)
        counts = Counter(
            e["name"].rsplit(b".", 1)[-1].decode("ascii", "replace") for e in sound_entries
        )
        row = {
            "archive": archive_name, "entry_count": len(sound_entries),
            "lv_records": counts["lv"], "lpb_records": counts["lpb"],
            "lpd_records": counts["lpd"],
            "count_scope": "physical archive entries; no cross-archive override dedup",
            "LV_f32_scalars": 0, "LV_all_finite_and_aligned": True,
            "LV_minimum": None, "LV_maximum": None,
        }
        with sound_path.open("rb") as handle:
            for entry in sound_entries:
                if not entry["name"].endswith(b".lv"):
                    continue
                names.add(entry["name"])
                if entry["size"] > 4 * 1024 * 1024:
                    raise ValueError("LV exceeds the actual host sidecar bound")
                handle.seek(entry["offset"])
                raw = handle.read(entry["size"])
                raw = blowfish_le_decrypt(sound_cipher, raw) if sound_cipher else raw
                if not raw or len(raw) % 4:
                    row["LV_all_finite_and_aligned"] = False
                    continue
                values = [v[0] for v in struct.iter_unpack("<f", raw)]
                finite = all(map(math.isfinite, values))
                row["LV_all_finite_and_aligned"] &= finite
                row["LV_f32_scalars"] += len(values)
                if finite:
                    row["LV_minimum"] = min(values) if row["LV_minimum"] is None else min(row["LV_minimum"], min(values))
                    row["LV_maximum"] = max(values) if row["LV_maximum"] is None else max(row["LV_maximum"], max(values))
                if legacy_sample is None:
                    legacy_sample = {
                        "resource_identity": archive_name + "::" + entry["name"].decode("ascii"),
                        "decoded_bytes": len(raw), "decoded_sha256": sha(raw),
                        "f32_count": len(values), "all_finite": finite,
                        "minimum": min(values), "maximum": max(values),
                        "is_runtime_execution": False,
                    }
        sound_inventory.append(row)
    sidecar_totals = {
        "LV_physical_entries": sum(row["lv_records"] for row in sound_inventory),
        "LV_distinct_full_byte_names": len(names),
        "LPB_physical_entries": sum(row["lpb_records"] for row in sound_inventory),
        "LPD_physical_entries": sum(row["lpd_records"] for row in sound_inventory),
        "LV_f32_scalars_read_only": sum(row["LV_f32_scalars"] for row in sound_inventory),
        "LV_all_finite_and_aligned": all(row["LV_all_finite_and_aligned"] for row in sound_inventory),
        "identity_scope": "exact full CP932 resource byte name; not content or function identity; loader precedence not inferred",
        "is_Rust_or_game_execution": False,
    }
    spans = {
        "chain_dimension_parser": (0x189325, 0x23),
        "chain_kind_enum": (0x197E80, 0xDF),
        "exact_pair_kind_frames": (0x197861, 0x35),
        "pattern_chain_raw_enable_guard": (0x17329A, 0x41),
        "fix_actual_endpoint_write": (0x16D3A1, 0x20),
        "linear_kind_driver_parameters": (0x16D411, 0x5A),
        "reset_active_blink_on_pattern_change": (0x184620, 0x8C),
        "LPD_exact_boundary_is_record": (0x1AB2FA, 0x43),
        "LPD_default_preset": (0x1A19E0, 0x72),
        "LPD_override_weights": (0x1AB3B0, 0x32),
        "LPD_override_origin_radius": (0x1AB3F0, 0x31),
        "LPD_all_bank_records_bind_actual_bytes": (0x184EE0, 0x4B),
        "LV_actual_evaluator_constructor": (0x1754BC, 0x6D),
        "LV_owned_f32_array": (0x1A1E24, 0x4E),
        "LV_interior_symmetric_smoothing": (0x1A1E90, 0x54),
        "LV_direct_sample_without_LPD_range": (0x1A1F00, 0x28),
        "M2Plane924_enabled_channel": (0x238DF7, 0x67),
        "M2Plane924_query_device_loader_property": (0x238F09, 0x2F),
        "M2Plane924_owned_sidecar_pair": (0x238F38, 0x4C),
        "M2Plane219_query_and_task_clock": (0x23828A, 0x42),
        "task_clock_leaf_preserves_pushed_sound_arguments": (0x1DD440, 0x7),
        "manager_pushes_independent_two_clocks": (0x17DFBA, 0x14),
        "player_passes_sound_clock_to_lip": (0x17400C, 0x17),
        "LV_constructor_origin_zero": (0x175502, 0x22),
        "LV_open_unused_third_argument": (0x1A1DF0, 0x104),
        "sound219_elapsed": (0x1ED3CB, 0x98),
        "sound220_sidecar_pair": (0x1ED463, 0x69),
        "sound221_legacy_pair": (0x1ED4CC, 0x70),
        "sound222_has_sidecar": (0x1ED53C, 0x7D),
        "sound200_device_query": (0x1CC090, 0x16),
        "sound200_device_interface": (0x1D0F80, 0x12),
    }
    paths = [
        "simulator/crates/runtime/src/emote_m2chain.rs",
        "simulator/crates/runtime/src/emote_m2banks.rs",
        "simulator/crates/runtime/src/emote_m2live.rs",
        "simulator/crates/runtime/src/emote_m2lip.rs",
        "simulator/crates/runtime/src/emote_cpu.rs",
        "simulator/crates/runtime/src/emote_variable_timeline.rs",
        "simulator/crates/runtime/src/emote_m2selector.rs",
        "simulator/crates/runtime/src/emote.rs",
        "simulator/crates/desktop/src/emote_native.rs",
        "scripts/analysis/recovery_A_emote_chain_lpd_20261117.py",
    ]
    modified_scripts = [
        "scripts/analysis/recovery_A_emote_chain_lpd_20261117.py",
        "scripts/analysis/recovery_A_emote_variable_timeline_20261117.py",
        "scripts/analysis/recovery_A_emote_integration_20261117.py",
        "scripts/analysis/recovery_A_emote_manager_20261117.py",
        "scripts/analysis/recovery_A_emote_lip_physics_20261117.py",
    ]
    summary = {
        "native906_same_profile_nonzero_fix_transition_in_source": True,
        "native906_same_profile_timeline_change_preserves_live_owner": True,
        "native906_identity_checks_actual_player_name": True,
        "native906_timed_chains_complete": False,
        "native906_model_replacement_fade_complete": False,
        "native907_force_history_preserved_on_fixed_reselection": True,
        "LPD_all_target_batch_concrete_backend": True,
        "legacy_LV_actual_model_sidecar_backend": True,
        "legacy_LV_source_is_f32_not_LPD_scaled": True,
        "typed_desktop_owned_sidecar_adapter_in_source": True,
        "native906_secondary_smooth_branch_diagnosed": True,
        "LPD_batch_transactional_source_publish": True,
        "LPD_curve_storage_shared_by_Arc": True,
        "LPD_source_origin_automatically_recovered": False,
        "LV_constructor_origin_verified_zero": True,
        "native924_legacy_LV_branch_wired_in_ordinary_and_task_receiver": True,
        "native924_tick_uses_independent_actual_sound_elapsed": True,
        "native924_unbound_waveform_LPD_branches_explicit_errors": True,
        "zero_duration_instant_selector_timeline_source": True,
        "first_model_timeline_labels_current_subset": 55,
        "first_model_instant_target_labels_remaining": 60,
        "immediate_eye_eyebrow_and_integer_mouth_source": True,
        "actual_LV_entries": sidecar_totals["LV_physical_entries"],
        "actual_LV_distinct_resource_names": sidecar_totals["LV_distinct_full_byte_names"],
        "actual_LV_raw_format_all_finite_aligned": sidecar_totals["LV_all_finite_and_aligned"],
        "native924_automatic_sound_binding_complete": False,
        "NEKO_sound_commands_version_difference_confirmed": True,
        "full_NEKO_strict_frame_claim": False,
        "complete_native_count_added": 0,
        "compiled": False, "tests_run": False, "Rust_execution": False,
        "game_or_DLL_execution": False,
    }
    payload = {
        "schema": "cs2-recovery-A-emote-chain-lpd/v1",
        "task_date": "2026-11-17",
        "generated_at_utc": datetime.now(timezone.utc).isoformat(),
        "binary": {"path": prior["binary_path"], "sha256": exe.digest,
                   "file_version": prior["file_version"]},
        "summary": summary,
        "APIs": {
            "906_unchanged_public_entry": "EmoteServices::import_m2_resource(task,name,bytes,CpuM2Load)",
            "actual_chain": "CpuM2PatternChains::from_expanded/select -> kind,frames,CP932 from/intermediate/to labels",
            "owner_reuse": "CpuM2LiveOwner::reselect_fixed; material/transition state survives fixed selection",
            "LPD_batch": "EmoteServices::bind_m2_lpd(task,&actual_sidecar,actual_source_origin,&[CpuM2LipBinding]) -> bound_count",
            "LPD_target_identity": "playback_info(task).m2_owner.lip_curve_targets",
            "LPD_preset": "CpuM2LipPreset::default is original1a19e0; overrides require real #lipper data",
            "LPD_binding": "CpuM2LipBinding{dimension,pattern_cp932,preset}",
            "all_sidecars": "EmoteServices::bind_m2_lip_sidecar(task,bytes,CpuM2LipSidecarKind,actual_origin,bindings)",
            "LV": "CpuM2LipCurve::from_legacy_lv(bytes,actual_origin,&preset); direct f32 sample and interior smoothing",
            "desktop_typed_adapter": "LoggingHost::emote_bind_sound_lip_sidecar_hook(task,logical_channel,actual_preset_origin,bindings)",
            "actual_924_LV": "LoggingHost::emote_link_legacy_sound_lip -> bind actual owned LV/origin0 ->928(1); update uses sound_elapsed_ticks",
            "two_clocks": "EmoteTick{task_elapsed,lip_elapsed:Option<u32>,increment}; live lip without actual lip_elapsed errors",
            "actual_tick": "advance_with_random -> live_owner.advance_lip -> CPU sampler dictionary",
            "immediate_controls": "emote_immediate_target / evaluate_emote_immediate_controls; timeline zero-duration eye/eyebrow and integer mouth persist in owner",
        },
        "sound_abi": {
            "version_scope": "NEKO2.6.1.76 only; existing desktop sound receiver was recovered from Yukikoi2.6.1.90",
            "wrapper924_args": ["type54 target", "raw enable", "logical sound channel"],
            "command540": {"data0": "logical sound channel", "data1": "raw enable"},
            "commands": sound_commands,
            "roles": {
                "185": "query selected/linked source playing status",
                "200": "underlying audio-device interface virtual+0x1c; stored bank+0x28 by1755d0, passed as loader third argument (unused in LV/LPD open)",
                "211": "enable actual PCM waveform monitor fallback",
                "213": "owned waveform data/capacity pair, not a fabricated scalar signal",
                "219": "accumulated selected sound-task ticks or MAX during pending controls",
                "220": "data1 out pair [LPD pointer,length]",
                "221": "data1 out pair [legacy lip pointer,length]",
                "222": "has either sidecar",
            },
            "clock_correction": "23828a pushes raw219 sound_elapsed,0,1 before task-clock leaf1dd440; those arguments remain for manager17def0;173d30 uses sound_elapsed for B banks while174040 uses task_elapsed",
            "origin_correction": "175460 constructor sets evaluator+24=0;1a1df0 ignores third open argument and has no profile override, so actual LV origin0 is verified",
            "unresolved": "automatic LPD lipper preset overrides and no-sidecar waveform/flow evaluator",
        },
        "actual_sidecar_inventory": sound_inventory,
        "actual_sidecar_totals": sidecar_totals,
        "actual_LV_sample_metadata": legacy_sample,
        "configured_chains": inventories,
        "evidence_snippets": {key: block(exe, *span) for key, span in spans.items()},
        "sources": [{"path": path, "sha256": sha((ROOT/path).read_bytes())} for path in paths],
        "modified_paths": {
            "runtime_and_desktop": [p for p in paths if p.startswith("simulator/")],
            "analysis_scripts": modified_scripts,
            "reports": [
                "reports/recovery_A_20261117_emote_" + stem + suffix
                for stem in ("chain_lpd", "variable_timeline", "integration", "manager", "lip_physics")
                for suffix in (".json", ".md", "_summary.json")
            ] + ["reports/recovery_A_20261117_emote_current_summary.json"],
            "shared_factory_lib_or_original_ledger_modified_in_this_batch": False,
            "README_modified": False,
        },
        "navigation_only": "source_function_bindings_20261117; zero verification or completion inferred",
        "classification": "scripts/analysis + reports; no extracted assets, temporary files, or new directories",
    }
    out = ROOT / "reports/recovery_A_20261117_emote_chain_lpd"
    out.with_suffix(".json").write_text(json.dumps(payload, ensure_ascii=False, indent=2)+"\n", encoding="utf-8")
    compact = {key: value for key, value in payload.items()
               if key not in {"configured_chains", "evidence_snippets", "sources"}}
    compact["configured_chains"] = [
        {key: value for key, value in row.items() if key != "pairs"} for row in inventories
    ]
    out.with_name(out.name+"_summary.json").write_text(json.dumps(compact, ensure_ascii=False, indent=2)+"\n", encoding="utf-8")
    out.with_suffix(".md").write_text(
        "# A / M2 fix复用、LPD批量后端与声音版本ABI（2026-11-17）\n\n"
        "906已新增真实#chain解析和已确认fix分支。同profile固定切换保留material/transition历史，"
        "只改timeline保留整个live owner；身份额外比较实际player_name。"
        "ca01.inc的直接维度1/2/4/6为fix；$eye有30条、$blow有72条linear10帧显式pair，"
        "body.dif的12维为sin_fs30。timed/nomove和旧surface fade仍报未恢复；没有用线性插值替代。\n\n"
        "新增具体backend批量接口EmoteServices::bind_m2_lpd(task,bytes,actual_origin,bindings)，"
        "一次解析实际LPD、验证全部dimension/pattern，全部成功后原子发布新owner；"
        "同preset曲线以Arc共享，后续tick写入真实sampler变量。"
        "playback_info().m2_owner.lip_curve_targets提供实际目标身份。"
        "preset默认0..5、phoneme权重0/1/.4/.2/.8/.6、平滑半径1均来自1a19e0；"
        "不能拿lip_table的50/700 waveform阈值当这些参数。\n\n"
        "立即继续批次已实现实际NEKO使用的legacy LV：pcm_a.int有152条、update00.int有1473条LV，"
        f"全PCM归档加update00共{sidecar_totals['LV_physical_entries']}条，"
        f"按完整资源字节名去重{sidecar_totals['LV_distinct_full_byte_names']}条，"
        "未发现LPB/LPD。去重只针对资源名，不是函数去重或运行完成量。"
        f"只读解密并解释{sidecar_totals['LV_f32_scalars_read_only']}个f32，"
        f"全部有限且4字节对齐={sidecar_totals['LV_all_finite_and_aligned']}；未运行Rust evaluator。"
        "原版175460分配另一evaluator，1a1df0复制无头f32数组并平滑内部点，"
        "1a1f00直接返回数组值；不存在LPD的二次0..5缩放。"
        "CpuM2LipCurve::from_legacy_lv及统一bind_m2_lip_sidecar已落盘。"
        "desktop新增emote_bind_sound_lip_sidecar_hook，读取D实际持有的LPB/LV并送入同一backend。"
        "继续追踪已确认LV装载器第三参数未使用，origin保持构造0；"
        "所以ordinary924及owned command540的实际LV分支已自动接线。"
        "更新hook通过typed sound_elapsed_ticks读取声音累计tick，EmoteTick.lip_elapsed单独传递；"
        "不再把M2 task tick当lip时刻。没有sidecar的waveform分支、自动LPD preset分支仍明确报错。\n\n"
        "重要声音ABI：NEKO2.6.1.76与D当前Yukikoi2.6.1.90命令编号有差异。"
        "NEKO219查累计tick，220/221传出两种sidecar指针/长度，222查是否有sidecar；"
        "185查playing，200查底层设备virtual+1c，211/213用于waveform监视与数据。"
        "924参数是target,enable,channel；540的data0=channel,data1=enable。"
        "200返回值被1755d0放入bank+28，再作为装载器第三参数；"
        "对1a1df0/1a1a90完整装载路径核对后确认该参数未使用，不能据此称为origin。"
        "23828a还原出219的声音时钟与1dd440取得的M2任务时钟分别传给manager；"
        "lip用前者，模型用后者。该更正取代此前阶段对device+1c/origin的推测。"
        "精确分支RVA与原字节见JSON，可供D/main加入按profile的typed适配。\n\n"
        "继续即时控制器批次：driver6750、91a0的零duration分支直接写f32；"
        "132e2..13305对mouth主标签CVTTSS2SI并忽略duration，talkLabel单独处理。"
        "这些入口已并入统一timeline调度表，实际首模型新增「困り」「初始化」；"
        "加上「走行ループ」后源码支持子集55/115，剩60个timed目标标签，"
        "该子集数量不代表strict帧或运行验收数量。\n\n"
        "本批新增完整native计数0，仍不宣称NEKO strict全帧可用。"
        "未编译、未测试、未运行Rust/游戏/DLL。脚本归scripts/analysis，报告归reports；"
        "没有临时产物、资产输出、新目录或README/原ledger改动。\n",
        encoding="utf-8",
    )
    current_path = ROOT / "reports/recovery_A_20261117_emote_current_summary.json"
    current = read_json(current_path)
    current["chain_lpd_batch"] = compact
    current["summary"]["backend_native_branches"]["906"] = (
        "real load/reuse/fixed pattern changes; timed chains and old-surface fade incomplete"
    )
    current["summary"]["backend_native_branches"]["924"] = (
        "owned legacy LV import, direct smoothed f32 sample and independent actual sound ticks wired; LPD/waveform paths incomplete"
    )
    current["summary"]["first_model_timeline_source_subset"] = 55
    current["summary"]["first_model_timed_target_labels_remaining"] = 60
    current["summary"]["actual_LV_distinct_resource_names"] = len(names)
    current["summary"]["actual_LV_all_finite_aligned_read_only"] = sidecar_totals["LV_all_finite_and_aligned"]
    current["summary"]["native924_legacy_LV_ordinary_branch_in_source"] = True
    current["host_APIs"]["tick_binding"] = (
        "state.advance_with_random(task,EmoteTick{task_elapsed,lip_elapsed:actual_sound_elapsed_option,increment:0},shared_rng)"
    )
    current["lifecycle_APIs"]["tick"] = current["host_APIs"]["tick_binding"]
    current["lifecycle_APIs"]["907"] = (
        "M2 load automatically constructs selected live owner; raw PSB needs explicit actual bank binding"
    )
    current["lifecycle_APIs"]["load"] = (
        "import_m2_resource(task,actual_name,Option<&EmoteResource>,CpuM2Load); inspect generation and owner"
    )
    current["summary"]["remaining_strict_blockers"] = [
        "timed M2 chains, model replacement/fade and secondary smooth transitions",
        "native924 automatic LPD preset branch, waveform/flow controls and force/global controls",
        "ordinary target0/raw69/70 addressed task routing",
        "metadata instant target controllers and full stencil/mesh scheduling",
        "nonidentity final display mapping and live-controller save/restore",
    ]
    current["source_hashes_current"] = payload["sources"]
    current_path.write_text(json.dumps(current, ensure_ascii=False, indent=2)+"\n", encoding="utf-8")
    print(json.dumps(summary, ensure_ascii=True))


if __name__ == "__main__":
    main()

"""Fixed-driver clampControl family and actual first-model usage, no execution."""
import json
import struct

from recovery_A_20261116 import ROOT, Image, read_json, sha
from recovery_A_psb_20261117 import Psb, load_sample
from recovery_A_emote_model_render_20261117 import DRIVER_SHA
from recovery_A_receiver_contracts_20261116 import block


def main():
    identity = read_json(ROOT / "reports/recovery_A_20261116_emote.json")
    driver = Image((ROOT / identity["binary_path"]).parent / "emotedriver.dll", DRIVER_SHA)
    name, data = load_sample(0)
    records = Psb(data).root["metadata"]["clampControl"]
    enabled = [r for r in records if r.get("enabled") != 0]
    spans = {"controller_values_then_clamp": (0x136f3, 8),
             "parse_enabled_and_type": (0x1c52d, 0x84),
             "parse_range_and_store_record": (0x1c703, 0x90),
             "base_plus_timeline_offset_inputs": (0x1cd4e, 0x90),
             "type1_radial_limit": (0x1cdde, 0xac),
             "type0_surface_mapping": (0x1ce8a, 0x131),
             "output_to_real_driver_variable_receiver": (0x1cfbb, 0x70),
             "mirror_disabled_is_identity": (0x1d490, 0x1e)}
    summary = {
        "batch": "actual PSB clampControl to parameter cursor and mesh pixels",
        "resource": name, "resource_sha256": sha(data),
        "enabled_records": len(enabled),
        "type0_records": sum(r["type"] == 0 for r in enabled),
        "type1_records": sum(r["type"] == 1 for r in enabled),
        "pipeline": "controller bases + timeline differences -> clampControl type0 surface/type1 radial -> parameter_cursor -> frameList -> mesh/atlas triangles -> CPU RGBA",
        "API": "CpuPlayer::effective_variables; internal clamp::apply",
        "unclamped_controller_values_retained_for_M2_owner": True,
        "duplicate_clamp_records_read_independent_input_banks": True,
        "frame_sampler_receives_actual_clamp_outputs": True,
        "blanket_clampControl_render_rejection_removed": True,
        "mirror_control_remains_separate_explicit_diagnostic": True,
        "pending_final_combined_runtime_validation_not_missing_source": True,
        "remaining_declared_emote_families": 9,
        "closed_entire_families_added": 0, "completed_native_equivalence_added": 0,
        "full_NEKO_strict_pixel_claim": False,
        "compiled": False, "tests_run": False, "Rust_VM_game_DLL_APK_executed": False,
        "syntax_reading": "rustfmt parser only; not typecheck",
        "main_cross_PSB_success_replacement": "source connected by main; not guarded as a whole",
        "main_remaining_replacement_branches": ["nonzero secondary profile", "failed reopen side effects"],
    }
    paths = ["simulator/crates/runtime/src/emote_clamp.rs", "simulator/crates/runtime/src/emote_cpu.rs",
             "simulator/crates/runtime/src/emote.rs", "scripts/analysis/recovery_A_emote_clamp_20261117.py"]
    payload = {"summary": summary,
               "perPE": [{"path": str(driver.path.relative_to(ROOT)), "sha256": driver.digest,
                          "addresses": "RVA", "spans": {k: [a, n] for k, (a, n) in spans.items()},
                          "constants": {hex(a): struct.unpack("<f", driver.get(a, 4))[0]
                                        for a in (0x7dfac, 0x7dfc0, 0x7dfe4, 0x7df9c)}}],
               "actual_enabled_records": enabled,
               "evidence": {key: block(driver, *span) for key, span in spans.items()},
               "sources": [{"path": p, "sha256": sha((ROOT / p).read_bytes())} for p in paths]}
    prefix = ROOT / "reports/recovery_A_20261117_emote_clamp"
    prefix.with_suffix(".json").write_text(json.dumps(payload, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    prefix.with_name(prefix.name + "_summary.json").write_text(
        json.dumps({k: payload[k] for k in ("summary", "perPE")}, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    prefix.with_suffix(".md").write_text(
        "# A / 实际PSB clampControl到帧网格\n\n"
        "新增runtime/emote_clamp.rs；emote.rs仅新增子模块，emote_cpu.rs仅effective_variables和clamp通用警告接线。"
        "不改main import_m2_resource、surface或driver transform。"
        f"{name}启用{len(enabled)}条，type0四条、type1两条；输入均来自实际控制器变量。\n\n"
        "driver13530→136f3→1c930在controller/timeline/offset合成后处理clamp；"
        "1cd67把min/max归一到[-1,1]，type0按斜率/半径正弦余弦曲面映射，"
        "type1超单位圆才按atan2/独立cos/sin限制。两者均保留原先两轴非零门控。"
        "1cfbb还原range并调用真实13710写driver变量；不能把type0简化成矩形或圆形clamp。\n\n"
        "CpuPlayer::effective_variables现在于timeline差分之后执行clamp，随后parameter_cursor→frameList→"
        "实际BP/atlas纹理三角形。原始controller/M2 owner bank不被覆盖；多条clamp读取同一独立输入快照。"
        "enabled mirror仍由现有strict警告拒绝；本批不假定mirror、材料或transition已实现。"
        "9个声明族仍partial，完整native等价新增0，未宣称完整NEKO像素。\n\n"
        "main跨PSB成功替换链已接通，A后续不再称其全部guard；仍保留非零secondary及失败重开副作用。"
        "未编译、测试、Rust/VM/游戏/DLL/APK执行；仅静态PE/资源解析与rustfmt语法读取。"
        "脚本分类scripts/analysis，报告reports，无新目录、临时产物或根README/localization修改。\n", encoding="utf-8")
    print(json.dumps(summary))


if __name__ == "__main__":
    main()

"""Existing scalar receiver -> actual transitionControl parameters; static only."""
import json

from recovery_A_20261116 import ROOT, Image, read_json, sha
from recovery_A_emote_model_render_20261117 import DRIVER_SHA, flatten
from recovery_A_psb_20261117 import Psb, load_sample
from recovery_A_receiver_contracts_20261116 import block


def main():
    identity = read_json(ROOT / "reports/recovery_A_20261116_emote.json")
    driver = Image((ROOT / identity["binary_path"]).parent / "emotedriver.dll", DRIVER_SHA)
    name, data = load_sample(0)
    root = Psb(data).root
    controls = [v for v in root["metadata"]["transitionControl"] if v.get("enabled") != 0]
    labels = {v["label"] for v in controls}
    consumers = []
    for obj_name, obj in root["object"].items():
        for motion_name, motion in obj["motion"].items():
            params = motion.get("parameter", [])
            for index, layer in enumerate(flatten(motion.get("layer", []))):
                parameter = layer.get("parameterize")
                if isinstance(parameter, int) and 0 <= parameter < len(params) and params[parameter].get("id") in labels:
                    consumers.append({"object": obj_name, "motion": motion_name,
                                      "index": index, "label": layer.get("label"),
                                      "type": layer.get("type"), "parameter": params[parameter]["id"]})
    spans = {
        "metadata_transition_parser_dispatch": (0x12647, 0x2c),
        "enabled_flag_and_scalar_dimension_one": (0x19f4c, 0x4c),
        "scalar_current_start_target_zero": (0xd5ec, 0x3c),
        "scalar_arrays_and_queue_release": (0xd654, 0x21),
        "transition_target_existing_scalar": (0x13331, 0x2f),
        "transition_update_existing_scalar": (0x1a250, 0x23),
        "transition_output_bank_write": (0x1a332, 0x17),
        "controller_plus_offsets_to_model": (0x13658, 0x4c),
    }
    summary = {
        "batch": "transition scalar constructor/live output -> PSB parameters -> real geometry/pixels",
        "resource": name, "decoded_sha256": sha(data),
        "enabled_transition_records": len(controls),
        "actual_parameter_consumers": len(consumers),
        "receiver_reused": "CpuM2TimedVariables scalar da10/dbb0; no new M2 owner/queue",
        "new_consumer": "timeline::apply_transition_outputs before additive timelines/clamp",
        "constructor_zero_and_live_base_in_source": True,
        "blanket_enabled_transition_diagnostic_removed": True,
        "API": "existing CpuPlayer::render_strict / CpuEmoteBackend::render_rgba",
        "release": "existing CpuPlayer owns values/timed state; per-frame geometry/masks drop after draw, task unload drops player",
        "remaining": ["other enabled metadata families", "generic shared-kind3/textured-type12 (zero observations in135 PSZ)", "nondefault projection/stencil mode", "final combined runtime/pixel validation"],
        "completed_native_equivalence_added": 0,
        "full_NEKO_strict_pixel_claim": False,
        "compiled": False, "typechecked": False, "tests_run": False, "target_execution": False,
    }
    sources = ["simulator/crates/runtime/src/emote_timeline.rs", "simulator/crates/runtime/src/emote_cpu.rs",
               "scripts/analysis/recovery_A_emote_transition_output_20261117.py"]
    payload = {"summary": summary,
               "perPE": [{"path": driver.path.relative_to(ROOT).as_posix(), "sha256": driver.digest,
                          "addresses": "RVA", "spans": {k: list(v) for k, v in spans.items()}}],
               "controls": controls, "consumers": consumers,
               "evidence": {k: block(driver, *v) for k, v in spans.items()},
               "sources": [{"path": p, "sha256": sha((ROOT / p).read_bytes())} for p in sources]}
    small = {k: payload[k] for k in ("summary", "perPE")}
    prefix = ROOT / "reports/recovery_A_20261117_emote_transition_output"
    for suffix, value in ((".json", payload), ("_summary.json", small)):
        prefix.with_name(prefix.name + suffix).write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    prefix.with_suffix(".md").write_text(
        "# A / 实际transition scalar输出\n\n"
        "emote_timeline::apply_transition_outputs补零默认/live f32输出，emote_cpu::controller_variables在timeline差分/clamp前消费，"
        "删除transitionControl启用即拒绝；已有CpuM2TimedVariables继续持有真实scalar target/time，不新增M2 owner或修改主线import/advance。\n\n"
        "driver SHA85eb459e…93ce：12647→19e80解析，19f4c检查enabled、19f8b→d510(1)，d5f1..d608三个scalar数组初始化0；"
        "13331..1335b向da10发target，1a263经dbb0更新，1a341写变量bank；13658..1369f加差分后送模型。"
        "d654..d674释放原数组，模拟器复用player的timed/value拥有及卸载释放。\n\n"
        f"{name}的{len(controls)}条启用记录有{len(consumers)}处物理参数接收：平移/旋转/透明/引用/mesh采样后进入真实atlas、mask和RGBA。"
        "API仍为render_strict/render_rgba，无宿主新增hook。其它启用控制器和通用图形缺口保留；完整native等价新增0。"
        "只静态PE/PSB与rustfmt语法读取，无编译、类型检查、测试或执行。脚本归scripts/analysis、报告归reports，无新目录/临时资产。\n", encoding="utf8")
    current_path = ROOT / "reports/recovery_A_20261117_emote_current_summary.json"
    current = read_json(current_path)
    current["transition_output_batch"] = small
    current["summary"]["latest_A_batch"] = summary
    current["summary"]["transition_controller_output_consumer"] = "zero/default plus live scalar -> additive offsets/clamp -> actual model parameters/pixels"
    report = prefix.with_suffix(".md").relative_to(ROOT).as_posix()
    if report not in current.setdefault("reports", []):
        current["reports"].append(report)
    for row in payload["sources"]:
        current["source_hashes_current"] = [v for v in current.get("source_hashes_current", []) if v["path"] != row["path"]]
        current["source_hashes_current"].append(row)
    current_path.write_text(json.dumps(current, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    print(json.dumps(summary), flush=True)


if __name__ == "__main__":
    main()

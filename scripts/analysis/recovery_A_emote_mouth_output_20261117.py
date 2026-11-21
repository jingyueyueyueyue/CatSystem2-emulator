"""Bounded mouth output -> actual PSB parameter -> pixel-source evidence."""
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
    controls = [v for v in root["metadata"]["mouthControl"] if v.get("enabled") != 0]
    labels = {v[k] for v in controls for k in ("label", "talkLabel")}
    consumers = []
    for obj_name, obj in root["object"].items():
        for motion_name, motion in obj["motion"].items():
            params = motion.get("parameter", [])
            for index, layer in enumerate(flatten(motion.get("layer", []))):
                parameter = layer.get("parameterize")
                if not isinstance(parameter, int) or not 0 <= parameter < len(params):
                    continue
                if params[parameter].get("id") not in labels:
                    continue
                consumers.append({"object": obj_name, "motion": motion_name,
                                  "layer_index": index, "label": layer.get("label"),
                                  "type": layer.get("type"), "parameter": params[parameter],
                                  "frames": len(layer.get("frameList", []))})
    spans = {
        "beginFrame_parse_and_constructor": (0x19690, 0x8b),
        "constructor_integer_seed_and_zero_talk": (0xbc7a, 0x22),
        "integer_and_independent_talk_target": (0x132e2, 0x4d),
        "talk_zero_duration_write": (0xbe38, 0x3b),
        "talk_queued_scalar_f32_pow": (0xbf10, 0x128),
        "separate_output_values": (0xc038, 0x1d),
        "update_outputs": (0x19c40, 0x2b),
        "label_bank_write_then_talkLabel_lookup": (0x19d2e, 0x1a),
        "talkLabel_bank_write": (0x19e15, 0x1c),
        "controller_plus_offsets_to_model": (0x13658, 0x4c),
    }
    summary = {
        "batch": "mouth independent output lanes -> actual parameterized mouth texture pixels",
        "resource": name, "decoded_sha256": sha(data),
        "enabled_mouth_controllers": len(controls),
        "actual_parameter_consumers": len(consumers),
        "selector_integer_seed_and_live_output": True,
        "talk_zero_seed_and_existing_live_scalar_output": True,
        "mouth_selector_and_talk_are_separate_not_multiplied": True,
        "output_placement": "controller_variables before additive timeline and clamp",
        "pixel_chain": "face_mouth -> actual kind3 source selection; face_talk -> parameter_cursor -> held/interpolated sprite/BP -> atlas alpha/EQUAL/color raster",
        "lifetime": "uses existing CpuPlayer variables/timed state; frame-owned geometry and planes release after render; unload drops player/model references",
        "API": "existing CpuPlayer::render_strict / CpuEmoteBackend::render_rgba; no host hook",
        "remaining": ["other enabled metadata controller families", "generic shared-kind3/textured-type12 branches (zero observations in135 fixed PSZ)", "nondefault stencil render mode1", "full combined runtime/pixel validation deferred"],
        "completed_native_equivalence_added": 0, "historical_partial_family_count": 9,
        "full_NEKO_strict_pixel_claim": False, "compiled": False,
        "typechecked": False, "tests_run": False, "target_execution": False,
        "syntax": "rustfmt parser only",
    }
    sources = ["simulator/crates/runtime/src/emote_timeline.rs",
               "simulator/crates/runtime/src/emote_cpu.rs",
               "scripts/analysis/recovery_A_emote_mouth_output_20261117.py"]
    payload = {"summary": summary,
               "perPE": [{"path": driver.path.relative_to(ROOT).as_posix(), "sha256": driver.digest,
                          "addresses": "RVA", "spans": {k: list(v) for k, v in spans.items()}}],
               "controls": controls, "consumers": consumers,
               "evidence": {k: block(driver, *v) for k, v in spans.items()},
               "sources": [{"path": p, "sha256": sha((ROOT / p).read_bytes())} for p in sources]}
    small = {k: payload[k] for k in ("summary", "perPE")}
    prefix = ROOT / "reports/recovery_A_20261117_emote_mouth_output"
    for suffix, value in ((".json", payload), ("_summary.json", small)):
        prefix.with_name(prefix.name + suffix).write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    prefix.with_suffix(".md").write_text(
        "# A / 实际mouth输出到纹理像素\n\n"
        "emote_timeline::apply_mouth_outputs接通beginFrame整数默认、实时整数选择及talk零默认/既有scalar输出；"
        "emote_cpu::controller_variables在timeline差分和clamp之前消费，移除mouthControl一律拒绝。"
        "这是两个独立模型参数，无猜测乘法或静态姿势代替实时输出。原timed状态/主线M2 owner不改。\n\n"
        "固定driver SHA85eb459e…93ce：19690..1971b解析beginFrame并构造bc20；bc7a..bc9b写整数种子及talk0；"
        "132e2..1332f分别写整数和浮点target；bf10/c038输出两值；19c5b更新后19d3e/19e2c分别写label/talkLabel。"
        "13658..1369f继续原base+offset→模型参数。\n\n"
        f"{name}启用{len(controls)}个mouth控制器、{len(consumers)}处真实parameter接收："
        "face_mouth选择引用，face_talk采样实际口型atlas/BP，再走已有mesh/stencil/color RGBA。"
        "渲染临时几何/plane随帧释放，卸载复用现有player/model释放，无新后台对象或宿主接线。\n\n"
        "保留其它启用controller及通用stencil缺口；不声称完整NEKO像素，整native等价新增0。"
        "仅只读PE/PSB和rustfmt语法读取，无编译、类型检查、测试、Rust/游戏执行。"
        "脚本归scripts/analysis、报告归reports；无临时资产或新目录。\n", encoding="utf8")
    current_path = ROOT / "reports/recovery_A_20261117_emote_current_summary.json"
    current = read_json(current_path)
    current["mouth_output_batch"] = small
    current["summary"]["latest_A_batch"] = summary
    current["summary"]["mouth_controller_output_consumer"] = "integer selector and separate live scalar talk -> actual PSB parameters -> atlas/BP pixels"
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

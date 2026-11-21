"""Actual terminal-frame/priority cursor evidence; no target/raster execution."""
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
    layers, priorities = [], []
    for obj_name, obj in root["object"].items():
        for motion_name, motion in obj["motion"].items():
            for index, layer in enumerate(flatten(motion.get("layer", []))):
                frames = layer.get("frameList", [])
                if len(frames) >= 2 and frames[-1].get("type") == 0:
                    layers.append({"object": obj_name, "motion": motion_name, "index": index,
                                   "label": layer.get("label"), "type": layer.get("type"),
                                   "penultimate_time": frames[-2]["time"],
                                   "terminal_time": frames[-1]["time"]})
            frames = motion.get("priority", [])
            if len(frames) >= 2 and frames[-1].get("content") is None:
                priorities.append({"object": obj_name, "motion": motion_name,
                                   "penultimate_time": frames[-2]["time"],
                                   "terminal_time": frames[-1]["time"]})
    spans = {
        "priority_parser_count174": (0x34e3d, 0x33),
        "priority_seek_clamps_count_minus_two": (0x3b60b, 0x1a),
        "priority_forward_stops_at_penultimate": (0x3c54a, 0x15),
        "layer_seek_clamps_count_minus_two": (0x3bc3c, 0x23),
        "layer_forward_stops_at_penultimate": (0x3c845, 0xf),
        "parameter_layer_stops_at_penultimate": (0x3bf22, 0xc),
        "sample_current_hidden_returns": (0x3d6d4, 0x10),
        "hidden_next_disables_interpolation": (0x3d6ee, 0xa),
    }
    summary = {
        "batch": "terminal frame/priority boundary -> retained actual sprites and stencil sources",
        "resource": name, "decoded_sha256": sha(data),
        "actual_layer_lists_with_type0_terminal": len(layers),
        "actual_priority_lists_with_null_terminal": len(priorities),
        "source_delta": "sample_emote_layer and priority_order exclude final boundary from current records; forward and seek use penultimate current",
        "pixel_chain": "late object/parameter cursor -> held real layer contents + nonempty priority -> expanded references/mesh -> sampled atlas mask/EQUAL -> color RGBA",
        "release": "no new persistent state; retained per-frame Arc ranges and stencil planes release after existing render",
        "global_clock_and_M2_owner_unchanged": True,
        "API": "existing CpuPlayer::render_strict / CpuEmoteBackend::render_rgba",
        "remaining": ["generic stencil shared-range/type12 variants without actual input", "nondefault projection/render mode and other enabled metadata families", "full combined runtime/pixel validation deferred"],
        "completed_native_equivalence_added": 0,
        "full_NEKO_strict_pixel_claim": False,
        "compiled": False, "typechecked": False, "tests_run": False, "target_execution": False,
    }
    sources = ["simulator/crates/runtime/src/emote_timeline.rs", "simulator/crates/runtime/src/emote_cpu.rs",
               "scripts/analysis/recovery_A_emote_terminal_frame_20261117.py"]
    payload = {"summary": summary,
               "perPE": [{"path": driver.path.relative_to(ROOT).as_posix(), "sha256": driver.digest,
                          "addresses": "RVA", "spans": {k: list(v) for k, v in spans.items()}}],
               "actual_layers": layers, "actual_priorities": priorities,
               "evidence": {k: block(driver, *v) for k, v in spans.items()},
               "sources": [{"path": p, "sha256": sha((ROOT / p).read_bytes())} for p in sources]}
    small = {k: payload[k] for k in ("summary", "perPE")}
    prefix = ROOT / "reports/recovery_A_20261117_emote_terminal_frame"
    for suffix, value in ((".json", payload), ("_summary.json", small)):
        prefix.with_name(prefix.name + suffix).write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    prefix.with_suffix(".md").write_text(
        "# A / 实际终止帧与priority边界\n\n"
        "sample_emote_layer与priority_order改为把最后一条仅当next边界，current最多到倒数第二条。"
        "此前time到达末尾会选中type0/null，把持续显示层和整个priority源列表清空；本批保留真实前一内容和源列表，"
        "继续已有引用展开、世界Z排序、mesh、alpha mask/EQUAL和颜色RGBA，不生成替代图元。\n\n"
        "固定driver SHA85eb459e…93ce：34e3d解析priority及count174；3b617/3b61c将随机seek限制到count-2；"
        "3c54a/3c553阻止priority前进到最后一条。3bc40/3bc50同样限制layer随机seek，3c845/3c84c和"
        "3bf27/3bf2a分别限制普通/parameter layer前进。3d6ee禁止向hidden next插值，所以末尾type0不是持续hidden current。\n\n"
        f"{name}中{len(layers)}个layer列表和{len(priorities)}个priority列表具有该实际末尾布局。"
        "不改main的M2时钟、import/advance或target生产者，仅修绘制时采样消费者。"
        "无新增持久对象；当前帧geometry/Arc range/mask plane仍由既有绘制结束释放。\n\n"
        "完整native等价新增0，未宣称完整NEKO像素；仅静态PE/PSB及rustfmt语法读取。"
        "无编译、类型检查、测试或执行。脚本归scripts/analysis、报告归reports，无新目录/临时资产。\n", encoding="utf8")
    current_path = ROOT / "reports/recovery_A_20261117_emote_current_summary.json"
    current = read_json(current_path)
    current["terminal_frame_batch"] = small
    current["summary"]["latest_A_batch"] = summary
    current["summary"]["terminal_frame_priority_consumers"] = {"layer_lists": len(layers), "priority_lists": len(priorities)}
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

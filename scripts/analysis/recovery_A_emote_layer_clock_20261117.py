"""Fixed per-layer clock selection and actual parameter-parent descendants."""
from collections import Counter
import json

from recovery_A_20261116 import ROOT, Image, read_json, sha
from recovery_A_emote_model_render_20261117 import DRIVER_SHA
from recovery_A_psb_20261117 import Psb, load_sample
from recovery_A_receiver_contracts_20261116 import block


def main():
    identity = read_json(ROOT / "reports/recovery_A_20261116_emote.json")
    driver = Image((ROOT / identity["binary_path"]).parent / "emotedriver.dll", DRIVER_SHA)
    name, data = load_sample(0)
    rows, counts = [], Counter()
    for obj_name, obj in Psb(data).root["object"].items():
        for motion_name, motion in obj["motion"].items():
            def visit(layer, ancestors):
                parameter = layer.get("parameterize")
                if ancestors and parameter is None:
                    kind = layer.get("type", 2)
                    counts["kind_" + str(kind)] += 1
                    rows.append({"object": obj_name, "motion": motion_name,
                                 "label": layer.get("label"), "kind": kind,
                                 "parameterized_ancestor_labels": ancestors})
                child_ancestors = ancestors + [layer.get("label")] if parameter is not None else ancestors
                for child in layer.get("children", []):
                    visit(child, child_ancestors)
            for layer in motion.get("layer", []):
                visit(layer, [])
    spans = {"sample_selects_own_parameter_or_player_clock": (0x3d6f8, 0x1a),
             "parameter_seek_reads_own_cursor": (0x3beed, 0x21),
             "parameter_cursor_conversion_already_present": (0x327cc, 0x6d)}
    summary = {
        "batch": "physical child keeps actual motion clock while each parameterized layer selects its own cursor",
        "resource": name, "decoded_sha256": sha(data),
        "actual_unparameterized_descendants_below_parameter_layers": len(rows),
        "descendants_by_kind": dict(counts),
        "source_delta": "RenderContext::layer recursion passes original motion time, not parent's parameter_cursor result",
        "pixel_chain": "independent child frame sample -> kind3 expansion/blank stencil/BP/atlas -> real mask/EQUAL/color raster",
        "applies_to_color_and_mask_source_passes": True,
        "API": "CpuEmoteBackend::render_rgba; no state/host changes",
        "remaining": ["independent/stateful reference time modes and nonzero motion mask", "general nonneutral color/curve/projection/stencil variants", "full dynamic/pixel validation"],
        "completed_native_equivalence_added": 0, "full_NEKO_strict_pixel_claim": False,
        "compiled": False, "typechecked": False, "tests_run": False, "target_execution": False,
    }
    paths = ["simulator/crates/runtime/src/emote_cpu.rs", "scripts/analysis/recovery_A_emote_layer_clock_20261117.py"]
    small = {"summary": summary, "perPE": [{"sha256": driver.digest, "addresses": "RVA",
             "spans": {k: list(v) for k, v in spans.items()}}]}
    payload = {**small, "actual_descendants": rows, "evidence": {k: block(driver, *v) for k, v in spans.items()},
               "sources": [{"path": p, "sha256": sha((ROOT / p).read_bytes())} for p in paths]}
    prefix = ROOT / "reports/recovery_A_20261117_emote_layer_clock"
    for suffix, value in ((".json", payload), ("_summary.json", small)):
        prefix.with_name(prefix.name + suffix).write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    prefix.with_suffix(".md").write_text(
        "# A / 物理子层独立采样时钟\n\n"
        "layer递归现传原motion time，各层再单独选择自己的parameter_cursor；先前传父层参数游标，"
        "会使无parameterize的子层跟着父层的变量跳时间。3d6f8..3d712明确只读当前layer+c的parameter+28，"
        "为空时读player+104；3befd同样单独读取当前参数游标。父层仍提供已有几何／alpha继承。\n\n"
        f"{name}有{len(rows)}个实际无参数子孙位于参数父层之下，按kind为{dict(counts)}。"
        "同一接收器服务颜色与mask源，因此真实嵌套motion、blank stencil和atlas/BP统一消费修正后的帧。"
        "无新持久状态、宿主API或main command/import/advance修改。引用motion自己的状态式timeOffset／非零motion.mask仍待恢复，"
        "不据本次修正宣称整个时钟族完成。无编译、类型检查、测试或执行；仅grammar读取。\n", encoding="utf8")
    current_path = ROOT / "reports/recovery_A_20261117_emote_current_summary.json"
    current = read_json(current_path)
    current["physical_layer_clock_batch"] = small
    current["summary"]["latest_A_batch"] = summary
    report = prefix.with_suffix(".md").relative_to(ROOT).as_posix()
    if report not in current.setdefault("reports", []):
        current["reports"].append(report)
    for row in payload["sources"]:
        current["source_hashes_current"] = [v for v in current.get("source_hashes_current", []) if v["path"] != row["path"]]
        current["source_hashes_current"].append(row)
    current_path.write_text(json.dumps(current, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    rolling_path = ROOT / "reports/recovery_A_20261117_rolling_delivery.md"
    rolling = rolling_path.read_text(encoding="utf8")
    note = "再交付`recovery_A_20261117_emote_layer_clock.md`：物理递归传motion时钟，各层独立选parameter游标，首模型92个无参数子孙不再被父游标污染；已有几何／alpha继承保留。颜色与mask共用实际接收器，无主线时钟owner修改。\n\n"
    if note not in rolling:
        rolling_path.write_text(rolling.replace("# A / 本轮小批交付\n\n", "# A / 本轮小批交付\n\n" + note, 1), encoding="utf8")
    print(json.dumps(summary, ensure_ascii=True), flush=True)


if __name__ == "__main__":
    main()

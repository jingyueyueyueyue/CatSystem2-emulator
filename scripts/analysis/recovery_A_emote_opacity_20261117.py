"""Publish the actual byte-opacity pixel chain from fixed PE/resource bytes."""
import json

from recovery_A_20261116 import ROOT, Image, read_json, sha
from recovery_A_emote_model_render_20261117 import DRIVER_SHA, flatten
from recovery_A_psb_20261117 import Psb, load_sample
from recovery_A_receiver_contracts_20261116 import block


def main():
    identity = read_json(ROOT / "reports/recovery_A_20261116_emote.json")
    driver = Image((ROOT / identity["binary_path"]).parent / "emotedriver.dll", DRIVER_SHA)
    scope = read_json(ROOT / "archive/2026-11/20261117_emote_pixel_scope.json")
    colors = read_json(ROOT / "archive/2026-11/20261117_emote_color_actual_probe.json")
    name, data = load_sample(0)
    consumers = []
    opacity_intervals = []
    for obj_name, obj in Psb(data).root["object"].items():
        for motion_name, motion in obj["motion"].items():
            params = motion.get("parameter", [])
            for index, layer in enumerate(flatten(motion.get("layer", []))):
                frames = [f for f in layer.get("frameList", []) if "opa" in (f.get("content") or {})]
                if not frames:
                    continue
                parameter = layer.get("parameterize")
                consumers.append({"object": obj_name, "motion": motion_name, "index": index,
                                  "label": layer.get("label"), "kind": layer.get("type"),
                                  "parameter": params[parameter] if isinstance(parameter, int) else None,
                                  "inherit_mask": layer.get("inheritMask"), "frames": frames})
                for a, b in zip(layer.get("frameList", []), layer.get("frameList", [])[1:]):
                    ac, bc = a.get("content") or {}, b.get("content") or {}
                    if a.get("type") == 3 and b.get("type") != 0 and ac.get("opa", 255) != bc.get("opa", 255):
                        opacity_intervals.append({"object": obj_name, "motion": motion_name, "index": index,
                                                  "label": layer.get("label"), "from_time": a["time"], "to_time": b["time"],
                                                  "from_opacity": ac.get("opa", 255), "to_opacity": bc.get("opa", 255)})
    spans = {
        "opacity_default": (0x37274, 7),
        "parsed_opacity_low_byte": (0x3751f, 0x50),
        "held_sample_opacity": (0x3de98, 9),
        "interpolation_then_byte_round": (0x3d9d0, 0x73),
        "nonnegative_round_half_up": (0x29550, 0x42),
        "skip_effective_parent_containers": (0x3e810, 0x42),
        "implicit_root_opacity": (0x3e7a3, 0x14),
        "opacity_inherit_or_implicit_root": (0x3eb9b, 0x3b),
        "referenced_motion_root_receives_caller_opacity": (0x41923, 0x39),
        "primitive_consumes_effective_opacity": (0x44cad, 0x66),
        "packed_alpha_floor_divide_255": (0x2bb60, 0x2b),
    }
    summary = {
        "batch": "actual nonopaque frame -> byte hierarchy -> atlas/BP alpha pixels",
        "resources_in_fixed_inventory": len(scope["resources"]),
        "actual_nonopaque_frame_records": scope["summary"]["nonopaque_layer_frames"],
        "first_resource": name, "first_decoded_sha256": sha(data),
        "first_resource_layers_with_explicit_opacity": len(consumers),
        "first_resource_actual_opacity_interpolation_intervals": len(opacity_intervals),
        "explicit_non_neutral_colors": colors["summary"].get("nonneutral_color_frames", 0),
        "explicit_color_frames_all_neutral": colors["summary"]["neutral_color_frames"],
        "actual_observed_drawing_guards": {k: scope["summary"].get(k, 0) for k in (
            "enabled_bustControl", "enabled_hairControl", "enabled_loopControl", "enabled_partsControl",
            "nonlocal_coordinate", "other_mesh_sync_bits", "unknown_layer_kind", "non32_bp",
            "bp_nonone_mesh_transform", "reference_motion_mask", "curve_ccc", "curve_acc",
            "curve_zcc", "curve_scc", "curve_occ", "curve_cp")},
        "source_delta": ["byte endpoints and round-half-up opacity interpolation",
                         "floor(local*effective_parent/255) at each physical layer",
                         "inheritMask400 implicit-root fallback and400000 opacity-parent skip",
                         "nested references retain caller byte alpha; mask texture alpha remains unattenuated"],
        "pixel_chain": "parameter_cursor -> sample_emote_layer -> Transform byte opacity -> existing atlas/BP triangles -> bilinear sample -> source-over RGBA",
        "lifetime": "frame-owned byte transforms only; existing geometry/Arc/mask planes drop at end of render and unload",
        "API": "existing CpuPlayer::render_strict / CpuEmoteBackend::render_rgba; no host hooks",
        "remaining": ["nonneutral/per-corner color and nonzero blend mode (zero observed in135PSZ)",
                      "independent reference inheritance, generic projection/curves/mesh/stencil variants",
                      "combined dynamic-owner and pixel validation deferred"],
        "completed_native_equivalence_added": 0, "full_NEKO_strict_pixel_claim": False,
        "compiled": False, "typechecked": False, "tests_run": False, "target_execution": False,
    }
    paths = ["simulator/crates/runtime/src/emote_timeline.rs", "simulator/crates/runtime/src/emote_cpu.rs",
             "scripts/analysis/recovery_A_emote_opacity_20261117.py", "scripts/analysis/recovery_A_emote_pixel_scope_20261117.py",
             "scripts/analysis/recovery_A_emote_color_20261117.py"]
    small = {"summary": summary, "perPE": [{"path": driver.path.relative_to(ROOT).as_posix(),
             "sha256": driver.digest, "addresses": "RVA", "spans": {k: list(v) for k, v in spans.items()}}]}
    payload = {**small, "first_resource_consumers": consumers, "first_resource_interpolation_intervals": opacity_intervals,
               "evidence": {k: block(driver, *v) for k, v in spans.items()},
               "sources": [{"path": p, "sha256": sha((ROOT / p).read_bytes())} for p in paths]}
    prefix = ROOT / "reports/recovery_A_20261117_emote_opacity"
    for suffix, value in ((".json", payload), ("_summary.json", small)):
        prefix.with_name(prefix.name + suffix).write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    prefix.with_suffix(".md").write_text(
        "# A / 实际透明度帧到像素\n\n"
        "固定135份PSZ的6936条显式颜色均为0x808080ff；非中性／角颜色、非零bm未命中，后置。"
        "现有绘制guard相关的非默认坐标、额外mesh同步位、非32点BP、BP非1 meshTransform、非空曲线／路径、"
        "motion.mask、启用bust/hair/loop/parts也均未命中。这里只说明固定输入范围，零观察不计实现，不概括所有动态状态。\n\n"
        "本批接3240条实际opa非255帧的消费（包括全透明端点）：timeline的opa按byte端点插值后取整，物理层每级floor(local*parent/255)，"
        "inheritMask400清除时用当前motion隐式root，400000容器不成为后代的opacity parent。"
        "引用motion把caller的byte alpha继续传入。sprite/BP仍进入真实纹理双线性采样与RGBA合成，"
        "mask仍只用纹理原alpha，不被层alpha衰减；没有替代图形或preview成功。\n\n"
        "PE SHA85eb459e…93ce：37569为输入低byte，3d9d0..3da3d为插值后29550取整；"
        "3eb9b..3ebd0为继承／root与255除法；3e828..3e850跳过400000容器；"
        "41929..41953传递引用root alpha；44cde..44d12与2bb60把有效alpha送入真实primitive颜色。"
        f"{name}的{len(consumers)}个实际层、{len(opacity_intervals)}个透明度插值区间、参数与帧见JSON。\n\n"
        "仍缺非中性／角颜色与非零bm、独立reference继承及通用projection/curve/mesh/stencil分支；"
        "不把静态样本零命中叫作全模型成功。完整native等价新增0，像素验收未执行。"
        "只改timeline和CPU绘制区；保留main command/import/advance及911–917/920/921/966/909。"
        "无编译、类型检查、测试或目标执行；rustfmt只读grammar。脚本归scripts/analysis、报告归reports、"
        "逐资源与语法临时证据归archive/2026-11，无新目录。\n", encoding="utf8")
    current_path = ROOT / "reports/recovery_A_20261117_emote_current_summary.json"
    current = read_json(current_path)
    current["opacity_pixel_batch"] = small
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
    note = "当前新增：`recovery_A_20261117_emote_opacity.md`接通135PSZ共3240条opa非255帧（包括全透明端点）的byte插值／逐层alpha／引用root／实际纹理与BP像素；mask保留原纹理alpha。6936条显式颜色全中性，角颜色／非零bm及本次指定其它绘制guard未命中，保留通用诊断。现有API不变；无编译、类型检查、测试或执行。\n\n"
    rolling = rolling.replace("135PSZ共3240条非全透明帧", "135PSZ共3240条opa非255帧（包括全透明端点）")
    if note not in rolling:
        rolling_path.write_text(rolling.replace("# A / 本轮小批交付\n\n", "# A / 本轮小批交付\n\n" + note, 1), encoding="utf8")
    print(json.dumps(summary, ensure_ascii=True), flush=True)


if __name__ == "__main__":
    main()

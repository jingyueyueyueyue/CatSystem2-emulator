"""Static XY world-depth -> expanded primitive sort -> CPU pixel evidence."""
from collections import Counter
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
    coords, records = Counter(), []
    for obj_name, obj in root["object"].items():
        for motion_name, motion in obj["motion"].items():
            for index, layer in enumerate(flatten(motion.get("layer", []))):
                coords[str(layer.get("coordinate", 0))] += 1
                for frame in layer.get("frameList", []):
                    content = frame.get("content") or {}
                    coord = content.get("coord") or []
                    if len(coord) == 3 and coord[2] != 0:
                        records.append({"object": obj_name, "motion": motion_name, "layer_index": index,
                                        "label": layer.get("label"), "kind": layer.get("type"),
                                        "time": frame.get("time"), "coord": coord})
    spans = {
        "XY_parent_transforms_xy_and_adds_world_z": (0x3ead3, 0xb9),
        "default_coordinate_callback_is_identity": (0x3e576, 0x2e),
        "identity_projection_callback_returns_without_mutation": (0x2dd60, 0x3),
        "priority_list_reverse_primitive_insertion": (0x44a33, 0x34),
        "primitive_keeps_player_priority": (0x44bd2, 0x9),
        "world_z_copied_to_primitive24": (0x44c13, 0x9),
        "full_color_list_sort_before_virtual_draw": (0x483d, 0x3c),
        "descending_z_then_player_priority_comparator": (0x2dd20, 0x31),
        "stable_sort_wrapper_uses_comparator": (0x4bcc0, 0x6b),
        "sort_insertion_and_merge_paths": (0x4d590, 0x96),
        "referenced_motion_inherits_common_player_priority": (0x42df9, 0x16),
        "default_player_priority_zero": (0x304f0, 0xa),
        "mesh_raster_vertex_z_zero": (0x58b44, 0x6f),
    }
    summary = {
        "batch": "default XY world Z -> global expanded primitive stable sort -> real mesh/stencil pixels",
        "API": "existing CpuPlayer::render_strict / CpuEmoteBackend::render_rgba; no new host hook",
        "source": "Transform.z -> collect_motion -> Arc<MotionPrimitives>/OwnedStencilScope -> stable depth sort -> real alpha/EQUAL sprite raster",
        "sort": "descending finite f32 world Z; native reverse-priority insertion order on equal Z/common player priority",
        "nested_motion_primitives_sorted_together": True,
        "same_complete_owner_chain_reuses_actual_mask_plane_within_frame": True,
        "nonzero_Z_no_longer_misclassified_as_perspective": True,
        "resource": {"name": name, "sha256": sha(data), "physical_coordinate_modes": dict(coords),
                     "nonzero_Z_frame_records": len(records),
                     "affected_physical_layers": len({(v["object"], v["motion"], v["layer_index"]) for v in records})},
        "remaining": ["nondefault coordinate/projection/stereo modes", "composite kind3 owner reused as a direct mask source", "textured type12 targets/reversal", "mode1 stencil targets", "nonempty cc curves/non32 BP", "enabled controller composition diagnostics"],
        "completed_native_equivalence_added": 0,
        "historical_partial_families": 9,
        "full_NEKO_strict_pixel_claim": False,
        "compiled": False, "tests_run": False, "target_execution": False,
        "syntax": "rustfmt parser only, no typecheck",
    }
    paths = ["simulator/crates/runtime/src/emote_cpu.rs", "scripts/analysis/recovery_A_emote_depth_order_20261117.py"]
    payload = {"summary": summary,
               "perPE": [{"path": driver.path.relative_to(ROOT).as_posix(), "sha256": driver.digest,
                          "addresses": "RVA", "spans": {k: list(v) for k, v in spans.items()}}],
               "actual_depth_records": records,
               "evidence": {k: block(driver, *span) for k, span in spans.items()},
               "sources": [{"path": p, "sha256": sha((ROOT / p).read_bytes())} for p in paths]}
    prefix = ROOT / "reports/recovery_A_20261117_emote_depth_order"
    small = {k: payload[k] for k in ("summary", "perPE")}
    for suffix, value in ((".json", payload), ("_summary.json", small)):
        prefix.with_name(prefix.name + suffix).write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    prefix.with_suffix(".md").write_text(
        "# A / 实际NEKO嵌套图元世界Z排序\n\n"
        "emote_cpu.rs局部渲染接收改为先收集再绘制：Transform.z按f32累加；"
        "collect_motion按原priority反向插入并展开引用motion，保留Arc<MotionPrimitives>及完整owner scopes。"
        "全部真实颜色图元统一stable sort后，以原source bindings写mask，再经EQUAL绘制atlas/mesh三角形。"
        "同一帧同一完整owner链复用真实plane，避免每个sprite重复展开mask motion。宿主API不变。\n\n"
        "证据：3eaea..3eb84在coordinate0独立累加世界Z；44c13写primitive+24；"
        "483d..4876在全部展开后调用4bcc0→2dd20，按Z降序、再按player priority排序。"
        "42e02把同一player priority传引用motion；44a50逆读priority。58720的实际网格顶点Z=0，"
        "所以本分支Z是painter order，不能按非零值一律拒绝为未实现透视。"
        "非默认coordinate仍明确诊断；不宣称实现stereo/projection callback。\n\n"
        f"{name}的525物理层均coordinate0，{len(records)}条非零Z frame记录现在有实际排序消费者。"
        "原BP组合/mesh祖先/纹理和stencil范围保持，未改main import/advance/force/settings/banks/surface。\n\n"
        "剩余：非默认坐标/投影、kind3 composite owner复用为直接mask源的共享range、textured type12、mode1、非空cc/非32BP及启用controller组合。"
        "完整native等价新增0，不宣称完整NEKO像素。只静态PE/PSB与rustfmt语法读取，无编译/测试/执行。"
        "脚本归scripts/analysis，报告归reports，无新目录或临时资产。\n", encoding="utf8")
    current_path = ROOT / "reports/recovery_A_20261117_emote_current_summary.json"
    current = read_json(current_path)
    current["depth_order_batch"] = small
    current["summary"]["latest_A_batch"] = summary
    current["summary"]["default_XY_nonzero_Z_source_consumer"] = "global stable expanded primitive sort;50 actual first-model frame records"
    for row in payload["sources"]:
        current["source_hashes_current"] = [v for v in current.get("source_hashes_current", []) if v["path"] != row["path"]]
        current["source_hashes_current"].append(row)
    report = prefix.with_suffix(".md").relative_to(ROOT).as_posix()
    if report not in current.setdefault("reports", []):
        current["reports"].append(report)
    current_path.write_text(json.dumps(current, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    print(json.dumps(summary, ensure_ascii=True))


if __name__ == "__main__":
    main()

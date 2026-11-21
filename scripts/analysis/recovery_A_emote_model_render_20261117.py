"""Actual NEKO PSB render blockers; bounded PE/resource reads, no renderer execution."""
from collections import Counter
import json

from recovery_A_20261116 import ROOT, Image, read_json, sha
from recovery_A_psb_20261117 import Psb, load_sample
from recovery_A_receiver_contracts_20261116 import block

DRIVER_SHA = "85eb459e94e74ab608f649eb7fb440f2526e90aaa1af7276ed01dc10018903ce"


def flatten(layers):
    result = []
    def visit(layer):
        result.append(layer)
        for child in layer.get("children", []):
            visit(child)
    for layer in layers:
        visit(layer)
    return result


def inventory(root):
    counts, colors, owners, markers = Counter(), Counter(), [], []
    for obj, data in root["object"].items():
        for motion, data in data["motion"].items():
            layers = flatten(data.get("layer", []))
            for index, layer in enumerate(layers):
                kind = layer.get("type", 2)
                counts[str(kind)] += 1
                if kind == 1:
                    markers.append({"object": obj, "motion": motion, "index": index,
                                    "label": layer.get("label"),
                                    "children": len(layer.get("children", []))})
                if layer.get("stencilType", 0):
                    indices = data.get("layerIndexMap", {})
                    refs = []
                    for label in layer.get("stencilCompositeMaskLayerList", []):
                        source = indices.get(label)
                        refs.append({"label": label, "index": source,
                                     "kind": layers[source].get("type", 2)
                                     if isinstance(source, int) and 0 <= source < len(layers) else None})
                    owners.append({"object": obj, "motion": motion, "index": index,
                                   "label": layer.get("label"), "kind": kind,
                                   "stencil_type": layer["stencilType"], "sources": refs})
                for frame in layer.get("frameList", []):
                    content = frame.get("content") or {}
                    if "color" in content:
                        colors[str(content["color"])] += 1
    return {"physical_layers": sum(counts.values()), "layer_types": dict(counts),
            "explicit_colors": dict(colors), "markers": markers, "stencil_owners": owners}


def main():
    identity = read_json(ROOT / "reports/recovery_A_20261116_emote.json")
    driver = Image((ROOT / identity["binary_path"]).parent / "emotedriver.dll", DRIVER_SHA)
    name, data = load_sample(0)
    observed = inventory(Psb(data).root)
    summary = {
        "batch": "actual model neutral color and leaf marker render paths",
        "resource": name, "resource_sha256": sha(data),
        "physical_layers": observed["physical_layers"],
        "layer_types": observed["layer_types"],
        "neutral_explicit_color_records": observed["explicit_colors"].get(str(0x808080ff), 0),
        "other_explicit_color_records": sum(v for k, v in observed["explicit_colors"].items()
                                             if k != str(0x808080ff)),
        "leaf_marker_records": sum(v["children"] == 0 for v in observed["markers"]),
        "marker_with_children_still_diagnostic": True,
        "neutral_colors_reach_actual_texture_triangle_rasterizer": True,
        "leaf_markers_emit_no_fabricated_pixels": True,
        "composite_owner_count_still_pending": len(observed["stencil_owners"]),
        "closed_subbranches": ["neutral packed color identity", "type1 leaf no-primitive gate"],
        "remaining_declared_emote_families": 9,
        "full_NEKO_strict_pixel_claim": False, "completed_native_equivalence_added": 0,
        "compiled": False, "tests_run": False, "Rust_VM_game_DLL_APK_executed": False,
    }
    spans = {"neutral_color_identity": (0x2c070, 0x21),
             "nonneutral_packed_combine": (0x2c0b8, 0x91),
             "reference_group_membership": (0x44ac1, 0x10f),
             "ordinary_primitive_kinds_and_list_insert": (0x44dd8, 0x52)}
    sources = ["simulator/crates/runtime/src/emote_cpu.rs",
               "scripts/analysis/recovery_A_emote_model_render_20261117.py"]
    payload = {"summary": summary,
               "perPE": [{"path": str(driver.path.relative_to(ROOT)), "sha256": driver.digest,
                          "addresses": "RVA", "spans": {k: [a, n] for k, (a, n) in spans.items()}}],
               "resource_inventory": observed,
               "evidence": {key: block(driver, *span) for key, span in spans.items()},
               "sources": [{"path": p, "sha256": sha((ROOT / p).read_bytes())} for p in sources]}
    prefix = ROOT / "reports/recovery_A_20261117_emote_model_render"
    prefix.with_suffix(".json").write_text(json.dumps(payload, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    prefix.with_name(prefix.name + "_summary.json").write_text(
        json.dumps({k: payload[k] for k in ("summary", "perPE")}, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    prefix.with_suffix(".md").write_text(
        "# A / 实际NEKO模型的小批渲染接收\n\n"
        "直接修改runtime/emote_cpu.rs的layer/sprite；保持main的surface/import/advance及根变换接线。"
        f"只读解析{name}：{observed['physical_layers']}物理层；"
        f"{summary['neutral_explicit_color_records']}处显式颜色全部为0x808080ff，"
        f"{summary['leaf_marker_records']}个type1均无子层。\n\n"
        "driver SHA85eb459e…93ce，RVA2c079/2c085把该DWORD作为颜色乘法恒等值；"
        "sprite现在只放行该有证据值，保留其他颜色的明确诊断。"
        "44b71的0x1401掩码只为0/10/12生成图元，type3另走引用分支；"
        "无子层type1现在占用物理/priority索引但不造图元或形状填充，有子层变体仍诊断。\n\n"
        "两项均直接进入既有strict纹理/mesh三角形CPU路径；没有新增preview、白矩形或静态肖像。"
        f"本模型{len(observed['stencil_owners'])}个composite owner仍需闭合range/nested调度，"
        "非neutral颜色、type1子层及其余9族不宣称完成。完整native等价新增0，不宣称完整NEKO像素。\n\n"
        "未编译、测试、运行Rust/VM/游戏/DLL/APK；只做PE/资源静态分析和rustfmt语法读取。"
        "脚本归scripts/analysis，报告归reports；无新目录、临时产物、localization或根README修改。\n", encoding="utf-8")
    print(json.dumps(summary))
    print(json.dumps(observed["stencil_owners"], ensure_ascii=False))


if __name__ == "__main__":
    main()

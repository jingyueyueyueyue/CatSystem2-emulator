"""Inventory only the two requested actual PSZ stencil branches; no rendering."""
from collections import Counter
import argparse
import json

from recovery_A_20261116 import ROOT, sha
from recovery_A_emote_model_render_20261117 import flatten
from recovery_A_psb_20261117 import Psb, load_sample


def publish(payload):
    summary = {
        "batch": "actual PSZ stencil branch relevance",
        **payload["summary"],
        "actual_input_scope": "all 135 PSZ in the fixed NEKOPARA resource inventory",
        "zero_observations_do_not_implement_generic_branches": True,
        "generic_shared_group_and_textured_type12_diagnostics_retained": True,
        "existing_blank_type12_consumer": "explicit source ranges -> atlas/BP triangles -> alpha GREATER64 -> linked stencil EQUAL -> color RGBA",
        "completed_native_equivalence_added": 0,
        "full_NEKO_strict_pixel_claim": False,
        "compiled": False, "typechecked": False, "tests_run": False, "target_executed": False,
    }
    small = {"summary": summary, "perPE": [{
        "sha256": "85eb459e94e74ab608f649eb7fb440f2526e90aaa1af7276ed01dc10018903ce",
        "addresses": "RVA", "existing_evidence": "reports/recovery_A_20261117_emote_group_owner.json",
        "shared_group_rebuild": "44f69..45037",
        "type12_parameters": "44e81..44eeb",
    }]}
    prefix = ROOT / "reports/recovery_A_20261117_emote_stencil_resources"
    for suffix in (".json", "_summary.json"):
        prefix.with_name(prefix.name + suffix).write_text(json.dumps(small, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    prefix.with_suffix(".md").write_text(
        "# A / 实际PSZ的stencil分支范围\n\n"
        "固定135份PSZ逐份只读结构统计：kind3自身owner=0，作为另一owner直接源的composite kind3共享范围=0，"
        "textured type12层/帧=0。逐资源decoded SHA及空结果见archive/2026-11/20261117_emote_stencil_actual_probe.json。"
        "这三个通用分支仍保留明确诊断，不能称为本资源集的实际像素阻断，也不据零观察计为实现。\n\n"
        "实际blank type12已有源范围→atlas/BP三角形→alpha GREATER64→linked stencil EQUAL→真实RGBA消费者；"
        "本批不新增无实际输入的shared-kind3/textured-type12或非空cc/skin推测。后续优先实际模型仍启用的接收链。"
        "原版44f69..45037共享范围重建及44e81..44eeb的type12参数保留为未实现通用分支证据。\n\n"
        "完整native等价新增0；未编译、类型检查、测试或执行。脚本归scripts/analysis、报告归reports、逐资源临时证据归archive/2026-11；无新目录。\n", encoding="utf8")
    current_path = ROOT / "reports/recovery_A_20261117_emote_current_summary.json"
    current = json.loads(current_path.read_text(encoding="utf8"))
    current["actual_stencil_resource_scope"] = small
    report = prefix.with_suffix(".md").relative_to(ROOT).as_posix()
    if report not in current.setdefault("reports", []):
        current["reports"].append(report)
    current_path.write_text(json.dumps(current, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    print(json.dumps(summary), flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--report-only", action="store_true", help="publish the saved fixed inventory without rescanning")
    args = parser.parse_args()
    path = ROOT / "archive/2026-11/20261117_emote_stencil_actual_probe.json"
    if args.report_only:
        publish(json.loads(path.read_text(encoding="utf8")))
        return
    resources = []
    counts = Counter()
    for index in range(135):
        name, data = load_sample(index)
        root = Psb(data).root
        shared, textured, owners = [], [], []
        for obj_name, obj in root["object"].items():
            for motion_name, motion in obj["motion"].items():
                layers = flatten(motion.get("layer", []))
                indices = motion.get("layerIndexMap", {})
                for layer_index, layer in enumerate(layers):
                    kind = layer.get("type", 2)
                    stencil = layer.get("stencilType", 0) or 0
                    location = {"object": obj_name, "motion": motion_name,
                                "index": layer_index, "label": layer.get("label")}
                    if kind == 3 and stencil:
                        owners.append({**location, "stencil_type": stencil})
                    if stencil & 4:
                        for label in layer.get("stencilCompositeMaskLayerList", []):
                            source_index = indices.get(label)
                            if not isinstance(source_index, int) or not 0 <= source_index < len(layers):
                                continue
                            source = layers[source_index]
                            source_stencil = source.get("stencilType", 0) or 0
                            if source.get("type") == 3 and source_stencil & 4:
                                shared.append({**location, "source_index": source_index,
                                               "source_label": label, "source_stencil_type": source_stencil,
                                               "source_composite_list": source.get("stencilCompositeMaskLayerList", [])})
                    if kind == 12:
                        frames = [f for f in layer.get("frameList", [])
                                  if (f.get("content") or {}).get("src") not in (None, "blank")]
                        if frames:
                            textured.append({**location, "stencil_type": stencil,
                                             "layer": {k: v for k, v in layer.items() if k != "children"},
                                             "textured_frames": frames})
        counts["resources"] += 1
        counts["kind3_owner_layers"] += len(owners)
        counts["direct_shared_group_source_pairs"] += len(shared)
        counts["resources_with_shared_source"] += bool(shared)
        counts["textured_type12_layers"] += len(textured)
        counts["textured_type12_frame_records"] += sum(len(v["textured_frames"]) for v in textured)
        counts["resources_with_textured_type12"] += bool(textured)
        resources.append({"index": index, "name": name, "decoded_sha256": sha(data),
                          "kind3_owners": owners, "shared_sources": shared, "textured_type12": textured})
        if (index + 1) % 15 == 0:
            print(json.dumps({"read": index + 1, "shared_pairs": counts["direct_shared_group_source_pairs"],
                              "textured_layers": counts["textured_type12_layers"]}), flush=True)
    payload = {"summary": dict(counts), "resources": resources}
    path.write_text(json.dumps(payload, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    publish(payload)


if __name__ == "__main__":
    main()

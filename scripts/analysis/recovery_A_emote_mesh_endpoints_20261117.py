"""Actual-model BP endpoint inventory and bounded original default-lattice evidence."""
from collections import Counter
import json
import struct

from recovery_A_20261116 import ROOT, Image, read_json, sha
from recovery_A_psb_20261117 import Psb, load_sample
from recovery_A_emote_model_render_20261117 import DRIVER_SHA, flatten
from recovery_A_receiver_contracts_20261116 import block


def main():
    identity = read_json(ROOT / "reports/recovery_A_20261116_emote.json")
    driver = Image((ROOT / identity["binary_path"]).parent / "emotedriver.dll", DRIVER_SHA)
    name, data = load_sample(0)
    counts = Counter()
    colors = Counter()
    for obj in Psb(data).root["object"].values():
        for motion in obj["motion"].values():
            for layer in flatten(motion.get("layer", [])):
                frames = layer.get("frameList", [])
                for a, b in zip(frames, frames[1:]):
                    x, y = a.get("content"), b.get("content")
                    if a.get("type") != 3 or not x or not y:
                        continue
                    pa = (x.get("mesh") or {}).get("bp") or []
                    pb = (y.get("mesh") or {}).get("bp") or []
                    if pa or pb:
                        counts[f"{len(pa)}->{len(pb)}"] += 1
                    if x.get("color") != y.get("color"):
                        colors[f"{x.get('color')}->{y.get('color')}"] += 1
    spans = {"one_empty_endpoint_global_substitution": (0x3db15, 0xc3),
             "identity_lattice_initialization": (0x296d0, 0x1c0),
             "f32_pair_interpolation": (0x2bebf, 0x53),
             "exact_keyframe_raw_copy_gate": (0x3d75e, 0x6a),
             "four_neutral_color_defaults": (0x37245, 0x24),
             "one_color_interpolation": (0x3da73, 0x36)}
    summary = {"batch": "missing BP endpoint -> native identity lattice -> interpolated mesh -> atlas pixels",
               "resource": name, "resource_sha256": sha(data), "interpolated_BP_pairs": dict(counts),
               "previously_blocked_actual_pairs": counts["0->32"] + counts["32->0"],
               "existing_32_to_32_pairs": counts["32->32"],
               "default_color_endpoint_pairs": dict(colors),
               "default_color_pairs_source_completed": sum(colors.values()),
               "source_API": "sample_emote_layer -> existing CpuBezierPatch/grid -> RenderContext::sprite -> CPU triangles",
               "missing_endpoint": "16 row-major (index&3)/3, (index>>2)/3 f32 pairs",
               "both_missing_keeps_no_active_BP": True,
               "exact_keyframe_raw_copy_preserved": True,
               "remaining": ["nonempty cc/skin", "non32 BP", "corner/nonneutral colors", "kind3 itself owning stencil", "active own BP stencil activation", "mode1 targets"],
               "completed_native_equivalence_added": 0, "newly_closed_entire_families": 0,
               "compiled": False, "tests_run": False, "target_execution": False,
               "full_NEKO_strict_pixel_claim": False, "syntax": "rustfmt parser only"}
    paths = ["simulator/crates/runtime/src/emote_timeline.rs", "scripts/analysis/recovery_A_emote_mesh_endpoints_20261117.py"]
    payload = {"summary": summary,
               "perPE": [{"path": str(driver.path.relative_to(ROOT)), "sha256": driver.digest,
                          "addresses": "RVA", "spans": {k: [a, n] for k, (a, n) in spans.items()},
                          "identity_divisor_RVA": "7dfdc", "identity_divisor_f32": struct.unpack("<f", driver.get(0x7dfdc, 4))[0]}],
               "evidence": {k: block(driver, *s) for k, s in spans.items()},
               "sources": [{"path": p, "sha256": sha((ROOT / p).read_bytes())} for p in paths]}
    prefix = ROOT / "reports/recovery_A_20261117_emote_mesh_endpoints"
    for suffix, value in ((".json", payload), ("_summary.json", {k: payload[k] for k in ("summary", "perPE")})):
        (prefix.parent / (prefix.name + suffix)).write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    prefix.with_suffix(".md").write_text(
        "# A / 实际BP缺省端点\n\n"
        "只改emote_timeline.rs：一端缺BP时按driver3db7d/3dba4补global97df8，"
        "296d0初始化16对(x/3,y/3)；随后走既有f32插值→BP组合/网格→atlas纹理三角形。"
        "首模型150对0→32、129对32→0此前明确阻断，本批补齐源码；另95对32→32保留。"
        "两端都空仍无active BP；精确关键帧保留原始copy，未制造形变或替代像素。\n\n"
        "同批3724b..3725f证明四角缺省0x808080ff；首模型另36对显式neutral与缺省color之间的"
        "插值补齐生产，继续走既有packed DWORD核和真实texture raster。非neutral/corner仍诊断。\n\n"
        "cc/skin、非32BP继续诊断；本批未修改main force/import/surface/update或宿主。"
        "整族/native等价新增0，不宣称已执行的NEKO像素。仅静态PE/PSB解析和rustfmt语法读取，"
        "未编译/测试/运行。脚本归scripts/analysis，报告归reports，无新目录/临时资产/README修改。\n", encoding="utf8")
    current_path = ROOT / "reports/recovery_A_20261117_emote_current_summary.json"
    current = read_json(current_path)
    current["mesh_endpoints_batch"] = {k: payload[k] for k in ("summary", "perPE")}
    current["summary"]["latest_A_batch"] = summary
    current["summary"]["remaining_strict_blockers"][2] = (
        "main connected material/inertia input -> spring -> driver -> PSB for14 configs; dynamic groups/enable/explicit impulse/window input remain main-owned")
    current["summary"]["main_material_force_owner"] = "main source delivery reports/emote_force_20261117.md; A did not modify force/import/advance"
    report = prefix.with_suffix(".md").relative_to(ROOT).as_posix()
    if report not in current["reports"]:
        current["reports"].append(report)
    for row in payload["sources"]:
        current["source_hashes_current"] = [r for r in current["source_hashes_current"] if r["path"] != row["path"]]
        current["source_hashes_current"].append(row)
    current_path.write_text(json.dumps(current, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    print(json.dumps(summary))


if __name__ == "__main__":
    main()

"""Inventory actual PSZ color consumers; static resource reads, no raster run."""
from collections import Counter
import json

from recovery_A_20261116 import ROOT, sha
from recovery_A_emote_model_render_20261117 import flatten
from recovery_A_psb_20261117 import Psb, load_sample

NEUTRAL = 0x808080ff


def main():
    totals = Counter()
    resources = []
    for index in range(135):
        name, data = load_sample(index)
        root = Psb(data).root
        counts = Counter()
        examples = []
        colors = Counter()
        guards = Counter()
        for obj_name, obj in root["object"].items():
            for motion_name, motion in obj["motion"].items():
                for layer_index, layer in enumerate(flatten(motion.get("layer", []))):
                    kind = layer.get("type", 2)
                    for frame in layer.get("frameList", []):
                        content = frame.get("content") or {}
                        if content.get("bm", 0):
                            guards["nonzero_blend_mode"] += 1
                        if "color" not in content:
                            continue
                        value = content["color"]
                        counts["explicit_color_frames"] += 1
                        colors[json.dumps(value, ensure_ascii=True, separators=(",", ":"))] += 1
                        neutral = value == NEUTRAL or isinstance(value, list) and all(v == NEUTRAL for v in value)
                        if neutral:
                            counts["neutral_color_frames"] += 1
                            continue
                        counts["nonneutral_color_frames"] += 1
                        counts["corner_color_frames" if isinstance(value, list) else "uniform_color_frames"] += 1
                        counts["nonneutral_kind_" + str(kind)] += 1
                        if len(examples) < 12:
                            examples.append({"object": obj_name, "motion": motion_name,
                                             "layer_index": layer_index, "label": layer.get("label"),
                                             "kind": kind, "inherit_mask": layer.get("inheritMask"),
                                             "time": frame["time"], "color": value,
                                             "source": content.get("src"), "icon": content.get("icon")})
        counts["resources"] = 1
        counts["resources_with_nonneutral"] = int(bool(counts["nonneutral_color_frames"]))
        counts["resources_with_corner_color"] = int(bool(counts["corner_color_frames"]))
        totals.update(counts)
        resources.append({"index": index, "name": name, "decoded_sha256": sha(data),
                          "counts": dict(counts), "color_values": dict(colors),
                          "other_observed_guards": dict(guards), "examples": examples})
        if (index + 1) % 15 == 0:
            print(json.dumps({"read": index + 1, "nonneutral": totals["nonneutral_color_frames"]}), flush=True)
    path = ROOT / "archive/2026-11/20261117_emote_color_actual_probe.json"
    path.write_text(json.dumps({"summary": dict(totals), "resources": resources}, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    print(json.dumps(dict(totals)), flush=True)


if __name__ == "__main__":
    main()

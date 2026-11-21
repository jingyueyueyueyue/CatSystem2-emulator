"""Find actual inputs of existing drawing/timeline guards; no raster execution."""
from collections import Counter
import json

from recovery_A_20261116 import ROOT, sha
from recovery_A_emote_model_render_20261117 import flatten
from recovery_A_psb_20261117 import Psb, load_sample


def observe(root):
    counts, examples = Counter(), {}
    for key in ("bustControl", "hairControl", "loopControl", "partsControl"):
        entries = root.get("metadata", {}).get(key, [])
        active = [e for e in entries if e.get("enabled", 1)]
        counts["enabled_" + key] = len(active)
        if active:
            examples[key] = active[:2]
    for obj_name, obj in root["object"].items():
        for motion_name, motion in obj["motion"].items():
            for index, layer in enumerate(flatten(motion.get("layer", []))):
                loc = {"object": obj_name, "motion": motion_name, "index": index,
                       "label": layer.get("label"), "kind": layer.get("type", 2)}
                if layer.get("coordinate", 0):
                    counts["nonlocal_coordinate"] += 1
                    examples.setdefault("coordinate", dict(loc, value=layer["coordinate"]))
                if layer.get("meshSyncChildMask", 0) & ~8:
                    counts["other_mesh_sync_bits"] += 1
                if layer.get("type", 2) not in (0, 2, 3, 12) and not (layer.get("type") == 1 and not layer.get("children")):
                    counts["unknown_layer_kind"] += 1
                for frame in layer.get("frameList", []):
                    content = frame.get("content") or {}
                    for key in ("ccc", "acc", "zcc", "scc", "occ", "cp"):
                        if content.get(key) is not None:
                            counts["curve_" + key] += 1
                            examples.setdefault(key, dict(loc, time=frame["time"], value=content[key]))
                    bp = content.get("mesh", {}).get("bp") or []
                    if bp and len(bp) != 32:
                        counts["non32_bp"] += 1
                    if bp and layer.get("meshTransform", 0) != 1:
                        counts["bp_nonone_mesh_transform"] += 1
                    if content.get("opa", 255) != 255:
                        counts["nonopaque_layer_frames"] += 1
                        examples.setdefault("opacity", dict(loc, time=frame["time"], opacity=content["opa"], inherit_mask=layer.get("inheritMask")))
                    if (content.get("motion") or {}).get("mask", 0):
                        counts["reference_motion_mask"] += 1
                        examples.setdefault("reference_motion", dict(loc, value=content["motion"]))
    return dict(counts), examples


def main():
    totals, resources = Counter(), []
    for index in range(135):
        name, data = load_sample(index)
        counts, examples = observe(Psb(data).root)
        totals.update(counts)
        resources.append({"index": index, "name": name, "decoded_sha256": sha(data),
                          "counts": counts, "examples": examples})
        if (index + 1) % 15 == 0:
            print(json.dumps({"read": index + 1, "observed": dict(totals)}), flush=True)
    output = ROOT / "archive/2026-11/20261117_emote_pixel_scope.json"
    output.write_text(json.dumps({"summary": dict(totals), "resources": resources}, ensure_ascii=False, indent=2) + "\n", encoding="utf8")


if __name__ == "__main__":
    main()

"""One fixed first-model parameter/priority/atlas declaration slice, no evaluation."""
from collections import Counter
import json

from recovery_A_20261116 import ROOT, sha
from recovery_A_emote_model_render_20261117 import flatten
from recovery_A_psb_20261117 import Psb, load_sample


def main():
    name, raw = load_sample(0)
    if sha(raw) != "cd715016fcf54e1abf3fac020516c538a72eef5be90d2edf3314afd5a8fb5114":
        raise ValueError("fixed first-model identity changed")
    root = Psb(raw).root
    counts = Counter()
    motions, masks, textures = [], Counter(), []
    for source, data in root["source"].items():
        texture = data.get("texture")
        if texture is not None:
            textures.append({"source": source, "texture": texture,
                "icon_count": len(data.get("icon", {})),
                "icon_attr": dict(Counter(str(icon.get("attr")) for icon in data.get("icon", {}).values())),
                "icon_rectangles": data.get("icon", {})})
    for obj, data in root["object"].items():
        for motion, data in data["motion"].items():
            layers = flatten(data.get("layer", []))
            counts["layers"] += len(layers)
            for layer in layers:
                masks[str(layer.get("inheritMask"))] += 1
            motions.append({"object": obj, "motion": motion,
                "parameter": data.get("parameter"), "parameterize": data.get("parameterize"),
                "variable": data.get("variable"), "priority": data.get("priority"),
                "layer_bindings": [{"label": layer.get("label"), "kind": layer.get("type"),
                    "parameterize": layer.get("parameterize"), "inheritMask": layer.get("inheritMask")}
                    for layer in layers]})
            for p in data.get("parameter", []):
                counts["parameters"] += 1
                counts["enabled_" + str(p.get("enabled"))] += 1
    result = {"resource": name, "sha256": sha(raw), "spec": root.get("spec"),
        "counts": dict(counts), "inherit_masks": dict(masks), "textures": textures,
        "motions": motions, "status": "declarations only; no cursor/priority/texture algorithm executed"}
    out = ROOT / "archive/2026-11/20261117_emote_owner_consumers.json"
    out.write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    print(json.dumps({"counts": dict(counts), "inherit_masks": dict(masks),
        "textures": [{k:v for k,v in row.items() if k != "icon_rectangles"} for row in textures]}, ensure_ascii=True))


if __name__ == "__main__":
    main()

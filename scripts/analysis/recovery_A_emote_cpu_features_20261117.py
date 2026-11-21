"""Bounded original-driver and real PSB evidence for incremental CPU features.

Never loads a game/DLL/SDK or executes Rust; no compilation/tests. Reads one
PSB and selected original DLL snippets, not a whole CFG candidate sweep.
"""
from __future__ import annotations

from collections import Counter
from datetime import datetime, timezone
import json

from recovery_A_20261116 import ROOT, Image, sha
from recovery_A_receiver_contracts_20261116 import block
from recovery_A_psb_20261117 import Psb, load_sample

DRIVER_SHA = "85eb459e94e74ab608f649eb7fb440f2526e90aaa1af7276ed01dc10018903ce"


def model_inventory(root):
    layers, frame_types, content_fields, flags = Counter(), Counter(), Counter(), Counter()
    samples, count = {}, 0

    def walk(value, path):
        nonlocal count
        if isinstance(value, dict):
            if "frameList" in value and "children" in value:
                count += 1
                kind = value.get("type")
                layers[kind] += 1
                frame_types.update(str(f.get("type")) for f in value["frameList"])
                for field in ["stencilType", "coordinate", "meshTransform", "meshCombine",
                              "transformOrder", "inheritMask"]:
                    flags[field + "=" + str(value.get(field))] += 1
                short = {k: v for k, v in value.items()
                         if k not in {"children", "frameList"}}
                short["child_labels"] = [c.get("label") for c in value["children"]]
                short["frame_sample"] = value["frameList"][:2]
                samples.setdefault(str(kind), {"path": path, "layer": short})
                for frame in value["frameList"]:
                    content_fields.update(frame.get("content", {}).keys())
            for key, child in value.items():
                walk(child, path + "/" + key)
        elif isinstance(value, list):
            for i, child in enumerate(value):
                walk(child, path + "/" + str(i))
    walk(root["object"], "/object")
    return {
        "layer_count": count, "layer_type_counts": dict(sorted(layers.items())),
        "frame_type_counts": frame_types, "content_field_counts": content_fields,
        "layer_flag_counts": flags, "layer_samples": samples,
        "root_easing_count": len(root.get("easing", [])),
    }


def main():
    path = next(ROOT.glob("*NEKO*/nekopara_vol3/emotedriver.dll"))
    image = Image(path, DRIVER_SHA)
    name, data = load_sample(0)
    psb = Psb(data)
    anchors = {
        "frame_type_flags": (0x3709A, 0x29),
        "frame_hidden_flag": (0x371A8, 0x16),
        "frame_defaults": (0x37245, 0x4B),
        "interval_parser": (0x379D2, 0x55),
        "linear_ratio_and_hidden_next_guard": (0x3D6D4, 0xB0),
        "linear_zoom": (0x3D82D, 0x5F),
        "angle_shortest_turn": (0x2B8F0, 0xA3),
        "linear_vector_no_path": (0x2BC1A, 0x6C),
        "linear_mesh_points_SIMD": (0x2BEA6, 0xAC),
        "linear_mesh_points_scalar": (0x2BF66, 0x63),
        "transform_dispatch": (0x3D1DA, 0x38),
        "transform_mirror_zoom": (0x3D212, 0xBA),
        "transform_rotation": (0x3D2CC, 0xC2),
        "transform_shear": (0x3D38E, 0x95),
        "stencil_property_parser": (0x35D8C, 0x49),
        "stencil_composite_list_parser": (0x36D26, 0x4F),
        "mesh_control_point_parser": (0x3834E, 0x74),
        "texture_RGBA8_path": (0x54FC0, 0x80),
    }
    snippets = {key: block(image, *value) for key, value in anchors.items()}
    sources = [
        "simulator/crates/runtime/src/emote.rs",
        "simulator/crates/runtime/src/emote_cpu.rs",
        "simulator/crates/runtime/src/emote_timeline.rs",
        "scripts/analysis/recovery_A_emote_cpu_features_20261117.py",
    ]
    payload = {
        "schema": "cs2-recovery-A-emote-cpu-features/v1",
        "task_date": "2026-11-17",
        "generated_at_utc": datetime.now(timezone.utc).isoformat(),
        "batch": "timeline-and-affine",
        "original_driver": {"path": str(path.relative_to(ROOT)), "sha256": DRIVER_SHA},
        "resource": {"name": name, "PSB_sha256": sha(data), "decoded_values": psb.nodes},
        "model_inventory": model_inventory(psb.root),
        "APIs": {
            "sampler": "sample_emote_layer(&PsbValue,time:f64)->Result<CpuTimelineSample>",
            "angle": "interpolate_emote_angle(a:f32,b:f32,t:f32)->Result<f32>",
            "affine": "emote_layer_affine(layer,content)->Result<[f64;6]>",
            "renderer": "CpuPlayer::render_strict / CpuEmoteBackend::render_rgba use these helpers",
            "diagnostics": "CpuTimelineSample.warnings feed strict renderer; no preview promotion",
        },
        "implemented_in_source": [
            "Sorted finite frameList validation and type0/type2/type3 handling",
            "Next-type0 guard: hold current content until hidden boundary",
            "Raw f32 linear scalar/vector and32-value BP endpoint interpolation",
            "content.ti positive integer quantization of elapsed frame time",
            "Angle shortest-turn crossing and original once-only360 wrap",
            "Actual zx/zy zoom,sx/sy shear,fx/fy mirrors and four-element transformOrder",
            "Nonlinear curves,coordinate paths,missing BP identity and missing color endpoint defaults remain warnings",
        ],
        "snippets": snippets,
        "transform_jump_table_RVA": 0x3D428,
        "transform_case_RVAs": [image.u32(0x3D428 + n*4)-image.base for n in range(4)],
        "source_files": [{"path": p, "sha256": sha((ROOT/p).read_bytes())} for p in sources],
        "verification": {
            "method": "Original bytes + real PSB structural inventory + rustfmt parser only",
            "compiled": False, "tests_run": False, "game_or_DLL_executed": False,
            "full_NEKO_strict_frame_claim": False, "integrated_completed_native_count": 0,
        },
        "next_blockers": [
            "General stencil joined/reference-motion scheduling (local literal sources connected in the next mesh/stencil batch)",
            "meshTransform/meshCombine hierarchy ownership and meshSyncChildMask (BP basis and own grids recovered in the next batch)",
            "3D projection,packed RGBA color modes,nonlinear curves/controller physics",
        ],
        "follow_up_reports": [
            "reports/recovery_A_20261117_emote_mesh_stencil.json",
            "reports/recovery_A_20261117_emote_texture_color.json",
        ],
    }
    prefix = ROOT / "reports/recovery_A_20261117_emote_cpu_features"
    prefix.with_suffix(".json").write_text(json.dumps(payload, ensure_ascii=False, indent=2)+"\n",
                                           encoding="utf-8")
    inv = payload["model_inventory"]
    lines = [
        "# A: strict CPU timeline/affine batch", "",
        "Original driver/PSB reads only; no compile/tests or engine execution.", "",
        "## Available source/API", "",
    ]
    lines += [f"- `{v}`" for v in payload["APIs"].values()]
    lines += ["", "## Actual source changes", ""]
    lines += ["- "+v for v in payload["implemented_in_source"]]
    lines += [
        "", "## Evidence", "",
        f"Driver SHA256 `{DRIVER_SHA}`.",
        "Frame parser0x3709a maps0->hidden,2->held,3->interpolation. "
        "Evaluator0x3d6e4 requires interpolated current and nonhidden next;0x3d712 "
        "quantizes by ti then divides elapsed by endpoint duration.",
        "0x2b8f0 is the shortest-angle helper. 0x2bc24 performs plain3-component "
        "lerp when no coordinate path is bound;0x2be30 interpolates BP coordinate pairs.",
        "0x3d428 is the transform dispatch table:0 mirror,1 rotate,2 zoom,3 shear. "
        "The old generic scale-vector path has been replaced by the original scalar fields.",
        "", "## Real first-model inventory", "",
        f"- {inv['layer_count']} actual layers, counts{dict(inv['layer_type_counts'])}.",
        f"- Keyframe types{dict(inv['frame_type_counts'])}; root easing entries{inv['root_easing_count']}.",
        "- These are structural counts, not completed native or strict pixel counts.",
        "", "## Remaining blockers", "",
    ]
    lines += ["- "+v for v in payload["next_blockers"]]
    lines += [
        "- Full NEKO strict frames remain blocked; the new sampler does not bypass other warnings.",
        "", "## Classification", "",
        "New helper stays runtime/emote*; analysis script scripts/analysis; durable JSON/md reports/. "
        "No host exports/dispatch,README,game assets or temporary files were changed.",
    ]
    prefix.with_suffix(".md").write_text("\n".join(lines)+"\n", encoding="utf-8")
    print(json.dumps({"layers": inv["layer_count"], "types": inv["layer_type_counts"],
                      "keyframes": inv["frame_type_counts"], "compiled": False}, ensure_ascii=True))


if __name__ == "__main__":
    main()

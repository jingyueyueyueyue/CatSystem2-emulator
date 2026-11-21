"""Read-only mesh/stencil evidence and source handoff; no engine/Rust execution."""
from __future__ import annotations

from collections import Counter
from datetime import datetime, timezone
import json

from recovery_A_20261116 import ROOT, Image, sha
from recovery_A_receiver_contracts_20261116 import block
from recovery_A_psb_20261117 import Psb, load_sample

DRIVER_SHA = "85eb459e94e74ab608f649eb7fb440f2526e90aaa1af7276ed01dc10018903ce"


def inventory(root):
    flags, controls, bindings = Counter(), Counter(), []
    for object_name, obj in root["object"].items():
        for motion_name, motion in obj["motion"].items():
            layers = []

            def walk(layer):
                layers.append(layer)
                for frame in layer["frameList"]:
                    mesh = frame.get("content", {}).get("mesh")
                    if mesh:
                        bp = mesh.get("bp")
                        controls["bp_" + ("null" if bp is None else str(len(bp)))] += 1
                        controls["cc_" + ("null" if mesh.get("cc") is None else "non-null")] += 1
                for key in ["meshTransform", "meshCombine", "meshDivision", "meshSyncChildMask"]:
                    flags[key + "=" + str(layer.get(key))] += 1
                for child in layer["children"]:
                    walk(child)
            for layer in motion["layer"]:
                walk(layer)
            for i, layer in enumerate(layers):
                kind = layer.get("stencilType", 0)
                if not kind & 4:
                    continue
                row = {"object": object_name, "motion": motion_name,
                       "owner_layer_index": i, "owner_label": layer["label"],
                       "stencil_type": kind, "sources": [], "ignored": []}
                for label in layer["stencilCompositeMaskLayerList"]:
                    index = motion["layerIndexMap"].get(label)
                    if index is None or layers[index]["type"] not in {0, 3}:
                        row["ignored"].append(label)
                    else:
                        row["sources"].append({
                            "index": index, "type": layers[index]["type"],
                            "label": label,
                        })
                bindings.append(row)
    return {"layer_mesh_fields": flags, "keyframe_control_points": controls,
            "composite_mask_bindings": bindings}


def main():
    path = next(ROOT.glob("*NEKO*/nekopara_vol3/emotedriver.dll"))
    image = Image(path, DRIVER_SHA)
    name, data = load_sample(0)
    psb = Psb(data)
    observed = inventory(psb.root)
    anchors = {
        "bicubic_scalar": (0x2C2A9, 0x27D),
        "bicubic_SIMD_entry": (0x2C150, 0x159),
        "additive_identity_offset": (0x2BDB0, 0x76),
        "meshDivision_parser": (0x35F91, 0x99),
        "quality_default_one": (0x30785, 0x1B),
        "quality_setter": (0x25C0, 0x3D),
        "own_grid_aspect_split": (0x3905D, 0x6F),
        "inherited_grid_aspect_split": (0x3FF9E, 0x42),
        "parent_BP_projection": (0x3F72D, 0xD0),
        "composite_label_lookup_and_type_gate": (0x3A5FB, 0x6E),
        "composite_source_flag": (0x3A6BD, 0xD),
        "mask_reference_pass_loop": (0x4FF0, 0xAF),
        "mask_reference_finalize": (0x55D5, 0x32),
        "D3D_stencil_alpha_state": (0x5780F, 0xC6),
        "D3D_stencil_increment_equal": (0x57A96, 0x44),
        "D3D_stencil_decrement_equal": (0x57BDE, 0x45),
        "D3D_stencil_test_equal_keep": (0x57CCD, 0x93),
    }
    sources = [
        "simulator/crates/runtime/src/emote.rs",
        "simulator/crates/runtime/src/emote_cpu.rs",
        "simulator/crates/runtime/src/emote_mesh.rs",
        "simulator/crates/runtime/src/emote_stencil.rs",
        "scripts/analysis/recovery_A_emote_mesh_stencil_20261117.py",
    ]
    summary = {
        "mesh_patch_points": 16,
        "mesh_basis_recovered": "bicubic Bernstein; raw coordinates, no unit-domain clamp",
        "mesh_combination_operator_recovered": "destination += source - identity",
        "own_grid_rule_recovered": "trunc(meshDivision*quality); split by width/(width+height); +1 each axis",
        "driver_default_quality": 1.0,
        "actual_model_composite_mask_owners": len(observed["composite_mask_bindings"]),
        "actual_model_resolved_mask_sources": sum(len(b["sources"]) for b in observed["composite_mask_bindings"]),
        "stencil_pixel_operators_implemented": True,
        "local_type5_literal_source_rasterization_connected": True,
        "PSB_type5_scheduler_integrated": False,
        "additive_mesh_hierarchy_integrated": False,
        "full_NEKO_strict_frame_claim": False,
        "integrated_completed_native_count": 0,
        "compiled": False, "tests_run": False, "original_code_executed": False,
    }
    apis = {
        "patch": "CpuBezierPatch::from_interleaved/identity/evaluate/combine_offsets",
        "grid": "CpuMeshGrid::from_division(width,height,division,quality)",
        "quality": "CpuEmoteBackend::set_mesh_quality_scale(quality)",
        "mask_owners": "CpuMotionModel::stencil_bindings(object,motion)",
        "raw_mask_owners": "resolve_emote_stencil_bindings(&motion)",
        "stencil": "CpuStencilBuffer::new/clear/write_fragment/test_equal",
        "textured_mask": "rasterize_emote_stencil_triangle(buffer,vertices,texture,opacity,reference,operation)",
        "RGBA_mask": "CpuStencilBuffer::write_rgba_mask(single_primitive,reference,operation)",
        "gated_RGBA_pixels": "CpuStencilBuffer::blend_rgba_equal(destination,source,reference)",
        "strict": "CpuEmoteBackend::render_rgba still rejects unresolved model warnings before Canvas writes",
        "local_type5": "RenderContext motion->literal_stencil_clip->textured mask triangles->EQUAL gated sprite pixels",
    }
    limits = [
        "Mesh basis/operators and own-grid formula now derive from exact driver bytes; final float/SSE raster output has not been oracle-compared.",
        "Nonblank atlas sprites now apply their own BP, and out-of-domain patch inputs extrapolate instead of clamping.",
        "meshCombine point operator is implemented, but ownership/ancestor skipping and meshSyncChildMask are not fully integrated.",
        "Strict rejects additive BP hierarchies until ownership and join-target propagation are recovered.",
        "Stencil mode0 EQUAL/ref, alpha GREATER64, wrapping INCR/DECR are concrete CPU operations; mode1 fallback remains absent.",
        "Textured stencil triangles share the actual bilinear/top-left model rasterizer and update the8-bit plane per sampled fragment.",
        "Local type5 literal sprite sources are now rasterized and used to gate descendant sprite pixels. Nested-motion source masks and general inheritance keep an explicit strict warning.",
        "A completed source-over mask must not replace per-primitive alpha testing. RGBA_mask API requires one primitive alpha surface.",
        "PSB type5 full scheduling across joined/reference motions is not integrated, so default NEKO strict frames remain rejected.",
        "No new diagnostic preview/pixel hash is used as native success; no original DLL/game execution or compile/tests.",
    ]
    payload = {
        "schema": "cs2-recovery-A-emote-mesh-stencil/v1", "task_date": "2026-11-17",
        "generated_at_utc": datetime.now(timezone.utc).isoformat(),
        "summary": summary, "APIs": apis,
        "source_driver": {"path": str(path.relative_to(ROOT)), "sha256": DRIVER_SHA},
        "source_PSB": {"name": name, "sha256": sha(data), "values": psb.nodes},
        "model_inventory": observed,
        "snippets": {key: block(image, *span) for key, span in anchors.items()},
        "source_files": [{"path": p, "sha256": sha((ROOT/p).read_bytes())} for p in sources],
        "limits": limits,
    }
    prefix = ROOT / "reports/recovery_A_20261117_emote_mesh_stencil"
    prefix.with_suffix(".json").write_text(json.dumps(payload, ensure_ascii=False, indent=2)+"\n",
                                           encoding="utf-8")
    prefix.with_name(prefix.name+"_summary").with_suffix(".json").write_text(
        json.dumps({"summary": summary, "APIs": apis}, ensure_ascii=False, indent=2)+"\n",
        encoding="utf-8")
    lines = [
        "# A: CPU mesh/stencil batch", "",
        "Original bytes and actual PSB structure only; no compilation/tests or original execution.", "",
        "## Concrete code", "",
    ]
    lines += ["- "+v for v in apis.values()]
    lines += [
        "", "## Original evidence", "",
        "- Driver SHA256`"+DRIVER_SHA+"`.",
        "- 0x2c150/0x2c2a9:16-point bicubic Bernstein evaluation; no clamp on u/v.",
        "- 0x2bdb0:pointwise source-minus-unit-grid displacement addition.",
        "- 0x3909c..390c9:truncated meshDivision*quality, aspect-ratio split,+1 each axis. "
        "0x3078c initializes quality1.0;0x25c0 writes the actual quality setter.",
        "- 0x3a5fb..3a6c3:composite names resolved via layerIndexMap; only kind0/3 "
        "sources retained, missing labels/other kinds skipped.",
        "- 0x5780f..578cb:stencil enable,failKEEP,8-bit read mask,alpha GREATER64.",
        "- 0x57a96/0x57bde:reference EQUAL + wrappingINCR/DECR on pass/depth-fail.",
        "- 0x57ccd:reference EQUAL +KEEP for color drawing.",
        "- Local type5 source sprites now use the real sampled-alpha pass inside the CPU renderer; "
        "the general join/reference chain remains an explicit strict refusal.",
        "", "## Actual first-model bindings", "",
        f"{summary['actual_model_composite_mask_owners']} composite owners / "
        f"{summary['actual_model_resolved_mask_sources']} resolved source layers. "
        "These are ownership counts, not integrated mask/native completion.",
        "", "## Still blocked", "",
    ]
    lines += ["- "+v for v in limits]
    lines += ["", "## Paths/classification", ""]
    lines += ["- "+p for p in sources]
    lines += [
        "- reports/recovery_A_20261117_emote_mesh_stencil.json/md",
        "- reports/recovery_A_20261117_emote_mesh_stencil_summary.json",
        "", "No new directories/temp outputs,root README/localization or host wiring changes. "
        "Analysis scripts are classified under scripts/analysis.",
    ]
    prefix.with_suffix(".md").write_text("\n".join(lines)+"\n", encoding="utf-8")
    print(json.dumps(summary, ensure_ascii=True, indent=2))


if __name__ == "__main__":
    main()

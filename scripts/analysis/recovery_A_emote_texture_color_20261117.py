"""Small source/evidence handoff for texture converters and packed color.

Bounded byte reads only. No compiler, tests or original executable is invoked.
"""
from __future__ import annotations

from datetime import datetime, timezone
import json

from recovery_A_20261116 import ROOT, Image, sha
from recovery_A_receiver_contracts_20261116 import block

DRIVER_SHA = "85eb459e94e74ab608f649eb7fb440f2526e90aaa1af7276ed01dc10018903ce"


def main():
    path = next(ROOT.glob("*NEKO*/nekopara_vol3/emotedriver.dll"))
    image = Image(path, DRIVER_SHA)
    anchors = {
        "packed_color_lerp": (0x2B9A0, 0xA7),
        "color_lerp_caller": (0x3DA73, 0x2F),
        "packed_color_defaults": (0x37245, 0x4B),
        "texture_type_dispatch": (0x54F83, 0xA1),
        "texture_RGBA5650": (0x545D0, 0x6D),
        "texture_RGBA5551": (0x546D0, 0x85),
        "texture_RGBA4444": (0x547E0, 0x7E),
        "texture_RGBA8_swap": (0x54860, 0x4A),
    }
    files = [
        "simulator/crates/runtime/src/emote.rs",
        "simulator/crates/runtime/src/emote_cpu.rs",
        "simulator/crates/runtime/src/emote_timeline.rs",
        "simulator/crates/runtime/src/emote_texture.rs",
        "scripts/analysis/recovery_A_emote_texture_color_20261117.py",
    ]
    summary = {
        "RGBA8_raw_and_RL_pipeline_preserved": True,
        "new_common_raw_formats": ["RGBA5650", "RGBA5551", "RGBA4444"],
        "unknown_win_16bit_or_compressed_16bit": "explicit error",
        "packed_color_linear_interpolation_source": "original2b9a0 fixed-point256 ratio",
        "packed_color_curve_missing_endpoints": "explicit renderer warning",
        "layer_color_render_state_and_blend_modes": "still strict-blocked",
        "compiled": False, "tests_run": False, "original_code_executed": False,
        "NEKO_complete_strict_frame": False, "integrated_completed_native_count": 0,
    }
    apis = {
        "texture": "decode_emote_texture(kind,platform,width,height,bytes)->Result<Vec<u8>>",
        "model_integration": "CpuMotionModel::load now uses the shared texture decoder",
        "color": "interpolate_emote_color(start:u32,end:u32,ratio:f32)->Result<u32>",
        "timeline_integration": "sample_emote_layer handles explicit two-color endpoints through exact packed arithmetic",
    }
    limits = [
        "First NEKO model texture remains win/RGBA8. New common16-bit formats come from original converter bytes, not additional NEKO compatibility evidence.",
        "16-bit input is accepted only at exact raw size; no guessed compressed stream is decoded.",
        "Packed interpolation is independent of RGBA/ARGB interpretation; original DWORD operations are preserved.",
        "Static layer color/bm,nonlinear occ curves and missing endpoint defaults still prevent strict native drawing when needed.",
        "No current-source Rust pixel output,tests or original DLL execution is claimed.",
    ]
    out = {
        "schema": "cs2-recovery-A-emote-texture-color/v1", "task_date": "2026-11-17",
        "generated_at_utc": datetime.now(timezone.utc).isoformat(),
        "summary": summary, "APIs": apis, "limits": limits,
        "driver_sha256": DRIVER_SHA,
        "snippets": {key: block(image, *span) for key, span in anchors.items()},
        "source_files": [{"path": p, "sha256": sha((ROOT/p).read_bytes())} for p in files],
    }
    prefix = ROOT / "reports/recovery_A_20261117_emote_texture_color"
    prefix.with_suffix(".json").write_text(json.dumps(out, ensure_ascii=False, indent=2)+"\n",
                                           encoding="utf-8")
    prefix.with_name(prefix.name+"_summary").with_suffix(".json").write_text(
        json.dumps({"summary": summary, "APIs": apis}, ensure_ascii=False, indent=2)+"\n",
        encoding="utf-8")
    lines = [
        "# A: texture/packed-color batch", "",
        "No compilation/tests or original executable loading.", "",
        "## Delivered", "",
    ]
    lines += ["- "+v for v in apis.values()]
    lines += [
        "- Common raw5650/5551/4444 inputs convert to real straight RGBA pixels. "
        "RGBA8 raw/RL and win BGRA conversion remain available.",
        "", "## Original-byte anchors", "",
        "- DriverSHA256`"+DRIVER_SHA+"`.",
        "- Type dispatch0x54f83..55024 selects converters545d0/546d0/547e0.",
        "- 5650:bits11..15 R,5..10 G,0..4 B,opaque alpha.",
        "- 5551:bits11..15 R,6..10 G,1..5 B,bit0 alpha.",
        "- 4444:bits12..15 R,8..11 G,4..7 B,0..3 A.",
        "- Packed interpolation0x2b9a0 uses trunc256*ratio,wrapping DWORD products,"
        "channel masks and shifts. The actual timeline caller is0x3da73.",
        "", "## Limits", "",
    ]
    lines += ["- "+v for v in limits]
    lines += ["", "## Paths/classification", ""]
    lines += ["- "+p for p in files]
    lines += [
        "- reports/recovery_A_20261117_emote_texture_color.json/md",
        "- reports/recovery_A_20261117_emote_texture_color_summary.json",
        "", "No transient output or new directories; scripts remain scripts/analysis. "
        "Root README,host wiring,localization and game files were not changed.",
    ]
    prefix.with_suffix(".md").write_text("\n".join(lines)+"\n", encoding="utf-8")
    # One small current handoff; no full CFG or game-resource payload.
    rollup = {
        "schema": "cs2-recovery-A-emote-current-summary/v1",
        "task_date": "2026-11-17",
        "summary": {
            "status": "source-only; host integration owned by parent",
            "descriptor_slots": 36,
            "single_wrappers": 24, "batch_wrappers": 4, "blocked_wrappers": 8,
            "backend_native_branches": {
                "906": "simple mode0 actual resource load",
                "919": "exact raw color percentages into real CPU RGBA",
            },
            "new_CPU_batches": [
                "f32 frame sampler,type0/type2/type3,ti quantization,shortest-angle,ordered affine",
                "16-point bicubic patch,offset combination operator,aspect-ratio mesh grids",
                "stencil source bindings,real triangle mask writes,EQUAL-gated local type5 sprite drawing",
                "common raw16-bit texture formats and packed DWORD color interpolation",
            ],
            "remaining_strict_blockers": [
                "general stencil joined/reference-motion/inheritance scheduler",
                "additive BP hierarchy and meshSyncChildMask",
                "3D projection,nonlocal coordinates,unsupported layer1/other kinds",
                "layer color/blend modes,nonlinear curves,controller physics",
            ],
            "first_model_structural_counts": {
                "layers": 525, "type3_keyframes": 1023,
                "composite_mask_owners": 6, "resolved_mask_sources": 8,
            },
            "compiled": False, "tests_run": False, "Rust_pixel_execution": False,
            "full_NEKO_strict_frame_claim": False,
            "integrated_completed_native_count": 0,
        },
        "main_entry": (
            "EmoteNativeServices::receive(index,args,vm,resolved_target,pairs); "
            "existing draw delegates to strict backend and writes Canvas only after success"
        ),
        "new_APIs": [
            "sample_emote_layer", "interpolate_emote_angle", "interpolate_emote_color",
            "emote_layer_affine", "CpuBezierPatch", "CpuMeshGrid",
            "CpuEmoteBackend::set_mesh_quality_scale", "CpuMotionModel::stencil_bindings",
            "CpuStencilBuffer", "rasterize_emote_stencil_triangle", "decode_emote_texture",
        ],
        "reports": [
            "reports/recovery_A_20261117_emote_dispatch.md",
            "reports/recovery_A_20261117_emote_cpu_features.md",
            "reports/recovery_A_20261117_emote_mesh_stencil.md",
            "reports/recovery_A_20261117_emote_texture_color.md",
        ],
        "new_source_paths": [
            "simulator/crates/runtime/src/emote_timeline.rs",
            "simulator/crates/runtime/src/emote_mesh.rs",
            "simulator/crates/runtime/src/emote_stencil.rs",
            "simulator/crates/runtime/src/emote_texture.rs",
        ],
        "modified_source_paths": [
            "simulator/crates/runtime/src/emote.rs",
            "simulator/crates/runtime/src/emote_cpu.rs",
        ],
        "scope": "no lib exports,host dispatch/constructors,root README or game files changed",
        "classification": "scripts/analysis + reports; no transient artifacts in this batch",
    }
    (ROOT/"reports/recovery_A_20261117_emote_current_summary.json").write_text(
        json.dumps(rollup, ensure_ascii=False, indent=2)+"\n", encoding="utf-8")
    print(json.dumps(summary, ensure_ascii=True, indent=2))


if __name__ == "__main__":
    main()

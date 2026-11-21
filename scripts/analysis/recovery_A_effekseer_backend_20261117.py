"""Record the bounded type61 source delivery and immutable PE evidence.

Reads source and PE bytes only. It never invokes a Rust compiler, tests, VM,
game or reference engine. Runtime behavior claims remain source-level subsets.
"""
import hashlib
import json

from recovery_A_20261116 import ROOT, Image
from recovery_A_receiver_contracts_20261116 import block

PREFIX = "recovery_A_20261117_effekseer_backend"
SOURCE = [
    "simulator/crates/runtime/src/effekseer.rs",
    "simulator/crates/runtime/src/effekseer_format.rs",
    "simulator/crates/desktop/src/effekseer_native.rs",
]
HOOKS = [
    "simulator/crates/runtime/src/lib.rs",
    "simulator/crates/desktop/src/lib.rs",
    "simulator/crates/desktop/src/data_services.rs",
    "simulator/crates/desktop/src/native_tasks.rs",
]
REMAINING = [
    {"family": "SKFE10plus", "branch": "target has >=11 default seed and >=13 rendering-node fields; parser explicitly rejects versions outside0..9"},
    {"family": "random", "branch": "nondegenerate lifetime, interval, offset, transform and spawn ranges; no invented RNG seed"},
    {"family": "instance_tree", "branch": "only root-to-leaf sprite trees; none/nested parent matrices and remove-on-children remain rejected"},
    {"family": "emit_shapes", "branch": "sphere/circle/model emission and generation rotation binding remain rejected"},
    {"family": "transforms", "branch": "axis/FCurve, 3D Euler, Y-axis-fixed billboard remain rejected"},
    {"family": "color", "branch": "random/easing/FCurve all-color remain rejected; fixed and corner RGBA are real"},
    {"family": "other_geometry", "branch": "ribbon/ring/model/track remain rejected"},
    {"family": "sound", "branch": "wave banks and embedded sound-node scheduling remain rejected"},
    {"family": "distortion", "branch": "normal/distortion banks and background sampling remain rejected"},
    {"family": "graphics_interop", "branch": "independent manager depth plane, inherited DX9 alpha-test/separate-alpha/mipmap state and device-loss parity not recovered; hidden task/depth-range scheduler parity is not asserted"},
    {"family": "task_rotation", "branch": "task message55 live instance rotation explicitly rejects; owned messages51/59/66 now update real XYZ/color/shared projection; other base/delegated command branches remain partial"},
]


def main():
    pe = Image(ROOT / "Yukikoi Melt/YukikoiMelt.exe",
               "63de0e14cd8ce11ad2d68e7ed6076641ff5e5b73d822603cc52764009d812976")
    inputs = []
    for relative in SOURCE + HOOKS:
        data = (ROOT / relative).read_bytes()
        inputs.append({"path": relative, "bytes": len(data),
                       "sha256": hashlib.sha256(data).hexdigest(),
                       "ownership": "A module" if relative in SOURCE else "minimal additive shared hook"})
    evidence = [block(pe, rva, size) for rva, size in [
        (0x1ac7e0, 0x80), (0x1ac860, 0x40), (0x1ac8a0, 0x40),
        (0x18b9f0, 0x130), (0x18bb20, 0x50), (0x18bd20, 0xf0),
        (0x18be10, 0xc0), (0x18bed0, 0x100),
        (0x18bfd0, 0xd0), (0x18c0a0, 0x130),
        (0x18c444, 0x1b9), (0x18c670, 0x60), (0x18c6d0, 0x4b),
        (0x18c720, 0x15b), (0xd3000, 0x2fb), (0xd3340, 0xac),
        (0x18c25e, 0x97), (0x18c3ef, 0x55),
        (0xd43d0, 0xd1), (0x2005c0, 0x54),
    ]]
    summary = {
        "ordinary_slots_with_concrete_backend_source": [1014, 1015, 1016],
        "owned_task_commands": [51, 59, 66, 578, 579, 580, 581, 582],
        "shared_projection_last_constructor_or_scale_write": True,
        "draw_does_not_replace_shared_projection_per_task": True,
        "task_type": 61, "backend_optional": False,
        "real_resource_categories": ["efk", "efktex"],
        "accepted_SKFE_versions": list(range(10)),
        "accepted_topology": "root and direct sprite leaves with deterministic ranges",
        "real_pipeline": "ordinary descriptor -> owned type61 receiver -> efk read/SKFE parse -> efktex decode -> shared manager Play -> draw Update(1frame) -> triangles/texture sampling/depth/blend -> existing Canvas RGBA",
        "task_update_does_not_advance_particle_clock": True,
        "repeat_parameter_is_raw_flag": True,
        "unload_does_not_stop_independent_shared_playbacks": True,
        "repeat_play_does_not_stop_previous_handle": True,
        "source_remaining_family_count": len(REMAINING),
        "completed_native_equivalence_count_added": 0,
        "full_type61_equivalence": False,
        "actual_game_effect_resource_loaded_or_rendered": False,
        "compiled": False, "tests_run": False, "Rust_or_VM_execution": False,
        "original_binary_or_DLL_execution": False,
        "syntax_read_only": "rustfmt parser for three A module files; no rustc/cargo",
        "reference_commit": "4a3f2649f4937538279450ed244319bdcef0617d",
        "reference_license": "Effekseer MIT; DirectX Tool Kit Ms-PL part not used; independently written Rust, no source copying",
        "coordinate_correction": "RVA d3000 is target-eye(+Z), up cross forward(+X), not RH; d3340 has +1/(far-near). Camera eye(0,0,-100), ortho viewport/(scale*16), near1 far300.",
        "helper_address_correction": "texture basename helper VA6005c0 is RVA2005c0 for base400000; raw byte slash scanning retained",
    }
    per_pe = [{"path": "Yukikoi Melt/YukikoiMelt.exe", "sha256": pe.digest,
               "factory_type": 61, "constructor_rva": 0x18b9f0,
               "receiver_rva": 0x18c1f0, "task_update_rva": 0x18bed0,
               "draw_update_rva": 0x18be10, "draw_callback_rva": 0x18c670,
               "native_wrappers": [{"slot": s, "rva": r, "args": a, "command": c}
                                   for s, r, a, c in [(1014,0x1ac7e0,8,578),(1015,0x1ac860,4,579),(1016,0x1ac8a0,8,580)]]}]
    payload = {"schema": "cs2-recovery-A-effekseer-backend/v1", "summary": summary,
               "perPE": per_pe, "source_files": inputs,
               "remaining_implementation_families": REMAINING,
               "PE_evidence": evidence}
    out = ROOT / "reports"
    (out / (PREFIX + ".json")).write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
    (out / (PREFIX + "_summary.json")).write_text(json.dumps({"summary": summary, "perPE": per_pe}, indent=2) + "\n", encoding="utf-8")
    lines = ["# A: type61 Effekseer concrete CPU batch", "",
             "1014/1015/1016 have concrete resource, lifetime, frame update and CPU pixel source plus additive host wiring. This is a bounded implementation, not proof of all native function equivalence. No compilation, tests or target execution.", "",
             "## Actual receiver and pixels", "", summary["real_pipeline"], "",
             "- 1014/578: destructive old-effect release before IO; empty path releases only; failure never restores the old resource.",
             "- 1015/579: clears only task effect; manager playbacks own Arc resources and may remain live under another task draw.",
             "- 1016/580: raw repeat flag; new Play overwrites only task current handle; old shared instances stay alive.",
             "- update_task checks Exists and restarts using current XYZ/color. draw_task advances the shared manager by1 frame and draws every live playback.",
             "- Owned51/59/66 consume real XYZ/color/scale. Shared renderer projection follows the latest constructor/changed-scale write, not whichever task draws next. Message55 rotation stays an explicit error.",
             "- Actual accepted geometry: deterministic root/direct-sprite emission, lifetime and max count, fixed/PVA/easing transforms, gravity, fixed/corner RGBA, fades, UV fixed/scroll/animation, texture nearest/bilinear and repeat/clamp, triangles and independent manager depth surface.",
             "- Persistent live effects are counted as unsupported graph snapshot state, so save cannot silently drop them.", "",
             "## Exact PE navigation", "",
             "SHA256 `" + pe.digest + "`, base400000; type61 ctor18b9f0, receiver18c1f0, task update18bed0, draw-body18be10, callback18c670. Wrapper RVAs1ac7e0/1ac860/1ac8a0.", "",
             summary["coordinate_correction"], "", summary["helper_address_correction"], "",
             "## Remaining", ""]
    lines += ["- **" + x["family"] + "**: " + x["branch"] for x in REMAINING]
    lines += ["", "## Modified source and hooks", ""]
    lines += ["- `" + x["path"] + "` (" + x["ownership"] + ")" for x in inputs]
    lines += ["", "The fixed upstream texts are research inputs under archive/2026-11/20261117_effekseer101_*. License and hashes are recorded in recovery_A_20261117_effekseer_reference.json. No EXE or game asset is included in this delivery. No new directory, localization or root README change."]
    (out / (PREFIX + ".md")).write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(json.dumps(summary))


if __name__ == "__main__":
    main()

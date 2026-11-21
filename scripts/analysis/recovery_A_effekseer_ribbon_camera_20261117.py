"""Static camera-facing ribbon evidence; no compiled/target execution."""
import hashlib
import json
from recovery_A_20261116 import ROOT, Image
from recovery_A_receiver_contracts_20261116 import block

PREFIX = "recovery_A_20261117_effekseer_ribbon_camera"


def main():
    prior = json.loads((ROOT / "reports/recovery_A_20261117_effekseer_ribbon.json").read_text(encoding="utf-8"))
    image = Image(ROOT / prior["perPE"][0]["path"], prior["perPE"][0]["sha256"])
    remaining = prior["remaining_implementation_families"]
    for row in remaining:
        if row["family"] == "other_geometry":
            row["branch"] = "sprite/ring and fixed/camera-facing group ribbons have real pixel source for deterministic SKFE0..9; zero/singular camera axes reject; SKFE13 spline subdivisions, model and track remain unsupported"
    summary = {
        "batch": "target camera-facing ribbon frame",
        "real_pipeline": "Ribbon viewpoint flag -> world-row decomposition -> actual type61 camera-front column -> native two-step inverse-square-root cross frame -> asymmetric left-X/right-Y scale -> true group quads -> installed CPU texture raster",
        "new_source_branches": ["nonzero viewpoint flag", "sqrt(square)/square row multiplier", "actual fixed-camera column2", "native fast normalize", "left width scaleX/right width scaleY", "explicit zero/nonfinite frame diagnostics"],
        "PE_public_reference_difference": "100a04..100a25 scales left width by X and right width by Y; public101 text scales both by X",
        "source_remaining_family_count": len(remaining), "whole_family_closures": 0,
        "completed_native_equivalence_count_added": 0,
        "compiled": False, "tests_run": False, "Rust_VM_game_DLL_APK_execution": False,
        "syntax_only": "rustfmt parser, not typecheck",
    }
    pe = [dict(prior["perPE"][0], matrix_GetSRT_rva=0xe2f60,
               renderer_camera_set_rva=0xf43d0, renderer_front_get_rva=0xf44e0,
               ribbon_camera_transform_rva=0x1009b6)]
    source = []
    for row in prior["source_files"]:
        data = (ROOT / row["path"]).read_bytes()
        source.append({"path": row["path"], "bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()})
    evidence = [block(image, rva, size) for rva, size in [
        (0x1009b6, 0x3df), (0xe2f60, 0xc7), (0xf43d0, 0x40),
        (0xf44e0, 0x1d), (0x18c7e7, 0x8e),
    ]]
    payload = {"summary": summary, "perPE": pe, "source_files": source,
               "remaining_implementation_families": remaining, "PE_evidence": evidence}
    out = ROOT / "reports"
    (out / (PREFIX + ".json")).write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
    (out / (PREFIX + "_summary.json")).write_text(json.dumps({"summary": summary, "perPE": pe}, indent=2) + "\n", encoding="utf-8")
    lines = ["# A: Effekseer camera-facing ribbons", "", summary["real_pipeline"], "",
             "effekseer_format now retains ViewpointDependent!=0; effekseer_ribbon::camera_edges/normal/cross calculates actual camera-facing edge positions. This feeds the existing group geometry and concrete CPU raster without a new host hook.", "",
             "PE1009b6..100d95 calls GetSRT e2f60, scales widths, takes the camera front from virtual+58/f44e0 and computes a two-step magic0x5f3759df normalization for front, right and reconstructed front. GetSRT uses sqrt(squareSum)/squareSum for its row multiplier. f43d9..f4403 proves renderer front is camera column2; type61's18c7e7..18c874 installs fixed eye(0,0,-100), target0, upY.", "",
             summary["PE_public_reference_difference"], "",
             "Zero/singular/nonfinite row or cross-product cases remain explicit errors. No substitute direction is inserted. SKFE10+ including spline subdivisions remains rejected. Mixed active/removing group count parity and legacy sprite/ring fade order are still unaudited rather than implicitly complete.", "",
             "Executable SHA256: " + image.digest, "",
             "No compile/typecheck, tests or Rust/VM/game/DLL/APK execution. rustfmt syntax reading only. Complete native-equivalence added0;11 families remain partial.", "", "## Remaining families", ""]
    lines += ["- " + row["family"] + ": " + row["branch"] for row in remaining]
    lines += ["", "Source files: effekseer_ribbon.rs and effekseer_format.rs. Script classified under scripts/analysis; reports contain byte/hash/disassembly evidence. Prior dated archive disassembly retained. No new directory, root README, localization, game asset or Emote edits."]
    (out / (PREFIX + ".md")).write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(json.dumps(summary))


if __name__ == "__main__":
    main()

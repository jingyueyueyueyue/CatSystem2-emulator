"""Static PE-bound absolute-force receiver delivery, never executes target code."""
import hashlib
import json
import struct
from recovery_A_20261116 import ROOT, Image
from recovery_A_receiver_contracts_20261116 import block

PREFIX = "recovery_A_20261117_effekseer_force"


def main():
    prior = json.loads((ROOT / "reports/recovery_A_20261117_effekseer_hsva.json").read_text(encoding="utf-8"))
    image = Image(ROOT / prior["perPE"][0]["path"], prior["perPE"][0]["sha256"])
    remaining = prior["remaining_implementation_families"]
    for row in remaining:
        if row["family"] == "transforms":
            row["branch"] = "Euler/axis/FCurve/bind/billboards and PE absolute none/gravity/attraction now have source; singular/nonfinite/overflow/address-invalid paths explicitly reject; full target instance-helper parity remains unaudited"
    summary = {
        "batch": "absolute world-space attraction into actual recursive pixels",
        "real_pipeline": "SKFE absolute tag2/16bytes -> per-instance force state -> target0 world-space steering/range attenuation -> corrected parent/child matrices -> actual sprite/ring vertices -> existing CPU textured triangles/Canvas",
        "new_source_branches": ["absolute attraction force/control/min/max", "parent-position initialization independent of binding", "pre-correction currentPosition/motion bookkeeping", "range-dependent force", "persistent revision velocity/displacement", "PE epsilon after norm", "all-four-parameter target magnification", "default global target0"],
        "PE_public_reference_differences": ["e2651 adds f32 bits38d1b717 to speed before normalization/rescaling", "dcadf..dcb36 magnifies force, control, minRange, maxRange", "d9fdd..d9feb initializes target independently of Play position"],
        "source_remaining_family_count": len(remaining), "whole_family_closures": 0,
        "completed_native_equivalence_count_added": 0,
        "compiled": False, "tests_run": False, "Rust_VM_game_DLL_APK_execution": False,
        "syntax_only": "rustfmt parser, not typecheck",
    }
    pe = [dict(prior["perPE"][0], absolute_reader_rva=0xdca09,
               absolute_update_rva=0xe2430, instance_init_position_rva=0xdef0a,
               global_play_init_rva=0xd9fbf)]
    source = []
    for path in [r["path"] for r in prior["source_files"]] + ["simulator/crates/runtime/src/effekseer_force.rs"]:
        data = (ROOT / path).read_bytes()
        source.append({"path": path, "bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()})
    evidence = [block(image, rva, size) for rva, size in [
        (0xdca09, 0x133), (0xe2430, 0x322), (0xe2136, 0x68),
        (0xdef0a, 0x41), (0xd9fbf, 0x59),
    ]]
    epsilon = image.pe.get_data(0x5f9fa4, 4)
    payload = {"summary": summary, "perPE": pe, "source_files": source,
               "remaining_implementation_families": remaining,
               "epsilon": {"rva": 0x5f9fa4, "bytes": epsilon.hex(), "f32": struct.unpack("<f", epsilon)[0]},
               "PE_evidence": evidence}
    out = ROOT / "reports"
    (out / (PREFIX + ".json")).write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
    (out / (PREFIX + "_summary.json")).write_text(json.dumps({"summary": summary, "perPE": pe}, indent=2) + "\n", encoding="utf-8")
    lines = ["# A: Effekseer absolute force", "", summary["real_pipeline"], "",
             "New effekseer_force.rs implements Absolute::{None,Gravity,Attraction} and per-instance ForceState::sample. Reader::node accepts the real16-byte tag2 payload; Tree initializes independent persistent state from the full parent position and applies the correction after local/generation/parent matrices, before actual geometry. No optional backend or new host hook is required.", "",
             "Exact PE e2430..e2751: target difference uses currentPosition minus current global motion plus accumulated correction; force attenuates between min/max ranges. Velocity gains direction*force*delta, then steering blends normalized current velocity with target direction. The target adds epsilon0x38d1b717 to the speed before both normalization and rescaling, including zero-speed cases. Nonfinite results reject rather than inserting a safe-direction substitute.", "",
             "Target loader dca09..dcb36 stores force/control/minRange/maxRange and, when its magnification branch applies, multiplies all four, including control. This differs from the older public behavior text. def0a..def48 initializes previous position from the full parent's matrix regardless of T binding and zeros correction/velocity. d9fdd..d9feb initializes global target XYZ to0; type61 does not call a target setter, so no guessed task-position target is used.", "",
             "Exact executable SHA256: " + image.digest, "",
             "No compiler, tests or Rust/VM/game/DLL/APK execution. rustfmt syntax reading only. Native full-equivalence added0. Eleven families remain partial; the transform family still contains singular/address-invalid branches and unaudited helper parity.", "", "## Remaining families", ""]
    lines += ["- " + row["family"] + ": " + row["branch"] for row in remaining]
    lines += ["", "Source files recorded with hashes in the JSON. Script classified under scripts/analysis, evidence in reports; no temporary outputs, new directories, root README, localization or game asset edits. Independent Rust from PE and pinned MIT behavior research; no copied C++ or engine execution."]
    (out / (PREFIX + ".md")).write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(json.dumps(summary))


if __name__ == "__main__":
    main()

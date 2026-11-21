"""Static emitter/axis delivery; no target or runtime execution."""
import hashlib
import json
from recovery_A_20261116 import ROOT, Image
from recovery_A_receiver_contracts_20261116 import block
from recovery_A_effekseer_tree_20261117 import FILES

PREFIX = "recovery_A_20261117_effekseer_emitters"


def main():
    previous = json.loads((ROOT / "reports/recovery_A_20261117_effekseer_tree.json").read_text(encoding="utf-8"))
    remaining = previous["remaining_implementation_families"]
    for row in remaining:
        if row["family"] == "emit_shapes":
            row["branch"] = "point, deterministic sphere, ordered/reverse circle and EffectsRotation implemented; random circle, model and line emission pending"
        if row["family"] == "transforms":
            row["branch"] = "Euler/AxisPVA/AxisEasing, bind0..3 and all billboard modes implemented; FCurve and attractive absolute force pending; singular decomposition explicit error"
    image = Image(ROOT / "Yukikoi Melt/YukikoiMelt.exe", previous["perPE"][0]["sha256"])
    summary = {"batch": "deterministic emitter/axis and exact LH conversion",
               "implemented_subbranches": ["sphere", "circle ordered", "circle reversed", "generation EffectsRotation", "axis PVA", "axis easing", "exact type61 LH load conversion"],
               "source_remaining_family_count": len(remaining), "whole_family_closures": 0,
               "completed_native_equivalence_count_added": 0,
               "compiled": False, "tests_run": False, "Rust_VM_game_DLL_APK_execution": False,
               "critical_correction": "type61 ctor18bc93..18bc9d passes0(LH) to manager vtable+38; target d7eb0 writes setting+8. Parser now negates translation Z, Euler XY, axis Z, sphere XY, matching dd24d..dd59a; point and gravity intentionally unchanged.",
               "pipeline": "node spawn by each parent generation number -> per-instance generation matrix -> EffectsRotation/local/parent/world gravity -> real Quad/CPU raster",
               "circle_layout": "target dce7b selects32-byte legacy circle below10; versions>=10 add axis/noise fields and remain rejected",
               "axis_not_normalized": True,
               "remaining_implementation_families": remaining}
    source = []
    for path in FILES:
        data = (ROOT / path).read_bytes()
        source.append({"path": path, "bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()})
    evidence = [block(image, r, n) for r,n in [
        (0xdcc12,0x6c), (0xdcde9,0x108), (0xdd24d,0x34d),
        (0x18bc93,0xd), (0xd7eb0,0x13),
    ]]
    pe = [dict(previous["perPE"][0], coordinate_setter_rva=0xd7eb0, generation_reader_rva=0xdcde9)]
    payload = {"summary": summary, "perPE": pe, "source_files": source,
               "remaining_implementation_families": remaining, "PE_evidence": evidence}
    out = ROOT / "reports"
    (out/(PREFIX+".json")).write_text(json.dumps(payload,indent=2)+"\n",encoding="utf-8")
    (out/(PREFIX+"_summary.json")).write_text(json.dumps({"summary":summary,"perPE":pe},indent=2)+"\n",encoding="utf-8")
    lines = ["# A: Effekseer emitters / axis", "", summary["pipeline"], "", summary["critical_correction"], "", summary["circle_layout"], "",
             "Deterministic sphere uses sampled radius and X/Y rotation. Circle index comes from each parent's generated count modulo division, with reversal before mapping to angle. EffectsRotation applies the whole generation matrix; disabled uses its translation only. AxisPVA/Easing uses the stored axis directly, without inventing normalization. All paths produce actual existing textured triangles.", "",
             "Native equivalence added0. No compile, tests, Rust/VM/game/DLL/APK execution. Only static PE/source reads and rustfmt parsing.", "", "## Remaining families", ""]
    lines += ["- "+r["family"]+": "+r["branch"] for r in remaining]
    lines += ["", "Source files: "]+["- "+r["path"] for r in source]
    lines += ["", "Script classified under scripts/analysis; pinned texts archived under archive/2026-11/20261117_effekseer101_*. No root README, localization, new directory or original assets changed."]
    (out/(PREFIX+".md")).write_text("\n".join(lines)+"\n",encoding="utf-8")
    print(json.dumps({k:v for k,v in summary.items() if k != "remaining_implementation_families"}))


if __name__ == "__main__": main()

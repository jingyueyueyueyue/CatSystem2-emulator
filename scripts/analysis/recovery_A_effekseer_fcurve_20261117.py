"""Static PE-bound FCurve batch, no execution or synthetic test inputs."""
import hashlib
import json
from recovery_A_20261116 import ROOT, Image
from recovery_A_receiver_contracts_20261116 import block

PREFIX = "recovery_A_20261117_effekseer_fcurve"


def main():
    prior = json.loads((ROOT / "reports/recovery_A_20261117_effekseer_ring.json").read_text(encoding="utf-8"))
    remaining = prior["remaining_implementation_families"]
    for row in remaining:
        if row["family"] == "transforms":
            row["branch"] = "Euler/axis/FCurve/bind/billboards implemented for deterministic resources; attractive absolute-force controller and singular/overflow/address-invalid branches remain explicit errors"
        if row["family"] == "color":
            row["branch"] = "sprite fixed/easing/FCurve RGBA and ring fixed/easing RGBA implemented; HSVA and nondegenerate RNG remain rejected"
    image = Image(ROOT / "Yukikoi Melt/YukikoiMelt.exe", prior["perPE"][0]["sha256"])
    summary = {
        "batch": "sampled transforms and sprite RGBA",
        "new_source_branches": ["translation3 sampled XYZ", "rotation5 sampled Euler", "scaling5 sampled XYZ", "sprite color3 sampled RGBA", "constant/loop/reverse-loop edges", "signed32 frame subtraction", "native frequency interpolation", "empty defaults0/1/255", "deterministic per-axis offsets", "exact key/offset magnification and LH conversion"],
        "real_pipeline": "checked FCurve load -> Instance preincrement render age -> native edge/frequency sample -> actual world matrix/color -> existing sprite/ring CPU triangles",
        "PE_facts": {"reader": "eddc0", "sampler": "ede60..edf8f", "coordinate_change": "edfa0", "magnification": "ee000", "XYZ_reader": "ee120"},
        "important_asymmetry": "LH converts Euler/axis and rotation-FCurve XY; translation-FCurve Z is not negated by the target's conversion switch. Maginify changes keys and random offset, not the empty default.",
        "invalid_address_policy": "PE reads key[index+1] before testing last interval. Insufficient storage rejects instead of inventing interpolation or loading memory past resource keys.",
        "source_remaining_family_count": len(remaining), "whole_family_closures": 0,
        "completed_native_equivalence_count_added": 0,
        "compiled": False, "tests_run": False, "Rust_VM_game_DLL_APK_execution": False,
    }
    files = [r["path"] for r in prior["source_files"]]+["simulator/crates/runtime/src/effekseer_curve.rs"]
    source = []
    for path in files:
        data = (ROOT / path).read_bytes()
        source.append({"path":path,"bytes":len(data),"sha256":hashlib.sha256(data).hexdigest()})
    pe = [dict(prior["perPE"][0], fcurve_reader_rva=0xeddc0, fcurve_sample_rva=0xede60)]
    evidence = [block(image,r,n)for r,n in [(0xeddc0,0x99),(0xede60,0x130),(0xedfa0,0x5d),(0xee000,0x4e),(0xee120,0x45)]]
    payload = {"summary":summary,"perPE":pe,"source_files":source,
               "remaining_implementation_families":remaining,"PE_evidence":evidence}
    out = ROOT / "reports"
    (out/(PREFIX+".json")).write_text(json.dumps(payload,indent=2)+"\n",encoding="utf-8")
    (out/(PREFIX+"_summary.json")).write_text(json.dumps({"summary":summary,"perPE":pe},indent=2)+"\n",encoding="utf-8")
    lines = ["# A: Effekseer sampled curves", "",summary["real_pipeline"],"",
             "FCurve source now drives real XYZ translation, Euler rotation, scale and sprite RGBA. Loop/reverse-loop/constant edges follow the signed frame rules; sampled random offsets must be degenerate and are added to the actual keys. Empty channels preserve their constructor defaults. Nonfinite time/value, native INT_MIN edge, invalid frequency/loop and insufficient next-key storage are explicit errors.","",
             summary["important_asymmetry"],"",summary["invalid_address_policy"],"",
             "Exact PE SHA256 "+image.digest+"; reader eddc0, scalar sampler ede60..edf8f, coordinate switch edfa0, multiplier ee000, XYZ reader ee120. Fixed public behavior reference remains commit4a3f2649f4937538279450ed244319bdcef0617d, MIT research only; no copied source or engine linkage.","",
             "No compile, tests, Rust/VM/game/DLL/APK execution. Syntax read with rustfmt only. Native full equivalence added0.","","## Remaining families",""]
    lines += ["- "+r["family"]+": "+r["branch"] for r in remaining]
    lines += ["","## Source files",""]+["- "+r["path"] for r in source]
    lines += ["","Script classified under scripts/analysis. Newly read pinned FCurve texts use archive/2026-11/20261117_effekseer101_*. No new directory, root README/localization or game asset edits."]
    (out/(PREFIX+".md")).write_text("\n".join(lines)+"\n",encoding="utf-8")
    print(json.dumps(summary))


if __name__ == "__main__": main()

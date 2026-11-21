"""Static concrete ring/color delivery, with immutable PE/format provenance."""
import hashlib
import json
from recovery_A_20261116 import ROOT, Image
from recovery_A_receiver_contracts_20261116 import block
from recovery_A_effekseer_tree_20261117 import FILES

PREFIX = "recovery_A_20261117_effekseer_ring"


def main():
    prior = json.loads((ROOT / "reports/recovery_A_20261117_effekseer_emitters.json").read_text(encoding="utf-8"))
    remaining = prior["remaining_implementation_families"]
    for row in remaining:
        if row["family"] == "other_geometry":
            row["branch"] = "ring real radial mesh implemented for deterministic parameters; ribbon/model/track remain explicit errors"
        if row["family"] == "color":
            row["branch"] = "fixed/degenerate-random/easing RGBA implemented for sprite and ring; HSVA, nondegenerate RNG, FCurve pending"
    image = Image(ROOT / "Yukikoi Melt/YukikoiMelt.exe", prior["perPE"][0]["sha256"])
    summary = {
        "batch": "actual ring CPU mesh and timed RGBA", "node_kinds": [0,2,4],
        "implemented_subbranches": ["ring angle fixed/degenerate-random/ease", "inner/outer location fixed/PVA/ease", "center ratio fixed/degenerate-random/ease", "three radial RGBA curves", "two real radial bands per segment", "original triangle diagonal", "radial UV halves", "LH radial height", "full3D billboard/fixed vertices", "sprite RGBA easing", "lifetime generation count independent of simultaneous pool"],
        "actual_pipeline": "SKFE kind4 -> real ring parameters -> preincrement renderer time -> inner/center/outer radius+height -> two bands/segment -> world matrix+projection -> existing textured raster/Canvas pixels",
        "source_remaining_family_count": len(remaining), "whole_family_closures": 0,
        "completed_native_equivalence_count_added": 0,
        "compiled": False, "tests_run": False, "Rust_VM_game_DLL_APK_execution": False,
        "limits": ["SKFE0..9 only", "1..1024 ring segments", "262144 submitted quads per manager draw", "no invented RNG/HSVA/FCurve", "real-effect pixel execution awaits final unified validation"],
        "reference_commit": "4a3f2649f4937538279450ed244319bdcef0617d",
        "source_independent_no_reference_engine_link_or_copy": True,
    }
    files = FILES + ["simulator/crates/runtime/src/effekseer_geometry.rs"]
    source = []
    for path in files:
        raw = (ROOT / path).read_bytes()
        source.append({"path":path,"bytes":len(raw),"sha256":hashlib.sha256(raw).hexdigest()})
    pe = [dict(prior["perPE"][0], ring_vtable_rva=0x5f96e4, ring_reader_rva=0xef5c0,
               ring_begin_render_rva=0xefec0, ring_instance_render_rva=0xeffa0)]
    evidence = [block(image,r,n)for r,n in [(0xef5c0,0x32f),(0xefec0,0xd0),(0xeffa0,0x270),(0x18bbe1,0x1d)]]
    payload = {"summary":summary,"perPE":pe,"source_files":source,
               "remaining_implementation_families":remaining,"PE_evidence":evidence}
    out = ROOT / "reports"
    (out/(PREFIX+".json")).write_text(json.dumps(payload,indent=2)+"\n",encoding="utf-8")
    (out/(PREFIX+"_summary.json")).write_text(json.dumps({"summary":summary,"perPE":pe},indent=2)+"\n",encoding="utf-8")
    lines = ["# A: real Effekseer ring / timed color", "",summary["actual_pipeline"],"",
             "Ring is actual two-band radial geometry with three independent colors, matching the existing triangle diagonal and texture U progression/V halves. Inner/outer radius and height use fixed/PVA/easing; viewing angle and center ratio use fixed/easing. Degenerate random ranges are deterministic; other ranges explicitly reject. Sprite RGBA easing also uses preincrement render age while fade/UV retain postincrement living time. Full3D fixed and billboard transforms carry ring height to depth.","",
             "The node common data is independent from renderer data. None nodes do not emit geometry. MaxGeneration is now a signed-32 positive lifetime total, independently bounded by the live/releasing2000-slot manager pool; resources with >2000 total emissions are not rejected merely for that total.","",
             "Exact PE: SHA256 "+image.digest+"; kind4 factory/vtable5f96e4, reader ef5c0, begin efec0, instance rendering effa0, ctor18bbe1..18bbfb installs the ring renderer. Fixed public format and renderer behavior reference commit "+summary["reference_commit"]+". Independently implemented; no copied C++ or reference engine dependency.","",
             "No compile, tests, Rust/VM/game/DLL/APK execution. Only rustfmt syntax parsing and static PE/source reads. Native equivalence added0; no original-effect pixel acceptance claim.","","## Remaining families",""]
    lines += ["- "+r["family"]+": "+r["branch"] for r in remaining]
    lines += ["","## Modified source",""]+["- "+r["path"] for r in source]
    lines += ["","No new directories or root README/localization edits. Analysis scripts classified under scripts/analysis, fixed research text archived under archive/2026-11/20261117_effekseer101_*. No game/EXE assets included."]
    (out/(PREFIX+".md")).write_text("\n".join(lines)+"\n",encoding="utf-8")
    print(json.dumps(summary))


if __name__ == "__main__": main()

"""Static UV reader/sampler evidence and source ledger; no target execution."""
import hashlib
import json
from recovery_A_20261116 import ROOT, Image
from recovery_A_receiver_contracts_20261116 import block

PREFIX = "recovery_A_20261117_effekseer_uv"


def main():
    prior = json.loads((ROOT / "reports/recovery_A_20261117_effekseer_force.json").read_text(encoding="utf-8"))
    image = Image(ROOT / prior["perPE"][0]["path"], prior["perPE"][0]["sha256"])
    remaining = prior["remaining_implementation_families"]
    summary = {
        "batch": "actual UV FCurve timeline and signed scroll sampling",
        "real_pipeline": "SKFE texture UV4 -> four scalar FCurves(positionXY,sizeXY) -> postincrement particle age -> sprite/ring UV -> actual CPU texture samples/Canvas",
        "new_source_branches": ["position/size Vector2 FCurve payload", "four empty defaults0", "degenerate per-channel random offsets", "same native FCurve edge/frequency sampling", "scroll signed frame truncation", "UV nonfinite/native frame-domain diagnostics"],
        "PE_facts": {"reader": "dbdaa..dbdf6", "vector_constructor": "ee050", "vector_reader": "ee090", "sample": "e28f4..e2988", "scroll": "e2891..e28f1", "offset_init": "e1265..e132b"},
        "source_remaining_family_count": len(remaining), "whole_family_closures": 0,
        "completed_native_equivalence_count_added": 0,
        "compiled": False, "tests_run": False, "Rust_VM_game_DLL_APK_execution": False,
        "syntax_only": "rustfmt parser, not compiler/typecheck",
    }
    pe = [dict(prior["perPE"][0], texture_reader_rva=0xdbbf0,
               uv_curve_sample_rva=0xe28f4, vector2_curve_reader_rva=0xee090)]
    source = []
    for row in prior["source_files"]:
        data = (ROOT / row["path"]).read_bytes()
        source.append({"path": row["path"], "bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()})
    evidence = [block(image, rva, size) for rva, size in [
        (0xdbdaa, 0x4e), (0xee050, 0x68), (0xe28f4, 0x95),
        (0xe2891, 0x61), (0xe1265, 0xcc),
    ]]
    payload = {"summary": summary, "perPE": pe, "source_files": source,
               "remaining_implementation_families": remaining, "PE_evidence": evidence}
    out = ROOT / "reports"
    (out / (PREFIX + ".json")).write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
    (out / (PREFIX + "_summary.json")).write_text(json.dumps({"summary": summary, "perPE": pe}, indent=2) + "\n", encoding="utf-8")
    lines = ["# A: Effekseer texture timelines", "", summary["real_pipeline"], "",
             "Reader::node now accepts UV tag4 as positionXY and sizeXY scalar curves, using native Vector2 constructor default0 for all channels. Uv::Keys samples those curves and propagates explicit errors into Manager::draw_task; actual sprite corners/ring segment UVs consume the result through the existing CPU raster. No diagnostic image or optional adapter substitutes for the texture.", "",
             "PE dbdaa..dbdf6 reads two Vector2 FCurves without an additional version guard. ee050/ee090 constructs defaults0 and reads each channel. e1265..e132b initializes per-instance random offsets; only equal bounds are currently supported. e28f4..e2988 samples signed living frame plus UV start offset, whose legacy0..9 value is0. No UV FCurve coordinate/magnification conversion is invented.", "",
             "Scroll e2891..e28e0 truncates living time with signed CVTT before float multiplication; source now preserves that sequence. Animation checks the signed conversion domain of its frame quotient. Nonfinite outputs and unsupported random ranges reject explicitly. SKFE10+ remains rejected rather than guessing random animation-start, scroll-range or color-bind fields.", "",
             "Executable SHA256: " + image.digest, "",
             "No compile/typecheck, tests or Rust/VM/game/DLL/APK execution. rustfmt syntax reading only. Full native-equivalence added0;11 implementation families remain partial.", "", "## Remaining families", ""]
    lines += ["- " + row["family"] + ": " + row["branch"] for row in remaining]
    lines += ["", "Script classified under scripts/analysis; evidence in reports; no new directory, temporary artifact, root README, localization or asset edits. Independent Rust, no copied reference implementation."]
    (out / (PREFIX + ".md")).write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(json.dumps(summary))


if __name__ == "__main__":
    main()

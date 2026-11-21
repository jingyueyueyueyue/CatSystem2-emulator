"""Static PE evidence and delivery ledger for the independent HSVA receiver."""
import hashlib
import json
import struct
from recovery_A_20261116 import ROOT, Image
from recovery_A_receiver_contracts_20261116 import block

PREFIX = "recovery_A_20261117_effekseer_hsva"


def main():
    prior = json.loads((ROOT / "reports/recovery_A_20261117_effekseer_fcurve.json").read_text(encoding="utf-8"))
    image = Image(ROOT / prior["perPE"][0]["path"], prior["perPE"][0]["sha256"])
    remaining = prior["remaining_implementation_families"]
    for row in remaining:
        if row["family"] == "color":
            row["branch"] = "sprite RGBA FCurve and sprite/ring RGBA/HSVA fixed and easing paths implemented; nondegenerate random colors remain rejected without native RNG ownership"
    summary = {
        "batch": "sprite/ring HSVA and native color easing order",
        "real_pipeline": "SKFE color1/2 -> checked raw HSVA -> byte interpolation/truncation -> PE-bound HSV conversion -> sprite corners/ring radial bands -> existing textured CPU triangles/Canvas",
        "implemented_branches": ["deterministic random-color HSVA", "easing raw HSVA before RGB conversion", "252 hue clamp and six42-unit sectors", "stored f32 reciprocal constants", "unchanged alpha", "Horner color easing", "start.mode alone selects easing output in target PE"],
        "source_remaining_family_count": len(remaining),
        "whole_family_closures": 0,
        "completed_native_equivalence_count_added": 0,
        "syntax_only": "rustfmt parser; no compiler/typecheck",
        "compiled": False, "tests_run": False,
        "Rust_VM_game_DLL_APK_execution": False,
    }
    pe = [dict(prior["perPE"][0], hsva_converter_rva=0xecab0,
               color_interpolate_rva=0xf1af0, ring_color_update_rva=0xf0d30)]
    constants = []
    for rva, name in [(0x5f9fb4, "saturation reciprocal"), (0x5f9fc0, "hue reciprocal"), (0x5fa03c, "one")]:
        raw = image.pe.get_data(rva, 4)
        constants.append({"rva": rva, "name": name, "bytes": raw.hex(), "f32": struct.unpack("<f", raw)[0]})
    evidence = [block(image, rva, size) for rva, size in [
        (0xecab0, 0x13e), (0xed820, 0x14d), (0xf1af0, 0xea),
        (0xf0900, 0x8b), (0xf0c80, 0xac), (0xf0d30, 0xbf),
    ]]
    source = []
    for row in prior["source_files"]:
        path = row["path"]
        data = (ROOT / path).read_bytes()
        source.append({"path": path, "bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()})
    payload = {"summary": summary, "perPE": pe, "source_files": source,
               "remaining_implementation_families": remaining,
               "PE_constants": constants, "PE_evidence": evidence}
    out = ROOT / "reports"
    (out / (PREFIX + ".json")).write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
    (out / (PREFIX + "_summary.json")).write_text(json.dumps({"summary": summary, "perPE": pe}, indent=2) + "\n", encoding="utf-8")
    lines = ["# A: Effekseer HSVA", "", summary["real_pipeline"], "",
             "Source changes: effekseer_format::Reader::range_color/color_curve and effekseer_geometry::ColorCurve::value/hsva_color. No new public API or shared-host edit is needed. Sprite and ring now consume the parsed color space and emit actual vertex colors through the existing CPU raster path.", "",
             "PE ecab0..ecbed clamps hue to252, divides into42-unit sectors and multiplies the exact stored f32 reciprocals3b808081/3cc30c31. Alpha is preserved. Easing ed903..ed946 and f0d5d..f0da8 uses Horner evaluation, raw channel saturation/truncation, then HSV conversion selected only by start.mode; end.mode is parsed but does not select output. Tag0 fixed colors and sprite FCurve remain RGBA. Reserved color byte is consumed without invented semantics.", "",
             "Ring reader f0900, initializer f0c80 and updater f0d30 confirm the same actual converter. Nondegenerate ranges still fail explicitly; this batch does not introduce a replacement RNG. Unknown color-space tags fail explicitly.", "",
             "Exact executable SHA256: " + image.digest, "",
             "No compile, tests or Rust/VM/game/DLL/APK execution. rustfmt syntax reading only. No complete native-equivalence count added;11 implementation families remain partial.", "", "## Remaining families", ""]
    lines += ["- " + row["family"] + ": " + row["branch"] for row in remaining]
    lines += ["", "Analysis script classified under scripts/analysis; PE evidence embedded in reports. Existing pinned MIT behavior research only, independently written Rust. No new directory, temporary file, game asset, root README or localization edit."]
    (out / (PREFIX + ".md")).write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(json.dumps(summary))


if __name__ == "__main__":
    main()

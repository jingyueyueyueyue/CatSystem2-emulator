"""Static PE/source ledger for rendered-instance fade, without execution."""
import hashlib
import json
import struct

from recovery_A_20261116 import ROOT, Image
from recovery_A_receiver_contracts_20261116 import block

PREFIX = "recovery_A_20261117_effekseer_fade"


def main():
    prior = json.loads((ROOT / "reports/recovery_A_20261117_effekseer_ribbon_camera.json").read_text(encoding="utf-8"))
    image = Image(ROOT / prior["perPE"][0]["path"], prior["perPE"][0]["sha256"])
    remaining = prior["remaining_implementation_families"]
    for row in remaining:
        if row["family"] == "color":
            row["branch"] = "sprite/ring/ribbon fixed, deterministic ranges, RGBA/HSVA easing, sprite FCurve, PE byte modulation and preincrement base-alpha fade are implemented; nondegenerate color RNG and parent color binding remain incomplete"
    summary = {
        "batch": "sprite/ring/ribbon rendered-instance fade order",
        "real_pipeline": "Renderer::fade(render_age) -> geometry::fade_alpha base color -> sprite corner/ribbon edge -> play-global byte modulation -> existing CPU texture raster",
        "new_source_branches": ["preincrement fade age for sprite and ring", "Horner fade-in/out with native strict comparisons", "base-alpha fade precedes corner/global modulation", "ring fades all three radial colors separately", "finite signed truncation then low-byte wrapping; no invented alpha clamp", "ribbon shares the proven fade helper"],
        "explicit_boundaries": ["nonfinite fade including activated zero-duration division", "alpha outside signed CVTTSS2SI domain", "nondegenerate color RNG", "parent color binding"],
        "source_remaining_family_count": len(remaining),
        "whole_family_closures": 0,
        "completed_native_equivalence_count_added": 0,
        "compiled": False, "tests_run": False, "Rust_VM_game_DLL_APK_execution": False,
        "syntax_only": "rustfmt parser, not compiler/typecheck",
    }
    pe = [dict(prior["perPE"][0], fade_alpha_rva=0xdd9d0,
               sprite_update_rva=0xedb80, sprite_fade_rva=0xedd09,
               ring_update_rva=0xf0490, ring_three_color_calls_rva=0xf0765,
               ring_fade_rva=0xf0db5)]
    evidence = [block(image, rva, size) for rva, size in [
        (0xdd9d0, 0xcc), (0xedd09, 0xab), (0xf0765, 0x40),
        (0xf0d30, 0xc2), (0xeeae6, 0x3d), (0xed450, 0x33),
    ]]
    tables = []
    for rva in (0x5f96e4, 0x5f97f4, 0x5f9844):
        raw = image.pe.get_data(rva, 72)
        tables.append({"rva": rva, "bytes": raw.hex(),
                       "entries_rva": [x - image.base for x in struct.unpack("<18I", raw)],
                       "GetFadeAlpha_slot": 16})
    source = []
    for row in prior["source_files"]:
        data = (ROOT / row["path"]).read_bytes()
        source.append({"path": row["path"], "bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()})
    payload = {"summary": summary, "perPE": pe, "source_files": source,
               "remaining_implementation_families": remaining,
               "vtable_evidence": tables, "PE_evidence": evidence}
    out = ROOT / "reports"
    (out / (PREFIX + ".json")).write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
    (out / (PREFIX + "_summary.json")).write_text(json.dumps({"summary": summary, "perPE": pe}, indent=2) + "\n", encoding="utf-8")
    lines = ["# A: Effekseer rendered-instance fade", "", summary["real_pipeline"], "",
             "Source: effekseer.rs::Renderer::fade and Manager::draw_task; effekseer_geometry.rs::Geometry::quads/fade_alpha; effekseer_ribbon.rs::Ribbon::quads. These feed the already installed texture/depth/blend/Canvas path. No host adapter or diagnostic image is substituted.", "",
             "The three real node vtables bind slot+40 to dd9d0. dd9f0..dda29 evaluates fade-in using current instance+158, and dda3b..dda90 evaluates fade-out with strict age+frames>life and Horner f32 order. Sprite edd09..edd40 applies the factor to allColor alpha during rendered-instance update; ring f0765..f07a0 calls f0d30 for all three radial colors and f0db5..f0de8 performs the same alpha operation. Ribbon eeae6..eeb1d agrees. Corner/edge and global modulation occur afterwards. UV remains separately sampled using the postincrement clock.", "",
             "Removed the old sprite/ring postincrement fade and final-vertex alpha multiplication. The shared helper preserves native signed truncation followed by low-byte storage rather than imposing 0..1 or byte clamping. Nonfinite fade and conversion-outside-domain cases still produce explicit diagnostics. Parent color-binding and nondegenerate RNG are separate missing branches, not silently supplied.", "",
             "Fixed executable SHA256: " + image.digest, "",
             "No compile/typecheck, tests, Rust function/VM/game/DLL/APK execution. rustfmt syntax reading only. Full native-equivalence added0;11 families remain partial.", "", "## Remaining families", ""]
    lines += ["- " + row["family"] + ": " + row["branch"] for row in remaining]
    lines += ["", "Analysis script classified under scripts/analysis; formal byte/hash/disassembly evidence under reports. No temporary outputs, new directories, root README, localization, game assets, shared host or Emote edits."]
    (out / (PREFIX + ".md")).write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(json.dumps(summary))


if __name__ == "__main__":
    main()

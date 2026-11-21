"""Static delivery ledger for actual parent-group ribbon geometry."""
import hashlib
import json
from recovery_A_20261116 import ROOT, Image
from recovery_A_receiver_contracts_20261116 import block

PREFIX = "recovery_A_20261117_effekseer_ribbon"


def main():
    prior = json.loads((ROOT / "reports/recovery_A_20261117_effekseer_uv.json").read_text(encoding="utf-8"))
    image = Image(ROOT / prior["perPE"][0]["path"], prior["perPE"][0]["sha256"])
    remaining = prior["remaining_implementation_families"]
    for row in remaining:
        if row["family"] == "other_geometry":
            row["branch"] = "sprite/ring and fixed-frame group-connected ribbon have real pixel source; viewpoint-dependent ribbon, SKFE13 spline subdivisions, model/track remain explicitly unsupported"
        if row["family"] == "instance_tree":
            row["branch"] += "; ribbon group InstanceCount parity across mixed removing/active states remains unaudited"
    summary = {
        "batch": "fixed-frame ribbon group receiver and native byte color modulation",
        "real_pipeline": "SKFE kind3 -> true ribbon layout/color/width -> same-parent ordered active group -> independent left/right world edges -> adjacent-edge quads -> existing depth/blend/texture CPU raster/Canvas",
        "new_source_branches": ["ribbon lacks sprite/ring order field", "legacy/current blend and width layout", "fixed/degenerate-random/easing RGBA or HSVA", "group ownership preserved", "group-first UV at index/(count-1)", "default corners bypass modulation", "actual native SIMD byte multiplication", "preincrement ribbon fade before edge/global color"],
        "not_supported": ["viewpoint-dependent ribbon frame", "SKFE13 spline subdivision", "nondegenerate RNG", "model/track", "mixed removing/active group count parity unaudited"],
        "source_remaining_family_count": len(remaining), "whole_family_closures": 0,
        "completed_native_equivalence_count_added": 0,
        "compiled": False, "tests_run": False, "Rust_VM_game_DLL_APK_execution": False,
        "syntax_only": "rustfmt parser, not typecheck",
    }
    pe = [dict(prior["perPE"][0], ribbon_vtable_rva=0x5f97f4,
               ribbon_reader_rva=0xee2a0, ribbon_group_begin_rva=0xee560,
               ribbon_instance_render_rva=0xee600, ribbon_color_update_rva=0xeea40,
               ribbon_renderer_vtable_rva=0x5f9ca8, ribbon_renderer_geometry_rva=0xffb00)]
    source = []
    for path in [r["path"] for r in prior["source_files"]] + ["simulator/crates/runtime/src/effekseer_ribbon.rs"]:
        data = (ROOT / path).read_bytes()
        source.append({"path": path, "bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()})
    evidence = [block(image, rva, size) for rva, size in [
        (0xddc59, 0x59), (0xee2a0, 0x1fb), (0xee560, 0x68),
        (0xee600, 0x2c0), (0xeea40, 0xeb), (0xf45b0, 0x65),
        (0xf4f60, 0x11a), (0x1008a9, 0x103), (0x100dde, 0x141),
        (0xed450, 0x33), (0xf00ae, 0x22), (0x18c6d0, 0x4b),
    ]]
    mask = image.pe.get_data(0x5fa550, 16)
    payload = {"summary": summary, "perPE": pe, "source_files": source,
               "remaining_implementation_families": remaining,
               "SIMD_mask": {"rva": 0x5fa550, "bytes": mask.hex(), "byte_operation": "(product >>8) | (product &1), not division255"},
               "research_reference": {"commit": "4a3f2649f4937538279450ed244319bdcef0617d", "archived_reference_text_count": 29,
                   "target_geometry_disassembly": "archive/2026-11/20261117_effekseer101_ribbon_geometry_pe.txt", "source_copy": False},
               "PE_evidence": evidence}
    out = ROOT / "reports"
    (out / (PREFIX + ".json")).write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
    (out / (PREFIX + "_summary.json")).write_text(json.dumps({"summary": summary, "perPE": pe}, indent=2) + "\n", encoding="utf-8")
    lines = ["# A: Effekseer actual ribbons", "", summary["real_pipeline"], "",
             "New effekseer_ribbon.rs::Ribbon::quads consumes concrete same-parent groups from Tree::visible_groups. Adjacent instance edges generate actual textured quads with original diagonal; groups do not join unrelated parents. Manager::draw_task projects these world vertices directly and feeds the already-installed CPU raster, without an adapter or alternative preview.", "",
             "Reader ee2a0 confirms no order field for kind3, no color FCurve, two edge colors and two width positions, with default-position payload from version8. Constructor ddc59..ddcad sets UV0/0/1/1 and subdivision1 for accepted0..9. ee595..ee5a8 samples only the group's first UV; actual renderer10093a..1009a7 distributes V using index/(count-1). This differs from older public101 renderer text. Viewpoint-dependent frames still reject explicitly in this batch.", "",
             "Target colors use pmullw, mask0001 at5fa550, shift8, OR: (a*b>>8)|(a*b&1). Geometry::modulate now follows that byte operation for proven sprite/ring/global and new ribbon paths. Default corner/edge tags bypass multiplication; multiplying by255 is not generally identity in this PE. Ribbon fade uses preincrement age and precedes edge/global multiplication, per eeaee..eeb1d. Sprite/ring fade-order parity is not claimed closed by this batch.", "",
             "Executable SHA256: " + image.digest, "",
             "No compilation/typecheck, tests or Rust/VM/game/DLL/APK execution. rustfmt syntax reading only. Full native equivalence added0;11 families remain partial.", "", "## Remaining families", ""]
    lines += ["- " + row["family"] + ": " + row["branch"] for row in remaining]
    lines += ["", "New source: simulator/crates/runtime/src/effekseer_ribbon.rs; local changes: effekseer.rs/effekseer_format.rs/effekseer_geometry.rs/effekseer_tree.rs. No desktop/main hooks changed in this batch.", "",
              "Analysis script classified under scripts/analysis. Three pinned MIT ribbon research texts and temporary disassembly are archived under existing archive/2026-11/20261117_effekseer101_*; reference manifest updated to29 texts. No new directories, root README, localization, game assets or Emote edits."]
    (out / (PREFIX + ".md")).write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(json.dumps(summary))


if __name__ == "__main__":
    main()

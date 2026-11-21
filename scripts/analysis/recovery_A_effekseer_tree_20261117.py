"""Static source delivery ledger: tree/queue batch, not an execution test."""
import hashlib
import json

from recovery_A_20261116 import ROOT, Image
from recovery_A_receiver_contracts_20261116 import block
from recovery_A_effekseer_backend_20261117 import REMAINING

PREFIX = "recovery_A_20261117_effekseer_tree"
FILES = ["simulator/crates/runtime/src/" + n for n in [
    "effekseer.rs", "effekseer_format.rs", "effekseer_tree.rs", "effekseer_transform.rs",
]] + ["simulator/crates/desktop/src/effekseer_native.rs"]


def main():
    image = Image(ROOT / "Yukikoi Melt/YukikoiMelt.exe",
                  "63de0e14cd8ce11ad2d68e7ed6076641ff5e5b73d822603cc52764009d812976")
    remaining = [dict(x) for x in REMAINING]
    for x in remaining:
        if x["family"] == "instance_tree":
            x["branch"] = "recursive none/sprite and per-parent groups implemented; negative offset/prewarm and >16 children remain explicitly rejected; target instance helper parity not yet individually matched"
        if x["family"] == "transforms":
            x["branch"] = "XYZ Euler, bind0..3, fixed/ordinary/rotated/Y-fixed billboard implemented; axis and FCurve pending; singular/nonfinite decomposition explicit error"
    summary = {
        "batch": "recursive deterministic instance tree and raw70 owned queue",
        "actual_source_pipeline": "SKFE recursive none/sprite -> parent-owned child groups -> frame schedule/bind matrices -> Quad vertices -> existing desktop textured raster -> Canvas RGBA",
        "new_source_branches": ["recursive none/sprite", "per-parent child schedules", "first update enables stepping only", "preincrement matrix/postincrement UV and fade", "life/parent/children removal", "detach preserves last parent matrix", "two-phase removing pool slots", "pool exhaustion retries generation", "container/group draw ordering", "XYZ Euler", "translation/rotation/scale bind0..3", "all four billboard tags", "raw70 owned queue before body consumption"],
        "source_remaining_family_count": len(remaining),
        "remaining_family_count_is_not_unimplemented_function_count": True,
        "whole_family_closures": 0,
        "completed_native_equivalence_count_added": 0,
        "compiled": False, "tests_run": False, "Rust_VM_game_DLL_APK_execution": False,
        "syntax_only": "rustfmt parse of A files; no compiler",
        "queue_fix": {"enqueue": "queue_task_command_buffers copies path/vector buffers", "consume": "native_tasks Phase::Body -> update_task_command_buffers -> route_task_command_buffers -> installed type61 receiver", "no_IO_or_manager_mutation_at_enqueue": True},
        "reference_commit": "4a3f2649f4937538279450ed244319bdcef0617d",
        "reference_scope": "independent format/behavior implementation; MIT research texts, no C++ copying; PE node format and recursive factory checked, instance helpers not yet fully matched",
    }
    pe = [{"path": "Yukikoi Melt/YukikoiMelt.exe", "sha256": image.digest,
           "node_factory_rva": 0xddbd0, "node_reader_rva": 0xdc3f0,
           "recursive_child_read_rva": 0xdd6a7, "none_vtable_rva": 0x5f9694,
           "sprite_vtable_rva": 0x5f9844, "task_receiver_rva": 0x18c1f0}]
    source = []
    for path in FILES:
        data = (ROOT / path).read_bytes()
        source.append({"path": path, "bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()})
    evidence = [block(image, rva, size) for rva, size in [
        (0xddbd0, 0x1f0), (0xdc3f0, 0x170), (0xdd6a7, 0x5c), (0x18c1f0, 0x6e),
    ]]
    payload = {"summary": summary, "perPE": pe, "source_files": source,
               "remaining_implementation_families": remaining, "PE_evidence": evidence}
    out = ROOT / "reports"
    (out / (PREFIX + ".json")).write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
    (out / (PREFIX + "_summary.json")).write_text(json.dumps({"summary": summary, "perPE": pe}, indent=2) + "\n", encoding="utf-8")
    lines = ["# A: Effekseer recursive tree / queue delivery", "",
             summary["actual_source_pipeline"], "",
             "Source is concrete and connected to the existing CPU pixel path. No compilation, tests or Rust/VM/game/DLL/APK execution; full native equivalence added0.", "",
             "Raw70 bug confirmed and fixed: dispatch_effekseer_message now enqueues owned paths/vectors using queue_task_command_buffers. The existing Body stage drains them through update_task_command_buffers. Sync messages still route immediately. Acceptance no longer performs resource IO or manager mutations.", "",
             "Node0 none carries common transforms/timing/children but emits no replacement geometry. Node2 sprites can now nest. Each parent owns separate child groups; native container preorder and per-group generation order are retained. Parent death detaches after calculating its last binding; children can outlive parents. Removing pool slots survive two cleanup phases. Capacity2000 stalls the due generation rather than silently dropping it.", "",
             "SRT uses row vectors and ZXY order. Bind0/1/2/3 independently controls translation, rotation, scale; all-parent/all-root binding preserves the raw3x3, mixed binding follows row-norm GetSRT including shear. Fixed sprites transform actual3D corners. Ordinary, rotated and Y-fixed billboards follow the fixed type61 camera. Singular/nonfinite matrices return an explicit diagnostic.", "",
             "Fixed PE SHA256 " + image.digest + "; node factory ddbd0, common reader dc3f0, recursive children dd6a7..dd6fb; none/sprite vtables5f9694/5f9844. The reader explicitly has additional>=10 and>=13 fields, so this batch still rejects SKFE10+. Instance behavior additionally uses the pinned MIT format/behavior reference; it is not claimed individually PE-matched for every helper.", "",
             "## Remaining families", ""]
    lines += ["- " + x["family"] + ": " + x["branch"] for x in remaining]
    lines += ["", "## Source files", ""] + ["- " + f["path"] for f in source]
    lines += ["", "Scripts remain under scripts/analysis; newly read pinned research texts are archive/2026-11/20261117_effekseer101_*. No new directory, root README, localization or game asset edits."]
    (out / (PREFIX + ".md")).write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(json.dumps(summary))


if __name__ == "__main__":
    main()

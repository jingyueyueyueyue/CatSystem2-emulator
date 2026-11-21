"""Bounded static ledger for live type61 leaf rotation and group audit."""
import hashlib
import json
import struct

from recovery_A_20261116 import ROOT, Image
from recovery_A_receiver_contracts_20261116 import block

PREFIX = "recovery_A_20261117_effekseer_rotation"


def main():
    prior = json.loads((ROOT / "reports/recovery_A_20261117_effekseer_fade.json").read_text(encoding="utf-8"))
    image = Image(ROOT / prior["perPE"][0]["path"], prior["perPE"][0]["sha256"])
    remaining = prior["remaining_implementation_families"]
    for row in remaining:
        if row["family"] == "instance_tree":
            row["branch"] = "recursive per-parent groups and active-only ribbon count now PE-audited; negative offset/prewarm, >16 children and other helper parity remain"
        if row["family"] == "task_rotation":
            row["branch"] = "native93/typed55/raw69/raw70 leaf live root rotation implemented; registered-child command56 broadcast explicitly rejected; other delegated/base branches remain"
    summary = {
        "batch": "leaf live root rotation and active/removing group audit",
        "real_pipeline": "native93/typed55/raw69/raw70 -> owned XYZ radians -> current playback root Z*X*Y with native trig -> tree parent binding -> actual sprite/ring/ribbon geometry -> installed CPU pixels",
        "runtime_APIs": ["Manager::rotation_reads_data", "Manager::set_live_rotation"],
        "host_APIs": ["LoggingHost::effekseer_rotation_native", "LoggingHost::receive_effekseer_transform"],
        "registered_child_command56": "explicit diagnostic, not dropped or claimed complete",
        "repeat_inherits_old_rotation": False,
        "no_handle_sync_reads_pointer": False,
        "queued_nonnull_XYZ_owned_before_consumption": True,
        "active_removing_group_count_PE_audited": True,
        "source_remaining_family_count": len(remaining),
        "completed_native_equivalence_count_added": 0,
        "compiled": False, "tests_run": False, "Rust_VM_game_DLL_APK_execution": False,
        "syntax_only": "rustfmt parser, not typecheck",
        "next_priority": "actual PSB/EFK rendering/time/texture/nesting blockers, not exceptional f32 refinement",
    }
    pe = [dict(prior["perPE"][0], ordinary93_rva=0x1956c0,
               manager_vtable_rva=0x5f9528, live_rotation_rva=0xd8600,
               trig_rva=0xd2e50, child_broadcast_rva=0x1376b0,
               group_update_rva=0xe3580, group_cleanup_rva=0xe29c0)]
    evidence = [block(image, r, n) for r, n in [
        (0x1956c0, 0x73), (0x18c347, 0x57), (0xd8600, 0x20f),
        (0xd2e50, 0x11e), (0xe2ca0, 0x178), (0xe2f60, 0xc7),
        (0x1376b0, 0xda), (0xe3580, 0x110), (0xe29c0, 0x50),
        (0xee578, 0x30), (0xe13d8, 0xa6), (0xe1ded, 9),
    ]]
    constants = [{"rva": r, "bytes": image.pe.get_data(r, 4).hex(),
                  "f32": struct.unpack("<f", image.pe.get_data(r, 4))[0]}
                 for r in (0x5f9fe0, 0x5fa23c, 0x5f9fe4, 0x5f9fb8,
                           0x5f9fc4, 0x5f9fa8, 0x5f9fb0, 0x5f9f98,
                           0x5f9f9c, 0x5f9f88, 0x5f9f90)]
    paths = [x["path"] for x in prior["source_files"]] + [
        "simulator/crates/runtime/src/effekseer_trig.rs",
        "simulator/crates/desktop/src/task_commands.rs",
        "simulator/crates/desktop/src/lib.rs",
    ]
    source = [{"path": p, "sha256": hashlib.sha256((ROOT / p).read_bytes()).hexdigest()}
              for p in dict.fromkeys(paths)]
    payload = {"summary": summary, "perPE": pe, "source_files": source,
               "remaining_implementation_families": remaining,
               "PE_evidence": evidence, "trig_constants": constants,
               "manager_rotation_slot": {"rva": 0x5f9528 + 0xc8,
                    "bytes": image.pe.get_data(0x5f9528 + 0xc8, 4).hex(),
                    "entry_rva": image.u32(0x5f9528 + 0xc8) - image.base}}
    (ROOT / "reports" / (PREFIX + ".json")).write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
    (ROOT / "reports" / (PREFIX + "_summary.json")).write_text(json.dumps({"summary": summary, "perPE": pe}, indent=2) + "\n", encoding="utf-8")
    lines = ["# A: type61 live leaf rotation", "", summary["real_pipeline"], "",
        "New effekseer_trig.rs; local changes effekseer.rs/effekseer_tree.rs/effekseer_transform.rs and desktop/effekseer_native.rs. Two additive hooks: desktop/lib.rs before movie wrappers, and native_tasks/task_commands.rs only kind61 after the existing kind54 hook. No task factory or M2 code changed.", "",
        "Native93 at1956c0 forwards command55, selector and actual XYZ f32 stack buffer. Receiver18c347 tests stored handle!=-1 before reading the vector; manager vtable5f9528+c8 points tod8600. That setter uses current root/stored matrix, keeps row lengths/translation and composes identity*Z*X*Y. The apparent repeated scaleX is a stack-reading error: d8742 reads before add esp4; d8774 and d87a7 then read Y/Z. The new kernel uses exact d2e50 constants and arithmetic. New and repeat Play reset rotation; no task rotation property was invented.", "",
        "PE18c383 always calls1376b0 with command56 to registered children. Leaf receiver chains now execute; non-leaf chains explicitly fail before the rotation mutation. No child broadcast, full scheduler parity or complete native equivalence is claimed. Raw70 owns nonnull vector bytes at enqueue; synchronous no-handle commands do not read a VM pointer.", "",
        "Group audit: e35d3..e3638 immediately removes dead instances from active14/18/1c into removing20/24/28; e29c0 advances removing1->2 then frees2. ee57c reads active count1c and ee595 reads active head14. Current active-only Tree::visible_groups is consistent; no code change needed for this subbranch.", "",
        "Verification: rustfmt syntax reading only; no compilation/typecheck, tests, function/VM/game/DLL/APK execution.11 families remain partial; complete-native count added0. Next work follows actual asset usage. Main-owned M2 surface/replacement and driver transforms preserved.", "",
        "Scripts classified under scripts/analysis; formal evidence in reports; no temporary output, new directory, root README, localization or game asset edit."]
    (ROOT / "reports" / (PREFIX + ".md")).write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(json.dumps(summary))


if __name__ == "__main__":
    main()

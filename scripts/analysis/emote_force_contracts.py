"""Read the fixed NEKO force/accumulator contracts and config inventory only."""
from __future__ import annotations

import hashlib
import json
import subprocess
import sys
from pathlib import Path

from audit_runtime_evidence import blowfish_le_decrypt, kif_entries
from recovery_A_20261116 import ROOT, Image
from recovery_A_emote_host_20261117 import sections
from root_clock_profiles import snippet

PE = "c71c5b85e6eca660474f156d96b0021a53a5f05690dd077d7861d72ff5b11adc"
SOURCES = ["simulator/crates/runtime/src/" + name for name in (
    "emote_m2force.rs", "emote_m2physics.rs", "emote_m2live.rs", "emote_cpu.rs", "emote.rs",
    "emote_m2settings.rs", "emote_m2banks.rs",
)] + ["simulator/crates/desktop/src/" + name for name in (
    "emote_native.rs", "emote_settings.rs", "emote_geometry.rs", "fes_services.rs",
)]


def syntax_read() -> None:
    rows = []
    for name in SOURCES:
        raw = (ROOT / name).read_bytes()
        run = subprocess.run(
            [str(Path.home() / ".cargo/bin/rustfmt.exe"), "--edition", "2024",
             "--config", "skip_children=true", "--emit", "stdout"],
            input=raw, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, check=False,
        )
        rows.append({"path": name, "sha256": hashlib.sha256(raw).hexdigest(),
                     "syntax_exit": run.returncode,
                     "diagnostic": run.stderr.decode("utf-8", errors="replace")})
    path = ROOT / "archive/2026-11/20261117_emote_force_source_syntax.json"
    path.write_text(json.dumps({"grammar_only": True, "compiled": False,
                                "type_checked": False, "tests_run": False,
                                "sources": rows}, indent=2) + "\n", encoding="utf-8")
    if any(row["syntax_exit"] for row in rows):
        raise ValueError("Rust grammar read failed; see archived diagnostics")


def main() -> None:
    prior = json.loads((ROOT / "reports/recovery_A_20261116_emote.json").read_text(encoding="utf-8"))
    image = Image(ROOT / prior["binary_path"], PE)
    spans = {
        "defval_explicit_or_driver_registration": (0x175CD6, 0x37),
        "secondary_defval_registration": (0x175FDC, 0x3D),
        "accumulator_baseline_set": (0x183260, 0x1EB),
        "accumulator_prepare": (0x183690, 0x105),
        "accumulator_driver_commit": (0x1837A0, 0x15A),
        "force_update_order_and_profile_input": (0x173EF7, 0x108),
        "force_clock_and_axis_input": (0x193090, 0x170),
        "force_integer_steps_and_output": (0x1933AE, 0x4C),
        "difference_constructor_reciprocal_maximum": (0x192E20, 0x12F),
        "difference_read_all_before_impulses": (0x194500, 0x86),
        "difference_impulse": (0x194682, 0x71),
        "material_spring": (0x194730, 0x18F),
        "material_registered_output": (0x1948C0, 0x149),
        "hold_reset": (0x193820, 0x31),
        "level_group_parser": (0x1A80CC, 0x233),
        "enable_group_parser": (0x1A83BC, 0x22D),
        "material_group_level": (0x193C30, 0x560),
        "material_group_enable": (0x194190, 0x370),
        "global_level_setter": (0x181210, 0x80),
        "global_enable_setter": (0x181290, 0x70),
        "global_group_getters": (0x1815A0, 0xC0),
        "fes_m2_getconf": (0x386970, 0x4E0),
        "fes_m2_setconf": (0x386E50, 0x500),
        "wrapper_position_defaults": (0x170EEA, 0x28),
        "wrapper_force_position_setter": (0x1743E0, 0x3A),
        "wrapper_force_link_gate": (0x173F34, 0x50),
        "force_enable_history_reset": (0x193400, 0x3C3),
        "group_resource_transition_producer": (0x178570, 0x168),
        "group_transition_load_caller": (0x171B18, 0x60),
        "group_identity_comparison": (0x1878B0, 0xE2),
        "group_first_impulse_mark": (0x1943D0, 0xC3),
        "group_first_impulse_consumer": (0x194682, 0x71),
        "group_path_basename": (0x2A7600, 0x54),
        "group_path_strip_extension": (0x2A6ED0, 0xB0),
    }
    evidence = {name: snippet(image, start, length) for name, (start, length) in spans.items()}
    contracts = {
        "force_clock_and_axis_input": [("mov", "dword ptr [edi + 0x60], 0"),
                                       ("movss", "dword ptr [edi], xmm2")],
        "force_integer_steps_and_output": [("call", "0x594500"), ("call", "0x594730"),
                                           ("call", "0x5948c0"), ("sub", "eax, ecx")],
        "difference_impulse": [("mulss", "xmm1, dword ptr [esi + edx + 0x34]"),
                               ("mulss", "xmm1, dword ptr [esi + edx + 0x38]"),
                               ("mulss", "xmm1, dword ptr [esi + edx + 0x30]")],
        "accumulator_driver_commit": [("call", "dword ptr [eax + 0x90]")],
    }
    for name, required in contracts.items():
        actual = {(i["mnemonic"], i["operands"]) for i in evidence[name]["instructions"]}
        if not set(required).issubset(actual):
            raise ValueError(f"Fixed PE contract changed: {name}")
    archive = image.path.parent / "m2config.int"
    entries, _, cipher = kif_entries(archive, 0x0F6BB0AE)
    configs = []
    group_configs = []
    with archive.open("rb") as handle:
        for entry in entries:
            if not entry["name"].lower().endswith((b".inc", b".dif", b".cnf", b".pfl")):
                continue
            handle.seek(entry["offset"])
            raw = handle.read(entry["size"])
            raw = blowfish_le_decrypt(cipher, raw) if cipher else raw
            config = sections(raw)
            if config.get("group") or config.get("group_inertia"):
                group_configs.append({"name": entry["name"].decode("cp932"),
                    "decoded_sha256": hashlib.sha256(raw).hexdigest(),
                    "groups": len(config.get("group", [])),
                    "resource_names": sum(len(row) for row in config.get("group", [])),
                    "inertia_variables": len(config.get("group_inertia", []))})
            if not config.get("material") and not config.get("inertia_dif"):
                continue
            dif = config.get("inertia_dif", [])
            configs.append({
                "name": entry["name"].decode("cp932"),
                "decoded_sha256": hashlib.sha256(raw).hexdigest(),
                "records": {k: len(config.get(k, [])) for k in (
                    "material", "inertia_lr", "inertia_ud", "inertia_dif", "defval",
                    "material_min", "material_max", "inertia_lv", "inertia_enable")},
                "difference_zero_ranges": sum(float(row[3]) == 0 for row in dif),
                "difference_duplicate_pairs": len(dif) - len({tuple(row[:2]) for row in dif}),
            })
    result = {
        "schema": "cs2-emote-force-contracts/v1", "pe_sha256": PE,
        "binary_path": prior["binary_path"], "evidence": evidence,
        "configs": configs,
        "group_configs": group_configs,
        "source_sha256": {name: hashlib.sha256((ROOT / name).read_bytes()).hexdigest() for name in SOURCES},
        "implemented_chain": [
            "actual primary/secondary defval -> distinct owner accumulator -> per-tick prepare",
            "actual message51/native84 bc/c0 -> LR/UD impulses; sampled driver+accumulator -> DIF impulses",
            "DIF uses reciprocal range and constructor material maximum, in original f32 order",
            "DWORD task clock -> every integer spring step -> registered material variable output",
            "real zero-duration driver setter -> actual PSB controller/timeline -> renderer",
            "907 reset consumes live offsets; same-owner selection retains force history; replacement releases it",
            "actual inertia_lv/inertia_enable named groups -> base/min/max five-lane interpolation and material enables",
            "FES m2_getconf/m2_setconf -> registered global group maps -> existing live owners and future loads",
            "known group changes reset material springs and all LR/UD/DIF histories before real force/render consumption",
            "position command linked flag160 -> temporary force suppression/restoration before first actual position command",
            "different resource stems in actual secondary group -> group_inertia marks -> one-shot DIF maximum*reciprocal impulse",
        ],
        "remaining": [
            "persistent M2 settings export/import and other m2_getconf/m2_setconf keys",
            "external window movement offset producer",
            "secondary controller effects and complete Emote object graph save/restore",
            "other original PE profiles, complete function equivalence, full-game acceptance",
        ],
        "ordinary_routes_added": 0, "full_function_equivalence_added": 0,
        "compiled": False, "type_checked": False, "tests_run": False,
        "original_code_executed": False,
    }
    if "--syntax" in sys.argv:
        syntax_read()
    path = ROOT / "reports/emote_force_contracts_20261117.json"
    path.write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"report": str(path.relative_to(ROOT)), "configs": len(configs),
                      "material_records": sum(c["records"]["material"] for c in configs),
                      "difference_records": sum(c["records"]["inertia_dif"] for c in configs),
                      "compiled": False, "tests_run": False}))


if __name__ == "__main__":
    main()

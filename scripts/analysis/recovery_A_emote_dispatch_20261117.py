"""Inventory the single Emote descriptor table and bounded original tint bytes.

No CFG sweep, original execution, Rust compilation or tests. The Rust descriptor
table is the ABI source; this script cross-references the earlier PE evidence
and emits a small integration/workload handoff. Run with python -B.
"""
from __future__ import annotations

from collections import Counter
from datetime import datetime, timezone
import json
import re

from recovery_A_20261116 import ROOT, Image, read_json, sha
from recovery_A_receiver_contracts_20261116 import block

DISPATCH = "simulator/crates/runtime/src/emote_dispatch.rs"
SOURCES = [
    DISPATCH,
    "simulator/crates/runtime/src/emote.rs",
    "simulator/crates/runtime/src/emote_cpu.rs",
    "simulator/crates/desktop/src/emote_native.rs",
    "scripts/analysis/recovery_A_emote_dispatch_20261117.py",
]


def arguments(text):
    """Split macro fields without splitting strings or nested constructors."""
    fields, start, depth, quoted, escaped = [], 0, 0, False, False
    for i, char in enumerate(text):
        if quoted:
            if escaped:
                escaped = False
            elif char == "\\":
                escaped = True
            elif char == '"':
                quoted = False
        elif char == '"':
            quoted = True
        elif char in "({[":
            depth += 1
        elif char in ")}]":
            depth -= 1
        elif char == "," and depth == 0:
            fields.append(text[start:i].strip())
            start = i + 1
    fields.append(text[start:].strip())
    if depth or quoted:
        raise ValueError("Unbalanced descriptor row")
    return fields


def descriptor_rows(evidence):
    contracts = {r["index"]: r for r in evidence["priority_contracts"]}
    rows = []
    for line in (ROOT / DISPATCH).read_text(encoding="utf-8").splitlines():
        matched = re.fullmatch(r"\s*(single|batch|blocked)!\((.*)\),\s*", line)
        if not matched:
            continue
        kind, text = matched.groups()
        parts = arguments(text)
        index, rva = int(parts[0]), int(parts[1], 0)
        contract = contracts[index]
        if rva != contract["entry_rva"]:
            raise ValueError(f"Stale RVA for native{index}")
        row = {
            "index": index,
            "wrapper_rva": rva,
            "shape": kind,
            "argument_bytes": 8 if kind == "batch" else int(parts[2]),
            "reply": "Continue" if kind == "batch" else parts[3],
            "commands": contract["commands"],
            "static_callsites": contract["callsites"],
            "group": contract["group"],
            "parameter_words": contract["words"],
            "original_contract": contract["contract"],
            "wrapper_ready": kind != "blocked",
            "integrated_completed": False,
        }
        if row["argument_bytes"] != len(contract["words"]) * 4:
            raise ValueError(f"Stale argument count for native{index}")
        observed = contract["argument_bytes_counts"]
        if any(int(size) != row["argument_bytes"] for size in observed):
            raise ValueError(f"Argument count conflicts with KCS provenance: {index}")
        expected_reply = ("ThreeWords" if index == 910 else
                          "Value" if contract["native_return_status"] == 1 else "Continue")
        if row["reply"] != expected_reply:
            raise ValueError(f"Stale reply type for native{index}")
        if kind == "single":
            code_pattern = r":\s*(\d+)" if parts[4].startswith("BooleanArg1") else r"\d+"
            codes = [int(c) for c in re.findall(code_pattern, parts[4])]
            if codes != contract["commands"]:
                raise ValueError(f"Stale command binding: native{index}")
            row.update(packing=parts[5], operation=parts[6])
        elif kind == "batch":
            prefix = [int(c) for c in re.findall(r"\d+", parts[3])]
            codes = prefix + [int(parts[2])]
            if codes != contract["commands"]:
                raise ValueError(f"Stale batch command binding: native{index}")
            row.update(value_kind=parts[4], diagnostic=json.loads(parts[5]))
        else:
            row["diagnostic"] = json.loads(parts[4])
        rows.append(row)
    if [r["index"] for r in rows] != sorted(contracts):
        raise ValueError("Descriptor identities are missing, duplicated or unsorted")
    return rows


def main():
    evidence = read_json(ROOT / "reports/recovery_A_20261116_emote.json")
    rows = descriptor_rows(evidence)
    image = Image(ROOT / evidence["binary_path"], evidence["PE_sha256"])
    # This is a bounded snippet, even when it happens to end at a ret.
    tint = block(image, 0x239640, 0xC7)
    constructor = block(image, 0x237CF0, 0x180)
    shape_counts = Counter(r["shape"] for r in rows)
    summary = {
        "identified_complex_native_slots": len(rows),
        "ordinary_static_callsites": sum(r["static_callsites"] for r in rows),
        "single_wrapper_descriptors": shape_counts["single"],
        "batch_wrapper_descriptors": shape_counts["batch"],
        "blocked_wrapper_descriptors": shape_counts["blocked"],
        "wrapper_planning_ready": sum(r["wrapper_ready"] for r in rows),
        "source_backend_operations_available": [906, 919],
        "source_backend_operations_with_explicit_owner_binding": [907],
        "native907_binding": "actual reset banks required; see recovery_A_20261117_emote_host_summary.json",
        "source_backend_operation_limit": (
            "906 simple mode0 archive name only;919 exact task tint arithmetic. "
            "Neither establishes complete model rendering or integrated native completion."
        ),
        "controller_zero_state_only_success_removed": [927, 928],
        "integrated_completed_native_count": 0,
        "full_NEKO_rendering_compatibility_claim": False,
        "Rust_compiled": False,
        "Rust_tests_run": False,
    }
    apis = {
        "descriptor_table": "cs2_runtime::emote::EMOTE_NATIVE_DESCRIPTORS",
        "descriptor_lookup": "emote_native_descriptor(index)",
        "group_detection": "EmoteNativeServices::recognizes(index)",
        "single_receiver": (
            "EmoteNativeServices::receive(index,args,vm,"
            "resolved_target:Option<u32>,pairs:Option<&BTreeMap<Vec<u8>,u32>>)"
        ),
        "backend": "CpuEmoteBackend::new(viewport) + set_resources(provider)",
        "profile": "state.bind_pe_sha256(real_sha256); state.set_backend(Box::new(cpu))",
        "task_lifetime": "state.register_task(actual_id,54); state.release_task(actual_id)",
        "base_task_color": "state.set_base_argb(task,actual_task_argb)",
        "raw_tint": "m2plane_tint_argb(base_argb,[raw_R,raw_G,raw_B])",
        "read_tint": "CpuEmoteBackend::tint_argb(task)",
        "diagnostic_pixels": (
            "CpuEmoteBackend::render_preview(task) -> DiagnosticFrame; "
            "includes task tint and retains every warning"
        ),
        "tick": "state.advance(task,EmoteTick{task_elapsed,increment})",
        "pixels": "EmoteNativeServices::draw(task,&mut Canvas)",
        "legacy_adapters": "call/call_batch retained; both derive from the same descriptors",
    }
    integration = [
        "Export runtime emote and include desktop emote_native in the parent's modules.",
        "Own EmoteNativeServices once in DataServices; do not create one per native call.",
        "Bind exact PE SHA256 c71c5b85...11adc. Other images must use their own native_profile.",
        "Attach CpuEmoteBackend with host-supplied viewport and time_units_per_frame; attach EmoteArchiveResources with resolved archive category.",
        "Register/release actual type54 task IDs with the task tree. Do not reinterpret a numeric script target as a host pointer.",
        "Call recognizes(index) and receive(...) before RequiresImplementation profile rejection.",
        "Single calls need Some(actual_task_id). Batch raw arg1==0 needs neither target nor pairs; a nonzero handle requires an actual borrowed typed table.",
        "Blocked globals/record/descriptor ABI report their own diagnostic; do not count them as implemented.",
        "Advance from the real task clock. Propagate actual base ARGB before draw when task color changes.",
        "Strict draw rejects model warnings before any canvas writes. Diagnostic preview remains an analysis-only API.",
    ]
    limitations = [
        "This batch implements descriptor-driven routing and two restricted backend branches in source; it is not36 completed native implementations.",
        "No host dispatch/export/constructors were changed; main agent owns integration.",
        "Native927/928 zero requests now reject until original reset variable effects184d70/184cd0 are mapped.",
        "Native907 effective-enable scene overrides,924 sound link and remaining controller/timeline semantics are unresolved.",
        "Native1028 retains its observed args+4 read ambiguity and is not guessed.",
        "Default NEKO model has unsupported stencil/composite,skin,3D,layer and physics behavior; strict renderer still rejects it.",
        "Previous Python preview hashes remain historical analysis observations, not execution of the changed Rust renderer.",
        "Tint DWORD arithmetic is recovered exactly; CPU texture rasterization/compositing has not been compared with original output.",
        "Source rasterizer now keeps straight RGBA to avoid applying alpha twice during Canvas submission.",
        "FreeMote is research-only CC-BY-NC-SA4.0; no source was copied, no native engine/SDK/game code was executed.",
    ]
    payload = {
        "schema": "cs2-recovery-A-emote-dispatch/v1",
        "task_date": "2026-11-17",
        "generated_at_utc": datetime.now(timezone.utc).isoformat(),
        "summary": summary,
        "PE_sha256": evidence["PE_sha256"],
        "PE_file_version": evidence["file_version"],
        "task_type": 54,
        "APIs": apis,
        "integration_steps": integration,
        "descriptors": rows,
        "new_original_evidence": {
            "tint_helper": tint,
            "constructor": constructor,
            "tint_contract": {
                "handler_RVA": 0x238CF4,
                "helper_RVA": 0x239640,
                "raw_percentages_offsets": [0x3A4, 0x3A8, 0x3AC],
                "base_ARGB_offset": 0x388,
                "base_ARGB_constructor_default": 0xFFFFFFFF,
                "vertex_color_offsets": [0x2C8, 0x2E4, 0x300, 0x31C],
                "channel_formula": (
                    "scaled=raw.wrapping_mul(255);"
                    "q=((scaled*0x51eb851f)>>32)>>5;"
                    "product=q.wrapping_mul(base_channel);"
                    "out=min((((product*0x80808081)>>32)>>7),255)"
                ),
                "alpha": "base ARGB alpha preserved",
                "clamp_input_percentage": False,
            },
        },
        "source_files": [{"path": path, "sha256": sha((ROOT / path).read_bytes())}
                         for path in SOURCES],
        "related_reports": [
            "reports/recovery_A_20261116_emote.json",
            "reports/recovery_A_20261117_psb_backend.json",
            "reports/kcs_call_inventory_20261116.json",
        ],
        "limitations": limitations,
        "classification": (
            "functional script in scripts/analysis; durable reports in reports; "
            "no transient output or game assets created by this batch"
        ),
    }
    prefix = ROOT / "reports/recovery_A_20261117_emote_dispatch"
    prefix.with_suffix(".json").write_text(
        json.dumps(payload, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )
    # Small enough to load in workload snapshots without any instruction listing.
    prefix.with_name(prefix.name + "_summary").with_suffix(".json").write_text(
        json.dumps({"summary": summary, "APIs": apis}, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )
    lines = [
        "# A: Emote descriptor receiver batch", "",
        "Task date2026-11-17. No compilation/tests or original-code execution.", "",
        "## Ready for parent integration", "",
        f"- One sorted table covers{len(rows)} complex ordinary slots / "
        f"{summary['ordinary_static_callsites']} static callsites.",
        f"- {shape_counts['single']} single wrappers +{shape_counts['batch']} ordered-table wrappers "
        f"share one receiver;{shape_counts['blocked']} retain explicit ABI/semantic diagnostics.",
        "- Backend source accepts906 simple mode0 load and919 real color modulation. "
        "This is not a claim of completed native integration or complete NEKO frames.",
        "- 927/928 zero-value state-only acceptance removed: real reset effects remain missing.", "",
        "## APIs", "", "| Role | API |", "|---|---|",
    ]
    lines += [f"| {role} | `{api}` |" for role, api in apis.items()]
    lines += ["", "## Parent wiring", ""]
    lines += [f"{i}. {text}" for i, text in enumerate(integration, 1)]
    lines += [
        "", "## New color evidence", "",
        "919 wrapperRVA0x24d670 ->command536 ->receiver0x238cf4 ->helper0x239640.",
        "Raw R/G/B DWORDs go to+3a4/+3a8/+3ac. Constructor0x237d90 sets base ARGB+388 "
        "toffffffff. Helper performs wrapping255*percentage, unsigned /100, "
        "wrapping multiply by each base channel, unsigned /255, then min255. "
        "It preserves base alpha and writes four vertex colors+2c8/+2e4/+300/+31c.",
        "This raw computation is now a public pure function and the backend retains it "
        "across resource replacement. Actual task ARGB can be propagated independently.",
        "", "## Descriptor inventory", "",
        "| Native | RVA | Args bytes | Shape | Static calls |",
        "|---:|---|---:|---|---:|",
    ]
    lines += [f"| {r['index']} | 0x{r['wrapper_rva']:x} | {r['argument_bytes']} | "
              f"{r['shape']} | {r['static_callsites']} |" for r in rows]
    lines += ["", "## Limits", ""]
    lines += ["- " + limitation for limitation in limitations]
    lines += ["", "## Changed paths / classification", ""]
    lines += ["- " + path for path in SOURCES]
    lines += [
        "- reports/recovery_A_20261117_emote_dispatch.json/md",
        "- reports/recovery_A_20261117_emote_dispatch_summary.json",
        "", "No new directory, README/localization change or transient artifact. "
        "Existing diagnostic assets remain archive/2026-11/20261117_*.",
    ]
    prefix.with_suffix(".md").write_text("\n".join(lines) + "\n", encoding="utf-8")
    previous = ROOT / "reports/recovery_A_20261117_psb_backend.md"
    if previous.is_file():
        text = previous.read_text(encoding="utf-8").split(
            "\n## Descriptor receiver follow-up"
        )[0]
        text += (
            "\n## Descriptor receiver follow-up\n\n"
            "See reports/recovery_A_20261117_emote_dispatch.md/json for the unified36-slot "
            "descriptor table and receive(...) API. The backend now implements exact919 "
            "task tint arithmetic and retains simple906 load. Previous927/928 zero-only "
            "state assignment was removed because the real controller reset effects are "
            "not recovered. No native integration/full-NEKO completion is claimed. "
            "Previous Python pixels/hashes are historical observations, not execution of "
            "the changed Rust straight-RGBA renderer.\n"
        )
        previous.write_text(text, encoding="utf-8")
    print(json.dumps(summary, ensure_ascii=True, indent=2))


if __name__ == "__main__":
    main()

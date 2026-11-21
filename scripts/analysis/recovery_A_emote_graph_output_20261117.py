"""Bounded driver/PSB graph-output evidence; no target or renderer execution."""
import json

from recovery_A_20261116 import ROOT, Image, read_json, sha
from recovery_A_psb_20261117 import Psb, load_sample
from recovery_A_emote_model_render_20261117 import DRIVER_SHA
from recovery_A_receiver_contracts_20261116 import block


def main():
    identity = read_json(ROOT / "reports/recovery_A_20261116_emote.json")
    driver = Image((ROOT / identity["binary_path"]).parent / "emotedriver.dll", DRIVER_SHA)
    name, data = load_sample(0)
    metadata = Psb(data).root["metadata"]
    controls = {section: [entry for entry in metadata[section] if entry.get("enabled") != 0]
                for section in ("eyeControl", "eyebrowControl")}
    spans = {"eye_signed_begin_to_f32": (0x6440, 0x34),
             "eyebrow_signed_begin_to_f32": (0x8f37, 0x23),
             "eye_graph_and_disabled_blink_output": (0x6da5, 0x65),
             "eyebrow_graph_output": (0x959e, 0x11),
             "eye_update_and_variable_bank_lookup": (0x18610, 0x28),
             "eye_output_committed_to_variable_bank": (0x186f7, 0x1b),
             "eyebrow_update_and_variable_bank_lookup": (0x19440, 0x28),
             "base_and_offsets_then_model_variable": (0x13658, 0x4c)}
    summary = {"batch": "graph controller constructor -> live target -> model parameter -> texture pixels",
               "resource": name, "resource_sha256": sha(data),
               "enabled_eye": len(controls["eyeControl"]),
               "enabled_eyebrow": len(controls["eyebrowControl"]),
               "automatic_blink_disabled": all(c["blinkEnabled"] == 0 for c in controls["eyeControl"]),
               "constructor_default_in_source": True,
               "timed_graph_receiver": "existing CpuM2TimedVariables; primary pair paths not reimplemented",
               "output_receiver": "CpuPlayer::controller_variables -> additive offsets -> clamp -> parameter_cursor -> BP/atlas CPU raster",
               "strict_refusal_scope": ["automatic blink", "degenerate/reversed eye interval", "mouth talk composition", "other enabled controller families"],
               "newly_closed_entire_families": 0, "completed_native_equivalence_added": 0,
               "historical_partial_family_count": 9,
               "full_NEKO_strict_pixel_claim": False, "compiled": False,
               "tests_run": False, "target_execution": False, "syntax": "rustfmt parser only"}
    sources = ["simulator/crates/runtime/src/emote_m2selector.rs",
               "simulator/crates/runtime/src/emote_cpu.rs",
               "scripts/analysis/recovery_A_emote_graph_output_20261117.py"]
    payload = {"summary": summary,
               "perPE": [{"path": str(driver.path.relative_to(ROOT)), "sha256": driver.digest,
                          "addresses": "RVA", "spans": {k: [a, n] for k, (a, n) in spans.items()}}],
               "controls": controls, "evidence": {k: block(driver, *s) for k, s in spans.items()},
               "sources": [{"path": p, "sha256": sha((ROOT / p).read_bytes())} for p in sources]}
    prefix = ROOT / "reports/recovery_A_20261117_emote_graph_output"
    for suffix, value in ((".json", payload), ("_summary.json", {k: payload[k] for k in ("summary", "perPE")})):
        (prefix.parent / (prefix.name + suffix)).write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    prefix.with_suffix(".md").write_text(
        "# A / 眼眉输出链\n\n"
        "emote_m2selector.rs补62e0/8e00的signed beginFrame→f32默认生产，已有实时写入优先；"
        "emote_cpu.rs放行已实现的眼眉图队列输出，自动blink和退化interval继续明确拒绝。"
        "实际首模型1眼/5眉，blinkEnabled=0；从既有timed graph到变量bank、timeline差分、"
        "clamp、parameter_cursor、真实atlas/mesh纹理像素的源码链相连。"
        "证据driver SHA85eb459e…93ce，6440/8f37构造，6da5/959e输出，"
        "18621/19451更新，1870d写bank，13699送模型变量。\n\n"
        "mouth talk合成、transition及其余未恢复控制器仍诊断；未新增整族/native等价计数，"
        "未声称完整NEKO像素。未编译、测试、执行；仅静态读取和rustfmt语法读取。"
        "脚本归scripts/analysis、报告归reports，无临时资产/新目录/README修改。\n", encoding="utf8")

    # Preserve old batches as historical snapshots; current status lives here.
    current_path = ROOT / "reports/recovery_A_20261117_emote_current_summary.json"
    current = read_json(current_path)
    for batch in ("model_render", "composite_ranges", "clamp", "graph_output"):
        small = read_json(ROOT / f"reports/recovery_A_20261117_emote_{batch}_summary.json")
        current[batch + "_batch"] = small
        report = f"reports/recovery_A_20261117_emote_{batch}.md"
        if report not in current["reports"]:
            current["reports"].append(report)
    current["summary"]["remaining_strict_blockers"][0] = (
        "main connected same-resource primary/931 surface and successful cross-PSB replacement; nonzero secondary profile and failed reopen side effects remain")
    current["summary"]["remaining_strict_blockers"][-1] = (
        "local/caller mask schedule and captured source ranges implemented; kind3 reference itself owning mask, active own BP, textured type12 targets and mode1 remain diagnostic")
    current["summary"]["backend_native_branches"]["906"] = (
        "primary timed graph/scalar/selector and shared bank implemented; main connected surface/successful cross-PSB replacement; nonzero secondary remains")
    current["summary"]["history_notice"] = "embedded earlier batch statements are dated snapshots, not current missing-source counts"
    current["summary"]["main_material_force_owner"] = "main owns emote_m2physics/force/live and CPU update hooks; latest source status is reports/emote_force_20261117.md"
    current["summary"]["latest_A_batch"] = summary
    for path in [*sources, "simulator/crates/runtime/src/emote_clamp.rs", "simulator/crates/runtime/src/emote.rs"]:
        current["source_hashes_current"] = [r for r in current["source_hashes_current"] if r["path"] != path]
        current["source_hashes_current"].append({"path": path, "sha256": sha((ROOT / path).read_bytes())})
    current_path.write_text(json.dumps(current, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    print(json.dumps(summary))


if __name__ == "__main__":
    main()

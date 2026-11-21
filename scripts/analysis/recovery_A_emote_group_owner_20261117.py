"""Bounded original group-owner evidence; only PE/PSB bytes are read."""
import json

from recovery_A_20261116 import ROOT, Image, read_json, sha
from recovery_A_emote_model_render_20261117 import DRIVER_SHA, inventory
from recovery_A_psb_20261117 import Psb, load_sample
from recovery_A_receiver_contracts_20261116 import block


def main():
    identity = read_json(ROOT / "reports/recovery_A_20261116_emote.json")
    driver = Image((ROOT / identity["binary_path"]).parent / "emotedriver.dll", DRIVER_SHA)
    name, data = load_sample(0)
    observed = inventory(Psb(data).root)
    cc_probe = read_json(ROOT / "archive/2026-11/20261117_emote_cc_probe.json")
    spans = {
        "reference_root_copies_ancestor_tail_not_current_owner": (0x41b09, 0x15),
        "second_reference_entry_copies_same_tail": (0x43cc9, 0x15),
        "owner_group_captures_referenced_primitives": (0x44ace, 0xa3),
        "composite_owner_own_primitive_first": (0x44f30, 0xa0),
        "composite_group_source_flattens_captured_range": (0x45000, 0x42),
        "color_pass_skips_nontextured_primitive": (0x4990, 0x10),
        "composite_mask_skips_nontextured_primitive": (0x50d0, 0xf),
        "noncomposite_mask_skips_own_group": (0x540b, 0x9),
        "cc_curve_remaps_BP_interpolation_ratio": (0x2be30, 0x3d),
        "hidden_frame_type0_parser_flag": (0x3709a, 0x29),
        "hidden_frame_type0_sets_frame10": (0x371a8, 0x4),
        "sample_cache_base_b0_stride_d4": (0x3d692, 0x2a),
        "stencil_activation_tests_sample_hidden_not_mesh2c0": (0x402e2, 0x27),
    }
    summary = {
        "batch": "kind3 stencil owner -> ancestor tail/captured source -> real alpha/EQUAL pixels",
        "receivers": ["RenderContext::motion", "resolve_emote_stencil_bindings", "literal_stencil_clip -> sprite -> CPU triangles"],
        "kind3_own_blanket_refusal_removed": True,
        "reference_root_inherits_ancestor_tail_not_current_owner": True,
        "physical_children_still_link_current_owner": True,
        "noncomposite_group_has_no_own_mask_geometry": True,
        "empty_positive_mask_keeps_rejecting_EQUAL": True,
        "explicit_composite_group_sources_keep_captured_range": True,
        "composite_sprite_owner_own_primitive_precedes_explicit_sources": True,
        "visible_own_BP_stencil_owner_no_longer_wrongly_refused": True,
        "correction": "402e2 tests frame.hidden at b0+10=c0, not mesh activity layer2c0",
        "first_model": {"resource": name, "sha256": sha(data),
                        "kind3_owner_count": sum(v["kind"] == 3 for v in observed["stencil_owners"]),
                        "composite_owner_count": len(observed["stencil_owners"])},
        "cc_static_inventory": cc_probe["summary"],
        "cc_meaning": "BP interpolation curve remap, not established skin weights; nonempty branch remains diagnostic",
        "remaining": ["direct composite source kind3 also owning a composite mask: rebuilt shared range", "textured type12 target/reversal", "mode1 targets", "non32 BP", "meshSyncChildMask bits other than8", "nonempty cc remap"],
        "historical_partial_families": 9,
        "completed_native_equivalence_added": 0,
        "full_NEKO_strict_pixel_claim": False,
        "compiled": False, "tests_run": False, "target_execution": False,
        "syntax": "rustfmt parsing only, no typecheck",
    }
    paths = ["simulator/crates/runtime/src/emote_cpu.rs", "simulator/crates/runtime/src/emote_stencil.rs",
             "simulator/crates/runtime/src/emote_timeline.rs", "scripts/analysis/recovery_A_emote_group_owner_20261117.py"]
    payload = {"summary": summary,
               "perPE": [{"path": driver.path.relative_to(ROOT).as_posix(), "sha256": driver.digest,
                          "addresses": "RVA", "spans": {k: list(v) for k, v in spans.items()}}],
               "evidence": {k: block(driver, *span) for k, span in spans.items()},
               "sources": [{"path": p, "sha256": sha((ROOT / p).read_bytes())} for p in paths]}
    prefix = ROOT / "reports/recovery_A_20261117_emote_group_owner"
    small = {k: payload[k] for k in ("summary", "perPE")}
    for suffix, value in ((".json", payload), ("_summary.json", small)):
        prefix.with_name(prefix.name + suffix).write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    prefix.with_suffix(".md").write_text(
        "# A / kind3 stencil owner小批\n\n"
        "源码仅emote_cpu.rs的motion/cc诊断、emote_stencil.rs绑定及emote_timeline.rs诊断文字。"
        "41b12/43cd2明确复制caller的2d4祖先tail；kind3引用内容保持该链，"
        "当前owner只传给物理children。移除kind3自身owner的无条件拒绝。"
        "540b跳过group自身几何，故noncomposite group binding保留空mask和真实EQUAL，"
        "不拿其引用内容代替own mask。显式composite source仍走44b39/4501b捕获范围，"
        "真实纹理/mesh alpha→INCR/DECR→颜色EQUAL。另44f56确认sprite composite owner先画own primitive。\n\n"
        "更正旧own-BP阻断：371a8将type0写frame+10；3d692..3d6bc的sample缓存位于layer+b0+index*d4。"
        "故402e2的c0是hidden，绝非layer+2c0的mesh活动。删除错误的BP owner警告，"
        "visible owner继续以既有sample/BP组合/真实纹理图元写mask；hidden仍走已有隐藏分支。\n\n"
        f"首模型{name}六个owner均非kind3；因此不以本批宣称该模型新增可见像素。"
        "已归档全135 PSZ扫描非空mesh.cc=0；2be4d..2be67证明cc重映射BP插值ratio，"
        "历史cc/skin合称不准确，已修正诊断，未猜造skin。\n\n"
        "剩余：kind3 composite owner同时作为另一个owner直接源的重建共享range、textured type12、mode1、非32BP、其他meshSync位及非空cc。"
        "不改main import/advance/material settings/force/banks/surface。整函数等价新增0，"
        "不宣称完整NEKO像素。只做静态PE/PSB及rustfmt语法读取，无编译/测试/运行。"
        "脚本归scripts/analysis，报告归reports；cc临时证据归archive/2026-11/20261117_emote_cc_probe.json。\n",
        encoding="utf8")
    current_path = ROOT / "reports/recovery_A_20261117_emote_current_summary.json"
    current = read_json(current_path)
    current["group_owner_batch"] = small
    current["summary"]["latest_A_batch"] = summary
    blockers = current["summary"]["remaining_strict_blockers"]
    current["summary"]["remaining_strict_blockers"] = [
        ("local/caller chain, kind3 owner ancestor-tail transfer and captured composite sources are connected; own-BP false refusal corrected to real frame.hidden gate; kind3 composite owner used as another owner's direct source, textured type12 and mode1 remain diagnostic"
         if "kind3 reference itself" in v or "active own BP" in v or "local/caller chain" in v else v.replace("cc/skin", "nonempty cc BP-curve remap (absent in135 actual PSZ)"))
        for v in blockers]
    for row in payload["sources"]:
        current["source_hashes_current"] = [v for v in current.get("source_hashes_current", []) if v["path"] != row["path"]]
        current["source_hashes_current"].append(row)
    report = prefix.with_suffix(".md").relative_to(ROOT).as_posix()
    if report not in current.setdefault("reports", []):
        current["reports"].append(report)
    current_path.write_text(json.dumps(current, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    print(json.dumps(summary, ensure_ascii=True))


if __name__ == "__main__":
    main()

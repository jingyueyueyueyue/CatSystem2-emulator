"""Bounded five-model draw/evaluator predicates, never runs a renderer.

Use retained resource identities and config/AST evidence. The full ASTs were
not retained, so read only those five fixed members; no PE or game-wide scan.
"""
import argparse
from collections import Counter
import json

from recovery_A_20261116 import ROOT, read_json, sha
from recovery_A_emote_model_render_20261117 import flatten
from recovery_A_psb_20261117 import Psb, load_sample

OUT = ROOT / "archive/2026-11/20261117_emote_actual_draw_scope.json"
PREFIX = "recovery_A_20261117_emote_actual_draw"

# Resource-declaration predicates only. Do not present a zero in this list as
# execution of render_strict, or as coverage of every possible renderer error.
CHECKED = [
    "layer_type", "reference_inherit", "coordinate", "mesh_sync_bits",
    "reference_stencil_owner", "composite_source_owns_mask",
    "frame_type", "frame_order",
    *["timeline_curve_" + key for key in ("ccc", "acc", "zcc", "scc", "occ", "cp")],
    "mesh_cc", "mesh_bp_shape", "mesh_transform", "sprite_blend",
    "sprite_color", "textured_type12", "unresolved_reference", "reference_loop",
    "reference_flags_offset", "timeline_duplicate_track", "timeline_terminal",
    "timeline_frame_type",
]


def inspect(root):
    hits, counts, examples = Counter(), Counter(), {}

    def hit(key, path, value=None):
        hits[key] += 1
        examples.setdefault(key, {"path": path, "value": value})

    def frames(owner, path):
        rows = owner.get("frameList", [])
        previous = -float("inf")
        for index, frame in enumerate(rows):
            at = path + ["frameList", index]
            counts["frame_records"] += 1
            if frame.get("type", 2) not in (0, 2, 3):
                hit("frame_type", at, frame.get("type"))
            if frame.get("time", 0) < previous:
                hit("frame_order", at)
            previous = frame.get("time", 0)
            content = frame.get("content")
            if not isinstance(content, dict):
                continue
            for key in ("ccc", "acc", "zcc", "scc", "occ", "cp"):
                if content.get(key) is not None:
                    hit("timeline_curve_" + key, at, content[key])
            mesh = content.get("mesh") or {}
            if mesh.get("cc"):
                hit("mesh_cc", at)
            points = mesh.get("bp") or []
            if points and len(points) != 32:
                hit("mesh_bp_shape", at, len(points))
            if points and owner.get("meshTransform", 0) != 1:
                hit("mesh_transform", at, owner.get("meshTransform", 0))
            if owner.get("type", 2) != 3:
                if content.get("bm", 0):
                    hit("sprite_blend", at, content["bm"])
                if content.get("color", 0x808080ff) != 0x808080ff:
                    hit("sprite_color", at, content["color"])
                if owner.get("type", 2) == 12 and content.get("src") not in (None, "blank"):
                    hit("textured_type12", at, content.get("src"))
            else:
                source, icon = content.get("src"), content.get("icon")
                if source is None or icon is None:
                    continue
                target = root["object"].get(source, {}).get("motion", {}).get(icon)
                counts["reference_records"] += 1
                if target is None:
                    hit("unresolved_reference", at, [source, icon])
                elif target.get("loopTime", -1) >= 0:
                    hit("reference_loop", at, target.get("loopTime"))
                settings = content.get("motion") or {}
                if settings.get("mask", 0) or settings.get("timeOffset", 0):
                    hit("reference_flags_offset", at, settings)

    for obj_name, obj in root["object"].items():
        for motion_name, motion in obj["motion"].items():
            layers = flatten(motion.get("layer", []))
            counts["motions"] += 1
            counts["layers"] += len(layers)
            for index, layer in enumerate(layers):
                path = ["object", obj_name, "motion", motion_name, "layer_flat", index]
                kind = layer.get("type", 2)
                if kind not in (0, 1, 2, 3, 12) or kind == 1 and layer.get("children"):
                    hit("layer_type", path, kind)
                if kind == 3 and layer.get("motionIndependentLayerInherit", 0):
                    hit("reference_inherit", path)
                if layer.get("coordinate", 0):
                    hit("coordinate", path, layer["coordinate"])
                if layer.get("meshSyncChildMask", 0) & ~8:
                    hit("mesh_sync_bits", path, layer["meshSyncChildMask"])
                if layer.get("stencilType", 0) and kind == 3:
                    hit("reference_stencil_owner", path)
                frames(layer, path)
            indexes = motion.get("layerIndexMap", {})
            for layer in layers:
                for label in layer.get("stencilCompositeMaskLayerList", []):
                    index = indexes.get(label)
                    if isinstance(index, int) and 0 <= index < len(layers):
                        source = layers[index]
                        if source.get("type", 2) == 3 and source.get("stencilType", 0):
                            hit("composite_source_owns_mask", [obj_name, motion_name, label])
    # Only already implemented evaluator families, not another zero-hit
    # metadata-family inventory. These validate resource declarations only.
    metadata = root.get("metadata", {})
    for timeline in metadata.get("timelineControl", []):
        names = set()
        for variable in timeline.get("variableList", []):
            counts["timeline_tracks"] += 1
            label = variable.get("label")
            path = ["metadata", "timelineControl", timeline.get("label"), label]
            if label in names:
                hit("timeline_duplicate_track", path)
            names.add(label)
            keys = variable.get("frameList", [])
            if keys and keys[-1].get("type", 0) != 0:
                hit("timeline_terminal", path)
            if any(frame.get("type", 0) not in (0, 2) for frame in keys):
                hit("timeline_frame_type", path)
    return {"counts": dict(counts), "guard_hits": dict(hits), "examples": examples}


def probe():
    profile = read_json(ROOT / "reports/recovery_A_20261117_emote_profile.json")
    fixed = profile["actual_PSZ_identities"]
    if len(fixed) != 5:
        raise ValueError("bounded input must stay five fixed shared PSZs")
    rows, total = [], Counter()
    for index, previous in enumerate(fixed):
        name, raw = load_sample(index)
        if name != previous["name"] or sha(raw) != previous["decoded_sha256"]:
            raise ValueError("fixed shared resource identity changed")
        row = {"resource": name, "decoded_sha256": sha(raw), **inspect(Psb(raw).root)}
        rows.append(row)
        total.update(row["guard_hits"])
    banks = read_json(ROOT / "archive/2026-11/20261117_emote_bank_inputs.json")
    data = {"summary": {"fixed_model_count": len(rows), "guard_hits": dict(total),
            "static_predicates_only": True, "no_renderer_or_evaluator_execution": True,
            "no_PE_read": True, "no135_resource_scan": True,
            "full_AST_not_retained_previously": True,
            "fixed_five_members_reparsed_only": True},
        "resources": rows,
        "profile_inputs": [{"resource": r["resource"], "sha256": r["sha256"]}
            for r in banks["resources"]],
        "mapping_scope": "five exact ca01/ca01L shared resources;14 cached include identities are not assumed to map every PSZ",
        "source": "simulator/crates/runtime/src/emote_cpu.rs plus timeline/variable_timeline guard predicates",
        "limitations": ["field presence is not reachability or runtime pixel validation",
            "not all error paths are resource-declaration predicates",
            "capacity, dynamic flags, graph nonfinite values and live lifecycle remain outside this static check"]}
    OUT.write_text(json.dumps(data, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    print(json.dumps(data["summary"]))
    for row in rows:
        print(json.dumps({"resource": row["resource"], "hits": row["guard_hits"],
            "examples": row["examples"]}, ensure_ascii=True))


def write_report():
    """Read retained evidence only; never repeat member parsing in report mode."""
    data = read_json(OUT)
    inventory_path = ROOT / "reports/kcs_call_inventory_20261116.json"
    audit_path = ROOT / "reports/all_sample_content_audit.json"
    calls = next(game for game in read_json(inventory_path)["games"]
                 if "yukikoi" in game["root"].lower())
    audit = next(game for game in read_json(audit_path)["games"]
                 if "yukikoi" in game["root"].lower())
    efk_calls = [{"resource": program["resource"], "sha256": program["sha256"],
                  "calls": [call for call in program["calls"]
                            if call["index"] in (1014, 1015, 1016)]}
                 for program in calls["programs"]
                 if any(call["index"] in (1014, 1015, 1016) for call in program["calls"])]
    context_path = ROOT / "archive/2026-11/20261117_kcs_a_efk_wrapper_scope.json"
    context = read_json(context_path)
    load = next(row for row in context["instructions"]
                if row["opcode"] == 0x2b and row["operands"][0]["value"] == 1014)
    before = [row for row in context["instructions"] if row["offset"] < load["offset"]][-3:]
    extensions = audit["archives"]["entries_by_extension"]
    efk = {
        "indexed_archives": audit["archives"]["indexed"],
        "indexed_entries": audit["archives"]["indexed_entries"],
        "archive_index_diagnostics": audit["diagnostics"],
        "named_EFK_family_members": {
            extension: extensions.get(extension, 0)
            for extension in ("efk", "efkefc", "efkmodel", "efkmat")
        },
        "cached_programs": calls["program_count"],
        "native_calls": efk_calls,
        "load_argument_context": before + [load],
        "load_operand_is_concrete_asset_name": False,
        "fixed_wrapper_string_count": len(context["strings"]),
        "EFK_asset_literals_in_fixed_wrapper": [row for row in context["strings"]
            if any(extension in row["cp932"].lower()
                   for extension in (".efk", ".efkefc", ".efkmodel", ".efkmat"))],
        "actual_effect_member_identified": False,
        "scope": "cached archive extension totals and88 KCS call identities, plus one fixed wrapper instruction/string context; no payload-wide signature search or new game-wide inventory",
        "limitation": "no named effect member or literal is not proof of no hidden/renamed/external effect, nor that the wrapper is unreachable; indirect or scene-driven callers are unresolved",
    }
    totals = Counter()
    for row in data["resources"]:
        totals.update(row["counts"])
    metadata_path = ROOT / "reports/recovery_A_20261117_emote_metadata_scope_summary.json"
    metadata = read_json(metadata_path)["summary"]
    summary = {
        **data["summary"],
        "batch": "five fixed PSZ draw declarations and bounded Yukikoi EFK producer check",
        "counts": dict(totals),
        "profile_input_identities_reused": len(data["profile_inputs"]),
        "declared_guard_predicates_checked": len(CHECKED),
        "guard_statement_hits": sum(data["summary"]["guard_hits"].values()),
        "prior_metadata_guard_hits_reused": metadata["observed_metadata_strict_guard_hits"],
        "actual_EFK_member_identified": efk["actual_effect_member_identified"],
        "EFK_native_call_sites": sum(len(row["calls"]) for row in efk_calls),
        "new_runtime_consumers": 0,
        "guards_removed": 0,
        "completed_native_equivalence_added": 0,
        "full_NEKO_strict_pixel_claim": False,
        "all_draw_or_evaluator_errors_proven_absent": False,
        "compiled": False, "typechecked": False, "tests_run": False,
        "Rust_PE_VM_game_APK_device_execution": False,
        "new_runtime_API": None,
        "next_required_input": "a concrete still-rejected PSZ model path/state, or actual Yukikoi EFK member/decoded bytes with resource identity; do not implement zero-hit families",
    }
    per_pe = [{
        "sha256": "85eb459e94e74ab608f649eb7fb440f2526e90aaa1af7276ed01dc10018903ce",
        "role": "NEKO driver identity retained from prior evidence; not read this batch",
        "new_PE_scan": False,
    }, {
        "sha256": "63de0e14cd8ce11ad2d68e7ed6076641ff5e5b73d822603cc52764009d812976",
        "role": "Yukikoi type61 identity retained from prior evidence; not read this batch",
        "native_calls": [1014, 1015, 1016], "receiver_rva": "18c1f0",
        "new_PE_scan": False,
    }]
    unproven = [
        "texture/icon rectangles, parameter and priority indices, affine singularities, capacity and numeric errors were not exhaustively evaluated by these declaration predicates",
        "live owner/tick/forced-seek state, active controller targets and dynamic composition can still reject; no evaluator or pixel algorithm was executed",
        "five ca01/ca01L shared resources do not stand for all135 PSZs or every14profile state",
        "EFK resource/layout/RNG/geometry family cannot be selected from a generic native call alone; previous11 partial implementation families are unchanged",
    ]
    evidence_paths = [OUT, inventory_path, audit_path, context_path, metadata_path]
    payload = {
        "schema": "cs2-recovery-A-actual-draw/v1", "summary": summary,
        "perPE": per_pe, "resources": data["resources"],
        "profile_inputs": data["profile_inputs"], "mapping_scope": data["mapping_scope"],
        "checked_declaration_predicates": CHECKED,
        "Yukikoi_EFK_producer": efk,
        "unproven_or_remaining": unproven,
        "evidence": [{"path": path.relative_to(ROOT).as_posix(),
                      "sha256": sha(path.read_bytes())} for path in evidence_paths],
    }
    for suffix, value in ((".json", payload),
                          ("_summary.json", {"summary": summary, "perPE": per_pe})):
        (ROOT / "reports" / (PREFIX + suffix)).write_text(
            json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    lines = ["# A：固定模型绘制与EFK实际生产入口", "",
        "本批新增runtime消费者0，删除guard0，完整函数等价新增0；没有编译、类型检查、测试或执行Rust/PE/VM/游戏。", "",
        f"5份固定ca01_l_body_1..5.psz共240个motion、2351层、10245条frame、225条引用记录及19065条timeline变量轨。{len(CHECKED)}项声明谓词未命中拒绝。仅身份及裁剪AST已有留存，完整AST未留存，因此本批仅补读这5个固定成员；没有扩135资源或PE扫描。14份include身份沿缓存，不将其当成已核对所有角色模型。", "",
        "已核对种类：层类型/坐标/独立引用，mesh bp形状/cc/同步位/transform，帧类型/顺序/非线性曲线，非中性色及blend，textured type12，引用目标/loop/flags/offset，kind3 stencil直接源，timeline重复轨/终止帧/帧类型。此前metadata启用类零命中仅引用旧摘要，没有重扫。", "",
        "Yukikoi旧缓存完整索引24包22776成员，EFK/EFKEFC/EFKMODEL/EFKMAT扩展名均0；88个KCS中的1014、1015、1016各只有sscript.kcs内一处（偏移844116、835511、844218）。固定sscript SHA797acd759f64c280fbb39725fdc413b055d56343e821d16036cc7e38516b4d95。1014前844099取operand flags0xa3/value8，844110 PUSH flags0xe1/value0，未解出具体资源名。1922个初始化串无EFK文件名字面量，仅有efkplane调试格式。故尚无实际效果成员能支持选择一个新的SKFE拒绝族。不能据此断言该封装不可达或不存在改名/外部资源。", "",
        "没有真实命中时不扩写零命中渲染，不删除必要诊断。既有Effekseer11部分族保持；本批没有新API或主线接点。", "",
        "## 明确保留范围", ""]
    lines += ["- " + item for item in unproven]
    lines += ["", "## 分类与归档", "",
        "脚本scripts/analysis/recovery_A_emote_actual_draw_20261117.py；正式JSON、summary及本报告在reports；固定5份结果和sscript定点上下文在archive/2026-11/20261117_*。--report仅读缓存，不重扫资源。未改runtime、desktop、README、译文或游戏资源。"]
    (ROOT / "reports" / (PREFIX + ".md")).write_text("\n".join(lines) + "\n", encoding="utf8")
    print(json.dumps(summary))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--probe", action="store_true", help="Read only the five fixed members")
    parser.add_argument("--report", action="store_true", help="Write reports from retained evidence only")
    args = parser.parse_args()
    if args.probe:
        probe()
    if args.report or not args.probe:
        write_report()


if __name__ == "__main__":
    main()

"""Static, single-model composite range graph; never samples or renders a frame."""
from __future__ import annotations

import argparse
from collections import deque
import json
from pathlib import Path
import subprocess

from recovery_A_20261116 import ROOT, read_json, sha
from recovery_A_psb_20261117 import Psb, load_sample
from recovery_A_emote_model_render_20261117 import flatten


CACHE = ROOT / "archive/2026-11/20261117_emote_shared_ranges.json"
PREFIX = ROOT / "reports/recovery_A_20261117_emote_shared_ranges"
SYNTAX = ROOT / "archive/2026-11/20261117_emote_shared_ranges_syntax.json"


def probe():
    previous = read_json(ROOT / "reports/recovery_A_20261117_emote_composite_ranges.json")
    name, raw = load_sample(0)
    if sha(raw) != previous["summary"]["resource_sha256"]:
        raise ValueError("fixed first model identity changed")
    root = Psb(raw).root
    motions = {(obj, label): value for obj, value in root["object"].items()
               for label, value in value["motion"].items()}
    records = []
    for owner in previous["actual_owners"]:
        motion = motions[owner["object"], owner["motion"]]
        layers = flatten(motion["layer"])
        entry = dict(owner)
        entry["owner_layer"] = {k: v for k, v in layers[owner["index"]].items()
                                if k != "children"}
        entry["source_layers"] = [dict(index=s["index"], layer={k: v for k, v in layers[s["index"]].items()
                                                                 if k != "children"})
                                  for s in owner["sources"] if s["index"] is not None]
        records.append(entry)
    # Preserve exact structural input once. Subsequent reporting reads this
    # bounded one-model cache; it neither reopens archives nor runs an evaluator.
    payload = {"resource": name, "sha256": sha(raw), "owners": records,
               "motions": [{"object": obj, "motion": label, "value": value}
                           for (obj, label), value in motions.items()]}
    CACHE.write_text(json.dumps(payload, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"resource": name, "owners": len(records), "cache": str(CACHE.relative_to(ROOT))}))


def report():
    cached = read_json(CACHE)
    previous = read_json(ROOT / "reports/recovery_A_20261117_emote_composite_ranges.json")
    motions = {(m["object"], m["motion"]): m["value"] for m in cached["motions"]}
    owners = cached["owners"]
    owner_ids = {(o["object"], o["motion"], o["index"]): o for o in owners}

    def refs(key):
        result = []
        for index, layer in enumerate(flatten(motions[key]["layer"])):
            if layer.get("type") != 3:
                continue
            targets = {(f["content"].get("src"), f["content"].get("icon"))
                       for f in layer.get("frameList", []) if f.get("content")}
            for target in sorted(targets):
                if target in motions:
                    result.append((index, target))
        return result

    def nested(target):
        queue = deque([(target, [])])
        seen, found = set(), []
        while queue:
            key, path = queue.popleft()
            if key in seen:
                continue
            seen.add(key)
            for identity in owner_ids:
                if identity[:2] == key:
                    found.append({"owner": list(identity), "via": path})
            for index, next_key in refs(key):
                queue.append((next_key, path + [{"object": key[0], "motion": key[1],
                                                "reference_index": index, "target": list(next_key)}]))
        return found

    records, direct_guard_hits = [], 0
    for owner in owners:
        item = {k: owner[k] for k in ("object", "motion", "index", "kind", "stencil_type", "sources")}
        item["captured_motion_sources"] = []
        for source in owner["source_layers"]:
            layer = source["layer"]
            if layer.get("type") != 3:
                continue
            direct_guard_hits += int(layer.get("stencilType", 0) & 4 != 0)
            targets = {(f["content"].get("src"), f["content"].get("icon"))
                       for f in layer.get("frameList", []) if f.get("content")}
            for target in sorted(targets):
                if target not in motions:
                    raise ValueError("unresolved actual source motion")
                item["captured_motion_sources"].append({
                    "source_index": source["index"], "source_stencil_type": layer.get("stencilType", 0),
                    "target": list(target), "nested_owners": nested(target)})
        records.append(item)
    cpu_path = "simulator/crates/runtime/src/emote_cpu.rs"
    cpu = (ROOT / cpu_path).read_text(encoding="utf-8")
    observed = {
        "shared_geometry_arcs": "pending: Vec<Option<Arc<DrawLayer>>>" in cpu,
        "one_time_range_publication": "scope.ranges.set(ranges)" in cpu,
        "capture_returned_to_parent": "ranges[index] = range;" in cpu,
        "owner_range_rebuilt_before_draw": "ranges[binding.owner_index] = vec![owner.clone()]" in cpu,
        "mask_consumes_published_owner_range": "self.stencil_source_range(range, &mut plane, reference, operation)?" in cpu,
        "old_motion_resampler_absent": "fn stencil_source_motion(" not in cpu,
        "old_nested_guard_absent": "rebuilt shared range is not recovered" not in cpu,
    }
    summary = {
        "resource": cached["resource"], "resource_sha256": cached["sha256"],
        "actual_composite_owner_definitions": len(owners),
        "captured_motion_source_definitions": sum(len(v["captured_motion_sources"]) for v in records),
        "direct_sprite_source_owner_definitions": sum(not v["captured_motion_sources"] for v in records),
        "old_direct_nested_owner_guard_actual_hits": direct_guard_hits,
        "source_motion_targets_with_nested_owner_definitions": sum(bool(s["nested_owners"])
            for v in records for s in v["captured_motion_sources"]),
        "new_actual_shared_consumers": 1,
        "old_resampling_receiver_replaced": True,
        "actual_guard_hits_removed": direct_guard_hits,
        "nested_shared_range_diagnostic_replaced_by_real_receiver": observed["old_nested_guard_absent"],
        "CPU_static_observation": observed,
        "new_main_hooks_required": [],
        "remaining_six_owner_scope": "shared-range receiver is connected; no additional actual guard proven in this narrow batch, pixels and lifecycle remain unexecuted",
        "remaining_specific_receivers": [
            "textured type12 target/reversal and mode1 remain diagnostic; this fixed model has blank owners",
            "nonneutral/per-corner color and nondefault projection remain diagnostic; not broadened in this batch",
            "strict end-to-end frame and original/native equivalence remain unverified under the execution prohibition"],
        "completed_native_equivalence_added": 0,
        "full_NEKO_strict_pixel_claim": False,
        "compiled": False, "typechecked": False, "tests_run": False,
        "renderer_or_evaluator_executed": False,
        "counts_scope": "fixed model declarations and reference graph, not evaluated active frame counts",
    }
    if SYNTAX.exists():
        grammar = read_json(SYNTAX)
        summary["source_grammar_only"] = {
            "report": str(SYNTAX.relative_to(ROOT)), "returncode": grammar["returncode"],
            "source_matches": grammar["source_sha256"] == sha((ROOT / cpu_path).read_bytes()),
            "not_a_typecheck_or_execution": True}
    per_pe = [dict(previous["perPE"][0], contracts=[
        {"RVA": "44b39..44b67", "meaning": "capture the referenced player's actual primitive pointer range"},
        {"RVA": "44e2a..44e57", "meaning": "append the same leaf primitive pointer to caller capture"},
        {"RVA": "44f30..450ae", "meaning": "own-first rebuild, then append current source group ranges or sprite pointers"},
        {"RVA": "50d0..52b4", "meaning": "texture-only mask pass, ignoring source primitive color-stencil links"}])]
    per_pe[0].pop("spans", None)
    payload = {"summary": summary, "perPE": per_pe, "actual_owner_graph": records,
               "evidence_inputs": [str(CACHE.relative_to(ROOT)),
                    "reports/recovery_A_20261117_emote_composite_ranges.json",
                    "reports/recovery_A_20261117_emote_group_owner.json",
                    "archive/2026-11/20261117_emote_shared_ranges_pe.json"],
               "sources": [{"path": p, "sha256": sha((ROOT / p).read_bytes())}
                           for p in [cpu_path, "scripts/analysis/recovery_A_emote_shared_ranges_20261117.py"]]}
    PREFIX.with_suffix(".json").write_text(json.dumps(payload, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    PREFIX.with_name(PREFIX.name + "_summary.json").write_text(
        json.dumps({"summary": summary, "perPE": per_pe}, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    PREFIX.with_suffix(".md").write_text(
        "# A / 六个实际composite owner的共享图元范围\n\n"
        "直接修改emote_cpu.rs的collect/render/composite窄域：同一次采样得到的DrawLayer由Arc持有，"
        "collect_motion返回实际捕获范围；每motion在颜色绘制前按44f69重建own-first composite列表。"
        "mask消费已发布的范围，沿现有texture/mesh三角形、GREATER64、INCR/DECR及颜色EQUAL到RGBA；"
        "不再重采样源motion，不重用源颜色stencil作二次裁切。范围只持有geometry，无scope反向引用，"
        "帧结束随颜色列表/plane缓存释放；无新的tick/import/宿主接口。\n\n"
        "固定ca01_l_body_1.psz的6个type12/type5 owner中，3个引用motion源、3个使用sprite源。"
        "旧1496 guard实际命中0：3个source层本身stencilType均0；引用motion内部的owner与此条件不同。"
        "本批实现的是这3个实际motion源缺失的共享capture消费者，并统一6owner的range接收；"
        "不把零命中guard删除计作实际阻断减少。具体nested路径见JSON actual_owner_graph。\n\n"
        "实际嵌套为head_parts/頭部全体変形#15→source#14→頭部変形基礎#6，"
        "后者的reference#144/#148分别进入face_parts/目R#8与目L#8。"
        "另外两个motion源为body_parts/下半身変形基礎及全身変形基礎，内部无owner。\n\n"
        "证据复用driver SHA85eb459e…93ce：44b39捕获子图元，44e2a插入同一primitive指针，"
        "44f69清空再own-first，4501b读取source当前range；50d0..52b4不跟源颜色stencil链接。"
        "blank owner仍不产生替代矩形，缺失label仍忽略，隐藏/未进入priority源不贡献geometry。\n\n"
        "仍缺textured type12目标/反转及mode1等现有诊断，未扩无命中分支。"
        "新共享消费者1；完整native等价新增0，未宣称完整NEKO像素。"
        "仅单模型静态格式读取、既有PE合同及源码读取；未编译、类型检查、测试或执行采样/渲染算法。"
        "脚本已分类analysis，结构与两段窄PE证据归archive/2026-11/20261117_*；未改README。\n", encoding="utf-8")
    current_path = ROOT / "reports/recovery_A_20261117_emote_current_summary.json"
    current = read_json(current_path)
    current["summary"]["latest_composite_shared_ranges"] = {
        "report": str(PREFIX.with_name(PREFIX.name + "_summary.json").relative_to(ROOT)),
        "actual_owner_definitions": len(owners), "actual_motion_source_definitions": 3,
        "old_guard_actual_hits": direct_guard_hits, "new_shared_consumers": 1,
        "CPU_internal_receiver_connected": all(observed.values()),
        "main_hooks_pending": [], "pixels_executed": False,
        "complete_native_equivalence_added": 0}
    blockers = current["summary"].get("remaining_strict_blockers", [])
    for index, value in enumerate(blockers):
        if "kind3 composite owner used as another owner's direct source" in value:
            blockers[index] = (
                "local/caller chains, ancestor-tail transfer and same-sample captured ranges now feed owner rebuild and real mask geometry; "
                "the former direct nested-range diagnostic is replaced by shared-range consumption, actual first-model guard hits0; "
                "textured type12/mode1 remain diagnostic, full frame equivalence unexecuted")
    current_path.write_text(json.dumps(current, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(summary, ensure_ascii=True))


def syntax():
    source = ROOT / "simulator/crates/runtime/src/emote_cpu.rs"
    raw = source.read_bytes()
    process = subprocess.run([str(Path.home() / ".cargo/bin/rustfmt.exe"), "--edition", "2024",
                              "--config", "skip_children=true", "--emit", "stdout"],
                             input=raw, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    observed = {"source": str(source.relative_to(ROOT)), "source_sha256": sha(raw),
                "method": "rustfmt grammar via stdin; output discarded, source never formatted",
                "returncode": process.returncode,
                "stderr": process.stderr.decode("utf-8", errors="replace"),
                "compiler_invoked": False, "typechecked": False, "tests_run": False,
                "target_source_executed": False}
    SYNTAX.write_text(json.dumps(observed, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"grammar_returncode": process.returncode, "typechecked": False}))
    if process.returncode:
        raise SystemExit(process.returncode)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--probe", action="store_true")
    parser.add_argument("--report", action="store_true")
    parser.add_argument("--syntax", action="store_true")
    args = parser.parse_args()
    if args.probe:
        probe()
    if args.syntax:
        syntax()
    if args.report:
        report()


if __name__ == "__main__":
    main()

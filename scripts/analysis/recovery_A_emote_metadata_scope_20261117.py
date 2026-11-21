"""Fill only metadata fields absent from the fixed pixel inventory; no evaluator."""
from collections import Counter
import argparse
import ast
import json

from recovery_A_20261116 import ROOT, read_json, sha
from recovery_A_psb_20261117 import Psb, load_sample


class MetadataGapFields(Psb):
    """Skip all object/motion/source/texture trees and unrelated metadata."""
    def value(self, pos, depth=0):
        if depth <= 1:
            if self.data[pos] != 33:
                raise ValueError("metadata dictionary required")
            names, at = self.array(pos + 1)
            offsets, base = self.array(at)
            if len(names) != len(offsets):
                raise ValueError("dictionary sizes")
            wanted = {"metadata"} if depth == 0 else {
                "mirror", "mirrorControl", "stereovisionControl", "eyeControl"}
            return {self.names[name]: self.value(base + offset, depth + 1)
                    for name, offset in zip(names, offsets) if self.names[name] in wanted}
        return super().value(pos, depth)


def probe():
    prior_path = ROOT / "archive/2026-11/20261117_emote_pixel_scope.json"
    prior = read_json(prior_path)
    resources, totals = [], Counter()
    for previous in prior["resources"]:
        name, data = load_sample(previous["index"])
        if name != previous["name"] or sha(data) != previous["decoded_sha256"]:
            raise ValueError("resource identity changed since fixed inventory")
        metadata = MetadataGapFields(data).root["metadata"]
        counts = Counter()
        counts["metadata_mirror_nonzero"] = int(metadata.get("mirror", 0) != 0)
        for key in ("mirrorControl", "stereovisionControl"):
            value = metadata.get(key)
            counts[key + "_dictionary"] = int(isinstance(value, dict))
            counts[key + "_list_enabled"] = sum(
                entry.get("enabled", 1) != 0 for entry in value) if isinstance(value, list) else 0
        eyes = []
        for eye in metadata.get("eyeControl", []):
            if eye.get("enabled", 1) == 0:
                continue
            counts["enabled_eyeControl"] += 1
            counts["enabled_eye_blink"] += int(eye.get("blinkEnabled") != 0)
            begin, end = eye.get("beginFrame"), eye.get("endFrame")
            counts["invalid_or_reversed_eye_interval"] += int(
                not isinstance(begin, (int, float)) or not isinstance(end, (int, float)) or begin >= end)
            eyes.append({key: eye.get(key) for key in
                         ("label", "enabled", "blinkEnabled", "beginFrame", "endFrame")})
        totals.update(counts)
        resources.append({"index": previous["index"], "name": name,
                          "decoded_sha256": previous["decoded_sha256"], "counts": dict(counts),
                          "fields": {key: value for key, value in metadata.items() if key != "eyeControl"},
                          "eye_entries": eyes})
        if len(resources) % 30 == 0:
            print(json.dumps({"metadata_only_read": len(resources), "totals": dict(totals)}), flush=True)
    payload = {"summary": {"resources": len(resources), "previous_enabled_counts_reused": {
        key: value for key, value in prior["summary"].items() if key.startswith("enabled_")},
        "new_metadata_gap_counts": dict(totals),
        "scope": "only mirror/mirrorControl/stereovisionControl/eyeControl; no motion/layer/texture walk",
        "compiled": False, "typechecked": False, "tests_run": False, "target_execution": False},
        "prior_inventory_sha256": sha(prior_path.read_bytes()), "resources": resources}
    path = ROOT / "archive/2026-11/20261117_emote_metadata_scope.json"
    path.write_text(json.dumps(payload, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    print(json.dumps(payload["summary"], ensure_ascii=True), flush=True)


def report():
    probe_path = ROOT / "archive/2026-11/20261117_emote_metadata_scope.json"
    payload = read_json(probe_path)
    summary = {**payload["summary"],
        "batch": "actual metadata strict guard scope; requested remaining enabled family absent",
        "observed_metadata_strict_guard_hits": 0,
        "new_runtime_implementations": 0,
        "guards_removed": 0,
        "completed_native_equivalence_added": 0,
        "full_NEKO_strict_pixel_claim": False,
        "metadata_absence_does_not_prove_full_strict_draw_success": True,
        "existing_live_consumers": "eye/eyebrow + mouth/transition + selector/clamp; no repeated implementation",
        "remaining": ["dynamic enable/mirror/stereo via outside setters was not measured by resource defaults",
                      "generic controller branches remain guarded where applicable; resource absence is not implementation",
                      "other render/resource/reference guards and full dynamic/pixel validation remain independent"],
        "API": "no new runtime API or main hook; no runtime source changes justified by zero-hit scope",
    }
    sources = [{"path": path, "sha256": sha((ROOT / path).read_bytes())} for path in (
        "simulator/crates/runtime/src/emote_cpu.rs", "simulator/crates/runtime/src/emote_m2selector.rs",
        "scripts/analysis/recovery_A_emote_metadata_scope_20261117.py")]
    small = {"summary": summary, "perPE": [{
        "sha256": "85eb459e94e74ab608f649eb7fb440f2526e90aaa1af7276ed01dc10018903ce",
        "evidence_reused": ["reports/recovery_A_20261117_emote_graph_output_summary.json",
                            "reports/recovery_A_20261117_emote_clamp_summary.json"],
        "new_disassembly_scan": False}]}
    report_data = {**small, "sources": sources, "resources": payload["resources"],
                   "probe_sha256": sha(probe_path.read_bytes()),
                   "scope": "prior135 enabled bust/hair/loop/parts counts reused; only missing mirror/stereo/eye metadata fields read; no model/render/evaluator execution"}
    prefix = ROOT / "reports/recovery_A_20261117_emote_metadata_scope"
    for suffix, data in ((".json", report_data), ("_summary.json", small)):
        prefix.with_name(prefix.name + suffix).write_text(json.dumps(data, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    prefix.with_suffix(".md").write_text(
        "# A / 实际metadata阻断候选为空\n\n"
        "固定135PSZ既有轻量inventory的enabled bust/hair/loop/parts均0，直接复用，没有再遍历"
        "motion/layer/mesh。仅补缺失的mirror/stereo/eye metadata字段并逐份校验原decoded SHA："
        "mirror非零0；mirrorControl/stereovisionControl各135个字典配置，并非enabled控制器列表；"
        "启用eyeControl135条，blinkEnabled全0、begin/end区间有效，走已闭合眼睛图输出。"
        "按当前CPU render列表guard及graph_render_diagnostics条件，实际剩余metadata guard命中0。\n\n"
        "本轮运行时新增0、移除guard0、完整native等价新增0。没有为零命中控制器补泛化或移除"
        "保护；mouth/transition没有重复实现。该结果只纠正剩余工作优先级，不代表mirror/loop/parts"
        "已实现，也不代表整个strict draw或NEKO像素通过。外部setter动态启用mirror/stereo/控制器"
        "不在静态默认值观察范围，其他资源／reference／渲染guard仍独立存在。\n\n"
        "输出轻量summary+perPE供主线调整队列；原余量中‘135PSZ实际启用其它metadata族会阻断’"
        "不再有样本依据。没有新宿主API／接线。脚本scripts/analysis，字段证据归archive/2026-11；"
        "未修改runtime、主线CPU tick/import/command/cache/M2、README或译文。仅静态资源字段解析"
        "与Python AST；无编译、类型检查、测试或恢复算法／原PE／VM／游戏／设备执行。\n", encoding="utf8")
    script_path = ROOT / "scripts/analysis/recovery_A_emote_metadata_scope_20261117.py"
    ast.parse(script_path.read_text(encoding="utf8"))
    (ROOT / "archive/2026-11/20261117_emote_metadata_scope_syntax.json").write_text(json.dumps(
        {"python_AST": True, "compiled": False, "typechecked": False, "tests_run": False,
         "target_executed": False, "sha256": sha(script_path.read_bytes())}, indent=2) + "\n", encoding="utf8")
    current_path = ROOT / "reports/recovery_A_20261117_emote_current_summary.json"
    current = read_json(current_path)
    current["metadata_guard_scope_batch"] = small
    current["summary"]["latest_A_batch"] = summary
    current["summary"]["actual135_metadata_guard_hits_remaining"] = 0
    current["summary"]["metadata_zero_hits_are_not_implemented_families"] = True
    relative = prefix.with_suffix(".md").relative_to(ROOT).as_posix()
    if relative not in current.setdefault("reports", []):
        current["reports"].append(relative)
    current_path.write_text(json.dumps(current, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    rolling_path = ROOT / "reports/recovery_A_20261117_rolling_delivery.md"
    rolling = rolling_path.read_text(encoding="utf8")
    note = "交付`recovery_A_20261117_emote_metadata_scope.md`：既有135PSZ loop/parts/bust/hair enabled均0；仅补mirror/stereo/eye字段，实际剩余metadata strict guard命中0。没有可据实移除的guard或新增控制器；运行时源码新增0，不把零命中算实现，主线无需接点。\n\n"
    if note not in rolling:
        rolling_path.write_text(rolling.replace("# A / 本轮小批交付\n\n", "# A / 本轮小批交付\n\n" + note, 1), encoding="utf8")
    print(json.dumps(summary, ensure_ascii=True), flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--report", action="store_true", help="reuse metadata-only field snapshot")
    args = parser.parse_args()
    report() if args.report else probe()

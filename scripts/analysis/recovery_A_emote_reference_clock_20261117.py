"""Read actual reference clocks and bounded driver bytes; never evaluate a player."""
from collections import Counter
import argparse
import ast
import json
from pathlib import Path
import subprocess

from recovery_A_20261116 import ROOT, Image, read_json, sha
from recovery_A_emote_model_render_20261117 import DRIVER_SHA, flatten
from recovery_A_psb_20261117 import Psb, load_sample
from recovery_A_receiver_contracts_20261116 import block


SPANS = {
    "reference_player_allocated_once": (0x36321, 0x96),
    "same_motion_name_retains_player": (0x34813, 0x1b),
    "motion_parameter_binding": (0x35124, 0x72),
    "parameter_clock_overrides_delta": (0x3abf1, 0xda),
    "reference_initial_clock": (0x41012, 0x92),
    "initial_seek_clamp": (0x352b0, 0x58),
    "reference_parent_delta_update": (0x41b33, 0x50),
    "hidden_reference_releases_motion": (0x41b83, 0x70),
}


def probe():
    identity = read_json(ROOT / "reports/recovery_A_20261116_emote.json")
    driver = Image((ROOT / identity["binary_path"]).parent / "emotedriver.dll", DRIVER_SHA)
    totals, resources = Counter(), []
    for index in range(135):
        name, data = load_sample(index)
        root = Psb(data).root
        counts, rows = Counter(), []
        for obj_name, obj in root["object"].items():
            for motion_name, motion in obj["motion"].items():
                for layer_index, layer in enumerate(flatten(motion.get("layer", []))):
                    if layer.get("type", 2) != 3:
                        continue
                    counts["reference_layers"] += 1
                    parameterized = layer.get("parameterize") is not None
                    counts["caller_parameter_layers" if parameterized else "caller_time_layers"] += 1
                    for frame in layer.get("frameList", []):
                        content = frame.get("content") or {}
                        if frame.get("type", 2) == 0:
                            counts["hidden_reference_frames"] += 1
                        if "src" not in content or "icon" not in content:
                            continue
                        target = root["object"].get(content["src"], {}).get("motion", {}).get(content["icon"])
                        counts["reference_frame_records"] += 1
                        if target is None:
                            counts["unresolved_target_records"] += 1
                            continue
                        binding = target.get("parameterize")
                        param = target.get("parameter", [])[binding] if binding is not None else None
                        counts["target_parameter_records" if param else "target_delta_records"] += 1
                        counts["target_loop_records" if target.get("loopTime", -1) >= 0 else "target_nonloop_records"] += 1
                        counts["target_zero_duration_records" if target.get("lastTime", 0) == 0 else "target_nonzero_duration_records"] += 1
                        settings = content.get("motion") or {}
                        if settings.get("mask", 0):
                            counts["nonzero_motion_mask_records"] += 1
                        if settings.get("timeOffset", 0):
                            counts["nonzero_time_offset_records"] += 1
                        rows.append({"object": obj_name, "motion": motion_name,
                                     "layer": layer_index, "label": layer.get("label"),
                                     "caller_parameterize": layer.get("parameterize"),
                                     "begin": frame.get("time", 0), "frame_type": frame.get("type", 2),
                                     "target": [content["src"], content["icon"]],
                                     "target_parameterize": binding,
                                     "target_parameter": param,
                                     "lastTime": target.get("lastTime", 0),
                                     "loopTime": target.get("loopTime", -1),
                                     "settings": settings})
        totals.update(counts)
        resources.append({"index": index, "name": name, "decoded_sha256": sha(data),
                          "counts": dict(counts), "references": rows})
        if (index + 1) % 30 == 0:
            print(json.dumps({"resources_read": index + 1, "counts": dict(totals)}), flush=True)
    payload = {"summary": {"resources": len(resources), **dict(totals)},
               "perPE": [{"sha256": driver.digest, "addresses": "RVA", "spans": SPANS}],
               "evidence": {key: block(driver, *span) for key, span in SPANS.items()},
               "resources": resources}
    path = ROOT / "archive/2026-11/20261117_emote_reference_clock_probe.json"
    path.write_text(json.dumps(payload, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    print(json.dumps(payload["summary"], ensure_ascii=True), flush=True)


def report():
    probe = read_json(ROOT / "archive/2026-11/20261117_emote_reference_clock_probe.json")
    identity = read_json(ROOT / "reports/recovery_A_20261116_emote.json")
    driver = Image((ROOT / identity["binary_path"]).parent / "emotedriver.dll", DRIVER_SHA)
    bindings = Counter((row["target_parameter"] or {}).get("id")
                       for resource in probe["resources"] for row in resource["references"]
                       if row["target_parameter"] is not None)
    summary = {
        "batch": "actual referenced motion parameter owner -> target layer and priority time -> existing color/mask pixels",
        "actual_inventory": probe["summary"],
        "parameter_reference_frame_records_by_input": dict(bindings),
        "actual_resource_count_with_parameter_targets": sum(
            r["counts"].get("target_parameter_records", 0) > 0 for r in probe["resources"]),
        "source_chain": "effective_variables -> bound_parameter_clock(target motion.parameterize) -> motion_frame -> layer + priority -> captured reference primitives -> atlas/BP + stencil raster",
        "lifecycle": "parameter-driven target samples current own cursor, including backward changes, without accumulating caller time; frame-local geometry/mask buffers drop after draw; persistent ordinary reference lifecycle is separate and still missing",
        "api": "existing CpuPlayer::render_strict / CpuEmoteBackend::render_rgba; no host hook for this parameter branch",
        "remaining": [
            "ordinary reference player owns raw/sample clocks; caller parameter cursor must not substitute for elapsed delta (actual highlight targets)",
            "same-name keep, hidden selection release/reopen and non-parameter target lifecycle require persistent reference owner",
            "full combined runtime/pixel validation deferred"],
        "zero_observed_reference_loop_offset_mask_not_expanded": True,
        "completed_native_equivalence_added": 0, "full_NEKO_strict_pixel_claim": False,
        "compiled": False, "typechecked": False, "tests_run": False, "target_execution": False,
    }
    small = {"summary": summary, "perPE": [{"sha256": driver.digest,
             "addresses": "RVA", "spans": SPANS}]}
    paths = ["simulator/crates/runtime/src/emote_cpu.rs",
             "scripts/analysis/recovery_A_emote_reference_clock_20261117.py"]
    sources = [{"path": p, "sha256": sha((ROOT / p).read_bytes())} for p in paths]
    payload = {**small, "sources": sources,
               "per_resource": [{k: r[k] for k in ("index", "name", "decoded_sha256", "counts")}
                                for r in probe["resources"]],
               "evidence": {key: block(driver, *span) for key, span in SPANS.items()}}
    prefix = ROOT / "reports/recovery_A_20261117_emote_reference_clock"
    for suffix, value in ((".json", payload), ("_summary.json", small)):
        prefix.with_name(prefix.name + suffix).write_text(
            json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    prefix.with_suffix(".md").write_text(
        "# A / 实际引用motion参数时钟\n\n"
        "新增RenderContext::bound_parameter_clock/motion_frame，颜色collect_motion和两条composite-mask入口"
        "统一选择目标motion自己的parameterize。此前错误地使用caller时间；现在face_talk/face_tears/"
        "face_eye_open/face_eyebrow/head_LR/face_cheek经目标参数range转换，直接进入目标layer及priority采样，"
        "并由已有真实atlas/BP和stencil像素消费者使用。参数反向变化也直接重新采样，无需假造delta。\n\n"
        f"135份PSZ共6075条引用帧记录，其中2430条目标参数驱动，分布{dict(bindings)}；"
        "这是静态资源记录数量，非同时活跃实例或运行通过数量。其余3645条目标普通时钟，包含2025条"
        "零时长目标记录；全部引用目标loopTime=-1、motion.mask/timeOffset为0。参数时钟按原PE优先，"
        "不套普通loop/clamp。\n\n"
        "固定driver SHA256 85eb459e94e74ab608f649eb7fb440f2526e90aaa1af7276ed01dc10018903ce："
        "35124..35144绑定player+2d0到本motion参数表；3abf1..3ac67读取parameter+28并直接seek/前后采样，"
        "绕过普通delta。尚未把普通reference当作已完成：36321创建持久player，34813同名保留，"
        "41050初始化caller-frameBegin+offset，41b39传父delta，41b83隐藏时释放motion状态但保留player。"
        "下一批继续实际highlight普通owner，不扩零命中loop/mask/offset。\n\n"
        "源码仅修改CPU绘制区域，未动command/import/advance或M2。仅rustfmt stdin语法读取及Python AST；"
        "无编译、类型检查、测试、算法/原PE/VM/游戏执行。完整native等价新增0，完整NEKO像素未声明。\n",
        encoding="utf8")
    rustfmt = next(Path("C:/Users/jingyue/.rustup/toolchains").glob("*/bin/rustfmt.exe"))
    grammar = subprocess.run([str(rustfmt), "--edition", "2024", "--emit", "stdout"],
                             input=(ROOT / paths[0]).read_bytes(), stdout=subprocess.DEVNULL,
                             stderr=subprocess.PIPE, check=False)
    ast.parse((ROOT / paths[1]).read_text(encoding="utf8"))
    syntax = {"grammar_only": True, "compiled": False, "typechecked": False,
              "tests_run": False, "target_executed": False,
              "sources": sources, "rust_grammar_exit_code": grammar.returncode,
              "rust_stderr": grammar.stderr.decode("utf8", "replace"), "python_AST": True}
    (ROOT / "archive/2026-11/20261117_emote_reference_clock_syntax.json").write_text(
        json.dumps(syntax, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    current_path = ROOT / "reports/recovery_A_20261117_emote_current_summary.json"
    current = read_json(current_path)
    current["reference_parameter_clock_batch"] = small
    current["summary"]["latest_A_batch"] = summary
    relative = prefix.with_suffix(".md").relative_to(ROOT).as_posix()
    if relative not in current.setdefault("reports", []):
        current["reports"].append(relative)
    current_path.write_text(json.dumps(current, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    rolling = ROOT / "reports/recovery_A_20261117_rolling_delivery.md"
    text = rolling.read_text(encoding="utf8")
    note = "交付`recovery_A_20261117_emote_reference_clock.md`：135PSZ的2430/6075条引用帧目标自带参数时钟，现进入颜色／mask真实像素。普通3645条引用owner建时、同名保留及隐藏释放继续，未计已完成。\n\n"
    if note not in text:
        rolling.write_text(text.replace("# A / 本轮小批交付\n\n", "# A / 本轮小批交付\n\n" + note, 1), encoding="utf8")
    print(json.dumps({"summary": summary, "syntax": syntax}, ensure_ascii=True), flush=True)


def owner_report():
    probe = read_json(ROOT / "archive/2026-11/20261117_emote_reference_clock_probe.json")
    identity = read_json(ROOT / "reports/recovery_A_20261116_emote.json")
    driver = Image((ROOT / identity["binary_path"]).parent / "emotedriver.dll", DRIVER_SHA)
    spans = {**SPANS, "ordinary_raw_delta_and_sample_clamp": (0x3ade2, 0x3c),
             "nonloop_stop_keeps_last_sample": (0x3ae32, 0x40),
             "caller_parameter_or_raw_initial_clock": (0x40f4e, 0x41)}
    #Only inspect the already identified first-resource visibility cuts, not
    #another135-resource audit or execution of any restored timeline.
    name, data = load_sample(0)
    root = Psb(data).root
    hidden = [{"object": obj, "motion": motion, "label": layer.get("label"), "time": frame["time"]}
              for obj, ov in root["object"].items() for motion, mv in ov["motion"].items()
              for layer in flatten(mv.get("layer", [])) if layer.get("type", 2) == 3
              for index, frame in enumerate(layer.get("frameList", []))
              if frame.get("type", 2) == 0 and index < len(layer["frameList"]) - 1]
    highlights = [row for resource in probe["resources"] for row in resource["references"]
                  if row["label"] in ("hi_L", "hi_R")]
    summary = {
        "batch": "persistent reference holder clocks + hidden/reopen lifecycle -> shared color/stencil samples",
        "actual_resources": 135, "reference_layers": 4590,
        "ordinary_target_frame_records": 3645, "parameter_target_frame_records": 2430,
        "actual_highlight_reference_frame_records": len(highlights),
        "first_resource_nonterminal_hidden_records": len(hidden),
        "hidden_record_count_is_not_terminal_sentinel_count": True,
        "implemented_source": ["path-keyed own raw/sample/playing state; same selected object/motion keeps clock",
             "new selection initializes caller cursor minus selected key begin plus timeOffset; own target parameter overrides",
             "ordinary nonloop clocks consume player elapsed delta, hold last sample at stop",
             "hidden releases selection/descendants, retains own holder; root/resource player drop releases all",
             "physical reference update visits references outside primitive priority",
             "color and composite-mask passes consume same captured sample; repeated draws do not commit clocks",
             "captured raw+100 travels separately from sampled+104 into nested owner initialization",
             "update stamp includes effective input variables so same-tick command writes cannot masquerade as updated owner history"],
        "integration_API": "CpuPlayer::update_reference_timelines(&mut self) -> Result<()> after every completed variable/frame tick, also ticks with no draw and initial load/select",
        "host_hook_connected_by_A": False,
        "root_rebuild_API": "CpuPlayer::reset_reference_timelines(&mut self); actual root rebuild/seek only, followed by update_reference_timelines",
        "unbound_hook_behavior": "ordinary reference diagnostic prevents strict success; draw never manufactures missed update history",
        "source_chain": "actual variable/frame update -> update_reference_timelines -> physical key sample -> holder select/advance/release -> DrawLayer.reference -> collect_motion/stencil_source_motion -> existing atlas/BP mask/color RGBA",
        "remaining": ["main per-tick/initial-load integration hook remains required",
                      "root elapsed tick source remains existing CpuPlayer.frame; original root playback/seek integration stays main-owned",
                      "seek/reverse/forced flags and nonzero-loop reference variants remain explicit unsupported or outside claimed actual family",
                      "full combined dynamic/pixel validation deferred"],
        "compiled": False, "typechecked": False, "tests_run": False, "target_execution": False,
        "completed_native_equivalence_added": 0, "full_NEKO_strict_pixel_claim": False,
    }
    paths = ["simulator/crates/runtime/src/emote_cpu.rs", "simulator/crates/runtime/src/emote_timeline.rs",
             "scripts/analysis/recovery_A_emote_reference_clock_20261117.py"]
    sources = [{"path": p, "sha256": sha((ROOT / p).read_bytes())} for p in paths]
    small = {"summary": summary, "perPE": [{"sha256": driver.digest, "addresses": "RVA", "spans": spans}]}
    payload = {**small, "sources": sources, "first_resource": name,
               "first_resource_hidden_cuts": hidden,
               "evidence": {key: block(driver, *span) for key, span in spans.items()}}
    prefix = ROOT / "reports/recovery_A_20261117_emote_reference_owner"
    for suffix, value in ((".json", payload), ("_summary.json", small)):
        prefix.with_name(prefix.name + suffix).write_text(
            json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    prefix.with_suffix(".md").write_text(
        "# A / 实际reference owner时钟与生命周期\n\n"
        "已落emote_timeline.rs的CpuReferenceTimelines：每条引用实例按物理索引路径持有独立选中名、"
        "raw/sample时钟及playing。初选按caller游标减当前key起点加timeOffset，后续同名保留，"
        "普通目标按player tick差推进；到lastTime停住但保留末帧。目标自带parameterize仍按前批自己的"
        "参数游标采样。隐藏清选中状态并丢弃所选motion的后代owner，保留本holder以便重新进入；"
        "替换／卸载自然随CpuPlayer销毁。\n\n"
        "CPU绘制端在primitive排序前采样每个物理reference，DrawLayer.reference捕获结果，颜色与"
        "composite-mask两条路径共同消费，杜绝每次mask/draw重开或重复推进。后续小批已独立向下传递"
        "raw+100与sample+104，普通父owner停住后重新显现不会把夹紧sample当作初选raw；目标自身"
        "parameterize仍同时写两条时钟。逐tick更新遍历物理"
        "引用，包括当前priority不输出像素的引用。渲染只用状态副本，不提交时间／显隐。\n\n"
        "**主线接线API：CpuPlayer::update_reference_timelines(&mut self)->Result<()>，在每次变量／"
        "frame完整更新之后调用，包括无绘制tick、初始load/select；不应只在draw调用。**"
        "A没有修改advance/import/command，当前尚未插入主线。没有本tick更新标记时普通引用发出"
        "明确diagnostic阻断strict，不能把API存在计作运行完成。该hook仅采样引用，不再次运行M2"
        "控制器，也不分配RGBA像素。更新标记同时比较effective variables，避免同tick写变量后"
        "误认owner已更新。真正重建／seek根层树时先调reset_reference_timelines，再update；普通"
        "reference同名保留不得调用reset。\n\n"
        f"135PSZ既有清单：6075条引用帧，目标参数2430、普通3645；hi_L/hi_R实际{len(highlights)}条。"
        f"首模型有{len(hidden)}条非末尾隐藏key：眼左右瞳及hi_L/hi_R各在20，瞳后续40重新进入。"
        "这与普通末尾sentinel分开记录，不把5130条全部隐藏记录误称真实释放次数。"
        "原PE固定driver SHA85eb459e...903ce，36321建holder player；34813同名早返；41050..41067"
        "算初始时间；41b39传父delta；3ade2推进raw并夹sample；41b83..41be7隐藏释放选中motion。\n\n"
        "剩余：主线hook接入；根player的elapsed tick来源沿用CpuPlayer.frame，实际根playback／seek"
        "接线仍归主线；"
        "reverse/seek/强制flag与非零loop不作已恢复声明。实际135资源没有非零motion.mask、offset或"
        "loop，不扩零命中分支。无编译、类型检查、测试或实现执行；仅源grammar读取。完整native"
        "等价新增0，完整NEKO像素未声明。\n", encoding="utf8")
    fmt = next(Path("C:/Users/jingyue/.rustup/toolchains").glob("*/bin/rustfmt.exe"))
    checks = []
    for source in paths[:2]:
        grammar = subprocess.run([str(fmt), "--edition", "2024", "--emit", "stdout"],
                                 input=(ROOT / source).read_bytes(), stdout=subprocess.DEVNULL,
                                 stderr=subprocess.PIPE, check=False)
        checks.append({"path": source, "grammar_exit_code": grammar.returncode,
                       "stderr": grammar.stderr.decode("utf8", "replace")})
    ast.parse((ROOT / paths[2]).read_text(encoding="utf8"))
    (ROOT / "archive/2026-11/20261117_emote_reference_owner_syntax.json").write_text(json.dumps(
        {"grammar_only": True, "compiled": False, "typechecked": False, "tests_run": False,
         "target_executed": False, "checks": checks, "sources": sources}, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    current_path = ROOT / "reports/recovery_A_20261117_emote_current_summary.json"
    current = read_json(current_path)
    current["reference_owner_clock_batch"] = small
    current["summary"]["latest_A_batch"] = summary
    for source in sources:
        current["source_hashes_current"] = [row for row in current.get("source_hashes_current", [])
                                             if row["path"] != source["path"]]
        current["source_hashes_current"].append(source)
    relative = prefix.with_suffix(".md").relative_to(ROOT).as_posix()
    if relative not in current.setdefault("reports", []):
        current["reports"].append(relative)
    current_path.write_text(json.dumps(current, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    rolling_path = ROOT / "reports/recovery_A_20261117_rolling_delivery.md"
    rolling = rolling_path.read_text(encoding="utf8")
    note = "交付`recovery_A_20261117_emote_reference_owner.md`：持久引用raw/sample、同名保留、隐藏释放／重入、颜色mask共享采样已落源码；主线需在每个完成tick及初始load调用`CpuPlayer::update_reference_timelines`，A未越界改advance/import，未接hook不计完成。\n\n"
    if note not in rolling:
        rolling_path.write_text(rolling.replace("# A / 本轮小批交付\n\n", "# A / 本轮小批交付\n\n" + note, 1), encoding="utf8")
    print(json.dumps({"summary": summary, "grammar": checks}, ensure_ascii=True), flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--report", action="store_true", help="reuse the fixed read-only resource probe")
    parser.add_argument("--owner-report", action="store_true", help="deliver reference owners without resurveying135 resources")
    args = parser.parse_args()
    owner_report() if args.owner_report else report() if args.report else probe()

"""Reuse fixed135 reference inventory; bounded direction evidence, no player run."""
from collections import Counter
import ast
import json
from pathlib import Path
import subprocess

from recovery_A_20261116 import ROOT, Image, read_json, sha
from recovery_A_emote_model_render_20261117 import DRIVER_SHA
from recovery_A_receiver_contracts_20261116 import block


SPANS = {
    "parameter_sample_overrides_delta_and_selects_both_directions": (0x3abf1, 0x7e),
    "new_reverse_zero_time_starts_at_end": (0x3acc0, 0x47),
    "first_sample_does_not_add_delta_twice": (0x3ade2, 0x3c),
    "ordinary_forward_stop": (0x3ae32, 0x40),
    "ordinary_reverse_nonloop_stop": (0x3af14, 0x55),
    "caller_forced_flags_not_just_resource_mask": (0x40ee1, 0x17),
    "caller_own_parameter_or_inherited_flags": (0x40f4e, 0x18),
    "caller_flags_or_resource_mask": (0x41012, 0x1c),
    "same_name_unless_force_bits_1_or_4": (0x34813, 0x2b),
    "initial_parameter_flags_zero": (0x327ad, 0x0c),
    "parameter_record_stores_initial_flags": (0x3294c, 0x10),
    "live_variable_cursor_write_clears_flags": (0x33140, 0x26),
}


def main():
    probe = read_json(ROOT / "archive/2026-11/20261117_emote_reference_clock_probe.json")
    rows = [row for resource in probe["resources"] for row in resource["references"]]
    modes = Counter("parameter" if row["target_parameter"] is not None else "ordinary"
                    for row in rows)
    parameter_inputs = Counter(row["target_parameter"]["id"] for row in rows
                               if row["target_parameter"] is not None)
    zero = {
        "target_nonnegative_loopTime": sum(row["loopTime"] >= 0 for row in rows),
        "resource_motion_mask_nonzero": sum(row["settings"].get("mask", 0) != 0 for row in rows),
        "resource_timeOffset_nonzero": sum(row["settings"].get("timeOffset", 0) != 0 for row in rows),
    }
    identity = read_json(ROOT / "reports/recovery_A_20261116_emote.json")
    driver = Image((ROOT / identity["binary_path"]).parent / "emotedriver.dll", DRIVER_SHA)
    summary = {
        "batch": "actual nonloop ordinary/parameter reference direction consumers",
        "inventory_reused_no_new135_scan": True,
        "actual_resources": len(probe["resources"]), "reference_frame_records": len(rows),
        "target_time_modes": dict(modes), "parameter_input_records": dict(parameter_inputs),
        "zero_observed_resource_variants_deferred": zero,
        "negative_tick_occurrence_not_measured": True,
        "source_delta": [
            "nonloop ordinary clocks now consume finite negative delta rather than reject it",
            "reverse crosses below0 -> raw/sample0 and stop; exactly0 retains playing",
            "new selection on known negative update at raw0 starts at lastTime; skips first delta addition",
            "parameter frame takes precedence over ordinary direction; its backward sample remains unclamped",
        ],
        "pixel_chain": "main existing per-tick update_reference_timelines -> owner raw/sample -> captured DrawLayer.reference -> color/composite-mask -> atlas/BP triangles -> RGBA",
        "integration": "existing main hook; no new tick/import/command/cache/M2 edits or host call required",
        "main_hook_status": "main confirmed initial raw/M2 loads, complete update before surface capture, finish, root select/reset, timeline play/stop and932 preparation",
        "resource_mask_zero_does_not_prove_caller_force_flags_zero": True,
        "known_parameter_flag_writes": "327ad initial local flags0 ->32959 record+2c;33159 live cursor write clears+2c; no inferred mapping from movement direction to force bits",
        "remaining": [
            "caller parameter+2c forced selection bits1/4 producer is not bound; resource mask0 alone cannot close it",
            "first-ever tick after reset has no previous elapsed timestamp; direction is unknown unless host supplies actual delta",
            "root negative speed/seek semantics remain main-owned; measured negative-tick call frequency is unknown",
            "nonzero loop/offset are zero in fixed resource inventory, deferred",
            "full dynamic/pixel validation deferred",
        ],
        "completed_native_equivalence_added": 0, "full_NEKO_strict_pixel_claim": False,
        "compiled": False, "typechecked": False, "tests_run": False, "target_execution": False,
    }
    paths = ["simulator/crates/runtime/src/emote_timeline.rs",
             "simulator/crates/runtime/src/emote_cpu.rs",
             "scripts/analysis/recovery_A_emote_reference_direction_20261117.py"]
    sources = [{"path": path, "sha256": sha((ROOT / path).read_bytes())} for path in paths]
    small = {"summary": summary, "perPE": [{"sha256": driver.digest, "addresses": "RVA", "spans": SPANS}]}
    payload = {**small, "sources": sources,
               "inventory_source": "archive/2026-11/20261117_emote_reference_clock_probe.json",
               "inventory_sha256": sha((ROOT / "archive/2026-11/20261117_emote_reference_clock_probe.json").read_bytes()),
               "evidence": {name: block(driver, *span) for name, span in SPANS.items()}}
    prefix = ROOT / "reports/recovery_A_20261117_emote_reference_direction"
    for suffix, value in ((".json", payload), ("_summary.json", small)):
        prefix.with_name(prefix.name + suffix).write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    prefix.with_suffix(".md").write_text(
        "# A / 实际reference共用方向消费者\n\n"
        "直接沿既有135PSZ清单：2430条目标参数时钟、3645条普通时钟；非零loop/motion.mask/"
        "timeOffset均0，不重扫资源或扩该分支。参数游标前后改变已共用自采样与真实颜色／mask；"
        "本批将普通非循环owner的负delta由拒绝改为原PE接收：3af14..3af4f仅raw<0时置raw/sample0"
        "并停住，恰好0保持active；3acc0..3acf8在已知负更新中初选raw0改为lastTime；3ade2..3adea"
        "由f9跳过首次delta累加。停止后的同名选择仍不重开。原始／采样时间由前批捕获给现有"
        "color/stencil接收器，未新增宿主调用。\n\n"
        "主线已确认update_reference_timelines落在初始raw/M2加载、完整tick/surface capture前、"
        "finish、根选择reset后、play/stop timeline、932准备；此补记替代上一报告的未接hook状态。"
        "A只改emote_timeline，不碰主线tick/import/command/cache/M2。负tick本身的实际调用频率"
        "未知，2430/3645是资源帧记录而非反向运行次数。\n\n"
        "仍缺：caller parameter+2c提供的强制flags生产者。40ee1/40f4e取该字段，4101b与resource"
        "motion.mask合并，34813按bits1/4决定同名是否重选。因此资源mask0不能证明动态强制flags0；"
        "已核327ad初始flags0经32959存入record+2c，33159写游标时清+2c；不能把参数向后移动"
        "自行映射成强制bit1/4。首次reset后无前一tick，反向初始方向需宿主实际delta，未伪造。非零loop/offset"
        "后置，完整native等价新增0，完整NEKO像素未声明。仅源grammar读取及静态证据，无编译／"
        "类型检查／测试／实现、PE、VM、游戏执行。\n", encoding="utf8")
    fmt = next(Path("C:/Users/jingyue/.rustup/toolchains").glob("*/bin/rustfmt.exe"))
    checks = []
    #Only changed Rust source; CPU's concurrent main updates are outside this batch.
    grammar = subprocess.run([str(fmt), "--edition", "2024", "--emit", "stdout"],
                             input=(ROOT / paths[0]).read_bytes(), stdout=subprocess.DEVNULL,
                             stderr=subprocess.PIPE, check=False)
    checks.append({"path": paths[0], "grammar_exit_code": grammar.returncode,
                   "stderr": grammar.stderr.decode("utf8", "replace")})
    ast.parse((ROOT / paths[2]).read_text(encoding="utf8"))
    (ROOT / "archive/2026-11/20261117_emote_reference_direction_syntax.json").write_text(json.dumps(
        {"grammar_only": True, "compiled": False, "typechecked": False, "tests_run": False,
         "target_executed": False, "checks": checks}, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    current_path = ROOT / "reports/recovery_A_20261117_emote_current_summary.json"
    current = read_json(current_path)
    current["reference_direction_batch"] = small
    current["summary"]["latest_A_batch"] = summary
    current["summary"]["reference_owner_update_hook_main_connected"] = True
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
    note = "交付`recovery_A_20261117_emote_reference_direction.md`：实际nonloop共享owner已接负delta、反向严格跨0停止与已知反向初选末帧；主线hook已确认落盘，无新接点。2430/3645为目标参数／普通资源记录，不是负tick频率；caller动态强制flags仍缺生产者，零resource mask不可代替证明。\n\n"
    if note not in rolling:
        rolling_path.write_text(rolling.replace("# A / 本轮小批交付\n\n", "# A / 本轮小批交付\n\n" + note, 1), encoding="utf8")
    print(json.dumps({"summary": summary, "grammar": checks}, ensure_ascii=True), flush=True)


if __name__ == "__main__":
    main()

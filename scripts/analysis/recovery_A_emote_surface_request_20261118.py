"""Static contract for actual NEKO931 callers and deferred surface consumption."""
import hashlib
import json

from recovery_A_20261116 import ROOT, read_json


PREFIX = ROOT / "reports/recovery_A_20261118_emote_surface_request"
CPU = "simulator/crates/runtime/src/emote_cpu.rs"
SURFACE = "simulator/crates/runtime/src/emote_surface.rs"
CACHE = "archive/2026-11/20261118_emote_surface_request.json"


def location(path, text, anchor):
    at = text.find(anchor)
    return {"path": path, "anchor": anchor,
            "line": None if at < 0 else text[:at].count("\n") + 1}


def main():
    prior = read_json(ROOT / "reports/emote_surface_contracts_20261117.json")
    bounded = read_json(ROOT / CACHE)
    if bounded["PE_sha256"] != prior["pe_sha256"]:
        raise ValueError("surface evidence identity mismatch")
    inventory = read_json(ROOT / "reports/kcs_call_inventory_20261116.json")
    game = next(game for game in inventory["games"] if "NEKO" in game["root"])
    sites = [{"resource": program["resource"], "offset": call["offset"],
              "argument_bytes": call["argument_bytes"]}
             for program in game["programs"] for call in program["calls"] if call["index"] == 931]
    cpu = (ROOT / CPU).read_text(encoding="utf8")
    surface = (ROOT / SURFACE).read_text(encoding="utf8")
    arm = cpu.split("EmoteOperation::SurfaceFade => {", 1)[1].split(
        "EmoteOperation::RecomputeControls", 1)[0]
    helper = cpu.split("    fn prepare_m2_surface_update(", 1)[1].split(
        "    pub fn render_preview(", 1)[0]
    update = cpu.split("    fn advance_with_random(", 1)[1].split("    fn ", 1)[0]
    observed = {
        "931_only_stages_capture": "state.surface.request_capture(command.data1)" in arm
            and "capture_surface(task)" not in arm,
        "unloaded_player_diagnostic_retained": "state.player.is_none()" in arm,
        "pre_update_helper_in_source": "self.capture_surface(task)?" in helper,
        "existing_hold_precedes_old_capture": helper.find("state.surface.hold(now)")
            < helper.find("self.capture_surface(task)?"),
        "zero_or_disabled_skips_capture": "enabled && self.duration != 0" in surface
            and ".surface.discard_pending()" in helper,
        "helper_does_not_advance_owner_or_driver": not any(anchor in helper for anchor in (
            "advance_frames(", "apply_907(", "advance_with_random(", "consume_m2_values(")),
        "manager_update_uses_helper": "prepare_m2_surface_update(" in update,
        "old_direct_hold_removed": "state.surface.hold(tick.task_elapsed)" not in update,
        "uncaptured_request_is_not_silent_success":
            "M2 deferred surface request must capture before driver update" in surface,
    }
    connected = observed["manager_update_uses_helper"] and observed["old_direct_hold_removed"]
    hook = ("CPU advance_with_random: before borrowing CpuTask, call "
            "if self.prepare_m2_surface_update(task,tick.task_elapsed)? { return Ok(()); }; "
            "remove its old direct state.surface.hold(tick.task_elapsed) block. "
            "The helper replaces that hold; do not run both.")
    summary = {
        "batch": "931 pending setter -> manager pre-update old RGBA capture -> existing strict new pixels/blend/release",
        "actual_931_static_sites": sites,
        "actual_runtime_hits_measured": False,
        "original_setter": "1743c0 writes+a0/+9c only; it has no render or capture call",
        "original_consumer": "17add5 existing hold;17afd5/17afe0 gate pending and positive duration;17afe7 captures old;1741dc clears9c after driver update",
        "source_observed": observed,
        "new_shared_receivers": 1,
        "new_complete_shared_chains": int(connected),
        "main_hooks_pending": [] if connected else [hook],
        "remaining": ["931 before a loaded PSB player remains explicit; no fabricated old transparent frame",
                      "original complete multi-player manager and object-graph persistence remain separate"],
        "full_native_or_original_function_equivalence_added": 0,
        "new_model_or_full_game_scan": False,
        "compiled": False, "typechecked": False, "grammar_reader_run": False,
        "tests_run": False, "target_or_pixel_algorithm_executed": False,
    }
    contracts = {"1743c0..1743d6": "setter writes raw duration and pending flag without old-surface capture",
                 "17add5..17ae4e": "active hold remains authoritative before a later bare545 request",
                 "17aee9..17af36": "requested/scene/actual engine gates capture",
                 "17afd5..17afe7": "pending plus positive duration captures old surface at manager update",
                 "1741dc/1741e6": "actual player update clears pending9c/a8",
                 "17b7a3..17b7d4": "capture new and arm the existing owned blend"}
    per_pe = [{"sha256": bounded["PE_sha256"], "addresses": "RVA", "contracts": contracts}]
    payload = {"summary": summary, "perPE": per_pe,
        "sources": [location(CPU, cpu, "    fn prepare_m2_surface_update("),
                    location(CPU, cpu, "EmoteOperation::SurfaceFade => {"),
                    location(SURFACE, surface, "    pub(super) fn request_capture(")],
        "source_sha256": {name: hashlib.sha256((ROOT / name).read_bytes()).hexdigest()
                          for name in (CPU, SURFACE)},
        "evidence": {"bounded_new_PE": CACHE,
                     "reused_surface_contracts": "reports/emote_surface_contracts_20261117.json",
                     "reused_KCS_arguments": "archive/2026-11/20261118_emote_dynamic_receivers.json"}}
    for suffix, value in ((".json", payload), ("_summary.json", {"summary": summary, "perPE": per_pe})):
        PREFIX.with_name(PREFIX.name + suffix).write_text(
            json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    status = "主线更新前接点已静态观察，未运行验证。" if connected else (
        "待主线一个更新前接点：advance_with_random在借用task前调用"
        "prepare_m2_surface_update(task,tick.task_elapsed)，true即返回；删除原直接surface.hold块，"
        "不可运行两次hold。当前不记源码闭链。")
    PREFIX.with_suffix(".md").write_text(
        "# A / 931延后捕获的共享表面消费者\n\n"
        "NEKO sscript实际931位置339474、557658、806892，均8bytes。既有参数缓存可见"
        "339474取R+680任务及R+896时长，806892取任务及参数slot4；未测动态次数/具体时长。\n\n"
        "固定PE c71c5b85…1adc的1743c0仅写+a0和+9c，原CPU却在命令接收时strict capture。"
        "原版先处理17add5活动hold；17afd5/17afe0在下一manager更新检查请求及正时长，"
        "17afe7才捕获旧面，1741dc在driver更新后清9c。现931仅记录请求，新"
        "prepare_m2_surface_update先处理hold退场，再按真实门控捕获旧RGBA；禁用/零时长"
        "不触发像素捕获。原有资源替换预捕获、strict新面、整数混合、最终tint和释放沿用。\n\n"
        f"{status}\n\n"
        "未装载PSB的931仍诊断，不制造透明旧面；已有PSB但求值不支持仍在真正捕获时明确报错。"
        "完整manager/存档另列，不增加完整native或原函数等价计数。"
        "只静态读取缓存、定点PE和源码，未编译、检查、grammar读取、测试或执行算法/游戏。"
        "脚本归analysis，定点证据与临时文本归archive/2026-11/20261118_*，未改README。\n",
        encoding="utf8")
    print(json.dumps({"report": str(PREFIX.relative_to(ROOT)), "source_observed": observed,
                      "pending_hooks": 0 if connected else 1}, ensure_ascii=True))


if __name__ == "__main__":
    main()

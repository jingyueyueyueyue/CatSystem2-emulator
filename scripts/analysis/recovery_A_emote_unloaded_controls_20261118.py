"""Report a bounded KCS/PE/source contract; never evaluate the target or renderer."""
import hashlib
import json

from recovery_A_20261116 import ROOT, read_json


PREFIX = ROOT / "reports/recovery_A_20261118_emote_unloaded_controls"
CACHE = ROOT / "archive/2026-11/20261118_emote_dynamic_receivers.json"
CPU = "simulator/crates/runtime/src/emote_cpu.rs"


def source(text, anchor):
    index = text.find(anchor)
    return {"path": CPU, "anchor": anchor,
            "line": None if index < 0 else text[:index].count("\n") + 1}


def main():
    evidence = read_json(CACHE)
    inventory = read_json(ROOT / "reports/kcs_call_inventory_20261116.json")
    game = next(game for game in inventory["games"] if "NEKO" in game["root"])
    sites = {str(index): [
        {"resource": program["resource"], "offset": call["offset"],
         "argument_bytes": call["argument_bytes"]}
        for program in game["programs"] for call in program["calls"]
        if call["index"] == index
    ] for index in (907, 927, 928)}
    cpu = (ROOT / CPU).read_text(encoding="utf8")
    imp = cpu.split("    fn import_m2_resource(", 1)[1].split("    fn command(", 1)[0]
    helper = cpu.split("    fn apply_unloaded_control_requests(", 1)[1].split(
        "    fn tint_argb(", 1)[0]
    enable = cpu.split("EmoteOperation::EnableControls => {", 1)[1].split(
        "EmoteOperation::ColorPercentages", 1)[0]
    banks = cpu.split("EmoteOperation::ControllerA | EmoteOperation::ControllerB => {", 1)[1].split(
        "EmoteOperation::SurfaceFade", 1)[0]
    observed = {
        "empty_wrapper_907_request_staged": "state.unloaded_control_requests[0] = Some(command.data1)" in enable,
        "empty_wrapper_927_928_requests_staged": "state.unloaded_control_requests[index] = Some(command.data1)" in banks,
        "both_paths_require_no_player_and_no_owner": all(
            "state.player.is_none() && state.m2_owner.is_none()" in arm for arm in (enable, banks)),
        "helper_reuses_live_consumers": all(anchor in helper for anchor in (
            "owner.apply_907(raw, variables, current)?", "owner.set_blink_enable(raw, variables)",
            "owner.set_lip_enable(raw, variables)")),
        "first_owner_import_hook": "apply_unloaded_control_requests(" in imp,
        "loaded_missing_bank_diagnostic_retained": all(anchor in cpu for anchor in (
            "actual profile controller banks/scene override must be bound after resource load",
            "M2 controller command requires an actually imported live owner")),
        "release_removes_CpuTask": ".remove(&task)" in cpu,
    }
    pending = [] if observed["first_owner_import_hook"] else [
        "CPU import_m2_resource: after constructing/configuring the first owner, before validation/publish, "
        "if previous_owner.is_none() read player.controller_variables() and call "
        "self.players[&task].apply_unloaded_control_requests(&mut owner,&mut player.variables,&current)?"
    ]
    contracts = {
        "170f4e/170f76/170f80": "constructor initializes persistent10c/14c/150 to1",
        "1744ce..17450d": "null60/f8 and empty bank vectors skip effects; requested raw10c is still written at174504",
        "174c2f..174ca0": "raw14c stored before scene masking and actual blink-vector traversal",
        "174cbf..174d11": "raw150 stored before scene masking and actual lip-vector traversal",
        "174dce..174e72": "recompute reads persistent requests and independently masks actual engine/transition/bank controls",
    }
    per_pe = [{"sha256": evidence["PE_sha256"], "addresses": "RVA", "contracts": contracts}]
    summary = {
        "batch": "registered empty wrapper907/927/928 requests -> first real owner -> existing driver/bank pixels",
        "actual_static_sites_per_native": {index: len(rows) for index, rows in sites.items()},
        "actual_callers_are_not_runtime_hits": True,
        "actual_preload_branch_runtime_hits": None,
        "producer_907": "sscript792597..792710: task=u32(2480+1180*i+680), raw=function arg4; only taskID!=0 is checked",
        "producer_initial_banks": "sscript791194/791263 and791382/791451 follow906; raw1/0 comes from record888 bits2/4",
        "source_observed": observed,
        "new_stored_request_receivers": 3,
        "new_complete_producer_to_pixel_chains": 0 if pending else 1,
        "full_native_or_original_function_equivalence_added": 0,
        "main_hooks_pending": pending,
        "new_model_or_full_game_scan": False,
        "compiled": False, "typechecked": False, "grammar_reader_run": False,
        "tests_run": False, "target_VM_or_pixel_algorithm_executed": False,
    }
    payload = {"summary": summary, "perPE": per_pe, "sites": sites,
        "source_sha256": hashlib.sha256((ROOT / CPU).read_bytes()).hexdigest(),
        "sources": [source(cpu, anchor) for anchor in (
            "    fn apply_unloaded_control_requests(", "EmoteOperation::EnableControls => {",
            "EmoteOperation::ControllerA | EmoteOperation::ControllerB => {")],
        "static_evidence_cache": str(CACHE.relative_to(ROOT)).replace("\\", "/"),
        "KCS_sha256": evidence["KCS_sha256"]}
    for suffix, value in ((".json", payload), ("_summary.json", {"summary": summary, "perPE": per_pe})):
        PREFIX.with_name(PREFIX.name + suffix).write_text(
            json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    status = "首次owner接点已静态观察；尚无运行验证。" if not pending else (
        "尚缺主线CPU import窄接点：首次owner完成真实配置后、现有校验/提交前，"
        "在previous_owner.is_none()下取player.controller_variables()，再调用"
        "CpuTask::apply_unloaded_control_requests(&mut owner,&mut player.variables,&current)。"
        "不重放到复用owner；当前不记完整闭链。")
    PREFIX.with_suffix(".md").write_text(
        "# A / 空wrapper的M2控制请求\n\n"
        "真实NEKO sscript有907八处、927十处、928十三处静态调用。792597..792710的907"
        "只检查R+680任务ID，raw来自函数参数4；初装的927/928则在906后按R+888的bit2/4发1/0。"
        "这些是实际调用位置，未运行测量其中装载前分支次数。\n\n"
        "固定PE c71c5b85…1adc：170f4e/76/80把wrapper10c/14c/150初始化为1；"
        "174504无driver仍保存907 raw，174c35/174cc5先保存927/928 raw再遍历真实银行。"
        "174dce重算从同三字段读取请求。CPU旧分支在无PSB时直接报错；现保存三项独立原始请求，"
        "新owner helper复用apply_907/set_blink_enable/set_lip_enable，后续仍由真实值表/PSB像素和任务释放消费。"
        "已装载PSB但缺银行的诊断保持，不造空owner或默认变量。\n\n"
        f"{status}\n\n"
        f"源码：{CPU}:{payload['sources'][0]['line']}；普通入口/完整函数等价新增0。"
        "本批仅此共享状态缺口，不重复总体剩余清单。未编译、类型检查、grammar读取、测试或执行目标/像素算法。"
        "分析脚本归scripts/analysis，定点KCS/PE与临时文本归archive/2026-11/20261118_*；未改README。\n",
        encoding="utf8")
    print(json.dumps({"report": str(PREFIX.relative_to(ROOT)), "source_observed": observed,
                      "pending_hook_count": len(pending)}, ensure_ascii=True))


if __name__ == "__main__":
    main()

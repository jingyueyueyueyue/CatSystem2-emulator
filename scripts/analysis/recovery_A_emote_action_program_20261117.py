"""Summarize one cached NEKO caller chain and fixed program contracts.

No game/model/CFG scan, compiler, grammar reader or target evaluator is run.
The PE snippets and resource-directive count were collected read-only already.
"""
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
PREFIX = ROOT / "reports/recovery_A_20261117_emote_action_program"
SOURCES = ["simulator/crates/runtime/src/" + name for name in (
    "emote_m2program.rs", "emote_m2banks.rs", "emote_m2live.rs",
    "emote_dispatch.rs", "emote_cpu.rs",
)]


def read(path):
    return json.loads((ROOT / path).read_text(encoding="utf8"))


def digest(path):
    return hashlib.sha256((ROOT / path).read_bytes()).hexdigest()


def main():
    inventory = read("reports/kcs_call_inventory_20261116.json")
    game = next(game for game in inventory["games"] if "NEKO" in game["root"])
    sites = [{"resource": program["resource"], **call} for program in game["programs"]
             for call in program["calls"] if call["index"] in (965, 1027, 1028, 1047, 1048)]
    cache = read("archive/2026-11/20261117_emote_program_pe.json")
    prior = read("reports/emote_hittest_contracts_20261117.json")
    action = read("reports/emote_action_contracts_20261117.json")
    cpu = (ROOT / SOURCES[-1]).read_text(encoding="utf8")
    host = (ROOT / "simulator/crates/desktop/src/emote_native.rs").read_text(encoding="utf8")
    owner = (ROOT / SOURCES[0]).read_text(encoding="utf8")
    banks = (ROOT / SOURCES[1]).read_text(encoding="utf8")
    descriptor = (ROOT / SOURCES[-2]).read_text(encoding="utf8")
    observed = {
        "ordinary1028_retained_tail_arm": "EmotePacking::DiscardedTailString =>" in host,
        "ordinary965_existing_record80_planner": "EmotePacking::LabelRecord80 =>" in host,
        "runtime965_semantic_operation": "Fixed(547), LabelRecord80, SelectProgram)" in descriptor,
        "CPU965_calls_live_owner": "owner.select_program_record80(record)?" in cpu,
        "input_classified_from_actual_expanded_config": "CpuM2ProgramInput::from_expanded(configs.iter().copied())" in banks,
        "populated_program_explicit_diagnostic": "M2 %time/%set program nodes and waveform evaluator are not recovered" in owner,
        "replacement_owner_hook": "owner.inherit_program_owner(" in cpu,
        "owned547_mapping": "547 => 965" in host,
    }
    pending = []
    if not observed["replacement_owner_hook"]:
        pending.append("CPU import: in the previous_owner branch call owner.inherit_program_owner(previous); no advance hook is required for the proven empty-node states")
    if not observed["owned547_mapping"]:
        pending.append("desktop owned receiver:547=>965 and copy exactly80 owned data1 bytes into buffer1; ordinary965 already has the record planner")
    summary = {
        "batch": "actual NEKO action loop already shared;965 record receiver for proven no-wave-directive configurations",
        "KCS_sites": sites,
        "new_action_queue_or_algorithm": 0,
        "new_program_record_receiver": 1,
        "new_waveform_or_pixel_algorithm": 0,
        "actual_965_argument": "task=u32(R+680), formatted string=R+900, words=u32(R+916)/u32(R+920), R=2480+1180*i",
        "words_consumed_by_receiver": "record C string only; record+72/+76 are not forwarded to175230/198730",
        "supported_program_states": ["empty label disables owner+20 without clearing or resetting owners", "nonempty label enables only after actual expanded secondary inputs prove no %time/%set; all live wave-node maps remain empty"],
        "unsupported_program_states": ["unbound/default-constructed secondary input with nonempty label", "any recognized %time/%set input with nonempty label", "label lacking NUL in the72-byte record prefix"],
        "existing_resource_evidence": {"report": "reports/emote_hittest_contracts_20261117.json", "config_entries_previously_read": prior["config_entries_read"], "recognized_time_set_directives": prior["program_time_set_directives"], "new_resource_reads": 0},
        "source_observation": observed,
        "main_hooks_pending": pending,
        "full_native_function_equivalence_added": 0,
        "native965_all_branches_complete": False,
        "compiled": False, "typechecked": False, "grammar_reader_run_this_batch": False,
        "tests_run": False, "Rust_VM_PE_game_or_pixel_evaluator_executed": False,
    }
    contracts = {
        "24d790..24d804": "native965 formats arg1 into80byte stack record, writes arg2/3 at+72/+76, emits547, continue status2",
        "239197..2391e8": "empty C string ->175250(0); nonempty C string only ->175230(string), then175250(1); no numeric tail forwarded",
        "17438a..17439e": "wrapper100+4 is rebound to secondary config68+88",
        "1981f0..1982c2": "construct empty previous/current/compiled maps, enable20=0",
        "198776..1987a8": "null config return; reset config flags; compile label; compile miss return before modifying live maps",
        "1a3e53..1a3f27": "each #section label is passed to program parser1a54a0",
        "1a5500..1a5661": "only %prefixed rows branch into recognized %time/%set program producers; other rows skip",
        "1983a6..1983cc": "missing owner or enable0 returns; otherwise iterate actual current-node map, empty map produces no driver writes",
        "175250..175264": "set owner20 when wrapper100 exists, no reset/clear",
    }
    keys = ("0x24d790", "0x239197", "0x2391ac", "0x17438a", "0x1981f0", "0x198219", "0x198730", "0x1a4980", "0x1a3db0", "0x1a3e68", "0x1a54a0", "0x1983a0", "0x175250")
    # These anchors start at established instructions. Snippets remain bounded
    # contracts, not a claim of recovering whole function boundaries or CFGs.
    payload = {"summary": summary, "perPE": [{"sha256": cache["sha256"], "addresses": "RVA", "contracts": contracts}],
        "evidence": {key: cache["spans"][key] for key in keys},
        "previous_action_evidence": {"report": "reports/emote_action_contracts_20261117.json", "sha256": digest("reports/emote_action_contracts_20261117.json"), "contracts": {key: action["evidence"][key]["sha256"] for key in ("action_select_and_saved_name_union", "skip_until_restore_flag", "capture_gate_wait_and_one_step")}},
        "sources": [{"path": path, "sha256": digest(path)} for path in SOURCES],
        "remaining": pending + ["actual wave-node compilation/evaluation and previous/current program transitions remain unsupported; the old zero-directive inventory is not full-program equivalence", "static callsites do not establish executed reachability, rendering acceptance or a completed original function"]}
    for suffix, data in ((".json", payload), ("_summary.json", {"summary": summary, "perPE": payload["perPE"]})):
        PREFIX.with_name(PREFIX.name + suffix).write_text(json.dumps(data, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    PREFIX.with_suffix(".md").write_text(
        "# A / NEKO action共享链与965实际record分支\n\n"
        "既有库存：1027@898140开始、1028@899222查询、1047@899412跳过、1048@896754清保存各1处，均为同一sscript。"
        "现有action+180已经共享选择、当前步骤、等待、saved恢复与释放；本批没有新增队列或重写该算法。1028宿主尾参数读取已静态看到主线接入。\n\n"
        "实际965@807878（16bytes）从R+680取得task，R+900取得格式化字符串，R+916/+920进入record+72/+76。"
        "24d790→239197/2391ac→175230/175250证明接收方只消费首部C字符串。174398将当前secondary+88绑定给独立program+100；不是action或spline。\n\n"
        "新增emote_m2program.rs：空标签只清enable；非空仅在实际expanded secondary配置没有%time/%set时启用。"
        "配置构造器从实际行生成证据状态，手工default保持Unbound；有有效程序directive继续明确拒绝。"
        "无directive时没有可编译波形节点，1983c2的真实map循环为空，故无需伪造float/pixels或额外tick。"
        "该范围复用此前106份配置0条directive证据，本批没有重新解包或扫描资源。\n\n"
        "runtime已接descriptor SelectProgram→CPU→live owner；info.program_enabled可观察原enable位。"
        "同owner复用/克隆自然保留。主线最窄接点：跨PSB构造替换owner时在previous_owner分支调用"
        "owner.inherit_program_owner(previous)。owned547另需547=>965并复制80bytes到buffer1；ordinary planner已存在。\n\n"
        + "当前待接：" + ("；".join(pending) if pending else "无上述静态接点缺口") + "。\n\n"
        "仍缺有%time/%set的节点编译、求值及新旧程序过渡；965全分支完成=false，完整函数等价新增0、像素算法新增0。"
        "本批未编译、类型检查、grammar读取、测试或执行Rust/VM/PE/游戏/求值器。源码归runtime，脚本归analysis，证据归日期archive；未改README或desktop/lib。\n",
        encoding="utf8")
    print(json.dumps(summary, ensure_ascii=True))


if __name__ == "__main__":
    main()

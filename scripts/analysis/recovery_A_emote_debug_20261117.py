"""Native933 text/clipboard provenance; bounded static reads, no target execution."""
from __future__ import annotations

import argparse
import json
from pathlib import Path
import re
import subprocess

from recovery_A_20261116 import ROOT, Image, read_json, sha
from recovery_A_receiver_contracts_20261116 import block

PREFIX = "recovery_A_20261117_emote_debug"
SNAPSHOT = ROOT / "archive/2026-11/20261117_emote_debug_evidence.json"
LIVE_SNAPSHOT = ROOT / "archive/2026-11/20261117_emote_debug_live_evidence.json"
DRIVER_SCOPE = ROOT / "archive/2026-11/20261117_emote_debug_driver_scope.json"
SOURCES = ["simulator/crates/runtime/src/emote_m2debug.rs",
           "simulator/crates/runtime/src/emote_m2live.rs",
           "simulator/crates/runtime/src/emote_m2values.rs"]


def probe():
    prior = read_json(ROOT / "reports/recovery_A_20261116_emote.json")
    image = Image(ROOT / prior["binary_path"], prior["PE_sha256"])
    spans = {
        "native933_global_clipboard": (0x24CBB0, 0x58),
        "manager_selector0_bank_text": (0x17EEA0, 0x539),
        "registered_values_driver94_or_owner30": (0x16E3F0, 0x127),
        "manager_signed_task_key_lookup": (0x16B770, 0x4A),
        "manager_lookup_uses_same_task_tree": (0x17D425, 0x1D),
        "name_length4000_and_conversion": (0x2A54C0, 0xAE),
        "UTF8_to_CP932_not_escape": (0x2A5570, 0xE3),
        "CF_TEXT_clipboard": (0x2A7000, 0x6A),
    }
    inventory = read_json(ROOT / "reports/kcs_call_inventory_20261116.json")
    calls = []
    for game in inventory["games"]:
        for program in game["programs"]:
            if program["resource"] != "nekopara_vol3/kcs.int::main.kcs":
                continue
            for call in program["calls"]:
                if call.get("index") == 933:
                    calls.append({"resource": program["resource"],
                        "sha256": program["sha256"], "table": program["table_hint"],
                        "program_version": program["program_version"], **call})
    if len(calls) != 1:
        raise ValueError("fixed NEKO inventory no longer contains exactly one933 call")
    literals = {}
    for name, address in {"bank": 0x5E4180, "file": 0x5E4194,
            "character": 0x5E41A4, "motion": 0x5E41B4,
            "value": 0x5E41C4, "separator": 0x5E3F44}.items():
        raw = image.get(address, 100).split(b"\0", 1)[0]
        literals[name] = {"rva": address, "bytes": raw.hex(),
                          "CP932": raw.decode("cp932")}
    imports = {f"{address:x}": image.imports.get(address) for address in
        (0x4C62EC, 0x4C62F0, 0x4C6538, 0x4C653C, 0x4C6540, 0x4C6544,
         0x4C62D8, 0x4C62DC, 0x4C62E0)}
    data = {"PE": {"path": prior["binary_path"], "sha256": image.digest},
        "evidence": {name: block(image, *span) for name, span in spans.items()},
        "literals": literals, "imports": imports, "ordinary_calls": calls,
        "inventory_source": "reports/kcs_call_inventory_20261116.json",
        "counts_are_static_calls_not_execution": True,
        "target_executed": False, "clipboard_accessed": False}
    SNAPSHOT.write_text(json.dumps(data, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    print(json.dumps({"bounded_spans": len(spans), "NEKO_static_calls": len(calls)}))


def live_evidence():
    prior = read_json(ROOT / "reports/recovery_A_20261116_emote.json")
    image = Image((ROOT / prior["binary_path"]).parent / "emotedriver.dll",
        "85eb459e94e74ab608f649eb7fb440f2526e90aaa1af7276ed01dc10018903ce")
    spans = {"virtual94_bridge": (0x2B40, 0x19),
        "base54_read_and_model_fallback": (0x133E0, 0x12B),
        "underlying_model_unqualified_and_qualified_variables": (0x337E0, 0x1B0)}
    data = {"driver": {"path": image.path.relative_to(ROOT).as_posix(), "sha256": image.digest},
        "EXE_owner_source": SNAPSHOT.relative_to(ROOT).as_posix(),
        "EXE_owner_span": "16e423..16e4f5; driver94 f32 or absent-driver current30",
        "evidence": {name: block(image, *span) for name, span in spans.items()},
        "scope": "three bounded getter spans; no new resource/CFG inventory",
        "target_executed": False}
    LIVE_SNAPSHOT.write_text(json.dumps(data, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    print(json.dumps({"bounded_live_getter_spans": len(spans)}))


def driver_scope():
    # One already established actual model and cached configs only. This is
    # static name/field provenance, not execution of the Rust owner/evaluator.
    from recovery_A_psb_20261117 import Psb, load_sample
    resource, raw = load_sample(0)
    root = Psb(raw).root
    metadata = root["metadata"]
    primary = next(row for row in read_json(ROOT /
        "archive/2026-11/20261117_emote_bank_inputs.json")["resources"]
        if row["resource"] == "ca01.inc")
    secondary = read_json(ROOT /
        "archive/2026-11/20261117_emote_pattern_query_evidence.json")["resource"]

    def patterns(sections):
        result, current, defaults = {}, None, {}
        for row in sections.get("pattern", []):
            if row[0].startswith("%"):
                current = (row[0][1:], row[1])
                result.setdefault(current, set())
                if len(row) >= 4 and row[3] == "default":
                    defaults.setdefault(current[0], []).append(current[1])
            elif current is not None:
                result[current].add(row[0])
        selected = {dimension: min(defaults.get(dimension, []) or
            [label for dim, label in result if dim == dimension])
            for dimension, _ in result}
        return result, selected

    pp, pd = patterns(primary["sections"])
    sp, sd = patterns(secondary["sections"])
    registered = set().union(*pp.values(), *sp.values())
    primary_selected = set().union(*(names for (dim, label), names in pp.items() if pd[dim] == label))
    secondary_selected = set().union(*(names for (dim, label), names in sp.items() if sd[dim] == label))
    secondary_selected.update(row[0] for row in secondary["sections"].get("defval", []))
    seed = primary_selected | secondary_selected
    outputs = {}
    for section in ("eyeControl", "eyebrowControl", "mouthControl", "transitionControl"):
        for entry in metadata.get(section, []):
            if entry.get("enabled", 1) == 0:
                continue
            outputs.setdefault(entry["label"], []).append(section + ".label")
            if section == "mouthControl" and entry.get("talkLabel"):
                outputs.setdefault(entry["talkLabel"], []).append(section + ".talkLabel")
    for entry in metadata.get("selectorControl", []):
        if entry.get("enabled", 1) != 0:
            for option in entry.get("optionList", []):
                outputs.setdefault(option["label"], []).append("selectorControl.optionList.label")
    motion_variables = [entry for obj in root["object"].values()
        for motion in obj.get("motion", {}).values() for entry in motion.get("variable", [])]
    ui_variables = metadata.get("variableList", [])
    data = {"scope": "ca01 canonical default initial owner with body.dif; not all selections or retained histories",
        "resource": resource, "decoded_sha256": sha(raw),
        "cached_primary": {"resource": primary["resource"], "sha256": primary["sha256"]},
        "cached_secondary": {"resource": secondary["name"], "sha256": secondary["decoded_sha256"]},
        "registered_name_union": sorted(registered), "registered_count": len(registered),
        "explicit_initial_name_inputs": sorted(seed),
        "registered_initial_explicit_count": len(registered & seed),
        "registered_filled_by_existing_controller_outputs": {
            name: outputs[name] for name in sorted(registered - seed) if name in outputs},
        "registered_missing_current_name_provenance": sorted(registered - seed - outputs.keys()),
        "registered_qualified_names": [name for name in sorted(registered) if "::" in name or "/" in name],
        "motion_variable_records": len(motion_variables),
        "metadata_variableList_records": len(ui_variables),
        "metadata_variableList_entry_fields": sorted({key for row in ui_variables for key in row}),
        "metadata_variableList_is_label_frame_UI_not_numeric_initial_value": True,
        "new_resolver_implemented": False,
        "reason": "all22 actual initial registered names already have explicit/current controller producers; no identity resolver or guessed UI first-frame default added",
        "static_provenance_only": True, "Rust_or_original_executed": False,
        "limitations": ["no claim that all dynamic rebindings retain this exact name set",
                        "general model unqualified/qualified mutable-map fallback remains outside this actual-input proof"]}
    DRIVER_SCOPE.write_text(json.dumps(data, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    print(json.dumps({"one_model": resource, "registered": len(registered),
        "explicit": len(registered & seed), "controller_outputs": len(registered - seed & outputs.keys()),
        "missing": data["registered_missing_current_name_provenance"]}, ensure_ascii=True))


def report():
    evidence = read_json(SNAPSHOT)
    api = (ROOT / "simulator/crates/runtime/src/emote.rs").read_text(encoding="utf8")
    cpu = (ROOT / "simulator/crates/runtime/src/emote_cpu.rs").read_text(encoding="utf8")
    values = (ROOT / "simulator/crates/runtime/src/emote_m2values.rs").read_text(encoding="utf8")
    live = (ROOT / "simulator/crates/runtime/src/emote_m2live.rs").read_text(encoding="utf8")
    dispatch = (ROOT / "simulator/crates/runtime/src/emote_dispatch.rs").read_text(encoding="utf8")
    desktop = (ROOT / "simulator/crates/desktop/src/emote_native.rs").read_text(encoding="utf8")
    clipboard = (ROOT / "simulator/crates/desktop/src/clipboard_native.rs").read_text(encoding="utf8")
    small = {"summary": {
        "batch": "933 selector0 live-bank CP932 debug text; clipboard provenance correction",
        "NEKO_actual_static_calls": len(evidence["ordinary_calls"]),
        "argument_bytes": 0, "native_return_status": 2,
        "corrected_semantics": "global M2 manager debug text -> Windows CF_TEXT clipboard; not binary save-state",
        "source": "emote_m2live::CpuM2DebugText::{default,insert_bank,into_cp932}",
        "input_contract": "selector0 source-vector/UTF8 character/UTF8 motion; registered names with actual f32 driver samples, owner+30 only without driver",
        "output_contract": "signed task-id order, first CP932 resource, UTF8->CP932 names, preconversion name order, CRLF and six fractional digits; no trailing NUL",
        "empty_output_contract": "skip clipboard publication entirely; do not empty the existing clipboard",
        "source_formatter_implemented": True,
        "owner_snapshot_API": "CpuM2LiveOwner::debug_registered_values(Option<&BTreeMap<String,f64>>)->Vec<(String,f32)>",
        "snapshot_lifetime": "reads current retained value tree; owned UTF8/f32 pairs; no owner mutation or defaults substitution",
        "backend_trait_hook_observed": "fn debug_m2_text_cp932(&self) -> Result<Vec<u8>>" in api,
        "service_forwarding_hook_observed": "pub fn debug_m2_text_cp932(&self) -> Result<Vec<u8>>" in api
            and "self.backend.as_ref().ok_or(EmoteError::MissingBackend)?.debug_m2_text_cp932()" in api,
        "CPU_formatter_call_observed": "CpuM2DebugText::default(" in cpu,
        "CPU_live_snapshot_call_observed": "owner.debug_registered_values(current.as_ref())" in cpu
            and ".map(CpuPlayer::controller_variables).transpose()?" in cpu,
        "CPU_model_variable_fallback_guard_observed": all(value in cpu for value in
            ("registered.iter().any(|(name, _)| !current.contains_key(name))",
             "M2 debug registered name requires the model-variable fallback getter")),
        "value_snapshot_hook_observed": "debug_registered_values(" in values,
        "owner_snapshot_hook_observed": "pub fn debug_registered_values(" in live,
        "descriptor_933_manager_debug_observed": re.search(
            r"index:\s*933,\s*wrapper_rva:\s*0x24cbb0,\s*argument_bytes:\s*0,\s*"
            r"reply:\s*Continue,\s*shape:\s*EmoteDispatchShape::Manager,\s*"
            r"operation:\s*EmoteOperation::DebugBankText", dispatch) is not None,
        "desktop_933_host_hook_observed": all(value in desktop for value in
            ("if index == 933", "state.debug_m2_text_cp932()", "publish_cp932_clipboard(933, &bytes)")),
        "desktop_empty_text_skip_observed": "if !bytes.is_empty() { self.publish_cp932_clipboard(933, &bytes)?; }" in desktop,
        "shared_CF_TEXT_host_observed": "fn publish_cp932_clipboard(" in clipboard and "write_windows_clipboard(1, 1," in clipboard,
        "Android_host_queue_observed": "Cp932.decode(bytes)" in clipboard and "pending.push_back(text.encode_utf16().collect())" in clipboard,
        "old_blocked_933_observed": "blocked!(933," in dispatch,
        "A_modified_CPU_dispatch_desktop": False,
        "A_modified_value_table_for_live_snapshot": True,
        "remaining": [
            "main CPU selector0 collection/identity conversion until statically observed",
            "main host clipboard service and933 dispatch; no host clipboard was read or modified",
            "general mutable model unqualified/qualified fallback is unresolved; bounded initial producer-name coverage is not a complete driver-map implementation",
            "CP932 Windows best-fit/unencodable conversion and legacy CRT rounding edge equivalence not claimed",
            "whole-project final validation; no native equivalence or complete NEKO pixel claim"],
        "compiled": False, "typechecked": False, "tests_run": False,
        "Rust_or_original_executed": False, "completed_native_equivalence_added": 0},
        "perPE": [{"sha256": evidence["PE"]["sha256"],
            "anchors_rva": [f"{row['start_rva']:x}" for row in evidence["evidence"].values()],
            "evidence_path": SNAPSHOT.relative_to(ROOT).as_posix()}]}
    if all(small["summary"][key] for key in ("backend_trait_hook_observed",
            "service_forwarding_hook_observed", "CPU_formatter_call_observed",
            "CPU_live_snapshot_call_observed", "value_snapshot_hook_observed", "owner_snapshot_hook_observed")):
        small["summary"]["remaining"].pop(0)
    if small["summary"]["desktop_933_host_hook_observed"] and small["summary"]["shared_CF_TEXT_host_observed"] \
            and not small["summary"]["old_blocked_933_observed"]:
        small["summary"]["remaining"] = [row for row in small["summary"]["remaining"]
            if not row.startswith("main host clipboard service")]
    scope = read_json(DRIVER_SCOPE) if DRIVER_SCOPE.exists() else None
    if scope:
        small["summary"]["actual_initial_driver_scope"] = {key: scope[key] for key in
            ("scope", "registered_count", "registered_initial_explicit_count",
             "registered_filled_by_existing_controller_outputs", "registered_missing_current_name_provenance",
             "registered_qualified_names", "new_resolver_implemented", "static_provenance_only")}
        small["summary"]["actual_initial_driver_scope"].update({
            "registered_names_with_current_producers": scope["registered_count"]
                - len(scope["registered_missing_current_name_provenance"]),
            "runtime_driver_values_observed": False,
            "general_driver_fallback_implemented": False,
            "coverage_not_extrapolated_to_all14_profiles": True,
        })
    if LIVE_SNAPSHOT.exists():
        live_evidence_data = read_json(LIVE_SNAPSHOT)
        small["perPE"].append({"sha256": live_evidence_data["driver"]["sha256"],
            "anchors_rva": [f"{row['start_rva']:x}" for row in live_evidence_data["evidence"].values()],
            "evidence_path": LIVE_SNAPSHOT.relative_to(ROOT).as_posix()})
    prefix = ROOT / "reports" / PREFIX
    prefix.with_name(PREFIX + "_summary.json").write_text(json.dumps(small, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    full = {**small, "ordinary_calls": evidence["ordinary_calls"],
        "actual_driver_scope": scope,
        "literals": evidence["literals"], "imports": evidence["imports"],
        "source_hashes": {path: sha((ROOT / path).read_bytes()) for path in SOURCES},
        "evidence_sha256": sha(SNAPSHOT.read_bytes())}
    prefix.with_suffix(".json").write_text(json.dumps(full, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    text = """# A / 933实际银行文本与剪贴板

更正旧`global-save-state`/binary序列化解释：固定NEKO SHA c71c5b85…5b11adc，24cbb0两次调用17eea0估长/生成文本，再交2a7000。后者IAT明确为OpenClipboard、EmptyClipboard、SetClipboardData(CF_TEXT=1)、CloseClipboard。返回native status2，不写VM数值结果；全局manager不存在或文本长度0时跳过剪贴板。原版不生成二进制存档。

既有KCS清单中NEKO的`nekopara_vol3/kcs.int::main.kcs`（SHA f526c98e…c7e9466），offset5459有一处ordinary933、0参数调用；这是静态存在，不代表已执行或启动必经。没有重扫资源/CFG。

17eea0只枚举每task的selector0；source vector、character、motion任一缺失就略过，仅打印source第一项。16b770证明task key有符号排序。16e3f0从真实value-table登记树取名，driver存在时查virtual94当前f32，否则用value+30；不是配置初值，也不是所有PSB变量。原UTF-8名顺序必须在转码前保留。2a54c0仅在长度>4000时清空，2a5570调用MultiByteToWideChar(65001)→WideCharToMultiByte(932)，没有CSV转义。

已新增实际文本构造`runtime/emote_m2debug.rs`，并由m2live内部声明、重导出`CpuM2DebugText`。`insert_bank(id,&source_names,character_utf8,motion_utf8,registered_values)`持有快照；`into_cp932()->Result<Vec<u8>>`输出原日文栏目、CRLF、六位小数和bank分隔空行，不含尾NUL。空串必须保持旧剪贴板，不做清空。有限值、可编码名有具体字节消费；Windows best-fit及旧CRT特殊舍入边缘未宣称等价，非有限值继续诊断。formatter和快照本身不作完整函数等价证明；本批没有实际操作宿主剪贴板。

本批快照已实际落于m2values及m2live：

```rust
let rows: Vec<(String,f32)> = owner.debug_registered_values(Some(&current_driver));
//无driver才传None：读取同一owner每条live current_30。
dump.insert_bank(task_id, &resources, character_utf8, motion_utf8, rows)?;
```

owner已有同名转发，值表直接遍历self.values，不从当前config重新造名，也不漏旧profile留下的登记项；每项Some(driver)读当前值并窄化f32，map缺名0，None读当前value+30。没有乘speed/weight、加offset、取chain末目标或回退配置初值；读取不清dirty/chain、不推进clock，返回独立拥有的String/f32。CPU按现存players收selector0，用player.controller_variables取得真实样本，resource_name给单元素vector，m2_player_name由CP932解成内部UTF-8（勿给profile alias），m2_timeline直接给UTF-8。无owner不造银行。A未改CPU/trait/dispatch/desktop。

getter限定：driver2b40→133e0先查base map54，miss经337e0查底层unqualified map2b0或qualified map2b8，只有最终miss在33981返回0。因此输入map应代表完整当前driver查询结果；不能将只含部分override的字典误称完整driver。CPU现对登记名缺current key明确返回933 model-variable-fallback诊断，不发布缺名0；该guard由summary静态观察，不是通用fallback已恢复。独立timeline差分仍不加入933。A未改CPU消费者；新归档仅三段getter，没有重扫14inc/135PSZ/CFG。

主线只在非空输出时请求宿主写剪贴板并补NUL。未知宿主clipboard继续明确诊断，不以日志/返回0替代成功。清空旧内容属于原版宿主写入步骤，不在A生成器内执行。

主线集成现按trait/service、CPU实时快照、933 Manager/DebugBankText描述表、desktop和共用clipboard源码静态观察，已存在的接点不再保留“待接”条目。desktop先拦933，非空才发布；Windows复用CF_TEXT，Android进入既有pending队列。Android入队也不当作已执行系统写入。具体observed见summary，A不覆写主线集成报告。

当前主线是否接入由summary静态读取判断，observed不代表执行。全函数等价新增0。脚本归scripts/analysis，固定八段PE及IAT/字面证据归archive/2026-11/20261117_emote_debug_evidence.json，报告归reports；没有新目录、README/译文/游戏修改。未编译、类型检查、测试、执行Rust/PE/VM/游戏或操作剪贴板。
"""
    if scope:
        text += ("\n定点driver底层输入：只读已知ca01_l_body_1.psz及缓存ca01.inc/body.dif。"
            f"当前canonical默认初装的{scope['registered_count']}个登记名中，"
            f"{scope['registered_initial_explicit_count']}个有实际初始差分输入，"
            "余下face_talk由已有mouthControl.talkLabel输出进入controller_variables；缺生产者名0，限定名0。"
            f"模型所有motion.variable均为空；metadata.variableList有{scope['metadata_variableList_records']}条，"
            "每条只有label/frameList，不能把首帧或范围起点当数值默认。"
            "因此本次实际初装登记树22/22已有当前值来源，不新增无补值作用的resolver。"
            "这是源码名集合覆盖证据，不是执行快照；不外推全14profile、所有动态选择或跨模型历史。"
            "通用2b0/2b8可变map与限定名fallback仍保留范围，不能继续将任意部分map的缺名0叫完整driver。"
            "静态定点证据归archive/2026-11/20261117_emote_debug_driver_scope.json。\n")
    prefix.with_suffix(".md").write_text(text, encoding="utf8")
    current_path = ROOT / "reports/recovery_A_20261117_emote_current_summary.json"
    current = read_json(current_path)
    current["debug_text_batch"] = small
    current["summary"]["latest_A_batch"] = small["summary"]
    relative = prefix.with_suffix(".md").relative_to(ROOT).as_posix()
    if relative not in current.setdefault("reports", []):
        current["reports"].append(relative)
    current_path.write_text(json.dumps(current, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    rolling_path = ROOT / "reports/recovery_A_20261117_rolling_delivery.md"
    rolling = rolling_path.read_text(encoding="utf8")
    note = ("交付`" + PREFIX + ".md`：933已证实为selector0实时银行文本→CF_TEXT剪贴板，"
            "纠正旧binary save-state误判；m2debug实现CP932字节构造，m2live/m2values已有同名debug_registered_values读取真实登记树及driver/current30。"
            "CPU/trait/service/933描述表及desktop共享clipboard接点以summary源码observed为准，已接部分不再列待接；"
            "ca01默认初装22/22登记名有当前生产者，通用动态model fallback仍未恢复，不含timeline差分。"
            "1处真实静态KCS调用，不视为运行；未增完整native等价计数。\n\n")
    rolling = re.sub(r"交付`" + PREFIX + r"\.md`：[^\n]*\n\n", "", rolling)
    rolling_path.write_text(rolling.replace("# A / 本轮小批交付\n\n", "# A / 本轮小批交付\n\n" + note, 1), encoding="utf8")
    print(json.dumps(small["summary"], ensure_ascii=True))


def syntax():
    rows = []
    for path in SOURCES:
        raw = (ROOT / path).read_bytes()
        run = subprocess.run([str(Path.home() / ".cargo/bin/rustfmt.exe"), "--edition", "2024",
            "--config", "skip_children=true", "--emit", "stdout"], input=raw,
            stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, check=False)
        rows.append({"path": path, "sha256": sha(raw), "grammar_exit": run.returncode,
                     "diagnostic": run.stderr.decode("utf8", "replace")})
    (ROOT / "archive/2026-11/20261117_emote_debug_source_syntax.json").write_text(
        json.dumps({"grammar_only": True, "compiled": False, "typechecked": False,
                    "tests_run": False, "sources": rows}, indent=2) + "\n", encoding="utf8")
    print(json.dumps({"grammar_sources": len(rows), "failures": sum(row["grammar_exit"] != 0 for row in rows)}))


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--probe", action="store_true")
    parser.add_argument("--report", action="store_true")
    parser.add_argument("--syntax", action="store_true")
    parser.add_argument("--live-evidence", action="store_true")
    parser.add_argument("--driver-scope", action="store_true")
    args = parser.parse_args()
    if args.probe: probe()
    if args.live_evidence: live_evidence()
    if args.driver_scope: driver_scope()
    if args.report: report()
    if args.syntax: syntax()

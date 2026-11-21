"""Fixed NEKO926/534 output query: bounded bytes and config parser provenance."""
from __future__ import annotations

import argparse
import ast
import json
import re
import struct

from recovery_A_20261116 import ROOT, Image, read_json, sha
from recovery_A_receiver_contracts_20261116 import block
from audit_runtime_evidence import kif_entries, blowfish_le_decrypt
from recovery_A_emote_host_20261117 import sections

SNAPSHOT = ROOT / "archive/2026-11/20261117_emote_pattern_query_evidence.json"
PREFIX = "recovery_A_20261117_emote_pattern_query"


def probe():
    identity = read_json(ROOT / "reports/recovery_A_20261116_emote.json")
    image = Image(ROOT / identity["binary_path"], identity["PE_sha256"])
    spans = {
        "native926_output_resolver": (0x24CD80, 0x39),
        "command534": (0x238C96, 0x22),
        "manager_output_query": (0x17DD00, 0x154),
        "config_default_pattern_enumeration": (0x187400, 0x300),
        "primary_default_selection": (0x1727ED, 0x79),
        "secondary_default_selection": (0x1735B2, 0x2B),
        "pattern_node_insert_default_copy": (0x18EE60, 0x210),
        "pattern_node_allocate_copy": (0x190130, 0x130),
    }
    literals = []
    config_code = image.get(0x187000, 0x6000)
    for section in image.sections:
        for match in re.finditer(b"default\x00", section["data"]):
            rva = section["rva"] + match.start()
            refs = [0x187000 + found.start() for found in
                    re.finditer(re.escape(struct.pack("<I", image.base + rva)), config_code)]
            if refs:
                literals.append({"rva": rva, "config_pointer_references": refs})
                for ref in refs:
                    before = image.get(ref - 0x800, 0x800)
                    offset = before.rfind(bytes.fromhex("558bec"))
                    if offset < 0:
                        raise ValueError("pattern parser prologue requires explicit recovery")
                    start = ref - 0x800 + offset
                    spans["pattern_parser_default_producer"] = (start, ref - start + 0x260)
    archive = image.path.parent / "m2config.int"
    entries, _, cipher = kif_entries(archive, 0x0F6BB0AE)
    entry = next(row for row in entries if row["name"] == b"body.dif")
    with archive.open("rb") as handle:
        handle.seek(entry["offset"])
        raw = handle.read(entry["size"])
    raw = blowfish_le_decrypt(cipher, raw) if cipher else raw
    config = sections(raw)
    include_entry = next(row for row in entries if row["name"] == b"ca01.inc")
    with archive.open("rb") as handle:
        handle.seek(include_entry["offset"])
        include_raw = handle.read(include_entry["size"])
    include_raw = blowfish_le_decrypt(cipher, include_raw) if cipher else include_raw
    cached = read_json(ROOT / "reports/recovery_A_20261117_emote_host.json")["resources"]
    payload = {"PE": {"path": identity["binary_path"], "sha256": image.digest},
        "evidence": {key: block(image, *span) for key, span in spans.items()},
        "delimiters": {"comma_9e4164": image.get(0x5E4164, 2).hex(),
                       "sentinel_9e4150": image.get(0x5E4150, 2).hex()},
        "default_literals_and_config_refs": literals,
        "resource": {"name": "body.dif", "decoded_sha256": sha(raw),
            "archive_offset": entry["offset"],
            "sections": {key: config.get(key, []) for key in ("include", "dif", "pattern")}},
        "constructor_actual_inputs": {
            "ca01.inc": {"decoded_sha256": sha(include_raw), "archive_offset": include_entry["offset"],
                "sections": {key: sections(include_raw).get(key, []) for key in ("include", "dif", "pattern")}},
            **{name: {"decoded_sha256": cached[name]["decoded_sha256"],
                "sections": {key: cached[name]["sections"].get(key, []) for key in ("include", "dif", "pattern")}}
                for name in ("ca_m.dif", "ca_l.dif")}},
        "compiled": False, "typechecked": False, "tests_run": False, "target_executed": False}
    SNAPSHOT.write_text(json.dumps(payload, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    print(json.dumps({"anchors": list(spans), "delimiters": payload["delimiters"],
        "resource": payload["resource"]["name"]}, ensure_ascii=True))


def report():
    evidence = read_json(SNAPSHOT)
    config = evidence["resource"]["sections"]
    dimensions = sorted({int(row[0]) - 1 for row in config["dif"]})
    headers = [row for row in config["pattern"] if row[0].startswith("%")]
    flags = {}
    for row in headers:
        flags.setdefault(int(row[0][1:]) - 1, {}).setdefault(row[1], len(row) > 3 and row[3] == "default")
    labels = []
    for dimension in dimensions:
        patterns = sorted(flags.get(dimension, {}), key=lambda label: label.encode("cp932"))
        labels.append(next((label for label in patterns if flags[dimension][label]),
                           patterns[0] if patterns else "$"))
    def compare(inputs):
        declared = set()
        flags, old, repeated = {}, {}, 0
        for source in inputs:
            declared.update(int(row[0]) - 1 for row in source["dif"])
            for row in source["pattern"]:
                if not row[0].startswith("%"):
                    continue
                dimension = int(row[0][1:]) - 1
                patterns = flags.setdefault(dimension, {})
                if row[1] in patterns:
                    repeated += 1
                patterns.setdefault(row[1], len(row) > 3 and row[3] == "default")
                if any(word.lower() == "default" for word in row[3:]):
                    old.setdefault(dimension, row[1])
        changes, empty, fallback = [], 0, 0
        for dimension in sorted(declared):
            patterns = sorted(flags.get(dimension, {}), key=lambda word: word.encode("cp932"))
            chosen = next((word for word in patterns if flags[dimension][word]), patterns[0] if patterns else "$")
            empty += not patterns
            fallback += bool(patterns and not any(flags[dimension].values()))
            if old.get(dimension, "$") != chosen:
                changes.append({"dimension": dimension, "old": old.get(dimension, "$"), "registered": chosen})
        return {"registered_dimensions": len(declared), "pattern_headers_unique": sum(map(len, flags.values())),
            "duplicate_pattern_headers": repeated, "empty_dimensions": empty, "nonempty_first_key_fallback_dimensions": fallback,
            "multiple_default_dimensions": sum(sum(values.values()) > 1 for values in flags.values()),
            "unregistered_pattern_dimensions": sorted(set(flags) - declared), "default_selection_changes": changes}
    actual = evidence["constructor_actual_inputs"]
    if actual["ca01.inc"]["sections"]["include"]:
        raise ValueError("bounded primary include needs its actual nested input before reporting")
    comparisons = {name: compare([actual["ca01.inc"]["sections"], actual[name]["sections"]])
                   for name in ("ca_m.dif", "ca_l.dif")}
    comparisons["body.dif"] = compare([config])
    source_paths = ["simulator/crates/runtime/src/emote_m2profile.rs",
                    "scripts/analysis/recovery_A_emote_pattern_query_20261117.py"]
    cpu_path = ROOT / "simulator/crates/runtime/src/emote_cpu.rs"
    dispatch_path = ROOT / "simulator/crates/runtime/src/emote_dispatch.rs"
    cpu = cpu_path.read_text(encoding="utf8")
    dispatch = dispatch_path.read_text(encoding="utf8")
    desktop_path = ROOT / "simulator/crates/desktop/src/emote_native.rs"
    desktop = desktop_path.read_text(encoding="utf8")
    source_checks = {"CPU_actual_owner_query": "owner.secondary_default_pattern_query_cp932()" in cpu,
        "CPU_empty_without_owner": "EmoteOperation::QuerySecondaryPatterns" in cpu and "None => Vec::new()" in cpu,
        "single534_string_output": "single!(926, 0x24cd80, 8, ByteString, Fixed(534), StringOutput" in dispatch,
        "both_profile_constructors": desktop.count("CpuM2ControllerConfig::from_expanded_registered_defaults(") >= 2,
        "ordinary_and_owned_ByteString": desktop.count("EmoteReply::ByteString(mut bytes)") >= 2,
        "StringOutput_packing": "EmotePacking::StringOutput" in desktop}
    old = read_json(ROOT / "reports/recovery_A_20261116_emote.json")
    callsites = next(row["callsites"] for row in old["priority_contracts"] if row["index"] == 926)
    small = {"summary": {
        "batch": "926/534 registered secondary-default output query",
        "native926_static_8byte_callsites": callsites,
        "actual_resource": "body.dif", "actual_resource_dimensions": len(dimensions),
        "actual_pattern_headers": len(headers),
        "actual_marked_default_headers": sum(len(row) > 3 and row[3] == "default" for row in headers),
        "actual_derived_query_CP932": ",".join(labels),
        "actual_derived_query_bytes_without_NUL": len(",".join(labels).encode("cp932")),
        "actual_derived_query_is_static_data_not_Rust_execution": True,
        "constructor_actual_configuration_comparison": comparisons,
        "source_API": "CpuM2LiveOwner::secondary_default_pattern_query_cp932() -> Result<Vec<u8>> (no trailing NUL)",
        "required_constructor": "CpuM2ControllerConfig::from_expanded_registered_defaults(expanded_configs) -> Result<Self>",
        "query_source": "secondary_config (+68), registered dimensions/patterns/default flags; never live selected_patterns (+78)",
        "default_selection_shared_with_original_consumers": {
            "primary": "1727fb calls187400;172839..17285e appends only omitted trailing dimensions",
            "secondary": "1735d3..1735d8 calls187400 only for an empty requested vector",
            "explicit_dollar_selection_preserved": True,
            "constructor_change_is_query_only": False,
            "constructor_correction_required": False},
        "empty_owner_or_config_if_manager_exists": "CPU supplies empty bytes; original clears output first",
        "absent_task_manager": "handler leaves destination untouched",
        "scope_modified": [source_paths[0]],
        "CPU_helper_call_present": "secondary_default_pattern_query_cp932(" in cpu,
        "old_926_blocked_descriptor_present": "blocked!(926," in dispatch,
        "main_integration_source_checks": source_checks,
        "main_integration_source_connected": all(source_checks.values()),
        "ordinary_routes_added_by_A": 0, "completed_native_equivalence_added": 0,
        "remaining": [*([] if all(source_checks.values()) else ["main ordinary926/owned534 writeback integration not all statically observed"]),
                      "runtime query still requires final whole-project validation; no execution or full906/Emote equivalence claimed"],
        "compiled": False, "typechecked": False, "tests_run": False, "target_executed": False},
        "perPE": [{"sha256": evidence["PE"]["sha256"],
            "anchors_rva": ["24cd80", "24cd8d", "238c96", "17dd00", "17dd36", "17dd79", "17dda4",
                            "17dddf", "187470", "187480", "1874e1", "187670", "188980", "188d08", "188d29",
                            "1727fb", "172839", "17285e", "1735d3", "1735d8"],
            "evidence_path": SNAPSHOT.relative_to(ROOT).as_posix()}]}
    prefix = ROOT / "reports" / PREFIX
    prefix.with_name(PREFIX + "_summary.json").write_text(json.dumps(small, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    full = {**small, "resource": evidence["resource"], "dimension_labels": list(zip(dimensions, labels)),
        "constructor_actual_inputs": actual,
        "sources": [{"path": path, "sha256": sha((ROOT / path).read_bytes())} for path in source_paths],
        "evidence_sha256": sha(SNAPSHOT.read_bytes()),
        "prior_callsite_evidence": "reports/recovery_A_20261116_emote.json:priority_contracts[index926]"}
    prefix.with_suffix(".json").write_text(json.dumps(full, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    prefix.with_suffix(".md").write_text(
        "# A / 926与534：真实secondary默认pattern输出\n\n"
        "修正旧report：926不是输入record。NEKO固定PE c71c5b85…5b11adc，24cd8d将args+4交"
        "4772a0 resolver，data1是可写输出；238c96→17dd00(taskid,selector0,output)。17dd36先写NUL，"
        "17dd79取player+68 secondary config，再187400生成默认pattern向量，17dddf用9e4164=2c00"
        "按原序插入逗号。与当前secondary vector+78无关。原KCS清单有2处8bytes静态调用，未重扫KCS。\n\n"
        "187470按config+24的dimension树顺序取pattern map；187480找首个node+58非零标签。"
        "188980 parser在188d08精确比较header第四项小写default，188d29保存标记；不是任意后续字段或大小写无关。"
        "没有标记时，1874e1的dimension-node+34是map size：非空在187670取map第一项，空才输出9e4150=$。"
        "这不是当前选中项，也不是dimension+34存了一个默认字符串。\n\n"
        "实际body.dif SHA e3735854…945d80a：12个已注册维度、" + str(len(headers)) + "个pattern header、"
        "12个default marker，无include。静态推导输出`" + ",".join(labels) + "`（不含NUL共23bytes），"
        "不是执行Rust得到的验收结果。\n\n"
        "源码只改m2profile。新增公共构造器`CpuM2ControllerConfig::from_expanded_registered_defaults(expanded)`，"
        "复用现有完整解析器，再保留每个pattern的精确第四项flag、重复key首定义，按字节key选择第一个flag；"
        "注册的空dimension保留，未注册dimension的header按188c97忽略。"
        "主线primary/secondary两处均已改用该构造器，不能以旧constructor丢失的多marker信息替代。\n\n"
        "实际constructor差异：ca_m.dif/ca_l.dif均include同一个ca01.inc，6维共44个pattern，无default marker；"
        "原187400对每维取字节树第一key=0，因此新构造器将旧$默认修正为6个0。body.dif的12维均为1，未变。"
        "这三组均无重复header、多default、空dimension或未注册dimension的header；未为无命中边界扩消费者。\n\n"
        "已核对主选择消费者：primary在1727fb调用同一个187400，172839..17285e只把请求缺少的尾部维度追加；"
        "secondary在1735d3判断请求向量为空后，1735d8也调用187400。故无marker的ca_m/ca_l默认0是原装载/选择语义，"
        "并非为了926查询而改写主选择；显式传入$仍保留，不改成0。此次确认无需修改Rust或回退canonical constructor。\n\n"
        "实际接收API：`CpuM2LiveOwner::secondary_default_pattern_query_cp932() -> Result<Vec<u8>>`，"
        "输出CP932原字节、无尾NUL，由主线ordinary926/owned534写回。"
        "owner/config缺失但task manager存在：返回空串，仍写结尾NUL；task manager根本不存在：handler不触碰output。"
        "也可直接对canonical config调用`registered_default_pattern_query_cp932()`。"
        + ("主线当前CPU owner查询/无owner空串、Single534 StringOutput、primary/secondary canonical构造、"
           "ordinary与owned ByteString写回六项源码静态条件均存在。" if all(source_checks.values()) else
           "主线接收链仍有源码静态条件未全部满足，具体见summary.main_integration_source_checks。") +
        "A未改emote.rs/dispatch.rs/CPU/desktop，未把helper存在算完整原函数等价。\n\n"
        "脚本归scripts/analysis；固定PE、单资源临时证据归archive/2026-11/20261117_emote_pattern_query_evidence.json。"
        "无新目录、README、译文或游戏备份修改；未编译、类型检查、测试或执行Rust/PE/VM/游戏。\n",
        encoding="utf8")
    current_path = ROOT / "reports/recovery_A_20261117_emote_current_summary.json"
    current = read_json(current_path)
    current["pattern_query_batch"] = small
    current["summary"]["latest_A_batch"] = small["summary"]
    relative = prefix.with_suffix(".md").relative_to(ROOT).as_posix()
    if relative not in current.setdefault("reports", []):
        current["reports"].append(relative)
    current_path.write_text(json.dumps(current, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    rolling_path = ROOT / "reports/recovery_A_20261117_rolling_delivery.md"
    rolling = rolling_path.read_text(encoding="utf8")
    note = ("交付`" + PREFIX + ".md`：926/534改正为secondary config默认pattern输出，2处8bytes真实静态调用。"
        "共享constructor按精确第四项default/字节树序/非空首项或空$恢复metadata，owner API返回CP932逗号串；"
        "实际body.dif为12个1，ca_m/ca_l无default的6维应首key0，已修正旧$。"
        + ("主线CPU/dispatch/ordinary与owned写回已静态接通；" if all(source_checks.values()) else "主线接收链尚待静态全部接通；")
        + "A未增完整native等价计数。\n\n")
    rolling = re.sub(r"交付`" + PREFIX + r"\.md`：[^\n]*\n\n", "", rolling)
    if note not in rolling:
        rolling_path.write_text(rolling.replace("# A / 本轮小批交付\n\n", "# A / 本轮小批交付\n\n" + note, 1), encoding="utf8")
    print(json.dumps(small["summary"], ensure_ascii=True))


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--probe", action="store_true")
    parser.add_argument("--report", action="store_true")
    args = parser.parse_args()
    ast.parse(__import__("pathlib").Path(__file__).read_text(encoding="utf8"))
    if args.probe:
        probe()
    if args.report:
        report()

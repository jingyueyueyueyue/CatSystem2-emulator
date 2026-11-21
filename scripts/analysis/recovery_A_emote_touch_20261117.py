"""Bounded 1029/1030 primary #touch provenance, without runtime execution."""
from __future__ import annotations

import argparse
import json
from pathlib import Path
import re
import subprocess

from audit_runtime_evidence import blowfish_le_decrypt, kif_entries
from recovery_A_20261116 import ROOT, Image, read_json, sha
from recovery_A_receiver_contracts_20261116 import block
from recovery_A_emote_host_20261117 import sections

PREFIX = "recovery_A_20261117_emote_touch"
SNAPSHOT = ROOT / "archive/2026-11/20261117_emote_touch_evidence.json"
SOURCES = ["simulator/crates/runtime/src/" + name for name in
           ("emote_m2touch.rs", "emote_m2banks.rs", "emote_m2live.rs")]


def probe():
    prior = read_json(ROOT / "reports/recovery_A_20261116_emote.json")
    image = Image(ROOT / prior["binary_path"], prior["PE_sha256"])
    spans = {
        "native1029_count": (0x24CEB0, 0x58),
        "native1030_item_two_output_addresses": (0x24CDC0, 0xE3),
        "owned554": (0x23936A, 0x3A),
        "primary_config_lookup": (0x1758E0, 0x16),
        "color_and_table_unsigned_lookup": (0x187A40, 0x72),
        "touch_key_and_reference_producer": (0x18B490, 0x3E0),
        "named_block_rows": (0x17FD20, 0x26B),
        "row_column_bounds_and_count": (0x17FF90, 0x76),
        "named_block_exact_search": (0x1A1500, 0x96),
        "shared_integer_parser": (0x2A4DB0, 0xD4),
    }
    # Reuse the fourteen actual include names already established by main's
    # hittest inventory; do not rescan PSZ, CFGs or every config resource.
    hittest = read_json(ROOT / "reports/emote_hittest_contracts_20261117.json")
    names = {row["resource"].encode("cp932") for row in hittest["resources"]}
    archive = image.path.parent / "m2config.int"
    entries, _, cipher = kif_entries(archive, 0x0F6BB0AE)
    resources = []
    for entry in entries:
        if entry["name"] not in names:
            continue
        with archive.open("rb") as handle:
            handle.seek(entry["offset"])
            raw = handle.read(entry["size"])
        raw = blowfish_le_decrypt(cipher, raw) if cipher else raw
        config = sections(raw)
        touch = config.get("touch", [])
        declarations = []
        for row in touch:
            if row[0].startswith(("$", "#")):
                break
            if len(row) >= 2:
                declarations.append(row)
        tables = {}
        for records in config.values():
            active = None
            for row in records:
                if row[0].startswith(("$", "#")):
                    active = row[0][1:] if row[0].startswith("$") else None
                    if active is not None:
                        tables.setdefault(active, [])
                elif active is not None:
                    tables[active].append(row)
        keys = []
        for row in declarations:
            color = int(row[0], 0) & 0xFFFFFFFF
            if any(record["color"] == color for record in keys):
                continue
            keys.append({"color": color, "color_hex": f"{color:08x}",
                "tables": [{"label": name[1:], "rows": tables.get(name[1:], [])}
                           for name in row[1:] if name.startswith("$")]})
        resources.append({"resource": entry["name"].decode("cp932"),
            "archive_offset": entry["offset"], "decoded_sha256": sha(raw),
            "touch_section_occurrences": len(re.findall(rb"(?m)^\s*#touch\s*$", raw)),
            "keys": keys})
    if len(resources) != len(names):
        raise ValueError("one of the fixed fourteen actual include inputs is missing")
    action_entry = next(entry for entry in entries if entry["name"] == b"action.def")
    with archive.open("rb") as handle:
        handle.seek(action_entry["offset"])
        raw_action = handle.read(action_entry["size"])
    raw_action = blowfish_le_decrypt(cipher, raw_action) if cipher else raw_action
    action_labels = sorted({row[0][1:] for row in sections(raw_action).get("action", [])
                            if row[0].startswith("$")})
    data = {"PE": {"path": prior["binary_path"], "sha256": image.digest},
        "evidence": {name: block(image, *span) for name, span in spans.items()},
        "touch_literal": image.get(0x5E4324, 6).hex(),
        "resource_scope": "fourteen include names from existing main hittest inventory only",
        "resources": resources,
        "existing_action_consumer_source": {"resource": "action.def",
            "decoded_sha256": sha(raw_action), "labels": action_labels,
            "main_source": "simulator/crates/runtime/src/emote_m2action.rs"},
        "compiled": False, "typechecked": False,
        "tests_run": False, "Rust_or_original_executed": False}
    SNAPSHOT.write_text(json.dumps(data, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    print(json.dumps({"bounded_resources": len(resources), "anchors": len(spans)}))


def report():
    evidence = read_json(SNAPSHOT)
    rows = evidence["resources"]
    old = read_json(ROOT / "reports/recovery_A_20261116_emote.json")
    calls = {str(slot): next(row["callsites"] for row in old["priority_contracts"]
                           if row["index"] == slot) for slot in (1029, 1030)}
    table_count = sum(len(key["tables"]) for row in rows for key in row["keys"])
    row_count = sum(len(table["rows"]) for row in rows for key in row["keys"] for table in key["tables"])
    paired = sum(len(record) >= 2 for row in rows for key in row["keys"]
                 for table in key["tables"] for record in table["rows"])
    labels = set(evidence["existing_action_consumer_source"]["labels"])
    action_hits = sum(record[0] in labels for row in rows for key in row["keys"]
                      for table in key["tables"] for record in table["rows"])
    cpu = (ROOT / "simulator/crates/runtime/src/emote_cpu.rs").read_text(encoding="utf8")
    dispatch = (ROOT / "simulator/crates/runtime/src/emote_dispatch.rs").read_text(encoding="utf8")
    small = {"summary": {
        "batch": "1029/1030 primary #touch descriptor tables and actual CP932 fields",
        "actual_include_resources": len(rows),
        "actual_color_keys": sum(len(row["keys"]) for row in rows),
        "actual_ordered_tables": table_count, "actual_descriptor_rows": row_count,
        "actual_rows_with_both_output_columns": paired,
        "actual_column0_rows_matching_existing_action_labels": action_hits,
        "counts_are_static_resource_records_not_function_or_playthrough_counts": True,
        "ordinary_static_callsites": calls,
        "new_API": ["CpuM2LiveOwner::touch_descriptor_count(color:u32,table:u32)->u32",
                    "CpuM2LiveOwner::touch_descriptor_item_cp932(color:u32,table:u32,row:u32)->[Option<Vec<u8>>;2]"],
        "primary_config_source": "wrapper+64 ->config+1a0; current config is rebound and released with the actual owner",
        "item_output_contract": "independent CP932 columns, no NUL; None leaves that output untouched; success is first.is_some()",
        "missing_owner_contract": "count0, item[None,None]; existing task/manager diagnostics not broadened",
        "count_wrapper_ABI": "target/color/table, 12bytes; return count/status1; owned554(data0=color,data1=table) resolves descriptor",
        "owned554_semantics": "descriptor identity lookup; not the native1029 row-count operation; no owned554 count route claimed",
        "item_wrapper_ABI": "target/color/table/row/output0/output1,24bytes; return first-column existence/status1",
        "source_parse_connected": "touch_descriptors =" in (ROOT / SOURCES[1]).read_text(encoding="utf8"),
        "CPU_count_call_observed": "touch_descriptor_count(" in cpu,
        "CPU_item_call_observed": "touch_descriptor_item_cp932(" in cpu,
        "old_blocked_descriptors_present": [slot for slot in (1029,1030) if f"blocked!({slot}," in dispatch],
        "A_modified_CPU_dispatch_desktop": False,
        "remaining": ["main CPU/dispatch/VM two-output integration until statically observed",
                      "shared section reader cannot order the same $block across distinct #sections; explicit diagnostic remains",
                      "whole-project final validation pending; no native equivalence or game compatibility claimed"],
        "deferred_zero_hit_965_program_definitions": 0,
        "compiled": False, "typechecked": False, "tests_run": False, "Rust_or_original_executed": False,
        "completed_native_equivalence_added": 0},
        "perPE": [{"sha256": evidence["PE"]["sha256"],
            "anchors_rva": ["24ceb0", "24cdc0", "23936a", "1758e0", "187a40", "18b490", "17fd20", "17ff90", "17fff0"],
            "evidence_path": SNAPSHOT.relative_to(ROOT).as_posix()}]}
    if small["summary"]["CPU_count_call_observed"] and small["summary"]["CPU_item_call_observed"] \
            and not small["summary"]["old_blocked_descriptors_present"]:
        small["summary"]["remaining"].pop(0)
    prefix = ROOT / "reports" / PREFIX
    prefix.with_name(PREFIX + "_summary.json").write_text(json.dumps(small, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    full = {**small, "resources": rows,
        "existing_action_consumer_source": evidence["existing_action_consumer_source"],
        "source_hashes": {path: sha((ROOT / path).read_bytes()) for path in SOURCES},
        "evidence_sha256": sha(SNAPSHOT.read_bytes())}
    prefix.with_suffix(".json").write_text(json.dumps(full, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    ca = next(row for row in rows if row["resource"] == "ca01.inc")
    example = ca["keys"][0]["tables"][0]["rows"][0]
    text = (
        "# A / 1029、1030共享touch描述表\n\n"
        "固定NEKO PE SHA c71c5b85…5b11adc：24ceb0与24cdc0均发owned554；23936a→1758e0取wrapper+64的primary config，"
        "187a40按无符号颜色key及table序号取config+1a0。18b490的#touch生产者保留首个同key声明，按原token序追加$块。"
        "17fd20保留块的原行和所有字节列，17fff0返回行数，17ff90按row/column独立检查边界。\n\n"
        f"复用既有hittest清单的14份实际inc，未扫135PSZ或全部CFG：{len(rows)}份含"
        f"{small['summary']['actual_color_keys']}个颜色key、{table_count}个有序表、{row_count}行，"
        f"其中{paired}行有两列。ca01.inc的ffff0000表0首行为`{' / '.join(example)}`；"
        "表序号0为普通、1为sad的含义来自实际资源名，不是新的选择算法。静态调用1029为2处、1030为1处。"
        f"{action_hits}/{row_count}行的第一列均命中主线已实现action.def的实际标签；"
        "查询只输出原字节，由调用者再选动作，不在查询内私自触发动作或声音。\n\n"
        "实际源码：新增emote_m2touch.rs；emote_m2banks共享from_expanded直接构造同一registry，"
        "emote_m2live只加内部模块声明。查询由当前primary config消费，906初建/重绑定/释放无需第二份缓存或原进程指针。"
        "未改CPU、dispatch、desktop或M2导入/时钟。\n\n"
        "主线窄接点：`owner.touch_descriptor_count(color,table)->u32`；"
        "`owner.touch_descriptor_item_cp932(color,table,row)->[Option<Vec<u8>>;2]`。"
        "1029实参target/color/table共12bytes；1030实参target/color/table/row/output0/output1共24bytes，"
        "两个输出地址经resolver、不是格式化输入。字节无NUL，Some字段单独写bytes+NUL，None必须保持该目的串原值；"
        "1030返回第一列是否存在，即使第二列缺失也不改返回值。与926预清输出的行为不同。"
        "无owner对应count0/item[None,None]，不放宽现有找不到type54 target的诊断。\n\n"
        "本批不按描述符名自动启动动作，也不拿它作像素hittest结果；原版这里只枚举字符串。"
        "965的%time/%set已有实际0定义证据，后置。共享section reader无法恢复跨不同#section的同名$块原序，"
        "该输入继续明确诊断；当前14份每份单一#touch，没有扩零命中布局。主线接线状态见summary，未把API存在计完整函数。\n\n"
        "脚本归scripts/analysis，固定字节证据归archive/2026-11/20261117_emote_touch_evidence.json，"
        "报告归reports。未建目录、改README/译文/原游戏；未编译、类型检查、测试或执行Rust/PE/VM/游戏。\n")
    prefix.with_suffix(".md").write_text(text, encoding="utf8")
    current_path = ROOT / "reports/recovery_A_20261117_emote_current_summary.json"
    current = read_json(current_path)
    current["touch_descriptor_batch"] = small
    current["summary"]["latest_A_batch"] = small["summary"]
    relative = prefix.with_suffix(".md").relative_to(ROOT).as_posix()
    if relative not in current.setdefault("reports", []):
        current["reports"].append(relative)
    current_path.write_text(json.dumps(current, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    rolling_path = ROOT / "reports/recovery_A_20261117_rolling_delivery.md"
    rolling = rolling_path.read_text(encoding="utf8")
    note = ("交付`" + PREFIX + ".md`：1029/1030共享#touch描述表已实际从expanded config进入当前primary owner；"
        f"14份inc有42颜色key/84有序表/{row_count}行，两列独立字节输出；"
        f"第一列{action_hits}/{row_count}命中现有action.def标签。CPU/dispatch/VM写回由主线接，"
        "None必须保留目的串原值；未增完整native等价计数。\n\n")
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
    (ROOT / "archive/2026-11/20261117_emote_touch_source_syntax.json").write_text(
        json.dumps({"grammar_only": True, "compiled": False, "typechecked": False,
                    "tests_run": False, "sources": rows}, indent=2) + "\n", encoding="utf8")
    print(json.dumps({"grammar_sources": len(rows), "failures": sum(row["grammar_exit"] != 0 for row in rows)}))


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--probe", action="store_true")
    parser.add_argument("--report", action="store_true")
    parser.add_argument("--syntax", action="store_true")
    args = parser.parse_args()
    if args.probe: probe()
    if args.report: report()
    if args.syntax: syntax()

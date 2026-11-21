"""Slice one fixed Yukikoi1014 producer; --report consumes retained evidence.

Never execute KCS/native code, an evaluator, a renderer, or a compiler.
"""
import argparse
import hashlib
import json
import struct
import zlib

from audit_runtime_evidence import blowfish_le_decrypt, operand_count
from startup_inventory import ROOT, entries_at

OUT = ROOT / "archive/2026-11/20261117_efk_record_producer.json"
SHA = "797acd759f64c280fbb39725fdc413b055d56343e821d16036cc7e38516b4d95"


def probe():
    path = ROOT / "Yukikoi Melt/kcs.int"
    entries, layout, cipher = entries_at(path, 0x23A893A2)
    entry = next(row for row in entries if row["name"] == b"sscript.kcs")
    with path.open("rb") as handle:
        handle.seek(entry["offset"])
        data = handle.read(entry["size"])
    data = blowfish_le_decrypt(cipher, data) if cipher else data
    if hashlib.sha256(data).hexdigest() != SHA:
        raise ValueError("fixed sscript identity changed")
    code_size, = struct.unpack_from("<I", data, 16)
    body = zlib.decompress(data[52:])
    code = body[:code_size]
    rows, at = [], 0
    while at < len(code):
        op = code[at]
        n = operand_count(op)
        if n is None or at + 1 + 5 * n > len(code):
            raise ValueError("invalid fixed instruction")
        args = [{"flags": code[p], "value": struct.unpack_from("<I", code, p + 1)[0]}
                for p in range(at + 1, at + 1 + 5 * n, 5)]
        rows.append({"offset": at, "opcode": op, "operands": args})
        at += 1 + 5 * n
    slices = []
    for i, row in enumerate(rows[:-1]):
        if (row["opcode"] != 0x3e or row["operands"][0] not in [
                {"flags": 0x86, "value": 2620}, {"flags": 0x86, "value": 572012}]
                or row["operands"][1]["flags"] != 0x63):
            continue
        field = rows[i + 1]
        if field["opcode"] != 0x3e or field["operands"][0]["flags"] != 0x46:
            continue
        if field["operands"][1]["value"] != 8:
            continue
        end = i + 2
        while end < min(len(rows), i + 70):
            end += 1
            if rows[end - 1]["opcode"] in (0x2b, 0x2c, 0x3c):
                break
        slices.append({"record_field_expression_at": row["offset"],
                       "bank_base": row["operands"][0]["value"],
                       "instructions": rows[max(0, i - 8):end]})
    calls = [row for row in rows if row["opcode"] == 0x2a
             and row["operands"][0]["value"] == 843964]
    # Whole-record copies can produce inline+8 without an explicit field8
    # expression. Retain bounded literal-size memcpy sites, not VM evaluation.
    copies = []
    for i, row in enumerate(rows):
        if row["opcode"] != 0x2b or row["operands"] != [
                {"flags": 3, "value": 35}, {"flags": 3, "value": 12}]:
            continue
        nearby = rows[max(0, i - 20):i + 1]
        if any(arg["value"] in (2620, 1180) for old in nearby for arg in old["operands"]):
            copies.append({"call_offset": row["offset"], "instructions": nearby})
    result = {"resource": "Yukikoi Melt/kcs.int::sscript.kcs", "sha256": SHA,
              "layout": layout, "archive_entry": {k: entry[k] for k in ("offset", "size")},
              "record": {"base": 2620, "stride": 1180, "string_offset": 8,
                         "task_id_offset": 680, "loader_entry": 843964,
                         "native1014_offset": 844116},
              "loader_callers": calls, "field8_slices": slices,
              "whole_record_memcpy_candidates": copies,
              "staging_to_live_copy": {"call_offset": 733323, "bytes": 1180,
                  "destination": "2620 + 1180*u32(frame+16)",
                  "source": "572012 + 1180*u32(frame+16)"},
              "status": "static instruction slices; not VM evaluation or reachability proof"}
    OUT.write_text(json.dumps(result, ensure_ascii=True, indent=2) + "\n", encoding="utf8")
    print(json.dumps({"field8_slices": len(slices), "callers": calls,
                      "whole_record_copy_sites": [row["call_offset"] for row in copies]}))


def write_report():
    evidence = json.loads(OUT.read_text(encoding="utf8"))
    if evidence["sha256"] != SHA:
        raise ValueError("fixed KCS evidence identity changed")
    atlas_path = ROOT / "archive/2026-11/20261117_emote_owner_consumers.json"
    atlas = json.loads(atlas_path.read_text(encoding="utf8"))
    strings_path = ROOT / "archive/2026-11/20261117_kcs_a_efk_config_write.json"
    strings = {row["address"]: row["cp932"] for row in
               json.loads(strings_path.read_text(encoding="utf8"))["strings"]}
    summary = {
        "batch": "Yukikoi sole1014 call upstream expressions; fixed-first-model owner/atlas evidence",
        "fixed_KCS_count": 1, "native1014_static_sites": 1,
        "loader_internal_callers": len(evidence["loader_callers"]),
        "loader_record_indexes": {"begin_inclusive": 80, "end_exclusive": 88},
        "actual_effect_member_identified": False,
        "concrete_effect_filename": None,
        "copy_direction_correction": "30371 copies record+8 into frame+36; it is not a resource-name producer",
        "new_runtime_consumers": 0, "guards_removed": 0,
        "completed_native_equivalence_added": 0,
        "new_main_hook": None,
        "fixed_first_model_owner_counts": atlas["counts"],
        "fixed_first_model_motions": len(atlas["motions"]),
        "fixed_first_model_icons": sum(row["icon_count"] for row in atlas["textures"]),
        "no_additional_runtime_rejection_proven": True,
        "compiled": False, "typechecked": False, "tests_run": False,
        "Rust_PE_VM_game_APK_device_executed": False,
        "renderer_parameter_priority_or_texture_algorithm_executed": False,
        "new_game_wide_scan": False,
        "strict_NEKO_pixels_claimed": False,
    }
    per_pe = [{"sha256": "63de0e14cd8ce11ad2d68e7ed6076641ff5e5b73d822603cc52764009d812976",
               "role": "Yukikoi61 retained original identity; no new PE read",
               "native1014_wrapper_rva": "1ac7e0", "receiver_rva": "18c1f0"},
              {"sha256": "85eb459e94e74ab608f649eb7fb440f2526e90aaa1af7276ed01dc10018903ce",
               "role": "NEKO fixed driver bounded atlas-parser byte reads only",
               "atlas_attr_parse_rva": "2db1f..2db41",
               "atlas_rect_parse_rva": "2dc13..2dced",
               "truncated_dimension_parse_rva": "54dc1..54e03"}]
    expressions = {
        "KCS_identity": {"resource": evidence["resource"], "sha256": SHA,
                         "address_unit": "decompressed KCS code offsets, decimal"},
        "record": "R(i)=2620+1180*i",
        "native1014_at_844116": {"arg0": "u32(R(i)+680)", "arg1": "R(i)+8 (inline CP932 string address)"},
        "conditional_call_at_569425": "i=80..87; u32(R+0)!=0; (u32(R+0)&1)!=0 -> CALL843964",
        "wrapper_no_task_branch": "844020 skips load if task-id is zero",
        "copy_direction_source_evidence": {
            "VM_push_word": "script/src/lib.rs push_word writes at old_sp and increments sp",
            "VM_native_arguments": "opcode2b passes memory[old_sp-parameter_bytes..old_sp] in stored order",
            "native13": "desktop/src/lib.rs target=word0,source=word4"},
        "bounded_string_accesses": [
            {"call_offset": 30371, "native": 13, "target": "frame+36", "source": "R(u32(frame+0))+8", "role": "read for diagnostic/local copy"},
            {"call_offset": 27306, "native": 13, "target": "frame+48+8", "source": "R(u32(frame+0))+8", "role": "read into local record"},
            {"call_offset": 843494, "native": 13, "target": "R(u32(frame+0))+8", "literal_address": 682960, "literal": strings[682960], "role": "empty initialization"},
            {"call_offset": 67428, "native": 13, "target": "R(u32(frame+0))+8", "literal_address": 548609, "literal": strings[548609], "role": "empty initialization"},
        ],
        "whole_record_copy_at_733323": evidence["staging_to_live_copy"],
        "other_whole_record_copy_sources": [
            {"call_offset": 285387, "target": "R(u32(frame+100))", "source": "R(u32(frame+92))", "bytes": 1180},
            {"call_offset": 289569, "target": "R(u32(frame+100))", "source": "R(u32(frame+92))", "bytes": 1180}],
        "scope_limit": "candidate slices do not prove all aliases, scene producers, record ranges, or reachability; no VM data-flow algorithm was executed",
    }
    files = ["scripts/analysis/recovery_A_efk_producer_20261117.py",
             "scripts/analysis/recovery_A_emote_owner_consumers_20261117.py",
             "scripts/analysis/recovery_A_emote_atlas_20261117.py"]
    payload = {"schema": "cs2-recovery-A-fixed-producer/v1", "summary": summary,
        "perPE": per_pe, "expressions": expressions,
        "fallback_consumer_scope": {"resource": atlas["resource"], "sha256": atlas["sha256"],
            "parameter": "102 enabled entries; existing parameter_cursor/bound_parameter_clock consume them",
            "priority": "48 two-entry schedules; current list plus terminal boundary; existing priority_order consumes them",
            "texture": "one win/RGBA8 4096x4096 atlas;91 icon rectangles; original attr0/1 parsed to icon+4, downstream flag semantics not established here",
            "truncated_dimensions": "4095x2122 declaration does not change stored4096x4096 raw chunk; no speculative decoder change",
            "no_new_runtime_API": True},
        "remaining_minimum_inputs": [
            "concrete scene/string producer reaching R(80..87)+8, including the copied record image; a generic wrapper alone cannot select an SKFE parser/renderer rejection",
            "an actual still-rejected E-mote owner state/model path; this first-model parameter/priority/texture slice does not establish a new unsupported branch"],
        "source_files": [{"path": path, "sha256": hashlib.sha256((ROOT/path).read_bytes()).hexdigest()} for path in files],
        "evidence_files": [{"path": str(path.relative_to(ROOT)).replace('\\', '/'),
                            "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}
                           for path in [OUT, atlas_path, strings_path,
                               ROOT/"archive/2026-11/20261117_kcs_a_efk_caller_record.json",
                               ROOT/"archive/2026-11/20261117_emote_atlas_evidence.json"]]}
    prefix = ROOT / "reports/recovery_A_20261117_efk_producer"
    prefix.with_suffix(".json").write_text(json.dumps(payload, ensure_ascii=False, indent=2)+"\n", encoding="utf8")
    prefix.with_name(prefix.name+"_summary.json").write_text(
        json.dumps({"summary": summary, "perPE": per_pe}, ensure_ascii=False, indent=2)+"\n", encoding="utf8")
    prefix.with_suffix(".md").write_text(
        "# A：Yukikoi唯一1014资源实参上游\n\n"
        "本批运行时源码新增0、guard删除0、完整函数等价新增0；未编译、类型检查、测试或执行Rust/PE/VM/游戏。\n\n"
        "固定sscript SHA256="+SHA+"。以下为解压后KCS十进制偏移，不能与PE RVA混用。"
        "569425是CALL843964唯一内部调用者；569271..569629遍历80–87、检查record脏位。"
        "R(i)=2620+1180*i，844116的1014接收u32(R+680)任务号和R+8内联字符串地址；任务号0分支844020跳过load。\n\n"
        "重要纠正：30371是native13(frame+36,R+8)，27306是native13(frame+56,R+8)，均为读取，不能作为动态文件名生产者。"
        "843494与67428只把空串写入R+8。733323确实复制1180字节，从572012+1180*i到R(i)；"
        "285387/289569还会整记录互拷，因此直接field8切片不是完整别名/上游覆盖。"
        "真实EFK成员和文件名仍未静态定位，不能任选一个SKFE拒绝族实现。\n\n"
        "已按要求转到缓存首模型owner/priority/纹理：102参数全enabled，48组priority均当前列表加终止边界，"
        "91个icon属于单4096² win/RGBA8 atlas。原2db1f..2db41解析attr到icon+4；"
        "该位下游语义尚未确定，不猜旋转/alpha或擅改尺寸。既有参数、排序和纹理消费者已存在，"
        "此切片未证实新的实际拒绝；不把这点外推135模型或宣称strict像素成功。\n\n"
        "主线当前无新增挂钩。最小剩余输入是实际scene/字符串生产者到R(80..87)+8的记录内容，"
        "或一个有具体模型路径/状态的E-mote拒绝。没有扩大游戏/135PSZ扫描，也未动主线或其他代理源码。\n\n"
        "脚本分类scripts/analysis；正式JSON/小summary/本报告归reports；定点指令、atlas字段和原字节证据归"
        "archive/2026-11/20261117_*。--report仅读缓存，不重复解析资源。未改README、译文或游戏文件。\n", encoding="utf8")
    current_path = ROOT / "reports/recovery_A_20261117_emote_current_summary.json"
    if current_path.exists():
        current = json.loads(current_path.read_text(encoding="utf8"))
        current.setdefault("summary", {})["latest_yukikoi1014_producer"] = {
            "summary_path": "reports/recovery_A_20261117_efk_producer_summary.json",
            "native1014_static_sites": 1, "internal_callers": 1,
            "resource_expression": "2620+1180*i+8, i=80..87; filename unresolved",
            "read_direction_corrected": True,
            "actual_effect_member_identified": False,
            "new_runtime_consumers": 0, "completed_native_equivalence_added": 0}
        current_path.write_text(json.dumps(current, ensure_ascii=False, indent=2)+"\n", encoding="utf8")
    print(json.dumps(summary, ensure_ascii=True))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--probe", action="store_true")
    parser.add_argument("--report", action="store_true")
    args = parser.parse_args()
    if args.probe or not args.report:
        probe()
    if args.report:
        write_report()


if __name__ == "__main__":
    main()

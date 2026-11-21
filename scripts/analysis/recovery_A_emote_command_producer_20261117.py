"""Two retained NEKO command sites; KCS/PE bytes only, no VM evaluation."""
import argparse
import json
from pathlib import Path
import struct
import subprocess
import zlib

from audit_runtime_evidence import blowfish_le_decrypt, kif_entries, operand_count
from recovery_A_20261116 import ROOT, Image, read_json, sha
from recovery_A_receiver_contracts_20261116 import block

OUT = ROOT / "archive/2026-11/20261117_emote_command_producers.json"
PE = ROOT / "archive/2026-11/20261117_emote_program_pe.json"
KCS_SHA = "5554f1f99f83c63087572ec1a7fb8ddf31987cedab4185c8503a1b1e29f5011a"
PREFIX = ROOT / "reports/recovery_A_20261117_emote_action_query"
SYNTAX = ROOT / "archive/2026-11/20261117_emote_action_query_syntax.json"
SOURCES = ["simulator/crates/runtime/src/emote_dispatch.rs", "simulator/crates/runtime/src/emote_cpu.rs"]


def probe():
    identity = read_json(ROOT / "reports/recovery_A_20261116_emote.json")
    path = (ROOT / identity["binary_path"]).parent / "kcs.int"
    entries, _, cipher = kif_entries(path, 0x0F6BB0AE)
    entry = next(row for row in entries if row["name"] == b"sscript.kcs")
    with path.open("rb") as handle:
        handle.seek(entry["offset"])
        raw = handle.read(entry["size"])
    raw = blowfish_le_decrypt(cipher, raw) if cipher else raw
    if sha(raw) != KCS_SHA:
        raise ValueError("fixed NEKO sscript identity changed")
    code_length, = struct.unpack_from("<I", raw, 16)
    init_count, = struct.unpack_from("<I", raw, 24)
    body = zlib.decompress(raw[52:])
    code, at, rows = body[:code_length], 0, []
    while at < code_length:
        opcode = code[at]
        count = operand_count(opcode)
        if count is None or at + 1 + 5 * count > code_length:
            raise ValueError("invalid fixed KCS instruction")
        operands = [{"flags": code[pos], "value": struct.unpack_from("<I", code, pos + 1)[0]}
                    for pos in range(at + 1, at + 1 + 5 * count, 5)]
        rows.append({"offset": at, "opcode": opcode, "operands": operands})
        at += 1 + 5 * count
    sites = []
    for index, row in enumerate(rows):
        if row["opcode"] != 0x2b or row["operands"][0] not in (
            {"flags": 3, "value": 965}, {"flags": 3, "value": 1028}):
            continue
        sites.append({"native": row["operands"][0]["value"], "call_offset": row["offset"],
            "context": rows[max(0, index - 32):index + 12]})
    init = body[code_length:]
    strings = []
    for index in range(init_count):
        offset, address = struct.unpack_from("<II", init, index * 8)
        value = init[offset:].split(b"\0", 1)[0]
        strings.append({"address": address, "cp932": value.decode("cp932", errors="backslashreplace")})
    data = {"resource": "NEKO/kcs.int::sscript.kcs", "sha256": KCS_SHA,
        "scope": "only native965/1028 producer sites; instruction windows, not enclosing function boundaries or reachability proof",
        "sites": sites, "strings": strings, "target_executed": False}
    OUT.write_text(json.dumps(data, ensure_ascii=True, indent=2) + "\n", encoding="utf8")
    print(json.dumps({"sites": sites}, ensure_ascii=True))


def pe(spans):
    identity = read_json(ROOT / "reports/recovery_A_20261116_emote.json")
    image = Image(ROOT / identity["binary_path"], identity["PE_sha256"])
    data = read_json(PE) if PE.exists() else {"sha256": image.digest, "addresses": "RVA", "spans": {}}
    for raw in spans:
        start, size = (int(value, 0) for value in raw.split(":"))
        if not 0 < size <= 4096:
            raise ValueError("bounded span required")
        data["spans"][hex(start)] = block(image, start, size)
    handler = image.u32(0x239458 + 4 * (547 - 525)) - image.base
    data["command547_handler"] = hex(handler)
    data["spans"]["owned547"] = block(image, handler, 0x42)
    PE.write_text(json.dumps(data, ensure_ascii=True, indent=2) + "\n", encoding="utf8")
    for name, span in data["spans"].items():
        if name in (list(map(lambda s: hex(int(s.split(":")[0], 0)), spans)) + ["owned547"]):
            print(name)
            for instruction in span["instructions"]:
                print(hex(instruction["rva"]), instruction["mnemonic"], instruction["operands"])


def syntax():
    rows = []
    for path in SOURCES:
        raw = (ROOT / path).read_bytes()
        result = subprocess.run([str(Path.home() / ".cargo/bin/rustfmt.exe"), "--edition", "2024",
            "--config", "skip_children=true", "--emit", "stdout"], input=raw,
            stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        rows.append({"path": path, "sha256": sha(raw), "returncode": result.returncode,
            "stderr": result.stderr.decode("utf8", errors="replace")})
    SYNTAX.write_text(json.dumps({"method": "rustfmt stdin grammar only; sources unchanged",
        "sources": rows, "compiled": False, "typechecked": False, "tests_run": False,
        "target_executed": False}, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    print(json.dumps({"grammar_returncodes": [row["returncode"] for row in rows]}))
    if any(row["returncode"] for row in rows):
        raise SystemExit(1)


def report():
    inputs = read_json(OUT)
    # Early exploratory cache called an arithmetic opcode01 a function entry.
    # Drop those invalid navigation labels without re-reading the fixed KCS.
    for site in inputs["sites"]:
        site.pop("function_entry", None)
        site.pop("direct_callers", None)
    inputs["scope"] = "native965/1028 bounded instruction windows; no function-boundary or reachability claim"
    OUT.write_text(json.dumps(inputs, ensure_ascii=True, indent=2) + "\n", encoding="utf8")
    site = next(site for site in inputs["sites"] if site["native"] == 1028)
    evidence = read_json(PE)
    descriptor = (ROOT / SOURCES[0]).read_text(encoding="utf8")
    cpu = (ROOT / SOURCES[1]).read_text(encoding="utf8")
    host = (ROOT / "simulator/crates/desktop/src/emote_native.rs").read_text(encoding="utf8")
    vm = (ROOT / "simulator/crates/script/src/lib.rs").read_text(encoding="utf8")
    inventory = read_json(ROOT / "reports/kcs_call_inventory_20261116.json")
    neko = next(game for game in inventory["games"] if "NEKO" in game["root"])
    original_calls = [call for program in neko["programs"] for call in program["calls"]]
    spans = {key: evidence["spans"][key] for key in ("0x24cf70", "0x239332", "0x1758c0")}
    hook = """EmotePacking::DiscardedTailString => {
    let storage = vm.native_argument_storage(descriptor.argument_storage_bytes())?;
    let address = word(storage, 1)?;
    request.buffer1 = EmoteBuffer::Bytes(formatted(vm, address)?);
}"""
    observed = {
        "exact_descriptor_pop4": "single!(1028, 0x24cf70, 4, Value, Fixed(553), DiscardedTailString, QueryActionState)" in descriptor,
        "extra_storage_size_API": "pub fn argument_storage_bytes(&self) -> usize" in descriptor,
        "CPU_query_uses_actual_action_owner": "u32::from(self.m2_action_busy(task)?)" in cpu,
        "already_present_backend_actual_busy": "owner.action.busy()" in cpu,
        "VM_retained_parameter_storage_API_exists": "pub fn native_argument_storage(&self, length: usize)" in vm,
        "desktop_tail_formatting_arm_present": "EmotePacking::DiscardedTailString =>" in host,
    }
    summary = {
        "batch": "actual NEKO1028 four-byte caller -> retained extra parameter read -> live action-step query",
        "KCS_resource": "nekopara_vol3/kcs.int::sscript.kcs", "KCS_sha256": KCS_SHA,
        "KCS_call_offset_decimal": site["call_offset"],
        "NEKO1014_static_calls": sum(call["index"] == 1014 for call in original_calls),
        "NEKO1028_static_calls": sum(call["index"] == 1028 for call in original_calls),
        "argument_bytes_popped": 4, "parameter_storage_bytes_read": 8,
        "task_argument": "u32(2480 + 1180*u32(frame+0) + 680)",
        "return": "u32(wrapper180.current_step.is_some()), tested by KCS conditional at899233",
        "discarded_format_read": "storage+4 original VM DWORD -> existing formatted/expand_kcs_format; formatted bytes are discarded by native command553",
        "new_runtime_descriptor": 1, "new_CPU_command_consumer": 1,
        "new_action_algorithm": 0, "new_pixel_algorithm": 0,
        "static_source_observation": observed,
        "main_hook_pending": [] if observed["desktop_tail_formatting_arm_present"] else ["planner DiscardedTailString arm using retained parameter storage; keep checked_descriptor/pop at4bytes"],
        "ordinary_source_full_chain_claim": observed["desktop_tail_formatting_arm_present"],
        "full_native_function_equivalence_added": 0, "compiled": False,
        "typechecked": False, "tests_run": False, "target_or_evaluator_executed": False,
        "no_new_PSZ_or_game_wide_scan": True,
        "deferred965": "actual producer807878; no-wave-directive record branch is separately implemented in recovery_A_20261117_emote_action_program; populated program evaluator remains unsupported",
    }
    if SYNTAX.exists():
        parsed = read_json(SYNTAX)
        summary["source_grammar_only"] = {"returncodes": [row["returncode"] for row in parsed["sources"]],
            "matches_current_sources": all(row["sha256"] == sha((ROOT / row["path"]).read_bytes()) for row in parsed["sources"])}
    per_pe = [{"sha256": evidence["sha256"], "addresses": "RVA",
        "contracts": {"24cf70..24cfcd": "initial return0; global guard; read and format args+4; discard string; dispatch553; return receiver boolean/status1",
            "239332..239368": "initialize output0; actual task selector0 lookup;1758c0 output",
            "1758c0..1758d2": "null wrapper180 returns0, otherwise current-step pointer+8!=0"}}]
    payload = {"summary": summary, "perPE": per_pe, "KCS_site": site,
        "main_planner_patch": hook, "evidence": spans,
        "remaining": ([] if observed["desktop_tail_formatting_arm_present"] else ["main owns desktop planner/lib wiring; the retained-tail formatting arm is still pending"]) + [
            "extra formatting errors must propagate; do not substitute0, ignore the access, add an argument or change KCS pop count",
            "actual action wait/capture/start/skip/release still use the existing shared owner; no new renderer or autonomous animation",
            "static one call is not executed reachability, full-function equivalence or playthrough validation"],
        "source_hashes": [{"path": path, "sha256": sha((ROOT / path).read_bytes())}
            for path in SOURCES + ["scripts/analysis/recovery_A_emote_command_producer_20261117.py"]]}
    for suffix, data in ((".json", payload), ("_summary.json", {"summary": summary, "perPE": per_pe})):
        PREFIX.with_name(PREFIX.name + suffix).write_text(json.dumps(data, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    PREFIX.with_suffix(".md").write_text(
        "# A / 实际1028查询调用链\n\n"
        "既有库存中NEKO没有1014调用；1014历史命中来自Yukikoi/type61，本批未混入Emote。"
        "转NEKO sscript的真实1028：KCS SHA5554f1f9…5011a，offset899222，声明4bytes；"
        "target来自u32(2480+1180*u32(frame+0)+680)，899233直接测试返回DWORD。"
        "未重扫游戏/模型，未扩blink/裁剪/FIFO。\n\n"
        "固定NEKO PE SHAc71c5b85…1adc：24cf9a读取参数后一个DWORD并格式化到临时全局缓冲，"
        "结果随后被丢弃；24cfb3实际派发553。239332清输出后做task/selector0查找，1758c0只测试"
        "wrapper+180的current-step指针+8，空owner为0。它不是timeline忙碌或步数。\n\n"
        "已改runtime/emote_dispatch.rs：1028保留4bytes弹栈，新增DiscardedTailString，"
        "argument_storage_bytes()返回8；只接有owned格式化bytes的包。CPU新增QueryActionState，"
        "复用已有m2_action_busy→owner.action.busy，不推进时钟／消费队列。start/skip/clear/释放继续用"
        "同一个真实owner，不新增替代动画或恒定返回值。\n\n"
        "主线最窄接点在desktop/emote_native.rs::plan的packing match，仍校验args.len()==4：\n\n"
        "```rust\n" + hook + "\n```\n\n"
        "利用现有VM native_argument_storage，读取原本参数区后的真实VM字，不补0、不改弹栈长度；"
        "格式化失败沿原宿主错误传播。" + ("当前静态源码已看到主线合入该planner分支；这是源码接通观察，不是运行验收。" if observed["desktop_tail_formatting_arm_present"] else "该planner分支尚待主线合入，不计完整源码接通。") +
        "owned553原有typed查询保持不变。\n\n"
        "965的807878实参窗口也已保留：record+900字符串、+916/+920 raw words；未凭旧0定义摘要"
        "推广成功或扩程序算法。后续无波形directive的965分支另见action_program交付，非空程序求值仍明确拒绝。完整native等价新增0；旧两源仅stdin grammar读取，"
        "没有编译、类型检查、测试、Rust/VM/PE/求值/渲染/游戏/设备执行。脚本归analysis，"
        "固定KCS/PE和语法产物归archive/2026-11/20261117_*，报告/小summary归reports；"
        "无README、译文或新目录变更。\n", encoding="utf8")
    current_path = ROOT / "reports/recovery_A_20261117_emote_current_summary.json"
    current = read_json(current_path)
    current["summary"]["latest_actual_command_query"] = {
        "report": PREFIX.with_name(PREFIX.name + "_summary.json").relative_to(ROOT).as_posix(),
        "native": 1028, "actual_static_call_sites": 1, "new_CPU_consumer": True,
        "host_pending": summary["main_hook_pending"], "full_equivalence_added": 0}
    current_path.write_text(json.dumps(current, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    print(json.dumps(summary, ensure_ascii=True))


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--probe", action="store_true")
    parser.add_argument("--span", action="append", default=[])
    parser.add_argument("--syntax", action="store_true")
    parser.add_argument("--report", action="store_true")
    args = parser.parse_args()
    if args.probe:
        probe()
    if args.span:
        pe(args.span)
    if args.syntax:
        syntax()
    if args.report:
        report()

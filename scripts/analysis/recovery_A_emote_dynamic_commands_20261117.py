"""Bounded reads of already inventoried NEKO dynamic commands, never a VM."""
import argparse
import hashlib
import json
import struct
import zlib

from audit_runtime_evidence import blowfish_le_decrypt, kif_entries, operand_count
from recovery_A_20261116 import ROOT, Image, read_json, sha
from recovery_A_receiver_contracts_20261116 import block

PREFIX = ROOT / "reports/recovery_A_20261117_emote_dynamic_commands"
CACHE = ROOT / "archive/2026-11/20261117_emote_dynamic_commands.json"
KCS_SHA = "5554f1f99f83c63087572ec1a7fb8ddf31987cedab4185c8503a1b1e29f5011a"


def save(data):
    CACHE.write_text(json.dumps(data, ensure_ascii=True, indent=2) + "\n", encoding="utf8")


def window(spans):
    identity = read_json(ROOT / "reports/recovery_A_20261116_emote.json")
    path = (ROOT / identity["binary_path"]).parent / "kcs.int"
    entries, _, cipher = kif_entries(path, 0x0F6BB0AE)
    entry = next(row for row in entries if row["name"] == b"sscript.kcs")
    with path.open("rb") as handle:
        handle.seek(entry["offset"])
        raw = handle.read(entry["size"])
    raw = blowfish_le_decrypt(cipher, raw) if cipher else raw
    if sha(raw) != KCS_SHA:
        raise ValueError("fixed KCS changed")
    length, = struct.unpack_from("<I", raw, 16)
    code = zlib.decompress(raw[52:])[:length]
    strings = {row["address"]: row["cp932"] for row in read_json(
        ROOT / "archive/2026-11/20261117_emote_command_producers.json")["strings"]}
    data = read_json(CACHE) if CACHE.exists() else {"KCS_sha256": KCS_SHA, "windows": {}, "PE": {}}
    for span in spans:
        start, end = (int(v, 0) for v in span.split(":"))
        if not 0 <= start < end <= len(code) or end - start > 5000:
            raise ValueError("bounded existing instruction anchors only")
        at, rows = start, []
        while at < end:
            opcode = code[at]
            count = operand_count(opcode)
            if count is None or at + 1 + count * 5 > len(code):
                raise ValueError("invalid instruction anchor")
            operands = []
            for pos in range(at + 1, at + 1 + 5 * count, 5):
                value, = struct.unpack_from("<I", code, pos + 1)
                operand = {"flags": code[pos], "value": value}
                if value in strings:
                    operand["static_string_at_value"] = strings[value]
                operands.append(operand)
            rows.append({"offset": at, "opcode": opcode, "operands": operands})
            at += 1 + count * 5
        data["windows"][span] = rows
        print(json.dumps({"window": span, "instructions": rows}, ensure_ascii=True))
    save(data)


def pe(spans):
    identity = read_json(ROOT / "reports/recovery_A_20261116_emote.json")
    image = Image(ROOT / identity["binary_path"], identity["PE_sha256"])
    data = read_json(CACHE) if CACHE.exists() else {"windows": {}, "PE": {}}
    data["PE_sha256"] = image.digest
    for span in spans:
        start, size = (int(v, 0) for v in span.split(":"))
        if not 0 < size <= 2048:
            raise ValueError("bounded PE read only")
        evidence = block(image, start, size)
        data["PE"][hex(start)] = evidence
        print(json.dumps({"RVA": hex(start), "instructions": evidence["instructions"]}, ensure_ascii=True))
    save(data)


def source(path, anchor):
    text = (ROOT / path).read_text(encoding="utf8")
    index = text.find(anchor)
    return {"path": path, "line": None if index < 0 else text[:index].count("\n") + 1,
            "anchor": anchor, "present": index >= 0}


def report():
    """Existing caches and source text only; no resource or code evaluation."""
    cpu_path = "simulator/crates/runtime/src/emote_cpu.rs"
    cpu = (ROOT / cpu_path).read_text(encoding="utf8")
    arm = cpu.split("EmoteOperation::TextureFilter => {", 1)[1].split("EmoteOperation::SplineLabel", 1)[0]
    evidence = read_json(CACHE)
    prior = read_json(ROOT / "reports/emote_resource_controls_20261117.json")
    inventory = read_json(ROOT / "reports/kcs_call_inventory_20261116.json")
    game = next(g for g in inventory["games"] if "NEKO" in g["root"])
    calls = [{"resource": p["resource"], **c} for p in game["programs"] for c in p["calls"]]
    summary = {
        "batch": "918 before906: persistent empty selector0 wrapper filter -> first actual texture rebuild",
        "actual_static_sites": [c for c in calls if c["index"] in (918, 906)],
        "record": "R=2480+1180*i; task=u32(R+680); filter=i32(R+688); load spec=R+8",
        "KCS_order": "790247..790337 prepares918,790529 calls919,790643 calls906",
        "receiver_contract": "task ctor237d7e creates empty selector0 wrapper through17d160/17a8d0; command535 queries it and1745d0 writes134 without driver/config/bank checks",
        "source_change": "TextureFilter stores raw word on registered CpuTask before any resource/live owner; existing rebuild/filter/cache/pixel/release consumers retained",
        "source_observed": {
            "raw_write": "state.texture_filter_raw = command.data1;" in arm,
            "loaded_owner_gate_removed": "if state.m2_owner.is_some()" not in arm,
            "actual_rebuild_reads_word": "let texture_filter_raw = state.texture_filter_raw;" in cpu,
            "new_model_receives_filter": "texture_filter_raw," in cpu,
        },
        "new_shared_consumers": 1, "full_native_equivalence_added": 0,
        "main_hooks_pending": [], "new_PSZ_or_full_game_scan": False,
        "compiled": False, "typechecked": False, "grammar_reader_run": False,
        "tests_run": False, "target_or_evaluator_executed": False,
    }
    contracts = {"237d79..237d7e": "push selector0 and taskid;17d160 registration",
        "17d247..17d250": "register manager group then ensure inner selector",
        "17a8fd..17a963": "missing selector allocates194-byte wrapper via170be0, inserts it without loading PSB",
        "238cb6..238cee": "command535 resolves actual group/selector0 wrapper before raw setter",
        "1745d0..1745df": "unconditional wrapper134 raw write",
        "175ac8..175b79": "next real rebuild filters texture before driver construction"}
    per_pe = [{"sha256": evidence["PE_sha256"], "addresses": "RVA", "contracts": contracts}]
    payload = {"summary": summary, "perPE": per_pe,
        "source": source(cpu_path, "EmoteOperation::TextureFilter => {"),
        "source_sha256": hashlib.sha256((ROOT / cpu_path).read_bytes()).hexdigest(),
        "static_evidence_cache": str(CACHE.relative_to(ROOT)).replace("\\", "/"),
        "reused_evidence": {k: prior["evidence"][k] for k in (
            "command535_setter", "wrapper_filter_raw_write", "wrapper_filter_default_zero",
            "rebuild_filter_before_driver_creation")}}
    PREFIX.with_suffix(".json").write_text(json.dumps(payload, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    PREFIX.with_name(PREFIX.name + "_summary.json").write_text(json.dumps(
        {"summary": summary, "perPE": per_pe}, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    PREFIX.with_suffix(".md").write_text(
        "# A / 实际918先设置、906后装载\n\n"
        "NEKO sscript固定SHA5554f1f9…5011a：790337先918，790529设置颜色，790643才906。"
        "R=2480+1180*i，task=u32(R+680)，filter=i32(R+688)，load-spec=R+8。"
        "调用次数918/906各1处，动态字段是否非零未经执行测量。\n\n"
        "固定PE c71c5b85…1adc：237d7e→17d160→17a8d0在type54构造时创建selector0空wrapper；"
        "238cb6的535查此wrapper，1745d0直接写134，无driver/配置/银行检查。"
        "CPU旧arm要求m2_owner存在，首次装载前实际值因此丢失；现窄改该arm保存raw。"
        "后续906→既有shared_model→纹理解码/滤镜→带filter缓存key→PSB真实像素消费者不变，"
        "同资源复用不重新过滤，释放随CpuTask和共享弱缓存回收。\n\n"
        f"源码：{cpu_path}:{payload['source']['line']}。无需desktop/lib新hook。"
        "新增实际状态消费者1，完整native/函数等价新增0，未声称strict像素通过。"
        "仅静态KCS/PE/源码；未编译、类型检查、grammar读取、测试或执行算法/游戏/设备。"
        "脚本已归analysis，定点证据与临时文本已归日期archive；未改README。\n",
        encoding="utf8")
    print(json.dumps({"summary_written": str(PREFIX.relative_to(ROOT)), "source": payload["source"], **summary["source_observed"]}))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--window", nargs="+")
    parser.add_argument("--span", nargs="+")
    parser.add_argument("--report", action="store_true")
    args = parser.parse_args()
    if args.window:
        window(args.window)
    if args.span:
        pe(args.span)
    if args.report:
        report()

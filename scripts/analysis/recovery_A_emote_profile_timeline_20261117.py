"""Fixed-profile timeline pair counts and bounded original bank lifecycle bytes."""
from __future__ import annotations

import argparse
import ast
import json
from pathlib import Path

from recovery_A_20261116 import ROOT, Image, read_json, sha
from recovery_A_receiver_contracts_20261116 import block
from recovery_A_emote_profile_20261117 import cpu_integration

SNAPSHOT = ROOT / "archive/2026-11/20261117_emote_profile_timeline_evidence.json"
PREFIX = "recovery_A_20261117_emote_profile_timeline"


def probe():
    path = ROOT / "archive/2026-11/20261117_emote_profile_evidence.json"
    prior = read_json(path)
    profiles = prior["resources"]["profiles"]
    a, b = (profiles[name]["sections"] for name in ("ca01.pfl", "ca01L.pfl"))
    pairs = []
    for first in a["charaname"]:
        for second in b["charaname"]:
            if first[2] != second[2]:
                continue
            for ma in a["motname"]:
                if ma[0] != first[0]:
                    continue
                for mb in b["motname"]:
                    if mb[0] == second[0]:
                        pairs.append({"resource": first[2], "from_motion": ma[1], "to_motion": mb[1],
                            "from_timeline": ma[2], "to_timeline": mb[2],
                            "base_changes": first[1] != second[1], "timeline_changes": ma[2] != mb[2]})
    counts = {"shared_resource_rows": len(prior["resources"]["shared_resources"]),
              "same_resource_ordered_config_pairs_one_direction": len(pairs),
              "same_motion_id_pairs": sum(p["from_motion"] == p["to_motion"] for p in pairs),
              "different_timeline_pairs": sum(p["timeline_changes"] for p in pairs),
              "different_base_pairs": sum(p["base_changes"] for p in pairs),
              "scope": "ca01 -> ca01l valid static configuration pairs; not observed KCS transitions"}
    image = Image(ROOT / prior["PE"]["path"], prior["PE"]["sha256"])
    spans = {
        "manager_retained_load_and_motion_order": (0x17D679, 0x150),
        "resource_equal_ignores_transition": (0x171A28, 0x22),
        "timeline_defaults_and_bank_rebuild": (0x171D50, 0x16D),
        "timeline_play_stop": (0x171EC0, 0x180),
        "default_variable_enumeration": (0x187700, 0xE0),
        "bank_release": (0x177A10, 0x200),
        "blink_bank_construct": (0x177C10, 0x470),
        "lip_bank_release": (0x178080, 0x110),
        "lip_bank_construct": (0x178190, 0x2D0),
        "primary_timeline_change_force_check": (0x172640, 0x3C),
        "primary_timeline_forced_dimension": (0x172E36, 0xB3),
        "primary_surface_prepass": (0x1727D8, 0x480),
        "blink_bank_driver_seed": (0x177D41, 0x130),
    }
    payload = {"PE": prior["PE"], "summary": counts, "pairs": pairs,
        "prior_evidence_sha256": sha(path.read_bytes()),
        "resource_equal_mode_independent": prior["evidence"]["profile_switch"]["instructions"][:28],
        "evidence": {key: block(image, *span) for key, span in spans.items()},
        "compiled": False, "typechecked": False, "tests_run": False, "target_executed": False}
    SNAPSHOT.write_text(json.dumps(payload, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    print(json.dumps(counts, ensure_ascii=True))


def report():
    evidence = read_json(SNAPSHOT)
    cpu = cpu_integration()
    sources = ["simulator/crates/runtime/src/emote_m2live.rs",
               "simulator/crates/runtime/src/emote_m2profile.rs"]
    small = {"summary": {
        "batch": "retained-resource timeline bank lifecycle, independent of transition0",
        **evidence["summary"],
        "source_implemented": [
            "171d50 timeline change stages #defval then restores active old blink bases through the existing value owner",
            "releases old blink/lip/waveform/sidecar banks and reconstructs only the bank vectors from the new primary config",
            "equal primary labels still select rebuilt banks, without restarting equal scalar chains",
            "material/force, secondary selections, speed/weight and driver timers stay owned",
            "initial resource load shares the same bank-only constructor; no duplicate bank implementation"],
        "unchanged_API": "CpuM2LiveOwner::prepare_profile_reuse -> CpuM2ProfileReuse { owner, variables, surface_duration }",
        "CPU_source_modified_by_A": False,
        "CPU_source_integration": cpu,
        "CPU_required_condition": "same_resource && (!same_profile || state.m2_timeline != load.timeline_label)",
        "transition0_PE_retains_resource_owner": True,
        "transition0_CPU_connected": cpu["zero_transition_connected"],
        "remaining": [
            *([] if cpu["zero_transition_connected"] and "m2_timeline" in (cpu["condition"] or "") else
              ["main widens existing helper condition for transition0 and same-profile timeline changes; CPU import/tick/command remains main-owned"]),
            "different base object and different primary dimension layout remain explicit unsupported branches; actual shared profiles have zero base changes",
            "virtual driver18 complete equivalence and whole native906 remain unclaimed; existing CPU stop/play-label path is retained",
            "source is statically implemented, without Rust execution, type checking, tests or complete NEKO pixel validation"],
        "ordinary_routes_added": 0, "completed_native_equivalence_added": 0,
        "compiled": False, "typechecked": False, "tests_run": False, "target_executed": False},
        "perPE": [{"sha256": evidence["PE"]["sha256"],
            "anchors_rva": ["171a38", "171a44", "171bb8", "17d6e8", "17d74a", "17d7c4",
                "171db1", "171e10", "171e65", "171e6e", "171e75", "171e7c", "171e89",
                "17266f", "172e36", "17301c", "173119", "17317d", "187700", "177b00"],
            "evidence_path": SNAPSHOT.relative_to(ROOT).as_posix()}]}
    prefix = ROOT / "reports" / PREFIX
    prefix.with_name(PREFIX + "_summary.json").write_text(
        json.dumps(small, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    full = {**small, "pairs": evidence["pairs"],
        "sources": [{"path": path, "sha256": sha((ROOT / path).read_bytes())} for path in sources],
        "evidence_sha256": sha(SNAPSHOT.read_bytes()),
        "no_new_resource_or_CFG_scan": True}
    prefix.with_suffix(".json").write_text(json.dumps(full, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    prefix.with_suffix(".md").write_text(
        "# A / 同资源timeline银行生命周期\n\n"
        "ca01/ca01L共5个既有PSZ，80个单方向有效静态配置配对：20个timeline相同、60个不同、"
        "base变化0。它们是配置可能组合，不是实际调用次数；没有重扫135PSZ。\n\n"
        "固定NEKO PE SHA c71c5b85…5b11adc：171a38/171a44资源相等且状态1直接到171bb8，"
        "此路径不读transition。17d6e8先复用资源，17d74a随后无条件请求171d50，17d7c4才选择primary。"
        "171db1比较timeline名，变化时171e10经187700枚举#defval写16df30，接着177a10恢复旧active blink基值并释放、"
        "177c10重建blink、178080释放lip、178190重建lip；171e89设置110使相同pattern仍走bank选择，"
        "172e36仍对相等scalar端点跳过chain。不存在以transition0决定全owner重建的分支。\n\n"
        "已改emote_m2profile：prepare_profile_reuse不再拒绝timeline变化，按上述次序只重建两类银行，"
        "旧waveform/sidecar随旧bank释放，不将旧phase伪装成新timeline的live行为。"
        "默认值和旧blink基值进入已有value_table；force/material、secondary vector、速度权重、"
        "未变chain与driver timers保留。emote_m2live初建也使用同一个bank构造器；"
        "reselect内部refresh_banks仅强制bank重选，不制造同名scalar过渡。\n\n"
        "API未变：`prepare_profile_reuse(...) -> CpuM2ProfileReuse { owner, variables, surface_duration }`。"
        "主线CPU应具备条件`same_resource && (!same_profile || state.m2_timeline != load.timeline_label)`，"
        "包括transition0；prepared安装、旧RGBA预捕获、reusing_owner和既有stop/play timeline均沿用。"
        "当前源码实际条件为`" + str(cpu["condition"]) + "`；本批未写CPU。\n\n"
        "仍缺：不同base对象的reopen及不同primary维度布局；本对实际base命中0。"
        "未宣称原driver virtual18及完整906等价，未增加ordinary/native完成计数。"
        "仅静态PE/配置和源码grammar读取，未编译、类型检查、测试或执行算法/PE/游戏。"
        "脚本归scripts/analysis，临时固定证据归archive/2026-11/20261117_emote_profile_timeline_*；无新增目录、README或译文修改。\n",
        encoding="utf8")
    current_path = ROOT / "reports/recovery_A_20261117_emote_current_summary.json"
    current = read_json(current_path)
    current["profile_timeline_batch"] = small
    current["summary"]["latest_A_batch"] = small["summary"]
    relative = prefix.with_suffix(".md").relative_to(ROOT).as_posix()
    if relative not in current.setdefault("reports", []):
        current["reports"].append(relative)
    current_path.write_text(json.dumps(current, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    rolling_path = ROOT / "reports/recovery_A_20261117_rolling_delivery.md"
    rolling = rolling_path.read_text(encoding="utf8")
    note = ("交付`" + PREFIX + ".md`：5共享PSZ/80静态配对中60个timeline变化已接默认值→旧blink基值恢复→"
        "bank-only重建→真实primary银行选择；value/force/timers保留。PE复用与transition0无关，"
        + ("CPU当前静态条件已覆盖transition0/timeline变化；" if cpu["zero_transition_connected"] and "m2_timeline" in (cpu["condition"] or "")
           else "主线仍需扩现有helper条件；")
        + "A未写CPU。base/layout仍明确拒绝，整native等价新增0。\n\n")
    import re
    rolling = re.sub(r"交付`" + PREFIX + r"\.md`：[^\n]*\n\n", "", rolling)
    if note not in rolling:
        rolling_path.write_text(rolling.replace("# A / 本轮小批交付\n\n", "# A / 本轮小批交付\n\n" + note, 1), encoding="utf8")
    print(json.dumps({"report": relative, "pairs": evidence["summary"],
                      "CPU_condition": cpu["condition"]}, ensure_ascii=True))


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--probe", action="store_true")
    parser.add_argument("--report", action="store_true")
    args = parser.parse_args()
    ast.parse(Path(__file__).read_text(encoding="utf8"))
    if args.probe:
        probe()
    if args.report:
        report()

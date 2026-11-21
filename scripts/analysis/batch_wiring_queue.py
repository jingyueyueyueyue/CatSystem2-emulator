"""Build evidence-grouped wiring batches without generating runtime stubs.

Candidate command numbers are recovered only from seven-push virtual call
sites in hash-pinned wrappers. A same-number source receiver is navigation,
not proof of type/version compatibility or completed implementation.
"""
from __future__ import annotations
from collections import Counter, defaultdict
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import re

from native_batch_contracts import analyze
from native_coverage import RustSource

ROOT = Path(__file__).resolve().parents[2]

def command_sites(entry: dict) -> list[dict]:
    pushes = []
    found = []
    for instruction in entry["instructions"]:
        mnemonic, operands = instruction["mnemonic"], instruction["operands"]
        if mnemonic == "push":
            pushes.append(instruction)
        elif mnemonic == "call":
            if re.fullmatch(r"dword ptr \[[a-z]{3} \+ 0x10\]", operands) and len(pushes) >= 7:
                arguments = pushes[-7:]
                code = arguments[-3]["operands"]
                if re.fullmatch(r"0x[0-9a-f]+|\d+", code):
                    found.append({
                        "command": int(code, 0), "call_rva": instruction["rva"],
                        "pushes": arguments,
                        "status": "linear-seven-push-candidate-requires-control-flow-and-owner-review",
                    })
            pushes = []
    return found

def receivers() -> dict[int, list[dict]]:
    result = defaultdict(list)
    for path in sorted((ROOT / "simulator/crates/desktop/src").glob("*.rs")):
        raw = path.read_bytes()
        text = raw.decode("utf-8-sig")
        if not re.search(r"match\s+(?:command|packet|message)\.code\s*\{", text):
            continue
        relative = path.relative_to(ROOT).as_posix()
        source = RustSource(text, relative, require_dispatch=False)
        for name, (start, end) in source.functions.items():
            for match in re.finditer(r"match\s+(?:command|packet|message)\.code\s*\{",
                                     source.mask[start:end]):
                brace = start + match.end() - 1
                try:
                    arms = source.match_arms(brace)
                except ValueError:
                    # An enum/bit-pattern receiver is not a numeric candidate.
                    continue
                for arm in arms:
                    for command in arm.values or set():
                        result[command].append({
                            "source": relative, "function": name, "line": arm.line,
                            "source_sha256": hashlib.sha256(raw).hexdigest(),
                            "status": "same-command-number-navigation-not-owner-equivalence",
                        })
    return result

def main() -> None:
    coverage_path = ROOT / "reports/native_coverage_20261116.json"
    workload_path = ROOT / "reports/workload_status_20261116.json"
    coverage = json.loads(coverage_path.read_text(encoding="utf-8"))
    workload = json.loads(workload_path.read_text(encoding="utf-8"))
    ordinary = workload["domains"]["native_by_host_table"]["ordinary"]
    missing = sorted(ordinary["missing_indices"])
    native_rows = {row["index"]: row for row in coverage["natives"]}
    wrappers = analyze(missing, 192)
    by_slot = defaultdict(list)
    for binary in wrappers["binaries"]:
        for entry in binary["entries"]:
            by_slot[entry["slot"]].append({
                "pe": binary["path"], "sha256": binary["sha256"],
                "entry_rva": entry["entry_rva"], "table_rva": entry["table_rva"],
                "first_linear_return_seen": entry["first_linear_return_seen"],
                "command_sites": command_sites(entry),
                "direct_calls": entry["direct_calls"],
                "wrapper_byte_sha256": hashlib.sha256(bytes.fromhex("".join(
                    item["bytes"] for item in entry["instructions"]))).hexdigest(),
            })
    existing = receivers()
    rows = []
    for slot in missing:
        native = native_rows[slot]
        profiles = by_slot[slot]
        commands = sorted({site["command"] for profile in profiles for site in profile["command_sites"]})
        candidates = []
        for command in commands:
            candidates.extend({"command": command, **row} for row in existing.get(command, []))
        files = Counter(row["source"] for row in candidates)
        batch = files.most_common(1)[0][0] if files else (
            "wrapper-has-command-but-receiver-recovery-required" if commands
            else "direct-helper-or-platform-semantic-recovery-required")
        rows.append({
            "index": slot, "suggested_batch": batch,
            "observed_argument_bytes": native["argument_bytes"],
            "sample_games": native["games_count"], "static_call_sites": native["call_sites"],
            "profiles": profiles, "receiver_candidates": candidates,
            "review_gate": [
                "exact target task kind and reachable receiver",
                "per-PE command identity and complete argument ownership",
                "real state/pixel/resource side effects before returning",
                "native return status and zero/invalid target branches",
                "add descriptor and actual dispatch, then refresh static snapshot",
            ],
            "automatically_implemented": False,
        })
    groups = defaultdict(list)
    for row in rows:
        groups[row["suggested_batch"]].append(row["index"])
    report = {
        "schema": "cs2-batch-wiring-queue/v1",
        "counts_scope": "published-workload-snapshot-not-current-behavior-remaining",
        "generated_at": datetime.now(timezone.utc).isoformat(),
        "coverage_sha256": hashlib.sha256(coverage_path.read_bytes()).hexdigest(),
        "ordinary_missing": len(missing),
        "same_number_receiver_navigation_candidates": sum(bool(row["receiver_candidates"]) for row in rows),
        "linear_command_wrapper_candidates": sum(
            any(profile["command_sites"] for profile in row["profiles"]) for row in rows),
        "groups": dict(sorted(groups.items())), "entries": rows,
        "runtime_source_generated": False, "implemented_count_added": 0,
        "compiled": False, "tests_run": False, "original_code_executed": False,
    }
    destination = ROOT / "reports/batch_wiring_queue_20261117.json"
    destination.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    lines = [
        "# 剩余接线批量队列", "",
        f"来源workload快照的普通未路由{len(missing)}项（非当前行为剩余量）；"
        f"{report['linear_command_wrapper_candidates']}项有静态七参数消息包装候选，"
        f"{report['same_number_receiver_navigation_candidates']}项命中同编号源码接收者导航。", "",
        "同消息编号可能属于不同对象或版本，以下分组不是已可直接支持的声明。"
        "逐项审查对象型别、ABI、拥有缓冲、真实副作用和返回状态后，"
        "才加入共享描述表。此脚本没有生成占位实现或增加完成计数。", "",
        "当前源码已可能包含后续固定版本实现；本队列不因此自动标记全版本完成，"
        "也不应继续将旧快照槽位数解释为只剩几个函数。最新交付见batch_progress.md。", "",
        "|建议审查批|旧快照编号|", "|---|---|",
    ]
    for group, indices in sorted(groups.items()):
        lines.append(f"|`{group}`|{', '.join(map(str, indices))}|")
    lines += ["", "每项固定PE SHA/RVA、包装push序列、原被调帮助函数、源码行号及哈希"
              "保存在同名JSON；当前仅扫描两份基线PE，其他版本继续单独恢复。", "",
              "正式脚本归scripts/analysis，报告归reports；不编译、不运行函数、VM或游戏。"]
    destination.with_suffix(".md").write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(json.dumps({key: report[key] for key in [
        "ordinary_missing", "linear_command_wrapper_candidates",
        "same_number_receiver_navigation_candidates", "implemented_count_added",
    ]}))

if __name__ == "__main__":
    main()

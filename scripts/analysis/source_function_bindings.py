"""Bind recovered ordinary entry identities to current source candidates.

Numeric slot agreement is a navigation aid, never cross-version semantic
equivalence. This ledger records incomplete/constant routes explicitly and
does not modify the original recovery ledger or claim an implementation done.
"""
from __future__ import annotations
from collections import Counter, defaultdict
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]

def read(relative: str) -> dict:
    return json.loads((ROOT / relative).read_text(encoding="utf-8"))

def digest(path: Path) -> str:
    state = hashlib.sha256()
    with path.open("rb") as file:
        for data in iter(lambda: file.read(1024 * 1024), b""):
            state.update(data)
    return state.hexdigest()

def main() -> None:
    recovery_path = ROOT / "reports/recovery_A_20261116.json"
    native_path = ROOT / "reports/native_coverage_20261116.json"
    recovery = read(recovery_path.relative_to(ROOT).as_posix())
    native = read(native_path.relative_to(ROOT).as_posix())
    coverage = {row["index"]: row for row in native["natives"]}
    freshness = []
    preserved = []
    for source in native["sources"]:
        path = ROOT / source["path"]
        freshness.append(path.is_file() and digest(path) == source["sha256"])
        archived = ROOT / source["archive"] if source.get("archive") else None
        preserved.append(archived is not None and archived.is_file()
                         and digest(archived) == source["sha256"])
    if not all(freshness) and not all(preserved):
        raise ValueError("Native source changed and exact inspected bytes are not archived; refresh coverage")
    groups: dict[str, list[dict]] = defaultdict(list)
    for sha256, image in recovery["images"].items():
        for table in image["tables"]:
            if table["table_hint"] != "ordinary":
                continue
            for slot in table["slots"]:
                index = slot["index"]
                if index not in coverage:
                    continue
                source = coverage[index]
                key = slot.get("function_identity", f"{sha256}:{slot['entry_rva']:08x}")
                groups[key].append({
                    "ordinary_index": index, "table_rva": table["table_rva"],
                    "table_classification": table["table_identity_evidence"],
                    "registration_chain_verified": table["registration_chain_verified"],
                    "source_status": source["status"],
                    "source_route_present": source["rust_route_present"],
                    "source_arms": source["rust_arms"],
                    "source_helpers": [{
                        name: helper.get(name) for name in [
                            "source", "line", "helper", "function_name",
                            "accepted_argument_bytes", "minimum_argument_bytes",
                        ]
                    } for helper in source["helpers"]],
                    "source_semantics_verified": False,
                    "original_version_semantics_verified": False,
                })
    rows = []
    counts = Counter()
    for identity, aliases in sorted(groups.items()):
        sha256, rva = identity.split(":")
        routed = [row for row in aliases if row["source_route_present"]]
        status = ("no-source-route" if not routed else
                  "mixed-alias-source-candidate" if len(routed) != len(aliases) else
                  "source-candidates-awaiting-version-semantics")
        counts[status] += 1
        rows.append({
            "identity": identity, "pe_sha256": sha256, "entry_rva": int(rva, 16),
            "status": status, "aliases": aliases,
            "equivalent_implementation_verified": False,
            "reachability_exclusion_verified": False,
            "limits": "Source candidates follow numeric ordinary slots. Shared entries and version-specific behavior require separate proof.",
        })
    output = {
        "schema": "cs2-source-function-binding-candidates/v1",
        "status": "source-navigation-candidates-not-equivalence-or-completion",
        "inputs": {
            recovery_path.relative_to(ROOT).as_posix(): digest(recovery_path),
            native_path.relative_to(ROOT).as_posix(): digest(native_path),
        },
        "source_snapshot_current_at_read": all(freshness),
        "inspected_source_bytes_preserved": all(preserved),
        "source_evidence_archive": native.get("source_evidence_storage"),
        "current_completion_proof": False,
        "summary": {
            "ordinary_entry_identities": len(rows), "status_counts": dict(counts),
            "candidate_slot_bindings": sum(len(row["aliases"]) for row in rows),
            "verified_complete_original_functions": 0,
            "verified_unreachable_original_functions": 0,
            "non_native_code_functions_not_measured_here": True,
        },
        "functions": rows,
    }
    target = ROOT / "reports/source_function_bindings_20261117.json"
    target.write_text(json.dumps(output, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    lines = [
        "# 原版入口到当前源码的候选绑定",
        "",
        "只读普通注册表和当前源码证据。编号相同不能证明版本行为相同，"
        "候选绑定不计为全部函数实现或原版等价验证。",
        "",
        f"- 已关联原版 SHA-256＋RVA 入口身份：{len(rows):,}。",
        f"- 数字槽到源码候选记录：{output['summary']['candidate_slot_bindings']:,}。",
        "- 本报告新增原版完整等价函数：0；新增不可达排除：0。",
        f"- 读取时仍匹配当前源码：{all(freshness)}；已保留全部被分析源码字节：{all(preserved)}。",
        "- 普通表之外的函数、CRT/codec/helper不因本表存在被宣称完成。",
        "",
        "|候选状态|函数身份数|",
        "|---|---:|",
    ]
    lines += [f"|{status}|{count:,}|" for status, count in sorted(counts.items())]
    lines += ["", "JSON保留每项真实入口身份、表RVA、别名、源码行、helper及来源哈希；"
              "后续逐函数审查以此定位，不能用单个全局编号状态覆盖各PE语义。并行修改后"
              "继续使用原快照时必须存在每项可重新核哈希的归档，不冒充当前工作目录。",
              "", "分析脚本已分类至scripts/analysis，报告归reports；没有编译、执行或测试。",
              ""]
    target.with_suffix(".md").write_text("\n".join(lines), encoding="utf-8")
    print(json.dumps(output["summary"], indent=2))

if __name__ == "__main__":
    main()

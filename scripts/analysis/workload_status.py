"""Publish evidence-based implementation workload without running game code.

Consumes existing PE, native and content reports without rescanning games.
Source changes mark the native snapshot stale until a planned refresh. Counts describe different domains and must
not be added or turned into a single game-completion percentage.
"""

from __future__ import annotations

import hashlib
import json
from collections import Counter
from datetime import datetime
from pathlib import Path
from zoneinfo import ZoneInfo


ROOT = Path(__file__).resolve().parents[2]
REPORT = ROOT / "reports" / "workload_status_20261116"


def read_report(relative: str) -> dict:
    return json.loads((ROOT / relative).read_text(encoding="utf-8"))


def percentage(part: int, whole: int) -> float | None:
    return round(100 * part / whole, 2) if whole else None


def reviewed_partial_routes() -> list[dict]:
    """Record reviewed forwarding missed by the numeric route extractor.

    These prove source entry paths, not complete receivers or all versions.
    Keep the automatic result intact and retain hashes for this narrower fact.
    """
    checks = [
        ([576], {
            "simulator/crates/desktop/src/lib.rs":
                ("if index == 576", "return self.restore_css_task_native(arguments)"),
            "simulator/crates/desktop/src/css_task_native.rs":
                ("fn restore_css_task_native", "const COMMANDS:", "self.receive_css_task_command"),
        }),
        ([688, 689], {
            "simulator/crates/desktop/src/lib.rs":
                ("native_tasks::ObjectGraphReceiver::supports(index)",
                 "return self.portable_object_graph_native(index, arguments)"),
            "simulator/crates/desktop/src/native_tasks.rs":
                ("pub(crate) use object_graph::ObjectGraphReceiver;",),
            "simulator/crates/desktop/src/object_graph.rs":
                ("matches!(index, 688 | 689)", "fn portable_object_graph_native",
                 "return self.original_state_tree_native(index, args)"),
            "simulator/crates/desktop/src/state_tree_native.rs":
                ("fn original_state_tree_native", "self.state_tree_abi()"),
        }),
    ]
    result = []
    for indices, sources in checks:
        evidence = []
        for name, required in sources.items():
            path = ROOT / name
            if not path.is_file():
                break
            raw = path.read_bytes()
            source = raw.decode("utf-8-sig")
            if not all(marker in source for marker in required):
                break
            evidence.append({"path": name, "sha256": hashlib.sha256(raw).hexdigest()})
        else:
            result.append({"indices": indices, "sources": evidence,
                           "status": "reviewed-fixed-version-partial-source-route",
                           "complete_semantics": False})
    return result


def build_report() -> dict:
    native = read_report("reports/native_coverage_20261116.json")
    functions = read_report("reports/runtime_function_coverage_all_ghidra.json")
    content = read_report("reports/all_sample_content_audit.json")
    provenance_path = ROOT / "reports/kcs_call_inventory_20261116.json"
    provenance = read_report("reports/kcs_call_inventory_20261116.json") if provenance_path.is_file() else None
    recovered_path = ROOT / "reports/recovery_A_20261116_summary.json"
    recovered = read_report("reports/recovery_A_20261116_summary.json") if recovered_path.is_file() else None
    function_scope = functions["scope"]
    native_scope = native["scope"]
    statuses = native_scope["status_counts"]
    source_freshness = []
    for source in native["sources"]:
        path = ROOT / source["path"]
        actual = hashlib.sha256(path.read_bytes()).hexdigest() if path.is_file() else None
        source_freshness.append({
            "path": source["path"],
            "expected_sha256": source["sha256"],
            "actual_sha256": actual,
            "matches": actual == source["sha256"],
        })
    diagnostics = Counter()
    diagnostic_games = []
    opcodes = set()
    for game in content["games"]:
        opcodes.update(game["scripts"]["opcodes"])
        issues = game.get("diagnostics", [])
        if issues:
            diagnostic_games.append({
                "root": game["root"],
                "historical_diagnostic_count": len(issues),
                "evidence": "content audit snapshot; not rerun by this report",
            })
        for issue in issues:
            diagnostics[str(issue.get("message", issue.get("error", "unspecified")))] += 1
    total = native_scope["observed_numeric_slots"]
    routed = native_scope["observed_slots_with_explicit_arm"]
    behavior = statuses.get("source-behavior", 0)
    table_domains = {}
    if provenance:
        indices_by_table = {}
        for game in provenance["games"]:
            for program in game["programs"]:
                if program.get("role") == "excluded-installer-script":
                    continue
                indices = indices_by_table.setdefault(program["table_hint"], set())
                indices.update(call["index"] for call in program["calls"] if call["index"] is not None)
        rows = {row["index"]: row for row in native["natives"]}
        ordinary = indices_by_table.get("ordinary", set())
        ordinary_status = Counter(rows[index]["status"] for index in ordinary)
        table_domains["ordinary"] = {
            "observed": len(ordinary),
            "routed": sum(rows[index]["rust_route_present"] for index in ordinary),
            "unmapped": ordinary_status.get("unmapped", 0),
            "source_behavior": ordinary_status.get("source-behavior", 0),
            "status_counts": dict(sorted(ordinary_status.items())),
            "missing_indices": sorted(index for index in ordinary if rows[index]["status"] == "unmapped"),
            "semantics_verified": False,
        }
        reviewed = reviewed_partial_routes()
        corrected = sorted(set().union(*(set(row["indices"]) for row in reviewed))
                           & set(table_domains["ordinary"]["missing_indices"]))
        table_domains["ordinary"].update({
            "reviewed_partial_routes": reviewed,
            "extractor_missed_partial_indices": corrected,
            "routed_including_reviewed_partial_subsets":
                table_domains["ordinary"]["routed"] + len(corrected),
            "remaining_without_any_reviewed_source_route": sorted(
                set(table_domains["ordinary"]["missing_indices"]) - set(corrected)),
        })
        for table in ("motion", "particle", "unknown"):
            table_domains[table] = {
                "observed": len(indices_by_table.get(table, set())),
                "indices": sorted(indices_by_table.get(table, set())),
                "ordinary_dispatch_not_used_for_coverage": True,
                "semantics_verified": False,
            }
        table_domains["scope"] = {
            "programs_scanned": provenance["program_count"],
            "runtime_programs": provenance.get("runtime_program_count"),
            "parse_diagnostics": provenance["diagnostic_count"],
            "excluded_programs": provenance.get("excluded_programs", []),
            "classification": "mount/path hints and unique-name patch mapping; registration unverified",
        }
    binding_path = ROOT / "reports/source_function_bindings_20261117.json"
    binding = read_report("reports/source_function_bindings_20261117.json") if binding_path.is_file() else None
    binding_summary = None if binding is None else {
        **binding["summary"],
        "source": binding_path.relative_to(ROOT).as_posix(),
        "native_snapshot_matches": binding["inputs"].get("reports/native_coverage_20261116.json")
            == hashlib.sha256((ROOT / "reports/native_coverage_20261116.json").read_bytes()).hexdigest(),
        "counts_as_complete_functions": False,
    }
    ppi_path = ROOT / "reports/particle_ppi_manifest_20261117.json"
    ppi = read_report("reports/particle_ppi_manifest_20261117.json") if ppi_path.is_file() else None
    gap_path = ROOT / "archive/2026-11/20261117_implementation_gap_markers.json"
    gap_markers = json.loads(gap_path.read_text(encoding="utf-8")) if gap_path.is_file() else None
    archive_evidence_path = ROOT / "archive/2026-11/20261117_format_B_unindexed_archives.json"
    archive_resolution = None
    if archive_evidence_path.is_file():
        evidence = json.loads(archive_evidence_path.read_text(encoding="utf-8"))
        files = [{key: row[key] for key in ("path", "sha256", "classification", "entry_count")
                  if key in row} for row in evidence["files"]]
        archive_resolution = {
            "source": archive_evidence_path.relative_to(ROOT).as_posix(),
            "audit_snapshot_matches": evidence["audit_sha256"] == hashlib.sha256(
                (ROOT / "reports/all_sample_content_audit.json").read_bytes()).hexdigest(),
            "files": files,
            "non_resource_text_files": sum(row["classification"] == "non-resource CP936 prose" for row in files),
            "runtime_fuyin_archives": sum(row["classification"] == "runtime translation PNG archive FUYINPAK" for row in files),
            "runtime_fuyin_entries": sum(row.get("entry_count", 0) for row in files
                if row["classification"] == "runtime translation PNG archive FUYINPAK"),
            "reader_implemented_not_rerun": True,
            "image_replacement_source_implemented": (
                ROOT / "simulator/crates/desktop/src/image_island_patch.rs").is_file(),
            "image_replacement_verified": False,
        }
    return {
        "schema": "cs2-workload-status/v1",
        "generated_at": datetime.now(ZoneInfo("Asia/Shanghai")).isoformat(timespec="seconds"),
        "status": "static-snapshot-not-acceptance",
        "method": "read existing evidence and hash source; no compiler or game execution",
        "native_source_current": all(item["matches"] for item in source_freshness),
        "source_function_binding_candidates": binding_summary,
        "source_gap_marker_navigation": None if gap_markers is None else {
            "summary": gap_markers["summary"], "generated_at": gap_markers["generated_at"],
            "source": gap_path.relative_to(ROOT).as_posix(),
            "scope": gap_markers["scope"], "limitations": gap_markers["limitations"],
            "counts_as_remaining_functions": False,
        },
        "source_freshness": source_freshness,
        "domains": {
            "pe_functions": {
                "binary_count": function_scope["binary_count"],
                "sample_count": function_scope["sample_count"],
                "candidate_count": function_scope["capstone_candidate_count"],
                "candidate_instruction_coverage_percent":
                    function_scope["candidate_instruction_coverage_percent"],
                "linear_decoded_instruction_coverage_percent":
                    function_scope["decoded_instruction_coverage_percent"],
                "ghidra_import_ok": function_scope["ghidra_ok_count"],
                "ghidra_failed_or_missing": function_scope["ghidra_failed_count"],
                "semantic_bindings_recorded": function_scope["semantic_implementation_count"],
                "remaining_semantic_functions": None,
                "completion_percent": None,
                "latest_recovery": recovered["summary"] if recovered else None,
                "reason": (
                    "Candidate recovery is incomplete and lacks source-to-original semantic "
                    "bindings. Import success and decoded bytes do not establish functions "
                    "or implementation. Zero bindings does not mean zero Rust behavior."
                ),
            },
            "native_numeric_slots": {
                "observed": total,
                "routed": routed,
                "routed_percent": percentage(routed, total),
                "unmapped": statuses.get("unmapped", 0),
                "unmapped_percent": percentage(statuses.get("unmapped", 0), total),
                "source_behavior": behavior,
                "source_behavior_percent": percentage(behavior, total),
                "status_counts": statuses,
                "semantically_verified": native_scope["semantically_verified_implementation_count"],
                "caveat": (
                    "Numeric slots aggregate ordinary, motion and particle call tables. "
                    "An ordinary route does not establish another table's behavior. "
                    "A route is not a verified implementation."
                ),
            },
            "native_by_host_table": table_domains,
            "particle_ppi_models": None if ppi is None else {
                **ppi["summary"], "source": ppi_path.relative_to(ROOT).as_posix(),
                "manifest_sha256": hashlib.sha256(ppi_path.read_bytes()).hexdigest(),
                "verification_scope": ppi["verification_scope"],
                "counts_as_complete_functions_or_games": False,
            },
            "kcs_opcodes": {
                "observed_distinct": len(opcodes),
                "observed": sorted(opcodes),
                "full_cross_version_equivalence_verified": False,
            },
            "content": {
                "samples": content["game_count"],
                "archive_total": sum(g["archives"]["total"] for g in content["games"]),
                "indexed_archives": sum(g["archives"]["indexed"] for g in content["games"]),
                "indexed_entries": sum(g["archives"]["indexed_entries"] for g in content["games"]),
                "historical_diagnostic_count": sum(diagnostics.values()),
                "historical_diagnostic_games": diagnostic_games,
                "bounded_unindexed_resolution": archive_resolution,
                "note": "Historical audit predates parser fixes. Do not treat it as current failures.",
            },
            "full_game_acceptance": {
                "required_samples": content["game_count"],
                "recorded_complete_sample_acceptances": 0,
                "note": (
                    "No complete sample acceptance is recorded in supplied evidence. "
                    "Bounded startup, synthetic tests and changing frame hashes are "
                    "not end-to-end acceptance."
                ),
            },
        },
        "game_native_summary": native["game_summary"],
        "highest_frequency_gaps": native["highest_frequency_gaps"][:30],
        "remaining_workstreams": [
            "R5：统一构建ARM64 APK、安装并推进已连接平板的实际流程。",
            "R6：推进20样本启动、正文、选择、存读档和媒体；同源真实阻断成组修复。",
        ],
        "estimate": {
            "remaining_hours": None,
            "overall_completion_percent": None,
            "reason": (
                "Unknown function denominator, protected formats, version-specific native "
                "semantics and absence of measured implementation throughput prevent a "
                "defensible time or overall percentage estimate."
            ),
        },
    }


def markdown(report: dict) -> str:
    domains = report["domains"]
    pe = domains["pe_functions"]
    slots = domains["native_numeric_slots"]
    ordinary = domains.get("native_by_host_table", {}).get("ordinary")
    displayed_routes = ordinary if ordinary else slots
    route_total = displayed_routes.get("observed", 0)
    route_label = "普通表native旧快照" if ordinary else "native数字槽位旧快照"
    recovered = pe.get("latest_recovery")
    candidate_summary = (
        f"{recovered['unique_image_count']} 个不同 PE 内容、"
        f"{recovered['new_unique_candidate_count']:,} 个 SHA-256＋RVA 候选"
        if recovered else f"{pe['binary_count']} 个 PE，{pe['candidate_count']:,} 个候选"
    )
    byte_coverage = (
        recovered["new_path_byte_coverage_percent"]
        if recovered else pe["candidate_instruction_coverage_percent"]
    )
    lines = [
        "# 剩余工作量与证据状态",
        "",
        f"生成时间：{report['generated_at']}。本报告不编译、不运行游戏。",
        "",
        "当前已进入Windows与Android整编及实际启动推进阶段；"
        "当前交付包与最新结果以remaining_to_full_games_20261118.md为准。"
        "不能把候选函数数、源码行数、可解析资源数或启动帧数当作完整游戏完成率。",
        "",
        "| 层次 | 当前量化结果 | 距离完成的含义 |",
        "| --- | --- | --- |",
        f"| 原版函数账本 | {candidate_summary} | "
        "候选总数仍非完整分母，缺源码语义绑定 |",
        f"| 函数边界恢复 | 候选内指令占代码段 {byte_coverage}% | "
        "不是模拟器实现百分比；间接入口、加壳代码等尚需恢复 |",
        f"| Ghidra | {pe['ghidra_import_ok']}/{pe['binary_count']} 个有 import 结果 | "
        f"{pe['ghidra_failed_or_missing']} 个未成功；import 不是语义恢复 |",
        f"| {route_label} | {displayed_routes['routed']}/{route_total} 有路由 | "
        f"{displayed_routes['unmapped']}个自动提取未命中；人工核对的部分入口另列，不能作剩余函数数 |",
        f"| 同表实际源码行为旧分类 | 静态检查归类{displayed_routes['source_behavior']}个 | "
        "并行源码尚未统一重分类；不是完整函数实现数 |",
        f"| KCS opcode | 样本联合出现 {domains['kcs_opcodes']['observed_distinct']} 个 | "
        "跨版本完整等价性未验证 |",
        f"| 完整游戏验收 | 记录中 0/{domains['full_game_acceptance']['required_samples']} 完整通过 | "
        "已有冷启动和单帧证据不足以完成验收 |",
        "",
        "native 的 685 个数字编号跨普通、运动和粒子表聚合，不能直接称为 "
        "685 个不同原生函数，也不能用普通表命中证明其他表已实现。",
        "",
        "| native 静态分类 | 数量 |",
        "| --- | ---: |",
    ]
    for status, count in sorted(slots["status_counts"].items()):
        lines.append(f"| `{status}` | {count} |")
    markers = report.get("source_gap_marker_navigation")
    if markers:
        summary = markers["summary"]
        lines += [
            "", f"当前源码未恢复提示导航：静态读取{summary['source_files']}份生产Rust源码，"
            f"发现{summary['messages']}条明确未恢复/未实现消息，位于"
            f"{summary['enclosing_functions']}个函数中。注释、日志分类器和尾部测试模块已排除。"
            "这些消息仍包括通用兜底、未使用或无效输入；缺代码也未必留消息，"
            "因此这两个数都不能当成剩余函数或行为总数。逐项源码、行号、哈希及读取字节"
            f"见`{markers['source']}`。", "",
        ]
    recovered = pe.get("latest_recovery")
    if recovered:
        lines += [
            "", "较新的间接入口/注册表恢复结果：", "",
            f"- 54 条 PE 路径对应 {recovered['unique_image_count']} 个不同文件内容。",
            f"- 路径计数候选 {recovered['new_path_candidate_count']:,}；"
            f"按 PE SHA-256＋RVA 去重为 {recovered['new_unique_candidate_count']:,}，"
            f"其中 {recovered.get('zero_instruction_rejected_seeds', 0)} 个种子没有解码指令。",
            f"- {recovered['semantic_evidence_unique_candidates']:,} 个有部分语义线索，"
            f"{recovered['unknown_unique_candidates']:,} 个尚无；"
            f"{recovered['exact_local_facts_unique_candidates']} 个具有局部指令事实。",
            f"- 候选 CFG 字节覆盖从旧快照 2.62% 提升到 "
            f"{recovered['new_path_byte_coverage_percent']}%；仍然不是源码实现率。",
            "",
            "候选包括系统/CRT、编解码器和依赖库；有语义线索也可能只是文档契约或"
            "一个字段读写事实，不能算完整函数已实现。不同版本形状相似也不能直接视为同一行为。",
            "",
        ]
    tables = domains.get("native_by_host_table", {})
    if tables:
        ordinary = tables["ordinary"]
        lines += [
            "", "按宿主表分开后的实现缺口：", "",
            "全量 KCS 解析保留资源、程序版本和调用位置；路径/同名补丁推断的表"
            "归属尚需原版注册链确认。安装目录 `inst/inst.kcs` 不纳入运行时实现分母。",
            "", "| 表 | 实际观察编号 | 当前源码缺口 |",
            "| --- | ---: | --- |",
            f"| 普通 | {ordinary['observed']} | {ordinary['unmapped']} 个自动提取未命中，"
            f"{ordinary['routed']} 有分派，{ordinary['source_behavior']} 归为实际源码行为 |",
            f"| 运动 | {tables['motion']['observed']} | 使用独立 Motion host；不能从普通表计算缺口 |",
            f"| 粒子 | {tables['particle']['observed']} | 使用独立 Particle host；需按程序版本核对 |",
            "",
            "例如数字 7 在样本中属于运动/粒子调用，普通表并未观察到该编号。"
            "不能将普通宿主拒绝 7 记为这些脚本未实现，也不能用普通表的数学行为代替它们。",
            "",
        ]
        if ordinary.get("reviewed_partial_routes"):
            lines += [
                "人工复核当前前置分派：576进入固定PE的CSS接收器，688/689经重导出的"
                "ObjectGraphReceiver进入原始状态树或显式portable接收器。自动提取器漏识别"
                f"{ordinary['extractor_missed_partial_indices']}；计入这些已核部分入口后为"
                f"{ordinary['routed_including_reviewed_partial_subsets']}/{ordinary['observed']}"
                "个编号有源码路径。这只说明存在分派；实际正目标、对象类型、版本及"
                "全部分支仍有缺口，不能写成全部函数已实现。逐文件哈希保存在JSON。", "",
            ]
    ppi = domains.get("particle_ppi_models")
    if ppi:
        lines += [
            "", "独立PPI效果模型清单：", "",
            f"{ppi['files']}份资源去重{ppi['identities']}个身份；"
            f"{ppi['implemented_source_routes']}个有独立源码模型，"
            f"{ppi['fixed_sdk2_rejected_sdk1']}个有固定宿主拒绝证据；"
            f"仍未归类{ppi['remaining']}个，其中待补SDK2算法{ppi['remaining_sdk2']}个。",
            "此分母独立于普通入口与原版函数；源码模型不等于编译、完整宿主等价或游戏验收。",
            "逐项路径及消费链见particle_ppi_manifest_20261117.json，按snapshot刷新。", "",
        ]
    resolution = domains["content"].get("bounded_unindexed_resolution")
    if resolution:
        lines += [
            "", "历史未索引项的定点收口：", "",
            f"原审计548/551保持历史快照；3项后续定点读取中，{resolution['non_resource_text_files']}项是CP936小说文本，"
            f"{resolution['runtime_fuyin_archives']}项为真实FUYINPAK图片包，含{resolution['runtime_fuyin_entries']}个PNG条目。",
            "FUYINPAK reader已接现有资源索引/范围读取链；"
            + ("ISLAND逐slice CRC/PNG替换已接缓存、ANM及list源码消费者，尚未执行像素匹配验收。"
                if resolution.get("image_replacement_source_implemented") else "图片替换消费者仍在整合。") +
            "没有重扫游戏，不能把该源码交付写成549/551实测结果。",
            f"审计身份匹配：{resolution['audit_snapshot_matches']}；来源：{resolution['source']}。", "",
        ]
    lines += [
        "",
        "原版函数账本的完整等价绑定仍为零；独立source_function_bindings报告提供"
        "入口到源码候选导航，但数字编号一致尚未证明逐版本语义。已有Rust代码具有实际"
        "行为，候选绑定和CFG字节覆盖都不是玩家功能完成率。",
        "",
        "历史内容诊断仍保留原审计时间含义。已有 CST 修复不能由本统计自动升级为"
        "实机通过，也不能继续把旧诊断全称为当前失败。",
        "",
        "剩余主线：",
        "",
    ]
    lines += [f"{index}. {item}" for index, item in enumerate(report["remaining_workstreams"], 1)]
    lines += [
        "",
        "目前没有可辩护的总完成百分比或工时估算。已知仍有入口内部行为/版本缺口、"
        "未完整恢复的原版函数边界和全部游戏的完整流程验收，因此不能称为接近收尾。"
        "只有在调用表、函数身份和实现吞吐有稳定证据后，才可估算剩余工期。",
        "具体剩余见`remaining_to_full_games_20261118.md`。未证样本调用的动态目录、"
        "整机活对象快照等扩展后置，不把历史条件小批合计为当前必修量或全项目工期。",
        "",
        f"源码快照是否仍匹配：`{report['native_source_current']}`。代理继续修改源码时，"
        "保留已发布native分类并标注过期；只在需要重新分类时统一刷新，避免每批重复全扫。",
        "",
        "脚本归 `scripts/analysis/`，报告归 `reports/`；没有临时产物、游戏备份或译文变更。",
        "",
    ]
    return "\n".join(lines)


def main() -> None:
    report = build_report()
    REPORT.with_suffix(".json").write_text(
        json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )
    REPORT.with_suffix(".md").write_text(markdown(report), encoding="utf-8")
    print(json.dumps({
        "native_source_current": report["native_source_current"],
        "domains": report["domains"],
    }, ensure_ascii=True, indent=2))


if __name__ == "__main__":
    main()

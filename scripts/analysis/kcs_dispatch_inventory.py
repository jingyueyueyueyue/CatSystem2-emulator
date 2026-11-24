"""Compare a KCS provenance inventory with the current Rust host tables."""

from __future__ import annotations

import argparse
import json
import re
from collections import Counter
from pathlib import Path

from native_coverage import (
    DESKTOP, ROOT, VERSION_CHANGED, RustSource, external_data_abi,
    external_dispatches, helper_abi, helpers, logging_host_source,
    numbers, possible_routes, rust_mask, source_kind,
)


def argument_table(module: RustSource, name: str, assignment: str) -> dict[int, int]:
    body, _ = module.function(name)
    match = re.search(assignment + r"\bmatch\s+index\s*\{", rust_mask(body))
    if match is None:
        raise ValueError(f"{module.path}: cannot find the argument table for {name}")
    brace = module.functions[name][0] + match.end() - 1
    table = {}
    for arm in module.match_arms(brace):
        if arm.values is None:
            continue
        literal = re.fullmatch(r"(?:Some\((\d+)\)|(\d+))", arm.body.strip())
        if literal is None:
            raise ValueError(f"{module.path}:{arm.line}: argument count is not a literal")
        table.update((index, int(literal[1] or literal[2]) * 4) for index in arm.values)
    return table


def source_coverage(root: Path):
    paths = set((root / "simulator/crates").rglob("*.rs"))
    android = root / "simulator/android/app/src"
    paths.update(path for path in android.rglob("*") if path.suffix in {".java", ".c", ".xml"})
    blobs = {path.relative_to(root).as_posix(): path.read_bytes() for path in paths if path.is_file()}
    ordinary = logging_host_source(blobs[DESKTOP])
    overlays = external_dispatches(root, ordinary, blobs)
    desktop = "simulator/crates/desktop/src/"
    modules = {path.removeprefix(desktop).removesuffix(".rs"): RustSource(
        data.decode("utf-8-sig"), path, require_dispatch=False,
    ) for path, data in sorted(blobs.items()) if path.startswith(desktop) and path != DESKTOP}
    helper_sources = {name: ordinary for name in ordinary.functions}
    for module in modules.values():
        for name in module.functions:
            helper_sources.setdefault(name, module)
    motion = modules["motion_vm"]
    particle = modules["particle_vm"]
    limits, _ = particle.function("native_count")
    version_limits = re.search(r"if\s+self\.version\(\)\s*==\s*(\d+)\s*\{\s*(\d+)\s*\}\s*else\s*\{\s*(\d+)\s*\}", limits)
    if version_limits is None:
        raise ValueError("cannot find the particle version limits")
    particle_version, particle_new, particle_old = map(int, version_limits.groups())
    opcodes = blobs["simulator/crates/script/src/lib.rs"].decode("utf-8-sig")
    declared = re.search(r"const ACTUAL_OPCODES: &\[u8\] = &\[([^\]]+)\]", opcodes)
    if declared is None:
        raise ValueError("cannot find the executable KCS opcode list")
    opcode_set = set(re.findall(r"0x[0-9a-f]+", declared[1]))
    motion_args = argument_table(motion, "native_argument_words", "")
    particle_args = argument_table(particle, "particle_native", r"let\s+words\s*=\s*")
    dispatch = ordinary.mask[ordinary.call_brace:ordinary.call_match]
    version_guard = next((guard for guard in re.finditer(
        r"if matches!\(index, ([0-9. |=]+)\)\s*\{\s*return Err\(cs2_script::ScriptError::UnimplementedNativeCall",
        dispatch,
    ) if numbers(guard[1]) == VERSION_CHANGED), None)
    if version_guard is None:
        raise ValueError("ordinary version gate changed; review pre-dispatch coverage")

    def checked_helper(name: str, *markers: str) -> dict:
        module = helper_sources[name]
        body, line = module.function(name)
        if not all(marker in rust_mask(body) for marker in markers):
            raise ValueError(f"{module.path}:{line}: review special ABI for {name}")
        return {"source": module.path, "line": line}

    movie_control = checked_helper("control_movie_native",
                                   "arguments.is_empty() && (266..=270).contains(&index)",
                                   "!implicit && arguments.len() != expected")
    movie_volume = checked_helper("movie_options_native", "index == 417 && arguments.len() == 4")
    task_restore = checked_helper("task_values_native", "self.task_values_native_inner(index, args)")
    task_restore_inner = checked_helper("task_values_native_inner", "240 | 250 =>",
                                        "let start = read_u32(args, 12)?", "if index == 240 {",
                                        "let count = read_u32(args, 16)?")
    legacy_motion = checked_helper("legacy_utate_motion_properties_native",
                                   "!matches!(index, 299..=302)", "LEGACY_MOTION_GROUP_COUNT) * 4",
                                   "Some(CAMPANELLA_SHA) | Some(TSUNABAN_SHA)")
    compatible = {**{(index, 0): movie_control for index in range(266, 270)},
                  (417, 4): movie_volume,
                  (250, 16): task_restore_inner | {"wrapper": task_restore}}

    special = {}
    for index, helper, description in (
        (576, "restore_css_task_native", "Exact PE wrapper and supported CSS receiver required"),
        (688, "original_state_tree_native", "Exact PE wrapper and supported original or portable graph receiver required"),
        (689, "original_state_tree_native", "Exact PE wrapper and supported original or portable graph receiver required"),
    ):
        module = helper_sources[helper]
        _, line = module.function(helper)
        special[index] = {"status": "conditional-receiver", "source": module.path,
                          "line": line, "condition": description}

    def ordinary_call(index: int, length: int) -> dict:
        if index in special:
            return special[index]
        if (index, length) in compatible:
            return {"status": "source-behavior", **compatible[index, length]}
        if index in {299, 301, 302} and length == 36:
            return {"status": "conditional-version", **legacy_motion,
                    "condition": "Legacy eight-group argument layout supported for verified Utate, Campanella and Tsunaban profiles"}
        if index in VERSION_CHANGED:
            return {"status": "conditional-version", "source": DESKTOP,
                    "line": ordinary.line(ordinary.call_brace + version_guard.start()),
                    "condition": "Exact PE result, E-mote backend, or verified legacy bridge. Otherwise rejected before the numeric match."}
        overlay = next((item for item in overlays if index in item["supported"]), None)
        module = overlay["module"] if overlay else ordinary
        routes = possible_routes(module.arms, index, length)
        if not routes:
            return {"status": "unmapped", "source": module.path}
        arm = routes[0]
        result = {"status": source_kind(arm), "source": module.path, "line": arm.line}
        if (overlay and overlay.get("optional_adapter")) or len(routes) > 1:
            result["status"] = "conditional-dispatch"
            return result
        if "reserved_native::call" in rust_mask(arm.body):
            result["status"] = "conditional-version"
            result["condition"] = "Exact PE constant or preceding concrete receiver required"
            return result
        if result["status"] != "source-behavior":
            return result
        constraints = ([external_data_abi(module, index, arm)] if overlay else [
            helper_abi(helper_sources[name], name, index) for name in helpers(arm)
            if name in helper_sources
        ])
        for abi in constraints:
            exact = abi.get("accepted_argument_bytes")
            minimum = abi.get("minimum_argument_bytes")
            if ((exact is not None and length not in exact)
                    or (minimum is not None and length < minimum)):
                result.update(status="review-ordinary-argument-size", argument_evidence=abi)
                break
        return result

    def classify(table: str, version: int, index: int, length: int) -> dict:
        if table == "ordinary":
            return ordinary_call(index, length)
        if table == "motion":
            expected = motion_args.get(index)
            module = motion
            receiver = "native_argument_words"
        elif table == "particle":
            limit = particle_new if version == particle_version else particle_old
            expected = particle_args.get(index) if index < limit else None
            module = particle
            receiver = "particle_native"
        else:
            return {"status": "unknown-table"}
        status = ("unmapped" if expected is None else
                  "argument-rejected" if expected != length else "source-behavior")
        return {"status": status, "source": module.path, "line": module.function(receiver)[1],
                "expected_argument_bytes": expected}

    return opcode_set, classify


def compare(inventory: dict, root: Path) -> dict:
    implemented_opcodes, classify = source_coverage(root)
    rows = {}
    games = []
    observed_opcodes = set()
    for game in inventory["games"]:
        name = Path(game["root"]).name
        sightings = Counter()
        counts = Counter()
        unknown_opcodes = Counter()
        programs = [program for program in game["programs"] if program["role"] == "runtime-candidate"]
        for program in programs:
            observed_opcodes.update(program["opcodes"])
            unknown_opcodes.update({opcode: count for opcode, count in program["opcodes"].items()
                                    if opcode not in implemented_opcodes})
            for call in program["calls"]:
                if call["index"] is None or call["argument_bytes"] is None:
                    counts["dynamic-call"] += 1
                    continue
                key = (program["table_hint"], program["program_version_raw"], call["index"], call["argument_bytes"])
                if key not in rows:
                    rows[key] = classify(*key) | {"games": {}, "first_sighting": {
                        "game": name, "resource": program["resource"], "offset": call["offset"],
                    }}
                sightings[key] += 1
        for key, sites in sightings.items():
            rows[key]["games"][name] = sites
            counts[rows[key]["status"]] += sites
        games.append({"game": name, "programs": len(programs), "call_status_counts": dict(counts),
                      "unknown_opcodes": dict(unknown_opcodes)})
    rows = [row | dict(zip(("table", "program_version_raw", "index", "argument_bytes"), key))
            for key, row in sorted(rows.items())]
    return {"game_count": len(games), "archives_scanned": inventory["archives_scanned"],
            "runtime_program_count": inventory["runtime_program_count"],
            "observed_opcodes": sorted(observed_opcodes), "games": games, "calls": rows}


def markdown(report: dict) -> str:
    rejected = {"unmapped", "explicit-unimplemented", "argument-rejected", "unknown-table"}
    lines = ["# 全工作区 KCS 宿主分派核对", "",
             f"{report['game_count']} 个游戏目录，{report['archives_scanned']} 个已索引 INT，"
             f"{report['runtime_program_count']} 个运行时 KCS，{len(report['observed_opcodes'])} 个实见 opcode。", "",
             "分析基于物理文件、宿主表路径线索和当前源码。包含未确认实际挂载的补丁副本及未执行分支。"
             "普通宿主的条件路径需要实际 PE、参数和接收对象才能确定结果。", "",
             "各游戏的 `mot.int` 都包含同一套六份 `pt*.kcs`，每份有一次 0 字节 `#23` 调用。"
             "如果送进现在的运动宿主，参数校验会拒绝。已实现的 16 字节 `#23` 是 XYZ 矩阵旋转，"
             "六个粒子控制脚本的原版注册表尚未核实。", "",
             "| 游戏 | KCS | 调用点 | VM 缺少的 opcode | 按表线索无路由或拒绝 | 普通参数待核 |",
             "| --- | ---: | ---: | ---: | ---: | ---: |"]
    for game in report["games"]:
        counts = game["call_status_counts"]
        lines.append(f"| {game['game']} | {game['programs']} | {sum(counts.values())} | "
                     f"{len(game['unknown_opcodes'])} | {sum(counts.get(status, 0) for status in rejected)} | "
                     f"{counts.get('review-ordinary-argument-size', 0)} |")
    lines += ["", "## 需进一步核对的参数组合", "",
              "| 宿主表 | 程序版本 | Native | 参数字节 | 调用点 | 状态 | 源码 |",
              "| --- | ---: | ---: | ---: | ---: | --- | --- |"]
    for row in report["calls"]:
        if row["status"] not in rejected | {"review-ordinary-argument-size", "conditional-receiver"}:
            continue
        lines.append(f"| {row['table']} | {row['program_version_raw']} | {row['index']} | {row['argument_bytes']} | "
                     f"{sum(row['games'].values())} | {row['status']} | `{row.get('source', '')}:{row.get('line', '')}` |")
    return "\n".join(lines) + "\n"


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("inventory", type=Path)
    parser.add_argument("--output", type=Path, required=True, help="Report path without extension.")
    args = parser.parse_args()
    report = compare(json.loads(args.inventory.read_bytes()), ROOT)
    args.output.with_suffix(".json").write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    args.output.with_suffix(".md").write_text(markdown(report), encoding="utf-8")
    print(json.dumps(Counter(row["status"] for row in report["calls"]), sort_keys=True))


if __name__ == "__main__":
    main()

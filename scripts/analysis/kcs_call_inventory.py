"""Preserve KCS call provenance without compiling or executing any program.

Reads local KIF tables and KCS containers. Table classifications are resource
mount hints, not original registration-chain proof; ambiguous updates stay
ambiguous. Dynamic operands are retained instead of disappearing from counts.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import struct
import sys
import zlib
from collections import Counter, defaultdict
from pathlib import Path

sys.dont_write_bytecode = True

from pefile import PEFormatError

from audit_runtime_evidence import blowfish_le_decrypt, operand_count, runtime_fingerprint
from startup_inventory import ROOT, SKIP, entries_at, normalized


REPORT = ROOT / "reports/kcs_call_inventory_20261116"
MAX_BYTES = 256 * 1024 * 1024


def table_hint(resource: str) -> str:
    for part in normalized(resource).replace("::", "/").split("/"):
        if part in {"mot", "mot.int", "motion"}:
            return "motion"
        if part in {"ptcl", "ptcl.int", "particle"}:
            return "particle"
        if part in {"kcs", "kcs.int"}:
            return "ordinary"
    return "unknown"


def parse_program(data: bytes, resource: str) -> dict:
    if len(data) < 52 or data[:4] != b"KCS\0":
        raise ValueError("not a KCS v2 container")
    version, program_version, header_length, code_length = struct.unpack_from("<4I", data, 4)
    init_length = struct.unpack_from("<I", data, 28)[0]
    expected = code_length + init_length
    if version != 2 or header_length != 52 or expected > MAX_BYTES:
        raise ValueError("unsupported KCS header/expansion")
    decoder = zlib.decompressobj()
    body = decoder.decompress(data[52:], expected + 1)
    if len(body) != expected or not decoder.eof:
        raise ValueError("KCS expansion length/stream end mismatch")
    cursor = 0
    calls = []
    opcodes = Counter()
    code = body[:code_length]
    while cursor < len(code):
        offset = cursor
        opcode = code[cursor]
        count = operand_count(opcode)
        if count is None:
            raise ValueError(f"unknown opcode {opcode:#x} at {offset:#x}")
        cursor += 1
        if cursor + count * 5 > len(code):
            raise ValueError(f"truncated instruction at {offset:#x}")
        if opcode == 0x2B:
            first, index, second, size = struct.unpack_from("<BIBI", code, cursor)
            calls.append({
                "offset": offset,
                "index": index if first in (3, 0x23) else None,
                "argument_bytes": size if second in (3, 0x23) else None,
                "index_operand": {"flags": first, "payload": index},
                "argument_operand": {"flags": second, "payload": size},
            })
        opcodes[f"0x{opcode:02x}"] += 1
        cursor += count * 5
    return {
        "resource": resource,
        "role": (
            "excluded-installer-script"
            if normalized(resource) == "inst/inst.kcs"
            else "runtime-candidate"
        ),
        "sha256": hashlib.sha256(data).hexdigest(),
        "program_version_raw": program_version,
        "program_version": f"{program_version & 0xffff}.{program_version >> 16}",
        "table_hint": table_hint(resource),
        "table_evidence": "resource-mount/path-only",
        "instruction_count": sum(opcodes.values()),
        "opcodes": dict(sorted(opcodes.items())),
        "trailing_compressed_bytes": len(decoder.unused_data),
        "calls": calls,
    }


def scan_game(game: dict, *, all_archives: bool = False) -> dict:
    root = Path(game["root"])
    seeds = {normalized(name): int(value.split()[0], 0)
             for name, value in game["archives"]["selected_name_seeds"].items()}
    programs = []
    diagnostics = []
    excluded = SKIP - {"patch", "patches"} if all_archives else SKIP
    paths = [p for p in sorted(root.rglob("*"))
             if p.is_file() and not any(part.lower() in excluded for part in p.relative_to(root).parts)]
    candidate_seeds = {0, *seeds.values()}
    if all_archives:
        for path in paths:
            if path.suffix.lower() != ".exe":
                continue
            try:
                candidate_seeds.add(int(runtime_fingerprint(path)["archive_name_seed"], 16))
            except (ValueError, OSError, PEFormatError):
                continue
    archives_scanned = 0
    for path in paths:
        relative = path.relative_to(root).as_posix()
        suffix = path.suffix.lower()
        if suffix == ".kcs":
            try:
                programs.append(parse_program(path.read_bytes(), relative))
            except (ValueError, zlib.error) as error:
                diagnostics.append({"resource": relative, "message": str(error)})
        if suffix != ".int" or not (all_archives or
            path.stem.lower() in {"kcs", "mot", "ptcl", "ptcldat", "config", "system"}
            or path.stem.lower().startswith("update")
        ):
            continue
        try:
            known_seed = seeds.get(normalized(relative))
            entries, layout, cipher = entries_at(path, known_seed or 0)
            if all_archives and cipher and known_seed is None and len(candidate_seeds) > 1:
                entries, name_seed = recover_archive_names(path, entries, candidate_seeds)
                if name_seed is None:
                    diagnostics.append({"resource": relative, "message": "encrypted archive name seed is ambiguous"})
                    continue
            archives_scanned += 1
            with path.open("rb") as handle:
                for entry in entries:
                    name = entry["name"].decode("cp932", "backslashreplace")
                    if not name.lower().endswith(".kcs"):
                        continue
                    resource = f"{relative}::{name}"
                    if entry["size"] > MAX_BYTES:
                        diagnostics.append({"resource": resource, "message": "KCS exceeds analysis limit"})
                        continue
                    handle.seek(entry["offset"])
                    data = handle.read(entry["size"])
                    if cipher:
                        data = blowfish_le_decrypt(cipher, data)
                    try:
                        program = parse_program(data, resource)
                        program["archive_layout"] = layout
                        programs.append(program)
                    except (ValueError, zlib.error) as error:
                        diagnostics.append({"resource": resource, "message": str(error)})
        except (ValueError, OSError) as error:
            diagnostics.append({"resource": relative, "message": str(error)})
    base_tables = defaultdict(set)
    for program in programs:
        if program["table_hint"] != "unknown":
            basename = normalized(program["resource"]).split("::")[-1].rsplit("/", 1)[-1]
            base_tables[basename].add(program["table_hint"])
    for program in programs:
        if program["table_hint"] != "unknown":
            continue
        basename = normalized(program["resource"]).split("::")[-1].rsplit("/", 1)[-1]
        tables = sorted(base_tables[basename])
        if len(tables) == 1:
            program["table_hint"] = tables[0]
            program["table_evidence"] = "unique-basename-match-to-a-known-mount; registration-not-verified"
        elif tables:
            program["ambiguous_tables"] = tables
    return {
        "root": str(root), "program_count": len(programs), "archives_scanned": archives_scanned,
        "programs": programs, "diagnostics": diagnostics,
    }


def recover_archive_names(path: Path, baseline: list, seeds: set[int]):
    # The payload cipher is independent of the name seed. Use recognizable
    # member extensions as alignment evidence when no per-file seed is known.
    known_suffixes = {
        b"anm", b"cst", b"cstl", b"csv", b"dat", b"fes", b"hg2", b"hg3", b"kcs", b"kx2",
        b"mot", b"mpg", b"ogg", b"png", b"ppi", b"psz", b"ttf", b"txt", b"wav", b"webm", b"wmv", b"xml",
    }
    def score(entries):
        return sum(entry["name"].lower().rsplit(b".", 1)[-1] in known_suffixes for entry in entries)

    guesses = [(score(baseline), 0, baseline)]
    for seed in sorted(seeds - {0}):
        entries, _, _ = entries_at(path, seed)
        guesses.append((score(entries), seed, entries))
    guesses.sort(reverse=True, key=lambda guess: guess[0])
    best, second = guesses[:2]
    if best[0] <= second[0]:
        return baseline, None
    return best[2], best[1]


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--all-games", action="store_true", help="Scan all current games and INT archives.")
    parser.add_argument("--output", type=Path, default=REPORT, help="Output path without extension.")
    args = parser.parse_args()
    audit = json.loads((ROOT / "reports/all_sample_content_audit.json").read_text(encoding="utf-8"))
    sources = audit["games"]
    if args.all_games:
        known = {Path(game["root"]).name: game["archives"] for game in sources}
        sources = [{"root": str(path), "archives": known.get(path.name, {"selected_name_seeds": {}})}
                   for path in sorted((ROOT / "games").iterdir()) if path.is_dir()]
    games = [scan_game(game, all_archives=args.all_games) for game in sources]
    grouped = {}
    for game in games:
        for program in game["programs"]:
            if program["role"] != "runtime-candidate":
                continue
            key = (program["table_hint"], program["program_version"])
            group = grouped.setdefault(key, {
                "table_hint": key[0], "program_version": key[1], "programs": 0,
                "static_call_sites": 0, "dynamic_call_sites": 0,
                "indices": set(), "index_argument_bytes": defaultdict(Counter),
            })
            group["programs"] += 1
            for call in program["calls"]:
                if call["index"] is None or call["argument_bytes"] is None:
                    group["dynamic_call_sites"] += 1
                else:
                    group["static_call_sites"] += 1
                    group["indices"].add(call["index"])
                    group["index_argument_bytes"][str(call["index"])][str(call["argument_bytes"])] += 1
    summary = []
    for key in sorted(grouped):
        group = grouped[key]
        group["indices"] = sorted(group["indices"])
        group["distinct_numeric_indices"] = len(group["indices"])
        group["index_argument_bytes"] = dict(sorted(group["index_argument_bytes"].items(), key=lambda x: int(x[0])))
        summary.append(group)
    result = {
        "schema": "cs2-kcs-call-provenance/v1", "status": "static-no-program-execution",
        "game_count": len(games),
        "archives_scanned": sum(game["archives_scanned"] for game in games),
        "program_count": sum(game["program_count"] for game in games),
        "runtime_program_count": sum(
            program["role"] == "runtime-candidate"
            for game in games for program in game["programs"]
        ),
        "excluded_programs": [
            {"root": game["root"], "resource": program["resource"], "role": program["role"]}
            for game in games for program in game["programs"]
            if program["role"] != "runtime-candidate"
        ],
        "diagnostic_count": sum(len(game["diagnostics"]) for game in games),
        "summary": summary, "games": games,
        "limitations": [
            "Table hints use physical mounts and unique basename matches; original registration is not proven.",
            "All physical patch and loose programs remain visible; this is not an active-resource execution trace.",
            "Only signed/unsigned literal DWORD index and byte counts are promoted to static values.",
            "Dynamic operands and their raw flags are retained; no native or VM is executed.",
        ],
    }
    args.output.with_suffix(".json").write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    lines = [
        "# KCS 调用身份与宿主表清单", "",
        "静态读取 KCS，不编译、不执行 VM 或原版。宿主表是路径/同名资源线索，"
        "尚不能代替原版注册链证据。保留动态操作数、版本、资源和指令偏移。",
        "", "| 表线索 | 程序版本 | 程序 | 数字编号 | 静态调用位置 | 动态调用位置 |",
        "| --- | --- | ---: | ---: | ---: | ---: |",
    ]
    for group in summary:
        lines.append(f"| {group['table_hint']} | {group['program_version']} | {group['programs']} | "
                     f"{group['distinct_numeric_indices']} | {group['static_call_sites']} | {group['dynamic_call_sites']} |")
    lines += ["", f"合计 {result['program_count']} 个程序，{result['diagnostic_count']} 个诊断。"
              f"其中 {result['runtime_program_count']} 个运行时程序候选；"
              "安装目录的 inst/inst.kcs 保留原始记录但排除于运行时统计。"
              "相同数字属于不同表时分别计数；版本内去重不代表跨版本实现相同。", ""]
    args.output.with_suffix(".md").write_text("\n".join(lines), encoding="utf-8")
    print(json.dumps({key: result[key] for key in ("game_count", "archives_scanned", "program_count", "diagnostic_count")},
                     ensure_ascii=True, indent=2))


if __name__ == "__main__":
    main()

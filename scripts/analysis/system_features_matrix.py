"""Group a FES inventory by system feature and retain physical-resource evidence."""

import argparse
from collections import Counter, defaultdict
import json
from pathlib import Path, PurePosixPath, PureWindowsPath
import re


ROOT = Path(__file__).resolve().parents[2]
FES_DISPATCH = (
    "database_native.rs", "emote_settings.rs", "message_native.rs", "system_open_native.rs",
    "window_control.rs",
)
STRING_TOKEN = re.compile(r'(?<![\w"])b?"([A-Za-z_][A-Za-z_0-9.]*)"')
GUARDS = {
    "bail": r"\bbail!\(",
    "native_state_error": r"\bnative_state_error(?:_owned)?\(",
    "unimplemented_native": r"\bUnimplementedNativeCall\b",
    "panic_or_stub": r"\b(?:panic|todo|unimplemented)!\(",
    "unwrap_or_expect": r"\.(?:unwrap|expect)\(",
    "strict_text_candidate": r"\b(?:from_utf8|from_utf16|decode_without_bom_handling_and_without_replacement)\(",
}


DOMAINS = {
    "标题／语种": r"title.*|ttlflow|langselect",
    "历史回看": r"hist.*|_?log.*|sys_log|dialog_bkjump",
    "存读档": r"_?saveload.*|save|load|dave_select|dialog_qdata",
    "设置／快捷键": r"conf(ig)?(_.*)?|conf\d+|shortcut|omake_sysvoice|dlsysvoice",
    "鉴赏入口": r"extra\d*|premium(_st)?|omake|ex_menu|exflow",
    "CG 鉴赏": r"(cgmode|cgview)|(?:prm|ex|omake)_cg.*",
    "背景／立绘": r"(?:prm|ex|omake)_(?:ch|char)(?:\d+|_.*)?|(?:prm|ex|omake)_bg(?:\d+|_.*)?",
    "音乐鉴赏": r"(?:prm|ex|omake)_(?:snd|bgm|music).*|(?:music|mus|snd)mode",
    "影片鉴赏": r"moviemode|(?:prm|ex|omake)_movie.*",
    "场景回想": r"scen(e)?mode|(?:prm|ex|omake)_(?:rec|scn|scene|hscene|ending).*",
    "词典／Tips": r"tips.*|dictionary.*",
    "确认／通知": r"confirm.*|dialog.*|warning|tr_notice.*|inputcheck|info_open",
    "平台联动": r"tweet.*|ex_wall.*|dlwp|act_disc|activate",
}

COMMAND_DOMAINS = {
    "历史回看": r"define:LOGSTRING",
    "存读档": r"define:SAVEPANEL|global:save(?:copy|delete|exchg|apend_str|get_str)",
    "背景／立绘": r"define:(?:BGTABLE2|CHTABLE[23])",
    "词典／Tips": r"global:dic_.*",
    "平台联动": r"global:(?:opensite(?:enc)?|fw_.*)",
}


def features(script):
    stem = PurePosixPath(script["resource"]).stem.lower()
    matches = {label for label, pattern in DOMAINS.items() if re.fullmatch(pattern, stem)}
    for label, pattern in COMMAND_DOMAINS.items():
        if any(re.fullmatch(pattern, command) for command in script["statements"]):
            matches.add(label)
    return matches


def escaped(value):
    return value.replace("|", r"\|").replace("`", "'")


def source_candidates(root):
    desktop = root / "simulator/crates/desktop/src"
    script = root / "simulator/crates/script/src"
    files = {*desktop.glob("fes*.rs"), *(desktop / name for name in FES_DISPATCH),
             script / "scene.rs", *script.glob("fes*.rs"), *script.glob("fes/**/*.rs")}
    references = defaultdict(dict)
    for path in sorted(files):
        for line_no, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
            if line.lstrip().startswith("//"):
                continue
            for match in STRING_TOKEN.finditer(line):
                references[match[1].lower()].setdefault(path.relative_to(root).as_posix(), line_no)
    return references


def dispatch_audit(inventory, references):
    stats = defaultdict(Counter)
    bindings = defaultdict(list)
    for game in inventory["games"]:
        name = PureWindowsPath(game["path"]).name
        for script in game["scripts"]:
            subcommands = script.get("subcommands", {})
            for key, count in script["statements"].items():
                kind, sep, verb = key.partition(":")
                if sep and kind not in {"define", "@"} and not kind.startswith("init/"):
                    stats[key][name] += count
                    if kind != "global" and (binding := subcommands.get(verb)):
                        bindings[key].append({
                            "game": name, "source": script["source"], "occurrences": count, **binding,
                        })

    rows = {}
    for key, counts in sorted(stats.items()):
        _, _, verb = key.partition(":")
        rows[key] = {"occurrences": sum(counts.values()), "games": sorted(counts),
                     "literal_candidates": references.get(verb.lower(), {}),
                     "script_subcommands": bindings.get(key, [])}
    return rows


def guard_candidates(root):
    guards = defaultdict(list)
    patterns = {name: re.compile(pattern) for name, pattern in GUARDS.items()}
    for path in sorted((root / "simulator/crates").glob("*/src/**/*.rs")):
        if path.stem == "tests" or path.stem.endswith("_tests") or "tests" in path.parts:
            continue
        for line_no, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
            if line.lstrip().startswith("//"):
                continue
            for kind, pattern in patterns.items():
                if pattern.search(line):
                    guards[kind].append({"path": path.relative_to(root).as_posix(), "line": line_no})
    return guards


def audit_section(dispatch, guards):
    missing = [(key, row) for key, row in dispatch.items() if not row["literal_candidates"]]
    lines = [
        "## 分派与错误出口索引", "",
        f"扫描 {len(dispatch)} 个全局或具名对象命令键。其中 {len(missing)} 个键在扫描范围内缺少同名源码字面量。",
        "同名匹配提供候选位置。不同接收器可共用命令名，宏、动态命令和继承分派也可能使用其他表示。"
        "下列差集待逐条核对实际命令及接收器。源码行号和各命令的游戏名单见生成的 JSON。", "",
        "| 待核对命令 | 出现次数 | 涉及目录 | 脚本内绑定 |",
        "| --- | ---: | ---: | --- |",
    ]
    for key, row in missing:
        declared = Counter()
        for binding in row["script_subcommands"]:
            declared[binding["target"]] += binding["occurrences"]
        targets = " · ".join(f"`{escaped(target)}` ×{count}" for target, count in sorted(declared.items()))
        lines.append(f"| `{escaped(key)}` | {row['occurrences']} | {len(row['games'])} | {targets} |")
    lines.extend(["", "| 错误或校验的候选写法 | 命中行数 |", "| --- | ---: |"])
    lines.extend(f"| `{kind}` | {len(guards[kind])} |" for kind in GUARDS)
    lines.extend(["", "源码索引按行统计，部分文件内联了测试，分级仍需结合调用契约与实跑证据。", ""])
    return lines


def generate(inventory, audit):
    games = inventory["games"]
    index = []
    details = []
    keys = set()
    for game in games:
        name = escaped(PureWindowsPath(game["path"]).name)
        groups = defaultdict(list)
        counted = 0
        for script in game["scripts"]:
            keys.update(script["statements"])
            labels = features(script)
            counted += bool(labels)
            for label in labels:
                groups[label].append(script)
        index.append(f"| {name} | {len(game['scripts'])} | {counted} | "
                     + " · ".join(f"{label} {len(groups[label])}" for label in DOMAINS if groups[label]) + " |")
        details.extend([f"## {name}", "",
                        "| 功能 | 脚本文件名（物理副本数） | 对象类型及子命令 | 全局指令 |",
                        "| --- | --- | --- | --- |"])
        for label in DOMAINS:
            scripts = groups[label]
            if not scripts:
                continue
            files = Counter(escaped(PurePosixPath(script["resource"]).name) for script in scripts)
            names = ", ".join(f"`{key}` ×{count}" if count > 1 else f"`{key}`"
                              for key, count in sorted(files.items()))
            commands = {key for script in scripts for key in script["statements"]}
            methods = defaultdict(list)
            globals_ = []
            for key in sorted(commands):
                kind, sep, verb = key.partition(":")
                if kind == "global":
                    globals_.append(f"`{escaped(verb)}`")
                elif sep and not kind.startswith("init/"):
                    if kind == "define":
                        methods[verb]
                    else:
                        methods[kind].append(escaped(verb))
            objects = " · ".join(f"`{escaped(kind)}` {', '.join(verbs)}".rstrip()
                                 for kind, verbs in sorted(methods.items()))
            details.append(f"| {label} | {names} | {objects or '—'} | {', '.join(globals_) or '—'} |")
        details.append("")
    display_names = {PurePosixPath(script["resource"]).name.lower()
                     for game in games for script in game["scripts"]}
    return "\n".join([
        "# 系统功能 FES 静态矩阵", "",
        f"扫描覆盖 **{len(games)}** 个目录、**{sum(len(g['scripts']) for g in games)}** 份物理脚本、"
        f"**{len(display_names)}** 个文件名及 **{len(keys)}** 个统计键。",
        "", "功能域依据文件名、对象声明和可识别的全局指令推定。一个文件可能属于多个功能域。"
        "物理副本计入各行，实际入口、挂载优先级、动态接收器与命令执行情况需从回放确认。"
        "完整路径与初始化字段见输入的逐物理文件清单。",
        "", "静态分派核查、错误处置和验收进度见 [系统功能修复台账](system_features_followup_20261126.md)。",
        "", "| 目录 | FES 总数 | 识别出功能的脚本数 | 功能域及脚本数 |", "| --- | ---: | ---: | --- |",
        *index, "", *audit_section(audit["dispatch"], audit["guards"]), *details,
    ])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("inventory", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--audit-json", type=Path)
    args = parser.parse_args()
    inventory = json.loads(args.inventory.read_text(encoding="utf-8"))
    if inventory.get("schema") != "cs2-fes-command-inventory/v1":
        parser.error("expected a FES command inventory v1")
    audit = {
        "schema": "cs2-system-features-static-audit/v1",
        "dispatch": dispatch_audit(inventory, source_candidates(ROOT)),
        "guards": guard_candidates(ROOT),
    }
    args.output.write_text(generate(inventory, audit), encoding="utf-8")
    if args.audit_json:
        args.audit_json.write_text(json.dumps(audit, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()

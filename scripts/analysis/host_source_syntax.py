"""Parse Android host source grammars without a compiler or runtime.

Uses separately installed binary parser wheels. JNI name/calling-convention
macros are substituted only for C grammar parsing; headers and types are not
checked and no compiler, Gradle, application or game is launched.
"""
from __future__ import annotations
import hashlib
import importlib.metadata
import json
from pathlib import Path
import re
import sys

ROOT = Path(__file__).resolve().parents[2]
PARSERS = Path.home() / "AppData/Local/CS2AnalysisPython"
sys.path.insert(0, str(PARSERS))

def main() -> None:
    import tree_sitter
    import tree_sitter_c
    import tree_sitter_java
    inputs = [
        ("java", ROOT / "simulator/android/app/src/main/java/org/catsystem2/player/PlayerActivity.java"),
        ("c", ROOT / "simulator/android/app/src/main/cpp/bridge.c"),
    ]
    records = []
    for language, path in inputs:
        raw = path.read_bytes()
        source = raw.decode("utf-8-sig")
        if language == "c":
            source = re.sub(r"^\s*#.*$", "", source, flags=re.MULTILINE)
            source = source.replace("JNIEXPORT", "").replace("JNICALL", "")
            source = re.sub(r"\bJNI\((\w+)\)", r"Java_org_catsystem2_player_PlayerActivity_\1", source)
        capsule = tree_sitter_java.language() if language == "java" else tree_sitter_c.language()
        parser = tree_sitter.Parser(tree_sitter.Language(capsule))
        tree = parser.parse(source.encode("utf-8"))
        pending = [tree.root_node]
        errors = []
        while pending:
            node = pending.pop()
            if node.type == "ERROR" or node.is_missing:
                errors.append({
                    "kind": node.type, "missing": node.is_missing,
                    "line": node.start_point.row + 1, "column": node.start_point.column + 1,
                    "end_line": node.end_point.row + 1,
                })
            pending.extend(reversed(node.children))
        records.append({
            "source": path.relative_to(ROOT).as_posix(),
            "sha256": hashlib.sha256(raw).hexdigest(), "language": language,
            "grammar_errors": errors, "root_has_error": tree.root_node.has_error,
            "preprocessing": "known JNI macros only" if language == "c" else "none",
        })
    report = {
        "status": "source-grammar-only-not-types-linking-or-game-acceptance",
        "compiler_invoked": False, "tests_run": False, "device_modified": False,
        "parser_wheels": {name: importlib.metadata.version(name)
                          for name in ["tree-sitter", "tree-sitter-java", "tree-sitter-c"]},
        "sources": records,
    }
    output = ROOT / "archive/2026-11/20261117_android_host_grammar.json"
    output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"report": output.relative_to(ROOT).as_posix(),
                      "grammar_errors": sum(len(row["grammar_errors"]) for row in records),
                      "compiler_invoked": False}, indent=2))
    if any(row["root_has_error"] for row in records):
        raise SystemExit(1)

if __name__ == "__main__":
    main()

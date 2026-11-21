"""Read source grammars only: no compiler, VM, network or device invocation."""
from __future__ import annotations
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(Path.home() / "AppData/Local/CS2AnalysisPython"))

def main():
    import tree_sitter
    import tree_sitter_c
    import tree_sitter_java
    files = [("java", "simulator/android/app/src/main/java/org/catsystem2/player/" + name)
             for name in ("PlayerActivity.java", "AndroidTweetHost.java", "TweetActivity.java")]
    files += [("c", "simulator/android/app/src/main/cpp/bridge.c")]
    rows = []
    for language, name in files:
        path = ROOT / name
        raw = path.read_bytes()
        source = raw.decode("utf-8-sig")
        if language == "c":
            source = re.sub(r"^\s*#.*$", "", source, flags=re.MULTILINE)
            source = source.replace("JNIEXPORT", "").replace("JNICALL", "")
            source = re.sub(r"\bJNI\((\w+)\)", r"Java_org_catsystem2_player_PlayerActivity_\1", source)
        capsule = tree_sitter_java.language() if language == "java" else tree_sitter_c.language()
        tree = tree_sitter.Parser(tree_sitter.Language(capsule)).parse(source.encode("utf8"))
        pending = [tree.root_node]
        errors = []
        while pending:
            node = pending.pop()
            if node.type == "ERROR" or node.is_missing:
                errors.append({"kind": node.type, "line": node.start_point.row + 1,
                               "column": node.start_point.column + 1})
            pending.extend(reversed(node.children))
        rows.append({"path": name, "sha256": hashlib.sha256(raw).hexdigest(),
                     "grammar_errors": errors, "parse": not tree.root_node.has_error})
    formatter = Path.home() / ".rustup/toolchains/stable-x86_64-pc-windows-gnu/bin/rustfmt.exe"
    for name in ["tweet_native.rs", "tweet_worker.rs", "tweet_platform.rs", "tweet_host.rs", "lib.rs"]:
        path = ROOT / "simulator/crates/desktop/src" / name
        raw = path.read_bytes()
        parsed = subprocess.run([str(formatter), "--edition", "2024", "--config",
            "skip_children=true", "--emit", "stdout"], input=raw, capture_output=True)
        rows.append({"path": path.relative_to(ROOT).as_posix(), "sha256": hashlib.sha256(raw).hexdigest(),
                     "parse": parsed.returncode == 0, "unchanged": raw == path.read_bytes(),
                     "diagnostic": parsed.stderr.decode("utf8", errors="replace")})
    for name in ["simulator/crates/android/src/lib.rs", "simulator/crates/formats/src/tweet_credentials.rs",
                 "simulator/crates/formats/src/lib.rs", "simulator/crates/formats/src/archive.rs"]:
        path = ROOT / name
        raw = path.read_bytes()
        parsed = subprocess.run([str(formatter), "--edition", "2024", "--config", "skip_children=true",
                                 "--emit", "stdout"], input=raw, capture_output=True)
        rows.append({"path": path.relative_to(ROOT).as_posix(), "sha256": hashlib.sha256(raw).hexdigest(),
                     "parse": parsed.returncode == 0, "unchanged": raw == path.read_bytes(),
                     "diagnostic": parsed.stderr.decode("utf8", errors="replace")})
    output = ROOT / "archive/2026-11/20261117_format_B_android_tweet_syntax.json"
    output.write_text(json.dumps({"compiler_invoked": False, "type_checked": False,
        "tests_run": False, "network_accessed": False, "device_modified": False, "files": rows}, indent=2)+"\n", encoding="utf8")
    print(json.dumps({"report": output.relative_to(ROOT).as_posix(),
                      "files": len(rows), "parse_failures": [r for r in rows if not r["parse"]]}, indent=2))
    if any(not r["parse"] for r in rows): raise SystemExit(1)

if __name__ == "__main__": main()

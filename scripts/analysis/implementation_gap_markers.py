"""Inventory explicit recovery messages in current production Rust sources.

Static source navigation only. A message can describe an unused/invalid input,
so neither messages nor enclosing functions are counts of missing features.
No compiler, VM, game, resource scan or device access is performed.
"""
from __future__ import annotations

import hashlib
import json
import re
from collections import Counter
from datetime import datetime
from pathlib import Path
from zoneinfo import ZoneInfo

from native_coverage import archive_source_blobs, matching, rust_mask

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "archive/2026-11/20261117_implementation_gap_markers.json"
GAP = re.compile(
    r"not (?:yet )?(?:implemented|recovered)|unrecovered|no recovered|"
    r"requires its unrecovered|remain(?:s)? unsupported", re.I)


def string_literals(source: str):
    """Skip comments/character literals and return actual Rust string tokens."""
    at = 0
    while at < len(source):
        if source.startswith("//", at):
            end = source.find("\n", at)
            at = len(source) if end < 0 else end
        elif source.startswith("/*", at):
            at += 2
            depth = 1
            while at < len(source) and depth:
                if source.startswith("/*", at):
                    depth += 1
                    at += 2
                elif source.startswith("*/", at):
                    depth -= 1
                    at += 2
                else:
                    at += 1
        else:
            raw = re.match(r'(?:br|r)(#{0,255})"', source[at:at + 260]) if source[at] in "br" else None
            if raw:
                finish = '"' + raw[1]
                end = source.find(finish, at + raw.end())
                if end < 0:
                    raise ValueError("unterminated raw string")
                yield at, source[at + raw.end():end]
                at = end + len(finish)
            elif source[at] == '"':
                start = at
                at += 1
                while at < len(source):
                    if source[at] == "\\":
                        at += 2
                    elif source[at] == '"':
                        yield start, source[start + 1:at]
                        at += 1
                        break
                    else:
                        at += 1
            elif source[at] == "'" and (char := re.match(
                r"'(?:\\(?:u\{[0-9a-fA-F_]+\}|x[0-9a-fA-F]{2}|.)|[^'\\\n])'", source[at:])):
                at += char.end()
            else:
                at += 1


def main():
    blobs = {}
    rows = []
    for path in sorted((ROOT / "simulator/crates").glob("*/src/**/*.rs")):
        if any(part in {"tests", "test"} for part in path.parts):
            continue
        name = path.relative_to(ROOT).as_posix()
        raw = path.read_bytes()
        blobs[name] = raw
        source = raw.decode("utf-8-sig")
        mask = rust_mask(source)
        test = re.search(r"#\s*\[\s*cfg\s*\(\s*test\s*\)\s*\]\s*mod\s+tests\b", mask)
        end = test.start() if test else len(source)
        functions = []
        for found in re.finditer(r"\bfn\s+(\w+)\s*\(", mask[:end]):
            opening = mask.find("(", found.start())
            arguments_end = matching(mask, opening)
            brace = mask.find("{", arguments_end)
            semicolon = mask.find(";", arguments_end)
            if brace >= 0 and not (0 <= semicolon < brace):
                functions.append((found[1], brace, matching(mask, brace)))
        for offset, value in string_literals(source[:end]):
            if not GAP.search(value):
                continue
            if re.search(r"\.contains\s*\(\s*$", mask[max(0, offset - 80):offset]):
                # Log classifiers quote an error; they do not reject a feature.
                continue
            owner = next((name for name, start, stop in reversed(functions) if start < offset < stop), None)
            if owner is None:
                # Excludes error type display templates and descriptor tables.
                continue
            rows.append({"path": name, "line": source.count("\n", 0, offset) + 1,
                         "function": owner, "message": value,
                         "sample_trigger_confirmed": None})
    hashes = {name: hashlib.sha256(raw).hexdigest() for name, raw in blobs.items()}
    archive = archive_source_blobs(ROOT, blobs, hashes)
    functions = {(row["path"], row["function"]) for row in rows}
    result = {
        "generated_at": datetime.now(ZoneInfo("Asia/Shanghai")).isoformat(timespec="seconds"),
        "scope": "explicit recovery-message navigation, not remaining-function count",
        "method": "current production Rust literals; comments and trailing test modules excluded",
        "limitations": "Does not count missing code without messages; generic fallbacks and unused inputs remain candidates.",
        "summary": {"source_files": len(blobs), "messages": len(rows),
                    "enclosing_functions": len(functions),
                    "by_crate": dict(Counter(row["path"].split('/')[2] for row in rows))},
        "sources": [{"path": name, "sha256": hashes[name], "archive": archive[name]} for name in blobs],
        "markers": rows,
    }
    OUT.write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(result["summary"], ensure_ascii=True))


if __name__ == "__main__":
    main()

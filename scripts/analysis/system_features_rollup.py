"""Collect PC system replay receipts and group host diagnostics across batches."""

import argparse
from collections import Counter, defaultdict
import json
from pathlib import Path
import re

from run_playthrough_batch import REPLAY_EVENT_LINE


DIAGNOSTIC_EVENTS = {
    "vm-error", "task-error", "save-file-error", "settings-error", "system-open-error", "image-capture-error",
    "engine_tick_stalled", "frame-capture-failed",
}
SCRIPT = re.compile(r'\b(?:script|fes)=["\']?([^"\'\s]+\.fes)\b', re.IGNORECASE)
PC = re.compile(r"\bpc=(0x[0-9a-f]+)", re.IGNORECASE)
NATIVE = re.compile(r"\bnative(?: call| index=|_index=| slot=)?\s*(\d+)\b", re.IGNORECASE)
CONTEXT_PREFIX = re.compile(r"^(?:FES )?task=\d+:\s*|^pc=0x[0-9a-f]+ .*? (?:frames=\d+)\s*", re.IGNORECASE)


def diagnostics(path):
    with path.open(encoding="utf-8", errors="replace") as stream:
        for line in stream:
            match = REPLAY_EVENT_LINE.match(line)
            if not match:
                continue
            frame, event = match.groups()
            kind, _, detail = event.partition(" ")
            if kind == "native-call" and "is not implemented" in detail:
                kind = "unimplemented-native"
            elif kind not in DIAGNOSTIC_EVENTS:
                continue
            location = []
            for regex in (SCRIPT, PC, NATIVE):
                found = regex.search(detail)
                location.append(found[1] if found else None)
            signature = CONTEXT_PREFIX.sub("", detail, count=1)
            yield int(frame), (kind, signature, *location)


def collect(batches):
    receipts = {}
    for batch in batches:
        for log in batch.rglob("*.events.log"):
            receipt = log.with_name(log.name.removesuffix(".events.log") + ".json")
            if receipt.is_file():
                receipts[receipt.resolve()] = log.resolve()
        for receipt in batch.glob("[0-9][0-9].json"):
            receipts.setdefault(receipt.resolve(), None)
    if not receipts:
        raise ValueError("no PC replay receipts found")

    cases = []
    groups = defaultdict(list)
    for receipt, log in sorted(receipts.items()):
        data = json.loads(receipt.read_text(encoding="utf-8"))
        cases.append({
            "receipt": str(receipt), "index": data["index"], "status": data["status"],
            "exit_code": data.get("exit_code"),
        })
        if log is not None:
            for frame, key in diagnostics(log):
                groups[key].append({"log": str(log), "frame": frame})

    return {
        "cases": cases,
        "statuses": dict(Counter(case["status"] for case in cases)),
        "nonzero_exits": [case for case in cases if case["exit_code"] not in (0, None)],
        "diagnostics": [
            {"kind": key[0], "signature": key[1], "fes": key[2], "pc": key[3], "native": key[4],
             "count": len(occurrences), "occurrences": occurrences}
            for key, occurrences in sorted(groups.items(), key=lambda item: (-len(item[1]), str(item[0])))
        ],
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("batch", type=Path, nargs="+")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    try:
        for batch in args.batch:
            if not batch.is_dir():
                raise ValueError(f"batch directory does not exist: {batch}")
        report = collect(args.batch)
        with args.output.open("x", encoding="utf-8") as stream:
            json.dump(report, stream, ensure_ascii=False, indent=2)
            stream.write("\n")
    except (OSError, ValueError, KeyError) as error:
        parser.error(str(error))
    print(f"{len(report['cases'])} receipts, {len(report['diagnostics'])} diagnostic groups")


if __name__ == "__main__":
    main()

"""Audit UI-input to actual Vulkan-present latency by input_id."""

from __future__ import annotations

import argparse
from collections import Counter
import json
import math
from pathlib import Path
import re


PREFIX = re.compile(r"^.*?\bCS2Perf[ \t]*:[ \t]*", re.MULTILINE)
EVENTS = {
    "primary_down", "primary_down_timing", "input_photon", "input_photon_unresolved",
}
EXIT_CODES = {"PASS": 0, "FAIL": 1, "UNMEASURABLE": 3, "INCOMPLETE": 4}


def summary(values: list[int]) -> dict | None:
    if not values:
        return None
    ordered = sorted(values)
    count = len(ordered)

    def at(percent: int) -> int:
        return ordered[math.ceil(count * percent / 100) - 1]

    return {
        "samples": count,
        "mean_ms": sum(ordered) / count / 1000,
        "p50_ms": at(50) / 1000,
        "p95_ms": at(95) / 1000,
        "p99_ms": at(99) / 1000,
        "max_ms": ordered[-1] / 1000,
    }


def records(log: str, session: int):
    for line in PREFIX.sub("", log).splitlines():
        kind, _, fields = line.partition(" ")
        if kind not in EVENTS:
            continue
        record = {"kind": kind}
        for field in fields.split():
            name, separator, value = field.partition("=")
            if separator:
                record[name] = int(value) if name != "reason" else value
        if record.get("session") == session:
            if not isinstance(record.get("input_id"), int) or record["input_id"] <= 0:
                raise ValueError(f"{kind} has no positive input_id; capture a complete current-format session")
            yield record


def unique(record_map: dict[int, dict], record: dict) -> None:
    input_id = record["input_id"]
    if input_id in record_map:
        raise ValueError(f"Duplicate or conflicting input_id {input_id}")
    record_map[input_id] = record


def report(log: str, session: int, p95_limit_ms: float, allow_unchanged: bool) -> dict:
    enqueued, consumed, resolved, unresolved = {}, {}, {}, {}
    for record in records(log, session):
        destination = {
            "primary_down": enqueued,
            "primary_down_timing": consumed,
            "input_photon": resolved,
            "input_photon_unresolved": unresolved,
        }[record["kind"]]
        unique(destination, record)
    for name, subset in (("consumed", consumed), ("resolved", resolved), ("unresolved", unresolved)):
        unknown = sorted(set(subset) - set(enqueued))
        if unknown:
            raise ValueError(f"{name} input IDs were not enqueued: {unknown}")
    resolved_ids = set(resolved)
    unresolved_ids = set(unresolved)
    if resolved_ids & unresolved_ids:
        raise ValueError("An input_id is both resolved and unresolved")
    consumed_ids = set(consumed)
    if (resolved_ids | unresolved_ids) - consumed_ids:
        raise ValueError("A resolved input_id has no consumption record")

    samples = []
    for input_id, record in resolved.items():
        start = enqueued[input_id]["input_mono_us"]
        if record["input_mono_us"] != start or record["input_to_photon_us"] < 0:
            raise ValueError(f"Input {input_id} has inconsistent start or latency")
        event_to_ui_ms = consumed[input_id]["event_to_ui_ms"]
        samples.append({
            "input_id": input_id,
            "input_to_photon_us": record["input_to_photon_us"],
            "event_to_photon_us": (
                record["input_to_photon_us"] + event_to_ui_ms * 1000
                if event_to_ui_ms >= 0 else None
            ),
            "submit_to_photon_us": record["submit_to_photon_us"],
            "ui_to_jni_us": enqueued[input_id].get("ui_to_jni_us"),
            "mailbox_us": consumed[input_id]["ui_to_worker_us"],
            "present_id": record["present_id"],
            "actual_present_ns": record["actual_present_ns"],
        })
    samples.sort(key=lambda sample: sample["input_id"])
    pending = sorted(consumed_ids - resolved_ids - unresolved_ids)
    not_consumed = sorted(set(enqueued) - consumed_ids)
    reasons = Counter(record["reason"] for record in unresolved.values())
    accepted_unresolved = {"unchanged"} if allow_unchanged else set()
    incomplete = bool(pending or not_consumed or set(reasons) - accepted_unresolved)
    metrics = {
        field: summary([sample[field] for sample in samples if sample[field] is not None])
        for field in (
            "input_to_photon_us", "event_to_photon_us", "submit_to_photon_us",
            "ui_to_jni_us", "mailbox_us",
        )
    }
    photon = metrics["input_to_photon_us"]
    if photon is None:
        status = "UNMEASURABLE"
    elif incomplete:
        status = "INCOMPLETE"
    else:
        status = "PASS" if photon["p95_ms"] <= p95_limit_ms else "FAIL"
    return {
        "schema": "cs2-input-photon/v1",
        "session": session,
        "status": status,
        "p95_limit_ms": p95_limit_ms,
        "allow_unchanged": allow_unchanged,
        "counts": {
            "enqueued": len(enqueued), "consumed": len(consumed), "resolved": len(resolved),
            "unresolved": len(unresolved), "pending": len(pending), "not_consumed": len(not_consumed),
        },
        "unresolved_reasons": dict(sorted(reasons.items())),
        "pending_input_ids": pending,
        "not_consumed_input_ids": not_consumed,
        "metrics": metrics,
        "samples": samples,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("log", type=Path)
    parser.add_argument("--session", type=int, required=True)
    parser.add_argument("--p95-limit-ms", type=float, default=70)
    parser.add_argument("--allow-unchanged", action="store_true")
    args = parser.parse_args()
    if not math.isfinite(args.p95_limit_ms) or args.p95_limit_ms <= 0:
        parser.error("p95 limit must be finite and positive")
    try:
        result = report(
            re.sub(r"\r\n?", "\n", args.log.read_text(encoding="utf-8-sig")),
            args.session, args.p95_limit_ms, args.allow_unchanged,
        )
        print(json.dumps(result, ensure_ascii=False, indent=2))
        return EXIT_CODES[result["status"]]
    except (OSError, ValueError, KeyError) as error:
        parser.exit(2, f"{error}\n")


if __name__ == "__main__":
    raise SystemExit(main())

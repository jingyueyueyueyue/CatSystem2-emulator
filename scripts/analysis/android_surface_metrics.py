"""Measure actual-present intervals from one SurfaceView and one replay phase."""

from __future__ import annotations

import argparse
import codecs
import json
import math
from pathlib import Path
import re
import shlex
import subprocess
import time


PENDING_NS = (1 << 63) - 1
LIMIT_NS = 33_000_000
NS_PER_MS = 1_000_000
NS_PER_SECOND = 1_000_000_000
SUSTAINED_FPS = 59
SUSTAINED_P95_NS = 16_800_000
SUSTAINED_P99_NS = 25_100_000
SUSTAINED_DURATION_NS = 600 * NS_PER_SECOND
HOT_DURATION_NS = 120 * NS_PER_SECOND
REFRESH_LINE = re.compile(r"[0-9]{1,19}")
LATENCY_ROW = re.compile(r"[+-]?[0-9]{1,19}\s+([+-]?[0-9]{1,19})\s+[+-]?[0-9]{1,19}")
EXIT_CODES = {"PASS": 0, "FAIL": 1, "UNMEASURABLE": 3}


def parse_snapshots(raw: bytes) -> list[set[int]]:
    has_utf16_bom = raw.startswith((codecs.BOM_UTF16_LE, codecs.BOM_UTF16_BE))
    text = raw.decode("utf-16" if has_utf16_bom else "utf-8-sig")
    snapshots: list[set[int]] = []
    for raw_line in text.splitlines():
        line = raw_line.strip()
        if REFRESH_LINE.fullmatch(line):
            snapshots.append(set())
        elif snapshots and (row := LATENCY_ROW.fullmatch(line)):
            actual_ns = int(row[1])
            if 0 < actual_ns < PENDING_NS:
                snapshots[-1].add(actual_ns)
    return snapshots or [set()]


def percentile_ns(sorted_intervals: list[int], percent: int) -> int | None:
    rank = (percent * len(sorted_intervals) + 99) // 100
    return sorted_intervals[rank - 1] if rank else None


def milliseconds(nanoseconds: int | None) -> float | None:
    return nanoseconds / NS_PER_MS if nanoseconds is not None else None


def summarize(snapshots: list[set[int]], min_samples: int, profile: str = "33ms") -> dict:
    """Compute intervals only where the rolling histories give continuous coverage."""
    timestamps = sorted({timestamp for snapshot in snapshots for timestamp in snapshot})
    windows = iter(sorted((min(snapshot), max(snapshot)) for snapshot in snapshots if snapshot))
    window = next(windows, None)
    covered_until = 0
    covered_intervals = []
    uncovered = 0
    for previous, current in zip(timestamps, timestamps[1:]):
        while window is not None and window[0] <= previous:
            covered_until = max(covered_until, window[1])
            window = next(windows, None)
        if current <= covered_until:
            covered_intervals.append((current, current - previous))
        else:
            uncovered += 1

    intervals = sorted(interval for _, interval in covered_intervals)
    sample_count = len(intervals)
    span_ns = timestamps[-1] - timestamps[0] if timestamps else 0
    p95_ns = percentile_ns(intervals, 95)
    p99_ns = percentile_ns(intervals, 99)
    reason = None
    if uncovered:
        status = "UNMEASURABLE"
        reason = "Snapshot histories do not cover the full span between recorded presents."
    elif p95_ns is None or sample_count < min_samples:
        status = "UNMEASURABLE"
        reason = "Too few valid present-to-present intervals."
    else:
        status = "PASS" if p95_ns <= LIMIT_NS else "FAIL"

    within_limit = sum(interval <= LIMIT_NS for interval in intervals)
    metrics = {
        "status": status,
        "reason": reason,
        "snapshot_count": len(snapshots),
        "empty_snapshot_count": sum(not snapshot for snapshot in snapshots),
        "present_count": len(timestamps),
        "present_span_ms": milliseconds(span_ns) if timestamps else None,
        "sample_count": sample_count,
        "min_samples": min_samples,
        "uncovered_interval_count": uncovered,
        "p50_ms": milliseconds(percentile_ns(intervals, 50)),
        "p95_ms": milliseconds(p95_ns),
        "p99_ms": milliseconds(p99_ns),
        "max_ms": milliseconds(intervals[-1] if intervals else None),
        "share_le_33_ms": within_limit / sample_count if sample_count else None,
    }
    if profile == "60hz":
        metrics.update(
            profile=profile,
            actual_fps=sample_count * NS_PER_SECOND / span_ns if span_ns and not uncovered else None,
            min_duration_seconds=SUSTAINED_DURATION_NS // NS_PER_SECOND,
            min_actual_fps=SUSTAINED_FPS,
            max_p95_ms=milliseconds(SUSTAINED_P95_NS),
            max_p99_ms=milliseconds(SUSTAINED_P99_NS),
            share_le_16_8_ms=(
                sum(interval <= SUSTAINED_P95_NS for interval in intervals) / sample_count
                if sample_count else None
            ),
            final_two_minutes=None,
        )
        if reason is None:
            if span_ns < SUSTAINED_DURATION_NS:
                metrics.update(status="UNMEASURABLE", reason="The 60 Hz profile requires a continuous ten-minute capture.")
            else:
                passed = (
                    sample_count * NS_PER_SECOND >= SUSTAINED_FPS * span_ns
                    and p95_ns <= SUSTAINED_P95_NS
                    and p99_ns <= SUSTAINED_P99_NS
                )
                metrics["status"] = "PASS" if passed else "FAIL"
        if span_ns >= HOT_DURATION_NS and not uncovered:
            cutoff = timestamps[-1] - HOT_DURATION_NS
            # Count all presents after the cutoff and keep their full preceding intervals.
            hot = sorted(interval for end, interval in covered_intervals if end > cutoff)
            metrics["final_two_minutes"] = {
                "sample_count": len(hot),
                "actual_fps": len(hot) * NS_PER_SECOND / HOT_DURATION_NS,
                "p95_ms": milliseconds(percentile_ns(hot, 95)),
                "p99_ms": milliseconds(percentile_ns(hot, 99)),
                "max_ms": milliseconds(hot[-1] if hot else None),
            }
    return metrics


def capture(args: argparse.Namespace) -> None:
    if args.output.exists():
        raise FileExistsError(f"snapshot file already exists: {args.output}")
    if not args.output.parent.is_dir():
        raise FileNotFoundError(f"output parent directory does not exist: {args.output.parent}")
    remote = f"dumpsys SurfaceFlinger --latency {shlex.quote(args.layer)}"
    command = [args.adb, "-s", args.serial, "shell", "-T", remote]
    snapshots = []
    for index in range(1, args.count + 1):
        try:
            result = subprocess.run(command, stdin=subprocess.DEVNULL, capture_output=True, timeout=10)
        except subprocess.TimeoutExpired as error:
            raise RuntimeError(f"snapshot {index}: adb timed out after 10 seconds") from error
        first_line = result.stdout.partition(b"\n")[0].strip()
        if result.returncode or not first_line.isdigit():
            details = (result.stderr or result.stdout).decode("utf-8", errors="replace").strip()
            raise RuntimeError(
                f"snapshot {index}: no SurfaceFlinger latency dump (adb exit {result.returncode}): {details}"
            )
        snapshots.append(result.stdout)
        if index < args.count:
            time.sleep(args.interval_seconds)

    with args.output.open("xb") as destination:
        destination.write(b"\n\n".join(snapshots))


def positive_int(value: str) -> int:
    number = int(value)
    if number < 1:
        raise argparse.ArgumentTypeError("expected a positive integer")
    return number


def positive_seconds(value: str) -> float:
    number = float(value)
    if not math.isfinite(number) or number <= 0:
        raise argparse.ArgumentTypeError("expected a finite, positive number of seconds")
    return number


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    modes = parser.add_subparsers(dest="mode", required=True)
    report = modes.add_parser(
        "report",
        help="read saved latency snapshots without using adb",
        description="Use one exact layer, device boot and replay phase per report. "
        "The dump contains rolling history and may include frames before capture started.",
        epilog="Exit codes: 0 PASS, 1 FAIL, 3 UNMEASURABLE, 2 command error.",
    )
    report.add_argument("snapshots", nargs="+", type=Path, help="files with one or more raw latency dumps")
    report.add_argument("--min-samples", type=positive_int, default=1, help="minimum valid intervals (default: 1)")
    report.add_argument(
        "--profile", choices=("33ms", "60hz"), default="33ms",
        help="33ms checks p95 <= 33 ms; 60hz checks ten minutes at >= 59 FPS, p95 <= 16.8 ms and p99 <= 25.1 ms",
    )

    collect = modes.add_parser("capture", help="poll latency on an explicitly selected device")
    collect.add_argument("--serial", required=True, help="adb device serial")
    collect.add_argument("--layer", required=True, help="exact SurfaceView layer name")
    collect.add_argument("--output", type=Path, required=True, help="new file for raw snapshots")
    collect.add_argument("--adb", default="adb", help="adb executable (default: adb on PATH)")
    collect.add_argument("--count", type=positive_int, default=12, help="number of snapshots (default: 12)")
    collect.add_argument("--interval-seconds", type=positive_seconds, default=0.5, help="delay between polls (default: 0.5)")
    args = parser.parse_args(argv)

    try:
        if args.mode == "capture":
            if not args.serial.strip() or not args.layer.strip():
                collect.error("--serial and --layer must be nonempty")
            capture(args)
            print(f"Saved {args.count} latency snapshots to {args.output}")
            return 0

        snapshots = [snapshot for path in args.snapshots for snapshot in parse_snapshots(path.read_bytes())]
        metrics = summarize(snapshots, args.min_samples, args.profile)
        print(json.dumps(metrics, indent=2))
        return EXIT_CODES[metrics["status"]]
    except (OSError, RuntimeError, UnicodeError) as error:
        parser.exit(2, f"{parser.prog}: error: {error}\n")


if __name__ == "__main__":
    raise SystemExit(main())

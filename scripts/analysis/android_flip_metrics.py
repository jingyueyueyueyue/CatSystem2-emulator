"""Bound the time from native primary_down to a verified text change (Pillow)."""

from __future__ import annotations

import argparse
import csv
from dataclasses import asdict, dataclass
import json
from pathlib import Path
import re
import sys

from PIL import Image


LIMIT_NS = 150_000_000
CAPTURE_FIELDS = ["seq", "png", "start_ns", "end_ns"]
PRIMARY_DOWN = re.compile(r"\bprimary_down session=(\d+) input_mono_us=(\d+)\s*$")
EXIT_CODES = {"PASS": 0, "FAIL": 1, "INSUFFICIENT_SAMPLES": 3, "WINDOW_UNCERTAIN": 4}
MANIFEST_HELP = """Example manifest (paths relative to the manifest):
{
  "clock": "CLOCK_MONOTONIC",
  "capture_timing": "Describe device-side timing source and how bounds were obtained",
  "tap_log": "cs2perf.log", "session": 1, "input_mono_us": 1000000,
  "roi": [120, 780, 400, 830],
  "reference": {
    "before_png": "old.png", "after_png": "new.png",
    "before_text": "old line", "after_text": "new line", "reviewed": true
  },
  "samples": [
    {"png": "sample_00.png", "start_ns": 980000000, "end_ns": 985000000},
    {"png": "sample_01.png", "start_ns": 1050000000, "end_ns": 1060000000},
    {"png": "sample_02.png", "start_ns": 1090000000, "end_ns": 1100000000}
  ]
}

Use one isolated input and one stable old-to-new text transition, with auto/skip
disabled. Review both references by hand and choose a fixed crop of differing
glyphs clear of cursors, buttons and moving background. Samples match the
reference crop exactly. Other images are unknown, including partial text.

Pass Euler's raw CS2Perf log (or a prefixed logcat export). input_mono_us comes
from native primary_down and is truncated to microseconds. Supply ascending,
non-overlapping bounds in nanoseconds on the same device CLOCK_MONOTONIC and
boot. Bounds must enclose acquisition of the currently displayed frame, with
capture uncertainty included. For video, document the PTS-to-clock anchor and
its uncertainty in capture_timing and supply the resulting bounds per PNG.
Host request/return times, unanchored PTS and CLOCK_BOOTTIME cannot be used.

Exit codes: 0 PASS (upper bound <= 150 ms), 1 FAIL (lower bound > 150 ms),
2 invalid input, 3 insufficient samples, 4 uncertain window.
"""


@dataclass(frozen=True)
class Sample:
    png: str
    start_ns: int
    end_ns: int
    state: str


def integer(value: object, name: str) -> int:
    if type(value) is not int or value < 0:
        raise ValueError(f"{name} must be a nonnegative integer")
    return value


def nonempty(value: object, name: str) -> str:
    if not isinstance(value, str) or not value.strip():
        raise ValueError(f"{name} must be a nonempty string")
    return value.strip()


def png_crop(path: Path, roi: tuple[int, ...], expected_size: tuple[int, int] | None = None):
    with Image.open(path, formats=["PNG"]) as image:
        if getattr(image, "n_frames", 1) != 1:
            raise ValueError(f"{path}: expected a single-frame PNG")
        if expected_size is not None and image.size != expected_size:
            raise ValueError(f"{path}: expected {expected_size}, got {image.size}")
        left, top, right, bottom = roi
        if not (left < right <= image.width and top < bottom <= image.height):
            raise ValueError(f"{path}: ROI is outside the PNG or empty")
        if roi == (0, 0, *image.size):
            raise ValueError("Choose a focused text ROI within the frame")
        return image.size, image.crop(roi).convert("RGBA").tobytes()


def read_samples(raw_samples: list, directory: Path, roi: tuple[int, ...], size: tuple[int, int],
                 before: bytes, after: bytes) -> list[Sample]:
    samples = []
    for index, item in enumerate(raw_samples, start=1):
        if not isinstance(item, dict):
            raise ValueError(f"sample {index} must be an object")
        png = nonempty(item.get("png"), f"sample {index} png")
        start = integer(item.get("start_ns"), f"sample {index} start_ns")
        end = integer(item.get("end_ns"), f"sample {index} end_ns")
        if end < start:
            raise ValueError(f"sample {index} end_ns precedes start_ns")
        _, pixels = png_crop(directory / png, roi, size)
        state = "OTHER"
        if pixels == before:
            state = "BEFORE"
        elif pixels == after:
            state = "AFTER"
        samples.append(Sample(png, start, end, state))
    return samples


def summarize(samples: list[Sample], tap_start: int, other_tap: bool) -> dict:
    tap_end = tap_start + 999
    result = {
        "text_change_verified": False,
        "tap_start_ns": tap_start,
        "tap_end_ns": tap_end,
        "max_capture_window_ns": max((sample.end_ns - sample.start_ns for sample in samples), default=None),
        "sample_counts": {state: sum(sample.state == state for sample in samples)
                          for state in ("BEFORE", "AFTER", "OTHER")},
        "pre_tap_before": None,
        "last_before": None,
        "first_after": None,
        "latency_lower_ns": None,
        "latency_upper_ns": None,
    }

    def finish(status: str, reason: str | None = None) -> dict:
        return {"status": status, "reason": reason, **result}

    if other_tap:
        return finish("WINDOW_UNCERTAIN", "Another primary_down occurs in the sampled window.")
    if any(current.start_ns < previous.end_ns for previous, current in zip(samples, samples[1:])):
        return finish("WINDOW_UNCERTAIN", "Capture intervals overlap or are out of order.")

    first_after = next((index for index, sample in enumerate(samples) if sample.state == "AFTER"), None)
    if first_after is None:
        return finish("INSUFFICIENT_SAMPLES", "No sample matches the reviewed new text.")
    after = samples[first_after]
    result["first_after"] = asdict(after)
    if after.start_ns <= tap_end:
        return finish("WINDOW_UNCERTAIN", "New text is visible before the input or its capture overlaps the input.")
    if any(sample.state == "BEFORE" for sample in samples[first_after + 1:]):
        return finish("WINDOW_UNCERTAIN", "Old text reappears after new text in the sampled window.")
    old_samples = [sample for sample in samples[:first_after] if sample.state == "BEFORE"]
    baseline = next((sample for sample in reversed(old_samples) if sample.end_ns < tap_start), None)
    if baseline is None:
        if any(sample.start_ns <= tap_end for sample in old_samples):
            return finish("WINDOW_UNCERTAIN", "The old-text baseline capture overlaps the input.")
        return finish("INSUFFICIENT_SAMPLES", "No reviewed old-text baseline was captured before the input.")

    before = old_samples[-1]
    lower = max(0, before.start_ns - tap_end)
    upper = after.end_ns - tap_start
    result.update(text_change_verified=True, pre_tap_before=asdict(baseline), last_before=asdict(before),
                  latency_lower_ns=lower, latency_upper_ns=upper)
    if upper <= LIMIT_NS:
        return finish("PASS")
    if lower > LIMIT_NS:
        return finish("FAIL")
    return finish("WINDOW_UNCERTAIN", "The verified transition's latency bounds straddle 150 ms.")


def logged_taps(tap_log: Path) -> list[tuple[int, int]]:
    return [(int(match[1]), int(match[2])) for line in tap_log.read_text(encoding="utf-8-sig").splitlines()
            if (match := PRIMARY_DOWN.search(line))]


def run(manifest: Path) -> dict:
    data = json.loads(manifest.read_text(encoding="utf-8-sig"))
    return analyze(data, manifest.parent)


def analyze(data: dict, directory: Path) -> dict:
    if not isinstance(data, dict) or data.get("clock") != "CLOCK_MONOTONIC":
        raise ValueError("Manifest must explicitly declare device CLOCK_MONOTONIC.")
    timing = nonempty(data.get("capture_timing"), "capture_timing")
    session = integer(data.get("session"), "session")
    input_us = integer(data.get("input_mono_us"), "input_mono_us")
    tap_log = directory / nonempty(data.get("tap_log"), "tap_log")
    taps = [tap for tap_session, tap in logged_taps(tap_log) if tap_session == session]
    if taps.count(input_us) != 1:
        raise ValueError("Expected exactly one matching primary_down in the selected CS2Perf session.")

    roi_values = data.get("roi")
    if not isinstance(roi_values, list) or len(roi_values) != 4:
        raise ValueError("roi must be [left, top, right, bottom] in original PNG pixels")
    roi = tuple(integer(value, "roi coordinate") for value in roi_values)
    reference = data.get("reference")
    if not isinstance(reference, dict) or reference.get("reviewed") is not True:
        raise ValueError("reference.reviewed must attest to manual review of both text crops")
    before_text = nonempty(reference.get("before_text"), "reference.before_text")
    after_text = nonempty(reference.get("after_text"), "reference.after_text")
    if before_text == after_text:
        raise ValueError("Reviewed reference texts must differ")
    before_png = nonempty(reference.get("before_png"), "reference.before_png")
    after_png = nonempty(reference.get("after_png"), "reference.after_png")
    size, before = png_crop(directory / before_png, roi)
    _, after = png_crop(directory / after_png, roi, size)
    if before == after:
        raise ValueError("Reference pixels inside the ROI are identical")

    raw_samples = data.get("samples")
    if not isinstance(raw_samples, list):
        raise ValueError("samples must be an array of periodic PNG captures")
    samples = read_samples(raw_samples, directory, roi, size, before, after)
    other_tap = bool(samples) and any(
        tap * 1000 + 999 >= samples[0].start_ns and tap * 1000 <= samples[-1].end_ns
        for tap in taps if tap != input_us
    )
    return {**summarize(samples, input_us * 1000, other_tap), "clock": data["clock"],
            "tap_log": data["tap_log"], "observation": "text ROI in captured images",
            "session": session, "input_mono_us": input_us, "capture_timing": timing,
            "limit_ns": LIMIT_NS, "roi": roi, "reference": reference}


def prepare(args: argparse.Namespace) -> Path:
    directory = args.directory
    samples = []
    with (directory / "capture.csv").open(encoding="ascii", newline="") as timings:
        rows = csv.DictReader(timings)
        if rows.fieldnames != CAPTURE_FIELDS:
            raise ValueError("Expected capture.csv from android_flip_capture")
        for sequence, row in enumerate(rows, start=1):
            filename = f"frame_{sequence:03d}.png"
            if None in row or any(value is None for value in row.values()):
                raise ValueError(f"Malformed capture row {sequence}")
            if row["seq"] != str(sequence) or row["png"] != filename:
                raise ValueError(f"Unexpected capture sequence or filename in row {sequence}")
            start = integer(int(row["start_ns"]), "capture start_ns")
            end = integer(int(row["end_ns"]), "capture end_ns")
            if end <= start or (samples and start < samples[-1]["end_ns"]):
                raise ValueError(f"Invalid capture bounds in row {sequence}")
            samples.append({"png": filename, "start_ns": start, "end_ns": end})
    if not 2 <= len(samples) <= 60:
        raise ValueError("Expected 2 to 60 completed screenshots")
    lower = samples[0]["start_ns"]
    upper = samples[-1]["end_ns"]
    tap_log = "cs2perf.log"
    taps = [(session, tap) for session, tap in logged_taps(directory / tap_log)
            if tap * 1000 + 999 >= lower and tap * 1000 <= upper]
    if len(taps) != 1:
        raise ValueError(f"Expected one isolated native primary_down in the capture window; found {len(taps)}")
    session, input_us = taps[0]
    before_text = nonempty(args.before_text, "before-text")
    after_text = nonempty(args.after_text, "after-text")
    if before_text == after_text:
        raise ValueError("Reviewed reference texts must differ")
    manifest = {
        "clock": "CLOCK_MONOTONIC",
        "capture_timing": "android_flip_capture: device CLOCK_MONOTONIC before fork and after screencap -p exit",
        "capture_csv": "capture.csv",
        "tap_log": tap_log,
        "session": session,
        "input_mono_us": input_us,
        "roi": args.roi,
        "reference": {"before_png": args.before_png, "after_png": args.after_png,
                      "before_text": before_text, "after_text": after_text, "reviewed": args.reviewed},
        "samples": samples,
    }
    analyze(manifest, directory)
    destination = directory / "flip.json"
    with destination.open("x", encoding="utf-8") as output:
        output.write(json.dumps(manifest, ensure_ascii=False, indent=2) + "\n")
    return destination


def main(argv: list[str] | None = None) -> int:
    arguments = sys.argv[1:] if argv is None else argv
    if arguments[:1] == ["prepare"]:
        parser = argparse.ArgumentParser(prog="android_flip_metrics.py prepare",
                                         description="Build a manifest from capture.csv and cs2perf.log.")
        parser.add_argument("directory", type=Path)
        parser.add_argument("--before-png", required=True)
        parser.add_argument("--after-png", required=True)
        parser.add_argument("--before-text", required=True)
        parser.add_argument("--after-text", required=True)
        parser.add_argument("--roi", type=int, nargs=4, required=True, metavar=("LEFT", "TOP", "RIGHT", "BOTTOM"))
        parser.add_argument("--reviewed", action="store_true", required=True)
        args = parser.parse_args(arguments[1:])
        try:
            print(prepare(args))
        except (OSError, ValueError, csv.Error, Image.DecompressionBombError) as error:
            parser.exit(2, f"{parser.prog}: error: {error}\n")
        return 0

    parser = argparse.ArgumentParser(description=__doc__, epilog=MANIFEST_HELP,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("manifest", type=Path, help="JSON evidence manifest (see 'prepare --help' to generate one)")
    args = parser.parse_args(arguments)
    try:
        report = run(args.manifest)
    except (OSError, ValueError, Image.DecompressionBombError) as error:
        parser.exit(2, f"{parser.prog}: error: {error}\n")
    print(json.dumps(report, indent=2))
    return EXIT_CODES[report["status"]]


if __name__ == "__main__":
    raise SystemExit(main())

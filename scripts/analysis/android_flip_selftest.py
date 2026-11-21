"""Offline self-check for the flip capture CSV, native log and PNG workflow."""

import csv
import json
from pathlib import Path
import subprocess
import sys
from tempfile import TemporaryDirectory
import unittest

from PIL import Image, ImageDraw


METRICS = Path(__file__).with_name("android_flip_metrics.py")
TAP_NS = 1_000_000_000
TAP_LOG = "I/CS2Perf: primary_down session=7 input_mono_us=1000000\n"


class FlipSelfTest(unittest.TestCase):
    def setUp(self):
        temporary = TemporaryDirectory(prefix="android-flip-selftest-")
        self.addCleanup(temporary.cleanup)
        self.directory = Path(temporary.name)
        self.log = self.directory / "cs2perf.log"
        self.log.write_text(TAP_LOG, encoding="utf-8-sig")
        self.manifest = self.directory / "flip.json"
        self.capture = self.directory / "capture.csv"

    def write_capture(self, *frames):
        with self.capture.open("w", encoding="ascii", newline="") as stream:
            writer = csv.writer(stream)
            writer.writerow(["seq", "png", "start_ns", "end_ns"])
            for index, (text, start_ms, end_ms) in enumerate(frames, 1):
                filename = f"frame_{index:03d}.png"
                writer.writerow([index, filename, TAP_NS + start_ms * 1_000_000, TAP_NS + end_ms * 1_000_000])
                image = Image.new("RGB", (152, 48))
                draw = ImageDraw.Draw(image)
                draw.text((10, 10), text, fill="white")
                draw.rectangle((130, 12, 145, 24), fill="green" if index % 2 else "blue")
                image.save(self.directory / filename)

    def cli(self, *args):
        return subprocess.run([sys.executable, "-B", str(METRICS), *map(str, args)],
                              capture_output=True, encoding="utf-8", errors="replace")

    def prepare(self, after="frame_004.png"):
        return self.cli("prepare", self.directory, "--before-png", "frame_001.png", "--after-png", after,
                        "--before-text", "old text", "--after-text", "new text",
                        "--roi", 4, 4, 118, 40, "--reviewed")

    def verify(self, code):
        prepared = self.prepare()
        self.assertEqual(prepared.returncode, 0, prepared.stderr)
        result = self.cli(self.manifest)
        self.assertEqual(result.returncode, code, result.stderr or result.stdout)
        return json.loads(result.stdout)

    def test_selected_tap_and_first_matching_frame(self):
        self.write_capture(("old text", -40, -20), ("old text", 20, 30), ("new", 40, 55),
                           ("new text", 100, 150), ("new text", 160, 175))
        self.log.write_text(TAP_LOG + "primary_down session=7 input_mono_us=900000\n", encoding="utf-8-sig")
        result = self.verify(0)
        self.assertEqual(result["status"], "PASS")
        self.assertEqual((result["tap_start_ns"], result["tap_end_ns"]), (TAP_NS, TAP_NS + 999))
        self.assertEqual(result["sample_counts"], {"BEFORE": 2, "AFTER": 2, "OTHER": 1})
        self.assertEqual(result["pre_tap_before"]["png"], "frame_001.png")
        self.assertEqual(result["first_after"]["png"], "frame_004.png")
        self.assertEqual(result["latency_upper_ns"], 150_000_000)

    def test_long_capture_window_is_uncertain(self):
        self.write_capture(("old text", -40, -20), ("old text", 20, 30), ("new", 40, 55),
                           ("new text", 100, 220))
        result = self.verify(4)
        self.assertEqual(result["status"], "WINDOW_UNCERTAIN")
        self.assertTrue(result["text_change_verified"])
        self.assertEqual(result["max_capture_window_ns"], 120_000_000)

    def test_late_old_text_proves_failure(self):
        self.write_capture(("old text", -40, -20), ("old text", 155, 170), ("new", 180, 190),
                           ("new text", 200, 220))
        self.assertEqual(self.verify(1)["latency_lower_ns"], 154_999_001)

    def test_partial_text_does_not_prove_a_flip(self):
        self.write_capture(("old text", -40, -20), ("old text", 20, 30), ("new", 40, 55))
        reference = Image.new("RGB", (152, 48))
        ImageDraw.Draw(reference).text((10, 10), "new text", fill="white")
        reference.save(self.directory / "reviewed-new.png")
        prepared = self.prepare("reviewed-new.png")
        self.assertEqual(prepared.returncode, 0, prepared.stderr)
        result = self.cli(self.manifest)
        self.assertEqual(result.returncode, 3, result.stderr or result.stdout)
        self.assertIsNone(json.loads(result.stdout)["first_after"])

    def test_skip_incomplete_capture(self):
        self.write_capture(("old text", -40, -20), ("new text", 20, 30))
        self.capture.rename(self.directory / "capture.csv.partial")
        result = self.prepare("frame_002.png")
        self.assertEqual(result.returncode, 2)
        self.assertFalse(self.manifest.exists())

    def test_reject_overlap_or_multiple_taps(self):
        self.write_capture(("old text", -40, -20), ("new text", -25, 30))
        result = self.prepare("frame_002.png")
        self.assertEqual(result.returncode, 2)
        self.assertIn("Invalid capture bounds", result.stderr)
        self.write_capture(("old text", -40, -20), ("new text", 30, 40))
        self.log.write_text(TAP_LOG + "primary_down session=7 input_mono_us=1005000\n", encoding="utf-8")
        result = self.prepare("frame_002.png")
        self.assertEqual(result.returncode, 2)
        self.assertIn("found 2", result.stderr)
        self.assertFalse(self.manifest.exists())

    def test_reject_corrupt_png_before_creating_manifest(self):
        self.write_capture(("old text", -40, -20), ("new text", 20, 30))
        (self.directory / "frame_002.png").write_bytes(b"truncated PNG")
        result = self.prepare("frame_002.png")
        self.assertEqual(result.returncode, 2)
        self.assertFalse(self.manifest.exists())


if __name__ == "__main__":
    unittest.main()

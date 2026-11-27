"""Change FES settings, quit, and verify the restored controls in a new process."""

import argparse
from datetime import datetime
import os
from pathlib import Path
from xml.etree import ElementTree

from PIL import Image, ImageChops

from run_button_batch import NORMAL_EXIT
from run_djibril_save_roundtrip import log
from run_pc_ordinary_save_roundtrip import await_resources, observed_receipts
from run_playthrough_batch import (
    INVENTORY, identify_player, load_inventory, new_output_directory, playthrough_cache_root, run_case, write_json,
)


INDEX = 9
FILENAME = "setup.xml"
BASE = [
    "story-events", "capture-every:0", "no-final-capture", "button-probe",
    "move:2325:195,459", "require-hit:2340:16", "2340:195,459",
    "move:2520:750,100", "inspect:2640",
]
EDITS = [
    "move:2650:726,188", "require-hit:2660:43", "2660:726,188",
    "move:2770:716,237", "require-hit:2780:45", "2780:716,237",
    "move:2890:648,452", "require-hit:2900:24", "2900:648,452",
    "move:2920:750,100", "inspect:2980",
]
QUIT = [
    "move:3000:1164,683", "require-hit:3010:42", "3010:1164,683",
    "move:4000:195,561", "require-hit:4020:18", "4020:195,561",
    "move:4710:552,366", "require-hit:4740:26", "4740:552,366",
]
CONTROLS = {"font": (620, 175, 834, 203), "speed": (620, 431, 840, 476)}


def screenshot(result, frame):
    with Image.open(Path(result["output"]) / f"{INDEX:02}.{frame}.png") as image:
        return image.convert("RGB")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--player", type=Path, required=True)
    parser.add_argument("--cache-root", type=Path, help="absolute directory for the playthrough cache")
    args = parser.parse_args()
    player = args.player.resolve(strict=True)
    build = identify_player(player, require_integration=True)
    env = os.environ.copy()
    try:
        cache = playthrough_cache_root(args.cache_root)
    except ValueError as error:
        parser.error(str(error))
    output = new_output_directory(cache, datetime.now().astimezone())
    print(output, flush=True)
    env["CS2_MEDIA_CACHE"] = str(output / "movie-input")
    if os.name == "nt":
        env["PATH"] = os.pathsep.join(("C:/msys64/mingw64/bin", env.get("PATH", "")))
    game = load_inventory(INVENTORY)[0][INDEX]
    receipt = {"status": "failed", "build": build}
    try:
        await_resources(output)
        writer_dir = output / "write"
        writer_dir.mkdir()
        writer = run_case(INDEX, game, 4950, BASE + EDITS + QUIT, player, writer_dir, 240, env)
        receipt["writer"] = writer
        exit_frame = next((int(match[1]) for line in log(writer, "stderr")
                           if (match := NORMAL_EXIT.fullmatch(line))), None)
        if (writer["status"] != "unexpected_exit" or exit_frame is None or exit_frame <= 4740
                or not observed_receipts(writer, {(2661, 43, 8, 0), (2781, 45, 8, 0),
                                                  (2901, 24, 8, 0), (4741, 26, 24, 0)})):
            raise ValueError("settings edits or the normal game exit were not observed")
        setup = Path(writer["profile"]) / FILENAME
        events = Path(writer["stdout"]).read_text(encoding="utf-8")
        if "configuration-written setup.xml:" not in events:
            raise ValueError("normal exit did not report a setup.xml write")
        writer.update(status="normal_exit", normal_exit_frame=exit_frame)
        writer_report = writer_dir / f"{INDEX:02}.json"
        verified_report = writer_report.with_suffix(".verified.json")
        write_json(verified_report, writer)
        verified_report.replace(writer_report)
        config = ElementTree.parse(setup).getroot()
        receipt["settings"] = {
            "font": config.findtext("./FONT/face"), "speed": config.findtext("./MESSAGE/speed"),
        }
        if any(not value for value in receipt["settings"].values()):
            raise ValueError("setup.xml lacks the edited settings")

        await_resources(output)
        reader_dir = output / "reopen"
        reader_dir.mkdir()
        reader = run_case(INDEX, game, 2750, BASE, player, reader_dir, 240, env,
                          checkpoint_files={FILENAME: setup},
                          window_checks={2640: ["flow", "config"]})
        receipt["reader"] = reader
        if reader["status"] != "visual_review_required" or reader["pid"] == writer["pid"]:
            raise ValueError("a new process did not reach the FES settings window")
        before, edited, restored = screenshot(writer, 2640), screenshot(writer, 2980), screenshot(reader, 2640)
        for label, bounds in CONTROLS.items():
            changed = edited.crop(bounds)
            if not ImageChops.difference(before.crop(bounds), changed).getbbox():
                raise ValueError(f"{label} did not visibly change")
            if ImageChops.difference(restored.crop(bounds), changed).getbbox():
                raise ValueError(f"{label} did not match the saved view on cold startup")
        receipt["status"] = "passed"
    except (OSError, ValueError, ElementTree.ParseError) as error:
        receipt["reason"] = str(error)
    write_json(output / "settings_roundtrip.json", receipt)
    print(receipt["status"], receipt.get("reason", ""), flush=True)
    return int(receipt["status"] != "passed")


if __name__ == "__main__":
    raise SystemExit(main())

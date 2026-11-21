"""Write and cold-read an ordinary Djibril slot with the Windows replay player."""

import argparse
from datetime import datetime
import json
import os
from pathlib import Path
import re
import shutil

from check_neko_save_roundtrip import slot_identity
from run_button_batch import MOUSE_PRESS
from run_playthrough_batch import (
    INVENTORY, identify_player, load_inventory, new_output_directory, run_case, write_json,
)


INDEX = 10
SLOT = "save0000.dat"
SAVE_SCENARIO = Path(__file__).with_name("pc_button_save_and_tutorial.json")
REOPEN = [
    "story-events", "capture-every:0", "button-probe", "inspect:690",
    "move:708:190,281", "require-hit:710:40", "710:190,281", "inspect:1010",
    "move:1038:279,135", "require-hit:1040:33", "1040:279,135", "inspect:1350",
    "move:1358:427,272", "require-hit:1360:61", "1360:427,272", "inspect:1680",
    "1700:13", "inspect:1920",
]
ENTER = re.compile(
    r"pressed: 1, repeated: false, region: \d+, receiver: 8, response: 0, "
    r"physical: \[RoutedPhysicalButton \{ source: Keyboard, code: 13 \}\]"
)
WRITE = re.compile(r"frame=\d+ fes-datasave slot=0 container=\d+ written=true")


def log(result, stream):
    return Path(result[stream]).read_text(encoding="utf-8", errors="replace").splitlines()


def consumed(lines, frame, region):
    return any(hit[1] == str(region) and hit[3] == "0"
               for line in lines if line.startswith(f"frame {frame}: pointer ")
               for hit in MOUSE_PRESS.finditer(line))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--player", required=True, type=Path)
    parser.add_argument("--timeout", type=int, default=240)
    args = parser.parse_args()
    if args.timeout < 1:
        parser.error("timeout must be positive")

    player = args.player.resolve(strict=True)
    build = identify_player(player, require_integration=True)
    env = os.environ.copy()
    cache = Path(env.get("CS2_BUILD_CACHE_ROOT", "F:/CS2-build-cache"))
    output = new_output_directory(cache / "playthrough", datetime.now().astimezone())
    print(output, flush=True)
    env["CS2_MEDIA_CACHE"] = str(output / "movie-input")
    if os.name == "nt":
        env["PATH"] = os.pathsep.join(("C:/msys64/mingw64/bin", env.get("PATH", "")))
    game = load_inventory(INVENTORY)[0][INDEX]
    scenario = json.loads(SAVE_SCENARIO.read_text(encoding="utf-8"))[str(INDEX)]
    save_dir, reopen_dir = output / "save", output / "reopen"
    save_dir.mkdir()
    record = {"status": "failed", "build": build}
    try:
        saved = run_case(INDEX, game, scenario["frames"], scenario["inputs"],
                         player, save_dir, args.timeout, env)
        record["producer"] = saved
        if (saved["status"] != "visual_review_required" or
                sum(bool(WRITE.fullmatch(line)) for line in log(saved, "stdout")) != 1):
            raise ValueError("missing successful in-game write to ordinary slot 0")

        slot = Path(saved["profile"]) / SLOT
        identity = slot_identity(slot)
        record["slot"] = identity
        profile = reopen_dir / f"{INDEX:02}.profile"
        profile.mkdir(parents=True)
        staged = profile / SLOT
        shutil.copyfile(slot, staged)
        if slot_identity(staged) != identity:
            raise ValueError("staged slot differs from the producer slot")

        reopened = run_case(INDEX, game, 1940, REOPEN, player, reopen_dir, args.timeout, env)
        record["consumer"] = reopened
        if (reopened["status"] != "visual_review_required" or
                reopened["pid"] is None or reopened["pid"] == saved["pid"]):
            raise ValueError("cold reader did not complete in a separate process")
        host = log(reopened, "stderr")
        if not all(consumed(host, frame, region) for frame, region in ((711, 40), (1041, 33), (1361, 61))):
            raise ValueError("missing consumed title, ordinary slot or confirmation click")
        if (not any(ENTER.search(line) for line in host if line.startswith("frame 1701: pointer ")) or
                "frame=1701 message-resume message=[2, 0, 0, 0]" not in log(reopened, "stdout")):
            raise ValueError("missing Enter receipt and resumed story script")
        before, after = "10.1680.png", "10.1920.png"
        if not {before, after}.issubset(reopened["captures"]) or slot_identity(staged) != identity:
            raise ValueError("missing story captures or ordinary slot modified by load")
        record.update(status="visual_review_required", restored_page=before, next_page=after)
    except (OSError, ValueError) as error:
        record["reason"] = str(error)
    write_json(output / "roundtrip.json", record)
    print(record["status"], record.get("reason", ""), flush=True)
    return int(record["status"] == "failed")


if __name__ == "__main__":
    raise SystemExit(main())

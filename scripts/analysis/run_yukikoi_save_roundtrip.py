"""Write and cold-load one Yukikoi Melt ordinary slot through the game menus."""

import argparse
from datetime import datetime
import json
import os
from pathlib import Path
import shutil

from check_neko_save_roundtrip import slot_identity
from run_button_batch import MOUSE_PRESS
from run_djibril_save_roundtrip import consumed, log, WRITE
from run_playthrough_batch import (
    INVENTORY, identify_player, load_inventory, new_output_directory, run_case, write_json,
)


INDEX = 4
SLOT = "save0000.dat"
PRODUCER = Path(__file__).with_name("pc_story_04_ordinary_save.json")
REOPEN = [
    "story-events", "capture-every:0", "button-probe", "inspect:1170",
    "move:1172:638,446", "require-hit:1174:19", "1174:638,446",
    "inspect:1350",
    "move:1358:165,442", "require-hit:1360:16", "1360:165,442", "inspect:1500",
    "move:1503:354,171", "require-hit:1505:57", "1505:354,171", "inspect:1660",
    "move:1663:587,355", "require-hit:1665:86", "1665:587,355", "inspect:2050",
    "move:2068:640,610", "2070:640,610", "inspect:2220",
]
LOAD_RECEIPTS = {(1361, 16, 8, 1), (1506, 57, 30, 0), (1666, 86, 78, 1)}


def pointer_receipts(host, frames):
    return {
        (frame, *map(int, press.groups()))
        for frame in frames
        for line in host if line.startswith(f"frame {frame}: pointer ")
        for press in MOUSE_PRESS.finditer(line)
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--player", required=True, type=Path)
    parser.add_argument("--timeout", type=int, default=300)
    args = parser.parse_args()
    if args.timeout < 1:
        parser.error("timeout must be positive")

    player = args.player.resolve(strict=True)
    build = identify_player(player, require_integration=True)
    env = os.environ.copy()
    cache = Path(env.get("CS2_BUILD_CACHE_ROOT", "F:/CS2-build-cache"))
    out = new_output_directory(cache / "playthrough", datetime.now().astimezone())
    print(out, flush=True)
    env["CS2_MEDIA_CACHE"] = str(out / "movie-input")
    if os.name == "nt":
        env["PATH"] = os.pathsep.join(("C:/msys64/mingw64/bin", env.get("PATH", "")))
    game = load_inventory(INVENTORY)[0][INDEX]
    scenario = json.loads(PRODUCER.read_text(encoding="utf-8"))[str(INDEX)]
    save_dir, reopen_dir = out / "save", out / "reopen"
    save_dir.mkdir()
    result = {"status": "failed", "build": build}
    try:
        saved = run_case(INDEX, game, scenario["frames"], scenario["inputs"], player, save_dir, args.timeout, env)
        result["producer"] = saved
        if (saved["status"] != "visual_review_required"
                or sum(bool(WRITE.fullmatch(line)) for line in log(saved, "stdout")) != 1
                or not consumed(log(saved, "stderr"), 2186, 128)):
            detail = (saved["first_engine_error"] or {}).get("detail")
            raise ValueError(detail or "ordinary slot 0 write was not observed")

        slot = Path(saved["profile"]) / SLOT
        identity = slot_identity(slot)
        result["slot"] = identity
        profile = reopen_dir / f"{INDEX:02}.profile"
        profile.mkdir(parents=True)
        staged = profile / SLOT
        shutil.copyfile(slot, staged)
        if slot_identity(staged) != identity:
            raise ValueError("staged slot differs from the producer slot")

        reopened = run_case(INDEX, game, 2300, REOPEN, player, reopen_dir, args.timeout, env)
        result["consumer"] = reopened
        if (reopened["status"] != "visual_review_required" or reopened["pid"] is None
                or reopened["pid"] == saved["pid"]):
            detail = (reopened["first_engine_error"] or {}).get("detail")
            raise ValueError(detail or "cold read did not finish in a new process")

        host = log(reopened, "stderr")
        receipts = pointer_receipts(host, (1361, 1506, 1666, 2071))
        if not LOAD_RECEIPTS.issubset(receipts):
            raise ValueError("missing title, ordinary slot or confirmation receipt")
        events = log(reopened, "stdout")
        if ((2071, 147, 122, 1) not in receipts or
                "frame=2071 message-resume message=[2, 640, 610, 0]" not in events or
                not any(line.startswith("frame=2071 native-call index=1040 ")
                        and 'debug="[message] ' in line for line in events)):
            raise ValueError("missing story click receipt or next dialogue request")
        captures = ["04.2050.png", "04.2220.png"]
        if not set(captures).issubset(reopened["captures"]) or slot_identity(staged) != identity:
            raise ValueError("missing story captures or ordinary slot modified by load")
        result.update(status="visual_review_required", restored_page=captures[0], next_page=captures[1],
                      story_captures=captures, click_receipts=sorted(receipts))
    except (OSError, ValueError) as error:
        result["reason"] = str(error)
    write_json(out / "roundtrip.json", result)
    print(result["status"], result.get("reason", ""), flush=True)
    return int(result["status"] == "failed")


if __name__ == "__main__":
    raise SystemExit(main())

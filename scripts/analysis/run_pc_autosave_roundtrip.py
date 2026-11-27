"""Write Campanella's battle autosave and cold-load those bytes in a new process."""

import argparse
from datetime import datetime
import json
import os
from pathlib import Path
import re

from run_pc_ordinary_save_roundtrip import await_resources
from run_playthrough_batch import (
    INVENTORY, complete_css_file, identify_player, load_checkpoint_files, load_inventory,
    new_output_directory, playthrough_cache_root, run_case, write_json,
)


INDEX = 18
SLOT = "save1000.dat"
SYSTEM = "savegen.dat"
READ_EVENT = re.compile(r"frame=(\d+) save-slot-read .* slot=(\d+) handle=([1-9]\d*)$")
SCENARIO_DIR = Path(__file__).resolve().parent
GOOD_STATUS = {"frame_limit", "visual_review_required"}


def scenario(action):
    path = SCENARIO_DIR / f"pc_system_features_autosave_{action}_regression.json"
    return json.loads(path.read_text(encoding="utf-8"))[str(INDEX)]


def slot_read_frame(result, slot, start, end):
    with Path(result["stdout"]).open(encoding="utf-8", errors="replace") as events:
        for line in events:
            if (match := READ_EVENT.fullmatch(line.rstrip("\r\n"))):
                frame = int(match[1])
                if int(match[2]) == slot and start <= frame <= end:
                    return frame
    raise ValueError(f"no successful read of slot {slot} from frame {start} through {end}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--player", type=Path, required=True)
    parser.add_argument("--cache-root", type=Path, help="absolute directory for the playthrough cache")
    args = parser.parse_args()
    try:
        player = args.player.resolve(strict=True)
        build = identify_player(player, require_integration=True)
        cache = playthrough_cache_root(args.cache_root)
        game = load_inventory(INVENTORY)[0][INDEX]
        write, read = scenario("write"), scenario("read")
        seeds = load_checkpoint_files(write["checkpoint_files"])
        output = new_output_directory(cache, datetime.now().astimezone())
    except (OSError, ValueError) as error:
        parser.error(str(error))

    print(output, flush=True)
    env = os.environ.copy()
    env.setdefault("CS2_REPLAY_PERF", "1")
    env.setdefault("CS2_MEDIA_CACHE", str(cache / "movie-input"))
    if os.name == "nt":
        env["PATH"] = os.pathsep.join(("C:/msys64/mingw64/bin", env.get("PATH", "")))

    receipt = {"status": "failed", "build": build}
    try:
        write_dir = output / "write"
        write_dir.mkdir()
        await_resources(output)
        writer = run_case(INDEX, game, write["frames"], write["inputs"], player, write_dir, 360, env,
                          checkpoint_files=seeds, event_checks=write["event_checks"],
                          save_checks=write["save_checks"])
        receipt["writer"] = writer
        if writer["status"] not in GOOD_STATUS:
            raise ValueError(f"battle autosave write returned {writer['status']}")
        receipt["source_slot_frame"] = slot_read_frame(writer, 1004, 3310, 4450)
        profile = Path(writer["profile"])
        system_save = profile / SYSTEM
        if not complete_css_file(system_save, system_save.stat().st_size):
            raise ValueError("writer's system save is incomplete")

        read_dir = output / "reopen"
        read_dir.mkdir()
        await_resources(output)
        reader = run_case(INDEX, game, read["frames"], read["inputs"], player, read_dir, 240, env,
                          checkpoint_files={SLOT: profile / SLOT, SYSTEM: system_save},
                          window_checks={int(frame): names for frame, names in read["window_checks"].items()},
                          event_checks=read["event_checks"], save_checks=read["save_checks"])
        receipt["reader"] = reader
        if reader["status"] not in GOOD_STATUS or reader["pid"] == writer["pid"]:
            raise ValueError(f"cold autosave read did not finish in a new process ({reader['status']})")
        receipt["autosave_read_frame"] = slot_read_frame(reader, 1000, 3240, 3860)
        receipt["status"] = "passed"
    except (OSError, ValueError) as error:
        receipt["reason"] = str(error)

    write_json(output / "autosave_roundtrip.json", receipt)
    print(receipt["status"], receipt.get("reason", ""), flush=True)
    return int(receipt["status"] != "passed")


if __name__ == "__main__":
    raise SystemExit(main())

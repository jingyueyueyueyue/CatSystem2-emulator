"""Write and cold-load an ordinary slot through the Windows replay player."""

import argparse
from datetime import datetime
import json
import os
from pathlib import Path
import shutil
import time

from check_neko_save_roundtrip import slot_identity
from run_djibril_save_roundtrip import WRITE, log
from run_playthrough_batch import (
    GIB, INVENTORY, available_ram_bytes, identify_player, load_inventory,
    new_output_directory, run_case, write_json,
)
from run_yukikoi_save_roundtrip import pointer_receipts


SLOT = "save0000.dat"
CASES = {
    3: {
        "save_receipts": {(2771, 18, 12, 1), (2921, 128, 32, 0)},
        "load_receipts": {(2453, 16, 8, 1), (2571, 65, 31, 0), (2746, 94, 86, 1), (3011, 73, 8, 0)},
        "resume": "frame=3011 message-resume message=[2, 640, 540, 0]",
        "captures": ("03.2990.png", "03.3170.png"),
        "frames": 3190,
        "inputs": [
            "230:557,324", "465:557,324", "525:557,324", "549:557,324", "573:557,324",
            "inspect:2448", "move:2450:176,414", "require-hit:2452:16", "2452:176,414", "inspect:2557",
            "move:2568:471,146", "require-hit:2570:65", "2570:471,146", "inspect:2740",
            "move:2743:546,348", "require-hit:2745:94", "2745:546,348", "inspect:2990",
            "move:3008:640,540", "require-hit:3010:73", "3010:640,540", "inspect:3170",
        ],
    },
    6: {
        "save_receipts": {(2121, 23, 12, 0), (2311, 55, 36, 0), (2486, 106, 101, 0)},
        "load_receipts": {(499, 19, 11, 0), (611, 32, 13, 0), (771, 50, 45, 0), (1441, 64, 8, 0)},
        "resume": "frame=1441 message-resume message=[2, 1135, 687, 0]",
        "captures": ("06.1400.png", "06.1600.png"),
        "frames": 1640,
        "inputs": [
            "inspect:495", "move:497:84,308", "require-hit:498:19", "498:84,308", "inspect:603",
            "move:608:682,208", "require-hit:610:32", "610:682,208", "inspect:760",
            "move:768:543,346", "require-hit:770:50", "770:543,346", "inspect:1400",
            "move:1438:1135,687", "require-hit:1440:64", "1440:1135,687", "inspect:1600",
        ],
    },
}


def await_resources(output):
    deadline = time.monotonic() + 120
    while available_ram_bytes() < 4 * GIB or shutil.disk_usage(output).free < 22 * GIB:
        if time.monotonic() >= deadline:
            raise ValueError("insufficient RAM or disk for one replay")
        time.sleep(5)


def observed_receipts(result, expected):
    return expected.issubset(pointer_receipts(log(result, "stderr"), {press[0] for press in expected}))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--player", required=True, type=Path)
    parser.add_argument("--index", required=True, choices=CASES, type=int)
    parser.add_argument("--timeout", type=int, default=360)
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

    index = args.index
    case = CASES[index]
    game = load_inventory(INVENTORY)[0][index]
    producer = Path(__file__).with_name(f"pc_story_{index:02}_ordinary_save.json")
    scenario = json.loads(producer.read_text(encoding="utf-8"))[str(index)]
    save_dir, reopen_dir = output / "save", output / "reopen"
    save_dir.mkdir()
    result = {"status": "failed", "build": build}
    try:
        await_resources(output)
        saved = run_case(index, game, scenario["frames"], scenario["inputs"],
                         player, save_dir, args.timeout, env)
        result["producer"] = saved
        if (saved["status"] != "visual_review_required"
                or sum(bool(WRITE.fullmatch(line)) for line in log(saved, "stdout")) != 1
                or not observed_receipts(saved, case["save_receipts"])):
            detail = (saved["first_engine_error"] or {}).get("detail")
            raise ValueError(detail or "ordinary slot write or menu click was not verified")

        slot = Path(saved["profile"]) / SLOT
        identity = slot_identity(slot)
        result["slot"] = identity
        profile = reopen_dir / f"{index:02}.profile"
        profile.mkdir(parents=True)
        staged = profile / SLOT
        shutil.copyfile(slot, staged)
        if slot_identity(staged) != identity:
            raise ValueError("staged slot differs from the producer slot")

        await_resources(output)
        inputs = ["story-events", "capture-every:0", "button-probe", *case["inputs"]]
        reopened = run_case(index, game, case["frames"], inputs, player, reopen_dir, args.timeout, env)
        result["consumer"] = reopened
        if (reopened["status"] != "visual_review_required" or reopened["pid"] is None
                or reopened["pid"] == saved["pid"]):
            detail = (reopened["first_engine_error"] or {}).get("detail")
            raise ValueError(detail or "cold read did not finish in a new process")

        if not observed_receipts(reopened, case["load_receipts"]):
            raise ValueError("missing title, slot, confirmation, or story click receipt")
        if case["resume"] not in log(reopened, "stdout"):
            raise ValueError("story did not resume at the click")
        before, after = case["captures"]
        if not {before, after}.issubset(reopened["captures"]) or slot_identity(staged) != identity:
            raise ValueError("missing story captures or ordinary slot modified by load")
        result.update(status="visual_review_required", restored_page=before, next_page=after)
    except (OSError, ValueError) as error:
        result["reason"] = str(error)
    write_json(output / "roundtrip.json", result)
    print(result["status"], result.get("reason", ""), flush=True)
    return int(result["status"] == "failed")


if __name__ == "__main__":
    raise SystemExit(main())

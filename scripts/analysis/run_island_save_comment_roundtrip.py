"""Verify ISLAND save comment acceptance, cancellation and cold preview on PC."""

import argparse
from datetime import datetime
import json
import os
from pathlib import Path
import re
import struct
import zlib

from run_pc_ordinary_save_roundtrip import await_resources
from run_playthrough_batch import (
    INVENTORY, identify_player, load_inventory, new_output_directory, playthrough_cache_root, run_case, write_json,
)


INDEX = 3
SCENARIO = Path(__file__).with_name("pc_system_features_island_save_regression.json")
SLOTS = ("save0000.dat", "save0001.dat")
PROMPT = "\u8bf7\u5728\u8fd9\u91cc\u8f93\u5165\u6587\u5b57"
REOPEN = [
    "story-events", "capture-every:0", "no-final-capture", "button-probe",
    "230:557,324", "465:557,324", "525:557,324", "549:557,324", "573:557,324",
    "move:4030:176,414", "require-hit:4050:16", "4050:176,414", "move:4970:636,62", "inspect:5030",
    "move:5040:1056,44", "require-hit:5060:59", "5060:1056,44", "move:5120:636,62", "inspect:5310",
    "move:5320:1122,44", "require-hit:5340:60", "5340:1122,44", "move:5400:636,62", "inspect:5590",
    "move:5600:80,138", "require-hit:5620:39", "5620:80,138", "move:5680:636,62", "inspect:5870",
]
PREVIEW = [380, 147, 675, 165]


def comment_record(path):
    data = path.read_bytes()
    signature, packed, used, _ = struct.unpack_from("<4sIII", data)
    if signature != b"CSS\0" or len(data) != 16 + packed or used > 64 * 1024 * 1024:
        raise ValueError(f"invalid CSS header in {path.name}")
    stream = zlib.decompressobj()
    body = stream.decompress(data[16:], used + 1)
    if not stream.eof or stream.unused_data or len(body) != used:
        raise ValueError(f"invalid CSS body in {path.name}")
    cursor = 0
    while cursor < used:
        record, length = struct.unpack_from("<II", body, cursor)
        cursor += 8
        end = cursor + length
        if end > used:
            raise ValueError(f"truncated CSS record in {path.name}")
        if record == 0x200:
            return body[cursor:end]
        cursor = end
    raise ValueError(f"missing CSS comment record in {path.name}")


def require_replay(result):
    if result["status"] != "visual_review_required" or result["exit_code"] != 0:
        raise ValueError(f"replay ended with {result['status']}")


def verify_writer(writer, comment):
    host_log = Path(writer["stderr"]).read_text(encoding="utf-8")
    prompts = re.findall(r'^frame \d+: text entry title=(".*?") initial=(".*") limit=76 accepted=(true|false)$',
                         host_log, re.MULTILINE)
    if [(json.loads(title), answer) for title, _, answer in prompts] != [(PROMPT, "true"), (PROMPT, "false")]:
        raise ValueError("unexpected save comment title or input result")
    events = Path(writer["stdout"]).read_text(encoding="utf-8")
    for slot in range(len(SLOTS)):
        if not re.search(rf"^frame=\d+ fes-datasave slot={slot} container=\d+ written=true$", events, re.MULTILINE):
            raise ValueError(f"save slot {slot} has no successful write event")
    profile = Path(writer["profile"])
    paths = {name: profile / name for name in SLOTS}
    accepted, cancelled = (comment_record(paths[name]) for name in SLOTS)
    if accepted != comment.encode("utf-8") + b"\0" * 4:
        raise ValueError("confirmed comment did not persist as UTF-8")
    original = cancelled.partition(b"\0")[0].decode("gbk")
    if original.isascii() or any(json.loads(initial) != original for _, initial, _ in prompts):
        raise ValueError("GBK preview and cancelled CSS comment differ")
    return paths, original


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--player", required=True, type=Path)
    parser.add_argument("--cache-root", type=Path, help="absolute directory for the playthrough cache")
    parser.add_argument("--timeout", type=int, default=360)
    args = parser.parse_args()
    if args.timeout < 1:
        parser.error("timeout must be positive")
    player = args.player.resolve(strict=True)
    build = identify_player(player, require_integration=True)
    game = load_inventory(INVENTORY)[0][INDEX]
    writer = json.loads(SCENARIO.read_text(encoding="utf-8"))[str(INDEX)]
    comment, = [entry.removeprefix("text-entry:") for entry in writer["inputs"]
                if entry.startswith("text-entry:")]
    env = os.environ.copy()
    try:
        cache = playthrough_cache_root(args.cache_root)
    except ValueError as error:
        parser.error(str(error))
    output = new_output_directory(cache, datetime.now().astimezone())
    print(output, flush=True)
    save_dir, reopen_dir = output / "save", output / "reopen"
    save_dir.mkdir()
    reopen_dir.mkdir()
    env["CS2_MEDIA_CACHE"] = str(output / "movie-input")
    if os.name == "nt":
        env["PATH"] = os.pathsep.join(("C:/msys64/mingw64/bin", env.get("PATH", "")))
    result = {"status": "failed", "build": build}
    try:
        await_resources(output)
        saved = run_case(INDEX, game, writer["frames"], ["button-probe", *writer["inputs"]],
                         player, save_dir, args.timeout, env,
                         window_checks={int(frame): names for frame, names in writer["window_checks"].items()})
        result["producer"] = saved
        require_replay(saved)
        paths, original = verify_writer(saved, comment)

        await_resources(output)
        reopened = run_case(
            INDEX, game, 5910, REOPEN, player, reopen_dir, args.timeout, env,
            checkpoint_files=paths,
            window_checks={frame: ["flow", "title", "load"] for frame in (5030, 5310, 5590, 5870)},
            pixel_checks=[
                {"before": 5030, "after": 5310, "bounds": PREVIEW, "expect": "changed"},
                {"before": 5030, "after": 5590, "bounds": PREVIEW, "expect": "changed"},
                {"before": 5310, "after": 5590, "bounds": [1032, 20, 1148, 70], "expect": "changed"},
                {"before": 5030, "after": 5870, "bounds": PREVIEW, "expect": "same"},
            ],
        )
        result["consumer"] = reopened
        require_replay(reopened)
        if reopened["pid"] == saved["pid"]:
            raise ValueError("cold preview ran in the producer process")
        for name, path in paths.items():
            if (Path(reopened["profile"]) / name).read_bytes() != path.read_bytes():
                raise ValueError(f"cold preview changed {name}")
        result.update(status="passed", comment=comment, cancelled_comment=original)
    except (OSError, ValueError, struct.error, zlib.error) as error:
        result["reason"] = str(error)
    write_json(output / "island_comment_roundtrip.json", result)
    print(result["status"], result.get("reason", ""), flush=True)
    return int(result["status"] != "passed")


if __name__ == "__main__":
    raise SystemExit(main())

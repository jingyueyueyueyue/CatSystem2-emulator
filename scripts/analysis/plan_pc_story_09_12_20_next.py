"""Generate the next guarded Kanmi and Hatsumira routes without executing a game."""

import argparse
import json
from pathlib import Path

from audit_pc_story_12_op001_movie import SCENES
from audit_runtime_evidence import runtime_fingerprint
from check_pc_story_11_13_sources import read_cst, require
from check_pc_story_12_op001 import BODY_BYTES, PE, RW_MOSAIC_WAITS
from plan_pc_story_09_20_continuation import HERE, kanmi_config


HATSUMIRA_MODES = {"r261": (30, 7500), "r619": (62, 23000), "post_op_r42": (62, 45000)}


def hatsumira_pages(game):
    identity = runtime_fingerprint(game / "Hatsumira.exe")
    require(identity["sha256"] == PE, "Hatsumira PE mismatch")
    seed = int(identity["archive_name_seed"], 16)
    rows, _ = read_cst(game, "scene.int::op001.cst", seed, SCENES["op001"])
    points = []
    previous = -1
    for wait, (group, _, kind, data) in enumerate(rows):
        if kind not in (2, 3):
            continue
        require(kind == 2 and not data and group == len(points), f"unexpected input at r{wait}")
        texts = [(index, len(raw)) for index, (g, _, k, raw) in enumerate(rows[previous + 1:wait], previous + 1)
                 if g == group and k == 0x20]
        sources = [(index, size) for index, size in texts if size]
        require(len(sources) == 1, f"expected one nonempty body at r{wait}")
        body, size = sources[0]
        points.append({"group": group, "wait": wait, "body": body, "bytes": size,
                       "empty": [index for index, length in texts if not length]})
        previous = wait
    require(len(points) == 62 and points[-1]["wait"] == 619
            and tuple(p["wait"] for p in points[:21]) == RW_MOSAIC_WAITS
            and tuple(p["bytes"] for p in points[:21]) == BODY_BYTES,
            "op001 checkpoint sequence differs")
    return points


def hatsumira_config(mode):
    limit, frames = HATSUMIRA_MODES[mode]
    config = json.loads((HERE / "pc_story_12_op001_r222.json").read_text(encoding="utf-8"))
    scenario = config["12"]
    scenario["frames"] = frames
    scenario["route"].update(label=f"Hatsumira: guarded op001 through g{limit - 1}/{mode}",
                             hatsumira_r72=f"observe_{mode}")
    if mode == "post_op_r42":
        scenario["route"].update(label="Hatsumira: natural opening movie and first post-OP wait",
                                 idle_frames=18000)
        scenario["diagnostics"].extend(f"trace-native:{native}" for native in range(265, 271))
    return config


def write_new(path, data):
    payload = json.dumps(data, ensure_ascii=True, indent=2) + "\n"
    if path.exists():
        require(path.read_text(encoding="utf-8") == payload, f"existing archive differs: {path.name}")
        return
    with path.open("x", encoding="utf-8", newline="\n") as stream:
        stream.write(payload)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--run09", type=Path, required=True)
    parser.add_argument("--run12", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    games = {n: Path(json.loads((p / f"{n}.json").read_text(encoding="utf-8"))["game"])
             for n, p in (("09", args.run09), ("12", args.run12))}
    points = hatsumira_pages(games["12"])
    for mode in HATSUMIRA_MODES:
        write_new(args.output / f"20261121_pc_story_12_{mode}.json", hatsumira_config(mode))
    write_new(args.output / "20261121_pc_story_12_op001_waits.json", points)
    for mode in ("sc02_r147", "sc02_r547"):
        write_new(args.output / f"20261121_pc_story_09_{mode}.json", kanmi_config(games["09"], mode))


if __name__ == "__main__":
    main()

"""Replay an archived player against an original game with isolated profile state."""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
from pathlib import Path

from run_playthrough_batch import file_sha256, identify_player


def summarize(prefix: Path, frames: range, expected_poses: int | None = None) -> dict:
    samples = {}
    for line in prefix.with_suffix(".host.log").read_text(encoding="utf-8-sig").splitlines():
        kind = line.split(maxsplit=1)[0] if line else ""
        if kind not in ("replay_perf", "plane_detail", "gpu_output", "emote_gpu"):
            continue
        data = dict(re.findall(r"(\w+)=(\d+)", line))
        key = int(data.pop("frame")), int(data.pop("call"))
        if key[0] not in frames or key[1]:
            continue
        if kind == "replay_perf":
            phases = json.loads(line.split("phase_ns=", 1)[1])
            data.update(compose_emote_us=phases[20] / 1000, compose_plane_us=phases[22] / 1000)
        for field, value in data.items():
            if field != "session":
                samples.setdefault(f"{kind}.{field}", {})[key[0]] = float(value)
    result = {}
    for name, values in samples.items():
        if tuple(sorted(values)) != tuple(frames):
            raise RuntimeError(f"{name} did not cover every requested frame")
        ordered = sorted(values.values())
        middle = len(ordered) // 2
        p50 = (ordered[middle - 1] + ordered[middle]) / 2 if len(ordered) % 2 == 0 else ordered[middle]
        result[name] = {"p50": p50, "p95": ordered[int(len(ordered) * .95) - 1], "max": ordered[-1]}
    if not result:
        raise RuntimeError("No performance samples")
    if expected_poses is not None:
        poses = samples.get("emote_gpu.poses", {})
        for frame in frames:
            if poses.get(frame) != expected_poses:
                raise RuntimeError(f"Expected {expected_poses} E-mote poses at frame {frame}, got {poses.get(frame)}")
    result["screenshots"] = {
        path.name[len(prefix.name) + 1:]: file_sha256(path)
        for path in sorted(prefix.parent.glob(f"{prefix.name}.*.png"))
    }
    return result


def run(args: argparse.Namespace) -> dict:
    if args.frames <= 0 or args.metrics_first < 0 or args.metrics_first >= args.frames:
        raise ValueError("Expected 0 <= metrics-first < frames")
    player = args.player.resolve(strict=True)
    identity = identify_player(player, require_integration=True)
    game_root = args.game_root.resolve(strict=True)
    if not game_root.is_dir():
        raise ValueError("The original game root must be a directory")
    route = json.loads(args.route_args.read_text(encoding="utf-8-sig"))
    if not isinstance(route, list) or len(route) < 3 or not all(isinstance(item, str) for item in route):
        raise ValueError("Route arguments must be a JSON string array including game, output and frame entries")
    if not re.fullmatch(r"[A-Za-z0-9_-]+", args.label):
        raise ValueError("Label must contain only ASCII letters, digits, underscore or hyphen")
    args.output_root.mkdir(parents=True, exist_ok=True)
    prefix = args.output_root / args.label
    profile = prefix.with_suffix(".profile")
    profile.mkdir()
    staged = {}
    for name in ("setup.xml", "save0000.dat", "savegen.dat"):
        source = args.seed_profile / name
        target = profile / name
        shutil.copy2(source, target)
        if file_sha256(source) != file_sha256(target):
            raise RuntimeError(f"Profile seed changed while copying: {name}")
        staged[name] = file_sha256(target)
    route[0] = str(game_root)
    route[1:3] = [str(prefix), str(args.frames)]
    route = [item for item in route if item not in set(args.drop_input)]
    route.extend(args.append_input)
    workers = None if args.workers == "auto" else int(args.workers)
    env = dict(os.environ, CS2_REPLAY_PERF="1", CS2_REPLAY_PERF_DETAIL="1",
               CS2_EMOTE_GPU=args.emote_gpu)
    if workers is None:
        env.pop("CS2_EMOTE_PREP_WORKERS", None)
    else:
        env["CS2_EMOTE_PREP_WORKERS"] = str(workers)
    if args.row_pool is not None:
        env["CS2_RENDERER_ROW_POOL"] = args.row_pool
    with (prefix.with_suffix(".events.log").open("xb") as events,
          prefix.with_suffix(".host.log").open("xb") as host):
        completed = subprocess.run([str(player), *route], env=env, stdout=events, stderr=host,
                                   timeout=args.timeout, check=False)
    if completed.returncode or not prefix.with_suffix(f".{args.frames}.png").is_file():
        raise RuntimeError(f"Replay did not reach frame {args.frames}; returncode={completed.returncode}")
    frames = range(args.metrics_first, args.frames)
    if not frames:
        raise ValueError("Metrics range is empty")
    metrics = summarize(prefix, frames, args.expected_poses)
    receipt = {
        "schema": "cs2-emote-performance/v1", "player": identity, "game_root": str(game_root),
        "game_resources_copied": False, "profile": str(profile), "profile_seed_sha256": staged,
        "argv": [str(player), *route], "emote_gpu": args.emote_gpu, "prepare_workers": workers,
        "row_pool": args.row_pool, "frames": {"first": frames.start, "last": frames.stop - 1},
        "expected_poses": args.expected_poses,
        "metrics": metrics,
    }
    for path, document in [(prefix.with_suffix(".metrics.json"), metrics),
                           (prefix.with_suffix(".receipt.json"), receipt)]:
        with path.open("x", encoding="utf-8") as stream:
            json.dump(document, stream, ensure_ascii=False, indent=2)
            stream.write("\n")
    return metrics


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--player", type=Path, required=True)
    parser.add_argument("--game-root", type=Path, required=True)
    parser.add_argument("--route-args", type=Path, required=True)
    parser.add_argument("--seed-profile", type=Path, required=True)
    parser.add_argument("--output-root", type=Path, required=True)
    parser.add_argument("--label", required=True)
    parser.add_argument("--frames", type=int, default=2590)
    parser.add_argument("--metrics-first", type=int, default=2510)
    parser.add_argument("--emote-gpu", choices=("0", "1"), default="1")
    parser.add_argument("--workers", choices=("auto", "1", "2", "4"), default="4",
                        help="Use auto to inherit the player's default worker policy")
    parser.add_argument("--expected-poses", type=int, choices=(1, 2, 3, 4),
                        help="Require this many GPU E-mote poses in every measured frame")
    parser.add_argument("--row-pool", choices=("0", "1"))
    parser.add_argument("--drop-input", action="append", default=[])
    parser.add_argument("--append-input", action="append", default=[])
    parser.add_argument("--timeout", type=int, default=720)
    args = parser.parse_args()
    try:
        print(json.dumps(run(args), ensure_ascii=False, indent=2))
    except (OSError, ValueError, RuntimeError, KeyError, json.JSONDecodeError) as error:
        parser.exit(2, f"{error}\n")


if __name__ == "__main__":
    main()

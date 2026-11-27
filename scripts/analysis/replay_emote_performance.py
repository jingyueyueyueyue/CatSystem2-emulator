"""Replay an archived player against an original game with isolated profile state."""

from __future__ import annotations

import argparse
import json
import math
import os
import re
import shutil
import subprocess
from pathlib import Path

from run_playthrough_batch import file_sha256, identify_player


def summarize(prefix: Path, frames: range, expected_poses: int | None = None, *,
              interactive: bool = False) -> dict:
    samples = {}
    fallbacks_by_frame = {}
    transitions = []
    previews = {}
    for line in prefix.with_suffix(".host.log").read_text(encoding="utf-8-sig").splitlines():
        kind = line.split(maxsplit=1)[0] if line else ""
        if kind in ("emote_fallback", "fast_forward_state", "replay_preview"):
            fields = dict(re.findall(r"(\w+)=([\w-]+)", line))
            frame = int(fields["frame"])
            if frame not in frames:
                continue
            if kind == "emote_fallback":
                reason = fields["reason"]
                counts = fallbacks_by_frame.setdefault(frame, {})
                counts[reason] = counts.get(reason, 0) + int(fields["count"])
            elif kind == "fast_forward_state":
                transitions.append({"frame": frame, "active": fields["active"] == "true"})
            else:
                previews[frame] = (int(fields["monotonic_us"]), not int(fields["skipped"]))
            continue
        if kind == "movie_scale":
            fields = dict(re.findall(r"(\w+)=(\d+)", line))
            frame = int(fields["frame"])
            if frame in frames and (interactive or int(fields["call"]) == 0):
                name = "movie_scale.reused_us" if int(fields["reused"]) else "movie_scale.draw_us"
                values = samples.setdefault(name, {})
                values[frame] = values.get(frame, 0) + float(fields["elapsed_us"])
            continue
        if kind not in ("replay_perf", "replay_tick", "plane_detail", "gpu_output", "emote_gpu", "emote_scene"):
            continue
        data = dict(re.findall(r"(\w+)=(\d+)", line))
        key = int(data.pop("frame")), int(data.pop("call"))
        if key[0] not in frames or (key[1] and not interactive):
            continue
        if kind == "replay_perf":
            phases = json.loads(line.split("phase_ns=", 1)[1])
            data.update(
                script_children_us=phases[7] / 1000,
                script_list_raster_us=phases[10] / 1000,
                compose_unattributed_us=phases[4] / 1000,
                compose_total_us=(phases[4] + sum(phases[18:25])) / 1000,
                compose_text_us=phases[19] / 1000,
                compose_emote_us=phases[20] / 1000,
                compose_plane_us=phases[22] / 1000,
            )
        for field, value in data.items():
            if field == "session":
                continue
            values = samples.setdefault(f"{kind}.{field}", {})
            number = float(value)
            if kind == "emote_scene" and field.startswith("max_"):
                number = max(number, values.get(key[0], 0))
            elif interactive and (
                kind in ("emote_scene", "gpu_output")
                or field.endswith("_us")
                or field in ("sync_waits", "sync_poses")
            ):
                number += values.get(key[0], 0)
            values[key[0]] = number
    result = {}
    for name, values in samples.items():
        require_all_frames = name.startswith(("replay_perf.", "replay_tick.")) or (
            not interactive and not name.startswith("movie_scale.")
        )
        if require_all_frames and tuple(sorted(values)) != tuple(frames):
            raise RuntimeError(f"{name} did not cover every requested frame")
        ordered = sorted(values.values())
        middle = len(ordered) // 2
        p50 = (ordered[middle - 1] + ordered[middle]) / 2 if len(ordered) % 2 == 0 else ordered[middle]
        result[name] = {
            "samples": len(ordered), "p50": p50,
            "p95": ordered[math.ceil(len(ordered) * .95) - 1],
            "p99": ordered[math.ceil(len(ordered) * .99) - 1], "max": ordered[-1],
        }
        if name in ("replay_perf.elapsed_us", "replay_tick.ready_us"):
            for limit in (33, 50, 100):
                result[name][f"over_{limit}_ms"] = sum(value > limit * 1000 for value in ordered)
        if name in ("emote_scene.updated", "emote_scene.held", "emote_scene.static_held", "emote_scene.stale"):
            result[name]["total"] = int(sum(ordered))
    if not result:
        raise RuntimeError("No performance samples")
    def fallback_counts(first, stop):
        counts = {}
        for frame, reasons in fallbacks_by_frame.items():
            if first <= frame < stop:
                for reason, count in reasons.items():
                    counts[reason] = counts.get(reason, 0) + count
        return counts

    def preview_period(first, stop):
        start_us, end_us = previews[first][0], previews[stop - 1][0]
        duration = (end_us - start_us) / 1e6
        intervals = sum(previews[frame][1] for frame in range(first + 1, stop) if frame in previews)
        return {"first": first, "last": stop - 1, "seconds": duration, "preview_intervals": intervals,
                "preview_hz": intervals / duration if duration > 0 else None,
                "emote_fallbacks": fallback_counts(first, stop),
                "readback_failures": {
                    field: int(sum(value for frame, value in
                                   samples.get(f"gpu_output.{field}", {}).items() if first <= frame < stop))
                    for field in ("device_failed", "packed_failed", "emote_failed")
                }}

    result["emote_fallbacks"] = fallback_counts(frames.start, frames.stop)
    result["fast_forward_transitions"] = transitions
    if previews:
        periods = []
        boundaries = sorted({frames.start, frames.stop, *(event["frame"] for event in transitions)})
        for first, stop in zip(boundaries, boundaries[1:]):
            if first >= stop or first not in previews or stop - 1 not in previews:
                continue
            periods.append(preview_period(first, stop))
        result["preview_periods"] = periods
        result["preview_recovery"] = []
        for event in transitions:
            first = event["frame"]
            if event["active"] or first not in previews:
                continue
            cutoff_us = previews[first][0] + 2_000_000
            next_boundary = boundaries[boundaries.index(first) + 1]
            stop = next((frame for frame in range(first + 1, next_boundary)
                         if frame not in previews or previews[frame][0] > cutoff_us), next_boundary)
            result["preview_recovery"].append(preview_period(first, stop))
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
               CS2_EMOTE_GPU=args.emote_gpu, CS2_REPLAY_INTERACTIVE=str(int(args.interactive)))
    if args.interactive:
        env["CS2_REPLAY_DEVICE_OUTPUT"] = "1"
        env["CS2_REPLAY_FIXED_RETRY_SLEEP"] = str(int(args.retry_sleep == "fixed"))
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
    metrics = summarize(prefix, frames, args.expected_poses, interactive=args.interactive)
    receipt = {
        "schema": "cs2-emote-performance/v1", "player": identity, "game_root": str(game_root),
        "game_resources_copied": False, "profile": str(profile), "profile_seed_sha256": staged,
        "argv": [str(player), *route], "emote_gpu": args.emote_gpu, "prepare_workers": workers,
        "gpu_options": {name: env.get(name) for name in (
            "CS2_REPLAY_DEVICE_OUTPUT", "CS2_GPU_TEXTURED_PLANES", "CS2_EMOTE_MESH_BATCH",
            "CS2_EMOTE_MESH_LAYERED", "CS2_EMOTE_ASYNC_SUBMIT", "CS2_EMOTE_ASYNC_ENCODE", "CS2_EMOTE_ASYNC_ATLAS",
            "CS2_EMOTE_LOAD_ATLAS", "CS2_SCENE_DEFER_READBACK",
        )},
        "row_pool": args.row_pool, "movie_scale_workers": env.get("CS2_MOVIE_SCALE_WORKERS"),
        "frames": {"first": frames.start, "last": frames.stop - 1},
        "expected_poses": args.expected_poses, "interactive": args.interactive,
        "retry_sleep": args.retry_sleep if args.interactive else None,
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
    parser.add_argument("--interactive", action="store_true",
                        help="Use the paced interactive pose and resource workers and count all retry CPU work")
    parser.add_argument("--retry-sleep", choices=("host", "fixed"), default="host",
                        help="Use the host retry delay, or always sleep 1 ms for interactive replay comparison")
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

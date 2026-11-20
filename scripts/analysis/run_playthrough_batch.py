"""Run frame or state-driven PC game scenarios with isolated saves and live logs."""

from __future__ import annotations

import argparse
from collections import deque
from concurrent.futures import FIRST_COMPLETED, ThreadPoolExecutor, wait
import ctypes
from datetime import datetime
import hashlib
import json
import math
import os
from pathlib import Path, PureWindowsPath
import re
import shutil
import stat
import subprocess
import tempfile
import time


ROOT = Path(__file__).resolve().parents[2]
INVENTORY = ROOT / "reports/startup_inventory_20261116.json"
ADDITIONAL_GAMES = ROOT / "reports/pc_playthrough_additional_games_20261120.json"
GIB = 1024 ** 3
STATE_FRAME = re.compile(rb"replay frame=([0-9]+)(?=\s)")
LEGACY_FRAME = re.compile(rb"frame=([0-9]+) logic-frame(?:\s|$)")
STATE_EVENT_FRAME = re.compile(rb"(?:story )?frame=([0-9]+) (?:[a-z][a-z0-9-]*(?=\s|$)|state=)")
REPLAY_STATUS = re.compile(rb"replay_status=([a-z][a-z0-9_]*)(?=\s)")
ENGINE_ERROR = re.compile(rb"frame=([0-9]+) (task-error|vm-error) (.*)")
ROUTE_DIAGNOSTIC = re.compile(r"trace-native:[0-9]+|inspect-log:[1-9][0-9]*")
SHA256 = re.compile(r"[0-9a-f]{64}")
SAVE_NAME = re.compile(r"save(?:[0-9]+|gen)\.dat|slot_.*\.cs2save", re.IGNORECASE)


def write_json(path, value):
    with path.open("x", encoding="utf-8") as stream:
        json.dump(value, stream, ensure_ascii=True, indent=2)
        stream.write("\n")


def file_sha256(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def is_sha256(value):
    return isinstance(value, str) and SHA256.fullmatch(value) is not None


def manifest_path(value):
    if not isinstance(value, str) or any(
        not part or part in (".", "..") or part.endswith((".", " "))
        or re.search(r'[\\:*?"<>|\x00-\x1f]', part) or PureWindowsPath(part).is_reserved()
        for part in value.split("/")
    ):
        raise ValueError("expected a workspace-relative manifest path")
    return Path(value)


def plain_stat(path):
    info = path.lstat()
    if stat.S_ISLNK(info.st_mode) or (
        getattr(info, "st_file_attributes", 0) & stat.FILE_ATTRIBUTE_REPARSE_POINT
    ):
        raise ValueError(f"links and reparse points are not allowed: {path}")
    return info


def checked_path(path):
    if not path.is_absolute() or ".." in path.parts:
        raise ValueError(f"expected an absolute path without traversal: {path}")
    for parent in reversed(path.parents):
        if not stat.S_ISDIR(plain_stat(parent).st_mode):
            raise ValueError(f"not a directory: {parent}")
    return plain_stat(path)


def resource_tree_sha256(root):
    if not stat.S_ISDIR(checked_path(root).st_mode):
        raise ValueError(f"not a game directory: {root}")
    files = []
    pending = [root]
    while pending:
        for path in pending.pop().iterdir():
            manifest_path(path.name)
            info = plain_stat(path)
            name = path.name.lower()
            if SAVE_NAME.fullmatch(name) or name.endswith(".profile") or name in ("save", "saves", "savedata"):
                raise ValueError(f"resource tree contains a save or profile: {path}")
            if stat.S_ISDIR(info.st_mode):
                pending.append(path)
            elif stat.S_ISREG(info.st_mode) and info.st_nlink == 1:
                files.append((path.relative_to(root).as_posix(), path))
            else:
                raise ValueError(f"not a regular unlinked resource: {path}")
    digest = hashlib.sha256()
    for relative, path in sorted(files):
        digest.update(f"{file_sha256(path)}  {relative}\n".encode("utf-8"))
    return digest.hexdigest()


def load_ordinary_saves(path):
    manifest = json.loads(path.read_text(encoding="utf-8"))
    if (not isinstance(manifest, dict) or manifest.get("schema") != "cs2-ordinary-saves/v1"
            or not isinstance(manifest.get("approved"), dict)):
        raise ValueError("invalid ordinary save manifest")
    return manifest["approved"]


def stage_ordinary_save(save, game_path, player, profile):
    if not isinstance(save, dict) or save.get("kind") != "ordinary" or save.get("platform") != "windows-pc":
        raise ValueError("only approved Windows PC ordinary saves are supported")
    evidence = save.get("evidence")
    if not isinstance(evidence, dict) or not all(
        isinstance(evidence.get(key), str) and evidence[key].strip()
        for key in ("write_receipt", "reopen_receipt", "resume_assertion")
    ):
        raise ValueError("ordinary save lacks reviewed write, reopen and resume evidence")
    for role in ("producer", "consumer"):
        identity = save.get(role)
        if not isinstance(identity, dict) or not all(
            is_sha256(identity.get(key)) for key in ("sha256", "source_tree_sha256")
        ):
            raise ValueError(f"ordinary save lacks {role} build identity")
    identity = identify_player(player, require_integration=True)
    if any(identity[key] != save["consumer"][key] for key in ("sha256", "source_tree_sha256")):
        raise ValueError("ordinary save consumer build identity mismatch")

    expected_game = ROOT / manifest_path(save.get("game"))
    if game_path != expected_game:
        raise ValueError("ordinary save game identity mismatch")
    resource_hash = save.get("resource_tree_sha256")
    if not is_sha256(resource_hash) or resource_tree_sha256(expected_game) != resource_hash:
        raise ValueError("ordinary save resource tree SHA-256 mismatch")

    slot, size, sha256 = save.get("slot"), save.get("size"), save.get("sha256")
    if (type(slot) is not int or not 0 <= slot <= 9999 or type(size) is not int
            or size <= 16 or not is_sha256(sha256)):
        raise ValueError("invalid ordinary save slot, size or SHA-256")
    filename = f"save{slot:04}.dat"
    relative = manifest_path(save.get("file"))
    if len(relative.parts) < 3 or relative.parts[0] != "archive" or relative.name != filename:
        raise ValueError("ordinary save must name the approved slot in the workspace archive")
    source = ROOT / relative
    info = checked_path(source)
    if not stat.S_ISREG(info.st_mode) or info.st_nlink != 1 or info.st_size != size:
        raise ValueError("ordinary save is not a regular unlinked file of the approved size")
    checked_path(profile.parent)
    with source.open("rb") as stream:
        header = stream.read(16)
        if header[:4] != b"CSS\0" or int.from_bytes(header[4:8], "little") != size - 16:
            raise ValueError("ordinary save is not a complete CSS slot file")
        stream.seek(0)
        if hashlib.file_digest(stream, "sha256").hexdigest() != sha256:
            raise ValueError("ordinary save source SHA-256 mismatch")
        stream.seek(0)
        profile.mkdir()
        target = profile / filename
        with target.open("xb") as destination:
            shutil.copyfileobj(stream, destination)
    if target.stat().st_size != size or file_sha256(target) != sha256:
        raise ValueError("ordinary save copy does not match the approved slot")
    return {**save, "status": "staged", "target": str(target)}


class ReplayProgress:
    def __init__(self, state_mode):
        self.state_mode = state_mode
        self.frame_pattern = STATE_FRAME if state_mode else LEGACY_FRAME
        self.frame = None
        self.last_advance = None
        self.replay_status = None
        self.first_engine_error = None
        self.event_line = 0

    def sample(self, stream):
        previous = self.frame
        remaining = os.fstat(stream.fileno()).st_size - stream.tell()
        while remaining > 0:
            line = stream.readline(remaining)
            if not line.endswith(b"\n"):
                stream.seek(-len(line), os.SEEK_CUR)
                break
            remaining -= len(line)
            self.event_line += 1
            error_match = ENGINE_ERROR.match(line)
            if error_match and self.first_engine_error is None:
                self.first_engine_error = {
                    "frame": int(error_match[1]),
                    "kind": error_match[2].decode("ascii"),
                    "detail": error_match[3].rstrip(b"\r").decode("utf-8", errors="replace"),
                    "line": self.event_line,
                }
            frame_match = error_match or self.frame_pattern.match(line)
            if self.state_mode and not frame_match:
                frame_match = STATE_EVENT_FRAME.match(line)
            if frame_match:
                frame = int(frame_match[1])
                if self.frame is None or frame > self.frame:
                    self.frame = frame
            status_match = REPLAY_STATUS.match(line)
            if status_match and (self.state_mode or status_match[1] != b"completed"):
                self.replay_status = status_match[1].decode("ascii")
        if self.frame != previous:
            self.last_advance = time.monotonic()

    def status(self, *, save_rejected, timed_out, exit_code, frame_idle_seconds, stall_window):
        if save_rejected:
            return "ordinary_save_rejected"
        if self.first_engine_error is not None or self.replay_status == "engine_error":
            return "engine_error"
        if timed_out:
            if self.frame is None:
                return "timed_out_no_frame"
            if frame_idle_seconds is None or frame_idle_seconds >= stall_window:
                return "timed_out_stalled"
            return "timed_out_progressing"
        if exit_code is None:
            return "launch_error"
        if self.replay_status == "scenario_error":
            return "scenario_error"
        if exit_code != 0:
            return "process_error"
        return self.replay_status or ("missing_replay_status" if self.state_mode else "visual_review_required")


def wait_for_replay(process, events, progress, deadline, poll_interval):
    try:
        while process.poll() is None:
            progress.sample(events)
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                return process.poll() is None
            try:
                process.wait(timeout=min(poll_interval, remaining))
            except subprocess.TimeoutExpired:
                continue
    finally:
        if process.poll() is None:
            process.kill()
            process.wait()
        progress.sample(events)
    return False


def run_case(index, game, frames, inputs, player, output, timeout, env, *,
             route=None, diagnostics=(), stall_seconds=30, ordinary_save=None):
    game_root = ROOT / game["root"]
    game_path = game_root
    if ordinary_save is None:
        game_path = game_root.resolve(strict=True)
        if not game_path.is_dir() or not game_path.is_relative_to(ROOT):
            raise ValueError(f"Game is not a directory in the workspace: {game_path}")
    case_dir = output
    if route is not None:
        case_dir = output / f"{index:02}"
        case_dir.mkdir()
    prefix = case_dir / f"{index:02}"
    command = [str(player), str(game_path), str(prefix), str(frames)]
    route_path = None
    if route is None:
        command.extend(inputs)
    else:
        route_path = prefix.with_suffix(".route.json")
        write_json(route_path, route)
        command.extend(("--route-json", str(route_path), *diagnostics))

    started = time.monotonic()
    profile = prefix.with_suffix(".profile")
    events_path = prefix.with_suffix(".events.log")
    host_path = prefix.with_suffix(".host.log")
    progress = ReplayProgress(route is not None)
    stall_window = min(stall_seconds, timeout / 2)
    timed_out = False
    exit_code = None
    save_receipt = None
    save_rejected = False
    if ordinary_save is not None:
        save_id, save = ordinary_save
        try:
            save_receipt = {**stage_ordinary_save(save, game_root, player, profile), "id": save_id}
        except (OSError, ValueError) as error:
            save_rejected = True
            save_receipt = {"id": save_id, "status": "rejected", "reason": str(error)}
    if not save_rejected:
        deadline = (started if ordinary_save is None else time.monotonic()) + timeout
        with events_path.open("xb") as events, host_path.open("xb") as host, \
                events_path.open("rb") as reader:
            try:
                process = subprocess.Popen(command, cwd=ROOT, env=env, stdout=events, stderr=host)
            except OSError as error:
                host.write(f"{error}\n".encode("utf-8", errors="backslashreplace"))
            else:
                with process:
                    timed_out = wait_for_replay(process, reader, progress, deadline,
                                                min(1, stall_window / 4))
                    if not timed_out:
                        exit_code = process.returncode

    finished = time.monotonic()
    frame_idle_seconds = None if progress.last_advance is None else finished - progress.last_advance
    status = progress.status(save_rejected=save_rejected, timed_out=timed_out,
                             exit_code=exit_code, frame_idle_seconds=frame_idle_seconds,
                             stall_window=stall_window)

    record = {
        "index": index,
        "game": str(game_path),
        "mode": "frame" if route is None else "state",
        "command": command,
        "status": status,
        "replay_status": progress.replay_status,
        "first_engine_error": progress.first_engine_error,
        "timed_out": timed_out,
        "exit_code": exit_code,
        "seconds": round(finished - started, 2),
        "last_frame": progress.frame,
        "frame_idle_seconds": None if frame_idle_seconds is None else round(frame_idle_seconds, 2),
        "output": str(case_dir),
        "route_json": None if route_path is None else str(route_path),
        "profile": str(profile),
        "stdout": None if save_rejected else str(events_path),
        "stderr": None if save_rejected else str(host_path),
        "captures": sorted(path.name for path in case_dir.glob(f"{prefix.name}.*.png")),
    }
    if save_receipt is not None:
        record["ordinary_save"] = save_receipt
    write_json(output / f"{index:02}.json", record)
    return record


def available_ram_bytes():
    if os.name == "nt":
        class MemoryStatus(ctypes.Structure):
            _fields_ = [("length", ctypes.c_uint32), ("load", ctypes.c_uint32)] + [
                (field, ctypes.c_uint64) for field in (
                    "total_phys", "avail_phys", "total_page", "avail_page",
                    "total_virtual", "avail_virtual", "avail_extended_virtual",
                )
            ]

        memory = MemoryStatus()
        memory.length = ctypes.sizeof(memory)
        query = ctypes.WinDLL("kernel32", use_last_error=True).GlobalMemoryStatusEx
        query.argtypes = (ctypes.POINTER(MemoryStatus),)
        query.restype = ctypes.c_int
        if not query(ctypes.byref(memory)):
            raise ctypes.WinError(ctypes.get_last_error())
        return memory.avail_phys
    return os.sysconf("SC_AVPHYS_PAGES") * os.sysconf("SC_PAGE_SIZE")


def run_batch(cases, player, output, args, env, ordinary_saves=None):
    pending = deque(cases)
    active = set()
    save_rejected = False
    with ThreadPoolExecutor(max_workers=args.jobs) as pool:
        while pending or active:
            while pending and len(active) < args.jobs:
                free_ram = available_ram_bytes()
                free_disk = shutil.disk_usage(output).free
                required_ram = int((args.reserve_ram_gib + (len(active) + 1) * args.ram_per_job_gib) * GIB)
                required_disk = int((args.reserve_disk_gib + (len(active) + 1) * args.disk_per_job_gib) * GIB)
                if free_ram < required_ram or free_disk < required_disk:
                    if active:
                        break
                    resources = {
                        "free_ram_bytes": free_ram, "required_ram_bytes": required_ram,
                        "free_disk_bytes": free_disk, "required_disk_bytes": required_disk,
                    }
                    for index, game, settings in pending:
                        write_json(output / f"{index:02}.json", {
                            "index": index, "game": game["root"],
                            "mode": "state" if "route" in settings else "frame",
                            "status": "resource_deferred", "exit_code": None, "seconds": 0, "captures": [],
                            "resources": resources,
                        })
                        print(f"{index:02}: resource_deferred", flush=True)
                    print(f"Admission paused: {json.dumps(resources)}", flush=True)
                    return 1
                index, game, settings = pending.popleft()
                save_id = settings.get("ordinary_save")
                ordinary_save = None if save_id is None else (save_id, ordinary_saves[save_id])
                active.add(pool.submit(run_case, index, game, settings.get("frames", args.frames),
                                       settings.get("inputs", []), player, output, args.timeout, env,
                                       route=settings.get("route"), diagnostics=settings.get("diagnostics", ()),
                                       stall_seconds=args.stall_seconds,
                                       ordinary_save=ordinary_save))
            if active:
                done, active = wait(active, return_when=FIRST_COMPLETED)
                for task in done:
                    result = task.result()
                    save_rejected |= result["status"] == "ordinary_save_rejected"
                    print(f'{result["index"]:02}: {result["status"]}, {result["seconds"]}s', flush=True)
    return int(save_rejected)


def load_inventory(path):
    games = json.loads(path.read_text(encoding="utf-8"))["games"]
    default_inventory = path.resolve() == INVENTORY
    if default_inventory:
        additions = json.loads(ADDITIONAL_GAMES.read_text(encoding="utf-8"))["games"]
        games.extend(additions)
    return games, range(1 if default_inventory else 0, len(games))


def load_scenarios(paths, indices):
    merged = {}
    owners = {}
    for path in paths:
        document = json.loads(path.read_text(encoding="utf-8"))
        if not isinstance(document, dict):
            raise ValueError(f"scenarios must be a JSON object keyed by sample index: {path}")
        for index in indices:
            key = str(index)
            if key not in document:
                continue
            if key in merged:
                raise ValueError(f"scenario {index} occurs in both {owners[key]} and {path}")
            merged[key] = document[key]
            owners[key] = path
    if len(paths) > 1:
        missing = [str(index) for index in indices if str(index) not in merged]
        if missing:
            raise ValueError(f"no scenario provided for selected indices: {', '.join(missing)}")
    return merged


def new_output_directory(cache_root, now):
    month = cache_root.resolve() / now.strftime("%Y-%m")
    month.mkdir(parents=True, exist_ok=True)
    return Path(tempfile.mkdtemp(prefix=now.strftime("%Y%m%d_%H%M%S_%f_"),
                                 suffix="_playthrough", dir=month))


def identify_player(player, *, require_integration=False):
    provenance = player.with_name("build.json")
    if require_integration and not all(
        stat.S_ISREG(checked_path(path).st_mode) for path in (player, provenance)
    ):
        raise ValueError("ordinary save requires an archived integration player and build.json")
    sha256 = file_sha256(player)
    identity = {"player": str(player), "sha256": sha256}
    if not provenance.is_file():
        return identity
    build = json.loads(provenance.read_text(encoding="utf-8"))
    if not isinstance(build, dict):
        raise ValueError(f"Invalid replay build receipt: {provenance}")
    if require_integration and build.get("build_stage") != "integration":
        raise ValueError("ordinary save requires an integration build receipt")
    artifacts = build.get("artifacts")
    artifact = artifacts.get(str(player)) if isinstance(artifacts, dict) else None
    if build.get("schema") != "cs2-build-attempt/v1" or not isinstance(artifact, dict):
        if require_integration:
            raise ValueError("ordinary save requires matching executable provenance")
        return identity
    before, after = (build.get(key) for key in ("source_before", "source_after"))
    before = before.get("sha256") if isinstance(before, dict) else None
    after = after.get("sha256") if isinstance(after, dict) else None
    if (build.get("status") != "built" or build.get("source_consistent") is not True
            or not before or before != after or artifact.get("sha256") != sha256
            or (require_integration and not is_sha256(after))):
        raise ValueError(f"Replay player does not match a consistent build: {player}")
    return {**identity, "source_tree_sha256": after, "provenance": str(provenance)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--player", type=Path, required=True)
    parser.add_argument("--inventory", type=Path, default=INVENTORY,
                        help="local JSON inventory containing a games array with workspace-relative or absolute root paths")
    parser.add_argument("--indices", nargs="+", type=int,
                        help="sample indices (default: all games except the V401 fixture in the workspace inventory)")
    parser.add_argument("--scenarios", type=Path, action="append", default=[],
                        help="JSON keyed by sample index; repeat for disjoint groups of selected games")
    parser.add_argument("--ordinary-saves", type=Path,
                        help="reviewed manifest; only scenarios with an approved ordinary_save ID preload a slot")
    parser.add_argument("--frames", type=int, default=12)
    parser.add_argument("--jobs", type=int, default=2)
    parser.add_argument("--timeout", type=int, default=600)
    parser.add_argument("--stall-seconds", type=float, default=30,
                        help="timeout stall window in seconds (capped at half the timeout)")
    parser.add_argument("--ram-per-job-gib", type=float, default=2, help="RAM budget per replay (default 2 GiB)")
    parser.add_argument("--disk-per-job-gib", type=float, default=2, help="disk budget per replay (default 2 GiB)")
    parser.add_argument("--reserve-ram-gib", type=float, default=2, help="RAM headroom (default 2 GiB)")
    parser.add_argument("--reserve-disk-gib", type=float, default=20, help="disk headroom (default 20 GiB)")
    args = parser.parse_args()
    games, default_indices = load_inventory(args.inventory)
    indices = list(dict.fromkeys(args.indices)) if args.indices else default_indices
    if any(index < 0 or index >= len(games) for index in indices):
        parser.error("sample index outside the inventory")
    if min(args.jobs, args.frames, args.timeout) < 1:
        parser.error("jobs, frames and timeout must be positive")
    budgets = (args.stall_seconds, args.ram_per_job_gib, args.disk_per_job_gib,
               args.reserve_ram_gib, args.reserve_disk_gib)
    if any(not math.isfinite(value) or value <= 0 for value in budgets):
        parser.error("stall seconds and resource budgets must be finite and positive")
    try:
        scenarios = load_scenarios(args.scenarios, indices)
        ordinary_saves = load_ordinary_saves(args.ordinary_saves) if args.ordinary_saves else {}
    except (OSError, ValueError) as error:
        parser.error(str(error))
    cases = []
    for index in indices:
        settings = scenarios.get(str(index), {})
        if not isinstance(settings, dict):
            parser.error(f"scenario {index} must be a JSON object")
        frames = settings.get("frames", args.frames)
        inputs = settings.get("inputs", [])
        if type(frames) is not int or frames < 1:
            parser.error(f"scenario {index} frames must be a positive integer")
        if not isinstance(inputs, list) or any(not isinstance(item, str) for item in inputs):
            parser.error(f"scenario {index} inputs must be a JSON array of strings")
        if "route" in settings and (not isinstance(settings["route"], dict) or inputs):
            parser.error(f"scenario {index} route must be a JSON object without frame inputs")
        if "diagnostics" in settings:
            diagnostics = settings["diagnostics"]
            if "route" not in settings or not isinstance(diagnostics, list) or any(
                not isinstance(flag, str) or not ROUTE_DIAGNOSTIC.fullmatch(flag) for flag in diagnostics
            ):
                parser.error(f"scenario {index} diagnostics require a route and trace-native or inspect-log flags")
        if "ordinary_save" in settings:
            save_id = settings["ordinary_save"]
            if not isinstance(save_id, str) or not save_id or save_id not in ordinary_saves:
                parser.error(f"scenario {index} ordinary_save must name an approved --ordinary-saves entry")
        cases.append((index, games[index], settings))

    now = datetime.now().astimezone()
    default_root = Path("F:/CS2-build-cache/playthrough") if os.name == "nt" else ROOT / "archive"
    cache_override = os.environ.get("CS2_BUILD_CACHE_ROOT")
    cache_root = Path(cache_override) / "playthrough" if cache_override else default_root
    if not cache_root.is_absolute():
        parser.error("CS2_BUILD_CACHE_ROOT must be an absolute path")
    try:
        if any("ordinary_save" in settings for _, _, settings in cases):
            candidate = args.player.absolute()
            checked_path(candidate)
            try:
                checked_path(candidate.with_name("build.json"))
            except FileNotFoundError:
                pass
        player = args.player.resolve(strict=True)
        source = identify_player(player)
    except (OSError, ValueError) as error:
        parser.error(str(error))
    output = new_output_directory(cache_root, now)
    write_json(output / "build_identity.json", source)
    print(output, flush=True)
    env = os.environ.copy()
    env.setdefault("CS2_MEDIA_CACHE", str(output / "movie-input"))
    if os.name == "nt":
        env["PATH"] = os.pathsep.join(("C:/msys64/mingw64/bin", env.get("PATH", "")))
    return run_batch(cases, player, output, args, env, ordinary_saves)


if __name__ == "__main__":
    raise SystemExit(main())

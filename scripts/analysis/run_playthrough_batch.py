"""Run frame or state-driven PC game scenarios with isolated saves and live logs."""

from __future__ import annotations

import argparse
from collections import deque
from concurrent.futures import FIRST_COMPLETED, ThreadPoolExecutor, wait
from contextlib import nullcontext
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
from xml.etree import ElementTree as ET

from batch_progress import BatchProgress


ROOT = Path(__file__).resolve().parents[2]
INVENTORY = ROOT / "reports/startup_inventory_20261116.json"
ADDITIONAL_GAMES = ROOT / "reports/pc_playthrough_additional_games_20261120.json"
GIB = 1024 ** 3
ADMISSION_POLL_SECONDS = 15
STATE_FRAME = re.compile(rb"replay frame=([0-9]+)(?=\s)")
LEGACY_FRAME = re.compile(rb"frame=([0-9]+) logic-frame(?:\s|$)")
STATE_EVENT_FRAME = re.compile(rb"(?:story )?frame=([0-9]+) (?:[a-z][a-z0-9-]*(?=\s|$)|state=)")
STORY_POSITION = re.compile(
    rb"(?:story|replay) frame=([0-9]+) state=([^ ]+) scene=(.*) group=(-?[0-9]+) record=(-?[0-9]+)(?=\s)"
)
ROUTE_STOP = re.compile(rb"replay frame=([0-9]+): (.+)")
TRAILING_MOVIE_DIAGNOSTIC = re.compile(rb"frame [0-9]+: movie .+")
REPLAY_STATUS = re.compile(rb"replay_status=([a-z][a-z0-9_]*)(?=\s)")
ENDING_EVENT = re.compile(rb"replay frame=([0-9]+) ending_(card|verified)=([A-Za-z0-9_]+)(?=\s)")
ENGINE_ERROR = re.compile(rb"frame=([0-9]+) (task-error|vm-error) (.*)")
ROUTE_DIAGNOSTIC = re.compile(r"trace-native:[0-9]+|inspect-log:[1-9][0-9]*")
WINDOW_SNAPSHOT = re.compile(r"^frame (\d+): wait .*? windows\(owner,.*?\)=\[(.*?)\] body_wait_shell\(")
WINDOW_NAME = re.compile(r'\(\d+, "([a-zA-Z0-9_.]+)", (?:true|false), ')
REPLAY_EVENT_LINE = re.compile(r"^frame=(\d+) (.*)$")
GUARDED_BODY_INPUT = re.compile(
    r"legacy-(?:(?:02|06|16|19)-(?:enter|continue|opening|segment)|02-next-segment|(?:02|16)-continuous"
    r"|(?:04|05)-(?:enter|opening|segment)"
    r"|11-13-(?:enter|continue):(?:11|13)|11-radio-(?:stop|r94-enter)|13-r(?:139|188))"
    r"|labyrinth15-(?:next|opening|r19-enter-observe|cross-window-successor)|djibril10-op01-next"
)
SHA256 = re.compile(r"[0-9a-f]{64}")
SAVE_NAME = re.compile(r"save(?:[0-9]+|gen)\.dat|slot_.*\.cs2save", re.IGNORECASE)
REUSABLE_ATTEMPT_STATUSES = {
    "checkpoint_reached", "engine_error", "unexpected_branch", "unknown_state",
    "unrecognized_wait", "input_missed", "checkpoint_mismatch", "visual_review_required",
}


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


def resource_tree_sha256(root, *, include_saves=False):
    if not stat.S_ISDIR(checked_path(root).st_mode):
        raise ValueError(f"not a game directory: {root}")
    files = []
    pending = [root]
    while pending:
        for path in pending.pop().iterdir():
            manifest_path(path.name)
            info = plain_stat(path)
            name = path.name.lower()
            if not include_saves and (SAVE_NAME.fullmatch(name) or name.endswith(".profile")
                                      or name in ("save", "saves", "savedata")):
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


def complete_css_file(path, size):
    with path.open("rb") as stream:
        header = stream.read(16)
    return (len(header) == 16 and header[:4] == b"CSS\0"
            and int.from_bytes(header[4:8], "little") <= size - 16)


def load_checkpoint_files(files):
    if not isinstance(files, dict) or not files:
        raise ValueError("checkpoint_files must map profile filenames to archived files")
    staged = {}
    for name, source in files.items():
        if not (re.fullmatch(r"save(?:[0-9]{4}|gen)\.dat", name) or name in {"dic.dat", "setup.xml"}):
            raise ValueError(f"invalid checkpoint profile filename: {name}")
        relative = manifest_path(source)
        if len(relative.parts) < 3 or relative.parts[0] != "archive":
            raise ValueError("checkpoint sources must be in the workspace archive")
        path = ROOT / relative
        info = checked_path(path)
        if not stat.S_ISREG(info.st_mode):
            raise ValueError(f"checkpoint source is not a regular file: {path}")
        if SAVE_NAME.fullmatch(name) and not complete_css_file(path, info.st_size):
            raise ValueError(f"checkpoint source is not a complete CSS file: {path}")
        staged[name] = path
    return staged


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
    def __init__(self, state_mode, guarded_body=False):
        self.state_mode = state_mode
        self.expects_checkpoint = state_mode or guarded_body
        self.frame_pattern = STATE_FRAME if state_mode else LEGACY_FRAME
        self.frame = None
        self.last_advance = None
        self.replay_status = None
        self.first_engine_error = None
        self.last_story_position = None
        self.last_page_wait = None
        self.event_line = 0
        self.ending_card = None
        self.verified_ending = None

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
            if status_match and (self.expects_checkpoint or status_match[1] != b"completed"):
                self.replay_status = status_match[1].decode("ascii")
            if position := STORY_POSITION.match(line):
                frame = int(position[1])
                if self.last_story_position is None or frame >= self.last_story_position["frame"]:
                    observed = {
                        "frame": frame,
                        "state": position[2].decode("utf-8", errors="replace"),
                        "scene": position[3].decode("utf-8", errors="replace"),
                        "group": int(position[4]),
                        "record": int(position[5]),
                        "line": self.event_line,
                    }
                    self.last_story_position = observed
                    if observed["state"] == "page" and observed["group"] >= 0 and observed["record"] >= 0:
                        self.last_page_wait = observed
            if self.state_mode and (event := ENDING_EVENT.match(line)):
                frame, kind, name = int(event[1]), event[2], event[3].decode("ascii")
                if kind == b"card":
                    self.ending_card = (name, frame)
                elif self.ending_card and self.ending_card[0] == name and self.ending_card[1] < frame:
                    self.verified_ending = {"id": name, "card_frame": self.ending_card[1],
                                            "clear_verified_frame": frame, "log_line": self.event_line}
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
        if self.replay_status == "completed":
            return "checkpoint_reached"
        return self.replay_status or ("missing_replay_status" if self.expects_checkpoint else "visual_review_required")


def wait_for_replay(process, events, progress, deadline, poll_interval, *, watchdog_seconds=None):
    started = time.monotonic()
    try:
        while process.poll() is None:
            progress.sample(events)
            remaining = deadline - time.monotonic()
            if watchdog_seconds is not None:
                last_tick = progress.last_advance if progress.last_advance is not None else started
                watchdog_remaining = watchdog_seconds - (time.monotonic() - last_tick)
                if watchdog_remaining <= 0 and process.poll() is None:
                    progress.watchdog_stalled = True
                    return True
                remaining = min(remaining, watchdog_remaining)
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


def route_stop_diagnostics(host_path):
    # A guarded replay can append movie diagnostics after the final wait snapshot.
    with host_path.open("rb") as stream:
        stream.seek(max(0, os.fstat(stream.fileno()).st_size - 65536))
        lines = stream.read().splitlines()
    diagnostics = {}
    while lines and TRAILING_MOVIE_DIAGNOSTIC.fullmatch(lines[-1]):
        lines.pop()
    if lines and lines[-1].startswith(b"replay wait: "):
        diagnostics["wait"] = lines.pop()[len(b"replay wait: "):].decode("utf-8", errors="replace")
    if lines and (reason := ROUTE_STOP.fullmatch(lines[-1])) and not reason[2].startswith(b"verified "):
        diagnostics.update(frame=int(reason[1]), reason=reason[2].decode("utf-8", errors="replace"))
    return diagnostics or None


def check_fes_windows(host_path, checks):
    snapshots = {}
    with host_path.open(encoding="utf-8", errors="replace") as stream:
        for line in stream:
            if (match := WINDOW_SNAPSHOT.match(line)) and (frame := int(match[1])) in checks:
                snapshots[frame] = WINDOW_NAME.findall(match[2])
    return [
        {"frame": frame, "expected": expected, "actual": snapshots.get(frame),
         "passed": frame in snapshots and set(expected) == set(snapshots[frame])}
        for frame, expected in checks.items()
    ]


def check_replay_events(events_path, checks):
    rows = [{**check, "matched_frame": None} for check in checks]
    with events_path.open(encoding="utf-8", errors="replace") as stream:
        for line in stream:
            match = REPLAY_EVENT_LINE.match(line)
            if match is None:
                continue
            frame = int(match[1])
            for row in rows:
                if (row["matched_frame"] is None and row["from"] <= frame <= row["through"]
                        and row["contains"] in match[2]):
                    row["matched_frame"] = frame
    return rows


def check_replay_pixels(prefix, checks):
    from PIL import Image, ImageChops

    crops = {}
    rows = []
    for check in checks:
        bounds = tuple(check["bounds"])
        for frame in (check["before"], check["after"]):
            key = (frame, bounds)
            if key not in crops:
                path = prefix.with_name(f"{prefix.name}.{frame:04}.png")
                with Image.open(path) as image:
                    if bounds[2] > image.width or bounds[3] > image.height:
                        raise ValueError(f"pixel check bounds exceed capture: {path}")
                    crops[key] = image.crop(bounds).convert("RGB")
        difference = ImageChops.difference(crops[check["before"], bounds], crops[check["after"], bounds])
        changed = difference.getbbox() is not None
        rows.append({**check, "changed": changed, "passed": changed == (check["expect"] == "changed")})
    return rows


def seed_fast_text_profile(profile):
    """Set the fastest standard message speed in a private replay profile."""
    profile.mkdir(exist_ok=True)
    setup = profile / "setup.xml"
    if setup.exists():
        document = ET.parse(setup).getroot()
        if document.tag != "document":
            raise ValueError(f"Unexpected setup.xml root: {setup}")
    else:
        document = ET.Element("document")
    message = document.find("MESSAGE")
    if message is None:
        message = ET.SubElement(document, "MESSAGE")
    speed = message.find("speed")
    if speed is None:
        speed = ET.SubElement(message, "speed")
    speed.text = "0"
    ET.ElementTree(document).write(setup, encoding="utf-8", xml_declaration=True)
    return {"message_speed": 0, "setup_sha256": file_sha256(setup)}


def load_attempt_history(cache_root, indices):
    receipts = {}
    filenames = {f"{index:02}.json": index for index in indices}
    paths = cache_root.glob("[0-9][0-9][0-9][0-9]-[0-9][0-9]/*_playthrough/??.json")
    for path in sorted(paths, reverse=True):
        index = filenames.get(path.name)
        if index is None:
            continue
        try:
            receipt = json.loads(path.read_text(encoding="utf-8"))
            key = receipt.get("attempt_key")
            if receipt.get("status") == "visual_review_required" and (
                receipt.get("exit_code") != 0
                or receipt.get("last_frame") != int(receipt["command"][3])
                or not all((Path(receipt["output"]) / capture).is_file()
                           for capture in receipt["captures"])
            ):
                continue
            if (receipt.get("index") == index and is_sha256(key)
                    and receipt.get("status") in REUSABLE_ATTEMPT_STATUSES
                    and isinstance(receipt.get("seconds"), (float, int))
                    and not receipt.get("timed_out")
                    and all(Path(receipt[field]).is_file() for field in ("stdout", "stderr"))):
                receipts.setdefault((index, key), (path, receipt))
        except (OSError, ValueError, TypeError, AttributeError, KeyError, IndexError):
            continue
    return receipts


def attempt_key(player_sha256, game_path, command_args, timeout, stall_seconds, env,
                save_receipt, text_setup, neko_language):
    # A different build, resource tree, route, or starting profile requires a new replay.
    cache_variables = {"CS2_BUILD_CACHE_ROOT", "CS2_MEDIA_CACHE"}
    settings = {
        "version": 1,
        "player_sha256": player_sha256,
        "game": str(game_path),
        "resource_tree_sha256": (save_receipt["resource_tree_sha256"] if save_receipt
                                 else resource_tree_sha256(game_path, include_saves=True)),
        "command_args": command_args,
        "timeout": timeout,
        "stall_seconds": stall_seconds,
        "environment": {key: value for key, value in env.items()
                        if key.startswith("CS2_") and key not in cache_variables},
        "slot": [save_receipt["slot"], save_receipt["sha256"]] if save_receipt else None,
        "fast_text": text_setup,
        "neko_language": neko_language,
    }
    encoded = json.dumps(settings, sort_keys=True, separators=(",", ":")).encode("ascii")
    return hashlib.sha256(encoded).hexdigest()


def run_case(index, game, frames, inputs, player, output, timeout, env, *,
             route=None, diagnostics=(), stall_seconds=30, ordinary_save=None, neko_language=None,
             fast_text=False, reuse_attempts=None, game_root_override=None, checkpoint_files=None,
             watchdog=False, window_checks=None, event_checks=None, pixel_checks=None, save_checks=None):
    game_root = game_root_override if game_root_override is not None else ROOT / game["root"]
    game_path = game_root
    if ordinary_save is None:
        game_path = game_root.resolve(strict=True)
        if not game_path.is_dir():
            raise ValueError(f"Not a game directory: {game_path}")
        if game_root_override is None and not game_path.is_relative_to(ROOT):
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
    if checkpoint_files:
        profile.mkdir()
        for name, source in checkpoint_files.items():
            target = profile / name
            shutil.copyfile(source, target)
            # Games use save timestamps to choose the initial slot and page.
            os.utime(target, ns=(target.stat().st_atime_ns, source.stat().st_mtime_ns))
    if neko_language is not None:
        from stage_neko_cold_language import seed_language_profile

        seed_language_profile(profile, neko_language)
    events_path = prefix.with_suffix(".events.log")
    host_path = prefix.with_suffix(".host.log")
    progress = ReplayProgress(route is not None, guarded_body=any(GUARDED_BODY_INPUT.fullmatch(item) for item in inputs))
    stall_window = min(stall_seconds, timeout / 2)
    timed_out = False
    exit_code = None
    pid = None
    save_receipt = None
    text_setup = None
    save_rejected = False
    key = None
    reuse_unavailable = None
    if ordinary_save is not None:
        save_id, save = ordinary_save
        try:
            save_receipt = {**stage_ordinary_save(save, game_root, player, profile), "id": save_id}
        except (OSError, ValueError) as error:
            save_rejected = True
            save_receipt = {"id": save_id, "status": "rejected", "reason": str(error)}
    if not save_rejected:
        if fast_text:
            text_setup = seed_fast_text_profile(profile)
        if reuse_attempts is not None and not any((checkpoint_files, window_checks, event_checks, pixel_checks, save_checks)):
            player_sha256, history = reuse_attempts
            try:
                key = attempt_key(player_sha256, game_path, [frames, route, inputs, diagnostics],
                                  timeout, stall_seconds, env, save_receipt, text_setup, neko_language)
            except (OSError, ValueError) as error:
                reuse_unavailable = str(error)
            previous = history.get((index, key))
            if previous is not None:
                path, receipt = previous
                record = {
                    "index": index, "game": str(game_path), "status": "skipped_attempt",
                    "attempt_key": key, "previous_status": receipt["status"],
                    "previous_receipt": str(path), "previous_seconds": receipt["seconds"],
                    "last_story_position": receipt.get("last_story_position"),
                    "first_engine_error": receipt.get("first_engine_error"),
                    "route_stop": receipt.get("route_stop"),
                    "seconds": 0, "captures": [],
                }
                write_json(output / f"{index:02}.json", record)
                return record
        deadline = time.monotonic() + timeout
        with events_path.open("xb") as events, host_path.open("xb") as host, \
                events_path.open("rb") as reader:
            try:
                process = subprocess.Popen(command, cwd=ROOT, env=env, stdout=events, stderr=host,
                                           creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
            except OSError as error:
                host.write(f"{error}\n".encode("utf-8", errors="backslashreplace"))
            else:
                with process:
                    pid = process.pid
                    timed_out = wait_for_replay(process, reader, progress, deadline,
                                                min(1, stall_window / 4),
                                                watchdog_seconds=stall_seconds if watchdog else None)
                    if not timed_out:
                        exit_code = process.returncode

    finished = time.monotonic()
    frame_idle_seconds = None if progress.last_advance is None else finished - progress.last_advance
    status = progress.status(save_rejected=save_rejected, timed_out=timed_out,
                             exit_code=exit_code, frame_idle_seconds=frame_idle_seconds,
                             stall_window=stall_window)
    if getattr(progress, "watchdog_stalled", False) and status != "engine_error":
        status = "watchdog_stalled"

    record = {
        "index": index,
        "game": str(game_path),
        "mode": "frame" if route is None else "state",
        "command": command,
        "status": status,
        "replay_status": progress.replay_status,
        "first_engine_error": progress.first_engine_error,
        "last_story_position": progress.last_story_position,
        "last_page_wait": progress.last_page_wait,
        "timed_out": timed_out,
        "exit_code": exit_code,
        "pid": pid,
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
    if checkpoint_files:
        record["checkpoint_files"] = {name: str(path) for name, path in checkpoint_files.items()}
    if record["status"] in {"visual_review_required", "frame_limit"}:
        if event_checks:
            record["event_checks"] = check_replay_events(events_path, event_checks)
            if any(row["matched_frame"] is None for row in record["event_checks"]):
                record["status"] = "event_check_failed"
        if window_checks:
            record["window_checks"] = check_fes_windows(host_path, window_checks)
            if not all(check["passed"] for check in record["window_checks"]):
                record["status"] = "window_check_failed"
        if pixel_checks:
            record["pixel_checks"] = check_replay_pixels(prefix, pixel_checks)
            if not all(check["passed"] for check in record["pixel_checks"]):
                record["status"] = "pixel_check_failed"
        if save_checks:
            record["save_checks"] = []
            for name, seed in save_checks.items():
                expected = file_sha256(checkpoint_files[seed]) if seed is not None else None
                path = profile / name
                actual = file_sha256(path) if path.is_file() else None
                complete = actual is not None and complete_css_file(path, path.stat().st_size)
                record["save_checks"].append({
                    "file": name, "seed": seed, "expected_sha256": expected,
                    "actual_sha256": actual, "passed": complete and (seed is None or actual == expected),
                })
            if not all(check["passed"] for check in record["save_checks"]):
                record["status"] = "save_check_failed"
    if neko_language is not None:
        record["neko_language"] = neko_language
    if text_setup is not None:
        record["fast_text"] = text_setup
    if key is not None:
        record["attempt_key"] = key
    if reuse_unavailable is not None:
        record["attempt_reuse_unavailable"] = reuse_unavailable
    if progress.expects_checkpoint and not save_rejected and status in {
        "unexpected_branch", "unknown_state", "unrecognized_wait", "input_missed",
        "checkpoint_mismatch", "frame_limit",
    }:
        record["route_stop"] = route_stop_diagnostics(host_path)
    if status == "checkpoint_reached" and progress.verified_ending:
        ending = progress.verified_ending
        capture = f"{prefix.name}.{ending['card_frame']:04}.png"
        if capture in record["captures"]:
            record["status"] = "ending_verified"
            record["ending"] = {**ending, "card": capture}
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


def run_batch(cases, player, output, args, env, ordinary_saves=None, *, fast_text=False,
              reuse_attempts=None, game_root_override=None, ledger=None, strict=False):
    pending = deque(cases)
    active = {}
    case_failed = False
    admission_deadline = None
    with ThreadPoolExecutor(max_workers=args.jobs) as pool:
        while pending or active:
            while pending and len(active) < args.jobs:
                free_ram = available_ram_bytes()
                free_disk = shutil.disk_usage(output).free
                # Available resources already exclude resident processes and written files.
                required_ram = int((args.reserve_ram_gib + args.ram_per_job_gib) * GIB)
                required_disk = int((args.reserve_disk_gib + args.disk_per_job_gib) * GIB)
                if free_ram < required_ram or free_disk < required_disk:
                    if active:
                        break
                    resources = {
                        "free_ram_bytes": free_ram, "required_ram_bytes": required_ram,
                        "free_disk_bytes": free_disk, "required_disk_bytes": required_disk,
                    }
                    now = time.monotonic()
                    if admission_deadline is None and args.admission_wait_seconds > 0:
                        admission_deadline = now + args.admission_wait_seconds
                        print(f"Waiting for playback resources: {json.dumps(resources)}", flush=True)
                    if admission_deadline is not None and now < admission_deadline:
                        time.sleep(min(ADMISSION_POLL_SECONDS, admission_deadline - now))
                        continue
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
                if admission_deadline is not None:
                    print("Playback resources available; resuming batch", flush=True)
                    admission_deadline = None
                index, game, settings = pending.popleft()
                receipt = output / f"{index:02}.json"
                if settings.get("defer"):
                    result = {"index": index, "game": game["root"], "status": "scenario_deferred",
                              "reason": settings["defer"], "exit_code": None, "seconds": 0, "captures": []}
                    write_json(receipt, result)
                    case_failed |= strict
                    if ledger:
                        ledger.finish(game["root"], result, receipt)
                    continue
                save_id = settings.get("ordinary_save")
                ordinary_save = None if save_id is None else (save_id, ordinary_saves[save_id])
                if ledger:
                    ledger.start(game["root"], receipt)
                task = pool.submit(run_case, index, game, settings.get("frames", args.frames),
                                   settings.get("inputs", []), player, output, args.timeout, env,
                                   route=settings.get("route"), diagnostics=settings.get("diagnostics", ()),
                                   stall_seconds=args.stall_seconds,
                                   ordinary_save=ordinary_save, neko_language=settings.get("neko_language"),
                                   fast_text=fast_text, reuse_attempts=reuse_attempts,
                                   game_root_override=game_root_override,
                                   checkpoint_files=settings.get("checkpoint_files"), watchdog=ledger is not None,
                                   window_checks=settings.get("window_checks"),
                                   event_checks=settings.get("event_checks"),
                                   pixel_checks=settings.get("pixel_checks"), save_checks=settings.get("save_checks"))
                active[task] = (index, game, settings)
            if active:
                recheck_resources = pending and len(active) < args.jobs
                done, _ = wait(active, return_when=FIRST_COMPLETED,
                               timeout=ADMISSION_POLL_SECONDS if recheck_resources else None)
                for task in done:
                    index, game, settings = active.pop(task)
                    try:
                        result = task.result()
                    except (OSError, ValueError) as error:
                        result = {
                            "index": index, "game": game["root"],
                            "mode": "state" if "route" in settings else "frame",
                            "status": "case_setup_failed", "exit_code": None,
                            "seconds": 0, "captures": [], "reason": str(error),
                        }
                        write_json(output / f"{index:02}.json", result)
                    case_failed |= result["status"] in {
                        "ordinary_save_rejected", "case_setup_failed", "window_check_failed", "pixel_check_failed",
                        "event_check_failed", "save_check_failed",
                    }
                    if strict:
                        case_failed |= (result.get("exit_code") != 0 or result.get("first_engine_error") is not None
                                        or result["status"] not in {"visual_review_required", "frame_limit",
                                                                    "checkpoint_reached", "ending_verified"})
                    if ledger:
                        ledger.finish(game["root"], result, output / f"{index:02}.json")
                    print(f'{result["index"]:02}: {result["status"]}, {result["seconds"]}s', flush=True)
    return int(case_failed)


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


def postrun_ending_evidence(case_dir, index, ending):
    """Identify the artifacts reviewed after a fixed-frame ending run."""
    name = f"{index:02}"
    return {path.as_posix(): file_sha256(case_dir / path) for path in (
        Path(f"{name}.events.log"), Path(f"{name}.host.log"),
        Path(f"{name}.{ending['card_frame']:04}.png"),
        Path(f"{name}.{ending['clear_verified_frame']:04}.png"),
        Path(f"{name}.profile/savegen.dat"), Path(f"{name}.cold.events.log"),
        Path(f"{name}.cold.host.log"),
    )}


def find_cleared_cases(games, indices, cache_root):
    """Reuse completed first-ending receipts across candidates in the same test campaign."""
    pending = {index: (ROOT / games[index]["root"]).resolve() for index in indices}
    cleared = {}
    batches = cache_root.resolve().glob("[0-9][0-9][0-9][0-9]-[0-9][0-9]/*_playthrough")
    for batch in sorted(batches, reverse=True):
        if not pending:
            break
        for index, game_path in tuple(pending.items()):
            receipt_path = batch / f"{index:02}.json"
            try:
                receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
                replay_status = receipt.get("replay_status")
                if (receipt.get("status") != "ending_verified" or receipt.get("index") != index
                        or Path(receipt["game"]) != game_path or receipt.get("exit_code") != 0
                        or replay_status not in ("completed", "frame_limit") or receipt.get("timed_out")
                        or receipt.get("first_engine_error")):
                    continue
                ending = receipt["ending"]
                card_frame = ending["card_frame"]
                if (type(card_frame) is not int or card_frame < 0
                        or type(ending["clear_verified_frame"]) is not int
                        or not re.fullmatch(r"[A-Za-z0-9_]+", ending["id"])):
                    continue
                card = f"{index:02}.{card_frame:04}.png"
                case_dir = batch / f"{index:02}"
                if (ending["clear_verified_frame"] <= card_frame or ending["card"] != card
                        or not (case_dir / card).is_file()):
                    continue
                identity_path = batch / "build_identity.json"
                identity = json.loads(identity_path.read_text(encoding="utf-8"))
                if not all(is_sha256(identity.get(key)) for key in ("sha256", "source_tree_sha256")):
                    continue
                progress = ReplayProgress(state_mode=True)
                with (case_dir / f"{index:02}.events.log").open("rb") as events:
                    progress.sample(events)
                if progress.first_engine_error or progress.replay_status != replay_status:
                    continue
                if replay_status == "completed":
                    if not progress.verified_ending or any(
                        ending.get(key) != value for key, value in progress.verified_ending.items()
                    ):
                        continue
                elif (progress.frame != receipt.get("last_frame")
                      or progress.frame != ending["clear_verified_frame"]
                      or "postrun_evidence" not in ending
                      or ending["postrun_evidence"] != postrun_ending_evidence(case_dir, index, ending)):
                    continue
            except (OSError, ValueError, TypeError, AttributeError, KeyError):
                # A partial, outdated or removed receipt cannot exclude a game.
                continue
            cleared[index] = {"index": index, "game": str(game_path), "status": "skipped_cleared",
                              "ending": ending, "ending_receipt": str(receipt_path),
                              "ending_build_identity": str(identity_path)}
            del pending[index]
    return cleared


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


def playthrough_cache_root(option: Path | None = None, *, single_game: bool = False) -> Path:
    override = os.environ.get("CS2_BUILD_CACHE_ROOT")
    if option is not None:
        if not option.is_absolute():
            raise ValueError("--cache-root must be an absolute path")
        base = option
    elif override:
        base = Path(override)
        if not base.is_absolute():
            raise ValueError("CS2_BUILD_CACHE_ROOT must be an absolute path")
    elif single_game or os.name != "nt":
        return ROOT / "archive"
    elif Path("F:/").is_dir():
        base = Path("F:/CS2-build-cache")
    else:
        base = Path(os.environ.get("SystemDrive", "C:") + "/cs2-playthrough-cache")
    return base / "playthrough"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--player", type=Path, required=True)
    parser.add_argument("--cache-root", type=Path, help="absolute directory for the playthrough cache")
    game_input = parser.add_mutually_exclusive_group()
    game_input.add_argument("--game-root", type=Path,
                            help="run one case (index 0) directly from an existing game directory")
    game_input.add_argument("--inventory", type=Path, default=INVENTORY,
                            help="local JSON inventory containing a games array with roots in the workspace")
    parser.add_argument("--indices", nargs="+", type=int,
                        help="sample indices (default: all games except the V401 fixture in the workspace inventory)")
    parser.add_argument("--scenarios", type=Path, action="append", default=[],
                        help="JSON keyed by sample index; repeat for disjoint groups of selected games")
    parser.add_argument("--ordinary-saves", type=Path,
                        help="reviewed manifest; only scenarios with an approved ordinary_save ID preload a slot")
    parser.add_argument("--skip-cleared", action="store_true",
                        help="without a progress ledger, skip games with an archived verified ending")
    parser.add_argument("--fast-text", action="store_true",
                        help="seed the fastest native text speed in each isolated replay profile")
    parser.add_argument("--mute", action="store_true",
                        help="silence Windows audio output while retaining playback timing")
    parser.add_argument("--reuse-attempts", action="store_true",
                        help="reuse a completed frame run, checkpoint, or diagnosed stop for identical inputs")
    parser.add_argument("--frames", type=int, default=12)
    parser.add_argument("--button-probe", action="store_true",
                        help="record actual mouse targets at fixed-frame inspect checkpoints")
    parser.add_argument("--strict", action="store_true",
                        help="return a failing exit code for engine errors, missed inputs and incomplete runs")
    parser.add_argument("--jobs", type=int, default=2, help="worker cap, or 0 to derive it from available CPU cores")
    parser.add_argument("--progress", type=Path,
                        help="persist this campaign's checkpoints, blockers and verified clear results")
    parser.add_argument("--timeout", type=int, default=600)
    parser.add_argument("--stall-seconds", type=float, default=30,
                        help="timeout stall window in seconds (capped at half the timeout)")
    parser.add_argument("--ram-per-job-gib", type=float, default=2, help="RAM budget per replay (default 2 GiB)")
    parser.add_argument("--disk-per-job-gib", type=float, default=2, help="disk budget per replay (default 2 GiB)")
    parser.add_argument("--reserve-ram-gib", type=float, default=2, help="RAM headroom (default 2 GiB)")
    parser.add_argument("--reserve-disk-gib", type=float, default=20, help="disk headroom (default 20 GiB)")
    parser.add_argument("--admission-wait-seconds", type=int, default=0,
                        help="wait this long for resources before deferring cases (default immediate)")
    args = parser.parse_args()
    try:
        if args.game_root is None:
            games, default_indices = load_inventory(args.inventory)
        else:
            args.game_root = args.game_root.resolve(strict=True)
            if not args.game_root.is_dir():
                raise ValueError(f"Not a game directory: {args.game_root}")
            games, default_indices = [{"root": str(args.game_root)}], [0]
    except (OSError, ValueError, KeyError) as error:
        parser.error(str(error))
    indices = list(dict.fromkeys(args.indices)) if args.indices else default_indices
    if any(index < 0 or index >= len(games) for index in indices):
        parser.error("sample index outside the inventory")
    if min(args.frames, args.timeout) < 1 or args.jobs < 0:
        parser.error("frames and timeout must be positive and jobs nonnegative")
    if args.jobs == 0:
        cpu_count = getattr(os, "process_cpu_count", os.cpu_count)
        args.jobs = max(1, (cpu_count() or 1) // 2)
    if args.admission_wait_seconds < 0:
        parser.error("admission wait must be nonnegative")
    budgets = (args.stall_seconds, args.ram_per_job_gib, args.disk_per_job_gib,
               args.reserve_ram_gib, args.reserve_disk_gib)
    if any(not math.isfinite(value) or value <= 0 for value in budgets):
        parser.error("stall seconds and resource budgets must be finite and positive")
    now = datetime.now().astimezone()
    try:
        cache_root = playthrough_cache_root(args.cache_root, single_game=args.game_root is not None)
        with BatchProgress(args.progress) if args.progress else nullcontext() as ledger:
            if ledger:
                ledger.sync_inventory(games, default_indices)
            return run_selection(args, parser, games, indices, cache_root, now, ledger)
    except (OSError, ValueError) as error:
        parser.error(str(error))


def run_selection(args, parser, games, indices, cache_root, now, ledger):
    try:
        cleared = {}
        if ledger:
            for index in indices:
                if row := ledger.cleared(games[index]["root"]):
                    cleared[index] = {"index": index, "game": games[index]["root"], "status": "skipped_cleared",
                                      "ending": row["ending"], "ending_receipt": row["clear_evidence"]}
        pending = [index for index in indices if index not in cleared]
        if args.skip_cleared and ledger is None:
            cleared.update(find_cleared_cases(games, pending, cache_root))
            pending = [index for index in pending if index not in cleared]
        scenarios = load_scenarios(args.scenarios, pending)
        ordinary_saves = load_ordinary_saves(args.ordinary_saves) if args.ordinary_saves else {}
    except (OSError, ValueError) as error:
        parser.error(str(error))
    cases = []
    for index in pending:
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
        if "checkpoint_files" in settings:
            if "ordinary_save" in settings or "neko_language" in settings:
                parser.error(f"scenario {index} checkpoint_files require an isolated profile")
            settings = {**settings, "checkpoint_files": load_checkpoint_files(settings["checkpoint_files"])}
        if "save_checks" in settings:
            checks = settings["save_checks"]
            seeds = settings.get("checkpoint_files", {})
            if (not isinstance(checks, dict) or not checks
                    or any(not re.fullmatch(r"save[0-9]{4}\.dat", name)
                           or (name in seeds or "ordinary_save" in settings if seed is None
                               else not isinstance(seed, str) or seed not in seeds)
                           for name, seed in checks.items())):
                parser.error(f"scenario {index} save_checks require a seeded filename or null for an unseeded slot")
        if "window_checks" in settings:
            checks = settings["window_checks"]
            if ("route" in settings or not isinstance(checks, dict) or not checks
                    or any(not re.fullmatch(r"[1-9][0-9]*", frame) or int(frame) > frames
                           or not {f"inspect:{frame}", f"inspect-log:{frame}"}.intersection(inputs)
                           or not isinstance(names, list)
                           or any(not isinstance(name, str) or not re.fullmatch(r"[a-zA-Z0-9_.]+", name)
                                  for name in names)
                           for frame, names in checks.items())):
                parser.error(f"scenario {index} window_checks must map inspect frames to FES window names")
            settings = {**settings, "window_checks": {int(frame): names for frame, names in checks.items()}}
        if "event_checks" in settings:
            checks = settings["event_checks"]
            if ("route" in settings or not isinstance(checks, list) or not checks
                    or any(not isinstance(check, dict) or set(check) != {"from", "through", "contains"}
                           or type(check["from"]) is not int or type(check["through"]) is not int
                           or not 0 <= check["from"] <= check["through"] <= frames
                           or not isinstance(check["contains"], str) or not check["contains"].strip()
                           for check in checks)):
                parser.error(f"scenario {index} event_checks require frame ranges and nonempty event text")
        if "pixel_checks" in settings:
            checks = settings["pixel_checks"]
            if ("route" in settings or not isinstance(checks, list) or not checks
                    or any(not isinstance(check, dict) or set(check) != {"before", "after", "bounds", "expect"}
                           or type(check["before"]) is not int or type(check["after"]) is not int
                           or not 1 <= check["before"] < check["after"] <= frames
                           or any(not {f"inspect:{check[key]}", f"capture:{check[key]}"}.intersection(inputs)
                                  for key in ("before", "after"))
                           or not isinstance(check["bounds"], list) or len(check["bounds"]) != 4
                           or any(type(value) is not int or value < 0 for value in check["bounds"])
                           or check["bounds"][2] <= check["bounds"][0]
                           or check["bounds"][3] <= check["bounds"][1]
                           or check["expect"] not in ("same", "changed") for check in checks)):
                parser.error(f"scenario {index} pixel_checks require two capture frames, a rectangle and an expectation")
        if "defer" in settings and (not isinstance(settings["defer"], str) or not settings["defer"].strip()):
            parser.error(f"scenario {index} defer must give a reason")
        if "neko_language" in settings and (
            index != 1 or settings["neko_language"] not in ("jp", "cn")
            or "route" not in settings or "ordinary_save" in settings
        ):
            parser.error("neko_language requires a fresh state route for sample 01 and jp or cn")
        if args.button_probe:
            if "route" in settings:
                parser.error("--button-probe requires fixed-frame scenarios")
            settings = {**settings, "inputs": [*inputs, "button-probe"]}
        cases.append((index, games[index], settings))

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
    for index in indices:
        if index in cleared:
            write_json(output / f"{index:02}.json", cleared[index])
            if ledger and not ledger.cleared(games[index]["root"]):
                ledger.finish(games[index]["root"], cleared[index], output / f"{index:02}.json")
            print(f"{index:02}: skipped_cleared ({cleared[index]['ending']['id']})", flush=True)
    env = os.environ.copy()
    if args.mute:
        env["CS2_REPLAY_MUTE"] = "1"
    env.setdefault("CS2_MEDIA_CACHE", str(cache_root / "movie-input"))
    env.setdefault("RUST_BACKTRACE", "full")
    if os.name == "nt":
        env["PATH"] = os.pathsep.join(("C:/msys64/mingw64/bin", env.get("PATH", "")))
    reuse_attempts = (source["sha256"], load_attempt_history(cache_root, pending)) if args.reuse_attempts else None
    if ledger:
        cases = ledger.prioritize_blockers(cases)
    return run_batch(cases, player, output, args, env, ordinary_saves, fast_text=args.fast_text,
                     reuse_attempts=reuse_attempts, game_root_override=args.game_root, ledger=ledger,
                     strict=args.strict)


if __name__ == "__main__":
    raise SystemExit(main())

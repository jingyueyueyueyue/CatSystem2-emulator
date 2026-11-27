"""Replay individually observed game buttons in isolated Windows profiles."""

import argparse
from concurrent.futures import FIRST_COMPLETED, ThreadPoolExecutor, wait
from dataclasses import dataclass
from datetime import datetime
import json
import os
from pathlib import Path
import re
import shutil
import time

from run_playthrough_batch import (
    GIB, INVENTORY, REPLAY_EVENT_LINE, available_ram_bytes, identify_player, load_inventory,
    new_output_directory, playthrough_cache_root, run_case, write_json,
)

TITLE_SCENARIOS = Path(__file__).with_name("pc_button_titles.json")
REGION = re.compile(
    r"frame (\d+): button region=(\d+) bounds=\[(-?\d+), (-?\d+), (-?\d+), (-?\d+)\]"
    r" enabled=true point=Some\(\[(\d+), (\d+)\]\)"
)
REGION_STATE = re.compile(
    r"frame (\d+): button region=(\d+) bounds=\[(-?\d+), (-?\d+), (-?\d+), (-?\d+)\]"
    r" enabled=(true|false) point=.*"
)
INSPECTED_FRAME = re.compile(r"frame (\d+): wait ")
MENU = re.compile(r"frame (\d+): menu installed HostGameMenu.*visible: true")
INPUT = re.compile(r"(?:move:|right:|require-hit:)?(\d+):(?:-?\d+,-?\d+|\d+)$")
MOUSE_PRESS = re.compile(
    r"pressed: 1, repeated: false, region: (\d+), receiver: (\d+), response: (\d+), "
    r"physical: \[RoutedPhysicalButton \{ source: Mouse, code: 1 \}\]"
)
FRAMED_MOUSE_PRESS = re.compile(
    r"InputRouteReceipt \{ frame: (\d+), key: \d+, group: \d+, raw_mask: \d+, " + MOUSE_PRESS.pattern
)
POINTER_ROUTES = re.compile(r"frame (\d+): pointer \[")
NORMAL_EXIT = re.compile(r"frame (\d+): game requested normal exit")
LIVE_BUTTON = re.compile(r"frame (\d+): live button before=(\d+) region=(\d+)")
CASE_NAME = re.compile(r"[a-z0-9]+(?:-[a-z0-9]+)*")


@dataclass(frozen=True)
class ButtonCase:
    name: str
    before: int | None
    click: int | None
    region: int | None
    flags: list[str]
    expectation: str = "observed"
    after: int | None = None
    live_frames: int | None = None
    response: int = 0
    region_change: tuple[int, bool | tuple | None, bool | tuple | None] | None = None
    message_point: tuple[int, int] | None = None


def entry_inputs(source, frame):
    return ["story-events", "capture-every:0", "button-probe", *[
        flag for flag in source["inputs"]
        if (action := INPUT.fullmatch(flag)) and int(action[1]) < frame
    ]]


def observed_cases(index, baseline, source):
    host = baseline / f"{index:02}.host.log"
    if not host.exists():
        return
    regions = {}
    commands = {}
    for line in host.read_text(encoding="utf-8", errors="replace").splitlines():
        if hit := REGION.fullmatch(line):
            frame, region, left, top, right, bottom, x, y = map(int, hit.groups())
            regions.setdefault(frame, []).append((region, (left, top, right, bottom), x, y))
        elif menu := MENU.match(line):
            for command in (40027, 40057):
                if re.search(rf"command: {command}, label: .*?, enabled: true,", line):
                    commands[command] = int(menu[1])

    tried = set()
    for frame, candidates in sorted(regions.items()):
        capture = baseline / f"{index:02}.{frame:04}.png"
        if not capture.is_file():
            continue
        with capture.open("rb") as image:
            header = image.read(24)
        if header[:8] != b"\x89PNG\r\n\x1a\n" or len(header) != 24:
            raise ValueError(f"missing PNG image dimensions in {capture}")
        width = int.from_bytes(header[16:20], "big")
        height = int.from_bytes(header[20:24], "big")
        for region, bounds, x, y in candidates:
            if (region, bounds) in tried or bounds == (0, 0, width, height):
                continue
            tried.add((region, bounds))
            click = frame + 3
            flags = [*entry_inputs(source, frame), "settings-apply", f"move:{click - 1}:{x},{y}",
                     f"require-hit:{click}:{region}", f"{click}:{x},{y}"]
            yield ButtonCase(f"region-{frame}-{region}", frame, click, region, flags)

    for command, installed in sorted(commands.items()):
        click = installed + 30
        flags = [*entry_inputs(source, installed), "settings-apply",
                 f"menu:{click}:{command}", f"menu:{click + 75}:{command}"]
        yield ButtonCase(f"menu-{command}", installed + 1, click, None, flags)


def named_cases(path):
    source = json.loads(path.read_text(encoding="utf-8"))
    cases = {}
    for index, entries in source.items():
        game_cases = {}
        for name, fields in entries.items():
            region = fields["region"]
            expectation = fields.get("expectation", "action")
            flags = fields["inputs"]
            response = fields.get("response", 0)
            if (not CASE_NAME.fullmatch(name) or type(region) is not int or region <= 0
                    or expectation not in {"action", "disabled", "exit"}
                    or type(response) is not int or response not in {0, 1}
                    or (response != 0 and expectation != "action")):
                raise ValueError(f"invalid button case {index}/{name}")
            before, click, after = (fields.get(key) for key in ("before", "click", "after"))
            live_frames = fields.get("live_frames")
            if live_frames is None:
                if (not all(type(frame) is int for frame in (before, click, after))
                        or not 0 < before < click < after
                        or not any(re.fullmatch(rf"{click}:-?\d+,-?\d+", flag) for flag in flags)):
                    raise ValueError(f"invalid fixed button case {index}/{name}")
            elif (type(live_frames) is not int or live_frames <= 0 or expectation == "disabled"
                    or any(frame is not None for frame in (before, click, after))):
                raise ValueError(f"invalid live button case {index}/{name}")
            accept_message = fields.get("accept_message_resume", False)
            if type(accept_message) is not bool:
                raise ValueError(f"invalid accept_message_resume flag for {index}/{name}")
            message_point = None
            if accept_message:
                clicks = [match.groups() for flag in flags
                          if (match := re.fullmatch(rf"{click}:(-?\d+),(-?\d+)", flag))]
                if (expectation == "disabled" or response != 0 or len(clicks) != 1
                        or f"require-hit:{click}:{region}" not in flags):
                    raise ValueError(f"invalid guarded legacy click {index}/{name}")
                x, y = clicks[0]
                message_point = (int(x), int(y))
            region_change = fields.get("region_change")
            if region_change is not None:
                if (expectation != "action" or live_frames is not None
                        or f"require-hit:{click}:{region}" not in flags
                        or not isinstance(region_change, list) or len(region_change) != 3):
                    raise ValueError(f"invalid region change case {index}/{name}")
                watched, prior, following = region_change
                if (type(watched) is not int or watched <= 0 or prior == following
                        or (prior is not None and following is not None and type(prior) is not type(following))
                        or any(state is not None and type(state) is not bool and (
                            not isinstance(state, list) or len(state) != 4
                            or any(type(coordinate) is not int for coordinate in state)
                        ) for state in (prior, following))):
                    raise ValueError(f"invalid region states for {index}/{name}")
                region_change = (watched, *(tuple(state) if isinstance(state, list) else state
                                            for state in (prior, following)))
            game_cases[name] = ButtonCase(
                name, before, click, region,
                ["story-events", "capture-every:0", "button-probe", *flags],
                expectation, after, live_frames, response, region_change, message_point,
            )
        cases[int(index)] = list(game_cases.values())
    return cases


def message_resume_frame(path, click, after, point):
    x, y = point
    expected = f"message-resume message=[2, {x}, {y}, 0]"
    with Path(path).open(encoding="utf-8", errors="replace") as stream:
        for line in stream:
            if match := REPLAY_EVENT_LINE.match(line):
                frame, event = match.groups()
                if click < int(frame) <= after and event == expected:
                    return int(frame)
    return None


def evaluate_button(result, spec):
    name, before, click, region = spec.name, spec.before, spec.click, spec.region
    index = result["index"]
    log = Path(result["stderr"]).read_text(encoding="utf-8", errors="replace").splitlines()
    if spec.live_frames is not None:
        found = [(int(hit[1]), int(hit[2])) for line in log if (hit := LIVE_BUTTON.fullmatch(line))
                 and int(hit[3]) == region]
        if len(found) != 1:
            result.update(case=name, expected_region=region, expectation=spec.expectation)
            if result["status"] in {"frame_limit", "visual_review_required"}:
                result["status"] = "live_button_missing"
            return result
        click, before = found[0]
    # The replay releases three ticks after key-down. FES may consume the
    # native input on any tick before the release is processed.
    last_click_frame = click + 4
    receipts = [line for line in log if (hit := POINTER_ROUTES.match(line))
                and click < int(hit[1]) <= last_click_frame]
    settings = [line for line in log if "settings " in line and line.startswith("frame ")]
    presses = [
        {"region": int(hit[2]), "receiver": int(hit[3]), "response": int(hit[4])}
        for line in receipts for hit in FRAMED_MOUSE_PRESS.finditer(line)
        if click < int(hit[1]) <= last_click_frame
    ]
    exit_frame = next((int(match[1]) for line in log if (match := NORMAL_EXIT.fullmatch(line))), None)
    before_name = f"{index:02}.{before:04}.png"
    result.update(
        case=name, click_frame=click, expected_region=region, expectation=spec.expectation,
        receipts=receipts,
        mouse_presses=presses, settings=settings, normal_exit_frame=exit_frame,
        before_capture=before_name if before_name in result["captures"] else None,
        after_captures=[capture for capture in result["captures"]
                        if int(Path(capture).stem.split(".")[-1]) > click],
    )
    message_frame = None
    if spec.message_point is not None:
        message_frame = message_resume_frame(result["stdout"], click, spec.after, spec.message_point)
        result["message_resume_frame"] = message_frame
    consumed = message_frame is not None or any(
        press["region"] == region and press["response"] == 0 for press in presses
    )
    if spec.expectation == "action":
        result["expected_response"] = spec.response
    if result["status"] == "unexpected_exit" and exit_frame is not None and exit_frame >= click:
        if spec.expectation == "exit":
            result["status"] = ("normal_exit" if consumed and result["before_capture"] and result["after_captures"]
                                else "exit_receipt_missing")
        elif spec.expectation == "observed":
            result["status"] = "exit_review_required"
    elif result["status"] == "visual_review_required":
        if not result["before_capture"] or not result["after_captures"]:
            result["status"] = "button_capture_missing"
        elif spec.expectation == "exit":
            result["status"] = "expected_exit_missing"
        elif spec.region_change is not None:
            watched, prior, following = spec.region_change
            frames = {before, spec.after}
            inspected = {int(match[1]) for line in log if (match := INSPECTED_FRAME.match(line))}
            states = {}
            for line in log:
                hit = REGION_STATE.fullmatch(line)
                if hit is None or int(hit[2]) != watched or (frame := int(hit[1])) not in frames:
                    continue
                states[frame] = (hit[7] == "true" if type(prior if prior is not None else following) is bool
                                 else tuple(int(hit[index]) for index in range(3, 7)))
            actual = [states.get(before), states.get(spec.after)]
            result["region_change"] = {"region": watched, "expected": [prior, following], "actual": actual}
            if not frames <= inspected:
                result["status"] = "region_snapshot_missing"
            else:
                result["status"] = ("region_change_verified" if actual == [prior, following]
                                    else "region_change_missing")
        elif spec.expectation == "disabled":
            prefix = f"frame {before}: button region={region} "
            disabled = any(line.startswith(prefix) and line.endswith(" enabled=false point=None")
                           for line in log)
            if not disabled:
                result["status"] = "disabled_region_missing"
            elif not presses:
                result["status"] = "disabled_input_receipt_missing"
            elif any(press["response"] == 0 for press in presses):
                result["status"] = "disabled_input_consumed"
            else:
                result["status"] = "disabled_review_required"
        elif region is None:
            result["status"] = "settings_roundtrip" if any(
                "settings reopened resource=" in line for line in settings
            ) else "settings_review_required"
        elif message_frame is not None or any(press["region"] == region and (
            spec.expectation == "observed" or press["response"] == spec.response
        ) for press in presses):
            result["status"] = "action_review_required"
        else:
            result["status"] = "button_receipt_missing"
    return result


def run_button(index, game, spec, player, output, timeout, env):
    directory = output / f"{index:02}-{spec.name}"
    directory.mkdir()
    try:
        if spec.live_frames is None:
            after = spec.after or spec.click + 105
            before = "inspect" if spec.expectation == "disabled" or spec.region_change else "capture"
            frames = after + 46
            flags = [*spec.flags, "no-final-capture", f"{before}:{spec.before}", f"inspect:{after}"]
        else:
            frames = spec.live_frames
            flags = [*spec.flags, "no-final-capture"]
        result = run_case(index, game, frames, flags, player, directory, timeout, env)
        result = evaluate_button(result, spec)
    except (OSError, ValueError) as error:
        result = {"index": index, "case": spec.name, "status": "case_setup_failed", "detail": str(error)}
    write_json(directory / "button.json", result)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--player", type=Path, required=True)
    parser.add_argument("--cache-root", type=Path, help="absolute directory for the playthrough cache")
    parser.add_argument("--probes", type=Path)
    parser.add_argument("--scenarios", type=Path, default=TITLE_SCENARIOS)
    parser.add_argument("--cases", type=Path, action="append", default=[],
                        help="named button cases with explicit action expectations (repeatable)")
    parser.add_argument("--indices", type=int, nargs="+")
    parser.add_argument("--jobs", type=int, default=3)
    parser.add_argument("--timeout", type=int, default=300)
    parser.add_argument("--reserve-ram-gib", type=float, default=2)
    parser.add_argument("--ram-per-job-gib", type=float, default=2)
    parser.add_argument("--admission-wait-seconds", type=int, default=120)
    args = parser.parse_args()
    if min(args.jobs, args.timeout) < 1:
        parser.error("jobs and timeout must be positive")
    if min(args.reserve_ram_gib, args.ram_per_job_gib, args.admission_wait_seconds) <= 0:
        parser.error("resource limits must be positive")
    if args.probes is None and not args.cases:
        parser.error("provide --probes, --cases or both")

    player = args.player.resolve(strict=True)
    identity = identify_player(player)
    sources = {}
    if args.probes is not None:
        recorded = json.loads((args.probes / "build_identity.json").read_text(encoding="utf-8"))
        if identity["sha256"] != recorded["sha256"]:
            parser.error("the button probe and click replay must use the same executable")
        sources = json.loads(args.scenarios.read_text(encoding="utf-8"))
    games, defaults = load_inventory(INVENTORY)
    named = {}
    for case_file in args.cases:
        for index, cases in named_cases(case_file).items():
            named.setdefault(index, []).extend(cases)
    default_indices = dict.fromkeys([*defaults, *named]) if args.probes is not None else named
    indices = list(dict.fromkeys(args.indices)) if args.indices else list(default_indices)
    if any(index < 0 or index >= len(games) or (str(index) not in sources and index not in named)
           for index in indices):
        parser.error("select an index with inventory and scenario entries")

    pending = []
    counts = {}
    for index in indices:
        cases = list(named.get(index, []))
        if str(index) in sources:
            source = sources[str(index)]
            if source.get("button_replay") == "named":
                if not cases:
                    parser.error(f"index {index} needs its named button cases")
            else:
                cases.extend(observed_cases(index, args.probes, source))
        if len({case.name for case in cases}) != len(cases):
            parser.error(f"duplicate button case for index {index}")
        counts[str(index)] = len(cases)
        pending.extend((index, games[index], case) for case in cases)
    try:
        cache = playthrough_cache_root(args.cache_root)
    except ValueError as error:
        parser.error(str(error))
    output = new_output_directory(cache, datetime.now().astimezone())
    write_json(output / "button_inventory.json", {
        "counts": counts, "probes": str(args.probes) if args.probes else None,
        "cases": [str(path) for path in args.cases], "build": identity,
    })
    print(output, flush=True)
    env = os.environ.copy()
    env.setdefault("CS2_MEDIA_CACHE", str(cache / "movie-input"))
    if os.name == "nt":
        env["PATH"] = os.pathsep.join(("C:/msys64/mingw64/bin", env.get("PATH", "")))
    active = set()
    results = []
    pending.reverse()
    deadline = None
    with ThreadPoolExecutor(max_workers=args.jobs) as pool:
        while pending or active:
            needed_ram = (args.reserve_ram_gib + args.ram_per_job_gib * (1 + len(active))) * GIB
            needed_disk = (22 + 2 * len(active)) * GIB
            if (pending and len(active) < args.jobs and available_ram_bytes() >= needed_ram
                    and shutil.disk_usage(output).free >= needed_disk):
                index, game, spec = pending.pop()
                active.add(pool.submit(run_button, index, game, spec, player, output, args.timeout, env))
                deadline = None
                continue
            if not active:
                deadline = deadline if deadline is not None else time.monotonic() + args.admission_wait_seconds
                if time.monotonic() >= deadline:
                    results.extend({"index": index, "case": spec.name, "status": "resource_deferred"}
                                   for index, _, spec in reversed(pending))
                    break
                time.sleep(5)
                continue
            done, active = wait(active, timeout=5, return_when=FIRST_COMPLETED)
            for future in done:
                result = future.result()
                results.append(result)
                print(f'{result["index"]:02} {result["case"]} {result["status"]}', flush=True)
    write_json(output / "button_results.json", results)
    reviewed_or_pending = {
        "action_review_required", "exit_review_required", "settings_roundtrip",
        "settings_review_required", "disabled_review_required", "normal_exit", "region_change_verified",
    }
    return int(any(count == 0 for count in counts.values())
               or any(item["status"] not in reviewed_or_pending for item in results))


if __name__ == "__main__":
    raise SystemExit(main())

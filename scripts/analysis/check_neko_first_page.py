"""Check a NEKOPARA Vol.3 first-page trace without launching the player."""

import argparse
import json
import re
from pathlib import Path


REVEAL = re.compile(r"^message-reveal (\w+) (.*)$")
FIELD = re.compile(r"\b([a-z_]+)=(\S+)")
INPUT = re.compile(r"^frame \d+: down ")
INSPECT = re.compile(r"^frame (\d+):$")
LAYER = re.compile(r'^layer (\d+) "(\\0text:737472\[0\]|keywait)" .*? opacity=(\d+) .* nonzero_alpha=(\d+)$')
STORY = re.compile(r"^story frame=(\d+) state=(\S+) scene=(\S+) group=(-?\d+) record=(-?\d+)$")
WAIT = re.compile(r"^frame=(\d+) input-wait native 71 address=0xd6d$")


def reveal(line):
    match = REVEAL.match(line)
    return (match[1], dict(FIELD.findall(match[2]))) if match else (None, {})


def measure_first_page(lines):
    for index in range(1, len(lines)):
        kind, scan = reveal(lines[index])
        if kind != "scan" or (scan["limit"], scan["end"]) != ("1", "38"):
            continue
        kind, start = reveal(lines[index - 1])
        if (
            kind == "start" and (start["display"], start["playback"]) == ("Visible", "0")
            and (start["id"], start["gen"]) == (scan["id"], scan["gen"])
        ):
            break
    else:
        return None

    task_start = last_task = current_task = int(start["task"])
    playback = delta_sum = ticks = glyphs = pixels = paints = 0
    pixel_times = set()
    clock_matches = True
    scan = None
    completion = None
    stop = "end_of_log"
    for offset, line in enumerate(lines[index:], index + 1):
        if INPUT.match(line):
            stop = "input_before_completion"
            break
        kind, fields = reveal(line)
        if fields.get("id") != start["id"]:
            continue
        if fields["gen"] != start["gen"]:
            stop = "text_generation_changed"
            break
        if kind in ("start", "tick"):
            current_task = int(fields["task"])
        if kind == "tick":
            delta = int(fields["delta"])
            observed_playback = int(fields["playback"])
            clock_matches &= current_task - last_task == delta >= 0 and observed_playback == playback
            ticks += 1
            delta_sum += delta
            playback = observed_playback + delta
            last_task = current_task
        elif kind == "scan":
            if fields["force"] != "false" or (fields["limit"], fields["end"]) != ("1", "38"):
                stop = "forced_or_different_range"
                break
            scan = fields
            if fields["first_pending"] == "None" and fields["separator"] == "false" and glyphs == 38:
                completion = {"host_line": offset, "task": current_task, "playback": int(fields["playback"])}
                stop = "natural_completion"
                break
        elif kind == "paint" and scan:
            if fields["failure"] != "None":
                stop = "paint_failure"
                break
            glyphs += int(fields["completed"])
            submitted = fields["copy"] == "Full" or fields["copy"] == "Dirty" and fields["copied_dirty"] == "true"
            change = re.fullmatch(r"Some\((\d+)\)", fields["changed"])
            if submitted and change and int(change[1]) > 0:
                paints += 1
                pixels += int(change[1])
                pixel_times.add(int(scan["playback"]))

    return {
        "id": int(start["id"]), "generation": int(start["gen"]),
        "task_start": task_start, "task_last": last_task, "task_span": last_task - task_start,
        "ticks": ticks, "delta_sum": delta_sum, "playback_after_last_tick": playback,
        "clock_matches": clock_matches and delta_sum == last_task - task_start == playback,
        "glyphs": glyphs, "positive_pixel_submissions": paints, "changed_pixels_sum": pixels,
        "post_first_pixel_times": len(pixel_times - {0}), "completion": completion, "stop": stop,
    }


def first_page_prompt(host_lines, event_lines, measurement):
    story = [(int(match[1]), match.groups()[1:]) for line in event_lines if (match := STORY.match(line))]
    waits = {int(match[1]) for line in event_lines if (match := WAIT.match(line))}
    target = str(measurement["id"])
    generation = str(measurement["generation"])
    frame = None
    layers = {}
    for line in host_lines[measurement["completion"]["host_line"]:]:
        if INPUT.match(line):
            break
        kind, fields = reveal(line)
        if fields.get("id") == target:
            if fields["gen"] != generation or (kind == "start" and fields["display"] != "Visible"):
                break
            if kind == "scan" and (fields["limit"], fields["end"], fields["force"], fields["first_pending"]) != (
                "1", "38", "false", "None",
            ):
                break
        if match := INSPECT.match(line):
            frame = int(match[1])
            layers.clear()
        elif frame in waits and (match := LAYER.match(line)) and int(match[3]) and int(match[4]):
            owner, name = match.groups()[:2]
            layers.setdefault(owner, {})[name] = int(match[4])
            if len(layers[owner]) == 2 and next(
                (state for observed, state in reversed(story) if observed <= frame), None
            ) == ("active", "01", "0", "59"):
                return {"frame": frame, "text_alpha_pixels": layers[owner][r"\0text:737472[0]"],
                        "keywait_alpha_pixels": layers[owner]["keywait"]}
    return None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("run", type=Path, help="playthrough directory with 01.host.log and 01.events.log")
    args = parser.parse_args()
    host = (args.run / "01.host.log").read_text(encoding="utf-8", errors="replace").splitlines()
    events = (args.run / "01.events.log").read_text(encoding="utf-8", errors="replace").splitlines()
    measurement = measure_first_page(host)
    complete = measurement and measurement["completion"]
    passed = bool(complete and measurement["clock_matches"] and measurement["task_span"] <= 240
                  and measurement["post_first_pixel_times"] >= 2)
    prompt = first_page_prompt(host, events, measurement) if passed else None
    print(json.dumps({"first_page_passed": passed, "measurement": measurement,
                      "first_page_prompt": prompt}, indent=2))
    return 0 if passed else 1


if __name__ == "__main__":
    raise SystemExit(main())

"""Verify Labyrinth's opening waits from replay metadata."""

import argparse
import json
from pathlib import Path
import re
from types import SimpleNamespace

from check_pc_body_successor import DOWN, ENTER, RECEIPT, WAIT
from check_pc_story_11_13_successor import read_run, verify_task_lifetime
from check_pc_story_11_13_sources import require


PAGE = re.compile(
    r'labyrinth15=Some\("owner=(\d+) context=(\d+) loaded=(\d+) '
    r'wait=Some\(\((\d+), (\d+), (\d+)\)\) local100=Some\(1\) '
    r'generation=Some\(StoryBodyGeneration \{ source_epoch: ([1-9]\d*), source_frame: (\d+), '
    r'natural_complete: (?:None|Some\(\(\7, (\d+)\)\)), committed_epoch: Some\(\7\), '
    r'remaining: 0, remaining_segment: 0, pending_text: false, drawing: false, skip_pending: false, '
    r'forced: false, projection: Some\(\(true, [1-9]\d*\)\) \}\) '
    r'sources=Some\(\[CstBodySource \{ context_handle: \2, load_id: ([1-9]\d*), group: \5, '
    r'absolute_record: (\d+) \}\]\) layout_complete=(?:None|Some\(\(\7, (\d+)\)\)) '
    r'committed=true composited=true enter=Some\(\(([1-9]\d*), ([1-9]\d*)\)\)"\)'
)
OP_RELEASE = re.compile(r"frame=(\d+) native-call index=268 .* movie_naturally_released handle=(\d+)")
OP_LOAD = re.compile(r'frame=(\d+) native-call index=265 .* movie_load handle=(\d+) resource="movie.int/opmovie.mpg"')
SCENE_LOAD = re.compile(r'frame=(\d+) native-call .* debug="sscript loaded > yum_h01"')
ROOT_RESUME = re.compile(r"frame=(\d+) message-resume message=\[2, 0, 0, 0\]")
RELEASE = re.compile(r"frame (\d+): 11/13 Enter released after frame (\d+)")
R19_RELEASE = re.compile(r"labyrinth15-r19 release frame=(\d+)")
R38_POSITION = re.compile(r"story frame=(\d+) state=active scene=yum_h01 group=7 record=38")
CROSS_WINDOW = {7: (38, 37), 8: (41, 40)}


def parse_page(line):
    row = WAIT.fullmatch(line)
    if not row or not (match := PAGE.search(row[2])):
        return None
    frame = int(row[1])
    owner, context, loaded, marked, group, wait, epoch, source_frame, natural, load, body, layout, region, receiver = (
        int(value) if value is not None else None for value in match.groups()
    )
    expected = (7 + 2 * group, 6 + 2 * group) if 0 <= group <= 6 else CROSS_WINDOW.get(group)
    if (wait, body) != expected:
        return None
    completed = (natural or layout) if group == 0 else natural
    if completed is None or not (loaded <= source_frame <= completed <= frame and loaded <= marked < frame):
        return None
    if f"scene=yum_h01 group={group} record={wait} " not in row[2] or (
        "message_wait=Some(2564) vm_waiting_for_message=true pending_messages=0 movies=0 " not in row[2]
    ):
        return None
    return {"frame": frame, "owner": owner, "context": context, "loaded": loaded, "marked": marked,
            "group": group, "wait": wait, "epoch": epoch, "source_frame": source_frame,
            "complete_frame": completed, "load": load, "body": body, "region": region, "receiver": receiver}


def verify(host, events, opening_only, continuation=False, after_r19=False, cross_window=False):
    presses = [(int(match[1]), match[2]) for line in host if (match := DOWN.fullmatch(line))]
    enter_count = 0 if opening_only else (8 if cross_window else 7 if after_r19 else 6 if continuation else 1)
    if cross_window and [line for line in events if line.startswith("replay_status=")] == ["replay_status=frame_limit"]:
        require(sum(button == "Keyboard(13)" for _, button in presses) == 7,
                "cross-window successor did not finish at r41")
        return verify(host, events, False, after_r19=True)
    expected = ["MouseButton(0)", "MouseButton(0)"] + ["Keyboard(13)"] * enter_count
    require([button for _, button in presses] == expected and [at for at, _ in presses[:2]] == [692, 821],
            "unexpected input sequence")
    pages_by_frame = {}
    for line in host:
        if page := parse_page(line):
            require(pages_by_frame.setdefault(page["frame"], page) == page,
                    "conflicting body observations for one frame")
    pages = list(pages_by_frame.values())
    require(pages and pages[0]["wait"] == 7, "opening wait lacks a committed live body")
    first = pages[-1] if opening_only else pages[0]
    require(any(int(match[1]) == first["loaded"] for line in events
                if (match := SCENE_LOAD.fullmatch(line))), "missing yum_h01 load")
    releases = [(int(match[1]), int(match[2])) for line in events
                if (match := OP_RELEASE.fullmatch(line)) and 821 < int(match[1]) < first["loaded"]]
    require(len(releases) == 1, "missing unique natural OP release")
    released, handle = releases[0]
    loads = [(int(match[1]), int(match[2])) for line in events if (match := OP_LOAD.fullmatch(line))]
    require(len(loads) == 1 and loads[0][1] == handle and 821 < loads[0][0] < released,
            "natural release does not match the opened OP")
    result = {"status": "opening_wait_verified", "opening": first,
              "op_release_frame": released, "op_handle": handle}
    if opening_only:
        require(all(page["wait"] == 7 for page in pages), "unexpected successor in opening observation")
        return result

    gates = [frame for frame, _ in presses[2:]]
    if after_r19:
        pages = [page for page in pages if page["frame"] <= gates[-1]]
    if cross_window:
        pages = [page for page in pages if page["frame"] in gates or page["group"] == 8]
    guarded = pages if after_r19 else pages[:-1]
    require([page["group"] for page in pages] == list(range(len(gates) + (not after_r19)))
            and [page["frame"] for page in guarded] == gates,
            "missing ordered guarded body waits")
    identity = ("owner", "context", "load", "loaded", "region", "receiver")
    for before, after in zip(pages, pages[1:]):
        changed_window = cross_window and after["group"] == 7
        stable = ("context", "load", "loaded") if changed_window else identity
        current_epoch = (changed_window and before["owner"] != after["owner"]) or after["epoch"] > before["epoch"]
        require(all(before[key] == after[key] for key in stable) and current_epoch
                and min(after["marked"], after["source_frame"]) > before["frame"],
                f"stale body or changed receiver at r{after['wait']}")
    verify_task_lifetime(events, SimpleNamespace(**first), pages[6 if cross_window else -1]["frame"])
    if cross_window:
        verify_task_lifetime(events, SimpleNamespace(**pages[7]), pages[8]["frame"])
    receipts = [match.groups() for line in host for match in RECEIPT.finditer(line)
                if int(match[1]) > gates[0] and int(match[5]) and match[9] == "0"]
    resumes = [gate + 1 for gate in gates]
    require(receipts == [(str(frame), "2", "0", "1", "1", "false",
                          str(page["region"]), str(page["receiver"]), "0", ENTER)
                         for page, frame in zip(guarded, resumes, strict=True)],
            "Enter lacked a single consuming receiver")
    require([int(match[1]) for line in events if (match := ROOT_RESUME.fullmatch(line))
             and int(match[1]) > gates[0]] == resumes
            and all(f"frame={frame} input-wait native 71 address=0xa04" in events for frame in resumes),
            "Enter lacked matching root VM wait and resume")
    status = "frame_limit" if after_r19 else "completed"
    require([line for line in events if line.startswith("replay_status=")] == [f"replay_status={status}"],
            "unexpected replay status")
    if continuation or after_r19 or cross_window:
        releases = [tuple(map(int, match.groups())) for line in host if (match := RELEASE.fullmatch(line))]
        require(releases == [(page["frame"], gate) for page, gate in zip(pages[1:], gates)]
                and all(frame > gate + 3 for frame, gate in releases),
                "missing scanned release between adjacent body waits")
        if after_r19:
            return dict(result, pages=pages, enter_frames=resumes,
                        **verify_r19_observation(host, events, gates[-1]))
        if cross_window:
            transition = verify_r19_observation(host, events, gates[6], gates[7])
            require(transition["status"] == "r38_position_observed", "missing r38 transition")
            return dict(result, status="cross_window_successor_verified", pages=pages,
                        enter_frames=resumes, transition=transition)
        return dict(result, status="continuation_verified", pages=pages, enter_frames=resumes)
    return dict(result, status="successor_verified", successor=pages[-1], enter_frame=resumes[0])


def verify_r19_observation(host, events, gate, next_gate=None):
    releases = [int(match[1]) for line in events if (match := R19_RELEASE.fullmatch(line))]
    require(len(releases) == 1 and releases[0] > gate + 3, "missing scanned r19 Enter release")
    observed = [int(match[1]) for line in host if (match := WAIT.fullmatch(line))
                and int(match[1]) > releases[0]]
    require(observed, "missing post-r19 frame inspections")
    position = [int(match[1]) for line in events if (match := R38_POSITION.fullmatch(line))
                and int(match[1]) > gate and (next_gate is None or int(match[1]) < next_gate)]
    if not position:
        return {"status": "r19_enter_verified_r38_pending", "release_frame": releases[0],
                "inspected_frames": observed}
    recognized = [("wipe wipe18 120", 0), ("wait 30", 30), ("rdraw 120", 120), ("view 1 meswnd_h", 0)]
    frame = gate
    for command, delay in recognized:
        hits = [int(line.split(" ", 1)[0][6:]) for line in events if line.startswith("frame=")
                and line.endswith(f'debug="[command] {command}"') and int(line.split(" ", 1)[0][6:]) >= frame
                and int(line.split(" ", 1)[0][6:]) <= position[0]]
        require(hits, f"missing ordered {command} after r19")
        frame = hits[0] + delay
    require(any(at > position[0] for at in observed), "missing inspection after the r38 marker")
    return {"status": "r38_position_observed", "r38_frame": position[0],
            "release_frame": releases[0], "inspected_frames": observed}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("run", type=Path)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--opening-only", action="store_true")
    mode.add_argument("--continuation", action="store_true", help="validate seven live waits through r19")
    mode.add_argument("--after-r19", action="store_true", help="validate one further Enter and observe r38")
    mode.add_argument("--cross-window", action="store_true", help="validate r19 through the live r41 wait")
    args = parser.parse_args()
    try:
        _, host, events = read_run(args.run, "15")
        print(json.dumps(verify(host, events, args.opening_only, args.continuation, args.after_r19,
                                args.cross_window), indent=2))
    except (OSError, ValueError, KeyError) as error:
        print(json.dumps({"status": "unverified", "reason": str(error)}))
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

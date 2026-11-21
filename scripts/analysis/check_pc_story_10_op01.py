"""Check the Djibril startup and OP01 live wait using replay metadata."""

import argparse
import json
from pathlib import Path
import re
from types import SimpleNamespace

from check_pc_body_successor import DOWN, ENTER, RECEIPT, WAIT
from check_pc_story_11_13_successor import BODY_SOURCE, read_run, source_reads, verify_task_lifetime
from check_pc_story_11_13_sources import require


SCENE_LOAD = re.compile(r'frame=(\d+) native-call .* debug="sscript loaded > (start|OP01)"', re.IGNORECASE)
LIVE_WAIT = re.compile(
    r'djibril10=Some\("owner=(?P<owner>[1-9]\d*) context=(?P<context>[1-9]\d*) '
    r'loaded=(?P<loaded>\d+) wait=Some\(\((?P<marked>\d+), (?P<group>\d+), (?P<wait>\d+)\)\) .*? '
    r'root=true poll=true wait_key=(?:true|false) local100=Some\(1\) fes=true sscript=true '
    r'generation=Some\(StoryBodyGeneration \{ source_epoch: (?P<epoch>[1-9]\d*), '
    r'source_frame: (?P<source>\d+), natural_complete: Some\(\((?P=epoch), (?P<complete>\d+)\)\), '
    r'committed_epoch: Some\((?P=epoch)\), .*? '
    r'sources=Some\(\[(?P<sources>.+?)\]\) exact=true committed=true composited=true '
    r'enter=Some\(\((?P<region>[1-9]\d*), (?P<receiver>[1-9]\d*)\)\) ready=true"\)'
)
RELEASE = re.compile(r"frame (\d+): 11/13 Enter released after frame (\d+)")
SOURCE_PROFILE = {"source": {"probe": "Djibril10", "native": 327, "stored": "Cp932", "output": "Cp932"}}
BODY_RECORDS = {(0, 4): [3], (1, 15): [13, 14]}


def verify(host, events, start_only, first_turn=False):
    presses = [(int(match[1]), match[2]) for line in host if (match := DOWN.fullmatch(line))]
    require(presses[:1] == [(710, "MouseButton(0)")]
            and [key for _, key in presses[1:]] == (["Keyboard(13)"] if first_turn else []),
            "unexpected input timeline")
    starts, op01_loads = [], []
    for line in events:
        if match := SCENE_LOAD.fullmatch(line):
            loads = starts if match[2].lower() == "start" else op01_loads
            loads.append(int(match[1]))
    require(len(starts) == 1 and starts[0] > 710, "missing unique start load after title input")
    if start_only:
        return {"status": "start_verified", "start_frame": starts[0]}

    require(any(frame >= starts[0] for frame in op01_loads), "OP01 load not observed")
    reads = source_reads(events, SOURCE_PROFILE)
    pages = []
    for line in host:
        wait = WAIT.fullmatch(line)
        if not wait or not (proof := LIVE_WAIT.search(wait[2])):
            continue
        frame = int(wait[1])
        page = {key: int(value) for key, value in proof.groupdict().items() if key != "sources"}
        group, record = page["group"], page["wait"]
        body = BODY_RECORDS.get((group, record))
        if not body or f"scene=OP01 group={group} record={record} " not in wait[2]:
            continue
        sources = [BODY_SOURCE.fullmatch(item) for item in re.split(r"(?<=\}), ", proof["sources"])]
        if len(sources) != len(body) or not all(sources):
            continue
        tags = [tuple(map(int, tag.groups())) for tag in sources]
        load = tags[0][1]
        if tags != [(page["context"], load, group, index) for index in body]:
            continue
        loaded, marked, source, complete = (page[key] for key in ("loaded", "marked", "source", "complete"))
        if (loaded in op01_loads and starts[0] <= loaded <= marked < frame
                and loaded <= source <= complete <= frame
                and all(any(loaded <= at <= source for at in reads.get(tag, ())) for tag in tags)
                and f"frame={frame} input-wait native 71 address=0x9f6" in events
                and "message_wait=Some(2550) vm_waiting_for_message=true pending_messages=0 movies=0 " in wait[2]):
            pages.append({"frame": frame, "load": load, "body": body, **page})
    require(pages and pages[0]["wait"] == 4, "OP01 first live wait lacks root, body or receiver proof")
    if not first_turn:
        return {"status": "op01_wait_verified", "start_frame": starts[0], **pages[0]}

    gate = presses[1][0]
    require(len(pages) == 2 and pages[0]["frame"] == gate and pages[1]["wait"] == 15,
            "missing guarded 0/4 -> 1/15 transition")
    before, after = pages
    identity = ("owner", "context", "load", "loaded", "region", "receiver")
    require(all(before[key] == after[key] for key in identity)
            and after["epoch"] > before["epoch"] and min(after["source"], after["marked"]) > gate,
            "stale body or changed receiver")
    resume = gate + 1
    consumers = [match.groups() for line in host for match in RECEIPT.finditer(line)
                 if int(match[1]) > gate and int(match[5]) and match[9] == "0"]
    require(consumers == [(str(resume), "2", "0", "1", "1", "false",
                           str(before["region"]), str(before["receiver"]), "0", ENTER)]
            and f"frame={resume} input-wait native 71 address=0x9f6" in events
            and [line for line in events if line.startswith("frame=") and " message-resume " in line
                 and int(line.split(" ", 1)[0][6:]) > gate]
            == [f"frame={resume} message-resume message=[2, 0, 0, 0]"],
            "missing unique consuming Enter with same-frame root VM resume")
    require([tuple(map(int, match.groups())) for line in host if (match := RELEASE.fullmatch(line))]
            == [(after["frame"], gate)] and after["frame"] > gate + 3,
            "physical Enter release was not scanned")
    require([line for line in events if line.startswith("replay_status=")] == ["replay_status=completed"],
            "guard did not stop at 1/15")
    verify_task_lifetime(events, SimpleNamespace(**before), after["frame"])
    return {"status": "op01_first_turn_verified", "start_frame": starts[0], "pages": pages, "enter_frame": resume}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("run", type=Path)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--start-only", action="store_true")
    mode.add_argument("--first-turn", action="store_true")
    args = parser.parse_args()
    try:
        _, host, events = read_run(args.run, "10")
        print(json.dumps(verify(host, events, args.start_only, args.first_turn), indent=2))
    except (OSError, ValueError, KeyError, TypeError) as error:
        print(json.dumps({"status": "unverified", "reason": str(error)}))
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

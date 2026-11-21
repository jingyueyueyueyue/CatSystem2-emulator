"""Verify the guarded Rebellions opening segment from a batch log."""

import argparse
import json
from pathlib import Path
import re

from check_pc_body_successor import (
    BODY, BODY_GENERATION, BODY_PAGE, CONTEXT, ENTER, ENTER_RESUME, PROFILES,
    REBELLIONS_BODY_BINDING, REBELLIONS_RELEASE, TASK_CREATED, WINDOW,
    collect_events, read_host, rebellions_entry, rebellions_receiver,
)
from check_pc_story_12_op001 import EVENT, require, verify_single_wipe
from check_pc_story_11_13_successor import read_run
from plan_pc_story_09_20_continuation import (
    REBELLIONS_MODES, REBELLIONS_NOVEL_FES, adjacent_pages, read_scene, rebellions_config, scene_steps,
)


WAITS = (13, 15, 26, 30, 32, 34, 38, 40, 42, 44, 48)
BODY_BYTES = (18, 36, 14, 15, 17, 31, 40, 18, 29, 29, 21)
SCENE, _, _, WINDOW_NAME, _, ROOT_WAIT, PE = PROFILES["20"]
NOVEL_WAIT = re.compile(
    r"frame (\d+): Rebellions r211 observation down=(\d+) context=(\d+) loaded=(\d+) binding=(.*?) "
    r"marked=(\d+) owner=(\d+) member=(\S+) sha256=([a-f0-9]{64}) (.+)"
)
COMMAND = re.compile(r'frame=(\d+) native-call index=1 .* debug="\[command\] (.*)"')
NOVEL_TRANSITION = ("rwipe WIPE50 120", "rdraw 120", "novel on", "view 0 novel01", "frameon fade 30")


def verify_enter(events, before, settled, receipt):
    down = before["frame"]
    require((receipt["frame"], receipt["key"], receipt["group"], receipt["raw_mask"], receipt["pressed"],
             receipt["repeated"], receipt["physical"], receipt["region"], receipt["receiver"])
            == (down + 1, 2, 0, 1, 1, False, ENTER, before["region"], before["receiver"])
            and (down + 1, ROOT_WAIT) in events["root_wait"]
            and [(at, packet) for at, packet in events["resume"] if down < at <= settled]
            == [(down + 1, ENTER_RESUME)],
            f"f{down}: Enter consumer or root resume")


def verify_novel_wait(host, lines, events, before, receipt, point):
    matches = [m for line in host if (m := NOVEL_WAIT.fullmatch(line))]
    require(len(matches) == 1, "expected exactly one r211 observation")
    match = matches[0]
    frame, down, context, loaded = map(int, match.groups()[:4])
    marked, owner = map(int, match.groups()[5:7])
    member, digest, detail = match.groups()[7:]
    require((down, context, loaded) == (before["frame"], before["handle"], before["loaded"])
            and marked in events["input"] and down + 3 < frame and down < marked < frame
            and (frame, ROOT_WAIT) in events["root_wait"] and REBELLIONS_NOVEL_FES.get(member) == digest,
            "unexpected r211 wait, load or FES identity")
    verify_enter(events, before, frame, receipt)
    require(all(not any(down < at <= frame for at, *_ in events["rebellions_tasks"].get(before[key], ()))
                for key in ("region", "receiver")), "the previous input target was recreated")
    commands = [m[2].strip() for line in lines if (m := COMMAND.fullmatch(line))
                and down < int(m[1]) <= marked and m[2].strip() in NOVEL_TRANSITION]
    require(tuple(commands) == NOVEL_TRANSITION, "r211 window transition command order")
    reads = [at for handle, load, group, record, at, size in events["rebellions_sources"]
             if (handle, load, group, record, size) ==
             (context, before["load"], point["group"], point["body"], point["body_bytes"])
             and loaded <= at <= frame]
    require(reads, "missing same-load GBK r210 source record")
    return {"frame": frame, "marked": marked, "owner": owner, "member": member, "sha256": digest,
            "binding": match[5], "read_frames": reads, "diagnostic": detail}


def observe_page(frame, status, layers, binding, group, record, body_bytes, events, scene=SCENE):
    owner, region, receiver = binding
    quiet = (f"focused=true paused=false message_wait=Some({ROOT_WAIT}) "
             "vm_waiting_for_message=true pending_messages=0 movies=0")
    require(quiet in status and 'trace_enabled=true' in status and f'pe_sha256=Some("{PE}")' in status
            and 'startup=Some("kcs.int/main.kcs")' in status,
            f"r{record}: root or game identity")
    windows = status.partition(" windows(")[2].partition(" body_wait_shell(")[0]
    active = WINDOW.findall(windows)
    flow = [row for row in active if row[1:] == ("flow", "false", "GAME_WAIT", str(frame))]
    require(len(active) == windows.count('Some(("') == 2 and len(flow) == 1
            and (str(owner), WINDOW_NAME, "true", "MES_SETSTATE", str(frame)) in active
            and layers.get(owner, {}).get(BODY, 0) > 0 and layers.get(owner, {}).get("keywait", 0) > 0,
            f"r{record}: settled body window or submitted pixels")
    context = CONTEXT.fullmatch(status.partition(")=[")[2].partition("] windows(")[0])
    generation, page = BODY_GENERATION.search(status), BODY_PAGE.search(status)
    require(context and context[2] == scene and generation and page, f"r{record}: missing CST proof")
    handle, _, loaded, marked, actual_group, actual_record = context.groups()
    handle, loaded, marked = int(handle), int(loaded), int(marked)
    require((int(actual_group), int(actual_record)) == (group, record)
            and loaded in events["load"] and loaded <= marked < frame and marked in events["input"]
            and (frame, ROOT_WAIT) in events["root_wait"]
            and any(marked <= at <= frame and task == owner for at, task in events["wait_start"]),
            f"r{record}: CST wait or MES_WAITINPUT")
    text_owner, epoch, source_frame, complete = map(int, generation.groups())
    page_context, load, source_group, body_record = map(int, page.groups())
    require((text_owner, page_context, source_group, body_record) == (owner, handle, group, record - 1)
            and loaded <= source_frame <= complete <= frame, f"r{record}: committed body")
    reads = [at for key, aid, g, body, at, size in events["rebellions_sources"]
             if (key, aid, g, body) == (handle, load, group, body_record)
             and loaded <= at <= source_frame and size == body_bytes]
    require(reads, f"r{record}: no matching GBK native 327 body length")
    return {"frame": frame, "group": group, "record": record, "owner": owner, "region": region,
            "receiver": receiver, "loaded": loaded, "handle": handle, "load": load, "epoch": epoch,
            "read_frame": reads[-1], "source_frame": source_frame, "marked": marked, "complete": complete}


def verify(meta, host, lines, mode="opening"):
    events = collect_events(lines, SCENE)
    require(meta.get("index") == 20 and meta.get("mode") == "state" and meta.get("exit_code") == 0
            and meta.get("captures") == [] and not meta.get("first_engine_error")
            and meta.get("replay_status") == "completed" and events["replay_status"] == ["completed"],
            "expected a completed zero-capture state run")
    observations, receipts, presses = read_host(host)
    positions = events["route_pages"]
    novel = mode == "novel_r211"
    if mode in REBELLIONS_MODES:
        limit, _ = REBELLIONS_MODES[mode]
        records = read_scene(Path(meta["game"]), SCENE)
        points = adjacent_pages(scene_steps(records, SCENE)[:limit])
        expected = tuple(p["wait"] for p in points)
        sizes = tuple(p["body_bytes"] for p in points)
        route_file = Path(meta["route_json"]).resolve()
        require(route_file.parent == Path(meta["stderr"]).resolve().parent, "route and logs came from different runs")
        require(json.loads(route_file.read_text(encoding="utf-8")) == rebellions_config(mode)["20"]["route"],
                "wrong dedicated Rebellions route config")
    else:
        expected = {"r13": WAITS[:1], "r15": WAITS[:2], "r26": WAITS[:3], "opening": WAITS}[mode]
        sizes = BODY_BYTES[:len(expected)]
    require([(g, r) for _, g, r in positions] == list(enumerate(expected)), "wrong adjacent wait sequence")
    bindings = [tuple(map(int, match.groups())) for line in host
                if (match := REBELLIONS_BODY_BINDING.fullmatch(line))]
    releases = [int(match[1]) for line in host if (match := REBELLIONS_RELEASE.fullmatch(line))]
    frames = [frame for frame, _, _ in positions]
    require([frame for frame, *_ in bindings] == frames and releases == frames[1:],
            "missing live body binding or scanned release")
    inspect = {frame: (status, layers) for frame, status, layers in observations}
    require(all(frame in inspect for frame in frames), "missing composition inspection")
    pages = [observe_page(frame, *inspect[frame], binding[1:], group, record, size, events)
             for (frame, group, record), binding, size in zip(positions, bindings, sizes)]
    require(rebellions_entry(presses, receipts, events, frames[0])
            and presses[3:] == [(frame, "Keyboard(13)") for frame in (frames if novel else frames[:-1])],
            "entry or body key sequence")
    consumed = [r for r in receipts if r["frame"] > frames[0] and r["pressed"] and r["response"] == 0]
    require(len(consumed) == len(pages) - 1 + novel, "expected one consumer per dispatched body key")
    for before, after, receipt in zip(pages, pages[1:], consumed):
        down, settled = before["frame"], after["frame"]
        verify_enter(events, before, settled, receipt)
        require(settled > down + 3 and after["epoch"] > before["epoch"]
                and min(after["marked"], after["source_frame"]) > down
                and all(before[field] == after[field]
                        for field in ("owner", "region", "receiver", "handle", "load", "loaded"))
                and rebellions_receiver(events["rebellions_tasks"], receipt, before, after)
                and all(not any(down < frame <= settled for frame, *_ in events["rebellions_tasks"].get(before[key], ()))
                        for key in ("owner", "region", "receiver")),
                f"r{after['record']}: generation or task lifetime")
    status = {"r13": "first_wait_verified", "r15": "successor_verified", "r26": "r26_verified",
              "opening": "opening_segment_verified", "r61": "r61_verified", "r189": "r189_verified",
              "novel_r211": "novel_r211_observed"}[mode]
    result = {"status": status, "pages": pages, "receipts": consumed}
    if len(pages) >= 3:
        tasks = {}
        for line in lines:
            if (event := EVENT.fullmatch(line)) and (task := TASK_CREATED.match(event[2])):
                tasks.setdefault(int(task[1]), []).append((int(event[1]), int(task[2]), int(task[3]), task[4]))
        result["r26_wipe"] = verify_single_wipe(lines, tasks, pages[1], pages[2], "rwipe WIPE01A 120", 118)
        if mode in REBELLIONS_MODES:
            result["r58_wipe"] = verify_single_wipe(lines, tasks, pages[10], pages[11], "rwipe WIPE01 120", 118)
        if mode in ("r189", "novel_r211"):
            result["r150_wipe"] = verify_single_wipe(lines, tasks, pages[12], pages[13], "rwipe WIPE31 90", 88)
    if novel:
        point = adjacent_pages(scene_steps(records, SCENE)[:28])[-1]
        result["novel"] = verify_novel_wait(host, lines, events, pages[-1], consumed[-1], point)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("run", type=Path, help="batch directory or its 20 subdirectory")
    parser.add_argument("--mode", choices=("r13", "r15", "r26", "opening", *REBELLIONS_MODES), default="opening")
    args = parser.parse_args()
    sample = args.run / "20" if (args.run / "20").is_dir() else args.run
    batch = sample.parent if sample.name == "20" else sample
    try:
        meta = json.loads((batch / "20.json").read_text(encoding="utf-8"))
        if meta.get("status") == "resource_deferred":
            print(json.dumps({"status": "resource_deferred"}))
            return 2
        if args.mode in REBELLIONS_MODES:
            identity, host, lines = read_run(batch, "20")
            result = {"build": identity, **verify(meta, host, lines, args.mode)}
        else:
            host = (sample / "20.host.log").read_text(encoding="utf-8", errors="replace").splitlines()
            lines = (sample / "20.events.log").read_text(encoding="utf-8", errors="replace").splitlines()
            result = verify(meta, host, lines, args.mode)
    except (KeyError, OSError, ValueError) as error:
        print(json.dumps({"status": "rejected", "reason": str(error)}))
        return 1
    print(json.dumps(result, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())


"""Check extended routes or read-only boundary observations from PC replay logs."""

import argparse
import json
from pathlib import Path
import re

import check_pc_story_08_eyecatch as eye08
import check_pc_story_14_18_continuous as body14_18
from plan_pc_story_08_14_18 import CASES, MANIFEST, build, preceding_page


LOAD = re.compile(r'^frame=(\d+) native-call .*debug="sscript loaded > ([^"]+)"$', re.M)
DOWN = re.compile(r"^frame (\d+): down (.*)$", re.M)
require = body14_18.require


def log_files(batch, index):
    label = f"{index:02}"
    prefix = batch / label / label
    result = json.loads((batch / f"{label}.json").read_text(encoding="utf-8"))
    return result, prefix.with_suffix(".host.log").read_text(encoding="utf-8"), (
        prefix.with_suffix(".events.log").read_text(encoding="utf-8")
    )


def body_route(index, result, host, events, review, candidate):
    pages = review["pages"][:review["previous_group"] + 1] if candidate else review["pages"]
    if index != 8:
        return body14_18.classify(index, result, host, events, review | {"pages": pages})
    expected = [(p["group"], p["wait"], p["wait_ordinal"], p["body"][0], p["body_sizes"][0]) for p in pages]
    return {"index": index} | eye08.classify(result, host, events, len(pages) - 1, expected)


def boundary_location(scenes, mode):
    kind = {"scene": "next", "choice": "choice", "auto": "auto"}[mode]
    for item in scenes.values():
        for row in item["boundaries"]:
            if row["kind"] == kind:
                return item, row
    raise ValueError(f"no {kind} boundary on the scanned path")


def boundary_press(index, scene, page, host, events):
    downs = DOWN.findall(host)
    require(downs and downs[-1][1] == "Keyboard(13)", "last physical down is not the boundary Enter")
    at = int(downs[-1][0])
    group, wait = page["group"], page["wait"]
    if index == 8:
        require(any(tuple(map(int, proof[:3])) == (at, group, wait)
                    for proof in eye08.ORDINARY_BODY.findall(events)), "08 boundary page proof missing")
        arm = re.findall(rf"^frame {at}: 08 armed Enter region=(\d+) receiver=(\d+) candidates=", host, re.M)
    elif index == 14:
        arm = re.findall(rf"^frame {at}: 14 armed {re.escape(scene)} g{group}/{wait} "
                         r"region=(\d+) receiver=(\d+) body=", host, re.M)
    else:
        arm = re.findall(rf"^frame {at}: 18 armed g{group}/{wait} body=.* bottom=\((\d+), (\d+)\)", host, re.M)
    require(len(arm) == 1, "last pre-boundary wait was not armed")
    receipt = at + 1
    consumed = [(int(f), int(key), region, owner) for f, key, region, owner, response
                in eye08.KEYBOARD_RECEIPT.findall(host) if int(f) >= receipt and response == "0"]
    require(consumed == [(receipt, 2, *arm[0])], "wrong, duplicate or late Enter consumption")
    positive = body14_18.PRESS.findall(host)
    require(len(positive) == len(re.findall(r"\bpressed: [1-9]\d*,", host))
            and all(int(row[0]) < receipt or int(row[0]) == receipt
                    and row[-1] == "[RoutedPhysicalButton { source: Keyboard, code: 13 }]"
                    for row in positive), "unparsed or unexpected physical receipts at or after boundary Enter")
    require(re.findall(rf"^frame={receipt} message-resume (.*)$", events, re.M) == ["message=[2, 0, 0, 0]"],
            "boundary Enter did not resume the root VM in the receipt frame")
    return receipt


def wait_snapshot(result, host, events, scene, page=None, expected_address=None):
    frame = result.get("last_frame")
    snapshots = re.findall(rf"^(?:frame {frame}: wait |replay wait: )(.*\bframe={frame} .*)$", host, re.M)
    require(snapshots, "terminal host wait snapshot missing; collect inspect-log at the observed stop")
    snapshot = snapshots[-1]
    idle = re.search(rf"\bscene={re.escape(scene)} group=-?\d+ record=-?\d+ focused=true paused=false "
                     r"message_wait=Some\((\d+)\) vm_waiting_for_message=true pending_messages=0 movies=0 ",
                     snapshot, re.I)
    address = hex(int(idle[1])) if idle else None
    require(address is not None and (expected_address is None or address == expected_address)
            and re.search(rf"^frame={frame} input-wait native 71 address={address}$", events, re.M),
            "destination root VM was not observed in its live idle wait")
    if page:
        require(f"scene={scene} group={page['group']} record={page['wait']} ".lower() in snapshot.lower(),
                "destination did not settle at the expected executed page")
    return snapshot


def observe_boundary(index, result, host, events, scenes, mode):
    require(result.get("mode") == "state" and result.get("index") == index
            and result.get("first_engine_error") is None and result.get("exit_code") == 0
            and result.get("timed_out") is False and isinstance(result.get("last_frame"), int),
            "unclean or non-state replay")
    item, row = boundary_location(scenes, mode)
    page = preceding_page(item, row)
    require(page and page["kind"] == "input", "missing source body wait")
    receipt = boundary_press(index, item["scene"], page, host, events)
    loads = [(int(frame), name) for frame, name in LOAD.findall(events)]
    prior_loads = [(at, name) for at, name in loads if at < receipt]
    require(prior_loads and prior_loads[-1][1].lower() == item["scene"].lower(), "wrong source CST load")
    commands = [int(frame) for frame in re.findall(
        rf'^frame=(\d+) native-call .*debug="\[command\] {re.escape(row["command"])}"$', events, re.M)
        if int(frame) >= receipt]
    targets = [(at, name.lower()) for at, name in loads if at >= receipt]
    if mode == "scene" and row.get("conditional"):
        # On 14 the conditional and the post-movie continuation share the same target.
        sibling = next(r for r in item["boundaries"] if r["kind"] == "next" and not r["conditional"])
        commands += [int(at) for at in re.findall(
            rf'^frame=(\d+) native-call .*debug="\[command\] {re.escape(sibling["command"])}"$', events, re.M)
            if int(at) >= receipt]
    require(commands, f"missing executed {row['command']} trace after the boundary receipt")
    if mode == "scene":
        route = [row["target"].lower()]
        destination = scenes[route[-1]]
        while not destination["pages"]:
            successor = next(r["target"] for r in destination["boundaries"] if r["kind"] == "next" and not r["conditional"])
            route.append(successor.lower())
            destination = scenes[route[-1]]
        require([name for _, name in targets] == route and min(commands) <= targets[0][0], "unexpected CST load chain")
        calls = body14_18.native_frames(events)
        first = destination["pages"][0]
        loaded = targets[-1][0]
        context = [words[0] for slot, words, _ in calls.get(loaded, []) if slot == 325 and len(words) == 2]
        require(len(context) == 1, "missing unique destination native325 load")
        waits = [at for at, entries in calls.items() if loaded <= at < result["last_frame"]
                 and (326, (context[0], first["group"], first["wait_ordinal"]), "") in entries
                 and any(slot == 1 and detail == ' debug="[input]"' for slot, _, detail in entries)]
        require(len(waits) == 1, "first destination [input] marker missing or ambiguous")
        body14_18.verify_page(calls, context[0], first["group"], waits[0], first)
        snapshot = wait_snapshot(result, host, events, destination["scene"], first)
        contexts = re.findall(r'\((\d+), "([^"]+)", (\d+),', snapshot.split(" windows(", 1)[0])
        require([(int(handle), name.lower(), int(at)) for handle, name, at in contexts]
                == [(context[0], destination["scene"].lower(), loaded)],
                "terminal CST context differs or the previous load is still active")
    else:
        require(not targets, "unexpected CST load after the observation-only boundary Enter")
        snapshot = wait_snapshot(result, host, events, item["scene"])
        if mode == "choice":
            require(re.search(r'\(\d+, "select(?:\.fes)?", (?:true|false), Some\(\("WAIT", false,', snapshot),
                    "live choice window not proven by the terminal inspector")
        else:
            returned = [int(at) for at in re.findall(r'^frame=(\d+) .*debug="\[command\] auto off"$', events, re.M)]
            require(any(min(commands) < at < result["last_frame"] for at in returned),
                    "auto did not return to manual mode after this boundary")
    return {"index": index, "gate": "boundary_observed", "source": item["scene"],
            "source_wait": [page["group"], page["wait"]], "receipt_frame": receipt,
            "boundary": row, "new_loads": targets, "later_physical_inputs": 0,
            "next_input_authorized": False}


def diagnose_08_g0(result, host, events):
    stop = result.get("last_frame")
    require(result.get("index") == 8 and result.get("replay_status") == "unrecognized_wait"
            and result.get("first_engine_error") is None
            and f"replay frame={stop}: 08 g0/144 did not settle: 08 executed CST input is missing or pending" in host,
            "expected the first 08 CST wait stop")
    releases = list(map(int, eye08.RELEASE.findall(host)))
    downs = [(int(frame), key) for frame, key in DOWN.findall(host)]
    require(len(releases) == 1 and len(downs) == 3
            and all(key == "MouseButton(0)" and frame < releases[0] for frame, key in downs),
            "expected one eye release and no subsequent input")
    released = releases[0]
    receipt = downs[-1][0] + 1
    require(re.findall(rf"^frame={receipt} message-resume message=(.*)$", events, re.M)
            == ["[2, 640, 360, 0]"], "eye press lacks the original VM resume")
    loads = LOAD.findall(events)
    require(len(loads) == 1, "expected one CST load")
    loaded, scene = loads[0]
    require(scene == "c01_01", "wrong first scene")
    snapshot = wait_snapshot(result, host, events, scene, expected_address="0x32f")
    pending = re.search(r'contexts\(id,scene,loaded,candidate,page\)=\[\((\d+), "c01_01", '
                        r'(\d+), Some\(\((\d+), 2, 0, 144\)\), None\)\]', snapshot)
    require(pending and pending[2] == loaded, "same-load r144 candidate missing")
    context, loaded, queried = map(int, pending.groups())
    calls = body14_18.native_frames(events)
    require(loaded < queried < receipt <= released < stop
            and (326, (context, 0, 144), "") in calls[queried],
            "CST lookup was not observed before the eye press")
    owners = [int(owner) for frame, owner, kind, _, name in eye08.TASK.findall(events)
              if int(frame) <= loaded and kind == "12" and name == "meswnd"]
    require(len(owners) == 1, "expected one ordinary meswnd at first scene load")
    owner = owners[0]
    require(not any(
        (slot == 1 and detail == ' debug="[input]"') or (slot == 69 and words == (owner, 13, 0, 0))
        for frame, rows in calls.items() if loaded <= frame <= stop for slot, words, detail in rows
    ), "script input marker or reveal completion already exists")
    report = {"index": 8, "gate": "g0_r144_candidate_only", "loaded_frame": loaded,
              "candidate_frame": queried, "eye_receipt_frame": receipt, "released_frame": released,
              "stop_frame": stop, "meswnd": owner, "next_input_authorized": False}
    commands = [(frame, detail) for frame, rows in calls.items() if released <= frame <= stop
                for slot, _, detail in rows if slot == 1 and detail.startswith(' debug="[command] ')]
    wipe = [(frame, int(task), int(parent)) for at, task, kind, parent, name in eye08.TASK.findall(events)
            if kind == "39" and name == "rwipe" and (frame := int(at)) >= released]
    if len(wipe) == 1:
        frame, task, parent = wipe[0]
        if commands[-2:] == [(frame, ' debug="[command] rwipe wipe101 30"'),
                             (frame, ' debug="[command] wait"')]:
            tail = [words for slot, words, _ in calls[frame] if slot == 327 and words[:1] == (context,)]
            if [words[2:] for words in tail[-2:]] == [(0, 130), (0, 131)]:
                report["last_executed_command"] = {"record": 131, "frame": frame,
                                                   "wipe_task": task, "script_task": parent}
    return report


def diagnose_14_g5(result, host, events, pages):
    last = result.get("last_frame")
    require(result.get("index") == 14 and result.get("replay_status") == "unrecognized_wait"
            and re.search(rf"^replay frame={last}: 14 OP g5/115 did not settle: "
                          r"current body has no natural completion and submission$", host, re.M),
            "expected the original g5 readiness stop")
    receipt = boundary_press(14, "OP", pages[4], host, events)
    waits = re.findall(r'contexts\(id,scene,loaded,candidate,page\)=\[\((\d+), "OP", (\d+), None, '
                       r'Some\(\((\d+), (\d+), 5, 115\)\)\)\]', host)
    require(len(waits) == 1, "unique executed r115 and OP load missing")
    context, loaded, marker, load = map(int, waits[0])
    generations = re.findall(
        r"14 body owner=12 generation=Some\(StoryBodyGeneration \{ source_epoch: (\d+), source_frame: (\d+), "
        r"natural_complete: Some\(\((\d+), (\d+)\)\), committed_epoch: Some\((\d+)\), remaining: 0, "
        r"remaining_segment: 0, pending_text: false, drawing: false, skip_pending: false, forced: false, "
        r"projection: Some\(\(true, ([1-9]\d*)\)\)", host)
    require(generations, "natural visible committed body diagnostic missing")
    epoch, source, natural_epoch, complete, committed_epoch, _ = map(int, generations[-1])
    require(0 < epoch == natural_epoch == committed_epoch and loaded < receipt == source < marker <= complete < last
            and body14_18.valid_14_source_order(5, source, marker, receipt, last),
            "g5 does not match the verified one-frame early queue")
    calls = body14_18.native_frames(events)
    body14_18.verify_page(calls, context, 5, marker, pages[5])
    read = re.findall(
        rf"^frame=(\d+) cst-source-record .*context_handle: {context}, load_id: {load}, group: 5, absolute_record: 114 "
        rf"\}}, frame: \1, probe: Grisaia14, native: 327, kind: 32, stored_codec: Gbk, output_codec: Gbk, "
        rf"raw_bytes: {pages[5]['body_sizes'][0]}, output_bytes: {pages[5]['body_sizes'][0]}, "
        r"body_tagged: true, localized: false \}$", events, re.M)
    require(len(read) == 1 and loaded <= int(read[0]) <= marker, "original r114 source read missing")
    wait_snapshot(result, host, events, "OP", pages[5], "0x918")
    return {"index": 14, "gate": "g5_marker_order_reproduced", "load": load, "earliest_read": int(read[0]),
            "receipt": receipt, "source": source, "marker": marker, "natural_complete": complete,
            "committed_epoch": epoch, "stop": last, "next_input_authorized": False}


def diagnose_18_g4(result, host, events, pages):
    last = result.get("last_frame")
    require(result.get("index") == 18 and result.get("replay_status") == "unrecognized_wait"
            and f"replay frame={last}: 18 g4/112 body did not settle: executed CST input marker missing or pending"
            in host, "expected the original g4 readiness stop")
    receipt = boundary_press(18, "0001_00", pages[3], host, events)
    snapshot = wait_snapshot(result, host, events, "0001_00", pages[3], "0x59f")
    context = re.search(rf'\((\d+), "0001_00", (\d+), Some\(\({receipt}, 2, 4, 112\)\), '
                        r'Some\(\((\d+), 2, 3, 100\)\)\)', snapshot)
    require(context, "r112 candidate and r100 executed wait do not match the known stop")
    calls = body14_18.native_frames(events)
    rows = calls.get(receipt, [])
    exact_read = lambda slot, words: (slot == 327 and len(words) == 4 and words[0] == int(context[1])
                                     and words[2:] == (4, pages[4]["body_ordinals"][0]))
    require(any(exact_read(slot, words) and detail.startswith(' debug="[message] ')
                for (slot, words, _), (_, _, detail) in zip(rows, rows[1:])),
            "the r102 message execution is missing")
    commands = [detail for slot, _, detail in rows if slot == 1]
    draw = ' debug="[command] rdraw 30"'
    require(draw in commands and ' debug="[command] wait"' in commands[commands.index(draw) + 1:]
            and all(detail != ' debug="[input]"' for at, entries in calls.items()
                    if receipt <= at <= last for _, _, detail in entries),
            "the script did not stop between rdraw and [input]")
    owners = re.findall(rf'^frame={receipt} task-create id=(\d+) kind=38 parent=(\d+) name="rdraw"', events, re.M)
    require(len(owners) == 1, "expected one rdraw task at the receipt frame")
    return {"index": 18, "gate": "g4_rdraw_wait_reproduced", "receipt": receipt, "context": int(context[1]),
            "rdraw_task": int(owners[0][0]), "script": int(owners[0][1]),
            "candidate_wait": [4, 112], "executed_wait": [3, 100], "stop": last, "next_input_authorized": False}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("batch", type=Path, nargs="?")
    parser.add_argument("--indices", nargs="+", type=int, choices=CASES, default=list(CASES))
    modes = parser.add_mutually_exclusive_group()
    modes.add_argument("--static", action="store_true")
    modes.add_argument("--candidate", action="store_true", help="Check the shorter 22:50 routes")
    modes.add_argument("--boundary", choices=("scene", "choice", "auto"))
    modes.add_argument("--diagnose-08-g0", action="store_true")
    modes.add_argument("--diagnose-14-g5", action="store_true")
    modes.add_argument("--diagnose-18-g4", action="store_true")
    args = parser.parse_args()
    if args.static == (args.batch is not None):
        parser.error("provide either --static or a replay batch directory")
    if args.diagnose_08_g0:
        try:
            report = diagnose_08_g0(*log_files(args.batch, 8))
        except (OSError, ValueError) as error:
            report = {"index": 8, "gate": "unproven", "reason": str(error)}
        print(json.dumps(report, indent=2))
        return int(report["gate"] == "unproven")
    if args.diagnose_14_g5:
        args.indices = [14]
    if args.diagnose_18_g4:
        args.indices = [18]
    manifest, scenes = build(args.indices)
    stored = json.loads(MANIFEST.read_text(encoding="utf-8"))
    reports = []
    for index in args.indices:
        key = str(index)
        review = manifest["review"]["routes"][key]
        try:
            require(manifest[key] == stored[key] and review == stored["review"]["routes"][key]
                    and all(stored["review"].get(field) == value
                            for field, value in manifest["review"].items() if field != "routes"),
                    "scenario differs from original CST derivation")
            if args.static:
                reports.append({"index": index, "gate": "static_verified", "waits": [p["wait"] for p in review["pages"]]})
            elif args.boundary:
                reports.append(observe_boundary(index, *log_files(args.batch, index), scenes[index], args.boundary))
            elif args.diagnose_14_g5:
                reports.append(diagnose_14_g5(*log_files(args.batch, index), review["pages"]))
            elif args.diagnose_18_g4:
                reports.append(diagnose_18_g4(*log_files(args.batch, index), review["pages"]))
            else:
                reports.append(body_route(index, *log_files(args.batch, index), review, args.candidate))
        except (OSError, ValueError) as error:
            reports.append({"index": index, "gate": "unproven", "reason": str(error)})
    print(json.dumps(reports, indent=2))
    return int(any(row["gate"] == "unproven" for row in reports))


if __name__ == "__main__":
    raise SystemExit(main())

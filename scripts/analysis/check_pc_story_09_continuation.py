"""Verify guarded Kanmi waits and consuming Enters, including the sc02 handoff."""

import argparse
from collections import defaultdict
import json
from pathlib import Path
import re

from check_pc_body_successor import (
    ENTER, ENTER_RESUME, KANMI_ENTER_TARGET, KANMI_SOURCE, KANMI_TITLE_PRESS, KANMI_TITLE_RESUME,
    LEFT, ROUTE_PAGE, TASK_CREATED, collect_events, read_host,
)
from check_pc_story_11_13_sources import require
from check_pc_story_11_13_successor import read_run
from check_pc_story_12_op001 import EVENT, verify_single_wipe
from plan_pc_story_09_20_continuation import (
    KANMI_LOAD_MODES, KANMI_MODES, KANMI_SC02_SEPARATED_BODY, SCENES, adjacent_pages, kanmi_config, kanmi_load_config,
    read_scene, scene_steps,
)
from run_playthrough_batch import file_sha256


CONFIGS = {
    mode: Path(__file__).with_name(f"pc_story_09_title_to_{mode}_guarded.json")
    for mode in ("r47", "r53", "r80", "r102")
}
R43_ONLY_ERROR = "Kanmi requires its exact title and two body checkpoints"
ADJACENT_CONTRACT_ERROR = "Kanmi requires its exact title and adjacent body checkpoints"
BODY = re.compile(
    r"frame=(\d+) kanmi-route-body owner=([1-9]\d*) region=(\d+) receiver=(\d+) "
    r"context=([1-9]\d*) load=([1-9]\d*) body=(\d+) epoch=([1-9]\d*) source_frame=(\d+)"
)
TRANSITION_COMMAND = re.compile(r'frame=(\d+) native-call index=1 .* debug="\[command\] (.*)"')
BUS_COMMANDS = ("particle 0 stop", "particle 1 stop", "frameoff fade", "rwipe wipe02c 100",
                "wait 60", "wait 180", "wait 480", "wait 60", "rwipe wipe02c 100", "wait 30", "se 1")
SCENE_LOAD = re.compile(r"frame=(\d+) kanmi-scene-load handle=(\d+) load_id=(\d+) scene=sc02 sha256=([a-f0-9]{64})")
SC02_SOURCE = re.compile(KANMI_SOURCE.pattern.replace("probe: Kanmi,", "probe: KanmiSc02,"))
PREFIX = re.compile(r"frame=(\d+) kanmi-body-prefix record=(Some\(555\)|None)")
CROSS_SCENE_COMMANDS = ("frameoff fade", "rwipe wipe04a 100", "frameoff fade", "rdraw 100",
                        "wait 60", "rwipe wipe07a 240", "wait 240", "rdraw 100", "wait 60", "next sc02")
RASTER_CONFIGURE = re.compile(
    r"kanmi-raster configure owner=(\d+) target=(\d+) mode=0x00000001 "
    r"amplitude=0x41200000 wavelength=0x43480000 phase_period=0x43960000"
)
RASTER_START = re.compile(r"kanmi-raster start owner=(\d+) target=(\d+) bound=true")
RASTER_NATIVE = re.compile(r"native-call index=(66|67|196|197) arguments=\d+ bytes \[([0-9a-f]+)\]")
SAVE_CONTROL = re.compile(
    r"replay frame=(\d+) kanmi_ordinary_save_control slot=(false|true) region=(\d+) owner=(\d+) point=\[-?\d+, -?\d+\]"
)
SAVE_MOUSE = re.compile(
    r"replay frame=(\d+) kanmi_ordinary_save_received slot=(false|true) region=(\d+) owner=(\d+) response=([01])"
)
SAVE_DONE = re.compile(r"replay frame=(\d+) kanmi_ordinary_save scene=sc02 group=25 record=147 slot=0 sha256=([0-9a-f]{64})")
LOAD_OPEN = re.compile(r"replay frame=(\d+) kanmi_ordinary_load_open slot=0 sha256=([0-9a-f]{64})")
OP_ARRIVED = re.compile(r"replay frame=(\d+) kanmi_ordinary_op_arrived branch=(sc02_0[12]) "
                        r"branch_frame=(\d+) natural_movie_frame=(\d+) scene=sc03 group=0 record=7")
OP_BRANCH_DIGESTS = {
    "sc02_01": "6643eaccfaf2898448a8bfbe3527916d1980b4111658502d6f47dd1b23f7958d",
    "sc02_02": "57471b4c2cbafeeaf7d2e02bc7a2a5bf1685ace512636e77baea973523b2fd87",
}
LOAD_BODY = re.compile(
    r"replay frame=(\d+) kanmi_ordinary_load_(restored|advanced) context=(\d+) load=(\d+) body=(\d+) epoch=(\d+) source_frame=(\d+)"
)


def verify_raster_transition(lines, before, after):
    anchors = defaultdict(list)
    calls = []
    commands = ("rdraw 60", "pl 0 raster 1 10 200 300", "wait", "wait 30", "pl 0 stop")
    for index, line in enumerate(lines):
        if not (event := EVENT.fullmatch(line)) or not before["frame"] < int(event[1]) <= after["frame"]:
            continue
        frame, detail = int(event[1]), event[2]
        if (command := TRANSITION_COMMAND.fullmatch(line)) and command[2] in commands:
            anchors[command[2]].append((index, frame))
        if (task := TASK_CREATED.match(detail)) and (task[2], task[4]) == ("18", "raster"):
            anchors["created"].append((index, frame, int(task[1]), int(task[3])))
        if configured := RASTER_CONFIGURE.fullmatch(detail):
            anchors["configured"].append((index, frame, int(configured[1]), int(configured[2])))
        if started := RASTER_START.fullmatch(detail):
            anchors["started"].append((index, frame, int(started[1]), int(started[2])))
        if native := RASTER_NATIVE.fullmatch(detail):
            args = bytes.fromhex(native[2])
            if len(args) >= 4:
                calls.append((index, frame, int(native[1]), int.from_bytes(args[:4], "little")))
    required = (*commands, "created", "configured", "started")
    require(all(len(anchors[name]) == 1 for name in required), "missing or repeated Raster control or binding receipt")
    created, configured, started = (anchors[name][0] for name in ("created", "configured", "started"))
    owner, parent = created[2:]
    target = configured[3]
    require(owner > 0 and target > 0 and parent == before["receiver"]
            and configured[2:] == started[2:] == (owner, target),
            "Raster factory, 20-byte record and callback did not agree on owner and target")
    rdraw, raster, wait, timed_wait, stop = (anchors[name][0] for name in commands)
    require(rdraw[0] < raster[0] < wait[0] < timed_wait[0] < stop[0]
            and raster[0] < created[0] < configured[0] < started[0] < stop[0],
            "Raster callback did not follow the scripted rdraw and waits")
    for slot, before_event, after_event, arg in (
        (66, raster[0], created[0], 18),
        (196, created[0], configured[0], owner),
        (197, configured[0], started[0], owner),
    ):
        require(any(index == slot and word == arg and before_event < i < after_event
                    for i, _, index, word in calls), f"missing Raster native {slot} arguments")
    return {"from": 971, "to": 983, "owner": owner, "target": target,
            "configured": configured[1], "started": started[1], "stop": stop[1]}


def verify_raster_reset(lines, before, after, raster):
    reset = "pl 0 raster"
    commands = []
    deletes = []
    replacements = []
    for index, line in enumerate(lines):
        if not (event := EVENT.fullmatch(line)) or not before["frame"] < int(event[1]) <= after["frame"]:
            continue
        frame, detail = int(event[1]), event[2]
        if (command := TRANSITION_COMMAND.fullmatch(line)) and command[2] == reset:
            commands.append((index, frame))
        if (task := TASK_CREATED.match(detail)) and int(task[1]) == raster["owner"]:
            replacements.append(index)
        if (call := RASTER_NATIVE.fullmatch(detail)) and call[1] == "67":
            args = bytes.fromhex(call[2])
            if len(args) == 4 and int.from_bytes(args, "little") == raster["owner"]:
                deletes.append((index, frame))
    require(len(commands) == 1, "expected a single no-argument raster reset")
    command_index, command_frame = commands[0]
    lifetime_end = min((index for index in replacements if index > command_index), default=len(lines))
    raster_deletes = [(index, frame) for index, frame in deletes if command_index < index < lifetime_end]
    require(raster_deletes == [(command_index + 1, command_frame)],
            "the no-argument raster reset must delete the configured task before its ID is reused")
    return {"from": 1004, "to": 1029, "owner": raster["owner"], "reset": command_frame, "delete": command_frame}


def verify_sc02_load(lines, before, after):
    require((before["scene"], before["group"], before["wait"]) == ("sc01", 293, 1324)
            and (after["scene"], after["group"], after["wait"]) == ("sc02", 0, 10)
            and after["load"] > before["load"], "unexpected Kanmi scene handoff")
    matches = [(index, m) for index, line in enumerate(lines) if (m := SCENE_LOAD.fullmatch(line))]
    require(len(matches) == 1, "expected one verified sc02 load")
    load_index, load = matches[0]
    at, context, load_id = map(int, load.groups()[:3])
    require((context, load_id, load[4]) == (after["context"], after["load"], SCENES["sc02"][2])
            and before["frame"] < at <= after["read_frame"] <= after["source_frame"],
            "sc02 load did not precede its committed body")
    commands = [(index, int(m[1]), m[2].strip()) for index, line in enumerate(lines)
                if (m := TRANSITION_COMMAND.fullmatch(line))]
    exit_commands = [cmd for index, frame, cmd in commands if index < load_index and before["frame"] < frame <= at
                     and cmd in CROSS_SCENE_COMMANDS]
    entry_commands = [cmd for index, frame, cmd in commands if index > load_index and at <= frame <= after["frame"]
                      and cmd in ("rdraw 100", "wait 60")]
    require(tuple(exit_commands) == CROSS_SCENE_COMMANDS and entry_commands == ["rdraw 100", "wait 60"],
            "unexpected sc01 exit or sc02 entry command order")
    return {"loaded": at, "context": context, "load": load_id, "sha256": load[4],
            "exit_commands": exit_commands, "entry_commands": entry_commands}


def verify_bus_transition(lines, before, after):
    commands = [(int(match[1]), match[2].strip()) for line in lines if (match := TRANSITION_COMMAND.fullmatch(line))
                and before < int(match[1]) <= after and match[2].strip() in BUS_COMMANDS]
    require(tuple(command for _, command in commands) == BUS_COMMANDS,
            "r53 to r78 did not traverse the expected effect and wait sequence")
    first, second = (frame for frame, command in commands if command == "rwipe wipe02c 100")
    tasks = defaultdict(list)
    for line in lines:
        if (event := EVENT.fullmatch(line)) and before < int(event[1]) <= after:
            if task := TASK_CREATED.match(event[2]):
                tasks[int(task[1])].append((int(event[1]), int(task[2]), int(task[3]), task[4]))
    wipes = [verify_single_wipe(lines, tasks, {"frame": start, "receiver": 8}, {"marked": stop},
                                "rwipe wipe02c 100", 98)
             for start, stop in ((before, second - 1), (second - 1, after))]
    require([wipe["configured"] for wipe in wipes] == [first, second], "wrong bus wipe task frames")
    return {"from": 53, "to": 78, "commands": commands, "wipes": wipes}


def verify_ordinary_write(lines, downs, receipts, terminal, profile):
    controls = [m.groups() for line in lines if (m := SAVE_CONTROL.fullmatch(line))]
    mouse = [m.groups() for line in lines if (m := SAVE_MOUSE.fullmatch(line))]
    done = [m.groups() for line in lines if (m := SAVE_DONE.fullmatch(line))]
    require(len(controls) == len(mouse) == 2 and len(done) == 1, "missing ordinary save control, mouse or file receipt")
    require([c[1] for c in controls] == ["false", "true"] and int(controls[0][3]) == terminal["owner"]
            and controls[0][3] != controls[1][3], "wrong ordinary save control owners")
    frames = [int(c[0]) for c in controls]
    require(terminal["frame"] <= frames[0] < frames[1] and downs == [(frame, "MouseButton(0)") for frame in frames],
            "unexpected ordinary save clicks")
    for (start, mode, region, owner), (at, actual_mode, actual_region, actual_owner, response) in zip(controls, mouse):
        hit = [r for r in receipts if r["frame"] == int(at) and r["key"] == r["group"] == 0
               and r["raw_mask"] == 2 and r["pressed"] == 1 and not r["repeated"] and r["physical"] == LEFT]
        expected = (int(region), int(owner), int(response))
        require((mode, region, owner) == (actual_mode, actual_region, actual_owner) and 0 < int(at) - int(start) <= 3
                and [(r["region"], r["receiver"], r["response"]) for r in hit
                     if r["response"] == 0 or (r["region"], r["receiver"]) == expected[:2]] == [expected],
                "ordinary save mouse receipt did not match its guarded target")
    writes = [m.groups() for line in lines if (m := EVENT.fullmatch(line)) and m[2].startswith("fes-datasave ")]
    saved_at, digest = done[0]
    slot = profile / "save0000.dat"
    require(len(writes) == 1 and int(mouse[-1][0]) <= int(writes[0][0]) <= int(saved_at)
            and writes[0][1].startswith("fes-datasave slot=0 container=") and writes[0][1].endswith(" written=true")
            and slot.is_file() and file_sha256(slot) == digest,
            "ordinary save does not match the slot 0 write and file readback")
    return {"slot": 0, "path": str(slot), "sha256": digest, "frame": int(saved_at), "write_frame": int(writes[0][0])}


def verify_ordinary_reopen(run, build, game, save, terminal=149, through_op=False):
    summary = json.loads((run / "09.json").read_text(encoding="utf-8"))
    identity, host, lines = read_run(run, "09")
    require((build is None or identity == build) and str(Path(summary["game"])) == str(Path(game)),
            "cold load used a different build or game")
    route = Path(summary["route_json"]).resolve()
    profile = Path(summary["profile"]).resolve()
    require(route.is_relative_to(run.resolve()) and profile.is_relative_to(run.resolve())
            and json.loads(route.read_text(encoding="utf-8"))
            == kanmi_load_config(terminal, through_op=through_op)["9"]["route"],
            "cold load route or profile mismatch")
    staged = profile / "save0000.dat"
    require(staged.resolve() != Path(save["path"]).resolve() and staged.is_file() and file_sha256(staged) == save["sha256"],
            "cold load requires a matching slot in a distinct profile")
    opened = [m.groups() for line in lines if (m := LOAD_OPEN.fullmatch(line))]
    bodies = [m.groups() for line in lines if (m := LOAD_BODY.fullmatch(line))]
    end_group, _, _ = KANMI_LOAD_MODES[terminal]
    steps = scene_steps(read_scene(Path(game), "sc02"), "sc02")[:end_group + 1]
    points = adjacent_pages(steps, KANMI_SC02_SEPARATED_BODY)[25:]
    frames = [int(b[0]) for b in bodies]
    expected_bodies = [("restored" if i == 0 else "advanced", str(p["body"])) for i, p in enumerate(points)]
    require(len(opened) == 1 and opened[0][1] == save["sha256"]
            and [b[1:2] + b[4:5] for b in bodies] == expected_bodies
            and len({b[2:4] for b in bodies}) == 1
            and [(int(m[1]), m[2], int(m[3]), int(m[4])) for line in lines if (m := ROUTE_PAGE.fullmatch(line))]
            == [(frame, "sc02", point["group"], point["wait"]) for frame, point in zip(frames, points)],
            "cold load did not restore or advance the registered bodies")
    restored, advanced = frames[0], frames[-1]
    reads = set()
    load_events = []
    for line in lines:
        if match := SCENE_LOAD.fullmatch(line):
            load_events.append((int(match[1]), match.groups()[1:3], match[4]))
        if (event := EVENT.fullmatch(line)) and (source := SC02_SOURCE.fullmatch(event[2])):
            require(event[1] == source[5], "CST read frame mismatch")
            reads.add(tuple(map(int, source.groups())))
    _, receipts, downs = read_host(host)
    dispatched = frames if through_op else frames[:-1]
    require(len(load_events) == 1 and load_events[0][1:] == (bodies[0][2:4], SCENES["sc02"][2])
            and len(downs) > 2 and [b for _, b in downs[:2]] == ["MouseButton(0)", "MouseButton(0)"]
            and int(opened[0][0]) <= downs[0][0] < downs[1][0] < load_events[0][0] < int(bodies[0][6])
            and downs[2:] == [(frame, "Keyboard(13)") for frame in dispatched],
            "cold load entry or adjacent body chronology mismatch")
    previous_frame = load_events[0][0]
    previous_epoch = 0
    for frame, row, point in zip(frames, bodies, points):
        context, load, body, epoch, source_frame = map(int, row[2:])
        matching_read = any((c, l, g, r, size) == (context, load, point["group"], body, point["body_bytes"])
                            and load_events[0][0] <= read <= source_frame
                            for c, l, g, r, read, size in reads)
        require(previous_frame < source_frame <= frame and previous_epoch < epoch and matching_read,
                f"cold load lacks the actual CST origin for sc02 r{point['wait']}")
        previous_frame, previous_epoch = frame, epoch
        if prefix := point.get("prefix"):
            prefix_record = prefix["record"]
            require(f"replay frame={frame} kanmi_ordinary_load_body_prefix record={prefix_record}" in lines
                    and any((c, l, g, r, size) == (context, load, point["group"], prefix_record, prefix["bytes"])
                            and load_events[0][0] <= read <= source_frame for c, l, g, r, read, size in reads),
                    f"cold load lacks the r{prefix_record} paragraph in the r{point['wait']} composition")
    require([(r["frame"], r["key"], r["group"], r["physical"]) for r in receipts
             if r["pressed"] and r["response"] == 0 and r["physical"] == ENTER]
            == [(frame + 1, 2, 0, ENTER) for frame in dispatched]
            and lines.count("replay_status=completed") == 1
            and not any(" fes-datasave " in line for line in lines),
            "cold load lacked unique Enter receipts or rewrote a slot")
    opening = {}
    if through_op:
        arrivals = [m for line in lines if (m := OP_ARRIVED.fullmatch(line))]
        require(len(arrivals) == 1, "missing final sc03 entry proof")
        stop, branch, branch_at, movie = arrivals[0].groups()
        landed, imported, natural = int(stop), int(branch_at), int(movie)
        branch_loaded = re.compile(rf"frame={imported} kanmi-op-branch-load scene={branch} "
                                   rf"handle=\d+ load_id=[1-9]\d* sha256={OP_BRANCH_DIGESTS[branch]}")
        film_ended = re.compile(rf"frame={natural} kanmi-film-end handle=\d+ playback=\d+ natural=true")
        sc03 = re.compile(r"frame=(\d+) kanmi-scene-load handle=\d+ load_id=[1-9]\d* "
                          r"scene=sc03 sha256=68952bbbf0273ba4f1a1d7b77a38304fa65ce71a536d33d140393c22a996c209")
        destinations = [int(m[1]) for line in lines if (m := sc03.fullmatch(line))]
        require(advanced < imported <= natural < landed and len(destinations) == 1
                and natural <= destinations[0] < landed
                and sum(bool(branch_loaded.fullmatch(line)) for line in lines) == 1
                and sum(bool(film_ended.fullmatch(line)) for line in lines) == 1,
                "branch identity, natural OP end, or actual sc03 load was not verified")
        opening = {"opening": {"branch": branch, "branch_frame": imported, "natural_movie_frame": natural,
                               "scene": "sc03", "group": 0, "record": 7, "frame": landed}}
    return {"run": str(run), "sha256": save["sha256"], "restored_frame": restored, "advanced_frame": advanced,
            "scene": "sc02", "restored_record": 147, "advanced_record": terminal, "pages": len(frames),
            "consumer": identity, **opening}


def verify(host, lines, points, terminal, save_profile=None):
    _, receipts, downs = read_host(host)
    events = collect_events(lines, "sc01")
    pages = [(int(m[1]), m[2], int(m[3]), int(m[4])) for line in lines if (m := ROUTE_PAGE.fullmatch(line))]
    require(events["replay_status"] == ["completed"] and len(pages) == len(points),
            "expected every confirmed wait and a completed route")
    save_downs = downs[len(points):]
    downs = downs[:len(points)]
    require([button for _, button in downs] == ["MouseButton(0)"] + ["Keyboard(13)"] * (len(points) - 1)
            and (not save_downs or save_profile is not None),
            "expected one New Game click and one Enter per nonterminal body")
    require([page[1:] for page in pages] == [(p.get("scene", "sc01"), p["group"], p["wait"]) for p in points],
            "unexpected scene/group/record sequence")
    frames = [page[0] for page in pages]
    mouse = downs[0][0] + 1
    dispatches = [frame for frame, _ in downs[1:]]
    require(dispatches == frames[:-1] and all(b > a + 3 for a, b in zip(frames, frames[1:])),
            "body inputs were not issued from adjacent completed waits after release")
    require(lines.count(f'replay frame={frames[-1]} checkpoint="{terminal}"') == 1 and mouse < frames[0],
            "missing terminal stop or opening click preceded by body")

    consumed = [entry for entry in receipts if entry["pressed"] and entry["response"] == 0
                and (save_profile is None or entry["frame"] <= frames[-1])]
    require(len(consumed) == len(points), "unexpected number of consuming physical receipts")
    fields = ("frame", "key", "group", "raw_mask", "pressed", "repeated", "region", "receiver", "physical")
    expected = [(mouse, 0, 0, 2, 1, False, 14, 8, LEFT)]
    expected.extend((frame + 1, 2, 0, 1, 1, False, 60, 8, ENTER) for frame in dispatches)
    require([tuple(entry[field] for field in fields) for entry in consumed] == expected,
            "title click or Enter reached a different receiver")
    require([int(m[1]) for line in host if (m := KANMI_TITLE_PRESS.fullmatch(line))] == [mouse],
            "missing guarded New Game receipt")
    require([tuple(map(int, m.groups())) for line in host if (m := KANMI_ENTER_TARGET.fullmatch(line))]
            == [(frame, 60, 8, 60, 8) for frame in dispatches], "live Enter target changed")
    require([(at, packet) for at, packet in events["resume"] if at >= mouse and (save_profile is None or at <= frames[-1])]
            == [(mouse, KANMI_TITLE_RESUME), *((frame + 1, ENTER_RESUME) for frame in dispatches)],
            "VM resume does not match the physical consumption frames")
    require(all((frame, 0xCAE) in events["root_wait"] for frame in
                (*frames, *(at + 1 for at in dispatches))), "missing root wait at 0xcae")

    bodies = list(dict.fromkeys(tuple(map(int, match.groups())) for line in lines if (match := BODY.fullmatch(line))))
    require(len(bodies) == len(points), "expected one current body identity at each wait")
    require(len({body[1:4] for body in bodies}) == 1, "body owner or input target changed")
    sources = {}
    for line in lines:
        at, sep, event = line.partition(" ")
        if sep and at.startswith("frame=") and (match := KANMI_SOURCE.fullmatch(event) or SC02_SOURCE.fullmatch(event)):
            context, load, group, record, read, length = map(int, match.groups())
            if at == f"frame={read}":
                scene = "sc02" if match.re is SC02_SOURCE else "sc01"
                sources.setdefault((scene, context, load, group, record, length), []).append(read)
    verified = []
    preceding_input = mouse - 1
    preceding_epoch = 0
    for at, point, body in zip(frames, points, bodies):
        frame, owner, region, receiver, context, load, record, epoch, source_frame = body
        scene = point.get("scene", "sc01")
        require((frame, region, receiver, record) == (at, 60, 8, point["body"])
                and preceding_input < source_frame <= frame and preceding_epoch < epoch,
                f"r{point['wait']} has a stale or wrong committed body")
        if verified and scene == verified[-1]["scene"]:
            require((context, load) == (verified[-1]["context"], verified[-1]["load"]),
                    f"{scene} r{point['wait']}: CST load changed within a scene")
        reads = [read for read in sources.get((scene, context, load, point["group"], record, point["body_bytes"]), ())
                 if mouse < read <= source_frame]
        require(reads, f"r{record} lacks a same-load native 327 source read of the expected length")
        verified.append({"frame": frame, "scene": scene, "group": point["group"], "wait": point["wait"], "body": record,
                         "owner": owner, "receiver": receiver, "context": context, "load": load, "epoch": epoch,
                         "source_frame": source_frame, "read_frame": min(reads),
                         "body_bytes": point["body_bytes"]})
        if prefix := point.get("prefix"):
            matches = [m[2] for line in lines if (m := PREFIX.fullmatch(line)) and int(m[1]) == at]
            require(len(matches) == 1, "missing composed r562 prefix observation")
            key = (scene, context, load, point["group"], prefix["record"], prefix["bytes"])
            prefix_reads = [read for read in sources.get(key, ()) if mouse < read <= source_frame]
            require(prefix_reads, "r562 lacks the preceding native 327 r555 read")
            verified[-1]["prefix_read_frame"] = min(prefix_reads)
            verified[-1]["composed_records"] = [prefix["record"], record] if matches[0] != "None" else [record]
        preceding_input, preceding_epoch = at, epoch
    result = {"status": "continuation_verified", "pages": verified, "receipts": consumed}
    opening_pages = {page["wait"]: page for page in verified if page["scene"] == "sc01"}
    if 78 in opening_pages:
        result["transition"] = verify_bus_transition(lines, opening_pages[53]["frame"], opening_pages[78]["frame"])
    if 983 in opening_pages:
        raster = verify_raster_transition(lines, opening_pages[971], opening_pages[983])
        result["raster_transition"] = raster
        if 1029 in opening_pages:
            result["raster_reset"] = verify_raster_reset(lines, opening_pages[1004], opening_pages[1029], raster)
    if verified[-1]["scene"] == "sc02":
        first_sc02 = next(i for i, page in enumerate(verified) if page["scene"] == "sc02")
        result["scene_handoff"] = verify_sc02_load(lines, verified[first_sc02 - 1], verified[first_sc02])
    if save_profile is not None:
        result["ordinary_save"] = verify_ordinary_write(lines, save_downs, receipts, verified[-1], save_profile)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("run", type=Path)
    parser.add_argument("--mode", choices=(*CONFIGS, *KANMI_MODES), default="r47")
    parser.add_argument("--reopen", type=Path, help="separate cold-load run for the verified ordinary slot")
    parser.add_argument("--load-segment", type=Path, metavar="WRITE_RECEIPT",
                        help="verify the isolated sc02 r147 continuation against a verified slot")
    parser.add_argument("--load-to", type=int, choices=tuple(KANMI_LOAD_MODES), default=562)
    args = parser.parse_args()
    try:
        summary = json.loads((args.run / "09.json").read_text(encoding="utf-8"))
        if args.load_segment is not None:
            producer = json.loads(args.load_segment.read_text(encoding="utf-8"))
            require(producer["status"] == "continuation_verified" and producer["sample"] == "09",
                    "missing a verified 09 ordinary save write")
            result = verify_ordinary_reopen(args.run, None, summary["game"], producer["ordinary_save"], args.load_to)
            print(json.dumps({"sample": "09", "status": "checkpoint_pass", "producer": producer["build"],
                              **result}, indent=2))
            return 0
        if summary.get("status") == "resource_deferred":
            print(json.dumps({"status": "resource_deferred"}))
            return 2
        if summary["status"] == "scenario_error":
            diagnostic = (args.run / "09" / "09.host.log").read_text(encoding="utf-8")
            if R43_ONLY_ERROR in diagnostic:
                raise ValueError("player only accepts the r43 contract; apply the continuation patches before replay")
            if args.mode != "r47" and ADJACENT_CONTRACT_ERROR in diagnostic:
                raise ValueError(f"player rejected the {args.mode} route before New Game; check its patch and route steps")
        config = (kanmi_config(Path(summary["game"]), args.mode) if args.mode in KANMI_MODES
                  else json.loads(CONFIGS[args.mode].read_text(encoding="utf-8")))
        expected_route = config["9"]["route"]
        route_file = Path(summary["route_json"]).resolve()
        require(route_file.is_relative_to(args.run.resolve()), "summary references an external route")
        route = json.loads(route_file.read_text(encoding="utf-8"))
        require(route.get("kanmi_exact") == expected_route["kanmi_exact"]
                and route.get("steps") == expected_route["steps"],
                f"expected the {args.mode} route contract, steps and resource fingerprints")
        identity, host, lines = read_run(args.run, "09")
        profile = Path(summary["profile"]).resolve() if args.mode == "sc02_r147_save" else None
        require(profile is None or profile.is_relative_to(args.run.resolve()), "summary references an external save profile")
        result = verify(host, lines, config["checkpoints"], expected_route["steps"][-1]["label"], profile)
        if args.reopen is not None:
            require(profile is not None, "--reopen requires sc02_r147_save mode")
            result["ordinary_reopen"] = verify_ordinary_reopen(args.reopen, identity, summary["game"], result["ordinary_save"])
    except (OSError, ValueError, KeyError, TypeError) as error:
        print(json.dumps({"status": "unverified", "reason": str(error)}))
        return 1
    print(json.dumps({"sample": "09", "build": identity, **result}, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

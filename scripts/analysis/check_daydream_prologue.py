"""Verify sample 17 state-replay logs and report the last proven body page."""

import argparse
from collections import defaultdict
import json
from pathlib import Path
import re

from generate_daydream_prologue_exit import (
    CASE_NEXT, CASE_NEXT_LABEL, CASE_STOP_LABEL, CASE_TITLE_LABEL, TAIL_CHECKPOINTS, TITLE_LABEL,
)
from generate_daydream_r96_route import PLAN, PREFIX_WAITS, check_plan


PE = "1f3288d5fd5408bee4d395faab708aca78c9eeaa6c172dc06dcce09c844ba9d5"
QUIET = "focused=true paused=false message_wait=Some(3437) vm_waiting_for_message=true pending_messages=0 movies=0"
PROBE = re.compile(r"frame (\d+): probe wait (.+)")
POSITION = re.compile(r'replay frame=(\d+) state=(page|page_probe) scene=(\S+) group=(\d+) record=(\d+)')
DOWN = re.compile(r'frame (\d+): down (.+)')
ROUTES = re.compile(r'frame (\d+): pointer .*?, routes \[(.*)\]')
RECEIPT = re.compile(
    r'InputRouteReceipt \{ frame: (\d+), key: (\d+), group: (\d+), raw_mask: (\d+), '
    r'pressed: (\d+), repeated: (\w+), region: (\d+), receiver: (\d+), response: (\d+), '
    r'physical: \[(.*?)\] \}'
)
SOURCE = re.compile(
    r'CstBodySource \{ context_handle: (\d+), load_id: (\d+), group: (\d+), absolute_record: (\d+) \}'
)
BODY = re.compile(
    r'body_page\(context,sources,exact_match,composited\)=Some\(\((\d+), Some\(\[(.*?)\]\), (\w+), (\w+)\)\)'
)
GENERATION = re.compile(
    r'body_generation\(owner,generation,current_committed\)=Some\(\((\d+), Some\(StoryBodyGeneration '
    r'\{ source_epoch: (\d+), source_frame: (\d+), natural_complete: Some\(\((\d+), (\d+)\)\), '
    r'committed_epoch: Some\((\d+)\), remaining: 0, remaining_segment: 0, pending_text: false, '
    r'drawing: false, skip_pending: false, forced: false, projection: Some\(\(true, ([1-9]\d*)\)\) \}\), true\)\)'
)
SHELL = re.compile(
    r'body_wait_shell\(window,owner,scene,loaded,marked,group,record\)=Some\(\("cinema", (\d+), '
    r'"prologue", (\d+), (\d+), (\d+), (\d+)\)\)'
)
CONTEXT = re.compile(r'contexts\(id,scene,loaded,candidate,page\)=\[\((\d+), "prologue", (\d+), None, Some\(\((\d+), 2, (\d+), (\d+)\)\)\)\]')
EMPTY = re.compile(r'daydream_empty_body\(context,body,raw_bytes,empty,raw_bytes\)=Some\(\((\d+), (\d+), (\d+), (\d+), (\d+)\)\)')
EVENT = re.compile(r'frame=(\d+) (.*)')
ENTER = "RoutedPhysicalButton { source: Keyboard, code: 13 }"
TITLE_WAIT = re.compile(
    r'frame (\d+): Daydream post-prologue title wait flow=(\d+) ttlflow=(\d+) title=(\d+) button=btn1\[0\] region=(\d+)'
)
CASE_LOAD = re.compile(r'frame=(\d+) native-call .* debug="sscript loaded > (\w+)"')
DEFAULT_WINDOW = re.compile(
    r'frame (\d+): Daydream r238 default meswnd owner=(\d+) parent=(\d+) context=(\d+) load=(\d+) view_frame=(\d+) wait_key=false'
)
TITLE_PROBE = re.compile(
    r'frame (\d+): Daydream title probe title=(\d+) region=(\d+) rectangle=(\[.*?\]) '
    r'point=(\[.*?\]) masked=Some\((\[.*?\])\) candidates=(\[.*?\])'
)
TITLE_LEFT = re.compile(r'frame (\d+): Daydream verified title left press (InputRouteReceipt \{.*\})')
CASE_BODY = re.compile(
    r'frame (\d+): Daydream first CASE scene=(\w+) group=0 record=(\d+) window=(\w+) owner=(\d+) '
    r'context=(\d+) load=(\d+) loaded=(\d+) marked=(\d+) sources=(\[.*?\]) epoch=(\d+) '
    r'source_frame=(\d+) natural_complete=(\d+) composed=true'
)
CASE_NEXT_BODY = re.compile(CASE_BODY.pattern.replace("first CASE", "second CASE").replace("group=0", "group=1"))
CASE_ARMED = re.compile(r"frame (\d+): Daydream CASE Enter scene=(\w+) owner=(\d+) region=(\d+) receiver=(\d+)")
CASE_ENTER = re.compile(r"frame (\d+): Daydream verified CASE Enter and VM resume (InputRouteReceipt \{.*\})")
CASE_ENDS = {"c01_01_01": (66, "cinema", [65]), "c02_01_01": (66, "meswnd", [64, 65]),
             "c03_01_01": (62, "cinema", [60, 61])}
MOUSE_LEFT = "RoutedPhysicalButton { source: Mouse, code: 1 }"
SCRIPT_COMMAND = re.compile(r'frame=(\d+) native-call .* debug="\[command\] ([^"]+)"')


def require(passed, reason):
    if not passed:
        raise ValueError(reason)


def host_evidence(path):
    probes, routes, presses, verified, armed = defaultdict(list), defaultdict(list), [], defaultdict(list), defaultdict(list)
    with path.open(encoding="utf-8", errors="replace") as lines:
        for number, raw in enumerate(lines, 1):
            line = raw.rstrip("\r\n")
            if match := PROBE.fullmatch(line):
                probes[int(match[1])].append((number, match[2]))
            elif match := DOWN.fullmatch(line):
                presses.append((int(match[1]), match[2]))
            elif match := ROUTES.fullmatch(line):
                routes[int(match[1])].extend(RECEIPT.findall(match[2]))
            elif match := re.fullmatch(r'replay frame=(\d+): verified keyboard press and VM resume .+: (InputRouteReceipt \{.*\})', line):
                verified[int(match[1])].extend(RECEIPT.findall(match[2]))
            elif match := re.fullmatch(r'frame (\d+): guarded Enter window=Some\(\("cinema", (\d+)\)\) region=62 receiver=Some\(8\)', line):
                armed[int(match[1])].append(int(match[2]))
    return probes, routes, presses, verified, armed


def event_evidence(path):
    positions, statuses, loads = [], [], []
    inputs, root_waits = set(), set()
    resumes, checkpoints = defaultdict(list), defaultdict(list)
    with path.open(encoding="utf-8", errors="replace") as lines:
        for number, raw in enumerate(lines, 1):
            line = raw.rstrip("\r\n")
            if match := POSITION.fullmatch(line):
                positions.append((int(match[1]), match[2], match[3], int(match[4]), int(match[5])))
            elif line.startswith("replay_status="):
                statuses.append(line.partition("=")[2])
            elif match := re.fullmatch(r'replay frame=(\d+) checkpoint=".*"', line):
                checkpoints[int(match[1])].append(number)
            elif match := EVENT.fullmatch(line):
                frame, event = int(match[1]), match[2]
                if event.startswith("message-resume "):
                    resumes[frame].append((number, event))
                elif event.endswith('debug="[input]"'):
                    inputs.add(frame)
                elif event == "input-wait native 71 address=0xd6d":
                    root_waits.add(frame)
                elif 'debug="sscript loaded > ' in event:
                    loads.append((frame, event.endswith('debug="sscript loaded > prologue"')))
    return positions, statuses, loads, inputs, root_waits, resumes, checkpoints


def verify_empty(snapshot, context, empty):
    _, body, lengths, record, _ = empty
    observed = EMPTY.search(snapshot)
    require(observed is not None, f"missing r{record} CST length diagnostic")
    actual_context, actual_body, body_len, actual_record, empty_len = map(int, observed.groups())
    require((actual_context, actual_body, actual_record, empty_len) == (context, body, record, 0)
            and body_len in lengths, f"r{record} is not the expected zero-byte CST record")
    return {"record": record, "raw_bytes": empty_len, "preceding_body_raw_bytes": body_len,
            "fes_queue_operation": "unobserved"}


def verify_page(position, probe, body_records, empty, inputs, root_waits):
    frame, state, scene, group, record = position
    line, snapshot = probe
    at = f"g{group}/r{record} f{frame} host line {line}"
    require(f'trace_enabled=true profile=Some(Daydream) pe_sha256=Some("{PE}") startup=Some("kcs.int/main.kcs") '
            f'frame={frame} state={state} scene={scene} group={group} record={record} {QUIET}' in snapshot,
            f"{at}: profile or root readiness mismatch")
    shell, context_row = SHELL.search(snapshot), CONTEXT.search(snapshot)
    require(shell is not None and context_row is not None, f"{at}: missing live cinema shell or unique CST context")
    owner, loaded, marked, shell_group, shell_record = map(int, shell.groups())
    context, ctx_loaded, ctx_marked, ctx_group, ctx_record = map(int, context_row.groups())
    require((shell_group, shell_record) == (group, record)
            and (ctx_loaded, ctx_marked, ctx_group, ctx_record) == (loaded, marked, group, record)
            and loaded <= marked < frame and marked in inputs and frame in root_waits,
            f"{at}: missing exact 0x02 marker and matching native wait evidence")
    windows = snapshot.partition(" windows(")[2].partition(" body_wait_shell(")[0]
    require(windows.count('Some(("') == 2
            and re.search(rf'\(\d+, "flow", false, Some\(\("GAME_WAIT", false, Some\(Frame \{{ after: {frame} \}}\)', windows)
            and re.search(rf'\({owner}, "cinema(?:\.fes)?", true, Some\(\("MES_SHOW", false, Some\(Frame \{{ after: {frame} \}}\)', windows),
            f"{at}: missing settled GAME_WAIT and cinema MES_SHOW/WAIT_KEY")
    empty_result = verify_empty(snapshot, context, empty) if empty else None
    body, generation = BODY.search(snapshot), GENERATION.search(snapshot)
    require(body is not None and generation is not None and body.groups()[2:] == ("true", "true"),
            f"{at}: wait observed but current natural commit and composition are unproven")
    body_context, source_text = int(body[1]), body[2]
    sources = [tuple(map(int, fields)) for fields in SOURCE.findall(source_text)]
    require(bool(sources) and SOURCE.sub("", source_text) == ", " * (len(sources) - 1), f"{at}: invalid source list")
    load = sources[0][1]
    require(load > 0 and body_context == context
            and sources == [(context, load, group, source) for source in body_records],
            f"{at}: missing exact ordered visible sources {body_records}")
    generation_owner, epoch, source_frame, complete_epoch, complete_frame, commit_epoch, _ = map(int, generation.groups())
    require(generation_owner == owner and epoch > 0 and epoch == complete_epoch == commit_epoch
            and loaded <= source_frame <= complete_frame <= frame,
            f"{at}: stale or forced generation")
    return {"group": group, "wait_record": record, "frame": frame, "host_line": line, "owner": owner,
            "context": context, "load_id": load, "loaded_frame": loaded, "wait_marked_frame": marked,
            "body_records": body_records, "source_frame": source_frame, "epoch": epoch,
            "natural_complete_frame": complete_frame, "empty_body": empty_result}


def verify_enter(before, position, routes, verified, armed, presses, resumes, root_waits):
    start = before["frame"]
    end, _, _, _, target = position
    at = f"r{before['wait_record']} -> r{target}"
    require([(frame, key) for frame, key in presses if start <= frame < end] == [(start, "Keyboard(13)")]
            and end >= start + 4 and armed[start] == [before["owner"]], f"{at}: missing exact guarded dispatch or completed release frame")
    receipt_frame = start + 1
    expected = (str(receipt_frame), "2", "0", "1", "1", "false", "62", "8", "0", ENTER)
    consumers = [receipt for frame in range(start + 1, end + 1) for receipt in routes.get(frame, ())
                 if int(receipt[4]) > 0 and receipt[8] == "0"]
    require(consumers == [expected] and verified[receipt_frame] == [expected], f"{at}: missing unique Enter consumer")
    outputs = [(frame, number, message) for frame in range(start + 1, end + 1) for number, message in resumes.get(frame, ())]
    require(len(outputs) == 1 and (outputs[0][0], outputs[0][2]) == (receipt_frame, "message-resume message=[2, 0, 0, 0]")
            and receipt_frame in root_waits, f"{at}: missing unique same-frame root VM resume")
    return {"from": before["wait_record"], "to": target, "down_frame": start,
            "receipt_frame": receipt_frame, "resume_event_line": outputs[0][1], "region": 62, "receiver": 8}


def verify_successor(before, after, receipt_frame):
    require(all(before[key] == after[key] for key in ("context", "load_id", "loaded_frame"))
            and receipt_frame <= after["wait_marked_frame"] and receipt_frame <= after["source_frame"]
            and (before["owner"] != after["owner"] or before["epoch"] != after["epoch"]),
            f"r{after['wait_record']}: successor lacks same-load fresh body evidence")


def verify_title(run, end, before, routes, presses, verified, armed, resumes, root_waits, checkpoints, report,
                 case=False):
    start = before["frame"]
    require(end >= start + 4, "title appeared before the final Enter release could settle")
    receipt = verify_enter(before, (start + 4, "title", "-", -1, "title"), routes,
                           verified, armed, presses, resumes, root_waits)
    require(all(frame <= start or int(press[4]) == 0 or press[8] != "0"
                for frame, presses_at_frame in routes.items() for press in presses_at_frame
                if start < frame <= end and frame != receipt["receipt_frame"]),
            "unexpected consumed input during title transition")
    title_lines = []
    with (run / "17.host.log").open(encoding="utf-8", errors="replace") as stream:
        for line, raw in enumerate(stream, 1):
            if match := TITLE_WAIT.fullmatch(raw.rstrip("\r\n")):
                title_lines.append((line, *map(int, match.groups())))
    require(len(title_lines) == 1, "missing unique post-prologue title wait receipt")
    host_line, frame, flow, ttlflow, title, region = title_lines[0]
    require(frame == end and min(flow, ttlflow, title, region) > 0 and len({flow, ttlflow, title, region}) == 4
            and title != before["owner"] and len(checkpoints[end]) == 1,
            "title terminal frame or live owners did not match")
    tail = []
    title_checkpoint = False
    title_label = CASE_TITLE_LABEL if case else TITLE_LABEL
    with (run / "17.events.log").open(encoding="utf-8", errors="replace") as stream:
        for line, raw in enumerate(stream, 1):
            event = raw.rstrip("\r\n")
            if event == f'replay frame={end} checkpoint="{title_label}"':
                title_checkpoint = True
            match = SCRIPT_COMMAND.fullmatch(event)
            if match and len(tail) < len(TAIL_CHECKPOINTS) and start < int(match[1]) < end:
                record, command = TAIL_CHECKPOINTS[len(tail)]
                if match[2].strip() == command:
                    tail.append({"record": record, "command": command, "frame": int(match[1]), "event_line": line})
    require(title_checkpoint, "missing post-prologue title checkpoint")
    require(len(tail) == len(TAIL_CHECKPOINTS), "missing ordered prologue tail command checkpoints")
    report["verified_enters"].append(receipt)
    report["terminal_title"] = {"frame": end, "host_line": host_line, "flow_owner": flow,
                                "ttlflow_owner": ttlflow, "title_owner": title, "region": region, "title_inputs": int(case)}
    report["prologue_tail"] = tail


def case_evidence(run, advance):
    patterns = {"default": DEFAULT_WINDOW, "title_probe": TITLE_PROBE, "left": TITLE_LEFT, "body": CASE_BODY,
                "title": TITLE_WAIT}
    if advance:
        patterns.update(next_body=CASE_NEXT_BODY, armed=CASE_ARMED, enter=CASE_ENTER)
    evidence = defaultdict(list)
    with (run / "17.host.log").open(encoding="utf-8", errors="replace") as stream:
        for number, line in enumerate(stream, 1):
            for key, pattern in patterns.items():
                if match := pattern.fullmatch(line.rstrip("\r\n")):
                    evidence[key].append((number, match.groups()))
                    break
    require(all(len(evidence[key]) == 1 for key in patterns), "missing unique Daydream CASE probe or receipt")
    return {key: rows[0] for key, rows in evidence.items()}


def verify_case(run, end, evidence, routes, checkpoints, report, advance):
    title = report["terminal_title"]
    before = report["last_verified_page"]
    default_line, default = evidence["default"]
    restored, owner, parent, context, load, view = map(int, default)
    require((parent, context, load) == (8, before["context"], before["load_id"])
            and owner > 0 and owner != before["owner"]
            and before["frame"] < view <= restored < title["frame"],
            "default meswnd creation did not match the r221 transition")

    probe_line, probe = evidence["title_probe"]
    require(tuple(map(int, probe[:3])) == (title["frame"], title["title_owner"], title["region"]),
            "title hit probe did not match the live title binding")
    rectangle, point, masked, candidates = map(json.loads, probe[3:])
    require(len(rectangle) == 4 and len(point) == 2 and len(masked) == 15
            and rectangle[0] <= point[0] < rectangle[2] and rectangle[1] <= point[1] < rectangle[3]
            and masked[0] == title["region"] and masked[8] != 0 and masked[9] != 0
            and title["region"] in candidates,
            "the title pointer did not hit the enabled btn1[0] region")

    left_line, (left_frame, left_text) = evidence["left"]
    left = RECEIPT.fullmatch(left_text)
    require(left is not None and left[1] == left_frame and title["frame"] < int(left_frame) <= title["frame"] + 3
            and left.groups()[1:6] == ("0", "0", "2", "1", "false")
            and tuple(map(int, left.groups()[6:8])) == (title["region"], title["title_owner"])
            and left[9] in ("0", "1") and left[10] == MOUSE_LEFT,
            "title press did not reach the bound FES owner")
    first_frame = int(evidence["body"][1][0])
    require(left.groups() in routes[int(left_frame)]
            and [p for f, rows in routes.items() if title["frame"] < f <= first_frame for p in rows
                 if int(p[4]) > 0 and (p[8] == "0" or p == left.groups())] == [left.groups()],
            "missing or competing physical title consumer")

    body_line, body = evidence["body"]
    frame, scene, record, window, owner, context, load, loaded, marked, sources, epoch, published, complete = body
    frame, record, owner, context, load, loaded, marked, epoch, published, complete = map(
        int, (frame, record, owner, context, load, loaded, marked, epoch, published, complete))
    require((frame < end if advance else frame == end) and CASE_ENDS.get(scene) == (record, window, json.loads(sources))
            and min(owner, context, load, epoch) > 0 and load != before["load_id"]
            and int(left_frame) <= loaded <= min(marked, published) and marked < frame
            and published <= complete <= frame and frame >= title["frame"] + 4,
            "terminal CASE body was not the fresh, composed first wait")

    terminal = False
    terminal_label = CASE_NEXT_LABEL if advance else CASE_STOP_LABEL
    loads = []
    created = False
    view_event = False
    with (run / "17.events.log").open(encoding="utf-8", errors="replace") as stream:
        for line in stream:
            line = line.rstrip("\r\n")
            if line == f'replay frame={end} checkpoint="{terminal_label}"':
                terminal = True
            if line.startswith(f'frame={restored} task-create id={evidence["default"][1][1]} kind=34 parent=8 name="meswnd" '):
                created = True
            if line.startswith(f'frame={view} native-call index=1040 ') and line.endswith('debug="[command] view 0 default"'):
                view_event = True
            if match := CASE_LOAD.fullmatch(line):
                if title["frame"] < int(match[1]) <= end:
                    loads.append((int(match[1]), match[2]))
            if '[command] fselect' in line and (match := EVENT.fullmatch(line)):
                require(not title["frame"] < int(match[1]) <= end, "a CST choice appeared before the CASE body")
    require(created and view_event, "missing live default-window creation or view command")
    require(bool(loads) and loads[0][0] >= int(left_frame)
            and [name for _, name in loads] == ["start", "c00_00", "brunch", "brunch_A", scene[:6], scene]
            and terminal and len(checkpoints[end]) == 1,
            "missing ordered title-to-CASE loads or final input-free checkpoint")
    report["r238_default_window"] = {"frame": restored, "host_line": default_line, "view_frame": view}
    report["title_left"] = {"probe_line": probe_line, "host_line": left_line, "point": point,
                            "receipt_frame": int(left_frame), "region": title["region"], "owner": title["title_owner"]}
    first = {"frame": frame, "host_line": body_line, "scene": scene, "group": 0, "record": record,
             "window": window, "owner": owner, "context": context, "load_id": load,
             "sources": json.loads(sources), "natural_complete": complete, "body_inputs": int(advance)}
    if advance:
        report["first_case"] = first | {"loaded_frame": loaded, "epoch": epoch}
    else:
        report["terminal_case"] = first


def verify_case_successor(end, evidence, routes, root_waits, resumes, report):
    first = report["first_case"]
    armed_line, (down, scene, owner, region, receiver) = evidence["armed"]
    start = int(down)
    require((start, scene, int(owner)) == (first["frame"], first["scene"], first["owner"])
            and min(int(region), int(receiver)) > 0 and len({owner, region, receiver}) == 3,
            "CASE Enter was not armed against the first body and a live root button")
    receipt_frame = start + 1
    expected = (str(receipt_frame), "2", "0", "1", "1", "false", region, receiver, "0", ENTER)
    receipt_line, (frame, receipt) = evidence["enter"]
    require(int(frame) == receipt_frame and RECEIPT.findall(receipt) == [expected]
            and [row for f, rows in routes.items() if start < f <= end for row in rows
                 if int(row[4]) > 0 and row[8] == "0"] == [expected],
            "CASE Enter did not have exactly one physical consumer on the bound button")
    require(receipt_frame in root_waits and [event for _, event in resumes[receipt_frame]]
            == ["message-resume message=[2, 0, 0, 0]"], "CASE Enter did not resume the root VM on the same frame")

    body_line, row = evidence["next_body"]
    frame, scene, record, window, owner, context, load, loaded, marked, sources, epoch, published, complete = row
    frame, record, owner, context, load, loaded, marked, epoch, published, complete = map(
        int, (frame, record, owner, context, load, loaded, marked, epoch, published, complete))
    before, after, expected_window, expected_sources, _ = CASE_NEXT[first["scene"]]
    require((first["record"], record, window, json.loads(sources))
            == (before, after, expected_window, expected_sources)
            and (scene, owner, context, load, loaded) == (first["scene"], first["owner"], first["context"],
                                                           first["load_id"], first["loaded_frame"])
            and epoch > first["epoch"] and receipt_frame <= min(marked, published)
            and marked < end and published <= complete <= end and frame == end and end >= start + 4,
            "CASE successor is not the adjacent composed body from the same live window and CST load")
    report["case_enter"] = {"armed_line": armed_line, "host_line": receipt_line, "receipt_frame": receipt_frame,
                            "region": int(region), "receiver": int(receiver)}
    report["terminal_case"] = {"frame": frame, "host_line": body_line, "scene": scene, "group": 1, "record": record,
                               "window": window, "owner": owner, "context": context, "load_id": load,
                               "sources": json.loads(sources), "natural_complete": complete, "body_inputs": 0}


def verify_run(result_path, target, report):
    result = json.loads(result_path.read_text(encoding="utf-8"))
    require((result["index"], result["mode"], result["status"], result["replay_status"], result["exit_code"],
             result["timed_out"], result["first_engine_error"]) == (17, "state", "completed", "completed", 0, False, None),
            "batch did not complete a successful sample 17 state replay")
    run = result_path.parent / "17"
    probes, routes, presses, verified, armed = host_evidence(run / "17.host.log")
    positions, statuses, loads, inputs, root_waits, resumes, checkpoints = event_evidence(run / "17.events.log")
    plan = json.loads(PLAN.read_text(encoding="utf-8"))
    check_plan(plan)
    tail = plan["static_successors"]
    all_waits = list(PREFIX_WAITS) + [row["wait_record"] for row in tail]
    advance = target == "case-next"
    case = target in ("case", "case-next")
    title_exit = target == "title" or case
    evidence = case_evidence(run, advance) if case else None
    case_positions = [position for position in positions if position[2] != "prologue"]
    if case:
        positions = [position for position in positions if position[2] == "prologue"]
    group = all_waits.index(221 if title_exit else target)
    require(statuses == ["completed"] and [(s, g, r) for _, _, s, g, r in positions]
            == [("prologue", i, r) for i, r in enumerate(all_waits[:group + 1])],
            "replay did not visit the exact ordered prologue checkpoints")
    end = result["last_frame"]
    input_positions = positions if title_exit else positions[:-1]
    expected_presses = [(frame, "Keyboard(13)") for frame, _, _, _, _ in input_positions]
    title_frame = int(evidence["title"][1][0]) if case else end
    if case:
        expected_presses.append((title_frame, "MouseButton(0)"))
    if advance:
        expected_presses.append((int(evidence["body"][1][0]), "Keyboard(13)"))
    require((positions[-1][0] < end if title_exit else positions[-1][0] == end)
            and len(checkpoints[end]) == 1
            and presses == expected_presses,
            f"r{target} was not an input-free terminal checkpoint")
    expected_bodies = {81: [80], 83: [82], **{row["wait_record"]: row["body_records"] for row in tail}}
    empty_rows = {row["wait_record"]: (row["wait_record"], row["nonempty_record"], row["nonempty_bytes_cn_jp"],
                                      row["empty_record"], row["empty_bytes_cn_jp"]) for row in plan["empty_body_observations"]}
    previous = None
    for position in positions[18:]:
        frame, state, _, _, record = position
        allowed_states = ("page",)
        if record in empty_rows:
            allowed_states = ("page_probe",) if target == 96 else ("page", "page_probe")
        require(state in allowed_states and len(probes[frame]) == 1, f"r{record} has no unique exact checkpoint probe")
        if previous:
            receipt = verify_enter(previous, position, routes, verified, armed, presses, resumes, root_waits)
            report["verified_enters"].append(receipt)
        page = verify_page(position, probes[frame][0], expected_bodies[record], empty_rows.get(record), inputs, root_waits)
        require([(f, prologue) for f, prologue in loads if f <= frame] == [(page["loaded_frame"], True)],
                f"r{record} has missing or competing CST load events")
        if previous:
            verify_successor(previous, page, receipt["receipt_frame"])
        report["last_verified_page"] = page
        previous = page
    if title_exit:
        verify_title(run, title_frame, previous, routes, presses, verified, armed, resumes, root_waits, checkpoints,
                     report, case=case)
    if case:
        verify_case(run, end, evidence, routes, checkpoints, report, advance)
        if advance:
            verify_case_successor(end, evidence, routes, root_waits, resumes, report)
        pages = [report["first_case"], report["terminal_case"]] if advance else [report["terminal_case"]]
        require(case_positions == [(page["frame"], "page", page["scene"], page["group"], page["record"])
                                   for page in pages], "missing exact live CASE body positions")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("run", type=Path, help="batch directory containing 17.json, or the 17.json itself")
    parser.add_argument("--to", choices=("83", "96", "107", "221", "title", "case", "case-next"), default="96")
    args = parser.parse_args()
    result = args.run / "17.json" if args.run.is_dir() else args.run
    target = args.to if args.to in ("title", "case", "case-next") else int(args.to)
    report = {"target": target, "result": str(result), "verified_enters": [], "last_verified_page": None}
    try:
        verify_run(result, target, report)
    except (KeyError, TypeError, ValueError, OSError) as error:
        report.update(status="rejected", reason=str(error))
    else:
        report["status"] = "verified"
    print(json.dumps(report, indent=2))
    return 0 if report["status"] == "verified" else 1


if __name__ == "__main__":
    raise SystemExit(main())

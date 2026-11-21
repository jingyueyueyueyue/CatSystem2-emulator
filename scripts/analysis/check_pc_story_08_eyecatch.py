"""Check the 08 hover control or guarded eye release using existing replay logs."""

import argparse
import json
from pathlib import Path
import re
from struct import unpack_from

from check_pc_body_successor import BODY_GENERATION


PE = "72c9c1e7335a92bc2f1c731275a8da34d638d1730b9d5003fb6a8e85fe5b15c8"
POINT = [640, 360]
TITLE_AND_CARD = [(4950, 634, 90, 13, 7), (5030, 120, 185, 32, 23)]
STORY_POLL = re.compile(
    r'^frame (\d+): wait trace_enabled=true profile=Some\(Eyecatch08Observation\) '
    rf'pe_sha256=Some\("{PE}"\) startup=Some\("kcs.int/main.kcs"\) '
    r'frame=\1 state=active scene=c01_01 group=-1 record=-1 focused=true paused=false '
    r'message_wait=Some\(\d+\) vm_waiting_for_message=true pending_messages=0 movies=0 '
    r'contexts\(id,scene,loaded,candidate,page\)=\[\((\d+), "c01_01", (\d+), '
    r'(?:Some\(\(\d+, \d+, \d+, \d+\)\)|None), None\)\] '
    r'windows\(.*?\)=\[\((\d+), "flow", false, '
    r'Some\(\("GAME_WAIT", false, Some\(Frame \{ after: \1 \}\)\)\), false, None\), '
    r'\((\d+), "eyecatch", false, Some\(\("WAIT", false, Some\(Frame \{ after: \1 \}\)\)\), '
    r'false, None\)\] .*? eyecatch_08\(window,owner\)=Some\(\("eyecatch", \5\)\)(?: |$)',
    re.MULTILINE,
)
HOVER = re.compile(
    r'^frame (\d+): pointer \(640,360\) unmasked=Some\((\[[\d, ]+\])\) '
    r'masked=Some\(\2\) press_candidates=Ok\((\[[\d, ]+\])\)$',
    re.MULTILINE,
)
TASK = re.compile(
    r'^frame=(\d+) task-create id=(\d+) kind=(\d+) parent=(\d+) name="([^"]*)"',
    re.MULTILINE,
)
DOWN = re.compile(r"^frame (\d+): down (.*)$", re.MULTILINE)
GUARDED_ENTRY = re.compile(
    r"^frame (\d+): guarded 08 entry \[(\d+), (\d+)\] region=(\d+) owner=(\d+)$", re.MULTILINE,
)
ENTRY_HOVER = re.compile(
    r"^frame (\d+): 08 entry hover \[(\d+), (\d+)\] region=(\d+) owner=(\d+)$", re.MULTILINE,
)
MOUSE_RECEIPT = re.compile(
    r"InputRouteReceipt \{ frame: (\d+), key: ([02]), group: 0, raw_mask: 2, "
    r"pressed: 1, repeated: false, region: (\d+), receiver: (\d+), response: (\d+), "
    r"physical: \[RoutedPhysicalButton \{ source: Mouse, code: 1 \}\] \}"
)
RELEASE = re.compile(r"^frame (\d+): verified 08 eye EXIT, unbind and release$", re.MULTILINE)
FIRST_BODY = re.compile(
    r"^frame=(\d+) eye08-body owner=(\d+) parent=(\d+) context=(\d+) load=([1-9]\d*) "
    r"loaded=(\d+) epoch=([1-9]\d*) source_frame=(\d+) marked=(\d+) address=(0x[0-9a-f]+)$",
    re.MULTILINE,
)
ORDINARY_BODY = re.compile(
    r"^frame=(\d+) eye08-ordinary-body group=(\d+) wait=(\d+) context=(\d+) load=(\d+) loaded=(\d+) "
    r"body=Eye08Body \{ flow: (\d+), script: (\d+), owner: (\d+), plane: (\d+), string: (\d+), "
    r"generation: (\d+), marked: (\d+), reveal: (\d+), opaque: (\d+) \}$",
    re.MULTILINE,
)
ORIGINAL_READ = re.compile(
    r"^frame=(\d+) cst-source-record CstSourceRecordObservation \{ source: CstBodySource \{ "
    r"context_handle: (\d+), load_id: (\d+), group: (\d+), absolute_record: (\d+) \}, frame: \1, "
    r"probe: Eye08, native: 327, kind: 32, stored_codec: (\w+), output_codec: \6, "
    r"raw_bytes: (\d+), output_bytes: \7, body_tagged: true, localized: false \}$",
    re.MULTILINE,
)
KEYBOARD_RECEIPT = re.compile(
    r"InputRouteReceipt \{ frame: (\d+), key: ([02]), group: 0, raw_mask: 1, pressed: 1, "
    r"repeated: false, region: (\d+), receiver: (\d+), response: (\d+), "
    r"physical: \[RoutedPhysicalButton \{ source: Keyboard, code: 13 \}\] \}",
)
SUCCESSOR = ((0, 144, 144, 143, 20), (1, 148, 3, 147, 28), (2, 150, 1, 149, 34))


def classify_hover(result, host, events):
    stopped = {"gate": "unproven", "input_authorized": False}
    if (result.get("index") != 8 or result.get("mode") != "frame" or result.get("exit_code") != 0
            or result.get("timed_out") is not False or result.get("first_engine_error") is not None
            or result.get("replay_status") not in (None, "frame_limit")):
        return stopped | {"reason": "replay failed or stopped at another gate"}

    actions = result.get("command", [])[4:]
    if [action for action in actions if re.fullmatch(r"\d+:.*", action)] != [
        f"{frame}:{x},{y}" for frame, x, y, _, _ in TITLE_AND_CARD
    ] or re.findall(r"^frame (\d+): down (.*)$", host, re.MULTILINE) != [
        (str(frame), "MouseButton(0)") for frame, _, _, _, _ in TITLE_AND_CARD
    ]:
        return stopped | {"reason": "expected only the two normal-entry mouse downs"}
    for frame, x, y, region, owner in TITLE_AND_CARD:
        if f"require-hit:{frame}:{region}" not in actions or (
            f"frame {frame + 1}: pointer [{x}, {y}], routes [InputRouteReceipt {{ "
            f"frame: {frame + 1}, key: 0, group: 0, raw_mask: 2, pressed: 1, repeated: false, "
            f"region: {region}, receiver: {owner}, response: 0, physical: "
            "[RoutedPhysicalButton { source: Mouse, code: 1 }] }]"
        ) not in host:
            return stopped | {"reason": "missing consuming normal-entry mouse receipt"}
        if re.findall(
            rf"^frame={frame + 1} message-resume (.*)$", events, re.MULTILINE
        ) != [f"message=[2, {x}, {y}, 0]"]:
            return stopped | {"reason": "missing unique normal-entry VM message resume"}
    if len(re.findall(r"\bpressed: [1-9]\d*,", host)) != len(TITLE_AND_CARD):
        return stopped | {"reason": "extra positive routing receipt"}

    polls = [tuple(map(int, match.groups())) for match in STORY_POLL.finditer(host)]
    hovers = list(HOVER.finditer(host))
    if len(polls) != 2 or len(hovers) != 1 or polls[0][1:] != polls[1][1:]:
        return stopped | {"reason": "missing matching completed-frame eye polls and a unique hover"}
    before, context, loaded, flow, eye = polls[0]
    after = polls[1][0]
    hover = hovers[0]
    move_frame, words, candidates = int(hover[1]), json.loads(hover[2]), json.loads(hover[3])
    if not TITLE_AND_CARD[-1][0] < loaded < before < move_frame < after == result.get("last_frame") or (
        f"move:{move_frame}:640,360" not in actions
    ):
        return stopped | {"reason": "hover was not bracketed by completed eye polls"}
    if (len(words) != 15 or len(candidates) != 2 or len(set(candidates)) != 2
            or words[0] != candidates[0] or not words[2] & 2 or words[3:7] != [0, 0, 800, 600]
            or words[8] == 0 or words[11] != 0):
        return stopped | {"reason": "unexpected ordered candidates or masked region"}

    loads = re.findall(r'^frame=(\d+) native-call .*debug="sscript loaded > ([^"]+)"$', events, re.MULTILINE)
    load = re.search(
        rf'^frame={loaded} native-call index=325 arguments=8 bytes \[([0-9a-f]{{16}})\]\n'
        rf'frame={loaded} native-call .*debug="sscript loaded > c01_01"$',
        events, re.MULTILINE,
    )
    if loads != [(str(loaded), "c01_01")] or load is None or (
        int.from_bytes(bytes.fromhex(load[1][:8]), "little") != context
    ):
        return stopped | {"reason": "scene instance does not match native325"}

    tasks = {}
    for frame, task, kind, parent, name in TASK.findall(events):
        if int(frame) <= after:
            tasks[int(task)] = (int(frame), int(kind), int(parent), name)
    if eye not in tasks or flow not in tasks or any(region not in tasks for region in candidates):
        return stopped | {"reason": "missing task creation evidence for ordered candidates"}
    eye_created, eye_kind, parent, eye_name = tasks[eye]
    if (not loaded <= eye_created <= before or (eye_kind, eye_name) != (34, "child fes")
            or tasks.get(parent, (0, -1, -1, ""))[1:3] != (0, flow)
            or tasks[flow][1:] != (34, 0, "main")
            or tasks[candidates[0]][1:] != (6, eye, "low button")
            or tasks[candidates[1]][1:] != (6, flow, "low button")
            or any(tasks[region][0] > before for region in candidates)):
        return stopped | {"reason": "ordered candidates do not belong to the eye and its flow ancestor"}

    return stopped | {
        "gate": "eyecatch_hover_control",
        "scene": "c01_01",
        "context": context,
        "loaded_frame": loaded,
        "completed_wait_frames": [before, after],
        "hover_frame": move_frame,
        "hover_point": POINT,
        "eye_owner": eye,
        "flow_owner": flow,
        "masked_hit": words[0],
        "ordered_candidates": candidates,
        "same_frame_arm_observed": False,
        "eye_click_observed": False,
    }


def classify_exit(result, host, events, body_presses=0):
    stopped = {"gate": "unproven", "input_authorized": False}
    if (result.get("index") != 8 or result.get("mode") != "state"
            or result.get("status") not in ("checkpoint_reached", "completed")
            or result.get("replay_status") != "completed"
            or result.get("exit_code") != 0 or result.get("timed_out") is not False
            or result.get("first_engine_error") is not None or result.get("captures") != []):
        return stopped | {"reason": f"state replay did not complete cleanly ({result.get('status')})"}
    releases = RELEASE.findall(host)
    if len(releases) != 1 or not events.rstrip().endswith("\nreplay_status=completed"):
        return stopped | {"reason": "missing terminal controller verification of EXIT, unbind and release"}
    released = int(releases[0])

    entries = [tuple(map(int, entry)) for entry in GUARDED_ENTRY.findall(host)]
    hovers = list(HOVER.finditer(host))
    eyes = re.findall(r"^frame (\d+): 08 eye owner=(\d+), moving without a press$", host, re.MULTILINE)
    all_downs = [(int(frame), button) for frame, button in DOWN.findall(host)]
    downs, body_downs = all_downs[:3], all_downs[3:]
    if len(body_downs) != body_presses or any(key != "Keyboard(13)" or frame < released for frame, key in body_downs):
        return stopped | {"reason": f"expected exactly {body_presses} Enter presses after eye release"}
    if len(entries) != 2 or len(hovers) != 1 or len(eyes) != 1 or len(downs) != 3:
        return stopped | {"reason": "expected title click, card click, one eye hover and one eye click"}
    hover_frame, eye = map(int, eyes[0])
    hover = hovers[0]
    words, candidates = json.loads(hover[2]), json.loads(hover[3])
    click = hover_frame + 1
    if (int(hover[1]) != hover_frame or released < click + 3
            or not 0 < entries[0][0] < entries[1][0] < hover_frame
            or downs != [(frame, "MouseButton(0)") for frame in (entries[0][0], entries[1][0], click)]
            or len(words) != 15 or len(candidates) != 2 or len(set(candidates)) != 2
            or words[0] != candidates[0] or not words[2] & 2 or words[3:7] != [0, 0, 800, 600]
            or words[8] == 0 or words[11] != 0):
        return stopped | {"reason": "unexpected input order or eye hover target"}

    receipts = [tuple(map(int, receipt)) for receipt in MOUSE_RECEIPT.findall(host)]
    keyboard_receipts = KEYBOARD_RECEIPT.findall(host) if body_presses else []
    if (len(receipts) + len(keyboard_receipts) != len(re.findall(r"\bpressed: [1-9]\d*,", host))
            or len(receipts) not in (3, 4, 6)):
        return stopped | {"reason": "extra or unexpected physical receipts"}
    for entry, (_, x, y, region, _), receipt in zip(entries, TITLE_AND_CARD, receipts):
        down, px, py, target, owner = entry
        frame = down + 1
        if ((px, py, target) != (x, y, region) or receipt != (frame, 0, region, owner, 0)
                or re.findall(rf"^frame={frame} message-resume (.*)$", events, re.MULTILINE)
                != [f"message=[2, {x}, {y}, 0]"]):
            return stopped | {"reason": "entry action lacks its fresh physical receipt and VM resume"}

    tasks = {}
    for frame, task, kind, parent, name in TASK.findall(events):
        if int(frame) <= hover_frame:
            tasks[int(task)] = (int(kind), int(parent), name)
    eye_region, flow_region = candidates
    flow = tasks.get(flow_region, (0, 0, ""))[1]
    eye_parent = tasks.get(eye, (0, 0, ""))[1]
    primary = [(click + 1, 0, eye_region, eye, 1), (click + 1, 0, flow_region, flow, 1)]
    aliases = [(click + 1, 2, eye_region, eye, 1), (click + 1, 2, flow_region, flow, 1)]
    eye_receipts = receipts[2:]
    if len(eye_receipts) == 4:
        routes_match = eye_receipts == primary + aliases
    else:
        routes_match = eye_receipts[0][:4] == primary[0][:4] and (
            len(eye_receipts) == 1 or (eye_receipts[0][4] != 0 and eye_receipts[1][:4] == primary[1][:4])
        )
    if (tasks.get(eye) != (34, eye_parent, "child fes")
            or tasks.get(eye_parent, (-1, -1, ""))[:2] != (0, flow)
            or tasks.get(flow) != (34, 0, "main")
            or tasks.get(eye_region) != (6, eye, "low button")
            or tasks.get(flow_region) != (6, flow, "low button")
            or not routes_match):
        return stopped | {"reason": "physical eye press chain does not match the live task ancestry"}

    loads = re.findall(r'^frame=(\d+) native-call .*debug="sscript loaded > ([^"]+)"$', events, re.MULTILINE)
    if len(loads) != 1 or loads[0][1] != "c01_01" or not entries[1][0] + 1 < int(loads[0][0]) < hover_frame:
        return stopped | {"reason": "expected a single c01_01 load between card entry and eye hover"}
    return stopped | {
        "gate": "eyecatch_release",
        "scene": "c01_01",
        "loaded_frame": int(loads[0][0]),
        "eye_owner": eye,
        "flow_owner": flow,
        "ordered_candidates": candidates,
        "press_receipt_frame": click + 1,
        "release_frame": released,
        "body_wait_verified": False,
    }


def classify_ordinary(result, host, events, eye, body_presses, expected_pages=SUCCESSOR):
    stopped = eye | {"gate": "unproven"}
    pages = [tuple(map(int, row)) for row in ORDINARY_BODY.findall(events)]
    if len(pages) != body_presses + 1 or len(expected_pages) < len(pages):
        return stopped | {"reason": "wrong number of ordinary body proof events"}
    reads = [(int(f), int(ctx), int(load), int(g), int(record), int(size))
             for f, ctx, load, g, record, _, size in ORIGINAL_READ.findall(events)]
    tasks = [tuple(map(int, row[:4])) + (row[4],) for row in TASK.findall(events)]
    for page, (group, wait, ordinal, record, size) in zip(pages, expected_pages):
        frame, got_group, got_wait, context, load, loaded, flow, script, owner, plane, string, gen, marked, reveal, opaque = page
        if ((got_group, got_wait) != (group, wait) or loaded != eye["loaded_frame"]
                or not eye["press_receipt_frame"] <= marked <= reveal < frame or not (gen and load and opaque)):
            return stopped | {"reason": f"invalid ordinary body proof at g{group}/{wait}"}
        source = [f for f, ctx, load_id, g, r, length in reads
                  if (ctx, load_id, g, r, length) == (context, load, group, record, size)]
        lookup = "".join(value.to_bytes(4, "little").hex() for value in (context, group, ordinal))
        completion = owner.to_bytes(4, "little").hex() + "0d0000000000000000000000"
        if (len(source) != 1 or not loaded <= source[0] <= marked
                or not re.search(rf"^frame={frame} input-wait native 71 address=0x32f$", events, re.MULTILINE)
                or not re.search(rf"^frame={reveal} native-call index=69 arguments=16 bytes \[{completion}\]",
                                 events, re.MULTILINE)
                or not re.search(rf"^frame={marked} native-call index=326 arguments=12 bytes \[{lookup}\]",
                                 events, re.MULTILINE)
                or not re.search(rf'^frame={marked} .*debug="\[input\]"$', events, re.MULTILINE)):
            return stopped | {"reason": f"missing source, executed wait or reveal receipt at g{group}/{wait}"}
        live = {task: (kind, parent, name) for f, task, kind, parent, name in tasks if f <= frame}
        if (flow != eye["flow_owner"] or live.get(script, (-1, -1, ""))[:2] != (0, flow)
                or live.get(owner) != (12, script, "meswnd") or live.get(plane) != (2, owner, "message main")
                or live.get(string) != (45, plane, "string")):
            return stopped | {"reason": f"ordinary body task chain differs at g{group}/{wait}"}
    if result.get("last_frame") != pages[-1][0] or eye["release_frame"] > pages[0][0]:
        return stopped | {"reason": "route did not terminate at the expected proof"}
    armed = [tuple(map(int, row)) for row in re.findall(
        r"^frame (\d+): 08 armed Enter region=(\d+) receiver=(\d+) candidates=.*$", host, re.MULTILINE)]
    keyboard = [tuple(map(int, row)) for row in KEYBOARD_RECEIPT.findall(host)]
    keyboard_frames = {row[0] for row in keyboard}
    if len(armed) != body_presses or keyboard_frames != {down + 1 for down, _, _ in armed}:
        return stopped | {"reason": "unexpected bottom-button arm or receipt frame"}
    for arm, second, (down, region, receiver) in zip(pages, pages[1:], armed):
        consumed = [row for row in keyboard if row[0] == down + 1 and row[-1] == 0]
        if (down != arm[0] or receiver != arm[7] or f"frame {down}: down Keyboard(13)" not in host.splitlines()
                or consumed != [(down + 1, 2, region, receiver, 0)]
                or re.findall(rf"^frame={down + 1} message-resume (.*)$", events, re.MULTILINE)
                != ["message=[2, 0, 0, 0]"]):
            return stopped | {"reason": "Enter did not consume at the live bottom button and resume the VM"}
        if (arm[3:11] != second[3:11] or not (arm[11] < second[11] and down < second[12])
                or any(down < f <= second[0] and task in (*arm[6:11], region) for f, task, *_ in tasks)):
            return stopped | {"reason": "successor changed the guarded load, tasks, or generation"}
    legacy_gates = ("first_body_verified", "second_body_verified", "third_body_verified")
    gate = legacy_gates[body_presses] if body_presses < len(legacy_gates) else "continuous_body_route_settled"
    return eye | {"gate": gate,
                  "body_wait_verified": True, "waits": [page[2] for page in pages],
                  "ordinary_generations": [page[11] for page in pages],
                  "opaque_rgb_matches": [page[14] for page in pages]}


def classify(result, host, events, body_presses=0, expected_pages=SUCCESSOR):
    if result.get("mode") == "state":
        eye = classify_exit(result, host, events, body_presses)
        if eye["gate"] != "eyecatch_release":
            return eye
        ordinary = ORDINARY_BODY.search(events)
        bodies = FIRST_BODY.findall(events)
        if not ordinary and not bodies and not body_presses:
            return eye
        entries = [tuple(map(int, entry)) for entry in GUARDED_ENTRY.findall(host)]
        hovers = [tuple(map(int, hover)) for hover in ENTRY_HOVER.findall(host)]
        if (len(hovers) != len(entries) or not hovers[0][0] < entries[0][0] < hovers[1][0]
                or any(hover[0] >= entry[0] or hover[1:] != entry[1:]
                       for hover, entry in zip(hovers, entries))):
            return eye | {"gate": "unproven", "reason": "title or card lacks a completed hover on the consuming owner"}
        if ordinary or body_presses:
            return classify_ordinary(result, host, events, eye, body_presses, expected_pages)
        stopped = eye | {"gate": "unproven", "reason": "invalid 08 first-body receipt"}
        if len(bodies) != 1:
            return stopped
        values = bodies[0]
        frame, owner, parent, context, load, loaded, epoch, source_frame, marked = map(int, values[:-1])
        address = values[-1]
        if (loaded != eye["loaded_frame"] or not eye["press_receipt_frame"] <= source_frame <= frame
                or not eye["press_receipt_frame"] <= marked < frame or frame < eye["release_frame"]):
            return stopped
        status = re.search(rf"^frame {frame}: wait (.*)$", host, re.MULTILINE)
        body = (f'body_wait_shell(window,owner,scene,loaded,marked,group,record)='
                f'Some(("meswnd2", {owner}, "c01_01", {loaded}, {marked}, 0, 144))')
        tag = (f"CstBodySource {{ context_handle: {context}, load_id: {load}, group: 0, "
               f"absolute_record: 143 }}")
        page = f"body_page(context,sources,exact_match,composited)=Some(({context}, Some([{tag}]), true, true))"
        if status is None or body not in status[1] or page not in status[1]:
            return stopped
        generation = BODY_GENERATION.search(status[1])
        if not generation or tuple(map(int, generation.groups()[:3])) != (owner, epoch, source_frame):
            return stopped
        reads = re.findall(
            rf"^frame=(\d+) cst-source-record CstSourceRecordObservation \{{ source: {re.escape(tag)}, "
            r"frame: \1, probe: Eye08, native: 327, kind: 32, stored_codec: \w+, output_codec: \w+, "
            r"raw_bytes: 20, output_bytes: [1-9]\d*, body_tagged: true, localized: false \}$", events, re.MULTILINE,
        )
        if (len(reads) != 1 or not loaded <= int(reads[0]) <= source_frame
                or not re.search(rf"^frame={frame} input-wait native 71 address={address}$", events, re.MULTILINE)
                or not re.search(rf'^frame={marked} .*debug="\[input\]"$', events, re.MULTILINE)
                or not any(int(task) == owner and int(kind) == 34 and int(p) == parent and int(at) <= frame
                           for at, task, kind, p, _ in TASK.findall(events))):
            return stopped
        return eye | {"gate": "first_body_verified", "body_wait_verified": True,
                      "first_body": {"frame": frame, "group": 0, "wait": 144, "record": 143,
                                     "owner": owner, "parent": parent, "context": context, "load": load}}
    return classify_hover(result, host, events)


def static_successor():
    from cst_variants import digest, expand_cst
    from plan_island_bad02 import checkpoint_rows
    from run_playthrough_batch import INVENTORY, load_inventory

    config = json.loads(Path(__file__).with_name("pc_story_08_post_eyecatch_wait.json").read_text(encoding="utf-8"))
    source = config["source"]
    raw = (Path(load_inventory(INVENTORY)[0][8]["root"]) / source["path"]).read_bytes()
    if digest(raw) != source["container_sha256"]:
        raise ValueError("08 loose CST digest differs")
    data, _ = expand_cst(raw)
    waits, _ = checkpoint_rows(data)
    _, _, offsets, strings = unpack_from("<IIII", data)

    def read_record(record):
        pos = 16 + strings + unpack_from("<I", data, 16 + offsets + record * 4)[0]
        return data[pos + 1], data[pos + 2:data.index(0, pos + 2)]

    digests = (config["records"]["first_body"]["payload_sha256"],
               config["records"]["next_static_body"]["payload_sha256"],
               "cfac28e945c0f205420976d4640b98a2bebd917595c0267e5cb0f60a39a96996")
    for (group, wait, ordinal, record, size), expected_digest in zip(SUCCESSOR, digests):
        if waits[group] != {"group": group, "record": wait, "kind": "input", "nonempty_body_records": [record]}:
            raise ValueError(f"08 static wait differs at g{group}/{wait}")
        start = unpack_from("<II", data, 16 + group * 8)[1]
        kind, payload = read_record(record)
        if wait - start != ordinal or kind != 0x20 or len(payload) != size or digest(payload) != expected_digest:
            raise ValueError(f"08 body digest differs at r{record}")
    handoff = [read_record(record) for record in (145, 146)]
    if handoff != [(0x30, b"se 9 vol 100"), (0x30, b"se 9 loop se078")]:
        raise ValueError("08 static successor controls differ")
    return {"gate": "static_successor", "scene": "c01_01", "waits": [page[1] for page in SUCCESSOR],
            "controls": [command.decode("ascii") for _, command in handoff]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("log_directory", type=Path, nargs="?")
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--require-exit", action="store_true", help="Require the guarded release")
    mode.add_argument("--require-first-body", action="store_true", help="Require the read-only r144 checkpoint")
    mode.add_argument("--require-next-body", action="store_true", help="Require one Enter and the settled r148 successor")
    mode.add_argument("--through-g2", action="store_true", help="Require two Enter presses and the settled r150 successor")
    mode.add_argument("--static-successor", action="store_true", help="Check r144, r148 and r150 against the local CST")
    args = parser.parse_args()
    if args.static_successor:
        print(json.dumps(static_successor(), indent=2))
        return 0
    base = args.log_directory
    if base is None:
        parser.error("provide a batch directory or --static-successor")
    run = json.loads((base / "08.json").read_text(encoding="utf-8"))
    logs = base / "08" if run.get("mode") == "state" else base
    if run.get("status") == "resource_deferred":
        result = {"gate": "unproven", "input_authorized": False, "reason": "replay was resource_deferred"}
    else:
        body_presses = 2 if args.through_g2 else int(args.require_next_body)
        result = classify(run, (logs / "08.host.log").read_text(encoding="utf-8"),
                          (logs / "08.events.log").read_text(encoding="utf-8"), body_presses)
    print(json.dumps(result, indent=2))
    passed = ("first_body_verified", "eyecatch_release")
    if args.through_g2:
        passed = ("third_body_verified",)
    elif args.require_next_body:
        passed = ("second_body_verified",)
    elif args.require_first_body:
        passed = ("first_body_verified",)
    elif not args.require_exit:
        passed += ("eyecatch_hover_control",)
    return 0 if result["gate"] in passed else 1


if __name__ == "__main__":
    raise SystemExit(main())

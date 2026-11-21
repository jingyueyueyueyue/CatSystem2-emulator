"""Check the first body wait and one-page successor for 09/20 without launching replay."""

import argparse
import json
import re
from pathlib import Path

from check_neko_first_page import INSPECT, LAYER


KANMI_WAIT_ADDRESS = 0xCAE
KANMI_TITLE_RESUME = "message-resume message=[2, 140, 354, 0]"
ENTER_RESUME = "message-resume message=[2, 0, 0, 0]"
PROFILES = {
    "09": ("sc01", 41, 43, "meswnd.fes", ("MES_SHOW", "WAIT"), KANMI_WAIT_ADDRESS,
           "c264b2465889c78723079a694d2654ef1091253db657a735e92f58b75031aa26"),
    "20": ("A_01_01", 13, 15, "meswnd01.fes", ("WAIT", "MES_SETSTATE"), 2768,
           "a099ac1cee7af60c8f32869485c9df4aab344b5eb91c1018ae8bb7df2c1f39f3"),
}
WAIT = re.compile(r"^frame (\d+): wait (.*)$")
WINDOW = re.compile(r'\((\d+), "([^"]+)", (true|false), '
                    r'Some\(\("([^"]+)", false, Some\(Frame \{ after: (\d+) \}\)\)\)')
CONTEXT = re.compile(r'\((\d+), "([^"]+)", (\d+), None, Some\(\((\d+), 2, (\d+), (\d+)\)\)\)')
LIST = re.compile(r'^list \d+ .* enabled=true submit=true .* nonzero_alpha=Some\([1-9]\d*\)$')
RECEIPT = re.compile(r'InputRouteReceipt \{ frame: (\d+), key: (\d+), group: (\d+), raw_mask: (\d+), '
                     r'pressed: (\d+), repeated: (true|false), region: (\d+), receiver: (\d+), response: (\d+), '
                     r'physical: \[(.*?)\] \}')
DOWN = re.compile(r'^frame (\d+): down (.+)$')
ROOT_WAIT = re.compile(r"^input-wait native 71 address=(0x[0-9a-f]+)$")
ENTER = "RoutedPhysicalButton { source: Keyboard, code: 13 }"
LEFT = "RoutedPhysicalButton { source: Mouse, code: 1 }"
ROUTE_PAGE = re.compile(r"^replay frame=(\d+) state=page scene=(\w+) group=(\d+) record=(\d+)$")
KANMI_TITLE_PRESS = re.compile(r"^frame (\d+): verified Kanmi New Game mouse press and VM resume$")
KANMI_ENTER_TARGET = re.compile(
    r"^frame (\d+): Kanmi Enter expected region=(\d+) receiver=(\d+), live region=(\d+) receiver=(\d+)$"
)
KANMI_ENTER_MISMATCH = re.compile(r"^replay frame=(\d+): Kanmi Enter target did not match the live body$")
KANMI_TITLE_MISMATCH = re.compile(r"^replay frame=(\d+): Kanmi New Game live target changed$")
KANMI_GUARDED_PAGE = re.compile(
    r'^frame (\d+): guarded press successor=Position \{ state: "page", scene: "sc01", '
    r'group: (\d+), record: (\d+) \} window=Some\(\("meswnd", (\d+)\)\)$'
)
KANMI_READ = re.compile(r"^native-call index=327 arguments=16 bytes \[([0-9a-f]{32})\]$")
KANMI_QUEUE = re.compile(r"^fes-text-sourced-queued bytes=(\d+) first=\S+ fes=Cp932 cst=\[Cp932\]$")
KANMI_SOURCE = re.compile(
    r"^cst-source-record CstSourceRecordObservation \{ source: CstBodySource \{ context_handle: (\d+), "
    r"load_id: ([1-9]\d*), group: (\d+), absolute_record: (\d+) \}, frame: (\d+), "
    r"probe: Kanmi, native: 327, kind: 32, stored_codec: Cp932, output_codec: Cp932, "
    r"raw_bytes: ([1-9]\d*), output_bytes: \6, body_tagged: true, localized: false \}$"
)
BODY = r"\0text:737472[0]"
REBELLIONS_WAIT_START = re.compile(
    r"^native-call index=630 arguments=16 bytes \[([a-f0-9]{8})(?:73de040082de0400|84de040093de0400)00000000\]$"
)
REBELLIONS_BODY_BINDING = re.compile(
    r"^frame (\d+): Rebellions body owner=(\d+) region=(\d+) receiver=(\d+)$"
)
REBELLIONS_RELEASE = re.compile(r"^frame (\d+): Rebellions Enter release observed$")
REBELLIONS_ENTRY = (
    (16, (480, 667), (3, 16)),
    (14, (353, 308), (3, 14)),
    (15, (723, 418), (15,)),
)
REBELLIONS_SOURCE = re.compile(
    r"^cst-source-record CstSourceRecordObservation \{ source: CstBodySource \{ context_handle: (\d+), "
    r"load_id: ([1-9]\d*), group: (\d+), absolute_record: (\d+) \}, frame: (\d+), "
    r"probe: Rebellions(?:Second|Third)?, native: 327, kind: 32, stored_codec: Gbk, output_codec: Gbk, "
    r"raw_bytes: ([1-9]\d*), output_bytes: \6, body_tagged: true, localized: false \}$"
)
BODY_GENERATION = re.compile(
    r"body_generation\(owner,generation,current_committed\)=Some\(\((\d+), "
    r"Some\(StoryBodyGeneration \{ source_epoch: ([1-9]\d*), source_frame: (\d+), "
    r"natural_complete: Some\(\(\2, (\d+)\)\), committed_epoch: Some\(\2\), remaining: 0, "
    r"remaining_segment: 0, pending_text: false, drawing: false, skip_pending: false, forced: false, "
    r"projection: Some\(\(true, [1-9]\d*\)\) \}\), true\)\)"
)
BODY_PAGE = re.compile(
    r"body_page\(context,sources,exact_match,composited\)=Some\(\((\d+), "
    r"Some\(\[CstBodySource \{ context_handle: \1, load_id: ([1-9]\d*), group: (\d+), "
    r"absolute_record: (\d+) \}\]\), true, true\)\)"
)
TASK_CREATED = re.compile(r'^task-create id=(\d+) kind=(\d+) parent=(\d+) name="([^"]*)" ')


def read_host(lines, layer_pattern=LAYER):
    layers, observations, receipts, presses = {}, [], [], []
    inspection, submitted = None, False
    for line in lines:
        if " pointer " in line and ", routes [" in line:
            for frame, key, group, mask, pressed, repeated, region, receiver, response, physical in RECEIPT.findall(line):
                receipts.append({
                    "frame": int(frame), "key": int(key), "group": int(group), "raw_mask": int(mask),
                    "pressed": int(pressed), "repeated": repeated == "true", "region": int(region),
                    "receiver": int(receiver), "response": int(response), "physical": physical,
                })
        if down := DOWN.fullmatch(line):
            presses.append((int(down[1]), down[2]))
        if match := INSPECT.fullmatch(line):
            inspection, layers, submitted = int(match[1]), {}, False
        elif line.startswith("list "):
            submitted = bool(LIST.fullmatch(line))
        elif inspection is not None and submitted and (match := layer_pattern.fullmatch(line)) and int(match[3]):
            owner, name, _, alpha = match.groups()
            layers.setdefault(int(owner), {})[name] = int(alpha)
        elif (match := WAIT.fullmatch(line)) and int(match[1]) == inspection:
            observations.append((inspection, match[2], layers))
            inspection = None
    return observations, receipts, presses


def collect_events(lines, scene):
    events = {"load": set(), "message": set(), "input": set(), "resume": [],
              "root_wait": set(), "sourced": set(), "wait_start": set(), "replay_status": [],
              "route_pages": [], "source_records": [], "kanmi_r42_bytes": {},
              "kanmi_r42_reads": set(), "kanmi_queue": set(),
              "rebellions_sources": [], "rebellions_tasks": {}}
    for line in lines:
        if line.startswith("replay_status="):
            events["replay_status"].append(line.partition("=")[2])
            continue
        if page := ROUTE_PAGE.fullmatch(line):
            if page[2] == scene:
                events["route_pages"].append((int(page[1]), int(page[3]), int(page[4])))
            continue
        match = re.match(r"^frame=(\d+) (.*)$", line)
        if not match:
            continue
        frame, event = int(match[1]), match[2]
        if event.endswith(f'debug="sscript loaded > {scene}"'):
            events["load"].add(frame)
        elif 'debug="[message] ' in event:
            events["message"].add(frame)
        elif event.endswith('debug="[input]"'):
            events["input"].add(frame)
        elif event.startswith("message-resume "):
            events["resume"].append((frame, event))
        elif wait := ROOT_WAIT.fullmatch(event):
            events["root_wait"].add((frame, int(wait[1], 16)))
        elif event.startswith("fes-text-sourced-queued "):
            events["sourced"].add(frame)
            if scene == "sc01" and (queue := KANMI_QUEUE.fullmatch(event)):
                events["kanmi_queue"].add((frame, int(queue[1])))
        elif source := KANMI_SOURCE.fullmatch(event):
            if frame == int(source[5]):
                context, load, group, record, read, size = map(int, source.groups())
                events["source_records"].append((context, load, group, record, read))
                if (group, record) == (1, 42):
                    events["kanmi_r42_bytes"][context, load, read] = size
        elif scene == "sc01" and (call := KANMI_READ.fullmatch(event)):
            arguments = bytes.fromhex(call[1])
            if arguments[8:] == bytes.fromhex("0100000000000000"):
                events["kanmi_r42_reads"].add((frame, int.from_bytes(arguments[:4], "little")))
        elif source := REBELLIONS_SOURCE.fullmatch(event):
            if frame == int(source[5]):
                events["rebellions_sources"].append(tuple(map(int, source.groups())))
        elif scene in {"A_01_01", "A_01_02", "A_01_03"} and (task := TASK_CREATED.match(event)):
            task_id, kind, parent = map(int, task.groups()[:3])
            events["rebellions_tasks"].setdefault(task_id, []).append((frame, kind, parent, task[4]))
        elif scene in {"A_01_01", "A_01_02", "A_01_03"} and (call := REBELLIONS_WAIT_START.fullmatch(event)):
            events["wait_start"].add((frame, int.from_bytes(bytes.fromhex(call[1]), "little")))
    return events


def verify_kanmi_route(host_lines, receipts, presses, events, receipt_receiver, expect_mismatch):
    # The exact state controller emits its guarded pages on stdout. The
    # generic wait diagnostics for this runtime still report "active".
    expected_status = "checkpoint_mismatch" if expect_mismatch else "completed"
    expected_count = 1 if expect_mismatch else 2
    pages = events["route_pages"]
    if events["replay_status"] != [expected_status] or len(pages) != expected_count or len(presses) != expected_count:
        return None
    first, first_group, first_wait = pages[0]
    mouse_frame, mouse = presses[0]
    if (first_group, first_wait) != (0, 41) or mouse != "MouseButton(0)" or not mouse_frame < first:
        return None
    consumed = [receipt for receipt in receipts if receipt["pressed"] and receipt["response"] == 0]
    if len(consumed) != expected_count:
        return None
    title = consumed[0]
    title_frame = mouse_frame + 1
    title_presses = [r for r in receipts if r["frame"] == title_frame and r["pressed"]]
    if (title["frame"], title["physical"], title["raw_mask"], title["key"], title["region"],
        title["group"], title["pressed"], title["repeated"]) != (title_frame, LEFT, 2, 0, 14, 0, 1, False):
        return None
    title_confirmed = [int(m[1]) for line in host_lines if (m := KANMI_TITLE_PRESS.fullmatch(line))]
    forwarded = {**title, "region": 3, "receiver": 0, "response": 1}
    if (title_presses not in ([title], [forwarded, title]) or title_confirmed != [title_frame]
            or [(f, p) for f, p in events["resume"] if f == title_frame] != [(title_frame, KANMI_TITLE_RESUME)]):
        return None
    earlier = {(context, load) for context, load, group, record, frame in events["source_records"]
               if (group, record) == (0, 40) and mouse_frame < frame <= first}
    if len(earlier) != 1:
        return None
    live_receiver = 8 if receipt_receiver is None else receipt_receiver
    expected_receiver = 6 if expect_mismatch else live_receiver
    targets = [tuple(map(int, m.groups())) for line in host_lines if (m := KANMI_ENTER_TARGET.fullmatch(line))]
    if len(targets) != 1:
        return None
    guard_frame, region, receiver, live_region, actual_receiver = targets[0]
    if first > guard_frame or (region, receiver, live_region, actual_receiver) != (60, expected_receiver, 60, live_receiver):
        return None
    later_resumes = [(frame, packet) for frame, packet in events["resume"] if frame > title_frame]
    if expect_mismatch:
        mismatches = [int(m[1]) for line in host_lines if (m := KANMI_ENTER_MISMATCH.fullmatch(line))]
        if (receiver == actual_receiver or mismatches != [guard_frame]
                or any(packet == ENTER_RESUME for _, packet in later_resumes)
                or any(ENTER in r["physical"] for r in receipts)
                or any(record == 42 and frame > guard_frame
                       for _, _, _, record, frame in events["source_records"])):
            return None
        return {"frame": guard_frame, "region": live_region,
                "expected_receiver": receiver, "live_receiver": actual_receiver}
    after, after_group, after_wait = pages[1]
    key_frame, key = presses[1]
    if ((after_group, after_wait) != (1, 43) or key != "Keyboard(13)"
            or key_frame != guard_frame or after <= key_frame + 3):
        return None
    advance = consumed[1]
    if (advance["frame"], advance["physical"], advance["raw_mask"], advance["key"], advance["region"],
        advance["receiver"], advance["group"], advance["pressed"], advance["repeated"]) != (
            key_frame + 1, ENTER, 1, 2, 60, live_receiver, 0, 1, False
        ):
        return None
    root_resume = [(frame, packet) for frame, packet in later_resumes if frame > key_frame]
    if (root_resume != [(key_frame + 1, ENTER_RESUME)]
            or (key_frame + 1, KANMI_WAIT_ADDRESS) not in events["root_wait"]):
        return None
    later = {(context, load) for context, load, group, record, frame in events["source_records"]
             if (group, record) == (1, 42) and key_frame < frame <= after}
    if later:
        return advance if earlier == later else None

    # Source diagnostics coalesce identical reads. For this route, group 1 ordinal 0
    # is absolute r42. Pair the retained source identity with the fresh read and queue.
    guarded = [tuple(map(int, match.groups())) for line in host_lines
               if (match := KANMI_GUARDED_PAGE.fullmatch(line))]
    if (len(guarded) != 2 or [item[:3] for item in guarded] != pages
            or guarded[0][3] != guarded[1][3]
            or any((frame, KANMI_WAIT_ADDRESS) not in events["root_wait"] for frame in (first, after))):
        return None
    prefetched = {(context, load, size) for (context, load, read), size in events["kanmi_r42_bytes"].items()
                  if mouse_frame < read <= key_frame}
    if len(prefetched) != 1:
        return None
    context, load, size = prefetched.pop()
    if (earlier == {(context, load)}
            and any(handle == context and key_frame < frame <= after and (frame, size) in events["kanmi_queue"]
                    and frame in events["message"] and frame in events["input"]
                    for frame, handle in events["kanmi_r42_reads"])):
        return advance
    return None


def rebellions_body(status, events, owner, context, loaded, position, frame):
    if position[:2] not in ((0, 13), (1, 15)):
        return None
    generation = BODY_GENERATION.search(status)
    page = BODY_PAGE.search(status)
    if generation is None or page is None:
        return None
    text_owner, epoch, source_frame, completed = map(int, generation.groups())
    body_context, load_id, group, record = map(int, page.groups())
    if ((text_owner, body_context, group, record) != (owner, context, position[0], position[1] - 1)
            or not loaded <= source_frame <= completed <= frame):
        return None
    for source_context, source_load, source_group, source_record, read_at, count in reversed(events["rebellions_sources"]):
        if ((source_context, source_load, source_group, source_record) == (context, load_id, group, record)
                and loaded <= read_at <= source_frame):
            return {"load_id": load_id, "record": record, "epoch": epoch, "source_frame": source_frame,
                    "complete_frame": completed, "source_read_frame": read_at, "gbk_bytes": count}
    return None


def rebellions_receiver(tasks, receipt, first, after):
    def last_created(task):
        return next((entry for entry in reversed(tasks.get(task, [])) if entry[0] <= after["frame"]), None)

    region, receiver = receipt["region"], receipt["receiver"]
    button = last_created(region)
    owner = last_created(first["owner"])
    return (button is not None and owner is not None
            and button[0] <= first["frame"] and button[1:] == (6, receiver, "bottom button")
            and owner[0] <= first["frame"] and owner[1] == 34 and owner[2] == receiver)


def rebellions_entry(presses, receipts, events, first_frame):
    prefix = [(frame, button) for frame, button in presses if frame < first_frame]
    if len(prefix) != len(REBELLIONS_ENTRY):
        return False
    for (down, button), (region, (x, y), order) in zip(prefix, REBELLIONS_ENTRY):
        frame = down + 1
        pushed = [item for item in receipts if item["frame"] == frame and item["pressed"]]
        regions = tuple(item["region"] for item in pushed)
        # The forwarding callback is allowed at the first two screens.
        if (button != "MouseButton(0)" or regions not in ((region,), order)
                or any(item["pressed"] != 1 or item["physical"] != LEFT or item["raw_mask"] != 2
                       or item["repeated"] or (item["key"], item["group"]) != (0, 0) for item in pushed)
                or pushed[-1]["response"] != 0 or pushed[-1]["receiver"] == 0
                or any((item["receiver"], item["response"]) != (0, 1) for item in pushed[:-1])
                or (frame, 0xAD0) not in events["root_wait"]
                or [packet for at, packet in events["resume"] if at == frame]
                != [f"message-resume message=[2, {x}, {y}, 0]"]):
            return False
    return True


def verify(observations, receipts, presses, events, profile, receipt_receiver=None,
           *, host_lines=(), expect_mismatch=False, expect_r13_observe=False):
    scene, first_record, next_record, name, blocks, address, digest = profile
    found = []
    for frame, status, layers in observations:
        windows_text = status.partition(" windows(")[2].partition(" body_wait_shell(")[0]
        windows = WINDOW.findall(windows_text)
        owners = [item for item in windows if item[1] == name]
        if len(owners) != 1:
            continue
        owner, _, wait_key, block, deadline = owners[0]
        owner = int(owner)
        pixels = layers.get(owner, {})
        quiet = f"focused=true paused=false message_wait=Some({address}) vm_waiting_for_message=true pending_messages=0 movies=0"
        flow = any(item[1:] == ("flow", "false", "GAME_WAIT", str(frame)) for item in windows)
        contexts = status.partition(")=[")[2].partition("] windows(")[0]
        context = CONTEXT.fullmatch(contexts)
        expected = None
        if context and context[2] == scene:
            loaded, marked, group, record = map(int, context.groups()[2:])
            if loaded in events["load"] and loaded <= marked < frame and marked in events["input"]:
                expected = (group, record, marked)
        shell = (block in blocks and wait_key == "true" and int(deadline) == frame
                 and flow and windows_text.count('Some(("') == len(windows) == 2
                 and quiet in status and pixels.get(BODY, 0) > 0)
        wait_start = (None if scene != "A_01_01" or expected is None else
                      max((started for started, receiver in events["wait_start"]
                           if receiver == owner and expected[2] <= started <= frame), default=None))
        identity = (f'kanmi_verified_pe=Some("{digest}")' if scene == "sc01"
                    else f'pe_sha256=Some("{digest}")')
        body = (rebellions_body(status, events, owner, int(context[1]), loaded, expected, frame)
                if scene == "A_01_01" and expected is not None else None)
        state_ready = False
        if expected is not None:
            group, record, _ = expected
            if scene == "A_01_01":
                state_ready = (block == "MES_SETSTATE" and (frame, group, record) in events["route_pages"]
                               and wait_start is not None and body is not None)
            else:
                state_ready = f"state=page scene={scene} group={group} record={record} " in status
        ready = (shell and pixels.get("keywait", 0) > 0 and state_ready
                 and 'trace_enabled=true' in status and identity in status
                 and 'startup=Some("kcs.int/main.kcs")' in status)
        found.append({"frame": frame, "owner": owner, "block": block, "wait_key": wait_key == "true",
                      "body_alpha": pixels.get(BODY, 0), "prompt_alpha": pixels.get("keywait", 0),
                      "position": expected, "context": int(context[1]) if expected else None,
                      "loaded": loaded if expected else None, "wait_shell_matched": shell,
                      "wait_start": wait_start,
                      "page_confirmed": ready})
        if scene == "A_01_01":
            found[-1]["body_provenance"] = body
    if scene == "sc01":
        route_receipt = verify_kanmi_route(host_lines, receipts, presses, events, receipt_receiver, expect_mismatch)
        if route_receipt is not None:
            return ("receiver_mismatch_verified" if expect_mismatch else "successor_verified"), found, route_receipt
        if (not presses and events["replay_status"] == ["checkpoint_mismatch"]
                and any(KANMI_TITLE_MISMATCH.fullmatch(line) for line in host_lines)):
            return "title_target_mismatch", found, None
        if expect_mismatch:
            return "receiver_mismatch_unverified", found, None
    bindings = {
        int(match[1]): tuple(map(int, match.groups()[1:]))
        for line in host_lines if (match := REBELLIONS_BODY_BINDING.fullmatch(line))
    }
    releases = [int(match[1]) for line in host_lines if (match := REBELLIONS_RELEASE.fullmatch(line))]
    if expect_r13_observe:
        for first in found:
            frame = first["frame"]
            binding = bindings.get(frame)
            if (first["page_confirmed"] and first["position"][:2] == (0, 13)
                    and binding is not None and binding[0] == first["owner"]
                    and not any(down >= frame for down, _ in presses)
                    and events["route_pages"] == [(frame, 0, 13)]
                    and events["replay_status"] == ["completed"]
                    and rebellions_entry(presses, receipts, events, frame)):
                return "r13_observe_verified", found, None
        return "r13_observe_unverified", found, None
    observed_receipt = None
    for first in found:
        if not first["wait_shell_matched"] or not first["prompt_alpha"] or not first["position"] \
                or first["position"][:2] != (0, first_record):
            continue
        body_presses = [(frame, button) for frame, button in presses if frame >= first["frame"]]
        if len(body_presses) != 1 or body_presses[0][1] != "Keyboard(13)":
            continue
        dispatched = body_presses[0][0]
        consumed = [receipt for receipt in receipts if receipt["frame"] > first["frame"]
                    and receipt["pressed"] and receipt["response"] == 0]
        if len(consumed) != 1:
            continue
        receipt = consumed[0]
        frame = receipt["frame"]
        if (frame != dispatched + 1 or receipt["repeated"] or receipt["pressed"] != 1
                or receipt["raw_mask"] != 1 or receipt["physical"] != ENTER
                or (receipt["key"], receipt["group"]) != (2, 0)
                or receipt["receiver"] in (0, first["owner"])
                or receipt_receiver is not None and receipt["receiver"] != receipt_receiver
                or (frame, address) not in events["root_wait"]):
            continue
        for after in found:
            if not after["wait_shell_matched"] or not after["prompt_alpha"] or not after["position"] \
                    or after["position"][:2] != (1, next_record) or after["owner"] != first["owner"] \
                    or (after["context"], after["loaded"]) != (first["context"], first["loaded"]):
                continue
            new_marker = after["position"][2]
            resumes = [(at, packet) for at, packet in events["resume"] if first["frame"] < at <= new_marker]
            if (resumes != [(frame, ENTER_RESUME)]
                    or not events["message"].intersection(range(frame, new_marker + 1))
                    or not events["sourced"].intersection(range(frame, new_marker + 1))
                    or after["body_alpha"] == first["body_alpha"] and not after["page_confirmed"]
                    or scene == "A_01_01" and not after["page_confirmed"]
                    and (after["wait_start"] is None or after["wait_start"] <= new_marker)):
                continue
            if scene == "A_01_01" and not rebellions_receiver(events["rebellions_tasks"], receipt, first, after):
                continue
            strict = (scene == "A_01_01" and first["page_confirmed"] and after["page_confirmed"]
                      and events["replay_status"] == ["completed"])
            if strict:
                before_body, after_body = first["body_provenance"], after["body_provenance"]
                strict = (after_body["load_id"] == before_body["load_id"]
                          and after_body["epoch"] > before_body["epoch"]
                          and after_body["source_frame"] >= frame and after_body["source_read_frame"] >= frame
                          and events["route_pages"] == [(first["frame"], 0, 13), (after["frame"], 1, 15)]
                          and bindings.get(first["frame"]) == bindings.get(after["frame"])
                          == (first["owner"], receipt["region"], receipt["receiver"])
                          and releases == [after["frame"]]
                          and first["frame"] == dispatched
                          and rebellions_entry(presses, receipts, events, first["frame"]))
            if strict:
                return "successor_verified", found, receipt
            observed_receipt = receipt
    if observed_receipt is not None:
        return "successor_observed", found, observed_receipt
    return "observe_only", found, None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("sample", choices=PROFILES)
    parser.add_argument("run", type=Path, help="batch output directory or the sample's log directory")
    parser.add_argument("--receipt-receiver", type=int, help="also require this receiver for the consuming Enter receipt")
    parser.add_argument("--expect-receiver-mismatch", action="store_true",
                        help="verify that 09 withheld Enter on the deliberate wrong receiver")
    parser.add_argument("--expect-r13-observe", action="store_true",
                        help="verify the state-routed 20 control with no body input")
    parser.add_argument("--player-sha256", help="require this player hash in the batch build_identity.json")
    args = parser.parse_args()
    if args.expect_receiver_mismatch and args.sample != "09":
        parser.error("--expect-receiver-mismatch is specific to 09")
    if args.expect_r13_observe and args.sample != "20":
        parser.error("--expect-r13-observe is specific to 20")
    if args.player_sha256 and not re.fullmatch(r"[0-9a-fA-F]{64}", args.player_sha256):
        parser.error("--player-sha256 must be a SHA-256 hex digest")
    run = args.run / args.sample if (args.run / args.sample).is_dir() else args.run
    batch = run.parent if run.name == args.sample else run
    try:
        if args.player_sha256:
            identity = json.loads((batch / "build_identity.json").read_text(encoding="utf-8"))
            if identity.get("sha256") != args.player_sha256.lower():
                raise ValueError("batch player hash does not match")
        summary = batch / f"{args.sample}.json"
        if summary.is_file() and json.loads(summary.read_text(encoding="utf-8")).get("status") == "resource_deferred":
            print(json.dumps({"status": "resource_deferred", "checks": [], "receipt": None}))
            return 1
        host_lines = (run / f"{args.sample}.host.log").read_text(encoding="utf-8", errors="replace").splitlines()
        with (run / f"{args.sample}.events.log").open(encoding="utf-8", errors="replace") as events_file:
            events = collect_events((line.rstrip() for line in events_file), PROFILES[args.sample][0])
    except (OSError, ValueError) as error:
        parser.error(str(error))
    observations, receipts, presses = read_host(host_lines)
    status, evidence, receipt = verify(
        observations, receipts, presses, events, PROFILES[args.sample], args.receipt_receiver,
        host_lines=host_lines, expect_mismatch=args.expect_receiver_mismatch,
        expect_r13_observe=args.expect_r13_observe,
    )
    print(json.dumps({"status": status, "checks": evidence, "receipt": receipt}, indent=2))
    return 0 if status in {"successor_verified", "receiver_mismatch_verified", "r13_observe_verified"} else 1


if __name__ == "__main__":
    raise SystemExit(main())

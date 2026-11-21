"""Check guarded NEKOPARA continuation, ordinary slot 0, and independent reopen logs."""

import argparse
from collections import defaultdict
import json
from pathlib import Path
import re
import stat
import xml.etree.ElementTree as ET

from check_neko_cold_language import (
    BOOT, CHECKPOINT, COLUMNS, CSTL_SHA256, GUARD, MESWND_CREATE, POINTER, PROBE_DETAILS, PROBE_RECEIVER,
    RECEIPT, extension_evidence,
    read_boot, read_sources, read_waits, table_path, verify, verify_body,
)
from check_neko_first_page import INPUT
from run_playthrough_batch import checked_path, file_sha256, is_sha256
from stage_neko_cold_language import (
    BODY_HIT_REGION, BODY_POINT, SAVE_ENTRY_LABEL as SAVE, SAVE_ENTRY_MODE, SAVE_PANEL_STOP,
    read_extension_contract, startup_probe_routes,
)


CONTINUATION_WAITS = (218, 337, 368, 381, 399, 405, 410, 461, 475, 481, 483, 485)
BODY_TARGET = " neko-body-target "
BODY_PROBE = re.compile(r'^frame (\d+): probe \[625, 478\] (.*)$')
EVIDENCE_PATH = Path(__file__).with_name("pc_story_neko_r337_to_r485_evidence.json")
PROBES = {"to-200": 22, "to-213": 23, "to-218": 24, "to-337": 25}
SLOT_NAME = "save0000.dat"
WRITE = re.compile(r'^frame=(\d+) fes-datasave slot=0 container=([1-9]\d*) written=true$')
OPEN = re.compile(r'^frame=(\d+) neko-slot-open native=247 slot=0 file="save0000\.dat" handle=([1-9]\d*) sha256=([a-f0-9]{64})$')
CST_HELPER = re.compile(
    r'^frame=(\d+) neko-cst-helper handle=(\d+) load_id=(\d+) outer_load_id=(\d+) '
    r'pc=Some\((\d+)\) source="scene\.int::sub_getattm2\.cst"$'
)
CST_RELOAD = re.compile(
    r'^frame=(\d+) neko-cst-reload handle=(\d+) load_id=(\d+) pc=(?:Some\((\d+)\)|None) preserved=(true|false)$'
)
CONTROL = " neko-save-control "
KITCHEN = " neko-kitchen-ready "
SAVE_PANEL = " neko-save-panel "
OBSERVED_SAVE_SLOT = (12, 32, [3, 32, 77, 52, 57, 123, 7])
SAVE_SLOT_PROBE = re.compile(
    r'^frame (\d+): probe \[285, 184\] masked=(?:Some\((\[.*?\])\)|None) '
    r'press_candidates=(\[.*?\]) window=Some\(\("saveload", (\d+)\)\) press_receivers=\[(.*)\]$'
)
TITLE_LOAD, SLOT, CONFIRM = "Open NEKO Load", "Select ordinary slot 0", "Confirm ordinary Load"
WRITE_SLOT, SAVE_CONFIRM = "Write ordinary slot 0", "Confirm ordinary Save"
RESTORED = "Stop at restored 01/24/218"
RESUME = "Turn at restored 01/24/218"
SAVE_DONE = "Stop after ordinary Save"


def lines(path):
    return path.read_text(encoding="utf-8-sig").splitlines()


def issue(report, condition, detail, kind="missing"):
    if not condition:
        report[kind].append(detail)


def route_status(events, report):
    statuses = [line for line in events if line.startswith("replay_status=")]
    issue(report, statuses == ["replay_status=completed"], f"completed state route, saw {statuses}")


def check_edges(events, waits, stop, report):
    for group in range(22, min(stop, 24)):
        if group not in waits or group + 1 not in waits:
            continue
        origin, target = waits[group], waits[group + 1]
        before, after = origin["runtime_body"], target["runtime_body"]
        created = [int(match[1]) for line in events if (match := MESWND_CREATE.match(line))
                   and origin["frame"] < int(match[1]) <= target["frame"]]
        same = (origin["owner"] == target["owner"] and before["context"] == after["context"]
                and before["load_id"] == after["load_id"] and not created)
        issue(report, same and before["source_epoch"] != after["source_epoch"]
              and origin["frame"] < after["source_frame"],
              f"01/{group}→{group + 1}: continuous meswnd/CST load and a fresh body", "failures")


def observations(events, tag):
    for line in events:
        head, separator, payload = line.partition(tag)
        if separator and re.fullmatch(r"frame=\d+", head):
            yield int(head[6:]), json.loads(payload)


def check_kitchen_assets(events, wait, press_frame, contract, report):
    kitchen = contract["kitchen"]
    expected = {"owner": wait["owner"], "background": kitchen["background"]["source"],
                "model": kitchen["model"]["source"], "object": kitchen["model"]["object"]}
    assets = [row for frame, row in observations(events, KITCHEN) if press_frame < frame == wait["frame"]]
    issue(report, len(assets) == 1, "r337 terminal background/model binding receipt")
    if len(assets) == 1:
        issue(report, all(assets[0].get(key) == value for key, value in expected.items()),
              "r337 background/model binding mismatch", "failures")
        report["kitchen_assets"] = assets[0]


def check_probe(run, language, mode, require_save_hit, require_kitchen_assets=False):
    _, routes, _, _ = startup_probe_routes(mode)
    contract, _, _ = read_extension_contract()
    records, bodies = extension_evidence()
    save_point = contract["ordinary_save_probe"]["save"]["button_probe_point"]
    result = verify(run, language, PROBES[mode] + 1, routes[language], records, bodies, save_point=save_point)
    waits = result.pop("pages")
    result["observed_pages"] = len(waits)
    events_path = run / "01.events.log"
    if events_path.is_file():
        events = lines(events_path)
        check_edges(events, waits, PROBES[mode], result)
        if mode == "to-337" and 24 in waits and 25 in waits:
            host = lines(run / "01.host.log")
            label = routes[language]["steps"][-2]["label"]
            presses = [int(match[1]) for line in host if (match := RECEIPT.fullmatch(line))
                       and json.loads(match[2]) == label]
            issue(result, len(presses) == 1, "one r218 physical press receipt before r337")
            if len(presses) == 1:
                issue(result, presses[0] < waits[25]["runtime_body"]["source_frame"],
                      "r337 body source must follow the consumed r218 press", "failures")
                if require_kitchen_assets:
                    check_kitchen_assets(events, waits[25], presses[0], contract, result)
    if require_save_hit:
        issue(result, result.get("save_hit_probe", {}).get("status") == "enabled_candidate",
              "enabled r218 Save candidate with a live meswnd receiver")
    return result


def title_probe(language, startup):
    _, routes, _, _ = startup_probe_routes("title")
    save = read_extension_contract()[0]["ordinary_save_probe"]
    route = routes[language]
    terminal = route["steps"][-1]
    terminal.update(label="Probe settled NEKO title Load without input",
                    probe_points=[save["reopen"]["title_probe_point"]])
    if startup == "title":
        route["steps"] = [terminal]
    route["label"] = f"NEKO {language} {startup} Load observation"
    return route


def continuation_evidence():
    resource = json.loads(EVIDENCE_PATH.read_bytes())
    if (resource["schema"], resource["evidence"], resource["cstl_sha256"], resource["columns"]) != (
        "cs2-neko-r337-resource-route/v1", "archive_members_only", CSTL_SHA256, COLUMNS,
    ):
        raise ValueError("wrong NEKO continuation resource fingerprint or schema")
    pages = {page["group"]: page for page in resource["pages"] if 25 <= page["group"] <= 35}
    if [(group, page["wait"]) for group, page in pages.items()] != list(enumerate(CONTINUATION_WAITS[1:], 25)):
        raise ValueError("NEKO continuation evidence does not match the route")
    records, bodies = extension_evidence()
    positions = list(records[:25])
    fingerprints = {language: list(rows[:22]) for language, rows in bodies.items()}
    if positions[-1][0] != CONTINUATION_WAITS[0] or any(len(rows) != 22 for rows in fingerprints.values()):
        raise ValueError("wrong r218 body evidence for the continuation join")
    for page in pages.values():
        if any((row["jp"]["bytes"] == 0) != (row["cn"]["bytes"] == 0) for row in page["text"]):
            raise ValueError("NEKO continuation disagrees on empty localized records")
        body, = [row for row in page["text"] if row["jp"]["bytes"] > 0]
        positions.append((page["wait"], body["record"], body["ordinal"], None))
        for language, rows in fingerprints.items():
            fingerprint = body[language]
            rows.append((fingerprint["bytes"], fingerprint["sha256"]))
    return positions, fingerprints, pages


def continuation_steps(target):
    count = CONTINUATION_WAITS.index(target)
    positions = list(enumerate(CONTINUATION_WAITS[:count + 1], 24))
    actions = [{"label": RESUME if group == 24 else f"Turn at 01/{group}/{record}",
                "wait": {"state": "page", "scene": "01", "window": "meswnd", "group": group, "record": record},
                "body_target": True}
               for group, record in positions[:-1]]
    group, record = positions[-1]
    terminal = RESTORED if count == 0 else f"Stop at 01/{group}/{record} without input"
    return tuple(group for group, _ in positions), actions, terminal


def menu_plan(language, startup, save_confirm, target):
    _, routes, _, _ = startup_probe_routes("to-218")
    opener = title_probe(language, startup)
    groups, actions, terminal = continuation_steps(target)
    return {
        "schema": "cs2-neko-ordinary-save-plan/v1",
        "language": language,
        "writer_typed_prefix": routes[language]["steps"][:-1],
        "writer_menu_labels": [SAVE, WRITE_SLOT, *([SAVE_CONFIRM] if save_confirm else []), SAVE_DONE],
        "reader_typed_prefix": opener["steps"][:-1],
        "reader_menu_labels": [TITLE_LOAD, SLOT, CONFIRM],
        "reader_restore": [24, 218],
        "reader_body_label": actions[0]["label"] if actions else None,
        "reader_body_actions": actions,
        "reader_terminal": {"label": terminal, "wait": [groups[-1], target]},
        "terminal_input": False,
    }


def ordinary_route(language, operation, startup, slot_sha256, target=218):
    if operation == "save":
        if slot_sha256 is not None or startup != "title" or target != 218:
            raise ValueError("save requires an empty slot, without a digest, and ends at r218")
        route = save_entry_route(language)
    else:
        if not is_sha256(slot_sha256) or slot_sha256 != slot_sha256.lower():
            raise ValueError("load requires the lowercase SHA-256 of the producer's exited slot 0")
        route = title_probe(language, startup)
    route["label"] = f"NEKO {language} ordinary {operation} slot 0"
    route["neko_ordinary"] = {"mode": operation, "language": language, "slot_sha256": slot_sha256}
    if target != 218:
        continuation_evidence()
        route["neko_ordinary"]["continue_to"] = target
    return route


def save_entry_route(language):
    return startup_probe_routes(SAVE_ENTRY_MODE)[1][language]


def check_save_entry(run, language):
    report = {"missing": [], "failures": []}
    route = save_entry_route(language)
    issue(report, json.loads((run / "01.route.json").read_text(encoding="utf-8-sig")) == route,
          "exact staged one-time Save panel route", "failures")
    events, host = (lines(run / f"01.{name}.log") for name in ("events", "host"))
    route_status(events, report)
    check_bootstrap(events, run / "01", language, "Save entry", report)
    records, bodies = extension_evidence()
    waits, _ = read_waits(host, records)
    sources = read_sources(events)
    issue(report, set(waits) == set(range(25)), "25 composed scene-01 waits through r218")
    for group, wait in waits.items():
        if group <= 24:
            verify_body(wait, group, language, sources, report["missing"], report["failures"],
                        records[group], bodies, False)
    check_edges(events, waits, 24, report)
    actions = route["steps"][:-1]
    set_save_entry_response(host, actions[-1], report)
    contract, _, _ = read_extension_contract()
    presses, end = check_presses(events, host, actions, waits, SAVE_PANEL_STOP, contract, report)
    check_save_panel(events, host, waits, presses, end, contract, report)
    slot = run / "01.profile" / SLOT_NAME
    issue(report, not any(re.match(r"^frame=\d+ (fes-datasave|neko-slot-open) ", line) for line in events)
          and not slot.exists() and not slot.is_symlink(),
          "ordinary slot was touched during the input-free slot probe", "failures")
    return report


def set_save_entry_response(host, action, report):
    actual = [match for line in host if (match := RECEIPT.fullmatch(line)) and json.loads(match[2]) == SAVE]
    issue(report, len(actual) == 1, "one physical Save press receipt")
    if len(actual) == 1:
        response = int(actual[0][5])
        issue(report, response in (0, 1), "ordinary Save press response", "failures")
        action["mouse_press"]["response"] = response


def check_save_panel(events, host, waits, presses, panel_checkpoint, contract, report):
    panels = list(observations(events, SAVE_PANEL))
    issue(report, len(panels) == 1 and 24 in waits and SAVE in presses,
          "one observed Save panel after the r218 receipt")
    if len(panels) == 1 and 24 in waits and SAVE in presses:
        frame, panel = panels[0]
        wait = waits[24]
        body = wait["runtime_body"]
        save = contract["ordinary_save_probe"]["save"]
        issue(report, (frame == panel_checkpoint == panel["driver_frame"] == presses[SAVE]["successor"]
                      and presses[SAVE]["frame"] < frame and panel["block"] == "WAIT"
                      and panel["globals"] == save["globals"] and panel["locals"] == save["locals"]
                      and panel["slot"] == 0 and panel["slot_exists"] is False
                      and panel["retained"] == ["01", 24, 218, body["context"], body["load_id"], wait["owner"]]
                      and panel["point"] == save["slot_probe_point"] and panel["slot_region"] in panel["candidates"]
                      and [row[0] for row in panel["receivers"]] == panel["candidates"]
                      and [panel["slot_region"], panel["owner"]] in panel["receivers"]),
              "current empty slot 0 and retained r218 source in the settled Save panel", "failures")
        probes = patterns_by_frame(host, SAVE_SLOT_PROBE)[frame]
        issue(report, len(probes) == 1, "one live slot 0 hit probe at the Save panel stop")
        if len(probes) == 1:
            _, mask_text, candidates, owner, receiver_text = probes[0].groups()
            mask = json.loads(mask_text) if mask_text else []
            receivers = [[int(region), int(receiver) if receiver else None]
                         for region, receiver in PROBE_RECEIVER.findall(receiver_text)]
            issue(report, (len(mask) > 8 and mask[8] != 0 and mask[0] == panel["slot_region"]
                          and json.loads(candidates) == panel["candidates"]
                          and int(owner) == panel["owner"] and receivers == panel["receivers"]),
                  "Save panel receipt agrees with the live enabled slot and receiver probe", "failures")
        report["panel"] = {"frame": frame, **panel}


def slot_identity(path):
    info = checked_path(path)
    if path.name != SLOT_NAME or not stat.S_ISREG(info.st_mode) or info.st_nlink != 1 or info.st_size <= 16:
        raise ValueError("expected a regular, unlinked ordinary save0000.dat")
    with path.open("rb") as stream:
        header = stream.read(16)
    if header[:4] != b"CSS\0" or int.from_bytes(header[4:8], "little") != info.st_size - 16:
        raise ValueError("ordinary slot is not a complete CSS file")
    return {"size": info.st_size, "sha256": file_sha256(path)}


def read_processes(path, receipt, report, *, checkpoint=False):
    prefix = Path(receipt["output_prefix"])
    processes = (receipt["producer"], receipt["consumer"])
    schema = "cs2-neko-checkpoint-reopen/v1" if checkpoint else "cs2-neko-save-roundtrip/v1"
    if receipt["schema"] != schema or not prefix.is_absolute() or prefix.name != "01":
        raise ValueError(f"expected {schema} and an absolute private output prefix ending in /01")
    producer, consumer = processes
    producer_prefix = Path(producer["output_prefix"]) if checkpoint else prefix
    if not producer_prefix.is_absolute() or producer_prefix.name != "01" or checkpoint and producer_prefix == prefix:
        raise ValueError("expected an absolute checkpoint producer prefix ending in /01")
    valid_order = (all(type(run[key]) is int and run[key] > 0 for run in processes
                       for key in ("pid", "start_ms", "exit_ms")) and producer["pid"] != consumer["pid"]
                   and producer["start_ms"] <= producer["exit_ms"] < consumer["start_ms"] <= consumer["exit_ms"])
    issue(report, valid_order, "distinct producer and consumer PIDs with producer exit before consumer start", "failures")
    for key in ("sha256", "source_tree_sha256", "resource_tree_sha256"):
        matched = producer[key] == consumer[key] or checkpoint and key != "resource_tree_sha256"
        issue(report, all(is_sha256(run[key]) for run in processes) and matched,
              f"recorded producer and consumer {key} for {schema}", "failures")
    issue(report, all(run["exit_code"] == 0 and not run["timed_out"] for run in processes)
          and Path(producer["output_prefix"]) == producer_prefix and Path(consumer["output_prefix"]) == prefix,
          "both processes exit normally using their recorded private output prefixes", "failures")
    if any(Path(run[key]).name != run[key] for run in processes for key in ("events", "host")):
        raise ValueError("process log names must be relative leaf names")
    producer_logs = producer_prefix.parent if checkpoint else path.parent
    logs = [base / run[key] for base, run in ((producer_logs, producer), (path.parent, consumer))
            for key in ("events", "host")]
    log_infos = [checked_path(log) for log in logs]
    issue(report, all(stat.S_ISREG(info.st_mode) and info.st_nlink == 1 for info in log_infos)
          and len({(info.st_dev, info.st_ino) for info in log_infos}) == 4,
          "four distinct regular producer and consumer logs", "failures")
    slot = slot_identity(prefix.with_suffix(".profile") / SLOT_NAME)
    issue(report, producer["slot_before"] is None and producer["slot_after"] == slot
          and consumer["slot_before"] == slot and consumer["slot_after"] == slot,
          "empty slot before write and identical CSS size/hash from producer exit through consumer exit", "failures")
    report["slot"] = slot
    report["processes"] = {name: {key: run[key] for key in ("pid", "start_ms", "exit_ms", "events", "host", "sha256",
                                                            "source_tree_sha256", "resource_tree_sha256")}
                           for name, run in zip(("producer", "consumer"), processes)}
    return producer_prefix, prefix, tuple(lines(log) for log in logs)


def patterns_by_frame(log, pattern):
    matches = defaultdict(list)
    for line in log:
        if match := pattern.fullmatch(line):
            matches[int(match[1])].append(match)
    return matches


def check_control(row, label, save, frame, wait, report):
    specs = {
        SAVE: ("meswnd", "meswnd btn[4]", "MES_SHOW", save["save"]["button_probe_point"]),
        WRITE_SLOT: ("saveload", "btdata[0]", "WAIT", save["save"]["slot_probe_point"]),
        SAVE_CONFIRM: ("confirm2", "confirm2 btn[0]", "WAIT", save["reopen"]["confirm_probe_point"]),
        TITLE_LOAD: ("title", "title btn1[1]", "WAIT", save["reopen"]["title_probe_point"]),
        SLOT: ("saveload", "btdata[0]", "WAIT", save["save"]["slot_probe_point"]),
        CONFIRM: ("confirm2", "confirm2 btn[0]", "WAIT", save["reopen"]["confirm_probe_point"]),
    }
    window, control, block, point = specs[label]
    valid = (row["window"] == window and row["control"] == control and row["block"] == block
             and row["point"] == point and row["driver_frame"] == frame and row["ready"] is True)
    if label in (WRITE_SLOT, SLOT):
        mode = save["save"] if label == WRITE_SLOT else save["reopen"]
        valid &= (row["globals"] == mode["globals"] and row["locals"] == mode["locals"]
                  and row["slot"] == 0 and row["slot_exists"] is (label == SLOT))
        if label == SLOT:
            valid &= row["ttlflow"] == "LOAD_WAIT"
    if label in (SAVE_CONFIRM, CONFIRM):
        valid &= row["saveload"] == "CONFIRM_WAIT" and row["slot"] == 0
    if label in (SAVE, WRITE_SLOT, SAVE_CONFIRM):
        body = wait["runtime_body"]
        valid &= row["retained"] == ["01", 24, 218, body["context"], body["load_id"], wait["owner"]]
    if label == SAVE:
        valid &= wait["frame"] == frame and row["owner"] == wait["owner"] and row["hit_region"] != BODY_HIT_REGION
    if label == TITLE_LOAD:
        valid &= (row["flow"] == "WAIT_TITLE" and row["ttlflow"] == "FES_WAIT"
                  and row["hit_region"] != 14)
    issue(report, valid, f"{label}: exact current-frame live FES control and mode", "failures")


def check_body_target(rows, probes, wait, frame, group, record, label, report):
    if len(rows) != 1 or wait is None:
        report["missing"].append(f"{label}: one current-frame meswnd low button receipt")
        return None
    row, = rows
    region, owner = row["region"], row["owner"]
    candidates, receivers = row["candidates"], row["receivers"]
    valid = ((row["label"], row["group"], row["record"], row["control"], row["point"], owner, frame)
             == (label, group, record, "meswnd low button", BODY_POINT, wait["owner"], wait["frame"])
             and row["hit_region"] in candidates and [pair[0] for pair in receivers] == candidates
             and [pair for pair in receivers if pair[1] == owner] == [[region, owner]])
    issue(report, valid, f"{label}: one enabled live auxiliary target owned by the current meswnd", "failures")
    observed = []
    for probe in probes:
        match = PROBE_DETAILS.fullmatch(probe[2])
        if match and match[1] is not None:
            mask = json.loads(match[1])
            if len(mask) > 8 and mask[0] == row["hit_region"] and mask[8] != 0:
                observed.append((int(match[3]), json.loads(match[2]), [
                    [int(receiver[1]), int(receiver[2]) if receiver[2] else None]
                    for receiver in PROBE_RECEIVER.finditer(match[4])
                ]))
    issue(report, observed == [(owner, candidates, receivers)], f"{label}: live mask and receiver probe")
    return {**row, "response": 1}


def check_presses(events, host, actions, waits, terminal, contract, report, *, continuing=False):
    action_labels = [action["label"] for action in actions]
    checkpoints = [(int(match[1]), json.loads(match[3])) for line in events
                   if (match := CHECKPOINT.fullmatch(line)) and match[2] == "checkpoint"]
    verified = [(int(match[1]), json.loads(match[3])) for line in events
                if (match := CHECKPOINT.fullmatch(line)) and match[2] == "verified"]
    if continuing:
        join = next((index for index, (_, label) in enumerate(checkpoints) if label == terminal), None)
        if join is not None:
            checkpoints = checkpoints[:join + 1]
            verified = [(frame, label) for frame, label in verified if frame <= checkpoints[-1][0]]
    expected = [*action_labels, terminal]
    labels = [label for _, label in checkpoints]
    verified_labels = [label for _, label in verified]
    complete = labels == expected and verified_labels == action_labels
    if not complete:
        report["missing"].append("exact ordered checkpoints and verified successors through " + terminal)
    if labels != expected[:len(labels)] or verified_labels != action_labels[:len(verified_labels)]:
        report["failures"].append("unexpected or repeated checkpoint/verification before " + terminal)
        return {}, None
    downs = [int(line.split()[1][:-1]) for line in host if INPUT.match(line)]
    guards, pointers, probes = (patterns_by_frame(host, pattern) for pattern in (GUARD, POINTER, BODY_PROBE))
    receipts = [match for line in host if (match := RECEIPT.fullmatch(line))]
    if complete:
        if continuing:
            downs = [frame for frame in downs if frame < checkpoints[-1][0]]
            receipts = [receipt for receipt in receipts if int(receipt[1]) < checkpoints[-1][0]]
        input_limit = "before continuing" if continuing else "with zero terminal input"
        issue(report, downs == [frame for frame, _ in checkpoints[:-1]] and len(receipts) == len(actions),
              f"exactly the expected physical presses, {input_limit}", "failures")
    observed_controls = defaultdict(list)
    for frame, row in observations(events, CONTROL):
        observed_controls[(frame, row["label"])].append(row)
    body_targets = defaultdict(list)
    for frame, row in observations(events, BODY_TARGET):
        body_targets[frame].append(row)
    consumed = {}
    for action, (frame, label), (after, _), (next_frame, _) in zip(actions, checkpoints, verified, checkpoints[1:]):
        wait_condition = action.get("wait", {})
        wait = waits.get(wait_condition.get("group"))
        if wait_condition.get("state") == "page":
            issue(report, wait is not None and wait["frame"] == frame, f"{label}: composited wait at the press")
        if "mouse_press" in action:
            press = action["mouse_press"]
            window, point, hit = wait_condition["window"], action["click"], wait_condition["hit"]["region"]
        elif action.get("body_target"):
            press = check_body_target(body_targets[frame], probes[frame], wait, frame, wait_condition["group"],
                                      wait_condition["record"], label, report)
            if press is None:
                continue
            window, point, hit = "meswnd", press["point"], press["hit_region"]
        else:
            rows = observed_controls[(frame, label)]
            if len(rows) != 1 or label in (SAVE, WRITE_SLOT, SAVE_CONFIRM) and 24 not in waits:
                report["missing"].append(f"{label}: one current-frame FES control receipt")
                continue
            row = rows[0]
            check_control(row, label, contract["ordinary_save_probe"], frame, waits.get(24), report)
            window, point, hit = row["window"], row["point"], row["hit_region"]
            press = row
            issue(report, press["region"] in press["candidates"]
                  and [pair[0] for pair in row["receivers"]] == press["candidates"]
                  and [press["region"], row["owner"]] in row["receivers"],
                  f"{label}: live control candidate and receiver mapping", "failures")
        guard = guards[frame]
        matches = [match for match in receipts if json.loads(match[2]) == label]
        if len(guard) != 1 or len(matches) != 1:
            report["missing"].append(f"{label}: one guard and one physical receipt")
            continue
        actual, receipt = guard[0], matches[0]
        owner = int(actual[5])
        receipt_frame = int(receipt[1])
        wanted_owner = press.get("owner", wait["owner"] if wait else owner)
        valid = ((json.loads(actual[2]), json.loads(actual[3]), actual[4], owner)
                 == (point, press["candidates"], window, wanted_owner)
                 and tuple(map(int, receipt.groups()[2:])) == (press["region"], owner, press["response"])
                 and frame < receipt_frame <= frame + 3 and receipt_frame < after <= next_frame
                 and (not action.get("body_target") or frame + 3 < after == next_frame))
        issue(report, valid, f"{label}: guarded physical left press and verified successor", "failures")
        hit_observed = False
        for pointer in pointers[frame]:
            mask = json.loads(pointer[4])
            if ([int(pointer[2]), int(pointer[3])] == point and len(mask) > 8 and mask[0] == hit
                    and mask[8] != 0 and json.loads(pointer[5]) == press["candidates"]):
                hit_observed = True
        issue(report, hit_observed, f"{label}: enabled live hit at the press frame")
        consumed[label] = {"frame": receipt_frame, "checkpoint": frame, "successor": after,
                           "owner": owner, "region": press["region"], "candidates": press["candidates"]}
    return consumed, checkpoints[-1][0] if complete else None


def check_bootstrap(events, prefix, language, process, report):
    index, ui = read_boot(prefix.parent, language, report["missing"], report["failures"])
    boots = [match.groups() for line in events if (match := BOOT.fullmatch(line))]
    issue(report, len(boots) == 1, f"one {process} language bootstrap receipt")
    if len(boots) == 1:
        setup, ui_index, interface, main_index, main, table, game_table = boots[0]
        expected_table = "config/gamelangtbl_cn.txt" if language == "cn" else None
        issue(report, (setup, ui_index, interface, main_index, main, table_path(table), table_path(game_table))
              == ("true", ui, "en", str(index), language, "language_en.txt", expected_table),
              f"{process} requested the recorded language and interface", "failures")


def check_reopen_body(events, host, language, groups, report):
    records, bodies, resource_pages = continuation_evidence()
    waits, _ = read_waits(host, records)
    sources = read_sources(events)
    check_cst_helper_returns(events, waits, report)
    issue(report, set(waits) == set(groups), "exactly the requested restored and successor waits")
    for group in groups:
        if group not in waits:
            continue
        wait = waits[group]
        previous = waits.get(group - 1)
        read_window_start = previous["frame"] + 1 if group in (25, 31) and previous else None
        body = wait["runtime_body"]
        helper_reload_count = sum(edge["preserved"] and (edge["frame"], edge["handle"], edge["load_id"])
                                  == (wait["loaded"], body["context"], body["load_id"])
                                  for edge in report["cst_helper_returns"])
        verify_body(wait, group, language, sources, report["missing"], report["failures"],
                    records[group], bodies, False, read_window_start=read_window_start,
                    helper_reload_count=helper_reload_count)
        first_frame = wait["loaded"] + 1 if read_window_start is None else max(wait["loaded"], read_window_start)
        for row in resource_pages.get(group, {}).get("text", []):
            if row[language]["bytes"] != 0:
                continue
            context = wait["runtime_body"]["context"]
            tails = [read for read in sources[1][(context, group, row["ordinal"])]
                     if first_frame <= read["frame"] <= wait["marker"] and read["kind"] == "0x20"
                     and read["destination"] == "0x00062c6b"]
            issue(report, bool(tails), f"01/{group}/{wait['record']}: actual zero-byte 1038 tail r{row['record']}")
            issue(report, all((read["main"], read["requested"], read["source"], read["column"], read["bytes"],
                               read["sha256"]) == (language, language, "cstl", COLUMNS.index(language), 0,
                                                   row[language]["sha256"]) for read in tails),
                  f"01/{group}/{wait['record']}: wrong language or bytes in tail r{row['record']}", "failures")
    return waits


def check_cst_helper_returns(events, waits, report):
    active_loads = {}
    pending = {}
    returns = []
    for line in events:
        if match := CST_HELPER.fullmatch(line):
            frame, handle, helper_id, outer_id, pc = map(int, match.groups())
            issue(report, handle not in pending and active_loads.get(handle) == outer_id
                  and helper_id > 0 and helper_id != outer_id and pc in (0x14e91, 0x14f97),
                  f"frame {frame}: invalid CST helper entry: {line}", "failures")
            pending[handle] = (frame, helper_id, outer_id, pc)
        elif match := CST_RELOAD.fullmatch(line):
            frame, handle, load_id = map(int, match.groups()[:3])
            active_loads[handle] = load_id
            if handle not in pending:
                continue
            entered, helper_id, outer_id, entry_pc = pending.pop(handle)
            pc = int(match[4]) if match[4] is not None else None
            preserved = match[5] == "true"
            issue(report, entered == frame and pc in (0x14f97, 0x15178) and preserved and load_id == outer_id,
                  f"frame {frame}: CST helper load {helper_id} failed to restore outer load {outer_id}: {line}",
                  "failures")
            returns.append({"frame": frame, "entry_frame": entered, "handle": handle, "helper_load_id": helper_id,
                            "outer_load_id": outer_id, "load_id": load_id,
                            "entry_pc": entry_pc, "return_pc": pc, "preserved": preserved})

    issue(report, not pending, f"CST helpers without a return: {pending}", "failures")
    report["cst_helper_returns"] = returns
    before, after = waits.get(30), waits.get(31)
    if before and after:
        body = after["runtime_body"]
        observed = [edge for edge in returns if before["frame"] < edge["entry_frame"] <= after["marker"]
                    and edge["handle"] == body["context"]
                    and (edge["entry_pc"], edge["return_pc"]) == (0x14e91, 0x14f97)]
        issue(report, len(observed) == 3, f"r410->r461: expected three CST helper returns, saw {len(observed)}")
        issue(report, all(edge["load_id"] == body["load_id"] for edge in observed),
              "r461 body did not retain the CST load across the three helper returns", "failures")


def check_body_edges(events, waits, actions, presses, report):
    creations = [(int(match[1]), int(match[2])) for line in events if (match := MESWND_CREATE.match(line))]
    edges = []
    for action in actions:
        group, label = action["wait"]["group"], action["label"]
        origin, target, press = waits.get(group), waits.get(group + 1), presses.get(label)
        if origin is None or target is None or press is None:
            continue
        before, after = origin["runtime_body"], target["runtime_body"]
        created = [(frame, owner) for frame, owner in creations if origin["frame"] < frame <= target["frame"]]
        continuous = not created and origin["owner"] == target["owner"] and before["source_epoch"] != after["source_epoch"]
        rebuilt = (group == 24 and len(created) == 1 and created[0][0] >= press["frame"]
                   and created[0][1] == target["owner"])
        valid = (all(before[field] == after[field] for field in ("context", "load_id"))
                 and press["frame"] <= after["source_frame"] <= target["marker"]
                 and press["successor"] == target["frame"] and (continuous or rebuilt))
        issue(report, valid, f"01/{group}->{group + 1}: fresh adjacent body with continuous CST load and valid window lifetime",
              "failures")
        edges.append({"from": [group, origin["record"]], "to": [group + 1, target["record"]],
                      "press": press, "next_body_source_frame": after["source_frame"],
                      "from_owner": origin["owner"], "to_owner": target["owner"], "meswnd_creations": created})
    report["body_edges"] = edges


def check_roundtrip(path, target, require_kitchen_assets, *, continuing=False, checkpoint=False):
    result = {"missing": [], "failures": []}
    receipt = json.loads(path.read_text(encoding="utf-8-sig"))
    language = receipt["language"]
    if language not in ("jp", "cn") or type(receipt["save_confirm"]) is not bool:
        raise ValueError("expected jp/cn and a boolean save_confirm from the observed producer route")
    if receipt["reopen_startup"] not in ("title", "language-select"):
        raise ValueError("reopen_startup must match the observed title or language-select entry")
    producer_prefix, prefix, logs = read_processes(path, receipt, result, checkpoint=checkpoint)
    writer_events, writer_host, reader_events, reader_host = logs
    route_status(writer_events, result)
    route_status(reader_events, result)
    check_bootstrap(writer_events, producer_prefix, language, "producer", result)
    check_bootstrap(reader_events, prefix, language, "consumer", result)
    contract, _, _ = read_extension_contract()
    records, bodies = extension_evidence()
    writer_waits, _ = read_waits(writer_host, records)
    sources = read_sources(writer_events)
    _, routes, _, _ = startup_probe_routes("to-218")
    issue(result, set(writer_waits) == set(range(25)), "producer body waits from 01/0/59 to 01/24/218")
    for group in range(25):
        if group in writer_waits:
            verify_body(writer_waits[group], group, language, sources, result["missing"], result["failures"],
                        records[group], bodies, False)
    check_edges(writer_events, writer_waits, 24, result)
    writer_labels = [SAVE, WRITE_SLOT] + ([SAVE_CONFIRM] if receipt["save_confirm"] else [])
    write_actions = [*save_entry_route(language)["steps"][:-1], *({"label": label} for label in writer_labels[1:])]
    set_save_entry_response(writer_host, write_actions[-len(writer_labels)], result)
    write_presses, write_end = check_presses(writer_events, writer_host, write_actions, writer_waits,
                                            SAVE_DONE, contract, result)
    slot_press = write_presses.get(WRITE_SLOT)
    panel_checkpoint = slot_press["checkpoint"] if slot_press is not None else None
    check_save_panel(writer_events, writer_host, writer_waits, write_presses, panel_checkpoint, contract, result)
    if slot_press is not None and (panel := result.get("panel")) is not None:
        issue(result, (slot_press["owner"], slot_press["region"], slot_press["candidates"])
              == (panel["owner"], panel["slot_region"], panel["candidates"]) == OBSERVED_SAVE_SLOT,
              "Save press uses the same empty slot observed in the settled panel", "failures")
    writes = [line for line in writer_events if re.match(r"^frame=\d+ fes-datasave ", line)]
    saved = WRITE.fullmatch(writes[0]) if len(writes) == 1 else None
    last_write = write_presses.get(writer_labels[-1])
    issue(result, bool(writes), "ordinary slot 0 FES datasave receipt")
    if writes:
        issue(result, saved is not None, "expected exactly one successful ordinary slot 0 write", "failures")
    if saved:
        result["write_receipt"] = writes[0]
        if last_write is not None and write_end is not None:
            issue(result, last_write["frame"] <= int(saved[1]) <= write_end,
                  "ordinary slot write after its guarded menu press and before terminal", "failures")

    groups, body_actions, terminal = continuation_steps(target)
    if continuing:
        terminal = f"Turn at 01/{groups[-1]}/{target}"
    reader_waits = check_reopen_body(reader_events, reader_host, language, groups, result)
    read_actions = [*({"label": label} for label in (TITLE_LOAD, SLOT, CONFIRM)), *body_actions]
    if receipt["reopen_startup"] == "language-select":
        read_actions.insert(0, routes[language]["steps"][0])
    read_presses, read_end = check_presses(reader_events, reader_host, read_actions, reader_waits,
                                         terminal, contract, result, continuing=continuing)
    restore, confirmed = reader_waits.get(24), read_presses.get(CONFIRM)
    all_opens = [line for line in reader_events if re.match(r"^frame=\d+ neko-slot-open ", line)]
    opens = [] if confirmed is None else [line for line in all_opens
             if int(line.split()[0][6:]) >= confirmed["frame"]]
    opened = OPEN.fullmatch(opens[0]) if len(opens) == 1 else None
    result["preconfirmation_slot_opens"] = len(all_opens) - len(opens) if confirmed is not None else None
    if confirmed is None and all_opens:
        result["unbound_slot_opens"] = all_opens
        issue(result, False, "guarded physical Load confirmation for the observed native 247 open")
    else:
        issue(result, bool(opens), "native 247 receipt for the ordinary slot used in restoration")
    if opens:
        issue(result, opened is not None and opened[3] == result["slot"]["sha256"],
              "one native 247 open of the recorded ordinary slot bytes", "failures")
    if opened:
        result["open_receipt"] = opens[0]
        if restore is not None and confirmed is not None:
            issue(result, confirmed["frame"] <= int(opened[1]) <= restore["runtime_body"]["source_frame"],
                  "ordinary slot open after confirmation and before the restored r218 body", "failures")
    end_wait = reader_waits.get(groups[-1])
    issue(result, end_wait is not None and end_wait["frame"] == read_end,
          "settled r485 continuation entry" if continuing else "settled input-free consumer terminal")
    issue(result, not any(re.match(r"^frame=\d+ fes-datasave ", line) for line in reader_events),
          "consumer wrote an ordinary save", "failures")
    if body_actions:
        check_body_edges(reader_events, reader_waits, body_actions, read_presses, result)
        if end_wait is not None and read_end is not None:
            result["terminal_target"] = check_body_target(
                [row for frame, row in observations(reader_events, BODY_TARGET) if frame == read_end],
                patterns_by_frame(reader_host, BODY_PROBE)[read_end], end_wait, read_end, groups[-1], target,
                terminal, result,
            )
    if require_kitchen_assets and end_wait and RESUME in read_presses:
        check_kitchen_assets(reader_events, end_wait, read_presses[RESUME]["frame"], contract, result)
    result["terminal"] = end_wait
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    modes = parser.add_subparsers(dest="mode", required=True)
    probe = modes.add_parser("probe", help="check one existing typed cold-start replay")
    probe.add_argument("--run", type=Path, required=True)
    probe.add_argument("--language", choices=("jp", "cn"), required=True)
    probe.add_argument("--to", choices=PROBES, required=True)
    probe.add_argument("--require-save-hit", action="store_true")
    probe.add_argument("--require-kitchen-assets", action="store_true")
    title = modes.add_parser("title-plan", help="print a typed Load-button observation route")
    title.add_argument("--language", choices=("jp", "cn"), required=True)
    title.add_argument("--startup", choices=("title", "language-select"), required=True)
    plan = modes.add_parser("menu-plan", help="print producer and consumer steps for a menu-capable player")
    plan.add_argument("--language", choices=("jp", "cn"), required=True)
    plan.add_argument("--startup", choices=("title", "language-select"), required=True)
    plan.add_argument("--save-confirm", action="store_true")
    plan.add_argument("--to", type=int, choices=CONTINUATION_WAITS, required=True)
    route = modes.add_parser("menu-route", help="print an executable r218 Save or Load route for the archived patch")
    route.add_argument("--language", choices=("jp", "cn"), required=True)
    route.add_argument("--operation", choices=("save", "load"), required=True)
    route.add_argument("--startup", choices=("title", "language-select"), default="title")
    route.add_argument("--slot-sha256")
    route.add_argument("--to", type=int, choices=CONTINUATION_WAITS, default=218)
    entry_plan = modes.add_parser("save-entry-plan", help="print the single r218 Save entry route")
    entry_plan.add_argument("--language", choices=("jp", "cn"), required=True)
    entry = modes.add_parser("save-entry", help="check the single r218 Save entry and input-free slot probe")
    entry.add_argument("--language", choices=("jp", "cn"), required=True)
    entry.add_argument("--run", type=Path, required=True)
    slot = modes.add_parser("slot", help="print size and SHA-256 for a complete ordinary slot 0")
    slot.add_argument("path", type=Path)
    check = modes.add_parser("roundtrip", help="check the producer and independent consumer logs")
    check.add_argument("--receipt", type=Path, required=True)
    check.add_argument("--to", type=int, choices=CONTINUATION_WAITS, default=337)
    check.add_argument("--require-kitchen-assets", action="store_true")
    check.add_argument("--checkpoint", action="store_true", help="verify an archived-slot candidate reopen")
    args = parser.parse_args()
    if args.mode == "probe" and args.require_save_hit and args.to != "to-218":
        parser.error("--require-save-hit requires --to to-218")
    if getattr(args, "require_kitchen_assets", False) and args.to not in (337, "to-337"):
        parser.error("--require-kitchen-assets requires r337")
    try:
        if args.mode == "title-plan":
            print(json.dumps(title_probe(args.language, args.startup), indent=2))
            return 0
        if args.mode == "menu-plan":
            print(json.dumps(menu_plan(args.language, args.startup, args.save_confirm, args.to), indent=2))
            return 0
        if args.mode == "menu-route":
            print(json.dumps(ordinary_route(args.language, args.operation, args.startup, args.slot_sha256, args.to), indent=2))
            return 0
        if args.mode == "save-entry-plan":
            print(json.dumps(save_entry_route(args.language), indent=2))
            return 0
        if args.mode == "slot":
            print(json.dumps(slot_identity(args.path.absolute()), indent=2))
            return 0
        if args.mode == "probe":
            result = check_probe(args.run, args.language, args.to, args.require_save_hit, args.require_kitchen_assets)
        elif args.mode == "save-entry":
            result = check_save_entry(args.run, args.language)
        else:
            result = check_roundtrip(args.receipt.absolute(), args.to, args.require_kitchen_assets,
                                     checkpoint=args.checkpoint)
    except (OSError, ValueError, KeyError, TypeError, IndexError, ET.ParseError) as error:
        result = {"missing": [str(error)], "failures": []}
    status = 1 if result["failures"] else 2 if result["missing"] else 0
    kind = ("pass", "fail", "insufficient")[status]
    if getattr(args, "checkpoint", False):
        kind = f"checkpoint_{kind}"
    print(json.dumps({"status": kind, **result}, indent=2, ensure_ascii=False))
    return status


if __name__ == "__main__":
    raise SystemExit(main())

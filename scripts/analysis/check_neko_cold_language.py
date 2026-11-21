"""Verify NEKOPARA Japanese and Traditional Chinese cold boots from replay logs only."""

import argparse
import hashlib
import json
import re
import xml.etree.ElementTree as ET
from collections import defaultdict
from pathlib import Path

from check_neko_first_page import INPUT, INSPECT, LAYER, reveal
from stage_neko_cold_language import BODY_HIT_REGION, BODY_PRESS, STARTUP_PROBES, read_extension_contract, startup_probe_routes


PAGES = {
    "jp": ("ショコラ", (
        "「いらっしゃいませー、ソレイユへよーこそー♪」",
        "「本日は店内でお召し上がりですか？　お持ち帰りでしょーか！」",
        "「こちらがメニューになりますのでどーぞ♪」",
    )),
    "cn": ("巧克力", (
        "「歡迎光臨Soleil～♪」",
        "「今天要在店裡用嗎？還是要外帶呢？」",
        "「這是我們的菜單～♪」",
    )),
}
OPENING_PAGE_COUNT = len(PAGES["jp"][1])
PAGE_RECORDS = (
    (59, 58, 58, 57),
    (87, 85, 25, 24),
    (92, 90, 2, 1),
    (98, 96, 3, None),
    (103, 101, 2, None),
    (112, 110, 6, None),
    (117, 115, 2, None),
    (122, 120, 2, None),
    (127, 125, 2, None),
    (136, 134, 6, None),
    (141, 139, 2, None),
    (146, 144, 2, None),
)
EXTENDED_BODY = {
    "jp": (
        (75, "dae6827c8793cc34c3ca8f2d70db04d6f572faf756ae0f8b0a4cdace02487d3a"),
        (63, "6f70387e8bbabe3d88edb08697a919e1af027484a727ea3949ce36372b8334bc"),
        (78, "357d42529effe2bf1a49c853385a36e77c8bee56b066f075d479d62880e2589e"),
        (78, "52aa01c656a7f14a10fb8584941e7c1588f44174b6475538e0584d634358af51"),
        (87, "921beb56ca75fd65b1d2331bebbff00d17d1820329e961269675d4d6a0a970e9"),
        (105, "96ac67db59c53a916a274bac52b791548010d189e4058e55626c2fb3da41709b"),
        (87, "27b17ecd54d918b9b55216e2a8d36800a6b1d2ac475341100b21ce75849c2259"),
        (75, "92b671f5bb787bce18f3c3d1c9c530e6592e9910df828001bc76b6c38f500f2a"),
        (81, "8a100a22895de06c066d91587abba2bca926070a55939af6ae121365eb00b79d"),
    ),
    "cn": (
        (60, "3116278e9683acb4ad6d3a28b92b2d28374f1945784b4fc7f011536a6ceb6f4f"),
        (51, "9a8c29848b359135b1cf6f8d11baca995f66c6d725b6e5fb8bf0216093410dcd"),
        (84, "278353185ef07d1563394269b86b544b0a3939475ed58ca153b073c07d7ec1ff"),
        (69, "b3fcb58ef5804d96e94eaec3a415ce1b415f2278213e9b21fab95809e65503b8"),
        (63, "16cac10063f4679a452abf600d33c384eb50ff591fa4b90b2e04778d8911ace6"),
        (78, "bdbeb436d44f92df0e5e444dd628c16edf3df09a5a8022d2ab40b0a4c455ce44"),
        (48, "73a0955b252811c8af12a3c49597e472604639a81fa0cd97c0bdc3598e033a17"),
        (58, "8cf97208f89b4fde80c92c3ef49687fa13816afb9585572cca2c78d3695b3da9"),
        (56, "00e0691c88c98e67b4ad666ea9dfdd193c54ab0bba490b92c20510a76b583d7e"),
    ),
}
BODY = r"\0text:737472[0]"
COLUMNS = ["cn", "en", "jp"]
PE_SHA256 = "c71c5b85e6eca660474f156d96b0021a53a5f05690dd077d7861d72ff5b11adc"
CSTL_SHA256 = "8085d37fd751947577579ff0ac6eea7a58f5f6863e5df0f4d9561666a6065ae6"
SHELL = re.compile(r'body_wait_shell\(.*?\)=Some\(\("meswnd", (\d+), "01", (\d+), (\d+), (\d+), (\d+)\)\)')
WAIT_DIAGNOSTIC = re.compile(r'^frame (\d+): (probe wait|wait) ')
CURRENT_GENERATION = re.compile(
    r'body_generation\(owner,generation,current_committed\)=Some\(\((\d+), Some\(StoryBodyGeneration \{ '
    r'source_epoch: (\d+), source_frame: (\d+), natural_complete: Some\(\((\d+), (\d+)\)\), '
    r'committed_epoch: Some\((\d+)\), remaining: 0, remaining_segment: 0, pending_text: false, '
    r'drawing: false, skip_pending: false, forced: false, projection: Some\(\(true, ([1-9]\d*)\)\) \}\), true\)\)'
)
COMPOSITED_BODY = re.compile(
    r'body_page\(context,sources,exact_match,composited\)=Some\(\((\d+), Some\(\[CstBodySource \{ '
    r'context_handle: (\d+), load_id: ([1-9]\d*), group: (\d+), absolute_record: (\d+) \}\]\), true, true\)\)'
)
TITLE_GUARD = re.compile(
    r'^frame (\d+): guarded press \[1155, 235\] candidates=\[3, 14, 19, 11, 7\] '
    r'window=Some\(\("title", (\d+)\)\) receiver=Some\(\2\)$'
)
TITLE_RECEIPT = re.compile(
    r'InputRouteReceipt \{ frame: (\d+), key: 0, group: 0, raw_mask: 2, pressed: 1, repeated: false, '
    r'region: 14, receiver: (\d+), response: 0, physical: \[RoutedPhysicalButton \{ source: Mouse, code: 1 \}\] \}'
)
BOOT = re.compile(
    r'^log: language-bootstrap from_setup=(true|false) ui_index=(\d+) ui=Some\("(en|jp|cn)"\) '
    r'main_index=(\d+) main=Some\("(en|jp|cn)"\) interface_table=(\S+) game_table=(\S+)$'
)
LOAD = re.compile(
    r'^frame=(\d+) neko-cstl-load handle=(\d+) requested="[\w.]+/[\w.]+\.cstl" '
    r'source=(".*?") sha256=([0-9a-f]{64}) columns=(\[.*\])$'
)
READ = re.compile(
    r'^frame=(?P<frame>\d+) neko-cstl-read handle=(?P<handle>\d+) input=(?P<group>\d+) '
    r'ordinal=(?P<record>\d+) kind=(?P<kind>0x2[01]) destination=(?P<destination>0x[0-9a-f]+) '
    r'ui=Some\("en"\) main=Some\("(?P<main>jp|cn)"\) '
    r'requested=(?:Some\("(?P<requested>[^"\\]*)"\)|None) source=(?P<source>cstl|raw-cst) '
    r'column=(?:Some\((?P<column>\d+)\)|None) bytes=(?P<bytes>\d+) sha256=(?P<sha256>[0-9a-f]{64})$'
)
QUEUE = re.compile(r'^frame=(\d+) fes-text-sourced-queued bytes=(\d+) first=U\+[0-9A-F]+ fes=\S+ cst=\[Utf8\]$')
GUARD = re.compile(
    r'^frame (\d+): guarded press (\[.*?\]) candidates=(\[.*?\]) '
    r'window=Some\(\("(\w+)", (\d+)\)\) receiver=Some\(\5\)$'
)
POINTER = re.compile(
    r'^frame (\d+): pointer \((-?\d+),(-?\d+)\) unmasked=.*? masked=Some\((\[.*?\])\) '
    r'press_candidates=Ok\((\[.*?\])\)$'
)
RECEIPT = re.compile(
    r'^replay frame=(\d+): verified mouse press (".*"): InputRouteReceipt \{ '
    r'frame: \1, key: 0, group: 0, raw_mask: 2, pressed: 1, repeated: false, '
    r'region: (\d+), receiver: (\d+), response: (\d+), '
    r'physical: \[RoutedPhysicalButton \{ source: Mouse, code: 1 \}\] \}$'
)
CHECKPOINT = re.compile(r'^replay frame=(\d+) (checkpoint|verified)=(".*")$')
PROBE_DETAILS = re.compile(
    r'^masked=(?:Some\((\[.*?\])\)|None) press_candidates=(\[.*?\]) '
    r'window=Some\(\("meswnd", (\d+)\)\) press_receivers=\[(.*)\]$'
)
PROBE_RECEIVER = re.compile(r'\((\d+), (?:Some\((\d+)\)|None)\)')
MESWND_CREATE = re.compile(r'^frame=(\d+) task-create id=(\d+) kind=\d+ parent=\d+ name="meswnd"(?: |$)')


def extension_evidence():
    _, pages, _ = read_extension_contract()
    positions = tuple((row["wait_record"], row["body_absolute"], row["body_ordinal"], None) for row in pages)
    fingerprints = {
        language: tuple((row[f"{language}_utf8_bytes"], row[f"{language}_sha256"]) for row in pages)
        for language in EXTENDED_BODY
    }
    if positions[0] != PAGE_RECORDS[-1] or any(
        rows[0] != EXTENDED_BODY[language][-1] for language, rows in fingerprints.items()
    ):
        raise ValueError("NEKO extension evidence does not match the r146 join")
    return (PAGE_RECORDS + positions[1:],
            {language: EXTENDED_BODY[language] + rows[1:] for language, rows in fingerprints.items()})


def read_boot(run, language, missing, failures):
    index = {"jp": 1, "cn": 2}[language]
    path = run / "01.profile" / "setup.xml"
    ui = None
    if path.is_file():
        config = ET.parse(path).getroot()
        ui = config.findtext("language/id/UI")
        if config.findtext("language/id/main") != str(index):
            failures.append(f"setup.xml does not preserve main={index}")
    else:
        missing.append("private setup.xml")
    return index, ui


def resource_path(path):
    return path.replace("\\", "/").removeprefix("nekopara_vol3/")


def table_path(value):
    return None if value == "None" else resource_path(json.loads(value[5:-1]))


def read_sources(lines):
    loads, reads, queued = defaultdict(list), defaultdict(list), defaultdict(list)
    for line in lines:
        if match := LOAD.fullmatch(line):
            frame, handle, source, digest, columns = match.groups()
            loads[int(frame)].append((int(handle), resource_path(json.loads(source)), digest, json.loads(columns)))
        elif match := READ.fullmatch(line):
            receipt = match.groupdict()
            for field in ("frame", "handle", "group", "record", "bytes"):
                receipt[field] = int(receipt[field])
            if receipt["column"] is not None:
                receipt["column"] = int(receipt["column"])
            reads[(receipt["handle"], receipt["group"], receipt["record"])].append(receipt)
        elif match := QUEUE.fullmatch(line):
            queued[int(match[1])].append(int(match[2]))
    return loads, reads, queued


def read_runtime_body(line, frame, owner, loaded, marker, group, record, body_record):
    generation = CURRENT_GENERATION.search(line)
    source = COMPOSITED_BODY.search(line)
    if not generation or not source or f'frame={frame} state=page scene=01 group={group} record={record} ' not in line:
        return None
    generation_owner, epoch, source_frame, complete_epoch, complete, committed_epoch, opacity = map(int, generation.groups())
    context, source_context, load_id, source_group, absolute_record = map(int, source.groups())
    context_marker = (rf'contexts\(id,scene,loaded,candidate,page\)=\[\({context}, "01", {loaded}, '
                      rf'None, Some\(\({marker}, \d+, {group}, {record}\)\)\)\]')
    if (owner != generation_owner or not epoch or epoch != complete_epoch or epoch != committed_epoch
            or not loaded <= source_frame <= complete <= frame or not marker < frame
            or context != source_context or source_group != group or absolute_record != body_record
            or not re.search(context_marker, line)):
        return None
    return {"context": context, "load_id": load_id, "absolute_record": absolute_record, "source_epoch": epoch,
            "source_frame": source_frame, "natural_completion_frame": complete, "opacity": opacity,
            "exact_match": True, "body_and_prompt_composited": True}


def read_waits(lines, page_records=PAGE_RECORDS):
    waits, progress, presses = {}, {}, []
    frame, submitted, layers = None, False, {}
    for line in lines:
        if INPUT.match(line):
            presses.append(int(line.split()[1].rstrip(":")))
        kind, fields = reveal(line)
        if kind and "gen" in fields:
            key = (fields["id"], fields["gen"])
            if kind == "start" and fields["previous_task"] == "0" and fields["display"] == "Visible":
                progress[key] = {"start": int(fields["task"]), "task": int(fields["task"]),
                                 "pending": False, "forced": False, "playback": 0,
                                 "delta": 0, "paint_times": set(), "complete": None}
            if key in progress:
                state = progress[key]
                if kind in ("start", "tick"):
                    state["task"] = int(fields["task"])
                if kind == "tick" and state["complete"] is None:
                    state["delta"] += int(fields["delta"])
                elif kind == "scan":
                    state["forced"] |= fields["force"] == "true"
                    state["playback"] = int(fields["playback"])
                    if fields["first_pending"] != "None":
                        state["pending"] = True
                    elif (state["pending"] and state["playback"] > 0 and not state["forced"]
                          and fields["separator"] == "false" and state["complete"] is None):
                        state["complete"] = {
                            "task": state["task"], "playback": state["playback"],
                            "end": int(fields["end"]), "tick_delta": state["delta"],
                        }
                elif kind == "paint" and state["complete"] is None:
                    submitted = fields["copy"] == "Full" or (fields["copy"] == "Dirty" and fields["copied_dirty"] == "true")
                    if submitted and fields["failure"] == "None" and re.fullmatch(r"Some\([1-9]\d*\)", fields["changed"]):
                        state["paint_times"].add(state["playback"])
        if match := INSPECT.match(line):
            frame, submitted, layers = int(match[1]), False, {}
        elif frame is not None and line.startswith("list "):
            submitted = line.startswith("list 10 ") and "enabled=true submit=true" in line
        elif frame is not None and submitted and (match := LAYER.match(line)) and int(match[3]):
            layers[(int(match[1]), match[2])] = int(match[4])
        elif diagnostic := WAIT_DIAGNOSTIC.match(line):
            observed = int(diagnostic[1])
            match = SHELL.search(line)
            if match:
                owner, loaded, marker, group, record = map(int, match.groups())
                window = f'({owner}, "meswnd.fes", true, Some(("MES_SHOW", false, Some(Frame {{ after: {observed} }})'
                ready = all(token in line for token in (
                    window, 'trace_enabled=true profile=Some(Nekopara)', f'pe_sha256=Some("{PE_SHA256}")',
                    'startup=Some("kcs.int/main.kcs")', "focused=true paused=false",
                    "vm_waiting_for_message=true pending_messages=0 movies=0",
                ))
                if (ready and 0 <= loaded < marker <= observed and 0 <= group < len(page_records)
                        and page_records[group][0] == record and not any(marker <= press < observed for press in presses)):
                    wait = {"frame": observed, "loaded": loaded, "marker": marker, "owner": owner, "record": record}
                    if diagnostic[2] == "probe wait":
                        body = read_runtime_body(line, observed, owner, loaded, marker, group, record, page_records[group][1])
                        if body:
                            waits.setdefault(group, {**wait, "evidence": "state_probe", "runtime_body": body})
                    elif frame == observed and group < OPENING_PAGE_COUNT:
                        body, prompt = (layers.get((owner, name), 0) for name in (BODY, "keywait"))
                        if body and prompt:
                            natural = [dict(state["complete"], id=int(key[0]), generation=int(key[1]),
                                            pixel_times=sorted(state["paint_times"]))
                                       for key, state in progress.items() if state["start"] == marker
                                       and state["complete"] and not state["forced"]
                                       and state["complete"]["task"] <= observed]
                            waits.setdefault(group, {**wait, "evidence": "frame_inspect", "body_alpha": body,
                                                     "prompt_alpha": prompt, "natural": natural})
            frame, submitted, layers = None, False, {}
    return waits, presses


def has_new_game_receipt(host, presses, loaded, state_mode):
    guards = {(int(match[1]), int(match[2])) for line in host if (match := TITLE_GUARD.fullmatch(line))}
    before_story = [press for press in presses if press < loaded]
    if not before_story:
        return False
    press = before_story[-1]
    for line in host:
        verified = line.startswith("replay frame=") and ": verified mouse press " in line
        if not (verified if state_mode else re.match(r'^frame \d+: pointer \[1155, 235\], routes ', line)):
            continue
        for match in TITLE_RECEIPT.finditer(line):
            frame, owner = map(int, match.groups())
            if (press < frame <= min(press + 3, loaded) and (not state_mode or (press, owner) in guards)):
                return True
    return False


def text_fingerprint(text):
    encoded = text.encode("utf-8")
    return len(encoded), hashlib.sha256(encoded).hexdigest()


def verify_body(wait, group, language, sources, missing, failures, page_record, extended_body, opening_cstl_only,
                *, read_window_start=None, helper_reload_count=0):
    loads, reads, queued = sources
    scene_loads = loads[wait["loaded"]]
    if helper_reload_count:
        if len(scene_loads) != helper_reload_count or any(row != scene_loads[0] for row in scene_loads):
            failures.append(f"CSTL receipts do not match the preserved helper returns at f{wait['loaded']}")
            return
        scene_loads = scene_loads[:1]
    elif read_window_start is not None and scene_loads and all(row == scene_loads[0] for row in scene_loads):
        scene_loads = scene_loads[:1]
    if len(scene_loads) != 1:
        missing.append(f"one CSTL resource receipt for scene 01 at f{wait['loaded']}")
        return
    handle, path, digest, columns = scene_loads[0]
    wait["cstl_resource"] = {"source": path, "sha256": digest, "columns": columns, "handle": handle}
    if (path, digest, columns) != ("scene.int::01.cstl", CSTL_SHA256, COLUMNS):
        failures.append(f"unexpected scene 01 CSTL resource at f{wait['loaded']}")
        return
    runtime_body = wait.get("runtime_body")
    if runtime_body and runtime_body["context"] != handle:
        failures.append(f"composited page does not belong to CSTL handle {handle}")
        return
    wait_record, _, body_ordinal, speaker_ordinal = page_record
    expected_name, expected_pages = PAGES[language]
    if group < OPENING_PAGE_COUNT:
        expected_text = expected_pages[group]
        expected_reads = (("speaker", speaker_ordinal, "0x21", text_fingerprint(expected_name)),
                          ("body", body_ordinal, "0x20", text_fingerprint(expected_text)))
        source_frame = None
    else:
        expected_reads = (("body", body_ordinal, "0x20", extended_body[language][group - OPENING_PAGE_COUNT]),)
        source_frame = runtime_body["source_frame"]
    for key, record, kind, (byte_count, digest) in expected_reads:
        page = f"01/{group}/{wait_record} ordinal={record}"
        first_frame = wait["loaded"] + 1 if read_window_start is None else max(wait["loaded"], read_window_start)
        last_frame = wait["marker"] if source_frame is None else min(wait["marker"], source_frame)
        if source_frame is not None and read_window_start is None:
            first_frame = max(first_frame, source_frame)
        candidates = [receipt for receipt in reads[(handle, group, record)]
                      if first_frame <= receipt["frame"] <= last_frame and receipt["kind"] == kind
                      and receipt["destination"] == "0x00062c6b"]
        if not candidates:
            if opening_cstl_only and group >= OPENING_PAGE_COUNT:
                submission = queued[source_frame]
                wait["queued_body_bytes_at_read"] = submission
                if not submission or submission[0] != byte_count:
                    missing.append(f"first FES body submission matching expected length at 01/{group}/{wait_record}")
                continue
            frame_detail = "" if source_frame is None else f" in f{first_frame}..f{last_frame}"
            missing.append(f"actual 1038 {key} receipt for {page}{frame_detail}")
            continue
        primary = candidates[0]
        wait[f"primary_{key}"] = primary
        expected = (language, language, "cstl", COLUMNS.index(language), byte_count, digest)
        for receipt in candidates if source_frame is not None else (primary,):
            actual = tuple(receipt[field] for field in ("main", "requested", "source", "column", "bytes", "sha256"))
            if actual != expected:
                failures.append(f"wrong primary {key} for {page} at f{receipt['frame']}: {actual!r}; expected {expected!r}")
        if key == "body":
            submission = queued[primary["frame"]]
            wait["queued_body_bytes_at_read"] = submission
            if not submission:
                missing.append(f"primary FES body submission at f{primary['frame']}")
            elif submission[0] != primary["bytes"]:
                failures.append(f"first FES body submission does not match 1038 at f{primary['frame']}")
            if read_window_start is not None and source_frame != primary["frame"]:
                rebuilt = queued[source_frame]
                wait["queued_body_bytes_at_source"] = rebuilt
                if not rebuilt or rebuilt[0] != byte_count:
                    missing.append(f"rebuilt FES body submission matching the 1038 read at f{source_frame}")

    if runtime_body:
        return
    natural = wait["natural"]
    if len(natural) != 1:
        missing.append(f"one natural visible reveal generation at f{wait['marker']}")
    else:
        completion = natural[0]
        if completion["tick_delta"] <= 0 or len(completion["pixel_times"]) < 2:
            missing.append(f"clock and changing submitted glyph pixels at f{wait['marker']}")
        if group == 0 and completion["end"] != len(expected_text):
            failures.append(f"wrong visible first-page reveal end: {completion['end']} (expected {len(expected_text)})")


def save_hit_probe(details, owner, point):
    probe = {"point": point, "owner": owner, "status": "unresolved"}
    if not (match := PROBE_DETAILS.fullmatch(details)):
        return probe
    mask = json.loads(match[1]) if match[1] else []
    candidates = json.loads(match[2])
    receivers = [(int(region), int(receiver) if receiver else None)
                 for region, receiver in PROBE_RECEIVER.findall(match[4])]
    enabled = len(mask) > 8 and mask[8] != 0
    region = mask[0] if mask else None
    control_regions = [candidate for candidate, receiver in receivers
                       if receiver == owner and candidate not in BODY_PRESS["candidates"]]
    probe.update(observed_owner=int(match[3]), hit_region=region, enabled=enabled,
                 candidates=candidates, receivers=receivers, additional_meswnd_regions=control_regions)
    if (enabled and int(match[3]) == owner and region is not None
            and [candidate for candidate, _ in receivers] == candidates
            and any(receiver == owner for _, receiver in receivers)
            and (region != BODY_HIT_REGION or control_regions)):
        probe["status"] = "enabled_candidate"
    return probe


def verify_kitchen_transition(events, origin, target, press_frame, result):
    previous_body, next_body = origin.get("runtime_body"), target.get("runtime_body")
    if not previous_body or not next_body:
        result["missing"].append("composited r218 and r337 bodies around the kitchen edge")
        return
    same_load = all(previous_body[key] == next_body[key] for key in ("context", "load_id"))
    fresh_body = press_frame < next_body["source_frame"] <= target["marker"]
    if not same_load or not fresh_body:
        result["failures"].append("kitchen edge did not preserve the CST load and advance to a fresh body")
    creation_frames = [int(match[1]) for line in events if (match := MESWND_CREATE.match(line))
                       and int(match[2]) == target["owner"] and press_frame < int(match[1]) <= target["frame"]]
    if origin["owner"] != target["owner"] and not creation_frames:
        result["missing"].append("live kitchen meswnd creation after the r218 press for the changed owner")
    elif len(creation_frames) > 1:
        result["failures"].append("ambiguous kitchen meswnd lifetime")
    result["kitchen_transition"] = {"press_frame": press_frame, "from_owner": origin["owner"],
                                    "to_owner": target["owner"], "creation_frames": creation_frames,
                                    "window_rebuilt": bool(creation_frames),
                                    "same_cst_load": same_load, "fresh_body": fresh_body}


def verify_route(route, events, host, waits, presses, result, save_point=None):
    failures, missing = result["failures"], result["missing"]
    steps = route["steps"]
    expected_labels = [step["label"] for step in steps]
    checkpoints, successors = [], []
    for line in events:
        if match := CHECKPOINT.fullmatch(line):
            rows = checkpoints if match[2] == "checkpoint" else successors
            rows.append((int(match[1]), json.loads(match[3])))
    if ([label for _, label in checkpoints] != expected_labels
            or [label for _, label in successors] != expected_labels[:-1]):
        missing.append("exact ordered route checkpoints and successors")
        return
    frames = [frame for frame, _ in checkpoints]
    if (presses != frames[:-1] or any(
        not before < verified <= after
        for before, (verified, _), after in zip(frames, successors, frames[1:])
    )):
        failures.append("unexpected physical downs, successor timing or terminal input")

    guards, receipts, pointers = defaultdict(list), defaultdict(list), defaultdict(list)
    for line in host:
        if match := GUARD.fullmatch(line):
            guards[int(match[1])].append((json.loads(match[2]), json.loads(match[3]), match[4], int(match[5])))
        elif match := RECEIPT.fullmatch(line):
            receipts[json.loads(match[2])].append(tuple(map(int, (match[1], *match.groups()[2:]))))
        elif match := POINTER.fullmatch(line):
            pointers[int(match[1])].append(([int(match[2]), int(match[3])], json.loads(match[4]), json.loads(match[5])))
    for (frame, label), (verified, _), step in zip(checkpoints, successors, steps):
        guard, receipt = guards[frame], receipts[label]
        expected = step["mouse_press"]
        if (len(guard) != 1 or guard[0][:3] != (step["click"], expected["candidates"], step["wait"]["window"])
                or len(receipt) != 1 or not frame < receipt[0][0] <= min(frame + 3, verified)
                or receipt[0][1:] != (expected["region"], guard[0][3], expected["response"])):
            missing.append(f"one guarded physical left press from the live owner for {label}")
        hit = step["wait"]["hit"]
        if not any(point == hit["point"] and len(mask) > 8 and mask[0] == hit["region"] and mask[8] != 0
                   and candidates == expected["candidates"] for point, mask, candidates in pointers[frame]):
            missing.append(f"enabled live hit for {label}")
        if step["wait"]["state"] == "page":
            wait = waits.get(step["wait"]["group"])
            if wait is None or wait["frame"] != frame or len(guard) != 1 or wait["owner"] != guard[0][3]:
                missing.append(f"ready composited body with the press owner for {label}")

    terminal = steps[-1]
    terminal_frame = frames[-1]
    wait = waits.get(terminal["wait"]["group"])
    for point in terminal["probe_points"]:
        prefix = f"frame {terminal_frame}: probe {point} "
        observations = [line for line in host if line.startswith(prefix)]
        if len(observations) != 1:
            missing.append(f"one terminal probe at {point}")
        else:
            details = observations[0][len(prefix):]
            result.setdefault("terminal_probes", []).append(details)
            if point == save_point and wait is not None:
                result["save_hit_probe"] = save_hit_probe(details, wait["owner"], point)
    if wait is None or wait["frame"] != terminal_frame:
        missing.append("ready composited terminal with no input")
    elif terminal["wait"]["record"] == 337 and 24 in waits:
        verify_kitchen_transition(events, waits[24], wait, frames[-2], result)
    if any(re.match(r'^frame=\d+ fes-datasave ', line) for line in events):
        failures.append("unexpected FES datasave during the input-free menu probe route")
    result["terminal"] = {**terminal["wait"], "frame": terminal_frame}
    result["physical_presses"] = len(presses)


def verify(run, language, pages=3, route=None, page_records=PAGE_RECORDS, extended_body=EXTENDED_BODY,
           opening_cstl_only=False, save_point=None):
    failures, missing = [], []
    result = {"run": str(run), "failures": failures, "missing": missing, "pages": {}}
    try:
        identity = json.loads((run / "build_identity.json").read_text(encoding="utf-8-sig"))
        result["identity"] = [identity[key] for key in ("sha256", "source_tree_sha256")]
        if not all(re.fullmatch(r"[0-9a-fA-F]{64}", value) for value in result["identity"]):
            missing.append("complete build identity")
        else:
            result["identity"] = [value.lower() for value in result["identity"]]
        index, ui = read_boot(run, language, missing, failures)
        events = (run / "01.events.log").read_text(encoding="utf-8").splitlines()
        host = (run / "01.host.log").read_text(encoding="utf-8").splitlines()
        boots = [match.groups() for line in events if (match := BOOT.fullmatch(line))]
        if len(boots) != 1:
            missing.append("one language-bootstrap receipt")
        else:
            from_setup, ui_index, ui_prefix, main_index, prefix, interface, table = boots[0]
            interface, table = table_path(interface), table_path(table)
            result["startup"] = {"ui": ui_prefix, "main": prefix, "interface_table": interface, "game_table": table}
            if from_setup != "true" or main_index != str(index) or prefix != language or ui != ui_index:
                failures.append("startup selection does not match the private setup.xml")
            if (ui_index, ui_prefix, interface) != ("0", "en", "language_en.txt"):
                failures.append("UI control is not the unchanged English interface table")
            expected = "config/gamelangtbl_cn.txt" if language == "cn" else None
            if table != expected:
                failures.append(f"unexpected {language} game table: {table}")
        sources = read_sources(events)
        waits, presses = read_waits(host, page_records)
        state_mode = any(line.startswith("replay route=") for line in events)
        if state_mode and "replay_status=completed" not in events:
            missing.append("completed state route")
        if 0 in waits and not has_new_game_receipt(host, presses, waits[0]["loaded"], state_mode):
            missing.append("cold title to New Game receipt")
        if route is not None:
            if [line for line in events if line.startswith("replay_status=")] != ["replay_status=completed"]:
                missing.append("typed route completion")
            if json.loads((run / "01.route.json").read_text(encoding="utf-8-sig")) != route:
                failures.append("staged route differs from the requested typed route")
            verify_route(route, events, host, waits, presses, result, save_point)
        for group, page_record in enumerate(page_records[:pages]):
            wait = waits.get(group)
            if wait is None:
                missing.append(f"visible wait at 01/{group}/{page_record[0]}")
                continue
            result["pages"][group] = wait
            verify_body(wait, group, language, sources, missing, failures, page_record, extended_body, opening_cstl_only)
    except (OSError, ValueError, KeyError, TypeError, ET.ParseError) as error:
        missing.append(str(error))
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--jp", type=Path, required=True, help="Japanese isolated reopen run directory")
    parser.add_argument("--cn", type=Path, required=True, help="Traditional Chinese isolated reopen run directory")
    modes = parser.add_mutually_exclusive_group()
    modes.add_argument("--pages", type=int, choices=range(1, len(PAGE_RECORDS) + 1),
                       help=f"opening pages required (default: {OPENING_PAGE_COUNT})")
    modes.add_argument("--startup-probe", choices=[mode for mode in STARTUP_PROBES if mode != "title"],
                       help="require exact route, physical left presses and terminal probes")
    parser.add_argument("--opening-cstl-only", action="store_true",
                        help="for old players, require full 1038 evidence only for the first three opening pages")
    parser.add_argument("--require-save-hit", action="store_true",
                        help="for to-218, also require an enabled Save candidate on the live meswnd")
    parser.add_argument("--summary", action="store_true", help="omit per-page receipts from the JSON output")
    args = parser.parse_args()
    page_records, extended_body, routes = PAGE_RECORDS, EXTENDED_BODY, {}
    save_point = None
    pages = args.pages or OPENING_PAGE_COUNT
    if args.require_save_hit and args.startup_probe != "to-218":
        parser.error("--require-save-hit requires --startup-probe to-218")
    if args.startup_probe:
        pages = STARTUP_PROBES[args.startup_probe][0] + 1
        if args.opening_cstl_only and pages > len(PAGE_RECORDS):
            parser.error("extension probes require every page's 1038 receipt")
        try:
            _, routes, _, extension = startup_probe_routes(args.startup_probe)
            if extension and extension.get("save_button_probe"):
                save_point = extension["save_button_probe"]["point"]
            if pages > len(PAGE_RECORDS):
                page_records, extended_body = extension_evidence()
        except (OSError, ValueError, TypeError, KeyError) as error:
            parser.error(str(error))
    elif args.opening_cstl_only:
        parser.error("--opening-cstl-only requires a typed --startup-probe")
    results = {language: verify(run, language, pages, routes.get(language), page_records, extended_body,
                                args.opening_cstl_only, save_point)
               for language, run in (("jp", args.jp), ("cn", args.cn))}
    if args.require_save_hit:
        for result in results.values():
            if result.get("save_hit_probe", {}).get("status") != "enabled_candidate":
                result["missing"].append("enabled r218 Save candidate with a live meswnd receiver")
    if args.summary:
        for result in results.values():
            result["observed_pages"] = len(result.pop("pages"))
    failures = []
    if args.jp.resolve() == args.cn.resolve():
        failures.append("JP and CN must use separate isolated run directories")
    jp, cn = (results[language].get("identity") for language in ("jp", "cn"))
    if jp and cn and jp != cn:
        failures.append("JP and CN must be from the same binary and source tree")
    status = 1 if failures or any(result["failures"] for result in results.values()) else (
        2 if any(result["missing"] for result in results.values()) else 0)
    print(json.dumps({"status": ("pass", "fail", "insufficient")[status], "required_pages": pages,
                      "cstl_receipt_scope": "first_three_pages" if args.opening_cstl_only else "every_page",
                      "failures": failures, **results},
                     indent=2, ensure_ascii=False))
    return status


if __name__ == "__main__":
    raise SystemExit(main())

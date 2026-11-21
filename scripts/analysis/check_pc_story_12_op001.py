"""Verify the Hatsumira op001 replay receipts without starting a player."""

import argparse
from collections import defaultdict
import json
from pathlib import Path
import re
import struct

from check_pc_body_successor import BODY, ENTER, LEFT, ROOT_WAIT, TASK_CREATED, WINDOW, read_host


OPENING_WAITS = (69, 72, 92, 100, 109, 148, 151, 154, 157, 161, 164, 167, 170, 174, 179)
RW_MOSAIC_WAITS = (*OPENING_WAITS, 195, 206, 210, 214, 218, 222)
BODY_BYTES = (46, 136, 120, 30, 58, 44, 42, 82, 80, 26, 38, 76, 16, 22, 14, 92,
              22, 120, 18, 72, 30)
PE = "c48d383c335c5b283bb0c17bd9f14b2b8bfef4a2e3d635365157b1c69498df78"
TITLE = re.compile(r"frame (\d+): Hatsumira title propagated flow=(\d+) title=(\d+) and resumed root")
RELEASE = re.compile(r"frame (\d+): Hatsumira Enter release observed")
FIELDS = ("owner", "region", "receiver", "context", "load", "loaded", "marked",
          "epoch", "source_frame", "read_frame", "complete")
PAGE = re.compile(r"frame (\d+): Hatsumira page group=(\d+) record=(\d+) HatsumiraPage \{ "
                  + ", ".join(fr"{key}: (\d+)" for key in FIELDS) + r" \}")
ROUTE_PAGE = re.compile(r"replay frame=(\d+) state=page scene=op001 group=(\d+) record=(\d+)")
POST_PAGE = re.compile(r"frame (\d+): Hatsumira post-op handle=(\d+) played=(\d+) released=(\d+) HatsumiraPage \{ "
                       + ", ".join(fr"{key}: (\d+)" for key in FIELDS) + r" \}")
SOURCE = re.compile(
    r"cst-source-record CstSourceRecordObservation \{ source: CstBodySource \{ context_handle: (\d+), "
    r"load_id: (\d+), group: (\d+), absolute_record: (\d+) \}, frame: (\d+), "
    r"probe: Hatsumira12, native: 327, kind: 32, stored_codec: Cp932, output_codec: Cp932, "
    r"raw_bytes: ([1-9]\d*), output_bytes: \6, body_tagged: true, localized: false \}"
)
POST_SOURCE = re.compile(SOURCE.pattern.replace("Hatsumira12,", "Hatsumira12PostOp,"))
CACHE_RESET = re.compile(
    r"hatsumira-body-cache-color-reset task=(\d+) target=0x23af8 bytes=(\d+) source=CstBodySource \{ "
    r"context_handle: (\d+), load_id: (\d+), group: (\d+), absolute_record: (\d+) \}"
)
EVENT = re.compile(r"frame=(\d+) (.*)")
RENDER_NATIVE = re.compile(r"native-call index=(520|527|528|530|531|853|854|855|905) arguments=\d+ bytes \[([0-9a-f]+)\]")
WIPE_COMMAND = 'debug="[command] rwipe wipe04b 15"'
RW_MOSAIC_COMMAND = 'debug="[command] rwmos 60 10 0"'
TITLE_RESUME = "message-resume message=[2, 640, 443, 0]"
ENTER_RESUME = "message-resume message=[2, 0, 0, 0]"


def require(condition, reason):
    if not condition:
        raise ValueError(reason)


def verify_title(frame, flow, title, receipts, waits, resumes):
    actual = [r for r in receipts if r["frame"] == frame and r["pressed"]]
    expected = [(key, region, receiver) for key in (0, 2)
                for region, receiver in zip((3, 15, 37, 7), (0, title, title, flow))]
    require([(r["key"], r["region"], r["receiver"]) for r in actual] == expected
            and all((r["group"], r["raw_mask"], r["pressed"], r["response"], r["repeated"], r["physical"])
                    == (0, 2, 1, 1, False, LEFT) for r in actual)
            and frame in waits and resumes[frame] == [TITLE_RESUME], "title receipt or root resume mismatch")


def verify_enter(frame, region, receiver, receipts, waits, resumes, *, until=None):
    consumers = [r for r in receipts if r["frame"] >= frame and (until is None or r["frame"] <= until)
                 and r["pressed"] and r["response"] == 0]
    require(len(consumers) == 1, "expected exactly one body consumer")
    got = consumers[0]
    require((got["frame"], got["region"], got["receiver"], got["key"], got["group"], got["raw_mask"],
             got["pressed"], got["repeated"], got["physical"])
            == (frame, region, receiver, 2, 0, 1, 1, False, ENTER)
            and frame in waits and resumes[frame] == [ENTER_RESUME], "Enter receipt or root resume mismatch")
    return got


def render_trace(events, after_input, through, command):
    calls = []
    commands = []
    for line in events:
        event = EVENT.fullmatch(line)
        if not event or not after_input < int(event[1]) <= through:
            continue
        frame, detail = int(event[1]), event[2]
        if detail.endswith(command):
            commands.append(frame)
        elif call := RENDER_NATIVE.fullmatch(detail):
            packet = bytes.fromhex(call[2])
            require(len(packet) % 4 == 0, "misaligned render native packet")
            words = tuple(word for (word,) in struct.iter_unpack("<I", packet))
            calls.append((frame, int(call[1]), words))
    return commands, calls


def verify_r92_wipe(events, tasks, before, after):
    commands, calls = render_trace(events, before["frame"], after["marked"], WIPE_COMMAND)
    require(len(commands) == 2 and commands[0] < commands[1],
            "expected r81 and r85 wipe04b commands after the second Enter")
    first_wipe, second_wipe = commands
    created = [task for task, lifetimes in tasks.items()
               if (first_wipe, 39, before["receiver"], "rwipe") in lifetimes]
    require(len(created) == 1, "wipe04b task missing or ambiguous")
    task = created[0]
    require(not any(first_wipe < frame < second_wipe for frame, *_ in tasks[task]),
            "wipe task id was reused before the second wipe")
    trace = [(frame, native, words) for frame, native, words in calls
             if first_wipe <= frame < second_wipe and words and words[0] == task]
    configured = [(frame, native, words) for frame, native, words in trace if native in (530, 905)]
    require(len(configured) == 1 and configured[0][1] == 530,
            "expected one ordinary 530 configuration for wipe04b")
    configured_frame, _, record = configured[0]
    require(configured_frame == first_wipe and len(record) == 5
            and record[1:4] == (13, 0, 0x472FC800) and record[4] != 0,
            "wipe04b packet has the wrong duration, mode, depth, or mask")
    enabled = [frame for frame, native, words in trace
               if native == 531 and words == (task, 1)]
    require(len(enabled) == 1 and configured_frame <= enabled[0],
            "wipe04b missing its single enable")
    polled = [frame for frame, native, words in trace
              if native == 520 and words == (task,) and enabled[0] <= frame]
    require(polled, "first wipe04b was never polled before the second wipe")
    disabled = [frame for frame, native, words in trace
                if native == 531 and words == (task, 0) and polled[0] <= frame]
    return {"task": task, "configured": configured_frame, "duration": record[1],
            "enabled": enabled[0], "first_poll": polled[0], "last_poll": polled[-1],
            "disabled": disabled, "second_wipe": second_wipe}


def verify_draw(events, tasks, before, after, *, duration=58, command="rdraw 60"):
    expected_command = f'debug="[command] {command}"'
    commands, calls = render_trace(events, before["frame"], after["marked"], expected_command)
    require(len(commands) == 1, f"expected one {command} before the r{after['record']} wait")
    created = [task for task, lifetimes in tasks.items()
               if (commands[0], 38, before["receiver"], "rdraw") in lifetimes]
    require(len(created) == 1, "rdraw task missing or ambiguous")
    task = created[0]
    require(not any(commands[0] < frame <= after["marked"] for frame, *_ in tasks[task]),
            "rdraw task id was reused before the body wait")
    trace = [(frame, native, words) for frame, native, words in calls
             if frame >= commands[0] and words and words[0] == task]
    configured = [(frame, words) for frame, native, words in trace if native == 527]
    require(len(configured) == 1 and len(configured[0][1]) == 4, "missing native 527 draw packet")
    configured_frame, (_, actual_duration, mode, depth_bits) = configured[0]
    require((actual_duration, mode, depth_bits) == (duration, 0, 0x472FC800),
            f"native 527 packet does not match {command} timing and depth")
    enabled = [frame for frame, native, words in trace if native == 528 and words == (task, 1)]
    require(len(enabled) == 1 and configured_frame <= enabled[0], "rdraw missing its single enable")
    polled = [frame for frame, native, words in trace if native == 520 and words == (task,)
              and enabled[0] <= frame]
    disabled = [frame for frame, native, words in trace if native == 528 and words == (task, 0)]
    require(polled and len(disabled) == 1 and enabled[0] + duration <= polled[-1] <= disabled[0] < after["marked"],
            "rdraw did not reach the configured duration and disable before the body wait")
    return {"task": task, "configured": configured_frame, "duration": duration, "mode_bits": mode,
            "depth": 45_000, "enabled": enabled[0], "first_poll": polled[0], "last_poll": polled[-1],
            "disabled": disabled[0]}


def verify_single_wipe(events, tasks, before, after, command, duration):
    commands, calls = render_trace(events, before["frame"], after["marked"], f'debug="[command] {command}"')
    require(len(commands) == 1, f"expected one {command} before the next body wait")
    start = commands[0]
    created = [task for task, lifetimes in tasks.items()
               if (start, 39, before["receiver"], "rwipe") in lifetimes]
    require(len(created) == 1, "wipe task missing or ambiguous")
    task = created[0]
    trace = [(frame, native, words) for frame, native, words in calls
             if start <= frame and words and words[0] == task]
    configured = [(frame, words) for frame, native, words in trace if native == 530]
    require(len(configured) == 1 and configured[0][0] == start and len(configured[0][1]) == 5
            and configured[0][1][1:4] == (duration, 0, 0x472FC800) and configured[0][1][4] != 0,
            "wipe native 530 duration, mode, depth or mask differs")
    enabled = [frame for frame, native, words in trace if native == 531 and words == (task, 1)]
    disabled = [frame for frame, native, words in trace if native == 531 and words == (task, 0)]
    require(len(enabled) == len(disabled) == 1 and disabled[0] <= after["marked"],
            "wipe must enable and disable exactly once before the body wait")
    require(not any(start < frame <= disabled[0] for frame, *_ in tasks[task]),
            "wipe task id was reused before completion")
    polled = [frame for frame, native, words in trace if native == 520 and words == (task,)
              and enabled[0] <= frame <= disabled[0]]
    require(polled and polled[-1] >= enabled[0] + duration - 1,
            "wipe did not naturally reach the configured duration")
    return {"task": task, "configured": start, "duration": duration, "enabled": enabled[0],
            "first_poll": polled[0], "last_poll": polled[-1], "disabled": disabled[0]}


def verify_history(observations, receipts, presses, waits, resumes, dialogue):
    require([key for _, key in presses] == ["MouseButton(0)", "Keyboard(13)"], "historical press sequence")
    click, down = (f for f, _ in presses)
    verify_title(click + 1, 6, 8, receipts, waits, resumes)
    receipt = verify_enter(down + 1, 109, 8, receipts, waits, resumes)

    def body_wait(frame, text, layers):
        return (frame in waits and f'pe_sha256=Some("{PE}")' in text
                and "message_wait=Some(3113) vm_waiting_for_message=true" in text
                and {(6, "flow", "false", "LOOP_GAME", frame),
                     (110, "meswnd.fes", "true", "MES_SHOW", frame)}
                == {(int(owner), name, key, block, int(after))
                    for owner, name, key, block, after in WINDOW.findall(text)}
                and layers.get(110, {}).get(BODY, 0) > 0)

    samples = [(frame, layers[110][BODY]) for frame, text, layers in observations
               if body_wait(frame, text, layers)]
    before = dict(samples).get(down)
    after = [(f, alpha) for f, alpha in samples if f > down + 1 and alpha > (before or 0)]
    require(before and after and {"[command] pcm 1 NNC_op001_002", "[input]"} <= dialogue[down + 1],
            "missing historical first or successor wait, pixels, or dialogue event")
    frame, alpha = after[-1]
    return {"status": "successor_observed", "first": {"frame": down, "body_alpha": before},
            "successor": {"frame": frame, "body_alpha": alpha}, "receipt": receipt}


def verify_rw_mosaic(events, tasks, before, after):
    commands, calls = render_trace(events, before["frame"], after["marked"], RW_MOSAIC_COMMAND)
    require(len(commands) == 1, "expected one rwmos 60 10 0 before r206")
    start = commands[0]
    prepares = [(frame, words) for frame, native, words in calls if native == 853 and frame >= start]
    require(len(prepares) == 1 and len(prepares[0][1]) == 6, "missing unique rwmos native 853 preparation")
    prepared, words = prepares[0]
    task, duration = words[:2]
    lifetimes = [item for item in tasks[task] if item[0] <= after["marked"]]
    require(lifetimes and lifetimes[-1][0] <= prepared
            and lifetimes[-1][1:] == (50, before["receiver"], "rwmos"), "wrong rwmos task lifetime")
    require(duration >= 2, "rwmos duration is too short to create a retained effect")
    trace = [(frame, native, packet) for frame, native, packet in calls
             if frame >= prepared and packet and packet[0] == task]
    controls = [(i, frame, native, packet) for i, (frame, native, packet) in enumerate(trace) if native != 520]
    require([(native, packet) for _, _, native, packet in controls]
            == [(853, words), (854, (task,)), (855, (task, 1)), (855, (task, 0))],
            "rwmos must prepare, capture the second frame, play and stop in order")
    captured = controls[1][1]
    play_index, started, _, _ = controls[2]
    stop_index, stopped, _, _ = controls[3]
    polls = [frame for frame, native, packet in trace[play_index + 1:stop_index]
             if native == 520 and packet == (task,)]
    require(polls and started < polls[-1] <= stopped
            and stopped - started >= duration - 1, "rwmos stopped before the timed polling sequence completed")
    return {"task": task, "command": start, "prepared": prepared, "packet": words,
            "captured": captured, "started": started,
            "first_poll": polls[0], "last_poll": polls[-1], "stopped": stopped}


def verify_post_op(host, events, before, waits, resumes, receipts, tasks):
    snapshots = [tuple(map(int, m.groups())) for line in host if (m := POST_PAGE.fullmatch(line))]
    require(len(snapshots) == 1, "expected one post-OP checkpoint")
    frame, handle, played, released, *fields = snapshots[0]
    page = dict(zip(FIELDS, fields))
    require(events.count(f"replay frame={frame} state=page scene=OPMOVIE_END group=0 record=42") == 1,
            "missing OPMOVIE_END 0/42 route receipt")
    down = before["frame"]
    media = []
    for line in events:
        if not (event := EVENT.fullmatch(line)):
            continue
        at, detail = int(event[1]), event[2]
        if not down < at <= frame or not detail.startswith("native-call "):
            continue
        if detail.lower().endswith(f' movie_load handle={handle} resource="movie.int/hatsuop.mpg"'):
            media.append((at, "load"))
        elif detail.endswith(f" movie_hatsumira_op_started handle={handle}"):
            media.append((at, "play"))
        elif detail.endswith(f" movie_hatsumira_op_released handle={handle}"):
            media.append((at, "natural_release"))
    require(len(media) == 3 and media[0][1] == "load"
            and down < media[0][0] <= played < released
            and media[1:] == [(played, "play"), (released, "natural_release")],
            "missing exact hatsuop load/play/natural-release order")
    require(frame > down + 3 and (page["region"], page["receiver"]) == (before["region"], before["receiver"])
            and all(not any(down < at <= frame for at, *_ in tasks[before[key]])
                    for key in ("region", "receiver"))
            and page["load"] > before["load"] and released <= page["loaded"] <= page["marked"] < frame
            and page["loaded"] <= page["read_frame"] <= page["source_frame"] <= page["complete"] <= frame
            and frame in waits and page["epoch"] > 0 and page["owner"] != page["receiver"],
            "post-OP CST lifetime, generation, or input target mismatch")
    read = (page["context"], page["load"], 0, 41, page["read_frame"], 126)
    require(any((event := EVENT.fullmatch(line)) and (source := POST_SOURCE.fullmatch(event[2]))
                and int(event[1]) == read[4] and tuple(map(int, source.groups())) == read
                for line in events),
            "missing post-OP r41 same-load source read")
    receipt = verify_enter(down + 1, before["region"], before["receiver"], receipts, waits, resumes)
    return {"frame": frame, "media_handle": handle, "media": media, "page": page, "receipt": receipt}


def verify(host, events, meta, mode, *, require_cache_reset=False, checkpoints=None):
    observations, receipts, presses = read_host(host)
    waits, resumes, sources, tasks, dialogue = set(), defaultdict(list), {}, defaultdict(list), defaultdict(set)
    cache_resets = defaultdict(list)
    for line in events:
        if not (event := EVENT.fullmatch(line)):
            continue
        frame, detail = int(event[1]), event[2]
        if detail.startswith("message-resume "):
            resumes[frame].append(detail)
        elif (wait := ROOT_WAIT.fullmatch(detail)) and wait[1] == "0xc29":
            waits.add(frame)
        elif source := SOURCE.fullmatch(detail):
            key = tuple(map(int, source.groups()[:5]))
            require(key[-1] == frame, "CST read frame differs from event frame")
            sources[key] = int(source[6])
        elif reset := CACHE_RESET.fullmatch(detail):
            task, size, *origin = map(int, reset.groups())
            cache_resets[(task, *origin)].append((frame, size))
        elif task := TASK_CREATED.match(detail):
            tasks[int(task[1])].append((frame, int(task[2]), int(task[3]), task[4]))
        elif 'debug="[command] pcm 1 NNC_op001_002"' in detail:
            dialogue[frame].add("[command] pcm 1 NNC_op001_002")
        elif 'debug="[input]"' in detail:
            dialogue[frame].add("[input]")

    require(meta.get("index") == 12 and meta.get("exit_code") == 0
            and meta.get("captures") == [] and not meta.get("first_engine_error"),
            "sample 12 result must have exited cleanly with zero captures")
    if mode == "historical":
        return verify_history(observations, receipts, presses, waits, resumes, dialogue)
    require(meta.get("mode") == "state" and meta.get("replay_status") == "completed"
            and events.count("replay_status=completed") == 1, "state replay did not complete")

    titles = [tuple(map(int, m.groups())) for line in host if (m := TITLE.fullmatch(line))]
    require(len(titles) == 1, "expected one verified title click")
    click, flow, title = titles[0]
    verify_title(click, flow, title, receipts, waits, resumes)
    pages = [dict(zip(("frame", "group", "record", *FIELDS), map(int, m.groups())))
             for line in host if (m := PAGE.fullmatch(line))]
    waits_by_mode = {"observe": OPENING_WAITS[:1], "enter": OPENING_WAITS[:2],
                     "r92": OPENING_WAITS[:3], "r100": OPENING_WAITS[:4], "opening": OPENING_WAITS,
                     "r195": RW_MOSAIC_WAITS[:16], "r206": RW_MOSAIC_WAITS[:17], "r222": RW_MOSAIC_WAITS}
    if checkpoints is None:
        checkpoints = [{"group": group, "wait": record, "body": record - 1, "bytes": BODY_BYTES[group]}
                       for group, record in enumerate(waits_by_mode[mode])]
    expected = [(point["group"], point["wait"]) for point in checkpoints]
    require([(p["group"], p["record"]) for p in pages] == expected, "wrong terminal page sequence")
    route_pages = [tuple(map(int, m.groups())) for line in events if (m := ROUTE_PAGE.fullmatch(line))]
    require(route_pages == [(p["frame"], p["group"], p["record"]) for p in pages],
            "page logs disagree")
    for p, point in zip(pages, checkpoints):
        frame = p["frame"]
        source = (p["context"], p["load"], p["group"], point["body"], p["read_frame"])
        require(click < p["loaded"] <= p["marked"] < frame and frame in waits
                and p["loaded"] <= p["read_frame"] <= p["source_frame"] <= p["complete"] <= frame
                and p["load"] > 0 and p["epoch"] > 0 and p["owner"] != p["receiver"]
                and sources.get(source) == point["bytes"],
                f"r{p['record']} has no matching wait, native 327 body length, or completion")
        for task, kind, parent, name in (
                (p["owner"], 34, p["receiver"], "meswnd"),
                (p["region"], 6, p["receiver"], "bottom button"),
                (p["receiver"], 0, flow, None)):
            created = [entry for entry in tasks[task] if entry[0] <= frame]
            require(created and created[-1][0] > click and created[-1][1:3] == (kind, parent)
                    and (name is None or created[-1][3] == name), f"task {task} has no matching live parent chain")

    first = pages[0]
    if require_cache_reset:
        origin = (first["context"], first["load"], 0, 68)
        source_bytes = sources[(*origin, first["read_frame"])]
        matched = [frame for frame, size in cache_resets[(first["receiver"], *origin)]
                   if first["read_frame"] <= frame <= first["source_frame"] and size == source_bytes + 3]
        require(matched, "r68 cache reset receipt missing or does not match the committed page")
        first["cache_reset_frame"] = matched[0]
    clicks = [(click - 1, "MouseButton(0)")]
    releases = [int(m[1]) for line in host if (m := RELEASE.fullmatch(line))]
    if mode == "observe":
        require(presses == clicks and not releases
                and all(r["frame"] < first["frame"] or not r["pressed"] for r in receipts),
                "body input in the observation run")
        return {"status": "first_wait_verified", "pages": pages}

    dispatched = pages if mode == "post_op_r42" else pages[:-1]
    require(presses == clicks + [(page["frame"], "Keyboard(13)") for page in dispatched],
            "expected one physical Enter at each nonterminal page")
    require(releases == [page["frame"] for page in pages[1:]], "missing device-scan release")
    verified = []
    for index, (before, after) in enumerate(zip(pages, pages[1:])):
        down = before["frame"]
        verified.append(verify_enter(down + 1, before["region"], before["receiver"], receipts, waits, resumes,
                                     until=after["frame"] if index < len(pages) - 2 or mode == "post_op_r42" else None))
        require(after["frame"] > down + 3, "missing device-scan release")
        require(all(before[key] == after[key] for key in ("context", "load", "loaded", "owner", "region", "receiver"))
                and after["epoch"] > before["epoch"] and after["source_frame"] > down and after["marked"] > down
                and all(not any(down < frame <= after["frame"] for frame, *_ in tasks[before[key]])
                        for key in ("owner", "region", "receiver")),
                "successor load, generation, or task lifetime mismatch")
    if mode == "enter":
        return {"status": "successor_verified", "pages": pages, "receipt": verified[0]}
    wipe = verify_r92_wipe(events, tasks, pages[1], pages[2])
    if mode == "r92":
        return {"status": "r92_verified", "pages": pages, "receipts": verified, "wipe": wipe}
    draws = [verify_draw(events, tasks, before, after) for before, after in zip(pages[2:4], pages[3:5])]
    status = "opening_segment_verified" if mode == "opening" else f"{mode}_verified"
    result = {"status": status, "pages": pages, "receipts": verified, "wipe": wipe, "draws": draws}
    if len(pages) >= 16:
        result["r195_wipe"] = verify_single_wipe(events, tasks, pages[14], pages[15], "rwipe wipe05 40", 38)
    if len(pages) >= 17:
        result["r206_rwmos"] = verify_rw_mosaic(events, tasks, pages[15], pages[16])
    if mode in ("r261", "r619", "post_op_r42"):
        result["r237_draw"] = verify_draw(events, tasks, pages[22], pages[23], duration=18, command="rdraw 20")
    if mode in ("r619", "post_op_r42"):
        result["r279_wipe"] = verify_single_wipe(events, tasks, pages[29], pages[30], "rwipe wipe06 30", 28)
        result["nonadjacent_sources"] = [
            {"wait": point["wait"], "source": point["body"], "empty": point["empty"]}
            for point in checkpoints if point["body"] + 1 != point["wait"]
        ]
    if mode == "post_op_r42":
        result["post_op"] = verify_post_op(host, events, pages[-1], waits, resumes, receipts, tasks)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("run", type=Path, help="12 log directory or its parent")
    parser.add_argument("--mode", choices=("observe", "enter", "r92", "r100", "opening", "r195", "r206", "r222",
                                           "r261", "r619", "post_op_r42", "historical"), default="enter")
    parser.add_argument("--require-cache-reset", action="store_true",
                        help="require the native-15 color-reset receipt for the first page's CST origin")
    args = parser.parse_args()
    if args.require_cache_reset and args.mode == "historical":
        parser.error("--require-cache-reset needs a guarded state replay")
    prefix = args.run / "12"
    metadata = prefix.with_suffix(".json")
    if prefix.is_dir():
        prefix /= "12"
    elif not metadata.is_file() and args.run.name == "12":
        metadata = args.run.with_suffix(".json")
    try:
        meta = json.loads(metadata.read_text(encoding="utf-8"))
        if meta.get("status") == "resource_deferred":
            print(json.dumps({"status": "resource_deferred"}))
            return 2
        host = prefix.with_suffix(".host.log").read_text(encoding="utf-8", errors="replace").splitlines()
        events = prefix.with_suffix(".events.log").read_text(encoding="utf-8", errors="replace").splitlines()
        checkpoints = None
        if args.mode in ("r261", "r619", "post_op_r42"):
            from plan_pc_story_09_12_20_next import HATSUMIRA_MODES, hatsumira_config, hatsumira_pages

            limit, _ = HATSUMIRA_MODES[args.mode]
            route_file = Path(meta["route_json"]).resolve()
            require(route_file == prefix.with_suffix(".route.json").resolve(), "route and logs differ")
            require(json.loads(route_file.read_text(encoding="utf-8")) == hatsumira_config(args.mode)["12"]["route"],
                    "wrong Hatsumira continuation config")
            checkpoints = hatsumira_pages(Path(meta["game"]))[:limit]
        result = verify(host, events, meta, args.mode, require_cache_reset=args.require_cache_reset,
                        checkpoints=checkpoints)
    except (KeyError, OSError, ValueError) as error:
        print(json.dumps({"status": "rejected", "reason": str(error)}, ensure_ascii=True))
        return 1
    print(json.dumps(result, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

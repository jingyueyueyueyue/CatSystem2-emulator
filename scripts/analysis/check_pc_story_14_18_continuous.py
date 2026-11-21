"""Verify the consecutive 14 and 18 body routes from local sources or replay logs."""

import argparse
import ast
import json
from pathlib import Path
import re
from struct import unpack, unpack_from

from check_pc_story_14_entry import BODY_SNAPSHOT, KEYBOARD_CHAIN
from check_pc_story_18_gate import NATIVE, PRESS


MANIFESTS = {
    14: "pc_story_14_continuous_g3.json",
    18: "pc_story_18_continuous_g3.json",
}
EXTENDED_MANIFESTS = {
    14: "pc_story_14_continuous_g5.json",
    18: "pc_story_18_continuous_g6.json",
}
CAMPANELLA = (
    r"CampanellaWait \{ context: (\d+), load: (\d+), script: (\d+), owner: (\d+), "
    r"plane: (\d+), string: (\d+), generation: (\d+), marked: (\d+), opaque: (\d+) \}"
)


def require(condition, reason):
    if not condition:
        raise ValueError(reason)


def verify_source(index, review):
    from cst_variants import digest, expand_cst, grisaia_root, read_entries
    from plan_island_bad02 import checkpoint_rows
    from run_playthrough_batch import INVENTORY, load_inventory

    archive, member = review["member"].split("::")
    root = grisaia_root() if index == 14 else Path(load_inventory(INVENTORY)[0][18]["root"])
    matches = [data for entry, _, data in read_entries(root / archive, review["name_seed"])
               if entry["name"].lower() == member.lower().encode("ascii")]
    require(len(matches) == 1 and digest(matches[0]) == review["sha256"], "CST member digest differs")
    body, _ = expand_cst(matches[0])
    waits, _ = checkpoint_rows(body)
    _, _, offsets, strings = unpack_from("<IIII", body)
    for group, page in enumerate(review["pages"]):
        start = unpack_from("<II", body, 16 + group * 8)[1]
        row = waits[group]
        require((row["group"], row["record"], row["kind"]) == (group, page["wait"], "input")
                and row["record"] - start == page["wait_ordinal"]
                and row["nonempty_body_records"] == page["body"] + page.get("newline_only", [])
                and [record - start for record in page["body"]] == page["body_ordinals"],
                f"static checkpoint differs at g{group}")
        for record in page.get("newline_only", []):
            pos = 16 + strings + unpack_from("<I", body, 16 + offsets + record * 4)[0]
            require(body[pos + 2: body.index(0, pos + 2)] == b"\\n",
                    f"r{record} differs from the newline-only record")
    return {"index": index, "static_pages": [page["wait"] for page in review["pages"]]}


def press_rows(host, title):
    presses = {}
    for match in PRESS.finditer(host):
        frame, key, group, mask, pressed = map(int, match.groups()[:5])
        region, receiver, response = map(int, match.groups()[6:9])
        physical = match[10]
        source = "Mouse" if mask == 2 else "Keyboard"
        code = 1 if mask == 2 else 13
        require(group == 0 and mask == (2 if frame == title else 1) and pressed == 1 and match[6] == "false"
                and physical == f"[RoutedPhysicalButton {{ source: {source}, code: {code} }}]",
                f"invalid physical receipt at f{frame}")
        presses.setdefault(frame, []).append((key, region, receiver, response))
    require(sum(map(len, presses.values())) == len(re.findall(r"\bpressed: [1-9]\d*,", host)),
            "unparsed positive receipt")
    return presses


def native_frames(events):
    calls = {}
    for line in events.splitlines():
        match = NATIVE.match(line)
        if match:
            frame, slot, size = map(int, match.groups()[:3])
            data = bytes.fromhex(match[4])
            if len(data) == size and size % 4 == 0:
                calls.setdefault(frame, []).append((slot, unpack(f"<{size // 4}I", data), line[match.end():]))
    return calls


def verify_keyboard(index, rows, target, candidates, *, group=None):
    require(rows and [row for row in rows if row[3] == 0] == [(2, *target, 0)]
            and rows[-1] == (2, *target, 0), "wrong keyboard consumer")
    if index == 14:
        require(target == (61, 8), "14 body input receiver differs")
        if group in (6, 7):
            require(target in candidates and len(candidates) == len(set(candidates))
                    and all(pair in KEYBOARD_CHAIN for pair in candidates), "invalid 14 live binding")
        else:
            require(not candidates or candidates == KEYBOARD_CHAIN, "14 historical binding differs")
            candidates = KEYBOARD_CHAIN
        chain = [(0, *pair, 1) for pair in candidates]
        chain += [(2, *pair, int(pair != target)) for pair in candidates[:candidates.index(target) + 1]]
        require(rows == chain, "14 keyboard callback order differs")
        return
    require(target in candidates and len(set(candidates)) == len(candidates), "invalid 18 live binding")
    eligible = [(key, *pair) for key in (0, 2) for pair in candidates]
    for key, region, receiver, _ in rows:
        require((key, region, receiver) in eligible, "18 keyboard callback order differs")
        eligible = eligible[eligible.index((key, region, receiver)) + 1:]


def valid_14_source_order(group, source, marker, receipt, settled):
    delay = {5: 1, 6: 30}.get(group)
    return receipt <= source <= settled and (
        source >= marker or group == 2 or (delay is not None and source == receipt and marker == receipt + delay)
    )


def verify_14_g6_commands(calls, context, receipt, marker):
    commands = []
    for frame in range(receipt, marker + 1):
        rows = calls.get(frame, [])
        for (slot, words, _), (following, _, detail) in zip(rows, rows[1:]):
            if (slot == 327 and len(words) == 4 and words[0] == context and words[2] == 6
                    and following == 1 and detail.startswith(' debug="[command] ')):
                commands.append((frame, words[3], detail.removeprefix(' debug="[command] ').removesuffix('"')))
    require(commands == [
        (receipt, 0, "bgm 0 bgm03"),
        (receipt, 1, "se 1 loop se150b"),
        (receipt, 2, "se 1 fade 0 50 50"),
        (receipt, 3, "wait 30"),
        (marker, 4, "se 0 fade 60 50 25"),
        (marker, 5, "pcm MOB_op_001"),
    ], "14 g6 did not execute the pinned OP commands across wait 30")


def classify(index, result, host, events, review):
    require(result.get("status") in ("completed", "checkpoint_reached")
            and (result.get("index"), result.get("mode"),
             result.get("replay_status"), result.get("exit_code"), result.get("timed_out"),
             result.get("first_engine_error"), result.get("captures"))
            == (index, "state", "completed", 0, False, None, [])
            and events.rstrip().splitlines()[-1:] == ["replay_status=completed"],
            f"route did not complete ({result.get('status')})")
    snapshot = BODY_SNAPSHOT if index == 14 else CAMPANELLA
    scene = "OP " if index == 14 else ""
    if index == 14:
        arm_pattern = rf"^frame (\d+): 14 armed OP g(\d+)/(\d+) region=(\d+) receiver=(\d+) body=({snapshot})$"
    else:
        arm_pattern = rf"^frame (\d+): 18 armed g(\d+)/(\d+) body=({snapshot}) bottom=\((\d+), (\d+)\) candidates=(\[[\d, ()]+\])$"
    arms = list(re.finditer(arm_pattern, host, re.MULTILINE))
    settled = list(re.finditer(rf"^frame (\d+): {index} settled {scene}g(\d+)/(\d+) after release body=({snapshot})$",
                              host, re.MULTILINE))
    downs = [(int(frame), key) for frame, key in re.findall(r"^frame (\d+): down (.*)$", host, re.MULTILINE)]
    advances = len(review["pages"]) - 1
    require(len(arms) == len(settled) == advances and len(downs) == advances + 1
            and downs[0][1] == "MouseButton(0)", f"expected one title click and exactly {advances} guarded body presses")
    live_candidates = {}
    if index == 14:
        armed_frames = {int(arm[1]) for arm in arms}
        for frame, pairs in re.findall(r"^frame (\d+): 14 Enter candidates=(\[[\d, ()]+\])$", host, re.M):
            at = int(frame)
            require(at in armed_frames and at not in live_candidates, "unmatched 14 live candidate receipt")
            live_candidates[at] = ast.literal_eval(pairs)
    title = downs[0][0] + 1
    presses = press_rows(host, title)
    resumes = re.findall(r"^frame=(\d+) message-resume message=(\[.*\])$", events, re.MULTILINE)

    def resume(frame):
        return [packet for at, packet in resumes if int(at) == frame]

    title_rows = [(0, 3, 0, 1), (0, 37, 11, 0)] if index == 14 else [(0, 11, review["title_owner"], 0)]
    require(presses.pop(title, []) == title_rows and resume(title) == [review["title_resume"]],
            "title click receipt or root VM resume differs")

    calls = native_frames(events)
    loads = [(frame, text.removeprefix(' debug="sscript loaded > ').removesuffix('"'))
             for frame, rows in calls.items() for slot, _, text in rows
             if slot == 1 and text.startswith(' debug="sscript loaded > ')]
    require([scene for _, scene in loads] == review["loads"], "CST load sequence differs")
    loaded = loads[-1][0]
    handles = [words[0] for slot, words, _ in calls[loaded] if slot == 325 and len(words) == 2]
    require(len(handles) == 1, "missing unique CST load context")
    context = handles[0]

    previous_body = None
    markers = []
    recreated = [(int(at), int(task)) for at, task in
                 re.findall(r"^frame=(\d+) task-create id=(\d+)\b", events, re.MULTILINE)]
    for group, (arm, after) in enumerate(zip(arms, settled)):
        frame, source_group, source_wait = map(int, arm.groups()[:3])
        finish, next_group, next_wait = map(int, after.groups()[:3])
        if index == 14:
            target = tuple(map(int, arm.groups()[3:5]))
            body = tuple(map(int, arm.groups()[6:]))
            instance_len, guarded_ids = 3, (8, 12, 61)
            expected_marker = body[-1]
        else:
            target = tuple(map(int, arm.groups()[-3:-1]))
            body = tuple(map(int, arm.groups()[4:-3]))
            instance_len, guarded_ids = 6, (*body[2:6], target[0])
            expected_marker = body[-2]
            require(target[1] == body[2], "18 Enter receiver is not the live body script")
        next_body = tuple(map(int, after.groups()[4:]))
        source, successor = review["pages"][group:group + 2]
        require((source_group, source_wait, next_group, next_wait)
                == (group, source["wait"], group + 1, successor["wait"])
                and downs[group + 1] == (frame, "Keyboard(13)") and frame + 4 <= finish
                and context == body[0] and body[1] > 0
                and body[:instance_len] == next_body[:instance_len]
                and 0 < body[instance_len] < next_body[instance_len]
                and (previous_body is None or previous_body == arm[6 if index == 14 else 4])
                and title < loaded < expected_marker <= frame,
                f"checkpoint or body generation differs at g{group}")
        next_marker = next_body[-1 if index == 14 else -2]
        require(frame < next_marker < finish, f"g{group + 1} did not execute after the press")
        if group:
            require(frame == markers[-1]["settled_frame"], f"g{group} was not armed on the settled page")
        candidates = ast.literal_eval(arm.groups()[-1]) if index == 18 else live_candidates.get(frame, [])
        verify_keyboard(index, presses.pop(frame + 1, []), target, candidates, group=group)
        require(resume(frame + 1) == ["[2, 0, 0, 0]"],
                f"{index} body input did not resume the root VM on the receipt frame")
        if index == 14:
            source_receipt = markers[-1]["pressed_frame"] if group else title
            require(body[2] == 12
                    and valid_14_source_order(group, body[4], expected_marker, source_receipt, frame)
                    and valid_14_source_order(next_group, next_body[4], next_marker, frame + 1, finish),
                    "14 body owner or submission order differed")
            if next_group == 6 and next_body[4] < next_marker:
                verify_14_g6_commands(calls, context, frame + 1, next_marker)
        else:
            require(body[-1] > 0 and next_body[-1] > 0, "18 composed opaque body pixels missing")
        require(not any(frame < at <= finish and task in guarded_ids for at, task in recreated),
                f"task lifetime changed after g{group}")
        if group == 0:
            verify_page(calls, context, group, expected_marker, source)
        early_body_start = frame + 1 if index == 18 and next_group == 4 else next_marker
        verify_page(calls, context, next_group, next_marker, successor, early_body_start)
        if index == 18:
            if next_group == 4:
                verify_18_draw(events, calls, body[2], frame + 1, next_marker)
            verify_reveal(calls, body[3], next_marker, finish)
            if group == 0:
                verify_reveal(calls, body[3], expected_marker, frame)
        previous_body = after[4]
        markers.append({"pressed_frame": frame + 1, "wait": [next_group, next_wait], "settled_frame": finish})
    require(not presses and result.get("last_frame") == markers[-1]["settled_frame"],
            "unexpected input or terminal frame after the last verified body")
    return {"index": index, "gate": "continuous_body_route_settled", "advances": markers}


def verify_reveal(calls, owner, start, finish):
    require(any((69, (owner, 13, 0, 0), "") in rows
                for at, rows in calls.items() if start <= at < finish),
            "18 reveal completion missing")


def verify_18_draw(events, calls, script, start, marker):
    draws = [(int(frame), int(owner)) for frame, owner in re.findall(
        rf'^frame=(\d+) task-create id=(\d+) kind=38 parent={script} name="rdraw"', events, re.M)
        if start <= int(frame) <= marker]
    require(len(draws) == 1, "18 g4 did not create one rdraw task")
    created, owner = draws[0]
    configs = [words for slot, words, _ in calls.get(created, [])
               if slot == 527 and len(words) == 4 and words[0] == owner]
    require(len(configs) == 1 and 0 < configs[0][1] <= 30, "18 g4 rdraw 527 duration argument differs")
    trace = [(int(frame), int(slot), int(scalar), query, int(duration), int(now), int(end), rendered)
             for frame, slot, scalar, query, duration, now, end, rendered
             in re.findall(rf"^frame=(\d+) campanella-rdraw-native index=(\d+) task={owner} scalar=(\d+) "
                           r"query=(\S+) .*? duration=(\d+) now=(\d+) started=\d+ end=(\d+) "
                           r"active=\d+ available=true rendered=(true|false) ", events, re.M)
             if created <= int(frame) <= marker]
    duration = configs[0][1]
    prepared = [at for at, slot, _, _, frames, *_ in trace if slot == 527 and frames == duration]
    require(prepared == [created]
            and any(slot == 528 and scalar == 1 and frames == duration for _, slot, scalar, _, frames, *_ in trace)
            and any(at > created and slot == 520 and query == "0" and frames == duration
                    and now >= end and rendered == "true"
                    for at, slot, _, query, frames, now, end, rendered in trace),
            "18 g4 rdraw did not report configured duration, start and natural query completion")


def verify_page(calls, context, group, frame, page, body_start=None):
    body_start = frame if body_start is None else body_start
    seen = []
    markers = []
    for at in range(body_start, frame + 1):
        rows = calls.get(at, [])
        for (slot, words, _), (next_slot, _, detail) in zip(rows, rows[1:]):
            if next_slot != 1:
                continue
            if slot == 327 and len(words) == 4 and words[0] == context and words[2] == group:
                if detail.startswith(' debug="[message] ') and words[3] in page["body_ordinals"]:
                    seen.append(words[3])
            if detail == ' debug="[input]"':
                markers.append((at, slot, words))
    require(seen == page["body_ordinals"] and markers == [(frame, 326, (context, group, page["wait_ordinal"]))],
            f"executed body reads or wait lookup differ at g{group}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("batch", type=Path, nargs="?")
    parser.add_argument("--indices", type=int, choices=MANIFESTS, nargs="+", default=[14, 18])
    parser.add_argument("--static", action="store_true")
    parser.add_argument("--extended", action="store_true", help="Use the g5 (14) and g6 (18) manifests")
    args = parser.parse_args()
    if args.static == (args.batch is not None):
        parser.error("provide either a batch directory or --static")
    reports = []
    for index in args.indices:
        try:
            manifest = (EXTENDED_MANIFESTS if args.extended else MANIFESTS)[index]
            review = json.loads(Path(__file__).with_name(manifest).read_text(encoding="utf-8"))["review"]
            if args.static:
                reports.append(verify_source(index, review))
                continue
            prefix = args.batch / str(index) / str(index)
            result = json.loads((args.batch / f"{index}.json").read_text(encoding="utf-8"))
            reports.append(classify(index, result, prefix.with_suffix(".host.log").read_text(encoding="utf-8"),
                          prefix.with_suffix(".events.log").read_text(encoding="utf-8"), review))
        except (OSError, ValueError) as error:
            reports.append({"index": index, "gate": "unproven", "reason": str(error)})
    print(json.dumps(reports, indent=2))
    return int(any(report.get("gate") == "unproven" for report in reports))


if __name__ == "__main__":
    raise SystemExit(main())


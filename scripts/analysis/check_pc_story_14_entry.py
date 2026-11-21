"""Verify 14's OP route receipts or inspect its local CST successor."""

import argparse
import json
from pathlib import Path
import re
from struct import unpack


NATIVE = re.compile(r"^frame=(\d+) native-call index=(\d+) arguments=(\d+) bytes \[([0-9a-f]*)\](.*)$")
PRESS = re.compile(
    r"InputRouteReceipt \{ frame: (\d+), key: (\d+), group: (\d+), raw_mask: (\d+), "
    r"pressed: 1, repeated: false, region: (\d+), receiver: (\d+), response: (\d+), "
    r"physical: \[RoutedPhysicalButton \{ source: (Mouse|Keyboard), code: (\d+) \}\] \}"
)
FIRST_PAGE = (0, 5, 6)
SECOND_PAGE = (1, 0, 1)
BODY_SNAPSHOT = (
    r"Grisaia14Body \{ context: (\d+), load: (\d+), owner: (\d+), "
    r"epoch: (\d+), source_frame: (\d+), marked: (\d+) \}"
)
KEYBOARD_CHAIN = [(3, 0), (26, 12), (24, 12), (23, 12), (21, 12), (19, 12),
                  (16, 12), (27, 12), (61, 8), (28, 12), (7, 6)]


def classify(result, host, events):
    report = {"gate": "unproven", "input_authorized": False}
    if (result.get("index") != 14 or result.get("exit_code") != 0
            or result.get("timed_out", False) is not False or result.get("first_engine_error") is not None
            or result.get("replay_status") not in (None, "frame_limit", "completed")):
        return report | {"reason": "wrong sample or failed replay"}

    live = result.get("mode") == "state"
    downs = [(int(frame), key) for frame, key in re.findall(r"^frame (\d+): down (.*)$", host, re.MULTILINE)]
    if (not downs or len(downs) > 2 or downs[0][1] != "MouseButton(0)"
            or (len(downs) == 2 and (downs[1][1] != "Keyboard(13)" or downs[0][0] >= downs[1][0]))):
        return report | {"reason": "expected one title down and at most one Enter down"}
    title_frame = downs[0][0] + 1
    keyboard_frame = downs[1][0] + 1 if len(downs) == 2 else None

    presses = {}
    for match in PRESS.finditer(host):
        frame, key, group, mask, region, owner, response = map(int, match.groups()[:7])
        presses.setdefault(frame, []).append((key, group, mask, region, owner, response, match[8], int(match[9])))
    if presses.pop(title_frame, []) != [(0, 0, 2, 3, 0, 1, "Mouse", 1), (0, 0, 2, 37, 11, 0, "Mouse", 1)]:
        return report | {"reason": "title press chain differs"}
    keyboard = presses.get(keyboard_frame, [])
    if set(presses) - {keyboard_frame} or bool(keyboard) != (keyboard_frame is not None):
        return report | {"reason": "unexpected or missing physical press"}
    logical_keys = [item[0] for item in keyboard]
    if keyboard and (set(logical_keys) != {0, 2} or logical_keys != sorted(logical_keys)):
        return report | {"reason": "unexpected logical keyboard dispatch order"}
    if keyboard and (
        any((group, mask, source, code) != (0, 1, "Keyboard", 13)
            for _, group, mask, _, _, _, source, code in keyboard)
        or [item for item in keyboard if item[5] == 0] != [(2, 0, 1, 61, 8, 0, "Keyboard", 13)]
        or keyboard[-1][3:6] != (61, 8, 0)
    ):
        return report | {"reason": "first Enter did not consume at the recorded bottom button"}
    if len(PRESS.findall(host)) != len(re.findall(r"pressed: [1-9]\d*,", host)):
        return report | {"reason": "unknown positive input receipt"}
    expected_resumes = [(title_frame, "2, 512, 354, 0")]
    if keyboard:
        expected_resumes.append((keyboard_frame, "2, 0, 0, 0"))
    resumes = [(int(f), packet) for f, packet in re.findall(
        r"^frame=(\d+) message-resume message=\[(.*)\]$", events, re.MULTILINE
    ) if int(f) in {title_frame, keyboard_frame}]
    if resumes != expected_resumes:
        return report | {"reason": "entry or page input did not resume the expected VM message"}

    calls = []
    for number, line in enumerate(events.splitlines(), 1):
        match = NATIVE.match(line)
        if match:
            frame, slot, size = map(int, match.groups()[:3])
            data = bytes.fromhex(match[4])
            if len(data) == size and size % 4 == 0:
                calls.append((number, frame, slot, unpack(f"<{size // 4}I", data), match[5]))
    loads = [call for call in calls if call[2] == 1 and call[4].startswith(' debug="sscript loaded > ')]
    if [call[4] for call in loads] != [
        ' debug="sscript loaded > start"', ' debug="sscript loaded > OP"',
    ]:
        return report | {"reason": "normal entry scene sequence differs"}
    load_line, loaded_frame = loads[-1][:2]
    load_calls = [call for call in calls if call[0] < load_line and call[1:3] == (loaded_frame, 325) and len(call[3]) == 2]
    if len(load_calls) != 1:
        return report | {"reason": "missing a unique OP CST context load"}
    context = load_calls[0][3][0]

    def page(frame, expected):
        group, body, wait = expected
        local = [call for call in calls if call[0] > load_line and call[1] == frame]
        messages = [call for call in local if call[2] == 1 and call[4].startswith(' debug="[message] ')]
        markers = [call for call in local if call[2] == 1 and call[4] == ' debug="[input]"']
        if len(messages) != 1 or len(markers) != 1:
            return False
        message_line, marker_line = messages[0][0], markers[0][0]
        body_reads = [call for call in local if call[0] < message_line and call[2] == 327]
        wait_reads = [call for call in local if message_line < call[0] < marker_line and call[2] == 326]
        if not body_reads or not wait_reads:
            return False
        words = body_reads[-1][3]
        return (len(words) == 4 and (words[0], *words[2:]) == (context, group, body)
                and wait_reads[-1][3] == (context, group, wait))

    def wait_frames(expected):
        group, _, wait = expected
        candidates = {frame for number, frame, slot, words, _ in calls
                      if number > load_line and slot == 326 and words == (context, group, wait)}
        return sorted(frame for frame in candidates if page(frame, expected))

    first_frames = [frame for frame in wait_frames(FIRST_PAGE)
                    if keyboard_frame is None or frame < keyboard_frame]
    if not title_frame < loads[0][1] < loaded_frame or len(first_frames) != 1:
        return report | {"reason": "g0 body and executed wait proof absent"}
    report |= {"gate": "g0_wait_observed", "context": context, "first_wait": [0, 6],
               "first_marker_frame": first_frames[0]}
    if keyboard:
        next_frames = [frame for frame in wait_frames(SECOND_PAGE) if frame >= keyboard_frame]
        if len(next_frames) != 1:
            return report | {"reason": "first Enter has a receipt but g1 source proof is absent"}
        report |= {
            "gate": "g1_wait_observed", "next_wait": [1, 8], "press_frame": keyboard_frame,
            "successor_marker_frame": next_frames[0],
            "keyboard_receipt_count": len(keyboard), "consuming_region": 61, "consuming_receiver": 8,
        }
    if live:
        live_result = verify_live_route(result, host, events, report, keyboard)
        return report | live_result
    return report


def verify_live_route(result, host, events, report, keyboard):
    failed = {"gate": "unproven"}
    if (result.get("status") not in ("completed", "checkpoint_reached") or result.get("replay_status") != "completed"
            or result.get("captures") != [] or not events.rstrip().endswith("\nreplay_status=completed")
            or report["gate"] != "g1_wait_observed"):
        return failed | {"reason": "guarded route did not complete at g1/8"}
    armed = re.findall(r"^frame (\d+): 14 armed OP g0/6 region=61 receiver=8 body=" + BODY_SNAPSHOT + r"$",
                       host, re.MULTILINE)
    settled = re.findall(r"^frame (\d+): 14 settled OP g1/8 after release body=" + BODY_SNAPSHOT + r"$",
                         host, re.MULTILINE)
    if len(armed) != 1 or len(settled) != 1:
        return failed | {"reason": "missing current-generation proofs before Enter and after release"}
    start, context, load, owner, epoch, source, marked = map(int, armed[0])
    end, next_context, next_load, next_owner, next_epoch, next_source, next_marked = map(int, settled[0])
    press_frame = report["press_frame"]
    expected = [(0, region, receiver, 1) for region, receiver in KEYBOARD_CHAIN]
    expected += [(2, region, receiver, int(region != 61)) for region, receiver in KEYBOARD_CHAIN[:9]]
    if ([(key, region, receiver, response) for key, _, _, region, receiver, response, _, _ in keyboard] != expected
            or press_frame != start + 1 or end < start + 4
            or not (context == report["context"] == next_context and load == next_load > 0
                    and owner == next_owner == 12 and next_epoch > epoch > 0)
            or not (marked == report["first_marker_frame"] <= source <= start
                    < press_frame <= next_marked == report["successor_marker_frame"] <= next_source <= end)):
        return failed | {"reason": "receipt or successor provenance differs from the armed OP instance"}
    return {"gate": "g1_wait_settled_after_live_enter", "armed_frame": start,
            "settled_frame": end, "load": load, "owner": owner, "body_epochs": [epoch, next_epoch]}


def static_successor():
    from cst_variants import digest, expand_cst, grisaia_root, read_entries
    from plan_island_bad02 import checkpoint_rows

    contract = json.loads(Path(__file__).with_name("pc_story_14_first_two_pages.json")
                          .read_text(encoding="utf-8"))["first_two_pages"]
    member = contract["source"]
    archive, scene = member["member"].split("::")
    matches = [data for entry, _, data in read_entries(grisaia_root() / archive)
               if entry["name"].lower() == scene.encode("ascii").lower()]
    if len(matches) != 1 or digest(matches[0]) != member["member_sha256"]:
        raise ValueError("expected exactly one matching UPDATE01.INT::op.cst member")
    body, _ = expand_cst(matches[0])
    waits, boundaries = checkpoint_rows(body)
    expected = [{"group": page["group"], "record": page["wait_absolute"], "kind": "input",
                 "nonempty_body_records": [page["body_absolute"]]}
                for page in (contract[key] for key in ("first_page", "next_page", "later_page"))]
    controls = [row for row in boundaries if row["group"] == 2]
    if waits[:3] != expected or [(row["record"], row["command"]) for row in controls] != [
        (9, "frameoff fade 0"), (10, "wait"), (49, "rdraw 60"), (50, "wait"),
    ]:
        raise ValueError("OP wait or transition order differs")
    return {"gate": "op_successor_static", "member": member["member"], "sha256": digest(matches[0]),
            "waits": waits[:3], "g2_controls": controls, "input_authorized": False}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("log_directory", type=Path, nargs="?")
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--require-next-wait", action="store_true",
                      help="Require an Enter receipt and executed g1 wait")
    mode.add_argument("--require-live-route", action="store_true",
                      help="Require the live route's current and successor body proofs")
    mode.add_argument("--static-successor", action="store_true",
                      help="Read local UPDATE01.INT::op.cst and verify the g1/8 to g2/78 record sequence")
    args = parser.parse_args()
    if args.static_successor:
        try:
            print(json.dumps(static_successor(), indent=2))
        except (OSError, ValueError) as error:
            parser.exit(1, f"{error}\n")
        return 0
    base = args.log_directory
    if base is None:
        parser.error("provide a batch log directory or --static-successor")
    run = json.loads((base / "14.json").read_text(encoding="utf-8"))
    if run.get("mode") == "state":
        base /= "14"
    if run.get("status") == "resource_deferred":
        print(json.dumps({"gate": "unproven", "reason": "replay was resource_deferred"}))
        return 1
    report = classify(
        run,
        (base / "14.host.log").read_text(encoding="utf-8"),
        (base / "14.events.log").read_text(encoding="utf-8"),
    )
    print(json.dumps(report, indent=2))
    passed = report["gate"] in ("g0_wait_observed", "g1_wait_observed", "g1_wait_settled_after_live_enter") and "reason" not in report
    if args.require_next_wait:
        passed = passed and report["gate"] in ("g1_wait_observed", "g1_wait_settled_after_live_enter")
    if args.require_live_route:
        passed = passed and report["gate"] == "g1_wait_settled_after_live_enter"
    return 0 if passed else 1


if __name__ == "__main__":
    raise SystemExit(main())

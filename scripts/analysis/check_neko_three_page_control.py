"""Verify the fixed three-page NEKOPARA control from existing replay logs."""

import argparse
import json
import re
from pathlib import Path

from check_neko_first_page import INSPECT, LAYER, reveal


PE = "c71c5b85e6eca660474f156d96b0021a53a5f05690dd077d7861d72ff5b11adc"
BODY = r"\0text:737472[0]"
DOWN = re.compile(r"^frame (\d+): down (.+)$")
RECEIPT = re.compile(
    r"InputRouteReceipt \{ frame: (\d+), key: 0, group: 0, raw_mask: 2, "
    r"pressed: 1, repeated: false, region: 52, receiver: (\d+), response: 1, "
    r"physical: \[RoutedPhysicalButton \{ source: Mouse, code: 1 \}\] \}"
)
WINDOW = re.compile(
    r'\((\d+), "meswnd\.fes", (true|false), '
    r'Some\(\("MES_SHOW", false, Some\(Frame \{ after: (\d+) \}\)\)\)'
)
CHECKPOINTS = {
    1250: (952, 0, 59, True),
    1600: (1439, 1, 87, False),
    1710: (1439, 1, 87, True),
    1840: (1439, 1, 87, True),
    1841: (1841, 2, 92, False),
    2040: (1841, 2, 92, True),
    2400: (1841, 2, 92, True),
}


def collect(lines):
    reveals, by_marker, snapshots, presses, receipts = {}, {}, {}, [], {}
    inspection = None
    for line in lines:
        kind, fields = reveal(line)
        if kind and "gen" in fields:
            key = (fields["id"], int(fields["gen"]))
            progress = reveals.setdefault(key, {"task": None, "complete": None, "forced": False})
            if kind == "start":
                progress["task"] = int(fields["task"])
                if fields["previous_task"] == "0" and fields["display"] == "Visible":
                    by_marker.setdefault(progress["task"], set()).add(key)
            elif kind == "scan":
                progress["forced"] |= fields["force"] == "true"
                if fields["first_pending"] == "None" and progress["complete"] is None:
                    progress["complete"] = progress["task"]
            continue
        if match := DOWN.match(line):
            presses.append((int(match[1]), match[2]))
        for frame, owner in RECEIPT.findall(line):
            receipts.setdefault(int(frame), set()).add(int(owner))
        if match := INSPECT.match(line):
            inspection = int(match[1])
            snapshots[inspection] = {"submitted": False, "layers": {}}
        if inspection is None:
            continue
        snapshot = snapshots[inspection]
        if line.startswith("list "):
            snapshot["submitted"] = line.startswith("list 10 ") and "enabled=true submit=true" in line
        elif snapshot["submitted"] and (match := LAYER.match(line)) and int(match[3]):
            snapshot["layers"].setdefault(int(match[1]), {})[match[2]] = int(match[4])
        elif line.startswith(f"frame {inspection}: wait "):
            marker = CHECKPOINTS.get(inspection, (None,))[0]
            keys = by_marker.get(marker, set())
            snapshot["reveal"] = dict(reveals[next(iter(keys))]) if len(keys) == 1 else {}
            snapshot["generation"] = next(iter(keys))[1] if len(keys) == 1 else None
            snapshot["wait"] = line
            inspection = None
    return snapshots, presses, receipts


def verify(snapshots, presses, receipts, events):
    failures, results = [], {}
    for frame, (marker, group, record, should_be_ready) in CHECKPOINTS.items():
        snapshot = snapshots.get(frame, {})
        wait = snapshot.get("wait", "")
        windows = WINDOW.findall(wait)
        owner = int(windows[0][0]) if len(windows) == 1 else None
        wait_key = owner is not None and windows[0][1] == "true"
        layers = snapshot.get("layers", {}).get(owner, {})
        progress = snapshot.get("reveal", {})
        body, prompt = layers.get(BODY, 0), layers.get("keywait", 0)
        completed = progress.get("complete")
        observation = (
            owner is not None and f'Some("{PE}")' in wait and 'startup=Some("kcs.int/main.kcs")' in wait
            and f"Some(({marker}, 2, {group}, {record}))" in wait
            and "focused=true paused=false message_wait=Some(3437) vm_waiting_for_message=true pending_messages=0 movies=0" in wait
            and int(windows[0][2]) == frame and progress.get("forced") is False and body > 0
        )
        ready = bool(observation and wait_key and completed is not None and prompt > 0)
        if not observation or ready != should_be_ready or not should_be_ready and (completed is not None or prompt > 0):
            failures.append(f"unexpected readiness evidence at f{frame}")
        results[frame] = {"owner": owner, "generation": snapshot.get("generation"), "completion_task": completed,
                          "body_alpha": body, "prompt_alpha": prompt, "wait_key": wait_key, "ready": ready}
    if not results[1841]["wait_key"]:
        failures.append("f1841 must exercise the WAIT_KEY=true false-positive control")
    if presses != [(frame, "MouseButton(0)") for frame in (400, 680, 1250, 1840)]:
        failures.append("unexpected press schedule")
    for before in (1250, 1840):
        owner = results[before]["owner"]
        if receipts.get(before + 1) != {owner} or f"frame={before + 1} message-resume message=[2, 625, 478, 0]" not in events:
            failures.append(f"missing verified message-window receipt after f{before}")
    if len({results[frame]["generation"] for frame in (1250, 1710, 2040)}) != 3:
        failures.append("the three ready pages must have distinct reveal generations")
    return results, failures


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("run", type=Path, help="fixed replay directory containing 01.host.log and 01.events.log")
    args = parser.parse_args()
    with (args.run / "01.host.log").open(encoding="utf-8", errors="replace") as host:
        snapshots, presses, receipts = collect(line.rstrip("\n") for line in host)
    with (args.run / "01.events.log").open(encoding="utf-8", errors="replace") as events:
        resumes = {line.rstrip("\n") for line in events if " message-resume " in line}
    results, failures = verify(snapshots, presses, receipts, resumes)
    print(json.dumps({"passed": not failures, "checkpoints": results, "failures": failures}, indent=2))
    return int(bool(failures))


if __name__ == "__main__":
    raise SystemExit(main())

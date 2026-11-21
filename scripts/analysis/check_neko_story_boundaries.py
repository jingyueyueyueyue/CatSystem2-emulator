"""Verify NEKOPARA branch and ending milestones from PC replay events."""

import argparse
import hashlib
import json
import math
from pathlib import Path
import re


STORY = re.compile(r"story frame=(\d+) state=(\w+) scene=([-\w]+) group=(-?\d+) record=(-?\d+)")
DOWN = re.compile(r"frame (\d+): down \S+")
LOAD = re.compile(r"frame=(\d+) neko-boundary-load handle=([1-9]\d*) member=([\w.]+) sha256=([0-9a-f]{64})")
BRANCH = re.compile(r"frame=(\d+) neko-branch-flags scene=01 group=491 record=5386 flag1003=(\d+) flag1004=(\d+)")
CLEAR = re.compile(r"frame=(\d+) neko-cleared-title owner=([1-9]\d*) flag550=1")
UNLOCK = re.compile(r"frame=(\d+) local-achievement-unlocked ACHIEVEMENT_2_15")
BRANCH_PATHS = {
    True: ("01_01end_1.cst", "h01.cst"),
    False: ("01_01end_2.cst", "01_01_2.cst", "01_02.cst"),
}
RESOURCE_PATH = Path(__file__).resolve().parents[2] / "simulator/crates/desktop/src/neko_chapter_resources.txt"
RESOURCES = {}
for line in RESOURCE_PATH.read_text(encoding="ascii").splitlines():
    if line and not line.startswith("#"):
        archive, member, cst_sha, cstl_sha, *waits = line.split()
        RESOURCES[member] = (archive, cst_sha, cstl_sha, tuple(map(int, waits)))


def matches(lines, pattern):
    if isinstance(pattern, str):
        pattern = re.compile(pattern)
    return [match.groups() for line in lines if (match := pattern.fullmatch(line))]


def unique(rows, label):
    if len(rows) != 1:
        raise ValueError(f"{label}: expected one receipt, found {len(rows)}")
    return rows[0]


def film_evidence(lines, movie, lower, upper, downs):
    key = rf" {movie} handle=([1-9]\d*) playback=([1-9]\d*)"
    start, handle, playback, count = unique(
        matches(lines, r"frame=(\d+) neko-film-start" + key + r" count=(\d+)"), f"{movie} start")
    end, end_handle, end_playback, natural = unique(
        matches(lines, r"frame=(\d+) neko-film-end" + key + r" natural=(true|false)"), f"{movie} end")
    start, end = int(start), int(end)
    if (handle, playback, count, natural) != (end_handle, end_playback, "1", "true") or not lower <= start < end <= upper:
        raise ValueError(f"{movie}: wrong lifetime, repeat count or completion")
    if any(start <= frame <= end for frame in downs):
        raise ValueError(f"{movie}: input during playback")
    samples = []
    for frame, candidate, instance, pts in matches(lines, r"frame=(\d+) neko-film-composed" + key + r" pts=(\S+)"):
        frame, pts = int(frame), float(pts)
        if (candidate, instance) != (handle, playback) or not start <= frame < end or not math.isfinite(pts):
            raise ValueError(f"{movie}: unexpected compositor receipt")
        samples.append((frame, pts))
    if (len(samples) < 2 or samples[0][1] < 0
            or any(a >= b or x >= y for (a, x), (b, y) in zip(samples, samples[1:]))):
        raise ValueError(f"{movie}: no ordered advancing compositor receipts")
    return {"start": start, "end": end, "handle": int(handle), "playback": int(playback),
            "composited_frames": len(samples), "first_pts": samples[0][1], "last_pts": samples[-1][1]}


def guard_turn(lines, host, downs, scene, group, record):
    label = f"Turn at {scene}/{group}/{record}"
    start, = unique(matches(lines, rf'replay frame=(\d+) checkpoint="{re.escape(label)}"'), label)
    start = int(start)
    guards = matches(host, rf'frame ({start}): guarded press \[625, 478\] candidates=\[[0-9, ]+\] '
                     r'window=Some\(\("meswnd", (\d+)\)\) receiver=Some\((\d+)\)')
    _, owner, receiver = unique(guards, f"{label} live guard")
    press, routed_frame, region, routed_owner = unique(
        matches(host, rf'replay frame=(\d+): verified mouse press "{re.escape(label)}": '
                r'InputRouteReceipt \{ frame: (\d+), key: 0, group: 0, raw_mask: 2, pressed: 1, repeated: false, '
                r'region: (\d+), receiver: (\d+), response: 1, '
                r'physical: \[RoutedPhysicalButton \{ source: Mouse, code: 1 \}\] \}'),
        f"{label} physical receipt")
    if press != routed_frame or routed_owner != owner:
        raise ValueError(f"{label}: wrong receiver or physical frame")
    press = int(press)
    consumed = []
    for frame, payload in matches(lines, r"frame=(\d+) neko-cstl-consumed (\{.*\})"):
        row = json.loads(payload)
        if row["started"] == start:
            consumed.append((int(frame), row))
    verified, row = unique(consumed, f"{label} root and release receipt")
    if (owner != receiver or downs.count(start) != 1 or not start < press <= start + 3 < verified
            or (row["press_frame"], row["region"], row["owner"], row["released"]) !=
            (press, int(region), int(owner), True)
            or row["root_region"] <= 0 or row["root_owner"] <= 0
            or lines.count(f"frame={press} message-resume message=[2, 625, 478, 0]") != 1):
        raise ValueError(f"{label}: missing single guarded physical consumption")
    return start, press


def branch(lines, host, downs, stories, loads, *, continues=False):
    start, press = guard_turn(lines, host, downs, "01", 491, 5386)
    sample, a, b = map(int, unique(matches(lines, BRANCH), "01 branch flags"))
    if sample != start or not any(frame <= sample and (state, scene, group, record) == ("page", "01", 491, 5386)
                                  for frame, state, scene, group, record in stories):
        raise ValueError("01 branch flag receipt did not come from the live final page")
    path = BRANCH_PATHS[a != 0 and b == 0]
    scene, first_wait = ("H01", 51) if len(path) == 2 else ("01_02", 24)
    successor, = unique([(frame,) for frame, state, name, group, record in stories
                         if (state, name.lower(), group, record) == ("page", scene.lower(), 0, first_wait)
                         and frame > press], "branch first live wait")
    branch_loads = [(frame, name) for frame, name in loads if press <= frame <= successor
                    and name in {member for route in BRANCH_PATHS.values() for member in route}]
    if (tuple(name for _, name in branch_loads) != path
            or any(start < frame < successor or frame == successor and not continues for frame in downs)):
        raise ValueError("branch package sequence differs or input crossed the transition")
    label = f"Turn at {scene.lower()}/0/{first_wait}" if continues else f"Stop at {scene}/0/{first_wait} without input"
    terminal, = unique(matches(lines, rf'replay frame=(\d+) checkpoint="{re.escape(label)}"'), "branch handoff")
    terminal = int(terminal)
    if terminal < successor or any(start < frame < terminal or frame == terminal and not continues for frame in downs):
        raise ValueError("branch handoff precedes the live successor or received unexpected input")
    return {"press": press, "flags": [a, b], "loads": branch_loads,
            "successor": [successor, scene, 0, first_wait], "terminal": terminal}


def ending(lines, host, downs, stories, loads, visual):
    _, credits_press = guard_turn(lines, host, downs, "13_02", 55, 330)
    final_start, final_press = guard_turn(lines, host, downs, "14", 66, 491)
    credits = film_evidence(lines, "ed", credits_press, final_start, downs)
    flag_frame, handle = unique(matches(lines, r"frame=(\d+) neko-credits-flag handle=([1-9]\d*) flag550=0"),
                                "uncleared credits flag")
    chapter = next((frame for frame, name in loads if name == "14.cst" and frame >= credits["end"]), None)
    final_waits = [frame for frame, state, scene, group, record in stories
                   if (state, scene, group, record) == ("page", "14", 66, 491)]
    if (int(handle) != credits["handle"] or int(flag_frame) != credits["start"]
            or not any(frame <= credits_press and name == "13_02.cst" for frame, name in loads)
            or chapter is None or not any(chapter <= frame <= final_start for frame in final_waits)):
        raise ValueError("credits did not lead from uncleared 13_02 into a confirmed chapter 14 final page")
    unlocked, = unique(matches(lines, UNLOCK), "local ending achievement unlock")
    title, owner = unique(matches(lines, CLEAR), "fresh cleared title")
    unlocked, title = int(unlocked), int(title)
    if (not final_press <= unlocked <= title
            or not any(frame == title and state == "title" for frame, state, *_ in stories)
            or any(frame > final_start for frame in downs)):
        raise ValueError("missing ordered fresh unlock, flag550=1 or input-free returned title")
    report = {"credits": credits, "chapter14": chapter, "unlock": unlocked, "title": title, "title_owner": int(owner)}
    if visual is not None:
        manifest = json.loads(visual.read_text(encoding="utf-8-sig"))
        for role in ("credits", "end_card"):
            item = manifest[role]
            frame, pts = item["frame"], item["pts"]
            event = (f'frame={frame} neko-film-composed ed handle={credits["handle"]} '
                     f'playback={credits["playback"]} pts=')
            samples = [float(line[len(event):]) for line in lines if line.startswith(event)]
            if pts not in samples or not credits["start"] <= frame < credits["end"]:
                raise ValueError(f"{role}: snapshot has no matching credits compositor receipt")
            path = visual.parent / item["path"]
            if hashlib.sha256(path.read_bytes()).hexdigest() != item["sha256"]:
                raise ValueError(f"{role}: snapshot SHA-256 differs")
        if (manifest["credits"]["frame"] == manifest["end_card"]["frame"]
                or manifest["credits"]["sha256"] == manifest["end_card"]["sha256"]):
            raise ValueError("credits and end card must have distinct frames and images")
        report["visual_frames"] = {role: manifest[role]["frame"] for role in ("credits", "end_card")}
    return report


def story_evidence(lines, host):
    downs = [int(frame) for (frame,) in matches(host, DOWN)]
    stories = [(int(frame), state, scene, int(group), int(record))
               for frame, state, scene, group, record in matches(lines, STORY)]
    loads = []
    for frame, _, name, digest in matches(lines, LOAD):
        name = name.lower()
        if name in RESOURCES and RESOURCES[name][1] != digest:
            raise ValueError(f"unexpected resource digest for {name}")
        loads.append((int(frame), name))
    return downs, stories, loads


def branch_evidence(lines, host, *, continues=False):
    return branch(lines, host, *story_evidence(lines, host), continues=continues)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("phase", choices=("branch", "ending"))
    parser.add_argument("--events", type=Path, required=True)
    parser.add_argument("--host", type=Path, required=True)
    parser.add_argument("--visual", type=Path, help="reviewed snapshot manifest for credits and end card")
    args = parser.parse_args()
    try:
        lines = args.events.read_text(encoding="utf-8-sig").splitlines()
        host = args.host.read_text(encoding="utf-8-sig").splitlines()
        if args.phase == "branch":
            report = branch_evidence(lines, host)
        else:
            report = ending(lines, host, *story_evidence(lines, host), args.visual)
    except (OSError, ValueError, KeyError, TypeError) as error:
        parser.exit(1, f"{error}\n")
    status = "insufficient" if args.phase == "ending" and args.visual is None else "pass"
    print(json.dumps({"status": status, **report}, indent=2))
    return 2 if status == "insufficient" else 0


if __name__ == "__main__":
    raise SystemExit(main())

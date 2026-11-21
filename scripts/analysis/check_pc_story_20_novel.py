"""Generate and verify Rebellions' guarded novel and restored-window runs."""

import argparse
import json
from pathlib import Path
import re

from check_pc_body_successor import (
    REBELLIONS_BODY_BINDING, REBELLIONS_RELEASE, collect_events, read_host, rebellions_entry,
)
from check_pc_story_11_13_successor import read_run
from check_pc_story_20_opening_segment import (
    COMMAND, NOVEL_TRANSITION, ROOT_WAIT, SCENE, observe_page, verify_enter,
)
from plan_pc_story_09_20_continuation import (
    REBELLIONS_NOVEL_FES, SCENES, adjacent_pages, read_scene, rebellions_config, scene_steps,
)
from check_pc_story_11_13_sources import require
from check_neko_first_page import LAYER


SECOND = "A_01_02"
THIRD = "A_01_03"
MODES = {
    "novel_guard_r211": (SCENE, 27, 22000, "guard_novel_r211"),
    "novel_guard_r215": (SCENE, 29, 25000, "guard_novel_r215"),
    "restored_r301": (SCENE, 47, 45000, "guard_restored_r301"),
    "restored_r610": (SCENE, 122, 145000, "guard_restored_r610"),
    "second_r93": (SECOND, 2, 150000, "guard_second_r93"),
    "second_r583": (SECOND, 88, 220000, "guard_second_r583"),
    "third_r97": (THIRD, 2, 230000, "guard_third_r97"),
}
ENTRY_DATE_READS = {SECOND: [19, 20], THIRD: [21, 22]}
PAGE_WAITS = {(29, 215), (32, 225), (37, 238), (41, 249)}
WINDOW_SWAPS = {27: NOVEL_TRANSITION, 47: ("novel off", "view 0 wnd01", "rdraw 120")}
BODY_PROOF = re.compile(
    r"frame (\d+): Rebellions guarded body context=(\d+) load=([1-9]\d*) epoch=([1-9]\d*) "
    r"source_frame=(\d+) kind=([23]) member=(\w+\.fes) marked=(\d+) complete=(\d+)"
)
NOVEL_LAYER = re.compile(LAYER.pattern.replace("|keywait)", "|keywait2?)"))
NATIVE_WAIT = re.compile(r"frame=(\d+) native-call index=630 arguments=16 bytes \[([0-9a-f]{32})\]")
PAGE_MARK = re.compile(r'frame=(\d+) native-call index=(?:1|1040) .* debug="\[page\]"')
SCENE_LOAD = re.compile(r"frame=(\d+) rebellions-scene-load handle=(\d+) load_id=(\d+) "
                        r"scene=(A_01_0[23]) sha256=([0-9a-f]{64})")
DATE_READ = re.compile(r"frame=(\d+) cst-command-compat-read scene=(A_01_0[23]) handle=(\d+) "
                       r"group=\d+ record=(\d+) native=327 ordinal=\d+ command=.*")
WAIT_PACKETS = {
    2: {"73de040082de040000000000", "84de040093de040000000000"},
    3: {"95de0400a3de040000000000", "a5de0400b3de040000000000"},
}


def config(mode):
    scene, _, frames, native_mode = MODES[mode]
    scenario = rebellions_config("r189")["20"]
    scenario["frames"] = frames
    scenario["route"].update(label=f"Rebellions: guarded {scene} through {mode}", rebellions_r15=native_mode)
    return {"20": scenario}


def points_for(game, mode):
    terminal_scene, terminal, _, _ = MODES[mode]
    points = scene_steps(read_scene(game, SCENE), SCENE)
    require(len(points) == 123 and points[-1]["wait"] == 610, "A_01_01 wait count")
    require({(p["group"], p["wait"]) for p in points if p["kind"] == 3} == PAGE_WAITS,
            "unexpected page wait set")
    for index, point in enumerate(points):
        require(point["group"] == index and len(point["bodies"]) == 1
                and point["bodies"][0]["record"] + 1 == point["wait"], f"g{index}: body source")
    for group, commands in WINDOW_SWAPS.items():
        observed = [cmd for _, cmd in points[group]["commands"] if cmd in commands]
        require(tuple(observed) == commands, f"g{group}: window transition commands")
    if terminal_scene == SCENE:
        return points[:terminal + 1]
    following = scene_steps(read_scene(game, SECOND), SECOND)
    require(len(adjacent_pages(following)) == 89 and following[-1]["wait"] == 583,
            "A_01_02 adjacent body route")
    if terminal_scene == SECOND:
        return points + following[:terminal + 1]
    third = scene_steps(read_scene(game, THIRD), THIRD)[:terminal + 1]
    require([p["wait"] for p in adjacent_pages(third)] == [88, 90, 97], "A_01_03 adjacent body route")
    return points + following + third


def verify(meta, host, lines, mode):
    require(meta["mode"] == "state" and meta["replay_status"] == "completed", "unfinished guarded route")
    route_file = Path(meta["route_json"]).resolve()
    require(route_file.parent == Path(meta["stderr"]).resolve().parent
            and json.loads(route_file.read_text(encoding="utf-8")) == config(mode)["20"]["route"],
            "wrong sample or route config")
    points = points_for(Path(meta["game"]), mode)
    scene_events = {scene: collect_events(lines, scene) for scene in dict.fromkeys(p["scene"] for p in points)}
    events = scene_events[SCENE]
    observations, receipts, presses = read_host(host, NOVEL_LAYER)
    positions = [(frame, scene, group, record) for scene, observed in scene_events.items()
                 for frame, group, record in observed["route_pages"]]
    require(events["replay_status"] == ["completed"]
            and [(scene, g, r) for _, scene, g, r in positions]
            == [(p["scene"], p["group"], p["wait"]) for p in points],
            "wrong wait sequence")
    frames = [frame for frame, _, _, _ in positions]
    require(frames == sorted(set(frames)), "nonchronological or duplicate page checkpoint")
    bindings = [tuple(map(int, m.groups())) for line in host if (m := REBELLIONS_BODY_BINDING.fullmatch(line))]
    proofs = [m for line in host if (m := BODY_PROOF.fullmatch(line))]
    releases = [int(m[1]) for line in host if (m := REBELLIONS_RELEASE.fullmatch(line))]
    require([row[0] for row in bindings] == frames and [int(m[1]) for m in proofs] == frames
            and releases == frames[1:], "missing body, binding or release receipt")
    require(rebellions_entry(presses, receipts, events, frames[0])
            and presses[3:] == [(frame, "Keyboard(13)") for frame in frames[:-1]],
            "unexpected input or input at terminal checkpoint")
    consumed = [r for r in receipts if r["frame"] > frames[0] and r["pressed"] and r["response"] == 0]
    require(len(consumed) == len(frames) - 1, "expected exactly one body consumer per Enter")
    waits = [(int(m[1]), int.from_bytes(bytes.fromhex(m[2][:8]), "little"), m[2][8:])
             for line in lines if (m := NATIVE_WAIT.fullmatch(line))]
    page_marks = {int(m[1]) for line in lines if (m := PAGE_MARK.fullmatch(line))}
    commands = [(int(m[1]), m[2].strip()) for line in lines if (m := COMMAND.fullmatch(line))]
    loads = [(int(m[1]), int(m[2]), int(m[3]), m[4], m[5]) for line in lines if (m := SCENE_LOAD.fullmatch(line))]
    date_reads = [(int(m[1]), m[2], int(m[3]), int(m[4])) for line in lines if (m := DATE_READ.fullmatch(line))]
    inspect = {frame: (status, layers) for frame, status, layers in observations}
    pages = []
    for point, (frame, scene, group, record), (_, owner, region, receiver), proof in zip(points, positions, bindings, proofs):
        context, load, epoch, source_frame, kind = map(int, proof.groups()[1:6])
        member, marked, complete = proof[7], int(proof[8]), int(proof[9])
        novel = scene == SCENE and 27 <= group <= 46
        size = point["bodies"][0]["bytes"]
        require(kind == point["kind"] and marked < frame and marked in (page_marks if kind == 3 else events["input"])
                and source_frame <= complete <= frame and (frame, ROOT_WAIT) in events["root_wait"]
                and (member in REBELLIONS_NOVEL_FES if novel else member == "meswnd01.fes")
                and any(marked <= at <= frame and task == owner and packet in WAIT_PACKETS[kind]
                        for at, task, packet in waits), f"r{record}: wait kind, FES, or native packet")
        require(any((h, l, g, r, n) == (context, load, group, record - 1, size) and read <= source_frame
                    for h, l, g, r, read, n in events["rebellions_sources"]), f"r{record}: exact GBK source")
        page = dict(frame=frame, scene=scene, group=group, record=record, owner=owner, region=region, receiver=receiver,
                    context=context, load=load, epoch=epoch, source_frame=source_frame, member=member,
                    marked=marked, complete=complete, kind=kind)
        if not novel:
            ordinary = observe_page(frame, *inspect[frame], (owner, region, receiver), group, record, size,
                                    scene_events[scene], scene)
            require((ordinary["handle"], ordinary["load"], ordinary["epoch"], ordinary["source_frame"],
                     ordinary["marked"], ordinary["complete"]) == (context, load, epoch, source_frame, marked, complete),
                    f"r{record}: ordinary-window proof disagreement")
        else:
            _, layers = inspect[frame]
            prompt = "keywait" if kind == 3 else "keywait2"
            require(layers.get(owner, {}).get(prompt, 0) > 0, f"r{record}: missing submitted {prompt}")
        for task, expected_type, parent, name in ((owner, 34, receiver, None), (region, 6, receiver, "bottom button")):
            latest = next((entry for entry in reversed(events["rebellions_tasks"].get(task, ())) if entry[0] <= frame), None)
            require(latest and latest[1:3] == (expected_type, parent) and (name is None or latest[3] == name),
                    f"r{record}: mismatched task creation")
        if pages:
            before = pages[-1]
            verify_enter(events, before, frame, consumed[len(pages) - 1])
            down = before["frame"]
            switched_scene = scene != before["scene"]
            switched_window = scene == SCENE and group in WINDOW_SWAPS
            require(frame > down + 3 and min(marked, source_frame) > down
                    and all(before[k] == page[k] for k in ("region", "receiver")),
                    f"r{record}: successor timing or identity")
            unchanged = ("region", "receiver") if switched_scene or switched_window else ("owner", "region", "receiver")
            require(all(not any(down < at <= frame for at, *_ in events["rebellions_tasks"].get(before[key], ()))
                        for key in unchanged), f"r{record}: continuing task recreated")
            if switched_scene:
                loaded = ordinary["loaded"]
                require(load > before["load"] and down < loaded <= marked
                        and [row for row in loads if row[3] == scene] == [(loaded, context, load, scene, SCENES[scene][2])]
                        and [r for at, src, h, r in date_reads if src == scene and loaded <= at <= marked and h == context]
                            == ENTRY_DATE_READS[scene]
                        and (owner == before["owner"] and epoch > before["epoch"]
                             or any(down < at <= frame for at, *_ in events["rebellions_tasks"].get(owner, ()))),
                        f"{scene} fresh load, date repair, or window generation")
            else:
                require((context, load) == (before["context"], before["load"]), f"r{record}: scene load changed")
            if switched_window:
                expected = WINDOW_SWAPS[group]
                require(tuple(cmd for at, cmd in commands if down < at <= marked and cmd in expected) == expected
                        and member != before["member"]
                        and any(down < at <= frame for at, *_ in events["rebellions_tasks"].get(owner, ())),
                        f"r{record}: missing window transition")
            elif not switched_scene:
                require(owner == before["owner"] and member == before["member"] and epoch > before["epoch"],
                        f"r{record}: stale window generation")
        pages.append(page)
    if mode in {"second_r583", "third_r97"}:
        repaired = {r for _, scene, _, r in date_reads if scene == SECOND}
        require(repaired == {19, 20, 240, 241}, "missing second scene date transitions")
    return {"status": f"{mode}_verified", "pages": len(pages), "body_enters": len(consumed),
            "page_waits": [page for page in pages if page["kind"] == 3],
            "window_swaps": [page for page in pages if page["scene"] == SCENE and page["group"] in WINDOW_SWAPS],
            "terminal": pages[-1]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument("--emit", type=Path, help="existing directory for new configs")
    source.add_argument("--run", type=Path, help="batch directory")
    parser.add_argument("--mode", choices=MODES)
    args = parser.parse_args()
    try:
        if args.emit:
            for mode in ((args.mode,) if args.mode else MODES):
                path = args.emit / f"20261121_pc_story_20_{mode}.json"
                with path.open("x", encoding="utf-8") as output:
                    output.write(json.dumps(config(mode), indent=2) + "\n")
            return 0
        if args.mode is None:
            parser.error("--run requires --mode")
        identity, host, lines = read_run(args.run, "20")
        meta = json.loads((args.run / "20.json").read_text(encoding="utf-8"))
        print(json.dumps({"build": identity, **verify(meta, host, lines, args.mode)}, indent=2))
        return 0
    except (KeyError, OSError, ValueError) as error:
        print(json.dumps({"status": "rejected", "reason": str(error)}))
        return 1


if __name__ == "__main__":
    raise SystemExit(main())

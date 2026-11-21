
"""Generate and verify the sample 17 fresh-profile route to CASE 3 END1."""

import argparse
from collections import defaultdict
import json
import re
from pathlib import Path
import zlib

from check_daydream_prologue import CASE_NEXT_BODY, DOWN, ENTER, MOUSE_LEFT, RECEIPT
from generate_daydream_case_evidence import ORDERS, load_commands, walk
from generate_daydream_prologue_exit import (
    GAME, HERE, build_case_successor_scenario, cst_rows, members, require,
)

OUTPUT = HERE / "pc_story_17_case03_end1.json"
PATCH = "generate_daydream_case03_end1_player.patch"
END_FLAGS = "399=3 214=1 224=1 234=1 215=0 225=0 235=1 760=1 1000=0 1001=0 1002=0 1003=1"
LOAD = re.compile(r'frame=(\d+) native-call .* debug="sscript loaded > (\w+)"')
BODY = re.compile(r'frame (\d+): Daydream END1 body scene=(\w+) group=(\d+) record=(\d+) .* region=(\d+) receiver=(\d+)')
MENU = re.compile(r'frame (\d+): Daydream END1 choice scene=(\w+) option=(\d+) target=(\w+) owner=(\d+) region=(\d+) point=\[.*\]')
INPUT = re.compile(r'frame (\d+): Daydream END1 (Enter|choice press) (InputRouteReceipt \{.*\})')
MOVIE = re.compile(r'frame=(\d+) .* movie_daydream_end1_(started|released|interrupted) handle=(\d+)')
CLEAR = re.compile(r'frame (\d+): Daydream CASE 3 END1 natural_movie_frame=(\d+) next=c99_10 (.*)')
MENU_TARGETS = {"c99_02_a_01": "c99_02_x_01", "c99_02_b_01": "c99_02_x_01",
                "c99_02_c_01": "c99_02_x_01", "brunch_2": "c03_04"}
MENU_ROWS = {
    "c99_02_a_01": (1, 2, 3),
    "c99_02_b_01": (4, 5, 6),
    "c99_02_c_01": (7, 9),
    "brunch_2": (10, 11, 12),
}


def case03_routes(rules):
    paths = []
    for order in ORDERS:
        trail, movies, menus = walk(rules, "start", defaultdict(int), iter(order[:2]), iter((12, 10, 11)))
        end = trail.index("c03_ed_x")
        route = trail[:end + 2]
        require(route[5] == f"c0{order[0]}_01_01" and route[-3:] == ["c03_ed", "c03_ed_x", "c99_10"]
                and next(movie for movie in movies if movie.startswith("ed_")) == "ed_c03",
                f"invalid first END1 route for intro order {order}")
        first_selection = next(item for item in menus if item[0] == "brunch_2")
        require(first_selection == ("brunch_2", MENU_ROWS["brunch_2"], 12)
                and all(options == MENU_ROWS[scene] and selection == options[-1]
                        for scene, options, selection in menus[:3]),
                f"invalid minimal first END1 selections for intro order {order}")
        paths.append(route)
    return paths


def wait_records(resources, scene):
    rows = list(cst_rows(resources[f"{scene}.cst"][1]))
    waits = []
    for group, record, kind, payload in rows:
        require(kind != 0x03, f"{scene} r{record}: novel wait needs its own contract")
        if kind == 0x02:
            require(group == len(waits) and not payload,
                    f"{scene} r{record}: nonconsecutive or parameterized body wait")
            waits.append(record)
    return waits


def build_scenario():
    jp_rules = load_commands("update02.int")
    cn_rules = jp_rules | load_commands("update03.int")
    paths = case03_routes(jp_rules)
    require(paths == case03_routes(cn_rules), "localized CST control flow changed")
    names = {f"{scene}.cst" for trail in paths for scene in trail[:-1]}
    jp, cn = (members(GAME, archive, names) for archive in ("update02.int", "update03.int"))
    pages = {}
    for scene in sorted(name.removesuffix(".cst") for name in names):
        waits = wait_records(jp, scene)
        if f"{scene}.cst" in cn:
            require(waits == wait_records(cn, scene), f"localized wait layout changed in {scene}")
        if waits:
            pages[scene] = waits
    require(jp_rules["c03_ed"] == ((0, 1, None, b"movie ed_c03"), (0, 3, None, b"next c03_ed_x"))
            and jp_rules["c03_ed_x"][:3] == ((0, 0, None, b"#760 = 1"), (0, 1, None, b"#1003 = 1"),
                                             (0, 2, None, b"#235 = 1"))
            and jp_rules["c03_06_h"][-1] == (176, 877, None, b"next c03_ed"),
            "CASE 3 END1 terminal resources changed")
    script = members(GAME, "fes.int", {"select.fes"})["select.fes"][1]
    lines = zlib.decompress(script[16:]).splitlines()
    require(all(lines[i - 1] == line for i, line in
                {53: b"BUTTON fr1[10]", 229: b"\\m_wait = 1", 230: b"next WAIT",
                 240: b"exit \\m_ec", 271: b"#fr1.PUSH_L", 272: b"\\m_ec = @"}.items()),
            "select.fes row handler changed")
    scenario = build_case_successor_scenario()
    settings = scenario["17"]
    settings["frames"] = 5_000_000
    settings["route"].update(label="Daydream: fresh profile to CASE 3 END1",
                             daydream_case03_end1={"paths": paths, "pages": pages})
    settings["review"].update({
        "player_patch": f"scripts/analysis/{PATCH}",
        "generator": f"scripts/analysis/{Path(__file__).name}",
        "acceptance": "Verify natural ed_c03 playback and c03_ed_x writes on entry to c99_10, "
                      "with no c99_10 body input.",
        "body_waits_by_intro_order": [sum(len(pages.get(scene, [])) for scene in path[:-1]) for path in paths],
    })
    return scenario


def rendered(scenario):
    text = json.dumps(scenario, indent=2, ensure_ascii=True)
    return re.sub(r"\[\n\s+(\d+(?:,\n\s+\d+)*)\n\s+\]",
                  lambda match: "[" + " ".join(match[1].split()) + "]", text) + "\n"


def verify_run(path, plan):
    result = json.loads(path.read_text(encoding="utf-8"))
    require((result["index"], result["mode"], result["status"], result["replay_status"], result["exit_code"],
             result["timed_out"], result["first_engine_error"]) == (17, "state", "completed", "completed", 0, False, None),
            "batch did not complete a successful sample 17 replay")
    host = path.parent / "17/17.host.log"
    events = path.parent / "17/17.events.log"
    second = []
    clears = []
    inputs = []
    bodies = []
    menus = []
    receipts = []
    with host.open(encoding="utf-8", errors="replace") as stream:
        for number, line in enumerate(stream, 1):
            line = line.rstrip("\r\n")
            if match := CASE_NEXT_BODY.fullmatch(line):
                second.append((int(match[1]), match[2]))
            elif match := CLEAR.fullmatch(line):
                clears.append((number, int(match[1]), int(match[2]), match[3]))
            elif match := DOWN.fullmatch(line):
                inputs.append((int(match[1]), match[2]))
            elif match := BODY.fullmatch(line):
                bodies.append((int(match[1]), match[2], *map(int, match.groups()[2:])))
            elif match := MENU.fullmatch(line):
                menus.append((int(match[1]), match[2], int(match[3]), match[4], int(match[5]), int(match[6])))
            elif match := INPUT.fullmatch(line):
                receipts.append((int(match[1]), match[2], RECEIPT.fullmatch(match[3])))
    require(len(second) == len(clears) == 1, "missing unique second CASE body or END1 flag snapshot")
    clear_line, end, natural, flags = clears[0]
    require(end == result["last_frame"] and flags.replace("#", "") == END_FLAGS, "CASE 3 END1 terminal flags differ")
    cutoff, first_case = second[0]
    by_down = {frame: ("Keyboard(13)", region, receiver) for frame, _, _, _, region, receiver in bodies}
    by_down.update({frame: ("MouseButton(0)", region, owner) for frame, _, _, _, owner, region in menus})
    require(len(by_down) == len(bodies) + len(menus) == len(receipts)
            and [(frame, key) for frame, key in inputs if frame >= cutoff]
                == sorted((frame, action[0]) for frame, action in by_down.items()),
            "an END1 input lacked a unique prepared action or physical receipt")
    body_frames = set()
    for (start, (key, region, receiver)), (frame, kind, receipt) in zip(sorted(by_down.items()), receipts):
        body = key == "Keyboard(13)"
        expected = ("2", "0", "1", "1", "false") if body else ("0", "0", "2", "1", "false")
        require(receipt is not None and int(receipt[1]) == frame and receipt.groups()[1:6] == expected
                and tuple(map(int, receipt.groups()[6:8])) == (region, receiver)
                and ((kind, frame, receipt[9], receipt[10]) == ("Enter", start + 1, "0", ENTER) if body
                     else kind == "choice press" and start < frame <= start + 3
                         and receipt[9] in ("0", "1") and receipt[10] == MOUSE_LEFT),
                f"input receipt did not match the armed action at frame {start}")
        if body:
            body_frames.add(frame)
    loads = []
    movie = []
    resumes = defaultdict(int)
    waits = set()
    endings = []
    with events.open(encoding="utf-8", errors="replace") as stream:
        for line in stream:
            line = line.rstrip("\r\n")
            if match := LOAD.fullmatch(line):
                loads.append((int(match[1]), match[2].lower()))
            elif match := MOVIE.fullmatch(line):
                movie.append((int(match[1]), match[2], int(match[3])))
            elif line.startswith("frame=") and line.endswith(" message-resume message=[2, 0, 0, 0]"):
                frame = int(line.split()[0][6:])
                if frame in body_frames:
                    resumes[frame] += 1
            elif line.startswith("frame=") and line.endswith(" input-wait native 71 address=0xd6d"):
                frame = int(line.split()[0][6:])
                if frame in body_frames:
                    waits.add(frame)
            elif line.endswith(" ending_verified=DAYDREAM_CASE03_END1"):
                endings.append(line)
    require(waits == body_frames and all(resumes[frame] == 1 for frame in waits),
            "an END1 Enter did not resume the VM once on its receipt frame")
    path = []
    for frame, name in loads:
        if name == "prologue" and frame < cutoff:
            continue
        if path and name == path[-1][1] and name in {"brunch_b_12", "brunch_b_13", "brunch_b_23"}:
            path[-1] = frame, name
        else:
            path.append((frame, name))
    names = [name for _, name in path]
    require(names in plan["paths"] and names[5] == first_case, "CST load route is outside the six fresh-profile paths")
    expected_bodies = [
        (index, name, group, record)
        for index, name in enumerate(names[5:-1], 5)
        for group, record in enumerate(plan["pages"].get(name, []))
        if index != 5 or group >= 1
    ]
    require(len(bodies) == len(expected_bodies) and all(
        (scene, group, record) == (name, expected_group, expected_record)
        and path[index][0] <= frame < path[index + 1][0]
        for (frame, scene, group, record, _, _), (index, name, expected_group, expected_record)
        in zip(bodies, expected_bodies)
    ), "an exact body wait was skipped or advanced outside its CST load")
    expected_menus = [(index, name) for index, name in enumerate(names) if name in MENU_TARGETS]
    require(len(menus) == len(expected_menus) == 4 and all(
        scene == name and option == MENU_ROWS[name][-1] and target == MENU_TARGETS[name] == names[index + 1]
        and path[index][0] <= frame < path[index + 1][0]
        for (frame, scene, option, target, _, _), (index, name) in zip(menus, expected_menus)
    ), "unexpected menu, option, or CST successor")
    require(len(movie) == 2 and movie[0][1] == "started" and movie[1][1] == "released"
            and movie[0][2] == movie[1][2]
            and path[-3][0] <= movie[0][0] < movie[1][0] == natural <= path[-2][0] <= path[-1][0] <= end
            and endings == [f"replay frame={end} ending_verified=DAYDREAM_CASE03_END1"]
            and all(frame < path[-1][0] for frame in by_down),
            "ed_c03 was not naturally released before the formal clear and input-free c99_10 entry")
    return {"status": "verified", "ending": "CASE 3 END1", "frame": end, "host_line": clear_line,
            "body_enters_after_first_turn": len(bodies), "choices": [item[2] for item in menus],
            "movie_handle": movie[0][2], "natural_movie_frame": natural, "flags": flags}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--check", action="store_true")
    mode.add_argument("--run", type=Path, help="verify a completed batch directory or 17.json")
    mode.add_argument("--save-entry", action="store_true",
                      help="emit a route that opens ordinary Save at CASE 1 r121 and stops at the empty slot")
    args = parser.parse_args()
    try:
        if args.run is not None:
            plan = json.loads(OUTPUT.read_text(encoding="utf-8"))["17"]["route"]["daydream_case03_end1"]
            result = args.run / "17.json" if args.run.is_dir() else args.run
            print(json.dumps(verify_run(result, plan), indent=2))
            return
        scenario = build_scenario()
        if args.save_entry:
            settings = scenario["17"]
            route = settings["route"]
            require(all("c01_01_01" in path for path in route["daydream_case03_end1"]["paths"])
                    and route["daydream_case03_end1"]["pages"]["c01_01_01"][13] == 121,
                    "ordinary Save anchor no longer belongs to every fresh route")
            route["label"] = "Daydream: ordinary Save entry at CASE 1 r121"
            route["daydream_case03_end1"]["save_entry_at_r121"] = True
            settings["review"]["player_patch"] = "archive/2026-11/20261121_daydream_case01_ordinary_save_entry.patch"
            settings["review"]["acceptance"] = "Verify one Save toolbar click and stop at empty ordinary slot 0."
            print(rendered(scenario), end="")
            return
        data = rendered(scenario).encode("utf-8")
        if args.check:
            require(OUTPUT.read_bytes() == data, "stale CASE 3 END1 scenario")
        else:
            OUTPUT.write_bytes(data)
        print(OUTPUT.name)
    except (KeyError, ValueError, OSError, StopIteration) as error:
        parser.error(str(error))


if __name__ == "__main__":
    main()


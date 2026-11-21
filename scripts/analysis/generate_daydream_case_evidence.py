"""Check the fresh-profile Daydream CASE routes offline and print a receipt."""

from collections import defaultdict
from itertools import permutations, product
import json
import re
from xml.etree import ElementTree
import zlib

from generate_daydream_prologue_exit import GAME, build_evidence, cst_rows, identity, members, require
from startup_inventory import entries_at


ORDERS = tuple(permutations((1, 2, 3)))
PICK = {"c99_02_a_01": 3, "c99_02_b_01": 6, "c99_02_c_01": 9}
INTRO = (214, 224, 234)
END1 = (215, 225, 235)
EPISODE = (216, 226, 236)
EXTRAS = (1014, 1012, 1013)
END1_REPLAYS = (1001, 1002, 1003)
END1_SUCCESSORS = ("c99_10", "c99_13", "c99_16")
FLAGS = {202, 211, 212, 213, 221, 222, 223, 231, 232, 233, 290, 299, 399,
         760, 1000, 1001, 1002, 1003, 1005, *INTRO, *END1, *EPISODE, *EXTRAS}
IF = re.compile(rb"if\s*\(([^()]*)\)\s*(.+)")
WRITE = re.compile(rb"#(\d+)\s*=\s*(.+)")
OPTION = re.compile(rb"(\d+\s+\w+)\s+.+")


def number(token, flags):
    token = token.strip()
    if token.startswith(b"#"):
        flag = int(token[1:])
        require(flag in FLAGS, f"untracked condition flag #{flag}")
        return flags[flag]
    return int(token)


def matches(condition, flags):
    if condition is None:
        return True
    if b"==" not in condition:
        return all(number(token, flags) for token in condition.split(b"&&"))
    left, right = condition.split(b"==")
    return sum(number(token, flags) for token in left.split(b"+")) == number(right, flags)


def load_commands(archive):
    entries, _, _ = entries_at(GAME / archive, 0 if archive == "update03.int" else 0x1CF643B4)
    names = [entry["name"].decode("ascii", "replace").lower() for entry in entries]
    selected = [name for name in names if re.fullmatch(r"(?:start|replay_0[0-3]|brunch\w*|c(?:0[0-3]|99)_\w+)\.cst", name)]
    resources = members(GAME, archive, selected)
    scenes = {}
    for name, asset in resources.items():
        commands = []
        for group, record, kind, payload in cst_rows(asset[1]):
            if kind != 0x30:
                continue
            action = payload.strip()
            condition = None
            if predicate := IF.fullmatch(action):
                condition, action = predicate.groups()
            option = OPTION.fullmatch(action)
            if option:
                action = option[1]
            tokens = action.split()
            if tokens == [b"rand", b"298", b"1", b"4"]:
                # CASE 3 uses this draw for presentation, not a route edge.
                continue
            assignment = WRITE.fullmatch(action)
            if tokens and (tokens[0] in (b"next", b"rand", b"fselect", b"movie")
                           or option
                           or (assignment and int(assignment[1]) in FLAGS)):
                commands.append((group, record, condition, action))
        scenes[name.removesuffix(".cst")] = tuple(commands)
    return scenes


def walk(scenes, start, flags, draws, choices, *, question_choices=None, on_entry=None, stop_before=None):
    """Follow CST control records with symbolic RNG and menu choices."""
    scene = start
    trail, movies, menus = [], [], []
    for _ in range(550):
        trail.append(scene)
        if on_entry is not None:
            on_entry(scene, flags)
        if scene == stop_before:
            return trail, movies, menus
        options = None
        target = None
        for _, record, condition, action in scenes[scene]:
            if not matches(condition, flags):
                continue
            tokens = action.decode("ascii").split()
            command = tokens[0]
            if command == "next":
                require(options is None, f"{scene} r{record}: next inside fselect")
                target = tokens[1].lower() if len(tokens) == 2 else None
                break
            if command == "rand":
                require(tokens == ["rand", "299", "1", "3"], f"{scene} r{record}: unexpected RNG")
                flags[299] = next(draws)
            elif command == "movie" and len(tokens) == 2:
                movies.append(tokens[1])
            elif command == "fselect":
                require(options is None, f"{scene} r{record}: repeated fselect")
                options = {}
            elif command.isdecimal():
                require(options is not None, f"{scene} r{record}: option before fselect")
                options[int(command)] = tokens[1].lower()
            elif assignment := WRITE.fullmatch(action):
                flags[int(assignment[1])] = number(assignment[2], flags)

        if options is not None:
            if scene in PICK:
                chosen = PICK[scene] if question_choices is None else next(question_choices)
            else:
                chosen = next(choices)
            require(chosen in options, f"{scene}: option {chosen} absent from {options}")
            menus.append((scene, tuple(options), chosen))
            target = options[chosen]
        if target is None:
            return trail, movies, menus
        scene = target
    raise ValueError(f"route did not terminate near {scene}")


def check_routes(scenes):
    clear_receipts = {}
    # Force one rejected draw at stage 1 to exercise the reroll edge for each first CASE.
    for intro, end1 in product(ORDERS, repeat=2):
        flags = defaultdict(int)
        observed = {}

        def record_clear(scene, root):
            if scene in END1_SUCCESSORS:
                observed[scene] = {flag: root[flag] for flag in (399, *INTRO, *END1, 760, 1000, *END1_REPLAYS)}

        trail, movies, menus = walk(scenes, "start", flags, iter((intro[0], *intro[:2])),
                                    iter(i + 9 for i in end1), on_entry=record_clear)
        first_case = f"c0{intro[0]}_01"
        require(trail[:6] == ["start", "c00_00", "brunch", "brunch_a", first_case, first_case + "_01"],
                f"unexpected title-to-CASE prefix {trail[:6]}")
        require([node for node in trail if node in {f"c0{i}_01_01" for i in (1, 2, 3)}]
                == [f"c0{i}_01_01" for i in intro], f"intro order {intro}")
        require([chosen - 9 for scene, _, chosen in menus if scene == "brunch_2"] == list(end1),
                f"END1 menu order {end1}")
        remaining = {10, 11, 12}
        for scene, visible, chosen in menus:
            if scene == "brunch_2":
                require(set(visible) == remaining, f"{scene}: wrong remaining CASE options")
                remaining.remove(chosen)
        require([movie for movie in movies if movie.startswith("ed_")]
                == [*(f"ed_c0{i}" for i in end1), "ed_c00"], f"ending movies {end1}: {movies}")
        for index, scene in enumerate(END1_SUCCESSORS):
            cleared = set(end1[:index + 1])
            bits = [int(case in cleared) for case in (1, 2, 3)]
            expected = dict(zip((399, *INTRO, *END1, 760, 1000, *END1_REPLAYS),
                                (3 + index, 1, 1, 1, *bits, 1, 0, *bits), strict=True))
            case = end1[index]
            require(observed.get(scene) == expected
                    and trail[trail.index(scene) - 2:trail.index(scene)] == [f"c0{case}_ed", f"c0{case}_ed_x"],
                    f"END1 staged clear at {scene} {intro}/{end1}")
            if index == 0:
                clear_receipts[case] = {"option": case + 9, "target": f"c0{case}_04",
                                        "movie": f"ed_c0{case}.mpg", "successor": scene, "root_flags": expected}
        require(trail[-1] == "c00_end" and flags[399] == 6
                and all(flags[i] == 1 for i in (*INTRO, *END1, 760, 1000, 1001, 1002, 1003))
                and all(flags[i] == 0 for i in (202, *EPISODE, *EXTRAS, 1005)), f"TRUE END1 flags {intro}/{end1}")

    episode_receipts = {}
    for order in ORDERS:
        flags = defaultdict(int, {1000: 1})
        observed = {}

        def record_episode(scene, root):
            if scene in ("c00_ep_01_x", "c00_ep_02_x", "c00_ep_03_x"):
                observed[scene] = {flag: root[flag] for flag in (*EPISODE, *EXTRAS, 1005)}

        trail, movies, menus = walk(scenes, "c00_ep", flags, iter(()), iter(i + 12 for i in order),
                                    on_entry=record_episode)
        remaining = {13, 14, 15}
        for scene, visible, chosen in menus:
            require(scene == "c00_ep_123" and set(visible) == remaining, "wrong EPILOGUE options")
            remaining.remove(chosen)
        require(not remaining and [node for node in trail if node in {"c01_ep", "c02_ep", "c03_ep"}]
                == [f"c0{i}_ep" for i in order], f"EPILOGUE order {order}")
        for index, case in enumerate(order):
            scene = f"c00_ep_0{case}_x"
            cleared = set(order[:index + 1])
            bits = [int(number in cleared) for number in (1, 2, 3)]
            expected = dict(zip((*EPISODE, *EXTRAS, 1005), (*bits, *bits, 0), strict=True))
            require(observed.get(scene) == expected, f"END2 staged clear at {scene} {order}")
            episode_receipts[case] = {"option": case + 12, "successor": scene,
                                      "set_flags": [EPISODE[case - 1], EXTRAS[case - 1]]}
        require(trail[-1] == "c00_ep_ed_x" and movies == ["ed_c00_ep"] and flags[1005] == 1
                and all(flags[i] == 1 for i in EXTRAS) and all(flags[i] == 0 for i in EPISODE),
                f"TRUE END2 flags {order}")

    trail, movies, menus = walk(scenes, "c00_ep", flags, iter(()), iter((16,)))
    require(menus == [("c00_ep_123", (13, 14, 15, 16), 16)] and trail[-1] == "c00_ep_ed_x"
            and movies == ["ed_c00_ep"], "TRUE END2 repeat menu changed")
    return {
        "first_end1": clear_receipts,
        "end1_successors_in_order": END1_SUCCESSORS,
        "true_end1": {"movie": "ed_c00.mpg", "terminal_cst": "c00_end",
                      "root_flags": {399: 6, 202: 0, **{flag: 1 for flag in (*INTRO, *END1, 760, 1000, *END1_REPLAYS)}}},
        "episode_end2": episode_receipts,
        "true_end2": {"movie": "ed_c00_ep.mpg", "terminal_cst": "c00_ep_ed_x",
                      "root_flags": {1005: 1, **{flag: 0 for flag in EPISODE}, **{flag: 1 for flag in EXTRAS}}},
    }


def check_question_branches(scenes):
    paths = (
        ("c99_02_a_01", (1, 2, 3), (1, 2, 3), ("c99_02_a_01_a", "c99_02_a_01_b")),
        ("c99_02_b_01", (4, 5, 6), (4, 5, 6), ("c99_02_b_01_a", "c99_02_b_01_b")),
        ("c99_02_c_01", (7, 9), (7, 8, 9), ("c99_02_c_01_a", "c99_02_c_01_b")),
    )
    receipt = {}
    for scene, initial_rows, chosen, loop_targets in paths:
        flags = defaultdict(int)
        trail, _, menus = walk(scenes, scene, flags, iter(()), iter(()), question_choices=iter(chosen),
                               stop_before="c99_02_x_01")
        require(trail == [scene, loop_targets[0], scene, loop_targets[1], scene, "c99_02_x_01"]
                and menus == [(scene, initial_rows, chosen[0]), (scene, chosen, chosen[1]),
                              (scene, chosen, chosen[2])]
                and flags[290] == int(chosen[0] == 7), f"question menu reentry changed in {scene}")
        receipt[scene] = {"initial_rows": initial_rows, "checked_picks": chosen,
                          "checked_targets": [*loop_targets, "c99_02_x_01"]}
    return receipt


def check_title_and_bonus(scenes):
    transition = {
        "meswnd.fes": {265: b"next WAIT\n#WAIT", 517: b"#SETBUTTON\nnamebtn enable (\\211!=0)&&(\\50!=-1)",
                       654: b"if (\\52==0)\nhelp disp 0\nqsave_ok disp 0",
                       658: b"keywait anm 0\nkeywait disp 0",
                       670: b"#MES_SETSTATE", 684: b"if (\\53==0)\ncall SETBUTTON\nendif"},
        "flow.fes": {26: b"#START_TITLE\nendfes\nendkcs\nwait 1\nexecfes ttlflow\nnext TITLE_WAIT\n#TITLE_WAIT",
                     67: b"#GAME_WAIT\nif (_EXITCODE_!=-1)\nnext START_TITLE"},
        "ttlflow.fes": {30: b"#START_TITLE\nendfes\nendkcs\nwait 1\n$801 = 0\nexecfes title\nnext FES_WAIT\n#FES_WAIT"},
        "title.fes": {120: b"if ($542==1)\nnext IN2", 185: b"#IN2", 202: b"if ($541==2)\nnext SETBTN02",
                      205: b"next SETBTN00\n#SETBTN00\nbtn1[0] noact 0",
                      214: b"btn1[0] disp 1\nbtn1[0] fade 8 0 255", 256: b"btn1 enable 1",
                      281: b"automove btn1[0]\n$541 = 0\nbaseimg\nnext WAIT", 383: b"#WAIT",
                      405: b"#GO_STORY\nstrvar 200 start", 602: b"#btn1.PUSH_L",
                      605: b"if (@==0)\nnext GO_STORY"},
    }
    checks = {
        "title.fes": {
            223: b"if ($760)\nbtn1[3] disp 1",
            245: b"if ($1000)\nbtn1[9] disp 1",
            250: b"if ($1000)\nbtn1[5] disp 1",
            313: b"btn3[4] disp 1",
            327: b"btn3 enable 1",
            498: b"#OMAKE\ncall OUTBTN\nnext SETBTN02",
            557: b"#GO_EX_OMK",
            564: b"exit 23",
            570: b"#GO_EPG\nstrvar 200 c00_ep",
            614: b"if (@==3)\nnext OMAKE",
            629: b"if (@==5)\nnext GO_EPG",
            669: b"if (@==4)\nnext GO_EX_OMK",
        },
        "ttlflow.fes": {62: b"if (_EXITCODE_==23)\nnext EX_OMAKE"},
        "flow.fes": {51: b"if (_EXITCODE_==23)\nnext EX_OMAKEMODE",
                     115: b"#EX_OMAKEMODE\nendfes\nwait 1\nexecfes omake",
                     167: b"if (_EXITCODE_==30)\n$512 = -2\nendfes\nwait\nexeckcs sscript"},
        "omake.fes": {
            86: b"if ($1014)\n\\100 = 1\n\\101 = 1",
            90: b"if ($1012)\n\\102 = 1\n\\103 = 1",
            94: b"if ($1013)\n\\104 = 1",
            223: b"#btn.PUSH_L\nif (\\(100+@))",
        },
    }
    for name, checkpoints in transition.items():
        checks.setdefault(name, {}).update(checkpoints)
    replays = []
    for case, button, line in ((1, 6, 438), (2, 7, 453), (3, 8, 468), (0, 9, 483)):
        replay = f"replay_{case:02}"
        handler = f"GO_CASE{case:02}"
        target = f"c0{case}_01"
        flag = 202 + case * 10
        checks["title.fes"][line] = f"#{handler}\nstrvar 200 {replay}".encode()
        checks["title.fes"][617 + 3 * (button - 6)] = f"if (@=={button})\nnext {handler}".encode()
        require(scenes[replay] == ((0, 0, None, f"#{flag} = 1".encode()),
                                   (0, 1, None, f"next {target}".encode())), f"{replay}: entry changed")
        replays.append({"title_button": f"btn1[{button}]", "unlock_flag": 1000 + case,
                        "scene": replay, "sets_flag": flag, "successor": target})
    checks["title.fes"].update({260: b"if ($1001==0)\nbtn1[6] enable 0",
                                264: b"if ($1002==0)\nbtn1[7] enable 0",
                                268: b"if ($1003==0)\nbtn1[8] enable 0",
                                278: b"if ($1000==0)\nbtn1[9] enable 0"})
    assets = members(GAME, "fes.int", checks)
    require(assets.keys() == checks.keys(), "missing title or bonus FES")
    scripts = {name: [line.strip() for line in zlib.decompress(asset[1][16:]).splitlines()]
               for name, asset in assets.items()}
    for name, checkpoints in checks.items():
        lines = scripts[name]
        for line, expected in checkpoints.items():
            expected_lines = expected.splitlines()
            require(lines[line - 1:line - 1 + len(expected_lines)] == expected_lines,
                    f"{name}:{line}: title or bonus control changed")
    lines = scripts["omake.fes"]
    bonus = ["c01_h_03", "c01_h_04", "c02_h_02", "c02_h_04", "c03_h_03"]
    handlers = [b"GO_RIN01", b"GO_RIN02", b"GO_OLV01", b"GO_OLV02", b"GO_SMM01"]
    for index, (scene, handler) in enumerate(zip(bonus, handlers, strict=True)):
        require(lines[226 + 3 * index:228 + 3 * index]
                == [f"if (@=={index})".encode(), b"next " + handler], f"bonus button {index} changed")
        require(lines[148 + 13 * index:150 + 13 * index]
                == [b"#" + handler, f"strvar 200 {scene}".encode()]
                and scenes[scene][-1][-1] == b"next", f"bonus scene {scene} changed")
    return bonus, replays, {name: identity(assets[name]) | {"lines": {
        line: text.decode("ascii") for line, text in checkpoints.items()
    }} for name, checkpoints in transition.items()}


def main():
    opening = build_evidence(GAME)
    jp = load_commands("update02.int")
    cn = jp | load_commands("update03.int")
    require(jp == cn, "CN CASE control differs from the JP control subset")
    endings = check_routes(jp)
    questions = check_question_branches(jp)
    bonus, replays, transition_fes = check_title_and_bonus(jp)
    config = members(GAME, "config.int", {"meswnd.xml"})["meswnd.xml"]
    windows = ElementTree.fromstring(config[1])
    scripts = {name: windows.findtext(f"{name}/mes/script") for name in ("cinema", "case02")}
    require(scripts == {"cinema": "cinema.fes", "case02": "meswnd.fes"}, "CASE message window changed")
    require(windows.findtext("default/mes/script") == "meswnd.fes", "default transition message window changed")
    select_scripts = {node.text for node in windows.findall("*/fselect/script")}
    require(select_scripts == {"select.fes"}, "choice window changed")
    for case in opening["first_body_waits_for_branch_399_eq_0"]:
        case["window_script"] = scripts[case["assets"]["cn"]["view_mode"]]

    required_movies = {f"ed_c0{i}.mpg" for i in (0, 1, 2, 3)} | {"ed_c00_ep.mpg", "op_c00.mpg"}
    movie_entries, _, _ = entries_at(GAME / "movie.int", 0x1CF643B4)
    movie_names = {entry["name"].decode("ascii").lower() for entry in movie_entries}
    require(required_movies <= movie_names, "ending or CASE 0 movie missing")
    print(json.dumps({
        "status": "offline_verified",
        "fresh_main_order_pairs_per_language": len(ORDERS) ** 2,
        "first_epilogue_orders_per_language": len(ORDERS),
        "first_body_waits": opening["first_body_waits_for_branch_399_eq_0"],
        "window_config": identity(config),
        "choice_window_script": "select.fes",
        "movies": sorted(required_movies),
        "bonus_scenes": bonus,
        "question_branches": questions,
        "ending_checkpoints": endings,
        "title_replays": replays,
        "post_r242": {
            "default_window": "meswnd.fes",
            "tail_commands": [row for row in opening["prologue_tail_commands"] if row[0] >= 242],
            "fes": transition_fes,
        },
    }, ensure_ascii=True, indent=2))


if __name__ == "__main__":
    try:
        main()
    except StopIteration:
        raise SystemExit("the route requested an unexpected RNG draw or menu choice") from None
    except (KeyError, OSError, ValueError) as error:
        raise SystemExit(str(error)) from error

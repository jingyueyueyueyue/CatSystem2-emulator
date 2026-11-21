"""Verify Daydream transition assets and generate guarded title and CASE routes."""

import argparse
from hashlib import sha256
import json
from pathlib import Path
from struct import unpack_from
import zlib

from audit_runtime_evidence import blowfish_le_decrypt
from cst_variants import WORKSPACE, expand_cst, inspect_body
from generate_daydream_r96_route import CONTINUATION_SCENARIOS, ENTER, PLAN, build_continuation_scenario
from startup_inventory import entries_at


HERE = Path(__file__).resolve().parent
GAME = WORKSPACE / "games" / "\u767d\u663c\u5922\u306e\u9752\u5199\u771f"
R221_RECEIPT = WORKSPACE / "archive/2026-11/20261120_daydream_r221_receipt.json"
OUTPUT = HERE / "pc_story_17_r221_to_title_observe.json"
CASE_OUTPUT = HERE / "pc_story_17_title_to_first_case.json"
CASE_NEXT_OUTPUT = HERE / "pc_story_17_first_case_advance.json"
CASE_NEXT_PATCH = "generate_daydream_case_successor_player.patch"
CASE_PATCH = "generate_daydream_case_start_player.patch"
EVIDENCE = HERE / "pc_story_17_prologue_exit_evidence.json"
PATCH = "generate_daydream_prologue_exit_player.patch"
TITLE = {"state": "title", "scene": "-", "group": -1, "record": -1, "window": "title"}
TITLE_LABEL = "Daydream: stop at the settled post-prologue title"
CASE_TITLE_LABEL = "Daydream: verify the settled title story button"
CASE_STOP_LABEL = "Daydream: stop at the first CASE body wait"
CASE_NEXT_LABEL = "Daydream: stop at the second CASE body wait"
CASE_NEXT = {
    "c01_01_01": (66, 68, "cinema", [67], [(0x20, b"")]),
    "c02_01_01": (66, 71, "meswnd", [70], [(0x30, b"rdraw"), (0x30, b"pcm K_c02_01_002"),
                                           (0x21, b""), (0x20, b"")]),
    "c03_01_01": (62, 65, "cinema", [64], [(0x30, b"se 0 se4029"), (0x20, b"")]),
}
EXIT_LABEL = "r221: one guarded Enter, then observe natural return to title"
TAIL_CHECKPOINTS = (
    (222, "frameoff"), (225, "rdraw 60"), (228, "rdraw 120"), (232, "rdraw 120"),
    (237, "initialize_header"), (242, "fesds"), (272, "drawdef"), (273, "wipedef"), (274, "wait"),
)


def members(game, archive, wanted):
    path = game / archive
    entries, _, cipher = entries_at(path, 0 if archive == "update03.int" else 0x1CF643B4)
    found = {}
    with path.open("rb") as stream:
        for entry in entries:
            name = entry["name"].decode("ascii", errors="replace")
            if name.lower() not in wanted:
                continue
            if name.lower() in found or entry["size"] > 8 * 1024 * 1024:
                raise ValueError(f"invalid selected member {archive}::{name}")
            stream.seek(entry["offset"])
            data = stream.read(entry["size"])
            if len(data) != entry["size"]:
                raise ValueError(f"short read of {archive}::{name}")
            if cipher:
                data = blowfish_le_decrypt(cipher, data)
            found[name.lower()] = (f"{archive}::{name}", data)
    return found


def cst_rows(data):
    body, _ = expand_cst(data)
    inspect_body(body)
    _, groups, offsets, strings = unpack_from("<4I", body)
    for group in range(groups):
        count, start = unpack_from("<2I", body, 16 + group * 8)
        for record in range(start, start + count):
            relative, = unpack_from("<I", body, 16 + offsets + record * 4)
            position = 16 + strings + relative
            yield group, record, body[position + 1], body[position + 2:body.index(0, position + 2)]


def identity(resource):
    return {"member": resource[0], "sha256": sha256(resource[1]).hexdigest()}


def require(condition, message):
    if not condition:
        raise ValueError(message)


def build_evidence(game=GAME):
    plan = json.loads(PLAN.read_text(encoding="utf-8"))
    receipt = json.loads(R221_RECEIPT.read_text(encoding="utf-8"))
    r221 = receipt["last_verified_page"]
    enter = receipt["verified_enters"][-1]
    require((receipt["status"], receipt["target"], r221["group"], r221["wait_record"], r221["body_records"],
             r221["empty_body"]["record"], r221["empty_body"]["raw_bytes"]) ==
            ("verified", 221, 40, 221, [215], 220, 0), "archived r221 checkpoint does not match the exit anchor")
    require((enter["from"], enter["to"], enter["region"], enter["receiver"]) ==
            (205, 221, ENTER["region"], ENTER["receiver"])
            and enter["receipt_frame"] == r221["source_frame"]
            and r221["source_frame"] <= r221["natural_complete_frame"] < r221["frame"]
            and r221["wait_marked_frame"] < r221["frame"], "archived r221 runtime receipt is inconsistent")
    case_wrappers = {f"c0{number}_01.cst" for number in (1, 2, 3)}
    case_bodies = {f"c0{number}_01_01.cst" for number in (1, 2, 3)}
    dispatch = {"start.cst", "c00_00.cst", "brunch.cst", "brunch_a.cst"} | case_wrappers
    jp = members(game, "update02.int", {"prologue.cst"} | dispatch | case_bodies)
    cn = members(game, "update03.int", {"prologue.cst"} | dispatch | case_bodies)
    require(not (dispatch & cn.keys()), "a new update03 CST dispatch override needs review")
    tails = []
    for language, resources in (("cn", cn), ("jp", jp)):
        asset = resources["prologue.cst"]
        require(identity(asset) == {key: plan["resources"][language][key] for key in ("member", "sha256")},
                f"unexpected {language} prologue resource")
        rows = list(cst_rows(asset[1]))
        tail = [row for row in rows if row[1] >= 222]
        require(rows[221][:3] == (40, 221, 2) and [row[:3] for row in tail] ==
                [(41, record, 0x30) for record in range(222, 275)], "prologue tail layout changed")
        tails.append([(record, payload.decode("ascii")) for _, record, _, payload in tail])
    require(tails[0] == tails[1] and all(tails[0][record - 222] == (record, command)
                                       for record, command in TAIL_CHECKPOINTS),
            "prologue tail command checkpoints changed")

    fes_lines = {
        "flow.fes": {30: "execfes ttlflow", 31: "next TITLE_WAIT", 67: "#GAME_WAIT",
                     68: "if (_EXITCODE_!=-1)", 69: "next START_TITLE"},
        "ttlflow.fes": {35: "execfes title", 36: "next FES_WAIT"},
        "title.fes": {69: "btn1[0] sys_title 100 110 120 130 100 0 0", 120: "if ($542==1)",
                      121: "next IN2", 127: "strvar 200 prologue",
                      129: "$542 = 1", 207: "btn1[0] noact 0", 214: "btn1[0] disp 1",
                      256: "btn1 enable 1", 284: "next WAIT", 383: "#WAIT",
                      405: "#GO_STORY", 406: "strvar 200 start", 602: "#btn1.PUSH_L",
                      605: "if (@==0)", 606: "next GO_STORY"},
    }
    fes_assets = members(game, "fes.int", fes_lines.keys())
    fes_evidence = []
    for name, expected in fes_lines.items():
        asset = fes_assets[name]
        packed, expanded = unpack_from("<2I", asset[1], 4)
        text = zlib.decompress(asset[1][16:])
        require(asset[1][:4] == b"FES\0" and packed + 16 == len(asset[1]) and expanded == len(text),
                f"invalid FES container in {name}")
        lines = text.splitlines()
        require(all(lines[number - 1] == line.encode("ascii") for number, line in expected.items()),
                f"unexpected FES control in {name}")
        fes_evidence.append(identity(asset) | {"lines": expected})

    dispatch_evidence = {}
    for name in sorted(dispatch):
        asset = jp[name]
        rows = list(cst_rows(asset[1]))
        require(all(kind == 0x30 for _, _, kind, _ in rows), f"unexpected wait or text in {name}")
        dispatch_evidence[name] = identity(asset) | {
            "commands": [(record, payload.decode("ascii")) for _, record, _, payload in rows]
        }

    first_waits = []
    for name in sorted(case_bodies):
        cases = {}
        for language, resources in (("cn", cn), ("jp", jp)):
            body_records = []
            view_mode = None
            for group, record, kind, payload in cst_rows(resources[name][1]):
                if kind in (2, 3):
                    cases[language] = identity(resources[name]) | {
                        "group": group, "record": record, "kind": kind, "body_records": body_records,
                        "view_mode": view_mode,
                    }
                    break
                if kind == 0x30:
                    tokens = payload.split()
                    require(not set(tokens) & {b"next", b"select", b"select2", b"fselect"},
                            f"scene jump or choice before first body wait in {name}")
                    if tokens[:2] == [b"view", b"0"] and len(tokens) == 3:
                        view_mode = tokens[2].decode("ascii")
                if kind == 0x20 and payload.strip():
                    body_records.append(record)
        keys = ("group", "record", "kind", "body_records", "view_mode")
        require(tuple(cases["cn"][key] for key in keys) == tuple(cases["jp"][key] for key in keys)
                and cases["cn"]["kind"] == 2 and cases["cn"]["body_records"], f"first wait differs in {name}")
        first_waits.append({"scene": name.removesuffix(".cst"), "assets": cases})

    return {
        "schema": "daydream-prologue-exit-evidence/v1",
        "runtime_status": "r221 verified on the 19:06 player; title transition awaits replay",
        "r221": {"receipt": R221_RECEIPT.relative_to(WORKSPACE).as_posix(), "batch_result": receipt["result"],
                 "verified_page": r221, "incoming_enter": enter},
        "prologue_tail_group": 41,
        "prologue_tail_commands": tails[0],
        "prologue_tail_log_checkpoints": TAIL_CHECKPOINTS,
        "first_post_prologue_wait": {"position": TITLE, "flow": "TITLE_WAIT", "ttlflow": "FES_WAIT",
                                     "title": "WAIT", "button": "btn1[0]", "region": "resolve live", "inputs": 0},
        "fes_sources": fes_evidence,
        "later_dispatch_after_title_input": dispatch_evidence,
        "first_body_waits_for_branch_399_eq_0": first_waits,
    }


def build_exit_scenario():
    scenario = build_continuation_scenario(221)
    settings = scenario["17"]
    expected = json.loads(CONTINUATION_SCENARIOS[221].read_text(encoding="utf-8"))
    require(scenario == expected, "regenerate and check the existing r221 scenario before extending it")
    route = settings["route"]
    steps = route["steps"]
    terminal = steps[-1]
    steps[-1] = terminal | {"label": EXIT_LABEL, "key": 13, "keyboard_press": ENTER, "after": TITLE}
    steps.append({"label": TITLE_LABEL, "wait": TITLE})
    route["label"] = "Daydream: guarded prologue through r221, then observe title"
    settings["review"].update({
        "generator": f"scripts/analysis/{Path(__file__).name}",
        "player_patch": f"scripts/analysis/{PATCH}",
        "transition_sources": f"scripts/analysis/{EVIDENCE.name}",
        "acceptance": "Verify one Enter at r221 and a settled title wait with zero title inputs. "
                      "Use check_daydream_prologue.py --to title.",
    })
    return scenario


def build_case_scenario():
    scenario = build_exit_scenario()
    settings = scenario["17"]
    route = settings["route"]
    route.update(label="Daydream: one guarded title click to the first CASE body wait",
                 daydream_case_start=True)
    route["steps"][-1]["label"] = CASE_TITLE_LABEL
    settings["review"].update({
        "player_patch": f"scripts/analysis/{CASE_PATCH}",
        "acceptance": "Verify the r221 tail, one live btn1[0] left press, the ordered CASE loads, "
                      "and the first composed CASE body wait with no body input. "
                      "Use check_daydream_prologue.py --to case.",
    })
    return scenario


def check_case_successors(game=GAME):
    names = {f"{scene}.cst" for scene in CASE_NEXT}
    for archive in ("update02.int", "update03.int"):
        assets = members(game, archive, names)
        for scene, (first, second, _, sources, layout) in CASE_NEXT.items():
            rows = list(cst_rows(assets[f"{scene}.cst"][1]))
            require(rows[first][:3] == (0, first, 0x02) and rows[second][:3] == (1, second, 0x02),
                    f"{archive} {scene}: adjacent waits changed")
            edge = rows[first + 1:second]
            require(all(group == 1 and payload.strip() for group, _, _, payload in edge)
                    and [(kind, payload.strip() if kind == 0x30 else b"")
                         for _, _, kind, payload in edge] == layout
                    and [record for _, record, kind, _ in edge if kind == 0x20] == sources,
                    f"{archive} {scene}: unexpected command, choice, or body source between waits")


def build_case_successor_scenario():
    check_case_successors()
    scenario = build_case_scenario()
    settings = scenario["17"]
    settings["route"].update(label="Daydream: one guarded CASE Enter to the second body wait",
                             daydream_case_first_advance=True)
    settings["review"].update({
        "player_patch": f"scripts/analysis/{CASE_NEXT_PATCH}",
        "acceptance": "Verify the guarded title path, one CASE Enter with a same-frame VM resume, "
                      "and the adjacent same-load CASE body wait with no further input. "
                      "Use check_daydream_prologue.py --to case-next.",
    })
    return scenario


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    try:
        outputs = ((EVIDENCE, build_evidence()), (OUTPUT, build_exit_scenario()),
                   (CASE_OUTPUT, build_case_scenario()), (CASE_NEXT_OUTPUT, build_case_successor_scenario()))
        for path, value in outputs:
            payload = (json.dumps(value, ensure_ascii=True, indent=2) + "\n").encode("utf-8")
            if args.check:
                require(path.read_bytes() == payload, f"stale output: {path.name}")
            else:
                path.write_bytes(payload)
            print(path.name)
    except (KeyError, OSError, TypeError, ValueError) as error:
        parser.error(str(error))


if __name__ == "__main__":
    main()

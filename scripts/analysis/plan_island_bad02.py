"""Index ISLAND BAD_02 checkpoints and generate guarded replay segments."""

from __future__ import annotations

import argparse
from hashlib import sha256
import json
from pathlib import Path
import re
from struct import unpack_from

from cst_variants import WORKSPACE, expand_cst, inspect_body, read_entries


ROUTE = (
    "com01", "OPMOVIE", "OPMOVIE_END", "com01B", "com01_selA_1",
    "com01_flowA_1", "com01_flowA", "com01_flowB_1", "com01_selB",
    "com01_selB_1", "com01_selC", "com01_selC_1", "BAD_02",
)
ARCHIVE_NAME_SEED = 0x08787C5D
WAIT_KINDS = {2: "input", 3: "page"}
MESWND_PREFIX = Path(__file__).with_name("pc_story_island_meswnd_53_to_54_probe.json")
MESWND_ANCHORS = ((53, 514), (54, 518), (121, 810))
MESWND_POINTS = [[557, 530], [557, 324]]
MESWND_REGION = 82
OPMOVIE_END_WAIT = {"state": "page", "scene": "OPMOVIE_END", "group": 0, "record": 19}
COMMANDS = {
    b"auto", b"draw", b"fes", b"fesjs", b"frameoff", b"frameon", b"fselect",
    b"if", b"movie", b"next", b"rdraw", b"return", b"rwipe", b"select",
    b"set", b"view", b"wait", b"wipe",
}
COMMAND_TOKEN = re.compile(rb"[a-z]+", re.IGNORECASE)


def checkpoint_rows(body):
    inspect_body(body)
    _, groups, offsets, strings = unpack_from("<IIII", body)
    waits = []
    boundaries = []
    pending_body = []
    for group in range(groups):
        count, start = unpack_from("<II", body, 16 + group * 8)
        for record in range(start, start + count):
            relative, = unpack_from("<I", body, 16 + offsets + record * 4)
            position = 16 + strings + relative
            kind = body[position + 1]
            payload = body[position + 2:body.index(b"\0", position + 2)]
            if kind in WAIT_KINDS:
                waits.append({"group": group, "record": record,
                              "kind": WAIT_KINDS[kind], "nonempty_body_records": pending_body})
                pending_body = []
            elif kind == 0x20 and payload:
                pending_body.append(record)
            elif kind == 0x30:
                stripped = payload.lstrip()
                token = COMMAND_TOKEN.match(stripped)
                if stripped.startswith(b"#") or token and token[0].lower() in COMMANDS:
                    command = payload.decode("ascii") if payload.isascii() else None
                    boundaries.append({"group": group, "record": record, "command": command})
    return waits, boundaries


def archive_snapshot(path):
    scenes = {}
    names = {scene.lower().encode("ascii") + b".cst": scene for scene in ROUTE}
    for entry, _, packed in read_entries(path, ARCHIVE_NAME_SEED):
        name = names.get(entry["name"].lower())
        if name is None:
            continue
        if name in scenes:
            raise ValueError(f"duplicate {name} in {path.name}")
        body, _ = expand_cst(packed)
        waits, boundaries = checkpoint_rows(body)
        scenes[name] = {"sha256": sha256(packed).hexdigest(), "waits": waits, "boundaries": boundaries}
    if missing := names.values() - scenes.keys():
        raise ValueError(f"missing members in {path.name}: {sorted(missing)}")
    return scenes


def build_plan(game):
    original = archive_snapshot(game / "scene.int")
    localized = archive_snapshot(game / "cnscene.int")
    scenes = {}
    for name in ROUTE:
        japanese, chinese = original[name], localized[name]
        if wait_shape(japanese["waits"]) != wait_shape(chinese["waits"]) or japanese["boundaries"] != chinese["boundaries"]:
            raise ValueError(f"JP/CN control flow differs for {name}; review the actual archive separately")
        scenes[name] = {
            "jp_sha256": japanese["sha256"], "cn_sha256": chinese["sha256"],
            "waits": japanese["waits"], "boundaries": japanese["boundaries"],
        }
    ending_waits = scenes["BAD_02"]["waits"]
    if not ending_waits or (ending_waits[-1]["group"], ending_waits[-1]["record"]) != (37, 207):
        raise ValueError("unexpected BAD_02 terminal wait")
    clear_flag = [row for row in scenes["BAD_02"]["boundaries"] if row["group"] == 38 and row["record"] == 212]
    if len(clear_flag) != 1 or not clear_flag[0]["command"] or "#722" not in clear_flag[0]["command"]:
        raise ValueError("BAD_02 clear flag record did not match the route")
    return {
        "schema": "cs2-island-bad02-source-inventory/v1", "status": "static_evidence_only_no_input",
        "scene_order_candidate": ROUTE, "scenes": scenes,
        "requires_runtime": [
            "Resolve the actual CST winner per scene and match its digest and load instance.",
            "Wait for a matching live reader, current submitted body and natural transitions.",
            "Resolve branch conditions and all choice rows from live state; do not replay the scene list blindly.",
            "Require the final card, return to title, and shared slot 722 bit 1 before counting completion.",
        ],
    }


def wait_shape(waits):
    return [(row["group"], row["record"], row["kind"]) for row in waits]


def meswnd_position(group, record):
    return {"state": "page", "scene": "com01", "group": group, "record": record, "window": "meswnd"}


def meswnd_step(origin, label, after, *, scene="com01", region=MESWND_REGION):
    point = MESWND_POINTS[1]
    return {
        "label": label,
        "wait": {**meswnd_position(*origin), "scene": scene, "hit": {"point": point, "region": region}},
        "probe_points": MESWND_POINTS,
        "click": point,
        "mouse_press": {"candidates": [3, region, 7], "region": region,
                        "key": 2, "group": 0, "response": 0},
        "after": after,
    }


def meswnd_scenario(plan, through):
    start, verified, final = MESWND_ANCHORS
    waits = [row for row in wait_shape(plan["scenes"]["com01"]["waits"])
             if start[0] <= row[0] <= final[0]]
    anchors = [(group, record) for group, record, _ in waits]
    if (
        anchors[:2] != [start, verified] or anchors[-1:] != [final]
        or [group for group, _ in anchors] != list(range(start[0], final[0] + 1))
        or any(kind != "input" for _, _, kind in waits)
        or any(origin[1] >= successor[1] for origin, successor in zip(anchors, anchors[1:]))
    ):
        raise ValueError("com01 message-window section is no longer a contiguous series of input waits")
    if through not in anchors[1:]:
        raise ValueError("endpoint must be an exact com01 message-window input wait from 54/518 through 121/810")

    scenario = json.loads(MESWND_PREFIX.read_text(encoding="utf-8"))
    case = scenario["3"]
    route = case["route"]
    steps = route["steps"]
    probe = steps.pop()
    if (
        probe["wait"] != meswnd_position(*verified) or probe["probe_points"] != MESWND_POINTS
        or set(probe) != {"label", "wait", "probe_points"}
        or steps[-1]["after"] != probe["wait"] or route["page_fallback_before"] > len(steps) - 1
    ):
        raise ValueError("the verified com01 message-window prefix changed")

    case["frames"] = max(case["frames"], 24000)
    route["label"] = f"ISLAND: guarded com01 message-window chain through {through[0]}/{through[1]}"
    selected = anchors[1:anchors.index(through) + 1]
    for origin, successor in zip(selected, selected[1:]):
        label = f"{origin[0]}/{origin[1]}: guarded press toward {successor[0]}/{successor[1]}"
        steps.append(meswnd_step(origin, label, meswnd_position(*successor)))
    steps.append({
        **probe, "label": f"{through[0]}/{through[1]}: observe both points without input",
        "wait": meswnd_position(*through),
    })
    return scenario


def opmovie_scenario(plan):
    scenes = plan["scenes"]
    outro = {row["record"]: row["command"] for row in scenes["com01"]["boundaries"] if row["group"] == 122}
    movie = {row["record"]: row["command"].strip() for row in scenes["OPMOVIE"]["boundaries"] if row["command"]}
    if (any(outro.get(record) != command for record, command in [(823, "rdraw 60"), (824, "wait"), (827, "next OPMOVIE")])
        or any(row["group"] >= 122 for row in scenes["com01"]["waits"])
        or movie.get(3) != "if(#849==0 && (#612&1)==0) movie opmovie.webm 1 100"
        or movie.get(4) != "if(#849==0 && (#612&1)!=0) movie opmovie.webm 3 100"
        or movie.get(7) != "next OPMOVIE_END"
        or wait_shape(scenes["OPMOVIE_END"]["waits"])[:1] != [(0, 19, "input")]):
        raise ValueError("the ISLAND com01 ending or OP transition changed")

    final = MESWND_ANCHORS[-1]
    scenario = meswnd_scenario(plan, final)
    route = scenario["3"]["route"]
    route["label"] = "ISLAND: guarded com01 exit through the natural opening movie"
    route["idle_frames"] = 18000
    route["steps"][-1] = {
        **meswnd_step(final, "121/810: one final press, then wait for the opening movie", OPMOVIE_END_WAIT),
        "shared_flags": [{"slot": 849, "mask": 0xffffffff, "expected": 0}],
    }
    route["steps"].append({
        "label": "OPMOVIE_END 0/19: observe the first post-movie page",
        "wait": OPMOVIE_END_WAIT,
        "probe_points": MESWND_POINTS,
    })
    return scenario


def postmovie_scenario(plan, through):
    scene = "OPMOVIE_END"
    anchors = [(row["group"], row["record"], row["kind"]) for row in plan["scenes"][scene]["waits"]]
    try:
        stop = anchors.index((*through, "input"))
    except ValueError as error:
        raise ValueError("endpoint must match a post-movie input wait") from error
    selected = anchors[:stop + 1]
    if (
        stop == 0 or selected[:2] != [(0, 19, "input"), (1, 21, "input")]
        or any(group != index or kind != "input" for index, (group, _, kind) in enumerate(selected))
        or any(start[1] >= end[1] for start, end in zip(selected, selected[1:]))
    ):
        raise ValueError("post-movie section is no longer a contiguous series of input waits")

    def position(wait):
        group, record, _ = wait
        return {**OPMOVIE_END_WAIT, "window": "meswnd", "group": group, "record": record}

    scenario = opmovie_scenario(plan)
    route = scenario["3"]["route"]
    route["label"] = f"ISLAND: guarded post-movie chain through {through[0]}/{through[1]}"
    steps = route["steps"]
    steps.pop()
    steps[-1]["after"] = position(selected[0])
    for origin, successor in zip(selected, selected[1:]):
        label = f"{scene} {origin[0]}/{origin[1]}: guarded press toward {successor[0]}/{successor[1]}"
        region = 108 if origin[0] >= 136 else 81
        steps.append(meswnd_step(origin[:2], label, position(successor), scene=scene, region=region))
    steps.append({"label": f"{scene} {through[0]}/{through[1]}: observe both points without input",
                  "wait": position(selected[-1]), "probe_points": MESWND_POINTS})
    return scenario


def first_branch_scenario(plan):
    scenes = plan["scenes"]
    outro = {row["record"]: row["command"].strip() for row in scenes["OPMOVIE_END"]["boundaries"]
             if row["group"] == 333 and row["command"]}
    transition = {row["record"]: row["command"] for row in scenes["com01B"]["boundaries"]
                  if row["group"] == 0}
    if outro != {2064: "frameoff", 2065: "wait", 2071: "rwipe wipe01 30",
                 2072: "wait", 2073: "next com01B"}:
        raise ValueError("unexpected OPMOVIE_END tail")
    if (transition.get(3) != "if(#722 == 0 && #525 == 0) next com01_selA_1"
        or wait_shape(scenes["com01_selA_1"]["waits"])[:1] != [(0, 19, "input")]):
        raise ValueError("unexpected first-play branch")

    final = (332, 2063)
    scenario = postmovie_scenario(plan, final)
    route = scenario["3"]["route"]
    route["label"] = "ISLAND: guarded first-play transition from OPMOVIE_END to com01_selA_1"
    branch = {**meswnd_position(0, 19), "scene": "com01_selA_1"}
    route["steps"][-1] = {
        **meswnd_step(final, "OPMOVIE_END 332/2063: enter first-play branch", branch,
                      scene="OPMOVIE_END", region=108),
        "shared_flags": [{"slot": slot, "mask": 0xffffffff, "expected": 0} for slot in (722, 525)],
    }
    route["steps"].append({
        "label": "com01_selA_1 0/19: observe first-play branch",
        "wait": branch,
        "probe_points": MESWND_POINTS,
    })
    return scenario


def beach_observation_scenario(plan):
    scenario = first_branch_scenario(plan)
    route = scenario["3"]["route"]
    route["label"] = "ISLAND: guarded first-play route to the beach full-window page"
    steps = route["steps"]
    steps.pop()
    sections = (
        ("com01_selA_1", (22, 94), "com01_flowA_1", 1, True),
        ("com01_flowA_1", (0, 1), "com01_flowA", 1, False),
        ("com01_flowA", (2, 5), "com01_flowB_1", 1, True),
        ("com01_flowB_1", (0, 1), "com01_selB_1", 26, True),
    )
    for scene, final, successor_scene, successor_record, conditional in sections:
        waits = wait_shape(plan["scenes"][scene]["waits"])
        if (not waits or waits[-1][:2] != final
            or any(group != index or kind != "input" for index, (group, _, kind) in enumerate(waits))
            or any(start[1] >= end[1] for start, end in zip(waits, waits[1:]))):
            raise ValueError(f"expected contiguous input waits through {scene} {final}")
        for origin, successor in zip(waits, waits[1:]):
            after = {**meswnd_position(*successor[:2]), "scene": scene}
            label = f"{scene} {origin[0]}/{origin[1]}: guarded press toward {successor[0]}/{successor[1]}"
            steps.append(meswnd_step(origin[:2], label, after, scene=scene, region=108))
        if wait_shape(plan["scenes"][successor_scene]["waits"])[:1] != [(0, successor_record, "input")]:
            raise ValueError(f"unexpected first wait in {successor_scene}")
        after = {**meswnd_position(0, successor_record), "scene": successor_scene}
        if successor_scene == "com01_selB_1":
            after.update(state="page_probe", window="fullwnd")
        step = meswnd_step(final, f"{scene}: guarded transition to {successor_scene}", after,
                           scene=scene, region=108)
        if conditional:
            step["shared_flags"] = [{"slot": 525, "mask": 16, "expected": 0}]
        steps.append(step)
    steps.append({
        "label": "com01_selB_1 0/26: observe full-window controls without input",
        "wait": after,
        "probe_points": MESWND_POINTS,
    })
    return scenario


def beach_message_window_scenario(plan, *, through_choice=False):
    scene = "com01_selB_1"
    beach = plan["scenes"][scene]
    if (wait_shape(beach["waits"])[:3] != [(0, 26, "input"), (1, 67, "input"), (2, 93, "input")]
        or not any(row["record"] == 29 and row["command"] == "view 1 meswnd" for row in beach["boundaries"])):
        raise ValueError("unexpected beach message-window transition")
    scenario = beach_observation_scenario(plan)
    route = scenario["3"]["route"]
    route["label"] = "ISLAND: guarded beach full-window to message-window transition"
    old_probe = route["steps"].pop()
    point = MESWND_POINTS[1]
    after = {**meswnd_position(1, 67), "scene": scene}
    steps = route["steps"]
    steps.append({
        "label": f"{scene} 0/26: one guarded full-window press",
        "wait": {**old_probe["wait"], "hit": {"point": point, "region": 108}},
        "probe_points": MESWND_POINTS,
        "click": point,
        "mouse_press": {"candidates": [3, 108, 24, 7], "region": 24,
                        "key": 0, "group": 0, "response": 1},
        "after": after,
    })
    if through_choice:
        waits = wait_shape(beach["waits"])[1:]
        tail = {(row["group"], row["record"], row["command"]) for row in beach["boundaries"]}
        if (waits[-1] != (51, 361, "input") or (52, 362, "next com01_selC") not in tail
            or any(index != row[0] or row[2] != "input" for index, row in enumerate(waits, start=1))):
            raise ValueError("unexpected beach message-window waits or choice transition")
        route["label"] = "ISLAND: guarded beach route to the choice menu"
        choice = {"state": "choice", "scene": "com01_selC", "window": "select"}
        successors = [{**meswnd_position(*row[:2]), "scene": scene} for row in waits[1:]] + [choice]
        for origin, end in zip(waits, successors):
            step = meswnd_step(origin[:2], f"{scene} {origin[0]}/{origin[1]}: guarded beach advance",
                               end, scene=scene, region=108)
            step["mouse_press"]["candidates"] = [3, 108, 61, 7]
            steps.append(step)
        steps.append({"label": "com01_selC: observe choice without input", "wait": choice,
                      "probe_points": MESWND_POINTS, "capture": True})
    else:
        steps.append({"label": f"{scene} 1/67: observe the rebuilt message window without input",
                      "wait": after, "probe_points": MESWND_POINTS})
    return scenario


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    modes = parser.add_mutually_exclusive_group()
    modes.add_argument("--meswnd-through", metavar="GROUP/RECORD",
                       help="generate a state route to an exact com01 message-window checkpoint")
    modes.add_argument("--observe-opmovie-end", action="store_true",
                       help="generate the guarded com01 exit with an input-free post-movie probe")
    modes.add_argument("--postmovie-through", metavar="GROUP/RECORD",
                       help="generate the post-movie route to an exact input wait")
    modes.add_argument("--observe-first-branch", action="store_true",
                       help="follow the first-play branch from the end of OPMOVIE_END")
    modes.add_argument("--observe-beach", action="store_true",
                       help="advance to the first full-window page at the beach")
    modes.add_argument("--observe-beach-message-window", action="store_true",
                       help="perform the guarded beach full-window press, then observe the new message window")
    modes.add_argument("--observe-beach-choice", action="store_true",
                       help="advance through beach dialogue and stop at the choice menu without selecting")
    modes.add_argument("--bad02-ending", action="store_true",
                       help="select the first beach row and follow the guarded BAD_02 route to the title")
    args = parser.parse_args()
    plan = build_plan(WORKSPACE / "games" / "ISLAND")
    try:
        endpoint = args.meswnd_through or args.postmovie_through
        if endpoint is not None:
            match = re.fullmatch(r"(\d+)/(\d+)", endpoint, re.ASCII)
            if match is None:
                parser.error("route endpoint must be GROUP/RECORD")
            generate = meswnd_scenario if args.meswnd_through is not None else postmovie_scenario
            plan = generate(plan, tuple(map(int, match.groups())))
        elif args.observe_opmovie_end:
            plan = opmovie_scenario(plan)
        elif args.observe_first_branch:
            plan = first_branch_scenario(plan)
        elif args.observe_beach:
            plan = beach_observation_scenario(plan)
        elif args.observe_beach_message_window:
            plan = beach_message_window_scenario(plan)
        elif args.observe_beach_choice or args.bad02_ending:
            plan = beach_message_window_scenario(plan, through_choice=True)
            if args.bad02_ending:
                case = plan["3"]
                case["frames"] = max(case["frames"], 36000)
                route = case["route"]
                route["label"] = "ISLAND: new game through BAD_02 and return to title"
                route["island_bad02"] = True
                route["steps"][-1].pop("capture")
    except ValueError as error:
        parser.error(str(error))
    print(json.dumps(plan, indent=2))


if __name__ == "__main__":
    main()

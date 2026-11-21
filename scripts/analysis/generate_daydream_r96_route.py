"""Generate sample 17's guarded prologue scenarios and continuation data."""

import argparse
import json
from pathlib import Path


HERE = Path(__file__).resolve().parent
PREFIX = HERE / "pc_story_17_r18_to_r83_guarded_enter.json"
PLAN = HERE / "pc_story_17_r83_to_prologue_exit_plan.json"
OUTPUT = HERE / "pc_story_17_r18_to_r96_guarded_enter.json"
CONTINUATION_OUTPUT = HERE / "pc_story_17_r96_to_r221_route_data.json"
CONTINUATION_SCENARIOS = {
    107: HERE / "pc_story_17_r18_to_r107_guarded_enter.json",
    221: HERE / "pc_story_17_r18_to_r221_guarded_enter.json",
}
PLAYER_PATCH = "generate_daydream_r96_player.patch"
PREFIX_WAITS = (18, 22, 25, 28, 31, 33, 35, 37, 40, 44, 47, 55, 57, 62, 64, 75, 77, 79, 81, 83)
ENTER = {"region": 62, "receiver": 8, "key": 2, "group": 0, "response": 0}
PROBE_POINTS = [[640, 395]]


def wait(group, record, state="page"):
    return {"state": state, "scene": "prologue", "group": group, "record": record, "window": "cinema"}


def check_plan(plan):
    anchor = plan["observed_anchor"]
    if (plan["kind"], plan["title_id"], plan["scene"]) != ("analysis_only", "17", "prologue"):
        raise ValueError("wrong sample or scene in the source plan")
    if (anchor["group"], anchor["wait_record"], anchor["body_records"]) != (19, 83, [82]):
        raise ValueError("source plan must join the observed g19/r83")
    pages = plan["static_successors"]
    if [page["group"] for page in pages] != list(range(20, 41)):
        raise ValueError("source plan must account for every remaining dialogue wait")
    previous_wait = 83
    empty_pages = []
    for page in pages:
        record = page["wait_record"]
        body = page["body_records"]
        empty = page.get("empty_records", [])
        if (not body or body + empty != sorted(set(body + empty))
                or not previous_wait < body[0] <= (body + empty)[-1] < record):
            raise ValueError(f"invalid source records for g{page['group']}/r{record}")
        if empty:
            empty_pages.append((record, body, empty))
        continuation = page.get("newline_continuation")
        if continuation is not None and (len(body) < 2 or continuation != body[-1]):
            raise ValueError(f"newline continuation is outside the ordered body at r{record}")
        previous_wait = record
    if empty_pages != [(96, [90], [95]), (221, [215], [220])]:
        raise ValueError("empty bodies must match the r96 and r221 source boundaries")
    empty_observations = [
        (row["wait_record"], row["nonempty_record"], row["nonempty_bytes_cn_jp"],
         row["empty_record"], row["empty_bytes_cn_jp"])
        for row in plan["empty_body_observations"]
    ]
    if empty_observations != [(96, 90, [38, 48], 95, [0, 0]), (221, 215, [36, 42], 220, [0, 0])]:
        raise ValueError("unexpected source lengths for the two empty-body waits")
    tail = plan["transition_stop"]
    if (tail["group"], tail["absolute_record_span"], tail["dialogue_wait_count"], tail["destination"]) != (
            41, [222, 274], 0, None):
        raise ValueError("the prologue tail must have no inferred dialogue successor")
    return pages[0]


def continuation_plan(plan):
    check_plan(plan)
    pages = plan["static_successors"]
    verified = plan["verified_r96"]
    if (verified["group"], verified["wait_record"], verified["body_records"],
            verified["empty_record"], verified["empty_raw_bytes"]) != (20, 96, [90], 95, 0):
        raise ValueError("continuation must join the verified r96 anchor")
    if not (verified["exact_match"] is True and verified["composited"] is True
            and verified["terminal_input"] is False and verified["load_id"] > 0
            and verified["enter_receipt_frame"] == verified["enter_down_frame"] + 1
            and verified["enter_receipt_frame"] <= verified["source_frame"]
            <= verified["natural_complete_frame"] < verified["frame"]
            and verified["wait_marked_frame"] < verified["frame"]):
        raise ValueError("missing completed r96 runtime evidence")
    return {
        "kind": "analysis_only",
        "title_id": "17",
        "scene": "prologue",
        "window": "cinema",
        "wait_kind": 2,
        "runtime_status": "r96 verified. The r107 and r221 scenarios require the player patch and PC replay.",
        "player_patch": f"scripts/analysis/{PLAYER_PATCH}",
        "scenarios": {str(target): f"scripts/analysis/{path.name}" for target, path in CONTINUATION_SCENARIOS.items()},
        "source_plan": f"scripts/analysis/{PLAN.name}",
        "verified_r96": verified,
        "first_run": {"from_wait": 96, "terminal_wait": 107, "guarded_enters": 1},
        "r96_input_gate": {
            "wait": wait(20, 96, "page_probe"),
            "visible_body_records": [90],
            "zero_byte_body_records": [95],
            "expected_successor": wait(21, 107),
            "successor_body_records": [106],
            "checks": [
                "Require the exact r96 live body and the adjacent r107 CST wait in the current document",
                "Require the current region 62 to be enabled and bound to receiver 8",
                "Resolve owner and load from the current run; require the same CST context and load at r107",
            ],
        },
        "physical_input": {"key": 13, "keyboard_press": ENTER, "root_resume": [2, 0, 0, 0],
                           "presses_per_edge": 1, "resume_on_first_frame": True,
                           "require_release_before_next_input": True},
        "edges": [{"before": before, "after": after} for before, after in zip(pages, pages[1:])],
        "full_run": {"from_wait": 96, "terminal_wait": 221, "guarded_enters": len(pages) - 1},
        "terminal_wait": 221,
        "terminal_input": False,
        "page_conditions": [
            "Unique settled cinema MES_SHOW and flow GAME_WAIT, current-frame local WAIT_KEY",
            "Exact CST 0x02 wait and [input] marker, same current context and load",
            "Exact ordered body sources, fresh natural completion, same-epoch commit and composite",
            "Each advance requires one live physical Enter consumer and one matching VM resume",
        ],
        "empty_body_observations": plan["empty_body_observations"],
        "command_checkpoints": plan["command_checkpoints"],
    }


def build_scenario():
    first = check_plan(json.loads(PLAN.read_text(encoding="utf-8")))
    prefix = json.loads(PREFIX.read_text(encoding="utf-8"))["17"]["route"]
    steps = prefix["steps"]
    waits = [wait(group, record) for group, record in enumerate(PREFIX_WAITS)]
    if prefix["page_scenes"] != [] or len(steps) != len(waits):
        raise ValueError("r83 prefix must be an explicit route without fallback")
    for step, before, after in zip(steps, waits, waits[1:]):
        if (step["wait"], step["after"], step["key"], step["keyboard_press"]) != (before, after, 13, ENTER):
            raise ValueError(f"unguarded r83 prefix at r{before['record']}")
        if set(step) - {"label", "wait", "after", "key", "keyboard_press", "probe_points"}:
            raise ValueError(f"unexpected input or settings in the r83 prefix at r{before['record']}")
    if steps[-1]["wait"] != waits[-1] or set(steps[-1]) - {"label", "wait", "probe_points"}:
        raise ValueError("r83 prefix must end at an input-free checkpoint")

    probe = wait(first["group"], first["wait_record"], "page_probe")
    return {
        "17": {
            "frames": 12000,
            "route": {
                "label": "Daydream: verified r83 prefix plus one Enter to observe r96",
                "idle_frames": 1200,
                "page_scenes": [],
                "steps": steps[:-1] + [
                    {
                        "label": "r83: one guarded Enter to observe r96",
                        "wait": waits[-1],
                        "key": 13,
                        "keyboard_press": ENTER,
                        "after": probe,
                        "probe_points": PROBE_POINTS,
                    },
                    {
                        "label": "r96: terminal observation of body r90 and empty r95",
                        "wait": probe,
                        "probe_points": PROBE_POINTS,
                    },
                ],
            },
            "review": {
                "static_source_plan": f"scripts/analysis/{PLAN.name}",
                "generator": f"scripts/analysis/{Path(__file__).name}",
                "entry": "Normal startup in a fresh profile",
                "acceptance": "Require the r83 Enter receipt and VM resume, then inspect r96's live cinema wait, "
                              "same-load source r90, natural completion, commit and composition after empty r95. "
                              "The route sends no input at r96.",
            },
        },
    }


def build_continuation_scenario(through):
    plan = continuation_plan(json.loads(PLAN.read_text(encoding="utf-8")))
    scenario = build_scenario()
    settings = scenario["17"]
    route = settings["route"]
    steps = route["steps"]
    anchor = steps.pop()["wait"]
    gate = plan["r96_input_gate"]
    if anchor != gate["wait"] or steps[-1]["after"] != anchor:
        raise ValueError("r96 continuation must reuse the observed startup prefix")

    for edge in plan["edges"]:
        before, after = edge["before"], edge["after"]
        target = wait(after["group"], after["wait_record"])
        steps.append({
            "label": f"r{before['wait_record']}: one guarded Enter toward r{target['record']}",
            "wait": anchor,
            "key": 13,
            "keyboard_press": ENTER,
            "after": target,
            "probe_points": PROBE_POINTS,
        })
        anchor = target
        if target["record"] == through:
            break
    else:
        raise ValueError(f"r{through} is not a continuation checkpoint")

    steps.append({"label": f"r{through}: terminal observation", "wait": anchor, "probe_points": PROBE_POINTS})
    route["label"] = f"Daydream: guarded prologue route through terminal r{through}"
    press_count = anchor["group"] - 20
    settings["review"].update({
        "player_patch": f"scripts/analysis/{PLAYER_PATCH}",
        "acceptance": f"Run check_daydream_prologue.py --to {through} against the batch results. "
                      f"Require {press_count} fresh guarded Enter {'receipt' if press_count == 1 else 'receipts'} "
                      "from r96 onward, "
                      f"ending with the composed body and zero input at r{through}.",
    })
    return scenario


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--check", action="store_true", help="check the existing output without writing")
    mode.add_argument("--continuation-plan", action="store_true", help="print analysis data for r96 through r221")
    mode.add_argument("--write-continuation-plan", action="store_true", help="write the r96 to r221 analysis file")
    parser.add_argument("--through", type=int, choices=(96, *CONTINUATION_SCENARIOS),
                        help="scenario terminal record (default: 96)")
    args = parser.parse_args()
    if args.through is not None and (args.continuation_plan or args.write_continuation_plan):
        parser.error("--through applies only to scenario generation and checks")
    try:
        if args.continuation_plan or args.write_continuation_plan:
            plan = json.loads(PLAN.read_text(encoding="utf-8"))
            data = json.dumps(continuation_plan(plan), ensure_ascii=True, indent=2) + "\n"
            if args.write_continuation_plan:
                CONTINUATION_OUTPUT.write_bytes(data.encode("utf-8"))
                print(CONTINUATION_OUTPUT)
            else:
                print(data, end="")
            return
        target = args.through or 96
        output = OUTPUT if target == 96 else CONTINUATION_SCENARIOS[target]
        scenario = build_scenario() if target == 96 else build_continuation_scenario(target)
        data = (json.dumps(scenario, ensure_ascii=True, indent=2) + "\n").encode("utf-8")
        if args.check:
            if output.read_bytes() != data:
                parser.error(f"stale scenario: {output}")
        else:
            output.write_bytes(data)
    except (KeyError, OSError, TypeError, ValueError) as error:
        parser.error(str(error))
    terminal = scenario["17"]["route"]["steps"][-1]["wait"]
    print(f"17: {terminal['group']} guarded Enter presses, terminal g{terminal['group']}/r{target} {terminal['state']}")
    print(output)


if __name__ == "__main__":
    main()

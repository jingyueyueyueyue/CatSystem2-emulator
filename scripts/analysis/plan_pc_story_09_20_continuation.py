"""Read pinned CSTs and generate guarded continuation scenarios and audit tables."""

import argparse
import copy
import hashlib
import json
from pathlib import Path
import re

from check_pc_story_11_13_sources import read_cst, require


HERE = Path(__file__).resolve().parent
RUNTIME_SCENES = HERE.parents[1] / "simulator" / "crates" / "desktop" / "src" / "cst_context_native.rs"
SCENES = {
    "sc01": (0x16580A97, "cp932", "7c0d796d069db416291effc217aa3664ba449f23ce43b88a11a5d13cfff1a55e"),
    "sc02": (0x16580A97, "cp932", "8284a2ad28f30b0f82008ae443b7dea6f811427d8c2f3d5314d6467e89364b10"),
    "A_01_01": (0x02CD76AE, "gbk", "92de81ed6532c41611106715b4d8185cd5210ff0b9a177586e1d4d9f1b839176"),
    "A_01_02": (0x02CD76AE, "gbk", "40e144527644238f1a97f8b1a1e2eda841eecebd7f62909e2108ed4af1deab10"),
    "A_01_03": (0x02CD76AE, "gbk", "7b314b3e706ad3e31b39fe86387745947e99e180b18f0873e7f3d70d655407fb"),
}
KANMI_MODES = {"r134": (28, 7100), "r256": (52, 14000), "r983": (208, 30000), "sc02_r10": (294, 100000),
               "sc02_r147": (294, 115000), "sc02_r147_save": (294, 115000),
               "sc02_r547": (294, 145000), "sc02_r562": (294, 145000),
               "sc02_r1379": (294, 195000)}
KANMI_SC02_PAGES = {"sc02_r10": 1, "sc02_r147": 26, "sc02_r147_save": 26,
                  "sc02_r547": 101, "sc02_r562": 102, "sc02_r1379": 265}
KANMI_SC02_SEPARATED_BODY = {101: (555, 560, 561), 201: (1076,), 202: (1083,), 203: (1102,)}
KANMI_LOAD_MODES = {
    149: (26, "kanmi-load-sc02-r147/v1", 14000),
    562: (101, "kanmi-load-sc02-r562/v1", 55000),
    1379: (264, "kanmi-load-sc02-r1379/v1", 90000),
}
REBELLIONS_MODES = {"r61": (13, 11000), "r189": (27, 17000), "novel_r211": (27, 20000)}
REBELLIONS_NOVEL_FES = {
    "novelwnd01.fes": "38baff83143a3651fdb675e4aefd2778c31d0d05019c8a13c20d7b0624916490",
    "novelwnd01m.fes": "a6c275bd0db99bb0ca182d1ab4e8ab541fa9c1329797245541340b3620cfdcc5",
}
FLOW_COMMAND = re.compile(r"\b(?:next|select|jump|goto|choice|movie)\b", re.IGNORECASE)
DIRECT_NEXT = re.compile(r"next ([A-Za-z0-9_]+)")
BODY_CONTROL_ONLY = re.compile(rb"(?:\\[nr]|\\p[clr]?)*")


def read_scene(game, scene, identity=None):
    seed, _, digest = SCENES[scene] if identity is None else identity
    records, _ = read_cst(game, f"scene.int::{scene}.cst", seed, digest)
    return records


def scene_steps(records, scene, codec=None):
    codec = SCENES[scene][1] if codec is None else codec
    steps = []
    start = 0
    for wait, (group, _, kind, _) in enumerate(records):
        if kind not in (2, 3):
            continue
        bodies = [{"record": index, "ordinal": ordinal, "bytes": len(data),
                   "sha256": hashlib.sha256(data).hexdigest()}
                  for index, (g, ordinal, k, data) in enumerate(records[start:wait], start)
                  if g == group and k == 0x20 and data]
        commands = script_commands(records, codec, start, wait)
        steps.append({"scene": scene, "group": group, "wait": wait, "kind": kind,
                      "bodies": bodies, "commands": commands})
        start = wait + 1
    return steps


def script_commands(records, codec, start, end):
    return [[index, data.decode(codec, errors="backslashreplace").strip()]
            for index, (_, _, kind, data) in enumerate(records[start:end], start)
            if kind == 0x30 and data]


def rebellions_scene_identities():
    source = RUNTIME_SCENES.read_text(encoding="utf-8")
    entries = re.findall(r'\("(a_\d{2}_\d{2})\.cst", "([0-9a-f]{64})"\)', source)
    by_name = {name.upper(): (0x02CD76AE, "gbk", digest) for name, digest in entries}
    require(len(by_name) == len(entries) and by_name.get("A_01_01") == SCENES["A_01_01"],
            "runtime Rebellions GBK identity table is missing or inconsistent")
    return by_name


def scene_flow_boundary(records, codec, supported_scenes):
    commands = [(record, cmd) for record, cmd in script_commands(records, codec, 0, len(records))
                if FLOW_COMMAND.search(cmd)]
    if len(commands) == 1 and (match := DIRECT_NEXT.fullmatch(commands[0][1])):
        destination = match[1]
        if destination in supported_scenes:
            return {"kind": "next", "record": commands[0][0], "scene": destination}
    if commands:
        return {"kind": "observe_flow", "record": commands[0][0], "commands": commands}
    return {"kind": "observe_end", "record": len(records)}


def rebellions_campaign(game, first_scene="A_01_01"):
    identities = rebellions_scene_identities()
    scenes = []
    visited = set()
    scene = first_scene
    while scene not in visited:
        visited.add(scene)
        identity = identities[scene]
        records = read_scene(game, scene, identity)
        boundary = scene_flow_boundary(records, identity[1], identities)
        points = []
        novel = False
        for point in scene_steps(records, scene, identity[1]):
            if point["wait"] >= boundary["record"]:
                break
            for _, command in point["commands"]:
                if command in {"novel on", "novel off"}:
                    novel = command == "novel on"
            require(point["group"] == len(points) and point["bodies"] and (novel or point["kind"] == 2),
                    f"{scene}: r{point['wait']} is not a supported body wait")
            bodies = [body["record"] for body in point["bodies"]
                      if not BODY_CONTROL_ONLY.fullmatch(records[body["record"]][3])]
            require(bodies, f"{scene}: r{point['wait']} has no displayable body source")
            points.append({
                "group": point["group"], "record": point["wait"], "kind": point["kind"],
                "window": "novel" if novel else "message",
                "bodies": bodies,
            })
        require(bool(points), f"{scene}: no verified records before flow boundary")
        scenes.append({"scene": scene, "sha256": identity[2], "waits": points, "boundary": boundary})
        if boundary["kind"] != "next":
            return {"schema": "cs2-rebellions-story-plan/v1", "scenes": scenes}
        scene = boundary["scene"]
    raise ValueError(f"Rebellions scenario revisits {scene}")


def adjacent_pages(steps, source_records=None):
    source_records = source_records or {}
    pages = []
    for index, point in enumerate(steps):
        records = source_records.get(index, (point["wait"] - 1,))
        bodies = point["bodies"]
        require(point["group"] == index and point["kind"] == 2
                and tuple(body["record"] for body in bodies) == records,
                f"{point['scene']}:{point['wait']} has unexpected body records")
        body = bodies[1] if len(records) == 3 else bodies[0]
        page = dict(scene=point["scene"], group=index, wait=point["wait"],
                    body=body["record"], ordinal=body["ordinal"], body_bytes=body["bytes"],
                    sha256=body["sha256"])
        if len(records) == 3:
            page["prefix"] = bodies[0]
        pages.append(page)
    return pages


def kanmi_config(game, mode):
    limit, frames = KANMI_MODES[mode]
    points = adjacent_pages(scene_steps(read_scene(game, "sc01"), "sc01")[:limit])
    sc02_count = KANMI_SC02_PAGES.get(mode, 0)
    if sc02_count:
        records = read_scene(game, "sc02")
        steps = scene_steps(records, "sc02")[:sc02_count]
        if sc02_count > 101:
            require(records[561][3] == br"\n", "unexpected control record at sc02 r562")
        points.extend(adjacent_pages(steps, KANMI_SC02_SEPARATED_BODY))
    require(len(points) == limit + sc02_count, "unexpected Kanmi route length")
    config = json.loads((HERE / "pc_story_09_title_to_r102_guarded.json").read_text(encoding="utf-8"))
    scenario = config["9"]
    route = scenario["route"]
    scenario["frames"] = frames
    if sc02_count or mode == "r983":
        scenario["diagnostics"].extend(f"trace-native:{slot}" for slot in (66, 67, 196, 197, 200, 527, 528))
    route["label"] = f"09 New Game to {points[-1]['scene']} r{points[-1]['wait']}, guarded adjacent waits"
    if mode == "sc02_r147_save":
        route["label"] += ", ordinary slot 0"
    if sc02_count > 101:
        route["label"] = f"09 New Game to sc02 r{points[-1]['wait']}, with composed body observation"
    route["kanmi_exact"]["contract"] = f"kanmi-title-{mode.replace('_', '-')}/v1"
    template = route["steps"][-2]
    stop = route["steps"].pop()
    for current, following in zip(points[18:], points[19:]):
        wait = page_condition(current)
        after = page_condition(following)
        route["steps"].append(dict(copy.deepcopy(template), wait=wait, after=after,
                                   label=f"{wait['scene']} r{wait['record']}: one guarded Enter"))
    wait = page_condition(points[-1])
    route["steps"].append(dict(stop, wait=wait, label=f"Stop at {wait['scene']} g{wait['group']}/r{wait['record']}"))
    config["checkpoints"] = points
    return config


def page_condition(point):
    return dict(state="page", scene=point["scene"], group=point["group"], record=point["wait"], window="meswnd")


def kanmi_load_config(terminal=149, through_op=False):
    template = json.loads((HERE / "pc_story_09_title_to_r102_guarded.json").read_text(encoding="utf-8"))["9"]
    _, contract, frames = KANMI_LOAD_MODES[terminal]
    require(not through_op or terminal == 1379, "Kanmi opening requires the final sc02 page")
    label = f"09 cold Load slot 0 and advance from sc02 r147 to r{terminal}"
    if through_op:
        label += ", observe the branch and natural OP through sc03 r7"
        contract = "kanmi-load-sc02-post-op/v1"
    route = {"label": label, "idle_frames": 16000 if through_op else 4000, "page_scenes": [], "steps": [],
             "kanmi_exact": dict(template["route"]["kanmi_exact"], contract=contract)}
    return {"9": {"frames": frames, "diagnostics": ["trace-native:71", "trace-native:327", "trace-native:630"],
                  "route": route}}


def rebellions_config(mode):
    _, frames = REBELLIONS_MODES[mode]
    config = json.loads((HERE / "pc_story_12_20_opening_segments.json").read_text(encoding="utf-8"))["20"]
    config["frames"] = frames
    if mode != "r61":
        config["diagnostics"].extend(("trace-native:527", "trace-native:528"))
    config["route"].update(label=f"Rebellions: guarded A_01_01 through {mode}",
                           rebellions_r15=f"observe_{mode}")
    return {"20": config}


def rebellions_ordinary_config(mode, campaign=None):
    limits = {"save_r61": 14000, "load_r61": 16000, "resume_r61_third": 230000,
              "resume_r61_campaign": 3000000}
    scenario = rebellions_config("r189")["20"]
    scenario["frames"] = limits[mode]
    scenario["diagnostics"].append("trace-native:247")
    require((mode == "resume_r61_campaign") == (campaign is not None),
            "Rebellions campaign replay needs a generated CST plan")
    scenario["route"].update(label=f"Rebellions ordinary slot 0: {mode}",
                             rebellions_r15=mode if campaign is None else campaign)
    return {"20": scenario}


def write_json(path, data):
    path.write_text(json.dumps(data, ensure_ascii=True, indent=2) + "\n", encoding="utf-8")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--run09", type=Path, required=True)
    parser.add_argument("--run20", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True, help="existing archive directory")
    args = parser.parse_args()
    games = {index: Path(json.loads((run / f"{index}.json").read_text(encoding="utf-8"))["game"])
             for index, run in (("09", args.run09), ("20", args.run20))}
    audit = {}
    for scene, (_, codec, digest) in SCENES.items():
        records = read_scene(games["09" if scene.startswith("sc") else "20"], scene)
        steps = scene_steps(records, scene)
        audit[scene] = {"sha256": digest, "codec": codec, "records": len(records), "waits": steps,
                        "flow_boundary": scene_flow_boundary(records, codec, SCENES),
                        "after_final_wait": script_commands(records, codec, steps[-1]["wait"] + 1, len(records))}
    for mode in KANMI_MODES:
        write_json(args.output / f"20261121_pc_story_09_{mode}.json", kanmi_config(games["09"], mode))
    for terminal in KANMI_LOAD_MODES:
        record = 147 if terminal == 149 else terminal
        write_json(args.output / f"20261121_pc_story_09_sc02_r{record}_load.json", kanmi_load_config(terminal))
    write_json(args.output / "20261121_pc_story_09_sc03_r7_load.json", kanmi_load_config(1379, through_op=True))
    for mode in REBELLIONS_MODES:
        limit, _ = REBELLIONS_MODES[mode]
        require(len(adjacent_pages(audit["A_01_01"]["waits"][:limit])) == limit, "short Rebellions route")
        write_json(args.output / f"20261121_pc_story_20_{mode}.json", rebellions_config(mode))
    for mode in ("save_r61", "load_r61", "resume_r61_third"):
        write_json(args.output / f"20261121_pc_story_20_{mode}.json", rebellions_ordinary_config(mode))
    write_json(args.output / "20261121_pc_story_09_20_scene_audit.json", audit)
    campaign = rebellions_campaign(games["20"])
    write_json(args.output / "20261121_pc_story_20_a_line_plan.json", campaign)
    write_json(args.output / "20261121_pc_story_20_resume_r61_campaign.json",
               rebellions_ordinary_config("resume_r61_campaign", campaign))


if __name__ == "__main__":
    main()

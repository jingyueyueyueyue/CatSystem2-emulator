"""Stage two isolated NEKOPARA cold-boot profiles; never launch the player."""

import argparse
from datetime import datetime
import hashlib
import json
from pathlib import Path
import stat
import tempfile
import xml.etree.ElementTree as ET

from run_playthrough_batch import INVENTORY, ROOT, checked_path, file_sha256, identify_player, is_sha256, write_json


CACHE = Path("F:/CS2-build-cache/playthrough")
STARTUP_CONTRACT = ROOT / "scripts/analysis/pc_story_neko_startup_segments_candidate.json"
EXTENSION_CONTRACT = ROOT / "scripts/analysis/pc_story_neko_scene01_to_kitchen_candidate.json"
EXTENSION_PROBES = {"to-200": (22, 22000), "to-213": (23, 24000), "to-218": (24, 25000),
                    "to-337": (25, 30000), "to-368": (26, 33000), "to-461": (31, 40000)}
STARTUP_PROBES = {"title": (None, 3600), "first-press": (1, 4500), "three-pages": (2, 6000),
                  "to-117": (6, 10000), "to-146": (11, 15000), **EXTENSION_PROBES}
OPENING_WAIT_RECORDS = (59, 87, 92, 98, 103, 112, 117, 122, 127, 136, 141, 146)
FLAGS = ["capture-every:0", "no-final-capture", "story-events", "story-trace",
         "trace-native:325", "trace-native:326", "trace-native:1037", "trace-native:1038"]
BODY_POINT = [625, 478]
BODY_HIT_REGION = 77
BODY_PRESS = {"candidates": [3, 77, 52, 57, 7], "region": 52, "key": 0, "group": 0, "response": 1}
SAVE_ENTRY_MODE = "save-entry"
SAVE_ENTRY_LABEL = "Open NEKO Save"
SAVE_PANEL_STOP = "Stop at NEKO Save panel without input"
SAVE_ENTRY_PRESS = {"candidates": [3, 45, 77, 52, 57, 7], "region": 45, "key": 0, "group": 0, "response": 0}
TITLE_POINT = [1155, 235]
TITLE_WAIT = {"state": "title", "hit": {"point": TITLE_POINT, "region": 14}}
OPENING = tuple({"state": "page", "scene": "01", "group": group, "record": record, "window": "meswnd"}
                for group, record in enumerate(OPENING_WAIT_RECORDS[:3]))


def read_extension_contract():
    contents = EXTENSION_CONTRACT.read_bytes()
    contract = json.loads(contents)
    if (contract["schema"], contract["scene"], contract["contract_only"]) != ("cs2-neko-route-data/v1", "01", True):
        raise ValueError("invalid NEKO scene 01 extension contract")
    fields = contract["page_fields"]
    if len(fields) != len(set(fields)) or any(len(row) != len(fields) for row in contract["pages"]):
        raise ValueError("invalid NEKO scene 01 page fields")
    return contract, tuple(dict(zip(fields, row)) for row in contract["pages"]), hashlib.sha256(contents).hexdigest()


def extend_startup_positions(positions, mode):
    contract, pages, digest = read_extension_contract()
    suffix = tuple((page["group"], page["wait_record"]) for page in pages)
    join_group, join_record = positions[-1]
    pages_added = contract["extension"]["added_waits"]
    if (len(suffix) != pages_added + 1 or not suffix or suffix[0] != (join_group, join_record)
            or any(type(group) is not int or type(record) is not int for group, record in suffix)
            or any(right[0] != left[0] + 1 or right[1] <= left[1] for left, right in zip(suffix, suffix[1:]))
            or tuple(contract["extension"]["terminal"]) != suffix[-1]
            or contract["extension"]["terminal_input"] is not False):
        raise ValueError("NEKO extension must continue the exact opening join without gaps or terminal input")
    segments = contract["segments"]
    checkpoints = tuple(tuple(segment["to"]) for segment in segments)
    if (not checkpoints or checkpoints[-1] != suffix[-1]
            or tuple(tuple(segment["from"]) for segment in segments) != (suffix[0], *checkpoints[:-1])
            or any(checkpoint not in suffix for checkpoint in checkpoints)
            or any(segment["presses"] != segment["to"][0] - segment["from"][0] or segment["presses"] <= 0
                   for segment in segments)):
        raise ValueError("NEKO extension segments do not match the contiguous page sequence")
    presses, _ = EXTENSION_PROBES[mode]
    target = (presses, int(mode.removeprefix("to-")))
    if target not in checkpoints:
        raise ValueError(f"NEKO extension has no exact terminal for {mode}")
    segment = segments[checkpoints.index(target)]
    save_probe = contract["ordinary_save_probe"]
    terminal_points = [BODY_POINT]
    if target == tuple(save_probe["save_page"]):
        terminal_points.append(save_probe["save"]["button_probe_point"])
    extension_receipt = {
        "path": str(EXTENSION_CONTRACT), "sha256": digest,
        "required_runtime": "Exact scene-01 allowlist and live NEKO body guard through this terminal.",
        "required_1038_through_group": presses,
        "suggested_prior_probe": f"to-{segment['from'][1]}",
        "presses_from_prior": segment["presses"],
        "menu_input": False,
    }
    if target == tuple(save_probe["save_page"]):
        extension_receipt["save_button_probe"] = {
            "control": save_probe["save"]["control"],
            "point": save_probe["save"]["button_probe_point"],
            "input": False,
        }
    return positions + suffix[1:1 + presses - join_group], terminal_points, extension_receipt


def startup_probe_routes(mode):
    if mode == SAVE_ENTRY_MODE:
        frames, routes, digest, extension = startup_probe_routes("to-218")
        for language, route in routes.items():
            terminal = route["steps"].pop()
            panel = {**terminal["wait"], "state": "neko_save_panel", "window": "saveload"}
            point = extension["save_button_probe"]["point"]
            hit = {"point": point, "region": SAVE_ENTRY_PRESS["region"]}
            route["label"] = f"NEKO {language} r218 one-time Save entry"
            route["steps"].extend([
                {**terminal, "label": SAVE_ENTRY_LABEL, "wait": {**terminal["wait"], "hit": hit},
                 "click": point, "mouse_press": dict(SAVE_ENTRY_PRESS), "after": panel},
                {"label": SAVE_PANEL_STOP, "wait": panel, "probe_points": [[285, 184]]},
            ])
        extension["menu_input"] = True
        extension["save_button_probe"]["input"] = True
        extension["required_runtime"] = "Exact r218 body route and the one-time NEKO Save entry guard."
        return frames, routes, digest, extension

    contents = STARTUP_CONTRACT.read_bytes()
    contract = json.loads(contents)
    probe = contract["startup_title_probe"]
    presses, frames = STARTUP_PROBES[mode]
    positions = tuple((page["group"], page["wait_record"]) for page in contract["pages"])
    if positions != tuple(enumerate(OPENING_WAIT_RECORDS)):
        raise ValueError("NEKO startup contract has an unregistered scene 01 wait sequence")
    terminal_points, extension = [BODY_POINT], None
    if mode in EXTENSION_PROBES:
        positions, terminal_points, extension = extend_startup_positions(positions, mode)
    pages = tuple({"state": "page", "scene": "01", "group": group, "record": record, "window": "meswnd"}
                  for group, record in positions)
    title = {"state": "title", "scene": "-", "window": "title"}
    modal = {"state": "language_select", "scene": "langselect", "window": "langselect"}
    language_point = [784, 405]
    routes = {}
    for language in ("jp", "cn"):
        route = probe["routes"][language]
        confirm, terminal = route["steps"]
        expected_confirm = {"wait": {**modal, "hit": {"point": language_point, "region": 27}},
                            "click": language_point, "mouse_press": {"candidates": [3, 27, 28, 11, 7],
                                                                      "region": 27, "key": 0, "group": 0, "response": 0},
                            "neko_startup_language": language, "after": title}
        expected_terminal = {"wait": title, "probe_points": [TITLE_POINT]}
        if ({key: value for key, value in confirm.items() if key != "label"} != expected_confirm
                or {key: value for key, value in terminal.items() if key != "label"} != expected_terminal
                or set(route) != {"label", "idle_frames", "page_scenes", "page_fallback_before", "steps"}
                or route["page_scenes"] != [] or route["page_fallback_before"] != 0
                or route["idle_frames"] != 1800 or probe["frames"] != 3600):
            raise ValueError(f"invalid {language} cold-start title contract")
        if presses is not None:
            start = {"label": "Settled NEKO title: New Game only",
                     "wait": {**title, "hit": TITLE_WAIT["hit"]}, "click": TITLE_POINT,
                     "mouse_press": {"candidates": [3, 14, 19, 11, 7], "region": 14,
                                     "key": 0, "group": 0, "response": 0},
                     "after": pages[0], "neko_startup_language": language}
            turns = [
                {"label": f"Turn at 01/{before['group']}/{before['record']}",
                 "wait": {**before, "hit": {"point": BODY_POINT, "region": BODY_HIT_REGION}},
                 "click": BODY_POINT, "mouse_press": BODY_PRESS, "after": after, "probe_points": [BODY_POINT]}
                for before, after in zip(pages[:presses], pages[1:presses + 1])
            ]
            endpoint = pages[presses]
            terminal = {"label": f"Stop at 01/{endpoint['group']}/{endpoint['record']} without input",
                        "wait": endpoint, "probe_points": terminal_points}
            route = {**route, "label": f"NEKO {language} cold start to 01/{endpoint['group']}/{endpoint['record']}",
                     "steps": [confirm, start, *turns, terminal]}
        routes[language] = route
    return frames, routes, hashlib.sha256(contents).hexdigest(), extension


def seed_language_profile(profile, language):
    """Create a fresh test profile containing only the main language preference."""
    index = {"jp": 1, "cn": 2}[language]
    profile.mkdir()
    setup = profile / "setup.xml"
    document = ET.Element("document")
    selection = ET.SubElement(ET.SubElement(document, "language"), "id")
    ET.SubElement(selection, "UI").text = "0"
    ET.SubElement(selection, "main").text = str(index)
    with setup.open("xb") as stream:
        ET.ElementTree(document).write(stream, encoding="utf-8", xml_declaration=True)
    return setup


def batch_startup_scenario(language, mode="three-pages"):
    frames, routes, _, _ = startup_probe_routes(mode)
    return {"frames": frames, "route": routes[language], "neko_language": language}


def load_opening_route(path, expected_sha256):
    path = path.absolute()
    if not stat.S_ISREG(checked_path(path).st_mode):
        raise ValueError(f"expected a regular NEKO scenario file: {path}")
    contents = path.read_bytes()
    if hashlib.sha256(contents).hexdigest() != expected_sha256:
        raise ValueError("NEKO scenario does not match the requested SHA-256")
    scenario = json.loads(contents)["1"]
    if not isinstance(scenario, dict) or set(scenario) != {"frames", "route"}:
        raise ValueError("NEKO sample 1 must have only frames and route (no frame inputs or save)")
    frames, route = scenario["frames"], scenario["route"]
    if type(frames) is not int or frames <= 3 or not isinstance(route, dict):
        raise ValueError("NEKO route requires a positive frame budget and a route object")
    fallback = route.get("page_fallback_before", 0)
    if (not isinstance(route.get("label"), str) or not route["label"]
            or set(route) - {"label", "idle_frames", "page_scenes", "page_fallback_before", "steps"}
            or route.get("page_scenes") != [] or type(fallback) is not int or fallback != 0
            or type(route.get("idle_frames")) is not int or not 3 < route["idle_frames"] <= frames):
        raise ValueError("NEKO route requires bounded waits and no page fallback")
    steps = route.get("steps")
    wanted = [
        {"wait": TITLE_WAIT, "click": TITLE_POINT, "after": OPENING[0]},
        *({"wait": {**before, "hit": {"point": BODY_POINT, "region": BODY_HIT_REGION}},
           "click": BODY_POINT, "mouse_press": BODY_PRESS, "after": after}
          for before, after in zip(OPENING, OPENING[1:])),
        {"wait": OPENING[-1]},
    ]
    if (not isinstance(steps, list) or len(steps) != len(wanted)
            or any(not isinstance(step, dict) or not isinstance(step.get("label"), str)
                   or not step["label"]
                   or json.dumps({key: value for key, value in step.items() if key != "label"}, sort_keys=True)
                   != json.dumps(expected, sort_keys=True)
                   for step, expected in zip(steps, wanted))):
        raise ValueError("NEKO route must be title -> 01/0/59 -> 01/1/87 -> input-free 01/2/92 with verified presses")
    return path, frames, route


def stage(player, expected_sha256, route=None, route_sha256=None, startup_probe=None, *, fast_text=False):
    if startup_probe is not None and (route is not None or route_sha256 is not None):
        raise ValueError("--startup-probe cannot be combined with --route or --route-sha256")
    if (route is None) != (route_sha256 is None):
        raise ValueError("--route and --route-sha256 must be supplied together")
    expected_sha256 = expected_sha256.lower()
    if not is_sha256(expected_sha256) or (route_sha256 is not None and not is_sha256(route_sha256.lower())):
        raise ValueError("expected a 64-digit SHA-256 for each pinned file")
    player = player.absolute()
    archive_roots = (ROOT / "archive", CACHE.parent / "archive")
    if player.name != "story_replay.exe" or not any(player.is_relative_to(root) for root in archive_roots):
        raise ValueError("expected an archived integration story_replay.exe")
    identity = identify_player(player, require_integration=True)
    if identity["sha256"] != expected_sha256:
        raise ValueError("story_replay.exe does not match the requested SHA-256")
    opening = None if route is None else load_opening_route(route, route_sha256.lower())
    startup = None if startup_probe is None else startup_probe_routes(startup_probe)

    game = ROOT / json.loads(INVENTORY.read_text(encoding="utf-8"))["games"][1]["root"]
    if "NEKO WORKs" not in game.name or "vol.3" not in game.name:
        raise ValueError("inventory sample 01 is not NEKOPARA Vol.3")
    if not game.is_relative_to(ROOT / "games") or not stat.S_ISDIR(checked_path(game).st_mode):
        raise ValueError("NEKOPARA must be an existing directory in the workspace games root")
    if not stat.S_ISDIR(checked_path(CACHE).st_mode):
        raise ValueError(f"not a directory: {CACHE}")
    now = datetime.now().astimezone()
    month = CACHE / now.strftime("%Y-%m")
    month.mkdir(exist_ok=True)
    if not stat.S_ISDIR(checked_path(month).st_mode):
        raise ValueError(f"not a directory: {month}")
    output = Path(tempfile.mkdtemp(prefix=now.strftime("%Y%m%d_%H%M%S_%f_"), suffix="_neko_language_stage", dir=month))

    route_receipt = None
    if opening is not None:
        manifest, frames, route_body = opening
        staged_route = output / "01.route.json"
        write_json(staged_route, route_body)
        route_receipt = {
            "manifest": str(manifest), "manifest_sha256": route_sha256.lower(),
            "path": str(staged_route), "sha256": file_sha256(staged_route), "frames": frames,
            "terminal": OPENING[-1], "expected_body_presses": 2,
            "validation": "offline_route_shape_only",
        }

    runs = {}
    for language, index in (("jp", 1), ("cn", 2)):
        run = output / language
        profile = run / "01.profile"
        run.mkdir()
        setup = seed_language_profile(profile, language)
        if fast_text:
            from run_playthrough_batch import seed_fast_text_profile
            seed_fast_text_profile(profile)
        write_json(run / "build_identity.json", identity)
        runs[language] = {
            "run": str(run), "setup": str(setup), "setup_sha256": file_sha256(setup),
            "ui_index": 0, "main_index": index,
            "argv_prefix": [str(player), str(game), str(run / "01")],
            "stdout": str(run / "01.events.log"), "stderr": str(run / "01.host.log"),
        }
        if route_receipt is not None:
            runs[language]["argv"] = [*runs[language]["argv_prefix"], str(route_receipt["frames"]),
                                      "--route-json", route_receipt["path"]]
        elif startup is not None:
            frames, routes, _, _ = startup
            route_body = routes[language]
            staged_route = run / "01.route.json"
            write_json(staged_route, route_body)
            body_probe = "to-218" if startup_probe == SAVE_ENTRY_MODE else startup_probe
            runs[language]["route"] = {"path": str(staged_route), "sha256": file_sha256(staged_route),
                                       "frames": frames, "terminal": route_body["steps"][-1]["wait"],
                                       "expected_body_presses": STARTUP_PROBES[body_probe][0] or 0}
            runs[language]["argv"] = [*runs[language]["argv_prefix"], str(frames), "--route-json", str(staged_route)]

    receipt = {
        "schema": "cs2-neko-language-stage/v1", "state": "staged_not_run",
        "settings_origin": "synthetic_language_only_not_ui_persistence_evidence",
        "run_root": str(output), "player": identity, "runs": runs,
        "environment": {"CS2_MESSAGE_REVEAL_TRACE": "fes"},
    }
    if fast_text:
        receipt.update(settings_origin="synthetic_language_and_fast_text", message_delay=0)
    if startup is not None:
        receipt.update(schema="cs2-neko-language-stage/v3", mode="state", startup_probe=startup_probe,
                       contract_sha256=startup[2])
        if startup[3] is not None:
            receipt["extension_contract"] = startup[3]
    elif route_receipt is not None:
        receipt.update(schema="cs2-neko-language-stage/v2", mode="state", route=route_receipt)
    else:
        receipt["frame_flags"] = FLAGS
    write_json(output / "stage.json", receipt)
    return output


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--player", type=Path)
    parser.add_argument("--sha256", help="expected SHA-256 of the pinned integration player")
    modes = parser.add_mutually_exclusive_group()
    modes.add_argument("--route", type=Path, help="legacy sample 1 scenario with an explicit stop at 01/2/92")
    modes.add_argument("--startup-probe", choices=(*STARTUP_PROBES, SAVE_ENTRY_MODE),
                       help="stage guarded JP/CN cold boots; kitchen modes require the extended player allowlist")
    parser.add_argument("--print-plan", action="store_true",
                        help="print both typed startup routes without inspecting a player or creating files")
    parser.add_argument("--print-batch-scenario", choices=("jp", "cn"),
                        help="print the typed sample 01 scenario for the PC batch runner")
    parser.add_argument("--route-sha256", help="expected SHA-256 of the input scenario manifest")
    args = parser.parse_args()
    try:
        if args.print_batch_scenario:
            if args.print_plan or args.route or any(value is not None for value in
                                                    (args.player, args.sha256, args.route_sha256)):
                parser.error("--print-batch-scenario accepts only an optional --startup-probe")
            scenario = batch_startup_scenario(args.print_batch_scenario, args.startup_probe or "three-pages")
            print(json.dumps({"1": scenario}, ensure_ascii=True, indent=2))
            return
        if args.print_plan:
            if args.startup_probe is None or any(value is not None for value in
                                                 (args.player, args.sha256, args.route_sha256)):
                parser.error("--print-plan requires only --startup-probe")
            frames, routes, digest, extension = startup_probe_routes(args.startup_probe)
            print(json.dumps({"state": "plan_only", "startup_probe": args.startup_probe, "frames": frames,
                              "contract_sha256": digest, "extension_contract": extension,
                              "routes": routes}, ensure_ascii=True, indent=2))
            return
        if args.player is None or args.sha256 is None:
            parser.error("--player and --sha256 are required to stage profiles")
        output = stage(args.player, args.sha256, args.route, args.route_sha256, args.startup_probe)
    except (OSError, ValueError, KeyError, TypeError) as error:
        parser.error(str(error))
    print(output / "stage.json")


if __name__ == "__main__":
    main()

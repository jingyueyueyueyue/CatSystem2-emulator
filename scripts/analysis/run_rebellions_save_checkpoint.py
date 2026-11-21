"""Cold-load a Rebellions ordinary slot written by the same integration build."""

import argparse
from datetime import datetime
from collections import defaultdict
import json
from pathlib import Path
import re
import shutil
import subprocess
from tempfile import mkdtemp
import time

from check_pc_body_successor import (
    ENTER, REBELLIONS_BODY_BINDING, REBELLIONS_RELEASE, REBELLIONS_SOURCE, ROUTE_PAGE, read_host,
)
from check_pc_story_11_13_sources import require
from check_pc_story_11_13_successor import read_run
from check_pc_story_20_novel import BODY_PROOF, NOVEL_LAYER, WAIT_PACKETS, NATIVE_WAIT
from plan_pc_story_09_20_continuation import REBELLIONS_NOVEL_FES, rebellions_campaign, rebellions_ordinary_config
from run_playthrough_batch import (
    GIB, ROOT, ReplayProgress, available_ram_bytes, file_sha256, wait_for_replay, write_json,
)


WRITTEN = re.compile(r"frame=(\d+) rebellions-ordinary-written group=12 record=61 slot=0 "
                     r"write_frame=(\d+) sha256=([0-9a-f]{64})")
RESTORED = re.compile(r"frame=(\d+) rebellions-ordinary-restored group=12 record=61 "
                      r"open_frame=(\d+) sha256=([0-9a-f]{64})")
SCENE_LOAD = re.compile(r"frame=(\d+) rebellions-scene-load handle=(\d+) load_id=(\d+) "
                        r"scene=(\w+) sha256=([0-9a-f]{64})")
SOURCE = re.compile(REBELLIONS_SOURCE.pattern.replace("Second|Third", "Second|Third|Later"))


def read_producer(run):
    identity, _, events = read_run(run, "20")
    meta = json.loads((run / "20.json").read_text(encoding="utf-8"))
    profile = Path(meta["profile"]).resolve()
    require(profile.is_relative_to(run.resolve()), "producer profile is external")
    route = Path(meta["route_json"]).resolve()
    require(route.is_relative_to(run.resolve())
            and json.loads(route.read_text(encoding="utf-8")) == rebellions_ordinary_config("save_r61")["20"]["route"],
            "producer used a different route")
    slot = profile / "save0000.dat"
    writes = [match for line in events if (match := WRITTEN.fullmatch(line))]
    require(len(writes) == 1 and events.count("replay_status=completed") == 1, "ordinary slot write was not verified")
    saved, write, digest = writes[0].groups()
    pages = [match for line in events if (match := ROUTE_PAGE.fullmatch(line))]
    require(len(pages) == 13 and pages[-1].groups()[1:] == ("A_01_01", "12", "61")
            and int(pages[-1][1]) < int(write) <= int(saved)
            and sum(line.startswith(f"frame={write} fes-datasave slot=0 container=")
                    and line.endswith(" written=true") for line in events) == 1,
            "ordinary slot was not written at r61 by the game")
    require(slot.is_file() and file_sha256(slot) == digest, "producer slot hash changed")
    return identity, Path(meta["game"]), slot, digest


def verify_campaign(points, host, lines, pages, receipts, presses, inspection, status):
    expected = [(scene, point) for scene in points["scenes"] for point in scene["waits"]][12:]
    require(len(pages) <= len(expected), "campaign replay exceeded its source plan")
    frames = [int(page[1]) for page in pages]
    require(frames == sorted(set(frames)), "duplicate campaign pages")
    binding = {int(m[1]): tuple(map(int, m.groups()[1:])) for line in host
               if (m := REBELLIONS_BODY_BINDING.fullmatch(line))}
    proof = {int(m[1]): m for line in host if (m := BODY_PROOF.fullmatch(line))}
    releases = [int(m[1]) for line in host if (m := REBELLIONS_RELEASE.fullmatch(line))]
    root = set()
    sourced = defaultdict(list)
    loads = {}
    waits = defaultdict(list)
    for line in lines:
        if (m := NATIVE_WAIT.fullmatch(line)):
            waits[int.from_bytes(bytes.fromhex(m[2][:8]), "little")].append((int(m[1]), m[2][8:]))
        if not line.startswith("frame="):
            continue
        prefix, _, detail = line.partition(" ")
        if (m := SCENE_LOAD.fullmatch(line)):
            loads[int(m[2]), int(m[3])] = (int(m[1]), m[4], m[5])
        elif (m := SOURCE.fullmatch(detail)):
            sourced[tuple(map(int, m.groups()[:4]))].append(int(m[5]))
        elif detail == "input-wait native 71 address=0xad0":
            root.add(int(prefix.removeprefix("frame=")))
    consuming = defaultdict(list)
    for receipt in receipts:
        if receipt["pressed"] and receipt["response"] == 0 and receipt["frame"] > frames[0]:
            consuming[receipt["frame"]].append(receipt)
    require(set(binding) == set(proof) == set(frames) and releases == frames[1:], "missing campaign page proof")
    observations = []
    for page, (scene, point) in zip(pages, expected):
        frame, group, record = int(page[1]), int(page[3]), int(page[4])
        owner, region, receiver = binding[frame]
        p = proof[frame]
        handle, load, epoch, source_at, kind = map(int, p.groups()[1:6])
        member, marked, complete = p[7], int(p[8]), int(p[9])
        layers = inspection.get(frame, {}).get(owner, {})
        prompt = "keywait2" if point["window"] == "novel" and kind == 2 else "keywait"
        require((page[2], group, record, kind) == (scene["scene"], point["group"], point["record"], point["kind"])
                and source_at <= complete <= frame and marked < frame and frame in root
                and (member in REBELLIONS_NOVEL_FES if point["window"] == "novel" else member == "meswnd01.fes")
                and layers.get(prompt, 0) > 0
                and (point["window"] == "novel" or layers.get(r"\0text:737472[0]", 0) > 0)
                and all(any(at <= source_at for at in sourced[handle, load, group, body]) for body in point["bodies"])
                and any(marked <= at <= frame and packet in WAIT_PACKETS[kind] for at, packet in waits[owner]),
                f"{page[2]} r{record}: wrong committed page, GBK source or native wait")
        if observations:
            prev = observations[-1]
            require(source_at > prev["frame"] and (receiver, region) == (prev["receiver"], prev["region"]),
                    f"{page[2]} r{record}: stale source or changed input target")
            if scene["scene"] != prev["scene"]:
                loaded, name, digest = loads[handle, load]
                require(prev["load"] < load and prev["frame"] < loaded <= marked
                        and (name, digest) == (scene["scene"], scene["sha256"]), f"{name}: scene load does not match the plan")
            else:
                require((handle, load) == (prev["handle"], prev["load"]), "campaign scene identity changed within a scene")
        observations.append(dict(frame=frame, scene=scene["scene"], group=group, record=record, handle=handle,
                                 load=load, owner=owner, receiver=receiver, region=region, epoch=epoch, window=member))
    enter_frames = [frame for frame, button in presses[2:] if button == "Keyboard(13)"]
    require(len(enter_frames) == len(presses[2:]) and enter_frames in (frames[:-1], frames), "extra campaign inputs")
    for page in observations[:len(enter_frames)]:
        matches = consuming[page["frame"] + 1]
        require(len(matches) == 1 and all(matches[0][key] == value for key, value in
                dict(key=2, group=0, region=page["region"], receiver=page["receiver"],
                     physical=ENTER, repeated=False, pressed=1).items())
                and page["frame"] + 1 in root, "campaign Enter consumption did not match the prior page")
    require(sum(map(len, consuming.values())) == len(enter_frames), "extra input consumers in campaign")
    return {"status": "campaign_boundary_verified" if status == "unrecognized_wait" and len(pages) == len(expected)
            else "campaign_prefix_verified", "pages": len(pages), "body_enters": len(enter_frames),
            "replay_status": status, "planned_pages": len(expected), "terminal": observations[-1]}


def verify_consumer(run, identity, digest, mode, campaign=None):
    if campaign is not None:
        return verify_campaign_consumer(run, identity, digest, campaign)
    actual, host, lines = read_run(run, "20")
    require(actual == identity and lines.count("replay_status=completed") == 1, "cold replay did not finish on the same build")
    restored = [match for line in lines if (match := RESTORED.fullmatch(line))]
    require(len(restored) == 1 and restored[0][3] == digest
            and not any(" fes-datasave " in line for line in lines), "cold replay changed or failed to restore the slot")
    loaded, opened = map(int, restored[0].groups()[:2])
    pages = [match for line in lines if (match := ROUTE_PAGE.fullmatch(line))]
    require(len(pages) >= 2 and pages[0].groups() == (str(loaded), "A_01_01", "12", "61")
            and pages[1].groups()[1:] == ("A_01_01", "13", "150"), "cold replay did not restore r61 and advance to r150")
    _, receipts, presses = read_host(host)
    require(len(presses) >= 3 and [button for _, button in presses[:2]] == ["MouseButton(0)", "MouseButton(0)"]
            and presses[0][0] < presses[1][0] < opened <= loaded
            and presses[2] == (loaded, "Keyboard(13)")
            and [(r["key"], r["group"], r["physical"]) for r in receipts
                 if r["frame"] == loaded + 1 and r["pressed"] and r["response"] == 0] == [(2, 0, ENTER)],
            "restore or Enter receipt mismatch")
    terminal = ("A_01_01", "13", "150") if mode == "load_r61" else ("A_01_03", "2", "97")
    require(pages[-1].groups()[1:] == terminal, "cold replay reached a different terminal page")
    return {"status": "roundtrip_pass", "slot": 0, "sha256": digest,
            "restored_frame": loaded, "advanced_frame": int(pages[1][1]), "pages": len(pages),
            "terminal": dict(scene=terminal[0], group=int(terminal[1]), record=int(terminal[2])),
            "build": identity}


def verify_campaign_consumer(run, identity, digest, campaign):
    meta = json.loads((run / "20.json").read_text(encoding="utf-8"))
    actual = json.loads((run / "build_identity.json").read_text(encoding="utf-8"))
    status = meta["replay_status"]
    require(actual == identity and meta["command"][0] == identity["player"] and meta["exit_code"] == 0
            and not meta["first_engine_error"] and not meta["timed_out"], "campaign launch or engine failure")
    require(json.loads((run / "20.route.json").read_text(encoding="utf-8"))
            == rebellions_ordinary_config("resume_r61_campaign", campaign)["20"]["route"], "campaign route changed")
    host, lines = [(run / f"20.{extension}.log").read_text(encoding="utf-8").splitlines()
                   for extension in ("host", "events")]
    restored = [m for line in lines if (m := RESTORED.fullmatch(line))]
    require(len(restored) == 1 and restored[0][3] == digest and not any(" fes-datasave " in line for line in lines)
            and lines.count(f"replay_status={status}") == 1, "campaign did not restore the ordinary slot")
    loaded, opened = map(int, restored[0].groups()[:2])
    pages = [m for line in lines if (m := ROUTE_PAGE.fullmatch(line))]
    require(len(pages) >= 2 and pages[0].groups() == (str(loaded), "A_01_01", "12", "61"), "wrong first restored page")
    states, receipts, presses = read_host(host, NOVEL_LAYER)
    require(len(presses) >= 3 and [button for _, button in presses[:2]] == ["MouseButton(0)", "MouseButton(0)"]
            and presses[0][0] < presses[1][0] < opened <= loaded and presses[2] == (loaded, "Keyboard(13)"),
            "campaign restore input receipt mismatch")
    inspection = {frame: layers for frame, _, layers in states}
    return {**verify_campaign(campaign, host, lines, pages, receipts, presses, inspection, status),
            "slot": 0, "sha256": digest, "restored_frame": loaded, "build": identity}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("save_run", type=Path)
    parser.add_argument("output_parent", type=Path)
    parser.add_argument("--through", choices=("load_r61", "resume_r61_third", "resume_r61_campaign"), default="load_r61")
    args = parser.parse_args()
    identity, game, slot, digest = read_producer(args.save_run)
    deadline = time.monotonic() + 240
    while available_ram_bytes() < 4 * GIB:
        require(time.monotonic() < deadline, "cold replay memory budget unavailable")
        time.sleep(10)
    require(shutil.disk_usage(args.output_parent).free >= 22 * GIB, "cold replay disk budget unavailable")

    out = Path(mkdtemp(prefix=datetime.now().strftime("%Y%m%d_%H%M%S_") + "rebellions_cold_load_",
                       dir=args.output_parent.resolve()))
    print(out, flush=True)
    prefix = out / "20"
    profile, route, events, host = (prefix.with_suffix(suffix)
                                    for suffix in (".profile", ".route.json", ".events.log", ".host.log"))
    profile.mkdir()
    staged = profile / slot.name
    shutil.copyfile(slot, staged)
    require(list(profile.iterdir()) == [staged] and file_sha256(staged) == digest, "isolated slot staging failed")
    campaign = rebellions_campaign(game) if args.through == "resume_r61_campaign" else None
    config = rebellions_ordinary_config(args.through, campaign)["20"]
    write_json(route, config["route"])
    write_json(out / "build_identity.json", identity)
    command = [identity["player"], str(game), str(prefix), str(config["frames"]), "--route-json", str(route),
               *config["diagnostics"]]
    progress = ReplayProgress(state_mode=True)
    timeout = {"load_r61": 900, "resume_r61_third": 5400, "resume_r61_campaign": 43200}[args.through]
    with events.open("xb") as stdout, host.open("xb") as stderr, events.open("rb") as reader:
        with subprocess.Popen(command, cwd=ROOT, stdout=stdout, stderr=stderr) as process:
            timed_out = wait_for_replay(process, reader, progress, time.monotonic() + timeout, 1)
            exit_code = process.returncode
    write_json(out / "20.json", {
        "index": 20, "game": str(game), "command": command, "exit_code": exit_code, "timed_out": timed_out,
        "captures": [p.name for p in out.glob("20.*.png")], "route_json": str(route), "profile": str(profile),
        "stdout": str(events), "stderr": str(host), "replay_status": progress.replay_status,
        "first_engine_error": progress.first_engine_error, "last_story_position": progress.last_story_position,
    })
    require(file_sha256(staged) == digest, "cold replay changed the ordinary slot")
    check = verify_consumer(out, identity, digest, args.through, campaign)
    write_json(out / "roundtrip.json", {**check, "producer": str(args.save_run.resolve()), "consumer": str(out)})
    print(json.dumps(check, ensure_ascii=True), flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

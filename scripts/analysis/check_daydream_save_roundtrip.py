"""Stage and verify a sample 17 ordinary PC save roundtrip."""

import argparse
import json
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import time
import zlib

from check_daydream_prologue import DOWN, ENTER, MOUSE_LEFT, RECEIPT
from generate_daydream_case03_end1 import build_scenario, rendered
from generate_daydream_prologue_exit import GAME, require, cst_rows, members
from run_playthrough_batch import (
    ROOT, ReplayProgress, checked_path, file_sha256, identify_player,
    seed_fast_text_profile, wait_for_replay, write_json,
)


SLOT = "save0000.dat"
SYSTEM = "savegen.dat"
WRITE_LABEL = "Daydream ordinary save written at r121"
LOAD_LABEL = "Daydream ordinary load successor r123"
SHA = r"[0-9a-f]{64}"
CONTROL = re.compile(r"frame=(\d+) daydream-ordinary-control name=(\w+) owner=(\d+) region=(\d+) x=(-?\d+) y=(-?\d+)")
PRESS = re.compile(r"frame (\d+): Daydream ordinary (\w+) (InputRouteReceipt \{.*\})")
TOOLBAR = re.compile(r"frame (\d+): Daydream r121 Save button owner=(\d+) region=(\d+) point=\[(-?\d+), (-?\d+)\] .*")
TOOLBAR_RECEIPT = re.compile(r"frame (\d+): Daydream END1 choice press (InputRouteReceipt \{.*\})")
EMPTY = re.compile(r"frame (\d+): Daydream ordinary Save panel owner=(\d+) empty_slot=0 region=(\d+)")
WRITE = re.compile(r"frame=(\d+) fes-datasave slot=0 container=([1-9]\d*) written=true")
WRITTEN = re.compile(rf"frame=(\d+) daydream-ordinary-written scene=c01_01_01 group=13 record=121 slot=0 write_frame=(\d+) sha256=({SHA})")
OPEN = re.compile(rf'frame=(\d+) daydream-slot-open native=247 slot=0 file="save0000.dat" handle=([1-9]\d*) sha256=({SHA})')
RESTORED = re.compile(r"frame=(\d+) daydream-ordinary-restored scene=c01_01_01 group=13 record=121 source=120 "
                      r"context=(\d+) load=(\d+) owner=(\d+) epoch=(\d+) published=(\d+) region=(\d+) receiver=(\d+)")
SUCCESSOR = re.compile(r"frame=(\d+) daydream-ordinary-successor scene=c01_01_01 group=14 record=123 source=122 "
                       r"context=(\d+) load=(\d+) owner=(\d+) epoch=(\d+) published=(\d+)")


def read_json(path):
    return json.loads(path.read_text(encoding="utf-8"))


def rows(path):
    with path.open(encoding="utf-8", errors="replace") as stream:
        for line in stream:
            yield line.rstrip("\r\n")


def css_identity(path):
    info = checked_path(path)
    require(stat.S_ISREG(info.st_mode) and info.st_nlink == 1 and 16 < info.st_size <= 64 * 1024 * 1024,
            "CSS must be one regular unlinked file")
    with path.open("rb") as stream:
        header = stream.read(16)
    require(header[:4] == b"CSS\0" and int.from_bytes(header[4:8], "little") + 16 == info.st_size,
            "CSS header length and file size differ")
    return {"size": info.st_size, "sha256": file_sha256(path)}


def write_scenario():
    for archive in ("update02.int", "update03.int"):
        source = members(GAME, archive, {"c01_01_01.cst"})["c01_01_01.cst"][1]
        shape = [(g, r, k, bool(payload)) for g, r, k, payload in cst_rows(source) if 120 <= r <= 123]
        require(shape == [(13, 120, 32, True), (13, 121, 2, False), (14, 122, 32, True), (14, 123, 2, False)],
                f"{archive}: saved page and successor controls changed")
    scenario = build_scenario()
    route = scenario["17"]["route"]
    route["label"] = "Daydream: write ordinary Save slot 0 at CASE 1 r121"
    route["daydream_case03_end1"].update(save_entry_at_r121=True, write_slot_at_r121=True)
    scenario["17"]["review"].update(
        player_patch="archive/2026-11/20261121_daydream_case01_ordinary_roundtrip_v2.patch",
        acceptance="Verify empty ordinary slot 0, one physical click and one successful FES datasave.",
    )
    return scenario


def only(matches, label):
    require(len(matches) == 1, f"expected one {label}, found {len(matches)}")
    return matches[0]


def scan(path, patterns):
    found = {key: [] for key in patterns}
    for line in rows(path):
        for key, pattern in patterns.items():
            if match := pattern.fullmatch(line):
                found[key].append(match)
    return found


def checkpoint(label):
    return re.compile(rf'replay frame=(\d+) checkpoint="{re.escape(label)}"')


def verify_completed(path):
    statuses = []
    for line in rows(path):
        require(re.match(r"frame=\d+ (?:task|vm)-error ", line) is None, "engine failure in replay")
        if line.startswith("replay_status="):
            statuses.append(line)
    require(statuses == ["replay_status=completed"], "replay did not complete")


def verify_press(start, owner, region, frame, raw, keyboard=False):
    match = RECEIPT.fullmatch(raw)
    require(match is not None, "missing structured physical receipt")
    expected = ("2", "0", "1", "1", "false") if keyboard else ("0", "0", "2", "1", "false")
    require(int(match[1]) == frame and match.groups()[1:6] == expected
            and tuple(map(int, match.groups()[6:8])) == (region, owner)
            and (frame == start + 1 and match[9] == "0" and match[10] == ENTER if keyboard
                 else start < frame <= start + 3 and match[9] in ("0", "1") and match[10] == MOUSE_LEFT),
            "physical input does not match the live guarded control")


def verify_writer(batch):
    result = read_json(batch)
    require((result["index"], result["mode"], result["status"], result["replay_status"],
             result["exit_code"], result["timed_out"], result["first_engine_error"])
            == (17, "state", "checkpoint_reached", "completed", 0, False, None), "writer did not complete")
    prefix = batch.parent / "17/17"
    profile = prefix.with_suffix(".profile")
    events_path, host_path = (prefix.with_suffix(ext) for ext in (".events.log", ".host.log"))
    command = result["command"]
    identity = read_json(batch.parent / "build_identity.json")
    player = Path(identity["player"])
    require(Path(command[0]) == player and Path(command[1]) == GAME and Path(command[2]) == prefix
            and Path(result["profile"]) == profile and Path(result["stdout"]) == events_path
            and Path(result["stderr"]) == host_path and file_sha256(player) == identity["sha256"],
            "writer inputs or executable identity changed")
    route = read_json(prefix.with_suffix(".route.json"))
    require(route["daydream_case03_end1"]["write_slot_at_r121"] is True,
            "writer did not use the ordinary slot route")
    ordinary_files = [p.name for p in profile.iterdir() if re.fullmatch(r"save\d+\.dat", p.name, re.I)]
    require(ordinary_files == [SLOT], "expected only ordinary slot 0 in writer profile")
    slot, system = (css_identity(profile / name) for name in (SLOT, SYSTEM))
    verify_completed(events_path)
    events = scan(events_path, {"control": CONTROL, "write": WRITE, "written": WRITTEN,
                               "checkpoint": checkpoint(WRITE_LABEL), "all_writes": re.compile(r"frame=\d+ fes-datasave .*")})
    start, name, panel, region, _, _ = only(events["control"], "slot control").groups()
    write, _ = only(events["write"], "successful slot 0 write").groups()
    terminal, write_bound, digest = only(events["written"], "slot file receipt").groups()
    start, panel, region, write, terminal = map(int, (start, panel, region, write, terminal))
    require(name == "write" and len(events["all_writes"]) == 1 and write == int(write_bound)
            and terminal == result["last_frame"] == int(only(events["checkpoint"], "writer checkpoint")[1])
            and digest == slot["sha256"], "writer terminal or slot digest mismatch")
    host = scan(host_path, {"toolbar": TOOLBAR, "toolbar_press": TOOLBAR_RECEIPT, "empty": EMPTY, "press": PRESS, "down": DOWN})
    toolbar, toolbar_owner, toolbar_region, _, _ = map(int, only(host["toolbar"], "r121 Save toolbar").groups())
    prior = [receipt for receipt in host["toolbar_press"] if int(receipt[1]) > toolbar]
    before, raw = only(prior, "r121 toolbar physical receipt").groups()
    verify_press(toolbar, toolbar_owner, toolbar_region, int(before), raw)
    require(tuple(map(int, only(host["empty"], "empty slot guard").groups())) == (start, panel, region),
            "write did not originate in the empty slot guard")
    frame, kind, raw = only(host["press"], "slot physical receipt").groups()
    require(kind == "write" and int(before) <= start < int(frame) <= write <= terminal
            and [(int(m[1]), m[2]) for m in host["down"] if int(m[1]) >= toolbar]
            == [(toolbar, "MouseButton(0)"), (start, "MouseButton(0)")], "unexpected input during Save")
    verify_press(start, panel, region, int(frame), raw)
    return {"profile": str(profile), "slot": slot, "system": system, "player": identity, "write_frame": write}


def stage(batch, folder, resume_player=None, *, fast_text=False):
    require(not fast_text or resume_player is not None, "fast text requires route continuation")
    writer = verify_writer(batch)
    reader = (writer["player"] if resume_player is None else
              identify_player(resume_player.resolve(strict=True), require_integration=True))
    plan = {"paths": [], "pages": {}, "ordinary_load_sha256": writer["slot"]["sha256"]}
    if resume_player is not None:
        plan = build_scenario()["17"]["route"]["daydream_case03_end1"] | {
            "ordinary_load_sha256": writer["slot"]["sha256"], "resume_after_load": True,
        }
    checked_path(folder.parent)
    folder.mkdir()
    prefix = folder / "17-load"
    profile = prefix.with_suffix(".profile")
    profile.mkdir()
    for name, key in ((SLOT, "slot"), (SYSTEM, "system")):
        dest = profile / name
        with (Path(writer["profile"]) / name).open("rb") as original, dest.open("xb") as copy:
            shutil.copyfileobj(original, copy)
        require(css_identity(dest) == writer[key], "private profile copy changed")
    text_setup = {"fast_text": seed_fast_text_profile(profile)} if fast_text else {}
    route = prefix.with_suffix(".route.json")
    label = "Daydream: continue CASE 1 from ordinary slot 0" if resume_player is not None else "Daydream: ordinary slot 0 cold load to r123"
    frames = "5000000" if resume_player is not None else "27500"
    write_json(route, {"label": label, "idle_frames": 6000,
                       "page_scenes": [], "steps": [], "daydream_case03_end1": plan})
    manifest = folder / "stage.json"
    write_json(manifest, {"schema": "cs2-daydream-ordinary-stage/v1", "writer_batch": str(batch),
                          "writer": writer, "route_sha256": file_sha256(route),
                          "reader": reader, "resume": resume_player is not None,
                          **text_setup,
                          "argv": [reader["player"], str(GAME), str(prefix), frames, "--route-json", str(route)]})
    return manifest


def read_stage(manifest):
    record = read_json(manifest)
    prefix = manifest.parent / "17-load"
    route = prefix.with_suffix(".route.json")
    writer = verify_writer(Path(record["writer_batch"]))
    resume = record.get("resume", False)
    reader = record.get("reader", writer["player"])
    frames = "5000000" if resume else "27500"
    argv = [reader["player"], str(GAME), str(prefix), frames, "--route-json", str(route)]
    require(record["schema"] == "cs2-daydream-ordinary-stage/v1" and record["writer"] == writer
            and (identify_player(Path(reader["player"]), require_integration=True) == reader if resume else writer["player"] == reader)
            and argv == record["argv"] and file_sha256(route) == record["route_sha256"]
            and css_identity(prefix.with_suffix(".profile") / SLOT) == writer["slot"],
            "cold reader staging identity changed")
    return prefix, record


def run_reader(manifest, timeout):
    prefix, record = read_stage(manifest)
    profile = prefix.with_suffix(".profile")
    fast_text = record.get("fast_text")
    expected = {SLOT, SYSTEM}
    if fast_text is not None:
        expected.add("setup.xml")
    require({p.name for p in profile.iterdir()} == expected
            and css_identity(profile / SYSTEM) == record["writer"]["system"], "cold profile must begin with only staged CSS")
    if fast_text is not None:
        require(record.get("resume") and fast_text["message_speed"] == 0
                and file_sha256(profile / "setup.xml") == fast_text["setup_sha256"],
                "cold profile fast text settings changed")
    events_path, host_path = (prefix.with_suffix(ext) for ext in (".events.log", ".host.log"))
    process_path = manifest.parent / "load-process.json"
    require(not process_path.exists(), "reader was already run in this private profile")
    env = {**os.environ, "CS2_MEDIA_CACHE": str(manifest.parent / "movie-input")}
    if os.name == "nt":
        env["PATH"] = os.pathsep.join(("C:/msys64/mingw64/bin", env.get("PATH", "")))
    deadline = time.monotonic() + timeout
    progress = ReplayProgress(True)
    with events_path.open("xb") as events, host_path.open("xb") as host, events_path.open("rb") as reader, \
            subprocess.Popen(record["argv"], cwd=ROOT, env=env, stdout=events, stderr=host) as process:
        expired = wait_for_replay(process, reader, progress, deadline, 1)
    receipt = {"pid": process.pid, "argv": record["argv"], "exit_code": process.returncode,
               "timed_out": expired, "stage_sha256": file_sha256(manifest)}
    if record.get("resume"):
        receipt.update(last_frame=progress.frame, status=progress.replay_status,
                       last_page_wait=progress.last_page_wait, first_engine_error=progress.first_engine_error)
    if fast_text is not None:
        receipt["fast_text"] = fast_text
    write_json(process_path, receipt)
    return receipt


def verify_reader(manifest):
    prefix, record = read_stage(manifest)
    process = read_json(manifest.parent / "load-process.json")
    require(process["exit_code"] == 0 and process["timed_out"] is False and process["pid"] > 0
            and process["argv"] == record["argv"] and process["stage_sha256"] == file_sha256(manifest),
            "reader process failed or differs from the staged command")
    events_path, host_path = (prefix.with_suffix(ext) for ext in (".events.log", ".host.log"))
    verify_completed(events_path)
    events = scan(events_path, {"control": CONTROL, "open": OPEN, "restored": RESTORED, "next": SUCCESSOR,
                               "checkpoint": checkpoint(LOAD_LABEL), "writes": re.compile(r"frame=\d+ fes-datasave .*")})
    start, context, cst_load, owner, epoch, published, region, receiver = map(
        int, only(events["restored"], "restored r121 proof").groups())
    end, next_context, next_load, next_owner, next_epoch, next_published = map(
        int, only(events["next"], "r123 successor proof").groups())
    opened, _, digest = only(events["open"], "native 247 open").groups()
    opened = int(opened)
    require(not events["writes"] and digest == record["writer"]["slot"]["sha256"] and opened <= published < start
            and (context, cst_load, owner) == (next_context, next_load, next_owner) and epoch < next_epoch
            and start < next_published < end == int(only(events["checkpoint"], "reader checkpoint")[1]),
            "reader CSS, restored page or successor identity mismatch")
    host = scan(host_path, {"press": PRESS, "down": DOWN})
    presses = host["press"]
    controls = [match.groups()[:4] for match in events["control"]]
    names = [control[1] for control in controls]
    require(names in (["title", "slot"], ["title", "slot", "confirm"]) and len(presses) == len(names) + 1
            and [(int(m[1]), m[2]) for m in host["down"]]
            == [(int(row[0]), "MouseButton(0)") for row in controls] + [(start, "Keyboard(13)")],
            "cold reader issued input outside the guarded route")
    last = 0
    for (sent, control, control_owner, control_region), press in zip(controls, presses):
        frame, kind, raw = press.groups()
        sent, frame = int(sent), int(frame)
        require(kind == control and last < sent < frame <= opened, "cold reader menu receipt order differs")
        verify_press(sent, int(control_owner), int(control_region), frame, raw)
        last = frame
    entered, kind, raw = presses[-1].groups()
    entered = int(entered)
    require(kind == "enter" and entered <= next_published, "reader r121 Enter was not followed by r123")
    verify_press(start, receiver, region, entered, raw, keyboard=True)
    resume = f"frame={entered} message-resume message=[2, 0, 0, 0]"
    wait = f"frame={entered} input-wait native 71 address=0xd6d"
    selected = [line for line in rows(events_path) if line in (resume, wait)]
    require(selected.count(resume) == 1 and wait in selected,
            "reader Enter did not resume the root wait exactly once")
    return {"status": "verified", "slot": record["writer"]["slot"], "write_frame": record["writer"]["write_frame"],
            "open_frame": opened, "restored_r121_frame": start, "enter_frame": entered, "successor_r123_frame": end}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="mode", required=True)
    sub.add_parser("write-scenario")
    staging = sub.add_parser("stage")
    staging.add_argument("writer_batch", type=Path, help="completed writer 17.json")
    staging.add_argument("folder", type=Path, help="new cold-reader directory")
    staging.add_argument("--resume-player", type=Path,
                         help="integration player with the guarded CASE route continuation")
    staging.add_argument("--fast-text", action="store_true", help="seed fastest native message speed for resuming")
    run = sub.add_parser("run-load")
    run.add_argument("manifest", type=Path)
    run.add_argument("--timeout", type=int, default=900)
    check = sub.add_parser("verify")
    check.add_argument("manifest", type=Path)
    args = parser.parse_args()
    try:
        if args.mode == "write-scenario":
            print(rendered(write_scenario()), end="")
        elif args.mode == "stage":
            print(stage(args.writer_batch.absolute(), args.folder.absolute(), args.resume_player, fast_text=args.fast_text))
        elif args.mode == "run-load":
            require(args.timeout > 0, "timeout must be positive")
            result = run_reader(args.manifest.absolute(), args.timeout)
            if not read_json(args.manifest).get("resume"):
                result = verify_reader(args.manifest.absolute())
            print(json.dumps(result, indent=2))
        else:
            print(json.dumps(verify_reader(args.manifest.absolute()), indent=2))
    except (OSError, ValueError, KeyError, TypeError, IndexError) as error:
        parser.error(str(error))


if __name__ == "__main__":
    main()

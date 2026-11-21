"""Run and verify NEKOPARA ordinary save and cold load in private PC profiles."""

import argparse
import json
import math
import os
from pathlib import Path
import re
import shutil
import subprocess
import time
import xml.etree.ElementTree as ET

from check_neko_save_roundtrip import (CONTINUATION_WAITS, OPEN, RESUME, SLOT_NAME, WRITE,
                                       check_roundtrip, lines, ordinary_route, slot_identity)
from check_neko_story_boundaries import RESOURCES
from neko_cstl_continuation import (FIRST_CHAPTERS, branch_chapters, check_continuation_roundtrip,
                                    continuation_route, resource_pages, verify)
from run_playthrough_batch import GIB, ROOT, available_ram_bytes, file_sha256, resource_tree_sha256, write_json
from stage_neko_cold_language import CACHE, INVENTORY, SAVE_ENTRY_MODE, stage


CHAPTER_CHECKPOINT = "chapter_checkpoint_roundtrip.json"
SCENE01_CHECKPOINT = "scene01_checkpoint_roundtrip.json"


def run_pass(run, name, route, identity, slot_before, env, timeout, frames=25000):
    folder = Path(run["run"])
    route_path = folder / f"{name}.route.json"
    events = folder / f"{name}.events.log"
    host = folder / f"{name}.host.log"
    prefix = Path(run["argv_prefix"][2])
    slot = prefix.with_suffix(".profile") / "save0000.dat"
    write_json(route_path, route)
    argv = [*run["argv_prefix"], str(frames), "--route-json", str(route_path), "trace-native:630"]
    if "cstl_pages" in route["neko_ordinary"]:
        argv.append("trace-native:1040")
    started = time.time_ns() // 1_000_000
    timed_out = False
    with events.open("xb") as stdout, host.open("xb") as stderr, subprocess.Popen(
        argv, cwd=ROOT, env=env, stdout=stdout, stderr=stderr,
        creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
    ) as process:
        try:
            process.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            timed_out = True
            process.kill()
            process.wait()
        finished = time.time_ns() // 1_000_000
    try:
        slot_after = slot_identity(slot)
    except FileNotFoundError:
        slot_after = None
    return {**identity, "pid": process.pid, "start_ms": started, "exit_ms": finished,
            "exit_code": process.returncode, "timed_out": timed_out, "output_prefix": str(prefix),
            "events": events.name, "host": host.name, "slot_before": slot_before,
            "slot_after": slot_after}


def opening_checkpoint(source, language, slot, staged, resource_sha256):
    writer = json.loads((source / "save.json").read_text(encoding="utf-8"))
    save_config, load_config = (
        json.loads((source / f"{name}.route.json").read_text(encoding="utf-8-sig"))["neko_ordinary"]
        for name in ("save", "load")
    )
    if (save_config["mode"] != "save" or load_config["mode"] != "load"
            or save_config["language"] != language or load_config["language"] != language
            or load_config["slot_sha256"] != slot["sha256"] or load_config.get("resume_at") is not None):
        raise ValueError(f"{language}: seed route must prove the same language and ordinary r218 slot")
    if (Path(writer["output_prefix"]) != source / "01" or writer["exit_code"] != 0
            or writer["timed_out"] or writer["slot_before"] is not None or writer["slot_after"] != slot
            or writer["resource_tree_sha256"] != resource_sha256
            or any(writer[key] != staged["player"][key] for key in ("sha256", "source_tree_sha256"))
            or writer["events"] != "save.events.log"):
        raise ValueError(f"{language}: ordinary slot, resources or archived producer identity changed")
    write_events = lines(source / writer["events"])
    writes = [line for line in write_events if " fes-datasave " in line]
    if len(writes) != 1 or not WRITE.fullmatch(writes[0]) or write_events[-1:] != ["replay_status=completed"]:
        raise ValueError(f"{language}: no completed in-game ordinary slot write")
    read_events = lines(source / "load.events.log")
    opens = [int(match[1]) for line in read_events if (match := OPEN.fullmatch(line)) and match[3] == slot["sha256"]]
    advances = [int(match[1]) for line in read_events if (match := re.fullmatch(
        rf"replay frame=(\d+) verified={re.escape(json.dumps(RESUME))}", line))]
    if len(opens) != 1 or len(advances) != 1 or opens[0] >= advances[0]:
        raise ValueError(f"{language}: no matching in-game reopen and first page turn")
    return writer, None


def scene_checkpoint(source, path, language, slot, staged, resource_sha256, *, retry_restored=False):
    receipt = json.loads(path.read_text(encoding="utf-8"))
    saved_scene = receipt.get("scene", "01")
    schema = "cs2-neko-scene01-checkpoint-roundtrip/v1" if saved_scene == "01" else "cs2-neko-chapter-checkpoint-roundtrip/v1"
    if (receipt.get("schema") != schema or saved_scene not in ("01", *FIRST_CHAPTERS[2:])
            or receipt.get("status") not in ({"pass", "fail"} if retry_restored else {"pass"})
            or receipt.get("language") != language):
        raise ValueError(f"{language}: no verified chapter checkpoint roundtrip")
    writer, reader = receipt["writer"], receipt["reader"]
    saved_at = receipt["save_at"]
    if (type(saved_at) is not int or saved_at < (547 if saved_scene == "01" else 1) or writer["pid"] == reader["pid"]
            or writer["exit_ms"] > reader["start_ms"] or writer["slot_after"] != slot
            or reader["slot_before"] != slot or reader["slot_after"] != slot):
        raise ValueError(f"{language}: checkpoint CSS or process order changed")
    proofs = {}
    for label, process in (("save", writer), ("load", reader)):
        name = f"{label}-checkpoint"
        if (Path(process["output_prefix"]) != source / "01" or process["exit_code"] != 0
                or process["timed_out"] or process["resource_tree_sha256"] != resource_sha256
                or any(process[key] != staged["player"][key] for key in ("sha256", "source_tree_sha256"))
                or (process["events"], process["host"]) != (f"{name}.events.log", f"{name}.host.log")):
            raise ValueError(f"{language}: checkpoint {label} process identity changed")
        route = json.loads((source / f"{name}.route.json").read_text(encoding="utf-8"))
        config = route["neko_ordinary"]
        if (config["language"] != language or config["mode"] != "load"
                or config["slot_sha256"] != process["slot_before"]["sha256"]):
            raise ValueError(f"{language}: checkpoint {label} route or input slot changed")
        if label == "load" and (config.get("resume_at") != saved_at or config.get("save_after", False)
                                or config.get("resume_scene", "01") != saved_scene):
            raise ValueError(f"{language}: checkpoint reader did not load the recorded saved page")
        proofs[label] = verify(source / process["events"], source / process["host"], route)
    written = proofs["save"].get("checkpoint_written", {})
    if (proofs["save"]["status"] != "pass" or written.get("record") != saved_at
            or written.get("scene") != saved_scene or written.get("sha256") != slot["sha256"]):
        raise ValueError(f"{language}: no matching in-game checkpoint write")
    read = proofs["load"]
    if read["status"] == "pass" and read["guarded_edges"] >= 1:
        return writer, saved_scene, saved_at
    if not retry_restored:
        raise ValueError(f"{language}: no matching in-game checkpoint write, cold reopen and first page turn")
    page = read["last_page"] or {}
    opens = [match for line in lines(source / reader["events"]) if (match := OPEN.fullmatch(line))]
    if (read["replay_status"] != ["checkpoint_mismatch"] or read["verified_pages"] != 1
            or page.get("record") != saved_at or page.get("scene", "01") != saved_scene
            or len(opens) != 1 or opens[0][3] != slot["sha256"]
            or page["earliest"] != max(int(opens[0][1]), page["loaded"])):
        raise ValueError(f"{language}: retry requires an exact cold restoration of the written checkpoint")
    return writer, saved_scene, saved_at


def stage_checkpoint(previous, language, run, resource_sha256, *, retry_restored=False):
    source = (previous / language).resolve()
    if not source.is_relative_to(CACHE.resolve()):
        raise ValueError("checkpoint must come from the PC replay cache")
    staged = json.loads((source.parent / "stage.json").read_text(encoding="utf-8"))
    slot_path = source / "01.profile" / SLOT_NAME
    slot = slot_identity(slot_path)
    receipt = next((source / name for name in (CHAPTER_CHECKPOINT, SCENE01_CHECKPOINT) if (source / name).is_file()), None)
    if receipt is not None:
        writer, saved_scene, saved_at = scene_checkpoint(source, receipt, language, slot, staged, resource_sha256,
                                                          retry_restored=retry_restored)
    else:
        if retry_restored:
            raise ValueError(f"{language}: no ordinary scene checkpoint to retry")
        writer, saved_at = opening_checkpoint(source, language, slot, staged, resource_sha256)
        saved_scene = "01"
    destination = Path(run["argv_prefix"][2]).with_suffix(".profile") / SLOT_NAME
    with slot_path.open("rb") as original, destination.open("xb") as copy:
        shutil.copyfileobj(original, copy)
    if slot_identity(destination) != slot:
        raise ValueError(f"{language}: staged slot identity changed during copy")
    return writer, slot, saved_scene, saved_at


def run_checkpoint_roundtrip(run, language, startup, identity, seed_writer, seed_slot, seed_scene, seed_record,
                             save_pages, read_pages, save_chapters, read_chapters, env, timeout, frames):
    folder = Path(run["run"])
    if save_chapters:
        saved_scene = save_chapters[-1]["scene"]
        save_at = save_chapters[-1]["pages"][-1]["wait"]
        target = read_chapters[-1]["pages"][-1]["wait"]
        schema, filename = "cs2-neko-chapter-checkpoint-roundtrip/v1", CHAPTER_CHECKPOINT
    else:
        saved_scene, save_at, target = "01", save_pages[-1]["wait"], read_pages[-1]["wait"]
        schema, filename = "cs2-neko-scene01-checkpoint-roundtrip/v1", SCENE01_CHECKPOINT
    report = {"schema": schema, "status": "fail", "language": language,
              "scene": saved_scene, "save_at": save_at, "to": target,
              "seed": {"writer": seed_writer, "slot": seed_slot}}
    try:
        save_route = continuation_route(language, startup, seed_slot["sha256"], save_pages,
                                        resume_at=seed_record, resume_scene=seed_scene, chapters=save_chapters)
        save_route["neko_ordinary"]["save_after"] = True
        writer = run_pass(run, "save-checkpoint", save_route, identity, seed_slot, env, timeout, frames)
        report["writer"] = writer
        if writer["exit_code"] != 0 or writer["timed_out"]:
            raise ValueError("checkpoint writer did not exit normally")
        save_proof = verify(folder / writer["events"], folder / writer["host"], save_route)
        report["save_proof"] = save_proof
        saved = writer["slot_after"]
        if (save_proof["status"] != "pass" or saved is None
                or save_proof["checkpoint_written"]["sha256"] != saved["sha256"]):
            raise ValueError("checkpoint writer did not prove the saved CSS")

        slot_path = Path(writer["output_prefix"]).with_suffix(".profile") / SLOT_NAME
        if slot_identity(slot_path) != saved:
            raise ValueError("checkpoint CSS changed before the reader started")
        read_route = continuation_route(language, "title", saved["sha256"], read_pages,
                                        resume_at=save_at, resume_scene=saved_scene, chapters=read_chapters)
        reader = run_pass(run, "load-checkpoint", read_route, identity, saved, env, timeout, frames)
        report["reader"] = reader
        if (reader["exit_code"] != 0 or reader["timed_out"] or reader["pid"] == writer["pid"]
                or writer["exit_ms"] > reader["start_ms"] or reader["slot_after"] != saved):
            raise ValueError("checkpoint reader must exit normally with a new PID and the same CSS")
        read_proof = verify(folder / reader["events"], folder / reader["host"], read_route)
        report["read_proof"] = read_proof
        if read_proof["status"] != "pass" or read_proof["guarded_edges"] < 1:
            raise ValueError("checkpoint reader did not prove a guarded page turn")
        report["status"] = "pass"
    except (OSError, ValueError, KeyError, TypeError, IndexError) as error:
        report["error"] = str(error)
    receipt = folder / filename
    write_json(receipt, report)
    print(f"{language}: checkpoint_{report['status']} ({receipt})", flush=True)
    return report["status"] == "pass"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--player", type=Path, required=True)
    parser.add_argument("--language", choices=("jp", "cn"), action="append")
    parser.add_argument("--to", type=int, default=485,
                        help="scene 01 wait record, including guarded continuation through r5386")
    parser.add_argument("--observe-branch", action="store_true", help="allow the guarded 01/491/5386 branch turn")
    following = parser.add_mutually_exclusive_group()
    following.add_argument("--follow-branch-pages", type=int, metavar="COUNT",
                           help="verify COUNT pages in the chapter selected by the game after --to 5386")
    following.add_argument("--follow-scene02-pages", type=int, metavar="COUNT",
                           help="follow the actual branch to scene 02 and verify its first COUNT pages")
    following.add_argument("--follow-to-chapter", choices=FIRST_CHAPTERS[2:],
                           help="continue to this chapter and verify its starting pages")
    parser.add_argument("--chapter-pages", type=int, default=2,
                        help="number of pages to verify in --follow-to-chapter (default 2)")
    parser.add_argument("--timeout", type=int, help="wall-clock seconds per process (900 or 1800 for a long route)")
    parser.add_argument("--min-available-ram-gib", type=float, default=4,
                        help="minimum available RAM for the next language run (default 4 GiB)")
    parser.add_argument("--frames", type=int, help="load frame cap (25000 or 40000 for a long route)")
    parser.add_argument("--resume-from", type=Path,
                        help="reopen a verified ordinary slot from a prior PC stage for candidate testing")
    parser.add_argument("--retry-restored-checkpoint", action="store_true",
                        help="reuse a verified write and cold restoration whose first advancing guard rejected the successor")
    parser.add_argument("--fast-text", action="store_true",
                        help="use the game's fastest text reveal setting in the isolated profile")
    parser.add_argument("--save-at", type=int,
                        help="write a normal slot at this wait in scene 01 or --follow-to-chapter, then cold-load")
    args = parser.parse_args()
    if any(value is not None and value <= 0 for value in (args.timeout, args.frames)):
        parser.error("--timeout and --frames must be positive")
    if args.retry_restored_checkpoint and (args.resume_from is None or args.save_at is None):
        parser.error("--retry-restored-checkpoint requires --resume-from and --save-at")
    if not math.isfinite(args.min_available_ram_gib) or args.min_available_ram_gib <= 0:
        parser.error("--min-available-ram-gib must be finite and positive")
    if args.observe_branch and args.to != 5386:
        parser.error("--observe-branch requires --to 5386")
    if args.follow_branch_pages is not None and (args.to != 5386 or not 2 <= args.follow_branch_pages <= 365):
        parser.error("--follow-branch-pages requires --to 5386 and a count between 2 and 365")
    if args.follow_scene02_pages is not None and (args.to != 5386 or not 2 <= args.follow_scene02_pages <= 512):
        parser.error("--follow-scene02-pages requires --to 5386 and a count between 2 and 512")
    if args.follow_to_chapter and (args.to != 5386
            or not 2 <= args.chapter_pages <= RESOURCES[f"{args.follow_to_chapter}.cst"][3][1] + 1):
        parser.error("--follow-to-chapter requires --to 5386 and a valid count for --chapter-pages")
    if args.save_at is not None and (args.resume_from is None or args.follow_branch_pages is not None
                                    or args.follow_scene02_pages is not None
                                    or not args.follow_to_chapter and (args.observe_branch
                                        or not 547 <= args.save_at < min(args.to, 5386))):
        parser.error("--save-at requires --resume-from and a wait before the endpoint in scene 01 or --follow-to-chapter")
    languages = tuple(dict.fromkeys(args.language or ("jp", "cn")))
    continuous = args.to not in CONTINUATION_WAITS
    pages = {}
    save_pages = {}
    branch_pages = {}
    save_branch_pages = {}
    if continuous:
        game = ROOT / json.loads(INVENTORY.read_text(encoding="utf-8"))["games"][1]["root"]
        try:
            pages = {language: resource_pages(game, language, args.to) for language in languages}
            if args.save_at is not None and not args.follow_to_chapter:
                save_pages = {language: resource_pages(game, language, args.save_at) for language in languages}
            if args.follow_branch_pages is not None:
                branch_pages = {language: branch_chapters(game, language, args.follow_branch_pages) for language in languages}
            if args.follow_scene02_pages is not None:
                branch_pages = {language: branch_chapters(game, language, None, through="02", last_pages=args.follow_scene02_pages)
                                for language in languages}
            if args.follow_to_chapter:
                branch_pages = {language: branch_chapters(game, language, None, through=args.follow_to_chapter,
                                                          last_pages=args.chapter_pages)
                                for language in languages}
                if args.save_at is not None:
                    save_pages = pages
                    for language, chapters in branch_pages.items():
                        chapter = chapters[-1]
                        count = next((i for i, page in enumerate(chapter["pages"], 1) if page["wait"] == args.save_at), 0)
                        if not 2 <= count < len(chapter["pages"]):
                            raise ValueError("--save-at needs a guarded page after the chapter's first page and before its endpoint")
                        save_branch_pages[language] = [*chapters[:-1], {**chapter, "pages": chapter["pages"][:count]}]
        except (OSError, ValueError, KeyError, IndexError) as error:
            parser.error(str(error))
    timeout = args.timeout or (1800 if continuous else 900)
    frames = args.frames or (40000 if continuous else 25000)
    out = stage(args.player, file_sha256(args.player), startup_probe=SAVE_ENTRY_MODE,
                fast_text=args.fast_text)
    print(out, flush=True)
    staged = json.loads((out / "stage.json").read_text(encoding="utf-8"))
    env = {**os.environ, "CS2_MEDIA_CACHE": str(out / "movie-input")}
    if not continuous:
        env.update(staged["environment"])
    if os.name == "nt":
        env["PATH"] = os.pathsep.join(("C:/msys64/mingw64/bin", env.get("PATH", "")))
    identity = {key: staged["player"][key] for key in ("sha256", "source_tree_sha256")}
    identity["resource_tree_sha256"] = resource_tree_sha256(Path(staged["runs"]["jp"]["argv_prefix"][1]))
    failed = False
    for language in languages:
        run = staged["runs"][language]
        folder = Path(run["run"])
        if available_ram_bytes() < args.min_available_ram_gib * GIB or shutil.disk_usage(folder).free < 22 * GIB:
            write_json(folder / "check.json", {"status": "resource_deferred"})
            print(f"{language}: resource_deferred ({folder})", flush=True)
            failed = True
            continue
        checkpoint = args.resume_from is not None
        startup = "language-select" if checkpoint else "title"
        seed_scene, seed_record = "01", None
        if checkpoint:
            try:
                writer, slot, seed_scene, seed_record = stage_checkpoint(
                    args.resume_from, language, run, identity["resource_tree_sha256"],
                    retry_restored=args.retry_restored_checkpoint)
                chapters = branch_pages.get(language, [])
                target_scene = chapters[-1]["scene"] if chapters else "01"
                target = args.save_at or (chapters[-1]["pages"][-1]["wait"] if chapters else args.to)
                order = ("01", *FIRST_CHAPTERS)
                if seed_record is not None and (order.index(seed_scene), seed_record) >= (order.index(target_scene), target):
                    raise ValueError(f"next target must be after the verified ordinary {seed_scene}/r{seed_record} slot")
            except (OSError, ValueError, KeyError, TypeError) as error:
                write_json(folder / "check.json", {"status": "checkpoint_rejected", "error": str(error)})
                print(f"{language}: checkpoint_rejected ({folder}): {error}", flush=True)
                failed = True
                continue
            if args.save_at is not None:
                failed |= not run_checkpoint_roundtrip(
                    run, language, startup, identity, writer, slot, seed_scene, seed_record,
                    save_pages[language], pages[language], save_branch_pages.get(language), branch_pages.get(language),
                    env, timeout, frames,
                )
                continue
        else:
            writer = run_pass(run, "save", ordinary_route(language, "save", startup, None),
                              identity, None, env, timeout)
            write_json(folder / "save.json", writer)
            slot = writer["slot_after"]
            with (folder / writer["events"]).open("rb") as events:
                written = any(line.rstrip() == b"replay_status=completed" for line in events)
            if writer["exit_code"] != 0 or slot is None or not written:
                print(f"{language}: ordinary save failed ({folder})", flush=True)
                failed = True
                continue
        if continuous:
            route = continuation_route(language, startup, slot["sha256"], pages[language], resume_at=seed_record,
                                       resume_scene=seed_scene, chapters=branch_pages.get(language))
            if args.observe_branch:
                route["neko_ordinary"]["observe_branch"] = True
        else:
            route = ordinary_route(language, "load", startup, slot["sha256"], args.to)
        reader = run_pass(run, "load", route, identity, slot, env, timeout, frames)
        receipt = folder / ("checkpoint_reopen.json" if checkpoint else "roundtrip.json")
        schema = "cs2-neko-checkpoint-reopen/v1" if checkpoint else "cs2-neko-save-roundtrip/v1"
        write_json(receipt, {"schema": schema, "language": language,
                             "save_confirm": False, "reopen_startup": startup,
                             "output_prefix": reader["output_prefix"], "producer": writer, "consumer": reader})
        try:
            if seed_record is not None:
                result = verify(folder / reader["events"], folder / reader["host"], route)
                passed = (result["status"] == "pass" and reader["slot_before"] == reader["slot_after"] == slot
                          and reader["exit_code"] == 0 and not reader["timed_out"]
                          and reader["pid"] != writer["pid"] and writer["exit_ms"] < reader["start_ms"])
                report = {"missing": [] if passed else ["verified continuation of the ordinary checkpoint"],
                          "failures": [], "continuation": result}
            else:
                report = (check_continuation_roundtrip(receipt, route, checkpoint=checkpoint) if continuous
                          else check_roundtrip(receipt, args.to, False, checkpoint=checkpoint))
        except (OSError, ValueError, KeyError, TypeError, IndexError, ET.ParseError) as error:
            report = {"missing": [str(error)], "failures": []}
        status = "fail" if report["failures"] else "insufficient" if report["missing"] else "pass"
        kind = f"checkpoint_{status}" if checkpoint else status
        write_json(folder / "check.json", {"status": kind, **report})
        print(f"{language}: {kind} ({receipt})", flush=True)
        failed |= status != "pass"
    return int(failed)


if __name__ == "__main__":
    raise SystemExit(main())

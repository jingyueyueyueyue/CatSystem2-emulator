"""Write and cold-load an ordinary slot through the Windows replay player."""

import argparse
from contextlib import contextmanager, nullcontext
from datetime import datetime
import json
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import time

from check_neko_save_roundtrip import slot_identity
from run_djibril_save_roundtrip import WRITE, log
from run_playthrough_batch import (
    GIB, INVENTORY, available_ram_bytes, identify_player, load_inventory,
    new_output_directory, playthrough_cache_root, run_case, write_json,
)
from run_yukikoi_save_roundtrip import pointer_receipts


SLOT = "save0000.dat"
WRITE_REJECTED = re.compile(r"frame=\d+ fes-datasave slot=0 container=\d+ written=false error=.+")
CASES = {
    3: {
        "save_receipts": {(2771, 18, 12, 1), (2921, 128, 32, 0)},
        "load_receipts": {(2453, 16, 8, 1), (2571, 65, 31, 0), (2746, 94, 86, 1), (3011, 73, 8, 0)},
        "resume": "frame=3011 message-resume message=[2, 640, 540, 0]",
        "captures": ("03.2990.png", "03.3170.png"),
        "frames": 3190,
        "inputs": [
            "230:557,324", "465:557,324", "525:557,324", "549:557,324", "573:557,324",
            "inspect:2448", "move:2450:176,414", "require-hit:2452:16", "2452:176,414", "inspect:2557",
            "move:2568:471,146", "require-hit:2570:65", "2570:471,146", "inspect:2740",
            "move:2743:546,348", "require-hit:2745:94", "2745:546,348", "inspect:2990",
            "move:3008:640,540", "require-hit:3010:73", "3010:640,540", "inspect:3170",
        ],
    },
    6: {
        "save_receipts": {(3911, 23, 12, 0), (4201, 55, 36, 0), (4426, 105, 100, 0)},
        "rejected_write_frame": 4590,
        "load_receipts": {(2331, 32, 13, 0), (2671, 50, 45, 0), (3451, 64, 8, 0)},
        "load_windows": {
            1910: ["flow", "ttlflow", "title"],
            2310: ["flow", "ttlflow", "saveload", "saveload_c"],
            2650: ["flow", "ttlflow", "saveload", "saveload_c", "confirm"],
            3420: ["flow", "meswnd.fes"],
        },
        "resume": "frame=3451 message-resume message=[2, 1135, 687, 0]",
        "captures": ("06.3420.png", "06.3660.png"),
        "frames": 3690,
        "inputs": [
            "inspect-log:1910", "move:1915:84,308", "require-hit:1920:19", "1920:84,308", "inspect-log:2310",
            "move:2315:682,208", "require-hit:2330:32", "2330:682,208", "inspect-log:2650",
            "move:2655:543,346", "require-hit:2670:50", "2670:543,346", "inspect:3420",
            "move:3435:1135,687", "require-hit:3450:64", "3450:1135,687", "inspect:3660",
        ],
    },
}


def await_resources(output):
    deadline = time.monotonic() + 120
    while available_ram_bytes() < 4 * GIB or shutil.disk_usage(output).free < 22 * GIB:
        if time.monotonic() >= deadline:
            raise ValueError("insufficient RAM or disk for one replay")
        time.sleep(5)


def observed_receipts(result, expected):
    return expected.issubset(pointer_receipts(log(result, "stderr"), {press[0] for press in expected}))


@contextmanager
def deny_new_files(profile):
    identity = subprocess.check_output(["whoami.exe", "/user", "/fo", "csv", "/nh"])
    sid = re.search(rb"\bS-1-(?:\d+-)+\d+\b", identity)
    if sid is None:
        raise ValueError("Windows user SID unavailable for the private profile")
    account = "*" + sid[0].decode("ascii")

    def change_acl(option, entry):
        result = subprocess.run(["icacls.exe", str(profile), option, entry], capture_output=True)
        if result.returncode:
            raise ValueError(f"icacls {option} failed for the private profile (exit {result.returncode})")

    change_acl("/deny", account + ":(WD)")
    try:
        try:
            with (profile / "directory-acl-check").open("xb"):
                pass
        except PermissionError:
            yield
        else:
            (profile / "directory-acl-check").unlink()
            raise ValueError("the private profile still allows new files")
    finally:
        change_acl("/remove:d", account)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--player", required=True, type=Path)
    parser.add_argument("--cache-root", type=Path, help="absolute directory for the playthrough cache")
    parser.add_argument("--index", required=True, choices=CASES, type=int)
    parser.add_argument("--timeout", type=int, default=360)
    parser.add_argument("--overwrite-seed", type=Path,
                        help="overwrite an existing ordinary slot staged from this archive")
    denial = parser.add_mutually_exclusive_group()
    denial.add_argument("--read-only", action="store_true",
                        help="on Windows, verify a read-only seeded slot survives a rejected overwrite")
    denial.add_argument("--deny-directory-writes", action="store_true",
                        help="on Windows, deny file creation in the isolated profile and verify the seeded slot")
    args = parser.parse_args()
    if args.timeout < 1:
        parser.error("timeout must be positive")
    reject_write = args.read_only or args.deny_directory_writes
    if reject_write and (os.name != "nt" or args.index != 6 or args.overwrite_seed is None):
        parser.error("write rejection probes require Windows, --index 6, and --overwrite-seed")

    player = args.player.resolve(strict=True)
    build = identify_player(player, require_integration=True)
    env = os.environ.copy()
    try:
        cache = playthrough_cache_root(args.cache_root)
    except ValueError as error:
        parser.error(str(error))
    output = new_output_directory(cache, datetime.now().astimezone())
    print(output, flush=True)
    env["CS2_MEDIA_CACHE"] = str(output / "movie-input")
    if os.name == "nt":
        env["PATH"] = os.pathsep.join(("C:/msys64/mingw64/bin", env.get("PATH", "")))

    index = args.index
    case = CASES[index]
    game = load_inventory(INVENTORY)[0][index]
    producer = Path(__file__).with_name(f"pc_story_{index:02}_ordinary_save.json")
    scenario = json.loads(producer.read_text(encoding="utf-8"))[str(index)]
    save_dir, reopen_dir = output / "save", output / "reopen"
    save_dir.mkdir()
    result = {"status": "failed", "build": build}
    try:
        if args.overwrite_seed is not None:
            seed = args.overwrite_seed.resolve(strict=True)
            profile = save_dir / f"{index:02}.profile"
            profile.mkdir()
            staged_seed = profile / SLOT
            shutil.copyfile(seed, staged_seed)
            result["seed"] = {"path": str(seed), **slot_identity(staged_seed)}
            if args.read_only:
                staged_seed.chmod(stat.S_IREAD)
                result["read_only"] = True
        await_resources(output)
        save_windows = {case["rejected_write_frame"]: ["flow", "meswnd.fes", "saveload", "saveload_c"]} if reject_write else None
        writers = deny_new_files(profile) if args.deny_directory_writes else nullcontext()
        with writers:
            saved = run_case(index, game, scenario["frames"], scenario["inputs"],
                             player, save_dir, args.timeout, env, window_checks=save_windows)
        if args.deny_directory_writes:
            result["directory_write_denied"] = True
            with (profile / "directory-acl-check").open("xb"):
                pass
            (profile / "directory-acl-check").unlink()
        result["producer"] = saved
        expected_write = WRITE_REJECTED if reject_write else WRITE
        if (saved["status"] != "visual_review_required"
                or sum(bool(expected_write.fullmatch(line)) for line in log(saved, "stdout")) != 1
                or not observed_receipts(saved, case["save_receipts"])):
            detail = (saved["first_engine_error"] or {}).get("detail")
            raise ValueError(detail or "expected ordinary slot write outcome or menu click was not verified")

        slot = Path(saved["profile"]) / SLOT
        identity = slot_identity(slot)
        result["slot"] = identity
        if args.overwrite_seed is not None:
            identical = result["seed"]["sha256"] == identity["sha256"]
            if reject_write and not identical:
                raise ValueError("rejected overwrite changed the slot bytes")
            if args.read_only and slot.stat().st_mode & stat.S_IWRITE:
                raise ValueError("rejected overwrite changed the read-only attribute")
            if not reject_write and identical:
                raise ValueError("overwrite left the seeded slot byte-identical")
        profile = reopen_dir / f"{index:02}.profile"
        profile.mkdir(parents=True)
        staged = profile / SLOT
        shutil.copyfile(slot, staged)
        if slot_identity(staged) != identity:
            raise ValueError("staged slot differs from the producer slot")

        await_resources(output)
        inputs = ["story-events", "capture-every:0", "button-probe", *case["inputs"]]
        reopened = run_case(index, game, case["frames"], inputs, player, reopen_dir, args.timeout, env,
                            window_checks=case.get("load_windows"))
        result["consumer"] = reopened
        if (reopened["status"] != "visual_review_required" or reopened["pid"] is None
                or reopened["pid"] == saved["pid"]):
            detail = (reopened["first_engine_error"] or {}).get("detail")
            raise ValueError(detail or "cold read did not finish in a new process")

        if not observed_receipts(reopened, case["load_receipts"]):
            raise ValueError("missing title, slot, confirmation, or story click receipt")
        if case["resume"] not in log(reopened, "stdout"):
            raise ValueError("story did not resume at the click")
        before, after = case["captures"]
        if not {before, after}.issubset(reopened["captures"]) or slot_identity(staged) != identity:
            raise ValueError("missing story captures or ordinary slot modified by load")
        result.update(status="visual_review_required", restored_page=before, next_page=after)
    except (OSError, ValueError) as error:
        result["reason"] = str(error)
    write_json(output / "roundtrip.json", result)
    print(result["status"], result.get("reason", ""), flush=True)
    return int(result["status"] == "failed")


if __name__ == "__main__":
    raise SystemExit(main())

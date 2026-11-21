"""Check Campanella startup inputs and verify recorded entry receipts."""

import argparse
from hashlib import sha256
import json
from pathlib import Path
import re
from struct import unpack_from
from xml.etree import ElementTree

import pefile

from audit_runtime_evidence import blowfish_le_decrypt, runtime_fingerprint
from check_pc_story_14_18_continuous import classify, native_frames, press_rows, require
from cst_variants import expand_cst
from plan_island_bad02 import checkpoint_rows
from run_playthrough_batch import INVENTORY, ROOT, load_inventory
from startup_inventory import entries_at


JP_ROOT = ROOT / "archive/2026-11/20261120_campanella_pc/jp"
CN_ROOT = JP_ROOT.with_name("cn")
PE_SHA256 = "659d5f069f3c97f69a61e12449cf1510b45e48a10649b9ac6977c2b93d6d28ad"
XML_SHA256 = "36e082008b51cdb7c820506fcc3d3c2ad285ecafd395a020b4f467a16bbb2a78"
ENTRY = "kcs.int/main.kcs"
KCS_SHA256 = "5891e73bdbcf5d8eede4cee81e87c8a2d76695a55f40ba773e26d3595d89c401"
OVERLAY = "update00.int"
OVERLAY_SHA256 = "c7465d70274449232d3e6161f0eefdac3af4dc28351746030b20c834456da974"
OVERLAY_CST_SHA256 = "a46748a49c7970fc13a19f0b3522b59523a3be78a51bb19ee0ba0932e7d86b27"
FONT = "WenQuanYi_msgothic.ttf"
FONT_SHA256 = "88cd7c2b992024550d8e2ce357d2c6146bc3ea07a57cf67272eb5f4cbef07517"


def list_members(path):
    data = path.read_bytes().split(b"\0", 1)[0]
    members = []
    for line in data.decode("ascii").removesuffix("\n").split("\n"):
        directory, slash, name = line.removesuffix("\r").partition("/")
        if not slash or not name or len(directory) >= 64 or len(name) >= 32:
            raise ValueError(f"{path.name}: invalid archive list entry {line!r}")
        members.append(name)
    return members


def read_member(root, archive, member, signature, seed):
    entries, _, cipher = entries_at(root / archive, seed)
    matches = [entry for entry in entries if entry["name"].lower() == member]
    if len(matches) != 1:
        raise ValueError(f"{archive}: expected one {member!r}")
    entry = matches[0]
    with (root / archive).open("rb") as handle:
        handle.seek(entry["offset"])
        data = handle.read(entry["size"])
    if cipher:
        data = blowfish_le_decrypt(cipher, data)
    if len(data) != entry["size"] or not data.startswith(signature):
        raise ValueError(f"{archive}: invalid {member!r} payload")
    return data


def check(root, report, *, localized=False):
    identity = runtime_fingerprint(root / "cs2.exe")
    seed = int(identity["archive_name_seed"], 0)
    config = (root / "config/startup.xml").read_bytes()
    xml = ElementTree.fromstring(config.decode("cp932"))
    report.update(pe_sha256=identity["sha256"], name_seed=identity["archive_name_seed"],
                  xml_sha256=sha256(config).hexdigest(), entry=xml.findtext("SCRIPT/start"))
    needed = {
        int(value)
        for i in range(64)
        if (value := xml.findtext(f"INT/need{i:02}")) is not None
    }
    report["required"] = []
    for slot in sorted(slot for slot in needed if 0 <= slot < 128):
        key = f"INT/int{slot:02}"
        name = xml.findtext(f"{key}/int")
        directory = xml.findtext(f"{key}/dir")
        try:
            if not name:
                raise ValueError("empty archive name")
            members = list_members(root / name) if name.lower().endswith(".lst") else [name]
            for member in members:
                entries_at(root / member, seed)
        except (OSError, ValueError) as error:
            report["first_required_failure"] = dict(slot=slot, name=name, directory=directory)
            raise ValueError(f"{key}: {error}") from error
        report["required"].append(dict(slot=slot, name=name, directory=directory, archives=members))

    if (report["pe_sha256"], report["xml_sha256"], seed, report["entry"]) != (
        PE_SHA256, XML_SHA256, 0x4260E643, ENTRY
    ):
        raise ValueError("original Campanella PE, startup XML, seed or entry differs")
    overlays = sorted(path.name for path in root.iterdir() if
                      re.fullmatch(r"update(?:[0-9]{2})?\.int", path.name, re.IGNORECASE))
    numbered = [name for name in overlays if name.lower() != "update.int"]
    if localized:
        if [name.lower() for name in numbered] != [OVERLAY]:
            raise ValueError(f"expected only {OVERLAY} as a numbered overlay: {overlays}")
        overlay = root / numbered[0]
        overlay_digest = sha256(overlay.read_bytes()).hexdigest()
        if overlay_digest != OVERLAY_SHA256 or sha256((root / FONT).read_bytes()).hexdigest() != FONT_SHA256:
            raise ValueError("localized overlay or companion font differs")
        report.update(overlay=overlay.name, overlay_sha256=overlay_digest, font=FONT)
    elif overlays:
        raise ValueError(f"unexpected overlays in Japanese replay root: {overlays}")
    manifest = Path(__file__).with_name("pc_story_18_continuous_g6.json")
    review = json.loads(manifest.read_text(encoding="utf-8"))["review"]
    kcs = sha256(read_member(root, "kcs.int", b"main.kcs", b"KCS\0", seed)).hexdigest()
    archive, member = review["member"].split("::")
    cst = sha256(read_member(root, archive, member.encode("ascii"), b"CatScene", seed)).hexdigest()
    report.update(kcs_sha256=kcs, cst_sha256=cst)
    if (kcs, cst) != (KCS_SHA256, review["sha256"]):
        raise ValueError("startup KCS or first scene CST differs")
    if localized:
        translated = read_member(root, numbered[0], member.encode("ascii"), b"CatScene", seed)
        translated_digest = sha256(translated).hexdigest()
        require(translated_digest == OVERLAY_CST_SHA256, "the localized first scene differs")
        waits, _ = checkpoint_rows(expand_cst(translated)[0])
        require(waits and (waits[0]["group"], waits[0]["record"], waits[0]["kind"]) == (0, 94, "input"),
                "the localized first input marker differs")
        report.update(first_scene_candidate=f"{numbered[0]}::{member}", overlay_cst_sha256=translated_digest,
                      first_input_marker=[0, 94])


def check_entry(batch, report, *, localized=False, through_g9=False):
    result = json.loads((batch / "18.json").read_text(encoding="utf-8"))
    prefix = batch / "18/18"
    host = prefix.with_suffix(".host.log").read_text(encoding="utf-8")
    events = prefix.with_suffix(".events.log").read_text(encoding="utf-8")
    report["root"] = result["game"]
    require(result["index"] == 18 and result["mode"] == "state"
            and result["game"] == result["command"][1], "18 state-run identity differs")
    require("required startup INT registration failed" not in host, "required startup INT registration failed")
    root = Path(result["game"]).resolve()
    if localized:
        require(root == CN_ROOT.resolve(), "18 replay root is not the isolated localized copy")
    if localized or root == JP_ROOT.resolve():
        require(result.get("profile") == str(prefix.with_suffix(".profile").resolve()),
                "18 profile is not isolated in the batch")
    require(re.search(r"^frame=0 scene-entry KCS \d+ instructions$", events, re.M)
            and re.search(r"^frame=1 (?:logic-frame |input-wait |native-call |campanella-config-export )", events, re.M),
            "startup KCS or first logical frame missing")
    titles = re.findall(r"^frame (\d+): 18 verified title click region=11 receiver=7$", host, re.M)
    require(len(titles) == 1, "expected one guarded 18 title click")
    title = int(titles[0])
    presses = press_rows(host, title)
    require(presses.get(title) == [(0, 11, 7, 0)] and min(presses) == title,
            "unexpected title input receipt")
    require(re.findall(rf"^frame={title} message-resume message=(.*)$", events, re.M)
            == ["[2, 183, 434, 0]"], "title did not resume VM on the receipt frame")
    calls = native_frames(events)
    loads = [(frame, rows) for frame, rows in calls.items()
             for slot, _, detail in rows
             if slot == 1 and detail == ' debug="sscript loaded > 0001_00"']
    loaded, rows = loads[0] if len(loads) == 1 else (0, [])
    handles = [words[0] for slot, words, _ in rows if slot == 325 and len(words) == 2]
    require(title < loaded and len(handles) == 1,
            "expected a single native325 0001_00 load after title")
    # A gate can finish before emitting a terminal wait snapshot with the PE.
    pe = re.findall(r'pe_sha256=Some\("([0-9a-f]{64})"\)', host)
    require(not pe or set(pe) == {PE_SHA256}, "replay PE differs from the Campanella profile")
    settled = re.findall(r"^frame (\d+): 18 settled g3/100 after release body=", host, re.M)
    report.update(first_logic_frame=1, title_receipt_frame=title, first_scene="0001_00",
                  first_scene_loaded_frame=loaded, g3_r100_reported_frame=int(settled[0]) if len(settled) == 1 else None,
                  pe_logged=bool(pe))
    if localized and through_g9:
        check_localized_load(events, loaded, handle=handles[0])
    elif localized:
        check_localized_stop(result, host, events, calls, presses, title, loaded, handles[0], report)
    if through_g9:
        manifest = Path(__file__).with_name("pc_story_18_continuous_g9.json")
        review = json.loads(manifest.read_text(encoding="utf-8"))["review"]["cn" if localized else "jp"]
        archive, member = review["member"].split("::")
        raw = read_member(root, archive, member.encode("ascii"), b"CatScene", 0x4260E643)
        require(sha256(raw).hexdigest() == review["sha256"], "g9 route CST member differs")
        body, _ = expand_cst(raw)
        waits, _ = checkpoint_rows(body)
        rows = waits[:len(review["wait_records"])]
        require([row["record"] for row in rows] == review["wait_records"], "g9 input records differ")
        _, _, offsets, strings = unpack_from("<IIII", body)
        pages = []
        for group, row in enumerate(rows):
            require((row["group"], row["kind"]) == (group, "input"), "nonconsecutive g9 input records")
            start = unpack_from("<II", body, 16 + group * 8)[1]
            text = []
            for record in row["nonempty_body_records"]:
                pos = 16 + strings + unpack_from("<I", body, 16 + offsets + record * 4)[0] + 2
                if body[pos:body.index(0, pos)] not in (b"\\n", b"\n"):
                    text.append(record - start)
            pages.append({"wait": row["record"], "wait_ordinal": row["record"] - start,
                          "body_ordinals": text})
        verified = classify(18, result, host, events, {"pages": pages, "title_owner": 7,
                            "title_resume": "[2, 183, 434, 0]", "loads": ["0001_00"]})
        report.update(scene_source=review["member"], scene_sha256=review["sha256"],
                      verified_advances=verified["advances"])


def check_localized_load(events, loaded, handle):
    loads = re.findall(r"^frame=(\d+) campanella-cn-cst-load handle=(\d+) source=(\S+) sha256=([0-9a-f]{64})$",
                       events, re.M)
    member = f"{OVERLAY}::0001_00.cst"
    require(loads == [(str(loaded), str(handle), member, OVERLAY_CST_SHA256)],
            "localized overlay member was not loaded in the live CST context")
    return member


def check_localized_stop(result, host, events, calls, presses, title, loaded, handle, report):
    require(result.get("status") == "checkpoint_reached"
            and result.get("replay_status") == "completed"
            and result.get("exit_code") == 0 and result.get("timed_out") is False
            and result.get("first_engine_error") is None
            and events.rstrip().endswith("\nreplay_status=completed"), "localized entry checkpoint incomplete")
    require(presses == {title: [(0, 11, 7, 0)]}
            and re.findall(r"^frame (\d+): down (.*)$", host, re.M)
            == [(str(title - 1), "MouseButton(0)")], "extra physical input in localized entry route")
    member = check_localized_load(events, loaded, handle)
    stops = re.findall(r"^frame (\d+): 18 localized entry g0/94 handle=(\d+) loaded=(\d+) marked=(\d+)$", host, re.M)
    require(len(stops) == 1, "expected one settled localized entry")
    frame, stop_handle, stop_load, marked = map(int, stops[0])
    last_event_frame = result.get("last_frame")
    # The batch counts stdout frames, while the settled entry is logged on stderr.
    require((stop_handle, stop_load) == (handle, loaded) and isinstance(last_event_frame, int)
            and loaded < marked < frame and marked <= last_event_frame <= frame,
            "localized terminal frame or context differs")
    rows = calls.get(marked, [])
    executed = [(slot, words) for (slot, words, _), (following, _, detail) in zip(rows, rows[1:])
                if following == 1 and detail == ' debug="[input]"']
    require(executed == [(326, (handle, 0, 94))], "localized g0/r94 input marker did not execute")
    report.update(scene_source=member, scene_sha256=OVERLAY_CST_SHA256,
                  marker_frame=marked, settled_frame=frame, keyboard_presses=0)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    modes = parser.add_mutually_exclusive_group()
    modes.add_argument("--root", type=Path)
    modes.add_argument("--batch", type=Path, help="verify an existing state replay's entry receipts")
    parser.add_argument("--localized", action="store_true",
                        help="check the archived CN root or verify its localized entry batch")
    parser.add_argument("--through-g9", action="store_true", help="verify nine guarded body advances")
    parser.add_argument("--inventory-out", type=Path)
    args = parser.parse_args()
    if args.batch is not None and args.inventory_out is not None:
        parser.error("--inventory-out requires a static game root")
    if args.through_g9 and args.batch is None:
        parser.error("--through-g9 requires --batch")
    root = args.root or (CN_ROOT if args.localized else JP_ROOT)
    report = {"index": 18, "gate": "startup_inputs_unverified"}
    if args.batch is None:
        report["root"] = str(root)
    try:
        if args.batch is not None:
            report["gate"] = "startup_entry_unverified"
            check_entry(args.batch.resolve(strict=True), report, localized=args.localized, through_g9=args.through_g9)
            report["gate"] = "continuous_body_route_settled" if args.through_g9 else "startup_entry_verified"
        else:
            root = root.resolve(strict=True)
            check(root, report, localized=args.localized)
            if args.inventory_out is not None:
                games, _ = load_inventory(INVENTORY)
                inventory = {"games": [{"root": game["root"]} for game in games]}
                inventory["games"][18] = {"root": root.relative_to(ROOT).as_posix()}
                destination = args.inventory_out.resolve()
                with destination.open("x", encoding="utf-8") as output:
                    output.write(json.dumps(inventory, indent=2) + "\n")
                report["inventory"] = str(destination)
            report["gate"] = "startup_inputs_verified"
    except (OSError, ValueError, KeyError, ElementTree.ParseError, pefile.PEFormatError) as error:
        report["reason"] = str(error)
    print(json.dumps(report, indent=2))
    return int(report["gate"] not in ("startup_inputs_verified", "startup_entry_verified", "continuous_body_route_settled"))


if __name__ == "__main__":
    raise SystemExit(main())

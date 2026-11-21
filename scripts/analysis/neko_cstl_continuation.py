"""Generate and check guarded NEKOPARA scene 01 routes from an ordinary slot."""

import argparse
from collections import defaultdict
import hashlib
import json
from pathlib import Path
import re
import struct

from check_neko_cold_language import read_sources
from check_neko_save_roundtrip import BODY_PROBE, CONFIRM, OPEN, WRITE, check_roundtrip, ordinary_route, slot_identity
from cst_variants import expand_cst
from format_B_recovery import directory, read_entry
from check_neko_story_boundaries import DOWN, RESOURCES, branch_evidence, film_evidence, matches, story_evidence, unique
from stage_neko_cold_language import INVENTORY, ROOT


COLUMNS = ["cn", "en", "jp"]
JOIN_GROUP = 35
LAST_GROUP = 491
OPENING_RESTORE_RECORD = 3697
OPENING_WAIT_LIMIT = 14400
PRIMARY = "0x00062c6b"
CONTROL_ONLY = re.compile(rb"(?:\\[nr]|\\p[clr]?)*")
ORIGIN = re.compile(r'^frame=(\d+) neko-cst-origin handle=(\d+) load_id=(\d+) '
                    r'source="([\w.]+::[\w.]+)" sha256=([0-9a-f]{64})$')
POINTER = re.compile(r"^frame (\d+): pointer (\[-?\d+, -?\d+\]), routes (.*)$")
RECEIPT = re.compile(
    r"InputRouteReceipt \{ frame: (\d+), key: (\d+), group: 0, raw_mask: 2, pressed: 1, repeated: false, "
    r"region: (\d+), receiver: (\d+), response: (\d+), "
    r"physical: \[RoutedPhysicalButton \{ source: Mouse, code: 1 \}\] \}"
)
FIRST_CHAPTERS = ("h01", "01_02", "02", "03", "04", "05", "06", "07")
CHAPTER_JOINS = {("h01", "01_02"): ("01_01.cst", "01_02.cst"),
                 **{(scene, following): (f"{following}.cst",)
                    for scene, following in zip(FIRST_CHAPTERS[1:], FIRST_CHAPTERS[2:])}}


def digest(data):
    return hashlib.sha256(data).hexdigest()


def messages(data):
    offset = 8

    def integer():
        nonlocal offset
        result = 0
        while data[offset] == 255:
            result += 255
            offset += 1
        result += data[offset]
        offset += 1
        return result

    def field():
        nonlocal offset
        size = integer()
        value = data[offset:offset + size]
        if len(value) != size:
            raise ValueError("truncated CSTL field")
        offset += size
        return value

    columns = [field().decode("ascii") for _ in range(integer())]
    if columns != COLUMNS:
        raise ValueError("unexpected CSTL columns")
    rows = [[(field(), field()) for _ in columns] for _ in range(integer())]
    if offset != len(data):
        raise ValueError("unexpected CSTL tail")
    return rows


def scene_members(game, scene):
    archive_name, cst_sha256, cstl_sha256, _ = RESOURCES[f"{scene}.cst"]
    archive = game / "nekopara_vol3" / archive_name
    wanted = tuple(f"{scene}.{ext}".encode("ascii") for ext in ("cst", "cstl"))
    _, entries, cipher = directory(archive, 0x0f6bb0ae)
    with archive.open("rb") as stream:
        members = {entry["name"].lower(): read_entry(stream, entry, cipher) for entry in entries
                   if entry["name"].lower() in wanted}
    cst, cstl = (members[name] for name in wanted)
    if (digest(cst), digest(cstl)) != (cst_sha256, cstl_sha256):
        raise ValueError(f"unexpected {archive_name}::{scene} CST or CSTL digest")
    return cst, cstl


def resource_pages(game, language, target=None):
    cst, cstl = scene_members(game, "01")
    return guarded_pages(cst, cstl, "01", language, JOIN_GROUP, LAST_GROUP, 483, target=target)


def branch_chapters(game, language, page_count, *, through="01_02", last_pages=None):
    chapters = []
    for scene in FIRST_CHAPTERS[:FIRST_CHAPTERS.index(through) + 1]:
        _, _, _, (_, last_group, _) = RESOURCES[f"{scene}.cst"]
        if last_pages is None:
            count = page_count
        else:
            count = last_pages if scene == through else last_group + 1
        cst, cstl = scene_members(game, scene)
        pages = guarded_pages(cst, cstl, scene, language, 0, min(last_group, count - 1), -1)
        chapters.append({"scene": scene, "pages": pages})
    return chapters


def guarded_pages(cst, cstl, scene, language, first_group, last_group, prior_wait, *, target=None):
    cst, _ = expand_cst(cst)
    _, count, offsets, start = struct.unpack_from("<IIII", cst)
    groups = [struct.unpack_from("<II", cst, 16 + 8 * i) for i in range(count)]
    records = []
    for offset in range(offsets, start, 4):
        address = 16 + start + struct.unpack_from("<I", cst, 16 + offset)[0]
        record = cst[address:cst.index(0, address)]
        records.append((record[1], record[2:]))

    localized = messages(cstl)
    column = COLUMNS.index(language)
    pages = []
    for group in range(first_group, last_group + 1):
        size, first = groups[group]
        wait = first + size - 1
        if first != prior_wait + 1 or records[wait] != (0x02, b""):
            raise ValueError(f"non-adjacent ordinary wait {scene}/{group}/{wait}")
        rows = records[first:wait]
        if any(kind in (0x02, 0x03) for kind, _ in rows):
            raise ValueError(f"intervening input at {scene}/{group}/{wait}")
        fragments = localized[group][column][1].split(b"\x1f")
        text = []
        for ordinal, (kind, _) in enumerate(rows):
            if kind != 0x20:
                continue
            localized_text = fragments[len(text)] if len(text) < len(fragments) else b""
            localized_text = localized_text.split(b"\0", 1)[0]
            body = CONTROL_ONLY.fullmatch(localized_text) is None
            text.append({"ordinal": ordinal, "record": first + ordinal, "body": body,
                         "bytes": len(localized_text), "sha256": digest(localized_text)})
        if not any(row["body"] for row in text):
            raise ValueError(f"no compositable body at {scene}/{group}/{wait}")
        pages.append({"group": group, "wait": wait, "text": text})
        if wait == target:
            break
        prior_wait = wait
    if target is not None and (len(pages) < 2 or pages[-1]["wait"] != target):
        raise ValueError("target must be a supported ordinary wait after r485")
    return pages


def plan(game, language, startup, slot, target, resume_at=None, save_after=False):
    pages = resource_pages(game, language, target)
    if resume_at is not None and not any(page["wait"] == resume_at >= 547 for page in pages):
        raise ValueError("resume-at must be a guarded wait at or after r547 within the route")
    if save_after and (not 547 <= target < 5386 or resume_at == target):
        raise ValueError("checkpoint Save needs an incoming guarded edge before the branch")
    route = continuation_route(language, startup, slot_identity(slot)["sha256"], pages, resume_at=resume_at)
    if save_after:
        route["neko_ordinary"]["save_after"] = True
    return route


def continuation_route(language, startup, slot_sha256, pages, *, resume_at=None, resume_scene="01", chapters=None):
    target = pages[-1]["wait"]
    route = ordinary_route(language, "load", startup, slot_sha256, 485)
    route["label"] = f"NEKO {language} ordinary slot 0 to 01/{pages[-1]['group']}/{target}"
    route["neko_ordinary"].update(continue_to=target, cstl_pages=pages)
    if resume_at is not None:
        route["neko_ordinary"]["resume_at"] = resume_at
    if resume_scene != "01":
        route["neko_ordinary"]["resume_scene"] = resume_scene
    if chapters:
        route["neko_ordinary"].update(observe_branch=True, branch_chapters=chapters)
    if resume_scene == "01" and resume_at == OPENING_RESTORE_RECORD:
        route["idle_frames"] = max(route["idle_frames"], OPENING_WAIT_LIMIT)
    return route


def tagged(lines, name):
    prefix = re.compile(rf"^frame=(\d+) {name} ")
    return [(int(match[1]), json.loads(line[match.end():])) for line in lines if (match := prefix.match(line))]


def eyecatch_evidence(lines, lower, upper, old_owner, owner, downs):
    selected = [line for line in lines if (match := re.match(r"^frame=(\d+) ", line))
                and lower <= int(match[1]) < upper]
    (rebuilt,) = unique(matches(selected, rf'^frame=(\d+) task-create id={owner} kind=34 parent=\d+ name="meswnd" .*'),
                        "eyecatch successor window")
    (enter,) = unique(matches(selected, r'^frame=(\d+) native-call index=1040 .* debug="\[command\] fesjs eyecatch 2"'),
                      "eyecatch entry")
    (voice,) = unique(matches(selected, r'^frame=(\d+) fes-media .* sound_load channel=15 .*resource="pcm_b.int/B_eyecatch.ogg" .*'),
                      "eyecatch voice")
    (resumed,) = unique(matches(selected, r'^frame=(\d+) native-call index=1040 .* debug="\[command\] scenetitle .*'),
                        "eyecatch successor script")
    rebuilt, enter, voice, resumed = map(int, (rebuilt, enter, voice, resumed))
    if (owner == old_owner or not lower <= rebuilt <= enter <= voice < resumed < upper
            or any(lower <= down < upper for down in downs)):
        raise ValueError("eyecatch did not naturally hand over to the newly composed succeeding page")
    return {"rebuilt": rebuilt, "entered": enter, "voice_loaded": voice, "script_resumed": resumed, "owner": owner}


def verify(path, host, route):
    """Check logged runtime decisions without reading a save or invoking a player."""
    config = route["neko_ordinary"]
    pages = config["cstl_pages"]
    language = config["language"]
    observe_branch = config.get("observe_branch", False)
    chapters = {chapter["scene"]: chapter["pages"] for chapter in config.get("branch_chapters", [])}
    save_after = config.get("save_after", False)
    resume_at = config.get("resume_at")
    resume_scene = config.get("resume_scene", "01")
    if resume_scene != "01":
        if resume_scene not in chapters or not any(page["wait"] == resume_at for page in chapters[resume_scene]):
            raise ValueError("unknown checkpoint resume chapter or page")
        pages = []
    elif resume_at is not None:
        start = next((i for i, page in enumerate(pages) if page["wait"] == resume_at >= 547), None)
        if start is None:
            raise ValueError("unknown checkpoint resume page")
        pages = pages[start:]
    lines = path.read_text(encoding="utf-8-sig").splitlines()
    host_lines = host.read_text(encoding="utf-8-sig").splitlines()
    if not save_after and any(re.match(r"^frame=\d+ fes-datasave ", line) for line in lines):
        raise ValueError("read-only continuation wrote an ordinary save")
    loads, reads, _ = read_sources(lines)
    origins = {(int(match[1]), int(match[2]), int(match[3]), match[4], match[5])
               for line in lines if (match := ORIGIN.fullmatch(line))}
    observations = tagged(lines, "neko-cstl-page")
    pre_branch = len(pages)
    scenes = ["01"] * pre_branch
    follow_scene = observations[pre_branch][1].get("scene") if len(observations) > pre_branch else None
    if resume_scene != "01":
        ordered = list(chapters)
        for chapter in ordered[ordered.index(resume_scene):]:
            selected_pages = [page for page in chapters[chapter] if chapter != resume_scene or page["wait"] >= resume_at]
            pages.extend(selected_pages)
            scenes.extend([chapter] * len(selected_pages))
    elif follow_scene is not None:
        if follow_scene not in ("h01", "01_02") or follow_scene not in chapters:
            raise ValueError(f"unexpected guarded branch chapter {follow_scene}")
        selected = [follow_scene]
        if "02" in chapters:
            ordered = list(chapters)
            selected = ordered[ordered.index(follow_scene):]
        pages = [*pages]
        for chapter in selected:
            pages.extend(chapters[chapter])
            scenes.extend([chapter] * len(chapters[chapter]))
    seen = []
    for index, (frame, observed) in enumerate(observations):
        if index >= len(pages):
            raise ValueError("extra guarded page")
        expected = pages[index]
        group, wait = expected["group"], expected["wait"]
        scene = scenes[index]
        tag = f"{scene}/{group}/{wait}"
        if (observed["group"], observed["record"], observed["sources"]) != (
                group, wait, [row["record"] for row in expected["text"] if row["body"]]) or observed.get("scene", "01") != scene:
            raise ValueError(f"{tag}: wrong page or body contributors")
        context, load = observed["context"], observed["load_id"]
        loaded = observed["loaded"]
        archive, cst_digest, table_digest, _ = RESOURCES[f"{scene}.cst"]
        table = f"{archive}::{scene}.cstl"
        if ((loaded, context, load, f"{archive}::{scene}.cst", cst_digest) not in origins
                or not any((ctx, name.lower(), sha, columns) == (context, table, table_digest, COLUMNS)
                           for ctx, name, sha, columns in loads[loaded])):
            raise ValueError(f"{tag}: missing matching CST/CSTL load")
        if not (loaded <= observed["earliest"] <= observed["source_frame"] <= observed["complete"] <= frame
                and observed["source_frame"] <= observed["marked"] < frame):
            raise ValueError(f"{tag}: wrong generation timing")
        if seen:
            new_chapter = scenes[index - 1] != scene
            same_load = (context, load) == (seen[-1]["context"], seen[-1]["load_id"])
            if same_load == new_chapter:
                raise ValueError(f"{tag}: unexpected CST context or load lifetime")
            if not new_chapter and not (scene == "01" and group in (287, 389)) and (
                    observed["owner"] != seen[-1]["owner"] or observed["epoch"] == seen[-1]["epoch"]):
                raise ValueError(f"{tag}: window changed or generation reused")
        for row in expected["text"]:
            candidates = [read for read in reads[(context, group, row["ordinal"])]
                          if read["kind"] == "0x20" and read["destination"] == PRIMARY
                          and observed["earliest"] <= read["frame"] <= observed["marked"]]
            expected_read = (language, language, "cstl", COLUMNS.index(language), row["bytes"], row["sha256"])
            keys = ("main", "requested", "source", "column", "bytes", "sha256")
            if not candidates or any(tuple(read[k] for k in keys) != expected_read for read in candidates):
                raise ValueError(f"{tag}: wrong or missing CSTL read r{row['record']}")
            if row["body"] and not any(read["frame"] <= observed["source_frame"] for read in candidates):
                raise ValueError(f"{tag}: body missing before generation")
        seen.append({"frame": frame, **observed})
    end = pages[-1]
    success = [line for line in lines if line.startswith("replay_status=")] == ["replay_status=completed"]
    report = {"status": "insufficient", "last_page": seen[-1] if seen else None,
              "verified_pages": len(seen), "expected_pages": len(pages),
              "last_checkpoint": next((line for line in reversed(lines) if line.startswith("replay frame=")), None),
              "replay_status": [line.split("=", 1)[1] for line in lines if line.startswith("replay_status=")]}
    if not success:
        return report
    if len(seen) != len(pages):
        raise ValueError("completed route omitted guarded pages")
    if chapters and follow_scene is None:
        raise ValueError("completed route omitted guarded post-branch pages")
    decisions = defaultdict(list)
    for frame, decision in tagged(lines, "neko-body-target"):
        decisions[frame].append(decision)
    consumers = defaultdict(list)
    for frame, receipt in tagged(lines, "neko-cstl-consumed"):
        consumers[receipt["started"]].append((frame, receipt))
    body_presses = [int(match[1]) for line in host_lines
                    if (match := re.match(r"^frame (\d+): down MouseButton\(0\)", line))]
    physical = {}
    for line in host_lines:
        if match := POINTER.fullmatch(line):
            point = tuple(json.loads(match[2]))
            physical.setdefault((int(match[1]), point), []).extend(
                tuple(map(int, receipt)) for receipt in RECEIPT.findall(match[3]))
    resumes = {}
    for line in lines:
        if match := re.fullmatch(r"frame=(\d+) message-resume (.*)", line):
            frame = int(match[1])
            resumes.setdefault(frame, []).append(match[2])
    slot = route["neko_ordinary"]["slot_sha256"]
    checkpoints = defaultdict(list)
    confirms = []
    for line in lines:
        if match := re.fullmatch(r"replay frame=(\d+) checkpoint=(.*)", line):
            frame, label = int(match[1]), json.loads(match[2])
            checkpoints[frame].append(label)
            if label == CONFIRM:
                confirms.append(frame)
    reopen = [match for line in lines if (match := OPEN.fullmatch(line))
              and len(confirms) == 1 and confirms[0] < int(match[1])]
    if len(reopen) != 1 or reopen[0][3] != slot or int(reopen[0][1]) > seen[0]["frame"]:
        raise ValueError("missing exact ordinary slot open")
    if resume_at is not None and seen[0]["earliest"] != max(int(reopen[0][1]), seen[0]["loaded"]):
        raise ValueError("first saved-page proof was not bound to the ordinary reopen")
    if resume_scene == "01" and resume_at == OPENING_RESTORE_RECORD:
        upper = min(seen[0]["marked"], seen[0]["complete"]) - 1
        downs = [int(match[1]) for line in host_lines if (match := DOWN.fullmatch(line))]
        report["opening_replayed_on_load"] = film_evidence(lines, "op", seen[0]["earliest"], upper, downs)
    probes = {int(match[1]): match[2] for line in host_lines if (match := BODY_PROBE.fullmatch(line))}
    branch = branch_evidence(lines, host_lines, continues=bool(follow_scene)) if observe_branch and resume_scene == "01" else None
    if follow_scene and branch is not None and branch["successor"][1].lower() != follow_scene:
        raise ValueError("guarded post-branch chapter did not match the branch flags")
    joins = []
    stories, boundary_loads = (story_evidence(lines, host_lines)[1:] if "02" in chapters else ([], []))
    for index, page in enumerate(seen):
        frame = page["frame"]
        matches = [row for row in decisions.get(frame, [])
                   if (row["group"], row["record"], row["owner"]) ==
                   (page["group"], page["record"], page["owner"])]
        if len(matches) != 1:
            raise ValueError(f"f{frame}: missing unique live target")
        target = matches[0]
        if target["point"] != [625, 478] or target["control"] != "meswnd low button" or frame not in probes:
            raise ValueError(f"f{frame}: missing live body probe")
        last = index == len(seen) - 1
        branch_turn = branch is not None and index == pre_branch - 1
        terminal = last and not branch_turn
        scene = page.get("scene", "01")
        if terminal:
            label = f"Save at {scene}/{end['group']}/{end['wait']}" if save_after else (
                f"Stop at {scene}/{end['group']}/{end['wait']} without input")
        else:
            label = f"Turn at {scene}/{page['group']}/{page['record']}"
        expected_labels = [label]
        if terminal and save_after:
            expected_labels.append("Open NEKO checkpoint Save")
        if target["label"] != label or checkpoints.get(frame) != expected_labels:
            raise ValueError(f"f{frame}: unexpected checkpoint")
        if terminal:
            if not save_after and any(down >= frame for down in body_presses):
                raise ValueError("input at or beyond terminal")
            break
        transitions = consumers.get(frame, [])
        if body_presses.count(frame) != 1 or len(transitions) != 1:
            raise ValueError(f"f{frame}: wrong press or consumption count")
        verified_frame, consumed = transitions[0]
        press = consumed["press_frame"]
        successor_frame = branch["successor"][0] if branch_turn else seen[index + 1]["frame"]
        chapter_turn = branch_turn or scene != scenes[index + 1]
        if chapter_turn and not branch_turn:
            successor = seen[index + 1]
            name = scenes[index + 1]
            first = [(f, member) for f, member in boundary_loads if press <= f <= successor_frame]
            wait = ("page", name, successor["group"], successor["record"])
            if (tuple(member for _, member in first) != CHAPTER_JOINS.get((scene, name))
                    or not any(first[-1][0] <= f <= successor_frame
                               and (state, actual.lower(), group, record) == wait
                               for f, state, actual, group, record in stories)):
                raise ValueError(f"{scene} to {name}: unexpected chapter load or first wait")
            joins.append({"from": scene, "to": name, "press": press, "loads": first, "first_page": successor_frame})
        if scene == "01" and page["record"] == 3631:
            next_page = seen[index + 1]
            upper = min(next_page["marked"], next_page["complete"]) - 1
            report["opening"] = film_evidence(lines, "op", press, upper, body_presses)
        if scene == "01" and page["record"] == 4575:
            next_page = seen[index + 1]
            report["eyecatch2"] = eyecatch_evidence(lines, press, next_page["frame"], page["owner"],
                                                   next_page["owner"], body_presses)
        receipts = physical.get((press, (625, 478)), [])
        low = (press, 0, consumed["region"], consumed["owner"], 1)
        root = (press, 2, consumed["root_region"], consumed["root_owner"], 0)
        if (not frame < press <= frame + 3 < verified_frame
                or not (successor_frame <= verified_frame if branch_turn else verified_frame == successor_frame)
                or (not last and seen[index + 1]["earliest"] != (max(press, seen[index + 1]["loaded"]) if chapter_turn else press))
                or (branch_turn and (branch["press"] != press or verified_frame != branch["terminal"]))
                or resumes.get(press) != ["message=[2, 625, 478, 0]"]
                or receipts.count(low) != 1 or [r for r in receipts if r[-1] == 0] != [root]
                or consumed["released"] is not True or consumed["region"] != target["region"]
                or consumed["owner"] != page["owner"]
                or consumed["root_region"] != target["hit_region"]
                or [consumed["root_region"], consumed["root_owner"]] not in target["receivers"]):
            raise ValueError(f"f{frame}: invalid body/root consumption or release")
    turns = seen if branch and not follow_scene else seen[:-1]
    if sum(map(len, consumers.values())) != len(turns):
        raise ValueError("wrong total of guarded edges")
    expected_presses = [page["frame"] for page in turns]
    if save_after:
        names = ["Open NEKO checkpoint Save", "Write ordinary slot 0", "Confirm NEKO checkpoint overwrite"]
        controls = [(frame, row) for frame, row in tagged(lines, "neko-save-control") if frame >= seen[-1]["frame"]]
        written = tagged(lines, "neko-checkpoint-written")
        writes = [WRITE.fullmatch(line) for line in lines if " fes-datasave " in line]
        if [row["label"] for _, row in controls] != names or len(written) != 1 or len(writes) != 1 or not writes[0]:
            raise ValueError("checkpoint Save must prove toolbar, occupied slot, confirm and unique write")
        last = seen[-1]
        scene = last.get("scene", "01")
        retained = [scene, last["group"], last["record"], last["context"], last["load_id"], last["owner"]]
        control_frames = [frame for frame, _ in controls]
        saved_frame, saved = written[0]
        write_frame = int(writes[0][1])
        if (not last["frame"] == control_frames[0] < control_frames[1] < control_frames[2] < write_frame <= saved_frame
                or any(row["retained"] != retained for _, row in controls)
                or controls[1][1]["slot_exists"] is not True
                or controls[2][1]["slot"] != 0):
            raise ValueError("checkpoint Save changed its retained page, slot or ordering")
        for frame, control in controls:
            receipts = [receipt for current in range(frame + 1, frame + 4)
                        for receipt in physical.get((current, tuple(control["point"])), [])
                        if receipt[1:] == (0, control["region"], control["owner"], control["response"])]
            if len(receipts) != 1:
                raise ValueError("checkpoint Save missing unique physical control receipt")
        if ((saved["scene"], saved["group"], saved["record"], saved["slot"], saved["language"]) !=
                (scene, last["group"], last["record"], 0, language)
                or saved["write_frame"] != write_frame or saved["previous_sha256"] != slot
                or saved["sha256"] == slot or not re.fullmatch(r"[a-f0-9]{64}", saved["sha256"])):
            raise ValueError("checkpoint Save digest or position did not match")
        expected_presses += control_frames
        report["checkpoint_written"] = saved
    if [down for down in body_presses if down >= seen[0]["frame"]] != expected_presses:
        raise ValueError("unexpected input within guarded segment")
    if branch:
        report["branch"] = branch
    if joins:
        report["chapter_joins"] = joins
    report.update(status="pass", guarded_edges=len(turns))
    return report


def check_continuation_roundtrip(receipt, route, *, checkpoint=False):
    config = route["neko_ordinary"]
    process = json.loads(receipt.read_text(encoding="utf-8-sig"))
    report = check_roundtrip(receipt, 485, False, continuing=True, checkpoint=checkpoint)
    if (config["language"] != process["language"] or config["slot_sha256"] != report["slot"]["sha256"]):
        report["failures"].append("continuation route language and slot must match the roundtrip receipt")
    try:
        result = verify(receipt.parent / process["consumer"]["events"],
                        receipt.parent / process["consumer"]["host"], route)
    except ValueError as exc:
        report["failures"].append(str(exc))
    else:
        report["continuation"] = result
        if result["status"] != "pass":
            report["missing"].append("completed continuous CSTL route")
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="operation", required=True)
    routes = sub.add_parser("route")
    routes.add_argument("--language", choices=("jp", "cn"), required=True)
    routes.add_argument("--startup", choices=("title", "language-select"), default="title")
    routes.add_argument("--slot", type=Path, required=True)
    routes.add_argument("--to", type=int, required=True)
    routes.add_argument("--save-after", action="store_true", help="overwrite only a private copy of ordinary slot 0")
    routes.add_argument("--resume-at", type=int, help="guard the saved scene 01 wait before any body input")
    checks = sub.add_parser("check")
    checks.add_argument("--route", type=Path, required=True)
    checks.add_argument("--events", type=Path, required=True)
    checks.add_argument("--host", type=Path, required=True)
    checks.add_argument("--slot", type=Path, help="verify the final on-disk ordinary slot identity")
    roundtrip = sub.add_parser("roundtrip")
    roundtrip.add_argument("--route", type=Path, required=True)
    roundtrip.add_argument("--receipt", type=Path, required=True)
    roundtrip.add_argument("--checkpoint", action="store_true", help="verify an archived-slot candidate reopen")
    args = parser.parse_args()
    game = ROOT / json.loads(INVENTORY.read_text(encoding="utf-8"))["games"][1]["root"]
    try:
        if args.operation == "route":
            result = plan(game, args.language, args.startup, args.slot, args.to, args.resume_at, args.save_after)
        else:
            route = json.loads(args.route.read_text(encoding="utf-8-sig"))
            config = route["neko_ordinary"]
            if config["cstl_pages"] != resource_pages(game, config["language"], config["continue_to"]):
                raise ValueError("route does not match the pinned CST/CSTL")
            if args.operation == "roundtrip":
                result = check_continuation_roundtrip(args.receipt, route, checkpoint=args.checkpoint)
                result["status"] = "fail" if result["failures"] else "insufficient" if result["missing"] else "pass"
            else:
                result = verify(args.events, args.host, route)
                if result["status"] == "pass" and (config.get("save_after") or config.get("resume_at") is not None):
                    if args.slot is None:
                        raise ValueError("checkpoint Save/Load check requires --slot")
                    observed = result.get("checkpoint_written", config)
                    expected = observed.get("sha256", config["slot_sha256"])
                    identity = slot_identity(args.slot)
                    if identity["sha256"] != expected:
                        raise ValueError("final ordinary CSS differs from the checkpoint receipt")
                    result["slot"] = identity
    except (OSError, ValueError, KeyError, IndexError, TypeError, struct.error) as exc:
        parser.exit(1, f"{exc}\n")
    status = {"fail": 1, "insufficient": 2}.get(result.get("status"), 0)
    if getattr(args, "checkpoint", False):
        result["status"] = f"checkpoint_{result['status']}"
    print(json.dumps(result, indent=2))
    return status


if __name__ == "__main__":
    raise SystemExit(main())

"""Derive the 08/14/18 route plans from the original CST records."""

import argparse
import json
from pathlib import Path
import re
from struct import unpack_from

from cst_variants import digest, expand_cst, grisaia_root, read_entries
from plan_island_bad02 import checkpoint_rows
from run_playthrough_batch import INVENTORY, load_inventory


HERE = Path(__file__).resolve().parent
CASES = {
    8: ("c01_01", (), 2, 5, 600),
    14: ("OP", (("UPDATE01.INT", 0), ("scene.int", 0)), 5, 8, 900),
    18: ("0001_00", (("scene.int", 1113646659),), 6, 9, 1400),
}
MANIFEST = HERE / "pc_story_08_14_18_next_segment.json"
ARCHIVED_PATCH = "archive/2026-11/20261120_pc_story_08_14_18_cst_segment.patch"
EYE_RENDER_PATCH = "archive/2026-11/20261121_pc_story_08_wipe_draw_abi.patch"
TRANSFER = re.compile(rb"(?:(if\s*\(.*\))\s*)?(next|jump|call|return)\b(?:\s+(\w+))?", re.I)
OPTION = re.compile(rb"(\d+)\s+([a-zA-Z0-9_]+)\s+(.+)")


class SceneSource:
    def __init__(self, index):
        self.root = grisaia_root() if index == 14 else Path(load_inventory(INVENTORY)[0][index]["root"])
        self.archives = {}
        for archive, seed in CASES[index][1]:
            self.archives[archive] = {
                entry["name"].lower(): packed for entry, _, packed in read_entries(self.root / archive, seed)
            }

    def read(self, scene):
        member = scene.lower() + ".cst"
        if not self.archives:
            location = "scene/" + member
            return location, (self.root / location).read_bytes()
        for archive, entries in self.archives.items():
            if member.encode("ascii") in entries:
                return f"{archive}::{member}", entries[member.encode("ascii")]
        raise ValueError(f"missing CST {scene}")


def read_scene(source, scene):
    member, packed = source.read(scene)
    data, _ = expand_cst(packed)
    waits, _ = checkpoint_rows(data)
    _, groups, offsets, strings = unpack_from("<IIII", data)
    records = {}
    starts = []
    for group in range(groups):
        count, start = unpack_from("<II", data, 16 + 8 * group)
        starts.append(start)
        for record in range(start, start + count):
            offset, = unpack_from("<I", data, 16 + offsets + 4 * record)
            pos = 16 + strings + offset
            records[record] = group, data[pos + 1], data[pos + 2:data.index(0, pos + 2)]

    boundaries = []
    for record, (group, kind, raw) in records.items():
        if kind != 0x30:
            continue
        raw = raw.strip()
        words = raw.lower().split()
        if not words:
            continue
        transfer = TRANSFER.fullmatch(raw)
        if transfer:
            _, verb, target = transfer.groups()
            boundaries.append({"kind": verb.decode("ascii").lower(), "group": group, "record": record,
                               "command": raw.decode("ascii"), "conditional": transfer[1] is not None,
                               "target": target.decode("ascii") if target else None})
        elif words[0] in (b"select", b"fselect"):
            options = []
            for at in range(record + 1, len(records)):
                option_group, option_kind, value = records[at]
                row = OPTION.fullmatch(value.strip())
                if option_group != group or option_kind != 0x30 or row is None:
                    break
                options.append({"record": at, "id": int(row[1]), "target": row[2].decode("ascii"),
                                "label_hex": row[3].hex()})
            if not options:
                raise ValueError(f"{scene} r{record}: unrecognized choice rows")
            boundaries.append({"kind": "choice", "group": group, "record": record,
                               "command": raw.decode("ascii"), "options": options})
        elif (words[0] in (b"auto", b"fesjs", b"movie", b"staffroll", b"title", b"end_of_kaisou")
              or words == [b"wait", b"key"]):
            boundaries.append({"kind": words[0].decode("ascii"), "group": group, "record": record,
                               "command": raw.decode("ascii")})
        elif re.search(rb"\b(?:next|jump|call|return|fselect|select)\b", raw, re.I):
            raise ValueError(f"{scene} r{record}: unrecognized control flow {raw!r}")

    pages = []
    for wait in waits:
        group, record = wait["group"], wait["record"]
        body = [r for r in wait["nonempty_body_records"] if records[r][2] != b"\\n"]
        newline = [r for r in wait["nonempty_body_records"] if records[r][2] == b"\\n"]
        pages.append({"group": group, "kind": wait["kind"], "wait": record,
                      "wait_ordinal": record - starts[group], "body": body,
                      "body_ordinals": [r - starts[group] for r in body],
                      "body_sizes": [len(records[r][2]) for r in body],
                      "name_records": [r for r in range(starts[group], record) if records[r][1] == 0x21],
                      **({"newline_only": newline} if newline else {})})
    return {"scene": scene, "member": member, "sha256": digest(packed), "pages": pages,
            "boundaries": boundaries}


def scan(index):
    source = SceneSource(index)
    scene = CASES[index][0]
    scenes = {}
    while scene.lower() not in scenes:
        item = read_scene(source, scene)
        scenes[scene.lower()] = item
        controls = item["boundaries"]
        if any(row["kind"] in ("choice", "jump", "call", "return", "title") for row in controls):
            break
        targets = [row["target"] for row in controls if row["kind"] == "next" and not row["conditional"]]
        if len(targets) != 1:
            raise ValueError(f"{scene}: expected one unconditional continuation, found {targets}")
        scene = targets[0]
    if index == 8:
        # The recollection exit is an alternate continuation of the two #008 branches.
        alternate = read_scene(source, "rc_end")
        if alternate["pages"] or [row["kind"] for row in alternate["boundaries"]] != ["end_of_kaisou"]:
            raise ValueError("08: alternate recollection exit differs")
        scenes["rc_end"] = alternate
    return scenes


def preceding_page(scene, boundary):
    return next((page for page in reversed(scene["pages"]) if page["wait"] < boundary["record"]), None)


def summarize(scenes):
    report = []
    for scene in scenes.values():
        boundaries = []
        for row in scene["boundaries"]:
            page = preceding_page(scene, row)
            boundaries.append(row | {"preceding_wait": [page["group"], page["wait"]] if page else None})
        report.append({k: scene[k] for k in ("scene", "member", "sha256")} |
                      {"body_wait_count": len(scene["pages"]), "first_wait": scene["pages"][:1],
                       "last_wait": scene["pages"][-1:], "boundaries": boundaries})
    return report


def scenario(index, opening):
    _, _, prior, last, window = CASES[index]
    base = HERE / f"pc_story_{index:02}_continuous_g{prior}.json"
    previous = json.loads(base.read_text(encoding="utf-8"))
    original = previous["review"]
    expected_hash = original.get("sha256", "942ca2dbd98382b7196ba97f8b38709d672ce5d9c97d62068a9e3b423dafdee2")
    if opening["sha256"] != expected_hash:
        raise ValueError(f"{index}: opening CST differs from the runtime pin")
    pages = opening["pages"][:last + 1]
    if (len(pages) != last + 1 or any(page["kind"] != "input" or page["group"] != group
                                     or not page["body"] for group, page in enumerate(pages))
            or any(row["record"] <= pages[-1]["wait"] for row in opening["boundaries"])):
        raise ValueError(f"{index}: supported opening segment differs")
    if index == 8:
        if (any(len(page["body"]) != 1 for page in pages)
                or [page["wait"] for page in pages[:prior + 1]] != original["waits"]
                or [page["body"][0] for page in pages[:prior + 1]] != original["body_records"]):
            raise ValueError("08: ordinary body shape differs")
    else:
        for verified, parsed in zip(original["pages"], pages[:prior + 1], strict=True):
            if any(parsed[key] != value for key, value in verified.items()):
                raise ValueError(f"{index}: existing page evidence differs")
        if index == 14 and any(len(page["body"]) != 1 for page in pages):
            raise ValueError("14: FES body shape differs")
    entry = previous[str(index)]
    entry["frames"] += (last - prior) * window
    if index == 8:
        entry["diagnostics"].extend(f"trace-native:{native}" for native in (520, 527, 528, 530, 531))
    entry["route"].update(label=f"{index:02} {opening['scene']}: {last} guarded Enter presses through g{last}/{pages[-1]['wait']}",
                          guarded_body_last_group=last)
    return entry, {"member": opening["member"], "sha256": expected_hash, "pages": pages,
                   "previous_group": prior, "readiness_frames_per_added_page": window,
                   **{key: original[key] for key in ("title_owner", "title_resume", "loads") if key in original}}


def build(indices):
    manifest = {"review": {"runtime_increment": ARCHIVED_PATCH,
                           "08_prerequisite": "archive/2026-11/20261120_pc_story_08_eye_sleep.patch",
                           "08_render_increment": EYE_RENDER_PATCH,
                           "14_prerequisite": "archive/2026-11/20261120_pc_story_14_op_g5_marker_order.patch",
                           "14_g6_increment": "archive/2026-11/20261121_pc_story_14_op_g6_marker_order.patch",
                           "14_g6_g7_input_increment": "archive/2026-11/20261121_pc_story_14_op_g6_g7_enter_binding.patch",
                           "18_prerequisite": "archive/2026-11/20261120_pc_story_18_rdraw527.patch",
                           "routes": {}}}
    scanned = {}
    for index in indices:
        scenes = scan(index)
        key = str(index)
        manifest[key], manifest["review"]["routes"][key] = scenario(index, next(iter(scenes.values())))
        scanned[index] = scenes
    return manifest, scanned


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--indices", nargs="+", type=int, choices=CASES, default=list(CASES))
    modes = parser.add_mutually_exclusive_group()
    modes.add_argument("--write-scenario", action="store_true")
    modes.add_argument("--pages", metavar="SCENE")
    args = parser.parse_args()
    manifest, scanned = build(args.indices)
    if args.pages and any(args.pages.lower() not in scenes for scenes in scanned.values()):
        parser.error("--pages requires a scene on each selected path, select one game with --indices")
    if args.write_scenario:
        if set(args.indices) != CASES.keys():
            parser.error("--write-scenario requires all three indices")
        MANIFEST.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    else:
        report = {index: scenes[args.pages.lower()] if args.pages else summarize(scenes)
                  for index, scenes in scanned.items()}
        print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()

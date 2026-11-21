"""Check the 08 continuation and original Save entry resources offline."""

import argparse
from hashlib import sha256
import json
from pathlib import Path
import re
from struct import unpack_from
import zlib

import check_pc_story_08_eyecatch as eye08
from check_pc_story_08_14_18_segment import log_files, require
from check_pc_story_18_startup import read_member
from cst_variants import expand_cst
from plan_pc_story_08_14_18 import MANIFEST, SceneSource, read_scene


SCENARIO = MANIFEST.with_name("pc_story_08_c01_01_g27.json")
SEED = 0x803D26A6


def check_save(root):
    kcs = read_member(root, "kcs.int", b"meswnd.kcs", b"KCS\0", SEED)
    require(sha256(kcs).hexdigest() == "520b24957f94f5ec86b2226001cbf8d4b61c18f5758e0c0f8c4e9171702bd5bb",
            "meswnd KCS differs from the reviewed Save producer")
    code = zlib.decompress(kcs[52:])
    require(unpack_from("<BBIBI", code, 0x5472) == (0x34, 3, 6, 3, 0x55E9)
            and code[0x55F4:0x5611] == bytes.fromhex(
                "25030300ffff 250300000000 250313000000 2b03460000000310000000"),
            "mes btn 6 does not dispatch the reviewed Save message")
    raw = read_member(root, "fes.int", b"saveload.fes", b"FES\0", SEED)
    require(sha256(raw).hexdigest() == "4554ea15abb1c4d41525f5a017359d95b502ab88db0909c1d20313ee1bee36a8",
            "saveload FES differs from the reviewed panel")
    panel = zlib.decompress(raw[16:]).replace(b"\r\n", b"\n")
    require(b"#WAIT\n" in panel
            and b"#data_btn.PUSH_L\nif ($802==1&&$800==0&&@<3)\nbreak\n" in panel
            and b"$865 = ($802-1)*6+@\n" in panel
            and b"#SAVEDATA\ndatasave $865 _SAVEOBJ_\n" in panel,
            "ordinary Save wait, slot mapping or write branch differs")
    return {"panel": "saveload/WAIT", "mode": 0, "page": 1,
            "first_ordinary_slot": 3, "button": "data_btn[3]", "save_message": 19}


def check_static():
    source = SceneSource(8)
    scene = read_scene(source, "c01_01")
    manifest = json.loads(SCENARIO.read_text(encoding="utf-8"))
    prior = json.loads(MANIFEST.read_text(encoding="utf-8"))["8"]
    waits = [144, 148, 150, 166, 170, 174, 180, *range(184, 265, 4)]
    pages = scene["pages"][:len(waits)]
    require(scene["sha256"] == manifest["review"]["sha256"]
            and [(page["group"], page["wait"], page["body"], page["kind"]) for page in pages]
            == [(group, wait, [wait - 1], "input") for group, wait in enumerate(waits)],
            "08 continuation differs from the pinned single-record waits")
    expected = prior | {"frames": prior["frames"] + 22 * 600, "route": prior["route"] | {
        "label": "08 c01_01: 27 guarded Enter presses through g27/264", "guarded_body_last_group": 27}}
    require(manifest["8"] == expected, "08 scenario differs from the reviewed g5 continuation")

    data, _ = expand_cst(source.read("c01_01")[1])
    _, _, offsets, strings = unpack_from("<IIII", data)
    for record, command in ((175, b"eg 2 fec01,1,1"), (176, b"rdraw 30"), (270, b"initialize_header")):
        offset = 16 + strings + unpack_from("<I", data, 16 + offsets + record * 4)[0]
        require(data[offset + 1:offset + 3 + len(command)] == b"\x30" + command + b"\0",
                f"08 CST command at r{record} differs")
    require(scene["pages"][28]["wait"] == 402 and [(row["record"], row["target"])
            for row in scene["boundaries"]] == [(508, "c01_02")], "08 scene boundary differs")
    return pages, check_save(source.root)


def check_batch(batch, pages, last_group):
    result, host, events = log_files(batch, 8)
    expected = [(page["group"], page["wait"], page["wait_ordinal"], page["body"][0], page["body_sizes"][0])
                for page in pages[:last_group + 1]]
    report = eye08.classify(result, host, events, last_group, expected)
    require(report["gate"] == "continuous_body_route_settled", report.get("reason", report["gate"]))
    if last_group >= 6:
        proofs = [tuple(map(int, row)) for row in eye08.ORDINARY_BODY.findall(events)]
        armed, settled, script = proofs[5][0], proofs[6][0], proofs[6][7]
        draw_tasks = [int(task) for at, task, kind, parent, name in eye08.TASK.findall(events)
                      if armed < int(at) <= settled and (int(kind), int(parent), name) == (38, script, "rdraw")]
        require(len(draw_tasks) == 1, "expected one new script-owned rdraw for g6")
        draw = draw_tasks[0]
        pair = [(int(at), int(index), int(scalar)) for at, index, scalar in re.findall(
            rf"^frame=(\d+) 08-render-native index=(527|528) task={draw} scalar=(\d+) ", events, re.M)
                if armed < int(at) <= settled and (index, scalar) in (("527", "0"), ("528", "1"))]
        require([(index, scalar) for _, index, scalar in pair] == [(527, 0), (528, 1)]
                and pair[0][0] <= pair[1][0], "g6 rdraw configure/start receipt differs")
        report["g6_rdraw"] = {"task": draw, "configured": pair[0][0], "started": pair[1][0]}
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("batch", type=Path, nargs="?")
    parser.add_argument("--last-group", type=int, choices=(5, 6, 27), default=27)
    args = parser.parse_args()
    try:
        pages, save = check_static()
        report = {"gate": "static_verified", "waits": [page["wait"] for page in pages], "save": save}
        if args.batch is not None:
            report["replay"] = check_batch(args.batch, pages, args.last_group)
    except (OSError, ValueError) as error:
        print(json.dumps({"gate": "unproven", "reason": str(error)}))
        return 1
    print(json.dumps(report, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

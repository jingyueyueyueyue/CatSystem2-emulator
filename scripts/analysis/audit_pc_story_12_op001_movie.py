"""Read the Hatsumira source route from r195 through the first post-OP pages."""

import argparse
import hashlib
import json
from pathlib import Path

from audit_runtime_evidence import runtime_fingerprint
from check_pc_story_11_13_sources import archive_member, kif_entries, read_cst, require
from check_pc_story_12_op001 import PE, RW_MOSAIC_WAITS


SCENES = {
    "op001": "9911a9f8daecd80062fe4fcdf3f438845b5ecafdf55c1292a97ce18cd0233087",
    "OPMOVIE": "8211076eda0c69dd6ec4df5b57fc2e89bec8327fb20682c62771c2b4a43f4f78",
    "OPMOVIE_END": "6fc067e685d21ec1e19d59e62c040d7d3364f6f4a1b3d987d7b845b038ac3bbb",
}
SSCRIPT = "1932f5e6f308b5e4676c6b2437d98e191bfaebcb9a8f50f64d78bffb1feccab5"


def commands(records, start, end):
    return [{"record": i, "command": data.decode("ascii").strip()}
            for i, (_, _, kind, data) in enumerate(records[start:end], start) if kind == 0x30]


def pages(records, start, end):
    result = []
    for record in range(start, end + 1):
        group, _, kind, data = records[record]
        if kind not in (0x02, 0x03):
            continue
        require(kind == 0x02 and not data, f"unexpected input boundary at r{record}")
        segment = [(i, k, raw) for i, (actual, _, k, raw) in enumerate(records[start:record], start)
                   if actual == group and k in (0x20, 0x21)]
        bodies = [(i, raw) for i, k, raw in segment if k == 0x20 and raw]
        require(len(bodies) == 1, f"expected one nonempty body at g{group}/r{record}")
        body, raw = bodies[0]
        raw.decode("cp932")
        result.append({"group": group, "wait_record": record, "body_record": body, "body_bytes": len(raw),
                       "body_sha256": hashlib.sha256(raw).hexdigest(),
                       "speakers": [i for i, k, raw in segment if k == 0x21 and raw],
                       "empty_text_records": [i for i, _, raw in segment if not raw],
                       "commands": commands(records, start, record)})
        start = record + 1
    return result


def audit(game):
    fingerprint = runtime_fingerprint(game / "Hatsumira.exe")
    require(fingerprint["sha256"] == PE, "Hatsumira.exe hash differs")
    seed = int(fingerprint["archive_name_seed"], 16)
    _, sscript = archive_member(game, "kcs.int::sscript.kcs", seed, SSCRIPT)
    scenes = {name: read_cst(game, f"scene.int::{name}.cst", seed, digest) for name, digest in SCENES.items()}
    op001, _ = scenes["op001"]
    movie, _ = scenes["OPMOVIE"]
    after, _ = scenes["OPMOVIE_END"]
    group, _, kind, payload = op001[195]
    require((group, kind, payload) == (15, 0x02, b""), "unexpected anchor at r195")
    before_pages = pages(op001, 196, 619)
    require([page["group"] for page in before_pages] == list(range(16, 62))
            and tuple(page["wait_record"] for page in before_pages[:5]) == RW_MOSAIC_WAITS[16:]
            and before_pages[-1]["wait_record"] == 619, "unexpected pre-OP page sequence")
    tail = commands(op001, 620, len(op001))
    require(len(tail) == len(op001) - 620 and tail[-1] == {"record": 631, "command": "next OPMOVIE"},
            "unexpected op001 exit sequence")
    movie_commands = commands(movie, 0, len(movie))
    require(len(movie_commands) == len(movie) == 8 and movie_commands[-1]["command"] == "next OPMOVIE_END",
            "unexpected OPMOVIE records")
    after_pages = pages(after, 0, 51)
    require([(page["group"], page["wait_record"], page["body_record"]) for page in after_pages]
            == [(0, 42, 41), (1, 46, 45), (2, 51, 50)], "unexpected post-OP body sequence")
    entries, _, _ = kif_entries(game / "movie.int", seed)
    movies = [row for row in entries if row["name"].lower() == b"hatsuop.mpg"]
    require(len(movies) == 1 and movies[0]["size"] == 108539908, "unexpected hatsuop.mpg member")
    return {"status": "source_route_verified", "pe_sha256": PE, "sscript": sscript,
            "scenes": {name: member for name, (_, member) in scenes.items()},
            "anchor": {"group": 15, "wait_record": 195}, "op001_pages": before_pages, "op001_exit": tail,
            "opmovie_commands": movie_commands,
            "movie": {"resource": "movie.int::hatsuop.mpg", "offset": movies[0]["offset"], "bytes": movies[0]["size"]},
            "post_op_pages": after_pages}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("game", type=Path)
    args = parser.parse_args()
    try:
        result = audit(args.game)
    except (OSError, ValueError, IndexError) as error:
        print(json.dumps({"status": "rejected", "reason": str(error)}))
        return 1
    print(json.dumps(result, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

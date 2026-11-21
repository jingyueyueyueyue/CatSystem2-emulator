"""Audit control records and save anchors on two routes to the PC credits."""

import argparse
import hashlib
import json
from pathlib import Path
import re
import struct

from audit_runtime_evidence import runtime_fingerprint
from check_pc_story_11_13_sources import blowfish_le_decrypt, expand_plain_cst, kif_entries, require


ROUTES = {
    "09": (
        "sc02 sc02_01 sc02_02 sc03 sc04 sc05 sc06 sc07 sc07s sc08 sc09 sc10 sc11 "
        "sc12 sc13 sc14 sc15 sc16 scra01 scra02 scra03 scra04 scra05 scra06 scra07 scra08 "
        "scra09 sced01 sced01_a sced01_b sced01_c sced01_d sced01_e"
    ).split(),
    "12": (
        "OPMOVIE_END op002 op002_a op003 air_com001 air_com002 air_com003 air_com004 air_com005 "
        "air_com006 air_com007a air_com007a_a air_com007a_b air_com007a_c air_com007b air_com008 "
        "air_com009 air_com010 air_com011a air_com011b air_com012a air_com012b air_com013 "
        "yuk_ev001a yuk_ev001b yuk_ev001c yuk_ev001d yuk_c004 "
        "mel_com001 mel_com002 mel_com003 mel_com004 mel_com004b mel_com004_ret mel_com005 "
        "mel_com006 mel_com007 mel_com008 yuk_ev002a yuk_ev002b yuk_ev002c yuk_ev002c_end yuk_ev002d "
        "ria_com001 ria_com002 ria_com003a ria_com003b ria_com004 ria_com005 ria_com006a ria_com006b "
        "ria_com007 yuk_ev003a yuk_ev003b yuk_ev003b_end select_root select_root_cha select_root_ret "
        "yuk_ev003c yuk_ev003d_start yuk_ev003d yuk_ev003d_a yuk_ev003d_b yuk_h yuk_imo_start yuk_imo "
        "yuk_imo_a_start yuk_imo_a yuk_imo_aa yuk_imo_ab yuk_imo_ac yuk_imo_b yuk_imo_ba yuk_imo_c "
        "yuk_imo_d_start yuk_imo_d yuk_imo_e_start yuk_imo_e yuk_imo_f yuk_imo_fa_start yuk_imo_fa "
        "yuk_imo_fb yuk_imo_fc yuk_imo_g yuk_imo_h yuk_001_start yuk_001 yuk_001_a yuk_001_c yuk_002 "
        "yuk_002_a yuk_002_b yuk_002_c yuk_002_d yuk_002_e yuk_003 yuk_003_a yuk_003_b yuk_003_c "
        "yuk_003_d yuk_003_e yuk_003_select yuk_ED1 yuk_ED2 YUK_EDMOVIE1 YUK_EDMOVIE2 "
        "EDMOVIE_ENDYUK1 EDMOVIE_ENDYUK2 yuk_ep"
    ).split(),
}
MENU_ROW = re.compile(rb"^(\d+) +([A-Za-z0-9_]+)(?=\s|$)")
BRANCH = re.compile(rb"^(?:(if\s*\([A-Za-z0-9_ #&|<>=!()+\-]*\))\s+)?next +([A-Za-z0-9_]+)$")
FLAGS = re.compile(rb"(?:^| )#(170|194|525|540|612|710|711|748|750|759|760|761|763|849|900)\s*=(?!=)")
CONTROLS = re.compile(rb"[A-Za-z0-9_#&|<>=!() .,\-]+")
END_ASSET = re.compile(rb"(?:^| )(staff\w*|\w+_staff\w*|\w*clear\w*|\w*endcard\w*)\b", re.I)
END_CARD = re.compile(rb"eg 1 (?:vfa_6a|pos 0 -200|blend 0|fade (?:60 0 255|40 255 0))")


def inspect(data, scene):
    body, _ = expand_plain_cst(data)
    _, count, offsets, strings = struct.unpack_from("<4I", body)
    waits, branches, menus, writes, media, timings, special_waits = [], [], [], [], [], [], []
    index = 0
    sources = []
    menu = None
    for group in range(count):
        size, first = struct.unpack_from("<II", body, 16 + 8 * group)
        for ordinal in range(size):
            relative, = struct.unpack_from("<I", body, 16 + offsets + 4 * (first + ordinal))
            start = 16 + strings + relative
            kind = body[start + 1]
            if kind == 0x20 and body[start + 2] != 0:
                sources.append(index)
            elif kind in (2, 3):
                waits.append([group, index])
                if kind != 2 or sources != [index - 1]:
                    special_waits.append([group, index, kind, sources])
                sources = []
            elif kind == 0x30:
                command = body[start + 2:body.index(b"\0", start)].strip()
                if command.split(maxsplit=1)[:1] == [b"fselect"]:
                    menu = {"record": index, "options": []}
                    menus.append(menu)
                elif menu is not None and (option := MENU_ROW.match(command)):
                    menu["options"].append([index, int(option[1]), option[2].decode("ascii")])
                else:
                    menu = None
                    if branch := BRANCH.fullmatch(command):
                        branches.append([index, branch[2].decode("ascii"),
                                         branch[1].decode("ascii") if branch[1] else None])
                    if FLAGS.search(command) and CONTROLS.fullmatch(command):
                        writes.append([index, command.decode("ascii")])
                    if (command == b"title" or (b"movie " in command and CONTROLS.fullmatch(command))
                            or (scene.lower().startswith("sced") and END_ASSET.search(command)
                                and CONTROLS.fullmatch(command))
                            or (scene == "sced01_d" and END_CARD.fullmatch(command))):
                        media.append([index, command.decode("ascii")])
                    if scene.lower().startswith(("sced", "edmovie", "yuk_edmovie")):
                        if re.fullmatch(rb"wait(?: [0-9]+)?", command):
                            timings.append([index, command.decode("ascii")])
            index += 1
    return dict(records=index, waits=len(waits), first_wait=waits[0] if waits else None,
                last_wait=waits[-1] if waits else None, special_waits=special_waits,
                branches=branches, menus=menus, writes=writes, media=media, ending_waits=timings)


def audit(batch, sample):
    summary = json.loads((batch / f"{sample}.json").read_text(encoding="utf-8"))
    game = Path(summary["game"])
    pe = next(file for file in game.iterdir()
              if file.name.lower() == "hatsumira.exe") if sample == "12" else None
    seed = int(runtime_fingerprint(pe)["archive_name_seed"], 16) if pe else 0x16580A97
    requested = {scene.lower().encode("ascii") + b".cst": scene for scene in ROUTES[sample]}
    alternatives = {name + b"l" for name in requested}
    scene_dir = game / "scene"
    if scene_dir.is_dir():
        require(not any(file.name.lower().encode("ascii", "ignore") in (requested.keys() | alternatives)
                        for file in scene_dir.iterdir()), "loose CST/CSTL candidate")
    result = {}
    for archive in game.glob("*.int"):
        if archive.name.lower() != "scene.int" and not archive.name.lower().startswith("update"):
            continue
        entries, _, cipher = kif_entries(archive, seed)
        with archive.open("rb") as stream:
            for entry in entries:
                member = entry["name"].lower()
                require(member not in alternatives, "CSTL candidate requires a separate source audit")
                if member not in requested:
                    continue
                scene = requested[member]
                require(scene not in result, f"multiple CST candidates for {scene}")
                stream.seek(entry["offset"])
                data = stream.read(entry["size"])
                if cipher:
                    data = blowfish_le_decrypt(cipher, data)
                result[scene] = dict(archive=archive.name, sha256=hashlib.sha256(data).hexdigest(),
                                     **inspect(data, scene))
    require(set(ROUTES[sample]) == result.keys(), f"missing CST members: {set(ROUTES[sample]) - result.keys()}")
    return {scene: result[scene] for scene in ROUTES[sample]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--run09", type=Path, required=True)
    parser.add_argument("--run12", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    result = {"status": "control-records", "09": audit(args.run09, "09"), "12": audit(args.run12, "12")}
    with args.out.open("x", encoding="utf-8") as output:
        output.write(json.dumps(result, ensure_ascii=True, separators=(",", ":")) + "\n")


if __name__ == "__main__":
    main()

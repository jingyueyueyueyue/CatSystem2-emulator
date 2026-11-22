"""Build a portable game glyph map from its Windows UIF config."""

import argparse
import json
from pathlib import Path

from fontTools import subset
from fontTools.ttLib import TTFont
from fontTools.ttLib.tables._c_m_a_p import CmapSubtable


BASE_FACE = "NotoSansCJKSC-Regular"
MS_GOTHIC = "\uff2d\uff33 \u30b4\u30b7\u30c3\u30af"
FONT_PROFILES = {
    "hatsumira": ("Hatsumira Simplified Chinese", "HatsumiraSimplifiedChinese-Regular", (MS_GOTHIC,)),
    "utatemeguri": (
        "Utate Meguri Simplified Chinese", "UtateMeguriSimplifiedChinese-Regular",
        ("HG\uff7a\uff9e\uff7c\uff6f\uff78E", MS_GOTHIC),
    ),
}


def read_mapping(config):
    settings = json.loads(config.read_text(encoding="utf-8-sig"))["character_substitution"]
    source = settings["source_characters"]
    target = settings["target_characters"]
    if settings["enable"] is not True or not source or len(source) != len(target):
        raise ValueError("UIF substitution is disabled or has unequal mapping lengths")
    if len(set(source)) != len(source) or any(not 0 < ord(char) < 0xFFFF for char in source):
        raise ValueError("UIF source must contain unique BMP characters")
    return {ord(old): ord(new) for old, new in zip(source, target, strict=True)}


def build_font(config, base, font_number, output, profile="hatsumira"):
    mapping = read_mapping(config)
    family, postscript, game_faces = FONT_PROFILES[profile]
    with TTFont(base, fontNumber=font_number, recalcTimestamp=False) as font:
        if font["name"].getDebugName(6) != BASE_FACE:
            raise ValueError(f"select the {BASE_FACE} face of the source font")
        targets = set(mapping.values())
        missing = targets - font.getBestCmap().keys()
        if missing:
            raise ValueError("base font lacks " + ", ".join(f"U+{char:04X}" for char in sorted(missing)))

        names = font["name"]
        for name_id in (1, 2, 3, 4, 6, 16, 17, 18, 20, 21, 22, 25):
            names.removeNames(nameID=name_id)
        for name_id, name in {1: family, 2: "Regular", 3: postscript, 4: family, 6: postscript}.items():
            names.setName(name, name_id, 3, 1, 0x0409)
        # Register the startup.xml aliases as private game font names.
        for name_id, face in zip((1, 4), game_faces, strict=False):
            names.setName(face, name_id, 3, 1, 0x0411)

        if "CFF " in font:
            cff = font["CFF "].cff
            top = cff.topDictIndex[0]
            cff.fontNames[0] = postscript
            top.FamilyName = family
            top.FullName = family
            if hasattr(top, "CIDFontName"):
                top.CIDFontName = postscript

        options = subset.Options()
        options.name_IDs = [0, 1, 2, 3, 4, 5, 6, 13, 14]
        options.name_languages = [0x0409, 0x0411]
        subsetter = subset.Subsetter(options=options)
        subsetter.populate(unicodes=targets)
        subsetter.subset(font)

        target_cmap = font.getBestCmap()
        cmap = CmapSubtable.newSubtable(4)
        cmap.platformID, cmap.platEncID, cmap.language = 3, 1, 0
        cmap.cmap = {source: target_cmap[target] for source, target in mapping.items()}
        font["cmap"].tables = [cmap]
        output.parent.mkdir(parents=True, exist_ok=True)
        font.save(output)
    return len(mapping)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("config", type=Path, help="game's uif_config.json")
    parser.add_argument("base_font", type=Path, help="Noto Sans CJK collection or SC OTF")
    parser.add_argument("output", type=Path, help="output font path (.ttf or .otf to match input outlines)")
    parser.add_argument("--font-number", type=int, default=-1, help="SC face index for a TTC")
    parser.add_argument("--profile", choices=FONT_PROFILES, default="hatsumira")
    args = parser.parse_args()
    try:
        count = build_font(args.config, args.base_font, args.font_number, args.output, args.profile)
    except (OSError, ValueError, KeyError) as error:
        parser.exit(1, f"{error}\n")
    print(f"Built {args.output.name} with {count} mappings")


if __name__ == "__main__":
    main()

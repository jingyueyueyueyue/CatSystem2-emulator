"""Package Tunaban's CP932 scenes and UIF glyph map for CatSystem2 Player."""

import argparse
import json
import re
from hashlib import sha256
from pathlib import Path
from tempfile import TemporaryDirectory

from build_hatsumira_font import build_font
from restore_kanmi_localization import kif, replace_file


ARCHIVE = "tunaban_cn.int"
FONT = "TunabanSimplifiedChinese.otf"


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("game", type=Path, help="game directory containing scene/ and uif_config.json")
    parser.add_argument("base_font", type=Path, help="Noto Sans CJK collection or SC OTF")
    parser.add_argument("output", type=Path)
    parser.add_argument("--font-number", type=int, default=-1, help="SC face index for a TTC")
    parser.add_argument("--license", type=Path, required=True, help="OFL file for the source font")
    args = parser.parse_args()

    try:
        scenes = {}
        for path in sorted((args.game / "scene").glob("*.cst")):
            name = path.name.lower()
            if not re.fullmatch(r"[a-z0-9_]{1,59}\.cst", name) or name in scenes:
                raise ValueError(f"invalid or duplicate scene name: {path.name}")
            content = path.read_bytes()
            if not content.startswith(b"CatScene"):
                raise ValueError(f"{path.name}: invalid CatScene signature")
            scenes[name] = content
        if not scenes:
            raise ValueError("no CatScene resources found in the supplied scene directory")

        with TemporaryDirectory(prefix="cs2-tunaban-") as temporary:
            font = Path(temporary) / FONT
            cmap_size = build_font(
                args.game / "uif_config.json", args.base_font, args.font_number, font, "tunaban",
            )
            archive = kif(scenes | {FONT: font.read_bytes(), "TunabanFont-OFL.txt": args.license.read_bytes()})

        manifest = {"patches": [{"archive": ARCHIVE, "scene_encoding": "cp932"}]}
        args.output.mkdir(parents=True, exist_ok=True)
        replace_file(args.output / ARCHIVE, archive)
        replace_file(args.output / "cs2-localization.json", (json.dumps(manifest, indent=2) + "\n").encode())
    except (OSError, ValueError, KeyError) as error:
        parser.exit(1, f"{error}\n")

    print(f"{len(scenes)} original scenes, {cmap_size} font mappings")
    print(f"{ARCHIVE} SHA-256 {sha256(archive).hexdigest()}")


if __name__ == "__main__":
    main()

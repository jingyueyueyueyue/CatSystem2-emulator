"""Build contact sheets from completed Windows button case receipts."""

import argparse
import json
from pathlib import Path

from PIL import Image, ImageDraw, ImageOps


THUMB = (480, 270)
ROWS = 4
ROW_HEIGHT = THUMB[1] + 28


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("batch", type=Path)
    parser.add_argument("--indices", type=int, nargs="+")
    args = parser.parse_args()

    cases = []
    for path in sorted(args.batch.glob("*/button.json")):
        case = json.loads(path.read_text(encoding="utf-8"))
        if args.indices is None or case["index"] in args.indices:
            cases.append((path.parent, case))
    if not cases:
        parser.error("no completed button receipts for these indices")

    for start in range(0, len(cases), ROWS):
        group = cases[start:start + ROWS]
        sheet = Image.new("RGB", (THUMB[0] * 2, ROW_HEIGHT * len(group)), "#17191b")
        labels = ImageDraw.Draw(sheet)
        for row, (directory, case) in enumerate(group):
            top = row * ROW_HEIGHT
            label = f'{directory.name} | {case["status"]}'
            labels.text((8, top + 6), label, fill="white")
            after = case.get("after_captures") or []
            captures = (case.get("before_capture"), after[0] if after else None)
            for column, name in enumerate(captures):
                left = column * THUMB[0]
                if not name:
                    label = "Before capture missing" if column == 0 else "After capture missing"
                    labels.text((left + 8, top + 60), label, fill="white")
                    continue
                with Image.open(directory / name) as capture:
                    thumb = ImageOps.contain(capture.convert("RGB"), THUMB)
                sheet.paste(thumb, (left, top + 28))
        suffix = "all" if args.indices is None else "-".join(map(str, args.indices))
        target = args.batch / f"button-review-{suffix}-{start // ROWS + 1:02}.png"
        sheet.save(target)
        print(target)


if __name__ == "__main__":
    main()

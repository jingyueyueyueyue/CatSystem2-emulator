"""Read HG tag headers without extracting assets or running game programs."""
from __future__ import annotations

import argparse
import hashlib
import json
import struct
from pathlib import Path

from format_B_recovery import directory, blowfish_le_decrypt

ROOT = Path(__file__).resolve().parents[2]


def scan(read, size: int) -> list[dict]:
    if read(0, 12) != b"HG-3" + struct.pack("<II", 12, 0x300):
        return []
    findings = []
    frame_offset = 12
    for _ in range(100_000):
        next_frame, frame_id = struct.unpack("<II", read(frame_offset, 8))
        frame_end = frame_offset + next_frame if next_frame else size
        if frame_end <= frame_offset + 8 or frame_end > size:
            raise ValueError("HG frame boundary")
        tag_offset = frame_offset + 8
        native = []
        standard = None
        for _ in range(100_000):
            if tag_offset + 16 > frame_end:
                raise ValueError("HG tag header boundary")
            header = read(tag_offset, 16)
            name = header[:8].split(b"\0", 1)[0]
            next_tag, length = struct.unpack_from("<II", header, 8)
            if tag_offset + 16 + length > frame_end:
                raise ValueError("HG tag payload boundary")
            if name == b"stdinfo":
                standard = struct.unpack("<10I", read(tag_offset + 16, 40))
            if name.startswith(b"img") and name[3:].isdigit():
                if length < 24:
                    raise ValueError("HG native header boundary")
                fields = struct.unpack("<6I", read(tag_offset + 16, 24))
                if 24 + fields[2] + fields[4] > length:
                    raise ValueError("HG compressed stream crosses tag")
                native.append({
                    "name": name.decode("ascii"),
                    "tag_offset": tag_offset, "tag_length": length,
                    "fields": list(fields), "header_hex": header.hex(),
                })
            if next_tag == 0:
                break
            if next_tag < 16 + length:
                raise ValueError("HG tag link overlap")
            tag_offset += next_tag
        else:
            raise ValueError("HG tag limit")
        if len(native) > 1 or any(
            tag["name"] != "img0000"
            or tag["fields"][0] != 0
            or (standard and tag["fields"][1] != standard[1])
            for tag in native
        ):
            findings.append({
                "frame_offset": frame_offset, "frame_id": frame_id,
                "standard": list(standard) if standard else None,
                "native": native,
            })
        if next_frame == 0:
            return findings
        frame_offset = frame_end
    raise ValueError("HG frame limit")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--game", type=int, action="append", help="one-based inventory game")
    parser.add_argument("--limit", type=int, default=20_000, help="maximum image entries per game")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    archive = (ROOT / "archive" / "2026-11").resolve()
    if not output.is_relative_to(archive):
        raise ValueError("analysis output must be under archive/2026-11")
    inventory = json.loads((ROOT / "reports/all_sample_content_audit.json").read_text("utf-8-sig"))
    games = args.game or [5, 17, 3]
    result = {"date": "2026-11-17", "method": "bounded tag-header reads only", "games": []}
    for index in games:
        game = inventory["games"][index - 1]
        root = Path(game["root"])
        item = {"game": index, "root": str(root.relative_to(ROOT)),
                "archives": 0, "images": 0, "findings": [], "errors": []}
        for relative, seed_text in game["archives"]["selected_name_seeds"].items():
            path = root / relative
            if not (path.name.lower().startswith(("image", "update"))
                    and path.suffix.lower() == ".int"):
                continue
            seed = int(seed_text.split()[0], 16)
            try:
                width, entries, cipher = directory(path, seed)
                item["archives"] += 1
                with path.open("rb") as stream:
                    for entry in entries:
                        if not entry["name"].lower().endswith(b".hg3"):
                            continue
                        if item["images"] >= args.limit:
                            break

                        def read(offset: int, length: int) -> bytes:
                            if offset < 0 or offset + length > entry["size"]:
                                raise ValueError("read crosses HG entry")
                            start = offset // 8 * 8 if cipher else offset
                            end = min((offset + length + 7) // 8 * 8, entry["size"]) if cipher else offset + length
                            stream.seek(entry["offset"] + start)
                            data = stream.read(end - start)
                            if len(data) != end - start:
                                raise ValueError("short archive read")
                            if cipher:
                                data = blowfish_le_decrypt(cipher, data)
                            return data[offset - start:offset - start + length]

                        try:
                            frames = scan(read, entry["size"])
                            item["images"] += 1
                            if frames:
                                record = {
                                    "archive": str(path.relative_to(ROOT)),
                                    "seed": f"{seed:08x}", "name_hex": entry["name"].hex(),
                                    "name": entry["name"].decode("cp932", "backslashreplace"),
                                    "offset": entry["offset"], "size": entry["size"],
                                    "name_width": width, "frames": frames,
                                }
                                # Hash only discovered slice resources. No files
                                # are extracted and no source metadata is changed.
                                digest = hashlib.sha256()
                                for offset in range(0, entry["size"], 1024 * 1024):
                                    digest.update(read(offset, min(1024 * 1024, entry["size"] - offset)))
                                record["sha256"] = digest.hexdigest()
                                item["findings"].append(record)
                        except (ValueError, struct.error) as error:
                            item["errors"].append({
                                "archive": relative, "name_hex": entry["name"].hex(),
                                "error": str(error),
                            })
            except (OSError, ValueError, struct.error) as error:
                item["errors"].append({"archive": relative, "error": str(error)})
            if item["images"] >= args.limit:
                break
        result["games"].append(item)
        print(json.dumps({key: value if key not in ("findings", "errors") else len(value)
                          for key, value in item.items()}, ensure_ascii=True), flush=True)
    output.write_text(json.dumps(result, indent=2, ensure_ascii=True) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()

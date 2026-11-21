"""Read-only CST/KIF/startup evidence; never runs or extracts game programs."""

from __future__ import annotations

import argparse
import hashlib
import json
import struct
import zlib
from collections import Counter
from pathlib import Path

import pefile
from Crypto.Cipher import Blowfish

from audit_runtime_evidence import (
    LegacyRandom,
    blowfish_le_decrypt,
    named_resource,
    unobfuscate_name,
    verification_seed,
)
from cst_variants import expand_cst, inspect_body


ROOT = Path(__file__).resolve().parents[2]
MAX_BUFFER = 64 * 1024 * 1024


def sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def directory(path: Path, seed: int):
    """Validate both on-disk name widths, including unencrypted old KIF."""
    length = path.stat().st_size
    candidates = []
    errors = []
    with path.open("rb") as handle:
        header = handle.read(8)
        if len(header) != 8 or header[:4] != b"KIF\0":
            raise ValueError("not KIF")
        count = struct.unpack_from("<I", header, 4)[0]
        if not 0 < count <= 1_000_000:
            raise ValueError("invalid entry count")
        for width in (64, 32):
            try:
                table_end = 8 + count * (width + 8)
                if table_end > min(length, MAX_BUFFER):
                    raise ValueError("directory length")
                handle.seek(0)
                table = handle.read(table_end)
                entries = []
                cipher = None
                for index in range(count):
                    offset = 8 + index * (width + 8)
                    raw = table[offset : offset + width]
                    if b"\0" not in raw:
                        raise ValueError("name has no terminator")
                    name = raw.split(b"\0", 1)[0]
                    position, size = struct.unpack_from("<II", table, offset + width)
                    if index == 0 and name.lower() == b"__key__.dat":
                        key = LegacyRandom(size).next()
                        cipher = Blowfish.new(key.to_bytes(4, "little"), Blowfish.MODE_ECB)
                        continue
                    if cipher:
                        metadata = struct.pack("<II", (position + index) & 0xFFFFFFFF, size)
                        position, size = struct.unpack(
                            "<II", blowfish_le_decrypt(cipher, metadata)
                        )
                        name = unobfuscate_name(name, (seed + index) & 0xFFFFFFFF)
                    if position + size > length or position < table_end:
                        raise ValueError("entry outside payload")
                    entries.append({"name": name, "offset": position, "size": size})
                candidates.append((width, entries, cipher))
            except (ValueError, struct.error) as error:
                errors.append(f"name{width}: {error}")
    if len(candidates) != 1:
        raise ValueError(f"{len(candidates)} valid layouts; {errors}")
    return candidates[0]


def read_entry(handle, entry: dict, cipher) -> bytes:
    if entry["size"] > MAX_BUFFER:
        raise ValueError("resource limit")
    handle.seek(entry["offset"])
    data = handle.read(entry["size"])
    if len(data) != entry["size"]:
        raise ValueError("short read")
    return blowfish_le_decrypt(cipher, data) if cipher else data


def inspect_kcs(data: bytes) -> dict:
    if len(data) < 52 or data[:4] != b"KCS\0":
        raise ValueError("not a complete KCS header")
    header = struct.unpack_from("<12I", data, 4)
    version, _, header_size, code, init_offset, count, init_size, _, _, data_offset, _, capacity = header
    expected = code + init_size
    if (version != 2 or header_size != 52 or init_offset != 52 + code or
            data_offset != 52 + expected or expected > capacity or capacity > MAX_BUFFER):
        raise ValueError("KCS header layout")
    decoder = zlib.decompressobj()
    body = decoder.decompress(data[52:], expected + 1)
    if (not decoder.eof or decoder.unused_data or decoder.unconsumed_tail or
            len(body) != expected):
        raise ValueError("KCS incomplete or checksum-invalid zlib")
    init = body[code:]
    if count * 8 > len(init):
        raise ValueError("KCS string table bounds")
    for index in range(count):
        source, target = struct.unpack_from("<II", init, index * 8)
        if source < count * 8 or source >= len(init):
            raise ValueError("KCS string offset")
        end = init.find(b"\0", source)
        if end < 0 or target + end - source + 1 > header[7]:
            raise ValueError("KCS string initialization bounds")
    return {
        "code_length": code, "string_count": count,
        "body_sha256": sha(body), "zlib_checksum_valid": True,
    }


def inspect_cst(path: Path, seed: int) -> dict:
    width, entries, cipher = directory(path, seed)
    counts = Counter()
    exceptions = []
    with path.open("rb") as handle:
        for entry in entries:
            if not entry["name"].lower().endswith(b".cst"):
                continue
            try:
                data = read_entry(handle, entry, cipher)
                body, compression = expand_cst(data)
                info = inspect_body(body)
                counts[compression["compression"]] += 1
                counts["structured"] += 1
                counts["records"] += info["record_count"]
                if not info["record_offsets_increasing"]:
                    counts["unordered_offsets"] += 1
                strings = 16 + info["string_table"]
                offsets = 16 + info["offset_table"]
                previous_end = strings
                text_bytes = 0
                for index in range(info["record_count"]):
                    relative = struct.unpack_from("<I", body, offsets + index * 4)[0]
                    start = strings + relative
                    end = body.index(0, start) + 1
                    text_bytes += end - start - 3
                    counts["record_gaps"] += start != previous_end
                    previous_end = end
                counts["trailing_string_bytes"] += len(body) - previous_end
                counts["max_owned_text_bytes"] = max(counts["max_owned_text_bytes"], text_bytes)
                if (compression["compression"] == "xor-zlib-recovered-key" or
                        not info["record_offsets_increasing"]):
                    exceptions.append({
                        "name": entry["name"].decode("ascii"),
                        "offset": entry["offset"],
                        "length": len(data),
                        "sha256": sha(data),
                        **compression,
                        **info,
                    })
            except (ValueError, struct.error) as error:
                counts["rejected"] += 1
                exceptions.append({
                    "name_hex": entry["name"].hex(), "error": str(error),
                })
    return {
        "archive": str(path.relative_to(ROOT)), "name_width": width,
        "summary": dict(counts), "exceptions": exceptions,
    }


def inspect_startup(game: dict) -> dict:
    root = Path(game["root"])
    result = {"root": str(root.relative_to(ROOT)), "archives": [], "loose_xml": []}
    seeds = game["archives"]["selected_name_seeds"]
    for path in sorted(root.glob("*")):
        if path.suffix.lower() == ".xml":
            data = path.read_bytes()
            result["loose_xml"].append({
                "name": path.name, "sha256": sha(data), "size": len(data),
                "prefix_hex": data[:16].hex(),
                "entry_lines": [
                    line.decode("ascii", "backslashreplace")
                    for line in data.splitlines()
                    if any(word in line.lower() for word in
                           (b"script", b"entry", b"main", b"startup"))
                ],
            })
        if path.name.lower() not in ("config.int", "kcs.int", "fes.int"):
            continue
        seed = int(seeds.get(path.name, "0x0").split()[0], 16)
        try:
            width, entries, cipher = directory(path, seed)
        except ValueError as error:
            result["archives"].append({"name": path.name, "error": str(error)})
            continue
        item = {
            "name": path.name, "name_width": width, "seed": f"{seed:08x}",
            "encrypted": cipher is not None, "entries": [],
        }
        with path.open("rb") as handle:
            for entry in entries:
                name = entry["name"]
                if path.name.lower() == "fes.int" and not name.lower().endswith(b".xml"):
                    continue
                data = read_entry(handle, entry, cipher)
                record = {
                    "name": name.decode("ascii", "backslashreplace"),
                    "name_hex": name.hex(), "offset": entry["offset"],
                    "size": len(data), "prefix_hex": data[:16].hex(), "sha256": sha(data),
                }
                if name.lower().endswith(b".kcs"):
                    record["kcs"] = inspect_kcs(data)
                if name.lower() == b"startup.xml":
                    record["entry_lines"] = [
                        line.decode("ascii", "backslashreplace")
                        for line in data.splitlines()
                        if any(word in line.lower() for word in
                               (b"script", b"entry", b"main", b"startup"))
                    ]
                item["entries"].append(record)
        result["archives"].append(item)
    result["version_sources"] = []
    for path in sorted(root.glob("*.exe")):
        if path.stat().st_size > 12 * 1024 * 1024:
            continue
        try:
            pe = pefile.PE(str(path), fast_load=False)
        except pefile.PEFormatError:
            continue
        key = named_resource(pe, "KEY_CODE", "KEY")
        key = bytes(byte ^ 0xCD for byte in key) if key else b"windmill"
        for kind in ("V_CODE", "V_CODE2"):
            encrypted = named_resource(pe, kind, "DATA")
            if not encrypted:
                continue
            code = blowfish_le_decrypt(Blowfish.new(key, Blowfish.MODE_ECB), encrypted)
            text = code.split(b"\0", 1)[0]
            result["version_sources"].append({
                "name": path.name, "sha256": sha(path.read_bytes()), "kind": kind,
                "code_hex": code.hex(),
                "code_ascii": text.decode("ascii", "backslashreplace"),
                "printable_ascii": bool(text) and all(32 <= byte <= 126 for byte in text),
                "seed": f"{verification_seed(code):08x}",
            })
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--all-cst", action="store_true")
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    audit = json.loads((ROOT / "reports/all_sample_content_audit.json").read_text("utf-8"))
    result = {"date": "2026-11-17", "batch": "20261116",
              "mode": "static bytes only", "startup": [], "cst": []}
    for index, game in enumerate(audit["games"]):
        if index in (2, 5, 8, 18, 19):
            result["startup"].append(inspect_startup(game))
        root = Path(game["root"])
        seeds = game["archives"]["selected_name_seeds"]
        selected = {
            item["resource"].partition("::")[0]
            for item in game.get("diagnostics", [])
            if item["resource"].lower().endswith(".cst")
        }
        for path in sorted(root.glob("*")):
            if path.suffix.lower() != ".int":
                continue
            if not (path.name in selected or
                    (args.all_cst and (path.stem.lower() == "scene" or
                                      path.stem.lower().startswith("update")))):
                continue
            seed = int(seeds.get(path.name, "0x0").split()[0], 16)
            try:
                result["cst"].append(inspect_cst(path, seed))
            except ValueError as error:
                result["cst"].append({
                    "archive": str(path.relative_to(ROOT)), "error": str(error),
                })
    text = json.dumps(result, ensure_ascii=False, indent=2) + "\n"
    if args.output:
        output = args.output.resolve()
        archive = (ROOT / "archive/2026-11").resolve()
        if output.parent != archive or not output.name.startswith("20261116_format_B_"):
            raise ValueError("output must be archive/2026-11/20261116_format_B_*")
        output.write_text(text, encoding="utf-8")
        print(json.dumps({
            "output": str(output), "cst_archives": len(result["cst"]),
            "structured": sum(i.get("summary", {}).get("structured", 0)
                              for i in result["cst"]),
            "rejected": sum(i.get("summary", {}).get("rejected", 0) for i in result["cst"]),
            "archive_errors": [i for i in result["cst"] if "error" in i],
        }, ensure_ascii=False, indent=2))
    else:
        print(text, end="")


if __name__ == "__main__":
    main()

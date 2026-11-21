"""Read-only evidence for the three CST diagnostic groups dated 2026-11-16.

Run with ``python -X utf8 -B scripts/analysis/cst_variants.py``. Results go to
stdout; no game, extracted resource, cache, or report file is written. Archive
decryption uses this workspace's existing analysis helper, not reference code.
Text and entry names remain bytes; hashes never depend on a guessed code page.
"""

from __future__ import annotations

import argparse
from collections import Counter
import hashlib
import json
from pathlib import Path
import struct
import sys
import zlib

sys.dont_write_bytecode = True

from audit_runtime_evidence import blowfish_le_decrypt, kif_entries

WORKSPACE = Path(__file__).resolve().parents[2]
MAX_BUFFER = 64 * 1024 * 1024
MAX_ITEMS = 1_000_000


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def grisaia_root() -> Path:
    audit = json.loads(
        (WORKSPACE / "reports/all_sample_content_audit.json").read_text(encoding="utf-8")
    )
    return next(
        Path(game["root"])
        for game in audit["games"]
        if any(
            d["resource"].startswith("UPDATE01.INT::ama_002b.cst")
            for d in game.get("diagnostics", [])
        )
    )


def lcg_xor_words(
    image: bytearray, start: int, end: int, seed: int, increment: int
) -> None:
    if start < 0 or end > len(image) or (end - start) % 4:
        raise ValueError("invalid static DWORD XOR range")
    for offset in range(start, end, 4):
        seed = (seed * 1664525 + increment) & 0xFFFFFFFF
        word = struct.unpack_from("<I", image, offset)[0] ^ seed
        struct.pack_into("<I", image, offset, word)


def loader_lzss(data: bytes, expected: int) -> bytes:
    """Pure buffer reconstruction of the local PE loader's RVA 0x3cc630.

    This does not run machine code. The loader's length and Adler32 fields
    are validated independently by its caller.
    """
    if not 0 <= expected <= MAX_BUFFER:
        raise ValueError("LZSS expansion exceeds reader limit")
    window = bytearray(b" " * 4078 + bytes(18))
    write = 0xFEE
    cursor = 0
    flags = 0
    output = bytearray()
    while cursor < len(data):
        flags >>= 1
        if not flags & 0x100:
            flags = data[cursor] | 0xFF00
            cursor += 1
        if cursor == len(data) and len(output) == expected:
            break
        if flags & 1:
            if cursor == len(data):
                raise ValueError("truncated LZSS literal")
            value = data[cursor]
            cursor += 1
            output.append(value)
            window[write] = value
            write = (write + 1) & 4095
        else:
            if cursor + 2 > len(data):
                raise ValueError("truncated LZSS match")
            low, high = data[cursor : cursor + 2]
            cursor += 2
            offset = low | ((high & 0xF0) << 4)
            count = (high & 15) + 3
            for index in range(count):
                value = window[(offset + index) & 4095]
                output.append(value)
                window[write] = value
                write = (write + 1) & 4095
        if len(output) > expected:
            raise ValueError("LZSS output exceeds declared length")
    if len(output) != expected:
        raise ValueError("LZSS output length mismatch")
    return bytes(output)


def loader_substitution_blocks(data: bytes, schedule: bytes, table: bytes) -> bytes:
    """Translate the local loader's RVA 0x3ce930 as buffer operations."""
    if len(schedule) != 60 or len(table) != 256:
        raise ValueError("invalid loader substitution parameters")
    result = bytearray(data)
    for start in range(0, len(result) - 7, 8):
        block = bytearray(result[start : start + 8])
        cursor = 0
        for _ in range(8):
            for index in range(4):
                block[index + 4] ^= table[block[index] ^ schedule[cursor]]
                cursor += 1
            block[1] ^= table[block[4] ^ schedule[cursor]]
            cursor += 1
            block[2] ^= table[block[5] ^ block[4]]
            block[3] ^= table[block[6] ^ schedule[cursor]]
            cursor += 1
            block[0] ^= table[block[7] ^ schedule[cursor]]
            cursor += 1
        for index in range(4):
            block[index + 4] ^= table[block[index] ^ schedule[cursor]]
            cursor += 1
        result[start : start + 8] = block
    return bytes(result)


def loader_section_schedule(key: bytes) -> bytes:
    """Key ordering recovered from RVA 0x3ce8c0."""
    if len(key) != 15:
        raise ValueError("invalid loader section key")
    result = bytearray()
    index = 11
    for _ in range(9):
        for _ in range(4):
            result.append(key[index])
            index = (index + 1) % 15
        index = (index + 8) % 15
        if index == 12:
            break
        result.extend(key[index : index + 3])
        index = (index + 11) % 15
    if len(result) != 60:
        raise ValueError("invalid loader section schedule")
    return bytes(result)


def grisaia_patch_image(full_runtime: bool = False) -> tuple[bytearray, dict]:
    """Recover identified loader data in memory; never start or export a PE."""
    import pefile

    path = next(
        p
        for p in grisaia_root().glob("*.exe")
        if p.stat().st_size == 1293688
        and digest(p.read_bytes())
        == "a43c031234778cbc0c02b7d12edbd1045ccf9a9565fc8fe712362fe043e7a79a"
    )
    pe = pefile.PE(str(path))
    image = bytearray(pe.get_memory_mapped_image())
    size = max(pe.OPTIONAL_HEADER.SizeOfImage, 0x3F0000)
    if size > MAX_BUFFER:
        raise ValueError("PE image exceeds static analysis limit")
    image.extend(bytes(size - len(image)))
    lcg_xor_words(image, 0x3CA004, 0x3DB000, 0x601E1A72, 0x3C6EF35F)
    cursor = 0x3CA004
    while True:
        count, target = struct.unpack_from("<II", image, cursor)
        if not count:
            break
        start = target - pe.OPTIONAL_HEADER.ImageBase
        if count > 128 or not 0 <= start <= len(image) - count * 4:
            raise ValueError("invalid loader restoration table")
        image[start : start + count * 4] = image[cursor + 8 : cursor + 8 + count * 4]
        cursor += 8 + count * 4
    lcg_xor_words(image, 0x3CCEC8, 0x3CD2C8, 0x7CCEC8, 0x3C6EF375)
    lcg_xor_words(image, 0x3CD33C, 0x3E6AA0, 0x7CD332, 0x3C6EF375)
    expanded, stored, checksum = struct.unpack_from("<III", image, 0x3CD332)
    output = loader_lzss(bytes(image[0x3CD33E : 0x3CD33E + stored]), expanded)
    if zlib.adler32(output, 0) != checksum:
        raise ValueError("loader LZSS Adler32 mismatch")
    image[0x3CD332 : 0x3CD332 + expanded] = output
    table = bytes(image[0x3CC14C : 0x3CC14C + 256])
    header_key = bytes(image[0x3CD34F : 0x3CD34F + 15]) * 4
    header = bytearray(
        loader_substitution_blocks(
            bytes(image[0x3CD2DE : 0x3CD2DE + 64]), header_key, table
        )
    )
    previous = 0x37
    for index, value in enumerate(header):
        header[index] ^= previous
        previous = value
    entry, base = struct.unpack_from("<II", header)
    if entry != 0x518157 or base != pe.OPTIONAL_HEADER.ImageBase:
        raise ValueError("invalid reconstructed PE loader header")
    key_va = struct.unpack_from("<I", header, 0x2C)[0]
    lcg_xor_words(image, key_va - base, key_va - base + 512, key_va, 0x3C6EF35F)
    section_key = bytes(image[key_va - base + 32 : key_va - base + 47])
    section_schedule = loader_section_schedule(section_key)
    restored_sections = []
    if full_runtime:
        compressed_mask, encrypted_mask = struct.unpack_from("<II", header, 16)
        for index in range(5):
            offset = 0x3CCEE4 + index * 40
            virtual_size, rva, stored_size = struct.unpack_from("<III", image, offset + 8)
            if not rva:
                continue
            payload = bytes(image[rva : rva + stored_size])
            if encrypted_mask & (1 << index):
                payload = loader_substitution_blocks(payload, section_schedule, table)
            if compressed_mask & (1 << index):
                logical_size, compressed_size = struct.unpack_from("<II", payload)
                if (
                    logical_size > virtual_size
                    or compressed_size > len(payload) - 8
                    or logical_size > MAX_BUFFER
                ):
                    raise ValueError("invalid packed PE section lengths")
                decoder = zlib.decompressobj()
                payload = decoder.decompress(
                    payload[8 : 8 + compressed_size], logical_size + 1
                )
                if (
                    not decoder.eof
                    or decoder.unused_data
                    or decoder.unconsumed_tail
                    or len(payload) != logical_size
                ):
                    raise ValueError("invalid packed PE section zlib stream")
            image[rva : rva + len(payload)] = payload
            restored_sections.append(
                {"rva": rva, "length": len(payload), "sha256": digest(payload)}
            )
    return image, {
        "path": str(path.relative_to(WORKSPACE)),
        "sha256": digest(path.read_bytes()),
        "image_base": pe.OPTIONAL_HEADER.ImageBase,
        "expanded_loader_size": expanded,
        "expanded_loader_adler32_seed0": f"{checksum:08x}",
        "expanded_loader_sha256": digest(output),
        "restored_entry_va": entry,
        "loader_header_hex": header.hex(),
        "section_schedule_hex": section_schedule.hex(),
        "restored_sections": restored_sections,
        "mode": "pure buffer reconstruction; no PE execution",
    }


def read_entries(path: Path, seed: int = 0):
    entries, layout, cipher = kif_entries(path, seed)
    with path.open("rb") as handle:
        for entry in entries:
            if not entry["name"].lower().endswith(b".cst"):
                continue
            if entry["size"] > MAX_BUFFER:
                raise ValueError("CST archive entry exceeds reader limit")
            handle.seek(entry["offset"])
            data = handle.read(entry["size"])
            if len(data) != entry["size"]:
                raise ValueError("truncated archive entry")
            if cipher is not None:
                data = blowfish_le_decrypt(cipher, data)
            yield entry, layout, data


def xor_cst_words(data: bytes, left: int, step: int) -> bytes:
    """Undo the local patch's word mask, retaining both embedded key words."""
    bound = (len(data) & ~15) // 4 - 4
    key_index = bound // 4
    if bound < 8 or bound * 4 + 4 > len(data):
        raise ValueError("CST XOR word range is unsupported")
    output = bytearray(data)
    key = left ^ step
    index = 4
    while index <= bound:
        if index == key_index:
            index += 2
            continue
        value = struct.unpack_from("<I", output, index * 4)[0] ^ key
        struct.pack_into("<I", output, index * 4, value)
        key = (key + step) & 0xFFFFFFFF
        index += 1
    return bytes(output)


def fixed_deflate_prefix(script_length: int, final: bool) -> bytes:
    """First two fixed-Huffman bytes for the known inner length field."""
    bits = 2 | int(final)
    shift = 3
    for value in script_length.to_bytes(4, "little")[:2]:
        code, width = (0x30 + value, 8) if value < 144 else (0x190 + value - 144, 9)
        reverse = int(f"{code:0{width}b}"[::-1], 2)
        bits |= reverse << shift
        shift += width
    return (bits & 65535).to_bytes(2, "little")


def recover_xor_cst(data: bytes) -> tuple[bytes, dict]:
    if len(data) < 48 or data[:8] != b"CatScene":
        raise ValueError("not a bounded XOR CatScene container")
    stored, declared = struct.unpack_from("<II", data, 8)
    if (
        not stored
        or len(data) != stored + 16
        or len(data) > MAX_BUFFER
        or not 16 <= declared <= MAX_BUFFER
    ):
        raise ValueError("invalid XOR CatScene lengths")
    bound = (len(data) & ~15) // 4 - 4
    key_index = bound // 4
    left, step = struct.unpack_from("<II", data, bound)
    candidates = []
    if key_index == 3:
        # The encoder overwrote key word 4 with word 3. Recover a seed only
        # from the evidenced fixed-Huffman CST header, requiring a unique,
        # complete, checksum-valid stream and the full CST structure.
        if struct.unpack_from("<I", data, 16)[0] != declared:
            raise ValueError("lost-key CST does not have the evidenced overwrite")
        for final in (False, True):
            prefix = fixed_deflate_prefix(declared - 16, final)
            for window in range(8):
                cmf = window * 16 + 8
                for level in range(4):
                    flag = level << 6
                    flag += (31 - ((cmf * 256 + flag) % 31)) % 31
                    seed = int.from_bytes(bytes((cmf, flag)) + prefix, "little")
                    decoded = xor_cst_words(data, declared, seed)
                    try:
                        body, evidence = expand_plain_cst(decoded)
                        inspect_body(body)
                    except (ValueError, zlib.error, struct.error):
                        continue
                    if struct.unpack_from("<I", body)[0] != declared - 16:
                        continue
                    candidates.append((decoded, declared, seed, evidence))
    else:
        decoded = xor_cst_words(data, left, step)
        body, evidence = expand_plain_cst(decoded)
        inspect_body(body)
        candidates.append((decoded, left, step, evidence))
    if len(candidates) != 1:
        raise ValueError(f"XOR CST recovery has {len(candidates)} validated candidates")
    decoded, left, step, evidence = candidates[0]
    evidence = dict(evidence)
    evidence.update(
        compression=(
            "xor-zlib-recovered-key" if key_index == 3 else "xor-zlib"
        ),
        key_word_index=key_index,
        initial_key=f"{left ^ step:08x}",
        key_increment=f"{step:08x}",
        recovered_key_word=key_index == 3,
        decoded_container_sha256=digest(decoded),
    )
    return decoded, evidence


def expand_cst(data: bytes) -> tuple[bytes, dict]:
    try:
        return expand_plain_cst(data)
    except (ValueError, zlib.error, struct.error) as plain_error:
        try:
            decoded, evidence = recover_xor_cst(data)
            body, _ = expand_plain_cst(decoded)
            return body, evidence
        except (ValueError, zlib.error, struct.error) as xor_error:
            raise ValueError(
                f"plain CST: {plain_error}; XOR CST: {xor_error}"
            ) from xor_error


def expand_plain_cst(data: bytes) -> tuple[bytes, dict]:
    if len(data) < 16 or data[:8] != b"CatScene":
        raise ValueError("invalid or truncated CatScene header")
    compressed, declared = struct.unpack_from("<II", data, 8)
    if declared > MAX_BUFFER or len(data) > MAX_BUFFER:
        raise ValueError("CST exceeds reader limit")
    stored = compressed or declared
    if len(data) != 16 + stored:
        raise ValueError("CST stored length differs from archive entry")
    payload = data[16:]
    evidence = {
        "stored_length": stored,
        "declared_length": declared,
        "payload_prefix_hex": payload[:16].hex(),
    }
    if not compressed:
        evidence["compression"] = "uncompressed"
        return payload, evidence
    if (
        len(payload) < 2
        or payload[0] & 15 != 8
        or payload[0] >> 4 > 7
        or int.from_bytes(payload[:2], "big") % 31
    ):
        raise ValueError("unsupported CST payload transformation: no valid zlib header")
    decoder = zlib.decompressobj()
    body = decoder.decompress(payload, declared + 1)
    if decoder.unconsumed_tail or len(body) > declared:
        raise ValueError("CST output exceeds declared capacity")
    if decoder.unused_data:
        raise ValueError("CST has bytes after the zlib stream")
    if decoder.eof:
        evidence["compression"] = "zlib"
    else:
        # The two scene.int entries have a complete DEFLATE stream followed
        # by exactly three checksum bytes. The fourth Adler32 byte is zero.
        raw = zlib.decompressobj(-15)
        raw_body = raw.decompress(payload[2:], declared + 1)
        checksum = zlib.adler32(body).to_bytes(4, "big")
        if (
            not raw.eof
            or raw.unconsumed_tail
            or raw_body != body
            or len(raw.unused_data) != 3
            or raw.unused_data != checksum[:3]
            or checksum[3] != 0
        ):
            raise ValueError("incomplete zlib stream outside the evidenced variant")
        repaired = zlib.decompressobj()
        repaired_body = repaired.decompress(payload + b"\0", declared + 1)
        if (
            not repaired.eof
            or repaired.unconsumed_tail
            or repaired.unused_data
            or repaired_body != body
            or len(body) != declared
        ):
            raise ValueError("missing-zero-checksum recovery did not validate")
        evidence.update(
            compression="zlib-missing-final-zero-adler32-byte",
            stored_adler32_hex=raw.unused_data.hex(),
            computed_adler32_hex=checksum.hex(),
        )
    return body, evidence


def inspect_body(body: bytes) -> dict:
    if len(body) < 16:
        raise ValueError("truncated CST body")
    declared, count, offsets, strings = struct.unpack_from("<IIII", body)
    actual = len(body) - 16
    if (
        count > MAX_ITEMS
        or count * 8 > offsets
        or offsets > strings
        or strings > actual
        or (strings - offsets) % 4
    ):
        raise ValueError("invalid CST table boundaries")
    record_count = (strings - offsets) // 4
    if record_count > MAX_ITEMS:
        raise ValueError("record count exceeds reader limit")
    records = []
    record_offsets = []
    kinds = Counter()
    for index in range(record_count):
        relative = struct.unpack_from("<I", body, 16 + offsets + index * 4)[0]
        start = 16 + strings + relative
        end = body.find(b"\0", start)
        if end < start + 2 or body[start] != 1:
            raise ValueError(f"invalid CST record {index} at {start:#x}")
        kinds[body[start + 1]] += 1
        records.append(body[start:end])
        record_offsets.append(relative)
    previous = 0
    for index in range(count):
        length, start = struct.unpack_from("<II", body, 16 + index * 8)
        if start != previous or start + length > record_count:
            raise ValueError(f"noncontiguous CST input {index}")
        previous = start + length
    if previous != record_count:
        raise ValueError("CST inputs do not cover the record table")
    content = b"".join(record + b"\0" for record in records)
    return {
        "script_declared_length": declared,
        "script_actual_length": actual,
        "script_length_excess": declared - actual,
        "input_count": count,
        "record_count": record_count,
        "offset_table": offsets,
        "string_table": strings,
        "record_kinds": {str(k): v for k, v in sorted(kinds.items())},
        "record_offsets_increasing": all(
            left < right for left, right in zip(record_offsets, record_offsets[1:])
        ),
        "first_record_relative": record_offsets[0] if record_offsets else None,
        "body_sha256": digest(body),
        "records_sha256": digest(content),
        "non_ascii_record_count": sum(any(b >= 128 for b in r) for r in records),
    }


def scan_archive(path: Path, seed: int, selected: set[str]) -> dict:
    result = {
        "path": str(path.relative_to(WORKSPACE)),
        "archive_sha256": digest(path.read_bytes()),
        "records": [],
        "summary": {},
    }
    totals = Counter()
    for entry, layout, data in read_entries(path, seed):
        name = entry["name"].decode("ascii", errors="backslashreplace")
        record = {
            "name": name,
            "name_hex": entry["name"].hex(),
            "archive_layout": layout,
            "offset": entry["offset"],
            "length": len(data),
            "sha256": digest(data),
            "historical_diagnostic": name in selected,
        }
        if len(data) >= 16 and data[:8] == b"CatScene":
            stored, declared = struct.unpack_from("<II", data, 8)
            record["outer_header"] = {
                "compressed_length": stored,
                "declared_body_length": declared,
            }
        try:
            body, evidence = expand_cst(data)
            record.update(evidence)
            record.update(inspect_body(body))
            record["status"] = "structured"
            totals[record["compression"]] += 1
            totals["inner_length_mismatch"] += (
                record["script_declared_length"] != record["script_actual_length"]
            )
            totals["inner_length_overdeclared"] += record["script_length_excess"] > 0
            totals["inner_length_underdeclared"] += record["script_length_excess"] < 0
            totals["outer_length_mismatch"] += len(body) != record["declared_length"]
            totals["record_total"] += record["record_count"]
            totals["input_total"] += record["input_count"]
            totals["non_ascii_record_total"] += record["non_ascii_record_count"]
        except (ValueError, zlib.error, struct.error) as error:
            record["status"] = "rejected"
            record["error"] = str(error)
            record["payload_prefix_hex"] = data[16:48].hex()
        totals[record["status"]] += 1
        totals["historical_diagnostics"] += record["historical_diagnostic"]
        result["records"].append(record)
    result["summary"] = dict(totals)
    return result


def video_timestamps(data: bytes, base: int = 0) -> list[tuple[int, int]]:
    """Collect complete video PES headers from an evidence window.

    This is a header observation, not a movie decoder or full PS validator.
    Missing packets outside the window are never treated as end of stream.
    """
    observations = []
    for cursor in range(len(data) - 8):
        if data[cursor : cursor + 3] != b"\0\0\1":
            continue
        if not 0xE0 <= data[cursor + 3] <= 0xEF:
            continue
        size = struct.unpack_from(">H", data, cursor + 4)[0]
        end = cursor + 6 + size
        if not size or end > len(data):
            continue
        start = cursor + 6
        if data[start] & 0xC0 == 0x80:
            if end - start < 8:
                continue
            flags = data[start + 1] >> 6
            if flags not in (2, 3) or data[start + 2] < 5:
                continue
            if start + 3 + data[start + 2] > end:
                continue
            start += 3
        else:
            while start < end and data[start] == 0xFF:
                start += 1
            if start + 2 <= end and data[start] & 0xC0 == 0x40:
                start += 2
        if start + 5 > end:
            continue
        value = data[start : start + 5]
        if value[0] >> 4 not in (2, 3) or not (value[0] & value[2] & value[4] & 1):
            continue
        timestamp = (
            ((value[0] >> 1 & 7) << 30)
            | (value[1] << 22)
            | ((value[2] >> 1) << 15)
            | (value[3] << 7)
            | (value[4] >> 1)
        )
        observations.append((base + cursor, timestamp))
    return observations


def inspect_movie_window(game: dict) -> dict | None:
    path = Path(game["root"]) / "movie.int"
    if not path.exists():
        return None
    seeds = game["archives"]["selected_name_seeds"]
    seed = int(seeds.get("movie.int", "0x0").split()[0], 16)
    entries, layout, cipher = kif_entries(path, seed)
    entry = next((e for e in entries if e["name"].lower().endswith(b".mpg")), None)
    if entry is None:
        return None
    window_size = 1024 * 1024
    tail_start = max(0, entry["size"] - window_size) // 8 * 8
    with path.open("rb") as handle:
        handle.seek(entry["offset"])
        prefix = handle.read(min(entry["size"], window_size))
        handle.seek(entry["offset"] + tail_start)
        tail = handle.read(entry["size"] - tail_start)
    if cipher is not None:
        prefix = blowfish_le_decrypt(cipher, prefix)
        tail = blowfish_le_decrypt(cipher, tail)
    first = video_timestamps(prefix)
    last = video_timestamps(tail, tail_start)
    return {
        "archive": str(path.relative_to(WORKSPACE)),
        "archive_layout": layout,
        "entry": entry["name"].decode("ascii", errors="backslashreplace"),
        "entry_offset": entry["offset"],
        "entry_length": entry["size"],
        "prefix_sha256": digest(prefix),
        "suffix_sha256": digest(tail),
        "suffix_relative_start": tail_start,
        "prefix_video_pts_count": len(first),
        "prefix_first_video_pts": first[0] if first else None,
        "prefix_last_video_pts": first[-1] if first else None,
        "prefix_video_pts_span_seconds": (
            (first[-1][1] - first[0][1]) / 90000 if len(first) > 1 else None
        ),
        "suffix_video_pts_count": len(last),
        "suffix_last_video_pts": last[-1] if last else None,
        "program_end_at_entry_end": tail.endswith(b"\0\0\1\xb9"),
        "duration_status": "unknown: prefix/suffix observations do not decode a whole movie",
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--summary", action="store_true", help="Print counts instead of per-entry evidence"
    )
    parser.add_argument(
        "--movie-evidence",
        action="store_true",
        help="Also observe first/last video timestamps in bounded movie.int windows",
    )
    args = parser.parse_args()
    audit = json.loads(
        (WORKSPACE / "reports/all_sample_content_audit.json").read_text(encoding="utf-8")
    )
    reports = []
    movies = []
    for game in audit["games"]:
        by_archive: dict[str, set[str]] = {}
        for item in game.get("diagnostics", []):
            archive, delimiter, name = item["resource"].partition("::")
            if delimiter and name.lower().endswith(".cst"):
                by_archive.setdefault(archive, set()).add(name)
        if not by_archive:
            continue
        if args.movie_evidence:
            movie = inspect_movie_window(game)
            if movie is not None:
                movies.append(movie)
        seeds = game["archives"]["selected_name_seeds"]
        for filename, selected in by_archive.items():
            # These three failing archives are plain KIF. The matching game's
            # already recorded seed is kept for encrypted directory variants.
            seed_record = seeds.get(filename, next(iter(seeds.values()), "0x0"))
            seed = int(seed_record.split()[0], 16)
            report = scan_archive(Path(game["root"]) / filename, seed, selected)
            if args.summary:
                report.pop("records")
            reports.append(report)
    result = {"date": "2026-11-16", "archives": reports}
    if args.movie_evidence:
        result["movie_windows"] = movies
    print(json.dumps(result, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()

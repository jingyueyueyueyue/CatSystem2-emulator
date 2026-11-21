"""Restore the verified Djibril 5 scene archive without loading installer code."""

import argparse
import hashlib
import json
from pathlib import Path
import struct
import sys
import zlib

import pefile

# Reuse the existing offline KIF and CST readers.
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "analysis"))
from audit_runtime_evidence import blowfish_le_decrypt, kif_entries, runtime_fingerprint
from cst_variants import expand_plain_cst


PROFILES = {
    "djibril5": (
        "Djibril5.exe", "012c71635932c241cde4df986377516279a50b907166757e89ea7b86ff3f813f",
        "cbe602d9f74025153d000731d05c2da7786257572542a3f8a99817cb46153b40",
        0x6301850C, "fe0f4a3d6d7c65368b201a79fa0352482f2a4f1a41dff3e1606f69a43fb64a8b", 264,
    ),
    "labyrinth": (
        "Grisaia2.exe", "ac226c8455eff72114d6b1b9929fae01ac49ba90b8ba579f049f179c4c6dd8d5",
        "943e29e48e7d699824c53c61fc4b5bdd713c7e0067cd6d1e95e2fdf808ac0fdd",
        0xD474E6D5, "dfe8e8509ed2f1ad7f19c7ad00b3213fab33859e77c1bce0e5dc82a3129cd76d", 216,
    ),
}
TABLE_SOURCE = "9b6c7047ddd1fc929fd2e74e62fde6ba4dfc22c1e25e0c8b7ef3887796bafcdc"


def digest(data):
    return hashlib.sha256(data).hexdigest()


def require(condition, reason):
    if not condition:
        raise ValueError(reason)


def rotate(value, bits):
    return ((value << bits) | (value >> (8 - bits))) & 255


def reverse_words(block):
    return block[:4][::-1] + block[4:][::-1]


def restore(data, entries, cipher, table, expected_crc):
    size = len(data)
    first, last = size // 5, size - size // 5
    marker = reverse_words(cipher.encrypt(reverse_words(b"CatScene")))
    inverse = []
    for base in range(0, len(table), 256):
        column = table[base:base + 256]
        require(len(set(column)) == 256, "invalid conversion table")
        inverse.append(bytes(sorted(range(256), key=column.__getitem__)))

    # Only the signature block is needed. Record contents are never decoded as text.
    sample = []
    for entry in entries:
        start = entry["offset"]
        if first <= start and start + 7 <= last:
            sample.extend((start + index, value ^ data[start + index])
                          for index, value in enumerate(marker))

    candidates = []
    for selector in range(16):
        key = [None] * 41
        for offset, mask in sample:
            row = ((offset >> 8) & 255) * 16 + selector
            value = (rotate(inverse[row][mask], (-offset - 4) % 8) - offset) & 255
            slot = offset % len(key)
            if key[slot] not in (None, value):
                break
            key[slot] = value
        else:
            if None in key:
                continue
            candidate = bytes(key)
            if (candidate.isalnum() and sum(candidate) % 16 == selector
                    and zlib.crc32(candidate) == expected_crc):
                candidates.append((selector, candidate))
    require(len(candidates) == 1, "conversion parameters do not match the expected CRC")

    selector, key = candidates[0]
    result = bytearray(data)
    for offset in range(first, last + 1):
        row = ((offset >> 8) & 255) * 16 + selector
        column = rotate((key[offset % len(key)] + offset) & 255, (offset + 4) % 8)
        result[offset] ^= table[row * 256 + column]
    return result


def validate_cst(data):
    body, _ = expand_plain_cst(data)
    _, groups, offsets, strings = struct.unpack_from("<4I", body)
    require(8 * groups <= offsets <= strings <= len(body) - 16
            and (strings - offsets) % 4 == 0, "invalid scene table bounds")
    count = (strings - offsets) // 4
    previous = 0
    for group in range(groups):
        length, start = struct.unpack_from("<II", body, 16 + 8 * group)
        require(start == previous and start + length <= count, "invalid scene group range")
        previous = start + length
    require(previous == count, "scene group ranges do not cover the record table")
    for index in range(count):
        offset, = struct.unpack_from("<I", body, 16 + offsets + 4 * index)
        record = 16 + strings + offset
        require(record + 2 < len(body) and body[record] == 1
                and body.find(b"\0", record + 2) >= 0, "invalid scene record framing")
    return groups, count


def audit(game, table_source, profile):
    exe, pe_sha, fwp_sha, crc, output_sha, count = PROFILES[profile]
    fingerprint = runtime_fingerprint(game / exe)
    require(fingerprint["sha256"] == pe_sha, "unexpected game executable")
    source = game / "scene.int.FWP"
    data = source.read_bytes()
    require(digest(data) == fwp_sha, "unexpected FWP archive")
    dll = table_source.read_bytes()
    require(digest(dll) == TABLE_SOURCE, "unexpected table source")
    table = pefile.PE(data=dll, fast_load=True).get_data(0xACE0, 1 << 20)
    require(len(table) == 1 << 20, "truncated conversion table")

    entries, _, cipher = kif_entries(source, int(fingerprint["archive_name_seed"], 16))
    require(cipher is not None and len(entries) == count
            and all(row["name"].lower().endswith(b".cst") for row in entries), "unexpected KIF directory")
    result = restore(data, entries, cipher, table, crc)
    require(digest(result) == output_sha, "converted archive differs from the verified digest")

    start = None
    for entry in entries:
        offset, size = entry["offset"], entry["size"]
        member = blowfish_le_decrypt(cipher, bytes(result[offset:offset + size]))
        groups, records = validate_cst(member)
        if entry["name"].lower() == b"start.cst":
            require(start is None, "duplicate start scene")
            start = {"offset": offset, "bytes": size, "sha256": digest(member),
                     "groups": groups, "records": records}
    if profile == "djibril5":
        require(start is not None, "missing start scene")
    else:
        require(result == (game / "scene.int").read_bytes(), "installed reference differs")
    return result, {"status": "verified", "profile": profile, "scene_int_sha256": output_sha,
                    "scene_members": count, "start": start,
                    "installed_reference_match": True if profile == "labyrinth" else None}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("game", type=Path)
    parser.add_argument("--table-source", type=Path)
    parser.add_argument("--profile", choices=PROFILES, default="djibril5")
    parser.add_argument("--output", type=Path, help="write a new scene archive after all checks pass")
    args = parser.parse_args()
    if args.output and args.profile != "djibril5":
        parser.error("--output is available only for djibril5")
    try:
        data, receipt = audit(args.game, args.table_source or args.game / "fwinstps.dll", args.profile)
        if args.output:
            with args.output.open("xb") as stream:
                stream.write(data)
            receipt["output"] = str(args.output)
        print(json.dumps(receipt, indent=2))
    except (OSError, ValueError, pefile.PEFormatError, struct.error, zlib.error) as error:
        print(json.dumps({"status": "rejected", "reason": str(error)}))
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

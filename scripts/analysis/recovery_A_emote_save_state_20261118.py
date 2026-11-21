"""NEKO ordinary save receiver navigation, from one fixed KCS, never a VM.

--evidence decodes instruction boundaries once and retains only actual save
and target-producer contexts. The default report reads that small cache.
This is separate from C's opt-in portable whole-host graph.
"""
import argparse
import hashlib
import json
import struct
import zlib

from audit_runtime_evidence import blowfish_le_decrypt, kif_entries, operand_count
from recovery_A_20261116 import ROOT, read_json, sha


CACHE = ROOT / "archive/2026-11/20261118_emote_ordinary_save.json"
PREFIX = ROOT / "reports/recovery_A_20261118_emote_ordinary_save"
KCS_SHA = "5554f1f99f83c63087572ec1a7fb8ddf31987cedab4185c8503a1b1e29f5011a"


def code_bytes():
    identity = read_json(ROOT / "reports/recovery_A_20261116_emote.json")
    path = (ROOT / identity["binary_path"]).parent / "kcs.int"
    entries, _, cipher = kif_entries(path, 0x0F6BB0AE)
    entry = next(row for row in entries if row["name"] == b"sscript.kcs")
    with path.open("rb") as handle:
        handle.seek(entry["offset"])
        raw = handle.read(entry["size"])
    raw = blowfish_le_decrypt(cipher, raw) if cipher else raw
    if sha(raw) != KCS_SHA:
        raise ValueError("fixed NEKO sscript changed")
    length, = struct.unpack_from("<I", raw, 16)
    return identity, zlib.decompress(raw[52:])[:length]


def decode(code, start=0, end=None):
    rows = []
    at = start
    end = len(code) if end is None else end
    while at < end:
        opcode = code[at]
        count = operand_count(opcode)
        if count is None or at + 1 + count * 5 > len(code):
            raise ValueError("invalid fixed KCS instruction boundary")
        rows.append({"offset": at, "opcode": opcode, "operands": [
            {"flags": code[pos], "value": struct.unpack_from("<I", code, pos + 1)[0]}
            for pos in range(at + 1, at + 1 + count * 5, 5)]})
        at += 1 + count * 5
    return rows


def evidence():
    identity, code = code_bytes()
    rows = decode(code)
    positions = {row["offset"]: index for index, row in enumerate(rows)}
    sites = [688, 689, 922, 923]
    selected = {}
    for index, row in enumerate(rows):
        operands = row["operands"]
        if row["opcode"] == 43 and operands[0]["flags"] == 3:
            native = operands[0]["value"]
            if native in sites or native in (244, 254) and 704000 <= row["offset"] < 735000:
                selected[str(row["offset"])] = rows[max(0, index - 13):index + 5]
        # Only writes to the actual 688/689 target slot. Do not classify a
        # task merely from reads of its numeric handle or a native number.
        if row["opcode"] == 21 and operands[0]["value"] == 181808:
            selected["target_write_" + str(row["offset"])] = rows[max(0, index - 15):index + 4]
        if row["opcode"] == 62 and operands[0]["value"] == 181808 and any(
                before["opcode"] == 43 and before["operands"][0] == {"flags": 3, "value": 66}
                for before in rows[max(0, index - 12):index]):
            selected["target_factory_" + str(row["offset"])] = rows[max(0, index - 15):index + 6]
    # Existing resource loader/caller anchors; selected windows are stored,
    # rather than the full instruction stream or executable resource bytes.
    for anchor, before, after in ((788026, 8, 8), (790643, 12, 3), (704691, 12, 4)):
        index = positions[anchor]
        selected["anchor_" + str(anchor)] = rows[index - before:index + after]
    payload = {"PE_sha256": identity["PE_sha256"], "KCS_sha256": KCS_SHA,
        "resource": "nekopara_vol3/kcs.int::sscript.kcs", "contexts": selected,
        "native_sites": {str(native): [row["offset"] for row in rows if row["opcode"] == 43
            and row["operands"][0] == {"flags": 3, "value": native}] for native in sites},
        "scope": "one fixed sscript; boundary decode and literal producer references only",
        "original_or_VM_executed": False}
    CACHE.write_text(json.dumps(payload, ensure_ascii=True, indent=2) + "\n", encoding="utf8")
    print(json.dumps({"cache": CACHE.relative_to(ROOT).as_posix(),
                      "native_sites": payload["native_sites"], "contexts": len(selected)}))


def windows(spans):
    """Read only named existing instruction anchors, without evaluating KCS."""
    _, code = code_bytes()
    data = read_json(CACHE)
    if data["KCS_sha256"] != KCS_SHA:
        raise ValueError("save evidence KCS mismatch")
    for span in spans:
        start, end = (int(value, 0) for value in span.split(":"))
        if not 0 <= start < end <= len(code) or end - start > 14000:
            raise ValueError("bounded save/restore window required")
        rows = decode(code, start, end)
        data.setdefault("windows", {})[span] = rows
        print("window " + span)
        for row in rows:
            args = " ".join(f"{op['flags']:02x}:{op['value']}" for op in row["operands"])
            print(f"{row['offset']} op{row['opcode']:02x} {args}")
    CACHE.write_text(json.dumps(data, ensure_ascii=True, indent=2) + "\n", encoding="utf8")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--evidence", action="store_true")
    parser.add_argument("--window", action="append", default=[])
    args = parser.parse_args()
    if args.evidence:
        evidence()
    if args.window:
        windows(args.window)

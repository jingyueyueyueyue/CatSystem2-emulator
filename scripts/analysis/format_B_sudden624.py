"""Bounded static byte recovery for the sudden-love 624 version.

No machine instructions, original functions, VM, compiler or game are run.
The small XOR/DWORD and bit-stream transforms are independently expressed
from the immutable PE spans, not imported from a reference implementation.
Decoded bytes stay in memory; evidence is archived as JSON only.
"""
from __future__ import annotations
import hashlib
import json
import struct
import sys

sys.dont_write_bytecode = True
from recovery_A_20261116 import Image, ROOT, instruction_row

SHA = "35ec21bcfcee0f324206e945eef1c082e35fd8af594ea0e738a46cd941f5b3f3"


def decode_stage(image):
    source = next(s["data"] for s in image.sections if s["rva"] == 0x76F000)
    header = b"".join((((v ^ 0x1D7A9C1C) + 0x1BA31715) & 0xFFFFFFFF).to_bytes(4, "little")
                      for (v,) in struct.iter_unpack("<I", source[:0x1000]))
    data = header + source[0x1000:]
    cursor, tag, bits = 0x1F8, 0, 0
    output = bytearray()

    def byte():
        nonlocal cursor
        if cursor >= len(data):
            raise ValueError("truncated static packed stream")
        value = data[cursor]
        cursor += 1
        return value

    def bit():
        nonlocal tag, bits
        if not bits:
            tag, bits = byte(), 8
        bits -= 1
        return (tag >> bits) & 1

    def gamma():
        value = 1
        while True:
            value = 2 * value + bit()
            if value > 0x1000000:
                raise ValueError("packed integer exceeds static bound")
            if not bit():
                return value

    def copy(offset, length):
        if not 0 < offset <= len(output) or len(output) + length > 0x1000000:
            raise ValueError("static back-reference exceeds byte range")
        for _ in range(length):
            output.append(output[-offset])

    output.append(byte())
    mode, previous = 2, 0
    while len(output) <= 0x1000000:
        if not bit():
            output.append(byte())
            mode = 2
        elif not bit():
            offset = gamma() - mode
            mode = 1
            if not offset:
                copy(previous, gamma())
            else:
                offset = ((offset - 1) << 8) | byte()
                previous = offset
                length = gamma()
                length += 2 if offset >= 0x7D00 or offset <= 0x7F else 1 if offset >= 0x500 else 0
                copy(offset, length)
        elif not bit():
            value = byte()
            offset = value >> 1
            if not offset:
                return header, bytes(output), cursor
            copy(offset, 2 + (value & 1))
            previous, mode = offset, 1
        else:
            offset = 0
            for _ in range(4):
                offset = offset * 2 + bit()
            if offset > len(output):
                raise ValueError("static single byte back-reference exceeds range")
            output.append(output[-offset] if offset else 0)
            mode = 2
    raise ValueError("static packed output exceeds 16 MiB")


def source():
    rows = json.loads((ROOT / "reports/recovery_A_20261116_summary.json").read_text(encoding="utf8"))
    row = next(row for row in rows["perPE"] if row["sha256"] == SHA)
    return Image(ROOT / row["binary_path"], SHA)


def main():
    image = source()
    header, decoded, consumed = decode_stage(image)
    companion = image.path.with_name("cs2_cn.exe")
    translated = Image(companion, hashlib.sha256(companion.read_bytes()).hexdigest())
    routines = {}
    for name, va, length in [("ordinary624", 0x480CF0, 0xA2),
                             ("companion_resolver", 0x4C7110, 0x13A),
                             ("inline_table_registration", 0x453974, 0x40)]:
        raw = translated.get(va - translated.base, length)
        routines[name] = {"va": va, "sha256": hashlib.sha256(raw).hexdigest(),
                          "instructions": [instruction_row(i, translated.base)
                                           for i in translated.md.disasm(raw, va)]}
    assert translated.u32(0x761A78 - translated.base + 624 * 4) == 0x480CF0
    payload = {"original_code_executed": False, "compiled": False, "tests_run": False,
               "packed_path": image.path.relative_to(ROOT).as_posix(), "packed_sha256": SHA,
               "stage_header_sha256": hashlib.sha256(header).hexdigest(),
               "stage_sha256": hashlib.sha256(decoded).hexdigest(), "stage_size": len(decoded),
               "stage_stream_consumed": consumed,
               "companion_path": companion.relative_to(ROOT).as_posix(),
               "companion_sha256": translated.digest, "ordinary_table_va": 0x761A78,
               "routines": routines, "packed624_equivalence_proven": False,
               "consumer": "ordinary624 -> companion ANM existence/alias resolver -> message269; normalized portable message299 -> retained list frame directory -> ImageAnimation/load; ordinary625/message271 normalized301 -> actual list animation update/pixels",
               "limits": "Translated PE same version is fixed evidence, not proof that the encrypted original wrapper is identical. The existing modern portable resolver/receiver is reused; no fake successful missing-ANM object is allocated."}
    path = ROOT / "archive/2026-11/20261117_format_B_sudden624.json"
    path.write_text(json.dumps(payload, ensure_ascii=True, indent=2) + "\n", encoding="utf8")
    print(json.dumps({"output": path.relative_to(ROOT).as_posix(), "stage_size": len(decoded)}))


if __name__ == "__main__":
    main()

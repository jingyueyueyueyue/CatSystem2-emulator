"""Read a named local KCS and emit bounded static instruction/string context."""
from __future__ import annotations
import argparse
import hashlib
import json
from pathlib import Path
import struct
import zlib
from collections import deque
from audit_runtime_evidence import blowfish_le_decrypt, operand_count
from startup_inventory import ROOT, entries_at, normalized

def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--game", required=True, help="Path relative to workspace")
    parser.add_argument("--resource", required=True, help="Loose KCS or archive.int::name.kcs")
    parser.add_argument("--offset", type=lambda value: int(value, 0), default=0)
    parser.add_argument("--radius", type=int, default=1024)
    parser.add_argument("--address", type=lambda value: int(value, 0),
                        help="Select occurrences of this raw operand DWORD instead of an offset window")
    parser.add_argument("--native", type=int,
                        help="Select immediate calls to this ordinary numeric slot, including nearby instructions")
    parser.add_argument("--name", default="context")
    args = parser.parse_args()
    if not 0 <= args.offset or not 1 <= args.radius <= 65536:
        parser.error("offset/radius is outside the bounded analysis range")
    if not args.name or any(char not in "abcdefghijklmnopqrstuvwxyz0123456789_-" for char in args.name):
        parser.error("name must use lowercase ASCII letters, digits, underscore or hyphen")
    root = (ROOT / args.game).resolve()
    if not root.is_relative_to(ROOT.resolve()):
        parser.error("game must remain in this workspace")
    if "::" in args.resource:
        archive, entry_name = args.resource.split("::", 1)
        path = (root / archive).resolve()
        if not path.is_relative_to(root):
            parser.error("archive escapes game directory")
        audit = json.loads((ROOT / "reports/all_sample_content_audit.json").read_text(encoding="utf-8"))
        game = next(game for game in audit["games"] if Path(game["root"]).resolve() == root)
        seeds = game["archives"]["selected_name_seeds"]
        selected = next((value for name, value in seeds.items() if normalized(name) == normalized(archive)), "0")
        seed = int(selected.split()[0], 0)
        entries, _, cipher = entries_at(path, seed)
        entry = next(entry for entry in entries if entry["name"].decode("cp932") == entry_name)
        if entry["size"] > 64 * 1024 * 1024:
            raise ValueError("KCS source exceeds 64 MiB")
        with path.open("rb") as file:
            file.seek(entry["offset"])
            data = file.read(entry["size"])
        if cipher:
            data = blowfish_le_decrypt(cipher, data)
    else:
        path = (root / args.resource).resolve()
        if not path.is_relative_to(root):
            parser.error("resource escapes game directory")
        data = path.read_bytes()
    if data[:4] != b"KCS\0" or len(data) < 52:
        raise ValueError("source is not KCS")
    code_length = struct.unpack_from("<I", data, 16)[0]
    init_count, init_length = struct.unpack_from("<2I", data, 24)
    length = code_length + init_length
    if length > 64 * 1024 * 1024:
        raise ValueError("KCS expansion exceeds 64 MiB")
    decoder = zlib.decompressobj()
    body = decoder.decompress(data[52:], length + 1)
    if len(body) != length or not decoder.eof:
        raise ValueError("KCS expansion mismatch")
    code = body[:code_length]
    instructions = []
    history = deque(maxlen=12)
    native_context: dict[int, dict] = {}
    after = 0
    offset = 0
    while offset < len(code):
        opcode = code[offset]
        count = operand_count(opcode)
        if count is None or offset + 1 + 5 * count > len(code):
            raise ValueError(f"invalid instruction at {offset:#x}")
        operands = [{"flags": code[at], "value": struct.unpack_from("<I", code, at + 1)[0]}
                    for at in range(offset + 1, offset + 1 + count * 5, 5)]
        selected = (any(operand["value"] == args.address for operand in operands)
                    if args.address is not None else abs(offset - args.offset) <= args.radius)
        row = {"offset": offset, "opcode": opcode, "operands": operands}
        if args.native is not None:
            matched = opcode == 0x2b and operands[0]["flags"] in [3, 0x23] and operands[0]["value"] == args.native
            if matched:
                native_context.update((item["offset"], item) for item in history)
                after = 12
            if after:
                native_context[offset] = row
                after -= 1
            history.append(row)
            selected = False
        if selected:
            instructions.append(row)
        offset += 1 + count * 5
    if args.native is not None:
        instructions = [row for _, row in sorted(native_context.items())]
    strings = []
    init = body[code_length:]
    if init_count > 1_000_000 or init_count * 8 > len(init):
        raise ValueError("KCS string initializer table exceeds its region")
    for index in range(init_count):
        at, address = struct.unpack_from("<2I", init, index * 8)
        if at < init_count * 8 or at >= len(init):
            raise ValueError("invalid KCS string initializer")
        end = init.find(b"\0", at)
        if end < 0:
            raise ValueError("unterminated KCS string initializer")
        value = init[at:end]
        strings.append({"address": address, "hex": value.hex(),
                        "cp932": value.decode("cp932", errors="backslashreplace")})
    report = {
        "status": "static-instructions-not-control-flow-or-execution-proof",
        "game": args.game, "resource": args.resource, "sha256": hashlib.sha256(data).hexdigest(),
        "instructions": instructions, "strings": strings, "original_code_executed": False,
    }
    output = ROOT / "archive/2026-11" / f"20261117_kcs_{args.name}.json"
    output.write_text(json.dumps(report, ensure_ascii=True, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"report": output.relative_to(ROOT).as_posix(),
                      "instructions": len(instructions), "strings": len(strings)}))

if __name__ == "__main__":
    main()

"""Read only history-named FES entries already present in the fixed inventory."""
import hashlib
import json
from pathlib import Path
import re
import struct
import sys
import zlib
import xml.etree.ElementTree as ET

sys.dont_write_bytecode = True
from startup_inventory import entries_at, normalized
from audit_runtime_evidence import blowfish_le_decrypt
from recovery_A_20261116 import Image, instruction_row

ROOT = Path(__file__).resolve().parents[2]
inventory = json.loads((ROOT / "archive/2026-11/20261117_F_fes_graphics_usage.json").read_text(encoding="utf-8"))
audit = json.loads((ROOT / "reports/all_sample_content_audit.json").read_text(encoding="utf-8"))
seeds = {normalized(str(Path(game["root"]).relative_to(ROOT))): {
    normalized(name): int(value.split()[0], 0)
    for name, value in game["archives"]["selected_name_seeds"].items()
} for game in audit["games"]}
rows = []
archives = {}
for game in inventory["games"]:
    root = ROOT / game["root"]
    for record in game["fes"]:
        source = record["source"]
        name = source.rsplit("::", 1)[-1].replace("\\", "/").rsplit("/", 1)[-1]
        if not re.search(r"(?:log|backlog|history|rireki)", name, re.I):
            continue
        if "::" not in source:
            data = (root / source).read_bytes()
        else:
            archive, entry_name = source.split("::", 1)
            path = root / archive
            if path not in archives:
                entries, _, cipher = entries_at(path, seeds[normalized(game["root"])].get(normalized(archive), 0))
                archives[path] = (entries, cipher)
            entries, cipher = archives[path]
            entry = next(entry for entry in entries
                if entry["name"].decode("ascii", "backslashreplace") == entry_name)
            if entry["size"] > 16 * 1024 * 1024:
                raise ValueError("history FES exceeds bounded reader")
            with path.open("rb") as stream:
                stream.seek(entry["offset"])
                data = stream.read(entry["size"])
            if cipher:
                data = blowfish_le_decrypt(cipher, data)
        if hashlib.sha256(data).hexdigest() != record["sha256"] or data[:4] != b"FES\0":
            raise ValueError("history FES differs from the fixed manifest")
        packed, expanded = struct.unpack_from("<II", data, 4)
        if expanded > 16 * 1024 * 1024 or packed + 16 > len(data):
            raise ValueError("history FES has invalid bounds")
        body = zlib.decompress(data[16:16 + packed])
        if len(body) != expanded:
            raise ValueError("history FES expanded size differs")
        declarations, calls, remake = [], [], []
        lines = []
        for number, raw in enumerate(body.splitlines(), 1):
            words = raw.split(b"//", 1)[0].strip().split()
            lines.append((number, raw, words))
            if len(words) >= 2 and words[0] in (b"LOGSTRING", b"LOGSTRING8", b"NVSTRING", b"NVSTRING8"):
                declarations.append(words[1].split(b"[", 1)[0])
            if any(word.lower() == b"remake" for word in words):
                remake.append(dict(line=number, text=raw.decode("ascii", "backslashreplace")))
        for number, raw, words in lines:
            if len(words) >= 2 and words[0].split(b"[", 1)[0] in declarations:
                calls.append(dict(line=number, text=raw.decode("ascii", "backslashreplace")))
        context = [dict(line=number, text=raw.decode("ascii", "backslashreplace"))
                   for number, raw, words in lines
                   if remake and (175 <= number <= 195 or 350 <= number <= 370)]
        rows.append(dict(root=game["root"], source=source, sha256=record["sha256"],
            declarations=[name.decode("ascii", "backslashreplace") for name in declarations],
            calls=calls, remake=remake, remake_context=context))
pe = Image(ROOT / "Yukikoi Melt/YukikoiMelt.exe",
    "63de0e14cd8ce11ad2d68e7ed6076641ff5e5b73d822603cc52764009d812976")
ranges = [(0x2d87e0, 0x2d8949), (0x2bed60, 0x2bedad),
          (0x28d390, 0x28d85f), (0x289790, 0x2899ed),
          (0x26d900, 0x26d94a), (0x27a7b1, 0x27a842)]
pe_evidence = dict(path="Yukikoi Melt/YukikoiMelt.exe", sha256=pe.digest,
    functions=[dict(start=hex(a), end=hex(b), instructions=[
        instruction_row(i, pe.base) for i in pe.md.disasm(pe.get(a, b-a), pe.base+a)])
        for a, b in ranges],
    contract={
        "callback": "kind38 only; language expressions fetched/evaluated separately per bank",
        "language": "26d900 gets declared wide prefix; 27a7e5 reads language/Lxx/prefix into first record string; invalid index returns null",
        "history": "28d390 iterates existing rows, resolves retained source/input, clears and appends main/sub strings; no append/trim/rewrap",
        "messages": "289790 counts prior Message records for CSTL fragment, exact registered name; absent binding falls back to CP932->UTF8 CST",
        "display": "2bed60 sets row+84 subtitle enable; independent row+88/+8c objects require separate subtitle surfaces",
        "remaining": "real sample sub=-1 only; independent subtitle surfaces deliberately remain rejected"})
sample_resources = []
for archive, entry_name in [("config.int", b"startup.xml"), ("scene.int", b"__tar01.cstl")]:
    path = ROOT / "Yukikoi Melt" / archive
    entries, _, cipher = entries_at(path, 0x23a893a2)
    entry = next(entry for entry in entries if entry["name"] == entry_name)
    if entry["size"] > 1024 * 1024:
        raise ValueError("fixed language example exceeds static reader limit")
    with path.open("rb") as stream:
        stream.seek(entry["offset"])
        data = stream.read(entry["size"])
    if cipher:
        data = blowfish_le_decrypt(cipher, data)
    item = dict(source=archive + "::" + entry_name.decode("ascii"),
                sha256=hashlib.sha256(data).hexdigest())
    if entry_name.endswith(b".xml"):
        xml = ET.fromstring(data.rstrip(b"\0"))
        item["prefixes"] = [child.findtext("prefix") for child in xml.find("language")
                            if re.fullmatch(r"L\d+", child.tag)]
        item["enable_sub"] = xml.findtext("language/enable_sub")
        item["log_settings"] = {tag: xml.findtext("GAME/log/" + tag)
                                for tag in ("col", "nameline", "nameformat/first", "nameformat/last")}
    else:
        if data[:8] != b"CSTL\0\0\0\0":
            raise ValueError("fixed language example signature differs")
        cursor = 8
        def ff():
            global cursor
            value = 0
            while True:
                byte = data[cursor]
                cursor += 1
                value += byte
                if byte != 255:
                    return value
        names = []
        for _ in range(ff()):
            length = ff()
            names.append(data[cursor:cursor + length].decode("utf-8"))
            cursor += length
        item["languages"] = names
        item["message_count"] = ff()
    sample_resources.append(item)
result = dict(mode="fixed-history-resource-read-only-not-execution",
    inventory="archive/2026-11/20261117_F_fes_graphics_usage.json",
    selection="existing resource basename contains log/backlog/history/rireki",
    games=len({row["root"] for row in rows}), resources=len(rows),
    remake_calls=sum(len(row["remake"]) for row in rows), rows=rows, pe=pe_evidence,
    fixed_language_examples=sample_resources,
    compiler_invoked=False, type_check=False, tests_run=False, engine_executed=False)
target = ROOT / "reports/fes_log_F_evidence_20261117.json"
target.write_text(json.dumps(result, ensure_ascii=True, indent=2) + "\n", encoding="utf-8")
print(json.dumps({key: result[key] for key in ("games", "resources", "remake_calls")}))
for row in rows:
    if row["remake"]:
        print(json.dumps(dict(root=row["root"], source=row["source"], remake=row["remake"])))

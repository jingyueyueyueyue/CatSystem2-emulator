"""Bounded fixed-driver atlas parser evidence; no renderer/VM execution."""
import argparse
import json
import struct

from recovery_A_20261116 import ROOT, Image
from recovery_A_receiver_contracts_20261116 import block

SHA = "85eb459e94e74ab608f649eb7fb440f2526e90aaa1af7276ed01dc10018903ce"
OUT = ROOT / "archive/2026-11/20261117_emote_atlas_evidence.json"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--span", action="append", default=[])
    parser.add_argument("--calls-to", action="append", default=[])
    args = parser.parse_args()
    path = next(ROOT.glob("*NEKO*/nekopara_vol3/emotedriver.dll"))
    image = Image(path, SHA)
    result = json.loads(OUT.read_text(encoding="utf8")) if OUT.exists() else {
        "sha256": SHA, "addresses": "RVA", "snippets": {}}
    if result["sha256"] != SHA:
        raise ValueError("stale cached driver identity")
    result["strings"] = []
    result.setdefault("direct_call_refs", {})
    for target in args.calls_to:
        dest = int(target, 0)
        refs = []
        for code in image.sections:
            if not code["executable"]:
                continue
            data = code["data"]
            at = data.find(b"\xe8")
            while at >= 0:
                if at + 5 <= len(data):
                    relative, = struct.unpack_from("<i", data, at + 1)
                    if code["rva"] + at + 5 + relative == dest:
                        refs.append(code["rva"] + at)
                at = data.find(b"\xe8", at + 1)
        result["direct_call_refs"][hex(dest)] = refs
    for name in ["attr", "originX", "originY", "truncated_width", "truncated_height"]:
        needle = name.encode("ascii") + b"\0"
        for section in image.sections:
            at = section["data"].find(needle)
            if at < 0:
                continue
            address = section["rva"] + at
            pointer = struct.pack("<I", image.base + address)
            refs = []
            for code in image.sections:
                if not code["executable"]:
                    continue
                pos = code["data"].find(pointer)
                while pos >= 0:
                    refs.append(code["rva"] + pos)
                    pos = code["data"].find(pointer, pos + 1)
            result["strings"].append({"name": name, "rva": address, "operand_refs": refs})
    if args.span:
        for span in args.span:
            start, length = (int(value, 0) for value in span.split(":"))
            if not 0 < length <= 4096:
                raise ValueError("bounded atlas evidence span required")
            result["snippets"][hex(start)] = block(image, start, length)
    else:
        for row in result["strings"]:
            for ref in row["operand_refs"]:
                result["snippets"][hex(ref)] = block(image, ref - 16, 240)
    result["static_only"] = True
    OUT.write_text(json.dumps(result, ensure_ascii=True, indent=2) + "\n", encoding="utf8")
    print(json.dumps({"strings": result["strings"], "calls": result["direct_call_refs"], "snippets": list(result["snippets"])}))


if __name__ == "__main__":
    main()

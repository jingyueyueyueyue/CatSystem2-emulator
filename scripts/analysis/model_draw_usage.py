"""Read selected, already inventoried Campanella MDL object properties only."""
import argparse
import hashlib
import json
from pathlib import Path
import struct

from audit_runtime_evidence import blowfish_le_decrypt
from startup_inventory import entries_at

ROOT = Path(__file__).resolve().parents[2]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--model", action="append", required=True)
    args = parser.parse_args()
    inventory = json.loads((ROOT / "archive/2026-11/20261117_C_old_3d_resources.json").read_text(encoding="utf-8"))
    root = ROOT / json.loads((ROOT / "archive/2026-11/20261117_kcs_c-hg648.json").read_text(encoding="utf-8"))["game"]
    results = []
    for name in args.model:
        row = next(row for row in inventory if row["name"] == name)
        archive = root / row["archive"]
        entries, _, cipher = entries_at(archive, 0x4260e643)
        entry = next(e for e in entries if e["name"] == name.encode("ascii"))
        if entry["size"] > 64 * 1024 * 1024:
            raise ValueError("selected model exceeds bounded read")
        with archive.open("rb") as stream:
            stream.seek(entry["offset"])
            data = stream.read(entry["size"])
        if cipher:
            data = blowfish_le_decrypt(cipher, data)
        if hashlib.sha256(data).hexdigest() != row["sha256"]:
            raise ValueError("selected original resource identity changed")
        graph = next(c for c in row["chunks"] if c["kind"] == 100)
        end = graph["at"] + graph["next"]
        stack = [graph["at"] + 12]
        seen = set()
        objects = []
        while stack:
            at = stack.pop()
            if at in seen or not graph["at"] + 12 <= at < end - 12:
                raise ValueError("cyclic/out of bounds original record")
            seen.add(at)
            sibling, child, size = struct.unpack_from("<3I", data, at)
            if at + 12 + size > end or len(seen) > 100_000:
                raise ValueError("selected record exceeds original graph")
            if sibling:
                stack.append(at + sibling)
            if child:
                stack.append(at + child)
            raw = data[at + 12:at + 12 + size]
            if len(raw) < 0x78 or struct.unpack_from("<I", raw)[0] != 0:
                continue
            kind = struct.unpack_from("<I", raw, 0x48)[0]
            count = struct.unpack_from("<I", raw, 0x74)[0]
            if kind > 3 or count > (size - 0x78) // 64:
                continue  # Buffer/primitive records also have type0.
            props = {}
            for index in range(count):
                record = raw[0x78 + index * 64:0x78 + (index + 1) * 64]
                key, value = [s.split(b"\0", 1)[0].decode("cp932") for s in (record[:32], record[32:])]
                props[key] = value
            objects.append({"at": at, "name": raw[8:0x48].split(b"\0", 1)[0].decode("cp932"),
                            "kind": kind, "properties": props})
        results.append({"resource": row["archive"] + "::" + name, "sha256": row["sha256"],
                        "objects": objects})
    output = ROOT / "archive/2026-11/20261117_F_model_draw_usage.json"
    output.write_text(json.dumps({"original_code_executed": False, "models": results},
                                 ensure_ascii=True, indent=2) + "\n", encoding="utf-8")
    print(json.dumps([{"resource": r["resource"], "objects": len(r["objects"]),
                       "xyz": [n for n in r["objects"] if n["properties"].get("billboard_xyz", "").startswith("1")]}
                      for r in results], ensure_ascii=True))


if __name__ == "__main__":
    main()

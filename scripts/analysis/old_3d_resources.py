"""Read original model headers/scene tables without executing their code."""
import hashlib
import json
from pathlib import Path
import struct

from audit_runtime_evidence import blowfish_le_decrypt
from startup_inventory import entries_at

ROOT = Path(__file__).resolve().parents[2]


def main():
    evidence = json.loads((ROOT / "archive/2026-11/20261117_kcs_c-hg648.json").read_text(encoding="utf-8"))
    root = ROOT / evidence["game"]
    rows = []
    for archive in root.glob("*.int"):
        entries, _, cipher = entries_at(archive, 0x4260e643)
        selected = [entry for entry in entries if entry["name"].lower().endswith(b".mdl")
                    or entry["name"].lower() == b"3dtable.txt"]
        for entry in selected:
            if entry["size"] > 64 * 1024 * 1024:
                raise ValueError("model resource exceeds bounded read")
            with archive.open("rb") as stream:
                stream.seek(entry["offset"])
                data = stream.read(entry["size"])
            if cipher:
                data = blowfish_le_decrypt(cipher, data)
            chunks = []
            if entry["name"].lower().endswith(b".mdl"):
                at = 8
                while at+12 <= len(data) and len(chunks) < 10000:
                    kind, count, step = struct.unpack_from("<3I", data, at)
                    chunks.append({"at": at, "kind": kind, "count": count, "next": step,
                                   "prefix": data[at+12:at+92].hex()})
                    if step < 12:
                        break
                    at += step
            rows.append({"archive": archive.name, "name": entry["name"].decode("ascii", "backslashreplace"),
                         "sha256": hashlib.sha256(data).hexdigest(), "length": len(data),
                         "header": data[:128].hex(), "chunks": chunks,
                         "table_text": data.decode("cp932", "backslashreplace")
                         if not entry["name"].lower().endswith(b".mdl") else None})
    output = ROOT / "archive/2026-11/20261117_C_old_3d_resources.json"
    output.write_text(json.dumps(rows, indent=2, ensure_ascii=True)+"\n", encoding="utf-8")
    print(json.dumps({"resources": len(rows), "examples": rows[:2]}, ensure_ascii=True)[:6000])


if __name__ == "__main__":
    main()

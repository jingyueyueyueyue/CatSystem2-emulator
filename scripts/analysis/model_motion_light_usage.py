"""Statically inspect the already inventoried Campanella MDL graph/motion records."""
import hashlib
import json
from pathlib import Path
import struct
import zlib

from audit_runtime_evidence import blowfish_le_decrypt
from startup_inventory import entries_at
from audit_runtime_evidence import operand_count
from cst_variants import expand_cst

ROOT = Path(__file__).resolve().parents[2]


def main():
    inventory = json.loads((ROOT / "archive/2026-11/20261117_C_old_3d_resources.json").read_text(encoding="utf-8"))
    game = ROOT / json.loads((ROOT / "archive/2026-11/20261117_kcs_c-hg648.json").read_text(encoding="utf-8"))["game"]
    archives = {}
    results = []
    for row in inventory:
        if not row["name"].endswith(".mdl"):
            continue
        path = game / row["archive"]
        if path not in archives:
            archives[path] = entries_at(path, 0x4260e643)
        entries, _, cipher = archives[path]
        entry = next(e for e in entries if e["name"] == row["name"].encode("ascii"))
        with path.open("rb") as stream:
            stream.seek(entry["offset"])
            data = stream.read(entry["size"])
        if cipher:
            data = blowfish_le_decrypt(cipher, data)
        if hashlib.sha256(data).hexdigest() != row["sha256"]:
            raise ValueError("inventoried resource identity changed")
        graph = next(c for c in row["chunks"] if c["kind"] == 100)
        end = graph["at"] + graph["next"]
        stack = [graph["at"] + 12]
        seen = set()
        lights = []
        while stack:
            at = stack.pop()
            if at in seen or not graph["at"] + 12 <= at < end - 12:
                raise ValueError("original graph link cycle/bounds")
            seen.add(at)
            sibling, child, size = struct.unpack_from("<3I", data, at)
            if at + 12 + size > end or len(seen) > 100_000:
                raise ValueError("original record bounds")
            if sibling:
                stack.append(at + sibling)
            if child:
                stack.append(at + child)
            raw = data[at + 12:at + 12 + size]
            if len(raw) >= 0x78 and struct.unpack_from("<I", raw)[0] == 0:
                kind, = struct.unpack_from("<I", raw, 0x48)
                count, = struct.unpack_from("<I", raw, 0x74)
                if kind == 2 and count <= (size - 0x78) // 64:
                    payload = data[at + child + 12:at + child + 12 + struct.unpack_from("<I", data, at + child + 8)[0]] if child else b""
                    lights.append({"name": raw[8:0x48].split(b"\0", 1)[0].decode("cp932"), "payload": payload.hex()})
        motions = [{"at": c["at"], "bytes": data[c["at"] + 12:c["at"] + 12 + c["count"]].hex()}
                   for c in row["chunks"] if c["kind"] == 104]
        results.append({"resource": row["name"], "sha256": row["sha256"], "lights": lights, "motions": motions})
    output = ROOT / "archive/2026-11/20261117_F_model_motion_light_usage.json"
    producers = []
    for archive_name in ("scene.int", "kcs.int"):
        path = game / archive_name
        entries, _, cipher = entries_at(path, 0x4260e643)
        with path.open("rb") as stream:
            for entry in entries:
                if archive_name == "kcs.int" and entry["name"] not in (b"bscript.kcs", b"sscript.kcs"):
                    continue
                if entry["size"] > 64 * 1024 * 1024:
                    raise ValueError("producer exceeds bounded read")
                stream.seek(entry["offset"])
                data = stream.read(entry["size"])
                if cipher:
                    data = blowfish_le_decrypt(cipher, data)
                calls = []
                if data.startswith(b"KCS\0"):
                    body = zlib.decompress(data[52:])
                    code_size, = struct.unpack_from("<I", data, 16)
                    offset = 0
                    while offset < code_size:
                        opcode = body[offset]
                        count = operand_count(opcode)
                        if count is None:
                            raise ValueError("unknown static instruction")
                        if opcode == 43:
                            flag, index, _, size = struct.unpack_from("<BIBI", body, offset + 1)
                            if flag in (3, 0x23) and 552 <= index <= 556:
                                calls.append({"offset": offset, "index": index, "size": size})
                        offset += 1 + count * 5
                elif data.startswith(b"CatScene"):
                    body, _ = expand_cst(data)
                else:
                    body = data
                hits = []
                for token in (b"anm3d", b"cameratest_03", b"model_tower_area_01"):
                    start = 0
                    while (at := body.find(token, start)) >= 0:
                        hits.append({"token": token.decode("ascii"), "offset": at,
                                     "context": body[max(0, at - 32):at + 96].hex()})
                        start = at + len(token)
                if hits or calls:
                    producers.append({"resource": archive_name + "::" + entry["name"].decode("cp932"),
                                      "sha256": hashlib.sha256(data).hexdigest(), "hits": hits, "calls": calls})
    output.write_text(json.dumps({"original_code_executed": False, "models": results, "producers": producers}, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"models": len(results), "light_models": [r["resource"] for r in results if r["lights"]],
                      "motion_models": [r["resource"] for r in results if r["motions"]], "producers": producers}, ensure_ascii=True))


if __name__ == "__main__":
    main()

"""Bounded, static FES/raw-HG source evidence; no engine or pixel algorithm runs."""
from pathlib import Path
import hashlib
import json
import re
import struct
import subprocess
import sys
import zlib

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "archive/2026-11/20261117_format_B_fes_image_sources.json"


def sha(data):
    return hashlib.sha256(data).hexdigest()


def inspect(data, source):
    if data[:4] != b"FES\0":
        raise ValueError("not FES")
    stored, length = struct.unpack_from("<II", data, 4)
    body = zlib.decompress(data[16:16 + stored])
    if len(body) != length:
        raise ValueError("FES length mismatch")
    rows = []
    columns = []
    section = b""
    for number, raw in enumerate(body.splitlines(), 1):
        words = raw.strip().split()
        if not words:
            continue
        if words[0].startswith(b"#"):
            section = words[0]
            columns = words[1:] if section == b"#OBJECT" else []
        if (b"IDN" in columns or b"keywait" in raw or b"userfontobj" in raw
                or (b"FILE" in columns and b'"' in raw)):
            rows.append({"line": number, "raw_hex": raw.hex(),
                         "text": raw.decode("cp932", "backslashreplace"),
                         "columns": [c.decode("ascii", "backslashreplace") for c in columns]})
    return {"source": source, "sha256": sha(data), "body_sha256": sha(body), "rows": rows,
            "literal_quoted_file_rows": sum("FILE" in r["columns"]
                and not bytes.fromhex(r["raw_hex"]).lstrip().startswith(b"#")
                and b'"' in bytes.fromhex(r["raw_hex"]) for r in rows)}


def hg_directory(path):
    data = path.read_bytes()
    if data[:4] != b"HG-3":
        raise ValueError("expected identified HG-3")
    offset = struct.unpack_from("<I", data, 4)[0]
    frames = []
    seen = set()
    while offset < len(data):
        if offset in seen:
            raise ValueError("HG-3 frame loop")
        seen.add(offset)
        next_offset, frame_id = struct.unpack_from("<II", data, offset)
        tag = offset + 8
        fields = None
        while tag + 16 <= len(data):
            name, following, length = struct.unpack_from("<8sII", data, tag)
            if name.rstrip(b"\0") == b"stdinfo":
                fields = list(struct.unpack_from("<10I", data, tag + 16))
                break
            if not following:
                break
            tag += following
        frames.append({"id": frame_id, "offset": offset, "stdinfo": fields})
        if not next_offset:
            break
        offset += next_offset
    return {"source": path.relative_to(ROOT).as_posix(), "sha256": sha(data),
            "size": len(data), "frames": frames, "smallest_unsigned_id": min(f["id"] for f in frames)}


def main():
    if "--syntax" in sys.argv:
        formatter = Path.home() / ".rustup/toolchains/stable-x86_64-pc-windows-gnu/bin/rustfmt.exe"
        rows = []
        for name in ("fes_image_source.rs", "fes_media.rs", "image_manager.rs", "list_native.rs"):
            path = ROOT / "simulator/crates/desktop/src" / name
            data = path.read_bytes()
            result = subprocess.run([str(formatter), "--edition", "2024", "--emit", "stdout",
                "--config", "skip_children=true"], input=data, capture_output=True)
            rows.append({"source": path.relative_to(ROOT).as_posix(), "sha256": sha(data),
                "grammar_read": result.returncode == 0, "source_unchanged": data == path.read_bytes(),
                "diagnostic": result.stderr.decode("utf8", "replace")})
        output = OUT.with_name("20261117_format_B_fes_image_sources_syntax.json")
        output.write_text(json.dumps({"type_checked": False, "compiled": False, "executed": False,
            "sources": rows}, indent=2) + "\n", encoding="utf8")
        print(json.dumps({"output": output.relative_to(ROOT).as_posix(),
            "grammar_read": all(r["grammar_read"] for r in rows)}))
        return
    rows = []
    # Already identified loose baseline sources, no filesystem/PE rediscovery.
    for name in ("meswnd_A", "meswnd_B", "log_A", "log_B", "select"):
        path = ROOT / "cs2_full_v401/system/fes" / (name + ".fes")
        rows.append(inspect(path.read_bytes(), path.relative_to(ROOT).as_posix()))
    # One already inventoried production FES archive, fixed name seed.
    from startup_inventory import entries_at
    from audit_runtime_evidence import blowfish_le_decrypt
    archive = ROOT / "Yukikoi Melt/fes.int"
    entries, _, cipher = entries_at(archive, 0x23A893A2)
    with archive.open("rb") as file:
        for name in (b"meswnd.fes", b"select.fes", b"log.fes"):
            entry = next((e for e in entries if e["name"] == name), None)
            if entry is None:
                continue
            file.seek(entry["offset"])
            data = file.read(entry["size"])
            if cipher:
                data = blowfish_le_decrypt(cipher, data)
            rows.append(inspect(data, archive.relative_to(ROOT).as_posix() + "::" + name.decode("ascii")))
    images = [hg_directory(ROOT / "cs2_full_v401/system/image" / (name + ".hg3"))
              for name in ("sys_click", "sys_userfont")]
    OUT.write_text(json.dumps({"mode": "static-identified-FES-and-HG-directories",
        "engine_executed": False, "pixel_algorithms_executed": False, "sources": rows,
        "images": images, "multifile_scope": "quoted FILE rows in these named sources only; runtime expansions not evaluated"},
        ensure_ascii=True, indent=2) + "\n", encoding="utf8")
    print(json.dumps({"output": OUT.relative_to(ROOT).as_posix(), "sources": len(rows),
        "raw_frames": {i["source"]: len(i["frames"]) for i in images}}))


if __name__ == "__main__":
    main()

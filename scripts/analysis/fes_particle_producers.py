"""Find particle producers in the already pinned FES resource manifest.

This reads resource bytes, never a VM or a game executable. Archive names and
offsets come from the existing inventory; it does not discover new game files.
The result is a literal-source inventory, not an assertion of reachability.
"""
from __future__ import annotations

import hashlib
import json
import argparse
from pathlib import Path
import re
import struct
import sys
import zlib

sys.dont_write_bytecode = True
from startup_inventory import entries_at, normalized
from audit_runtime_evidence import blowfish_le_decrypt

ROOT = Path(__file__).resolve().parents[2]
MANIFEST = ROOT / "archive/2026-11/20261117_F_fes_graphics_usage.json"
OUTPUT = ROOT / "archive/2026-11/20261117_fes_particle_producers.json"
LIMIT = 16 * 1024 * 1024


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--matched-only", action="store_true", help="Read only the literal matches from the previous inventory.")
    args = parser.parse_args()
    manifest_bytes = MANIFEST.read_bytes()
    inventory = json.loads(manifest_bytes)
    selected = None
    if args.matched_only:
        previous = json.loads(OUTPUT.read_bytes())
        if previous["manifest_sha256"] != hashlib.sha256(manifest_bytes).hexdigest():
            raise ValueError("matched inventory belongs to a different manifest")
        selected = {(game["root"], record["source"])
                    for game in previous["games"] for record in game["matched_resources"]}
    audit = json.loads((ROOT / "reports/all_sample_content_audit.json").read_bytes())
    seeds = {
        normalized(str(Path(game["root"]).relative_to(ROOT))): {
            normalized(name): int(value.split()[0], 0)
            for name, value in game["archives"]["selected_name_seeds"].items()
        }
        for game in audit["games"]
    }
    archives = {}
    games = []
    for game in inventory["games"]:
        row = {"root": game["root"], "resources": 0, "matched_resources": [],
               "diagnostics": [], "manifest_diagnostics": game.get("diagnostics", [])}
        for record in game["fes"]:
            source = record["source"]
            if selected is not None and (game["root"], source) not in selected:
                continue
            try:
                root = ROOT / game["root"]
                if "::" not in source:
                    path = root / source
                    if path.stat().st_size > LIMIT:
                        raise ValueError("loose FES exceeds bounded reader")
                    raw = path.read_bytes()
                else:
                    archive, entry_name = source.split("::", 1)
                    path = root / archive
                    if path not in archives:
                        entries, _, cipher = entries_at(
                            path, seeds[normalized(game["root"])].get(normalized(archive), 0))
                        archives[path] = (entries, cipher)
                    entries, cipher = archives[path]
                    entry = next(item for item in entries
                        if item["name"].decode("ascii", "backslashreplace") == entry_name)
                    if entry["size"] > LIMIT:
                        raise ValueError("archive FES exceeds bounded reader")
                    with path.open("rb") as stream:
                        stream.seek(entry["offset"])
                        raw = stream.read(entry["size"])
                    if len(raw) != entry["size"]:
                        raise ValueError("archive FES read is truncated")
                    if cipher:
                        raw = blowfish_le_decrypt(cipher, raw)
                if hashlib.sha256(raw).hexdigest() != record["sha256"]:
                    raise ValueError("FES differs from the pinned manifest")
                if len(raw) < 16 or raw[:4] != b"FES\0":
                    raise ValueError("FES signature/header is unavailable")
                packed, expanded = struct.unpack_from("<II", raw, 4)
                if packed + 16 > len(raw) or expanded > LIMIT:
                    raise ValueError("FES lengths exceed the bounded source")
                inflater = zlib.decompressobj()
                body = inflater.decompress(raw[16:16 + packed], expanded + 1)
                if len(body) != expanded or not inflater.eof or inflater.unconsumed_tail:
                    raise ValueError("FES expansion is incomplete or exceeds its declared length")
                lines = body.splitlines()
                hits = [i for i, line in enumerate(lines)
                        if re.search(rb"PARTICLE", line, re.IGNORECASE)]
                row["resources"] += 1
                if hits:
                    names = [match.group(1) for line in lines
                             if (match := re.match(rb"\s*PARTICLE\s+([^\s\[]+)", line, re.IGNORECASE))]
                    producers = [j for j, line in enumerate(lines)
                                 if any(re.search(rb"(?<![A-Za-z0-9_])" + re.escape(name) + rb"(?![A-Za-z0-9_])", line)
                                        for name in names)]
                    context_lines = sorted({j for i in hits for j in range(max(0, i - 2), min(len(lines), i + 4))})
                    row["matched_resources"].append({
                        "source": source, "sha256": record["sha256"],
                        "body_sha256": hashlib.sha256(body).hexdigest(),
                        "literal_hit_lines": [i + 1 for i in hits],
                        "context": [{"line": j + 1, "text": lines[j].decode("ascii", "backslashreplace")}
                                    for j in context_lines],
                        "particle_objects": [name.decode("ascii", "backslashreplace") for name in names],
                        "producer_lines": [{"line": j + 1, "text": lines[j].decode("ascii", "backslashreplace")}
                                           for j in producers],
                    })
            except (OSError, ValueError, KeyError, StopIteration, struct.error, zlib.error) as error:
                row["diagnostics"].append({"source": source, "error": str(error)})
        games.append(row)
    result = {"mode": "pinned-resource-static-read-only", "manifest": str(MANIFEST.relative_to(ROOT)),
              "manifest_sha256": hashlib.sha256(manifest_bytes).hexdigest(), "games": games,
              "resource_count": sum(g["resources"] for g in games),
              "matched_resource_count": sum(len(g["matched_resources"]) for g in games),
              "diagnostic_count": sum(len(g["diagnostics"]) for g in games),
              "scope": "Literal PARTICLE tokens, including comments; macros/includes and dynamic reachability require separate review.",
              "target_code_executed": False}
    output = OUTPUT.with_name(OUTPUT.stem + "_callers.json") if args.matched_only else OUTPUT
    output.write_text(json.dumps(result, ensure_ascii=True, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({key: result[key] for key in ("resource_count", "matched_resource_count", "diagnostic_count")}))


if __name__ == "__main__":
    main()

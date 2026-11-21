"""Inventory configured startup entries by reading local XML and KIF tables.

No VM or game executable is run. Only startup XML payloads are decoded. Archive
name seeds come from the existing, identified sample audit, not filename guesses.
"""

from __future__ import annotations

import hashlib
import json
import re
import struct
import sys
from pathlib import Path

sys.dont_write_bytecode = True

from Crypto.Cipher import Blowfish
from audit_runtime_evidence import (
    LegacyRandom, blowfish_le_decrypt, kif_entries, unobfuscate_name,
)


ROOT = Path(__file__).resolve().parents[2]
REPORT = ROOT / "reports/startup_inventory_20261116"
SKIP = {"backup", "backups", "patch", "patches", "archive", ".git", "save", "savedata"}


def normalized(value: str) -> str:
    return value.replace("\\", "/").strip("/").lower()


def entries_at(path: Path, seed: int):
    try:
        entries, layout, cipher = kif_entries(path, seed)
        return entries, layout + "-64", cipher
    except ValueError as standard_error:
        # Legacy KIF uses 32-byte names and the same 8-byte entry metadata.
        with path.open("rb") as handle:
            header = handle.read(8)
            if header[:4] != b"KIF\0" or len(header) != 8:
                raise standard_error
            count = struct.unpack_from("<I", header, 4)[0]
            if count > 1_000_000 or 8 + 40 * count > path.stat().st_size:
                raise standard_error
            table = handle.read(count * 40)
        entries = []
        for index in range(count):
            record = table[index * 40:(index + 1) * 40]
            name_end = record[:32].find(b"\0")
            if name_end < 0:
                raise standard_error
            offset, size = struct.unpack_from("<II", record, 32)
            entries.append({"name": record[:name_end], "offset": offset, "size": size})
        encrypted = bool(entries and entries[0]["name"].lower() == b"__key__.dat")
        cipher = None
        if encrypted:
            key = LegacyRandom(entries[0]["size"]).next()
            cipher = Blowfish.new(struct.pack("<I", key), Blowfish.MODE_ECB)
            for index, entry in enumerate(entries[1:], 1):
                metadata = struct.pack("<II", (entry["offset"] + index) & 0xFFFFFFFF, entry["size"])
                entry["offset"], entry["size"] = struct.unpack(
                    "<II", blowfish_le_decrypt(cipher, metadata)
                )
                entry["name"] = unobfuscate_name(entry["name"], (seed + index) & 0xFFFFFFFF)
            entries = entries[1:]
        for entry in entries:
            if entry["offset"] < 8 + 40 * count or entry["offset"] + entry["size"] > path.stat().st_size:
                raise standard_error
        return entries, ("encrypted" if encrypted else "plain") + "-32", cipher


def config_fields(data: bytes) -> dict:
    """Read ASCII markup without guessing the code page of unrelated fields."""
    result = {}
    for section, leaf in [
        (b"SCRIPT", b"start"), (b"SCREEN", b"width"), (b"SCREEN", b"height"),
        (b"SCREEN", b"v_width"), (b"SCREEN", b"v_height"),
    ]:
        block = re.search(rb"<" + section + rb"(?:\s[^>]*)?>(.*?)</" + section + rb">", data, re.S)
        field = re.search(rb"<" + leaf + rb">(.*?)</" + leaf + rb">", block[1], re.S) if block else None
        if field:
            result[(section + b"/" + leaf).decode("ascii")] = field[1].strip().decode("cp932", "backslashreplace")
    return result


def scan_game(game: dict) -> dict:
    root = Path(game["root"])
    seed_map = {
        normalized(name): int(value.split()[0], 0)
        for name, value in game["archives"]["selected_name_seeds"].items()
    }
    configs = []
    names: dict[str, list[str]] = {}
    errors = []
    for path in sorted(root.rglob("*")):
        if not path.is_file() or any(part.lower() in SKIP for part in path.relative_to(root).parts):
            continue
        relative = path.relative_to(root).as_posix()
        suffix = path.suffix.lower()
        if suffix == ".kcs":
            for key in [normalized(relative), path.name.lower()]:
                names.setdefault(key, []).append(relative)
        if path.name.lower() == "startup.xml":
            data = path.read_bytes()
            configs.append({"source": relative, "sha256": hashlib.sha256(data).hexdigest(),
                            "fields": config_fields(data), "priority": 1000})
        if suffix != ".int" or not (
            path.stem.lower() in {"config", "kcs", "system"}
            or path.stem.lower().startswith("update")
        ):
            continue
        try:
            entries, layout, cipher = entries_at(path, seed_map.get(normalized(relative), 0))
            for entry in entries:
                name = entry["name"].decode("cp932", "backslashreplace").replace("\\", "/")
                qualified = f"{relative}/{name}"
                for key in [normalized(name), normalized(qualified), normalized(f"{path.name}/{name}")]:
                    names.setdefault(key, []).append(qualified)
                if normalized(name).rsplit("/", 1)[-1] != "startup.xml":
                    continue
                if entry["size"] > 16 * 1024 * 1024:
                    raise ValueError("startup XML exceeds the analysis limit")
                with path.open("rb") as handle:
                    handle.seek(entry["offset"])
                    data = handle.read(entry["size"])
                if cipher:
                    data = blowfish_le_decrypt(cipher, data)
                priority = 600 + int(path.stem[-2:]) if re.fullmatch("update[0-9]{2}", path.stem.lower()) else 500
                configs.append({
                    "source": qualified, "sha256": hashlib.sha256(data).hexdigest(),
                    "layout": layout, "fields": config_fields(data), "priority": priority,
                })
        except (ValueError, OSError) as error:
            errors.append({"source": relative, "error": str(error)})
    configs.sort(key=lambda item: (-item["priority"], normalized(item["source"])))
    selected = next((c for c in configs if c["fields"].get("SCRIPT/start")), None)
    entry = selected["fields"].get("SCRIPT/start") if selected else None
    resolved = names.get(normalized(entry or ""), [])
    return {
        "root": str(root), "configuration_candidates": configs,
        "selected_configuration": selected["source"] if selected else None,
        "configured_entry": entry,
        "matching_resources": sorted(set(resolved)),
        "entry_found_in_static_index": bool(resolved),
        "kcs_named_resources": sorted({key for key in names if key.endswith(".kcs") and "/" not in key}),
        "diagnostics": errors,
    }


def main() -> None:
    audit = json.loads((ROOT / "reports/all_sample_content_audit.json").read_text(encoding="utf-8"))
    games = [scan_game(game) for game in audit["games"]]
    result = {
        "schema": "cs2-startup-inventory/v1",
        "status": "static-read-only-no-VM-execution",
        "game_count": len(games),
        "configured_entries_found": sum(g["entry_found_in_static_index"] for g in games),
        "games": games,
    }
    REPORT.with_suffix(".json").write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    lines = [
        "# 启动入口静态清单", "", "日期：2026-11-16。只读 XML 和 KIF 表，不运行 VM、原版或编译。",
        "", "| 样本 | 配置来源 | SCRIPT/start | 资源存在 |",
        "| --- | --- | --- | --- |",
    ]
    for game in games:
        lines.append(f"| {Path(game['root']).name} | `{game['selected_configuration']}` | "
                     f"`{game['configured_entry']}` | {game['entry_found_in_static_index']} |")
    lines += [
        "", "资源存在只证明配置引用可定位，不证明该入口或游戏已运行。"
        "脚本分类在 scripts/analysis/，正式清单在 reports/；未提取、移动或修改游戏文件。", "",
    ]
    REPORT.with_suffix(".md").write_text("\n".join(lines), encoding="utf-8")
    print(json.dumps({"games": len(games), "entries_found": result["configured_entries_found"],
                      "unresolved": [g for g in games if not g["entry_found_in_static_index"]]},
                     ensure_ascii=True, indent=2))


if __name__ == "__main__":
    main()

"""Pin and inventory CatSystem2 research references; never import their code.

Existing checkouts are inventoried without fetch, checkout or modification.
--sync acquires only missing references, then freezes their concrete commit.
Git/network diagnostics are kept under the dated workspace archive.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import subprocess
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime
from pathlib import Path
from zoneinfo import ZoneInfo


ROOT = Path(__file__).resolve().parents[2]
REFERENCE = ROOT / "resources/reference"
LOGS = ROOT / "archive/2026-11"
SOURCES = [
    ("KatzeSystem2", "Alphaly2K/KatzeSystem2", None, "Rust format/runtime documentation; independent implementation"),
    ("FelineSystem2", "kokseen1/FelineSystem2", None, "Independent C++ archive/image/scene behavior comparison"),
    ("TriggersTools.CatSystem2", "trigger-segfault/TriggersTools.CatSystem2", None, "Format and version comparison"),
    ("catsystem-py", "trigger-segfault/catsystem-py", None, "Python format containers and version evidence"),
    ("CatSceneEditor", "marcussacana/CatSceneEditor", None, "INT/CSTL/debugger structure only; no editor integration"),
    ("FuckGalEngine", "Inori/FuckGalEngine", "CatSystem2", "Old CatSystem2 encryption and format comparison"),
    ("chinesize", "regomne/chinesize", "CatSystem2", "Extraction evidence and fixture reference only"),
    ("CatSystem2-Simple-Translating-Tools", "Wolverator/CatSystem2-Simple-Translating-Tools", None,
     "Resource overrides and update/archive behavior only"),
    ("FreeMote", "UlyssesWu/FreeMote", None,
     "PSB/PSZ structure and E-mote asset research only; no SDK/runtime or code adoption"),
]


def git(path: Path, *args: str) -> str:
    result = subprocess.run(
        ["git", "-C", str(path), *args], capture_output=True, encoding="utf-8",
        errors="replace", timeout=120,
    )
    if result.returncode:
        raise RuntimeError(result.stderr.strip())
    return result.stdout.strip()


def acquire(source: tuple, sync: bool) -> dict:
    name, repository, sparse, purpose = source
    path = REFERENCE / name
    url = f"https://github.com/{repository}.git"
    record = {"name": name, "source": url, "path": path.relative_to(ROOT).as_posix(),
              "purpose": purpose, "runtime_dependency": False}
    try:
        if not path.exists():
            if not sync:
                return {**record, "status": "not-acquired"}
            log_path = LOGS / f"20261116_reference_{name}.log"
            with log_path.open("w", encoding="utf-8") as log:
                result = subprocess.run(
                    ["git", "clone", "--filter=blob:none", "--no-checkout", "--depth", "1",
                     url, str(path)], stdout=log, stderr=log, timeout=180,
                )
                if result.returncode:
                    raise RuntimeError(f"clone failed; see {log_path.relative_to(ROOT).as_posix()}")
            if sparse:
                git(path, "sparse-checkout", "set", sparse)
            commit = git(path, "rev-parse", "HEAD")
            git(path, "checkout", "--detach", commit)
        record["commit"] = git(path, "rev-parse", "HEAD")
        record["origin"] = git(path, "remote", "get-url", "origin")
        record["local_changes"] = git(path, "status", "--porcelain").splitlines()
        record["sparse_scope"] = sparse
        licenses = []
        for folder in [path, path / sparse if sparse else path]:
            if not folder.is_dir():
                continue
            for candidate in folder.iterdir():
                lower = candidate.name.lower()
                if candidate.is_file() and any(lower.startswith(prefix) for prefix in ("license", "licence", "copying")):
                    data = candidate.read_bytes()
                    licenses.append({
                        "path": candidate.relative_to(ROOT).as_posix(),
                        "sha256": hashlib.sha256(data).hexdigest(),
                        "first_line": data.decode("utf-8-sig", "replace").strip().splitlines()[0] if data else "",
                    })
        record["license_files"] = list({item["path"]: item for item in licenses}.values())
        first_lines = " ".join(item["first_line"].lower() for item in record["license_files"])
        if "mit license" in first_lines:
            record["license"] = "MIT"
        elif "gnu lesser general public license" in first_lines:
            record["license"] = "LGPL-2.1 (consult retained license)"
        elif "attribution-noncommercial-sharealike 4.0" in first_lines:
            record["license"] = "CC-BY-NC-SA-4.0; reference-only-no-code-copy"
        elif record["license_files"]:
            record["license"] = "consult-retained-license"
        else:
            record["license"] = "not-established-no-code-copy"
        record["status"] = "pinned"
    except (RuntimeError, OSError, subprocess.TimeoutExpired) as error:
        record["status"] = "acquisition-or-inventory-error"
        record["error"] = str(error)
    return record


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sync", action="store_true")
    args = parser.parse_args()
    with ThreadPoolExecutor(max_workers=4) as pool:
        records = list(pool.map(lambda source: acquire(source, args.sync), SOURCES))
    manifest = {
        "schema": "cs2-reference-snapshots/v1",
        "generated_at": datetime.now(ZoneInfo("Asia/Shanghai")).isoformat(timespec="seconds"),
        "policy": "research-only; no reference project is a runtime dependency",
        "sources": records,
    }
    (REFERENCE / "manifest.json").write_text(
        json.dumps(manifest, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )
    print(json.dumps(records, ensure_ascii=True, indent=2))


if __name__ == "__main__":
    main()

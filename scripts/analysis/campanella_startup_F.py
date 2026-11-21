"""One authorized diagnostic player run for startup inventory index18.

Never launches the original game or rebuilds the player. Existing output
refuses a second invocation; the caller must authorize any later run.
"""
from __future__ import annotations

import datetime
import json
import os
from pathlib import Path
import subprocess
import time


ROOT = Path(__file__).resolve().parents[2]
OUTPUT = ROOT / "archive/2026-11/20261118_startup_batch"
INVENTORY = ROOT / "reports/startup_inventory_20261116.json"
EXECUTABLE = ROOT / "archive/2026-11/20261118_011723_714911_workspace_pipeline_integration/cs2-player.exe"


def main() -> None:
    inventory = json.loads(INVENTORY.read_text(encoding="utf-8"))
    game = inventory["games"][18]
    if Path(game["root"]).name != "\u795d\u798f\u306e\u30ab\u30f3\u30d1\u30cd\u30e9":
        raise ValueError("Inventory index18 is no longer the assigned Campanella sample")
    if not OUTPUT.is_dir() or not EXECUTABLE.is_file():
        raise FileNotFoundError("Expected existing diagnostic binary and output directory")
    for suffix in ("invocation.json", "stdout.log", "stderr.log", "exit.json", "diagnostic.json"):
        if (OUTPUT / f"campanella.{suffix}").exists():
            raise FileExistsError("Campanella diagnostic output already exists; do not run twice")
    command = [str(EXECUTABLE), "run", game["root"],
               "--frames", "120", "--budget", "20000", "--json"]
    environment = os.environ.copy()
    path_key = next((key for key in environment if key.upper() == "PATH"), "PATH")
    environment[path_key] = "C:/msys64/mingw64/bin;" + environment.get(path_key, "")
    record = {
        "inventory": str(INVENTORY), "inventory_index_0based": 18,
        "inventory_item_1based": 19, "sample": game["root"],
        "configured_entry": game["configured_entry"],
        "command": command, "cwd": str(ROOT),
        "path_prefix": "C:/msys64/mingw64/bin", "timeout_seconds": 600,
        "started_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "original_game_executed": False, "player_rebuilt": False,
        "verification_scope": "One prebuilt diagnostic startup run; not full-game or latest-source acceptance",
    }
    (OUTPUT / "campanella.invocation.json").write_text(
        json.dumps(record, ensure_ascii=True, indent=2) + "\n", encoding="utf-8")
    started = time.monotonic()
    with (OUTPUT / "campanella.stdout.log").open("xb") as stdout, \
            (OUTPUT / "campanella.stderr.log").open("xb") as stderr:
        try:
            result = subprocess.run(command, cwd=ROOT, env=environment,
                                    stdout=stdout, stderr=stderr, timeout=600, check=False)
            record.update(status="exited", returncode=result.returncode)
        except subprocess.TimeoutExpired:
            record.update(status="timeout_killed", returncode=None)
        except OSError as error:
            record.update(status="launch_failed", returncode=None, launch_error=repr(error))
    record["elapsed_seconds"] = round(time.monotonic() - started, 3)
    record["finished_utc"] = datetime.datetime.now(datetime.timezone.utc).isoformat()
    (OUTPUT / "campanella.exit.json").write_text(
        json.dumps(record, ensure_ascii=True, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({key: record[key] for key in ("status", "returncode", "elapsed_seconds")}))


if __name__ == "__main__":
    main()

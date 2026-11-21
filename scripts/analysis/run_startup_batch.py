"""Run a bounded, noninteractive startup batch with one recorded attempt per sample."""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import subprocess
import time

ROOT = Path(__file__).resolve().parents[2]
STATUS_DLL_INIT_FAILED_LOGOFF = 0xC000026B
WINDOWS_INTERRUPTION_CODES = {
    STATUS_DLL_INIT_FAILED_LOGOFF: "STATUS_DLL_INIT_FAILED_LOGOFF",
    0x40010004: "DBG_TERMINATE_PROCESS",
}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--player", type=Path, required=True)
    parser.add_argument("--indices", type=int, nargs="+", required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--frames", type=int, default=120)
    parser.add_argument("--timeout", type=int, default=240)
    args = parser.parse_args()
    player = args.player.resolve(strict=True)
    inventory = ROOT / "reports/startup_inventory_20261116.json"
    games = json.loads(inventory.read_text(encoding="utf-8"))["games"]
    provenance = json.loads((player.parent / "build.json").read_text(encoding="utf-8"))
    if provenance.get("status") != "built" or not provenance.get("source_consistent"):
        raise ValueError("Select a completed, source-consistent integration build")
    args.output.mkdir(parents=True, exist_ok=True)
    environment = os.environ.copy()
    path_key = next((key for key in environment if key.upper() == "PATH"), "PATH")
    environment[path_key] = "C:/msys64/mingw64/bin;" + environment.get(path_key, "")
    environment["CS2_LOG_PROGRESS"] = "1"
    for index in args.indices:
        game = games[index]
        prefix = args.output / f"{index:02d}"
        command = [str(player), "log", game["root"], "--frames", str(args.frames),
                   "--budget", "20000", "--json"]
        record = {"inventory_index": index, "root": game["root"], "command": command,
                  "build": str(player.parent / "build.json"),
                  "source_sha256": provenance["source_before"]["sha256"],
                  "scope": "bounded startup only; no complete-game acceptance"}
        record_path = prefix.with_suffix(".run.json")
        with record_path.open("x", encoding="utf-8") as stream:
            json.dump(record, stream, ensure_ascii=True, indent=2)
        started = time.monotonic()
        with prefix.with_suffix(".stdout.json").open("xb") as stdout, \
                prefix.with_suffix(".stderr.txt").open("xb") as stderr:
            try:
                result = subprocess.run(command, cwd=ROOT, env=environment,
                                        stdout=stdout, stderr=stderr, timeout=args.timeout)
                code = result.returncode & 0xFFFFFFFF
                interruption = WINDOWS_INTERRUPTION_CODES.get(code) if os.name == "nt" else None
                record.update(status="interrupted" if interruption else "exited",
                              exit_code=result.returncode)
                if interruption:
                    record["termination_reason"] = interruption
            except subprocess.TimeoutExpired:
                record.update(status="timeout_killed", exit_code=None)
        record["elapsed_seconds"] = round(time.monotonic() - started, 2)
        record_path.write_text(json.dumps(record, ensure_ascii=True, indent=2) + "\n",
                               encoding="utf-8")
        summary = {key: record[key] for key in
                   ("inventory_index", "status", "exit_code", "elapsed_seconds")}
        if "termination_reason" in record:
            summary["termination_reason"] = record["termination_reason"]
        print(json.dumps(summary), flush=True)
        if record.get("termination_reason"):
            break


if __name__ == "__main__":
    main()

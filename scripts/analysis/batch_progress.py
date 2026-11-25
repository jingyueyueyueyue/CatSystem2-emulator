"""Persist the active PC playthrough campaign across replay builds."""

from collections import Counter
import json
import os
from pathlib import Path
import re
import tempfile


SCHEMA = "cs2-batch-progress/v1"
STATUSES = {"pending", "running", "blocked", "cleared"}
BLOCKED = {"engine_error", "process_error", "watchdog_stalled", "frame_stalled", "unexpected_exit"}


class BatchProgress:
    def __init__(self, path):
        self.path = path.resolve()
        self.lock = None

    def __enter__(self):
        self.path.parent.mkdir(parents=True, exist_ok=True)
        self.lock = self.path.with_suffix(self.path.suffix + ".lock").open("a+b")
        try:
            if os.name == "nt":
                import msvcrt
                self.lock.seek(0)
                msvcrt.locking(self.lock.fileno(), msvcrt.LK_NBLCK, 1)
            else:
                import fcntl
                fcntl.flock(self.lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError as error:
            self.lock.close()
            raise ValueError(f"Progress ledger is already in use: {self.path}") from error
        try:
            self.rows = self.read()
        except BaseException:
            self.lock.close()
            raise
        return self

    def __exit__(self, *_):
        self.lock.close()

    def read(self):
        try:
            data = json.loads(self.path.read_text(encoding="utf-8"))
        except FileNotFoundError:
            return {}
        if not isinstance(data, dict) or data.get("schema") != SCHEMA or not isinstance(data.get("games"), list):
            raise ValueError(f"Invalid progress ledger: {self.path}")
        rows = {}
        for row in data["games"]:
            if (not isinstance(row, dict) or not isinstance(row.get("game_id"), str)
                    or row.get("status") not in STATUSES or row["game_id"] in rows
                    or any(field not in row for field in ("last_checkpoint", "blocker_signature", "fix_ref"))
                    or (row["status"] == "cleared" and not row.get("clear_evidence"))):
                raise ValueError(f"Invalid game in progress ledger: {self.path}")
            rows[row["game_id"]] = row
        return rows

    def sync_inventory(self, games, indices):
        for index in indices:
            game_id = games[index]["root"]
            row = self.rows.setdefault(game_id, {
                "game_id": game_id, "status": "pending", "last_checkpoint": None,
                "blocker_signature": None, "fix_ref": None,
            })
            row["index"] = index
            if row["status"] == "running":
                receipt = Path(row["attempt_receipt"])
                if receipt.is_file():
                    self.finish(game_id, json.loads(receipt.read_text(encoding="utf-8")), receipt)
                else:
                    row["status"] = row.pop("previous_status", "pending")
        self.write()

    def cleared(self, game_id):
        row = self.rows[game_id]
        return row if row["status"] == "cleared" else None

    def prioritize_blockers(self, cases):
        def signature(case):
            row = self.rows[case[1]["root"]]
            return row["blocker_signature"] if row["status"] == "blocked" else None

        counts = Counter(key for case in cases if (key := signature(case)) is not None)
        return sorted(cases, key=lambda case: -counts[signature(case)])

    def start(self, game_id, receipt):
        row = self.rows[game_id]
        row.update(previous_status=row["status"], status="running", attempt_receipt=str(receipt))
        self.write()

    def finish(self, game_id, result, receipt):
        row = self.rows[game_id]
        status = result["status"]
        previous = row.pop("previous_status", row["status"])
        row.update(attempt_receipt=str(receipt), last_result=status)
        if status in {"ending_verified", "skipped_cleared"}:
            row.update(status="cleared", ending=result["ending"], blocker_signature=None,
                       clear_evidence=result.get("ending_receipt", str(receipt)))
        elif status in BLOCKED:
            row.update(status="blocked", blocker_signature=blocker_signature(result))
        elif status in {"checkpoint_reached", "frame_limit", "timed_out_progressing"}:
            row.update(status="pending", blocker_signature=None)
        else:
            row["status"] = previous
        position = result.get("last_story_position")
        if position and position.get("scene") not in (None, "-") and position.get("record", -1) >= 0:
            observed = {
                **position, "events": result.get("stdout"), "receipt": str(receipt),
            }
            row["last_observed_position"] = observed
            if not (row["last_checkpoint"] or {}).get("resume_save"):
                row["last_checkpoint"] = observed
        self.write()

    def write(self):
        fd, temporary = tempfile.mkstemp(dir=self.path.parent, prefix=self.path.name + ".", suffix=".tmp")
        try:
            with os.fdopen(fd, "w", encoding="utf-8", newline="\n") as stream:
                rows = sorted(self.rows.values(), key=lambda row: (row.get("index", 0), row["game_id"]))
                json.dump({"schema": SCHEMA, "games": rows}, stream, ensure_ascii=False, indent=2)
                stream.write("\n")
                stream.flush()
                os.fsync(stream.fileno())
            os.replace(temporary, self.path)
        finally:
            Path(temporary).unlink(missing_ok=True)


def blocker_signature(result):
    error = result.get("first_engine_error")
    if not error:
        return result["status"]
    detail = re.sub(r"^(?:task=\d+:\s*|pc=\S+.*?\b(?:invalid|unsupported) )", "", error["detail"])
    detail = re.sub(r"\b0x[0-9a-fA-F]+\b", "<offset>", detail)
    return f"{error['kind']}: {detail}"

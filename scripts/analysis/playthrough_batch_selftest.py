"""Offline tests for the PC replay batch runner; never launch a player."""

from contextlib import redirect_stderr, redirect_stdout
from concurrent.futures import Future
import hashlib
import io
import json
import os
from pathlib import Path
import struct
from tempfile import TemporaryDirectory
from types import SimpleNamespace
import unittest
from unittest.mock import patch
import zlib

import run_playthrough_batch as batch


class PlaythroughBatchSelfTest(unittest.TestCase):
    def setUp(self):
        temporary = TemporaryDirectory(prefix="playthrough-batch-selftest-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        root_patch = patch.object(batch, "ROOT", self.root)
        root_patch.start()
        self.addCleanup(root_patch.stop)
        self.game = self.root / "games" / "synthetic"
        self.game.mkdir(parents=True)
        self.resource = self.game / "startup.xml"
        self.resource.write_bytes(b"synthetic resource")
        archive = self.root / "archive" / "synthetic"
        archive.mkdir(parents=True)
        self.source = archive / "save0000.dat"
        payload = b"synthetic test data; not a playable save"
        compressed = zlib.compress(payload)
        self.content = struct.pack("<4sIII", b"CSS\0", len(compressed), len(payload), 1000) + compressed
        self.source.write_bytes(self.content)
        self.source.with_name("savegen.dat").write_bytes(b"must not be copied")
        self.player = archive / "story_replay.exe"
        self.player.write_bytes(b"not an executable")
        self.provenance = self.player.with_name("build.json")
        self.output = self.root / "batch"
        self.output.mkdir()
        self.profile = self.output / "20.profile"
        self.game_entry = {"root": "games/synthetic"}
        consumer = {"sha256": batch.file_sha256(self.player), "source_tree_sha256": "2" * 64}
        batch.write_json(self.provenance, {
            "schema": "cs2-build-attempt/v1", "build_stage": "integration",
            "status": "built", "source_consistent": True,
            "source_before": {"sha256": consumer["source_tree_sha256"]},
            "source_after": {"sha256": consumer["source_tree_sha256"]},
            "artifacts": {str(self.player): {"sha256": consumer["sha256"]}},
        })
        self.save = {
            "kind": "ordinary", "platform": "windows-pc", "game": self.game_entry["root"],
            "resource_tree_sha256": batch.resource_tree_sha256(self.game),
            "producer": {"sha256": "1" * 64, "source_tree_sha256": "1" * 64},
            "consumer": consumer, "slot": 0, "file": "archive/synthetic/save0000.dat",
            "size": len(self.content), "sha256": hashlib.sha256(self.content).hexdigest(),
            "evidence": {"write_receipt": "synthetic write", "reopen_receipt": "synthetic reopen",
                         "resume_assertion": "synthetic text anchor"},
        }

    def run_case(self, **kwargs):
        return batch.run_case(20, self.game_entry, 12, [], self.player, self.output, 30, {}, **kwargs)

    def stage(self, save=None):
        return batch.stage_ordinary_save(self.save if save is None else save,
                                         self.game, self.player, self.profile)

    def test_only_approved_slot_staged_before_spawn(self):
        def spawn(command, **kwargs):
            self.assertEqual(Path(command[2]).with_suffix(".profile"), self.profile)
            self.assertEqual(list(self.profile.iterdir()), [self.profile / self.source.name])
            self.assertEqual(self.source.read_bytes(), self.content)
            self.assertEqual(batch.file_sha256(self.profile / self.source.name), self.save["sha256"])
            raise OSError("synthetic launch failure")

        with patch.object(batch.subprocess, "Popen", side_effect=spawn) as popen:
            receipt = self.run_case(ordinary_save=("synthetic", self.save))
        popen.assert_called_once()
        self.assertEqual(receipt["ordinary_save"]["status"], "staged")
        self.assertEqual(receipt["status"], "launch_error")

    def test_existing_profile_is_not_modified(self):
        self.profile.mkdir()
        original = self.profile / self.source.name
        original.write_bytes(b"leave untouched")
        with self.assertRaises(FileExistsError):
            self.stage()
        self.assertEqual(original.read_bytes(), b"leave untouched")

    def test_consumer_and_producer_identity_required(self):
        mutated = {**self.save, "consumer": {**self.save["consumer"], "source_tree_sha256": "3" * 64}}
        with patch.object(batch.subprocess, "Popen") as popen:
            result = self.run_case(ordinary_save=("synthetic", mutated))
        popen.assert_not_called()
        self.assertEqual(result["status"], "ordinary_save_rejected")
        self.assertIn("consumer build identity mismatch", result["ordinary_save"]["reason"])
        with self.assertRaisesRegex(ValueError, "producer build identity"):
            self.stage({**self.save, "producer": {}})
        self.assertFalse(self.profile.exists())

    def test_resource_and_save_hashes_checked(self):
        with self.assertRaisesRegex(ValueError, "game identity mismatch"):
            self.stage({**self.save, "game": "games/other"})
        with self.assertRaisesRegex(ValueError, "source SHA-256 mismatch"):
            self.stage({**self.save, "sha256": "3" * 64})
        self.resource.write_bytes(b"resource changed")
        with self.assertRaisesRegex(ValueError, "resource tree SHA-256 mismatch"):
            self.stage()
        self.assertFalse(self.profile.exists())

    def test_copy_rehash_failure_does_not_spawn(self):
        corrupted = bytes([self.content[0] ^ 1]) + self.content[1:]
        with patch.object(batch.shutil, "copyfileobj", side_effect=lambda source, output: output.write(corrupted)), \
                patch.object(batch.subprocess, "Popen") as popen:
            result = self.run_case(ordinary_save=("synthetic", self.save))
        popen.assert_not_called()
        self.assertEqual(result["status"], "ordinary_save_rejected")
        self.assertIn("copy does not match", result["ordinary_save"]["reason"])
        self.assertEqual(self.source.read_bytes(), self.content)

    def test_rejects_traversal_wrong_filename_and_linked_slot(self):
        with self.assertRaisesRegex(ValueError, "workspace-relative"):
            self.stage({**self.save, "file": "archive/../synthetic/save0000.dat"})
        with self.assertRaisesRegex(ValueError, "approved slot"):
            self.stage({**self.save, "slot": 1})
        os.link(self.source, self.source.with_name("linked.dat"))
        with self.assertRaisesRegex(ValueError, "regular unlinked file"):
            self.stage()
        self.assertFalse(self.profile.exists())

    def test_missing_provenance_rejects_seed_but_preserves_default_behavior(self):
        self.provenance.unlink()
        self.assertNotIn("source_tree_sha256", batch.identify_player(self.player))
        with self.assertRaises(FileNotFoundError):
            self.stage()
        with patch.object(batch.subprocess, "Popen", side_effect=OSError("synthetic")) as popen:
            result = self.run_case()
        popen.assert_called_once()
        self.assertEqual(result["status"], "launch_error")
        self.assertNotIn("ordinary_save", result)
        self.assertFalse(self.profile.exists())

    def test_scenario_groups_share_one_selection(self):
        first = self.root / "first.json"
        second = self.root / "second.json"
        opening = {"frames": 600, "route": {"label": "opening"}}
        control = {"frames": 240, "inputs": ["capture-every:0"]}
        batch.write_json(first, {"1": opening, "3": {}, "notes": "first lane"})
        batch.write_json(second, {"2": control, "3": {}, "notes": "second lane"})
        self.assertEqual(batch.load_scenarios([first, second], [1, 2]), {"1": opening, "2": control})
        with self.assertRaisesRegex(ValueError, "scenario 3 occurs in both"):
            batch.load_scenarios([first, second], [1, 2, 3])
        with self.assertRaisesRegex(ValueError, "no scenario provided for selected indices: 4"):
            batch.load_scenarios([first, second], [1, 2, 4])
        self.assertEqual(batch.load_scenarios([first], [1, 2]), {"1": opening})

    def test_fast_text_updates_only_message_speed_in_staged_setup(self):
        self.profile.mkdir()
        setup = self.profile / "setup.xml"
        setup.write_text(
            "<document><language><id><main>2</main></id></language>"
            "<MESSAGE><speed>4</speed><waitpcm>1</waitpcm></MESSAGE></document>", encoding="utf-8",
        )
        receipt = batch.seed_fast_text_profile(self.profile)
        document = batch.ET.parse(setup).getroot()
        self.assertEqual(document.findtext("MESSAGE/speed"), "0")
        self.assertEqual(document.findtext("MESSAGE/waitpcm"), "1")
        self.assertEqual(document.findtext("language/id/main"), "2")
        self.assertEqual(receipt, {"message_speed": 0, "setup_sha256": batch.file_sha256(setup)})

    def test_same_attempt_reuses_stop_and_input_change_replays(self):
        cache = self.root / "playthrough"
        old = cache / "2026-11" / "20261121_old_playthrough"
        old.mkdir(parents=True)
        for log in ("events", "host"):
            (old / log).touch()
        route = {"label": "unrecognized wait"}
        player_sha256 = batch.file_sha256(self.player)
        key = batch.attempt_key(player_sha256, self.game, [12, route, [], ()], 30, 30, {},
                                None, None, None)
        batch.write_json(old / "20.json", {
            "index": 20, "attempt_key": key, "status": "unrecognized_wait", "seconds": 12,
            "stdout": str(old / "events"), "stderr": str(old / "host"),
        })
        history = batch.load_attempt_history(cache, [20])
        with patch.object(batch.subprocess, "Popen") as popen:
            result = self.run_case(route=route, reuse_attempts=(player_sha256, history))
        popen.assert_not_called()
        self.assertEqual(result["status"], "skipped_attempt")
        self.assertEqual(result["previous_receipt"], str(old / "20.json"))

        self.output = self.output.with_name("next-batch")
        self.output.mkdir()
        self.resource.write_bytes(b"new resource version")
        with patch.object(batch.subprocess, "Popen", side_effect=OSError("test launch")) as popen:
            result = self.run_case(route=route, reuse_attempts=(player_sha256, history))
        popen.assert_called_once()
        self.assertEqual(result["status"], "launch_error")
        self.assertNotEqual(result["attempt_key"], key)

    def test_reuse_completed_frame_run_requires_full_frames_and_evidence(self):
        old = self.root / "playthrough" / "2026-11" / "20261124_old_playthrough"
        old.mkdir(parents=True)
        for name in ("20.events.log", "20.host.log", "20.0012.png"):
            (old / name).touch()
        key = "b" * 64
        receipt = {
            "index": 20, "attempt_key": key, "status": "visual_review_required", "seconds": 12,
            "command": [str(self.player), str(self.game), str(old / "20"), "12"],
            "output": str(old), "captures": ["20.0012.png"],
            "stdout": str(old / "20.events.log"), "stderr": str(old / "20.host.log"),
            "exit_code": 0, "last_frame": 12,
        }
        path = old / "20.json"
        batch.write_json(path, receipt)
        self.assertIn((20, key), batch.load_attempt_history(old.parents[1], [20]))
        (old / "20.0012.png").unlink()
        self.assertNotIn((20, key), batch.load_attempt_history(old.parents[1], [20]))
        (old / "20.0012.png").touch()
        path.unlink()
        batch.write_json(path, {**receipt, "last_frame": 11})
        self.assertNotIn((20, key), batch.load_attempt_history(old.parents[1], [20]))

    def test_missing_game_does_not_interrupt_next_case(self):
        cases = [(1, {"root": "games/missing"}, {}), (2, self.game_entry, {})]
        args = SimpleNamespace(jobs=1, frames=12, timeout=30, stall_seconds=10,
                               reserve_ram_gib=1, ram_per_job_gib=1,
                               reserve_disk_gib=1, disk_per_job_gib=1, admission_wait_seconds=0)
        with patch.object(batch, "available_ram_bytes", return_value=8 * batch.GIB), \
                patch.object(batch.shutil, "disk_usage", return_value=SimpleNamespace(free=8 * batch.GIB)), \
                patch.object(batch.subprocess, "Popen", side_effect=OSError("test launch")), \
                redirect_stdout(io.StringIO()):
            self.assertEqual(batch.run_batch(cases, self.player, self.output, args, {}), 1)
        first = json.loads((self.output / "01.json").read_text(encoding="utf-8"))
        second = json.loads((self.output / "02.json").read_text(encoding="utf-8"))
        self.assertEqual(first["status"], "case_setup_failed")
        self.assertEqual(second["status"], "launch_error")

    def test_batch_resumes_after_resources_return(self):
        args = SimpleNamespace(jobs=1, frames=12, timeout=30, stall_seconds=10,
                               reserve_ram_gib=1, ram_per_job_gib=1,
                               reserve_disk_gib=1, disk_per_job_gib=1, admission_wait_seconds=30)
        with patch.object(batch, "available_ram_bytes", side_effect=[batch.GIB, 8 * batch.GIB]), \
                patch.object(batch.shutil, "disk_usage", return_value=SimpleNamespace(free=8 * batch.GIB)), \
                patch.object(batch.time, "sleep") as sleep, \
                patch.object(batch.subprocess, "Popen", side_effect=OSError("test launch")) as popen, \
                redirect_stdout(io.StringIO()):
            self.assertEqual(batch.run_batch([(1, self.game_entry, {})], self.player, self.output, args, {}), 0)
        sleep.assert_called_once_with(15)
        popen.assert_called_once()
        result = json.loads((self.output / "01.json").read_text(encoding="utf-8"))
        self.assertEqual(result["status"], "launch_error")

    def test_batch_rechecks_resources_with_an_active_case(self):
        args = SimpleNamespace(jobs=2, frames=12, timeout=30, stall_seconds=10,
                               reserve_ram_gib=1, ram_per_job_gib=1,
                               reserve_disk_gib=1, disk_per_job_gib=1, admission_wait_seconds=0)
        cases = [(1, self.game_entry, {}), (2, self.game_entry, {})]
        first, second = Future(), Future()
        second.set_result({"index": 2, "status": "checkpoint_reached", "seconds": 1})

        def wait_for_cases(active, *, return_when, timeout):
            self.assertEqual(return_when, batch.FIRST_COMPLETED)
            if not first.done():
                if timeout == batch.ADMISSION_POLL_SECONDS:
                    self.assertEqual(list(active), [first])
                    return set(), {first}
                self.assertEqual(set(active), {first, second})
                first.set_result({"index": 1, "status": "checkpoint_reached", "seconds": 2})
            return set(active), set()

        with patch.object(batch, "available_ram_bytes", side_effect=[4 * batch.GIB, 2 * batch.GIB, 4 * batch.GIB]), \
                patch.object(batch, "ThreadPoolExecutor") as pool, \
                patch.object(batch.shutil, "disk_usage", return_value=SimpleNamespace(free=8 * batch.GIB)), \
                patch.object(batch, "wait", side_effect=wait_for_cases) as wait_cases, \
                redirect_stdout(io.StringIO()):
            pool.return_value.__enter__.return_value.submit.side_effect = [first, second]
            self.assertEqual(batch.run_batch(cases, self.player, self.output, args, {}), 0)
        self.assertEqual(wait_cases.call_count, 2)

    def test_pending_v401_cannot_be_selected(self):
        manifest = Path(__file__).with_name("pc_ordinary_saves.json")
        self.assertEqual(batch.load_ordinary_saves(manifest), {})
        inventory = self.root / "inventory.json"
        scenarios = self.root / "scenarios.json"
        batch.write_json(inventory, {"games": [self.game_entry]})
        batch.write_json(scenarios, {"0": {"ordinary_save": "v401-pc-slot-0"}})
        cli = ["run_playthrough_batch.py", "--player", str(self.player), "--inventory", str(inventory),
               "--scenarios", str(scenarios), "--ordinary-saves", str(manifest)]
        errors = io.StringIO()
        with patch("sys.argv", cli), redirect_stderr(errors), patch.object(batch.subprocess, "Popen") as popen:
            with self.assertRaises(SystemExit) as failure:
                batch.main()
        self.assertEqual(failure.exception.code, 2)
        self.assertIn("must name an approved", errors.getvalue())
        popen.assert_not_called()

    def test_route_diagnostics_reject_fixed_input_and_captures(self):
        inventory = self.root / "inventory.json"
        batch.write_json(inventory, {"games": [self.game_entry]})
        for flag in ("1600:13", "1600:640,360", "inspect:1600", "inspect-log:0"):
            with self.subTest(flag=flag):
                scenarios = self.root / f"scenario_{flag.encode().hex()}.json"
                batch.write_json(scenarios, {"0": {"route": {}, "diagnostics": [flag]}})
                cli = ["run_playthrough_batch.py", "--player", str(self.player), "--inventory", str(inventory),
                       "--scenarios", str(scenarios)]
                with patch("sys.argv", cli), redirect_stderr(io.StringIO()), patch.object(batch.subprocess, "Popen") as popen:
                    with self.assertRaises(SystemExit) as failure:
                        batch.main()
                self.assertEqual(failure.exception.code, 2)
                popen.assert_not_called()

    def test_cli_requires_manifest_and_scenario_opt_in(self):
        inventory, manifest = (self.root / name for name in ("inventory.json", "approved.json"))
        batch.write_json(inventory, {"games": [self.game_entry]})
        batch.write_json(manifest, {"schema": "cs2-ordinary-saves/v1", "approved": {"synthetic": self.save}})
        base = ["run_playthrough_batch.py", "--player", str(self.player), "--inventory", str(inventory),
                "--ordinary-saves", str(manifest)]
        scenarios = self.root / "scenarios.json"
        diagnostics = ["trace-native:530", "inspect-log:1636"]
        batch.write_json(scenarios, {"0": {"ordinary_save": "synthetic", "route": {}, "diagnostics": diagnostics}})
        for seeded, args in ((False, base), (True, base + ["--scenarios", str(scenarios)])):
            with self.subTest(seeded=seeded):
                def spawn(command, **kwargs):
                    profile = Path(command[2]).with_suffix(".profile")
                    self.assertEqual(profile.exists(), seeded)
                    if seeded:
                        self.assertEqual(command[-2:], diagnostics)
                        self.assertEqual((profile / self.source.name).read_bytes(), self.content)
                    raise OSError("synthetic")

                printed = io.StringIO()
                with patch("sys.argv", args), patch.dict(os.environ, {"CS2_BUILD_CACHE_ROOT": str(self.root)}), \
                        patch.object(batch, "available_ram_bytes", return_value=100 * batch.GIB), \
                        patch.object(batch.shutil, "disk_usage", return_value=SimpleNamespace(free=100 * batch.GIB)), \
                        patch.object(batch.subprocess, "Popen", side_effect=spawn) as popen, redirect_stdout(printed):
                    self.assertEqual(batch.main(), 0)
                popen.assert_called_once()
                output = Path(printed.getvalue().splitlines()[0])
                receipt = json.loads((output / "00.json").read_text(encoding="utf-8"))
                self.assertEqual("ordinary_save" in receipt, seeded)

    def test_sparse_state_heartbeat_and_engine_error_keep_max_frame(self):
        events = self.output / "events.log"
        progress = batch.ReplayProgress(state_mode=True)
        with events.open("wb", buffering=0) as writer, events.open("rb") as reader:
            writer.write(b"replay frame=1680 heartbeat\nframe=1720 message-resume synthetic\n")
            progress.sample(reader)
            self.assertEqual(progress.frame, 1720)
            writer.write(b"story frame=1750 state=ready scene=synthetic\nframe=1774 task-error pending")
            progress.sample(reader)
            self.assertEqual(progress.frame, 1750)
            self.assertIsNone(progress.first_engine_error)
            writer.write(b"\r\nreplay frame=1680 heartbeat\nreplay_status=engine_error\n")
            progress.sample(reader)
            self.assertEqual(progress.frame, 1774)
            self.assertEqual(progress.first_engine_error, {
                "frame": 1774, "kind": "task-error", "detail": "pending", "line": 4,
            })
            self.assertEqual(progress.replay_status, "engine_error")
            last_advance = progress.last_advance
            writer.write(b"frame=1770 message-resume old\n")
            progress.sample(reader)
            self.assertEqual(progress.frame, 1774)
            self.assertEqual(progress.last_advance, last_advance)

    def test_legacy_engine_error_counts_even_without_tick(self):
        events = self.output / "events.log"
        events.write_bytes(b"frame=0 logic-frame tick\nframe=12 vm-error failure\nframe=11 logic-frame tick\n")
        with events.open("rb") as stream:
            progress = batch.ReplayProgress(state_mode=False)
            progress.sample(stream)
        self.assertEqual(progress.frame, 12)
        self.assertEqual(progress.first_engine_error["frame"], 12)

    def test_legacy_hit_guard_stop_is_reported_separately_from_engine_errors(self):
        events = self.output / "events.log"
        events.write_bytes(b"frame=12 logic-frame tick\nreplay_status=input_missed\n")
        with events.open("rb") as stream:
            progress = batch.ReplayProgress(state_mode=False)
            progress.sample(stream)
        self.assertEqual(progress.frame, 12)
        self.assertEqual(progress.replay_status, "input_missed")
        self.assertIsNone(progress.first_engine_error)

    def test_receipt_distinguishes_execution_outcomes_in_both_modes(self):
        events = self.output / "events.log"
        cases = (
            (b"replay_status=input_missed\n", 0, False, 0, "input_missed"),
            (b"replay_status=frame_stalled\n", 0, False, 0, "frame_stalled"),
            (b"replay_status=scenario_error\n", 1, False, None, "scenario_error"),
            (b"replay_status=engine_error\n", 1, False, None, "engine_error"),
            (b"frame=12 task-error failed\n", None, True, 10, "engine_error"),
            (b"frame=12 logic-frame tick\n", None, True, 2, "timed_out_progressing"),
            (b"frame=12 logic-frame tick\n", None, True, 10, "timed_out_stalled"),
            (b"", None, True, None, "timed_out_no_frame"),
            (b"", 1, False, None, "process_error"),
        )
        for state_mode in (False, True):
            for log, exit_code, timed_out, idle, expected in cases:
                with self.subTest(state_mode=state_mode, expected=expected):
                    events.write_bytes(log)
                    progress = batch.ReplayProgress(state_mode)
                    with events.open("rb") as stream:
                        progress.sample(stream)
                    self.assertEqual(progress.status(save_rejected=False, timed_out=timed_out,
                                                     exit_code=exit_code, frame_idle_seconds=idle,
                                                     stall_window=5), expected)

    def test_finished_state_route_records_a_checkpoint(self):
        events = self.output / "events.log"
        events.write_bytes(b'replay frame=74 state=page scene=after the rain group=2 record=17\n'
                           b'story frame=70 state=active scene=previous group=1 record=15\n'
                           b'replay frame=78 state=unknown scene=- group=-1 record=-1\n'
                           b'replay frame=80 checkpoint="stop at the next page"\nreplay_status=completed\n')
        with events.open("rb") as stream:
            progress = batch.ReplayProgress(state_mode=True)
            progress.sample(stream)
            self.assertEqual(progress.replay_status, "completed")
            self.assertEqual(progress.last_story_position["state"], "unknown")
            self.assertEqual(progress.last_page_wait, {"frame": 74, "state": "page",
                             "scene": "after the rain", "group": 2, "record": 17, "line": 1})
        self.assertEqual(progress.status(save_rejected=False, timed_out=False, exit_code=0,
                                         frame_idle_seconds=0, stall_window=5), "checkpoint_reached")

    def test_route_stop_reads_final_wait_and_ignores_successful_input_receipts(self):
        host = self.output / "host.log"
        host.write_bytes(b"replay frame=100: verified keyboard input\r\n")
        self.assertIsNone(batch.route_stop_diagnostics(host))
        with host.open("ab") as stream:
            stream.write(b"replay frame=113: unexpected branch\r\nreplay wait: pending message\r\n")
        self.assertEqual(batch.route_stop_diagnostics(host), {"frame": 113, "reason": "unexpected branch",
                                                            "wait": "pending message"})

    def test_guarded_frame_wait_survives_trailing_movie_diagnostics(self):
        host = self.output / "host.log"
        host.write_bytes(b"replay wait: frame=2700 movies=1\r\n"
                         b"frame 2700: movie handle=384 active=true\r\n")
        self.assertEqual(batch.route_stop_diagnostics(host), {"wait": "frame=2700 movies=1"})

    def test_legacy_frame_entry_can_finish_with_a_guarded_checkpoint(self):
        events = self.output / "events.log"
        events.write_bytes(b"replay_status=completed\n")
        for guarded, expected in [(False, "visual_review_required"), (True, "checkpoint_reached")]:
            with self.subTest(guarded=guarded), events.open("rb") as stream:
                progress = batch.ReplayProgress(state_mode=False, guarded_body=guarded)
                progress.sample(stream)
                self.assertEqual(progress.status(save_rejected=False, timed_out=False, exit_code=0,
                                                 frame_idle_seconds=0, stall_window=5), expected)

    def test_registered_story_continuations_are_guarded(self):
        for flag in ("legacy-13-r139", "legacy-13-r188", "labyrinth15-next", "labyrinth15-opening",
                     "labyrinth15-r19-enter-observe",
                     "legacy-11-radio-r94-enter", "djibril10-op01-next", "legacy-04-segment", "legacy-05-segment"):
            with self.subTest(flag=flag):
                self.assertIsNotNone(batch.GUARDED_BODY_INPUT.fullmatch(flag))
        self.assertIsNone(batch.GUARDED_BODY_INPUT.fullmatch("inspect-log:3186"))

    def test_ending_needs_prior_card_with_the_same_id(self):
        events = self.output / "events.log"
        events.write_bytes(b"replay frame=10 ending_verified=BAD_02\n"
                           b"replay frame=20 ending_card=BAD_02\n"
                           b"replay frame=30 ending_verified=BAD_01\n")
        progress = batch.ReplayProgress(state_mode=True)
        with events.open("rb") as stream:
            progress.sample(stream)
            self.assertIsNone(progress.verified_ending)
            with events.open("ab") as append:
                append.write(b"replay frame=40 ending_verified=BAD_02\n")
            progress.sample(stream)
        self.assertEqual(progress.verified_ending, {"id": "BAD_02", "card_frame": 20,
                                                    "clear_verified_frame": 40, "log_line": 4})

    def test_campaign_skips_a_verified_game_with_archived_evidence(self):
        cache = self.root / "playthrough"
        run = cache / "2026-11" / "verified_playthrough"
        case = run / "01"
        case.mkdir(parents=True)
        card = case / "01.0020.png"
        card.touch()
        events = case / "01.events.log"
        ending_log = (b"replay frame=20 ending_card=BAD_02\nreplay frame=40 ending_verified=BAD_02\n"
                      b"replay_status=completed\n")
        events.write_bytes(ending_log)
        batch.write_json(run / "build_identity.json", self.save["consumer"])
        ending = {"id": "BAD_02", "card_frame": 20, "clear_verified_frame": 40, "card": card.name, "log_line": 2}
        receipt = run / "01.json"
        batch.write_json(receipt, {"index": 1, "game": str(self.game), "status": "ending_verified",
                                   "exit_code": 0, "replay_status": "completed", "ending": ending})
        same_inventory = [{}, self.game_entry]
        cleared = batch.find_cleared_cases(same_inventory, [1], cache)
        self.assertEqual(cleared[1]["ending_receipt"], str(receipt))
        self.assertEqual(cleared[1]["ending"], ending)
        self.assertEqual(cleared[1]["status"], "skipped_cleared")
        self.assertEqual(batch.find_cleared_cases([{}, {"root": "games/other"}], [1], cache), {})
        events.write_bytes(ending_log.replace(b"ending_verified=BAD_02", b"ending_verified=BAD_01"))
        self.assertEqual(batch.find_cleared_cases(same_inventory, [1], cache), {})
        events.write_bytes(ending_log)
        card.unlink()
        self.assertEqual(batch.find_cleared_cases(same_inventory, [1], cache), {})


if __name__ == "__main__":
    unittest.main()

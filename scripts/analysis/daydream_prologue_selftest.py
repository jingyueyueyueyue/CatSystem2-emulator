"""Exercise the sample 17 log checks with in-memory synthetic evidence."""

from collections import defaultdict
import json
import unittest

from check_daydream_prologue import ENTER, PE, QUIET, verify_enter, verify_page, verify_successor
from generate_daydream_r96_route import PLAN, continuation_plan


def probe(group, wait, body, empty, body_bytes):
    return (
        f'trace_enabled=true profile=Some(Daydream) pe_sha256=Some("{PE}") startup=Some("kcs.int/main.kcs") '
        f'frame=210 state=page_probe scene=prologue group={group} record={wait} {QUIET} '
        f'contexts(id,scene,loaded,candidate,page)=[(159, "prologue", 10, None, Some((200, 2, {group}, {wait})))] '
        'windows(owner,name,wait_key,driver(block,pending,wait),novel_ready,novel_prompt)='
        '[(6, "flow", false, Some(("GAME_WAIT", false, Some(Frame { after: 210 }))), false, None), '
        '(65, "cinema.fes", true, Some(("MES_SHOW", false, Some(Frame { after: 210 }))), false, None)] '
        f'body_wait_shell(window,owner,scene,loaded,marked,group,record)=Some(("cinema", 65, "prologue", 10, 200, {group}, {wait})) '
        'body_generation(owner,generation,current_committed)=Some((65, Some(StoryBodyGeneration { '
        'source_epoch: 52, source_frame: 170, natural_complete: Some((52, 205)), committed_epoch: Some(52), '
        'remaining: 0, remaining_segment: 0, pending_text: false, drawing: false, skip_pending: false, forced: false, '
        'projection: Some((true, 255)) }), true)) '
        'body_page(context,sources,exact_match,composited)=Some((159, Some([CstBodySource { '
        f'context_handle: 159, load_id: 1, group: {group}, absolute_record: {body} }}]), true, true)) '
        f'daydream_empty_body(context,body,raw_bytes,empty,raw_bytes)=Some((159, {body}, {body_bytes}, {empty}, 0))'
    )


class DaydreamPrologueTests(unittest.TestCase):
    def test_empty_page_rejects_nonempty_or_stale_source(self):
        for group, wait, body, record, lengths in [
            (20, 96, 90, 95, [38, 48]), (40, 221, 215, 220, [36, 42]),
        ]:
            with self.subTest(wait=wait):
                snapshot = probe(group, wait, body, record, lengths[0])
                position = (210, "page_probe", "prologue", group, wait)
                expected_empty = (wait, body, lengths, record, [0, 0])
                verified = verify_page(position, (1, snapshot), [body], expected_empty, {200}, {210})
                self.assertEqual(verified["empty_body"]["fes_queue_operation"], "unobserved")
                self.assertEqual(verified["body_records"], [body])
                for old, new, reason in [
                    (f"{record}, 0))", f"{record}, 1))", "zero-byte"),
                    (f"absolute_record: {body}", f"absolute_record: {record}", "ordered visible sources"),
                    (f"{record}, 0))", f"{record - 1}, 0))", "zero-byte"),
                    ("committed_epoch: Some(52)", "committed_epoch: Some(51)", "generation"),
                    ("forced: false", "forced: true", "composition are unproven"),
                    ("}]), true, true))", "}]), true, false))", "composition are unproven"),
                ]:
                    with self.subTest(wait=wait, reason=reason):
                        self.assertIn(old, snapshot)
                        with self.assertRaisesRegex(ValueError, reason):
                            verify_page(position, (1, snapshot.replace(old, new)), [body], expected_empty, {200}, {210})

    def test_first_enter_rejects_stale_load_epoch_and_extra_consumers(self):
        before = {"frame": 140, "owner": 65, "wait_record": 83, "context": 159,
                  "load_id": 1, "loaded_frame": 10, "epoch": 51}
        position = (210, "page_probe", "prologue", 20, 96)
        after = {"frame": 210, "owner": 65, "wait_record": 96, "context": 159, "load_id": 1,
                 "loaded_frame": 10, "epoch": 52, "source_frame": 170, "wait_marked_frame": 200}
        receipt = ("141", "2", "0", "1", "1", "false", "62", "8", "0", ENTER)
        routes = {141: [receipt]}
        verified = defaultdict(list, {141: [receipt]})
        armed = defaultdict(list, {140: [65]})
        presses = [(140, "Keyboard(13)")]
        resumes = {141: [(1, "message-resume message=[2, 0, 0, 0]")]}
        advance = verify_enter(before, position, routes, verified, armed, presses, resumes, {141})
        self.assertEqual(advance["to"], 96)
        verify_successor(before, after, advance["receipt_frame"])
        for changes in ({"epoch": 51}, {"load_id": 2}, {"source_frame": 139}):
            with self.subTest(changes=changes), self.assertRaisesRegex(ValueError, "same-load fresh body"):
                verify_successor(before, after | changes, advance["receipt_frame"])
        with self.assertRaisesRegex(ValueError, "unique Enter consumer"):
            verify_enter(before, position, {141: [receipt, receipt]}, verified, armed, presses, resumes, {141})
        with self.assertRaisesRegex(ValueError, "unique same-frame root VM resume"):
            verify_enter(before, position, routes, verified, armed, presses, {141: resumes[141] * 2}, {141})

    def test_continuation_data_requires_empty_records_to_stay_empty(self):
        plan = json.loads(PLAN.read_text(encoding="utf-8"))
        data = continuation_plan(plan)
        self.assertEqual(len(data["edges"]), 20)
        self.assertEqual(data["first_run"], {"from_wait": 96, "terminal_wait": 107, "guarded_enters": 1})
        self.assertEqual(data["terminal_wait"], 221)
        self.assertFalse(data["terminal_input"])
        plan["static_successors"][0]["body_records"].append(95)
        with self.assertRaisesRegex(ValueError, "invalid source records"):
            continuation_plan(plan)


if __name__ == "__main__":
    unittest.main()

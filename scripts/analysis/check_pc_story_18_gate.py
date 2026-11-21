"""Classify Campanella's first body from existing PC logs. Never sends input."""

import argparse
import json
from pathlib import Path
import re
from struct import unpack


PE = '659d5f069f3c97f69a61e12449cf1510b45e48a10649b9ac6977c2b93d6d28ad'
NATIVE = re.compile(r'^frame=(\d+) native-call index=(\d+) arguments=(\d+) bytes \[([0-9a-f]+)\]')
BODY = re.compile(
    r'^message plane (\d+) string kind=45 parent=(\d+) visible=true .*?'
    r'task=Some\(\(45, 0, (\d+)\)\).*?generation=(\d+) submitted_alpha=(\d+) '
    r'opaque_final_rgb_matches=Some\(\((\d+), (\d+)\)\).*?'
    r'draw_origin=Some\(\[(\d+), (\d+)\]\) opaque_texture_viewport_clipped=Some\(\(0, 0\)\)',
    re.MULTILINE,
)
CHECKPOINT = re.compile(r'^frame (\d+):$', re.MULTILINE)
PRESS = re.compile(
    r'InputRouteReceipt \{ frame: (\d+), key: (\d+), group: (\d+), raw_mask: (\d+), '
    r'pressed: ([1-9]\d*), repeated: (true|false), region: (\d+), receiver: (\d+), response: (\d+), '
    r'physical: (\[[^\]\r\n]*\]) \}'
)
ROUTE_FIELDS = ('key', 'group', 'raw_mask', 'region', 'receiver', 'response', 'physical_code')


def successor_gate(host, calls, context, settled_frame, body, verified_binding):
    event_line, body_frame = body[:2]
    expected_chain = [verified_binding] if isinstance(verified_binding, dict) else verified_binding
    if not isinstance(expected_chain, list) or not expected_chain or any(
        not isinstance(item, dict) or set(item) != set(ROUTE_FIELDS)
        or any(type(item[field]) is not int or item[field] < 0 for field in ROUTE_FIELDS)
        or item['physical_code'] != 13 or item['raw_mask'] != 1
        for item in expected_chain
    ):
        return 'live_binding_unproven'
    presses = [match.groups() for match in PRESS.finditer(host) if int(match[1]) > settled_frame]
    if len(presses) != len(expected_chain):
        return 'physical_receipt_unproven'
    press_frame = int(presses[0][0])
    down = re.findall(r'^frame (\d+): down (.*)$', host, re.MULTILINE)
    if (press_frame > body_frame or (str(press_frame - 1), 'Keyboard(13)') not in down
            or [item for item in down if int(item[0]) >= settled_frame] != [(str(press_frame - 1), 'Keyboard(13)')]):
        return 'physical_receipt_unproven'
    for receipt, binding in zip(presses, expected_chain):
        frame, key, group, raw_mask, pressed, repeated, region, receiver, response, physical = receipt
        expected = tuple(binding[field] for field in ROUTE_FIELDS[:-1])
        source = '[RoutedPhysicalButton { source: Keyboard, code: 13 }]'
        if (int(frame) != press_frame or pressed != '1' or repeated != 'false' or physical != source
                or tuple(map(int, (key, group, raw_mask, region, receiver, response))) != expected):
            return 'physical_receipt_unproven'
    next_mark = next((call for call in calls if call[0] > event_line and call[2] == 1
                      and call[4] == ' debug="[input]"'), None)
    if next_mark is None or not any(
        event_line < line < next_mark[0] and frame == next_mark[1] and native == 326 and words == (context, 1, 4)
        for line, frame, native, words, _ in calls
    ):
        return 'g1_body_executed_wait_unproven'
    return 'g1_wait_observed_pixels_unverified'


def classify(result, host, events, verified_binding=None):
    report = {'gate': 'unproven', 'input_authorized': False}
    if (result.get('index') != 18 or result.get('exit_code') != 0 or result.get('timed_out') is not False
            or result.get('first_engine_error') is not None or result.get('replay_status') not in (None, 'frame_limit')):
        return report | {'reason': 'replay failed or ended at another gate'}
    title = re.search(
        r'^frame (\d+): pointer \[183, 434\], routes \[InputRouteReceipt \{ frame: \1, '
        r'key: 0, group: 0, raw_mask: 2, pressed: 1, repeated: false, region: 11, receiver: 7, '
        r'response: 0, physical: \[RoutedPhysicalButton \{ source: Mouse, code: 1 \}.*$',
        host, re.MULTILINE,
    )
    if not title or f'pe_sha256=Some("{PE}") startup=Some("kcs.int/main.kcs")' not in host:
        return report | {'reason': 'missing title receipt or sample identity'}
    title_frame = int(title[1])
    if re.findall(rf'^frame={title_frame} message-resume (.*)$', events, re.MULTILINE) != [
        'message=[2, 183, 434, 0]'
    ]:
        return report | {'reason': 'title click lacks its unique same-frame VM resume'}
    hit_guard = f'require-hit:{title_frame - 1}:11' in result.get('command', [])
    presses = [match.groups() for match in PRESS.finditer(host)]
    if len(presses) != len(re.findall(r'\bpressed: [1-9]\d*,', host)):
        return report | {'reason': 'unrecognized positive input receipt'}
    downs = re.findall(r'^frame (\d+): down (.*)$', host, re.MULTILINE)
    report['g0_no_advance_control'] = (
        downs == [(str(title_frame - 1), 'MouseButton(0)')]
        and presses == [(
            str(title_frame), '0', '0', '2', '1', 'false', '11', '7', '0',
            '[RoutedPhysicalButton { source: Mouse, code: 1 }]',
        )]
    )
    report['title_hit_guard_configured'] = hit_guard
    calls = []
    for line_number, line in enumerate(events.splitlines(), 1):
        match = NATIVE.match(line)
        if not match:
            continue
        frame, native, size = map(int, match.groups()[:3])
        data = bytes.fromhex(match[4])
        if size == len(data) and size % 4 == 0:
            calls.append((line_number, frame, native, unpack(f'<{size // 4}I', data), line[match.end():]))
    loads = [call for call in calls if call[2] == 1 and ' debug="sscript loaded > ' in call[4]]
    if len(loads) != 1 or loads[0][4] != ' debug="sscript loaded > 0001_00"':
        return report | {'reason': 'scene load absent or another scene was loaded'}
    load_line = loads[0][0]
    if not loads[0][1] > title_frame or not any(
        native == 325 and len(words) == 2 and line == load_line - 1
        for line, _, native, words, _ in calls
    ):
        return report | {'reason': 'missing native325 context binding'}
    context = next(words[0] for line, _, _, words, _ in calls if line == load_line - 1)

    def executed_body(group, offset):
        return [
            read for read, debug in zip(calls, calls[1:])
            if read[0] > load_line and read[0] + 1 == debug[0] and read[1] == debug[1]
            and read[2] == 327 and len(read[3]) == 4 and (read[3][0], *read[3][2:]) == (context, group, offset)
            and debug[2] == 1 and debug[4].startswith(' debug="[message] ')
        ]

    first, second = executed_body(0, 88), executed_body(0, 89)
    if len(first) != 1 or len(second) != 1 or first[0][1] != second[0][1] or first[0][0] >= second[0][0]:
        return report | {'reason': 'g0/88 and g0/89 not both seen executing in order'}
    after_message = [call for call in calls if call[0] > second[0][0] + 1 and call[1] == second[0][1]]
    marker = next((call for call in after_message if call[2] == 1 and call[4].startswith(' debug=')), None)
    if not marker or marker[4] != ' debug="[input]"':
        return report | {'reason': 'missing g0 input marker after the two body records'}
    lookup_lines = [
        line for line, frame, native, words, _ in calls
        if second[0][0] + 1 < line < marker[0] and frame == marker[1]
        and native == 326 and words == (context, 0, 90)
    ]
    lookup = bool(lookup_lines)
    report |= {'context': context, 'g0_body_frame': first[0][1], 'g0_wait_record': 90,
               'g0_wait_lookup_traced': lookup, 'g0_wait_lookup_event_lines': lookup_lines,
               'g0_input_marker_event_line': marker[0]}

    next_line = executed_body(1, 1)
    if len(next_line) == 1 and next_line[0][0] > marker[0]:
        report |= {'g1_92': {'status': 'executed_body', 'frame': next_line[0][1], 'event_line': next_line[0][0]}}
    else:
        report['g1_92'] = {'status': 'not_observed' if not next_line else 'ambiguous'}

    snapshots = []
    for string_id, parent, frame, generation, alpha, matched, total, xpos, ypos in (
        tuple(map(int, match.groups())) for match in BODY.finditer(host)
    ):
        if not (alpha >= total > 0 and matched == total and (xpos, ypos) == (200, 487)):
            continue
        lifecycle = re.search(
            rf'^frame=(\d+) task-create id={string_id} kind=45 parent={parent} name="string"', events, re.MULTILINE,
        )
        parent_task = re.search(
            rf'^frame=(\d+) task-create id={parent} kind=2 parent=(\d+) name="message main"', events, re.MULTILINE,
        )
        if not lifecycle or not parent_task or not int(parent_task[1]) <= int(lifecycle[1]) <= first[0][1]:
            continue
        owner = int(parent_task[2])
        owner_task = re.search(
            rf'^frame=(\d+) task-create id={owner} kind=12 parent=\d+ name="meswnd"', events, re.MULTILINE,
        )
        if not owner_task or int(owner_task[1]) > int(parent_task[1]):
            continue
        wait = re.search(rf'^frame {frame}: wait .*focused=true paused=false message_wait=Some\((\d+)\) vm_waiting_for_message=true '
                         r'pending_messages=0 movies=0\b', host, re.MULTILINE)
        queued = any(line > marker[0] and sent_at == first[0][1] and native == 619 and words == (string_id, 2)
                     for line, sent_at, native, words, _ in calls)
        end_line = next_line[0][0] if next_line else float('inf')
        polls = [call for call in calls if call[2:4] == (616, (string_id,)) and marker[0] < call[0] < end_line]
        if not wait or not queued or not polls or polls[-1][1] >= frame or not any(
            line > polls[-1][0] and sent_at == polls[-1][1] and native == 69 and words == (owner, 13, 0, 0)
            for line, sent_at, native, words, _ in calls
        ):
            continue
        snapshots.append((frame, string_id, parent, owner, generation, alpha, total, int(wait[1])))

    check_frames = set(map(int, CHECKPOINT.findall(host)))
    if next_line:
        check_frames = {frame for frame in check_frames if frame < next_line[0][1]}
        snapshots = [entry for entry in snapshots if entry[0] in check_frames]
    if (len(snapshots) >= 2 and len(check_frames) >= 2
            and [entry[0] for entry in snapshots[-2:]] == sorted(check_frames)[-2:]
            and snapshots[-2][1:] == snapshots[-1][1:]):
        start, end = snapshots[-2:]
        report |= {'gate': 'g0_first_body_settled', 'settled_frames': [start[0], end[0]], 'receiver': end[1],
                   'message_main': end[2], 'meswnd_owner': end[3], 'generation': end[4],
                   'submitted_alpha': end[5], 'opaque_rgb': [end[6], end[6]],
                   'origin': [200, 487], 'clipped': [0, 0], 'vm_wait_address': end[7],
                   'g0_wait_evidence': 'lookup_and_settled_body' if lookup else 'lookup_unproven'}
        meswnd = re.search(
            rf'^frame=\d+ task-create id={end[3]} kind=12 parent=(\d+) name="meswnd"', events, re.MULTILINE
        )
        if meswnd:
            ordinary = int(meswnd[1])
            buttons = [
                (int(frame), int(button))
                for frame, button in re.findall(
                    rf'^frame=(\d+) task-create id=(\d+) kind=6 parent={ordinary} name="bottom button"',
                    events, re.MULTILINE,
                )
                if int(frame) <= end[0]
            ]
            report['receiver_preflight'] = {
                'status': 'live_keyboard_routes_unproven',
                'ordinary_script': ordinary,
                'bottom_button_creations': [{'frame': frame, 'id': button} for frame, button in buttons],
                'keyboard_physical_code': 13,
            }
        if len(next_line) == 1 and report['g1_92']['status'] == 'executed_body':
            report['g1_92']['transition_gate'] = (
                successor_gate(host, calls, context, end[0], next_line[0], verified_binding)
                if lookup else 'g0_wait_lookup_unproven'
            )
    else:
        report['reason'] = 'no two matching post-reveal observations of the live g0 receiver'
    return report


def classify_live(result, host, events):
    failed = {'gate': 'unproven', 'input_authorized': False}
    if (result.get('index') != 18 or result.get('mode') != 'state'
            or result.get('status') not in ('completed', 'checkpoint_reached') or result.get('replay_status') != 'completed'
            or result.get('exit_code') != 0 or result.get('timed_out') is not False
            or result.get('first_engine_error') is not None or result.get('captures') != []
            or events.rstrip().splitlines()[-1:] != ['replay_status=completed']):
        return failed | {'reason': f"live route did not complete ({result.get('status')})"}

    body = (r'CampanellaWait \{ context: (\d+), load: (\d+), script: (\d+), owner: (\d+), '
            r'plane: (\d+), string: (\d+), generation: (\d+), marked: (\d+), opaque: (\d+) \}')
    title = re.findall(r'^frame (\d+): 18 verified title click region=11 receiver=(\d+)$', host, re.MULTILINE)
    armed = re.findall(r'^frame (\d+): 18 armed g0/90 body=' + body
                       + r' bottom=\((\d+), (\d+)\) candidates=(\[[\d, ()]+\])$', host, re.MULTILINE)
    verified = re.findall(r'^frame (\d+): 18 verified Enter key=2 group=0 region=(\d+) receiver=(\d+)$',
                          host, re.MULTILINE)
    settled = re.findall(r'^frame (\d+): 18 settled g1/95 after release body=' + body + '$',
                         host, re.MULTILINE)
    if any(len(markers) != 1 for markers in (title, armed, verified, settled)):
        return failed | {'reason': 'missing unique title, armed Enter, receipt or settled successor'}
    title_frame, title_owner = map(int, title[0])
    arm, context, load, script, owner, plane, string, generation, g0, opaque, region, receiver = map(int, armed[0][:-1])
    finish, context1, load1, script1, owner1, plane1, string1, generation1, g1, opaque1 = map(int, settled[0])
    press_frame = arm + 1
    if (tuple(map(int, verified[0])) != (press_frame, region, receiver) or receiver != script
            or (context, load, script, owner, plane, string) != (context1, load1, script1, owner1, plane1, string1)
            or not (title_frame < g0 < arm < g1 < finish and finish >= arm + 4)
            or not (0 < generation < generation1 and load > 0 and opaque > 0 and opaque1 > 0)
            or re.findall(r'^frame (\d+): down (.*)$', host, re.MULTILINE) != [
                (str(title_frame - 1), 'MouseButton(0)'), (str(arm), 'Keyboard(13)')]):
        return failed | {'reason': 'body lifetime, generation, physical down order or release boundary differed'}

    presses = [match.groups() for match in PRESS.finditer(host)]
    if (len(presses) != len(re.findall(r'\bpressed: [1-9]\d*,', host)) or not presses
            or presses[0] != (str(title_frame), '0', '0', '2', '1', 'false', '11', str(title_owner), '0',
                              '[RoutedPhysicalButton { source: Mouse, code: 1 }]')
            or re.findall(rf'^frame={title_frame} message-resume (.*)$', events, re.MULTILINE)
            != ['message=[2, 183, 434, 0]']):
        return failed | {'reason': 'missing physical title receipt or same-frame root resume'}
    candidates = [tuple(map(int, pair)) for pair in re.findall(r'\((\d+), (\d+)\)', armed[0][-1])]
    if (region, receiver) not in candidates or len(set(candidates)) != len(candidates):
        return failed | {'reason': 'invalid eligible keyboard region order'}
    key_order = 0
    remaining = candidates
    consumed = False
    for frame, key, group, mask, pressed, repeated, target, callback, response, physical in presses[1:]:
        key = int(key)
        pair = (int(target), int(callback))
        if key_order == 0 and key == 2:
            key_order = 2
            remaining = candidates
        if (consumed or int(frame) != press_frame or key != key_order or group != '0' or mask != '1'
                or pressed != '1' or repeated != 'false' or pair not in remaining
                or physical != '[RoutedPhysicalButton { source: Keyboard, code: 13 }]'):
            return failed | {'reason': 'physical Enter callbacks differ from the armed route'}
        remaining = remaining[remaining.index(pair) + 1:]
        if response == '0':
            if key != 2 or pair != (region, receiver):
                return failed | {'reason': 'wrong consuming Enter callback'}
            consumed = True
    if not consumed:
        return failed | {'reason': 'missing consuming Enter callback'}

    calls = []
    for line in events.splitlines():
        match = NATIVE.match(line)
        if match:
            frame, native, size = map(int, match.groups()[:3])
            data = bytes.fromhex(match[4])
            if size == len(data) and size % 4 == 0:
                calls.append((frame, native, unpack(f'<{size // 4}I', data), line[match.end():]))
    loads = [i for i, call in enumerate(calls) if call[1] == 1 and ' debug="sscript loaded > ' in call[3]]
    if len(loads) != 1 or loads[0] == 0:
        return failed | {'reason': 'missing unique CST load'}
    load_frame, _, _, detail = calls[loads[0]]
    lookup = calls[loads[0] - 1]
    if (detail != ' debug="sscript loaded > 0001_00"' or not title_frame < load_frame < g0
            or lookup[:2] != (load_frame, 325) or len(lookup[2]) != 2 or lookup[2][0] != context):
        return failed | {'reason': 'CST load context differs'}

    for frame, group, offsets, wait, end in ((g0, 0, [88, 89], 90, arm), (g1, 1, [1], 4, finish)):
        page = [call for call in calls if call[0] == frame]
        executed = [read[2][3] for read, message in zip(page, page[1:])
                    if read[1] == 327 and len(read[2]) == 4 and (read[2][0], read[2][2]) == (context, group)
                    and read[2][3] in offsets and message[1] == 1 and message[3].startswith(' debug="[message] ')]
        markers = [i for i, call in enumerate(page) if call[1] == 1 and call[3] == ' debug="[input]"']
        if (executed != offsets or len(markers) != 1 or markers[0] == 0
                or page[markers[0] - 1][1:3] != (326, (context, group, wait))
                or not any(frame <= at < end and native == 69 and words == (owner, 13, 0, 0)
                           for at, native, words, _ in calls)):
            return failed | {'reason': f'g{group} body execution, wait lookup or reveal completion is missing'}

    if any(arm < int(frame) <= finish and int(task) in (script, owner, plane, string, region)
           for frame, task in re.findall(r'^frame=(\d+) task-create id=(\d+)\b', events, re.MULTILINE)):
        return failed | {'reason': 'guarded task ID was reused after Enter'}
    return failed | {'gate': 'g1_95_settled_after_live_enter', 'context': context, 'load': load,
                     'press_frame': press_frame, 'region': region, 'receiver': receiver,
                     'settled_frame': finish, 'body_generation': [generation, generation1],
                     'opaque_rgb_matches': [opaque, opaque1]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('log_directory', type=Path, help='Existing batch directory with 18.json and both logs')
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument('--require-wait-lookup', action='store_true',
                        help='Require the guarded title and zero-body-input control with native326(context,0,90)')
    mode.add_argument('--require-live-route', action='store_true', help='Require the state route through settled g1/95')
    parser.add_argument('--verified-advance-binding', type=Path,
                        help='Optional ordered chain of separately verified keyboard receipt fields in JSON')
    args = parser.parse_args()
    base = args.log_directory
    if args.require_live_route:
        result = json.loads((base / '18.json').read_text(encoding='utf-8'))
        logs = base / '18'
        report = (classify_live(result, (logs / '18.host.log').read_text(encoding='utf-8'),
                                (logs / '18.events.log').read_text(encoding='utf-8'))
                  if result.get('mode') == 'state' and result.get('status') != 'resource_deferred'
                  else {'gate': 'unproven', 'reason': 'no live state replay logs'})
        print(json.dumps(report, indent=2))
        return 0 if report['gate'] == 'g1_95_settled_after_live_enter' else 1
    binding = json.loads(args.verified_advance_binding.read_text(encoding='utf-8')) if args.verified_advance_binding else None
    report = classify(json.loads((base / '18.json').read_text(encoding='utf-8')),
                      (base / '18.host.log').read_text(encoding='utf-8'),
                      (base / '18.events.log').read_text(encoding='utf-8'), binding)
    print(json.dumps(report, indent=2))
    passed = report['gate'] == 'g0_first_body_settled'
    if args.require_wait_lookup:
        passed = (passed and report['title_hit_guard_configured'] and report['g0_no_advance_control']
                  and report['g0_wait_lookup_traced'] and report['g1_92']['status'] == 'not_observed')
    return 0 if passed else 1


if __name__ == '__main__':
    raise SystemExit(main())

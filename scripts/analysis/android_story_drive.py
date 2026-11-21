"""Drive ordinary Android story pages from the debug CS2Story log, never choices.

A page needs two observations with the same token and an advancing frame. After
input, only a different page confirmed by rows received after ADB returns can
authorize another tap. The pre-tap dump vetoes stale or superseded stream rows.
"""

import argparse
import io
import json
import re
import subprocess
import threading
import time
from contextlib import contextmanager
from dataclasses import asdict, dataclass
from datetime import datetime
from queue import Empty, Full, Queue

from PIL import Image

from android_load_probe import ARCHIVE, PACKAGE, adb, package_sha256
from android_story_replay import session, verify_session


SCENE = r'[A-Za-z0-9_.-]+'
SIGNED = r'[+-]?[0-9]+'
U64 = r'[0-9]{1,20}'
HEARTBEAT_MIN_FRAMES = 30
ROW = re.compile(
    rf'D/CS2Story\( *([0-9]+)\): v1 session=({U64}) seq=({U64}) frame=({U64}) '
    rf'state=(page|choice|active|ended|unknown) scene=({SCENE}) group=({SIGNED}) record=({SIGNED})'
)
HEADER = re.compile(rb'--------- beginning of [a-z]+')
PACKAGE_PATTERN = re.escape(PACKAGE.encode('ascii'))
PLAYER_FOCUS = re.compile(rb'Window\{[0-9a-fA-F]+ u[0-9]+ ' + PACKAGE_PATTERN
                          + rb'/(?:' + PACKAGE_PATTERN + rb')?\.PlayerActivity\}')
KEYGUARD_SHOWING = rb'(?:mKeyguardShowing|KeyguardShowing|mShowingLockscreen|isStatusBarKeyguard)'
KEYGUARD_BLOCKING = rb'(?:mDreamingLockscreen|mAodShowing|AodShowing|mKeyguardGoingAway|KeyguardGoingAway)'
KEYGUARD_DELEGATE = re.compile(
    rb'(?m)^([ \t]*)KeyguardServiceDelegate:?[ \t]*\r?\n'
    rb'(?:\1[ \t]+[^\r\n]+\r?\n)*?\1[ \t]+showing=(\S+)'
)
CHECKPOINT = 'checkpoint'


@dataclass(frozen=True)
class StoryRow:
    session: int
    seq: int
    frame: int
    state: str
    scene: str
    group: int
    record: int

    @property
    def position(self):
        return self.scene, self.group, self.record

    @property
    def identity(self):
        return self.state, *self.position


def _parse_row(line, pid):
    line = line.rstrip(b'\r\n')
    if not line or HEADER.fullmatch(line):
        return None
    match = ROW.fullmatch(line.decode('ascii', errors='replace'))
    if not match or match[1] != pid:
        raise ValueError('Malformed, unsupported, or wrong-PID CS2Story row')
    session_id, seq, frame = map(int, match.group(2, 3, 4))
    if not (0 < session_id < 2**64 and 0 < seq < 2**64 and 0 <= frame < 2**64):
        raise ValueError('CS2Story row has out-of-range counters')
    return StoryRow(session_id, seq, frame, *match.group(5, 6), int(match[7]), int(match[8]))


def _check_transition(previous, row):
    if row.session != previous.session:
        raise ValueError('CS2Story session changed')
    if row.frame < previous.frame:
        raise ValueError('CS2Story frame regressed')
    if row.seq != previous.seq + (row.identity != previous.identity):
        raise ValueError('CS2Story sequence regressed, skipped, or disagrees with identity')
    if row.seq == previous.seq and 0 < row.frame - previous.frame < HEARTBEAT_MIN_FRAMES:
        raise ValueError('CS2Story same-token heartbeat is fewer than 30 frames apart')


def _logcat(pid, *options):
    return ('logcat', f'--pid={pid}', '-b', 'main', '-v', 'brief', *options, '-s', 'CS2Story:D', '*:S')


@contextmanager
def _story_stream(serial, pid):
    events = Queue(maxsize=256)
    finished = threading.Event()
    errors = []
    with subprocess.Popen(['adb', '-s', serial, *_logcat(pid, '-T', '1')],
                          stdout=subprocess.PIPE, stderr=subprocess.STDOUT) as process:
        def read():
            try:
                for line in process.stdout:
                    events.put_nowait((time.monotonic(), line))
                events.put_nowait(None)
            except (Full, OSError) as error:
                errors.append(error)
            finally:
                finished.set()

        def check_open():
            if errors and isinstance(errors[0], Full):
                raise ValueError('CS2Story stream overflowed')
            if finished.is_set() or process.poll() is not None:
                raise OSError('CS2Story logcat stream closed unexpectedly')

        def next_line(deadline):
            check_open()
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                return None
            try:
                event = events.get(timeout=remaining)
            except Empty:
                check_open()
                return None
            if event is None:
                raise OSError('CS2Story logcat stream closed unexpectedly')
            return event

        reader = threading.Thread(target=read, daemon=True)
        reader.start()
        try:
            yield next_line, check_open
        finally:
            if process.poll() is None:
                process.kill()
            reader.join(timeout=2)


def _bounded_adb(serial, deadline, *command):
    remaining = deadline - time.monotonic()
    if remaining <= 0:
        raise subprocess.TimeoutExpired(command, 0)
    return subprocess.run(['adb', '-s', serial, *command], check=True, capture_output=True,
                          timeout=min(15, remaining)).stdout


def _verify_focus(serial, pid, deadline):
    data = _bounded_adb(serial, deadline, 'shell', f'pidof {PACKAGE} && dumpsys window')
    observed_pid, _, window_dump = data.partition(b'\n')
    if observed_pid.strip() != pid.encode('ascii'):
        raise ValueError('Player PID changed before the window focus check')
    focus = {value.strip() for value in re.findall(rb'(?m)^[ \t]*mCurrentFocus[ \t]*=([^\r\n]*)', window_dump)}
    displays = set(re.findall(rb'\bmTopFocusedDisplayId[ \t]*=[ \t]*(\S+)', window_dump))
    if len(focus) != 1 or not PLAYER_FOCUS.fullmatch(next(iter(focus))) or displays - {b'0'}:
        raise ValueError('PlayerActivity is not the single confirmed focused window on the default display')
    showing = re.findall(rb'\b' + KEYGUARD_SHOWING + rb'=(\S+)', window_dump)
    showing.extend(value for _, value in KEYGUARD_DELEGATE.findall(window_dump))
    blocking = re.findall(rb'\b' + KEYGUARD_BLOCKING + rb'=(\S+)', window_dump)
    if not showing or any(value != b'false' for value in showing + blocking):
        raise ValueError('Keyguard is showing, transitioning, or cannot be confirmed unlocked')


def _snapshot(serial, pid, candidate, deadline):
    log = _bounded_adb(serial, deadline, *_logcat(pid, '-d'))
    latest = None
    for line in reversed(log.splitlines()):
        row = _parse_row(line, pid)
        if row is None:
            continue
        if latest is None:
            latest = row
        if row.session != candidate.session or row.seq != candidate.seq:
            break
        _check_transition(row, latest)
        if row == candidate:
            return latest
    raise ValueError('CS2Story token changed or disappeared before tap')


def _capture(args, output, report, name, started):
    data = adb(args.serial, 'exec-out', 'screencap', '-p')
    with Image.open(io.BytesIO(data), formats=['PNG']) as image:
        size = image.size
        image.verify()
    (output / name).write_bytes(data)
    report['captures'][name] = {'size': size, 'elapsed_ms': (time.monotonic() - started) * 1000}
    return size


def _boundary(args, row):
    if row.state in ('choice', 'ended', 'unknown'):
        return row.state
    if row.scene == '-' or row.scene not in args.allow_scene:
        return 'scene_not_allowed'
    if row.state == 'page':
        if row.position == args.stop_at:
            return CHECKPOINT
        if -1 in (row.group, row.record):
            return 'unknown_page'
    return None


def _drive(args, expected, stream, report, started):
    next_line, check_open = stream
    deadline = started + args.timeout
    after_tap = started
    pre_tap_frame = -1
    previous = None
    previous_received = started
    last_frame_advance = None
    tapped_position = None
    pid = expected[0]

    while (event := next_line(deadline)) is not None:
        received, line = event
        if time.monotonic() >= deadline:
            break
        row = _parse_row(line, pid)
        if row is None:
            continue
        report['last_row'] = asdict(row)
        if previous is None:
            previous, previous_received = row, received
            continue
        _check_transition(previous, row)
        frame_advanced = row.frame > previous.frame
        if frame_advanced and received >= after_tap and row.frame > pre_tap_frame:
            last_frame_advance = received
            if report['stage'] == 'logcat_handshake' and time.monotonic() - received <= args.heartbeat_max_age:
                report['logcat_ready_ms'] = (received - started) * 1000
                report['stage'] = 'wait_for_story'
        boundary = _boundary(args, row)
        if boundary and (frame_advanced or row.seq != previous.seq):
            if boundary == CHECKPOINT:
                if 'logcat_ready_ms' not in report:
                    previous, previous_received = row, received
                    continue
                report['stage'] = 'verify_checkpoint'
                verify_session(args, expected)
                if time.monotonic() >= deadline:
                    break
                _snapshot(args.serial, pid, row, deadline)
                if time.monotonic() >= deadline:
                    break
                check_open()
                if time.monotonic() - received > args.heartbeat_max_age:
                    return 'stale_checkpoint'
            return boundary
        confirmed = (frame_advanced and row.seq == previous.seq and previous_received >= after_tap
                     and time.monotonic() - previous_received <= args.heartbeat_max_age)
        previous, previous_received = row, received
        if not confirmed or boundary or row.state != 'page' or row.position == tapped_position:
            continue
        report['confirmed_page'] = asdict(row)
        if len(report['actions']) == args.max_actions:
            return 'max_actions'
        report['stage'] = 'verify_before_tap'
        guard_started = time.monotonic()
        verify_session(args, expected)
        guard_finished = time.monotonic()
        if guard_finished >= deadline:
            break
        report['stage'] = 'snapshot_before_tap'
        latest = _snapshot(args.serial, pid, row, deadline)
        report['pre_tap_row'] = asdict(latest)
        snapshot_finished = time.monotonic()
        if snapshot_finished >= deadline:
            break
        report['stage'] = 'verify_focus_before_tap'
        _verify_focus(args.serial, pid, deadline)
        check_open()
        now = time.monotonic()
        if now >= deadline:
            break
        if now - received > args.heartbeat_max_age:
            return 'stale_before_tap'
        action = {'row': asdict(latest), 'start_ms': (now - started) * 1000, 'delivery': 'unknown',
                  'confirm_to_tap_ms': (now - received) * 1000,
                  'guard_ms': (guard_finished - guard_started) * 1000,
                  'snapshot_ms': (snapshot_finished - guard_finished) * 1000,
                  'focus_ms': (now - snapshot_finished) * 1000}
        report['actions'].append(action)
        report['stage'] = 'send_tap'
        _bounded_adb(args.serial, deadline, 'shell', 'input', 'tap', *map(str, args.tap))
        after_tap = time.monotonic()
        action.update(delivery='adb_returned', adb_return_ms=(after_tap - started) * 1000)
        tapped_position = row.position
        pre_tap_frame = latest.frame
        report['stage'] = 'wait_for_story'
    if 'logcat_ready_ms' not in report:
        return 'logcat_handshake_timeout'
    if (last_frame_advance is not None and last_frame_advance >= after_tap
            and deadline - last_frame_advance <= args.heartbeat_max_age):
        return 'timeout_live'
    return 'timeout_no_heartbeat'


def _run(args):
    expected = session(args.serial)
    verify_session(args, expected)
    apk = package_sha256(args.serial).lower()
    if apk != args.apk_sha256:
        raise ValueError(f'Installed APK SHA-256 is {apk}, expected {args.apk_sha256}')
    verify_session(args, expected)
    _verify_focus(args.serial, expected[0], time.monotonic() + min(15, args.timeout))
    args.confirmed_surface = expected[1]

    now = datetime.now().astimezone()
    output = ARCHIVE / now.strftime('%Y-%m') / now.strftime('%Y%m%d_%H%M%S_%f_android_story_drive')
    output.mkdir(parents=True)
    report = {'label': args.label, 'serial': args.serial, 'game_root': args.game_root,
              'apk_sha256': apk, 'pid': expected[0], 'surface': expected[1],
              'allow_scene': sorted(args.allow_scene), 'stop_at': args.stop_at, 'max_actions': args.max_actions,
              'timeout_seconds': args.timeout, 'heartbeat_max_age_seconds': args.heartbeat_max_age,
              'tap': args.tap, 'actions': [], 'captures': {}, 'stage': 'capture_start'}
    started = time.monotonic()
    try:
        if args.capture_start:
            width, height = _capture(args, output, report, 'before.png', started)
            if not (args.tap[0] < width and args.tap[1] < height):
                raise ValueError(f'Tap {args.tap} is outside {(width, height)}')
        report['stage'] = 'logcat_handshake'
        with _story_stream(args.serial, expected[0]) as stream:
            report['reason'] = _drive(args, expected, stream, report, started)
    except subprocess.TimeoutExpired as error:
        report.update(reason='command_timeout', error=str(error))
    except (OSError, ValueError, SyntaxError, subprocess.SubprocessError) as error:
        report.update(reason='error', error=str(error))
    except KeyboardInterrupt:
        report['reason'] = 'interrupted'
    try:
        _capture(args, output, report, 'after.png', started)
    except (OSError, ValueError, SyntaxError, subprocess.SubprocessError) as error:
        report['evidence_error'] = str(error)
    destination = output / 'drive.json'
    destination.write_text(json.dumps(report, indent=2) + '\n', encoding='utf-8')
    print(json.dumps({'reason': report['reason'], 'attempted': len(report['actions']), 'report': str(destination)}))
    return 0 if report['reason'] == CHECKPOINT else 1


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--serial', required=True)
    parser.add_argument('--label', required=True)
    parser.add_argument('--game-root', required=True, help='Exact root from this PID\'s CS2Player Opened log')
    parser.add_argument('--apk-sha256', required=True, help='Expected installed APK SHA-256 (64 hex digits)')
    parser.add_argument('--allow-scene', action='append', required=True, help='Exact scene name; repeat to allow more')
    parser.add_argument('--stop-at', nargs=3, required=True, metavar=('SCENE', 'GROUP', 'RECORD'))
    parser.add_argument('--max-actions', required=True, type=int, help='Hard limit on tap attempts (1-60)')
    parser.add_argument('--tap', nargs=2, required=True, type=int, metavar=('X', 'Y'),
                        help='Previously verified ordinary-page tap coordinates')
    parser.add_argument('--timeout', type=float, default=30, help='Total monotonic drive deadline in seconds')
    parser.add_argument('--heartbeat-max-age', type=float, default=3, help='Maximum age of an advancing frame in seconds')
    parser.add_argument('--capture-start', action='store_true', help='Also save a screenshot before driving')
    args = parser.parse_args(argv)
    if not re.fullmatch(r'[0-9a-fA-F]{64}', args.apk_sha256):
        parser.error('--apk-sha256 must contain exactly 64 hexadecimal digits')
    args.apk_sha256 = args.apk_sha256.lower()
    args.allow_scene = frozenset(args.allow_scene)
    if any(scene == '-' or not re.fullmatch(SCENE, scene) for scene in args.allow_scene):
        parser.error('--allow-scene must name an exact, known scene')
    scene, group, record = args.stop_at
    try:
        coordinates = tuple(int(part) for part in (group, record) if re.fullmatch(SIGNED, part))
    except ValueError:
        coordinates = ()
    if scene not in args.allow_scene or len(coordinates) != 2 or -1 in coordinates:
        parser.error('--stop-at needs an allowed scene and known integer group and record')
    args.stop_at = scene, *coordinates
    if (not args.serial.strip() or not args.game_root.strip() or not args.label.strip() or min(args.tap) < 0
            or not 1 <= args.max_actions <= 60):
        parser.error('Expected nonempty serial, label and root, nonnegative coordinates and 1-60 actions')
    if not (0 < args.heartbeat_max_age <= 60 and 0 < args.timeout <= 3600):
        parser.error('Expected a heartbeat age of at most 60 s and a deadline of at most 3600 s, both positive')
    args.confirmed_surface = None
    try:
        return _run(args)
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        parser.exit(2, f'{error}\n')


if __name__ == '__main__':
    raise SystemExit(main())

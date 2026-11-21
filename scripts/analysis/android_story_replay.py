"""Replay a bounded series of taps in one confirmed Android game session."""

import argparse
import io
import json
import re
import subprocess
import time
from datetime import datetime

from PIL import Image

from android_load_probe import ARCHIVE, PACKAGE, adb, package_sha256


def session(serial):
    state = adb(serial, 'shell', 'pidof org.catsystem2.player; dumpsys SurfaceFlinger --list').decode()
    process, _, layers = state.partition('\n')
    process = process.strip()
    surfaces = re.findall(r'SurfaceView\[org\.catsystem2\.player/[^\]]+\]\(BLAST\)#\d+', layers)
    if not process.isdecimal() or len(surfaces) != 1:
        raise ValueError('The emulator is outside a single active game surface')
    return process, surfaces[0]


def verify_session(args, expected):
    if session(args.serial) != expected:
        raise ValueError('The game session or render surface changed')
    log = adb(args.serial, 'logcat', f'--pid={expected[0]}', '-d', '-v', 'brief', '-s', 'CS2Player:I', '*:S')
    opened = None
    error = None
    for line in log.decode(errors='replace').splitlines():
        if '): Opened ' in line:
            opened = line.partition('): Opened ')[2]
            error = None
        elif line.startswith('E/CS2Player') or '): Game tick stalled:' in line:
            error = line
    if opened is not None and opened != args.game_root:
        raise ValueError(f'The current session opened {opened}, expected {args.game_root}')
    if opened is None and args.confirmed_surface is None:
        raise ValueError('The game open log expired. Inspect the current game and provide --confirmed-surface.')
    if error:
        raise ValueError(error)
    return 'game_log' if opened is not None else 'confirmed_surface'


def replay(args):
    expected_session = session(args.serial)
    if args.confirmed_surface is not None and args.confirmed_surface != expected_session[1]:
        raise ValueError('The confirmed surface is no longer active')
    verification = verify_session(args, expected_session)
    apk = package_sha256(args.serial)
    now = datetime.now().astimezone()
    output = ARCHIVE / now.strftime('%Y-%m') / now.strftime('%Y%m%d_%H%M%S_%f_android_story_replay')
    output.mkdir(parents=True)
    report = {'label': args.label, 'serial': args.serial, 'game_root': args.game_root,
              'apk_sha256': apk, 'pid': expected_session[0], 'surface': expected_session[1],
              'verification': verification,
              'requested_taps': args.repeat, 'tap': args.tap, 'interval_seconds': args.interval,
              'taps': [], 'captures': {}, 'status': 'stopped', 'progression': 'unverified'}
    stamp = time.perf_counter()

    def capture(name):
        started = time.perf_counter() - stamp
        data = adb(args.serial, 'exec-out', 'screencap', '-p')
        returned = time.perf_counter() - stamp
        with Image.open(io.BytesIO(data)) as image:
            size = image.size
        (output / name).write_bytes(data)
        report['captures'][name] = {'start_ms': started * 1000, 'adb_return_ms': returned * 1000}
        return size

    def record_error(field, error, stage, tap_index=None):
        report[field] = str(error)
        detail = {'stage': stage,
                  'kind': 'command_timeout' if isinstance(error, subprocess.TimeoutExpired) else 'error',
                  'elapsed_ms': (time.perf_counter() - stamp) * 1000}
        if tap_index is not None:
            detail['tap_index'] = tap_index
        report[f'{field}_detail'] = detail

    stage = 'capture_before'
    tap_index = None
    try:
        width, height = capture('before.png')
        stage = 'validate_tap'
        tap_x, tap_y = args.tap
        if not (0 <= tap_x < width and 0 <= tap_y < height):
            raise ValueError(f'Tap {args.tap} is outside {(width, height)}')
        for tap_index in range(1, args.repeat + 1):
            stage = 'verify_before_tap'
            verify_session(args, expected_session)
            stage = 'send_tap'
            started = time.perf_counter() - stamp
            adb(args.serial, 'shell', 'input', 'tap', str(tap_x), str(tap_y))
            report['taps'].append({'index': tap_index, 'start_ms': started * 1000,
                                   'adb_return_ms': (time.perf_counter() - stamp) * 1000})
            stage = 'tap_interval'
            time.sleep(args.interval)
            if args.capture_every and tap_index % args.capture_every == 0:
                stage = 'capture_periodic'
                capture(f'tap_{tap_index:03}.png')
        stage = 'verify_after_taps'
        tap_index = None
        verify_session(args, expected_session)
        report['status'] = 'inputs_sent'
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        record_error('error', error, stage, tap_index)

    stage = 'capture_after'
    try:
        capture('after.png')
        stage = 'collect_session_log'
        log = adb(args.serial, 'logcat', f'--pid={expected_session[0]}',
                  '-d', '-v', 'brief', '-s', 'CS2Player:I', 'CS2Perf:I', '*:S')
        (output / 'session.log').write_bytes(log)
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        record_error('evidence_error', error, stage)
        report['status'] = 'stopped'
    destination = output / 'replay.json'
    destination.write_text(json.dumps(report, indent=2) + '\n', encoding='utf-8')
    print(json.dumps({'status': report['status'], 'sent': len(report['taps']), 'report': str(destination)}))
    return 0 if report['status'] == 'inputs_sent' else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--serial', required=True)
    parser.add_argument('--label', required=True)
    parser.add_argument('--game-root', required=True, help='Exact root from the CS2Player Opened log entry')
    parser.add_argument('--confirmed-surface', help='Exact game SurfaceView from dumpsys after visually confirming the game')
    parser.add_argument('--tap', nargs=2, required=True, type=int, metavar=('X', 'Y'))
    parser.add_argument('--repeat', type=int, default=1)
    parser.add_argument('--interval', type=float, default=0.5)
    parser.add_argument('--capture-every', type=int, default=0)
    args = parser.parse_args()
    if not (1 <= args.repeat <= 60 and 0 <= args.interval <= 30 and 0 <= args.capture_every <= 60):
        parser.error('Expected 1–60 taps, a 0–30 second interval and a valid screenshot cadence')
    try:
        return replay(args)
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        parser.exit(2, f'{error}\n')


if __name__ == '__main__':
    raise SystemExit(main())

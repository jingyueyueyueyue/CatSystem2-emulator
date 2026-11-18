"""Measure a known visual checkpoint after one tablet tap (requires Pillow)."""

import argparse
import hashlib
import io
import json
import shlex
import subprocess
import time
from datetime import datetime
from pathlib import Path

from PIL import Image, ImageChops, ImageStat


PACKAGE = 'org.catsystem2.player'
ARCHIVE = Path(__file__).resolve().parents[2] / 'archive'


def adb(serial, *args):
    return subprocess.run(['adb', '-s', serial, *args], check=True,
                          capture_output=True, timeout=15).stdout


def thumbnail(image, crop):
    if crop:
        left, top, right, bottom = crop
        if not (0 <= left < right <= image.width and 0 <= top < bottom <= image.height):
            raise ValueError(f'Crop {crop} is outside {image.size}')
        image = image.crop(crop)
    return image.convert('RGB').resize((96, 54), Image.Resampling.BOX)


def package_sha256(serial):
    path, = adb(serial, 'shell', 'pm', 'path', PACKAGE).decode().splitlines()
    result = adb(serial, 'shell', f'sha256sum {shlex.quote(path.removeprefix("package:"))}')
    return result.decode().split()[0]


def measure(args):
    with Image.open(args.reference) as reference_file:
        expected_size = reference_file.size
        reference = thumbnail(reference_file, args.crop)

    def compare(png):
        with Image.open(io.BytesIO(png)) as image:
            if image.size != expected_size:
                raise ValueError(f'Screen size {image.size} differs from reference {expected_size}')
            difference = ImageChops.difference(reference, thumbnail(image, args.crop))
            return sum(ImageStat.Stat(difference).mean) / 3

    apk = package_sha256(args.serial)
    before = adb(args.serial, 'exec-out', 'screencap', '-p')
    tap_x, tap_y = args.tap
    if not (0 <= tap_x < expected_size[0] and 0 <= tap_y < expected_size[1]):
        raise ValueError(f'Tap {args.tap} is outside {expected_size}')
    before_distance = compare(before)
    if before_distance <= args.max_distance:
        raise ValueError('Reference is already visible before input')

    now = datetime.now()
    output = ARCHIVE / now.strftime('%Y-%m') / now.strftime('%Y%m%d_%H%M%S_%f_android_load_probe')
    output.mkdir(parents=True)
    (output / 'before.png').write_bytes(before)

    time_origin = time.perf_counter()
    adb(args.serial, 'shell', 'input', 'tap', str(tap_x), str(tap_y))
    tap_return = time.perf_counter() - time_origin
    samples = []
    last_miss = 0.0
    result = None
    observed = before
    while time.perf_counter() - time_origin < args.timeout:
        capture_start = time.perf_counter() - time_origin
        observed = adb(args.serial, 'exec-out', 'screencap', '-p')
        capture_return = time.perf_counter() - time_origin
        score = compare(observed)
        samples.append({'capture_start_ms': capture_start * 1000,
                        'capture_return_ms': capture_return * 1000, 'distance': score})
        if score <= args.max_distance:
            result = {'last_nonmatching_capture_start_ms': last_miss * 1000,
                      'first_matching_capture_return_ms': capture_return * 1000}
            break
        last_miss = capture_start
        time.sleep(args.interval)

    (output / 'observed.png').write_bytes(observed)
    report = {'status': 'observed' if result else 'timeout', 'label': args.label,
              'serial': args.serial, 'apk_sha256': apk, 'tap': args.tap,
              'capture_size': expected_size, 'crop': args.crop, 'threshold': args.max_distance,
              'reference': str(args.reference.resolve()),
              'reference_sha256': hashlib.sha256(args.reference.read_bytes()).hexdigest(),
              'before_distance': before_distance, 'tap_adb_return_ms': tap_return * 1000,
              'observation': result, 'samples': samples}
    destination = output / 'probe.json'
    destination.write_text(json.dumps(report, indent=2) + '\n', encoding='utf-8')
    print(json.dumps({'status': report['status'], 'observation': result, 'report': str(destination)}))
    return 0 if result else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--serial', required=True)
    parser.add_argument('--label', required=True)
    parser.add_argument('--tap', type=int, nargs=2, required=True, metavar=('X', 'Y'))
    parser.add_argument('--reference', required=True, type=Path)
    parser.add_argument('--crop', type=int, nargs=4, metavar=('LEFT', 'TOP', 'RIGHT', 'BOTTOM'))
    parser.add_argument('--timeout', type=float, default=45)
    parser.add_argument('--interval', type=float, default=0.12)
    parser.add_argument('--max-distance', type=float, default=8,
                        help='Maximum mean channel difference on the downscaled crop, from 0 to 255')
    args = parser.parse_args()
    if not (0 <= args.max_distance <= 255 and args.timeout > 0 and args.interval >= 0):
        parser.error('Invalid timeout, capture interval or color threshold')
    try:
        return measure(args)
    except (ValueError, OSError, subprocess.SubprocessError) as error:
        parser.exit(2, f'{error}\n')


if __name__ == '__main__':
    raise SystemExit(main())

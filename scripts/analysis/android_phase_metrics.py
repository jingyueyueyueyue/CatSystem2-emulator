"""Summarize raw CS2Perf frame timings for one captured Android player session."""

import argparse
import json
import math
from pathlib import Path

from android_surface_metrics import parse_snapshots


FIELDS = ('session', 'post', 'frame', 'step_calls', 'step_us', 'script_us',
          'image_read_us', 'image_decode_us', 'image_patch_us', 'compose_us',
          'present_us', 'lock_us', 'copy_us', 'post_us', 'interval_us',
          'post_mono_us', 'tick_to_post_us')
PHASES = ('script_us', 'image_read_us', 'image_decode_us', 'image_patch_us',
          'compose_us', 'present_us')
SCRIPT_PHASES = ('script_input_us', 'script_root_us', 'script_children_us', 'script_fes_us',
                 'script_list_us', 'script_list_raster_us', 'script_motion_raster_us', 'script_button_us')
MEASURES = (*PHASES, 'step_us', 'lock_us', 'copy_us', 'post_us', 'interval_us',
            'tick_to_post_us')


def summary(values):
    ordered = sorted(values)
    if not ordered:
        return None
    return {'samples': len(ordered), 'mean_ms': sum(ordered) / len(ordered) / 1000,
            'p50_ms': ordered[math.ceil(len(ordered) * .5) - 1] / 1000,
            'p95_ms': ordered[math.ceil(len(ordered) * .95) - 1] / 1000,
            'max_ms': ordered[-1] / 1000}


def parse(log, session, first_post, last_post):
    rows = {}
    for line in log.splitlines():
        if not line.startswith('f,') or line.startswith('f,session,'):
            continue
        values = list(map(int, line.split(',')[1:]))
        fields = FIELDS if len(values) == len(FIELDS) else (*FIELDS, *SCRIPT_PHASES)
        record = dict(zip(fields, values, strict=True))
        if record['session'] != session or not first_post <= record['post'] <= last_post:
            continue
        previous = rows.setdefault(record['post'], record)
        if previous != record:
            raise ValueError('Conflicting post sequence. Capture only one player process.')
    return [rows[post] for post in sorted(rows)]


def report(rows):
    posts = [row['post'] for row in rows]
    metrics = {field: summary([row[field] for row in rows]) for field in MEASURES}
    metrics['interval_us'] = summary([current['interval_us'] for previous, current
                                      in zip(rows, rows[1:])
                                      if current['post'] == previous['post'] + 1])
    result = {'samples': len(rows), 'first_post': posts[0], 'last_post': posts[-1],
            'missing_posts': posts[-1] - posts[0] + 1 - len(rows),
            'phases_by_total_time': sorted(PHASES, key=lambda field: -sum(row[field] for row in rows)),
            'metrics': metrics}
    if all(SCRIPT_PHASES[0] in row for row in rows):
        result['script_subphases'] = {
            field: summary([row[field] for row in rows])
            for field in sorted(SCRIPT_PHASES, key=lambda field: -sum(row[field] for row in rows))
        }
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('log', type=Path, help='adb logcat -v raw -s CS2Perf:I')
    parser.add_argument('--session', type=int, required=True)
    parser.add_argument('--first-post', type=int, default=1)
    parser.add_argument('--last-post', type=int, default=(1 << 63) - 1)
    parser.add_argument('--surface-snapshots', type=Path,
                        help='filter to the actual-present time span from the same device boot and replay phase')
    args = parser.parse_args()
    try:
        rows = parse(args.log.read_text(encoding='utf-8-sig'), args.session,
                     args.first_post, args.last_post)
        if args.surface_snapshots is not None:
            snapshots = parse_snapshots(args.surface_snapshots.read_bytes())
            presents = set().union(*snapshots)
            if not presents:
                raise ValueError('No valid actual-present timestamps in the selected snapshots')
            first_present_ns, last_present_ns = min(presents), max(presents)
            rows = [row for row in rows
                    if first_present_ns <= row['post_mono_us'] * 1000 <= last_present_ns]
        if not rows:
            raise ValueError('No posts in the selected session and range')
        print(json.dumps(report(rows), indent=2))
    except (ValueError, OSError) as error:
        parser.exit(2, f'{error}\n')


if __name__ == '__main__':
    main()

"""Join saved SurfaceFlinger latency dumps to CS2Perf posts from the same boot and layer."""

import argparse
from bisect import bisect_left, bisect_right
import codecs
from collections import Counter
import json
from pathlib import Path

from android_phase_metrics import parse, summary
from android_surface_metrics import (
    LATENCY_ROW,
    LIMIT_NS,
    PENDING_NS,
    REFRESH_LINE,
    positive_int,
    summarize,
)


PHASES = (
    'step_us', 'script_list_raster_us', 'compose_us', 'lock_us', 'post_us',
    'interval_us', 'tick_to_post_us',
)


def read_latency(paths):
    histories = []
    refresh_periods = set()
    ready_by_present = {}
    for path in paths:
        raw = path.read_bytes()
        encoding = 'utf-16' if raw.startswith((codecs.BOM_UTF16_LE, codecs.BOM_UTF16_BE)) else 'utf-8-sig'
        history = None
        for line in raw.decode(encoding).splitlines():
            line = line.strip()
            if REFRESH_LINE.fullmatch(line):
                refresh_periods.add(int(line))
                history = set()
                histories.append(history)
            elif history is not None and LATENCY_ROW.fullmatch(line):
                _, actual, ready = map(int, line.split())
                if 0 < actual < PENDING_NS:
                    history.add(actual)
                    candidates = ready_by_present.setdefault(actual, set())
                    if 0 < ready <= actual:
                        candidates.add(ready)
    return histories, sorted(refresh_periods), ready_by_present


def join_posts(ready_by_present, posts, tolerance_ns):
    post_times = [record['post_mono_us'] * 1000 for record in posts]
    if post_times[0] <= 0 or any(
        current <= previous for previous, current in zip(post_times, post_times[1:])
    ):
        raise ValueError('Post timestamps must increase strictly within the selected capture and session.')

    matches = {}
    status = {}
    candidate_indices = Counter()
    for actual, ready_times in ready_by_present.items():
        if not ready_times:
            status[actual] = 'missing_ready_timestamp'
            continue
        indices = set()
        for ready in ready_times:
            lower = bisect_left(post_times, ready - tolerance_ns)
            upper = bisect_right(post_times, ready + tolerance_ns)
            indices.update(range(lower, upper))
        candidate_indices.update(indices)
        if len(ready_times) != 1:
            status[actual] = 'ambiguous_ready_timestamp'
        elif not indices:
            status[actual] = 'no_post_in_tolerance'
        elif len(indices) != 1:
            status[actual] = 'multiple_posts_in_tolerance'
        else:
            matches[actual] = (next(iter(ready_times)), next(iter(indices)))

    results = {}
    for actual, (ready, index) in matches.items():
        if candidate_indices[index] == 1:
            results[actual] = (ready, posts[index])
            status[actual] = 'matched'
        else:
            status[actual] = 'post_shared_by_multiple_presents'
    return results, status


def describe_intervals(timestamps, matches, status, include_frames):
    groups = {'le_33_ms': [], 'gt_33_ms': []}
    if include_frames:
        frames = []
    for previous, actual in zip(timestamps, timestamps[1:]):
        gap_ns = actual - previous
        group = groups['le_33_ms' if gap_ns <= LIMIT_NS else 'gt_33_ms']
        match = matches.get(actual)
        if match is None:
            group.append(None)
            if include_frames:
                frames.append({'actual_ns': actual, 'interval_ns': gap_ns, 'match': status[actual]})
            continue

        ready, post = match
        previous_match = matches.get(previous)
        post_delta = post['post'] - previous_match[1]['post'] if previous_match is not None else None
        phases = {field: post[field] for field in PHASES if field in post}
        phases['ready_to_present_us'] = (actual - ready) / 1000
        if post['post'] > 1 and post['tick_to_post_us']:
            phases['outside_tick_us'] = max(0, post['interval_us'] - post['tick_to_post_us'])
        group.append(phases)
        if include_frames:
            frames.append({
                'actual_ns': actual, 'interval_ns': gap_ns, 'ready_ns': ready,
                'ready_minus_post_ns': ready - post['post_mono_us'] * 1000,
                'post': post['post'], 'frame': post['frame'],
                'post_number_delta': post_delta, **phases,
            })

    results = {}
    for name, intervals in groups.items():
        matched = [phases for phases in intervals if phases is not None]
        fields = sorted({field for phases in matched for field in phases})
        results[name] = {
            'intervals': len(intervals),
            'matched_end_posts': len(matched),
            'timings': {field: summary([phases[field] for phases in matched if field in phases])
                        for field in fields},
        }
    return results, frames if include_frames else None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('log', type=Path, help='saved CS2Perf raw log')
    parser.add_argument('snapshots', type=Path, nargs='+', help='raw --latency output for one exact layer')
    parser.add_argument('--session', type=positive_int, required=True)
    parser.add_argument('--tolerance-us', type=positive_int, default=300,
                        help='maximum absolute ready/post timestamp difference in us (default: 300)')
    parser.add_argument('--min-samples', type=positive_int, default=60,
                        help='minimum intervals for the SurfaceFlinger performance gate (default: 60)')
    parser.add_argument('--frames', action='store_true', help='include one record per actual display interval')
    args = parser.parse_args()

    try:
        raw = args.log.read_bytes()
        encoding = 'utf-16' if raw.startswith((codecs.BOM_UTF16_LE, codecs.BOM_UTF16_BE)) else 'utf-8-sig'
        posts = parse(raw.decode(encoding), args.session, 1, PENDING_NS)
        if not posts:
            raise ValueError('No CS2Perf posts for the selected session.')
        histories, refresh_periods, ready_by_present = read_latency(args.snapshots)
        if not ready_by_present:
            raise ValueError('No valid SurfaceFlinger actual-present timestamps.')
        surface = summarize(histories, args.min_samples)
        matches, status = join_posts(ready_by_present, posts, args.tolerance_us * 1000)
        report = {
            'surface': surface,
            'display_periods_ns': refresh_periods,
            'tolerance_us': args.tolerance_us,
            'present_match_counts': dict(Counter(status.values())),
        }
        if not surface['uncovered_interval_count']:
            intervals, frames = describe_intervals(sorted(ready_by_present), matches, status, args.frames)
            report['intervals'] = intervals
            if frames is not None:
                report['frames'] = frames
        print(json.dumps(report, indent=2))
    except (ValueError, OSError) as error:
        parser.exit(2, f'{parser.prog}: {error}\n')


if __name__ == '__main__':
    main()

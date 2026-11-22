"""Summarize raw CS2Perf frame timings for one captured Android player session."""

import argparse
import json
import math
import re
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
SCRIPT_LOAD_PHASES = ('script_read_us', 'script_parse_us')
COMPOSE_PHASES = ('compose_layers_us', 'compose_text_us', 'compose_emote_us',
                  'compose_transition_us', 'compose_other_us')
LAYER_PHASES = ('compose_plane_us', 'compose_kx_us', 'compose_resource_redraw_us',
                'compose_layers_other_us')
SCRIPT_FIELDS = (*FIELDS, *SCRIPT_PHASES)
LOAD_FIELDS = (*SCRIPT_FIELDS, *SCRIPT_LOAD_PHASES)
COMPOSE_FIELDS = (*LOAD_FIELDS, *COMPOSE_PHASES)
FRAME_SCHEMAS = {len(fields): fields for fields in
                 (FIELDS, SCRIPT_FIELDS, LOAD_FIELDS, COMPOSE_FIELDS, (*COMPOSE_FIELDS, *LAYER_PHASES))}
MEASURES = (*PHASES, 'step_us', 'lock_us', 'copy_us', 'post_us', 'interval_us',
            'tick_to_post_us')


def summary(values):
    ordered = sorted(values)
    if not ordered:
        return None
    return {'samples': len(ordered), 'mean_ms': sum(ordered) / len(ordered) / 1000,
            'p50_ms': ordered[math.ceil(len(ordered) * .5) - 1] / 1000,
            'p95_ms': ordered[math.ceil(len(ordered) * .95) - 1] / 1000,
            'p99_ms': ordered[math.ceil(len(ordered) * .99) - 1] / 1000,
            'max_ms': ordered[-1] / 1000}


def parse(log, session, first_post, last_post):
    rows = {}
    for line in log.splitlines():
        if not line.startswith('f,') or line.startswith('f,session,'):
            continue
        values = list(map(int, line.split(',')[1:]))
        fields = FRAME_SCHEMAS.get(len(values))
        if fields is None:
            raise ValueError(f'Unexpected CS2Perf frame column count: {len(values)}')
        record = dict(zip(fields, values, strict=True))
        if record['session'] != session or not first_post <= record['post'] <= last_post:
            continue
        previous = rows.setdefault(record['post'], record)
        if previous != record:
            raise ValueError('Conflicting post sequence. Capture only one player process.')
    return [rows[post] for post in sorted(rows)]


def parse_named_rows(log, kind, session):
    for line in log.splitlines():
        if line.startswith(f'{kind} '):
            row = {name: int(value) for name, value in
                   (field.split('=', 1) for field in line.split()[1:])}
            if row.pop('session') == session:
                yield row


def report(rows):
    posts = [row['post'] for row in rows]
    metrics = {field: summary([row[field] for row in rows]) for field in MEASURES}
    metrics['interval_us'] = summary([current['interval_us'] for previous, current
                                      in zip(rows, rows[1:])
                                      if current['post'] == previous['post'] + 1])
    extra_phases = tuple(field for field in SCRIPT_LOAD_PHASES if any(field in row for row in rows))
    for field in (*extra_phases, *COMPOSE_PHASES, *LAYER_PHASES):
        values = [row[field] for row in rows if field in row]
        if values:
            metrics[field] = summary(values)
    result = {'samples': len(rows), 'first_post': posts[0], 'last_post': posts[-1],
            'missing_posts': posts[-1] - posts[0] + 1 - len(rows),
            'phases_by_total_time': sorted((*PHASES, *extra_phases),
                                           key=lambda field: -sum(row.get(field, 0) for row in rows)),
            'metrics': metrics}
    if all(SCRIPT_PHASES[0] in row for row in rows):
        result['script_subphases'] = {
            field: summary([row[field] for row in rows])
            for field in sorted(SCRIPT_PHASES, key=lambda field: -sum(row[field] for row in rows))
        }
    return result


def append_session_metrics(result, log, session, rows, whole_session):
    for kind in ('open', 'load', 'first_post'):
        record = next(parse_named_rows(log, kind, session), None)
        if record is not None:
            result[kind] = record
    unchanged = list(parse_named_rows(log, 'unchanged', session))
    frames = [row['frame'] for row in rows if row['frame']]
    if not whole_session:
        unchanged = [row for row in unchanged if frames and frames[0] <= row['frame'] <= frames[-1]]
    gpu = {}
    for row in parse_named_rows(log, 'gpu_output', session):
        if whole_session or frames and frames[0] <= row['frame'] <= frames[-1]:
            key = row['frame'], row['call']
            if gpu.setdefault(key, row) != row:
                raise ValueError('Conflicting GPU output record for one frame and step call.')
    if gpu:
        gpu_rows = list(gpu.values())
        result['gpu_output'] = {
            'rows': len(gpu_rows),
            'textured_group_rejects': sum(row['textured_group_rejects'] for row in gpu_rows),
            'textured_single_rejects': sum(row['textured_single_rejects'] for row in gpu_rows),
        }
        for kind in ('device', 'packed', 'emote'):
            fields = ('calls', 'failed', 'bytes')
            result['gpu_output'][kind] = {
                **{field: sum(row.get(f'{kind}_{field}', 0) for row in gpu_rows) for field in fields},
                'wall_per_sampled_step': summary([row.get(f'{kind}_us', 0) for row in gpu_rows
                                                  if row.get(f'{kind}_calls', 0)]),
            }
    emote = {}
    for row in parse_named_rows(log, 'emote_gpu', session):
        if whole_session or frames and frames[0] <= row['frame'] <= frames[-1]:
            key = row['frame'], row['call']
            if emote.setdefault(key, row) != row:
                raise ValueError('Conflicting E-mote GPU record for one frame and step call.')
    if emote:
        emote_rows = list(emote.values())
        timings = ('collect_us', 'evaluate_us', 'prepare_us', 'scanlines_cpu_us',
                   'pack_cpu_us', 'encode_us', 'readback_us', 'repair_us')
        result['emote_gpu'] = {
            'rows': len(emote_rows),
            'poses': sum(row['poses'] for row in emote_rows),
            'draws': sum(row['draws'] for row in emote_rows),
            'batches': sum(row.get('batches', row['draws']) for row in emote_rows),
            'pixels': sum(row['pixels'] for row in emote_rows),
            'repaired': sum(row['repaired'] for row in emote_rows),
            'timings': {field: summary([row[field] for row in emote_rows]) for field in timings},
            'timing_note': 'collect includes evaluate and prepare; scanlines and pack are CPU work inside prepare',
        }
    if unchanged:
        measures = ('step_us', 'script_us', *SCRIPT_PHASES, *SCRIPT_LOAD_PHASES, 'image_read_us',
                    'image_decode_us', 'image_patch_us', 'compose_us', *COMPOSE_PHASES,
                    *LAYER_PHASES)
        result['unchanged_frames'] = {
            'samples': len(unchanged),
            'metrics': {field: summary([row[field] for row in unchanged if field in row])
                        for field in measures if any(field in row for row in unchanged)},
        }
    for key, fields in (('compose_subphases', COMPOSE_PHASES), ('layer_subphases', LAYER_PHASES)):
        completed = [row for row in (*rows, *unchanged) if row['frame'] and fields[0] in row]
        if not completed:
            continue
        totals = {field: sum(row[field] for row in completed) for field in fields}
        result[key] = {
            field: {**summary([row[field] for row in completed]), 'total_ms': totals[field] / 1000}
            for field in sorted(fields, key=lambda field: -totals[field])
        }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('log', type=Path, help='raw or threadtime Logcat with CS2Perf rows')
    parser.add_argument('--session', type=int, required=True)
    parser.add_argument('--first-post', type=int, default=1)
    parser.add_argument('--last-post', type=int, default=(1 << 63) - 1)
    parser.add_argument('--surface-snapshots', type=Path,
                        help='filter to the actual-present time span from the same device boot and replay phase')
    args = parser.parse_args()
    try:
        log = re.sub(r'^.*?\bCS2Perf[ \t]*:[ \t]*', '',
                     args.log.read_text(encoding='utf-8-sig'), flags=re.MULTILINE)
        rows = parse(log, args.session, args.first_post, args.last_post)
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
        result = report(rows)
        whole_session = (args.first_post == 1 and args.last_post == (1 << 63) - 1
                         and args.surface_snapshots is None)
        append_session_metrics(result, log, args.session, rows, whole_session)
        print(json.dumps(result, indent=2))
    except (ValueError, OSError) as error:
        parser.exit(2, f'{error}\n')


if __name__ == '__main__':
    main()

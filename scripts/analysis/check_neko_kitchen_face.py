"""Check two NEKOPARA kitchen poses from a PC face13 host log, without images."""

import argparse
import json
import re
from pathlib import Path


MODEL = re.compile(r'^neko-emote frame=(\d+) task=(\d+) resource=ca01_l_body_1\.psz object="all_parts" .*\bclock=(\d+)\b')
WAIT_LOOP = '\u5dee\u5206\u7528_waiting_loop'
RASTER = 'neko-emote rgba origin=[0, 0] size=1280x720'
POSES = {77: (30, (255, 224, 206, 255)), 279: (10, (255, 241, 237, 255))}
COLOR = re.compile(r'^face13 color order=(\d+) physical=\d+ draw=(\d+) z=(\S+) owners=(.*)$', re.M)
SPRITE = re.compile(r'^face13 sprite draw=(\d+) src="tex" icon="(\d+)" bounds=\[([^\]]+)\] opacity=(\S+) mask=(true|false)$', re.M)
MOTION = re.compile(r'^face13 motion "face_parts"/"([\u76ee\u7709][RL])" .* sample=(\d+)\b', re.M)
RGBA = r'\[(\d+, \d+, \d+, \d+)\]'


def collect(lines):
    blocks = {clock: [] for clock in POSES}
    active = None
    for line in lines:
        if line.startswith('neko-emote frame='):
            match = MODEL.match(line)
            active = None
            if match and WAIT_LOOP in line:
                frame, task, clock = map(int, match.groups())
                if clock in POSES:
                    active = (frame, task, clock, [])
        elif active and line.startswith('face13 '):
            active[3].append(line)
        elif active and line.startswith('neko-emote rgba '):
            frame, task, clock, records = active
            trace = ''.join(records)
            if (line.rstrip() == RASTER and trace.startswith(f'face13 begin clock={clock} ')
                    and COLOR.search(trace)):
                blocks[clock].append((frame, task, trace))
            active = None
    return blocks


def sprite_payload(draw, sprite):
    return draw['details'][sprite.end():].split('face13 sprite ', 1)[0]


def changed_pixel(payload, point):
    match = re.search(rf'^face13 pixel \({point}\) pass alpha=(\S+) .* rgba={RGBA}->{RGBA}$', payload, re.M)
    if not match or float(match[1]) <= 0 or match[2] == match[3]:
        return None
    return tuple(map(int, match[3].split(', ')))


def check_pose(trace, clock):
    failures = []
    eye_time, mouth_expected = POSES[clock]
    eyes = {'0042': '585,272', '0050': '694,272'} if clock == 77 else {'0057': '585,272', '0058': '694,272'}
    depths = {'0019': 5, '0020': 22, '0008': 23, '0018': 26, '0035': 29, '0029': 30}
    depths.update(zip(eyes, (37, 38) if clock == 77 else (33, 34)))
    draws = {}
    colors = list(COLOR.finditer(trace))
    for index, color in enumerate(colors):
        end = colors[index + 1].start() if index + 1 < len(colors) else len(trace)
        details = trace[color.end():end]
        for sprite in SPRITE.finditer(details):
            draw_id, icon, bounds, opacity, mask = sprite.groups()
            if draw_id != color[2] or mask != 'false' or icon not in depths or float(opacity) <= 0:
                continue
            if icon in draws:
                failures.append(f'duplicate color draw for atlas {icon}')
            draws[icon] = {'draw': int(draw_id), 'order': int(color[1]), 'z': float(color[3]),
                           'owners': color[4], 'bounds': [float(value) for value in bounds.split(', ')],
                           'details': details, 'sprite': sprite}

    expected_samples = {f'{part}{side}': sample for part, sample in (('\u76ee', eye_time), ('\u7709', 30)) for side in 'RL'}
    motion_markers = [(part, int(sample)) for part, sample in MOTION.findall(trace)]
    samples = dict(motion_markers)
    if len(motion_markers) != len(expected_samples) or samples != expected_samples:
        failures.append(f'eye/brow motion samples differ: {samples}')
    if missing := depths.keys() - draws.keys():
        failures.append(f'missing color draws: {", ".join(sorted(missing))}')
    else:
        orders = [draws[icon]['order'] for icon in depths]
        if orders != sorted(set(orders)) or any(draws[icon]['z'] != depth for icon, depth in depths.items()):
            failures.append('wrong layer order: back hair < face < mouth < front hair < brows < eyes')

    mouth = draws.get('0008')
    mouth_rgba = changed_pixel(sprite_payload(mouth, mouth['sprite']), '640,334') if mouth else None
    if mouth_rgba != mouth_expected:
        failures.append(f'mouth task pixel at (640,334): {mouth_rgba}, expected {mouth_expected}')

    eye_results = {}
    for icon, point in eyes.items():
        if icon not in draws:
            continue
        draw = draws[icon]
        pixel = changed_pixel(sprite_payload(draw, draw['sprite']), point)
        eye_results[icon] = {'point': point, 'rgba': pixel}
        if clock == 77:
            if draw['owners'] != '[]' or pixel is None:
                failures.append(f'eyelid {icon} did not change unmasked eye pixel ({point})')
            continue
        owner, mask_id, mask_icon, expected = ((268, 264, '0059', (138, 44, 51, 255)) if icon == '0057'
                                               else (242, 238, '0063', (142, 47, 52, 255)))
        mask_sprites = [sprite for sprite in SPRITE.finditer(draw['details'])
                        if sprite[1] == str(mask_id) and sprite[2] == mask_icon and sprite[5] == 'true']
        mask_write = f'face13 pixel ({point}) mask alpha=255 ref=0 value=0->1'
        stencil = (draw['owners'] == f'[(8, Some({owner}))]'
                   and 'face13 plane clear=0 kinds=[5] final_ref=1' in draw['details']
                   and 'face13 color_stencil ref=1 ' in draw['details']
                   and len(mask_sprites) == 1 and mask_sprites[0].start() < draw['sprite'].start()
                   and mask_write in sprite_payload(draw, mask_sprites[0]).splitlines())
        eye_results[icon]['stencil_0_to_1'] = stencil
        if not stencil or pixel != expected:
            failures.append(f'iris {icon} stencil or task pixel mismatch at ({point})')

    layers = {icon: {key: draw[key] for key in ('draw', 'order', 'z', 'bounds')} for icon, draw in draws.items()}
    return {'motion_samples': samples, 'layers': layers, 'mouth_rgba': mouth_rgba, 'eyes': eye_results}, failures


def verify(blocks):
    results, failures, missing = {}, [], []
    timing = None
    for clock, candidates in blocks.items():
        if len(candidates) != 1:
            missing.append(f'model clock {clock}: expected one completed face13 block, found {len(candidates)}')
            continue
        frame, task, trace = candidates[0]
        result, errors = check_pose(trace, clock)
        results[clock] = {'task': task, 'logic_frame': frame, 'completed_frame': frame + 1, **result}
        failures.extend(f'f{frame}/clock {clock}: {error}' for error in errors)
    if len(results) == len(POSES):
        early, late = results[77], results[279]
        if early['task'] != late['task']:
            failures.append('the two phases belong to different raster tasks')
        frame_span = late['logic_frame'] - early['logic_frame']
        if frame_span <= 0:
            failures.append('model clock advanced out of replay frame order')
        if early['task'] == late['task'] and frame_span > 0:
            timing = {'task': early['task'], 'logic_frame_span': frame_span, 'model_clock_span': 279 - 77}
        for icon in ('0035', '0029'):
            first, second = early['layers'].get(icon), late['layers'].get(icon)
            if first and second and first['bounds'] == second['bounds']:
                failures.append(f'brow {icon} bounds did not move between the two poses')
    status = 'failed' if failures else 'insufficient_evidence' if missing else 'passed'
    return {'status': status, 'poses': results, 'timing': timing, 'failures': failures, 'missing': missing}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('host_log', type=Path, help='host log with neko-emote and face13 diagnostics')
    args = parser.parse_args()
    with args.host_log.open(encoding='utf-8', errors='replace') as host:
        report = verify(collect(host))
    print(json.dumps(report, indent=2))
    return {'passed': 0, 'failed': 1, 'insufficient_evidence': 2}[report['status']]


if __name__ == '__main__':
    raise SystemExit(main())

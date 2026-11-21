"""Targeted FES media commands in the existing twenty-sample inventory.

Only loose FES and FES/config/system/update KIF entries are read. No engine,
VM, media decoder, compiler or test process is invoked.
"""
from __future__ import annotations
import argparse
import ast
import hashlib
import json
from pathlib import Path
import re
import struct
import subprocess
import sys
import zlib

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[2]

SOURCES = ['simulator/crates/desktop/src/' + name for name in
           ['fes_sound.rs', 'fes_playlist.rs', 'sound_media.rs', 'fes_motion.rs',
            'fes_text.rs', 'fes_log.rs', 'fes_text_state.rs']]


def pe_evidence():
    from recovery_A_20261116 import Image
    path = ROOT / 'Yukikoi Melt/YukikoiMelt.exe'
    image = Image(path, '63de0e14cd8ce11ad2d68e7ed6076641ff5e5b73d822603cc52764009d812976')
    ranges = [
        ('table_constructor', 0x2bf5d0, 0x2bf63c),
        ('table_destroy', 0x2bf640, 0x2bf689),
        ('table_resize_entry_order', 0x2bf690, 0x2bf842),
        ('table_shuffle', 0x2bf850, 0x2bf9ab),
        ('table_repeat_select', 0x2bf9b0, 0x2bfb2d),
        ('table_getid_next_reverse', 0x2bfcb0, 0x2bfe7d),
        ('table_initializer', 0x2c62e0, 0x2c6371),
        ('setid_callback', 0x2c8bc0, 0x2c8caa),
        ('getid_callback', 0x2c8ce0, 0x2c8d9c),
        ('stop_callback', 0x2c9d00, 0x2c9da1),
        ('sndtbl_callback', 0x2cae90, 0x2caff5),
        ('next_callback', 0x2cb000, 0x2cb0f5),
        ('reverse_callback', 0x2cb100, 0x2cb1f3),
        ('repeat_shuffle_callbacks', 0x2cb200, 0x2cb36d),
        ('loop_callback', 0x2d8290, 0x2d8329),
        ('duration_time_callback', 0x2d5b50, 0x2d5d12),
        ('duration_getter_setter', 0x2a6f40, 0x2a6f5e),
        ('loaded_duration_result', 0x1b8370, 0x1b83df),
        ('crossfade_callback', 0x2d8b80, 0x2d8ca3),
        ('crossfade_setter', 0x2a71e0, 0x2a71f7),
        ('load_mode_getter', 0x2a6ba0, 0x2a6bba),
        ('play_count_setter_getter', 0x2a6c70, 0x2a6cb1),
        ('media_load_consumer', 0x1b8200, 0x1b8500),
        ('shared_settings_volume_pan_consumer', 0x1b8571, 0x1b874e),
        ('play_callback_action_clear', 0x1b87a3, 0x1b88f4),
        ('alias_setter_getter', 0x2a6e80, 0x2a6eb7),
        ('sound_play_callback_switch', 0x2ca6de, 0x2ca741),
        ('sound_result_flag_setter_getter', 0x2a6ee0, 0x2a6f00),
        ('sound_result_flag_consumer', 0x180150, 0x180191),
        ('sound_table_same_media_consumer', 0x1b81f0, 0x1b81f5),
        ('retained_media_binding_lookup', 0x180070, 0x1800c4),
        ('missing_binding_media_kind_dispatch', 0x180367, 0x180382),
        ('sound_result_unload_consumer', 0x180536, 0x18055b),
        ('media_record_unlink_destroy', 0x180588, 0x1805d1),
        ('owner_release_unbind_then_media', 0x17acd3, 0x17ad12),
        ('sound_base_enable_getter', 0x2a28a0, 0x2a28a4),
        ('sound_delete_callback', 0x2ca990, 0x2ca9e6),
        ('recursive_object_delete', 0x2b2bb0, 0x2b2c2c),
        ('sound_destructor', 0x2a6a10, 0x2a6a90),
        ('base_destructor_detach_motion', 0x2a2620, 0x2a26de),
    ]
    rows = []
    for name, start, end in ranges:
        data = image.get(start, end-start)
        rows.append(dict(name=name, start_rva=hex(start), end_rva_exclusive=hex(end),
            sha256=hashlib.sha256(data).hexdigest(), bytes=data.hex(),
            instructions=[dict(rva=hex(i.address-image.base), op=i.mnemonic, args=i.op_str)
                          for i in image.md.disasm(data, image.base+start)]))
    targets = {0x2a6e80: [], 0x2b5030: [], 0x2b4fa0: [], 0x2bfb30: [],
               0x2a71e0: [], 0x2a6ef0: [], 0x180070: []}
    for section in image.sections:
        if not section['executable']: continue
        data = section['data']
        position = data.find(b'\xe8')
        while position >= 0:
            if position+5 <= len(data):
                target = section['rva']+position+5+struct.unpack_from('<i', data, position+1)[0]
                if target in targets: targets[target].append(hex(section['rva']+position))
            position = data.find(b'\xe8', position+1)
    output = ROOT / 'reports/fes_media_E_PE_20261117.json'
    dispatch = {}
    for kind in (9, 10):
        live = struct.unpack('<I', image.get(0x1805d4 + (kind-1)*4, 4))[0]-image.base
        slot = image.get(0x180698 + kind-1, 1)[0]
        missing = struct.unpack('<I', image.get(0x180674 + slot*4, 4))[0]-image.base
        dispatch[str(kind)] = dict(live_target_rva=hex(live), missing_target_rva=hex(missing))
    output.write_text(json.dumps(dict(mode='fixed-PE-read-only-not-execution',
        source=path.relative_to(ROOT).as_posix(), sha256=image.digest, image_base=hex(image.base),
        direct_call_candidates={hex(key): value for key, value in targets.items()},
        sound_media_dispatch=dispatch,
        ranges=rows, source_files=SOURCES,
        boundaries=['setid takes order position; getid returns actual entry id',
                    'next/rev callbacks request load only; no NEXT/REV notice is produced',
                    'loop changes current repeat count without restarting the source',
                    'crossfade writes subsequent load mode/span without requesting a load',
                    'time splits the cached real source duration into minutes/seconds/milliseconds',
                    'media volume/pan consumes shared settings dirty OR object dirty without reloading or restarting playback',
                    'play second argument writes +188 exported in media result +54; playback183/registration218 reads distinct base +14',
                    'object destruction does not stop playback185; retained media result +54/+4c conditionally releases channel182 after binding disappears',
                    'alias setter has no direct call evidence; indirect setter and NEXT/REV producers remain open'],
        compiler_invoked=False, tests_run=False, engine_executed=False), indent=2)+'\n', encoding='utf-8')
    print(json.dumps(dict(report=output.relative_to(ROOT).as_posix(), ranges=len(rows))))


def syntax():
    rows = []
    for name in SOURCES:
        raw = (ROOT/name).read_bytes()
        result = subprocess.run([str(Path.home()/'.cargo/bin/rustfmt.exe'),
            '--edition', '2024', '--emit', 'stdout', '--config', 'skip_children=true'],
            input=raw, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, check=False)
        rows.append(dict(source=name, sha256=hashlib.sha256(raw).hexdigest(),
            exit_code=result.returncode, diagnostics=result.stderr.decode('utf-8', 'replace')))
    ast.parse(Path(__file__).read_text(encoding='utf-8'))
    output = ROOT/'archive/2026-11/20261117_fes_media_E_source_syntax.json'
    output.write_text(json.dumps(dict(mode='source-grammar-only-not-types-or-runtime',
        compiler_invoked=False, tests_run=False, engine_executed=False, sources=rows), indent=2)+'\n', encoding='utf-8')
    failures = sum(row['exit_code'] != 0 for row in rows)
    print(json.dumps(dict(report=output.relative_to(ROOT).as_posix(), failures=failures)))
    if failures: raise SystemExit(1)


def image_source_evidence():
    """One already-pinned FES, its real image files; no scene/decoder runs."""
    source = 'cs2_full_v401/system/fes/cgmode.fes'
    data = (ROOT / source).read_bytes()
    digest = hashlib.sha256(data).hexdigest()
    if digest != 'f6dce21feafe66f0567875a60c62cab94daf5aa339750f2b47b39238dcd8c8a5':
        raise ValueError('pinned cgmode FES changed')
    stored, length = struct.unpack_from('<II', data, 4)
    body = zlib.decompress(data[16:16 + stored])
    if len(body) != length: raise ValueError('FES container length mismatch')
    rows = [dict(line=number, text=raw.decode('ascii', 'backslashreplace'))
            for number, raw in enumerate(body.splitlines(), 1)
            if b'btn_thumb' in raw]
    images = []
    for index in range(3):
        name = f'cs2_full_v401/system/image/sys_cg_t{index}.hg3'
        raw = (ROOT / name).read_bytes()
        images.append(dict(source=name, length=len(raw),
            sha256=hashlib.sha256(raw).hexdigest(), magic=raw[:4].hex()))
    output = ROOT / 'archive/2026-11/20261117_fes_image_source_E.json'
    output.write_text(json.dumps(dict(mode='pinned-FES-and-resource-static-read-only',
        source=source, sha256=digest, rows=rows, images=images,
        contracts=['resources/reference/KatzeSystem2/docs/fes.md:94',
                   'resources/reference/KatzeSystem2/docs/fes.md:102',
                   'resources/reference/KatzeSystem2/docs/graphics.md:677',
                   'resources/reference/KatzeSystem2/docs/graphics.md:764'],
        remaining=['PLANE reads the shared canvas, not a FILE load',
                   'multi-file zero-alpha composition receiver',
                   'IDN=-1 retained raw multi-frame source',
                   'ANM continues through its separate existing receiver'],
        compiler_invoked=False, tests_run=False, engine_executed=False), indent=2) + '\n', encoding='utf-8')
    print(json.dumps(dict(report=output.relative_to(ROOT).as_posix(), images=len(images))))


def image_source_syntax():
    names = ['fes_image_source.rs', 'fes_media.rs', 'fes_task_platform.rs', 'lib.rs']
    rows = []
    for name in names:
        source = 'simulator/crates/desktop/src/' + name
        raw = (ROOT / source).read_bytes()
        result = subprocess.run([str(Path.home()/'.cargo/bin/rustfmt.exe'),
            '--edition', '2024', '--emit', 'stdout', '--config', 'skip_children=true'],
            input=raw, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, check=False)
        rows.append(dict(source=source, sha256=hashlib.sha256(raw).hexdigest(),
            exit_code=result.returncode, diagnostics=result.stderr.decode('utf-8', 'replace')))
    ast.parse(Path(__file__).read_text(encoding='utf-8'))
    output = ROOT / 'archive/2026-11/20261117_fes_image_source_E_syntax.json'
    output.write_text(json.dumps(dict(mode='source-grammar-only-not-types-or-runtime',
        compiler_invoked=False, tests_run=False, engine_executed=False, sources=rows), indent=2) + '\n', encoding='utf-8')
    failures = sum(row['exit_code'] != 0 for row in rows)
    print(json.dumps(dict(report=output.relative_to(ROOT).as_posix(), failures=failures)))
    if failures: raise SystemExit(1)


def inspect(data, source):
    if data[:4] != b'FES\0' or len(data) < 16:
        raise ValueError('not a FES container')
    compressed, expanded = struct.unpack_from('<II', data, 4)
    if expanded > 16 * 1024 * 1024 or compressed + 16 > len(data):
        raise ValueError('FES size exceeds its container')
    body = zlib.decompress(data[16:16 + compressed])
    if len(body) != expanded:
        raise ValueError('FES expanded size mismatch')
    rows = body.splitlines()
    hits, media, initializers = [], {}, []
    section, columns, header = b'', [], b''
    for number, raw in enumerate(rows, 1):
        line = raw.split(b'//', 1)[0].strip()
        words = line.split()
        if not words:
            continue
        if line.startswith(b'#'):
            section = words[0].upper()
            header = line
            if section == b'#OBJECT': columns = words[1:]
            continue
        if section == b'#DEFINE' and words[0].upper() in (b'SOUND', b'SNDTABLE'):
            media[words[1].split(b'[')[0]] = words[0].decode('ascii')
        target = words[0].split(b'[')[0]
        if target in media:
            record = dict(line=number, text=raw.decode('cp932', 'backslashreplace'),
                          header=header.decode('cp932', 'backslashreplace'))
            if section == b'#OBJECT':
                record['columns'] = [word.decode('ascii') for word in columns]
                initializers.append(record)
            else:
                hits.append(record)
        elif re.search(rb'\balias\b', line, re.I):
            hits.append(dict(line=number, text=raw.decode('cp932', 'backslashreplace'), kind='literal-alias'))
    return dict(source=source, sha256=hashlib.sha256(data).hexdigest(),
                body_sha256=hashlib.sha256(body).hexdigest(),
                media={key.decode('cp932', 'backslashreplace'): value for key, value in media.items()},
                initializers=initializers, hits=hits)


def main():
    from startup_inventory import SKIP, entries_at, normalized
    from audit_runtime_evidence import blowfish_le_decrypt
    inventory = json.loads((ROOT / 'reports/all_sample_content_audit.json').read_text(encoding='utf-8'))
    games = []
    for game in inventory['games']:
        root = Path(game['root'])
        seeds = {normalized(name): int(value.split()[0], 0)
                 for name, value in game['archives']['selected_name_seeds'].items()}
        records, diagnostics = [], []
        paths = sorted(set(root.rglob('*.fes')) | set(root.rglob('*.int')) | set(root.rglob('*.INT')))
        for path in paths:
            if any(part.lower() in SKIP for part in path.relative_to(root).parts):
                continue
            relative = path.relative_to(root).as_posix()
            try:
                if path.suffix.lower() == '.fes':
                    records.append(inspect(path.read_bytes(), relative))
                    continue
                stem = path.stem.lower()
                if not (stem in {'fes', 'cnfes', 'config', 'system'} or stem.startswith('update')):
                    continue
                entries, _, cipher = entries_at(path, seeds.get(normalized(relative), 0))
                with path.open('rb') as handle:
                    for entry in entries:
                        name = entry['name'].decode('cp932', 'backslashreplace')
                        if not name.lower().endswith('.fes'):
                            continue
                        if entry['size'] > 16 * 1024 * 1024:
                            raise ValueError('FES stored size exceeds limit')
                        handle.seek(entry['offset'])
                        data = handle.read(entry['size'])
                        if cipher:
                            data = blowfish_le_decrypt(cipher, data)
                        records.append(inspect(data, relative + '::' + name))
            except (ValueError, zlib.error) as error:
                diagnostics.append(dict(source=relative, message=str(error)))
        games.append(dict(root=str(root.relative_to(ROOT)), fes_count=len(records),
                          scripts=[row for row in records if row['hits']], diagnostics=diagnostics))
    output = ROOT / 'reports/fes_media_E_contracts_20261117.json'
    output.write_text(json.dumps(dict(mode='read-only-FES-media-command-evidence',
        compiler_invoked=False, engine_executed=False, games=games), ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
    print(json.dumps(dict(report=output.relative_to(ROOT).as_posix(), games=len(games),
        fes=sum(game['fes_count'] for game in games),
        hit_scripts=sum(len(game['scripts']) for game in games),
        diagnostics=sum(len(game['diagnostics']) for game in games))))


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--pe', action='store_true')
    parser.add_argument('--syntax', action='store_true')
    parser.add_argument('--image-source', action='store_true')
    parser.add_argument('--image-source-syntax', action='store_true')
    args = parser.parse_args()
    if args.image_source: image_source_evidence()
    elif args.image_source_syntax: image_source_syntax()
    elif args.pe: pe_evidence()
    elif args.syntax: syntax()
    else: main()

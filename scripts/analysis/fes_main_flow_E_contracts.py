"""Pinned, read-only FES call evidence and source grammar; no engine execution."""
from __future__ import annotations
import argparse
import ast
import hashlib
import json
from pathlib import Path
import struct
import subprocess
import zlib

ROOT = Path(__file__).resolve().parents[2]
SOURCES = [
    'simulator/crates/desktop/src/fes_main_flow.rs',
    'simulator/crates/desktop/src/fes_driver.rs',
    'simulator/crates/desktop/src/fes_services.rs',
    'simulator/crates/desktop/src/fes_task_platform.rs',
    'simulator/crates/desktop/src/fes_media.rs',
    'simulator/crates/desktop/src/fes_motion.rs',
    'simulator/crates/desktop/src/fes_controls.rs',
    'simulator/crates/desktop/src/fes_text.rs',
    'simulator/crates/desktop/src/fes_log.rs',
    'simulator/crates/desktop/src/fes_log_image.rs',
    'simulator/crates/runtime/src/fes.rs',
    'simulator/crates/desktop/src/fes_statement.rs',
    'simulator/crates/desktop/src/message_platform.rs',
    'simulator/crates/desktop/src/lib.rs',
]
FES = {
    'title.fes': '78551e7d16c23c922ac9ef398203d68b6fd77eda750b053e7e36e8dd2b13730b',
    'select.fes': '4174bcc532c687f9547269dd0081acef2e3ad06b370e5ffa60b210874c186646',
    'log_A.fes': '7cc41ce6b7a46059838a770a753b7aeedf10403bbae025eb78601c6bc1fdea49',
    'log_B.fes': 'b6ef1f91b9a4dbf0091f8780525136ced83dabd878325491e32539c852720a6b',
}


def digest(data):
    return hashlib.sha256(data).hexdigest()


def evidence():
    rows = []
    for name, expected in FES.items():
        path = ROOT / 'cs2_full_v401/system/fes' / name
        raw = path.read_bytes()
        if digest(raw) != expected or raw[:4] != b'FES\0':
            raise ValueError(f'pinned FES changed: {name}')
        compressed, expanded = struct.unpack_from('<II', raw, 4)
        if len(raw) != compressed + 16:
            raise ValueError(f'FES container length: {name}')
        body = zlib.decompress(raw[16:])
        if len(body) != expanded:
            raise ValueError(f'FES expanded length: {name}')
        calls = []
        metadata_section = False
        for number, line in enumerate(body.splitlines(), 1):
            if line.startswith(b'#'):
                metadata_section = line.startswith(b'#OBJECT') and any(word in line for word in (b'POS2', b'SIZE2'))
            if metadata_section or any(word in line.lower() for word in (b'noact', b'keypos', b'lockactive', b'attach', b'keyblock', b'pos2', b'size2', b'log ')):
                calls.append(dict(line=number, text=line.decode('cp932')))
        rows.append(dict(path=path.relative_to(ROOT).as_posix(), sha256=expected,
                         expanded_sha256=digest(body), calls=calls))
    references = []
    for name in ('fes.md', 'graphics.md', 'fes-text.md', 'backlog.md'):
        path = ROOT / 'resources/reference/KatzeSystem2/docs' / name
        references.append(dict(path=path.relative_to(ROOT).as_posix(), sha256=digest(path.read_bytes())))
    output = ROOT / 'reports/fes_main_flow_E_contracts_20261117.json'
    output.write_text(json.dumps(dict(date='2026-11-17', mode='static-container-and-source-only',
        engine_executed=False, vm_executed=False, tests_run=False, compiler_invoked=False,
        samples=rows, contract_documents=references, alias_pe=alias_pe(),
        log_receiver=dict(
            source='simulator/crates/desktop/src/fes_log.rs',
            contract_sources=[dict(path=name, sha256=digest((ROOT / name).read_bytes())) for name in (
                'resources/reference/KatzeSystem2/src/runtime/fes/logstring.rs',
                'resources/reference/KatzeSystem2/src/backlog.rs')],
            owner='existing fes_text owner; independent surfaces for visible history rows',
            container='existing LoggingHost.backlogs; no duplicate retained history',
            consumer='MessageObject -> DesktopMessageDevice submitted RGBA -> existing text projections',
            release='setlog 0 unbinds; owner release frees all row surfaces and leaves Backlog alive',
            remaining=['localized remake/subtitle producer',
                       'media alias setter and real NEXT/REV producers']),
        scope=['noact', 'lockactive', 'keypos', 'attach', 'EXPOS',
               'POS2/SIZE2 plane, CIMAGE, IMAGE position and STRING layout',
               'media alias noact/lockactive ordered notice and raw return',
               'BUTTON 9/10 key slots 4..7', 'input 139', 'input reply 95',
               'attached slider 369 and SCROLL2 377']),
        ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
    print(output.relative_to(ROOT).as_posix())


def alias_pe():
    # Fixed dispatcher and callbacks only. No broad PE scan or engine loading.
    from recovery_A_20261116 import Image, instruction_row
    source = 'Yukikoi Melt/YukikoiMelt.exe'
    expected = '63de0e14cd8ce11ad2d68e7ed6076641ff5e5b73d822603cc52764009d812976'
    image = Image(ROOT / source, expected)
    spans = [('single_statement_dispatch', 0x2c7350, 0x2c75bf),
             ('object_callback_table_dispatch', 0x2c76d8, 0x2c7756),
             ('noact_callback_return_zero', 0x2cada0, 0x2cae8b),
             ('lockactive_callback_return_minus_three', 0x2de800, 0x2de8ab),
             ('media_alias_setter_and_getter_field_164', 0x2a6e80, 0x2a6eb7),
             ('natural_stop_notification_one', 0x17afbc, 0x17b033)]
    records = []
    for label, start, end in spans:
        raw = image.get(start, end - start)
        records.append(dict(label=label, start_rva=hex(start), end_rva=hex(end), sha256=digest(raw),
            instructions=[instruction_row(instruction, image.base)
                          for instruction in image.md.disasm(raw, image.base + start)]))
    return dict(source=source, sha256=expected, ranges=records,
                returns=dict(noact=0, lockactive=-3),
                notices=dict(noact=7, lockactive=22),
                effects='append existing scene notices; caller converts raw integer to bool')


def log_projection_evidence():
    """Only the two pinned history files and bounded existing PE entries."""
    output = ROOT / 'reports/fes_main_flow_E_contracts_20261117.json'
    saved = json.loads(output.read_text(encoding='utf-8'))
    samples = []
    for name in ('log_A.fes', 'log_B.fes'):
        path = ROOT / 'cs2_full_v401/system/fes' / name
        raw = path.read_bytes()
        if digest(raw) != FES[name] or raw[:4] != b'FES\0':
            raise ValueError(f'pinned FES changed: {name}')
        compressed, expanded = struct.unpack_from('<II', raw, 4)
        if len(raw) != compressed + 16:
            raise ValueError(f'FES container length: {name}')
        body = zlib.decompress(raw[16:])
        if len(body) != expanded:
            raise ValueError(f'FES expanded length: {name}')
        calls, viewports = [], []
        for number, raw_line in enumerate(body.splitlines(), 1):
            line = raw_line.split(b'//', 1)[0].strip()
            words = line.split()
            if b'VIEWPORT' in line.upper():
                viewports.append(number)
            if words and (words[0].split(b'[')[0] in (b'name', b'log')
                          or words[:2] in ([b'IMAGE', b'name[4]'], [b'STRING', b'name[4]'])):
                calls.append(dict(line=number, text=raw_line.decode('cp932')))
        samples.append(dict(path=path.relative_to(ROOT).as_posix(), sha256=digest(raw),
                            body_sha256=digest(body), calls=calls, viewport_lines=viewports))
    from recovery_A_20261116 import Image, instruction_row
    source = 'Yukikoi Melt/YukikoiMelt.exe'
    expected = '63de0e14cd8ce11ad2d68e7ed6076641ff5e5b73d822603cc52764009d812976'
    image = Image(ROOT / source, expected)
    table_rva = 0x512160 + 21 * 20
    entry = image.get(table_rva, 20)
    if entry[:16].split(b'\0')[0] != b'settbl' or struct.unpack_from('<I', entry, 16)[0] != image.base + 0x2cb370:
        raise ValueError('fixed settbl callback entry changed')
    records = []
    for label, start, end in (
        ('settbl_callback_evaluate_once_and_stop_at_hole', 0x2cb370, 0x2cb4cb),
        ('image_button_table_setter_and_frame_resolver', 0x2a4600, 0x2a4655),
        ('setid_callback', 0x2c8bc0, 0x2c8caa),
    ):
        raw = image.get(start, end - start)
        records.append(dict(label=label, start_rva=hex(start), end_rva_exclusive=hex(end),
                            sha256=digest(raw), instructions=[instruction_row(i, image.base)
                            for i in image.md.disasm(raw, image.base + start)]))
    saved['log_projection'] = dict(
        mode='pinned-two-container-and-bounded-PE-read-only', samples=samples,
        pe=dict(source=source, sha256=expected, table_entry_rva=hex(table_rva),
                table_entry_bytes=entry.hex(), ranges=records),
        viewport='existing controls reference/cursor/codec; every original text bank and allocated history row uses its root layer key at projection submission',
        viewport_sample_boundary='neither pinned log_A nor log_B contains VIEWPORT; no viewport sample hit claimed',
        name_image='log_A lines 410/418/426/434 write original frame_ids[0]; next setid 0 uses existing resource decoder and pixel/list consumer',
        state_boundary='no new viewport, image, history, pixel or input map; no text codec field change',
        remaining=['localized remake/subtitle producer', 'media alias setter and real NEXT/REV producers without sample evidence',
                   'history page execution and viewport runtime validation prohibited in this phase'])
    saved['log_receiver']['remaining'] = ['localized remake/subtitle producer',
        'media alias setter and real NEXT/REV producers']
    output.write_text(json.dumps(saved, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
    print(json.dumps(dict(report=output.relative_to(ROOT).as_posix(),
                         pinned_history_files=len(samples), viewport_hits=sum(len(s['viewport_lines']) for s in samples))))


def log_consumption_evidence():
    # Reuse already-pinned call evidence; do not reopen or scan game archives.
    saved = json.loads((ROOT / 'reports/fes_main_flow_E_contracts_20261117.json').read_text(encoding='utf-8'))
    samples = []
    for sample in saved['log_projection']['samples']:
        calls = [call for call in sample['calls'] if call['text'].split()[:2] in (
            ['log', 'setlog'], ['log', 'draw'], ['log', 'disp'], ['log', 'next'],
            ['log', 'rev'], ['log', 'page'], ['log', 'pos'], ['log', 'blockcolor'])]
        samples.append(dict(path=sample['path'], sha256=sample['sha256'], calls=calls))
    sources = ['simulator/crates/desktop/src/fes_log.rs',
        'simulator/crates/desktop/src/fes_text.rs',
        'simulator/crates/desktop/src/fes_text_state.rs']
    output = ROOT / 'archive/2026-11/20261117_fes_log_consumption_E.json'
    output.write_text(json.dumps(dict(mode='existing-call-evidence-and-static-source-only',
        evidence='reports/fes_main_flow_E_contracts_20261117.json#log_projection',
        samples=samples, sources={name: digest((ROOT / name).read_bytes()) for name in sources},
        producer='setlog/scroll/draw/disp/palette -> original LogString.redraw; template configuration/glyph requests',
        consumer='stage every existing row -> retire template batch -> render_bank per-row pending/mapping/draw/frame cursors',
        correction='later row failure no longer replaces completed rows or clears unconsumed row character requests on retry',
        state='no added serialized fields, component version or cross-owner hook; same surfaces, bank keys, Backlog and list projections',
        remaining=['no sample VIEWPORT line in pinned log_A/log_B; existing receiver retained',
            'IMAGE settbl/setid already connected; no duplicate resource consumer',
            'independent subtitle surfaces remain an explicit refusal; no fabricated rows',
            'history UI/device/runtime validation not performed'],
        compiler_invoked=False, type_check_run=False, tests_run=False,
        original_code_executed=False), ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
    print(json.dumps(dict(report=output.relative_to(ROOT).as_posix(), samples=len(samples), state_fields_added=0)))


def log_consumption_syntax():
    from settings_menu_E_contracts import syntax as source_syntax
    ast.parse(Path(__file__).read_text(encoding='utf-8'))
    source_syntax(['simulator/crates/desktop/src/fes_log.rs'],
        '20261117_fes_log_consumption_E_source_syntax.json')


def select_consumption_evidence():
    """Two pinned choice resources and their fixed return/character entries."""
    from startup_inventory import entries_at
    from audit_runtime_evidence import blowfish_le_decrypt, operand_count
    from recovery_A_20261116 import Image, instruction_row

    def entry(archive, seed, name, expected):
        entries, _, cipher = entries_at(ROOT / archive, seed)
        found = next(row for row in entries if row['name'].lower() == name)
        with (ROOT / archive).open('rb') as stream:
            stream.seek(found['offset'])
            raw = stream.read(found['size'])
        if cipher:
            raw = blowfish_le_decrypt(cipher, raw)
        if digest(raw) != expected:
            raise ValueError('pinned choice resource changed: ' + archive)
        return raw

    yukikoi = entry('Yukikoi Melt/fes.int', 0x23a893a2, b'select.fes',
        'c5a6c4a4301a94c9e23547b62e6ff0ccbfa64cdac63cebfedfaf55099d108cfe')
    v401 = (ROOT / 'cs2_full_v401/system/fes/select.fes').read_bytes()
    if digest(v401) != FES['select.fes']:
        raise ValueError('pinned V401 choice changed')
    samples = []
    for name, raw, spans in (
        ('cs2_full_v401/system/fes/select.fes', v401,
         [(33, 35), (124, 140), (173, 184), (187, 211), (228, 236)]),
        ('Yukikoi Melt/fes.int::select.fes', yukikoi,
         [(1123, 1127), (1162, 1173), (1191, 1207), (1226, 1242), (1307, 1320), (1352, 1357)]),
    ):
        body = zlib.decompress(raw[16:])
        samples.append(dict(path=name, sha256=digest(raw), body_sha256=digest(body),
            lines=[dict(line=i, raw_hex=line.hex(), escaped=repr(line))
                   for i, line in enumerate(body.splitlines(), 1)
                   if any(start <= i <= end for start, end in spans)]))

    image = Image(ROOT / 'Yukikoi Melt/YukikoiMelt.exe',
        '63de0e14cd8ce11ad2d68e7ed6076641ff5e5b73d822603cc52764009d812976')
    pe_ranges = []
    for label, start, end in (
        ('layout_font_raw_remainder_and_unicode_dispatch', 0x2d127d, 0x2d136b),
        ('wide_font_copy_inheritance_and_dirty', 0x2b0ba0, 0x2b0bfc),
        ('utf8_wide_conversion_and_original_stride', 0x1ff280, 0x1ff3be),
    ):
        raw = image.get(start, end - start)
        pe_ranges.append(dict(label=label, start_rva=hex(start), end_rva=hex(end),
            sha256=digest(raw), instructions=[instruction_row(i, image.base)
                for i in image.md.disasm(raw, image.base + start)]))
    table = image.get(0x512160 + 56 * 20, 20)
    if table[:16].split(b'\0')[0] != b'layout' or struct.unpack_from('<I', table, 16)[0] != image.base + 0x2cf840:
        raise ValueError('fixed layout callback changed')

    raw = entry('cs2_full_v401/system/kcs.int', 0, b'sscript.kcs',
        '17de548390b8c2b15a4f413418bbf1b408d6dee6ab830879f09cf305f5c128fd')
    code = zlib.decompress(raw[52:])[:struct.unpack_from('<I', raw, 16)[0]]
    instructions, cursor = [], 0
    while cursor < len(code):
        opcode = code[cursor]
        count = operand_count(opcode)
        if count is None:
            raise ValueError('unknown pinned KCS opcode')
        if 0xc3300 <= cursor < 0xc3960:
            instructions.append(dict(offset=hex(cursor), opcode=hex(opcode),
                operands=[dict(flags=hex(code[cursor + 1 + i * 5]),
                    value=hex(struct.unpack_from('<I', code, cursor + 2 + i * 5)[0]))
                    for i in range(count)]))
        cursor += 1 + count * 5
    sources = ['simulator/crates/desktop/src/' + name for name in (
        'fes_text.rs', 'fes_log.rs', 'fes_driver.rs', 'fes_task_platform.rs',
        'fes_tasks.rs', 'native_tasks.rs', 'message_backend.rs')]
    output = ROOT / 'archive/2026-11/20261117_fes_select_consumption_E.json'
    output.write_text(json.dumps(dict(mode='pinned-data-and-static-source-only',
        samples=samples, pe=dict(sha256=image.digest, layout_table_entry=table.hex(), ranges=pe_ranges),
        kcs=dict(resource='cs2_full_v401/system/kcs.int::sscript.kcs', sha256=digest(raw),
            instructions=instructions), sources={name: digest((ROOT / name).read_bytes()) for name in sources},
        correction='Unicode layout font uses the existing original byte stride; leading continuation ends conversion and explicitly clears the face; raw-empty still selects the shared default',
        state='existing MessageObject configuration/inheritance/dirty -> message device fonts and submitted pixels; no new codec or shared hook',
        engine_executed=False, tests_run=False, compiler_invoked=False), indent=2) + '\n', encoding='utf-8')
    print(json.dumps(dict(report=output.relative_to(ROOT).as_posix(), samples=len(samples), pe_ranges=len(pe_ranges))))


def log_event_context_evidence():
    # Reuse fixed sample call lists; no game/archive rescan.
    from recovery_A_20261116 import Image, instruction_row
    saved = json.loads((ROOT / 'reports/fes_main_flow_E_contracts_20261117.json').read_text(encoding='utf-8'))
    samples = [dict(path=s['path'], sha256=s['sha256'],
        calls=[c for c in s['calls'] if 'blockcolor @' in c['text']])
        for s in saved['log_projection']['samples']]
    image = Image(ROOT / 'Yukikoi Melt/YukikoiMelt.exe',
        '63de0e14cd8ce11ad2d68e7ed6076641ff5e5b73d822603cc52764009d812976')
    table_rva = 0x512160 + 76 * 20
    table = image.get(table_rva, 20)
    if table[:16].split(b'\0')[0] != b'blockcolor' or struct.unpack_from('<I', table, 16)[0] != image.base + 0x2d50b0:
        raise ValueError('fixed blockcolor callback changed')
    start, end = 0x2d50b0, 0x2d51d6
    raw = image.get(start, end - start)
    sources = ['simulator/crates/desktop/src/' + name for name in
        ('fes_log.rs', 'fes_text.rs', 'fes_driver.rs')]
    output = ROOT / 'archive/2026-11/20261117_fes_log_event_context_E.json'
    output.write_text(json.dumps(dict(mode='existing-sample-evidence-and-fixed-PE-static-read',
        evidence='reports/fes_main_flow_E_contracts_20261117.json#log_projection', samples=samples,
        pe=dict(sha256=image.digest, table_rva=hex(table_rva), table_bytes=table.hex(),
            start_rva=hex(start), end_rva=hex(end), sha256_range=digest(raw),
            instructions=[instruction_row(i, image.base) for i in image.md.disasm(raw, image.base + start)]),
        sources={name: digest((ROOT / name).read_bytes()) for name in sources},
        producer='original BUTTON target.bank -> driver handler expression.current -> FesDriverCommand.context.current',
        correction='history argument defaults apply only to absent arguments; supplied expressions retain command.context.current, once before bank broadcast',
        consumer='original LogString row_colors/redraw -> allocated TextBank rows -> same MessageObject/device submitted pixels -> same list',
        body_boundary='meswnd apend/draw/isdraw/isdrawmark/skip and ANM icon consumers already exist; no general auto/skip state or budget changes',
        state='no new state, codec fields, shared hook or native route',
        compiler_invoked=False, type_check_run=False, tests_run=False, engine_executed=False), indent=2) + '\n', encoding='utf-8')
    print(json.dumps(dict(report=output.relative_to(ROOT).as_posix(), sample_calls=sum(len(s['calls']) for s in samples))))


def confirmation_control_evidence():
    # Two already-used, pinned loose resources; no inventory or game scan.
    from recovery_A_20261116 import Image, instruction_row
    samples = []
    for name, expected, spans in (
        ('saveload.fes', '79e05d14259c5d5160117f59aff9b4816d2a307f09aa42554dd80fc2f9eb8b23',
         ((370, 404), (459, 480))),
        ('conf.fes', '3671c94bf9185bd00aad3ee67a078d9824cf05594e2c7fbbece775c457245df1',
         ((37, 77),)),
    ):
        path = ROOT / 'cs2_full_v401/system/fes' / name
        raw = path.read_bytes()
        if digest(raw) != expected or raw[:4] != b'FES\0':
            raise ValueError('fixed confirmation FES changed: ' + name)
        body = zlib.decompress(raw[16:])
        lines = body.decode('cp932').splitlines()
        samples.append(dict(path=path.relative_to(ROOT).as_posix(), sha256=expected,
            body_sha256=digest(body), calls=[dict(line=i, text=lines[i-1])
            for a, b in spans for i in range(a, b+1)]))
    image = Image(ROOT / 'Yukikoi Melt/YukikoiMelt.exe',
        '63de0e14cd8ce11ad2d68e7ed6076641ff5e5b73d822603cc52764009d812976')
    tables = []
    for name, at, callback in (('basebtnpri', 0x5112e0, 0x2db320),
                               ('resetactive', 0x511424, 0x2dde90)):
        raw = image.get(at, 36)
        if raw[:32].split(b'\0')[0] != name.encode() or image.u32(at+32) != image.base+callback:
            raise ValueError('fixed confirmation callback changed: ' + name)
        tables.append(dict(name=name, rva=hex(at), bytes=raw.hex(), callback=hex(callback)))
    ranges = []
    for label, start, end in (
        ('base_priority_notice14_return_minus3', 0x2db320, 0x2db3ce),
        ('resetactive_notice11_return0', 0x2dde90, 0x2dded3),
        ('notice_append', 0x2d9140, 0x2d91ae),
        ('notice_pop', 0x2b54e0, 0x2b5557),
        ('body_notice_dispatch', 0x17c4cb, 0x17c4f3),
        ('reset_mouse_unfocus_then_input146', 0x17ca08, 0x17cae9),
        ('base_low_button_input129', 0x17cb5e, 0x17cbc1),
        ('mouse_named_before_unfocus', 0x17ffc0, 0x180069),
        ('unfocus_default_and_original_object_header', 0x2b4b04, 0x2b4c9c),
    ):
        raw = image.get(start, end-start)
        ranges.append(dict(label=label, start_rva=hex(start), end_rva=hex(end), sha256=digest(raw),
            instructions=[instruction_row(i, image.base) for i in image.md.disasm(raw, image.base+start)]))
    sources = ['simulator/crates/desktop/src/' + name for name in
        ('fes_controls_flow.rs', 'fes_main_flow.rs', 'fes_controls.rs')]
    output = ROOT / 'archive/2026-11/20261117_fes_confirmation_controls_E.json'
    output.write_text(json.dumps(dict(mode='pinned-resource-and-bounded-static-PE-only', samples=samples,
        pe=dict(sha256=image.digest, tables=tables, switch_table_rva='0x17d654',
            resetactive_case=hex(image.u32(0x17d654+11*4)-image.base),
            priority_case=hex(image.u32(0x17d654+14*4)-image.base), ranges=ranges),
        sources={name:digest((ROOT/name).read_bytes()) for name in sources},
        producer='original confirmation/cancel FES -> existing driver deferred/notice queues',
        consumer='original low BUTTON -> input129 priority sort; real pointer -> original UNFOCUS -> input146 -> next input focus update',
        pending_main_hooks=['append FesDriverEffect::ControlFlow(main_flow::controls_flow::Request)',
            'FesTaskPlatform::auxiliary_button(owner) reads existing low_buttons'],
        compiler_invoked=False, type_check_run=False, tests_run=False, engine_executed=False),
        ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
    print(json.dumps(dict(report=output.relative_to(ROOT).as_posix(), samples=len(samples), pe_ranges=len(ranges))))


def save_panel_data_evidence():
    # This mode only reads the already pinned producer and contract sources.
    # It does not execute panel/CSS implementation, VM, PE, or grammar tools.
    path = ROOT / 'cs2_full_v401/system/fes/saveload.fes'
    raw = path.read_bytes()
    expected = '79e05d14259c5d5160117f59aff9b4816d2a307f09aa42554dd80fc2f9eb8b23'
    if digest(raw) != expected or raw[:4] != b'FES\0':
        raise ValueError('fixed saveload producer changed')
    body = zlib.decompress(raw[16:])
    if digest(body) != '28ab5f19d49c398b99870c8708d4ccb63de241d1461bfef491a9fdf01959de19':
        raise ValueError('fixed saveload body changed')
    lines = body.decode('cp932').splitlines()
    spans = [(39, 42), (99, 107), (161, 196), (216, 228), (236, 264), (405, 408)]
    sources = ['simulator/crates/desktop/src/' + name for name in
        ('fes_save_panel.rs', 'fes_save_panel_time.rs', 'fes_files.rs')]
    references = ['resources/reference/KatzeSystem2/' + name for name in
        ('docs/fes.md', 'docs/runtime-probes.md', 'src/runtime/fes/scene/panel.rs',
         'src/fes/save_panel.rs', 'src/fes/save_panel/content.rs')]
    output = ROOT / 'archive/2026-11/20261117_fes_save_panel_data_E.json'
    output.write_text(json.dumps(dict(date='2026-11-17', mode='fixed-producer-source-only',
        sample=dict(path=path.relative_to(ROOT).as_posix(), sha256=expected,
            body_sha256=digest(body), calls=[dict(line=i, text=lines[i-1])
            for a, b in spans for i in range(a, b+1)]),
        sources={name: digest((ROOT/name).read_bytes()) for name in sources},
        contracts={name: digest((ROOT/name).read_bytes()) for name in references},
        consumer='existing private save_root -> fes_files::read_css -> CssFile::try_record '
            '-> the real panel-owned 1024-DWORD region -> existing shared numeric variables',
        integration_status='receiver delivered; dedicated panel construction and host dispatch not hooked',
        remaining=['main module/real panel binding/command/owner-release hook',
            'ptype/setnum/dispid and cell-source application',
            'shared IMAGE compositor, real CSS preview/text and object digit sources',
            'panel state transaction support; keep existing whole-graph guards'],
        compiler_invoked=False, type_check_run=False, grammar_tool_invoked=False,
        tests_run=False, implementation_executed=False, engine_executed=False),
        ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
    print(output.relative_to(ROOT).as_posix())


def save_panel_consumer_evidence():
    # A bounded source/resource snapshot, not an invocation of the receiver.
    path = ROOT / 'cs2_full_v401/system/fes/saveload.fes'
    raw = path.read_bytes()
    expected = '79e05d14259c5d5160117f59aff9b4816d2a307f09aa42554dd80fc2f9eb8b23'
    if digest(raw) != expected or raw[:4] != b'FES\0':
        raise ValueError('fixed saveload producer changed')
    body = zlib.decompress(raw[16:])
    if digest(body) != '28ab5f19d49c398b99870c8708d4ccb63de241d1461bfef491a9fdf01959de19':
        raise ValueError('fixed saveload body changed')
    lines = body.decode('cp932').splitlines()
    spans = [(39, 46), (99, 107), (162, 196), (216, 228), (237, 264), (405, 408)]
    images = []
    for name in ('sys_sl_panel.hg3', 'sys_sl_font_gdate_n.hg3', 'sys_sl_font_gdate_w.hg3',
                 'sys_sl_font_gdate_ex.hg3', 'sys_sl_font_date.hg3', 'sys_userfont.hg3'):
        image_path = ROOT / 'cs2_full_v401/system/image' / name
        encoded = image_path.read_bytes()
        images.append(dict(path=image_path.relative_to(ROOT).as_posix(), sha256=digest(encoded),
                           length=len(encoded), header=encoded[:32].hex()))
    sources = ['simulator/crates/desktop/src/' + name for name in
        ('fes_save_panel.rs', 'fes_save_panel_bank.rs', 'fes_save_panel_cell.rs',
         'fes_save_panel_pixels.rs', 'fes_save_panel_state.rs', 'fes_save_panel_host.rs',
         'fes_services.rs', 'fes_task_platform.rs', 'fes_motion.rs')]
    integrations = ['simulator/crates/desktop/src/' + name for name in
        ('fes_task_state.rs', 'fes_bank_state.rs', 'fes_platform_state.rs', 'fes_cpu_state.rs')]
    existing = ['archive/2026-11/' + name for name in
        ('20261117_fes_save_panel_data_E.json', '20261117_fes_save_panel_callbacks_E.json',
         '20261117_fes_save_panel_userfont_E.json', '20261117_fes_save_panel_userfont_tail_E.json')]
    output = ROOT / 'archive/2026-11/20261118_fes_save_panel_consumer_E.json'
    output.write_text(json.dumps(dict(date='2026-11-18', mode='fixed-resource-and-source-read-only',
        sample=dict(path=path.relative_to(ROOT).as_posix(), sha256=expected, body_sha256=digest(body),
                    calls=[dict(line=i, text=lines[i-1]) for a, b in spans for i in range(a, b+1)]),
        resources=images, sources={name: digest((ROOT/name).read_bytes()) for name in sources},
        C_owned_integration_read_only={name: digest((ROOT/name).read_bytes()) for name in integrations},
        prior_evidence={name: digest((ROOT/name).read_bytes()) for name in existing},
        fixed_PE=dict(path='Yukikoi Melt/YukikoiMelt.exe',
                      sha256='63de0e14cd8ce11ad2d68e7ed6076641ff5e5b73d822603cc52764009d812976',
                      provenance='prior pinned PE archives above; not re-executed or rescanned',
                      callbacks_VA={'ptype': '0x6cba20', 'setsave': '0x6cc3e0', 'userfont': '0x6ce350'}),
        canonical_store='tasks.fes.platform.save_panels',
        consumer='real SAVEPANEL banks -> original IMAGE motion -> setsave CSS/flags/date -> '
                 'NORM/SIMAGE/SSTR/NUM_O2 composition -> runtime.media.image -> existing type15 list',
        release='existing postorder plan removes list layers, then exact bank source/media; owner/reload teardown drops owned stages',
        checkpoint='CPU7 dedicated component -> prepared platform.save_panels -> original driver/pending/CPU/list validation -> one FesTaskStore commit',
        remaining=['SAVEPANEL inherited SIZE2/other unsupported IMAGE properties',
                   'cropped userfont ABI and duplicate mapping precedence not fully confirmed',
                   'CSS preview BMP/JPEG/PNG decoding not confirmed',
                   'text glyph sizes above 120 explicitly rejected',
                   'cross-version and runtime acceptance pending; no total FES completion claim'],
        compiler_invoked=False, type_check_run=False, grammar_tool_invoked=False, tests_run=False,
        implementation_executed=False, engine_executed=False), ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
    print(output.relative_to(ROOT).as_posix())


def save_panel_userfont_order_evidence():
    # Follow only the already known registration accessor and its renderer.
    from recovery_A_20261116 import Image, instruction_row
    image = Image(ROOT / 'Yukikoi Melt/YukikoiMelt.exe',
        '63de0e14cd8ce11ad2d68e7ed6076641ff5e5b73d822603cc52764009d812976')
    sample = ROOT / 'cs2_full_v401/system/fes/saveload.fes'
    raw = sample.read_bytes()
    if digest(raw) != '79e05d14259c5d5160117f59aff9b4816d2a307f09aa42554dd80fc2f9eb8b23':
        raise ValueError('fixed saveload producer changed')
    lines = zlib.decompress(raw[16:]).decode('cp932').splitlines()
    ranges = []
    selected = {0x6ba79f, 0x6ba7a7, 0x6ba7da,
                0x6ba824, 0x6ba82b, 0x6ba83c, 0x6ba843, 0x6ba846,
                0x5b629f, 0x5b62a8, 0x5b62b1, 0x5b63ee, 0x5b63f4,
                0x5b63ff, 0x5b6401, 0x5b640a, 0x5b6411,
                0x5e7b5f, 0x5e7b67, 0x5e7b6a, 0x5e7b6c, 0x5e7b77,
                0x5e7b83, 0x5e7b86, 0x5e7b87, 0x5e7b8f,
                0x5e7c20, 0x5e7c2b, 0x5e7c3c, 0x5e7c4e}
    for label, start, end in (
        ('append_88_byte_mapping', 0x6ba750, 0x6ba800),
        ('index_accessor_from_vector_begin', 0x6ba800, 0x6ba897),
        ('renderer_registers_index_zero_then_increments', 0x5b6292, 0x5b6417),
        ('character_lookup_erase_then_insert', 0x5e7b10, 0x5e7c6b),
    ):
        encoded = image.get(start-image.base, end-start)
        ranges.append(dict(label=label, start_va=hex(start), end_va=hex(end), sha256=digest(encoded),
            raw=encoded.hex(), selected_instructions=[instruction_row(i, image.base)
                for i in image.md.disasm(encoded, start) if i.address in selected]))
    source = 'simulator/crates/desktop/src/fes_save_panel_pixels.rs'
    glyph = 'cs2_full_v401/system/image/sys_userfont.hg3'
    output = ROOT / 'archive/2026-11/20261118_fes_save_panel_userfont_order_E.json'
    output.write_text(json.dumps(dict(mode='fixed-producer-and-bounded-static-PE-read-only',
        pe=dict(path=image.path.relative_to(ROOT).as_posix(), sha256=image.digest, ranges=ranges),
        producer=dict(path=sample.relative_to(ROOT).as_posix(), sha256=digest(raw),
            calls=[dict(line=i, text=lines[i-1]) for i in range(188, 197)]),
        resources={glyph: digest((ROOT/glyph).read_bytes())},
        sources={source: digest((ROOT/source).read_bytes())},
        conclusion='renderer consumes the appended vector in ascending index order; '
                   'glyph registration erases the previous character, so the final duplicate wins; '
                   'V401 0x8744 uses frame9, not frame5',
        consumer='existing panel text pixel consumer searches retained mappings in reverse; '
                 'same HG/glyph decoder, resource paths, vector codec and release owner',
        compiler_invoked=False, type_check_run=False, tests_run=False,
        implementation_executed=False, original_code_executed=False),
        ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
    print(output.relative_to(ROOT).as_posix())


def syntax():
    rows = []
    for name in SOURCES:
        raw = (ROOT / name).read_bytes()
        result = subprocess.run([str(Path.home() / '.cargo/bin/rustfmt.exe'),
            '--edition', '2024', '--emit', 'stdout', '--config', 'skip_children=true'],
            input=raw, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, check=False)
        rows.append(dict(source=name, sha256=digest(raw), exit_code=result.returncode,
                         diagnostics=result.stderr.decode('utf-8', 'replace')))
    ast.parse(Path(__file__).read_text(encoding='utf-8'))
    output = ROOT / 'archive/2026-11/20261117_fes_main_flow_E_source_syntax.json'
    output.write_text(json.dumps(dict(status='grammar-only-not-types-linking-or-runtime',
        compiler_invoked=False, tests_run=False, engine_vm_game_apk_executed=False,
        sources=rows), indent=2) + '\n', encoding='utf-8')
    failures = sum(row['exit_code'] != 0 for row in rows)
    print(json.dumps(dict(report=output.relative_to(ROOT).as_posix(), failures=failures)))
    if failures:
        raise SystemExit(1)


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--syntax', action='store_true')
    parser.add_argument('--log-projection', action='store_true')
    parser.add_argument('--log-consumption', action='store_true')
    parser.add_argument('--log-consumption-syntax', action='store_true')
    parser.add_argument('--select-consumption', action='store_true')
    parser.add_argument('--select-consumption-syntax', action='store_true')
    parser.add_argument('--log-event-context', action='store_true')
    parser.add_argument('--log-event-context-syntax', action='store_true')
    parser.add_argument('--confirmation-controls', action='store_true')
    parser.add_argument('--confirmation-controls-syntax', action='store_true')
    parser.add_argument('--save-panel-data', action='store_true')
    parser.add_argument('--save-panel-consumer', action='store_true')
    parser.add_argument('--save-panel-userfont-order', action='store_true')
    args = parser.parse_args()
    if args.save_panel_userfont_order:
        save_panel_userfont_order_evidence()
    elif args.save_panel_consumer:
        save_panel_consumer_evidence()
    elif args.save_panel_data:
        save_panel_data_evidence()
    elif args.confirmation_controls_syntax:
        from settings_menu_E_contracts import syntax as source_syntax
        ast.parse(Path(__file__).read_text(encoding='utf-8'))
        source_syntax(['simulator/crates/desktop/src/' + name for name in
            ('fes_controls_flow.rs', 'fes_main_flow.rs', 'fes_controls.rs')],
            '20261117_fes_confirmation_controls_E_source_syntax.json')
    elif args.confirmation_controls:
        confirmation_control_evidence()
    elif args.log_event_context_syntax:
        from settings_menu_E_contracts import syntax as source_syntax
        ast.parse(Path(__file__).read_text(encoding='utf-8'))
        source_syntax(['simulator/crates/desktop/src/fes_log.rs'],
            '20261117_fes_log_event_context_E_source_syntax.json')
    elif args.log_event_context:
        log_event_context_evidence()
    elif args.select_consumption_syntax:
        from settings_menu_E_contracts import syntax as source_syntax
        ast.parse(Path(__file__).read_text(encoding='utf-8'))
        source_syntax(['simulator/crates/desktop/src/fes_text.rs'],
            '20261117_fes_select_consumption_E_source_syntax.json')
    elif args.select_consumption:
        select_consumption_evidence()
    elif args.log_consumption_syntax:
        log_consumption_syntax()
    elif args.log_consumption:
        log_consumption_evidence()
    elif args.syntax:
        syntax()
    elif args.log_projection:
        log_projection_evidence()
    else:
        evidence()

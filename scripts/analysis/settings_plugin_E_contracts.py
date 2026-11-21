"""Read pinned cs2confx PE data/instructions; never load the original DLL."""
from __future__ import annotations
import ast
import argparse
import hashlib
import json
import struct
import zlib
from pathlib import Path
from recovery_A_20261116 import Image, ROOT, instruction_row
from settings_menu_E_contracts import dialog


def position_evidence():
    from startup_inventory import entries_at
    from audit_runtime_evidence import blowfish_le_decrypt, operand_count
    plugin = Image(ROOT / 'Yukikoi Melt/cs2confx.dll',
        'baefc568b127d0dda738758b32b9a70a40a7241cb26996e8c58c34be61d3eb19')
    templates = json.loads((ROOT / 'reports/settings_plugin_E_contracts_20261117.json').read_text(encoding='utf-8'))
    regions = []
    for label, rva, size in [('130-message-position-write', 0x1c0a9, 0x4d),
            ('130-Apply', 0x1c133, 0x10), ('130-reset-from-default', 0x1c193, 0x4f),
            ('130-radio-selection', 0x1c282, 0x36)]:
        regions.append(dict(label=label, rva=hex(rva),
            sha256=hashlib.sha256(plugin.get(rva, size)).hexdigest(),
            instructions=[instruction_row(i, plugin.base) for i in plugin.insns(rva, size)]))
    path = ROOT / 'cs2_full_v401/system/kcs.int'
    entries, _, cipher = entries_at(path, 0)
    entry = next(e for e in entries if e['name'].lower() == b'sscript.kcs')
    with path.open('rb') as handle:
        handle.seek(entry['offset'])
        data = handle.read(entry['size'])
    if cipher: data = blowfish_le_decrypt(cipher, data)
    digest = hashlib.sha256(data).hexdigest()
    if digest != '17de548390b8c2b15a4f413418bbf1b408d6dee6ab830879f09cf305f5c128fd':
        raise ValueError('pinned sscript.kcs changed')
    code_length = struct.unpack_from('<I', data, 16)[0]
    body = zlib.decompress(data[52:])
    code, init = body[:code_length], body[code_length:]
    rows = []
    # Only decode fixed regions of this already-inventoried program, no VM.
    for start, end in [(13382, 13463), (17959, 17994), (20087, 21037),
                       (791487, 791510)]:
        cursor = start
        while cursor < end:
            opcode = code[cursor]
            count = operand_count(opcode)
            if count is None: raise ValueError('unknown static opcode')
            operands = [dict(zip(('flags', 'payload'), struct.unpack_from('<BI', code, cursor + 1 + i * 5)))
                for i in range(count)]
            rows.append(dict(offset=cursor, opcode=hex(opcode), operands=operands))
            cursor += 1 + count * 5
    strings = []
    for index in range(struct.unpack_from('<I', data, 24)[0]):
        source, target = struct.unpack_from('<II', init, index * 8)
        if target in [379665, 379676, 379678, 379689]:
            strings.append(dict(target=target, init_offset=source,
                text=init[source:init.index(b'\0', source)].decode('ascii')))
    sample = ROOT / 'cs2_full_v401/system/fes/meswnd_A.txt'
    sample_bytes = sample.read_bytes()
    lines = sample_bytes.decode('cp932').splitlines()
    output = ROOT / 'archive/2026-11/20261117_settings_plugin_position_E.json'
    report = dict(mode='bounded-fixed-PE-and-one-existing-KCS-static-read-only',
        plugin_sha256=plugin.digest, page130=next(p for p in templates['dialogs'] if p['id'] == 130),
        regions=regions, kcs=dict(resource='cs2_full_v401/system/kcs.int::sscript.kcs',
            sha256=digest, native741_offset=20145, rows=rows, strings=strings),
        fes=dict(path=sample.relative_to(ROOT).as_posix(), sha256=hashlib.sha256(sample_bytes).hexdigest(),
            lines=[dict(line=index + 1, text=lines[index]) for index in range(794, 819)]),
        delivered=dict(commands=[1137, 1138, 1139], scratch='root+fc00', shared_global='global+fbf8',
            values=[0, 1, 2], startup_default='SCREEN/mw_pos', private_setting='FULL_SCREEN/mw_pos',
            owner='existing FesSettingsStore.fullscreen_position; no added store fields',
            consumer='741 (actual fullscreen gate) ->645 parameters0/1/2 ->630 MES_SHIFT ->PLANE pos',
            host='existing Windows/Android Choice, scratch, Apply/Reset/Cancel'),
        remaining=['130 full_size/full_pos/wall controls have no closed host consumer in this batch',
            'existing739/740 dimensions and actual fullscreen receipt determine physical edge placement; no invented caps/offsets',
            'other plugin pages remain partial; this adds no complete457/628 claim'],
        compiler_invoked=False, type_check_run=False, tests_run=False, original_code_executed=False)
    output.write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
    print(json.dumps(dict(report=output.relative_to(ROOT).as_posix(), commands=[1137,1138,1139], native741=20145)))


def position_syntax():
    from settings_menu_E_contracts import syntax
    ast.parse(Path(__file__).read_text(encoding='utf-8'))
    syntax([
        'simulator/crates/desktop/src/settings_menu_plugin_position.rs',
        'simulator/crates/desktop/src/settings_menu_plugin_sound.rs',
        'simulator/crates/desktop/src/settings_menu_general.rs',
        'simulator/crates/desktop/src/settings_menu_native.rs',
        'simulator/crates/desktop/src/fes_settings.rs',
    ], '20261117_settings_plugin_position_E_source_syntax.json')


def image_evidence():
    plugin = Image(ROOT / 'Yukikoi Melt/cs2confx.dll',
        'baefc568b127d0dda738758b32b9a70a40a7241cb26996e8c58c34be61d3eb19')
    engine = Image(ROOT / 'Yukikoi Melt/YukikoiMelt.exe',
        '63de0e14cd8ce11ad2d68e7ed6076641ff5e5b73d822603cc52764009d812976')
    templates = json.loads((ROOT / 'reports/settings_plugin_E_contracts_20261117.json').read_text(encoding='utf-8'))
    regions = []
    for image, label, rva, size in [
        (plugin, 'page-registration', 0x1a2d5, 0xb4),
        (plugin, 'simple-detailed-page-selection', 0x1eae0, 0xc5),
        (plugin, 'startup-CONFIG_B-view3d-default0', 0x3ffb4, 0x4c),
        (plugin, '128-apply-backbuffer', 0x1c515, 0x30),
        (plugin, '128-reset-backbuffer1', 0x1c5a4, 0x6c),
        (plugin, '131-apply-backbuffer', 0x1d0e2, 0x30),
        (plugin, '131-reset-backbuffer1', 0x1d192, 0x6e),
        (plugin, 'configuration-key-serialization', 0x439ea, 0xc6),
        (plugin, '129-size-options-and-scratch-index', 0x1ca40, 0x447),
        (plugin, 'A7D0-two-dimension-values', 0x1a7d0, 0x16),
        (plugin, '108-resolution-comparison', 0x1b320, 0x9f),
        (plugin, '108-reset-resolution-comparison', 0x1ba23, 0x61),
        (plugin, '108-refresh-resolution-comparison', 0x1bcd0, 0x52),
        (engine, 'host-passes-window-dimensions-and-real-Apply-callback', 0x83a20, 0x240),
        (engine, 'auxiliary-backbuffer-gate-and-distinct-limit', 0x1c5500, 0xff),
    ]:
        regions.append(dict(source=image.path.relative_to(ROOT).as_posix(), label=label, rva=hex(rva),
            sha256=hashlib.sha256(image.get(rva, size)).hexdigest(),
            instructions=[instruction_row(i, image.base) for i in image.insns(rva, size)]))
    literals = [dict(rva=hex(rva), text=plugin.get(rva, 64).split(b'\0', 1)[0].decode('ascii'))
        for rva in [0x74708, 0x74710, 0x75b2c, 0x75b38, 0x75b7c, 0x75b88]]
    output = ROOT / 'archive/2026-11/20261117_settings_plugin_image_E.json'
    report = dict(mode='bounded-fixed-PE-data-and-instruction-read-only',
        plugin_sha256=plugin.digest, engine_sha256=engine.digest,
        pages=[page for page in templates['dialogs'] if page['id'] in [108, 128, 129, 131]],
        regions=regions, literals=literals,
        delivered=dict(pages=[128, 131], command=1120, root='0xfbcc', shared_global='0xfbc4',
            path='D3D/backbuffer', reset=1,
            selection='CONFIG_B/view3d: nonzero128, zero131; default0, separate from CONFIG/type',
            consumer='live gate before native858 or retained-frame auxiliary acquisition; preserve existing owners/surfaces'),
        corrections=[
            'A7D0 stores two width/height values at14220/14224.108 compares them with resolution pairs. They are not callbacks.',
            '1291128 stores UI scratch root+fbe0 combo index0/1/2.43a8c instead reads serializer object+fff0; no cross-base identity inferred.',
            '5c55e5 compares a different global+fbe0 against pixel extents. Do not feed the combo index as this limit.',
        ],
        remaining=['129 size/compression lack real image/texture consumers; not exposed',
            '128/131 other renderer options not exposed without matching consumers',
            '108 and130 host display/layout options remain partial; no invented text page',
            'existing canvas_native/frame_effects D3D/tex_size-as-limit reads need a separate real cap producer; no cross-owner edit'],
        compiler_invoked=False, tests_run=False, original_code_executed=False)
    output.write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
    print(json.dumps(dict(report=output.relative_to(ROOT).as_posix(), fields=['D3D/backbuffer'],
        page_selection='CONFIG_B/view3d', extra_callbacks_added=False)))


def image_syntax():
    from settings_menu_E_contracts import syntax
    ast.parse(Path(__file__).read_text(encoding='utf-8'))
    syntax([
        'simulator/crates/desktop/src/settings_menu_plugin_image.rs',
        'simulator/crates/desktop/src/settings_menu_plugin_sound.rs',
        'simulator/crates/desktop/src/settings_menu_general.rs',
        'simulator/crates/desktop/src/settings_menu_native.rs',
        'simulator/crates/desktop/src/configuration.rs',
    ], '20261117_settings_plugin_image_E_source_syntax.json')


def sound_evidence():
    plugin = Image(ROOT / 'Yukikoi Melt/cs2confx.dll',
        'baefc568b127d0dda738758b32b9a70a40a7241cb26996e8c58c34be61d3eb19')
    engine = Image(ROOT / 'Yukikoi Melt/YukikoiMelt.exe',
        '63de0e14cd8ce11ad2d68e7ed6076641ff5e5b73d822603cc52764009d812976')
    templates = json.loads((ROOT / 'reports/settings_plugin_E_contracts_20261117.json').read_text(encoding='utf-8'))
    regions = []
    for image, rva, size in [(plugin, 0x1a2d5, 0xb4), (plugin, 0x1d620, 0x3b9),
                             (engine, 0x92a20, 0x208), (engine, 0x123090, 0x85)]:
        regions.append(dict(source=image.path.relative_to(ROOT).as_posix(), rva=hex(rva),
            sha256=hashlib.sha256(image.get(rva, size)).hexdigest(),
            instructions=[instruction_row(i, image.base) for i in image.insns(rva, size)]))
    dispatch = []
    for command in [1027, 1028, 1102, 1122, 1123]:
        slot = plugin.get(0x1d9fc + command - 1027, 1)[0]
        dispatch.append(dict(command=command, slot=slot, va=hex(plugin.u32(0x1d9f0 + slot * 4))))
    page = next(page for page in templates['dialogs'] if page['id'] == 109)
    report = dict(mode='fixed-PE-data-and-instruction-read-only',
        plugin_sha256=plugin.digest, engine_sha256=engine.digest,
        page109=page, dispatch=dispatch, regions=regions,
        template_inventory=[dict(id=page['id'], title=page['parsed']['title'],
            controls=[control['id'] for control in page['parsed']['controls']]) for page in templates['dialogs']],
        fields=[dict(command=1027, root='0xf57c', global_offset='0xf574', path='SOUND/sticky',
            consumer='492bc2..492c23 device creation flag0x20; next device creation'),
            dict(command=1123, root='0xf580', global_offset='0xf578', path='SOUND/stereo',
            consumer='523090/5230c0 live pan gates; Apply uses existing task options/command19')],
        excluded=['no message speed or volume controls exist in this fixed plugin template set',
            '1028 Direct3DSound: no corresponding portable backend switch',
            '1114 cache and1122 dualcore: no inspected matching live consumer'],
        compiler_invoked=False, tests_run=False, original_code_executed=False)
    output = ROOT / 'archive/2026-11/20261117_settings_plugin_sound_E.json'
    output.write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
    print(json.dumps(dict(report=output.relative_to(ROOT).as_posix(), dispatch=dispatch)))


def sound_syntax():
    from settings_menu_E_contracts import syntax
    ast.parse(Path(__file__).read_text(encoding='utf-8'))
    syntax([
        'simulator/crates/desktop/src/settings_menu_plugin_sound.rs',
        'simulator/crates/desktop/src/settings_menu_general.rs',
        'simulator/crates/desktop/src/settings_menu_native.rs',
        'simulator/crates/desktop/src/configuration.rs',
        'simulator/crates/desktop/src/portable_host.rs',
        'simulator/crates/desktop/src/sound_control.rs',
        'simulator/crates/android/src/lib.rs',
        'simulator/android/app/src/main/java/org/catsystem2/player/PlayerActivity.java',
    ], '20261117_settings_plugin_sound_E_source_syntax.json')


def general_syntax():
    from settings_menu_E_contracts import syntax
    ast.parse(Path(__file__).read_text(encoding='utf-8'))
    syntax([
        'simulator/crates/desktop/src/settings_menu_general.rs',
        'simulator/crates/desktop/src/settings_menu_general_windows.rs',
        'simulator/crates/desktop/src/settings_menu_native.rs',
        'simulator/crates/desktop/src/settings_menu_windows.rs',
        'simulator/crates/desktop/src/configuration.rs',
        'simulator/crates/desktop/src/window_control.rs',
        'simulator/crates/desktop/src/portable_host.rs',
        'simulator/crates/desktop/src/lib.rs',
        'simulator/crates/android/src/lib.rs',
        'simulator/android/app/src/main/cpp/bridge.c',
        'simulator/android/app/src/main/java/org/catsystem2/player/PlayerActivity.java',
    ], '20261117_settings_plugin_general_E_source_syntax.json')


def general_evidence():
    plugin = Image(ROOT / 'Yukikoi Melt/cs2confx.dll',
        'baefc568b127d0dda738758b32b9a70a40a7241cb26996e8c58c34be61d3eb19')
    engine = Image(ROOT / 'Yukikoi Melt/YukikoiMelt.exe',
        '63de0e14cd8ce11ad2d68e7ed6076641ff5e5b73d822603cc52764009d812976')
    regions = []
    field_refs = {}
    for offset in [0xfbb4, 0xf580, 0xf584, 0xf594]:
        hits = []
        for section in engine.sections:
            if not section['executable']: continue
            data = section['data']
            needle = offset.to_bytes(4, 'little')
            at = data.find(needle)
            while at >= 0:
                rva = section['rva'] + at
                hits.append(hex(rva))
                at = data.find(needle, at + 1)
        field_refs[hex(offset)] = hits
    for image, start, size in [(plugin, 0x31c00, 0x20), (plugin, 0x1df70, 0x70),
                               (engine, 0x83630, 0x3f0), (engine, 0x83a20, 0x240),
                               (plugin, 0x43620, 0x210), (engine, 0x1335c0, 0x1e0),
                               (engine, 0x26c2c0, 0x70),
                               (engine, 0x8d690, 0xb0), (engine, 0x92a20, 0x80),
                               (engine, 0x8ca10, 0x90), (engine, 0x93326, 0xf9),
                               (engine, 0x902a0, 0x110), (engine, 0x11b070, 0x1a0)]:
        instructions = [instruction_row(i, image.base) for i in image.insns(start, size)]
        regions.append(dict(source=image.path.relative_to(ROOT).as_posix(),
            rva=hex(start), sha256=hashlib.sha256(image.get(start, size)).hexdigest(),
            instructions=instructions))
    output = ROOT / 'archive/2026-11/20261117_settings_plugin_general_callbacks_E.json'
    output.write_text(json.dumps(dict(mode='bounded-fixed-PE-static-read-only',
        images=[dict(source=image.path.relative_to(ROOT).as_posix(), sha256=image.digest,
            imports=list(image.imports.values())) for image in [plugin, engine]],
        field_refs=field_refs, regions=regions, compiler_invoked=False, tests_run=False,
        original_code_executed=False), indent=2) + '\n', encoding='utf-8')
    print(json.dumps(field_refs))
    for region in [regions[10]]:
        print(region['rva'])
        for row in region['instructions']:
            if row['mnemonic'] != 'int3': print(hex(row['rva']), row['mnemonic'], row['operands'])
    for rva in [0x5dc638, 0x5f8b94, 0x5f8b98, 0x5f8bcc, 0x4e3790, 0x4e3838, 0x4e38b4]:
        raw = engine.get(rva, 256)
        end = next((at for at in range(0, len(raw)-1, 2) if raw[at:at+2] == b'\0\0'), len(raw))
        print(hex(rva), raw[:end].decode('utf-16le', 'backslashreplace').encode('ascii', 'backslashreplace').decode('ascii'))


def main():
    image = Image(ROOT / 'Yukikoi Melt/cs2confx.dll',
        'baefc568b127d0dda738758b32b9a70a40a7241cb26996e8c58c34be61d3eb19')
    resources = []
    for kind in image.pe.DIRECTORY_ENTRY_RESOURCE.entries:
        if kind.id != 5: continue
        for name in kind.directory.entries:
            for language in name.directory.entries:
                entry = language.data.struct
                raw = image.pe.get_data(entry.OffsetToData, entry.Size)
                resources.append(dict(id=name.id, language=language.id,
                    sha256=hashlib.sha256(raw).hexdigest(), parsed=dialog(raw)))
    exports = [dict(ordinal=entry.ordinal,
        name=entry.name.decode('ascii') if entry.name else None,
        va=image.base + entry.address)
        for entry in image.pe.DIRECTORY_ENTRY_EXPORT.symbols]
    regions = [(0x1001A240, 0x1A0), (0x1001A3E0, 0x90),
               (0x1001A470, 0x10), (0x1001A480, 0x10)]
    report = dict(schema='cs2-settings-plugin-E-contracts/v1',
        pe=dict(path=image.path.relative_to(ROOT).as_posix(), sha256=image.digest,
            image_base=image.base, machine=image.pe.FILE_HEADER.Machine),
        exports=exports, dialogs=resources,
        regions=[dict(va=va, size=size, instructions=[instruction_row(i, image.base)
            for i in image.insns(va - image.base, size)]) for va, size in regions],
        native=628, menu_command=40057, implementation_complete=False,
        original_dll_loaded=False, original_code_executed=False,
        compiler_invoked=False, tests_run=False,
        existing_host_chain='49153a -> 483a20 save1/load-module/ordinal1..4 -> virtual object methods -> callback483630',
        remaining=['inspect each original plugin page and its field paths',
            'restore actual shared configuration consumers and callback side effects',
            'preserve save-before-open and private output boundaries; never execute the original DLL'])
    script = Path(__file__)
    ast.parse(script.read_text(encoding='utf-8'))
    report['script_sha256'] = hashlib.sha256(script.read_bytes()).hexdigest()
    output = ROOT / 'reports/settings_plugin_E_contracts_20261117.json'
    output.write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
    print(output.relative_to(ROOT).as_posix())


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--general', action='store_true')
    parser.add_argument('--general-syntax', action='store_true')
    parser.add_argument('--sound', action='store_true')
    parser.add_argument('--sound-syntax', action='store_true')
    parser.add_argument('--image', action='store_true')
    parser.add_argument('--image-syntax', action='store_true')
    parser.add_argument('--position', action='store_true')
    parser.add_argument('--position-syntax', action='store_true')
    args = parser.parse_args()
    if args.position_syntax: position_syntax()
    elif args.position: position_evidence()
    elif args.image_syntax: image_syntax()
    elif args.image: image_evidence()
    elif args.sound_syntax: sound_syntax()
    elif args.sound: sound_evidence()
    elif args.general_syntax: general_syntax()
    elif args.general: general_evidence()
    else: main()

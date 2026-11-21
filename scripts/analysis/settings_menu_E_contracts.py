"""Read pinned PE resources/instructions only; never load or execute the engine."""
from __future__ import annotations
import argparse
import ast
import hashlib
import json
import re
import struct
import subprocess
import sys
from recovery_A_20261116 import Image, ROOT, instruction_row

SOURCES = [
    'simulator/crates/desktop/src/settings_menu_native.rs',
    'simulator/crates/desktop/src/settings_menu_windows.rs',
    'simulator/crates/desktop/src/ui_resources.rs',
    'simulator/crates/desktop/src/window_control.rs',
    'simulator/crates/desktop/src/system_open_native.rs',
    'simulator/crates/desktop/src/input_history_native.rs',
    'simulator/crates/desktop/src/fes_settings.rs',
    'simulator/crates/desktop/src/message_platform.rs',
    'simulator/crates/desktop/src/message_native.rs',
    'simulator/crates/desktop/src/message_backend.rs',
    'simulator/crates/desktop/src/sound_legacy.rs',
    'simulator/crates/desktop/src/configuration.rs',
    'simulator/crates/desktop/src/language_adapters.rs',
    'simulator/crates/desktop/src/lib.rs',
    'simulator/crates/android/src/lib.rs',
    'simulator/android/app/src/main/cpp/bridge.c',
    'simulator/android/app/src/main/java/org/catsystem2/player/PlayerActivity.java',
]


def syntax(sources=None, output_name='20261117_settings_menu_E_source_syntax.json'):
    from pathlib import Path
    import re
    sys.path.insert(0, str(Path.home() / 'AppData/Local/CS2AnalysisPython'))
    import tree_sitter
    import tree_sitter_c
    import tree_sitter_java
    rows = []
    for name in SOURCES if sources is None else sources:
        raw = (ROOT / name).read_bytes()
        row = dict(source=name, sha256=hashlib.sha256(raw).hexdigest())
        if name.endswith('.rs'):
            result = subprocess.run([str(Path.home() / '.cargo/bin/rustfmt.exe'),
                '--edition', '2024', '--emit', 'stdout', '--config', 'skip_children=true'],
                input=raw, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, check=False)
            row.update(exit_code=result.returncode, diagnostics=result.stderr.decode('utf-8', 'replace'))
        else:
            source = raw.decode('utf-8-sig')
            if name.endswith('.c'):
                source = re.sub(r'^\s*#.*$', '', source, flags=re.MULTILINE)
                source = source.replace('JNIEXPORT', '').replace('JNICALL', '')
                source = re.sub(r'\bJNI\((\w+)\)', r'Java_org_catsystem2_player_PlayerActivity_\1', source)
                language = tree_sitter_c.language()
            else: language = tree_sitter_java.language()
            tree = tree_sitter.Parser(tree_sitter.Language(language)).parse(source.encode('utf-8'))
            row.update(exit_code=int(tree.root_node.has_error), diagnostics='tree-sitter grammar only')
        rows.append(row)
    ast.parse(Path(__file__).read_text(encoding='utf-8'))
    output = ROOT / 'archive/2026-11' / output_name
    output.write_text(json.dumps(dict(status='grammar-only-not-types-linking-or-runtime',
        compiler_invoked=False, tests_run=False, engine_vm_game_apk_executed=False,
        sources=rows), indent=2) + '\n', encoding='utf-8')
    failures = sum(row['exit_code'] != 0 for row in rows)
    print(json.dumps(dict(report=output.relative_to(ROOT).as_posix(), failures=failures)))
    if failures: raise SystemExit(1)


class Reader:
    def __init__(self, data): self.data, self.at = data, 0
    def unpack(self, fmt):
        value = struct.unpack_from('<' + fmt, self.data, self.at)
        self.at += struct.calcsize('<' + fmt)
        return value[0] if len(value) == 1 else value
    def align(self): self.at = (self.at + 3) & ~3
    def string(self):
        units = []
        while (unit := self.unpack('H')) != 0: units.append(unit)
        return b''.join(struct.pack('<H', unit) for unit in units).decode('utf-16le')
    def ordinal(self):
        if struct.unpack_from('<H', self.data, self.at)[0] == 0xffff:
            self.at += 2
            return self.unpack('H')
        return self.string()


def dialog(data):
    r = Reader(data)
    extended = data[:4] == b'\x01\x00\xff\xff'
    if extended:
        version, signature, help_id, exstyle, style, count = r.unpack('HHIIIH')
    else: style, exstyle, count = r.unpack('IIH')
    bounds = r.unpack('hhhh')
    menu, klass, title = r.ordinal(), r.ordinal(), r.string()
    font = None
    if style & 0x40:
        size = r.unpack('H')
        if extended: weight, italic, charset = r.unpack('HBB')
        font = [size, r.string()]
    controls = []
    for _ in range(count):
        r.align()
        if extended:
            help_id, exstyle, style = r.unpack('III')
            bounds_i, identity = r.unpack('hhhh'), r.unpack('I')
        else:
            style, exstyle = r.unpack('II')
            bounds_i, identity = r.unpack('hhhh'), r.unpack('H')
        klass, text = r.ordinal(), r.ordinal()
        extra = r.unpack('H')
        r.at += extra
        controls.append(dict(id=identity, klass=klass, text=text, style=hex(style), bounds=bounds_i))
    return dict(title=title, bounds=bounds, font=font, controls=controls)


def menu(data):
    r = Reader(data)
    version, offset = r.unpack('HH')
    if version != 0: return dict(version=version, raw=data.hex())
    r.at += offset
    def items():
        result = []
        while True:
            flags = r.unpack('H')
            identity = None if flags & 0x10 else r.unpack('H')
            text = r.string()
            result.append(dict(flags=hex(flags), id=identity, text=text,
                               children=items() if flags & 0x10 else []))
            if flags & 0x80: return result
    return items()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--disasm', nargs=2, action='append', default=[])
    parser.add_argument('--syntax', action='store_true')
    args = parser.parse_args()
    if args.syntax:
        syntax()
        return
    image = Image(ROOT / 'Yukikoi Melt/YukikoiMelt.exe',
                  '63de0e14cd8ce11ad2d68e7ed6076641ff5e5b73d822603cc52764009d812976')
    resources = []
    for kind in image.pe.DIRECTORY_ENTRY_RESOURCE.entries:
        if kind.id not in (4, 5): continue
        for name in kind.directory.entries:
            if kind.id == 4 and name.id not in (104, 105): continue
            for lang in name.directory.entries:
                entry = lang.data.struct
                data = image.pe.get_data(entry.OffsetToData, entry.Size)
                resources.append(dict(kind=kind.id, id=name.id, language=lang.id,
                                      sha256=hashlib.sha256(data).hexdigest(),
                                      parsed=(menu if kind.id == 4 else dialog)(data)))
    regions = [(0x483880, 0x19F), (0x483D60, 0x89),
               (0x483DF0, 0x4D0), (0x4842C0, 0x3CC),
               (0x489610, 0x4AC), (0x48BC00, 0x263),
               (0x4893D0, 0x233), (0x493507, 0xBD),
               (0x66C300, 0x27), (0x679C74, 0x1F2),
               (0x67BFCF, 0x3C), (0x490448, 0x8E), (0x66C2E0, 0x20),
               (0x66C360, 0x2A), (0x49156A, 0x3A), (0x491676, 0x1B),
               (0x486CA0, 0x66B), (0x488F50, 0x27A),
               (0x679DC3, 0x38), (0x67C64D, 0x38), (0x67D5A6, 0x1D),
               (0x67CE94, 0x45), (0x67DC44, 0x18),
               (0x4912C0, 0x19B), (0x491901, 0x49),
               (0x487DA0, 0x6EA), (0x4884A0, 0x3F4), (0x48C820, 0x3B0),
               (0x679DFB, 0x65), (0x48B480, 0x709), (0x485D10, 0xE55),
               (0x678AA8, 0x36), (0x679E66, 0x85),
               (0x678BFE, 0x34), (0x67A0C9, 0x3B), (0x4892C0, 0x10A),
               (0x491C60, 0x1A), (0x4846A0, 0x483),
               (0x489B80, 0x241), (0x489DD0, 0x362), (0x48A140, 0x160),
               (0x67B153, 0x1A), (0x67C791, 0x24), (0x53F76F, 0x19), (0x668F10, 0x30),
               (0x66A180, 0xE2), (0x484B40, 0x106E), (0x48A2A0, 0x107F),
               (0x49153A, 0x30), (0x523230, 0x60), (0x523360, 0x35), (0x483A20, 0x239),
               (0x483630, 0x245)]
    regions += [(int(va, 0), int(size, 0)) for va, size in args.disasm]
    report = dict(schema='cs2-settings-menu-E-contracts/v1',
                  pe=dict(path=image.path.relative_to(ROOT).as_posix(), sha256=image.digest),
                  resources=resources,
                  regions=[dict(va=va, size=size, instructions=[instruction_row(i, image.base)
                      for i in image.insns(va - image.base, size)]) for va, size in regions],
                  original_code_executed=False, compiler_invoked=False, tests_run=False)
    report['partial_native_slots'] = [457, 628]
    report['menu_command_selectors'] = []
    for command in [40008, 40009, 40011, 40021, 40027, 40034, 40045, 40047, 40052, 40057]:
        selector = image.get(0x491A60 - image.base + command - 0x9C48, 1)[0]
        target = struct.unpack('<I', image.get(0x491A00 - image.base + selector * 4, 4))[0]
        report['menu_command_selectors'].append(dict(command=command, selector=selector, target_va=target))
    report['fully_completed_native_slots'] = []
    startup = ROOT / 'Yukikoi Melt/config/startup.xml'
    startup_bytes = startup.read_bytes()
    def startup_integer(section, name):
        group = re.search(rb'<' + section + rb'>(.*?)</' + section + rb'>', startup_bytes, re.S)
        leaf = re.search(rb'<' + name + rb'>\s*([0-9]+)\s*</' + name + rb'>', group[1]) if group else None
        return int(leaf[1]) if leaf else None
    report['static_startup_sample'] = dict(path=startup.relative_to(ROOT).as_posix(),
        sha256=hashlib.sha256(startup_bytes).hexdigest(),
        fields={section.decode() + '/' + name.decode(): startup_integer(section, name)
            for section, name in [(b'GAME', b'strframe_count'), (b'GAME', b'autoframe_count'), (b'CONFIG_B', b'mes')]},
        resource_priority_not_inferred_from_loose_file=True)
    report['menu_confirmation_literals'] = []
    for va in [0x9F8A10, 0x9F8A3C]:
        data = image.get(va - image.base, 8192)
        end = next(index for index in range(0, len(data) - 1, 2) if data[index:index + 2] == b'\0\0')
        report['menu_confirmation_literals'].append(dict(va=va, raw_hex=data[:end].hex(), text=data[:end].decode('utf-16le')))
    report['system_volume_literals'] = []
    for va in [0x8E810C, 0x8E8128, 0x9DC8E8]:
        data = image.get(va - image.base, 128)
        end = next(index for index in range(0, len(data) - 1, 2) if data[index:index + 2] == b'\0\0')
        report['system_volume_literals'].append(dict(va=va, raw_hex=data[:end].hex(), text=data[:end].decode('utf-16le')))
    report['input_code_tables'] = []
    for va in [0xA43350, 0xA43010, 0xA43140]:
        rows = []
        for index in range(256):
            data = image.get(va - image.base + index * 33, 33)
            if data[0] == 255: break
            rows.append(dict(code=data[0], raw_hex=data[1:].hex(), label=data[1:].split(b'\0')[0].decode('cp932')))
        else: raise ValueError('pinned physical-key table has no sentinel')
        raw = image.get(va - image.base, len(rows) * 33 + 1)
        report['input_code_tables'].append(dict(va=va, row_bytes=33, count=len(rows), sha256=hashlib.sha256(raw).hexdigest(), rows=rows))
    report['settings_serialized_keys'] = [{
        'va': va, 'raw_hex': image.get(va - image.base, 32).split(b'\0')[0].hex(),
        'text': image.get(va - image.base, 32).split(b'\0')[0].decode('ascii'),
    } for va in [0x90D794, 0x90D7A0, 0x90E59C, 0x90E5A4, 0x90ED94, 0x90ED9C,
                 0x90E9B4, 0x90E9C0, 0x90F1AC, 0x90F1B8,
                 0x90D7AC, 0x90D7BC, 0x90D7C8, 0x90D7D4,
                 0x90D808, 0x90D818, 0x90D838, 0x90D848, 0x90D864, 0x90D874,
                 0x90D744, 0x90D748, 0x90D960, 0x90D968, 0x90D96C, 0x90D714, 0x90D718]]
    report['implemented_subbehaviors'] = [
        '457 asynchronous ordered request, real Windows property sheets and Android UI: messages41f/482/419/483, effects420/41a, windows422/428/6b/41c selected by actual CONFIG/type and CONFIG_B flags, plus enabled auto-selection484',
        'checkbox1096/1111/1119 read/write shared FES block2 offsets8/12/24; reset from default definition before user overrides',
        'Apply/OK change shared FES words and settings-dirty; after any successful Apply, close saves actual configuration once, including Cancel after Apply',
        'unmodified OK/Cancel causes no mutation or save; Reset edits scratch values only until Apply',
        'detailed effect controls write actual block2 offsets0/4/8/12/16/20/24; speed0..4, Boolean toggles, delay1..99 and auto-cursor-delay enable dependency',
        'effects Reset copies32 bytes to local UI scratch but commits only the page controls; hidden-by-mode effect fields and skipmot28 remain unchanged, matching48bcd4/486de4',
        'auto-selection484 follows CONFIG_B/autoselect default0 and actual FES auto_select/native999/configuration GAME/autoselect',
        'auto-selection Reset changes radio display but not scratch value, matching489195..4891b0; clicking a checked radio still updates scratch',
        'window controls commit actual block3 packedARGB/enabledrag/drawwnd; full RGB/alpha0..255 or alpha-only0..100 according to original page selection',
        'window Reset restores its complete12-byte default block, including fields absent from simple-mode controls; original startup visibility and live-drag disable gate are retained',
        'simple message41f/482 reads actual speed/preset/check fields; slider bounds come from GAME/strframe_count and autoframe_count, optional fullskip follows enablefullskip',
        'actual automatic-wait preset edits update existing base/character pair together; message Apply writes block1 speed/auto_p/base/CHAR/still/cont/waitpcm/optionalskip_full and explicitly clears ind',
        'message Reset reads72-byte default snapshot into its mapped control scratch, leaving unrelated block1 words unchanged on Apply; runtime sample preview is still absent',
        'message-page gate uses original CONFIG_B/mes, not msg; absent/unsupported slider bounds record a page-unavailable event and leave the other recovered settings pages available',
        'detailed message419/483 writes actual ind and log_limit; GAME/log/type default1 controls log_limit visibility, original constructor678c0a and parser67a0c9..67a0fd',
        'detailed automatic-wait checkbox enables preset on zero, decimal base/CHAR on nonzero, and both toggle edges copy the selected actual preset pair; no recomputation while merely opening',
        'decimal seconds display one/two fractional digits, parse at most one/two original fractional digits without padding, clamp stored base/CHAR0..100; log_limit10..10000',
        'bounded variable-size Android Apply packet validates all page identities/control counts/changed-value domains before writing any field',
        '628 raw mode write precedes posting; receiver selects68 for zero,69 for every nonzero DWORD at delivery',
        'actual original menu data and language resolver, installed menu receipt and stale UI identities',
        'manual118 existence deletion, enabledrag40034 deletion, WINDOW/mode1 menu detach',
        'menu40002/40008/40009/40021/40027 use existing close/fullscreen/minimize/settings operations',
        'Windows window-procedure subclass receives actual synchronous menu WM_COMMAND and chains original winit procedure',
        'menu40034/40047/40052 notify actual main task with sender0/code20,data0,0; code449,data0,1; code335,data0,1 respectively',
        'task-menu notification keeps queue head and main-task traversal/root VM continuation across budget yields',
        'menu118 opens actual original MANUAL/path (show1),40045 opens active private save directory (show5), with original path/language decoding and read-only original boundary',
        'menu opens have independent non-VM receipt and shared system-open host identity counter; host failure is recorded and dequeued, original menu discards ShellExecute result',
        'Windows actual ShellExecuteW preserves Unicode host paths; Android actual document intent plus bounded read-only private-directory browser and document grants',
        'menu119 enqueues actual sender0/code159/data0,1 before the real host Yes/No confirmation; only Yes queues code21/data0,0 ahead of commands posted while the confirmation was open',
        'title-return confirmation reads original UTF16 language tokens0000-0035/0036 from immutable PE data, resolves current language, checks receipt identity; actual MessageBoxW/Android dialog and canceled-session cleanup',
        'detailed input429 filters actual enabled/nonhidden KEYCUSTOMIZE definitions and edits four physical codes/repeat bytes for keyboard/mouse/joypad using immutable original code-label tables106/8/15 rows',
        'Windows combo/check controls and Android Spinner/CheckBox select actual logical key and physical source; navigation is not an edit, repeated numeric-code table/cache is not a separate binding store',
        'input view preserves489dd0 keyboard repeat operands, mouse code operands and joypad code2 gate; Windows uses actual CheckDlgButton/GetDlgButtonChecked, Android uses corresponding Boolean controls; arbitrary nonstandard Win32 check-state equivalence is not runtime-verified',
        'input Apply rebuilds all80 actual DeviceKeys through the existing setter, resetting live masks/held/repeat/forced metadata; input Reset restores default whole binding records including hidden/disabled/touch fields before applying visible edits',
        'actual KEY/enable_joypad flag gates host device admission and deferred physical observation, clearing disabled joypad pending/physical ownership; default zero follows67b155; no actual hardware provider is fabricated',
        'private setup.xml persistence reuses the existing KEY kb/ms/jp code/repeat writer and platform enable_joypad field; existing startup/default plus private-user reader restores the same live bindings, no second persistence format',
        'legacy sound418/41e volume subpanel writes the actual five raw volume words15d4..15e4, five mute words15e8..15f8 and cont_voice16; Windows/Android numeric edits preserve original BGM/during-voice cross-clamping on either edit',
        'sound Reset copies5628 pre-user default bytes into UI scratch; original Apply commits only five volumes/five mutes/cont_voice and raw W role lookup results, preserving definitions/mastervol/group; the earlier whole-block live Reset interpretation was corrected',
        'sound Apply refreshes the existing category volume cache and settings dirty; existing FES command19/media refresh and private VOICE serialization consume the same block, no extra sound_task creation context',
        'independent SOUNDCONFIG mode or absent proven default sound snapshot skips this legacy subpanel with an explicit unavailable event; no inert legacy controls for a different active sound consumer',
        'menu40011 delivers original sender0/code159/data0,1 before a real OS volume UI request; Windows tries actual system-directory sndvol32.exe then sndvol.exe, preserving show1 and original return>=32 comparison; Android submits Settings.ACTION_SOUND_SETTINGS with a matched actual success/failure receipt',
        'system-volume request shares ordered window delivery and monotonic UI identities, records failures and completes without retrying the launch; custom mixer-object branch523360 remains unrecovered rather than claimed equivalent',
    ]
    report['remaining_behavior'] = [
        '457 independent/system sound pages, system page, live simple/detailed mode switching, message/color samples and system-info export; font and role UI details/caveats are in the dedicated E font/sound-role evidence reports',
        'full-window color sample/trackbar visual feedback is still absent; numeric fields submit actual component values rather than pretending preview equivalence',
        'Rust has no direct block1/3 object consumer, but actual sample FES scripts read messpd/wndcolor/wndframe after configuration notifications; this does not prove missing consumers or justify guessed object writes; exact version notification/object coverage remains to inspect',
        'original settings still need independent/system sound/speech/plugin consumer recovery; recovered font changes share actual MessageDefaults/FES dirty consumers; actual Windows/Android hardware joypad provider remains absent',
        '628 original window+2c display-format fallback lock, custom mixer object branch, system-config external plugin contract and all other enabled menu command receivers',
        'menu version/other-debug/language/system-config commands explicitly reject missing receivers',
        'original resource-module selection may differ from primary PE; unsupported/missing numeric templates fail explicitly',
        'no build, type check, UI/device/game/VM execution or runtime equivalence claim',
    ]
    report['static_script_consumers'] = [dict(path=name,
        sha256=hashlib.sha256((ROOT / name).read_bytes()).hexdigest()) for name in [
            'cs2_full_v401/system/fes/meswnd_A.txt', 'cs2_full_v401/system/fes/meswnd_B.txt']]
    report['static_sound_consumers'] = [dict(path=name,
        sha256=hashlib.sha256((ROOT / name).read_bytes()).hexdigest()) for name in [
            'simulator/crates/desktop/src/sound_legacy.rs', 'simulator/crates/desktop/src/sound_media.rs',
            'simulator/crates/desktop/src/fes_sound.rs', 'simulator/crates/desktop/src/configuration.rs']]
    report['followup_contracts'] = ['reports/settings_font_E_contracts_20261117.json', 'reports/settings_sound_roles_E_contracts_20261117.json']
    report['source_sha256'] = {name: hashlib.sha256((ROOT / name).read_bytes()).hexdigest() for name in SOURCES}
    path = ROOT / 'reports/settings_menu_E_contracts_20261117.json'
    path.write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
    print(path.relative_to(ROOT).as_posix())


if __name__ == '__main__': main()

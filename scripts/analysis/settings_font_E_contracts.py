"""Read pinned font dialog code/data, never load or execute original code."""
from __future__ import annotations
import ast
import hashlib
import json
import struct
from recovery_A_20261116 import Image, ROOT, instruction_row
from settings_menu_E_contracts import dialog


def main():
    image = Image(ROOT / 'Yukikoi Melt/YukikoiMelt.exe',
                  '63de0e14cd8ce11ad2d68e7ed6076641ff5e5b73d822603cc52764009d812976')
    # All regions start at decoded function entries or previously established
    # instruction boundaries. Selector tables are data, never disassembled.
    spans = [(0x487430, 0x87B), (0x48BEC0, 0x86F), (0x601530, 0xBB),
             (0x601610, 0x4E), (0x601660, 0x5D), (0x6016C0, 0x70),
             (0x601730, 0x70), (0x6017A0, 0xD0), (0x601870, 0x50),
             (0x6018C0, 0x60), (0x601920, 0x30), (0x601950, 0x30), (0x679C74, 0x1F2)]
    report = dict(schema='cs2-settings-font-E-contracts/v1',
        pe=dict(path=image.path.relative_to(ROOT).as_posix(), sha256=image.digest, image_base=image.base),
        regions=[dict(va=va, size=size, instructions=[instruction_row(i, image.base)
            for i in image.insns(va - image.base, size)]) for va, size in spans],
        resources=[], command_selectors=[], original_code_executed=False,
        compiler_invoked=False, tests_run=False, completed_native_slots=[])
    for kind in image.pe.DIRECTORY_ENTRY_RESOURCE.entries:
        if kind.id != 5: continue
        for name in kind.directory.entries:
            if name.id not in [0x41B, 0x421]: continue
            for lang in name.directory.entries:
                item = lang.data.struct
                raw = image.get(item.OffsetToData, item.Size)
                report['resources'].append(dict(id=name.id, language=lang.id,
                    sha256=hashlib.sha256(raw).hexdigest(), parsed=dialog(raw)))
    for selector_va, table_va in [(0x487CD8, 0x487CC0), (0x48C75C, 0x48C744)]:
        for command in [1121, 1123, 1130, 1132, 1145, 1300, 1306]:
            selector = image.get(selector_va - image.base + command - 1121, 1)[0]
            target = struct.unpack('<I', image.get(table_va - image.base + selector * 4, 4))[0]
            report['command_selectors'].append(dict(selector_va=selector_va, command=command,
                selector=selector, target_va=target))
    report['serialized_gate'] = [dict(va=va, text=image.get(va - image.base, 16).split(b'\0')[0].decode('ascii'))
        for va in [0x90D754, 0x90D75C]]
    report['contracts'] = dict(
        native=457, resources=[0x41B, 0x421], config_gate='CONFIG_B/font', gate_default=1,
        controls={'face':1121, 'sample':1125, 'outline':1132, 'shadow':1130,
                  'bold':1123, 'proportional':1300, 'variable_width':1306, 'reset':1145},
        scratch={'face':0xA8DF1C, 'outline':0xA8E01C, 'shadow':0xA8E020,
                 'bold':0xA8E024, 'proportional':0xA8E028, 'fixed':0xA8E02C},
        face_limit_utf16_units=63, preview_size=32, preview_weights=[400,700],
        font_charset=128, vertical=False, fixed_filter='(lfPitchAndFamily & 15) == 1',
        reset='1145 enters487a39/48c4c4: check proportional=1, rebuild full list, select default face or first; default outline/shadow, bold=0, variable_width=0 => stored fixed=1',
        previous_partial_reset_interpretation_rejected='starting at487b8c missed predecessor487a39; selector table fixes the actual entry')
    report['implemented'] = [
        'Windows real EnumFontFamiliesExW Japanese128 horizontal names/pitch; only actual names available to shared message font device are selectable',
        'Android real installed/resource SFNT names, monospace metadata and Japanese glyph support; not a fabricated GDI charset provider',
        'both platforms actual filtered font selector, proportion toggle preserves name or chooses first, real CPU32-pixel preview of translated original sample',
        'Windows top-down DIB/STATIC and Android RGBA Bitmap/ImageView; failures show an explicit message, no placeholder pixels',
        'Apply commits MessageDefaults, raw five controls and FES serialization fields; existing dirty broadcast/command19 refreshes inherited text without recreating messages',
        'Reset uses pre-user resource defaults for face/shadow/outline but original literal1/0/0 for proportional/bold/variable; Cancel only discards scratch after earlier Apply remains committed',
        'existing private setup.xml writer/reader persists FONT face/shadow/around/rightdown/bold/proportional/fixed; existing bounded UI receipts and frontend session identity remain active',
    ]
    report['remaining'] = [
        '40057 is cs2confx.dll external system configuration, distinct from457 font pages; no equivalent system plugin UI yet',
        'font device catalog excludes original GDI faces unsupported by the message SFNT backend; Android has no Win32 charset128 provider; no exact GDI hinting/metrics equivalence claim',
        '457 role UI followed in settings_sound_roles_E_contracts_20261117.json with original raw-W lookup misses preserved; independent/system sound, system page, mode switch, message/color samples and628 other commands remain partial',
        'grammar-only verification; no typecheck, compile, test, native/VM/game/APK/device execution',
    ]
    names = ['settings_menu_native.rs', 'settings_menu_windows.rs', 'message_native.rs',
             'message_backend.rs', 'message_platform.rs', 'fes_settings.rs', 'language_native.rs', 'fes_tasks.rs', 'fes_text.rs']
    sources = ['simulator/crates/desktop/src/' + name for name in names] + [
        'simulator/crates/android/src/lib.rs', 'simulator/android/app/src/main/cpp/bridge.c',
        'simulator/android/app/src/main/java/org/catsystem2/player/PlayerActivity.java']
    report['source_sha256'] = {name:hashlib.sha256((ROOT / name).read_bytes()).hexdigest() for name in sources}
    ast.parse((ROOT / 'scripts/analysis/settings_font_E_contracts.py').read_text(encoding='utf8'))
    path = ROOT / 'reports/settings_font_E_contracts_20261117.json'
    path.write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n', encoding='utf8')
    print(path.relative_to(ROOT).as_posix())


if __name__ == '__main__': main()

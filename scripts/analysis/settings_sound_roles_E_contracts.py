"""Pinned data-only inspection of457 role selection and Reset commit bounds."""
from __future__ import annotations
import ast
import hashlib
import json
import struct
from recovery_A_20261116 import Image, ROOT, instruction_row


def main():
    image = Image(ROOT / 'Yukikoi Melt/YukikoiMelt.exe',
        '63de0e14cd8ce11ad2d68e7ed6076641ff5e5b73d822603cc52764009d812976')
    spans = [(0x484B40,0x106E),(0x48A2A0,0x107F),(0x5FF280,0xD0)]
    report = dict(schema='cs2-settings-sound-roles-E-contracts/v1',
        pe=dict(path=image.path.relative_to(ROOT).as_posix(), sha256=image.digest, image_base=image.base),
        native=457, partial=True, completed_native_slots=[],
        compiler_invoked=False, tests_run=False, original_code_executed=False,
        regions=[dict(va=va,size=size,instructions=[instruction_row(i,image.base)
            for i in image.insns(va-image.base,size)]) for va,size in spans])
    report['selectors'] = []
    for command in [1114,1116,1118,1145]:
        selector=image.get(0x485BE0-image.base+command-1078,1)[0]
        target=struct.unpack('<I',image.get(0x485BC4-image.base+selector*4,4))[0]
        report['selectors'].append(dict(command=command,selector=selector,target_va=target))
    report['imports'] = [dict(va=item.address,name=item.name.decode('ascii'))
        for entry in image.pe.DIRECTORY_ENTRY_IMPORT for item in entry.imports
        if item.name and item.address in [0x8E24C0,0x8E24C8,0x8E24BC]]
    report['contracts'] = dict(resources=[0x418,0x41E],channels=48,record_bytes=116,first_record=20,
        play_offset=0,name_offset=4,name_bytes=32,
        list_control=1114,list_style='0x50a10109: notify/multiple-selection, no sort',
        display='484e10 UTF8-to-wide5ff280 then current-language4cdeb0; LB_FINDSTRINGEXACT1a2 deduplicates, LB_ADDSTRING180 preserves declaration order',
        refresh='485686 converts/translates each scratch name and LB_SETSEL185 overwrites duplicate selection; later record wins; unknown rows stay unchanged',
        all_on='1116->4853d7 writes1 only to named scratch play records, then401 refreshes actual list',
        all_off='1118->4853f8 writes0 only to named scratch play records, then401 refreshes actual list',
        lookup='1114->48509a and Apply484f54 pass RAW byte-name pointers to actual SendDlgItemMessageW LB_FINDSTRINGEXACT; no UTF8 conversion/language resolution at those lookups',
        lookup_miss='preserve existing play DWORD; never silently change to translated-name matching',
        empty_name='Apply writes live play=1',
        reset='1145->485418 copies5628 resource-default bytes to scratch and401 refreshes visible controls; Apply writes five volumes/five mutes/cont_voice and original role lookup result into live, then copies live back to scratch',
        corrected_previous_batch='Reset never copies complete default scratch directly into live; hidden definitions/mastervol/group remain unchanged')
    report['implementation'] = [
        'real Windows multiple-selection ListBox and Android ListView, current-language names, original order/dedup/last-record selection and allON/allOFF',
        'actual private VOICE play00..47 writer, 606/FES play/media consumers share the same raw block; lookup misses retain live state and produce an explicit matched/missed event',
        'Reset updates only UI scratch; eventual Apply preserves raw definition/mastervol/group words and follows the original field/lookup commit bounds',
        'no original runtime operation or native/VM/UI/device/game/APK invocation used to inspect these facts',
    ]
    report['remaining'] = [
        'source-pointer mismatch means a displayed checked role need not change live play; no corrected-name semantics are invented or claimed',
        'portable equality preserves ASCII case-insensitive names; full Win32 locale collation equivalence remains unresolved',
        'sample playback, independent/system sound,40057 plugin pages,other menu families remain incomplete;457/628 not counted complete',
    ]
    names=['simulator/crates/desktop/src/'+name for name in ['settings_menu_native.rs','settings_menu_windows.rs',
        'sound_legacy.rs','configuration.rs','fes_sound.rs','sound_media.rs']]
    names+=['simulator/android/app/src/main/java/org/catsystem2/player/PlayerActivity.java']
    report['source_sha256']={name:hashlib.sha256((ROOT/name).read_bytes()).hexdigest() for name in names}
    ast.parse((ROOT/'scripts/analysis/settings_sound_roles_E_contracts.py').read_text(encoding='utf8'))
    path=ROOT/'reports/settings_sound_roles_E_contracts_20261117.json'
    path.write_text(json.dumps(report,ensure_ascii=False,indent=2)+'\n',encoding='utf8')
    print(path.relative_to(ROOT).as_posix())


if __name__=='__main__':main()

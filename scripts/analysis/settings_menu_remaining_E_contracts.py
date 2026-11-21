"""Read-only next-menu evidence; no UI/native/game code is invoked."""
from __future__ import annotations
import ast
import json
import struct
from recovery_A_20261116 import Image, ROOT, instruction_row


def main():
    image=Image(ROOT/'Yukikoi Melt/YukikoiMelt.exe',
        '63de0e14cd8ce11ad2d68e7ed6076641ff5e5b73d822603cc52764009d812976')
    spans=[(0x4914FD,0x30),(0x4915A4,0x28),(0x4915D1,0xA5),(0x491984,0x54),
           (0x491C80,0x16E),(0x491DF0,0xDD),(0x491ED0,0x81),(0x491F60,0x1D5),(0x492140,0x53C),
           (0x533FF0,0x84),(0x5340F0,0x90),(0x4D3500,4)]
    report=dict(schema='cs2-settings-menu-remaining-E-contracts/v1',
        pe=dict(path=image.path.relative_to(ROOT).as_posix(),sha256=image.digest,image_base=image.base),
        compiler_invoked=False,tests_run=False,original_code_executed=False,
        completed_commands=[],selectors=[],regions=[dict(va=va,size=size,instructions=[instruction_row(i,image.base)
            for i in image.insns(va-image.base,size)]) for va,size in spans])
    for command in [40019,40037,40041,40063,40065,40074,40075,40084,40085,40094]:
        selector=image.get(0x491A60-image.base+command-0x9C48,1)[0]
        target=struct.unpack('<I',image.get(0x491A00-image.base+selector*4,4))[0]
        report['selectors'].append(dict(command=command,selector=selector,target_va=target))
    report['next_receivers']={
        '40041':'4914fd first159 then DialogBox resource9 callback4830f0; actual About resource/version data still needs bounded receiver',
        '40019':'4915a4 first159 then DialogBox resource81 callback490650; actual system-info producer/UI still missing',
        '40037':'4915d1 first159 then active private save directory and system-detail file producer; not an arbitrary OS settings launch',
        '40063':'491984->5340f0(first argument12, packet30,0,0); distinct envelope12 vs default533ff0 envelope11; exact target/receiver not guessed',
        '40065..40074':'primary-language index=command-40065,491c80 valid/range/unchanged gate,table/image/title/save/dirty/menu consumers',
        '40075..40084':'main-text index=command-40075,491df0 valid/range/unchanged gate,table/save/dirty/menu consumers',
        '40085..40094':'subtitle index=command-40085,491ed0 invalid->FFFFFFFF,enabled global+e0d4 gate,save/dirty/menu consumers',
        '491f60':'checks actual three selections and conditional subtitle-off row; labels/dynamic menu production still needed before exposing range commands',
    }
    report['boundary']='Do not equate raw5340f0 argument12 with current_task_id or task12 without receiver evidence; do not claim these menu commands complete.'
    report['language_menu_producer']={
        'entry':0x492140,'record_bytes':56,'prefix_offset':0,'name_offset':24,
        'main_modifier_byte':48,'main_key_word':50,'subtitle_modifier_byte':52,'subtitle_key_word':54,
        'subtitle_gate':'global+e0d4','count_gate':'zero/one language removes placeholder and parent menu; count>1 creates actual entries',
        'unfinished':'Existing portable language state stores prefixes only; recover actual name/modifier/key fields before producing language menus. Do not invent labels from prefix or omit shortcut semantics to count commands complete.',
    }
    report['language_menu_formats']=[]
    for va in [0x9F7FD8,0x9F7FE8,0x9F8014,0x9F803C,0x9F8064,0x9F8080,
               0x9F8094,0x9F80BC,0x9F80E0,0x9F8108,0x9F8120,0x9F8134]:
        raw=image.get(va-image.base,256)
        end=next(at for at in range(0,len(raw),2) if raw[at:at+2]==b'\0\0')
        report['language_menu_formats'].append(dict(va=va,raw_hex=raw[:end].hex(),text=raw[:end].decode('utf-16le')))
    ast.parse((ROOT/'scripts/analysis/settings_menu_remaining_E_contracts.py').read_text(encoding='utf8'))
    path=ROOT/'reports/settings_menu_remaining_E_contracts_20261117.json'
    path.write_text(json.dumps(report,ensure_ascii=False,indent=2)+'\n',encoding='utf8')
    print(path.relative_to(ROOT).as_posix())


if __name__=='__main__':main()

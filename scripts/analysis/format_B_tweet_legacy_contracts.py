"""Fixed original UI/credential bytes. Does not read user credential files."""
from __future__ import annotations
import hashlib
import json
from recovery_A_20261116 import Image, ROOT, instruction_row

def main():
    image = Image(ROOT / "Yukikoi Melt/YukikoiMelt.exe",
        "63de0e14cd8ce11ad2d68e7ed6076641ff5e5b73d822603cc52764009d812976")
    spans = {"ctor": (0x58df90, 0x18a), "load_cache": (0x58f8c2, 0x315),
        "write_cache": (0x58ffea, 0x151), "protect": (0x5308b0, 0x115),
        "decrypt_rounds": (0x531870, 0x440), "encrypt_rounds": (0x531cb0, 0x440),
        "protect_blocks": (0x5320f0, 0x1b9), "blowfish_key_schedule": (0x5322b0, 0x20e),
        "seed": (0x518c90, 0x47), "random": (0x518ce0, 0x145),
        "special_folder": (0x6010b0, 0x17d), "count_text": (0x58f140, 0x173),
        "tweet_dialog": (0x58ea90, 0x580), "oauth_dialog": (0x58f010, 0x130),
        "dbcs_lead": (0x600a00, 0x50)}
    rows = {name: {"va": hex(va), "size": size,
                  "sha256": hashlib.sha256(image.get(va-image.base, size)).hexdigest(),
                  "instructions": [instruction_row(i, image.base) for i in image.insns(va-image.base, size)]}
            for name, (va, size) in spans.items()}
    table = image.get(0x601230-image.base, 48)
    imports = [row for rva, row in image.imports.items()
               if rva+image.base in (0x8e2314,0x8e2374,0x8e2378,0x8e2500,0x8e24c8,0x8e2508)]
    prefixes = []
    pointer_table = image.get(0xa28580-image.base, 12)
    for index in range(0, 12, 4):
        va = int.from_bytes(pointer_table[index:index+4], "little")
        if va:
            raw = image.get(va-image.base, 32).split(b"\0", 1)[0]
            prefixes.append({"va": hex(va), "bytes": raw.hex(), "ascii": raw.decode("ascii", "replace")})
    output = ROOT / "archive/2026-11/20261117_format_B_tweet_legacy_contracts.json"
    output.write_text(json.dumps({"path": str(image.path.relative_to(ROOT)), "sha256": image.digest,
        "target_executed": False, "credentials_read": False, "routines": rows,
        "imports": imports, "special_folder_dispatch": [hex(int.from_bytes(table[i:i+4], "little")) for i in range(0,48,4)],
        "pi_constants_first18_hex": image.get(0xa260a0-image.base, 72).hex(), "url_prefixes": prefixes}, indent=2)+"\n", encoding="utf8")
    print(json.dumps({"report": output.relative_to(ROOT).as_posix(), "imports": imports,
        "selector7_va": hex(int.from_bytes(table[28:32], "little")), "url_prefixes": prefixes}, indent=2))

if __name__ == "__main__": main()

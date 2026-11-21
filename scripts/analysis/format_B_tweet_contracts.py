"""Read immutable Tweet PE bytes; no target code or network is executed."""
from __future__ import annotations
import argparse
import json
import hashlib
import subprocess
from pathlib import Path
from recovery_A_20261116 import Image, ROOT, instruction_row

SPANS = {
    "ctor": (0x58df90, 0x240), "dtor": (0x58e450, 0xc0),
    "receiver": (0x58f350, 0x2dc), "worker": (0x58f800, 0x161c),
    "wrappers": (0x5ac920, 0x280), "singleton_text": (0x66c700, 0x238),
    "singleton_path": (0x66c3d0, 0x330), "tweet_dialog": (0x58ea90, 0x580),
    "oauth_dialog": (0x58f010, 0x340), "reply_error": (0x58e5d0, 0x4c0),
}

def main():
    p = argparse.ArgumentParser()
    p.add_argument("--show", nargs="*", default=[])
    p.add_argument("--span", nargs=2)
    p.add_argument("--string", nargs="*", default=[])
    p.add_argument("--syntax-only", action="store_true")
    a = p.parse_args()
    if a.syntax_only:
        formatter = Path.home() / ".rustup/toolchains/stable-x86_64-pc-windows-gnu/bin/rustfmt.exe"
        files = ["tweet_native.rs", "tweet_worker.rs", "tweet_protocol.rs", "tweet_platform.rs",
                 "lib.rs", "native_tasks.rs", "task_commands.rs", "data_services.rs", "image_export_native.rs"]
        rows = []
        for name in files:
            path = ROOT / "simulator/crates/desktop/src" / name
            raw = path.read_bytes()
            result = subprocess.run([str(formatter), "--edition", "2024", "--config", "skip_children=true",
                "--emit", "stdout"], input=raw, capture_output=True, check=False)
            rows.append({"path": path.relative_to(ROOT).as_posix(), "sha256": hashlib.sha256(raw).hexdigest(),
                "parse": result.returncode == 0, "unchanged": raw == path.read_bytes(),
                "diagnostic": result.stderr.decode("utf8", errors="replace")})
        output = ROOT / "archive/2026-11/20261117_format_B_tweet_syntax.json"
        output.write_text(json.dumps({"compiled": False, "type_checked": False, "files": rows}, indent=2)+"\n", encoding="utf8")
        print(json.dumps(rows, indent=2)); return
    im = Image(ROOT / "Yukikoi Melt/YukikoiMelt.exe",
        "63de0e14cd8ce11ad2d68e7ed6076641ff5e5b73d822603cc52764009d812976")
    spans = dict(SPANS)
    if a.span:
        spans["extra"] = tuple(int(x, 16) for x in a.span)
        a.show.append("extra")
    rows = {name: [instruction_row(i, im.base) for i in im.insns(va-im.base, n)]
            for name, (va, n) in spans.items()}
    strings = {}
    for instructions in rows.values():
        for i in instructions:
            import re
            for match in re.findall(r"0x[0-9a-f]+", i["operands"]):
                va = int(match, 16)
                section = im.section(va-im.base)
                if section and not section["executable"]:
                    raw = im.get(va-im.base, 160)
                    strings[match] = {"hex": raw.hex(), "ascii": raw.split(b"\0", 1)[0].decode("ascii", "replace")}
    output = ROOT / "archive/2026-11/20261117_format_B_tweet_contracts.json"
    output.write_text(json.dumps({"PE": str(im.path.relative_to(ROOT)), "sha256": im.digest,
        "target_executed": False, "spans": spans, "routines": rows, "strings": strings,
        "imports": list(im.imports.values())}, indent=2)+"\n", encoding="utf8")
    for name in a.show:
        print(name)
        for i in rows[name]:
            print(f'{i["rva"]+im.base:08x} {i["mnemonic"]:8s} {i["operands"]}')
    if not a.show: print(output.relative_to(ROOT))
    for value in a.string:
        raw = im.get(int(value, 16)-im.base, 300)
        wide = raw[:next((i for i in range(0, len(raw)-1, 2) if raw[i:i+2] == b"\0\0"), len(raw)//2*2)].decode("utf-16le", errors="replace")
        print(value, "ascii", ascii(raw.split(b"\0", 1)[0].decode("cp932", errors="replace")), "wide", ascii(wide))

if __name__ == "__main__": main()

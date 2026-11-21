"""Read-only evidence for the ordinary445 dialog's actual title and APIs."""
from __future__ import annotations
import json
from recovery_A_20261116 import Image, ROOT, instruction_row

def main() -> None:
    rows = []
    for name, digest, wrapper, caption, iat, wide in [
        ("cs2_full_v401/system/cs2_open.exe",
         "0e1dde51e86e9152d8e1d27b82efb15916cb9f82d23dd30990212fafd96c8b9f",
         0x51B460, 0x998958, 0x98E3C8, False),
        ("Yukikoi Melt/YukikoiMelt.exe",
         "63de0e14cd8ce11ad2d68e7ed6076641ff5e5b73d822603cc52764009d812976",
         0x5912E0, 0x8EF85C, 0x8E26A8, True),
    ]:
        image = Image(ROOT / name, digest)
        raw = image.get(caption - image.base, 128)
        end = next(index for index in range(0, len(raw) - 1, 2) if raw[index:index + 2] == b"\0\0") \
            if wide else raw.index(0)
        title = raw[:end].decode("utf-16-le" if wide else "cp932")
        if title != "system error":
            raise ValueError("native445 dialog title changed")
        instructions = []
        for item in image.insns(wrapper - image.base, 300):
            instructions.append(instruction_row(item, image.base))
            if item.mnemonic == "ret": break
        rows.append({
            "pe": name, "sha256": image.digest, "image_base": image.base,
            "wrapper": instructions, "caption_va": caption, "caption_bytes": raw[:end].hex(),
            "title": title, "import": image.imports[iat - image.base],
        })
    output = ROOT / "reports/host_dialog_contracts_20261117.json"
    output.write_text(json.dumps({
        "schema": "cs2-host-dialog445/v1", "profiles": rows,
        "contract": "formatted text, actual system-error title, flags0/OK dialog; state2; OS result ignored",
        "status": "static-PE-and-source-wiring-not-runtime-equivalence",
        "compiled": False, "tests_run": False, "game_or_DLL_execution": False,
    }, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"report": output.relative_to(ROOT).as_posix(), "profiles": len(rows)}))

if __name__ == "__main__":
    main()

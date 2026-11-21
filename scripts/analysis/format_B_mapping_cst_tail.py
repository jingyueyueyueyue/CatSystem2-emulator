"""Read-only float-map/CST/save-wrapper evidence; never executes original code."""
from __future__ import annotations
import hashlib
import json
from pathlib import Path
import capstone
import pefile
from native_batch_contracts import analyze

ROOT = Path(__file__).resolve().parents[2]
EXPECTED = "63de0e14cd8ce11ad2d68e7ed6076641ff5e5b73d822603cc52764009d812976"
RANGES = {
    "history_merge981": (0x68B6F0, 0x190),
    "option1064": (0x689B80, 0x260),
    "save1031": (0x6EAD50, 0x300),
    "save990": (0x6EB3C0, 0x400),
}


def main() -> None:
    raw = (ROOT / "Yukikoi Melt/YukikoiMelt.exe").read_bytes()
    if hashlib.sha256(raw).hexdigest() != EXPECTED:
        raise SystemExit("Unexpected PE SHA256")
    pe = pefile.PE(data=raw, fast_load=True)
    md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_32)
    routines = {key: [{"va": item.address, "bytes": item.bytes.hex(),
                       "mnemonic": item.mnemonic, "operands": item.op_str}
                      for item in md.disasm(pe.get_data(va - 0x400000, length), va)]
                for key, (va, length) in RANGES.items()}
    stem = ROOT / "archive/2026-11/20261117_format_B_mapping_cst_tail"
    Path(str(stem) + "_static.json").write_text(json.dumps({
        "sha256": EXPECTED, "original_code_executed": False, "routines": routines,
    }, indent=2) + "\n", encoding="utf-8")
    Path(str(stem) + "_wrappers.json").write_text(json.dumps(
        analyze([*range(885, 903), 981, 990, 1031, 1064], 256), indent=2) + "\n",
        encoding="utf-8")
    print(stem.relative_to(ROOT).as_posix())


if __name__ == "__main__":
    main()

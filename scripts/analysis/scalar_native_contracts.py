"""Read-only pinned scalar helper evidence. Does not evaluate engine code."""
from __future__ import annotations
import hashlib
import json
from pathlib import Path
import struct
import capstone
import pefile

ROOT = Path(__file__).resolve().parents[2]
SOURCE = ROOT / "Yukikoi Melt/YukikoiMelt.exe"
SHA256 = "63de0e14cd8ce11ad2d68e7ed6076641ff5e5b73d822603cc52764009d812976"

def main() -> None:
    data = SOURCE.read_bytes()
    if hashlib.sha256(data).hexdigest() != SHA256:
        raise ValueError("scalar evidence PE fingerprint changed")
    pe = pefile.PE(data=data, fast_load=True)
    base = pe.OPTIONAL_HEADER.ImageBase
    decoder = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_32)
    def instructions(address: int, length: int) -> list[dict]:
        return [
            {"va": ins.address, "bytes": ins.bytes.hex(),
             "mnemonic": ins.mnemonic, "operands": ins.op_str}
            for ins in decoder.disasm(pe.get_data(address - base, length), address)
        ]
    constants = []
    for address, name in [(0x9BF1C0, "exponent_multiplier"), (0x9BF220, "zero_negative_infinity")]:
        raw = pe.get_data(address - base, 16)
        constants.append({
            "va": address, "label": name, "bytes": raw.hex(),
            "double_values": [repr(value) for value in struct.unpack("<2d", raw)],
        })
    report = {
        "binary": SOURCE.relative_to(ROOT).as_posix(), "sha256": SHA256,
        "status": "static-scalar-identification-not-floating-point-equivalence",
        "slot": 461, "wrapper": instructions(0x5936F0, 0x24),
        "helper": instructions(0x859CE0, 0x2B0), "constants": constants,
        "identification": "double common logarithm: exponent multiplier split around log10(2); wrapper promotes f32 and converts result back to f32",
        "limits": "Independent host log10 has not been compared bitwise with the original CRT; errno, FP flags and non-default rounding are not claimed.",
        "original_code_executed": False,
    }
    output = ROOT / "archive/2026-11/20261117_scalar_native_contracts.json"
    output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"report": output.relative_to(ROOT).as_posix(), "original_code_executed": False}))

if __name__ == "__main__":
    main()

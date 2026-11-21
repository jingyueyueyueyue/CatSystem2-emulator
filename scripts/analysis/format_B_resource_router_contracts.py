"""Read-only PE/list evidence; does not call any runtime or original function."""
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
    "startup_mounts": (0x48D407, 0x208),
    "register_mount_and_list": (0x52C790, 0x55C),
    "first_free_slot": (0x52D580, 0x14),
    "routing_modes": (0x52D5A0, 0x906),
    "list_line_reader": (0x600800, 0x6D),
    "member_copy": (0x52A230, 0x22),
    "sound_exists284": (0x52C5A0, 0x29),
    "motion_contains514": (0x6E93D0, 0x65),
    "motion_register_basename": (0x6E8AFA, 0xA9),
    "motion_basename_helper": (0x600620, 0x56),
    "image_records703": (0x5CA010, 0x490),
    "image_allocate703": (0x5CA590, 0xCE),
    "image_animation_probe703": (0x5CB7B0, 0x112),
    "image_selector_parser703": (0x6EFFA0, 0xC9),
}


def main() -> None:
    raw = (ROOT / "Yukikoi Melt/YukikoiMelt.exe").read_bytes()
    assert hashlib.sha256(raw).hexdigest() == EXPECTED
    pe = pefile.PE(data=raw, fast_load=True)
    md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_32)
    routines = {key: [{"va": i.address, "bytes": i.bytes.hex(),
                       "mnemonic": i.mnemonic, "operands": i.op_str}
                      for i in md.disasm(pe.get_data(va - 0x400000, size), va)]
                for key, (va, size) in RANGES.items()}
    samples = []
    for path in ROOT.rglob("*.lst"):
        if any(part in {"archive", "reference", "target"} for part in path.parts):
            continue
        data = path.read_bytes()
        samples.append({"path": path.relative_to(ROOT).as_posix(),
                        "sha256": hashlib.sha256(data).hexdigest(), "hex": data.hex()})
    stem = ROOT / "archive/2026-11/20261117_format_B_resource_router"
    Path(str(stem) + "_static.json").write_text(json.dumps({
        "sha256": EXPECTED, "original_code_executed": False,
        "routines": routines, "list_samples": samples,
    }, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    Path(str(stem) + "_direct.json").write_text(json.dumps(analyze([284,514,703,704],256), indent=2) + "\n", encoding="utf8")
    print(stem.relative_to(ROOT).as_posix())


if __name__ == "__main__":
    main()

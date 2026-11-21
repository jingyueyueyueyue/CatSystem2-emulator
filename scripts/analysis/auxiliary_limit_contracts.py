"""Pin the auxiliary pixel limit separately from the image-reduction enum.

Read immutable PE bytes and source grammar only. Never invoke the game, its
DLL, a compiler, the VM or the implemented image/capture algorithms.
"""
from __future__ import annotations

import json

from recovery_A_20261116 import Image, ROOT, instruction_row


def main() -> None:
    engine = Image(ROOT / "Yukikoi Melt/YukikoiMelt.exe",
        "63de0e14cd8ce11ad2d68e7ed6076641ff5e5b73d822603cc52764009d812976")
    plugin = Image(ROOT / "Yukikoi Melt/cs2confx.dll",
        "baefc568b127d0dda738758b32b9a70a40a7241cb26996e8c58c34be61d3eb19")
    regions = []
    for image, role, rva, size in [
        (plugin, "separate-tex-size0-and-cap-sentinel-minus1", 0x3e586, 0x1e),
        (plugin, "user-tex-size-field-serialization", 0x43a80, 0x18),
        (engine, "backbuffer-gates-reuse-and-limit", 0x1c5500, 0xff),
        (engine, "cap-sentinel-to-min-device-extents", 0x1c6151, 0x1f),
    ]:
        regions.append(dict(source=image.path.relative_to(ROOT).as_posix(),
            sha256=image.digest, role=role, rva=hex(rva),
            instructions=[instruction_row(i, image.base) for i in image.insns(rva, size)]))
    output = ROOT / "archive/2026-11/20261117_auxiliary_limit_contracts.json"
    output.write_text(json.dumps(dict(
        mode="bounded-static-PE-read-and-source-contract",
        regions=regions,
        source_consumer={
            "producer": "RenderTaskStore::auxiliary_dimension_limit",
            "software_limit": "existing MAX_WORK_BYTES/4 for a positive RGBA surface axis",
            "consumers": ["ordinary858", "frame_effect_capture_policy"],
            "reuse": "existing pool remains before any new allocation-size check",
            "not_consumed_as_pixels": "D3D/tex_size enum0/1/2",
        },
        boundaries=["software capability is not a claim about original GPU caps",
            "NPOT/device-reset equivalence remains independent",
            "image-reduction enum has no new texture-reduction consumer in this batch"],
        compiler_invoked=False, tests_run=False, original_code_executed=False,
    ), indent=2) + "\n", encoding="utf-8")
    print(output.relative_to(ROOT).as_posix())


if __name__ == "__main__":
    main()

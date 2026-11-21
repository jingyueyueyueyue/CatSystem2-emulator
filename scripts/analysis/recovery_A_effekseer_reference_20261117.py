"""Pin public Effekseer format/lifecycle references for independent static work.

Downloads source text only. Never loads or executes the reference engine.
Research inputs belong in the existing dated archive, not runtime sources.
"""
import hashlib
import json
import urllib.request

from recovery_A_20261116 import ROOT

COMMIT = "4a3f2649f4937538279450ed244319bdcef0617d"
FILES = ["Release/LICENSE.txt"] + ["Dev/Cpp/Effekseer/Effekseer/" + name for name in [
    "Effekseer.Effect.cpp", "Effekseer.EffectNode.cpp", "Effekseer.EffectNode.h",
    "Effekseer.EffectNodeRoot.cpp", "Effekseer.EffectNodeSprite.cpp", "Effekseer.EffectNodeSprite.h",
    "Effekseer.Instance.cpp", "Effekseer.InternalStruct.h", "Effekseer.Base.h",
    "Effekseer.Manager.cpp", "Effekseer.InstanceContainer.cpp", "Effekseer.InstanceGroup.cpp",
    "Effekseer.Instance.h", "Effekseer.Matrix43.cpp", "Effekseer.Matrix43.h",
    "Effekseer.EffectNodeRing.cpp", "Effekseer.EffectNodeRing.h",
    "Effekseer.EffectNodeRibbon.cpp", "Effekseer.EffectNodeRibbon.h",
    "Effekseer.Setting.cpp",
    "Effekseer.FCurves.cpp", "Effekseer.FCurves.h",
]] + ["Dev/Cpp/Effekseer/Effekseer.h",
      "Dev/Cpp/EffekseerRendererDX9/EffekseerRenderer/EffekseerRendererDX9.RenderState.cpp",
      "Dev/Cpp/EffekseerRendererDX9/EffekseerRenderer/EffekseerRendererDX9.Renderer.cpp",
      "Dev/Cpp/EffekseerRendererCommon/EffekseerRenderer.SpriteRendererBase.h",
      "Dev/Cpp/EffekseerRendererCommon/EffekseerRenderer.RingRendererBase.h",
      "Dev/Cpp/EffekseerRendererCommon/EffekseerRenderer.RibbonRendererBase.h"]


def main():
    rows = []
    for name in FILES:
        output = ROOT / "archive/2026-11" / ("20261117_effekseer101_" + name.rsplit("/", 1)[1])
        url = f"https://raw.githubusercontent.com/effekseer/Effekseer/{COMMIT}/{name}"
        if not output.exists():
            request = urllib.request.Request(url, headers={"User-Agent": "CS2-static-format-research"})
            with urllib.request.urlopen(request, timeout=30) as response:
                data = response.read(4 * 1024 * 1024)
            output.write_bytes(data)
        data = output.read_bytes()
        rows.append({"upstream_path": name, "archive_path": output.relative_to(ROOT).as_posix(),
                     "url": url, "sha256": hashlib.sha256(data).hexdigest(), "bytes": len(data)})
    payload = {"schema": "cs2-recovery-A-effekseer-reference/v1", "commit": COMMIT,
               "tag_navigation": "101", "files": rows, "source_execution": False,
               "source_copy_into_runtime": False,
               "scope": "read format and behavioral contracts; implement independently in Rust"}
    (ROOT / "reports/recovery_A_20261117_effekseer_reference.json").write_text(
        json.dumps(payload, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"commit": COMMIT, "archived_text_files": len(rows), "no_execution": True}))


if __name__ == "__main__":
    main()

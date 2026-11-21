"""Read changed Rust grammar through rustfmt stdin; never build or run Rust."""
from pathlib import Path
import hashlib
import json
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]
FILES = [
    "formats/src/animation_alias.rs",
    "script/src/anm.rs", "script/src/anm_snapshot.rs", "runtime/src/object_graph.rs",
    "desktop/src/image_manager.rs", "desktop/src/image_resource_transform.rs",
    "desktop/src/image_resource_surface.rs",
    "desktop/src/image_resource_state.rs", "desktop/src/image_transform_native.rs",
    "desktop/src/image_native.rs", "desktop/src/list_native.rs", "desktop/src/list_snapshot.rs",
    "desktop/src/image_resample.rs",
    "desktop/src/image_copy.rs",
    "desktop/src/list_tiles.rs", "desktop/src/object_graph.rs", "desktop/src/data_services.rs",
    "desktop/src/lib.rs",
    "desktop/src/plane_native.rs", "desktop/src/plane_snapshot.rs",
    "desktop/src/canvas_native.rs", "desktop/src/canvas_snapshot.rs",
    "desktop/src/movie_plane.rs", "desktop/src/raster_plane.rs",
]

EMPTY_SURFACES = [
    "renderer/src/image_surface.rs", "renderer/src/lib.rs",
    "desktop/src/image_pixels.rs", "desktop/src/image_resource_state.rs",
    "desktop/src/image_resample.rs", "desktop/src/image_manager.rs",
    "desktop/src/image_resource_surface.rs", "desktop/src/image_resource_transform.rs",
    "desktop/src/image_transform_native.rs", "desktop/src/image_native.rs",
    "desktop/src/list_native.rs", "desktop/src/list_tiles.rs",
    "desktop/src/image_blit.rs", "desktop/src/image_copy.rs",
    "desktop/src/image_global_blit.rs", "desktop/src/lib.rs",
]

PLANE_STATE = [
    "desktop/src/plane_snapshot.rs", "desktop/src/plane_history.rs",
    "desktop/src/raster_plane.rs", "desktop/src/media_wrapper_native.rs",
    "desktop/src/kx_native.rs", "desktop/src/list_snapshot.rs",
]

def main():
    formatter = Path.home() / ".rustup/toolchains/stable-x86_64-pc-windows-gnu/bin/rustfmt.exe"
    rows = []
    files = [name for name in FILES if name in {
        "desktop/src/image_manager.rs", "desktop/src/image_resource_transform.rs",
        "desktop/src/image_resample.rs", "desktop/src/image_copy.rs", "desktop/src/lib.rs",
        "desktop/src/image_native.rs",
    }] if "--current-batch" in sys.argv else FILES
    if "--empty-surfaces" in sys.argv:
        files = EMPTY_SURFACES
    if "--plane-state" in sys.argv:
        files = PLANE_STATE
    if "--resource-blur" in sys.argv:
        files = ["desktop/src/image_resource_transform.rs", "desktop/src/image_resource_surface.rs"]
    for name in files:
        path = ROOT / "simulator/crates" / name
        raw = path.read_bytes()
        parsed = subprocess.run([str(formatter), "--edition", "2024", "--config",
            "skip_children=true", "--emit", "stdout"], input=raw, capture_output=True, check=False)
        rows.append({"path": path.relative_to(ROOT).as_posix(),
            "sha256": hashlib.sha256(raw).hexdigest(), "grammar_read": parsed.returncode == 0,
            "unchanged": raw == path.read_bytes(), "diagnostic": parsed.stderr.decode("utf8", errors="replace")})
    output = ROOT / ("archive/2026-11/20261117_format_B_image_copy_syntax.json"
        if "--current-batch" in sys.argv else "archive/2026-11/20261117_format_B_image_state_syntax.json")
    if "--empty-surfaces" in sys.argv:
        output = ROOT / "archive/2026-11/20261117_format_B_empty_surfaces_syntax.json"
    if "--plane-state" in sys.argv:
        output = ROOT / "archive/2026-11/20261117_format_B_plane_state_syntax.json"
    if "--resource-blur" in sys.argv:
        output = ROOT / "archive/2026-11/20261117_format_B_resource_blur_syntax.json"
    output.write_text(json.dumps({"compiled": False, "type_checked": False, "tests_run": False,
        "original_code_executed": False, "syntax": rows}, ensure_ascii=True, indent=2) + "\n", encoding="utf8")
    print(json.dumps({"output": output.relative_to(ROOT).as_posix(),
        "files": len(rows), "grammar_read": all(r["grammar_read"] for r in rows),
        "unchanged": all(r["unchanged"] for r in rows),
        "diagnostics": [r for r in rows if not r["grammar_read"]]}, ensure_ascii=True))

if __name__ == "__main__":
    main()

"""Record fixed-driver shared mesh nodes and the actual atlas-grid scope."""
import json

from recovery_A_20261116 import ROOT, Image, read_json, sha
from recovery_A_emote_model_render_20261117 import DRIVER_SHA, flatten
from recovery_A_psb_20261117 import Psb, load_sample
from recovery_A_receiver_contracts_20261116 import block


def main():
    identity = read_json(ROOT / "reports/recovery_A_20261116_emote.json")
    driver = Image((ROOT / identity["binary_path"]).parent / "emotedriver.dll", DRIVER_SHA)
    name, data = load_sample(0)
    root = Psb(data).root
    grids = []
    for obj_name, obj in root["object"].items():
        for motion_name, motion in obj["motion"].items():
            for index, layer in enumerate(flatten(motion.get("layer", []))):
                if layer.get("type", 2) != 0 or layer.get("meshTransform", 0) != 1:
                    continue
                content = next((f["content"] for f in layer.get("frameList", [])
                                if f.get("content") and f["content"].get("src") not in (None, "blank")), None)
                if content is None:
                    continue
                icon = root["source"][content["src"]]["icon"][content["icon"]]
                width, height, division = icon["width"], icon["height"], layer["meshDivision"]
                columns = division * width // (width + height) + 1
                rows = division - (columns - 1) + 1
                grids.append({"object": obj_name, "motion": motion_name, "index": index,
                              "label": layer.get("label"), "subdivisions": [columns, rows],
                              "previous_node_evaluations_per_draw": 4 * columns * rows,
                              "shared_node_evaluations_per_draw": (columns + 1) * (rows + 1)})
    spans = {"shared_vertex_dimensions_and_buffers": (0x5874c, 0x69),
             "shared_grid_materialization": (0x2f080, 0x8c),
             "row_strip_reads_shared_nodes": (0x58b44, 0x10b),
             "draw_strip_then_release_stack": (0x58c50, 0x66)}
    summary = {
        "batch": "actual mesh shared-node production -> existing color/mask triangles",
        "source": "CpuMeshGrid::raster_layout -> sprite frame-local XY/UV grid -> neighboring cell triangle reads -> real raster",
        "resource": name, "decoded_sha256": sha(data),
        "declared_atlas_sprite_grid_layers": len(grids),
        "scope": "static first-icon grid sizes at quality1; not active-frame counts or an execution benchmark",
        "previous_node_evaluations_for_these_grid_sizes": sum(v["previous_node_evaluations_per_draw"] for v in grids),
        "shared_node_evaluations_for_these_grid_sizes": sum(v["shared_node_evaluations_per_draw"] for v in grids),
        "lifetime": "bounded513x513 nodes at most per sprite invocation; buffer drops on return/error",
        "API": "CpuEmoteBackend::render_rgba; no host hook",
        "remaining": ["nonneutral/corner color and nonzero blend", "independent reference inheritance and general curve/projection/stencil variants", "full dynamic/pixel validation"],
        "completed_native_equivalence_added": 0, "full_NEKO_strict_pixel_claim": False,
        "compiled": False, "typechecked": False, "tests_run": False, "target_execution": False,
    }
    paths = ["simulator/crates/runtime/src/emote_cpu.rs", "simulator/crates/runtime/src/emote_mesh.rs",
             "scripts/analysis/recovery_A_emote_mesh_nodes_20261117.py"]
    small = {"summary": summary, "perPE": [{"sha256": driver.digest, "addresses": "RVA",
             "spans": {k: list(v) for k, v in spans.items()}}]}
    payload = {**small, "declared_resource_grids": grids,
               "evidence": {k: block(driver, *v) for k, v in spans.items()},
               "sources": [{"path": p, "sha256": sha((ROOT / p).read_bytes())} for p in paths]}
    prefix = ROOT / "reports/recovery_A_20261117_emote_mesh_nodes"
    for suffix, value in ((".json", payload), ("_summary.json", small)):
        prefix.with_name(prefix.name + suffix).write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    prefix.with_suffix(".md").write_text(
        "# A / 实际mesh共用顶点\n\n"
        "sprite每次绘制先生成有界XY/UV网格，每个节点只过一次祖先BP链，再给相邻cell的真实三角形复用。"
        "颜色与mask使用同一生产／消费接收器，buffer随该次调用结束／错误释放。"
        "5874c..587b5与2f080证明原版先加顶点边界、分配共用网格；58b44..58c24从共用节点读行strip。\n\n"
        f"{name}有{len(grids)}个声明了自身mesh格点的atlas sprite层；按首icon和quality1的纯结构尺寸，"
        f"每份该组网格的节点求值数从{summary['previous_node_evaluations_for_these_grid_sizes']}减为"
        f"{summary['shared_node_evaluations_for_these_grid_sizes']}，并非活动帧计数或运行基准。"
        "原API仍为render_rgba；没有增加宿主接线。剩余非中性／角颜色、独立reference与通用projection/curve/stencil；"
        "函数等价新增0，完整像素验收仍待统一执行。仅grammar读取，无编译／类型检查／测试／执行。\n",
        encoding="utf8")
    current_path = ROOT / "reports/recovery_A_20261117_emote_current_summary.json"
    current = read_json(current_path)
    current["mesh_shared_nodes_batch"] = small
    current["summary"]["latest_A_batch"] = summary
    report = prefix.with_suffix(".md").relative_to(ROOT).as_posix()
    if report not in current.setdefault("reports", []):
        current["reports"].append(report)
    for row in payload["sources"]:
        current["source_hashes_current"] = [v for v in current.get("source_hashes_current", []) if v["path"] != row["path"]]
        current["source_hashes_current"].append(row)
    current_path.write_text(json.dumps(current, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    rolling_path = ROOT / "reports/recovery_A_20261117_rolling_delivery.md"
    rolling = rolling_path.read_text(encoding="utf8")
    note = "随后已接`recovery_A_20261117_emote_mesh_nodes.md`：真实sprite/BP/mask接收器复用帧内网格XY/UV节点，每节点只变形一次，buffer随调用释放；这是实际像素生产复用，整函数等价新增0。\n\n"
    if note not in rolling:
        rolling_path.write_text(rolling.replace("# A / 本轮小批交付\n\n", "# A / 本轮小批交付\n\n" + note, 1), encoding="utf8")
    print(json.dumps(summary, ensure_ascii=True), flush=True)


if __name__ == "__main__":
    main()

"""One fixed non-ca01 PSB declaration probe; no sampling or pixel execution."""
import argparse
from collections import Counter
import json

from recovery_A_20261116 import ROOT, read_json, sha
from recovery_A_emote_actual_draw_20261117 import inspect
from recovery_A_emote_model_render_20261117 import flatten
from recovery_A_psb_20261117 import Psb, load_sample

OUT = ROOT / "archive/2026-11/20261117_emote_cb01_body1.json"
PREFIX = ROOT / "reports/recovery_A_20261117_emote_cb01_body1"
NAME = "cb01_l_body_1.psz"
IDENTITY = "0a224bc34b60143b49be7470fe336c419e5ff3ba526ead9aed27a0bf6cd9c47b"


def probe():
    name, raw = load_sample(27)
    if name != NAME or sha(raw) != IDENTITY:
        raise ValueError("fixed cb01 member identity changed")
    psb = Psb(raw)
    data = {"resource": name, "decoded_sha256": sha(raw),
            "scope": "single fixed non-ca01 model; declarations only",
            "declarations": inspect(psb.root), "root": psb.root,
            "chunk_lengths": psb.lengths, "extra_chunk_lengths": psb.extra_lengths}
    OUT.write_text(json.dumps(data, ensure_ascii=False, indent=2) + "\n", encoding="utf8")


def summary():
    data = read_json(OUT)
    root = data["root"]
    metadata = root.get("metadata", {})
    enabled = {key: [entry for entry in value
                    if isinstance(entry, dict) and entry.get("enabled") != 0]
               for key, value in metadata.items()
               if key.endswith("Control") and key != "timelineControl" and isinstance(value, list)}
    kinds = Counter()
    meshes = Counter()
    for obj in root["object"].values():
        for motion in obj["motion"].values():
            for layer in flatten(motion.get("layer", [])):
                kinds[layer.get("type", 2)] += 1
                if layer.get("stencilType", 0):
                    meshes["stencil_" + str(layer["stencilType"])] += 1
                for frame in layer.get("frameList", []):
                    content = frame.get("content")
                    if isinstance(content, dict):
                        for key in content:
                            meshes["content_" + key] += 1
    textures = [{"source": key, "texture": value.get("texture")}
                for key, value in root.get("source", {}).items() if "texture" in value]
    instant = set(metadata.get("instantVariableList", []))
    immediate = {entry["label"] for section in ("selectorControl", "eyeControl", "eyebrowControl", "mouthControl")
                 for entry in enabled.get(section, [])}
    missing_instant = [{"timeline": timeline["label"], "variable": track["label"]}
                       for timeline in metadata.get("timelineControl", [])
                       for track in timeline.get("variableList", [])
                       if track.get("frameList") and track["label"] in instant
                       and track["label"] not in immediate]
    return {"resource": data["resource"], "decoded_sha256": data["decoded_sha256"],
        "declarations": data["declarations"], "base": metadata.get("base"),
        "enabled_metadata_counts": {key: len(value) for key, value in enabled.items()}, "layers": dict(kinds),
        "fields": dict(meshes), "textures": textures, "chunk_lengths": data["chunk_lengths"],
        "instant_variables": sorted(instant), "instant_receiver_missing": missing_instant,
        "enabled_eye_auto_blink": sum(entry.get("blinkEnabled", 0) != 0 for entry in enabled.get("eyeControl", [])),
        "strict_metadata_hits": {key: len(enabled.get(key, [])) for key in (
            "bustControl", "hairControl", "loopControl", "mirrorControl", "partsControl", "stereovisionControl")
            if enabled.get(key)},
        "eye_interval_declaration_hits": sum(not (entry.get("beginFrame", 0) < entry.get("endFrame", 0))
            for entry in enabled.get("eyeControl", []))}


def report(data):
    counts = data["declarations"]["counts"]
    summary_data = {
        "scope": "one fixed different-character/body model; declaration predicates only",
        "resource": NAME, "decoded_sha256": IDENTITY,
        "fixed_members_newly_parsed": 1, "counts": counts,
        "declared_draw_guard_hits": data["declarations"]["guard_hits"],
        "enabled_strict_metadata_hits": data["strict_metadata_hits"],
        "enabled_eye_auto_blink": data["enabled_eye_auto_blink"],
        "eye_interval_declaration_hits": data["eye_interval_declaration_hits"],
        "nonempty_instant_tracks_without_receiver": len(data["instant_receiver_missing"]),
        "composite_owners_reuse_existing_consumer": data["fields"].get("stencil_5", 0),
        "texture": {"platform": "win", "format": "RGBA8", "dimensions": [4096, 4096],
            "truncated_dimensions": [4093, 2128], "chunk_bytes": data["chunk_lengths"][0],
            "full_raw_RGBA_chunk_present": data["chunk_lengths"] == [4096 * 4096 * 4],
            "new_crop_decoder_required_by_this_input": False},
        "new_runtime_source_consumers": 0, "removed_guards": 0,
        "completed_native_equivalence_added": 0, "main_hook_required": None,
        "strict_draw_proven": False, "compiled": False, "typechecked": False,
        "tests_run": False, "evaluator_or_renderer_executed": False,
        "PE_VM_game_APK_device_executed": False,
        "new_PE_read": False, "full_game_or135_model_scan": False,
        "result": "no new unsupported declared family found in the selected model; keep all diagnostics",
    }
    per_pe = [{"sha256": "85eb459e94e74ab608f649eb7fb440f2526e90aaa1af7276ed01dc10018903ce",
        "role": "existing exact NEKO driver contract only; no fresh PE read",
        "reused_contracts": {"44b39/44e2a/44f69/4501b": "same-sample capture/shared composite owner range",
                             "50d0..52b4": "existing textured primitive stencil consumer"}}]
    source_paths = ["simulator/crates/runtime/src/emote_cpu.rs", "simulator/crates/runtime/src/emote_texture.rs",
        "simulator/crates/runtime/src/emote_m2selector.rs", "simulator/crates/runtime/src/emote_variable_timeline.rs",
        "scripts/analysis/recovery_A_emote_next_model_20261117.py"]
    remaining = [
        "These are resource declarations, not evaluated draw or controller states; they cannot establish a full strict frame.",
        "No concrete new rejected producer was located. Dynamic reference ownership, active graph/controller targets and numeric/capacity paths are outside this static check.",
        "The selected cb01 resource does not establish coverage of all14profiles or all135PSZ resources.",
        "No additional PE contract/receiver is needed without a concrete rejected model field or owner/command state; do not add a generic cc/skin/type12 or automatic-blink implementation from zero hits.",
    ]
    payload = {"summary": summary_data, "perPE": per_pe, "declarations": data,
        "remaining": remaining, "cache": {"path": OUT.relative_to(ROOT).as_posix(), "sha256": sha(OUT.read_bytes())},
        "source_identities_only_not_execution": [{"path": path, "sha256": sha((ROOT / path).read_bytes())}
                                                for path in source_paths]}
    for suffix, value in ((".json", payload), ("_summary.json", {"summary": summary_data, "perPE": per_pe})):
        PREFIX.with_name(PREFIX.name + suffix).write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    PREFIX.with_suffix(".md").write_text(
        "# A / 非ca01模型定点结论\n\n"
        f"只新增读取psz.int第27项 `{NAME}`，解包PSB SHA-256 `{IDENTITY}`；"
        f"{counts['motions']} motions、{counts['layers']}层、{counts['frame_records']}帧记录、"
        f"{counts['reference_records']}引用记录、{counts['timeline_tracks']}声明timeline tracks。"
        "保留完整AST缓存，报告模式只读缓存；未重扫20游戏或135模型。\n\n"
        "既有26项draw/evaluator声明guard命中0；非空cc、非neutral颜色、非零bm、textured type12及"
        "reference flags/loop/offset均无命中。6个type12是blank composite owner，复用已接受的共享range消费者；"
        "未把它们冒认新的textured type12输入。metadata明确拒绝的6类无enabled记录，"
        "eye自动blink=0且区间0..10；8个instant名称的非空track均有既有receiver。\n\n"
        "一个定点候选是tex.texture的truncated_width=4093、truncated_height=2128。"
        "但width/height=4096且chunk长度=67108864，恰为完整原始RGBA8尺寸；当前win/RGBA8消费者已接。"
        "本输入没有证明需要新增裁剪／RLE消费者，故未扩实现。\n\n"
        "**本批运行时源码新增0、移除guard0、完整native等价新增0；无需main hook。**"
        "源码保留所有严格诊断。未执行采样、求值或绘制，不能声称该模型strict通过或完整像素兼容；"
        "仍缺具体动态owner/命令状态或另一实际拒绝字段，才能选择下一个有证据consumer，不能从声明0命中虚构实现。"
        "复用已有SHA85eb459e…93ce的44b39/44e2a/44f69/4501b共享range与50d0..52b4 stencil合同，未重新读PE。\n\n"
        "分析脚本归scripts/analysis；完整结构缓存归archive/2026-11/20261117_emote_cb01_body1.json，"
        "短报告/summary归reports。未新增目录，未改README／译文／游戏；无编译、类型检查、测试、"
        "Rust/求值/渲染/PE/VM/游戏/APK/设备执行。\n", encoding="utf8")
    current_path = ROOT / "reports/recovery_A_20261117_emote_current_summary.json"
    current = read_json(current_path)
    current["summary"]["latest_distinct_character_model"] = {
        "report": PREFIX.with_name(PREFIX.name + "_summary.json").relative_to(ROOT).as_posix(),
        "resource": NAME, "new_runtime_consumers": 0, "declared_guard_hits": 0,
        "new_hook": None, "strict_frame_proven": False}
    current_path.write_text(json.dumps(current, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    print(json.dumps(summary_data, ensure_ascii=True))


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--probe", action="store_true")
    parser.add_argument("--report", action="store_true")
    args = parser.parse_args()
    if args.probe:
        probe()
    data = summary()
    if args.report:
        report(data)
    else:
        print(json.dumps(data, ensure_ascii=True))

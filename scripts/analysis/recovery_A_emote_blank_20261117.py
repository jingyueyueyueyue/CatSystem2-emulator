"""Fixed-driver blank descriptor and empty-mask semantics; static only."""
import json

from recovery_A_20261116 import ROOT, Image, read_json, sha
from recovery_A_receiver_contracts_20261116 import block


def main():
    identity = read_json(ROOT / "reports/recovery_A_20261116_emote.json")
    driver = Image((ROOT / identity["binary_path"]).parent / "emotedriver.dll",
                   "85eb459e94e74ab608f649eb7fb440f2526e90aaa1af7276ed01dc10018903ce")
    if driver.get(0x7d3ec, 6) != b"blank\0" or driver.get(0x7d3f4, 2) != b":\0":
        raise ValueError("driver blank descriptor identity changed")
    spans = {
        "blank_name_compare_and_split": (0x38ef0, 0x85),
        "blank_valid_kind1_integer_dimensions_float_origins": (0x38f75, 0x66),
        "blank_kind_to_draw_primitive": (0x44b83, 0x2c),
        "ordinary_color_skips_nonzero_primitive_kind": (0x4990, 0x10),
        "composite_mask_skips_nonzero_primitive_kind": (0x50d0, 0xf),
        "composite_skip_advances_to_next_source": (0x52b4, 0x15),
        "own_mask_skips_nonzero_primitive_kind": (0x540b, 0x9),
    }
    paths = ["simulator/crates/runtime/src/emote_cpu.rs",
             "scripts/analysis/recovery_A_emote_blank_20261117.py"]
    summary = {
        "blank_descriptor_width_height_are_i32": True,
        "blank_descriptor_origin_is_f32": True,
        "blank_kind1_produces_no_color_or_mask_primitive": True,
        "blank_BPs_still_transform_actual_descendants": True,
        "positive_blank_mask_keeps_empty_linked_plane_and_rejects_color": True,
        "negative_blank_mask_keeps_linked_reference_and_does_not_decrement": True,
        "blank_mask_no_longer_drops_entire_clip": True,
        "opaque_rectangle_or_white_texture_substitution": False,
        "closed_subbranches": ["stencil_scheduler/blank-source-kind1", "mesh_hierarchy/blank-descriptor-width-origin-storage"],
        "entire_families_closed": 0,
        "remaining_declared_family_count": 9,
        "primary_pairs_inherited": 102, "primary_blocked_pairs_inherited": 0,
        "timed_labels_pending_final_runtime_validation": 60,
        "complete_native_equivalence_count_added": 0,
        "compiled": False, "tests_run": False, "Rust_VM_game_DLL_execution": False,
        "full_NEKO_pixel_claim": False,
    }
    missing = ["kind3 itself owning a mask and captured primitive ranges",
               "nested source mask schedule and source2cc ordinary color/range lifecycle",
               "group variants other than confirmed blank kind1 and mode1 target/projection/blend/composition",
               "malformed blank descriptors accepted by original CRT prefix conversions remain explicit errors"]
    payload = {"schema": "cs2-recovery-A-emote-blank/v1", "summary": summary,
               "driver": {"sha256": driver.digest, "addresses": "RVA"},
               "actual_receiver": "RenderContext::stencil_source_motion/literal_stencil_clip/mesh_dimensions",
               "pixel_consequence": "preserve actual linked empty mask and final EQUAL test; other textured triangles remain the pixel producer",
               "remaining_specific_branches": missing,
               "evidence": {key: block(driver, *span) for key, span in spans.items()},
               "sources": [{"path": p, "sha256": sha((ROOT / p).read_bytes())} for p in paths]}
    prefix = ROOT / "reports/recovery_A_20261117_emote_blank"
    prefix.with_suffix(".json").write_text(json.dumps(payload, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    small = {k: v for k, v in payload.items() if k != "evidence"}
    prefix.with_name(prefix.name + "_summary.json").write_text(json.dumps(small, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    prefix.with_suffix(".md").write_text(
        "# A / Emote blank图元与空mask接收链\n\n"
        "本批直接修改emote_cpu.rs。38f75把blank descriptor写成valid1/kind1；44b96把kind复制到draw primitive首字节。"
        "4997的普通颜色、50d6的composite source、540b的own-mask均跳过非零kind。因此blank不能替代为白纹理或不透明矩形。\n\n"
        "literal_stencil_clip现在跳过blank实际源，但保留原linked plane/reference，不再返回None丢掉整张clip："
        "positive空mask仍使实际color三角形不通过EQUAL；negative空mask不递减原reference。"
        "stencil_source_motion删除同一已恢复分支的警告。blank BP仍用于后代真实mesh几何；宽高按i32、origin按f32存储量化。\n\n"
        "没有新增宿主API；CpuEmoteBackend::render_rgba沿用现有严格像素链。"
        "仍缺kind3自身owner范围、nested source mask、source2cc普通color/range、其他group和mode1。"
        "闭合2个子分支，整个声明族新增闭合0，现有9族仍在；primary102/102与pair阻断0沿用，60timed待最终统一验证，完整native新增0。\n\n"
        "仅rustfmt语法读取与静态PE分析；未编译、测试、运行Rust/VM/游戏/DLL/APK。"
        "脚本归scripts/analysis，报告归reports，无新目录、临时游戏产物或根README修改。\n", encoding="utf-8")
    print(json.dumps(summary))


if __name__ == "__main__":
    main()

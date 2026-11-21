"""NEKO composite source range -> real textured mask -> color EQUAL, static only."""
import json

from recovery_A_20261116 import ROOT, Image, read_json, sha
from recovery_A_psb_20261117 import Psb, load_sample
from recovery_A_emote_model_render_20261117 import DRIVER_SHA, inventory
from recovery_A_receiver_contracts_20261116 import block


def main():
    identity = read_json(ROOT / "reports/recovery_A_20261116_emote.json")
    driver = Image((ROOT / identity["binary_path"]).parent / "emotedriver.dll", DRIVER_SHA)
    if driver.get(0x7cbd0, 16) != b"\xff" * 16:
        raise ValueError("mask vertex color identity changed")
    name, data = load_sample(0)
    observed = inventory(Psb(data).root)
    owners = observed["stencil_owners"]
    spans = {
        "source_kind_filter_and_membership": (0x3a655, 0x7b),
        "source_group_captures_real_primitive_range": (0x44ace, 0xa3),
        "ordinary_sprite_also_inserted_in_color_list": (0x44dd8, 0x52),
        "composite_own_first_and_source_range_flatten": (0x44f0f, 0x1b2),
        "mask_list_ignores_source_parent_stencil_link": (0x50d0, 0xf),
        "mask_uses_opaque_white_vertex_colors": (0x526a, 0x42),
        "mask_source_loop_next": (0x52b4, 0x15),
        "active_owner_geometry_gate": (0x402cd, 0x8c),
        "final_color_equal_reference": (0x55d5, 0x32),
    }
    summary = {
        "batch": "composite captured source ranges and texture-only mask alpha",
        "resource": name, "resource_sha256": sha(data),
        "actual_composite_owners": len(owners),
        "actual_composite_types": sorted({v["stencil_type"] for v in owners}),
        "owners_with_direct_sprite_sources": sum(any(s["kind"] == 0 for s in v["sources"]) for v in owners),
        "owners_with_captured_motion_sources": sum(any(s["kind"] == 3 for s in v["sources"]) for v in owners),
        "unresolved_source_labels_skipped_as_original": sum(s["index"] is None for v in owners for s in v["sources"]),
        "source_membership_does_not_suppress_ordinary_color": True,
        "source_group_captured_range_flattens_local_owner_descendants": True,
        "mask_source_local_color_stencil_not_reapplied": True,
        "mask_texture_alpha_not_multiplied_by_layer_opacity_or_color": True,
        "composite_blank_owner_does_not_emit_opaque_rectangle": True,
        "blanket_stencil_warning_replaced_by_specific_unimplemented_variants": True,
        "pipeline": "sampled source motion -> real atlas/mesh triangles -> texture-alpha GREATER64 + linked INCR/DECR -> color EQUAL -> existing CPU RGBA",
        "remaining_specific_receivers": [
            "ordinary kind3 reference itself owning a mask / group join",
            "active own BP stencil activation",
            "textured type12 target/reversal state",
            "mode1 separate targets and final composition",
            "nonneutral color inheritance / corner colors",
            "per-frame mask flags and remaining enabled controller composition still under audit",
        ],
        "remaining_declared_emote_families": 9,
        "newly_closed_entire_families": 0,
        "completed_native_equivalence_added": 0, "full_NEKO_strict_pixel_claim": False,
        "compiled": False, "tests_run": False, "Rust_VM_game_DLL_APK_executed": False,
        "syntax_reading": "rustfmt parser only; not typecheck or execution",
    }
    paths = ["simulator/crates/runtime/src/emote_cpu.rs",
             "scripts/analysis/recovery_A_emote_composite_ranges_20261117.py"]
    payload = {"summary": summary,
               "perPE": [{"path": str(driver.path.relative_to(ROOT)), "sha256": driver.digest,
                          "addresses": "RVA", "spans": {k: [a, n] for k, (a, n) in spans.items()}}],
               "actual_owners": owners,
               "evidence": {key: block(driver, *span) for key, span in spans.items()},
               "sources": [{"path": p, "sha256": sha((ROOT / p).read_bytes())} for p in paths]}
    prefix = ROOT / "reports/recovery_A_20261117_emote_composite_ranges"
    prefix.with_suffix(".json").write_text(json.dumps(payload, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    prefix.with_name(prefix.name + "_summary.json").write_text(
        json.dumps({k: payload[k] for k in ("summary", "perPE")}, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    prefix.with_suffix(".md").write_text(
        "# A / 实际composite源范围到CPU纹理像素\n\n"
        "修改仅emote_cpu.rs的layer/stencil_source_motion/sprite，未触碰main surface/import/render/advance。"
        f"{name}共{len(owners)}个type12/type5 owner：3个使用普通sprite源，3个捕获motion源；"
        "2个找不到的label按原版忽略。\n\n"
        "driver SHA85eb459e…93ce：3a6c3标2cc；44ae6..44b50因该标记捕获type3的实际子primitive范围。"
        "44e0b仍把0/10普通primitive插入颜色列表，故source不能被整层隐藏。"
        "44f0f..450ae先建立owner自己的范围，再将source3捕获范围展开、source0追加。"
        "50d0..52b4只遍历范围内kind0图元，不跟各源primitive+78的颜色stencil链；"
        "因此source motion中的local owner后代应贡献实际纹理alpha，而不是再次按其颜色mask裁切。\n\n"
        "本批删除错误的nested-source通用诊断，沿原priority递归实际源纹理/网格。"
        "52a1固定四个white DWORD7cbd0，所以mask pass现在不乘layer opacity/color或普通blend；"
        "仍由真实texture alpha作GREATER64、linked INCR/DECR，最终颜色EQUAL。"
        "删除全体stencil无条件拒绝，改保留kind3自身owner、active own BP、textured type12和mode1具体缺口；"
        "blank owner不画替代矩形。source普通颜色与mask仍各走真实接收路径。\n\n"
        "这闭合了实际六个composite owner使用的范围/alpha子链源码，并非完整NEKO运行验收。"
        "per-frame mask flag和enabled controller组合继续审查；9个声明族仍partial，完整native等价新增0。"
        "跨资源fade仍归main且未宣称完成。\n\n"
        "只读PE/PSB分析与rustfmt语法读取；无编译、测试、Rust/VM/游戏/DLL/APK执行。"
        "脚本归scripts/analysis，报告归reports；无临时产物、新目录或根README/localization修改。\n", encoding="utf-8")
    print(json.dumps(summary))


if __name__ == "__main__":
    main()

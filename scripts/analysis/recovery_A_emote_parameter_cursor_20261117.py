"""Static parameter cursor PE contract and model field inventory."""
import json
from collections import Counter

from recovery_A_20261116 import ROOT, Image, read_json, sha
from recovery_A_receiver_contracts_20261116 import block
from recovery_A_psb_20261117 import Psb, load_sample


def main():
    identity = read_json(ROOT / "reports/recovery_A_20261116_emote.json")
    driver = Image((ROOT / identity["binary_path"]).parent / "emotedriver.dll",
                   "85eb459e94e74ab608f649eb7fb440f2526e90aaa1af7276ed01dc10018903ce")
    name, data = load_sample()
    document = Psb(data)
    # Format parsing only; no timeline, function, VM or renderer execution.
    counts = Counter()
    for obj in document.root["object"].values():
        for motion in obj["motion"].values():
            for p in motion.get("parameter", []):
                counts["parameters"] += 1
                counts["discretization_true"] += bool(p.get("discretization"))
                begin, end, division = (p.get(k, 0) for k in ("rangeBegin", "rangeEnd", "division"))
                counts["descending"] += begin > end
                counts["equal_range"] += begin == end
                counts["nonpositive_division"] += division <= 0
    summary = {
        "parameter_initial_and_live_write_cursor_in_source": True,
        "discrete_truncates_raw_variable_before_clamp": True,
        "CVTTSS2SI_out_of_range_returns_i32_MIN": True,
        "ascending_and_descending_ranges": True,
        "equal_range_or_nonpositive_division_returns_original_zero_cursor": True,
        "formula_order": "f32((clamp(discrete?cvttss2si(value):value,min(begin,end),max(begin,end))-begin)*division)/(end-begin)",
        "removed_strict_warning": "parameter discretization is not recovered",
        "closed_subbranches": ["PSB parameter/discretization", "PSB parameter/reversed-and-degenerate-range"],
        "entire_declared_families_closed": 0,
        "remaining_declared_families": 9,
        "complete_native_equivalence_count_added": 0,
        "primary_pairs_inherited": 102, "primary_blocked_pairs_inherited": 0,
        "timed_labels_pending_final_runtime_validation": 60,
        "compiled": False, "tests_run": False, "Rust_VM_game_DLL_execution": False,
        "full_NEKO_pixel_claim": False,
    }
    paths = ["simulator/crates/runtime/src/emote_cpu.rs",
             "scripts/analysis/recovery_A_emote_parameter_cursor_20261117.py"]
    payload = {"schema": "cs2-recovery-A-emote-parameter-cursor/v1", "summary": summary,
               "driver": {"sha256": driver.digest, "addresses": "RVA"},
               "source_receiver": "RenderContext::layer -> parameter_cursor -> sample_emote_layer -> actual geometry/color/mask raster",
               "resource_inventory": {"resource": name, "PSB_sha256": sha(data), "counts": dict(counts)},
               "evidence": {"initial_cursor": block(driver, 0x3279d, 0xa3),
                            "live_variable_write_cursor": block(driver, 0x33110, 0xe5)},
               "remaining": ["default variable producer/resolution parity is unchanged", "nonfinite inputs retain explicit diagnostics", "all nine earlier receiver families remain partial"],
               "sources": [{"path": p, "sha256": sha((ROOT / p).read_bytes())} for p in paths]}
    prefix = ROOT / "reports/recovery_A_20261117_emote_parameter_cursor"
    prefix.with_suffix(".json").write_text(json.dumps(payload, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    prefix.with_name(prefix.name + "_summary.json").write_text(json.dumps({k:v for k,v in payload.items() if k != "evidence"}, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    prefix.with_suffix(".md").write_text(
        "# A / Emote实际parameter cursor\n\n"
        "emote_cpu.rs新增parameter_cursor并接到RenderContext::layer实际采样入口。"
        "原3279d..32839初始化与33140..331ee变量写后重算使用同一规则："
        "discretization先对原始变量CVTTSS2SI向零截断，再夹到两个endpoint之间；"
        "随后按f32减begin、乘division、除(end-begin)的顺序映射到frame。"
        "descending range保留方向，equal range或division<=0返回原0 cursor。"
        "先前的discretization严格警告已删除。\n\n"
        "这是直接影响真实texture/mesh/stencil采样的接收者实现，没有新宿主API。"
        "其余默认变量生产者和非finite分支不宣称闭合，9个声明族保持partial。"
        "完整native新增0；primary102/102、阻断0沿用；60timed最终统一验证。\n\n"
        "固定driver SHA256=" + driver.digest + "。JSON包含精确原字节、RVA、源码哈希及实际首模型parameter字段静态计数。"
        "仅rustfmt读取语法、Python解析资源与PE；未编译、测试或执行Rust/VM/函数/游戏/DLL/APK。"
        "功能脚本分类scripts/analysis，报告归reports，无新增临时文件或目录。\n", encoding="utf-8")
    print(json.dumps({"summary": summary, "resource_counts": dict(counts)}))


if __name__ == "__main__":
    main()

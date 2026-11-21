"""Report retained M2 spline playback from bounded PE/KCS caches and source text."""
import hashlib
import json

from recovery_A_20261116 import ROOT, read_json


PREFIX = ROOT / "reports/recovery_A_20261118_emote_spline_lifetime"
SPLINE = "simulator/crates/runtime/src/emote_m2spline.rs"
CPU = "simulator/crates/runtime/src/emote_cpu.rs"
LIVE = "simulator/crates/runtime/src/emote_m2live.rs"
CACHE = "archive/2026-11/20261118_emote_spline_lifetime.json"


def location(path, source, anchor):
    at = source.find(anchor)
    return {"path": path, "anchor": anchor,
            "line": None if at < 0 else source[:at].count("\n") + 1}


def main():
    prior = read_json(ROOT / "reports/emote_spline_contracts_20261117.json")
    bounded = read_json(ROOT / CACHE)
    if bounded["PE_sha256"] != prior["pe_sha256"]:
        raise ValueError("spline evidence PE mismatch")
    sources = {path: (ROOT / path).read_text(encoding="utf8")
               for path in (SPLINE, CPU, LIVE)}
    spline, cpu, live = (sources[path] for path in (SPLINE, CPU, LIVE))
    import_body = cpu.split("    fn import_m2_resource(", 1)[1].split("    fn ", 1)[0]
    inherit_at = import_body.find("owner.inherit_spline_owner(previous)")
    bind_at = import_body.find("owner.spline.config = load.spline_config")
    observed = {
        "dedicated_inherit_receiver": "fn inherit_spline_owner(" in spline,
        "copies_label_enable_first_origin_wrote_and_shared_config":
            "self.spline = previous.spline.clone();" in spline,
        "CPU_import_inherits_before_config_rebind": 0 <= inherit_at < bind_at,
        "CPU_rebinds_actual_manager_config": bind_at >= 0,
        "advance_uses_current_material_and_force_owners":
            "self.spline.advance(tick, &self.materials, &mut self.force)?" in live,
        "advance_preserves_original_origin":
            "if self.first { self.origin = tick; self.first = false; }" in spline
            and "tick.wrapping_sub(self.origin)" in spline,
    }
    connected = all(observed.values())
    staged = cpu.split("    fn apply_spline_request(", 1)[1].split("    fn tint_argb(", 1)[0]
    playback = spline.split("    pub(super) fn inherit_playback_from(", 1)[1].split(
        "    pub fn advance(", 1)[0]
    first_owner = import_body.split("        if previous_owner.is_none() {", 1)[1].split(
        "        CpuM2TimedVariables::validate(", 1)[0]
    command = cpu.split("EmoteOperation::SplineLabel => {", 1)[1].split(
        "EmoteOperation::FinishPending", 1)[0]
    preload_observed = {
        "registered_wrapper_setter_is_connected": "state.apply_spline_request(name)" in command,
        "empty_wrapper_stages_owned_spline_state": "else if self.player.is_none()" in staged
            and "self.unloaded_spline.get_or_insert_with(Default::default)" in staged,
        "same_PE_start_stop_receivers": "spline.start(label)" in staged and "spline.stop()" in staged,
        "no_preload_sampling": "advance(" not in staged,
        "first_owner_consumer_connected": "registered.apply_unloaded_spline_request(&mut owner)" in first_owner,
        "consumer_copies_playback_only": "owner.spline.inherit_playback_from(spline)" in staged
            and "self.label.clone_from(&previous.label)" in playback
            and all("self." + name + " = previous." + name in playback
                    for name in ("enabled", "first", "origin", "wrote"))
            and "self.config" not in playback,
        "actual_config_bound_before_first_owner_consumer":
            0 <= bind_at < import_body.find("registered.apply_unloaded_spline_request(&mut owner)"),
        "staging_cleared_only_after_publish":
            0 <= import_body.find("state.m2_owner = Some(owner)")
            < import_body.find("state.unloaded_spline = None"),
        "task_release_drops_staged_storage": ".remove(&task)" in cpu,
    }
    preload_connected = all(preload_observed.values())
    hook = ("CPU import_m2_resource: in the existing previous_owner branch, immediately "
            "after owner.inherit_program_owner(previous), add "
            "owner.inherit_spline_owner(previous); before the existing "
            "owner.spline.config = load.spline_config binding. Do not start the curve again.")
    actual_sites = [
        {"resource": "sscript.kcs", "offset": 721713, "native": 929, "argument_bytes": 8},
        {"resource": "sscript.kcs", "offset": 806408, "native": 929, "argument_bytes": 8},
        {"resource": "sscript.kcs", "offset": 806653, "native": 930, "argument_bytes": 4},
    ]
    cached_sites = {(row["offset"], row["operands"][0]["value"])
                    for rows in bounded["windows"].values() for row in rows
                    if row["opcode"] == 43}
    if any((site["offset"], site["native"]) not in cached_sites for site in actual_sites):
        raise ValueError("bounded KCS site missing")
    summary = {
        "batch": "929/930 empty wrapper -> first real owner -> retained spline across PSB replacement",
        "actual_static_sites": actual_sites,
        "runtime_hits_or_cross_resource_overlap_measured": False,
        "dynamic_label_equals_spline_inf_test_proven": False,
        "preload_branch_runtime_hits_measured": False,
        "source_observed": observed,
        "preload_source_observed": preload_observed,
        "new_shared_lifecycle_receivers": 2,
        "new_complete_source_chains": int(connected) + int(preload_connected),
        "main_hooks_pending": ([] if connected else [hook]) + ([] if preload_connected else [
            "929/930 first-owner source chain has an unobserved component; see preload_source_observed"]),
        "remaining": [
            "929 dynamic labels and overlap with resource replacement are not runtime measured",
            "complete M2 graph save/load and multi-player manager equivalence are separate",
        ],
        "full_native_or_original_function_equivalence_added": 0,
        "new_resource_or_full_game_scan": False,
        "compiled": False, "typechecked": False, "grammar_reader_run": False,
        "tests_run": False, "target_or_evaluator_executed": False,
    }
    contracts = {
        "170e02..170e46": "constructor stores config/material/accumulator dependencies into a separate owner at wrapper+fc",
        "174d40..174d80": "start copies label; sets enable24/first28; clears origin2c/wrote30",
        "174d90..174db6": "stop clears label/enable only",
        "23906b..2390b8/2390cd..2390fe": "543/544 resolve selector0 wrapper then start/stop without a loaded-driver check",
        "173ef7..173efe": "no config60 skips update before any spline sampling; preload origin is not consumed",
        "175900..17611c": "replacement releases driver+4 and source+8, rebuilds using the same wrapper; does not recreate spline+fc",
        "173ef7..173fb4": "spring then eligible spline before common driver target submission",
        "185ba0..185cf2": "first eligible tick captures origin; curve uses current material output scale and shared accumulator",
    }
    per_pe = [{"sha256": bounded["PE_sha256"], "addresses": "RVA", "contracts": contracts}]
    payload = {
        "summary": summary, "perPE": per_pe,
        "sources": [location(SPLINE, spline, "    pub(in crate::emote) fn inherit_spline_owner("),
                    location(CPU, cpu, "owner.inherit_spline_owner(previous)"),
                    location(CPU, cpu, "    fn apply_spline_request("),
                    location(CPU, cpu, "    fn apply_unloaded_spline_request("),
                    location(CPU, cpu, "registered.apply_unloaded_spline_request(&mut owner)"),
                    location(SPLINE, spline, "    pub(super) fn inherit_playback_from("),
                    location(LIVE, live, "self.spline.advance(tick, &self.materials, &mut self.force)?")],
        "source_sha256": {path: hashlib.sha256((ROOT / path).read_bytes()).hexdigest()
                          for path in sources},
        "evidence": {"bounded_PE_and_KCS": CACHE,
                     "reused_spline_contracts": "reports/emote_spline_contracts_20261117.json",
                     "KCS_sha256": bounded["KCS_sha256"],
                     "reused_PE_block_sha256": {key: prior["evidence"][key]["sha256"] for key in (
                         "command543_lookup_and_label", "command544_lookup_and_stop",
                         "wrapper_fc_three_real_dependencies", "label_start_and_stop",
                         "update_spring_then_optional_spline_then_accumulator")},
                     "replacement_body_sha256": bounded["PE"]["0x175900"]["raw_sha256"],
                     "replacement_tail_sha256": bounded["PE"]["0x1760ff"]["raw_sha256"]},
    }
    for suffix, value in ((".json", payload), ("_summary.json", {"summary": summary, "perPE": per_pe})):
        PREFIX.with_name(PREFIX.name + suffix).write_text(
            json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    status = ("主线单行接点已在当前CPU静态观察，未运行验证。" if connected else
              "待主线一个接点：CPU import既有previous_owner分支，在inherit_program_owner之后"
              "调用owner.inherit_spline_owner(previous)；保留其后实际load.spline_config绑定。"
              "当前不记完整源码闭链。")
    PREFIX.with_suffix(".md").write_text(
        "# A / spline首次装载与跨PSB替换生命周期\n\n"
        "NEKO sscript已有929两处721713/806408、930一处806653，分别8/8/4bytes。"
        "动态标签来自参数/记录；没有证明它等于spline.inf唯一test标签，也未测替换期间交叉命中。\n\n"
        "固定PE c71c5b85…1adc：170e02..170e46把配置、材质、累加器依赖放入独立wrapper+fc；"
        "174d40只在显式start重置标签、enable24、first28、origin2c、wrote30；"
        "174d90只停标签/enable。175900..17611c重建的是driver+4及source+8，保留wrapper和+fc。"
        "CPU原先重建live owner时继承action/program，却给spline空播放状态，导致曲线丢失。\n\n"
        "新增emote_m2spline.rs::CpuM2LiveOwner::inherit_spline_owner(&mut self, previous:&Self)，"
        "复制真实标签/启停/首更/起点/写入状态及共享配置引用，不调用start、不重置时钟。"
        "后续现有173ef7→185ba0共享核仍读取新material/force所有者，再提交driver变量和PSB像素；"
        "没有旧driver指针，最终任务释放沿既有所有权回收。\n\n"
        f"{status}\n\n"
        "续批已自接首次owner完整链：23906b/2390cd查selector0后直接调用174d40/174d90，"
        "没有driver门控；173ef7在无实际配置60时跳过更新。CpuTask现用unloaded_spline保存"
        "拥有标签的同一start/stop状态，包含start→stop保留的first/origin/wrote。"
        "首次import在真实load.spline_config绑定后调用apply_unloaded_spline_request，"
        "inherit_playback_from只复制五个播放字段，保留新配置；验证或装载失败仍保留请求，"
        "成功publish后清临时快照，后续走已接live继承，释放任务一并释放。"
        f"首次owner链静态观察={preload_connected}，无需主线新hook；没有提前采样或创造player/变量。\n\n"
        "仍缺：完整存档、多player等价另列；929动态标签/装载前分支尚无运行测量。"
        "两批合计新增共享生命周期接收器2，完整native/原函数等价新增0。"
        "只读既有调用库存、定点PE和源码；未编译、类型检查、grammar读取、测试或执行目标/算法。"
        "脚本归scripts/analysis，定点证据和临时文本归archive/2026-11/20261118_*；未改README。\n",
        encoding="utf8")
    print(json.dumps({"report": str(PREFIX.relative_to(ROOT)), "source_observed": observed,
                      "preload_source_observed": preload_observed,
                      "pending_hooks": len(summary["main_hooks_pending"])}, ensure_ascii=True))


if __name__ == "__main__":
    main()

"""Cached ca01 mouth producer -> actual pending/active receiver, static only."""
import argparse
import json
from pathlib import Path
import subprocess

from recovery_A_20261116 import ROOT, read_json, sha


PREFIX = ROOT / "reports/recovery_A_20261117_emote_mouth_lifecycle"
EVIDENCE = ROOT / "archive/2026-11/20261117_emote_mouth_lifecycle_pe.json"
SYNTAX = ROOT / "archive/2026-11/20261117_emote_mouth_lifecycle_syntax.json"
SOURCES = ["simulator/crates/runtime/src/emote_m2mouth.rs",
           "simulator/crates/runtime/src/emote_m2timed.rs"]


def syntax():
    records = []
    for path in SOURCES:
        raw = (ROOT / path).read_bytes()
        result = subprocess.run([str(Path.home() / ".cargo/bin/rustfmt.exe"), "--edition", "2024",
                                 "--config", "skip_children=true", "--emit", "stdout"],
                                input=raw, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        records.append({"path": path, "sha256": sha(raw), "returncode": result.returncode,
                        "stderr": result.stderr.decode("utf-8", errors="replace")})
    SYNTAX.write_text(json.dumps({"method": "rustfmt grammar through stdin, source untouched",
        "sources": records, "compiler_invoked": False, "typechecked": False,
        "tests_run": False, "target_source_executed": False}, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"grammar_returncodes": [r["returncode"] for r in records]}))
    if any(r["returncode"] for r in records):
        raise SystemExit(1)


def report():
    old = read_json(ROOT / "reports/recovery_A_20261117_emote_mouth_output.json")
    inputs = read_json(ROOT / "archive/2026-11/20261117_emote_bank_inputs.json")
    config = next(r for r in inputs["resources"] if r["resource"] == "ca01.inc")
    controls = [r for r in old["controls"] if r.get("enabled") != 0]
    talk_names = {r["talkLabel"] for r in controls}
    actual = [r for r in config["sections"]["lip_variable"] if r[2] in talk_names]
    consumers = [r for r in old["consumers"] if r["parameter"]["id"] in talk_names]
    helper = (ROOT / SOURCES[0]).read_text(encoding="utf-8")
    timed = (ROOT / SOURCES[1]).read_text(encoding="utf-8")
    observed = {
        "enabled_talk_label_selected": "if mouth::is_talk_label(root, name)" in timed,
        "pending_is_distinct_from_active": "pending: bool" in helper and "segment: Option<Segment>" in helper,
        "same_target_shortcut_checks_active_only": "self.segment.as_ref().is_some_and" in helper,
        "actual_tick_starts_pending": "if self.pending {" in helper,
        "same_timed_owner_dispatches_tick": "Self::MouthTalk(state) => return state.advance(delta)" in timed,
        "existing_cancel_finish_keep_ownership": "self.active.remove(name)" in timed
            and "base.insert(name, f64::from(state.target()))" in timed,
    }
    summary = {
        "scope": "ca01 enabled mouth talk replacement-target lifecycle, existing M2 producer/API only",
        "model": old["summary"]["resource"], "model_sha256": old["summary"]["decoded_sha256"],
        "profile": config["resource"], "profile_sha256": config["sha256"],
        "enabled_mouth_controllers": len(controls), "actual_lip_variable_records": len(actual),
        "actual_talk_parameter_consumers": len(consumers),
        "already_present_M2_blink_records_not_reimplemented": len(config["sections"]["blink"]),
        "metadata_auto_blink_enabled_in_fixed_model": 0,
        "new_independent_mouth_talk_receiver": True,
        "concrete_change": "positive duration stays pending until tick; a second same-target request before that tick replaces it, active shorter-endpoint shortcut only applies after dequeue",
        "CPU_static_observation": observed,
        "producer_consumer": "ca01 #pattern/#lip_variable -> shared value/chain target -> submit_targets -> enabled talkLabel MouthTalk -> same advance/finish/cancel -> face_talk -> parameter/ref/texture/mesh/stencil/RGBA",
        "main_hooks_required": [], "A_modified_CPU_import_advance_force": False,
        "new_actual_guard_hits_removed": 0, "complete_native_equivalence_added": 0,
        "remaining": ["driver append flag/true FIFO mode is not represented by current M2 target API and is not claimed here",
                      "no full independent automatic speech or enabled PSB auto-blink algorithm added",
                      "whole-frame/source lifetime acceptance remains deferred; no pixel execution claim"],
        "compiled": False, "typechecked": False, "tests_run": False,
        "target_or_evaluator_executed": False,
        "counts_scope": "existing static enabled/config/parameter declarations, not observed runtime frequency",
    }
    if SYNTAX.exists():
        parsed = read_json(SYNTAX)
        summary["source_grammar_only"] = {"returncodes": [r["returncode"] for r in parsed["sources"]],
            "source_matches": all(r["sha256"] == sha((ROOT / r["path"]).read_bytes()) for r in parsed["sources"])}
    per_pe = [{"sha256": old["perPE"][0]["sha256"], "addresses": "RVA",
        "contracts": {"132e2..13329": "separate integer label and float talkLabel receiver dispatch",
                      "be38..be70": "zero duration clears request/active state and writes current talk immediately",
                      "be73..bef7": "only active phase can retain shorter equal endpoint; otherwise replace pending tuple",
                      "bf7f..bfff": "dequeue on tick, snapshot current output and start phase0",
                      "bf41..c033": "f32 progression, endpoint commit, double pow narrowed into f32"},
        "evidence_path": EVIDENCE.relative_to(ROOT).as_posix(), "evidence_sha256": sha(EVIDENCE.read_bytes())}]
    payload = {"summary": summary, "perPE": per_pe, "actual_lip_variable_records": actual,
        "actual_consumers": consumers, "enabled_controls": controls,
        "sources": [{"path": path, "sha256": sha((ROOT / path).read_bytes())} for path in SOURCES],
        "evidence": read_json(EVIDENCE)}
    PREFIX.with_suffix(".json").write_text(json.dumps(payload, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    PREFIX.with_name(PREFIX.name + "_summary.json").write_text(json.dumps(
        {"summary": summary, "perPE": per_pe}, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    PREFIX.with_suffix(".md").write_text(
        "# A / 实际mouth talk pending→active→像素\n\n"
        "新增emote_m2mouth.rs，m2timed只按enabled mouthControl.talkLabel转入独立receiver。"
        "固定ca01已启用mouth1项、lip_variable12条写face_talk、该模型12处parameter消费；"
        "M2 blink4条已有源码，不重复实现；PSB内建自动blink未启用，不扩此支。\n\n"
        "真实差异：原通用scalar在submit时即为active，导致tick前第二个同终点较长请求被错误保留短请求。"
        "driver be79仅活动state+18才走此优化；beae清队列并保持当前输出，bf7f到tick才出队建phase。"
        "新receiver独立持有pending/active，沿既有advance写face_talk；零时长、取消、finish及卸载复用原owner释放。"
        "整数face_mouth独立，不与talk相乘；下一层仍是已有参数／引用／atlas／mesh／stencil真实RGBA。\n\n"
        "接点已在CpuM2TimedVariables::submit_targets/advance/finish_all/cancel内部完成，无main hook，"
        "未改CPU import/advance/force、desktop或trait。证据SHA85eb459e…93ce，132e2、be00、bf10；"
        "新增三段定点证据，其余复用既有mouth报告，不扫描资源或CFG。\n\n"
        "当前API未表达driver append flag，本批只补原有replacement路径，不称完整FIFO/自动说话。"
        "完整native等价新增0；仅静态证据与stdin语法读取，无编译、类型检查、测试、目标/求值/像素执行。"
        "脚本归analysis，PE和语法输出归archive/2026-11/20261117_*；无新目录或README修改。\n", encoding="utf-8")
    current_path = ROOT / "reports/recovery_A_20261117_emote_current_summary.json"
    current = read_json(current_path)
    current["summary"]["latest_mouth_talk_lifecycle"] = {
        "report": PREFIX.with_name(PREFIX.name + "_summary.json").relative_to(ROOT).as_posix(),
        "new_receiver": True, "actual_lip_records": len(actual), "actual_parameter_consumers": len(consumers),
        "main_hooks_pending": [], "pixels_executed": False, "complete_native_equivalence_added": 0}
    current_path.write_text(json.dumps(current, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(summary))


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--syntax", action="store_true")
    parser.add_argument("--report", action="store_true")
    args = parser.parse_args()
    if args.syntax:
        syntax()
    if args.report:
        report()

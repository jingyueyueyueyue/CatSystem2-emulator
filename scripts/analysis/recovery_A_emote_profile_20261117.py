"""Bounded M2 profile-reuse evidence; static PE/config reads, no engine execution."""
from __future__ import annotations

import argparse
import ast
import json
from pathlib import Path
import re
import subprocess

from audit_runtime_evidence import blowfish_le_decrypt, kif_entries
from recovery_A_20261116 import ROOT, Image, read_json, sha
from recovery_A_emote_host_20261117 import sections
from recovery_A_receiver_contracts_20261116 import block

PREFIX = "recovery_A_20261117_emote_profile"
SNAPSHOT = ROOT / "archive/2026-11/20261117_emote_profile_evidence.json"


def cpu_integration():
    """Source presence only: never executes or typechecks the receiver."""
    relative = "simulator/crates/runtime/src/emote_cpu.rs"
    raw = (ROOT / relative).read_bytes()
    source = raw.decode("utf8")
    start = source.find("    fn import_m2_resource(")
    end = source.find("\n    fn ", start + 1)
    body = source[start:end if end >= 0 else None] if start >= 0 else ""
    checks = {
        "prepare_helper": "owner.prepare_profile_reuse(" in body,
        "previous_identity_inputs": "&state.m2_player_name" in body and "&state.m2_timeline" in body,
        "prepared_variables": "player.variables = prepared.variables" in body,
        "prepared_owner": "prepared.owner" in body,
        "prepared_surface_duration": "fade_duration = prepared.surface_duration" in body,
        "capture_before_publish": 0 <= body.find("self.capture_surface(task)") < body.find("state.player = Some(player)"),
        "reusing_owner": "let reusing_owner = profile_reuse.is_some()" in body,
        "skip_owner_reconstruction": "if !reusing_owner {" in body and "else if reusing_owner {" in body,
        "timeline_stop_play": 'player.stop_timeline("")' in body and "player.play_timeline(&load.timeline_label, 2)" in body,
    }
    condition = re.search(r"let profile_reuse\s*=\s*if\s+([^\{]+)\{", body)
    condition = " ".join(condition.group(1).split()) if condition else None
    return {"path": relative, "sha256": sha(raw), "static_source_presence_only": True,
            "checks": checks, "connected": all(checks.values()),
            "condition": condition,
            "zero_transition_connected": bool(condition and "transition_raw" not in condition),
            "prior_integration_grammar": "archive/2026-11/20261117_emote_profile_integration_syntax.json"}


def probe():
    identity = read_json(ROOT / "reports/recovery_A_20261116_emote.json")
    image = Image(ROOT / identity["binary_path"], identity["PE_sha256"])
    prior = read_json(ROOT / "reports/recovery_A_20261117_emote_host.json")
    cached = prior["resources"]
    first = cached["ca01.pfl"]
    second = cached["ca01L.pfl"]
    shared = []
    for a in first["sections"]["charaname"]:
        for b in second["sections"]["charaname"]:
            if a[2] == b[2]:
                shared.append({"resource_stem": a[2], "first_chara": a[0],
                               "second_chara": b[0], "first_player": a[1],
                               "second_player": b[1]})
    archive = image.path.parent / "m2config.int"
    entries, _, cipher = kif_entries(archive, 0x0F6BB0AE)
    entry = next(entry for entry in entries if entry["name"] == b"body.dif")
    with archive.open("rb") as handle:
        handle.seek(entry["offset"])
        raw = handle.read(entry["size"])
    raw = blowfish_le_decrypt(cipher, raw) if cipher else raw
    config = sections(raw)
    spans = {
        "profile_switch": (0x171A00, 0xC10),
        "primary_selection": (0x172610, 0xF60),
        "secondary_selection_prepass": (0x173570, 0x24A),
        "secondary_selection_commit": (0x173BC0, 0x182),
    }
    payload = {
        "PE": {"path": identity["binary_path"], "sha256": image.digest},
        "resources": {"cached_host_evidence_sha256": sha((ROOT /
            "reports/recovery_A_20261117_emote_host.json").read_bytes()),
            "profile_names": ["ca01.pfl", "ca01L.pfl"],
            "profiles": {key: cached[key] for key in ("ca01.pfl", "ca01L.pfl")},
            "shared_resources": shared,
            "body": {"member": "body.dif", "archive_offset": entry["offset"],
                "decoded_sha256": sha(raw), "smooth": config.get("smooth", []),
                "group": config.get("group", []),
                "group_inertia": config.get("group_inertia", [])}},
        "evidence": {key: block(image, *span) for key, span in spans.items()},
        "scope": "two fixed real profiles and body.dif; no new PSZ inventory or CFG scan",
        "compiled": False, "typechecked": False, "tests_run": False,
        "target_executed": False,
    }
    SNAPSHOT.write_text(json.dumps(payload, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    print(json.dumps({"profile_shared_rows": len(shared),
                      "examples": shared[:3], "body": payload["resources"]["body"]},
                     ensure_ascii=True))


def extend():
    payload = read_json(SNAPSHOT)
    image = Image(ROOT / payload["PE"]["path"], payload["PE"]["sha256"])
    spans = {
        "manager_load": (0x17A990, 0x380),
        "manager_profile_lookup": (0x17D600, 0x590),
        "player_config_setters": (0x171340, 0x6C0),
        "manager_profile_inputs": (0x17D240, 0x3C0),
        "resource_open": (0x175900, 0x6E0),
        "difference_configuration": (0x177270, 0x780),
        "install_primary_secondary": (0x174210, 0x1B0),
        "secondary_smooth_import": (0x178570, 0x170),
    }
    payload["evidence"].update({key: block(image, *span) for key, span in spans.items()})
    SNAPSHOT.write_text(json.dumps(payload, ensure_ascii=False, indent=2) + "\n", encoding="utf8")


def report(grammar=False):
    evidence = read_json(SNAPSHOT)
    inventory_path = ROOT / "archive/2026-11/20261117_emote_pixel_scope.json"
    fixed = read_json(inventory_path)
    names = {row["name"]: row["decoded_sha256"] for row in fixed["resources"]}
    actual_psz = [{"name": row["resource_stem"] + ".psz",
                   "decoded_sha256": names.get(row["resource_stem"] + ".psz")}
                  for row in evidence["resources"]["shared_resources"]]
    if any(row["decoded_sha256"] is None for row in actual_psz):
        raise ValueError("profile-shared PSZ absent from the fixed actual135 inventory")
    sources = ["simulator/crates/runtime/src/emote_m2live.rs",
               "simulator/crates/runtime/src/emote_m2profile.rs"]
    cpu = cpu_integration()
    profile_source = (ROOT / sources[1]).read_text(encoding="utf8")
    timeline_rebuild = "configured.rebuild_profile_timeline_banks(" in profile_source
    if grammar:
        rows = []
        for path in sources:
            raw = (ROOT / path).read_bytes()
            run = subprocess.run([str(Path.home() / ".cargo/bin/rustfmt.exe"),
                "--edition", "2024", "--config", "skip_children=true", "--emit", "stdout"],
                input=raw, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, check=False)
            rows.append({"path": path, "sha256": sha(raw), "grammar_exit": run.returncode,
                         "diagnostic": run.stderr.decode("utf8", errors="replace")})
        (ROOT / "archive/2026-11/20261117_emote_profile_syntax.json").write_text(json.dumps({
            "grammar_only": True, "sources": rows, "compiled": False,
            "typechecked": False, "tests_run": False, "target_executed": False}, indent=2) + "\n", encoding="utf8")
        if any(row["grammar_exit"] for row in rows):
            raise ValueError("Rust source grammar read failed")
    small = {
        "summary": {
            "batch": "retained PSB profile configuration and actual shared bank consumer",
            "actual_profile_files_checked": 2,
            "actual_shared_resource_rows": len(evidence["resources"]["shared_resources"]),
            "shared_resources_present_in_prior_actual135": len(actual_psz),
            "actual_body_smooth_rows": len(evidence["resources"]["body"]["smooth"]),
            "new_source_files": [sources[1]], "modified_source_files": [sources[0]],
            "runtime_source": [
                "configuration rebind retains current PSB variables, primary/secondary selection and live banks",
                "reselect now selects retained bank records instead of reconstructing their definitions",
                "registered value/chain speed and weight, current timers and force survive; same-timeline banks retain sidecars and phases",
                *( ["changed timeline restores #defval and old blink bases, then rebuilds blink/lip only through the shared bank constructor"] if timeline_rebuild else []),
                "changed primary selection uses existing shared chain_requests/value_table and actual driver samples",
                "optional real old-RGBA surface duration is returned before any CPU publish",
                "later917 uses the newly bound secondary config on the same owner and value table"],
            "CPU_hook_pending": not cpu["connected"], "CPU_source_modified": cpu["connected"],
            "A_CPU_source_modified": False,
            "blanket_profile_guard_removed": cpu["connected"],
            "CPU_source_integration": cpu,
            "timeline_bank_rebuild_source_present": timeline_rebuild,
            "ordinary_routes_added": 0, "completed_native_equivalence_added": 0,
            "full_NEKO_strict_pixel_claim": False,
            "remaining": [
                *([] if cpu["connected"] else ["CPU source helper/publish/capture integration is incomplete"]),
                *([] if cpu["zero_transition_connected"] else ["main must widen helper to same_resource && (!same_profile || changed timeline), including transition0; keep CPU import ownership"]),
                "different base object and primary dimension layout remain explicit errors on this new path",
                *([] if timeline_rebuild else ["different timeline default/bank rebuild remains explicit"]),
                "group_change still belongs to real differing-resource replacement, not an alias switch",
                "no dynamic call frequency, compilation, type checking, algorithm or game validation has been performed"],
            "compiled": False, "typechecked": False, "tests_run": False, "target_executed": False,
        },
        "perPE": [{"sha256": evidence["PE"]["sha256"],
            "anchors_rva": ["174210", "174243", "174246", "1742f2", "174372", "17d546",
                            "171a38", "171bb8", "172610", "173119", "17317d", "1732d6", "173570"],
            "evidence_path": str(SNAPSHOT.relative_to(ROOT)).replace("\\", "/")}],
    }
    api = {
        "method": "CpuM2LiveOwner::prepare_profile_reuse(&load, previous_player_name, previous_timeline, variables, controller_current, surface_enabled, default_surface_duration)",
        "result": "CpuM2ProfileReuse { owner, variables, surface_duration } (internal)",
        "condition": cpu["condition"],
        "recommended_condition": "same_resource && (!same_profile || state.m2_timeline != load.timeline_label); includes transition0; base/layout guards remain in helper",
        "capture": "surface_duration participates in fade_duration before capture_surface; Some(0) remains a real request",
        "publish": "player.variables = reuse.variables; owner = reuse.owner; treat as reusing_owner=true",
        "preserve": "do not clear m2_timed_variables, reinstall secondary/defaults or rerun apply_907 on the prepared owner",
        "unmodified": "existing primary imports, new-resource construction, secondary917 receiver and surface lifecycle remain separate",
    }
    full = {**small, "APIs": api, "sources": [{"path": path, "sha256": sha((ROOT / path).read_bytes())}
        for path in sources + ["scripts/analysis/recovery_A_emote_profile_20261117.py"]],
        "resources": evidence["resources"], "actual_PSZ_identities": actual_psz,
        "prior_actual135_inventory_sha256": sha(inventory_path.read_bytes()),
        "evidence_sha256": sha(SNAPSHOT.read_bytes()),
        "prior_chain_evidence": "reports/recovery_A_20261117_emote_intermediate.json",
        "prior_secondary_evidence": "reports/emote_secondary_contracts_20261117.json"}
    prefix = ROOT / ("reports/" + PREFIX)
    prefix.with_suffix(".json").write_text(json.dumps(full, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    prefix.with_name(prefix.name + "_summary.json").write_text(json.dumps(small, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    integration_text = (
        "主线CPU实际源码已存在helper、prepared owner/variables、旧RGBA捕获、reusing_owner及跳过重建；"
        "本脚本按当前源码静态条件生成状态，不再硬编码待接。"
        if cpu["connected"] else
        "CPU源码静态条件尚未全部匹配，具体缺项见summary.CPU_source_integration.checks。"
    )
    prefix.with_suffix(".md").write_text(
        "# A / 同PSB profile配置复用\n\n"
        "静态实际命中：ca01.pfl/ca01L.pfl共5个相同body PSZ，均按decoded SHA关联既有135份资源清单；"
        "其primary分别ca_m.dif/ca_l.dif，"
        "均include ca01.inc，secondary均body.dif。后者有16条#smooth（head_LR/head_UD/body_LR/body_UD等）"
        "及7组资源分组。不是动态调用次数，也没有重扫135PSZ。\n\n"
        "174210（17d546调用）先换primary64/secondary68并以16cb00注册名字；已有值不重建。"
        "171a38资源向量相等且player为1时直接到171bb8，跳过172070的重建。173119/17317d选择"
        "原银行，而非新建定义；随后1732d6→1766b0→16cf70共享已有timed/value table。"
        "#smooth仍复用既有17709c当前driver采样修正，未复制第二套插值。\n\n"
        "本批源码：m2live::reselect改为clone并重新选中真实银行；同选择也保留新config。"
        "新增内部emote_m2profile::prepare_profile_reuse，保留secondary选择、value/chain速度/权重、"
        "force/material；相同timeline保留sidecar和phase，改变timeline仅重建两类银行。不同primary选择进入现有chain消费者。Fade先提出真实旧RGBA"
        "捕获时长，再令scalar Fix，图像仍由既有surface owner消费。原资源不变，不产生group惯性脉冲；"
        "后续917沿新secondary配置继续同一个select_secondary owner。\n\n"
        "CPU接线静态核对（本批未改CPU）：\n\n"
        "1. 当前条件`" + str(cpu["condition"]) + "`调用旧owner.prepare_profile_reuse"
        "(&load,&state.m2_player_name,&state.m2_timeline,&old_player.variables,&old_player.controller_variables()?,"
        "state.surface_enabled(),state.surface.default_duration())?并保存结果。\n"
        "2. capture_surface之前将结果surface_duration交现有fade_duration；None无需捕获，Some(0)仍是真请求。\n"
        "3. 克隆原player后取结果variables/owner，保留m2_timed_variables；reusing_owner包含此结果，"
        "跳过new/install_secondary/seed_history/apply_907重建段。其余validate/最终提交/geometry/surface接线沿用。\n\n"
        + integration_text + "当前CPU条件为`" + str(cpu["condition"]) + "`。"
        "不同base object、primary维度布局变化仍明确报错；timeline银行重建状态见"
        "`recovery_A_20261117_emote_profile_timeline.md`。完整原函数等价新增0，ordinary路由新增0。"
        "不宣称NEKO完整像素。只读PE/config和源grammar，无编译/类型检查/测试/算法或目标运行。"
        "脚本归scripts/analysis，固定证据和grammar记录归archive/2026-11/20261117_emote_profile_*；"
        "没有修改CPU、README或译文。\n", encoding="utf8")
    current_path = ROOT / "reports/recovery_A_20261117_emote_current_summary.json"
    current = read_json(current_path)
    current["profile_reuse_batch"] = small
    current["summary"]["latest_A_batch"] = small["summary"]
    relative = prefix.with_suffix(".md").relative_to(ROOT).as_posix()
    if relative not in current.setdefault("reports", []):
        current["reports"].append(relative)
    current_path.write_text(json.dumps(current, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    rolling_path = ROOT / "reports/recovery_A_20261117_rolling_delivery.md"
    rolling = rolling_path.read_text(encoding="utf8")
    note = ("交付`recovery_A_20261117_emote_profile.md`：2份实际PFL共享5份已知PSZ，body.dif有16条#smooth。"
            "m2live保留真实银行并重绑定配置；新m2profile提供prepare_profile_reuse的实际owner/variables/"
            "surface_duration。" + integration_text + "未增完整native等价计数。\n\n")
    rolling = re.sub(r"交付`recovery_A_20261117_emote_profile\.md`：[^\n]*\n\n", "", rolling)
    if note not in rolling:
        rolling_path.write_text(rolling.replace("# A / 本轮小批交付\n\n",
            "# A / 本轮小批交付\n\n" + note, 1), encoding="utf8")
    print(json.dumps(small["summary"], ensure_ascii=True))


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--probe", action="store_true")
    parser.add_argument("--extend", action="store_true")
    parser.add_argument("--report", action="store_true")
    parser.add_argument("--syntax", action="store_true")
    args = parser.parse_args()
    ast.parse(Path(__file__).read_text(encoding="utf8"))
    if args.probe:
        probe()
    if args.extend:
        extend()
    if args.report:
        report(args.syntax)

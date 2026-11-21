"""Bounded Emote host clock/viewport/default-selection evidence.

Read-only EXE/DLL/KIF analysis. No Rust build/tests or original engine loading.
"""
from __future__ import annotations

from datetime import datetime, timezone
import json
import re

from recovery_A_20261116 import ROOT, Image, read_json, sha
from recovery_A_receiver_contracts_20261116 import block
from audit_runtime_evidence import kif_entries, blowfish_le_decrypt

DRIVER_SHA = "85eb459e94e74ab608f649eb7fb440f2526e90aaa1af7276ed01dc10018903ce"


def sections(data):
    text = data.decode("cp932", errors="strict")
    text = re.sub(r"/\*.*?\*/", "", text, flags=re.S)
    result, current = {}, None
    for line in text.splitlines():
        line = line.split("//", 1)[0].strip()
        if not line:
            continue
        if line.startswith("#"):
            current = line[1:].lower()
            result.setdefault(current, [])
        elif current is not None:
            result[current].append(line.split())
    return result


def main():
    prior = read_json(ROOT/"reports/recovery_A_20261116_emote.json")
    game = (ROOT/prior["binary_path"]).parent
    exe = Image(game/"NEKOPARAvol3.exe", prior["PE_sha256"])
    driver = Image(game/"emotedriver.dll", DRIVER_SHA)
    archive = game/"m2config.int"
    entries, kind, cipher = kif_entries(archive, 0x0F6BB0AE)
    configs = {}
    for name in [b"m2view.cnf", b"ca01.pfl", b"ca01L.pfl", b"ca_m.dif", b"ca_l.dif"]:
        entry = next(e for e in entries if e["name"] == name)
        with archive.open("rb") as handle:
            handle.seek(entry["offset"])
            data = handle.read(entry["size"])
        data = blowfish_le_decrypt(cipher, data) if cipher else data
        configs[name.decode("ascii")] = {
            "resource_identity": str(archive.relative_to(ROOT))+"::"+name.decode("ascii"),
            "archive_offset": entry["offset"], "bytes": len(data),
            "decoded_sha256": sha(data), "sections": sections(data),
        }
    exe_spans = {
        "task_elapsed": (0x1DD440, 7),
        "task_clock_receive": (0x1DCBE0, 0x16),
        "M2Plane_update_to_manager": (0x23828A, 0x48),
        "player_u32_delta_to_driver_f32": (0x174156, 0x46),
        "player_clock_sentinel": (0x174040, 0x29),
        "display_mapping": (0x238090, 0x132),
        "offscreen_world_offset_default": (0x19B393, 0x0A),
        "offscreen_camera_and_projection": (0x19C050, 0x225),
        "native907_resolve_effective_flags": (0x1744A0, 0x70),
        "native907_reset_A_records": (0x174AF0, 0x95),
        "native907_reset_B_records": (0x1747D8, 0x31),
        "native907_controller_enable_tail": (0x1937AB, 0x18),
        "native907_transition_bank_A": (0x193415, 0x7B),
        "native907_transition_bank_B": (0x193567, 0x6A),
        "native907_transition_bank_C": (0x193712, 0x6D),
        "load_spec_colon_tokenizer": (0x17E56E, 0x9E),
        "load_spec_comma_tokenizer": (0x17E654, 0x67),
        "load_spec_extra_vector": (0x17E6AB, 0xB3),
        "profile_motion_driver_start": (0x171EDF, 0x7F),
        "load_profile_selector": (0x17D764, 0xC5),
    }
    dll_spans = {
        "original_Tick_vtable_fc": (0x3540, 0x11),
        "core_default_clock_gate": (0xFA8D, 0x23),
        "motion_default_speed1": (0x30493, 0x5D),
        "driver_Tick_no_ms_conversion": (0x12BB0, 0x86),
        "driver_Tick_motion_advance": (0x12CD2, 0x26),
        "motion_time_multiplier": (0x3ABC0, 0x28),
        "metadata_base_chara": (0xFDEA, 0xD6),
        "metadata_base_motion": (0xFED9, 0x59),
        "host_transform_and_viewport_read": (0x56194, 0x55),
    }
    summary = {
        "driver_tick_units_per_frame_default": 1.0,
        "driver_tick_divisor_default": 1,
        "driver_motion_speed_default": 1.0,
        "first_original_player_update_delta": 1,
        "time_source": "raw original task64-task58, passed as EmoteTick.task_elapsed",
        "unproven_ms_or_fps_conversion_applied": False,
        "host_display_half_pixel_offset": -0.5,
        "fixed_PE_wide_internal_viewport_origin_scale": None,
        "default_offscreen_origin_formula": ["width/2-0.5", "height/2-0.5"],
        "default_offscreen_scale": 1.0,
        "default_offscreen_world_translation": 0.0,
        "actual_1280x720_default_offscreen_origin": [639.5, 359.5],
        "viewport_source": "actual host WORLD/VIEW/PROJECTION/GetViewport; no center from PSB screenSize",
        "actual_m2config_entries": len(entries),
        "actual_m2view_wndsize": configs["m2view.cnf"]["sections"].get("wndsize"),
        "actual_m2view_position": configs["m2view.cnf"]["sections"].get("position"),
        "default_base_motion_source": "driver reads PSB metadata.base.chara and metadata.base.motion",
        "profile_timeline_source": "PFL charaname/motname; timelineControl label is separate from base object motion",
        "native907_still_requires_full_controller_reset_bank": True,
        "native907_is_not_proven_visibility_switch": True,
        "native907_bound_bank_receiver_implemented_in_source": True,
        "native907_missing_bank_is_error": True,
        "native907_actual_PFL_to_controller_bank_import_complete": False,
        "native906_resource_plan_reads_actual_config_and_PSZ": True,
        "native906_plan_is_completed_native_execution": False,
        "host_exports_or_dispatch_edited": False,
        "compiled": False, "tests_run": False, "original_code_executed": False,
        "integrated_completed_native_count": 0,
    }
    apis = {
        "original_clock_backend": "CpuEmoteBackend::new_original_task_clock(viewport)",
        "default_target_viewport": "CpuViewport::original_neko_target(width,height); actual1280x720 =>639.5,359.5,scale1,time1",
        "default_target_matrices": "CpuD3d9Projection::original_neko_target(width,height,actual_world_offset)",
        "clock_step": "CpuOriginalTaskClock::advance(raw_task_clock)->f32",
        "tick_binding": "state.advance(task,EmoteTick{task_elapsed:raw_task64_minus58,increment:unused_in_original_mode})",
        "real_projection": "CpuD3d9Projection::project([x,y,z])",
        "reduced_projection": "CpuD3d9Projection::axis_aligned_viewport(width,height); rejects perspective/rotation/nonuniform",
        "display_quad": "CpuOriginalDisplayMapping::point/quad; host offset/scale from actual render context",
        "host_object_affine": "EmoteServices::set_root_affine(task,[a,b,c,d,tx,ty]); actual resolved object transform only",
        "M2_config": "CpuM2Config::parse_cp932/records/profile_file/select_profile/difference_transform",
        "M2_spec": "CpuM2LoadSpec::parse_unquoted; profile,chara,pattern...:timeline",
        "actual_archive_plan": "desktop load_m2_resource_plan(ResourceIndex,config_category,config_name,spec)",
        "profile_result": "CpuM2ProfileSelection resource_stem/player_name/timeline_label/difference_files",
        "raw_psb_default": "CpuPlayer::new uses CpuMotionModel metadata.base object/motion",
        "native907_bind": "EmoteServices::bind_native907_state(task,CpuNative907State)",
        "native907_effect": "CpuNative907State::apply resolves raw/scene flags,resets typed banks,writes actual PSB sampler variables",
    }
    contracts = {
        "native907": {
            "wrapper_RVA": 0x24CC40, "task_type": 54, "command": 526,
            "receiver_data1": "raw DWORD enable request",
            "effective_flags": (
                "copy raw to engine/control; if raw!=0 and scene_override exists, "
                "scene_override[0]==0 =>both0, else engine=override[1],control=override[2]"
            ),
            "effects": [
                "write effective engine flag at linked controller+10 when nonnull",
                "invoke linked transition controller193400(control); when control==0 clear+58",
                "always reset A bank via174a60: write active named driver variables to record+40,clear+60/+64,mark bank dirty",
                "always reset B bank via174790: clear bank+20/+24,set active record+ac=1,copy first phase DWORD to+98 when present",
                "preserve raw request at player+10c; this is not a clamp-to-bool instruction",
            ],
            "remaining": "receiver is implemented for bound actual owned reset banks. Automatic PFL-to-bank import and baseline evaluation remain incomplete; blanket visibility=false would invent semantics",
        },
        "viewport": {
            "final_quad_formula": "logical_xy * actual_render_scale + actual_render_origin -0.5",
            "M2Plane_helper_RVA": 0x238090,
            "internal_driver_capture_RVA": 0x56194,
            "transform_ids": {"world": 256, "view": 2, "projection": 3},
            "original_no_render_context_scale_fallback": 1.0,
            "internal_origin_not_fixed": True,
            "original_default_offscreen_RVA": 0x19C050,
            "default_world_offset_constructor_RVA": 0x19B393,
            "default_camera_eye": ["-0.5", "0.5", "-height/2"],
            "default_camera_target": [-0.5, 0.5, 0],
            "default_camera_up": [0, -1, 0],
            "post_look_at_change": "negate only first matrix element",
            "default_plane_mapping": "pixel.xy=world.xy+(width/2-0.5,height/2-0.5)",
        },
    }
    files = [
        "simulator/crates/runtime/src/emote.rs", "simulator/crates/runtime/src/emote_cpu.rs",
        "simulator/crates/runtime/src/emote_host.rs", "simulator/crates/runtime/src/emote_m2config.rs",
        "simulator/crates/runtime/src/emote_control.rs", "simulator/crates/runtime/src/emote_dispatch.rs",
        "simulator/crates/desktop/src/emote_native.rs",
        "scripts/analysis/recovery_A_emote_host_20261117.py",
    ]
    out = {
        "schema": "cs2-recovery-A-emote-host/v1", "task_date": "2026-11-17",
        "generated_at_utc": datetime.now(timezone.utc).isoformat(),
        "summary": summary, "APIs": apis, "contracts": contracts,
        "source_PE_sha256": prior["PE_sha256"], "source_driver_sha256": DRIVER_SHA,
        "resources": configs,
        "EXE_evidence": {key: block(exe,*span) for key,span in exe_spans.items()},
        "driver_evidence": {key: block(driver,*span) for key,span in dll_spans.items()},
        "source_files": [{"path": path, "sha256": sha((ROOT/path).read_bytes())} for path in files],
    }
    prefix = ROOT/"reports/recovery_A_20261117_emote_host"
    prefix.with_suffix(".json").write_text(json.dumps(out,ensure_ascii=False,indent=2)+"\n",encoding="utf-8")
    prefix.with_name(prefix.name+"_summary").with_suffix(".json").write_text(
        json.dumps({"summary":summary,"APIs":apis,"contracts":contracts},ensure_ascii=False,indent=2)+"\n",encoding="utf-8")
    lines = [
        "# A: original Emote host parameters and selection", "",
        "Read-only EXE/DLL/archive evidence; no compilation/tests or original execution.", "",
        "## Ready parameters / APIs", "",
        "- Original task clock is a raw DWORD. Driver input is f32(delta), default divisor1 and motion speed1; "
        "no /60 or /1000 conversion occurs in this identified path.",
        "- First174040 update uses current-1; previous168 zero-sentinel behavior is retained in CpuOriginalTaskClock.",
        "- CpuEmoteBackend::new_original_task_clock(viewport) uses EmoteTick.task_elapsed and ignores increment.",
        "- Final display quad is logical_xy*actual_host_scale+actual_host_origin-0.5.",
        "- Internal renderer reads real host WORLD/VIEW/PROJECTION and viewport; there is no universal PE-bound center/scale.",
        "- Default offscreen setup19c050 is now recovered:origin(width/2-0.5,height/2-0.5),scale1 "
        "when world offset168 is its constructor default0. Actual1280x720 =>639.5/359.5. "
        "Use CpuViewport::original_neko_target(width,height).",
        "- CpuD3d9Projection projects actual supplied matrices; axis_aligned_viewport only reduces a proven uniform affine plane.",
        "", "## APIs", "",
    ]
    lines += ["- "+api for api in apis.values()]
    lines += [
        "", "## Actual resource path", "",
        "- m2config.int has106 records; m2view.cnf provides datapath=psz,dataext=psz,wndsize1280x720,position0/0.",
        "- ca01.pfl chara1 selects player body/resource ca01_l_body_1. motion0 selects a timelineControl waiting-loop label,ca_m.dif and body.dif.",
        "- ca01L.pfl selects the same model with ca_l.dif instead.",
        "- ca_m.dif contains scale0.6/0.6,position-640/-204,scaleoffset0/-152; "
        "ca_l.dif contains scale0.8/0.8,position-640/-152,scaleoffset0/-340.",
        "- These resource values are not collapsed into one guessed CPU origin. "
        "Host object placement,scale offset and internal projection are separate inputs.",
        "- Raw PSB default is metadata.base.chara/motion,as confirmed by driver0xfdea/0xff0d. "
        "PFL motname selects a metadata.timelineControl label through EXE171ec0 virtual+d4; it is not another object.motion path.",
        "- Native906 unquoted grammar from17e530 is profile,chara,pattern...:timeline. "
        "Absent colon suffix defaults0; comma tokens after the second are a separate pattern vector.",
        "- load_m2_resource_plan reads actual startup CNF,selected PFL,differences/includes and referenced PSZ through ResourceIndex. "
        "Returning a plan is not executing all native906 timeline/controller effects.",
        "", "## Native907 correction and remaining receiver", "",
        "- 907 carries a raw enable DWORD,not a motion-selection string. Manager always uses object selector0.",
        "- 1744a0 applies scene override then resets both controller banks,even when the requested DWORD did not change.",
        "- A bank rewrites real named driver variables and resets transition counters; B bank resets active phases.",
        "- CpuNative907State now implements the bound-bank operation,including actual named-variable writes consumed by the sampler. "
        "EmoteServices::bind_native907_state exposes it through the boxed backend.",
        "- Setting CpuPlayer.enabled alone would invent visibility semantics and omit these effects. "
        "An unbound bank remains an error; automatic PFL-to-bank construction is incomplete. "
        "Resource replacement clears old907 bindings and external baselines must be refreshed by the real owner.",
        "", "## Paths/classification", "",
    ]
    lines += ["- "+path for path in files]
    lines += [
        "- reports/recovery_A_20261117_emote_host.json/md",
        "- reports/recovery_A_20261117_emote_host_summary.json",
        "", "No root README,host exports/dispatch or assets changed; no temporary outputs. "
        "Functional script classified under scripts/analysis.",
    ]
    prefix.with_suffix(".md").write_text("\n".join(lines)+"\n",encoding="utf-8")
    current_path=ROOT/"reports/recovery_A_20261117_emote_current_summary.json"
    if current_path.is_file():
        current=read_json(current_path)
        current["summary"]["backend_native_branches"]["907"]=(
            "bound actual reset banks only; scene override + transition reset + real sampler variable writes"
        )
        current["summary"]["native907_automatic_profile_bank_import_complete"]=False
        current["summary"]["original_task_clock_units_per_frame"]=1.0
        current["summary"]["original_offscreen_default_1280x720"]={
            "origin":[639.5,359.5],"scale":1.0,"world_offset":0.0,
        }
        current["host_parameters"]=str(prefix.with_name(prefix.name+"_summary").with_suffix(".json").relative_to(ROOT))
        current["host_APIs"]=apis
        current["source_hashes_current"]=[{"path":path,"sha256":sha((ROOT/path).read_bytes())}for path in files]
        for path in [
            "simulator/crates/runtime/src/emote_host.rs",
            "simulator/crates/runtime/src/emote_control.rs",
            "simulator/crates/runtime/src/emote_m2config.rs",
        ]:
            if path not in current["new_source_paths"]:
                current["new_source_paths"].append(path)
        if str(prefix.with_suffix(".md").relative_to(ROOT)) not in current["reports"]:
            current["reports"].append(str(prefix.with_suffix(".md").relative_to(ROOT)))
        current_path.write_text(json.dumps(current,ensure_ascii=False,indent=2)+"\n",encoding="utf-8")
    print(json.dumps(summary,ensure_ascii=True,indent=2))


if __name__=="__main__":
    main()

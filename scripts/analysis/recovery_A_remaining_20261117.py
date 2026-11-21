"""Curated A behavior backlog from existing evidence/source; no corpus scan."""
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
PREFIX = ROOT / "reports/recovery_A_20261118_remaining"
RT = "simulator/crates/runtime/src/"


def read(path):
    return json.loads((ROOT / path).read_text(encoding="utf8"))


def ref(path, anchor):
    text = (ROOT / path).read_text(encoding="utf8")
    at = text.find(anchor)
    return {"path": path, "line": None if at < 0 else text[:at].count("\n") + 1,
            "anchor": anchor, "present": at >= 0}


def row(key, name, missing, hits, dependency, evidence, path, anchor, batches):
    return {"id": key, "family": name, "still_missing": missing,
            "actual_hit_scope": hits, "core_dependency": dependency,
            "evidence": evidence, "source": ref(path, anchor),
            "conditional_source_batches_optimistic_conservative": batches}


def main():
    m2 = [
        row("M1", "manager与多player生命周期",
            "同task多个internal selector完整创建/选择/更新/绘制；922/923全局manager行为",
            "NEKO固定906只selector0；既有普通922/923调用各0，非零selector动态命中未证",
            "真实selector/key生产者与manager所有权",
            "reports/recovery_A_20261117_emote_manager.json", RT+"emote_dispatch.rs", "blocked!(922", [2,4]),
        row("M2", "资源重开变体与失败生命周期",
            "同资源不同base/维度layout重开；跨资源失败的原版破坏性副作用。成功跨PSB、profile/timeline复用已接，不重复计缺",
            "ca01/ca01l既有80有向配置配对：60变timeline已接，base变更0；失败触发数未测",
            "driver重建、旧bank/force/value/table/surface的精确移交",
            "reports/recovery_A_20261117_emote_profile_timeline.md;reports/emote_surface_20261117.md",
            RT+"emote_m2profile.rs", "M2 retained-resource profile changes the base object", [2,4]),
        row("M3", "965非空波形程序",
            "%time/%set节点编译、当前/旧程序交接和真实波形求值；已证无directive配置分支已接",
            "965实际sscript@807878共1处；既有106配置有效directive0，本次不重扫",
            "program+100真实节点producer，不可拿action+180代替",
            "reports/recovery_A_20261117_emote_action_program.md", RT+"emote_m2program.rs", "M2 %time/%set", [2,4]),
        row("M4", "933底层driver变量getter",
            "base map缺名后的unqualified/qualified模型变量fallback；不能默给0",
            "933实际main.kcs@5459共1处；ca01默认22名=21显式+face_talk已全有来源，缺名0；其它live状态未穷尽",
            "driver133e0→337e0查询，不加入additive timeline差分",
            "reports/recovery_A_20261117_emote_debug_summary.json", RT+"emote_cpu.rs", "M2 debug registered name requires", [1,2]),
        row("M5", "模型控制器扩展",
            "PSB自主blink、其它enabled metadata控制器、talk append/FIFO等未闭合模式；M2 blink、普通mouth/transition已接",
            "既有135PSZ的eye blink enabled0、bust/hair/loop/parts enabled0；动态启用未测，不将0命中算实现",
            "真实动态启用/flags生产者与graph/timed共享owner",
            "reports/recovery_A_20261117_emote_metadata_scope_summary.json;reports/recovery_A_20261117_emote_mouth_lifecycle.md",
            RT+"emote_m2selector.rs", "automatic blink owner is not recovered", [2,4]),
        row("M6", "reference特殊时间选择",
            "caller forced selection bits1/4、非零loop/offset及首次反向重锚组合；普通/参数引用持续owner和释放已接",
            "135PSZ引用6075：普通3645、参数2430；已测resource loop/offset/mask非零各0，caller flags命中未知",
            "真实caller状态与持久reference时钟，不能从方向猜flags",
            "reports/recovery_A_20261117_emote_reference_direction_summary.json", RT+"emote_cpu.rs", "reference loop/nonzero motion flags", [1,2]),
        row("M7", "扩展mesh/stencil/颜色绘制",
            "非空cc插值、未恢复meshSync/meshTransform、textured type12/mode1、非中性/角颜色、非默认坐标/投影等共享像素消费",
            "固定5PSZ的26项声明guard0命中；既有135PSZ cc/type12/直源kind3等0；六composite共享range已接，不能再当未实现",
            "有触发的模型/动态状态及原driver范围/纹理/网格契约",
            "reports/recovery_A_20261117_emote_actual_draw_summary.json;reports/recovery_A_20261117_emote_shared_ranges_summary.json",
            RT+"emote_cpu.rs", "textured type12 target/reversal state", [3,6]),
        row("M8", "声音evaluator未覆盖协议",
            "非默认quiet-inhibit+78生产者、LPD未声明default的原版不确定状态、失败open的原版继续行为。成功LV/LPD、声音时钟/PCM及停用恢复已接",
            "924普通实际调用8处；既有4648个LV资源已格式检查；上述特定分支实际触发未证，不能套成8处均阻断",
            "+78真实producer；合法preset和sound时钟；unsafe/未初始化值不应伪造",
            "reports/recovery_A_20261117_emote_destructive_open.md;reports/recovery_A_20261117_emote_waveform_summary.json",
            RT+"emote_m2wave.rs", "pub quiet_scan_inhibited", [1,3]),
    ]
    prior_efk = read("reports/recovery_A_20261117_effekseer_rotation.json")
    efk = prior_efk["remaining_implementation_families"]
    plans = {"SKFE10plus":[1,2], "random":[2,4], "instance_tree":[2,4],
        "emit_shapes":[1,2], "transforms":[1,2], "color":[1,2], "other_geometry":[2,4],
        "sound":[1,2], "distortion":[1,2], "graphics_interop":[1,2], "task_rotation":[1,2]}
    for item in efk:
        item["conditional_source_batches_optimistic_conservative"] = plans[item["family"]]
        item["actual_hit_scope"] = "Yukikoi唯一1014@844116已定位，但R+8具体效果成员仍未知；不声称该特征被实际效果使用"
        item["evidence"] = "reports/recovery_A_20261117_effekseer_rotation.json;reports/recovery_A_20261117_efk_producer_summary.json"
    shared = row("S1", "M2/EFK活对象存读档",
        "完整player/PSB identity、value/chain/timed/bank/音频link/surface/action/instance图快照与原位恢复；playback_info不是存档",
        "type54真实游戏owner已证；EFK1014有静态producer；带这些owner的save实际次数未测",
        "主线对象图协议及所有跨owner引用/时钟恢复，需要主线hook",
        "reports/recovery_A_20261117_emote_lifecycle.md;simulator/crates/desktop/src/object_graph.rs",
        "simulator/crates/desktop/src/object_graph.rs", "pub fn object_graph_unsupported_task_families", [2,4])
    all_rows = m2 + efk + [shared]
    estimates = lambda rows: [sum(r["conditional_source_batches_optimistic_conservative"][i] for r in rows) for i in (0,1)]
    summary = {
        "date": "2026-11-18", "scope": "A专属PSB/M2/E-mote与type61 EFK；已知行为家族去重，不是全工程函数清单",
        "enumeration_exhaustive": False,
        "known_partial_behavior_families": {"M2_Emote":len(m2), "Effekseer":len(efk), "shared_persistence_counted_once":1, "total_curated":len(all_rows)},
        "missing_original_function_count": None,
        "reason_no_function_count": "同一receiver/helper横跨多个行为，多个版本/入口共享核；当前没有逐原函数全路径等价映射，guard/61函数提示均不是完整缺口",
        "latest_source_fix": "sscript918@790337先于906@790643；CPU不再丢弃空selector0 wrapper的filter设置，既有纹理/像素/释放消费者沿用",
        "new_actual_consumer":1, "full_function_equivalence_added":0,
        "proven_closed_do_not_recount": ["primary102/102源码路径、pair阻断0", "60timed标签已有源码待最终统一验证", "同资源非零profile/timeline复用及secondary533", "成功跨PSB surface替换", "material/force/spring设置", "reference持久owner/clock、mouth/transition、六composite共享range", "926/1029/1030输出、933当前值主路径", "1027/1028/1047/1048共享action与965已证空程序分支"],
        "actual_game_path_remaining": "本批已钉住并修复918顺序缺口；当前证据不能枚举所有动态live状态，亦不能证明NEKO已无游戏阻断。5模型声明guard0不是135模型像素成功，更不是0缺源码。",
        "conditional_source_batches": {"M2_Emote_optimistic_conservative":estimates(m2),
            "EFK_optimistic_conservative":estimates(efk), "shared_save_optimistic_conservative":estimates([shared]),
            "known_A_backlog_optimistic_conservative":estimates(all_rows),
            "unit": "类似本轮窄生产→消费源码批；最乐观合并共核，保守拆资源/生命周期/渲染子链；人工规划假设，不是统计预测",
            "conditions": ["真实producer/输入与固定PE契约可获得", "主线接点并行合并无返工", "不新增跨版本和资源格式族"],
            "not_an_upper_bound": True, "excludes": ["统一编译/类型检查/测试/游戏验收和由此发现的回修", "尚未识别的资源/动态分支", "其他代理、平台和其它游戏模块"],
            "complete_game_eta": "无法由静态账本给小时/百分比或可靠总批次；NEKO必经子集可能少于完整A后端，而未知动态命中也可能增加工作"},
        "exceptional_domain_not_independent_game_feature": "nonfinite/CRT best-fit/越界/未初始化原版缺陷保留诊断；不按每个error算函数，也不为缩数实现静默0",
        "no_new_resource_or_game_scan":True, "compiled":False, "typechecked":False, "grammar_reader_run":False,
        "tests_run":False, "Rust_VM_PE_game_APK_device_or_pixel_execution":False,
    }
    payload = {"summary":summary,"M2_Emote":m2,"Effekseer":efk,"shared":[shared],
        "perPE":[{"sha256":"c71c5b85e6eca660474f156d96b0021a53a5f05690dd077d7861d72ff5b11adc","role":"NEKO"},
            {"sha256":"85eb459e94e74ab608f649eb7fb440f2526e90aaa1af7276ed01dc10018903ce","role":"NEKO driver"},
            {"sha256":"63de0e14cd8ce11ad2d68e7ed6076641ff5e5b73d822603cc52764009d812976","role":"Yukikoi EFK"}]}
    PREFIX.with_suffix(".json").write_text(json.dumps(payload,ensure_ascii=False,indent=2)+"\n",encoding="utf8")
    PREFIX.with_name(PREFIX.name+"_summary.json").write_text(json.dumps({"summary":summary,"perPE":payload["perPE"]},ensure_ascii=False,indent=2)+"\n",encoding="utf8")
    md = ["# A域当前剩余（2026-11-18）", "",
        "已知仍部分实现：M2/E-mote 8个共享行为族、EFK 11族、二者活对象存读档合计另1族，共20个去重规划单元。**不是20个函数，也不是穷尽所有缺口。**旧9族摘要混有已完成surface/secondary/force，不继续照搬。",
        "", "本批先落源码：918@790337在906@790643前设置的filter原被m2_owner检查丢掉；已按237d7e→17d160→17a8d0空wrapper生命周期去除此门控。见recovery_A_20261117_emote_dynamic_commands.md，CPU约2585行；无需新hook。", "",
        "|族|确定仍缺行为|实际命中/依赖边界|最乐观/保守源码批|", "|---|---|---|---|"]
    for item in m2:
        md.append(f"|{item['id']} {item['family']}|{item['still_missing']}|{item['actual_hit_scope']}；依赖{item['core_dependency']}|{' / '.join(map(str,item['conditional_source_batches_optimistic_conservative']))}|")
    md.extend(["", "EFK仍11族："+"；".join(i["family"]+"（"+i["branch"]+"）" for i in efk)+"。", "",
        "EFK唯一1014@844116的task=u32(R+680)、resource=R+8、R=2620+1180*i已定，但具体成员未定；上述11族没有实际效果特征命中数，不冒称全是当前Yukikoi阻断。依赖精确资源producer、共享RNG/实例/renderer及宿主图形状态。", "",
        "S1：M2/EFK活对象存读档还缺完整状态序列化/恢复与主线对象图接线，另估2/4批；不把playback_info当存档。", "",
        "**条件批次数**：M2 14/29批，EFK 14/28批，共用存档2/4批，合计最乐观30、保守61个类似窄源码批。它们是按以上已知子链拆分的人工规划假设，不是已测工时或总工作上界。前提是实际输入/契约可取得、无新版本族、接线无返工；不含统一构建测试验收、未知动态分支及其它代理模块。不能据此给完整游戏小时数/百分比。若只收NEKO已证首屏/正文子集可少很多，但目前没有执行证据可给可靠总批次。", "",
        "不要继续计缺：primary102/102源码、60timed已有源码待验证、同资源profile/timeline和secondary链、成功跨PSB surface、force/material设置、mouth/transition/reference、六composite共享range、查询/action/空program分支。5PSZ声明guard0不是所有动态状态均成功。原版异常/nonfinite/未初始化/unsafe域单列边界，不按提示数累计函数。", "",
        "JSON每族有source.path/line/anchor和已有evidence可导航；本清单仅静读现有源码/摘要，没有重扫资源。所有运行结论仍待统一最终验证。本批不编译、类型检查、grammar读取、测试或执行任何目标/求值器。脚本已归analysis、报告归reports，临时PE/KCS证据归archive/2026-11日期文件。",
    ])
    PREFIX.with_suffix(".md").write_text("\n".join(md)+"\n",encoding="utf8")
    print(json.dumps({"written":str(PREFIX.relative_to(ROOT)),"families":summary["known_partial_behavior_families"],"conditional_batches":estimates(all_rows)},ensure_ascii=True))


if __name__ == "__main__":
    main()

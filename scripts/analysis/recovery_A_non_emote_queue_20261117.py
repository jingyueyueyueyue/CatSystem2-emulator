"""Bounded, read-only correction of nine ordinary-slot receiver identities.

No original function, VM, game, network operation, compiler or test is run.
RTTI and vtable consumers are evidence; wrappers alone are not completion.
"""
import json
import struct

from recovery_A_20261116 import ROOT, Image, sha
from recovery_A_receiver_contracts_20261116 import block


def main():
    binary = "Yukikoi Melt/YukikoiMelt.exe"
    image = Image(ROOT / binary,
                  "63de0e14cd8ce11ad2d68e7ed6076641ff5e5b73d822603cc52764009d812976")

    def receiver(vtable, expected):
        col = struct.unpack("<I", image.get(vtable - 4, 4))[0] - image.base
        descriptor = struct.unpack("<5I", image.get(col, 20))[3] - image.base
        name = image.get(descriptor + 8, 100).split(b"\0", 1)[0].decode("ascii")
        if name != expected:
            raise ValueError("receiver RTTI changed")
        command = struct.unpack("<I", image.get(vtable + 0x1c, 4))[0] - image.base
        return {"vtable_rva": vtable, "COL_rva": col,
                "type_descriptor_rva": descriptor, "RTTI": name,
                "command_receiver_rva": command}

    owners = {
        "Effekseer": receiver(0x4ee930, ".?AVkcTaskEffekseer@@"),
        "Tweet": receiver(0x4eebb0, ".?AVkcTaskTweet@@"),
    }
    factory = struct.unpack("<66I", image.get(0x136428, 66 * 4))
    if factory[61] - image.base != 0x1362fd or factory[62] - image.base != 0x136334:
        raise ValueError("factory table changed")
    owners["Effekseer"].update(task_type=61, constructor_rva=0x18b9f0,
                               allocation_bytes=0x2f4, update_rva=0x18bed0)
    owners["Tweet"].update(task_type=62, constructor_rva=0x18df90,
                          allocation_bytes=0x2688, initial_status=8)
    if struct.unpack("<2f", image.get(0x4ee9a8, 8)) != (0.0625, -0.0625):
        raise ValueError("Effekseer coordinate scale changed")
    cases = [
        (1014, 0x1ac7e0, 8, 578, "Effekseer", 0x18c444,
         "Stop old handle if effect exists; set handle=-1, active=0; Release and null old effect; nonempty path loads via d3850 with scale1.0. Resource category is efk, texture category efktex. This is not a PSB/M2 loader."),
        (1015, 0x1ac860, 4, 579, "Effekseer", 0x18c4ef,
         "If effect exists, Release via virtual+8 and clear effect pointer. This case does not itself clear the playback handle or active flag."),
        (1016, 0x1ac8a0, 8, 580, "Effekseer", 0x18c513,
         "If effect exists, retain repeat flag at2ec; manager virtual138 plays effect at (x/16,-y/16,z), writes handle2c4; helper18c6d0 applies packed color2f0 via virtuald0; active2e8=1. Update18bed0 queries Exists(virtuala4); on natural end Stop(virtual94), clears handle/active, and if repeat!=0 replays at current XYZ and reapplies color."),
        (1018, 0x1aca60, 16, 583, "Tweet", 0x18f38f,
         "Non-null owned payload copies2048 bytes into task+78. Wrapper formats first two CP932 strings and constructs a trailing1024-byte record; encoding helper semantics still require recovery."),
        (1019, 0x1ac960, 8, 584, "Tweet", 0x18f3c2,
         "Append NUL-terminated payload to first empty of four512-byte records at87c, with full-table no-op. Original copy is unbounded; safe host must reject overflow. Wrapper performs additional singleton66c700 transformation."),
        (1020, 0x1ac920, 8, 585, "Tweet", 0x18f424,
         "Append escaped CP932 text into first empty of four64-byte records at107c. DBCS pairs are preserved; backslash consumes next byte. Original has unsafe bounds/trailing escape behavior; do not invent safe native equivalence."),
        (1021, 0x1acb60, 4, 586, "Tweet", 0x18f4df,
         "Issue actual task queries22 and96, then if worker74 absent reset status878 and allocate worker(callback18f800). The callback contains twitter OAuth/network/UI flows; queueing alone is not completion. Wrapper reads arg1 despite registered argument_bytes4; VM alias contract unresolved."),
        (1022, 0x1aca20, 4, 587, "Tweet", 0x18f57a,
         "Write actual task status at878 to output; wrapper EAX=1 is the Value return-kind tag, not a fixed script result1. Other eight use Continue tag2. Initial and asynchronous status producers must be retained."),
        (1023, 0x1acb20, 8, 588, "Tweet", 0x18f59b,
         "Replace escaped CP932 record at127c for nonempty payload; same DBCS/backslash decoder as585. Empty payload leaves record unchanged; do not silently clear it."),
    ]
    rows = [{"slot": slot, "wrapper_rva": rva, "argument_bytes": size,
             "message": code, "receiver_family": owner, "case_rva": branch,
             "contract": contract, "implementation_status": "receiver_evidence_only"}
            for slot, rva, size, code, owner, branch, contract in cases]
    spans = {
        "factory_dispatch_table": (0x1357a1, 0x13),
        "effekseer_constructor_vtable_and_initial_fields": (0x18ba16, 0xdc),
        "effekseer_factory_constructor_call": (0x1362fd, 0x37),
        "effekseer_command_cases": (0x18c444, 0x1bd),
        "effekseer_resource_load": (0xd3850, 0xa6),
        "effekseer_actual_loop_lifecycle": (0x18bee8, 0xda),
        "effekseer_handle_color_setter": (0x18c6d0, 0x4b),
        "effekseer_destructor_shared_manager_release": (0x18bd45, 0x9b),
        "effekseer_original_magic_compare": (0xd3e5a, 0x3d),
        "tweet_command_dispatch_and_cases": (0x18f350, 0x2dc),
        "tweet_async_worker_callback": (0x18f800, 0xc2),
        "tweet_wrapper_worker_declared_arg_overread": (0x1acb60, 0x3e),
        "tweet_wrapper_status_return": (0x1aca20, 0x34),
        "tweet_actual_initial_state": (0x18dffd, 0x4d),
        "tweet_destructor_joins_worker_and_releases_client": (0x18e450, 0x7a),
    }
    summary = {"ordinary_slots_reviewed": 9, "Emote_M2_receivers": 0,
               "Effekseer_slots": [1014, 1015, 1016],
               "Tweet_slots": [1018, 1019, 1020, 1021, 1022, 1023],
               "completed_native_count_added": 0, "source_backend_added": False,
               "queue_classification_correction_only": True,
               "compiled": False, "tests_run": False, "target_execution": False,
               "network_or_message_sent": False}
    status_writes = [{"rva": i.address - image.base, "instruction": i.mnemonic + " " + i.op_str}
                     for i in image.insns(0x18f800, 0xc0c)
                     if "0x878]" in i.op_str and i.mnemonic == "mov"]
    payload = {"schema": "cs2-recovery-A-non-emote-queue/v1", "summary": summary,
               "perPE": [{"path": binary, "sha256": image.digest,
                          "ordinary_table_rva": 0x6270e8,
                          "factory_rva": 0x135750, "factory_jump_table_rva": 0x136428,
                          "receivers": owners, "contracts": rows,
                          "Effekseer_magic": {"RVA": 0x5e5824, "bytes": image.get(0x5e5824, 4).hex(),
                                              "ascii": "SKFE", "PSB": False},
                          "Tweet_status_producer_writes": status_writes,
                          "Tweet_status_values_have_no_invented_enum_names": True}],
               "evidence": {key: block(image, *value) for key, value in spans.items()},
               "notes": ["Names verified through MSVC COL/type-descriptor/vtable, not nearby strings alone.",
                         "0xa71c00 is virtual zero-fill/global scratch, not a verified immutable empty-string literal.",
                         "No source routing or implementation status was changed; no PE bytes redistributed."],
               "sources": [{"path": "scripts/analysis/recovery_A_non_emote_queue_20261117.py"}]}
    payload["sources"][0]["sha256"] = sha((ROOT / payload["sources"][0]["path"]).read_bytes())
    prefix = ROOT / "reports/recovery_A_20261117_non_emote_queue"
    prefix.with_suffix(".json").write_text(json.dumps(payload, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    prefix.with_name(prefix.name + "_summary.json").write_text(
        json.dumps({"schema": payload["schema"], "summary": summary, "perPE": payload["perPE"]},
                   ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    md = ["# A / 九项普通槽实际接收者更正", "",
          "9项均已找到真实接收者，但本批未实现完整native，新增完成数0。不能继续将这9项合称Emote缺口。", "",
          f"固定PE：`{binary}`，SHA256 `{image.digest}`，普通表RVA `6270e8`。", "",
          "| 族 | RTTI | 虚表RVA | 接收RVA |", "| --- | --- | --- | --- |"]
    md += [f"| {name} | `{owner['RTTI']}` | `{owner['vtable_rva']:x}` | `{owner['command_receiver_rva']:x}` |" for name, owner in owners.items()]
    md += ["", "| 槽 | 消息 | 包装RVA | 接收分支RVA | 实际契约 |", "| --- | --- | --- | --- | --- |"]
    md += [f"| {r['slot']} | {r['message']} | `{r['wrapper_rva']:x}` | `{r['case_rva']:x}` | {r['contract']} |" for r in rows]
    md += ["", "Effekseer只读身份：构造函数`18b9f0`写虚表`4ee930`；RTTI为kcTaskEffekseer，构造调用`136328`，资源类别efk/efktex。不能交给PSB解析器冒充执行。",
           "", "工厂`135750`、跳表`136428`精确确认Effekseer=type61、Tweet=type62（仅此SHA，不推广跨版）。Effekseer格式magic为SKFE（RVA5e5824），实际比较d3e5a；其repeat字段2ec在18bf3e消费，不能当持续时间。",
           "", "1016实际生命周期：manager+138 Play→handle2c4→color2f0 setter18c6d0；update+8接收18bed0→manager+a4 Exists，结束后manager+94 Stop，repeat非零则以最新XYZ重播并恢复色调。位置常量RVA4ee9a8/4ee9ac精确为+1/16、-1/16。",
           "", "Tweet只读身份：COL `6075ec`指向kcTaskTweet，虚表`4eebb0`的+1c精确指向`18f350`。实际异步回调`18f800`包含OAuth/Twitter工作及UI；本批没有执行或发送。",
           "", "1022状态真实初值8（18e029），启动worker时0；固定callback中的已确认写入为" + json.dumps(status_writes) + "。未给数值猜测枚举名。析构18e450实际等待/回收worker74并释放client167c；仅读字段/构造种子不能替代这些live生产链。",
           "", "注意：wrapper使用`a71c00`全局临时区，它处于虚拟零填充区，不能当永久只读空字符串；1018–1023消息payload必须复制为owned bytes。",
           "", "本批不修改主统计/接线，不增完成计数。脚本已分类scripts/analysis，正式证据归reports；无临时文件、新目录、游戏资产或根README修改。未编译、测试、运行。", ""]
    prefix.with_suffix(".md").write_text("\n".join(md), encoding="utf-8")
    print(json.dumps(summary))


if __name__ == "__main__":
    main()

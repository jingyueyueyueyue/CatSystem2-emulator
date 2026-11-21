# 09 与 20 首场接续

09 的 23:40 和 23:58 回执均在 f3238 验到 `sc01 18/102`。20 的 23:11 回执在 f2724 验到 `A_01_01 2/26`。以下档位均从新游戏起步，便于复查入口、每次物理输入和终点。 [09 23:58](F:/CS2-build-cache/playthrough/2026-11/20261120_235848_203818_u4liwo6v_playthrough/09.json) · [20 23:11](F:/CS2-build-cache/playthrough/2026-11/20261120_231101_690425_x8sj7k9f_playthrough/20/20.host.log) · [原始 CST 步骤表](F:/CS2-build-cache/archive/2026-11/20261121_pc_story_09_20_scene_audit.json)

| 档位 | 正文与等待 | 验收 |
| --- | --- | --- |
| 09 `r134` | `18/102 → 19/118 → … → 27/134`。r108、r113 各有 `rwipe wipe01a 100`，r115 有 `wait 60`。 | 共 28 个正文等键、27 次 Enter。 |
| 09 `r256` | 延续至 `51/256`。从 `28/141` 起覆盖出场和语音，r250–253 还有角色层、draw 及 `pcm c_chisato_0011`。 | 共 52 个正文等键、51 次 Enter。 |
| 09 `sc02_r10` | `sc01 293/1324 → sc02 0/10`。离场有 `wipe04a 100`、`wipe07a 240`、两次 `rdraw 100` 和 `wait 60/240/60`，r1354 执行 `next sc02`。入场后有 `rdraw 100`、`wait 60`，r9 正文 50 字节。 | 共 295 个正文等键、294 次 Enter。核对新 CST load 的摘要和读入顺序。 |
| 20 `r61` | `2/26 → … → 10/48 → 11/58 → 12/61`。r53 有 `rwipe WIPE01 120`。 | 共 13 个正文等键、12 次 Enter。 |
| 20 `r189` | `12/61 → 13/150 → … → 26/189`。r66 有 `rwipe WIPE31 90`，随后是日期层和进窗。r181–186 有两次短 draw，r188 正文 96 字节。 | 共 27 个正文等键、26 次 Enter。 |
| 20 `novel_r211` | 从相同长档的 `26/189` 送一次 Enter，经 `WIPE50 120 → rdraw 120 → novel on → view 0 novel01 → frameon fade 30`。 | 核对 `27/211` 和 r210 的 39 字节 GBK 读入。记录运行中的 FES 指纹、owner、接收者解析、提示、正文和合成状态，等待观察完成。 |

09 每一步继续核对同一消息窗、实际 region 60 → task 8、物理 Enter 唯一消费、根 `0xcae` 同帧恢复、扫描释放、CST 原记录、最新自然完成的正文及提示像素。跨场仍要求原 owner、region、receiver 连续存活，并要求 load ID 增长和实际加载晚于离场按键。若场边界重建任一任务，记录旧任务及新任务的创建、父链与物理接收者后再定接续守卫。`sc02 0/10` 之后可先加 `1/12` 和 `2/15`，r11、r14 正文分别为 126、46 字节。后段出现多正文或正文与等待不相邻的组合，步骤表保留每个等键的正文数组。

20 的普通 `meswnd01` 每页核对活 `sscript` 的 bottom button、`MES_SETSTATE`、`MES_WAITINPUT`、同一 CST load、当前代自然完成和正文/提示合成，以及根 `0xad0` 的消费回执。两种 `novelwnd01*.fes` 已按安装资源解包。`MES_WAITINPUT` 显示 `keywait2` 并设本地槽 100 为 1，`MES_WAITPAGE` 显示 `keywait` 并设槽 100 为 2。应以实跑确认 native 630 的实际命令、提示像素、正文追加区间和接收者，再把各模式作为独立门槛。

1. 先用 `novel_r211` 确认窗体改建后的第一处 `0x02`。收到 `novel_r211_observed` 后检查诊断中的 `binding`、`generation`、`sources`、`composited` 及 `keywait2`。只有正文确实完成并提交、新窗实际提示可见且接收者无歧义，才具备验证 `28/213` 的条件。两页正文分别为 39 和 59 字节。
2. 首次页边界独立停 `29/215`，其余三处为 `32/225`、`37/238`、`41/249`。先确认运行分支使用的 `MES_WAITPAGE`、槽 100 和实际 `keywait`，再从对应边界单步验证清页和下一条 GBK 正文的独立提交。保留累积 novel 页当前代与当前段的关联。
3. 到 `46/282` 后，r292 `novel off`、r293 `view 0 wnd01`，经 `rdraw 120` 回到 `47/301`。此处先观测恢复的 `meswnd01` 身份，之后从新 owner 与 receiver 重新建立普通等键判据。后续普通段可按表从 `47/301` 走至 `122/610`。
4. 从 `122/610` 只发一次经验证的 Enter，见 r615 `WIPE31A 90`、r617 `wait 180` 及媒体和覆盖层清理，r641 自然执行 `next A_01_02`。新场的 GBK 覆盖白名单与探针需增列经摘要核对的 `scene.int::A_01_02.cst`。核对新 load、`WIPE03B 90`、日期层、`meswnd01` 当前 owner 和活动接收者，先停 `0/85`，再按同场规则接 `1/89 → 2/93`。这三页正文长度为 28、39、27 字节。

## 合批和离线核验

09 与 20 的主候选可各自合入，r211 观察候选依赖 20 主候选。主线程已合入的补丁可用 `git apply --reverse --check` 确认。 [09](F:/CS2-build-cache/archive/2026-11/20261121_story_09_kanmi_sc02_continuation.patch) · [20 r189](F:/CS2-build-cache/archive/2026-11/20261121_story_20_rebellions_r189_continuation.patch) · [20 r211](F:/CS2-build-cache/archive/2026-11/20261121_story_20_rebellions_r211_observation.patch)

```powershell
$analysis = 'D:/迅雷下载/CS2/scripts/analysis'
$archive = 'F:/CS2-build-cache/archive/2026-11'
$mode09 = 'r256' # r134, r256, sc02_r10
$mode20 = 'r189' # r61, r189, novel_r211
$timeout09 = if ($mode09 -eq 'sc02_r10') { 2400 } else { 600 }
python -B "$analysis/run_playthrough_batch.py" --player $StoryPlayer --indices 9 --scenarios "$archive/20261121_pc_story_09_$mode09.json" --jobs 1 --timeout $timeout09
python -B "$analysis/run_playthrough_batch.py" --player $StoryPlayer --indices 20 --scenarios "$archive/20261121_pc_story_20_$mode20.json" --jobs 1
python -B "$analysis/check_pc_story_09_continuation.py" $Run09 --mode $mode09
python -B "$analysis/check_pc_story_20_opening_segment.py" $Run20 --mode $mode20
```

检查器确认终点零后续键、零截图。09 输出 `continuation_verified`，跨场另输出 `scene_handoff`。20 输出 `r61_verified`、`r189_verified` 或 `novel_r211_observed`。后者记录换窗后的状态，正文合成和新接收者仍以诊断实值为准。长档帧数与跨场墙钟预算只限制观察时长。三份补丁已过隔离应用和 Rust 语法解析，新档位由主线程构建和回放。

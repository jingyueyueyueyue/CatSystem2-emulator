# 09、12、20 PC 接续

00:24 批的 09 `sc01 27/134`（f4303）和 20 `A_01_01 12/61`（f3291）均通过独立检查。12 最近完成的 `op001 20/222`（f1466）沿用原回执。 [00:24 批](F:/CS2-build-cache/playthrough/2026-11/20261121_002446_022100_phfaym7b_playthrough) · [12 r222 批](F:/CS2-build-cache/playthrough/2026-11/20261120_235848_203818_u4liwo6v_playthrough)

| 样本 | 可调度候选 | 守卫与待观察条件 |
| --- | --- | --- |
| 09 | `sc02_r147`、`sc02_r547` | 使用现有 sc01 → sc02 交接，分别覆盖 sc02 前 26、101 个连续单来源等键。每个同场后继要求同 load、自然提交、原活接收者、唯一消费和扫描释放。首个不同的构型是 `101/562`，需核对 r555、r560、r561 的实际入队与合成顺序。r1379 后原脚本按 `#759` 去 `sc02_01` 或 `sc02_02`，对应两种片头播放路径，均进 `sc03`。 |
| 12 | `r261`、`r619`、`post_op_r42` | r261 验 r222 后九个等键及 r232 的绘制。r619 覆盖余下 32 个等键，按原始非空来源核对六处非相邻记录。r42 档在 r619 后只发一枚 Enter，绑同一片头 handle 的加载、播放、自然结束与释放，随后核对 `OPMOVIE_END 0/42` 的新 load、当前代和活绑定。该页守卫通过后可接 `1/46 → 2/51`，再按源表准备到 `417/3759 → op002`。 |
| 20 | 现成 `r189`、`novel_r211`，之后接续到 `A_01_02` | 00:24 批确认 `WIPE01 120`，从 r61 的下一道 `WIPE31 90` 开始继续。r211 先看 novel FES 与接收者，按普通和 `0x03` 两类独立验收。切回 `wnd01` 后，`47/301 → 122/610` 有 76 个普通等键，重新建立当前窗的任务与来源守卫。过场后 `A_01_02` 另有 89 个普通单来源等键，首档停 `0/85`，后续可直至 `88/583 → A_01_03`。 |

20 的 **运行修复** 是为胜出的 `scene.int::A_01_02.cst` 补 GBK 身份。原场景的每条记录均通过 GBK 解码，40 个正文记录会被 CP932 拒绝，首处是 r84。补丁只允许精确路径、成员和资源摘要。12 的影片补丁只增加诊断，利用现有播放完成状态区分自然结束和释放。12 的 6 个空记录边界及 09 的 101 个单来源后继属于 **守卫扩展**。跨窗后若实际世代、接收者或任务生命周期不同，保留原始回执再扩守卫。

## 终点证据

- 09 的两组 `sced01*`、`sced02*` 按 `#761/#763`、`#762/#764` 分支，最终执行 `title`。需要独立验收制作名单时间轴、结束卡合成、标题返回，以及各分支在新进程读回的实际通关状态。
- 12 的结束影片由对应 `EDMOVIE` 场景与 `#194` 分支路由，`#612` 在影片脚本中更新。多个 `EDMOVIE_END*` 还会进入尾声。验收需要影片自然释放、尾声或结束卡的自然提交、实际通关状态写回和冷启动读回。
- 20 的 `movie_ed1_end` 至 `movie_ed4_end` 继续进入 `A_06_09`、`B_06_04`、`C_11_02`、`D_10_16`。制作名单完成回执与随后剧情、结束卡及通关状态分别核对。各入口依真实已解锁菜单与原输入接收者推进。

通关回执需记录隔离档写入前、标题返回后及冷启动后可复核的持久状态。以上终点为静态入口定位，PC 结束卡、制作名单与通关写回仍待实跑。

## 合入与执行

09 r547 增量依赖先前 sc02 首等键增量。12 先合 r619，随后合影片回执与 r42 守卫。20 GBK 增量可以单独合入，r189 和 r211 继续沿用已归档候选。 [09 r547](F:/CS2-build-cache/archive/2026-11/20261121_story_09_kanmi_sc02_r547.patch) · [12 r619](F:/CS2-build-cache/archive/2026-11/20261121_story_12_hatsumira_op001_r619.patch) · [12 影片回执](F:/CS2-build-cache/archive/2026-11/20261121_story_12_hatsumira_op_movie_receipts.patch) · [12 r42](F:/CS2-build-cache/archive/2026-11/20261121_story_12_hatsumira_post_op_r42.patch) · [20 GBK](F:/CS2-build-cache/archive/2026-11/20261121_story_20_rebellions_a01_02_gbk.patch)

```powershell
$analysis = 'D:/迅雷下载/CS2/scripts/analysis'
$archive = 'F:/CS2-build-cache/archive/2026-11'
$mode09 = 'sc02_r147' # sc02_r147, sc02_r547
$mode12 = 'r619' # r261, r619, post_op_r42
$mode20 = 'r189' # r189, novel_r211
python -B "$analysis/run_playthrough_batch.py" --player $StoryPlayer --indices 9 --scenarios "$archive/20261121_pc_story_09_$mode09.json" --jobs 1 --timeout 3000
python -B "$analysis/run_playthrough_batch.py" --player $StoryPlayer --indices 12 --scenarios "$archive/20261121_pc_story_12_$mode12.json" --jobs 1 --timeout 1200
python -B "$analysis/run_playthrough_batch.py" --player $StoryPlayer --indices 20 --scenarios "$archive/20261121_pc_story_20_$mode20.json" --jobs 1
python -B "$analysis/check_pc_story_09_continuation.py" $Run09 --mode $mode09
python -B "$analysis/check_pc_story_12_op001.py" $Run12 --mode $mode12 --require-cache-reset
python -B "$analysis/check_pc_story_20_opening_segment.py" $Run20 --mode $mode20
```

09 跨场档要求 `continuation_verified` 和 `scene_handoff`。12 当前可选状态为 `r261_verified`、`r619_verified`、`post_op_r42_verified`。20 的 `novel_r211_observed` 记录诊断实值，必须确认新窗合成和真实接收者后才从 r211 推进。所有候选的帧数只限制观察时长，终点零后续按键。补丁仅做隔离应用及语法检查，由主线程编译与限流调度。
